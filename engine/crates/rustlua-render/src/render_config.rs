//! 项目级渲染质量配置（`project.toml` 的 `[render]` 段）。
//!
//! 单一配置来源：编辑器的「渲染设置」面板与 `game` 宿主都读这一份，默认值 = 逐项开关的
//! 旧行为（无 `[render]` 段的工程像素逐字节不变）。本模块只做**数据 + 文本**，不碰 GPU；
//! 落到渲染器由 `rustlua_gpu::Renderer::apply_render_config` 负责。
//!
//! 与 `--quality` 的关系：预设不是第二份真值，`with_quality` 只是把预设包含的几个字段
//! 填成同一组值。文件里存的是逐项字段，预设只在 UI/命令行入口作为"一键填值"使用。

use std::path::{Path, PathBuf};

/// `project.toml` 里的段名（表头本身无语义，键按全文件扁平扫描，与既有工程设置一致）。
pub const RENDER_SECTION: &str = "render";

pub const DEFAULT_PLANAR_DIVISOR: u32 = 2;
pub const DEFAULT_SHADOW_RESOLUTION: u32 = 2048;
pub const DEFAULT_SHADOW_CASCADES: u32 = 4;
pub const DEFAULT_POINT_SHADOW_RESOLUTION: u32 = 1024;
pub const DEFAULT_POINT_SHADOW_LIGHTS: u32 = 8;
pub const MAX_WATER_REFRACT_STEPS: u32 = 32;

/// 画质预设（只描述"把哪些字段填成哪组值"，本身不入文件）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum QualityPreset {
    /// 逐项开关的旧行为。
    #[default]
    Default,
    /// planar 1/4 + 水面廉价光照。
    Medium,
    /// medium + 点光 cube 阴影 512。
    Low,
}

impl QualityPreset {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }

    /// 从稳定短名解析（大小写不敏感；未知值返回 `None`）。
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "default" => Some(Self::Default),
            "medium" => Some(Self::Medium),
            "low" => Some(Self::Low),
            _ => None,
        }
    }
}

/// 项目级生产画质项。字段顺序与 `upsert_section` 写出的行序一致。
#[derive(Clone, Debug, PartialEq)]
pub struct RenderConfig {
    /// 平面反射目标分辨率除数（1..=8，默认 2 = 半分辨率）。
    pub planar_divisor: u32,
    /// 全部水面强制廉价光照档（默认只按 spec 的 `cheap=1`）。
    pub water_cheap: bool,
    /// 全部地形强制廉价细节档。
    pub terrain_cheap: bool,
    /// 全部普通网格强制廉价着色档。
    pub crowd_cheap: bool,
    /// 3D 最大绘制距离（米，0 = 关闭）。
    pub draw_distance: f32,
    /// 距离档只作用于网格路径含该子串的实例（`None` = 全体）。
    pub draw_distance_filter: Option<String>,
    /// 方向光 CSM 阴影图边长（512/1024/2048/4096；**构建期**）。
    pub shadow_resolution: u32,
    /// 方向光 CSM 实际渲染的级联层数（1..=4；**构建期**）。
    pub shadow_cascades: u32,
    /// 点光 cube 阴影面的边长（512/1024；**构建期**）。
    pub point_shadow_resolution: u32,
    /// 点光阴影盏数预算（1..=8；运行期）。
    pub point_shadow_lights: u32,
    /// 阴影投射者的距离预算（米，0 = 关闭）。
    pub shadow_caster_distance: f32,
    /// 深度加权水面折射增益（0 = 关闭 = 旧的固定偏移）。
    pub water_refraction: f32,
    /// 水下视角的水-气界面强度（0..=1）。
    pub underwater_view: f32,
    /// 水面折射的屏幕空间 Snell 步进次数（0..=32）。
    pub water_refract_steps: u32,
    /// 剖切口的实体封口深度（米，0 = 关闭）。
    pub clip_cap: f32,
    /// 封口是否也吃到单面网格。
    pub clip_cap_both_sides: bool,
    /// 自定义特效叠加层：工程内 `.sgraph` 路径（`None` = 关闭，像素与旧版相同）。
    pub effect_overlay: Option<String>,
    /// M98：GPU 粒子软边宽度（深度单位）。默认 0.02 = 旧行为；0 = 关（硬边）；越大软边越宽。
    pub particle_softness: f32,
}

