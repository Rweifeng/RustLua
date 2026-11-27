//! 安全删除（反向依赖守卫）的回归测试。
//!
//! 四条门：
//! 1. 被引用的资源**一次 Delete 不删**，并把消费者（哪份文档的哪个组件）报出来；
//! 2. 没被引用的资源一次 Delete 就删（守卫不该给最常见的路径加一步操作）；
//! 3. 脚本文本与配置里的引用（含音效的**文件名**写法）也要报出来，且报得出行号；
//! 4. 守卫在文档里数到的条数，必须等于"改指会改的条数"——两边口径分叉就等于没有守卫。

use super::*;
use crate::EditorApp;
use editor::open_project;
use rustlua_infra::logging::LogSink;
use rustlua_scene::scene::{Scene, SceneEntityId};

/// 被场景引用的贴图（组件里写的就是这条路径）。
const PLAYER: &str = "assets/hero/player.png";
/// 没人引用的贴图。
const SPARE: &str = "assets/hero/spare.png";
/// 音效（运行时按**文件名**引用：`audio.play("hit")` 这种写法没有路径）。
const HIT: &str = "assets/audio/hit.ogg";

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

/// 临时工程：一张被场景引用的贴图 + 一张没人用的贴图 + 一个音效 + 脚本目录。
///
/// 夹具自己决定引用关系（示例工程的内容会随演示增删，写死它的结构只会制造假失败）。
fn fixture(label: &str) -> (EditorApp, PathBuf) {
    let source = sample_project_root();
    assert!(
        source.join("project.toml").is_file(),
        "sample project missing: {}",
        source.display()
    );
    let root = temp_root(label);
    fs::create_dir_all(root.join("assets/hero")).unwrap();
    fs::create_dir_all(root.join("assets/audio")).unwrap();
    fs::create_dir_all(root.join("scenes")).unwrap();
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::copy(source.join("project.toml"), root.join("project.toml")).unwrap();
    fs::write(root.join(PLAYER), b"png").unwrap();
    fs::write(root.join(SPARE), b"png").unwrap();
    fs::write(root.join(HIT), b"ogg").unwrap();
    write_scene(&root.join("scenes/main.scene"), &[PLAYER]);
    let project = open_project(&root).expect("临时工程应可打开");
    let app = EditorApp::new(project, None, LogSink::new(64));
    (app, root)
}

#[test]
fn a_referenced_resource_is_kept_until_the_user_confirms() {
    let (mut app, root) = fixture("delete-referenced");
    app.select_resource(PathBuf::from(PLAYER));

    app.delete_selected_resource();

    assert!(
        root.join(PLAYER).is_file(),
        "被引用的资源不能一次就删掉：文件还在吗？"
    );
    let guard = app.pending_delete.as_ref().expect("应停在待确认态");
    assert_eq!(guard.reference_count(), 1, "{:?}", guard.references);
    assert!(
        guard.references[0].contains("scenes/main.scene") && guard.references[0].contains("Sprite"),
        "要报出是哪份文档的哪个组件：{:?}",
        guard.references
    );
    assert!(app.status.contains("1 处引用"), "{}", app.status);

    // 第二次确认：用户点了「强制删除」才真删。
    app.force_delete_pending_resource();
    assert!(!root.join(PLAYER).exists(), "确认后必须真删");
    assert!(app.pending_delete.is_none(), "确认后不该还挂着待确认态");
    assert!(app.status.contains("断链"), "{}", app.status);
}

#[test]
fn cancelling_a_pending_delete_keeps_the_file() {
    let (mut app, root) = fixture("delete-cancel");
    app.select_resource(PathBuf::from(PLAYER));
    app.delete_selected_resource();
    assert!(app.pending_delete.is_some());

    app.cancel_pending_delete();

    assert!(root.join(PLAYER).is_file(), "取消就不该删");
    assert!(app.pending_delete.is_none());
    assert!(app.status.contains("已取消删除"), "{}", app.status);
}

#[test]
fn an_unreferenced_resource_is_deleted_in_one_step() {
    let (mut app, root) = fixture("delete-unreferenced");
    app.select_resource(PathBuf::from(SPARE));

    app.delete_selected_resource();

    assert!(
        !root.join(SPARE).exists(),
        "没人引用的资源不该被多一步操作挡住"
    );
    assert!(app.pending_delete.is_none(), "不该停在待确认态");
    assert!(app.status.contains("已删除资源"), "{}", app.status);
}

