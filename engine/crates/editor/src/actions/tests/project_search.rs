//! ③：全工程搜索的验收测试（固定工程 seed）。
//!
//! 验收门写的是**命中集合 + 跳转目标**，不是"面板里有几行"：
//! - 命中集合证明四类来源（文档字段 / 元数据 / 资源路径 / Lua 文本）都真的进了索引，
//!   而且每行都带"文档 ▸ 实体 ▸ 字段"；
//! - 跳转目标证明双击之后**真的选中了那个实体的那个字段**（选区 + Inspector 过滤词都要能
//!   把那一行留在结果里），而不是只把状态栏写漂亮。
//!
//! 夹具是自建的临时工程（两张场景 + 一张贴图 + 一个脚本），不用示例工程：
//! 命中条数必须由夹具决定，示例工程的内容会随演示增删。

use super::*;
use crate::EditorApp;
use crate::actions::project_index::{SearchHit, SearchKind};
use editor::{SceneDocument, filter_field_rows, open_project};
use rustlua_infra::logging::LogSink;
use rustlua_scene::scene::{Scene, SceneEntityId};

/// 两张场景都引用的贴图。
const TEXTURE: &str = "assets/sprites/player.png";
/// `Sprite` 上那条字段的 Inspector 路径（跳转目标就按这个字符串钉住）。
const TEXTURE_FIELD: &str = "Sprite.texture";
/// 手写面板组件 `Script` 引用的脚本（没有通用字段行，只能定位到实体）。
const SCRIPT: &str = "scripts/hero.lua";
/// `hero.lua` 里那句关键词的行号（夹具写死这五行，改脚本要同步改这里）。
const DAMAGE_MULTIPLIER_LINE: u32 = 4;

fn sprite_payload(texture: &str) -> String {
    format!("texture={texture};size=1x1;tint=1,1,1,1;sorting_layer=default")
}

/// 固定 seed：
/// - `scenes/level.scene`：E1 `Hero`（贴图 + 脚本）、E2 `Boss`（实体元数据 `role=vanguard`）、
///   文档元数据 `author=qoder`；
/// - `scenes/other.scene`：E1 `Turret` 指同一张贴图；
/// - `assets/sprites/player.png`、`scripts/hero.lua`。
fn seeded_fixture(label: &str) -> (EditorApp, PathBuf) {
    let source = sample_project_root();
    assert!(
        source.join("project.toml").is_file(),
        "sample project missing: {}",
        source.display()
    );
    let root = temp_root(label);
    // `open_project` 认这三个目录（缺一个就报 MissingDirectory），夹具先把骨架搭好。
    fs::create_dir_all(root.join("assets/sprites")).unwrap();
    fs::create_dir_all(root.join("scenes")).unwrap();
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::copy(source.join("project.toml"), root.join("project.toml")).unwrap();
    fs::write(root.join(TEXTURE), b"\x89PNG\r\n\x1a\n not a real png").unwrap();
    fs::write(
        root.join(SCRIPT),
        "-- 英雄行为\n\
         local speed = 1\n\
         local range = 3\n\
         local damage_multiplier = 2\n\
         return { speed = speed }\n",
    )
    .unwrap();

    let mut level = Scene::new("level");
    level.add_entity(SceneEntityId(1), "Hero", None).unwrap();
    let hero = level.entities.get_mut(&SceneEntityId(1)).unwrap();
    hero.components
        .insert("Sprite".to_owned(), sprite_payload(TEXTURE));
    hero.components
        .insert("Script".to_owned(), format!("file={SCRIPT}"));
    level.add_entity(SceneEntityId(2), "Boss", None).unwrap();
    level
        .entities
        .get_mut(&SceneEntityId(2))
        .unwrap()
        .metadata
        .insert("role".to_owned(), "vanguard".to_owned());
    level
        .metadata
        .insert("author".to_owned(), "qoder".to_owned());
    level.save(root.join("scenes/level.scene")).unwrap();

    let mut other = Scene::new("other");
    other.add_entity(SceneEntityId(1), "Turret", None).unwrap();
    other
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert("Sprite".to_owned(), sprite_payload(TEXTURE));
    other.save(root.join("scenes/other.scene")).unwrap();

    let project = open_project(&root).expect("临时工程应可打开");
    let mut app = EditorApp::new(project, None, LogSink::new(64));
    app.project_loaded = true;
    app.resources.refresh(&root).expect("资源索引应可建立");
    (app, root)
}

