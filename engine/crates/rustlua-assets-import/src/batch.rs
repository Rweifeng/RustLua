//! 批量导入驱动：逐项导入、逐项写回资源库，**一项坏不打断整批**。
//!
//! 住在导入层而不是运行时：它要 [`ImportCache`]（写 `.cache/cache/`）与注册表，这两样
//! 运行时都不碰——运行时只读已经写好的资源库与产物。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use rustlua_assets::importer::ImporterRegistry;
use rustlua_assets::resource_db::{AssetId, ResourceDatabase};
use rustlua_assets::resource_kind::ResourceKind;

use crate::import_cache::ImportCache;
/// Result of importing one item within a batch.
#[derive(Clone, Debug)]
pub struct ImportItemResult {
    /// Absolute path of the source that was imported.
    pub source: PathBuf,
    /// True when the cache hit and no new artifact was written.
    pub reused: bool,
    /// Cache artifact path when the item succeeded, project-root relative.
    pub cache_path: Option<PathBuf>,
    /// Error message when the item failed, otherwise `None`.
    pub error: Option<String>,
}

/// Aggregate counts for a batch run; one bad item never aborts the batch.
#[derive(Clone, Debug, Default)]
pub struct ImportSummary {
    pub total: usize,
    pub imported: usize,
    pub reused: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Wall-clock time of the whole batch in milliseconds.
    pub elapsed_ms: u64,
    /// Per-item results, in the same order as `items`.
    pub results: Vec<ImportItemResult>,
}

/// 一项资源导入成功后，把需要"派生产物"的源文件补齐（P3-R03-4）。
///
/// `Environment` 的运行时真源是同名 `.rlenv`（导入期烘焙），组件串里作者写的却是
/// `.hdr` 源文件；如果只在离线工具里烘焙，设计者往编辑器里丢一个 `.hdr` 就会得到
/// "读取 xxx.rlenv 失败"。所以这里在导入这一步补齐产物，失败就整项判失败（原因可读）。
fn bake_companion(source: &Path, kind: ResourceKind) -> Result<(), String> {
    if kind != ResourceKind::Environment {
        return Ok(());
    }
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension != "hdr" {
        return Ok(());
    }
    crate::environment::bake_hdr_file(source, crate::environment::BakeOptions::default())
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Runs a batch of imports, writing per-item state back to the database.
///
/// Items are imported one at a time; a failing item is recorded via
/// `mark_failed` and the batch continues. `cancel` is checked between items so
/// a caller can stop the batch cleanly. The database is saved once at the end.
pub fn run_import_batch(
    project: &Path,
    db: &mut ResourceDatabase,
    cache: &mut ImportCache,
    registry: &ImporterRegistry,
    items: &[(AssetId, PathBuf)],
    cancel: &AtomicBool,
) -> ImportSummary {
    run_import_batch_with_progress(project, db, cache, registry, items, cancel, |_, _| {})
}

/// Runs a batch of imports with a per-item progress callback.
///
/// Same behavior as [`crate::batch::run_import_batch`], but `on_progress(processed, total)`
/// is called after each item so a caller can drive a determinate progress bar
/// from real work. `processed` is the number of items already examined
/// (including the current one); `total` is the batch size.
pub fn run_import_batch_with_progress<F>(
    _project: &Path,
    db: &mut ResourceDatabase,
    cache: &mut ImportCache,
    registry: &ImporterRegistry,
    items: &[(AssetId, PathBuf)],
    cancel: &AtomicBool,
    mut on_progress: F,
) -> ImportSummary
where
    F: FnMut(usize, usize),
{
    let started = Instant::now();
    let mut summary = ImportSummary {
        total: items.len(),
        ..Default::default()
    };
    let total = items.len();
    for (index, (id, source)) in items.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(importer) = registry.from_path(source) else {
            summary.skipped += 1;
            summary.results.push(ImportItemResult {
                source: source.clone(),
                reused: false,
                cache_path: None,
                error: Some("不支持的资源类型".to_owned()),
            });
            continue;
        };
        match cache.import_file(*id, source, importer) {
            Ok(report) => {
                if report.reused {
                    summary.reused += 1;
                } else {
                    summary.imported += 1;
                }
                let cache_path = report.cache_path.clone();
                let hash = report.fingerprint.clone();
                match db.mark_ready(*id, cache_path, hash) {
                    Ok(()) => match bake_companion(source, importer.kind()) {
                        Ok(()) => summary.results.push(ImportItemResult {
                            source: source.clone(),
                            reused: report.reused,
                            cache_path: Some(report.cache_path),
                            error: None,
                        }),
                        Err(error) => {
                            summary.failed += 1;
                            let message = format!("{}: {error}", source.display());
                            summary.results.push(ImportItemResult {
                                source: source.clone(),
                                reused: false,
                                cache_path: None,
                                error: Some(message.clone()),
                            });
                            if let Err(db_error) = db.mark_failed(*id, message) {
                                tracing::warn!("mark_failed({id}) 失败: {db_error}");
                            }
                        }
                    },
                    Err(error) => {
                        summary.failed += 1;
                        summary.results.push(ImportItemResult {
                            source: source.clone(),
                            reused: false,
                            cache_path: None,
                            error: Some(format!("{}: {error}", source.display())),
                        });
                    }
                }
            }
            Err(error) => {
                summary.failed += 1;
                let message = format!("{}: {error}", source.display());
                summary.results.push(ImportItemResult {
                    source: source.clone(),
                    reused: false,
                    cache_path: None,
                    error: Some(message.clone()),
                });
                if let Err(db_error) = db.mark_failed(*id, message) {
                    tracing::warn!("mark_failed({id}) 失败: {db_error}");
                }
            }
        }
        on_progress(index + 1, total);
    }
    summary.elapsed_ms = started.elapsed().as_millis() as u64;
    if let Err(error) = db.save() {
        tracing::warn!("导入后保存资源数据库失败: {error}");
    }
    summary
}
