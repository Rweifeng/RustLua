//! Diagnostic location parsing and jump targets (docs/55 §5 P1: "Console 统一脚本、导入、构建、
//! 渲染错误，并支持文件/行/实体定位").
//!
//! Parsing is a pure function so it is testable without a window, and the shell decides what a
//! jump means (select an entity, open a scene/prefab document, or narrow the console filter).

use rustlua_scene::scene::SceneEntityId;
use std::path::PathBuf;

/// One resolvable location extracted from a log line or diagnostic message.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiagnosticLocation {
    /// Project-relative or absolute path mentioned by the message.
    pub path: Option<PathBuf>,
    /// 1-based line number when the message carries one.
    pub line: Option<u32>,
    /// Entity the message refers to (`entity=42`, `实体 42`).
    pub entity: Option<SceneEntityId>,
}

impl DiagnosticLocation {
    pub fn is_empty(&self) -> bool {
        self.path.is_none() && self.line.is_none() && self.entity.is_none()
    }

    /// Whether this location can open a document (scene/prefab/widget files).
    pub fn is_document_path(&self) -> bool {
        self.path.as_ref().is_some_and(|path| {
            matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("scene") | Some("prefab") | Some("widget")
            )
        })
    }

    /// Human-readable form for the status bar.
    pub fn describe(&self) -> String {
        let mut out = String::new();
        if let Some(path) = &self.path {
            out.push_str(&path.to_string_lossy().replace('\\', "/"));
        }
        if let Some(line) = self.line {
            if out.is_empty() {
                out.push_str(&format!("第 {line} 行"));
            } else {
                out.push_str(&format!(":{line}"));
            }
        }
        if let Some(entity) = self.entity {
            if !out.is_empty() {
                out.push_str(" · ");
            }
            out.push_str(&format!("实体 {}", entity.0));
        }
        out
    }
}

/// Parse a location out of a log line or diagnostic message.
///
/// Recognised forms (all optional and combinable):
/// - `scripts/main.lua:12`, `assets/scenes/main.scene:3`, `E:\proj\a.lua:12`
/// - `第 12 行`
/// - `entity=42`, `entity 42`, `entity:42`, `实体 42`
pub fn parse_location(text: &str) -> Option<DiagnosticLocation> {
    let mut location = DiagnosticLocation::default();

    // path[:line]
    for token in text.split_whitespace() {
        if let Some((path, line)) = split_path_and_line(token) {
            location.path = Some(path);
            location.line = line;
            break;
        }
    }

    // 第 N 行（中文诊断）
    if location.line.is_none()
        && let Some(line) = parse_chinese_line(text)
    {
        location.line = Some(line);
    }

    // entity 引用
    if let Some(entity) = parse_entity(text) {
        location.entity = Some(entity);
    }

    (!location.is_empty()).then_some(location)
}

/// Split `path:line` (or `path#Lline`) into its parts; `line` is `None` when absent.
///
/// A trailing colon that is *not* followed by digits still yields the path (`foo.lua:bar` →
/// path `foo.lua`), because knowing the file is more useful than nothing. Drive letters stay
/// safe: only the **last** colon is considered and the head must itself look like a path
/// (so `E:` alone is never treated as one).
fn split_path_and_line(token: &str) -> Option<(PathBuf, Option<u32>)> {
    let token = token
        .trim_matches(|c: char| matches!(c, '(' | ')' | '[' | ']' | ',' | ';' | '"' | '\'' | '`'));
    let digits_after = |text: &str| -> Option<u32> {
        (!text.is_empty() && text.chars().all(|c| c.is_ascii_digit()))
            .then(|| text.parse().ok())
            .flatten()
    };
    if let Some(index) = token.rfind(':') {
        let (head, tail) = token.split_at(index);
        if looks_like_path(head) {
            return Some((PathBuf::from(head), digits_after(&tail[1..])));
        }
    }
    if let Some(index) = token.rfind("#L") {
        let (head, tail) = token.split_at(index);
        if looks_like_path(head) {
            return Some((PathBuf::from(head), digits_after(&tail[2..])));
        }
    }
    looks_like_path(token).then(|| (PathBuf::from(token), None))
}

