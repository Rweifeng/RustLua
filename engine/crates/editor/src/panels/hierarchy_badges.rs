//! 层级树的预制体徽标（只读）：嵌套实例根 + 覆盖条数。
//!
//! 两种文档、两种事实源，刻意不互相抄：
//!
//! * **预制体文档**：嵌套节点由 `prefab.source` 认出（实例根是 `prefab.root=true` 那些）；
//!   覆盖条数直接数文件级记录 `prefab.override.*`（变体↔基础）与
//!   `prefab.nested_override.*`（外层↔内层）——零磁盘 I/O，解析不了编号的记录直接跳过；
//! * **场景**：实例根同样靠 `prefab.root=true` 认出；节点级覆盖是**与源预制体的 diff**，
//!   走 Inspector 同一个 [`collect_instance_overrides`]，所以层级上的数字与 Inspector
//!   那段列表不会各说各话。嵌套实例（实例里再挂实例）归它自己的内层源负责，
//!   外层实例的 diff 遇到它就停（`ForeignPrefabNode`），不会重复计数。
//!
//! 徽标只是装饰：源读不出来、记录写坏都只是**不显示**，不弹错、不改文档。
//! 索引按 (文档路径, 修订, 实体数, 源重载代际) 重建；拖动预览期间修订不变，
//! 于是连续拖动不会每帧重算，正常编辑一次一算（预制体文档那条路径根本不碰磁盘）。

use crate::EditorApp;
use crate::util::strip_verbatim_prefix;
use eframe::egui;
use rustlua_scene::prefab::{
    PREFAB_NESTED_OVERRIDE_META_PREFIX, PREFAB_OVERRIDE_META_PREFIX, PREFAB_SOURCE_META_KEY,
};
use rustlua_scene::scene::{Scene, SceneEntityId};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

const NESTED_BADGE_COLOR: egui::Color32 = egui::Color32::from_rgb(120, 180, 240);
const OVERRIDE_BADGE_COLOR: egui::Color32 = egui::Color32::from_rgb(240, 170, 90);

/// 一条覆盖记录属于哪条谱系（决定悬停说明，不参与显示文本）。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum BadgeOrigin {
    /// 变体相对基础：`prefab.override.*`。
    VariantBase,
    /// 外层相对内层：`prefab.nested_override.*`。
    NestedInner,
    /// 场景实例相对源预制体（带源相对路径）。
    InstanceSource(String),
}

/// 一行要显示的徽标。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct EntityBadges {
    /// 嵌套实例根的内层来源（相对路径）。
    pub(crate) nested: Option<String>,
    /// 各谱系的覆盖字段条数。
    pub(crate) overrides: BTreeMap<BadgeOrigin, usize>,
}

impl EntityBadges {
    pub(crate) fn has_badges(&self) -> bool {
        self.nested.is_some() || !self.overrides.is_empty()
    }

    pub(crate) fn override_total(&self) -> usize {
        self.overrides.values().sum()
    }

    /// 悬停说明：每条谱系一句，数字与上面的总数同源。
    pub(crate) fn tooltip(&self) -> String {
        let mut lines = Vec::new();
        if let Some(source) = &self.nested {
            lines.push(format!("嵌套预制体实例：来源 {source}"));
        }
        for (origin, count) in &self.overrides {
            lines.push(match origin {
                BadgeOrigin::VariantBase => {
                    format!("变体相对基础记录 {count} 处字段覆盖（prefab.override.*）")
                }
                BadgeOrigin::NestedInner => {
                    format!("外层相对内层记录 {count} 处字段覆盖（prefab.nested_override.*）")
                }
                BadgeOrigin::InstanceSource(source) => {
                    format!("相对源预制体 {source} 的 {count} 处字段覆盖")
                }
            });
        }
        lines.join("\n")
    }
}

/// 索引的有效性键：文档身份 + 内容修订 + 实体数 + 源预制体重载代际。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BadgeKey {
    path: Option<PathBuf>,
    revision: u64,
    entities: usize,
    generation: u64,
}

/// 层级面板的徽标索引（按文档状态缓存）。
#[derive(Default)]
pub(crate) struct BadgeIndex {
    key: Option<BadgeKey>,
    by_entity: BTreeMap<SceneEntityId, EntityBadges>,
    #[cfg(test)]
    builds: u64,
}

