//! 编辑器通用工具函数：路径清理、项目错误格式化、中文字体回退、目录选择。

use editor::ProjectError;
use eframe::egui::{self, FontData, FontDefinitions, FontFamily, Response, Ui};
use std::{
    collections::BTreeSet,
    env,
    path::{Path, PathBuf},
};

/// 组件类型的中文显示名（**仅界面用**）。
///
/// 组件名同时是存盘数据、命令与 AI 契约里的标识，因此底层一律保留英文；
/// 这里只负责属性面板标题、组件菜单等处的展示。未登记的组件原样返回。
pub(crate) fn component_display_name(name: &str) -> &str {
    match name {
        "Transform" => "变换",
        "Camera" => "相机",
        "Sprite" => "精灵",
        "Tilemap" => "瓦片地图",
        "Persistent" => "跨场景保留",
        "Animation" => "动画状态机",
        "AnimationPlayer" => "动画播放器",
        "Animation3D" => "3D 动画",
        "MeshRenderer" => "网格渲染器",
        "Light" => "灯光",
        "Collider" => "2D 碰撞体",
        "Collider3d" => "3D 碰撞体",
        "Text" => "文本",
        "WorldText" => "世界文本",
        "Image" => "图片",
        "UiLayout" => "界面布局",
        "Container" => "容器",
        "LayoutItem" => "布局项",
        "UiEvents" => "界面事件",
        "UiBindings" => "属性绑定",
        "ButtonStates" => "按钮状态",
        "UiAnimation" => "界面动画",
        "Script" => "Lua 脚本",
        "Environment" => "环境",
        "Quad" => "四边形",
        "RenderMode" => "渲染模式",
        "Visibility" => "可见性",
        "ParticleEmitter" => "粒子发射器",
        "ScrollView" => "滚动视图",
        "Mask" => "矩形遮罩",
        "CanvasGroup" => "画布组",
        _ => name,
    }
}

/// 控件类型的中文显示名（调色板/新建菜单/UI 工作室层级用；类型标识仍是英文）。
pub(crate) fn widget_display_name(kind: &str) -> &str {
    match kind {
        "Text" | "text" => "文本",
        "Image" | "image" => "图片",
        "Button" | "button" => "按钮",
        "Panel" | "panel" => "面板",
        "Container" | "container" => "容器",
        "toggle" => "开关",
        "slider" => "滑块",
        "progress" => "进度条",
        "input" => "输入框",
        "scroll_view" => "滚动视图",
        "list" => "列表",
        "grid" => "网格",
        "tabs" => "标签页",
        "modal" => "模态框",
        "tooltip" => "提示气泡",
        "spacer" => "间隔",
        "safe_area" => "安全区",
        _ => kind,
    }
}

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
    /// 这条测试盯住的是**用户看到的那个按钮**：曾经用 U+275A 双竖杠字形，字体缺字形时渲染成
    /// 两个"豆腐块"（用户截图反馈）；现在两种状态都由画笔按这里的选择绘制。
    #[test]
    fn pause_button_shows_the_opposite_action_icon() {
        assert_eq!(pause_resume_icon(false), PlaybackIcon::Pause);
        assert_eq!(pause_resume_icon(true), PlaybackIcon::Play);
    }
}

#[cfg(test)]
mod draw_icon_tests {
    use super::{egui, pencil_polygons};

