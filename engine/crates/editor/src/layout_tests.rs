//! 编辑器布局的行为测试：dock 树、面板尺寸与多标签切换的整帧断言。
//!
//! 从 `main.rs` 的 `mod layout_tests` 原样搬出（P1 结构收口）；模块路径仍是
//! `crate::layout_tests::*`，测试名不变。

use crate::{EditorApp, SceneTool, SceneViewMode, WorkspaceTab};
use eframe::egui;
use rustlua_infra::logging::LogSink;

/// 每次调用都是唯一的临时工程根（`{:?}` 的 SystemTime 保证同纳秒也不会撞车）。
fn temp_root(label: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "rustlua-editor-layout-{label}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp root");
    root
}

/// 阶段 2 收尾：六个工作台面板的可见性走同一条持久化路径；两个中央视图同时为
/// `false` 的文件按"只留 Scene"收敛，并把聚焦搬到还亮着的那个。
#[test]
fn every_panel_visibility_round_trips_and_the_central_pair_converges() {
    let root = temp_root("panel-visibility");
    let project = || editor::ProjectInfo {
        root: root.clone(),
        version: "test".to_owned(),
        directories: Vec::new(),
    };
    let mut app = EditorApp::new(project(), None, LogSink::new(64));
    app.project_loaded = true;
    app.show_hierarchy_panel = false;
    app.show_inspector_panel = false;
    app.show_scene_view = false;
    app.persist_layout();
    let raw = std::fs::read_to_string(editor::layout_path(&root)).expect("layout.json");
    let json: serde_json::Value = serde_json::from_str(&raw).expect("json");
    for key in [
        editor::PANEL_HIERARCHY,
        editor::PANEL_INSPECTOR,
        editor::PANEL_SCENE_VIEW,
    ] {
        assert_eq!(
            json["panel_visible"][key], false,
            "{key} 的关闭状态必须写进文件：{json}"
        );
    }

    let mut reloaded = EditorApp::new(project(), None, LogSink::new(64));
    reloaded.project_loaded = true;
    reloaded.load_layout_for_project();
    assert!(!reloaded.show_hierarchy_panel, "层级可见性必须恢复");
    assert!(!reloaded.show_inspector_panel, "属性可见性必须恢复");
    assert!(!reloaded.show_scene_view, "Scene 可见性必须恢复");
    assert!(reloaded.show_game_view, "没关的中央视图不能被顺手改掉");
    assert_eq!(reloaded.workspace_tab, WorkspaceTab::Game);

    // 手改成"两个中央视图都关"（守卫只拦菜单，拦不住文件）。
    let mut stored = reloaded.layout.clone();
    stored
        .panel_visible
        .insert(editor::PANEL_GAME_VIEW.to_owned(), false);
    editor::save_layout(&crate::util::strip_verbatim_prefix(&root), &stored)
        .expect("save broken layout");
    let mut both_hidden = EditorApp::new(project(), None, LogSink::new(64));
    both_hidden.project_loaded = true;
    both_hidden.load_layout_for_project();
    assert!(
        both_hidden.show_scene_view,
        "两个中央视图都关时必须回落成只留 Scene：{:?}",
        both_hidden.layout.panel_visible
    );
    assert!(!both_hidden.show_game_view, "回落不该把两个都打开");
    assert_eq!(both_hidden.workspace_tab, WorkspaceTab::Scene);
    let _ = std::fs::remove_dir_all(root);
}

