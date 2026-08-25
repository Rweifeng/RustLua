//! Persistent resource database for editor-facing asset identity and metadata.
//!
//! The database deliberately uses a small, dependency-free line format so the
//! first editor can open projects without requiring a serialization runtime.
//!
//! Schema v2 (header `# rustlua-resource-db-v2`) adds the resource family, a
//! content hash and timing metadata. Schema v1 files are migrated on load.

use crate::core::resource_kind::ResourceKind;
use std::{
    collections::HashMap,
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
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

/// Lifecycle state of a resource.
///
/// Six states are durable (persisted on disk); the three transient states
/// (`ImportQueued` / `Importing` / `Removed`) describe in-session work and fall
/// back when the database is saved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceState {
    /// Discovered on disk but not yet imported.
    Discovered,
    /// Last import succeeded; the cached artifact is consumable.
    Ready,
    /// Last import failed; the error is recorded on the asset.
    ImportFailed,
    /// Source changed on disk since the last import.
    Stale,
    /// Source no longer exists on disk.
    Missing,
    /// Source moved outside the scanned roots.
    Orphaned,
    /// Transient: queued for import in the current session.
    ImportQueued,
    /// Transient: import in progress in the current session.
    Importing,
    /// Transient: marked for removal; not written on save.
    Removed,
}

impl ResourceState {
    /// True for states that are meaningful across editor restarts.
    #[must_use]
    pub fn is_durable(self) -> bool {
        matches!(
            self,
            Self::Discovered
                | Self::Ready
                | Self::ImportFailed
                | Self::Stale
                | Self::Missing
                | Self::Orphaned
        )
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::Ready => "ready",
            Self::ImportFailed => "import_failed",
            Self::Stale => "stale",
            Self::Missing => "missing",
            Self::Orphaned => "orphaned",
            Self::ImportQueued => "import_queued",
            Self::Importing => "importing",
            Self::Removed => "removed",
        }
    }

    fn parse(value: &str) -> Result<Self, ResourceDbError> {
        match value {
            "discovered" => Ok(Self::Discovered),
            "ready" => Ok(Self::Ready),
            "import_failed" => Ok(Self::ImportFailed),
            "stale" => Ok(Self::Stale),
            "missing" => Ok(Self::Missing),
            "orphaned" => Ok(Self::Orphaned),
            "import_queued" => Ok(Self::ImportQueued),
            "importing" => Ok(Self::Importing),
            "removed" => Ok(Self::Removed),
            _ => Err(ResourceDbError::InvalidStatus(value.to_owned())),
        }
    }

    /// The durable state to persist for this state, or `None` when the record
    /// should be dropped entirely (`Removed`).
    fn durable_fallback(self) -> Option<Self> {
        match self {
            Self::Discovered
            | Self::Ready
            | Self::ImportFailed
            | Self::Stale
            | Self::Missing
            | Self::Orphaned => Some(self),
            Self::ImportQueued | Self::Importing => Some(Self::Discovered),
            Self::Removed => None,
        }
    }
}

/// One file discovered during a scan, with the mtime needed for change detection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiskAsset {
    pub path: PathBuf,
    /// Source file mtime in unix nanoseconds.
    pub modified: u64,
}

/// One source asset and the metadata needed by an importer/editor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetRecord {
    pub id: AssetId,
    pub source_path: PathBuf,
    pub kind: ResourceKind,
    pub status: ResourceState,
    pub importer_version: String,
    pub cache_path: PathBuf,
    /// Content fingerprint of the last successful import (empty until imported).
    pub content_hash: String,
    /// Source file mtime in unix nanoseconds when it was last seen.
    pub last_modified: u64,
    /// Unix seconds of the last successful import.
    pub last_success: u64,
    /// Unix seconds of the last failed import.
    pub last_error_at: u64,
    pub dependencies: Vec<AssetId>,
    pub error: Option<String>,
}

