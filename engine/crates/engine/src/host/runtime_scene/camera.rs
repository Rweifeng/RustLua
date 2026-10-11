//! 2D 相机的选择口径与精灵屏幕矩形（WP12 阶段 2 从 `runtime_scene.rs` 原样搬出）。
//!
//! `first_enabled_ortho_camera_in` 保持私有：它只被本文件的两台相机选择函数共用，搬动后
//! 语义不变——"哪台算场景相机"仍然只有一个答案（见函数自己的注释）。
//! `declared_camera_2d_in` / `selected_camera_2d_in` 被 `mod.rs` 的 `init` 与相机同步调用，
//! `sprite_screen_rect` 被 UI 点击目标收集调用，故标 `pub(super)`。

use rustlua_camera::camera::{Camera2D, CameraSelection, OrthoTerms, Viewport};
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::transform::{Transform, Vec2};
use rustlua_render::sprite::Sprite;

// 阶段 3c 把 2D 相机的三条方法（同步/生效载荷/步进）接进本文件：类型定义仍在 mod.rs，
// 固有 impl 块按模块分散即可。
use super::RuntimeScene;

/// 世界里声明的第一台**启用**的 2D 相机（`Camera projection=orthographic`），没有则 `None`。
///
/// **启动场景与换图读的是同一份口径**：`init` 用它决定初始相机，`sync_camera_from_scene`
/// 用它决定换图后的 bounds。两处各写一遍的话，"哪台算场景相机"就会有两个答案。
///
/// docs/54 §4.2：被禁用的相机不参与选择（与 3D 侧同一条规则），但过滤在遍历
/// **里面**——"第一台被禁用"不该挡掉后面启用的那台。
///
/// 运行时世界里只有**一个** `Camera` 组件，因此这里单趟遍历、按投影分支取值，
/// 不再需要"先取一份 `Vec<EntityId>` 再逐实体 `get`"。
pub(super) fn declared_camera_2d_in(world: &rustlua_ecs::world::World) -> Option<Camera2D> {
    first_enabled_ortho_camera_in(world).map(|(_, camera)| camera)
}

/// 排序序第一台**启用**的正交相机（拆分批 9：这是"默认 2D 相机"的唯一定义处）。
fn first_enabled_ortho_camera_in(
    world: &rustlua_ecs::world::World,
) -> Option<(EntityId, Camera2D)> {
    let mut found = None;
    world.for_each::<rustlua_camera::camera::Camera>(|id, camera| {
        if found.is_some() {
            return;
        }
        if let Some(orthographic) = camera.as_orthographic()
            && orthographic.enabled
        {
            found = Some((id, orthographic));
        }
    });
    found
}

/// 拆分批 9：**2D 相机的来源**——`camera.set_active` 选中的那台**正交**相机（与 3D pass 共用同
/// 一条选择链，只是候选按投影过滤）。
///
/// 语义（为什么与 `active_camera3d` 不完全对称）：
/// - 选中的是正交相机且启用 ⇒ 返回它（宿主采用它的**载荷**作为 2D 取景基准，即"换 2D 相机"）；
/// - 选中的正交相机被禁用 ⇒ 告警并回退到排序序第一台启用的正交相机（与 3D 侧选中禁用相机同一
///   规则、同一告警形状）；
/// - 没选中、或选中的是**透视**相机 ⇒ 返回 `None`＝**2D 取景保持现状**。这一条很关键：脚本切换
///   3D 相机（最常见操作）绝不该把 2D 相机重置回作者写的初始值——2D 的 center/zoom 归
///   `Camera2dController`（跟随/死区/平滑/抖动/请求），重置会在切相机那一帧闪一下。
pub(super) fn selected_camera_2d_in(
    world: &rustlua_ecs::world::World,
    selection: CameraSelection,
) -> Option<(EntityId, Camera2D)> {
    let mut chosen = None;
    let mut selected_disabled = false;
    world.for_each::<rustlua_camera::camera::Camera>(|id, camera| {
        // 正交相机是 2D 的唯一候选：选中的透视相机在这里被过滤掉（它服务 3D pass）。
        let Some(component) = camera.as_orthographic() else {
            return;
        };
        if !selection.is_selected(id.index() as u64) {
            return;
        }
        if component.enabled {
            chosen = Some((id, component));
        } else {
            selected_disabled = true;
        }
    });
    if chosen.is_some() {
        return chosen;
    }
    if selected_disabled {
        tracing::warn!(
            target: "render",
            "[render] `camera.set_active` 选中的相机 `enabled=false`，2D 视图已回退到下一台可用相机"
        );
        return first_enabled_ortho_camera_in(world);
    }
    None
}

