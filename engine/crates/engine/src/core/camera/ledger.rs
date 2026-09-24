//! 烘焙**计数账本**（相机拆分批 8）：只存在于测试构建，release 里整个模块被编译掉。
//!
//! ## 为什么要有它
//!
//! docs/54 批 3 写的是"宿主每帧烘焙一次"，但现实是**每个消费者每帧各烘焙一次**：overlay 批、
//! 瓦片层、世界文字、宿主的 overlay 视图各自从同一台 `Camera2D` 算出自己那份 [`OrthoTerms`] /
//! [`CameraView`]。这条口径差多少此前**没人量过**，于是"要不要把 2D 也做成帧级共享"只能靠估算
//! （估算是 <1µs/帧，低于测量噪声 ⇒ 按仓库口径不该动手）。
//!
//! 账本把这件事变成**确定的数字**：测试里钉住"一帧典型路径烘焙了几次"，任何一次增减都会让门
//! 报红，逼改动方说明那是**有意**（同步改数字与本文件的口径）还是**回退**。
//!
//! ## 为什么用 `thread_local!`
//!
//! `cargo test` 的测试函数并行跑在不同线程上：全局计数会被别的测试污染，账本立刻失去意义。
//! 线程局部计数只统计本测试自己触发的烘焙。
//!
//! 时间**不在这里量**：测试里的时间不可信（同机噪声、CI 抖动），仓库对时间的判定统一放在
//! `wp0_*` 基准与 `compare-baseline.ps1` 的阈值里。这里只记次数。

use std::cell::Cell;

thread_local! {
    /// `OrthoTerms::new` 的次数（2D 逐点换算的"预算项"烘焙）。
    static ORTHO_TERMS: Cell<usize> = const { Cell::new(0) };
    /// `CameraView::from_camera2d` 的次数（2D 完整视图：矩阵 + 预算项 + 可见域）。
    static VIEW_2D: Cell<usize> = const { Cell::new(0) };
    /// `CameraView::from_camera3d` 的次数（3D 完整视图）。
    static VIEW_3D: Cell<usize> = const { Cell::new(0) };
    /// **生效 3D 相机被解析的次数**（`RuntimeScene::active_camera3d`）。拆分批 8 之前，一帧里
    /// 渲染提取与 Lua 回填各解析一次（两次必须一致，靠"两步之间没人改世界"这个隐含约定）；
    /// 定帧之后同一帧只解析一次，这个数字就是那条不变量的**账**。
    static CAMERA3D_RESOLUTIONS: Cell<usize> = const { Cell::new(0) };
    /// 本帧定帧值被世界变更作废的次数（`RuntimeScene::invalidate_frame_view_3d`）。
    static FRAME_VIEW_INVALIDATIONS: Cell<usize> = const { Cell::new(0) };
}

/// 一次记账窗口内的烘焙次数。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct BakeLedger {
    /// `OrthoTerms::new` 次数。
    pub(crate) ortho_terms: usize,
    /// `CameraView::from_camera2d` 次数（其中每次内部还会各算一份 `OrthoTerms`）。
    pub(crate) view_2d: usize,
    /// `CameraView::from_camera3d` 次数。
    pub(crate) view_3d: usize,
    /// 生效 3D 相机被解析的次数（同帧应为 1）。
    pub(crate) camera3d_resolutions: usize,
    /// 定帧值被作废的次数。
    pub(crate) frame_view_invalidations: usize,
}

/// 清零本线程的账本（一次记账窗口的开始）。
pub(crate) fn reset() {
    ORTHO_TERMS.with(|value| value.set(0));
    VIEW_2D.with(|value| value.set(0));
    VIEW_3D.with(|value| value.set(0));
    CAMERA3D_RESOLUTIONS.with(|value| value.set(0));
    FRAME_VIEW_INVALIDATIONS.with(|value| value.set(0));
}

/// 读本线程累计的烘焙次数（不清零）。
pub(crate) fn snapshot() -> BakeLedger {
    BakeLedger {
        ortho_terms: ORTHO_TERMS.with(Cell::get),
        view_2d: VIEW_2D.with(Cell::get),
        view_3d: VIEW_3D.with(Cell::get),
        camera3d_resolutions: CAMERA3D_RESOLUTIONS.with(Cell::get),
        frame_view_invalidations: FRAME_VIEW_INVALIDATIONS.with(Cell::get),
    }
}

pub(crate) fn count_ortho_terms() {
    ORTHO_TERMS.with(|value| value.set(value.get() + 1));
}

pub(crate) fn count_view_2d() {
    VIEW_2D.with(|value| value.set(value.get() + 1));
}

pub(crate) fn count_view_3d() {
    VIEW_3D.with(|value| value.set(value.get() + 1));
}

pub(crate) fn count_camera3d_resolution() {
    CAMERA3D_RESOLUTIONS.with(|value| value.set(value.get() + 1));
}

pub(crate) fn count_frame_view_invalidations() {
    FRAME_VIEW_INVALIDATIONS.with(|value| value.set(value.get() + 1));
}
