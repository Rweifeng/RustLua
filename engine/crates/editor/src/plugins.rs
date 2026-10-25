//! E04-2：内置静态插件清单 + 宿主通道。
//!
//! 插件只拿到 [`HostSnapshot`]（只读）与 [`PluginAction`]（唯一写入口）；
//! 面板/菜单/命令都由宿主绘制，插件拿不到 `&mut EditorApp`。

use crate::EditorApp;
use eframe::egui;
use rustlua_editor_api::{
    Capability, CommandDescriptor, EditorPlugin, HostSnapshot, ImporterDescriptor,
    InspectorSectionDescriptor, MenuItemDescriptor, PanelDescriptor, PluginAction, PluginContext,
    PluginError, PluginHost, PluginManifest, VisualScriptNodeDescriptor,
};
use std::collections::BTreeMap;

/// 插件动作节点：真实的 [`rustlua_visual_script::NodeSpec`]，进同一张查找表。
const LOG_NODE: rustlua_visual_script::NodeSpec = rustlua_visual_script::NodeSpec {
    kind: "plugin.project_stats.log",
    title: "项目统计·日志",
    category: rustlua_visual_script::Category::Action,
    event: None,
    inputs: &[
        rustlua_visual_script::PortSpec {
            name: "in",
            ty: rustlua_visual_script::PortType::Flow,
        },
        rustlua_visual_script::PortSpec {
            name: "text",
            ty: rustlua_visual_script::PortType::Value(rustlua_visual_script::ValueType::String),
        },
    ],
    outputs: &[],
    params: &[],
};

/// 能力位的中文名（诊断页只展示这个，不上屏 `Panel/Command/...`）。
fn capability_label(capability: Capability) -> &'static str {
    match capability {
        Capability::Panel => "面板",
        Capability::MenuItem => "菜单项",
        Capability::Command => "命令",
        Capability::InspectorSection => "属性区段",
        Capability::AssetImporter => "资源导入器",
        Capability::VisualScriptNode => "可视化脚本节点",
        Capability::SettingsPage => "设置页",
    }
}

/// 核心编辑器不是特殊路径：它也只是注册表里的一个（空）插件。
pub(crate) struct CoreEditorPlugin;

impl EditorPlugin for CoreEditorPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new("core", "核心编辑器", env!("CARGO_PKG_VERSION"))
    }
}

/// 示例插件：一个面板 + 一个菜单条目 + 两条命令。
/// 只读快照、只发动作，因此禁用它不会影响场景文档。
struct ProjectStatsPlugin;

