//! 内置 Assistant 的模型 Provider 抽象（M75-03）。
//!
//! 外部 MCP 客户端与内置面板共用同一份 [`ToolSpec`](crate::surface::ToolSpec)；本模块只负责
//! 把「对话 + 工具面」翻译成 OpenAI 兼容 / Anthropic 的请求，并把响应翻译回 [`ModelResponse`]。
//! 工具执行不在这里发生——面板拿到 tool call 后仍走 `ToolHost` / `execute_tool`。
//!
//! **密钥边界**：API Key 只存在于进程内存与 `curl --config -` 的 stdin 中；不写磁盘、不进
//! argv、不进请求体、不进对话上下文。`ProviderConfig` 的 `Debug` 已脱敏。
//!
//! 不新增 HTTP 客户端依赖：走系统 `curl`（`RUSTLUA_CURL` 可覆盖程序名/路径）。请求体与响应
//! 各写一个临时文件（只含对话与工具面），请求结束即删；密钥只出现在 stdin 配置里。

use crate::surface::ToolSpec;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

pub const DEFAULT_MAX_TOKENS: u32 = 4096;
pub const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";

/// 内置助手的系统提示。写动作的风险层由编辑器网关兜底，提示词只负责行为引导。
pub const SYSTEM_PROMPT: &str = "\
你是 RustLua 编辑器的内置助手，可以调用工具读取和修改当前工程。

规则：
- 先读后写：不确定实体 id、组件名或字段时，先用读工具确认。
- 所有写工具都要经过编辑器的确认网关：T1 可在场景内撤销，T2/T3 每次都需要用户批准。
  不要把尚未获批的动作说成已经完成；被拒绝时如实说明并询问下一步。
- 修改尽量原子、最小；完成后用一两句话说明改了什么（带实体 id/名称）。
- 写 Lua 时严格遵循工具面里的 API 契约，不要臆造函数名。
- 用用户的语言回答（默认中文）。";

/// 模型服务商类型。`OpenAi` 覆盖所有 OpenAI 兼容端点（含 LM Studio/vLLM）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    #[default]
    OpenAi,
    Anthropic,
    Ollama,
}

impl ProviderKind {
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "openai" | "open-ai" | "openai-compatible" | "openai_compatible" => Some(Self::OpenAi),
            "anthropic" | "claude" => Some(Self::Anthropic),
            "ollama" | "local" => Some(Self::Ollama),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI 兼容",
            Self::Anthropic => "Anthropic",
            Self::Ollama => "本地 Ollama",
        }
    }

    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::OpenAi => "https://api.openai.com/v1",
            Self::Anthropic => "https://api.anthropic.com/v1",
            Self::Ollama => "http://127.0.0.1:11434/v1",
        }
    }

    pub fn default_model(self) -> &'static str {
        match self {
            Self::OpenAi => "gpt-4o-mini",
            Self::Anthropic => "claude-sonnet-4-5",
            Self::Ollama => "qwen2.5-coder:7b",
        }
    }

    /// 面板提示用：这些环境变量里的 Key 会被自动采用（UI 输入优先）。
    pub fn api_key_env_names(self) -> &'static [&'static str] {
        match self {
            Self::OpenAi => &["RUSTLUA_AI_API_KEY", "OPENAI_API_KEY"],
            Self::Anthropic => &["RUSTLUA_AI_API_KEY", "ANTHROPIC_API_KEY"],
            Self::Ollama => &["RUSTLUA_AI_API_KEY"],
        }
    }
}

/// 可持久化的 Provider 配置：**不含密钥**。密钥由 UI 临时输入或环境变量在请求时解析。
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ProviderSettings {
    pub kind: ProviderKind,
    pub base_url: String,
    pub model: String,
    pub max_tokens: u32,
}

impl Default for ProviderSettings {
    fn default() -> Self {
        Self {
            kind: ProviderKind::default(),
            base_url: ProviderKind::default().default_base_url().to_owned(),
            model: ProviderKind::default().default_model().to_owned(),
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }
}

impl ProviderSettings {
    pub fn for_kind(kind: ProviderKind) -> Self {
        Self {
            kind,
            base_url: kind.default_base_url().to_owned(),
            model: kind.default_model().to_owned(),
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }

    /// 切换服务商时把仍是旧默认值的字段换成新默认值，保留用户自定义。
    pub fn switch_kind(&mut self, kind: ProviderKind) {
        if self.base_url.trim().is_empty() || self.kind.default_base_url() == self.base_url {
            self.base_url = kind.default_base_url().to_owned();
        }
        if self.model.trim().is_empty() || self.kind.default_model() == self.model {
            self.model = kind.default_model().to_owned();
        }
        self.kind = kind;
    }
}

/// 一次请求所需的完整配置。`api_key` 仅驻留内存；为空时回退到环境变量。
#[derive(Clone, Eq, PartialEq)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub max_tokens: u32,
}

impl fmt::Debug for ProviderConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderConfig")
            .field("kind", &self.kind)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("max_tokens", &self.max_tokens)
            .field(
                "api_key",
                &if self.api_key.is_empty() {
                    "<unset>"
                } else {
                    "<redacted>"
                },
            )
            .finish()
    }
}

