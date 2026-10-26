//! `editor` 的库门面。
//!
//! 编辑器的可复用部分已经按边界拆成两个独立 crate：
//!
//! * [`rustlua_editor_core`]：工程模型、场景文档、历史/补丁、选择、布局设置、资源与
//!   schema 操作——不依赖 egui / eframe / wgpu，也不认识 `EditorApp`；
//! * [`rustlua_editor_ui_studio`]：`.ui.json` 宿主、UI Studio 无头状态与 UiSpec → `.scene`
//!   导出——同样不依赖 egui / `EditorApp`。
//!
//! 这里保留原有的 `editor::…` 路径（glob 再导出），因此面板、命令路由、集成测试与
//! 外部调用方不需要跟着改；真正留在本 crate 的只有需要引擎运行时/性能追踪的那几块
//! （`runtime_sync`、`profiler`、`schema_fields`）与 bin 侧的 GUI shell。

pub use rustlua_editor_core::*;
pub use rustlua_editor_ui_studio::*;

pub mod profiler;
pub mod runtime_sync;
pub mod schema_fields;

pub use rustlua_assets::resource_kind::ResourceKind;

pub use profiler::{
    DEFAULT_FRAME_BUDGET_MS, EditorProfiler, Percentiles, ProfilerReport, TimingSummary,
    percentiles_ms,
};

pub use schema_fields::{
    ChoiceSource, FieldEdit, FieldUnit, HANDWRITTEN_PANEL_COMPONENTS, SORTING_LAYERS,
    SchemaComponentAction, SchemaComponentNote, SchemaComponentToggle, SchemaFieldError,
    SchemaFieldRow, component_actions, component_notes, component_toggles, driven_components,
    edit_component_field, edit_component_payload, field_choice_source, field_unit,
    filter_field_rows, is_builtin_asset_reference, is_schema_driven, mark_missing_assets,
    mirror_fields, picker_candidates, preview_field_edit, rows_for_component, rows_for_entity,
    rows_for_selection,
};

/// 测试用唯一令牌（原因与 `rustlua-editor-core::test_token` 相同）。
///
/// 新 crate 的测试二进制各自独立，`pub(crate)` 帮助函数不能跨 crate 复用；
/// 留在本 crate 的 `schema_fields` 测试也需要自己的那一份。
#[cfg(test)]
pub(crate) fn test_token() -> u128 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = u128::from(COUNTER.fetch_add(1, Ordering::Relaxed));
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    (u128::from(std::process::id()) << 96) ^ (counter << 64) ^ (nanos & 0xFFFF_FFFF_FFFF_FFFF)
}