/// 打开磁盘文档（clean），让它以内存版参与索引。
fn open_document(app: &mut EditorApp, root: &Path, relative: &str) {
    let absolute = root.join(relative);
    let mut document = SceneDocument::open(&absolute).expect("打开场景");
    document.path = Some(absolute);
    app.document.current_scene = Some(document);
}

/// 重建索引并按 `query` 搜一次（面板上「重建索引」+ 输入框的两个入口，测试里直接调 shell）。
fn hits_for(app: &mut EditorApp, query: &str) -> Vec<SearchHit> {
    app.rebuild_project_search_index();
    app.project_search.query = query.to_owned();
    app.run_project_search();
    app.project_search.hits.clone()
}

/// 命中行的可读形态（`文档 ▸ 实体 ▸ 字段`），逐条钉住用。
fn locations(hits: &[SearchHit]) -> Vec<String> {
    hits.iter()
        .map(|hit| format!("{}｜{}", hit.kind.label(), hit.location_text()))
        .collect()
}

/// 找到第一行指定类别的命中（跳转测试只关心"那一类"，不关心它在列表里排第几）。
fn first_hit(hits: &[SearchHit], kind: SearchKind) -> SearchHit {
    hits.iter()
        .find(|hit| hit.kind == kind)
        .unwrap_or_else(|| panic!("没有 {kind:?} 命中：{:?}", locations(hits)))
        .clone()
}

#[test]
fn the_index_covers_every_documented_source() {
    let (mut app, root) = seeded_fixture("index-sources");
    app.rebuild_project_search_index();
    let index = app.project_search.index.as_ref().expect("重建后必须有索引");
    assert_eq!(index.scanned_documents, 2, "两份场景都要读到");
    assert_eq!(index.scanned_lua_files, 1, "脚本也要进索引");
    assert!(index.unreadable.is_empty(), "{:?}", index.unreadable);
    assert!(
        app.project_search.message.contains("2 份文档 · 1 个脚本"),
        "摘要要说清扫了些什么：{}",
        app.project_search.message
    );

    // 四类来源逐条钉住（顺序即索引顺序：文档按路径、字段按实体、再是资源/脚本）。
    assert_eq!(
        locations(&hits_for(&mut app, "player.png")),
        [
            format!("字段｜scenes/level.scene ▸ E#1 Hero ▸ {TEXTURE_FIELD}"),
            format!("字段｜scenes/other.scene ▸ E#1 Turret ▸ {TEXTURE_FIELD}"),
            format!("资源｜{TEXTURE}"),
        ]
    );
    assert_eq!(
        locations(&hits_for(&mut app, "damage_multiplier")),
        [format!("脚本｜{SCRIPT}:{DAMAGE_MULTIPLIER_LINE}")],
        "脚本命中要带行号，跳转才知道定位到哪一行"
    );
    assert_eq!(
        locations(&hits_for(&mut app, "vanguard")),
        ["元数据｜scenes/level.scene ▸ E#2 Boss ▸ role".to_owned()],
        "实体元数据要可搜（值 + 键 + 归属文档/实体）"
    );
    assert_eq!(
        locations(&hits_for(&mut app, "author qoder")),
        ["元数据｜scenes/level.scene ▸ author".to_owned()],
        "文档级元数据同样可搜"
    );
    assert_eq!(
        locations(&hits_for(&mut app, "Boss")),
        ["实体｜scenes/level.scene ▸ E#2 Boss".to_owned()],
        "实体名命中不该顺手把它的元数据也列出来"
    );
    assert_eq!(
        locations(&hits_for(&mut app, "level.scene")),
        [
            "文档｜scenes/level.scene".to_owned(),
            "资源｜scenes/level.scene".to_owned(),
        ],
        "场景本身也在资源索引里：文档行=打开它，资源行=在资源面板里定位那一行，是两个动作"
    );
    let _ = root;
}

