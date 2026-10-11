//! assetc — 资源构建工具（M7-01）。
//!
//! 扫描项目数据文件（`project.toml`、`scenes/`、`scripts/`、`assets/`），
//! 注册到 [`ResourceDatabase`]，并通过 [`ImportCache`] 增量导入缓存，
//! 最后可打包为发布资源目录并生成清单。
//!
//! 命令（见 `main.rs`）：`scan` / `import` / `clean` / `list` / `package` / `--check`。

use anyhow::anyhow;
use rustlua_assets::dependency::{DependencyRef, extract_refs, recompute_edges};
use rustlua_assets::import_cache::ImportCache;
use rustlua_assets::importer::run_import_batch;
use rustlua_assets::resource_db::{AssetId, DiskAsset, ResourceDatabase, ResourceState};
use rustlua_assets::resource_kind::ResourceKind;
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Duration,
};

/// assetc 打包清单格式版本；变更会触发发布脚本重新打包。
pub const PACKAGE_VERSION: &str = "assetc-package-2";

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
/// 打包结果由 `scripts/verify.ps1` 的发布自检打印计数（`assetc_package_ok files=<n>`），
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
    let registry = rustlua_scene::asset_import::full_registry();
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
    let registry = rustlua_scene::asset_import::full_registry();
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
/// 项目数据；`scripts/package.ps1` 的原子发布正是这样布局的
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

/// 生成发布资源包：复制 `project.toml`、`scenes/`、`scripts/`、`assets/`
/// 到 `out`，并写入 `manifest.txt`（路径、大小、校验和）。
/// `compress` 对 `assets/` 下支持压缩的资产生效；`encrypt` 仅对 Lua 与场景文件生效。
pub fn package(
    project: &Path,
    out: &Path,
    compress: bool,
    encrypt: bool,
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
    // 首包字体：收集项目用到的字符清单，运行时预生成字形（首帧文字不卡）。
    let chars = rustlua_scene::char_collect::collect_project_chars(project);
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
        // These APIs consume runtime paths synchronously. Their path values are
        // often assembled through Lua variables, which this string scanner
        // cannot prove complete; use the safe full-package fallback for them.
        if kind == ResourceKind::LuaScript
            && [
                "data.load",
                "data.reload",
                "asset.load",
                "prefab.spawn",
                "audio.",
            ]
            .iter()
            .any(|call| text.contains(call))
        {
            opaque.insert(source.clone());
        }
        let mut targets = std::collections::BTreeSet::new();
        for reference in extract_refs(&text, kind) {
            let candidate = match reference {
                DependencyRef::Asset(path) => path.to_string_lossy().replace('\\', "/"),
                DependencyRef::LuaModule(name) => format!("scripts/{name}.lua"),
                DependencyRef::SceneName(name) => format!("scenes/{name}.scene"),
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
            } else {
                opaque.insert(source.clone());
            }
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
        package(&root, &out, false, false).unwrap();
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
        package(&root, &out, false, false).unwrap();
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
        package(&root, &out, false, false).unwrap();
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
        package(&root, &out, false, false).unwrap();
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
        package(&root, &out, true, false).unwrap();
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
        let error = package(&root, &root, false, false).expect_err("输出=项目 必须被拒绝");
        assert!(error.to_string().contains("相同"), "{error}");

        for sub in ["assets", "scripts", "scenes"] {
            let nested = root.join(sub).join("dist");
            let error = package(&root, &nested, false, false)
                .expect_err("输出位于项目数据目录内部 必须被拒绝");
            assert!(error.to_string().contains("项目数据目录"), "{error}");
            assert!(!nested.exists(), "被拒绝的打包不应留下输出目录");
        }

        // 带 `..` 的等价写法也要被识别（词法比较即可，不需要目录存在）。
        let dotted = root.join("assets").join("dist").join("..").join("dist");
        let error = package(&root, &dotted, false, false).expect_err("`..` 绕不过数据目录检查");
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
        package(&root, &out, false, false).expect("builds/ 下的输出必须被允许");
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
        let error = package(&root, &out, false, false).expect_err("打包必须拒绝越界链接");
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
        let manifest = package(&root, &out, false, false).unwrap();
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
}
