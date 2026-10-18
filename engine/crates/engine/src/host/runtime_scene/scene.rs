//! `RuntimeScene` 的构造入口、`init`、资产与调试/预览访问器（WP12 阶段 3a 从 `mod.rs` 原样搬出）。
//!
//! 固有 `impl` 块不要求和类型定义在同一个模块（同 crate 即可），而私有字段的可见域是
//! 『定义它的模块及其所有后代』⇒ 字段属性零改动（计划 §3.1.1）。`init` 与两条 collider 调试
//! 线框方法被**留在 `mod.rs` 的每帧主路径**调用，方向是子→父，故标 `pub(super)`（§3.1.4）。
//! 结构体本身仍定义在 `mod.rs`——搬走它会逼着约 40 个字段逐个放宽可见性。

use crate::host::frame::{OverlayVertex, SceneDraw, VectorFontHandle};
use crate::host::gamepad::GamepadPoller;
use rustlua_assets::asset_handle::{AssetHandle, AssetRecord, AssetStore};
use rustlua_camera::camera::{Camera2D, OrthoTerms, Viewport};
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::hierarchy::{Parent, SpatialReport};
use rustlua_ecs::scheduler::Scheduler;
use rustlua_ecs::spatial_transform::LocalTransform;
use rustlua_ecs::tilemap::Tilemap;
use rustlua_ecs::transform::{Transform, Transform3D, Vec2};
use rustlua_infra::par::Parallelism;
use rustlua_physics::physics::PhysicsAuthority;
use rustlua_render::font::FontAtlas;
use rustlua_render::layer::RenderLayer;
use rustlua_render::sprite_batch::SpriteBatch;
use rustlua_scene::scene::{Scene, SceneEntityId};
use rustlua_scripting::script::LuaRuntime;
use rustlua_ui::ui::UI_REFERENCE_SIZE;
use rustlua_ui::ui_layout::{UiRect, UiRectLayout};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
    sync::{Arc, Mutex},
};

use super::{
    PhysicsStageOrder,
    RuntimeScene,
    TextRenderMode,
    build_ui_scene_nodes,
    // `mod.rs` 的 `use camera::{...}` 是私有导入，但私有导入对定义模块的所有后代可见，
    // 所以子模块按 `super::名字` 取用即可，不必再放宽 camera.rs 里那三条。
    declared_camera_2d_in,
    load_runtime_scene,
    project_text_render_mode,
    push_screen_line,
};

impl RuntimeScene {
    pub fn new(project_root: Option<&Path>) -> anyhow::Result<Self> {
        let scene = load_runtime_scene(project_root);
        Self::init(project_root, scene, true, false)
    }

    /// P3：显式声明"这是发布包"（`game --pack`）。决定存档与玩家设置写到哪里——
    /// 发布包 → 用户数据目录（`%APPDATA%\<app>`），开发工程 → 工程内。
    /// 不显式声明时 `user_dir::is_release_pack` 仍能凭 `<exe>/pack` 内 `project.toml` 的
    /// `release = true` 认出双击启动的发布包。
    pub fn new_with_layout(project_root: Option<&Path>, packaged: bool) -> anyhow::Result<Self> {
        let scene = load_runtime_scene(project_root);
        Self::init(project_root, scene, true, packaged)
    }

    /// 实例化一个具体场景（编辑器"运行当前场景"用），而非工程的启动场景。
    /// 编辑器处理的永远是开发工程：玩家设置留在工程根（面板直接编辑那份）。
    pub fn from_scene(project_root: Option<&Path>, scene: Scene) -> anyhow::Result<Self> {
        Self::init(project_root, scene, true, false)
    }

    /// Instantiates a scene WITHOUT running the project script, for the editor's
    /// scene view (render the scene as edited, no game logic).
    pub fn from_scene_edit(project_root: Option<&Path>, scene: Scene) -> anyhow::Result<Self> {
        Self::init(project_root, scene, false, false)
    }

    /// Lua 侧 `app.quit()` 只置引擎的退出请求标记（见 `script.rs`），桌面运行器
    /// 借此轮询并在帧末真正退出，否则“退出”按钮在窗口环境无效。
    pub fn quit_requested(&self) -> bool {
        self.script.quit_requested()
    }

    /// 取走并清零"上次取走以来 Lua `update()` 的累计耗时"（微秒）。
    ///
    /// 宿主每次跑完固定步后调用一次，喂给 `PerformanceStats::lua_ms`。用"取走即清零"
    /// 而不是查询，是为了让 0 步帧（帧率高于逻辑步时）自然得到 0，不会把上一帧的
    /// Lua 耗时重复计入。
    pub fn take_script_micros(&mut self) -> u64 {
        std::mem::take(&mut self.script_micros)
    }

