//! assetc — 资源构建工具（M7-01）。
//!
//! 扫描项目数据文件（`project.toml`、`scenes/`、`scripts/`、`assets/`），
//! 注册到 [`ResourceDatabase`]，并通过 [`ImportCache`] 增量导入缓存，
//! 最后可打包为发布资源目录并生成清单。
//!
//! 命令（见 `main.rs`）：`scan` / `import` / `clean` / `list` / `package` / `--check`。

use anyhow::anyhow;
use rustlua_assets::dependency::{
    DependencyRef, extract_refs, module_path, recompute_edges, scan_path_call_args, scene_path,
};
use rustlua_assets::resource_db::{AssetId, DiskAsset, ResourceDatabase, ResourceState};
use rustlua_assets::resource_kind::ResourceKind;
use rustlua_assets_import::import_cache::ImportCache;
use rustlua_assets_import::run_import_batch;
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Component, Path, PathBuf},
    sync::atomic::AtomicBool,
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
const SKIP_DIRS: &[&str] = &[".cache", ".temp", ".git", "target"];

/// 生成器产出的**开发用文件**：预览拼图与美术清单。
///
/// 这些文件由 `tools/gen_*.py` 生成、供人看（美术对照图）或供生成器之间共享
/// （`manifest.json` 里的组件串），**游戏运行期不读它们**（引擎与 Lua 侧都没有
/// 读取点，`docs/48` 有清单与理由）。它们留在仓库里（生成器就在美术目录旁边写，
/// 这也是美术期望的位置），但不进发行包：
///   * `_` 前缀：生成器的"非游戏资源"约定（`_preview.png`、`_overview.png`…）；
///   * 名字里带 `preview` 的 PNG：不带下划线的预览（`preview.png`、
///     `season_preview.png`、`title_bg_pixel_preview.png`）；
///   * `manifest.json`：开发期清单（运行期只认 Lua 数据表与场景文件）。
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
    if lower.ends_with(".png") && lower.contains("preview") {
        return true;
    }
    name.starts_with('_')
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
struct DependencyIndex {
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

/// 读取 project.toml 的 `font` 键（相对项目根的主字体路径）。
fn project_font_key(project: &Path) -> Option<String> {
    let text = fs::read_to_string(project.join("project.toml")).ok()?;
    text.lines().find_map(|line| {
        let line = line.trim().trim_start_matches('\u{feff}');
        line.strip_prefix("font =")
            .map(|value| value.trim().trim_matches('"').to_owned())
            .filter(|value| !value.trim().is_empty())
    })
}

/// `project.toml` 里被声明为**运行期字体**的相对路径（`font` 与 `font_fallback`）。
/// 宿主启动期只会读这两份；`assets/fonts/` 下其余字体文件属于开发树（缺字对照、
/// 停用待下架），不构成"包里的资源"。按行扁平扫描，与 `project_font_key` 同口径。
fn project_font_declarations(project: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(project.join("project.toml")) else {
        return Vec::new();
    };
    ["font =", "font_fallback ="]
        .iter()
        .filter_map(|prefix| {
            text.lines().find_map(|line| {
                let line = line.trim().trim_start_matches('\u{feff}');
                line.strip_prefix(*prefix)
                    .map(|value| value.trim().trim_matches('"').to_owned().replace('\\', "/"))
            })
        })
        .filter(|value| !value.is_empty())
        .collect()
}

/// 验证项目目录存在 `project.toml`。
pub fn ensure_project(project: &Path) -> anyhow::Result<()> {
    let manifest = project.join("project.toml");
    if !manifest.is_file() {
        return Err(anyhow!(
            "不是有效的游戏项目（缺少 {}）：{}",
            manifest.display(),
            project.display()
        ));
    }
    Ok(())
}

/// 递归收集目录下所有文件（含 mtime），跳过本地工作目录。
///
/// 符号链接：**解析后必须仍位于被扫描的根目录内**。指向项目外的链接（例如
/// `assets/secret -> C:\Users\...`）会在打包时被 `fs::copy` 跟随，把项目外的
/// 文件悄悄塞进发布包；这里直接报错而不是静默跟随或静默跳过，避免"发布包里
/// 多了不该有的文件"这种事后很难查的问题。
pub fn collect_files(root: &Path, output: &mut Vec<(PathBuf, u64)>) -> anyhow::Result<()> {
    let anchor = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    collect_files_in(root, &anchor, output)
}

fn collect_files_in(
    root: &Path,
    anchor: &Path,
    output: &mut Vec<(PathBuf, u64)>,
) -> anyhow::Result<()> {
    let mut children = fs::read_dir(root)
        .map_err(|error| anyhow!("读取目录失败 {}: {error}", root.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| anyhow!("读取目录失败 {}: {error}", root.display()))?;
    children.sort_by_key(|entry| entry.path());
    for child in children {
        let path = child.path();
        let file_type = child.file_type()?;
        if file_type.is_symlink() {
            ensure_symlink_stays_inside(&path, anchor)?;
        }
        let is_dir = if file_type.is_symlink() {
            fs::metadata(&path)
                .map(|meta| meta.is_dir())
                .unwrap_or(false)
        } else {
            file_type.is_dir()
        };
        if is_dir {
            if SKIP_DIRS.contains(&path.file_name().and_then(|n| n.to_str()).unwrap_or("")) {
                continue;
            }
            collect_files_in(&path, anchor, output)?;
        } else if !is_dev_artifact(&path) {
            let modified = modified_nanos(&path)?;
            output.push((path, modified));
        }
    }
    Ok(())
}

/// 校验符号链接解析后仍在 `anchor` 内；越界即报错。
fn ensure_symlink_stays_inside(path: &Path, anchor: &Path) -> anyhow::Result<()> {
    let resolved = fs::canonicalize(path)
        .map_err(|error| anyhow!("解析符号链接失败 {}: {error}", path.display()))?;
    if path_escapes_root(anchor, &resolved) {
        return Err(anyhow!(
            "符号链接指向项目目录外，打包被拒绝：{} → {}",
            path.display(),
            resolved.display()
        ));
    }
    Ok(())
}

/// 纯路径判定：`resolved` 是否越出 `anchor`。
///
/// `anchor`/`resolved` 期望已规范化（`canonicalize` 的结果不含 `..`）；这里额外把
/// 任何 `..` 组件一律判为越界，避免调用方传入未规范化的路径时靠词法前缀比较被绕过
/// （`Path::starts_with` 不做 `..` 归一化）。
fn path_escapes_root(anchor: &Path, resolved: &Path) -> bool {
    resolved
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
        || !resolved.starts_with(anchor)
}

fn modified_nanos(path: &Path) -> anyhow::Result<u64> {
    let modified = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| anyhow!("读取文件时间失败 {}: {error}", path.display()))?;
    Ok(modified
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0))
}

/// 收集项目的数据文件（清单 + 场景 + 脚本 + 资源），含 mtime。
pub fn collect_data_files(project: &Path) -> anyhow::Result<Vec<(PathBuf, u64)>> {
    let mut paths = Vec::new();
    let manifest = project.join("project.toml");
    if manifest.is_file() {
        let modified = modified_nanos(&manifest)?;
        paths.push((manifest, modified));
    }
    for sub in ["scenes", "scripts", "assets"] {
        let dir = project.join(sub);
        if dir.is_dir() {
            collect_files(&dir, &mut paths)?;
        }
    }
    paths.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(paths)
}

fn status_label(status: &ResourceState) -> &'static str {
    match status {
        ResourceState::Discovered => "discovered",
        ResourceState::Ready => "ready",
        ResourceState::ImportFailed => "import_failed",
        ResourceState::Stale => "stale",
        ResourceState::Missing => "missing",
        ResourceState::Orphaned => "orphaned",
        ResourceState::Invalid => "invalid",
        // 瞬态状态不会落到数据库，理论上不会出现在 scan 结果中。
        ResourceState::ImportQueued | ResourceState::Importing => "importing",
        ResourceState::Removed => "removed",
    }
}

