//! 发布闭包检查（P0，`docs/67` §5.8 的延伸）：**计划表里的每一条逻辑资源路径，
//! 都必须在它所在分发根的容器索引里真能解析**。
//!
//! 存在理由：`check-bundle-mounts.js` 只证明"计划声明的根/入口/容器文件存在"，
//! `check-boot-closure.js` 只证明"boot 元数据与首屏清单一致"。两者都**不打开容器**，
//! 于是"容器建好了但少了某个按需资源"这类缺陷可以一路绿到真机——启动场景刚好不用它，
//! 玩到一半才发现白图。容器里的条目名是逻辑路径（`assets/x.png`），与发布后的物理位置
//! （`pack/`、`pack-sub/`、微信的 `.txt` 改名）是两套坐标，只有在**产物上**逐条对账才算闭环。
//!
//! 检查项（全部针对最终发布目录，不再回看工程源）：
//! 1. 计划表能解析（含自校验 checksum）；
//! 2. 每个非 `cdn` 组的容器在它所属的根里存在（接受微信白名单的 `.rlb.txt` 改名）；
//! 3. 容器索引可解析，`entries` 数与计划一致，`block_size` 与计划一致；
//! 4. 组里每个逻辑路径都在索引里（**核心断言**）；
//! 5. 条目原始字节合计 == `raw_bytes`，文件大小 == `bundle_bytes`，整包 FNV == `hash`；
//! 6. 根下 `bundles/` 里没有计划外的容器，也没有计划里声明却没落地的容器。
//!
//! 用法：`assetc closure <发布根> [--plan <bundle_plan.json>] [--json]`。
//! 发布根是**分发根的父亲**（WebGL/PC 是含 `pack/` 的发布目录，微信是 launcher）。

use anyhow::{Context, anyhow};
use rustlua_infra::bundle::plan::{BundlePlan, PlannedGroup};
use rustlua_infra::bundle::{BundleIndex, checksum};
use rustlua_infra::bundle_plan::root_of_unit;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// 检查结果。问题**全部收集**（发布脚本要一次列全，而不是修一条跑一次）。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ClosureReport {
    /// 实际参与检查的根（`pack`、`pack-sub`、`pack-<name>`）。
    pub roots: Vec<String>,
    /// 参与检查的组数（不含 `cdn`）。
    pub groups: usize,
    /// 计划声明的条目总数（不含 `cdn`）。
    pub entries: u64,
    /// 计划里的 `cdn` 组名（**全部**，不论最终在哪里核对的）。
    ///
    /// `cdn` 组的字节有两种落点（2026-10-20 真产物体检后修正口径）：
    /// - **在包里**：WebGL/PC 把 `cdn` 组当普通内容打进去（webgl 没有远端那一跳），微信
    ///   remote 形态也是先落 `pack/bundles/` 再镜像出去——这种按普通组的同一套断言核（计 `cdn_local`）；
    /// - **只在远端**：容器被搬到了镜像目录（`-CdnPackDir` / `assets/`），发布根里没有——
    ///   这种必须用 `--cdn <目录>` 核，否则算"未检查"（计为问题，不是静默跳过）。
    pub cdn_groups: Vec<String>,
    /// 在**发布根里**核对过的 `cdn` 组数（字节就在包内）。
    pub cdn_local: usize,
    /// 在**远端目录里**核对过的 `cdn` 组数（`--cdn` 指定）。
    pub cdn_checked: usize,
    /// 在**远端目录里**核对过的**非 `cdn` 组**数。瘦首包（`-AssetPack remote`）把整个 `pack/`
    /// 裁成骨架后，主包/分包的容器全都在资源包里——这些组只有 `--cdn` 才核得到。
    pub remote_checked: usize,
    /// 核对过的 `keep` 条目数（必须留在目录里的真实文件）。
    pub keep_checked: usize,
    /// 其中**只**在远端资源包里找到的 `keep` 条目数（瘦首包形态：字体/音频随资源包上 CDN）。
    pub keep_remote: usize,
    /// 计划表里**有没有** keep 元数据：`false` = 旧版本打的计划（那时还没有这个字段），
    /// 于是"keep=0"只表示"没有契约可查"，不表示"确认零条需要留在目录里"。
    pub keep_contract: bool,
    pub problems: Vec<String>,
}

impl ClosureReport {
    /// 一次都没核对过的 `cdn` 组数（既不在包里，也没给 `--cdn`）。
    pub fn cdn_skipped(&self) -> usize {
        self.cdn_groups
            .len()
            .saturating_sub(self.cdn_local + self.cdn_checked)
    }

    /// 一行摘要（发布日志用）。
    pub fn summary(&self) -> String {
        format!(
            "roots={} groups={} entries={} cdn_skipped={} cdn_local={} cdn_checked={} remote_checked={} keep={} keep_remote={} keep_contract={}",
            self.roots.len(),
            self.groups,
            self.entries,
            self.cdn_skipped(),
            self.cdn_local,
            self.cdn_checked,
            self.remote_checked,
            self.keep_checked,
            self.keep_remote,
            self.keep_contract
        )
    }
}

