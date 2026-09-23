//! WP1 原型：在加载边界把可读的 v2 场景编译成类型化运行时数据。

use std::{collections::BTreeMap, fmt};

use crate::core::{
    animation::AnimationStateMachine,
    animation3d::Animation3dDescriptor,
    entity_script::EntityScript,
    light::Light,
    mesh3d::MeshRenderer,
    particle::ParticleEmitter,
    physics::ColliderDesc,
    physics3d::ColliderDesc3d,
    quad::Quad,
    resource_db::{AssetId, ResourceDatabase},
    scene::{Scene, SceneEntityId},
    spatial_transform::LocalTransform,
    sprite::SpriteComponent,
    tilemap::Tilemap,
    transform::Transform3D,
    ui::{UiImage, UiText},
    ui_events::UiEvents,
    ui_layout::{ContainerStyle, UiRectLayout},
    ui_states::ButtonStateSet,
    world_text::WorldText,
};

/// 把组件里的资源路径解析成资源库身份。
///
/// 只返回库中**已有记录**的 ID；解析不到就必须返回 `None`。刻意不提供"按路径现编
/// 一个 ID"的兜底：资源身份的唯一真源是资源库（它按 `path:importer_version` 播种，
/// 见 `core/resource_db.rs` 的 `register`），另编一套会让调用方把"未解析"误当成
/// 真实身份。
pub trait AssetResolver {
    fn asset_id(&self, source_path: &str) -> Option<AssetId>;
}

impl AssetResolver for ResourceDatabase {
    fn asset_id(&self, source_path: &str) -> Option<AssetId> {
        // 先按规范化路径精确命中；Windows 上引用大小写与磁盘不一致时退化为大小写
        // 不敏感查找（与 core/dependency.rs 的解析口径一致）。
        if let Ok(Some(record)) = self.find_by_path(source_path) {
            return Some(record.id);
        }
        self.find_by_path_ci(source_path)
    }
}

/// 场景组件名对应的类型化键。
///
/// **变换与相机各只有一个键**：2D/3D 用同一个组件名、同一份表示（`Transform` 是统一 TRS，
/// `Camera` 用 `projection` 判别字段承载两种投影）。`Collider` 与 `Collider3d` 仍然是两套
/// 独立真源——那对组件没有统一，合并会让"哪个是权威"变成隐式推断（docs/40 §5）。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ComponentId {
    /// 唯一的变换组件（2D 与 3D 共用；空间由 `spatial_transform::entity_space` 推断）。
    Transform,
    /// 唯一的相机组件（`Camera` 里带显式投影分支）。
    Camera,
    Light,
    MeshRenderer,
    UiLayout,
    Container,
    Text,
    Image,
    UiEvents,
    ButtonStates,
    WorldText,
    Sprite,
    /// docs/54 §4.3：一等 Quad（几何 + 材质，贴图可选）。
    Quad,
    Tilemap,
    ParticleEmitter,
    Collider,
    Collider3d,
    Animation,
    Animation3d,
    /// docs/54 §4.1：每实体 `Script` 组件（脚本归属从工程级下放到实体）。
    Script,
    /// docs/54 §4.3：显式 2D 显示模式（world / overlay / screen）。
    RenderMode,
}

impl ComponentId {
    /// 运行时认识的**全部**组件名（[`Self::as_str`] 的返回值就是场景/Prefab 里写的那个名字）。
    ///
    /// 存在的理由：`EntityScript`/`Script` 与 `Particle`/`ParticleEmitter` 是两代**键名漂移**
    /// ——注册表描述的名字没人消费，而真实工程里的组件被审计报"没有 schema"。
    /// 逐个写断言挡不住第三例，所以把枚举与运行时名字放在一处，
    /// 让 `component_schema` 的 `every_component_id_the_runtime_knows_has_a_schema`
    /// **机械化**地断言"运行时认识的每个组件名都有 schema"。
    ///
    /// 新增组件时的顺序：先在这里加变体与名字，再在 `compile_component` 加深分支，
    /// 最后在 `component_schema::builtin_registry` 注册（漏了最后一步上面那条测试会判红）。
    pub const ALL: &'static [Self] = &[
        Self::Transform,
        Self::Camera,
        Self::Light,
        Self::MeshRenderer,
        Self::UiLayout,
        Self::Container,
        Self::Text,
        Self::Image,
        Self::UiEvents,
        Self::ButtonStates,
        Self::WorldText,
        Self::Sprite,
        Self::Quad,
        Self::Tilemap,
        Self::ParticleEmitter,
        Self::Collider,
        Self::Collider3d,
        Self::Animation,
        Self::Animation3d,
        Self::Script,
        Self::RenderMode,
    ];

    /// 场景/Prefab 文件里写的组件名（与 `compile_component` 的 match 分支一一对应）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Transform => "Transform",
            Self::Camera => crate::core::camera::CAMERA_COMPONENT,
            Self::Light => "Light",
            Self::MeshRenderer => "MeshRenderer",
            Self::UiLayout => "UiLayout",
            Self::Container => "Container",
            Self::Text => "Text",
            Self::Image => "Image",
            Self::UiEvents => "UiEvents",
            Self::ButtonStates => "ButtonStates",
            Self::WorldText => "WorldText",
            Self::Sprite => "Sprite",
            Self::Quad => "Quad",
            Self::Tilemap => "Tilemap",
            Self::ParticleEmitter => "ParticleEmitter",
            Self::Collider => "Collider",
            Self::Collider3d => "Collider3d",
            Self::Animation => "Animation",
            Self::Animation3d => "Animation3D",
            Self::Script => "Script",
            Self::RenderMode => "RenderMode",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypedComponent {
    /// 唯一的变换组件，值就是统一 TRS（2D 消费者用 `LocalTransform::to_2d` 投影）。
    Transform(Transform3D),
    /// 唯一的相机组件，值是带投影分支的统一取值。
    Camera(crate::core::camera::Camera),
    Light(Light),
    MeshRenderer {
        renderer: MeshRenderer,
        /// `None` = 未解析（没有资源库或库里没有这条记录）。不要用编造的 ID 顶替。
        asset: Option<AssetId>,
    },
    UiLayout(UiRectLayout),
    Container(ContainerStyle),
    Text(UiText),
    Image(UiImage),
    UiEvents(UiEvents),
    ButtonStates(ButtonStateSet),
    WorldText(WorldText),
    Sprite(SpriteComponent),
    /// docs/54 §4.3：一等 Quad。它不走 `Sprite` 那个槽——两者的差别是**有没有纹理区域**，
    /// 合并成一个类型会让"这个矩形到底会不会被帧动画改 UV"变成隐式推断。
    Quad(Quad),
    Tilemap(Tilemap),
    /// docs/54 §8-①：粒子发射器是**场景组件**（连续发射由 `rate` 表达），
    /// 因此"同一场景表达 Sprite/Mesh/相机/粒子/文字/UI"不再需要脚本代劳。
    ParticleEmitter(ParticleEmitter),
    Collider(ColliderDesc),
    Collider3d(ColliderDesc3d),
    Animation(AnimationStateMachine),
    Animation3d(Animation3dDescriptor),
    /// docs/54 §4.1：每实体脚本（文件 + 启用开关）。
    Script(EntityScript),
    /// docs/54 §4.3：显式 2D 显示模式。
    RenderMode(crate::core::render_mode::RenderMode),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypedEntity {
    pub id: SceneEntityId,
    pub name: String,
    pub parent: Option<SceneEntityId>,
    pub components: BTreeMap<ComponentId, TypedComponent>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypedScene {
    pub source_version: u32,
    pub name: String,
    pub entities: Vec<TypedEntity>,
    pub parent_first: Vec<SceneEntityId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedSceneDiagnostic {
    pub entity: SceneEntityId,
    pub name: String,
    pub component: String,
    pub message: String,
}

impl fmt::Display for TypedSceneDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "entity {} ({}) component {}: {}",
            self.entity.0, self.name, self.component, self.message
        )
    }
}

