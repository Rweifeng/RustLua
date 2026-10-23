//! 确定性 Lua 代码生成：每个事件一个函数，节点 id → 稳定局部变量名。
//!
//! 流程用 `goto` + 标签落成线性代码，因此：
//! - 分支/顺序/闸门不需要运行时解释器；
//! - 带守卫的流程环直接变成 Lua 循环（gate/sequence 决定是否回边）；
//! - 所有局部变量都包在 `do ... end` 里，避免 `goto` 跳进局部变量作用域（Lua 语法错误）。
//!
//! 生成物最终由项目 `scripts/generated/<name>.lua` 承载、经现有 Lua VM 加载；本模块不写盘。

use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::error::GraphError;
use crate::graph::{Graph, Node, ValueType, lua_var_name};
use crate::registry::{EventKind, node_spec, plugin_node};
use crate::validate::validate;

/// 校验并生成 Lua 文本。任何校验错误都不会产生半成品。
pub fn compile(graph: &Graph, source: &str) -> Result<String, Vec<GraphError>> {
    compile_with_trace(graph, source, false)
}

/// 与 [`compile`] 同一套语义，但每个节点入口多一行
/// `if __vgraph_trace then __vgraph_trace("<node id>") end`。
///
/// 宿主没装 `__vgraph_trace` 时这行是空操作，因此生成物仍能独立运行；
/// 编辑器用它实现 E03-4b 的「当前执行节点」高亮（见 `LuaRuntime::vgraph_trace_last`）。
pub fn compile_traced(graph: &Graph, source: &str) -> Result<String, Vec<GraphError>> {
    compile_with_trace(graph, source, true)
}

fn compile_with_trace(graph: &Graph, source: &str, trace: bool) -> Result<String, Vec<GraphError>> {
    validate(graph)?;
    Ok(Codegen::new(graph, source, trace).generate())
}

struct Codegen<'a> {
    graph: &'a Graph,
    source: String,
    out: String,
    label_counter: usize,
    trace: bool,
}

#[derive(Default)]
struct Locals {
    declarations: Vec<String>,
    names: BTreeSet<String>,
}

impl<'a> Codegen<'a> {
    fn new(graph: &'a Graph, source: &str, trace: bool) -> Self {
        Self {
            graph,
            source: sanitize_comment(source),
            out: String::new(),
            label_counter: 0,
            trace,
        }
    }

    fn generate(mut self) -> String {
        let source = if self.source.is_empty() {
            "<memory>"
        } else {
            &self.source
        };
        let _ = writeln!(self.out, "-- generated from {source}; do not edit");
        let _ = writeln!(
            self.out,
            "-- graph: {} (vgraph {})",
            sanitize_comment(&self.graph.name),
            self.graph.version
        );
        let _ = writeln!(self.out);
        let _ = writeln!(self.out, "local M = {{}}");

        for node in &self.graph.nodes {
            let Some(spec) = node_spec(&node.kind) else {
                continue;
            };
            let Some(event) = spec.event else {
                continue;
            };
            let parameters: Vec<&str> = event
                .parameters()
                .iter()
                .map(|(_, binding)| *binding)
                .collect();
            let _ = writeln!(
                self.out,
                "\nfunction M.{}({})",
                event.function_name(),
                parameters.join(", ")
            );
            self.emit_trace(&node.id, 1);
            self.emit_event(node, event, 1);
            let _ = writeln!(self.out, "end");
        }

        let _ = writeln!(self.out, "\nreturn M");
        self.out
    }

    fn emit_event(&mut self, node: &Node, event: EventKind, indent: usize) {
        let end_label = self.fresh_label("fend");
        let start = self.flow_target(&node.id, "then");
        self.emit_chain(start.as_deref(), &end_label, indent, &mut Vec::new());
        let _ = writeln!(self.out, "{}::{end_label}::", pad(indent));
        // 标签后面必须是空语句；Lua 允许 label 位于 block 末尾，这里补一行注释保证可读性。
        let _ = writeln!(self.out, "{}-- {}.", pad(indent), event.function_name());
    }

