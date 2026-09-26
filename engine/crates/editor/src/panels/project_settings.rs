//! 「项目设置」独立弹出窗口：在 `项目` 菜单打开，编辑当前项目的 `project.toml`
//! 结构化字段并保存。project.toml 全工作区没有 toml crate，所有消费者都把文件当
//! 扁平 `key = value` 逐行解析（见引擎 `runtime_scene.rs` 的 `extract_project_key`），
//! 因此这里也采用同样语义的宽松解析，保存时**保序**替换已存在键并原样保留未知
//! 键与注释（`[project]` 表头无结构含义，仅作标题）。
//!
//! M20-02 追加「音频」小节：编辑 `<project>/settings.toml` 的 `[audio]` 段四路
//! 音量（主/BGM/音效/界面），与运行期 `audio` Lua API 共用引擎
//! `core::settings::ProjectSettings` 的读写实现（保序、未知段原样保留）。
//!
//! M20-08 追加「输入」小节：按动作列出绑定文本、捕获式改绑（键盘/手柄按钮/
//! 手柄轴方向）、跨动作冲突提示与保存前校验、「恢复默认」二次确认、摇杆/扳机
//! 死区配置。纯逻辑在 `panels/input_bindings.rs`（可单测），本文件只负责绘制与
//! 保存编排。

use crate::EditorApp;
use crate::panels::input_bindings::{
    self, InputBindingsUi, action_label, binding_label, describe_conflicts,
};
use crate::util::strip_verbatim_prefix;
use eframe::egui;
use engine::core::settings::ProjectSettings;
use std::collections::BTreeSet;
use std::path::Path;

/// 编辑器可管理/展示的项目键（字符串值）。
const STRING_KEYS: &[&str] = &[
    "name",
    "version",
    "engine",
    "main_script",
    "startup_scene",
    "font",
    "font_fallback",
];

/// `text_render_mode` 的可选值与中文展示名。
const TEXT_RENDER_MODES: &[(&str, &str)] =
    &[("auto", "自动"), ("vector", "矢量轮廓"), ("sdf", "距离场")];

/// 项目设置面板的缓冲状态（挂在 `EditorApp` 上；关闭窗口时仅在打开时重新读取，
/// 避免项目切换后残留旧内容）。
#[derive(Debug)]
pub(crate) struct ProjectSettingsUi {
    pub(crate) open: bool,
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) engine: String,
    pub(crate) main_script: String,
    pub(crate) startup_scene: String,
    pub(crate) font: String,
    pub(crate) font_fallback: String,
    pub(crate) font_weight: f32,
    pub(crate) text_render_mode: &'static str,
    /// 音频分组音量（M20-02，`settings.toml` 的 `[audio]` 段）；0..=1。
    pub(crate) audio_master: f32,
    pub(crate) audio_bgm: f32,
    pub(crate) audio_sfx: f32,
    pub(crate) audio_ui: f32,
    /// 输入小节（M20-08）：动作绑定、死区、捕获模式与冲突缓存。
    pub(crate) input: InputBindingsUi,
    /// 最近一次操作提示（读取失败/保存结果）。
    pub(crate) message: String,
}

impl Default for ProjectSettingsUi {
    fn default() -> Self {
        Self {
            open: false,
            name: String::new(),
            version: String::new(),
            engine: String::new(),
            main_script: String::new(),
            startup_scene: String::new(),
            font: String::new(),
            font_fallback: String::new(),
            font_weight: 400.0,
            text_render_mode: "auto",
            audio_master: 1.0,
            audio_bgm: 1.0,
            audio_sfx: 1.0,
            audio_ui: 1.0,
            input: InputBindingsUi::default(),
            message: String::new(),
        }
    }
}

