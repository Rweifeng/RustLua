//! 每帧渲染契约：怎么投影（[`Projection`]）、投到多大视口、清屏策略与绘制顺序
//! （[`RenderView`] / [`FrameViews`]）。
//!
//! 渲染器消费这些类型而**不回查 ECS**：相机的姿态与投影参数由宿主从组件取出后在这里成帧。
//! "2D 用另一套视图说法"这类分叉从类型上就不存在——两个家族共用同一套字段。

use super::bake::CameraView;
use super::component::{Camera2D, Camera3D, Viewport};
use crate::core::math3d::Mat4;

/// 统一投影描述（WP5）：正交（2D 世界）与透视（3D 世界）的唯一说法。
///
/// 相机可以跨 2D 与 3D 复用，但**不强制**两者共用 shader 与深度规则——本类型只回答
/// "怎么投影"，不回答"画出来长什么样"（那是渲染器与 [`RenderView`] 的事）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Projection {
    /// 正交：`zoom` 是"每世界单位多少像素"，与 [`Camera2D`] 一致。
    Orthographic { zoom: f32 },
    /// 透视：`fov_y` 是**垂直**视场角（弧度），与 [`Camera3D`] 一致。
    Perspective { fov_y: f32, near: f32, far: f32 },
}

impl Projection {
    /// 从 2D 相机的 zoom 构造。
    #[must_use]
    pub const fn from_camera2d(camera: Camera2D) -> Self {
        Self::Orthographic { zoom: camera.zoom }
    }

    /// 从 3D 相机构造。
    #[must_use]
    pub const fn from_camera3d(camera: Camera3D) -> Self {
        Self::Perspective {
            fov_y: camera.fov_y,
            near: camera.near,
            far: camera.far,
        }
    }

    /// 该投影是否需要深度（透视要，2D 正交不参与世界深度）。
    #[must_use]
    pub const fn uses_depth(self) -> bool {
        matches!(self, Self::Perspective { .. })
    }

    /// 是否符合"2D 世界"的规则（世界 y 向上、不参与深度、像素对齐由 zoom 决定）。
    #[must_use]
    pub const fn is_orthographic(self) -> bool {
        matches!(self, Self::Orthographic { .. })
    }
}

/// 清屏策略：渲染视图自己声明要不要清、清成什么颜色（此前分散在渲染器与编辑器里）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ClearPolicy {
    /// 不清屏（叠加到已有内容之上，例如 2D overlay 覆在 3D pass 之后）。
    None,
    Color([f32; 4]),
}

/// 3D 主视图的默认清屏色（线性空间）。渲染器此前把它写死在 pass 里；现在它既是渲染器
/// 未收到任何视图声明时的历史默认，也是宿主 `RenderView::main_3d` 的默认值来源。
pub const DEFAULT_3D_CLEAR: [f32; 4] = [0.04, 0.07, 0.12, 1.0];

/// 渲染视图（WP5）：一帧渲染所需的相机侧描述——视口、投影、清屏策略、绘制顺序与可见遮罩。
///
/// 目标与 docs/40 §3 WP5 一致：**渲染器消费快照，而不回查 ECS**。2D 与 3D 各一份视图，
/// 共用同一套字段，因此"2D 用另一套投影/视口说法"这类分叉从类型上就不存在了。
///
/// 矩阵**不在这里另存一份**：它由 [`CameraView`]（本帧的烘焙结果）给出，因此"投影参数与
/// 矩阵互相矛盾"这种状态从类型上不可表达——只有构造时烘焙成功与否两种情形。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderView {
    pub projection: Projection,
    pub viewport: Viewport,
    pub clear: ClearPolicy,
    /// 同帧内多个视图的绘制顺序（小的先画）。
    pub order: i32,
    /// 可见层遮罩（按位与场景层比较；`u32::MAX` = 全可见）。
    pub visibility: u32,
    /// 本帧的**烘焙结果**（世界 → 裁剪空间矩阵 + 正交预算项 + 可见域）。
    ///
    /// docs/54 §8-③：渲染器消费的必须是这一份契约，而不是自己回查 `Camera3D` 或宿主另塞一份
    /// 矩阵。`None` = 这台相机当前退化（位置与目标重合、zoom 非正等），渲染器此时退回历史
    /// 路径并留下诊断，不猜一个矩阵。
    view: Option<CameraView>,
}

/// 正交视图（2D 主视图 / overlay）的默认裁剪深度范围。
///
/// `z = 0` 因此落在深度**中间**，向 -z 靠近相机、向 +z 远离，两侧各 1 个世界单位可用
/// （2D 的层间偏移通常远小于此）。世界 Sprite 的 `Sprite::z` 也按这个范围解释。
pub const ORTHO_NEAR: f32 = -1.0;
/// 见 [`ORTHO_NEAR`]。
pub const ORTHO_FAR: f32 = 1.0;

impl RenderView {
    /// 2D overlay 视图：正交投影 + 不清屏（叠在 3D pass 之上）。
    #[must_use]
    pub fn overlay_2d(camera: Camera2D, viewport: Viewport) -> Self {
        Self::from_baked(
            Projection::from_camera2d(camera),
            viewport,
            ClearPolicy::None,
            1,
            CameraView::from_camera2d(camera, viewport).ok(),
        )
    }

    /// 主 3D 视图：透视投影 + 用给定颜色清屏。
    #[must_use]
    pub fn main_3d(camera: Camera3D, viewport: Viewport, clear: [f32; 4]) -> Self {
        Self::from_baked(
            Projection::from_camera3d(camera),
            viewport,
            ClearPolicy::Color(clear),
            0,
            CameraView::from_camera3d(camera, viewport).ok(),
        )
    }