#[test]
fn a_script_reference_is_reported_with_its_file_and_line() {
    let (mut app, root) = fixture("delete-script");
    fs::write(
        root.join("scripts/main.lua"),
        "-- 玩家贴图\nlocal tex = \"assets/hero/player.png\"\nreturn tex\n",
    )
    .unwrap();
    app.select_resource(PathBuf::from(PLAYER));

    app.delete_selected_resource();

    let guard = app.pending_delete.as_ref().expect("应停在待确认态");
    assert!(
        guard
            .references
            .iter()
            .any(|reference| reference.contains("scripts/main.lua")
                && reference.contains("第 2 行")
                && reference.contains(PLAYER)),
        "脚本文本里的引用要报到文件与行号：{:?}",
        guard.references
    );
    assert!(
        guard.reference_count() >= 2,
        "场景与脚本两处消费者都要报出来：{:?}",
        guard.references
    );
}

/// A2：只被**材质**引用的贴图也必须拦住删除。
///
/// 材质/图集/Spine 清单以前不在守卫的扫描面里，而依赖图早已连上它们的边——
/// 结果是"依赖视图说有引用、删除守卫说没人用"，点一下删除就把图删了。
#[test]
fn a_texture_referenced_only_by_a_material_blocks_deletion() {
    let (mut app, root) = fixture("delete-material");
    fs::create_dir_all(root.join("assets/materials")).unwrap();
    fs::write(
        root.join("assets/materials/steel.material"),
        "shader=pbr\nbase_color=assets/hero/player.png\nmetallic=1\nroughness=0.4\n",
    )
    .unwrap();
    // 把场景里的 Sprite 引用清空，让"只被材质引用"这一条件真的成立。
    write_scene(&root.join("scenes/main.scene"), &[]);
    app.select_resource(PathBuf::from(PLAYER));

    app.delete_selected_resource();

    let guard = app
        .pending_delete
        .as_ref()
        .expect("材质引用必须拦住删除（只登记了 .lua/.toml/.json 就漏了）");
    assert!(
        guard
            .references
            .iter()
            .any(|reference| reference.contains("assets/materials/steel.material")),
        "要报出是哪份材质在引用：{:?}",
        guard.references
    );
    assert!(root.join(PLAYER).is_file(), "被引用时不能一次删掉");
}

/// 图集与 Spine 清单同理：它们是资源描述文档，不是"普通文本"。
#[test]
fn atlas_and_spine_descriptors_are_scanned_too() {
    let (mut app, root) = fixture("delete-descriptors");
    fs::create_dir_all(root.join("assets/atlas")).unwrap();
    fs::create_dir_all(root.join("assets/spine")).unwrap();
    fs::write(
        root.join("assets/atlas/hero.atlas"),
        "texture=assets/hero/player.png\nregion hero\n",
    )
    .unwrap();
    fs::write(
        root.join("assets/spine/hero.spine"),
        "skeleton=assets/models/hero.json\natlas=assets/spine/hero.atlas\npage=assets/hero/player.png\n",
    )
    .unwrap();
    write_scene(&root.join("scenes/main.scene"), &[]);
    app.select_resource(PathBuf::from(PLAYER));

    app.delete_selected_resource();

    let guard = app.pending_delete.as_ref().expect("图集/清单都引用它");
    assert!(
        guard
            .references
            .iter()
            .any(|reference| reference.contains("hero.atlas")),
        "图集引用要报出来：{:?}",
        guard.references
    );
    assert!(
        guard
            .references
            .iter()
            .any(|reference| reference.contains("hero.spine")),
        "Spine 清单引用要报出来：{:?}",
        guard.references
    );
}

#[test]
fn an_audio_reference_is_reported_by_its_file_name() {
    let (mut app, root) = fixture("delete-audio");
    fs::write(root.join("scripts/sfx.lua"), "audio.play(\"hit\")\n").unwrap();
    app.select_resource(PathBuf::from(HIT));

    app.delete_selected_resource();

    let guard = app.pending_delete.as_ref().expect("应停在待确认态");
    assert!(
        guard
            .references
            .iter()
            .any(|reference| reference.contains("scripts/sfx.lua")
                && reference.contains("音效名 \"hit\"")),
        "音效没有路径写法，只能按文件名认：{:?}",
        guard.references
    );
    assert!(root.join(HIT).is_file(), "被引用的音效不能一次就删掉");
}

/// 守卫与改指必须**同一口径**：改指会改的引用数，就是守卫数到的引用数。
///
/// 这条是守卫本身的成立条件：判据分叉就会出现"守卫说没人引用、资源一删就断链"。
/// 脚本文本不在改指的范围内（改指只改文档），所以这里只比**文档**里的那部分。
#[test]
fn the_guard_counts_exactly_what_a_rewrite_would_change() {
    let (mut app, root) = fixture("delete-agrees");
    fs::write(
        root.join("scripts/main.lua"),
        "local tex = \"assets/hero/player.png\"\n",
    )
    .unwrap();
    app.select_resource(PathBuf::from(PLAYER));
    app.delete_selected_resource();

    let guard = app.pending_delete.as_ref().expect("应停在待确认态");
    let document_hits = guard
        .references
        .iter()
        .filter(|reference| reference.starts_with("scenes/"))
        .count();
    let report = editor::rewrite_project_references(&root, PLAYER, "assets/hero/player2.png", None);
    assert_eq!(
        document_hits, report.rewritten_references,
        "守卫数到的文档引用必须与改指会改的条数一致：守卫 {document_hits} / 改指 {:?}",
        report.rewritten_references
    );
    assert_eq!(report.rewritten_references, 1, "场景里只有一条引用");
}

