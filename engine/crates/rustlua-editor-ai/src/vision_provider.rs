//! M76-06：截图审查的远程 Vision Provider。
//!
//! 截图导入此前只有固定 provider 与录制回放；这里补一条真正可联网的实现：
//! OpenAI 兼容 / Anthropic 的聊天端点 + 内联 PNG（data URL / base64 图片块）。
//! 模型只被要求输出 `suggestions` JSON；白名单过滤仍由
//! [`crate::import_tools::suggest_screenshot_patch`] 负责，Provider 不做取舍。
//!
//! **边界**：密钥只从环境变量（或宿主注入的 `key_source`）在请求时解析，不写盘、
//! 不进请求体、`Debug` 不打印。响应里的远程图片 URL 一律不下载。

use crate::provider::{
    CurlTransport, HttpRequest, HttpResponse, ProviderConfig, ProviderError, ProviderKind,
    Transport,
};
use crate::ui_provider::{ProviderFailure, VisionProvider, VisionReport, VisionRequest};
use serde_json::{Value, json};
use std::fmt;

/// 远程 Vision 密钥的首选环境变量；未设置时回退到通用 Provider 的密钥名。
pub const VISION_KEY_ENV: &str = "RUSTLUA_AI_VISION_API_KEY";

/// 活体测试开关；未置 1 时 `#[ignore]` 用例也不会被误跑。
pub const VISION_LIVE_ENV: &str = "RUSTLUA_VISION_LIVE";

const RESPONSE_INSTRUCTION: &str = "\
只输出 JSON，不要解释。格式：\
{\"suggestions\":[{\"key\":\"现有节点 key 或空串\",\"property\":\"text|style|layout|...\",\
\"value\":任意 JSON,\"rationale\":\"依据\"}]}。";

fn env_key() -> Option<String> {
    ["RUSTLUA_AI_VISION_API_KEY", "RUSTLUA_VISION_API_KEY"]
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .or_else(|| ProviderConfig::from_env().resolve_api_key())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteVisionConfig {
    pub kind: ProviderKind,
    pub endpoint: String,
    pub model: String,
    pub max_tokens: u32,
    /// 单次请求超时（透传 curl `max-time`）。
    pub timeout_secs: u32,
    /// 总尝试次数（含第一次），至少 1。
    pub max_attempts: u32,
    pub retry_delay_ms: u64,
}

impl Default for RemoteVisionConfig {
    fn default() -> Self {
        Self {
            kind: ProviderKind::OpenAi,
            endpoint: "https://api.openai.com/v1/chat/completions".to_owned(),
            model: "gpt-4o-mini".to_owned(),
            max_tokens: 1024,
            timeout_secs: 120,
            max_attempts: 3,
            retry_delay_ms: 250,
        }
    }
}

impl RemoteVisionConfig {
    #[must_use]
    pub fn from_env() -> Self {
        let base = ProviderConfig::from_env();
        let mut config = Self {
            kind: base.kind,
            endpoint: base.endpoint(),
            model: base.model,
            max_tokens: base.max_tokens.min(2048),
            ..Self::default()
        };
        for (name, target) in [
            ("RUSTLUA_VISION_URL", &mut config.endpoint),
            ("RUSTLUA_VISION_MODEL", &mut config.model),
        ] {
            if let Ok(value) = std::env::var(name) {
                let value = value.trim();
                if !value.is_empty() {
                    *target = value.to_owned();
                }
            }
        }
        if let Some(tokens) = std::env::var("RUSTLUA_VISION_MAX_TOKENS")
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
        {
            config.max_tokens = tokens.clamp(1, 32768);
        }
        if let Some(attempts) = std::env::var("RUSTLUA_VISION_ATTEMPTS")
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
        {
            config.max_attempts = attempts.clamp(1, 5);
        }
        if let Some(secs) = std::env::var("RUSTLUA_VISION_TIMEOUT")
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
        {
            config.timeout_secs = secs.clamp(1, 600);
        }
        config
    }
}

type KeySource = Box<dyn Fn() -> Option<String> + Send + Sync>;

pub struct RemoteVisionProvider {
    config: RemoteVisionConfig,
    transport: Box<dyn Transport>,
    key_source: KeySource,
}

// 手写 Debug：只报密钥是否已配置，不把值带进日志/快照。
impl fmt::Debug for RemoteVisionProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteVisionProvider")
            .field("config", &self.config)
            .field("key_source", &"<resolved at call time>")
            .finish()
    }
}

