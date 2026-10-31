//! P1-3b：断链审计与「同名唯一命中」一键改指（shell 侧；面板只发命令）。
//!
//! 为什么需要独立一份"读全部文档"的审计：Inspector 的 ⚠ 只看**当前选中的实体**，
//! 而断链最常见的来源恰恰是"没打开过的那份场景/预制体"。资源搬走后剩下的引用
//! 在运行时只是画不出来的白块（`MissingTexture`），没有任何地方会把它们汇总出来。
//!
//! 三条口径（与面板模块头一致）：
//! 1. **文档集合 = 改指会改写的那一批**（[`editor::project_scene_documents`]），
//!    当前打开的文档额外用**内存版**（含未保存编辑）参与统计，磁盘上那一份不重复计。
//! 2. **判据与 Inspector 同源**：`rows_for_entity` 的 `path_rule` + 资源索引查不到，
//!    内置哨兵（`__white__` 等）跳过。资源索引为空时直接不出结果——"索引还没建好"
//!    不是"全工程都是断链"。
//! 3. **改指只认同名唯一命中**，其余情况把原因留给用户看，不猜路径。
//!
//! 扫描读的是 [`crate::actions::project_index::ProjectIndex`]——与全工程搜索同一份索引实现，
//! 所以"谁引用了这条路径"和"这条路径存不存在"两边永远给同一个答案。

use crate::EditorApp;
use crate::actions::project_index::{IndexedMetadata, ProjectIndex};
use crate::panels::broken_refs::{BrokenHit, BrokenReference};
use editor::CommandResult;
use rustlua_scene::prefab::{
    PREFAB_NESTED_STAMP_META_PREFIX, PREFAB_SOURCE_META_KEY, PREFAB_VARIANT_BASE_META_KEY,
};
use std::collections::{BTreeMap, BTreeSet};

impl EditorApp {
    /// 扫描全工程，把结果写进断链检查窗（只读，不改任何文档）。
    ///
    /// 代价：逐份解析 `.scene`/`.prefab`（Debug 下几百份文档是百毫秒级），所以只由
    /// 面板上的显式按钮触发；Play 期间照样能扫，因为它不写文档。
    pub(crate) fn scan_broken_references(&mut self) -> CommandResult {
        if !self.project_loaded {
            self.broken_refs.clear();
            self.status = "断链检查需要先打开工程".to_owned();
            return CommandResult::Rejected(self.status.clone());
        }
        // 资源索引是"存在性"的唯一事实源；没建好时不做判断（宁可不报，也不制造一屏假断链）。
        let assets: BTreeSet<String> = self
            .resources
            .entries()
            .iter()
            .map(editor::ResourceEntry::path_string)
            .collect();
        if assets.is_empty() {
            self.broken_refs.clear();
            self.status = "资源索引还没建立，先刷新资源再检查断链".to_owned();
            return CommandResult::NoOp;
        }
        // 与全工程搜索**同一份索引实现**（`ProjectIndex`）：文档集合、`.prefab` 走哪个解析器、
        // `rows_for_entity` 的路径规则判据都只有一份，不会出现"搜索说这个资源没人用、
        // 断链检查却列出它"这类自相矛盾。
        let index = ProjectIndex::scan(
            &self.project.root,
            self.document.current_scene.as_ref(),
            assets.iter().cloned().collect(),
        );
        let mut hits: BTreeMap<String, BrokenHit> = BTreeMap::new();
        // 1) 组件字段：判据与 Inspector 的 ⚠ 同源（`mark_missing_assets` 那四条）。
        for field in index.fields.iter().filter(|field| field.path_rule) {
            let value = field.value.trim().replace('\\', "/");
            if value.is_empty()
                || assets.contains(&value)
                || editor::is_builtin_asset_reference(&value)
            {
                continue;
            }
            hits.entry(value).or_default().push(format!(
                "{} ▸ E#{} ▸ {}",
                field.document, field.entity.0, field.path
            ));
        }
        // 2) 谱系元数据：没有 Inspector 行，但改指同样会改写它们（口径见模块头第 2 条）。
        for metadata in &index.metadata {
            let Some((value, consumer)) = metadata_reference(metadata) else {
                continue;
            };
            if !assets.contains(&value) {
                hits.entry(value).or_default().push(consumer);
            }
        }
        let scanned = index.scanned_documents;
        let rows = broken_rows(hits, &assets);
        let found = rows.len();
        self.broken_refs.rows = rows;
        self.broken_refs.scanned_documents = scanned;
        self.broken_refs.unreadable = index.unreadable.clone();
        self.broken_refs.scanned = true;
        self.broken_refs.message = if scanned == 0 {
            "工程里没有可扫描的场景/Prefab 文档。".to_owned()
        } else {
            String::new()
        };
        self.status = if found == 0 {
            format!("断链检查：读过 {scanned} 份文档，没有断链")
        } else {
            format!("断链检查：读过 {scanned} 份文档，发现 {found} 条断链")
        };
        CommandResult::Applied
    }

