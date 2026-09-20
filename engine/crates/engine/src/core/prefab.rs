//! Prefab assets and repeatable scene instantiation with explicit overrides.

use crate::core::{
    resource_db::AssetId,
    scene::{Scene, SceneEntity, SceneEntityId, SceneError},
};
use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
    str::FromStr,
};

pub const CURRENT_PREFAB_VERSION: u32 = 1;

/// 实体元数据键：该实体是哪个 Prefab 源的实例（编辑器与运行时共用这个约定）。
pub const PREFAB_SOURCE_META_KEY: &str = "prefab.source";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Prefab {
    pub version: u32,
    pub asset_id: AssetId,
    pub name: String,
    pub root: SceneEntityId,
    pub entities: BTreeMap<SceneEntityId, SceneEntity>,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrefabInstance {
    pub prefab_id: AssetId,
    pub instance_root: SceneEntityId,
    pub source_to_instance: BTreeMap<SceneEntityId, SceneEntityId>,
    pub overrides: BTreeMap<(SceneEntityId, String), String>,
}

impl Prefab {
    /// Creates a prefab from one scene entity and all descendants.
    pub fn from_scene(
        scene: &Scene,
        root: SceneEntityId,
        asset_id: AssetId,
        name: impl Into<String>,
    ) -> Result<Self, PrefabError> {
        let root_entity = scene
            .entities
            .get(&root)
            .ok_or(PrefabError::MissingEntity(root))?;
        let mut ids = Vec::new();
        collect_ids(scene, root, &mut ids)?;
        let mut entities = BTreeMap::new();
        for id in ids {
            let mut entity = scene
                .entities
                .get(&id)
                .expect("collected entity exists")
                .clone();
            if id == root {
                entity.parent = None;
            }

            entities.insert(id, entity);
        }
        // Rebuild children after collection so ordering is independent of insertion order.
        let entity_ids: std::collections::BTreeSet<_> = entities.keys().copied().collect();
        for entity in entities.values_mut() {
            entity.children.retain(|id| entity_ids.contains(id));
        }
        let _ = root_entity;
        Ok(Self {
            version: CURRENT_PREFAB_VERSION,
            asset_id,
            name: name.into(),
            root,
            entities,
            metadata: BTreeMap::new(),
        })
    }

    /// Instantiates a fresh copy and applies all saved instance overrides.
    pub fn instantiate(
        &self,
        scene: &mut Scene,
        parent: Option<SceneEntityId>,
        overrides: &BTreeMap<(SceneEntityId, String), String>,
    ) -> Result<PrefabInstance, PrefabError> {
        self.validate()?;
        if let Some(parent_id) = parent
            && !scene.entities.contains_key(&parent_id)
        {
            return Err(PrefabError::Scene(SceneError::MissingParent(parent_id)));
        }
        let mut map = BTreeMap::new();
        let mut next = scene
            .entities
            .keys()
            .map(|id| id.0)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        for source_id in self.entities.keys() {
            map.insert(*source_id, SceneEntityId(next));
            next = next.checked_add(1).ok_or(PrefabError::IdExhausted)?;
        }
        for (source_id, source_entity) in &self.entities {
            let instance_id = map[source_id];
            let mapped_parent = if *source_id == self.root {
                parent
            } else {
                source_entity.parent.map(|id| map[&id])
            };
            let mut entity = source_entity.clone();
            entity.id = instance_id;
            entity.parent = mapped_parent;
            entity.children = source_entity.children.iter().map(|id| map[id]).collect();
            scene.entities.insert(instance_id, entity);
        }
        if let Some(parent_id) = parent {
            scene
                .entities
                .get_mut(&parent_id)
                .expect("validated parent")
                .children
                .push(map[&self.root]);
        }
        let instance = PrefabInstance {
            prefab_id: self.asset_id,
            instance_root: map[&self.root],
            source_to_instance: map,
            overrides: overrides.clone(),
        };
        instance.apply_overrides(scene)?;
        Ok(instance)
    }

    /// 这份 Prefab **内部**引用的其它 Prefab 源（`prefab.source` 元数据的去重值）。
    ///
    /// 非空就表示它是一份**嵌套 Prefab**：根节点自己带 `prefab.source`（本 Prefab 的根是另一个
    /// Prefab 的实例），或者树里某个后代是别的 Prefab 的实例。
    /// 编辑器目前**不支持嵌套**，所以这条信息是"该拒绝什么、该警告什么"的事实源
    /// （见 `editor::prefab_graph` 的扫描器）。
    #[must_use]
    pub fn nested_sources(&self) -> Vec<String> {
        let mut sources: Vec<String> = self
            .entities
            .values()
            .filter_map(|entity| entity.metadata.get(PREFAB_SOURCE_META_KEY))
            .filter(|source| !source.trim().is_empty())
            .cloned()
            .collect();
        sources.sort();
        sources.dedup();
        sources
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), PrefabError> {
        self.validate()?;
        let path = path.as_ref();
        let temp = path.with_extension("tmp");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&temp, self.to_text())?;
        fs::rename(temp, path)?;
        Ok(())
    }
    pub fn load(path: impl AsRef<Path>) -> Result<Self, PrefabError> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|e| PrefabError::Io {
            path: path.display().to_string(),
            reason: e.to_string(),
        })?;
        Self::from_text(&text).map_err(|e| e.with_path(path))
    }
    pub fn to_text(&self) -> String {
        let mut out = format!(
            "prefab_version={}\nasset_id={}\nname={}\nroot={}\n",
            self.version,
            self.asset_id,
            escape(&self.name),
            self.root.0
        );
        for (k, v) in &self.metadata {
            out.push_str(&format!("meta\t{}\t{}\n", escape(k), escape(v)));
        }
        for e in self.entities.values() {
            out.push_str(&format!(
                "entity\t{}\t{}\t{}\n",
                e.id.0,
                escape(&e.name),
                e.parent.map(|p| p.0.to_string()).unwrap_or_default()
            ));
            for (k, v) in &e.components {
                out.push_str(&format!(
                    "component\t{}\t{}\t{}\n",
                    e.id.0,
                    escape(k),
                    escape(v)
                ));
            }
            for (k, v) in &e.resources {
                out.push_str(&format!("resource\t{}\t{}\t{}\n", e.id.0, escape(k), v));
            }
            for (k, v) in &e.metadata {
                out.push_str(&format!(
                    "entity_meta\t{}\t{}\t{}\n",
                    e.id.0,
                    escape(k),
                    escape(v)
                ));
            }
        }
        out
    }
    pub fn from_text(text: &str) -> Result<Self, PrefabError> {
        let mut version = None;
        let mut asset = None;
        let mut name = None;
        let mut root = None;
        let mut metadata = BTreeMap::new();
        let mut entities = BTreeMap::new();
        for (n, line) in text.lines().enumerate() {
            let line_no = n + 1;
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(v) = line.strip_prefix("prefab_version=") {
                version = Some(v.parse().map_err(|_| PrefabError::Parse {
                    line: line_no,
                    reason: "invalid prefab_version".into(),
                    path: None,
                })?);
                continue;
            }
            if let Some(v) = line.strip_prefix("asset_id=") {
                asset = Some(AssetId::from_str(v).map_err(|_| PrefabError::Parse {
                    line: line_no,
                    reason: "invalid asset_id".into(),
                    path: None,
                })?);
                continue;
            }
            if let Some(v) = line.strip_prefix("name=") {
                name = Some(unescape(v, line_no)?);
                continue;
            }
            if let Some(v) = line.strip_prefix("root=") {
                root = Some(SceneEntityId(number(v, line_no)?));
                continue;
            }
            let f: Vec<_> = line.split('\t').collect();
            match f.first().copied() {
                Some("meta") => {
                    need(&f, 3, line_no)?;
                    metadata.insert(unescape(f[1], line_no)?, unescape(f[2], line_no)?);
                }
                Some("entity") => {
                    need(&f, 4, line_no)?;
                    let id = SceneEntityId(number(f[1], line_no)?);
                    entities.insert(
                        id,
                        SceneEntity {
                            id,
                            name: unescape(f[2], line_no)?,
                            parent: if f[3].is_empty() {
                                None
                            } else {
                                Some(SceneEntityId(number(f[3], line_no)?))
                            },
                            children: Vec::new(),
                            components: BTreeMap::new(),
                            resources: BTreeMap::new(),
                            metadata: BTreeMap::new(),
                        },
                    );
                }
                Some("component") | Some("resource") | Some("entity_meta") => {
                    need(&f, 4, line_no)?;
                    let id = SceneEntityId(number(f[1], line_no)?);
                    let e = entities.get_mut(&id).ok_or(PrefabError::Parse {
                        line: line_no,
                        reason: "record references unknown entity".into(),
                        path: None,
                    })?;
                    let key = unescape(f[2], line_no)?;
                    match f[0] {
                        "component" => {
                            e.components.insert(key, unescape(f[3], line_no)?);
                        }
                        "resource" => {
                            e.resources.insert(
                                key,
                                AssetId::from_str(f[3]).map_err(|_| PrefabError::Parse {
                                    line: line_no,
                                    reason: "invalid resource UUID".into(),
                                    path: None,
                                })?,
                            );
                        }
                        _ => {
                            e.metadata.insert(key, unescape(f[3], line_no)?);
                        }
                    }
                }
                _ => {
                    return Err(PrefabError::Parse {
                        line: line_no,
                        reason: "unknown record".into(),
                        path: None,
                    });
                }
            }
        }
        let prefab = Self {
            version: version.ok_or(PrefabError::MissingField("prefab_version".into()))?,
            asset_id: asset.ok_or(PrefabError::MissingField("asset_id".into()))?,
            name: name.ok_or(PrefabError::MissingField("name".into()))?,
            root: root.ok_or(PrefabError::MissingField("root".into()))?,
            entities,
            metadata,
        };
        let mut prefab = prefab;
        for e in prefab.entities.values_mut() {
            e.children.clear();
        }
        let ids: Vec<_> = prefab.entities.keys().copied().collect();
        for id in ids {
            if let Some(parent) = prefab.entities[&id].parent {
                prefab
                    .entities
                    .get_mut(&parent)
                    .ok_or(PrefabError::MissingEntity(parent))?
                    .children
                    .push(id);
            }
        }
        prefab.validate()?;
        Ok(prefab)
    }
    pub fn validate(&self) -> Result<(), PrefabError> {
        if self.version != CURRENT_PREFAB_VERSION {
            return Err(PrefabError::UnsupportedVersion(self.version));
        }
        if !self.entities.contains_key(&self.root) {
            return Err(PrefabError::MissingEntity(self.root));
        }
        for (id, e) in &self.entities {
            if let Some(parent) = e.parent {
                let p = self
                    .entities
                    .get(&parent)
                    .ok_or(PrefabError::MissingEntity(parent))?;
                if !p.children.contains(id) {
                    return Err(PrefabError::TreeMismatch(*id));
                }
            }
        }
        Ok(())
    }
}

