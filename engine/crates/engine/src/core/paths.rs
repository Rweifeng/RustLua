//! 游戏工程路径解析：游戏工程（game_test / game_test_2d_production / game_farm）
//! 已迁出仓库，位于 `C:\GitHub\games`。这里提供统一解析，避免各处硬编码相对路径。

use std::path::PathBuf;

/// 游戏工程根目录（game_test / game_test_2d_production / game_farm 所在）。
///
/// 解析顺序：
/// 1. 环境变量 `P2_GAMES_DIR`（显式指定，最优先）；
/// 2. 按仓库布局推导：`CARGO_MANIFEST_DIR` 向上 4 级（到 GitHub）再进 `games`
///    （即 `C:\GitHub\p2\engine\crates\engine` → `C:\GitHub\games`）；
/// 3. 回退固定位置 `C:\GitHub\games`。
pub fn games_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("P2_GAMES_DIR") {
        return PathBuf::from(dir);
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
        .map(|github| github.join("games"))
        .unwrap_or_else(|| PathBuf::from(r"C:\GitHub\games"))
}

/// 编译期游戏工程根目录前缀（供 `include_bytes!` 等需要字面量的场景）。
/// 相对 `CARGO_MANIFEST_DIR` 向上 4 级到 GitHub 再进 `games`（各 crate 一致）。
pub const GAMES_DIR_RELATIVE: &str = "../../../../games";