impl EditorPlugin for ProjectStatsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new("project_stats", "项目统计", env!("CARGO_PKG_VERSION"))
            .with_capability(Capability::Panel)
            .with_capability(Capability::MenuItem)
            .with_capability(Capability::Command)
            .with_capability(Capability::InspectorSection)
            .with_capability(Capability::AssetImporter)
            .with_capability(Capability::VisualScriptNode)
    }

    fn on_load(&mut self) -> Result<(), PluginError> {
        static REGISTER: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
        REGISTER
            .get_or_init(|| {
                rustlua_visual_script::register_plugin_node(&LOG_NODE, "print({text})", false)
            })
            .clone()
            .map_err(PluginError::Load)
    }

    fn register(&mut self, context: &mut PluginContext) {
        context.panel(PanelDescriptor::new("project_stats.panel", "项目统计"));
        context.panel_draw("project_stats.panel", |ui, host| {
            let snapshot = host.snapshot();
            ui.label(format!(
                "项目：{}",
                snapshot.project_root.as_deref().unwrap_or("（未打开）")
            ));
            ui.label(format!(
                "场景：{}",
                snapshot.scene_name.as_deref().unwrap_or("（未打开）")
            ));
            ui.label(format!("实体：{}", snapshot.entity_count));
            ui.label(format!("资源：{}", snapshot.resource_count));
            ui.label(format!(
                "选中实体：{}",
                snapshot
                    .selected_entity
                    .map(|id| format!("#{id}"))
                    .unwrap_or_else(|| "（无）".to_owned())
            ));
            if snapshot.playing {
                ui.label("运行中");
            }
            if ui.button("刷新统计").clicked() {
                let summary = stats_summary(snapshot);
                host.submit(PluginAction::SetStatus(summary));
            }
        });
        context.menu_item(MenuItemDescriptor {
            menu: "插件".to_owned(),
            id: "project_stats.menu".to_owned(),
            title: "项目统计".to_owned(),
            command: Some("project_stats.toggle".to_owned()),
        });
        context.command(CommandDescriptor::new(
            "project_stats.toggle",
            "显示/隐藏项目统计面板",
        ));
        context.command_handler("project_stats.toggle", |host| {
            host.submit(PluginAction::TogglePanel {
                panel: "project_stats.panel".to_owned(),
            });
        });
        context.command(CommandDescriptor::new(
            "project_stats.refresh",
            "刷新项目统计",
        ));
        context.command_handler("project_stats.refresh", |host| {
            let summary = stats_summary(host.snapshot());
            host.submit(PluginAction::SetStatus(summary));
        });
        context.inspector_section(InspectorSectionDescriptor::new(
            "project_stats.inspector",
            "项目统计",
        ));
        context.inspector_section_draw("project_stats.inspector", |ui, host| {
            match host.snapshot().selected_entity {
                Some(id) => ui.label(format!("选中实体：#{id}")),
                None => ui.label("（无选中实体）"),
            };
            if ui.button("写入状态栏").clicked() {
                let message = match host.snapshot().selected_entity {
                    Some(id) => format!("插件检查器：选中实体 #{id}"),
                    None => "插件检查器：无选中实体".to_owned(),
                };
                host.submit(PluginAction::SetStatus(message));
            }
        });
        context.asset_importer(ImporterDescriptor::new(
            "project_stats.stats",
            vec!["rlstats".to_owned()],
        ));
        context.visual_script_node(VisualScriptNodeDescriptor::new(
            "plugin.project_stats.log",
            "项目统计·日志",
        ));
    }
}

fn stats_summary(snapshot: &HostSnapshot) -> String {
    format!(
        "项目统计：{} 实体 / {} 资源",
        snapshot.entity_count, snapshot.resource_count
    )
}

pub(crate) fn builtin_plugins() -> Vec<Box<dyn EditorPlugin>> {
    vec![Box::new(CoreEditorPlugin), Box::new(ProjectStatsPlugin)]
}

/// 宿主实现：只收集动作，帧末由编辑器统一应用。
pub(crate) struct EditorPluginHost {
    snapshot: HostSnapshot,
    actions: Vec<PluginAction>,
}

impl PluginHost for EditorPluginHost {
    fn snapshot(&self) -> &HostSnapshot {
        &self.snapshot
    }

    fn submit(&mut self, action: PluginAction) {
        self.actions.push(action);
    }
}

impl EditorApp {
    pub(crate) fn initialize_ui_extensions(&mut self) {
        if !self.plugin_registry.names().is_empty() {
            return;
        }
        for plugin in builtin_plugins() {
            let id = match self.plugin_registry.register(plugin) {
                Ok(id) => id,
                Err(error) => {
                    let message = format!("插件注册失败：{error}");
                    self.plugin_status.push(message.clone());
                    tracing::error!(target: "editor", "[plugin] {message}");
                    continue;
                }
            };
            match self.plugin_registry.load(&id) {
                Ok(()) => {
                    let message = format!("插件 {id} 已加载（API v1）");
                    self.plugin_status.push(message.clone());
                    tracing::info!(target: "editor", "[plugin] {message}");
                }
                Err(error) => {
                    let message = format!("插件 {id} 加载失败：{error}");
                    self.plugin_status.push(message.clone());
                    tracing::error!(target: "editor", "[plugin] {message}");
                }
            }
        }
    }

