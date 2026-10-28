//! 架构守卫：**面板只发 action，不直接写文档**（docs/55 §4.1 规则 1）。
//!
//! 计划里那条"View 只发出 typed action"如果只写在文档里，第一次赶进度就会被绕过。这里把它
//! 变成一条**棘轮测试**：扫描 `panels/*.rs` 里"直接改动文档"的调用点，与一张钉住的计数表比较。
//!
//! - 计数**只能变少**：新增一处直接写入就会红，提示走 `EditorApp::dispatch`；
//! - 变少（迁移完成）也必须更新这张表——这是刻意的：迁移进度要显式记在代码里；
//! - 扫描前把源码里的空白全部去掉，所以跨行链式调用（`self\n.history\n.execute(`）同样能被抓到。
//!
//! 为什么用源码扫描而不是类型系统：Rust 没有"只在 shell 层可见"的可见性粒度，
//! 而这条规则的价值恰恰是**防止未来的增量破坏**，不是证明现在绝对干净。

#![cfg(test)]

use std::collections::BTreeMap;
use std::path::Path;

/// 直接改动文档/历史的调用点特征（已去空白后比较）。
///
/// 注意**不要**同时列 `self.history.execute(` 与 `.history.execute(`：
/// 后者是前者的子串，会同一个调用点被数两次（第一版就是这样，把 4 处 execute
/// 数成了 8 处）。只保留更短的那个。
const WRITE_NEEDLES: &[&str] = &[
    ".history.execute(",
    "self.history.push(",
    "self.document.current_scene.as_mut()",
    "self.set_current_scene_document(",
];

/// 每个面板文件允许的直接写入次数（只能不增；减少时必须同步改这里）。
///
/// 数值就是"还没迁移完的家底"：`0` 表示该面板已经完全只发 action。
/// 非 0 的几项都是**已知欠账**，每一条都要在 docs/56 §7 的迁移清单里有对应工作项：
///
/// - `inspector.rs`：**已清零**（第 27 轮）——手写组件面板的提交、实体操作、Prefab 逐字段
///   Apply/Revert/写回/解包/重建实例整条路径都搬到了 shell（`crate::actions`），面板只发
///   typed action；剩下的辅助函数（`insert_default_component`、`prefab_instance_source` 等）
///   是**纯函数/只读**，不写文档。
/// - `hierarchy.rs`：**已清零**（第 28 轮）——创建/删除/重命名/换父/粘贴/复制/隐藏锁定标记
///   落盘/场景新建都搬到了 `crate::hierarchy_ops`（面板只发 typed action）；
/// - `panels/scene_view/`：**已清零**（第 29 轮）——Tile 绘制落盘与拖放生成实体（prefab/model/sprite）
///   搬到了 `crate::scene_ops`；拖拽/Gizmo 的提交层更早已在 `crate::scene_drag`。
///   第 41 轮把该文件拆成 `panels/scene_view/`，守卫随之按相对路径递归扫描
///   （`scene_view/draw.rs` 这样的键），拆分**不缩小**这层守卫的覆盖面。
///   第 42 轮把它的测试搬进 `scene_view/tests/`：`count_writes` 本来就只统计生产代码、
///   测试模块按惯例排除，所以这里不再登记测试文件（原来的 `scene_view/tests.rs` 条目已删）。
///
/// 至此 `panels/` 下**所有**文件的直接写入计数都是 0：新增一处就会判红并给出"改用
/// `self.dispatch(...)`"的修复提示（这条规则现在是**不变量**，不再是欠账表）。
/// - `game_view.rs` / `header.rs`：**已清零**（第 28 轮）——Play 的 Stop 收尾与崩溃快照替换
///   都搬到了 shell（`crate::actions`），面板只发 typed action / 请求确认；
const PINNED_WRITES: &[(&str, usize)] = &[
    // M75-03：助手把对话存在 `messages`（不是文档历史），写动作只经 `execute_tool`。
    ("assistant.rs", 0),
    ("atlas.rs", 0),
    // P1-3b：断链检查窗只发 `ScanBrokenReferences` / `RepairBrokenReference`；
    // 扫描与改写都在 shell（`crate::actions::broken_refs`），面板不碰文档。
    ("broken_refs.rs", 0),
    ("bundles.rs", 0),
    ("console.rs", 0),
    ("domain_data.rs", 0),
    ("game_view.rs", 0),
    // P3-E03-2：画布只改 `GraphDocument`，不写场景文档。
    ("graph.rs", 0),
    ("header.rs", 0),
    ("hierarchy.rs", 0),
    ("input_bindings.rs", 0),
    ("inspector/mod.rs", 0),
    // P1 结构收口：`inspector.rs` 拆成目录（绘制 `mod.rs` / UI 子编辑器 `ui_editors.rs` /
    // Prefab 侧 `prefab.rs` / 纯文本辅助 `helpers.rs`）。拆分**不缩小**覆盖面——每个生产
    // 文件单独登记；`inspector/tests/` 按既有约定自动跳过（与 `scene_view/tests/` 一致）。
    ("inspector/prefab.rs", 0),
    ("inspector/helpers.rs", 0),
    ("inspector/ui_editors.rs", 0),
    ("material.rs", 0),
    ("keymap.rs", 0),
    ("mod.rs", 0),
    ("performance.rs", 0),
    ("progress.rs", 0),
    ("project_settings.rs", 0),
    ("publish.rs", 0),
    ("resource_browser.rs", 0),
    ("runtime_debug.rs", 0),
    ("scene_view/mod.rs", 0),
    ("scene_view/draw.rs", 0),
    ("scene_view/pointer.rs", 0),
    ("scene_view/scene_geometry.rs", 0),
    ("scene_view/view_state.rs", 0),
    ("scene_view/view_3d.rs", 0),
    ("scene_view/tool_panel.rs", 0),
    ("scene_view/tilemap.rs", 0),
    ("scene_view/render.rs", 0),
    ("scene_view/projection.rs", 0),
    ("schema_inspector.rs", 0),
    ("tilemap_tools.rs", 0),
    // P3-E01-3：时间线只读预览，播放头状态不进场景文档。
    ("timeline.rs", 0),
    // P1-3a：贴图预览窗只解码与显示，状态是窗内缓存（不是文档）。
    ("texture_preview.rs", 0),
    ("toolbar.rs", 0),
    // 树控件原语（缩进/行标签/拖放落点）：纯绘制与几何，不碰任何文档。
    ("tree.rs", 0),
    // M76-04：UI Studio 只经 Spec 宿主提案写 `.ui.json`，不碰场景文档。
    ("ui_studio.rs", 0),
];

