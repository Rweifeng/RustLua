//! 分包配置与计划表（`docs/67` §5）：实现在 [`rustlua_infra::bundle_plan`]。
//!
//! 本模块只是构建工具侧的**再导出**：`assetc` 与编辑器面板必须用同一份归约规则，
//! 否则"预览一套、打包另一套"（`docs/67` §8 第 4 项）。计划表的**结构**（`bundle_plan.json`）
//! 仍在 `rustlua_infra::bundle::plan`：构建期写、运行期读，两边必须同一份定义。

pub use rustlua_infra::bundle::plan::{BundlePlan, PLAN_FILE, PLAN_FORMAT, PlannedGroup};
pub use rustlua_infra::bundle_plan::{
    BundleConfig, GroupSpec, MAX_BUNDLE_BYTES, MAX_TOTAL_BYTES, ProjectKeys, UnitKind, build_plan,
    check_plan, is_main_unit, parse_unit, plan_to_json, sanitize, scan_project_entries, sub_name,
    validate_plan,
};
