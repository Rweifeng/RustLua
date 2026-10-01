use crate::DocumentError;
use crate::SceneDocument;
use engine::core::scene::{SceneEntity, SceneEntityId};
use std::collections::BTreeSet;

/// 场景树**文档操作**的无状态助手。
///
/// E03 之后本类型不再持有任何“当前选择”：选择状态统一收敛到
/// [`crate::SelectionModel`]（`EditorApp.selection`）。所有编辑操作显式传入目标
/// 实体 ID，调用方（面板/命令层）负责从统一选择模型读取主选中/集合。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SceneTreeState {}

const ROOT_ORDER_META_KEY: &str = "editor.root_order";

impl SceneTreeState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 根实体迭代器（Hierarchy 面板渲染入口）。
    pub fn roots<'a>(&self, document: &'a SceneDocument) -> impl Iterator<Item = &'a SceneEntity> {
        let mut ordered = root_order(document);
        for entity in document.scene.entities.values() {
            if entity.parent.is_none() && !ordered.contains(&entity.id) {
                ordered.push(entity.id);
            }
        }
        ordered
            .into_iter()
            .filter_map(|id| document.scene.entities.get(&id))
    }

    /// `parent` 的直接子实体（按场景树存储顺序）。
    pub fn children<'a>(
        &self,
        document: &'a SceneDocument,
        parent: SceneEntityId,
    ) -> Result<Vec<&'a SceneEntity>, SceneTreeError> {
        let entity = document
            .scene
            .entities
            .get(&parent)
            .ok_or(SceneTreeError::UnknownEntity(parent))?;
        Ok(entity
            .children
            .iter()
            .filter_map(|id| document.scene.entities.get(id))
            .collect())
    }

    /// 创建一个实体并**返回新 ID**（不修改任何选择状态）。
    ///
    /// 调用方随后通过 `selection.add(id, …)` 把新实体纳入统一选择模型。
    pub fn create_entity(
        &self,
        document: &mut SceneDocument,
        name: impl Into<String>,
        parent: Option<SceneEntityId>,
    ) -> Result<SceneEntityId, SceneTreeError> {
        if let Some(parent_id) = parent {
            ensure_entity(document, parent_id)?;
        }
        let id = next_id(document);
        document
            .scene
            .add_entity(id, name, parent)
            .map_err(SceneTreeError::Scene)?;
        document.mark_dirty();
        Ok(id)
    }

    /// 复制实体及其完整子树，返回新子树根 ID。
    pub fn duplicate_subtree(
        &self,
        document: &mut SceneDocument,
        source: &SceneEntity,
        parent: Option<SceneEntityId>,
    ) -> Result<SceneEntityId, SceneTreeError> {
        if let Some(parent_id) = parent {
            ensure_entity(document, parent_id)?;
        }
        let root_id = duplicate_entity(document, source, parent)?;
        document.mark_dirty();
        Ok(root_id)
    }

    pub fn rename(
        &self,
        document: &mut SceneDocument,
        id: SceneEntityId,
        name: impl Into<String>,
    ) -> Result<(), SceneTreeError> {
        let entity = document
            .scene
            .entities
            .get_mut(&id)
            .ok_or(SceneTreeError::UnknownEntity(id))?;
        let name = name.into();
        if name.trim().is_empty() {
            return Err(SceneTreeError::EmptyName);
        }
        if entity.name != name {
            entity.name = name;
            document.mark_dirty();
        }
        Ok(())
    }

    /// 删除 `id` 及其整棵子树，返回被删除的完整实体 ID 集合。
    ///
    /// 调用方应把返回值交给 `SelectionModel::on_entities_removed` 做悬空清理，
    /// 以便删除后选择回退到父节点。
    pub fn delete(
        &self,
        document: &mut SceneDocument,
        id: SceneEntityId,
    ) -> Result<BTreeSet<SceneEntityId>, SceneTreeError> {
        ensure_entity(document, id)?;
        let ids = collect_subtree(document, id)?;
        let parent = document
            .scene
            .entities
            .get(&id)
            .and_then(|entity| entity.parent);
        if let Some(parent_id) = parent
            && let Some(parent_entity) = document.scene.entities.get_mut(&parent_id)
        {
            parent_entity.children.retain(|child| *child != id);
        }
        for entity_id in ids.iter() {
            document.scene.entities.remove(entity_id);
        }
        document.mark_dirty();
        let removed = ids.into_iter().collect();
        Ok(removed)
    }

    pub fn reparent(
        &self,
        document: &mut SceneDocument,
        id: SceneEntityId,
        parent: Option<SceneEntityId>,
    ) -> Result<(), SceneTreeError> {
        ensure_entity(document, id)?;
        if parent == Some(id) {
            return Err(SceneTreeError::Cycle { entity: id, parent });
        }
        if let Some(parent_id) = parent {
            ensure_entity(document, parent_id)?;
            if collect_subtree(document, id)?.contains(&parent_id) {
                return Err(SceneTreeError::Cycle { entity: id, parent });
            }
        }
        let old_parent = document
            .scene
            .entities
            .get(&id)
            .and_then(|entity| entity.parent);
        if old_parent == parent {
            return Ok(());
        }
        if let Some(old_parent_id) = old_parent
            && let Some(entity) = document.scene.entities.get_mut(&old_parent_id)
        {
            entity.children.retain(|child| *child != id);
        }
        if let Some(new_parent_id) = parent {
            document
                .scene
                .entities
                .get_mut(&new_parent_id)
                .unwrap()
                .children
                .push(id);
        }
        document.scene.entities.get_mut(&id).unwrap().parent = parent;
        document.mark_dirty();
        Ok(())
    }

    /// 将实体移动到目标节点的前面、后面，或作为目标的子节点。
    pub fn reparent_at(
        &self,
        document: &mut SceneDocument,
        id: SceneEntityId,
        target: SceneEntityId,
        position: ReparentPosition,
    ) -> Result<(), SceneTreeError> {
        ensure_entity(document, id)?;
        ensure_entity(document, target)?;
        if id == target || collect_subtree(document, id)?.contains(&target) {
            return Err(SceneTreeError::Cycle {
                entity: id,
                parent: Some(target),
            });
        }
        if position == ReparentPosition::Child {
            return self.reparent(document, id, Some(target));
        }

        let new_parent = document.scene.entities[&target].parent;
        let old_parent = document.scene.entities[&id].parent;
        if new_parent.is_none() {
            let mut roots = root_order(document);
            roots.retain(|root| *root != id);
            let target_index = roots
                .iter()
                .position(|root| *root == target)
                .ok_or(SceneTreeError::CannotReorderRoot(target))?;
            let insert_index = if position == ReparentPosition::Before {
                target_index
            } else {
                target_index + 1
            };
            if let Some(old_parent_id) = old_parent {
                document
                    .scene
                    .entities
                    .get_mut(&old_parent_id)
                    .unwrap()
                    .children
                    .retain(|child| *child != id);
                document.scene.entities.get_mut(&id).unwrap().parent = None;
            }
            roots.insert(insert_index, id);
            write_root_order(document, &roots);
            document.mark_dirty();
            return Ok(());
        }
        let new_parent_id = new_parent.unwrap();
        if old_parent == new_parent {
            let siblings = &mut document
                .scene
                .entities
                .get_mut(&new_parent_id)
                .unwrap()
                .children;
            siblings.retain(|child| *child != id);
            let target_index = siblings.iter().position(|child| *child == target).unwrap();
            let insert_index = if position == ReparentPosition::Before {
                target_index
            } else {
                target_index + 1
            };
            siblings.insert(insert_index, id);
        } else {
            if let Some(old_parent_id) = old_parent {
                document
                    .scene
                    .entities
                    .get_mut(&old_parent_id)
                    .unwrap()
                    .children
                    .retain(|child| *child != id);
            }
            let siblings = &mut document
                .scene
                .entities
                .get_mut(&new_parent_id)
                .unwrap()
                .children;
            let target_index = siblings.iter().position(|child| *child == target).unwrap();
            let insert_index = if position == ReparentPosition::Before {
                target_index
            } else {
                target_index + 1
            };
            siblings.insert(insert_index, id);
            document.scene.entities.get_mut(&id).unwrap().parent = new_parent;
        }
        document.mark_dirty();
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReparentPosition {
    Before,
    After,
    Child,
}

fn ensure_entity(document: &SceneDocument, id: SceneEntityId) -> Result<(), SceneTreeError> {
    document
        .scene
        .entities
        .contains_key(&id)
        .then_some(())
        .ok_or(SceneTreeError::UnknownEntity(id))
}

fn next_id(document: &SceneDocument) -> SceneEntityId {
    let mut id = document
        .scene
        .entities
        .keys()
        .map(|id| id.0)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    while document.scene.entities.contains_key(&SceneEntityId(id)) {
        id = id.saturating_add(1);
    }
    SceneEntityId(id)
}

fn root_order(document: &SceneDocument) -> Vec<SceneEntityId> {
    let mut ordered: Vec<_> = document
        .scene
        .metadata
        .get(ROOT_ORDER_META_KEY)
        .into_iter()
        .flat_map(|value| {
            value
                .split(',')
                .filter_map(|id| id.parse().ok().map(SceneEntityId))
        })
        .filter(|id| {
            document
                .scene
                .entities
                .get(id)
                .is_some_and(|entity| entity.parent.is_none())
        })
        .collect();
    for entity in document.scene.entities.values() {
        if entity.parent.is_none() && !ordered.contains(&entity.id) {
            ordered.push(entity.id);
        }
    }
    ordered
}

fn write_root_order(document: &mut SceneDocument, roots: &[SceneEntityId]) {
    document.scene.metadata.insert(
        ROOT_ORDER_META_KEY.to_owned(),
        roots
            .iter()
            .map(|id| id.0.to_string())
            .collect::<Vec<_>>()
            .join(","),
    );
}

fn duplicate_entity(
    document: &mut SceneDocument,
    source: &SceneEntity,
    parent: Option<SceneEntityId>,
) -> Result<SceneEntityId, SceneTreeError> {
    let id = next_id(document);
    let mut copy = source.clone();
    copy.id = id;
    copy.parent = parent;
    copy.children.clear();
    document.scene.entities.insert(id, copy);
    if let Some(parent_id) = parent {
        document
            .scene
            .entities
            .get_mut(&parent_id)
            .unwrap()
            .children
            .push(id);
    }
    for child_id in &source.children {
        let child = document
            .scene
            .entities
            .get(child_id)
            .cloned()
            .ok_or(SceneTreeError::UnknownEntity(*child_id))?;
        duplicate_entity(document, &child, Some(id))?;
    }
    Ok(id)
}

fn collect_subtree(
    document: &SceneDocument,
    root: SceneEntityId,
) -> Result<Vec<SceneEntityId>, SceneTreeError> {
    ensure_entity(document, root)?;
    let mut result = Vec::new();
    let mut visiting = BTreeSet::new();
    collect_subtree_inner(document, root, &mut result, &mut visiting)?;
    Ok(result)
}

fn collect_subtree_inner(
    document: &SceneDocument,
    id: SceneEntityId,
    result: &mut Vec<SceneEntityId>,
    visiting: &mut BTreeSet<SceneEntityId>,
) -> Result<(), SceneTreeError> {
    if !visiting.insert(id) {
        return Err(SceneTreeError::CorruptTree(id));
    }
    let entity = document
        .scene
        .entities
        .get(&id)
        .ok_or(SceneTreeError::UnknownEntity(id))?;
    for child in &entity.children {
        collect_subtree_inner(document, *child, result, visiting)?;
    }
    result.push(id);
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum SceneTreeError {
    #[error("场景树中不存在实体 {}", .0 .0)]
    UnknownEntity(SceneEntityId),
    #[error("当前没有选中的实体")]
    NothingSelected,
    #[error("实体名称不能为空")]
    EmptyName,
    #[error("不能将实体 {} 设置为父节点 {:?}，会形成循环", entity.0, parent.map(|id| id.0))]
    Cycle {
        entity: SceneEntityId,
        parent: Option<SceneEntityId>,
    },
    #[error("实体 {} 的子树包含循环或损坏引用", .0 .0)]
    CorruptTree(SceneEntityId),
    #[error("根节点 {} 没有可调整的同级顺序", .0 .0)]
    CannotReorderRoot(SceneEntityId),
    #[error("场景操作失败：{0}")]
    Scene(#[source] engine::core::scene::SceneError),
    #[error("文档操作失败：{0}")]
    Document(#[source] DocumentError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::core::scene::Scene;

    fn document() -> SceneDocument {
        SceneDocument::new_scene("main")
    }

    #[test]
    fn create_and_rename_marks_dirty() {
        let mut document = document();
        document.mark_clean();
        let tree = SceneTreeState::new();
        let id = tree.create_entity(&mut document, "Player", None).unwrap();
        assert_eq!(document.scene.entities[&id].name, "Player");
        tree.rename(&mut document, id, "Hero").unwrap();
        assert_eq!(document.scene.entities[&id].name, "Hero");
        assert!(document.is_dirty());
    }

    #[test]
    fn delete_removes_complete_subtree_and_reports_ids() {
        let mut document = document();
        let tree = SceneTreeState::new();
        let root = tree.create_entity(&mut document, "Root", None).unwrap();
        let child = tree
            .create_entity(&mut document, "Child", Some(root))
            .unwrap();
        let grandchild = tree
            .create_entity(&mut document, "Grandchild", Some(child))
            .unwrap();
        let removed = tree.delete(&mut document, root).unwrap();
        assert!(!document.scene.entities.contains_key(&root));
        assert!(!document.scene.entities.contains_key(&child));
        assert!(!document.scene.entities.contains_key(&grandchild));
        assert!(removed.contains(&root));
        assert!(removed.contains(&child));
        assert!(removed.contains(&grandchild));
    }

    #[test]
    fn reparent_updates_both_sides_and_roundtrips() {
        let mut document = document();
        let tree = SceneTreeState::new();
        let first = tree.create_entity(&mut document, "First", None).unwrap();
        let second = tree.create_entity(&mut document, "Second", None).unwrap();
        tree.reparent(&mut document, second, Some(first)).unwrap();
        assert_eq!(document.scene.entities[&second].parent, Some(first));
        assert_eq!(document.scene.entities[&first].children, vec![second]);
        assert!(Scene::from_text(&document.scene.to_text()).is_ok());
    }

    #[test]
    fn reparent_at_updates_sibling_order() {
        let mut document = document();
        let tree = SceneTreeState::new();
        let root = tree.create_entity(&mut document, "Root", None).unwrap();
        let first = tree
            .create_entity(&mut document, "First", Some(root))
            .unwrap();
        let second = tree
            .create_entity(&mut document, "Second", Some(root))
            .unwrap();
        let third = tree
            .create_entity(&mut document, "Third", Some(root))
            .unwrap();

        tree.reparent_at(&mut document, third, first, ReparentPosition::Before)
            .unwrap();
        assert_eq!(
            document.scene.entities[&root].children,
            vec![third, first, second]
        );
        tree.reparent_at(&mut document, first, second, ReparentPosition::After)
            .unwrap();
        assert_eq!(
            document.scene.entities[&root].children,
            vec![third, second, first]
        );
    }

    #[test]
    fn duplicate_subtree_gets_fresh_ids_and_preserves_data() {
        let mut document = document();
        let tree = SceneTreeState::new();
        let source_id = tree.create_entity(&mut document, "Source", None).unwrap();
        let child_id = tree
            .create_entity(&mut document, "Child", Some(source_id))
            .unwrap();
        document
            .scene
            .entities
            .get_mut(&source_id)
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "position=1,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
        let source = document.scene.entities[&source_id].clone();

        let copy_id = tree
            .duplicate_subtree(&mut document, &source, None)
            .unwrap();
        let copied_child_id = document.scene.entities[&copy_id].children[0];
        assert_ne!(copy_id, source_id);
        assert_ne!(copied_child_id, child_id);
        assert_eq!(document.scene.entities[&copy_id].name, "Source");
        assert_eq!(document.scene.entities[&copy_id].parent, None);
        assert_eq!(
            document.scene.entities[&copy_id].components["Transform"],
            "position=1,0,0;rotation=0,0,0,1;scale=1,1,1"
        );
        assert_eq!(
            document.scene.entities[&copied_child_id].parent,
            Some(copy_id)
        );
    }

    #[test]
    fn reparent_at_persists_root_order() {
        let mut document = document();
        let tree = SceneTreeState::new();
        let first = tree.create_entity(&mut document, "First", None).unwrap();
        let second = tree.create_entity(&mut document, "Second", None).unwrap();
        let third = tree.create_entity(&mut document, "Third", None).unwrap();

        tree.reparent_at(&mut document, third, first, ReparentPosition::Before)
            .unwrap();
        let roots: Vec<_> = tree.roots(&document).map(|entity| entity.id).collect();
        assert_eq!(roots, vec![third, first, second]);
        let roundtrip = SceneDocument {
            path: None,
            scene: Scene::from_text(&document.scene.to_text()).unwrap(),
            dirty: false,
            revision: 0,
            disk_signature: None,
        };
        let roots: Vec<_> = tree.roots(&roundtrip).map(|entity| entity.id).collect();
        assert_eq!(roots, vec![third, first, second]);
    }

    #[test]
    fn rejects_unknown_empty_name_and_cycle() {
        let mut document = document();
        let tree = SceneTreeState::new();
        let root = tree.create_entity(&mut document, "Root", None).unwrap();
        let child = tree
            .create_entity(&mut document, "Child", Some(root))
            .unwrap();
        assert!(matches!(
            tree.children(&document, SceneEntityId(999)),
            Err(SceneTreeError::UnknownEntity(_))
        ));
        assert!(matches!(
            tree.rename(&mut document, root, "  "),
            Err(SceneTreeError::EmptyName)
        ));
        assert!(matches!(
            tree.reparent(&mut document, root, Some(root)),
            Err(SceneTreeError::Cycle { .. })
        ));
        assert!(matches!(
            tree.reparent(&mut document, root, Some(child)),
            Err(SceneTreeError::Cycle { .. })
        ));
    }
}