impl ProviderConfig {
    pub fn new(kind: ProviderKind) -> Self {
        Self::from_settings(&ProviderSettings::for_kind(kind))
    }

    pub fn from_settings(settings: &ProviderSettings) -> Self {
        Self {
            kind: settings.kind,
            base_url: settings.base_url.clone(),
            model: settings.model.clone(),
            api_key: String::new(),
            max_tokens: settings.max_tokens.max(1),
        }
    }

    /// 从环境变量读非密钥配置；密钥留空，由 [`Self::resolve_api_key`] 在请求时解析，
    /// 避免把环境变量里的密钥复制进长期驻留的应用状态。
    pub fn from_env() -> Self {
        let kind = std::env::var("RUSTLUA_AI_PROVIDER")
            .ok()
            .and_then(|value| ProviderKind::from_name(&value))
            .unwrap_or_default();
        let mut settings = ProviderSettings::for_kind(kind);
        let base = std::env::var("RUSTLUA_AI_BASE_URL").ok().or_else(|| {
            let name = match kind {
                ProviderKind::OpenAi => "OPENAI_BASE_URL",
                ProviderKind::Anthropic => "ANTHROPIC_BASE_URL",
                ProviderKind::Ollama => "OLLAMA_HOST",
            };
            std::env::var(name).ok()
        });
        if let Some(base) = base.map(|value| value.trim().to_owned())
            && !base.is_empty()
        {
            settings.base_url = if kind == ProviderKind::Ollama && !base.contains("://") {
                format!("http://{base}")
            } else {
                base
            };
        }
        if let Ok(model) = std::env::var("RUSTLUA_AI_MODEL") {
            let model = model.trim().to_owned();
            if !model.is_empty() {
                settings.model = model;
            }
        }
        if let Some(tokens) = std::env::var("RUSTLUA_AI_MAX_TOKENS")
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
        {
            settings.max_tokens = tokens.clamp(1, 32768);
        }
        Self::from_settings(&settings)
    }

    pub fn settings(&self) -> ProviderSettings {
        ProviderSettings {
            kind: self.kind,
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            max_tokens: self.max_tokens,
        }
    }

    /// UI 里填的 Key 优先；否则按服务商读取环境变量。
    pub fn resolve_api_key(&self) -> Option<String> {
        let own = self.api_key.trim();
        if !own.is_empty() {
            return Some(own.to_owned());
        }
        self.kind
            .api_key_env_names()
            .iter()
            .find_map(|name| std::env::var(name).ok())
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    }

    pub fn has_env_key(&self) -> bool {
        self.kind
            .api_key_env_names()
            .iter()
            .any(|name| std::env::var(name).is_ok_and(|value| !value.trim().is_empty()))
    }

