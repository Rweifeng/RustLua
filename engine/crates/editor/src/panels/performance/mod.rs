//! 编辑器性能分析面板：一个**独立弹窗**，分两层展示读数。
//!
//! - **运行时层**（Play 预览）：帧率、帧间隔、CPU/GPU 渲染、Lua/更新耗时、draw call、
//!   set pass、实例/顶点/三角形、内存、贴图/网格缓存与加载队列（见 `runtime_perf.rs`）；
//! - **编辑器层**：编辑器帧/面板/命令耗时、布局缓存命中率、资源扫描与队列计数。
//!
//! 这份数字原先画在 Scene 视图**右上角的覆盖层**（`painter.text`）里，而那个角落正好是
//! 工具小面板（选择/移动/旋转/缩放）的位置——两段文字叠在一起：工具按钮被盖住、数字也读不全。
//! 现在数字搬进本窗口（可拖动、可缩放、可关闭），视口里不再有性能文字，工具条独占那个角落。
//! 工具栏的「性能」勾选框与「窗口 ▸ 性能分析面板」菜单项只负责**开关这个窗口**。
//!
//! 分工：`profiler.rs` 负责编辑器有界采样与报告，`runtime_perf.rs` 负责运行时快照与趋势，
//! 本模块只负责"把报告排成能读的表 / 把趋势画成折线"与画窗口。因此排版逻辑
//! （`performance_sections`）是纯函数，可以脱离 egui 断言。
//!
//! 拆分（P4-②）：按"数据 / 排版 / 趋势 / 窗口"四段拆成目录——`model.rs`（行与节的类型、
//! 格式化助手）、`sections.rs`（纯函数排版）、`trends.rs`（折线图绘制）、`panel.rs`（窗口与
//! 追踪时间线）、`tests/`（原文件末尾的测试模块，逐条未改）。**只搬位置，不改行为**：
//! `performance_sections` 仍是可脱离 egui 断言的纯函数。

use std::time::Duration;

use crate::EditorApp;
use crate::runtime_perf::{RuntimePerfHistory, RuntimePerfSnapshot, RuntimeSeries, bytes_to_mib};
use editor::{ProfilerReport, TimingSummary};
use eframe::egui;
use engine::host::CaptureLevel;
use engine::host::profile_trace::SpanRecord;
use engine::host::runtime_scene::FrameScheduleReport;
use rustlua_ui::ui_layout_cache::LayoutCacheStats;

mod model;
mod panel;
mod sections;
#[cfg(test)]
mod tests;
mod trends;

pub(crate) use model::*;
// `panel.rs` 里是 `impl EditorApp`（固有实现不参与 glob）与三个只被窗口和测试用到的
// 自由函数；非测试构建没有任何外部引用，故仅在测试里转发，免得常态构建报 unused。
#[cfg(test)]
pub(crate) use panel::*;
pub(crate) use sections::*;
pub(crate) use trends::*;