#[test]
fn a_script_name_searches_as_a_resource_not_as_every_line() {
    // 负控：搜脚本文件名只出「资源」一行 + 引用它的那个字段行。
    // 把路径拼进行文本的 haystack 会让这个文件的**每一行**都成为命中。
    let (mut app, _root) = seeded_fixture("script-name-not-every-line");
    let hits = hits_for(&mut app, "hero.lua");
    assert_eq!(
        locations(&hits),
        [
            "字段｜scenes/level.scene ▸ E#1 Hero ▸ Script.file",
            "资源｜scripts/hero.lua",
        ]
    );
    assert!(
        hits.iter().all(|hit| hit.kind != SearchKind::Script),
        "行文本里没有这个词就不该出脚本命中：{:?}",
        locations(&hits)
    );
}

#[test]
fn double_click_a_field_hit_selects_that_entity_and_that_row() {
    let (mut app, _root) = seeded_fixture("jump-field");
    let hits = hits_for(&mut app, "player.png");
    // 目标是 `other.scene` 里那条（当前没有打开任何文档，跳转要先把那份文档打开）。
    let index = hits
        .iter()
        .position(|hit| {
            hit.kind == SearchKind::Field && hit.document.as_deref() == Some("scenes/other.scene")
        })
        .expect("other.scene 里那条字段命中");
    assert_eq!(app.activate_search_hit(index), CommandResult::Applied);

    // 跳转目标：实体 + 字段，两样都要真的落到位。
    assert_eq!(app.selection.primary(), Some(SceneEntityId(1)));
    assert_eq!(app.inspector_field_filter, TEXTURE_FIELD);
    let rows = filter_field_rows(
        &app.schema_field_rows(),
        &app.inspector_field_filter.clone(),
    );
    assert!(
        rows.iter().any(|row| row.path == TEXTURE_FIELD),
        "过滤词必须真的把那一行留在 Inspector 结果里：{:?}",
        rows.iter().map(|row| row.path.clone()).collect::<Vec<_>>()
    );
    assert!(
        app.status.contains("已定位字段") && app.status.contains("scenes/other.scene"),
        "{}",
        app.status
    );
    assert!(app.show_inspector_panel, "属性面板要被带到前台");
}

#[test]
fn double_click_a_script_hit_opens_the_read_only_viewer_at_that_line() {
    let (mut app, _root) = seeded_fixture("jump-script");
    let hits = hits_for(&mut app, "damage_multiplier");
    let script = first_hit(&hits, SearchKind::Script);
    assert_eq!(script.line, Some(DAMAGE_MULTIPLIER_LINE));
    assert_eq!(
        app.activate_search_target(&script.target),
        CommandResult::Applied
    );
    assert!(app.lua_source_view.show, "只读查看器要被打开");
    assert_eq!(
        app.lua_source_view.relative.as_deref(),
        Some(SCRIPT),
        "{:?}",
        app.lua_source_view.relative
    );
    assert_eq!(
        app.lua_source_view.target_line,
        Some(DAMAGE_MULTIPLIER_LINE)
    );
    assert_eq!(
        app.lua_source_view.error, None,
        "查看器报错就等于跳转没落地"
    );
}

