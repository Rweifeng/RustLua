//! 游戏工程路径解析。当前默认示例工程是仓库内 `examples`；少量大型字体
//! 测试 fixture 仍在仓库外 `games` 目录，使用 `fixture_games_dir` 显式解析。

use std::path::PathBuf;

/// 当前游戏工程根目录（仓库内 examples）。
///
/// 解析顺序：
/// 1. 环境变量 `P2_GAMES_DIR`（显式指定，最优先）；
/// 2. 按仓库布局推导：`CARGO_MANIFEST_DIR` 向上 3 级到仓库根
///    （即 `E:\Codex\RustLua\engine\crates\engine` -> `E:\Codex\RustLua`）。
pub fn games_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("P2_GAMES_DIR") {
        return PathBuf::from(dir);
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(std::path::Path::parent)
        .and_then(std::path::Path::parent)
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 仓库外测试 fixture 所在目录（ui_demo / tetris / game_farm 等）。
pub fn fixture_games_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("P2_FIXTURES_DIR") {
        return PathBuf::from(dir);
    }
    games_dir()
        .parent()
        .map(|github| github.join("games"))
        .unwrap_or_else(|| PathBuf::from(r"C:\GitHub\games"))
}