    pub(crate) fn plugin_host_snapshot(&self) -> HostSnapshot {
        let document = self.document.current_scene.as_ref();
        HostSnapshot {
            project_root: Some(self.project.root.display().to_string()),
            scene_name: document.map(|document| document.scene.name.clone()),
            selected_entity: self.selection.primary().map(|id| id.0),
            entity_count: document.map_or(0, |document| document.scene.entities.len()),
            resource_count: self.resources.entries().len(),
            playing: self.is_playing(),
            visible_panels: self
                .plugin_panel_visible
                .iter()
                .filter(|(_, visible)| **visible)
                .map(|(panel, _)| panel.clone())
                .collect(),
        }
    }

    fn new_plugin_host(&self) -> EditorPluginHost {
        EditorPluginHost {
            snapshot: self.plugin_host_snapshot(),
            actions: Vec::new(),
        }
    }

    fn apply_plugin_actions(&mut self, actions: Vec<PluginAction>) {
        for action in actions {
            match action {
                PluginAction::SetStatus(message) => self.status = message,
                PluginAction::SetPanelVisible { panel, visible } => {
                    self.plugin_panel_visible.insert(panel, visible);
                }
                PluginAction::TogglePanel { panel } => {
                    let visible = self.plugin_panel_visible.entry(panel).or_insert(false);
                    *visible = !*visible;
                }
            }
        }
    }

    pub(crate) fn run_plugin_command(&mut self, command_id: &str) {
        let mut host = self.new_plugin_host();
        let result = self.plugin_registry.run_command(command_id, &mut host);
        let actions = std::mem::take(&mut host.actions);
        self.apply_plugin_actions(actions);
        if let Err(error) = result {
            let message = format!("插件命令 {command_id} 失败：{error}");
            self.plugin_status.push(message.clone());
            self.status = message;
        }
    }

    pub(crate) fn toggle_plugin_panel(&mut self, panel_id: &str) {
        let visible = self
            .plugin_panel_visible
            .entry(panel_id.to_owned())
            .or_insert(false);
        *visible = !*visible;
    }

    /// 把已加载插件的菜单条目追加到菜单栏；禁用后条目立即消失。
    pub(crate) fn draw_plugin_menu(&mut self, ui: &mut egui::Ui) {
        let mut menus: BTreeMap<String, Vec<(String, String, Option<String>)>> = BTreeMap::new();
        for (_, item) in self.plugin_registry.menu_items() {
            menus.entry(item.menu.clone()).or_default().push((
                item.id.clone(),
                item.title.clone(),
                item.command.clone(),
            ));
        }
        for (menu, items) in menus {
            ui.menu_button(menu, |ui| {
                for (id, title, command) in items {
                    let enabled = command
                        .as_deref()
                        .is_none_or(|command| self.plugin_registry.has_command(command));
                    if ui.add_enabled(enabled, egui::Button::new(title)).clicked() {
                        match command {
                            Some(command) => self.run_plugin_command(&command),
                            None => self.toggle_plugin_panel(&id),
                        }
                        ui.close();
                    }
                }
            });
        }
    }

    /// 绘制所有可见插件面板；禁用/卸载的插件面板即使记录为可见也不会画。
    pub(crate) fn draw_plugin_panels(&mut self, context: &egui::Context) {
        let panels: Vec<(String, String)> = self
            .plugin_registry
            .panels()
            .into_iter()
            .map(|(_, panel)| (panel.id.clone(), panel.title.clone()))
            .collect();
        for (id, title) in panels {
            let mut open = self.plugin_panel_visible.get(&id).copied().unwrap_or(false);
            if !open {
                continue;
            }
            let mut host = self.new_plugin_host();
            let mut failure = None;
            egui::Window::new(title)
                .id(egui::Id::new(("plugin_panel", id.clone())))
                .open(&mut open)
                .resizable(true)
                .default_width(260.0)
                .show(context, |ui| {
                    if let Err(error) = self.plugin_registry.draw_panel(&id, ui, &mut host) {
                        failure = Some(error.to_string());
                    }
                });
            self.plugin_panel_visible.insert(id.clone(), open);
            let actions = std::mem::take(&mut host.actions);
            self.apply_plugin_actions(actions);
            if let Some(failure) = failure {
                let message = format!("插件面板 {id} 失败：{failure}");
                self.plugin_status.push(message.clone());
                self.status = message;
            }
        }
    }