    /// 铅笔几何：笔尖必须在**左下**（那才是写字的那头），笔杆向右上延伸，两块图形都
    /// 不能越出分配的矩形。这些坐标写反不会有任何编译期信号，只能靠这里钉住。
    ///
    /// 矩形刻意取一个**非零原点**：早先的实现只按宽高归一、忘了加原点，铅笔就画到了
    /// 屏幕左上角，行末看上去像"按钮凭空消失"（目验截图才发现）。
    #[test]
    fn pencil_points_down_left_and_stays_in_its_box() {
        let rect = egui::Rect::from_min_size(egui::pos2(120.0, 45.0), egui::vec2(16.0, 16.0));
        let (shaft, tip) = pencil_polygons(rect);
        assert_eq!(shaft.len(), 4, "笔杆是斜放的四边形");
        assert_eq!(tip.len(), 3, "笔尖是三角形");
        for point in shaft.iter().chain(tip.iter()) {
            assert!(rect.contains(*point), "图形越出 {rect:?}：{point:?}");
        }
        // 屏幕坐标 y 向下：笔尖是"最靠左、最靠下"的那个点。
        let tip_point = tip
            .iter()
            .min_by(|a, b| a.x.partial_cmp(&b.x).unwrap())
            .copied()
            .unwrap();
        assert!(
            tip_point.x < shaft.iter().map(|p| p.x).reduce(f32::min).unwrap(),
            "笔尖没有比笔杆更靠左：{tip_point:?} vs {shaft:?}"
        );
        assert!(
            tip_point.y > shaft.iter().map(|p| p.y).reduce(f32::max).unwrap(),
            "笔尖没有比笔杆更靠下：{tip_point:?} vs {shaft:?}"
        );
        // 笔杆末端应接近矩形右上角（留出的边距不能吃掉半个方框）。
        let far = shaft
            .iter()
            .max_by(|a, b| {
                (a.x - a.y)
                    .partial_cmp(&(b.x - b.y))
                    .unwrap()
                    .then(a.x.partial_cmp(&b.x).unwrap())
            })
            .copied()
            .unwrap();
        assert!(
            far.x > rect.min.x + rect.width() * 0.6,
            "笔杆没有伸向右上：{far:?}"
        );
    }

