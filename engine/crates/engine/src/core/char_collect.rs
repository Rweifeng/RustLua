//! 打包时字符收集：扫描项目场景的 `Text` 组件与 Lua 字符串字面量，产出去重的
//! 字符集，供运行时预生成 MSDF 字形（首包字体），避免首帧文字卡顿。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::core::scene::Scene;
use crate::core::ui::UiText;

/// 常用中文标点（多数 CJK 字体都覆盖）。
const CJK_PUNCTUATION: &str = "，。！？：；、”“‘’（）《》…·—";

/// 收集项目用到的字符：基础集（ASCII 可打印 + 常用中文标点）+ 场景 `Text` 组件
/// + Lua 字符串字面量。去重后返回。
pub fn collect_project_chars(root: &Path) -> String {
    let mut chars = String::new();
    for c in ' '..='~' {
        chars.push(c);
    }
    chars.push_str(CJK_PUNCTUATION);

    // 场景 / 预制体的 Text 组件。
    for path in walk_files(root, &["scenes", "assets/prefabs"], &["scene", "prefab"]) {
        if let Ok(scene) = Scene::load(&path) {
            for entity in scene.entities.values() {
                if let Some(value) = entity.components.get("Text")
                    && let Ok(ui) = UiText::from_component_str(value)
                {
                    chars.push_str(&ui.text);
                }
                // WP4：世界文字的字符同样要进首包字形子集，否则发布包缺字。
                if let Some(value) = entity.components.get("WorldText")
                    && let Ok(text) = crate::core::world_text::WorldText::from_component_str(value)
                {
                    chars.push_str(&text.text.text);
                }
            }
        }
    }
    // Lua 源码里的字符串字面量。
    for path in walk_files(root, &["scripts"], &["lua"]) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            chars.push_str(&lua_string_literals(&text));
        }
    }
    dedup(&chars)
}

/// 提取 Lua 源码里所有字符串字面量包含的字符（用于预生成字形）。
///
/// 处理 `'`/`"` 引号串（`\` 转义跳过下一字符）、`--` 行注释、`--[[ ]]` 块注释、
/// `[[...]]` / `[=[...]=]` 长字符串。收集的是字符串内容里的每个字符（非仅引用）。
pub fn lua_string_literals(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // `--` 注释。
        if c == '-' && chars.get(i + 1) == Some(&'-') {
            // `--[[ ]]` 块注释：跳过长字符串结束。
            if let Some((level, content_start)) = long_bracket(&chars, i + 2) {
                let end = long_string_end(&chars, content_start, level);
                i = (end + 2 + level).min(chars.len());
                continue;
            }
            // 行注释到行尾。
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        // 长字符串 `[[...]]` / `[=[...]=]`。
        if c == '['
            && let Some((level, content_start)) = long_bracket(&chars, i)
        {
            let end = long_string_end(&chars, content_start, level);
            for &ch in &chars[content_start..end] {
                out.push(ch);
            }
            i = (end + 2 + level).min(chars.len());
            continue;
        }
        if c != '"' && c != '\'' {
            i += 1;
            continue;
        }
        // 引号字符串。
        let quote = c;
        i += 1;
        while i < chars.len() {
            if chars[i] == '\\' {
                if let Some(&escaped) = chars.get(i + 1) {
                    out.push(escaped);
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }
            if chars[i] == quote {
                i += 1;
                break;
            }
            out.push(chars[i]);
            i += 1;
        }
    }
    dedup(&out)
}

/// 从 `start`（指向 `[`）读取长括号级别（`=` 数），返回 `(level, content_start)`。
/// `[[` → level 0，`[=[` → level 1。
fn long_bracket(chars: &[char], start: usize) -> Option<(usize, usize)> {
    if chars.get(start) != Some(&'[') {
        return None;
    }
    let mut i = start + 1;
    while chars.get(i) == Some(&'=') {
        i += 1;
    }
    if chars.get(i) == Some(&'[') {
        Some((i - start - 1, i + 1))
    } else {
        None
    }
}

/// 从内容起点找长字符串结束 `]` + `level` 个 `=` + `]`，返回内容结束索引。
fn long_string_end(chars: &[char], content_start: usize, level: usize) -> usize {
    let mut i = content_start;
    while i < chars.len() {
        if chars[i] == ']' {
            let mut ok = true;
            for k in 1..=level {
                if chars.get(i + k) != Some(&'=') {
                    ok = false;
                    break;
                }
            }
            if ok && chars.get(i + 1 + level) == Some(&']') {
                return i;
            }
        }
        i += 1;
    }
    chars.len()
}

/// 递归遍历目录，收集扩展名匹配的文件。
fn walk_files(root: &Path, dirs: &[&str], extensions: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in dirs {
        walk_dir(&root.join(dir), extensions, &mut out);
    }
    out
}

fn walk_dir(dir: &Path, extensions: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_dir(&path, extensions, out);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str())
            && extensions.contains(&ext)
        {
            out.push(path);
        }
    }
}