/// M98：GPU 粒子软边默认宽度（= P3-R107 的旧硬编码值，像素不变）。
pub const DEFAULT_PARTICLE_SOFTNESS: f32 = 0.02;

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            planar_divisor: DEFAULT_PLANAR_DIVISOR,
            water_cheap: false,
            terrain_cheap: false,
            crowd_cheap: false,
            draw_distance: 0.0,
            draw_distance_filter: None,
            shadow_resolution: DEFAULT_SHADOW_RESOLUTION,
            shadow_cascades: DEFAULT_SHADOW_CASCADES,
            point_shadow_resolution: DEFAULT_POINT_SHADOW_RESOLUTION,
            point_shadow_lights: DEFAULT_POINT_SHADOW_LIGHTS,
            shadow_caster_distance: 0.0,
            water_refraction: 0.0,
            underwater_view: 0.0,
            water_refract_steps: 0,
            clip_cap: 0.0,
            clip_cap_both_sides: false,
            effect_overlay: None,
            particle_softness: DEFAULT_PARTICLE_SOFTNESS,
        }
    }
}

impl RenderConfig {
    /// 把每个字段夹到渲染器实际接受的档位（非法值回落到默认，不 panic）。
    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.planar_divisor = if (1..=8).contains(&self.planar_divisor) {
            self.planar_divisor
        } else {
            DEFAULT_PLANAR_DIVISOR
        };
        self.shadow_resolution = if [512, 1024, 2048, 4096].contains(&self.shadow_resolution) {
            self.shadow_resolution
        } else {
            DEFAULT_SHADOW_RESOLUTION
        };
        self.shadow_cascades = if (1..=4).contains(&self.shadow_cascades) {
            self.shadow_cascades
        } else {
            DEFAULT_SHADOW_CASCADES
        };
        self.point_shadow_resolution = if [512, 1024].contains(&self.point_shadow_resolution) {
            self.point_shadow_resolution
        } else {
            DEFAULT_POINT_SHADOW_RESOLUTION
        };
        self.point_shadow_lights = if (1..=8).contains(&self.point_shadow_lights) {
            self.point_shadow_lights
        } else {
            DEFAULT_POINT_SHADOW_LIGHTS
        };
        self.water_refract_steps = self.water_refract_steps.min(MAX_WATER_REFRACT_STEPS);
        self.underwater_view = finite_non_negative(self.underwater_view).min(1.0);
        self.water_refraction = finite_non_negative(self.water_refraction);
        self.shadow_caster_distance = finite_non_negative(self.shadow_caster_distance);
        self.clip_cap = finite_non_negative(self.clip_cap);
        self.draw_distance = finite_non_negative(self.draw_distance);
        self.draw_distance_filter = self
            .draw_distance_filter
            .map(|filter| filter.trim().to_owned())
            .filter(|filter| !filter.is_empty());
        self.effect_overlay = self
            .effect_overlay
            .map(|path| path.trim().to_owned())
            .filter(|path| !path.is_empty());
        // 0 = 硬边（关）；上限 0.5 防止把整颗粒子吃掉。
        self.particle_softness = finite_non_negative(self.particle_softness).min(0.5);
        self
    }

    /// 套用画质预设（只改预设包含的字段；未包含的保持原值）。
    #[must_use]
    pub fn with_quality(mut self, preset: QualityPreset) -> Self {
        match preset {
            QualityPreset::Default => {}
            QualityPreset::Medium => {
                self.planar_divisor = 4;
                self.water_cheap = true;
            }
            QualityPreset::Low => {
                self.planar_divisor = 4;
                self.water_cheap = true;
                self.point_shadow_resolution = 512;
            }
        }
        self
    }

    /// 构建期阴影参数 `(方向光边长, 级联层数, 点光面边长)`；必须传给 `Renderer::new_*`。
    #[must_use]
    pub const fn shadow_settings(&self) -> (u32, u32, u32) {
        (
            self.shadow_resolution,
            self.shadow_cascades,
            self.point_shadow_resolution,
        )
    }

    /// 两份配置之间的差异是否需要**重建渲染器**（三项构建期参数）。
    #[must_use]
    pub fn needs_renderer_rebuild(&self, other: &Self) -> bool {
        self.shadow_settings() != other.shadow_settings()
    }

    /// 命令行/面板的逐项覆盖（`None` = 保留当前值）。
    #[must_use]
    pub fn overlay(&self, overrides: &RenderOverrides) -> Self {
        let mut merged = self.clone();
        if let Some(value) = overrides.planar_divisor {
            merged.planar_divisor = value;
        }
        if let Some(value) = overrides.water_cheap {
            merged.water_cheap = value;
        }
        if let Some(value) = overrides.terrain_cheap {
            merged.terrain_cheap = value;
        }
        if let Some(value) = overrides.crowd_cheap {
            merged.crowd_cheap = value;
        }
        if let Some(value) = overrides.draw_distance {
            merged.draw_distance = value;
        }
        if let Some(value) = &overrides.draw_distance_filter {
            merged.draw_distance_filter = Some(value.clone());
        }
        if let Some(value) = overrides.shadow_resolution {
            merged.shadow_resolution = value;
        }
        if let Some(value) = overrides.shadow_cascades {
            merged.shadow_cascades = value;
        }
        if let Some(value) = overrides.point_shadow_resolution {
            merged.point_shadow_resolution = value;
        }
        if let Some(value) = overrides.point_shadow_lights {
            merged.point_shadow_lights = value;
        }
        if let Some(value) = overrides.shadow_caster_distance {
            merged.shadow_caster_distance = value;
        }
        if let Some(value) = overrides.water_refraction {
            merged.water_refraction = value;
        }
        if let Some(value) = overrides.underwater_view {
            merged.underwater_view = value;
        }
        if let Some(value) = overrides.water_refract_steps {
            merged.water_refract_steps = value;
        }
        if let Some(value) = overrides.clip_cap {
            merged.clip_cap = value;
        }
        if let Some(value) = overrides.clip_cap_both_sides {
            merged.clip_cap_both_sides = value;
        }
        merged.normalized()
    }

    /// 解析 `project.toml` 文本里的 `[render]` 键（未知键忽略，非法值回落默认）。
    #[must_use]
    pub fn from_toml(text: &str) -> Self {
        let mut config = Self::default();
        if let Some(value) = value_for(text, "planar_divisor") {
            config.planar_divisor = value.parse().unwrap_or(DEFAULT_PLANAR_DIVISOR);
        }
        if let Some(value) = value_for(text, "water_cheap") {
            config.water_cheap = parse_bool(value).unwrap_or(false);
        }
        if let Some(value) = value_for(text, "terrain_cheap") {
            config.terrain_cheap = parse_bool(value).unwrap_or(false);
        }
        if let Some(value) = value_for(text, "crowd_cheap") {
            config.crowd_cheap = parse_bool(value).unwrap_or(false);
        }
        if let Some(value) = value_for(text, "draw_distance") {
            config.draw_distance = value.parse().unwrap_or(0.0);
        }
        if let Some(value) = value_for(text, "draw_distance_filter") {
            config.draw_distance_filter = Some(value.to_owned());
        }
        if let Some(value) = value_for(text, "shadow_resolution") {
            config.shadow_resolution = value.parse().unwrap_or(DEFAULT_SHADOW_RESOLUTION);
        }
        if let Some(value) = value_for(text, "shadow_cascades") {
            config.shadow_cascades = value.parse().unwrap_or(DEFAULT_SHADOW_CASCADES);
        }
        if let Some(value) = value_for(text, "point_shadow_resolution") {
            config.point_shadow_resolution =
                value.parse().unwrap_or(DEFAULT_POINT_SHADOW_RESOLUTION);
        }
        if let Some(value) = value_for(text, "point_shadow_lights") {
            config.point_shadow_lights = value.parse().unwrap_or(DEFAULT_POINT_SHADOW_LIGHTS);
        }
        if let Some(value) = value_for(text, "shadow_caster_distance") {
            config.shadow_caster_distance = value.parse().unwrap_or(0.0);
        }
        if let Some(value) = value_for(text, "water_refraction") {
            config.water_refraction = value.parse().unwrap_or(0.0);
        }
        if let Some(value) = value_for(text, "underwater_view") {
            config.underwater_view = value.parse().unwrap_or(0.0);
        }
        if let Some(value) = value_for(text, "water_refract_steps") {
            config.water_refract_steps = value.parse().unwrap_or(0);
        }
        if let Some(value) = value_for(text, "clip_cap") {
            config.clip_cap = value.parse().unwrap_or(0.0);
        }
        if let Some(value) = value_for(text, "clip_cap_both_sides") {
            config.clip_cap_both_sides = parse_bool(value).unwrap_or(false);
        }
        if let Some(value) = value_for(text, "effect_overlay") {
            config.effect_overlay = Some(value.to_owned());
        }
        if let Some(value) = value_for(text, "particle_softness") {
            config.particle_softness = value.parse().unwrap_or(DEFAULT_PARTICLE_SOFTNESS);
        }
        config.normalized()
    }

    /// 保序写回 `[render]` 段：已有键就地替换，缺失键补进段尾；注释与其它段原样保留。
    /// 没有该段时在文件末尾追加。
    #[must_use]
    pub fn upsert_section(&self, existing: &str) -> String {
        let edits = self.key_values();
        upsert_lines(existing, RENDER_SECTION, &edits)
    }

    /// 键值对（键序 = 写出行序，也是 UI 分组顺序）。
    fn key_values(&self) -> Vec<(&'static str, String)> {
        vec![
            ("planar_divisor", self.planar_divisor.to_string()),
            ("water_cheap", self.water_cheap.to_string()),
            ("terrain_cheap", self.terrain_cheap.to_string()),
            ("crowd_cheap", self.crowd_cheap.to_string()),
            ("draw_distance", fmt_float(self.draw_distance)),
            (
                "draw_distance_filter",
                self.draw_distance_filter.clone().unwrap_or_default(),
            ),
            ("shadow_resolution", self.shadow_resolution.to_string()),
            ("shadow_cascades", self.shadow_cascades.to_string()),
            (
                "point_shadow_resolution",
                self.point_shadow_resolution.to_string(),
            ),
            ("point_shadow_lights", self.point_shadow_lights.to_string()),
            (
                "shadow_caster_distance",
                fmt_float(self.shadow_caster_distance),
            ),
            ("water_refraction", fmt_float(self.water_refraction)),
            ("underwater_view", fmt_float(self.underwater_view)),
            ("water_refract_steps", self.water_refract_steps.to_string()),
            ("clip_cap", fmt_float(self.clip_cap)),
            ("clip_cap_both_sides", self.clip_cap_both_sides.to_string()),
            (
                "effect_overlay",
                self.effect_overlay.clone().unwrap_or_default(),
            ),
            ("particle_softness", fmt_float(self.particle_softness)),
        ]
    }

    /// 设置文件路径（与音频/输入段共用同一份 `project.toml`）。
    #[must_use]
    pub fn path_in(root: &Path) -> PathBuf {
        root.join("project.toml")
    }

    /// 读取工程配置：文件不存在返回 `None`（调用方用默认值），读取失败返回可读原因。
    pub fn load_from(root: &Path) -> Result<Option<Self>, String> {
        let path = Self::path_in(root);
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Some(Self::from_toml(&text))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("读取 {} 失败：{error}", path.display())),
        }
    }

    /// 写回 `project.toml` 的 `[render]` 段（其余内容原样保留）。
    pub fn save_to(&self, root: &Path) -> Result<(), String> {
        let path = Self::path_in(root);
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        std::fs::write(&path, self.upsert_section(&existing))
            .map_err(|error| format!("保存 {} 失败：{error}", path.display()))
    }

    /// 日志/诊断用的一行摘要。
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "planar={} water_cheap={} terrain_cheap={} crowd_cheap={} draw_distance={} draw_filter={:?} \
             shadow={}/{}/{} lights={} shadow_caster_distance={} water_refraction={} underwater_view={} \
             refract_steps={} clip_cap={} clip_cap_both_sides={}",
            self.planar_divisor,
            self.water_cheap,
            self.terrain_cheap,
            self.crowd_cheap,
            self.draw_distance,
            self.draw_distance_filter,
            self.shadow_resolution,
            self.shadow_cascades,
            self.point_shadow_resolution,
            self.point_shadow_lights,
            self.shadow_caster_distance,
            self.water_refraction,
            self.underwater_view,
            self.water_refract_steps,
            self.clip_cap,
            self.clip_cap_both_sides
        )
    }
}
/// 逐项覆盖：`None` = 该项未被显式指定（保留项目文件/默认值）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderOverrides {
    pub planar_divisor: Option<u32>,
    pub water_cheap: Option<bool>,
    pub terrain_cheap: Option<bool>,
    pub crowd_cheap: Option<bool>,
    pub draw_distance: Option<f32>,
    pub draw_distance_filter: Option<String>,
    pub shadow_resolution: Option<u32>,
    pub shadow_cascades: Option<u32>,
    pub point_shadow_resolution: Option<u32>,
    pub point_shadow_lights: Option<u32>,
    pub shadow_caster_distance: Option<f32>,
    pub water_refraction: Option<f32>,
    pub underwater_view: Option<f32>,
    pub water_refract_steps: Option<u32>,
    pub clip_cap: Option<f32>,
    pub clip_cap_both_sides: Option<bool>,
}

