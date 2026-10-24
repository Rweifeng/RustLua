//! M75: AI 融合能力层（MCP tool 面 + 确认网关 + 确定性回放 + 内置面板 Provider）。
//!
//! 本 crate **不依赖编辑器**：只定义协议、工具契约与执行接口。真正的工具执行由宿主
//! （`editor` 的 `ai_host`）实现，保证 AI 动作仍走 `EditorCommand` 与事务/撤销。
//! 这样 wasm/game 构建不会因为引入 AI 而拉起任何依赖。

pub mod bridge;
pub mod image_provider;
pub mod import_tools;
pub mod lua_check;
pub mod mcp;
pub mod provider;
pub mod replay;
pub mod surface;
pub mod ui_provider;
pub mod ui_tools;
pub mod vision_provider;

pub use bridge::{
    AiBridge, GatePolicy, HostRequest, PendingToolCall, ToolError, ToolHost, execute_tool,
    tier_label,
};
pub use image_provider::{
    IMAGE_KEY_ENV, LocalImageProvider, RemoteImageConfig, RemoteImageProvider,
};
pub use import_tools::{
    ImportError, ImportedSpec, ScreenshotImport, import_figma_json, suggest_screenshot_patch,
};
pub use lua_check::{ApiIssue, surface_tables, validate_api_calls};
pub use mcp::HostCall;
pub use provider::{
    ChatMessage, CurlTransport, HttpRequest, HttpResponse, ModelResponse, ModelToolCall, Provider,
    ProviderConfig, ProviderError, ProviderKind, ProviderSettings, SYSTEM_PROMPT, Transport,
};
pub use replay::{ReplayError, ReplayFixture, ReplayReport, ReplayStep, run_replay};
pub use surface::{
    McpResource, ToolSpec, ToolTier, resource_list, resource_surface_json, surface_json,
    tool_by_name, tools,
};
pub use ui_provider::{
    ImageOutput, ImageProvider, ImageRequest, LlmOutput, LlmProvider, LlmRequest, ProviderFailure,
    ProviderTrace, RecordingImage, RecordingLlm, RecordingVision, ReplayImage, ReplayLlm,
    ReplayVision, VisionProvider, VisionReport, VisionRequest,
};
pub use ui_tools::{
    ApplyPatchRequest, ApplySpecRequest, ExportSpecRequest, GenerateAssetsRequest,
    ListAssetsRequest, PreviewSpecRequest, PromoteAssetRequest, UiToolError, UiToolErrorCode,
    UiToolRegistry, UiToolResult, ValidateSpecRequest, ValidateSpecResponse,
};
pub use vision_provider::{
    RemoteVisionConfig, RemoteVisionProvider, VISION_KEY_ENV, VISION_LIVE_ENV,
};
