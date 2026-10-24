//! M27-00：egui_dock 多标签停靠工作台（阶段 1）。
//!
//! 取代原先硬编码在 `main.rs::draw_frame` 里的四个固定面板区（`SidePanel` 左/右 +
//! `TopBottomPanel` 底 + `CentralPanel`）：整块工作区现在是一棵 [`egui_dock::DockState`]
//! 树，装在一个 `CentralPanel` 里，面板可拖拽重排、并排、堆叠成标签组。
//!
//! 三条不变量：
//!
//! 1. **默认树 = 迁移前的布局**（左列资源/层级、中央 Scene+Game、右属性、底部控制台），
//!    升级首帧的观感与旧版一致；
//! 2. **面板只画不改**：这里只做"取 `&mut Ui` → 调面板 body"，提交逻辑仍在各面板自己的
//!    shell 模块里；
//! 3. **`workspace_tab` 由标签身份派生**：Scene/Game 标签在渲染自己时写回 `workspace_tab`，
//!    工具栏的视图切换按钮改为"聚焦到对应标签"，避免两套状态互相追。
//!
//! 借用形态：`DockArea` 要 `&mut DockState`，`TabViewer` 又要 `&mut EditorApp`（含同一
//! 字段），直接双借用被编译器拒绝 ⇒ 每帧一次 `take`/`restore`（唯一的舞蹈发生地，见
//! [`show_workspace`]）。
//!
//! API 口径（0.18.0 实测，与网上旧教程不同）：**没有 `PanelUi` trait**，`TabViewer` 也不带
//! 泛型参数，内容统一走 `TabViewer::{id, title, ui}`；渲染入口是
//! `DockArea::new(&mut state).show_inside(ui, &mut viewer)`。

use crate::{EditorApp, WorkspaceTab};
use eframe::egui::{self, Id, WidgetText};
use egui_dock::{DockArea, DockState, Node, NodeIndex, Split, TabViewer, Tree};

/// 可停靠面板全集（阶段 1）。
///
/// `serde` derive 是阶段 2 的硬要求：`DockState<DockTab>` 整体要靠 egui_dock 自带的
/// serde 实现落盘，标签类型自己不可序列化就带不动整棵树。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
pub(crate) enum DockTab {
    Resource,
    Hierarchy,
    Scene,
    Game,
    Inspector,
    /// M75-03 内置 AI 助手：默认与属性同叶（属性仍是活动标签），不改变旧版首帧几何。
    Assistant,
    Console,
    /// P3-E01-3 `.anim` 时间线（只读预览）：默认与控制台同叶，控制台保持活动标签，
    /// 因此升级首帧看到的仍是控制台（几何不变）。
    Timeline,
    /// P3-E03-2 可视化脚本画布：默认不在树里，双击 `.vgraph` 资源时由 `sync_visibility` 补入。
    Graph,
    /// M76-04 UI Studio：默认不在树里，View 菜单打开时补入中央标签组。
    UiStudio,
}

impl DockTab {
    fn title(self) -> &'static str {
        match self {
            Self::Resource => "资源面板",
            Self::Hierarchy => "层级",
            Self::Scene => "Scene",
            Self::Game => "Game",
            Self::Inspector => "属性",
            Self::Assistant => "AI 助手",
            Self::Console => "控制台",
            Self::Timeline => "时间线",
            Self::Graph => "可视化脚本",
            Self::UiStudio => "UI Studio",
        }
    }

    /// `perf_baseline` 的面板计时桶名（与迁移前 `record_panel` 的键一致）。
    fn profiler_key(self) -> &'static str {
        match self {
            Self::Resource => "resource_browser",
            Self::Hierarchy => "hierarchy",
            Self::Scene | Self::Game => "workspace",
            Self::Inspector => "inspector",
            Self::Assistant => "assistant",
            Self::Console => "console",
            Self::Timeline => "timeline",
            Self::Graph => "graph",
            Self::UiStudio => "ui_studio",
        }
    }
}

/// 迁移前各面板的默认尺寸（名义工作区 1280×720），用来换算默认树的拆分比例，
/// 这样"升级首帧布局与旧版一致"这条验收不依赖用户当前窗口大小。
const SIZE_RESOURCE: f32 = 200.0;
const SIZE_HIERARCHY: f32 = 240.0;
const SIZE_INSPECTOR: f32 = 320.0;
const SIZE_CONSOLE: f32 = 150.0;
const NOMINAL_WIDTH: f32 = 1280.0;
const NOMINAL_HEIGHT: f32 = 720.0;

/// 左列（资源+层级）的名义宽度，`add_tab` 与默认树共用同一口径。
const SIZE_LEFT_COLUMN: f32 = SIZE_RESOURCE + SIZE_HIERARCHY;

/// 左列（资源+层级）里资源所占比例：`remove_tab` 会把腾出的空间还给兄弟节点，
/// 所以重新显示资源面板时锚点（层级）正好是整个左列。
const RESOURCE_OF_LEFT_COLUMN: f32 = SIZE_RESOURCE / SIZE_LEFT_COLUMN;

/// 迁移前的布局：`[资源 | 层级 | (Scene+Game) | 属性]`，底部整宽控制台。
///
/// 从"中央 Scene+Game 标签组"出发逐次拆分：每次都用 [`Tree::find_tab_from`] 重新定位
/// 叶子，因为拆分会搬动其它 `NodeIndex`。
#[must_use]
pub(crate) fn default_dock_state() -> DockState<DockTab> {
    let mut state = DockState::new(vec![DockTab::Scene, DockTab::Game]);
    let tree = state.main_surface_mut();
    split_tab(
        tree,
        DockTab::Scene,
        Split::Left,
        SIZE_LEFT_COLUMN / NOMINAL_WIDTH,
        DockTab::Hierarchy,
    );
    split_tab(
        tree,
        DockTab::Hierarchy,
        Split::Left,
        RESOURCE_OF_LEFT_COLUMN,
        DockTab::Resource,
    );
    split_tab(
        tree,
        DockTab::Scene,
        Split::Right,
        SIZE_INSPECTOR / (NOMINAL_WIDTH - SIZE_LEFT_COLUMN),
        DockTab::Inspector,
    );
    // M75-03：助手是属性的第二个标签（属性保持活动）——旧版首帧几何因此完全不变，
    // 用户需要时点一下标签即可，不必先把属性面板拖小。
    insert_assistant_beside_inspector(tree);
    split_at(
        tree,
        NodeIndex::root(),
        Split::Below,
        SIZE_CONSOLE / NOMINAL_HEIGHT,
        Node::leaf(DockTab::Console),
    );
    // E01-3：时间线与控制台同叶，控制台仍是活动标签——升级首帧的观感保持不变。
    insert_timeline_beside_console(tree);
    state
}

/// 把时间线插进控制台所在叶子的第二个位置，然后把活动标签还原成控制台。
fn insert_timeline_beside_console(tree: &mut Tree<DockTab>) {
    let Some((node, index)) = tree.find_tab_from(|candidate| *candidate == DockTab::Console) else {
        tree.push_to_first_leaf(DockTab::Timeline);
        return;
    };
    let Node::Leaf(leaf) = &mut tree[node] else {
        tree.push_to_first_leaf(DockTab::Timeline);
        return;
    };
    leaf.insert_tab(index.0 + 1, DockTab::Timeline);
    leaf.set_active_tab(index);
}