/// 找计划表：显式路径 → `<base>/bundle_plan.json`（微信 launcher 根）→
/// `<base>/pack/bundle_plan.json`（assetc 原始落点 / WebGL 与 PC 发布根）。
#[must_use]
pub fn locate_plan(base: &Path, explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = explicit {
        return Some(path.to_path_buf());
    }
    let candidates = [
        base.join("bundle_plan.json"),
        base.join("pack").join("bundle_plan.json"),
    ];
    candidates.into_iter().find(|path| path.is_file())
}

/// 物理文件名 → 计划里的逻辑容器名（`<x>.rlb`）。
///
/// 微信代码包只上传白名单后缀，`.rlb` 可能被改名（实测产物里是 `.rlb.txt`）。这里**不枚举**改名
/// 规则，而是按**容器格式**定界：从 `.rlb` 截断，后面的部分一律当传输期后缀。于是
/// `x.rlb` / `x.rlb.txt` / `x.rlb.bin` 都归到 `x.rlb`，`x.rlb2` 不是（`.rlb` 后面必须是 `.`）。
/// 改名的真相只有 `scripts/lib/code-package-rules.js` 一处；这里再抄一份规则，两边就会漂。
fn logical_bundle_name(file_name: &str) -> Option<&str> {
    let lower = file_name.to_ascii_lowercase();
    let index = lower.find(".rlb")?;
    let after = &file_name[index + 4..];
    if after.is_empty() || after.starts_with('.') {
        Some(&file_name[..index + 4])
    } else {
        None
    }
}

/// 某个根下与计划条目对应的真实容器文件（可能带传输后缀）。
fn container_candidates(root_dir: &Path, bundle: &str) -> Vec<PathBuf> {
    let logical = Path::new(bundle);
    let Some(name) = logical.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    let dir = root_dir.join(logical.parent().unwrap_or_else(|| Path::new("")));
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| {
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy();
            logical_bundle_name(&file_name) == Some(name)
        })
        .map(|entry| entry.path())
        .collect();
    found.sort();
    found
}

/// 读一个根下 `bundles/` 里真实存在的容器（还原成计划里的逻辑名）。
fn bundled_files(root_dir: &Path) -> BTreeSet<String> {
    let dir = root_dir.join("bundles");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return BTreeSet::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            logical_bundle_name(&name).map(|logical| format!("bundles/{logical}"))
        })
        .collect()
}

/// 检查的可选项。
#[derive(Debug, Clone, Copy, Default)]
pub struct ClosureOptions<'a> {
    /// 远端容器目录：`-AssetPack remote` 的资源包（`<发布根>/assets/pack`）或 `-CdnPackDir` 镜像。
    ///
    /// 两类用途：① 瘦首包把 `pack/` 裁成骨架，**所有**组的容器都在资源包里——本地找不到的组在这里核；
    /// ② 给定时所有 `cdn` 组都会在这里再核一遍（镜像才是要上传的那份）。
    /// 不给而计划里又有 `cdn` 组时，那些组**不检查**——这会被记成问题而不是静默跳过：
    /// "跳过"与"通过"在发布日志里长得一样，正是本模块存在的理由。
    pub cdn: Option<&'a Path>,
    /// 明确声明"这次不检查远端组"（CDN 产物不在这台机器上：验收机拿不到镜像）。
    ///
    /// 只有当调用方**知道**自己在跳过时才用它；摘要里仍会打印 `cdn_skipped=N`，
    /// 所以日志里"跳过了几个"永远可见，不会伪装成 0。
    pub allow_cdn_skip: bool,
}

/// 对最终发布目录做一次闭包检查。
///
/// 返回 `Ok(report)` 表示"检查跑完了"（是否通过看 `problems`）；`Err` 只用于
/// 连检查前提都不成立的情况（找不到计划表、计划表本身损坏）。
pub fn check_release_closure(
    base: &Path,
    plan_path: Option<&Path>,
) -> anyhow::Result<ClosureReport> {
    check_release_closure_with(base, plan_path, ClosureOptions::default())
}

/// 带选项的版本（`--cdn`）。
pub fn check_release_closure_with(
    base: &Path,
    plan_path: Option<&Path>,
    options: ClosureOptions<'_>,
) -> anyhow::Result<ClosureReport> {
    let plan_file = locate_plan(base, plan_path).ok_or_else(|| {
        anyhow!(
            "找不到 bundle_plan.json（{} 或 pack/）：这不是容器形态，或发布没跑 grouped 打包",
            base.join("bundle_plan.json").display()
        )
    })?;
    let bytes = std::fs::read(&plan_file)
        .with_context(|| format!("读计划表失败：{}", plan_file.display()))?;
    let plan = BundlePlan::parse(&bytes).map_err(anyhow::Error::msg)?;
    check_plan_against_dir(base, &plan, options)
}