    /// 豆腐守卫：这几个码位在编辑器的字体链里**没有字形**（实测 cmap：内嵌的
    /// Ubuntu-Light / Hack / NotoEmoji / emoji-icon-font 加上 `noto-cjk-fallback`
    /// 均未覆盖），源码里一旦再次出现就必然渲染成"豆腐块"（用户截图反馈的成因）。
    /// 需要这些图标时走 [`super::DrawIcon`] 的画笔绘制。
    ///
    /// 注意这条测试连**注释**一起管：所以注释里要用 `U+2715` 这种码点写法描述它们，
    /// 免得有人把"文档里的示例字符"抄成真实字面量，而审查者看不出区别。
    #[test]
    fn editor_sources_avoid_glyphless_dingbats() {
        const FORBIDDEN: [char; 4] = ['\u{270E}', '\u{2715}', '\u{2718}', '\u{275A}'];
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![src];
        let mut offenders = Vec::new();
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.filter_map(|entry| entry.ok()) {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                for (index, line) in text.lines().enumerate() {
                    for ch in FORBIDDEN {
                        if line.contains(ch) {
                            offenders.push(format!(
                                "{}:{} 出现 U+{:04X}",
                                path.display(),
                                index + 1,
                                ch as u32
                            ));
                        }
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "这些码位没有字形，会渲染成豆腐块：\n{}",
            offenders.join("\n")
        );
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

/// 暂停/继续按钮：**图标用画笔绘制**，不依赖字体里有 U+275A 这类字形。
///
/// 起因（用户反馈的截图）：原来按钮文字用 U+275A（成对竖杠），而编辑器配置的字体里没有这个
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
/// 不用 `▾` 字形的原因与 [`pause_resume_icon_button`] 的 U+275A 一样——字体缺字形渲染成豆腐块。
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

/// 由画笔绘制的图形标记。这里列出的符号全部来自 Dingbats 区（U+2700–U+27BF），
/// 而编辑器的字体链（内嵌拉丁字体 + `noto-cjk-fallback`）**没有这些字形**，写成文字
/// 只会渲染成"豆腐块"（用户截图反馈）。因此这些位置一律按本枚举用画笔绘制，
/// 与字体安装情况无关。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DrawIcon {
    /// 斜铅笔：编辑入口（如「打开预制体编辑」）。
    Pencil,
    /// 叉：缺失 / 导入失败 / 清除。
    Cross,
    /// 勾：成功。
    Check,
}

/// 铅笔的两块几何（笔杆四边形 + 笔尖三角形），坐标落在 `rect` 内。
///
/// 抽成纯函数是为了让单测能钉住形状：笔尖必须朝左下（那才是"写字的那头"），
/// 而画笔调用点里的坐标算错了不会有任何编译期信号。**返回绝对坐标**（带 rect 原点）
/// 也是有意的：只按宽高归一的话，图形会被画到屏幕左上角，面板里一个字都看不见
/// ——目验截图时踩过一次，所以单测刻意用非零原点的矩形。
#[must_use]
pub(crate) fn pencil_polygons(rect: egui::Rect) -> (Vec<egui::Pos2>, Vec<egui::Pos2>) {
    let (w, h) = (rect.width(), rect.height());
    let at = |fx: f32, fy: f32| egui::pos2(rect.min.x + fx * w, rect.min.y + fy * h);
    // 归一化：杆从 (0.34,0.66) 到 (0.72,0.28)，粗 0.16；笔尖收在 (0.22,0.78)。
    let (h1, h2) = (at(0.34, 0.66), at(0.72, 0.28));
    let (dx, dy) = (h2.x - h1.x, h2.y - h1.y);
    let len = (dx * dx + dy * dy).sqrt().max(f32::EPSILON);
    let (nx, ny) = (-dy / len * 0.08 * w, dx / len * 0.08 * h);
    let shaft = vec![
        egui::pos2(h1.x + nx, h1.y + ny),
        egui::pos2(h2.x + nx, h2.y + ny),
        egui::pos2(h2.x - nx, h2.y - ny),
        egui::pos2(h1.x - nx, h1.y - ny),
    ];
    let tip = vec![
        at(0.22, 0.78),
        egui::pos2(h1.x + nx, h1.y + ny),
        egui::pos2(h1.x - nx, h1.y - ny),
    ];
    (shaft, tip)
}

/// 在 `rect` 内按 `icon` 画一个图形，颜色由调用方决定。
fn paint_draw_icon(ui: &mut Ui, rect: egui::Rect, icon: DrawIcon, color: egui::Color32) {
    let origin = rect.min;
    let size = rect.size();
    let at = |fx: f32, fy: f32| egui::pos2(origin.x + fx * size.x, origin.y + fy * size.y);
    match icon {
        DrawIcon::Pencil => {
            let (shaft, tip) = pencil_polygons(rect);
            ui.painter().add(egui::Shape::convex_polygon(
                shaft,
                color,
                egui::Stroke::NONE,
            ));
            ui.painter()
                .add(egui::Shape::convex_polygon(tip, color, egui::Stroke::NONE));
        }
        DrawIcon::Cross => {
            let stroke = egui::Stroke::new(1.4_f32, color);
            ui.painter()
                .line_segment([at(0.28, 0.28), at(0.72, 0.72)], stroke);
            ui.painter()
                .line_segment([at(0.72, 0.28), at(0.28, 0.72)], stroke);
        }
        DrawIcon::Check => {
            ui.painter().add(egui::Shape::line(
                vec![at(0.20, 0.54), at(0.42, 0.74), at(0.80, 0.28)],
                egui::Stroke::new(1.4_f32, color),
            ));
        }
    }
}

/// 行内图形标记（不可点击）：宽高取当前正文行高，与同行文字基线对齐。
pub(crate) fn draw_icon_label(ui: &mut Ui, icon: DrawIcon, color: egui::Color32) {
    let side = ui
        .text_style_height(&egui::TextStyle::Body)
        .clamp(10.0, 20.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(side + 2.0, side), egui::Sense::hover());
    // 图形居中于分配到的方框，尺寸用行高而不是整个方框，避免勾/叉顶到上下边。
    let box_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(side, side));
    paint_draw_icon(ui, box_rect, icon, color);
}

/// 图形小按钮：外观走 egui 自己的按钮视觉（hover/按下反馈一致），图标由画笔绘制。
pub(crate) fn draw_icon_button(ui: &mut Ui, icon: DrawIcon, tooltip: &str) -> Response {
    let height = ui
        .text_style_height(&egui::TextStyle::Button)
        .max(ui.spacing().button_padding.y * 2.0)
        + 2.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2((height + 6.0).min(26.0), height),
        egui::Sense::click(),
    );
    let visuals = ui.style().interact(&response);
    if response.hovered() || response.is_pointer_button_down_on() {
        ui.painter()
            .rect_filled(rect, visuals.corner_radius, visuals.bg_fill);
    }
    let side = height - 4.0;
    let box_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(side, side));
    paint_draw_icon(ui, box_rect, icon, visuals.fg_stroke.color);
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

/// 编辑期内嵌中文字体的文件名（候选路径都以它结尾）。
pub(crate) const CJK_FONT_FILE_NAME: &str = "NotoSansSC-VF.ttf";

/// 中文字体候选路径，按优先级排列。
///
/// **为什么不能只用编译期路径**：原先这里写死 `env!("CARGO_MANIFEST_DIR")/fonts/…`，
/// 也就是构建机的绝对路径（`E:\Codex\RustLua\engine\crates\editor\fonts\…`）。
/// 绿色版（免安装、整目录拷到别的 Windows 主机）上那条路径根本不存在，
/// 于是中文全变成"豆腐块"——一个和包本身无关、却会让跨机验收直接失败的坑。
///
/// 优先级：显式环境变量 → **exe 同目录**（发布/便携形态）→
/// **从 exe 向上找回工作区里的那份**（本机开发形态）→ 编译期路径。
#[must_use]
pub(crate) fn cjk_font_candidates() -> Vec<PathBuf> {
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
        candidates.push(dir.join("fonts").join(CJK_FONT_FILE_NAME));
        // 允许把字体直接摆在 exe 旁边
        candidates.push(dir.join(CJK_FONT_FILE_NAME));
        // 开发形态：exe 在 <工作区>/engine/target/<profile>/ 下，向上爬能找到
        // crates/editor/fonts 里的那份（见 workspace_font_from_exe）。
        candidates.extend(workspace_font_from_exe(dir));
    }
    // 最后才试编译期源码树路径——它可能属于一台已经不存在的构建机。
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fonts")
            .join(CJK_FONT_FILE_NAME),
    );
    candidates
}

