//! 菜单栏：**按 Unity 编辑器的顶级菜单结构组织**（文件 / 编辑 / 资源 / 游戏对象 / 组件 / 窗口 / 帮助）。
//!
//! 设计要点：
//!
//! 1. **结构是数据**（[`MENU_BAR`]）：顶级菜单、子菜单、条目、快捷键、可用条件都在一张
//!    声明表里，渲染只负责走这棵树。因此"菜单结构与 Unity 对齐"可以被**单测断言**，
//!    而不是靠肉眼看截图（`menu_bar_follows_unity_structure` 等）。
//! 2. **条目只发动作**（[`EditorApp::run_menu_action`]）：菜单不直接改文档，能走 typed
//!    command 的一律走 `dispatch`（撤销/重做/复制/删除/保存），其余走既有入口方法。
//! 3. **诚实列举**：Unity 有而 RustLua 暂时没有的能力（打开场景的文件选择器、
//!    Build Settings 窗口、Preferences）**不假装存在**——要么指向现有等价入口，
//!    要么标成 `MenuGate` 不可用并在悬停说明原因。
//!
//! 与 Unity 的对应关系（顶级菜单顺序一致）：
//!
//! | Unity | RustLua | 说明 |
//! |---|---|---|
//! | File | 文件 | 新建/打开项目、保存场景、项目设置、发布、退出；"New Scene" 带场景名输入行 |
//! | Edit | 编辑 | 撤销/重做、复制粘贴组件、复制/删除实体、全选/取消选择、重命名 |
//! | Assets | 资源 | 刷新资源、创建预制体、收集字体字符、在资源管理器中打开项目根 |
//! | GameObject | 游戏对象 | 空对象 / 2D 对象 / 3D 对象 / UI 子菜单 + 重命名/复制/删除 |
//! | Component | 组件 | 直接由 `COMPONENT_REGISTRY` 的类别分组生成（新增组件不用改菜单） |
//! | Window | 窗口 | 面板开关、场景视图开关、工作台与视图模式、布局保存/重置 |
//! | Help | 帮助 | 使用手册、版本说明 |
//!
//! RustLua 特有：崩溃恢复（`⚠ 恢复`）作为独立菜单，仅在检测到快照时出现。

use crate::EditorApp;
use crate::WorkspaceTab;
use editor::SelectionSource;
use eframe::egui;

/// 条目的可用条件（渲染成"灰置 + 悬停原因"）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MenuGate {
    /// 始终可用。
    Always,
    /// 需要已打开项目（`project_loaded`）。
    ProjectLoaded,
    /// 需要当前选中至少一个实体。
    HasSelection,
}

impl MenuGate {
    const fn reason(self) -> &'static str {
        match self {
            Self::Always => "",
            Self::ProjectLoaded => "需要先打开一个项目",
            Self::HasSelection => "需要先在层级 / 场景视图中选中实体",
        }
    }
}

/// 一个菜单条目：标签 + 动作 + 快捷键提示 + 可用条件。
///
/// `shortcut` 只用于**显示**（真实快捷键在 main.rs 的输入处理里）；两者的对应关系由
/// `menu_shortcuts_match_the_documented_ones` 盯住，避免菜单写着 Ctrl+S 而实际不生效。
#[derive(Clone, Copy, Debug)]
pub(crate) struct MenuItem {
    pub(crate) label: &'static str,
    pub(crate) action: MenuAction,
    pub(crate) shortcut: Option<&'static str>,
    pub(crate) gate: MenuGate,
}

impl MenuItem {
    const fn new(label: &'static str, action: MenuAction) -> Self {
        Self {
            label,
            action,
            shortcut: None,
            gate: MenuGate::Always,
        }
    }

    const fn shortcut(mut self, shortcut: &'static str) -> Self {
        self.shortcut = Some(shortcut);
        self
    }

    const fn gate(mut self, gate: MenuGate) -> Self {
        self.gate = gate;
        self
    }
}

/// 菜单树的一行：条目 / 分隔线 / 子菜单 / 需要运行时数据的分组。
#[derive(Clone, Copy, Debug)]
pub(crate) enum MenuRow {
    Item(MenuItem),
    Separator,
    SubMenu {
        label: &'static str,
        rows: &'static [MenuRow],
    },
    /// 文件 ▸ 最近项目（运行时列表，最多 5 条）。
    RecentProjects,
    /// 组件：由 `COMPONENT_REGISTRY` 的类别分组生成。
    ComponentRegistry,
    /// 窗口 ▸ 工作台：场景 / 游戏（互斥选择）。
    WorkspaceTabs,
    /// 窗口 ▸ 面板：六个工作台面板逐项开关（复选框，状态来自 `show_*`）。
    PanelToggles,
    /// 窗口 ▸ 视图模式：2D / 3D / Widget / Prefab（互斥选择）。
    ViewModes,
    /// 窗口 ▸ 布局与面板宽度提示（只读信息行）。
    LayoutInfo,
}

