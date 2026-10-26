//! 产物预览与包间增量对比（`docs/67` §10）。
//!
//! 一句话：**读已经打出来的包，不再打一遍**。
//!
//! 为什么要这一层：`assetc package` 的命令行摘要说"打了多少 KB"，但作者真正想问的是另外两件事——
//! "这张 3 MB 的图到底进没进包、进了哪个包"（体积归因）和"这次改动之后，哪些东西和上次不一样"
//! （增量）。两者都只能从**产物自身**回答：重新打一遍拿到的永远是"我以为的产物"，不是"发出去的那份"
//! （图集 cook、字体子集、`--keep` 剔除、未声明字体删除都发生在打包中途）。
//!
//! 三条读数口径（都有门，见 `tests.rs` 的 `preview_*` / `diff_*`）：
//! 1. 条目指纹 = **内容 FNV-1a**，不是 mtime、不是长度。容器条目直接取索引里现成的
//!    `Entry::checksum`（零解压），目录条目读字节自己算；两者同一个哈希函数，
//!    由 `preview_entry_checksum_function_matches_the_container` 钉住。
//! 2. 只比**同形态**的两个包：目录形态（`--compress`）落盘的是编码后的字节，容器形态存的是原始字节，
//!    跨形态比会把"换了打包方式"报成"资源改了"。形态不同当场报错，不猜。
//! 3. 输出**一行一条、可 grep**：同一份产物两次读数逐字节相同（不含路径、不含耗时、不含时间戳），
//!    所以它能当门用，也能被发布脚本直接吃。

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use anyhow::anyhow;
use rustlua_infra::bundle::BundleIndex;

use crate::bundle_plan::{BundlePlan, PLAN_FILE};
use crate::package::{container_files, fnv1a, is_boot_entry};
use crate::{BUNDLE_DIR, PACKAGE_REPORT_FILE, PackageReport, collect_files};

/// 条目字节从哪儿读到的。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntrySource {
    /// 目录里的真实文件（目录形态的全部条目、容器形态的 `project.toml`）。
    Loose,
    /// `.rlb` 容器里的条目。
    Bundle,
    /// 计划表 `keep` 里声明"必须以真实文件留在包里"的条目（微信 `wx.loadFont` 的字体）。
    Kept,
}

impl EntrySource {
    /// 读数里用的短名（进文本，也进门的断言）。
    pub fn token(self) -> &'static str {
        match self {
            Self::Loose => "loose",
            Self::Bundle => "bundle",
            Self::Kept => "kept",
        }
    }
}

/// 一个条目在这个包里的读数。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactEntry {
    /// 包内相对路径（`/` 分隔，与 manifest/容器索引同一口径）。
    pub path: String,
    pub size: u64,
    /// 内容的 FNV-1a。
    pub checksum: u64,
    pub source: EntrySource,
    /// 容器形态：所在容器的包内相对路径。
    pub bundle: Option<String>,
    /// 容器形态：所在块的序号（启动闭包是否挤在第 0 块就看它）。
    pub block: Option<u32>,
}

/// 产物自带的**元数据**：它们不是资源，不该出现在条目清单里。
const SKELETON: &[&str] = &["manifest.txt", PACKAGE_REPORT_FILE, PLAN_FILE];

/// 一次产物预览的全部读数。
#[derive(Clone, Debug)]
pub struct ArtifactPreview {
    /// `none` / `single` / `grouped`（目录形态没有报告，按 `none` 记）。
    pub mode: String,
    pub entries: Vec<ArtifactEntry>,
    /// 包内 `package-report.json`（目录形态没有）。
    pub report: Option<PackageReport>,
    /// 包内 `bundle_plan.json`（非分组形态没有）。
    pub plan: Option<BundlePlan>,
}