fn normalized(source: &str) -> String {
    source.chars().filter(|ch| !ch.is_whitespace()).collect()
}

/// 收集 `panels/` 下的面板源码：**递归两层**（`panels/scene_view/*.rs` 这种拆出来的
/// 子目录同样要扫），键是相对 `panels/` 的路径（`/` 分隔）。
fn panel_sources() -> BTreeMap<String, String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/panels");
    let mut sources = BTreeMap::new();
    collect_panel_sources(&dir, "", &mut sources);
    sources
}

fn collect_panel_sources(dir: &Path, prefix: &str, sources: &mut BTreeMap<String, String>) {
    for entry in std::fs::read_dir(dir).expect("panels dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .expect("file name")
            .to_owned();
        if path.is_dir() {
            // 只下探一层：`panels/scene_view/` 这类拆分目录，不扫更深的任意层级。
            if !prefix.is_empty() {
                continue;
            }
            collect_panel_sources(&path, &format!("{file_name}/"), sources);
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some("rs") {
            continue;
        }
        // 守卫自己带着"反面样例"字符串，当然会命中——不扫它。
        if file_name == "architecture_tests.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("panel source");
        sources.insert(format!("{prefix}{file_name}"), text);
    }
}

fn count_writes(source: &str) -> Vec<String> {
    let text = production_text(source);
    let mut hits = Vec::new();
    for needle in WRITE_NEEDLES {
        let count = text.matches(needle).count();
        for _ in 0..count {
            hits.push((*needle).to_owned());
        }
    }
    hits
}

/// 只看**生产代码**：面板文件的测试模块按惯例放在文件末尾（顶格 `#[cfg(test)]` +
/// `mod …`）。测试里直接写文档是正当的，不该被算成"视图越权"。
/// 用"顶格属性 + 下一行是 mod"来识别，而不是最后一个 `#[cfg(test)]`——测试模块内部
/// 自己也会出现这个属性。返回去掉全部空白后的文本，跨行链式调用同样能被抓到。
fn production_text(source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut cut = lines.len();
    for (index, line) in lines.iter().enumerate() {
        let is_attribute =
            line.trim_end() == "#[cfg(test)]" && !line.starts_with(char::is_whitespace);
        let next_is_module = lines
            .get(index + 1)
            .is_some_and(|next| next.trim_start().starts_with("mod "));
        if is_attribute && next_is_module {
            cut = index;
            break;
        }
    }
    normalized(&lines[..cut].join("\n"))
}

#[test]
fn panels_only_emit_actions() {
    let sources = panel_sources();
    let pinned: BTreeMap<&str, usize> = PINNED_WRITES.iter().copied().collect();
    let mut actual: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, source) in &sources {
        let hits = count_writes(source);
        if !hits.is_empty() {
            actual.insert(name.clone(), hits);
        }
    }

    // 1) 新增的直接写入：任何未钉住的命中都算违规。
    let mut violations = Vec::new();
    for (name, hits) in &actual {
        let allowed = pinned.get(name.as_str()).copied().unwrap_or(0);
        if hits.len() > allowed {
            violations.push(format!(
                "{name}: 实测 {} 处，允许 {allowed} 处 → {:?}",
                hits.len(),
                hits
            ));
        }
    }
    assert!(
        violations.is_empty(),
        "面板里出现了**直接写文档**的新调用点（docs/55 §4.1：View 只发 typed action）。\n\
         请改为 `self.dispatch(editor::EditorCommand::…)`，把提交边界留在 shell：\n  {}",
        violations.join("\n  ")
    );

    // 2) 家底变少：迁移完成必须同步更新计数表（否则棘轮会失去意义）。
    let mut stale = Vec::new();
    for (name, allowed) in &pinned {
        if *allowed == 0 {
            continue;
        }
        let found = actual.get(*name).map_or(0, Vec::len);
        if found < *allowed {
            stale.push(format!("{name}: 表里 {allowed} 处，实测 {found} 处"));
        }
    }
    assert!(
        stale.is_empty(),
        "面板直接写入的家底变小了，请更新 PINNED_WRITES（这是迁移进度的显式记录）：\n  {}",
        stale.join("\n  ")
    );

    // 3) 计数表必须覆盖所有面板文件（新增面板文件时强制登记）。
    let mut missing = Vec::new();
    for name in sources.keys() {
        if !pinned.contains_key(name.as_str()) {
            missing.push(name.clone());
        }
    }
    assert!(
        missing.is_empty(),
        "新面板文件必须在 PINNED_WRITES 里登记（哪怕填 0）：{missing:?}"
    );
}

