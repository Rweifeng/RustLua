//! 工程级设置文件 `<project>/settings.toml`（M20-02）。
//!
//! 与 `project.toml`（场景/资源/启动配置）分离：本文件保存**玩家/编辑器可改的
//! 偏好**，当前只定义 `[audio]` 段（BGM/SFX/UI/Master 四路音量）。文件**可选**，
//! 缺失时全部使用默认值（1.0），不影响启动。
//!
//! ```toml
//! # RustLua 工程设置（由编辑器「项目设置」面板写入；缺失即默认）
//! [audio]
//! master = 1.0
//! bgm = 1.0
//! sfx = 1.0
//! ui = 1.0
//! ```
//!
//! 语义：
//! * 解析**宽松**——未知段、未知键、非法数值一律忽略（保留默认），不 panic；
//! * 写入**保序**——只替换/补齐 `[audio]` 段内的四个键，其余行（注释、其他段，
//!   例如 `[input]`）原样保留；
//! * 不引入任何新依赖（离线环境无法拉取 crates），逐行手写解析。
//!
//! M20-07 追加 `[input]` 段（命名动作映射）；M22/W1 起键位是**物理键名**：
//!
//! ```toml
//! [input]
//! map = "rustlua-actions-v2\ninteract=key:keye\nmove_up=key:keyw"
//! ```
//!
//! `map` 的值是 [`crate::core::input::ActionMap::to_text`] 的**文本原样**（含 `rustlua-actions-v2`
//! 版本头），按 TOML 基本字符串转义换行（`\n`）与反斜杠（`\\`）/引号（`\"`）
//! 写在一行里——多行基本字符串（`"""`）在本文件的手写解析器里不识别，因此
//! **不用**它。读取侧与 `[audio]` 一样宽松：文件缺失、段缺失、键缺失都不是
//! 错误（调用方保留当前映射）；键存在但内容非法（版本头不符 / 绑定文本非法）
//! 才回退默认映射，并把可定位的原因交给调用方记录诊断。
//!
//! **读 v1、写 v2**（M22/W1）：`rustlua-actions-v1` 的字符写法（`key:e`/`key:1`/
//! `key:up`/`key:+`/`key:other:1048577`）仍可解析并按 US-QWERTY 换算成物理键，
//! 有损换算（`key:+` → `key:equal`）逐条记进
//! [`crate::core::settings::InputSectionParse::migration_notes`]；写回一律是 v2。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::core::audio::{AudioBus, AudioSystem};
use crate::core::input::ActionMap;
use crate::core::input::GamepadDeadzone;

/// 工程设置文件名（相对项目根）。
pub const SETTINGS_FILE: &str = "settings.toml";

/// 音量所在的段名。
pub const AUDIO_SECTION: &str = "audio";

/// 命名动作映射所在的段名（M20-07）。
pub const INPUT_SECTION: &str = "input";

/// `[input]` 段内承载 [`crate::core::input::ActionMap::to_text`] 的键名（M20-07）。
pub const INPUT_MAP_KEY: &str = "map";

/// `[input]` 段内的摇杆死区键（M20-08）。
pub const INPUT_STICK_DEADZONE_KEY: &str = "stick_deadzone";

/// `[input]` 段内的扳机死区键（M20-08）。
pub const INPUT_TRIGGER_DEADZONE_KEY: &str = "trigger_deadzone";

/// `[input]` 段内的菜单导航摇杆阈值键（M20-09）。
pub const INPUT_NAV_THRESHOLD_KEY: &str = "nav_threshold";

/// `[input]` 段内的菜单导航「按住后首次重复延迟（毫秒）」键（M22/W3，写入用）。
pub const INPUT_NAV_DELAY_MS_KEY: &str = "nav_delay_ms";

/// `[input]` 段内的菜单导航「重复间隔（毫秒）」键（M22/W3，写入用）。
pub const INPUT_NAV_REPEAT_MS_KEY: &str = "nav_repeat_ms";

/// `[input]` 段内的菜单导航「按住后首次重复延迟（帧）」键（M20-09；**兼容读取**，
/// 按 60Hz 换算成毫秒，写入不再产生）。
pub const INPUT_NAV_DELAY_FRAMES_KEY: &str = "nav_delay_frames";

/// `[input]` 段内的菜单导航「重复间隔（帧）」键（M20-09；**兼容读取**）。
pub const INPUT_NAV_REPEAT_FRAMES_KEY: &str = "nav_repeat_frames";

