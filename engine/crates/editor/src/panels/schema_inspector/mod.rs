//! 通用（schema 驱动）组件字段编辑器（docs/55 阶段 2「schema 驱动 Inspector」）。
//!
//! 这一块的存在理由是计划里的硬要求：**新增组件不再修改巨型 match**。凡是引擎里注册了
//! schema、编辑器又没有手写面板的组件（`editor::driven_components` 推导出来的集合），
//! 都在这里按 schema 自动生成输入行；视图**只发 typed action**
//! （`EditorCommand::SetComponentField`），取值校验、Play 锁定、"无变化不产生历史"
//! 全部由 shell 的 dispatch 统一处理。
//!
//! 输入缓冲用 egui 的临时数据（按 `实体 + 路径` 建键）而不是新增 `EditorApp` 字段：
//! 未获得焦点时缓冲跟随文档（Undo/Redo、外部改动都会立刻反映），获得焦点时保留用户
//! 正在输入的内容。

use crate::EditorApp;
use editor::{ChoiceSource, field_choice_source};
use eframe::egui;
use rustlua_ecs::component_schema::FieldKind;

mod fields;
mod panel;
#[cfg(test)]
mod tests;

pub(crate) use fields::*;
