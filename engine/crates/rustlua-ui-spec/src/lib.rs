//! AI 与编辑器之间的 UI 契约层（方案 v2 M0）。
//!
//! 这层不依赖 editor、egui 或运行时 crate：AI 产出 [`spec::UiSpec`] /
//! [`patch::UiPatch`]，先由 [`validate`] 对照能力矩阵做确定性校验，再交给编译器
//! 落成 `EditorCommand`。这样 AI 永远不直接写 `.scene`、组件字符串或 Lua 代码。

pub mod asset;
pub mod capabilities;
pub mod compiler;
pub mod patch;
pub mod preview;
pub mod proposal;
pub mod slice;
pub mod spec;
pub mod validate;

pub use asset::{
    AssetBudget, AssetEntry, AssetLimits, AssetProvenance, AssetReviewReport, AssetRole,
    AssetSource, ImageGeneration, UiAssetPlan, review_assets,
};
pub use capabilities::Capabilities;
pub use compiler::{
    CompiledUi, JSON_SAFE_REVISION_LIMIT, NodeChange, UiCommand, UiSpecDiff, compile, content_hash,
    diff_specs, revision_hash,
};
pub use patch::{UiPatch, UiPatchOp, apply_patch, select_ops};
pub use preview::{
    LayoutRect, ResizeHandle, encode_rgba_png, layout_rects, offsets_for_rect, preview_fit,
    render_preview, resize_rect,
};
pub use proposal::{ProposalCost, ProposalDiff, ProposalKind, UiProposal};
pub use slice::{
    SliceError, SliceRegion, nine_slice_regions, slice_alpha, slice_grid, slice_manual, slice_rgba,
};
pub use spec::{
    Align, AnchorPreset, BindingMode, CanvasScaleMode, CanvasSpec, ContainerDirection,
    DocumentKind, Justify, ScrollbarDirection, StyleEasing, StyleState, StyleTransition,
    UiBindingSpec, UiDocumentMeta, UiLayoutSpec, UiNode, UiNodeKind, UiSpec, UiStyle,
};
pub use validate::{Diagnostic, Severity, validate_patch, validate_spec};