/// 从 project.toml 文本中提取一个键的值（与引擎语义一致：全文件扁平扫描，忽略
/// `[project]` 表头与其它行；仅匹配 `key =` 前缀；去掉首尾引号；空值视为无）。
fn value_for(content: &str, key: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim().trim_start_matches('\u{feff}');
        if let Some(rest) = trimmed.strip_prefix(&format!("{key} =")) {
            let value = rest.trim().trim_matches('"');
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    None
}

/// 解析一段 project.toml 到 `ProjectSettingsUi` 的字符串/数值字段。
pub(crate) fn parse_into(ui: &mut ProjectSettingsUi, content: &str, root: &Path) {
    for key in STRING_KEYS {
        let value = value_for(content, key).unwrap_or_default();
        match *key {
            "name" => ui.name = value,
            "version" => ui.version = value,
            "engine" => ui.engine = value,
            "main_script" => ui.main_script = value,
            "startup_scene" => ui.startup_scene = value,
            "font" => ui.font = value,
            "font_fallback" => ui.font_fallback = value,
            _ => {}
        }
    }
    let weight = value_for(content, "font_weight")
        .and_then(|raw| raw.parse::<f32>().ok())
        .filter(|weight| weight.is_finite() && *weight > 0.0)
        .unwrap_or(400.0);
    ui.font_weight = weight;
    let text_render_mode = value_for(content, "text_render_mode")
        .unwrap_or_else(|| "auto".to_owned())
        .to_ascii_lowercase();
    ui.text_render_mode = TEXT_RENDER_MODES
        .iter()
        .find(|(key, _)| *key == text_render_mode)
        .map(|(key, _)| *key)
        .unwrap_or("auto");
    if ui.name.is_empty() {
        ui.name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
    }
}

/// 数字值的渲染风格：整数值保留 `.0`（`400.0`，与脚手架一致），小数最短化。
pub(crate) fn fmt_font_weight(weight: f32) -> String {
    if weight.fract().abs() < 1e-6 {
        format!("{weight:.1}")
    } else {
        format!("{weight}")
    }
}

/// 某行是否为 `key =`（去前导空白/BOM 后判断，注释行以 `#` 开头不会命中）。
fn line_is_key(line: &str, key: &str) -> bool {
    let trimmed = line.trim().trim_start_matches('\u{feff}');
    trimmed
        .strip_prefix(key)
        .is_some_and(|rest| rest.starts_with(" ="))
}

/// 生成渲染后的键行：字符串加引号、数字不带引号。
fn render_line(key: &str, value: &str, quoted: bool) -> String {
    if quoted {
        format!("{key} = \"{value}\"")
    } else {
        format!("{key} = {value}")
    }
}

/// 保序写回：把 `edits`（`(键, 渲染值字符串, 是否加引号)`）写入文本。已有键替换
/// 该行（首个匹配），未知键/注释/空白/顺序原样保留；缺失键追加到文件末尾。
pub(crate) fn upsert_project_toml(content: &str, edits: &[(&str, String, bool)]) -> String {
    let mut had_newline = content.ends_with('\n');
    let mut lines: Vec<String> = content.lines().map(str::to_owned).collect();
    let mut matched: BTreeSet<&str> = BTreeSet::new();
    for line in &mut lines {
        let key_of_line = edits
            .iter()
            .find(|(key, _, _)| line_is_key(line, key))
            .map(|(key, _, _)| *key);
        if let Some(key) = key_of_line
            && matched.insert(key)
        {
            let (_, value, quoted) = edits.iter().find(|(k, _, _)| *k == key).unwrap();
            *line = render_line(key, value, *quoted);
        }
    }
    for (key, value, quoted) in edits {
        if !matched.contains(key) {
            lines.push(render_line(key, value, *quoted));
            had_newline = true;
        }
    }
    let mut result = lines.join("\n");
    if had_newline && !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// 字体/文本渲染相关键旧值（用于保存后判定是否需要重建运行时与图集）。
fn font_keys_from(content: &str) -> (String, String, Option<f32>, String) {
    let font = value_for(content, "font").unwrap_or_default();
    let fallback = value_for(content, "font_fallback").unwrap_or_default();
    let weight = value_for(content, "font_weight")
        .and_then(|raw| raw.parse::<f32>().ok())
        .filter(|w| w.is_finite() && *w > 0.0);
    let text_render_mode =
        value_for(content, "text_render_mode").unwrap_or_else(|| "auto".to_owned());
    (font, fallback, weight, text_render_mode)
}

/// 把 `<project>/settings.toml` 的 `[audio]` 段读进面板缓冲。
///
/// 文件缺失（可选文件）→ 保持引擎默认 1.0；读取失败 → 返回可读原因（面板提示，
/// 不 panic）。文件内非法数值由 `ProjectSettings::from_toml` 忽略并回落默认值。
pub(crate) fn load_audio_into(ui: &mut ProjectSettingsUi, root: &Path) -> Result<(), String> {
    let root = strip_verbatim_prefix(root);
    let settings = match ProjectSettings::load_from(&root) {
        Ok(Some(settings)) => settings,
        Ok(None) => ProjectSettings::new(),
        Err(error) => {
            return Err(format!(
                "读取 {} 失败：{error}",
                ProjectSettings::path_in(&root).display()
            ));
        }
    };
    apply_audio_buffers(ui, &settings);
    Ok(())
}

/// 把 `ProjectSettings` 写到面板缓冲（含 0..=1 钳制）。
fn apply_audio_buffers(ui: &mut ProjectSettingsUi, settings: &ProjectSettings) {
    use engine::core::audio::AudioBus;
    ui.audio_master = settings.volume(AudioBus::Master);
    ui.audio_bgm = settings.volume(AudioBus::Bgm);
    ui.audio_sfx = settings.volume(AudioBus::Sfx);
    ui.audio_ui = settings.volume(AudioBus::Ui);
}

/// 面板缓冲 → `ProjectSettings`（越界/非有限值钳制，写入不会产生非法文件）。
pub(crate) fn audio_settings_from(ui: &ProjectSettingsUi) -> ProjectSettings {
    use engine::core::audio::AudioBus;
    let mut settings = ProjectSettings::new();
    for (bus, value) in [
        (AudioBus::Master, ui.audio_master),
        (AudioBus::Bgm, ui.audio_bgm),
        (AudioBus::Sfx, ui.audio_sfx),
        (AudioBus::Ui, ui.audio_ui),
    ] {
        let value = if value.is_finite() { value } else { 1.0 };
        settings.set_volume(bus, value.clamp(0.0, 1.0));
    }
    settings
}

/// 保存音频小节到 `<project>/settings.toml`；失败返回可读原因（面板提示）。
pub(crate) fn save_audio_settings(ui: &ProjectSettingsUi, root: &Path) -> Result<(), String> {
    let root = strip_verbatim_prefix(root);
    let path = ProjectSettings::path_in(&root);
    audio_settings_from(ui)
        .save_to(&root)
        .map_err(|error| format!("保存 {} 失败：{error}", path.display()))
}

impl EditorApp {
    /// 项目菜单「项目设置…」：读取当前项目 project.toml 到缓冲并打开窗口。
    pub(crate) fn open_project_settings(&mut self) {
        let settings = &mut self.project_settings;
        settings.open = true;
        settings.message.clear();
        let root = strip_verbatim_prefix(&self.project.root);
        let path = root.join("project.toml");
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                parse_into(settings, &content, &root);
            }
            Err(error) => {
                settings.message = format!("读取 {} 失败：{error}", path.display());
            }
        }
        // M20-02：音频小节来自独立的 settings.toml（缺失即默认值）。
        if let Err(error) = load_audio_into(settings, &root) {
            if settings.message.is_empty() {
                settings.message = error;
            } else {
                settings.message.push('\n');
                settings.message.push_str(&error);
            }
        }
        // M20-08：输入小节（动作绑定 + 死区）同样来自 settings.toml 的 `[input]` 段。
        // 非法内容会回退默认并给出可定位诊断（追加到同一提示区，不覆盖音频提示）。
        if let Some(diagnostic) = settings.input.load(&root) {
            if settings.message.is_empty() {
                settings.message = diagnostic;
            } else {
                settings.message.push('\n');
                settings.message.push_str(&diagnostic);
            }
        }
    }

    /// M20-08：捕获模式下**先于编辑器快捷键**消费本帧键盘事件并改绑。
    ///
    /// 隔离层：命中后把本帧的 `Event::Key` 从输入事件流里移除，因此 `main.rs`
    /// 的 Ctrl+S/Ctrl+Z/Delete/Q/W/E/R 等快捷键看不到这次按键（捕获不会顺手
    /// 触发编辑器命令）；指针事件保留，所以「取消」按钮仍可点击。
    /// 手柄侧只在捕获期间轮询设备（`InputBindingsUi::poll_capture`）。
    pub(crate) fn capture_input_binding(&mut self, context: &egui::Context) {
        if !self.project_settings.open || !self.project_settings.input.is_capturing() {
            return;
        }
        // 手柄：新按下/推过阈值 → 立即改绑（去抖见 input_bindings）。
        if let Some(binding) = self.project_settings.input.poll_capture()
            && let Err(error) = self.project_settings.input.apply_capture(binding)
        {
            self.project_settings.input.message = error;
        }
        // 文本框获得焦点时（手动绑定输入中）不抢按键，`Event::Text` 仍会写进文本框。
        if context.wants_keyboard_input() {
            return;
        }
        let mut captured = None;
        let mut cancel = false;
        context.input(|input| {
            for event in &input.events {
                let egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    ..
                } = event
                else {
                    continue;
                };
                if *key == egui::Key::Escape {
                    cancel = true;
                    break;
                }
                if captured.is_none() {
                    captured = input_bindings::binding_from_egui_key(*key);
                }
            }
        });
        if cancel {
            self.project_settings.input.cancel_capture();
        }
        // 隔离编辑器自身的快捷键：吃掉本帧的按键事件（保留指针/文本事件）。
        context.input_mut(|input| {
            input
                .events
                .retain(|event| !matches!(event, egui::Event::Key { .. }));
        });
        if let Some(binding) = captured
            && let Err(error) = self.project_settings.input.apply_capture(binding)
        {
            self.project_settings.input.message = error;
        }
    }

    /// 独立弹出「项目设置」窗口（update 帧末绘制）。
    pub(crate) fn draw_project_settings_panel(&mut self, context: &egui::Context) {
        if !self.project_settings.open {
            return;
        }
        let mut open = self.project_settings.open;
        egui::Window::new("项目设置")
            .open(&mut open)
            .resizable(true)
            .default_width(460.0)
            .show(context, |ui| {
                let root = strip_verbatim_prefix(&self.project.root);
                ui.label(format!("{}", root.join("project.toml").display()));
                ui.separator();
                egui::Grid::new("project_settings_grid")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("名称");
                        ui.text_edit_singleline(&mut self.project_settings.name);
                        ui.end_row();
                        ui.label("版本");
                        ui.add_enabled(
                            false,
                            egui::TextEdit::singleline(&mut self.project_settings.version),
                        );
                        ui.end_row();
                        ui.label("引擎");
                        ui.add_enabled(
                            false,
                            egui::TextEdit::singleline(&mut self.project_settings.engine),
                        );
                        ui.end_row();
                        ui.label("入口脚本");
                        ui.text_edit_singleline(&mut self.project_settings.main_script);
                        ui.end_row();
                        ui.label("启动场景");
                        ui.text_edit_singleline(&mut self.project_settings.startup_scene);
                        ui.end_row();
                        ui.label("字体");
                        ui.text_edit_singleline(&mut self.project_settings.font);
                        ui.end_row();
                        ui.label("后备字体");
                        ui.text_edit_singleline(&mut self.project_settings.font_fallback);
                        ui.end_row();
                        ui.label("字重");
                        let mut weight = self.project_settings.font_weight;
                        if ui
                            .add(
                                egui::DragValue::new(&mut weight)
                                    .range(1.0..=1000.0)
                                    .speed(1.0),
                            )
                            .changed()
                        {
                            self.project_settings.font_weight = weight;
                        }
                        ui.end_row();
                        ui.label("文本渲染模式");
                        egui::ComboBox::from_id_salt("project_text_render_mode")
                            .selected_text(
                                TEXT_RENDER_MODES
                                    .iter()
                                    .find(|(key, _)| *key == self.project_settings.text_render_mode)
                                    .map(|(_, label)| *label)
                                    .unwrap_or("自动"),
                            )
                            .show_ui(ui, |ui| {
                                for (key, label) in TEXT_RENDER_MODES {
                                    ui.selectable_value(
                                        &mut self.project_settings.text_render_mode,
                                        key,
                                        *label,
                                    );
                                }
                            });
                        ui.end_row();
                    });
                ui.separator();
                // ---- M20-08：输入（动作绑定 + 死区，settings.toml 的 [input] 段）----
                self.draw_input_bindings_section(ui, &root);
                ui.separator();
                let missing = missing_asset_paths(&self.project_settings, &self.project.root);
                if !missing.is_empty() {
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 160, 60),
                        format!(
                            "以下路径不存在（保存后运行/发布时按各自回退处理）：{}",
                            missing.join("，")
                        ),
                    );
                }
                if !self.project_settings.message.is_empty() {
                    ui.label(&self.project_settings.message);
                }
                ui.horizontal(|ui| {
                    if ui.button("保存").clicked() {
                        self.save_project_settings();
                    }
                    if ui.button("取消").clicked() {
                        self.project_settings.open = false;
                    }
                });
            });
        self.project_settings.open = open;
    }

    /// M20-08「输入」小节：逐动作绑定列表 + 捕获式改绑 + 冲突提示 + 死区 + 恢复默认。
    ///
    /// 所有编辑先落在 `ProjectSettingsUi::input` 缓冲里，点「保存」才写
    /// `<project>/settings.toml` 的 `[input]` 段（保序，见引擎 `upsert_input_settings`）。
    fn draw_input_bindings_section(&mut self, ui: &mut egui::Ui, root: &Path) {
        ui.label("输入（按键 / 手柄）");
        ui.label(input_bindings::settings_path(root));
        let capturing = self.project_settings.input.is_capturing();
        // 捕获横幅：提示 + 手动绑定文本框 + 取消。捕获会隔离编辑器快捷键
        // （见 `capture_input_binding`）。
        if let Some(target) = self.project_settings.input.capture_target() {
            let target = target.to_owned();
            ui.colored_label(
                egui::Color32::from_rgb(230, 200, 90),
                format!(
                    "正在捕获「{}」：按键盘按键 / 手柄按钮 / 推摇杆或扳机；Esc 取消。",
                    action_label(&target)
                ),
            );
            ui.label(format!(
                "手柄后端 {}（{}）",
                self.project_settings.input.backend_name(),
                if self.project_settings.input.gamepad_connected() {
                    "已检测到设备"
                } else {
                    "未检测到设备：可只用键盘按钮捕获"
                }
            ));
            ui.horizontal(|ui| {
                ui.label("手动绑定文本");
                ui.text_edit_singleline(&mut self.project_settings.input.manual_binding);
                if ui.button("应用").clicked()
                    && let Err(error) = self.project_settings.input.apply_manual_to(&target)
                {
                    self.project_settings.input.message = error;
                }
                if ui.button("取消捕获").clicked() {
                    self.project_settings.input.cancel_capture();
                }
            });
        }
        // 冲突提示（同一物理输入绑到多个动作）：可读中文 + 阻止保存。
        let conflicts = self.project_settings.input.conflicts();
        if !conflicts.is_empty() {
            let describe = describe_conflicts(conflicts);
            ui.colored_label(
                egui::Color32::from_rgb(235, 120, 110),
                format!("绑定冲突（保存前必须解决）：{describe}"),
            );
        }
        // 动作列表先拷成拥有所有权的字符串：循环体里会改绑/清除（可变借用），
        // 不能同时持有 `map` 的借用。
        let actions: Vec<String> = self
            .project_settings
            .input
            .map
            .actions()
            .into_iter()
            .map(str::to_owned)
            .collect();
        egui::ScrollArea::vertical()
            .id_salt("project_settings_input_bindings")
            .max_height(220.0)
            .show(ui, |ui| {
                egui::Grid::new("project_settings_input_grid")
                    .num_columns(4)
                    .spacing([10.0, 4.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for action in actions {
                            let (list, has_bindings) = {
                                let bindings = self.project_settings.input.map.bindings(&action);
                                let list = if bindings.is_empty() {
                                    "（未绑定）".to_owned()
                                } else {
                                    bindings
                                        .iter()
                                        .map(|binding| binding_label(*binding))
                                        .collect::<Vec<_>>()
                                        .join(" / ")
                                };
                                (list, !bindings.is_empty())
                            };
                            ui.label(format!("{}({})", action_label(&action), action));
                            ui.label(list);
                            if ui
                                .add_enabled(!capturing, egui::Button::new("捕获"))
                                .clicked()
                            {
                                self.project_settings.input.begin_capture(&action);
                            }
                            if ui
                                .add_enabled(has_bindings, egui::Button::new("清除"))
                                .clicked()
                            {
                                self.project_settings.input.clear_action(&action);
                            }
                            ui.end_row();
                        }
                    });
            });
        // 死区：默认 0.15 / 0.05，写入 [input] 段（引擎侧 `GamepadDeadzone`）。
        let mut stick = self.project_settings.input.deadzone.stick;
        let mut trigger = self.project_settings.input.deadzone.trigger;
        egui::Grid::new("project_settings_input_deadzone")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label(format!("摇杆死区  {:.2}", stick));
                let stick_changed = ui
                    .add(
                        egui::Slider::new(&mut stick, 0.0..=0.9)
                            .fixed_decimals(2)
                            .show_value(false),
                    )
                    .changed();
                ui.end_row();
                ui.label(format!("扳机死区  {:.2}", trigger));
                let trigger_changed = ui
                    .add(
                        egui::Slider::new(&mut trigger, 0.0..=0.9)
                            .fixed_decimals(2)
                            .show_value(false),
                    )
                    .changed();
                ui.end_row();
                if stick_changed || trigger_changed {
                    self.project_settings.input.set_deadzone(stick, trigger);
                }
            });
        if self.project_settings.input.deadzone_changed() {
            ui.label("死区已改为非默认值（默认 0.15 / 0.05，保存后写入 [input] 段）。");
        }
        ui.horizontal(|ui| {
            if self.project_settings.input.confirm_reset {
                ui.colored_label(
                    egui::Color32::from_rgb(235, 120, 110),
                    "确认放弃当前改动、恢复默认绑定？",
                );
                if ui.button("确认恢复默认").clicked() {
                    self.project_settings.input.reset_defaults();
                }
                if ui.button("取消").clicked() {
                    self.project_settings.input.confirm_reset = false;
                }
            } else if ui.button("恢复默认").clicked() {
                // 二次确认：先进入待确认状态，下一次点击才真正重置。
                self.project_settings.input.confirm_reset = true;
            }
            if capturing && ui.button("取消捕获").clicked() {
                self.project_settings.input.cancel_capture();
            }
        });
        if !self.project_settings.input.message.is_empty() {
            ui.label(&self.project_settings.input.message);
        }
    }

    /// 保存缓冲到 project.toml：保序写回（保留注释/未知键），字体键变化时重建
    /// 共享图集缓存。
    fn save_project_settings(&mut self) {
        let root = strip_verbatim_prefix(&self.project.root);
        let path = root.join("project.toml");
        // M20-08：先做输入小节校验（跨动作冲突 = 配置错误），校验不通过就整体不落盘，
        // 避免写出「一半新一半旧」的设置文件。
        if let Err(error) = self.project_settings.input.save(&root) {
            self.project_settings.message = error;
            return;
        }
        // 项目打开时该文件必存在；万一缺失则从空骨架开始补全。
        let previous = std::fs::read_to_string(&path).unwrap_or_else(|_| "[project]\n".to_owned());
        let (old_font, old_fallback, old_weight, old_text_render_mode) = font_keys_from(&previous);

        let settings = &self.project_settings;
        let edits: Vec<(&str, String, bool)> = vec![
            ("name", settings.name.clone(), true),
            ("main_script", settings.main_script.clone(), true),
            ("startup_scene", settings.startup_scene.clone(), true),
            ("font", settings.font.clone(), true),
            ("font_fallback", settings.font_fallback.clone(), true),
            ("font_weight", fmt_font_weight(settings.font_weight), false),
            (
                "text_render_mode",
                settings.text_render_mode.to_owned(),
                true,
            ),
        ];
        let content = upsert_project_toml(&previous, &edits);
        if let Err(error) = std::fs::write(&path, content) {
            self.project_settings.message = format!("保存 {} 失败：{error}", path.display());
            return;
        }
        let font_changed = old_font != self.project_settings.font
            || old_fallback != self.project_settings.font_fallback
            || old_weight != Some(self.project_settings.font_weight);
        let text_render_mode_changed =
            old_text_render_mode != self.project_settings.text_render_mode;
        if font_changed || text_render_mode_changed {
            // 模式在运行时初始化时读取；字体图集/运行时缓存都按旧配置建立。
            // 强制重建 Scene View 运行时，避免保存后仍显示旧渲染路径。
            self.invalidate_font_render_resources();
        }
        self.project_settings.message = "已保存项目设置（输入绑定）。".to_owned();
    }
}