impl ArtifactPreview {
    /// 读一个包目录。
    ///
    /// 只读，不写：对 `builds/` 下的历史产物同样成立，所以"发布前预览"和"回查上周那个包"
    /// 是同一个入口。
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        if !dir.is_dir() {
            return Err(anyhow!("包目录不存在：{}", dir.display()));
        }
        let plan = read_plan(dir)?;
        let kept: BTreeSet<String> = plan
            .as_ref()
            .and_then(|plan| plan.keep.clone())
            .unwrap_or_default()
            .into_iter()
            .collect();
        let mut entries = Vec::new();
        for file in container_files(dir)? {
            let bytes =
                fs::read(&file).map_err(|error| anyhow!("读 {} 失败：{error}", file.display()))?;
            let index = BundleIndex::parse(&bytes)
                .map_err(|error| anyhow!("{}: {error}", file.display()))?;
            let bundle = relative_to(dir, &file);
            for entry in index.entries() {
                entries.push(ArtifactEntry {
                    path: entry.path.clone(),
                    size: u64::from(entry.len),
                    checksum: entry.checksum,
                    source: EntrySource::Bundle,
                    bundle: Some(bundle.clone()),
                    block: Some(entry.block),
                });
            }
        }
        let mut files = Vec::new();
        collect_files(dir, &mut files)?;
        for (path, _) in files {
            let relative = relative_to(dir, &path);
            if SKELETON.contains(&relative.as_str())
                || relative.starts_with(&format!("{BUNDLE_DIR}/"))
            {
                continue;
            }
            let bytes =
                fs::read(&path).map_err(|error| anyhow!("读 {} 失败：{error}", path.display()))?;
            entries.push(ArtifactEntry {
                source: if kept.contains(&relative) {
                    EntrySource::Kept
                } else {
                    EntrySource::Loose
                },
                size: bytes.len() as u64,
                checksum: fnv1a(&bytes),
                bundle: None,
                block: None,
                path: relative,
            });
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        // 同一条目既有容器版又有目录版 = `--keep` 没生效：容器里那份是白占配额的死重，
        // 而宿主读的是目录那份。`assetc closure` 在发布期查同一件事，这里提前到"看一眼"就报。
        //
        // 唯一的**合法**重复是 `project.toml`：骨架里那份是挂载容器**之前**就要读的（启动第一行
        // 就要拿 main_script），容器里那份是给 closure 核对与回滚用的，两份同源同字节。
        let doubled: Vec<&str> = entries
            .windows(2)
            .filter(|pair| pair[0].path == pair[1].path && pair[0].path != "project.toml")
            .map(|pair| pair[0].path.as_str())
            .collect();
        if !doubled.is_empty() {
            return Err(anyhow!(
                "同一条目既在容器里又留在目录里（--keep 未生效，双份占配额）：{}",
                doubled.join(", ")
            ));
        }
        let report = read_report(dir)?;
        let mode = match &report {
            Some(report) => report.mode.clone(),
            None => "none".to_owned(),
        };
        Ok(Self {
            mode,
            entries,
            report,
            plan,
        })
    }

    /// 包里所有条目的原始字节合计（不含清单/报告/容器索引）。
    ///
    /// 走 [`Self::unique`]：`project.toml` 合法地同时存在于容器与磁盘，按记录数求和会把它算两遍。
    #[must_use]
    pub fn raw_bytes(&self) -> u64 {
        self.unique().values().map(|entry| entry.size).sum()
    }

    /// 容器落盘字节：有报告时信报告（它就是打包当时算出来的），否则只数 `.rlb` 的磁盘长度。
    #[must_use]
    pub fn bundle_bytes(&self) -> u64 {
        if let Some(report) = &self.report {
            return report.bundle_bytes;
        }
        self.entries
            .iter()
            .filter(|entry| entry.source == EntrySource::Bundle)
            .map(|entry| entry.size)
            .sum()
    }

    /// 按路径去重后的条目表（容器版优先：它带块号，是"启动落在第几块"的读数依据）。
    ///
    /// 为什么会有重复：`project.toml` 是唯一**故意**双份的条目（骨架要在挂载前读它，容器里那份
    /// 给 closure 与回滚用）。任何其它路径出现双份，`load` 就直接报错了。
    #[must_use]
    pub fn unique(&self) -> BTreeMap<&str, &ArtifactEntry> {
        let mut out: BTreeMap<&str, &ArtifactEntry> = BTreeMap::new();
        for entry in &self.entries {
            let keep_existing = out
                .get(entry.path.as_str())
                .is_some_and(|existing| existing.source == EntrySource::Bundle);
            if !keep_existing {
                out.insert(entry.path.as_str(), entry);
            }
        }
        out
    }

    /// 合法双份的条目数（目前只可能是 `project.toml`，恒 0 或 1）。
    #[must_use]
    pub fn duplicated(&self) -> usize {
        self.entries.len() - self.unique().len()
    }

    /// 最大的 `top` 个条目（字节降序，同字节按路径升序——否则两次读数会因排序不稳定而抖）。
    #[must_use]
    pub fn largest(&self, top: usize) -> Vec<&ArtifactEntry> {
        let mut ordered: Vec<&ArtifactEntry> = self.unique().into_values().collect();
        ordered.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
        ordered.into_iter().take(top).collect()
    }