/// A path-like token: has a separator or a known source/asset extension, and is not a URL.
fn looks_like_path(token: &str) -> bool {
    if token.len() < 3 || token.contains("://") {
        return false;
    }
    if token.contains('/') || token.contains('\\') {
        return true;
    }
    let extension = token.rsplit_once('.').map(|(_, extension)| extension);
    matches!(
        extension,
        Some("lua")
            | Some("scene")
            | Some("prefab")
            | Some("widget")
            | Some("toml")
            | Some("rs")
            | Some("png")
            | Some("json")
            | Some("txt")
    )
}

fn parse_chinese_line(text: &str) -> Option<u32> {
    let marker = text.find("第")?;
    let rest = &text[marker + "第".len()..];
    let end = rest.find('行')?;
    rest[..end].trim().parse().ok()
}

fn parse_entity(text: &str) -> Option<SceneEntityId> {
    // `entity=42` / `entity:42` / `entity 42` / `实体 42`
    for marker in ["entity=", "entity:", "entity ", "实体 ", "实体="] {
        if let Some(index) = text.find(marker) {
            let rest = &text[index + marker.len()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(value) = digits.parse::<u64>() {
                return Some(SceneEntityId(value));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unix_style_path_with_line() {
        let location =
            parse_location("2026-01-01T00:00:00Z ERROR script: load failed at scripts/main.lua:12")
                .unwrap();
        assert_eq!(location.path, Some(PathBuf::from("scripts/main.lua")));
        assert_eq!(location.line, Some(12));
        assert!(!location.is_document_path());
        assert_eq!(location.describe(), "scripts/main.lua:12");
    }

    #[test]
    fn parses_windows_style_path_with_line() {
        let location =
            parse_location(r"error: cannot open E:\Codex\RustLua\assets\tex.png:7").unwrap();
        assert_eq!(
            location.path,
            Some(PathBuf::from(r"E:\Codex\RustLua\assets\tex.png"))
        );
        assert_eq!(location.line, Some(7));
    }

    #[test]
    fn parses_scene_document_paths_as_openable() {
        let location =
            parse_location("scene validation failed assets/scenes/main.scene:3").unwrap();
        assert!(location.is_document_path());
        assert_eq!(location.line, Some(3));
    }

    #[test]
    fn parses_chinese_line_marker() {
        let location =
            parse_location("[runtime] scripts/main.lua 第 42 行：attempt to index nil").unwrap();
        assert_eq!(location.path, Some(PathBuf::from("scripts/main.lua")));
        assert_eq!(location.line, Some(42));
    }

    #[test]
    fn parses_entity_references() {
        let cases = [
            "layout overflow entity=42",
            "layout overflow entity:42",
            "layout overflow entity 42",
            "layout overflow 实体 42",
        ];
        for case in cases {
            let location = parse_location(case).expect(case);
            assert_eq!(location.entity, Some(SceneEntityId(42)), "{case}");
        }
    }

    #[test]
    fn combines_path_line_and_entity() {
        let location =
            parse_location("WARN script: entity=7 invalid resource assets/x.png:3").unwrap();
        assert_eq!(location.entity, Some(SceneEntityId(7)));
        assert_eq!(location.path, Some(PathBuf::from("assets/x.png")));
        assert_eq!(location.line, Some(3));
        assert_eq!(location.describe(), "assets/x.png:3 · 实体 7");
    }

    #[test]
    fn rejects_lines_without_location() {
        assert!(parse_location("2026-01-01T00:00:00Z INFO editor: [editor] ready").is_none());
        assert!(parse_location("").is_none());
        // URL 不是项目路径。
        assert!(parse_location("see https://example.com/docs.html for details").is_none());
    }

    #[test]
    fn line_number_requires_digits() {
        // `foo.lua:bar` 只保留路径，不把 `bar` 当行号。
        let location = parse_location("cannot parse foo.lua:bar").unwrap();
        assert_eq!(location.path, Some(PathBuf::from("foo.lua")));
        assert_eq!(location.line, None);
    }
}
