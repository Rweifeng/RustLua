//! Persistent resource database for editor-facing asset identity and metadata.
//!
//! The database deliberately uses a small, dependency-free line format so the
//! first editor can open projects without requiring a serialization runtime.

use std::{
    collections::HashMap,
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
    str::FromStr,
};

/// Stable 128-bit asset identifier rendered in canonical UUID form.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AssetId([u8; 16]);

impl AssetId {
    /// Creates a deterministic UUID-shaped ID from a unique seed.
    pub fn from_seed(seed: &str) -> Self {
        let mut bytes = [0u8; 16];
        for (i, byte) in seed.as_bytes().iter().enumerate() {
            let slot = i % 16;
            bytes[slot] = bytes[slot].wrapping_add(*byte).rotate_left((i % 8) as u32);
            let other = (slot * 7 + 3) % 16;
            bytes[other] ^= byte.wrapping_mul(31);
        }
        // UUID version 4 shape and RFC 4122 variant; identity is persisted in the DB.
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Self(bytes)
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, byte) in self.0.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                write!(f, "-")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl FromStr for AssetId {
    type Err = ResourceDbError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let compact: String = value.chars().filter(|c| *c != '-').collect();
        if compact.len() != 32 || !compact.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(ResourceDbError::InvalidId(value.to_owned()));
        }
        let mut bytes = [0u8; 16];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&compact[i * 2..i * 2 + 2], 16)
                .map_err(|_| ResourceDbError::InvalidId(value.to_owned()))?;
        }
        Ok(Self(bytes))
    }
}

/// Current import state of a resource.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImportStatus {
    Registered,
    Imported,
    Failed,
    Missing,
}

impl ImportStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Registered => "registered",
            Self::Imported => "imported",
            Self::Failed => "failed",
            Self::Missing => "missing",
        }
    }
    fn parse(value: &str) -> Result<Self, ResourceDbError> {
        match value {
            "registered" => Ok(Self::Registered),
            "imported" => Ok(Self::Imported),
            "failed" => Ok(Self::Failed),
            "missing" => Ok(Self::Missing),
            _ => Err(ResourceDbError::InvalidStatus(value.to_owned())),
        }
    }
}

/// One source asset and the metadata needed by an importer/editor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetRecord {
    pub id: AssetId,
    pub source_path: PathBuf,
    pub status: ImportStatus,
    pub dependencies: Vec<AssetId>,
    pub importer_version: String,
    pub cache_path: PathBuf,
    pub error: Option<String>,
}

/// Persistent project resource database.
pub struct ResourceDatabase {
    root: PathBuf,
    db_path: PathBuf,
    records: HashMap<AssetId, AssetRecord>,
    by_path: HashMap<PathBuf, AssetId>,
}

