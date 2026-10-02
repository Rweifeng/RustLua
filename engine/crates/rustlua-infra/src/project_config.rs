//! `project.toml` / `release.toml` 的键读取（工程配置解析的唯一入口）。
//!
//! 为什么单独成模块：这些文件是**扁平的 `key = "value"`** 行（见 `examples/*/project.toml`），
//! 引擎只需要按行取值——但取值口径必须只有一份，否则"字体配置怎么解析"与
//! "发布包名怎么解析"会各写一遍（此前 `host/runtime_scene.rs` 与
//! `host/runtime_scene.rs` 与拆分前 `core/font.rs`（`project_font`）就各有一份同样的解析器，两边还可能各自漂移）。
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

/// 默认窗口尺寸（横屏）：工程未声明 `window_width`/`window_height` 时的回退值。
pub const DEFAULT_WINDOW_SIZE: (u32, u32) = (1280, 720);

/// 工程的发布窗口尺寸（`project.toml` 的 `window_width` / `window_height`）。
///
/// 为什么要有这两个键：**竖版游戏**（如 `game_jhyt` 的 720×1280）必须让窗口与
/// 设计分辨率一致，Lua 动态 UI（`ui.panel`/`ui.text_at`）是屏幕像素空间、不随参考
/// 分辨率缩放，窗口比例不对就会整块画到屏幕外。此前窗口尺寸写死在
/// `game_app.rs` 里，工程无法声明自己的版面比例。
///
/// 规则：两个键都必须是 ≥ 1 的整数才生效；只写一个、或写成非法值时**整对回退**到
/// [`DEFAULT_WINDOW_SIZE`] 并返回诊断（不做"半截生效"——那会得到一个畸形的窗口）。
#[must_use]
pub fn window_size(root: &Path) -> ((u32, u32), Vec<String>) {
    let width = read_key(root, "window_width");
    let height = read_key(root, "window_height");
    match (width, height) {
        (None, None) => (DEFAULT_WINDOW_SIZE, Vec::new()),
        (width, height) => {
            let parsed_width = width.as_deref().map(parse_dimension);
            let parsed_height = height.as_deref().map(parse_dimension);
            match (parsed_width, parsed_height) {
                (Some(Some(width)), Some(Some(height))) => ((width, height), Vec::new()),
                _ => {
                    let mut diagnostics = Vec::new();
                    match parsed_width {
                        Some(Some(_)) => {}
                        Some(None) => diagnostics.push(format!(
                            "project.toml:window_width 不是正整数：{}",
                            width.unwrap_or_default()
                        )),
                        None => diagnostics.push(
                            "project.toml:window_width 缺失（需与 window_height 同时声明）"
                                .to_owned(),
                        ),
                    }
                    match parsed_height {
                        Some(Some(_)) => {}
                        Some(None) => diagnostics.push(format!(
                            "project.toml:window_height 不是正整数：{}",
                            height.unwrap_or_default()
                        )),
                        None => diagnostics.push(
                            "project.toml:window_height 缺失（需与 window_width 同时声明）"
                                .to_owned(),
                        ),
                    }
                    diagnostics.push(format!(
                        "窗口尺寸整对回退到默认 {}x{}",
                        DEFAULT_WINDOW_SIZE.0, DEFAULT_WINDOW_SIZE.1
                    ));
                    (DEFAULT_WINDOW_SIZE, diagnostics)
                }
            }
        }
    }
}

/// 解析窗口尺寸的**单个轴**：只接受十进制正整数（拒绝 0、负数、小数、非数字）。
fn parse_dimension(raw: &str) -> Option<u32> {
    let value: u32 = raw.trim().parse().ok()?;
    (value >= 1).then_some(value)
}

/// 工程配置文件名。
pub const PROJECT_TOML: &str = "project.toml";
/// 发布包标记文件名（挨着可执行文件，`pack/` 的上一层）。
pub const RELEASE_TOML: &str = "release.toml";

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rustlua-projcfg-{tag}-{}", std::process::id()));
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

    #[test]
    fn window_size_reads_declared_pair_and_falls_back_as_a_whole() {
        let root = temp_dir("window");
        // 两个键都声明 → 按声明的走（竖版工程 720×1280）。
        fs::write(
            root.join(PROJECT_TOML),
            "name = \"Portrait\"\nwindow_width = 720\nwindow_height = 1280\n",
        )
        .unwrap();
        assert_eq!(window_size(&root), ((720, 1280), Vec::new()));

        // 都没写 → 默认横屏，且没有诊断（"没声明"不是错误）。
        fs::write(root.join(PROJECT_TOML), "name = \"Landscape\"\n").unwrap();
        assert_eq!(window_size(&root), (DEFAULT_WINDOW_SIZE, Vec::new()));

        // 只写一个 → 整对回退 + 诊断（不允许"半截生效"）。
        fs::write(root.join(PROJECT_TOML), "window_width = 720\n").unwrap();
        let (size, diagnostics) = window_size(&root);
        assert_eq!(size, DEFAULT_WINDOW_SIZE);
        assert_eq!(
            diagnostics.len(),
            2,
            "缺一个 + 一条整对回退说明: {diagnostics:?}"
        );

        // 非法值（0 / 负数）→ 整对回退 + 逐轴诊断。
        fs::write(
            root.join(PROJECT_TOML),
            "window_width = 0\nwindow_height = -5\n",
        )
        .unwrap();
        let (size, diagnostics) = window_size(&root);
        assert_eq!(size, DEFAULT_WINDOW_SIZE);
        assert!(diagnostics.iter().any(|line| line.contains("window_width")));
        assert!(
            diagnostics
                .iter()
                .any(|line| line.contains("window_height")),
            "{diagnostics:?}"
        );

        // 小数不是合法像素尺寸。
        fs::write(
            root.join(PROJECT_TOML),
            "window_width = 720.5\nwindow_height = 1280\n",
        )
        .unwrap();
        let (size, _) = window_size(&root);
        assert_eq!(size, DEFAULT_WINDOW_SIZE);
        let _ = fs::remove_dir_all(&root);
    }
}
