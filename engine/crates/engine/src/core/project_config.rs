//! `project.toml` / `release.toml` 的键读取（工程配置解析的唯一入口）。
//!
//! 为什么单独成模块：这些文件是**扁平的 `key = "value"`** 行（见 `examples/*/project.toml`），
//! 引擎只需要按行取值——但取值口径必须只有一份，否则"字体配置怎么解析"与
//! "发布包名怎么解析"会各写一遍（此前 `host/runtime_scene.rs` 与
//! `core/font.rs::project_font` 就各有一份同样的解析器，两边还可能各自漂移）。
//!
//! 取值规则：按行 trim（含 BOM），匹配 `<key> =` 前缀，去掉两侧引号与空白；
//! 空值视为缺失（`None`）——"键存在但为空"不能当成一个有效配置。

use std::{fs, path::Path};

/// `project.toml` 的单个键。
#[must_use]
pub fn read_key(root: &Path, key: &str) -> Option<String> {
    read_key_in(&root.join(PROJECT_TOML), key)
}

/// 任意配置文件（如发布包旁边的 `release.toml`）的单个键。
#[must_use]
pub fn read_key_in(path: &Path, key: &str) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    parse_key(&text, key)
}

/// 从已读入的文本里取键（测试与调用方已有文本时用）。
#[must_use]
pub fn parse_key(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim().trim_start_matches('\u{feff}');
        line.strip_prefix(&format!("{key} ="))
            .map(|value| value.trim().trim_matches('"').to_owned())
            .filter(|value| !value.trim().is_empty())
    })
}

/// 工程的启动场景（缺省 `scenes/main.scene`）。
#[must_use]
pub fn startup_scene(root: &Path) -> String {
    read_key(root, "startup_scene").unwrap_or_else(|| "scenes/main.scene".to_owned())
}

/// 工程名（`project.toml` 的 `name`）。
#[must_use]
pub fn project_name(root: &Path) -> Option<String> {
    read_key(root, "name")
}

/// 发布名（发布包旁边的 `release.toml` 的 `publish_name`）。
#[must_use]
pub fn publish_name(pack_root: &Path) -> Option<String> {
    let parent = pack_root.parent()?;
    read_key_in(&parent.join(RELEASE_TOML), "publish_name")
}

/// 工程配置文件名。
pub const PROJECT_TOML: &str = "project.toml";
/// 发布包标记文件名（挨着可执行文件，`pack/` 的上一层）。
pub const RELEASE_TOML: &str = "release.toml";

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("p2-projcfg-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn keys_are_read_with_quotes_bom_and_empty_values_handled() {
        let text =
            "\u{feff}name = \"江湖有田\"\n  font = assets/fonts/x.ttf  \nempty = \"\"\n# comment\n";
        assert_eq!(parse_key(text, "name").as_deref(), Some("江湖有田"));
        assert_eq!(
            parse_key(text, "font").as_deref(),
            Some("assets/fonts/x.ttf")
        );
        assert_eq!(parse_key(text, "empty"), None, "空值视为缺失");
        assert_eq!(parse_key(text, "missing"), None);
        // 前缀匹配不能误伤（`font` 不能命中 `font_fallback`）。
        assert_eq!(parse_key("font_fallback = \"b.ttf\"\n", "font"), None);
    }

    #[test]
    fn project_and_release_files_are_read_from_their_own_locations() {
        let root = temp_dir("release");
        let pack = root.join("pack");
        fs::create_dir_all(&pack).unwrap();
        fs::write(pack.join(PROJECT_TOML), "name = \"DemoGame\"\n").unwrap();
        fs::write(root.join(RELEASE_TOML), "publish_name = \"Demo\"\n").unwrap();
        assert_eq!(project_name(&pack).as_deref(), Some("DemoGame"));
        assert_eq!(publish_name(&pack).as_deref(), Some("Demo"));
        // 缺文件 → 缺失（不是错误）。
        assert_eq!(publish_name(&root), None);
        let _ = fs::remove_dir_all(&root);
    }
}
