//! 「项目设置」独立弹出窗口：在 `项目` 菜单打开，编辑当前项目的 `project.toml`
//! 结构化字段并保存。project.toml 全工作区没有 toml crate，所有消费者都把文件当
//! 扁平 `key = value` 逐行解析（见引擎 `runtime_scene.rs` 的 `extract_project_key`），
//! 因此这里也采用同样语义的宽松解析，保存时**保序**替换已存在键并原样保留未知
//! 键与注释（`[project]` 表头无结构含义，仅作标题）。

use crate::EditorApp;
use crate::util::strip_verbatim_prefix;
use eframe::egui;
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
        if let Some(key) = key_of_line {
            if matched.insert(key) {
                let (_, value, quoted) = edits.iter().find(|(k, _, _)| *k == key).unwrap();
                *line = render_line(key, value, *quoted);
            }
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

/// 字体相关键旧值（用于保存后判定是否需要重建共享字体图集）。
fn font_keys_from(content: &str) -> (String, String, Option<f32>) {
    let font = value_for(content, "font").unwrap_or_default();
    let fallback = value_for(content, "font_fallback").unwrap_or_default();
    let weight = value_for(content, "font_weight")
        .and_then(|raw| raw.parse::<f32>().ok())
        .filter(|w| w.is_finite() && *w > 0.0);
    (font, fallback, weight)
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
                    });
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

    /// 保存缓冲到 project.toml：保序写回（保留注释/未知键），字体键变化时重建
    /// 共享图集缓存。
    fn save_project_settings(&mut self) {
        let root = strip_verbatim_prefix(&self.project.root);
        let path = root.join("project.toml");
        // 项目打开时该文件必存在；万一缺失则从空骨架开始补全。
        let previous = std::fs::read_to_string(&path).unwrap_or_else(|_| "[project]\n".to_owned());
        let (old_font, old_fallback, old_weight) = font_keys_from(&previous);

        let settings = &self.project_settings;
        let edits: Vec<(&str, String, bool)> = vec![
            ("name", settings.name.clone(), true),
            ("main_script", settings.main_script.clone(), true),
            ("startup_scene", settings.startup_scene.clone(), true),
            ("font", settings.font.clone(), true),
            ("font_fallback", settings.font_fallback.clone(), true),
            ("font_weight", fmt_font_weight(settings.font_weight), false),
        ];
        let content = upsert_project_toml(&previous, &edits);
        if let Err(error) = std::fs::write(&path, content) {
            self.project_settings.message = format!("保存 {} 失败：{error}", path.display());
            return;
        }
        let font_changed = old_font != self.project_settings.font
            || old_fallback != self.project_settings.font_fallback
            || old_weight != Some(self.project_settings.font_weight);
        if font_changed {
            // 字体图集按 project.toml `font` 键缓存：改动后重建默认/自定义图集。
            self.scene_font_atlas = None;
            self.custom_font_atlases.lock().unwrap().clear();
        }
        self.project_settings.message = "已保存项目设置。".to_owned();
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
}
