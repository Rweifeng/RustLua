//! 发布包（RLB 容器）的挂载入口（`docs/67` §7.3，批 B）。
//!
//! 形态判定只看一个事实：`<工程根>/bundles/` 下有没有 `*.rlb`。
//! - 有 → 全部挂上，装 vfs 的 provider + lister：此后 `vfs::read` 先查容器、未命中回落文件系统，
//!   `vfs::list` 合并"容器条目"与"磁盘目录"（发布形态下 `scripts/` 只在容器里）；
//! - 没有 → 什么都不做（开发形态与今天的目录形态打包完全等价，零行为变化）。
//!
//! 为什么挂载放在 `game` 而不是 `rustlua-infra`：容器只提供"能挂多个包"的能力，
//! **什么形态该挂哪些包**是宿主决定的事（桌面看目录、Web 看预载表、微信看分包）。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use rustlua_infra::bundle::BundleSet;

/// 挂载结果（打印/报告用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountReport {
    pub bundles: Vec<PathBuf>,
    pub entries: usize,
    /// 计划里有、但文件缺失或解析失败的容器（`docs/67` §7.4 第二种情形）。
    pub missing: Vec<String>,
}

impl MountReport {
    /// 一行摘要（启动日志与 `--check` 都用它）。
    pub fn summary(&self) -> String {
        let names: Vec<String> = self
            .bundles
            .iter()
            .filter_map(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .collect();
        let mut line = format!(
            "bundles={} entries={} files=[{}]",
            self.bundles.len(),
            self.entries,
            names.join(", ")
        );
        if !self.missing.is_empty() {
            line.push_str(&format!(" missing=[{}]", self.missing.join(", ")));
        }
        line
    }
}

/// 进程内已挂载的包集合（`--check` 的全量校验需要它）。
static MOUNTED: OnceLock<Arc<Mutex<BundleSet>>> = OnceLock::new();

/// 计划里"本该有、实际没挂上"的容器（`--check` 把它们当缺失项）。
static MISSING: OnceLock<Vec<String>> = OnceLock::new();

/// 挂载 `<root>/bundles/*.rlb`；没有容器返回 `Ok(None)`（= 目录形态，什么都不改）。
pub fn mount_pack_bundles(root: &Path) -> Result<Option<MountReport>, String> {
    let bundles_dir = root.join("bundles");
    if !bundles_dir.is_dir() {
        return Ok(None);
    }
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&bundles_dir)
        .map_err(|error| format!("读 {} 失败：{error}", bundles_dir.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("rlb"))
        })
        .collect();
    paths.sort();
    if paths.is_empty() {
        return Ok(None);
    }

    let mut set = BundleSet::new(root);
    let mut entries = 0usize;
    let mut mounted = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for path in paths {
        match set.mount_file(&path) {
            Ok(count) => {
                entries += count;
                mounted.push(path);
            }
            Err(error) => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                failed.push(format!("{name}: {error}"));
            }
        }
    }

    // 计划表里"该有但没挂上"的组 → 登记为 pending：读到那些条目时诊断能点名到**组**
    // （`docs/67` §7.4 的第二种情形；批 C 把计划表接上之后它才有输入）。
    let plan = read_plan(root)?;
    let mut missing: Vec<String> = Vec::new();
    if let Some(plan) = &plan {
        let mounted_rel: Vec<String> = mounted
            .iter()
            .filter_map(|path| {
                path.strip_prefix(root)
                    .ok()
                    .map(|rel| rel.to_string_lossy().replace('\\', "/"))
            })
            .collect();
        for (bundle, members) in plan.pending_for(&mounted_rel) {
            missing.push(bundle.clone());
            set.declare_pending(&bundle, members);
        }
    }
    missing.extend(failed);

    let set = Arc::new(Mutex::new(set));
    // provider 是**进程级一次**安装（复核 P1-5）。`game` 里目前只有这一个安装点，
    // 但"第二次安装"必须当成幂等成功而不是启动失败——与 `lua_bridge::install_resource_provider`
    // 同一条纪律：后到者内容相同，报错只会把整条启动链弄红。
    // 真正的边界（同一进程换工程根会继续用旧包）写在 docs/67 的复核清单里。
    match rustlua_infra::bundle::install(Arc::clone(&set)) {
        Ok(()) => {}
        Err(error) => {
            tracing::warn!(
                target: "bundle",
                "bundle provider 已安装，沿用现有 provider（{error}）；同一进程不换工程根的前提下内容相同"
            );
        }
    }
    let _ = MOUNTED.set(set);
    let _ = MISSING.set(missing.clone());
    Ok(Some(MountReport {
        bundles: mounted,
        entries,
        missing,
    }))
}

/// 读 `bundle_plan.json`：没有或解析失败都当"没有计划"（不影响挂载，只影响诊断精度）。
/// 读计划表。
///
/// **存在但解析失败 = 硬错误**，不再降级成"没有计划"（复核 P2 的教训：自校验加上之后
/// 仍然静默通过，因为这里把错误吞了）。计划表是缺件归因的唯一依据：
/// 它坏了却继续启动，症状会变成"某些资源莫名读不到且报不出包名"。
fn read_plan(root: &Path) -> Result<Option<rustlua_infra::bundle::plan::BundlePlan>, String> {
    let path = root.join(rustlua_infra::bundle::plan::PLAN_FILE);
    if !path.is_file() {
        return Ok(None); // 目录形态没有计划表，属正常
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("读 {} 失败：{error}", path.display()))?;
    rustlua_infra::bundle::plan::BundlePlan::parse(text.as_bytes())
        .map(Some)
        .map_err(|error| format!("{}：{error}", path.display()))
}

/// 计划里缺失的容器（`--check` 用；空 = 该有的都在）。
pub fn missing_bundles() -> Vec<String> {
    MISSING.get().cloned().unwrap_or_default()
}

/// 逐条目全量校验所有已挂载的包（`--check` 用）。
///
/// 与"读一次没报错"的区别：这里会把每个块都解一遍、每个条目的 FNV 都核一遍，
/// 所以能抓到"只有某些块坏了"的发布事故。
pub fn verify_mounted() -> Result<usize, String> {
    let Some(set) = MOUNTED.get() else {
        return Ok(0);
    };
    let mut guard = set.lock().map_err(|_| "bundle set 锁中毒".to_owned())?;
    guard
        .verify_all()
        .map_err(|error| format!("资源包全量校验失败：{error}"))?;
    Ok(guard.entry_count())
}

/// 已挂载条目的包内相对路径（诊断/回归用；`game` 本体暂不消费，用 `#[allow]` 标明是有意的）。
#[allow(dead_code)]
pub fn mounted_paths() -> Vec<String> {
    let Some(set) = MOUNTED.get() else {
        return Vec::new();
    };
    let Ok(guard) = set.lock() else {
        return Vec::new();
    };
    guard.paths().map(str::to_owned).collect()
}
