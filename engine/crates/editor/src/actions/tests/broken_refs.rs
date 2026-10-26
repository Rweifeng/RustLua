//! P1-3b：断链审计（扫描/聚合/负控）与「同名唯一命中」一键改指的回归测试。
//!
//! 这些测试刻意跑在**自建的临时工程**上（两份场景 + 一张同名贴图），而不是示例工程：
//! 断链的数量与归属必须由夹具决定，示例工程的内容会随演示增删。

use super::*;
use crate::EditorApp;
use editor::{CommandResult, EditorCommand, SceneDocument, open_project};
use rustlua_infra::logging::LogSink;
use rustlua_scene::scene::{Scene, SceneEntityId};

/// 引用被搬走的那条路径（磁盘上没有这份文件）。
const BROKEN: &str = "assets/missing/player.png";
/// 工程里唯一命中的同名文件（改指的目标）。
const CANDIDATE: &str = "assets/hero/player.png";

fn sprite(payload: &str) -> (&'static str, String) {
    (
        "Sprite",
        format!("texture={payload};size=1x1;tint=1,1,1,1;sorting_layer=default"),
    )
}

/// 写一份带 Sprite 引用的场景（每条引用一个实体）。
fn write_scene(path: &Path, references: &[&str]) {
    let mut scene = Scene::new("fixture");
    for (index, reference) in references.iter().enumerate() {
        let id = SceneEntityId(index as u64 + 1);
        scene
            .add_entity(id, format!("E{}", index + 1), None)
            .unwrap();
        let (component, payload) = sprite(reference);
        scene
            .entities
            .get_mut(&id)
            .unwrap()
            .components
            .insert(component.to_owned(), payload);
    }
    scene.save(path).unwrap();
}

/// 临时工程：`project.toml` + 一张 `assets/hero/player.png` + 两份指向 `BROKEN` 的场景。
///
/// `index_assets` = 是否真的建立资源索引。关掉它就是在测"索引还没建好"那一档：
/// 扫描必须**拒绝声称没有断链**，而不是给出一屏假断链。
fn fixture(label: &str, index_assets: bool) -> (EditorApp, PathBuf) {
    let source = sample_project_root();
    assert!(
        source.join("project.toml").is_file(),
        "sample project missing: {}",
        source.display()
    );
    let root = temp_root(label);
    // `open_project` 认这三个目录（缺一个就报 MissingDirectory），夹具先把骨架搭好。
    fs::create_dir_all(root.join("assets/hero")).unwrap();
    fs::create_dir_all(root.join("scenes")).unwrap();
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::copy(source.join("project.toml"), root.join("project.toml")).unwrap();
    fs::write(root.join(CANDIDATE), b"\x89PNG\r\n\x1a\n not a real png").unwrap();
    write_scene(&root.join("scenes/main.scene"), &[BROKEN]);
    write_scene(&root.join("scenes/other.scene"), &[BROKEN, CANDIDATE]);
    let project = open_project(&root).expect("临时工程应可打开");
    let mut app = EditorApp::new(project, None, LogSink::new(64));
    app.project_loaded = true;
    if index_assets {
        app.resources.refresh(&root).expect("资源索引应可建立");
    }
    (app, root)
}

fn indexed_fixture(label: &str) -> (EditorApp, PathBuf) {
    fixture(label, true)
}

/// 打开磁盘文档，让它以**内存版**参与统计（`path` 决定改指时是否被跳过）。
fn open_document(app: &mut EditorApp, root: &Path, relative: &str) {
    let absolute = root.join(relative);
    let mut document = SceneDocument::open(&absolute).expect("打开场景");
    document.path = Some(absolute);
    app.document.current_scene = Some(document);
}

/// 一份文档里第一条 Sprite 的 `texture=` 取值（断言"改写落到哪一份"用）。
fn texture_of(root: &Path, relative: &str, entity: u64) -> String {
    let scene = Scene::load(root.join(relative)).unwrap_or_else(|error| {
        panic!("读回 {relative} 失败：{error}");
    });
    let payload = &scene
        .entities
        .get(&SceneEntityId(entity))
        .unwrap()
        .components["Sprite"];
    payload
        .split(';')
        .find_map(|segment| segment.strip_prefix("texture="))
        .unwrap_or("")
        .to_owned()
}

