//! 每帧绘制数据的产出：`vertices()` 与帧级 3D 视图（3D 相机/光源的**定帧**解析）、
//! 生效相机、自定义字体图集快照与 particles（WP12 阶段 3b 从 `mod.rs` 原样搬出）。
//!
//! 为什么这一堆要待在一个文件里：`frame_view_3d`/`active_camera3d`/`active_light`/`vertices`
//! 共用"同一帧只解析一次相机"这份约定（见下面 `frame_view_3d` 的注释）——拆到两个模块，
//! 这个不变量就跨文件了，正是本次拆分要避免的方向。
//! 每帧主路径的调用方（`mod.rs` 与兄弟子模块）用的私有方法标 `pub(super)`（计划 §3.1.4）。

use crate::host::frame::{
    BoundsSource, CustomFontAtlasPixels, FontAtlasPixels, OverlayVertex, Scene3d, Scene3dBounds,
    Scene3dDrawable, SceneDraw, SceneMesh, WHITE_TEXTURE_PATH, WorldSpriteVertex,
};
use rustlua_animation::animation::AnimationStateMachine;
use rustlua_animation::animation3d::Animation3D;
use rustlua_assets::asset_handle::AssetRecord;
use rustlua_camera::camera::Camera3D;
use rustlua_camera::camera::{CameraSelection, CameraView, DEFAULT_3D_CLEAR, RenderView, Viewport};
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::hierarchy::GlobalTransform;
use rustlua_ecs::transform::Transform3D;
use rustlua_ecs::transform::{Transform, Vec2};
use rustlua_math::math3d::{Mat4, Vec3};
use rustlua_render::layer::RenderLayer;
use rustlua_render::light::Light;
use rustlua_render::mesh3d::MeshRenderer;
use rustlua_render::quad::Quad;
use rustlua_render::sprite::{Sprite, SpriteAsset, SpriteSorting};
use rustlua_render::sprite_batch::{SpriteBatchItem, SpriteDrawBatch};
use rustlua_scene::scene::SceneEntityId;
use std::collections::HashSet;

use super::{FrameView3d, RuntimeScene, merge_adjacent_draws, texture_id_of};
use crate::host::runtime_scene::tilemap::sort_world_ranges;

impl RuntimeScene {
    /// Gathers the per-frame 3D render state from the runtime world: the active
    /// camera (Lua-selected, else the first `Camera3D` entity, else a default),
    /// the active light, and every entity with both a `Transform3D` and a
    /// `MeshRenderer`. Returns `None` when the scene has no 3D drawables.
    ///
    /// 视口是**唯一**的宽高比来源（相机拆分批 7）：这里不再有任何"把比例写回相机"的动作，
    /// 组件载荷与场景里写的一字不差。
    pub fn scene_3d(&self, viewport: Viewport) -> Option<Scene3d> {
        self.scene_3d_with_camera(viewport, None)
    }

