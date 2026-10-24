//! UI tool 面的唯一能力层（方案 v2 M0）。
//!
//! 本期只登记契约：工具名、强类型请求/返回与 tier；执行体在 M76-03。
//! 内部 Copilot / UI Studio 直接以 Rust 类型调用 [`UiToolRegistry`]（主入口），
//! 外部 MCP 适配层只把同一份请求/返回编解码成 JSON-RPC，不另写业务逻辑。

use rustlua_ui_spec::{
    AssetProvenance, AssetRole, Diagnostic, ImageGeneration, Severity, UiAssetPlan, UiPatch,
    UiProposal, UiSpec,
};
use serde::{Deserialize, Serialize};
use std::fmt;

pub mod names {
    pub const EXPORT_SPEC: &str = "ui_export_spec";
    pub const VALIDATE_SPEC: &str = "ui_validate_spec";
    pub const PREVIEW_SPEC: &str = "ui_preview_spec";
    pub const LIST_ASSETS: &str = "ui_list_assets";
    pub const APPLY_SPEC: &str = "ui_apply_spec";
    pub const APPLY_PATCH: &str = "ui_apply_patch";
    pub const CREATE_PRESET: &str = "ui_create_preset";
    pub const PROMOTE_ASSET: &str = "ui_promote_asset";
    pub const GENERATE_ASSETS: &str = "ui_generate_assets";
    pub const SLICE_ASSET: &str = "ui_slice_asset";
}

pub const TOOL_NAMES: &[&str] = &[
    names::EXPORT_SPEC,
    names::VALIDATE_SPEC,
    names::PREVIEW_SPEC,
    names::LIST_ASSETS,
    names::APPLY_SPEC,
    names::APPLY_PATCH,
    names::CREATE_PRESET,
    names::PROMOTE_ASSET,
    names::GENERATE_ASSETS,
    names::SLICE_ASSET,
];