    /// 主 2D 视图：正交投影 + 用给定颜色清屏（编辑器 Scene 2D 视图、纯 2D 游戏画面）。
    ///
    /// "主/叠加"说的是**清屏策略**而不是 2D/3D：2D 画面也可以是这一帧的底，需要先清屏。
    #[must_use]
    pub fn main_2d(camera: Camera2D, viewport: Viewport, clear: [f32; 4]) -> Self {
        Self::from_baked(
            Projection::from_camera2d(camera),
            viewport,
            ClearPolicy::Color(clear),
            0,
            CameraView::from_camera2d(camera, viewport).ok(),
        )
    }

    /// **相机退化**时的视图：框架还在（清屏、视口、顺序照旧生效），但**没有矩阵**。
    ///
    /// 只有"声明了视图却拿不到矩阵"这一种情形该用它（渲染器据此走历史回退分支，见
    /// [`RenderView::matrix`]）。正常路径用 [`RenderView::overlay_2d`] / `main_2d` / `main_3d`。
    #[must_use]
    pub const fn degenerate(
        projection: Projection,
        viewport: Viewport,
        clear: ClearPolicy,
        order: i32,
        visibility: u32,
    ) -> Self {
        Self {
            projection,
            viewport,
            clear,
            order,
            visibility,
            view: None,
        }
    }

    const fn from_baked(
        projection: Projection,
        viewport: Viewport,
        clear: ClearPolicy,
        order: i32,
        view: Option<CameraView>,
    ) -> Self {
        Self {
            projection,
            viewport,
            clear,
            order,
            visibility: u32::MAX,
            view,
        }
    }

    /// 本视图的视图投影矩阵（世界 → 裁剪空间）。退化相机返回 `None`。
    #[must_use]
    pub fn matrix(self) -> Option<Mat4> {
        self.view.map(CameraView::matrix)
    }

    /// 本帧的烘焙结果（渲染器需要视锥、逆矩阵或 NDC 换算时用它，而不是自己重算）。
    #[must_use]
    pub const fn baked(self) -> Option<CameraView> {
        self.view
    }

    /// 以本视图为模板改**清屏策略**（宿主对同一视图声明另一种清屏时用）。
    ///
    /// 提供它是为了不把烘焙结果暴露成可写字段：宿主能改的只有"框架"部分，
    /// 矩阵与投影参数始终来自同一次烘焙。
    #[must_use]
    pub const fn with_clear(mut self, clear: ClearPolicy) -> Self {
        self.clear = clear;
        self
    }

    /// 以本视图为模板改**绘制顺序**。
    #[must_use]
    pub const fn with_order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    /// 视口宽高比（透视投影需要）。
    #[must_use]
    pub fn aspect(self) -> f32 {
        self.viewport.width / self.viewport.height
    }
}

/// 一帧交给渲染器的视图集合（WP5 的接线边界）。
///
/// 引擎一帧最多两类视图：3D 主视图（清屏 + 深度）与 2D overlay（叠在 3D 之上、不清屏）。
/// `main` 为 `None` 表示宿主**没有声明**主视图：渲染器保持历史默认（默认清屏色、整目标
/// 视口），因此"不调用 `set_frame_views`"就是这次收敛的回滚档——像素与收敛前逐字节相同。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameViews {
    /// 3D 主视图：决定清屏与深度 pass 的视口。
    pub main: Option<RenderView>,
    /// 2D overlay 视图：决定 2D pass 的视口（可以是目标的一个子矩形）。
    pub overlay: Option<RenderView>,
}

impl FrameViews {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            main: None,
            overlay: None,
        }
    }

    /// 主 3D 视图 + 2D overlay 的标准组合（`order` 小的先画）。
    #[must_use]
    pub const fn layered(main: RenderView, overlay: RenderView) -> Self {
        Self {
            main: Some(main),
            overlay: Some(overlay),
        }
    }

    /// 只有 2D overlay 的纯 2D 场景。
    #[must_use]
    pub const fn overlay_only(overlay: RenderView) -> Self {
        Self {
            main: None,
            overlay: Some(overlay),
        }
    }

    /// 本帧的清屏策略：**顺序最前**的那个已声明视图说了算（`order` 小的先；相等时 `main` 优先）。
    ///
    /// docs/54 §4.2：`RenderView::order` 此前是个**没有读者**的字段（清屏硬编码取 `main`）——与 §4.2
    /// 要求的"视图包含 render order"不符。现在顺序规则只有一处（[`Self::ordered`]），清屏由它推出：
    /// 声明一个 `order = -1` 的 2D 背景视图并给它 `ClearPolicy::Color`，这一帧就由它清屏。
    /// 既有宿主不受影响（`main.order = 0` 恒为最小，除非宿主显式声明更小的顺序）。
    #[must_use]
    pub fn clear(self) -> Option<ClearPolicy> {
        self.ordered()
            .into_iter()
            .flatten()
            .next()
            .map(|view| view.clear)
    }

    /// 按 `order` 升序排列的已声明视图（未声明的排在最后并被跳过）。
    #[must_use]
    pub fn ordered(self) -> [Option<RenderView>; 2] {
        let mut views = [self.main, self.overlay];
        views.sort_by_key(|view| view.map_or(i32::MAX, |view| view.order));
        views
    }
}