impl ResourceDatabase {
    /// Opens an existing database or creates an empty one under `root/.p2`.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ResourceDbError> {
        let root = root.as_ref().to_path_buf();
        let db_path = root.join(crate::PROJECT_WORK_DIR).join("resources.db");
        let mut db = Self {
            root,
            db_path,
            records: HashMap::new(),
            by_path: HashMap::new(),
        };
        if db.db_path.exists() {
            db.load()?;
        }
        Ok(db)
    }

    /// Registers a source, preserving its ID when the same path is seen again.
    pub fn register(
        &mut self,
        source: impl AsRef<Path>,
        importer_version: &str,
    ) -> Result<AssetId, ResourceDbError> {
        let path = self.normalize(source)?;
        if let Some(id) = self.by_path.get(&path).copied() {
            return Ok(id);
        }
        let id = AssetId::from_seed(&format!("{}:{importer_version}", path.display()));
        let record = AssetRecord {
            id,
            source_path: path.clone(),
            status: ImportStatus::Registered,
            dependencies: Vec::new(),
            importer_version: importer_version.to_owned(),
            cache_path: PathBuf::from(crate::PROJECT_WORK_DIR)
                .join("cache")
                .join(id.to_string()),
            error: None,
        };
        self.by_path.insert(path, id);
        self.records.insert(id, record);
        Ok(id)
    }

    /// Updates an existing resource's import result without losing its identity.
    pub fn set_import_result(
        &mut self,
        id: AssetId,
        status: ImportStatus,
        cache_path: impl Into<PathBuf>,
        error: Option<String>,
    ) -> Result<(), ResourceDbError> {
        let record = self
            .records
            .get_mut(&id)
            .ok_or(ResourceDbError::NotFound(id))?;
        record.status = status;
        record.cache_path = cache_path.into();
        record.error = error;
        Ok(())
    }

    /// Replaces dependency edges for an asset.
    pub fn set_dependencies(
        &mut self,
        id: AssetId,
        dependencies: Vec<AssetId>,
    ) -> Result<(), ResourceDbError> {
        let record = self
            .records
            .get_mut(&id)
            .ok_or(ResourceDbError::NotFound(id))?;
        record.dependencies = dependencies;
        Ok(())
    }

    /// Moves/renames a source while preserving its UUID and references.
    pub fn move_resource(
        &mut self,
        old: impl AsRef<Path>,
        new: impl AsRef<Path>,
    ) -> Result<AssetId, ResourceDbError> {
        let old = self.normalize(old)?;
        let new = self.normalize(new)?;
        let id = self
            .by_path
            .remove(&old)
            .ok_or_else(|| ResourceDbError::PathNotFound(old.clone()))?;
        if self.by_path.contains_key(&new) {
            self.by_path.insert(old, id);
            return Err(ResourceDbError::PathAlreadyRegistered(new));
        }
        self.records
            .get_mut(&id)
            .expect("path index and records stay in sync")
            .source_path = new.clone();
        self.by_path.insert(new, id);
        Ok(id)
    }

    /// Removes an asset and returns assets that still depend on it.
    pub fn remove(&mut self, source: impl AsRef<Path>) -> Result<Vec<AssetId>, ResourceDbError> {
        let path = self.normalize(source)?;
        let id = self
            .by_path
            .remove(&path)
            .ok_or(ResourceDbError::PathNotFound(path))?;
        self.records.remove(&id);
        Ok(self
            .records
            .values()
            .filter(|r| r.dependencies.contains(&id))
            .map(|r| r.id)
            .collect())
    }

    pub fn get(&self, id: AssetId) -> Option<&AssetRecord> {
        self.records.get(&id)
    }
    pub fn find_by_path(
        &self,
        source: impl AsRef<Path>,
    ) -> Result<Option<&AssetRecord>, ResourceDbError> {
        let path = self.normalize(source)?;
        Ok(self.by_path.get(&path).and_then(|id| self.records.get(id)))
    }
    pub fn len(&self) -> usize {
        self.records.len()
    }
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Atomically replaces the database file, so failed imports cannot corrupt it.
    pub fn save(&self) -> Result<(), ResourceDbError> {
        if let Some(parent) = self.db_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp = self.db_path.with_extension("tmp");
        let mut output = String::from("# rustlua-resource-db-v1\n");
        let mut records: Vec<_> = self.records.values().collect();
        records.sort_by_key(|r| r.id.to_string());
        for r in records {
            output.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                r.id,
                escape(&r.source_path.to_string_lossy()),
                r.status.as_str(),
                escape(&r.importer_version),
                escape(&r.cache_path.to_string_lossy()),
                r.dependencies
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
                escape(r.error.as_deref().unwrap_or(""))
            ));
        }
        fs::write(&temp, output)?;
        fs::rename(temp, &self.db_path)?;
        Ok(())
    }

    fn normalize(&self, path: impl AsRef<Path>) -> Result<PathBuf, ResourceDbError> {
        let path = path.as_ref();
        let relative = path.strip_prefix(&self.root).unwrap_or(path);
        let normalized = relative.to_string_lossy().replace('\\', "/");
        if normalized.is_empty() || normalized == "." || normalized.starts_with("../") {
            return Err(ResourceDbError::InvalidPath(path.display().to_string()));
        }
        Ok(PathBuf::from(normalized))
    }

    fn load(&mut self) -> Result<(), ResourceDbError> {
        for (line_no, line) in fs::read_to_string(&self.db_path)?.lines().enumerate() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let fields: Vec<_> = line.split('\t').collect();
            if fields.len() != 7 {
                return Err(ResourceDbError::Malformed { line: line_no + 1 });
            }
            let id = fields[0].parse()?;
            let path = PathBuf::from(unescape(fields[1])?);
            let status = ImportStatus::parse(fields[2])?;
            let dependencies = if fields[5].is_empty() {
                Vec::new()
            } else {
                fields[5]
                    .split(',')
                    .map(str::parse)
                    .collect::<Result<_, _>>()?
            };
            let record = AssetRecord {
                id,
                source_path: path.clone(),
                status,
                dependencies,
                importer_version: unescape(fields[3])?,
                cache_path: PathBuf::from(unescape(fields[4])?),
                error: {
                    let e = unescape(fields[6])?;
                    (!e.is_empty()).then_some(e)
                },
            };
            self.by_path.insert(path, id);
            self.records.insert(id, record);
        }
        Ok(())
    }
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}
fn unescape(value: &str) -> Result<String, ResourceDbError> {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            _ => return Err(ResourceDbError::InvalidEscape(value.to_owned())),
        }
    }
    Ok(out)
}

