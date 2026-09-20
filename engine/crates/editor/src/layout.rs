//! Persisted editor layout / workspace state (docs/55 P1 + 阶段 3).
//!
//! Requirements from the plan: "可保存的停靠布局、默认布局、布局重置和损坏恢复", with the
//! acceptance "布局 100 次保存/恢复". That is exactly what this module provides:
//!
//! * a small, versioned, human-readable file at `<project>/.cache/editor/layout.json`
//!   (same place and style as `hierarchy-state.json`, and equally outside the scene format);
//! * **tolerant loading**: a missing file, a truncated file, or a value of the wrong type
//!   never panics — it falls back to defaults and reports why;
//! * **forward compatibility**: unknown top-level keys are preserved across a save, so a newer
//!   build's extra settings survive an older build writing the file;
//! * **round-trip stability**: save → load → save is byte-identical (locked by a 100-cycle test).
//!
//! Serialization is written by hand over `serde_json::Value` (the crate has `serde_json` but no
//! serde derive), matching how `util.rs` persists the Hierarchy collapse state.

use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

/// Bumped when the on-disk shape changes in a way that needs migration.
pub const LAYOUT_VERSION: u32 = 1;

/// Panel keys used in `panel_widths`. Kept as data so unknown keys are preserved.
pub const PANEL_RESOURCE: &str = "resource";
pub const PANEL_HIERARCHY: &str = "hierarchy";
pub const PANEL_INSPECTOR: &str = "inspector";
pub const PANEL_CONSOLE: &str = "console";

/// 面板宽度的合法范围（点）：过窄会裁字，过宽会挤掉 Scene View。
const PANEL_WIDTH_MIN: f32 = 120.0;
const PANEL_WIDTH_MAX: f32 = 900.0;
const ZOOM_MIN: f32 = 0.1;
const ZOOM_MAX: f32 = 8.0;

/// 工作台标签 / 视图模式 / 工具 / 密度的合法取值。
const KNOWN_TABS: &[&str] = &["scene", "game"];
const KNOWN_VIEW_MODES: &[&str] = &["2d", "3d", "prefab", "widget"];
const KNOWN_TOOLS: &[&str] = &["select", "move", "rotate", "scale"];
const KNOWN_DENSITIES: &[&str] = &["compact", "comfortable"];
/// Gizmo 变换空间（docs/55 P1 的 Local/World）。
const KNOWN_GIZMO_SPACES: &[&str] = &["world", "local"];
/// Gizmo 轴心模式（docs/55 P1 的 Pivot/Center）。
const KNOWN_GIZMO_PIVOTS: &[&str] = &["pivot", "center"];

/// One persisted layout.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorLayout {
    pub version: u32,
    /// 面板宽度（点），key 见 `PANEL_*`。
    pub panel_widths: BTreeMap<String, f32>,
    /// 面板可见性（资源面板/控制台等）。
    pub panel_visible: BTreeMap<String, bool>,
    pub workspace_tab: String,
    pub scene_view_mode: String,
    pub scene_tool: String,
    /// Gizmo 变换空间：`world` / `local`。
    pub gizmo_space: String,
    /// Gizmo 轴心模式：`pivot` / `center`。
    pub gizmo_pivot: String,
    pub scene_zoom: f32,
    pub preview_resolution: (u32, u32),
    pub density: String,
}

impl Default for EditorLayout {
    fn default() -> Self {
        Self {
            version: LAYOUT_VERSION,
            panel_widths: BTreeMap::from([
                (PANEL_RESOURCE.to_owned(), 200.0),
                (PANEL_HIERARCHY.to_owned(), 240.0),
                (PANEL_INSPECTOR.to_owned(), 360.0),
                (PANEL_CONSOLE.to_owned(), 150.0),
            ]),
            panel_visible: BTreeMap::from([
                (PANEL_RESOURCE.to_owned(), true),
                (PANEL_HIERARCHY.to_owned(), true),
                (PANEL_INSPECTOR.to_owned(), true),
                (PANEL_CONSOLE.to_owned(), true),
            ]),
            workspace_tab: "scene".to_owned(),
            scene_view_mode: "2d".to_owned(),
            scene_tool: "select".to_owned(),
            gizmo_space: "world".to_owned(),
            gizmo_pivot: "pivot".to_owned(),
            scene_zoom: 1.0,
            preview_resolution: (1280, 720),
            density: "compact".to_owned(),
        }
    }
}

