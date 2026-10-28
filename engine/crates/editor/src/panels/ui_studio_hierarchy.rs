//! M81：UI 工作室的节点层级——**内嵌在通用层级面板里**的独立模块。
//!
//! 层级树原先和画布同标签（画布下面那半块），挤占画布高度。抽出来后工作区停在
//! UI 工作室标签时，层级面板整体切到这个区段（与「选中资源时显示资源详情」同一种
//! 上下文替换），切回 Scene/Game 即回到实体层级；UI 工作室标签本身只留画布、令牌、
//! 资产与提案箱。行控件与拖放落点仍走 `tree.rs` 的共用原语，提交仍走 Spec 宿主提案。

use super::tree;
use super::ui_studio::{UiStudioPanel, report_proposal};
use crate::EditorApp;
use editor::ui_studio::{AlignMode, DistributeAxis, UiStudio};
use eframe::egui;
use rustlua_ui_spec::UiNodeKind;
use std::collections::BTreeSet;

/// 可添加的控件类型（顺序即下拉框顺序）；新节点布局由模型按类型给默认值。
fn addable_kinds() -> Vec<(UiNodeKind, &'static str)> {
    vec![
        (UiNodeKind::Panel { src: None }, "panel"),
        (
            UiNodeKind::Container {
                direction: Default::default(),
                padding: None,
                spacing: None,
                justify: Default::default(),
                align: Default::default(),
                columns: None,
            },
            "container",
        ),
        (
            UiNodeKind::Text {
                text: "文本".to_owned(),
                rich_text: false,
            },
            "text",
        ),
        (
            UiNodeKind::Image {
                src: String::new(),
                nine_slice: None,
            },
            "image",
        ),
        (
            UiNodeKind::Button {
                text: Some("按钮".to_owned()),
                src: None,
            },
            "button",
        ),
        (
            UiNodeKind::Toggle {
                text: Some("开关".to_owned()),
                checked: false,
            },
            "toggle",
        ),
        (
            UiNodeKind::Slider {
                min: 0.0,
                max: 1.0,
                value: 0.5,
            },
            "slider",
        ),
        (UiNodeKind::Progress { value: 0.5 }, "progress"),
        (
            UiNodeKind::Scrollbar {
                value: 0.0,
                size: 0.4,
                direction: Default::default(),
                handle: None,
            },
            "scrollbar",
        ),
        (UiNodeKind::Input { placeholder: None }, "input"),
        (UiNodeKind::ScrollView { horizontal: false }, "scroll_view"),
        (UiNodeKind::Mask { padding: None }, "mask"),
        (
            UiNodeKind::CanvasGroup {
                alpha: 1.0,
                interactable: true,
                blocks_raycasts: true,
            },
            "canvas_group",
        ),
        (UiNodeKind::List { virtualized: false }, "list"),
        (UiNodeKind::Grid { columns: 2 }, "grid"),
        (UiNodeKind::Tabs { selected: 0 }, "tabs"),
        (UiNodeKind::Modal, "modal"),
        (
            UiNodeKind::Tooltip {
                text: "提示".to_owned(),
            },
            "tooltip",
        ),
        (UiNodeKind::Spacer, "spacer"),
        (UiNodeKind::SafeArea, "safe_area"),
    ]
}

/// 多选对齐/分布 + 添加子节点：与 Unity 的 Align/Distribute 和 Hierarchy 菜单同语义。
fn draw_edit_ops(
    ui: &mut egui::Ui,
    studio: &UiStudio,
    add_child_kind: &mut usize,
    status: &mut String,
) {
    let count = studio.selection_count();
    ui.horizontal_wrapped(|ui| {
        ui.label(format!("对齐（已选 {count}）"));
        for mode in AlignMode::ALL {
            if ui
                .add_enabled(count >= 2, egui::Button::new(mode.label()))
                .clicked()
            {
                let outcome = studio
                    .align_selection(mode)
                    .map_err(|error| error.to_string());
                report_proposal(outcome, mode.label(), status);
            }
        }
        ui.separator();
        for (axis, label) in [
            (DistributeAxis::Horizontal, "水平分布"),
            (DistributeAxis::Vertical, "垂直分布"),
        ] {
            if ui
                .add_enabled(count >= 3, egui::Button::new(label))
                .clicked()
            {
                let outcome = studio
                    .distribute_selection(axis)
                    .map_err(|error| error.to_string());
                report_proposal(outcome, label, status);
            }
        }
    });
    // 左列面板很窄：类型下拉与「添加到选中」分两行，避免按钮被面板边缘裁掉。
    let kinds = addable_kinds();
    let index = (*add_child_kind).min(kinds.len() - 1);
    ui.horizontal_wrapped(|ui| {
        ui.label("添加子节点");
        egui::ComboBox::from_id_salt("ui_studio_add_child_kind")
            .selected_text(kinds[index].1)
            .show_ui(ui, |ui| {
                for (candidate, (_, label)) in kinds.iter().enumerate() {
                    ui.selectable_value(add_child_kind, candidate, *label);
                }
            });
    });
    let parent = studio
        .selection()
        .filter(|key| studio.accepts_children(key))
        .map(str::to_owned);
    if ui
        .add_enabled(parent.is_some(), egui::Button::new("添加到选中"))
        .clicked()
        && let Some(parent) = parent
    {
        match studio.add_node(&parent, kinds[index].0.clone()) {
            Ok((proposal, key)) => {
                *status = format!(
                    "已生成添加提案：{parent} → {key}（{}）",
                    proposal.proposal_id
                );
            }
            Err(error) => *status = error.to_string(),
        }
    }
}

