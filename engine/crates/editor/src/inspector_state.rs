use crate::SceneDocument;
use engine::core::{
    camera::Camera3D,
    entity_script::{EntityScript, SCRIPT_COMPONENT},
    math3d::{Quat, Vec3},
    physics3d::{ColliderDesc3d, ColliderShape3d},
    scene::{SceneEntity, SceneEntityId},
    sprite::SpriteComponent,
    transform::Transform3D,
    ui::{UiText},
};
use std::{error::Error, fmt, path::Path};

/// 统一 Transform 面板的两个编辑空间（WP9）。
///
/// docs/40 §3 WP9 要求"一个 Transform 面板按 2D/3D 编辑模式展示"，而编辑器此前是两个各自独立的
/// 折叠块（`Transform` 与 `Transform3D`）——同一件事在两个地方编辑。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum TransformSpace {
    TwoD,
    /// 实体同时有两种变换时的默认编辑空间。
    #[default]
    ThreeD,
}

impl TransformSpace {
    /// 面板实际该编辑哪个空间；`None` = 两种变换都没有，面板不显示。
    ///
    /// 规则：只有一种就按拥有者显示（此时模式开关没有意义），两种都有才听用户选。
    #[must_use]
    pub const fn resolve(has_2d: bool, has_3d: bool, mode: Self) -> Option<Self> {
        match (has_2d, has_3d) {
            (false, false) => None,
            (true, false) => Some(Self::TwoD),
            (false, true) => Some(Self::ThreeD),
            (true, true) => Some(mode),
        }
    }

    /// 两种变换是否都在（决定要不要显示模式开关）。
    #[must_use]
    pub const fn both_present(has_2d: bool, has_3d: bool) -> bool {
        has_2d && has_3d
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::TwoD => "2D",
            Self::ThreeD => "3D",
        }
    }
}

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
    /// WP5：姿态是否由实体的 `Transform3D` 提供（`pose=transform`）。Inspector 必须把这个
    /// 开关原样带回去，否则改任何一个相机字段都会把姿态来源悄悄复位。
    pub pose_from_transform: bool,
    /// docs/54 §4.2：相机是否启用（`enabled=false`）。同样必须原样带回——否则在 Inspector
    /// 里改任何一个相机字段都会把「我明明关掉了它」悄悄复位。
    pub enabled: bool,
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
            pose_from_transform: false,
            enabled: true,
        }
    }
}

// `Light` 的 Inspector 缓冲管线（`LightFields` + `set_light` + `encode/decode_light`）
// **已删除**：docs/55 阶段 2 把该组件迁到 **schema 生成**的字段编辑器
// （`panels/schema_inspector.rs` 按 `FieldKind` 选控件：方向 = DragValue×3、
// 颜色 = 取色器、强度 = DragValue），提交走 `EditorCommand::SetComponentField`
// → `PropertyPath` 写入，与其它组件同一条路径、同样"一次提交一条 Undo"。
//
// 保留这段说明是刻意的：下一次要给某个组件再加一套缓冲字段前，先确认它有没有 schema——
// 有 schema 就该走通用编辑器，而不是再写一遍手写面板。

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
    /// 像素字号（px）。
    pub font_size: f32,
    pub color_r: f32,
    pub color_g: f32,
    pub color_b: f32,
    /// 自定义字体（项目相对路径，如 assets/fonts/x.ttf）；空 = 项目默认字体。
    pub font: String,
}

impl Default for TextFields {
    fn default() -> Self {
        Self {
            text: String::new(),
            font_size: engine::core::ui::UI_TEXT_BASE_PX,
            color_r: 1.0,
            color_g: 1.0,
            color_b: 1.0,
            font: String::new(),
        }
    }
}

// `WorldText` 的 Inspector 缓冲管线（`WorldTextFields` + `set_world_text`/`clear_world_text` +
// `encode/decode/validate_world_text`）**已删除**（第 23 轮）：该组件迁到 schema 生成的字段编辑器，
// `font` 字段用**项目字体下拉**（`ChoiceSource::ProjectFonts`），其余用文本框/DragValue/取色器/勾选框。
// 注意：`WorldText` 的默认值（`text=文字;font_size=…;offset=0,-20;centered=true`）现在由
// `add_component` 直接用**引擎类型**的编码给出（`engine::core::world_text::WorldText`），
// 编辑器里不再有这份默认值。

// `Image` 的 Inspector 缓冲管线（`ImageFields` + `set_image`/`clear_image` +
// `encode/decode/validate_image`）**已删除**（第 22 轮）：该组件迁到 schema 生成的字段编辑器
// （`texture` 文本框、`color` 取色器、`nine_slice` 四个 DragValue）。
// 九宫格"非负"这条规则现在由 `FieldRule::NonNegative` 表达（对向量要求每个分量非负），
// 因此**审计**也一并获得这条校验；全零与省略的等价性由渲染侧的过滤
// （`any(|v| *v > 0.0)`）保证。

/// E16：Inspector 可编辑的 `UiLayout`（四边 Anchor+Offset）字段。
#[derive(Clone, Debug, PartialEq)]
pub struct UiLayoutFields {
    pub anchor_l: f32,
    pub anchor_t: f32,
    pub anchor_r: f32,
    pub anchor_b: f32,
    pub offset_l: f32,
    pub offset_t: f32,
    pub offset_r: f32,
    pub offset_b: f32,
    pub min_w: f32,
    pub min_h: f32,
    pub max_w: f32,
    pub max_h: f32,
    pub pivot_x: f32,
    pub pivot_y: f32,
}

impl UiLayoutFields {
    /// Buffer ↔ 引擎值对象转换（Inspector 与运行时共享同一解析）。
    pub fn to_layout(&self) -> engine::core::ui_layout::UiRectLayout {
        engine::core::ui_layout::UiRectLayout {
            anchors: [self.anchor_l, self.anchor_t, self.anchor_r, self.anchor_b],
            offsets: [self.offset_l, self.offset_t, self.offset_r, self.offset_b],
            min_size: [self.min_w, self.min_h],
            max_size: [self.max_w, self.max_h],
            pivot: [self.pivot_x, self.pivot_y],
        }
    }

    pub fn from_layout(layout: engine::core::ui_layout::UiRectLayout) -> Self {
        Self {
            anchor_l: layout.anchors[0],
            anchor_t: layout.anchors[1],
            anchor_r: layout.anchors[2],
            anchor_b: layout.anchors[3],
            offset_l: layout.offsets[0],
            offset_t: layout.offsets[1],
            offset_r: layout.offsets[2],
            offset_b: layout.offsets[3],
            min_w: layout.min_size[0],
            min_h: layout.min_size[1],
            max_w: layout.max_size[0],
            max_h: layout.max_size[1],
            pivot_x: layout.pivot[0],
            pivot_y: layout.pivot[1],
        }
    }
}

impl Default for UiLayoutFields {
    fn default() -> Self {
        Self::from_layout(engine::core::ui_layout::UiRectLayout::default())
    }
}

