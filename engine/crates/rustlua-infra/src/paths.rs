//! 游戏工程路径解析。当前默认示例工程是仓库内 `examples`；少量大型字体
//! 测试 fixture 仍在仓库外 `games` 目录，使用 `fixture_games_dir` 显式解析。
//!
//! 这里同时放**工程内相对路径**的两个权威判定（[`crate::paths::is_absolute_path`] /
//! [`crate::paths::escapes_project_root`]）：运行时脚本路径校验（`entity_script`）与 schema 的
//! `FieldRule::ProjectPath` 都用它们，避免"同一规则两处实现、一处漏了 Windows 根路径"。
//! 这两个判定确实曾经不一致：只查 `Path::is_absolute()` 时，`/etc/passwd` 在 Windows 上
//! 被当成相对路径（没有盘符），而引擎的组件式检查会正确拒绝它。

use std::path::{Component, Path, PathBuf};

/// 当前游戏工程根目录（仓库内 examples）。
///
/// 解析顺序：
/// 1. 环境变量 `RUSTLUA_GAMES_DIR`（显式指定，最优先）；
/// 2. 按仓库布局推导：`CARGO_MANIFEST_DIR` 向上 3 级到仓库根
///    （即 `E:\Codex\RustLua\engine\crates\engine` -> `E:\Codex\RustLua`）。
pub fn games_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("RUSTLUA_GAMES_DIR") {
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
    if let Some(dir) = std::env::var_os("RUSTLUA_FIXTURES_DIR") {
        return PathBuf::from(dir);
    }
    games_dir()
        .parent()
        .map(|github| github.join("games"))
        .unwrap_or_else(|| PathBuf::from(r"C:\GitHub\games"))
}

/// 是否是**绝对路径**（含 Windows 的 `/x` 根相对形式）。
///
/// `Path::is_absolute()` 在 Windows 上对 `/etc/passwd` 返回 `false`（没有盘符/UNC 前缀），
/// 但它的第一个组件是 `RootDir`——从"能不能逃出工程根"的角度看它就是绝对的。
/// 因此两个判定合起来看：绝对前缀**或**根目录组件。
#[must_use]
pub fn is_absolute_path(raw: &str) -> bool {
    let path = Path::new(raw);
    path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::RootDir | Component::Prefix(_)))
}

/// 路径是否会逃出工程根（含 `..`）。
#[must_use]
pub fn escapes_project_root(raw: &str) -> bool {
    Path::new(raw)
        .components()
        .any(|component| matches!(component, Component::ParentDir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_paths_cover_windows_root_relative_forms() {
        for raw in ["/etc/passwd", "/x.lua", r"C:\Windows\x.lua", "C:/x.lua"] {
            assert!(is_absolute_path(raw), "{raw:?} 必须被判为绝对路径");
        }
        for raw in ["scripts/x.lua", "x.lua", "./x.lua", "assets/a/b.png"] {
            assert!(!is_absolute_path(raw), "{raw:?} 是相对路径");
        }
    }

    #[test]
    fn traversal_is_detected_in_any_position() {
        for raw in ["../x.lua", "a/../b.lua", "a/b/../../../c.lua"] {
            assert!(escapes_project_root(raw), "{raw:?} 有 ..");
        }
        for raw in ["a/b.lua", "scripts/x.lua", "..hidden/x.lua"] {
            assert!(!escapes_project_root(raw), "{raw:?} 没有 .. 段");
        }
    }
}
