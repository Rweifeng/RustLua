//! M76-05: 出图 Provider 的三种实现（本地 / 远程 / 录制）。
//!
//! - [`LocalImageProvider`]：确定性程序化占位图，不联网、不需要密钥。测试与 CI 默认走它。
//! - [`RemoteImageProvider`]：OpenAI 兼容的 `/images/generations`，密钥**只**从环境变量
//!   （或宿主注入的 `key_source`，例如系统密钥库）读取；本结构体不存密钥、`Debug` 不打印密钥。
//!   超时由 curl `max-time` 约束，重试只覆盖可重试错误（5xx/429/传输失败）且有次数上限。
//! - 录制：沿用 [`crate::ui_provider::RecordingImage`] / [`crate::ui_provider::ReplayImage`]。
//!
//! 远程响应只接受内联 `b64_json`；只给 URL 的结果直接拒绝——否则出图链路就会多出一个
//! 「按模型给的 URL 去下载」的请求面（SSRF/代理泄漏），那不属于本工作包。

use crate::provider::{CurlTransport, HttpRequest, HttpResponse, ProviderError, Transport};
use crate::ui_provider::{ImageOutput, ImageProvider, ImageRequest, ProviderFailure};
use rustlua_ui_spec::{ImageGeneration, encode_rgba_png};
use serde_json::{Value, json};
use std::fmt;

/// 远程出图密钥的环境变量。只读，不写盘、不进日志、不进请求体。
pub const IMAGE_KEY_ENV: &str = "RUSTLUA_AI_IMAGE_API_KEY";

fn env_key() -> Option<String> {
    std::env::var(IMAGE_KEY_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

/// 本地确定性出图：同一 prompt/尺寸/种子永远得到同一份字节。
///
/// 画的是带描边的棋盘格（颜色由提示词哈希决定），足以让审阅界面立刻有真实 PNG 可看，
/// 也足以让「同输入同字节」与体积门在 CI 里可测；它不冒充美术产出。
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalImageProvider;

impl LocalImageProvider {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

fn local_rgba(request: &ImageRequest, width: u32, height: u32) -> Vec<u8> {
    let mut key = request.prompt.clone().into_bytes();
    key.extend_from_slice(request.negative.as_deref().unwrap_or("").as_bytes());
    key.extend_from_slice(&request.size[0].to_le_bytes());
    key.extend_from_slice(&request.size[1].to_le_bytes());
    key.extend_from_slice(&request.seed.unwrap_or(0).to_le_bytes());
    let hash = fnv1a(&key);
    let tint = [
        48 + (hash & 0x7f) as u8,
        48 + ((hash >> 8) & 0x7f) as u8,
        48 + ((hash >> 16) & 0x7f) as u8,
    ];
    let light = tint.map(|channel| channel.saturating_add(48));
    let cell = ((hash >> 24) & 0x07).max(2) as u32;
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let border = x == 0 || y == 0 || x == width - 1 || y == height - 1;
            let checker = ((x / cell) + (y / cell)).is_multiple_of(2);
            let color = if border {
                [24, 24, 28, 255]
            } else if checker {
                [tint[0], tint[1], tint[2], 255]
            } else {
                [light[0], light[1], light[2], 255]
            };
            rgba.extend_from_slice(&color);
        }
    }
    rgba
}

impl ImageProvider for LocalImageProvider {
    fn name(&self) -> &str {
        "local"
    }

    fn generate(&self, request: &ImageRequest) -> Result<ImageOutput, ProviderFailure> {
        let width = request.size[0].clamp(1, 4096);
        let height = request.size[1].clamp(1, 4096);
        if request.prompt.trim().is_empty() {
            return Err(ProviderFailure::new(
                "image.prompt_empty",
                "出图提示词不能为空",
            ));
        }
        let rgba = local_rgba(request, width, height);
        Ok(ImageOutput {
            png: encode_rgba_png(width, height, &rgba),
            generation: ImageGeneration {
                prompt: request.prompt.clone(),
                negative: request.negative.clone(),
                size: Some([width, height]),
                variants: Some(1),
                seed: request.seed,
            },
        })
    }
}

/// 远程出图的配置。密钥不在这里——见 [`RemoteImageProvider::key_source`]。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteImageConfig {
    pub endpoint: String,
    pub model: String,
    /// 单次请求的超时（透传给 curl `max-time`）。
    pub timeout_secs: u32,
    /// 总尝试次数（含第一次），至少 1。
    pub max_attempts: u32,
    /// 两次尝试之间的等待；测试里设 0 避免拖慢。
    pub retry_delay_ms: u64,
}