    /// 带可选相机覆盖的 3D 提取（M11-04 / R-05）：编辑器 Scene 3D 视图与运行时
    /// 共用同一条 ECS drawable/component 提取路径（单一事实来源），编辑器传入
    /// 视图导航相机作为覆盖（不改场景数据），运行时传入 `None` 走
    /// `active_camera3d`（Lua 选择 / 首个 Camera3D / 默认）。
    ///
    /// 覆盖相机同样**不被改写**：视口比例只进 [`Scene3d::view`]（烘焙结果），不进相机载荷。
    pub fn scene_3d_with_camera(
        &self,
        viewport: Viewport,
        camera_override: Option<Camera3D>,
    ) -> Option<Scene3d> {
        // Runtime extraction intentionally reads ECS state so physics, scripts,
        // active camera/light selection, and animation palettes remain live.
        // docs/54 §4.2：3D 网格同样吃**世界视图**的可见遮罩。
        let view_mask = self.view_world_mask;
        let mut filtered = 0usize;
        // P1-②：LOD 距离用相机位置。优先级：编辑器覆盖相机 → 本帧已定帧相机。
        // 刻意不在这里兜底 `active_camera3d()`：那是一次解析（有账本断言钉住解析次数）；
        // 没有覆盖/定帧时 LOD 退基础级（`renderer.mesh`），行为与未启用 LOD 逐字节相同。
        let lod_camera_position: Option<Vec3> =
            camera_override.map(|camera| camera.position).or_else(|| {
                self.frame_view_3d
                    .as_ref()
                    .map(|frame| frame.camera.position)
            });
        let drawables = self.script.with_world(|world| {
            let mut drawables = Vec::new();
            // WP2：与 2D 精灵同一条约定——开关每帧选定一次遍历用的组件（开启继承时是
            // `WorldTransform3D`，此时每个有 `Transform3D` 的实体都已拿到世界变换），
            // 因此不必逐实体"先试世界再退回局部"而多付一次 `get`。
            let mut collect = |id: EntityId, model: Mat4, renderer: &MeshRenderer| {
                if !rustlua_render::visibility::entity_visible_in(world, id, view_mask) {
                    filtered += 1;
                    return;
                }
                let name = self
                    .script
                    .entity_name(id)
                    .unwrap_or_else(|| format!("Entity {}", id.index()));
                let palette = world
                    .get::<Animation3D>(id)
                    .ok()
                    .flatten()
                    .map(|animation| animation.joint_matrices())
                    .unwrap_or_default();
                let bounds = if renderer.mesh == rustlua_render::mesh3d::GROUND_PLANE_SENTINEL {
                    Scene3dBounds {
                        local_aabb: rustlua_math::math3d::Aabb3::new(
                            Vec3::new(-100.0, -0.05, -100.0),
                            Vec3::new(100.0, 0.05, 100.0),
                        )
                        .unwrap_or_else(|_| rustlua_math::math3d::Aabb3 {
                            min: Vec3::new(-1.0, -1.0, -1.0),
                            max: Vec3::new(1.0, 1.0, 1.0),
                        }),
                        source: BoundsSource::Fallback,
                    }
                } else {
                    Scene3dBounds {
                        local_aabb: rustlua_math::math3d::Aabb3 {
                            min: Vec3::new(-0.5, -0.5, -0.5),
                            max: Vec3::new(0.5, 0.5, 0.5),
                        },
                        source: BoundsSource::Fallback,
                    }
                };
                // P1-②：LOD（距离驱动网格选择）。`LodGroup` 的 `levels[0]` 是基础级；
                // `select(distance)` 取最后一个 threshold<=distance 的级别。没有 LodGroup
                // 或没有可用相机时退 `renderer.mesh`（原行为，像素逐字节不变）。
                let mesh_path = match (
                    lod_camera_position,
                    world
                        .get::<rustlua_render::lod::LodGroup>(id)
                        .ok()
                        .flatten(),
                ) {
                    (Some(camera_pos), Some(group)) => {
                        let model_pos = Vec3::new(
                            model.columns[3][0],
                            model.columns[3][1],
                            model.columns[3][2],
                        );
                        let distance = (model_pos - camera_pos).length();
                        group.select(distance).to_owned()
                    }
                    _ => renderer.mesh.clone(),
                };
                // WP6：3D 提取不再每帧克隆 mesh 路径（数量级 ≤13 mesh，但同类分配问题
                // 一并消除）。先借用身份表做零分配的 `handle_of` 查找，未注册时才走
                // intern（首帧/换网格各一次）；`mesh_path` 直接 move 进 drawable。
                let mesh_handle = {
                    let assets = self.assets.borrow();
                    assets.handle_of(mesh_path.as_str())
                };
                let mesh_handle = Some(match mesh_handle {
                    Some(handle) => handle,
                    None => {
                        self.assets
                            .borrow_mut()
                            .intern(
                                mesh_path.as_str(),
                                AssetRecord {
                                    path: mesh_path.clone(),
                                    kind: rustlua_assets::resource_kind::ResourceKind::Model,
                                },
                            )
                            .0
                    }
                });
                drawables.push(Scene3dDrawable {
                    entity: SceneEntityId(id.index() as u64),
                    name,
                    mesh_path,
                    // docs/54 §4.4：网格也走同一套运行时身份（`ResourceKind::Model`）。3D 提取是
                    // `&self`，因此身份表在 `RefCell` 里——同一网格跨帧恒同句柄，渲染器据此做
                    // O(1) 的网格缓存查找并认出失效引用。
                    mesh_handle,
                    model,
                    palette,
                    bounds,
                });
            };
            if self.spatial_active {
                world.for_each2::<GlobalTransform, MeshRenderer>(|id, global, renderer| {
                    // 直接用派生矩阵：它是 `Global(parent) * Local(child)` 的真值，因此剪切/镜像的
                    // 父子组合也能拿到正确的世界矩阵（旧路径此时只能退回局部值）。
                    collect(id, global.matrix(), renderer);
                });
            } else {
                world.for_each2::<Transform3D, MeshRenderer>(|id, transform, renderer| {
                    if let Ok(model) = transform.model_matrix() {
                        collect(id, model, renderer);
                    }
                });
            }
            drawables
        });
        self.view_filtered.set(self.view_filtered.get() + filtered);
        if drawables.is_empty() {
            return None;
        }
        // 编辑器视图覆盖相机时不触碰场景数据（不写 ECS、不改脚本状态），仅作为
        // 渲染视角覆盖；运行时传 None 则按 Lua 选择 / 首个 Camera3D / 默认解析。
        //
        // 拆分批 8：无覆盖时优先复用**本帧已定帧**的那一份——相机与视图都来自同一次解析/烘焙。
        // 视图只跟（相机 × 视口）有关，因此视口不同时**换烘焙、不换相机**：相机不依赖视口，
        // 再解析一次只会白白多一趟遍历（也让"同帧只解析一次"这条账本数字失真）。
        let (camera, view) = match (camera_override, self.frame_view_3d) {
            (Some(camera), _) => (
                camera,
                RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
            ),
            (None, Some(frame)) if frame.viewport == viewport => (frame.camera, frame.view),
            (None, Some(frame)) => (
                frame.camera,
                RenderView::main_3d(frame.camera, viewport, DEFAULT_3D_CLEAR),
            ),
            (None, None) => {
                let camera = self.active_camera3d();
                (
                    camera,
                    RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
                )
            }
        };
        let light = self.active_light();
        Some(Scene3d {
            camera,
            // 拆分批 7：视图在提取处成帧，渲染器（含"宿主未声明 FrameViews"的历史回退）与
            // 宿主声明的主视图因此消费同一份视口派生矩阵。
            view,
            light,
            drawables,
        })
    }

    /// 本帧的 3D 视图烘焙结果（拆分批 8）：同帧复用定帧时的那一份（`vertices()` 定的），
    /// 视口不同则只重新烘焙（相机不依赖视口，不必再解析一次）。`None` = 相机退化（位置与目标
    /// 重合等），消费方据此走各自的历史回退。
    pub(super) fn frame_camera_view(&self, viewport: Viewport) -> Option<CameraView> {
        if let Some(frame) = self.frame_view_3d {
            if frame.viewport == viewport {
                return frame.view.baked();
            }
            return CameraView::from_camera3d(frame.camera, viewport).ok();
        }
        CameraView::from_camera3d(self.active_camera3d(), viewport).ok()
    }

    /// 拆分批 8：世界或相机姿态可能变了 ⇒ 作废本帧定下的 3D 视图。
    ///
    /// 所有"宿主能在帧内调用、且可能改到**生效相机或其姿态**"的入口都必须走它（不变量是
    /// **帧内一切消费方看到同一台相机**；代价只是下一次消费多一趟 O(#相机) 的解析）。用一个
    /// 有名字的函数而不是撒一行 `= None`：漏掉一处就是一条静默的陈旧路径，而名字能把
    /// "这里发生了相机相关的世界变更"写在调用点上。
    pub(super) fn invalidate_frame_view_3d(&mut self) {
        #[cfg(test)]
        rustlua_camera::camera::ledger::count_frame_view_invalidations();
        self.frame_view_3d = None;
    }

    /// 测试用：本帧已定帧的 3D 视图（拆分批 8）。`None` = 未定帧或已被世界变更作废。
    #[cfg(test)]
    pub(crate) fn frame_view_3d(&self) -> Option<RenderView> {
        self.frame_view_3d.map(|frame| frame.view)
    }

    /// 已定帧视图里的相机（M74-01 空间音频的听者来源）。
    ///
    /// 为什么要这个而不是直接再调一次 [`Self::active_camera3d`]：正常渲染路径每帧已经解析过
    /// 相机（`vertices`/`scene_3d` 定帧时），音频再解析一次就是**同一帧算两遍**——测试账本
    /// `ledger::camera3d_resolutions` 就是专门钉这件事的（`tests/frame_view.rs` 断言一帧
    /// 恰好 1 次定帧解析）。复用这份缓存让人声与画面**看到同一台相机**，也不多付一次查找。
    pub(super) fn frame_camera3d(&self) -> Option<Camera3D> {
        self.frame_view_3d.map(|frame| frame.camera)
    }