#[test]
fn scan_aggregates_one_broken_path_across_documents() {
    let (mut app, root) = indexed_fixture("scan-aggregate");
    // 不打开任何文档：两份场景都从磁盘读。
    assert_eq!(
        app.dispatch(EditorCommand::ScanBrokenReferences),
        CommandResult::Applied
    );
    let rows = &app.broken_refs.rows;
    assert_eq!(rows.len(), 1, "同一条断掉的路径只该出现一行：{rows:?}");
    assert_eq!(rows[0].value, BROKEN);
    assert_eq!(
        rows[0].consumer_count, 2,
        "两处引用都要计进总数（样本可以少，总数不能少）"
    );
    assert_eq!(
        rows[0].consumers.len(),
        2,
        "消费者证据：{}",
        rows[0].consumers.join(" / ")
    );
    assert!(
        rows[0]
            .consumers
            .iter()
            .all(|consumer| consumer.contains("Sprite.texture")),
        "证据要指到字段：{}",
        rows[0].consumers.join(" / ")
    );
    // 同名唯一命中 → 这才是"可以一键改指"的那一条。
    assert_eq!(rows[0].candidates, vec![CANDIDATE.to_owned()]);
    assert_eq!(app.broken_refs.fixable_count(), 1);
    assert_eq!(app.broken_refs.scanned_documents, 2);
    assert!(app.broken_refs.unreadable.is_empty());
    assert!(app.status.contains("2 份文档"), "{}", app.status);
    let _ = root;
}

#[test]
fn a_missing_index_never_claims_there_are_no_broken_links() {
    // 负控：资源索引还没建立时，扫描必须**不产出清单**也不把 `scanned` 打开。
    // 否则面板上那句"没有断链"其实是在说"我没看得见"。
    let (mut app, root) = fixture("scan-no-index", false);
    assert_eq!(
        app.dispatch(EditorCommand::ScanBrokenReferences),
        CommandResult::NoOp
    );
    assert!(!app.broken_refs.scanned, "没扫成就不算扫过");
    assert!(app.broken_refs.rows.is_empty());
    assert!(
        app.status.contains("资源索引"),
        "状态要说清为什么没结果：{}",
        app.status
    );
    let _ = root;
}

#[test]
fn a_builtin_sentinel_reference_is_not_reported_as_broken() {
    // `__white__` 是运行时认识的内置纹理名，永远不在资源索引里；把它算成断链
    // 会让清单里躺着一堆永远修不好的行（Inspector 侧同一个判据，见 mark_missing_assets）。
    let (mut app, root) = indexed_fixture("scan-sentinel");
    write_scene(
        &root.join("scenes/sentinel.scene"),
        &[rustlua_render::frame::WHITE_TEXTURE_PATH],
    );
    assert_eq!(
        app.dispatch(EditorCommand::ScanBrokenReferences),
        CommandResult::Applied
    );
    assert_eq!(
        app.broken_refs
            .rows
            .iter()
            .map(|row| row.value.clone())
            .collect::<Vec<_>>(),
        vec![BROKEN.to_owned()],
        "内置哨兵不该出现在断链清单里"
    );
    assert_eq!(
        app.broken_refs.scanned_documents, 3,
        "第三份文档要真的被读到（否则只剩 BROKEN 是因为根本没扫）"
    );
}

#[test]
fn an_unreadable_document_is_named_not_silently_dropped() {
    let (mut app, root) = indexed_fixture("scan-unreadable");
    fs::write(root.join("scenes/corrupt.scene"), "not a scene at all\n").unwrap();
    assert_eq!(
        app.dispatch(EditorCommand::ScanBrokenReferences),
        CommandResult::Applied
    );
    assert_eq!(
        app.broken_refs.unreadable.len(),
        1,
        "{:?}",
        app.broken_refs.unreadable
    );
    assert!(
        app.broken_refs.unreadable[0].contains("scenes/corrupt.scene"),
        "要点名是哪份文档：{}",
        app.broken_refs.unreadable[0]
    );
    assert_eq!(
        app.broken_refs.scanned_documents, 2,
        "坏文档不计进读过的份数"
    );
}