    /// 启动闭包条目（只列容器里的：目录形态没有"落在第几块"这回事）。
    #[must_use]
    pub fn boot_places(&self) -> Vec<(&ArtifactEntry, &str, u32)> {
        self.entries
            .iter()
            .filter(|entry| entry.source == EntrySource::Bundle && is_boot_entry(&entry.path))
            .filter_map(|entry| {
                entry
                    .bundle
                    .as_deref()
                    .zip(entry.block)
                    .map(|(bundle, block)| (entry, bundle, block))
            })
            .collect()
    }

    /// 确定性文本读数（`assetc preview` 打印它，编辑器面板也渲染它）。
    #[must_use]
    pub fn text(&self, top: usize) -> String {
        let mut out = format!(
            "assetc_preview mode={} entries={} raw_kb={} bundle_kb={} ratio={:.3}\n",
            self.mode,
            self.unique().len(),
            self.raw_bytes() / 1024,
            self.bundle_bytes() / 1024,
            if self.raw_bytes() == 0 {
                1.0
            } else {
                self.bundle_bytes() as f64 / self.raw_bytes() as f64
            }
        );
        if let Some(report) = &self.report {
            for group in &report.groups {
                let hash = self
                    .plan
                    .as_ref()
                    .and_then(|plan| plan.group(&group.name))
                    .map(|planned| planned.hash.as_str())
                    .unwrap_or("-");
                out.push_str(&format!(
                    "group {} unit={} entries={} raw_kb={} bundle_kb={} hash={} bundle={}\n",
                    group.name,
                    group.unit,
                    group.entries,
                    group.raw_bytes / 1024,
                    group.bundle_bytes / 1024,
                    hash,
                    group.bundle
                ));
            }
        }
        for entry in self.largest(top) {
            out.push_str(&format!(
                "largest {} bytes={} sum={:016x} source={}\n",
                entry.path,
                entry.size,
                entry.checksum,
                entry.source.token()
            ));
        }
        for entry in self
            .entries
            .iter()
            .filter(|entry| entry.source == EntrySource::Kept)
        {
            out.push_str(&format!(
                "kept {} bytes={} sum={:016x}\n",
                entry.path, entry.size, entry.checksum
            ));
        }
        let mut places: Vec<(&str, u32, &str)> = self
            .boot_places()
            .into_iter()
            .map(|(entry, bundle, block)| (bundle, block, entry.path.as_str()))
            .collect();
        places.sort();
        for (bundle, block, path) in &places {
            out.push_str(&format!("boot {bundle} block={block} {path}\n"));
        }
        let distinct: BTreeSet<(&str, u32)> = places
            .iter()
            .map(|(bundle, block, _)| (*bundle, *block))
            .collect();
        out.push_str(&format!(
            "assetc_preview_ok entries={} bundled={} loose={} kept={} duplicated={} boot_entries={} boot_blocks={} largest={}",
            self.unique().len(),
            self.count(EntrySource::Bundle),
            self.count(EntrySource::Loose),
            self.count(EntrySource::Kept),
            self.duplicated(),
            places.len(),
            distinct.len(),
            self.largest(1)
                .first()
                .map(|entry| entry.path.as_str())
                .unwrap_or("-")
        ));
        out
    }

    fn count(&self, source: EntrySource) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.source == source)
            .count()
    }
}

/// 两个包之间的增量。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackageDiff {
    /// 只在新包里的条目。
    pub added: Vec<ArtifactEntry>,
    /// 只在旧包里的条目。
    pub removed: Vec<ArtifactEntry>,
    /// 两边都有、但内容指纹不同的条目（`(旧, 新)`）。
    pub changed: Vec<(ArtifactEntry, ArtifactEntry)>,
    /// 两边都有、指纹也相同的条目数。
    pub unchanged: u64,
    /// 旧包持有者要拿到新包**必须重下**的容器（组名，来自 `bundle_plan.json` 的逐组哈希）。
    pub download: Vec<String>,
    /// 新包里已经没有的组（旧包持有者可以删）。
    pub dropped_groups: Vec<String>,
}