#[test]
fn double_click_a_resource_hit_pins_the_resource_browser_row() {
    let (mut app, _root) = seeded_fixture("jump-resource");
    let hits = hits_for(&mut app, "player.png");
    let resource = first_hit(&hits, SearchKind::Resource);
    assert_eq!(
        app.activate_search_target(&resource.target),
        CommandResult::Applied
    );
    assert!(app.show_resources_panel);
    assert_eq!(
        app.pending_resource_locate
            .as_ref()
            .map(|path| path.to_string_lossy().replace('\\', "/")),
        Some(TEXTURE.to_owned()),
        "定位目标就是资源索引里那一条"
    );
}

#[test]
fn a_hand_written_component_field_locates_the_entity_only() {
    // `Script.file` 由手写面板显示（没有通用字段行）。填一个 Inspector 里根本搜不到的
    // 过滤词，比不填更糟：那会让人以为"已经定位到这一行"。
    let (mut app, root) = seeded_fixture("jump-handwritten");
    open_document(&mut app, &root, "scenes/level.scene");
    let hits = hits_for(&mut app, "hero.lua");
    let field = first_hit(&hits, SearchKind::Field);
    app.inspector_field_filter = "上一轮留下的过滤词".to_owned();

    assert_eq!(
        app.activate_search_target(&field.target),
        CommandResult::Applied
    );
    assert_eq!(app.selection.primary(), Some(SceneEntityId(1)));
    assert!(
        app.inspector_field_filter.is_empty(),
        "手写面板的字段要把过滤词清掉：{:?}",
        app.inspector_field_filter
    );
    assert!(app.status.contains("手写面板"), "{}", app.status);
}

#[test]
fn jumping_to_another_document_parks_until_the_dirty_gate_decides() {
    let (mut app, root) = seeded_fixture("jump-dirty-gate");
    open_document(&mut app, &root, "scenes/level.scene");
    app.document
        .current_scene
        .as_mut()
        .expect("已打开文档")
        .mark_dirty();
    let hits = hits_for(&mut app, "player.png");
    let index = hits
        .iter()
        .position(|hit| hit.document.as_deref() == Some("scenes/other.scene"))
        .expect("other.scene 里那条命中");

    assert_eq!(app.activate_search_hit(index), CommandResult::Applied);
    assert!(
        app.pending_document_change.is_some(),
        "脏文档换页必须先走 E06 闸门"
    );
    assert!(
        app.project_search.pending_jump.is_some(),
        "定位要挂起，等闸门裁决"
    );
    assert!(
        app.document.current_scene.as_ref().unwrap().is_dirty(),
        "挂起期间文档不能被换掉"
    );
    assert_ne!(
        app.selection.primary(),
        Some(SceneEntityId(1)),
        "还没放行就不该改选区（level.scene 的 E1 也不是目标）"
    );

    // 「放弃并继续」→ 下一帧补做定位（真实路径由帧入口 `tick_pending_search_jump` 驱动）。
    let context = test_context();
    assert!(app.resolve_pending_change(&context, PendingDecision::Discard));
    app.tick_pending_search_jump();
    assert_eq!(app.project_search.pending_jump, None, "补做后口袋要空");
    // 判据用生产代码那一个（`open_project` 会把根 canonicalize，路径写法不该由测试猜）。
    assert!(
        app.current_document_is("scenes/other.scene"),
        "放行后当前文档该是被跳到的那一份：{:?}",
        app.document
            .current_scene
            .as_ref()
            .and_then(|document| document.path.clone())
    );
    assert_eq!(app.selection.primary(), Some(SceneEntityId(1)));
    assert_eq!(app.inspector_field_filter, TEXTURE_FIELD);
}