impl RemoteVisionProvider {
    #[must_use]
    pub fn new(config: RemoteVisionConfig) -> Self {
        let max_time_secs = config.timeout_secs;
        Self {
            config,
            transport: Box::new(CurlTransport {
                max_time_secs,
                ..CurlTransport::default()
            }),
            key_source: Box::new(env_key),
        }
    }

    /// 宿主接入系统密钥库时替换密钥来源；本 crate 不内置密钥存储。
    #[must_use]
    pub fn with_key_source(
        config: RemoteVisionConfig,
        source: impl Fn() -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        let mut provider = Self::new(config);
        provider.key_source = Box::new(source);
        provider
    }

    #[must_use]
    pub fn with_transport(config: RemoteVisionConfig, transport: Box<dyn Transport>) -> Self {
        Self {
            config,
            transport,
            key_source: Box::new(env_key),
        }
    }

    #[must_use]
    pub fn from_env() -> Self {
        Self::new(RemoteVisionConfig::from_env())
    }

    #[must_use]
    pub fn config(&self) -> &RemoteVisionConfig {
        &self.config
    }

    fn build_request(
        &self,
        request: &VisionRequest,
        key: &str,
    ) -> Result<HttpRequest, ProviderFailure> {
        if self.config.endpoint.chars().any(char::is_control) {
            return Err(ProviderFailure::new(
                "vision.endpoint_invalid",
                "Vision 端点含控制字符",
            ));
        }
        if request.question.trim().is_empty() {
            return Err(ProviderFailure::new(
                "vision.question_empty",
                "截图审查的问题不能为空",
            ));
        }
        if !request.image_png.starts_with(&[0x89, b'P', b'N', b'G']) {
            return Err(ProviderFailure::new(
                "vision.image_not_png",
                "截图审查只接受 PNG",
            ));
        }
        if key.chars().any(char::is_control) {
            return Err(ProviderFailure::new(
                "vision.key_invalid",
                "Vision 密钥含控制字符",
            ));
        }
        let encoded = encode_base64(&request.image_png);
        let question = format!("{}\n\n{RESPONSE_INSTRUCTION}", request.question.trim());
        let (body, headers) = match self.config.kind {
            ProviderKind::OpenAi | ProviderKind::Ollama => {
                let body = json!({
                    "model": self.config.model,
                    "max_tokens": self.config.max_tokens,
                    "messages": [{
                        "role": "user",
                        "content": [
                            {"type": "text", "text": question},
                            {
                                "type": "image_url",
                                "image_url": {"url": format!("data:image/png;base64,{encoded}")},
                            },
                        ],
                    }],
                });
                let mut headers = vec![("Content-Type".to_owned(), "application/json".to_owned())];
                if !key.is_empty() {
                    headers.push(("Authorization".to_owned(), format!("Bearer {key}")));
                }
                (body, headers)
            }
            ProviderKind::Anthropic => {
                let body = json!({
                    "model": self.config.model,
                    "max_tokens": self.config.max_tokens,
                    "messages": [{
                        "role": "user",
                        "content": [
                            {
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": "image/png",
                                    "data": encoded,
                                },
                            },
                            {"type": "text", "text": question},
                        ],
                    }],
                });
                let headers = vec![
                    ("Content-Type".to_owned(), "application/json".to_owned()),
                    ("x-api-key".to_owned(), key.to_owned()),
                    (
                        "anthropic-version".to_owned(),
                        crate::provider::DEFAULT_ANTHROPIC_VERSION.to_owned(),
                    ),
                ];
                (body, headers)
            }
        };
        let body = serde_json::to_string(&body).map_err(|error| {
            ProviderFailure::new("vision.encode", format!("请求序列化失败：{error}"))
        })?;
        Ok(HttpRequest {
            url: self.config.endpoint.clone(),
            headers,
            body,
        })
    }

    fn parse_response(&self, response: HttpResponse) -> Result<VisionReport, ProviderFailure> {
        if !(200..300).contains(&response.status) {
            let message: String = response.body.trim().chars().take(600).collect();
            return Err(ProviderFailure::new(
                format!("vision.http_{}", response.status),
                if message.is_empty() {
                    "（空响应体）".to_owned()
                } else {
                    message
                },
            ));
        }
        let value: Value = serde_json::from_str(&response.body).map_err(|error| {
            ProviderFailure::new("vision.decode", format!("响应不是合法 JSON：{error}"))
        })?;
        let text = match self.config.kind {
            ProviderKind::OpenAi | ProviderKind::Ollama => value
                .pointer("/choices/0/message/content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            ProviderKind::Anthropic => value
                .get("content")
                .and_then(Value::as_array)
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter_map(|block| block.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default(),
        };
        parse_suggestions(&text)
    }
}

fn parse_suggestions(text: &str) -> Result<VisionReport, ProviderFailure> {
    let trimmed = text.trim();
    let unfenced = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|rest| rest.strip_suffix("```"))
        .unwrap_or(trimmed)
        .trim();
    let candidate = match (unfenced.find('{'), unfenced.rfind('}')) {
        (Some(start), Some(end)) if start <= end => &unfenced[start..=end],
        _ => unfenced,
    };
    let value: Value = serde_json::from_str(candidate).map_err(|error| {
        ProviderFailure::new(
            "vision.decode",
            format!("模型输出里找不到 suggestions JSON：{error}"),
        )
    })?;
    let suggestions = value
        .get("suggestions")
        .cloned()
        .or_else(|| value.as_array().map(|_| value.clone()))
        .ok_or_else(|| ProviderFailure::new("vision.decode", "模型输出缺少 suggestions 数组"))?;
    let suggestions = serde_json::from_value(suggestions).map_err(|error| {
        ProviderFailure::new("vision.decode", format!("suggestions 字段不合法：{error}"))
    })?;
    Ok(VisionReport {
        suggestions,
        diagnostics: Vec::new(),
    })
}

