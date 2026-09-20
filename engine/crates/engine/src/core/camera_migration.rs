//! 摄像机组件的**统一迁移**：编辑器只保留一个 `Camera` 组件。
//!
//! 纪律与 [`crate::core::transform_migration`] 完全一致（内存迁移、幂等、坏数据不猜、
//! 磁盘文件在用户保存前不动）。区别在语义：相机**不像变换那样同构**——2D 相机是正交视口
//! （`center`/`zoom`/`bounds`），3D 相机是透视（`position`/`target`/`fov`/`near`/`far`）。
//!
//! 因此统一组件**不做"把 2D 相机改造成透视相机"这种会改变画面的映射**，而是用一个显式的
//! **投影判别字段**承载两种语义：
//!
//! ```text
//! projection=orthographic;center=..,..;rotation=..;zoom=..[;bounds=..][;enabled=false]
//! projection=perspective;position=..,..,..;target=..,..,..;fov=..;near=..;far=..[;pose=transform][;enabled=false]
//! ```
//!
//! 迁移只做三件事：**改名**（`Camera2D`/`Camera3D` → `Camera`）、**补判别字段**、
//! **规范化数字格式**（交给各自既有的编码器，不重写数字格式）。判别字段写在**最前**，
//! 便于检索与人工核对。

use crate::core::{
    camera::{Camera2D, Camera3D},
    scene::{Scene, SceneEntityId},
};

/// 统一后的组件名。
pub const CAMERA_COMPONENT: &str = "Camera";
/// 旧的 2D 组件名。
pub const LEGACY_CAMERA_2D_COMPONENT: &str = "Camera2D";
/// 旧的 3D 组件名。
pub const LEGACY_CAMERA_3D_COMPONENT: &str = "Camera3D";
/// 投影判别字段名。
pub const PROJECTION_FIELD: &str = "projection";

/// 统一组件承载的两种投影语义。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CameraProjection {
    Orthographic,
    Perspective,
}

impl CameraProjection {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Orthographic => "orthographic",
            Self::Perspective => "perspective",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "orthographic" => Some(Self::Orthographic),
            "perspective" => Some(Self::Perspective),
            _ => None,
        }
    }
}

/// 统一 `Camera` 组件的**取值**：一个显式的投影分支。
///
/// 消费方（编辑器 / 运行时 / 场景编译）拿到它之后**按投影分支**处理，而不是按组件名——
/// 这正是"不再区分 2D/3D 组件"要表达的语义。旧名字仍然是合法输入
/// （[`Self::from_component_str`] 同时接受 `Camera2D`/`Camera3D`/`Camera`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CameraComponent {
    Orthographic(Camera2D),
    Perspective(Camera3D),
}

impl CameraComponent {
    /// 从**任意历史写法**读取：`Camera2D` / `Camera3D` / `Camera`（含缺判别字段的兼容路径）。
    pub fn from_component_str(name: &str, payload: &str) -> Result<Self, String> {
        match name {
            LEGACY_CAMERA_2D_COMPONENT => Camera2D::from_component_str(payload)
                .map(Self::Orthographic)
                .map_err(|error| error.to_string()),
            LEGACY_CAMERA_3D_COMPONENT => Camera3D::from_component_str(payload)
                .map(Self::Perspective)
                .map_err(|error| error.to_string()),
            CAMERA_COMPONENT => {
                let (declared, rest) = split_projection(payload);
                let projection = match declared {
                    Some(projection) => projection,
                    None => infer_projection(&rest).ok_or_else(|| {
                        format!(
                            "{CAMERA_COMPONENT} 缺少 {PROJECTION_FIELD} 字段，且无法从字段集合判定投影"
                        )
                    })?,
                };
                Self::from_projection(projection, &rest)
            }
            other => Err(format!("不是相机组件：{other}")),
        }
    }

    /// 按已知投影读取**去掉判别字段之后**的字段串。
    pub fn from_projection(
        projection: CameraProjection,
        rest: &str,
    ) -> Result<Self, String> {
        match projection {
            CameraProjection::Orthographic => Camera2D::from_component_str(rest)
                .map(Self::Orthographic)
                .map_err(|error| error.to_string()),
            CameraProjection::Perspective => Camera3D::from_component_str(rest)
                .map(Self::Perspective)
                .map_err(|error| error.to_string()),
        }
    }