impl PrefabInstance {
    pub fn set_override(
        &mut self,
        source_entity: SceneEntityId,
        field: impl Into<String>,
        value: impl Into<String>,
    ) {
        self.overrides
            .insert((source_entity, field.into()), value.into());
    }
    pub fn apply_overrides(&self, scene: &mut Scene) -> Result<(), PrefabError> {
        for ((source, field), value) in &self.overrides {
            let id = *self
                .source_to_instance
                .get(source)
                .ok_or(PrefabError::MissingEntity(*source))?;
            let e = scene
                .entities
                .get_mut(&id)
                .ok_or(PrefabError::MissingEntity(id))?;
            if field == "name" {
                e.name = value.clone();
            } else if let Some(key) = field.strip_prefix("component:") {
                e.components.insert(key.to_owned(), value.clone());
            } else if let Some(path) = field.strip_prefix("property:") {
                // docs/55 阶段 3：逐字段 Override。只重写目标字段，其余字段保持逐字不变，
                // 因此"改了一个字段"不会把整个组件串换掉（也不会把别人的覆盖一起吞掉）。
                let path = crate::core::property_path::PropertyPath::parse(path)
                    .map_err(|error| PrefabError::InvalidOverride(format!("{field}: {error}")))?;
                let Some(current) = e.components.get(&path.component).cloned() else {
                    return Err(PrefabError::InvalidOverride(format!(
                        "{field}: 实例缺少组件 {}",
                        path.component
                    )));
                };
                let updated = crate::core::property_path::write_builtin(&current, &path, value)
                    .map_err(|error| PrefabError::InvalidOverride(format!("{field}: {error}")))?;
                e.components.insert(path.component, updated);
            } else if let Some(key) = field.strip_prefix("metadata:") {
                e.metadata.insert(key.to_owned(), value.clone());
            } else {
                return Err(PrefabError::InvalidOverride(field.clone()));
            }
        }
        Ok(())
    }
    /// Rebuild policy: callers create a new copy from the source and reapply these overrides.
    pub fn rebuild(
        &self,
        prefab: &Prefab,
        scene: &mut Scene,
        parent: Option<SceneEntityId>,
    ) -> Result<Self, PrefabError> {
        prefab.instantiate(scene, parent, &self.overrides)
    }
}