fn retryable_status(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

impl VisionProvider for RemoteVisionProvider {
    fn name(&self) -> &str {
        "remote-vision"
    }

    fn inspect(&self, request: &VisionRequest) -> Result<VisionReport, ProviderFailure> {
        let key = (self.key_source)().ok_or_else(|| {
            ProviderFailure::new(
                "vision.key_missing",
                format!(
                    "未配置 Vision 密钥：设置环境变量 {VISION_KEY_ENV}（或服务商自己的密钥变量）"
                ),
            )
        })?;
        let http = self.build_request(request, &key)?;
        let attempts = self.config.max_attempts.max(1);
        let mut last: Option<ProviderFailure> = None;
        for attempt in 0..attempts {
            if attempt > 0 && self.config.retry_delay_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(self.config.retry_delay_ms));
            }
            match self.transport.post(&http) {
                Ok(response) if (200..300).contains(&response.status) => {
                    return self.parse_response(response);
                }
                Ok(response) => {
                    let status = response.status;
                    let failure = self
                        .parse_response(response)
                        .expect_err("非 2xx 必须是错误");
                    if !retryable_status(status) {
                        return Err(failure);
                    }
                    last = Some(failure);
                }
                Err(error) => {
                    last = Some(
                        ProviderFailure::new("vision.transport", provider_error_message(&error))
                            .retryable(true),
                    );
                }
            }
        }
        Err(last
            .unwrap_or_else(|| ProviderFailure::new("vision.retry_exhausted", "重试次数已用尽")))
    }
}