/// 菜单条目的动作。渲染层不写文档，只把它交给 [`EditorApp::run_menu_action`]。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MenuAction {
    // 文件
    NewScene,
    NewProject,
    OpenProject,
    SaveScene,
    /// docs/55 阶段 3：明知磁盘被外部改动也覆盖（显式动作，不绑快捷键）。
    ForceSaveScene,
    ProjectSettings,
    PublishProject,
    Quit,
    // 编辑
    Undo,
    Redo,
    DuplicateSelection,
    DeleteSelection,
    CopyComponents,
    PasteComponents,
    SelectAll,
    DeselectAll,
    RenameEntity,
    // 编辑 ▸ 运行控制（与工具栏同一批 typed 命令；Unity 把它们放在 Edit 菜单）
    PlayStart,
    PlayStop,
    PlayTogglePause,
    PlayStep,
    // 资源
    RefreshResources,
    CreatePrefab,
    CollectFontChars,
    /// 资源 ▸ 图集 ▸ 配置与预览：`atlas.json` 编辑 + 打页预览（不写产物）。
    OpenAtlasPanel,
    /// 资源 ▸ 分包 ▸ 配置与预览：`bundles.json` 编辑 + 落盘字节预估（不产容器）。
    OpenBundlePanel,
    RevealProjectRoot,
    // 游戏对象
    CreateEmptyEntity,
    CreateSprite,
    CreateTilemap,
    Create3dEntity,
    CreateUiText,
    CreateUiImage,
    // 窗口
    ToggleResources,
    ToggleHierarchy,
    ToggleInspector,
    /// M75-03：内置 AI 助手标签（与属性同叶）。
    ToggleAssistant,
    ToggleSceneView,
    ToggleGameView,
    ToggleConsole,
    /// P3-E01-3：`.anim` 时间线面板（与控制台同叶）。
    ToggleTimeline,
    /// P3-E03-2：可视化脚本画布（默认隐藏，双击 `.vgraph` 打开）。
    ToggleGraph,
    /// M76-04：UI Studio（默认隐藏）。
    ToggleUiStudio,
    ToggleDomainData,
    TogglePublishPanel,
    TogglePerformancePanel,
    /// P3-E04-4：插件诊断窗口（列表/错误/启停/重载）。
    TogglePluginDiagnostics,
    TogglePixelGrid,
    ToggleSceneGuides,
    ToggleSafeArea,
    ToggleCollisionOverlay,
    ToggleSnap,
    SaveLayout,
    ResetLayout,
    // 帮助
    OpenManual,
    About,
}

const FILE_MENU: &[MenuRow] = &[
    // Unity 的 File ▸ New Scene 是**直接创建**（Untitled），菜单里没有名字输入框；
    // 场景名由 `create_scene` 自动取（`main`，冲突时 `main2`…）。
    MenuRow::Item(MenuItem::new("新建场景", MenuAction::NewScene).gate(MenuGate::ProjectLoaded)),
    MenuRow::Separator,
    MenuRow::Item(MenuItem::new("新建项目…", MenuAction::NewProject)),
    MenuRow::Item(MenuItem::new("打开项目…", MenuAction::OpenProject)),
    MenuRow::RecentProjects,
    MenuRow::Separator,
    MenuRow::Item(
        MenuItem::new("保存场景", MenuAction::SaveScene)
            .shortcut("Ctrl+S")
            .gate(MenuGate::ProjectLoaded),
    ),
    // docs/55 阶段 3：磁盘文件被外部改动时普通保存会被**拒绝**（不静默覆盖别人的改动），
    // 这里给出显式出口；没有冲突时它只是"再保存一次"，不会造成意外。
    MenuRow::Item(
        MenuItem::new("强制保存（覆盖外部修改）", MenuAction::ForceSaveScene)
            .gate(MenuGate::ProjectLoaded),
    ),
    MenuRow::Separator,
    MenuRow::Item(
        MenuItem::new("项目设置…", MenuAction::ProjectSettings).gate(MenuGate::ProjectLoaded),
    ),
    MenuRow::Item(MenuItem::new("发布项目…", MenuAction::PublishProject)),
    MenuRow::Separator,
    MenuRow::Item(MenuItem::new("退出", MenuAction::Quit)),
];

const EDIT_MENU: &[MenuRow] = &[
    MenuRow::Item(
        MenuItem::new("撤销", MenuAction::Undo)
            .shortcut("Ctrl+Z")
            .gate(MenuGate::ProjectLoaded),
    ),
    MenuRow::Item(
        MenuItem::new("重做", MenuAction::Redo)
            .shortcut("Ctrl+Y")
            .gate(MenuGate::ProjectLoaded),
    ),
    MenuRow::Separator,
    MenuRow::Item(
        MenuItem::new("复制组件", MenuAction::CopyComponents).gate(MenuGate::HasSelection),
    ),
    MenuRow::Item(
        MenuItem::new("粘贴组件", MenuAction::PasteComponents).gate(MenuGate::ProjectLoaded),
    ),
    MenuRow::Separator,
    MenuRow::Item(
        MenuItem::new("复制实体", MenuAction::DuplicateSelection)
            .shortcut("Ctrl+D")
            .gate(MenuGate::HasSelection),
    ),
    MenuRow::Item(
        MenuItem::new("删除实体", MenuAction::DeleteSelection)
            .shortcut("Delete")
            .gate(MenuGate::HasSelection),
    ),
    MenuRow::Item(
        MenuItem::new("重命名实体", MenuAction::RenameEntity).gate(MenuGate::HasSelection),
    ),
    MenuRow::Separator,
    MenuRow::Item(
        MenuItem::new("全选", MenuAction::SelectAll)
            .shortcut("Ctrl+A")
            .gate(MenuGate::ProjectLoaded),
    ),
    MenuRow::Item(MenuItem::new("取消选择", MenuAction::DeselectAll).gate(MenuGate::HasSelection)),
    // Unity 把 Play / Pause / Step 放在 Edit 菜单里（工具栏只是入口之一）。
    MenuRow::Separator,
    MenuRow::Item(MenuItem::new("播放", MenuAction::PlayStart).gate(MenuGate::ProjectLoaded)),
    MenuRow::Item(MenuItem::new("停止", MenuAction::PlayStop).gate(MenuGate::ProjectLoaded)),
    MenuRow::Item(
        MenuItem::new("暂停 / 继续", MenuAction::PlayTogglePause).gate(MenuGate::ProjectLoaded),
    ),
    MenuRow::Item(MenuItem::new("单帧步进", MenuAction::PlayStep).gate(MenuGate::ProjectLoaded)),
];