    pub fn endpoint(&self) -> String {
        let fallback = self.kind.default_base_url();
        let base = {
            let trimmed = self.base_url.trim();
            if trimmed.is_empty() {
                fallback
            } else {
                trimmed
            }
        };
        let base = base.trim_end_matches('/');
        match self.kind {
            ProviderKind::Anthropic => {
                if base.ends_with("/messages") {
                    base.to_owned()
                } else if base.ends_with("/v1") {
                    format!("{base}/messages")
                } else {
                    format!("{base}/v1/messages")
                }
            }
            ProviderKind::OpenAi | ProviderKind::Ollama => {
                if base.ends_with("/chat/completions") {
                    base.to_owned()
                } else if base.ends_with("/v1") {
                    format!("{base}/chat/completions")
                } else {
                    format!("{base}/v1/chat/completions")
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelResponse {
    pub text: String,
    pub tool_calls: Vec<ModelToolCall>,
    pub stop_reason: Option<String>,
}

/// 一家模型的对话消息。枚举保证每种角色都能无损翻译成两家的线上格式。
#[derive(Clone, Debug, PartialEq)]
pub enum ChatMessage {
    System(String),
    User(String),
    Assistant {
        text: String,
        tool_calls: Vec<ModelToolCall>,
    },
    Tool {
        call_id: String,
        name: String,
        content: String,
    },
}

impl ChatMessage {
    pub fn user(text: impl Into<String>) -> Self {
        Self::User(text.into())
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self::System(text.into())
    }

    pub fn assistant(text: impl Into<String>, tool_calls: Vec<ModelToolCall>) -> Self {
        Self::Assistant {
            text: text.into(),
            tool_calls,
        }
    }

    pub fn tool(
        call_id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self::Tool {
            call_id: call_id.into(),
            name: name.into(),
            content: content.into(),
        }
    }
}

#[derive(Debug)]
pub enum ProviderError {
    Config(String),
    Transport(String),
    Http { status: u16, message: String },
    Parse(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(message) => write!(formatter, "Provider 配置错误：{message}"),
            Self::Transport(message) => write!(formatter, "Provider 传输错误：{message}"),
            Self::Http { status, message } => {
                write!(formatter, "Provider 返回 HTTP {status}：{message}")
            }
            Self::Parse(message) => write!(formatter, "Provider 响应解析失败：{message}"),
        }
    }
}

impl std::error::Error for ProviderError {}

pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

// 只打印头名与 body 长度：Authorization 等值可能含密钥，绝不进日志。
impl fmt::Debug for HttpRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let header_names: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        formatter
            .debug_struct("HttpRequest")
            .field("url", &self.url)
            .field("headers", &header_names)
            .field("body_len", &self.body.len())
            .finish()
    }
}

#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

pub trait Transport: Send + Sync {
    fn post(&self, request: &HttpRequest) -> Result<HttpResponse, ProviderError>;
}

/// 系统 curl 传输。配置（含密钥）走 stdin（`--config -`），请求体/响应走临时文件。
#[derive(Clone, Debug)]
pub struct CurlTransport {
    pub program: String,
    pub max_time_secs: u32,
}

impl Default for CurlTransport {
    fn default() -> Self {
        Self {
            program: std::env::var("RUSTLUA_CURL")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "curl".to_owned()),
            max_time_secs: 120,
        }
    }
}

impl Transport for CurlTransport {
    fn post(&self, request: &HttpRequest) -> Result<HttpResponse, ProviderError> {
        if request.url.chars().any(char::is_control) {
            return Err(ProviderError::Config("请求 URL 含控制字符".to_owned()));
        }
        for (name, value) in &request.headers {
            if name.chars().any(char::is_control) || value.chars().any(char::is_control) {
                return Err(ProviderError::Config("请求头含控制字符".to_owned()));
            }
        }
        let scratch = ScratchDir::create()?;
        let body_path = scratch.path.join("request.json");
        let response_path = scratch.path.join("response.json");
        std::fs::write(&body_path, request.body.as_bytes())
            .map_err(|error| ProviderError::Transport(format!("写请求临时文件失败：{error}")))?;
        let config = self.curl_config(request, &body_path, &response_path);

        let mut child = Command::new(&self.program)
            .arg("--config")
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                ProviderError::Transport(format!(
                    "无法启动 curl（{}）：{error}；可用 RUSTLUA_CURL 指定路径",
                    self.program
                ))
            })?;
        {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| ProviderError::Transport("curl stdin 不可用".to_owned()))?;
            stdin
                .write_all(config.as_bytes())
                .map_err(|error| ProviderError::Transport(format!("写 curl 配置失败：{error}")))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|error| ProviderError::Transport(format!("等待 curl 失败：{error}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ProviderError::Transport(format!(
                "curl 退出 {}：{}",
                output.status,
                stderr.trim()
            )));
        }
        let status_text = String::from_utf8_lossy(&output.stdout);
        let status = status_text.trim().parse::<u16>().map_err(|_| {
            ProviderError::Transport(format!("curl 未返回 HTTP 状态码：{}", status_text.trim()))
        })?;
        let body = std::fs::read(&response_path)
            .map_err(|error| ProviderError::Transport(format!("读响应临时文件失败：{error}")))?;
        Ok(HttpResponse {
            status,
            body: String::from_utf8_lossy(&body).into_owned(),
        })
    }
}