    /// 启用/禁用插件（诊断页按钮与测试共用）；错误进状态栏与插件日志。
    pub(crate) fn set_plugin_enabled(&mut self, id: &str, enabled: bool) {
        match self.plugin_registry.set_enabled(id, enabled) {
            Ok(()) => {
                let action = if enabled { "启用" } else { "禁用" };
                let message = format!("插件 {id} 已{action}");
                self.plugin_status.push(message.clone());
                self.status = message;
            }
            Err(error) => {
                let message = format!("插件 {id} 启停失败：{error}");
                self.plugin_status.push(message.clone());
                self.status = message;
            }
        }
    }

    /// 静态插件重新加载 = 卸载后重建上下文；失败时保留诊断信息。
    pub(crate) fn reload_plugin(&mut self, id: &str) {
        match self.plugin_registry.reload(id) {
            Ok(()) => {
                let message = format!("插件 {id} 已重新加载");
                self.plugin_status.push(message.clone());
                self.status = message;
            }
            Err(error) => {
                let message = format!("插件 {id} 重新加载失败：{error}");
                self.plugin_status.push(message.clone());
                self.status = message;
            }
        }
    }

    /// 插件诊断窗口：列表（id/version/status/error）+ 启用/禁用 + 重新加载。
    pub(crate) fn draw_plugin_diagnostics(&mut self, context: &egui::Context) {
        let mut open = self.show_plugin_diagnostics;
        if !open {
            return;
        }
        let statuses = self.plugin_registry.statuses();
        let messages: Vec<String> = self
            .plugin_status
            .iter()
            .rev()
            .take(8)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        egui::Window::new("插件诊断")
            .id(egui::Id::new("plugin_diagnostics"))
            .open(&mut open)
            .resizable(true)
            .default_width(460.0)
            .show(context, |ui| {
                ui.label(format!("已注册插件：{}", statuses.len()));
                ui.separator();
                for status in &statuses {
                    ui.horizontal(|ui| {
                        ui.strong(format!("{}（{}）", status.name, status.id));
                        ui.weak(format!("v{} · API v{}", status.version, status.api_version));
                    });
                    let state = match (status.enabled, status.loaded) {
                        (true, true) => "已加载",
                        (true, false) => "启用但未加载",
                        (false, _) => "已禁用",
                    };
                    ui.label(format!("状态：{state}"));
                    if !status.capabilities.is_empty() {
                        let capabilities = status
                            .capabilities
                            .iter()
                            .map(|capability| capability_label(*capability))
                            .collect::<Vec<_>>()
                            .join(", ");
                        ui.label(format!("能力：{capabilities}"));
                    }
                    if let Some(error) = &status.error {
                        ui.colored_label(egui::Color32::LIGHT_RED, format!("错误：{error}"));
                    }
                    ui.horizontal(|ui| {
                        let toggle = if status.enabled { "禁用" } else { "启用" };
                        if ui.button(toggle).clicked() {
                            self.set_plugin_enabled(&status.id, !status.enabled);
                        }
                        if ui
                            .add_enabled(status.enabled, egui::Button::new("重新加载"))
                            .clicked()
                        {
                            self.reload_plugin(&status.id);
                        }
                    });
                    ui.separator();
                }
                ui.strong("最近插件日志");
                egui::ScrollArea::vertical()
                    .max_height(160.0)
                    .show(ui, |ui| {
                        for message in &messages {
                            ui.label(message);
                        }
                        if messages.is_empty() {
                            ui.weak("（无）");
                        }
                    });
            });
        self.show_plugin_diagnostics = open;
    }