/// E17：Inspector 可编辑的 `Container`（自动布局容器）字段。
#[derive(Clone, Debug, PartialEq)]
pub struct ContainerFields {
    /// `vbox|hbox|grid|margin|center`。
    pub kind: String,
    pub padding_l: f32,
    pub padding_t: f32,
    pub padding_r: f32,
    pub padding_b: f32,
    pub spacing: f32,
    /// Grid 列数；0 = 自动。
    pub columns: i32,
    pub stretch: bool,
    pub fill: bool,
    /// `start|center|end`。
    pub align: String,
}

impl ContainerFields {
    pub fn to_style(&self) -> Result<engine::core::ui_layout::ContainerStyle, String> {
        let kind = engine::core::ui_layout::ContainerKind::parse(&self.kind)
            .ok_or_else(|| format!("未知 Container kind：{}", self.kind))?;
        let align = engine::core::ui_layout::ChildAlign::parse(&self.align)
            .ok_or_else(|| format!("未知对齐：{}", self.align))?;
        Ok(engine::core::ui_layout::ContainerStyle {
            kind,
            padding: [
                self.padding_l,
                self.padding_t,
                self.padding_r,
                self.padding_b,
            ],
            spacing: self.spacing,
            columns: self.columns.max(0) as usize,
            stretch: self.stretch,
            fill: self.fill,
            align,
        })
    }

    pub fn from_style(style: engine::core::ui_layout::ContainerStyle) -> Self {
        Self {
            kind: style.kind.as_str().to_owned(),
            padding_l: style.padding[0],
            padding_t: style.padding[1],
            padding_r: style.padding[2],
            padding_b: style.padding[3],
            spacing: style.spacing,
            columns: style.columns as i32,
            stretch: style.stretch,
            fill: style.fill,
            align: style.align.as_str().to_owned(),
        }
    }
}

impl Default for ContainerFields {
    fn default() -> Self {
        Self::from_style(engine::core::ui_layout::ContainerStyle::default())
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
    // `MeshRenderer`（mesh 字段）**不在快照里**：它已迁到 schema 生成的通用字段编辑器
    // （走 `EditorCommand::SetComponentField`，语义规则见 `FieldRule::ProjectPath`）。
    pub camera3d: Option<Camera3DFields>,
    /// `Light` **不在快照里**：它已迁到 schema 生成的通用字段编辑器，提交走
    /// `EditorCommand::SetComponentField`（见 `LightFields` 删除处的说明）。
    pub collider3d: Option<Collider3dFields>,
    /// UI 组件（屏幕空间）。
    pub text: Option<TextFields>,
    /// `WorldText` 与 `Image` **不在快照里**：两者都已迁到 schema 生成的字段编辑器
    /// （第 22/23 轮），提交走 `EditorCommand::SetComponentField`。
    pub ui_layout: Option<UiLayoutFields>,
    pub container: Option<ContainerFields>,
    /// 运行时切场景不销毁。
    pub persistent: bool,
}

/// Inspector 对单个实体进行读取/编辑的**无状态**文档助手（E03）。
///
/// E03 之后本类型不再持有“当前选中实体”：选择状态统一收敛到
/// [`crate::SelectionModel`]。所有读/写方法显式传入目标实体 ID，
/// 由调用方（`EditorApp`）从统一选择模型读取主选中后传入。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InspectorState {}

impl InspectorState {
    pub fn new() -> Self {
        Self {}
    }