/// 层级拖动落点 → (父级 key, 插入下标)。中线附近落成子节点，靠边落成兄弟。
fn hierarchy_drop(
    studio: &UiStudio,
    source: &str,
    target: &str,
    pointer_y: f32,
    target_rect: egui::Rect,
) -> Option<(String, Option<i32>)> {
    if source == target || is_descendant(studio, source, target) {
        return None;
    }
    let slot =
        tree::DropSlot::from_pointer(target_rect, pointer_y, studio.accepts_children(target));
    if slot == tree::DropSlot::Child {
        return Some((target.to_owned(), None));
    }
    let (parent, index) = studio.parent_and_index(target)?;
    // 同一父级里先把源摘走会前移目标下标。
    let same_parent_source = studio
        .parent_and_index(source)
        .filter(|(source_parent, _)| *source_parent == parent)
        .map(|(_, source_index)| source_index);
    Some((
        parent,
        Some(tree::sibling_index(index, slot, same_parent_source) as i32),
    ))
}

/// `node` 是否在 `ancestor` 的子树里（含自身）：把节点拖进自己的子树没有落点。
fn is_descendant(studio: &UiStudio, ancestor: &str, node: &str) -> bool {
    let mut current = Some(node.to_owned());
    while let Some(key) = current {
        if key == ancestor {
            return true;
        }
        current = studio.parent_and_index(&key).map(|(parent, _)| parent);
    }
    false
}

/// 预设落盘的默认路径（「导出为 UI 预设」）：工程内相对路径，与手写文档同一扩展名。
/// 目标已存在时 `export_prefab` 会直接拒绝——导出只负责新建，不覆盖别人正在编辑的文档。
fn prefab_document_path(key: &str) -> String {
    format!("ui/prefabs/{key}.ui.json")
}