impl BadgeIndex {
    pub(crate) fn get(&self, id: SceneEntityId) -> Option<&EntityBadges> {
        self.by_entity.get(&id)
    }

    pub(crate) fn clear(&mut self) {
        self.key = None;
        self.by_entity.clear();
    }

    /// 重建次数（缓存门的读数）。
    #[cfg(test)]
    pub(crate) fn builds(&self) -> u64 {
        self.builds
    }

    fn matches(&self, key: &BadgeKey) -> bool {
        self.key.as_ref() == Some(key)
    }

    fn replace(&mut self, key: BadgeKey, by_entity: BTreeMap<SceneEntityId, EntityBadges>) {
        self.key = Some(key);
        self.by_entity = by_entity;
        #[cfg(test)]
        {
            self.builds += 1;
        }
    }
}

/// 只从文档本身能看出来的徽标：嵌套根 + 文件级覆盖记录（不碰磁盘）。
///
/// `is_prefab_document` 决定嵌套的判据：预制体文件里**任何**实例根都是"嵌套"
/// （这个文件本身不是场景）；场景里只有**实例里面再挂实例**的那层才算，顶层实例不标。
pub(crate) fn recorded_badges(
    scene: &Scene,
    is_prefab_document: bool,
) -> BTreeMap<SceneEntityId, EntityBadges> {
    let mut out: BTreeMap<SceneEntityId, EntityBadges> = BTreeMap::new();
    for (id, source) in nested_roots(scene, is_prefab_document) {
        out.entry(id).or_default().nested = Some(source);
    }
    for (id, origin) in recorded_overrides(scene) {
        *out.entry(id)
            .or_default()
            .overrides
            .entry(origin)
            .or_default() += 1;
    }
    out.retain(|_, badges| badges.has_badges());
    out
}

/// 嵌套实例根：`prefab.root=true` 且（预制体文档，或已在另一个实例根里面）。
///
/// 不能拿"祖先带 `prefab.source`"当判据：实例里的**每个**节点都带 `prefab.source`，
/// 那会把整棵实例都标成嵌套。`prefab.root` 才是"这一支的根"的标记。
fn nested_roots(scene: &Scene, is_prefab_document: bool) -> BTreeMap<SceneEntityId, String> {
    let mut out = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut stack: Vec<(SceneEntityId, bool)> = scene
        .entities
        .values()
        .filter(|entity| entity.parent.is_none())
        .map(|entity| (entity.id, false))
        .collect();
    while let Some((id, inside_instance)) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(entity) = scene.entities.get(&id) else {
            continue;
        };
        let source = entity
            .metadata
            .get(PREFAB_SOURCE_META_KEY)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty());
        let is_instance_root = entity
            .metadata
            .get("prefab.root")
            .is_some_and(|value| value == "true");
        if is_instance_root
            && let Some(source) = source
            && (is_prefab_document || inside_instance)
        {
            out.insert(id, source.to_owned());
        }
        let inside_next = inside_instance || is_instance_root;
        for child in &entity.children {
            stack.push((*child, inside_next));
        }
    }
    out
}

/// 文件级覆盖记录 → (实体编号, 谱系)，一条记录算一项。
fn recorded_overrides(scene: &Scene) -> Vec<(SceneEntityId, BadgeOrigin)> {
    let mut out = Vec::new();
    for key in scene.metadata.keys() {
        if let Some(rest) = key.strip_prefix(PREFAB_OVERRIDE_META_PREFIX) {
            if let Some(id) = record_target(rest) {
                out.push((id, BadgeOrigin::VariantBase));
            }
        } else if let Some(rest) = key.strip_prefix(PREFAB_NESTED_OVERRIDE_META_PREFIX)
            && let Some(id) = record_target(rest)
        {
            out.push((id, BadgeOrigin::NestedInner));
        }
    }
    out
}

/// `"<实体编号>.<字段>"` → 实体编号；写坏的键返回 `None`（只读视图不为它弹错）。
fn record_target(rest: &str) -> Option<SceneEntityId> {
    let (id, field) = rest.split_once('.')?;
    if field.is_empty() {
        return None;
    }
    id.parse::<u64>().ok().map(SceneEntityId)
}