const ASSETS_MENU: &[MenuRow] = &[
    MenuRow::Item(
        MenuItem::new("刷新资源", MenuAction::RefreshResources).gate(MenuGate::ProjectLoaded),
    ),
    MenuRow::Separator,
    MenuRow::Item(
        MenuItem::new("创建预制体（从选中实体）", MenuAction::CreatePrefab)
            .gate(MenuGate::HasSelection),
    ),
    MenuRow::Item(
        MenuItem::new("收集字体字符", MenuAction::CollectFontChars).gate(MenuGate::ProjectLoaded),
    ),
    MenuRow::SubMenu {
        label: "图集",
        rows: &[MenuRow::Item(
            MenuItem::new("配置与预览…", MenuAction::OpenAtlasPanel).gate(MenuGate::ProjectLoaded),
        )],
    },
    MenuRow::SubMenu {
        label: "分包",
        rows: &[MenuRow::Item(
            MenuItem::new("分包配置与预览…", MenuAction::OpenBundlePanel)
                .gate(MenuGate::ProjectLoaded),
        )],
    },
    MenuRow::Separator,
    MenuRow::Item(
        MenuItem::new("在资源管理器中打开项目根", MenuAction::RevealProjectRoot)
            .gate(MenuGate::ProjectLoaded),
    ),
];

const GAMEOBJECT_MENU: &[MenuRow] = &[
    MenuRow::Item(MenuItem::new("创建空对象", MenuAction::CreateEmptyEntity)),
    MenuRow::Separator,
    MenuRow::SubMenu {
        label: "2D 对象",
        rows: &[
            MenuRow::Item(MenuItem::new("精灵", MenuAction::CreateSprite)),
            MenuRow::Item(MenuItem::new("瓦片地图", MenuAction::CreateTilemap)),
        ],
    },
    MenuRow::SubMenu {
        label: "3D 对象",
        rows: &[MenuRow::Item(MenuItem::new(
            "空 3D 实体",
            MenuAction::Create3dEntity,
        ))],
    },
    MenuRow::SubMenu {
        label: "界面",
        rows: &[
            MenuRow::Item(MenuItem::new("文本", MenuAction::CreateUiText)),
            MenuRow::Item(MenuItem::new("图片", MenuAction::CreateUiImage)),
        ],
    },
    MenuRow::Separator,
    MenuRow::Item(
        MenuItem::new("复制实体", MenuAction::DuplicateSelection).gate(MenuGate::HasSelection),
    ),
    MenuRow::Item(
        MenuItem::new("删除实体", MenuAction::DeleteSelection).gate(MenuGate::HasSelection),
    ),
];

const COMPONENT_MENU: &[MenuRow] = &[MenuRow::ComponentRegistry];

const WINDOW_MENU: &[MenuRow] = &[
    MenuRow::PanelToggles,
    MenuRow::Separator,
    MenuRow::Item(MenuItem::new("领域数据", MenuAction::ToggleDomainData)),
    MenuRow::Item(MenuItem::new("发布面板", MenuAction::TogglePublishPanel)),
    MenuRow::Item(MenuItem::new(
        "性能分析面板",
        MenuAction::TogglePerformancePanel,
    )),
    MenuRow::Item(MenuItem::new(
        "插件诊断",
        MenuAction::TogglePluginDiagnostics,
    )),
    MenuRow::Separator,
    MenuRow::WorkspaceTabs,
    MenuRow::ViewModes,
    MenuRow::Separator,
    MenuRow::Item(MenuItem::new("像素网格", MenuAction::TogglePixelGrid)),
    MenuRow::Item(MenuItem::new("场景参考线", MenuAction::ToggleSceneGuides)),
    MenuRow::Item(MenuItem::new("安全区", MenuAction::ToggleSafeArea)),
    MenuRow::Item(MenuItem::new(
        "碰撞区域(调试)",
        MenuAction::ToggleCollisionOverlay,
    )),
    MenuRow::Item(MenuItem::new("网格吸附", MenuAction::ToggleSnap)),
    MenuRow::Separator,
    MenuRow::Item(MenuItem::new("保存布局", MenuAction::SaveLayout).shortcut("Ctrl+Shift+L")),
    MenuRow::Item(MenuItem::new("重置为默认布局", MenuAction::ResetLayout)),
    MenuRow::LayoutInfo,
];

const HELP_MENU: &[MenuRow] = &[
    MenuRow::Item(MenuItem::new("使用手册", MenuAction::OpenManual)),
    MenuRow::Item(MenuItem::new("版本说明", MenuAction::About)),
];

/// 顶级菜单表：顺序与 Unity 一致（文件 / 编辑 / 资源 / 游戏对象 / 组件 / 窗口 / 帮助）。
pub(crate) const MENU_BAR: &[(&str, &[MenuRow])] = &[
    ("文件", FILE_MENU),
    ("编辑", EDIT_MENU),
    ("资源", ASSETS_MENU),
    ("游戏对象", GAMEOBJECT_MENU),
    ("组件", COMPONENT_MENU),
    ("窗口", WINDOW_MENU),
    ("帮助", HELP_MENU),
];

