//! P3-E04-1：编辑器插件 API（manifest、能力描述、注册表与生命周期）。
//!
//! 这是**静态编译插件**的唯一契约层：叶子 crate（不依赖任何 workspace crate），
//! 编辑器依赖它，它不知道编辑器、场景或渲染的存在。v1 不承诺 Rust 动态 ABI；
//! 进程内 Rust 插件是受信代码，[`Permission`] 只是声明，不是沙箱。
//!
//! E04-2 起，面板与命令可以携带回调：面板只拿到 `&mut dyn PluginHost`（读快照 +
//! 提交 [`PluginAction`]），命令同理；插件永远拿不到 `&mut EditorApp`。检查器/导入器/
//! 可视化脚本节点的运行时消费属于 E04-3，诊断页属于 E04-4。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::panic::AssertUnwindSafe;

pub const PLUGIN_API_VERSION: u32 = 1;

/// 插件声明要注册的能力。注册表只校验声明与上下文是否一致由宿主决定；
/// 声明本身用于诊断页和"禁用后哪些入口应消失"。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Capability {
    Panel,
    MenuItem,
    Command,
    InspectorSection,
    AssetImporter,
    VisualScriptNode,
    SettingsPage,
}

/// 声明式权限。**进程内 Rust 插件是受信代码，权限位不构成沙箱**；
/// 它们只为未来的 Lua/进程外插件保留一致的能力清单。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Permission {
    ProjectRead,
    ProjectWrite,
    Network,
    Process,
}

/// 宿主给插件看的只读快照。插件不持有 `&mut EditorApp`，也不读文档内部结构。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HostSnapshot {
    pub project_root: Option<String>,
    pub scene_name: Option<String>,
    pub selected_entity: Option<u64>,
    pub entity_count: usize,
    pub resource_count: usize,
    pub playing: bool,
    /// 当前可见的插件面板 id（菜单/面板绘制据此显示开/关）。
    pub visible_panels: BTreeSet<String>,
}

/// 插件对编辑器的唯一写入通道：宿主收集后在本帧末统一应用。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PluginAction {
    SetStatus(String),
    SetPanelVisible { panel: String, visible: bool },
    TogglePanel { panel: String },
}

/// 插件宿主接口。只读状态走 [`PluginHost::snapshot`]，写操作一律 [`PluginHost::submit`]。
pub trait PluginHost {
    fn snapshot(&self) -> &HostSnapshot;
    fn submit(&mut self, action: PluginAction);
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: u32,
    pub capabilities: BTreeSet<Capability>,
    pub permissions: BTreeSet<Permission>,
    /// 必须先加载的插件 id（依赖顺序由注册表保证）。
    pub dependencies: Vec<String>,
}

impl PluginManifest {
    #[must_use]
    pub fn new(id: impl Into<String>, name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            version: version.into(),
            api_version: PLUGIN_API_VERSION,
            capabilities: BTreeSet::new(),
            permissions: BTreeSet::new(),
            dependencies: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_api_version(mut self, api_version: u32) -> Self {
        self.api_version = api_version;
        self
    }

    #[must_use]
    pub fn with_capability(mut self, capability: Capability) -> Self {
        self.capabilities.insert(capability);
        self
    }

    #[must_use]
    pub fn with_dependency(mut self, id: impl Into<String>) -> Self {
        self.dependencies.push(id.into());
        self
    }
}

/// 面板描述。绘制回调在 E04-2 接线；本阶段只校验 id/title。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PanelDescriptor {
    pub id: String,
    pub title: String,
    pub default_dock: Option<String>,
}