/// 扫描项目数据文件并注册到 ResourceDatabase。
pub fn scan(project: &Path) -> anyhow::Result<Vec<ScanEntry>> {
    ensure_project(project)?;
    let scanned_files = collect_data_files(project)?;
    let mut database = ResourceDatabase::open(project)?;
    // core 拆分后 `ImporterRegistry::builtin()` 不含 Scene/Prefab 导入器，
    // 扫描阶段必须用完整注册表，否则 .scene/.prefab 会误报"不支持的资源类型"。
    let registry = rustlua_assets_import::full_registry();
    let mut entries = Vec::new();
    let mut disk_assets = Vec::with_capacity(scanned_files.len());
    // 已注册记录延迟到 reconcile+传播后再重建，让 scan 输出反映最新状态
    // （依赖断链标 invalid、文件删除标 missing）。
    let mut registered: Vec<(PathBuf, String, AssetId)> = Vec::new();
    for (path, modified) in scanned_files {
        let relative = path
            .strip_prefix(project)
            .expect("path must be inside project root")
            .to_path_buf();
        // 即使是不支持的类型也计入扫描结果，避免 reconcile 误判为 Missing。
        disk_assets.push(DiskAsset {
            path: relative.clone(),
            modified,
        });
        let kind = ResourceKind::from_path(&relative);
        if kind == ResourceKind::Other {
            entries.push(ScanEntry {
                path: relative,
                kind: kind.token().to_owned(),
                status: "unsupported".to_owned(),
                asset_id: None,
                error: Some("不支持的资源类型".to_owned()),
            });
            continue;
        }
        let kind_token = kind.token().to_owned();
        let importer_version = registry
            .get(kind)
            .map(|importer| importer.version())
            .unwrap_or("none");
        match database.register(&relative, kind, importer_version) {
            Ok(id) => registered.push((relative, kind_token, id)),
            Err(error) => entries.push(ScanEntry {
                path: relative,
                kind: kind_token,
                status: "failed".to_owned(),
                asset_id: None,
                error: Some(error.to_string()),
            }),
        }
    }
    let mut scan_roots = Vec::new();
    for sub in ["scenes", "scripts", "assets"] {
        if project.join(sub).is_dir() {
            scan_roots.push(PathBuf::from(sub));
        }
    }
    database.reconcile(&disk_assets, &scan_roots);
    // 重算依赖边并传播失效（场景/脚本引用的资源缺失 → 消费者标 invalid）。
    recompute_edges(&mut database, &|path| {
        fs::read_to_string(project.join(path)).ok()
    });
    database.propagate_invalidation();
    database.save()?;
    // 用 reconcile+传播后的最新状态重建条目，保持按路径排序。
    for (relative, kind_token, id) in registered {
        let record = database
            .get(id)
            .expect("registered asset must be present in database");
        entries.push(ScanEntry {
            path: relative,
            kind: kind_token,
            status: status_label(&record.status).to_owned(),
            asset_id: Some(id.to_string()),
            error: record.error.clone(),
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

/// 增量导入所有已注册资源。
///
/// 单坏项不中断整个批次：失败资源在结果中以 `error` 记录，其余资源照常导入。
pub fn import(project: &Path) -> anyhow::Result<Vec<ImportOutcome>> {
    let entries = scan(project)?;
    let mut database = ResourceDatabase::open(project)?;
    let mut cache = ImportCache::open(project)?;
    let registry = rustlua_assets_import::full_registry();
    let items: Vec<_> = entries
        .iter()
        .filter_map(|entry| {
            let id = entry.asset_id.as_deref()?.parse().ok()?;
            Some((id, project.join(&entry.path)))
        })
        .collect();
    let cancel = AtomicBool::new(false);
    let summary = run_import_batch(
        project,
        &mut database,
        &mut cache,
        &registry,
        &items,
        &cancel,
    );
    // 导入后依赖可能已就绪：重新传播失效，让消费者从 invalid 撤销回可消费状态。
    database.propagate_invalidation();
    database.save()?;
    Ok(summary
        .results
        .into_iter()
        .map(|result| {
            let path = result
                .source
                .strip_prefix(project)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| result.source.clone());
            ImportOutcome {
                path,
                reused: result.reused,
                cache_path: result.cache_path,
                error: result.error,
            }
        })
        .collect())
}

/// 发布检查发现的断链：`consumer` 是直接消费者，`chain` 是
/// `消费者 → … → 根断链资源` 的完整链路（含消费者自身）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrokenLink {
    pub consumer: PathBuf,
    pub chain: Vec<PathBuf>,
    pub reason: String,
}

/// 依赖健康度检查：先注册并传播失效，再对每个 Invalid 消费者沿依赖下探到
/// 断链根，输出完整链路（如 `scenes/main.scene → assets/textures/player.png
/// (缺失)`）。成环时截断并标注循环依赖。健康项目返回空列表。
pub fn check_dependencies(project: &Path) -> anyhow::Result<Vec<BrokenLink>> {
    ensure_project(project)?;
    // 注册 + reconcile + 依赖重算 + 失效传播，让数据库反映最新状态。
    scan(project)?;
    let database = ResourceDatabase::open(project)?;
    let mut links = Vec::new();
    for record in database.iter() {
        if record.status != ResourceState::Invalid {
            continue;
        }
        let Some((chain, reason)) = walk_broken_chain(&database, record.id) else {
            continue;
        };
        let chain_paths = chain
            .iter()
            .map(|id| {
                database
                    .get(*id)
                    .map(|node| node.source_path.clone())
                    .unwrap_or_else(|| PathBuf::from(id.to_string()))
            })
            .collect();
        links.push(BrokenLink {
            consumer: record.source_path.clone(),
            chain: chain_paths,
            reason,
        });
    }
    links.sort_by(|a, b| a.consumer.cmp(&b.consumer));
    Ok(links)
}

/// 断链依赖在链路遍历中的权值（同传播语义：Missing/Orphaned/ImportFailed
/// 折叠为 broken-link，与 Invalid 同级）。
fn break_rank(status: ResourceState) -> u8 {
    match status {
        ResourceState::Invalid
        | ResourceState::Missing
        | ResourceState::Orphaned
        | ResourceState::ImportFailed => 6,
        _ => 0,
    }
}

/// 沿依赖下探到断链根：只沿 Invalid 依赖继续，遇到 Missing/Orphaned/
/// ImportFailed 即叶子；依赖健康却仍 Invalid 时保守返回自身。成环截断并
/// 标注循环依赖，避免无限递归。
fn walk_broken_chain(db: &ResourceDatabase, start: AssetId) -> Option<(Vec<AssetId>, String)> {
    let mut chain = vec![start];
    let mut current = start;
    let mut seen = HashSet::new();
    seen.insert(start);
    loop {
        let record = db.get(current)?;
        let worst = record
            .dependencies
            .iter()
            .filter_map(|dep| {
                let dep_record = db.get(*dep)?;
                (break_rank(dep_record.status) > 0).then_some((*dep, dep_record.status))
            })
            .max_by_key(|(_, status)| break_rank(*status));
        let Some((next, next_status)) = worst else {
            return Some((chain, "无效".to_owned()));
        };
        if !seen.insert(next) {
            chain.push(next);
            return Some((chain, "循环依赖".to_owned()));
        }
        chain.push(next);
        match next_status {
            ResourceState::Invalid => current = next,
            ResourceState::Missing => return Some((chain, "缺失".to_owned())),
            ResourceState::Orphaned => return Some((chain, "已失效".to_owned())),
            ResourceState::ImportFailed => return Some((chain, "导入失败".to_owned())),
            _ => return Some((chain, "无效".to_owned())),
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

fn copy_file(source: &Path, destination: &Path) -> anyhow::Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, destination).map_err(|error| {
        anyhow!(
            "复制失败 {} → {}: {error}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
fn copy_dir(source: &Path, destination: &Path) -> anyhow::Result<()> {
    if !source.is_dir() {
        return Ok(());
    }
    let mut files = Vec::new();
    collect_files(source, &mut files)?;
    for (file, _) in files {
        let relative = file
            .strip_prefix(source)
            .expect("file must be inside source dir");
        copy_file(&file, &destination.join(relative))?;
    }
    Ok(())
}

/// FNV-1a 校验和（用于清单，非加密用途）。
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// 复制目录；可选对文件进行发布编码（运行时自动解码）。
fn copy_dir_encoded(
    source: &Path,
    destination: &Path,
    compress: bool,
    encrypt: bool,
    should_compress: fn(&Path) -> bool,
    should_encrypt: fn(&Path) -> bool,
) -> anyhow::Result<()> {
    if !source.is_dir() {
        return Ok(());
    }
    let mut files = Vec::new();
    collect_files(source, &mut files)?;
    for (file, _) in files {
        let relative = file
            .strip_prefix(source)
            .expect("file must be inside source dir");
        let dest = destination.join(relative);
        let compress_file = compress && should_compress(&file);
        let encrypt_file = encrypt && should_encrypt(&file);
        if compress_file || encrypt_file {
            let bytes = fs::read(&file)?;
            let encoded = rustlua_assets::pack::encode(&bytes, compress_file, encrypt_file);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&dest, &encoded)?;
        } else {
            copy_file(&file, &dest)?;
        }
    }
    Ok(())
}

fn never_encode(_: &Path) -> bool {
    false
}

fn is_encryptable_data(path: &Path) -> bool {
    matches!(extension(path).as_deref(), Some("lua") | Some("scene"))
}

fn is_compressible_asset(path: &Path) -> bool {
    // mp3 虽然自身是压缩格式，实测工程内 16 个 clips 走 pack 的 gzip 默认档仍从
    // 1,497,215 B 降到 1,441,547 B（-3.7%）：低码率单声道流的帧头与填充字节可压。
    matches!(
        extension(path).as_deref(),
        Some("png")
            | Some("jpg")
            | Some("jpeg")
            | Some("wav")
            | Some("ogg")
            | Some("mp3")
            | Some("ttf")
            | Some("otf")
            | Some("glb")
    )
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

/// 按资产类型统计压缩前后大小、压缩耗时和解压耗时。
pub fn benchmark_compression(project: &Path) -> anyhow::Result<Vec<CompressionBenchmarkEntry>> {
    ensure_project(project)?;
    let assets = project.join("assets");
    if !assets.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    collect_files(&assets, &mut files)?;
    let mut entries = BTreeMap::<String, CompressionBenchmarkEntry>::new();
    for (file, _) in files {
        if !is_compressible_asset(&file) {
            continue;
        }
        let bytes =
            fs::read(&file).map_err(|error| anyhow!("读取失败 {}: {error}", file.display()))?;
        let kind = ResourceKind::from_path(&file).token().to_owned();
        let started = std::time::Instant::now();
        let compressed = rustlua_assets::pack::encode(&bytes, true, false);
        let compress_time = started.elapsed();
        let started = std::time::Instant::now();
        let decompressed =
            rustlua_assets::pack::decode(&compressed).map_err(|error| anyhow!("{error}"))?;
        let decompress_time = started.elapsed();
        if decompressed != bytes {
            return Err(anyhow!("压缩往返校验失败: {}", file.display()));
        }
        let entry = entries
            .entry(kind.clone())
            .or_insert_with(|| CompressionBenchmarkEntry {
                kind,
                ..CompressionBenchmarkEntry::default()
            });
        entry.files += 1;
        entry.original_bytes += bytes.len() as u64;
        entry.compressed_bytes += compressed.len() as u64;
        entry.compress_time += compress_time;
        entry.decompress_time += decompress_time;
    }
    Ok(entries.into_values().collect())
}

/// 打包时复制到输出目录的项目数据子目录。
const PACKAGE_SOURCE_DIRS: &[&str] = &["scenes", "scripts", "assets"];

/// 校验发布输出目录与项目目录的关系。
///
/// 拒绝三类**真会出事**的组合：
/// 1. `out == project`：直接覆写项目文件；
/// 2. `out` 位于 `scenes/`、`scripts/`、`assets/` 之内：这几个目录正是打包的输入，
///    把产物写进去会让 `collect_files` 一边遍历一边写入（自我复制/无限增长），
///    而且下一次打包会把产物当输入；
/// 3. `project` 位于 `out` 之内：会把项目连同产物一起递归进包。
///
/// **允许** `out` 位于项目内的其它位置（如 `builds/`）：`scan`/`collect_data_files`
/// 只遍历 `project.toml` 与上述三个子目录，因此 `builds/` 下的发布产物不会被当成
/// 项目数据；`scripts/build/package.ps1` 的原子发布正是这样布局的
/// （`<project>/builds/<platform>/.staging-*/pack`）。
fn ensure_distinct_output(project: &Path, out: &Path) -> anyhow::Result<()> {
    let project_abs = canonical_or_absolute(project)?;
    let out_abs = canonical_or_absolute(out)?;
    if out_abs == project_abs {
        return Err(anyhow!("输出目录不能与项目目录相同"));
    }
    for sub in PACKAGE_SOURCE_DIRS {
        let source_abs = canonical_or_absolute(&project_abs.join(sub))?;
        if out_abs.starts_with(&source_abs) {
            return Err(anyhow!(
                "输出目录不能位于项目数据目录内部（{}）：{}",
                sub,
                out_abs.display()
            ));
        }
    }
    if project_abs.starts_with(&out_abs) {
        return Err(anyhow!(
            "项目目录不能位于输出目录内部：{} ⊂ {}",
            project_abs.display(),
            out_abs.display()
        ));
    }
    Ok(())
}

/// 规范化为绝对路径：已存在则用 `canonicalize`，否则规范化其最近存在的祖先后拼回
/// 剩余部分（输出目录通常还不存在）。
fn canonical_or_absolute(path: &Path) -> anyhow::Result<PathBuf> {
    if let Ok(resolved) = fs::canonicalize(path) {
        return Ok(resolved);
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| anyhow!("读取当前目录失败: {error}"))?
            .join(path)
    };
    let mut existing = absolute.clone();
    let mut tail = Vec::new();
    while !existing.exists() {
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name.to_os_string());
                existing = parent.to_path_buf();
            }
            _ => break,
        }
    }
    let mut resolved = fs::canonicalize(&existing).unwrap_or(existing);
    for component in tail.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

/// 把 `project.toml` 声明的工程图（`splash_image` / `icon_image`）按声明的相对路径复制进包。
///
/// 为什么不能只靠 `assets/` 那一趟目录复制：启动图是**窗口出现之前就要读**的资源，工程常把
/// 它单独放成顶层目录（`splash/splash.png`，game_jhyt 就这样）。只扫 `assets/` 的话它根本不
/// 进包，运行期 `splash::startup_image` 读不到 ⇒ **静默回落**到编译期内置图，日志里仅有
/// 一条 warn——"换了启动图却没生效"就是这条链。
///
/// 与 `font` 同一条纪律：**声明即随包**（`--drop-undeclared-fonts` 也只保留声明过的字体）。
/// 缺失的文件不在这里报错（那是 `game --check` 的资源校验语境），但越出工程根的路径必须
/// 拒绝：`fs::copy` 会把项目外的文件悄悄塞进发布包。
fn copy_declared_images(project: &Path, out: &Path) -> anyhow::Result<()> {
    for key in ["splash_image", "icon_image"] {
        let Some(value) = project_key_value(project, key) else {
            continue;
        };
        // Windows 工程里两种分隔符都有；包内条目名统一用 `/`（与 manifest/容器同一口径）。
        let relative = value.replace('\\', "/");
        let path = Path::new(&relative);
        if relative.contains(':')
            || path
                .components()
                .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
        {
            return Err(anyhow!(
                "project.toml 的 {key} 必须是工程内相对路径（不得含 ..）：{relative}"
            ));
        }
        let source = project.join(path);
        if !source.is_file() {
            continue;
        }
        let destination = out.join(path);
        if destination.is_file() {
            // 已经在 `assets/` 下：目录复制那一趟已经带过去了，不再覆盖（也不必）。
            continue;
        }
        copy_file(&source, &destination)?;
    }
    Ok(())
}

/// 生成发布资源包：复制 `project.toml`、`scenes/`、`scripts/`、`assets/`
/// 到 `out`，并写入 `manifest.txt`（路径、大小、校验和）。
/// 此外还按 `project.toml` 的 `splash_image` / `icon_image` 把**声明的**工程图带进包
/// （它们可以不在 `assets/` 下，见 `copy_declared_images`）。
/// `compress` 对 `assets/` 下支持压缩的资产生效；`encrypt` 仅对 Lua 与场景文件生效。
/// `drop_undeclared_fonts`（微信形态，docs/52 配额门）：删掉 `assets/fonts/` 下**没被
/// `project.toml` 声明**（`font`/`font_fallback`）的字体二进制——它们是开发树对照物，
/// 宿主只 `wx.loadFont` 声明那一份，随包发就是把配额门压在不被读取的死重上。
/// 剔除发生在 `manifest.txt` 生成**之前**，清单与磁盘天然一致。
pub fn package(
    project: &Path,
    out: &Path,
    compress: bool,
    encrypt: bool,
    drop_undeclared_fonts: bool,
) -> anyhow::Result<PackageManifest> {
    ensure_project(project)?;
    ensure_distinct_output(project, out)?;
    copy_file(&project.join("project.toml"), &out.join("project.toml"))?;
    copy_dir_encoded(
        &project.join("scenes"),
        &out.join("scenes"),
        false,
        encrypt,
        never_encode,
        is_encryptable_data,
    )?;
    copy_dir_encoded(
        &project.join("scripts"),
        &out.join("scripts"),
        false,
        encrypt,
        never_encode,
        is_encryptable_data,
    )?;
    copy_dir_encoded(
        &project.join("assets"),
        &out.join("assets"),
        compress,
        false,
        is_compressible_asset,
        never_encode,
    )?;
    copy_declared_images(project, out)?;
    // 首包字体：收集项目用到的字符清单，运行时预生成字形（首帧文字不卡）。
    let chars = rustlua_assets_import::char_collect::collect_project_chars(project);
    fs::write(out.join("chars.txt"), &chars)?;
    // 发布子集化：把主字体裁剪到用到的字符，缩小体积（原 `subset-font.py` 的 Rust 版）。
    // 子集化失败（如 CFF 字体）时保留完整字体。
    if let Some(font_path) = project_font_key(project) {
        let src = project.join(&font_path);
        let dst = out.join(&font_path);
        if src.is_file()
            && dst.is_file()
            && let Ok(bytes) = fs::read(&src)
            && let Ok(subset) = rustlua_render::font_subset::subset_ttf(&bytes, &chars)
        {
            fs::write(&dst, subset)?;
        }
    }
    if drop_undeclared_fonts {
        let declared = project_font_declarations(project);
        let fonts_dir = out.join("assets").join("fonts");
        if fonts_dir.is_dir() {
            for entry in fs::read_dir(&fonts_dir)? {
                let path = entry?.path();
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if !matches!(ext.as_str(), "ttf" | "otf" | "woff" | "woff2") {
                    continue;
                }
                let relative = path
                    .strip_prefix(out)
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                if declared.contains(&relative) {
                    continue;
                }
                fs::remove_file(&path)?;
            }
        }
    }
    write_dependency_index(project, &out.join("dependencies.json"))?;

    let mut files = Vec::new();
    collect_files(out, &mut files)?;
    files.retain(|(path, _)| path != &out.join("manifest.txt"));
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut manifest = BTreeMap::new();
    let mut lines = vec![format!("assetc_manifest=1\t{PACKAGE_VERSION}")];
    for (path, _) in files {
        // 这里曾经是 `expect`：`collect_files(out)` 的结果理论上一定在 out 下，
        // 但打包中途 panic 会留下半个发布目录。改成可定位的错误。
        let relative = path
            .strip_prefix(out)
            .map_err(|_| {
                anyhow!(
                    "清单文件不在输出目录内：{} (out={})",
                    path.display(),
                    out.display()
                )
            })?
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = fs::read(&path)?;
        let entry = FileEntry {
            size: bytes.len() as u64,
            checksum: fnv1a(&bytes),
        };
        lines.push(format!(
            "{relative}\t{}\t{:016x}",
            entry.size, entry.checksum
        ));
        manifest.insert(relative, entry);
    }
    fs::write(out.join("manifest.txt"), lines.join("\n"))?;
    Ok(PackageManifest { entries: manifest })
}

/// 打包形态（`docs/67` 批 A/批 C）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BundleMode {
    /// 目录形态：一文件一资源（今天的 `assetc package`）。
    Loose,
    /// 单包形态：全部资源进一个 RLB 容器，输出根只留骨架。
    Single,
    /// 分组形态：按 `bundles.json`（缺省用内置内容域规则）切成多个容器 + `bundle_plan.json`。
    Grouped,
}

impl BundleMode {
    /// 命令行/日志用的短名。
    pub fn token(self) -> &'static str {
        match self {
            Self::Loose => "none",
            Self::Single => "single",
            Self::Grouped => "grouped",
        }
    }

    /// `--bundle <值>` 解析。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" | "loose" => Some(Self::Loose),
            "single" => Some(Self::Single),
            "grouped" | "groups" => Some(Self::Grouped),
            _ => None,
        }
    }
}

/// 打包体积报告（写进包内 `package-report.json`，也用于命令行摘要）。
///
/// **必须确定性**：只含由输入决定的值，不含耗时/时间戳/进程信息——否则同一份工程两次打包
/// 产物不同，`manifest` 与 golden 门都会抖（`docs/67` §4.5）。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PackageReport {
    /// 报告 schema，改动字段含义时自增。
    pub schema: u32,
    /// `none` / `single`。
    pub mode: String,
    /// 输出目录里的文件数（含清单与报告自身）。
    pub files: u64,
    /// 包内条目数（目录形态下等于文件数减清单）。
    pub entries: u64,
    /// 块数（目录形态恒 0）。
    pub blocks: u64,
    /// 条目原始字节合计。
    pub raw_bytes: u64,
    /// 容器落盘字节（目录形态 = `raw_bytes`）。
    pub bundle_bytes: u64,
    /// 索引落盘 / 解压后字节。
    pub index_bytes: u64,
    pub index_raw_bytes: u64,
    /// 压缩比 × 1000（整数，避免浮点在报告里抖动）。
    pub ratio_milli: u64,
    /// 容器文件的包内相对路径（目录形态为空；分组形态为第一个组，其余见 `groups`）。
    pub bundle_path: String,
    /// 分组形态的逐组结果（`single` / 目录形态为空）。
    #[serde(default)]
    pub groups: Vec<GroupReport>,
}