    /// 沿流程边走一段：`Some(node)` 继续生成节点，`None` 跳到本段结束标签。
    fn emit_chain(
        &mut self,
        node_id: Option<&str>,
        end_label: &str,
        indent: usize,
        active: &mut Vec<(String, String)>,
    ) {
        match node_id {
            Some(id) => self.emit_flow(id, end_label, indent, active),
            None => {
                let _ = writeln!(self.out, "{}goto {end_label}", pad(indent));
            }
        }
    }

    fn emit_flow(
        &mut self,
        node_id: &str,
        end_label: &str,
        indent: usize,
        active: &mut Vec<(String, String)>,
    ) {
        if let Some((_, label)) = active.iter().find(|(id, _)| id == node_id) {
            let _ = writeln!(self.out, "{}goto {label}", pad(indent));
            return;
        }
        let Some(node) = self.graph.node(node_id) else {
            let _ = writeln!(self.out, "{}goto {end_label}", pad(indent));
            return;
        };
        let Some(spec) = node_spec(&node.kind) else {
            let _ = writeln!(self.out, "{}goto {end_label}", pad(indent));
            return;
        };
        let label = self.fresh_label("flow");
        let _ = writeln!(
            self.out,
            "{}-- node {}: {}",
            pad(indent),
            sanitize_comment(&node.id),
            node.kind
        );
        let _ = writeln!(self.out, "{}::{label}::", pad(indent));
        self.emit_trace(&node.id, indent);
        active.push((node.id.clone(), label));

        match node.kind.as_str() {
            "branch" => {
                let mut locals = Locals::default();
                let condition = self.resolve_input(&node.id, "condition", &mut locals);
                let _ = writeln!(self.out, "{}do", pad(indent));
                self.flush_locals(&locals, indent + 1);
                let _ = writeln!(self.out, "{}if {condition} then", pad(indent + 1));
                let truthy = self.flow_target(&node.id, "true");
                self.emit_chain(truthy.as_deref(), end_label, indent + 2, active);
                let _ = writeln!(self.out, "{}else", pad(indent + 1));
                let falsy = self.flow_target(&node.id, "false");
                self.emit_chain(falsy.as_deref(), end_label, indent + 2, active);
                let _ = writeln!(self.out, "{}end", pad(indent + 1));
                let _ = writeln!(self.out, "{}end", pad(indent));
            }
            "sequence" => {
                let continuation = self.fresh_label("cont");
                let first = self.flow_target(&node.id, "then0");
                self.emit_chain(first.as_deref(), &continuation, indent, active);
                let _ = writeln!(self.out, "{}::{continuation}::", pad(indent));
                let second = self.flow_target(&node.id, "then1");
                self.emit_chain(second.as_deref(), end_label, indent, active);
            }
            "gate" => {
                let mut locals = Locals::default();
                let open = self.resolve_input(&node.id, "open", &mut locals);
                let _ = writeln!(self.out, "{}do", pad(indent));
                self.flush_locals(&locals, indent + 1);
                let _ = writeln!(
                    self.out,
                    "{}if not ({open}) then goto {end_label} end",
                    pad(indent + 1)
                );
                let _ = writeln!(self.out, "{}end", pad(indent));
                let then = self.flow_target(&node.id, "then");
                self.emit_chain(then.as_deref(), end_label, indent, active);
            }
            _ if spec.category == crate::registry::Category::Action => {
                self.emit_action(node, indent);
                let _ = writeln!(self.out, "{}goto {end_label}", pad(indent));
            }
            _ => {
                // 数据节点不会出现在流程边上；校验已拦截，这里只保证不生成坏语法。
                let _ = writeln!(self.out, "{}goto {end_label}", pad(indent));
            }
        }

        active.pop();
    }

    /// 节点进入点报告给宿主。`__vgraph_trace` 不存在时（游戏运行时/单测）不产生任何调用。
    fn emit_trace(&mut self, node_id: &str, indent: usize) {
        if self.trace {
            let _ = writeln!(
                self.out,
                "{}if __vgraph_trace then __vgraph_trace({}) end",
                pad(indent),
                lua_quote(node_id)
            );
        }
    }

