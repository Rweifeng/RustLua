//! 编辑器通用工具函数：路径清理、项目错误格式化、CJK 字体、目录选择。

use editor::ProjectError;
use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use std::{
    env,
    path::{Path, PathBuf},
};

/// 去除 Windows 逐字路径前缀 `\\?\`（会破坏 Lua require 的 searchpath 解析）。
pub(crate) fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text))
}

/// 供 UI 显示：去除 `\\?\` 前缀后的路径字符串（编辑器界面一律用它显示路径）。
pub(crate) fn display_path(path: &Path) -> String {
    strip_verbatim_prefix(path).display().to_string()
}

/// 最近打开项目记录文件（与 crash.log/editor.log 同目录）。
fn recent_projects_path() -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|dir| dir.join("recent_projects.txt")))
        .unwrap_or_else(|| PathBuf::from("recent_projects.txt"))
}

/// 读取最近打开的项目列表（每行一个路径，最新在前）。
pub(crate) fn load_recent_projects() -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(recent_projects_path()) else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// 落盘最近打开的项目列表。
pub(crate) fn save_recent_projects(projects: &[String]) {
    let _ = std::fs::write(recent_projects_path(), projects.join("\n"));
}

pub(crate) fn format_project_error(path: &Path, error: ProjectError) -> String {
    format!("无法打开项目 {}：{}", path.display(), error)
}

#[cfg(windows)]
pub(crate) fn choose_project_directory() -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{BROWSEINFOW, SHBrowseForFolderW, SHGetPathFromIDListW};

    let title: Vec<u16> = "选择游戏项目目录\0".encode_utf16().collect();
    let info = BROWSEINFOW {
        hwndOwner: std::ptr::null_mut(),
        pidlRoot: std::ptr::null_mut(),
        pszDisplayName: std::ptr::null_mut(),
        lpszTitle: title.as_ptr(),
        ulFlags: 0x0000_0001,
        lpfn: None,
        lParam: 0,
        iImage: 0,
    };
    let pidl = unsafe { SHBrowseForFolderW(&info) };
    if pidl.is_null() {
        return None;
    }
    let mut buffer = [0u16; 32768];
    let selected = unsafe { SHGetPathFromIDListW(pidl, buffer.as_mut_ptr()) != 0 };
    unsafe {
        CoTaskMemFree(pidl.cast());
    }
    if !selected {
        return None;
    }
    let length = buffer
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(buffer.len());
    Some(PathBuf::from(OsString::from_wide(&buffer[..length])))
}

#[cfg(not(windows))]
pub(crate) fn choose_project_directory() -> Option<PathBuf> {
    None
}

const CJK_FONT_FILE_NAMES: &[&str] = &["NotoSansSC-VF.ttf", "simhei.ttf", "Deng.ttf"];

fn cjk_font_candidates() -> Vec<PathBuf> {
    let mut candidates = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fonts")
            .join("NotoSansSC-VF.ttf"),
    ];
    if let Ok(executable) = env::current_exe()
        && let Some(parent) = executable.parent()
    {
        candidates.extend(
            CJK_FONT_FILE_NAMES
                .iter()
                .map(|name| parent.join("fonts").join(name)),
        );
        candidates.extend(CJK_FONT_FILE_NAMES.iter().map(|name| parent.join(name)));
    }
    if cfg!(target_os = "windows") {
        candidates.extend(
            CJK_FONT_FILE_NAMES
                .iter()
                .map(|name| PathBuf::from("C:/Windows/Fonts").join(name)),
        );
    }
    candidates
}

pub(crate) fn configure_cjk_fonts(context: &egui::Context) {
    let Some(path) = cjk_font_candidates()
        .into_iter()
        .find(|path| path.is_file())
    else {
        eprintln!("editor: no CJK font found; Chinese text may render as tofu boxes");
        return;
    };
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("editor: failed to read CJK font {}", path.display());
        return;
    };
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("p2-cjk".to_owned(), FontData::from_owned(bytes).into());
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, "p2-cjk".to_owned());
    }
    context.set_fonts(fonts);
}