/// Persistent project resource database.
pub struct ResourceDatabase {
    root: PathBuf,
    db_path: PathBuf,
    records: HashMap<AssetId, AssetRecord>,
    by_path: HashMap<PathBuf, AssetId>,
}

enum SchemaVersion {
    V1,
    V2,
}

impl ResourceDatabase {
    /// Opens an existing database or creates an empty one under `root/.cache`.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ResourceDbError> {
        let root = root.as_ref().to_path_buf();
        let db_path = root
            .join(crate::core::PROJECT_WORK_DIR)
            .join("resources.db");
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

    /// Opens a database, quarantining a corrupt file (renamed `.corrupt-<ts>`)
    /// and returning an empty database instead of failing.
    pub fn open_recoverable(root: impl AsRef<Path>) -> Result<Self, ResourceDbError> {
        let root = root.as_ref().to_path_buf();
        let db_path = root
            .join(crate::core::PROJECT_WORK_DIR)
            .join("resources.db");
        match Self::open(&root) {
            Ok(db) => Ok(db),
            Err(_) if db_path.is_file() => {
                let ts = unix_seconds();
                let quarantine = db_path.with_extension(format!("corrupt-{ts}"));
                fs::rename(&db_path, &quarantine)?;
                Self::open(&root)
            }
            Err(error) => Err(error),
        }
    }

    /// Registers a source, preserving its ID when the same path is seen again
    /// while refreshing the recorded kind and importer version.
    pub fn register(
        &mut self,
        source: impl AsRef<Path>,
        kind: ResourceKind,
        importer_version: &str,
    ) -> Result<AssetId, ResourceDbError> {
        let path = self.normalize(source)?;
        if let Some(id) = self.by_path.get(&path).copied() {
            let record = self
                .records
                .get_mut(&id)
                .expect("path index and records stay in sync");
            record.kind = kind;
            record.importer_version = importer_version.to_owned();
            return Ok(id);
        }
        let id = AssetId::from_seed(&format!("{}:{importer_version}", path.display()));
        let record = AssetRecord {
            id,
            source_path: path.clone(),
            kind,
            status: ResourceState::Discovered,
            importer_version: importer_version.to_owned(),
            cache_path: PathBuf::from(crate::core::PROJECT_WORK_DIR)
                .join("cache")
                .join(id.to_string()),
            content_hash: String::new(),
            last_modified: 0,
            last_success: 0,
            last_error_at: 0,
            dependencies: Vec::new(),
            error: None,
        };
        self.by_path.insert(path, id);
        self.records.insert(id, record);
        Ok(id)
    }

    /// Records a successful import: status becomes `Ready` and the content
    /// fingerprint and cache artifact are stored.
    pub fn mark_ready(
        &mut self,
        id: AssetId,
        cache_path: impl Into<PathBuf>,
        content_hash: impl Into<String>,
    ) -> Result<(), ResourceDbError> {
        let record = self
            .records
            .get_mut(&id)
            .ok_or(ResourceDbError::NotFound(id))?;
        record.status = ResourceState::Ready;
        record.cache_path = cache_path.into();
        record.content_hash = content_hash.into();
        record.error = None;
        record.last_success = unix_seconds();
        Ok(())
    }

    /// Records a failed import: status becomes `ImportFailed` with the error.
    pub fn mark_failed(
        &mut self,
        id: AssetId,
        error: impl Into<String>,
    ) -> Result<(), ResourceDbError> {
        let record = self
            .records
            .get_mut(&id)
            .ok_or(ResourceDbError::NotFound(id))?;
        record.status = ResourceState::ImportFailed;
        record.error = Some(error.into());
        record.last_error_at = unix_seconds();
        Ok(())
    }