pub fn is_ui_tool(name: &str) -> bool {
    TOOL_NAMES.contains(&name)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiToolErrorCode {
    InvalidArgument,
    StaleRevision,
    InvalidSpec,
    Capability,
    NotFound,
    ConfirmationRequired,
    BudgetExceeded,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiToolError {
    pub code: UiToolErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

impl UiToolError {
    pub fn new(code: UiToolErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            diagnostics: Vec::new(),
        }
    }

    pub fn with_diagnostics(mut self, diagnostics: Vec<Diagnostic>) -> Self {
        self.diagnostics = diagnostics;
        self
    }
}

impl fmt::Display for UiToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for UiToolError {}

pub type UiToolResult<T> = Result<T, UiToolError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportSpecRequest {
    /// 工程内 `.ui.json` 相对路径。
    pub document: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportSpecResponse {
    pub document: String,
    pub revision: u64,
    pub spec: UiSpec,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidateSpecRequest {
    pub spec: UiSpec,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidateSpecResponse {
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

impl ValidateSpecResponse {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

fn default_preview_scale() -> f32 {
    1.0
}

fn default_preview_width() -> u32 {
    1280
}

fn default_preview_height() -> u32 {
    720
}

/// 新建预设默认给 640×360 预览：足够看清布局，又不会让 MCP 回执膨胀到数 MB。
fn default_preset_width() -> u32 {
    640
}

fn default_preset_height() -> u32 {
    360
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreviewSpecRequest {
    pub spec: UiSpec,
    #[serde(default = "default_preview_width")]
    pub width: u32,
    #[serde(default = "default_preview_height")]
    pub height: u32,
    #[serde(default = "default_preview_scale")]
    pub scale: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreviewSpecResponse {
    pub width: u32,
    pub height: u32,
    /// 离屏渲染结果（PNG 字节）；MCP 层负责编码，内部 API 直接拿字节。
    pub png: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListAssetsRequest {
    /// 省略时列出整个工程已登记的 UI 资产。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiAssetEntry {
    pub key: String,
    pub role: AssetRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<AssetProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListAssetsResponse {
    #[serde(default)]
    pub assets: Vec<UiAssetEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApplySpecRequest {
    pub document: String,
    pub base_revision: u64,
    pub spec: UiSpec,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApplyPatchRequest {
    pub document: String,
    /// `UiPatch` 自带 `base_revision`，过期时由执行体直接拒绝。
    pub patch: UiPatch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApplyResponse {
    /// 写动作先成为提案；人工接受后才开事务、落命令、进 Undo 栈。
    pub proposal: UiProposal,
}

fn default_true() -> bool {
    true
}

/// 新建 UI 预设：只允许创建**不存在**的文档，已存在时拒绝并提示走 `ui_apply_patch`。
///
/// 与 `ui_apply_spec` 的区别是它把「提案 → 接受」一次做完，因为新文件没有可回滚的
/// 旧内容；`base_revision` 固定为 0，接受时再校验文件是否已被别人创建。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreatePresetRequest {
    pub document: String,
    pub spec: UiSpec,
    /// 是否随回执返回预览 PNG（base64）。默认 true，模型可据此自检布局。
    #[serde(default = "default_true")]
    pub preview: bool,
    #[serde(default = "default_preset_width")]
    pub width: u32,
    #[serde(default = "default_preset_height")]
    pub height: u32,
    #[serde(default = "default_preview_scale")]
    pub scale: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreatePresetResponse {
    pub document: String,
    pub revision: u64,
    pub spec: UiSpec,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_png_base64: Option<String>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromoteAssetRequest {
    pub asset_id: String,
    /// 未审阅的生成资产不得进入发布包。
    pub reviewed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default)]
    pub used_by: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromoteAssetResponse {
    pub provenance: AssetProvenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerateAssetsRequest {
    pub plan: Vec<UiAssetPlan>,
    /// 单次调用的图像上限；超过即拒绝而不是静默截断。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_images: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeneratedAsset {
    pub key: String,
    pub png: Vec<u8>,
    pub generation: ImageGeneration,
    pub provenance: AssetProvenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerateAssetsResponse {
    #[serde(default)]
    pub generated: Vec<GeneratedAsset>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SliceRegionRequest {
    pub name: String,
    /// `[x, y, width, height]`，像素坐标。
    pub rect: [u32; 4],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SliceAssetRequest {
    /// 工程内源 PNG 相对路径。
    pub source: String,
    /// 裁片与 `slices.json` 的输出目录（工程内相对路径）。
    pub output_dir: String,
    #[serde(default)]
    pub regions: Vec<SliceRegionRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlicedAssetEntry {
    pub name: String,
    pub path: String,
    pub rect: [u32; 4],
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SliceAssetResponse {
    pub source: String,
    pub source_width: u32,
    pub source_height: u32,
    pub output_dir: String,
    #[serde(default)]
    pub slices: Vec<SlicedAssetEntry>,
}

/// 进程内主入口。外部 MCP 通道由宿主把 JSON 解码成这些类型后调用同一实现。
pub trait UiToolRegistry {
    fn export_spec(&mut self, request: &ExportSpecRequest) -> UiToolResult<ExportSpecResponse>;
    fn validate_spec(
        &mut self,
        request: &ValidateSpecRequest,
    ) -> UiToolResult<ValidateSpecResponse>;
    fn preview_spec(&mut self, request: &PreviewSpecRequest) -> UiToolResult<PreviewSpecResponse>;
    fn list_assets(&mut self, request: &ListAssetsRequest) -> UiToolResult<ListAssetsResponse>;
    fn apply_spec(&mut self, request: &ApplySpecRequest) -> UiToolResult<ApplyResponse>;
    fn apply_patch(&mut self, request: &ApplyPatchRequest) -> UiToolResult<ApplyResponse>;
    /// 新建 `.ui.json` 预设（只新建，不覆盖）。T1：一次提案 + 接受，落盘后返回 revision。
    fn create_preset(
        &mut self,
        request: &CreatePresetRequest,
    ) -> UiToolResult<CreatePresetResponse>;
    fn promote_asset(
        &mut self,
        request: &PromoteAssetRequest,
    ) -> UiToolResult<PromoteAssetResponse>;
    fn generate_assets(
        &mut self,
        request: &GenerateAssetsRequest,
    ) -> UiToolResult<GenerateAssetsResponse>;
    /// 按手工矩形从源 PNG 裁片（落盘裁片 + `slices.json`）。T2：写工程文件。
    fn slice_asset(&mut self, request: &SliceAssetRequest) -> UiToolResult<SliceAssetResponse>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_object_safe(_registry: &dyn UiToolRegistry) {}

    #[test]
    fn registry_is_object_safe_and_names_are_unique() {
        assert_object_safe(&NoopRegistry);
        let unique: std::collections::BTreeSet<&str> = TOOL_NAMES.iter().copied().collect();
        assert_eq!(unique.len(), TOOL_NAMES.len());
        assert_eq!(TOOL_NAMES.len(), 10);
        assert!(is_ui_tool("ui_apply_patch"));
        assert!(is_ui_tool("ui_create_preset"));
        assert!(is_ui_tool("ui_slice_asset"));
        assert!(!is_ui_tool("write_lua_script"));
    }

    #[test]
    fn requests_round_trip_as_json() {
        let request = ApplySpecRequest {
            document: "ui/shop.ui.json".to_owned(),
            base_revision: 3,
            spec: minimal_spec(),
        };
        let decoded: ApplySpecRequest =
            serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap();
        assert_eq!(decoded, request);

        let patch = ApplyPatchRequest {
            document: "ui/shop.ui.json".to_owned(),
            patch: UiPatch {
                base_revision: 3,
                ops: Vec::new(),
            },
        };
        let decoded: ApplyPatchRequest =
            serde_json::from_str(&serde_json::to_string(&patch).unwrap()).unwrap();
        assert_eq!(decoded, patch);
    }

    #[test]
    fn create_preset_defaults_to_a_small_preview() {
        let request: CreatePresetRequest = serde_json::from_value(serde_json::json!({
            "document": "ui/forge.ui.json",
            "spec": minimal_spec()
        }))
        .expect("create preset request");
        assert!(request.preview, "默认要返回预览，用户才看得到结果");
        assert_eq!(
            (request.width, request.height),
            (640, 360),
            "默认预览不能回到 1280×720，否则回执会膨胀到数 MB"
        );
        assert_eq!(request.scale, 1.0);
    }

    #[test]
    fn validation_response_separates_errors_from_warnings() {
        let warning = ValidateSpecResponse {
            diagnostics: vec![Diagnostic::warning("w", "root", "warn")],
        };
        assert!(!warning.has_errors());
        let error = ValidateSpecResponse {
            diagnostics: vec![Diagnostic::error("e", "root", "error")],
        };
        assert!(error.has_errors());
    }

    #[test]
    fn promote_requires_review_flag_in_payload() {
        let request = PromoteAssetRequest {
            asset_id: "ui/panel".to_owned(),
            reviewed: true,
            license: Some("CC0".to_owned()),
            used_by: vec!["shop.root".to_owned()],
        };
        let raw = serde_json::to_value(&request).unwrap();
        assert_eq!(raw["reviewed"], serde_json::json!(true));
    }

    fn minimal_spec() -> UiSpec {
        serde_json::from_value(serde_json::json!({
            "spec_version": 1,
            "document": {"kind": "widget", "name": "shop"},
            "root": {"key": "root", "type": "panel"}
        }))
        .unwrap()
    }

    struct NoopRegistry;

    impl UiToolRegistry for NoopRegistry {
        fn export_spec(&mut self, _: &ExportSpecRequest) -> UiToolResult<ExportSpecResponse> {
            Err(UiToolError::new(
                UiToolErrorCode::Unavailable,
                "M76-03 落地前无执行体",
            ))
        }

        fn validate_spec(&mut self, _: &ValidateSpecRequest) -> UiToolResult<ValidateSpecResponse> {
            Err(UiToolError::new(
                UiToolErrorCode::Unavailable,
                "M76-03 落地前无执行体",
            ))
        }

        fn preview_spec(&mut self, _: &PreviewSpecRequest) -> UiToolResult<PreviewSpecResponse> {
            Err(UiToolError::new(
                UiToolErrorCode::Unavailable,
                "M76-03 落地前无执行体",
            ))
        }

        fn list_assets(&mut self, _: &ListAssetsRequest) -> UiToolResult<ListAssetsResponse> {
            Err(UiToolError::new(
                UiToolErrorCode::Unavailable,
                "M76-03 落地前无执行体",
            ))
        }

        fn apply_spec(&mut self, _: &ApplySpecRequest) -> UiToolResult<ApplyResponse> {
            Err(UiToolError::new(
                UiToolErrorCode::Unavailable,
                "M76-03 落地前无执行体",
            ))
        }

        fn apply_patch(&mut self, _: &ApplyPatchRequest) -> UiToolResult<ApplyResponse> {
            Err(UiToolError::new(
                UiToolErrorCode::Unavailable,
                "M76-03 落地前无执行体",
            ))
        }

        fn create_preset(&mut self, _: &CreatePresetRequest) -> UiToolResult<CreatePresetResponse> {
            Err(UiToolError::new(
                UiToolErrorCode::Unavailable,
                "M76-03 落地前无执行体",
            ))
        }

        fn promote_asset(&mut self, _: &PromoteAssetRequest) -> UiToolResult<PromoteAssetResponse> {
            Err(UiToolError::new(
                UiToolErrorCode::Unavailable,
                "M76-03 落地前无执行体",
            ))
        }

        fn generate_assets(
            &mut self,
            _: &GenerateAssetsRequest,
        ) -> UiToolResult<GenerateAssetsResponse> {
            Err(UiToolError::new(
                UiToolErrorCode::Unavailable,
                "M76-03 落地前无执行体",
            ))
        }

        fn slice_asset(&mut self, _: &SliceAssetRequest) -> UiToolResult<SliceAssetResponse> {
            Err(UiToolError::new(
                UiToolErrorCode::Unavailable,
                "M76-03 落地前无执行体",
            ))
        }
    }
}