    /// docs/54 §4.4：当前登记在身份表里的运行时资源（按句柄排序）。
    ///
    /// 这是**跨帧**的表（不像 `texture_paths` 每帧重建），因此可以直接断言"同一路径恒同句柄"。
    /// 返回克隆而不是引用：身份表在 `RefCell` 里（3D 提取是 `&self`，也要能登记网格身份），
    /// 借用作用域没法延长到调用方。
    #[must_use]
    pub fn asset_entries(&self) -> Vec<AssetRecord> {
        self.assets
            .borrow()
            .entries()
            .into_iter()
            .map(|(_, record)| record.clone())
            .collect()
    }

    /// 运行时资源身份表里的活条目数（不克隆整张表；性能面板常驻读数用）。
    #[must_use]
    pub fn asset_count(&self) -> usize {
        self.assets.borrow().len()
    }

    /// 场景声明的实体数（不含脚本运行期 spawn 的实体）。
    #[must_use]
    pub fn scene_entity_count(&self) -> usize {
        self.script.scene_entity_count()
    }

    /// 运行时 ECS 世界里的实体总数（含脚本运行期 spawn 的实体）。
    #[must_use]
    pub fn runtime_entity_count(&self) -> usize {
        self.script.with_world(|world| world.entity_count())
    }

    /// docs/54 §4.3：本帧三种显式显示模式各路由了几个实体 `(world, overlay, screen)`。
    ///
    /// 未声明模式的实体不计数（它们跟随全局开关），因此这三个数只在场景真的用了显式模式时非零——
    /// 也就是"这段几何走哪条路"变成可断言的事实。
    #[must_use]
    pub const fn render_mode_routed(&self) -> (usize, usize, usize) {
        self.render_mode_routed
    }

    /// docs/54 §4.3：加载期点名的"多种可绘制组件"冲突。
    #[must_use]
    pub fn renderable_conflicts(&self) -> Vec<String> {
        self.script.scene_renderable_conflicts()
    }

    /// 某个路径当前的运行时句柄（未登记则 `None`）。
    #[must_use]
    pub fn asset_handle(&self, path: &str) -> Option<AssetHandle> {
        self.assets.borrow().handle_of(path)
    }

    /// WP7 ②：collider 调试绘制开关。默认关闭；Lua 可用 `physics.debug_draw(true)` 打开。
    pub fn set_collider_debug_draw(&mut self, enabled: bool) {
        self.collider_debug_draw = enabled;
    }

    /// 当前 collider 调试绘制开关。
    #[must_use]
    pub const fn collider_debug_draw(&self) -> bool {
        self.collider_debug_draw
    }

    /// docs/54 §8-⑥：3D collider 调试绘制开关。默认关闭；Lua 用
    /// `physics3d.debug_draw(true)` 打开，编辑器/验收也可以直接设。
    pub fn set_collider_debug_draw_3d(&mut self, enabled: bool) {
        self.collider_debug_draw_3d = enabled;
    }

    /// 当前 3D collider 调试绘制开关。
    #[must_use]
    pub const fn collider_debug_draw_3d(&self) -> bool {
        self.collider_debug_draw_3d
    }