impl Default for RemoteImageConfig {
    fn default() -> Self {
        Self {
            endpoint: "https://api.openai.com/v1/images/generations".to_owned(),
            model: "gpt-image-1".to_owned(),
            timeout_secs: 120,
            max_attempts: 3,
            retry_delay_ms: 250,
        }
    }
}

impl RemoteImageConfig {
    /// 非密钥配置可来自环境变量；密钥**不**在这里解析。
    #[must_use]
    pub fn from_env() -> Self {
        let mut config = Self::default();
        for (name, target) in [
            ("RUSTLUA_AI_IMAGE_URL", &mut config.endpoint),
            ("RUSTLUA_AI_IMAGE_MODEL", &mut config.model),
        ] {
            if let Ok(value) = std::env::var(name) {
                let value = value.trim();
                if !value.is_empty() {
                    *target = value.to_owned();
                }
            }
        }
        if let Some(secs) = std::env::var("RUSTLUA_AI_IMAGE_TIMEOUT")
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
        {
            config.timeout_secs = secs.clamp(1, 600);
        }
        if let Some(attempts) = std::env::var("RUSTLUA_AI_IMAGE_ATTEMPTS")
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
        {
            config.max_attempts = attempts.clamp(1, 5);
        }
        config
    }
}

type KeySource = Box<dyn Fn() -> Option<String> + Send + Sync>;

pub struct RemoteImageProvider {
    config: RemoteImageConfig,
    transport: Box<dyn Transport>,
    key_source: KeySource,
}

// 手写 Debug：密钥来源只能报「是否已配置」，绝不能把值带进日志或快照。
impl fmt::Debug for RemoteImageProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteImageProvider")
            .field("config", &self.config)
            .field("key_source", &"<resolved at call time>")
            .finish()
    }
}

impl RemoteImageProvider {
    /// 默认走系统 curl；密钥只从 [`IMAGE_KEY_ENV`] 读。
    #[must_use]
    pub fn new(config: RemoteImageConfig) -> Self {
        let max_time = config.timeout_secs;
        Self {
            config,
            transport: Box::new(CurlTransport {
                max_time_secs: max_time,
                ..CurlTransport::default()
            }),
            key_source: Box::new(env_key),
        }
    }

    /// 宿主接入系统密钥库时替换密钥来源；本 crate 不内置任何密钥存储。
    #[must_use]
    pub fn with_key_source(
        config: RemoteImageConfig,
        source: impl Fn() -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        let mut provider = Self::new(config);
        provider.key_source = Box::new(source);
        provider
    }

    #[must_use]
    pub fn with_transport(config: RemoteImageConfig, transport: Box<dyn Transport>) -> Self {
        Self {
            config,
            transport,
            key_source: Box::new(env_key),
        }
    }

    #[must_use]
    pub fn config(&self) -> &RemoteImageConfig {
        &self.config
    }

