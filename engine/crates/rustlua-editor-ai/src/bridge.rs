//! 确认·事务网关与主线程执行桥。
//!
//! 协议线程**从不**接触 `EditorApp`（它不是 `Send`）：读 tool / 写 tool 都经
//! [`HostRequest`] 回到主线程，由宿主在 `EditorCommand` 的 dispatch 入口执行。
//! 写 tool 是否放行由 [`GatePolicy`] 决定；未放行的请求停在 [`AiBridge::pending`]，
//! 等 UI 显式批准/拒绝。

use crate::mcp;
use crate::surface::{ToolTier, tool_by_name};
use serde_json::Value;
use std::io::{BufRead, Write};
use std::panic::AssertUnwindSafe;
use std::sync::mpsc::{self, Receiver, Sender};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub enum ToolError {
    UnknownTool(String),
    InvalidArgs(String),
    /// 宿主业务拒绝（例如前置条件不满足、实体不存在）。
    Rejected(String),
    /// 确认网关未放行（无头/回放出现时是测试判红信号，不是可重试错误）。
    NeedsConfirmation {
        tool: String,
        tier: &'static str,
    },
    /// 宿主内部错误（IO、序列化等）。
    Host(String),
    /// 功能在当前运行形态不可用（如无 GUI 的截图）。
    Unavailable(String),
}

impl ToolError {
    pub fn message(&self) -> String {
        match self {
            Self::UnknownTool(name) => format!("未知 tool：{name}"),
            Self::InvalidArgs(reason) => format!("参数错误：{reason}"),
            Self::Rejected(reason) => format!("被拒绝：{reason}"),
            Self::NeedsConfirmation { tool, tier } => {
                format!("{tool}（{tier}）需要编辑器内确认")
            }
            Self::Host(reason) => format!("宿主错误：{reason}"),
            Self::Unavailable(reason) => format!("不可用：{reason}"),
        }
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message())
    }
}

impl std::error::Error for ToolError {}

/// 宿主必须实现的能力面。返回值一律是可 JSON 化的结构，便于 MCP 原样透传。
pub trait ToolHost {
    fn call_tool(&mut self, name: &str, args: &Value) -> Result<Value, ToolError>;
    fn read_resource(&mut self, uri: &str) -> Result<String, ToolError>;
    /// 确定性回放用：当前文档的可比较快照（默认不参与）。
    fn snapshot(&mut self) -> Option<Value> {
        None
    }
}

/// 确认策略。默认（[`GatePolicy::STRICT`]）所有写动作都要人确认；
/// T1 自动批处理是显式勾选的能力，不是默认值。
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub struct GatePolicy {
    /// 自动放行 T1（场景内可撤销写）。
    pub auto_t1: bool,
    /// 自动放行 T2（破坏性/跨文件写）。默认永远 false。
    pub allow_t2: bool,
    /// 自动放行 T3（工作区/会话级写）。默认永远 false。
    pub allow_t3: bool,
    /// 未放行的写请求是排队等 UI 确认（true）还是直接拒绝（false，无头/回放）。
    pub queue_unconfirmed: bool,
}

impl Default for GatePolicy {
    fn default() -> Self {
        Self::STRICT
    }
}

impl GatePolicy {
    /// 人在回路：写动作排队等确认（T1 也确认）。
    pub const STRICT: Self = Self {
        auto_t1: false,
        allow_t2: false,
        allow_t3: false,
        queue_unconfirmed: true,
    };
    /// 编辑器默认：T1 排队确认，T2/T3 同样排队。
    pub const fn interactive(auto_t1: bool) -> Self {
        Self {
            auto_t1,
            allow_t2: false,
            allow_t3: false,
            queue_unconfirmed: true,
        }
    }
    /// 无头/回放：T1 自动放行，T2/T3 显式拒绝（不挂起）。
    pub const HEADLESS: Self = Self {
        auto_t1: true,
        allow_t2: false,
        allow_t3: false,
        queue_unconfirmed: false,
    };

    pub fn allows(&self, tier: ToolTier) -> bool {
        match tier {
            ToolTier::Read => true,
            ToolTier::T1 => self.auto_t1,
            ToolTier::T2 => self.allow_t2,
            ToolTier::T3 => self.allow_t3,
        }
    }
}

/// 直接执行一次 tool（不经传输层）：内置面板、agent 与回放门共用。
pub fn execute_tool(
    host: &mut dyn ToolHost,
    policy: GatePolicy,
    name: &str,
    args: &Value,
) -> Result<Value, ToolError> {
    let spec = tool_by_name(name).ok_or_else(|| ToolError::UnknownTool(name.to_owned()))?;
    if spec.tier.is_write() && !policy.allows(spec.tier) {
        return Err(ToolError::NeedsConfirmation {
            tool: name.to_owned(),
            tier: tier_label(spec.tier),
        });
    }
    call_host(host, name, args)
}