impl EditorLayout {
    #[must_use]
    pub fn new_default() -> Self {
        Self::default()
    }

    /// 该面板宽度（缺失或非法时回落到默认值）。
    #[must_use]
    pub fn panel_width(&self, panel: &str) -> f32 {
        self.panel_widths
            .get(panel)
            .copied()
            .filter(|width| width.is_finite())
            .unwrap_or_else(|| {
                Self::default()
                    .panel_widths
                    .get(panel)
                    .copied()
                    .unwrap_or(240.0)
            })
    }

    /// 面板是否可见（缺失时默认可见）。
    #[must_use]
    pub fn panel_is_visible(&self, panel: &str) -> bool {
        self.panel_visible.get(panel).copied().unwrap_or(true)
    }

    /// 收敛非法取值（不 panic）：供 `load` 与外部直接构造后调用。
    pub fn clamp(&mut self) {
        if self.version == 0 {
            self.version = LAYOUT_VERSION;
        }
        for (key, width) in self.panel_widths.iter_mut() {
            let fallback = Self::default().panel_width(key);
            if !width.is_finite() {
                *width = fallback;
            }
            *width = width.clamp(PANEL_WIDTH_MIN, PANEL_WIDTH_MAX);
        }
        if !self.scene_zoom.is_finite() {
            self.scene_zoom = 1.0;
        }
        self.scene_zoom = self.scene_zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        let (width, height) = self.preview_resolution;
        if width == 0 || height == 0 || width > 16_384 || height > 16_384 {
            self.preview_resolution = (1280, 720);
        }
        if !KNOWN_TABS.contains(&self.workspace_tab.as_str()) {
            self.workspace_tab = "scene".to_owned();
        }
        if !KNOWN_VIEW_MODES.contains(&self.scene_view_mode.as_str()) {
            self.scene_view_mode = "2d".to_owned();
        }
        if !KNOWN_TOOLS.contains(&self.scene_tool.as_str()) {
            self.scene_tool = "select".to_owned();
        }
        if !KNOWN_GIZMO_SPACES.contains(&self.gizmo_space.as_str()) {
            self.gizmo_space = "world".to_owned();
        }
        if !KNOWN_GIZMO_PIVOTS.contains(&self.gizmo_pivot.as_str()) {
            self.gizmo_pivot = "pivot".to_owned();
        }
        if !KNOWN_DENSITIES.contains(&self.density.as_str()) {
            self.density = "compact".to_owned();
        }
    }

    /// 校验（`clamp` 已保证合法，这里给出显式报告供测试/诊断使用）。
    pub fn validate(&self) -> Result<(), String> {
        if self.version == 0 {
            return Err("version 不得为 0".to_owned());
        }
        for (key, width) in &self.panel_widths {
            if !width.is_finite() {
                return Err(format!("面板 {key} 宽度不是有限数"));
            }
            if *width < PANEL_WIDTH_MIN || *width > PANEL_WIDTH_MAX {
                return Err(format!("面板 {key} 宽度越界：{width}"));
            }
        }
        if !self.scene_zoom.is_finite() || self.scene_zoom < ZOOM_MIN || self.scene_zoom > ZOOM_MAX
        {
            return Err(format!("缩放越界：{}", self.scene_zoom));
        }
        Ok(())
    }