/// 从 exe 所在目录逐级向上，找回仓库里的那份编辑期中文字体。
///
/// **为什么要这一条**：质量门会在 `%TEMP%` 的一次性 worktree 里用**共享 target**
/// 构建，于是烘进 exe 的 `CARGO_MANIFEST_DIR` 指向一个门跑完就被删掉的目录
/// （实测症状：本机 `target/debug/editor.exe` 中文全为方框）。exe 本身一定落在
/// 真实工作区内，向上爬比编译期路径可靠。
fn workspace_font_from_exe(exe_dir: &Path) -> Option<PathBuf> {
    exe_dir.ancestors().find_map(|dir| {
        ["crates/editor/fonts", "engine/crates/editor/fonts"]
            .iter()
            .map(|relative| {
                relative
                    .split('/')
                    .fold(dir.to_path_buf(), |path, part| path.join(part))
                    .join(CJK_FONT_FILE_NAME)
            })
            .find(|path| path.is_file())
    })
}

/// 第一个真实存在的中文字体候选（全都不存在时 `None`）。
pub(crate) fn resolve_cjk_font() -> Option<PathBuf> {
    cjk_font_candidates()
        .into_iter()
        .find(|candidate| candidate.is_file())
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

    /// 门构建的 exe 里编译期路径指向已删除的 `%TEMP%` worktree，中文会全变方框：
    /// 候选必须还能从 exe 位置（`<工作区>/engine/target/<profile>/`）向上爬回源码树。
    #[test]
    fn workspace_font_is_found_by_walking_up_from_the_exe() {
        let crate_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        // 假想的 exe 位置：<工作区>/engine/target/debug（编译期路径完全不可见）。
        let engine_dir = crate_dir
            .parent()
            .and_then(std::path::Path::parent)
            .expect("crates/editor 上溯两级即 engine 目录");
        let found = super::workspace_font_from_exe(&engine_dir.join("target").join("debug"))
            .expect("从 target/debug 向上应找回 editor crate 里的字体");
        assert_eq!(
            found,
            crate_dir.join("fonts").join(super::CJK_FONT_FILE_NAME),
            "向上爬要命中的正是 crate 里那份字体"
        );
        // 候选表必须带上这条路径，且排在最后的编译期候选之前。
        let candidates = super::cjk_font_candidates();
        let workspace_rank = candidates
            .iter()
            .position(|candidate| candidate == &found)
            .expect("工作区字体必须在候选表里");
        assert!(
            workspace_rank + 1 < candidates.len(),
            "工作区路径要排在编译期路径之前：{candidates:?}"
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