impl std::error::Error for TypedSceneDiagnostic {}

/// 编译一条 v2 组件记录。
///
/// 返回 `None` 表示组件名不在已知集合内——未知、平台自定义与 Lua 动态组件都留在
/// 源场景里，这里刻意不解释，避免把它们静默当成已知组件。`Some(Err)` 是已知组件
/// 但内容非法，必须按实体/组件上报诊断而不是跳过。
///
/// 覆盖面按**真实源场景用量**排序（26 个源场景普查，2026-09-17；旧名已随统一合并）：
/// Transform 24、UiLayout 23、Text 23、Sprite 15、Tilemap 12、Collider 11、Animation 10、
/// UiEvents/Image 各 6、Light/Camera/MeshRenderer 各 3、Collider3d/Animation3D 各 2、
/// ButtonStates/Container 各 1。
fn compile_component(
    name: &str,
    value: &str,
    assets: Option<&dyn AssetResolver>,
) -> Option<Result<(ComponentId, TypedComponent), String>> {
    let parsed = match name {
        // **唯一的变换组件**：载荷只有一个形态（`position/rotation/scale`）。2D 实体用的是
        // 同一个组件名、同一个形态——空间差别由实体携带的其它组件推断
        // （`spatial_transform::entity_space`），2D 消费方用 `LocalTransform::to_2d` 投影。
        "Transform" => LocalTransform::from_unified_component_str(value).map(|local| {
            (
                ComponentId::Transform,
                TypedComponent::Transform(Transform3D {
                    position: local.translation,
                    rotation: local.rotation,
                    scale: local.scale,
                }),
            )
        }),
        // **唯一的相机组件**：值里带显式投影分支，消费方按投影分支处理。
        "Camera" => crate::core::camera::Camera::from_component_str(value)
            .map(|component| (ComponentId::Camera, TypedComponent::Camera(component))),
        "Light" => Light::from_component_str(value)
            .map(|component| (ComponentId::Light, TypedComponent::Light(component)))
            .map_err(|error| error.to_string()),
        "MeshRenderer" => MeshRenderer::from_component_str(value)
            .map(|renderer| {
                // 资源身份一律问资源库。库里没有该路径时保持 `None`：`mesh` 也可能是
                // 引擎内置哨兵（如地面平面），不是每个值都有资源记录。断链的可见性由
                // 资源库自己的依赖图与校验负责，这里不重复报诊断。
                let asset = assets.and_then(|resolver| resolver.asset_id(&renderer.mesh));
                (
                    ComponentId::MeshRenderer,
                    TypedComponent::MeshRenderer { renderer, asset },
                )
            })
            .map_err(|error| error.to_string()),
        "UiLayout" => UiRectLayout::from_component_str(value)
            .map(|component| (ComponentId::UiLayout, TypedComponent::UiLayout(component))),
        "Container" => ContainerStyle::from_component_str(value)
            .map(|component| (ComponentId::Container, TypedComponent::Container(component))),
        "Text" => UiText::from_component_str(value)
            .map(|component| (ComponentId::Text, TypedComponent::Text(component)))
            .map_err(|error| error.to_string()),
        "Image" => UiImage::from_component_str(value)
            .map(|component| (ComponentId::Image, TypedComponent::Image(component)))
            .map_err(|error| error.to_string()),
        "UiEvents" => UiEvents::parse(value)
            .map(|component| (ComponentId::UiEvents, TypedComponent::UiEvents(component)))
            .map_err(|error| error.to_string()),
        "ButtonStates" => ButtonStateSet::from_component_str(value).map(|component| {
            (
                ComponentId::ButtonStates,
                TypedComponent::ButtonStates(component),
            )
        }),
        // WP4：世界空间文字（锚在世界位置）。
        "WorldText" => WorldText::from_component_str(value)
            .map(|component| (ComponentId::WorldText, TypedComponent::WorldText(component)))
            .map_err(|error| error.to_string()),
        "Sprite" => SpriteComponent::from_component_str(value)
            .map(|component| (ComponentId::Sprite, TypedComponent::Sprite(component)))
            .map_err(|error| error.to_string()),
        // docs/54 §4.3：一等 Quad（几何 + 材质，贴图可选）。它与 `Sprite` 是两个独立组件而不是
        // 同一个槽的两个写法：Quad 没有纹理区域、也不参与帧动画，语义上就是"一个矩形"。
        "Quad" => Quad::from_component_str(value)
            .map(|component| (ComponentId::Quad, TypedComponent::Quad(component)))
            .map_err(|error| error.to_string()),
        "Tilemap" => Tilemap::from_component_str(value)
            .map(|component| (ComponentId::Tilemap, TypedComponent::Tilemap(component)))
            .map_err(|error| error.to_string()),
        // docs/54 §8-①：粒子发射器与其它渲染组件并列，不再是"只有脚本才能造"的东西。
        "ParticleEmitter" => ParticleEmitter::from_component_str(value)
            .map(|component| {
                (
                    ComponentId::ParticleEmitter,
                    TypedComponent::ParticleEmitter(component),
                )
            })
            .map_err(|error| error.to_string()),
        "Collider" => ColliderDesc::from_component_str(value)
            .map(|component| (ComponentId::Collider, TypedComponent::Collider(component)))
            .map_err(|error| error.to_string()),
        "Collider3d" => ColliderDesc3d::from_component_str(value)
            .map(|component| {
                (
                    ComponentId::Collider3d,
                    TypedComponent::Collider3d(component),
                )
            })
            .map_err(|error| error.to_string()),
        "Animation" => AnimationStateMachine::from_component_str(value)
            .map(|component| (ComponentId::Animation, TypedComponent::Animation(component)))
            .map_err(|error| error.to_string()),
        "Animation3D" => Animation3dDescriptor::from_component_str(value)
            .map(|component| {
                (
                    ComponentId::Animation3d,
                    TypedComponent::Animation3d(component),
                )
            })
            .map_err(|error| error.to_string()),
        // docs/54 §4.1：脚本归属从"工程级一份"下放到实体。
        "Script" => EntityScript::from_component_str(value)
            .map(|component| (ComponentId::Script, TypedComponent::Script(component)))
            .map_err(|error| error.to_string()),
        // docs/54 §4.3：显式 2D 显示模式——"这段几何走哪条路"从全局开关下放到实体。
        "RenderMode" => crate::core::render_mode::RenderMode::from_component_str(value)
            .map(|component| {
                (
                    ComponentId::RenderMode,
                    TypedComponent::RenderMode(component),
                )
            })
            .map_err(|error| error.to_string()),
        _ => return None,
    };
    Some(parsed)
}