/// 报告里的单组结果。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GroupReport {
    pub name: String,
    /// `main` / `sub` / `cdn`。
    pub unit: String,
    pub entries: u64,
    pub raw_bytes: u64,
    pub bundle_bytes: u64,
    /// 容器在包内的相对路径。
    pub bundle: String,
}

impl PackageReport {
    fn ratio_milli(raw: u64, stored: u64) -> u64 {
        if raw == 0 {
            1000
        } else {
            stored.saturating_mul(1000).checked_div(raw).unwrap_or(1000)
        }
    }

    /// 压缩比（浮点，仅供打印/测试）。
    pub fn ratio(&self) -> f64 {
        self.ratio_milli as f64 / 1000.0
    }
}

/// 容器形态下**故意留在目录里**的路径前缀。
pub type KeepPrefixes<'a> = &'a [String];

/// 判断一个条目是否要留在目录形态里（不进容器）。
///
/// 为什么需要这个口子：有些消费者**按真实文件路径**读资源，容器里的条目它们够不着。
/// 目前只有一个实例，但很硬：微信 `wx.loadFont(path)` 只吃本地文件，字体一旦进容器就加载不了
/// （实测：`package.ps1` 的"字体必须在 pack/ 里"断言当场拦下）。
/// 把这类资源按前缀排除之后，容器与目录两种形态可以并存——它们仍是 manifest 条目、
/// 仍参与完整性校验，只是不被打进 `.rlb`。
fn is_kept(relative: &str, keep: KeepPrefixes<'_>) -> bool {
    keep.iter()
        .any(|prefix| relative.starts_with(prefix.as_str()))
}

/// 启动闭包优先级：小的先排（0 = 启动就读，1 = 启动后按需）。
///
/// 判据只列**确实在 boot 期间被读**的东西（见 `game` / `game_web` 的启动序列）：
/// `project.toml`、启动场景、主脚本、图集重定向表。字体不算——容器形态下它被 `--keep`
/// 留在目录里，根本不进块。
fn boot_priority(relative: &str) -> u8 {
    // 0 = 引擎**确定**在 boot 期读的四个文件：project.toml、图集重定向表、启动场景、主脚本。
    //     它们合计约 75 KB，排在一起就能塞进同一个块。
    if relative == "project.toml"
        || relative == "atlas_redirect.json"
        || relative == "scenes/main.scene"
        || relative == "scripts/main.lua"
    {
        return 0;
    }
    // 1 = 其余 Lua 脚本：`require` 同步，boot 期间会连带读到（1600 KB / 155 个文件，约 7 块）。
    //     紧跟锚点，等于把"启动会摸到的块"压到容器最前面。
    if relative.starts_with("scripts/") {
        return 1;
    }
    // 2 = 其余（场景、预制体…）：启动只读其中一个场景，其余按需。
    2
}

/// 单包形态：先按目录形态产出（复用字体子集/依赖索引/安全边界检查），
/// 再把整棵树收进 `bundles/all-<hash8>.rlb`，输出根只留骨架
/// （`project.toml` + `manifest.txt` + 容器 + 报告）。
///
/// 为什么"先生成再收"而不是直接流式写包：这样"包内条目 == 目录形态的产物"是**构造性成立**的，
/// 等价性门（`bundle_package_matches_loose_output`）不需要再维护一份拷贝逻辑。
pub fn package_bundled(
    project: &Path,
    out: &Path,
    drop_undeclared_fonts: bool,
    fast: bool,
    encrypt: bool,
    keep: KeepPrefixes<'_>,
) -> anyhow::Result<PackageReport> {
    package_bundled_with(project, out, drop_undeclared_fonts, fast, encrypt, keep, 0)
}