/// 守卫本身必须能判红：给它一段"直接写文档"的样例源码，它要能抓出来。
#[test]
fn ratchet_detects_direct_document_writes() {
    let sample = r#"
        impl EditorApp {
            fn draw(&mut self) {
                let scene = self.document.current_scene.as_mut().unwrap();
                self.history
                    .execute(scene, "x", CommandKind::Property, |_| Ok(()))
                    .unwrap();
            }
        }
    "#;
    let hits = count_writes(sample);
    assert!(
        hits.iter()
            .any(|needle| needle == "self.document.current_scene.as_mut()"),
        "跨行链式调用也必须被抓到：{hits:?}"
    );
    assert!(
        hits.iter().any(|needle| needle == ".history.execute("),
        "跨行链式调用也必须被抓到：{hits:?}"
    );
    // 反例：只发 action 的面板代码不得命中。
    let clean = r#"
        impl EditorApp {
            fn draw(&mut self) {
                if ui.button("x").clicked() {
                    self.dispatch(editor::EditorCommand::Save);
                }
            }
        }
    "#;
    assert!(count_writes(clean).is_empty());
}

/// 撤销/重做快捷键的特征（去空白后比较）。**一次按键只能被一个地方听到**：
/// shell 在帧入口读它并派发 `EditorCommand::Undo`，面板再私读一遍就会同键消费两步
/// （历史上正是 `graph.rs` 的画布快捷键让"撤一步图"顺手也撤掉了场景的一步）。
const UNDO_SHORTCUT_NEEDLES: &[&str] = &["key_pressed(egui::Key::Z", "key_pressed(egui::Key::Y"];

#[test]
fn panels_never_read_the_undo_shortcut() {
    let mut offenders = Vec::new();
    for (name, source) in panel_sources() {
        let text = production_text(&source);
        for needle in UNDO_SHORTCUT_NEEDLES {
            if text.contains(needle) {
                offenders.push(format!("{name} 命中 {needle}"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "面板私读了 Ctrl+Z / Ctrl+Y（M79 P1-2 起撤销由全局账本统一路由）。\n\
         请删掉这段键盘读取，需要按钮入口就调 `crate::actions::undo_ledger` 的归属入口：\n  {}",
        offenders.join("\n  ")
    );

    // 守卫自己也要能判红：反面样例必须被抓到。
    let sample = "impl GraphPanelState {\n    fn shortcuts(&mut self) {\n        let undo = ui.input(|i| i.key_pressed(egui::Key::Z));\n    }\n}\n";
    assert!(
        production_text(sample).contains(UNDO_SHORTCUT_NEEDLES[0]),
        "反面样例必须命中"
    );
}