/// 宿主 panic 不穿透 UI 线程：桥接层是协议与编辑器之间的隔离带。
fn call_host(host: &mut dyn ToolHost, name: &str, args: &Value) -> Result<Value, ToolError> {
    std::panic::catch_unwind(AssertUnwindSafe(|| host.call_tool(name, args))).unwrap_or_else(
        |payload| {
            Err(ToolError::Host(format!(
                "tool {name} panic：{}",
                panic_message(payload)
            )))
        },
    )
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        return (*text).to_owned();
    }
    if let Some(text) = payload.downcast_ref::<String>() {
        return text.clone();
    }
    "未知 panic".to_owned()
}

pub fn tier_label(tier: ToolTier) -> &'static str {
    match tier {
        ToolTier::Read => "T0",
        ToolTier::T1 => "T1",
        ToolTier::T2 => "T2",
        ToolTier::T3 => "T3",
    }
}

/// 协议线程 → 主线程的请求。写请求的 `reply` 在确认之前一直挂着，MCP 客户端因此
/// 看到的是「等待人工确认」而不是静默成功。
pub enum HostRequest {
    Call {
        name: String,
        args: Value,
        reply: Sender<Result<Value, ToolError>>,
    },
    Resource {
        uri: String,
        reply: Sender<Result<String, ToolError>>,
    },
}

pub struct PendingToolCall {
    pub name: String,
    pub args: Value,
    pub tier: ToolTier,
    reply: Sender<Result<Value, ToolError>>,
}

impl std::fmt::Debug for PendingToolCall {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PendingToolCall")
            .field("name", &self.name)
            .field("args", &self.args)
            .field("tier", &self.tier)
            .finish()
    }
}

impl PendingToolCall {
    fn reject(self, reason: impl Into<String>) {
        let _ = self.reply.send(Err(ToolError::Rejected(reason.into())));
    }
}

/// 主线程侧句柄。`AiBridge` 本身不实现 `Send` 约束以外的任何线程逻辑；
/// 协议线程只持有发送端。
pub struct AiBridge {
    requests: Receiver<HostRequest>,
    pending: Vec<PendingToolCall>,
    /// 保留线程句柄只为诊断/测试收尾；生产进程随窗口退出。
    _transport: Option<TransportHandle>,
}

struct TransportHandle {
    _thread: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for AiBridge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiBridge")
            .field("pending", &self.pending.len())
            .finish()
    }
}

impl Default for AiBridge {
    fn default() -> Self {
        let (_sender, receiver) = mpsc::channel();
        Self {
            requests: receiver,
            pending: Vec::new(),
            _transport: None,
        }
    }
}

impl AiBridge {
    /// 无传输层（测试/内置面板直连）。请求通道永远为空。
    pub fn detached() -> Self {
        Self::default()
    }

    /// 启动 MCP stdio server（编辑器进程由 MCP 客户端拉起时使用）。
    ///
    /// `wake` 在请求投递到主线程通道后调用：GUI 空闲时 egui 不重绘，主线程只在
    /// 重绘帧里 `pump`，不唤醒就会让 tool 调用一直挂到下一次鼠标事件（客户端判超时）。
    pub fn start_stdio(wake: impl Fn() + Send + 'static) -> std::io::Result<Self> {
        Self::start_io(
            std::io::BufReader::new(std::io::stdin()),
            std::io::stdout(),
            wake,
        )
    }

    /// 可注入流的版本（测试用内存管道）。
    pub fn start_io<R, W>(
        reader: R,
        writer: W,
        wake: impl Fn() + Send + 'static,
    ) -> std::io::Result<Self>
    where
        R: BufRead + Send + 'static,
        W: Write + Send + 'static,
    {
        let (request_tx, requests) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("rustlua-mcp-stdio".to_owned())
            .spawn(move || {
                let handler = move |request: mcp::HostCall| -> Result<Value, ToolError> {
                    match request {
                        mcp::HostCall::Tool { name, args } => {
                            let (reply, result) = mpsc::channel();
                            request_tx
                                .send(HostRequest::Call { name, args, reply })
                                .map_err(|_| ToolError::Host("编辑器主线程已退出".to_owned()))?;
                            wake();
                            result
                                .recv()
                                .map_err(|_| ToolError::Host("工具执行通道已关闭".to_owned()))?
                        }
                        mcp::HostCall::Resource { uri } => {
                            let (reply, result) = mpsc::channel();
                            request_tx
                                .send(HostRequest::Resource { uri, reply })
                                .map_err(|_| ToolError::Host("编辑器主线程已退出".to_owned()))?;
                            wake();
                            let text = result
                                .recv()
                                .map_err(|_| ToolError::Host("资源读取通道已关闭".to_owned()))??;
                            Ok(Value::String(text))
                        }
                    }
                };
                if let Err(error) = mcp::serve(reader, writer, handler) {
                    eprintln!("[mcp] server 退出：{error}");
                }
            })?;
        Ok(Self {
            requests,
            pending: Vec::new(),
            _transport: Some(TransportHandle {
                _thread: Some(thread),
            }),
        })
    }