/// 布局持久化的**接线**测试（不只是模块单测）：保存 → 重启 → 恢复同一状态；
/// 损坏文件回落默认且不改动源文件；重置写回默认。
#[test]
fn layout_round_trips_through_the_editor_shell() {
    let root = temp_root("shell");
    let project = || editor::ProjectInfo {
        root: root.clone(),
        version: "test".to_owned(),
        directories: Vec::new(),
    };
    let mut app = EditorApp::new(project(), None, LogSink::new(64));
    app.project_loaded = true;
    // 改一批布局字段并保存。
    app.show_lua_console = false;
    app.workspace_tab = WorkspaceTab::Game;
    app.scene_view_mode = SceneViewMode::View3D;
    app.set_scene_tool(SceneTool::Rotate);
    app.scene_zoom = 3.25;
    app.preview_resolution = (1920, 1080);
    app.layout
        .panel_widths
        .insert(editor::PANEL_INSPECTOR.to_owned(), 420.0);
    app.persist_layout();
    assert!(
        editor::layout_path(&root).exists(),
        "保存布局必须落盘：{}",
        editor::layout_path(&root).display()
    );

    // 「重启」：新 App 读取同一项目。
    let mut reloaded = EditorApp::new(project(), None, LogSink::new(64));
    reloaded.project_loaded = true;
    reloaded.load_layout_for_project();
    assert!(!reloaded.show_lua_console, "面板可见性必须恢复");
    assert_eq!(reloaded.workspace_tab, WorkspaceTab::Game);
    assert_eq!(reloaded.scene_view_mode, SceneViewMode::View3D);
    assert_eq!(reloaded.scene_tool(), SceneTool::Rotate);
    assert!((reloaded.scene_zoom - 3.25).abs() < 1e-6);
    assert_eq!(reloaded.preview_resolution, (1920, 1080));
    assert_eq!(reloaded.layout.panel_width(editor::PANEL_INSPECTOR), 420.0);

    // 损坏恢复：写入垃圾后加载必须回落默认，且**不覆盖**源文件。
    let garbage = "{ this is not json";
    std::fs::write(editor::layout_path(&root), garbage).expect("write garbage");
    let mut broken = EditorApp::new(project(), None, LogSink::new(64));
    broken.project_loaded = true;
    broken.load_layout_for_project();
    assert!(broken.show_lua_console, "损坏时必须回落默认布局");
    assert_eq!(broken.workspace_tab, WorkspaceTab::Scene);
    assert!(
        broken.status.contains("损坏"),
        "状态栏应说明原因：{}",
        broken.status
    );
    assert_eq!(
        std::fs::read_to_string(editor::layout_path(&root)).expect("read"),
        garbage,
        "损坏文件不得被静默覆盖"
    );

    // 重置：写回默认布局。
    broken.reset_layout_to_default();
    assert_eq!(
        editor::load_layout(&root).layout(),
        editor::EditorLayout::default()
    );
    let _ = std::fs::remove_dir_all(root);
}

