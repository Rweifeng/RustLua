pub mod atlas_redirect;
pub mod bundle;
/// 分包配置与归约（`bundles.json` → 分组 → 计划表）：`assetc` 与编辑器面板共用一份规则。
pub mod bundle_plan;
pub mod editor_plugins;
pub mod frame_clock;
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