    fn build_request(
        &self,
        request: &ImageRequest,
        key: &str,
    ) -> Result<HttpRequest, ProviderFailure> {
        if self.config.endpoint.chars().any(char::is_control) {
            return Err(ProviderFailure::new(
                "image.endpoint_invalid",
                "出图端点含控制字符",
            ));
        }
        if request.prompt.trim().is_empty() {
            return Err(ProviderFailure::new(
                "image.prompt_empty",
                "出图提示词不能为空",
            ));
        }
        if key.chars().any(char::is_control) {
            return Err(ProviderFailure::new(
                "image.key_invalid",
                "出图密钥含控制字符",
            ));
        }
        let mut prompt = request.prompt.clone();
        if let Some(negative) = request
            .negative
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            prompt.push_str("\nAvoid: ");
            prompt.push_str(negative);
        }
        let body = json!({
            "model": self.config.model,
            "prompt": prompt,
            "size": format!("{}x{}", request.size[0], request.size[1]),
            "n": 1,
            "response_format": "b64_json",
            "seed": request.seed,
        });
        let body = serde_json::to_string(&body)
            .map_err(|error| ProviderFailure::new("image.encode", error.to_string()))?;
        Ok(HttpRequest {
            url: self.config.endpoint.clone(),
            headers: vec![
                ("Content-Type".to_owned(), "application/json".to_owned()),
                ("Authorization".to_owned(), format!("Bearer {key}")),
            ],
            body,
        })
    }

    fn parse_response(
        &self,
        request: &ImageRequest,
        response: HttpResponse,
    ) -> Result<ImageOutput, ProviderFailure> {
        if !(200..300).contains(&response.status) {
            let message: String = response.body.trim().chars().take(600).collect();
            return Err(ProviderFailure::new(
                format!("image.http_{}", response.status),
                if message.is_empty() {
                    "（空响应体）".to_owned()
                } else {
                    message
                },
            ));
        }
        let value: Value = serde_json::from_str(&response.body).map_err(|error| {
            ProviderFailure::new("image.decode", format!("响应不是合法 JSON：{error}"))
        })?;
        let data = value
            .get("data")
            .and_then(Value::as_array)
            .and_then(|data| data.first())
            .ok_or_else(|| ProviderFailure::new("image.decode", "响应缺少 data[0]"))?;
        let encoded = match data.get("b64_json").and_then(Value::as_str) {
            Some(encoded) if !encoded.trim().is_empty() => encoded,
            _ => {
                return Err(ProviderFailure::new(
                    "image.remote_url_unsupported",
                    "远程出图只接受内联 b64_json（不代下载模型给的 URL）",
                ));
            }
        };
        let png =
            decode_base64(encoded).map_err(|error| ProviderFailure::new("image.base64", error))?;
        if !png.starts_with(&[0x89, b'P', b'N', b'G']) {
            return Err(ProviderFailure::new("image.not_png", "远程结果不是 PNG"));
        }
        Ok(ImageOutput {
            png,
            generation: ImageGeneration {
                prompt: request.prompt.clone(),
                negative: request.negative.clone(),
                size: Some(request.size),
                variants: Some(1),
                seed: request.seed,
            },
        })
    }
}

