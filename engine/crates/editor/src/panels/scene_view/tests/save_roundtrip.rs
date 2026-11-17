//! 跨 crate 门：编辑器**写盘**之后，引擎读回来的是不是编辑器想表达的东西。
//!
//! ## 这条门补的是什么
//!
//! 本目录已有的 `sample_project_tile_paint_persists_after_save_and_reopen` 断言的是
//! **编辑器内存里的场景文档**——它证明"涂改逻辑写对了"，但证明不了"落盘再读回来还对"。
//! 那中间隔着两件事，各由不同 crate 负责：
//!
//! 1. `.scene` 的**文本序列化**（`rustlua-scene::scene::to_text`）；
//! 2. 组件串的**文法解析**（各 owning crate 的 `from_component_str`）。
//!
//! 两者任何一处漂移，编辑器的单元测试都不会红——它只读自己刚写完的内存对象。
//! 而用户会看到"改了、存了、重开还是老样子"。
//!
//! ## 为什么断言落在"引擎语义"而不是"文本相等"
//!
//! 断言 `to_text()` 相等只能证明往返稳定，证明不了**引擎理解正确**。所以这里走完整链路：
//! 编辑器改 → `Scene::save` 落盘 → `Scene::load` 读回 → 逐项比对**编辑器当初写进去的值**。
//! 用真实样例工程而不是手搭场景，因为文法覆盖度（可选字段、默认值省略、转义）都在真数据上。

use super::*;
use rustlua_scene::scene::Scene;

/// 编辑器改完 UI 布局与文本后，引擎从磁盘读回的必须是**同一组值**。
///
/// 选这两个组件是因为它们的序列化最容易漂：
/// - `UiLayout` 有四组可选子字段（`anchors`/`offsets`/`min_size`/`max_size`），
///   省略默认值时的往返是否保序，只有解析器自己知道；
/// - `Text` 的 `text=` 字段含**空格与分号转义**，写盘时若漏转义，读回就截断。
#[test]
fn editor_ui_edits_survive_save_and_engine_reload() {
    let root = copy_sample_project("ui-roundtrip");
    let project = open_project(&root).unwrap();
    let mut app = EditorApp::new(project, None, LogSink::new(64));
    app.refresh_resources();
    app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));

    // 找一个带 UiLayout 且有名字的节点（样例工程里 UI 场景一定有）。
    let target = app
        .document
        .current_scene
        .as_ref()
        .unwrap()
        .scene
        .entities
        .values()
        .find(|entity| entity.components.contains_key("UiLayout"))
        .map(|entity| entity.id)
        .expect("ui_test.scene 应当至少有一个 UiLayout 节点");

    // 编辑器侧写入：容器四边 = (left, top, right, bottom) = (37, 41, 260, 88)。
    // 这组数字刻意不是整十——若序列化按整数格式化，读回就会差。
    let expected_offsets = [37.0_f32, 41.0, 260.0, 88.0];
    let scene_path = root.join("scenes/ui_test.scene");
    {
        let document = app.document.current_scene.as_mut().unwrap();
        let entity = document.scene.entities.get_mut(&target).unwrap();
        let raw = entity
            .components
            .get("UiLayout")
            .cloned()
            .unwrap_or_default();
        let mut layout = rustlua_ui::ui_layout::UiRectLayout::from_component_str(&raw)
            .expect("样例工程的 UiLayout 应当可解析");
        // 锚点清零，让 offsets 成为四边绝对量，取消其它子字段的影响。
        layout.anchors = [0.0; 4];
        layout.offsets = [37.0, 41.0, 260.0, 88.0];
        entity
            .components
            .insert("UiLayout".to_owned(), layout.to_component_str());
    }
    app.document
        .current_scene
        .as_ref()
        .unwrap()
        .scene
        .save(&scene_path)
        .expect("场景应当能落盘");

    // —— 以下是**引擎侧**的读回，不经过编辑器任何内存对象 ——
    let reloaded = Scene::load(&scene_path).expect("引擎应当能读回编辑器写的场景");
    let entity = reloaded
        .entities
        .get(&target)
        .unwrap_or_else(|| panic!("读回的场景里丢了实体 {target:?}"));

    let raw = entity
        .components
        .get("UiLayout")
        .expect("读回的实体丢了 UiLayout 组件");
    let layout = rustlua_ui::ui_layout::UiRectLayout::from_component_str(raw)
        .unwrap_or_else(|error| panic!("引擎读回的 UiLayout 无法解析：{error}"));

    for (index, (got, want)) in layout
        .offsets
        .iter()
        .zip(expected_offsets.iter())
        .enumerate()
    {
        assert!(
            (got - want).abs() < 1e-4,
            "UiLayout.offsets[{index}] 往返后是 {got}，编辑器写的是 {want}：\
             序列化或解析丢精度（原始串：{raw}）"
        );
    }
    assert_eq!(layout.anchors, [0.0; 4], "锚点往返后变了（原始串：{raw}）");
}