/// 工程设置（当前只有音频分组音量；缺省全 1.0）。
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectSettings {
    volumes: BTreeMap<AudioBus, f32>,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectSettings {
    /// 默认设置：四路音量都为 1.0。
    #[must_use]
    pub fn new() -> Self {
        let mut volumes = BTreeMap::new();
        for bus in AudioBus::ALL {
            volumes.insert(bus, 1.0);
        }
        Self { volumes }
    }

    /// 读取某路音量（未知总线不会出现：`AudioBus` 是封闭枚举）。
    #[must_use]
    pub fn volume(&self, bus: AudioBus) -> f32 {
        self.volumes.get(&bus).copied().unwrap_or(1.0)
    }

    /// 写入某路音量；非有限值忽略，超界钳制到 `0..=1`。
    pub fn set_volume(&mut self, bus: AudioBus, volume: f32) {
        if !volume.is_finite() {
            return;
        }
        self.volumes.insert(bus, volume.clamp(0.0, 1.0));
    }

    /// 从运行中的音频系统取当前四路音量。
    #[must_use]
    pub fn from_audio(audio: &AudioSystem) -> Self {
        let mut settings = Self::new();
        for bus in AudioBus::ALL {
            settings.set_volume(bus, audio.bus_volume(bus));
        }
        settings
    }

    /// 把四路音量应用到音频系统（实际音量 = 主 × 分组，见 `AudioSystem`）。
    pub fn apply_to(&self, audio: &mut AudioSystem) {
        for bus in AudioBus::ALL {
            audio.set_bus_volume(bus, self.volume(bus));
        }
    }

    /// 解析设置文本；未知段/键与非法数值忽略，缺失键保留默认值。
    #[must_use]
    pub fn from_toml(text: &str) -> Self {
        let mut settings = Self::new();
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim().trim_start_matches('\u{feff}').trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                section = name.trim().to_ascii_lowercase();
                continue;
            }
            if section != AUDIO_SECTION {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let Some(bus) = AudioBus::from_key(key.trim()) else {
                // 未知键（或后续任务新增的键）忽略，不报错。
                continue;
            };
            if let Ok(volume) = value.trim().trim_matches('"').parse::<f32>() {
                settings.set_volume(bus, volume);
            }
        }
        settings
    }

    /// 规范文本（仅 `[audio]` 段；用于新建文件或测试对比）。
    #[must_use]
    pub fn to_toml(&self) -> String {
        let mut text = String::from("[audio]\n");
        for bus in AudioBus::ALL {
            text.push_str(bus.key());
            text.push_str(" = ");
            text.push_str(&fmt_volume(self.volume(bus)));
            text.push('\n');
        }
        text
    }

    /// 把设置文件路径解析为项目内的绝对路径。
    #[must_use]
    pub fn path_in(root: &Path) -> PathBuf {
        root.join(SETTINGS_FILE)
    }

    /// 读取工程设置；文件不存在返回 `None`（调用方使用默认值），读取失败返回错误。
    pub fn load_from(root: &Path) -> std::io::Result<Option<Self>> {
        let path = Self::path_in(root);
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Some(Self::from_toml(&text))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// 写回工程设置：保留文件其余内容（注释/未知键/其他段），只改写 `[audio]` 段。
    pub fn save_to(&self, root: &Path) -> std::io::Result<()> {
        let path = Self::path_in(root);
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        std::fs::write(&path, upsert_audio_section(&existing, self))
    }
}

/// 音量的稳定文本形式：整数保留一位小数（`1.0`），小数最多三位并去掉尾零
/// （`0.25`、`0.125`）。
#[must_use]
pub fn fmt_volume(volume: f32) -> String {
    fmt_compact(volume, 1.0)
}

/// 死区的稳定文本形式（M20-08；与 [`fmt_volume`] 同一套最短化规则）。
#[must_use]
pub fn fmt_deadzone(deadzone: f32) -> String {
    fmt_compact(deadzone, crate::core::input::MAX_DEADZONE)
}

/// 0..=max 区间内浮点的稳定文本（保留 `.0`，最多三位小数并去尾零）。
fn fmt_compact(value: f32, max: f32) -> String {
    let mut text = format!("{:.3}", value.clamp(0.0, max));
    if text.contains('.') {
        while text.ends_with('0') && !text.ends_with(".0") {
            text.pop();
        }
    }
    text
}

/// `[audio]` 段内替换/补齐四个音量键；其余行（含其他段与注释）原样保留。
///
/// * 已有 `key = ...`（首次出现）→ 替换该行的值；
/// * `[audio]` 段存在但缺键 → 追加到该段末尾；
/// * 完全没有 `[audio]` 段 → 在文件末尾追加新段（保留原有末尾换行语义）。
#[must_use]
pub fn upsert_audio_section(existing: &str, settings: &ProjectSettings) -> String {
    let had_newline = existing.ends_with('\n');
    let mut lines: Vec<String> = existing.lines().map(str::to_owned).collect();
    let mut section = String::new();
    // 段内已出现的键 → 行号。
    let mut replaced: BTreeMap<AudioBus, usize> = BTreeMap::new();
    let mut audio_end: Option<usize> = None;
    let mut has_audio_section = false;
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim().trim_start_matches('\u{feff}').trim();
        if let Some(name) = trimmed.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            section = name.trim().to_ascii_lowercase();
            if section == AUDIO_SECTION {
                has_audio_section = true;
                audio_end = Some(index);
            }
            continue;
        }
        if section != AUDIO_SECTION {
            continue;
        }
        // 段内任意非表头行（含注释/空行）都是「段尾」候选：追加键插在下一个表头
        // 之前，而不是插到后面别的段之后（`[input]` 等扩展段必须保持原位）。
        audio_end = Some(index);
        let Some((key, _)) = trimmed.split_once('=') else {
            continue;
        };
        let Some(bus) = AudioBus::from_key(key.trim()) else {
            continue;
        };
        replaced.entry(bus).or_insert(index);
    }
    for (bus, index) in &replaced {
        lines[*index] = format!("{} = {}", bus.key(), fmt_volume(settings.volume(*bus)));
    }
    let missing: Vec<AudioBus> = AudioBus::ALL
        .into_iter()
        .filter(|bus| !replaced.contains_key(bus))
        .collect();
    if !has_audio_section {
        if !lines.is_empty() && lines.last().is_some_and(|line| !line.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push(format!("[{AUDIO_SECTION}]"));
        for bus in missing {
            lines.push(format!(
                "{} = {}",
                bus.key(),
                fmt_volume(settings.volume(bus))
            ));
        }
    } else if !missing.is_empty() {
        let insert_at = audio_end.map_or(lines.len(), |index| index + 1);
        for (offset, bus) in missing.into_iter().enumerate() {
            lines.insert(
                insert_at + offset,
                format!("{} = {}", bus.key(), fmt_volume(settings.volume(bus))),
            );
        }
    }
    let mut result = lines.join("\n");
    if (had_newline || !result.is_empty()) && !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

// ---------------------------------------------------------------------------
// M20-07：[input] 段（命名动作映射）
// ---------------------------------------------------------------------------

/// `[input]` 段的解析结果。
///
/// 「段/键缺失」与「键存在但内容非法」是两回事：前者调用方保留**当前**映射
/// （例如玩家在游戏里刚改过键，而工程设置文件里还没写过 `[input]`），后者才
/// 回退默认映射并记录诊断。
#[derive(Debug, Clone, PartialEq)]
pub struct InputSectionParse {
    /// 解析出的映射；键缺失或内容非法时为 `None`。
    pub map: Option<ActionMap>,
    /// 回退/缺失原因（可直接写进诊断）：`missing_key`、`invalid_escape`、
    /// 版本头不符或绑定文本非法的原始错误文本（附 `settings.toml:<行号>`）。
    pub reason: Option<String>,
    /// **v1 → v2 迁移诊断**（M22/W1）：文件里写的是 `rustlua-actions-v1` 时，
    /// 每一条有损换算（`key:+` 只能落到 `key:equal`）各一行中文说明；读到 v2 文件
    /// 时为空。调用方（`input.load_settings()` / 编辑器面板）应把它显示给玩家，
    /// 而不是让改键静默变形。
    pub migration_notes: Vec<String>,
}

impl InputSectionParse {
    /// 是否找到了可用的 `[input].map`。
    #[must_use]
    pub const fn is_present(&self) -> bool {
        self.map.is_some()
    }
}

/// `[input].map` 的解析结果：要么是「映射 + 迁移诊断」，要么是**可定位的原因**。
///
/// 抽成别名而不是内联元组，是因为它同时带着行号出现在下面 `found` 的 `Option` 里
/// （两层嵌套后类型注记会难读）。
type InputMapParse = Result<(ActionMap, Vec<String>), String>;

/// 从设置文本解析 `[input].map`；非法内容返回原因而不是默认值。
///
/// M22/W1：同时接受 `rustlua-actions-v1`（旧格式）与 `v2`（当前格式）版本头，
/// v1 会被迁移成物理键并把**有损换算**记进 [`crate::core::settings::InputSectionParse::migration_notes`]。
#[must_use]
pub fn parse_input_section(text: &str) -> InputSectionParse {
    let mut section = String::new();
    let mut found: Option<(usize, InputMapParse)> = None;
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim().trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            section = name.trim().to_ascii_lowercase();
            continue;
        }
        if section != INPUT_SECTION {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim().to_ascii_lowercase() != INPUT_MAP_KEY || found.is_some() {
            // 未知键忽略（宽松）；同名键只取首次出现（与 [audio] 一致）。
            continue;
        }
        let parsed = unescape_toml_string(value.trim())
            .ok_or_else(|| "invalid_escape".to_owned())
            .and_then(|map_text| {
                ActionMap::from_text_with_notes(&map_text).map_err(|error| error.to_string())
            });
        found = Some((index + 1, parsed));
    }
    match found {
        Some((_, Ok((map, migration_notes)))) => InputSectionParse {
            map: Some(map),
            reason: None,
            migration_notes,
        },
        Some((line, Err(reason))) => InputSectionParse {
            map: None,
            reason: Some(format!("settings.toml:{line}: {reason}")),
            migration_notes: Vec::new(),
        },
        None => InputSectionParse {
            map: None,
            reason: Some(format!("missing_key: {INPUT_SECTION}.{INPUT_MAP_KEY}")),
            migration_notes: Vec::new(),
        },
    }
}