#[test]
fn cancelling_the_document_switch_voids_the_parked_jump() {
    let (mut app, root) = seeded_fixture("jump-dirty-cancel");
    open_document(&mut app, &root, "scenes/level.scene");
    app.document
        .current_scene
        .as_mut()
        .expect("已打开文档")
        .mark_dirty();
    let hits = hits_for(&mut app, "player.png");
    let index = hits
        .iter()
        .position(|hit| hit.document.as_deref() == Some("scenes/other.scene"))
        .expect("other.scene 里那条命中");
    app.activate_search_hit(index);
    let before = app.selection.primary();

    let context = test_context();
    assert!(app.resolve_pending_change(&context, PendingDecision::Cancel));
    app.tick_pending_search_jump();
    assert_eq!(
        app.project_search.pending_jump, None,
        "取消后必须作废，否则下一轮换文档会莫名其妙跳过去"
    );
    assert_eq!(app.selection.primary(), before, "取消不动选区");
    assert!(app.status.contains("已取消定位"), "{}", app.status);
    assert!(
        app.document.current_scene.as_ref().unwrap().is_dirty(),
        "取消后文档仍是当前那份且没被保存"
    );
}

#[test]
fn play_mode_refuses_to_move_the_editing_selection() {
    use editor::{PlaySession, PlayState};

    let (mut app, root) = seeded_fixture("jump-playing");
    open_document(&mut app, &root, "scenes/level.scene");
    app.play_session = Some(PlaySession::start(
        app.document.current_scene.as_ref().unwrap(),
    ));
    app.play.set_state_for_test(PlayState::Playing);
    let hits = hits_for(&mut app, "player.png");
    let field = first_hit(&hits, SearchKind::Field);
    match app.activate_search_target(&field.target) {
        CommandResult::Rejected(reason) => assert!(reason.contains("播放模式"), "{reason}"),
        other => panic!("Play 期间跳编辑文档必须被拒：{other:?}"),
    }
    // 只读的两类不受影响：查看脚本、在资源面板里定位那一行都不改编辑文档。
    let scripts = hits_for(&mut app, "damage_multiplier");
    let script = first_hit(&scripts, SearchKind::Script);
    assert_eq!(
        app.activate_search_target(&script.target),
        CommandResult::Applied
    );
    assert!(app.lua_source_view.show);
}

#[test]
fn an_empty_query_returns_nothing_and_a_stale_index_is_rebuilt() {
    let (mut app, root) = seeded_fixture("query-and-staleness");
    // 空查询不出"全部"：那只会把 400 行的上限一次性填满，等于没有信息。
    app.rebuild_project_search_index();
    app.project_search.query = "   ".to_owned();
    app.run_project_search();
    assert!(app.project_search.hits.is_empty());
    assert_eq!(app.project_search.total_hits, 0);

    // 换工程：旧索引的路径不属于这里，开窗时必须自动重建。
    // （比对基准用 `app.project.root`——`open_project` 走 canonicalize，写法与夹具不同。）
    let project_root = app.project.root.clone();
    app.project_search.index.as_mut().expect("刚建过索引").root = root.join("elsewhere");
    app.open_project_search();
    assert!(
        app.project_search
            .index
            .as_ref()
            .is_some_and(|index| index.belongs_to(&project_root)),
        "失效索引要被重建"
    );
    assert!(app.project_search.show);
}

#[test]
fn the_resource_browser_reverse_lookup_searches_by_path() {
    // 右键「查找引用…」的入口：查询词就是这条资源的工程内路径。
    let (mut app, root) = seeded_fixture("reverse-lookup");
    app.open_project_search_for(TEXTURE.to_owned());
    assert!(app.project_search.show);
    assert_eq!(app.project_search.query, TEXTURE);
    assert_eq!(
        locations(&app.project_search.hits.clone()),
        [
            format!("字段｜scenes/level.scene ▸ E#1 Hero ▸ {TEXTURE_FIELD}"),
            format!("字段｜scenes/other.scene ▸ E#1 Turret ▸ {TEXTURE_FIELD}"),
            format!("资源｜{TEXTURE}"),
        ],
        "两处引用 + 资源本体一行，一条不多一条不少"
    );
    let _ = root;
}
