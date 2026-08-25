//! assetc — 资源构建工具（M7-01）。
//!
//! 扫描项目数据文件（`project.toml`、`scenes/`、`scripts/`、`assets/`），
//! 注册到 [`ResourceDatabase`]，并通过 [`ImportCache`] 增量导入缓存，
//! 最后可打包为发布资源目录并生成清单。
//!
//! 命令（见 `main.rs`）：`scan` / `import` / `clean` / `list` / `package` / `--check`。

use engine::core::{
    import_cache::ImportCache,
    resource_db::{DiskAsset, ResourceDatabase, ResourceState},
    resource_kind::ResourceKind,
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// assetc 的导入器版本；变更会触发全部资源重新导入。
pub const IMPORTER_VERSION: &str = "assetc-m8-06";

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
        match database.register(&relative, kind, IMPORTER_VERSION) {
            Ok(id) => {
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
    database.save().map_err(|error| error.to_string())?;
    Ok(entries)
}

/// 增量导入所有已注册资源。
pub fn import(project: &Path) -> Result<Vec<ImportOutcome>, String> {
    let entries = scan(project)?;
    let mut cache = ImportCache::open(project).map_err(|error| error.to_string())?;
    let mut outcomes = Vec::new();
    for entry in entries {
        let Some(id) = entry.asset_id else {
            continue;
        };
        let id: engine::core::resource_db::AssetId = id
            .parse()
            .map_err(|error| format!("解析资源 ID 失败 {}: {error}", entry.path.display()))?;
        let source = project.join(&entry.path);
        match cache.import_file(id, &source, IMPORTER_VERSION) {
            Ok(report) => outcomes.push(ImportOutcome {
                path: entry.path,
                reused: report.reused,
                cache_path: Some(report.cache_path),
                error: None,
            }),
            Err(error) => outcomes.push(ImportOutcome {
                path: entry.path,
                reused: false,
                cache_path: None,
                error: Some(error.to_string()),
            }),
        }
    }
    if let Some(failure) = outcomes.iter().find_map(|outcome| outcome.error.as_deref()) {
        return Err(format!("资源导入失败: {failure}"));
    }
    Ok(outcomes)
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

/// 复制目录；`lua`/`scene` 文件在需要时压缩+加密（运行时自动解码）。
fn copy_dir_encoded(
    source: &Path,
    destination: &Path,
    compress: bool,
    encrypt: bool,
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
        let is_text = matches!(
            file.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .as_deref(),
            Some("lua") | Some("scene")
        );
        if is_text && (compress || encrypt) {
            let bytes = fs::read(&file).map_err(|e| e.to_string())?;
            let encoded = engine::core::pack::encode(&bytes, compress, encrypt);
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

/// 生成发布资源包：复制 `project.toml`、`scenes/`、`scripts/`、`assets/`
/// 到 `out`，并写入 `manifest.txt`（路径、大小、校验和）。
/// `compress`/`encrypt` 对 Lua 与场景文件生效（运行时自动解码）。
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
        compress,
        encrypt,
    )?;
    copy_dir_encoded(
        &project.join("scripts"),
        &out.join("scripts"),
        compress,
        encrypt,
    )?;
    copy_dir(&project.join("assets"), &out.join("assets"))?;

    let mut files = Vec::new();
    collect_files(out, &mut files)?;
    files.retain(|(path, _)| path != &out.join("manifest.txt"));
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut manifest = BTreeMap::new();
    let mut lines = vec![format!("assetc_manifest=1\t{IMPORTER_VERSION}")];
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
        "[project]\nname = \"check\"\nversion = \"0.0.0\"\nengine = \"p2\"\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"\"\n",
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

    fn temp_project(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("p2-assetc-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("assets/textures")).unwrap();
        fs::create_dir_all(root.join("scenes")).unwrap();
        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::write(
            root.join("project.toml"),
            "[project]\nname = \"t\"\nversion = \"0.1.0\"\nengine = \"p2\"\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n",
        )
        .unwrap();
        fs::write(
            root.join("scripts/main.lua"),
            "-- ok\nfunction update() return 'ok' end\n",
        )
        .unwrap();
        fs::write(
            root.join("scenes/main.scene"),
            "scene_version=1\nname=main\n",
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

    fn cleanup(root: &Path) {
        fs::remove_dir_all(root).ok();
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
    fn import_rejects_invalid_gltf_with_non_success_result() {
        let root = temp_project("invalid-gltf");
        fs::write(root.join("assets/bad.glb"), b"invalid glb").unwrap();
        let error = import(&root).unwrap_err();
        assert!(error.contains("资源导入失败") && error.contains("bad.glb"));
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
}
