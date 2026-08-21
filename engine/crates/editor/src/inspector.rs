use crate::SceneDocument;
use engine_core::scene::{SceneEntity, SceneEntityId};
use std::{error::Error, fmt, path::Path};

/// 第一批 Inspector 可编辑的 Transform 字段。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransformFields {
    pub position_x: f32,
    pub position_y: f32,
    pub rotation: f32,
    pub scale_x: f32,
    pub scale_y: f32,
}

impl Default for TransformFields {
    fn default() -> Self {
        Self {
            position_x: 0.0,
            position_y: 0.0,
            rotation: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
        }
    }
}

/// Inspector 对一个实体的可编辑快照。
#[derive(Clone, Debug, PartialEq)]
pub struct InspectorSnapshot {
    pub entity_id: SceneEntityId,
    pub name: String,
    pub transform: TransformFields,
    pub sprite_reference: Option<String>,
    pub lua_script: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InspectorState {
    selected: Option<SceneEntityId>,
}

impl InspectorState {
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
    ) -> Result<(), InspectorError> {
        if !document.scene.entities.contains_key(&id) {
            return Err(InspectorError::UnknownEntity(id));
        }
        self.selected = Some(id);
        Ok(())
    }

    pub fn clear_selection(&mut self) {
        self.selected = None;
    }

    pub fn snapshot(&self, document: &SceneDocument) -> Result<InspectorSnapshot, InspectorError> {
        let id = self.selected.ok_or(InspectorError::NothingSelected)?;
        let entity = document
            .scene
            .entities
            .get(&id)
            .ok_or(InspectorError::UnknownEntity(id))?;
        snapshot_for(entity)
    }