impl CurlTransport {
    fn curl_config(&self, request: &HttpRequest, body: &Path, response: &Path) -> String {
        let mut config = String::new();
        config.push_str("silent\nshow-error\nlocation\n");
        // 本地模型服务不应被进程级代理劫持（curl 默认会读 http_proxy）。
        if is_loopback_url(&request.url) {
            config.push_str("noproxy = \"*\"\n");
        }
        config.push_str(&format!("max-time = \"{}\"\n", self.max_time_secs));
        config.push_str(&format!("url = \"{}\"\n", curl_escape(&request.url)));
        config.push_str("request = \"POST\"\n");
        for (name, value) in &request.headers {
            config.push_str(&format!(
                "header = \"{}: {}\"\n",
                curl_escape(name),
                curl_escape(value)
            ));
        }
        config.push_str(&format!(
            "data-binary = \"@{}\"\n",
            curl_escape(&body.to_string_lossy())
        ));
        config.push_str(&format!(
            "output = \"{}\"\n",
            curl_escape(&response.to_string_lossy())
        ));
        config.push_str("write-out = \"%{http_code}\"\n");
        config
    }
}

fn curl_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn is_loopback_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://127.0.0.1")
        || lower.starts_with("http://localhost")
        || lower.starts_with("http://[::1]")
}

struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    fn create() -> Result<Self, ProviderError> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        for _ in 0..16 {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0);
            let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "rustlua-editor-ai-{}-{nanos}-{serial}",
                std::process::id()
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(ProviderError::Transport(format!(
                        "创建临时目录失败：{error}"
                    )));
                }
            }
        }
        Err(ProviderError::Transport("无法分配临时目录".to_owned()))
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

pub struct Provider {
    config: ProviderConfig,
    transport: Box<dyn Transport>,
}

impl Provider {
    pub fn new(config: ProviderConfig) -> Self {
        Self {
            config,
            transport: Box::new(CurlTransport::default()),
        }
    }

    pub fn with_transport(config: ProviderConfig, transport: Box<dyn Transport>) -> Self {
        Self { config, transport }
    }

    pub fn config(&self) -> &ProviderConfig {
        &self.config
    }

    pub fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSpec],
    ) -> Result<ModelResponse, ProviderError> {
        let request = self.build_request(messages, tools)?;
        let response = self.transport.post(&request)?;
        self.parse_response(response)
    }

    pub fn build_request(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSpec],
    ) -> Result<HttpRequest, ProviderError> {
        let model = self.config.model.trim();
        if model.is_empty() {
            return Err(ProviderError::Config("模型名不能为空".to_owned()));
        }
        let url = self.config.endpoint();
        if url.chars().any(char::is_control) {
            return Err(ProviderError::Config("Base URL 含控制字符".to_owned()));
        }
        let key = self.config.resolve_api_key();
        if self.config.kind == ProviderKind::Anthropic && key.is_none() {
            return Err(ProviderError::Config(
                "Anthropic 需要 API Key：在面板填入，或设置 ANTHROPIC_API_KEY".to_owned(),
            ));
        }
        if let Some(key) = &key
            && key.chars().any(char::is_control)
        {
            return Err(ProviderError::Config("API Key 含控制字符".to_owned()));
        }

        let (body, headers) = match self.config.kind {
            ProviderKind::OpenAi | ProviderKind::Ollama => {
                let mut body = json!({
                    "model": model,
                    "messages": openai_messages(messages),
                    "max_tokens": self.config.max_tokens,
                });
                if !tools.is_empty() {
                    body["tools"] = Value::Array(openai_tools(tools));
                }
                let mut headers = vec![("Content-Type".to_owned(), "application/json".to_owned())];
                if let Some(key) = &key {
                    headers.push(("Authorization".to_owned(), format!("Bearer {key}")));
                }
                (body, headers)
            }
            ProviderKind::Anthropic => {
                let (system, messages) = anthropic_messages(messages);
                let mut body = json!({
                    "model": model,
                    "max_tokens": self.config.max_tokens,
                    "messages": messages,
                });
                if let Some(system) = system {
                    body["system"] = Value::String(system);
                }
                if !tools.is_empty() {
                    body["tools"] = Value::Array(anthropic_tools(tools));
                }
                let headers = vec![
                    ("Content-Type".to_owned(), "application/json".to_owned()),
                    ("x-api-key".to_owned(), key.unwrap_or_default()),
                    (
                        "anthropic-version".to_owned(),
                        DEFAULT_ANTHROPIC_VERSION.to_owned(),
                    ),
                ];
                (body, headers)
            }
        };
        let body = serde_json::to_string(&body)
            .map_err(|error| ProviderError::Parse(format!("请求序列化失败：{error}")))?;
        Ok(HttpRequest { url, headers, body })
    }

    pub fn parse_response(&self, response: HttpResponse) -> Result<ModelResponse, ProviderError> {
        if !(200..300).contains(&response.status) {
            return Err(ProviderError::Http {
                status: response.status,
                message: api_error_message(&response.body),
            });
        }
        match self.config.kind {
            ProviderKind::OpenAi | ProviderKind::Ollama => parse_openai_response(&response.body),
            ProviderKind::Anthropic => parse_anthropic_response(&response.body),
        }
    }
}

