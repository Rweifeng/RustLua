//! Inspector 的 Prefab 侧：源缓存、覆盖摘要、实例判定与覆盖取值。
//!
//! 从 `panels/inspector.rs` 原样搬出（P1 结构收口）。这些方法只读文档与源缓存；
//! 任何写动作仍走 shell（见 `panels/architecture_tests.rs` 的棘轮）。

use super::helpers::*;
use super::*;

impl EditorApp {
    // 粘贴组件（`EditorCommand::PasteComponents`）与重命名（`RenameSelection`）的实现体
    // 都已搬到 shell（`crate::actions`）：面板按钮只发 typed action。

    /// 确保源预制体 已缓存（必要时重新读盘）。返回后不再持有 `&mut self` 借用，
    /// 调用方随后从 `prefab_source_cache` 取不可变引用即可。
    pub(crate) fn ensure_prefab_source_cached(&mut self, path: &Path) -> Result<(), String> {
        let stamp = prefab_stamp(path);
        let cached_fresh = stamp.is_some()
            && self
                .prefab_source_cache
                .get(path)
                .is_some_and(|cached| cached.stamp == stamp);
        if cached_fresh {
            return Ok(());
        }
        let prefab = Prefab::load(path).map_err(|error| format!("无法加载源预制体：{error}"))?;
        // M79 P2：真的重读了一次磁盘 = 源取值可能变了。整实例覆盖列表的缓存键带着这个计数，
        // 于是"场景文档一个字没改、源被别的会话（或 in-context 阶段）存了盘"也会让列表失效。
        self.prefab_source_generation = self.prefab_source_generation.wrapping_add(1);
        self.prefab_source_cache
            .insert(path.to_path_buf(), CachedPrefabSource { stamp, prefab });
        Ok(())
    }

    pub(super) fn prefab_override_summary(
        &mut self,
        selected: SceneEntityId,
    ) -> Option<Result<Vec<PrefabOverrideRow>, String>> {
        // 先把需要的元数据拷出来，结束对场景的不可变借用。
        let (prefab_source, source_id) = {
            let scene = self.current_scene()?;
            let entity = scene.scene.entities.get(&selected)?;
            let prefab_source = entity.metadata.get("prefab.source")?.clone();
            let source_id = entity
                .metadata
                .get("prefab.source_id")?
                .parse::<u64>()
                .ok()
                .map(SceneEntityId)?;
            (prefab_source, source_id)
        };
        let path = strip_verbatim_prefix(&self.project.root).join(&prefab_source);
        if let Err(error) = self.ensure_prefab_source_cached(&path) {
            return Some(Err(error));
        }
        // 之后全是不可变借用：缓存里的 Prefab 与当前场景可以同时持有。
        let prefab = self
            .prefab_source_cache
            .get(&path)
            .map(|cached| &cached.prefab)?;
        let Some(source_entity) = prefab.entities.get(&source_id) else {
            return Some(Err(format!("源预制体中缺少实体 {}", source_id.0)));
        };
        let scene = self.current_scene()?;
        let entity = scene.scene.entities.get(&selected)?;
        Some(Ok(prefab_override_rows(source_entity, entity)))
    }

    // 选择性 Revert（`EditorCommand::RevertPrefabOverride`）的实现体已搬到 shell
    // （`crate::actions` 的 `revert_prefab_override_field`）：面板只发 typed action。

    // 重新应用源预制体（`EditorCommand::ReapplyPrefab`）的实现体已搬到 shell
    // （`crate::actions` 的 `reapply_prefab`）：目标由 typed action 显式携带。

    /// 把实例写回源预制体 文件（`EditorCommand::SavePrefabToSource`）与逐字段 Apply
    /// （`EditorCommand::ApplyPrefabOverride`）的实现体都已搬到 shell（`crate::actions` 的
    /// `save_prefab_to_source` / `apply_prefab_override_field`）：面板只发 typed action。
    /// 是否是 Prefab 实例（有 `prefab.source`）。typed action 的前置条件检查用它，
    /// 避免"面板与命令层各写一遍元数据判断"。
    pub(crate) fn is_prefab_instance(&self, selected: SceneEntityId) -> bool {
        self.document
            .current_scene
            .as_ref()
            .and_then(|document| document.scene.entities.get(&selected))
            .is_some_and(|entity| entity.metadata.contains_key("prefab.source"))
    }

    /// 是否是 Prefab **根**实例（`prefab.root=true`）：只有根实例支持重新应用/写回/解包。
    pub(crate) fn is_prefab_root(&self, selected: SceneEntityId) -> bool {
        self.document
            .current_scene
            .as_ref()
            .and_then(|document| document.scene.entities.get(&selected))
            .is_some_and(|entity| {
                entity
                    .metadata
                    .get("prefab.root")
                    .is_some_and(|value| value == "true")
            })
    }

    /// Prefab 实例的 `(来源路径, 源实体 ID)`；非实例返回 `None`。
    pub(crate) fn prefab_instance_source(
        &self,
        selected: SceneEntityId,
    ) -> Option<(String, SceneEntityId)> {
        let scene = self.document.current_scene.as_ref()?;
        let entity = scene.scene.entities.get(&selected)?;
        Some((
            entity.metadata.get("prefab.source")?.clone(),
            entity
                .metadata
                .get("prefab.source_id")?
                .parse::<u64>()
                .ok()
                .map(SceneEntityId)?,
        ))
    }

    /// 覆盖字段的**实例侧**原始文本（应用回源时用的就是这个值）。
    pub(crate) fn prefab_override_instance_value(
        &self,
        selected: SceneEntityId,
        field: &str,
    ) -> Option<String> {
        let scene = self.document.current_scene.as_ref()?;
        let entity = scene.scene.entities.get(&selected)?;
        Some(prefab_override_side_value(entity, field))
    }

    // 「落盘 Prefab + 重建实例」整条路径（`write_prefab_and_rebuild_instances` 及其
    // Prefab 写路径辅助）已整体搬到 shell（`crate::actions`）：面板不再持有提交边界。

    // 解包 Prefab（`EditorCommand::UnpackPrefab`）的实现体已搬到 shell
    // （`crate::actions` 的 `unpack_prefab`）：面板只发 typed action。
}
