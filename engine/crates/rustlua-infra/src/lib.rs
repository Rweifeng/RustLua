pub mod atlas_redirect;
pub mod bundle;
/// 分包配置与归约（`bundles.json` → 分组 → 计划表）：`assetc` 与编辑器面板共用一份规则。
pub mod bundle_plan;
pub mod editor_plugins;
pub mod frame_clock;
// `logging` 模块**必须两个 target 都在**：渲染器（`engine/src/host/renderer/passes.rs`）
// 在 wasm 上也用它的 `EngineError` / `LogModule`。真正只有原生需要的订阅者装配
// （registry + fmt + `EnvFilter`）在 `logging.rs` 内部按 `cfg(not(wasm32))` 关掉了。
pub mod logging;
pub mod pack;
pub mod par;
pub mod paths;
pub mod performance;
pub mod preview_diagnostics;
pub mod project_config;
pub mod save_slot;
pub mod time_step;
pub mod user_dir;
pub mod vfs;
pub mod window;

pub use paths::fixture_games_dir;
pub use paths::games_dir;

/// Returns the engine core package version.
#[must_use]
pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Per-project working directory holding local runtime data: the resource
/// database, import cache, and save data. Local and gitignored.
pub const PROJECT_WORK_DIR: &str = ".cache";