    pub fn set_name(
        &self,
        document: &mut SceneDocument,
        name: impl Into<String>,
    ) -> Result<(), InspectorError> {
        let id = self.selected.ok_or(InspectorError::NothingSelected)?;
        let name = name.into();
        if name.trim().is_empty() {
            return Err(InspectorError::EmptyName);
        }
        let entity = entity_mut(document, id)?;
        if entity.name != name {
            entity.name = name;
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn set_transform(
        &self,
        document: &mut SceneDocument,
        transform: TransformFields,
    ) -> Result<(), InspectorError> {
        validate_transform(transform)?;
        let id = self.selected.ok_or(InspectorError::NothingSelected)?;
        let entity = entity_mut(document, id)?;
        let value = encode_transform(transform);
        if entity.components.get("Transform") != Some(&value) {
            entity.components.insert("Transform".into(), value);
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn set_sprite_reference(
        &self,
        document: &mut SceneDocument,
        reference: impl Into<String>,
    ) -> Result<(), InspectorError> {
        let reference = reference.into();
        validate_asset_reference(&reference)?;
        let id = self.selected.ok_or(InspectorError::NothingSelected)?;
        let entity = entity_mut(document, id)?;
        if entity.components.get("Sprite") != Some(&reference) {
            entity.components.insert("Sprite".into(), reference);
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn clear_sprite_reference(
        &self,
        document: &mut SceneDocument,
    ) -> Result<(), InspectorError> {
        let id = self.selected.ok_or(InspectorError::NothingSelected)?;
        let entity = entity_mut(document, id)?;
        if entity.components.remove("Sprite").is_some() {
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn set_lua_script(
        &self,
        document: &mut SceneDocument,
        script: impl Into<String>,
    ) -> Result<(), InspectorError> {
        let script = script.into();
        validate_lua_script(&script)?;
        let id = self.selected.ok_or(InspectorError::NothingSelected)?;
        let entity = entity_mut(document, id)?;
        if entity.components.get("LuaScript") != Some(&script) {
            entity.components.insert("LuaScript".into(), script);
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn clear_lua_script(&self, document: &mut SceneDocument) -> Result<(), InspectorError> {
        let id = self.selected.ok_or(InspectorError::NothingSelected)?;
        let entity = entity_mut(document, id)?;
        if entity.components.remove("LuaScript").is_some() {
            document.mark_dirty();
        }
        Ok(())
    }
}

fn entity_mut(
    document: &mut SceneDocument,
    id: SceneEntityId,
) -> Result<&mut SceneEntity, InspectorError> {
    document
        .scene
        .entities
        .get_mut(&id)
        .ok_or(InspectorError::UnknownEntity(id))
}

fn snapshot_for(entity: &SceneEntity) -> Result<InspectorSnapshot, InspectorError> {
    let transform = entity
        .components
        .get("Transform")
        .map_or(Ok(TransformFields::default()), |value| {
            decode_transform(value)
        })?;
    Ok(InspectorSnapshot {
        entity_id: entity.id,
        name: entity.name.clone(),
        transform,
        sprite_reference: entity.components.get("Sprite").cloned(),
        lua_script: entity.components.get("LuaScript").cloned(),
    })
}

fn validate_transform(transform: TransformFields) -> Result<(), InspectorError> {
    let values = [
        transform.position_x,
        transform.position_y,
        transform.rotation,
        transform.scale_x,
        transform.scale_y,
    ];
    if values.iter().any(|value| !value.is_finite()) {
        return Err(InspectorError::NonFiniteTransform);
    }
    if transform.scale_x <= 0.0 || transform.scale_y <= 0.0 {
        return Err(InspectorError::InvalidScale);
    }
    Ok(())
}

fn encode_transform(transform: TransformFields) -> String {
    format!(
        "x={:.9};y={:.9};rotation={:.9};scale_x={:.9};scale_y={:.9}",
        transform.position_x,
        transform.position_y,
        transform.rotation,
        transform.scale_x,
        transform.scale_y
    )
}

fn decode_transform(value: &str) -> Result<TransformFields, InspectorError> {
    let mut transform = TransformFields::default();
    let mut seen = [false; 5];
    for field in value.split(';') {
        let (key, raw) = field
            .split_once('=')
            .ok_or_else(|| InspectorError::InvalidTransform(value.to_owned()))?;
        let number: f32 = raw
            .parse()
            .map_err(|_| InspectorError::InvalidTransform(value.to_owned()))?;
        match key {
            "x" => {
                transform.position_x = number;
                seen[0] = true;
            }
            "y" => {
                transform.position_y = number;
                seen[1] = true;
            }
            "rotation" => {
                transform.rotation = number;
                seen[2] = true;
            }
            "scale_x" => {
                transform.scale_x = number;
                seen[3] = true;
            }
            "scale_y" => {
                transform.scale_y = number;
                seen[4] = true;
            }
            _ => return Err(InspectorError::InvalidTransform(value.to_owned())),
        }
    }
    if seen.iter().any(|value| !value) {
        return Err(InspectorError::InvalidTransform(value.to_owned()));
    }
    validate_transform(transform)?;
    Ok(transform)
}

fn validate_asset_reference(reference: &str) -> Result<(), InspectorError> {
    let reference = reference.trim();
    if reference.is_empty()
        || Path::new(reference).is_absolute()
        || reference.split(['/', '\\']).any(|part| part == "..")
    {
        return Err(InspectorError::InvalidAssetReference(reference.to_owned()));
    }
    let extension = Path::new(reference)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension != "png" && extension != "jpg" && extension != "jpeg" && extension != "webp" {
        return Err(InspectorError::InvalidAssetReference(reference.to_owned()));
    }
    Ok(())
}

fn validate_lua_script(script: &str) -> Result<(), InspectorError> {
    let script = script.trim();
    if script.is_empty()
        || Path::new(script).is_absolute()
        || script.split(['/', '\\']).any(|part| part == "..")
        || Path::new(script)
            .extension()
            .and_then(|value| value.to_str())
            != Some("lua")
    {
        return Err(InspectorError::InvalidLuaScript(script.to_owned()));
    }
    Ok(())
}

#[derive(Debug)]
pub enum InspectorError {
    UnknownEntity(SceneEntityId),
    NothingSelected,
    EmptyName,
    NonFiniteTransform,
    InvalidScale,
    InvalidTransform(String),
    InvalidAssetReference(String),
    InvalidLuaScript(String),
}

impl fmt::Display for InspectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownEntity(id) => write!(f, "Inspector 中不存在实体 {}", id.0),
            Self::NothingSelected => write!(f, "Inspector 尚未选择实体"),
            Self::EmptyName => write!(f, "实体名称不能为空"),
            Self::NonFiniteTransform => write!(f, "Transform 必须是有限数值"),
            Self::InvalidScale => write!(f, "Transform 缩放必须大于 0"),
            Self::InvalidTransform(value) => write!(f, "Transform 数据无效：{value}"),
            Self::InvalidAssetReference(value) => write!(f, "Sprite 资源引用无效：{value}"),
            Self::InvalidLuaScript(value) => write!(f, "Lua 脚本路径无效：{value}"),
        }
    }
}
impl Error for InspectorError {}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::scene::SceneEntityId;

    fn setup() -> (SceneDocument, InspectorState, SceneEntityId) {
        let mut document = SceneDocument::new_scene("main");
        let id = SceneEntityId(1);
        document.scene.add_entity(id, "Player", None).unwrap();
        document.mark_clean();
        let mut inspector = InspectorState::new();
        inspector.select(&document, id).unwrap();
        (document, inspector, id)
    }

    #[test]
    fn select_and_snapshot_defaults() {
        let (document, inspector, id) = setup();
        let snapshot = inspector.snapshot(&document).unwrap();
        assert_eq!(snapshot.entity_id, id);
        assert_eq!(snapshot.transform, TransformFields::default());
        assert!(snapshot.sprite_reference.is_none());
    }
    #[test]
    fn name_edit_marks_dirty() {
        let (mut document, inspector, _) = setup();
        inspector.set_name(&mut document, "Hero").unwrap();
        assert!(document.is_dirty());
        assert_eq!(document.scene.entities[&SceneEntityId(1)].name, "Hero");
    }
    #[test]
    fn empty_name_rejected_without_mutation() {
        let (mut document, inspector, _) = setup();
        assert!(matches!(
            inspector.set_name(&mut document, " "),
            Err(InspectorError::EmptyName)
        ));
        assert_eq!(document.scene.entities[&SceneEntityId(1)].name, "Player");
        assert!(!document.is_dirty());
    }
    #[test]
    fn transform_position_x_accepts() {
        let (mut document, inspector, _) = setup();
        inspector
            .set_transform(
                &mut document,
                TransformFields {
                    position_x: 1.0,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(document.is_dirty());
    }
    #[test]
    fn transform_position_y_accepts() {
        let (mut document, inspector, _) = setup();
        inspector
            .set_transform(
                &mut document,
                TransformFields {
                    position_y: -2.0,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            inspector.snapshot(&document).unwrap().transform.position_y,
            -2.0
        );
    }
    #[test]
    fn transform_rotation_accepts() {
        let (mut document, inspector, _) = setup();
        inspector
            .set_transform(
                &mut document,
                TransformFields {
                    rotation: 3.0,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            inspector.snapshot(&document).unwrap().transform.rotation,
            3.0
        );
    }
    #[test]
    fn transform_scale_x_accepts() {
        let (mut document, inspector, _) = setup();
        inspector
            .set_transform(
                &mut document,
                TransformFields {
                    scale_x: 2.0,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            inspector.snapshot(&document).unwrap().transform.scale_x,
            2.0
        );
    }
    #[test]
    fn transform_scale_y_accepts() {
        let (mut document, inspector, _) = setup();
        inspector
            .set_transform(
                &mut document,
                TransformFields {
                    scale_y: 3.0,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            inspector.snapshot(&document).unwrap().transform.scale_y,
            3.0
        );
    }
    #[test]
    fn nan_transform_rejected() {
        let (mut document, inspector, _) = setup();
        assert!(matches!(
            inspector.set_transform(
                &mut document,
                TransformFields {
                    position_x: f32::NAN,
                    ..Default::default()
                }
            ),
            Err(InspectorError::NonFiniteTransform)
        ));
        assert!(!document.is_dirty());
    }
    #[test]
    fn infinity_transform_rejected() {
        let (mut document, inspector, _) = setup();
        assert!(
            inspector
                .set_transform(
                    &mut document,
                    TransformFields {
                        rotation: f32::INFINITY,
                        ..Default::default()
                    }
                )
                .is_err()
        );
    }
    #[test]
    fn zero_scale_rejected() {
        let (mut document, inspector, _) = setup();
        assert!(matches!(
            inspector.set_transform(
                &mut document,
                TransformFields {
                    scale_x: 0.0,
                    ..Default::default()
                }
            ),
            Err(InspectorError::InvalidScale)
        ));
    }
    #[test]
    fn negative_scale_rejected() {
        let (mut document, inspector, _) = setup();
        assert!(
            inspector
                .set_transform(
                    &mut document,
                    TransformFields {
                        scale_y: -1.0,
                        ..Default::default()
                    }
                )
                .is_err()
        );
    }
    #[test]
    fn sprite_png_accepts() {
        let (mut document, inspector, _) = setup();
        inspector
            .set_sprite_reference(&mut document, "textures/player.png")
            .unwrap();
        assert_eq!(
            inspector
                .snapshot(&document)
                .unwrap()
                .sprite_reference
                .as_deref(),
            Some("textures/player.png")
        );
    }
    #[test]
    fn sprite_jpg_accepts() {
        let (mut document, inspector, _) = setup();
        inspector
            .set_sprite_reference(&mut document, "textures/player.jpg")
            .unwrap();
        assert!(document.is_dirty());
    }
    #[test]
    fn sprite_empty_rejected() {
        let (mut document, inspector, _) = setup();
        assert!(inspector.set_sprite_reference(&mut document, "").is_err());
        assert!(!document.is_dirty());
    }
    #[test]
    fn sprite_traversal_rejected() {
        let (mut document, inspector, _) = setup();
        assert!(
            inspector
                .set_sprite_reference(&mut document, "../player.png")
                .is_err()
        );
    }
    #[test]
    fn sprite_wrong_extension_rejected() {
        let (mut document, inspector, _) = setup();
        assert!(
            inspector
                .set_sprite_reference(&mut document, "player.txt")
                .is_err()
        );
    }
    #[test]
    fn sprite_clear_marks_dirty() {
        let (mut document, inspector, _) = setup();
        inspector
            .set_sprite_reference(&mut document, "player.png")
            .unwrap();
        document.mark_clean();
        inspector.clear_sprite_reference(&mut document).unwrap();
        assert!(document.is_dirty());
    }
    #[test]
    fn lua_accepts() {
        let (mut document, inspector, _) = setup();
        inspector
            .set_lua_script(&mut document, "scripts/player.lua")
            .unwrap();
        assert_eq!(
            inspector.snapshot(&document).unwrap().lua_script.as_deref(),
            Some("scripts/player.lua")
        );
    }
    #[test]
    fn lua_wrong_extension_rejected() {
        let (mut document, inspector, _) = setup();
        assert!(
            inspector
                .set_lua_script(&mut document, "scripts/player.txt")
                .is_err()
        );
    }
    #[test]
    fn lua_traversal_rejected() {
        let (mut document, inspector, _) = setup();
        assert!(
            inspector
                .set_lua_script(&mut document, "../player.lua")
                .is_err()
        );
    }
    #[test]
    fn lua_clear_marks_dirty() {
        let (mut document, inspector, _) = setup();
        inspector
            .set_lua_script(&mut document, "player.lua")
            .unwrap();
        document.mark_clean();
        inspector.clear_lua_script(&mut document).unwrap();
        assert!(document.is_dirty());
    }
    #[test]
    fn save_reload_preserves_fields() {
        let (mut document, inspector, _) = setup();
        inspector
            .set_transform(
                &mut document,
                TransformFields {
                    position_x: 4.0,
                    position_y: 5.0,
                    rotation: 0.5,
                    scale_x: 2.0,
                    scale_y: 3.0,
                },
            )
            .unwrap();
        inspector
            .set_sprite_reference(&mut document, "player.png")
            .unwrap();
        inspector
            .set_lua_script(&mut document, "player.lua")
            .unwrap();
        let text = document.scene.to_text();
        let loaded = engine_core::scene::Scene::from_text(&text).unwrap();
        assert_eq!(
            loaded.entities[&SceneEntityId(1)].components,
            document.scene.entities[&SceneEntityId(1)].components
        );
    }
}