/// Sprite 的世界矩形（pivot+缩放+旋转+位移）投影到屏幕的 AABB，供 UiEvents 命中。
pub(super) fn sprite_screen_rect(
    sprite: &Sprite,
    transform: &Transform,
    camera: OrthoTerms,
) -> Option<([f32; 2], [f32; 2])> {
    // 局部四角：(-pivot*size) 到 ((1-pivot)*size)，经缩放/旋转/位移后投影。
    let pivot = sprite.pivot;
    let size = sprite.size;
    let local = [
        Vec2::new(-pivot.x * size.x, -pivot.y * size.y),
        Vec2::new((1.0 - pivot.x) * size.x, -pivot.y * size.y),
        Vec2::new((1.0 - pivot.x) * size.x, (1.0 - pivot.y) * size.y),
        Vec2::new(-pivot.x * size.x, (1.0 - pivot.y) * size.y),
    ];
    let (sin, cos) = transform.rotation.sin_cos();
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    for corner in local {
        let scaled = Vec2::new(corner.x * transform.scale.x, corner.y * transform.scale.y);
        let rotated = Vec2::new(
            scaled.x * cos - scaled.y * sin,
            scaled.x * sin + scaled.y * cos,
        );
        let world = Vec2::new(
            transform.position.x + rotated.x,
            transform.position.y + rotated.y,
        );
        let screen = camera.world_to_screen(world);
        // 非有限的世界坐标不产出命中矩形（与旧的"投影失败即放弃"同一语义）。
        if !screen.x.is_finite() || !screen.y.is_finite() {
            return None;
        }
        min[0] = min[0].min(screen.x);
        min[1] = min[1].min(screen.y);
        max[0] = max[0].max(screen.x);
        max[1] = max[1].max(screen.y);
    }
    Some((min, [max[0] - min[0], max[1] - min[1]]))
}

impl RuntimeScene {
    /// 换图后同步 2D 相机：重新读新场景声明的 `Camera`，并把 **bounds 换成新地图的**。
    ///
    /// 为什么需要它：`process_scene_transition` 会**重建整个世界**（`ScriptState::world =
    /// World::new()` 后重新实例化），于是切图时场景声明的相机是**新的一份** `Camera2D`；
    /// 而宿主只在 `init`（启动场景）时读过一次 `declared_camera`。结果是"声明的相机只在
    /// 启动场景生效"——每张玩法地图各声明一份 bounds 的工程（`examples/game_jhyt`）里，
    /// 只有开工那张图受约束，之后所有地图的相机都退回"无 bounds"。
    ///
    /// 只取 `bounds` 与 `zoom`，**不取 center**：换图时相机**中心**的权威是每帧的
    /// `camera2d.*` 请求，把它重置回新场景的作者中心会在换图那一帧闪一下（与"场景声明的
    /// 相机是**初始**相机，不是第二份姿态真源"这条既有裁定一致）。新 bounds 在同一个固定
    /// 步里就会被 `step_camera2d` 夹取，因此不会渲染出越界的一帧。
    ///
    /// `zoom` 必须一起取：它与 center 不同，**没有**每帧的属主——控制器只在脚本显式
    /// `camera2d.set_zoom` 时改它（跟随/死区/平滑/抖动都只动 center），因此场景声明就是
    /// 每图取景倍率的唯一真源。换图不读它的话，启动场景没声明相机的工程（门面形态，如
    /// `examples/game_jhyt` 的 `startup_scene = scenes/main.scene`）里声明的 zoom 永远不
    /// 生效：实测从门面进入玩法图后画面停在 zoom=1，只有把玩法图直接当启动场景才看到 2。
    pub(super) fn sync_camera_from_scene(&mut self) {
        // 拆分批 9：同步的是**当前生效**那台的 bounds（显式选中优先，否则声明的第一台）——
        // 脚本显式选了另一台 2D 相机时，换图不该把它的 bounds 换成"第一台"的。
        let Some(declared) = self.effective_camera_2d_payload() else {
            return;
        };
        if self.camera.bounds != declared.bounds {
            tracing::info!(
                target: "render",
                scene = ?self.script.scene_name(),
                bounds = ?declared.bounds,
                "[render] 2D camera bounds refreshed from the new scene"
            );
        }
        self.camera.bounds = declared.bounds;
        if (self.camera.zoom - declared.zoom).abs() > f32::EPSILON {
            tracing::info!(
                target: "render",
                scene = ?self.script.scene_name(),
                zoom = declared.zoom,
                "[render] 2D camera zoom refreshed from the new scene"
            );
            self.camera.zoom = declared.zoom;
        }
    }