/// 按字符去重（保持首次出现顺序）。
fn dedup(s: &str) -> String {
    let mut seen = HashSet::new();
    s.chars().filter(|c| seen.insert(*c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lua_quoted_strings_with_escapes() {
        let src = r#"local a = "你好"; local b = '世界'; local c = "a\"b""#;
        let chars = lua_string_literals(src);
        assert!(chars.contains('你'));
        assert!(chars.contains('好'));
        assert!(chars.contains('世'));
        assert!(chars.contains('界'));
        assert!(chars.contains('a'));
        assert!(chars.contains('b'));
        // 转义后的引号不结束字符串：c = "a\"b" 收集到 a 和 b。
        assert!(chars.contains('a'));
        assert!(chars.contains('b'));
    }

    #[test]
    fn lua_skips_comments() {
        let src = "-- 你好注释\nlocal x = 1 -- 行注释 '忽略'\n--[[ 块注释 \"忽略\" ]]\nlocal s = \"保留\"";
        let chars = lua_string_literals(src);
        assert!(chars.contains('保'));
        assert!(chars.contains('留'));
        // 注释里的字符不应被收集。
        assert!(!chars.contains('注'));
        assert!(!chars.contains('忽'));
        assert!(!chars.contains('释'));
        assert!(!chars.contains('块'));
    }

    #[test]
    fn lua_long_brackets() {
        let src = "local a = [[长字符串]]\nlocal b = [=[含=号和]]结束]=]\nlocal c = [[\n跨行\n]]";
        let chars = lua_string_literals(src);
        assert!(chars.contains('长'));
        assert!(chars.contains('字'));
        assert!(chars.contains('含'));
        assert!(chars.contains('结'));
        assert!(chars.contains('跨'));
        assert!(chars.contains('行'));
        assert!(chars.contains('\n'));
    }

    #[test]
    fn lua_dedup_and_multibyte() {
        let src = "local s = '你好你好'";
        let chars = lua_string_literals(src);
        assert_eq!(chars.chars().filter(|c| *c == '你').count(), 1, "去重");
        assert!(chars.contains('好'));
    }

    #[test]
    fn base_set_covers_ascii_and_punctuation() {
        let chars = collect_project_chars(Path::new("nonexistent"));
        assert!(chars.contains('A') && chars.contains('z') && chars.contains('0'));
        assert!(chars.contains('，') && chars.contains('。') && chars.contains('？'));
        assert!(chars.contains(' '));
    }

    #[test]
    fn collects_scene_text_and_lua() {
        let root = temp_dir();
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::write(
            root.join("scenes/main.scene"),
            "scene_version=2\nname=main\nentity\t1\tTitle\ncomponent\t1\tText\ttext=开始游戏;font_size=24\n",
        )
        .unwrap();
        std::fs::write(root.join("scripts/main.lua"), r#"ui.text("t", "暂停")"#).unwrap();
        let chars = collect_project_chars(&root);
        assert!(
            chars.contains('开')
                && chars.contains('始')
                && chars.contains('游')
                && chars.contains('戏')
        );
        assert!(chars.contains('暂') && chars.contains('停'));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("p2_char_collect_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }
}
