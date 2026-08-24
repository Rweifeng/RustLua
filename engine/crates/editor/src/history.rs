use crate::SceneDocument;
use engine::core::scene::SceneEntityId;
use std::{error::Error, fmt};

/// A reversible editor operation backed by document snapshots.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandKind {
    Generic,
    Transform(SceneEntityId),
    Transform3D(SceneEntityId),
    Create,
    Delete,
    Rename,
    Reparent,
    Property,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditCommand {
    pub label: String,
    pub kind: CommandKind,
    before: SceneDocument,
    after: SceneDocument,
}

impl EditCommand {
    pub fn new(
        label: impl Into<String>,
        kind: CommandKind,
        before: SceneDocument,
        after: SceneDocument,
    ) -> Self {
        Self {
            label: label.into(),
            kind,
            before,
            after,
        }
    }
    pub fn before(&self) -> &SceneDocument {
        &self.before
    }
    pub fn after(&self) -> &SceneDocument {
        &self.after
    }
}

#[derive(Clone, Debug, Default)]
pub struct CommandHistory {
    undo: Vec<EditCommand>,
    redo: Vec<EditCommand>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryError {
    NothingToUndo,
    NothingToRedo,
}
impl fmt::Display for HistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NothingToUndo => "nothing to undo",
            Self::NothingToRedo => "nothing to redo",
        })
    }
}
impl Error for HistoryError {}

impl CommandHistory {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }
    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// Executes an edit; no-op edits are not recorded. New edits clear redo.
    pub fn execute<F, E>(
        &mut self,
        document: &mut SceneDocument,
        label: impl Into<String>,
        kind: CommandKind,
        edit: F,
    ) -> Result<bool, E>
    where
        F: FnOnce(&mut SceneDocument) -> Result<(), E>,
    {
        let before = document.clone();
        edit(document)?;
        if *document == before {
            return Ok(false);
        }
        let after = document.clone();
        self.redo.clear();
        if let Some(previous) = self.undo.last_mut()
            && previous.kind == kind
            && matches!(
                kind,
                CommandKind::Transform(_) | CommandKind::Transform3D(_)
            )
        {
            previous.after = after;
        } else {
            self.undo.push(EditCommand::new(label, kind, before, after));
        }
        Ok(true)
    }

    pub fn push(&mut self, command: EditCommand) {
        if command.before != command.after {
            self.redo.clear();
            self.undo.push(command);
        }
    }

    pub fn undo(&mut self, document: &mut SceneDocument) -> Result<&str, HistoryError> {
        let command = self.undo.pop().ok_or(HistoryError::NothingToUndo)?;
        *document = command.before.clone();
        self.redo.push(command);
        Ok(self.redo.last().expect("just pushed").label.as_str())
    }
    pub fn redo(&mut self, document: &mut SceneDocument) -> Result<&str, HistoryError> {
        let command = self.redo.pop().ok_or(HistoryError::NothingToRedo)?;
        *document = command.after.clone();
        self.undo.push(command);
        Ok(self.undo.last().expect("just pushed").label.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn doc() -> SceneDocument {
        SceneDocument::new("main")
    }
    #[test]
    fn undo_redo_and_new_edit_branch() {
        let mut d = doc();
        let mut h = CommandHistory::new();
        h.execute(&mut d, "create", CommandKind::Create, |d| {
            d.scene
                .add_entity(SceneEntityId(1), "A", None)
                .map_err(|_| ())
        })
        .unwrap();
        h.undo(&mut d).unwrap();
        assert!(d.scene.entities.is_empty());
        h.redo(&mut d).unwrap();
        h.undo(&mut d).unwrap();
        h.execute(&mut d, "other", CommandKind::Generic, |d| {
            d.scene
                .add_entity(SceneEntityId(2), "B", None)
                .map_err(|_| ())
        })
        .unwrap();
        assert!(!h.can_redo());
    }
    #[test]
    fn consecutive_transforms_merge() {
        let mut d = doc();
        d.scene.add_entity(SceneEntityId(1), "A", None).unwrap();
        let mut h = CommandHistory::new();
        for value in 1..=100 {
            h.execute(
                &mut d,
                "transform",
                CommandKind::Transform(SceneEntityId(1)),
                |d| {
                    d.scene
                        .entities
                        .get_mut(&SceneEntityId(1))
                        .unwrap()
                        .components
                        .insert("x".into(), value.to_string());
                    Ok::<_, ()>(())
                },
            )
            .unwrap();
        }
        assert_eq!(h.undo_len(), 1);
        h.undo(&mut d).unwrap();
        assert!(
            !d.scene.entities[&SceneEntityId(1)]
                .components
                .contains_key("x")
        );
    }
    #[test]
    fn consecutive_transform3d_edits_merge() {
        let mut d = doc();
        d.scene.add_entity(SceneEntityId(7), "A", None).unwrap();
        let mut h = CommandHistory::new();
        for value in 1..=50 {
            h.execute(
                &mut d,
                "3d transform",
                CommandKind::Transform3D(SceneEntityId(7)),
                |d| {
                    d.scene
                        .entities
                        .get_mut(&SceneEntityId(7))
                        .unwrap()
                        .components
                        .insert("Transform3D".into(), format!("position={value},0,0"));
                    Ok::<_, ()>(())
                },
            )
            .unwrap();
        }
        assert_eq!(h.undo_len(), 1, "consecutive 3D transforms merge");
    }
}
