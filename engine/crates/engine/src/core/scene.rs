//! Versioned, human-readable scene format with strict validation.

use crate::core::resource_db::AssetId;
use std::{
    collections::BTreeMap,
    error::Error,
    fmt::{self, Write as _},
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

/// The version written by `Scene::save` and required by `Scene::validate`.
///
/// Version 2 requires separate `UiLayout` geometry and also includes the 3D
/// render components. Older and future versions are rejected.
pub const CURRENT_SCENE_VERSION: u32 = 2;

/// 稳定实体标识。
///
/// 派生 `Hash`：编辑器需要按实体 ID 建立哈希索引（如 Hierarchy 过滤匹配索引），
/// 场景内部的存储顺序仍由 `BTreeMap` 保证，与哈希无关。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
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

    /// 实体自身或任一祖先带 `editor.hidden` 标记时返回 true。
    /// 父实体隐藏会级联遮蔽整棵子树（Unity 语义）；子实体自身的
    /// 标记保持不变，父级恢复显示后各自状态仍然生效。
    pub fn is_entity_hidden_with_ancestors(&self, id: SceneEntityId) -> bool {
        let mut cursor = Some(id);
        // P3-④：复用同一个 key 缓冲，不再每个祖先 `format!` 分配一次。
        let mut key = String::with_capacity("editor.hidden.".len() + 20);
        // 环保护：正常层级深度不可能超过实体数。
        for _ in 0..=self.entities.len() {
            let Some(entity_id) = cursor else {
                return false;
            };
            let Some(entity) = self.entities.get(&entity_id) else {
                return false;
            };
            key.clear();
            key.push_str("editor.hidden.");
            let _ = write!(key, "{}", entity_id.0);
            if self.metadata.get(&key).is_some_and(|value| value == "true") {
                return true;
            }
            cursor = entity.parent;
        }
        false
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
        // P3-④：逐字段直接写入输出缓冲（转义内联），不再每行 `format!` 造临时 String。
        let mut out = String::new();
        let _ = write!(out, "scene_version={}\nname=", self.version);
        push_escaped(&mut out, &self.name);
        out.push('\n');
        for (k, v) in &self.metadata {
            out.push_str("meta\t");
            push_escaped(&mut out, k);
            out.push('\t');
            push_escaped(&mut out, v);
            out.push('\n');
        }
        for e in self.entities.values() {
            let _ = write!(out, "entity\t{}\t", e.id.0);
            push_escaped(&mut out, &e.name);
            match e.parent {
                // 无父实体时不输出空尾字段，避免场景文件产生尾随空白。
                Some(parent) => {
                    let _ = writeln!(out, "\t{}", parent.0);
                }
                None => out.push('\n'),
            }
            for (k, v) in &e.components {
                let _ = write!(out, "component\t{}\t", e.id.0);
                push_escaped(&mut out, k);
                out.push('\t');
                push_escaped(&mut out, v);
                out.push('\n');
            }
            for (k, v) in &e.resources {
                let _ = write!(out, "resource\t{}\t", e.id.0);
                push_escaped(&mut out, k);
                out.push('\t');
                let _ = write!(out, "{v}");
                out.push('\n');
            }
            for (k, v) in &e.metadata {
                let _ = write!(out, "entity_meta\t{}\t", e.id.0);
                push_escaped(&mut out, k);
                out.push('\t');
                push_escaped(&mut out, v);
                out.push('\n');
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
            let (f, field_count) = split_record(line);
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
                need(f[0], field_count, 3, n + 1)?;
                scene
                    .metadata
                    .insert(unescape(f[1], n + 1)?, unescape(f[2], n + 1)?);
                continue;
            }
            if f[0] == "entity" {
                // Parent is optional: an entity is either a root (3 fields) or
                // a child (4 fields).
                if field_count != 3 && field_count != 4 {
                    return Err(SceneError::Parse {
                        line: n + 1,
                        field: "entity".into(),
                        reason: "expected 3 or 4 fields".into(),
                        path: None,
                    });
                }
                let id = number(f[1], n + 1)?;
                let parent = if field_count == 4 && !f[3].is_empty() {
                    Some(SceneEntityId(number(f[3], n + 1)?))
                } else {
                    None
                };
                scene.add_entity(SceneEntityId(id), unescape(f[2], n + 1)?, parent)?;
                continue;
            }
            if f[0] == "component" || f[0] == "resource" || f[0] == "entity_meta" {
                need(f[0], field_count, 4, n + 1)?;
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
        scene.validate()?;
        Ok(scene)
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
fn need(kind: &str, actual: usize, expected: usize, line: usize) -> Result<(), SceneError> {
    if actual != expected {
        Err(SceneError::Parse {
            line,
            field: kind.into(),
            reason: format!("expected {expected} fields, got {actual}"),
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
/// P3-④：单次遍历把转义结果直接写进输出缓冲（原来 3 次链式 `.replace()` 会产生 3 个临时 String）。
fn push_escaped(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
}

/// P3-④：把一行拆成**至多 4 个** Tab 分隔字段 + 精确总字段数（固定数组，无每行 Vec 分配）。
fn split_record(line: &str) -> ([&str; 4], usize) {
    let mut fields = [""; 4];
    let mut it = line.split('\t');
    let mut count = 0;
    for slot in fields.iter_mut() {
        match it.next() {
            Some(value) => {
                *slot = value;
                count += 1;
            }
            None => break,
        }
    }
    count += it.count();
    (fields, count)
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
    fn rejects_unsupported_version() {
        assert!(matches!(
            Scene::from_text("scene_version=0\nname=x\n"),
            Err(SceneError::UnsupportedVersion(0))
        ));
        assert!(matches!(
            Scene::from_text("scene_version=1\nname=x\n"),
            Err(SceneError::UnsupportedVersion(1))
        ));
        assert!(matches!(
            Scene::from_text("scene_version=3\nname=x\n"),
            Err(SceneError::UnsupportedVersion(3))
        ));
    }

    #[test]
    fn current_scene_roundtrip_is_not_marked_migrated() {
        let fresh = Scene::from_text(&format!(
            "scene_version={CURRENT_SCENE_VERSION}\nname=fresh\n"
        ))
        .unwrap();
        assert!(!fresh.metadata.contains_key("migrated_from"));
    }

    #[test]
    fn hidden_flag_cascades_to_descendants() {
        let mut s = Scene::new("main");
        s.add_entity(SceneEntityId(1), "Root", None).unwrap();
        s.add_entity(SceneEntityId(2), "Parent", Some(SceneEntityId(1)))
            .unwrap();
        s.add_entity(SceneEntityId(3), "Child", Some(SceneEntityId(2)))
            .unwrap();
        let key = |id: u64| format!("editor.hidden.{id}");
        assert!(!s.is_entity_hidden_with_ancestors(SceneEntityId(3)));
        s.metadata.insert(key(2), "true".into());
        // 父级隐藏：子级级联隐藏，父级自身隐藏，祖父不受影响。
        assert!(!s.is_entity_hidden_with_ancestors(SceneEntityId(1)));
        assert!(s.is_entity_hidden_with_ancestors(SceneEntityId(2)));
        assert!(s.is_entity_hidden_with_ancestors(SceneEntityId(3)));
        s.metadata.insert(key(2), "false".into());
        s.metadata.insert(key(3), "true".into());
        // 子级单独隐藏不影响父级。
        assert!(!s.is_entity_hidden_with_ancestors(SceneEntityId(2)));
        assert!(s.is_entity_hidden_with_ancestors(SceneEntityId(3)));
        s.metadata.insert(key(1), "true".into());
        assert!(s.is_entity_hidden_with_ancestors(SceneEntityId(3)));
    }

    #[test]
    fn version_2_scene_roundtrips_with_3d_components() {
        let text = "scene_version=2\nname=3d\n\
             entity\t1\tCamera\t\n\
             component\t1\tTransform\tposition=0,0,5;rotation=0,0,0,1;scale=1,1,1\n\
             component\t1\tCamera\tprojection=perspective;position=0,0,5;target=0,0,0;fov=60;near=0.1;far=100\n\
             entity\t2\tMesh\t\n\
             component\t2\tMeshRenderer\tmesh=assets/models/cube.glb\n";
        let scene = Scene::from_text(text).unwrap();
        assert_eq!(scene.version, 2);
        assert_eq!(
            scene.entities[&SceneEntityId(1)].components["Transform"],
            "position=0,0,5;rotation=0,0,0,1;scale=1,1,1"
        );
        assert_eq!(
            scene.entities[&SceneEntityId(1)].components["Camera"],
            "projection=perspective;position=0,0,5;target=0,0,0;fov=60;near=0.1;far=100"
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
    fn resources_entry_is_explicit() {
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
