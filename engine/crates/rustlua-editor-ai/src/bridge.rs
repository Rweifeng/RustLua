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
use std::time::Duration;

/// 单条 tool 调用等主线程回话的上限。写请求要进 GUI 确认队列，人工批准通常落在
/// 这个量级之外，所以宁可回一条「已排队」也不能把整条 stdio 通道挂死。
pub const DEFAULT_REPLY_WAIT: Duration = Duration::from_secs(30);

/// 超时后按 tier 组装「已排队」读数；未知 tool 由协议层先拦掉，不会走到这里。
fn queued_for_confirmation(name: &str) -> ToolError {
    ToolError::QueuedForConfirmation {
        tool: name.to_owned(),
        tier: tool_by_name(name).map_or("T?", |spec| tier_label(spec.tier)),
    }
}

/// 等主线程回话，但不许超过 `reply_wait`：请求可能只是停在确认队列里等人点，
/// 传输线程必须带着一条明确读数回到 stdio 循环去接下一个请求。
fn await_tool_reply(
    result: &Receiver<Result<Value, ToolError>>,
    name: &str,
    reply_wait: Duration,
) -> Result<Value, ToolError> {
    match result.recv_timeout(reply_wait) {
        Ok(value) => value,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(queued_for_confirmation(name)),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(ToolError::Host("工具执行通道已关闭".to_owned()))
        }
    }
}

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
    /// 写请求已在编辑器「AI 动作确认」里排队，但传输层不等人工点：先回这条，
    /// 否则一条未确认的写会把 stdio 通道连同后续所有只读调用一起挂死。
    /// 批准后动作仍会执行，拒绝则不改工作区。
    QueuedForConfirmation {
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
            Self::QueuedForConfirmation { tool, tier } => format!(
                "{tool}（{tier}）已在编辑器「AI 动作确认」里排队等待人工批准；批准后才会执行，重复提交只会多排一条"
            ),
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

/// 协议线程 → 主线程的请求。写请求在人工确认之前留在 [`AiBridge::pending`]，
/// 传输层按 [`DEFAULT_REPLY_WAIT`] 先回一条「已排队」，不挂住整条 stdio 通道。
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
            DEFAULT_REPLY_WAIT,
            wake,
        )
    }

    /// 可注入流的版本（测试用内存管道）。
    ///
    /// `reply_wait` 是单条请求等主线程回话的上限；超时只影响这一条请求的读数，
    /// 传输线程必须继续读下一行。
    pub fn start_io<R, W>(
        reader: R,
        writer: W,
        reply_wait: Duration,
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
                            let tool = name.clone();
                            request_tx
                                .send(HostRequest::Call { name, args, reply })
                                .map_err(|_| ToolError::Host("编辑器主线程已退出".to_owned()))?;
                            wake();
                            await_tool_reply(&result, &tool, reply_wait)
                        }
                        mcp::HostCall::Resource { uri } => {
                            let (reply, result) = mpsc::channel();
                            request_tx
                                .send(HostRequest::Resource { uri, reply })
                                .map_err(|_| ToolError::Host("编辑器主线程已退出".to_owned()))?;
                            wake();
                            let text = result
                                .recv_timeout(reply_wait)
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

    /// 停在确认队列里的写请求不能让 stdio 通道挂死：必须有界等待、回一条明确读数。
    #[test]
    fn unconfirmed_write_times_out_as_queued_instead_of_wedging_the_transport() {
        let (_sender, result) = mpsc::channel::<Result<Value, ToolError>>();
        let error = await_tool_reply(&result, "delete_entity", Duration::from_millis(20))
            .expect_err("主线程没回话，不该有 Ok");
        assert!(
            matches!(
                error,
                ToolError::QueuedForConfirmation { tier, .. } if tier == "T2"
            ),
            "{error}"
        );

        // 主线程照常回话时不引入任何延迟或语义变化。
        let (sender, answered) = mpsc::channel();
        sender.send(Ok(json!({"ok": true}))).expect("投递");
        assert_eq!(
            await_tool_reply(&answered, "create_entity", Duration::from_millis(500)),
            Ok(json!({"ok": true}))
        );

        // 通道关闭仍是宿主错误，不能冒充「已排队」。
        let (sender, closed) = mpsc::channel::<Result<Value, ToolError>>();
        drop(sender);
        assert!(matches!(
            await_tool_reply(&closed, "create_entity", Duration::from_millis(500)),
            Err(ToolError::Host(_))
        ));
    }

    /// 人在回路的完整一轮：写请求停在队列里（宿主没被碰、传输层按超时回「已排队」），
    /// 宿主批准后才执行，并把真实结果送回还在等的客户端。
    #[test]
    fn interactive_policy_queues_the_write_until_the_host_approves_it() {
        let (sender, requests) = mpsc::channel();
        let mut bridge = AiBridge {
            requests,
            pending: Vec::new(),
            _transport: None,
        };
        let (reply, result) = mpsc::channel();
        sender
            .send(HostRequest::Call {
                name: "delete_entity".to_owned(),
                args: json!({"entities": [1]}),
                reply,
            })
            .expect("投递");

        let mut host = FakeHost::default();
        // 第 1 帧：排队，不放行；客户端只能拿到有界的「已排队」读数。
        assert_eq!(bridge.pump(&mut host, GatePolicy::interactive(false)), 1);
        assert_eq!(bridge.pending().len(), 1);
        assert!(host.calls.is_empty(), "未批准的写不得到达宿主");
        assert!(matches!(
            await_tool_reply(&result, "delete_entity", Duration::from_millis(20)),
            Err(ToolError::QueuedForConfirmation { tier: "T2", .. })
        ));
        // 客户端超时不等于取消：队列项仍在，批准时照常执行（结果送回已丢弃的通道也只影响投递）。
        let (reply, result) = mpsc::channel();
        sender
            .send(HostRequest::Call {
                name: "delete_entity".to_owned(),
                args: json!({"entities": [2]}),
                reply,
            })
            .expect("投递");
        assert_eq!(bridge.pump(&mut host, GatePolicy::interactive(false)), 1);
        assert_eq!(bridge.pending().len(), 2);
        assert!(bridge.approve(1, &mut host));
        assert_eq!(
            await_tool_reply(&result, "delete_entity", Duration::from_millis(500)),
            Ok(json!({"ok": true, "tool": "delete_entity"}))
        );
        assert_eq!(bridge.pending().len(), 1, "批准只收走选中的那一条");
        // 拒绝同样不动文档，并且把原因回给客户端。
        bridge.reject(0, "用户在编辑器内拒绝");
        assert!(host.calls.len() == 1, "只有批准过的那条到达宿主");

        // 反面对照：显式放行的策略必须当帧执行、一条都不进队列——否则上面的
        // 「拦住」读数就只是队列本身的形状，测不到 policy 是否真的被读。
        let (sender, requests) = mpsc::channel();
        let mut bridge = AiBridge {
            requests,
            pending: Vec::new(),
            _transport: None,
        };
        let (reply, result) = mpsc::channel();
        sender
            .send(HostRequest::Call {
                name: "delete_entity".to_owned(),
                args: json!({"entities": [3]}),
                reply,
            })
            .expect("投递");
        let allow_all = GatePolicy {
            auto_t1: true,
            allow_t2: true,
            allow_t3: true,
            queue_unconfirmed: true,
        };
        let mut host = FakeHost::default();
        assert_eq!(bridge.pump(&mut host, allow_all), 1);
        assert!(!bridge.has_pending(), "放行策略下不该留下队列项");
        assert_eq!(host.calls.len(), 1);
        assert_eq!(
            await_tool_reply(&result, "delete_entity", Duration::from_millis(500)),
            Ok(json!({"ok": true, "tool": "delete_entity"}))
        );
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
