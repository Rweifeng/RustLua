//! assetc — 资源构建工具（M7-01）。
//!
//! 扫描项目数据文件（`project.toml`、`scenes/`、`scripts/`、`assets/`），
//! 注册到 [`ResourceDatabase`]，并通过 [`ImportCache`] 增量导入缓存，
//! 最后可打包为发布资源目录并生成清单。
//!
//! 命令（见 `main.rs`）：`scan` / `import` / `clean` / `list` / `package` / `--check`。

use engine::core::{
    dependency::recompute_edges,
    import_cache::ImportCache,
    importer::{ImporterRegistry, run_import_batch},
    resource_db::{AssetId, DiskAsset, ResourceDatabase, ResourceState},
    resource_kind::ResourceKind,
};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Duration,
};

/// assetc 打包清单格式版本；变更会触发发布脚本重新打包。
pub const PACKAGE_VERSION: &str = "assetc-package-1";

/// 扫描时跳过的本地工作目录。
const SKIP_DIRS: &[&str] = &[".cache", ".temp", ".git", "target"];

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
pub fn ensure_project(project: &Path) -> Result<(), String> {
    let manifest = project.join("project.toml");
    if !manifest.is_file() {
        return Err(format!(
            "不是有效的游戏项目（缺少 {}）：{}",
            manifest.display(),
            project.display()
        ));
    }
    Ok(())
}

/// 递归收集目录下所有文件（含 mtime），跳过本地工作目录。
pub fn collect_files(root: &Path, output: &mut Vec<(PathBuf, u64)>) -> Result<(), String> {
    let mut children = fs::read_dir(root)
        .map_err(|error| format!("读取目录失败 {}: {error}", root.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("读取目录失败 {}: {error}", root.display()))?;
    children.sort_by_key(|entry| entry.path());
    for child in children {
        let path = child.path();
        if child.file_type().map_err(|e| e.to_string())?.is_dir() {
            if SKIP_DIRS.contains(&path.file_name().and_then(|n| n.to_str()).unwrap_or("")) {
                continue;
            }
            collect_files(&path, output)?;
        } else {
            let modified = modified_nanos(&path)?;
            output.push((path, modified));
        }
    }
    Ok(())
}

fn modified_nanos(path: &Path) -> Result<u64, String> {
    let modified = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| format!("读取文件时间失败 {}: {error}", path.display()))?;
    Ok(modified
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0))
}

