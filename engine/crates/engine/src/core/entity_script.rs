//! docs/54 §4.1：**每实体 `Script` 组件**。
//!
//! §4.1 的实体结构里 `Script` 与 `Renderable`/`Camera`/`Animator` 并列，但此前脚本只有**工程级**
//! 一个：`scripts/main.lua` 的 `update()` 每帧被调一次，实体与行为的对应关系只能靠脚本自己维护
//! （遍历实体、按名字查、自己写分派表）。组件化之后，场景可以直接声明"这个实体跑哪个脚本文件"：
//!
//! ```text
//! entity  9  Patrol
//! component  9  Script  file=scripts/ai/patrol.lua
//! ```
//!
//! 刻意的几条边界：
//!
//! - **一个文件一份 Lua 状态，实体是实例**。`file` 相同的实体共享同一份 Lua 状态（顶层局部变量
//!   因此是按文件共享的），脚本用收到的 `id` 区分自己：`on_update(id, dt)`。这样上万实体不会变成
//!   上万份 Lua 状态；每实体状态请写在该文件里以 `id` 为键的表里（docs/18 写明）。
//! - **钩子名与工程级脚本分开**：工程级仍是 `on_create()` / `update()` / `on_destroy()`（一字未改），
//!   每实体脚本用 `on_spawn(id)` / `on_update(id, dt)` / `on_despawn(id)`。两组名字不重叠，因此
//!   一个文件同时被两边加载也不会互相踩。
//! - **路径被限制在工程根内**：绝对路径与含 `..` 的路径**报错**。脚本是可执行代码，场景文件能让
//!   它去读工程外的任何文件就不是"配置"而是"漏洞"了。
//! - **`enabled=false` 不写出字段**：与 `Camera2D/Camera3D` 的 `enabled` 同一口径，既有场景串不变。

use std::path::{Path, PathBuf};

/// 实体级脚本组件的**唯一权威名字**。
///
/// 场景里这个组件的键就叫 `Script`（见模块文档与 `typed_scene` 的组件表）。历史上编辑器
/// 写的是 `LuaScript`（裸路径）、schema 注册表写的是 `EntityScript`，而运行时只读 `Script`——
/// 三处名字不一致意味着"Inspector 里设了脚本路径，运行时永不加载"。现在运行时、加载器、
/// schema 注册表与编辑器都从这里取名字，旧写法由
/// 旧的 `LuaScript` 写法与它的迁移代码已在第 55 轮删除（见 docs/56 §3.51）。
pub const SCRIPT_COMPONENT: &str = "Script";

/// 实体级脚本组件的负载。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EntityScript {
    /// 相对工程根的脚本路径（如 `scripts/ai/patrol.lua`）。
    pub file: String,
    /// 关掉后不加载也不调用（但组件仍在，方便编辑器/诊断看到"这里有脚本，只是停了"）。
    pub enabled: bool,
}

impl Default for EntityScript {
    fn default() -> Self {
        Self {
            file: String::new(),
            enabled: true,
        }
    }
}

impl EntityScript {
    /// 解析场景 `Script` 组件串：`file=<相对路径>` 必填，可选 `enabled=false`。
    pub fn from_component_str(value: &str) -> Result<Self, EntityScriptError> {
        let mut file: Option<String> = None;
        let mut enabled = true;
        let mut seen = false;
        for field in value
            .trim()
            .split(';')
            .filter(|field| !field.trim().is_empty())
        {
            seen = true;
            let (key, raw) = field
                .split_once('=')
                .map(|(key, raw)| (key.trim(), raw.trim()))
                .ok_or_else(|| EntityScriptError::MalformedField(field.to_owned()))?;
            match key {
                "file" => {
                    if raw.is_empty() {
                        return Err(EntityScriptError::EmptyFile);
                    }
                    file = Some(raw.to_owned());
                }
                "enabled" => {
                    enabled = match raw {
                        "true" => true,
                        "false" => false,
                        other => {
                            return Err(EntityScriptError::InvalidField(format!(
                                "enabled={other}"
                            )));
                        }
                    };
                }
                other => return Err(EntityScriptError::UnknownField(other.to_owned())),
            }
        }
        if !seen {
            return Err(EntityScriptError::Empty);
        }
        let file = file.ok_or(EntityScriptError::MissingField("file"))?;
        let script = Self { file, enabled };
        script.validate()?;
        Ok(script)
    }