    #[must_use]
    pub const fn projection(self) -> CameraProjection {
        match self {
            Self::Orthographic(_) => CameraProjection::Orthographic,
            Self::Perspective(_) => CameraProjection::Perspective,
        }
    }

    #[must_use]
    pub const fn enabled(self) -> bool {
        match self {
            Self::Orthographic(camera) => camera.enabled,
            Self::Perspective(camera) => camera.enabled,
        }
    }

    #[must_use]
    pub fn as_orthographic(self) -> Option<Camera2D> {
        match self {
            Self::Orthographic(camera) => Some(camera),
            Self::Perspective(_) => None,
        }
    }

    #[must_use]
    pub fn as_perspective(self) -> Option<Camera3D> {
        match self {
            Self::Perspective(camera) => Some(camera),
            Self::Orthographic(_) => None,
        }
    }

    /// 统一写法（判别字段在前，字段体来自各自既有的编码器）。
    #[must_use]
    pub fn to_component_str(self) -> String {
        match self {
            Self::Orthographic(camera) => {
                canonical_payload(CameraProjection::Orthographic, &camera.to_component_str())
            }
            Self::Perspective(camera) => {
                canonical_payload(CameraProjection::Perspective, &camera.to_component_str())
            }
        }
    }
}

/// 一次场景级迁移的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CameraMigrationOutcome {
    /// 被改动的实体数（改名 / 补判别字段 / 合并都算一条）。
    pub changed_entities: usize,
    /// 解析失败、**原样保留**的组件：`(实体, 组件名, 原因)`。
    pub skipped: Vec<(SceneEntityId, String, String)>,
}

impl CameraMigrationOutcome {
    #[must_use]
    pub const fn changed(&self) -> bool {
        self.changed_entities > 0
    }
}

/// 把一个相机组件归一成统一写法。
///
/// - `Ok(None)`：已经是统一写法（不必改动）；
/// - `Ok(Some(payload))`：迁移后的组件串（调用方写回 `Camera` 键，并删掉旧键）；
/// - `Err(reason)`：解析失败（调用方原样保留）。
pub fn unified_camera_payload(name: &str, payload: &str) -> Result<Option<String>, String> {
    match name {
        LEGACY_CAMERA_2D_COMPONENT | LEGACY_CAMERA_3D_COMPONENT => {
            let camera = CameraComponent::from_component_str(name, payload)?;
            Ok(Some(camera.to_component_str()))
        }
        CAMERA_COMPONENT => {
            // 统一值是唯一事实源：它同时负责"缺判别字段时的兼容判定"与字段体校验。
            let camera = CameraComponent::from_component_str(name, payload)?;
            let canonical = camera.to_component_str();
            Ok((canonical != payload).then_some(canonical))
        }
        _ => Ok(None),
    }
}

/// 统一写法：判别字段在前，其后是**各自解析器**写出的字段串。
fn canonical_payload(projection: CameraProjection, body: &str) -> String {
    format!("{PROJECTION_FIELD}={};{body}", projection.as_str())
}

/// 从载荷里取出判别字段，并返回**去掉它之后**的其余字段串（保持原有顺序）。
fn split_projection(payload: &str) -> (Option<CameraProjection>, String) {
    let mut declared = None;
    let mut rest: Vec<&str> = Vec::new();
    for field in payload.split(';') {
        if field.trim().is_empty() {
            continue;
        }
        match field.split_once('=') {
            Some((key, value)) if key.trim() == PROJECTION_FIELD => {
                declared = CameraProjection::parse(value);
            }
            _ => rest.push(field),
        }
    }
    (declared, rest.join(";"))
}

/// 按字段集合推断投影：`center`/`zoom`/`bounds` 属于正交；`position`/`target`/`fov` 属于透视。
fn infer_projection(rest: &str) -> Option<CameraProjection> {
    let mut orthographic = false;
    let mut perspective = false;
    for field in rest.split(';') {
        let Some((key, _)) = field.split_once('=') else {
            continue;
        };
        match key.trim() {
            "center" | "zoom" | "bounds" => orthographic = true,
            "position" | "target" | "fov" | "near" | "far" => perspective = true,
            _ => {}
        }
    }
    match (orthographic, perspective) {
        (true, false) => Some(CameraProjection::Orthographic),
        (false, true) => Some(CameraProjection::Perspective),
        // 两套字段都出现（或都没有）→ 不猜，让调用方报出来。
        _ => None,
    }
}