/// 返回缓冲里引用但项目内不存在的资源路径（相对项目根）。空值跳过（按引擎
/// 回退语义处理：空 startup_scene = headless、空 font = 默认字体等）。
fn missing_asset_paths(settings: &ProjectSettingsUi, root: &Path) -> Vec<String> {
    let root = strip_verbatim_prefix(root);
    let mut missing = Vec::new();
    for (key, value) in [
        ("main_script", &settings.main_script),
        ("startup_scene", &settings.startup_scene),
        ("font", &settings.font),
        ("font_fallback", &settings.font_fallback),
    ] {
        if !value.is_empty() && !root.join(value).is_file() {
            missing.push(format!("{key} {value}"));
        }
    }
    missing
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "[project]\nname = \"demo\"\nversion = \"0.1.0\"\nengine = \"RustLua\"\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n# 一个注释\nextra_key = \"keep-me\"\n";

    #[test]
    fn parses_known_string_and_float_keys() {
        let mut ui = ProjectSettingsUi::default();
        parse_into(&mut ui, SAMPLE, Path::new("/tmp/demo"));
        assert_eq!(ui.name, "demo");
        assert_eq!(ui.version, "0.1.0");
        assert_eq!(ui.engine, "RustLua");
        assert_eq!(ui.main_script, "scripts/main.lua");
        assert_eq!(ui.startup_scene, "scenes/main.scene");
        assert_eq!(ui.font_weight, 400.0); // 缺省字重
        assert_eq!(ui.font, ""); // 文件无 font → 空（引擎回退默认）
        assert_eq!(ui.text_render_mode, "auto"); // 缺省渲染模式
    }

    #[test]
    fn parses_missing_name_falls_back_to_directory_name() {
        let mut ui = ProjectSettingsUi::default();
        parse_into(
            &mut ui,
            "[project]\nversion = \"0.1.0\"\n",
            Path::new("C:\\Games\\MyGame"),
        );
        assert_eq!(ui.name, "MyGame");
    }

    #[test]
    fn upsert_replaces_existing_key_in_place_and_keeps_unknowns_and_comments() {
        let edits = [
            ("name", "renamed".to_owned(), true),
            ("font", "assets/fonts/Other.ttf".to_owned(), true),
            ("font_weight", "700.0".to_owned(), false),
        ];
        let out = upsert_project_toml(SAMPLE, &edits);
        assert!(out.contains("name = \"renamed\""), "{out}");
        assert!(out.contains("# 一个注释"), "{out}");
        assert!(out.contains("extra_key = \"keep-me\""), "{out}");
        assert!(out.contains("font = \"assets/fonts/Other.ttf\""), "{out}");
        assert!(out.contains("font_weight = 700.0"), "{out}");
        assert!(!out.contains("name = \"demo\""), "{out}");
        // 保序：main_script 行在 name 行之后、注释行仍在。
        let name_pos = out.find("name = \"renamed\"").unwrap();
        let script_pos = out.find("main_script").unwrap();
        let comment_pos = out.find("# 一个注释").unwrap();
        assert!(name_pos < script_pos);
        assert!(script_pos < comment_pos);
    }

    #[test]
    fn upsert_appends_missing_keys_at_end() {
        let content = "[project]\nname = \"demo\"\n";
        let out = upsert_project_toml(
            content,
            &[
                ("name", "demo".to_owned(), true),
                ("font", "assets/fonts/x.ttf".to_owned(), true),
                ("font_weight", "400.0".to_owned(), false),
            ],
        );
        assert_eq!(
            out,
            "[project]\nname = \"demo\"\nfont = \"assets/fonts/x.ttf\"\nfont_weight = 400.0\n"
        );
    }

    #[test]
    fn upsert_on_empty_content_builds_skeleton() {
        let out = upsert_project_toml(
            "",
            &[
                ("name", "a".to_owned(), true),
                ("font_weight", "400.0".to_owned(), false),
            ],
        );
        assert_eq!(out, "name = \"a\"\nfont_weight = 400.0\n");
    }

    #[test]
    fn weight_formatting_is_stable() {
        assert_eq!(fmt_font_weight(400.0), "400.0");
        assert_eq!(fmt_font_weight(412.5), "412.5");
        let back: f32 = "412.5".parse().unwrap();
        assert_eq!(back, 412.5);
    }

    #[test]
    fn text_render_mode_is_case_insensitive_and_falls_back_to_auto() {
        let mut ui = ProjectSettingsUi::default();
        parse_into(
            &mut ui,
            "[project]\ntext_render_mode = \"Vector\"\n",
            Path::new("/tmp/demo"),
        );
        assert_eq!(ui.text_render_mode, "vector");

        ui.text_render_mode = "sdf";
        let content = upsert_project_toml(
            "[project]\nname = \"demo\"\ntext_render_mode = \"Auto\"\n",
            &[("text_render_mode", "sdf".to_owned(), true)],
        );
        assert!(content.contains("text_render_mode = \"sdf\""), "{content}");

        let mut ui = ProjectSettingsUi::default();
        parse_into(
            &mut ui,
            "text_render_mode = \"bad\"",
            Path::new("/tmp/demo"),
        );
        assert_eq!(ui.text_render_mode, "auto");
    }

    #[test]
    fn round_trip_preserves_idempotence() {
        let mut edits = vec![
            ("startup_scene", "scenes/other.scene".to_owned(), true),
            (
                "font_fallback",
                "assets/fonts/Fallback.ttf".to_owned(),
                true,
            ),
        ];
        let once = upsert_project_toml(SAMPLE, &edits);
        let twice = upsert_project_toml(&once, &edits);
        assert_eq!(once, twice);
        // 再次替换同键只改一次，不产生重复键行。
        edits[1].1 = "assets/fonts/Changed.ttf".to_owned();
        let thrice = upsert_project_toml(&once, &edits);
        let count = thrice.matches("font_fallback =").count();
        assert_eq!(count, 1, "{thrice}");
        assert!(thrice.contains("assets/fonts/Changed.ttf"));
    }

    /// 测试用临时项目目录（进程内唯一，避免并行测试互相覆盖）。
    fn temp_project(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "rustlua-editor-settings-{tag}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn audio_section_defaults_when_settings_file_is_missing() {
        let root = temp_project("missing");
        // 先塞一个脏值，确认「文件缺失」确实覆盖它为引擎默认（而不是保留旧缓冲）。
        let mut ui = ProjectSettingsUi {
            audio_master: 0.1,
            ..ProjectSettingsUi::default()
        };
        load_audio_into(&mut ui, &root).unwrap();
        // 缺文件 → 引擎默认 1.0（不是保留旧缓冲）。
        assert_eq!(ui.audio_master, 1.0);
        assert_eq!(ui.audio_bgm, 1.0);
        assert_eq!(ui.audio_sfx, 1.0);
        assert_eq!(ui.audio_ui, 1.0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn audio_section_loads_parses_and_saves_round_trip() {
        let root = temp_project("round-trip");
        std::fs::write(
            root.join("settings.toml"),
            "# 玩家设置\n[audio]\nmaster = 0.5\nbgm = 0.25\n# 注释保留\nsfx = 0.75\nui = bad\n\n[input]\nmap = \"keep-me\"\n",
        )
        .unwrap();
        let mut ui = ProjectSettingsUi::default();
        load_audio_into(&mut ui, &root).unwrap();
        assert_eq!(ui.audio_master, 0.5);
        assert_eq!(ui.audio_bgm, 0.25);
        assert_eq!(ui.audio_sfx, 0.75);
        // 非法值被忽略 → 默认 1.0，且不 panic。
        assert_eq!(ui.audio_ui, 1.0);

        ui.audio_bgm = 0.4;
        ui.audio_ui = 0.6;
        save_audio_settings(&ui, &root).unwrap();
        let text = std::fs::read_to_string(root.join("settings.toml")).unwrap();
        assert!(text.contains("# 玩家设置"), "{text}");
        assert!(text.contains("# 注释保留"), "{text}");
        assert!(text.contains("bgm = 0.4"), "{text}");
        assert!(text.contains("ui = 0.6"), "{text}");
        assert!(text.contains("map = \"keep-me\""), "{text}");
        let mut reloaded = ProjectSettingsUi::default();
        load_audio_into(&mut reloaded, &root).unwrap();
        assert_eq!(reloaded.audio_bgm, 0.4);
        assert_eq!(reloaded.audio_ui, 0.6);
        assert_eq!(reloaded.audio_master, 0.5);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn audio_section_clamps_buffers_and_reports_write_errors() {
        let root = temp_project("errors");
        let mut ui = ProjectSettingsUi {
            audio_master: 3.0,
            audio_bgm: -2.0,
            audio_sfx: f32::NAN,
            audio_ui: 0.5,
            ..ProjectSettingsUi::default()
        };
        let settings = audio_settings_from(&ui);
        use engine::core::audio::AudioBus;
        assert_eq!(settings.volume(AudioBus::Master), 1.0);
        assert_eq!(settings.volume(AudioBus::Bgm), 0.0);
        assert_eq!(settings.volume(AudioBus::Sfx), 1.0); // 非有限值回落默认
        assert_eq!(settings.volume(AudioBus::Ui), 0.5);
        ui.audio_master = 0.8;

        // 写失败（settings.toml 是目录）→ 返回可读原因而不是 panic。
        std::fs::create_dir_all(root.join("settings.toml")).unwrap();
        let error = save_audio_settings(&ui, &root).unwrap_err();
        assert!(error.contains("settings.toml"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
