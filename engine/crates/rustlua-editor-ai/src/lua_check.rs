//! AI 生成 Lua 的 API 面静态校验（M75-02 写脚本闭环）。
//!
//! 只拦「模块名存在、函数名不存在」这一类幻觉调用：`entity.spawn` 拼成 `entity.spwan`
//! 会在编辑器里静默变成 nil 调用，直到运行时才炸。字符串/注释里的文本不参与检查。
//! 真正的签名校验仍以 `lua_api_surface.json` 与运行期探针为准；这里是最便宜的第一道闸。

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApiIssue {
    pub line: usize,
    pub module: String,
    pub function: String,
    pub message: String,
}

pub fn surface_tables(surface: &Value) -> BTreeMap<String, BTreeSet<String>> {
    let mut tables = BTreeMap::new();
    for table in surface["tables"].as_array().into_iter().flatten() {
        let Some(name) = table["name"].as_str() else {
            continue;
        };
        let funcs = table["funcs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        tables.insert(name.to_owned(), funcs);
    }
    tables
}

/// 返回所有「已知模块 + 未知函数」的调用点。
pub fn validate_api_calls(
    code: &str,
    tables: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<ApiIssue> {
    let mut issues = Vec::new();
    let mut lexer = Lexer::new(code);
    while let Some(token) = lexer.next_code_token() {
        let IdentToken { text, line } = token;
        let Some(module_funcs) = tables.get(&text) else {
            continue;
        };
        if !lexer.consume_call_chain() {
            continue;
        }
        let Some(function) = lexer.last_ident.clone() else {
            continue;
        };
        if lexer.last_ident_line != line && !lexer.last_was_call {
            continue;
        }
        if !module_funcs.contains(&function) {
            issues.push(ApiIssue {
                line,
                module: text.clone(),
                function: function.clone(),
                message: format!(
                    "`{text}.{function}` 不在 Lua API 表面里（{} 已知 {} 个函数，最近的可选项：{}）",
                    text,
                    module_funcs.len(),
                    suggestions(&text, module_funcs, &function)
                ),
            });
        }
    }
    issues
}

fn suggestions(module: &str, funcs: &BTreeSet<String>, target: &str) -> String {
    let mut ranked: Vec<(usize, &String)> = funcs
        .iter()
        .map(|func| (edit_distance(func, target), func))
        .collect();
    ranked.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(right.1)));
    ranked
        .into_iter()
        .take(3)
        .map(|(_, func)| format!("`{module}.{func}`"))
        .collect::<Vec<_>>()
        .join("、")
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (i, a) in left.chars().enumerate() {
        let mut current = vec![i + 1];
        for (j, b) in right.iter().enumerate() {
            let cost = usize::from(a != *b);
            current.push(
                (previous[j] + cost)
                    .min(previous[j + 1] + 1)
                    .min(current[j] + 1),
            );
        }
        previous = current;
    }
    previous[right.len()]
}

#[derive(Clone, Debug)]
struct IdentToken {
    text: String,
    line: usize,
}

struct Lexer<'a> {
    bytes: &'a [u8],
    index: usize,
    line: usize,
    last_ident: Option<String>,
    last_ident_line: usize,
    last_was_call: bool,
}

impl<'a> Lexer<'a> {
    fn new(code: &'a str) -> Self {
        Self {
            bytes: code.as_bytes(),
            index: 0,
            line: 1,
            last_ident: None,
            last_ident_line: 0,
            last_was_call: false,
        }
    }