/// 场景里带来源的实例根（`prefab.root=true`）；有序，便于稳定遍历与测试。
fn instance_roots(scene: &Scene) -> Vec<(SceneEntityId, String)> {
    scene
        .entities
        .values()
        .filter(|entity| {
            entity
                .metadata
                .get("prefab.root")
                .is_some_and(|value| value == "true")
        })
        .filter_map(|entity| {
            entity
                .metadata
                .get(PREFAB_SOURCE_META_KEY)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .map(|value| (entity.id, value.to_owned()))
        })
        .collect()
}

impl EditorApp {
    /// 按当前文档重建（必要时）徽标索引。层级面板每帧在绘制行之前调用。
    ///
    /// 预制体文档只读内存里的记录；场景才去 stat/diff 源预制体（与 Inspector 共用
    /// `prefab_source_cache`，同一份源只读一次）。
    pub(crate) fn ensure_prefab_badges(&mut self) {
        let Some(document) = self.document.current_scene.as_ref() else {
            self.prefab_badges.clear();
            return;
        };
        let is_prefab_document = document.is_prefab_document();
        let roots = if is_prefab_document {
            // 预制体文档的覆盖是文件级记录，不需要读任何源。
            Vec::new()
        } else {
            instance_roots(&document.scene)
        };
        let key = BadgeKey {
            path: document.path.clone(),
            revision: document.revision,
            entities: document.scene.entities.len(),
            generation: self.prefab_source_generation,
        };
        if self.prefab_badges.matches(&key) {
            return;
        }
        let mut by_entity = {
            let scene = &self
                .document
                .current_scene
                .as_ref()
                .expect("上面已确认文档存在")
                .scene;
            recorded_badges(scene, is_prefab_document)
        };
        // 先让源缓存就位（一次 stat；内容没变不重读），再与 Inspector 走同一个 diff。
        for (_, source) in &roots {
            let path = strip_verbatim_prefix(&self.project.root).join(source);
            let _ = self.ensure_prefab_source_cached(&path);
        }
        if let Some(document) = self.document.current_scene.as_ref() {
            for (root, source) in &roots {
                let path = strip_verbatim_prefix(&self.project.root).join(source);
                let Some(prefab) = self
                    .prefab_source_cache
                    .get(&path)
                    .map(|cached| &cached.prefab)
                else {
                    continue;
                };
                let Ok(found) = crate::actions::prefab_overrides::collect_instance_overrides(
                    &document.scene,
                    *root,
                    source,
                    prefab,
                ) else {
                    continue;
                };
                for row in found.rows {
                    *by_entity
                        .entry(row.entity)
                        .or_default()
                        .overrides
                        .entry(BadgeOrigin::InstanceSource(source.clone()))
                        .or_default() += 1;
                }
            }
        }
        // 构建过程可能重读过源（代际自增）：用构建后的代际当键，否则下一帧会再算一遍。
        let key = BadgeKey {
            generation: self.prefab_source_generation,
            ..key
        };
        self.prefab_badges.replace(key, by_entity);
    }
}