impl PanelDescriptor {
    #[must_use]
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            default_dock: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MenuItemDescriptor {
    pub menu: String,
    pub id: String,
    pub title: String,
    pub command: Option<String>,
}

impl MenuItemDescriptor {
    #[must_use]
    pub fn new(menu: impl Into<String>, id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            menu: menu.into(),
            id: id.into(),
            title: title.into(),
            command: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandDescriptor {
    pub id: String,
    pub title: String,
    pub shortcut: Option<String>,
}

impl CommandDescriptor {
    #[must_use]
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            shortcut: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectorSectionDescriptor {
    pub id: String,
    pub title: String,
}

impl InspectorSectionDescriptor {
    #[must_use]
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImporterDescriptor {
    pub id: String,
    /// 小写扩展名，不含点（`png`、`anim`）。
    pub extensions: Vec<String>,
}

impl ImporterDescriptor {
    #[must_use]
    pub fn new(id: impl Into<String>, extensions: Vec<String>) -> Self {
        Self {
            id: id.into(),
            extensions,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VisualScriptNodeDescriptor {
    pub kind: String,
    pub title: String,
}

impl VisualScriptNodeDescriptor {
    #[must_use]
    pub fn new(kind: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            title: title.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingsPageDescriptor {
    pub id: String,
    pub title: String,
}

impl SettingsPageDescriptor {
    #[must_use]
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
        }
    }
}

/// 一次 `register` 期间插件登记的扩展点。卸载时整份替换，宿主据此移除入口。
///
/// 回调不能 `Clone`/`Debug`，因此上下文只实现 `Default`；描述符本身仍是可比较的纯数据。
#[derive(Default)]
pub struct PluginContext {
    panels: Vec<PanelDescriptor>,
    menu_items: Vec<MenuItemDescriptor>,
    commands: Vec<CommandDescriptor>,
    inspector_sections: Vec<InspectorSectionDescriptor>,
    asset_importers: Vec<ImporterDescriptor>,
    visual_script_nodes: Vec<VisualScriptNodeDescriptor>,
    settings_pages: Vec<SettingsPageDescriptor>,
    panel_draws: BTreeMap<String, PanelDraw>,
    command_handlers: BTreeMap<String, CommandHandler>,
    inspector_draws: BTreeMap<String, PanelDraw>,
}

type PanelDraw = Box<dyn FnMut(&mut egui::Ui, &mut dyn PluginHost)>;
type CommandHandler = Box<dyn FnMut(&mut dyn PluginHost)>;
type PanelDrawRef<'a> = &'a mut (dyn FnMut(&mut egui::Ui, &mut dyn PluginHost) + 'static);
type CommandHandlerRef<'a> = &'a mut (dyn FnMut(&mut dyn PluginHost) + 'static);

impl PluginContext {
    pub fn panel(&mut self, descriptor: PanelDescriptor) {
        self.panels.push(descriptor);
    }

    pub fn menu_item(&mut self, descriptor: MenuItemDescriptor) {
        self.menu_items.push(descriptor);
    }

    pub fn command(&mut self, descriptor: CommandDescriptor) {
        self.commands.push(descriptor);
    }

    pub fn inspector_section(&mut self, descriptor: InspectorSectionDescriptor) {
        self.inspector_sections.push(descriptor);
    }

    pub fn asset_importer(&mut self, descriptor: ImporterDescriptor) {
        self.asset_importers.push(descriptor);
    }

    pub fn visual_script_node(&mut self, descriptor: VisualScriptNodeDescriptor) {
        self.visual_script_nodes.push(descriptor);
    }

    pub fn settings_page(&mut self, descriptor: SettingsPageDescriptor) {
        self.settings_pages.push(descriptor);
    }

    /// 为已声明的面板挂绘制回调。`panel_id` 必须在同一上下文里 `panel()` 过。
    pub fn panel_draw(
        &mut self,
        panel_id: impl Into<String>,
        draw: impl FnMut(&mut egui::Ui, &mut dyn PluginHost) + 'static,
    ) {
        self.panel_draws.insert(panel_id.into(), Box::new(draw));
    }

    /// 为已声明的检查器段挂绘制回调（只在有选中实体时由 Inspector 调用）。
    pub fn inspector_section_draw(
        &mut self,
        section_id: impl Into<String>,
        draw: impl FnMut(&mut egui::Ui, &mut dyn PluginHost) + 'static,
    ) {
        self.inspector_draws
            .insert(section_id.into(), Box::new(draw));
    }

    /// 为已声明的命令挂执行回调。菜单条目点击与诊断页“运行”都会走这里。
    pub fn command_handler(
        &mut self,
        command_id: impl Into<String>,
        handler: impl FnMut(&mut dyn PluginHost) + 'static,
    ) {
        self.command_handlers
            .insert(command_id.into(), Box::new(handler));
    }

    pub(crate) fn panel_draw_for(&mut self, id: &str) -> Option<PanelDrawRef<'_>> {
        self.panel_draws.get_mut(id).map(Box::as_mut)
    }

    pub(crate) fn command_handler_for(&mut self, id: &str) -> Option<CommandHandlerRef<'_>> {
        self.command_handlers.get_mut(id).map(Box::as_mut)
    }

    pub(crate) fn inspector_draw_for(&mut self, id: &str) -> Option<PanelDrawRef<'_>> {
        self.inspector_draws.get_mut(id).map(Box::as_mut)
    }

    pub(crate) fn panel_draw_ids(&self) -> impl Iterator<Item = &str> {
        self.panel_draws.keys().map(String::as_str)
    }

    pub(crate) fn command_handler_ids(&self) -> impl Iterator<Item = &str> {
        self.command_handlers.keys().map(String::as_str)
    }

    pub(crate) fn inspector_draw_ids(&self) -> impl Iterator<Item = &str> {
        self.inspector_draws.keys().map(String::as_str)
    }

    #[must_use]
    pub fn panels(&self) -> &[PanelDescriptor] {
        &self.panels
    }

    #[must_use]
    pub fn menu_items(&self) -> &[MenuItemDescriptor] {
        &self.menu_items
    }

    #[must_use]
    pub fn commands(&self) -> &[CommandDescriptor] {
        &self.commands
    }

    #[must_use]
    pub fn inspector_sections(&self) -> &[InspectorSectionDescriptor] {
        &self.inspector_sections
    }

    #[must_use]
    pub fn asset_importers(&self) -> &[ImporterDescriptor] {
        &self.asset_importers
    }

    #[must_use]
    pub fn visual_script_nodes(&self) -> &[VisualScriptNodeDescriptor] {
        &self.visual_script_nodes
    }

    #[must_use]
    pub fn settings_pages(&self) -> &[SettingsPageDescriptor] {
        &self.settings_pages
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

/// 静态编译插件。`manifest` 只应描述元数据；扩展点必须在 `register` 里登记，
/// 这样禁用/重新加载时上下文可以整份重建。
pub trait EditorPlugin {
    fn manifest(&self) -> PluginManifest;
    fn register(&mut self, _context: &mut PluginContext) {}
    fn on_load(&mut self) -> Result<(), PluginError> {
        Ok(())
    }
    fn on_unload(&mut self) -> Result<(), PluginError> {
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PluginError {
    Duplicate(String),
    Missing(String),
    InvalidManifest {
        plugin: String,
        reason: String,
    },
    /// `register` 登记的扩展点冲突或自相矛盾（空 id/跨插件重名/回调无声明）。
    Context {
        plugin: String,
        reason: String,
    },
    IncompatibleVersion {
        plugin: String,
        found: u32,
    },
    MissingDependency {
        plugin: String,
        dependency: String,
    },
    DependencyCycle {
        plugin: String,
    },
    HasDependents {
        plugin: String,
        dependents: Vec<String>,
    },
    Disabled(String),
    Load(String),
    Unload(String),
    Panicked(String),
}

impl fmt::Display for PluginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Duplicate(id) => write!(formatter, "plugin {id:?} is already registered"),
            Self::Missing(id) => write!(formatter, "plugin {id:?} is not registered"),
            Self::InvalidManifest { plugin, reason } => {
                write!(
                    formatter,
                    "plugin {plugin:?} has an invalid manifest: {reason}"
                )
            }
            Self::Context { plugin, reason } => {
                write!(
                    formatter,
                    "plugin {plugin:?} registered conflicting extensions: {reason}"
                )
            }
            Self::IncompatibleVersion { plugin, found } => write!(
                formatter,
                "plugin {plugin:?} uses API v{found}; editor requires v{PLUGIN_API_VERSION}"
            ),
            Self::MissingDependency { plugin, dependency } => {
                write!(
                    formatter,
                    "plugin {plugin:?} depends on missing plugin {dependency:?}"
                )
            }
            Self::DependencyCycle { plugin } => {
                write!(formatter, "plugin dependency cycle at {plugin:?}")
            }
            Self::HasDependents { plugin, dependents } => write!(
                formatter,
                "plugin {plugin:?} is required by loaded plugins {dependents:?}"
            ),
            Self::Disabled(id) => write!(formatter, "plugin {id:?} is disabled"),
            Self::Load(id) => write!(formatter, "plugin {id:?} failed to load"),
            Self::Unload(id) => write!(formatter, "plugin {id:?} failed to unload"),
            Self::Panicked(id) => write!(formatter, "plugin {id:?} panicked"),
        }
    }
}

impl std::error::Error for PluginError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginStatus {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: u32,
    pub capabilities: Vec<Capability>,
    pub enabled: bool,
    pub loaded: bool,
    pub error: Option<String>,
}

struct RegisteredPlugin {
    plugin: Box<dyn EditorPlugin>,
    manifest: PluginManifest,
    context: PluginContext,
    enabled: bool,
    loaded: bool,
    error: Option<String>,
}

#[derive(Default)]
struct TakenExtensions {
    panels: BTreeSet<String>,
    commands: BTreeSet<String>,
    inspector_sections: BTreeSet<String>,
    importer_extensions: BTreeSet<String>,
    node_kinds: BTreeSet<String>,
}

/// 插件注册表：manifest 校验、依赖顺序、启用/禁用、panic 隔离与诊断。
#[derive(Default)]
pub struct PluginRegistry {
    plugins: BTreeMap<String, RegisteredPlugin>,
}

impl PluginRegistry {
    pub fn register(&mut self, plugin: Box<dyn EditorPlugin>) -> Result<String, PluginError> {
        let manifest = plugin.manifest();
        validate_manifest(&manifest)?;
        if self.plugins.contains_key(&manifest.id) {
            return Err(PluginError::Duplicate(manifest.id));
        }
        let id = manifest.id.clone();
        self.plugins.insert(
            id.clone(),
            RegisteredPlugin {
                plugin,
                manifest,
                context: PluginContext::default(),
                enabled: true,
                loaded: false,
                error: None,
            },
        );
        Ok(id)
    }

    /// 按依赖顺序加载；依赖缺失或成环都会拒绝，且不改变任何插件状态。
    pub fn load(&mut self, id: &str) -> Result<(), PluginError> {
        self.load_with_dependencies(id, &mut Vec::new())
    }

    fn load_with_dependencies(
        &mut self,
        id: &str,
        stack: &mut Vec<String>,
    ) -> Result<(), PluginError> {
        if stack.iter().any(|active| active == id) {
            return Err(PluginError::DependencyCycle {
                plugin: id.to_owned(),
            });
        }
        let (enabled, loaded, dependencies) = {
            let entry = self
                .plugins
                .get(id)
                .ok_or_else(|| PluginError::Missing(id.to_owned()))?;
            (
                entry.enabled,
                entry.loaded,
                entry.manifest.dependencies.clone(),
            )
        };
        if !enabled {
            return Err(PluginError::Disabled(id.to_owned()));
        }
        if loaded {
            return Ok(());
        }
        stack.push(id.to_owned());
        let dependencies_result = (|| {
            for dependency in dependencies {
                if !self.plugins.contains_key(&dependency) {
                    return Err(PluginError::MissingDependency {
                        plugin: id.to_owned(),
                        dependency,
                    });
                }
                self.load_with_dependencies(&dependency, stack)?;
            }
            Ok(())
        })();
        stack.pop();
        dependencies_result?;

        let taken = self.taken_extensions(id);
        let entry = self.plugins.get_mut(id).expect("加载前已确认插件存在");
        entry.context.clear();
        entry.error = None;
        if std::panic::catch_unwind(AssertUnwindSafe(|| {
            entry.plugin.register(&mut entry.context);
        }))
        .is_err()
        {
            entry.error = Some("panicked during register".to_owned());
            entry.context.clear();
            return Err(PluginError::Panicked(id.to_owned()));
        }
        if let Err(reason) = validate_context(&entry.context, &taken) {
            entry.error = Some(reason.clone());
            entry.context.clear();
            return Err(PluginError::Context {
                plugin: id.to_owned(),
                reason,
            });
        }
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| entry.plugin.on_load()));
        match result {
            Ok(Ok(())) => {
                entry.loaded = true;
                Ok(())
            }
            Ok(Err(error)) => {
                entry.error = Some(error.to_string());
                entry.context.clear();
                Err(PluginError::Load(id.to_owned()))
            }
            Err(_) => {
                entry.error = Some("panicked during load".to_owned());
                entry.context.clear();
                Err(PluginError::Panicked(id.to_owned()))
            }
        }
    }

    /// 已加载插件占用的扩展点 id（跨插件冲突检测用；`exclude` 是当前插件自己）。
    fn taken_extensions(&self, exclude: &str) -> TakenExtensions {
        let mut taken = TakenExtensions::default();
        for (id, entry) in &self.plugins {
            if id == exclude || !entry.loaded {
                continue;
            }
            taken
                .panels
                .extend(entry.context.panels().iter().map(|panel| panel.id.clone()));
            taken.commands.extend(
                entry
                    .context
                    .commands()
                    .iter()
                    .map(|command| command.id.clone()),
            );
            taken.inspector_sections.extend(
                entry
                    .context
                    .inspector_sections()
                    .iter()
                    .map(|section| section.id.clone()),
            );
            for importer in entry.context.asset_importers() {
                taken
                    .importer_extensions
                    .extend(importer.extensions.iter().cloned());
            }
            taken.node_kinds.extend(
                entry
                    .context
                    .visual_script_nodes()
                    .iter()
                    .map(|node| node.kind.clone()),
            );
        }
        taken
    }

    /// 卸载成功后清空上下文（面板/菜单/命令随之消失）。仍有已加载插件依赖它时拒绝。
    pub fn unload(&mut self, id: &str) -> Result<(), PluginError> {
        let dependents: Vec<String> = self
            .plugins
            .iter()
            .filter(|(candidate, entry)| {
                entry.loaded
                    && *candidate != id
                    && entry.manifest.dependencies.iter().any(|dep| dep == id)
            })
            .map(|(candidate, _)| candidate.clone())
            .collect();
        if !dependents.is_empty() {
            return Err(PluginError::HasDependents {
                plugin: id.to_owned(),
                dependents,
            });
        }
        let Some(entry) = self.plugins.get_mut(id) else {
            return Err(PluginError::Missing(id.to_owned()));
        };
        if !entry.loaded {
            entry.context.clear();
            return Ok(());
        }
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| entry.plugin.on_unload()));
        match result {
            Ok(Ok(())) => {
                entry.loaded = false;
                entry.context.clear();
                Ok(())
            }
            Ok(Err(error)) => {
                entry.error = Some(error.to_string());
                Err(PluginError::Unload(id.to_owned()))
            }
            Err(_) => {
                entry.error = Some("panicked during unload".to_owned());
                Err(PluginError::Panicked(id.to_owned()))
            }
        }
    }

    /// 禁用先卸载；启用会立即加载（依赖未满足时返回错误，但保持启用状态）。
    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<(), PluginError> {
        let current = self
            .plugins
            .get(id)
            .ok_or_else(|| PluginError::Missing(id.to_owned()))?
            .enabled;
        if current == enabled {
            return if enabled { self.load(id) } else { Ok(()) };
        }
        if !enabled {
            self.unload(id)?;
            self.plugins
                .get_mut(id)
                .expect("卸载前已确认插件存在")
                .enabled = false;
            return Ok(());
        }
        self.plugins
            .get_mut(id)
            .expect("设置前已确认插件存在")
            .enabled = true;
        self.load(id)
    }

    /// 静态插件的"重新加载"：卸载后重建上下文，再走一次依赖加载。
    pub fn reload(&mut self, id: &str) -> Result<(), PluginError> {
        self.unload(id)?;
        self.load(id)
    }

    #[must_use]
    pub fn is_loaded(&self, id: &str) -> bool {
        self.plugins.get(id).is_some_and(|entry| entry.loaded)
    }

    #[must_use]
    pub fn is_enabled(&self, id: &str) -> bool {
        self.plugins.get(id).is_some_and(|entry| entry.enabled)
    }

    #[must_use]
    pub fn context(&self, id: &str) -> Option<&PluginContext> {
        self.plugins.get(id).map(|entry| &entry.context)
    }

    /// 已加载插件声明的面板（插件 id, 描述符）。禁用/卸载后立即消失。
    #[must_use]
    pub fn panels(&self) -> Vec<(&str, &PanelDescriptor)> {
        self.plugins
            .iter()
            .filter(|(_, entry)| entry.loaded)
            .flat_map(|(id, entry)| {
                entry
                    .context
                    .panels()
                    .iter()
                    .map(move |descriptor| (id.as_str(), descriptor))
            })
            .collect()
    }

    /// 已加载插件的菜单条目。宿主把它们追加到自带菜单之后。
    #[must_use]
    pub fn menu_items(&self) -> Vec<(&str, &MenuItemDescriptor)> {
        self.plugins
            .iter()
            .filter(|(_, entry)| entry.loaded)
            .flat_map(|(id, entry)| {
                entry
                    .context
                    .menu_items()
                    .iter()
                    .map(move |descriptor| (id.as_str(), descriptor))
            })
            .collect()
    }

    /// 已加载插件的命令。
    #[must_use]
    pub fn commands(&self) -> Vec<(&str, &CommandDescriptor)> {
        self.plugins
            .iter()
            .filter(|(_, entry)| entry.loaded)
            .flat_map(|(id, entry)| {
                entry
                    .context
                    .commands()
                    .iter()
                    .map(move |descriptor| (id.as_str(), descriptor))
            })
            .collect()
    }

    /// 已加载插件的检查器段。
    #[must_use]
    pub fn inspector_sections(&self) -> Vec<(&str, &InspectorSectionDescriptor)> {
        self.plugins
            .iter()
            .filter(|(_, entry)| entry.loaded)
            .flat_map(|(id, entry)| {
                entry
                    .context
                    .inspector_sections()
                    .iter()
                    .map(move |descriptor| (id.as_str(), descriptor))
            })
            .collect()
    }

    /// 已加载插件声明的资源导入器（声明式：v1 不执行自定义导入逻辑）。
    #[must_use]
    pub fn asset_importers(&self) -> Vec<(&str, &ImporterDescriptor)> {
        self.plugins
            .iter()
            .filter(|(_, entry)| entry.loaded)
            .flat_map(|(id, entry)| {
                entry
                    .context
                    .asset_importers()
                    .iter()
                    .map(move |descriptor| (id.as_str(), descriptor))
            })
            .collect()
    }

    /// 已加载插件声明的可视化脚本节点（真实 NodeSpec 由插件在 on_load 注册）。
    #[must_use]
    pub fn visual_script_nodes(&self) -> Vec<(&str, &VisualScriptNodeDescriptor)> {
        self.plugins
            .iter()
            .filter(|(_, entry)| entry.loaded)
            .flat_map(|(id, entry)| {
                entry
                    .context
                    .visual_script_nodes()
                    .iter()
                    .map(move |descriptor| (id.as_str(), descriptor))
            })
            .collect()
    }

    #[must_use]
    pub fn has_command(&self, command_id: &str) -> bool {
        self.plugins.values().any(|entry| {
            entry.loaded
                && entry
                    .context
                    .commands()
                    .iter()
                    .any(|command| command.id == command_id)
        })
    }

    /// 绘制一个已加载插件的面板。panic 被隔离并记进该插件状态，调用方只需显示错误。
    pub fn draw_panel(
        &mut self,
        panel_id: &str,
        ui: &mut egui::Ui,
        host: &mut dyn PluginHost,
    ) -> Result<(), PluginError> {
        let Some(plugin_id) = self.plugins.iter().find_map(|(id, entry)| {
            (entry.loaded
                && entry
                    .context
                    .panels()
                    .iter()
                    .any(|panel| panel.id == panel_id))
            .then(|| id.clone())
        }) else {
            return Err(PluginError::Missing(panel_id.to_owned()));
        };
        let entry = self.plugins.get_mut(&plugin_id).expect("面板所属插件");
        let Some(draw) = entry.context.panel_draw_for(panel_id) else {
            entry.error = Some(format!("panel {panel_id:?} has no draw callback"));
            return Err(PluginError::Missing(panel_id.to_owned()));
        };
        if std::panic::catch_unwind(AssertUnwindSafe(|| draw(ui, host))).is_err() {
            entry.error = Some(format!("panicked while drawing panel {panel_id:?}"));
            return Err(PluginError::Panicked(plugin_id));
        }
        Ok(())
    }

    /// 绘制一个已加载插件的检查器段（调用方负责分隔线与标题）。
    pub fn draw_inspector_section(
        &mut self,
        section_id: &str,
        ui: &mut egui::Ui,
        host: &mut dyn PluginHost,
    ) -> Result<(), PluginError> {
        let Some(plugin_id) = self.plugins.iter().find_map(|(id, entry)| {
            (entry.loaded
                && entry
                    .context
                    .inspector_sections()
                    .iter()
                    .any(|section| section.id == section_id))
            .then(|| id.clone())
        }) else {
            return Err(PluginError::Missing(section_id.to_owned()));
        };
        let entry = self.plugins.get_mut(&plugin_id).expect("检查器段所属插件");
        let Some(draw) = entry.context.inspector_draw_for(section_id) else {
            entry.error = Some(format!(
                "inspector section {section_id:?} has no draw callback"
            ));
            return Err(PluginError::Missing(section_id.to_owned()));
        };
        if std::panic::catch_unwind(AssertUnwindSafe(|| draw(ui, host))).is_err() {
            entry.error = Some(format!(
                "panicked while drawing inspector section {section_id:?}"
            ));
            return Err(PluginError::Panicked(plugin_id));
        }
        Ok(())
    }

    /// 执行一个已加载插件的命令。插件只能经 `host.submit` 影响编辑器。
    pub fn run_command(
        &mut self,
        command_id: &str,
        host: &mut dyn PluginHost,
    ) -> Result<(), PluginError> {
        let Some(plugin_id) = self.plugins.iter().find_map(|(id, entry)| {
            (entry.loaded
                && entry
                    .context
                    .commands()
                    .iter()
                    .any(|command| command.id == command_id))
            .then(|| id.clone())
        }) else {
            return Err(PluginError::Missing(command_id.to_owned()));
        };
        let entry = self.plugins.get_mut(&plugin_id).expect("命令所属插件");
        let Some(handler) = entry.context.command_handler_for(command_id) else {
            entry.error = Some(format!("command {command_id:?} has no handler"));
            return Err(PluginError::Missing(command_id.to_owned()));
        };
        if std::panic::catch_unwind(AssertUnwindSafe(|| handler(host))).is_err() {
            entry.error = Some(format!("panicked while running command {command_id:?}"));
            return Err(PluginError::Panicked(plugin_id));
        }
        Ok(())
    }

    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.plugins.keys().cloned().collect()
    }

    #[must_use]
    pub fn statuses(&self) -> Vec<PluginStatus> {
        self.plugins
            .values()
            .map(|entry| PluginStatus {
                id: entry.manifest.id.clone(),
                name: entry.manifest.name.clone(),
                version: entry.manifest.version.clone(),
                api_version: entry.manifest.api_version,
                capabilities: entry.manifest.capabilities.iter().copied().collect(),
                enabled: entry.enabled,
                loaded: entry.loaded,
                error: entry.error.clone(),
            })
            .collect()
    }
}

fn validate_manifest(manifest: &PluginManifest) -> Result<(), PluginError> {
    if !is_valid_plugin_id(&manifest.id) {
        return Err(PluginError::InvalidManifest {
            plugin: manifest.id.clone(),
            reason: "id must be non-empty ASCII letters/digits/._-".to_owned(),
        });
    }
    if manifest.name.trim().is_empty() {
        return Err(PluginError::InvalidManifest {
            plugin: manifest.id.clone(),
            reason: "name must not be empty".to_owned(),
        });
    }
    if manifest.api_version != PLUGIN_API_VERSION {
        return Err(PluginError::IncompatibleVersion {
            plugin: manifest.id.clone(),
            found: manifest.api_version,
        });
    }
    if manifest.dependencies.iter().any(|dep| dep == &manifest.id) {
        return Err(PluginError::DependencyCycle {
            plugin: manifest.id.clone(),
        });
    }
    Ok(())
}

/// `register` 之后的上下文校验：id 非空、同插件内不重复、不与已加载插件撞名、
/// 回调必须挂在已声明的扩展点上。失败时调用方清空上下文并拒绝加载。
fn validate_context(context: &PluginContext, taken: &TakenExtensions) -> Result<(), String> {
    let mut panel_ids = BTreeSet::new();
    for panel in context.panels() {
        if panel.id.trim().is_empty() {
            return Err("panel id must not be empty".to_owned());
        }
        if !panel_ids.insert(panel.id.as_str()) {
            return Err(format!("duplicate panel id {:?}", panel.id));
        }
        if taken.panels.contains(&panel.id) {
            return Err(format!(
                "panel id {:?} is already registered by another plugin",
                panel.id
            ));
        }
    }
    let mut command_ids = BTreeSet::new();
    for command in context.commands() {
        if command.id.trim().is_empty() {
            return Err("command id must not be empty".to_owned());
        }
        if !command_ids.insert(command.id.as_str()) {
            return Err(format!("duplicate command id {:?}", command.id));
        }
        if taken.commands.contains(&command.id) {
            return Err(format!(
                "command id {:?} is already registered by another plugin",
                command.id
            ));
        }
    }
    let mut section_ids = BTreeSet::new();
    for section in context.inspector_sections() {
        if section.id.trim().is_empty() {
            return Err("inspector section id must not be empty".to_owned());
        }
        if !section_ids.insert(section.id.as_str()) {
            return Err(format!("duplicate inspector section id {:?}", section.id));
        }
        if taken.inspector_sections.contains(&section.id) {
            return Err(format!(
                "inspector section id {:?} is already registered by another plugin",
                section.id
            ));
        }
    }
    let mut importer_extensions = BTreeSet::new();
    for importer in context.asset_importers() {
        if importer.id.trim().is_empty() {
            return Err("importer id must not be empty".to_owned());
        }
        if importer.extensions.is_empty() {
            return Err(format!("importer {:?} claims no extensions", importer.id));
        }
        for extension in &importer.extensions {
            let valid = !extension.is_empty()
                && !extension.starts_with('.')
                && extension
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit());
            if !valid {
                return Err(format!(
                    "importer {:?} extension {extension:?} must be lowercase without a dot",
                    importer.id
                ));
            }
            if !importer_extensions.insert(extension.as_str()) {
                return Err(format!("duplicate importer extension {extension:?}"));
            }
            if taken.importer_extensions.contains(extension) {
                return Err(format!(
                    "importer extension {extension:?} is already claimed by another plugin"
                ));
            }
        }
    }
    let mut node_kinds = BTreeSet::new();
    for node in context.visual_script_nodes() {
        if node.kind.trim().is_empty() {
            return Err("visual script node kind must not be empty".to_owned());
        }
        if !node_kinds.insert(node.kind.as_str()) {
            return Err(format!("duplicate visual script node kind {:?}", node.kind));
        }
        if taken.node_kinds.contains(&node.kind) {
            return Err(format!(
                "visual script node kind {:?} is already registered by another plugin",
                node.kind
            ));
        }
    }
    for menu in context.menu_items() {
        if menu.id.trim().is_empty() || menu.menu.trim().is_empty() {
            return Err("menu item id and menu must not be empty".to_owned());
        }
        if let Some(command) = &menu.command
            && !command_ids.contains(command.as_str())
        {
            return Err(format!(
                "menu item {:?} references unknown command {command:?}",
                menu.id
            ));
        }
    }
    for panel_id in context.panel_draw_ids() {
        if !panel_ids.contains(panel_id) {
            return Err(format!("draw callback for undeclared panel {panel_id:?}"));
        }
    }
    for command_id in context.command_handler_ids() {
        if !command_ids.contains(command_id) {
            return Err(format!("handler for undeclared command {command_id:?}"));
        }
    }
    for section_id in context.inspector_draw_ids() {
        if !section_ids.contains(section_id) {
            return Err(format!(
                "draw callback for undeclared inspector section {section_id:?}"
            ));
        }
    }
    Ok(())
}

fn is_valid_plugin_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Clone, Copy, Default)]
    struct Behavior {
        fail_load: bool,
        panic_load: bool,
        panic_unload: bool,
    }

    struct TestPlugin {
        manifest: PluginManifest,
        log: Rc<RefCell<Vec<String>>>,
        behavior: Behavior,
        register_panel: bool,
    }

    impl TestPlugin {
        fn new(id: &str, log: &Rc<RefCell<Vec<String>>>) -> Self {
            Self {
                manifest: PluginManifest::new(id, id, "1.0.0"),
                log: Rc::clone(log),
                behavior: Behavior::default(),
                register_panel: false,
            }
        }

        fn dependency(mut self, id: &str) -> Self {
            self.manifest.dependencies.push(id.to_owned());
            self
        }

        fn behavior(mut self, behavior: Behavior) -> Self {
            self.behavior = behavior;
            self
        }

        fn panel(mut self) -> Self {
            self.register_panel = true;
            self.manifest.capabilities.insert(Capability::Panel);
            self
        }
    }

    impl EditorPlugin for TestPlugin {
        fn manifest(&self) -> PluginManifest {
            self.manifest.clone()
        }

        fn register(&mut self, context: &mut PluginContext) {
            self.log
                .borrow_mut()
                .push(format!("register:{}", self.manifest.id));
            if self.register_panel {
                context.panel(PanelDescriptor::new(
                    format!("{}.panel", self.manifest.id),
                    format!("{} 面板", self.manifest.id),
                ));
            }
        }

        fn on_load(&mut self) -> Result<(), PluginError> {
            self.log
                .borrow_mut()
                .push(format!("load:{}", self.manifest.id));
            if self.behavior.panic_load {
                panic!("intentional load panic");
            }
            if self.behavior.fail_load {
                return Err(PluginError::Load("intentional".to_owned()));
            }
            Ok(())
        }

        fn on_unload(&mut self) -> Result<(), PluginError> {
            self.log
                .borrow_mut()
                .push(format!("unload:{}", self.manifest.id));
            if self.behavior.panic_unload {
                panic!("intentional unload panic");
            }
            Ok(())
        }
    }

    fn registry_with(log: &Rc<RefCell<Vec<String>>>, plugins: Vec<TestPlugin>) -> PluginRegistry {
        let mut registry = PluginRegistry::default();
        for plugin in plugins {
            registry.register(Box::new(plugin)).expect("注册");
        }
        let _ = log;
        registry
    }

    #[test]
    fn manifest_rejects_bad_ids_versions_and_self_dependency() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut registry = PluginRegistry::default();
        for manifest in [
            PluginManifest::new("bad/id", "bad", "1"),
            PluginManifest::new("", "empty", "1"),
            PluginManifest::new("future", "future", "1").with_api_version(PLUGIN_API_VERSION + 1),
            PluginManifest::new("selfish", "self", "1").with_dependency("selfish"),
        ] {
            let plugin = TestPlugin {
                manifest,
                log: Rc::clone(&log),
                behavior: Behavior::default(),
                register_panel: false,
            };
            assert!(registry.register(Box::new(plugin)).is_err());
        }
        assert!(registry.names().is_empty());
    }

    #[test]
    fn duplicate_ids_are_rejected_without_replacing_the_first_plugin() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut registry = registry_with(&log, vec![TestPlugin::new("a", &log)]);
        let error = registry
            .register(Box::new(TestPlugin::new("a", &log)))
            .expect_err("重复 id");
        assert_eq!(error, PluginError::Duplicate("a".to_owned()));
        assert_eq!(registry.names(), ["a"]);
    }

    #[test]
    fn dependencies_load_first_and_unload_is_blocked_by_dependents() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut registry = registry_with(
            &log,
            vec![
                TestPlugin::new("base", &log),
                TestPlugin::new("app", &log).dependency("base"),
            ],
        );
        registry.load("app").expect("按依赖顺序加载");
        assert_eq!(
            log.borrow().as_slice(),
            ["register:base", "load:base", "register:app", "load:app"]
        );
        assert!(registry.is_loaded("base"));
        assert!(registry.is_loaded("app"));
        let error = registry.unload("base").expect_err("有已加载依赖方");
        assert_eq!(
            error,
            PluginError::HasDependents {
                plugin: "base".to_owned(),
                dependents: vec!["app".to_owned()],
            }
        );
        registry.unload("app").expect("先卸依赖方");
        registry.unload("base").expect("再无依赖方");
        assert!(!registry.is_loaded("app"));
        assert!(!registry.is_loaded("base"));
    }

    #[test]
    fn missing_dependency_and_cycles_are_rejected() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut registry =
            registry_with(&log, vec![TestPlugin::new("app", &log).dependency("nope")]);
        assert_eq!(
            registry.load("app").expect_err("缺依赖"),
            PluginError::MissingDependency {
                plugin: "app".to_owned(),
                dependency: "nope".to_owned(),
            }
        );
        assert!(!registry.is_loaded("app"));

        let mut registry = registry_with(
            &log,
            vec![
                TestPlugin::new("a", &log).dependency("b"),
                TestPlugin::new("b", &log).dependency("a"),
            ],
        );
        let error = registry.load("a").expect_err("成环应拒绝");
        assert!(matches!(error, PluginError::DependencyCycle { .. }));
        assert!(!registry.is_loaded("a"));
        assert!(!registry.is_loaded("b"));
    }

    #[test]
    fn load_errors_and_panics_are_isolated_and_reported_in_statuses() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut registry = registry_with(
            &log,
            vec![
                TestPlugin::new("ok", &log),
                TestPlugin::new("fail", &log).behavior(Behavior {
                    fail_load: true,
                    ..Behavior::default()
                }),
                TestPlugin::new("panic", &log).behavior(Behavior {
                    panic_load: true,
                    ..Behavior::default()
                }),
            ],
        );
        registry.load("fail").expect_err("返回错误");
        registry.load("panic").expect_err("panic 被隔离");
        registry.load("ok").expect("其他插件不受影响");
        assert!(!registry.is_loaded("fail"));
        assert!(!registry.is_loaded("panic"));
        let statuses = registry.statuses();
        let status = |id: &str| statuses.iter().find(|status| status.id == id).unwrap();
        assert!(status("fail").error.is_some());
        assert!(
            status("panic")
                .error
                .as_deref()
                .unwrap()
                .contains("panicked")
        );
        assert!(status("ok").error.is_none());
    }

    #[test]
    fn disable_unloads_and_enable_rebuilds_context() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut registry = registry_with(&log, vec![TestPlugin::new("app", &log).panel()]);
        registry.load("app").expect("加载");
        assert_eq!(
            registry.context("app").expect("上下文").panels()[0].id,
            "app.panel"
        );

        registry.set_enabled("app", false).expect("禁用");
        assert!(!registry.is_enabled("app"));
        assert!(!registry.is_loaded("app"));
        assert!(
            registry
                .context("app")
                .expect("上下文还在")
                .panels()
                .is_empty()
        );
        assert_eq!(
            registry.load("app").expect_err("禁用后不能加载"),
            PluginError::Disabled("app".to_owned())
        );

        registry.set_enabled("app", true).expect("重新启用并加载");
        assert!(registry.is_enabled("app"));
        assert!(registry.is_loaded("app"));
        assert_eq!(registry.context("app").expect("上下文").panels().len(), 1);
        assert_eq!(
            log.borrow().as_slice(),
            [
                "register:app",
                "load:app",
                "unload:app",
                "register:app",
                "load:app"
            ]
        );
    }

    #[test]
    fn unload_panic_keeps_plugin_loaded_and_reports_error() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut registry = registry_with(
            &log,
            vec![TestPlugin::new("app", &log).behavior(Behavior {
                panic_unload: true,
                ..Behavior::default()
            })],
        );
        registry.load("app").expect("加载");
        assert_eq!(
            registry.unload("app").expect_err("卸载 panic 被隔离"),
            PluginError::Panicked("app".to_owned())
        );
        assert!(registry.is_loaded("app"), "panic 后不能假装已卸载");
        let status = &registry.statuses()[0];
        assert!(status.error.as_deref().unwrap().contains("panicked"));
    }

    // E04-2：面板/菜单/命令回调。

    #[derive(Default)]
    struct UiBehavior {
        draw_panics: bool,
        run_panics: bool,
        seen_entity_count: Rc<RefCell<Option<usize>>>,
    }

    struct UiPlugin {
        manifest: PluginManifest,
        behavior: Rc<UiBehavior>,
        register_missing_draw: bool,
        menu_without_command: bool,
    }

    impl UiPlugin {
        fn new(id: &str) -> Self {
            Self {
                manifest: PluginManifest::new(id, id, "1.0.0"),
                behavior: Rc::new(UiBehavior::default()),
                register_missing_draw: false,
                menu_without_command: false,
            }
        }

        fn behavior(behavior: Rc<UiBehavior>) -> Self {
            Self {
                behavior,
                ..Self::new("ui")
            }
        }
    }

    impl EditorPlugin for UiPlugin {
        fn manifest(&self) -> PluginManifest {
            self.manifest.clone()
        }

        fn register(&mut self, context: &mut PluginContext) {
            let behavior = Rc::clone(&self.behavior);
            context.panel(PanelDescriptor::new("ui.panel", "UI 面板"));
            context.panel_draw("ui.panel", move |ui, host| {
                *behavior.seen_entity_count.borrow_mut() = Some(host.snapshot().entity_count);
                ui.label(format!("实体 {}", host.snapshot().entity_count));
                if behavior.draw_panics {
                    panic!("intentional draw panic");
                }
                host.submit(PluginAction::SetStatus("drawn".to_owned()));
            });
            context.menu_item(MenuItemDescriptor {
                menu: "插件".to_owned(),
                id: "ui.show".to_owned(),
                title: "UI".to_owned(),
                command: Some("ui.toggle".to_owned()),
            });
            context.command(CommandDescriptor::new("ui.toggle", "切换面板"));
            context.command_handler("ui.toggle", |host| {
                host.submit(PluginAction::TogglePanel {
                    panel: "ui.panel".to_owned(),
                });
            });
            context.command(CommandDescriptor::new("ui.run", "运行"));
            let behavior = Rc::clone(&self.behavior);
            context.command_handler("ui.run", move |host| {
                if behavior.run_panics {
                    panic!("intentional command panic");
                }
                host.submit(PluginAction::SetStatus("ran".to_owned()));
            });
            if self.register_missing_draw {
                context.panel_draw("ui.missing", |_, _| {});
            }
            if self.menu_without_command {
                context.menu_item(MenuItemDescriptor {
                    menu: "插件".to_owned(),
                    id: "ui.bad".to_owned(),
                    title: "坏条目".to_owned(),
                    command: Some("ui.unknown".to_owned()),
                });
            }
        }
    }

    struct TestHost {
        snapshot: HostSnapshot,
        actions: Vec<PluginAction>,
    }

    impl TestHost {
        fn new(entity_count: usize) -> Self {
            Self {
                snapshot: HostSnapshot {
                    entity_count,
                    ..HostSnapshot::default()
                },
                actions: Vec::new(),
            }
        }
    }

    impl PluginHost for TestHost {
        fn snapshot(&self) -> &HostSnapshot {
            &self.snapshot
        }

        fn submit(&mut self, action: PluginAction) {
            self.actions.push(action);
        }
    }

    fn draw_panel_once(
        registry: &mut PluginRegistry,
        host: &mut TestHost,
    ) -> Result<(), PluginError> {
        let context = egui::Context::default();
        let mut drawn = Ok(());
        let output = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                drawn = registry.draw_panel("ui.panel", ui, host);
            });
        });
        assert!(!output.shapes.is_empty(), "面板应产生绘制输出");
        drawn
    }

    #[test]
    fn extensions_follow_enable_disable_and_reappear_after_reload() {
        let mut registry = PluginRegistry::default();
        registry.register(Box::new(UiPlugin::new("ui"))).unwrap();
        registry.load("ui").unwrap();
        assert_eq!(registry.panels().len(), 1);
        assert_eq!(registry.menu_items().len(), 1);
        assert_eq!(registry.commands().len(), 2);
        assert!(registry.has_command("ui.run"));

        let mut host = TestHost::new(3);
        registry.run_command("ui.run", &mut host).unwrap();
        assert_eq!(host.actions, [PluginAction::SetStatus("ran".to_owned())]);

        registry.set_enabled("ui", false).unwrap();
        assert!(registry.panels().is_empty());
        assert!(registry.menu_items().is_empty());
        assert!(registry.commands().is_empty());
        assert!(!registry.has_command("ui.run"));
        assert_eq!(
            registry
                .run_command("ui.run", &mut host)
                .expect_err("禁用后命令不可用"),
            PluginError::Missing("ui.run".to_owned())
        );

        registry.set_enabled("ui", true).unwrap();
        assert_eq!(registry.panels().len(), 1);
        assert_eq!(registry.commands().len(), 2);
    }

    #[test]
    fn panel_draw_reads_snapshot_and_writes_only_through_actions() {
        let behavior = Rc::new(UiBehavior::default());
        let seen = Rc::clone(&behavior.seen_entity_count);
        let mut registry = PluginRegistry::default();
        registry
            .register(Box::new(UiPlugin::behavior(behavior)))
            .unwrap();
        registry.load("ui").unwrap();
        let mut host = TestHost::new(7);
        draw_panel_once(&mut registry, &mut host).unwrap();
        assert_eq!(*seen.borrow(), Some(7));
        assert_eq!(
            host.actions,
            [PluginAction::SetStatus("drawn".to_owned())],
            "面板只能提交动作"
        );
    }

    #[test]
    fn draw_and_command_panics_are_isolated_and_reported() {
        let behavior = Rc::new(UiBehavior {
            draw_panics: true,
            run_panics: true,
            seen_entity_count: Rc::new(RefCell::new(None)),
        });
        let mut registry = PluginRegistry::default();
        registry
            .register(Box::new(UiPlugin::behavior(Rc::clone(&behavior))))
            .unwrap();
        registry.load("ui").unwrap();
        let mut host = TestHost::new(1);
        assert_eq!(
            draw_panel_once(&mut registry, &mut host).expect_err("绘制 panic 隔离"),
            PluginError::Panicked("ui".to_owned())
        );
        assert_eq!(
            registry
                .run_command("ui.run", &mut host)
                .expect_err("命令 panic 隔离"),
            PluginError::Panicked("ui".to_owned())
        );
        let status = &registry.statuses()[0];
        assert!(status.error.as_deref().unwrap().contains("panicked"));
        // 隔离之后注册表本身仍可用：禁用/重新启用不受影响。
        registry.set_enabled("ui", false).unwrap();
        registry.set_enabled("ui", true).unwrap();
        assert!(registry.is_loaded("ui"));
    }

    struct ExtPlugin {
        manifest: PluginManifest,
        importer_extension: Option<&'static str>,
        node_kind: Option<&'static str>,
    }

    impl ExtPlugin {
        fn new(id: &str) -> Self {
            Self {
                manifest: PluginManifest::new(id, id, "1.0.0"),
                importer_extension: None,
                node_kind: None,
            }
        }
    }

    impl EditorPlugin for ExtPlugin {
        fn manifest(&self) -> PluginManifest {
            self.manifest.clone()
        }

        fn register(&mut self, context: &mut PluginContext) {
            context.inspector_section(InspectorSectionDescriptor::new(
                format!("{}.inspector", self.manifest.id),
                "插件检查器",
            ));
            context.inspector_section_draw(
                format!("{}.inspector", self.manifest.id),
                |ui, host| {
                    ui.label(format!("插件检查器：{:?}", host.snapshot().selected_entity));
                    host.submit(PluginAction::SetStatus("inspector-drawn".to_owned()));
                },
            );
            if let Some(extension) = self.importer_extension {
                context.asset_importer(ImporterDescriptor::new(
                    format!("{}.import", self.manifest.id),
                    vec![extension.to_owned()],
                ));
            }
            if let Some(kind) = self.node_kind {
                context.visual_script_node(VisualScriptNodeDescriptor::new(kind, "插件节点"));
            }
        }
    }

    #[test]
    fn inspector_importer_and_node_extensions_are_aggregated() {
        let mut registry = PluginRegistry::default();
        registry
            .register(Box::new(ExtPlugin {
                importer_extension: Some("rlstats"),
                node_kind: Some("plugin.ext.node"),
                ..ExtPlugin::new("ext")
            }))
            .unwrap();
        registry.load("ext").unwrap();
        assert_eq!(registry.inspector_sections().len(), 1);
        assert_eq!(
            registry.asset_importers()[0].1.extensions,
            ["rlstats".to_owned()]
        );
        assert_eq!(registry.visual_script_nodes()[0].1.kind, "plugin.ext.node");

        let mut host = TestHost::new(1);
        host.snapshot.selected_entity = Some(9);
        let context = egui::Context::default();
        let mut drawn = Ok(());
        let output = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                drawn = registry.draw_inspector_section("ext.inspector", ui, &mut host);
            });
        });
        drawn.unwrap();
        assert!(!output.shapes.is_empty());
        assert_eq!(
            host.actions,
            [PluginAction::SetStatus("inspector-drawn".to_owned())]
        );

        registry.set_enabled("ext", false).unwrap();
        assert!(registry.inspector_sections().is_empty());
        assert!(registry.asset_importers().is_empty());
        assert!(registry.visual_script_nodes().is_empty());
    }

    #[test]
    fn importer_extension_and_node_kind_conflicts_are_rejected() {
        let mut registry = PluginRegistry::default();
        registry
            .register(Box::new(ExtPlugin {
                importer_extension: Some("rlstats"),
                node_kind: Some("plugin.ext.node"),
                ..ExtPlugin::new("first")
            }))
            .unwrap();
        registry.load("first").unwrap();

        let mut duplicate_importer = ExtPlugin::new("second");
        duplicate_importer.importer_extension = Some("rlstats");
        registry.register(Box::new(duplicate_importer)).unwrap();
        assert!(matches!(
            registry.load("second").expect_err("扩展名撞车"),
            PluginError::Context { .. }
        ));

        let mut duplicate_node = ExtPlugin::new("third");
        duplicate_node.importer_extension = Some("other");
        duplicate_node.node_kind = Some("plugin.ext.node");
        registry.register(Box::new(duplicate_node)).unwrap();
        assert!(matches!(
            registry.load("third").expect_err("节点 kind 撞车"),
            PluginError::Context { .. }
        ));

        let mut bad_extension = ExtPlugin::new("fourth");
        bad_extension.importer_extension = Some(".Upper");
        registry.register(Box::new(bad_extension)).unwrap();
        assert!(matches!(
            registry.load("fourth").expect_err("扩展名必须小写无点"),
            PluginError::Context { .. }
        ));
    }

    #[test]
    fn conflicting_or_undeclared_extensions_are_rejected() {
        let mut registry = PluginRegistry::default();
        registry.register(Box::new(UiPlugin::new("ui"))).unwrap();
        registry.load("ui").unwrap();

        // 跨插件重名命令：第二个插件拒绝加载，且不污染状态。
        let mut duplicate = UiPlugin::new("other");
        duplicate.manifest.capabilities.insert(Capability::Command);
        registry.register(Box::new(duplicate)).unwrap();
        assert!(matches!(
            registry.load("other").expect_err("命令重名"),
            PluginError::Context { plugin, .. } if plugin == "other"
        ));
        assert!(!registry.is_loaded("other"));
        assert!(registry.is_loaded("ui"), "已加载插件不受影响");

        let mut missing_draw = UiPlugin::new("missing");
        missing_draw.register_missing_draw = true;
        let mut registry = PluginRegistry::default();
        registry.register(Box::new(missing_draw)).unwrap();
        assert!(matches!(
            registry.load("missing").expect_err("未声明的绘制回调"),
            PluginError::Context { .. }
        ));

        let mut bad_menu = UiPlugin::new("badmenu");
        bad_menu.menu_without_command = true;
        let mut registry = PluginRegistry::default();
        registry.register(Box::new(bad_menu)).unwrap();
        assert!(matches!(
            registry.load("badmenu").expect_err("菜单引用未知命令"),
            PluginError::Context { .. }
        ));
    }
}