fn collect_ids(
    scene: &Scene,
    id: SceneEntityId,
    out: &mut Vec<SceneEntityId>,
) -> Result<(), PrefabError> {
    let e = scene
        .entities
        .get(&id)
        .ok_or(PrefabError::MissingEntity(id))?;
    out.push(id);
    for child in &e.children {
        collect_ids(scene, *child, out)?;
    }
    Ok(())
}
fn need(f: &[&str], n: usize, line: usize) -> Result<(), PrefabError> {
    if f.len() != n {
        Err(PrefabError::Parse {
            line,
            reason: format!("expected {n} fields, got {}", f.len()),
            path: None,
        })
    } else {
        Ok(())
    }
}
fn number(v: &str, line: usize) -> Result<u64, PrefabError> {
    v.parse().map_err(|_| PrefabError::Parse {
        line,
        reason: "expected integer".into(),
        path: None,
    })
}
fn escape(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}
fn unescape(v: &str, line: usize) -> Result<String, PrefabError> {
    let mut o = String::new();
    let mut c = v.chars();
    while let Some(x) = c.next() {
        if x != '\\' {
            o.push(x);
        } else {
            match c.next() {
                Some('n') => o.push('\n'),
                Some('t') => o.push('\t'),
                Some('\\') => o.push('\\'),
                _ => {
                    return Err(PrefabError::Parse {
                        line,
                        reason: "invalid escape".into(),
                        path: None,
                    });
                }
            }
        }
    }
    Ok(o)
}

