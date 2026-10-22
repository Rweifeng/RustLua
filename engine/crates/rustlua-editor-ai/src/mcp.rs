//! 最小 MCP 传输层（JSON-RPC 2.0 over stdio）。
//!
//! 协议面只做翻译：`tools/list`、`resources/list` 来自静态 tool 面；
//! `tools/call` / `resources/read` 交给宿主（主线程执行）。不引入 SDK，
//! 因此 editor 的依赖树不因 AI 而膨胀（docs/71 §10 的退路方案）。

use crate::bridge::ToolError;
use crate::surface::{resource_list, tool_by_name, tools};
use serde_json::{Value, json};
use std::io::{BufRead, Write};

/// 宿主调用（由 bridge 注入）。
pub enum HostCall {
    Tool { name: String, args: Value },
    Resource { uri: String },
}

pub const DEFAULT_PROTOCOL_VERSION: &str = "2024-11-05";

/// 启动一个阻塞式 stdio 循环。每行一条 JSON-RPC 消息（MCP stdio 约定）；
/// 同时兼容 `Content-Length` 分帧，便于与少数客户端对接。
pub fn serve<R, W, F>(mut reader: R, mut writer: W, mut handler: F) -> std::io::Result<()>
where
    R: BufRead,
    W: Write,
    F: FnMut(HostCall) -> Result<Value, ToolError>,
{
    loop {
        let Some(message) = read_message(&mut reader)? else {
            return Ok(());
        };
        let Some(response) = handle_message(message, &mut handler) else {
            continue;
        };
        write_message(&mut writer, &response)?;
    }
}

fn read_message<R: BufRead>(reader: &mut R) -> std::io::Result<Option<Value>> {
    loop {
        let mut first = String::new();
        let read = reader.read_line(&mut first)?;
        if read == 0 {
            return Ok(None);
        }
        let trimmed = first.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Content-Length 分帧（大小写不敏感）。
        let lower = trimmed.to_ascii_lowercase();
        if let Some(length) = lower.strip_prefix("content-length:") {
            let length: usize = length
                .trim()
                .parse()
                .map_err(|_| invalid_data("Content-Length 不是数字"))?;
            let mut header = String::new();
            loop {
                header.clear();
                if reader.read_line(&mut header)? == 0 {
                    return Ok(None);
                }
                if header.trim().is_empty() {
                    break;
                }
            }
            let mut body = vec![0_u8; length];
            std::io::Read::read_exact(reader, &mut body)?;
            return parse_json(&body).map(Some);
        }
        return parse_json(first.as_bytes()).map(Some);
    }
}

fn parse_json(bytes: &[u8]) -> std::io::Result<Value> {
    serde_json::from_slice(bytes).map_err(|error| invalid_data(&format!("JSON 解析失败：{error}")))
}

fn invalid_data(message: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.to_owned())
}

fn write_message<W: Write>(writer: &mut W, message: &Value) -> std::io::Result<()> {
    serde_json::to_writer(&mut *writer, message).map_err(std::io::Error::other)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

/// 处理一条 JSON-RPC 消息。通知（没有 id）返回 `None`。
pub fn handle_message<F>(message: Value, handler: &mut F) -> Option<Value>
where
    F: FnMut(HostCall) -> Result<Value, ToolError>,
{
    let id = message.get("id").cloned();
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    if method.is_empty() {
        // 客户端发来的响应（本 server 不发起请求）：忽略。
        return None;
    }
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));

    // 通知：不回复。
    let notification = id.is_none();
    let result = dispatch(method, &params, handler);
    if notification {
        return None;
    }
    let id = id.unwrap_or(Value::Null);
    Some(match result {
        Ok(value) => json!({"jsonrpc": "2.0", "id": id, "result": value}),
        Err(RpcError::Protocol(code, message)) => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
        }
    })
}

enum RpcError {
    Protocol(i64, String),
}