    /// Resolves the active camera: Lua's `camera.set_active` selection, else the
    /// first entity with a `Camera3D` component, else a fixed default.
    ///
    /// **不改写相机**（拆分批 7）：返回的就是组件载荷本身。此前这里按传入的窗口比例覆写
    /// `aspect_ratio`，于是"组件里存的比例"与"渲染/拾取实际用的比例"成为两份状态。
    pub(super) fn active_camera3d(&self) -> Camera3D {
        #[cfg(test)]
        rustlua_camera::camera::ledger::count_camera3d_resolution();
        let fallback = Camera3D::perspective(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::ZERO,
            std::f32::consts::FRAC_PI_2,
            0.1,
            100.0,
        )
        .unwrap_or_else(|_| Camera3D {
            position: Vec3::new(0.0, 0.0, 5.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_y: std::f32::consts::FRAC_PI_2,
            near: 0.1,
            far: 100.0,
            pose_from_transform: false,
            enabled: true,
        });
        // WP5：多相机选择只有一条规则（显式选中优先，否则排序序第一个），由
        // `CameraSelection` 表述；宿主只在遍历候选时套用它，不再自己写一遍判断。
        // docs/54 §4.2：`enabled=false` 的相机不参与选择（既不当"第一个"，也不响应显式选中）。
        let selection = CameraSelection {
            active: self.script.active_camera().map(|id| id.index() as u64),
        };
        let camera = self.script.with_world(|world| {
            // 单趟遍历、**不建候选表**：显式选中优先，否则排序序第一个启用项。
            // （此前每调用一次就分配一个 `Vec<(u64, Camera3D)>`，而它每帧至少被调 2~3 次：
            // `scene_3d_with_camera`、`set_camera3d_view`、3D 调试线框。）
            let mut chosen: Option<Camera3D> = None;
            let mut fallback: Option<Camera3D> = None;
            let mut selected_disabled = false;
            world.for_each::<rustlua_camera::camera::Camera>(|id, camera| {
                // 唯一组件、按投影分支取值：正交相机不属于 3D pass 的候选（既不参与
                // "第一个"回退，也不响应显式选中——与它此前根本不在 `Camera3D` 存储里一致）。
                let Some(component) = camera.as_perspective() else {
                    return;
                };
                if !component.enabled {
                    selected_disabled |= selection.is_selected(id.index() as u64);
                    return;
                }
                if chosen.is_some() {
                    // 已命中显式选中项，且回退项也已就位：剩下的候选不必再解析姿态。
                    return;
                }
                // WP5：声明了 `pose=transform` 的相机，姿态改由**实体的 Transform3D** 提供
                // （位置 + -Z 视线 + +Y 上方向），本组件的 position/target 退化为回退值。
                // 没有 Transform3D 时保持组件姿态（scene_extract 会为此报诊断）。
                let camera = if component.pose_from_transform {
                    // docs/54 §8-⑦：姿态取实体的**世界**变换（父链组合后的结果）。取局部值
                    // 会让挂在父下的相机看不清"它到底在看哪里"；这里用 lazy 版本，因此不依赖
                    // "调用方本帧已经跑过传播"这个隐含顺序。
                    match rustlua_ecs::hierarchy::world_transform_3d_lazy(world, id) {
                        Ok(Some(pose)) => component.with_pose(pose).unwrap_or(component),
                        _ => component,
                    }
                } else {
                    component
                };
                if selection.is_selected(id.index() as u64) {
                    chosen = Some(camera);
                } else if fallback.is_none() {
                    fallback = Some(camera);
                }
            });
            if selected_disabled {
                tracing::warn!(
                    target: "render",
                    "[render] `camera.set_active` 选中的相机 `enabled=false`，已回退到下一台可用相机"
                );
            }
            chosen.or(fallback)
        });
        camera.unwrap_or(fallback)
    }

    /// Resolves the active light: Lua's `light.set_active` selection, else the
    /// first entity with a `Light` component, else a default directional light.
    fn active_light(&self) -> Light {
        let selected = self.script.active_light();
        self.script
            .with_world(|world| {
                let mut selected_light = None;
                let mut fallback = None;
                world.for_each::<Light>(|id, light| {
                    if fallback.is_none() {
                        fallback = Some(*light);
                    }
                    if selected == Some(id) {
                        selected_light = Some(*light);
                    }
                });
                selected_light.or(fallback)
            })
            .unwrap_or_default()
    }

    pub fn vertices(&mut self, width: u32, height: u32) -> anyhow::Result<SceneMesh> {
        self.viewport_width = width.max(1);
        self.viewport_height = height.max(1);
        // 拆分批 8：新的一帧 ⇒ 上一帧定下的 3D 视图不再可用（下面按本帧视口重新定帧）。
        // 这里刻意**不**走 `invalidate_frame_view_3d()`：那是"世界变更把定帧值作废"的账，
        // 帧边界不是世界变更（账本里两者分开记，混在一起就看不出"到底是谁作废的"）。
        self.frame_view_3d = None;
        self.script.set_ui_viewport(width, height);
        // WP2：提取前先把局部变换传播成世界变换（开启空间继承时才有实际效果；关闭时这一步
        // 只会清掉遗留的世界变换组件，消费者读到的仍是局部值 = 旧行为）。
        self.propagate_spatial_now();
        let viewport = Viewport::new(width.max(1) as f32, height.max(1) as f32)?;
        // WP4 ⑧：把本帧渲染用的 2D 相机与视口回填给 Lua，供 `camera2d.screen_to_world`
        // 一类换算使用。帧首回填 ⇒ 脚本看到的是上一帧末相机控制器算出的、也就是玩家刚看到的
        // 那一帧的投影；脚本在首帧渲染前调用换算会报错而不是猜一个默认相机。
        self.script
            .set_camera_view(self.camera, [width.max(1) as f32, height.max(1) as f32]);
        // docs/54 §8-⑦：把本帧 **3D pass 实际用的那台相机**回填给 Lua，于是 `camera3d.ray`
        // 与编辑器 3D 点选走的是同一份投影数学（同一条 `Camera3D::ray_from_screen`）。
        // 拆分批 7：回填的是**组件载荷本身**——宽高比由 Lua 侧的视口给出，不再写回相机。
        // 拆分批 8：这里同时是**定帧点**——解析一次 `active_camera3d()`、烘焙一次视图，之后同帧的
        // `scene_3d` 与 3D 调试线框复用同一份，于是"渲染用的相机"与"回填给 Lua 的相机"由**构造**
        // 决定同源，而不是靠"两步之间没人改世界"的巧合。
        let camera3d = self.active_camera3d();
        self.script
            .set_camera3d_view(camera3d, [width.max(1) as f32, height.max(1) as f32]);
        self.frame_view_3d = Some(FrameView3d {
            camera: camera3d,
            viewport,
            view: RenderView::main_3d(camera3d, viewport, DEFAULT_3D_CLEAR),
        });
        self.sprite_batch.clear();
        // WP6：收集阶段的可见域。开着时 `SpriteBatch::push` 直接丢掉不可能落在视口内的精灵，
        // 省下每项 80 字节的写入与后续排序——5 万精灵档实测这部分占掉整帧的大头（docs/53 §3.9）。
        // 世界矩形只看相机，与 `sprite_culling` 同一个开关：关掉即全收，像素不变。
        //
        // docs/54 §8-④：**世界通道的可见域是宿主声明的世界视图，不是 2D 相机**。2D 相机活在
        // 设计分辨率像素空间（默认中心 (640,360)），而 3D 世界视图里 `(-40, 0)` 之类坐标清晰
        // 可见；场景里有 3D 内容时宿主会声明 3D 主视图，此时按 2D 相机剔除会在**收集阶段**就把
        // 世界内容丢掉（`wp0_mixed_golden` 抓到的真实误丢）。因此有 3D 内容时世界通道全收：
        // 保守不剔除，绝不误丢。纯 2D 画面仍按 2D 视口裁剪，5 万精灵档的收益不变。
        let world_view_is_3d = self.world_sprite_depth && self.world_view_has_3d_content();
        let cull_bounds = if self.sprite_culling && !world_view_is_3d {
            Some(self.camera.visible_world_bounds(viewport)?)
        } else {
            None
        };
        self.sprite_batch.set_cull_bounds(cull_bounds);
        self.texture_paths.clear();
        // docs/54 §4.4：本帧的路径与去重表重建，但**身份表不清空**——句柄因此跨帧稳定。
        self.texture_lookup.clear();
        self.texture_handles.clear();
        // docs/54 §4.3：显式模式的批次每帧重建；没有实体声明模式时它们恒为空（零成本）。
        self.overlay_batch.clear();
        self.screen_batch.clear();
        self.overlay_batch.set_cull_bounds(cull_bounds);
        self.screen_batch.set_cull_bounds(cull_bounds);
        self.render_mode_routed = (0, 0, 0);
        // 每帧先清空：布局阶段（push_scene_ui 等）再把本帧用到的自定义字体 key 记入。
        self.frame_font_keys.clear();
        // Tile maps draw first (layer 0), sprite entities on top (layer 1+).
        //
        // WP6 顶点单趟化：瓦片与精灵的顶点**直接以 NDC 追加进同一个最终缓冲**，不再先各自
        // 物化一份 `SpriteVertex`（10k 精灵实测 1.9 MB）再由宿主逐顶点转换。draw range 由
        // 各段按当前缓冲长度自行偏移，因此两段可以连续追加而无需事后再合并。
        let mut vertices: Vec<OverlayVertex> = Vec::new();
        let mut draw_ranges: Vec<SpriteDrawBatch> = Vec::new();
        // WP6：世界 Sprite 深度通道的输出（世界坐标 + z）。关闭时全程为空，渲染器不执行该
        // 通道；打开时精灵与瓦片层都往这里写。
        let mut world_sprites: Vec<WorldSpriteVertex> = Vec::new();
        let mut world_sprite_draws: Vec<SpriteDrawBatch> = Vec::new();
        self.append_tilemap_vertices(
            &mut vertices,
            &mut world_sprites,
            &mut draw_ranges,
            &mut world_sprite_draws,
            viewport,
        )?;
        // WP6：精灵直接写进批处理，不再每帧先造一份
        // `(Transform, Sprite, Option<AnimationStateMachine>, Option<String>, Option<SpriteSorting>)`
        // 中间表——那份表每帧要搬上兆字节，还要为每个精灵克隆资源路径与排序层字符串。
        // 纹理注册顺序与旧路径完全一致（同一遍历顺序、同一注册点），因此纹理下标不变。
        {
            let RuntimeScene {
                script,
                sprite_batch,
                overlay_batch,
                screen_batch,
                texture_paths,
                texture_lookup,
                assets,
                texture_handles,
                ..
            } = self;
            let mut failure: Option<anyhow::Error> = None;
            let inherit = self.spatial_active;
            // docs/54 §4.2：世界视图的可见遮罩。被拦掉的精灵**不占序号、不产顶点**——
            // "不在这个视图里"不是"透明"。
            let view_mask = self.view_world_mask;
            let mut filtered = 0usize;
            let mut routed = (0usize, 0usize, 0usize);
            // P1-①：渲染级空间索引（视口候选预筛）。指纹未变则复用网格；变了则整体重建；
            // 查询得到视口候选集合，收集阶段只处理候选（静态场景收集 O(N) → O(视口内)）。
            // 与 `cull_bounds` 同条件：3D 世界视图（world_view_is_3d）下遮挡来自深度，网格
            // 预筛不生效；`sprite_culling` 关闭时同样不预筛（全收，像素不变）。
            let candidates: Option<std::collections::HashSet<EntityId>> =
                if self.spatial_index && self.sprite_culling && !world_view_is_3d {
                    let fingerprint = script.with_world(rustlua_ecs::hierarchy::local_fingerprint);
                    if self.spatial_index_fingerprint != Some(fingerprint) {
                        // 重建：与提取同一对组件族（inherit 分支同源），只取位置/缩放/尺寸；
                        // for_each2 按 EntityId 升序，rebuild 要求升序输入。
                        let cell_size = self.spatial_index_cell_size;
                        let grid = script.with_world(|world| {
                            let mut items: Vec<(EntityId, rustlua_render::spatial_index::Circle)> =
                                Vec::new();
                            if inherit {
                                world.for_each2::<GlobalTransform, Sprite>(|id, global, sprite| {
                                    if let Some(plane) = global.plane() {
                                        items.push((
                                            id,
                                            rustlua_render::spatial_index::Circle::from_sprite(
                                                plane.position,
                                                plane.scale,
                                                sprite.size,
                                            ),
                                        ));
                                    }
                                });
                            } else {
                                world.for_each2::<Transform, Sprite>(|id, transform, sprite| {
                                    items.push((
                                        id,
                                        rustlua_render::spatial_index::Circle::from_sprite(
                                            transform.position,
                                            transform.scale,
                                            sprite.size,
                                        ),
                                    ));
                                });
                            }
                            let mut grid =
                                rustlua_render::spatial_index::SpatialGrid2D::new(cell_size);
                            grid.rebuild(items);
                            grid
                        });
                        self.spatial_grid = Some(grid);
                        self.spatial_index_fingerprint = Some(fingerprint);
                    }
                    // 查询候选（CameraBounds → 查询 Bounds）。
                    cull_bounds.map(|bounds| {
                        let query_bounds = rustlua_render::spatial_index::Bounds {
                            min_x: bounds.min.x,
                            min_y: bounds.min.y,
                            max_x: bounds.max.x,
                            max_y: bounds.max.y,
                        };
                        self.spatial_grid
                            .as_ref()
                            .map(|grid| {
                                grid.query(query_bounds)
                                    .into_iter()
                                    .collect::<std::collections::HashSet<EntityId>>()
                            })
                            .unwrap_or_default()
                    })
                } else {
                    None
                };
            script.with_world(|world| {
                let mut index = 0usize;
                // WP2：提取用的变换来源由空间继承开关**每帧选定一次**：开启时整条换成
                // `WorldTransform`，关闭时仍走 `Transform`。刻意不做"每个实体先试世界再退回局部"
                // ——那会给每个精灵多一次 `get`（两层哈希），WP6 已量到那正是 5 万精灵档的大头。
                // 不变量（见 `rustlua_ecs::hierarchy`）：开启继承时**每个有局部变换的实体都有世界变换**
                // （组合不了或成环的保留局部值当世界值），所以整条替换不会漏画任何一个精灵。
                let mut collect = |id: EntityId, transform: &Transform, sprite: &Sprite| {
                    if failure.is_some() {
                        return;
                    }
                    if !rustlua_render::visibility::entity_visible_in(world, id, view_mask) {
                        filtered += 1;
                        return;
                    }
                    // 与旧实现一致：序号在"通过 Transform 检查"后前进，缺资源的精灵
                    // 仍然占一个序号（否则默认绘制层会整体前移）。
                    let layer_index = index;
                    index += 1;
                    // P1-①：空间索引预筛——不在视口候选里的实体在构建 item 之前就跳过；
                    // 与原路径 push 被 cull 拒绝的净效果一致（像素逐字节相同，docs/53 §3.9）。
                    if candidates.as_ref().is_some_and(|set| !set.contains(&id)) {
                        return;
                    }
                    let (texture, sprite) =
                        match world.get::<AnimationStateMachine>(id).ok().flatten() {
                            Some(machine) => {
                                let (uv_min, uv_max) = machine.current_frame_uv();
                                let sampled = match sprite.with_uv(uv_min, uv_max) {
                                    Ok(sampled) => sampled,
                                    Err(error) => {
                                        failure = Some(error.into());
                                        return;
                                    }
                                };
                                let sheet = machine.current_clip().sheet.as_str();
                                (
                                    texture_id_of(
                                        sheet,
                                        assets,
                                        texture_paths,
                                        texture_lookup,
                                        texture_handles,
                                    ),
                                    sampled,
                                )
                            }
                            None => match world.get::<SpriteAsset>(id).ok().flatten() {
                                Some(asset) => {
                                    // 图集重定向（`docs/67` §6.4）：`assetc` cook 之后散图已被页取代，
                                    // 工程源里写的仍是原路径。这里把它翻成「页 + 区域」，
                                    // **贴图上传与合批因此都不需要再知道图集的存在**：
                                    // 上传按页路径取图，合批按 texture_id（= 页）自然合并。
                                    //
                                    // 九宫格不重定向：`nine_slice` 的边框是相对**整张纹理**算的，
                                    // 换成页上的子区域会把边框取错位置。这条是保守选择——
                                    // 宁可让九宫格继续用散图（cook 时可以把它排除出 include），
                                    // 也不要画出错位的面板。
                                    let redirect = if sprite.nine_slice.is_none() {
                                        rustlua_render::atlas::redirect_region(asset.path())
                                    } else {
                                        None
                                    };
                                    match redirect {
                                        Some((page, region)) => {
                                            let sampled = match sprite
                                                .with_uv(region.uv_min, region.uv_max)
                                            {
                                                Ok(sampled) => sampled,
                                                Err(error) => {
                                                    failure = Some(error.into());
                                                    return;
                                                }
                                            };
                                            (
                                                texture_id_of(
                                                    &page,
                                                    assets,
                                                    texture_paths,
                                                    texture_lookup,
                                                    texture_handles,
                                                ),
                                                sampled,
                                            )
                                        }
                                        None => (
                                            texture_id_of(
                                                asset.path(),
                                                assets,
                                                texture_paths,
                                                texture_lookup,
                                                texture_handles,
                                            ),
                                            *sprite,
                                        ),
                                    }
                                }
                                None => return,
                            },
                        };
                    let layer = world
                        .get::<SpriteSorting>(id)
                        .ok()
                        .flatten()
                        .map(SpriteSorting::effective_layer)
                        .unwrap_or(1 + layer_index as i32);
                    // docs/54 §4.3：显式显示模式把这段几何路由到对应通道；未声明 = 跟随全局开关。
                    let item = SpriteBatchItem::new(texture, layer, sprite, *transform);
                    match world
                        .get::<rustlua_render::render_mode::RenderMode>(id)
                        .ok()
                        .flatten()
                        .copied()
                    {
                        Some(rustlua_render::render_mode::RenderMode::Overlay) => {
                            routed.1 += 1;
                            overlay_batch.push(item);
                        }
                        Some(rustlua_render::render_mode::RenderMode::Screen) => {
                            routed.2 += 1;
                            screen_batch.push(item);
                        }
                        Some(rustlua_render::render_mode::RenderMode::World) => {
                            routed.0 += 1;
                            sprite_batch.push(item);
                        }
                        None => sprite_batch.push(item),
                    }
                };
                // `for_each2` 把变换直接交到手上，省掉每个精灵一次 `get::<Transform>`——那是一次
                // `TypeId` 表查询 + 一次实体表查询（两层哈希），5 万精灵档实测就是它的主要成本
                // （docs/53 §3.10）。两者都按"排序实体序"遍历，且没有变换的精灵本来就被跳过
                // （不推进序号），因此语义不变。
                if inherit {
                    world.for_each2::<GlobalTransform, Sprite>(|id, global, sprite| {
                        // 平面视图：不开继承时走局部值，开启时走派生结果（压不平时已退回局部值）。
                        if let Some(plane) = global.plane() {
                            collect(id, &plane, sprite);
                        }
                    });
                } else {
                    world.for_each2::<Transform, Sprite>(|id, transform, sprite| {
                        collect(id, transform, sprite);
                    });
                }
            });
            if let Some(message) = failure {
                return Err(message);
            }
            self.view_filtered.set(self.view_filtered.get() + filtered);
            self.render_mode_routed = routed;
        }
        // docs/54 §4.3：一等 Quad（几何 + 材质，贴图可选）。**刻意走与精灵完全相同的那条路**：
        // Quad 在提取阶段降级成 `Sprite`（`Quad::to_sprite`：几何/材质/深度原样带过去，纹理区域
        // 取整张贴图），因此排序、剔除、深度、可见遮罩、世界/overlay 两条通道全部与精灵同源——
        // "Sprite 与 Quad 共享 RenderWorld 语义"是结构上成立的，而不是两套并行实现靠约定对齐。
        {
            let RuntimeScene {
                script,
                sprite_batch,
                overlay_batch,
                screen_batch,
                texture_paths,
                texture_lookup,
                assets,
                texture_handles,
                ..
            } = self;
            let mut failure: Option<anyhow::Error> = None;
            let inherit = self.spatial_active;
            let view_mask = self.view_world_mask;
            let mut filtered = 0usize;
            let white = rustlua_render::quad::QUAD_DEFAULT_TEXTURE;
            let mut routed = (0usize, 0usize, 0usize);
            script.with_world(|world| {
                let mut index = 0usize;
                let mut collect = |id: EntityId, transform: &Transform, quad: &Quad| {
                    if failure.is_some() {
                        return;
                    }
                    if !rustlua_render::visibility::entity_visible_in(world, id, view_mask) {
                        filtered += 1;
                        return;
                    }
                    // docs/54 §4.3：一个实体只能有一种可绘制表达。同时挂 `Sprite` 与 `Quad` 时按
                    // 写下来的优先级只画 `Sprite`（否则会画两遍）；加载期已经就这个冲突点过名。
                    if world.get::<Sprite>(id).ok().flatten().is_some() {
                        return;
                    }
                    let layer_index = index;
                    index += 1;
                    let sprite = match quad.to_sprite() {
                        Ok(sprite) => sprite,
                        Err(error) => {
                            failure = Some(error.into());
                            return;
                        }
                    };
                    // 贴图注册与精灵同一张表：同一个路径只会得到一个下标，因此 Quad 与 Sprite
                    // 引用同一张贴图时能合进同一个绘制段（不会因为"类型不同"而多一次 draw）。
                    let path = quad.texture.as_deref().unwrap_or(white);
                    let texture =
                        texture_id_of(path, assets, texture_paths, texture_lookup, texture_handles);
                    let layer = if quad.sorting == rustlua_render::sprite::SpriteSorting::default()
                    {
                        // 与精灵同一套默认层规则（按序号）。两个集合的序号各自从 0 起，因此同层时
                        // 后收集的 Quad 画在精灵之上——色块/进度条底这类用途本来就该压在上面。
                        1 + layer_index as i32
                    } else {
                        quad.sorting.effective_layer()
                    };
                    // docs/54 §4.3：显式显示模式把这段几何路由到对应通道；未声明 = 跟随全局开关。
                    let mode = world
                        .get::<rustlua_render::render_mode::RenderMode>(id)
                        .ok()
                        .flatten()
                        .copied();
                    let item = SpriteBatchItem::new(texture, layer, sprite, *transform);
                    match mode {
                        Some(rustlua_render::render_mode::RenderMode::Overlay) => {
                            routed.1 += 1;
                            overlay_batch.push(item);
                        }
                        Some(rustlua_render::render_mode::RenderMode::Screen) => {
                            routed.2 += 1;
                            screen_batch.push(item);
                        }
                        Some(rustlua_render::render_mode::RenderMode::World) => {
                            routed.0 += 1;
                            sprite_batch.push(item);
                        }
                        None => sprite_batch.push(item),
                    }
                };
                if inherit {
                    world.for_each2::<GlobalTransform, Quad>(|id, global, quad| {
                        if let Some(plane) = global.plane() {
                            collect(id, &plane, quad);
                        }
                    });
                } else {
                    world.for_each2::<Transform, Quad>(|id, transform, quad| {
                        collect(id, transform, quad);
                    });
                }
            });
            if let Some(message) = failure {
                return Err(message);
            }
            self.view_filtered.set(self.view_filtered.get() + filtered);
            self.render_mode_routed = (
                self.render_mode_routed.0 + routed.0,
                self.render_mode_routed.1 + routed.1,
                self.render_mode_routed.2 + routed.2,
            );
        }
        self.push_particles()?;
        // 精灵沿用现有的排序与错误语义，但顶点直接按 NDC 追加到 `vertices` 之后（瓦片之上）。
        // **必须预留容量**：追加式写入若从零一路翻倍到 6 万顶点，光增长就多分配约一倍字节
        // （实测 10k 精灵档 4.53 → 7.53 MB，比单趟化省下的还多）。按每精灵 6 个顶点一次开够；
        // `reserve` 在容量已足时是空操作，不会浪费瓦片层已预留的部分。
        // docs/54 §4.3：**显式显示模式**让同一场景混用世界通道与屏幕空间内容。
        //
        // `mode=world` 是请求世界通道：只要场景里有实体这样声明，这一帧就走世界通道（与宿主开关
        // 同一效果，通道是帧级的——默认实体也跟着进，这条写进 docs/16 避免意外）。
        // `mode=overlay` / `mode=screen` 是退出世界通道：它们**永远**走 NDC overlay 通道，
        // 分别落在 `Overlay2D` 与 `ScreenUI` 桶里（HUD 语义：画在世界内容之上）。
        let explicit_world = self.render_mode_routed.0 > 0;
        let world_channel = self.world_sprite_depth || explicit_world;
        if world_channel {
            // WP6：世界 Sprite 走带深度的通道——世界坐标 + `Sprite::z`，投影交给顶点着色器。
            // 排序/分段/剔除与 overlay 路径同源（同一个批处理、同一把剔除量尺）。
            //
            // docs/54 §8-④：世界通道的剔除量尺必须与世界视图一致——有 3D 内容时收集阶段
            // 已经按"全收"处理（见上面的 `world_view_is_3d`），这里同样不再用 2D 矩形过滤。
            let world_cull_bounds = if world_view_is_3d { None } else { cull_bounds };
            self.sprite_batch.build_world_into(
                &mut world_sprites,
                &mut world_sprite_draws,
                world_cull_bounds,
                self.sprite_parallelism,
            )?;
        } else {
            vertices.reserve(self.sprite_batch.len() * 6);
            draw_ranges.reserve(2);
            self.sprite_batch.build_overlay_into(
                self.camera,
                viewport,
                &mut vertices,
                &mut draw_ranges,
                self.sprite_parallelism,
                self.sprite_culling,
            )?;
        }
        // 显式屏幕空间的批次：无论这一帧走不走世界通道都走 overlay 通道。
        let mut overlay_ranges: Vec<SpriteDrawBatch> = Vec::new();
        let mut screen_ranges: Vec<SpriteDrawBatch> = Vec::new();
        if !self.overlay_batch.is_empty() {
            self.overlay_batch.build_overlay_into(
                self.camera,
                viewport,
                &mut vertices,
                &mut overlay_ranges,
                self.sprite_parallelism,
                self.sprite_culling,
            )?;
        }
        if !self.screen_batch.is_empty() {
            self.screen_batch.build_overlay_into(
                self.camera,
                viewport,
                &mut vertices,
                &mut screen_ranges,
                self.sprite_parallelism,
                self.sprite_culling,
            )?;
        }

        let mut vector_vertices = Vec::with_capacity(256);
        let mut vector_glyphs = Vec::with_capacity(64);
        let mut vector_segments = Vec::with_capacity(1024);
        tracing::debug!(
            target: "render",
            "[render] batch items={} vertices={} draw_ranges={}",
            self.sprite_batch.len(),
            vertices.len(),
            draw_ranges.len()
        );
        let mut draws: Vec<SceneDraw> = draw_ranges
            .iter()
            .map(|draw| SceneDraw {
                texture: draw.texture_id + 1, // 0 is reserved for the white texture
                start: draw.vertex_start as u32,
                count: draw.vertex_count as u32,
                sdf: false,
                bitmap: false,
                vector: false,
                render_layer: RenderLayer::World2D,
            })
            .collect();
        // docs/54 §4.3：显式屏幕空间的批次接在默认批次之后，桶分别是 `Overlay2D` 与 `ScreenUI`
        // （分桶顺序由渲染器保证：World2D → Overlay2D → ScreenUI），因此声明了模式的实体画在世界
        // 内容之上，而 `mode=screen` 又被其它 2D 内容盖不住。
        for (ranges, layer) in [
            (&overlay_ranges, RenderLayer::Overlay2D),
            (&screen_ranges, RenderLayer::ScreenUI),
        ] {
            draws.extend(ranges.iter().map(|draw| SceneDraw {
                texture: draw.texture_id + 1,
                start: draw.vertex_start as u32,
                count: draw.vertex_count as u32,
                sdf: false,
                bitmap: false,
                vector: false,
                render_layer: layer,
            }));
        }
        // WP7 ②：collider 调试绘制（世界内容之上、屏幕 UI 之下）。Lua 的开关请求
        // 在这里被取走，与 camera2d 的请求/消费模式一致。
        if let Some(enabled) = self.script.take_collider_debug_request() {
            self.collider_debug_draw = enabled;
        }
        if let Some(enabled) = self.script.take_collider_debug_request_3d() {
            self.collider_debug_draw_3d = enabled;
        }
        self.push_collider_debug(&mut vertices, &mut draws, viewport);
        self.push_collider_debug_3d(&mut vertices, &mut draws, viewport);
        // 世界空间文字：投影锚点后走同一套字形管线；绘制在精灵之上、屏幕 UI 之下。
        // 不受 `render_ui_overlay` 开关影响——它是世界内容，不是屏幕 UI。
        self.push_world_text(
            &mut vertices,
            &mut world_sprites,
            &mut draws,
            &mut world_sprite_draws,
            &mut vector_vertices,
            &mut vector_glyphs,
            &mut vector_segments,
            width,
            height,
        )?;
        // Screen-space UI (health bars, prompt text) draws last, on top。
        // 编辑器场景视图置 render_ui_overlay=false：UI 由 egui 在设计分辨率包围框内
        // 绘制布局框，跳过此 NDC 覆盖层避免冲突。
        if self.render_ui_overlay {
            // 实体层两趟共用同一份解算结果，每帧只算一次（`push_scene_ui_*` 不再自行解算，
            // 否则布局求解翻倍）。
            self.ensure_scene_ui_layout([width.max(1) as f32, height.max(1) as f32]);
            // **骨架在下**：预设/场景实体的 `Image` 先提交，动态层（panel/image/grid/bar/text）
            // 画在它之上。M66–M71 六屏迁移的前提就是这条不变式；顺序反了时卡底
            // （`ui_skill_card_9s.png` 主体全不透明）会整块埋掉压在它上面的 24×24 招式图标
            // ——两个宿主一起瞎，而状态层读数照样"shown"（实机 2026-10-07）。
            self.push_scene_ui_images(&mut vertices, &mut draws, width, height)?;
            self.push_ui_panels(&mut vertices, &mut draws, width, height)?;
            self.push_ui_images(&mut vertices, &mut draws, width, height)?;
            self.push_ui_grids(&mut vertices, &mut draws, width, height)?;
            self.push_ui_bars(&mut vertices, &mut draws, width, height)?;
            self.push_ui_text(
                &mut vertices,
                &mut draws,
                &mut vector_vertices,
                &mut vector_glyphs,
                &mut vector_segments,
                width,
                height,
            )?;
            // 场景文字仍在最最后：`prewarm_frame_glyphs` 必须是本帧最后一个会撑大字体
            // 图集的人（图集增长不回改已写入顶点的 UV）。
            self.push_scene_ui_text(
                &mut vertices,
                &mut draws,
                &mut vector_vertices,
                &mut vector_glyphs,
                &mut vector_segments,
                width,
                height,
            )?;
        }
        // UI 合批：相邻元素在顶点缓冲里连续，合并同纹理同 sdf 的 draw，减少
        // draw call（如一排同纹理方块/面板 → 一次绘制）。
        merge_adjacent_draws(&mut draws);
        // 每帧的 UI 指标不再打 INFO 日志（避免持续刷屏）；需要时由下方 debug 级输出。
        tracing::debug!(
            target: "render",
            "[render] mesh vertices={} draws={} paths={}",
            vertices.len(),
            draws.len(),
            self.texture_paths.len()
        );
        let font_atlas = if let Some(atlas) = &self.font_atlas {
            let atlas = atlas.lock().expect("font atlas lock");
            let version = atlas.version();
            // 仅当本帧网格会被渲染器实际消费（vertices 非空）时才推进“已上报”
            // 版本号。空网格帧渲染器整体跳过字体上传；若此刻推进版本号，下一个
            // 可绘制帧会以“已上报”发送空像素，而渲染器从未上传过该版本 →
            // write_texture 以空数据写整张图集 → wgpu 校验崩溃。
            let changed = version != self.last_font_version && !vertices.is_empty();
            if !vertices.is_empty() {
                self.last_font_version = version;
            }
            Some(FontAtlasPixels {
                width: atlas.width(),
                height: atlas.height(),
                pixels: if changed {
                    atlas.pixels().to_vec()
                } else {
                    Vec::new()
                },
                version,
            })
        } else {
            None
        };
        let custom_font_atlases = self.snapshot_custom_fonts(!vertices.is_empty());
        // WP6 世界通道的**跨产出者**排序：瓦片层、精灵、世界文字是三个各自有序的产出者
        // （瓦片与世界文字都在世界平面 z=0，精灵带自己的 z）。不写深度时顺序就是一切，
        // 因此在交给渲染器之前按"先层、再 z 从远到近、最后原有先后"整体重排一次，
        // 让半透明走画家算法正确叠加。
        sort_world_ranges(&mut world_sprites, &mut world_sprite_draws);
        Ok(SceneMesh {
            vertices,
            draws,
            world_sprites,
            world_sprite_draws,
            paths: self.texture_paths.clone(),
            // docs/54 §4.4：与 `paths` 同序的稳定身份，交给渲染器做 O(1) 的贴图缓存查找。
            path_handles: self.texture_handles.clone(),
            font_atlas,
            custom_font_atlases,
            vector_vertices,
            vector_glyphs,
            vector_segments,
        })
    }

    /// 生成本帧用到（且自上次上报后版本变化）的自定义字体图集像素，随 SceneMesh
    /// 交给渲染器上传/重绑。`pixels` 空 = 未变化（与默认图集 `font_atlas` 语义一致）。
    ///
    /// `will_draw` 必须与默认图集用同一个判据（本帧 `vertices` 非空）：渲染器在
    /// `vertices` 为空时**整体跳过** 2D 通道，自定义字体的上传与 bind 建立也随之
    /// 跳过。若此时仍把版本记为"已上报"，下一个可绘制帧会以"未变化"（空像素）
    /// 发送，而渲染器从未上传过该版本 → 该自定义字体永久不显示。
    fn snapshot_custom_fonts(&mut self, will_draw: bool) -> Vec<CustomFontAtlasPixels> {
        let mut seen: HashSet<String> = HashSet::new();
        let keys: Vec<String> = std::mem::take(&mut self.frame_font_keys)
            .into_iter()
            .filter(|key| seen.insert(key.clone()))
            .collect();
        let mut out = Vec::new();
        for key in keys {
            // docs/60 §10.7 定性：这里原本把**两种完全不同的原因**合并成一个 `None` ——
            //   ① 锁中毒（`Err`）：真出事了，而且此后**这个字体永远进不了快照**；
            //   ② 映射里还没这个 key：**正常的"未就绪"**（字体本帧稍后才建 / 首帧还没建），下一帧就好。
            // 合并的后果是 ① 这种硬失败**没有任何痕迹**，症状与"那种字体的文字不渲染、日志无字"同族。
            // 现在分开：① 告警（带 key 与错误）、② debug（不刷屏）。
            //
            // 为什么不跟着下面那句 `expect("font atlas lock")` 一起 panic：**wasm 上 panic 是硬 abort**
            // （docs/60 §4.3 的同一条教训），而"某个自定义字体不可用"不该把整个游戏带走。
            // （顺带记一笔：同一个锁在这里被捕获、在两行后被 `expect`，两套口径——本轮只统一到"留痕"这一层。）
            let handle = match self.custom_font_atlases.lock() {
                Ok(map) => map.get(&key).cloned(),
                Err(error) => {
                    tracing::warn!(
                        target: "render",
                        key = %key,
                        %error,
                        "[runtime] custom font atlas lock poisoned; font dropped from the snapshot"
                    );
                    continue;
                }
            };
            let Some(handle) = handle else {
                tracing::debug!(
                    target: "render",
                    key = %key,
                    "[render] custom font not registered yet; skipping this frame"
                );
                continue;
            };
            let atlas = handle.lock().expect("font atlas lock");
            let version = atlas.version();
            let changed = self.custom_font_versions.get(&key).copied() != Some(version);
            if will_draw {
                self.custom_font_versions.insert(key.clone(), version);
            }
            out.push(CustomFontAtlasPixels {
                key,
                pixels: FontAtlasPixels {
                    width: atlas.width(),
                    height: atlas.height(),
                    // 未被绘制的帧不复制像素：既不浪费内存，也不推进"已上报"版本。
                    pixels: if changed && will_draw {
                        atlas.pixels().to_vec()
                    } else {
                        Vec::new()
                    },
                    version,
                },
            });
        }
        out
    }

    /// Pushes every active particle as a small white-texture quad in world
    /// space, so they sit with the scene and fade out with their color alpha.
    fn push_particles(&mut self) -> anyhow::Result<()> {
        // 直接在借用内部写入批次：旧实现先把整条粒子数组 `to_vec()` 拷一份，10k 粒子
        // 每帧白拷 840 KB（`wp0-v4` 的粒子阶梯）。
        let RuntimeScene {
            script,
            sprite_batch,
            texture_paths,
            texture_lookup,
            assets,
            texture_handles,
            ..
        } = self;
        let texture = texture_id_of(
            WHITE_TEXTURE_PATH,
            assets,
            texture_paths,
            texture_lookup,
            texture_handles,
        );
        let mut failure: Option<anyhow::Error> = None;
        script.with_particles(|particles| {
            for particle in particles {
                let size = particle.size();
                // 尺寸曲线收到 0 时该粒子不该再占一个四边形。
                if size <= 0.0 {
                    continue;
                }
                let color = particle.color();
                let sprite = match Sprite::new(Vec2::new(size, size))
                    .and_then(|sprite| sprite.with_tint([color[0], color[1], color[2], color[3]]))
                {
                    Ok(sprite) => sprite,
                    Err(error) => {
                        failure = Some(error.into());
                        return;
                    }
                };
                // 自转复用 Transform，与精灵走同一条变换路径。
                let transform = Transform {
                    rotation: particle.rotation,
                    ..Transform::from_position(particle.position)
                };
                sprite_batch.push(SpriteBatchItem::new(texture, 1, sprite, transform));
            }
        });
        match failure {
            Some(message) => Err(message),
            None => Ok(()),
        }
    }
}
