//! P3-R03-4：`Environment` 场景组件（环境贴图 + 强度 + 绕 Y 旋转）。
//!
//! 组件串：`map=<项目相对 .rlenv|.hdr>[;intensity=<>=0>][;rotation=<弧度>]`。
//! `.hdr` 是源文件；运行时读的是**同名 `.rlenv`**（导入期烘焙，见
//! `rustlua_assets::env_asset`）。因此组件保留作者写的 `.hdr`，加载侧用
//! [`EnvironmentComponent::resolved_rlenv_path`] 换成产物路径。
//!
//! 组件挂在哪个实体上不重要——环境是场景级状态（渲染器只有一个环境 uniform/绑定）。
//! 运行时取第一个合法声明；重复声明不再是错误，值排序后取第一份（确定）。

use std::path::Path;

/// 场景文件里的组件名（与 `component_schema`/`typed_scene` 读数一致）。
pub const ENVIRONMENT_COMPONENT: &str = "Environment";

/// 环境组件的 CPU 侧取值。
#[derive(Clone, Debug, PartialEq)]
pub struct EnvironmentComponent {
    /// 作者写的路径：`assets/env/studio.rlenv` 或 `assets/env/studio.hdr`。
    pub map: String,
    /// 环境辐射倍率（`>= 0`）。
    pub intensity: f32,
    /// 绕 Y 轴旋转（弧度）。只影响环境采样方向。
    pub rotation: f32,
}

impl Default for EnvironmentComponent {
    fn default() -> Self {
        Self {
            map: String::new(),
            intensity: 1.0,
            rotation: 0.0,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum EnvironmentComponentError {
    #[error("malformed Environment field: {0}")]
    MalformedField(String),
    #[error("missing Environment map path")]
    MissingMap,
    #[error("invalid Environment map path: {0}")]
    InvalidMapPath(String),
    #[error("unsupported Environment map extension: {0}")]
    UnsupportedExtension(String),
    #[error("invalid Environment intensity: {0}")]
    InvalidIntensity(String),
    #[error("invalid Environment rotation: {0}")]
    InvalidRotation(String),
}

impl EnvironmentComponent {
    /// 解析场景组件串。字段顺序任意，重复字段后者生效。
    pub fn from_component_str(value: &str) -> Result<Self, EnvironmentComponentError> {
        let mut map: Option<String> = None;
        let mut intensity = 1.0_f32;
        let mut rotation = 0.0_f32;
        for field in value.split(';') {
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| EnvironmentComponentError::MalformedField(field.to_owned()))?;
            match key {
                "map" => {
                    validate_map(raw)?;
                    map = Some(raw.to_owned());
                }
                "intensity" => {
                    let parsed = raw
                        .parse::<f32>()
                        .map_err(|_| EnvironmentComponentError::InvalidIntensity(raw.to_owned()))?;
                    if !parsed.is_finite() || parsed < 0.0 {
                        return Err(EnvironmentComponentError::InvalidIntensity(raw.to_owned()));
                    }
                    intensity = parsed;
                }
                "rotation" => {
                    let parsed = raw
                        .parse::<f32>()
                        .map_err(|_| EnvironmentComponentError::InvalidRotation(raw.to_owned()))?;
                    if !parsed.is_finite() {
                        return Err(EnvironmentComponentError::InvalidRotation(raw.to_owned()));
                    }
                    rotation = parsed;
                }
                _ => return Err(EnvironmentComponentError::MalformedField(field.to_owned())),
            }
        }
        let map = map.ok_or(EnvironmentComponentError::MissingMap)?;
        Ok(Self {
            map,
            intensity,
            rotation,
        })
    }

    /// 编辑器写回时的规范串（字段顺序固定，值永不丢精度格式）。
    #[must_use]
    pub fn to_component_str(&self) -> String {
        format!(
            "map={};intensity={};rotation={}",
            self.map,
            format_float(self.intensity),
            format_float(self.rotation)
        )
    }

    /// 运行时实际读取的 `.rlenv` 路径：`.hdr` 换成同名产物，其余原样。
    #[must_use]
    pub fn resolved_rlenv_path(&self) -> String {
        let path = Path::new(&self.map);
        if path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("hdr"))
        {
            path.with_extension("rlenv").to_string_lossy().into_owned()
        } else {
            self.map.clone()
        }
    }
}

fn validate_map(map: &str) -> Result<(), EnvironmentComponentError> {
    if map.is_empty() {
        return Err(EnvironmentComponentError::MissingMap);
    }
    if rustlua_infra::paths::is_absolute_path(map)
        || rustlua_infra::paths::escapes_project_root(map)
    {
        return Err(EnvironmentComponentError::InvalidMapPath(map.to_owned()));
    }
    let supported = Path::new(map)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("rlenv") || extension.eq_ignore_ascii_case("hdr")
        });
    if !supported {
        return Err(EnvironmentComponentError::UnsupportedExtension(
            map.to_owned(),
        ));
    }
    Ok(())
}

fn format_float(value: f32) -> String {
    let text = format!("{value}");
    if text == "-0" { "0".to_owned() } else { text }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_fields_and_round_trips() {
        let component = EnvironmentComponent::from_component_str(
            "map=assets/env/studio.rlenv;intensity=2.5;rotation=0.75",
        )
        .expect("parse");
        assert_eq!(component.map, "assets/env/studio.rlenv");
        assert_eq!(component.intensity, 2.5);
        assert_eq!(component.rotation, 0.75);
        assert_eq!(
            component.to_component_str(),
            "map=assets/env/studio.rlenv;intensity=2.5;rotation=0.75"
        );
        let again =
            EnvironmentComponent::from_component_str(&component.to_component_str()).expect("again");
        assert_eq!(again, component);
    }

    #[test]
    fn defaults_apply_when_optional_fields_are_missing() {
        let component =
            EnvironmentComponent::from_component_str("map=assets/env/studio.hdr").expect("parse");
        assert_eq!(component.intensity, 1.0);
        assert_eq!(component.rotation, 0.0);
        assert_eq!(component.resolved_rlenv_path(), "assets/env/studio.rlenv");
        assert_eq!(
            EnvironmentComponent::from_component_str("map=assets/env/studio.rlenv")
                .expect("rlenv")
                .resolved_rlenv_path(),
            "assets/env/studio.rlenv"
        );
    }

    #[test]
    fn rejects_bad_paths_and_values() {
        for payload in [
            "",
            "map=",
            "map=assets/env/studio.png",
            "map=../outside.rlenv",
            "map=/etc/studio.rlenv",
            "map=assets/env/studio.rlenv;intensity=-1",
            "map=assets/env/studio.rlenv;intensity=NaN",
            "map=assets/env/studio.rlenv;rotation=abc",
            "map=assets/env/studio.rlenv;bogus=1",
            "intensity=1",
            "map=assets/env/studio.rlenv;intensity",
        ] {
            assert!(
                EnvironmentComponent::from_component_str(payload).is_err(),
                "must reject {payload:?}"
            );
        }
    }

    #[test]
    fn hdr_extension_is_case_insensitive_and_swapped() {
        let component =
            EnvironmentComponent::from_component_str("map=assets/env/STUDIO.HDR").expect("parse");
        assert_eq!(component.resolved_rlenv_path(), "assets/env/STUDIO.rlenv");
    }
}
