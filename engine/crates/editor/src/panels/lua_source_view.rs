//! 只读 Lua 源码查看器：**看**脚本、定位到报错行，不编辑。
//!
//! ## 为什么要有它
//!
//! 在它之前，编辑器里点控制台的一条 `scripts/main.lua:42` 只能把文件交给外部编辑器；
//! 机器上没装 VS Code/Notepad++ 时退化成系统文件关联，连行号都没有。改游戏逻辑时
//! "改—跑—看日志—猜" 的循环里，**看**这一环不该依赖用户另外装一套工具。
//!
//! 这里刻意只做只读视图：
//! - 没有编辑、保存、撤销——写脚本仍然交给用户的编辑器（`docs/55` §2.2 的边界不变）；
//! - 窗口里给一个「在外部编辑器打开」按钮，需要改的时候一步转过去；
//! - 行号视图用 `show_rows` 只画可见行，几千行的脚本滚动不掉帧。
//!
//! ## 口径
//!
//! - 目标行是 **1 起**，越界时夹到 `1..=行数`；空文件没有目标行。
//! - 换行统一按 `\n` 处理（`\r\n`/`\r` 都先归一），文件末尾的单个换行不算多一行。
//! - UTF-8 BOM 在显示前剥掉（它是编码标记，不是第一行的内容）。

use std::path::{Path, PathBuf};

use eframe::egui;

use crate::EditorApp;

/// 目标行的底色（半透明蓝，深浅主题下都能看出高亮而不盖住文字）。
const TARGET_LINE_FILL: egui::Color32 = egui::Color32::from_rgba_premultiplied(56, 96, 160, 70);

/// 行号列宽（点）。
const LINE_NUMBER_WIDTH: f32 = 52.0;

/// 这个文件能不能进只读查看器。`.luae` 是加密脚本，明文视图没有意义，仍走外部编辑器。
#[must_use]
pub(crate) fn is_lua_source(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("lua"))
}

/// 文本 → 行数组：剥 BOM、统一换行、去掉末尾换行产生的那一空行。
#[must_use]
pub(crate) fn split_source(text: &str) -> Vec<String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.is_empty() {
        return Vec::new();
    }
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<String> = normalized.split('\n').map(str::to_owned).collect();
    if lines.len() > 1 && lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// 把任意行号夹到有效范围；空文件返回 `None`。
#[must_use]
pub(crate) fn clamp_target(line: Option<u32>, line_count: usize) -> Option<u32> {
    let line = line?;
    if line_count == 0 {
        return None;
    }
    Some(line.clamp(1, line_count as u32))
}

/// 查看器窗口状态（挂在 `EditorApp` 上；只缓存当前这一份源码）。
#[derive(Default)]
pub(crate) struct LuaSourceViewState {
    pub show: bool,
    /// 绝对路径（真正读盘的那一个）。
    pub absolute: Option<PathBuf>,
    /// 界面上显示的路径（尽量相对工程根）。
    pub relative: Option<String>,
    pub lines: Vec<String>,
    /// 读盘失败原因（与"还没打开文件"区分开）。
    pub error: Option<String>,
    /// 要定位的行（1 起，已夹到有效范围）。
    pub target_line: Option<u32>,
    /// 上一次已经滚动到的 `(路径, 行)`：同一目标不每帧重复设偏移（否则用户滚不动）。
    scrolled_to: Option<(PathBuf, u32)>,
}

impl LuaSourceViewState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 换工程/关窗时整份作废。
    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// 读一份源码并记住目标行。读盘失败时窗口仍然打开，只显示错误。
    pub fn load(&mut self, absolute: &Path, relative: String, line: Option<u32>) {
        self.show = true;
        self.absolute = Some(absolute.to_path_buf());
        self.relative = Some(relative);
        self.error = None;
        self.scrolled_to = None;
        match std::fs::read_to_string(absolute) {
            Ok(text) => {
                self.lines = split_source(&text);
                self.target_line = clamp_target(line, self.lines.len());
            }
            Err(error) => {
                self.lines.clear();
                self.target_line = None;
                self.error = Some(format!("读 {} 失败：{error}", absolute.display()));
            }
        }
    }

    /// 重新读盘，保留目标行（外部编辑器改完脚本后按一下就能看到新内容）。
    pub fn reload(&mut self) {
        let (Some(absolute), Some(relative)) = (self.absolute.clone(), self.relative.clone())
        else {
            return;
        };
        let target = self.target_line;
        self.load(&absolute, relative, target);
    }
}