impl EditorApp {
    /// 渲染整条菜单栏（由 `draw_header_panel` 调用）。
    pub(crate) fn draw_menu_bar(&mut self, ui: &mut egui::Ui) {
        for (title, rows) in MENU_BAR {
            ui.menu_button(*title, |ui| {
                self.draw_menu_rows(ui, rows);
            });
        }
        // E04-2：已加载插件声明的菜单条目（禁用/卸载后立即消失）。
        self.draw_plugin_menu(ui);
        // 崩溃恢复：RustLua 特有，检测到快照时才出现（Unity 没有对应菜单）。
        if !self.recovery_scan.candidates.is_empty() {
            self.draw_recovery_menu(ui);
        }
    }

    fn draw_menu_rows(&mut self, ui: &mut egui::Ui, rows: &[MenuRow]) {
        for row in rows {
            match row {
                MenuRow::Separator => {
                    ui.separator();
                }
                MenuRow::Item(item) => {
                    let enabled = self.menu_gate_open(item.gate);
                    let label = match item.shortcut {
                        Some(shortcut) => format!("{}    {shortcut}", item.label),
                        None => item.label.to_owned(),
                    };
                    let response = ui.add_enabled(enabled, egui::Button::new(label));
                    let response = if enabled {
                        response
                    } else {
                        response.on_disabled_hover_text(item.gate.reason())
                    };
                    if response.clicked() {
                        self.run_menu_action(item.action);
                        ui.close();
                    }
                }
                MenuRow::SubMenu { label, rows } => {
                    ui.menu_button(*label, |ui| self.draw_menu_rows(ui, rows));
                }
                MenuRow::RecentProjects => {
                    ui.menu_button("最近项目", |ui| {
                        if self.recent_projects.is_empty() {
                            ui.label("（暂无最近项目）");
                        } else {
                            for path in self.recent_projects.clone() {
                                if ui
                                    .button(crate::util::display_path(std::path::Path::new(&path)))
                                    .clicked()
                                {
                                    self.open_recent_project(path);
                                    ui.close();
                                }
                            }
                        }
                    });
                }
                MenuRow::ComponentRegistry => {
                    for (category, names) in crate::panels::inspector::component_menu_groups() {
                        ui.menu_button(category, |ui| {
                            for name in names {
                                if ui
                                    .button(crate::util::component_display_name(name))
                                    .clicked()
                                {
                                    if let Err(error) = self.add_component(name) {
                                        self.status = error;
                                    }
                                    ui.close();
                                }
                            }
                        });
                    }
                }
                MenuRow::WorkspaceTabs => {
                    ui.label("工作台");
                    for (label, tab) in
                        [("场景", WorkspaceTab::Scene), ("游戏", WorkspaceTab::Game)]
                    {
                        if ui
                            .selectable_label(self.workspace_tab == tab, label)
                            .clicked()
                        {
                            self.focus_workspace_tab(tab);
                            ui.close();
                        }
                    }
                }
                MenuRow::PanelToggles => {
                    ui.label("面板");
                    // 勾选状态只读自 `show_*`，写入一律回到 [`EditorApp::run_menu_action`]：
                    // 菜单不是第二套状态来源。点完**不关菜单**，一次打开可以连勾多个面板。
                    for (label, action) in [
                        ("资源浏览器", MenuAction::ToggleResources),
                        ("层级", MenuAction::ToggleHierarchy),
                        ("场景", MenuAction::ToggleSceneView),
                        ("游戏", MenuAction::ToggleGameView),
                        ("属性", MenuAction::ToggleInspector),
                        ("AI 助手", MenuAction::ToggleAssistant),
                        ("控制台", MenuAction::ToggleConsole),
                        ("时间线", MenuAction::ToggleTimeline),
                        ("可视化脚本", MenuAction::ToggleGraph),
                        ("UI 工作室", MenuAction::ToggleUiStudio),
                    ] {
                        let mut shown = self.panel_is_shown(action);
                        if ui.checkbox(&mut shown, label).clicked() {
                            self.run_menu_action(action);
                        }
                    }
                }
                MenuRow::ViewModes => {
                    ui.label("视图模式");
                    for (label, mode) in [
                        ("2D", crate::SceneViewMode::View2D),
                        ("3D", crate::SceneViewMode::View3D),
                        ("控件", crate::SceneViewMode::Widget),
                        ("预制体", crate::SceneViewMode::Prefab),
                    ] {
                        if ui
                            .selectable_label(self.scene_view_mode == mode, label)
                            .clicked()
                        {
                            self.scene_view_mode = mode;
                            // docs/55 阶段 2：换模式后重新校验工具能力（见 `scene_tools`）。
                            self.revalidate_scene_tool_for_view_mode();
                            ui.close();
                        }
                    }
                }
                MenuRow::LayoutInfo => {
                    ui.separator();
                    if let Some(reason) = self.layout_load.reason() {
                        ui.colored_label(
                            egui::Color32::from_rgb(220, 140, 80),
                            format!("上次布局加载：{reason}"),
                        );
                    } else {
                        ui.weak("布局随编辑自动保存（面板宽度/可见性/视图状态）");
                    }
                }
            }
        }
    }