/// 标题文字里的 `=`、`;`、空格往返后必须**一字不差**。
///
/// `Text` 组件串是 `text=…;font_size=…` 这种分号分段格式，正文里出现分号或等号时
/// 必须转义。编辑器允许用户随便改文字，所以这条转义契约一旦破，表现是
/// "改了半天标题，重开只剩前半句"——而编辑器的内存断言看不出来（它存的是 Rust String）。
#[test]
fn editor_text_with_delimiters_survives_roundtrip() {
    let root = copy_sample_project("text-roundtrip");
    let project = open_project(&root).unwrap();
    let mut app = EditorApp::new(project, None, LogSink::new(64));
    app.refresh_resources();
    app.open_scene_resource(PathBuf::from("scenes/ui_test.scene"));

    // 一段把所有敏感字符都占上的标题：等号、分号、逗号、反斜杠、中文、连续空格。
    //
    // **两个常量故意分开写**：写入值（`written`）与期望读回值（`expected`）是两份独立
    // 字面量。如果只留一个变量既当写入又当断言，那么"把期望改错"也不会让测试红——
    // 两边一起变，永远相等，这条门就失去了检测力（反面对照实测确认过这一点）。
    let written = "HP = 100; ATK=7, \\ 满血";
    let expected = "HP = 100; ATK=7, \\ 满血";

    let target = app
        .document
        .current_scene
        .as_ref()
        .unwrap()
        .scene
        .entities
        .values()
        .find(|entity| entity.components.contains_key("Text"))
        .map(|entity| entity.id)
        .expect("ui_test.scene 应当至少有一个 Text 节点");

    let scene_path = root.join("scenes/ui_test.scene");
    {
        let document = app.document.current_scene.as_mut().unwrap();
        let entity = document.scene.entities.get_mut(&target).unwrap();
        let raw = entity.components.get("Text").cloned().unwrap_or_default();
        let mut text =
            rustlua_ui::ui::UiText::from_component_str(&raw).expect("样例工程的 Text 应当可解析");
        text.text = written.to_owned();
        entity
            .components
            .insert("Text".to_owned(), text.to_component_str());
    }
    app.document
        .current_scene
        .as_ref()
        .unwrap()
        .scene
        .save(&scene_path)
        .expect("场景应当能落盘");

    let reloaded = Scene::load(&scene_path).expect("引擎应当能读回编辑器写的场景");
    let raw = reloaded.entities[&target]
        .components
        .get("Text")
        .expect("读回的实体丢了 Text 组件");
    let text = rustlua_ui::ui::UiText::from_component_str(raw)
        .unwrap_or_else(|error| panic!("引擎读回的 Text 无法解析：{error}"));

    assert_eq!(
        text.text, expected,
        "文字往返后变了（原始串：{raw}）——分号/等号/反斜杠的转义契约破了"
    );
}