/// 便捷封装：读取 `[input].map`，缺失/非法时回退 [`ActionMap::with_defaults`]，
/// 并用默认值**只补齐缺失动作**（用户重绑定不被覆盖）。
///
/// 返回 `(映射, 诊断)`：诊断仅在需要提醒时出现（文件缺失不算——首次运行的
/// 正常路径；段/键缺失、内容非法都给出可定位的一行说明）。
#[must_use]
pub fn load_input_section(root: &Path) -> (ActionMap, Option<String>) {
    let path = ProjectSettings::path_in(root);
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // 首次运行：文件都没有，用完整默认映射，无需打扰用户。
            return (ActionMap::with_defaults(), None);
        }
        Err(error) => {
            return (
                ActionMap::with_defaults(),
                Some(format!("{}: read failed: {error}", path.display())),
            );
        }
    };
    let parsed = parse_input_section(&existing);
    match parsed.map {
        Some(map) => {
            // 文件里没有的动作用默认绑定补齐（新增动作在旧设置文件上仍可玩）。
            let mut map = map;
            map.merge_missing(&ActionMap::with_defaults());
            (map, None)
        }
        None => {
            let reason = parsed
                .reason
                .unwrap_or_else(|| "invalid_input_section".to_owned());
            (
                ActionMap::with_defaults(),
                Some(format!(
                    "{}: {reason}; reverting to defaults",
                    path.display()
                )),
            )
        }
    }
}

/// 按 TOML 基本字符串转义 `[input].map` 的值（单行，`\n` 代表换行）。
#[must_use]
pub fn escape_toml_string(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len() + 2);
    for character in text.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// 反转义 [`escape_toml_string`] 的输出；未知转义序列返回 `None`（调用方回退默认）。
fn unescape_toml_string(value: &str) -> Option<String> {
    let value = value.trim();
    let value = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(value);
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            result.push(character);
            continue;
        }
        match chars.next()? {
            'n' => result.push('\n'),
            'r' => result.push('\r'),
            't' => result.push('\t'),
            '"' => result.push('"'),
            '\\' => result.push('\\'),
            _ => return None,
        }
    }
    Some(result)
}

/// 保序写回 `[input].map`：其余行（注释、`[audio]` 段、未知键）原样保留。
///
/// * 已有 `map = ...`（首次出现，`[input]` 段内）→ 替换该行；
/// * `[input]` 段存在但缺 `map` → 追加到该段末尾（下一个表头之前）；
/// * 没有 `[input]` 段 → 在文件末尾追加新段。
///
/// M20-08：死区键**不在这里写**（`[input]` 段里已有的 `stick_deadzone` /
/// `trigger_deadzone` 行原样保留），因此游戏内 `input.save_settings()` 不会
/// 抹掉编辑器配置的死区。
#[must_use]
pub fn upsert_input_section(existing: &str, map: &ActionMap) -> String {
    upsert_input_settings(existing, map, None)
}

/// 保序写回整个 `[input]` 段（映射 + 可选死区，M20-08）。
///
/// `deadzone == None` 时只写 `map`（游戏内保存路径）；`Some` 时额外补齐/替换
/// `stick_deadzone` / `trigger_deadzone`（编辑器「项目设置 → 输入」路径）。
#[must_use]
pub fn upsert_input_settings(
    existing: &str,
    map: &ActionMap,
    deadzone: Option<GamepadDeadzone>,
) -> String {
    let mut entries: Vec<(&str, String)> = vec![(
        INPUT_MAP_KEY,
        format!("\"{}\"", escape_toml_string(&map.to_text())),
    )];
    if let Some(deadzone) = deadzone {
        entries.push((INPUT_STICK_DEADZONE_KEY, fmt_deadzone(deadzone.stick)));
        entries.push((INPUT_TRIGGER_DEADZONE_KEY, fmt_deadzone(deadzone.trigger)));
    }
    upsert_input_entries(existing, &entries)
}