    /// 读取 `id` 的 Inspector 快照（Inspector 面板同步字段的单一数据源）。
    pub fn snapshot(
        &self,
        document: &SceneDocument,
        id: SceneEntityId,
    ) -> Result<InspectorSnapshot, InspectorError> {
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
        id: SceneEntityId,
        name: impl Into<String>,
    ) -> Result<(), InspectorError> {
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
        id: SceneEntityId,
        transform: TransformFields,
    ) -> Result<(), InspectorError> {
        validate_transform(transform)?;
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
        id: SceneEntityId,
        reference: impl Into<String>,
    ) -> Result<(), InspectorError> {
        let reference = reference.into();
        validate_asset_reference(&reference)?;
        let sprite = engine::core::sprite::SpriteComponent::from_component_str(&reference)
            .map_err(|error| InspectorError::InvalidAssetReference(error.to_string()))?;
        let entity = entity_mut(document, id)?;
        let value = sprite.to_component_str();
        if entity.components.get("Sprite") != Some(&value) {
            entity.components.insert("Sprite".into(), value);
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn clear_sprite_reference(
        &self,
        document: &mut SceneDocument,
        id: SceneEntityId,
    ) -> Result<(), InspectorError> {
        let entity = entity_mut(document, id)?;
        if entity.components.remove("Sprite").is_some() {
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn set_lua_script(        &self,
        document: &mut SceneDocument,
        id: SceneEntityId,
        script: impl Into<String>,
    ) -> Result<(), InspectorError> {
        let script = script.into();
        validate_lua_script(&script)?;
        // 加载器/运行时读的是 `Script` + `file=<路径>`；这里写出**同一个**名字与载荷形式
        // （旧写法 `LuaScript` + 裸路径运行时根本不认，见 `core::component_migration`）。
        let value = entity_script_value(&script);
        let entity = entity_mut(document, id)?;
        if entity.components.get(SCRIPT_COMPONENT) != Some(&value) {
            entity.components.insert(SCRIPT_COMPONENT.to_owned(), value);
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn clear_lua_script(
        &self,
        document: &mut SceneDocument,
        id: SceneEntityId,
    ) -> Result<(), InspectorError> {
        let entity = entity_mut(document, id)?;
        let mut changed = entity.components.remove(SCRIPT_COMPONENT).is_some();
        // 旧键也顺手清掉，避免"清了脚本但旧键还在"。
        changed |= entity
            .components
            .remove(engine::core::component_migration::LEGACY_LUA_SCRIPT_COMPONENT)
            .is_some();
        if changed {
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn set_transform3d(
        &self,
        document: &mut SceneDocument,
        id: SceneEntityId,
        transform: Transform3DFields,
    ) -> Result<(), InspectorError> {
        validate_transform3d(transform)?;
        let entity = entity_mut(document, id)?;
        let value = encode_transform3d(transform);
        if entity.components.get("Transform3D") != Some(&value) {
            entity.components.insert("Transform3D".into(), value);
            document.mark_dirty();
        }
        Ok(())
    }

    // `set_mesh_renderer` / `clear_mesh_renderer` **已删除**（第 21 轮）：
    // `MeshRenderer` 与 `Light` 一样迁到 schema 生成的字段编辑器。
    // mesh 字段的语义规则（非空 / 工程内相对 / `.glb`|`.gltf` / 内置哨兵
    // `__ground_plane__`）现在由 `ComponentSchema` 的 `FieldRule::ProjectPath`
    // 表达，并且与 `mesh3d::validate_mesh_path` 由对拍测试盯住——比"编辑器里再写一遍校验"
    // 更严格，因为它同时作用于**审计**（真实场景文件）。

    pub fn set_camera3d(
        &self,
        document: &mut SceneDocument,
        id: SceneEntityId,
        camera: Camera3DFields,
    ) -> Result<(), InspectorError> {
        validate_camera3d(camera)?;
        let entity = entity_mut(document, id)?;
        let value = encode_camera3d(camera);
        if entity.components.get("Camera3D") != Some(&value) {
            entity.components.insert("Camera3D".into(), value);
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn set_collider3d(
        &self,
        document: &mut SceneDocument,
        id: SceneEntityId,
        collider: Collider3dFields,
    ) -> Result<(), InspectorError> {
        validate_collider3d(&collider)?;
        let entity = entity_mut(document, id)?;
        let value = encode_collider3d(&collider);
        if entity.components.get("Collider3d") != Some(&value) {
            entity.components.insert("Collider3d".into(), value);
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn clear_collider3d(
        &self,
        document: &mut SceneDocument,
        id: SceneEntityId,
    ) -> Result<(), InspectorError> {
        let entity = entity_mut(document, id)?;
        if entity.components.remove("Collider3d").is_some() {
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn set_text(
        &self,
        document: &mut SceneDocument,
        id: SceneEntityId,
        text: TextFields,
    ) -> Result<(), InspectorError> {
        validate_text(&text)?;
        let entity = entity_mut(document, id)?;
        let value = encode_text(&text)?;
        if entity.components.get("Text") != Some(&value) {
            entity.components.insert("Text".into(), value);
            document.mark_dirty();
        }
        Ok(())
    }

    pub fn clear_text(
        &self,
        document: &mut SceneDocument,
        id: SceneEntityId,
    ) -> Result<(), InspectorError> {
        let entity = entity_mut(document, id)?;
        if entity.components.remove("Text").is_some() {
            document.mark_dirty();
        }
        Ok(())
    }

    // `set_world_text` / `clear_world_text` **已删除**（第 23 轮）：`WorldText` 迁到 schema
    // 生成的字段编辑器——`font` 字段用**项目字体下拉**（`ChoiceSource::ProjectFonts`），
    // 其余字段用文本框 / DragValue / 取色器 / 勾选框。提交走 `EditorCommand::SetComponentField`。

    // `set_image` / `clear_image` **已删除**（第 22 轮）：`Image` 与 `Light`/`MeshRenderer`
    // 一样迁到 schema 生成的字段编辑器（`texture` 文本框、`color` 取色器、`nine_slice` 四个
    // DragValue），提交走 `EditorCommand::SetComponentField`。

    /// E16：写回 `UiLayout` 组件（四边 Anchor+Offset 布局）。
    pub fn set_ui_layout(
        &self,
        document: &mut SceneDocument,
        id: SceneEntityId,
        fields: UiLayoutFields,
    ) -> Result<(), InspectorError> {
        let layout = fields.to_layout();
        validate_unit_pair(&layout.pivot, "pivot")?;
        validate_unit_quadruple(&layout.anchors, "anchor")?;
        let entity = entity_mut(document, id)?;
        let value = layout.to_component_str();
        if entity.components.get("UiLayout") != Some(&value) {
            entity.components.insert("UiLayout".into(), value);
            document.mark_dirty();
        }
        Ok(())
    }

    /// E17：写回 `Container` 组件（自动布局样式）。
    pub fn set_container(
        &self,
        document: &mut SceneDocument,
        id: SceneEntityId,
        fields: ContainerFields,
    ) -> Result<(), InspectorError> {
        let style = fields
            .to_style()
            .map_err(InspectorError::InvalidContainer)?;
        let entity = entity_mut(document, id)?;
        let value = style.to_component_str();
        if entity.components.get("Container") != Some(&value) {
            entity.components.insert("Container".into(), value);
            document.mark_dirty();
        }
        Ok(())
    }

    /// 设置/清除「运行时不跟随场景切换销毁」标志（实体 metadata）。
    pub fn set_persistent(
        &self,
        document: &mut SceneDocument,
        id: SceneEntityId,
        enabled: bool,
    ) -> Result<(), InspectorError> {
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

    /// E08：把一份完整 Inspector 快照**直写**文档（不建 history、不落盘）。
    ///
    /// 语义与 `apply_inspector` 的 `history.execute` 闭包完全一致，供「Scene View
    /// 拖拽等活动事务进行中」把 Inspector 数值编辑并入事务 after：`before` 已是
    /// 拖动前快照，undo 一次即可同时回滚拖动与数值编辑。
    ///
    /// - `tilemap_component` / `animation_component`：仅当实体已携带对应组件时传入
    ///   编码串（None = 不改动）。
    /// - `multi_sprite_ids`：多选时除主选外同样具有 Sprite 的实体（与离散路径一致）。
    ///
    /// 返回是否发生了内容变化（调用方据此决定是否需要保存/回读）。
    #[allow(clippy::too_many_arguments)]
    pub fn write_inspector(
        &self,
        document: &mut SceneDocument,
        id: SceneEntityId,
        snapshot: InspectorSnapshot,
        tilemap_component: Option<&str>,
        animation_component: Option<&str>,
        multi_sprite_ids: &[SceneEntityId],
    ) -> Result<bool, InspectorError> {
        if snapshot.name.trim().is_empty() {
            return Err(InspectorError::EmptyName);
        }
        let before = document.clone();
        self.set_name(document, id, snapshot.name)?;
        self.set_transform(document, id, snapshot.transform)?;
        for sid in multi_sprite_ids {
            if *sid == id {
                continue;
            }
            if document
                .scene
                .entities
                .get(sid)
                .is_some_and(|entity| entity.components.contains_key("Transform"))
            {
                self.set_transform(document, *sid, snapshot.transform)?;
            }
        }
        if let Some(transform3d) = snapshot.transform3d {
            self.set_transform3d(document, id, transform3d)?;
            for sid in multi_sprite_ids {
                if *sid == id {
                    continue;
                }
                if document
                    .scene
                    .entities
                    .get(sid)
                    .is_some_and(|entity| entity.components.contains_key("Transform3D"))
                {
                    self.set_transform3d(document, *sid, transform3d)?;
                }
            }
        }
        if let Some(camera3d) = snapshot.camera3d {
            self.set_camera3d(document, id, camera3d)?;
            for sid in multi_sprite_ids {
                if *sid != id
                    && document
                        .scene
                        .entities
                        .get(sid)
                        .is_some_and(|entity| entity.components.contains_key("Camera3D"))
                {
                    self.set_camera3d(document, *sid, camera3d)?;
                }
            }
        }
        if let Some(collider3d) = snapshot.collider3d {
            self.set_collider3d(document, id, collider3d.clone())?;
            for sid in multi_sprite_ids {
                if *sid != id
                    && document
                        .scene
                        .entities
                        .get(sid)
                        .is_some_and(|entity| entity.components.contains_key("Collider3d"))
                {
                    self.set_collider3d(document, *sid, collider3d.clone())?;
                }
            }
        }
        if let Some(text) = snapshot.text {
            self.set_text(document, id, text.clone())?;
            for sid in multi_sprite_ids {
                if *sid == id {
                    continue;
                }
                if document
                    .scene
                    .entities
                    .get(sid)
                    .is_some_and(|entity| entity.components.contains_key("Text"))
                {
                    self.set_text(document, *sid, text.clone())?;
                }
            }
        }
        // E16/E17：布局组件单实体写回（不做多选批量——布局逐实体差异大，
        // 批量覆盖意义有限，与 Tilemap/Animation 同策略）。
        if let Some(ui_layout) = snapshot.ui_layout {
            self.set_ui_layout(document, id, ui_layout)?;
        }
        if let Some(container) = snapshot.container {
            self.set_container(document, id, container)?;
        }
        match snapshot.sprite_reference.as_deref() {
            Some(reference) if !reference.trim().is_empty() => {
                self.set_sprite_reference(document, id, reference)?;
                let sprite = reference.to_owned();
                for sid in multi_sprite_ids {
                    if *sid == id {
                        continue;
                    }
                    if let Some(entity) = document.scene.entities.get_mut(sid)
                        && entity.components.contains_key("Sprite")
                    {
                        entity
                            .components
                            .insert("Sprite".to_owned(), sprite.clone());
                        document.mark_dirty();
                    }
                }
            }
            _ => {
                self.clear_sprite_reference(document, id)?;
            }
        }
        if let Some(tilemap) = tilemap_component
            && let Some(entity) = document.scene.entities.get_mut(&id)
        {
            entity
                .components
                .insert("Tilemap".to_owned(), tilemap.to_owned());
            document.mark_dirty();
        }
        if let Some(animation) = animation_component
            && let Some(entity) = document.scene.entities.get_mut(&id)
        {
            entity
                .components
                .insert("Animation".to_owned(), animation.to_owned());
            document.mark_dirty();
        }
        match snapshot.lua_script.as_deref() {
            Some(script) => self.set_lua_script(document, id, script)?,
            None => {
                self.clear_lua_script(document, id)?;
            }
        }
        if let Some(script) = snapshot.lua_script.as_deref() {
            for sid in multi_sprite_ids {
                if *sid != id
                    && document
                        .scene
                        .entities
                        .get(sid)
                        .is_some_and(|entity| entity.components.contains_key(SCRIPT_COMPONENT))
                {
                    self.set_lua_script(document, *sid, script)?;
                }
            }
        }
        Ok(*document != before)
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
    Ok(InspectorSnapshot {
        entity_id: entity.id,
        name: entity.name.clone(),
        transform,
        sprite_reference: entity.components.get("Sprite").cloned(),
        lua_script: read_lua_script(entity),
        transform3d,
        camera3d,
        collider3d: entity
            .components
            .get("Collider3d")
            .map_or(Ok(None), |value| decode_collider3d(value).map(Some))?,
        text: entity
            .components
            .get("Text")
            .map_or(Ok(None), |value| decode_text(value).map(Some))?,
        ui_layout: entity
            .components
            .get("UiLayout")
            .map_or(Ok(None), |value| {
                engine::core::ui_layout::UiRectLayout::from_component_str(value)
                    .map(|layout| Some(UiLayoutFields::from_layout(layout)))
                    .map_err(InspectorError::InvalidUiLayout)
            })?,
        container: entity
            .components
            .get("Container")
            .map_or(Ok(None), |value| {
                engine::core::ui_layout::ContainerStyle::from_component_str(value)
                    .map(|style| Some(ContainerFields::from_style(style)))
                    .map_err(InspectorError::InvalidContainer)
            })?,
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
        font_size: fields.font_size,
        color: [fields.color_r, fields.color_g, fields.color_b],
        font: fields.font.clone(),
    }
    .to_component_str())
}

fn decode_text(value: &str) -> Result<TextFields, InspectorError> {
    let ui = UiText::from_component_str(value)
        .map_err(|error| InspectorError::InvalidText(error.to_string()))?;
    Ok(TextFields {
        text: ui.text,
        font_size: ui.font_size,
        color_r: ui.color[0],
        color_g: ui.color[1],
        color_b: ui.color[2],
        font: ui.font,
    })
}

fn validate_text(fields: &TextFields) -> Result<(), InspectorError> {
    let value = encode_text(fields)?;
    UiText::from_component_str(&value)
        .map(|_| ())
        .map_err(|error| InspectorError::InvalidText(error.to_string()))
}

/// E16：校验归一化二元组（pivot）处于 `0..=1`。
fn validate_unit_pair(values: &[f32; 2], name: &str) -> Result<(), InspectorError> {
    for (index, value) in values.iter().enumerate() {
        if !value.is_finite() || !(0.0..=1.0).contains(value) {
            return Err(InspectorError::InvalidUiLayout(format!(
                "{name}[{index}] 必须在 0..=1：{value}"
            )));
        }
    }
    Ok(())
}

/// E16：校验归一化四元组（四边 anchor）处于 `0..=1`。
fn validate_unit_quadruple(values: &[f32; 4], name: &str) -> Result<(), InspectorError> {
    for (index, value) in values.iter().enumerate() {
        if !value.is_finite() || !(0.0..=1.0).contains(value) {
            return Err(InspectorError::InvalidUiLayout(format!(
                "{name}[{index}] 必须在 0..=1：{value}"
            )));
        }
    }
    Ok(())
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
        pose_from_transform: camera.pose_from_transform,
        enabled: camera.enabled,
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
        pose_from_transform: camera.pose_from_transform,
        enabled: camera.enabled,
    })
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

/// 面板输入校验：**安全规则复用引擎**（工程内相对路径），**项目约定额外要求 `.lua`**。
///
/// 这两层刻意分开：引擎的 `EntityScript::validate` 只管"能不能逃出工程根"（运行时是
/// `root.join(file)` 后直接编译，扩展名不参与加载），编辑器则要求脚本目录的 `.lua` 约定。
/// 差异由 `lua_wrong_extension_rejected` 与引擎侧 `field_rules_match_engine_validators`
/// 两侧测试分别钉住，避免"以为两边一样"。
fn validate_lua_script(script: &str) -> Result<(), InspectorError> {
    let script = script.trim();
    let path_is_safe = EntityScript {
        file: script.to_owned(),
        enabled: true,
    }
    .validate()
    .is_ok();
    let extension_is_lua = Path::new(script)
        .extension()
        .and_then(|value| value.to_str())
        == Some("lua");
    if !path_is_safe || !extension_is_lua {
        return Err(InspectorError::InvalidLuaScript(script.to_owned()));
    }
    Ok(())
}

/// Inspector 里的"Lua"文本框 ↔ `Script` 组件载荷（`file=<路径>`）。
///
/// 面板显示**裸路径**（用户关心的是文件），组件串按运行时格式写；这样两边的差异只在这两个
/// 函数里，不会再出现"面板写裸路径、运行时读不懂"的分裂。
fn entity_script_value(script: &str) -> String {
    EntityScript {
        file: script.trim().to_owned(),
        enabled: true,
    }
    .to_component_str()
}

/// 读回脚本路径：优先 `Script`（含 `file=` 形式），兼容旧的 `LuaScript` 裸路径写法
/// （正常路径下 [`engine::core::component_migration`] 已经在打开场景时迁移过，
/// 这里只是不让"未经迁移的实体内存对象"显示成空）。
fn read_lua_script(entity: &SceneEntity) -> Option<String> {
    if let Some(value) = entity.components.get(SCRIPT_COMPONENT) {
        return match EntityScript::from_component_str(value) {
            Ok(script) => Some(script.file),
            // 解析不了就原样显示，让用户看到问题而不是空白。
            Err(_) => Some(value.clone()),
        };
    }
    entity
        .components
        .get(engine::core::component_migration::LEGACY_LUA_SCRIPT_COMPONENT)
        .map(|value| {
            value
                .strip_prefix("file=")
                .map_or_else(|| value.clone(), str::to_owned)
        })
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
    InvalidWorldText(String),
    InvalidImage(String),
    InvalidButton(String),
    InvalidUiLayout(String),
    InvalidContainer(String),
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
            Self::InvalidWorldText(value) => write!(f, "WorldText 数据无效：{value}"),
            Self::InvalidImage(value) => write!(f, "Image 数据无效：{value}"),
            Self::InvalidButton(value) => write!(f, "Button 数据无效：{value}"),
            Self::InvalidUiLayout(value) => write!(f, "UiLayout 数据无效：{value}"),
            Self::InvalidContainer(value) => write!(f, "Container 数据无效：{value}"),
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
        (document, InspectorState::new(), id)
    }

    /// WP9 统一 Transform 面板：面板该编哪个空间只由"实体拥有哪些变换"与"用户选的模式"决定。
    /// 两条关键性质：只有一种时**不听模式**（否则切到 3D 会让 2D-only 实体的面板空掉），
    /// 两种都没有时面板不显示。
    #[test]
    fn unified_transform_panel_resolves_the_edited_space() {
        assert_eq!(
            TransformSpace::resolve(false, false, TransformSpace::TwoD),
            None
        );
        assert_eq!(
            TransformSpace::resolve(false, false, TransformSpace::ThreeD),
            None
        );
        assert_eq!(
            TransformSpace::resolve(true, false, TransformSpace::ThreeD),
            Some(TransformSpace::TwoD),
            "只有 2D 变换时按拥有者显示，不看模式"
        );
        assert_eq!(
            TransformSpace::resolve(false, true, TransformSpace::TwoD),
            Some(TransformSpace::ThreeD),
            "只有 3D 变换时按拥有者显示，不看模式"
        );
        assert_eq!(
            TransformSpace::resolve(true, true, TransformSpace::TwoD),
            Some(TransformSpace::TwoD)
        );
        assert_eq!(
            TransformSpace::resolve(true, true, TransformSpace::ThreeD),
            Some(TransformSpace::ThreeD)
        );
        assert!(TransformSpace::both_present(true, true));
        assert!(!TransformSpace::both_present(true, false));
        assert!(!TransformSpace::both_present(false, true));
        assert_eq!(TransformSpace::default(), TransformSpace::ThreeD);
    }

    #[test]
    fn snapshot_defaults() {
        let (document, inspector, id) = setup();
        let snapshot = inspector.snapshot(&document, id).unwrap();
        assert_eq!(snapshot.entity_id, id);
        assert_eq!(snapshot.transform, TransformFields::default());
        assert!(snapshot.sprite_reference.is_none());
    }

    #[test]
    fn write_inspector_batches_transform_only_to_matching_entities() {
        let (mut document, inspector, id) = setup();
        let matching = SceneEntityId(2);
        let unrelated = SceneEntityId(3);
        document
            .scene
            .add_entity(matching, "Matching", None)
            .unwrap();
        document
            .scene
            .add_entity(unrelated, "Unrelated", None)
            .unwrap();
        inspector
            .set_transform(&mut document, id, TransformFields::default())
            .unwrap();
        inspector
            .set_transform(&mut document, matching, TransformFields::default())
            .unwrap();
        document.mark_clean();
        let mut snapshot = inspector.snapshot(&document, id).unwrap();
        snapshot.transform.position_x = 12.0;

        inspector
            .write_inspector(
                &mut document,
                id,
                snapshot,
                None,
                None,
                &[id, matching, unrelated],
            )
            .unwrap();
        let matching_transform = inspector.snapshot(&document, matching).unwrap().transform;
        assert_eq!(matching_transform.position_x, 12.0);
        assert!(
            !document.scene.entities[&unrelated]
                .components
                .contains_key("Transform")
        );
    }

    #[test]
    fn write_inspector_batches_transform3d_only_to_matching_entities() {
        let (mut document, inspector, id) = setup();
        let matching = SceneEntityId(2);
        let unrelated = SceneEntityId(3);
        document
            .scene
            .add_entity(matching, "Matching", None)
            .unwrap();
        document
            .scene
            .add_entity(unrelated, "Unrelated", None)
            .unwrap();
        inspector
            .set_transform(&mut document, id, TransformFields::default())
            .unwrap();
        inspector
            .set_transform3d(&mut document, id, Transform3DFields::default())
            .unwrap();
        inspector
            .set_transform3d(&mut document, matching, Transform3DFields::default())
            .unwrap();
        document.mark_clean();
        let mut snapshot = inspector.snapshot(&document, id).unwrap();
        let mut transform3d = snapshot.transform3d.unwrap();
        transform3d.position_z = 9.0;
        snapshot.transform3d = Some(transform3d);

        inspector
            .write_inspector(
                &mut document,
                id,
                snapshot,
                None,
                None,
                &[id, matching, unrelated],
            )
            .unwrap();
        let matching_transform = inspector
            .snapshot(&document, matching)
            .unwrap()
            .transform3d
            .unwrap();
        assert_eq!(matching_transform.position_z, 9.0);
        assert!(
            !document.scene.entities[&unrelated]
                .components
                .contains_key("Transform3D")
        );
    }

    #[test]
    fn name_edit_marks_dirty() {
        let (mut document, inspector, id) = setup();
        inspector.set_name(&mut document, id, "Hero").unwrap();
        assert!(document.is_dirty());
        assert_eq!(document.scene.entities[&id].name, "Hero");
    }
    #[test]
    fn empty_name_rejected_without_mutation() {
        let (mut document, inspector, id) = setup();
        assert!(matches!(
            inspector.set_name(&mut document, id, " "),
            Err(InspectorError::EmptyName)
        ));
        assert_eq!(document.scene.entities[&id].name, "Player");
        assert!(!document.is_dirty());
    }
    #[test]
    fn unknown_entity_rejected() {
        let (mut document, inspector, _id) = setup();
        let ghost = SceneEntityId(999);
        assert!(matches!(
            inspector.snapshot(&document, ghost),
            Err(InspectorError::UnknownEntity(_))
        ));
        assert!(matches!(
            inspector.set_name(&mut document, ghost, "X"),
            Err(InspectorError::UnknownEntity(_))
        ));
        assert!(matches!(
            inspector.clear_sprite_reference(&mut document, ghost),
            Err(InspectorError::UnknownEntity(_))
        ));
    }
    #[test]
    fn transform_position_x_accepts() {
        let (mut document, inspector, id) = setup();
        inspector
            .set_transform(
                &mut document,
                id,
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
        let (mut document, inspector, id) = setup();
        inspector
            .set_transform(
                &mut document,
                id,
                TransformFields {
                    position_y: -2.0,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            inspector
                .snapshot(&document, id)
                .unwrap()
                .transform
                .position_y,
            -2.0
        );
    }
    #[test]
    fn transform_rotation_accepts() {
        let (mut document, inspector, id) = setup();
        inspector
            .set_transform(
                &mut document,
                id,
                TransformFields {
                    rotation: 3.0,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            inspector
                .snapshot(&document, id)
                .unwrap()
                .transform
                .rotation,
            3.0
        );
    }
    #[test]
    fn transform_scale_x_accepts() {
        let (mut document, inspector, id) = setup();
        inspector
            .set_transform(
                &mut document,
                id,
                TransformFields {
                    scale_x: 2.0,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            inspector.snapshot(&document, id).unwrap().transform.scale_x,
            2.0
        );
    }
    #[test]
    fn transform_scale_y_accepts() {
        let (mut document, inspector, id) = setup();
        inspector
            .set_transform(
                &mut document,
                id,
                TransformFields {
                    scale_y: 3.0,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            inspector.snapshot(&document, id).unwrap().transform.scale_y,
            3.0
        );
    }
    #[test]
    fn nan_transform_rejected() {
        let (mut document, inspector, id) = setup();
        assert!(matches!(
            inspector.set_transform(
                &mut document,
                id,
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
        let (mut document, inspector, id) = setup();
        assert!(
            inspector
                .set_transform(
                    &mut document,
                    id,
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
        let (mut document, inspector, id) = setup();
        assert!(matches!(
            inspector.set_transform(
                &mut document,
                id,
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
        let (mut document, inspector, id) = setup();
        assert!(
            inspector
                .set_transform(
                    &mut document,
                    id,
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
        let (mut document, inspector, id) = setup();
        inspector
            .set_sprite_reference(&mut document, id, "texture=textures/player.png")
            .unwrap();
        assert_eq!(
            inspector
                .snapshot(&document, id)
                .unwrap()
                .sprite_reference
                .as_deref(),
            Some(
                "texture=textures/player.png;size=128x128;pivot=0.500,0.500;tint=1.000,1.000,1.000,1.000;sorting_layer=default;order=0"
            )
        );
    }
    #[test]
    fn sprite_jpg_accepts() {
        let (mut document, inspector, id) = setup();
        inspector
            .set_sprite_reference(&mut document, id, "texture=textures/player.jpg")
            .unwrap();
        assert!(document.is_dirty());
    }
    #[test]
    fn sprite_empty_rejected() {
        let (mut document, inspector, id) = setup();
        assert!(
            inspector
                .set_sprite_reference(&mut document, id, "")
                .is_err()
        );
        assert!(!document.is_dirty());
    }
    #[test]
    fn sprite_traversal_rejected() {
        let (mut document, inspector, id) = setup();
        assert!(
            inspector
                .set_sprite_reference(&mut document, id, "texture=../player.png")
                .is_err()
        );
    }
    #[test]
    fn sprite_wrong_extension_rejected() {
        let (mut document, inspector, id) = setup();
        assert!(
            inspector
                .set_sprite_reference(&mut document, id, "texture=player.txt")
                .is_err()
        );
    }
    #[test]
    fn sprite_clear_marks_dirty() {
        let (mut document, inspector, id) = setup();
        inspector
            .set_sprite_reference(&mut document, id, "texture=player.png")
            .unwrap();
        document.mark_clean();
        inspector.clear_sprite_reference(&mut document, id).unwrap();
        assert!(document.is_dirty());
    }
    #[test]
    fn lua_accepts() {
        let (mut document, inspector, id) = setup();
        inspector
            .set_lua_script(&mut document, id, "scripts/player.lua")
            .unwrap();
        // 写出的必须是运行时真正读的组件名与载荷形式：`Script` + `file=<路径>`。
        // （历史的 `LuaScript` + 裸路径运行时根本不读，这条断言就是那个缺陷的回归守卫。）
        let raw = &document.scene.entities[&id].components;
        assert_eq!(
            raw.get(engine::core::entity_script::SCRIPT_COMPONENT)
                .map(String::as_str),
            Some("file=scripts/player.lua"),
            "面板必须按运行时格式写 Script 组件"
        );
        assert!(
            !raw.contains_key("LuaScript"),
            "旧键名不得再被写进场景"
        );
        // 运行时解析器读得回来（"不是换个名字了事"）。
        let parsed = engine::core::entity_script::EntityScript::from_component_str(
            &raw[engine::core::entity_script::SCRIPT_COMPONENT],
        )
        .expect("runtime parser");
        assert_eq!(parsed.file, "scripts/player.lua");
        assert!(parsed.enabled);
        // 面板显示的是裸路径（用户关心的是文件），不是 `file=` 形式。
        assert_eq!(
            inspector
                .snapshot(&document, id)
                .unwrap()
                .lua_script
                .as_deref(),
            Some("scripts/player.lua")
        );
    }
    #[test]
    fn lua_wrong_extension_rejected() {
        let (mut document, inspector, id) = setup();
        assert!(
            inspector
                .set_lua_script(&mut document, id, "scripts/player.txt")
                .is_err()
        );
        // 这一条是**编辑器比引擎更严**的刻意差异：引擎只要求"工程内相对路径"
        // （运行时 `root.join(file)` 后直接编译，扩展名不参与判定），
        // 所以同样的载荷在引擎侧是合法的（见 `component_schema::field_rules_match_engine_validators`）。
        assert!(
            engine::core::entity_script::EntityScript::from_component_str(
                "file=scripts/player.txt"
            )
            .is_ok(),
            "引擎侧必须仍然接受它（否则运行时行为被编辑器约定改掉了）"
        );
    }

    /// Windows 上的"根相对"路径（`/etc/passwd.lua`）没有盘符，`Path::is_absolute()` 会返回
    /// `false`——编辑器曾经因此放行、而运行时拒绝，写出一个运行时不加载的脚本。
    /// 现在两边共用 `core::paths` 的判定。
    #[test]
    fn lua_windows_root_relative_path_rejected() {
        let (mut document, inspector, id) = setup();
        for script in ["/etc/passwd.lua", r"C:\windows\x.lua", "scripts/../../x.lua"] {
            assert!(
                inspector.set_lua_script(&mut document, id, script).is_err(),
                "{script} 必须被拒绝"
            );
        }
    }
    #[test]
    fn lua_traversal_rejected() {
        let (mut document, inspector, id) = setup();
        assert!(
            inspector
                .set_lua_script(&mut document, id, "../player.lua")
                .is_err()
        );
    }
    #[test]
    fn lua_clear_marks_dirty() {
        let (mut document, inspector, id) = setup();
        inspector
            .set_lua_script(&mut document, id, "player.lua")
            .unwrap();
        document.mark_clean();
        inspector.clear_lua_script(&mut document, id).unwrap();
        assert!(document.is_dirty());
    }
    #[test]
    fn transform3d_set_and_snapshot() {
        let (mut document, inspector, id) = setup();
        let value = Transform3DFields {
            position_x: 1.0,
            position_y: 2.0,
            position_z: 3.0,
            scale_x: 2.0,
            scale_y: 2.0,
            scale_z: 2.0,
            ..Default::default()
        };
        inspector.set_transform3d(&mut document, id, value).unwrap();
        assert!(document.is_dirty());
        assert_eq!(
            inspector.snapshot(&document, id).unwrap().transform3d,
            Some(value)
        );
    }
    #[test]
    fn transform3d_rejects_zero_scale() {
        let (mut document, inspector, id) = setup();
        let value = Transform3DFields {
            scale_x: 0.0,
            ..Default::default()
        };
        assert!(inspector.set_transform3d(&mut document, id, value).is_err());
        assert!(!document.is_dirty());
    }
    #[test]
    fn transform3d_rejects_non_unit_rotation() {
        let (mut document, inspector, id) = setup();
        let value = Transform3DFields {
            rotation_w: 2.0,
            ..Default::default()
        };
        assert!(inspector.set_transform3d(&mut document, id, value).is_err());
    }
    /// `MeshRenderer` 的缓冲管线已随面板迁移删除；这里验证**迁移后的语义**：
    /// 快照不再携带 mesh 字段，而 `.glb/.gltf/哨兵` 与 `.obj`/穿越路径的判定
    /// 由 schema 规则与引擎解析器共同保证（对拍测试在 `component_schema`）。
    #[test]
    fn mesh_renderer_is_edited_through_the_schema_path() {
        let (mut document, inspector, id) = setup();
        let entity = document.scene.entities.get_mut(&id).unwrap();
        entity.components.insert(
            "MeshRenderer".to_owned(),
            format!(
                "mesh={}",
                engine::core::mesh3d::GROUND_PLANE_SENTINEL
            ),
        );
        let snapshot = inspector.snapshot(&document, id).unwrap();
        assert!(!format!("{snapshot:?}").contains("mesh_renderer"));
        // 引擎解析器仍认得哨兵（迁移不改数据格式、也不放松引擎校验）。
        assert_eq!(
            engine::core::mesh3d::MeshRenderer::from_component_str(
                &document.scene.entities[&id].components["MeshRenderer"]
            )
            .expect("引擎解析器")
            .mesh,
            engine::core::mesh3d::GROUND_PLANE_SENTINEL
        );
    }
    #[test]
    fn camera3d_set_and_snapshot() {
        let (mut document, inspector, id) = setup();
        let value = Camera3DFields {
            position_x: 0.0,
            position_y: 3.0,
            position_z: 8.0,
            fov_degrees: 75.0,
            ..Default::default()
        };
        inspector.set_camera3d(&mut document, id, value).unwrap();
        let snapshot = inspector.snapshot(&document, id).unwrap();
        assert!(snapshot.camera3d.is_some());
        assert!((snapshot.camera3d.unwrap().fov_degrees - 75.0).abs() < 0.01);
    }
    #[test]
    fn camera3d_enabled_round_trips() {
        // docs/54 §4.2：`enabled` 必须能穿过 Inspector 编解码而不丢失，否则在面板里改任意
        // 相机字段都会把"我关掉了这台相机"悄悄复位。
        let (mut document, inspector, id) = setup();
        let value = Camera3DFields {
            enabled: false,
            ..Default::default()
        };
        inspector.set_camera3d(&mut document, id, value).unwrap();
        let raw = document.scene.entities[&id]
            .components
            .get("Camera3D")
            .expect("Camera3D 组件应已写入");
        assert!(raw.contains("enabled=false"), "禁用状态必须显式落盘：{raw}");
        let snapshot = inspector.snapshot(&document, id).unwrap();
        assert!(!snapshot.camera3d.unwrap().enabled);
        // 默认（启用）不写字段，保证既有场景串字节不变。
        let value = Camera3DFields::default();
        inspector.set_camera3d(&mut document, id, value).unwrap();
        let raw = document.scene.entities[&id].components["Camera3D"].clone();
        assert!(
            !raw.contains("enabled"),
            "启用时不应写出 enabled 字段：{raw}"
        );
    }
    #[test]
    fn camera3d_rejects_invalid_perspective() {
        let (mut document, inspector, id) = setup();
        let value = Camera3DFields {
            near: 10.0,
            far: 1.0,
            ..Default::default()
        };
        assert!(inspector.set_camera3d(&mut document, id, value).is_err());
    }
    /// `Light` 的缓冲管线已随面板迁移删除；这里改为验证**迁移后的路径**：
    /// 通用字段编辑器通过 `SetComponentField` 写入，快照里不再有 light 字段，
    /// 但实体上的 `Light` 组件串仍是引擎编码器认得的形式。
    #[test]
    fn light_is_edited_through_the_schema_path() {
        let (mut document, inspector, id) = setup();
        let entity = document.scene.entities.get_mut(&id).unwrap();
        entity
            .components
            .insert("Light".to_owned(), "direction=0,1,0;color=1,1,1;intensity=2".to_owned());
        // 快照不再携带 light（面板迁移后由通用编辑器负责）。
        let snapshot = inspector.snapshot(&document, id).unwrap();
        assert!(!format!("{snapshot:?}").contains("light"));
        // 组件串必须仍能被引擎解析器读回（迁移不得改变数据格式）。
        let light = engine::core::light::Light::from_component_str(
            &document.scene.entities[&id].components["Light"],
        )
        .expect("引擎解析器必须认得 Light");
        assert_eq!(light.intensity, 2.0);
    }
    #[test]
    fn save_reload_preserves_fields() {
        let (mut document, inspector, id) = setup();
        inspector
            .set_transform(
                &mut document,
                id,
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
            .set_sprite_reference(&mut document, id, "texture=player.png")
            .unwrap();
        inspector
            .set_lua_script(&mut document, id, "player.lua")
            .unwrap();
        let text = document.scene.to_text();
        let loaded = engine::core::scene::Scene::from_text(&text).unwrap();
        assert_eq!(
            loaded.entities[&id].components,
            document.scene.entities[&id].components
        );
    }
    #[test]
    fn collider3d_roundtrip_via_snapshot() {
        let (mut document, inspector, id) = setup();
        inspector
            .set_collider3d(
                &mut document,
                id,
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
        let snapshot = inspector.snapshot(&document, id).unwrap();
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
        let (mut document, inspector, id) = setup();
        let value = Collider3dFields {
            shape: "ball".to_owned(),
            radius: 0.0,
            ..Collider3dFields::default()
        };
        assert!(
            matches!(
                inspector.set_collider3d(&mut document, id, value),
                Err(InspectorError::InvalidCollider3d(_))
            ),
            "zero radius must be rejected"
        );
        assert!(
            !document.scene.entities[&id]
                .components
                .contains_key("Collider3d")
        );
    }
    #[test]
    fn collider3d_persists_through_scene_text() {
        let (mut document, inspector, id) = setup();
        inspector
            .set_collider3d(
                &mut document,
                id,
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
        let stored = loaded.entities[&id]
            .components
            .get("Collider3d")
            .expect("Collider3d persisted");
        let decoded = decode_collider3d(stored).unwrap();
        assert_eq!(decoded.shape, "capsule");
        assert_eq!(decoded.half_height, 1.0);
        assert_eq!(decoded.radius, 0.5);
    }
    /// `Image` 的缓冲管线已随面板迁移删除；这里验证**迁移后的语义**：
    /// 负九宫格仍被拒绝（改由 schema 规则表达），全零与省略语义相同（渲染侧过滤），
    /// 快照不再携带 image 字段。
    #[test]
    fn image_is_edited_through_the_schema_path() {
        let (mut document, inspector, id) = setup();
        let registry = engine::core::component_schema::builtin_registry();
        // 负九宫格：schema 规则拒绝（原来由编辑器 set_image 拒绝）。
        assert!(registry["Image"]
            .parse("texture=a.png;color=1,1,1;nine_slice=-1,0,0,0")
            .is_err());
        // 全零与省略都合法，且渲染侧用 `any(|v| *v > 0.0)` 过滤 → 语义相同。
        assert!(registry["Image"]
            .parse("texture=a.png;color=1,1,1;nine_slice=0,0,0,0")
            .is_ok());
        assert!(registry["Image"].parse("texture=a.png").is_ok());

        let entity = document.scene.entities.get_mut(&id).unwrap();
        entity.components.insert(
            "Image".to_owned(),
            "texture=a.png;color=1,1,1;nine_slice=8,8,16,16".to_owned(),
        );
        let snapshot = inspector.snapshot(&document, id).unwrap();
        assert!(!format!("{snapshot:?}").contains("image"));
        // 引擎解析器仍能读回九宫格（迁移不改格式）。
        let parsed = engine::core::ui::UiImage::from_component_str(
            &document.scene.entities[&id].components["Image"],
        )
        .expect("引擎解析器");
        assert_eq!(parsed.nine_slice, Some([8.0, 8.0, 16.0, 16.0]));
    }
    /// `WorldText` 的缓冲管线已随面板迁移删除（第 23 轮）；这里验证**迁移后的语义**：
    /// 快照不再携带 world_text 字段，而组件串（含中文与分号风险）在存盘往返后仍能由引擎解析器读回，
    /// 非有限的 offset 仍被 schema 规则拒绝。
    #[test]
    fn world_text_is_edited_through_the_schema_path() {
        let (mut document, inspector, id) = setup();
        let entity = document.scene.entities.get_mut(&id).unwrap();
        entity.components.insert(
            "WorldText".to_owned(),
            "text=受伤-12;font_size=18;color=1,0.2,0.2;offset=0,-24;centered=true".to_owned(),
        );
        let snapshot = inspector.snapshot(&document, id).unwrap();
        assert!(!format!("{snapshot:?}").contains("world_text"));

        // 存盘再读回：场景文本必须无损往返（组件串里带中文与分号风险）。
        let text = document.scene.to_text();
        let loaded = engine::core::scene::Scene::from_text(&text).unwrap();
        let reloaded = engine::core::world_text::WorldText::from_component_str(
            loaded.entities[&id]
                .components
                .get("WorldText")
                .expect("WorldText survives the round trip"),
        )
        .expect("reloaded component parses");
        assert_eq!(reloaded.text.text, "受伤-12");
        assert_eq!(reloaded.offset, [0.0, -24.0]);
        assert!(reloaded.centered);

        // 非有限 offset：schema 规则仍拒绝（原来由编辑器 validate_world_text 拦）。
        let registry = engine::core::component_schema::builtin_registry();
        assert!(registry["WorldText"]
            .parse("text=x;font_size=18;offset=NaN,0")
            .is_err());
    }
    /// `Image` 的缓冲管线已随面板迁移删除（第 22 轮）；见
    /// `image_is_edited_through_the_schema_path` 对迁移后语义的验证。
    #[test]
    fn image_nine_slice_persists_through_scene_text() {
        let (mut document, _inspector, id) = setup();
        let entity = document.scene.entities.get_mut(&id).unwrap();
        entity.components.insert(
            "Image".to_owned(),
            "texture=assets/textures/ui/panel.png;color=1,1,1;nine_slice=8,8,16,16".to_owned(),
        );
        let text = document.scene.to_text();
        let loaded = engine::core::scene::Scene::from_text(&text).unwrap();
        let stored = loaded.entities[&id]
            .components
            .get("Image")
            .expect("Image persisted");
        let parsed = engine::core::ui::UiImage::from_component_str(stored).expect("引擎解析器");
        assert_eq!(parsed.nine_slice, Some([8.0, 8.0, 16.0, 16.0]));
    }
    #[test]
    fn text_font_roundtrips_via_snapshot_and_scene_text() {
        let (mut document, inspector, id) = setup();
        inspector
            .set_text(
                &mut document,
                id,
                TextFields {
                    text: "Hi".to_owned(),
                    font_size: 24.0,
                    font: "assets/fonts/a;b=数字.ttf".to_owned(),
                    ..TextFields::default()
                },
            )
            .unwrap();
        assert!(document.is_dirty());
        let stored = document.scene.entities[&id]
            .components
            .get("Text")
            .expect("Text written");
        assert!(
            stored.ends_with(";font=assets/fonts/a\\;b\\=数字.ttf"),
            "{stored}"
        );
        // 场景文件文本往返后仍保留字体字段。
        let text = document.scene.to_text();
        let loaded = engine::core::scene::Scene::from_text(&text).unwrap();
        let persisted = loaded.entities[&id]
            .components
            .get("Text")
            .expect("Text persisted");
        let decoded = decode_text(persisted).unwrap();
        assert_eq!(decoded.font, "assets/fonts/a;b=数字.ttf");
        assert_eq!(decoded.text, "Hi");
        // 快照往返。
        let snapshot = inspector.snapshot(&document, id).unwrap();
        assert_eq!(
            snapshot.text.expect("text snapshot").font,
            "assets/fonts/a;b=数字.ttf"
        );
    }
    #[test]
    fn text_empty_font_stores_legacy_string_without_font_key() {
        let (mut document, inspector, id) = setup();
        inspector
            .set_text(
                &mut document,
                id,
                TextFields {
                    text: "Hi".to_owned(),
                    ..TextFields::default()
                },
            )
            .unwrap();
        let stored = document.scene.entities[&id]
            .components
            .get("Text")
            .expect("Text written");
        assert!(!stored.contains("font="), "{stored}");
        let decoded = decode_text(stored).unwrap();
        assert_eq!(decoded.font, "");
    }
}
