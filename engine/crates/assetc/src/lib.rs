//! assetc — 资源构建工具（M7-01）。
//!
//! 扫描项目数据文件（`project.toml`、`scenes/`、`scripts/`、`assets/`），
//! 注册到 [`rustlua_assets::resource_db::ResourceDatabase`]，并通过 [`ImportCache`] 增量导入缓存，
//! 最后可打包为发布资源目录并生成清单。
//!
//! 命令（见 `main.rs`）：`scan` / `import` / `clean` / `list` / `package` / `--check`。

use rustlua_assets_import::import_cache::ImportCache;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

pub mod atlas_cook;
pub mod bundle_plan;
/// 发布闭包检查：计划里的每一条逻辑资源路径都必须在产物容器索引里可解析。
pub mod closure;

/// assetc 打包清单格式版本；变更会触发发布脚本重新打包。
pub const PACKAGE_VERSION: &str = "assetc-package-2";

/// 资源包（RLB）容器版本在 `rustlua_infra::bundle::BUNDLE_VERSION`；这里只做展示用的旁注。
pub const BUNDLE_DIR: &str = "bundles";
/// 包内报告文件名（随包发布，供 CI/发布面板读；**内容必须确定性**，不写时间戳）。
pub const PACKAGE_REPORT_FILE: &str = "package-report.json";

/// 扫描时跳过的本地工作目录。
pub(crate) const SKIP_DIRS: &[&str] = &[".cache", ".temp", ".git", "target"];

/// 生成器产出的**开发用文件**：预览拼图与美术清单。
///
/// 这些文件由 `tools/gen_*.py` 生成、供人看（美术对照图）或供生成器之间共享
/// （`manifest.json` 里的组件串），**游戏运行期不读它们**（引擎与 Lua 侧都没有
/// 读取点，`docs/48` 有清单与理由）。它们留在仓库里（生成器就在美术目录旁边写，
/// 这也是美术期望的位置），但不进发行包：
///   * `_` 前缀：生成器的"非游戏资源"约定（`_preview.png`、`_overview.png`…）；
///   * 名字里带 `preview` 的 PNG：不带下划线的预览（`preview.png`、
///     `season_preview.png`、`title_bg_pixel_preview.png`）；
///   * `manifest.json`：开发期清单（运行期只认 Lua 数据表与场景文件）；
///   * `art/crops/<crop>/stage_<n>.png`：作物出图集的中间帧（运行期只读图集
///     `manor_crops.png` + `data/crop_art.lua` 的 `CropGrid`，逐帧 PNG 没有读取点；
///     2026-10-24 按 P2 资源打包口径定案，见 `docs/48` §1 末）。
///
/// 打包结果由 `scripts/verify/verify.ps1` 的发布自检打印计数（`assetc_package_ok files=<n>`），
/// 判定规则本身由本文件的单测 `dev_artifacts_are_not_packaged` 与
/// `game_jhyt` 的验收样例 `jhyt_m29_season_weather` ⑦（重扫 `assets/**` 并核对
/// `examples/game_jhyt/docs/48` 的决定表）共同守住，因此"预览图混进发行包"不会再悄悄发生。
pub fn is_dev_artifact(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    if lower == "manifest.json" {
        return true;
    }
    if lower.ends_with(".png") && (lower.contains("preview") || is_crop_stage_frame(path, &lower)) {
        return true;
    }
    name.starts_with('_')
}

/// `<…>/crops/<crop>/stage_<n>.png`：出图集的中间帧。祖父目录必须叫 `crops`，
/// 这样图集本体（`crops/manor_crops.png`，场景按 texture 直接引用）不会被误伤。
fn is_crop_stage_frame(path: &Path, lower_name: &str) -> bool {
    lower_name.starts_with("stage_")
        && path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some("crops")
}

