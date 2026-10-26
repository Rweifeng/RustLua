//! 游戏工程路径解析。当前默认示例工程是仓库内 `examples`；少量大型字体
//! 测试 fixture 仍在仓库外 `games` 目录，使用 `fixture_games_dir` 显式解析。
//!
//! 这里同时放**工程内相对路径**的两个权威判定（[`crate::paths::is_absolute_path`] /
//! [`crate::paths::escapes_project_root`]）：运行时脚本路径校验（`entity_script`）与 schema 的
//! `FieldRule::ProjectPath` 都用它们，避免"同一规则两处实现、一处漏了 Windows 根路径"。
//! 这两个判定确实曾经不一致：只查 `Path::is_absolute()` 时，`/etc/passwd` 在 Windows 上
//! 被当成相对路径（没有盘符），而引擎的组件式检查会正确拒绝它。

use std::path::{Component, Path, PathBuf};

/// 项目里放 shader 覆盖文件的目录名（计划 §6.5）。
///
/// 权威定义在这里而不是渲染 crate：编辑器领域层的文件监听（`rustlua-editor-core`）
/// 也要把它列进默认监听根，但那一层不该为了一个目录名依赖 `rustlua-gpu`（wgpu）。
/// 运行时默认路径、报错文案与监听根都读同一个常量。
pub const SHADER_SOURCE_DIR: &str = "shaders";

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

/// 规范化工程内资源路径，返回统一的 `/` 分隔相对路径。
///
/// 所有宿主（桌面、WebGL、微信）共享这一口径：拒绝绝对路径、盘符、`..`、空路径
/// 和 NUL；允许 `.` 与重复分隔符，但输出只保留规范化后的相对路径。
#[must_use]
pub fn normalize_project_path(raw: &str) -> Option<String> {
    if raw.is_empty() || raw.contains('\0') || is_absolute_path(raw) {
        return None;
    }
    let normalized = raw.replace('\\', "/");
    let mut parts = Vec::new();
    for part in normalized.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            value => parts.push(value),
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// 把工程根与工程内资源路径安全拼接。
#[must_use]
pub fn join_project_path(root: &Path, raw: &str) -> Option<PathBuf> {
    normalize_project_path(raw).map(|relative| root.join(relative))
}

#[cfg(test)]
mod normalized_tests {
    use super::*;

    #[test]
    fn normalize_project_paths_has_one_cross_platform_key() {
        assert_eq!(
            normalize_project_path(r"scripts\\./data//items.lua"),
            Some("scripts/data/items.lua".to_owned())
        );
        assert_eq!(
            normalize_project_path("assets/a.png"),
            Some("assets/a.png".to_owned())
        );
        for raw in ["", ".", "../x", "a/../../x", "/x", r"C:\\x", "a\0b"] {
            assert_eq!(normalize_project_path(raw), None, "{raw:?} 必须被拒绝");
        }
    }

    #[test]
    fn join_project_path_rejects_escape_attempts() {
        let root = Path::new("/project");
        assert_eq!(
            join_project_path(root, "scripts/main.lua"),
            Some(root.join("scripts/main.lua"))
        );
        assert_eq!(join_project_path(root, "../outside.lua"), None);
    }
}