    /// 改指一条断掉的引用：其它文档直接写盘，当前文档走一条 Undo。
    ///
    /// 与 `perform_resource_move` 同一条链路（`rewrite_project_references` + `history.execute`），
    /// 差别只在**文件不搬**：这里只修引用。播放模式下拒绝——运行态镜像与编辑文档会分叉。
    pub(crate) fn repair_broken_reference(
        &mut self,
        from: &str,
        to: &str,
    ) -> Result<String, String> {
        if self.is_playing() {
            return Err("播放模式下不能改写引用，请先点击停止".to_owned());
        }
        let root = self.project.root.clone();
        let from_relative = editor::normalize_relative(std::path::Path::new(from))
            .ok_or_else(|| "引用路径必须是工程内相对路径".to_owned())?;
        let to_relative = editor::normalize_relative(std::path::Path::new(to))
            .ok_or_else(|| "目标路径必须是工程内相对路径".to_owned())?;
        let from_text = from_relative.to_string_lossy().replace('\\', "/");
        let to_text = to_relative.to_string_lossy().replace('\\', "/");
        if from_text == to_text {
            return Err("源与目标是同一个路径".to_owned());
        }
        if !root.join(&to_relative).is_file() {
            return Err(format!("目标资源不存在：{to_text}"));
        }
        let open_document = self
            .document
            .current_scene
            .as_ref()
            .and_then(|document| document.path.clone());
        // `skip` = 当前打开的文档：它在内存里改（见下），磁盘上那一份不许被覆盖——
        // 否则用户未保存的编辑会被"改指"连带丢掉。
        let report = editor::rewrite_project_references(
            &root,
            &from_text,
            &to_text,
            open_document.as_deref(),
        );
        // 当前打开的文档：内存改写 + 一条 Undo（不写盘，保住未保存的编辑）。
        let mut open_document_components = 0;
        if let Some(scene) = self.document.current_scene.as_mut() {
            let label = format!("断链改指：{from_text} → {to_text}");
            let counter = &mut open_document_components;
            let _ = self
                .history
                .execute(scene, label, editor::CommandKind::Rename, |scene| {
                    let changed =
                        editor::rewrite_scene_references(&mut scene.scene, &from_text, &to_text);
                    if changed > 0 {
                        scene.mark_dirty();
                    }
                    *counter = changed;
                    Ok::<(), ()>(())
                });
        }
        // 清单是"这一次扫描"的结果：改完把这一行摘掉，不自动重扫（重扫的代价不该由一次点击付）。
        self.broken_refs.rows.retain(|row| row.value != from_text);
        self.broken_refs.message = format!(
            "已把 {from_text} 改指到 {to_text}（改写 {} 个文档 / {} 处引用）\
             ；清单是上次扫描的结果，需要最新状态请再点一次「扫描全工程」。",
            report.rewritten_files.len(),
            report.rewritten_references
        );
        if !report.failures.is_empty() {
            self.broken_refs.message.push_str(&format!(
                " 这些文档解析/写回失败，保持原样：{}",
                report.failures.join("、")
            ));
        }
        Ok(format!(
            "已改指 {from_text} → {to_text}（磁盘 {} 个文档 / 当前文档 {open_document_components} 处，可 Undo）",
            report.rewritten_files.len()
        ))
    }
}