fn finite_non_negative(value: f32) -> f32 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        0.0
    }
}

/// 稳定浮点文本：整数保留一位小数（`0.0`），小数最短化。
fn fmt_float(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

fn parse_bool(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn trimmed(line: &str) -> &str {
    line.trim().trim_start_matches('\u{feff}')
}

/// 提取一个键的值（全文件扁平扫描，与引擎既有 `project.toml` 读取语义一致）。
fn value_for<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    for line in text.lines() {
        let line = trimmed(line);
        if let Some(rest) = line.strip_prefix(key) {
            let Some(rest) = rest.strip_prefix(" =") else {
                continue;
            };
            let value = rest.trim().trim_matches('"');
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn is_header(line: &str) -> bool {
    let line = trimmed(line);
    line.starts_with('[') && line.ends_with(']')
}

fn line_is_key(line: &str, key: &str) -> bool {
    let line = trimmed(line);
    line.strip_prefix(key)
        .is_some_and(|rest| rest.starts_with(" ="))
}

fn render_line(key: &str, value: &str) -> String {
    let quoted = key == "draw_distance_filter";
    if quoted {
        format!("{key} = \"{value}\"")
    } else {
        format!("{key} = {value}")
    }
}

/// 段级保序 upsert（与 `rustlua-gameplay::settings` 的 `[audio]` 写回同一条语义）。
fn upsert_lines(existing: &str, section: &str, edits: &[(&str, String)]) -> String {
    let mut had_newline = existing.ends_with('\n');
    let mut lines: Vec<String> = existing.lines().map(str::to_owned).collect();
    let header = lines
        .iter()
        .position(|line| trimmed(line) == format!("[{section}]"));
    match header {
        Some(index) => {
            let end = lines[index + 1..]
                .iter()
                .position(|line| is_header(line))
                .map(|offset| index + 1 + offset)
                .unwrap_or(lines.len());
            let mut matched = vec![false; edits.len()];
            for line in &mut lines[index + 1..end] {
                let Some((slot, (key, value))) = edits
                    .iter()
                    .enumerate()
                    .find(|(slot, (key, _))| !matched[*slot] && line_is_key(line, key))
                    .map(|(slot, edit)| (slot, edit.clone()))
                else {
                    continue;
                };
                *line = render_line(key, &value);
                matched[slot] = true;
            }
            let missing: Vec<String> = edits
                .iter()
                .zip(&matched)
                .filter(|(_, done)| !**done)
                .map(|((key, value), _)| render_line(key, value))
                .collect();
            for (offset, line) in missing.into_iter().enumerate() {
                lines.insert(end + offset, line);
            }
        }
        None => {
            if lines.iter().any(|line| !line.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("[{section}]"));
            for (key, value) in edits {
                lines.push(render_line(key, value));
            }
            had_newline = true;
        }
    }
    let mut result = lines.join("\n");
    if had_newline && !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_keep_the_old_switch_behavior() {
        let config = RenderConfig::default();
        assert_eq!(config.planar_divisor, 2);
        assert_eq!(config.shadow_settings(), (2048, 4, 1024));
        assert_eq!(config.point_shadow_lights, 8);
        assert!(!config.water_cheap && !config.terrain_cheap && !config.crowd_cheap);
        assert_eq!(config.draw_distance, 0.0);
        assert_eq!(config.water_refraction, 0.0);
        assert_eq!(config.draw_distance_filter, None);
    }

    #[test]
    fn effect_overlay_path_round_trips_and_defaults_off() {
        assert_eq!(RenderConfig::default().effect_overlay, None);
        let text = RenderConfig::default().upsert_section("");
        assert!(
            text.contains("effect_overlay = "),
            "写出的 [render] 段应含该项：{text}"
        );
        let configured = RenderConfig {
            effect_overlay: Some("assets/fx/fire.sgraph".to_owned()),
            ..RenderConfig::default()
        };
        let written = configured.upsert_section("");
        let parsed = RenderConfig::from_toml(&written);
        assert_eq!(
            parsed.effect_overlay.as_deref(),
            Some("assets/fx/fire.sgraph"),
            "应解析出 .sgraph 路径：{written}"
        );
        // 空白值等价于关闭（不写空字符串进渲染器）。
        let empty = RenderConfig::from_toml("[render]\neffect_overlay = \"  \"\n");
        assert_eq!(empty.effect_overlay, None);
    }

    #[test]
    fn quality_presets_only_touch_their_own_fields() {
        let medium = RenderConfig::default().with_quality(QualityPreset::Medium);
        assert_eq!(medium.planar_divisor, 4);
        assert!(medium.water_cheap);
        assert_eq!(medium.point_shadow_resolution, 1024);

        let low = RenderConfig {
            shadow_cascades: 2,
            ..RenderConfig::default()
        }
        .with_quality(QualityPreset::Low);
        assert_eq!(low.planar_divisor, 4);
        assert_eq!(low.point_shadow_resolution, 512);
        assert_eq!(low.shadow_cascades, 2, "预设不包含的字段保持原值");

        assert_eq!(QualityPreset::parse("LOW"), Some(QualityPreset::Low));
        assert_eq!(QualityPreset::parse("ultra"), None);
    }

    #[test]
    fn parsing_clamps_invalid_values_instead_of_panicking() {
        let text = "[render]\n\
                    planar_divisor = 99\n\
                    shadow_resolution = 300\n\
                    shadow_cascades = 9\n\
                    point_shadow_resolution = 2048\n\
                    point_shadow_lights = 0\n\
                    underwater_view = 4.0\n\
                    water_refract_steps = 100\n\
                    clip_cap = not-a-number\n";
        let config = RenderConfig::from_toml(text);
        assert_eq!(config.planar_divisor, 2);
        assert_eq!(config.shadow_settings(), (2048, 4, 1024));
        assert_eq!(config.point_shadow_lights, 8);
        assert_eq!(config.underwater_view, 1.0);
        assert_eq!(config.water_refract_steps, 32);
        assert_eq!(config.clip_cap, 0.0);
    }

    #[test]
    fn parsing_reads_explicit_values() {
        let text = "[render]\n\
                    planar_divisor = 4\n\
                    water_cheap = true\n\
                    terrain_cheap = true\n\
                    crowd_cheap = true\n\
                    draw_distance = 35.0\n\
                    draw_distance_filter = \"grass\"\n\
                    shadow_resolution = 1024\n\
                    shadow_cascades = 2\n\
                    point_shadow_resolution = 512\n\
                    point_shadow_lights = 3\n\
                    shadow_caster_distance = 20.0\n\
                    water_refraction = 0.0008\n\
                    underwater_view = 0.5\n\
                    water_refract_steps = 8\n\
                    clip_cap = 1.5\n\
                    clip_cap_both_sides = true\n";
        let config = RenderConfig::from_toml(text);
        assert_eq!(config.planar_divisor, 4);
        assert!(config.water_cheap && config.terrain_cheap && config.crowd_cheap);
        assert_eq!(config.draw_distance, 35.0);
        assert_eq!(config.draw_distance_filter.as_deref(), Some("grass"));
        assert_eq!(config.shadow_settings(), (1024, 2, 512));
        assert_eq!(config.point_shadow_lights, 3);
        assert_eq!(config.shadow_caster_distance, 20.0);
        assert_eq!(config.water_refraction, 0.0008);
        assert_eq!(config.underwater_view, 0.5);
        assert_eq!(config.water_refract_steps, 8);
        assert_eq!(config.clip_cap, 1.5);
        assert!(config.clip_cap_both_sides);
    }

    #[test]
    fn upsert_replaces_keys_in_place_and_keeps_comments_and_other_sections() {
        let existing = "[project]\nname = \"demo\"\n\n[render]\n# 手写注释\nplanar_divisor = 1\nwater_cheap = true\n\n[audio]\nmaster = 0.5\n";
        let config = RenderConfig {
            planar_divisor: 4,
            water_cheap: false,
            ..RenderConfig::default()
        };
        let out = config.upsert_section(existing);
        assert!(out.contains("# 手写注释"), "{out}");
        assert!(out.contains("[audio]\nmaster = 0.5"), "{out}");
        assert!(out.contains("planar_divisor = 4"), "{out}");
        assert!(out.contains("water_cheap = false"), "{out}");
        assert!(!out.contains("planar_divisor = 1"), "{out}");
        // 新增键补进 [render] 段尾，不越过下一个表头。
        let render_pos = out.find("[render]").unwrap();
        let audio_pos = out.find("[audio]").unwrap();
        let cascades_pos = out.find("shadow_cascades = 4").unwrap();
        assert!(
            render_pos < cascades_pos && cascades_pos < audio_pos,
            "{out}"
        );
    }

    #[test]
    fn upsert_is_idempotent_and_appends_missing_section() {
        let config = RenderConfig {
            planar_divisor: 4,
            draw_distance: 30.0,
            ..RenderConfig::default()
        };
        let once = config.upsert_section("[project]\nname = \"demo\"\n");
        let twice = config.upsert_section(&once);
        assert_eq!(once, twice);
        assert!(once.contains("[render]\nplanar_divisor = 4"), "{once}");
        // 末尾键随字段追加而移动：这里只钉"最后确实是 [render] 的键"，不钉具体是哪一列。
        let last_line = once.lines().last().expect("non-empty");
        assert!(
            last_line.starts_with("effect_overlay = ")
                || last_line.starts_with("particle_softness = ")
                || last_line.starts_with("sprite_softness = "),
            "{once}"
        );
        assert_eq!(once.matches("[render]").count(), 1, "{once}");
        assert_eq!(once.matches("planar_divisor =").count(), 1, "{once}");
    }

    /// M98：粒子软边宽度可读写、默认值不变、越界被夹紧。
    #[test]
    fn particle_softness_round_trips_and_clamps() {
        let default = RenderConfig::default();
        assert!((default.particle_softness - 0.02).abs() < 1e-6);

        let text = RenderConfig {
            particle_softness: 0.08,
            ..RenderConfig::default()
        }
        .upsert_section("");
        assert!(text.contains("particle_softness = 0.08"), "{text}");
        let parsed = RenderConfig::from_toml(&text);
        assert!((parsed.particle_softness - 0.08).abs() < 1e-6);

        // 越界/非有限：夹紧到 [0, 0.5]，不是静默保留 NaN。
        let clamped = RenderConfig::from_toml("[render]\nparticle_softness = 9.0\n");
        assert!((clamped.particle_softness - 0.5).abs() < 1e-6);
        let disabled = RenderConfig::from_toml("[render]\nparticle_softness = 0\n");
        assert_eq!(disabled.particle_softness, 0.0);
    }

    #[test]
    fn empty_filter_round_trips_as_none() {
        let config = RenderConfig::default();
        let text = config.upsert_section("");
        assert!(text.contains("draw_distance_filter = \"\""), "{text}");
        assert_eq!(RenderConfig::from_toml(&text).draw_distance_filter, None);
    }

    #[test]
    fn only_construction_time_fields_request_a_renderer_rebuild() {
        let base = RenderConfig::default();
        assert!(!base.needs_renderer_rebuild(&base));
        assert!(!base.needs_renderer_rebuild(&RenderConfig {
            planar_divisor: 4,
            ..RenderConfig::default()
        }));
        for changed in [
            RenderConfig {
                shadow_resolution: 1024,
                ..RenderConfig::default()
            },
            RenderConfig {
                shadow_cascades: 2,
                ..RenderConfig::default()
            },
            RenderConfig {
                point_shadow_resolution: 512,
                ..RenderConfig::default()
            },
        ] {
            assert!(base.needs_renderer_rebuild(&changed));
        }
    }

    #[test]
    fn overrides_only_touch_provided_fields() {
        let base = RenderConfig {
            planar_divisor: 1,
            water_cheap: true,
            ..RenderConfig::default()
        };
        let merged = base.overlay(&RenderOverrides {
            planar_divisor: Some(4),
            ..RenderOverrides::default()
        });
        assert_eq!(merged.planar_divisor, 4);
        assert!(merged.water_cheap, "未覆盖的字段保持项目文件值");
    }

    #[test]
    fn save_and_load_round_trip() {
        let root = std::env::temp_dir().join(format!(
            "rustlua-render-config-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("project.toml"), "[project]\nname = \"demo\"\n").unwrap();
        let config = RenderConfig {
            planar_divisor: 4,
            terrain_cheap: true,
            draw_distance_filter: Some("grass".to_owned()),
            ..RenderConfig::default()
        };
        config.save_to(&root).unwrap();
        let loaded = RenderConfig::load_from(&root).unwrap().unwrap();
        assert_eq!(loaded, config);
        // 缺文件 → None（调用方用默认值）。
        let missing = root.join("nope");
        std::fs::create_dir_all(&missing).unwrap();
        assert_eq!(RenderConfig::load_from(&missing).unwrap(), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
