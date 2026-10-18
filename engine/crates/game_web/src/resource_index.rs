use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(serde::Deserialize)]
struct DependencyIndex {
    format: u32,
    edges: BTreeMap<String, Vec<String>>,
    opaque: Vec<String>,
}

pub struct ResourceSelection {
    pub paths: Vec<String>,
    pub full_fallback: bool,
    pub fallback_reason: Option<String>,
}

/// 从 `manifest.txt` 里挑出容器条目（`bundles/<name>.rlb`，名字升序）。
///
/// 为什么从清单里取而不是读 `bundle_plan.json`：清单是**每个宿主都已经拿在手上**的那份东西
/// （浏览器 `fetch` 过、微信适配层读过），这样不新增网络往返，也不需要 JS 懂容器格式。
/// 返回空 = 目录形态（宿主走原来的逐条目预载，行为与本改动之前逐字节一致）。
pub fn bundle_files(manifest: &str) -> Vec<String> {
    let mut out: Vec<String> = manifest
        .lines()
        .skip(1)
        .filter_map(|line| {
            let path = line.split('\t').next()?.trim();
            (path.starts_with("bundles/") && path.to_ascii_lowercase().ends_with(".rlb"))
                .then(|| path.to_owned())
        })
        .collect();
    out.sort();
    out
}

/// 选首屏要预载的资源。
///
/// ## 为什么**永远**返回整个 manifest（2026-10-17 的现场教训）
///
/// 这条函数一度返回"静态闭包"（可达性遍历的结果），那一刻起浏览器上就出现了一整族静默失败：
/// **全部 16 条音频不播**、**四季地表图集读不到 ⇒ 白块**、**6 张场景进不去**——实测被挡在
/// 预载之外的 560 个文件（audio 16 / art 536 / scene 6 / font 1 / dependencies.json 1），
/// 而**没有任何一条边指向它们**。原因是这些资源根本不是静态可证的：
///
/// * 音频由宿主**按名字**取回（`audio.sfx("ui_click")` 从不经同步 vfs，`assetc` 的扫描器
///   因此刻意不为它产边）；
/// * 四季地表名是 `string.format(pattern, season)` **拼**出来的；
/// * 场景是 `scene.load(name)` 里传来的**动态字符串**；
/// * 还有按目录常量拼路径、按数据表取名等成片写法。
///
/// 静态图能证明"这些一定读"，**证明不了"只读这些"**。运行期那条同步 vfs 是**失败即降级**
/// （音频不播、图变白块、场景进不去，脚本层面不报错），所以预载集合一旦收窄，症状就落在
/// 最不容易归因的地方。结论：**预载是"保证同步读不失败"的机制，不是依赖分析的产物**。
///
/// 遍历仍然跑（不短路），因为它的读数有价值：`fallback_reason` 会报出"静态图证明了什么、
/// 证明了几个"，那也是"闭包为什么不能当预载集合用"的可复现证据。
pub fn select_resources(
    index_text: &str,
    manifest_paths: &[String],
    roots: &[String],
) -> Result<ResourceSelection, String> {
    let index: DependencyIndex =
        serde_json::from_str(index_text).map_err(|error| format!("parse: {error}"))?;
    if index.format != 1 {
        return Err(format!("unsupported format {}", index.format));
    }

    let manifest: BTreeSet<&str> = manifest_paths.iter().map(String::as_str).collect();
    let opaque: BTreeSet<&str> = index.opaque.iter().map(String::as_str).collect();
    let mut selected = BTreeSet::new();
    let mut pending: VecDeque<String> = roots.iter().cloned().collect();
    let mut static_note = None;
    while let Some(path) = pending.pop_front() {
        if !selected.insert(path.clone()) {
            continue;
        }
        if !manifest.contains(path.as_str()) || opaque.contains(path.as_str()) {
            static_note = Some(if opaque.contains(path.as_str()) {
                format!("opaque reference at {path}")
            } else {
                format!("resource absent from manifest: {path}")
            });
            break;
        }
        if let Some(dependencies) = index.edges.get(&path) {
            pending.extend(dependencies.iter().cloned());
        }
    }

    let reason = match static_note {
        Some(note) => format!(
            "静态闭包不可证（{note}）；且按名取回的资源（音频/拼名贴图/动态场景）本就不产边 ⇒ 预载全量 {} 项",
            manifest_paths.len()
        ),
        None => format!(
            "静态闭包只覆盖 {} / {} 项：按名取回的资源（音频/拼名贴图/动态场景）不产边、无法证明完整 ⇒ 预载全量",
            selected.len(),
            manifest_paths.len()
        ),
    };
    Ok(full_selection(manifest_paths, reason))
}