    /// 把 collider 调试绘制追加到 overlay 缓冲（WP7 ②）。
    ///
    /// 数据源是**物理世界当前姿态**（`PhysicsWorld::debug_shapes`），不是场景里的组件串：
    /// 组件只在实例化时解析一次，而动态刚体每步都在动，调试绘制要看的正是后者。
    /// 动态体画绿、固定体画青、sensor 画黄——三者物理响应完全不同，颜色不分辨会误判。
    ///
    /// 全部用白色纹理，因此只追加**一个** draw range；画在世界内容之上、屏幕 UI 之下。
    pub(super) fn push_collider_debug(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        viewport: Viewport,
    ) {
        if !self.collider_debug_draw {
            return;
        }
        let shapes = self.script.collider_debug_shapes();
        if shapes.is_empty() {
            return;
        }
        // 预算项每帧一次：逐段投影因此不再校验相机、不再算三角函数。
        let Ok(camera) = OrthoTerms::new(self.camera, viewport) else {
            return;
        };
        let start = vertices.len();
        for shape in shapes {
            // 传感器黄、动态体绿、固定体青：三种响应**不是**同一件事，画成一个颜色会让
            // "物理到底动了什么"看不出来（docs/54 §8-⑥ 的调试契约）。
            let color = if shape.sensor {
                [1.0, 0.85, 0.2, 1.0]
            } else if shape.dynamic {
                [0.2, 1.0, 0.35, 1.0]
            } else {
                [0.25, 0.75, 1.0, 1.0]
            };
            // 轮廓本身由 `ColliderShape::wireframe_edges` 给出（局部坐标，2D/3D 同一套语义），
            // 这里只做姿态变换与投影。折线首尾相接，再逐段投影成细四边形。
            for (from, to) in shape.wireframe_segments() {
                // 非有限端点跳过该段（与旧的"投影失败即放弃"同一语义）。
                if !from.x.is_finite()
                    || !from.y.is_finite()
                    || !to.x.is_finite()
                    || !to.y.is_finite()
                {
                    continue;
                }
                let from = camera.world_to_screen(from);
                let to = camera.world_to_screen(to);
                push_screen_line(vertices, [from.x, from.y], [to.x, to.y], color, viewport);
            }
        }
        let count = vertices.len() - start;
        if count > 0 {
            draws.push(SceneDraw {
                texture: 0, // 0 = 内置白色纹理
                start: start as u32,
                count: count as u32,
                sdf: false,
                bitmap: false,
                vector: false,
                render_layer: RenderLayer::Overlay2D,
            });
        }
    }

    /// docs/54 §8-⑥：把 **3D** collider 调试线框追加到 overlay 缓冲。
    ///
    /// 与 2D 那条的唯一区别是投影：2D 用 `Camera2D::world_to_screen`，这里用本帧 3D pass
    /// 实际用的那台相机（`active_camera3d`）+ `picking::screen_from_world_unclipped`——
    /// 同一份投影数学，因此"看到的位置"和"射线命中的位置"必然一致（§8-⑦ 的要求）。
    /// 线框的边由 `ColliderShape3d::wireframe_edges` 给出（段数固定，可断言）。
    pub(super) fn push_collider_debug_3d(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        viewport: Viewport,
    ) {
        if !self.collider_debug_draw_3d {
            return;
        }
        let shapes = self.script.collider_debug_shapes_3d();
        if shapes.is_empty() {
            return;
        }
        // 每帧烘焙一次 3D 视图：逐段投影因此不再重建矩阵（此前每个端点都会重建一次
        // `view_projection_matrix`，等于每段两次 4×4 乘法加两次校验）。
        // 拆分批 8：同帧同视口直接吃 `vertices()` 定帧的那一份（不再多解析一次相机）。
        let Some(view) = self.frame_camera_view(viewport) else {
            return;
        };
        let start = vertices.len();
        for shape in shapes {
            let color = if shape.sensor {
                [1.0, 0.85, 0.2, 1.0]
            } else if shape.dynamic {
                [0.2, 1.0, 0.35, 1.0]
            } else {
                [0.25, 0.75, 1.0, 1.0]
            };
            for (from, to) in shape.wireframe_segments() {
                let (Some(from), Some(to)) = (
                    rustlua_render::picking::screen_from_view_unclipped(view, from, viewport),
                    rustlua_render::picking::screen_from_view_unclipped(view, to, viewport),
                ) else {
                    continue; // 相机背后/远裁剪面之外：不画，也不猜
                };
                push_screen_line(vertices, [from.x, from.y], [to.x, to.y], color, viewport);
            }
        }
        let count = vertices.len() - start;
        if count > 0 {
            draws.push(SceneDraw {
                texture: 0, // 0 = 内置白色纹理
                start: start as u32,
                count: count as u32,
                sdf: false,
                bitmap: false,
                vector: false,
                render_layer: RenderLayer::Overlay2D,
            });
        }
    }

    /// Updates one authored entity's 2D transform in-place for editor drag
    /// preview. Avoids rebuilding Lua/ECS for every mouse-move revision.
    pub fn set_scene_entity_transform_preview(
        &mut self,
        id: SceneEntityId,
        position: Vec2,
        rotation: f32,
        scale: Vec2,
    ) -> bool {
        let Some(handle) = self.script.scene_entity_handle(id) else {
            return false;
        };
        let mut updated = false;
        self.script.with_world_mut(|world| {
            if let Ok(Some(transform)) = world.get_mut::<Transform>(handle) {
                transform.position = position;
                transform.rotation = rotation;
                transform.scale = scale;
                updated = true;
            }
        });
        if updated {
            // 拆分批 8：拖拽预览改的是世界姿态——`pose=transform` 的相机可能就在被拖的父链上，
            // 因此本帧已定帧的 3D 视图作废（编辑器每帧重渲染，下一次消费会重新定帧）。
            self.invalidate_frame_view_3d();
        }
        updated
    }

