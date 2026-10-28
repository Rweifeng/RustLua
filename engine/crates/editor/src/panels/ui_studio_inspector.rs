//! M80：UI 工作室的节点检查器——**内嵌在通用属性面板里**的独立模块。
//!
//! 检查器原先挤在 UI 工作室标签里（画布下面那半块，和层级列表左右分栏）。抽出来后
//! UI 工作室只留画布/层级/提案；工作区停在 UI 工作室标签时，属性面板整体切到这个
//! 区段（与"选中资源时显示资源详情"同一种上下文替换），切回 Scene/Game 即回到实体
//! 属性。字段与写回路径不变：缓冲在 `UiStudioPanel::field_inputs`，提交仍走 Spec
//! 宿主的提案 → 接受/撤销/拒绝，面板不直接改 `.ui.json`。

use super::ui_studio::{UiStudioPanel, report_proposal};
use crate::EditorApp;
use editor::ui_studio::{InspectorEdit, InspectorRow, InspectorSection, UiStudio};
use eframe::egui;
use rustlua_ui_spec::AnchorPreset;

/// 锚点预设短标签，顺序与 [`AnchorPreset::ALL`] 一致（九宫格 + 横/纵/全拉伸）。
const ANCHOR_PRESET_LABELS: [&str; 12] = [
    "左上", "上中", "右上", "左中", "居中", "右中", "左下", "下中", "右下", "横拉", "纵拉", "全铺",
];

/// 锚点预设面板：换锚点但保持当前矩形（与 Unity 的 Anchor Presets 同语义）。
fn draw_anchor_presets(ui: &mut egui::Ui, studio: &UiStudio, key: &str, status: &mut String) {
    let current = studio
        .inspector_rows()
        .into_iter()
        .find(|row| row.field == "anchor")
        .map(|row| row.value);
    ui.label("锚点预设（保持当前矩形）");
    egui::Grid::new(("ui_studio_anchor_presets", key))
        .num_columns(3)
        .spacing([2.0, 2.0])
        .show(ui, |ui| {
            for (index, preset) in AnchorPreset::ALL.iter().enumerate() {
                let selected = current.as_deref() == Some(preset.as_str());
                if ui
                    .add(
                        egui::Button::new(ANCHOR_PRESET_LABELS[index])
                            .min_size(egui::vec2(34.0, 18.0))
                            .selected(selected),
                    )
                    .on_hover_text(preset.as_str())
                    .clicked()
                {
                    let outcome = studio
                        .apply_anchor_preset(key, *preset)
                        .map_err(|error| error.to_string());
                    report_proposal(outcome, "锚点", status);
                }
                if (index + 1) % 3 == 0 {
                    ui.end_row();
                }
            }
        });
}

