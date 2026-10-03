//! 编辑器通用工具函数：路径清理、项目错误格式化、中文字体回退、目录选择。

use editor::ProjectError;
use eframe::egui::{self, FontData, FontDefinitions, FontFamily, Response, Ui};
use std::{
    collections::BTreeSet,
    env,
    path::{Path, PathBuf},
};

/// 测试用唯一令牌：进程 ID + 原子计数 + 时间戳。
///
/// 只取时间戳不够——Windows 时钟粒度下并行测试可能拿到同一个纳秒值，临时目录互撞，
/// 于是 `opens_project_and_reports_required_directories` 曾在全量测试里偶发失败。
/// bin 与 lib 各有一份本地实现（不把测试支持暴露成公开 API）。
#[cfg(test)]
mod playback_icon_tests {
    use super::{PlaybackIcon, pause_resume_icon};

    /// 图标选择：运行中显示"暂停"（两条竖杠），暂停中显示"继续"（三角形）。
    ///
    /// 这条测试盯住的是**用户看到的那个按钮**：曾经用 `❚❚` 字形，字体缺字形时渲染成
    /// 两个"豆腐块"（用户截图反馈）；现在两种状态都由画笔按这里的选择绘制。
    #[test]
    fn pause_button_shows_the_opposite_action_icon() {
        assert_eq!(pause_resume_icon(false), PlaybackIcon::Pause);
        assert_eq!(pause_resume_icon(true), PlaybackIcon::Play);
    }
}

#[cfg(test)]
pub(crate) fn test_token() -> u128 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = u128::from(COUNTER.fetch_add(1, Ordering::Relaxed));
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    (u128::from(std::process::id()) << 96) ^ (counter << 64) ^ (nanos & 0xFFFF_FFFF_FFFF_FFFF)
}