/// 把助手插进属性所在叶子的第二个位置，然后把活动标签还原成属性。
fn insert_assistant_beside_inspector(tree: &mut Tree<DockTab>) {
    let Some((node, index)) = tree.find_tab_from(|candidate| *candidate == DockTab::Inspector)
    else {
        // 没有属性面板（默认树里不会发生）：退化成常驻的第一片叶子，面板不能丢。
        tree.push_to_first_leaf(DockTab::Assistant);
        return;
    };
    let Node::Leaf(leaf) = &mut tree[node] else {
        tree.push_to_first_leaf(DockTab::Assistant);
        return;
    };
    leaf.insert_tab(index.0 + 1, DockTab::Assistant);
    leaf.set_active_tab(index);
}

/// 在 `anchor` 旁边切出占其空间 `new_fraction` 的新标签。
fn split_tab(
    tree: &mut Tree<DockTab>,
    anchor: DockTab,
    at: Split,
    new_fraction: f32,
    new: DockTab,
) {
    let node = tree
        .find_tab_from(|candidate| *candidate == anchor)
        .map(|(node, _)| node)
        .expect("dock 树必须包含该锚点标签");
    split_at(tree, node, at, new_fraction, Node::leaf(new));
}

/// `Tree::split` 的 `fraction` 永远是**第一个子节点（左/上）**的占比，而 `Right`/`Below`
/// 拆分里第一个子节点是*旧*节点——直接把"新标签想要多大"传进去会得到反过来互补的值
/// （实测：控制台会占掉 79% 的高度）。这里按方向换算一次，调用点可按字面意思读。
fn split_at(
    tree: &mut Tree<DockTab>,
    node: NodeIndex,
    at: Split,
    new_fraction: f32,
    new: Node<DockTab>,
) {
    let fraction = match at {
        Split::Left | Split::Above => new_fraction,
        Split::Right | Split::Below => 1.0 - new_fraction,
    };
    tree.split(node, at, fraction, new);
}

/// 阶段 2 持久化：把 dock 树编码成可落盘的 JSON（`layout.json` 的 `dock_tree` 键）。
///
/// 用 egui_dock 自带的 serde 实现（`serde` feature），整棵树含活动标签、折叠态和浮动窗口
/// 一起带走。树里的 `Rect` 是绝对坐标，看着碍眼但无害：`DockArea` 每帧都从根矩形按
/// `fraction` 重算所有节点矩形（`widgets/dock_area/show/mod.rs` 的 `set_rect` 链），
/// 读回来的旧坐标在首帧就被覆盖，所以布局与窗口大小无关。
///
/// 要防的是一类具体的坏值：`Rect::NOTHING` 里的 ±∞ 经 `serde_json` 会变成 `null`，
/// 写进文件就再也读不回来（egui_dock 在 `WindowState::rect` 的注释里踩过同一个坑，
/// 它的默认值因此特意不用 `Rect::NOTHING`）。所以这里序列化后立刻试解析一次，
/// 失败就返回 `None`——宁可这一次不存布局，也不写一个打不开的文件。
#[must_use]
pub(crate) fn serialize(state: &DockState<DockTab>) -> Option<serde_json::Value> {
    let value = serde_json::to_value(state).ok()?;
    serde_json::from_value::<DockState<DockTab>>(value.clone())
        .ok()
        .map(|_| value)
}

/// 读回 dock 树；格式不符、含 `null` 浮点、标签重复都返回 `None`，由调用方回落默认树。
#[must_use]
pub(crate) fn deserialize(value: &serde_json::Value) -> Option<DockState<DockTab>> {
    let state = serde_json::from_value::<DockState<DockTab>>(value.clone()).ok()?;
    // 同一个标签出现两份会打断 [`sync_visibility`] 的前提（它按"树里有没有这个标签"来决定
    // 补还是删）：藏掉一处，另一处仍在。这种树按损坏处理，不做修复。
    let mut tabs = state
        .iter_all_tabs()
        .map(|(_, tab)| *tab)
        .collect::<Vec<_>>();
    tabs.sort_by_key(|tab| *tab as u8);
    let count = tabs.len();
    tabs.dedup();
    (tabs.len() == count).then_some(state)
}

/// 结构指纹：只覆盖"用户能感知的布局"——表面（含浮动窗）集合、树形状、拆分方向与比例、
/// 每个叶子的标签顺序、活动标签、折叠态。**不含矩形与标签栏滚动量**。
///
/// 用途是每帧一次的廉价变化检测：节点矩形随窗口大小每帧都变，直接比较序列化后的 JSON 会
/// 让"拖动窗口边缘"变成每帧写盘。哈希只在本次进程内比较、从不落盘，所以 `DefaultHasher`
/// 的算法跨版本不稳定无关紧要。
#[must_use]
pub(crate) fn signature(state: &DockState<DockTab>) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (index, surface) in state.iter_surfaces().enumerate() {
        // `Surface::Empty` 是关掉浮动窗后留下的洞，与"根本没有这个表面"要分开发哈希，
        // 否则"少一个窗口"和"窗口在但为空"会撞出同一个指纹。
        (index as u32).hash(&mut hasher);
        let Some(tree) = surface.node_tree() else {
            0u8.hash(&mut hasher);
            continue;
        };
        for node in tree.iter() {
            match node {
                Node::Empty => 0u8.hash(&mut hasher),
                Node::Leaf(leaf) => {
                    1u8.hash(&mut hasher);
                    leaf.tabs.len().hash(&mut hasher);
                    for tab in &leaf.tabs {
                        (*tab as u8).hash(&mut hasher);
                    }
                    leaf.active.0.hash(&mut hasher);
                    leaf.collapsed.hash(&mut hasher);
                }
                // 方向必须进哈希：同一个比例在横向/纵向是两套布局。
                Node::Horizontal(split) => {
                    2u8.hash(&mut hasher);
                    split.fraction.to_bits().hash(&mut hasher);
                    split.fully_collapsed.hash(&mut hasher);
                }
                Node::Vertical(split) => {
                    3u8.hash(&mut hasher);
                    split.fraction.to_bits().hash(&mut hasher);
                    split.fully_collapsed.hash(&mut hasher);
                }
            }
        }
    }
    hasher.finish()
}

/// 让 `WorkspaceTab` 成为 dock 的当前标签（工具栏视图按钮与启动恢复走这里）。
///
/// 既要改标签组的活动 tab，也要把焦点给这个窗口：`Tree::find_active_focused` 读的是
/// *聚焦窗口* 的活动标签，只设 `set_active_tab` 时"当前视图"仍停在旧焦点上。
pub(crate) fn focus_workspace_tab(state: &mut DockState<DockTab>, tab: WorkspaceTab) {
    let target = match tab {
        WorkspaceTab::Scene => DockTab::Scene,
        WorkspaceTab::Game => DockTab::Game,
    };
    let _ = focus_tab(state, target);
}

/// 聚焦任意标签（M75-03：GUI 证据要能切到助手；View 菜单/脚本化运行也共用）。
///
/// 返回 `false` 表示该标签当前不在树里（被 View 菜单关掉了），调用方应先让
/// `sync_visibility` 把它补回来再聚焦。
pub(crate) fn focus_tab(state: &mut DockState<DockTab>, target: DockTab) -> bool {
    let tree = state.main_surface_mut();
    if let Some((node, index)) = tree.find_tab_from(|candidate| *candidate == target) {
        tree.set_focused_node(node);
        tree.set_active_tab(node, index);
        true
    } else {
        false
    }
}

/// 当前活动标签：优先读聚焦表面；首帧尚未渲染（`focused_surface` 为空）时退回主表面。
///
/// 全局 Undo 的「UI Studio 聚焦时消费 UI 撤销栈」按这个结果分流。
pub(crate) fn active_tab(state: &mut DockState<DockTab>) -> Option<DockTab> {
    state
        .find_active_focused()
        .map(|(_, tab)| *tab)
        .or_else(|| {
            state
                .main_surface_mut()
                .find_active_focused()
                .map(|(_, tab)| *tab)
        })
}

