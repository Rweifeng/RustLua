//! 2D 相机控制器：跟随、偏移、死区、平滑、抖动，以及"脚本请求 → 宿主消费 → 回填真实值"的交接。
//!
//! 它是**策略**而不是相机数学：只决定 center 与 zoom，rotation 与 bounds 原样保留。
//! 它不认识 ECS（跟随目标的世界坐标由宿主从 ECS 取好喂进来），也不认识渲染。

use super::component::{Camera2D, Viewport};
use super::view::RenderView;
use crate::core::{entity::EntityId, transform::Vec2};

/// 单帧最大步长：卡帧/断点恢复后不让相机瞬移一大段（也让平滑与抖动的表现可预期）。
const MAX_STEP_SECONDS: f32 = 0.1;
/// 抖动的两个频率取互质近似值，避免 x/y 同相位导致抖动看起来像单轴摆动。
const SHAKE_FREQ_X: f32 = 37.0;
const SHAKE_FREQ_Y: f32 = 53.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera2dController {
    /// 跟随目标实体。`None` = 不跟随（相机中心由脚本或宿主直接设置）。
    follow: Option<EntityId>,
    /// 目标相对屏幕中心的偏移（像素）。
    offset: Vec2,
    /// 死区半宽/半高（像素）。目标在死区内时相机不动。
    dead_zone: Vec2,
    /// 平滑系数（每秒收敛率）。`0` = 立即跟随。
    smoothing: f32,
    shake_amplitude: f32,
    shake_remaining: f32,
    shake_duration: f32,
    elapsed: f32,
    /// 脚本请求的相机中心（`camera2d.set`）。宿主每帧消费一次。
    ///
    /// 相机数据在宿主（`RuntimeScene::camera`）而不在 Lua 状态里，因此这里用
    /// "脚本请求 → 宿主消费"的有界交接，与 `camera.set_active` 的既有做法一致。
    pending_center: Option<Vec2>,
    /// 脚本请求的缩放。
    pending_zoom: Option<f32>,
    /// 宿主每帧回填的真实值，供 `camera2d.get` / `get_zoom` 读到真数而非脚本的自述。
    reported_center: Vec2,
    reported_zoom: f32,
}

impl Default for Camera2dController {
    fn default() -> Self {
        Self {
            follow: None,
            offset: Vec2::zero(),
            dead_zone: Vec2::zero(),
            smoothing: 0.0,
            shake_amplitude: 0.0,
            shake_remaining: 0.0,
            shake_duration: 0.0,
            elapsed: 0.0,
            pending_center: None,
            pending_zoom: None,
            reported_center: Vec2::zero(),
            reported_zoom: 1.0,
        }
    }
}

impl Camera2dController {
    #[must_use]
    pub const fn follow_target(&self) -> Option<EntityId> {
        self.follow
    }

    #[must_use]
    pub const fn smoothing(&self) -> f32 {
        self.smoothing
    }

    #[must_use]
    pub const fn offset(&self) -> Vec2 {
        self.offset
    }

    #[must_use]
    pub const fn dead_zone(&self) -> Vec2 {
        self.dead_zone
    }

    /// 开始跟随实体，或传 `None` 取消跟随。
    pub fn follow(&mut self, target: Option<EntityId>) {
        self.follow = target;
    }

    pub fn set_offset(&mut self, offset: Vec2) {
        self.offset = offset;
    }

    pub fn set_dead_zone(&mut self, dead_zone: Vec2) {
        self.dead_zone = dead_zone;
    }

    /// 设置平滑系数（每秒收敛率）；非有限或负数按 0（立即跟随）处理。
    pub fn set_smoothing(&mut self, smoothing: f32) {
        self.smoothing = if smoothing.is_finite() && smoothing > 0.0 {
            smoothing
        } else {
            0.0
        };
    }

    /// 触发一次抖动：`amplitude` 像素、`duration` 秒，强度随剩余时间线性衰减。
    ///
    /// 同一 `(amplitude, duration)` 与同一帧序列下结果**确定**（相位由累计时间决定，
    /// 不用随机数），因此不会破坏规则回归的确定性。
    pub fn shake(&mut self, amplitude: f32, duration: f32) {
        if !amplitude.is_finite() || !duration.is_finite() || amplitude == 0.0 || duration <= 0.0 {
            return;
        }
        self.shake_amplitude = amplitude.abs();
        self.shake_duration = duration;
        self.shake_remaining = duration;
    }

    #[must_use]
    pub fn is_shaking(&self) -> bool {
        self.shake_remaining > 0.0
    }

    /// 脚本请求把相机中心设为 `center`（下一帧被宿主消费）。有限性由调用方校验。
    pub fn request_center(&mut self, center: Vec2) {
        self.pending_center = Some(center);
    }

    /// 宿主取走脚本请求的中心（取一次即清，避免重复应用）。
    pub fn take_pending_center(&mut self) -> Option<Vec2> {
        self.pending_center.take()
    }