fn full_selection(manifest_paths: &[String], reason: String) -> ResourceSelection {
    ResourceSelection {
        paths: manifest_paths.to_vec(),
        full_fallback: true,
        fallback_reason: Some(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    /// **预载必须是全量**，即使静态图能证明一个更小的闭包。
    ///
    /// 这条用例是 2026-10-17 那个现场转正：`scripts/main.lua` 静态可达的只有 helper 与场景，
    /// 但包里还有**按名取回**的音频与**拼名**拿到的贴图——收窄到闭包的那一刻，它们在浏览器上
    /// 全部读不到（音频不播、地表图变白块、场景进不去），而脚本层一条错都不报。
    #[test]
    fn preload_is_always_the_whole_manifest_even_when_the_static_graph_is_smaller() {
        let manifest = paths(&[
            "scripts/main.lua",
            "scripts/helper.lua",
            "scenes/main.scene",
            "assets/ui/title.png",
            "assets/audio/unused.ogg",
            "assets/art/tilesets/manor/manor_ground_spring.png",
        ]);
        let index = r#"{"format":1,"edges":{"scripts/main.lua":["scripts/helper.lua","scenes/main.scene"],"scenes/main.scene":["assets/ui/title.png"]},"opaque":[]}"#;
        let selected = select_resources(index, &manifest, &paths(&["scripts/main.lua"])).unwrap();
        assert_eq!(
            selected.paths, manifest,
            "按名取回/拼名的资源没有任何边指向，收窄就会漏"
        );
        assert!(
            selected.full_fallback,
            "全量预载要自报成兜底（full_fallback）而不是闭包"
        );
        let reason = selected.fallback_reason.expect("必须给出原因");
        assert!(
            reason.contains("3 / 6") || reason.contains("静态闭包"),
            "原因要说清静态图只证明了几个：{reason}"
        );
    }

    #[test]
    fn opaque_or_missing_reachable_resource_also_preloads_everything() {
        let manifest = paths(&["scripts/main.lua", "assets/a.png", "assets/b.png"]);
        let index = r#"{"format":1,"edges":{"scripts/main.lua":["assets/a.png"]},"opaque":["scripts/main.lua"]}"#;
        let selected = select_resources(index, &manifest, &paths(&["scripts/main.lua"])).unwrap();
        assert_eq!(selected.paths, manifest);
        assert!(selected.full_fallback);
        assert!(
            selected
                .fallback_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("opaque reference at scripts/main.lua")),
            "原因里要留下哪一条引用不可证：{:?}",
            selected.fallback_reason
        );

        let index =
            r#"{"format":1,"edges":{"scripts/main.lua":["assets/missing.png"]},"opaque":[]}"#;
        let selected = select_resources(index, &manifest, &paths(&["scripts/main.lua"])).unwrap();
        assert_eq!(selected.paths, manifest);
        assert!(selected.full_fallback);
        assert!(
            selected.fallback_reason.as_deref().is_some_and(
                |reason| reason.contains("resource absent from manifest: assets/missing.png")
            ),
            "原因里要留下缺失路径：{:?}",
            selected.fallback_reason
        );
    }

    #[test]
    fn rejects_unknown_index_versions() {
        assert!(select_resources(r#"{"format":2,"edges":{},"opaque":[]}"#, &[], &[]).is_err());
    }

    #[test]
    fn bundle_files_picks_only_rlb_entries() {
        let manifest = "assetc_manifest=1\tassetc-package-2\n\
             bundles/all-88768371.rlb\t3652689\t0987a5d488768371\n\
             package-report.json\t256\tfcf078d7d80992e3\n\
             project.toml\t4479\tf69733573fc1ae6d\n";
        assert_eq!(
            bundle_files(manifest),
            vec!["bundles/all-88768371.rlb".to_owned()],
            "只认 bundles/*.rlb；骨架与报告不算"
        );
        // 目录形态（没有 bundles/ 行）必须返回空 → 宿主走原来的逐条目预载
        let loose = "assetc_manifest=1\tassetc-package-2\nscripts/main.lua\t42\t00\n";
        assert!(bundle_files(loose).is_empty());
        // 多包按名字升序（挂载顺序决定查找顺序，必须确定性）
        let multi = "assetc_manifest=1\tx\nbundles/b.rlb\t1\t00\nbundles/a.rlb\t1\t00\n";
        assert_eq!(
            bundle_files(multi),
            vec!["bundles/a.rlb".to_owned(), "bundles/b.rlb".to_owned()]
        );
    }
}