    /// 崩溃恢复菜单（原实现不变，只是搬进本模块的渲染流程）。
    fn draw_recovery_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("⚠ 恢复", |ui| {
            ui.label("检测到崩溃恢复快照：");
            ui.separator();
            for candidate in self.recovery_scan.candidates.clone() {
                let label = candidate.original_path.as_ref().map_or_else(
                    || crate::util::display_path(&candidate.path),
                    |path| crate::util::display_path(path),
                );
                ui.horizontal(|ui| {
                    ui.label(&label);
                    if ui.button("恢复").clicked() {
                        self.recover_candidate(candidate.clone());
                        ui.close();
                    }
                    if ui.button("丢弃").clicked() {
                        let _ = self.recovery.discard(&candidate);
                        self.refresh_recovery_scan();
                        ui.close();
                    }
                });
            }
            ui.separator();
            if ui
                .button(format!(
                    "全部丢弃（{} 项）",
                    self.recovery_scan.candidates.len()
                ))
                .clicked()
            {
                match self.recovery.clear_all() {
                    Ok(removed) => self.status = format!("已清理 {removed} 个恢复快照"),
                    Err(error) => self.status = format!("清理恢复快照失败：{error}"),
                }
                self.refresh_recovery_scan();
                ui.close();
            }
        });
    }

    /// 面板开关的当前状态（[`MenuRow::PanelToggles`] 只读这里，写入走 [`Self::run_menu_action`]）。
    /// 只有六个工作台面板有状态；浮层窗口（领域数据、发布、性能）不画成复选框。
    #[must_use]
    pub(crate) fn panel_is_shown(&self, action: MenuAction) -> bool {
        match action {
            MenuAction::ToggleResources => self.show_resources_panel,
            MenuAction::ToggleHierarchy => self.show_hierarchy_panel,
            MenuAction::ToggleInspector => self.show_inspector_panel,
            MenuAction::ToggleAssistant => self.show_assistant_panel,
            MenuAction::ToggleSceneView => self.show_scene_view,
            MenuAction::ToggleGameView => self.show_game_view,
            MenuAction::ToggleConsole => self.show_lua_console,
            MenuAction::ToggleTimeline => self.show_timeline_panel,
            MenuAction::ToggleGraph => self.show_graph_panel,
            MenuAction::ToggleUiStudio => self.show_ui_studio,
            _ => false,
        }
    }

    /// 条目的可用条件（菜单只读状态，不改状态）。
    pub(crate) fn menu_gate_open(&self, gate: MenuGate) -> bool {
        match gate {
            MenuGate::Always => true,
            MenuGate::ProjectLoaded => self.project_loaded,
            MenuGate::HasSelection => self.selection.primary().is_some(),
        }
    }

    /// 执行一个菜单动作。能走 typed command 的一律走 `dispatch`（提交边界在 shell）。
    pub(crate) fn run_menu_action(&mut self, action: MenuAction) {
        match action {
            // ---- 文件 ----
            MenuAction::NewScene => self.create_scene(),
            MenuAction::NewProject => {
                self.create_new_project(crate::panels::header::ProjectTemplate::Blank)
            }
            MenuAction::OpenProject => self.open_project_menu(),
            MenuAction::SaveScene => {
                self.dispatch(editor::EditorCommand::Save);
            }
            MenuAction::ForceSaveScene => {
                self.save_scene_force();
            }
            MenuAction::ProjectSettings => self.open_project_settings(),
            MenuAction::PublishProject => self.publish.show_panel = true,
            MenuAction::Quit => {
                self.request_document_change(crate::actions::PendingChange::QuitEditor)
            }
            // ---- 编辑 ----
            MenuAction::Undo => {
                self.dispatch(editor::EditorCommand::Undo);
            }
            MenuAction::Redo => {
                self.dispatch(editor::EditorCommand::Redo);
            }
            MenuAction::DuplicateSelection => {
                self.dispatch(editor::EditorCommand::DuplicateSelection);
            }
            MenuAction::DeleteSelection => {
                self.dispatch(editor::EditorCommand::DeleteSelection);
            }
            MenuAction::CopyComponents => self.copy_selected_components(),
            MenuAction::PasteComponents => self.paste_selected_components(),
            MenuAction::SelectAll => self.select_all_entities(),
            MenuAction::DeselectAll => self.selection.clear(SelectionSource::Command),
            MenuAction::RenameEntity => self.rename_selected(),
            // 运行控制：与工具栏同一批 typed 命令（状态机在 `PlayController` 里）。
            MenuAction::PlayStart => {
                self.dispatch(editor::EditorCommand::StartPlay);
            }
            MenuAction::PlayStop => {
                self.dispatch(editor::EditorCommand::StopPlay);
            }
            MenuAction::PlayTogglePause => {
                if matches!(self.play_state(), editor::PlayState::Paused) {
                    self.dispatch(editor::EditorCommand::ResumePlay);
                } else {
                    self.dispatch(editor::EditorCommand::PausePlay);
                }
            }
            MenuAction::PlayStep => {
                self.dispatch(editor::EditorCommand::StepPlay { frames: 1 });
            }
            // ---- 资源 ----
            MenuAction::RefreshResources => {
                // 菜单刷新是**用户显式**动作：清空"已尝试"记录，允许重新排队导入。
                self.resources.clear_import_attempts();
                self.refresh_resources();
            }
            MenuAction::CreatePrefab => self.create_prefab_from_selected(),
            MenuAction::CollectFontChars => self.collect_font_chars(),
            MenuAction::OpenAtlasPanel => self.open_atlas_panel(),
            MenuAction::OpenBundlePanel => self.open_bundle_panel(),
            MenuAction::RevealProjectRoot => {
                crate::util::open_in_explorer(&self.project.root);
            }
            // ---- 游戏对象 ----
            // 菜单里没有名字输入框：新建名字取默认值（与层级「+」菜单同口径），
            // 想改名走层级 F2 / Inspector 的名称缓冲。
            MenuAction::CreateEmptyEntity => {
                self.entity_name = "新实体".to_owned();
                self.add_entity(None);
            }
            MenuAction::CreateSprite => self.add_entity_with_component("精灵", "Sprite"),
            MenuAction::CreateTilemap => self.add_entity_with_component("瓦片地图", "Tilemap"),
            MenuAction::Create3dEntity => {
                self.entity_name = "3D 实体".to_owned();
                self.add_3d_entity(None);
            }
            MenuAction::CreateUiText => self.add_entity_with_component("文本", "Text"),
            MenuAction::CreateUiImage => self.add_entity_with_component("图片", "Image"),
            // ---- 窗口（面板开关） ----
            MenuAction::ToggleResources => self.show_resources_panel = !self.show_resources_panel,
            MenuAction::ToggleHierarchy => self.show_hierarchy_panel = !self.show_hierarchy_panel,
            MenuAction::ToggleInspector => self.show_inspector_panel = !self.show_inspector_panel,
            MenuAction::ToggleAssistant => self.show_assistant_panel = !self.show_assistant_panel,
            MenuAction::ToggleConsole => self.show_lua_console = !self.show_lua_console,
            MenuAction::ToggleTimeline => self.show_timeline_panel = !self.show_timeline_panel,
            MenuAction::ToggleGraph => self.show_graph_panel = !self.show_graph_panel,
            MenuAction::ToggleUiStudio => self.show_ui_studio = !self.show_ui_studio,
            // 中央两个视图共用一条守卫（最后一个不许关）。
            MenuAction::ToggleSceneView => {
                let visible = !self.show_scene_view;
                self.set_central_view_visible(WorkspaceTab::Scene, visible);
            }
            MenuAction::ToggleGameView => {
                let visible = !self.show_game_view;
                self.set_central_view_visible(WorkspaceTab::Game, visible);
            }
            MenuAction::ToggleDomainData => {
                self.show_domain_data_panel = !self.show_domain_data_panel;
            }
            MenuAction::TogglePublishPanel => self.publish.show_panel = !self.publish.show_panel,
            MenuAction::TogglePerformancePanel => {
                self.show_performance_panel = !self.show_performance_panel;
            }
            MenuAction::TogglePluginDiagnostics => {
                self.show_plugin_diagnostics = !self.show_plugin_diagnostics;
            }
            MenuAction::TogglePixelGrid => self.show_pixel_grid = !self.show_pixel_grid,
            MenuAction::ToggleSceneGuides => self.show_scene_guides = !self.show_scene_guides,
            MenuAction::ToggleSafeArea => self.show_safe_area = !self.show_safe_area,
            MenuAction::ToggleCollisionOverlay => {
                self.show_collision_overlay = !self.show_collision_overlay;
            }
            MenuAction::ToggleSnap => self.snap_enabled = !self.snap_enabled,
            MenuAction::SaveLayout => self.persist_layout(),
            MenuAction::ResetLayout => self.reset_layout_to_default(),
            // ---- 帮助 ----
            MenuAction::OpenManual => self.open_help(),
            MenuAction::About => self.show_about = true,
        }
    }

    /// 全选当前场景实体（选择是单一事实源，因此这也是一条命令语义的操作）。
    fn select_all_entities(&mut self) {
        let Some(document) = self.current_scene() else {
            return;
        };
        let ids: Vec<_> = document.scene.entities.keys().copied().collect();
        if ids.is_empty() {
            self.status = "当前场景没有实体".to_owned();
            return;
        }
        let count = ids.len();
        self.selection.set_many(ids, None, SelectionSource::Command);
        self.status = format!("已选中 {count} 个实体");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn walk(rows: &[MenuRow], visit: &mut impl FnMut(&MenuRow)) {
        for row in rows {
            visit(row);
            if let MenuRow::SubMenu { rows, .. } = row {
                walk(rows, visit);
            }
        }
    }

    /// 顶级菜单必须与 Unity 一致（顺序也一致）：文件 / 编辑 / 资源 / 游戏对象 / 组件 / 窗口 / 帮助。
    #[test]
    fn menu_bar_follows_unity_structure() {
        let titles: Vec<&str> = MENU_BAR.iter().map(|(title, _)| *title).collect();
        assert_eq!(
            titles,
            vec!["文件", "编辑", "资源", "游戏对象", "组件", "窗口", "帮助"],
            "顶级菜单顺序必须与 Unity 对齐"
        );
    }

    /// 「新建场景」必须是**一条菜单项**、不要名字输入框（与 Unity 的 File ▸ New Scene 一致）：
    /// 场景名由 `create_scene` 自动取并在冲突时去重。
    #[test]
    fn new_scene_is_a_plain_menu_item() {
        let (title, rows) = MENU_BAR[0];
        assert_eq!(title, "文件");
        match rows.first() {
            Some(MenuRow::Item(item)) => {
                assert_eq!(item.label, "新建场景");
                assert_eq!(item.action, MenuAction::NewScene);
                assert_eq!(item.gate, MenuGate::ProjectLoaded);
            }
            other => panic!("文件菜单第一项必须是「新建场景」条目，实际 {other:?}"),
        }
        // 整棵菜单树里都不该再有场景名输入行。
        for (_, rows) in MENU_BAR {
            walk(rows, &mut |row| {
                if matches!(row, MenuRow::SubMenu { .. }) {
                    return;
                }
                assert!(
                    !matches!(row, MenuRow::Item(item) if item.label.contains("场景名")),
                    "菜单里不该出现场景名输入行"
                );
            });
        }
    }

    /// 「创建空对象」必须是**一条菜单项**、不要实体名输入行（与层级「+」菜单同口径）：
    /// 名字由创建入口取默认值，改名走层级 F2 / Inspector 的名称缓冲。
    #[test]
    fn create_empty_object_is_a_plain_menu_item() {
        let (title, rows) = MENU_BAR
            .iter()
            .find(|(title, _)| *title == "游戏对象")
            .copied()
            .expect("缺少游戏对象菜单");
        assert_eq!(title, "游戏对象");
        match rows.first() {
            Some(MenuRow::Item(item)) => {
                assert_eq!(item.label, "创建空对象");
                assert_eq!(item.action, MenuAction::CreateEmptyEntity);
            }
            other => panic!("游戏对象菜单第一项必须是「创建空对象」条目，实际 {other:?}"),
        }
        // 整棵菜单树里都不该再有实体名输入行。
        for (_, rows) in MENU_BAR {
            walk(rows, &mut |row| {
                assert!(
                    !matches!(row, MenuRow::Item(item) if item.label.contains("实体名")),
                    "菜单里不该出现实体名输入行"
                );
            });
        }
    }

    /// 结构卫生：每个菜单非空、不以分隔线开头/结尾、没有连续分隔线、标签唯一。
    #[test]
    fn menu_rows_are_well_formed() {
        for (title, rows) in MENU_BAR {
            assert!(!rows.is_empty(), "{title} 菜单不能为空");
            assert!(
                !matches!(rows.first(), Some(MenuRow::Separator)),
                "{title} 菜单不能以分隔线开头"
            );
            assert!(
                !matches!(rows.last(), Some(MenuRow::Separator)),
                "{title} 菜单不能以分隔线结尾"
            );
            let mut previous_separator = false;
            let mut labels = BTreeSet::new();
            walk(rows, &mut |row| match row {
                MenuRow::Separator => {
                    assert!(!previous_separator, "{title} 菜单里出现了连续分隔线");
                    previous_separator = true;
                }
                MenuRow::Item(item) => {
                    previous_separator = false;
                    assert!(!item.label.trim().is_empty(), "{title} 有空白条目");
                    assert!(
                        labels.insert(item.label),
                        "{title} 菜单里标签重复：{}",
                        item.label
                    );
                    if item.gate != MenuGate::Always {
                        assert!(
                            !item.gate.reason().is_empty(),
                            "{} 的不可用条件必须给出原因",
                            item.label
                        );
                    }
                }
                MenuRow::SubMenu { label, rows } => {
                    previous_separator = false;
                    assert!(!label.trim().is_empty(), "{title} 有空子菜单");
                    assert!(!rows.is_empty(), "{title} ▸ {label} 子菜单不能为空");
                }
                _ => previous_separator = false,
            });
        }
    }

    /// 菜单里显示的快捷键必须与文档/实际键位一致（避免"菜单写着 Ctrl+S 但不生效"）。
    #[test]
    fn menu_shortcuts_match_the_documented_ones() {
        let mut shortcuts = Vec::new();
        for (_, rows) in MENU_BAR {
            walk(rows, &mut |row| {
                if let MenuRow::Item(item) = row
                    && let Some(shortcut) = item.shortcut
                {
                    shortcuts.push((item.label, shortcut));
                }
            });
        }
        let expected = [
            ("保存场景", "Ctrl+S"),
            ("撤销", "Ctrl+Z"),
            ("重做", "Ctrl+Y"),
            ("复制实体", "Ctrl+D"),
            ("删除实体", "Delete"),
            ("全选", "Ctrl+A"),
            ("保存布局", "Ctrl+Shift+L"),
        ];
        assert_eq!(shortcuts, expected.to_vec());
    }

    /// 每个动作都必须能被执行到：逐个调用不 panic（用无项目/无选中的 App 跑一遍）。
    ///
    /// 跳过两类**会阻塞或需要真实环境**的动作：
    ///
    /// - **运行控制**（播放/停止/暂停/步进）：会真的尝试创建 GPU 预览会话，无窗口环境下要等到
    ///   初始化超时，单测会从毫秒级变成几十秒；这四条由
    ///   `command_router::play_control_commands_follow_the_state_machine` 等专门测试覆盖；
    /// - **新建/打开项目**：会弹出**原生目录选择对话框**（`choose_project_directory`），
    ///   单元测试不该弹窗。
    ///
    /// 其余条目（约 35 个）在这条测试里必须"无项目、无选中"也能安全执行。
    #[test]
    fn every_menu_action_is_executable_without_panicking() {
        let mut app = menu_test_app();
        let mut skipped = 0;
        for (_, rows) in MENU_BAR {
            walk(rows, &mut |row| {
                if let MenuRow::Item(item) = row {
                    if matches!(
                        item.action,
                        MenuAction::PlayStart
                            | MenuAction::PlayStop
                            | MenuAction::PlayTogglePause
                            | MenuAction::PlayStep
                            | MenuAction::NewProject
                            | MenuAction::OpenProject
                    ) {
                        skipped += 1;
                        return;
                    }
                    app.run_menu_action(item.action);
                }
                // 面板开关是复选框行（不是 `Item`），九个动作只在这里可达。
                if matches!(row, MenuRow::PanelToggles) {
                    for action in [
                        MenuAction::ToggleResources,
                        MenuAction::ToggleHierarchy,
                        MenuAction::ToggleSceneView,
                        MenuAction::ToggleGameView,
                        MenuAction::ToggleInspector,
                        MenuAction::ToggleAssistant,
                        MenuAction::ToggleConsole,
                        MenuAction::ToggleTimeline,
                        MenuAction::ToggleGraph,
                        MenuAction::ToggleUiStudio,
                    ] {
                        app.run_menu_action(action);
                    }
                }
            });
        }
        assert_eq!(skipped, 6, "被跳过的动作数量变了，请同步本测试说明");
        // 无项目时"需要项目"的条目全部灰置，但动作本身也必须安全可执行。
        assert!(!app.project_loaded);
    }

    /// 窗口菜单的面板开关是真的切换状态（不是摆设）。
    #[test]
    fn window_menu_toggles_panels() {
        let mut app = menu_test_app();
        for action in [
            MenuAction::ToggleResources,
            MenuAction::ToggleHierarchy,
            MenuAction::ToggleInspector,
            MenuAction::ToggleConsole,
            MenuAction::ToggleAssistant,
            MenuAction::ToggleTimeline,
            MenuAction::ToggleGraph,
            MenuAction::ToggleUiStudio,
        ] {
            let before = app.panel_is_shown(action);
            app.run_menu_action(action);
            assert_ne!(app.panel_is_shown(action), before, "{action:?} 没有生效");
            app.run_menu_action(action);
            assert_eq!(app.panel_is_shown(action), before, "{action:?} 没有还原");
        }

        let grid = app.show_pixel_grid;
        app.run_menu_action(MenuAction::TogglePixelGrid);
        assert_ne!(app.show_pixel_grid, grid);

        let collision = app.show_collision_overlay;
        app.run_menu_action(MenuAction::ToggleCollisionOverlay);
        assert_ne!(app.show_collision_overlay, collision);
    }

    /// 中央视图的守卫：两个不能同时关掉，关到最后一个时只留提示、不改状态。
    #[test]
    fn the_last_central_view_cannot_be_hidden() {
        let mut app = menu_test_app();
        app.run_menu_action(MenuAction::ToggleSceneView);
        assert!(!app.show_scene_view);
        assert!(app.show_game_view, "只关了一个，另一个不受影响");

        app.run_menu_action(MenuAction::ToggleGameView);
        assert!(app.show_game_view, "最后一个中央视图必须拒绝关闭");
        assert!(!app.show_scene_view, "拒绝时不能顺手把另一个打开");
        assert!(
            app.status.contains("中央视图"),
            "拒绝要在状态栏说明：{}",
            app.status
        );

        app.run_menu_action(MenuAction::ToggleSceneView);
        assert!(app.show_scene_view && app.show_game_view);
    }

    /// 关掉当前聚焦的中央视图时，聚焦要跟着搬到还亮着的那个（否则工具栏高亮画在不在的标签上）。
    #[test]
    fn hiding_the_focused_central_view_moves_focus() {
        let mut app = menu_test_app();
        app.focus_workspace_tab(WorkspaceTab::Scene);
        app.run_menu_action(MenuAction::ToggleSceneView);
        assert_eq!(app.workspace_tab, WorkspaceTab::Game);
        // 反过来：Game 聚焦时关 Game。
        app.run_menu_action(MenuAction::ToggleSceneView);
        app.focus_workspace_tab(WorkspaceTab::Game);
        app.run_menu_action(MenuAction::ToggleGameView);
        assert_eq!(app.workspace_tab, WorkspaceTab::Scene);
    }

    /// 需要选中实体的条目在无选中时被拦下（灰置），有选中时放行。
    #[test]
    fn gates_reflect_project_and_selection_state() {
        let app = menu_test_app();
        assert!(app.menu_gate_open(MenuGate::Always));
        assert!(!app.menu_gate_open(MenuGate::ProjectLoaded));
        assert!(!app.menu_gate_open(MenuGate::HasSelection));
        // 无项目时"复制组件"等需要选中的条目也必须灰置。
        assert!(MenuGate::HasSelection.reason().contains("选中"));
    }

    /// 组件菜单由注册表生成：类别齐全，且每个组件名都能被 `add_component` 认识。
    #[test]
    fn component_menu_comes_from_the_registry() {
        let groups = crate::panels::inspector::component_menu_groups();
        let categories: Vec<&str> = groups.iter().map(|(category, _)| *category).collect();
        for expected in ["变换与相机", "玩法", "渲染", "3D", "UI"] {
            assert!(categories.contains(&expected), "缺少类别 {expected}");
        }
        let names: Vec<&str> = groups
            .iter()
            .flat_map(|(_, names)| names.iter().copied())
            .collect();
        assert!(names.contains(&"Sprite"), "注册表里应有 Sprite");
        assert!(names.contains(&"Light"), "注册表里应有 Light");
        // 运行时切场景不销毁是**组件**（无载荷标记），不再由 Inspector 复选框写 metadata。
        assert!(
            names.contains(&rustlua_scene::scene::PERSISTENT_COMPONENT),
            "注册表里应有 Persistent 标记组件"
        );
        // 第 57 轮：变换与相机各只有**一个**组件，旧名已经从注册表删除，
        // 因此既不会作为可添加项出现，也不会在加载时被认出来。
        assert!(names.contains(&"Transform"), "注册表里应有统一的 Transform");
        assert!(names.contains(&"Camera"), "注册表里应有统一的 Camera");
        for legacy in ["Transform3D", "Camera3D", "Camera2D"] {
            assert!(
                !names.contains(&legacy),
                "旧名 {legacy} 不得作为可添加组件：{names:?}"
            );
        }
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "组件菜单不能有重复项");
    }

    fn menu_test_app() -> EditorApp {
        let project = editor::ProjectInfo {
            root: std::env::temp_dir().join(format!("rustlua-menu-{}", crate::util::test_token())),
            version: "test".to_owned(),
            directories: Vec::new(),
        };
        EditorApp::new(project, None, rustlua_infra::logging::LogSink::new(64))
    }
}