/// 去除 Windows 逐字路径前缀 `\\?\`（会破坏 Lua require 的 searchpath 解析）。
pub(crate) fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let text = text
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .or_else(|| text.strip_prefix(r"\\?\").map(str::to_owned))
        .unwrap_or_else(|| text.into_owned());
    PathBuf::from(text)
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

/// 项目本地编辑器状态文件（不进入 `.scene`；`.cache` 已被版本控制忽略）。
fn hierarchy_state_path(project_root: &Path) -> PathBuf {
    project_root
        .join(".cache")
        .join("editor")
        .join("hierarchy-state.json")
}

/// 读取指定场景的 Hierarchy 折叠实体 ID。损坏或缺省时返回空集合。
pub(crate) fn load_hierarchy_collapsed(project_root: &Path, scene_key: &str) -> BTreeSet<u64> {
    let Ok(text) = std::fs::read_to_string(hierarchy_state_path(project_root)) else {
        return BTreeSet::new();
    };
    let Ok(config) = serde_json::from_str::<serde_json::Value>(&text) else {
        return BTreeSet::new();
    };
    config
        .get("hierarchy")
        .and_then(|value| value.get(scene_key))
        .and_then(|value| value.get("collapsed"))
        .and_then(|value| value.as_array())
        .map(|values| values.iter().filter_map(|value| value.as_u64()).collect())
        .unwrap_or_default()
}

/// 保存指定场景的 Hierarchy 折叠实体 ID，保留配置文件中的其他键。
pub(crate) fn save_hierarchy_collapsed(
    project_root: &Path,
    scene_key: &str,
    collapsed: &BTreeSet<u64>,
) {
    let path = hierarchy_state_path(project_root);
    let mut config = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if !config.is_object() {
        config = serde_json::json!({});
    }
    let object = config
        .as_object_mut()
        .expect("config was normalized to an object");
    object.insert("version".to_owned(), serde_json::json!(1));
    let hierarchy = object
        .entry("hierarchy")
        .or_insert_with(|| serde_json::json!({}));
    if !hierarchy.is_object() {
        *hierarchy = serde_json::json!({});
    }
    hierarchy[scene_key] = serde_json::json!({
        "collapsed": collapsed.iter().copied().collect::<Vec<_>>(),
    });

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let token = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    let temp = parent.join(format!(".hierarchy-state.{token}.tmp"));
    if std::fs::write(
        &temp,
        serde_json::to_string_pretty(&config).unwrap_or_default(),
    )
    .is_err()
    {
        return;
    }
    if std::fs::rename(&temp, &path).is_err() {
        let _ = std::fs::remove_file(&temp);
    }
}

pub(crate) fn format_project_error(path: &Path, error: ProjectError) -> String {
    format!("无法打开项目 {}：{}", path.display(), error)
}

/// 固定尺寸的图标按钮：以图标为主，说明放到 hover 文本里。
pub(crate) fn icon_button(ui: &mut Ui, icon: &str, tooltip: &str) -> Response {
    ui.add_sized([24.0, 24.0], egui::Button::new(icon))
        .on_hover_text(tooltip)
}

/// 固定尺寸的图标切换按钮：用于工具条/面板开关。
pub(crate) fn icon_toggle(ui: &mut Ui, selected: bool, icon: &str, tooltip: &str) -> Response {
    ui.add_sized([24.0, 24.0], egui::Button::selectable(selected, icon))
        .on_hover_text(tooltip)
}

/// 播放控制图标要画成什么（纯数据，便于单测；画笔在 [`pause_resume_icon_button`]）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PlaybackIcon {
    /// 两条竖杠 = 暂停。
    Pause,
    /// 三角形 = 继续/播放。
    Play,
}

/// 暂停态显示"继续"（三角形），运行态显示"暂停"（两条竖杠）。
#[must_use]
pub(crate) const fn pause_resume_icon(paused: bool) -> PlaybackIcon {
    if paused {
        PlaybackIcon::Play
    } else {
        PlaybackIcon::Pause
    }
}

/// 暂停/继续按钮：**图标用画笔绘制**，不依赖字体里有 `❚` 这类字形。
///
/// 起因（用户反馈的截图）：原来按钮文字用 `❚❚`（U+275A），而编辑器配置的字体里没有这个
/// 字形，实际渲染成两个"豆腐块"（缺字方框）。图标用 `ui.painter()` 画出来就与字体无关，
/// 尺寸/配色仍走 egui 自己的按钮视觉（hover/按下反馈一致）。
pub(crate) fn pause_resume_icon_button(
    ui: &mut Ui,
    icon: PlaybackIcon,
    enabled: bool,
    tooltip: &str,
) -> Response {
    let size = egui::vec2(24.0, 24.0);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let visuals = ui.style().interact(&response);
    ui.painter().rect_filled(rect, 3.0, visuals.bg_fill);
    let color = if enabled {
        visuals.fg_stroke.color
    } else {
        ui.visuals().weak_text_color()
    };
    let center = rect.center();
    match icon {
        PlaybackIcon::Pause => {
            // 两条竖杠：居中对齐、宽 3px、高 12px、间距 3px。
            for offset in [-3.0_f32, 3.0] {
                let bar = egui::Rect::from_center_size(
                    egui::pos2(center.x + offset, center.y),
                    egui::vec2(3.0, 12.0),
                );
                ui.painter().rect_filled(bar, 1.0, color);
            }
        }
        PlaybackIcon::Play => {
            ui.painter().add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(center.x - 3.5, center.y - 6.0),
                    egui::pos2(center.x - 3.5, center.y + 6.0),
                    egui::pos2(center.x + 5.0, center.y),
                ],
                color,
                egui::Stroke::NONE,
            ));
        }
    }
    if enabled {
        response.on_hover_text(tooltip)
    } else {
        response.on_disabled_hover_text(tooltip)
    }
}

/// 画笔绘制"下三角"的小按钮：弹层入口（吸附步长等）。
/// 不用 `▾` 字形的原因与 [`pause_resume_icon_button`] 的 `❚❚` 一样——字体缺字形渲染成豆腐块。
pub(crate) fn chevron_down_button(ui: &mut Ui, tooltip: &str) -> Response {
    let size = egui::vec2(20.0, 24.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let visuals = ui.style().interact(&response);
    ui.painter().rect_filled(rect, 3.0, visuals.bg_fill);
    let center = rect.center();
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(center.x - 4.0, center.y - 2.0),
            egui::pos2(center.x + 4.0, center.y - 2.0),
            egui::pos2(center.x, center.y + 3.0),
        ],
        visuals.fg_stroke.color,
        egui::Stroke::NONE,
    ));
    response.on_hover_text(tooltip)
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