/// 面板可见性 ↔ 树的成员关系（View 菜单只改 `show_*` 布尔，这里负责落到树上）。
///
/// 查找必须跨**所有表面**：标签被用户拖成浮动窗口时它不在主表面，按主表面判断会每帧
/// 再补一份重复标签（同一个 `DockTab` 画两处）。关掉浮动窗则真的消失了，下一帧自动
/// 补回主表面——面板不会丢，这六个视图的开关只有 View 菜单一个来源。
///
/// 重新显示时落回原侧（资源在层级左、控制台在底部），但**不还原**本次会话里移除之前的
/// 精确位置与宽度——移除标签会把它让出的空间还给兄弟节点，锚点已经不在原来的尺寸上。
/// 跨重启的精确布局由 [`serialize`]/[`deserialize`] 负责：树整体落盘，重启后就在用户放的地方。
pub(crate) fn sync_visibility(state: &mut DockState<DockTab>, editor: &EditorApp) {
    set_tab_present(state, DockTab::Resource, editor.show_resources_panel);
    set_tab_present(state, DockTab::Hierarchy, editor.show_hierarchy_panel);
    set_tab_present(state, DockTab::Inspector, editor.show_inspector_panel);
    set_tab_present(state, DockTab::Assistant, editor.show_assistant_panel);
    // Scene 先于 Game：两个一起重新显示时，标签顺序仍是「Scene | Game」。
    set_tab_present(state, DockTab::Scene, editor.show_scene_view);
    set_tab_present(state, DockTab::Game, editor.show_game_view);
    set_tab_present(state, DockTab::Console, editor.show_lua_console);
    set_tab_present(state, DockTab::Timeline, editor.show_timeline_panel);
    set_tab_present(state, DockTab::Graph, editor.show_graph_panel);
    set_tab_present(state, DockTab::UiStudio, editor.show_ui_studio);
}

fn set_tab_present(state: &mut DockState<DockTab>, tab: DockTab, present: bool) {
    match (present, state.find_tab(&tab)) {
        (true, None) => add_tab(state, tab),
        (false, Some(location)) => {
            state.remove_tab(location);
        }
        _ => {}
    }
}

/// 某个标签在默认树里"原侧"的落点。锚点取**还留在树里的那个邻居**，全都不在才退到根。
fn add_tab(state: &mut DockState<DockTab>, tab: DockTab) {
    let tree = state.main_surface_mut();
    let leaf_of = |anchor: DockTab| {
        tree.find_tab_from(|candidate| *candidate == anchor)
            .map(|(node, _)| node)
    };
    match tab {
        DockTab::Resource => {
            let anchor = leaf_of(DockTab::Hierarchy).or_else(|| leaf_of(DockTab::Scene));
            if let Some(node) = anchor {
                split_at(
                    tree,
                    node,
                    Split::Left,
                    RESOURCE_OF_LEFT_COLUMN,
                    Node::leaf(tab),
                );
            }
        }
        DockTab::Hierarchy => {
            // 资源面板还在时挂在它右边（同一列里，比例按列宽算）；整列都没了才从中央切。
            match leaf_of(DockTab::Resource) {
                Some(node) => split_at(
                    tree,
                    node,
                    Split::Right,
                    SIZE_HIERARCHY / SIZE_LEFT_COLUMN,
                    Node::leaf(tab),
                ),
                None => split_at(
                    tree,
                    central_anchor(tree),
                    Split::Left,
                    SIZE_LEFT_COLUMN / NOMINAL_WIDTH,
                    Node::leaf(tab),
                ),
            }
        }
        DockTab::Inspector => {
            // 助手还在树里时：属性回到助手左边，恢复同一标签组（而不是在中央另开一列，
            // 那会把用户刚调好的工作区宽度再切一次）。
            if let Some((node, index)) =
                tree.find_tab_from(|candidate| *candidate == DockTab::Assistant)
            {
                let Node::Leaf(leaf) = &mut tree[node] else {
                    return;
                };
                leaf.insert_tab(index.0, tab);
                leaf.set_active_tab(index.0);
            } else {
                let anchor = leaf_of(DockTab::Scene)
                    .or_else(|| leaf_of(DockTab::Game))
                    .unwrap_or_else(NodeIndex::root);
                split_at(
                    tree,
                    anchor,
                    Split::Right,
                    SIZE_INSPECTOR / (NOMINAL_WIDTH - SIZE_LEFT_COLUMN),
                    Node::leaf(tab),
                );
            }
        }
        DockTab::Assistant => {
            // 默认与属性同叶；属性不在时按"属性那一列"的锚点新开一块，面板位置仍可预期。
            if let Some((node, index)) =
                tree.find_tab_from(|candidate| *candidate == DockTab::Inspector)
            {
                let Node::Leaf(leaf) = &mut tree[node] else {
                    return;
                };
                let inspector_was_active = leaf.active == index;
                leaf.insert_tab(index.0 + 1, tab);
                if inspector_was_active {
                    leaf.set_active_tab(index);
                }
            } else {
                let anchor = leaf_of(DockTab::Scene)
                    .or_else(|| leaf_of(DockTab::Game))
                    .unwrap_or_else(NodeIndex::root);
                split_at(
                    tree,
                    anchor,
                    Split::Right,
                    SIZE_INSPECTOR / (NOMINAL_WIDTH - SIZE_LEFT_COLUMN),
                    Node::leaf(tab),
                );
            }
        }
        // 中央两个标签同属一个标签组：重新显示时插回同一个叶子，不新开一块区域。
        DockTab::UiStudio => {
            // UI Studio 与 Game 同叶：打开时默认贴着中央标签组，不额外切宽。
            match tree
                .find_tab_from(|candidate| *candidate == DockTab::Game)
                .or_else(|| tree.find_tab_from(|candidate| *candidate == DockTab::Scene))
            {
                Some((node, index)) => tree[node].insert_tab((index.0 + 1).into(), tab),
                None => tree.push_to_first_leaf(tab),
            }
        }
        DockTab::Scene | DockTab::Game => {
            let other = if tab == DockTab::Scene {
                DockTab::Game
            } else {
                DockTab::Scene
            };
            match tree.find_tab_from(|candidate| *candidate == other) {
                Some((node, index)) => {
                    let at = if tab == DockTab::Scene {
                        index.0
                    } else {
                        index.0 + 1
                    };
                    tree[node].insert_tab(at.into(), tab);
                }
                // 两个中央视图都不在树里（手改过的布局）：退化成"挂到第一片叶子"。
                None => tree.push_to_first_leaf(tab),
            }
        }
        // 锚点是整棵树的根：移除控制台时它让出的空间回到了上方整块区域，
        // 挂在 Scene 叶子上只会切走 Scene 自己那一小块。
        DockTab::Console => split_at(
            tree,
            NodeIndex::root(),
            Split::Below,
            SIZE_CONSOLE / NOMINAL_HEIGHT,
            Node::leaf(tab),
        ),
        DockTab::Graph => {
            // 图与 Scene 并排（右侧），不抢中央标签组；Scene/Game 都不在时挂第一片叶子。
            match leaf_of(DockTab::Scene).or_else(|| leaf_of(DockTab::Game)) {
                Some(node) => split_at(tree, node, Split::Right, 0.34, Node::leaf(tab)),
                None => tree.push_to_first_leaf(tab),
            }
        }
        DockTab::Timeline => {
            // 时间线的"原侧"是控制台标签组的第二个标签；控制台不在主表面（浮动窗里或
            // 被关掉）时退回到"底部整宽"，仍可预期。
            match tree.find_tab_from(|candidate| *candidate == DockTab::Console) {
                Some((node, index)) => {
                    let Node::Leaf(leaf) = &mut tree[node] else {
                        return;
                    };
                    let console_was_active = leaf.active == index;
                    leaf.insert_tab(index.0 + 1, tab);
                    if console_was_active {
                        leaf.set_active_tab(index);
                    }
                }
                None => split_at(
                    tree,
                    NodeIndex::root(),
                    Split::Below,
                    SIZE_CONSOLE / NOMINAL_HEIGHT,
                    Node::leaf(tab),
                ),
            }
        }
    }
}