    /// Reconciles recorded assets against a fresh scan. New paths are handled by
    /// `register`; this transitions existing records based on disk state:
    /// changed sources become `Stale`, deleted files `Missing`, and files that
    /// moved outside the scanned roots `Orphaned`. Restored files go back to
    /// `Ready` (or `Discovered` if they never imported successfully).
    pub fn reconcile(&mut self, scan: &[DiskAsset], scan_roots: &[PathBuf]) {
        let mut scanned = HashMap::with_capacity(scan.len());
        for asset in scan {
            // 与 register 的 normalize 一致：路径统一为正斜杠再比较。
            let normalized = PathBuf::from(asset.path.to_string_lossy().replace('\\', "/"));
            scanned.insert(normalized, asset.modified);
        }
        let roots: Vec<PathBuf> = scan_roots
            .iter()
            .map(|root| PathBuf::from(root.to_string_lossy().replace('\\', "/")))
            .collect();
        for asset in scan {
            let normalized = PathBuf::from(asset.path.to_string_lossy().replace('\\', "/"));
            let Some(&id) = self.by_path.get(&normalized) else {
                continue;
            };
            let record = self
                .records
                .get_mut(&id)
                .expect("path index and records stay in sync");
            let changed = record.last_modified != 0 && record.last_modified != asset.modified;
            match record.status {
                ResourceState::Missing | ResourceState::Orphaned => {
                    record.status = if changed {
                        ResourceState::Stale
                    } else if record.content_hash.is_empty() {
                        ResourceState::Discovered
                    } else {
                        ResourceState::Ready
                    };
                }
                ResourceState::Ready | ResourceState::Stale | ResourceState::ImportFailed
                    if changed =>
                {
                    record.status = ResourceState::Stale;
                }
                _ => {}
            }
            record.last_modified = asset.modified;
        }
        for record in self.records.values_mut() {
            if scanned.contains_key(&record.source_path) || record.status == ResourceState::Missing
            {
                continue;
            }
            let inside_root = roots
                .iter()
                .any(|root| record.source_path.starts_with(root));
            record.status = if inside_root {
                ResourceState::Missing
            } else {
                ResourceState::Orphaned
            };
        }
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
    /// Transient states fall back: `Removed` records are dropped.
    pub fn save(&self) -> Result<(), ResourceDbError> {
        if let Some(parent) = self.db_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp = self.db_path.with_extension("tmp");
        let mut output = String::from("# rustlua-resource-db-v2\n");
        let mut records: Vec<_> = self.records.values().collect();
        records.sort_by_key(|r| r.id.to_string());
        for r in records {
            let Some(status) = r.status.durable_fallback() else {
                continue;
            };
            output.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                r.id,
                escape(&r.source_path.to_string_lossy()),
                r.kind.token(),
                status.as_str(),
                escape(&r.importer_version),
                escape(&r.cache_path.to_string_lossy()),
                escape(&r.content_hash),
                r.last_modified,
                r.last_success,
                r.last_error_at,
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
        let text = fs::read_to_string(&self.db_path)?;
        match header_version(&text)? {
            SchemaVersion::V1 => self.load_v1(&text)?,
            SchemaVersion::V2 => self.load_v2(&text)?,
        }
        Ok(())
    }

    fn load_v1(&mut self, text: &str) -> Result<(), ResourceDbError> {
        for (line_no, line) in text.lines().enumerate() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let fields: Vec<_> = line.split('\t').collect();
            if fields.len() != 7 {
                return Err(ResourceDbError::Malformed { line: line_no + 1 });
            }
            let id = fields[0].parse()?;
            let path = PathBuf::from(unescape(fields[1])?);
            let status = match fields[2] {
                "registered" => ResourceState::Discovered,
                "imported" => ResourceState::Ready,
                "failed" => ResourceState::ImportFailed,
                "missing" => ResourceState::Missing,
                other => return Err(ResourceDbError::InvalidStatus(other.to_owned())),
            };
            let record = AssetRecord {
                id,
                source_path: path.clone(),
                kind: ResourceKind::from_path(&path),
                status,
                importer_version: unescape(fields[3])?,
                cache_path: PathBuf::from(unescape(fields[4])?),
                content_hash: String::new(),
                last_modified: 0,
                last_success: 0,
                last_error_at: 0,
                dependencies: parse_dependencies(fields[5])?,
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

    fn load_v2(&mut self, text: &str) -> Result<(), ResourceDbError> {
        for (line_no, line) in text.lines().enumerate() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let fields: Vec<_> = line.split('\t').collect();
            if fields.len() != 12 {
                return Err(ResourceDbError::Malformed { line: line_no + 1 });
            }
            let id = fields[0].parse()?;
            let path = PathBuf::from(unescape(fields[1])?);
            let kind = ResourceKind::parse(fields[2])
                .ok_or_else(|| ResourceDbError::InvalidKind(fields[2].to_owned()))?;
            let status = ResourceState::parse(fields[3])?;
            let field = |index: usize| {
                fields[index]
                    .parse()
                    .map_err(|_| ResourceDbError::Malformed { line: line_no + 1 })
            };
            let record = AssetRecord {
                id,
                source_path: path.clone(),
                kind,
                status,
                importer_version: unescape(fields[4])?,
                cache_path: PathBuf::from(unescape(fields[5])?),
                content_hash: unescape(fields[6])?,
                last_modified: field(7)?,
                last_success: field(8)?,
                last_error_at: field(9)?,
                dependencies: parse_dependencies(fields[10])?,
                error: {
                    let e = unescape(fields[11])?;
                    (!e.is_empty()).then_some(e)
                },
            };
            self.by_path.insert(path, id);
            self.records.insert(id, record);
        }
        Ok(())
    }
}

fn header_version(text: &str) -> Result<SchemaVersion, ResourceDbError> {
    if let Some(header) = text.lines().find(|line| line.starts_with('#')) {
        return match header {
            "# rustlua-resource-db-v2" => Ok(SchemaVersion::V2),
            "# rustlua-resource-db-v1" => Ok(SchemaVersion::V1),
            other => Err(ResourceDbError::UnsupportedSchemaVersion(
                other.trim().to_owned(),
            )),
        };
    }
    if text.trim().is_empty() {
        // An empty file is treated as an empty database.
        return Ok(SchemaVersion::V2);
    }
    Err(ResourceDbError::Malformed { line: 1 })
}

fn parse_dependencies(value: &str) -> Result<Vec<AssetId>, ResourceDbError> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    value.split(',').map(str::parse).collect()
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
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
    InvalidKind(String),
    InvalidEscape(String),
    UnsupportedSchemaVersion(String),
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
            Self::InvalidStatus(v) => write!(f, "invalid resource state: {v}"),
            Self::InvalidKind(v) => write!(f, "invalid resource kind: {v}"),
            Self::InvalidEscape(v) => write!(f, "invalid escaped field: {v}"),
            Self::UnsupportedSchemaVersion(v) => {
                write!(f, "unsupported resource database schema: {v}")
            }
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
impl ResourceDatabase {
    fn get_mut_for_test(&mut self, id: AssetId) -> &mut AssetRecord {
        self.records.get_mut(&id).expect("record exists")
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
        let a = db
            .register("hero.png", ResourceKind::Texture, "texture-1")
            .unwrap();
        let b = db
            .register("scene.ron", ResourceKind::Scene, "scene-1")
            .unwrap();
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
        let id = db
            .register("bad.png", ResourceKind::Texture, "texture-1")
            .unwrap();
        db.mark_failed(id, "decode error").unwrap();
        db.save().unwrap();
        let db = ResourceDatabase::open(&root).unwrap();
        let record = db.get(id).unwrap();
        assert_eq!(record.status, ResourceState::ImportFailed);
        assert_eq!(record.error.as_deref(), Some("decode error"));
        assert!(record.last_error_at > 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn duplicate_registration_is_idempotent() {
        let root = temp("duplicate");
        let mut db = ResourceDatabase::open(&root).unwrap();
        assert_eq!(
            db.register("hero.png", ResourceKind::Texture, "texture-1")
                .unwrap(),
            db.register("hero.png", ResourceKind::Texture, "texture-1")
                .unwrap()
        );
        assert_eq!(db.len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn duplicate_registration_refreshes_kind_and_version() {
        let root = temp("refresh-version");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let id = db
            .register("hero.png", ResourceKind::Texture, "texture-1")
            .unwrap();
        db.mark_ready(id, ".cache/cache/hero", "hash").unwrap();
        let again = db
            .register("hero.png", ResourceKind::Model, "model-2")
            .unwrap();
        assert_eq!(again, id);
        let record = db.get(id).unwrap();
        assert_eq!(record.kind, ResourceKind::Model);
        assert_eq!(record.importer_version, "model-2");
        // Re-registration never downgrades an imported asset's state.
        assert_eq!(record.status, ResourceState::Ready);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn mark_ready_records_hash_and_clears_error() {
        let root = temp("ready");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let id = db
            .register("ok.png", ResourceKind::Texture, "texture-1")
            .unwrap();
        db.mark_failed(id, "boom").unwrap();
        db.mark_ready(id, ".cache/cache/ok", "aabb").unwrap();
        let record = db.get(id).unwrap();
        assert_eq!(record.status, ResourceState::Ready);
        assert_eq!(record.content_hash, "aabb");
        assert!(record.error.is_none());
        assert!(record.last_success > 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn transient_states_fall_back_on_save() {
        let root = temp("transient");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let queued = db
            .register("q.png", ResourceKind::Texture, "texture-1")
            .unwrap();
        let importing = db
            .register("i.png", ResourceKind::Texture, "texture-1")
            .unwrap();
        let removed = db
            .register("r.png", ResourceKind::Texture, "texture-1")
            .unwrap();
        db.get_mut_for_test(queued).status = ResourceState::ImportQueued;
        db.get_mut_for_test(importing).status = ResourceState::Importing;
        db.get_mut_for_test(removed).status = ResourceState::Removed;
        db.save().unwrap();
        let db = ResourceDatabase::open(&root).unwrap();
        assert_eq!(db.get(queued).unwrap().status, ResourceState::Discovered);
        assert_eq!(db.get(importing).unwrap().status, ResourceState::Discovered);
        assert!(
            db.get(removed).is_none(),
            "Removed records are dropped on save"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn v1_database_migrates_on_load() {
        let root = temp("v1-migrate");
        let db_path = root
            .join(crate::core::PROJECT_WORK_DIR)
            .join("resources.db");
        fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let id = AssetId::from_seed("assets/hero.png:texture-1");
        fs::write(
            &db_path,
            format!(
                "# rustlua-resource-db-v1\n{id}\tassets/hero.png\timported\ttexture-1\t.cache/cache/{id}\t\t\n"
            ),
        )
        .unwrap();
        let db = ResourceDatabase::open(&root).unwrap();
        let record = db.find_by_path("assets/hero.png").unwrap().unwrap();
        assert_eq!(record.status, ResourceState::Ready);
        assert_eq!(record.kind, ResourceKind::Texture);
        assert_eq!(record.content_hash, "");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn open_recoverable_quarantines_corrupt_database() {
        let root = temp("recover");
        let db_path = root
            .join(crate::core::PROJECT_WORK_DIR)
            .join("resources.db");
        fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        fs::write(
            &db_path,
            "# rustlua-resource-db-v2\nnot-a-valid-line-without-tabs",
        )
        .unwrap();
        assert!(ResourceDatabase::open(&root).is_err());
        let db = ResourceDatabase::open_recoverable(&root).unwrap();
        assert!(db.is_empty());
        let quarantine = fs::read_dir(db_path.parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.file_name().to_string_lossy().contains("corrupt-"));
        assert!(quarantine, "corrupt database is quarantined");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reconcile_tracks_add_change_delete_and_restore() {
        let root = temp("reconcile");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let id = db
            .register("assets/hero.png", ResourceKind::Texture, "texture-1")
            .unwrap();
        db.reconcile(
            &[DiskAsset {
                path: "assets/hero.png".into(),
                modified: 100,
            }],
            &[PathBuf::from("assets")],
        );
        assert_eq!(db.get(id).unwrap().status, ResourceState::Discovered);
        assert_eq!(db.get(id).unwrap().last_modified, 100);
        db.mark_ready(id, ".cache/cache/hero", "h1").unwrap();
        // 无变化扫描：保持 Ready。
        db.reconcile(
            &[DiskAsset {
                path: "assets/hero.png".into(),
                modified: 100,
            }],
            &[PathBuf::from("assets")],
        );
        assert_eq!(db.get(id).unwrap().status, ResourceState::Ready);
        // 文件修改：Stale。
        db.reconcile(
            &[DiskAsset {
                path: "assets/hero.png".into(),
                modified: 200,
            }],
            &[PathBuf::from("assets")],
        );
        assert_eq!(db.get(id).unwrap().status, ResourceState::Stale);
        // 文件删除：Missing。
        db.reconcile(&[], &[PathBuf::from("assets")]);
        assert_eq!(db.get(id).unwrap().status, ResourceState::Missing);
        // 文件恢复（mtime 与删除前一致）：Ready。
        db.reconcile(
            &[DiskAsset {
                path: "assets/hero.png".into(),
                modified: 200,
            }],
            &[PathBuf::from("assets")],
        );
        assert_eq!(db.get(id).unwrap().status, ResourceState::Ready);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reconcile_marks_out_of_scope_as_orphaned() {
        let root = temp("orphan");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let id = db
            .register("assets/hero.png", ResourceKind::Texture, "texture-1")
            .unwrap();
        // 扫描只覆盖 scenes；assets/hero.png 不再出现在任何扫描根下 → Orphaned。
        db.reconcile(
            &[DiskAsset {
                path: "scenes/main.scene".into(),
                modified: 5,
            }],
            &[PathBuf::from("scenes")],
        );
        assert_eq!(db.get(id).unwrap().status, ResourceState::Orphaned);
        // 移回扫描根：恢复为 Discovered（从未成功导入过）。
        db.reconcile(
            &[DiskAsset {
                path: "assets/hero.png".into(),
                modified: 9,
            }],
            &[PathBuf::from("assets"), PathBuf::from("scenes")],
        );
        assert_eq!(db.get(id).unwrap().status, ResourceState::Discovered);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reconcile_normalizes_backslash_scan_paths() {
        // Windows 上 collect_files 返回反斜杠路径；必须与 register 的正斜杠键对齐，
        // 否则 reconcile 把在盘资源误判为 Missing/Orphaned。
        let root = temp("backslash");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let id = db
            .register("assets\\hero.png", ResourceKind::Texture, "texture-1")
            .unwrap();
        db.reconcile(
            &[DiskAsset {
                path: "assets\\hero.png".into(),
                modified: 9,
            }],
            &[PathBuf::from("assets")],
        );
        assert_eq!(db.get(id).unwrap().status, ResourceState::Discovered);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn unsupported_schema_version_is_rejected() {
        let root = temp("schema");
        let db_path = root
            .join(crate::core::PROJECT_WORK_DIR)
            .join("resources.db");
        fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        fs::write(&db_path, "# rustlua-resource-db-v99\n").unwrap();
        assert!(matches!(
            ResourceDatabase::open(&root),
            Err(ResourceDbError::UnsupportedSchemaVersion(_))
        ));
        let _ = fs::remove_dir_all(root);
    }
}