/// 一个被扫描的资源条目。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanEntry {
    pub path: PathBuf,
    pub kind: String,
    pub status: String,
    pub asset_id: Option<String>,
    pub error: Option<String>,
}

/// 一次导入的结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportOutcome {
    pub path: PathBuf,
    pub reused: bool,
    pub cache_path: Option<PathBuf>,
    pub error: Option<String>,
}

/// 打包清单中的单个文件条目。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEntry {
    pub size: u64,
    pub checksum: u64,
}

/// 单类资源的压缩基准结果。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompressionBenchmarkEntry {
    pub kind: String,
    pub files: usize,
    pub original_bytes: u64,
    pub compressed_bytes: u64,
    pub compress_time: Duration,
    pub decompress_time: Duration,
}

#[derive(serde::Serialize)]
pub(crate) struct DependencyIndex {
    format: u32,
    edges: BTreeMap<String, Vec<String>>,
    opaque: Vec<String>,
}

impl CompressionBenchmarkEntry {
    pub fn ratio(&self) -> f64 {
        if self.original_bytes == 0 {
            0.0
        } else {
            self.compressed_bytes as f64 / self.original_bytes as f64
        }
    }
}

/// 清空导入缓存（保留资源数据库）。
pub fn clean(project: &Path) -> anyhow::Result<()> {
    let mut cache = ImportCache::open(project)?;
    cache.clear()?;
    Ok(())
}

/// 列出已注册资源。
pub fn list(project: &Path) -> anyhow::Result<Vec<ScanEntry>> {
    scan(project)
}

/// 冒烟检查：创建临时项目，验证 scan/import 闭环可用。
pub fn check() -> anyhow::Result<String> {
    let root = std::env::temp_dir().join(format!("rustlua-assetc-check-{}", std::process::id()));
    fs::create_dir_all(root.join("scripts"))?;
    fs::write(
        root.join("project.toml"),
        "[project]\nname = \"check\"\nversion = \"0.0.0\"\nengine = \"RustLua\"\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"\"\n",
    )?;
    fs::write(
        root.join("scripts/main.lua"),
        "-- check\nfunction update() return 'ok' end\n",
    )?;

    let scanned = scan(&root)?;
    let imported = import(&root)?;
    let summary = format!(
        "assetc_check_ok scanned={} imported={} reused={}",
        scanned.iter().filter(|e| e.status != "unsupported").count(),
        imported.len(),
        imported.iter().filter(|o| o.reused).count()
    );
    fs::remove_dir_all(&root).ok();
    Ok(summary)
}

/// 打包结果。
#[derive(Clone, Debug)]
pub struct PackageManifest {
    pub entries: BTreeMap<String, FileEntry>,
}

pub mod deps;
pub mod package;
pub mod scan;

/// 文件级拆分（M77-05 §2）后的根再导出：这些项的**规范住址在子模块**，根只给 `main.rs`、
/// 编辑器面板与 `wp0_*`/`jhyt_*` 例子一个稳定调用面。那些调用点分布在本仓 6 个文件里，
/// 其中 3 个此刻正被并行会话改着——为一次纯搬运去改它们，反而把搬运的半径放大了。
pub use deps::{BrokenLink, check_dependencies};
pub use package::{
    BundleMode, GroupReport, PackageReport, benchmark_compression, distribution_layout_json,
    distribution_layout_report, init_bundles, inspect_bundles, package, package_bundled,
    package_bundled_with, package_grouped, package_grouped_keep, package_grouped_keep_with,
    verify_bundle_equivalence,
};
pub use scan::{collect_data_files, collect_files, ensure_project, import, scan};

/// 跨模块共用的**内部**项：保持拆分前"同文件里直接可见"的调用形状，只有可见性从私有变成
/// `pub(crate)`，没有对外新增任何符号面。
pub(crate) use deps::write_dependency_index;
pub(crate) use scan::{project_font_declarations, project_font_key};

#[cfg(test)]
mod tests;
