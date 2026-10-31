//! E04-2：内置静态插件清单 + 宿主通道。
//!
//! 插件只拿到 [`HostSnapshot`]（只读）与 [`PluginAction`]（唯一写入口）；
//! 面板/菜单/命令都由宿主绘制，插件拿不到 `&mut EditorApp`。

use crate::EditorApp;
use crate::menu::{MenuGate, PaletteAction, PaletteItem};
use eframe::egui;
use rustlua_editor_api::{
    Capability, CommandDescriptor, EditorPlugin, HostSnapshot, ImporterDescriptor,
    InspectorSectionDescriptor, MenuItemDescriptor, PanelDescriptor, PluginAction, PluginContext,
    PluginError, PluginHost, PluginManifest, VisualScriptNodeDescriptor,
};
use std::collections::{BTreeMap, BTreeSet};

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

/// 插件命令声明里 `shortcut` 的绑定状态（分发只认 `Bound`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PluginShortcutState {
    /// 已绑定：按下就执行。
    Bound,
    /// 声明文本解析不了，或主键 egui 不认识。
    Unparsable,
    /// 与内置动作的生效键位相同（内置优先，插件这条不触发）；附带占用它的动作名。
    BuiltinConflict(String),
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
        let stored = self.loaded_plugin_states();
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
            if stored.get(&id).is_some_and(|enabled| !enabled) {
                // 用户在这台机器上关掉过它：登记成禁用然后**不 load**——`on_load` 会往全局
                // 可视化脚本节点表里塞东西，加载过的插件即使随后禁用也撤不回来。
                if let Err(error) = self.plugin_registry.set_enabled(&id, false) {
                    let message = format!("插件 {id} 按用户设置禁用失败：{error}");
                    self.plugin_status.push(message.clone());
                    tracing::error!(target: "editor", "[plugin] {message}");
                    continue;
                }
                let message = format!("插件 {id} 已按用户设置禁用");
                self.plugin_status.push(message.clone());
                tracing::info!(target: "editor", "[plugin] {message}");
                continue;
            }
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
        // 恢复上次开着哪些插件面板（用户级偏好；只恢复记为"可见"的）。
        for (panel, visible) in self.loaded_panel_states() {
            if visible {
                self.plugin_panel_visible.insert(panel, true);
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

    /// 插件命令声明的 `shortcut` 此刻的**真实**状态。
    ///
    /// 分发只认 [`PluginShortcutState::Bound`]；另外两种在诊断页照实显示——声明了却按不动的
    /// 键位不该静默失效。
    pub(crate) fn plugin_shortcut_state(&self, text: &str) -> PluginShortcutState {
        let Ok(spec) = editor::ShortcutSpec::parse(text) else {
            return PluginShortcutState::Unparsable;
        };
        if egui::Key::from_name(&spec.key).is_none() {
            return PluginShortcutState::Unparsable;
        }
        match crate::shortcuts::builtin_owner(&self.shortcuts, &spec) {
            Some(owner) => PluginShortcutState::BuiltinConflict(owner.label().to_owned()),
            None => PluginShortcutState::Bound,
        }
    }

    /// 执行本帧按下的插件命令快捷键；`true` = 真的执行了一条。
    ///
    /// 与菜单栏、命令面板、诊断页的「运行」走同一个 [`Self::run_plugin_command`]，
    /// 没有第二条命令通道。门槛与取舍：
    ///
    /// * 文本焦点在输入框里时不触发（与内置快捷键同一门槛）；
    /// * 只认**已加载**插件（禁用/卸载后立刻失效）；
    /// * 与内置动作生效键位相同的声明不绑定——内置优先，插件抢不走 Ctrl+S；
    /// * 解析不了的文本与 egui 不认识的键名不绑定，也不 panic；
    /// * 两条插件命令声明同一键位时按注册表顺序只执行第一条。
    pub(crate) fn run_pressed_plugin_shortcut(&mut self, context: &egui::Context) -> bool {
        if context.wants_keyboard_input() {
            return false;
        }
        let Some(command_id) = context.input(|input| self.pressed_plugin_command(input)) else {
            return false;
        };
        self.run_plugin_command(&command_id);
        true
    }

    fn pressed_plugin_command(&self, input: &egui::InputState) -> Option<String> {
        self.plugin_registry
            .commands()
            .into_iter()
            .find_map(|(_, command)| {
                let text = command.shortcut.as_deref()?;
                if self.plugin_shortcut_state(text) != PluginShortcutState::Bound {
                    return None;
                }
                let spec = editor::ShortcutSpec::parse(text).ok()?;
                crate::shortcuts::pressed(input, &spec).then(|| command.id.clone())
            })
    }

    pub(crate) fn toggle_plugin_panel(&mut self, panel_id: &str) {
        let visible = self
            .plugin_panel_visible
            .entry(panel_id.to_owned())
            .or_insert(false);
        *visible = !*visible;
        self.persist_panel_states();
    }

    /// 已加载插件在命令面板里的候选条目。
    ///
    /// - **菜单条目**按插件自己声明的菜单分组出现（`插件 / 项目统计`），与菜单栏同口径。
    /// - **没被菜单条目引用的命令**单独出现（`插件 / 项目统计 / 刷新项目统计`）；
    ///   已被引用的不重复出现（否则同一条命令在面板里会出现两次）。
    ///
    /// 禁用/卸载的插件不在注册表的"已加载"集合里，因此条目立刻消失——和菜单栏、
    /// 面板一样只认这一份运行时事实。
    pub(crate) fn plugin_palette_items(&self) -> Vec<PaletteItem> {
        let names: BTreeMap<String, String> = self
            .plugin_registry
            .statuses()
            .into_iter()
            .map(|status| (status.id, status.name))
            .collect();
        let mut referenced: BTreeSet<String> = BTreeSet::new();
        let mut items = Vec::new();
        for (_, menu) in self.plugin_registry.menu_items() {
            if let Some(command) = &menu.command {
                referenced.insert(command.clone());
            }
            items.push(PaletteItem {
                path: format!("{} / {}", menu.menu, menu.title),
                shortcut: None,
                action: match &menu.command {
                    Some(command) => PaletteAction::PluginCommand(command.clone()),
                    // 与菜单栏同口径：没挂命令的菜单条目就是开关它自己的面板。
                    None => PaletteAction::PluginPanel(menu.id.clone()),
                },
                gate: MenuGate::Always,
            });
        }
        for (plugin_id, command) in self.plugin_registry.commands() {
            if referenced.contains(&command.id) {
                continue;
            }
            let name = names.get(plugin_id).map_or(plugin_id, String::as_str);
            items.push(PaletteItem {
                path: format!("插件 / {name} / {}", command.title),
                shortcut: command.shortcut.clone(),
                action: PaletteAction::PluginCommand(command.id.clone()),
                gate: MenuGate::Always,
            });
        }
        items
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
            let was_open = open;
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
            if open != was_open {
                // 用户点了窗口的关闭按钮：可见性变了，落盘（下次启动仍是这个状态）。
                self.persist_panel_states();
            }
            let actions = std::mem::take(&mut host.actions);
            self.apply_plugin_actions(actions);
            if let Some(failure) = failure {
                let message = format!("插件面板 {id} 失败：{failure}");
                self.plugin_status.push(message.clone());
                self.status = message;
            }
        }
    }

    /// 启停插件的落盘位置：优先用测试/启动时注入的那份，否则回到默认用户目录。
    fn plugin_config_store(&self) -> Option<editor::UserConfigStore> {
        self.user_config_store()
    }

    /// 启动时读回用户级插件开关；没有主目录或没有该段就是空表（全部按内置默认）。
    fn loaded_plugin_states(&self) -> std::collections::BTreeMap<String, bool> {
        self.plugin_config_store()
            .and_then(|store| store.load_plugin_states())
            .unwrap_or_default()
    }

    /// 启动时读回用户级插件面板可见性；只有记为"可见"的才恢复（默认是关的）。
    fn loaded_panel_states(&self) -> std::collections::BTreeMap<String, bool> {
        self.plugin_config_store()
            .and_then(|store| store.load_panel_states())
            .unwrap_or_default()
    }

    /// 把插件面板的可见性写回用户级设置（`editor.json` 的 `panels` 段）。
    ///
    /// 写的是**整张已知表**（含 `false`）：只写"可见的那些"会让关掉的面板在文件里
    /// 仍是 `true`，下次启动又自己弹出来。
    fn persist_panel_states(&self) -> Option<std::io::Result<()>> {
        let store = self.plugin_config_store()?;
        let states = self
            .plugin_panel_visible
            .iter()
            .map(|(panel, visible)| (panel.clone(), *visible))
            .collect::<std::collections::BTreeMap<_, _>>();
        Some(store.save_panel_states(&states))
    }

    /// 把注册表里所有插件的当前启停状态写回用户级设置。
    ///
    /// 返回 `None` = 没有可用的用户目录（本次进程有效，下次启动回到默认）；写失败也照样
    /// 让内存生效，只是如实告诉用户下次不保留——启停是即时生效的 UI 动作，不该被磁盘挡住。
    fn persist_plugin_states(&self) -> Option<std::io::Result<()>> {
        let store = self.plugin_config_store()?;
        let states = self
            .plugin_registry
            .names()
            .into_iter()
            .map(|id| {
                let enabled = self.plugin_registry.is_enabled(&id);
                (id, enabled)
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        Some(store.save_plugin_states(&states))
    }

    /// 启用/禁用插件（诊断页按钮与测试共用）；错误进状态栏与插件日志。
    ///
    /// 成功时把**全部**插件的当前状态一起落盘（用户级设置，换工程也保留），所以禁用一次
    /// 之后重启仍是禁用的——这条正是"扩展生态"能用的前提。
    pub(crate) fn set_plugin_enabled(&mut self, id: &str, enabled: bool) {
        match self.plugin_registry.set_enabled(id, enabled) {
            Ok(()) => {
                let action = if enabled { "启用" } else { "禁用" };
                let message = format!("插件 {id} 已{action}");
                self.plugin_status.push(message.clone());
                self.status = match self.persist_plugin_states() {
                    Some(Ok(())) => message,
                    Some(Err(error)) => {
                        let notice = format!("插件状态保存失败：{error}");
                        self.plugin_status.push(notice.clone());
                        tracing::warn!(target: "editor", "[plugin] {notice}");
                        format!("{message}（本次有效，但保存失败，下次启动不保留：{error}）")
                    }
                    None => format!("{message}（找不到用户主目录，下次启动不保留）"),
                };
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
                    // 插件自己的命令：在这里能直接跑（与菜单栏、命令面板同一条执行入口），
                    // 免得"插件装了却找不到入口"。
                    let commands: Vec<(String, String, Option<String>)> = self
                        .plugin_registry
                        .commands()
                        .into_iter()
                        .filter(|(plugin_id, _)| *plugin_id == status.id)
                        .map(|(_, command)| {
                            (
                                command.id.clone(),
                                command.title.clone(),
                                command.shortcut.clone(),
                            )
                        })
                        .collect();
                    if !commands.is_empty() {
                        ui.weak("命令");
                        for (command_id, title, shortcut) in &commands {
                            ui.horizontal(|ui| {
                                ui.label(title);
                                // 快捷键照实显示状态：声明了却按不动时，这里要说明为什么。
                                if let Some(text) = shortcut {
                                    let note = match self.plugin_shortcut_state(text) {
                                        PluginShortcutState::Bound => text.clone(),
                                        PluginShortcutState::Unparsable => {
                                            format!("{text}（无法解析，未绑定）")
                                        }
                                        PluginShortcutState::BuiltinConflict(owner) => {
                                            format!("{text}（与内置「{owner}」冲突，未绑定）")
                                        }
                                    };
                                    ui.weak(note);
                                }
                                if ui.button("运行").clicked() {
                                    self.run_plugin_command(command_id);
                                }
                            });
                        }
                    }
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

    /// 默认的门落在**临时用户主目录**上。
    ///
    /// 插件开关与面板可见性都会写用户级设置；用真实 `~/.rustlua` 的话，门会改掉开发机
    /// （或 CI）的用户设置，而且下一次运行把它读回来还能让别的门判红。
    fn test_app() -> EditorApp {
        let home = crate::util::temp_user_home("plugins");
        test_app_with_user_config(Some(&home))
    }

    /// 同上，但先把**用户级设置目录**指到临时位置（插件开关就落在那里）。
    ///
    /// 顺序要紧：`user_config` 必须在 `initialize_ui_extensions` 之前设好，否则启动时
    /// 读不到已禁用的插件（真实启动也是这个顺序：`load_user_*` 都早于首帧）。
    fn test_app_with_user_config(user_home: Option<&std::path::Path>) -> EditorApp {
        let mut app = EditorApp::new(
            ProjectInfo {
                root: std::env::temp_dir().join(format!("rustlua-plugins-{}", std::process::id())),
                version: "test".to_owned(),
                directories: Vec::new(),
            },
            None,
            LogSink::new(32),
        );
        if let Some(user_home) = user_home {
            app.user_config = Some(editor::UserConfigStore::at(user_home));
        }
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

    /// 一次独立的"用户主目录"（插件开关的落点）；每次调用都新建，避免测试之间串味。
    fn temp_user_home(tag: &str) -> std::path::PathBuf {
        crate::util::temp_user_home(tag)
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
        frame_input_with(Vec::new())
    }

    fn frame_input_with(events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            events,
            ..egui::RawInput::default()
        }
    }

    /// 画面上所有文字及其左上角坐标。
    fn text_positions(output: &egui::FullOutput) -> Vec<(String, egui::Pos2)> {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some((text.galley.text().to_string(), text.pos)),
                _ => None,
            })
            .collect()
    }

    /// 找到与 `label` 同一行的「运行」按钮文字位置。
    ///
    /// 诊断页里每个命令一行、行尾一个「运行」按钮；按行（y）配对才不会点到别的命令。
    fn run_button_position(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
        let texts = text_positions(output);
        let label_pos = texts
            .iter()
            .find(|(text, _)| text == label)
            .map(|(_, pos)| *pos)
            .unwrap_or_else(|| panic!("诊断页里没有「{label}」这一行：{texts:?}"));
        texts
            .iter()
            .filter(|(text, _)| text == "运行")
            .min_by(|(_, a), (_, b)| {
                (a.y - label_pos.y)
                    .abs()
                    .total_cmp(&(b.y - label_pos.y).abs())
            })
            .map(|(_, pos)| *pos)
            .unwrap_or_else(|| panic!("诊断页里没有「运行」按钮：{texts:?}"))
    }

    fn pointer_button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
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

    /// 声明了 `shortcut` 的插件命令（快捷键真绑定门的被测件）。
    struct ShortcutPlugin {
        shortcut: &'static str,
    }

    impl EditorPlugin for ShortcutPlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest::new("shortcut_demo", "快捷键示例", "1.0.0")
                .with_capability(Capability::Command)
        }

        fn register(&mut self, context: &mut PluginContext) {
            let mut command = CommandDescriptor::new("shortcut_demo.report", "报告状态");
            command.shortcut = Some(self.shortcut.to_owned());
            context.command(command);
            context.command_handler("shortcut_demo.report", |host| {
                host.submit(PluginAction::SetStatus("插件快捷键命中".to_owned()));
            });
        }
    }

    fn shortcut_app(shortcut: &'static str) -> EditorApp {
        let mut app = test_app();
        app.plugin_registry
            .register(Box::new(ShortcutPlugin { shortcut }))
            .unwrap();
        app.plugin_registry.load("shortcut_demo").unwrap();
        app
    }

    fn key_event(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn ctrl_alt() -> egui::Modifiers {
        egui::Modifiers {
            ctrl: true,
            alt: true,
            command: true,
            ..egui::Modifiers::default()
        }
    }

    /// 真按一次：`RawInput` 里塞一条按键事件，走 `run_pressed_plugin_shortcut`。
    fn press_plugin_shortcut(
        app: &mut EditorApp,
        key: egui::Key,
        modifiers: egui::Modifiers,
    ) -> bool {
        let context = egui::Context::default();
        let mut fired = false;
        let _ = context.run(
            frame_input_with(vec![key_event(key, modifiers)]),
            |context| {
                fired = app.run_pressed_plugin_shortcut(context);
            },
        );
        fired
    }

    /// **插件命令声明的 `shortcut` 真的能按**（docs/73）：走与菜单/命令面板/诊断页
    /// 同一条命令入口；内置动作优先；禁用插件即失效；坏声明不 panic 且诊断页说明原因。
    ///
    /// 修复前这条声明只显示不触发——"面板里写着 Ctrl+…却按不动"是最典型的假入口。
    #[test]
    fn plugin_command_shortcuts_are_really_bound() {
        // 1) 声明的键位触发命令（状态栏由插件动作写入）。
        let mut app = shortcut_app("Ctrl+Alt+9");
        assert_eq!(
            app.plugin_shortcut_state("Ctrl+Alt+9"),
            PluginShortcutState::Bound
        );
        assert!(press_plugin_shortcut(&mut app, egui::Key::Num9, ctrl_alt()));
        assert_eq!(app.status, "插件快捷键命中");

        // 2) 修饰键不精确匹配不触发（少按 Alt）。
        app.status = "未触发".to_owned();
        assert!(!press_plugin_shortcut(
            &mut app,
            egui::Key::Num9,
            egui::Modifiers {
                ctrl: true,
                command: true,
                ..egui::Modifiers::default()
            }
        ));
        assert_eq!(app.status, "未触发");

        // 3) 与内置动作（保存场景 Ctrl+S）冲突：内置优先，插件那条不执行。
        let mut app = shortcut_app("Ctrl+S");
        assert_eq!(
            app.plugin_shortcut_state("Ctrl+S"),
            PluginShortcutState::BuiltinConflict("保存场景".to_owned())
        );
        assert!(!press_plugin_shortcut(
            &mut app,
            egui::Key::S,
            egui::Modifiers {
                ctrl: true,
                command: true,
                ..egui::Modifiers::default()
            }
        ));
        assert_ne!(app.status, "插件快捷键命中");

        // 4) 禁用插件后快捷键立刻失效（只认已加载插件）。
        let mut app = shortcut_app("Ctrl+Alt+9");
        app.plugin_registry
            .set_enabled("shortcut_demo", false)
            .unwrap();
        assert!(!press_plugin_shortcut(
            &mut app,
            egui::Key::Num9,
            ctrl_alt()
        ));

        // 5) 坏声明：不绑定、不 panic；诊断页照实写明原因。
        let mut app = shortcut_app("Ctrl+");
        assert_eq!(
            app.plugin_shortcut_state("Ctrl+"),
            PluginShortcutState::Unparsable
        );
        assert!(!press_plugin_shortcut(
            &mut app,
            egui::Key::Num9,
            ctrl_alt()
        ));
        app.show_plugin_diagnostics = true;
        let context = egui::Context::default();
        let _ = context.run(frame_input(), |context| {
            app.draw_plugin_diagnostics(context);
        });
        let output = context.run(frame_input(), |context| {
            app.draw_plugin_diagnostics(context);
        });
        let text = collect_text(&output);
        assert!(text.contains("无法解析，未绑定"), "text={text:?}");
    }

    /// 输入框有焦点时，插件快捷键不得抢走按键（与内置快捷键同一门槛）。
    #[test]
    fn plugin_shortcuts_do_not_fire_while_a_text_field_has_focus() {
        let mut app = shortcut_app("Ctrl+Alt+9");
        let context = egui::Context::default();
        let mut text = String::new();
        let _ = context.run(frame_input(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                ui.add(egui::TextEdit::singleline(&mut text))
                    .request_focus();
            });
        });
        assert!(
            context.wants_keyboard_input(),
            "前置：输入框应当持有键盘焦点"
        );
        let mut fired = true;
        let _ = context.run(
            frame_input_with(vec![key_event(egui::Key::Num9, ctrl_alt())]),
            |context| {
                // 每帧都得画出文本框，焦点才会一直留在它身上。
                egui::CentralPanel::default().show(context, |ui| {
                    ui.add(egui::TextEdit::singleline(&mut text));
                });
                fired = app.run_pressed_plugin_shortcut(context);
            },
        );
        assert!(!fired, "输入框聚焦时快捷键不得触发插件命令");
        assert_ne!(app.status, "插件快捷键命中");
    }

    #[test]
    fn diagnostics_lists_plugins_and_controls_enable_and_reload() {
        // 启停现在会写用户级设置：这条门必须落在临时目录上，不然它会把开发机（或 CI）
        // 上真实的 `~/.rustlua/editor.json` 里的插件开关改掉。
        let home = temp_user_home("diagnostics");
        let mut app = test_app_with_user_config(Some(&home));
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
        let _ = std::fs::remove_dir_all(home);
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

    /// **禁用一个插件必须活过一次重启**（用户级 `~/.rustlua/editor.json` 的 `plugins` 段）。
    ///
    /// 扩展生态能不能真用就看这条：每次启动都回到"全部启用"的话，用户关掉的那个插件
    /// 每次开机都会跑一遍 `on_load`——它往**全局**可视化脚本节点表里塞节点，进程内
    /// `set_enabled(false)` 撤不回来；菜单、面板、命令也全都回来了。
    ///
    /// 顺带钉住另一半：重新启用同样落盘，第三次启动又是可用的；且没被关的 `core`
    /// 一直按当前整张表写回，不会因为它没被点过就从文件里消失。
    ///
    /// 插件条目必须进命令面板：菜单条目按它声明的菜单分组，独立的命令单独出现，
    /// 已被菜单引用的命令**不重复**出现；禁用插件后立刻消失。
    ///
    /// 扩展生态能不能真用就看这条：插件的命令只能从插件诊断页点到的话，等于没有。
    #[test]
    fn plugin_entries_join_the_command_palette() {
        let mut app = test_app();
        let candidates = app.palette_candidates();
        let paths: Vec<&str> = candidates.iter().map(|item| item.path.as_str()).collect();
        assert!(
            paths.contains(&"插件 / 项目统计"),
            "插件菜单条目应在面板里：{paths:?}"
        );
        assert!(
            paths.contains(&"插件 / 项目统计 / 刷新项目统计"),
            "没被菜单引用的命令应单独出现：{paths:?}"
        );
        assert!(
            !paths.contains(&"插件 / 项目统计 / 显示/隐藏项目统计面板"),
            "已被菜单引用的命令不重复出现：{paths:?}"
        );

        // 过滤与内置条目同一套（多词 AND）。
        let hits: Vec<&str> = crate::menu::filter_palette_items(&candidates, "插件 刷新")
            .iter()
            .map(|item| item.path.as_str())
            .collect();
        assert_eq!(hits, vec!["插件 / 项目统计 / 刷新项目统计"], "{hits:?}");

        // 执行目标写明是插件命令（不是内置菜单动作）。
        let command = candidates
            .iter()
            .find(|item| item.path == "插件 / 项目统计 / 刷新项目统计")
            .expect("候选里有这条");
        assert_eq!(
            command.action,
            PaletteAction::PluginCommand("project_stats.refresh".to_owned())
        );

        // 禁用后条目立刻消失（注册表只报已加载插件）。
        app.plugin_registry
            .set_enabled("project_stats", false)
            .unwrap();
        let paths: Vec<String> = app
            .palette_candidates()
            .into_iter()
            .map(|item| item.path)
            .collect();
        assert!(
            !paths.iter().any(|path| path.starts_with("插件 /")),
            "禁用后插件条目必须消失：{paths:?}"
        );
    }

    /// **禁用一个插件必须活过一次重启**（用户级 `~/.rustlua/editor.json` 的 `plugins` 段）。
    ///
    /// 扩展生态能不能真用就看这条：每次启动都回到"全部启用"的话，用户关掉的那个插件
    /// 每次开机都会跑一遍 `on_load`——它往**全局**可视化脚本节点表里塞节点，进程内
    /// `set_enabled(false)` 撤不回来；菜单、面板、命令也全都回来了。
    #[test]
    fn disabled_plugin_stays_disabled_across_a_restart() {
        let home = temp_user_home("restart");
        let config = editor::UserConfigStore::at(&home).config_path();
        {
            let mut app = test_app_with_user_config(Some(&home));
            app.set_plugin_enabled("project_stats", false);
            assert!(
                app.status.starts_with("插件 project_stats 已禁用"),
                "状态栏要如实说明：{}",
                app.status
            );
            assert!(!app.plugin_registry.is_enabled("project_stats"));
            assert!(!app.plugin_registry.is_loaded("project_stats"));
        }
        let text = std::fs::read_to_string(&config).expect("启停应写进用户级设置");
        assert!(
            text.contains("\"project_stats\": false"),
            "禁用没落盘：{text}"
        );
        assert!(
            text.contains("\"core\": true"),
            "整张表都要写，没被点过的插件不能从文件里消失：{text}"
        );

        // 第二个进程、同一份用户目录：注册表里仍是禁用，而且根本没 load 过。
        let mut again = test_app_with_user_config(Some(&home));
        assert!(
            !again.plugin_registry.is_enabled("project_stats"),
            "重启后必须仍然禁用：{:?}",
            again.plugin_status
        );
        assert!(
            !again.plugin_registry.is_loaded("project_stats"),
            "禁用的插件不该被加载（on_load 会往全局节点表塞东西）"
        );
        assert!(
            again
                .plugin_status
                .iter()
                .any(|message| message.contains("已按用户设置禁用")),
            "启动日志要说清是被用户设置关的，别让人以为插件坏了：{:?}",
            again.plugin_status
        );
        assert!(
            again.plugin_registry.menu_items().is_empty(),
            "禁用后菜单条目不该回来"
        );
        assert!(
            !again.plugin_registry.has_command("project_stats.refresh"),
            "禁用后命令也不该能被调用"
        );

        // 重新启用：立刻可用，并且也写回文件；第三次启动仍是启用。
        again.set_plugin_enabled("project_stats", true);
        assert!(again.plugin_registry.is_loaded("project_stats"));
        let text = std::fs::read_to_string(&config).expect("editor.json 仍在");
        assert!(
            text.contains("\"project_stats\": true"),
            "启用没落盘：{text}"
        );
        let third = test_app_with_user_config(Some(&home));
        assert!(third.plugin_registry.is_enabled("project_stats"));
        assert!(third.plugin_registry.is_loaded("project_stats"));
        let _ = std::fs::remove_dir_all(home);
    }

    /// 诊断页的「运行」按钮必须**真的能执行**插件命令。
    ///
    /// `rustlua-editor-api` 与 docs/73 都写着诊断页能"运行"插件命令，所以这条门盯的是
    /// 那个按钮，而不是 `run_plugin_command` 这个函数存不存在——直接调函数只能证明函数
    /// 在，证明不了按钮接到了它。这里做的是**真点击**：把「运行」按钮的文字位置换算成
    /// 指针事件（按下 + 抬起），走 egui 自己的命中判定。
    #[test]
    fn plugin_commands_can_be_run_from_the_diagnostics_page() {
        let home = temp_user_home("diagnostics-run");
        let mut app = test_app_with_user_config(Some(&home));
        app.show_plugin_diagnostics = true;
        let context = egui::Context::default();
        let draw = |app: &mut EditorApp| {
            let _ = context.run(frame_input(), |context| {
                app.draw_plugin_diagnostics(context);
            });
            context.run(frame_input(), |context| {
                app.draw_plugin_diagnostics(context);
            })
        };

        let output = draw(&mut app);
        let texts = text_positions(&output);
        assert!(
            texts.iter().any(|(text, _)| text == "刷新项目统计"),
            "诊断页应列出插件的命令：{texts:?}"
        );
        assert!(
            texts.iter().any(|(text, _)| text == "运行"),
            "诊断页应有「运行」按钮：{texts:?}"
        );

        // 点「刷新项目统计」那一行的「运行」。
        let position = run_button_position(&output, "刷新项目统计") + egui::vec2(3.0, 3.0);
        let _ = context.run(
            frame_input_with(vec![pointer_button(position, true)]),
            |context| {
                app.draw_plugin_diagnostics(context);
            },
        );
        let _ = context.run(
            frame_input_with(vec![pointer_button(position, false)]),
            |context| {
                app.draw_plugin_diagnostics(context);
            },
        );
        assert!(
            app.status.starts_with("项目统计："),
            "诊断页的「运行」应真的执行插件命令：status={:?}",
            app.status
        );

        let _ = std::fs::remove_dir_all(home);
    }

    /// **插件面板的可见性必须活过一次重启**（用户级 `editor.json` 的 `panels` 段）。
    ///
    /// 插件开关已经是用户级的了，面板可见性却只活在内存里：重启后自己开的插件面板
    /// 全没了，用户得每次重新点一遍——扩展生态的"我装的东西"就记不住。
    #[test]
    fn plugin_panel_visibility_survives_a_restart() {
        let home = temp_user_home("panel-visibility");
        let config = editor::UserConfigStore::at(&home).config_path();
        {
            let mut app = test_app_with_user_config(Some(&home));
            assert_eq!(app.plugin_panel_visible.get("project_stats.panel"), None);
            app.toggle_plugin_panel("project_stats.panel");
            assert_eq!(
                app.plugin_panel_visible.get("project_stats.panel"),
                Some(&true)
            );
        }
        let text = std::fs::read_to_string(&config).expect("面板可见性应写进用户级设置");
        assert!(
            text.contains("\"project_stats.panel\": true"),
            "可见性没落盘：{text}"
        );

        // 第二个进程、同一份用户目录：面板仍然是开着的。
        let again = test_app_with_user_config(Some(&home));
        assert_eq!(
            again.plugin_panel_visible.get("project_stats.panel"),
            Some(&true),
            "重启后面板应仍然可见：{:?}",
            again.plugin_panel_visible
        );

        // 关掉同样要落盘：不然关掉的面板下次启动又自己弹出来。
        let mut third = test_app_with_user_config(Some(&home));
        third.toggle_plugin_panel("project_stats.panel");
        let text = std::fs::read_to_string(&config).expect("editor.json 仍在");
        assert!(
            text.contains("\"project_stats.panel\": false"),
            "关闭没落盘：{text}"
        );
        let fourth = test_app_with_user_config(Some(&home));
        assert_eq!(
            fourth.plugin_panel_visible.get("project_stats.panel"),
            None,
            "关掉的面板不该在下次启动时自己回来：{:?}",
            fourth.plugin_panel_visible
        );
        let _ = std::fs::remove_dir_all(home);
    }
}