    /// 拆分批 9：**本帧生效**的 2D 相机载荷（显式选中优先，否则排序序第一台启用的正交相机）。
    ///
    /// 与 `active_camera3d` 是同一条规则的两个投影分支：同一个 `CameraSelection`、同样的
    /// `enabled` 回退与告警。区别只在"谁拥有运动"——2D 的 center/zoom 之后由
    /// `Camera2dController`（跟随/死区/平滑/抖动/脚本请求）继续决定。
    pub(super) fn effective_camera_2d_payload(&self) -> Option<Camera2D> {
        let selection = CameraSelection {
            active: self.script.active_camera().map(|id| id.index() as u64),
        };
        self.script.with_world(|world| {
            selected_camera_2d_in(world, selection)
                .map(|(_, payload)| payload)
                // 没有选中 2D 相机（未选择 / 选中的是透视相机）时，换图同步的 bounds 取自
                // 场景声明的第一台启用正交相机 —— 与 `init` 的初始种子同一口径。
                .or_else(|| declared_camera_2d_in(world))
        })
    }

    /// 每帧推进 2D 相机控制器：解析跟随目标的世界位置 → 控制器算 center → 复用瓦片图
    /// 那套 `constrained_camera_center` 做 bounds 钳制（避免出现两套夹取规则）。
    ///
    /// 不跟随时控制器原样返回当前中心，因此 `camera2d.set` 的效果不会被覆盖。
    pub(super) fn step_camera2d(&mut self, delta_seconds: f32) {
        // 拆分批 9：`camera.set_active` 是**一条**选择链——它选中的正交相机就是 2D 相机的来源。
        // 选中项**变化**时采用一次它的**载荷**（center/zoom/rotation/bounds）；同一台不再重复采用。
        //
        // 为什么"变化时才采用"而不是"每帧取生效载荷"：2D 的 center/zoom 之后归控制器（跟随/死区/
        // 平滑/抖动/请求），每帧重读作者写的初始值会把控制器算出的取景逐帧抹平——`camera2d.follow`
        // 与 `shake` 会静默失效。为什么"选中的是透视相机时不重置"：切换 3D 相机是常见操作，
        // 它绝不该把 2D 取景弹回作者值（见 `selected_camera_2d_in` 的语义说明）。
        let selection = CameraSelection {
            active: self.script.active_camera().map(|id| id.index() as u64),
        };
        let source = self
            .script
            .with_world(|world| selected_camera_2d_in(world, selection));
        match source {
            Some((id, payload)) if Some(id) != self.adopted_camera_2d_source => {
                self.adopted_camera_2d_source = Some(id);
                tracing::info!(
                    target: "render",
                    center = ?payload.center,
                    zoom = payload.zoom,
                    "[render] 2D camera adopted from the entity selected by camera.set_active"
                );
                self.camera = payload;
            }
            Some(_) => {} // 同一台：不重复采用（否则每帧把控制器的取景抹平）
            None => self.adopted_camera_2d_source = None, // 选择离开 2D 相机 ⇒ 之后再选中会重新采用
        }
        // WP5：请求消费、跟随、advance、bounds 钳制的**次序**现在固定在
        // `Camera2dController::resolve_camera` 内部，宿主不再自己拼这套顺序，也不会出现
        // "宿主与控制器各夹一次 bounds"的分叉。
        let follow = self
            .script
            .camera2d_controller(|controller| controller.follow_target());
        let desired = follow.and_then(|entity| {
            self.script.with_world(|world| {
                // WP2：跟随目标跟的是**世界**位置（父链下的子实体会跟着父一起动）。
                rustlua_ecs::hierarchy::world_transform_2d(world, entity)
                    .ok()
                    .flatten()
                    .map(|transform| transform.position)
            })
        });
        let viewport = Viewport::new(
            self.viewport_width.max(1) as f32,
            self.viewport_height.max(1) as f32,
        )
        .unwrap_or_else(|_| Viewport::new(1.0, 1.0).expect("1x1 viewport is valid"));
        let current = self.camera;
        self.camera = self.script.camera2d_controller_mut(|controller| {
            controller.resolve_camera(current, desired, viewport, delta_seconds)
        });
        // 回填真实状态：`camera2d.get` 必须读到宿主实际在用的值。
        let (center, zoom) = (self.camera.center, self.camera.zoom);
        self.script
            .camera2d_controller_mut(|controller| controller.report(center, zoom));
    }
}