/// 组件载荷的**引擎校验**（唯一事实源，供编辑器在字段写入后兜底）。
///
/// 返回值三态：
/// - `None`：组件名不在引擎的已知集合里（Lua/平台自定义组件），引擎刻意不解释它；
/// - `Some(Ok(()))`：引擎自己的 typed 解析器能读这条载荷；
/// - `Some(Err(reason))`：已知组件但载荷非法，`reason` 是引擎解析器的原文。
///
/// 为什么需要这条入口：schema 描述的是**字符串契约**（字段类型 + 声明式规则），而引擎
/// 解析器还管着 schema 表达不了的**跨字段/几何不变量**（`Camera` 的 `near < far`、
/// look-at 非退化、`Transform` 的缩放非零……）。编辑器允许写入的载荷至少必须是引擎能读的，
/// 因此字段写入后跑一次这里——校验逻辑只有解析器一处定义，编辑器不另写一份，也不会漂移。
#[must_use]
pub fn validate_component_payload(name: &str, value: &str) -> Option<Result<(), String>> {
    compile_component(name, value, None).map(|parsed| parsed.map(|_| ()))
}

impl TypedScene {
    /// 无资源库的编译：`MeshRenderer` 的资源身份保持未解析。
    pub fn compile_v2(scene: &Scene) -> Result<Self, Vec<TypedSceneDiagnostic>> {
        Self::compile_v2_with_assets(scene, None)
    }

    /// 在加载边界一次性编译已知的 v2 组件。未知/动态组件仍留在源场景里，
    /// 这里刻意不解释它们——否则 Lua 自定义组件会被静默当成已知组件。
    ///
    /// 传入资源库时按真实身份解析资源引用；不传则资源字段为 `None`（未解析）。
    pub fn compile_v2_with_assets(
        scene: &Scene,
        assets: Option<&dyn AssetResolver>,
    ) -> Result<Self, Vec<TypedSceneDiagnostic>> {
        let (entities, mut diagnostics) = compile_entities(scene, assets);
        if !diagnostics.is_empty() {
            return Err(diagnostics);
        }
        let parent_first = parent_first_order(&entities, &mut diagnostics);
        if !diagnostics.is_empty() {
            return Err(diagnostics);
        }
        Ok(Self {
            source_version: scene.version,
            name: scene.name.clone(),
            entities,
            parent_first,
        })
    }

    /// 宽容编译：非法组件被丢弃并记为诊断，父链错误只记录不中断。
    ///
    /// 与 `compile_v2` 的全有或全无并存：编辑器场景视图需要"坏一个组件不至于整屏
    /// 空白"，关卡装载则需要 fail-fast。
    pub fn compile_v2_lenient(scene: &Scene) -> (Self, Vec<TypedSceneDiagnostic>) {
        let (entities, mut diagnostics) = compile_entities(scene, None);
        let parent_first = parent_first_order(&entities, &mut diagnostics);
        (
            Self {
                source_version: scene.version,
                name: scene.name.clone(),
                entities,
                parent_first,
            },
            diagnostics,
        )
    }
}