    /// 宿主侧写入一个瓦片格（WP4 ④）：编辑器画笔的实时预览与基准都要能在**不重建整个运行时**
    /// 的前提下改一格。走的是与 Lua `tilemap.set_cell` 同一个 `Tilemap::set_cell`，因此校验
    /// 与边界规则只有一份。返回 `false` 表示该实体不存在或没有 `Tilemap` 组件。
    pub fn set_tilemap_cell(
        &mut self,
        id: SceneEntityId,
        x: usize,
        y: usize,
        value: u8,
    ) -> anyhow::Result<bool> {
        let Some(handle) = self.script.scene_entity_handle(id) else {
            return Ok(false);
        };
        let mut updated = false;
        let mut failure: Option<anyhow::Error> = None;
        self.script
            .with_world_mut(|world| match world.get_mut::<Tilemap>(handle) {
                Ok(Some(tilemap)) => match tilemap.set_cell(x, y, value) {
                    Ok(()) => updated = true,
                    Err(error) => failure = Some(error.into()),
                },
                Ok(None) => {}
                Err(error) => failure = Some(error.into()),
            });
        match failure {
            Some(message) => Err(message),
            None => Ok(updated),
        }
    }

    /// Updates one authored UI entity's four-edge offsets in-place for editor
    /// drag preview. The document is only committed when the drag ends.
    pub fn set_scene_entity_ui_layout_preview(
        &mut self,
        id: SceneEntityId,
        offsets: [f32; 4],
    ) -> bool {
        let Some(handle) = self.script.scene_entity_handle(id) else {
            return false;
        };
        let mut updated = false;
        self.script.with_world_mut(|world| {
            if let Ok(Some(layout)) = world.get_mut::<UiRectLayout>(handle) {
                layout.offsets = offsets;
                updated = true;
            }
        });
        if updated {
            // Runtime UI overlay resolves from this snapshot, not the ECS copy.
            // `ui_scene_nodes` is keyed by visible entity order, while `id` is
            // the sparse authoring ID. The live ECS slot index is that order.
            let layout_key = handle.index() as u64;
            if let Some(node) = self.ui_scene_nodes.get_mut(&layout_key)
                && let Some(layout) = node.layout.as_mut()
            {
                layout.offsets = offsets;
            }
            self.ui_layout_rects.clear();
        }
        updated
    }

    /// Returns exact resolved UI rects by authored entity ID. These are the
    /// same rects consumed by the next game-screen render, so editor selection
    /// geometry and rendering share one layout resolution.
    pub fn scene_entity_ui_layout_rects(
        &mut self,
        ids: impl IntoIterator<Item = SceneEntityId>,
        viewport: [f32; 2],
    ) -> BTreeMap<u64, UiRect> {
        self.ensure_scene_ui_layout(viewport);
        ids.into_iter()
            .filter_map(|id| {
                let handle = self.script.scene_entity_handle(id)?;
                self.ui_layout_rects
                    .get(&(handle.index() as u64))
                    .map(|node| (id.0, node.rect))
            })
            .collect()
    }

    /// 运行期 `prefab.spawn` 实例**本帧真正用于绘制**的屏幕矩形，按实体名给出。
    ///
    /// 这就是「预设真的画出来了」的可断言读数：`push_scene_ui` 画 quad 用的正是同一批
    /// `ui_layout_rects`，所以这道读数不是第二套布局计算，而是将渲染消费的那份暴露出来。
    /// 名字来自脚本侧的 spawn 登记表（槽位号还原不出 `EntityId`，缺 generation）。
    pub fn prefab_ui_rects(&mut self, viewport: [f32; 2]) -> Vec<(String, UiRect)> {
        self.ensure_scene_ui_layout(viewport);
        let names = self.script.prefab_spawn_names();
        names
            .iter()
            .filter_map(|(key, name)| {
                self.ui_layout_rects
                    .get(key)
                    .map(|node| (name.clone(), node.rect))
            })
            .collect()
    }