    /// 在 Inspector 里追加插件检查器段；没有选中实体时整段跳过。
    pub(crate) fn draw_plugin_inspector_sections(&mut self, ui: &mut egui::Ui) {
        if self.selection.primary().is_none() {
            return;
        }
        let sections: Vec<(String, String)> = self
            .plugin_registry
            .inspector_sections()
            .into_iter()
            .map(|(_, section)| (section.id.clone(), section.title.clone()))
            .collect();
        for (id, title) in sections {
            ui.separator();
            ui.strong(title);
            let mut host = self.new_plugin_host();
            let mut failure = None;
            if let Err(error) = self
                .plugin_registry
                .draw_inspector_section(&id, ui, &mut host)
            {
                failure = Some(error.to_string());
            }
            let actions = std::mem::take(&mut host.actions);
            self.apply_plugin_actions(actions);
            if let Some(failure) = failure {
                let message = format!("插件检查器段 {id} 失败：{failure}");
                self.plugin_status.push(message.clone());
                self.status = message;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor::{ProjectInfo, SceneDocument, SelectionSource};
    use rustlua_ecs::spatial_transform::SceneEntityId;
    use rustlua_infra::logging::LogSink;

    fn test_app() -> EditorApp {
        let mut app = EditorApp::new(
            ProjectInfo {
                root: std::env::temp_dir().join(format!("rustlua-plugins-{}", std::process::id())),
                version: "test".to_owned(),
                directories: Vec::new(),
            },
            None,
            LogSink::new(32),
        );
        let mut document = SceneDocument::new_scene("main");
        document
            .scene
            .add_entity(SceneEntityId(1), "Root", None)
            .unwrap();
        app.document.current_scene = Some(document);
        app.project_loaded = true;
        app.initialize_ui_extensions();
        app
    }

    fn revision(app: &EditorApp) -> u64 {
        app.document
            .current_scene
            .as_ref()
            .map_or(0, |document| document.revision)
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

    fn frame_input() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            ..egui::RawInput::default()
        }
    }

    fn draw_plugin_frames(app: &mut EditorApp) -> String {
        let context = egui::Context::default();
        // 窗口首帧只登记区域，第二帧才有稳定的布局与文字（与真实编辑器一致）。
        let _ = context.run(frame_input(), |context| {
            app.draw_plugin_panels(context);
        });
        let output = context.run(frame_input(), |context| {
            app.draw_plugin_panels(context);
        });
        collect_text(&output)
    }

    #[test]
    fn builtin_plugins_follow_enable_disable_and_never_write_the_document() {
        let mut app = test_app();
        assert!(app.plugin_registry.is_loaded("core"));
        assert!(app.plugin_registry.is_loaded("project_stats"));
        assert_eq!(app.plugin_registry.panels().len(), 1);
        assert!(app.plugin_registry.has_command("project_stats.refresh"));

        let before_revision = revision(&app);
        let entity_count = app
            .document
            .current_scene
            .as_ref()
            .unwrap()
            .scene
            .entities
            .len();
        let resource_count = app.resources.entries().len();
        app.run_plugin_command("project_stats.refresh");
        assert_eq!(
            app.status,
            format!("项目统计：{entity_count} 实体 / {resource_count} 资源")
        );
        assert_eq!(revision(&app), before_revision, "命令不得直接写文档");
        assert_eq!(
            app.document
                .current_scene
                .as_ref()
                .unwrap()
                .scene
                .entities
                .len(),
            entity_count
        );

        app.plugin_registry
            .set_enabled("project_stats", false)
            .unwrap();
        assert!(app.plugin_registry.panels().is_empty());
        assert!(app.plugin_registry.menu_items().is_empty());
        assert!(!app.plugin_registry.has_command("project_stats.refresh"));
        app.run_plugin_command("project_stats.refresh");
        assert!(app.status.contains("失败"), "禁用后命令不可用");

        app.plugin_registry
            .set_enabled("project_stats", true)
            .unwrap();
        assert_eq!(app.plugin_registry.panels().len(), 1);
        assert!(app.plugin_registry.has_command("project_stats.refresh"));
    }

    #[test]
    fn panel_toggle_draws_headless_and_respects_disable() {
        let mut app = test_app();
        app.toggle_plugin_panel("project_stats.panel");
        let text = draw_plugin_frames(&mut app);
        assert!(
            text.contains("实体：1"),
            "面板应画出实体数：text={text:?} visible={:?} status={:?}",
            app.plugin_panel_visible,
            app.plugin_status
        );
        assert!(text.contains("资源："));
        assert!(app.plugin_status.iter().all(|line| !line.contains("失败")));

        app.plugin_registry
            .set_enabled("project_stats", false)
            .unwrap();
        let text = draw_plugin_frames(&mut app);
        assert!(!text.contains("实体：1"), "禁用后面板不再绘制");

        app.plugin_registry
            .set_enabled("project_stats", true)
            .unwrap();
        let text = draw_plugin_frames(&mut app);
        assert!(text.contains("实体：1"), "重新启用后面板恢复");
    }

    #[test]
    fn inspector_section_draws_only_with_a_selected_entity() {
        let mut app = test_app();
        assert_eq!(app.plugin_registry.inspector_sections().len(), 1);
        let context = egui::Context::default();
        let draw = |app: &mut EditorApp| {
            let output = context.run(frame_input(), |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    app.draw_plugin_inspector_sections(ui);
                });
            });
            collect_text(&output)
        };

