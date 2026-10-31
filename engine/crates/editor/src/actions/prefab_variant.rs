//! Prefab 变体：快照 + 谱系，v1 只允许字段级差异。
//!
//! 三条口径决定了这里的形状：
//!
//! 1. **快照是运行时唯一事实源**。变体文件就是一份完整 Prefab，运行时不解析谱系；
//!    谱系（`prefab.variant_base` / stamp / `prefab.override.*`）只给编辑器用。
//! 2. **基础动过、没同步，就不许保存**。否则"基础的新值"会被 diff 成"变体自己的覆盖"，
//!    把上游改动永久钉在变体里——这正是 stamp 存在的理由。
//! 3. **v1 只做字段级**。覆盖语法表达不了增删节点、改层级、改嵌套来源，遇到就明确拒绝，
//!    并告诉作者先同步再改字段。
//!
//! 保存时覆盖记录由「基础 vs 当前快照」重新 diff 得出；从基础同步时，则用磁盘上的
//! 上次保存版本识别**未保存的字段编辑**，把它们并进记录后再套到新基础上——这样既不会
//! 丢未保存修改，也不会把基础的变化误收成覆盖。

use super::prefab_rebuild::prefab_override_value;
use super::*;
use editor::file_watcher::FileSignature;
use rustlua_assets::resource_db::AssetId;
use rustlua_scene::prefab::{
    PREFAB_OVERRIDE_META_PREFIX, PREFAB_VARIANT_BASE_META_KEY, PREFAB_VARIANT_BASE_STAMP_META_KEY,
    PrefabInstance,
};
use rustlua_scene::scene::Scene;
use std::collections::BTreeMap;

impl EditorApp {
    /// 从资源面板的 `.prefab` 创建变体：快照等于基础，覆盖记录为空，谱系记下基础签名。
    pub(crate) fn create_prefab_variant(&mut self, base: &Path) -> Result<String, String> {
        if self.is_playing() {
            return Err("播放模式下不能创建变体，请先点击停止".to_owned());
        }
        let root = strip_verbatim_prefix(&self.project.root).clone();
        let base_relative = editor::normalize_relative(base)
            .ok_or_else(|| format!("基础预制体必须是工程内相对路径：{}", base.display()))?;
        if base_relative.extension().and_then(|ext| ext.to_str()) != Some("prefab") {
            return Err("只有 .prefab 资源能创建变体".to_owned());
        }
        let base_text = relative_text(&base_relative);
        let base_absolute = root.join(&base_relative);
        let mut prefab =
            Prefab::load(&base_absolute).map_err(|error| format!("加载基础预制体失败：{error}"))?;
        let stamp = FileSignature::stamp_of(&base_absolute)
            .ok_or_else(|| format!("无法读取基础预制体签名：{base_text}"))?;
        let target_absolute = unique_variant_path(&base_absolute);
        let target_file = target_absolute
            .file_name()
            .map(|name| base_relative.with_file_name(name))
            .ok_or_else(|| format!("无法确定变体文件名：{}", target_absolute.display()))?;
        let target_text = relative_text(&target_file);
        let stem = target_file
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "variant".to_owned());

        prefab.asset_id = AssetId::from_seed(&target_text);
        prefab.name = stem;
        prefab
            .set_variant_base(Some(&base_text))
            .map_err(|error| error.to_string())?;
        prefab
            .set_variant_base_stamp(Some(&stamp))
            .map_err(|error| error.to_string())?;
        prefab
            .set_overrides(&BTreeMap::new())
            .map_err(|error| error.to_string())?;
        prefab
            .save(&target_absolute)
            .map_err(|error| format!("写入变体失败：{error}"))?;