#[derive(Debug)]
pub enum PrefabError {
    Io {
        path: String,
        reason: String,
    },
    Parse {
        line: usize,
        reason: String,
        path: Option<PathBuf>,
    },
    MissingField(String),
    UnsupportedVersion(u32),
    MissingEntity(SceneEntityId),
    TreeMismatch(SceneEntityId),
    Scene(SceneError),
    InvalidOverride(String),
    IdExhausted,
}
impl PrefabError {
    fn with_path(mut self, path: &Path) -> Self {
        if let Self::Parse { path: slot, .. } = &mut self {
            *slot = Some(path.to_path_buf());
        }
        self
    }
}
impl fmt::Display for PrefabError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, reason } => write!(f, "prefab I/O failed for {path}: {reason}"),
            Self::Parse { line, reason, path } => write!(
                f,
                "prefab parse error{} at line {line}: {reason}",
                path.as_ref()
                    .map(|p| format!(" in {}", p.display()))
                    .unwrap_or_default()
            ),
            Self::MissingField(x) => write!(f, "missing prefab field: {x}"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported prefab version: {v}"),
            Self::MissingEntity(id) => write!(f, "missing prefab entity: {}", id.0),
            Self::TreeMismatch(id) => write!(f, "prefab tree mismatch: {}", id.0),
            Self::Scene(e) => e.fmt(f),
            Self::InvalidOverride(x) => write!(f, "invalid prefab override: {x}"),
            Self::IdExhausted => write!(f, "scene entity ID space exhausted"),
        }
    }
}
impl Error for PrefabError {}
impl From<std::io::Error> for PrefabError {
    fn from(e: std::io::Error) -> Self {
        Self::Io {
            path: "<prefab>".into(),
            reason: e.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scene() -> Scene {
        let mut s = Scene::new("source");
        s.add_entity(SceneEntityId(1), "Root", None).unwrap();
        s.add_entity(SceneEntityId(2), "Child", Some(SceneEntityId(1)))
            .unwrap();
        s.entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert("Transform".into(), "x=1".into());
        s
    }
    #[test]
    fn creates_instantiates_and_round_trips() {
        let s = scene();
        let id = AssetId::from_seed("player.prefab");
        let p = Prefab::from_scene(&s, SceneEntityId(1), id, "Player").unwrap();
        let mut target = Scene::new("target");
        let mut instance = p.instantiate(&mut target, None, &BTreeMap::new()).unwrap();
        instance.set_override(SceneEntityId(2), "component:Transform", "x=9");
        instance.apply_overrides(&mut target).unwrap();
        assert_eq!(target.entities.len(), 2);
        assert_eq!(
            target.entities[&instance.source_to_instance[&SceneEntityId(2)]].components["Transform"],
            "x=9"
        );
        assert_eq!(Prefab::from_text(&p.to_text()).unwrap(), p);
    }
    #[test]
    fn property_overrides_rewrite_one_field_only() {
        // 逐字段 Override：只改 Sprite.tint[1]，其余字段（含 size 的 `x` 形式与其它分量）
        // 必须逐字保留，且写回同一个值时不产生任何变化。
        let mut s = Scene::new("source");
        s.add_entity(SceneEntityId(1), "Root", None).unwrap();
        s.entities.get_mut(&SceneEntityId(1)).unwrap().components.insert(
            "Sprite".into(),
            "texture=a.png;size=16x16;pivot=0.500,0.500;tint=1.000,1.000,1.000,1.000;sorting_layer=world;order=3"
                .into(),
        );
        let p = Prefab::from_scene(&s, SceneEntityId(1), AssetId::from_seed("p"), "P").unwrap();
        let mut target = Scene::new("t");
        let mut instance = p.instantiate(&mut target, None, &BTreeMap::new()).unwrap();

        instance.set_override(SceneEntityId(1), "property:Sprite.tint[1]", "0.25");
        instance.apply_overrides(&mut target).unwrap();
        let payload =
            target.entities[&instance.source_to_instance[&SceneEntityId(1)]].components["Sprite"]
                .clone();
        assert_eq!(
            payload,
            "texture=a.png;size=16x16;pivot=0.500,0.500;tint=1.000,0.25,1.000,1.000;sorting_layer=world;order=3"
        );

        // 再应用同一个覆盖：结果完全一致（幂等，无格式抖动）。
        instance.apply_overrides(&mut target).unwrap();
        let again =
            target.entities[&instance.source_to_instance[&SceneEntityId(1)]].components["Sprite"]
                .clone();
        assert_eq!(payload, again);

        // 非法字段值不会被写进场景。
        instance.set_override(SceneEntityId(1), "property:Sprite.order", "not-a-number");
        assert!(instance.apply_overrides(&mut target).is_err());
        // 未注册组件没有 schema：明确报错而不是静默忽略。
        instance.set_override(SceneEntityId(1), "property:Nope.x", "1");
        assert!(instance.apply_overrides(&mut target).is_err());
    }

    #[test]
    fn multiple_instances_get_unique_ids_and_rebuild_policy_preserves_override() {
        let s = scene();
        let p = Prefab::from_scene(&s, SceneEntityId(1), AssetId::from_seed("p"), "P").unwrap();
        let mut target = Scene::new("t");
        let a = p.instantiate(&mut target, None, &BTreeMap::new()).unwrap();
        let mut overrides = BTreeMap::new();
        overrides.insert((SceneEntityId(1), "name".into()), "Boss".into());
        let b = p.instantiate(&mut target, None, &overrides).unwrap();
        assert_ne!(a.instance_root, b.instance_root);
        assert_eq!(target.entities[&b.instance_root].name, "Boss");
        assert_eq!(b.overrides.len(), 1);
    }
    #[test]
    fn rejects_bad_version() {
        let text =
            "prefab_version=2\nasset_id=00000000-0000-4000-8000-000000000000\nname=x\nroot=1\n";
        assert!(matches!(
            Prefab::from_text(text),
            Err(PrefabError::UnsupportedVersion(2))
        ));
    }
}