/// [`package_bundled`] 的可调块大小版本（`block_size = 0` 表示默认 256 KB）。
pub fn package_bundled_with(
    project: &Path,
    out: &Path,
    drop_undeclared_fonts: bool,
    fast: bool,
    encrypt: bool,
    keep: KeepPrefixes<'_>,
    block_size: u32,
) -> anyhow::Result<PackageReport> {
    let block_size = if block_size == 0 {
        rustlua_infra::bundle::DEFAULT_BLOCK_SIZE
    } else {
        block_size
    };
    use rustlua_infra::bundle::BundleBuilder;

    // 1) 目录形态打底 + （可选）图集 cook。cook 读的是 `out/assets/**`、写完页与重定向表后
    //    源图就不在了，所以后面的条目收集天然拿到"页 + 定义"而不是散图。
    package(project, out, false, false, drop_undeclared_fonts)?;
    if let Some(config) = atlas_cook::AtlasConfig::load(&project.join("atlas.json"))? {
        let redirect = atlas_cook::cook(project, out, &config)?;
        fs::write(
            out.join(atlas_cook::REDIRECT_FILE),
            serde_json::to_vec_pretty(&redirect)
                .map_err(|error| anyhow!("写重定向表失败：{error}"))?,
        )?;
    }

    // 2) 收集条目（排除稍后要重写的清单与报告）。
    let mut files = Vec::new();
    collect_files(out, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let manifest_path = out.join("manifest.txt");
    let report_path = out.join(PACKAGE_REPORT_FILE);
    let mut builder = BundleBuilder::new(block_size);
    // 块级 AEAD（docs/67 §4.4）：--encrypt 直接落进容器，贴图/音频/字体一起进加密域。
    if encrypt {
        builder = builder.with_encryption(true);
    }
    if fast {
        builder = builder.with_level(3);
    }
    let mut entries = 0u64;
    for (path, _) in &files {
        if path == &manifest_path || path == &report_path {
            continue;
        }
        let relative = path
            .strip_prefix(out)
            .map_err(|_| anyhow!("打包条目不在输出目录内：{}", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        if is_kept(&relative, keep) {
            continue; // 留在目录里：宿主按真实路径读它（微信 `wx.loadFont`）
        }
        let bytes = fs::read(path)?;
        builder
            .add(&relative, bytes)
            .map_err(|error| anyhow!("{error}"))?;
        entries += 1;
    }
    if entries == 0 {
        return Err(anyhow!("没有可打包的资源（输出目录为空）"));
    }

    // 3) 建包。
    let built = builder.build().map_err(|error| anyhow!("{error}"))?;
    let bundle_name = format!("all-{:08x}.rlb", built.checksum as u32);
    let bundle_rel = format!("{BUNDLE_DIR}/{bundle_name}");
    let bundle_path = out.join(BUNDLE_DIR).join(&bundle_name);
    if let Some(parent) = bundle_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&bundle_path, &built.bytes)?;

    // 4) 收尾：删松散文件 → 写报告 → 重写清单。
    let mut report = PackageReport {
        schema: 2,
        mode: BundleMode::Single.token().to_owned(),
        files: 0,
        entries,
        blocks: u64::from(built.blocks),
        raw_bytes: built.raw_len,
        bundle_bytes: built.bytes.len() as u64,
        index_bytes: u64::from(built.index_len),
        index_raw_bytes: u64::from(built.index_raw_len),
        ratio_milli: PackageReport::ratio_milli(built.raw_len, built.bytes.len() as u64),
        bundle_path: bundle_rel,
        groups: Vec::new(),
    };
    finalize_container_pack(out, &files, &[bundle_path], keep, &mut report)?;
    Ok(report)
}

/// 分组形态（`docs/67` §5，批 C）：按 `bundles.json`（缺省内置内容域规则）切成多个容器，
/// 产出 `bundle_plan.json`，并在构建期跑三条硬门（覆盖完整 / 不跨分发单元 / core 自足）。
pub fn package_grouped(
    project: &Path,
    out: &Path,
    drop_undeclared_fonts: bool,
    fast: bool,
) -> anyhow::Result<PackageReport> {
    package_grouped_keep(project, out, drop_undeclared_fonts, fast, false, &[])
}

/// [`package_grouped`] 的带"留在目录里"前缀的版本（微信字体走这条路，见 `is_kept`）。
pub fn package_grouped_keep(
    project: &Path,
    out: &Path,
    drop_undeclared_fonts: bool,
    fast: bool,
    encrypt: bool,
    keep: KeepPrefixes<'_>,
) -> anyhow::Result<PackageReport> {
    package_grouped_keep_with(
        project,
        out,
        drop_undeclared_fonts,
        fast,
        encrypt,
        keep,
        0,
        false,
    )
}

/// [`package_grouped_keep`] 的可调块大小版本（`0` = 默认 256 KB）。
///
/// 块大小的权衡（`docs/67` §9 的性能项）：块越大压缩率略好、同一次解压覆盖更多条目，
/// 但单次解压更久、惰性读的粒度更粗。默认 256 KB 是拍的，这个参数就是为了量它。
///
/// `exclude_cdn`：本地包（微信分包/整包）不携带 `unit = "cdn"` 的组——它们本来就该由
/// CDN 单独分发；带进 `pack/` 会变成"主包/分包里一份、CDN 上再一份"的死重
/// （`splash/` 品牌图就是这种：发布器已把它压成 `branding/splash.jpg`，资源组里那份是纯重复）。
///
/// 参数是平铺的打包选项标量（与 `main.rs` / `tests/bundle_mount.rs` 三个调用点保持一致），
/// 保持扁平比拆成配置结构更易读。
#[allow(clippy::too_many_arguments)]
pub fn package_grouped_keep_with(
    project: &Path,
    out: &Path,
    drop_undeclared_fonts: bool,
    fast: bool,
    encrypt: bool,
    keep: KeepPrefixes<'_>,
    block_size: u32,
    exclude_cdn: bool,
) -> anyhow::Result<PackageReport> {
    let block_size = if block_size == 0 {
        rustlua_infra::bundle::DEFAULT_BLOCK_SIZE
    } else {
        block_size
    };
    use bundle_plan::{
        BundleConfig, MAX_BUNDLE_BYTES, PLAN_FILE, build_plan, plan_to_json, validate_plan,
    };
    use rustlua_infra::bundle::{BundleBuilder, DEFAULT_BLOCK_SIZE};

    // 1) 目录形态打底（字体子集/依赖索引/安全边界都在这一层，与单包形态同源）+（可选）图集 cook。
    package(project, out, false, false, drop_undeclared_fonts)?;
    if let Some(config) = atlas_cook::AtlasConfig::load(&project.join("atlas.json"))? {
        let redirect = atlas_cook::cook(project, out, &config)?;
        fs::write(
            out.join(atlas_cook::REDIRECT_FILE),
            serde_json::to_vec_pretty(&redirect)
                .map_err(|error| anyhow!("写重定向表失败：{error}"))?,
        )?;
    }

    let manifest_path = out.join("manifest.txt");
    let report_path = out.join(PACKAGE_REPORT_FILE);
    let plan_path = out.join(PLAN_FILE);

    // 2) 收集条目（排除稍后要重写的清单/报告/计划）。
    //
    // 注意 `collect_files` 的第二个字段是**修改时间（纳秒）**，不是字节数
    // （见 `collect_files` 的文档与 `modified_nanos`）。分组要按字节切，所以这里单独取长度；
    // 拿 mtime 当字节数会把几个 20 位数字加爆 u64（批 C 第一次跑就踩了）。
    let mut files = Vec::new();
    collect_files(out, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut entries: Vec<(String, u64, PathBuf)> = Vec::new();
    // `--keep` 命中的**具体路径**要写进计划表：发布期 `assetc closure` 靠它断言"这些文件
    // 必须以真实文件留在发布根里，且不得再出现在任何容器索引里"。只把前缀传下去是不够的——
    // 前缀是规则，规则会在下一版改；契约必须随产物走。
    let mut kept_paths: Vec<String> = Vec::new();
    for (path, _mtime) in &files {
        if path == &manifest_path || path == &report_path || path == &plan_path {
            continue;
        }
        let relative = path
            .strip_prefix(out)
            .map_err(|_| anyhow!("打包条目不在输出目录内：{}", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        if is_kept(&relative, keep) {
            kept_paths.push(relative);
            continue; // 留在目录里：宿主按真实路径读它（微信 `wx.loadFont`）
        }
        let size = fs::metadata(path)
            .map_err(|error| anyhow!("读 {} 的长度失败：{error}", path.display()))?
            .len();
        entries.push((relative, size, path.clone()));
    }
    if entries.is_empty() {
        return Err(anyhow!("没有可打包的资源（输出目录为空）"));
    }

    // 3) 分组 + 三条硬门。
    let config = BundleConfig::load_or_default(&project.join("bundles.json"))?;
    let plan_entries: Vec<(String, u64)> = entries
        .iter()
        .map(|(path, size, _)| (path.clone(), *size))
        .collect();
    let mut plan = build_plan(&config, &plan_entries, DEFAULT_BLOCK_SIZE)?;
    if exclude_cdn {
        // 先把 cdn 组的路径从打包条目里剔掉，再把组从计划里删掉并重封自校验。
        // 顺序很重要：先 build_plan 才能知道哪些路径属于 cdn（归组规则只在 plan 里落地），
        // 再按路径过滤 entries，最后 sealed() 重算 groups 的 FNV。
        let cdn_paths: std::collections::BTreeSet<String> = plan
            .groups
            .iter()
            .filter(|group| group.unit == "cdn")
            .flat_map(|group| group.paths.iter().cloned())
            .collect();
        if !cdn_paths.is_empty() {
            entries.retain(|(path, _, _)| !cdn_paths.contains(path));
            plan.groups.retain(|group| group.unit != "cdn");
            plan = plan.sealed();
            if entries.is_empty() {
                return Err(anyhow!("排除 cdn 后没有可打包的资源"));
            }
        }
    }
    // 被 `--keep` 排除出容器的条目（微信字体）不在计划里，自然也不该参与"core 自足"断言：
    // 它们本来就是**目录里的真实文件**，按路径读得到，这正是 keep 的目的。
    let bundled: std::collections::BTreeSet<&str> =
        entries.iter().map(|(path, _, _)| path.as_str()).collect();
    let keys = bundle_plan::ProjectKeys {
        main_script: project_key_value(project, "main_script"),
        startup_scene: project_key_value(project, "startup_scene"),
        fonts: project_font_declarations(project)
            .into_iter()
            .filter(|font| bundled.contains(font.as_str()))
            .collect(),
    };
    validate_plan(
        &plan,
        entries.len() as u64,
        &keys,
        &config,
        MAX_BUNDLE_BYTES,
    )?;

    // 4) 逐组建包。
    let by_path: BTreeMap<&str, &PathBuf> = entries
        .iter()
        .map(|(relative, _, path)| (relative.as_str(), path))
        .collect();
    let mut bundle_paths: Vec<PathBuf> = Vec::new();
    let mut report_groups: Vec<GroupReport> = Vec::with_capacity(plan.groups.len());
    let (mut total_raw, mut total_bytes, mut total_blocks) = (0u64, 0u64, 0u64);
    let (mut total_index, mut total_index_raw) = (0u64, 0u64);
    for group in &mut plan.groups {
        let mut builder = BundleBuilder::new(block_size);
        // 块级 AEAD（docs/67 §4.4）：--encrypt 直接落进容器，贴图/音频/字体一起进加密域。
        if encrypt {
            builder = builder.with_encryption(true);
        }
        if fast {
            builder = builder.with_level(3);
        }
        // 组内装入次序 = **启动闭包优先**，其余按路径升序（`docs/67` §9 的性能项）。
        //
        // 为什么次序能省时间：容器按 256 KB 顺序切块，同块的条目共享一次解压。
        // 启动只读极少数条目（project.toml / 启动场景 / 主脚本 / 重定向表），
        // 它们在**哪个块**决定启动要解几次——散落在三个块里就要解三次。
        // 把它们排到组内最前面，启动就落在第 0 块（必要时第 1 块）。
        // 排序仍完全确定（同一输入 → 同一份字节），不吃掉 §4.5 的确定性要求。
        for relative in &group.paths {
            let path = by_path
                .get(relative.as_str())
                .ok_or_else(|| anyhow!("计划里的条目在产物里找不到：{relative}"))?;
            // 排序交给 builder：它内部会按 (优先级, 名字) 重排，在 assetc 侧先排是无效的（实测）。
            builder
                .add_ranked(relative, fs::read(path)?, boot_priority(relative))
                .map_err(|error| anyhow!("{error}"))?;
        }
        let built = builder
            .build()
            .map_err(|error| anyhow!("构建组 {} 失败：{error}", group.name))?;
        let name = format!(
            "{}-{:08x}.rlb",
            bundle_plan::sanitize(&group.name),
            built.checksum as u32
        );
        let relative = format!("{BUNDLE_DIR}/{name}");
        let path = out.join(BUNDLE_DIR).join(&name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, &built.bytes)?;
        // 复核 P0-1：单包上限按**落盘**字节判（微信分包口径），不是原始字节。
        if group.unit != "cdn" && built.bytes.len() as u64 > MAX_BUNDLE_BYTES {
            return Err(anyhow!(
                "组 {} 落盘 {} B 超过单包上限 {} B（微信分包口径）：请拆细规则或改用 unit = \"cdn\"",
                group.name,
                built.bytes.len(),
                MAX_BUNDLE_BYTES
            ));
        }
        total_raw += built.raw_len;
        total_bytes += built.bytes.len() as u64;
        total_blocks += u64::from(built.blocks);
        total_index += u64::from(built.index_len);
        total_index_raw += u64::from(built.index_raw_len);
        group.bundle = relative.clone();
        group.bundle_bytes = built.bytes.len() as u64;
        // 增量分发的比较键：整包 FNV（与文件名里的 8 位同源，这里给全 16 位）。
        group.hash = format!("{:016x}", built.checksum);
        report_groups.push(GroupReport {
            name: group.name.clone(),
            unit: group.unit.clone(),
            entries: group.entries,
            raw_bytes: built.raw_len,
            bundle_bytes: built.bytes.len() as u64,
            bundle: relative,
        });
        bundle_paths.push(path);
    }

    // 5) 计划表（先写：清单要覆盖它）。
    // 计划表自校验（复核 P2）：**写盘前**签，因为 `bundle`/`bundle_bytes`/`hash` 在逐组建包时
    // 才被回填；早签必然对不上。计划表是缺件归因的唯一依据，截断/错版必须当场发现。
    plan.keep = Some(kept_paths);
    plan.checksum = plan.compute_checksum();
    fs::write(&plan_path, plan_to_json(&plan)?)?;
    bundle_paths.push(plan_path.clone());

    // 6) 收尾。
    let mut report = PackageReport {
        schema: 2,
        mode: BundleMode::Grouped.token().to_owned(),
        files: 0,
        entries: entries.len() as u64,
        blocks: total_blocks,
        raw_bytes: total_raw,
        bundle_bytes: total_bytes,
        index_bytes: total_index,
        index_raw_bytes: total_index_raw,
        ratio_milli: PackageReport::ratio_milli(total_raw, total_bytes),
        bundle_path: report_groups
            .first()
            .map(|group| group.bundle.clone())
            .unwrap_or_default(),
        groups: report_groups,
    };
    finalize_container_pack(out, &files, &bundle_paths, keep, &mut report)?;
    Ok(report)
}

/// 容器形态的收尾：删掉已进容器的松散文件（保留 `project.toml` 与 `keep`）→ 写报告 → 重写清单。
///
/// `files` 是第 2 步收集到的**松散产物全集**（含清单与报告），`keep` 是必须留在磁盘上的
/// 已生成文件（容器、计划表）。
fn finalize_container_pack(
    out: &Path,
    files: &[(PathBuf, u64)],
    keep: &[PathBuf],
    prefixes: KeepPrefixes<'_>,
    report: &mut PackageReport,
) -> anyhow::Result<()> {
    let manifest_path = out.join("manifest.txt");
    let report_path = out.join(PACKAGE_REPORT_FILE);
    let project_toml = out.join("project.toml");

    for (path, _) in files {
        if path == &manifest_path || path == &report_path || path == &project_toml {
            continue;
        }
        if keep.iter().any(|kept| kept == path) {
            continue;
        }
        // `--keep` 排出去的文件（微信字体）本来就不在容器里，绝不能在这里被当"中间产物"删掉：
        // 删了之后容器读不到它（它不在容器里）、磁盘也没有，`wx.loadFont` 直接失效。
        let relative = path
            .strip_prefix(out)
            .map(|tail| tail.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if is_kept(&relative, prefixes) {
            continue;
        }
        if path.is_file() {
            fs::remove_file(path)?;
        }
    }
    for dir in ["scenes", "scripts", "assets"] {
        remove_dir_if_empty(&out.join(dir))?;
    }
    prune_empty_dirs(out, 0)?;

    let files_after = {
        let mut list = Vec::new();
        collect_files(out, &mut list)?;
        list.len() as u64
    };
    // 此刻磁盘上是"骨架 + 容器(+计划) + 第一轮留下的清单"，再加 1 份待写的报告。
    report.files = files_after + 1;
    fs::write(
        &report_path,
        serde_json::to_vec_pretty(report).map_err(|error| anyhow!("写报告失败：{error}"))?,
    )?;

    let mut listed = Vec::new();
    collect_files(out, &mut listed)?;
    listed.retain(|(path, _)| path != &manifest_path);
    listed.sort_by(|a, b| a.0.cmp(&b.0));
    let mut lines = vec![format!("assetc_manifest=1\t{PACKAGE_VERSION}")];
    for (path, _) in listed {
        let relative = path
            .strip_prefix(out)
            .map_err(|_| anyhow!("清单文件不在输出目录内：{}", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = fs::read(&path)?;
        lines.push(format!(
            "{relative}\t{}\t{:016x}",
            bytes.len(),
            fnv1a(&bytes)
        ));
    }
    fs::write(&manifest_path, lines.join("\n"))?;
    Ok(())
}

/// 读 `project.toml` 里的一个键（与 `project_font_key` 同一条逐行扫描口径）。
fn project_key_value(project: &Path, key: &str) -> Option<String> {
    let text = fs::read_to_string(project.join("project.toml")).ok()?;
    text.lines().find_map(|line| {
        let line = line.trim().trim_start_matches('\u{feff}');
        line.strip_prefix(&format!("{key} ="))
            .map(|rest| rest.trim().trim_matches('"').to_owned())
            .filter(|value| !value.is_empty())
    })
}

/// `assetc inspect <包目录|.rlb>`：打印容器索引摘要与**启动闭包落在第几个块**。
///
/// 为什么需要它：`game --check` 只说"1,371 个条目都读得出来"，看不出"启动要解几个块"——
/// 而那正是块排序优化的唯一指标。索引本身是 zstd 压缩的（`HEADER_FLAG_INDEX_COMPRESSED`），
/// 外部脚本解不了，所以这个读数只能由 Rust 侧给。
///
/// 输出刻意保持"一行一条 + 可 grep 的键值"：它同时是运维排查工具，
/// `assetc inspect <pack\bundles> | Select-String boot_blocks` 就是一条门。
/// 同 [`distribution_layout_report`]，但输出**机器可读 JSON**（发布脚本/适配层消费）。
///
/// 结构固定：`{ schema, entries, total_bytes, roots: [{ unit, root, bundles, entries, bytes }] }`。
/// 与文本版共用同一次 `distribution_layout` 调用，所以两者不可能给出不同的分组。
pub fn distribution_layout_json(target: &Path) -> anyhow::Result<String> {
    let plan_path = if target.is_dir() {
        target.join(bundle_plan::PLAN_FILE)
    } else {
        target.to_path_buf()
    };
    let bytes = fs::read(&plan_path)
        .map_err(|error| anyhow!("读 {} 失败：{error}", plan_path.display()))?;
    let plan = bundle_plan::BundlePlan::parse(&bytes).map_err(anyhow::Error::msg)?;
    let roots = bundle_plan::distribution_layout(&plan)?;
    let total: u64 = roots.iter().map(|root| root.bytes).sum();
    let entries: u64 = roots.iter().map(|root| root.entries).sum();
    let value = serde_json::json!({
        "schema": 1,
        "entries": entries,
        "total_bytes": total,
        "roots": roots
            .iter()
            .map(|root| serde_json::json!({
                "unit": root.unit,
                "root": root.root,
                "bundles": root.bundles,
                "entries": root.entries,
                "bytes": root.bytes,
            }))
            .collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&value).map_err(|error| anyhow!("序列化布局失败：{error}"))
}

/// 为任意工程生成一份 `bundles.json` 草稿（引擎级通用规则），可选 `force` 覆盖。
///
/// 用途：新工程接入分包时的**零猜测起点**——不绑定任何具体游戏路径，
/// 之后由编辑器面板或手改细化。
pub fn init_bundles(project: &Path, force: bool) -> anyhow::Result<String> {
    let path = project.join("bundles.json");
    if path.exists() && !force {
        anyhow::bail!(
            "{} 已存在：用 --force 覆盖，或直接在编辑器「资源 ▸ 分包」里编辑",
            path.display()
        );
    }
    let config = rustlua_infra::bundle_plan::BundleConfig::generic_domains();
    config.validate()?;
    std::fs::write(&path, config.to_json()?)?;
    Ok(format!(
        "assetc_bundles_init_ok path={} groups={}",
        path.display(),
        config.groups.len()
    ))
}

/// 读 `bundle_plan.json` 并打印分发布局（单元 → 包内根），供发布脚本消费。
pub fn distribution_layout_report(target: &Path) -> anyhow::Result<String> {
    let plan_path = if target.is_dir() {
        target.join(bundle_plan::PLAN_FILE)
    } else {
        target.to_path_buf()
    };
    let bytes = fs::read(&plan_path)
        .map_err(|error| anyhow!("读 {} 失败：{error}", plan_path.display()))?;
    let plan = bundle_plan::BundlePlan::parse(&bytes).map_err(anyhow::Error::msg)?;
    let roots = bundle_plan::distribution_layout(&plan)?;
    let mut out = String::from("assetc_layout");
    let mut total = 0u64;
    for root in &roots {
        total += root.bytes;
        out.push_str(&format!(
            "\n{} unit={} bundles={} entries={} kb={}",
            root.root,
            root.unit,
            root.bundles.len(),
            root.entries,
            root.bytes / 1024
        ));
    }
    let entries: u64 = plan.groups.iter().map(|group| group.entries).sum();
    out.push_str(&format!(
        "\nassetc_layout_ok roots={} entries={entries} kb={}",
        roots.len(),
        total / 1024
    ));
    Ok(out)
}

pub fn inspect_bundles(target: &Path) -> anyhow::Result<String> {
    use rustlua_infra::bundle::BundleIndex;

    // 启动闭包：boot 期间确实会读的条目（与 `boot_priority` 同口径）。
    let is_boot = |name: &str| {
        name == "project.toml"
            || name == "atlas_redirect.json"
            // 只认**启动场景**：把所有 scenes/* 都算进来会得到 3 个块的假读数
            // （那些场景启动时根本不读，只在切场景时才用）。
            || name == "scenes/main.scene"
            || name == "scripts/main.lua"
    };

    let mut files: Vec<PathBuf> = Vec::new();
    if target.is_file() {
        files.push(target.to_path_buf());
    } else {
        let nested = target.join(BUNDLE_DIR);
        let dir = if nested.is_dir() {
            nested
        } else {
            target.to_path_buf()
        };
        for entry in fs::read_dir(&dir)
            .map_err(|error| anyhow!("读 {} 失败：{error}", dir.display()))?
            .flatten()
        {
            let path = entry.path();
            if path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("rlb"))
            {
                files.push(path);
            }
        }
        files.sort();
    }
    if files.is_empty() {
        return Err(anyhow!("没有找到 .rlb 容器：{}", target.display()));
    }

    let mut out = String::new();
    let mut total_blocks = 0usize;
    let mut boot_places: Vec<(String, u32, String)> = Vec::new();
    for file in &files {
        let bytes =
            fs::read(file).map_err(|error| anyhow!("读 {} 失败：{error}", file.display()))?;
        let index =
            BundleIndex::parse(&bytes).map_err(|error| anyhow!("{}: {error}", file.display()))?;
        let name = file
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        let index_kb = index
            .blocks()
            .iter()
            .map(|block| block.stored_len as u64)
            .sum::<u64>()
            / 1024;
        out.push_str(&format!(
            "bundle {name} blocks={} entries={} payload_kb={index_kb} block_size={}\n",
            index.blocks().len(),
            index.len(),
            index.block_size()
        ));
        total_blocks += index.blocks().len();
        let mut per_block = vec![0usize; index.blocks().len()];
        for entry in index.entries() {
            if let Some(slot) = per_block.get_mut(entry.block as usize) {
                *slot += 1;
            }
        }
        for (number, block) in index.blocks().iter().enumerate() {
            out.push_str(&format!(
                "  block {number} flags={} stored={} raw={} entries={}\n",
                block.flags, block.stored_len, block.raw_len, per_block[number]
            ));
        }
        for entry in index.entries() {
            if is_boot(&entry.path) {
                boot_places.push((name.clone(), entry.block, entry.path.clone()));
            }
        }
    }
    boot_places.sort_by(|a, b| a.2.cmp(&b.2));
    for (bundle, block, name) in &boot_places {
        out.push_str(&format!("boot {bundle} block={block} {name}\n"));
    }
    let distinct: std::collections::BTreeSet<(String, u32)> = boot_places
        .iter()
        .map(|(bundle, block, _)| (bundle.clone(), *block))
        .collect();
    out.push_str(&format!(
        "inspect_ok bundles={} boot_entries={} boot_blocks={} total_blocks={total_blocks}",
        files.len(),
        boot_places.len(),
        distinct.len()
    ));
    Ok(out)
}

/// 等价性校验（`docs/67` 复核 P0-2）：容器形态的**每个条目**都必须与目录形态逐字节相等。
///
/// 为什么要有它：`game --check` 里的逐条目 FNV 只证明"容器内部自洽"，
/// 它证明不了"容器里的内容 == 目录形态的内容"。两者是不同的断言，发布前要都成立。
/// 这条入口同时也是发布脚本可以常跑的门（不是一次性测试）。
pub fn verify_bundle_equivalence(loose: &Path, container: &Path) -> anyhow::Result<usize> {
    use rustlua_infra::bundle::BundleSet;
    let bundles_dir = container.join(BUNDLE_DIR);
    let mut files: Vec<PathBuf> = fs::read_dir(&bundles_dir)
        .map_err(|error| anyhow!("读 {} 失败：{error}", bundles_dir.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("rlb"))
        })
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(anyhow!("{} 下没有 .rlb 容器", bundles_dir.display()));
    }
    let mut set = BundleSet::new(container);
    for file in &files {
        set.mount_file(file)
            .map_err(|error| anyhow!("挂载 {} 失败：{error}", file.display()))?;
    }
    let mut compared = 0usize;
    let mut first_mismatch: Option<String> = None;
    for path in set.paths().map(str::to_owned).collect::<Vec<_>>() {
        let expected = fs::read(loose.join(&path))
            .map_err(|error| anyhow!("目录形态缺少条目 {path}（容器里有）：{error}"))?;
        let actual = set
            .read(&path)?
            .ok_or_else(|| anyhow!("容器读不出条目 {path}"))?;
        compared += 1;
        if actual != expected && first_mismatch.is_none() {
            first_mismatch = Some(format!(
                "{path}（容器 {} B / 目录 {} B）",
                actual.len(),
                expected.len()
            ));
        }
    }
    if let Some(mismatch) = first_mismatch {
        return Err(anyhow!("条目字节不一致：{mismatch}"));
    }
    Ok(compared)
}

/// 删空目录（非递归、失败忽略）：`assets/` 下的小目录要一起收掉，否则目录形态的残骸还在。
fn remove_dir_if_empty(dir: &Path) -> anyhow::Result<()> {
    if dir.is_dir() && fs::read_dir(dir)?.next().is_none() {
        fs::remove_dir(dir)?;
    }
    Ok(())
}

/// 自底向上删掉所有空目录（限制深度，防误伤）。
fn prune_empty_dirs(root: &Path, depth: usize) -> anyhow::Result<()> {
    if depth > 16 {
        return Ok(());
    }
    let mut children = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            prune_empty_dirs(&path, depth + 1)?;
            children.push(path);
        }
    }
    for path in children {
        remove_dir_if_empty(&path)?;
    }
    Ok(())
}

/// Emit the static resource graph used by synchronous runtimes to avoid
/// loading unrelated package entries. Any dynamic/unsupported resource access
/// marks its source opaque so consumers can conservatively preload everything.
fn write_dependency_index(project: &Path, destination: &Path) -> anyhow::Result<()> {
    let mut files = Vec::new();
    for root in ["scenes", "scripts", "assets"] {
        let dir = project.join(root);
        if dir.is_dir() {
            collect_files(&dir, &mut files)?;
        }
    }
    let mut known = BTreeMap::<String, String>::new();
    let mut sources = Vec::new();
    for (path, _) in files {
        let relative = path
            .strip_prefix(project)
            .map_err(|_| anyhow!("依赖索引源文件不在项目目录内：{}", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        let folded = relative.to_ascii_lowercase();
        known.insert(folded, relative.clone());
        sources.push((relative, path));
    }

    let mut edges = BTreeMap::<String, std::collections::BTreeSet<String>>::new();
    let mut opaque = std::collections::BTreeSet::new();
    for (source, path) in sources {
        let kind = ResourceKind::from_path(&path);
        if !matches!(
            kind,
            ResourceKind::Scene
                | ResourceKind::Prefab
                | ResourceKind::Atlas
                | ResourceKind::LuaScript
        ) {
            continue;
        }
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => {
                opaque.insert(source);
                continue;
            }
        };
        // These APIs consume runtime paths synchronously. Most call sites pass a literal
        // (often through a same-file constant), which is provable; only the ones that stay
        // unresolvable after that keep the conservative full-package fallback.
        //
        // `audio.*` 曾经也在这张表里，但那条判据是反的：音频在运行期由宿主自己按名字取回
        // （浏览器 `pack/assets/audio/<name>.mp3`、微信走适配层路由），**不读**同步 vfs 预载表。
        // 把它算进"必须证明完整"的同步读取里，代价是每个调用 `audio.sfx(...)` 的脚本都被标
        // opaque——本工程 155 个 Lua 里有 88 个，全量预载的触发点正是这里。
        let unresolved_calls = if kind == ResourceKind::LuaScript {
            scan_path_call_args(&text)
        } else {
            Vec::new()
        };
        // 字面量截断（`"assets/ui/prefabs/"`、`"assets/audio/"`）本身不能定位文件，但当它有
        // 包内命中时，那些命中就是可证的边——否则一个目录级常量会把它所在文件整片标成 opaque。
        let prefix_refs = if kind == ResourceKind::LuaScript {
            asset_prefix_refs(&text)
        } else {
            Vec::new()
        };
        // 同文件字面量常量表：`local PRESET_PATH = "assets/prefabs/dialogue.prefab"` 这类写法
        // 让"变量实参"其实完全可证。非 Lua 源没有调用实参，表留空即可。
        let constants = if kind == ResourceKind::LuaScript {
            assigned_literals(&text)
        } else {
            BTreeMap::new()
        };
        let mut refs = extract_refs(&text, kind);
        let resolved = if kind == ResourceKind::LuaScript {
            let mut paths = Vec::new();
            for argument in &unresolved_calls {
                if let Some(path) = resolve_static_path(argument, &constants, &known) {
                    paths.push(path);
                }
            }
            for prefix in &prefix_refs {
                paths.extend(expand_asset_prefix(prefix, &known));
            }
            paths
        } else {
            Vec::new()
        };
        for path in resolved {
            let reference = DependencyRef::Asset(std::path::PathBuf::from(path));
            if !refs.contains(&reference) {
                refs.push(reference);
            }
        }
        let mut targets = std::collections::BTreeSet::new();
        for reference in refs {
            let candidate = match reference {
                DependencyRef::Asset(path) => path.to_string_lossy().replace('\\', "/"),
                DependencyRef::LuaModule(name) => module_path(&name),
                DependencyRef::SceneName(name) => scene_path(&name),
                DependencyRef::Dynamic(_) => {
                    opaque.insert(source.clone());
                    continue;
                }
            };
            if let Some(target) = known
                .get(&candidate.to_ascii_lowercase())
                .or_else(|| known.get(&candidate))
            {
                if target != &source {
                    targets.insert(target.clone());
                }
            } else if candidate.contains("assets/")
                || candidate == "project.toml"
                || candidate == "chars.txt"
            {
                // 根级工程文件的字面量引用是合法的（本索引只扫 scenes/scripts/assets，
                // 它们在包内但不在 known 里）；而 `assets/` 路径不是"引用不存在的东西"，
                // 只是这个文件不随包分发（例：M71 退役后已删除的 `assets/ui/prefabs/*.ui.json`），
                // 运行期同样读不到——两种情况都不需要把整包拖成全量预载。
                continue;
            } else {
                opaque.insert(source.clone());
            }
        }
        if targetless_asset_args(&unresolved_calls, &constants, &known) {
            // 实参确实是 `assets/` 路径却谁也解析不出来：索引无法证明闭包完整，回退全量。
            opaque.insert(source.clone());
        }
        if !targets.is_empty() {
            edges.insert(source, targets);
        }
    }

    let index = DependencyIndex {
        format: 1,
        edges: edges
            .into_iter()
            .map(|(source, targets)| (source, targets.into_iter().collect()))
            .collect(),
        opaque: opaque.into_iter().collect(),
    };
    let bytes =
        serde_json::to_vec(&index).map_err(|error| anyhow!("序列化依赖索引失败：{error}"))?;
    fs::write(destination, bytes)?;
    Ok(())
}

/// 同文件字面量常量表：`local PRESET_PATH = "assets/prefabs/dialogue.prefab"` → `PRESET_PATH`。
///
/// 只做这一种形态（标识符 = 纯字符串字面量），不做表达式求值：拼接、表字段、函数返回值
/// 一律留在"证不出来"那一侧，由调用方标 opaque 走全量兜底。这样静态分析的能力边界是
/// 一句话能说完的，而不是"基本能猜"。
fn assigned_literals(text: &str) -> BTreeMap<String, String> {
    // `==` 的左侧不是赋值：不排掉的话 `if M.DIR == "assets/ui/"` 会造出一条假常量。
    let text = text.replace("==", "!=");
    let chars: Vec<char> = text.chars().collect();
    let mut literals = BTreeMap::new();
    for quote in ['"', '\''] {
        let mut search = 0;
        while let Some(offset) = text[search..].find(quote) {
            let start = search + offset;
            search = start + 1;
            let Some(body) = literal_at(&chars, start) else {
                continue;
            };
            if let Some(name) = assignment_name(&chars, start) {
                literals.insert(name, body);
            }
        }
    }
    literals
}

/// 位置 `start` 处是一个字符串字面量时返回去引号的正文（含转义原样保留）。
fn literal_at(chars: &[char], start: usize) -> Option<String> {
    let quote = *chars.get(start)?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let mut index = start + 1;
    let mut body = String::new();
    while index < chars.len() {
        let c = chars[index];
        if c == '\\' {
            if let Some(&escaped) = chars.get(index + 1) {
                body.push(escaped);
                index += 2;
                continue;
            }
            break;
        }
        if c == quote {
            return Some(body);
        }
        body.push(c);
        index += 1;
    }
    None
}

/// 字面量之前紧邻 `=`（且不是 `==`）时，反向读出被赋值的名字。
fn assignment_name(chars: &[char], quote_index: usize) -> Option<String> {
    let mut index = quote_index;
    while index > 0 && chars[index - 1].is_whitespace() {
        index -= 1;
    }
    if index == 0 || chars[index - 1] != '=' {
        return None;
    }
    index -= 1;
    if index > 0 && chars[index - 1] == '=' {
        return None;
    }
    while index > 0 && chars[index - 1].is_whitespace() {
        index -= 1;
    }
    let end = index;
    while index > 0 && (chars[index - 1].is_alphanumeric() || matches!(chars[index - 1], '_' | '.'))
    {
        index -= 1;
    }
    (index < end).then(|| chars[index..end].iter().collect())
}

/// 调用实参 → 包内路径。`"assets/…"` 直接归一化；标识符查同文件常量表；查到的值再归一化一次。
fn resolve_static_path(
    argument: &str,
    constants: &BTreeMap<String, String>,
    known: &BTreeMap<String, String>,
) -> Option<String> {
    let trimmed = argument.trim();
    if let Some(value) = literal_body(trimmed) {
        return normalize_asset_path(&value, known);
    }
    let name = trimmed.trim_end_matches([')', ',', ';']);
    let value = constants.get(name)?;
    normalize_asset_path(value, known)
}

/// 去引号（`trim_matches` 与扫描器同口径：首尾成对的引号，中间不再含引号）。
fn literal_body(trimmed: &str) -> Option<String> {
    if trimmed.len() < 2 || !trimmed.starts_with(['"', '\'']) {
        return None;
    }
    let body = trimmed.trim_matches(['"', '\'']);
    (body != trimmed || trimmed.len() == 2).then(|| body.to_owned())
}

/// 绝对化的**包内**资源路径才算边：包外路径（用户目录、`/tmp/...`）运行期同样取不到，
/// 不算"引用不存在的东西"。大小写回落到项目里的真实写法（索引与清单按原样比对）。
fn normalize_asset_path(value: &str, known: &BTreeMap<String, String>) -> Option<String> {
    let trimmed = value.trim();
    let index = trimmed.find("assets/")?;
    let candidate = trimmed[index..].trim_end_matches([';', ':', '\n', '\r']);
    known
        .get(&candidate.to_ascii_lowercase())
        .or_else(|| known.get(candidate))
        .cloned()
}

/// 文件里出现的、以 `assets/` 开头但**在引用处就截断**的字符串前缀（`"assets/ui/prefabs/"`）。
///
/// 只收以 `/` 结尾的整串字面量：完整路径由 `lua_string_refs` 的 `assets/` 分支产出，
/// 拼接中段（`"assets/" .. kind .. "/x.png"`）也在这里产出，两者都可能不是完整路径，
/// 交给 `expand_asset_prefix` 按包内命中判定；命中不了就丢弃（同"不存在"口径）。
fn asset_prefix_refs(text: &str) -> Vec<String> {
    let mut prefixes = Vec::new();
    for literal in string_literals(text) {
        if literal.starts_with("assets/")
            && literal.ends_with('/')
            // 仅 `"assets/"`（无下钻）不算目录引用：它是**校验用的字面量**
            // （`string.find(entry.pattern, "assets/", 1, true)` 这类"路径确实是工程相对资产路径吗"
            // 的判据），展开它等于把整包挂成这个文件闭包的一部分。
            // 2026-10-17 实测：`scripts/data/season_art.lua:114` 的这一个字面量让它的边涨到
            // 1,207 条（整棵 assets 树、含 17.7 MB 的已停用字体），运行期的闭包选择因此永远
            // 走不到"少量首屏资源"那条路。目录引用至少给到一级子目录才有信息量。
            && literal.trim_end_matches('/').len() > "assets".len()
            && !prefixes.contains(&literal)
        {
            prefixes.push(literal);
        }
    }
    prefixes
}

/// 收集全部字符串字面量的正文（引号感知，`--` 注释直接跳过，行为与扫描器同源）。
fn string_literals(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut literals = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if c == '-' && chars.get(index + 1) == Some(&'-') {
            while index < chars.len() && chars[index] != '\n' {
                index += 1;
            }
            continue;
        }
        if c == '"' || c == '\'' {
            if let Some(body) = literal_at(&chars, index) {
                literals.push(body);
            }
            index += 1;
            continue;
        }
        index += 1;
    }
    literals
}

/// 前缀 → 包内命中全集（`"assets/ui/prefabs/"` → `assets/ui/prefabs/*.ui.json`）。
///
/// 目录常量 + 变量名拼出的路径**只可能**落在该目录下，所以这些命中是完整闭包而不是猜测；
/// 命中集为空时返回空（该前缀指的是包内不存在的东西）。
fn expand_asset_prefix(prefix: &str, known: &BTreeMap<String, String>) -> Vec<String> {
    let lowered = prefix.to_ascii_lowercase();
    known
        .iter()
        .filter(|(folded, _)| folded.starts_with(&lowered))
        .map(|(_, actual)| actual.clone())
        .collect()
}

/// 实参已能证明是 `assets/` 路径、却既不完整也不指向任何包内文件：
/// 这才是真的"索引证明不了闭包完整"，必须回退全量。
fn targetless_asset_args(
    unresolved: &[String],
    constants: &BTreeMap<String, String>,
    known: &BTreeMap<String, String>,
) -> bool {
    unresolved.iter().any(|argument| {
        let trimmed = argument.trim();
        let text = match literal_body(trimmed) {
            Some(value) => value,
            None => match constants.get(trimmed.trim_end_matches([')', ',', ';'])) {
                Some(value) => value.clone(),
                None => return false,
            },
        };
        // 绝对路径（`/…`）不属于包内资源，运行期同样读不到，不算索引的缺口。
        text.contains("assets/")
            && !text.starts_with('/')
            && normalize_asset_path(&text, known).is_none()
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// 点号模块名必须换算成子目录路径：`require("systems.farm")` 指的是
    /// `scripts/systems/farm.lua`。写错这一步的表现是**索引里全是 opaque**——
    /// `require` 的边一条都连不上，然后主脚本一出发就全量预载（真实工程实测：
    /// 155 个 Lua 里 83 个被标 opaque，`full_fallback=true`）。
    #[test]
    fn package_dependency_index_resolves_dotted_require_to_subdirectory() {
        let root = temp_project("package-dependency-index-dotted");
        fs::create_dir_all(root.join("scripts/systems")).unwrap();
        fs::write(
            root.join("scripts/systems/farm.lua"),
            "local M = {}\nreturn M\n",
        )
        .unwrap();
        // 目录分隔的写法（老工程里也存在）走同一条换算，两者必须都连上。
        fs::create_dir_all(root.join("scripts/ui")).unwrap();
        fs::write(root.join("scripts/ui/hud.lua"), "return {}\n").unwrap();
        fs::write(
            root.join("scripts/main.lua"),
            "local farm = require('systems.farm')\nlocal hud = require('ui/hud')\nreturn farm, hud\n",
        )
        .unwrap();
        let out = std::env::temp_dir().join(format!(
            "rustlua-assetc-dependency-index-dotted-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let _ = fs::remove_dir_all(&out);
        package(&root, &out, false, false, false).unwrap();
        let index: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("dependencies.json")).unwrap()).unwrap();
        let edges = index["edges"]["scripts/main.lua"].as_array().unwrap();
        for expected in ["scripts/systems/farm.lua", "scripts/ui/hud.lua"] {
            assert!(
                edges.iter().any(|path| path == expected),
                "require 边缺失 {expected}：{index}"
            );
        }
        assert!(
            index["opaque"].as_array().unwrap().is_empty(),
            "点号模块名不该把自己标成 opaque：{index}"
        );
        fs::remove_dir_all(&out).ok();
        cleanup(&root);
    }

    /// 声明的启动图必须随包，**哪怕它不在 `assets/` 下**（`splash/` 这类顶层目录）。
    ///
    /// 漏了这一手的症状不是"打包失败"，而是运行期静默回落到编译期内置图（日志里只有一条
    /// warn）——玩家看到的仍是引擎默认启动图，而包看着完全正常。
    #[test]
    fn declared_splash_outside_assets_is_packaged() {
        let root = temp_project("declared-splash");
        fs::create_dir_all(root.join("splash")).unwrap();
        // 与 `assets/textures/red-1x1.png` 同一份合法 PNG：内容必须逐字节进包。
        let png = fs::read(root.join("assets/textures/red-1x1.png")).unwrap();
        fs::write(root.join("splash/splash.png"), &png).unwrap();
        let config = fs::read_to_string(root.join("project.toml")).unwrap();
        fs::write(
            root.join("project.toml"),
            format!("{config}splash_image = \"splash/splash.png\"\n"),
        )
        .unwrap();

        let out = std::env::temp_dir().join(format!(
            "rustlua-assetc-declared-splash-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let _ = fs::remove_dir_all(&out);
        package(&root, &out, false, false, false).unwrap();
        let manifest = fs::read_to_string(out.join("manifest.txt")).unwrap();
        assert!(
            manifest
                .lines()
                .any(|line| line.starts_with("splash/splash.png\t")),
            "声明的启动图必须进清单：{manifest}"
        );
        assert_eq!(fs::read(out.join("splash/splash.png")).unwrap(), png);

        fs::remove_dir_all(&out).ok();
        cleanup(&root);
    }

    /// 声明图越出工程根必须报错：`fs::copy` 会把项目外的文件悄悄塞进发布包。
    #[test]
    fn declared_splash_rejects_escaping_path() {
        let root = temp_project("declared-splash-escape");
        let config = fs::read_to_string(root.join("project.toml")).unwrap();
        fs::write(
            root.join("project.toml"),
            format!("{config}splash_image = \"../outside.png\"\n"),
        )
        .unwrap();
        let error = package(
            &root,
            &std::env::temp_dir().join(format!(
                "rustlua-assetc-declared-splash-escape-{}-{}",
                std::process::id(),
                unique_suffix()
            )),
            false,
            false,
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("splash_image") && error.contains("相对路径"),
            "{error}"
        );
        cleanup(&root);
    }

    fn temp_project(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("rustlua-assetc-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("assets/textures")).unwrap();
        fs::create_dir_all(root.join("scenes")).unwrap();
        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::write(
            root.join("project.toml"),
            "[project]\nname = \"t\"\nversion = \"0.1.0\"\nengine = \"RustLua\"\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n",
        )
        .unwrap();
        fs::write(
            root.join("scripts/main.lua"),
            "-- ok\nfunction update() return 'ok' end\n",
        )
        .unwrap();
        fs::write(
            root.join("scenes/main.scene"),
            "scene_version=2\nname=main\n",
        )
        .unwrap();
        // 1x1 红色 PNG（有效可解码，与 game/assets/textures/red-1x1.png 一致）
        let png = [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99,
            0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        fs::write(root.join("assets/textures/red-1x1.png"), png).unwrap();
        root
    }

    fn unique_suffix() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos()
    }

    fn sample_project_root() -> PathBuf {
        rustlua_infra::games_dir().join("examples").join("game_api")
    }

    fn copy_sample_project(label: &str) -> Option<PathBuf> {
        let source = sample_project_root();
        if !source.join("project.toml").is_file() {
            return None;
        }
        let root = std::env::temp_dir().join(format!(
            "rustlua-assetc-sample-{label}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        copy_file(&source.join("project.toml"), &root.join("project.toml")).unwrap();
        copy_dir(&source.join("scripts"), &root.join("scripts")).unwrap();
        copy_dir(&source.join("scenes"), &root.join("scenes")).unwrap();
        copy_dir(&source.join("assets"), &root.join("assets")).unwrap();
        Some(root)
    }

    fn cleanup(root: &Path) {
        fs::remove_dir_all(root).ok();
    }

    fn replace_in_file(path: &Path, from: &str, to: &str) {
        let text = fs::read_to_string(path).unwrap();
        assert!(
            text.contains(from),
            "expected {} to contain {from}",
            path.display()
        );
        fs::write(path, text.replace(from, to)).unwrap();
    }

    #[test]
    fn scan_registers_supported_and_marks_unsupported() {
        let root = temp_project("scan");
        let entries = scan(&root).unwrap();
        let supported: Vec<_> = entries
            .iter()
            .filter(|e| e.status != "unsupported")
            .collect();
        assert!(
            !supported.is_empty(),
            "should register at least one resource"
        );
        assert!(supported.iter().all(|e| e.asset_id.is_some()));
        assert!(entries.iter().any(|e| e.path.ends_with("red-1x1.png")));
        assert!(entries.iter().any(|e| e.path.ends_with("main.lua")));
        cleanup(&root);
    }

    #[test]
    fn import_is_incremental_and_reuses_cache() {
        let root = temp_project("import");
        let first = import(&root).unwrap();
        assert!(first.iter().all(|o| o.error.is_none()), "first import ok");
        assert!(!first.is_empty());
        let second = import(&root).unwrap();
        assert!(
            second.iter().all(|o| o.reused),
            "second import reuses cache"
        );
        cleanup(&root);
    }

    #[test]
    fn import_does_not_abort_on_single_bad_asset() {
        let root = temp_project("invalid-gltf");
        fs::write(root.join("assets/bad.glb"), b"invalid glb").unwrap();
        let outcomes = import(&root).unwrap();
        let bad = outcomes
            .iter()
            .find(|outcome| outcome.path.ends_with("bad.glb"))
            .expect("bad item present in outcomes");
        assert!(bad.error.is_some(), "bad.glb records an error");
        let good: Vec<_> = outcomes
            .iter()
            .filter(|outcome| !outcome.path.ends_with("bad.glb"))
            .collect();
        assert!(!good.is_empty(), "other assets still imported");
        assert!(good.iter().all(|outcome| outcome.error.is_none()));
        cleanup(&root);
    }

    #[test]
    fn scan_reports_invalid_when_dependency_missing() {
        let root = temp_project("invalid");
        // 场景引用 assets/textures/red-1x1.png。
        fs::write(
            root.join("scenes/main.scene"),
            "[scene]\nSprite texture=assets/textures/red-1x1.png;size=32\n",
        )
        .unwrap();
        // 首次 scan：注册纹理并建立 scene→texture 依赖边。
        scan(&root).unwrap();
        // 删除被引用的纹理 → 再次 scan 应把消费者标 invalid（依赖缺失）。
        fs::remove_file(root.join("assets/textures/red-1x1.png")).unwrap();
        let entries = scan(&root).unwrap();
        let scene = entries
            .iter()
            .find(|e| e.path.ends_with("main.scene"))
            .expect("scene present in scan");
        assert_eq!(scene.status, "invalid");
        assert!(
            scene.error.as_deref().unwrap_or("").contains("缺失"),
            "error 应说明依赖缺失：{:?}",
            scene.error
        );
        cleanup(&root);
    }

    #[test]
    fn check_deps_reports_full_chain_consumer_to_root() {
        let root = temp_project("checkdeps-chain");
        // 三层链路：main.scene → hero.prefab → red-1x1.png。
        fs::create_dir_all(root.join("assets/sprites")).unwrap();
        fs::write(
            root.join("scenes/main.scene"),
            "[scene]\nSprite texture=assets/sprites/hero.prefab;size=32\n",
        )
        .unwrap();
        fs::write(
            root.join("assets/sprites/hero.prefab"),
            "prefab_version=1\nSprite texture=assets/textures/red-1x1.png;size=32\n",
        )
        .unwrap();
        // 首次 scan 注册并建边。
        scan(&root).unwrap();
        // 删除根纹理 → 整个链路失效。
        fs::remove_file(root.join("assets/textures/red-1x1.png")).unwrap();
        let links = check_dependencies(&root).unwrap();
        let scene_link = links
            .iter()
            .find(|link| link.consumer.ends_with("main.scene"))
            .expect("scene 断链存在");
        assert_eq!(scene_link.chain.len(), 3, "完整链路含消费者与断链根");
        assert!(scene_link.chain[0].ends_with("main.scene"));
        assert!(scene_link.chain[1].ends_with("hero.prefab"));
        assert!(scene_link.chain[2].ends_with("red-1x1.png"));
        assert_eq!(scene_link.reason, "缺失");
        cleanup(&root);
    }

    #[test]
    fn check_deps_truncates_on_cycle() {
        let root = temp_project("checkdeps-cycle");
        // a/b 两个 prefab 互相引用成环；a 依赖会缺失的纹理，依赖顺序让
        // walk 优先沿环下探（同 rank 取最后一个），从而触发循环截断。
        fs::write(
            root.join("assets/a.prefab"),
            "prefab_version=1\nSprite texture=assets/textures/red-1x1.png;size=32\nSprite texture=assets/b.prefab;size=32\n",
        )
        .unwrap();
        fs::write(
            root.join("assets/b.prefab"),
            "prefab_version=1\nSprite texture=assets/a.prefab;size=32\n",
        )
        .unwrap();
        scan(&root).unwrap();
        fs::remove_file(root.join("assets/textures/red-1x1.png")).unwrap();
        let links = check_dependencies(&root).unwrap();
        assert!(!links.is_empty(), "断链存在：{links:?}");
        assert!(
            links.iter().any(|link| link.reason == "循环依赖"),
            "成环下探应截断并标注循环依赖：{links:?}"
        );
        cleanup(&root);
    }

    #[test]
    fn check_deps_empty_when_healthy() {
        let root = temp_project("checkdeps-ok");
        // 场景引用已存在的纹理 → 无 Invalid。
        fs::write(
            root.join("scenes/main.scene"),
            "[scene]\nSprite texture=assets/textures/red-1x1.png;size=32\n",
        )
        .unwrap();
        let links = check_dependencies(&root).unwrap();
        assert!(links.is_empty(), "健康项目无断链：{links:?}");
        cleanup(&root);
    }

    #[test]
    fn clean_drops_import_cache() {
        let root = temp_project("clean");
        let outcomes = import(&root).unwrap();
        assert!(
            !outcomes.is_empty(),
            "import produced outcomes: {outcomes:?}"
        );
        // 用文件存在性断言（Windows 上对 Temp 目录 read_dir 有解析怪癖）
        for outcome in &outcomes {
            assert!(
                root.join(outcome.cache_path.as_ref().unwrap()).is_file(),
                "cache file exists after import: {:?}",
                outcome.cache_path
            );
        }
        clean(&root).unwrap();
        for outcome in &outcomes {
            assert!(
                !root.join(outcome.cache_path.as_ref().unwrap()).exists(),
                "cache file removed after clean: {:?}",
                outcome.cache_path
            );
        }
        cleanup(&root);
    }

    #[test]
    fn package_copies_data_and_writes_manifest() {
        let root = temp_project("package");
        let out = std::env::temp_dir().join(format!("rustlua-assetc-out-{}", std::process::id()));
        let _ = fs::remove_dir_all(&out);
        package(&root, &out, false, false, false).unwrap();
        assert!(out.join("project.toml").is_file());
        assert!(out.join("scripts/main.lua").is_file());
        assert!(out.join("assets/textures/red-1x1.png").is_file());
        assert!(out.join("dependencies.json").is_file());
        let manifest_text = fs::read_to_string(out.join("manifest.txt")).unwrap();
        assert_eq!(
            manifest_text.lines().next(),
            Some("assetc_manifest=1\tassetc-package-2")
        );
        assert!(
            manifest_text
                .lines()
                .any(|line| line.starts_with("dependencies.json\t")),
            "manifest includes the generated dependency index"
        );
        assert!(manifest_text.contains("red-1x1.png"), "manifest lists png");
        assert!(manifest_text.contains("main.lua"), "manifest lists lua");
        fs::remove_dir_all(&out).ok();
        cleanup(&root);
    }

    /// 微信形态配额门（docs/52）：`--drop-undeclared-fonts` 只留 `project.toml` 声明的
    /// `font`/`font_fallback`；未声明字体二进制不随包发、也不进 `manifest.txt`。
    /// 反面对照：不开旗标（PC 形态）时全量字体原样保留。
    #[test]
    fn package_drops_undeclared_fonts_when_requested() {
        let root = temp_project("package-font-drop");
        let mut toml = fs::read_to_string(root.join("project.toml")).unwrap();
        toml.push_str("font = \"assets/fonts/kept.ttf\"\nfont_fallback = \"\"\n");
        fs::write(root.join("project.toml"), toml).unwrap();
        fs::create_dir_all(root.join("assets/fonts")).unwrap();
        fs::write(root.join("assets/fonts/kept.ttf"), b"kept-font").unwrap();
        fs::write(root.join("assets/fonts/dead-weight.ttf"), b"dead").unwrap();
        fs::write(root.join("assets/fonts/LICENSE.txt"), b"lic").unwrap();
        let out =
            std::env::temp_dir().join(format!("rustlua-assetc-dropout-{}", std::process::id()));
        let out_keep =
            std::env::temp_dir().join(format!("rustlua-assetc-keepout-{}", std::process::id()));
        let _ = fs::remove_dir_all(&out);
        let _ = fs::remove_dir_all(&out_keep);
        package(&root, &out, false, false, true).unwrap();
        assert!(out.join("assets/fonts/kept.ttf").is_file());
        assert!(!out.join("assets/fonts/dead-weight.ttf").exists());
        assert!(
            out.join("assets/fonts/LICENSE.txt").is_file(),
            "非字体文件（授权说明）不受剔除影响"
        );
        let manifest_text = fs::read_to_string(out.join("manifest.txt")).unwrap();
        assert!(
            !manifest_text.contains("dead-weight.ttf"),
            "清单不得再列被剔除的字体（运行时按清单校验）"
        );
        assert!(manifest_text.contains("kept.ttf"));
        // 对照：不开旗标 = 既有 PC 行为逐字不变。
        package(&root, &out_keep, false, false, false).unwrap();
        assert!(out_keep.join("assets/fonts/dead-weight.ttf").is_file());
        fs::remove_dir_all(&out).ok();
        fs::remove_dir_all(&out_keep).ok();
        cleanup(&root);
    }

    #[test]
    fn package_dependency_index_records_static_edges_and_opaque_sources() {
        let root = temp_project("package-dependency-index");
        fs::write(
            root.join("scenes/main.scene"),
            "scene_version=2\nSprite texture=assets/textures/red-1x1.png;size=1\n",
        )
        .unwrap();
        fs::write(
            root.join("scripts/main.lua"),
            "local helper = require('helper')\nfunction update() return 'ok' end\n",
        )
        .unwrap();
        fs::write(root.join("scripts/helper.lua"), "return {}\n").unwrap();
        let out = std::env::temp_dir().join(format!(
            "rustlua-assetc-dependency-index-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let _ = fs::remove_dir_all(&out);
        package(&root, &out, false, false, false).unwrap();
        let index: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("dependencies.json")).unwrap()).unwrap();
        assert_eq!(index["format"], 1);
        assert!(
            index["edges"]["scenes/main.scene"]
                .as_array()
                .unwrap()
                .iter()
                .any(|path| path == "assets/textures/red-1x1.png")
        );
        assert!(
            index["edges"]["scripts/main.lua"]
                .as_array()
                .unwrap()
                .iter()
                .any(|path| path == "scripts/helper.lua")
        );
        assert!(index["opaque"].as_array().unwrap().is_empty());
        fs::remove_dir_all(&out).ok();

        fs::write(
            root.join("scripts/main.lua"),
            "local next_scene = state.scene\nscene.load(next_scene)\n",
        )
        .unwrap();
        package(&root, &out, false, false, false).unwrap();
        let index: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("dependencies.json")).unwrap()).unwrap();
        assert!(
            index["opaque"]
                .as_array()
                .unwrap()
                .iter()
                .any(|path| path == "scripts/main.lua")
        );
        fs::remove_dir_all(&out).ok();

        fs::write(
            root.join("scripts/main.lua"),
            "function update() audio.play('ui_click') end\n",
        )
        .unwrap();
        package(&root, &out, false, false, false).unwrap();
        let index: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("dependencies.json")).unwrap()).unwrap();
        // `audio.*` 不是同步 vfs 读取（运行期由宿主按名字取回），因此既不产边也不标 opaque：
        // 早先把它当"必须证明完整"的同步 API，代价是每个放过音效的脚本都拖垮全量预载。
        assert!(
            !index["opaque"]
                .as_array()
                .unwrap()
                .iter()
                .any(|path| path == "scripts/main.lua")
        );
        assert!(index["edges"]["scripts/main.lua"].is_null());
        fs::remove_dir_all(&out).ok();

        // 查表路由 + 兜底字面量：兜底那一支是可证的真依赖，不该把文件标成 opaque。
        fs::write(
            root.join("scripts/main.lua"),
            "function on_click()\n  local target = maps.get(name)\n  return scene.load(target and target or \"main\")\nend\n",
        )
        .unwrap();
        package(&root, &out, false, false, false).unwrap();
        let index: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("dependencies.json")).unwrap()).unwrap();
        assert!(
            index["edges"]["scripts/main.lua"]
                .as_array()
                .unwrap()
                .iter()
                .any(|path| path == "scenes/main.scene"),
            "兜底字面量应当成为真实场景边：{index}"
        );
        assert!(
            !index["opaque"]
                .as_array()
                .unwrap()
                .iter()
                .any(|path| path == "scripts/main.lua")
        );
        fs::remove_dir_all(&out).ok();

        // 同文件常量实参：`prefab.spawn(PRESET_PATH)` 的路径在上一行是字面量，属可证依赖。
        fs::create_dir_all(root.join("assets/prefabs")).unwrap();
        fs::write(
            root.join("assets/prefabs/dialogue.prefab"),
            "prefab_version=1\n",
        )
        .unwrap();
        fs::write(
            root.join("scripts/main.lua"),
            "local PRESET_PATH = \"assets/prefabs/dialogue.prefab\"\nfunction boot()\n  return prefab.spawn(PRESET_PATH)\nend\n",
        )
        .unwrap();
        package(&root, &out, false, false, false).unwrap();
        let index: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("dependencies.json")).unwrap()).unwrap();
        assert!(
            index["edges"]["scripts/main.lua"]
                .as_array()
                .unwrap()
                .iter()
                .any(|path| path == "assets/prefabs/dialogue.prefab"),
            "常量实参应当解析成真实预设边：{index}"
        );
        assert!(
            !index["opaque"]
                .as_array()
                .unwrap()
                .iter()
                .any(|path| path == "scripts/main.lua")
        );
        fs::remove_dir_all(&out).ok();

        // 已退役的目录常量 + 变量拼路径：包内没有命中 ⇒ 运行期同样读不到，
        // 不该把整包拖成全量预载（真实工程里那是 M71 删掉的 `assets/ui/prefabs/*.ui.json`）。
        fs::write(
            root.join("scripts/main.lua"),
            "local DIR = \"assets/ui/prefabs/\"\nfunction load(name)\n  return data.load(DIR .. name .. \".ui.json\")\nend\n",
        )
        .unwrap();
        package(&root, &out, false, false, false).unwrap();
        let index: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("dependencies.json")).unwrap()).unwrap();
        assert!(
            !index["opaque"]
                .as_array()
                .unwrap()
                .iter()
                .any(|path| path == "scripts/main.lua")
        );
        fs::remove_dir_all(&out).ok();

        // 拼出来的路径**确实**指向包内文件：前缀展开成完整命中集，同样是可证边。
        fs::create_dir_all(root.join("assets/ui/prefabs")).unwrap();
        fs::write(root.join("assets/ui/prefabs/dialogue.ui.json"), "{}\n").unwrap();
        fs::write(root.join("assets/ui/prefabs/hud.ui.json"), "{}\n").unwrap();
        package(&root, &out, false, false, false).unwrap();
        let index: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("dependencies.json")).unwrap()).unwrap();
        let edges = index["edges"]["scripts/main.lua"].as_array().unwrap();
        for expected in [
            "assets/ui/prefabs/dialogue.ui.json",
            "assets/ui/prefabs/hud.ui.json",
        ] {
            assert!(
                edges.iter().any(|path| path == expected),
                "目录前缀应当展开成 {expected}：{index}"
            );
        }
        assert!(
            !index["opaque"]
                .as_array()
                .unwrap()
                .iter()
                .any(|path| path == "scripts/main.lua")
        );
        fs::remove_dir_all(&out).ok();
        cleanup(&root);
    }

    #[test]
    fn package_dependency_index_ignores_bare_assets_prefix_literal() {
        let root = temp_project("package-bare-assets-prefix");
        // 只有"一级子目录"的目录引用才算可证闭包；裸 `"assets/"` 是**校验字面量**
        // （`string.find(path, "assets/", 1, true)`），展开它把整包挂成这个文件的依赖。
        assert_eq!(
            asset_prefix_refs("local ok = string.find(p, \"assets/\", 1, true)\n"),
            Vec::<String>::new()
        );
        assert_eq!(
            asset_prefix_refs("local dir = \"assets/art/tilesets/\"\n"),
            vec!["assets/art/tilesets/".to_owned()]
        );
        fs::create_dir_all(root.join("assets/art/tilesets")).unwrap();
        fs::write(root.join("assets/art/tilesets/ground.png"), b"png").unwrap();
        fs::write(root.join("assets/fonts-unused.ttf"), b"ttf").unwrap();
        fs::write(
            root.join("scripts/main.lua"),
            "local PATTERN = \"assets/\"\n\
             function update()\n\
             \x20 return string.find(PATTERN, \"assets/\", 1, true) ~= nil\n\
             end\n",
        )
        .unwrap();
        let out = std::env::temp_dir().join(format!(
            "rustlua-assetc-bare-prefix-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let _ = fs::remove_dir_all(&out);
        package(&root, &out, false, false, false).unwrap();
        let index: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("dependencies.json")).unwrap()).unwrap();
        assert!(
            index["edges"]["scripts/main.lua"].is_null(),
            "裸 `assets/` 不该产生任何边（否则整包进闭包）：{index}"
        );
        fs::remove_dir_all(&out).ok();
        cleanup(&root);
    }

    #[test]
    fn package_compresses_supported_assets_without_compressing_lua() {
        let root = temp_project("package-compress-assets");
        let out = std::env::temp_dir().join(format!(
            "rustlua-assetc-compress-out-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let _ = fs::remove_dir_all(&out);
        package(&root, &out, true, false, false).unwrap();
        let png = fs::read(out.join("assets/textures/red-1x1.png")).unwrap();
        let lua = fs::read(out.join("scripts/main.lua")).unwrap();
        assert!(png.starts_with(rustlua_assets::pack::PACK_MAGIC));
        assert!(!lua.starts_with(rustlua_assets::pack::PACK_MAGIC));
        fs::remove_dir_all(&out).ok();
        cleanup(&root);
    }

    /// 压缩白名单的口径：资产按格式压缩，代码（Lua/场景）走加密通道而非这里。
    /// 新增音频格式时必须同时改 `ResourceKind::from_path` 与这里，否则新格式会
    /// 以原始字节进包——体积问题只在发布期才暴露。
    #[test]
    fn compressible_assets_cover_every_packaged_asset_format() {
        for name in [
            "a.png", "a.jpg", "a.jpeg", "a.wav", "a.ogg", "a.mp3", "a.ttf", "a.otf", "a.glb",
            "A.MP3",
        ] {
            assert!(is_compressible_asset(Path::new(name)), "{name} 应可压缩");
        }
        for name in ["a.lua", "a.scene", "a.json", "a.txt"] {
            assert!(!is_compressible_asset(Path::new(name)), "{name} 不应进压缩");
        }
    }

    /// 打包输出目录不得与项目目录相同、也不得位于 `scenes/`/`scripts/`/`assets/`
    /// 之内：那几个目录正是打包输入，产物写进去会自我复制，下次打包还会把产物当输入。
    #[test]
    fn package_rejects_output_inside_data_dirs_or_equal_to_project() {
        let root = temp_project("package-outguard");
        let error = package(&root, &root, false, false, false).expect_err("输出=项目 必须被拒绝");
        assert!(error.to_string().contains("相同"), "{error}");

        for sub in ["assets", "scripts", "scenes"] {
            let nested = root.join(sub).join("dist");
            let error = package(&root, &nested, false, false, false)
                .expect_err("输出位于项目数据目录内部 必须被拒绝");
            assert!(error.to_string().contains("项目数据目录"), "{error}");
            assert!(!nested.exists(), "被拒绝的打包不应留下输出目录");
        }

        // 带 `..` 的等价写法也要被识别（词法比较即可，不需要目录存在）。
        let dotted = root.join("assets").join("dist").join("..").join("dist");
        let error =
            package(&root, &dotted, false, false, false).expect_err("`..` 绕不过数据目录检查");
        assert!(error.to_string().contains("项目数据目录"), "{error}");
        cleanup(&root);
    }

    /// 发布脚本的真实布局（`<project>/builds/<platform>/.staging-*/pack`）
    /// 必须**允许**：`scan`/`collect_data_files` 只遍历 `project.toml` 与
    /// `scenes/`/`scripts/`/`assets/`，因此 `builds/` 下的产物不会被当成项目数据。
    /// 回归背景：更严格的「输出不能位于项目内部」规则曾让 `package.ps1 -SelfTest`
    /// 直接失败（`assetc error: 输出目录不能位于项目目录内部`）。
    #[test]
    fn package_allows_output_inside_project_builds_dir() {
        let root = temp_project("package-builds-out");
        let out = root
            .join("builds")
            .join("pc")
            .join("jianghu_youtian")
            .join(".staging-20260101-000000-abcdef")
            .join("pack");
        package(&root, &out, false, false, false).expect("builds/ 下的输出必须被允许");
        assert!(out.join("scripts/main.lua").is_file());
        assert!(out.join("manifest.txt").is_file());
        // 项目数据子目录没有被产物污染（打包输入仍是纯项目数据）。
        assert!(!root.join("assets").join("builds").exists());
        cleanup(&root);
    }

    /// `canonical_or_absolute` 在输出目录尚不存在时也要给出可比较的绝对路径。
    #[test]
    fn canonical_or_absolute_resolves_missing_paths() {
        let root = temp_project("canonical");
        let missing = root.join("does").join("not").join("exist");
        assert!(!missing.exists());
        let resolved = canonical_or_absolute(&missing).unwrap();
        assert!(resolved.is_absolute(), "{}", resolved.display());
        assert!(resolved.ends_with(Path::new("does/not/exist")));
        // 已存在的路径解析为规范路径（Windows 上会去掉 8.3 短名/大小写差异）。
        let existing = canonical_or_absolute(&root).unwrap();
        assert_eq!(existing, fs::canonicalize(&root).unwrap());
        cleanup(&root);
    }

    /// 路径越界判定（纯函数）：符号链接安全校验的核心。
    #[test]
    fn path_containment_detects_escapes() {
        let root = Path::new("/project");
        assert!(!path_escapes_root(root, Path::new("/project/assets/a.png")));
        assert!(!path_escapes_root(root, Path::new("/project")));
        assert!(path_escapes_root(root, Path::new("/project2/assets/a.png")));
        assert!(path_escapes_root(root, Path::new("/etc/passwd")));
        assert!(path_escapes_root(
            root,
            Path::new("/project/../outside.png")
        ));
    }

    /// 指向项目外的符号链接必须在打包时报错，而不是被 `fs::copy` 跟随复制进发布包。
    /// Windows 上创建符号链接需要开发者模式/管理员权限，无权限时跳过。
    #[test]
    fn collect_files_rejects_symlink_escaping_the_project() {
        let root = temp_project("symlink-escape");
        let outside = std::env::temp_dir().join(format!(
            "rustlua-assetc-outside-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(&outside).unwrap();
        let secret = outside.join("secret.txt");
        fs::write(&secret, "outside the project").unwrap();

        let link = root.join("assets").join("linked-secret.txt");
        #[cfg(windows)]
        let created = std::os::windows::fs::symlink_file(&secret, &link).is_ok();
        #[cfg(not(windows))]
        let created = std::os::unix::fs::symlink(&secret, &link).is_ok();
        if !created {
            // 无符号链接权限：本用例在该环境下无法构造，跳过。
            fs::remove_dir_all(&outside).ok();
            cleanup(&root);
            return;
        }

        let mut files = Vec::new();
        let error =
            collect_files(&root.join("assets"), &mut files).expect_err("越界符号链接必须被拒绝");
        assert!(error.to_string().contains("项目目录外"), "{error}");
        // 同时确认整条打包路径也会失败（不会写出半个发布目录）。
        let out = std::env::temp_dir().join(format!(
            "rustlua-assetc-symlink-out-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let error = package(&root, &out, false, false, false).expect_err("打包必须拒绝越界链接");
        assert!(error.to_string().contains("项目目录外"), "{error}");

        fs::remove_file(&link).ok();
        fs::remove_dir_all(&out).ok();
        fs::remove_dir_all(&outside).ok();
        cleanup(&root);
    }

    /// 开发用文件（预览拼图 / 美术清单）不进打包清单，游戏资源照常进。
    #[test]
    fn dev_artifacts_are_not_packaged() {
        let root = temp_project("dev-artifacts");
        let art = root.join("assets").join("art").join("items");
        fs::create_dir_all(&art).unwrap();
        fs::write(art.join("lingzhi.png"), b"png").unwrap();
        fs::write(art.join("_preview.png"), b"png").unwrap();
        fs::write(art.join("preview.png"), b"png").unwrap();
        fs::write(art.join("manifest.json"), b"{}").unwrap();
        let title = root.join("assets").join("art").join("title");
        fs::create_dir_all(&title).unwrap();
        fs::write(title.join("season_preview.png"), b"png").unwrap();
        fs::write(title.join("title_bg_pixel.png"), b"png").unwrap();

        let mut files = Vec::new();
        collect_files(&root.join("assets"), &mut files).unwrap();
        let names: Vec<String> = files
            .iter()
            .map(|(path, _)| path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"lingzhi.png".to_owned()), "{names:?}");
        // `title_bg_pixel.png` 里含 "pixel" 不含 "preview"：必须留在包里（不能误伤）。
        assert!(
            names.contains(&"title_bg_pixel.png".to_owned()),
            "{names:?}"
        );
        assert!(!names.contains(&"_preview.png".to_owned()), "{names:?}");
        assert!(!names.contains(&"preview.png".to_owned()), "{names:?}");
        assert!(!names.contains(&"manifest.json".to_owned()), "{names:?}");
        assert!(
            !names.contains(&"season_preview.png".to_owned()),
            "{names:?}"
        );

        // 判定函数本身的边界。
        assert!(is_dev_artifact(Path::new("assets/art/items/_overview.png")));
        assert!(is_dev_artifact(Path::new("assets/art/ui/manifest.json")));
        assert!(!is_dev_artifact(Path::new("assets/art/items/wood.png")));
        assert!(!is_dev_artifact(Path::new(
            "assets/art/title/title_bg_pixel.png"
        )));
        cleanup(&root);
    }

    #[test]
    fn benchmark_compression_reports_asset_kinds() {
        let root = temp_project("benchmark-compression");
        let entries = benchmark_compression(&root).unwrap();
        let texture = entries
            .iter()
            .find(|entry| entry.kind == "texture")
            .unwrap();
        assert_eq!(texture.files, 1);
        assert!(texture.original_bytes > 0);
        assert!(texture.compressed_bytes > 0);
        cleanup(&root);
    }

    #[test]
    fn sample_project_import_checkdeps_and_package_pass() {
        let Some(root) = copy_sample_project("workflow") else {
            return;
        };
        let outcomes = import(&root).unwrap();
        assert!(
            outcomes.iter().all(|outcome| outcome.error.is_none()),
            "sample project import should succeed: {outcomes:?}"
        );

        let broken = check_dependencies(&root).unwrap();
        assert!(
            broken.is_empty(),
            "sample project should have no broken dependency chain: {broken:?}"
        );

        let out = std::env::temp_dir().join(format!(
            "rustlua-assetc-sample-pack-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let _ = fs::remove_dir_all(&out);
        let manifest = package(&root, &out, false, false, false).unwrap();
        assert!(
            manifest.entries.contains_key("scenes/api_gameplay.scene"),
            "packaged manifest should include production scene"
        );
        assert!(
            manifest
                .entries
                .contains_key("assets/prefabs/slime_green.prefab"),
            "packaged manifest should include prefab"
        );
        assert!(
            out.join("scripts/main.lua").is_file(),
            "packaged scripts should be copied"
        );
        fs::remove_dir_all(&out).ok();
        cleanup(&root);
    }

    #[test]
    fn sample_project_rename_breaks_dependencies_until_consumers_are_updated() {
        let Some(root) = copy_sample_project("rename-fix") else {
            return;
        };
        let old_path = root.join("assets/textures/slime.png");
        let new_rel = "assets/textures/slime-renamed.png";
        let new_path = root.join(new_rel);

        let outcomes = import(&root).unwrap();
        assert!(
            outcomes.iter().all(|outcome| outcome.error.is_none()),
            "sample project import should succeed before rename: {outcomes:?}"
        );

        fs::rename(&old_path, &new_path).unwrap();

        let broken = check_dependencies(&root).unwrap();
        assert!(
            !broken.is_empty(),
            "renaming a referenced texture should surface broken links"
        );
        assert!(
            broken.iter().all(|link| link.reason == "缺失"),
            "renamed-away texture should report missing root: {broken:?}"
        );
        let consumers: Vec<_> = broken.iter().map(|link| link.consumer.clone()).collect();
        assert!(
            consumers.contains(&PathBuf::from("assets/prefabs/slime_green.prefab")),
            "green prefab should become invalid after texture rename: {consumers:?}"
        );
        assert!(
            consumers.contains(&PathBuf::from("assets/prefabs/slime_red.prefab")),
            "red prefab should become invalid after texture rename: {consumers:?}"
        );
        assert!(
            consumers.contains(&PathBuf::from("scenes/api_gameplay.scene")),
            "gameplay scene should become invalid after texture rename: {consumers:?}"
        );

        // 修复：**逐个消费方**把旧路径改成新路径。消费方清单**从断链报告里取**，
        // 而不是手抄一份文件列表——示例工程每加一个引用者，手抄清单就会悄悄失效。
        for consumer in &consumers {
            replace_in_file(&root.join(consumer), "assets/textures/slime.png", new_rel);
        }

        let outcomes = import(&root).unwrap();
        assert!(
            outcomes.iter().all(|outcome| outcome.error.is_none()),
            "sample project import should succeed after fix: {outcomes:?}"
        );
        let broken = check_dependencies(&root).unwrap();
        assert!(
            broken.is_empty(),
            "updating all consumers should repair broken links: {broken:?}"
        );

        cleanup(&root);
    }

    // ---------------------------------------------------------------- 批 A：RLB 单包

    /// 建一个"有文本 + 有二进制 + 有依赖索引"的小工程，供打包门使用。
    fn bundle_project(label: &str) -> PathBuf {
        let root = temp_project(label);
        fs::create_dir_all(root.join("assets/ui")).unwrap();
        fs::write(
            root.join("scripts/data.lua"),
            "return { title = '测试标题', items = { 'a', 'b' } }\n".repeat(20),
        )
        .unwrap();
        fs::write(
            root.join("scripts/main.lua"),
            "local data = require('data')\nfunction update() return data.title end\n",
        )
        .unwrap();
        fs::write(
            root.join("assets/ui/panel.txt"),
            "nine_slice=1\nborder=4\n".repeat(10),
        )
        .unwrap();
        root
    }

    /// 启动闭包优先排序（`docs/67` §9 的性能项）：boot 期读的条目必须排在组内最前面。
    ///
    /// 这条只校验**排序性质**（纯函数、零依赖、秒级）；"实际少解几个块"的读数需要
    /// WebGL 产物 + 浏览器 harness（重，单独跑）。
    #[test]
    fn boot_closure_sorts_first() {
        let mut paths = vec![
            "assets/art/ui/hud/a.png".to_owned(),
            "scripts/data/items.lua".to_owned(),
            "scripts/main.lua".to_owned(),
            "scenes/main.scene".to_owned(),
            "project.toml".to_owned(),
            "atlas_redirect.json".to_owned(),
        ];
        paths.sort_by(|a, b| {
            boot_priority(a)
                .cmp(&boot_priority(b))
                .then_with(|| a.cmp(b))
        });
        let head: Vec<&str> = paths.iter().take(4).map(String::as_str).collect();
        assert_eq!(
            head,
            vec![
                "atlas_redirect.json",
                "project.toml",
                "scenes/main.scene",
                "scripts/main.lua"
            ],
            "启动闭包必须全部排在前面：{paths:?}"
        );
        // 0 档 = 四个锚点（字典序）；1 档 = 其余脚本；2 档 = 美术等
        assert_eq!(
            paths[4], "scripts/data/items.lua",
            "1 档是其余脚本：{paths:?}"
        );
        assert_eq!(paths[5], "assets/art/ui/hud/a.png", "2 档才是美术");
    }

    /// 单包产物 = 骨架 + 容器 + 报告 + 清单，且目录形态的数据目录**全部消失**。
    #[test]
    fn bundle_package_leaves_only_skeleton() {
        let root = bundle_project("bundle-skeleton");
        let out = std::env::temp_dir().join(format!(
            "rustlua-assetc-bundle-skeleton-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let _ = fs::remove_dir_all(&out);
        let report = package_bundled(&root, &out, false, false, false, &[]).unwrap();

        assert_eq!(report.mode, "single");
        assert!(report.entries >= 4, "条目数 {report:?}");
        assert!(report.blocks >= 1);
        assert!(report.ratio_milli < 1000, "必须比原始更小：{report:?}");
        assert!(report.bundle_path.starts_with("bundles/all-"));

        // 骨架在，数据目录不在。
        assert!(out.join("project.toml").is_file());
        assert!(out.join("manifest.txt").is_file());
        assert!(out.join(PACKAGE_REPORT_FILE).is_file());
        assert!(out.join(&report.bundle_path).is_file());
        for gone in ["scenes", "scripts", "assets"] {
            assert!(!out.join(gone).exists(), "{gone}/ 不该留在输出根");
        }

        // 清单只列骨架/容器/报告，且校验和与磁盘一致。
        let manifest = fs::read_to_string(out.join("manifest.txt")).unwrap();
        let lines: Vec<&str> = manifest.lines().collect();
        assert!(lines[0].starts_with("assetc_manifest=1\t"));
        assert_eq!(lines.len(), 4, "清单应当只有 3 个条目：{manifest}");
        for line in &lines[1..] {
            let mut parts = line.split('\t');
            let relative = parts.next().unwrap();
            let size: u64 = parts.next().unwrap().parse().unwrap();
            let checksum = u64::from_str_radix(parts.next().unwrap(), 16).unwrap();
            let bytes = fs::read(out.join(relative)).unwrap();
            assert_eq!(bytes.len() as u64, size, "{relative}");
            assert_eq!(fnv1a(&bytes), checksum, "{relative}");
        }

        fs::remove_dir_all(&out).ok();
        cleanup(&root);
    }

    /// 等价性门（`docs/67` §10.4）：目录形态与单包形态**逐条目字节相等**。
    ///
    /// 这条把"容器不改变内容"变成机器可证，比任何体积报告都重要。
    #[test]
    fn bundle_package_matches_loose_output() {
        use rustlua_infra::bundle::{BundleIndex, BundleReader, MemSource};

        let root = bundle_project("bundle-equivalence");
        let loose = std::env::temp_dir().join(format!(
            "rustlua-assetc-bundle-loose-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let packed = std::env::temp_dir().join(format!(
            "rustlua-assetc-bundle-packed-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let _ = fs::remove_dir_all(&loose);
        let _ = fs::remove_dir_all(&packed);

        package(&root, &loose, false, false, false).unwrap();
        let report = package_bundled(&root, &packed, false, false, false, &[]).unwrap();

        let bytes = fs::read(packed.join(&report.bundle_path)).unwrap();
        let index = BundleIndex::parse(&bytes).unwrap();
        let mut reader = BundleReader::new(index, MemSource::new(bytes));

        // 目录形态里除清单外的每个文件，都必须在容器里有一份字节相同的条目。
        let mut files = Vec::new();
        collect_files(&loose, &mut files).unwrap();
        let mut compared = 0;
        for (path, _) in files {
            let relative = path
                .strip_prefix(&loose)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if relative == "manifest.txt" {
                continue; // 两种形态的清单本来就不同（bundle 级 vs 逐文件）
            }
            let expected = fs::read(&path).unwrap();
            let actual = reader
                .read(&relative)
                .unwrap()
                .unwrap_or_else(|| panic!("容器缺少条目 {relative}"));
            assert_eq!(actual, expected, "条目 {relative} 字节不一致");
            compared += 1;
        }
        assert_eq!(compared as u64, report.entries, "条目数必须守恒");
        // 反向：容器里没有多余条目（index.paths ⊆ 目录形态的文件集）。
        for entry in reader.index().paths() {
            assert!(
                loose.join(entry).exists(),
                "容器里多出目录形态没有的条目 {entry}"
            );
        }
        reader.verify_all().unwrap();

        fs::remove_dir_all(&loose).ok();
        fs::remove_dir_all(&packed).ok();
        cleanup(&root);
    }

    /// 确定性门（`docs/67` §4.5）：同一输入连打两次，容器逐字节相同。
    #[test]
    fn bundle_package_is_deterministic() {
        let root = bundle_project("bundle-determinism");
        let mut previous: Option<Vec<u8>> = None;
        for round in 0..2 {
            let out = std::env::temp_dir().join(format!(
                "rustlua-assetc-bundle-det-{round}-{}-{}",
                std::process::id(),
                unique_suffix()
            ));
            let _ = fs::remove_dir_all(&out);
            let report = package_bundled(&root, &out, false, false, false, &[]).unwrap();
            let bytes = fs::read(out.join(&report.bundle_path)).unwrap();
            if let Some(before) = &previous {
                assert_eq!(before, &bytes, "两次打包的容器字节必须相同");
            }
            previous = Some(bytes);
            fs::remove_dir_all(&out).ok();
        }
        cleanup(&root);
    }

    /// 报告里的数字必须与容器本身对得上（否则体积报告就是装饰）。
    #[test]
    fn bundle_report_matches_container() {
        use rustlua_infra::bundle::BundleIndex;

        let root = bundle_project("bundle-report");
        let out = std::env::temp_dir().join(format!(
            "rustlua-assetc-bundle-report-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let _ = fs::remove_dir_all(&out);
        let report = package_bundled(&root, &out, false, false, false, &[]).unwrap();
        let bytes = fs::read(out.join(&report.bundle_path)).unwrap();
        let index = BundleIndex::parse(&bytes).unwrap();

        assert_eq!(report.entries, index.len() as u64);
        assert_eq!(report.blocks, index.blocks().len() as u64);
        assert_eq!(
            report.bundle_bytes,
            bytes.len() as u64,
            "报告的落盘字节必须等于容器文件大小"
        );
        // 容器名里的 8 位 hex 取自整包 FNV：名字换了内容一定换了（CDN/微信缓存失效的依据）。
        let expected_name = format!(
            "{BUNDLE_DIR}/all-{:08x}.rlb",
            rustlua_infra::bundle::checksum(&bytes) as u32
        );
        assert_eq!(report.bundle_path, expected_name);

        // 报告自身可被读回（发布面板/CI 依赖这份 JSON）。
        let text = fs::read_to_string(out.join(PACKAGE_REPORT_FILE)).unwrap();
        let parsed: PackageReport = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, report);

        fs::remove_dir_all(&out).ok();
        cleanup(&root);
    }
}