fn check_plan_against_dir(
    base: &Path,
    plan: &BundlePlan,
    options: ClosureOptions<'_>,
) -> anyhow::Result<ClosureReport> {
    let mut report = ClosureReport::default();
    // 两种产物形态，读者不同、物理布局不同：
    //   * 多根（微信）：`pack/`、`pack-sub/`、`pack-<name>/` 按 unit 分开，适配层按计划路由；
    //   * 单根（WebGL / PC）：没有分包，所有容器都在 `pack/bundles/`，`unit` 只影响首屏判定。
    // 判据是"根目录里有没有 pack-* 兄弟目录"——这正是适配层 `hasPackSubpackage` 的判据。
    let multi_root = std::fs::read_dir(base)
        .map(|entries| {
            entries.flatten().any(|entry| {
                entry.file_name().to_string_lossy().starts_with("pack-") && entry.path().is_dir()
            })
        })
        .unwrap_or(false);
    // 单根形态下 `pack/` 也可能不存在：`assetc package -o <dir> --bundle grouped` 的**原始输出**
    // 直接把容器写在 `<dir>/bundles/` 里，没有多包一层 `pack/`。这时把发布根本身当作 pack 根
    // （CI 的合成分组产物就走这条，免得为了跑门先手搭一遍 package.ps1 的目录结构）。
    let single = if base.join("pack").is_dir() {
        ("pack".to_owned(), base.join("pack"))
    } else {
        ("<发布根>".to_owned(), base.to_path_buf())
    };
    let root_for = |unit: &str| -> anyhow::Result<Option<(String, PathBuf)>> {
        Ok(match (multi_root, root_of_unit(unit)?) {
            (true, Some(name)) => {
                let dir = base.join(&name);
                Some((name, dir))
            }
            (false, Some(_)) => Some(single.clone()),
            (_, None) => None,
        })
    };
    // bundle → 组（同一容器被两组声明是计划损坏，直接报）。
    let mut claimed: BTreeMap<&str, &str> = BTreeMap::new();
    for group in &plan.groups {
        if let Some(previous) = claimed.insert(group.bundle.as_str(), group.name.as_str()) {
            report.problems.push(format!(
                "容器 {} 同时被组 {previous} 与 {} 声明",
                group.bundle, group.name
            ));
        }
    }

    // 逐个容器核对；顺带把"容器里到底有哪些条目"汇总起来——`keep` 断言要用它反查
    // "本该留在目录里的文件是不是也被塞进了容器"。
    let mut container_keys: BTreeSet<String> = BTreeSet::new();
    // `cdn` 组的字节如果就在包里（WebGL/PC 形态、微信 remote 的 `pack/` 原始落点），
    // 就按普通组的同一套断言核一遍。"组名是 cdn"本身不是跳过的理由。
    let cdn_local_root = {
        let pack = base.join("pack");
        if pack.is_dir() {
            ("pack".to_owned(), pack)
        } else {
            ("<发布根>".to_owned(), base.to_path_buf())
        }
    };
    let mut verified_cdn: BTreeSet<String> = BTreeSet::new();
    for group in &plan.groups {
        let Some((root, root_dir)) = root_for(&group.unit)? else {
            report.cdn_groups.push(group.name.clone());
            if container_candidates(&cdn_local_root.1, &group.bundle)
                .into_iter()
                .any(|path| path.is_file())
                && check_group_container(
                    group,
                    &cdn_local_root.0,
                    &cdn_local_root.1,
                    plan,
                    &mut report,
                    &mut container_keys,
                )
            {
                report.cdn_local += 1;
                verified_cdn.insert(group.name.clone());
            }
            continue;
        };
        report.groups += 1;
        report.entries += group.entries;
        // 瘦首包（`-AssetPack remote`）把 `pack/` 裁成骨架，**所有**组的容器都被搬到资源包
        // （`<发布根>/assets/pack/…`）再传 CDN——不只是 `cdn` 组。这时本地根必然找不到容器，
        // 而"找不到就判红"会把一份正确的瘦首包判死；只看 `cdn` 组又会把主包/分包的容器
        // （真正的重资源）整类放过。所以：本地缺件、给了 `--cdn`、远端目录里确实有 → 在远端核。
        if let Some(cdn_dir) = options.cdn
            && !container_candidates(&root_dir, &group.bundle)
                .into_iter()
                .any(|path| path.is_file())
            && container_candidates(cdn_dir, &group.bundle)
                .into_iter()
                .any(|path| path.is_file())
        {
            if check_group_container(
                group,
                "<cdn>",
                cdn_dir,
                plan,
                &mut report,
                &mut container_keys,
            ) {
                report.remote_checked += 1;
            }
            continue;
        }
        if !report.roots.contains(&root) {
            report.roots.push(root.clone());
        }
        check_group_container(
            group,
            &root,
            &root_dir,
            plan,
            &mut report,
            &mut container_keys,
        );
    }

    // 远端（`cdn`）组：字节只在镜像目录里（`-CdnPackDir` / `assets/`）时，必须用 `--cdn` 核。
    // 此前这些组被**静默跳过**（只数个数），于是"远端少了一个容器/容器内容与计划不一致"
    // 在发布日志里与"通过"长得一模一样。
    //
    // 给了 `--cdn` 就**所有** `cdn` 组都在镜像里核一遍：镜像才是要上传的那份，
    // 包内那份对得上不等于镜像对得上（`package.ps1 -CdnPackDir` 是复制，不是硬链接）。
    if let Some(cdn_dir) = options.cdn {
        for group in plan.groups.iter().filter(|group| group.unit == "cdn") {
            if check_group_container(
                group,
                "<cdn>",
                cdn_dir,
                plan,
                &mut report,
                &mut container_keys,
            ) {
                report.cdn_checked += 1;
                verified_cdn.insert(group.name.clone());
            }
        }
    }
    // 一次都没核到过的 `cdn` 组才是"未检查"：既不在包里，也没给 `--cdn`。
    if !options.allow_cdn_skip {
        for name in &report.cdn_groups {
            if verified_cdn.contains(name) {
                continue;
            }
            report.problems.push(format!(
                "组 {name} 属于 cdn，但发布根和远端目录里都没核对到它：这条闭包**未检查**（用 --cdn <目录> 指认镜像，或显式 --allow-cdn-skip 声明这次不查）"
            ));
        }
    }

    // `keep` 条目（微信字体/音频这类宿主按真实路径读的文件）：必须留在目录里，
    // 且不得再出现在任何容器索引里。两条都查，因为两种失效方向都会真机出事：
    // 只在容器里 → `wx.loadFont` 读不到真实路径；两边都有 → 白占配额且两份会漂移。
    let (keep_root, keep_dir) = if multi_root {
        ("pack".to_owned(), base.join("pack"))
    } else {
        single.clone()
    };
    // `None` = 这份计划是还没有 keep 元数据的旧版本打的：没有契约可查（不是"声明了零条"）。
    report.keep_contract = plan.keep.is_some();
    for relative in plan.keep.iter().flatten() {
        report.keep_checked += 1;
        let file = keep_dir.join(relative);
        if !file.is_file() {
            // 瘦首包（`-AssetPack remote`）把 `pack/` 裁成骨架，字体/音频随资源包导出到
            // `assets/pack/…`，适配层启动时先下载再按真实路径喂给 `wx.loadFont`。
            // 所以"不在发布根里"本身不等于坏，**只要它在资源包里**；两处都没有才是真缺件。
            let remote_copy = options.cdn.map(|dir| dir.join(relative));
            if remote_copy.as_ref().is_some_and(|path| path.is_file()) {
                report.keep_remote += 1;
            } else {
                let hint = match options.cdn {
                    Some(dir) => format!("（也不在远端目录 {}/ 里）", dir.display()),
                    None => {
                        "（瘦首包形态下它应随资源包导出，用 --cdn 指定那份目录才能核对）".to_owned()
                    }
                };
                report.problems.push(format!(
                    "keep 条目不在目录里：{relative}（应在 {keep_root}/，宿主按真实路径读它）{hint}"
                ));
            }
        }
        if container_keys.contains(relative) {
            report.problems.push(format!(
                "keep 条目同时被装进了容器：{relative}（--keep 没生效，同一份资源双份占配额）"
            ));
        }
    }

    // 计划外的容器：容器存在但没人声明 = 打包/搬运把文件放错了根。
    let mut scanned: BTreeSet<(String, PathBuf)> = BTreeSet::new();
    for group in &plan.groups {
        if let Some(entry) = root_for(&group.unit)? {
            scanned.insert(entry);
        }
    }
    for (root, root_dir) in scanned {
        let present = bundled_files(&root_dir);
        // "已声明"按**全部组**算（含 `cdn`）：remote 形态下 cdn 组的容器本来就先落在
        // `pack/bundles/` 再镜像到 CDN，把它算成"计划外"会是假红。真正的断言在 `--cdn` 那一节。
        let declared: BTreeSet<String> = plan
            .groups
            .iter()
            .map(|group| group.bundle.clone())
            .collect();
        for unexpected in present.difference(&declared) {
            report
                .problems
                .push(format!("{root}/ 里有计划外的容器：{unexpected}"));
        }
        // 缺容器已在上面逐组报过，这里不重复。
    }

    Ok(report)
}