    /// 脚本请求缩放。
    pub fn request_zoom(&mut self, zoom: f32) {
        self.pending_zoom = Some(zoom);
    }

    pub fn take_pending_zoom(&mut self) -> Option<f32> {
        self.pending_zoom.take()
    }

    /// 宿主回填真实相机状态，供 `camera2d.get` / `get_zoom` 读取。
    pub fn report(&mut self, center: Vec2, zoom: f32) {
        self.reported_center = center;
        self.reported_zoom = zoom;
    }

    #[must_use]
    pub const fn center(&self) -> Vec2 {
        self.reported_center
    }

    #[must_use]
    pub const fn zoom(&self) -> f32 {
        self.reported_zoom
    }

    /// 推进一帧，返回本帧相机中心。
    ///
    /// `desired` 是跟随目标的世界坐标（由调用方从 ECS 取；不跟随时传 `None`）。
    pub fn advance(&mut self, current: Vec2, desired: Option<Vec2>, delta_seconds: f32) -> Vec2 {
        let dt = if delta_seconds.is_finite() {
            delta_seconds.clamp(0.0, MAX_STEP_SECONDS)
        } else {
            0.0
        };
        self.elapsed += dt;

        let mut center = match desired {
            Some(desired) if self.follow.is_some() => {
                let desired = desired + self.offset;
                let target = Vec2::new(
                    advance_axis(current.x, desired.x, self.dead_zone.x),
                    advance_axis(current.y, desired.y, self.dead_zone.y),
                );
                if self.smoothing > 0.0 {
                    // 指数收敛：与帧率无关（同样的总时长给同样的结果）。
                    let blend = 1.0 - (-self.smoothing * dt).exp();
                    current + (target - current) * blend
                } else {
                    target
                }
            }
            _ => current,
        };

        if self.shake_remaining > 0.0 {
            self.shake_remaining = (self.shake_remaining - dt).max(0.0);
            let fade = if self.shake_duration > 0.0 {
                self.shake_remaining / self.shake_duration
            } else {
                0.0
            };
            let amplitude = self.shake_amplitude * fade;
            center.x += (self.elapsed * SHAKE_FREQ_X).sin() * amplitude;
            center.y += (self.elapsed * SHAKE_FREQ_Y + 1.7).sin() * amplitude;
            if self.shake_remaining <= 0.0 {
                self.shake_amplitude = 0.0;
            }
        }
        center
    }
}

/// 死区内的单轴推进：超出死区时把相机推到"目标刚好贴住死区边界"的位置。
fn advance_axis(current: f32, desired: f32, dead_zone: f32) -> f32 {
    let delta = desired - current;
    let dead_zone = dead_zone.max(0.0);
    if delta.abs() <= dead_zone {
        return current;
    }
    current + (delta.abs() - dead_zone) * delta.signum()
}

impl Camera2dController {
    /// 推进一帧并返回**本帧应当渲染用的相机**（WP5 收敛后的新边界）。
    ///
    /// 收敛前这段顺序散在宿主里：先取脚本请求的中心/缩放、再取跟随目标、再 advance、最后
    /// 用 `constrained_camera_center` 夹到 bounds。现在它只有一个入口，并且**先应用请求、
    /// 再跟随、最后夹取**的顺序被固定在类型内部——宿主不再需要知道这个次序。
    ///
    /// `follow_target` 是跟随目标的世界坐标（调用方从 ECS 取；不跟随或没有 `Transform`
    /// 时传 `None`）。`current` 的 rotation 与 bounds 原样保留：控制器只决定 center 与 zoom。
    pub fn resolve_camera(
        &mut self,
        current: Camera2D,
        follow_target: Option<Vec2>,
        viewport: Viewport,
        delta_seconds: f32,
    ) -> Camera2D {
        let mut camera = current;
        // 脚本的显式请求优先（`camera2d.set` 语义是"立刻到这儿"），取走即清。
        if let Some(center) = self.take_pending_center() {
            camera.center = center;
        }
        if let Some(zoom) = self.take_pending_zoom() {
            camera.zoom = zoom;
        }
        let advanced = self.advance(camera.center, follow_target, delta_seconds);
        camera.center = advanced;
        // bounds 钳制复用相机自己的那一份换算（与剔除用的可见域同源）。
        camera.center = camera.constrained_center(viewport).unwrap_or(advanced);
        camera
    }

    /// 推进一帧并直接给出本帧的 [`crate::core::camera::RenderView`]（2D overlay 视图）。
    ///
    /// 渲染器消费 `RenderView` 而不回查 ECS/宿主，这是 WP5 要求的形态；宿主侧接线可以分步
    /// 进行，但"相机 + 视口 + 清屏 + 顺序"的说法从这一处产生。
    pub fn resolve_view(
        &mut self,
        current: Camera2D,
        follow_target: Option<Vec2>,
        viewport: Viewport,
        delta_seconds: f32,
    ) -> RenderView {
        let camera = self.resolve_camera(current, follow_target, viewport, delta_seconds);
        RenderView::overlay_2d(camera, viewport)
    }
}