    fn emit_action(&mut self, node: &Node, indent: usize) {
        let mut locals = Locals::default();
        let call = match node.kind.as_str() {
            "set_transform" => {
                let entity = self.resolve_input(&node.id, "entity", &mut locals);
                let x = self.resolve_input(&node.id, "x", &mut locals);
                let y = self.resolve_input(&node.id, "y", &mut locals);
                let z = self.resolve_input(&node.id, "z", &mut locals);
                format!("transform3d.set({entity}, {x}, {y}, {z})")
            }
            "play_animation" => {
                let entity = self.resolve_input(&node.id, "entity", &mut locals);
                let clip = self.string_param(node, "clip");
                format!("animation3d.set({entity}, {clip})")
            }
            "play_audio" => format!("audio.play({})", self.string_param(node, "name")),
            "emit_event" => format!("event.emit({})", self.string_param(node, "name")),
            "ui_set_text" => {
                let text = self.resolve_input(&node.id, "text", &mut locals);
                let element = self.string_param(node, "element");
                format!("ui.text({element}, {text})")
            }
            "save_game" => "save.commit()".to_owned(),
            other => match plugin_node(other) {
                Some(plugin) if !plugin.returns_value => {
                    self.render_plugin(node, plugin.template, &mut locals)
                }
                _ => format!("-- unsupported action {other}"),
            },
        };
        let _ = writeln!(self.out, "{}do", pad(indent));
        self.flush_locals(&locals, indent + 1);
        let _ = writeln!(self.out, "{}{call}", pad(indent + 1));
        let _ = writeln!(self.out, "{}end", pad(indent));
    }

    fn flush_locals(&mut self, locals: &Locals, indent: usize) {
        for declaration in &locals.declarations {
            let _ = writeln!(self.out, "{}{declaration}", pad(indent));
        }
    }

    /// 解析一个输入端口：沿唯一入边取上游表达式。校验保证边存在。
    fn resolve_input(&mut self, node_id: &str, port: &str, locals: &mut Locals) -> String {
        let upstream = self
            .graph
            .edges
            .iter()
            .find(|edge| edge.to.node == node_id && edge.to.port == port)
            .map(|edge| (edge.from.node.clone(), edge.from.port.clone()));
        match upstream {
            Some((node, port)) => self.emit_expr(&node, &port, locals),
            None => "nil".to_owned(),
        }
    }