/// 核对一个组的容器：存在 → 可解析 → 计数/块大小/字节/FNV 与计划一致 → **每条计划路径都在索引里**。
///
/// 返回 `true` 表示"这个组真的检查完了"（`cdn_checked` 用它计数）；返回 `false` 时问题已记进
/// `report.problems`，调用方不需要再补一条。
fn check_group_container(
    group: &PlannedGroup,
    root: &str,
    root_dir: &Path,
    plan: &BundlePlan,
    report: &mut ClosureReport,
    container_keys: &mut BTreeSet<String>,
) -> bool {
    let Some(container) = container_candidates(root_dir, &group.bundle)
        .into_iter()
        .find(|path| path.is_file())
    else {
        // 报错要能直接指路：既说"缺哪个"，也说"这个根下实际有什么"——
        // 只有前者的报错，遇到"搬错根"时得人肉 diff 两份目录。
        let present = bundled_files(root_dir);
        let inventory = if present.is_empty() {
            "无".to_owned()
        } else {
            present.into_iter().collect::<Vec<_>>().join(", ")
        };
        report.problems.push(format!(
            "组 {} 的容器不在 {root}/ 里：{}（该根下现有容器：{inventory}）",
            group.name, group.bundle
        ));
        return false;
    };
    let bytes = match std::fs::read(&container) {
        Ok(bytes) => bytes,
        Err(error) => {
            report
                .problems
                .push(format!("读容器 {} 失败：{error}", container.display()));
            return false;
        }
    };
    let index = match BundleIndex::parse(&bytes) {
        Ok(index) => index,
        Err(error) => {
            report.problems.push(format!(
                "容器 {} 索引无法解析：{error}",
                container.display()
            ));
            return false;
        }
    };

    if index.len() as u64 != group.entries {
        report.problems.push(format!(
            "组 {} 条目数不符：计划 {}，容器 {}",
            group.name,
            group.entries,
            index.len()
        ));
    }
    if index.block_size() != plan.block_size {
        report.problems.push(format!(
            "组 {} 的 block_size 不符：计划 {}，容器 {}",
            group.name,
            plan.block_size,
            index.block_size()
        ));
    }
    let raw_bytes: u64 = index
        .entries()
        .iter()
        .map(|entry| u64::from(entry.len))
        .sum();
    if raw_bytes != group.raw_bytes {
        report.problems.push(format!(
            "组 {} 原始字节合计不符：计划 {}，容器 {raw_bytes}",
            group.name, group.raw_bytes
        ));
    }
    if group.bundle_bytes != 0 && bytes.len() as u64 != group.bundle_bytes {
        report.problems.push(format!(
            "组 {} 容器大小不符：计划 {} B，实际 {} B",
            group.name,
            group.bundle_bytes,
            bytes.len()
        ));
    }
    if !group.hash.is_empty() {
        let actual = format!("{:016x}", checksum(&bytes));
        if actual != group.hash {
            report.problems.push(format!(
                "组 {} 容器 FNV 不符：计划 {}，实际 {actual}",
                group.name, group.hash
            ));
        }
    }
    for path in index.paths() {
        container_keys.insert(path.to_owned());
    }

    // 核心断言：计划里每一条逻辑路径都必须能从容器里取出来。
    let missing: Vec<&String> = group
        .paths
        .iter()
        .filter(|path| !index.contains(path))
        .collect();
    if !missing.is_empty() {
        let sample: Vec<&str> = missing.iter().take(8).map(|path| path.as_str()).collect();
        report.problems.push(format!(
            "组 {}（{root}/{}）缺 {} 条计划内资源，例如：{}",
            group.name,
            group.bundle,
            missing.len(),
            sample.join(", ")
        ));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_infra::bundle::plan::BundlePlan;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rustlua-closure-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const RED_PNG: [u8; 70] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    fn project(root: &Path) {
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        std::fs::create_dir_all(root.join("assets/textures")).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "[project]\nname = \"closure\"\nversion = \"0.1.0\"\nengine = \"RustLua\"\n\
             main_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n",
        )
        .unwrap();
        std::fs::write(root.join("scripts/main.lua"), "function update() end\n").unwrap();
        std::fs::write(
            root.join("scenes/main.scene"),
            "scene_version=2\nname=main\n",
        )
        .unwrap();
        std::fs::write(root.join("assets/textures/red.png"), RED_PNG).unwrap();
    }

    /// 建一份真实的 grouped 发布产物：`<base>/pack/` 下是容器 + 计划表。
    fn release(base: &Path) -> BundlePlan {
        let project_dir = base.join("project");
        project(&project_dir);
        let pack_root = base.join("pack");
        crate::package_grouped(&project_dir, &pack_root, false, false).expect("grouped 打包");
        let bytes = std::fs::read(pack_root.join("bundle_plan.json")).unwrap();
        BundlePlan::parse(&bytes).unwrap()
    }

    /// 瘦首包（`-AssetPack remote`）：`pack/` 被裁成骨架，**所有**组的容器都随资源包导出到
    /// `<发布根>/assets/pack/…`。这时"本地找不到就判红"会把正确产物判死，而只核 `cdn` 组又会把
    /// 真正的大头（主包/分包容器）整类放过。`--cdn <资源包>` 必须能核到每一组。
    #[test]
    fn thin_release_checks_every_container_against_the_asset_export() {
        let base = temp_dir("thin-release");
        let plan = release(&base);
        let export = base.join("assets/pack");
        std::fs::create_dir_all(export.join("bundles")).unwrap();
        for group in &plan.groups {
            let from = base.join("pack").join(&group.bundle);
            if from.is_file() {
                std::fs::rename(from, export.join(&group.bundle)).unwrap();
            }
        }
        std::fs::remove_dir_all(base.join("pack/bundles")).unwrap();

        // 不给远端目录 → 每组的容器都"不在 pack/ 里"，必须逐组报出来（不是静默跳过）。
        let report = check_release_closure(&base, None).expect("计划表可解析");
        assert_eq!(report.remote_checked, 0);
        assert!(
            report
                .problems
                .iter()
                .filter(|p| p.contains("不在 pack/ 里"))
                .count()
                >= 2,
            "{:?}",
            report.problems
        );

        // 给了资源包目录 → 全部在远端核对通过，且摘要里 `remote_checked` 与组数对得上。
        let report = check_release_closure_with(
            &base,
            None,
            ClosureOptions {
                cdn: Some(export.as_path()),
                allow_cdn_skip: false,
            },
        )
        .expect("计划表可解析");
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert_eq!(report.remote_checked, report.groups, "{}", report.summary());
        assert_eq!(
            report.roots.len(),
            0,
            "瘦首包没有本地容器根：{}",
            report.summary()
        );

        // 反例：资源包里少一个容器 → 必须点名（远端缺件与本地缺件一样判红）。
        let victim = plan.groups[0].bundle.clone();
        std::fs::remove_file(export.join(&victim)).unwrap();
        let report = check_release_closure_with(
            &base,
            None,
            ClosureOptions {
                cdn: Some(export.as_path()),
                allow_cdn_skip: false,
            },
        )
        .expect("计划表可解析");
        assert!(!report.problems.is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// `assetc package --bundle grouped` 的**原始输出**没有多包一层 `pack/`：
    /// 此时发布根本身就是 pack 根，门必须照样能跑（CI 的合成产物走这条）。
    #[test]
    fn a_bare_grouped_output_is_treated_as_the_pack_root() {
        let base = temp_dir("bare");
        let project_dir = base.join("project");
        project(&project_dir);
        let release_dir = base.join("release");
        crate::package_grouped(&project_dir, &release_dir, false, false).expect("grouped 打包");
        let report = check_release_closure(&release_dir, None).expect("能定位计划表");
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert_eq!(report.roots, vec!["<发布根>".to_owned()]);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 传输期改名：微信只上传白名单后缀，`.rlb` 会被改成 `.rlb.txt`。容器识别按**格式**定界，
    /// 不枚举改名规则——规则只有 `code-package-rules.js` 一处真相，这里再抄一份就会漂。
    #[test]
    fn container_names_are_recognized_across_transport_renames() {
        assert_eq!(
            logical_bundle_name("core-1a2b3c4d.rlb.txt"),
            Some("core-1a2b3c4d.rlb")
        );
        assert_eq!(
            logical_bundle_name("core-1a2b3c4d.rlb"),
            Some("core-1a2b3c4d.rlb")
        );
        assert_eq!(logical_bundle_name("core.rlb.bin"), Some("core.rlb"));
        assert_eq!(logical_bundle_name("core.rlb2"), None);
        assert_eq!(logical_bundle_name("notes.txt"), None);
        assert_eq!(logical_bundle_name("MANIFEST.TXT"), None);
    }

    #[test]
    fn a_renamed_container_still_closes() {
        let base = temp_dir("renamed");
        let project_dir = base.join("project");
        project(&project_dir);
        let release_dir = base.join("release");
        crate::package_grouped(&project_dir, &release_dir, false, false).expect("grouped 打包");
        // 模拟微信代码包的上传改名：每个容器都加 `.txt`。
        for entry in std::fs::read_dir(release_dir.join("bundles"))
            .unwrap()
            .flatten()
        {
            let path = entry.path();
            std::fs::rename(&path, format!("{}.txt", path.display())).unwrap();
        }
        let report = check_release_closure(&release_dir, None).expect("能定位计划表");
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        // 报告里的根名仍是逻辑名（`<发布根>`），而不是被改名后的物理名。
        assert_eq!(report.roots, vec!["<发布根>".to_owned()]);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_real_grouped_release_is_closed() {
        let base = temp_dir("ok");
        release(&base);
        let report = check_release_closure(&base, None).expect("能定位计划表");
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert!(report.entries >= 3, "{}", report.summary());
        assert_eq!(report.roots, vec!["pack".to_owned()]);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 计划里写了一条容器里没有的路径（且 checksum 自洽）→ 必须报缺件。
    #[test]
    fn a_planned_path_missing_from_the_container_is_reported() {
        let base = temp_dir("missing-path");
        let plan = release(&base);
        let mut tampered = plan.clone();
        let group = tampered
            .groups
            .iter_mut()
            .find(|group| !group.paths.is_empty())
            .expect("至少一个组");
        group.paths.push("assets/textures/ghost.png".to_owned());
        group.paths.sort();
        group.entries += 1;
        let tampered = tampered.sealed();
        std::fs::write(
            base.join("pack/bundle_plan.json"),
            serde_json::to_vec(&tampered).unwrap(),
        )
        .unwrap();
        let report = check_release_closure(&base, None).expect("计划表本身可解析");
        assert!(
            report
                .problems
                .iter()
                .any(|problem| problem.contains("ghost.png")),
            "{:?}",
            report.problems
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 容器字节被改过 → FNV/索引必须报红，而不是安静通过。
    #[test]
    fn a_corrupted_container_is_reported() {
        let base = temp_dir("corrupt");
        let plan = release(&base);
        let bundle = plan.groups[0].bundle.clone();
        let path = base.join("pack").join(&bundle);
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(&path, &bytes).unwrap();
        let report = check_release_closure(&base, None).expect("计划表可解析");
        assert!(
            report
                .problems
                .iter()
                .any(|problem| problem.contains("索引无法解析") || problem.contains("FNV")),
            "{:?}",
            report.problems
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    // ---------------------------------------------------------------- keep（留在目录里）
    // 微信字体/音频这类文件必须按**真实路径**被宿主加载（`wx.loadFont` 只吃本地路径），
    // 所以它们不进容器。计划表记录具体路径，发布期由这里断言"真的还在目录里、也没被塞进容器"。

    fn release_with_keep(base: &Path) -> BundlePlan {
        let project_dir = base.join("project");
        project(&project_dir);
        let pack_root = base.join("pack");
        crate::package_grouped_keep(
            &project_dir,
            &pack_root,
            false,
            false,
            false,
            &["assets/".to_owned()],
        )
        .expect("grouped 打包（--keep assets/）");
        let bytes = std::fs::read(pack_root.join("bundle_plan.json")).unwrap();
        BundlePlan::parse(&bytes).expect("计划表可解析")
    }

    #[test]
    fn kept_files_must_stay_loose_and_are_recorded_in_the_plan() {
        let base = temp_dir("keep");
        let plan = release_with_keep(&base);
        let kept = plan.keep.clone().unwrap_or_default();
        assert!(!kept.is_empty(), "计划表必须记录 keep 的具体路径");
        assert!(
            kept.iter().all(|path| path.starts_with("assets/")),
            "{kept:?}"
        );
        let report = check_release_closure(&base, None).expect("计划表可解析");
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert_eq!(report.keep_checked, kept.len(), "{}", report.summary());
        assert!(report.keep_contract, "新产物必须带 keep 元数据");

        // 反例：把留在目录里的那份删掉 → 宿主会读不到（容器里也没有），必须点名。
        let victim = base.join("pack").join(&kept[0]);
        std::fs::remove_file(&victim).unwrap();
        let report = check_release_closure(&base, None).expect("计划表可解析");
        assert!(
            report
                .problems
                .iter()
                .any(|problem| problem.contains("keep 条目不在目录里")),
            "{:?}",
            report.problems
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// keep 的路径同时又出现在某个组的计划路径里 → 解析期就该拒绝（双份就是白占配额）。
    #[test]
    fn a_keep_path_that_is_also_planned_into_a_container_is_rejected() {
        let base = temp_dir("keep-conflict");
        let plan = release(&base);
        let mut bad = plan.clone();
        bad.keep = Some(vec![plan.groups[0].paths[0].clone()]);
        let bytes = serde_json::to_vec(&bad.sealed()).unwrap();
        let error = BundlePlan::parse(&bytes).expect_err("必须拒绝");
        assert!(error.contains("同时属于某个容器组"), "{error}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 手工容器：`a.txt` 在索引里、计划里没有任何组声明它，但 keep 声明了它
    /// （容器允许比计划多——这条断言就是防"多出来的那份恰好是 keep 文件"）。
    #[test]
    fn a_kept_file_that_is_also_inside_a_container_is_reported() {
        let base = temp_dir("keep-in-container");
        let pack = base.join("pack");
        std::fs::create_dir_all(pack.join("bundles")).unwrap();
        let mut builder =
            rustlua_infra::bundle::BundleBuilder::new(rustlua_infra::bundle::DEFAULT_BLOCK_SIZE);
        builder.add("a.txt", b"aaa".to_vec()).unwrap();
        let built = builder.build().unwrap();
        std::fs::write(pack.join("bundles/a.rlb"), &built.bytes).unwrap();
        std::fs::write(pack.join("a.txt"), b"aaa").unwrap();
        let plan = BundlePlan {
            schema: 1,
            format: rustlua_infra::bundle::plan::PLAN_FORMAT.to_owned(),
            block_size: rustlua_infra::bundle::DEFAULT_BLOCK_SIZE,
            groups: vec![PlannedGroup {
                name: "core".to_owned(),
                unit: "main".to_owned(),
                bundle: "bundles/a.rlb".to_owned(),
                hash: format!("{:016x}", checksum(&built.bytes)),
                entries: 1,
                raw_bytes: 3,
                bundle_bytes: built.bytes.len() as u64,
                paths: Vec::new(),
            }],
            checksum: String::new(),
            keep: Some(vec!["a.txt".to_owned()]),
        }
        .sealed();
        std::fs::write(
            pack.join("bundle_plan.json"),
            serde_json::to_vec(&plan).unwrap(),
        )
        .unwrap();
        let report = check_release_closure(&base, None).expect("计划表可解析");
        assert!(
            report
                .problems
                .iter()
                .any(|problem| problem.contains("同时被装进了容器")),
            "{:?}",
            report.problems
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    // ---------------------------------------------------------------- cdn（远端分发）
    // `-AssetPack remote` 的容器不在发布根里，而在 CDN 镜像目录里。此前这些组被**静默跳过**
    // （只数个数），于是"远端少了一个容器"在发布日志里与"通过"长得一模一样。

    /// 把非 core 组改成 cdn，并把它的容器搬到 `base/cdn/bundles/`。
    fn release_with_cdn(base: &Path) -> (BundlePlan, PathBuf) {
        let plan = release(base);
        let mut moved = plan.clone();
        let index = moved
            .groups
            .iter()
            .position(|group| group.name != "core")
            .unwrap_or(0);
        moved.groups[index].unit = "cdn".to_owned();
        let moved = moved.sealed();
        std::fs::write(
            base.join("pack/bundle_plan.json"),
            serde_json::to_vec(&moved).unwrap(),
        )
        .unwrap();
        let bundle = moved.groups[index].bundle.clone();
        let cdn = base.join("cdn");
        std::fs::create_dir_all(cdn.join("bundles")).unwrap();
        std::fs::rename(base.join("pack").join(&bundle), cdn.join(&bundle)).unwrap();
        (moved, cdn)
    }

    #[test]
    fn cdn_groups_without_a_remote_dir_are_reported_not_silently_skipped() {
        let base = temp_dir("cdn-unchecked");
        release_with_cdn(&base);
        let report = check_release_closure(&base, None).expect("计划表可解析");
        assert_eq!(report.cdn_checked, 0);
        assert_eq!(report.cdn_groups.len(), 1);
        assert!(
            report
                .problems
                .iter()
                .any(|problem| problem.contains("未检查")),
            "没给远端目录时必须报「未检查」，而不是当成通过：{:?}",
            report.problems
        );

        // 显式声明"这次不查"（CDN 产物不在本机）：允许通过，但摘要里跳过数必须可见。
        let report = check_release_closure_with(
            &base,
            None,
            ClosureOptions {
                cdn: None,
                allow_cdn_skip: true,
            },
        )
        .expect("计划表可解析");
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert!(
            report.summary().contains("cdn_skipped=1"),
            "{}",
            report.summary()
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn cdn_groups_are_checked_against_the_remote_dir() {
        let base = temp_dir("cdn-checked");
        let (plan, cdn) = release_with_cdn(&base);
        let options = ClosureOptions {
            cdn: Some(cdn.as_path()),
            allow_cdn_skip: false,
        };
        let report = check_release_closure_with(&base, None, options).expect("计划表可解析");
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert_eq!(report.cdn_checked, 1, "{}", report.summary());

        // 反例：把远端容器删掉 → 必须点名 `<cdn>` 缺件（而不是"没找到就跳过"）。
        let bundle = plan
            .groups
            .iter()
            .find(|group| group.unit == "cdn")
            .expect("cdn 组")
            .bundle
            .clone();
        std::fs::remove_file(cdn.join(&bundle)).unwrap();
        let report = check_release_closure_with(&base, None, options).expect("计划表可解析");
        assert!(
            report
                .problems
                .iter()
                .any(|problem| problem.contains("<cdn>")),
            "{:?}",
            report.problems
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// `cdn` 组的字节**就在包里**时必须就地核对（WebGL/PC 形态：没有远端那一跳，
    /// 容器原样躺在 `pack/bundles/`）。以前这里按"组名是 cdn"一律要求 `--cdn`，
    /// 于是真实 WebGL 产物会被判红——而"包里有、却不查"同样是错的。
    #[test]
    fn cdn_groups_inside_the_package_are_checked_in_place() {
        let base = temp_dir("cdn-local");
        let plan = release(&base);
        let mut moved = plan.clone();
        let index = moved
            .groups
            .iter()
            .position(|group| group.name != "core")
            .unwrap_or(0);
        moved.groups[index].unit = "cdn".to_owned();
        let moved = moved.sealed();
        std::fs::write(
            base.join("pack/bundle_plan.json"),
            serde_json::to_vec(&moved).unwrap(),
        )
        .unwrap();
        // 容器**没有**搬走：它还在 pack/bundles/ 里。
        let report = check_release_closure(&base, None).expect("计划表可解析");
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert_eq!(report.cdn_local, 1, "{}", report.summary());
        assert_eq!(report.cdn_checked, 0);
        assert!(
            report.summary().contains("cdn_skipped=0") && report.summary().contains("cdn_local=1"),
            "{}",
            report.summary()
        );

        // 反例：本地那份被改坏 → 就在本地判红（不因为组名是 cdn 就放过）。
        // 改的是**魔数**（索引层面的损坏）；闭包只解析索引、不读条目内容，
        // 条目字节对不对由运行期 `read()` 的逐条 FNV 兜（见 docs/67 §4.8）。
        let bundle = moved
            .groups
            .iter()
            .find(|group| group.unit == "cdn")
            .expect("cdn 组")
            .bundle
            .clone();
        let file = base.join("pack").join(&bundle);
        let mut bytes = std::fs::read(&file).unwrap();
        bytes[0] ^= 0xFF;
        std::fs::write(&file, &bytes).unwrap();
        let report = check_release_closure(&base, None).expect("计划表可解析");
        assert_eq!(report.cdn_local, 0, "判定为坏包的组不算核对完成");
        assert!(!report.problems.is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 多根产物里，组声明的 unit 与它容器实际所在的根不一致 → 必须点名。
    #[test]
    fn a_container_in_the_wrong_root_is_reported() {
        let base = temp_dir("wrong-root");
        let plan = release(&base);
        // 出现 pack-sub/ 即切到多根形态（微信 launcher）。
        std::fs::create_dir_all(base.join("pack-sub")).unwrap();
        let mut moved = plan.clone();
        let index = moved
            .groups
            .iter()
            .position(|group| group.name != "core")
            .unwrap_or(0);
        moved.groups[index].unit = "sub".to_owned();
        let moved = moved.sealed();
        std::fs::write(
            base.join("pack/bundle_plan.json"),
            serde_json::to_vec(&moved).unwrap(),
        )
        .unwrap();
        let report = check_release_closure(&base, None).expect("计划表可解析");
        assert!(
            report
                .problems
                .iter()
                .any(|problem| problem.contains("pack-sub")),
            "{:?}",
            report.problems
        );
        let _ = std::fs::remove_dir_all(&base);
    }
}
