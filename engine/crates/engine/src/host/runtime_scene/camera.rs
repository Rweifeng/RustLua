//! 2D 相机的选择口径与精灵屏幕矩形（WP12 阶段 2 从 `runtime_scene.rs` 原样搬出）。
//!
//! `first_enabled_ortho_camera_in` 保持私有：它只被本文件的两台相机选择函数共用，搬动后
//! 语义不变——"哪台算场景相机"仍然只有一个答案（见函数自己的注释）。
//! `declared_camera_2d_in` / `selected_camera_2d_in` 被 `mod.rs` 的 `init` 与相机同步调用，
//! `sprite_screen_rect` 被 UI 点击目标收集调用，故标 `pub(super)`。

use crate::core::{
    camera::{Camera2D, CameraSelection, OrthoTerms},
    entity::EntityId,
    sprite::Sprite,
    transform::{Transform, Vec2},
};

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
pub(super) fn declared_camera_2d_in(world: &crate::core::world::World) -> Option<Camera2D> {
    first_enabled_ortho_camera_in(world).map(|(_, camera)| camera)
}

/// 排序序第一台**启用**的正交相机（拆分批 9：这是"默认 2D 相机"的唯一定义处）。
fn first_enabled_ortho_camera_in(
    world: &crate::core::world::World,
) -> Option<(EntityId, Camera2D)> {
    let mut found = None;
    world.for_each::<crate::core::camera::Camera>(|id, camera| {
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
    world: &crate::core::world::World,
    selection: CameraSelection,
) -> Option<(EntityId, Camera2D)> {
    let mut chosen = None;
    let mut selected_disabled = false;
    world.for_each::<crate::core::camera::Camera>(|id, camera| {
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