#[test]
fn repair_rewrites_other_documents_on_disk_and_keeps_the_open_document_unsaved() {
    let (mut app, root) = indexed_fixture("repair-split");
    open_document(&mut app, &root, "scenes/main.scene");
    assert_eq!(
        app.dispatch(EditorCommand::ScanBrokenReferences),
        CommandResult::Applied
    );
    // 当前文档（内存版）+ other.scene（磁盘版）各一处引用。
    assert_eq!(app.broken_refs.rows[0].consumer_count, 2);

    let result = app.dispatch(EditorCommand::RepairBrokenReference {
        from: BROKEN.to_owned(),
        to: CANDIDATE.to_owned(),
    });
    assert_eq!(result, CommandResult::Applied, "{result:?}");

    // 1) 磁盘上那份：直接写回。
    assert_eq!(texture_of(&root, "scenes/other.scene", 1), CANDIDATE);
    // 2) 当前打开的文档：内存里改到了，但**磁盘文件保持旧路径**（skip 语义）——
    //    这条是"未保存的编辑不被改指连带写盘"的守卫。
    let document = app.document.current_scene.as_ref().unwrap();
    let in_memory = &document.scene.entities[&SceneEntityId(1)].components["Sprite"];
    assert!(
        in_memory.contains(CANDIDATE),
        "当前文档没吃到改指：{in_memory}"
    );
    assert!(document.is_dirty(), "改指要让文档变脏（否则会被静默丢掉）");
    assert_eq!(
        texture_of(&root, "scenes/main.scene", 1),
        BROKEN,
        "当前文档的磁盘文件不该被改写"
    );
    // 3) 恰好一条 Undo。
    assert_eq!(app.history.undo_len(), 1, "改指 = 一条撤销");
    assert_eq!(
        app.dispatch(EditorCommand::Undo),
        CommandResult::Applied,
        "Undo 要能回退当前文档里的改指"
    );
    assert!(
        app.document.current_scene.as_ref().unwrap().scene.entities
            [&SceneEntityId(1)]
        .components["Sprite"]
        .contains(BROKEN),
        "撤销后应回到断链原状"
    );
    // 4) 清单把已修掉的那行摘掉，且不自动重扫（重扫的代价不该由一次点击付）。
    assert!(
        app.broken_refs.rows.is_empty(),
        "{:?}",
        app.broken_refs.rows
    );
    assert!(
        app.broken_refs.message.contains("扫描全工程"),
        "要提示清单是旧快照：{}",
        app.broken_refs.message
    );
}

#[test]
fn repair_refuses_an_unknown_target_and_a_path_outside_the_project() {
    let (mut app, root) = indexed_fixture("repair-refused");
    // 目标文件不在盘上：拒绝，且一份文档都不动。
    match app.repair_broken_reference(BROKEN, "assets/hero/nope.png") {
        Err(reason) => assert!(reason.contains("目标资源不存在"), "{reason}"),
        Ok(status) => panic!("目标不存在必须被拒绝：{status}"),
    }
    // 越出工程根的路径进不了闸门（与资源重命名同一口径）。
    match app.repair_broken_reference(BROKEN, "../../outside/player.png") {
        Err(reason) => assert!(reason.contains("工程内相对路径"), "{reason}"),
        Ok(status) => panic!("越界路径必须被拒绝：{status}"),
    }
    assert_eq!(texture_of(&root, "scenes/other.scene", 1), BROKEN);
    assert_eq!(app.history.undo_len(), 0, "拒绝必须不产生历史");
}

#[test]
fn repair_is_refused_while_playing_and_changes_nothing() {
    use editor::PlaySession;
    use editor::PlayState;

    let (mut app, root) = indexed_fixture("repair-playing");
    open_document(&mut app, &root, "scenes/main.scene");
    app.play_session = Some(PlaySession::start(
        app.document.current_scene.as_ref().unwrap(),
    ));
    app.play.set_state_for_test(PlayState::Playing);

    match app.dispatch(EditorCommand::RepairBrokenReference {
        from: BROKEN.to_owned(),
        to: CANDIDATE.to_owned(),
    }) {
        CommandResult::Rejected(reason) => {
            assert!(reason.contains("播放模式"), "{reason}");
        }
        other => panic!("Play 期间的工程级改写必须被拒：{other:?}"),
    }
    assert_eq!(
        texture_of(&root, "scenes/other.scene", 1),
        BROKEN,
        "磁盘不动"
    );
    assert_eq!(app.history.undo_len(), 0, "拒绝不产生历史");

    // 扫描是只读的：Play 期间照样能看清单。
    assert_eq!(
        app.dispatch(EditorCommand::ScanBrokenReferences),
        CommandResult::Applied
    );
}

#[test]
fn clearing_the_panel_forgets_the_previous_project() {
    let (mut app, root) = indexed_fixture("panel-clear");
    assert_eq!(
        app.dispatch(EditorCommand::ScanBrokenReferences),
        CommandResult::Applied
    );
    assert!(app.broken_refs.scanned);
    app.broken_refs.clear();
    assert!(!app.broken_refs.scanned);
    assert!(app.broken_refs.rows.is_empty());
    assert_eq!(app.broken_refs.fixable_count(), 0);
    let _ = root;
}