fn openai_messages(messages: &[ChatMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| match message {
            ChatMessage::System(text) => json!({"role": "system", "content": text}),
            ChatMessage::User(text) => json!({"role": "user", "content": text}),
            ChatMessage::Assistant { text, tool_calls } => {
                let calls: Vec<Value> = tool_calls
                    .iter()
                    .map(|call| {
                        json!({
                            "id": call.id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": serde_json::to_string(&call.arguments)
                                    .unwrap_or_else(|_| "{}".to_owned()),
                            },
                        })
                    })
                    .collect();
                let mut value = json!({"role": "assistant", "content": text});
                if !calls.is_empty() {
                    value["tool_calls"] = Value::Array(calls);
                }
                value
            }
            ChatMessage::Tool {
                call_id, content, ..
            } => json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": content,
            }),
        })
        .collect()
}

fn anthropic_messages(messages: &[ChatMessage]) -> (Option<String>, Vec<Value>) {
    let mut system = Vec::new();
    let mut converted = Vec::new();
    for message in messages {
        match message {
            ChatMessage::System(text) => {
                if !text.trim().is_empty() {
                    system.push(text.clone());
                }
            }
            ChatMessage::User(text) => {
                converted.push(json!({
                    "role": "user",
                    "content": [{"type": "text", "text": text}],
                }));
            }
            ChatMessage::Assistant { text, tool_calls } => {
                let mut blocks = Vec::new();
                if !text.is_empty() {
                    blocks.push(json!({"type": "text", "text": text}));
                }
                for call in tool_calls {
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": call.arguments,
                    }));
                }
                if blocks.is_empty() {
                    blocks.push(json!({"type": "text", "text": ""}));
                }
                converted.push(json!({"role": "assistant", "content": blocks}));
            }
            ChatMessage::Tool {
                call_id, content, ..
            } => {
                converted.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": call_id,
                        "content": content,
                    }],
                }));
            }
        }
    }
    let system = if system.is_empty() {
        None
    } else {
        Some(system.join("\n\n"))
    };
    (system, converted)
}

fn openai_tools(tools: &[ToolSpec]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                },
            })
        })
        .collect()
}

fn anthropic_tools(tools: &[ToolSpec]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool.input_schema,
            })
        })
        .collect()
}