/// 通用的 `[input]` 段保序 upsert：按 `entries` 顺序替换已有键行，缺失键插入
/// 段末（下一个表头之前）。
fn upsert_input_entries(existing: &str, entries: &[(&str, String)]) -> String {
    let had_newline = existing.ends_with('\n');
    let mut lines: Vec<String> = existing.lines().map(str::to_owned).collect();
    let mut section = String::new();
    let mut replaced: Vec<Option<usize>> = vec![None; entries.len()];
    let mut input_end: Option<usize> = None;
    let mut has_input_section = false;
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim().trim_start_matches('\u{feff}').trim();
        if let Some(name) = trimmed
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            section = name.trim().to_ascii_lowercase();
            if section == INPUT_SECTION {
                has_input_section = true;
                input_end = Some(index);
            }
            continue;
        }
        if section != INPUT_SECTION {
            continue;
        }
        // 段内任意非表头行（含注释/空行）都是「段尾」候选：追加键插在下一个表头
        // 之前（`[audio]` 等后续段必须保持原位）。
        input_end = Some(index);
        let Some((key, _)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        if let Some(slot) = entries
            .iter()
            .position(|(name, _)| name.eq_ignore_ascii_case(&key))
            && replaced[slot].is_none()
        {
            replaced[slot] = Some(index);
        }
    }
    let mut missing: Vec<usize> = Vec::new();
    for (slot, (name, value)) in entries.iter().enumerate() {
        match replaced[slot] {
            Some(index) => lines[index] = format!("{name} = {value}"),
            None => missing.push(slot),
        }
    }
    if !has_input_section {
        if !lines.is_empty() && lines.last().is_some_and(|line| !line.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push(format!("[{INPUT_SECTION}]"));
        for slot in missing {
            let (name, value) = &entries[slot];
            lines.push(format!("{name} = {value}"));
        }
    } else if !missing.is_empty() {
        let insert_at = input_end.map_or(lines.len(), |index| index + 1);
        for (offset, slot) in missing.into_iter().enumerate() {
            let (name, value) = &entries[slot];
            lines.insert(insert_at + offset, format!("{name} = {value}"));
        }
    }
    let mut result = lines.join("\n");
    if (had_newline || !result.is_empty()) && !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// 把 `[input]` 段写入 `<root>/settings.toml`，保留文件其余内容。
pub fn save_input_section(root: &Path, map: &ActionMap) -> std::io::Result<PathBuf> {
    let path = ProjectSettings::path_in(root);
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    std::fs::write(&path, upsert_input_section(&existing, map))?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// M20-08：[input] 段的摇杆/扳机死区
// ---------------------------------------------------------------------------

/// 从设置文本解析 `[input]` 段的死区。与 `[audio]` 一样**宽松**：段缺失、键缺失、
/// 非法数值都保留默认值（0.15 / 0.05），段外同名键不生效，同名键只取首次出现。
#[must_use]
pub fn parse_deadzone_section(text: &str) -> GamepadDeadzone {
    let mut stick: Option<f32> = None;
    let mut trigger: Option<f32> = None;
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.trim().trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            section = name.trim().to_ascii_lowercase();
            continue;
        }
        if section != INPUT_SECTION {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let parsed = value.trim().trim_matches('"').parse::<f32>().ok();
        match key.trim().to_ascii_lowercase().as_str() {
            INPUT_STICK_DEADZONE_KEY if stick.is_none() => stick = parsed,
            INPUT_TRIGGER_DEADZONE_KEY if trigger.is_none() => trigger = parsed,
            _ => {}
        }
    }
    // 构造时统一钳制：非有限值/超界值不会产生非法曲线。
    GamepadDeadzone::new(
        stick.unwrap_or(GamepadDeadzone::DEFAULT.stick),
        trigger.unwrap_or(GamepadDeadzone::DEFAULT.trigger),
    )
}

/// 读取 `<root>/settings.toml` 的死区配置；文件缺失 → 默认值 + 无诊断（首次运行
/// 的正常路径），读取失败 → 默认值 + 可读原因。
#[must_use]
pub fn load_deadzone_section(root: &Path) -> (GamepadDeadzone, Option<String>) {
    let path = ProjectSettings::path_in(root);
    match std::fs::read_to_string(&path) {
        Ok(text) => (parse_deadzone_section(&text), None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            (GamepadDeadzone::DEFAULT, None)
        }
        Err(error) => (
            GamepadDeadzone::DEFAULT,
            Some(format!("{}: read failed: {error}", path.display())),
        ),
    }
}

/// 把映射 + 死区写回 `<root>/settings.toml`（保序：其余段/注释/未知键原样保留）。
pub fn save_input_settings(
    root: &Path,
    map: &ActionMap,
    deadzone: GamepadDeadzone,
) -> std::io::Result<PathBuf> {
    let path = ProjectSettings::path_in(root);
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    std::fs::write(&path, upsert_input_settings(&existing, map, Some(deadzone)))?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// M20-09：[input] 段的菜单导航手感（摇杆阈值与按住重复节奏）
// ---------------------------------------------------------------------------

/// 菜单导航手感（左摇杆推杆阈值 + 按住重复节奏）。
///
/// 与死区一样属于**玩家偏好**：只影响面板选中的滚动节奏，不影响玩法移动。
///
/// **M22/W3 起重复节奏用毫秒**（原 M20-09 是帧数，导致手感随刷新率变化：144Hz 的
/// 连发速度是 60Hz 的 2.4 倍）。旧的 `nav_delay_frames` / `nav_repeat_frames` 仍可读
/// （按 60Hz 换算），写入统一用 `nav_delay_ms` / `nav_repeat_ms`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavSettings {
    /// 摇杆推杆阈值（0..=1）：低于它不算导航输入（引擎轴判定阈值同源）。
    pub threshold: f32,
    /// 首次重复前的按住时长（毫秒，上限 [`Self::MAX_MS`]）。
    pub delay_ms: u32,
    /// 连发间隔（毫秒，下限 [`Self::MIN_REPEAT_MS`]，上限 [`Self::MAX_MS`]）。
    pub repeat_ms: u32,
}

impl Default for NavSettings {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl NavSettings {
    /// 默认手感：阈值 0.5、延迟 350ms、间隔 90ms。
    ///
    /// 与 M20-09 的「16 帧 / 8 帧 @60Hz」几乎一致（267ms / 133ms → 350ms / 90ms，
    /// 首次延迟略长、连发略快），这样键盘/手柄在任意帧率下手感一致。
    pub const DEFAULT: Self = Self {
        threshold: 0.5,
        delay_ms: 350,
        repeat_ms: 90,
    };

    /// 时长上限：再长也没有实用意义，同时防脏数据把导航变成「永不重复」。
    pub const MAX_MS: u32 = 5000;

    /// 连发间隔下限（约一个 60Hz 逻辑帧，防止退化成每步连发）。
    pub const MIN_REPEAT_MS: u32 = 16;

    /// 旧「帧」键换算成毫秒所用的固定步长（60Hz，与引擎默认固定步一致）。
    pub const LEGACY_FRAME_MS: f32 = 1000.0 / 60.0;

    /// 唯一构造入口：非有限/超界值统一钳制（阈值 0..=1、延迟 ≤ 上限、间隔 ≥ 下限）。
    #[must_use]
    pub fn new(threshold: f32, delay_ms: u32, repeat_ms: u32) -> Self {
        let threshold = if threshold.is_finite() {
            threshold.clamp(0.0, 1.0)
        } else {
            Self::DEFAULT.threshold
        };
        Self {
            threshold,
            delay_ms: delay_ms.min(Self::MAX_MS),
            repeat_ms: repeat_ms.clamp(Self::MIN_REPEAT_MS, Self::MAX_MS),
        }
    }

    /// 旧帧数 → 毫秒（兼容读取用）。非有限/负值按 0 处理（与 M20-09 的
    /// `v.max(0.0) as u32` 一致），上限由 [`Self::new`] 统一钳制。
    #[must_use]
    pub fn frames_to_ms(frames: f32) -> u32 {
        (frames.max(0.0) * Self::LEGACY_FRAME_MS).round() as u32
    }

    /// 是否与默认手感一致（编辑器/诊断用）。
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::DEFAULT
    }
}

/// 从设置文本解析 `[input]` 段的导航手感。宽松语义与死区一致：段缺失、键缺失、
/// 非法数值都保留默认值，段外同名键不生效，同名键只取首次出现。
///
/// 毫秒键优先于旧的帧键：两者同时存在时用毫秒键。
#[must_use]
pub fn parse_nav_section(text: &str) -> NavSettings {
    let mut threshold: Option<f32> = None;
    let mut delay_ms: Option<u32> = None;
    let mut repeat_ms: Option<u32> = None;
    let mut delay_frames: Option<u32> = None;
    let mut repeat_frames: Option<u32> = None;
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.trim().trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            section = name.trim().to_ascii_lowercase();
            continue;
        }
        if section != INPUT_SECTION {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        // 允许 `16` 与 `16.0` 两种写法：先试整数，再按浮点取整。
        let parse_num = |value: &str| -> Option<u32> {
            value
                .parse::<u32>()
                .ok()
                .or_else(|| value.parse::<f32>().ok().map(|v| v.max(0.0) as u32))
        };
        match key.trim().to_ascii_lowercase().as_str() {
            INPUT_NAV_THRESHOLD_KEY if threshold.is_none() => {
                threshold = value.parse::<f32>().ok();
            }
            INPUT_NAV_DELAY_MS_KEY if delay_ms.is_none() => {
                delay_ms = parse_num(value);
            }
            INPUT_NAV_REPEAT_MS_KEY if repeat_ms.is_none() => {
                repeat_ms = parse_num(value);
            }
            INPUT_NAV_DELAY_FRAMES_KEY if delay_frames.is_none() => {
                delay_frames = parse_num(value);
            }
            INPUT_NAV_REPEAT_FRAMES_KEY if repeat_frames.is_none() => {
                repeat_frames = parse_num(value);
            }
            _ => {}
        }
    }
    // 毫秒键优先；只有旧的帧键时按 60Hz 换算（留诊断由调用方决定，这里只做数值）。
    let delay =
        delay_ms.or_else(|| delay_frames.map(|frames| NavSettings::frames_to_ms(frames as f32)));
    let repeat =
        repeat_ms.or_else(|| repeat_frames.map(|frames| NavSettings::frames_to_ms(frames as f32)));
    NavSettings::new(
        threshold.unwrap_or(NavSettings::DEFAULT.threshold),
        delay.unwrap_or(NavSettings::DEFAULT.delay_ms),
        repeat.unwrap_or(NavSettings::DEFAULT.repeat_ms),
    )
}