/// 迁移整份场景里所有相机组件：`Camera2D`/`Camera3D` → 统一 `Camera`。
///
/// 同一实体两个旧名并存时**以 `Camera3D` 为准**（与变换统一同一策略：显式 3D 数据优先），
/// 另一个键被丢弃；这条在测试里钉住。
pub fn migrate_scene_cameras(scene: &mut Scene) -> CameraMigrationOutcome {
    let mut outcome = CameraMigrationOutcome::default();
    for entity in scene.entities.values_mut() {
        let legacy_3d = entity
            .components
            .get(LEGACY_CAMERA_3D_COMPONENT)
            .cloned();
        let legacy_2d = entity
            .components
            .get(LEGACY_CAMERA_2D_COMPONENT)
            .cloned();

        if let Some(payload) = legacy_3d {
            match unified_camera_payload(LEGACY_CAMERA_3D_COMPONENT, &payload) {
                Ok(Some(next)) => {
                    entity.components.insert(CAMERA_COMPONENT.to_owned(), next);
                    entity.components.remove(LEGACY_CAMERA_3D_COMPONENT);
                    entity.components.remove(LEGACY_CAMERA_2D_COMPONENT);
                    outcome.changed_entities += 1;
                }
                Ok(None) => unreachable!("旧名总要改名，故不会返回 None"),
                Err(reason) => {
                    outcome.skipped.push((
                        entity.id,
                        LEGACY_CAMERA_3D_COMPONENT.to_owned(),
                        reason,
                    ));
                    continue;
                }
            }
            continue;
        }

        if let Some(payload) = legacy_2d {
            match unified_camera_payload(LEGACY_CAMERA_2D_COMPONENT, &payload) {
                Ok(Some(next)) => {
                    entity.components.insert(CAMERA_COMPONENT.to_owned(), next);
                    entity.components.remove(LEGACY_CAMERA_2D_COMPONENT);
                    outcome.changed_entities += 1;
                }
                Ok(None) => unreachable!("旧名总要改名，故不会返回 None"),
                Err(reason) => outcome.skipped.push((
                    entity.id,
                    LEGACY_CAMERA_2D_COMPONENT.to_owned(),
                    reason,
                )),
            }
            continue;
        }

        let Some(payload) = entity.components.get(CAMERA_COMPONENT).cloned() else {
            continue;
        };
        match unified_camera_payload(CAMERA_COMPONENT, &payload) {
            Ok(Some(next)) => {
                entity.components.insert(CAMERA_COMPONENT.to_owned(), next);
                outcome.changed_entities += 1;
            }
            Ok(None) => {}
            Err(reason) => outcome
                .skipped
                .push((entity.id, CAMERA_COMPONENT.to_owned(), reason)),
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene_with(components: &[(&str, &str)]) -> (Scene, SceneEntityId) {
        let mut scene = Scene::new("camera-migration");
        let id = SceneEntityId(1);
        scene.add_entity(id, "Cam", None).unwrap();
        for (name, payload) in components {
            scene
                .entities
                .get_mut(&id)
                .unwrap()
                .components
                .insert((*name).to_owned(), (*payload).to_owned());
        }
        (scene, id)
    }

    /// 2D 相机迁移：只改名 + 补判别字段，**字段值与顺序来自 2D 解析器自己的编码器**（画面不变）。
    #[test]
    fn legacy_2d_camera_becomes_orthographic_camera() {
        let payload = "center=10.0000,20.0000;rotation=0.500000;zoom=2.000000;enabled=false";
        let migrated = unified_camera_payload(LEGACY_CAMERA_2D_COMPONENT, payload)
            .expect("可解析")
            .expect("旧名必须迁移");
        assert_eq!(
            migrated,
            format!(
                "projection=orthographic;{}",
                Camera2D::from_component_str(payload).unwrap().to_component_str()
            )
        );
        // 判别字段能被取回，且去掉它之后仍是合法的 2D 串（引擎旧解析器可以直接读）。
        let (projection, rest) = split_projection(&migrated);
        assert_eq!(projection, Some(CameraProjection::Orthographic));
        let parsed = Camera2D::from_component_str(&rest).expect("去掉判别字段后仍是合法 2D 串");
        assert_eq!(parsed.center, crate::core::transform::Vec2::new(10.0, 20.0));
        assert_eq!(parsed.zoom, 2.0);
        assert!(!parsed.enabled);
    }

    /// 3D 相机迁移：`fov` 仍是**角度**（组件串的既有语义），只是改名 + 补判别字段。
    #[test]
    fn legacy_3d_camera_becomes_perspective_camera() {
        let payload =
            "position=0.000000000,1.000000000,5.000000000;target=0.000000000,0.000000000,0.000000000;fov=60.0000;near=0.100000000;far=100.000000000;pose=transform";
        let migrated = unified_camera_payload(LEGACY_CAMERA_3D_COMPONENT, payload)
            .expect("可解析")
            .expect("旧名必须迁移");
        assert!(migrated.starts_with("projection=perspective;"), "{migrated}");
        let (projection, rest) = split_projection(&migrated);
        assert_eq!(projection, Some(CameraProjection::Perspective));
        let parsed = Camera3D::from_component_str(&rest).expect("去掉判别字段后仍是合法 3D 串");
        assert!((parsed.fov_y.to_degrees() - 60.0).abs() < 1e-3);
        assert!(parsed.pose_from_transform);
    }

    /// 幂等 + 场景级改名；第二遍必须 0 改动（否则文档会永远 dirty）。
    #[test]
    fn camera_migration_is_idempotent_and_renames_in_scene() {
        let payload = "center=0.0000,0.0000;zoom=1.000000";
        let (mut scene, id) = scene_with(&[(LEGACY_CAMERA_2D_COMPONENT, payload)]);
        let outcome = migrate_scene_cameras(&mut scene);
        assert_eq!(outcome.changed_entities, 1);
        assert!(outcome.skipped.is_empty());
        let components = &scene.entities[&id].components;
        assert!(!components.contains_key(LEGACY_CAMERA_2D_COMPONENT));
        let canonical = components[CAMERA_COMPONENT].clone();
        assert!(canonical.starts_with("projection=orthographic;"), "{canonical}");
        assert_eq!(unified_camera_payload(CAMERA_COMPONENT, &canonical), Ok(None));

        let outcome = migrate_scene_cameras(&mut scene);
        assert_eq!(outcome.changed_entities, 0);
        assert!(!outcome.changed());
    }

    /// 两个旧名并存：以 `Camera3D` 为准（与变换统一同一策略），另一个键被丢弃。
    #[test]
    fn explicit_3d_camera_wins_when_both_legacy_names_are_present() {
        let three_d = "position=0.000000000,0.000000000,5.000000000;target=0.000000000,0.000000000,0.000000000;fov=60.0000;near=0.100000000;far=100.000000000";
        let (mut scene, id) = scene_with(&[
            (LEGACY_CAMERA_2D_COMPONENT, "center=1.0000,1.0000;zoom=3.000000"),
            (LEGACY_CAMERA_3D_COMPONENT, three_d),
        ]);
        let outcome = migrate_scene_cameras(&mut scene);
        assert_eq!(outcome.changed_entities, 1);
        let components = &scene.entities[&id].components;
        assert!(!components.contains_key(LEGACY_CAMERA_2D_COMPONENT));
        assert!(!components.contains_key(LEGACY_CAMERA_3D_COMPONENT));
        assert!(
            components[CAMERA_COMPONENT].starts_with("projection=perspective;"),
            "{}",
            components[CAMERA_COMPONENT]
        );
    }

    /// 手写/早期数据：`Camera` 没写判别字段时按**字段集合**判定（不是猜数值）；
    /// 两套字段都出现或都没有 → 不猜，报出来并原样保留。
    #[test]
    fn missing_projection_is_inferred_from_the_field_set_or_reported() {
        let inferred_2d =
            unified_camera_payload(CAMERA_COMPONENT, "center=0.0000,0.0000;zoom=1.000000")
                .expect("按字段集合可判定")
                .expect("需要补判别字段");
        assert!(inferred_2d.starts_with("projection=orthographic;"), "{inferred_2d}");
        let inferred_3d = unified_camera_payload(
            CAMERA_COMPONENT,
            "position=0.000000000,0.000000000,5.000000000;target=0.000000000,0.000000000,0.000000000;fov=60.0000;near=0.100000000;far=100.000000000",
        )
        .expect("按字段集合可判定")
        .expect("需要补判别字段");
        assert!(inferred_3d.starts_with("projection=perspective;"), "{inferred_3d}");

        // 两套字段混在一起：不猜。
        let ambiguous = unified_camera_payload(
            CAMERA_COMPONENT,
            "center=0.0000,0.0000;zoom=1.000000;position=0,0,5;target=0,0,0;fov=60;near=0.1;far=100",
        );
        assert!(ambiguous.is_err(), "{ambiguous:?}");
        // 完全看不出投影：也不猜。
        let empty = unified_camera_payload(CAMERA_COMPONENT, "enabled=false");
        assert!(empty.is_err(), "{empty:?}");
    }

    /// **统一 Camera 的读侧**（第 52 轮）：一个取值类型 + 显式投影分支，旧名与新名都能读。
    #[test]
    fn unified_camera_reader_branches_on_projection_not_on_component_name() {
        // 旧名照读（编辑器与运行时在迁移完成前仍会遇到它们）。
        let orthographic = CameraComponent::from_component_str(
            LEGACY_CAMERA_2D_COMPONENT,
            "center=1.0000,2.0000;zoom=4.000000",
        )
        .expect("2D 可读");
        assert_eq!(orthographic.projection(), CameraProjection::Orthographic);
        assert!(orthographic.as_orthographic().is_some());
        assert!(orthographic.as_perspective().is_none());
        assert!(orthographic.enabled());

        let perspective = CameraComponent::from_component_str(
            LEGACY_CAMERA_3D_COMPONENT,
            "position=0.000000000,1.000000000,5.000000000;target=0.000000000,0.000000000,0.000000000;fov=60.0000;near=0.100000000;far=100.000000000;enabled=false",
        )
        .expect("3D 可读");
        assert_eq!(perspective.projection(), CameraProjection::Perspective);
        assert!(!perspective.enabled());
        assert!(perspective.as_perspective().is_some());

        // 统一写法读回来是同一个分支与同一份取值（读→写→读稳定）。
        let canonical = perspective.to_component_str();
        assert!(canonical.starts_with("projection=perspective;"), "{canonical}");
        assert_eq!(
            CameraComponent::from_component_str(CAMERA_COMPONENT, &canonical).expect("统一写法可读"),
            perspective
        );
        let canonical_2d = orthographic.to_component_str();
        assert_eq!(
            CameraComponent::from_component_str(CAMERA_COMPONENT, &canonical_2d)
                .expect("统一写法可读"),
            orthographic
        );
        // 判别字段与字段体不匹配（写了 orthographic 却给了 3D 字段）→ 报错，不猜。
        assert!(
            CameraComponent::from_component_str(
                CAMERA_COMPONENT,
                "projection=orthographic;position=0,0,5;target=0,0,0;fov=60;near=0.1;far=100"
            )
            .is_err()
        );
        // 不是相机组件 → 明确报错。
        assert!(CameraComponent::from_component_str("Sprite", "size=1x1").is_err());
    }

    #[test]
    fn broken_camera_payloads_are_left_untouched_and_reported() {
        let (mut scene, id) = scene_with(&[
            (LEGACY_CAMERA_2D_COMPONENT, "zoom=oops"),
            ("Sprite", "texture=;size=1x1;tint=1,1,1,1;sorting_layer=default"),
        ]);
        let outcome = migrate_scene_cameras(&mut scene);
        assert_eq!(outcome.changed_entities, 0);
        assert_eq!(outcome.skipped.len(), 1);
        assert_eq!(outcome.skipped[0].0, id);
        assert_eq!(outcome.skipped[0].1, LEGACY_CAMERA_2D_COMPONENT);
        let components = &scene.entities[&id].components;
        assert_eq!(components[LEGACY_CAMERA_2D_COMPONENT], "zoom=oops");
        assert!(!components.contains_key(CAMERA_COMPONENT));
        assert!(components["Sprite"].contains("size=1x1"));
        assert_eq!(
            unified_camera_payload("Sprite", "texture=;size=1x1"),
            Ok(None)
        );
    }
}