#[derive(Debug)]
pub enum ResourceDbError {
    Io(std::io::Error),
    InvalidId(String),
    InvalidStatus(String),
    InvalidEscape(String),
    Malformed { line: usize },
    InvalidPath(String),
    NotFound(AssetId),
    PathNotFound(PathBuf),
    PathAlreadyRegistered(PathBuf),
}
impl fmt::Display for ResourceDbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "resource database I/O failed: {e}"),
            Self::InvalidId(v) => write!(f, "invalid asset UUID: {v}"),
            Self::InvalidStatus(v) => write!(f, "invalid import status: {v}"),
            Self::InvalidEscape(v) => write!(f, "invalid escaped field: {v}"),
            Self::Malformed { line } => write!(f, "malformed resource database line {line}"),
            Self::InvalidPath(v) => write!(f, "invalid project-relative asset path: {v}"),
            Self::NotFound(id) => write!(f, "asset not found: {id}"),
            Self::PathNotFound(p) => write!(f, "asset path not found: {}", p.display()),
            Self::PathAlreadyRegistered(p) => {
                write!(f, "asset path already registered: {}", p.display())
            }
        }
    }
}
impl Error for ResourceDbError {}
impl From<std::io::Error> for ResourceDbError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rustlua-resource-db-{name}-{}", std::process::id()))
    }
    #[test]
    fn id_round_trips() {
        let id = AssetId::from_seed("hero.png");
        assert_eq!(id, id.to_string().parse().unwrap());
        assert_eq!(id.to_string().len(), 36);
    }
    #[test]
    fn move_preserves_identity_and_remove_reports_dependents() {
        let root = temp("move");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let a = db.register("hero.png", "png-1").unwrap();
        let b = db.register("scene.ron", "scene-1").unwrap();
        db.set_dependencies(b, vec![a]).unwrap();
        assert_eq!(db.move_resource("hero.png", "player.png").unwrap(), a);
        assert_eq!(db.find_by_path("player.png").unwrap().unwrap().id, a);
        assert_eq!(db.remove("player.png").unwrap(), vec![b]);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn persistence_keeps_failed_state_and_metadata() {
        let root = temp("persist");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let id = db.register("bad.png", "png-1").unwrap();
        db.set_import_result(
            id,
            ImportStatus::Failed,
            ".cache/cache/bad",
            Some("decode error".into()),
        )
        .unwrap();
        db.save().unwrap();
        let db = ResourceDatabase::open(&root).unwrap();
        let record = db.get(id).unwrap();
        assert_eq!(record.status, ImportStatus::Failed);
        assert_eq!(record.error.as_deref(), Some("decode error"));
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn duplicate_registration_is_idempotent() {
        let root = temp("duplicate");
        let mut db = ResourceDatabase::open(&root).unwrap();
        assert_eq!(
            db.register("hero.png", "png-1").unwrap(),
            db.register("hero.png", "png-1").unwrap()
        );
        assert_eq!(db.len(), 1);
        let _ = fs::remove_dir_all(root);
    }
}