/// 一条**谱系元数据**引用 → `(被指的路径, 消费者标签)`。
///
/// 这些引用不挂在组件字段上（没有 Inspector 行），但资源移动/改指同样会改写它们
/// （见 `rustlua_editor_core::resource_ops`）。审计必须同口径看得见，否则会出现
/// "改指漏了它、扫描还说没断链"——模块头第 1 条要防的正是这种自相矛盾。
///
/// 三种来源的"引用藏在哪"不同：实体元数据看 `prefab.source` 的**值**、变体基础看
/// 文档元数据 `prefab.variant_base` 的**值**，而嵌套签名把来源写在**键**的后半段。
///
/// 只认**工程内路径**（与 `ProjectPath` 字段同口径：`assets/` 开头）；普通元数据文本
/// （名字、签名、覆盖值）不参与断链判断。
fn metadata_reference(metadata: &IndexedMetadata) -> Option<(String, String)> {
    let document = metadata.document.as_str();
    let (value, consumer) = match metadata.entity {
        Some(entity) if metadata.key == PREFAB_SOURCE_META_KEY => (
            metadata.value.clone(),
            format!("{document} ▸ E#{} ▸ prefab.source", entity.0),
        ),
        None if metadata.key == PREFAB_VARIANT_BASE_META_KEY => (
            metadata.value.clone(),
            format!("{document} ▸ 谱系 ▸ prefab.variant_base"),
        ),
        None => {
            let source = metadata.key.strip_prefix(PREFAB_NESTED_STAMP_META_PREFIX)?;
            (
                source.to_owned(),
                format!("{document} ▸ 谱系 ▸ prefab.nested_stamp"),
            )
        }
        Some(_) => return None,
    };
    let value = value.trim().replace('\\', "/");
    if value.starts_with("assets/") {
        Some((value, consumer))
    } else {
        None
    }
}

/// 聚合结果 → 清单行（并算出每条路径的**同名候选**）。
fn broken_rows(
    hits: BTreeMap<String, BrokenHit>,
    assets: &BTreeSet<String>,
) -> Vec<BrokenReference> {
    hits.into_iter()
        // `value` 放最后：前面几处还在借用它（结构体字段的初始化按书写顺序求值）。
        .map(|(value, hit)| BrokenReference {
            candidates: same_name_candidates(&value, assets),
            consumer_count: hit.count,
            consumers: hit.consumers,
            value,
        })
        .collect()
}

/// 资源索引的最后一段（文件名）与 `broken` 相同、但路径不是它自己的那些资产。
///
/// 只认**完全相同**的文件名（大小写不敏感——Windows 文件系统本来就不区分）：不做相似度、
/// 不做"看起来像"的猜测。命中多份时调用方不给按钮。
fn same_name_candidates(broken: &str, assets: &BTreeSet<String>) -> Vec<String> {
    let needle = file_name_of(broken).to_ascii_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = assets
        .iter()
        .filter(|asset| {
            asset.as_str() != broken && file_name_of(asset).to_ascii_lowercase() == needle
        })
        .cloned()
        .collect();
    out.sort();
    out
}

/// 工程内相对路径（正斜杠）的最后一段。
fn file_name_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_match_the_file_name_only() {
        let assets: BTreeSet<String> = [
            "assets/other/player.png",
            "assets/hero/player.png",
            "assets/other/hero.png",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        // 同名两份 → 两个候选（调用方据此拒绝给按钮）。
        assert_eq!(
            same_name_candidates("assets/missing/player.png", &assets),
            vec!["assets/hero/player.png", "assets/other/player.png"]
        );
        // 大小写不同也算同名（Windows 文件系统本来就不区分）。
        assert_eq!(
            same_name_candidates("assets/missing/PLAYER.PNG", &assets),
            vec!["assets/hero/player.png", "assets/other/player.png"]
        );
        // 断掉的路径自己（哪怕仍在索引里）不能当自己的候选。
        assert_eq!(
            same_name_candidates("assets/hero/player.png", &assets),
            vec!["assets/other/player.png".to_owned()],
            "同名候选要排掉引用自己指的那份"
        );
        // 只剩一份同名：那个"断掉的路径自己"不能算候选。
        let only: BTreeSet<String> = ["assets/a/player.png".to_owned()].into_iter().collect();
        assert!(same_name_candidates("assets/a/player.png", &only).is_empty());
        // 没有目录分隔的裸文件名也能取到候选。
        assert_eq!(
            same_name_candidates("player.png", &assets),
            vec!["assets/hero/player.png", "assets/other/player.png"]
        );
    }
}