    /// 本帧真正用于绘制的 UI 矩形，按**实体名**给出。
    ///
    /// 名字来自脚本侧那张全局登记表（`entity.find` 用的同一份），所以场景里的实体与运行期
    /// `prefab.spawn` 出来的实例一律给名——迁移前后载体变了、名字没变，同一份黄金表就能逐 id
    /// 对账。没有 `UiLayout` 的 UI 实体不参与解算，因此也不出现在这里（与"不会被画"同口径）。
    pub fn ui_rects_by_name(&mut self, viewport: [f32; 2]) -> BTreeMap<String, UiRect> {
        self.ensure_scene_ui_layout(viewport);
        let names = self.script.entity_names_by_slot();
        names
            .iter()
            .filter_map(|(key, name)| {
                self.ui_layout_rects
                    .get(key)
                    .map(|node| (name.clone(), node.rect))
            })
            .collect()
    }

    /// 「场景实体 id → 句柄编码」登记表（见 [`Self::runtime_spawned_entities`] 的父边）。
    ///
    /// 运行期实体的父可能是文档里的行，宿主拿这张表把句柄反解成镜像里的行号。
    pub fn scene_entity_handles(&self) -> Vec<(SceneEntityId, u64)> {
        self.script
            .scene_entity_handles()
            .into_iter()
            .map(|(id, handle)| (id, encode_handle(handle)))
            .collect()
    }

    /// 活世界里**登记过名字的实体名**（声明的与脚本生成的都算），排序去重。
    ///
    /// 为什么不是 [`Self::runtime_spawned_entities`] 的名字：那张表是编辑器镜像用的，刻意**排除**
    /// 场景声明的实体（它们在文档行里已有身份）。而"世界读数"问的是反过来的事——主角通常是文档里
    /// 声明的那一位（工程运行时换图后照样按登记名 `entity.find("…")` 找它），排除声明实体等于
    /// 把主角筛掉。判据两边共用同一套名字表，与 Lua 侧 `entity.find` 的命名空间同源。
    pub fn named_entity_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .script
            .named_entity_handles()
            .into_iter()
            .map(|(_, name)| name)
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// 声明实体（场景文档里的行）**此刻在运行时里的局部变换**，按文档 id 给出。
    ///
    /// 只列「登记过句柄且仍活着」的实体，一次世界借用读完，供编辑器 Play 期间逐行回灌
    /// 镜像文档。两种"不在表里"含义不同，宿主用 [`Self::scene_entity_handles`] 区分：
    ///
    /// - 登记过句柄却不在这里 ⇒ 被脚本 `entity.destroy` 销毁了，镜像里那一行该摘掉；
    /// - 从没登记过句柄 ⇒ 编辑期新增、还没实例化过，那是编辑态自己的行，不该动。
    pub fn scene_entity_local_transforms(&self) -> Vec<(SceneEntityId, LocalTransform)> {
        let registered = self.script.scene_entity_handles();
        self.script.with_world(|world| {
            registered
                .into_iter()
                .filter(|(_, handle)| world.is_alive(*handle))
                .filter_map(|(id, handle)| {
                    super::spatial::local_transform_in(world, handle).map(|local| (id, local))
                })
                .collect()
        })
    }