/// 逐实体编译组件：未知组件跳过，非法组件记诊断但不中断。
fn compile_entities(
    scene: &Scene,
    assets: Option<&dyn AssetResolver>,
) -> (Vec<TypedEntity>, Vec<TypedSceneDiagnostic>) {
    let mut diagnostics = Vec::new();
    let mut entities = Vec::with_capacity(scene.entities.len());
    for source in scene.entities.values() {
        let mut components = BTreeMap::new();
        for (name, value) in &source.components {
            let Some(parsed) = compile_component(name, value, assets) else {
                continue;
            };
            match parsed {
                Ok((id, component)) => {
                    components.insert(id, component);
                }
                Err(message) => diagnostics.push(TypedSceneDiagnostic {
                    entity: source.id,
                    name: source.name.clone(),
                    component: name.clone(),
                    message,
                }),
            }
        }
        entities.push(TypedEntity {
            id: source.id,
            name: source.name.clone(),
            parent: source.parent,
            components,
        });
    }
    (entities, diagnostics)
}

/// 父先序实体顺序。父链错误记入诊断并跳过出错实体，其余实体继续入序——宽容模式
/// 需要一个可用的（可能不完整的）顺序，而严格模式在这之后按诊断非空直接失败。
///
/// 出错实体在回退时标记为"已完成"，避免同一处错误被外层循环重复上报。
fn parent_first_order(
    entities: &[TypedEntity],
    diagnostics: &mut Vec<TypedSceneDiagnostic>,
) -> Vec<SceneEntityId> {
    let mut order = Vec::with_capacity(entities.len());
    let by_id = entities
        .iter()
        .map(|entity| (entity.id, entity))
        .collect::<BTreeMap<_, _>>();
    let mut states = BTreeMap::new();
    fn visit(
        id: SceneEntityId,
        by_id: &BTreeMap<SceneEntityId, &TypedEntity>,
        states: &mut BTreeMap<SceneEntityId, u8>,
        order: &mut Vec<SceneEntityId>,
        diagnostics: &mut Vec<TypedSceneDiagnostic>,
    ) -> Result<(), ()> {
        match states.get(&id).copied() {
            Some(2) => return Ok(()),
            Some(1) => {
                let entity = by_id[&id];
                diagnostics.push(TypedSceneDiagnostic {
                    entity: id,
                    name: entity.name.clone(),
                    component: "parent".to_owned(),
                    message: "parent cycle".to_owned(),
                });
                return Err(());
            }
            _ => {}
        }
        states.insert(id, 1);
        let entity = by_id[&id];
        if let Some(parent) = entity.parent {
            if !by_id.contains_key(&parent) {
                diagnostics.push(TypedSceneDiagnostic {
                    entity: id,
                    name: entity.name.clone(),
                    component: "parent".to_owned(),
                    message: format!("missing parent {}", parent.0),
                });
                states.insert(id, 2);
                return Err(());
            }
            if visit(parent, by_id, states, order, diagnostics).is_err() {
                states.insert(id, 2);
                return Err(());
            }
        }
        states.insert(id, 2);
        order.push(id);
        Ok(())
    }
    for id in by_id.keys().copied() {
        // 单个实体的父链错误不阻断其余实体；错误已记入诊断。
        let _ = visit(id, &by_id, &mut states, &mut order, diagnostics);
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        math3d::Mat4, resource_db::ResourceDatabase, resource_kind::ResourceKind,
        scene_extract::extract_serialized_scene_3d,
    };

    const MESH: &str = "mesh=assets/models/cube.glb";

    fn scene_with(id: SceneEntityId, name: &str, components: &[(&str, &str)]) -> Scene {
        let mut scene = Scene::new("typed");
        scene.add_entity(id, name, None).unwrap();
        let entity = scene.entities.get_mut(&id).unwrap();
        for (key, value) in components {
            entity
                .components
                .insert((*key).to_owned(), (*value).to_owned());
        }
        scene
    }

    /// 每个真实源场景出现过的组件名各取一个合法样例；这张表同时锁定"名字 →
    /// ComponentId"的映射，改名或漏接会在编译期或断言处暴露。
    const SOURCE_SCENE_COMPONENTS: &[(&str, &str, ComponentId)] = &[
        (
            "Transform",
            "position=1,2,3;rotation=0,0,0,1;scale=1,1,1",
            ComponentId::Transform,
        ),
        (
            "Camera",
            "projection=perspective;position=0,0,5;target=0,0,0;fov=90;near=0.1;far=100",
            ComponentId::Camera,
        ),
        (
            "Light",
            "direction=0.4,0.8,0.5;color=1,1,1;intensity=2",
            ComponentId::Light,
        ),
        (
            "MeshRenderer",
            "mesh=assets/models/cube.glb",
            ComponentId::MeshRenderer,
        ),
        (
            "UiLayout",
            "anchors=0,0,0,0;offsets=0,0,0,0;min_size=0,0;max_size=100,100;pivot=0.5,0.5",
            ComponentId::UiLayout,
        ),
        (
            "Container",
            "kind=vbox;padding=0,0,0,0;spacing=4;columns=1;stretch=true;fill=false;align=center",
            ComponentId::Container,
        ),
        ("Text", "text=Hello;font_size=24", ComponentId::Text),
        (
            "Image",
            "texture=assets/textures/coin.png;color=1,1,1",
            ComponentId::Image,
        ),
        (
            "UiEvents",
            "event=click;function=on_click",
            ComponentId::UiEvents,
        ),
        ("ButtonStates", "normal=base.png", ComponentId::ButtonStates),
        (
            "Sprite",
            "texture=assets/textures/player.png;size=32x32;pivot=0.500,0.500;tint=1.000,1.000,1.000,1.000;sorting_layer=default;order=0",
            ComponentId::Sprite,
        ),
        ("Quad", "size=32,32;color=1,0,0,0.5", ComponentId::Quad),
        ("Script", "file=scripts/ai/patrol.lua", ComponentId::Script),
        ("RenderMode", "mode=screen", ComponentId::RenderMode),
        (
            "Tilemap",
            "tile_size=32x32;width=2;height=2;tiles=0000;solid=0000",
            ComponentId::Tilemap,
        ),
        ("Collider", "shape=ball;radius=16", ComponentId::Collider),
        (
            "Collider3d",
            "shape=ball;radius=16",
            ComponentId::Collider3d,
        ),
        (
            "Animation",
            "default=idle;clip:idle:assets/textures/hero-idle.png:4:4:true",
            ComponentId::Animation,
        ),
        (
            "Animation3D",
            "mesh=assets/models/warrior.glb;clip=Walk;looped=true",
            ComponentId::Animation3d,
        ),
    ];

    /// 脚本组件的名字只有**一个**：加载器认的名字必须就是
    /// [`crate::core::entity_script::SCRIPT_COMPONENT`]（历史上编辑器写 `LuaScript`、
    /// schema 写 `EntityScript`、运行时读 `Script`，三处不一致导致"设了脚本不生效"）。
    #[test]
    fn script_component_name_matches_the_shared_constant() {
        let name = crate::core::entity_script::SCRIPT_COMPONENT;
        assert_eq!(name, "Script");
        let mut scene = Scene::new("script-name");
        scene.add_entity(SceneEntityId(1), "E", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(name.to_owned(), "file=scripts/ai/patrol.lua".to_owned());
        let typed = TypedScene::compile_v2(&scene).unwrap();
        assert!(
            typed.entities[0]
                .components
                .contains_key(&ComponentId::Script),
            "加载器必须认得 {name}"
        );
        // schema 注册表也必须用同一个名字（见 component_schema 的守卫测试）。
        assert!(
            crate::core::component_schema::builtin_registry().contains_key(name),
            "{name} 必须有 schema"
        );
    }

    #[test]
    fn compiles_known_components_and_parent_topology_once() {
        let mut scene = Scene::new("typed");
        scene.add_entity(SceneEntityId(1), "Parent", None).unwrap();
        scene
            .add_entity(SceneEntityId(2), "Child", Some(SceneEntityId(1)))
            .unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "position=1,2,3;rotation=0,0,0,1;scale=1,1,1".into(),
            );
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                "MeshRenderer".into(),
                "mesh=assets/models/m8-test-triangle.glb".into(),
            );
        let typed = TypedScene::compile_v2(&scene).unwrap();
        assert_eq!(typed.parent_first, [SceneEntityId(1), SceneEntityId(2)]);
        let child = typed
            .entities
            .iter()
            .find(|entity| entity.id == SceneEntityId(2))
            .unwrap();
        assert!(child.components.contains_key(&ComponentId::Transform));
        assert!(child.components.contains_key(&ComponentId::MeshRenderer));
    }

    #[test]
    fn diagnostics_identify_entity_component_and_error() {
        let mut scene = Scene::new("invalid");
        scene.add_entity(SceneEntityId(7), "Broken", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(7))
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "position=bad;rotation=0,0,0,1;scale=1,1,1".into(),
            );
        let error = TypedScene::compile_v2(&scene).unwrap_err();
        assert_eq!(error[0].entity, SceneEntityId(7));
        assert_eq!(error[0].component, "Transform");
        assert!(error[0].to_string().contains("Broken"));
    }

    /// **统一 `Transform` / `Camera` 的编译**：变换只有一个键、一个形态；相机按键里的投影分支
    /// 取值。缺判别字段的两条兼容路径（旧名、按字段集合推断）已随统一删除——缺 `projection`
    /// 现在必须**报错**，因为"猜投影"猜错的代价是画面变了却没人知道。
    #[test]
    fn unified_transform_and_camera_compile_to_single_typed_variants() {
        let cases: &[(&str, &str, ComponentId)] = &[
            (
                "Transform",
                "position=1.000000000,2.000000000,3.000000000;rotation=0.000000000,0.000000000,0.000000000,1.000000000;scale=1.000000000,1.000000000,1.000000000",
                ComponentId::Transform,
            ),
            (
                "Camera",
                "projection=orthographic;center=10.0000,20.0000;zoom=2.000000",
                ComponentId::Camera,
            ),
            (
                "Camera",
                "projection=perspective;position=0.000000000,1.000000000,5.000000000;target=0.000000000,0.000000000,0.000000000;fov=60.0000;near=0.100000000;far=100.000000000",
                ComponentId::Camera,
            ),
        ];
        for (name, value, expected) in cases {
            let scene = scene_with(SceneEntityId(1), "Unified", &[(name, value)]);
            let typed = TypedScene::compile_v2(&scene)
                .unwrap_or_else(|error| panic!("{name} {value:?} 必须能编译：{error:?}"));
            let entity = &typed.entities[0];
            assert!(
                entity.components.contains_key(expected),
                "{name} {value:?} 应编译成 {expected:?}，实际 {:?}",
                entity.components.keys().collect::<Vec<_>>()
            );
            assert_eq!(entity.components.len(), 1, "{name} 只应产出一个类型化组件");
        }
        // 统一写法下的取值必须真的读进来（不是空壳）。
        let scene = scene_with(
            SceneEntityId(1),
            "Unified",
            &[(
                "Transform",
                "position=1.000000000,2.000000000,3.000000000;rotation=0.000000000,0.000000000,0.000000000,1.000000000;scale=1.000000000,1.000000000,1.000000000",
            )],
        );
        let typed = TypedScene::compile_v2(&scene).expect("编译");
        match &typed.entities[0].components[&ComponentId::Transform] {
            TypedComponent::Transform(transform) => {
                assert_eq!(
                    transform.position,
                    crate::core::math3d::Vec3::new(1.0, 2.0, 3.0)
                );
            }
            other => panic!("期望 Transform，实际 {other:?}"),
        }
        // 参考：同一个实体**没有**另一个变换键（统一之后不存在"两份局部真源"）。
        let scene = scene_with(
            SceneEntityId(1),
            "Unified",
            &[("Camera", "projection=orthographic;center=0,0;zoom=1")],
        );
        let typed = TypedScene::compile_v2(&scene).expect("编译");
        match &typed.entities[0].components[&ComponentId::Camera] {
            TypedComponent::Camera(camera) => assert_eq!(
                camera.projection(),
                crate::core::camera::CameraProjection::Orthographic
            ),
            other => panic!("期望 Camera，实际 {other:?}"),
        }
        // 判别字段与字段体不匹配 → 编译报错（不猜）。
        let scene = scene_with(
            SceneEntityId(1),
            "Unified",
            &[(
                "Camera",
                "projection=orthographic;position=0,0,5;target=0,0,0;fov=60;near=0.1;far=100",
            )],
        );
        assert!(TypedScene::compile_v2(&scene).is_err());
        // 缺判别字段 → 编译报错（旧名与"按字段集合推断"两条兼容路径已删除）。
        let scene = scene_with(
            SceneEntityId(1),
            "Unified",
            &[("Camera", "center=0,0;zoom=1")],
        );
        assert!(TypedScene::compile_v2(&scene).is_err());
    }

    /// 覆盖门：源场景里出现过的 17 个组件名必须全部编译成对应的 `ComponentId`，
    /// 且载荷确实是解析结果（不只是键存在）。
    #[test]
    fn covers_every_component_name_used_in_source_scenes() {
        let components: Vec<(&str, &str)> = SOURCE_SCENE_COMPONENTS
            .iter()
            .map(|(name, value, _)| (*name, *value))
            .collect();
        let scene = scene_with(SceneEntityId(1), "All", &components);
        let typed = TypedScene::compile_v2(&scene).expect("all source components must compile");
        let entity = &typed.entities[0];
        for (name, _, id) in SOURCE_SCENE_COMPONENTS {
            assert!(entity.components.contains_key(id), "{name} 未编译为 {id:?}");
        }
        assert_eq!(entity.components.len(), SOURCE_SCENE_COMPONENTS.len());

        // 抽样确认载荷是真正的解析结果。
        assert_eq!(
            entity.components[&ComponentId::Sprite],
            TypedComponent::Sprite(
                SpriteComponent::from_component_str(
                    "texture=assets/textures/player.png;size=32x32;pivot=0.500,0.500;tint=1.000,1.000,1.000,1.000;sorting_layer=default;order=0"
                )
                .unwrap()
            )
        );
        assert_eq!(
            entity.components[&ComponentId::Text],
            TypedComponent::Text(UiText::from_component_str("text=Hello;font_size=24").unwrap())
        );
    }

    /// 资源身份必须来自资源库，且与"仅按路径播种"不同——这条断言锁定 prototype
    /// 早期的 seed fallback 不会被重新引入。
    #[test]
    fn mesh_asset_resolves_through_the_resource_database() {
        let source_path = "assets/models/cube.glb";
        let root = std::env::temp_dir().join(format!("rustlua-wp1-02-{}", std::process::id()));
        let mut db = ResourceDatabase::open(&root).expect("opens an empty database");
        let registered = db
            .register(source_path, ResourceKind::Model, "gltf-1")
            .expect("registers the mesh source");
        assert_ne!(
            registered,
            AssetId::from_seed(source_path),
            "资源库按 path:importer_version 播种，与仅按路径播种必然不同"
        );

        let scene = scene_with(SceneEntityId(1), "Mesh", &[("MeshRenderer", MESH)]);
        let expected_renderer = MeshRenderer::from_component_str(MESH).unwrap();
        let typed =
            TypedScene::compile_v2_with_assets(&scene, Some(&db as &dyn AssetResolver)).unwrap();
        assert_eq!(
            typed.entities[0].components[&ComponentId::MeshRenderer],
            TypedComponent::MeshRenderer {
                renderer: expected_renderer.clone(),
                asset: Some(registered),
            }
        );

        // 没有资源库时保持"未解析"，不得编造一个 ID 顶上。
        let typed = TypedScene::compile_v2(&scene).unwrap();
        assert_eq!(
            typed.entities[0].components[&ComponentId::MeshRenderer],
            TypedComponent::MeshRenderer {
                renderer: expected_renderer,
                asset: None,
            }
        );
    }

    /// 未知与 Lua 动态组件留在源场景，既不解释也不报诊断——它们不是"非法已知组件"。
    #[test]
    fn unknown_components_are_left_in_the_source_scene() {
        let scene = scene_with(
            SceneEntityId(1),
            "Custom",
            &[
                ("Transform", "position=1,2,0;rotation=0,0,0,1;scale=1,1,1"),
                ("MyGameHealth", "hp=10"),
                ("LuaTable", "{}"),
            ],
        );
        let typed = TypedScene::compile_v2(&scene).unwrap();
        let entity = &typed.entities[0];
        assert_eq!(entity.components.len(), 1);
        assert!(entity.components.contains_key(&ComponentId::Transform));
        // 源场景原样保留，供 Lua / 编辑器回写使用。
        assert_eq!(
            scene.entities[&SceneEntityId(1)].components["MyGameHealth"],
            "hp=10"
        );
    }

    /// 编辑器字段写入的兜底门：三态语义（不认识的组件 / 引擎能读 / 引擎读不了），
    /// 且"引擎读不了"必须给出解析器原文——相机的 `near >= far` 就是 schema 的字段规则
    /// 表达不了、只能由解析器拦下的那类不变量。
    #[test]
    fn payload_validation_reports_unknown_and_engine_rejected_components() {
        // 自定义/Lua 组件：引擎刻意不解释（不是错误）。
        assert_eq!(validate_component_payload("MyGameHealth", "hp=10"), None);
        // 引擎能读。
        assert_eq!(
            validate_component_payload(
                "Camera",
                "projection=perspective;position=0,0,5;target=0,0,0;fov=90;near=0.1;far=100"
            ),
            Some(Ok(()))
        );
        // 已知组件但载荷非法：必须带原因，编辑器据此拒绝写入。
        let rejected = validate_component_payload(
            "Camera",
            "projection=perspective;position=0,0,5;target=0,0,0;fov=90;near=10;far=1",
        )
        .expect("Camera 是已知组件");
        assert!(rejected.is_err(), "near > far 必须被判红");
        assert!(
            !rejected.unwrap_err().is_empty(),
            "拒绝原因不能是空串（编辑器要把它显示给用户）"
        );
        // 落点退化（position == target）同样只有解析器拦得住。
        assert!(
            validate_component_payload(
                "Camera",
                "projection=perspective;position=0,0,5;target=0,0,5;fov=90;near=0.1;far=100"
            )
            .expect("known")
            .is_err()
        );
        // 判别字段必填：缺了它连"这是哪种相机"都不知道，必须拒绝。
        assert!(
            validate_component_payload(
                "Camera",
                "position=0,0,5;target=0,0,0;fov=90;near=0.1;far=100"
            )
            .expect("known")
            .is_err()
        );
    }

    /// 每个新增组件族的非法载荷都必须报诊断，且带上组件名；不能静默丢弃。
    #[test]
    fn invalid_payload_of_any_known_component_is_diagnosed() {
        for (name, bad_value) in [
            ("Sprite", "texture=;size=32"),
            ("Tilemap", "width=2;height=2"),
            ("Collider", "shape=ball"),
            ("UiLayout", "anchors=2,0,0,0"),
            ("Container", "kind=unknown"),
            ("Text", "text=Hi;font_size=0"),
            ("UiEvents", "event=bogus;function=f"),
            ("ButtonStates", "bad=1"),
            ("Animation", "clip=Walk"),
            ("Animation3D", "clip=Walk"),
            ("Collider3d", "shape=ball"),
        ] {
            let scene = scene_with(SceneEntityId(1), "Broken", &[(name, bad_value)]);
            let errors = match TypedScene::compile_v2(&scene) {
                Ok(_) => panic!("{name} 的非法载荷必须报错"),
                Err(errors) => errors,
            };
            assert_eq!(errors[0].component, name);
            assert!(!errors[0].message.is_empty(), "{name} 的诊断必须带可读原因");
        }
    }

    /// 宽容编译：坏组件只丢自己并记诊断，同场景的好实体照常可用——编辑器场景视图
    /// 依赖这一点，否则一个坏的 Sprite 就会让整个 3D 视图空白。
    #[test]
    fn lenient_compile_keeps_good_entities_and_reports_bad_ones() {
        let mut scene = Scene::new("lenient");
        scene.add_entity(SceneEntityId(1), "Good", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "position=1,2,3;rotation=0,0,0,1;scale=1,1,1".into(),
            );
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert("MeshRenderer".into(), MESH.into());
        scene.add_entity(SceneEntityId(2), "Bad", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert("Sprite".into(), "texture=;size=32".into());
        scene
            .add_entity(SceneEntityId(3), "Child", Some(SceneEntityId(2)))
            .unwrap();

        // 严格模式按既有语义失败。
        assert!(TypedScene::compile_v2(&scene).is_err());

        let (typed, diagnostics) = TypedScene::compile_v2_lenient(&scene);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].entity, SceneEntityId(2));
        assert_eq!(diagnostics[0].component, "Sprite");

        let good = typed
            .entities
            .iter()
            .find(|entity| entity.id == SceneEntityId(1))
            .unwrap();
        assert!(good.components.contains_key(&ComponentId::Transform));
        assert!(good.components.contains_key(&ComponentId::MeshRenderer));
        // 坏组件的实体本身保留（只是丢了那个组件），父子拓扑仍然完整。
        assert!(typed.parent_first.contains(&SceneEntityId(1)));
        assert!(typed.parent_first.contains(&SceneEntityId(2)));
        assert!(typed.parent_first.contains(&SceneEntityId(3)));
    }

    fn matrices_match(left: Mat4, right: Mat4) -> bool {
        left.columns
            .iter()
            .flatten()
            .zip(right.columns.iter().flatten())
            .all(|(a, b)| (a - b).abs() < 1e-5)
    }

    /// WP1-3 对拍门：把项目里**全部真实源场景**编译一遍并与旧提取路径逐项对拍。
    ///
    /// 断言三件事：① 每个场景都能无诊断编译；② 场景中的已知组件数与源组件数一致
    /// （若某个组件名未被 typed 覆盖，这里会差出来——覆盖缺口必须补，不能跳过）；
    /// ③ 3D drawable 与 `extract_serialized_scene_3d` 的数量、名称、模型矩阵、mesh
    /// 路径以及相机/光源集合完全一致。
    ///
    /// 场景目录缺失时**不静默通过**：本门的意义就是真的跑过全部工程场景。
    #[test]
    fn compiles_every_project_scene_and_matches_serialized_extraction() {
        let roots = [
            crate::core::games_dir().join("examples/game_api/scenes"),
            crate::core::games_dir().join("examples/game_jhyt/scenes"),
        ];
        let mut scene_count = 0;
        for root in roots {
            let entries = std::fs::read_dir(&root)
                .unwrap_or_else(|error| panic!("读取场景目录失败 {}: {error}", root.display()));
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("scene") {
                    continue;
                }
                let label = path.display().to_string();
                let scene = Scene::load(&path).unwrap_or_else(|error| panic!("{label}: {error}"));
                scene_count += 1;

                // ① 无诊断编译。
                let typed = TypedScene::compile_v2(&scene)
                    .unwrap_or_else(|errors| panic!("{label} 编译诊断：{errors:?}"));

                // ② 覆盖完整性：源组件总数必须与 typed 组件总数相等。
                let source_components: usize = scene
                    .entities
                    .values()
                    .map(|entity| entity.components.len())
                    .sum();
                let typed_components: usize = typed
                    .entities
                    .iter()
                    .map(|entity| entity.components.len())
                    .sum();
                assert_eq!(
                    typed_components, source_components,
                    "{label} 存在未被 typed 覆盖的组件名（源 {source_components} ≠ typed {typed_components}）"
                );

                // ③ 3D 对拍。
                let serialized = extract_serialized_scene_3d(&scene);
                assert!(
                    serialized.diagnostics.is_empty(),
                    "{label} 旧提取产生诊断：{:?}",
                    serialized.diagnostics
                );
                let old_3d: BTreeMap<&str, (Mat4, &str)> = serialized
                    .entities
                    .iter()
                    .map(|entity| {
                        (
                            entity.name.as_str(),
                            (entity.model, entity.mesh.mesh.as_str()),
                        )
                    })
                    .collect();
                let typed_3d: BTreeMap<&str, (Mat4, &str)> = typed
                    .entities
                    .iter()
                    .filter_map(|entity| {
                        let Some(TypedComponent::Transform(transform)) =
                            entity.components.get(&ComponentId::Transform)
                        else {
                            return None;
                        };
                        let Some(TypedComponent::MeshRenderer { renderer, .. }) =
                            entity.components.get(&ComponentId::MeshRenderer)
                        else {
                            return None;
                        };
                        // 与旧提取一致：非法变换的实体在旧路径会变成诊断而不是 drawable，
                        // 上面的诊断断言已保证这里不会静默丢实体。
                        let model = transform.model_matrix().ok()?;
                        Some((entity.name.as_str(), (model, renderer.mesh.as_str())))
                    })
                    .collect();
                assert_eq!(
                    typed_3d.len(),
                    old_3d.len(),
                    "{label} 新旧 3D drawable 数量不一致（typed {} ≠ 旧 {}）",
                    typed_3d.len(),
                    old_3d.len()
                );
                assert_eq!(
                    typed_3d.len(),
                    typed
                        .entities
                        .iter()
                        .filter(|entity| {
                            entity.components.contains_key(&ComponentId::Transform)
                                && entity.components.contains_key(&ComponentId::MeshRenderer)
                        })
                        .count(),
                    "{label} 3D 实体名重复，无法逐名对拍"
                );
                for (name, (model, mesh)) in &old_3d {
                    let (typed_model, typed_mesh) = typed_3d
                        .get(name)
                        .unwrap_or_else(|| panic!("{label} typed 缺少 3D 实体 {name}"));
                    assert!(
                        matrices_match(*model, *typed_model),
                        "{label} {name} 模型矩阵不一致"
                    );
                    assert_eq!(*mesh, *typed_mesh, "{label} {name} mesh 路径不一致");
                }

                // 相机与光源集合必须一致（旧路径按同一实体顺序收集）。
                let old_cameras: Vec<SceneEntityId> = serialized
                    .cameras
                    .iter()
                    .map(|(entity, _)| *entity)
                    .collect();
                let typed_cameras: Vec<SceneEntityId> = typed
                    .entities
                    .iter()
                    .filter(|entity| {
                        matches!(
                            entity.components.get(&ComponentId::Camera),
                            Some(TypedComponent::Camera(camera))
                                if camera.projection()
                                    == crate::core::camera::CameraProjection::Perspective
                        )
                    })
                    .map(|entity| entity.id)
                    .collect();
                assert_eq!(typed_cameras, old_cameras, "{label} 透视相机集合不一致");
                let old_lights: Vec<SceneEntityId> = serialized
                    .lights
                    .iter()
                    .map(|(entity, _)| *entity)
                    .collect();
                let typed_lights: Vec<SceneEntityId> = typed
                    .entities
                    .iter()
                    .filter(|entity| entity.components.contains_key(&ComponentId::Light))
                    .map(|entity| entity.id)
                    .collect();
                assert_eq!(typed_lights, old_lights, "{label} 光源集合不一致");
            }
        }
        assert!(scene_count > 0, "没有找到任何源场景，对拍门实际未执行");
    }

    /// docs/54 §8-①：**同一个场景**必须能同时表达 Sprite、Mesh、正交与透视相机、粒子、文字和 UI。
    ///
    /// 这条用真实场景文件（`examples/game_api/scenes/unified.scene`）而不是内存里拼一个模型：
    /// 判据要的是"场景格式 + Entity 模型 + typed 编译"三处都表达得出来，任何一处漏了都会在这里判红。
    #[test]
    fn one_scene_expresses_every_renderable_category() {
        let path = crate::core::games_dir().join("examples/game_api/scenes/unified.scene");
        let scene = Scene::load(&path).expect("load unified scene");
        let typed = TypedScene::compile_v2(&scene)
            .unwrap_or_else(|errors| panic!("unified.scene 编译诊断：{errors:?}"));

        let present: std::collections::BTreeSet<ComponentId> = typed
            .entities
            .iter()
            .flat_map(|entity| entity.components.keys().copied())
            .collect();
        for required in [
            // 2D 内容：精灵、一等 Quad、文字、UI、粒子发射器
            ComponentId::Sprite,
            ComponentId::Quad,
            ComponentId::WorldText,
            ComponentId::Text,
            ComponentId::UiLayout,
            ComponentId::ParticleEmitter,
            // 3D 内容：网格、光源
            ComponentId::MeshRenderer,
            ComponentId::Light,
            // 空间与相机：各**一个**组件（2D/3D 由载荷与判别字段表达，不是两个组件）
            ComponentId::Transform,
            ComponentId::Camera,
        ] {
            assert!(
                present.contains(&required),
                "unified.scene 缺少 {required:?}：一份场景要能同时表达全部渲染类别，实际 {present:?}"
            );
        }
        // 相机**两种投影**都要在这个场景里出现（只有一种就证明不了"一份场景同时表达 2D 与 3D"）。
        let projections: std::collections::BTreeSet<&str> = typed
            .entities
            .iter()
            .filter_map(|entity| match entity.components.get(&ComponentId::Camera) {
                Some(TypedComponent::Camera(camera)) => Some(camera.projection().as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            projections,
            ["orthographic", "perspective"].into_iter().collect(),
            "unified.scene 必须同时声明正交与透视相机"
        );
        // 粒子发射器必须是"连续"的，否则"场景表达粒子"就只是声明了一次爆发。
        let emitter = typed
            .entities
            .iter()
            .find_map(
                |entity| match entity.components.get(&ComponentId::ParticleEmitter) {
                    Some(TypedComponent::ParticleEmitter(emitter)) => Some(*emitter),
                    _ => None,
                },
            )
            .expect("particle emitter component");
        assert!(emitter.rate > 0.0, "场景声明的发射器应带连续发射速率");
    }
}
