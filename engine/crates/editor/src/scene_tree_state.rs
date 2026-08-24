use crate::{DocumentError, SceneDocument};
use engine::core::scene::{SceneEntity, SceneEntityId};
use std::{collections::BTreeSet, error::Error, fmt};

/// 编辑器场景树的选择状态和实体编辑操作。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SceneTreeState {
    selected: Option<SceneEntityId>,
}

impl SceneTreeState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn selected(&self) -> Option<SceneEntityId> {
        self.selected
    }

    pub fn select(
        &mut self,
        document: &SceneDocument,
        id: SceneEntityId,
    ) -> Result<(), SceneTreeError> {
        if !document.scene.entities.contains_key(&id) {
            return Err(SceneTreeError::UnknownEntity(id));
        }
        self.selected = Some(id);
        Ok(())
    }

    pub fn clear_selection(&mut self) {
        self.selected = None;
    }

    pub fn selected_entity<'a>(
        &self,
        document: &'a SceneDocument,
    ) -> Result<&'a SceneEntity, SceneTreeError> {
        let id = self.selected.ok_or(SceneTreeError::NothingSelected)?;
        document
            .scene
            .entities
            .get(&id)
            .ok_or(SceneTreeError::UnknownEntity(id))
    }

    pub fn roots<'a>(&self, document: &'a SceneDocument) -> impl Iterator<Item = &'a SceneEntity> {
        document
            .scene
            .entities
            .values()
            .filter(|entity| entity.parent.is_none())
    }

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

    pub fn create_entity(
        &mut self,
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
        self.selected = Some(id);
        Ok(id)
    }

    pub fn rename_selected(
        &mut self,
        document: &mut SceneDocument,
        name: impl Into<String>,
    ) -> Result<(), SceneTreeError> {
        let id = self.selected.ok_or(SceneTreeError::NothingSelected)?;
        self.rename(document, id, name)
    }

    pub fn rename(
        &mut self,
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

    pub fn delete_selected(&mut self, document: &mut SceneDocument) -> Result<(), SceneTreeError> {
        let id = self.selected.ok_or(SceneTreeError::NothingSelected)?;
        self.delete(document, id)
    }

    pub fn delete(
        &mut self,
        document: &mut SceneDocument,
        id: SceneEntityId,
    ) -> Result<(), SceneTreeError> {
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
        for entity_id in ids {
            document.scene.entities.remove(&entity_id);
        }
        if self
            .selected
            .is_some_and(|selected| ids_contains(document, selected).is_none())
        {
            self.selected = parent;
        }
        // 删除后若原选择已被移除，清除选择；保留父节点选择更符合场景树操作习惯。
        if self.selected == Some(id)
            || self
                .selected
                .is_some_and(|selected| !document.scene.entities.contains_key(&selected))
        {
            self.selected = parent;
        }
        document.mark_dirty();
        Ok(())
    }

    pub fn reparent_selected(
        &mut self,
        document: &mut SceneDocument,
        parent: Option<SceneEntityId>,
    ) -> Result<(), SceneTreeError> {
        let id = self.selected.ok_or(SceneTreeError::NothingSelected)?;
        self.reparent(document, id, parent)
    }

    pub fn reparent(
        &mut self,
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

// 仅用于在删除后判断旧选择是否仍存在；返回值避免借用 document 的复杂生命周期。
fn ids_contains(document: &SceneDocument, id: SceneEntityId) -> Option<SceneEntityId> {
    document.scene.entities.contains_key(&id).then_some(id)
}

#[derive(Debug)]
pub enum SceneTreeError {
    UnknownEntity(SceneEntityId),
    NothingSelected,
    EmptyName,
    Cycle {
        entity: SceneEntityId,
        parent: Option<SceneEntityId>,
    },
    CorruptTree(SceneEntityId),
    Scene(engine::core::scene::SceneError),
    Document(DocumentError),
}

impl fmt::Display for SceneTreeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownEntity(id) => write!(f, "场景树中不存在实体 {}", id.0),
            Self::NothingSelected => write!(f, "当前没有选中的实体"),
            Self::EmptyName => write!(f, "实体名称不能为空"),
            Self::Cycle { entity, parent } => write!(
                f,
                "不能将实体 {} 设置为父节点 {:?}，会形成循环",
                entity.0,
                parent.map(|id| id.0)
            ),
            Self::CorruptTree(id) => write!(f, "实体 {} 的子树包含循环或损坏引用", id.0),
            Self::Scene(error) => write!(f, "场景操作失败：{error}"),
            Self::Document(error) => write!(f, "文档操作失败：{error}"),
        }
    }
}
impl Error for SceneTreeError {}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::core::scene::Scene;

    fn document() -> SceneDocument {
        SceneDocument::new_scene("main")
    }

    #[test]
    fn create_select_and_rename_marks_dirty() {
        let mut document = document();
        document.mark_clean();
        let mut tree = SceneTreeState::new();
        let id = tree.create_entity(&mut document, "Player", None).unwrap();
        assert_eq!(tree.selected(), Some(id));
        assert_eq!(tree.selected_entity(&document).unwrap().name, "Player");
        tree.rename_selected(&mut document, "Hero").unwrap();
        assert_eq!(document.scene.entities[&id].name, "Hero");
        assert!(document.is_dirty());
    }

    #[test]
    fn delete_removes_complete_subtree_and_selects_parent() {
        let mut document = document();
        let mut tree = SceneTreeState::new();
        let root = tree.create_entity(&mut document, "Root", None).unwrap();
        let child = tree
            .create_entity(&mut document, "Child", Some(root))
            .unwrap();
        let grandchild = tree
            .create_entity(&mut document, "Grandchild", Some(child))
            .unwrap();
        tree.select(&document, root).unwrap();
        tree.delete_selected(&mut document).unwrap();
        assert!(!document.scene.entities.contains_key(&root));
        assert!(!document.scene.entities.contains_key(&child));
        assert!(!document.scene.entities.contains_key(&grandchild));
        assert_eq!(tree.selected(), None);
    }

    #[test]
    fn reparent_updates_both_sides_and_roundtrips() {
        let mut document = document();
        let mut tree = SceneTreeState::new();
        let first = tree.create_entity(&mut document, "First", None).unwrap();
        let second = tree.create_entity(&mut document, "Second", None).unwrap();
        tree.reparent(&mut document, second, Some(first)).unwrap();
        assert_eq!(document.scene.entities[&second].parent, Some(first));
        assert_eq!(document.scene.entities[&first].children, vec![second]);
        assert!(Scene::from_text(&document.scene.to_text()).is_ok());
    }

    #[test]
    fn rejects_unknown_empty_name_and_cycle() {
        let mut document = document();
        let mut tree = SceneTreeState::new();
        let root = tree.create_entity(&mut document, "Root", None).unwrap();
        let child = tree
            .create_entity(&mut document, "Child", Some(root))
            .unwrap();
        assert!(matches!(
            tree.select(&document, SceneEntityId(999)),
            Err(SceneTreeError::UnknownEntity(_))
        ));
        assert!(matches!(
            tree.rename(&mut document, root, "  "),
            Err(SceneTreeError::EmptyName)
        ));
        assert!(matches!(
            tree.reparent(&mut document, root, Some(child)),
            Err(SceneTreeError::Cycle { .. })
        ));
    }
}
