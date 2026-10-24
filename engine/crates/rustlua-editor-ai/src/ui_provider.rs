//! UI 制作链路的 Provider 抽象（方案 v2 M0）。
//!
//! Provider 只产出结构化内容（`UiSpec` / `UiPatch` / `UiAssetPlan` / 图像字节），
//! 不持有编辑器状态，也不直接落盘。所有调用都能被 [`ProviderTrace`] 录制，
//! 再由 `Replay*` provider 离线重放，CI 不依赖网络与模型。

use rustlua_ui_spec::{Capabilities, Diagnostic, ImageGeneration, UiAssetPlan, UiPatch, UiSpec};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmRequest {
    pub intent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<UiSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_key: Option<String>,
    /// 与校验器同一份能力矩阵，模型不能自行假定运行时能力。
    pub capabilities: Capabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

/// LLM 的唯一合法输出形态。自由文本不能直接进入编辑器。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LlmOutput {
    Spec { spec: Box<UiSpec> },
    Patch { patch: Box<UiPatch> },
    AssetPlan { assets: Vec<UiAssetPlan> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VisionRequest {
    pub image_png: Vec<u8>,
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<UiSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VisionSuggestion {
    /// 目标节点 key；为空表示建议新增节点。
    pub key: String,
    pub property: String,
    pub value: Value,
    pub rationale: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VisionReport {
    #[serde(default)]
    pub suggestions: Vec<VisionSuggestion>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageRequest {
    pub prompt: String,
    pub size: [u32; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub negative: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub style_tokens: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageOutput {
    pub png: Vec<u8>,
    pub generation: ImageGeneration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderFailure {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
}

impl ProviderFailure {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
        }
    }

    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }
}

impl fmt::Display for ProviderFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ProviderFailure {}

pub trait LlmProvider {
    fn name(&self) -> &str;
    fn propose(&self, request: &LlmRequest) -> Result<LlmOutput, ProviderFailure>;
}

pub trait VisionProvider {
    fn name(&self) -> &str;
    fn inspect(&self, request: &VisionRequest) -> Result<VisionReport, ProviderFailure>;
}

pub trait ImageProvider {
    fn name(&self) -> &str;
    fn generate(&self, request: &ImageRequest) -> Result<ImageOutput, ProviderFailure>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Llm,
    Vision,
    Image,
}

/// 一次调用的录制单元。`request_json` / `response_json` 是规范化的 JSON 文本；
/// 二进制结果（PNG）放在 `payload`，避免在 JSON 里塞字节数组。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderExchange {
    pub kind: ProviderKind,
    pub provider: String,
    pub request_json: String,
    pub response_json: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderTrace {
    pub exchanges: Vec<ProviderExchange>,
}

impl ProviderTrace {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("ProviderTrace 序列化")
    }

    pub fn from_json(raw: &str) -> Result<Self, ProviderFailure> {
        serde_json::from_str(raw)
            .map_err(|error| ProviderFailure::new("provider.trace_invalid", error.to_string()))
    }
}

fn push_exchange(trace: &Mutex<ProviderTrace>, exchange: ProviderExchange) {
    trace
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .exchanges
        .push(exchange);
}

fn canonical<T: Serialize>(value: &T) -> Result<String, ProviderFailure> {
    serde_json::to_string(value)
        .map_err(|error| ProviderFailure::new("provider.encode", error.to_string()))
}

pub struct RecordingLlm<P> {
    inner: P,
    trace: Mutex<ProviderTrace>,
}

impl<P> RecordingLlm<P> {
    pub fn new(inner: P) -> Self {
        Self {
            inner,
            trace: Mutex::new(ProviderTrace::default()),
        }
    }

    pub fn trace(&self) -> ProviderTrace {
        self.trace
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl<P: LlmProvider> LlmProvider for RecordingLlm<P> {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn propose(&self, request: &LlmRequest) -> Result<LlmOutput, ProviderFailure> {
        let output = self.inner.propose(request)?;
        push_exchange(
            &self.trace,
            ProviderExchange {
                kind: ProviderKind::Llm,
                provider: self.inner.name().to_owned(),
                request_json: canonical(request)?,
                response_json: canonical(&output)?,
                payload: Vec::new(),
            },
        );
        Ok(output)
    }
}

pub struct RecordingVision<P> {
    inner: P,
    trace: Mutex<ProviderTrace>,
}

impl<P> RecordingVision<P> {
    pub fn new(inner: P) -> Self {
        Self {
            inner,
            trace: Mutex::new(ProviderTrace::default()),
        }
    }

    pub fn trace(&self) -> ProviderTrace {
        self.trace
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl<P: VisionProvider> VisionProvider for RecordingVision<P> {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn inspect(&self, request: &VisionRequest) -> Result<VisionReport, ProviderFailure> {
        let output = self.inner.inspect(request)?;
        push_exchange(
            &self.trace,
            ProviderExchange {
                kind: ProviderKind::Vision,
                provider: self.inner.name().to_owned(),
                request_json: canonical(request)?,
                response_json: canonical(&output)?,
                payload: Vec::new(),
            },
        );
        Ok(output)
    }
}

pub struct RecordingImage<P> {
    inner: P,
    trace: Mutex<ProviderTrace>,
}

impl<P> RecordingImage<P> {
    pub fn new(inner: P) -> Self {
        Self {
            inner,
            trace: Mutex::new(ProviderTrace::default()),
        }
    }

    pub fn trace(&self) -> ProviderTrace {
        self.trace
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl<P: ImageProvider> ImageProvider for RecordingImage<P> {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn generate(&self, request: &ImageRequest) -> Result<ImageOutput, ProviderFailure> {
        let output = self.inner.generate(request)?;
        push_exchange(
            &self.trace,
            ProviderExchange {
                kind: ProviderKind::Image,
                provider: self.inner.name().to_owned(),
                request_json: canonical(request)?,
                response_json: canonical(&output.generation)?,
                payload: output.png.clone(),
            },
        );
        Ok(output)
    }
}

fn next_exchange(
    trace: &[ProviderExchange],
    cursor: &Mutex<usize>,
    kind: ProviderKind,
) -> Result<ProviderExchange, ProviderFailure> {
    let mut index = cursor
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let exchange = trace.get(*index).ok_or_else(|| {
        ProviderFailure::new(
            "provider.replay_exhausted",
            format!("录制已用尽：第 {index} 次调用没有对应记录"),
        )
    })?;
    if exchange.kind != kind {
        return Err(ProviderFailure::new(
            "provider.replay_order",
            format!(
                "第 {index} 次调用期望 {:?}，录制里是 {:?}",
                kind, exchange.kind
            ),
        ));
    }
    *index += 1;
    Ok(exchange.clone())
}

fn check_request(exchange: &ProviderExchange, expected: String) -> Result<(), ProviderFailure> {
    if exchange.request_json != expected {
        return Err(ProviderFailure::new(
            "provider.replay_request_mismatch",
            format!(
                "重放请求与录制不一致：期望 {expected}，录制 {}",
                exchange.request_json
            ),
        ));
    }
    Ok(())
}

pub struct ReplayLlm {
    trace: Vec<ProviderExchange>,
    cursor: Mutex<usize>,
}

impl ReplayLlm {
    pub fn new(trace: ProviderTrace) -> Self {
        Self {
            trace: trace.exchanges,
            cursor: Mutex::new(0),
        }
    }
}

impl LlmProvider for ReplayLlm {
    fn name(&self) -> &str {
        "replay"
    }

    fn propose(&self, request: &LlmRequest) -> Result<LlmOutput, ProviderFailure> {
        let exchange = next_exchange(&self.trace, &self.cursor, ProviderKind::Llm)?;
        check_request(&exchange, canonical(request)?)?;
        serde_json::from_str(&exchange.response_json)
            .map_err(|error| ProviderFailure::new("provider.replay_decode", error.to_string()))
    }
}

pub struct ReplayVision {
    trace: Vec<ProviderExchange>,
    cursor: Mutex<usize>,
}

impl ReplayVision {
    pub fn new(trace: ProviderTrace) -> Self {
        Self {
            trace: trace.exchanges,
            cursor: Mutex::new(0),
        }
    }
}

impl VisionProvider for ReplayVision {
    fn name(&self) -> &str {
        "replay"
    }

    fn inspect(&self, request: &VisionRequest) -> Result<VisionReport, ProviderFailure> {
        let exchange = next_exchange(&self.trace, &self.cursor, ProviderKind::Vision)?;
        check_request(&exchange, canonical(request)?)?;
        serde_json::from_str(&exchange.response_json)
            .map_err(|error| ProviderFailure::new("provider.replay_decode", error.to_string()))
    }
}

pub struct ReplayImage {
    trace: Vec<ProviderExchange>,
    cursor: Mutex<usize>,
}

impl ReplayImage {
    pub fn new(trace: ProviderTrace) -> Self {
        Self {
            trace: trace.exchanges,
            cursor: Mutex::new(0),
        }
    }
}

impl ImageProvider for ReplayImage {
    fn name(&self) -> &str {
        "replay"
    }

    fn generate(&self, request: &ImageRequest) -> Result<ImageOutput, ProviderFailure> {
        let exchange = next_exchange(&self.trace, &self.cursor, ProviderKind::Image)?;
        check_request(&exchange, canonical(request)?)?;
        let generation = serde_json::from_str(&exchange.response_json)
            .map_err(|error| ProviderFailure::new("provider.replay_decode", error.to_string()))?;
        Ok(ImageOutput {
            png: exchange.payload,
            generation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedLlm;

    impl LlmProvider for FixedLlm {
        fn name(&self) -> &str {
            "fixed"
        }

        fn propose(&self, request: &LlmRequest) -> Result<LlmOutput, ProviderFailure> {
            if request.intent.contains("fail") {
                return Err(ProviderFailure::new("provider.refused", "模型拒绝"));
            }
            Ok(LlmOutput::AssetPlan { assets: Vec::new() })
        }
    }

    struct FixedImage;

    impl ImageProvider for FixedImage {
        fn name(&self) -> &str {
            "fixed-image"
        }

        fn generate(&self, request: &ImageRequest) -> Result<ImageOutput, ProviderFailure> {
            Ok(ImageOutput {
                png: vec![0x89, 0x50, 0x4e, 0x47],
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

    fn request(intent: &str) -> LlmRequest {
        LlmRequest {
            intent: intent.to_owned(),
            spec: None,
            selected_key: Some("root".to_owned()),
            capabilities: Capabilities::core(),
            seed: Some(7),
        }
    }

    #[test]
    fn llm_trace_replays_offline() {
        let recorder = RecordingLlm::new(FixedLlm);
        let first = recorder.propose(&request("add a shop button")).unwrap();
        let trace = recorder.trace();
        assert_eq!(trace.exchanges.len(), 1);

        let json = trace.to_json();
        let decoded = ProviderTrace::from_json(&json).unwrap();
        let replay = ReplayLlm::new(decoded);
        assert_eq!(
            replay.propose(&request("add a shop button")).unwrap(),
            first
        );
    }

    #[test]
    fn replay_rejects_a_different_request() {
        let recorder = RecordingLlm::new(FixedLlm);
        recorder.propose(&request("add a shop button")).unwrap();
        let replay = ReplayLlm::new(recorder.trace());
        let failure = replay.propose(&request("delete the root")).unwrap_err();
        assert_eq!(failure.code, "provider.replay_request_mismatch");
    }

    #[test]
    fn replay_reports_exhaustion_and_order() {
        let replay = ReplayLlm::new(ProviderTrace::default());
        let failure = replay.propose(&request("anything")).unwrap_err();
        assert_eq!(failure.code, "provider.replay_exhausted");

        let recorder = RecordingImage::new(FixedImage);
        recorder
            .generate(&ImageRequest {
                prompt: "panel".to_owned(),
                size: [64, 32],
                negative: None,
                seed: Some(3),
                style_tokens: BTreeMap::new(),
            })
            .unwrap();
        let replay = ReplayLlm::new(recorder.trace());
        let failure = replay.propose(&request("anything")).unwrap_err();
        assert_eq!(failure.code, "provider.replay_order");
    }

    #[test]
    fn image_payload_round_trips_through_trace() {
        let recorder = RecordingImage::new(FixedImage);
        let request = ImageRequest {
            prompt: "wooden panel".to_owned(),
            size: [64, 32],
            negative: Some("text".to_owned()),
            seed: Some(11),
            style_tokens: BTreeMap::new(),
        };
        let first = recorder.generate(&request).unwrap();
        let trace = ProviderTrace::from_json(&recorder.trace().to_json()).unwrap();
        let replay = ReplayImage::new(trace);
        assert_eq!(replay.generate(&request).unwrap(), first);
    }

    #[test]
    fn failures_do_not_enter_the_trace() {
        let recorder = RecordingLlm::new(FixedLlm);
        assert!(recorder.propose(&request("fail now")).is_err());
        assert!(recorder.trace().exchanges.is_empty());
    }
}