impl EditorApp {
    /// 属性面板里的 UI 节点检查器（内嵌区段）。
    ///
    /// 返回 `false` = 当前不是 UI 上下文（工程没开 / UI 工作室不在前台），调用方照常
    /// 画资源详情或实体属性；返回 `true` = 本区段已接管属性面板内容。
    ///
    /// 判断依据只有 [`EditorApp::ui_studio_active`]（UI 工作室标签是否正在画），
    /// 不掺 UI 节点选中状态：工作区停在 UI 工作室时，属性面板就该是 UI 面板，哪怕
    /// 还没选中节点（此时显示占位说明），而不是继续展示上一次的实体属性。
    pub(crate) fn draw_ui_studio_inspector_section(&mut self, ui: &mut egui::Ui) -> bool {
        if !self.ui_studio_active || !self.project_loaded {
            return false;
        }

        let UiStudioPanel {
            studio,
            status,
            field_inputs,
            inspector_rows,
            ..
        } = &mut self.ui_studio_panel;
        let Some(studio) = studio.as_mut() else {
            ui.strong("UI 节点属性");
            ui.weak("UI 工作室还没有打开文档：双击 `.ui.json`，或在 UI 工作室标签里输入路径后点「打开」");
            return true;
        };

        // 抬头写清"这是 UI 节点，不是实体"：属性面板同一位置会在两种上下文之间切换。
        ui.horizontal(|ui| {
            ui.strong("UI 节点属性");
            if let Some(document) = studio.document() {
                ui.weak(document);
            }
            match studio.selection() {
                Some(key) => {
                    ui.separator();
                    ui.label(format!("选中 {key}"));
                }
                None => {
                    ui.weak("未选中节点");
                }
            }
        });

        let mut section = studio.section();
        ui.horizontal(|ui| {
            for candidate in InspectorSection::ALL {
                if ui
                    .selectable_label(section == candidate, candidate.label())
                    .clicked()
                {
                    section = candidate;
                }
            }
        });
        studio.set_section(section);
        if studio.selection_count() > 1 {
            ui.weak(format!(
                "多选 {} 个：检查器编辑主选中，对齐/分布用画布上方按钮",
                studio.selection_count()
            ));
        }

        let inspector = studio.inspector_rows();
        *inspector_rows = inspector.len() as u64;
        let mut apply: Option<(InspectorRow, String)> = None;
        // 外层属性面板已有纵向 ScrollArea，这里不再套一层（嵌套滚动会吃掉滚轮）。
        if inspector.is_empty() {
            ui.weak(studio.section_placeholder());
        } else {
            egui::Grid::new("ui_studio_inspector")
                .num_columns(3)
                .striped(true)
                .show(ui, |ui| {
                    for row in &inspector {
                        ui.label(&row.field);
                        if row.read_only {
                            ui.weak(&row.value);
                            ui.label("");
                        } else {
                            // 缓存键带 revision：接受提案后输入框自动回填新值。
                            let id = format!(
                                "{}/{}/{}",
                                studio.revision(),
                                studio.selection().unwrap_or(""),
                                row.field
                            );
                            let buffer =
                                field_inputs.entry(id).or_insert_with(|| row.value.clone());
                            ui.add(
                                egui::TextEdit::singleline(buffer)
                                    .desired_width(200.0)
                                    .font(egui::TextStyle::Monospace),
                            );
                            if ui.button("应用").clicked() {
                                apply = Some((row.clone(), buffer.clone()));
                            }
                        }
                        ui.end_row();
                    }
                });
        }
        if section == InspectorSection::Layout
            && let Some(key) = studio.selection().map(str::to_owned)
        {
            draw_anchor_presets(ui, studio, &key, status);
        }

        if let Some((row, text)) = apply
            && let Some(key) = studio.selection().map(str::to_owned)
        {
            match row.edit {
                InspectorEdit::None => {}
                InspectorEdit::LayoutField(field) => {
                    let outcome = studio
                        .set_layout_field(&key, field, &text)
                        .map_err(|error| error.to_string());
                    report_proposal(outcome, field.label(), status);
                }
                InspectorEdit::Style => {
                    let style = text.trim().to_owned();
                    let outcome = studio
                        .set_style(&key, (!style.is_empty()).then_some(style.as_str()))
                        .map_err(|error| error.to_string());
                    report_proposal(outcome, "样式", status);
                }
                InspectorEdit::Event(event) => {
                    let outcome = studio
                        .set_event(&key, &event, text.trim())
                        .map_err(|error| error.to_string());
                    report_proposal(outcome, "事件", status);
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor::open_project;
    use rustlua_infra::logging::LogSink;
    use std::path::{Path, PathBuf};

    fn write_spec(root: &Path) {
        std::fs::create_dir_all(root.join("ui")).unwrap();
        let spec: rustlua_ui_spec::UiSpec = serde_json::from_value(serde_json::json!({
            "spec_version": 1,
            "document": {"kind": "widget", "name": "panel"},
            "root": {
                "key": "root",
                "type": "panel",
                "children": [
                    {"key": "title", "type": "text", "text": "Studio"},
                    {"key": "buy", "type": "button", "text": "Buy"}
                ]
            }
        }))
        .unwrap();
        let encoded = editor::ui_spec_host::encode_spec(&spec).unwrap();
        std::fs::write(root.join("ui/panel.ui.json"), encoded).unwrap();
    }

    fn frame_text(output: &egui::FullOutput) -> String {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text_shape) => Some(text_shape.galley.text().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// M80：工作区停在 UI 工作室时，通用属性面板内嵌 UI 节点检查器并画出选中节点
    /// 的字段；切回 Scene 标签后必须回到实体属性，不留下"UI 上下文"。
    #[test]
    fn properties_panel_hosts_the_ui_inspector_while_ui_studio_is_active() {
        let examples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/game_api");
        let project = open_project(&examples).expect("样板工程");
        let mut app = EditorApp::new(project, None, LogSink::new(16));
        let temp =
            std::env::temp_dir().join(format!("rustlua-m80-inspector-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        write_spec(&temp);
        let mut studio = UiStudio::new(temp.clone());
        studio.open("ui/panel.ui.json").expect("打开文档");
        studio.select("root");
        app.ui_studio_panel.studio = Some(studio);
        app.ui_studio_panel.document_input = "ui/panel.ui.json".to_owned();
        app.show_ui_studio = true;
        app.pending_ui_studio_focus = true;
        // 属性与 AI 助手同叶：先把助手带到前台，模拟"属性标签被挡在后面"的现状。
        // 打开 UI 工作室必须把属性标签也带回来，否则内嵌检查器根本没有机会被画出来。
        {
            let dock = app.dock_state.as_mut().expect("默认 dock 树");
            assert!(
                crate::dock::focus_tab(dock, crate::dock::DockTab::Assistant),
                "助手应与属性同叶"
            );
        }

        let context = egui::Context::default();
        crate::util::configure_cjk_fallback(&context);
        let raw = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 800.0),
            )),
            ..Default::default()
        };
        let open = context.run(raw(), |context| {
            crate::dock::show_workspace(context, &mut app)
        });
        assert!(app.ui_studio_active, "打开 UI 工作室后它应是活动标签");
        {
            let dock = app.dock_state.as_ref().expect("dock 树还在");
            assert!(
                crate::dock::tab_is_active(dock, crate::dock::DockTab::Inspector),
                "打开 UI 工作室应把属性标签带到前台，否则内嵌检查器不可见"
            );
        }
        assert!(
            app.ui_studio_panel.inspector_rows >= 5,
            "属性面板必须画出检查器字段行（key/type + 布局字段），实际 {}",
            app.ui_studio_panel.inspector_rows
        );
        assert!(
            frame_text(&open).contains("UI 节点属性"),
            "属性面板没有切到 UI 上下文"
        );

        // 切回 Scene：活动标签变了，属性面板要立刻回到实体属性。
        app.focus_workspace_tab(crate::WorkspaceTab::Scene);
        let closed = context.run(raw(), |context| {
            crate::dock::show_workspace(context, &mut app)
        });
        assert!(
            !app.ui_studio_active,
            "切回 Scene 后 UI 工作室不再是活动标签"
        );
        assert!(
            !frame_text(&closed).contains("UI 节点属性"),
            "离开 UI 工作室后属性面板不得继续显示 UI 节点检查器"
        );
        let _ = std::fs::remove_dir_all(&temp);
    }
}