/// 中文字体候选路径，按优先级排列。
///
/// **为什么不能只用编译期路径**：原先这里写死 `env!("CARGO_MANIFEST_DIR")/fonts/…`，
/// 也就是构建机的绝对路径（`E:\Codex\RustLua\engine\crates\editor\fonts\…`）。
/// 绿色版（免安装、整目录拷到别的 Windows 主机）上那条路径根本不存在，
/// 于是中文全变成"豆腐块"——一个和包本身无关、却会让跨机验收直接失败的坑。
///
/// 优先级：显式环境变量 → **exe 同目录**（发布/便携形态）→ 编译期路径（开发形态）。
#[must_use]
pub(crate) fn cjk_font_candidates() -> Vec<PathBuf> {
    const FILE_NAME: &str = "NotoSansSC-VF.ttf";
    let mut candidates = Vec::new();
    if let Ok(explicit) = env::var("RUSTLUA_FONT") {
        let explicit = PathBuf::from(explicit);
        if !explicit.as_os_str().is_empty() {
            candidates.push(explicit);
        }
    }
    if let Ok(exe) = env::current_exe()
        && let Some(dir) = exe.parent()
    {
        // 包内布局：<包根>/editor.exe + <包根>/fonts/NotoSansSC-VF.ttf
        candidates.push(dir.join("fonts").join(FILE_NAME));
        // 允许把字体直接摆在 exe 旁边
        candidates.push(dir.join(FILE_NAME));
    }
    // 开发形态：源码树里的那份
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fonts")
            .join(FILE_NAME),
    );
    candidates
}

/// 为 egui 默认字体追加中文 fallback。
///
/// 找不到字体时**只退化、不静默**：打印试过的每一个路径，让人一眼看出是包缺了字体
/// 还是路径写错了（跨机验收时这一行日志就是结论）。
pub(crate) fn configure_cjk_fallback(context: &egui::Context) {
    let candidates = cjk_font_candidates();
    let mut bytes = None;
    for candidate in &candidates {
        match std::fs::read(candidate) {
            Ok(loaded) => {
                bytes = Some((candidate.clone(), loaded));
                break;
            }
            Err(error) => {
                eprintln!(
                    "editor: CJK fallback not at {} ({error})",
                    candidate.display()
                );
            }
        }
    }
    let Some((path, bytes)) = bytes else {
        eprintln!(
            "editor: no CJK fallback font found; tried {} path(s). \
             Chinese text will render as boxes. Set RUSTLUA_FONT to a .ttf, \
             or place NotoSansSC-VF.ttf in <exe dir>/fonts/.",
            candidates.len()
        );
        return;
    };
    eprintln!("editor: CJK fallback font = {}", path.display());
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "noto-cjk-fallback".to_owned(),
        FontData::from_owned(bytes).into(),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let family_fonts = fonts.families.entry(family).or_default();
        if !family_fonts.iter().any(|name| name == "noto-cjk-fallback") {
            family_fonts.push("noto-cjk-fallback".to_owned());
        }
    }
    context.set_fonts(fonts);
}

#[cfg(test)]
mod cjk_font_tests {
    use super::cjk_font_candidates;

    /// 便携/发布形态必须能命中"exe 同目录的 fonts/"，否则绿色版中文必然是豆腐块。
    ///
    /// 这条测试盯住的是一次真实的可移植性缺陷：候选表原先只有编译期路径
    /// （`CARGO_MANIFEST_DIR`），把整目录拷到别的 Windows 主机后那条路径不存在。
    #[test]
    fn exe_adjacent_font_is_a_candidate() {
        let candidates = cjk_font_candidates();
        assert!(
            candidates.len() >= 3,
            "至少要有 exe 旁/编译期两类候选：{candidates:?}"
        );
        let exe_dir = std::env::current_exe()
            .expect("current_exe")
            .parent()
            .expect("exe parent")
            .to_path_buf();
        assert!(
            candidates.contains(&exe_dir.join("fonts").join("NotoSansSC-VF.ttf")),
            "exe 同目录的 fonts/NotoSansSC-VF.ttf 必须在候选里：{candidates:?}"
        );
        // 开发形态仍然要命中源码树里的那份（否则本机开发会没字体）。
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.ends_with("fonts/NotoSansSC-VF.ttf")
                    && candidate.to_string_lossy().contains("crates")),
            "编译期源码树路径仍应保留：{candidates:?}"
        );
    }
}

/// 在资源管理器中打开目录（ShellExecuteW，比 `explorer <path>` 更可靠）。
///
/// **测试下是 no-op**：`menu.rs::every_menu_action_is_executable_without_panicking` 会逐个执行
/// 菜单动作（含「帮助 ▸ 使用手册」→ `open_help`），真去 ShellExecuteW 会让 `cargo test`
/// 每次凭空弹出资源管理器窗口（用户看到的"docs 目录自己打开"就是这个）。副作用只在
/// `cfg(test)` 关掉，生产行为一字未变。
#[cfg(all(windows, not(test)))]
pub(crate) fn open_in_explorer(path: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let path_wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let op_wide: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op_wide.as_ptr(),
            path_wide.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

#[cfg(any(not(windows), test))]
pub(crate) fn open_in_explorer(_path: &Path) {}