/// 读取 `<root>/settings.toml` 的导航手感；文件缺失 → 默认值 + 无诊断，
/// 读取失败 → 默认值 + 可读原因（与 [`load_deadzone_section`] 同语义）。
#[must_use]
pub fn load_nav_section(root: &Path) -> (NavSettings, Option<String>) {
    let path = ProjectSettings::path_in(root);
    match std::fs::read_to_string(&path) {
        Ok(text) => (parse_nav_section(&text), None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (NavSettings::DEFAULT, None),
        Err(error) => (
            NavSettings::DEFAULT,
            Some(format!("{}: read failed: {error}", path.display())),
        ),
    }
}

/// 保序写回导航手感三行（其余段/注释/未知键原样保留）。
///
/// M22/W3 起写毫秒键；文件里遗留的 `nav_delay_frames` / `nav_repeat_frames` 行不删
/// （保序读改写只替换/补齐自己管理的键），但因为毫秒键优先，它们不再生效。
#[must_use]
pub fn upsert_nav_settings(existing: &str, nav: NavSettings) -> String {
    let entries: Vec<(&str, String)> = vec![
        (INPUT_NAV_THRESHOLD_KEY, fmt_deadzone(nav.threshold)),
        (INPUT_NAV_DELAY_MS_KEY, nav.delay_ms.to_string()),
        (INPUT_NAV_REPEAT_MS_KEY, nav.repeat_ms.to_string()),
    ];
    upsert_input_entries(existing, &entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::audio::{AudioBus, AudioSystem};
    use crate::core::input::GamepadDeadzone;
    use crate::core::input::{ACTION_MAP_VERSION, ActionMapError, AxisDirection, Binding, KeyCode};

    #[test]
    fn defaults_are_all_full_volume() {
        let settings = ProjectSettings::new();
        for bus in AudioBus::ALL {
            assert_eq!(settings.volume(bus), 1.0, "bus {bus:?}");
        }
    }

    #[test]
    fn parses_audio_section_and_ignores_unknown_keys_and_bad_values() {
        let text = "# 注释\n[audio]\nmaster = 0.5\nbgm = 0.25\nsfx = bad\nunknown = 0.9\nui = 2.0\n[other]\nmaster = 0.1\n";
        let settings = ProjectSettings::from_toml(text);
        assert_eq!(settings.volume(AudioBus::Master), 0.5);
        assert_eq!(settings.volume(AudioBus::Bgm), 0.25);
        // 非法数值保留默认；超界钳制；段外的同名键不生效。
        assert_eq!(settings.volume(AudioBus::Sfx), 1.0);
        assert_eq!(settings.volume(AudioBus::Ui), 1.0);
    }

    #[test]
    fn upsert_replaces_in_place_and_preserves_comments_and_other_sections() {
        let existing = "# 顶部注释\n[audio]\nmaster = 1.0\n# 音量注释\nbgm = 1.0\n\n[input]\nmap = \"rustlua-actions-v1\\ninteract=key:e\"\n";
        let mut settings = ProjectSettings::new();
        settings.set_volume(AudioBus::Bgm, 0.3);
        settings.set_volume(AudioBus::Ui, 0.75);
        let out = upsert_audio_section(existing, &settings);
        assert!(out.contains("# 顶部注释"), "{out}");
        assert!(out.contains("# 音量注释"), "{out}");
        assert!(out.contains("bgm = 0.3"), "{out}");
        assert!(out.contains("ui = 0.75"), "{out}");
        // 缺失键补齐到 [audio] 段内（不是追加到文件末尾的 [input] 之后）。
        assert!(out.contains("sfx = 1.0"), "{out}");
        let audio_pos = out.find("[audio]").unwrap();
        let sfx_pos = out.find("sfx = 1.0").unwrap();
        let input_pos = out.find("[input]").unwrap();
        assert!(audio_pos < sfx_pos && sfx_pos < input_pos, "{out}");
        // 幂等：再次写回同样内容。
        let again = upsert_audio_section(&out, &settings);
        assert_eq!(out, again);
        // 往返一致。
        let parsed = ProjectSettings::from_toml(&out);
        assert_eq!(parsed.volume(AudioBus::Bgm), 0.3);
        assert_eq!(parsed.volume(AudioBus::Ui), 0.75);
        assert_eq!(parsed.volume(AudioBus::Master), 1.0);
    }

    #[test]
    fn upsert_on_empty_or_unrelated_content_appends_audio_section() {
        let settings = ProjectSettings::new();
        let out = upsert_audio_section("", &settings);
        assert_eq!(out, settings.to_toml());
        let out = upsert_audio_section("# 只有注释\n", &settings);
        assert!(out.contains("[audio]"), "{out}");
        assert!(out.contains("master = 1.0"), "{out}");
        // 无末尾换行的文件写回后仍带换行，且只在需要时补空行分隔。
        let out = upsert_audio_section("key = \"value\"", &settings);
        assert!(out.starts_with("key = \"value\"\n\n[audio]\n"), "{out}");
    }

    #[test]
    fn audio_round_trips_through_project_settings() {
        let mut audio = AudioSystem::new();
        audio.set_bus_volume(AudioBus::Master, 0.8);
        audio.set_bus_volume(AudioBus::Bgm, 0.3);
        audio.set_bus_volume(AudioBus::Sfx, 0.9);
        audio.set_bus_volume(AudioBus::Ui, 0.6);
        let settings = ProjectSettings::from_audio(&audio);

        let mut restored = AudioSystem::new();
        settings.apply_to(&mut restored);
        assert!((restored.bus_volume(AudioBus::Master) - 0.8).abs() < 1e-3);
        assert!((restored.bus_volume(AudioBus::Bgm) - 0.3).abs() < 1e-3);
        assert!((restored.bus_volume(AudioBus::Sfx) - 0.9).abs() < 1e-3);
        assert!((restored.bus_volume(AudioBus::Ui) - 0.6).abs() < 1e-3);
        // 有效音量 = 主 × 分组。
        assert!((restored.effective_volume(AudioBus::Bgm) - 0.24).abs() < 1e-3);
        assert!((restored.effective_volume(AudioBus::Master) - 0.8).abs() < 1e-3);
    }

    #[test]
    fn load_and_save_round_trip_on_disk() {
        let dir = std::env::temp_dir().join(format!(
            "rustlua-settings-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        // 缺失文件 → None（默认值）。
        assert!(ProjectSettings::load_from(&dir).expect("load").is_none());

        let mut settings = ProjectSettings::new();
        settings.set_volume(AudioBus::Bgm, 0.4);
        settings.save_to(&dir).expect("save");
        let loaded = ProjectSettings::load_from(&dir)
            .expect("load")
            .expect("file exists after save");
        assert_eq!(loaded, settings);
        // 保存不改动同名文件里的其他段（后续任务扩展 [input] 时不丢配置）。
        let path = ProjectSettings::path_in(&dir);
        let mut text = std::fs::read_to_string(&path).expect("read");
        text.push_str("\n[input]\nmap = \"keep\"\n");
        std::fs::write(&path, text).expect("write");
        settings.set_volume(AudioBus::Bgm, 0.2);
        settings.save_to(&dir).expect("save again");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("map = \"keep\""), "{text}");
        assert!(text.contains("bgm = 0.2"), "{text}");
        assert_eq!(
            ProjectSettings::load_from(&dir).expect("load").unwrap(),
            settings
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn volume_formatting_is_stable() {
        assert_eq!(fmt_volume(1.0), "1.0");
        assert_eq!(fmt_volume(0.0), "0.0");
        assert_eq!(fmt_volume(0.25), "0.25");
        assert_eq!(fmt_volume(0.125), "0.125");
        assert_eq!(fmt_volume(0.3333333), "0.333");
        assert_eq!(fmt_volume(2.0), "1.0");
        assert_eq!(fmt_volume(-1.0), "0.0");
    }

    // ---- M20-07：[input] 段 -------------------------------------------------

    /// 临时目录（进程内唯一，避免并行测试互相覆盖）。
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rustlua-input-settings-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn input_section_escapes_and_round_trips_in_text() {
        let mut map = ActionMap::with_defaults();
        assert!(map.unbind("interact", Binding::Key(KeyCode::KeyE)));
        map.bind("interact", Binding::Key(KeyCode::KeyF))
            .expect("rebind");
        // 完整映射（含版本头）→ 单行转义 → 解析回相等映射。
        // M22/W1：写出的是 **v2** 物理键文本（`key:keyf`），不是 v1 的字符形式。
        let escaped = escape_toml_string(&map.to_text());
        assert!(escaped.starts_with(ACTION_MAP_VERSION), "{escaped}");
        assert!(!escaped.contains('\n'), "{escaped}");
        assert!(escaped.contains("interact=key:keyf"), "{escaped}");
        assert!(
            !escaped.contains("interact=key:f,"),
            "不得再写出 v1 字符形式"
        );
        let text = format!("# 输入设置注释\n[{INPUT_SECTION}]\n{INPUT_MAP_KEY} = \"{escaped}\"\n");
        let parsed = parse_input_section(&text);
        assert_eq!(parsed.reason, None);
        assert_eq!(parsed.map.as_ref(), Some(&map));
        assert!(
            parsed.migration_notes.is_empty(),
            "v2 文件不应产生迁移说明：{:?}",
            parsed.migration_notes
        );
        // 反斜杠/引号也按 TOML 基本字符串规则转义，不破坏后续段。
        let tricky = "rustlua-actions-v1\ninteract=key:\\\\\nquote=key:\"";
        assert_eq!(
            unescape_toml_string(&escape_toml_string(tricky)).as_deref(),
            Some(tricky)
        );
    }

    /// M22/W1：读到 **v1** 的 `[input].map` 时迁移成物理键，并给出可显示的诊断。
    #[test]
    fn input_section_migrates_v1_keys_and_reports_lossy_conversions() {
        let text = format!(
            "[{INPUT_SECTION}]\n{INPUT_MAP_KEY} = \"rustlua-actions-v1\\n\
             move_up=key:w\\nui_up=key:other:1048577\\nvolume_up=key:+\\nslot_1=key:1\"\n"
        );
        let parsed = parse_input_section(&text);
        let map = parsed.map.expect("v1 必须能读入");
        assert_eq!(map.bindings("move_up"), [Binding::Key(KeyCode::KeyW)]);
        assert_eq!(map.bindings("ui_up"), [Binding::Key(KeyCode::ArrowUp)]);
        assert_eq!(map.bindings("slot_1"), [Binding::Key(KeyCode::Digit1)]);
        // `+` 是带 Shift 的字符写法 → 只能落回 `=` 那个物理键，必须记一条诊断。
        assert_eq!(map.bindings("volume_up"), [Binding::Key(KeyCode::Equal)]);
        assert!(
            parsed
                .migration_notes
                .iter()
                .any(|note| note.contains("key:+") && note.contains("key:equal")),
            "有损换算必须给出诊断：{:?}",
            parsed.migration_notes
        );
        // 无有损换算的 v1 文件也要提醒「已升级」（读起来不同）。
        let clean = parse_input_section(&format!(
            "[{INPUT_SECTION}]\n{INPUT_MAP_KEY} = \"rustlua-actions-v1\\nmove_up=key:w\"\n"
        ));
        assert_eq!(
            clean.migration_notes.len(),
            1,
            "{:?}",
            clean.migration_notes
        );
        assert!(clean.migration_notes[0].contains("v1"));
    }

    #[test]
    fn input_section_parses_leniently_and_reports_invalid_content() {
        // 缺键 → 不是错误，调用方保留当前映射（由 Lua 层决定提示）。
        let missing = parse_input_section("[audio]\nmaster = 1.0\n");
        assert!(!missing.is_present());
        assert_eq!(missing.reason.as_deref(), Some("missing_key: input.map"));
        // 未知键/非法转义忽略；注释与 # 行不影响解析。
        let unknown = parse_input_section("[input]\nvolume = 3\nmap = \"rustlua-actions-v1\\n\"\n");
        assert!(unknown.is_present(), "{unknown:?}");
        assert_eq!(unknown.map.as_ref().map(ActionMap::is_empty), Some(true));
        let bad_escape = parse_input_section("[input]\nmap = \"a\\qb\"\n");
        assert!(!bad_escape.is_present());
        assert_eq!(
            bad_escape.reason.as_deref(),
            Some("settings.toml:2: invalid_escape")
        );
        // 版本头不符 / 绑定非法 → 明确原因（含文件与行号），不 panic、不静默。
        let bad_version =
            parse_input_section("[input]\nmap = \"rustlua-actions-v0\\ninteract=key:e\"\n");
        assert!(!bad_version.is_present());
        let reason = bad_version.reason.unwrap_or_default();
        assert!(reason.contains("settings.toml:2"), "{reason}");
        assert!(reason.contains("rustlua-actions-v0"), "{reason}");
        let bad_binding =
            parse_input_section("[input]\nmap = \"rustlua-actions-v1\\ninteract=joystick:1\"\n");
        assert!(!bad_binding.is_present());
        assert!(
            bad_binding
                .reason
                .unwrap_or_default()
                .contains("joystick:1"),
            "非法绑定必须给出可定位原因"
        );
    }

    #[test]
    fn upsert_input_section_preserves_audio_comments_and_other_lines() {
        let existing = "# 顶部注释\n[audio]\nmaster = 0.5\n# 音量注释\nbgm = 0.25\nsfx = 1.0\nui = 1.0\n\n# 输入段注释\n[input]\n# 绑定表\nmap = \"rustlua-actions-v1\\ninteract=key:e\"\n\n[other]\nkey = \"keep\"\n";
        let mut map = ActionMap::with_defaults();
        assert!(map.unbind("interact", Binding::Key(KeyCode::KeyE)));
        map.bind("interact", Binding::Key(KeyCode::KeyF))
            .expect("rebind");
        let out = upsert_input_section(existing, &map);
        // `[audio]` 段、注释、未知段与未知键全部原样保留。
        assert!(out.contains("# 顶部注释"), "{out}");
        assert!(out.contains("# 音量注释"), "{out}");
        assert!(out.contains("# 输入段注释"), "{out}");
        assert!(out.contains("master = 0.5"), "{out}");
        assert!(out.contains("[other]"), "{out}");
        assert!(out.contains("key = \"keep\""), "{out}");
        // 新增一行 `# 绑定表` 注释本身也没被吃掉。
        assert!(out.contains("# 绑定表"), "{out}");
        assert!(out.contains("interact=key:keyf"), "{out}");
        // `map` 只有一处，且仍在 [input] 段内、[other] 之前。
        assert_eq!(out.matches("map = ").count(), 1, "{out}");
        let input_pos = out.find("[input]").expect("[input]");
        assert!(input_pos < out.find("map = ").expect("map"), "{out}");
        assert!(
            out.find("map = ").expect("map") < out.find("[other]").expect("[other]"),
            "{out}"
        );
        // 幂等 + `[audio]` 键序不变。
        assert_eq!(out, upsert_input_section(&out, &map));
        assert!(
            out.find("master =").unwrap() < out.find("bgm =").unwrap(),
            "{out}"
        );
        // 解析回的映射与内存一致。
        assert_eq!(parse_input_section(&out).map, Some(map));
    }

    #[test]
    fn upsert_input_section_appends_missing_section_and_key() {
        let map = ActionMap::with_defaults();
        // 完全没有 [input] 段 → 末尾追加新段。
        let out = upsert_input_section("[audio]\nmaster = 1.0\n", &map);
        assert!(out.contains("[audio]\nmaster = 1.0\n"), "{out}");
        assert!(out.contains("\n[input]\nmap = \""), "{out}");
        assert_eq!(parse_input_section(&out).map, Some(map.clone()));
        // [input] 段存在但没有 map 键 → 追加到段内（不是文件末尾的 [other] 之后）。
        let out = upsert_input_section("[input]\n# 注释\n\n[other]\nkey = 1\n", &map);
        let input_pos = out.find("[input]").unwrap();
        let map_pos = out.find("map = ").unwrap();
        let other_pos = out.find("[other]").unwrap();
        assert!(input_pos < map_pos && map_pos < other_pos, "{out}");
        // 空文件 → 单段文本，往返一致。
        let out = upsert_input_section("", &map);
        assert!(out.starts_with("[input]\nmap = \""), "{out}");
        assert_eq!(parse_input_section(&out).map, Some(map));
    }

    #[test]
    fn input_section_file_round_trip_keeps_audio_section_and_falls_back_with_diagnostic() {
        let dir = temp_dir("roundtrip");
        // 缺失文件 → 默认映射 + 无诊断（首次运行的正常路径）。
        assert_eq!(load_input_section(&dir), (ActionMap::with_defaults(), None));
        // 写盘：保留既有 [audio] 段与注释。
        let path = ProjectSettings::path_in(&dir);
        std::fs::write(
            &path,
            "# 玩家设置\n[audio]\nmaster = 0.4\nbgm = 1.0\nsfx = 1.0\nui = 1.0\n",
        )
        .expect("seed settings");
        let mut map = ActionMap::with_defaults();
        assert!(map.unbind("interact", Binding::Key(KeyCode::KeyE)));
        map.bind("interact", Binding::Key(KeyCode::KeyF))
            .expect("rebind");
        map.bind(
            "menu_cancel",
            Binding::GamepadAxis(1, AxisDirection::Negative),
        )
        .expect("pad axis binding");
        save_input_section(&dir, &map).expect("save input section");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("# 玩家设置"), "{text}");
        assert!(text.contains("master = 0.4"), "{text}");
        assert_eq!(load_input_section(&dir), (map.clone(), None));
        // 再写一次音频段：不覆盖 [input]，也不丢 [audio]。
        let settings = ProjectSettings::from_toml(&text);
        assert_eq!(settings.volume(AudioBus::Master), 0.4);
        settings.save_to(&dir).expect("save audio");
        let text = std::fs::read_to_string(&path).expect("read");
        assert_eq!(load_input_section(&dir), (map.clone(), None));
        assert!(text.contains("[input]"), "{text}");
        // 版本头不符 → 回退默认 + 可定位诊断，且不 panic。
        std::fs::write(
            &path,
            "[input]\nmap = \"rustlua-actions-v0\\ninteract=key:q\"\n",
        )
        .expect("write bad version");
        let (loaded, diagnostic) = load_input_section(&dir);
        assert_eq!(loaded, ActionMap::with_defaults());
        let diagnostic = diagnostic.expect("版本不符必须给诊断");
        assert!(diagnostic.contains("settings.toml"), "{diagnostic}");
        assert!(diagnostic.contains("rustlua-actions-v0"), "{diagnostic}");
        assert!(diagnostic.contains("reverting to defaults"), "{diagnostic}");
        // 部分映射 → 默认值补齐缺失动作，用户重绑定不被覆盖。
        std::fs::write(
            &path,
            "[input]\nmap = \"rustlua-actions-v1\\ninteract=key:f\"\n",
        )
        .expect("write partial");
        let (loaded, diagnostic) = load_input_section(&dir);
        assert_eq!(diagnostic, None);
        assert_eq!(loaded.bindings("interact"), [Binding::Key(KeyCode::KeyF)]);
        assert_eq!(loaded.bindings("move_up"), [Binding::Key(KeyCode::KeyW)]);
        assert_eq!(loaded.bindings("pause").len(), 1);
        assert_eq!(
            ActionMap::with_defaults().bindings("interact"),
            [Binding::Key(KeyCode::KeyE)],
            "回退/补齐不得改动默认表本身"
        );
        // 非法绑定文本 → 同样回退默认 + 原因（错误类型可定位）。
        std::fs::write(
            &path,
            "[input]\nmap = \"rustlua-actions-v1\\ninteract=joystick:1\"\n",
        )
        .expect("write bad binding");
        let (loaded, diagnostic) = load_input_section(&dir);
        assert_eq!(loaded, ActionMap::with_defaults());
        assert!(
            diagnostic.unwrap_or_default().contains("joystick:1"),
            "非法绑定必须给可定位原因"
        );
        assert!(matches!(
            ActionMap::from_text("rustlua-actions-v1\ninteract=joystick:1"),
            Err(ActionMapError::InvalidBinding(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- M20-08：[input] 段的死区配置 --------------------------------------

    #[test]
    fn deadzone_section_defaults_and_parses_leniently() {
        // 段/键缺失、非法数值、段外同名键 → 全部保留默认（0.15 / 0.05）。
        assert_eq!(parse_deadzone_section(""), GamepadDeadzone::new(0.15, 0.05));
        assert_eq!(
            parse_deadzone_section("[audio]\nstick_deadzone = 0.9\n"),
            GamepadDeadzone::DEFAULT,
            "段外同名键不生效"
        );
        let parsed = parse_deadzone_section(
            "# 输入\n[input]\nmap = \"rustlua-actions-v1\\n\"\nstick_deadzone = bad\ntrigger_deadzone = 0.2\n",
        );
        assert_eq!(parsed.stick, 0.15, "非法数值保留默认");
        assert!((parsed.trigger - 0.2).abs() < 1e-6);
        // 超界/负值被钳制；同名键只取首次出现。
        let clamped = parse_deadzone_section(
            "[input]\nstick_deadzone = 5.0\ntrigger_deadzone = -1.0\nstick_deadzone = 0.1\n",
        );
        assert_eq!(clamped.stick, crate::core::input::MAX_DEADZONE);
        assert_eq!(clamped.trigger, 0.0);
        // 文本格式化稳定（整数保留 .0，小数去尾零）。
        assert_eq!(fmt_deadzone(0.15), "0.15");
        assert_eq!(fmt_deadzone(0.05), "0.05");
        assert_eq!(fmt_deadzone(0.2), "0.2");
        assert_eq!(fmt_deadzone(0.0), "0.0");
    }

    #[test]
    fn deadzone_round_trips_through_settings_file_and_keeps_other_sections() {
        let dir = temp_dir("deadzone");
        let path = ProjectSettings::path_in(&dir);
        std::fs::write(
            &path,
            "# 玩家设置\n[audio]\nmaster = 0.4\n\n# 输入段注释\n[input]\n# 绑定表\nmap = \"rustlua-actions-v1\\ninteract=key:e\"\n",
        )
        .expect("seed settings");
        let mut map = ActionMap::with_defaults();
        assert!(map.unbind("interact", Binding::Key(KeyCode::KeyE)));
        map.bind("interact", Binding::Key(KeyCode::KeyF))
            .expect("rebind");
        let deadzone = GamepadDeadzone::new(0.3, 0.12);
        save_input_settings(&dir, &map, deadzone).expect("save input settings");

        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("stick_deadzone = 0.3"), "{text}");
        assert!(text.contains("trigger_deadzone = 0.12"), "{text}");
        assert!(text.contains("master = 0.4"), "{text}");
        assert!(text.contains("# 输入段注释"), "{text}");
        assert!(text.contains("# 绑定表"), "{text}");
        // 定位：死区键在 [input] 段内（`map` 之后），不是文件末尾的游离键。
        let map_pos = text.find("map = ").expect("map");
        assert!(
            map_pos < text.find("stick_deadzone").expect("stick"),
            "{text}"
        );
        // 往返一致：映射与死区都能读回。
        assert_eq!(load_input_section(&dir), (map.clone(), None));
        assert_eq!(load_deadzone_section(&dir), (deadzone, None));
        // 幂等。
        assert_eq!(text, upsert_input_settings(&text, &map, Some(deadzone)));
        // 只保存映射（游戏内 `input.save_settings()` 路径）不丢死区。
        let game_saved = upsert_input_section(&text, &map);
        assert!(game_saved.contains("stick_deadzone = 0.3"), "{game_saved}");
        assert_eq!(
            load_deadzone_section(&dir),
            (deadzone, None),
            "读回的死区不因只写 map 而变化"
        );
        // 缺失文件 → 默认死区 + 无诊断；`[input]` 段整体缺失时新建段并写入死区。
        let fresh = temp_dir("deadzone-fresh");
        assert_eq!(
            load_deadzone_section(&fresh),
            (GamepadDeadzone::DEFAULT, None)
        );
        save_input_settings(&fresh, &map, GamepadDeadzone::DEFAULT).expect("save fresh");
        let fresh_text = std::fs::read_to_string(ProjectSettings::path_in(&fresh)).expect("read");
        assert!(fresh_text.starts_with("[input]\nmap = \""), "{fresh_text}");
        assert!(fresh_text.contains("stick_deadzone = 0.15"), "{fresh_text}");
        assert!(
            fresh_text.contains("trigger_deadzone = 0.05"),
            "{fresh_text}"
        );
        assert_eq!(
            load_deadzone_section(&fresh),
            (GamepadDeadzone::DEFAULT, None)
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&fresh);
    }

    /// M20-09：导航手感解析的宽松语义与钳制（缺键/脏值/段外同名键）。
    #[test]
    fn nav_section_parses_leniently_and_clamps_dirty_values() {
        // 段/键缺失 → 默认手感。
        assert_eq!(parse_nav_section(""), NavSettings::DEFAULT);
        assert_eq!(
            parse_nav_section("[audio]\nmaster = 1\n"),
            NavSettings::DEFAULT
        );
        // 段外同名键不生效。
        assert_eq!(
            parse_nav_section("[audio]\nnav_threshold = 0.9\n"),
            NavSettings::DEFAULT
        );
        // 正常解析（毫秒键）。
        assert_eq!(
            parse_nav_section(
                "[input]\nnav_threshold = 0.65\nnav_delay_ms = 300\nnav_repeat_ms = 80\n"
            ),
            NavSettings::new(0.65, 300, 80)
        );
        // 旧「帧」键兼容：按 60Hz 换算成毫秒（10 帧 ≈ 167ms、4 帧 ≈ 67ms）。
        assert_eq!(
            parse_nav_section(
                "[input]\nnav_threshold = 0.65\nnav_delay_frames = 10\nnav_repeat_frames = 4.0\n"
            ),
            NavSettings::new(0.65, 167, 67)
        );
        // 毫秒键优先于旧帧键（同一文件同时存在时）。
        assert_eq!(
            parse_nav_section(
                "[input]\nnav_delay_frames = 10\nnav_delay_ms = 250\nnav_repeat_frames = 4\nnav_repeat_ms = 60\n"
            ),
            NavSettings::new(NavSettings::DEFAULT.threshold, 250, 60)
        );
        // 脏值：非有限/超界/缺键各自回落或钳制。
        assert_eq!(
            parse_nav_section("[input]\nnav_threshold = bad\nnav_delay_frames = -3\n"),
            NavSettings::new(
                NavSettings::DEFAULT.threshold,
                0,
                NavSettings::DEFAULT.repeat_ms
            )
        );
        let clamped = parse_nav_section(
            "[input]\nnav_threshold = 2.5\nnav_delay_frames = 999\nnav_repeat_frames = 0\n",
        );
        assert_eq!(clamped.threshold, 1.0);
        assert_eq!(
            clamped.delay_ms,
            NavSettings::MAX_MS,
            "999 帧 ≈ 16650ms 应被钳制到上限"
        );
        assert_eq!(
            clamped.repeat_ms,
            NavSettings::MIN_REPEAT_MS,
            "间隔下限约一帧，否则每步连发"
        );
        // 同名键只取首次出现。
        assert_eq!(
            parse_nav_section("[input]\nnav_threshold = 0.2\nnav_threshold = 0.8\n").threshold,
            0.2
        );
        assert!(!clamped.is_default());
        assert!(NavSettings::default().is_default());
    }

    /// M20-09：导航手感保序往返，且与映射/死区/注释互不覆盖。
    #[test]
    fn nav_settings_round_trip_and_keep_other_lines() {
        let dir = temp_dir("nav");
        let path = ProjectSettings::path_in(&dir);
        std::fs::write(
            &path,
            "# 玩家设置\n[audio]\nmaster = 0.4\n\n[input]\n# 绑定表\nmap = \"rustlua-actions-v1\\ninteract=key:e\"\nstick_deadzone = 0.2\n",
        )
        .expect("seed settings");
        let nav = NavSettings::new(0.6, 300, 80);
        let written = upsert_nav_settings(&std::fs::read_to_string(&path).expect("read"), nav);
        std::fs::write(&path, &written).expect("write");
        assert!(written.contains("nav_threshold = 0.6"), "{written}");
        assert!(written.contains("nav_delay_ms = 300"), "{written}");
        assert!(written.contains("nav_repeat_ms = 80"), "{written}");
        assert!(
            !written.contains("nav_delay_frames"),
            "写入不再产生旧的帧键：{written}"
        );
        // 其余内容与既有键原样保留。
        assert!(written.contains("master = 0.4"), "{written}");
        assert!(written.contains("# 绑定表"), "{written}");
        assert!(written.contains("stick_deadzone = 0.2"), "{written}");
        assert_eq!(load_nav_section(&dir), (nav, None));
        // 幂等。
        assert_eq!(written, upsert_nav_settings(&written, nav));
        // 只写映射（游戏内保存路径）不动导航三行。
        let map = ActionMap::with_defaults();
        let game_saved = upsert_input_section(&written, &map);
        assert!(game_saved.contains("nav_threshold = 0.6"), "{game_saved}");
        assert!(game_saved.contains("nav_delay_ms = 300"), "{game_saved}");
        assert_eq!(load_nav_section(&dir), (nav, None));
        // 缺失文件 → 默认手感 + 无诊断。
        let fresh = temp_dir("nav-fresh");
        assert_eq!(load_nav_section(&fresh), (NavSettings::DEFAULT, None));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&fresh);
    }
}