/// 阶段 2 的接线测试：dock 树跟着「保存 → 重启 → 恢复」这条真实路径走。
///
/// 只看 `panel_visible` 是看不出问题的——重排（把面板搬到别处、藏掉一个）只体现在树结构里，
/// 恢复后若结构回落到默认树，用户会觉得"布局没存住"。
#[test]
fn the_dock_tree_round_trips_through_the_editor_shell() {
    let root = temp_root("dock-tree");
    let project = || editor::ProjectInfo {
        root: root.clone(),
        version: "test".to_owned(),
        directories: Vec::new(),
    };
    let mut app = EditorApp::new(project(), None, LogSink::new(64));
    app.project_loaded = true;
    let context = egui::Context::default();
    // dock 树要有真实矩形才能落盘（`dock::serialize` 的 ±∞ 守卫），所以先画一帧。
    let _ = context.run(egui::RawInput::default(), |context| {
        crate::dock::show_workspace(context, &mut app);
    });
    // 一次结构变化：藏掉控制台（View 菜单的开关会真的从树里移除标签）。
    app.show_lua_console = false;
    let _ = context.run(egui::RawInput::default(), |context| {
        crate::dock::show_workspace(context, &mut app);
    });
    let expected = app
        .dock_state
        .as_ref()
        .map(crate::dock::signature)
        .expect("帧末必须放回 dock 树");
    app.persist_layout();

    let mut reloaded = EditorApp::new(project(), None, LogSink::new(64));
    reloaded.project_loaded = true;
    reloaded.load_layout_for_project();
    let restored = reloaded
        .dock_state
        .as_ref()
        .map(crate::dock::signature)
        .expect("加载后必须有 dock 树");
    assert_eq!(restored, expected, "结构必须活过一次重启");
    let tabs: Vec<crate::dock::DockTab> = reloaded
        .dock_state
        .as_ref()
        .expect("加载后必须有 dock 树")
        .main_surface()
        .tabs()
        .copied()
        .collect();
    assert!(
        !tabs.contains(&crate::dock::DockTab::Console),
        "隐藏的面板不得被默认树带回来：{tabs:?}"
    );
    assert!(
        reloaded.layout.dock_tree.is_some(),
        "保存下来的必须是真的面板树，而不是只靠 panel_visible 现算"
    );

    // 「重置布局」必须把树也打回默认形状：只清 `dock_tree` 而不在这里重建，
    // 用户看到的就是"重置只生效了一半"。
    reloaded.reset_layout_to_default();
    assert_eq!(reloaded.layout.dock_tree, None, "重置必须清掉存下来的树");
    assert_eq!(
        reloaded.dock_state.as_ref().map(crate::dock::signature),
        Some(crate::dock::signature(&crate::dock::default_dock_state())),
        "重置必须回到默认面板树"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// M27-03 验收 3 的启动三态：全新项目 / 文件损坏 / 树内容认不出来，都要给出
/// **一棵画得出来的默认树**。
///
/// 三种形态各挡一种真实坏法：`.cache/` 不进版本库、外部工具会往里写，所以"截断"与
/// "整文件不是 JSON"迟早会发生；第三种是文件合法、`dock_tree` 也确实是对象，但结构
/// 解析不了（egui_dock 换了序列化形状就是这个形态）——它是唯一会**静默丢掉用户布局**
/// 的一种：其它字段全都照常生效，用户的体感只剩"我摆好的面板莫名其妙回了原位"。
/// 因此这一种除了比指纹，还必须查控制台留痕。
///
/// 每种都真画一帧：只比指纹的话，"结构对但画不出"的树也算过。
#[test]
fn the_three_layout_startup_states_all_land_on_a_usable_default_tree() {
    let root = temp_root("dock-states");
    let project = || editor::ProjectInfo {
        root: root.clone(),
        version: "test".to_owned(),
        directories: Vec::new(),
    };
    let default_tree = crate::dock::signature(&crate::dock::default_dock_state());
    let context = egui::Context::default();

    // `text` 为 `None` 表示"文件不存在"（全新项目）；其余形态先写文件再启动。
    let boot = |text: Option<&str>| {
        let path = editor::layout_path(&root);
        match text {
            Some(text) => {
                std::fs::create_dir_all(path.parent().expect("布局文件有父目录")).expect("建目录");
                std::fs::write(&path, text).expect("写布局文件");
            }
            None => {
                let _ = std::fs::remove_file(&path);
            }
        }
        let mut app = EditorApp::new(project(), None, LogSink::new(64));
        app.project_loaded = true;
        app.load_layout_for_project();
        let tree = crate::dock::signature(app.dock_state.as_ref().expect("启动后必须有 dock 树"));
        let _ = context.run(egui::RawInput::default(), |context| {
            crate::dock::show_workspace(context, &mut app);
        });
        let state = app.dock_state.as_ref().expect("画完一帧必须把树放回");
        let tabs: Vec<_> = state.iter_all_tabs().map(|(_, tab)| *tab).collect();
        assert!(
            crate::dock::serialize(state).is_some(),
            "回落后的树必须还能写回 layout.json，否则下一次启动仍是同一棵坏树"
        );
        (app, tree, tabs)
    };

    // ① 全新项目：没有文件 → 默认树，而且**不许报警**（首启就弹"布局损坏"是错的）。
    let (fresh, tree, tabs) = boot(None);
    assert_eq!(tree, default_tree, "全新项目必须是默认面板树");
    assert_eq!(tabs.len(), 8, "默认树要八个常驻视图都在：{tabs:?}");
    assert!(
        fresh.console_notices.is_empty(),
        "全新项目不该有布局提示：{:?}",
        fresh.console_notices
    );
    assert!(!fresh.status.contains("损坏"), "{}", fresh.status);

    // ② 文件损坏：回落默认树 + 状态栏给原因 + **不覆盖**源文件。
    let truncated = "{\"version\": 2, \"dock_tree\": {";
    let (broken, tree, tabs) = boot(Some(truncated));
    assert_eq!(tree, default_tree, "损坏必须整棵回落默认，不能留半棵树");
    assert_eq!(tabs.len(), 8, "损坏回落后八个常驻视图都在：{tabs:?}");
    assert!(
        broken.status.contains("损坏"),
        "状态栏要说明为什么用默认布局：{}",
        broken.status
    );
    assert_eq!(
        std::fs::read_to_string(editor::layout_path(&root)).expect("读回布局文件"),
        truncated,
        "损坏文件不得被静默覆盖"
    );

    // ③ `dock_tree` 是对象但内容认不出来：默认树 + 控制台留一句话，且原树仍留在字段里
    //    （丢掉它等于把"损坏"降级成"用户没存过布局"，重置布局那条路就看不出区别了）。
    let bogus_tree = "{\"version\": 2, \"dock_tree\": {\"nodes\": [[\"NoSuchPanel\"]]}}";
    let (mismatched, tree, tabs) = boot(Some(bogus_tree));
    assert_eq!(tree, default_tree, "认不出的树必须整棵回落默认：{tree}");
    assert_eq!(tabs.len(), 8, "回落后八个常驻视图都在：{tabs:?}");
    assert!(
        mismatched.layout.dock_tree.is_some(),
        "回落后仍要留着文件里那棵坏树，它是用户的原始数据"
    );
    assert!(
        mismatched
            .console_notices
            .iter()
            .any(|line| line.contains("面板树")),
        "静默丢布局是这一条唯一要挡的坏形态：{:?}",
        mismatched.console_notices
    );
    let _ = std::fs::remove_dir_all(root);
}

/// M27-03 验收 3 的"版本不符"半边：比当前构建更新的布局文件**按字段照读**，不整份作废。
///
/// 章程要的是"版本不符也能启动且回落默认"，这里落在字段的粒度上：v2 相对 v1 只多一个
/// `dock_tree` 键，读法又是逐键取类型、越界值统一 `clamp`，所以新版写出的文件在旧版手里
/// 最坏是"看不懂的新键走默认"，不会把好字段一起赔掉。反过来若判成整份损坏，用户从旧版
/// 升上来会白白丢一次布局——那是更差的回落。
#[test]
fn a_newer_layout_version_is_read_field_by_field_and_keeps_its_number() {
    let root = temp_root("dock-newer-version");
    let project = || editor::ProjectInfo {
        root: root.clone(),
        version: "test".to_owned(),
        directories: Vec::new(),
    };
    let context = egui::Context::default();
    let mut app = EditorApp::new(project(), None, LogSink::new(64));
    app.project_loaded = true;
    // 先画一帧才有可序列化的矩形（`dock::serialize` 的 ±∞ 守卫），再藏掉控制台造一棵非默认树。
    let _ = context.run(egui::RawInput::default(), |context| {
        crate::dock::show_workspace(context, &mut app);
    });
    app.show_lua_console = false;
    let _ = context.run(egui::RawInput::default(), |context| {
        crate::dock::show_workspace(context, &mut app);
    });
    let expected = crate::dock::signature(app.dock_state.as_ref().expect("帧末必须放回 dock 树"));
    // 缩放要在画完之后才定：首帧的 Scene View 会按内容自适应改它，先设就会被覆盖。
    app.scene_zoom = 2.5;
    app.persist_layout();

    let path = editor::layout_path(&root);
    let text = std::fs::read_to_string(&path).expect("读布局文件");
    let future = text.replace("\"version\": 2", "\"version\": 99");
    assert_ne!(future, text, "样例文件必须带版本号键");
    std::fs::write(&path, &future).expect("写未来版本文件");

    let mut reloaded = EditorApp::new(project(), None, LogSink::new(64));
    reloaded.project_loaded = true;
    reloaded.load_layout_for_project();
    assert!(
        !reloaded.status.contains("损坏"),
        "高版本不是损坏：{}",
        reloaded.status
    );
    assert!(
        reloaded.console_notices.is_empty(),
        "高版本不该报警：{:?}",
        reloaded.console_notices
    );
    assert!(
        (reloaded.scene_zoom - 2.5).abs() < 1e-6,
        "看得见的字段必须照读，不能整份作废：实际 {}",
        reloaded.scene_zoom
    );
    assert_eq!(
        crate::dock::signature(reloaded.dock_state.as_ref().expect("启动后必须有 dock 树")),
        expected,
        "认得出的键照读，面板树也不能被版本号判死"
    );
    // 版本号保持 99：把它盖章成 2 等于冒充旧格式，下次真 v3 写回来时没人知道文件被动过。
    reloaded.persist_layout();
    assert_eq!(
        editor::load_layout(&root).layout().version,
        99,
        "回写不得偷偷降级版本号"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// 阶段 2 的**自动保存**路径：dock 树要等布局稳定一段时间后才序列化落盘。
///
/// 节流是被 egui_dock 的 `separator.extra`（175px 最小面板宽）逼出来的：它每帧把
/// `fraction` 重新夹紧一次，于是**拖动窗口边缘本身就会连续改变结构指纹**——没有稳定
/// 窗口的话一次拖窗就是每帧写一次盘。这里用"文件先写、树后写"的时序把这条路径钉住。
#[test]
fn the_dock_tree_autosaves_only_after_the_layout_settles() {
    let root = temp_root("dock-autosave");
    let project = || editor::ProjectInfo {
        root: root.clone(),
        version: "test".to_owned(),
        directories: Vec::new(),
    };
    let context = egui::Context::default();
    let draw = |app: &mut EditorApp| {
        // 矩形要真实才能序列化（`dock::serialize` 的 ±∞ 守卫），所以每步都画一帧。
        let _ = context.run(egui::RawInput::default(), |ctx| {
            crate::dock::show_workspace(ctx, app);
        });
    };
    let read = || -> serde_json::Value {
        serde_json::from_str(
            &std::fs::read_to_string(editor::layout_path(&root)).expect("layout.json"),
        )
        .expect("json")
    };

    let mut app = EditorApp::new(project(), None, LogSink::new(64));
    app.project_loaded = true;
    // 首次观察只登记基线：打开项目、加载布局这一步本身不该写盘。
    app.maybe_persist_layout_on_change();
    assert!(
        !editor::layout_path(&root).exists(),
        "基线帧不得自动写盘：{:?}",
        std::fs::read_to_string(editor::layout_path(&root))
    );

    // 画一帧给树真实矩形（`dock::serialize` 的 ±∞ 守卫在这里把关）。
    draw(&mut app);

    // 一次结构变化：藏掉控制台会真的把标签从树里移除。
    // 被跟踪的 `panel_visible` 同步变了，所以文件会立刻写——但节流管的是树，
    // 此刻文件里的 `dock_tree` 必须还是 null。
    app.show_lua_console = false;
    draw(&mut app);
    app.maybe_persist_layout_on_change();
    assert!(
        editor::layout_path(&root).exists(),
        "可见性变化应当立刻自动保存"
    );
    assert!(
        read()["dock_tree"].is_null(),
        "稳定窗口内不得序列化 dock 树：{}",
        read()
    );

    std::thread::sleep(crate::EditorApp::DOCK_LAYOUT_SETTLE + std::time::Duration::from_millis(50));
    app.maybe_persist_layout_on_change();
    assert!(
        read()["dock_tree"].is_object(),
        "布局稳定后必须把树补写进去：{}",
        read()
    );
    assert_eq!(app.dock_dirty_since, None, "写成功后脏标记必须清掉");

    // 自动保存的文件要能被下一次启动原样读回。
    let expected = app.dock_state.as_ref().map(crate::dock::signature);
    let mut reloaded = EditorApp::new(project(), None, LogSink::new(64));
    reloaded.project_loaded = true;
    reloaded.load_layout_for_project();
    assert_eq!(
        reloaded.dock_state.as_ref().map(crate::dock::signature),
        expected,
        "自动保存的树必须活过一次重启"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// 助手设置来自文本框，每击键都会变：稳定窗口内不得写盘，期满那一帧合并写一次；
/// 落点是**用户级** `~/.rustlua`（不是工程布局），Key 单独加密存。
#[cfg(feature = "ai-bridge")]
#[test]
fn assistant_settings_autosave_waits_for_typing_to_settle() {
    let root = temp_root("assistant-autosave");
    let user_home = root.join("user-home");
    let project = || editor::ProjectInfo {
        root: root.clone(),
        version: "test".to_owned(),
        directories: Vec::new(),
    };
    let mut app = EditorApp::new(project(), None, LogSink::new(64));
    app.project_loaded = true;
    app.user_config = Some(editor::UserConfigStore::at(&user_home));
    // 第一帧只登记基线：启动加载不算用户改动。
    assert_eq!(app.maybe_persist_user_config_on_change(), None);
    assert!(!user_home.join("editor.json").exists(), "基线帧不得写盘");

    app.assistant.apply_layout(Some(&editor::AssistantLayout {
        provider: "openai".to_owned(),
        base_url: "https://example.invalid/v1".to_owned(),
        model: "typing-in-progress".to_owned(),
        max_tokens: 512,
    }));
    let pending = app.maybe_persist_user_config_on_change();
    assert!(pending.is_some(), "稳定期内必须返回到期重绘的等待时长");
    assert!(
        !user_home.join("editor.json").exists(),
        "输入稳定期内不得写盘"
    );

    std::thread::sleep(
        crate::EditorApp::ASSISTANT_LAYOUT_SETTLE + std::time::Duration::from_millis(50),
    );
    assert_eq!(app.maybe_persist_user_config_on_change(), None);
    assert_eq!(
        app.assistant_config_dirty_since, None,
        "写成功后脏标记必须清掉"
    );
    let text = std::fs::read_to_string(user_home.join("editor.json")).expect("editor.json");
    assert!(!text.contains("api_key"), "非密钥文件不得出现 api_key");
    assert!(!text.contains("sk-"), "非密钥文件不得出现密钥");
    let saved: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(saved["assistant"]["model"], "typing-in-progress");
    assert!(
        !editor::layout_path(&root).exists(),
        "助手设置不得再写进工程布局"
    );

    // Key 单独进 `assistant.key`，Windows 下必须是 DPAPI 密文。
    app.assistant
        .set_api_key_for_test("sk-test-only-in-ciphertext");
    assert!(
        app.maybe_persist_user_config_on_change().is_some(),
        "Key 变化同样要等输入稳定"
    );
    std::thread::sleep(
        crate::EditorApp::ASSISTANT_LAYOUT_SETTLE + std::time::Duration::from_millis(50),
    );
    assert_eq!(app.maybe_persist_user_config_on_change(), None);
    let key_path = user_home.join("assistant.key");
    assert!(
        key_path.exists(),
        "填了 Key 必须落盘：{}",
        key_path.display()
    );
    #[cfg(windows)]
    {
        let raw = std::fs::read(&key_path).unwrap();
        assert!(
            !raw.windows(b"sk-test-only-in-ciphertext".len())
                .any(|window| window == b"sk-test-only-in-ciphertext"),
            "密钥不得以明文落盘"
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

/// 面板布局：左侧资源(200)+层级(190)、右侧属性(320)、中央。验证在多种窗口
/// 宽度下，中央面板（Scene/Game 视图所在）都不与右侧属性面板重叠。
#[test]
fn central_panel_does_not_overlap_inspector_at_any_width() {
    for width in [1280.0, 1024.0, 900.0, 800.0, 700.0, 600.0] {
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 720.0),
            )),
            ..Default::default()
        };
        ctx.begin_pass(raw);
        egui::SidePanel::left("t_resource")
            .default_width(200.0)
            .show(&ctx, |ui| {
                ui.label("resource");
            });
        egui::SidePanel::left("t_tree")
            .default_width(190.0)
            .show(&ctx, |ui| {
                ui.label("hierarchy");
            });
        let mut inspector_rect = egui::Rect::NOTHING;
        egui::SidePanel::right("t_inspector")
            .default_width(320.0)
            .show(&ctx, |ui| {
                inspector_rect = ui.max_rect();
            });
        let mut central_rect = egui::Rect::NOTHING;
        egui::CentralPanel::default().show(&ctx, |ui| {
            central_rect = ui.max_rect();
        });
        let _ = ctx.end_pass();

        assert!(
            central_rect.right() <= inspector_rect.left() + 1.0,
            "窗口宽 {width}: 中央面板右缘 {:.1} 越过属性面板左缘 {:.1}（重叠）",
            central_rect.right(),
            inspector_rect.left()
        );
        assert!(
            central_rect.width() >= 0.0,
            "窗口宽 {width}: 中央面板宽度为负：{}",
            central_rect.width()
        );
    }
}
