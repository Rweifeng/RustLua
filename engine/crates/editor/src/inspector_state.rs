use crate::SceneDocument;
use engine::core::{
    camera3d::Camera3D,
    light::Light,
    math3d::{Quat, Vec3},
    mesh3d::MeshRenderer,
    physics3d::{ColliderDesc3d, ColliderShape3d},
    scene::{SceneEntity, SceneEntityId},
    sprite::SpriteComponent,
    transform3d::Transform3D,
    ui::{UiButton, UiImage, UiText},
};
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

/// Inspector 可编辑的 Transform3D 字段（无 3D Gizmo，仅查看与数值编辑）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform3DFields {
    pub position_x: f32,
    pub position_y: f32,
    pub position_z: f32,
    pub rotation_x: f32,
    pub rotation_y: f32,
    pub rotation_z: f32,
    pub rotation_w: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub scale_z: f32,
}

impl Default for Transform3DFields {
    fn default() -> Self {
        Self {
            position_x: 0.0,
            position_y: 0.0,
            position_z: 0.0,
            rotation_x: 0.0,
            rotation_y: 0.0,
            rotation_z: 0.0,
            rotation_w: 1.0,
            scale_x: 1.0,
            scale_y: 1.0,
            scale_z: 1.0,
        }
    }
}

/// Inspector 可编辑的 Camera3D 字段（aspect 由渲染窗口决定，不存储）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera3DFields {
    pub position_x: f32,
    pub position_y: f32,
    pub position_z: f32,
    pub target_x: f32,
    pub target_y: f32,
    pub target_z: f32,
    pub fov_degrees: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for Camera3DFields {
    fn default() -> Self {
        Self {
            position_x: 0.0,
            position_y: 0.0,
            position_z: 5.0,
            target_x: 0.0,
            target_y: 0.0,
            target_z: 0.0,
            fov_degrees: 90.0,
            near: 0.1,
            far: 100.0,
        }
    }
}

/// Inspector 可编辑的方向光字段。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightFields {
    pub direction_x: f32,
    pub direction_y: f32,
    pub direction_z: f32,
    pub color_r: f32,
    pub color_g: f32,
    pub color_b: f32,
    pub intensity: f32,
}

impl Default for LightFields {
    fn default() -> Self {
        Self {
            direction_x: 0.4,
            direction_y: 0.8,
            direction_z: 0.5,
            color_r: 1.0,
            color_g: 1.0,
            color_b: 1.0,
            intensity: 1.0,
        }
    }
}

/// Inspector 可编辑的 Collider3d 字段（M9-03）。
#[derive(Clone, Debug, PartialEq)]
pub struct Collider3dFields {
    /// `ball` | `cuboid` | `capsule`
    pub shape: String,
    pub radius: f32,
    pub half_width: f32,
    pub half_height: f32,
    pub half_z: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub offset_z: f32,
    pub sensor: bool,
    pub dynamic: bool,
}

impl Default for Collider3dFields {
    fn default() -> Self {
        Self {
            shape: "ball".to_owned(),
            radius: 1.0,
            half_width: 1.0,
            half_height: 1.0,
            half_z: 1.0,
            offset_x: 0.0,
            offset_y: 0.0,
            offset_z: 0.0,
            sensor: false,
            dynamic: true,
        }
    }
}

/// Inspector 可编辑的 Text（屏幕空间 UI 文本标签）字段。
#[derive(Clone, Debug, PartialEq)]
pub struct TextFields {
    pub text: String,
    pub space: String,
    pub anchor_x: f32,
    pub anchor_y: f32,
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub color_r: f32,
    pub color_g: f32,
    pub color_b: f32,
}

impl Default for TextFields {
    fn default() -> Self {
        Self {
            text: String::new(),
            space: "canvas".to_owned(),
            anchor_x: 0.0,
            anchor_y: 0.0,
            x: 10.0,
            y: 10.0,
            scale: 1.0,
            color_r: 1.0,
            color_g: 1.0,
            color_b: 1.0,
        }
    }
}

