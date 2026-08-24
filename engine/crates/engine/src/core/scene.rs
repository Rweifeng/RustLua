//! Versioned, human-readable scene format with validation and migration hooks.

use crate::core::resource_db::AssetId;
use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
    str::FromStr,
};

/// The version written by `Scene::save` and required by `Scene::validate`.
///
/// Version 2 introduced the `Transform3D` / `MeshRenderer` / `Camera3D` /
/// `Light` components. Because component payloads are opaque strings, version 1
/// scenes migrate without data transformation: the migration only raises the
/// version and records `meta migrated_from=1` for traceability.
pub const CURRENT_SCENE_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SceneEntityId(pub u64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SceneEntity {
    pub id: SceneEntityId,
    pub name: String,
    pub parent: Option<SceneEntityId>,
    pub children: Vec<SceneEntityId>,
    pub components: BTreeMap<String, String>,
    pub resources: BTreeMap<String, AssetId>,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Scene {
    pub version: u32,
    pub name: String,
    pub entities: BTreeMap<SceneEntityId, SceneEntity>,
    pub metadata: BTreeMap<String, String>,
}

impl Scene {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            version: CURRENT_SCENE_VERSION,
            name: name.into(),
            entities: BTreeMap::new(),
            metadata: BTreeMap::new(),
        }
    }
    pub fn add_entity(
        &mut self,
        id: SceneEntityId,
        name: impl Into<String>,
        parent: Option<SceneEntityId>,
    ) -> Result<(), SceneError> {
        if self.entities.contains_key(&id) {
            return Err(SceneError::DuplicateEntity(id));
        }
        if let Some(parent_id) = parent
            && !self.entities.contains_key(&parent_id)
        {
            return Err(SceneError::MissingParent(parent_id));
        }
        let entity = SceneEntity {
            id,
            name: name.into(),
            parent,
            children: Vec::new(),
            components: BTreeMap::new(),
            resources: BTreeMap::new(),
            metadata: BTreeMap::new(),
        };
        self.entities.insert(id, entity);
        if let Some(parent_id) = parent {
            self.entities.get_mut(&parent_id).unwrap().children.push(id);
        }
        Ok(())
    }
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), SceneError> {
        self.validate()?;
        let path = path.as_ref();
        let temp = path.with_extension("tmp");
        if let Some(p) = path.parent() {
            fs::create_dir_all(p)?;
        }
        fs::write(&temp, self.to_text())?;
        fs::rename(temp, path)?;
        Ok(())
    }
    pub fn load(path: impl AsRef<Path>) -> Result<Self, SceneError> {
        let path = path.as_ref();
        let bytes = fs::read(path).map_err(|e| SceneError::Io {
            path: path.display().to_string(),
            reason: e.to_string(),
        })?;
        // 支持发布包的压缩/加密场景文件；明文原样返回。
        let decoded = crate::core::pack::decode(&bytes).map_err(|e| SceneError::Io {
            path: path.display().to_string(),
            reason: e.to_string(),
        })?;
        let text = String::from_utf8_lossy(&decoded).into_owned();
        Self::from_text(&text).map_err(|e| e.with_path(path))
    }
    pub fn to_text(&self) -> String {
        let mut out = format!(
            "scene_version={}\nname={}\n",
            self.version,
            escape(&self.name)
        );
        for (k, v) in &self.metadata {
            out.push_str(&format!("meta\t{}\t{}\n", escape(k), escape(v)));
        }
        for e in self.entities.values() {
            match e.parent {
                Some(parent) => {
                    out.push_str(&format!(
                        "entity\t{}\t{}\t{}\n",
                        e.id.0,
                        escape(&e.name),
                        parent.0
                    ));
                }
                None => {
                    // 无父实体时不输出空尾字段，避免场景文件产生尾随空白。
                    out.push_str(&format!("entity\t{}\t{}\n", e.id.0, escape(&e.name)));
                }
            }
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
    pub fn from_text(text: &str) -> Result<Self, SceneError> {
        let mut version = None;
        let mut name = None;
        let mut scene = Scene::new("");
        for (n, line) in text.lines().enumerate() {
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let f: Vec<_> = line.split('\t').collect();
            if let Some(value) = line.strip_prefix("scene_version=") {
                version = Some(value.parse().map_err(|_| SceneError::Parse {
                    line: n + 1,
                    field: "scene_version".into(),
                    reason: "expected integer".into(),
                    path: None,
                })?);
                continue;
            }
            if let Some(value) = line.strip_prefix("name=") {
                name = Some(unescape(value, n + 1)?);
                continue;
            }
            if f[0] == "meta" {
                need(&f, 3, n + 1)?;
                scene
                    .metadata
                    .insert(unescape(f[1], n + 1)?, unescape(f[2], n + 1)?);
                continue;
            }
            if f[0] == "entity" {
                // 兼容带/不带尾随父实体字段两种写法（3 或 4 字段）。
                if f.len() != 3 && f.len() != 4 {
                    return Err(SceneError::Parse {
                        line: n + 1,
                        field: "entity".into(),
                        reason: "expected 3 or 4 fields".into(),
                        path: None,
                    });
                }
                let id = number(f[1], n + 1)?;
                let parent = if f.len() == 4 && !f[3].is_empty() {
                    Some(SceneEntityId(number(f[3], n + 1)?))
                } else {
                    None
                };
                scene.add_entity(SceneEntityId(id), unescape(f[2], n + 1)?, parent)?;
                continue;
            }
            if f[0] == "component" || f[0] == "resource" || f[0] == "entity_meta" {
                need(&f, 4, n + 1)?;
                let id = SceneEntityId(number(f[1], n + 1)?);
                let e = scene
                    .entities
                    .get_mut(&id)
                    .ok_or(SceneError::UnknownEntity {
                        line: n + 1,
                        id: id.0,
                    })?;
                let key = unescape(f[2], n + 1)?;
                if f[0] == "resource" {
                    e.resources.insert(
                        key,
                        AssetId::from_str(f[3]).map_err(|_| SceneError::Parse {
                            line: n + 1,
                            field: "resource".into(),
                            reason: "invalid asset UUID".into(),
                            path: None,
                        })?,
                    );
                } else {
                    let value = unescape(f[3], n + 1)?;
                    if f[0] == "component" {
                        e.components.insert(key, value);
                    } else {
                        e.metadata.insert(key, value);
                    }
                }
                continue;
            }
            return Err(SceneError::Parse {
                line: n + 1,
                field: f[0].into(),
                reason: "unknown record".into(),
                path: None,
            });
        }
        scene.version = version.ok_or(SceneError::MissingField("scene_version".into()))?;
        scene.name = name.ok_or(SceneError::MissingField("name".into()))?;
        if scene.version == 0 || scene.version > CURRENT_SCENE_VERSION {
            return Err(SceneError::UnsupportedVersion(scene.version));
        }
        if scene.version < CURRENT_SCENE_VERSION {
            scene.migrate_to_current()?;
        }
        scene.validate()?;
        Ok(scene)
    }
    /// Brings a scene written by an older format up to `CURRENT_SCENE_VERSION`.
    ///
    /// Component payloads are opaque strings, so version 1 → 2 needs no data
    /// rewrite; the migration only raises the version and records the origin
    /// for traceability. Newer migrations should transform here and must reject
    /// versions they cannot handle with [`SceneError::UnsupportedVersion`].
    fn migrate_to_current(&mut self) -> Result<(), SceneError> {
        let from = self.version;
        match from {
            1 => {
                self.version = CURRENT_SCENE_VERSION;
                self.metadata
                    .entry("migrated_from".to_owned())
                    .or_insert_with(|| from.to_string());
                Ok(())
            }
            _ => Err(SceneError::UnsupportedVersion(from)),
        }
    }

    pub fn validate(&self) -> Result<(), SceneError> {
        if self.version != CURRENT_SCENE_VERSION {
            return Err(SceneError::UnsupportedVersion(self.version));
        }
        for (eid, e) in &self.entities {
            if let Some(p) = e.parent {
                let pe = self.entities.get(&p).ok_or(SceneError::MissingParent(p))?;
                if !pe.children.contains(eid) {
                    return Err(SceneError::TreeMismatch(*eid));
                }
            }
        }
        Ok(())
    }
}
fn need(f: &[&str], n: usize, line: usize) -> Result<(), SceneError> {
    if f.len() != n {
        Err(SceneError::Parse {
            line,
            field: f[0].into(),
            reason: format!("expected {n} fields, got {}", f.len()),
            path: None,
        })
    } else {
        Ok(())
    }
}
fn number(s: &str, line: usize) -> Result<u64, SceneError> {
    s.parse().map_err(|_| SceneError::Parse {
        line,
        field: "entity_id".into(),
        reason: "expected integer".into(),
        path: None,
    })
}
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}
fn unescape(s: &str, line: usize) -> Result<String, SceneError> {
    let mut o = String::new();
    let mut c = s.chars();
    while let Some(x) = c.next() {
        if x != '\\' {
            o.push(x)
        } else {
            match c.next() {
                Some('n') => o.push('\n'),
                Some('t') => o.push('\t'),
                Some('\\') => o.push('\\'),
                _ => {
                    return Err(SceneError::Parse {
                        line,
                        field: "escaped_value".into(),
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
pub enum SceneError {
    Io {
        path: String,
        reason: String,
    },
    Parse {
        line: usize,
        field: String,
        reason: String,
        path: Option<PathBuf>,
    },
    MissingField(String),
    UnsupportedVersion(u32),
    DuplicateEntity(SceneEntityId),
    MissingParent(SceneEntityId),
    UnknownEntity {
        line: usize,
        id: u64,
    },
    TreeMismatch(SceneEntityId),
}
impl SceneError {
    fn with_path(mut self, path: &Path) -> Self {
        if let Self::Parse { path: slot, .. } = &mut self {
            *slot = Some(path.to_path_buf());
        }
        self
    }
}
impl fmt::Display for SceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, reason } => write!(f, "scene I/O failed for {path}: {reason}"),
            Self::Parse {
                line,
                field,
                reason,
                path,
            } => write!(
                f,
                "scene parse error{} at line {line}, field {field}: {reason}",
                path.as_ref()
                    .map(|p| format!(" in {}", p.display()))
                    .unwrap_or_default()
            ),
            Self::MissingField(x) => write!(f, "missing scene field: {x}"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported scene version: {v}"),
            Self::DuplicateEntity(x) => write!(f, "duplicate entity: {}", x.0),
            Self::MissingParent(x) => write!(f, "missing parent entity: {}", x.0),
            Self::UnknownEntity { line, id } => write!(f, "unknown entity {id} at line {line}"),
            Self::TreeMismatch(x) => write!(f, "entity tree mismatch: {}", x.0),
        }
    }
}
impl Error for SceneError {}
impl From<std::io::Error> for SceneError {
    fn from(e: std::io::Error) -> Self {
        Self::Io {
            path: "<scene>".into(),
            reason: e.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root(n: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rustlua-scene-{n}-{}", std::process::id()))
    }
    #[test]
    fn round_trip() {
        let mut s = Scene::new("main");
        s.add_entity(SceneEntityId(1), "Root", None).unwrap();
        s.add_entity(SceneEntityId(2), "Player", Some(SceneEntityId(1)))
            .unwrap();
        s.entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert("Transform".into(), "x=1,y=2".into());
        s.metadata.insert("author".into(), "ai".into());
        let p = root("round").join("main.scene");
        s.save(&p).unwrap();
        assert_eq!(Scene::load(&p).unwrap(), s);
        let _ = fs::remove_dir_all(root("round"));
    }
    #[test]
    fn rejects_unknown_record_with_location() {
        let e = Scene::from_text("scene_version=1\nname=x\nbad\n").unwrap_err();
        assert!(e.to_string().contains("line 3"));
    }
    #[test]
    fn rejects_zero_or_future_version() {
        assert!(matches!(
            Scene::from_text("scene_version=0\nname=x\n"),
            Err(SceneError::UnsupportedVersion(0))
        ));
        assert!(matches!(
            Scene::from_text("scene_version=3\nname=x\n"),
            Err(SceneError::UnsupportedVersion(3))
        ));
    }

    #[test]
    fn version_1_scene_migrates_to_current_and_roundtrips() {
        let text = "scene_version=1\nname=legacy\n\
                    entity\t1\tPlayer\t\n\
                    component\t1\tTransform\tx=0;y=0;rotation=0;scale_x=1;scale_y=1\n";
        let migrated = Scene::from_text(text).unwrap();
        assert_eq!(migrated.version, CURRENT_SCENE_VERSION);
        assert_eq!(migrated.name, "legacy");
        assert_eq!(
            migrated.metadata.get("migrated_from").map(String::as_str),
            Some("1")
        );
        assert_eq!(migrated.entities.len(), 1);
        // A migrated scene saves and reloads as the current version.
        let p = root("migrate").join("main.scene");
        migrated.save(&p).unwrap();
        let reloaded = Scene::load(&p).unwrap();
        assert_eq!(reloaded.version, CURRENT_SCENE_VERSION);
        assert_eq!(reloaded, migrated);
        let _ = fs::remove_dir_all(root("migrate"));
    }

    #[test]
    fn migration_only_runs_for_older_versions() {
        // Current-version scenes are untouched: no `migrated_from` marker.
        let fresh = Scene::from_text(&format!(
            "scene_version={CURRENT_SCENE_VERSION}\nname=fresh\n"
        ))
        .unwrap();
        assert!(!fresh.metadata.contains_key("migrated_from"));
    }

    #[test]
    fn version_2_scene_roundtrips_with_3d_components() {
        let text = "scene_version=2\nname=3d\n\
             entity\t1\tCamera\t\n\
             component\t1\tTransform3D\tposition=0,0,5;rotation=0,0,0,1;scale=1,1,1\n\
             component\t1\tCamera3D\tposition=0,0,5;target=0,0,0;fov=60;near=0.1;far=100\n\
             entity\t2\tMesh\t\n\
             component\t2\tMeshRenderer\tmesh=assets/models/cube.glb\n";
        let scene = Scene::from_text(text).unwrap();
        assert_eq!(scene.version, 2);
        assert_eq!(
            scene.entities[&SceneEntityId(1)].components["Transform3D"],
            "position=0,0,5;rotation=0,0,0,1;scale=1,1,1"
        );
        assert_eq!(
            scene.entities[&SceneEntityId(2)].components["MeshRenderer"],
            "mesh=assets/models/cube.glb"
        );
        let p = root("v2").join("3d.scene");
        scene.save(&p).unwrap();
        assert_eq!(Scene::load(&p).unwrap(), scene);
        let _ = fs::remove_dir_all(root("v2"));
    }
    #[test]
    fn resources_and_migration_entry_are_explicit() {
        let mut s = Scene::new("x");
        s.add_entity(SceneEntityId(1), "x", None).unwrap();
        let id = AssetId::from_seed("a");
        s.entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .resources
            .insert("texture".into(), id);
        let text = s.to_text();
        assert!(text.contains("resource\t1\ttexture\t"));
        assert_eq!(Scene::from_text(&text).unwrap(), s);
    }
}