    fn emit_expr(&mut self, node_id: &str, port: &str, locals: &mut Locals) -> String {
        let Some(node) = self.graph.node(node_id) else {
            return "nil".to_owned();
        };
        let Some(spec) = node_spec(&node.kind) else {
            return "nil".to_owned();
        };
        if let Some(event) = spec.event {
            return event
                .lua_binding(port)
                .map_or_else(|| "nil".to_owned(), str::to_owned);
        }
        let local = lua_var_name(node_id);
        if locals.names.contains(&local) {
            return local;
        }
        let expr = match node.kind.as_str() {
            "number" => format_number(
                node.params
                    .get("value")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0),
            ),
            "bool" => {
                if node
                    .params
                    .get("value")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
                {
                    "true".to_owned()
                } else {
                    "false".to_owned()
                }
            }
            "string" => lua_quote(
                node.params
                    .get("value")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(""),
            ),
            "math" => {
                let a = self.resolve_input(node_id, "a", locals);
                let b = self.resolve_input(node_id, "b", locals);
                let op = match node.params.get("op").and_then(serde_json::Value::as_str) {
                    Some("sub") => "-",
                    Some("mul") => "*",
                    Some("div") => "/",
                    _ => "+",
                };
                format!("({a} {op} {b})")
            }
            "random" => {
                let min = self.resolve_input(node_id, "min", locals);
                let max = self.resolve_input(node_id, "max", locals);
                format!("({min} + math.random() * ({max} - {min}))")
            }
            other => match plugin_node(other) {
                Some(plugin) if plugin.returns_value => {
                    self.render_plugin(node, plugin.template, locals)
                }
                _ => "nil".to_owned(),
            },
        };
        locals.names.insert(local.clone());
        locals.declarations.push(format!("local {local} = {expr}"));
        local
    }

    fn string_param(&self, node: &Node, name: &str) -> String {
        lua_quote(
            node.params
                .get(name)
                .and_then(serde_json::Value::as_str)
                .unwrap_or(""),
        )
    }

    /// 展开插件节点模板。注册期已校验占位符，这里只按模板替换，不再改语义。
    fn render_plugin(&mut self, node: &Node, template: &str, locals: &mut Locals) -> String {
        let mut out = String::with_capacity(template.len());
        let mut rest = template;
        while let Some(open) = rest.find('{') {
            out.push_str(&rest[..open]);
            let Some(close) = rest[open + 1..].find('}') else {
                out.push_str(&rest[open..]);
                return out;
            };
            let token = &rest[open + 1..open + 1 + close];
            match token.strip_prefix("param:") {
                Some(name) => out.push_str(&self.plugin_param_literal(node, name)),
                None => out.push_str(&self.resolve_input(&node.id, token, locals)),
            }
            rest = &rest[open + close + 2..];
        }
        out.push_str(rest);
        out
    }

    fn plugin_param_literal(&self, node: &Node, name: &str) -> String {
        let Some(spec) = node_spec(&node.kind) else {
            return "nil".to_owned();
        };
        let Some(param) = spec.param(name) else {
            return "nil".to_owned();
        };
        let Some(value) = node.params.get(name) else {
            return "nil".to_owned();
        };
        match param.ty {
            ValueType::Number => format_number(value.as_f64().unwrap_or(0.0)),
            ValueType::Bool => if value.as_bool().unwrap_or(false) {
                "true"
            } else {
                "false"
            }
            .to_owned(),
            ValueType::String => lua_quote(value.as_str().unwrap_or("")),
            _ => "nil".to_owned(),
        }
    }

    fn flow_target(&self, node_id: &str, port: &str) -> Option<String> {
        self.graph
            .edges
            .iter()
            .find(|edge| edge.from.node == node_id && edge.from.port == port)
            .map(|edge| edge.to.node.clone())
    }

    fn fresh_label(&mut self, prefix: &str) -> String {
        self.label_counter += 1;
        format!("{prefix}_{}", self.label_counter)
    }
}

fn pad(indent: usize) -> String {
    "    ".repeat(indent)
}

fn sanitize_comment(text: &str) -> String {
    text.chars()
        .map(|ch| if ch == '\n' || ch == '\r' { ' ' } else { ch })
        .collect()
}

/// Lua 数字字面量：整数值去掉 `.0`，其余用 f64 最短往返表示。
fn format_number(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// Lua 字符串字面量：控制字符走十进制转义，UTF-8 原样保留。
fn lua_quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for ch in text.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            ch if (ch as u32) < 0x20 || ch == '\u{7f}' => {
                let _ = write!(quoted, "\\{:03}", ch as u32);
            }
            ch => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_strings_are_quoted_for_lua() {
        assert_eq!(format_number(2.0), "2");
        assert_eq!(format_number(-1.5), "-1.5");
        assert_eq!(lua_quote("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
        assert_eq!(lua_quote("\u{1}"), "\"\\001\"");
    }

    #[test]
    fn traced_compile_marks_every_node_entry_without_changing_clean_output() {
        let graph = Graph::from_json(
            r#"{"version":1,"name":"trace","nodes":[
                {"id":"start","kind":"on_start","position":[0,0]},
                {"id":"save","kind":"save_game","position":[200,0]}
            ],"edges":[{"from":{"node":"start","port":"then"},"to":{"node":"save","port":"in"}}]}"#,
        )
        .expect("图合法");
        let clean = compile(&graph, "trace.vgraph").expect("生成");
        assert!(!clean.contains("__vgraph_trace"), "普通生成物不埋点");
        let traced = compile_traced(&graph, "trace.vgraph").expect("生成");
        assert!(traced.contains("__vgraph_trace(\"start\")"));
        assert!(traced.contains("__vgraph_trace(\"save\")"));
        // 埋点只多出条件调用行，其余逐行不变。
        let stripped: Vec<&str> = traced
            .lines()
            .filter(|line| !line.trim_start().starts_with("if __vgraph_trace then"))
            .collect();
        assert_eq!(stripped.join("\n"), clean.trim_end());
    }
}
