use crate::SceneDocument;
use rustlua_ecs::entity_script::{EntityScript, SCRIPT_COMPONENT};
use rustlua_physics::physics3d::{ColliderDesc3d, ColliderShape3d};
use rustlua_scene::scene::{SceneEntity, SceneEntityId};
use std::path::Path;

/// 第一批 Inspector 可编辑的 Transform 字段。
///
/// **注意**：这不是 Inspector 的缓冲字段了（第 36 轮 `Transform` 已迁到 schema 编辑器）——
/// 它现在是 `scene_view`/`geometry`/`actions` 共用的**数据载体**（收集 2D 包围框、编码局部变换）。
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

// `Transform3DFields`（3D 变换的 Inspector 缓冲）与 `Camera3DFields` 一起删除：
// 变换第 36 轮、相机第 24 轮迁到 schema 生成的字段编辑器，第 57 轮统一成单一
// `Transform`/`Camera` 组件后连字段类型也只剩通用路径（`schema_fields`）需要的那一份。

// `Camera3D` 的 Inspector 缓冲管线（`Camera3DFields` + `set_camera3d` +
// `validate/encode/decode_camera3d`）**已删除**（第 24 轮）：该组件迁到 schema 生成的
// 字段编辑器（position/target = DragValue×3、fov = 以**角度**为单位的 DragValue、
// near/far、pose = 下拉、enabled = 勾选框），提交走 `EditorCommand::SetComponentField`。
// 面板里"不是字段"的那部分（统一 `Projection` 的只读投影摘要、`pose=transform` 时的生效姿态）
// 搬到了 `schema_fields::component_notes`，由通用编辑器在字段行下方呈现。
// **默认值**取引擎类型（`Camera3D::default()`），编辑器里不再有这份默认值。
//
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

// `Text` 的 Inspector 缓冲管线（`TextFields` + `set_text`/`clear_text` +
// `encode/decode/validate_text`）**已删除**（第 25 轮）：该组件迁到 schema 生成的字段编辑器
// （文本 = 输入框、字号 = DragValue、字体 = **项目字体下拉**、颜色 = 取色器）。
// 自由文本里的 `;`/`=`/`\` 由 schema 的转义字段负责（`FieldSchema::escaped`），
// 因此"文字里带分号"不会再写坏组件串，审计也不再把它判成漂移。
// 默认值取引擎类型（`rustlua_ui::ui::UiText::default()`）。

// `WorldText` 的 Inspector 缓冲管线（`WorldTextFields` + `set_world_text`/`clear_world_text` +
// `encode/decode/validate_world_text`）**已删除**（第 23 轮）：该组件迁到 schema 生成的字段编辑器，
// `font` 字段用**项目字体下拉**（`ChoiceSource::ProjectFonts`），其余用文本框/DragValue/取色器/勾选框。
// 注意：`WorldText` 的默认值（`text=文字;font_size=…;offset=0,-20;centered=true`）现在由
// `add_component` 直接用**引擎类型**的编码给出（`rustlua_scene::world_text::WorldText`），
// 编辑器里不再有这份默认值。

// `Image` 的 Inspector 缓冲管线（`ImageFields` + `set_image`/`clear_image` +
// `encode/decode/validate_image`）**已删除**（第 22 轮）：该组件迁到 schema 生成的字段编辑器
// （`texture` 文本框、`color` 取色器、`nine_slice` 四个 DragValue）。
// 九宫格"非负"这条规则现在由 `FieldRule::NonNegative` 表达（对向量要求每个分量非负），
// 因此**审计**也一并获得这条校验；全零与省略的等价性由渲染侧的过滤
// （`any(|v| *v > 0.0)`）保证。

