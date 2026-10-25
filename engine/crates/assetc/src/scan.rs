//! 项目扫描与增量导入：遍历数据目录、注册资源、跑导入批次。
//!
//! 从 `lib.rs` 纯搬运而来（M77-05 §2），行为未变。
use anyhow::anyhow;
use rustlua_assets::dependency::recompute_edges;
use rustlua_assets::resource_db::{AssetId, DiskAsset, ResourceDatabase, ResourceState};
use rustlua_assets::resource_kind::ResourceKind;
use rustlua_assets_import::import_cache::ImportCache;
use rustlua_assets_import::run_import_batch;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

/// 读取 project.toml 的 `font` 键（相对项目根的主字体路径）。
use crate::*;

pub(crate) fn project_font_key(project: &Path) -> Option<String> {
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
pub(crate) fn project_font_declarations(project: &Path) -> Vec<String> {
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
pub(crate) fn path_escapes_root(anchor: &Path, resolved: &Path) -> bool {
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