impl EditorApp {
    /// 层级面板里的 UI 节点树（内嵌区段）。
    ///
    /// 返回 `false` = 当前不是 UI 上下文（工程没开 / UI 工作室不在前台），调用方照常
    /// 画场景实体层级；返回 `true` = 本区段已接管层级面板内容。
    ///
    /// 判断依据只有 [`EditorApp::ui_studio_active`]（UI 工作室标签是否正在画），
    /// 不掺 UI 节点选中状态：工作区停在 UI 工作室时，层级面板就该是 UI 树，哪怕
    /// 还没选中节点。
    pub(crate) fn draw_ui_studio_hierarchy_section(&mut self, ui: &mut egui::Ui) -> bool {
        if !self.ui_studio_active || !self.project_loaded {
            return false;
        }

        let UiStudioPanel {
            studio,
            status,
            hierarchy_rows,
            hierarchy_drag,
            wrap_key,
            add_child_kind,
            ..
        } = &mut self.ui_studio_panel;

        // 抬头写清"这是 UI 节点，不是场景实体"：层级面板同一位置会在两种上下文之间切换。
        ui.horizontal(|ui| {
            ui.heading("层级");
            ui.colored_label(egui::Color32::from_rgb(96, 200, 255), "UI 节点树");
            if let Some(document) = studio.as_ref().and_then(UiStudio::document) {
                ui.weak(document);
            }
        });
        let Some(studio) = studio.as_mut() else {
            ui.weak("UI 工作室还没有打开文档：双击 `.ui.json`，或在 UI 工作室标签里输入路径后点「打开」");
            return true;
        };
        ui.small("UI 工作室打开中：切到场景标签可恢复实体层级");

        // 层级操作放在树上方：树按剩余高度滚动，操作行不会被挤到面板外。
        // 复制 / 删除 / 包裹进新容器，仍然全部走提案。
        ui.horizontal_wrapped(|ui| {
            ui.label("层级操作");
            let selected = studio.selection().map(str::to_owned);
            if ui
                .add_enabled(selected.is_some(), egui::Button::new("复制"))
                .clicked()
                && let Some(key) = selected.clone()
            {
                let outcome = studio
                    .duplicate_node(&key)
                    .map_err(|error| error.to_string());
                report_proposal(outcome, "复制", status);
            }
            if ui
                .add_enabled(selected.is_some(), egui::Button::new("删除"))
                .clicked()
                && let Some(key) = selected.clone()
            {
                let outcome = studio.remove_node(&key).map_err(|error| error.to_string());
                report_proposal(outcome, "删除", status);
            }
            if ui
                .add_enabled(selected.is_some(), egui::Button::new("导出预设"))
                .on_hover_text("把选中的子树落成 ui/prefabs/<节点>.ui.json")
                .clicked()
                && let Some(key) = selected.clone()
            {
                match studio.export_prefab(&key, &prefab_document_path(&key)) {
                    Ok(accepted) => {
                        *status = format!(
                            "已导出预设：{}（版本 {}）",
                            accepted.document, accepted.revision
                        );
                    }
                    Err(error) => *status = error.to_string(),
                }
            }
            ui.add(
                egui::TextEdit::singleline(wrap_key)
                    .desired_width(110.0)
                    .hint_text("新容器标识"),
            );
            if ui
                .add_enabled(
                    selected.is_some() && !wrap_key.trim().is_empty(),
                    egui::Button::new("包裹"),
                )
                .clicked()
                && let Some(key) = selected
            {
                let outcome = studio
                    .wrap_node(&key, wrap_key.trim())
                    .map_err(|error| error.to_string());
                report_proposal(outcome, "包裹", status);
            }
        });
        draw_edit_ops(ui, studio, add_child_kind, status);
        ui.separator();

        let rows = studio.hierarchy();
        *hierarchy_rows = rows.len() as u64;
        let selected_keys: BTreeSet<String> = studio.selected_keys().into_iter().collect();
        let additive = ui.input(|input| {
            input.modifiers.command || input.modifiers.ctrl || input.modifiers.shift
        });
        let mut row_rects: Vec<(String, egui::Rect)> = Vec::new();
        // 操作行已在树上方；树占剩余高度（留一行给状态），节点多时自己滚动。
        let tree_height = (ui.available_height() - 20.0).max(96.0);
        egui::ScrollArea::vertical()
            .id_salt("ui_studio_hierarchy")
            .max_height(tree_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for row in &rows {
                    let label = format!(
                        "{} · {}",
                        row.key,
                        crate::util::widget_display_name(row.kind)
                    );
                    let selected = selected_keys.contains(&row.key);
                    // 与场景层级同一套行：缩进参考线 + 可点可拖的标签。
                    let row_response = ui
                        .horizontal(|ui| {
                            ui.set_min_width(ui.available_width());
                            tree::indent(ui, row.depth);
                            tree::label_button(
                                ui,
                                selected,
                                true,
                                egui::Sense::click_and_drag(),
                                label,
                            )
                        })
                        .inner
                        .on_hover_text("拖动改父级/排序；点击选中，Ctrl/Shift 加选");
                    if row_response.clicked() {
                        if additive {
                            studio.toggle_select(&row.key);
                        } else {
                            studio.select(&row.key);
                        }
                    }
                    if row_response.drag_started() {
                        *hierarchy_drag = Some(row.key.clone());
                    }
                    row_rects.push((row.key.clone(), row_response.rect));
                }
                // 拖动中的落点提示：与场景层级同一条横线/描边。
                if let Some(source) = hierarchy_drag.as_ref()
                    && let Some(point) = ui.input(|input| input.pointer.interact_pos())
                    && let Some((target, rect)) =
                        row_rects.iter().find(|(_, rect)| rect.contains(point))
                {
                    let slot = tree::DropSlot::from_pointer(
                        *rect,
                        point.y,
                        studio.accepts_children(target),
                    );
                    let valid = hierarchy_drop(studio, source, target, point.y, *rect).is_some();
                    tree::paint_drop_target(ui.painter(), *rect, slot, valid);
                }
            });