        let text = draw(&mut app);
        assert!(!text.contains("项目统计"), "无选中实体时不画：{text:?}");

        app.selection
            .set_single(SceneEntityId(1), SelectionSource::Command);
        let text = draw(&mut app);
        assert!(text.contains("项目统计"), "有选中实体时画段标题：{text:?}");
        assert!(text.contains("选中实体：#1"), "text={text:?}");
        assert!(text.contains("写入状态栏"), "text={text:?}");
    }

    #[test]
    fn importer_and_visual_script_node_are_real_registrations() {
        let mut app = test_app();
        let importers = app.plugin_registry.asset_importers();
        assert_eq!(importers.len(), 1);
        assert_eq!(importers[0].1.id, "project_stats.stats");
        assert_eq!(importers[0].1.extensions, vec!["rlstats".to_owned()]);
        let nodes = app.plugin_registry.visual_script_nodes();
        assert!(
            nodes.iter().any(|(_, node)| node.kind == LOG_NODE.kind),
            "nodes={nodes:?}"
        );
        let spec = rustlua_visual_script::node_spec(LOG_NODE.kind)
            .expect("插件节点必须真的进同一张注册表");
        assert_eq!(spec.category, rustlua_visual_script::Category::Action);

        app.plugin_registry
            .set_enabled("project_stats", false)
            .unwrap();
        assert!(app.plugin_registry.asset_importers().is_empty());
        assert!(app.plugin_registry.visual_script_nodes().is_empty());

        app.plugin_registry
            .set_enabled("project_stats", true)
            .unwrap();
        assert_eq!(app.plugin_registry.asset_importers().len(), 1);
        assert_eq!(app.plugin_registry.visual_script_nodes().len(), 1);
    }

    #[test]
    fn diagnostics_lists_plugins_and_controls_enable_and_reload() {
        let mut app = test_app();
        app.show_plugin_diagnostics = true;
        let context = egui::Context::default();
        let draw = |app: &mut EditorApp| {
            let _ = context.run(frame_input(), |context| {
                app.draw_plugin_diagnostics(context);
            });
            let output = context.run(frame_input(), |context| {
                app.draw_plugin_diagnostics(context);
            });
            collect_text(&output)
        };

        let text = draw(&mut app);
        assert!(text.contains("已注册插件：2"), "text={text:?}");
        assert!(text.contains("项目统计"), "text={text:?}");
        assert!(text.contains("已加载"), "text={text:?}");

        app.set_plugin_enabled("project_stats", false);
        assert!(!app.plugin_registry.is_enabled("project_stats"));
        assert!(!app.plugin_registry.is_loaded("project_stats"));
        let text = draw(&mut app);
        assert!(text.contains("已禁用"), "text={text:?}");

        app.set_plugin_enabled("project_stats", true);
        assert!(app.plugin_registry.is_loaded("project_stats"));
        app.reload_plugin("project_stats");
        assert!(app.plugin_registry.is_loaded("project_stats"));
        assert!(
            app.plugin_status
                .iter()
                .any(|message| message.contains("已重新加载")),
            "status={:?}",
            app.plugin_status
        );
    }

    #[test]
    fn on_load_panic_is_isolated_and_visible_in_diagnostics() {
        struct PanicLoadPlugin;
        impl EditorPlugin for PanicLoadPlugin {
            fn manifest(&self) -> PluginManifest {
                PluginManifest::new("panic_load", "Panic Load", "1.0.0")
            }
            fn on_load(&mut self) -> Result<(), PluginError> {
                panic!("load panic");
            }
        }

        let mut app = test_app();
        app.plugin_registry
            .register(Box::new(PanicLoadPlugin))
            .unwrap();
        let error = app.plugin_registry.load("panic_load").unwrap_err();
        app.plugin_status
            .push(format!("插件 panic_load 加载失败：{error}"));

        assert!(!app.plugin_registry.is_loaded("panic_load"));
        let status = app
            .plugin_registry
            .statuses()
            .into_iter()
            .find(|status| status.id == "panic_load")
            .expect("panic 插件仍在注册表里");
        assert!(status.enabled);
        assert!(!status.loaded);
        assert!(status.error.is_some(), "status={status:?}");

        app.run_plugin_command("project_stats.refresh");
        assert!(app.status.starts_with("项目统计："), "其他插件仍可用");

        app.show_plugin_diagnostics = true;
        let context = egui::Context::default();
        let _ = context.run(frame_input(), |context| {
            app.draw_plugin_diagnostics(context);
        });
        let output = context.run(frame_input(), |context| {
            app.draw_plugin_diagnostics(context);
        });
        let text = collect_text(&output);
        assert!(text.contains("Panic Load"), "text={text:?}");
        assert!(text.contains("错误："), "text={text:?}");
        assert!(text.contains("加载失败"), "text={text:?}");
    }

    #[test]
    fn plugin_menu_entries_disappear_when_the_plugin_is_disabled() {
        let mut app = test_app();
        let context = egui::Context::default();
        let draw_menu = |app: &mut EditorApp| {
            let output = context.run(frame_input(), |context| {
                egui::TopBottomPanel::top("plugin_menu_test").show(context, |ui| {
                    app.draw_plugin_menu(ui);
                });
            });
            collect_text(&output)
        };
        assert_eq!(app.plugin_registry.menu_items().len(), 1);
        let text = draw_menu(&mut app);
        assert!(text.contains("插件"), "插件菜单应出现：{text:?}");

        app.plugin_registry
            .set_enabled("project_stats", false)
            .unwrap();
        assert!(app.plugin_registry.menu_items().is_empty());
        let text = draw_menu(&mut app);
        assert!(!text.contains("插件"), "禁用后菜单应消失：{text:?}");
    }

    #[test]
    fn panic_plugin_isolated_and_other_plugins_keep_working() {
        struct PanicPlugin;
        impl EditorPlugin for PanicPlugin {
            fn manifest(&self) -> PluginManifest {
                PluginManifest::new("panic_demo", "Panic 示例", "1.0.0")
            }
            fn register(&mut self, context: &mut PluginContext) {
                context.panel(PanelDescriptor::new("panic_demo.panel", "Panic"));
                context.panel_draw("panic_demo.panel", |_, _| panic!("draw panic"));
                context.command(CommandDescriptor::new("panic_demo.run", "Panic 命令"));
                context.command_handler("panic_demo.run", |_| panic!("command panic"));
            }
        }

        let mut app = test_app();
        app.plugin_registry.register(Box::new(PanicPlugin)).unwrap();
        app.plugin_registry.load("panic_demo").unwrap();
        app.run_plugin_command("panic_demo.run");
        assert!(app.status.contains("失败"), "命令 panic 被隔离");
        assert!(app.plugin_registry.is_loaded("panic_demo"));

        app.toggle_plugin_panel("panic_demo.panel");
        let context = egui::Context::default();
        let _ = context.run(frame_input(), |context| {
            app.draw_plugin_panels(context);
        });
        assert!(app.status.contains("失败"), "绘制 panic 被隔离");

        app.run_plugin_command("project_stats.refresh");
        assert!(app.status.starts_with("项目统计："), "其他插件仍可用");
    }
}