/// 收集项目的数据文件（清单 + 场景 + 脚本 + 资源），含 mtime。
pub fn collect_data_files(project: &Path) -> Result<Vec<(PathBuf, u64)>, String> {
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
pub fn scan(project: &Path) -> Result<Vec<ScanEntry>, String> {
    ensure_project(project)?;
    let scanned_files = collect_data_files(project)?;
    let mut database = ResourceDatabase::open(project).map_err(|error| error.to_string())?;
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
        let importer_version = ImporterRegistry::builtin()
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
    database.save().map_err(|error| error.to_string())?;
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
pub fn import(project: &Path) -> Result<Vec<ImportOutcome>, String> {
    let entries = scan(project)?;
    let mut database = ResourceDatabase::open(project).map_err(|error| error.to_string())?;
    let mut cache = ImportCache::open(project).map_err(|error| error.to_string())?;
    let registry = ImporterRegistry::builtin();
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
        registry,
        &items,
        &cancel,
    );
    // 导入后依赖可能已就绪：重新传播失效，让消费者从 invalid 撤销回可消费状态。
    database.propagate_invalidation();
    database.save().map_err(|error| error.to_string())?;
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
pub fn check_dependencies(project: &Path) -> Result<Vec<BrokenLink>, String> {
    ensure_project(project)?;
    // 注册 + reconcile + 依赖重算 + 失效传播，让数据库反映最新状态。
    scan(project)?;
    let database = ResourceDatabase::open(project).map_err(|error| error.to_string())?;
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
pub fn clean(project: &Path) -> Result<(), String> {
    let mut cache = ImportCache::open(project).map_err(|error| error.to_string())?;
    cache.clear().map_err(|error| error.to_string())
}

/// 列出已注册资源。
pub fn list(project: &Path) -> Result<Vec<ScanEntry>, String> {
    scan(project)
}

fn copy_file(source: &Path, destination: &Path) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::copy(source, destination).map_err(|error| {
        format!(
            "复制失败 {} → {}: {error}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
fn copy_dir(source: &Path, destination: &Path) -> Result<(), String> {
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
) -> Result<(), String> {
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
            let bytes = fs::read(&file).map_err(|e| e.to_string())?;
            let encoded = engine::core::pack::encode(&bytes, compress_file, encrypt_file);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::write(&dest, &encoded).map_err(|e| e.to_string())?;
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
    matches!(
        extension(path).as_deref(),
        Some("png")
            | Some("jpg")
            | Some("jpeg")
            | Some("wav")
            | Some("ogg")
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
pub fn benchmark_compression(project: &Path) -> Result<Vec<CompressionBenchmarkEntry>, String> {
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
            fs::read(&file).map_err(|error| format!("读取失败 {}: {error}", file.display()))?;
        let kind = ResourceKind::from_path(&file).token().to_owned();
        let started = std::time::Instant::now();
        let compressed = engine::core::pack::encode(&bytes, true, false);
        let compress_time = started.elapsed();
        let started = std::time::Instant::now();
        let decompressed = engine::core::pack::decode(&compressed)?;
        let decompress_time = started.elapsed();
        if decompressed != bytes {
            return Err(format!("压缩往返校验失败: {}", file.display()));
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

/// 生成发布资源包：复制 `project.toml`、`scenes/`、`scripts/`、`assets/`
/// 到 `out`，并写入 `manifest.txt`（路径、大小、校验和）。
/// `compress` 对 `assets/` 下支持压缩的资产生效；`encrypt` 仅对 Lua 与场景文件生效。
pub fn package(
    project: &Path,
    out: &Path,
    compress: bool,
    encrypt: bool,
) -> Result<PackageManifest, String> {
    ensure_project(project)?;
    if out == project {
        return Err("输出目录不能与项目目录相同".to_owned());
    }
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
    let chars = engine::core::char_collect::collect_project_chars(project);
    fs::write(out.join("chars.txt"), &chars).map_err(|e| e.to_string())?;
    // 发布子集化：把主字体裁剪到用到的字符，缩小体积（原 `subset-font.py` 的 Rust 版）。
    // 子集化失败（如 CFF 字体）时保留完整字体。
    if let Some(font_path) = project_font_key(project) {
        let src = project.join(&font_path);
        let dst = out.join(&font_path);
        if src.is_file()
            && dst.is_file()
            && let Ok(bytes) = fs::read(&src)
            && let Ok(subset) = engine::core::font_subset::subset_ttf(&bytes, &chars)
        {
            fs::write(&dst, subset).map_err(|e| e.to_string())?;
        }
    }

    let mut files = Vec::new();
    collect_files(out, &mut files)?;
    files.retain(|(path, _)| path != &out.join("manifest.txt"));
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut manifest = BTreeMap::new();
    let mut lines = vec![format!("assetc_manifest=1\t{PACKAGE_VERSION}")];
    for (path, _) in files {
        let relative = path
            .strip_prefix(out)
            .expect("file must be inside output dir")
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = fs::read(&path).map_err(|e| e.to_string())?;
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
    fs::write(out.join("manifest.txt"), lines.join("\n")).map_err(|e| e.to_string())?;
    Ok(PackageManifest { entries: manifest })
}

/// 冒烟检查：创建临时项目，验证 scan/import 闭环可用。
pub fn check() -> Result<String, String> {
    let root = std::env::temp_dir().join(format!("p2-assetc-check-{}", std::process::id()));
    fs::create_dir_all(root.join("scripts")).map_err(|e| e.to_string())?;
    fs::write(
        root.join("project.toml"),
        "[project]\nname = \"check\"\nversion = \"0.0.0\"\nengine = \"RustLua\"\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"\"\n",
    )
    .map_err(|e| e.to_string())?;
    fs::write(
        root.join("scripts/main.lua"),
        "-- check\nfunction update() return 'ok' end\n",
    )
    .map_err(|e| e.to_string())?;

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
        let root = std::env::temp_dir().join(format!("p2-assetc-{name}-{}", std::process::id()));
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
        engine::core::games_dir().join("examples")
    }

    fn copy_sample_project(label: &str) -> Option<PathBuf> {
        let source = sample_project_root();
        if !source.join("project.toml").is_file() {
            return None;
        }
        let root = std::env::temp_dir().join(format!(
            "p2-assetc-sample-{label}-{}-{}",
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
        let out = std::env::temp_dir().join(format!("p2-assetc-out-{}", std::process::id()));
        let _ = fs::remove_dir_all(&out);
        package(&root, &out, false, false).unwrap();
        assert!(out.join("project.toml").is_file());
        assert!(out.join("scripts/main.lua").is_file());
        assert!(out.join("assets/textures/red-1x1.png").is_file());
        let manifest_text = fs::read_to_string(out.join("manifest.txt")).unwrap();
        assert!(manifest_text.contains("red-1x1.png"), "manifest lists png");
        assert!(manifest_text.contains("main.lua"), "manifest lists lua");
        fs::remove_dir_all(&out).ok();
        cleanup(&root);
    }

    #[test]
    fn package_compresses_supported_assets_without_compressing_lua() {
        let root = temp_project("package-compress-assets");
        let out = std::env::temp_dir().join(format!(
            "p2-assetc-compress-out-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let _ = fs::remove_dir_all(&out);
        package(&root, &out, true, false).unwrap();
        let png = fs::read(out.join("assets/textures/red-1x1.png")).unwrap();
        let lua = fs::read(out.join("scripts/main.lua")).unwrap();
        assert!(png.starts_with(engine::core::pack::PACK_MAGIC));
        assert!(!lua.starts_with(engine::core::pack::PACK_MAGIC));
        fs::remove_dir_all(&out).ok();
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
            "p2-assetc-sample-pack-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let _ = fs::remove_dir_all(&out);
        let manifest = package(&root, &out, false, false).unwrap();
        assert!(
            manifest.entries.contains_key("scenes/production.scene"),
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
            consumers.contains(&PathBuf::from("scenes/production.scene")),
            "production scene should become invalid after texture rename: {consumers:?}"
        );

        replace_in_file(
            &root.join("assets/prefabs/slime_green.prefab"),
            "assets/textures/slime.png",
            new_rel,
        );
        replace_in_file(
            &root.join("assets/prefabs/slime_red.prefab"),
            "assets/textures/slime.png",
            new_rel,
        );
        replace_in_file(
            &root.join("scenes/production.scene"),
            "assets/textures/slime.png",
            new_rel,
        );
        replace_in_file(
            &root.join("scenes/main.scene"),
            "assets/textures/slime.png",
            new_rel,
        );
        replace_in_file(
            &root.join("scenes/ui_test.scene"),
            "assets/textures/slime.png",
            new_rel,
        );
        replace_in_file(
            &root.join("scripts/demo_ui_dynamic.lua"),
            "assets/textures/slime.png",
            new_rel,
        );

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