    /// 序列化为 JSON（未知键由 `save` 负责合并保留）。
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "version": self.version,
            "panel_widths": self.panel_widths,
            "panel_visible": self.panel_visible,
            "workspace_tab": self.workspace_tab,
            "scene_view_mode": self.scene_view_mode,
            "scene_tool": self.scene_tool,
            "gizmo_space": self.gizmo_space,
            "gizmo_pivot": self.gizmo_pivot,
            "scene_zoom": self.scene_zoom,
            "preview_resolution": [self.preview_resolution.0, self.preview_resolution.1],
            "density": self.density,
        })
    }

    /// 从 JSON 宽松解析：缺字段/类型不符回落到默认值，非法值收敛。
    #[must_use]
    pub fn from_json(value: &serde_json::Value) -> Self {
        let mut layout = Self::default();
        if let Some(version) = value.get("version").and_then(serde_json::Value::as_u64) {
            layout.version = version as u32;
        }
        if let Some(widths) = value.get("panel_widths").and_then(|v| v.as_object()) {
            for (key, width) in widths {
                if let Some(width) = width.as_f64() {
                    layout.panel_widths.insert(key.clone(), width as f32);
                }
            }
        }
        if let Some(visible) = value.get("panel_visible").and_then(|v| v.as_object()) {
            for (key, value) in visible {
                if let Some(value) = value.as_bool() {
                    layout.panel_visible.insert(key.clone(), value);
                }
            }
        }
        if let Some(tab) = value.get("workspace_tab").and_then(|v| v.as_str()) {
            layout.workspace_tab = tab.to_owned();
        }
        if let Some(mode) = value.get("scene_view_mode").and_then(|v| v.as_str()) {
            layout.scene_view_mode = mode.to_owned();
        }
        if let Some(tool) = value.get("scene_tool").and_then(|v| v.as_str()) {
            layout.scene_tool = tool.to_owned();
        }
        if let Some(space) = value.get("gizmo_space").and_then(|v| v.as_str()) {
            layout.gizmo_space = space.to_owned();
        }
        if let Some(pivot) = value.get("gizmo_pivot").and_then(|v| v.as_str()) {
            layout.gizmo_pivot = pivot.to_owned();
        }
        if let Some(zoom) = value.get("scene_zoom").and_then(serde_json::Value::as_f64) {
            layout.scene_zoom = zoom as f32;
        }
        if let Some([width, height]) = value
            .get("preview_resolution")
            .and_then(|v| v.as_array())
            .and_then(|values| match values.as_slice() {
                [width, height] => Some([width.as_u64(), height.as_u64()]),
                _ => None,
            })
            .and_then(|[width, height]| Some([width?, height?]))
        {
            layout.preview_resolution = (width as u32, height as u32);
        }
        if let Some(density) = value.get("density").and_then(|v| v.as_str()) {
            layout.density = density.to_owned();
        }
        layout.clamp();
        layout
    }
}

/// 加载结果：调用方据此决定提示与是否落盘。
#[derive(Clone, Debug, PartialEq)]
pub enum LayoutLoad {
    /// 读到并收敛后的布局。
    Loaded(EditorLayout),
    /// 文件不存在（首次打开项目）：用默认布局，**不算错误**。
    Missing,
    /// 文件存在但损坏/非法：用默认布局，并给出原因（**不覆盖**源文件）。
    Fallback { reason: String },
}

impl LayoutLoad {
    /// 无论成功与否都拿到一份可用布局。
    #[must_use]
    pub fn layout(&self) -> EditorLayout {
        match self {
            Self::Loaded(layout) => layout.clone(),
            Self::Missing | Self::Fallback { .. } => EditorLayout::default(),
        }
    }

    #[must_use]
    pub fn is_fallback(&self) -> bool {
        matches!(self, Self::Fallback { .. })
    }

    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Fallback { reason } => Some(reason),
            _ => None,
        }
    }
}

/// `<project>/.cache/editor/layout.json`（与 hierarchy-state.json 同目录，不进场景格式）。
#[must_use]
pub fn layout_path(project_root: &Path) -> PathBuf {
    project_root
        .join(".cache")
        .join("editor")
        .join("layout.json")
}

/// 读取布局；任何问题都退化为默认布局而不 panic。
#[must_use]
pub fn load_layout(project_root: &Path) -> LayoutLoad {
    let path = layout_path(project_root);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return LayoutLoad::Missing;
    };
    let value = match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(value) => value,
        Err(error) => {
            return LayoutLoad::Fallback {
                reason: format!("布局文件无法解析：{error}"),
            };
        }
    };
    if !value.is_object() {
        return LayoutLoad::Fallback {
            reason: "布局文件顶层不是对象".to_owned(),
        };
    }
    let layout = EditorLayout::from_json(&value);
    match layout.validate() {
        Ok(()) => LayoutLoad::Loaded(layout),
        Err(reason) => LayoutLoad::Fallback { reason },
    }
}

/// 原子落盘（tmp + rename），并**保留**文件里的未知顶层键。
pub fn save_layout(project_root: &Path, layout: &EditorLayout) -> io::Result<()> {
    let path = layout_path(project_root);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;

    // 合并已有未知键：新版本写的设置不会被旧版本抹掉。
    let mut value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    {
        let object = value
            .as_object_mut()
            .expect("value was normalized to an object");
        for (key, incoming) in layout
            .to_json()
            .as_object()
            .expect("to_json yields an object")
        {
            object.insert(key.clone(), incoming.clone());
        }
    }
    let text = serde_json::to_string_pretty(&value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text.as_bytes())?;
    std::fs::rename(&temporary, &path)?;
    Ok(())
}