/// Inspector 可编辑的 Image（屏幕空间 UI 贴图）字段。
#[derive(Clone, Debug, PartialEq)]
pub struct ImageFields {
    pub texture: String,
    pub space: String,
    pub anchor_x: f32,
    pub anchor_y: f32,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub color_r: f32,
    pub color_g: f32,
    pub color_b: f32,
}

impl Default for ImageFields {
    fn default() -> Self {
        Self {
            texture: String::new(),
            space: "canvas".to_owned(),
            anchor_x: 0.0,
            anchor_y: 0.0,
            x: 10.0,
            y: 10.0,
            width: 64.0,
            height: 64.0,
            color_r: 1.0,
            color_g: 1.0,
            color_b: 1.0,
        }
    }
}

/// Inspector 可编辑的 Button（屏幕空间 UI 按钮）字段。
#[derive(Clone, Debug, PartialEq)]
pub struct ButtonFields {
    pub label: String,
    pub space: String,
    pub anchor_x: f32,
    pub anchor_y: f32,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub bg_r: f32,
    pub bg_g: f32,
    pub bg_b: f32,
    pub text_r: f32,
    pub text_g: f32,
    pub text_b: f32,
}

impl Default for ButtonFields {
    fn default() -> Self {
        Self {
            label: String::new(),
            space: "canvas".to_owned(),
            anchor_x: 0.0,
            anchor_y: 0.0,
            x: 10.0,
            y: 10.0,
            width: 100.0,
            height: 32.0,
            bg_r: 0.2,
            bg_g: 0.45,
            bg_b: 0.8,
            text_r: 1.0,
            text_g: 1.0,
            text_b: 1.0,
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
    /// 3D 组件（仅在实体携带对应组件时存在）。
    pub transform3d: Option<Transform3DFields>,
    pub mesh_renderer: Option<String>,
    pub camera3d: Option<Camera3DFields>,
    pub light: Option<LightFields>,
    pub collider3d: Option<Collider3dFields>,
    /// UI 组件（屏幕空间）。
    pub text: Option<TextFields>,
    pub image: Option<ImageFields>,
    pub button: Option<ButtonFields>,
    /// 运行时切场景不销毁。
    pub persistent: bool,
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

    pub fn set_transform3d(
        &self,
        document: &mut SceneDocument,
        transform: Transform3DFields,
    ) -> Result<(), InspectorError> {
        validate_transform3d(transform)?;
        let id = self.selected.ok_or(InspectorError::NothingSelected)?;
        let entity = entity_mut(document, id)?;
        let value = encode_transform3d(transform);
        if entity.components.get("Transform3D") != Some(&value) {
            entity.components.insert("Transform3D".into(), value);
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn set_mesh_renderer(
        &self,
        document: &mut SceneDocument,
        reference: impl Into<String>,
    ) -> Result<(), InspectorError> {
        let reference = reference.into();
        validate_mesh_renderer(&reference)?;
        let id = self.selected.ok_or(InspectorError::NothingSelected)?;
        let entity = entity_mut(document, id)?;
        let value = MeshRenderer::from_component_str(&format!("mesh={reference}"))
            .map_err(|error| InspectorError::InvalidMeshRenderer(error.to_string()))?
            .to_component_str();
        if entity.components.get("MeshRenderer") != Some(&value) {
            entity.components.insert("MeshRenderer".into(), value);
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn clear_mesh_renderer(&self, document: &mut SceneDocument) -> Result<(), InspectorError> {
        let id = self.selected.ok_or(InspectorError::NothingSelected)?;
        let entity = entity_mut(document, id)?;
        if entity.components.remove("MeshRenderer").is_some() {
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn set_camera3d(
        &self,
        document: &mut SceneDocument,
        camera: Camera3DFields,
    ) -> Result<(), InspectorError> {
        validate_camera3d(camera)?;
        let id = self.selected.ok_or(InspectorError::NothingSelected)?;
        let entity = entity_mut(document, id)?;
        let value = encode_camera3d(camera);
        if entity.components.get("Camera3D") != Some(&value) {
            entity.components.insert("Camera3D".into(), value);
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn set_light(
        &self,
        document: &mut SceneDocument,
        light: LightFields,
    ) -> Result<(), InspectorError> {
        validate_light(light)?;
        let id = self.selected.ok_or(InspectorError::NothingSelected)?;
        let entity = entity_mut(document, id)?;
        let value = encode_light(light);
        if entity.components.get("Light") != Some(&value) {
            entity.components.insert("Light".into(), value);
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn set_collider3d(
        &self,
        document: &mut SceneDocument,
        collider: Collider3dFields,
    ) -> Result<(), InspectorError> {
        validate_collider3d(&collider)?;
        let id = self.selected.ok_or(InspectorError::NothingSelected)?;
        let entity = entity_mut(document, id)?;
        let value = encode_collider3d(&collider);
        if entity.components.get("Collider3d") != Some(&value) {
            entity.components.insert("Collider3d".into(), value);
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn clear_collider3d(&self, document: &mut SceneDocument) -> Result<(), InspectorError> {
        let id = self.selected.ok_or(InspectorError::NothingSelected)?;
        let entity = entity_mut(document, id)?;
        if entity.components.remove("Collider3d").is_some() {
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn set_text(
        &self,
        document: &mut SceneDocument,
        text: TextFields,
    ) -> Result<(), InspectorError> {
        validate_text(&text)?;
        let id = self.selected.ok_or(InspectorError::NothingSelected)?;
        let entity = entity_mut(document, id)?;
        let value = encode_text(&text)?;
        if entity.components.get("Text") != Some(&value) {
            entity.components.insert("Text".into(), value);
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn clear_text(&self, document: &mut SceneDocument) -> Result<(), InspectorError> {
        let id = self.selected.ok_or(InspectorError::NothingSelected)?;
        let entity = entity_mut(document, id)?;
        if entity.components.remove("Text").is_some() {
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn set_image(
        &self,
        document: &mut SceneDocument,
        image: ImageFields,
    ) -> Result<(), InspectorError> {
        validate_image(&image)?;
        let id = self.selected.ok_or(InspectorError::NothingSelected)?;
        let entity = entity_mut(document, id)?;
        let value = encode_image(&image)?;
        if entity.components.get("Image") != Some(&value) {
            entity.components.insert("Image".into(), value);
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn clear_image(&self, document: &mut SceneDocument) -> Result<(), InspectorError> {
        let id = self.selected.ok_or(InspectorError::NothingSelected)?;
        let entity = entity_mut(document, id)?;
        if entity.components.remove("Image").is_some() {
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn set_button(
        &self,
        document: &mut SceneDocument,
        button: ButtonFields,
    ) -> Result<(), InspectorError> {
        validate_button(&button)?;
        let id = self.selected.ok_or(InspectorError::NothingSelected)?;
        let entity = entity_mut(document, id)?;
        let value = encode_button(&button)?;
        if entity.components.get("Button") != Some(&value) {
            entity.components.insert("Button".into(), value);
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn clear_button(&self, document: &mut SceneDocument) -> Result<(), InspectorError> {
        let id = self.selected.ok_or(InspectorError::NothingSelected)?;
        let entity = entity_mut(document, id)?;
        if entity.components.remove("Button").is_some() {
            document.mark_dirty();
        }
        Ok(())
    }

    /// 设置/清除「运行时不跟随场景切换销毁」标志（实体 metadata）。
    pub fn set_persistent(
        &self,
        document: &mut SceneDocument,
        enabled: bool,
    ) -> Result<(), InspectorError> {
        let id = self.selected.ok_or(InspectorError::NothingSelected)?;
        let entity = entity_mut(document, id)?;
        if enabled {
            if entity.metadata.get("persistent").map(String::as_str) != Some("true") {
                entity
                    .metadata
                    .insert("persistent".to_owned(), "true".to_owned());
                document.mark_dirty();
            }
        } else if entity.metadata.remove("persistent").is_some() {
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
    let transform3d = entity
        .components
        .get("Transform3D")
        .map_or(Ok(None), |value| decode_transform3d(value).map(Some))?;
    let camera3d = entity
        .components
        .get("Camera3D")
        .map_or(Ok(None), |value| decode_camera3d(value).map(Some))?;
    let light = entity
        .components
        .get("Light")
        .map_or(Ok(None), |value| decode_light(value).map(Some))?;
    Ok(InspectorSnapshot {
        entity_id: entity.id,
        name: entity.name.clone(),
        transform,
        sprite_reference: entity.components.get("Sprite").cloned(),
        lua_script: entity.components.get("LuaScript").cloned(),
        transform3d,
        mesh_renderer: entity
            .components
            .get("MeshRenderer")
            .and_then(|value| value.strip_prefix("mesh="))
            .map(str::to_owned),
        camera3d,
        light,
        collider3d: entity
            .components
            .get("Collider3d")
            .map_or(Ok(None), |value| decode_collider3d(value).map(Some))?,
        text: entity
            .components
            .get("Text")
            .map_or(Ok(None), |value| decode_text(value).map(Some))?,
        image: entity
            .components
            .get("Image")
            .map_or(Ok(None), |value| decode_image(value).map(Some))?,
        button: entity
            .components
            .get("Button")
            .map_or(Ok(None), |value| decode_button(value).map(Some))?,
        persistent: entity
            .metadata
            .get("persistent")
            .is_some_and(|value| value == "true"),
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

fn validate_transform3d(transform: Transform3DFields) -> Result<(), InspectorError> {
    let value = encode_transform3d(transform);
    Transform3D::from_component_str(&value)
        .map(|_| ())
        .map_err(|error| InspectorError::InvalidTransform3d(error.to_string()))
}

fn encode_transform3d(transform: Transform3DFields) -> String {
    Transform3D {
        position: Vec3::new(
            transform.position_x,
            transform.position_y,
            transform.position_z,
        ),
        rotation: Quat {
            x: transform.rotation_x,
            y: transform.rotation_y,
            z: transform.rotation_z,
            w: transform.rotation_w,
        },
        scale: Vec3::new(transform.scale_x, transform.scale_y, transform.scale_z),
    }
    .to_component_str()
}

fn decode_transform3d(value: &str) -> Result<Transform3DFields, InspectorError> {
    let transform = Transform3D::from_component_str(value)
        .map_err(|error| InspectorError::InvalidTransform3d(error.to_string()))?;
    Ok(Transform3DFields {
        position_x: transform.position.x,
        position_y: transform.position.y,
        position_z: transform.position.z,
        rotation_x: transform.rotation.x,
        rotation_y: transform.rotation.y,
        rotation_z: transform.rotation.z,
        rotation_w: transform.rotation.w,
        scale_x: transform.scale.x,
        scale_y: transform.scale.y,
        scale_z: transform.scale.z,
    })
}

fn validate_collider3d(collider: &Collider3dFields) -> Result<(), InspectorError> {
    let value = encode_collider3d(collider);
    ColliderDesc3d::from_component_str(&value)
        .map(|_| ())
        .map_err(|error| InspectorError::InvalidCollider3d(error.to_string()))
}

fn encode_collider3d(collider: &Collider3dFields) -> String {
    let mut parts = match collider.shape.as_str() {
        "cuboid" => format!(
            "shape=cuboid;half_width={:.6};half_height={:.6};half_z={:.6}",
            collider.half_width, collider.half_height, collider.half_z
        ),
        "capsule" => format!(
            "shape=capsule;half_height={:.6};radius={:.6}",
            collider.half_height, collider.radius
        ),
        _ => format!("shape=ball;radius={:.6}", collider.radius),
    };
    parts.push_str(&format!(
        ";offset={:.6},{:.6},{:.6}",
        collider.offset_x, collider.offset_y, collider.offset_z
    ));
    if collider.sensor {
        parts.push_str(";sensor=true");
    }
    if !collider.dynamic {
        parts.push_str(";dynamic=false");
    }
    parts
}

fn decode_collider3d(value: &str) -> Result<Collider3dFields, InspectorError> {
    let desc = ColliderDesc3d::from_component_str(value)
        .map_err(|error| InspectorError::InvalidCollider3d(error.to_string()))?;
    let mut fields = Collider3dFields {
        sensor: desc.sensor,
        dynamic: desc.dynamic,
        offset_x: desc.offset.x,
        offset_y: desc.offset.y,
        offset_z: desc.offset.z,
        ..Collider3dFields::default()
    };
    match desc.shape {
        ColliderShape3d::Ball(radius) => {
            fields.shape = "ball".to_owned();
            fields.radius = radius;
        }
        ColliderShape3d::Cuboid(half) => {
            fields.shape = "cuboid".to_owned();
            fields.half_width = half.x;
            fields.half_height = half.y;
            fields.half_z = half.z;
        }
        ColliderShape3d::Capsule {
            half_height,
            radius,
        } => {
            fields.shape = "capsule".to_owned();
            fields.half_height = half_height;
            fields.radius = radius;
        }
    }
    Ok(fields)
}

fn encode_text(fields: &TextFields) -> Result<String, InspectorError> {
    Ok(UiText {
        text: fields.text.clone(),
        position: [fields.x, fields.y],
        scale: fields.scale,
        color: [fields.color_r, fields.color_g, fields.color_b],
        space: parse_ui_layout_space(&fields.space).map_err(InspectorError::InvalidText)?,
        anchor: [fields.anchor_x, fields.anchor_y],
    }
    .to_component_str())
}

fn decode_text(value: &str) -> Result<TextFields, InspectorError> {
    let ui = UiText::from_component_str(value)
        .map_err(|error| InspectorError::InvalidText(error.to_string()))?;
    Ok(TextFields {
        text: ui.text,
        space: ui.space.as_str().to_owned(),
        anchor_x: ui.anchor[0],
        anchor_y: ui.anchor[1],
        x: ui.position[0],
        y: ui.position[1],
        scale: ui.scale,
        color_r: ui.color[0],
        color_g: ui.color[1],
        color_b: ui.color[2],
    })
}

fn validate_text(fields: &TextFields) -> Result<(), InspectorError> {
    let value = encode_text(fields)?;
    UiText::from_component_str(&value)
        .map(|_| ())
        .map_err(|error| InspectorError::InvalidText(error.to_string()))
}

fn encode_image(fields: &ImageFields) -> Result<String, InspectorError> {
    Ok(UiImage {
        texture: fields.texture.clone(),
        position: [fields.x, fields.y],
        size: [fields.width, fields.height],
        color: [fields.color_r, fields.color_g, fields.color_b],
        space: parse_ui_layout_space(&fields.space).map_err(InspectorError::InvalidImage)?,
        anchor: [fields.anchor_x, fields.anchor_y],
    }
    .to_component_str())
}

fn decode_image(value: &str) -> Result<ImageFields, InspectorError> {
    let ui = UiImage::from_component_str(value)
        .map_err(|error| InspectorError::InvalidImage(error.to_string()))?;
    Ok(ImageFields {
        texture: ui.texture,
        space: ui.space.as_str().to_owned(),
        anchor_x: ui.anchor[0],
        anchor_y: ui.anchor[1],
        x: ui.position[0],
        y: ui.position[1],
        width: ui.size[0],
        height: ui.size[1],
        color_r: ui.color[0],
        color_g: ui.color[1],
        color_b: ui.color[2],
    })
}

fn validate_image(fields: &ImageFields) -> Result<(), InspectorError> {
    let value = encode_image(fields)?;
    UiImage::from_component_str(&value)
        .map(|_| ())
        .map_err(|error| InspectorError::InvalidImage(error.to_string()))
}

fn encode_button(fields: &ButtonFields) -> Result<String, InspectorError> {
    Ok(UiButton {
        label: fields.label.clone(),
        position: [fields.x, fields.y],
        size: [fields.width, fields.height],
        background: [fields.bg_r, fields.bg_g, fields.bg_b],
        text_color: [fields.text_r, fields.text_g, fields.text_b],
        space: parse_ui_layout_space(&fields.space).map_err(InspectorError::InvalidButton)?,
        anchor: [fields.anchor_x, fields.anchor_y],
    }
    .to_component_str())
}

fn decode_button(value: &str) -> Result<ButtonFields, InspectorError> {
    let ui = UiButton::from_component_str(value)
        .map_err(|error| InspectorError::InvalidButton(error.to_string()))?;
    Ok(ButtonFields {
        label: ui.label,
        space: ui.space.as_str().to_owned(),
        anchor_x: ui.anchor[0],
        anchor_y: ui.anchor[1],
        x: ui.position[0],
        y: ui.position[1],
        width: ui.size[0],
        height: ui.size[1],
        bg_r: ui.background[0],
        bg_g: ui.background[1],
        bg_b: ui.background[2],
        text_r: ui.text_color[0],
        text_g: ui.text_color[1],
        text_b: ui.text_color[2],
    })
}

fn validate_button(fields: &ButtonFields) -> Result<(), InspectorError> {
    let value = encode_button(fields)?;
    UiButton::from_component_str(&value)
        .map(|_| ())
        .map_err(|error| InspectorError::InvalidButton(error.to_string()))
}

fn parse_ui_layout_space(value: &str) -> Result<engine::core::ui::UiLayoutSpace, String> {
    match value {
        "screen" => Ok(engine::core::ui::UiLayoutSpace::Screen),
        "canvas" => Ok(engine::core::ui::UiLayoutSpace::Canvas),
        _ => Err(format!("invalid UI layout space: {value}")),
    }
}

fn validate_camera3d(camera: Camera3DFields) -> Result<(), InspectorError> {
    let value = encode_camera3d(camera);
    Camera3D::from_component_str(&value)
        .map(|_| ())
        .map_err(|error| InspectorError::InvalidCamera3d(error.to_string()))
}

fn encode_camera3d(camera: Camera3DFields) -> String {
    Camera3D {
        position: Vec3::new(camera.position_x, camera.position_y, camera.position_z),
        target: Vec3::new(camera.target_x, camera.target_y, camera.target_z),
        up: Vec3::Y,
        fov_y: camera.fov_degrees.to_radians(),
        aspect_ratio: 1.0,
        near: camera.near,
        far: camera.far,
    }
    .to_component_str()
}

fn decode_camera3d(value: &str) -> Result<Camera3DFields, InspectorError> {
    let camera = Camera3D::from_component_str(value)
        .map_err(|error| InspectorError::InvalidCamera3d(error.to_string()))?;
    Ok(Camera3DFields {
        position_x: camera.position.x,
        position_y: camera.position.y,
        position_z: camera.position.z,
        target_x: camera.target.x,
        target_y: camera.target.y,
        target_z: camera.target.z,
        fov_degrees: camera.fov_y.to_degrees(),
        near: camera.near,
        far: camera.far,
    })
}

fn validate_light(light: LightFields) -> Result<(), InspectorError> {
    let value = encode_light(light);
    Light::from_component_str(&value)
        .map(|_| ())
        .map_err(|error| InspectorError::InvalidLight(error.to_string()))
}

fn encode_light(light: LightFields) -> String {
    Light {
        direction: Vec3::new(light.direction_x, light.direction_y, light.direction_z),
        color: [light.color_r, light.color_g, light.color_b],
        intensity: light.intensity,
    }
    .to_component_str()
}

fn decode_light(value: &str) -> Result<LightFields, InspectorError> {
    let light = Light::from_component_str(value)
        .map_err(|error| InspectorError::InvalidLight(error.to_string()))?;
    Ok(LightFields {
        direction_x: light.direction.x,
        direction_y: light.direction.y,
        direction_z: light.direction.z,
        color_r: light.color[0],
        color_g: light.color[1],
        color_b: light.color[2],
        intensity: light.intensity,
    })
}

fn validate_mesh_renderer(reference: &str) -> Result<(), InspectorError> {
    MeshRenderer::from_component_str(&format!("mesh={reference}"))
        .map(|_| ())
        .map_err(|error| InspectorError::InvalidMeshRenderer(error.to_string()))
}

fn validate_asset_reference(reference: &str) -> Result<(), InspectorError> {
    let reference = reference.trim();
    let component = SpriteComponent::from_component_str(reference)
        .map_err(|error| InspectorError::InvalidAssetReference(error.to_string()))?;
    let texture = component.texture.trim();
    if Path::new(texture).is_absolute() || texture.split(['/', '\\']).any(|part| part == "..") {
        return Err(InspectorError::InvalidAssetReference(texture.to_owned()));
    }
    let extension = Path::new(texture)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension != "png" && extension != "jpg" && extension != "jpeg" && extension != "webp" {
        return Err(InspectorError::InvalidAssetReference(texture.to_owned()));
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
    InvalidTransform3d(String),
    InvalidMeshRenderer(String),
    InvalidCamera3d(String),
    InvalidLight(String),
    InvalidCollider3d(String),
    InvalidText(String),
    InvalidImage(String),
    InvalidButton(String),
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
            Self::InvalidTransform3d(value) => write!(f, "Transform3D 数据无效：{value}"),
            Self::InvalidMeshRenderer(value) => write!(f, "MeshRenderer 资源引用无效：{value}"),
            Self::InvalidCamera3d(value) => write!(f, "Camera3D 数据无效：{value}"),
            Self::InvalidLight(value) => write!(f, "Light 数据无效：{value}"),
            Self::InvalidCollider3d(value) => write!(f, "Collider3d 数据无效：{value}"),
            Self::InvalidText(value) => write!(f, "Text 数据无效：{value}"),
            Self::InvalidImage(value) => write!(f, "Image 数据无效：{value}"),
            Self::InvalidButton(value) => write!(f, "Button 数据无效：{value}"),
        }
    }
}
impl Error for InspectorError {}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::core::scene::SceneEntityId;

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
    fn transform3d_set_and_snapshot() {
        let (mut document, inspector, _) = setup();
        let value = Transform3DFields {
            position_x: 1.0,
            position_y: 2.0,
            position_z: 3.0,
            scale_x: 2.0,
            scale_y: 2.0,
            scale_z: 2.0,
            ..Default::default()
        };
        inspector.set_transform3d(&mut document, value).unwrap();
        assert!(document.is_dirty());
        assert_eq!(
            inspector.snapshot(&document).unwrap().transform3d,
            Some(value)
        );
    }
    #[test]
    fn transform3d_rejects_zero_scale() {
        let (mut document, inspector, _) = setup();
        let value = Transform3DFields {
            scale_x: 0.0,
            ..Default::default()
        };
        assert!(inspector.set_transform3d(&mut document, value).is_err());
        assert!(!document.is_dirty());
    }
    #[test]
    fn transform3d_rejects_non_unit_rotation() {
        let (mut document, inspector, _) = setup();
        let value = Transform3DFields {
            rotation_w: 2.0,
            ..Default::default()
        };
        assert!(inspector.set_transform3d(&mut document, value).is_err());
    }
    #[test]
    fn mesh_renderer_set_and_clear() {
        let (mut document, inspector, _) = setup();
        inspector
            .set_mesh_renderer(&mut document, "models/cube.glb")
            .unwrap();
        assert_eq!(
            inspector
                .snapshot(&document)
                .unwrap()
                .mesh_renderer
                .as_deref(),
            Some("models/cube.glb")
        );
        document.mark_clean();
        inspector.clear_mesh_renderer(&mut document).unwrap();
        assert!(document.is_dirty());
        assert!(
            inspector
                .snapshot(&document)
                .unwrap()
                .mesh_renderer
                .is_none()
        );
    }
    #[test]
    fn mesh_renderer_rejects_non_glb() {
        let (mut document, inspector, _) = setup();
        assert!(
            inspector
                .set_mesh_renderer(&mut document, "models/cube.obj")
                .is_err()
        );
        assert!(!document.is_dirty());
    }
    #[test]
    fn camera3d_set_and_snapshot() {
        let (mut document, inspector, _) = setup();
        let value = Camera3DFields {
            position_x: 0.0,
            position_y: 3.0,
            position_z: 8.0,
            fov_degrees: 75.0,
            ..Default::default()
        };
        inspector.set_camera3d(&mut document, value).unwrap();
        let snapshot = inspector.snapshot(&document).unwrap();
        assert!(snapshot.camera3d.is_some());
        assert!((snapshot.camera3d.unwrap().fov_degrees - 75.0).abs() < 0.01);
    }
    #[test]
    fn camera3d_rejects_invalid_perspective() {
        let (mut document, inspector, _) = setup();
        let value = Camera3DFields {
            near: 10.0,
            far: 1.0,
            ..Default::default()
        };
        assert!(inspector.set_camera3d(&mut document, value).is_err());
    }
    #[test]
    fn light_set_and_snapshot() {
        let (mut document, inspector, _) = setup();
        let value = LightFields {
            direction_y: 1.0,
            intensity: 2.0,
            ..Default::default()
        };
        inspector.set_light(&mut document, value).unwrap();
        let snapshot = inspector.snapshot(&document).unwrap();
        let light = snapshot.light.unwrap();
        assert_eq!(light.direction_y, 1.0);
        assert_eq!(light.intensity, 2.0);
    }
    #[test]
    fn light_rejects_negative_intensity() {
        let (mut document, inspector, _) = setup();
        let value = LightFields {
            intensity: -1.0,
            ..Default::default()
        };
        assert!(inspector.set_light(&mut document, value).is_err());
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
        let loaded = engine::core::scene::Scene::from_text(&text).unwrap();
        assert_eq!(
            loaded.entities[&SceneEntityId(1)].components,
            document.scene.entities[&SceneEntityId(1)].components
        );
    }
    #[test]
    fn collider3d_roundtrip_via_snapshot() {
        let (mut document, inspector, _) = setup();
        inspector
            .set_collider3d(
                &mut document,
                Collider3dFields {
                    shape: "cuboid".to_owned(),
                    half_width: 4.0,
                    half_height: 5.0,
                    half_z: 6.0,
                    offset_x: 1.0,
                    offset_y: 2.0,
                    offset_z: 3.0,
                    sensor: true,
                    dynamic: false,
                    ..Collider3dFields::default()
                },
            )
            .unwrap();
        let snapshot = inspector.snapshot(&document).unwrap();
        let collider = snapshot.collider3d.expect("Collider3d present");
        assert_eq!(collider.shape, "cuboid");
        assert_eq!(collider.half_width, 4.0);
        assert_eq!(collider.half_height, 5.0);
        assert_eq!(collider.half_z, 6.0);
        assert_eq!(
            (collider.offset_x, collider.offset_y, collider.offset_z),
            (1.0, 2.0, 3.0)
        );
        assert!(collider.sensor);
        assert!(!collider.dynamic);
    }
    #[test]
    fn collider3d_rejects_invalid_size() {
        let (mut document, inspector, _) = setup();
        let value = Collider3dFields {
            shape: "ball".to_owned(),
            radius: 0.0,
            ..Collider3dFields::default()
        };
        assert!(
            matches!(
                inspector.set_collider3d(&mut document, value),
                Err(InspectorError::InvalidCollider3d(_))
            ),
            "zero radius must be rejected"
        );
        assert!(
            !document.scene.entities[&SceneEntityId(1)]
                .components
                .contains_key("Collider3d")
        );
    }
    #[test]
    fn collider3d_persists_through_scene_text() {
        let (mut document, inspector, _) = setup();
        inspector
            .set_collider3d(
                &mut document,
                Collider3dFields {
                    shape: "capsule".to_owned(),
                    half_height: 1.0,
                    radius: 0.5,
                    dynamic: true,
                    ..Collider3dFields::default()
                },
            )
            .unwrap();
        let text = document.scene.to_text();
        let loaded = engine::core::scene::Scene::from_text(&text).unwrap();
        let stored = loaded.entities[&SceneEntityId(1)]
            .components
            .get("Collider3d")
            .expect("Collider3d persisted");
        let decoded = decode_collider3d(stored).unwrap();
        assert_eq!(decoded.shape, "capsule");
        assert_eq!(decoded.half_height, 1.0);
        assert_eq!(decoded.radius, 0.5);
    }
}