// `UiLayout` 的 Inspector 缓冲管线（`UiLayoutFields` + `set_ui_layout` +
// `validate_unit_pair/quadruple`）**已删除**（第 31 轮）：该组件迁到 schema 生成的字段编辑器
// （`anchors`/`offsets`/`min_size`/`max_size`/`pivot` = 向量 DragValue 连排），
// **锚点预设**改由组件级动作下拉（`SchemaComponentAction` + `ACTION_PROVIDERS`）承担——
// 一次改 `anchors`+`pivot` 仍是一条 Undo（`EditorCommand::SetComponentFields`）。
// anchors/pivot 的 `0..=1` 约束不再由编辑器自己校验：**引擎解析器兜底门**
// （`UiRectLayout::from_component_str` 里的 `validate_anchors`/`validate_unit`）就是同一条规则。
// 默认值取引擎类型（`UiRectLayout::default()`）。

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
    pub fn to_style(&self) -> Result<rustlua_ui::ui_layout::ContainerStyle, String> {
        let kind = rustlua_ui::ui_layout::ContainerKind::parse(&self.kind)
            .ok_or_else(|| format!("未知 Container kind：{}", self.kind))?;
        let align = rustlua_ui::ui_layout::ChildAlign::parse(&self.align)
            .ok_or_else(|| format!("未知对齐：{}", self.align))?;
        Ok(rustlua_ui::ui_layout::ContainerStyle {
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

    pub fn from_style(style: rustlua_ui::ui_layout::ContainerStyle) -> Self {
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
        Self::from_style(rustlua_ui::ui_layout::ContainerStyle::default())
    }
}

/// Inspector 对一个实体的可编辑快照。
#[derive(Clone, Debug, PartialEq)]
pub struct InspectorSnapshot {
    pub entity_id: SceneEntityId,
    pub name: String,
    // `Transform`/`Transform3D`（第 36 轮）与 `Sprite`（第 34 轮）**不在快照里**：
    // 字段都由 schema 生成，提交走 `EditorCommand::SetComponentField(s)`。
    pub lua_script: Option<String>,
    // `MeshRenderer`（mesh 字段）**不在快照里**：它已迁到 schema 生成的通用字段编辑器
    // （走 `EditorCommand::SetComponentField`，语义规则见 `FieldRule::ProjectPath`）。
    // `Camera3D` 同样**不在快照里**（第 24 轮）：字段由 schema 生成，只读摘要见
    // `schema_fields::component_notes`。
    /// `Light` **不在快照里**：它已迁到 schema 生成的通用字段编辑器，提交走
    /// `EditorCommand::SetComponentField`（见 `LightFields` 删除处的说明）。
    pub collider3d: Option<Collider3dFields>,
    /// `Text` / `WorldText` / `Image` / `UiLayout` **不在快照里**：都已迁到 schema 生成的
    /// 字段编辑器（第 22/23/25/31 轮），提交走 `EditorCommand::SetComponentField(s)`。
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

    // `set_transform` / `set_transform3d` **已删除**（第 36 轮）：`Transform`/`Transform3D`
    // 迁到 schema 生成的字段编辑器（等比锁 = 编辑器态开关 + 字段镜像、重置 = 组件级动作），
    // 提交走 `EditorCommand::SetComponentField(s)`。两个 `*Fields` 类型仍保留：
    // `scene_view`/`geometry`/`actions` 把它们当**数据载体**（收集包围框、编码变换）用。

    // `set_sprite_reference` / `clear_sprite_reference` **已删除**（第 34 轮）：`Sprite` 迁到
    // schema 生成的字段编辑器（纹理文本框带图片扩展名规则、排序层下拉、多选批量由第 33 轮的
    // 批量命令承担），提交走 `EditorCommand::SetComponentField(s)`。

    pub fn set_lua_script(
        &self,
        document: &mut SceneDocument,
        id: SceneEntityId,
        script: impl Into<String>,
    ) -> Result<(), InspectorError> {
        let script = script.into();
        validate_lua_script(&script)?;
        // 加载器/运行时读的是 `Script` + `file=<路径>`；这里写出**同一个**名字与载荷形式
        // （旧写法 `LuaScript` + 裸路径曾靠迁移兜底，第 55 轮已连同迁移代码一起删除。）
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
        let changed = entity.components.remove(SCRIPT_COMPONENT).is_some();
        if changed {
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

    // `set_camera3d` / `validate_camera3d` / `encode_camera3d` **已删除**（第 24 轮）：
    // `Camera3D` 与 `Light`/`MeshRenderer`/`Image`/`WorldText` 一样迁到 schema 生成的
    // 字段编辑器。相机跨字段的不变量（`near < far`、look-at 非退化、fov 开区间）由
    // **引擎解析器**在字段写入后兜底（`typed_scene::validate_component_payload`），
    // 因此编辑器里不再有第二份相机校验。

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

    // `set_text` / `clear_text` **已删除**（第 25 轮）：`Text` 迁到 schema 生成的字段编辑器，
    // 提交走 `EditorCommand::SetComponentField`（自由文本的转义在 schema/property_path 一侧）。

    // `set_world_text` / `clear_world_text` **已删除**（第 23 轮）：`WorldText` 迁到 schema
    // 生成的字段编辑器——`font` 字段用**项目字体下拉**（`ChoiceSource::ProjectFonts`），
    // 其余字段用文本框 / DragValue / 取色器 / 勾选框。提交走 `EditorCommand::SetComponentField`。

    // `set_image` / `clear_image` **已删除**（第 22 轮）：`Image` 与 `Light`/`MeshRenderer`
    // 一样迁到 schema 生成的字段编辑器（`texture` 文本框、`color` 取色器、`nine_slice` 四个
    // DragValue），提交走 `EditorCommand::SetComponentField`。

    // `set_ui_layout` **已删除**（第 31 轮）：`UiLayout` 迁到 schema 生成的字段编辑器 +
    // 组件级动作（锚点预设），提交走 `EditorCommand::SetComponentField(s)`。

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
        // E17：Container 单实体写回（不做多选批量——布局逐实体差异大，
        // 批量覆盖意义有限，与 Tilemap/Animation 同策略）。
        if let Some(container) = snapshot.container {
            self.set_container(document, id, container)?;
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
    Ok(InspectorSnapshot {
        entity_id: entity.id,
        name: entity.name.clone(),
        lua_script: read_lua_script(entity),
        collider3d: entity
            .components
            .get("Collider3d")
            .map_or(Ok(None), |value| decode_collider3d(value).map(Some))?,
        container: entity
            .components
            .get("Container")
            .map_or(Ok(None), |value| {
                rustlua_ui::ui_layout::ContainerStyle::from_component_str(value)
                    .map(|style| Some(ContainerFields::from_style(style)))
                    .map_err(InspectorError::InvalidContainer)
            })?,
        persistent: entity
            .metadata
            .get("persistent")
            .is_some_and(|value| value == "true"),
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
/// （正常路径下 [`rustlua_scene::scene_migration`] 已经在打开场景时迁移过，
/// 这里只是不让"未经迁移的实体内存对象"显示成空）。
fn read_lua_script(entity: &SceneEntity) -> Option<String> {
    if let Some(value) = entity.components.get(SCRIPT_COMPONENT) {
        return match EntityScript::from_component_str(value) {
            Ok(script) => Some(script.file),
            // 解析不了就原样显示，让用户看到问题而不是空白。
            Err(_) => Some(value.clone()),
        };
    }
    None
}

#[derive(Debug, thiserror::Error)]
pub enum InspectorError {
    #[error("Inspector 中不存在实体 {}", .0 .0)]
    UnknownEntity(SceneEntityId),
    #[error("Inspector 尚未选择实体")]
    NothingSelected,
    #[error("实体名称不能为空")]
    EmptyName,
    #[error("Transform 必须是有限数值")]
    NonFiniteTransform,
    #[error("Transform 缩放必须大于 0")]
    InvalidScale,
    #[error("Transform 数据无效：{0}")]
    InvalidTransform(String),
    #[error("Sprite 资源引用无效：{0}")]
    InvalidAssetReference(String),
    #[error("Lua 脚本路径无效：{0}")]
    InvalidLuaScript(String),
    #[error("Transform3D 数据无效：{0}")]
    InvalidTransform3d(String),
    #[error("MeshRenderer 资源引用无效：{0}")]
    InvalidMeshRenderer(String),
    #[error("Light 数据无效：{0}")]
    InvalidLight(String),
    #[error("Collider3d 数据无效：{0}")]
    InvalidCollider3d(String),
    #[error("WorldText 数据无效：{0}")]
    InvalidWorldText(String),
    #[error("Image 数据无效：{0}")]
    InvalidImage(String),
    #[error("Button 数据无效：{0}")]
    InvalidButton(String),
    #[error("Container 数据无效：{0}")]
    InvalidContainer(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_scene::scene::SceneEntityId;

    fn setup() -> (SceneDocument, InspectorState, SceneEntityId) {
        let mut document = SceneDocument::new_scene("main");
        let id = SceneEntityId(1);
        document.scene.add_entity(id, "Player", None).unwrap();
        document.mark_clean();
        (document, InspectorState::new(), id)
    }

    // `Transform`/`Transform3D` 的 Inspector 管线测试（`unified_transform_panel_resolves_the_edited_space`、
    // `write_inspector_batches_transform*`、`transform_*_accepts`、`zero/negative/infinity/nan_*_rejected`、
    // `transform3d_set_and_snapshot`、`transform3d_rejects_*`）随管线**一并删除**（第 36 轮）：
    // - 空间切换这条特性被刻意去掉（两个组件现在同时在通用编辑器里显示，没有"切换"这件事）；
    // - 批量语义由通用编辑器的 `SetComponentField(s)OnSelection` 承担，
    //   门在 `panels::schema_inspector::tests::multi_selection_edit_writes_all_targets_in_one_undo`
    //   与迁移测试里；
    // - "缩放必须为正 / 非有限值拒绝 / 四元数必须单位化"由**引擎解析器**保证
    //   （`Transform::from_component_str`/`Transform3D::from_component_str`），
    //   编辑器侧还有 `typed_scene::validate_component_payload` 兜底门。

    #[test]
    fn snapshot_defaults() {
        let (document, inspector, id) = setup();
        let snapshot = inspector.snapshot(&document, id).unwrap();
        assert_eq!(snapshot.entity_id, id);
        // `Transform`/`Transform3D`/`Sprite` 都已迁走：快照里不再有它们的字段。
        assert!(!format!("{snapshot:?}").contains("transform"));
        assert!(!format!("{snapshot:?}").contains("sprite"));
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
            inspector.set_container(&mut document, ghost, ContainerFields::default()),
            Err(InspectorError::UnknownEntity(_))
        ));
    }
    // `Sprite` 的缓冲管线与它的 6 个校验测试**已随面板迁移删除**（第 34 轮）：
    // "纹理必须是工程内相对路径 + png/jpg/jpeg/webp"这条规则现在由引擎 schema 的
    // `TEXTURE_RULE`/`REQUIRED_TEXTURE_RULE` 表达（覆盖迁移前 `validate_asset_reference`
    // 的全部样本），门在 `component_schema::tests::field_rules_match_engine_validators` 与
    // `panels::schema_inspector::tests::texture_reference_rule_is_enforced_through_the_generic_editor`。
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
            raw.get(rustlua_ecs::entity_script::SCRIPT_COMPONENT)
                .map(String::as_str),
            Some("file=scripts/player.lua"),
            "面板必须按运行时格式写 Script 组件"
        );
        assert!(!raw.contains_key("LuaScript"), "旧键名不得再被写进场景");
        // 运行时解析器读得回来（"不是换个名字了事"）。
        let parsed = rustlua_ecs::entity_script::EntityScript::from_component_str(
            &raw[rustlua_ecs::entity_script::SCRIPT_COMPONENT],
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
            rustlua_ecs::entity_script::EntityScript::from_component_str("file=scripts/player.txt")
                .is_ok(),
            "引擎侧必须仍然接受它（否则运行时行为被编辑器约定改掉了）"
        );
    }

    /// Windows 上的"根相对"路径（`/etc/passwd.lua`）没有盘符，`Path::is_absolute()` 会返回
    /// `false`——编辑器曾经因此放行、而运行时拒绝，写出一个运行时不加载的脚本。
    /// 现在两边共用 `rustlua_infra::paths` 的判定。
    #[test]
    fn lua_windows_root_relative_path_rejected() {
        let (mut document, inspector, id) = setup();
        for script in [
            "/etc/passwd.lua",
            r"C:\windows\x.lua",
            "scripts/../../x.lua",
        ] {
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
    /// `MeshRenderer` 的缓冲管线已随面板迁移删除；这里验证**迁移后的语义**：
    /// 快照不再携带 mesh 字段，而 `.glb/.gltf/哨兵` 与 `.obj`/穿越路径的判定
    /// 由 schema 规则与引擎解析器共同保证（对拍测试在 `component_schema`）。
    #[test]
    fn mesh_renderer_is_edited_through_the_schema_path() {
        let (mut document, inspector, id) = setup();
        let entity = document.scene.entities.get_mut(&id).unwrap();
        entity.components.insert(
            "MeshRenderer".to_owned(),
            format!("mesh={}", rustlua_render::mesh3d::GROUND_PLANE_SENTINEL),
        );
        let snapshot = inspector.snapshot(&document, id).unwrap();
        assert!(!format!("{snapshot:?}").contains("mesh_renderer"));
        // 引擎解析器仍认得哨兵（迁移不改数据格式、也不放松引擎校验）。
        assert_eq!(
            rustlua_render::mesh3d::MeshRenderer::from_component_str(
                &document.scene.entities[&id].components["MeshRenderer"]
            )
            .expect("引擎解析器")
            .mesh,
            rustlua_render::mesh3d::GROUND_PLANE_SENTINEL
        );
    }
    /// 相机的缓冲管线已随面板迁移删除（第 24 轮）、组件本身也在第 57 轮统一成单一 `Camera`；
    /// 这里验证**迁移后的路径**：快照里不再有相机字段（编辑由通用字段编辑器负责），
    /// 组件串仍是引擎编码器认得的统一形式，默认值来自引擎类型（`Camera3D::default()` + 判别字段）。
    #[test]
    fn camera_is_edited_through_the_schema_path() {
        let (mut document, inspector, id) = setup();
        document
            .scene
            .entities
            .get_mut(&id)
            .unwrap()
            .components
            .insert(
            "Camera".to_owned(),
            rustlua_camera::camera::Camera::Perspective(rustlua_camera::camera::Camera3D::default())
                .to_component_str(),
        );
        let snapshot = inspector.snapshot(&document, id).unwrap();
        assert!(
            !format!("{snapshot:?}").contains("camera3d"),
            "迁移后快照不得再携带相机缓冲字段"
        );
        let camera = rustlua_camera::camera::Camera::from_component_str(
            &document.scene.entities[&id].components["Camera"],
        )
        .expect("引擎解析器必须认得统一的 Camera")
        .as_perspective()
        .expect("默认添加的是透视相机");
        assert!(
            (camera.fov_y.to_degrees() - 90.0).abs() < 0.01,
            "默认相机 FOV 是 90°（组件串里存角度）：{}",
            camera.fov_y.to_degrees()
        );
        assert!(camera.enabled, "默认相机是启用的");
    }
    /// `Light` 的缓冲管线已随面板迁移删除；这里改为验证**迁移后的路径**：
    /// 通用字段编辑器通过 `SetComponentField` 写入，快照里不再有 light 字段，
    /// 但实体上的 `Light` 组件串仍是引擎编码器认得的形式。
    #[test]
    fn light_is_edited_through_the_schema_path() {
        let (mut document, inspector, id) = setup();
        let entity = document.scene.entities.get_mut(&id).unwrap();
        entity.components.insert(
            "Light".to_owned(),
            "direction=0,1,0;color=1,1,1;intensity=2".to_owned(),
        );
        // 快照不再携带 light（面板迁移后由通用编辑器负责）。
        let snapshot = inspector.snapshot(&document, id).unwrap();
        assert!(!format!("{snapshot:?}").contains("light"));
        // 组件串必须仍能被引擎解析器读回（迁移不得改变数据格式）。
        let light = rustlua_render::light::Light::from_component_str(
            &document.scene.entities[&id].components["Light"],
        )
        .expect("引擎解析器必须认得 Light");
        assert_eq!(light.intensity, 2.0);
    }
    #[test]
    fn save_reload_preserves_fields() {
        let (mut document, inspector, id) = setup();
        // `Transform` 的写入路径已迁到 typed 命令（第 36 轮）；这里保留场景文本往返这条断言，
        // 变换值用**引擎编码器**写入（与迁移前的落盘格式一致）。
        document
            .scene
            .entities
            .get_mut(&id)
            .unwrap()
            .components
            .insert(
                "Transform".to_owned(),
                rustlua_ecs::transform::Transform {
                    position: rustlua_ecs::transform::Vec2::new(4.0, 5.0),
                    rotation: 0.5,
                    scale: rustlua_ecs::transform::Vec2::new(2.0, 3.0),
                }
                .to_component_str(),
            );
        inspector
            .set_lua_script(&mut document, id, "player.lua")
            .unwrap();
        let text = document.scene.to_text();
        let loaded = rustlua_scene::scene::Scene::from_text(&text).unwrap();
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
        let loaded = rustlua_scene::scene::Scene::from_text(&text).unwrap();
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
        let registry = rustlua_ecs::component_schema::builtin_registry();
        // 负九宫格：schema 规则拒绝（原来由编辑器 set_image 拒绝）。
        assert!(
            registry["Image"]
                .parse("texture=a.png;color=1,1,1;nine_slice=-1,0,0,0")
                .is_err()
        );
        // 全零与省略都合法，且渲染侧用 `any(|v| *v > 0.0)` 过滤 → 语义相同。
        assert!(
            registry["Image"]
                .parse("texture=a.png;color=1,1,1;nine_slice=0,0,0,0")
                .is_ok()
        );
        assert!(registry["Image"].parse("texture=a.png").is_ok());

        let entity = document.scene.entities.get_mut(&id).unwrap();
        entity.components.insert(
            "Image".to_owned(),
            "texture=a.png;color=1,1,1;nine_slice=8,8,16,16".to_owned(),
        );
        let snapshot = inspector.snapshot(&document, id).unwrap();
        assert!(!format!("{snapshot:?}").contains("image"));
        // 引擎解析器仍能读回九宫格（迁移不改格式）。
        let parsed = rustlua_ui::ui::UiImage::from_component_str(
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
        let loaded = rustlua_scene::scene::Scene::from_text(&text).unwrap();
        let reloaded = rustlua_scene::world_text::WorldText::from_component_str(
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
        let registry = rustlua_ecs::component_schema::builtin_registry();
        assert!(
            registry["WorldText"]
                .parse("text=x;font_size=18;offset=NaN,0")
                .is_err()
        );
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
        let loaded = rustlua_scene::scene::Scene::from_text(&text).unwrap();
        let stored = loaded.entities[&id]
            .components
            .get("Image")
            .expect("Image persisted");
        let parsed = rustlua_ui::ui::UiImage::from_component_str(stored).expect("引擎解析器");
        assert_eq!(parsed.nine_slice, Some([8.0, 8.0, 16.0, 16.0]));
    }
    /// `Text` 的缓冲管线已随面板迁移删除（第 25 轮）；这里验证**迁移后的路径**：
    /// 快照里不再有 text 字段（编辑由通用字段编辑器负责），组件串仍是引擎编码器认得的
    /// 形式，"文字里带分号"的转义由 schema 的 `FieldSchema::escaped` 负责
    /// （`schema_fields`/`property_path` 侧有专门测试）。
    #[test]
    fn text_is_edited_through_the_schema_path() {
        let (mut document, inspector, id) = setup();
        document
            .scene
            .entities
            .get_mut(&id)
            .unwrap()
            .components
            .insert(
                "Text".to_owned(),
                rustlua_ui::ui::UiText::default().to_component_str(),
            );
        let snapshot = inspector.snapshot(&document, id).unwrap();
        assert!(
            !format!("{snapshot:?}").contains("text: Some"),
            "迁移后快照不得再携带 Text 缓冲字段"
        );
        let parsed = rustlua_ui::ui::UiText::from_component_str(
            &document.scene.entities[&id].components["Text"],
        )
        .expect("引擎解析器必须认得 Text");
        assert_eq!(parsed.font_size, rustlua_ui::ui::UI_TEXT_BASE_PX);
        assert_eq!(parsed.font, "", "默认不写 font 字段");
    }
}