    /// 每帧调用：处理已到达的请求。返回本帧执行的请求数。
    ///
    /// 写请求在策略不允许时进入 `pending`；`queue_unconfirmed=false` 时立即拒绝。
    pub fn pump(&mut self, host: &mut dyn ToolHost, policy: GatePolicy) -> usize {
        let mut handled = 0;
        while let Ok(request) = self.requests.try_recv() {
            match request {
                HostRequest::Call { name, args, reply } => {
                    let Some(spec) = tool_by_name(&name) else {
                        let _ = reply.send(Err(ToolError::UnknownTool(name)));
                        handled += 1;
                        continue;
                    };
                    if spec.tier.is_write() && !policy.allows(spec.tier) {
                        if policy.queue_unconfirmed {
                            self.pending.push(PendingToolCall {
                                name,
                                args,
                                tier: spec.tier,
                                reply,
                            });
                        } else {
                            let _ = reply.send(Err(ToolError::NeedsConfirmation {
                                tool: name,
                                tier: tier_label(spec.tier),
                            }));
                        }
                        handled += 1;
                        continue;
                    }
                    let result = call_host(host, &name, &args);
                    let _ = reply.send(result);
                    handled += 1;
                }
                HostRequest::Resource { uri, reply } => {
                    let _ = reply.send(host.read_resource(&uri));
                    handled += 1;
                }
            }
        }
        handled
    }

    pub fn pending(&self) -> &[PendingToolCall] {
        &self.pending
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn approve(&mut self, index: usize, host: &mut dyn ToolHost) -> bool {
        if index >= self.pending.len() {
            return false;
        }
        let call = self.pending.remove(index);
        let result = call_host(host, &call.name, &call.args);
        let _ = call.reply.send(result);
        true
    }

    pub fn reject(&mut self, index: usize, reason: impl Into<String>) -> bool {
        if index >= self.pending.len() {
            return false;
        }
        self.pending.remove(index).reject(reason);
        true
    }

    pub fn reject_all(&mut self, reason: impl Into<String>) {
        let reason = reason.into();
        for call in self.pending.drain(..) {
            call.reject(reason.clone());
        }
    }
}

impl Drop for AiBridge {
    fn drop(&mut self) {
        self.reject_all("编辑器正在退出");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Default)]
    struct FakeHost {
        calls: Vec<(String, Value)>,
        snapshot: Value,
    }

    impl ToolHost for FakeHost {
        fn call_tool(&mut self, name: &str, args: &Value) -> Result<Value, ToolError> {
            self.calls.push((name.to_owned(), args.clone()));
            self.snapshot = json!({"last": name});
            Ok(json!({"ok": true, "tool": name}))
        }

        fn read_resource(&mut self, uri: &str) -> Result<String, ToolError> {
            Ok(format!("resource:{uri}"))
        }

        fn snapshot(&mut self) -> Option<Value> {
            Some(self.snapshot.clone())
        }
    }

    #[test]
    fn headless_policy_runs_t1_and_rejects_t2() {
        let mut host = FakeHost::default();
        let read = execute_tool(
            &mut host,
            GatePolicy::HEADLESS,
            "get_scene_tree",
            &json!({}),
        );
        assert!(read.is_ok());

        let t1 = execute_tool(
            &mut host,
            GatePolicy::HEADLESS,
            "create_entity",
            &json!({"name": "A"}),
        );
        assert!(t1.is_ok());

        let t2 = execute_tool(
            &mut host,
            GatePolicy::HEADLESS,
            "delete_entity",
            &json!({"entities": [1]}),
        );
        assert!(matches!(t2, Err(ToolError::NeedsConfirmation { .. })));
        assert_eq!(host.calls.len(), 2, "被确认门拦住的请求不得到达宿主");
    }

    #[test]
    fn strict_policy_needs_confirmation_for_every_write() {
        let mut host = FakeHost::default();
        let result = execute_tool(
            &mut host,
            GatePolicy::STRICT,
            "create_entity",
            &json!({"name": "A"}),
        );
        assert!(matches!(result, Err(ToolError::NeedsConfirmation { .. })));
        assert!(host.calls.is_empty());
    }

    #[test]
    fn unknown_tool_is_reported_without_reaching_host() {
        let mut host = FakeHost::default();
        let result = execute_tool(&mut host, GatePolicy::HEADLESS, "nope", &json!({}));
        assert!(matches!(result, Err(ToolError::UnknownTool(_))));
        assert!(host.calls.is_empty());
    }

    struct PanicHost;

    impl ToolHost for PanicHost {
        fn call_tool(&mut self, _name: &str, _args: &Value) -> Result<Value, ToolError> {
            panic!("boom")
        }

        fn read_resource(&mut self, _uri: &str) -> Result<String, ToolError> {
            Ok(String::new())
        }
    }

    #[test]
    fn host_panic_is_reported_as_tool_error() {
        let mut host = PanicHost;
        let result = execute_tool(
            &mut host,
            GatePolicy::HEADLESS,
            "get_scene_tree",
            &json!({}),
        );
        assert!(
            matches!(&result, Err(ToolError::Host(message)) if message.contains("boom")),
            "{result:?}"
        );
    }
}