/// 中央工作区当前占的叶子（层级/属性要按它来切宽度）。
fn central_anchor(tree: &Tree<DockTab>) -> NodeIndex {
    tree.find_tab_from(|candidate| *candidate == DockTab::Scene)
        .or_else(|| tree.find_tab_from(|candidate| *candidate == DockTab::Game))
        .map(|(node, _)| node)
        .unwrap_or_else(NodeIndex::root)
}

/// M27-03 验收 1 的"反复重排面板"负载：按 `step % 6` 做一次**成对可逆**的结构变更，
/// 六步一轮转下来六个常驻视图一个不多一个不少。
///
/// 走的是用户拖动落在的**同一批 `DockState` API**（`detach_tab` / `remove_tab` +
/// [`add_tab`] / 叶子之间搬标签），因此压到的是同一条"结构变化 → 指纹 → 稳定 300ms 落盘
/// → 下一帧重排"的路径。差别必须说清楚：它**不是真实鼠标**，`DockArea` 的拖拽 hit-test
/// 与拖拽中的视觉反馈不在本负载覆盖内（那部分只能靠合成帧用例 + 人眼，见 `M27-03` 的
/// 未完成项）。
///
/// 返回这一步**实际**做了什么（含"跳过：原因"），soak 报告里逐条留痕——静默 no-op
/// 会被 200 步不变量测试当场抓出来。
pub(crate) fn churn(state: &mut DockState<DockTab>, step: usize) -> String {
    match step % 6 {
        0 => detach_console_to_float(state),
        1 => redock_console(state),
        2 => merge_into_group(state, DockTab::Inspector, DockTab::Scene),
        3 => split_back_out(state, DockTab::Inspector),
        4 => merge_into_group(state, DockTab::Hierarchy, DockTab::Resource),
        _ => split_back_out(state, DockTab::Hierarchy),
    }
}

/// 把控制台整块拆成浮动窗口。挑它是因为它是六个标签里唯一"底部整宽"的形态，
/// 拆出去能同时压到**表面增删 + 窗口创建**这条最贵的路径。
fn detach_console_to_float(state: &mut DockState<DockTab>) -> String {
    let Some(location) = state.find_tab(&DockTab::Console) else {
        return "跳过：控制台不在树里".to_owned();
    };
    if !location.0.is_main() {
        return "跳过：控制台已经在浮动窗口里".to_owned();
    }
    let rect = egui::Rect::from_min_size(egui::pos2(160.0, 160.0), egui::vec2(480.0, 280.0));
    state.detach_tab(location, rect);
    "控制台拆成浮动窗口".to_owned()
}

fn redock_console(state: &mut DockState<DockTab>) -> String {
    let Some(location) = state.find_tab(&DockTab::Console) else {
        return "跳过：控制台不在树里".to_owned();
    };
    if location.0.is_main() {
        return "跳过：控制台不在浮动窗口里".to_owned();
    }
    let Some(console) = state.remove_tab(location) else {
        return "跳过：控制台搬移失败".to_owned();
    };
    // `remove_tab` 会把空表面一并删掉；回主表面时按 View 菜单的同一套锚点规则落位，
    // 而不是自己另发明一种"放回去"的位置。
    add_tab(state, console);
    "控制台并回底部".to_owned()
}

/// 把 `tab` 并进 `anchor` 所在的标签组（等价于"拖到另一个标签页上放进来"）。
fn merge_into_group(state: &mut DockState<DockTab>, tab: DockTab, anchor: DockTab) -> String {
    let Some(location) = state.find_tab(&tab) else {
        return "跳过：该面板不在树里".to_owned();
    };
    let Some(held) = state.remove_tab(location) else {
        return "跳过：该面板搬移失败".to_owned();
    };
    let tree = state.main_surface_mut();
    // 锚点要在**搬走之后**再查一次：remove_tab 会清掉空叶子，`NodeIndex` 随之移动。
    match tree.find_tab_from(|candidate| *candidate == anchor) {
        Some((node, _)) => {
            tree[node].append_tab(held);
            format!("{}并入{}标签组", tab.title(), anchor.title())
        }
        None => {
            // 锚点也没了（用户自己关掉过）：标签必须回到树里，吞掉它就是丢面板。
            tree.push_to_first_leaf(held);
            format!(
                "{}回落到第一片叶子（{}不在树里）",
                tab.title(),
                anchor.title()
            )
        }
    }
}

/// 把 `tab` 从当前标签组拆成独立一块（等价于"把标签拖出去松开"），落点沿用 [`add_tab`]。
fn split_back_out(state: &mut DockState<DockTab>, tab: DockTab) -> String {
    let Some(location) = state.find_tab(&tab) else {
        return "跳过：该面板不在树里".to_owned();
    };
    if location.0 != egui_dock::SurfaceIndex::main() {
        return format!("跳过：{}不在主表面", tab.title());
    }
    let Some(held) = state.remove_tab(location) else {
        return "跳过：该面板搬移失败".to_owned();
    };
    add_tab(state, held);
    format!("{}拆成独立区域", tab.title())
}

/// 把整棵 dock 树画进本帧的 `CentralPanel`（菜单/标题栏仍是外层的 `TopBottomPanel`）。
pub(crate) fn show_workspace(context: &egui::Context, editor: &mut EditorApp) {
    let Some(mut dock_state) = editor.dock_state.take() else {
        return;
    };
    sync_visibility(&mut dock_state, editor);
    if editor.pending_ui_studio_focus {
        editor.pending_ui_studio_focus = false;
        let _ = focus_tab(&mut dock_state, DockTab::UiStudio);
    }
    // 上一帧面板 body 里发出的聚焦请求（如工具栏 Play 自动切 Game）：树已放回、
    // 可见性已同步（标签被隐藏过时先补回再聚焦），画之前补应用。
    if let Some(tab) = editor.pending_workspace_focus.take() {
        focus_workspace_tab(&mut dock_state, tab);
    }
    egui::CentralPanel::default().show(context, |ui| {
        // 这些面板是常驻视图：可见性只有 View 菜单一个来源，标签上不提供关闭/折叠按钮
        // （关掉的面板下一帧会被 [`sync_visibility`] 补回主表面）。附带一条硬理由：
        // egui_dock 的 tab bar 会为关闭/折叠按钮预留 48px，窄标签页因此能把可用宽度正好
        // 压成 0 并触发其内部断言。浮动窗口的关闭按钮保留（0.18 的替代 setter 尚未提供）。
        DockArea::new(&mut dock_state)
            .show_add_buttons(false)
            .show_close_buttons(false)
            .show_leaf_close_all_buttons(false)
            .show_leaf_collapse_buttons(false)
            .show_inside(ui, &mut DockTabViewer(editor));
    });
    editor.dock_state = Some(dock_state);
}