    /// 写回组件串。`enabled=true`（默认）**不写出来**，因此"读进来再写回去"对既有写法稳定。
    #[must_use]
    pub fn to_component_str(&self) -> String {
        if self.enabled {
            format!("file={}", self.file)
        } else {
            format!("file={};enabled=false", self.file)
        }
    }

    /// 路径必须是工程根内的**相对**路径：绝对路径与含 `..` 的路径都拒绝。
    ///
    /// 判定本身来自 `core::paths`（schema 的 `FieldRule::ProjectPath` 用同一份实现）——
    /// 两边各写一遍时曾经不一致：只查 `Path::is_absolute()` 会放行 Windows 上的 `/etc/passwd`。
    pub fn validate(&self) -> Result<(), EntityScriptError> {
        if self.file.trim().is_empty() {
            return Err(EntityScriptError::EmptyFile);
        }
        if crate::core::paths::is_absolute_path(&self.file) {
            return Err(EntityScriptError::AbsolutePath(self.file.clone()));
        }
        if crate::core::paths::escapes_project_root(&self.file) {
            return Err(EntityScriptError::EscapesProjectRoot(self.file.clone()));
        }
        Ok(())
    }

    /// 相对工程根解析出的绝对路径。
    #[must_use]
    pub fn resolved_path(&self, project_root: &Path) -> PathBuf {
        project_root.join(&self.file)
    }
}

/// `Script` 组件串的解析/校验错误。
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum EntityScriptError {
    #[error("script component is empty")]
    Empty,
    #[error("script is missing field {0}")]
    MissingField(&'static str),
    #[error("malformed script field: {0}")]
    MalformedField(String),
    #[error("unknown script field: {0}")]
    UnknownField(String),
    #[error("invalid script field: {0}")]
    InvalidField(String),
    #[error("script file must not be empty")]
    EmptyFile,
    #[error("script path must be relative to the project root: {0}")]
    AbsolutePath(String),
    #[error("script path must stay inside the project root: {0}")]
    EscapesProjectRoot(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_file_and_round_trips() {
        let script = EntityScript::from_component_str("file=scripts/ai/patrol.lua")
            .expect("valid script component");
        assert_eq!(script.file, "scripts/ai/patrol.lua");
        assert!(script.enabled);
        assert_eq!(
            script.to_component_str(),
            "file=scripts/ai/patrol.lua",
            "启用时不写出 enabled（既有场景串不变）"
        );

        let disabled = EntityScript::from_component_str("file=scripts/ai/patrol.lua;enabled=false")
            .expect("valid script component");
        assert!(!disabled.enabled);
        assert_eq!(
            EntityScript::from_component_str(&disabled.to_component_str()).unwrap(),
            disabled
        );
    }

    #[test]
    fn rejects_malformed_components() {
        for raw in [
            "",
            "script=patrol.lua",
            "file",
            "file=",
            "file=patrol.lua;enabled=maybe",
        ] {
            assert!(
                EntityScript::from_component_str(raw).is_err(),
                "{raw:?} 应当被拒绝"
            );
        }
        assert_eq!(
            EntityScript::from_component_str("file=patrol.lua;loop=true"),
            Err(EntityScriptError::UnknownField("loop".to_owned()))
        );
        assert_eq!(
            EntityScript::from_component_str("enabled=false"),
            Err(EntityScriptError::MissingField("file"))
        );
    }

    /// 脚本是可执行代码，场景文件不能把它指到工程外：绝对路径与 `..` 都必须在解析期就拒绝。
    #[test]
    fn refuses_paths_outside_the_project_root() {
        for raw in [
            "file=/etc/passwd.lua",
            "file=C:/Windows/system32/x.lua",
            "file=../outside.lua",
            "file=scripts/../../outside.lua",
        ] {
            assert!(
                EntityScript::from_component_str(raw).is_err(),
                "{raw:?} 应当被拒绝"
            );
        }
        let script =
            EntityScript::from_component_str("file=scripts/ai/patrol.lua").expect("valid script");
        assert_eq!(
            script.resolved_path(Path::new("/project")),
            Path::new("/project/scripts/ai/patrol.lua")
        );
    }
}