/// A4：**多选批量删除**——没引用的直接删，被引用的跳过并汇总点名。
#[test]
fn batch_delete_removes_only_unreferenced_resources_and_reports_the_rest() {
    let (mut app, root) = fixture("delete-batch");
    // 再加两张没人用的贴图，凑成"3 个可删 + 1 个被引用"。
    for extra in ["assets/hero/a.png", "assets/hero/b.png"] {
        fs::write(root.join(extra), b"png").unwrap();
    }
    app.resources.refresh(&root).expect("扫描临时工程");
    app.select_resource(PathBuf::from(SPARE));
    app.select_resource_with_modifiers(PathBuf::from("assets/hero/a.png"), true, false);
    app.select_resource_with_modifiers(PathBuf::from("assets/hero/b.png"), true, false);
    // 把场景里引用的那张也加进选区：它必须被跳过。
    app.select_resource_with_modifiers(PathBuf::from(PLAYER), true, false);
    assert_eq!(app.resource_selection.len(), 4);

    app.delete_selected_resource();

    assert!(!root.join(SPARE).exists(), "没引用的必须删掉");
    assert!(!root.join("assets/hero/a.png").exists());
    assert!(!root.join("assets/hero/b.png").exists());
    assert!(
        root.join(PLAYER).is_file(),
        "被场景引用的那张必须留下（批量不等于强制删除）"
    );
    assert!(
        app.status.contains("已删除 3 个资源") && app.status.contains(PLAYER),
        "状态要同时给出删除数与未删名单：{}",
        app.status
    );
    assert!(app.resource_selection.is_empty(), "删完清空多选");
    assert!(app.pending_delete.is_none(), "批量路径不进入单文件确认态");
}

/// A4：Shift 范围选按**可见顺序**取区间；Ctrl 再点一次取消该行。
#[test]
fn resource_selection_range_and_toggle_follow_the_visible_order() {
    let (mut app, root) = fixture("delete-range");
    for extra in ["assets/hero/a.png", "assets/hero/b.png"] {
        fs::write(root.join(extra), b"png").unwrap();
    }
    app.resources.refresh(&root).expect("扫描临时工程");
    app.resources_filter = "t:png".to_owned();
    app.resources.set_filter("t:png".to_owned());
    // 单选一个再 Shift 选另一个：区间内的所有可见项都进集合。
    app.select_resource(PathBuf::from("assets/hero/a.png"));
    let visible: Vec<PathBuf> = app
        .resources
        .visible_entries()
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    let last = visible.last().cloned().expect("至少有可见项");
    app.select_resource_with_modifiers(last.clone(), false, true);
    assert_eq!(
        app.resource_selection.len(),
        visible.len(),
        "Shift 范围选应覆盖全部可见项：{:?}",
        app.resource_selection
    );
    assert_eq!(app.selected_resource, Some(last));
    // Ctrl 点掉一个：集合变小，主选中让给剩下的最后一个。
    let removed = visible[0].clone();
    app.select_resource_with_modifiers(removed.clone(), true, false);
    assert!(!app.resource_selection.contains(&removed));
    assert_eq!(app.resource_selection.len(), visible.len() - 1);
    let _ = fs::remove_dir_all(root);
}

/// 读不动的文档不能被悄悄当成"没有引用"：它要出现在状态栏里。
///
/// 负控意义：守卫只在"引用数为 0"时才敢一次删；一旦某份文档解析失败却没被说出来，
/// "确认没人引用"就是一句谎话。
#[test]
fn an_unreadable_document_is_named_instead_of_being_counted_as_no_reference() {
    let (mut app, root) = fixture("delete-unreadable");
    fs::write(root.join("scenes/corrupt.scene"), "not a scene at all\n").unwrap();
    app.select_resource(PathBuf::from(SPARE));

    app.delete_selected_resource();

    // 读不动的文档不拦删除（那份文档本来就不参与运行），但必须点名。
    assert!(
        !root.join(SPARE).exists(),
        "没人引用 + 文档读不动：仍应删掉"
    );
    assert!(
        app.status.contains("文档读不动"),
        "读不动的文档要出现在状态栏里：{}",
        app.status
    );
}