    /// 运行期由脚本生成、**不属于场景文档**的实体清单（编辑器 Play 期间的层级同步用）。
    ///
    /// 候选取「登记过名字的 ∪ 带 2D/3D 变换的」：`entity.spawn` 不传名字时没有名字，
    /// 但它一定带 `Transform`；反过来只有组件的特殊实体也能靠名字进表。
    /// 声明实体不在此列——镜像里已经有它们的行，重复列一遍会变成两行。
    ///
    /// 姿态取实体**自己的** `Transform`/`Transform3D` 分量（局部值），与 `.scene` 文档里
    /// `Transform` 组件串的口径一致——镜像回灌是逐行原样写回，不做世界→局部反解；父链组合
    /// 交给编辑运行时按文档自己的空间语义再算一遍，两侧因此不会各解一次而错位。
    ///
    /// `ui` 标出带 UI 组件的那些：它们的**版面在组件里而不在姿态里**（`UiLayout` 的
    /// anchors/offsets、`Text` 的锚点），而编辑器镜像只回灌姿态与结构，因此画布上认不出
    /// 它们的框——这一位就是给编辑器"别拿姿态画一个假框"用的判据（见 docs/56 §3.53）。
    ///
    /// 顺序按句柄升序（槽位先、代际后），同一份世界两次调用给出同一份表。
    pub fn runtime_spawned_entities(&self) -> Vec<RuntimeSpawnedEntity> {
        let named = self.script.named_entity_handles();
        let declared: HashSet<u64> = self
            .script
            .scene_entity_handles()
            .into_iter()
            .map(|(_, handle)| encode_handle(handle))
            .collect();
        let names: HashMap<u64, String> = named
            .iter()
            .map(|(handle, name)| (encode_handle(*handle), name.clone()))
            .collect();
        let mut out = Vec::new();
        self.script.with_world(|world| {
            // 「这一位是 UI 实体」的判据取自**活世界里的组件类型**，不是名字或路径的启发式：
            // 界面实体的版面数据只存在组件里，镜像认不出它，标注因此必须来自同一个世界。
            let mut ui_handles: HashSet<u64> = HashSet::new();
            for handle in world.entities_with::<UiRectLayout>() {
                ui_handles.insert(encode_handle(handle));
            }
            for handle in world.entities_with::<rustlua_ui::ui::UiImage>() {
                ui_handles.insert(encode_handle(handle));
            }
            for handle in world.entities_with::<rustlua_ui::ui::UiText>() {
                ui_handles.insert(encode_handle(handle));
            }
            let mut candidates: Vec<EntityId> = named.iter().map(|(handle, _)| *handle).collect();
            candidates.extend(world.entities_with::<Transform>());
            candidates.extend(world.entities_with::<Transform3D>());
            candidates.sort_unstable();
            candidates.dedup();
            for handle in candidates {
                let key = encode_handle(handle);
                if !world.is_alive(handle) || declared.contains(&key) {
                    continue;
                }
                let name = names
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| format!("entity.{}", handle.index()));
                let (position, rotation, scale) =
                    match super::spatial::local_transform_in(world, handle) {
                        Some(local) => (
                            [
                                local.translation.x,
                                local.translation.y,
                                local.translation.z,
                            ],
                            [
                                local.rotation.x,
                                local.rotation.y,
                                local.rotation.z,
                                local.rotation.w,
                            ],
                            [local.scale.x, local.scale.y, local.scale.z],
                        ),
                        None => ([0.0; 3], [0.0, 0.0, 0.0, 1.0], [1.0; 3]),
                    };
                let parent = match world.get::<Parent>(handle) {
                    Ok(Some(parent)) => Some(encode_handle(parent.0)),
                    _ => None,
                };
                out.push(RuntimeSpawnedEntity {
                    handle: key,
                    name,
                    parent,
                    ui: ui_handles.contains(&key),
                    position,
                    rotation,
                    scale,
                });
            }
        });
        out
    }

    /// 覆盖 project.toml 的文本渲染模式（编辑器保存设置后重建运行时可注入）。
    pub fn set_text_render_mode(&mut self, mode: TextRenderMode) {
        self.text_render_mode = mode;
    }

    /// 注入共享字体图集（编辑器在运行时重建时复用，避免每帧重新生成 MSDF 字形）。
    pub fn set_font_atlas(&mut self, font_atlas: Option<Arc<Mutex<FontAtlas>>>) {
        self.font_atlas = font_atlas;
    }

    /// 注入共享默认矢量轮廓图集（编辑器跨运行时重建复用）。
    pub fn set_vector_atlas(&mut self, vector_atlas: Option<VectorFontHandle>) {
        self.vector_atlas = vector_atlas;
    }

    pub(super) fn init(
        project_root: Option<&Path>,
        scene: Scene,
        run_script: bool,
        packaged: bool,
    ) -> anyhow::Result<Self> {
        // docs/54 §4.1：`parent` 的默认语义由**场景自身**声明，而不是由宿主猜。未声明 ⇒ 组织
        // 关系（v2 历史语义），`spatial=local` ⇒ 父子参与空间组合（迁移后的场景）。宿主仍可用
        // `set_spatial_inheritance` 覆盖这次判断（回滚档）。
        let spatial_mode = rustlua_scene::scene_space::spatial_mode(&scene);
        if let Some(issue) = rustlua_scene::scene_space::spatial_mode_issue(&scene) {
            tracing::warn!(target: "scene", scene = %scene.name, "{issue}");
        }
        let mut script = LuaRuntime::new()?;
        // Project root must be set before instantiation so scene components that
        // load assets (e.g. Animation3D) can resolve their mesh paths.
        if let Some(root) = project_root {
            script.set_project_root(root.to_path_buf());
        }
        let instantiated = if run_script {
            script.instantiate_scene(&scene)?
        } else {
            script.instantiate_scene_with_editor_visibility(&scene, true)?
        };
        tracing::info!(target: "scene", "[scene] loaded name={} entities={}", scene.name, scene.entities.len());
        tracing::info!(target: "ecs", "[ecs] world initialized entities={instantiated}");
        if run_script && let Some(root) = project_root {
            script.set_audio_asset_dir(root.join("assets").join("audio"));
            // P3 用户数据目录：发布包（`<exe>/pack` + `release.toml`，或 `--pack`）
            // 把存档与玩家设置写到用户目录（`%APPDATA%\<app>`），不写程序目录；
            // 开发工程保持工程内 `.cache/save.sav` 与工程根 `settings.toml`。
            let roots = rustlua_infra::user_dir::data_roots(root, packaged);
            if let Err(error) = rustlua_infra::user_dir::ensure_save_dir(&roots) {
                tracing::warn!(target: "save", "{}", error);
            }
            if roots.packaged {
                script.set_settings_root(roots.settings_dir.clone());
                match rustlua_infra::user_dir::seed_settings(&roots, root) {
                    Ok(true) => tracing::info!(
                        target: "app",
                        path = %roots.settings_file().display(),
                        "[app] player settings seeded from the release package"
                    ),
                    Ok(false) => {}
                    Err(error) => tracing::warn!(target: "app", "{}", error),
                }
                tracing::info!(
                    target: "save",
                    save = %roots.save_file().display(),
                    settings = %roots.settings_file().display(),
                    app = %roots.app_name,
                    "[save] release layout: user data directory in use"
                );
            }
            script.set_save_path(roots.save_file());
            let script_path = root.join("scripts/main.lua");
            // 存在性判断要**先看文件系统，再看 vfs**：Web/微信上发布包在宿主的资源表里，
            // `Path::is_file()`（std::fs）恒为 false，于是"跑工程脚本"会静默不发生——
            // 表现成"脚本没生效"，实则是这里把门关着。判两次比"哪个平台用哪个"更稳：
            // 桌面走前半句就短路，Web 走后半句，两边判定一致。
            let script_exists =
                script_path.is_file() || rustlua_infra::vfs::read(&script_path).is_ok();
            if script_exists {
                script.execute_project_script(&script_path)?;
            } else {
                tracing::info!(target: "script", path = %script_path.display(), "[script] game project has no scripts/main.lua; script runtime idle");
            }
        }
        let text_render_mode = project_text_render_mode(project_root);
        // docs/54 §8-①：场景声明了自己的 `Camera2D`（正交相机）就用它当**初始**相机；
        // 没声明则保持历史默认（中心 = 设计分辨率中心、zoom = 1）。之后 Lua `camera2d.*`
        // 与相机控制器照旧移动它——声明的相机是起点，不是第二份姿态真源。
        let declared_camera = script.with_world(declared_camera_2d_in);
        let camera = match declared_camera {
            Some(camera) => camera,
            None => Camera2D::new(rustlua_camera::camera::DEFAULT_2D_CAMERA_CENTER, 1.0)?,
        };
        if declared_camera.is_some() {
            tracing::info!(
                target: "render",
                center = ?camera.center,
                zoom = camera.zoom,
                "[render] 2D camera comes from the scene's Camera2D component"
            );
        }
        tracing::info!(target: "render", "[render] SpriteBatch and Camera2D initialized");
        // 字体图集不再在此无条件加载：`load_project_font_atlas` 会按 chars.txt 预热
        // 全部 MSDF 字形，debug 构建下约 2s/项目（实测 ui_demo 135 字 ≈ 2.3s）。
        // 编辑器场景视图/预览通过 `set_font_atlas` 注入共享（已预热）图集；未注入
        // 时由 `ensure_font_atlas` 在首次排版文本时按需加载。这样场景编辑（revision
        // 变化 → 运行时重建）不再每次重建都白白预热一遍被丢弃的图集。
        let ui_scene_nodes = build_ui_scene_nodes(&scene, !run_script);
        let mut scene = Self {
            script,
            script_micros: 0,
            sprite_batch: SpriteBatch::new(),
            sprite_parallelism: Parallelism::detected(),
            system_parallelism: Parallelism::detected(),
            spatial_index: false,
            spatial_grid: None,
            spatial_index_fingerprint: None,
            spatial_index_cell_size: 256.0,
            collider_debug_draw: false,
            collider_debug_draw_3d: false,
            view_world_mask: rustlua_render::visibility::ALL_LAYERS,
            view_overlay_mask: rustlua_render::visibility::ALL_LAYERS,
            view_filtered: std::cell::Cell::new(0),
            tilemap_culling: true,
            sprite_culling: true,
            camera,
            frame_view_3d: None,
            adopted_camera_2d_source: None,
            viewport_width: 1280,
            viewport_height: 720,
            ui_reference: UI_REFERENCE_SIZE,
            render_ui_overlay: true,
            project_root: project_root.map(|root| root.to_path_buf()),
            texture_dims: HashMap::new(),
            texture_paths: Vec::new(),
            texture_lookup: HashMap::new(),
            assets: std::cell::RefCell::new(AssetStore::new()),
            texture_handles: Vec::new(),
            overlay_batch: SpriteBatch::with_capacity(16),
            screen_batch: SpriteBatch::with_capacity(16),
            render_mode_routed: (0, 0, 0),
            tilemap_cache: HashMap::new(),
            font_atlas: None,
            vector_atlas: None,
            custom_vector_atlases: Arc::new(Mutex::new(HashMap::new())),
            // 调试插桩：首次 vector text layout 日志状态。
            last_font_version: u64::MAX,
            custom_font_atlases: Arc::new(Mutex::new(HashMap::new())),
            custom_font_versions: HashMap::new(),
            frame_font_keys: Vec::new(),
            missing_fonts: HashSet::new(),
            missing_vector_fonts: HashSet::new(),
            text_render_mode,
            ui_scene_nodes,
            ui_layout_rects: BTreeMap::new(),
            applied_prefab_ui_nodes: BTreeMap::new(),
            applied_prefab_ui_revision: 0,
            step_scheduler: Scheduler::new(),
            frame_scheduler: Scheduler::new(),
            step_system_names: Vec::new(),
            frame_system_names: Vec::new(),
            pending_step_input: None,
            last_schedule: None,
            gamepad: GamepadPoller::platform_default(),
            // WP2 裁定：`parent` 一律按局部空间解释。默认打开；没有 `Parent` 的场景
            // （现有全部示例场景）不产生任何传播成本，回滚档是 `set_spatial_inheritance(false)`。
            spatial_inheritance: spatial_mode.inherits(),
            spatial_mode,
            spatial_fingerprint: None,
            spatial_skip_unchanged: true,
            spatial_skipped_frames: 0,
            spatial_incremental: rustlua_ecs::hierarchy::SpatialIncremental::default(),
            spatial_incremental_enabled: true,
            last_spatial_path: rustlua_ecs::hierarchy::SpatialPath::Full,
            last_spatial_recomputed: 0,
            spatial_path_counts: [0; 3],
            spatial_recomputed_total: 0,
            last_spatial_report: SpatialReport::default(),
            spatial_active: false,
            world_sprite_depth: false,
            physics_authority: PhysicsAuthority::default(),
            physics_stage_order: PhysicsStageOrder::default(),
            last_scene_emission: 0,
            last_physics_sync: (0, 0),
            last_entity_script_calls: 0,
        };
        scene.register_builtin_frame_systems();
        Ok(scene)
    }
}

