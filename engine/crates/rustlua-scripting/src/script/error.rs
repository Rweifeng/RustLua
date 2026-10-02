//! Lua 桥的错误与诊断：`LuaBridgeError`、脚本指纹、`ScriptDebugger`、报错构造器。
//!
//! `api_error` / `world_error` 是 275 个绑定函数共用的**唯一**报错出口：绑定层只经这两条
//! 产出 `mlua::Error`，因此 Lua 侧看到的报错文案格式一致、且都能被 `ScriptDebugger` 归并。

use std::{path::PathBuf, time::SystemTime};

use rustlua_ecs::world::WorldError;

/// Errors produced while loading or running a project Lua script.
///
/// 文本字段命名为 `reason` 而不是 `source`：thiserror 会把名为 `source` 的字段一律
/// 当作错误源（`AsDynError`），而这里携带的是已经格式化成字符串的 Lua 报错文本。
#[derive(Debug, thiserror::Error)]
pub enum LuaBridgeError {
    #[error("Lua VM initialization failed: {0}")]
    Init(String),
    #[error("failed to load {}: {reason}", .path.display())]
    Load { path: PathBuf, reason: String },
    #[error("script {} failed: {reason}", .path.display())]
    Runtime { path: PathBuf, reason: String },
    #[error("Lua API error: {0}")]
    Api(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptFingerprint {
    pub(super) modified: Option<SystemTime>,
    pub(super) length: u64,
    pub(super) hash: u64,
}

/// A structured diagnostic emitted by a Lua script load or execution failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptDiagnostic {
    pub path: PathBuf,
    pub phase: String,
    pub message: String,
    pub line: Option<u32>,
    pub stack_trace: String,
    pub occurrences: u32,
}

#[derive(Clone, Debug, Default)]
pub struct ScriptDebugger {
    records: Vec<ScriptDiagnostic>,
    filter: Option<String>,
}

impl ScriptDebugger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, path: PathBuf, phase: impl Into<String>, source: impl Into<String>) {
        let phase = phase.into();
        let source = source.into();
        let line = find_line_number(&source);
        if let Some(existing) = self
            .records
            .iter_mut()
            .find(|item| item.path == path && item.phase == phase && item.message == source)
        {
            existing.occurrences = existing.occurrences.saturating_add(1);
            return;
        }
        self.records.push(ScriptDiagnostic {
            path,
            phase,
            message: source.clone(),
            line,
            stack_trace: source,
            occurrences: 1,
        });
    }

    pub fn clear(&mut self) {
        self.records.clear();
    }

    pub fn set_filter(&mut self, filter: Option<String>) {
        self.filter = filter;
    }

    #[must_use]
    pub fn all(&self) -> &[ScriptDiagnostic] {
        &self.records
    }

    #[must_use]
    pub fn visible(&self) -> Vec<ScriptDiagnostic> {
        self.records
            .iter()
            .filter(|item| {
                self.filter.as_ref().is_none_or(|filter| {
                    item.message.contains(filter) || item.path.to_string_lossy().contains(filter)
                })
            })
            .cloned()
            .collect()
    }
}

pub(super) fn find_line_number(source: &str) -> Option<u32> {
    let bytes = source.as_bytes();
    for index in 0..bytes.len() {
        if bytes[index] != b':' {
            continue;
        }
        let start = index + 1;
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end > start
            && (end == bytes.len() || bytes[end] == 58 || bytes[end] == 32)
            && let Ok(line) = source[start..end].parse()
        {
            return Some(line);
        }
    }
    None
}

pub(super) fn api_error(message: impl Into<String>) -> mlua::Error {
    mlua::Error::RuntimeError(message.into())
}

pub(super) fn world_error(error: WorldError) -> mlua::Error {
    api_error(error.to_string())
}