fn dispatch<F>(method: &str, params: &Value, handler: &mut F) -> Result<Value, RpcError>
where
    F: FnMut(HostCall) -> Result<Value, ToolError>,
{
    match method {
        "initialize" => {
            let requested = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_PROTOCOL_VERSION);
            Ok(json!({
                "protocolVersion": requested,
                "capabilities": {
                    "tools": {"listChanged": false},
                    "resources": {"subscribe": false, "listChanged": false}
                },
                "serverInfo": {"name": "rustlua-editor-ai", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "RustLua 编辑器 AI 能力层。写 tool 受确认网关约束，所有场景编辑经 EditorCommand 落一条 Undo。"
            }))
        }
        "notifications/initialized" | "notifications/cancelled" => Ok(json!({})),
        "ping" => Ok(json!({})),
        "logging/setLevel" => Ok(json!({})),
        "prompts/list" => Ok(json!({"prompts": []})),
        "resources/list" => Ok(json!({
            "resources": resource_list().iter().map(|resource| json!({
                "uri": resource.uri,
                "name": resource.name,
                "mimeType": resource.mime_type,
                "description": resource.description,
            })).collect::<Vec<_>>()
        })),
        "resources/templates/list" => Ok(json!({"resourceTemplates": []})),
        "resources/read" => {
            let uri = params.get("uri").and_then(Value::as_str).ok_or_else(|| {
                RpcError::Protocol(-32602, "resources/read 需要字符串 uri".to_owned())
            })?;
            let value = handler(HostCall::Resource {
                uri: uri.to_owned(),
            })
            .map_err(|error| RpcError::Protocol(-32603, error.message()))?;
            let text = value
                .as_str()
                .map_or_else(|| value.to_string(), str::to_owned);
            let mime = resource_list()
                .iter()
                .find(|resource| resource.uri == uri)
                .map_or("text/plain", |resource| resource.mime_type);
            Ok(json!({"contents": [{"uri": uri, "mimeType": mime, "text": text}]}))
        }
        "tools/list" => Ok(json!({
            "tools": tools().iter().map(|tool| json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": tool.input_schema,
            })).collect::<Vec<_>>()
        })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).ok_or_else(|| {
                RpcError::Protocol(-32602, "tools/call 需要字符串 name".to_owned())
            })?;
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            if tool_by_name(name).is_none() {
                return Ok(tool_content(
                    Err(ToolError::UnknownTool(name.to_owned())),
                    name,
                ));
            }
            Ok(tool_content(
                handler(HostCall::Tool {
                    name: name.to_owned(),
                    args,
                }),
                name,
            ))
        }
        other => Err(RpcError::Protocol(-32601, format!("不支持的方法：{other}"))),
    }
}

fn tool_content(result: Result<Value, ToolError>, name: &str) -> Value {
    match result {
        Ok(value) => json!({
            "content": [{"type": "text", "text": serde_json::to_string(&value).unwrap_or_default()}],
            "structuredContent": value,
            "isError": false
        }),
        Err(error) => json!({
            "content": [{"type": "text", "text": error.message()}],
            "structuredContent": {"tool": name, "error": error},
            "isError": true
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn fake_handler(call: HostCall) -> Result<Value, ToolError> {
        match call {
            HostCall::Tool { name, args } => Ok(json!({"tool": name, "args": args})),
            HostCall::Resource { uri } => Ok(Value::String(format!("body:{uri}"))),
        }
    }

    fn run(lines: &str) -> Vec<Value> {
        let mut output = Vec::new();
        serve(
            Cursor::new(lines.as_bytes().to_vec()),
            &mut output,
            fake_handler,
        )
        .expect("serve");
        String::from_utf8(output)
            .expect("utf8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("response json"))
            .collect()
    }

    #[test]
    fn initialize_reports_tools_and_resources_capabilities() {
        let responses = run(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
        );
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["result"]["protocolVersion"], "2024-11-05");
        assert!(responses[0]["result"]["capabilities"]["tools"].is_object());
        assert!(responses[0]["result"]["capabilities"]["resources"].is_object());
    }

    #[test]
    fn tools_list_exposes_every_tool_with_schema() {
        let responses = run(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let tools = responses[0]["result"]["tools"].as_array().expect("tools");
        assert_eq!(tools.len(), crate::surface::tools().len());
        assert!(tools.iter().all(|tool| tool["inputSchema"].is_object()));
    }

    #[test]
    fn tools_call_dispatches_to_host() {
        let responses = run(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_scene_tree","arguments":{}}}"#,
        );
        assert_eq!(responses[0]["result"]["isError"], false);
        assert_eq!(
            responses[0]["result"]["structuredContent"]["tool"],
            "get_scene_tree"
        );
    }

    #[test]
    fn unknown_tool_is_a_tool_error_not_protocol_error() {
        let responses = run(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#,
        );
        assert_eq!(responses[0]["result"]["isError"], true);
        assert!(responses[0].get("error").is_none());
    }

    #[test]
    fn notifications_do_not_get_a_response() {
        let responses = run(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        assert!(responses.is_empty());
    }

    #[test]
    fn unknown_method_returns_method_not_found() {
        let responses = run(r#"{"jsonrpc":"2.0","id":5,"method":"nope/nope"}"#);
        assert_eq!(responses[0]["error"]["code"], -32601);
    }

    #[test]
    fn content_length_framing_is_supported() {
        let body = r#"{"jsonrpc":"2.0","id":6,"method":"ping"}"#;
        let input = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let responses = run(&input);
        assert_eq!(responses[0]["result"], json!({}));
    }

    #[test]
    fn resources_read_returns_text_contents() {
        let responses = run(
            r#"{"jsonrpc":"2.0","id":7,"method":"resources/read","params":{"uri":"rustlua://components"}}"#,
        );
        assert_eq!(
            responses[0]["result"]["contents"][0]["text"],
            "body:rustlua://components"
        );
    }
}
