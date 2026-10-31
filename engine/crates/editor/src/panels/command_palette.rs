//! P2：命令面板（Ctrl+Shift+P）。
//!
//! 数据来自菜单树（[`crate::menu::palette_items`]）**加上**已加载插件声明的条目
//! （[`crate::menu::PaletteAction`]）；执行走同一条通道——内置条目走
//! [`EditorApp::run_menu_action`]，插件条目走 [`EditorApp::run_plugin_command`] /
//! [`EditorApp::toggle_plugin_panel`]。面板不新增第二条命令通道，菜单里有、面板里就有；
//! 菜单灰置的条目在面板里同样灰置并给出同一条原因。

use crate::EditorApp;
use crate::menu::{PaletteAction, filter_palette_items, gate_reason};
use eframe::egui;

impl EditorApp {
    /// 打开/关闭命令面板；打开时清空上次的查询词（与 Unity 的行为一致）。
    pub(crate) fn toggle_command_palette(&mut self) {
        self.command_palette_open = !self.command_palette_open;
        if self.command_palette_open {
            self.command_palette_query.clear();
            self.command_palette_index = 0;
        }
    }

    pub(crate) fn draw_command_palette(&mut self, ctx: &egui::Context) {
        if !self.command_palette_open {
            return;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.command_palette_open = false;
            return;
        }
        let items = self.palette_candidates();
        let filtered = filter_palette_items(&items, &self.command_palette_query);
        if filtered.is_empty() {
            self.command_palette_index = 0;
        } else {
            self.command_palette_index = self
                .command_palette_index
                .min(filtered.len().saturating_sub(1));
        }
        let mut run: Option<usize> = None;
        let mut close = false;
        egui::Window::new("命令面板")
            .id(egui::Id::new("command_palette"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 72.0))
            .show(ctx, |ui| {
                ui.set_min_width(520.0);
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.command_palette_query)
                        .id(egui::Id::new("command_palette_query"))
                        .hint_text("输入命令名（如 保存 / 撤消 / 面板）")
                        .desired_width(f32::INFINITY),
                );
                response.request_focus();
                if response.changed() {
                    self.command_palette_index = 0;
                }
                // 上下键在结果里移动（单行输入框的输入法不会用上下键，直接读事件即可）。
                if !filtered.is_empty() {
                    if ui.input(|input| input.key_pressed(egui::Key::ArrowDown)) {
                        self.command_palette_index =
                            (self.command_palette_index + 1).min(filtered.len() - 1);
                    }
                    if ui.input(|input| input.key_pressed(egui::Key::ArrowUp)) {
                        self.command_palette_index = self.command_palette_index.saturating_sub(1);
                    }
                }
                if ui.input(|input| input.key_pressed(egui::Key::Enter)) && !filtered.is_empty() {
                    run = Some(self.command_palette_index);
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        if filtered.is_empty() {
                            ui.weak("没有匹配的命令");
                        }
                        for (index, item) in filtered.iter().enumerate() {
                            let enabled = self.menu_gate_open(item.gate);
                            let text = match &item.shortcut {
                                Some(shortcut) => format!("{}    {shortcut}", item.path),
                                None => item.path.clone(),
                            };
                            let response = ui
                                .add_enabled_ui(enabled, |ui| {
                                    ui.selectable_label(index == self.command_palette_index, text)
                                })
                                .inner;
                            if response.clicked() && enabled {
                                self.command_palette_index = index;
                                run = Some(index);
                            }
                            if !enabled {
                                response.on_hover_text(gate_reason(item.gate));
                            }
                        }
                    });
                ui.small("Enter 执行 · ↑/↓ 选择 · Esc 关闭");
            });
        if let Some(index) = run
            && let Some(item) = filtered.get(index)
        {
            if self.menu_gate_open(item.gate) {
                let action = item.action.clone();
                close = true;
                match action {
                    PaletteAction::Menu(action) => self.run_menu_action(action),
                    PaletteAction::PluginCommand(command) => self.run_plugin_command(&command),
                    PaletteAction::PluginPanel(panel) => self.toggle_plugin_panel(&panel),
                }
            } else {
                self.status = gate_reason(item.gate).to_owned();
            }
        }
        if close {
            self.command_palette_open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::PaletteAction;
    use editor::ProjectInfo;
    use rustlua_editor_api::{
        CommandDescriptor, EditorPlugin, MenuItemDescriptor, PanelDescriptor, PluginAction,
        PluginContext, PluginManifest,
    };
    use rustlua_infra::logging::LogSink;

    /// 一个"面板 + 不挂命令的菜单条目 + 独立命令"的测试插件。
    ///
    /// 覆盖插件在命令面板里的三种形态：菜单条目（不挂命令 → 直接开关面板）、
    /// 菜单条目挂命令、独立命令。
    struct PaletteDemoPlugin;

    impl EditorPlugin for PaletteDemoPlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest::new("palette_demo", "面板示例", "1.0.0")
        }

        fn register(&mut self, context: &mut PluginContext) {
            context.panel(PanelDescriptor::new("palette_demo.panel", "面板示例"));
            context.menu_item(MenuItemDescriptor {
                menu: "插件".to_owned(),
                // 与面板同 id：菜单栏那条"没挂命令的菜单条目"就是开关这个面板。
                id: "palette_demo.panel".to_owned(),
                title: "面板示例".to_owned(),
                command: None,
            });
            context.command(CommandDescriptor::new("palette_demo.run", "运行示例"));
            context.command_handler("palette_demo.run", |host| {
                host.submit(PluginAction::SetStatus("示例命令已执行".to_owned()));
            });
        }
    }

    fn palette_test_app() -> EditorApp {
        let mut app = EditorApp::new(
            ProjectInfo {
                root: std::env::temp_dir()
                    .join(format!("rustlua-palette-{}", crate::util::test_token())),
                version: "test".to_owned(),
                directories: Vec::new(),
            },
            None,
            LogSink::new(32),
        );
        // 面板可见性会落盘：门绝不能写真实的 `~/.rustlua/editor.json`。
        app.user_config = Some(editor::UserConfigStore::at(crate::util::temp_user_home(
            "palette",
        )));
        app.initialize_ui_extensions();
        app
    }

    fn frame_input(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            events,
            ..egui::RawInput::default()
        }
    }

    fn collect_text(output: &egui::FullOutput) -> String {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn enter_event() -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }
    }

    /// 插件命令必须能在命令面板里**搜到并执行**：这是扩展生态真正可用的前提
    /// （否则插件命令只能从插件诊断页点到，等于没有快捷入口）。
    #[test]
    fn plugin_commands_are_searchable_and_runnable_from_the_palette() {
        let mut app = palette_test_app();
        app.plugin_registry
            .register(Box::new(PaletteDemoPlugin))
            .unwrap();
        app.plugin_registry.load("palette_demo").unwrap();

        // 候选集里三种插件形态都在。
        let candidates = app.palette_candidates();
        let panel_entry = candidates
            .iter()
            .find(|item| item.path == "插件 / 面板示例")
            .expect("插件菜单条目在候选里");
        assert_eq!(
            panel_entry.action,
            PaletteAction::PluginPanel("palette_demo.panel".to_owned()),
            "不挂命令的菜单条目 = 开关它自己的面板"
        );
        assert!(
            candidates
                .iter()
                .any(|item| item.path == "插件 / 面板示例 / 运行示例"),
            "独立命令在候选里：{candidates:?}"
        );

        // 面板画出这条命令。
        app.command_palette_open = true;
        app.command_palette_query = "运行示例".to_owned();
        let context = egui::Context::default();
        let _ = context.run(frame_input(Vec::new()), |context| {
            app.draw_command_palette(context);
        });
        let output = context.run(frame_input(Vec::new()), |context| {
            app.draw_command_palette(context);
        });
        let text = collect_text(&output);
        assert!(
            text.contains("插件 / 面板示例 / 运行示例"),
            "命令面板里要能看到插件命令：text={text:?}"
        );

        // Enter 执行：走插件命令通道（状态栏拿到插件提交的动作）。
        let _ = context.run(frame_input(vec![enter_event()]), |context| {
            app.draw_command_palette(context);
        });
        assert_eq!(app.status, "示例命令已执行", "插件命令被执行了");
        assert!(!app.command_palette_open, "执行后面板关闭");

        // 不挂命令的菜单条目：执行后它自己的面板被打开。
        app.command_palette_open = true;
        app.command_palette_query = "面板示例".to_owned();
        let _ = context.run(frame_input(Vec::new()), |context| {
            app.draw_command_palette(context);
        });
        let _ = context.run(frame_input(vec![enter_event()]), |context| {
            app.draw_command_palette(context);
        });
        assert_eq!(
            app.plugin_panel_visible.get("palette_demo.panel"),
            Some(&true),
            "插件面板被打开：{:?}",
            app.plugin_panel_visible
        );
    }

    /// 禁用插件后，命令面板里立刻搜不到它的条目（面板只认"已加载"这一份事实）。
    #[test]
    fn disabled_plugin_entries_leave_the_palette() {
        let mut app = palette_test_app();
        app.plugin_registry
            .register(Box::new(PaletteDemoPlugin))
            .unwrap();
        app.plugin_registry.load("palette_demo").unwrap();
        assert!(
            app.palette_candidates()
                .iter()
                .any(|item| item.path == "插件 / 面板示例 / 运行示例")
        );

        app.plugin_registry
            .set_enabled("palette_demo", false)
            .unwrap();
        let paths: Vec<String> = app
            .palette_candidates()
            .into_iter()
            .map(|item| item.path)
            .collect();
        assert!(
            !paths.iter().any(|path| path.contains("面板示例")),
            "禁用后插件条目必须消失：{paths:?}"
        );
    }
}
