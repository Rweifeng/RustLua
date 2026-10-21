//! `Renderer` 的测试集合：WP12 阶段 4a 从 `renderer/mod.rs` 末尾的 `#[cfg(test)] mod tests`
//! 原样搬来，按域分成 7 个文件（源文件 6,089 行里这一段占 1,847 行，超过计划 §2 的 1,500 行
//! 上限）。断言、造数据与离屏渲染路径一字未改。
//!
//! 可见性依据（计划 §3.1.5）：本目录里的每个模块都是 `renderer` 的后代，`use super::*;` 的
//! glob 会把父模块的**私有项**连同本模块的 `use` 一并带下来，因此拆分没有放宽任何可见性；
//! 原先声明在本模块顶部的 helper 也仍在父作用域可见（本目录内没有测试之外的 helper）。
//! 这行措辞是刻意的：写成属性字面量会让 `grep -c '#\[test\]'` 的计数虚高 1。

use super::*;
use rustlua_camera::camera::{Camera2D, ClearPolicy, DEFAULT_3D_CLEAR, Projection, RenderView};
use rustlua_ecs::transform::Vec2;
use rustlua_math::math3d::Quat;

use crate::host::frame::OverlayVertex;
// WP12 阶段 4d：这两个名字原先由 `renderer/mod.rs` 顶部的 use 块带进来（测试靠 glob 取用），
// 构造代码搬走后 `mod.rs` 不再需要它们，就在这里显式声明。
use crate::host::frame::WorldSpriteVertex;
use wgpu::DeviceDescriptor;

mod capacity_and_caches;
mod compute;
mod depth_and_frame_views;
mod offscreen_and_tetris;
mod overlay_batching;
mod passes;
mod readback;
mod render_metrics;
mod texture_upload;
mod world_sprite;

/// GPU 用例在**没有适配器**时的显式跳过。原来这里写的是裸 `return;` 加一句 `// no GPU available; skip` 注释。
///
/// 为什么不能只是 `return;`：`cargo test` 对早退的用例**照样报 `ok`**，于是
/// "engine --lib 160 通过"可能包含十几个**一行没跑**的用例——正是本仓最忌讳的"跳过当通过"。
/// 这里做两件事：
///
/// 1. 默认打印一行 `gpu_skipped` 标记：配 `cargo test -- --nocapture` 可见、可被门统计。
///    注意 libtest 默认**捕获成功用例的 stdout**，所以不配 `--nocapture` 是看不见的——
///    这正是第 2 条存在的原因。
/// 2. 设了 **`RUSTLUA_REQUIRE_GPU=1`** 时**跳过即 panic**：在"应该有 GPU"的机器与发布门上，
///    把"悄悄没跑"变成硬失败，而不是记一个通过。
///
/// 第 2 条为什么必要：CI runner 没有 GPU，这类用例**在 CI 上恒为跳过**——也就是渲染器的 GPU 路径
/// **只有本地门能覆盖**；而本地门若不设这个变量，同样只是静默跳过。
fn gpu_skipped(reason: &str) {
    assert!(
        std::env::var("RUSTLUA_REQUIRE_GPU").as_deref() != Ok("1"),
        "RUSTLUA_REQUIRE_GPU=1 但 GPU 不可用（{reason}）：这条用例一行都没跑，不许记成通过"
    );
    println!("gpu_skipped reason={reason}");
}