        self.selected_resource = Some(target_file.clone());
        self.refresh_resources();
        Ok(format!("已创建变体 {target_text}（基础 {base_text}）"))
    }

    /// 「从基础同步」：新快照 = 新基础 + 变体自己的覆盖记录（含未保存的字段编辑）。
    ///
    /// 结果落在当前文档里并进一条 Undo，不直接写盘——作者还能在保存前继续调整。
    pub(crate) fn sync_prefab_variant_from_base(&mut self) -> Result<(), String> {
        if self.is_playing() {
            return Err("播放模式下不能同步变体，请先点击停止".to_owned());
        }
        let root = strip_verbatim_prefix(&self.project.root).clone();
        let Some(document) = self.document.current_scene.as_ref() else {
            return Err("没有打开的文档".to_owned());
        };
        let Some(base_relative) = document
            .scene
            .metadata
            .get(PREFAB_VARIANT_BASE_META_KEY)
            .cloned()
        else {
            return Err("当前文档不是预制体变体".to_owned());
        };
        let Some(document_path) = document.path.clone() else {
            return Err("变体缺少文件路径，无法识别上次保存的版本".to_owned());
        };
        let base_absolute = root.join(&base_relative);
        let base =
            Prefab::load(&base_absolute).map_err(|error| format!("加载基础预制体失败：{error}"))?;
        let current_stamp = FileSignature::stamp_of(&base_absolute)
            .ok_or_else(|| format!("无法读取基础预制体签名：{base_relative}"))?;
        let current = document
            .prefab_asset()
            .map_err(|error| format!("读取当前变体失败：{error}"))?;
        // 磁盘上是**上次保存**的变体：用它区分"未保存的字段编辑"和"基础自己变了"。
        let last_saved = Prefab::load(&document_path)
            .map_err(|error| format!("无法读取变体的上次保存版本（请先保存）：{error}"))?;

        if let Some(difference) = structural_difference(&last_saved, &current) {
            return Err(format!(
                "变体有未保存的结构改动（{difference}），v1 只支持字段级差异；请撤销这些改动后再同步"
            ));
        }
        if let Some(difference) = structural_difference(&base, &current) {
            return Err(format!(
                "基础的结构已经变化（{difference}），v1 只支持字段级同步；请改用独立预制体"
            ));
        }

        // 记录 = 上次保存的覆盖 + 未保存的字段编辑（后者以当前快照为准）。
        let mut overrides = last_saved
            .overrides()
            .map_err(|error| format!("变体覆盖记录损坏：{error}"))?;
        for (id, entity) in &current.entities {
            let Some(saved) = last_saved.entities.get(id) else {
                continue;
            };
            for field in diff_prefab_entity(saved, entity) {
                let value = prefab_override_value(entity, &field)?;
                overrides.insert((*id, field), value);
            }
        }

        let mut synced = base.clone();
        synced.asset_id = current.asset_id;
        synced.name = current.name.clone();
        synced
            .set_variant_base(Some(&base_relative))
            .map_err(|error| error.to_string())?;
        synced
            .set_variant_base_stamp(Some(&current_stamp))
            .map_err(|error| error.to_string())?;
        synced
            .set_overrides(&overrides)
            .map_err(|error| format!("变体覆盖无法套到新基础：{error}"))?;

        let mut rebaked = Scene::new(synced.name.clone());
        rebaked.entities = synced.entities.clone();
        let identity = rebaked.entities.keys().map(|id| (*id, *id)).collect();
        PrefabInstance {
            prefab_id: synced.asset_id,
            instance_root: synced.root,
            source_to_instance: identity,
            overrides: overrides.clone(),
        }
        .apply_overrides(&mut rebaked)
        .map_err(|error| format!("变体覆盖无法套到新基础：{error}"))?;

        let entities = rebaked.entities;
        let lineage: BTreeMap<String, String> = synced
            .metadata
            .iter()
            .filter(|(key, _)| lineage_key(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let Some(document) = self.document.current_scene.as_mut() else {
            return Err("没有打开的文档".to_owned());
        };
        let label = format!("从基础同步变体：{base_relative}");
        self.history
            .execute(
                document,
                label.as_str(),
                CommandKind::Property,
                move |document| {
                    document.scene.entities = entities;
                    document.scene.metadata.retain(|key, _| !lineage_key(key));
                    document.scene.metadata.extend(lineage);
                    document.mark_dirty();
                    Ok::<(), ()>(())
                },
            )
            .map_err(|()| "同步变体失败".to_owned())?;
        self.on_document_changed(DocumentChange::PrefabApply);
        self.status = format!(
            "已从基础同步变体 {base_relative}（保留 {} 个字段覆盖），Ctrl+S 保存",
            overrides.len()
        );
        Ok(())
    }
}

/// 变体源不支持从实例写回：实例只是快照的一个使用方，写回会改快照却不重算覆盖记录，
/// 谱系会立刻变成过期数据。变体要在文档里改（保存时自动重算覆盖）。
pub(crate) fn variant_write_back_refusal(prefab: &Prefab) -> Option<String> {
    prefab.variant_base().map(|base| {
        format!(
            "{base} 是预制体变体，不能从场景实例写回；请打开变体文件编辑（保存时会重新记录字段覆盖）"
        )
    })
}

/// 保存变体文档前的谱系准备：把当前快照相对基础的真实差异写成覆盖记录。
///
/// 由 [`EditorApp::save_scene_with_policy`] 在磁盘安全检查之后调用；失败即阻止保存。
pub(crate) fn prepare_variant_for_save(
    document: &mut editor::SceneDocument,
    project_root: &Path,
) -> Result<(), String> {
    let Some(base_relative) = document
        .scene
        .metadata
        .get(PREFAB_VARIANT_BASE_META_KEY)
        .cloned()
    else {
        return Ok(());
    };
    let root = strip_verbatim_prefix(project_root);
    let base_absolute = root.join(&base_relative);
    let base = Prefab::load(&base_absolute)
        .map_err(|error| format!("无法读取变体基础 {base_relative}：{error}。请先修复基础文件"))?;
    let current_stamp = FileSignature::stamp_of(&base_absolute)
        .ok_or_else(|| format!("无法读取基础预制体签名：{base_relative}"))?;
    if document
        .scene
        .metadata
        .get(PREFAB_VARIANT_BASE_STAMP_META_KEY)
        .map(String::as_str)
        != Some(current_stamp.as_str())
    {
        return Err(format!(
            "基础预制体 {base_relative} 在上次同步后被修改；请先用「资源 ▸ 从基础同步变体」再保存，\
             否则基础的新值会被记成变体自己的覆盖"
        ));
    }

    let mut variant = document
        .prefab_asset()
        .map_err(|error| format!("读取当前变体失败：{error}"))?;
    if let Some(difference) = structural_difference(&base, &variant) {
        return Err(format!(
            "变体只支持字段级差异（{difference}）；如需增删节点或改层级，请改用独立预制体"
        ));
    }
    let mut overrides = BTreeMap::new();
    for (id, base_entity) in &base.entities {
        let instance = variant
            .entities
            .get(id)
            .ok_or_else(|| format!("变体缺少实体 {}", id.0))?;
        for field in diff_prefab_entity(base_entity, instance) {
            let value = prefab_override_value(instance, &field)?;
            overrides.insert((*id, field), value);
        }
    }

    variant
        .set_variant_base(Some(&base_relative))
        .map_err(|error| error.to_string())?;
    variant
        .set_variant_base_stamp(Some(&current_stamp))
        .map_err(|error| error.to_string())?;
    variant
        .set_overrides(&overrides)
        .map_err(|error| error.to_string())?;

    // 紧接着的 `save_document` 会从 scene.metadata 重新生成资产：谱系记录必须先回写，
    // 否则刚算出来的覆盖只活在局部变量里，落盘时又退回旧记录。
    document.scene.metadata.retain(|key, _| !lineage_key(key));
    for (key, value) in variant.metadata.iter().filter(|(key, _)| lineage_key(key)) {
        document.scene.metadata.insert(key.clone(), value.clone());
    }
    Ok(())
}

fn lineage_key(key: &str) -> bool {
    key == PREFAB_VARIANT_BASE_META_KEY
        || key == PREFAB_VARIANT_BASE_STAMP_META_KEY
        || key.starts_with(PREFAB_OVERRIDE_META_PREFIX)
        || super::prefab_nested_sync::is_nested_lineage_key(key)
}

/// 结构差异（v1 的拒绝项）：根、实体集合、父子关系与顺序、嵌套身份证。
///
/// 判据与「资源 ▸ 查看结构差异…」窗口是同一个比较器（`prefab_structure_diff::
/// structural_changes`）：差异非空即拒绝，拒绝文案点名的是窗口里会列出的同一批条目。
fn structural_difference(reference: &Prefab, candidate: &Prefab) -> Option<String> {
    let changes =
        super::prefab_structure_diff::structural_changes(reference, candidate, "基础", "当前快照");
    (!changes.is_empty()).then(|| super::prefab_structure_diff::summarize(&changes))
}

fn relative_text(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn unique_variant_path(source: &Path) -> PathBuf {
    let parent = source.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "variant".to_owned());
    for index in 1..1000u32 {
        let suffix = if index == 1 {
            "_variant".to_owned()
        } else {
            format!("_variant{index}")
        };
        let candidate = parent.join(format!("{stem}{suffix}.prefab"));
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!("{stem}_variant_{}.prefab", std::process::id()))
}