impl EditorApp {
    /// 在编辑器内打开一份 Lua 源码（`path` 可以是工程相对或绝对路径）。
    pub(crate) fn open_lua_source(&mut self, path: &Path, line: Option<u32>) {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.project.root.join(path)
        };
        let relative = absolute
            .strip_prefix(&self.project.root)
            .map_or_else(
                |_| absolute.to_string_lossy().replace('\\', "/"),
                |value| value.to_string_lossy().replace('\\', "/"),
            );
        self.lua_source_view.load(&absolute, relative, line);
    }

    /// 查看器窗口（独立窗，只读；主循环里每帧调用）。
    pub(crate) fn draw_lua_source_view(&mut self, context: &egui::Context) {
        if !self.lua_source_view.show {
            return;
        }
        let title = self
            .lua_source_view
            .relative
            .clone()
            .map_or_else(|| "Lua 源码".to_owned(), |path| format!("Lua 源码 — {path}"));
        let mut open = self.lua_source_view.show;
        egui::Window::new(title)
            .id(egui::Id::new("lua_source_view"))
            .open(&mut open)
            .default_size([880.0, 620.0])
            .resizable(true)
            .show(context, |ui| {
                self.draw_lua_source_view_body(ui);
            });
        self.lua_source_view.show = open && self.lua_source_view.show;
    }

    fn draw_lua_source_view_body(&mut self, ui: &mut egui::Ui) {
        let Some(absolute) = self.lua_source_view.absolute.clone() else {
            ui.weak("没有打开任何脚本。");
            return;
        };
        ui.horizontal(|ui| {
            ui.monospace(absolute.display().to_string());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("在外部编辑器打开").clicked() {
                    let line = self.lua_source_view.target_line;
                    self.status = match crate::external_editor::open_in_external_editor(
                        &absolute,
                        line,
                        self.external_editor_dry_run,
                    ) {
                        Ok(plan) => crate::external_editor::describe(&plan, &absolute, line),
                        Err(error) => error,
                    };
                }
                if ui.button("重新加载").clicked() {
                    self.lua_source_view.reload();
                }
            });
        });
        if let Some(error) = &self.lua_source_view.error {
            ui.colored_label(egui::Color32::RED, error);
            return;
        }
        let line_count = self.lua_source_view.lines.len();
        ui.horizontal(|ui| {
            ui.weak(format!("{line_count} 行"));
            if let Some(line) = self.lua_source_view.target_line {
                ui.separator();
                ui.strong(format!("第 {line} 行"));
            }
        });
        ui.separator();

        let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 2.0;
        // 只在目标行变化时设置一次滚动偏移：每帧都设会把用户的滚动“拉回去”。
        let pending_scroll = match (
            self.lua_source_view.absolute.as_ref(),
            self.lua_source_view.target_line,
            self.lua_source_view.scrolled_to.as_ref(),
        ) {
            (Some(path), Some(line), Some((scrolled_path, scrolled_line)))
                if path == scrolled_path && line == *scrolled_line =>
            {
                None
            }
            (_, Some(line), _) => Some(line),
            _ => None,
        };
        let mut area = egui::ScrollArea::vertical()
            .id_salt("lua_source_body")
            .auto_shrink([false; 2]);
        if let Some(line) = pending_scroll {
            let offset = line.saturating_sub(1) as f32 * row_height;
            area = area.vertical_scroll_offset(offset);
        }
        let lines = &self.lua_source_view.lines;
        let target = self.lua_source_view.target_line;
        area.show_rows(ui, row_height, line_count, |ui, range| {
            for index in range {
                let number = index as u32 + 1;
                let mut frame =
                    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(4, 0));
                if Some(number) == target {
                    frame = frame.fill(TARGET_LINE_FILL);
                }
                frame.show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [LINE_NUMBER_WIDTH, row_height],
                            egui::Label::new(
                                egui::RichText::new(format!("{number:>4}"))
                                    .monospace()
                                    .weak(),
                            ),
                        );
                        ui.add(
                            egui::Label::new(egui::RichText::new(&lines[index]).monospace())
                                .truncate()
                                .selectable(true),
                        );
                    });
                });
            }
        });
        if let (Some(line), Some(path)) = (pending_scroll, self.lua_source_view.absolute.clone()) {
            self.lua_source_view.scrolled_to = Some((path, line));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{clamp_target, is_lua_source, split_source, LuaSourceViewState};
    use std::path::{Path, PathBuf};

    fn temp_file(tag: &str, contents: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "rustlua-lua-view-{tag}-{}-{}.lua",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn split_source_normalizes_newlines_and_strips_bom() {
        assert_eq!(split_source("local a = 1\r\nlocal b = 2\n"), ["local a = 1", "local b = 2"]);
        assert_eq!(split_source("\u{feff}first\rsecond"), ["first", "second"]);
        assert_eq!(split_source("only line"), ["only line"]);
        // 末尾单个换行不算多一行；空文件是零行。
        assert!(split_source("").is_empty());
        assert_eq!(split_source("\n"), [""]);
    }

    #[test]
    fn clamp_target_keeps_the_line_inside_the_file() {
        assert_eq!(clamp_target(Some(2), 3), Some(2));
        assert_eq!(clamp_target(Some(0), 3), Some(1));
        assert_eq!(clamp_target(Some(99), 3), Some(3));
        assert_eq!(clamp_target(None, 3), None);
        assert_eq!(clamp_target(Some(1), 0), None, "空文件没有可定位的行");
    }

    #[test]
    fn only_plain_lua_files_open_the_viewer() {
        assert!(is_lua_source(Path::new("scripts/main.lua")));
        assert!(is_lua_source(Path::new("scripts/MAIN.LUA")));
        assert!(!is_lua_source(Path::new("scripts/main.luae")), "加密脚本没有明文视图");
        assert!(!is_lua_source(Path::new("scripts/main.txt")));
    }

    #[test]
    fn loading_a_file_remembers_the_target_line() {
        let path = temp_file("load", b"local a = 1\nlocal b = 2\nlocal c = 3\n");
        let mut state = LuaSourceViewState::new();
        state.load(&path, "scripts/test.lua".to_owned(), Some(2));
        assert!(state.show);
        assert_eq!(state.lines.len(), 3);
        assert_eq!(state.target_line, Some(2));
        assert_eq!(state.relative.as_deref(), Some("scripts/test.lua"));
        assert!(state.error.is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_missing_file_still_opens_with_an_error() {
        let missing = std::env::temp_dir().join("rustlua-lua-view-definitely-missing.lua");
        let mut state = LuaSourceViewState::new();
        state.load(&missing, "scripts/missing.lua".to_owned(), Some(4));
        assert!(state.show, "打不开也要开窗说清楚原因");
        assert!(state.lines.is_empty());
        assert_eq!(state.target_line, None);
        assert!(state.error.as_deref().is_some_and(|error| error.contains("失败")));
    }

    #[test]
    fn reload_picks_up_new_content_and_keeps_the_target() {
        let path = temp_file("reload", b"old\n");
        let mut state = LuaSourceViewState::new();
        state.load(&path, "tests/reload.lua".to_owned(), Some(1));
        std::fs::write(&path, b"new one\nnew two\n").unwrap();
        state.reload();
        assert_eq!(state.lines, ["new one", "new two"]);
        assert_eq!(state.target_line, Some(1));
        let _ = std::fs::remove_file(path);
    }
}