fn retryable_status(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

impl ImageProvider for RemoteImageProvider {
    fn name(&self) -> &str {
        "remote"
    }

    fn generate(&self, request: &ImageRequest) -> Result<ImageOutput, ProviderFailure> {
        let key = (self.key_source)().ok_or_else(|| {
            ProviderFailure::new(
                "image.key_missing",
                format!("未配置出图密钥：设置环境变量 {IMAGE_KEY_ENV}"),
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
                    return self.parse_response(request, response);
                }
                Ok(response) => {
                    let status = response.status;
                    let failure = self
                        .parse_response(request, response)
                        .expect_err("非 2xx 必须是错误");
                    if !retryable_status(status) {
                        return Err(failure);
                    }
                    last = Some(failure);
                }
                Err(error) => {
                    last = Some(
                        ProviderFailure::new("image.transport", provider_error_message(&error))
                            .retryable(true),
                    );
                }
            }
        }
        Err(last.unwrap_or_else(|| ProviderFailure::new("image.retry_exhausted", "重试次数已用尽")))
    }
}

fn provider_error_message(error: &ProviderError) -> String {
    error.to_string()
}

fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut chunk = [0u8; 4];
    let mut filled = 0;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => 64,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            other => return Err(format!("非法 base64 字符：{other}")),
        };
        chunk[filled] = value;
        filled += 1;
        if filled == 4 {
            out.push((chunk[0] << 2) | (chunk[1] >> 4));
            if chunk[2] != 64 {
                out.push((chunk[1] << 4) | (chunk[2] >> 2));
                if chunk[3] != 64 {
                    out.push((chunk[2] << 6) | chunk[3]);
                }
            }
            filled = 0;
        }
    }
    if filled != 0 {
        return Err("base64 长度不是 4 的倍数".to_owned());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn request(seed: u64) -> ImageRequest {
        ImageRequest {
            prompt: "wooden shop button".to_owned(),
            size: [32, 16],
            negative: None,
            seed: Some(seed),
            style_tokens: BTreeMap::new(),
        }
    }

    fn png_bytes() -> Vec<u8> {
        let rgba = vec![255u8; 4 * 4 * 4];
        encode_rgba_png(4, 4, &rgba)
    }

    fn base64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
            let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
            let triple = (b0 << 16) | (b1 << 8) | b2;
            out.push(ALPHABET[(triple >> 18) as usize & 63] as char);
            out.push(ALPHABET[(triple >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 {
                ALPHABET[(triple >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                ALPHABET[triple as usize & 63] as char
            } else {
                '='
            });
        }
        out
    }

    #[test]
    fn local_provider_is_deterministic_and_names_its_seed() {
        let provider = LocalImageProvider::new();
        let first = provider.generate(&request(7)).unwrap();
        let again = provider.generate(&request(7)).unwrap();
        let other = provider.generate(&request(8)).unwrap();
        assert_eq!(first, again);
        assert_ne!(first.png, other.png);
        assert!(first.png.starts_with(&[0x89, b'P', b'N', b'G']));
        assert_eq!(first.generation.size, Some([32, 16]));
        assert_eq!(first.generation.seed, Some(7));
    }

    #[test]
    fn remote_provider_refuses_without_a_key() {
        let provider = RemoteImageProvider::with_key_source(RemoteImageConfig::default(), || None);
        let failure = provider.generate(&request(1)).unwrap_err();
        assert_eq!(failure.code, "image.key_missing");
        assert!(!failure.retryable);
    }

    struct Scripted {
        responses: Arc<std::sync::Mutex<Vec<Result<HttpResponse, ProviderError>>>>,
        calls: Arc<AtomicUsize>,
    }

    impl Transport for Scripted {
        fn post(&self, _request: &HttpRequest) -> Result<HttpResponse, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                return Err(ProviderError::Transport("脚本已用尽".to_owned()));
            }
            responses.remove(0)
        }
    }

    fn remote(
        responses: Vec<Result<HttpResponse, ProviderError>>,
    ) -> (RemoteImageProvider, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let transport = Scripted {
            responses: Arc::new(std::sync::Mutex::new(responses)),
            calls: calls.clone(),
        };
        let mut config = RemoteImageConfig {
            retry_delay_ms: 0,
            max_attempts: 3,
            ..RemoteImageConfig::default()
        };
        config.endpoint = "https://example.test/v1/images/generations".to_owned();
        let provider = RemoteImageProvider::with_transport(config, Box::new(transport));
        let provider = RemoteImageProvider {
            key_source: Box::new(|| Some("sk-test".to_owned())),
            ..provider
        };
        (provider, calls)
    }

    fn ok_body() -> String {
        json!({"data": [{"b64_json": base64(&png_bytes())}]}).to_string()
    }

    #[test]
    fn remote_retries_transient_failures_then_succeeds() {
        let (provider, calls) = remote(vec![
            Ok(HttpResponse {
                status: 500,
                body: "upstream".to_owned(),
            }),
            Ok(HttpResponse {
                status: 200,
                body: ok_body(),
            }),
        ]);
        let output = provider.generate(&request(3)).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(output.png.starts_with(&[0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn remote_does_not_retry_client_errors() {
        let (provider, calls) = remote(vec![Ok(HttpResponse {
            status: 400,
            body: json!({"error": {"message": "bad prompt"}}).to_string(),
        })]);
        let failure = provider.generate(&request(3)).unwrap_err();
        assert_eq!(failure.code, "image.http_400");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn remote_rejects_url_only_results() {
        let (provider, _) = remote(vec![Ok(HttpResponse {
            status: 200,
            body: json!({"data": [{"url": "https://example.test/a.png"}]}).to_string(),
        })]);
        let failure = provider.generate(&request(3)).unwrap_err();
        assert_eq!(failure.code, "image.remote_url_unsupported");
    }

    #[test]
    fn base64_round_trips_png_magic() {
        assert_eq!(
            decode_base64("iVBORw0KGgo=").unwrap(),
            vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]
        );
        assert!(decode_base64("abc").is_err());
        assert!(decode_base64("!!!!").is_err());
    }

    #[test]
    fn debug_never_exposes_the_key() {
        let (provider, _) = remote(Vec::new());
        let debug = format!("{provider:?}");
        assert!(!debug.contains("sk-test"), "{debug}");
    }
}