fn parse_openai_response(body: &str) -> Result<ModelResponse, ProviderError> {
    let value: Value = serde_json::from_str(body)
        .map_err(|error| ProviderError::Parse(format!("响应不是合法 JSON：{error}")))?;
    let choice = value
        .get("choices")
        .and_then(|choices| choices.get(0))
        .ok_or_else(|| ProviderError::Parse("响应缺少 choices[0]".to_owned()))?;
    let message = choice
        .get("message")
        .ok_or_else(|| ProviderError::Parse("响应缺少 message".to_owned()))?;
    let text = content_to_text(message.get("content"));
    let tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .enumerate()
                .map(|(index, call)| parse_openai_tool_call(call, index))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(ModelResponse {
        text,
        tool_calls,
        stop_reason: choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

fn content_to_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn parse_openai_tool_call(value: &Value, index: usize) -> Result<ModelToolCall, ProviderError> {
    let function = value
        .get("function")
        .ok_or_else(|| ProviderError::Parse(format!("tool_calls[{index}] 缺少 function")))?;
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| ProviderError::Parse(format!("tool_calls[{index}] 缺少 name")))?
        .to_owned();
    let arguments = match function.get("arguments") {
        None | Some(Value::Null) => json!({}),
        Some(Value::String(raw)) if raw.trim().is_empty() => json!({}),
        Some(Value::String(raw)) => serde_json::from_str(raw).map_err(|error| {
            ProviderError::Parse(format!("tool「{name}」的 arguments 不是合法 JSON：{error}"))
        })?,
        Some(value) if value.is_object() => value.clone(),
        Some(other) => {
            return Err(ProviderError::Parse(format!(
                "tool「{name}」的 arguments 类型异常：{other}"
            )));
        }
    };
    if !arguments.is_object() {
        return Err(ProviderError::Parse(format!(
            "tool「{name}」的 arguments 必须是 JSON 对象"
        )));
    }
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("call_{index}"));
    Ok(ModelToolCall {
        id,
        name,
        arguments,
    })
}

fn parse_anthropic_response(body: &str) -> Result<ModelResponse, ProviderError> {
    let value: Value = serde_json::from_str(body)
        .map_err(|error| ProviderError::Parse(format!("响应不是合法 JSON：{error}")))?;
    let blocks = value
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| ProviderError::Parse("响应缺少 content 数组".to_owned()))?;
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(part) = block.get("text").and_then(Value::as_str) {
                    text.push_str(part);
                }
            }
            Some("tool_use") => {
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| ProviderError::Parse("tool_use 缺少 name".to_owned()))?
                    .to_owned();
                let arguments = block.get("input").cloned().unwrap_or_else(|| json!({}));
                if !arguments.is_object() {
                    return Err(ProviderError::Parse(format!(
                        "tool「{name}」的 input 必须是 JSON 对象"
                    )));
                }
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("tool_{}", tool_calls.len()));
                tool_calls.push(ModelToolCall {
                    id,
                    name,
                    arguments,
                });
            }
            // thinking 等块不进入对话（会在下一轮作为历史丢掉，属已知限制）。
            _ => {}
        }
    }
    Ok(ModelResponse {
        text,
        tool_calls,
        stop_reason: value
            .get("stop_reason")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

fn api_error_message(body: &str) -> String {
    let parsed = serde_json::from_str::<Value>(body).ok();
    let message = parsed
        .as_ref()
        .and_then(|value| value.pointer("/error/message"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| body.trim().to_owned());
    let message = message.trim();
    if message.is_empty() {
        return "（空响应体）".to_owned();
    }
    message.chars().take(600).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::tools;
    use std::io::Read;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn config(kind: ProviderKind) -> ProviderConfig {
        let mut config = ProviderConfig::new(kind);
        config.model = "test-model".to_owned();
        config.api_key = "sk-test-secret".to_owned();
        config.max_tokens = 512;
        config
    }

    fn provider(kind: ProviderKind) -> Provider {
        Provider::with_transport(config(kind), Box::new(NoopTransport))
    }

    struct NoopTransport;

    impl Transport for NoopTransport {
        fn post(&self, _request: &HttpRequest) -> Result<HttpResponse, ProviderError> {
            Err(ProviderError::Transport("noop".to_owned()))
        }
    }

    fn sample_messages() -> Vec<ChatMessage> {
        vec![
            ChatMessage::system("你是编辑器助手"),
            ChatMessage::user("删掉 Root 的子节点"),
            ChatMessage::assistant(
                "先看一下场景树",
                vec![ModelToolCall {
                    id: "call_1".to_owned(),
                    name: "get_scene_tree".to_owned(),
                    arguments: json!({}),
                }],
            ),
            ChatMessage::tool("call_1", "get_scene_tree", "{\"entities\":[]}"),
        ]
    }

    #[test]
    fn openai_request_uses_registry_and_keeps_key_out_of_body() {
        let request = provider(ProviderKind::OpenAi)
            .build_request(&sample_messages(), tools())
            .expect("build request");
        assert_eq!(request.url, "https://api.openai.com/v1/chat/completions");
        let body: Value = serde_json::from_str(&request.body).expect("json body");
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["max_tokens"], 512);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][2]["tool_calls"][0]["type"], "function");
        assert_eq!(
            body["messages"][2]["tool_calls"][0]["function"]["arguments"],
            "{}"
        );
        assert_eq!(body["messages"][3]["tool_call_id"], "call_1");
        let wire_tools = body["tools"].as_array().expect("tools");
        assert_eq!(wire_tools.len(), tools().len());
        assert_eq!(wire_tools[0]["type"], "function");
        assert_eq!(wire_tools[0]["function"]["name"], "get_scene_tree");
        // 密钥只在 Authorization 头里，不进请求体/上下文。
        assert!(!request.body.contains("sk-test-secret"));
        assert!(
            request.headers.iter().any(|(name, value)| {
                name == "Authorization" && value == "Bearer sk-test-secret"
            })
        );
        let debug = format!("{request:?}");
        assert!(!debug.contains("sk-test-secret"), "{debug}");
    }

    #[test]
    fn anthropic_request_extracts_system_and_tool_results() {
        let request = provider(ProviderKind::Anthropic)
            .build_request(&sample_messages(), tools())
            .expect("build request");
        assert_eq!(request.url, "https://api.anthropic.com/v1/messages");
        let body: Value = serde_json::from_str(&request.body).expect("json body");
        assert_eq!(body["system"], "你是编辑器助手");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][1]["content"][0]["type"], "text");
        assert_eq!(body["messages"][1]["content"][1]["type"], "tool_use");
        assert_eq!(body["messages"][1]["content"][1]["name"], "get_scene_tree");
        assert_eq!(body["messages"][2]["role"], "user");
        assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(body["messages"][2]["content"][0]["tool_use_id"], "call_1");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
        assert!(!request.body.contains("sk-test-secret"));
        assert!(
            request
                .headers
                .iter()
                .any(|(name, value)| { name == "x-api-key" && value == "sk-test-secret" })
        );
        assert!(request.headers.iter().any(|(name, value)| {
            name == "anthropic-version" && value == DEFAULT_ANTHROPIC_VERSION
        }));
    }

    #[test]
    fn ollama_request_reuses_openai_wire_format_without_key() {
        let mut config = config(ProviderKind::Ollama);
        config.api_key.clear();
        config.base_url.clear();
        let request = Provider::with_transport(config, Box::new(NoopTransport))
            .build_request(&[ChatMessage::user("hi")], &[])
            .expect("build request");
        assert_eq!(request.url, "http://127.0.0.1:11434/v1/chat/completions");
        let body: Value = serde_json::from_str(&request.body).expect("json body");
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn openai_response_parses_text_and_tool_calls() {
        let response = HttpResponse {
            status: 200,
            body: json!({
                "choices": [{
                    "message": {
                        "content": "好的",
                        "tool_calls": [{
                            "id": "call_9",
                            "type": "function",
                            "function": {
                                "name": "create_entity",
                                "arguments": "{\"name\":\"Enemy\"}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            })
            .to_string(),
        };
        let parsed = provider(ProviderKind::OpenAi)
            .parse_response(response)
            .expect("parse");
        assert_eq!(parsed.text, "好的");
        assert_eq!(parsed.stop_reason.as_deref(), Some("tool_calls"));
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].name, "create_entity");
        assert_eq!(parsed.tool_calls[0].arguments["name"], "Enemy");
    }

    #[test]
    fn anthropic_response_parses_text_and_tool_use() {
        let response = HttpResponse {
            status: 200,
            body: json!({
                "content": [
                    {"type": "text", "text": "我先读一下"},
                    {"type": "tool_use", "id": "toolu_1", "name": "get_scene_tree", "input": {}}
                ],
                "stop_reason": "tool_use"
            })
            .to_string(),
        };
        let parsed = provider(ProviderKind::Anthropic)
            .parse_response(response)
            .expect("parse");
        assert_eq!(parsed.text, "我先读一下");
        assert_eq!(parsed.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(parsed.tool_calls[0].id, "toolu_1");
        assert_eq!(parsed.tool_calls[0].name, "get_scene_tree");
    }

    #[test]
    fn http_errors_are_structured_and_do_not_echo_the_key() {
        let response = HttpResponse {
            status: 401,
            body: json!({"error": {"message": "Incorrect API key provided: sk-test-secret"}})
                .to_string(),
        };
        let error = provider(ProviderKind::OpenAi)
            .parse_response(response)
            .expect_err("must fail");
        match &error {
            ProviderError::Http { status, message } => {
                assert_eq!(*status, 401);
                assert!(message.contains("Incorrect API key"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn invalid_tool_arguments_are_reported() {
        let response = HttpResponse {
            status: 200,
            body: json!({
                "choices": [{
                    "message": {
                        "tool_calls": [{
                            "id": "call_1",
                            "function": {"name": "create_entity", "arguments": "{oops"}
                        }]
                    }
                }]
            })
            .to_string(),
        };
        let error = provider(ProviderKind::OpenAi)
            .parse_response(response)
            .expect_err("must fail");
        assert!(matches!(error, ProviderError::Parse(_)), "{error:?}");
    }

    #[test]
    fn config_debug_redacts_key_and_settings_round_trip_without_key() {
        let config = config(ProviderKind::Anthropic);
        let debug = format!("{config:?}");
        assert!(!debug.contains("sk-test-secret"));
        assert!(debug.contains("<redacted>"));
        let settings = config.settings();
        let encoded = serde_json::to_string(&settings).expect("serialize settings");
        assert!(!encoded.contains("sk-test-secret"));
        assert!(!encoded.contains("api_key"));
        let decoded: ProviderSettings = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, settings);
    }

    #[test]
    fn switching_kind_replaces_only_default_endpoint_and_model() {
        let mut settings = ProviderSettings::for_kind(ProviderKind::OpenAi);
        settings.switch_kind(ProviderKind::Ollama);
        assert_eq!(settings.base_url, "http://127.0.0.1:11434/v1");
        assert_eq!(settings.model, "qwen2.5-coder:7b");
        settings.base_url = "http://10.0.0.2:8000/v1".to_owned();
        settings.model = "my-model".to_owned();
        settings.switch_kind(ProviderKind::OpenAi);
        assert_eq!(settings.base_url, "http://10.0.0.2:8000/v1");
        assert_eq!(settings.model, "my-model");
    }

    #[test]
    fn endpoint_accepts_full_or_v1_or_base_urls() {
        let mut config = config(ProviderKind::OpenAi);
        config.base_url = "https://example.test/v1/".to_owned();
        assert_eq!(
            config.endpoint(),
            "https://example.test/v1/chat/completions"
        );
        config.base_url = "https://example.test".to_owned();
        assert_eq!(
            config.endpoint(),
            "https://example.test/v1/chat/completions"
        );
        config.base_url = "https://example.test/v1/chat/completions".to_owned();
        assert_eq!(
            config.endpoint(),
            "https://example.test/v1/chat/completions"
        );
        config.kind = ProviderKind::Anthropic;
        config.base_url = "https://example.test".to_owned();
        assert_eq!(config.endpoint(), "https://example.test/v1/messages");
    }

    #[test]
    fn complete_passes_request_through_transport() {
        struct Recording(Arc<Mutex<Option<String>>>);
        impl Transport for Recording {
            fn post(&self, request: &HttpRequest) -> Result<HttpResponse, ProviderError> {
                *self.0.lock().expect("lock") = Some(request.body.clone());
                Ok(HttpResponse {
                    status: 200,
                    body: json!({"choices": [{"message": {"content": "pong"}}]}).to_string(),
                })
            }
        }
        let seen = Arc::new(Mutex::new(None));
        let provider = Provider::with_transport(
            config(ProviderKind::OpenAi),
            Box::new(Recording(seen.clone())),
        );
        let response = provider
            .complete(&[ChatMessage::user("ping")], &[])
            .expect("complete");
        assert_eq!(response.text, "pong");
        let seen = seen.lock().expect("lock").clone().expect("body");
        assert!(seen.contains("ping"));
    }

    #[test]
    fn curl_transport_round_trips_against_local_server() {
        if Command::new("curl").arg("--version").output().is_err() {
            eprintln!("skip: curl 不可用");
            return;
        }
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("addr");
        let captured = Arc::new(Mutex::new(None::<String>));
        let captured_for_thread = captured.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            stream
                .set_read_timeout(Some(Duration::from_secs(20)))
                .expect("timeout");
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 4096];
            let (header_end, content_length) = loop {
                let read = stream.read(&mut chunk).expect("read");
                assert!(read > 0, "连接在请求头完成前关闭");
                buffer.extend_from_slice(&chunk[..read]);
                if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buffer[..position]).to_ascii_lowercase();
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    break (position + 4, length);
                }
            };
            let mut body = buffer[header_end..].to_vec();
            while body.len() < content_length {
                let read = stream.read(&mut chunk).expect("read body");
                if read == 0 {
                    break;
                }
                body.extend_from_slice(&chunk[..read]);
            }
            *captured_for_thread.lock().expect("lock") = Some(format!(
                "{}\r\n\r\n{}",
                String::from_utf8_lossy(&buffer[..header_end - 4]),
                String::from_utf8_lossy(&body)
            ));
            let payload = br#"{"choices":[{"message":{"content":"pong"},"finish_reason":"stop"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
            stream.write_all(payload).expect("write body");
            stream.flush().expect("flush");
        });

        let mut config = config(ProviderKind::OpenAi);
        config.base_url = format!("http://127.0.0.1:{}", address.port());
        let provider = Provider::new(config);
        let response = provider
            .complete(&[ChatMessage::user("ping")], &[])
            .expect("curl complete");
        assert_eq!(response.text, "pong");
        server.join().expect("server");
        let captured = captured.lock().expect("lock").clone().expect("captured");
        assert!(
            captured
                .to_ascii_lowercase()
                .contains("authorization: bearer sk-test-secret"),
            "{captured}"
        );
        assert!(captured.contains("\"ping\""), "{captured}");
        assert!(captured.contains("POST /v1/chat/completions"), "{captured}");
    }

    #[test]
    fn missing_curl_binary_is_a_transport_error() {
        let transport = CurlTransport {
            program: "definitely-not-a-curl-binary".to_owned(),
            max_time_secs: 5,
        };
        let error = transport
            .post(&HttpRequest {
                url: "http://127.0.0.1:1/v1/chat/completions".to_owned(),
                headers: vec![("Content-Type".to_owned(), "application/json".to_owned())],
                body: "{}".to_owned(),
            })
            .expect_err("must fail");
        assert!(matches!(error, ProviderError::Transport(_)), "{error:?}");
    }
}