/// 对比两个包目录。
///
/// 判据是**内容指纹**，不是长度、也不是修改时间：同长度换内容（改一个像素、换一张同名图）
/// 是最常见也最容易漏的一种改动，只看长度就会报"没变"。
pub fn diff_packages(previous: &Path, current: &Path) -> anyhow::Result<PackageDiff> {
    let before = ArtifactPreview::load(previous)?;
    let after = ArtifactPreview::load(current)?;
    let container_form = |preview: &ArtifactPreview| preview.count(EntrySource::Bundle) > 0;
    if container_form(&before) != container_form(&after) {
        return Err(anyhow!(
            "只能比同形态的包：旧包 {}（{}），新包 {}（{}）。目录形态落的是编码后的字节、\
             容器形态存的是原始字节，跨形态比会把\"换了打包方式\"报成\"资源改了\"",
            previous.display(),
            before.mode,
            current.display(),
            after.mode
        ));
    }
    let before_map = before.unique();
    let after_map = after.unique();
    let mut diff = PackageDiff::default();
    for (path, entry) in &after_map {
        match before_map.get(path) {
            None => diff.added.push((*entry).clone()),
            Some(old) if old.checksum != entry.checksum => {
                diff.changed.push(((*old).clone(), (*entry).clone()));
            }
            Some(_) => diff.unchanged += 1,
        }
    }
    for (path, entry) in &before_map {
        if !after_map.contains_key(path) {
            diff.removed.push((*entry).clone());
        }
    }
    if let (Some(before_plan), Some(after_plan)) = (&before.plan, &after.plan) {
        // `local.diff(remote)`：持有 `before` 的宿主要把包更新到 `after` 得下哪些组。
        let plan_diff = before_plan.diff(after_plan);
        diff.download = plan_diff
            .download
            .iter()
            .map(|group| group.name.clone())
            .collect();
        diff.dropped_groups = plan_diff
            .remove
            .iter()
            .map(|group| group.name.clone())
            .collect();
    }
    Ok(diff)
}

/// 增量对比的确定性文本读数（`assetc diff`）。
pub fn diff_text(previous: &Path, current: &Path) -> anyhow::Result<String> {
    let diff = diff_packages(previous, current)?;
    let mut out = format!(
        "assetc_diff added={} removed={} changed={} unchanged={}\n",
        diff.added.len(),
        diff.removed.len(),
        diff.changed.len(),
        diff.unchanged
    );
    for entry in &diff.added {
        out.push_str(&format!(
            "added {} bytes={} sum={:016x} source={}\n",
            entry.path,
            entry.size,
            entry.checksum,
            entry.source.token()
        ));
    }
    for entry in &diff.removed {
        out.push_str(&format!(
            "removed {} bytes={} sum={:016x}\n",
            entry.path, entry.size, entry.checksum
        ));
    }
    for (old, new) in &diff.changed {
        out.push_str(&format!(
            "changed {} bytes={}→{} sum={:016x}→{:016x}\n",
            old.path, old.size, new.size, old.checksum, new.checksum
        ));
    }
    for group in &diff.download {
        out.push_str(&format!("download group={group}\n"));
    }
    for group in &diff.dropped_groups {
        out.push_str(&format!("dropped group={group}\n"));
    }
    out.push_str(&format!(
        "assetc_diff_ok download={} dropped={}",
        diff.download.len(),
        diff.dropped_groups.len()
    ));
    Ok(out)
}

/// `assetc preview <包目录> [--top N]` 的入口。
pub fn preview_text(dir: &Path, top: usize) -> anyhow::Result<String> {
    Ok(ArtifactPreview::load(dir)?.text(top))
}

fn read_report(dir: &Path) -> anyhow::Result<Option<PackageReport>> {
    let path = dir.join(PACKAGE_REPORT_FILE);
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&path).map_err(|error| anyhow!("读 {} 失败：{error}", path.display()))?;
    let report: PackageReport = serde_json::from_slice(&bytes)
        .map_err(|error| anyhow!("解析 {} 失败：{error}", path.display()))?;
    Ok(Some(report))
}

fn read_plan(dir: &Path) -> anyhow::Result<Option<BundlePlan>> {
    let path = dir.join(PLAN_FILE);
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&path).map_err(|error| anyhow!("读 {} 失败：{error}", path.display()))?;
    BundlePlan::parse(&bytes)
        .map(Some)
        .map_err(|error| anyhow!("解析 {} 失败：{error}", path.display()))
}

/// 相对包目录的路径，统一成包内口径（`/` 分隔）。
fn relative_to(dir: &Path, path: &Path) -> String {
    path.strip_prefix(dir)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