/// ECS 句柄的稳定编码：高 32 位是槽位、低 32 位是代际。
///
/// 编辑器要把它当字典键用（跨帧认出"还是同一个运行期实体"），而 `EntityId` 本身不是
/// 宿主 API 的一部分，因此这里只交出编码后的整数。`pub(crate)`：测试用同一个编码器算
/// 期望值，避免断言与生产各写一份编码。
pub(crate) fn encode_handle(handle: EntityId) -> u64 {
    (u64::from(handle.index()) << 32) | u64::from(handle.generation())
}

/// 一条运行期生成实体的快照（见 [`RuntimeScene::runtime_spawned_entities`]）。
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeSpawnedEntity {
    /// 句柄编码，跨帧稳定；槽位被复用时代际不同，因此不会认错人。
    pub handle: u64,
    /// 登记名（`entity.spawn(name, ...)` / prefab 实例名）；没登记时是 `entity.<槽位>`。
    pub name: String,
    /// 父实体的句柄编码。父可能是场景声明的实体——宿主用「声明实体句柄表」把它对回文档行。
    pub parent: Option<u64>,
    /// 这个实体带 UI 组件（`UiLayout`/`Image`/`Text`）吗——即它的版面在**组件里**而不在姿态里。
    ///
    /// 编辑器镜像只回灌结构 + 姿态，因此这一位为真时画布上算不出它的矩形；消费方
    /// （`editor` 的 Scene 视图）据此**不画**一个姿态位置的假框，把"它在哪儿"交给
    /// 设计分辨率框里那张真游戏画面。
    pub ui: bool,
    /// 实体**自己**的局部姿态（`position=..;rotation=..;scale=..` 那一套，见
    /// [`rustlua_ecs::spatial_transform::TRANSFORM_COMPONENT`]：2D/3D 在文档里共用这一个组件名）。
    pub position: [f32; 3],
    /// 四元数 `x,y,z,w`。
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}