/// 在行内名字右侧画徽标（只读：点击不落选区，悬停给出来源与条数）。
pub(crate) fn draw_badges(ui: &mut egui::Ui, badges: &EntityBadges) {
    if let Some(source) = &badges.nested {
        ui.label(egui::RichText::new("[嵌套]").color(NESTED_BADGE_COLOR))
            .on_hover_text(format!("嵌套预制体实例：来源 {source}"));
    }
    let total = badges.override_total();
    if total > 0 {
        ui.label(egui::RichText::new(format!("[覆盖 {total}]")).color(OVERRIDE_BADGE_COLOR))
            .on_hover_text(badges.tooltip());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_scene::prefab::PREFAB_VARIANT_BASE_META_KEY;
    use rustlua_scene::scene::Scene;

    fn instance_meta(scene: &mut Scene, id: SceneEntityId, source: &str, root: bool) {
        let entity = scene.entities.get_mut(&id).expect("实体存在");
        entity
            .metadata
            .insert(PREFAB_SOURCE_META_KEY.to_owned(), source.to_owned());
        entity
            .metadata
            .insert("prefab.root".to_owned(), root.to_string());
    }

    /// 预制体文档：实例根一律算嵌套（这个文件本身不是场景），记录逐条计数。
    #[test]
    fn prefab_documents_badge_nested_roots_and_count_recorded_overrides() {
        let mut scene = Scene::new("outer");
        scene.add_entity(SceneEntityId(1), "Outer", None).unwrap();
        scene
            .add_entity(SceneEntityId(2), "Inner", Some(SceneEntityId(1)))
            .unwrap();
        scene
            .add_entity(SceneEntityId(3), "Plain", Some(SceneEntityId(1)))
            .unwrap();
        instance_meta(&mut scene, SceneEntityId(2), "assets/inner.prefab", true);
        scene.metadata.insert(
            PREFAB_VARIANT_BASE_META_KEY.to_owned(),
            "assets/base.prefab".to_owned(),
        );
        scene.metadata.insert(
            "prefab.override.2.property:Sprite.tint".to_owned(),
            "1,0,0,1".to_owned(),
        );
        scene
            .metadata
            .insert("prefab.override.2.name".to_owned(), "MyInner".to_owned());
        scene.metadata.insert(
            "prefab.nested_override.2.property:Sprite.order".to_owned(),
            "5".to_owned(),
        );

        let badges = recorded_badges(&scene, true);
        let inner = &badges[&SceneEntityId(2)];
        assert_eq!(inner.nested.as_deref(), Some("assets/inner.prefab"));
        assert_eq!(inner.override_total(), 3, "变体记录 2 + 内层记录 1");
        assert_eq!(
            inner.overrides[&BadgeOrigin::VariantBase],
            2,
            "两条变体记录都算在内"
        );
        assert_eq!(inner.overrides[&BadgeOrigin::NestedInner], 1);
        assert!(!badges.contains_key(&SceneEntityId(1)), "普通节点不标");
        assert!(!badges.contains_key(&SceneEntityId(3)), "没有记录就不标");
        assert!(inner.tooltip().contains("prefab.override.*"));
        assert!(inner.tooltip().contains("assets/inner.prefab"));
    }

    /// 场景：顶层实例不标；实例里再挂的实例根才标嵌套。
    ///
    /// 关键反例：实例里的普通节点也带 `prefab.source`（这是实例成员标记），
    /// 它们没有 `prefab.root`，绝不能因此被标成嵌套。
    #[test]
    fn scenes_badge_only_instances_nested_inside_other_instances() {
        let mut scene = Scene::new("level");
        scene.add_entity(SceneEntityId(1), "Root", None).unwrap();
        scene
            .add_entity(SceneEntityId(2), "OuterInstance", Some(SceneEntityId(1)))
            .unwrap();
        scene
            .add_entity(SceneEntityId(3), "Body", Some(SceneEntityId(2)))
            .unwrap();
        scene
            .add_entity(SceneEntityId(4), "NestedInstance", Some(SceneEntityId(3)))
            .unwrap();
        instance_meta(&mut scene, SceneEntityId(2), "assets/outer.prefab", true);
        instance_meta(&mut scene, SceneEntityId(3), "assets/outer.prefab", false);
        instance_meta(&mut scene, SceneEntityId(4), "assets/inner.prefab", true);

        let badges = recorded_badges(&scene, false);
        assert!(
            !badges.contains_key(&SceneEntityId(2)),
            "顶层实例不是嵌套：{:?}",
            badges.keys().collect::<Vec<_>>()
        );
        assert!(
            !badges.contains_key(&SceneEntityId(3)),
            "实例成员带 prefab.source 但不是实例根，不能标嵌套"
        );
        assert_eq!(
            badges[&SceneEntityId(4)].nested.as_deref(),
            Some("assets/inner.prefab"),
            "实例里再挂的实例根才是嵌套"
        );
    }

    /// 写坏的记录键跳过而不是整份索引失败（只读视图不为坏数据弹错）。
    #[test]
    fn malformed_records_are_skipped() {
        let mut scene = Scene::new("level");
        scene.add_entity(SceneEntityId(1), "Root", None).unwrap();
        for key in [
            "prefab.override.not-a-number.name",
            "prefab.override.1",
            "prefab.override.1.",
            "prefab.nested_override.x.name",
        ] {
            scene.metadata.insert(key.to_owned(), "v".to_owned());
        }
        let badges = recorded_badges(&scene, true);
        assert!(badges.is_empty(), "坏键不该产生徽标：{badges:?}");
    }
}