        // 层级拖动落点：中线以上/下是兄弟，中间是子节点（叶子控件不做父级）。
        if let Some(source) = hierarchy_drag.clone() {
            if ui.input(|input| input.pointer.any_down()) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            }
            if ui.input(|input| input.pointer.any_released()) {
                if let Some(point) = ui.input(|input| input.pointer.interact_pos())
                    && let Some((target, rect)) =
                        row_rects.iter().find(|(_, rect)| rect.contains(point))
                {
                    *status = match hierarchy_drop(studio, &source, target, point.y, *rect) {
                        Some((parent, index)) => match studio.move_node(&source, &parent, index) {
                            Ok(proposal) => format!(
                                "已生成移动提案：{} → {}（revision {}）",
                                source,
                                parent,
                                proposal.base_revision + 1
                            ),
                            Err(error) => error.to_string(),
                        },
                        None => format!("{source} 没有有效落点"),
                    };
                }
                *hierarchy_drag = None;
            }
        }

        if !status.is_empty() {
            ui.weak(status.as_str());
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

    /// M81：工作区停在 UI 工作室时，通用层级面板内嵌 UI 节点树并画出节点行；
    /// 切回 Scene 标签后必须回到实体层级，不留下"UI 上下文"。
    #[test]
    fn hierarchy_panel_hosts_the_ui_tree_while_ui_studio_is_active() {
        let examples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/game_api");
        let project = open_project(&examples).expect("样板工程");
        let mut app = EditorApp::new(project, None, LogSink::new(16));
        let temp =
            std::env::temp_dir().join(format!("rustlua-m81-hierarchy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        write_spec(&temp);
        let mut studio = UiStudio::new(temp.clone());
        studio.open("ui/panel.ui.json").expect("打开文档");
        app.ui_studio_panel.studio = Some(studio);
        app.ui_studio_panel.document_input = "ui/panel.ui.json".to_owned();
        app.show_ui_studio = true;
        app.pending_ui_studio_focus = true;

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
                crate::dock::tab_is_active(dock, crate::dock::DockTab::Hierarchy),
                "打开 UI 工作室应把层级标签带到前台，否则内嵌的 UI 树不可见"
            );
        }
        assert!(
            app.ui_studio_panel.hierarchy_rows == 3,
            "层级面板必须画出 UI 节点行（root/title/buy 三行），实际 {}",
            app.ui_studio_panel.hierarchy_rows
        );
        let text = frame_text(&open);
        assert!(text.contains("UI 节点树"), "层级面板没有切到 UI 上下文");
        assert!(
            text.contains("root · 面板") && text.contains("buy · 按钮"),
            "UI 节点行必须交给共用行控件绘制"
        );
        assert!(
            !text.contains("搜索实体（type:Sprite"),
            "UI 上下文下不应再画实体层级的搜索框"
        );

        // 切回 Scene：活动标签变了，层级面板要立刻回到场景实体树。
        app.focus_workspace_tab(crate::WorkspaceTab::Scene);
        let closed = context.run(raw(), |context| {
            crate::dock::show_workspace(context, &mut app)
        });
        assert!(
            !app.ui_studio_active,
            "切回 Scene 后 UI 工作室不再是活动标签"
        );
        let text = frame_text(&closed);
        assert!(
            !text.contains("UI 节点树"),
            "离开 UI 工作室后层级面板不得继续显示 UI 节点树"
        );
        assert!(
            text.contains("搜索实体（type:Sprite"),
            "离开 UI 工作室后应恢复实体层级的搜索框"
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn hierarchy_drop_maps_pointer_band_to_parent_or_sibling() {
        let temp = std::env::temp_dir().join(format!("rustlua-m81-drop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        write_spec(&temp);
        let mut studio = UiStudio::new(&temp);
        studio.open("ui/panel.ui.json").expect("打开文档");
        let row = |y: f32| egui::Rect::from_min_size(egui::pos2(0.0, y), egui::vec2(100.0, 20.0));

        assert_eq!(
            hierarchy_drop(&studio, "title", "root", 10.0, row(0.0)),
            Some(("root".to_owned(), None)),
            "容器中线落成子节点"
        );
        assert_eq!(
            hierarchy_drop(&studio, "title", "buy", 1.0, row(0.0)),
            Some(("root".to_owned(), Some(0))),
            "上带落成兄弟，且同父级源在前要先扣下标"
        );
        assert_eq!(
            hierarchy_drop(&studio, "title", "buy", 19.0, row(0.0)),
            Some(("root".to_owned(), Some(1))),
            "下带落成兄弟：摘掉 title 后插到 buy 之后"
        );
        assert_eq!(
            hierarchy_drop(&studio, "buy", "title", 10.0, row(0.0)),
            Some(("root".to_owned(), Some(1))),
            "叶子控件中线不能当父级，退化成“放到它后面”"
        );
        assert_eq!(hierarchy_drop(&studio, "buy", "buy", 10.0, row(0.0)), None);
        assert_eq!(
            hierarchy_drop(&studio, "root", "title", 10.0, row(0.0)),
            None,
            "不能把节点拖进自己的子树"
        );
        assert_eq!(
            hierarchy_drop(&studio, "root", "buy", 19.0, row(0.0)),
            None,
            "拖进子树的兄弟带同样没有落点"
        );
        let _ = std::fs::remove_dir_all(&temp);
    }
}
