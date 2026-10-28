//! UI 文档宿主与 UI Studio 的无头状态层。
//!
//! 这层刻意不依赖 egui / eframe，也不认识编辑器 shell（`EditorApp`）：
//! `.ui.json` 的读写/revision/校验/预览/提案、UI Studio 的层级/检查器/预览/提案箱，
//! 以及 UiSpec → `.scene` 的确定性导出都在这里。egui 面板只做绘制，MCP 适配层与测试
//! 共用同一份实现。

pub mod review;
/// Spec → `.scene` 的确定性导出搬到了 [`rustlua_ui_scene_export`]（wasm 侧证据 harness 也要用
/// 同一份导出）。这里以原路径转出，`editor::ui_scene_export` 的调用面不变。
pub use rustlua_ui_scene_export as ui_scene_export;
pub mod ui_spec_host;
pub mod ui_studio;

/// 测试用唯一令牌：进程 ID + 原子计数 + 时间戳。
///
/// 与 `rustlua-editor-core::test_token` 同一个理由（Windows 时钟粒度下并行测试会撞
/// 临时目录），但各 crate 的测试二进制彼此独立，不能共享一个 `pub(crate)` 帮助函数。
#[cfg(test)]
pub(crate) fn test_token() -> u128 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = u128::from(COUNTER.fetch_add(1, Ordering::Relaxed));
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    (u128::from(std::process::id()) << 96) ^ (counter << 64) ^ (nanos & 0xFFFF_FFFF_FFFF_FFFF)
}