/// 重置为默认布局并落盘。
pub fn reset_layout(project_root: &Path) -> io::Result<EditorLayout> {
    let layout = EditorLayout::default();
    save_layout(project_root, &layout)?;
    Ok(layout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "rustlua-layout-{label}-{}-{counter}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");
        root
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let root = temp_root("missing");
        let load = load_layout(&root);
        assert_eq!(load, LayoutLoad::Missing);
        assert!(!load.is_fallback());
        assert_eq!(load.layout(), EditorLayout::default());
        let _ = std::fs::remove_dir_all(root);
    }

    /// Gizmo 变换空间（第 43 轮 P1）随布局持久化；未知/损坏值收敛回 `world`。
    #[test]
    fn gizmo_space_persists_and_converges() {
        let root = temp_root("gizmo-space");
        let layout = EditorLayout {
            gizmo_space: "local".to_owned(),
            gizmo_pivot: "center".to_owned(),
            ..EditorLayout::default()
        };
        save_layout(&root, &layout).expect("save");
        match load_layout(&root) {
            LayoutLoad::Loaded(loaded) => {
                assert_eq!(loaded.gizmo_space, "local", "局部空间必须被记住");
                assert_eq!(loaded.gizmo_pivot, "center", "中心轴心必须被记住");
                assert_eq!(loaded, layout);
            }
            other => panic!("加载失败：{other:?}"),
        }
        // 损坏值（手改/降级）不得让编辑器进入未知模式。
        let mut broken = layout.clone();
        broken.gizmo_space = "pivot".to_owned();
        broken.gizmo_pivot = "world".to_owned();
        broken.clamp();
        assert_eq!(broken.gizmo_space, "world");
        assert_eq!(broken.gizmo_pivot, "pivot");
        // 缺字段的老布局文件 → 默认值（前向兼容）。
        let mut value = layout.to_json();
        value.as_object_mut().expect("object").remove("gizmo_space");
        value.as_object_mut().expect("object").remove("gizmo_pivot");
        let restored = EditorLayout::from_json(&value);
        assert_eq!(restored.gizmo_space, "world");
        assert_eq!(restored.gizmo_pivot, "pivot");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn round_trip_is_stable_for_one_hundred_cycles() {
        // docs/55 阶段 3 验收：布局 100 次保存/恢复。
        let root = temp_root("cycles");
        let mut layout = EditorLayout::default();
        layout
            .panel_widths
            .insert(PANEL_INSPECTOR.to_owned(), 420.0);
        layout.workspace_tab = "game".to_owned();
        layout.scene_zoom = 2.5;
        layout.preview_resolution = (1920, 1080);
        layout.density = "comfortable".to_owned();
        save_layout(&root, &layout).expect("first save");
        let first_bytes = std::fs::read(layout_path(&root)).expect("read");

        for cycle in 0..100 {
            let loaded = load_layout(&root);
            let loaded = match loaded {
                LayoutLoad::Loaded(layout) => layout,
                other => panic!("第 {cycle} 次加载失败：{other:?}"),
            };
            assert_eq!(loaded, layout, "第 {cycle} 次往返必须完全一致");
            save_layout(&root, &loaded).expect("save");
        }
        let final_bytes = std::fs::read(layout_path(&root)).expect("read");
        assert_eq!(
            first_bytes, final_bytes,
            "100 次保存/恢复后文件必须逐字节一致"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_files_fall_back_without_panicking() {
        let root = temp_root("corrupt");
        let path = layout_path(&root);
        std::fs::create_dir_all(path.parent().unwrap()).expect("dir");

        for (label, text) in [
            ("truncated", "{\"version\": 1, \"panel_widths\": {"),
            ("not-object", "[1, 2, 3]"),
            ("garbage", "\u{0}\u{1}not json at all"),
            ("empty", ""),
        ] {
            std::fs::write(&path, text).expect("write");
            let load = load_layout(&root);
            assert!(load.is_fallback(), "{label} 必须走回落路径：{load:?}");
            assert!(load.reason().is_some(), "{label} 必须给出原因");
            assert_eq!(
                load.layout(),
                EditorLayout::default(),
                "{label} 回落后必须是默认布局"
            );
            // 损坏文件不被静默覆盖：内容保持原样，用户可见可修。
            assert_eq!(std::fs::read_to_string(&path).expect("read"), text);
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn out_of_range_values_are_clamped_and_reported() {
        let mut layout = EditorLayout::default();
        layout
            .panel_widths
            .insert(PANEL_HIERARCHY.to_owned(), f32::NAN);
        layout.panel_widths.insert(PANEL_RESOURCE.to_owned(), -50.0);
        layout
            .panel_widths
            .insert(PANEL_INSPECTOR.to_owned(), 9_999.0);
        layout.scene_zoom = f32::INFINITY;
        layout.preview_resolution = (0, 0);
        layout.workspace_tab = "nope".to_owned();
        layout.scene_view_mode = "4d".to_owned();
        layout.density = "huge".to_owned();
        assert!(layout.validate().is_err());
        layout.clamp();
        assert!(layout.validate().is_ok(), "{layout:?}");
        assert_eq!(layout.panel_width(PANEL_HIERARCHY), 240.0);
        assert_eq!(layout.panel_width(PANEL_RESOURCE), PANEL_WIDTH_MIN);
        assert_eq!(layout.panel_width(PANEL_INSPECTOR), PANEL_WIDTH_MAX);
        assert_eq!(layout.scene_zoom, 1.0);
        assert_eq!(layout.preview_resolution, (1280, 720));
        assert_eq!(layout.workspace_tab, "scene");
        assert_eq!(layout.scene_view_mode, "2d");
        assert_eq!(layout.density, "compact");
    }

    #[test]
    fn unknown_top_level_keys_survive_a_save() {
        // 前向兼容：新版本写的键，旧版本保存布局时不得抹掉。
        let root = temp_root("forward");
        let path = layout_path(&root);
        std::fs::create_dir_all(path.parent().unwrap()).expect("dir");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&serde_json::json!({
                "version": 1,
                "future_docking": { "tab": "left" },
                "panel_widths": { "inspector": 300.0 },
            }))
            .expect("json"),
        )
        .expect("write");

        let load = load_layout(&root);
        let mut layout = load.layout();
        layout.workspace_tab = "game".to_owned();
        save_layout(&root, &layout).expect("save");

        let text = std::fs::read_to_string(&path).expect("read");
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(
            value.get("future_docking"),
            Some(&serde_json::json!({ "tab": "left" })),
            "未知键必须保留"
        );
        assert_eq!(
            value.get("workspace_tab").and_then(|v| v.as_str()),
            Some("game")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reset_writes_defaults() {
        let root = temp_root("reset");
        let layout = EditorLayout {
            scene_zoom: 4.0,
            workspace_tab: "game".to_owned(),
            ..EditorLayout::default()
        };
        save_layout(&root, &layout).expect("save");

        let reset = reset_layout(&root).expect("reset");
        assert_eq!(reset, EditorLayout::default());
        assert_eq!(load_layout(&root).layout(), EditorLayout::default());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn partial_files_keep_defaults_for_missing_fields() {
        let root = temp_root("partial");
        let path = layout_path(&root);
        std::fs::create_dir_all(path.parent().unwrap()).expect("dir");
        std::fs::write(&path, "{\"version\": 1, \"scene_zoom\": 3.0}").expect("write");

        let load = load_layout(&root);
        assert!(matches!(load, LayoutLoad::Loaded(_)), "{load:?}");
        let layout = load.layout();
        assert_eq!(layout.scene_zoom, 3.0);
        assert_eq!(layout.workspace_tab, "scene", "缺失字段回落默认");
        assert_eq!(layout.panel_width(PANEL_INSPECTOR), 360.0);
        assert!(layout.panel_is_visible(PANEL_CONSOLE));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn wrong_typed_fields_are_ignored_not_fatal() {
        let root = temp_root("wrong-types");
        let path = layout_path(&root);
        std::fs::create_dir_all(path.parent().unwrap()).expect("dir");
        std::fs::write(
            &path,
            serde_json::to_string(&serde_json::json!({
                "version": "one",
                "panel_widths": { "inspector": "wide" },
                "panel_visible": { "console": "yes" },
                "scene_zoom": "big",
                "preview_resolution": ["1280"],
            }))
            .expect("json"),
        )
        .expect("write");

        let load = load_layout(&root);
        assert!(matches!(load, LayoutLoad::Loaded(_)), "{load:?}");
        let layout = load.layout();
        assert_eq!(layout.panel_width(PANEL_INSPECTOR), 360.0);
        assert!(layout.panel_is_visible(PANEL_CONSOLE));
        assert_eq!(layout.scene_zoom, 1.0);
        assert_eq!(layout.preview_resolution, (1280, 720));
        let _ = std::fs::remove_dir_all(root);
    }
}