/// `TabViewer`：只负责"标题 + 取面板 body"，不持任何文档写路径。
struct DockTabViewer<'a>(&'a mut EditorApp);

impl TabViewer for DockTabViewer<'_> {
    type Tab = DockTab;

    fn id(&mut self, tab: &mut Self::Tab) -> Id {
        Id::new(tab)
    }

    fn title(&mut self, tab: &mut Self::Tab) -> WidgetText {
        tab.title().into()
    }

    /// egui_dock 默认给每个标签体套一层双向 `ScrollArea`，全部关掉：Scene/Game 画布是
    /// "适配视口"的内容且自带缩放，外层滚动条只会碍事；其余面板内部各自已有
    /// `ScrollArea::vertical()`，外层这层只会多出一条重复的横向滚动条。
    fn scroll_bars(&self, _tab: &Self::Tab) -> [bool; 2] {
        [false, false]
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab) {
        let editor = &mut *self.0;
        let started = std::time::Instant::now();
        match tab {
            DockTab::Resource => {
                if editor.draw_resource_browser_body(ui) {
                    // 用户显式刷新：清空"已尝试"记录，让上次没导入成功的资源重新排队
                    // （自动路径不会清，见 `ResourceBrowserState::import_attempted`）。
                    editor.resources.clear_import_attempts();
                    editor.refresh_resources();
                }
            }
            DockTab::Hierarchy => editor.draw_hierarchy_body(ui),
            DockTab::Scene | DockTab::Game => {
                editor.workspace_tab = match tab {
                    DockTab::Game => WorkspaceTab::Game,
                    _ => WorkspaceTab::Scene,
                };
                // 浮层定位（性能面板等）依赖"工作区矩形"：迁移前取 `CentralPanel` 的响应
                // 矩形，现在取本标签的内容矩形。
                editor.workspace_rect = ui.max_rect();
                // 工具条画在一个把宽度钉到裁剪框的子 ui 里。egui_dock 会在本回调之前
                // `expand_to_include_rect(available_rect)`（show/leaf.rs 的 ScrollArea 内），
                // 只要工具条某一帧横向溢出裁剪框，之后每帧可用宽度都停在溢出值、
                // 工具条不再换行，zoom 数值就一直压在右侧属性面板下（窄叶子才触发）。
                // 给工具条一个宽度=可见裁剪宽度的 scope，横向滚动只在可见区域内发生，
                // 溢出也不会再回灌到父 ui 的可用宽度。
                // 原点必须留在父 ui 的内容原点：直接把 max_rect 钉成 clip_rect 会把子 ui
                // 起点推到裁剪框左缘（比标签体内容内缩还左 2~3px），整行工具条因此压过
                // 标签栏左边界（用户截图实测：工具条 x=25 vs 标签边框 x=27）。
                let band = ui.available_rect_before_wrap();
                let band = egui::Rect::from_min_size(
                    band.min,
                    egui::vec2(band.width().min(ui.clip_rect().width()), band.height()),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(band), |ui| {
                    editor.draw_workspace_toolbar(ui);
                });
                ui.separator();
                match tab {
                    DockTab::Game => editor.draw_game_view(ui),
                    _ => editor.draw_scene_view(ui),
                }
            }
            DockTab::Inspector => editor.draw_inspector_panel(ui),
            DockTab::Assistant => editor.draw_assistant_panel(ui),
            DockTab::Console => editor.draw_console_body(ui),
            DockTab::Timeline => editor.draw_timeline_panel(ui),
            DockTab::Graph => editor.draw_graph_panel(ui),
            DockTab::UiStudio => editor.draw_ui_studio_panel(ui),
        }
        editor
            .profiler
            .record_panel(tab.profiler_key(), started.elapsed());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EditorApp;
    use rustlua_infra::logging::LogSink;

    fn sample_app() -> EditorApp {
        let root =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples/game_api");
        let project = editor::open_project(&root).expect("打开样板工程");
        EditorApp::new(project, None, LogSink::new(16))
    }

    fn tabs_of(state: &DockState<DockTab>) -> Vec<DockTab> {
        state.main_surface().tabs().copied().collect()
    }

    fn collect_text(output: &egui::FullOutput) -> String {
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

    /// 渲染一帧后的叶子矩形（`Node::rect()` 只有画过一帧才有效）。
    fn leaf_rects(state: &DockState<DockTab>) -> Vec<(DockTab, egui::Rect)> {
        state
            .main_surface()
            .iter()
            .filter_map(|node| {
                let rect = node.rect()?;
                let tabs = node.tabs()?;
                Some(
                    tabs.iter()
                        .copied()
                        .map(move |tab| (tab, rect))
                        .collect::<Vec<_>>(),
                )
            })
            .flatten()
            .collect()
    }

    fn rect_of(state: &DockState<DockTab>, tab: DockTab) -> egui::Rect {
        leaf_rects(state)
            .into_iter()
            .find(|(candidate, _)| *candidate == tab)
            .map(|(_, rect)| rect)
            .unwrap_or(egui::Rect::NOTHING)
    }

    /// 渲染一帧（合成帧，不依赖 GPU），把 dock 树画到真实矩形上。
    fn draw_once(app: &mut EditorApp, width: f32, height: f32) {
        let context = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, height),
            )),
            ..Default::default()
        };
        let _ = context.run(raw, |context| show_workspace(context, app));
    }

    /// 递归找到序列化树里的第一个 `"tabs"` 数组（用来手改文件内容）。
    fn first_tabs_array_mut(value: &mut serde_json::Value) -> Option<&mut Vec<serde_json::Value>> {
        match value {
            serde_json::Value::Object(map) => map.iter_mut().find_map(|(key, entry)| {
                if key.as_str() == "tabs" {
                    return match entry {
                        serde_json::Value::Array(tabs) => Some(tabs),
                        _ => None,
                    };
                }
                first_tabs_array_mut(entry)
            }),
            serde_json::Value::Array(items) => items.iter_mut().find_map(first_tabs_array_mut),
            _ => None,
        }
    }

    /// 默认树必须覆盖迁移前的六个面板区，外加 M75-03 的助手与 P3-E01-3 的时间线标签。
    #[test]
    fn default_tree_has_every_previous_panel_region() {
        let mut tabs = tabs_of(&default_dock_state());
        tabs.sort_by_key(|tab| *tab as u8);
        assert_eq!(
            tabs,
            vec![
                DockTab::Resource,
                DockTab::Hierarchy,
                DockTab::Scene,
                DockTab::Game,
                DockTab::Inspector,
                DockTab::Assistant,
                DockTab::Console,
                DockTab::Timeline,
            ]
        );
    }

    /// Scene 与 Game 必须在同一个标签组里（迁移前它们共用 CentralPanel 的两个 tab）。
    #[test]
    fn scene_and_game_share_one_tab_group() {
        let state = default_dock_state();
        let tree = state.main_surface();
        let scene = tree.find_tab_from(|tab| *tab == DockTab::Scene);
        let game = tree.find_tab_from(|tab| *tab == DockTab::Game);
        assert_eq!(
            scene.map(|(node, _)| node),
            game.map(|(node, _)| node),
            "Scene/Game 应同属一个标签组"
        );
    }

    /// **阶段 1 的核心验收**：默认树画出来的几何就是迁移前那套固定面板布局
    /// （左列 200+240、中央 Scene/Game、右侧属性 360、底部控制台 150）。
    ///
    /// 拆分比例写反（egui_dock 的 `fraction` 属于**左/上**子节点，不是新节点）会在这里
    /// 立刻暴露，而不是等肉眼看截图。
    #[test]
    fn default_layout_matches_the_previous_fixed_panels() {
        let mut app = sample_app();
        let context = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(NOMINAL_WIDTH, NOMINAL_HEIGHT),
            )),
            ..Default::default()
        };
        let _ = context.run(raw, |context| show_workspace(context, &mut app));
        let mut state = app.dock_state.take().expect("帧末必须放回 dock 树");

        let close = |label: &str, actual: f32, expected: f32| {
            assert!(
                (actual - expected).abs() < 12.0,
                "{label}：实际 {actual:.1}，期望 ≈{expected:.1}"
            );
        };

        // 拆分比例是相对"整棵树拿到的矩形"生效的，而 `CentralPanel` 自带 8px 内边距，
        // 所以断言按实测树矩形换算，不写死 1280×720 的像素值。
        let rects = leaf_rects(&state);
        let area = rects
            .iter()
            .fold(egui::Rect::NOTHING, |acc, (_, rect)| acc.union(*rect));
        let fx = area.width() / NOMINAL_WIDTH;
        let fy = area.height() / NOMINAL_HEIGHT;

        let resource = rect_of(&state, DockTab::Resource);
        close("资源面板左沿", resource.left(), area.left());
        close("资源面板宽度", resource.width(), SIZE_RESOURCE * fx);
        let hierarchy = rect_of(&state, DockTab::Hierarchy);
        close(
            "层级左沿",
            hierarchy.left(),
            area.left() + SIZE_RESOURCE * fx,
        );
        close("层级宽度", hierarchy.width(), SIZE_HIERARCHY * fx);
        let scene = rect_of(&state, DockTab::Scene);
        close(
            "Scene 左沿",
            scene.left(),
            area.left() + (SIZE_RESOURCE + SIZE_HIERARCHY) * fx,
        );
        close(
            "Scene 宽度",
            scene.width(),
            (NOMINAL_WIDTH - SIZE_RESOURCE - SIZE_HIERARCHY - SIZE_INSPECTOR) * fx,
        );
        close(
            "Scene 高度",
            scene.height(),
            (NOMINAL_HEIGHT - SIZE_CONSOLE) * fy,
        );
        let inspector = rect_of(&state, DockTab::Inspector);
        close(
            "属性左沿",
            inspector.left(),
            area.right() - SIZE_INSPECTOR * fx,
        );
        close("属性宽度", inspector.width(), SIZE_INSPECTOR * fx);
        let console = rect_of(&state, DockTab::Console);
        close(
            "控制台顶边",
            console.top(),
            area.top() + (NOMINAL_HEIGHT - SIZE_CONSOLE) * fy,
        );
        close("控制台高度", console.height(), SIZE_CONSOLE * fy);
        close("控制台宽度（整宽）", console.width(), area.width());
        assert_eq!(
            rect_of(&state, DockTab::Game),
            scene,
            "Game 应与 Scene 重叠在同一标签组"
        );
        assert_eq!(
            rect_of(&state, DockTab::Assistant),
            inspector,
            "AI 助手应与属性重叠在同一标签组（默认不改变旧版几何）"
        );
        assert_eq!(
            rect_of(&state, DockTab::Timeline),
            console,
            "时间线应与控制台重叠在同一标签组（默认不改变旧版几何）"
        );
        // 控制台必须仍是该标签组里的活动标签：默认首帧看到的仍是控制台。
        let (_, active) = state
            .main_surface_mut()
            .find_active_focused()
            .expect("默认树必须有活动标签");
        assert_eq!(*active, DockTab::Console);
    }

    /// 隐藏即移除、重新显示即回到原侧（资源在层级左侧、控制台在底部）。
    #[test]
    fn visibility_flags_add_and_remove_tabs() {
        let mut state = default_dock_state();
        let mut editor = sample_app();
        editor.show_resources_panel = false;
        editor.show_lua_console = false;
        editor.show_timeline_panel = false;
        sync_visibility(&mut state, &editor);
        let hidden = tabs_of(&state);
        assert!(!hidden.contains(&DockTab::Resource), "资源面板应被移除");
        assert!(!hidden.contains(&DockTab::Console), "控制台应被移除");
        assert!(!hidden.contains(&DockTab::Timeline), "时间线应被移除");
        assert_eq!(hidden.len(), 5, "其余标签不受影响");

        editor.show_resources_panel = true;
        editor.show_lua_console = true;
        editor.show_timeline_panel = true;
        sync_visibility(&mut state, &editor);
        let shown = tabs_of(&state);
        assert_eq!(shown.len(), 8, "重新显示后标签数复原");
        assert!(shown.contains(&DockTab::Resource));
        assert!(shown.contains(&DockTab::Console));
        assert!(shown.contains(&DockTab::Timeline));
    }

    /// 阶段 2 收尾：其余四个面板同样按开关增删，且各自回到"原侧"——
    /// 中央视图回到**同一个标签组**，层级回到资源右侧，属性回到中央右边。
    #[test]
    fn every_panel_toggle_lands_on_its_own_side() {
        let mut state = default_dock_state();
        let mut editor = sample_app();
        editor.show_hierarchy_panel = false;
        editor.show_inspector_panel = false;
        editor.show_scene_view = false;
        sync_visibility(&mut state, &editor);
        let hidden = tabs_of(&state);
        for tab in [DockTab::Hierarchy, DockTab::Inspector, DockTab::Scene] {
            assert!(!hidden.contains(&tab), "{tab:?} 应被移除");
        }
        assert_eq!(hidden.len(), 5, "其余标签不受影响：{hidden:?}");

        editor.show_hierarchy_panel = true;
        editor.show_inspector_panel = true;
        editor.show_scene_view = true;
        sync_visibility(&mut state, &editor);
        let shown = tabs_of(&state);
        assert_eq!(shown.len(), 8, "重新显示后标签数复原：{shown:?}");
        let node_of = |state: &mut DockState<DockTab>, tab: DockTab| {
            state
                .main_surface_mut()
                .find_tab_from(|candidate| *candidate == tab)
                .map(|(node, _)| node)
        };
        assert_eq!(
            node_of(&mut state, DockTab::Scene),
            node_of(&mut state, DockTab::Game),
            "Scene 必须回到 Game 的标签组，而不是新开一块区域"
        );
        assert_ne!(
            node_of(&mut state, DockTab::Resource),
            node_of(&mut state, DockTab::Hierarchy),
            "层级应作为资源右侧的独立叶子回来，而不是叠成同一标签组"
        );
    }

    /// 工具栏/启动恢复的视图切换必须真的改到 dock 的活动标签。
    #[test]
    fn focus_workspace_tab_switches_active_tab() {
        let mut state = default_dock_state();
        focus_workspace_tab(&mut state, WorkspaceTab::Game);
        let active = state
            .main_surface_mut()
            .find_active_focused()
            .map(|(_, tab)| *tab);
        assert_eq!(active, Some(DockTab::Game));
    }

    /// 面板 body 绘制中（dock 树被 `show_workspace` 取走）点工具栏 Play：聚焦请求
    /// 必须挂起、下一帧补应用——否则 `workspace_tab` 会被 Scene 标签的写回冲掉，
    /// "Play 自动切 Game"整条失效（M27-00 把工具栏搬进 dock 后留下的死角）。
    #[test]
    fn a_focus_requested_during_draw_applies_next_frame() {
        let mut app = sample_app();
        draw_once(&mut app, NOMINAL_WIDTH, NOMINAL_HEIGHT);
        // 模拟 body 里的调用时机：树正被取走。
        let saved = app.dock_state.take();
        app.focus_workspace_tab(WorkspaceTab::Game);
        assert_eq!(
            app.pending_workspace_focus,
            Some(WorkspaceTab::Game),
            "树不在时必须挂起"
        );
        app.dock_state = saved;
        draw_once(&mut app, NOMINAL_WIDTH, NOMINAL_HEIGHT);
        let mut state = app.dock_state.take().expect("帧末必须放回 dock 树");
        assert_eq!(
            state
                .main_surface_mut()
                .find_active_focused()
                .map(|(_, tab)| *tab),
            Some(DockTab::Game),
            "挂起的聚焦请求必须下一帧生效"
        );
        assert_eq!(
            app.workspace_tab,
            WorkspaceTab::Game,
            "写回必须停在 Game，不能被 Scene 冲掉"
        );
    }

    /// 整帧渲染（合成帧，不依赖 GPU）：真实面板内容必须画进 dock 标签里。
    #[test]
    fn workspace_renders_real_panel_bodies() {
        let mut app = sample_app();
        app.console_notices.push("dock 阶段 1 探针".to_owned());
        let context = egui::Context::default();
        let output = context.run(egui::RawInput::default(), |context| {
            show_workspace(context, &mut app);
        });
        let text = collect_text(&output);
        // 控制台面板无标题行：用表头的「清空」按钮判定面板画出来了。
        assert!(text.contains("清空"), "控制台面板没画出来：{text}");
        assert!(
            text.contains("dock 阶段 1 探针"),
            "控制台内容没有画进 dock 标签：{text}"
        );
        assert!(text.contains("层级"), "层级面板没画出来：{text}");
        assert!(text.contains("属性"), "属性面板没画出来：{text}");
        assert!(text.contains("资源面板"), "资源面板没画出来：{text}");
    }

    /// 极端小窗口：dock 必须仍画完八个标签且不 panic。
    ///
    /// egui_dock 0.18 的 tab bar 会为"关闭全部/折叠"按钮预留 48px，窄标签页的可用宽度被
    /// 正好压成 `0.0` 时它内部 `assert_ne!` 直接 panic（阶段 1 实测被 resize 用例打崩过）。
    /// 那些按钮对常驻面板没有意义，已经关掉——本条守住这个决定。
    #[test]
    fn tiny_viewport_keeps_every_tab_and_does_not_panic() {
        let mut app = sample_app();
        let context = egui::Context::default();
        for _ in 0..3 {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(240.0, 180.0),
                )),
                ..Default::default()
            };
            let _ = context.run(raw, |context| show_workspace(context, &mut app));
        }
        let state = app.dock_state.take().expect("帧末必须放回 dock 树");
        assert_eq!(tabs_of(&state).len(), 8, "小窗口不得丢标签");
    }

    /// 结构可读转写（与 [`signature`] 同一趟遍历）：指纹撞车时用它打印出到底哪不同。
    fn structure_of(state: &DockState<DockTab>) -> Vec<String> {
        let mut lines = Vec::new();
        for (index, surface) in state.iter_surfaces().enumerate() {
            let Some(tree) = surface.node_tree() else {
                continue;
            };
            for node in tree.iter() {
                lines.push(match node {
                    Node::Empty => format!("表面{index}:空"),
                    Node::Leaf(leaf) => format!(
                        "表面{index}:叶 {:?} 活动#{} 折叠#{}",
                        leaf.tabs, leaf.active.0, leaf.collapsed
                    ),
                    Node::Horizontal(split) => format!(
                        "表面{index}:横 {:.6} 折叠#{}",
                        split.fraction, split.fully_collapsed
                    ),
                    Node::Vertical(split) => format!(
                        "表面{index}:竖 {:.6} 折叠#{}",
                        split.fraction, split.fully_collapsed
                    ),
                });
            }
        }
        lines
    }

    /// 根拆分节点的占比（默认树的根是"上=工作区 / 下=控制台"那条横向切分）。
    fn root_fraction(state: &DockState<DockTab>) -> f32 {
        match &state.main_surface()[NodeIndex::root()] {
            Node::Vertical(split) | Node::Horizontal(split) => split.fraction,
            other => panic!("默认树的根必须是拆分节点：{other:?}"),
        }
    }

    /// 阶段 2 的核心验收：用户重排后的树能存能读，读回来的结构与指纹完全一致。
    #[test]
    fn a_rearranged_tree_round_trips_through_json() {
        let mut app = sample_app();
        draw_once(&mut app, NOMINAL_WIDTH, NOMINAL_HEIGHT);
        // 模拟一次真实重排：把控制台搬到属性上方。
        {
            let mut state = app.dock_state.take().expect("帧末必须放回 dock 树");
            let location = state.find_tab(&DockTab::Console).expect("默认树里有控制台");
            let console = state.remove_tab(location).expect("移除控制台标签");
            let tree = state.main_surface_mut();
            let inspector = tree
                .find_tab_from(|candidate| *candidate == DockTab::Inspector)
                .expect("属性叶子")
                .0;
            tree.split(inspector, Split::Above, 0.5, Node::leaf(console));
            app.dock_state = Some(state);
        }
        // 新建节点还没矩形（`Rect::NOTHING`），必须再画一帧才能落盘（见
        // [`an_undrawn_tree_is_refused_rather_than_written_broken`]）。
        draw_once(&mut app, NOMINAL_WIDTH, NOMINAL_HEIGHT);
        let state = app.dock_state.take().expect("帧末必须放回 dock 树");

        let value = serialize(&state).expect("画过一帧的树必须能序列化");
        let restored = deserialize(&value).expect("存下去必须读得回来");
        assert_eq!(
            structure_of(&restored),
            structure_of(&state),
            "结构必须原样恢复"
        );
        assert_eq!(signature(&restored), signature(&state), "指纹必须原样恢复");
        let sorted = |mut tabs: Vec<DockTab>| {
            tabs.sort_by_key(|tab| *tab as u8);
            tabs
        };
        assert_eq!(
            sorted(tabs_of(&restored)),
            sorted(tabs_of(&state)),
            "标签集合必须一致"
        );
        // 落点这类"树里的小状态"也得带走。
        let moved = restored
            .main_surface()
            .find_tab_from(|candidate| *candidate == DockTab::Console)
            .expect("控制台必须还在主表面");
        let original = state
            .main_surface()
            .find_tab_from(|candidate| *candidate == DockTab::Console)
            .expect("控制台必须还在主表面");
        assert_eq!(moved, original, "标签落点必须一致");
        app.dock_state = Some(state);
    }

    /// 拖出独立窗口（float）**也随树一起持久化**——这条把原计划"第一期不存 floats"推翻了：
    /// `DockState` 本来就把浮动窗当作额外表面存着，serde 实现一并带走。
    ///
    /// 两个必须同时成立的点：浮动窗的矩形是**绝对坐标**（换显示器有可能落在外面），而
    /// [`sync_visibility`] 跨表面查标签，否则下一帧就会往主表面再补一份重复的控制台。
    #[test]
    fn a_detached_float_window_survives_the_round_trip() {
        let mut app = sample_app();
        draw_once(&mut app, NOMINAL_WIDTH, NOMINAL_HEIGHT);
        {
            let mut state = app.dock_state.take().expect("帧末必须放回 dock 树");
            let location = state.find_tab(&DockTab::Console).expect("默认树里有控制台");
            state.detach_tab(
                location,
                egui::Rect::from_min_size(egui::Pos2::new(120.0, 140.0), egui::vec2(420.0, 260.0)),
            );
            app.dock_state = Some(state);
        }
        draw_once(&mut app, NOMINAL_WIDTH, NOMINAL_HEIGHT);
        let state = app.dock_state.take().expect("帧末必须放回 dock 树");
        assert_eq!(state.surfaces_count(), 2, "浮动窗应是一个额外表面");

        let value = serialize(&state).expect("含浮动窗的树必须能序列化");
        let restored = deserialize(&value).expect("浮动窗必须读得回来");
        assert_eq!(restored.surfaces_count(), 2, "表面数量必须带走");
        assert_eq!(
            signature(&restored),
            signature(&state),
            "含浮动窗的指纹必须原样恢复"
        );

        let editor = sample_app();
        let mut moved = restored;
        sync_visibility(&mut moved, &editor);
        assert_eq!(
            moved
                .iter_all_tabs()
                .filter(|(_, tab)| **tab == DockTab::Console)
                .count(),
            1,
            "浮动窗里的控制台不得被重复补回主表面"
        );
        app.dock_state = Some(state);
    }

    /// 没画过的树**不能**落盘：节点矩形还是 `Rect::NOTHING`，里面的 ±∞ 经 `serde_json`
    /// 会变成 `null`，写进文件就再也读不回来。生产路径只在帧末存树，这条守住那个决定
    /// （宁可这次不存，也不写一个打不开的文件）。
    #[test]
    fn an_undrawn_tree_is_refused_rather_than_written_broken() {
        assert!(
            serialize(&default_dock_state()).is_none(),
            "未渲染的 dock 树不得序列化成可落盘 JSON"
        );
    }

    /// 变化检测的口径：矩形不进指纹（否则拖窗口边缘就是一次次写盘），结构必须进指纹
    /// （否则用户重排永远存不下来）。
    #[test]
    fn signature_excludes_rectangles_but_includes_structure() {
        let mut app = sample_app();
        draw_once(&mut app, NOMINAL_WIDTH, NOMINAL_HEIGHT);
        let mut state = app.dock_state.take().expect("帧末必须放回 dock 树");
        let before = signature(&state);
        // 只改矩形：模拟"窗口尺寸变了、别的都没动"。
        for node in state.main_surface_mut().iter_mut() {
            node.set_rect(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(123.0, 456.0),
            ));
        }
        assert_eq!(
            signature(&state),
            before,
            "矩形不得进指纹：`structure_of` = {:?}",
            structure_of(&state)
        );

        let location = state
            .find_tab(&DockTab::Resource)
            .expect("默认树里有资源面板");
        state.remove_tab(location);
        assert_ne!(
            signature(&state),
            before,
            "搬走一个面板必须改指纹，否则重排永远不会落盘"
        );
        app.dock_state = Some(state);
    }

    /// 为什么落盘要节流（`main.rs` 的 `DOCK_LAYOUT_SETTLE`）：egui_dock 每帧把每个拆分的
    /// `fraction` 重新夹紧到"两侧各留够 `separator.extra`（默认 175px）"的区间，于是
    /// **光缩窗口就会改布局**。这是库行为，不是我们存错了，指纹只能认它。
    #[test]
    fn shrinking_the_window_rewrites_split_fractions() {
        let mut wide = sample_app();
        draw_once(&mut wide, NOMINAL_WIDTH, NOMINAL_HEIGHT);
        let wide = wide.dock_state.take().expect("帧末必须放回 dock 树");
        let mut narrow = sample_app();
        draw_once(&mut narrow, 640.0, 360.0);
        let narrow = narrow.dock_state.take().expect("帧末必须放回 dock 树");

        let wide_fraction = root_fraction(&wide);
        let narrow_fraction = root_fraction(&narrow);
        assert!(
            narrow_fraction < wide_fraction,
            "窗口变小必须把控制台顶到最小面板尺寸：{wide_fraction} → {narrow_fraction}"
        );
        assert_ne!(
            signature(&wide),
            signature(&narrow),
            "上面那个结论要能反映到指纹上，落盘节流才不是白写的注释"
        );
    }

    /// 手改过的坏文件（同一标签出现两份）按损坏处理：[`sync_visibility`] 按"树里有没有"
    /// 决定补还是删，重复标签会让它判错。
    #[test]
    fn deserialize_rejects_a_tree_with_a_duplicate_tab() {
        let mut app = sample_app();
        draw_once(&mut app, NOMINAL_WIDTH, NOMINAL_HEIGHT);
        let state = app.dock_state.take().expect("帧末必须放回 dock 树");
        let mut value = serialize(&state).expect("画过一帧的树必须能序列化");
        let tabs = first_tabs_array_mut(&mut value).expect("序列化结果里应有 tabs 数组");
        let copy = tabs.first().expect("非空 tabs").clone();
        tabs.push(copy);
        assert!(
            deserialize(&value).is_none(),
            "重复标签的树必须整棵回落到默认布局"
        );
        app.dock_state = Some(state);
    }

    /// M27-03 验收 1 的自动化那一半：把 [`churn`] 跑满 **200 轮**，每轮都必须
    /// ① 真的改了结构（指纹变）、② 八个常驻视图不丢不重、③ 重排后的树仍可序列化落盘。
    ///
    /// 三条各自挡一种坏形态：只断言"没 panic"的话，负载静默空转也算过；
    /// 不查重复的话，浮动窗搬回来时多出一份控制台要等用户看见才知；
    /// 不查序列化的话，"重排出一个写不进 layout.json 的树"会在 300ms 后的落盘处才炸。
    ///
    /// 每轮之间都**画一帧**：新拆出来的浮动窗在被画之前节点矩形还是 `Rect::NOTHING`
    /// （±∞ 过 `serde_json` 会变 `null`，见 [`serialize`]），不画就断言序列化测的是
    /// "画没画过"而不是"重排排坏了"。画一帧也正好复现 soak 的真实节奏。
    #[test]
    fn two_hundred_dock_churns_rearrange_the_tree_every_round() {
        let mut app = sample_app();
        draw_once(&mut app, NOMINAL_WIDTH, NOMINAL_HEIGHT);
        let mut previous = {
            let state = app.dock_state.take().expect("帧末必须放回 dock 树");
            let signature = self::signature(&state);
            app.dock_state = Some(state);
            signature
        };
        for step in 0..200 {
            let note = {
                let mut state = app.dock_state.take().expect("帧末必须放回 dock 树");
                let note = churn(&mut state, step);
                app.dock_state = Some(state);
                note
            };
            assert!(!note.starts_with("跳过"), "step {step} 负载空转：{note}");
            draw_once(&mut app, NOMINAL_WIDTH, NOMINAL_HEIGHT);

            let state = app.dock_state.take().expect("帧末必须放回 dock 树");
            let current = self::signature(&state);
            assert_ne!(current, previous, "step {step}（{note}）没有改变任何结构");
            previous = current;

            let mut tabs: Vec<_> = state.iter_all_tabs().map(|(_, tab)| *tab).collect();
            tabs.sort_by_key(|tab| *tab as u8);
            let count = tabs.len();
            tabs.dedup();
            assert_eq!(count, 8, "step {step}（{note}）标签总数变成 {count}");
            assert_eq!(tabs.len(), 8, "step {step}（{note}）出现重复标签");
            assert!(
                serialize(&state).is_some(),
                "step {step}（{note}）重排后的树写不进 layout.json"
            );
            // 浮动窗只允许在"拆出去→并回来"这两步之间出现，飘着不回落就是负载写歪了。
            let floats = state.surfaces_count() - 1;
            assert!(floats <= 1, "step {step}（{note}）残留 {floats} 个浮动表面");
            app.dock_state = Some(state);
        }
    }
}