fn provider_error_message(error: &ProviderError) -> String {
    error.to_string()
}

fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let combined = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(combined >> 18) as usize & 63] as char);
        out.push(TABLE[(combined >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(combined >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[combined as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct ScriptedTransport {
        responses: Mutex<Vec<Result<HttpResponse, ProviderError>>>,
        requests: Arc<Mutex<Vec<HttpRequest>>>,
        calls: Arc<AtomicUsize>,
    }

    impl Transport for ScriptedTransport {
        fn post(&self, request: &HttpRequest) -> Result<HttpResponse, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.requests.lock().unwrap().push(HttpRequest {
                url: request.url.clone(),
                headers: request.headers.clone(),
                body: request.body.clone(),
            });
            self.responses.lock().unwrap().remove(0)
        }
    }

    type Scripted = (
        RemoteVisionProvider,
        Arc<Mutex<Vec<HttpRequest>>>,
        Arc<AtomicUsize>,
    );

    fn scripted(responses: Vec<Result<HttpResponse, ProviderError>>) -> Scripted {
        scripted_with(RemoteVisionConfig::default(), responses)
    }

    fn scripted_with(
        config: RemoteVisionConfig,
        responses: Vec<Result<HttpResponse, ProviderError>>,
    ) -> Scripted {
        scripted_with_key(config, Some("sk-secret"), responses)
    }

    fn scripted_with_key(
        config: RemoteVisionConfig,
        key: Option<&'static str>,
        responses: Vec<Result<HttpResponse, ProviderError>>,
    ) -> Scripted {
        let transport = ScriptedTransport {
            responses: Mutex::new(responses),
            ..ScriptedTransport::default()
        };
        let requests = transport.requests.clone();
        let calls = transport.calls.clone();
        let config = RemoteVisionConfig {
            max_attempts: 3,
            retry_delay_ms: 0,
            ..config
        };
        (
            RemoteVisionProvider::with_key_source(config, move || key.map(str::to_owned))
                .with_test_transport(Box::new(transport)),
            requests,
            calls,
        )
    }

    impl RemoteVisionProvider {
        fn with_test_transport(mut self, transport: Box<dyn Transport>) -> Self {
            self.transport = transport;
            self
        }
    }

    fn png() -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        bytes.extend_from_slice(&[0u8; 8]);
        bytes
    }

    fn request() -> VisionRequest {
        VisionRequest {
            image_png: png(),
            question: "把标题对齐到参考图".to_owned(),
            spec: None,
        }
    }

    fn answer(text: &str) -> Result<HttpResponse, ProviderError> {
        Ok(HttpResponse {
            status: 200,
            body: json!({"choices": [{"message": {"content": text}}]}).to_string(),
        })
    }

    #[test]
    fn openai_request_embeds_the_png_as_a_data_url_without_leaking_the_key() {
        let (provider, requests, _) = scripted(vec![answer(
            r#"{"suggestions":[{"key":"title","property":"text","value":"Boutique","rationale":"更大"}]}"#,
        )]);
        let report = provider.inspect(&request()).expect("vision");
        assert_eq!(report.suggestions.len(), 1);
        assert_eq!(report.suggestions[0].property, "text");

        let sent = requests.lock().unwrap();
        let body = &sent[0].body;
        assert!(body.contains("data:image/png;base64,"), "{body}");
        assert!(!body.contains("sk-secret"), "密钥不能进请求体");
        assert!(
            sent[0]
                .headers
                .iter()
                .any(|(name, value)| name == "Authorization" && value == "Bearer sk-secret")
        );
    }

    #[test]
    fn anthropic_uses_an_image_block_and_accepts_fenced_json() {
        let config = RemoteVisionConfig {
            kind: ProviderKind::Anthropic,
            endpoint: "https://api.anthropic.com/v1/messages".to_owned(),
            ..RemoteVisionConfig::default()
        };
        let (provider, requests, _) = scripted_with(
            config,
            vec![Ok(HttpResponse {
                status: 200,
                body: json!({
                    "content": [
                        {"type": "text", "text": "```json\n{\"suggestions\":[{\"key\":\"\",\"property\":\"layout\",\"value\":{\"gap\":4},\"rationale\":\"参考图间距\"}]}\n```"}
                    ]
                })
                .to_string(),
            })],
        );
        let report = provider.inspect(&request()).expect("vision");
        assert_eq!(report.suggestions[0].property, "layout");
        let sent = requests.lock().unwrap();
        assert!(
            sent[0].body.contains("\"type\":\"image\""),
            "{}",
            sent[0].body
        );
        assert!(sent[0].body.contains("\"media_type\":\"image/png\""));
        assert!(
            sent[0]
                .headers
                .iter()
                .any(|(name, value)| name == "x-api-key" && value == "sk-secret")
        );
    }

    #[test]
    fn missing_key_and_wrong_image_are_rejected_before_any_request() {
        let (provider, _, calls) = scripted_with_key(RemoteVisionConfig::default(), None, vec![]);
        let error = provider.inspect(&request()).expect_err("缺 key");
        assert_eq!(error.code, "vision.key_missing");
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let (provider, _, _) = scripted(vec![]);
        let bad = VisionRequest {
            image_png: b"not a png".to_vec(),
            ..request()
        };
        let error = provider.inspect(&bad).expect_err("非 PNG");
        assert_eq!(error.code, "vision.image_not_png");
    }

    #[test]
    fn retries_5xx_but_not_4xx_and_reports_unparseable_answers() {
        let (provider, _, calls) = scripted(vec![
            Ok(HttpResponse {
                status: 503,
                body: "busy".to_owned(),
            }),
            answer(r#"{"suggestions":[]}"#),
        ]);
        assert!(provider.inspect(&request()).is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        let (provider, _, calls) = scripted(vec![Ok(HttpResponse {
            status: 400,
            body: "bad request".to_owned(),
        })]);
        let error = provider.inspect(&request()).expect_err("4xx 不重试");
        assert_eq!(error.code, "vision.http_400");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let (provider, _, _) = scripted(vec![answer("模型没按要求输出")]);
        let error = provider.inspect(&request()).expect_err("不可解析");
        assert_eq!(error.code, "vision.decode");
    }

    #[test]
    #[ignore = "live: 需要 RUSTLUA_VISION_LIVE=1、密钥与 RUSTLUA_VISION_IMAGE 才会真正联网"]
    fn live_vision_returns_suggestions_for_a_real_screenshot() {
        assert_eq!(
            std::env::var(VISION_LIVE_ENV).as_deref(),
            Ok("1"),
            "活体测试必须显式设 {VISION_LIVE_ENV}=1，避免 CI 误联网"
        );
        let path = std::env::var("RUSTLUA_VISION_IMAGE")
            .expect("活体测试需要 RUSTLUA_VISION_IMAGE 指向一张 PNG 截图");
        let image = std::fs::read(&path).expect("读截图");
        let provider = RemoteVisionProvider::from_env();
        assert!(
            provider.config().endpoint.contains("://"),
            "端点必须是完整 URL：{}",
            provider.config().endpoint
        );
        let request = VisionRequest {
            image_png: image,
            question: std::env::var("RUSTLUA_VISION_QUESTION")
                .unwrap_or_else(|_| "描述这张 UI 截图的布局与配色".to_owned()),
            spec: None,
        };
        let report = provider.inspect(&request).expect("活体 Vision 调用");
        assert!(!report.suggestions.is_empty(), "活体调用必须返回建议");
        eprintln!(
            "live vision ok: endpoint={} model={} suggestions={}",
            provider.config().endpoint,
            provider.config().model,
            report.suggestions.len()
        );
    }
}