    fn peek(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.index + offset).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek(0)?;
        self.index += 1;
        if byte == b'\n' {
            self.line += 1;
        }
        Some(byte)
    }

    fn skip_to_line_end(&mut self) {
        while let Some(byte) = self.bump() {
            if byte == b'\n' {
                return;
            }
        }
    }

    fn skip_long_bracket(&mut self, level: usize) {
        // 结束符 = `]` + level 个 `=` + `]`。
        let mut matched = 0usize;
        while let Some(byte) = self.bump() {
            if matched == 0 {
                if byte == b']' {
                    matched = 1;
                }
            } else if matched == level + 1 {
                if byte == b']' {
                    return;
                }
                matched = 0;
            } else if byte == b'=' {
                matched += 1;
            } else {
                matched = 0;
            }
        }
    }

    fn skip_string(&mut self, quote: u8) {
        while let Some(byte) = self.bump() {
            if byte == b'\\' {
                self.bump();
            } else if byte == quote {
                return;
            }
        }
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            while self.peek(0).is_some_and(|byte| byte.is_ascii_whitespace()) {
                self.bump();
            }
            if self.peek(0) == Some(b'-') && self.peek(1) == Some(b'-') {
                self.bump();
                self.bump();
                if self.peek(0) == Some(b'[') {
                    // --[==[ 长注释
                    let mut level = 0;
                    while self.peek(0) == Some(b'=') {
                        level += 1;
                        self.bump();
                    }
                    if self.peek(0) == Some(b'[') {
                        self.bump();
                        self.skip_long_bracket(level);
                        continue;
                    }
                }
                self.skip_to_line_end();
                continue;
            }
            return;
        }
    }

    fn next_code_token(&mut self) -> Option<IdentToken> {
        loop {
            self.skip_whitespace_and_comments();
            let byte = self.peek(0)?;
            match byte {
                b'\'' | b'"' => {
                    self.bump();
                    self.skip_string(byte);
                    self.last_ident = None;
                    self.last_was_call = false;
                }
                b'[' => {
                    // [[ 长字符串 ]] / [=[ ... ]=]
                    self.bump();
                    let mut level = 0;
                    while self.peek(0) == Some(b'=') {
                        level += 1;
                        self.bump();
                    }
                    if self.peek(0) == Some(b'[') {
                        self.bump();
                        self.skip_long_bracket(level);
                    }
                    self.last_ident = None;
                    self.last_was_call = false;
                }
                byte if byte == b'_' || byte.is_ascii_alphabetic() => {
                    let start = self.index;
                    let line = self.line;
                    while let Some(byte) = self.peek(0) {
                        if byte == b'_' || byte.is_ascii_alphanumeric() {
                            self.bump();
                        } else {
                            break;
                        }
                    }
                    let text = String::from_utf8_lossy(&self.bytes[start..self.index]).into_owned();
                    self.last_ident = Some(text.clone());
                    self.last_ident_line = line;
                    self.last_was_call = false;
                    return Some(IdentToken { text, line });
                }
                _ => {
                    self.bump();
                    self.last_ident = None;
                    self.last_was_call = false;
                }
            }
        }
    }

    /// 从当前标识符之后消费 `.` / `:` 后的函数名；只有紧跟 `(` 才算一次调用。
    fn consume_call_chain(&mut self) -> bool {
        let saved = (self.index, self.line);
        self.skip_whitespace_and_comments();
        let separator = self.peek(0);
        if !matches!(separator, Some(b'.') | Some(b':')) {
            self.index = saved.0;
            self.line = saved.1;
            return false;
        }
        self.bump();
        self.skip_whitespace_and_comments();
        let start = self.index;
        while let Some(byte) = self.peek(0) {
            if byte == b'_' || byte.is_ascii_alphanumeric() {
                self.bump();
            } else {
                break;
            }
        }
        if self.index == start {
            return false;
        }
        let name = String::from_utf8_lossy(&self.bytes[start..self.index]).into_owned();
        let after_name = (self.index, self.line);
        self.skip_whitespace_and_comments();
        let is_call = self.peek(0) == Some(b'(');
        self.index = after_name.0;
        self.line = after_name.1;
        if !is_call {
            return false;
        }
        // 调用之后不再属于同一链（`a.b().c()` 的第二个名字要等下一轮扫描）。
        self.last_ident = Some(name);
        self.last_was_call = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tables() -> BTreeMap<String, BTreeSet<String>> {
        surface_tables(&json!({
            "tables": [
                {"name": "entity", "funcs": ["spawn", "destroy", "set_position"]},
                {"name": "ui", "funcs": ["show"]}
            ]
        }))
    }

    #[test]
    fn flags_unknown_function_on_known_module() {
        let issues = validate_api_calls("entity.spwan(\"a\")", &tables());
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("entity.spwan"));
        assert!(issues[0].message.contains("entity.spawn"));
    }

    #[test]
    fn accepts_known_calls_and_ignores_strings_comments() {
        let code = r#"
            -- entity.spwan()
            --[[ entity.spwan() ]]
            local text = "entity.spwan()"
            entity.spawn("a")
            ui.show("b")
            print("entity.spwan")
        "#;
        assert!(validate_api_calls(code, &tables()).is_empty());
    }

    #[test]
    fn ignores_unknown_modules_and_plain_fields() {
        let code = "local e = { spawn = 1 }; e.spawn(); foo.bar(); entity.spawn";
        assert!(validate_api_calls(code, &tables()).is_empty());
    }

    #[test]
    fn reports_line_numbers() {
        let code = "entity.spawn()\n\nentity.spwan()";
        let issues = validate_api_calls(code, &tables());
        assert_eq!(issues[0].line, 3);
    }
}
