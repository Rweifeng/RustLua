//! 棘轮：`script/` 下的**生产代码**只经 `domain` 认识领域 crate。
//!
//! 门面（`script/domain.rs`）的价值全在"唯一登记处"这四个字——一旦有人图省事再写一句
//! `use rustlua_ecs::…`，引用面就又散回去了，而编译器不会有任何意见。这条测试把规矩钉死，
//! 并像 `panels/architecture_tests.rs` 那样自带反面样例（守卫自己必须能判红）。
//!
//! 豁免只有两处，理由都明确：
//! - `domain.rs` 自己——它就是登记处；
//! - `tests/` 子树——测试直接构造领域类型是合理的，门面不该挡测试的视线。
//!
//! 局限：扫描按行做词法，不认跨行裸字符串（`r#"…"#`）的内部。`script/mod.rs` 里的
//! `LUA_API_PROBE` 是一段 Lua，不涉及 crate 名；若哪天探针里要写 crate 名，请先把它挪出
//! 生产文件，别让这条棘轮变成假红。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// 门面必须登记的"别名 ⇄ 领域 crate"表。新增领域 crate 时**两处一起改**：
/// 这张表 + `script/domain.rs`。
const PINNED_ALIASES: &[(&str, &str)] = &[
    ("animation", "rustlua_animation"),
    ("camera", "rustlua_camera"),
    ("ecs", "rustlua_ecs"),
    ("gameplay", "rustlua_gameplay"),
    ("infra", "rustlua_infra"),
    ("lua_api", "rustlua_lua_api"),
    ("math", "rustlua_math"),
    ("physics", "rustlua_physics"),
    ("render", "rustlua_render"),
    ("scene", "rustlua_scene"),
    ("ui", "rustlua_ui"),
    ("visual_script", "rustlua_visual_script"),
];

fn script_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/script")
}

/// `script/` 下除门面与测试树之外的全部生产源码（相对路径 → 内容）。
fn production_sources() -> Vec<(String, String)> {
    let mut sources = Vec::new();
    collect(&script_root(), "", &mut sources);
    sources.sort_by(|left, right| left.0.cmp(&right.0));
    sources
}

fn collect(dir: &Path, prefix: &str, out: &mut Vec<(String, String)>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("script dir")
        .map(|entry| entry.expect("dir entry").path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .expect("file name")
            .to_owned();
        if path.is_dir() {
            // 只豁免顶层的测试树；`api/` 这类生产子目录照常递归。
            if prefix.is_empty() && name == "tests" {
                continue;
            }
            collect(&path, &format!("{prefix}{name}/"), out);
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some("rs") {
            continue;
        }
        if prefix.is_empty() && name == "domain.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("script source");
        out.push((format!("{prefix}{name}"), text));
    }
}

/// 逐行去掉注释，行号保持不变（下标即行号 - 1）。
///
/// 字符串字面量里的 `//` 不是注释，`/* … */` 多行块注释整段略过——否则一句
/// `// 见 rustlua_ecs::…` 的普通注释就会把棘轮判红，逼人删掉有用的注释。
fn code_lines(source: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(source.lines().count());
    let mut in_block = false;
    for line in source.lines() {
        let mut kept = String::new();
        let mut rest = line;
        loop {
            if rest.is_empty() {
                break;
            }
            if in_block {
                match rest.find("*/") {
                    Some(end) => {
                        rest = &rest[end + 2..];
                        in_block = false;
                    }
                    None => rest = "",
                }
                continue;
            }
            let bytes = rest.as_bytes();
            let mut in_string = false;
            let mut escaped = false;
            let mut line_comment = None;
            let mut block_comment = None;
            for index in 0..bytes.len() {
                let byte = bytes[index];
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == b'"' {
                        in_string = false;
                    }
                } else if byte == b'"' {
                    in_string = true;
                } else if byte == b'/' && matches!(bytes.get(index + 1), Some(b'/')) {
                    line_comment = Some(index);
                    break;
                } else if byte == b'/' && matches!(bytes.get(index + 1), Some(b'*')) {
                    block_comment = Some(index);
                    break;
                }
            }
            match (line_comment, block_comment) {
                (Some(at), _) => {
                    kept.push_str(&blank_string_literals(&rest[..at]));
                    rest = "";
                }
                (None, Some(at)) => {
                    kept.push_str(&blank_string_literals(&rest[..at]));
                    in_block = true;
                    rest = &rest[at + 1..];
                }
                (None, None) => {
                    kept.push_str(&blank_string_literals(rest));
                    rest = "";
                }
            }
        }
        out.push(kept);
    }
    out
}

/// 把字符串字面量的**内容**抹成空格（引号保留）：`"rustlua_ecs"` 这种只是文案，
/// 不是编译期依赖，不该被算成引用。
fn blank_string_literals(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    for ch in text.chars() {
        if in_string {
            if escaped {
                escaped = false;
                out.push(' ');
            } else if ch == '\\' {
                escaped = true;
                out.push(' ');
            } else if ch == '"' {
                in_string = false;
                out.push('"');
            } else {
                out.push(' ');
            }
        } else if ch == '"' {
            in_string = true;
            out.push('"');
        } else {
            out.push(ch);
        }
    }
    out
}

/// 一行代码里出现的第一个 `rustlua_*` crate 名。
fn domain_crate_reference(line: &str) -> Option<&str> {
    let start = line.find("rustlua_")?;
    let tail = &line[start..];
    let end = tail
        .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .unwrap_or(tail.len());
    Some(&tail[..end])
}

#[test]
fn script_production_code_reaches_domain_crates_only_through_the_facade() {
    let mut offenders = Vec::new();
    for (name, source) in production_sources() {
        for (index, line) in code_lines(&source).iter().enumerate() {
            if let Some(krate) = domain_crate_reference(line) {
                offenders.push(format!("{name}:{} 引用了 {krate}", index + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "`script/` 的生产代码必须经 `domain` 取领域类型（表见 `script/domain.rs`）。\n\
         把 `use rustlua_xxx::…` 换成 `use crate::script::domain::别名::…`；别名没登记就先补门面：\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn facade_registers_every_script_domain_crate() {
    let facade = std::fs::read_to_string(script_root().join("domain.rs")).expect("domain.rs");
    let mut registered = BTreeSet::new();
    for line in code_lines(&facade) {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("pub(crate) use ") else {
            continue;
        };
        let Some(rest) = rest.strip_suffix(';') else {
            continue;
        };
        let Some((krate, alias)) = rest.split_once(" as ") else {
            continue;
        };
        registered.insert((alias.trim().to_owned(), krate.trim().to_owned()));
    }
    let expected: BTreeSet<(String, String)> = PINNED_ALIASES
        .iter()
        .map(|(alias, krate)| ((*alias).to_owned(), (*krate).to_owned()))
        .collect();
    assert_eq!(
        registered, expected,
        "门面登记表与 PINNED_ALIASES 不一致：新增/删除领域 crate 时两边要一起改"
    );

    // 死条目检查：登记了却没有任何生产代码使用的别名，说明这个领域已经不必经门面取用，
    // 留在表里只会让"谁依赖了哪些领域"重新变得不准确。
    let production: String = production_sources()
        .iter()
        .map(|(_, source)| code_lines(source).join("\n"))
        .collect::<Vec<_>>()
        .join("\n");
    let unused: Vec<&str> = PINNED_ALIASES
        .iter()
        .map(|(alias, _)| *alias)
        .filter(|alias| !production.contains(&format!("domain::{alias}")))
        .collect();
    assert!(
        unused.is_empty(),
        "这些门面别名在生产代码里一次都没被用到，请从 `domain.rs` 和 PINNED_ALIASES 里删掉：{unused:?}"
    );
}

/// 守卫自己必须能判红：直接 `use rustlua_*` 要被抓到，纯注释不该被误伤。
#[test]
fn ratchet_detects_a_direct_domain_crate_import() {
    let sample = "\
// rustlua_ui 只在注释里出现，不算引用
use rustlua_ecs::world::World;
let label = \"rustlua_x 在字符串里也不算\";
/* 块注释里的 rustlua_scene 同样不算 */
";
    let lines = code_lines(sample);
    let hits: Vec<&str> = lines
        .iter()
        .filter_map(|line| domain_crate_reference(line))
        .collect();
    assert_eq!(
        hits,
        vec!["rustlua_ecs"],
        "只应命中真正的 `use`，注释（含块注释）必须被剥掉"
    );
}
