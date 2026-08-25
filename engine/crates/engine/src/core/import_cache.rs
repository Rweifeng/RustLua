//! Incremental, atomic asset import cache for M4-02.

use crate::core::{
    gltf_asset::GltfAsset,
    resource_db::AssetId,
    texture::{TextureAsset, TextureColorSpace},
};
use std::{
    collections::HashMap,
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

/// Import formats recognized by the first importer registry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImportKind {
    Png,
    Wav,
    Ogg,
    Gltf,
    Glb,
    Lua,
    Scene,
    Prefab,
}

impl ImportKind {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, ImportCacheError> {
        let extension = path
            .as_ref()
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        match extension.as_str() {
            "png" => Ok(Self::Png),
            "wav" => Ok(Self::Wav),
            "ogg" => Ok(Self::Ogg),
            "gltf" => Ok(Self::Gltf),
            "glb" => Ok(Self::Glb),
            "lua" => Ok(Self::Lua),
            "scene" | "ron" | "json" => Ok(Self::Scene),
            "prefab" => Ok(Self::Prefab),
            _ => Err(ImportCacheError::UnsupportedExtension(
                path.as_ref().display().to_string(),
            )),
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Wav => "wav",
            Self::Ogg => "ogg",
            Self::Gltf => "gltf",
            Self::Glb => "glb",
            Self::Lua => "lua",
            Self::Scene => "scene",
            Self::Prefab => "prefab",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CacheRecord {
    fingerprint: String,
    importer_version: String,
    kind: String,
    cache_path: PathBuf,
}

/// Result of one incremental import attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportReport {
    pub cache_path: PathBuf,
    pub fingerprint: String,
    pub reused: bool,
    pub kind: ImportKind,
}

/// Persistent cache indexed by asset UUID.
pub struct ImportCache {
    root: PathBuf,
    metadata_path: PathBuf,
    records: HashMap<AssetId, CacheRecord>,
}

impl ImportCache {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ImportCacheError> {
        let root = root.as_ref().to_path_buf();
        let metadata_path = root
            .join(crate::core::PROJECT_WORK_DIR)
            .join("import-cache.db");
        let mut cache = Self {
            root,
            metadata_path,
            records: HashMap::new(),
        };
        if cache.metadata_path.exists() {
            cache.load()?;
        }
        Ok(cache)
    }

    /// Imports a source only when content or importer version changed.
    pub fn import_file(
        &mut self,
        id: AssetId,
        source: impl AsRef<Path>,
        importer_version: &str,
    ) -> Result<ImportReport, ImportCacheError> {
        let source = source.as_ref();
        let kind = ImportKind::from_path(source)?;
        let bytes = fs::read(source).map_err(|source_error| ImportCacheError::Read {
            path: source.display().to_string(),
            reason: source_error.to_string(),
        })?;
        let fingerprint = crate::core::hash::content_hash(&bytes);
        if let Some(record) = self.records.get(&id)
            && record.fingerprint == fingerprint
            && record.importer_version == importer_version
            && record.kind == kind.as_str()
            && self.root.join(&record.cache_path).is_file()
        {
            return Ok(ImportReport {
                cache_path: record.cache_path.clone(),
                fingerprint,
                reused: true,
                kind,
            });
        }
        validate_source(kind, source, &bytes)?;
        let relative_cache = PathBuf::from(crate::core::PROJECT_WORK_DIR)
            .join("cache")
            .join(format!("{}-{fingerprint}.bin", id));
        let absolute_cache = self.root.join(&relative_cache);
        if let Some(parent) = absolute_cache.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = absolute_cache.with_extension("tmp");
        fs::write(&temporary, &bytes)?;
        fs::rename(&temporary, &absolute_cache)?;
        self.records.insert(
            id,
            CacheRecord {
                fingerprint: fingerprint.clone(),
                importer_version: importer_version.to_owned(),
                kind: kind.as_str().to_owned(),
                cache_path: relative_cache.clone(),
            },
        );
        self.save()?;
        Ok(ImportReport {
            cache_path: relative_cache,
            fingerprint,
            reused: false,
            kind,
        })
    }

    pub fn contains(&self, id: AssetId) -> bool {
        self.records.contains_key(&id)
    }
    pub fn len(&self) -> usize {
        self.records.len()
    }
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Removes all generated artifacts and metadata so a full rebuild can start clean.
    pub fn clear(&mut self) -> Result<(), ImportCacheError> {
        let cache_dir = self.root.join(crate::core::PROJECT_WORK_DIR).join("cache");
        if cache_dir.exists() {
            fs::remove_dir_all(cache_dir)?;
        }
        self.records.clear();
        if self.metadata_path.exists() {
            fs::remove_file(&self.metadata_path)?;
        }
        Ok(())
    }

    fn save(&self) -> Result<(), ImportCacheError> {
        if let Some(parent) = self.metadata_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = self.metadata_path.with_extension("tmp");
        let mut output = String::from("# rustlua-import-cache-v1\n");
        let mut records: Vec<_> = self.records.iter().collect();
        records.sort_by_key(|(id, _)| id.to_string());
        for (id, record) in records {
            output.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\n",
                id,
                record.fingerprint,
                record.importer_version,
                record.kind,
                record.cache_path.to_string_lossy().replace('\\', "/")
            ));
        }
        fs::write(&temporary, output)?;
        fs::rename(temporary, &self.metadata_path)?;
        Ok(())
    }

    fn load(&mut self) -> Result<(), ImportCacheError> {
        for (line_no, line) in fs::read_to_string(&self.metadata_path)?.lines().enumerate() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let fields: Vec<_> = line.split('\t').collect();
            if fields.len() != 5 {
                return Err(ImportCacheError::Malformed { line: line_no + 1 });
            }
            let id = fields[0]
                .parse()
                .map_err(|_| ImportCacheError::Malformed { line: line_no + 1 })?;
            self.records.insert(
                id,
                CacheRecord {
                    fingerprint: fields[1].to_owned(),
                    importer_version: fields[2].to_owned(),
                    kind: fields[3].to_owned(),
                    cache_path: PathBuf::from(fields[4]),
                },
            );
        }
        Ok(())
    }
}

fn validate_source(kind: ImportKind, source: &Path, bytes: &[u8]) -> Result<(), ImportCacheError> {
    if bytes.is_empty() {
        return Err(ImportCacheError::EmptySource(source.display().to_string()));
    }
    if kind == ImportKind::Png {
        TextureAsset::from_png_bytes(bytes, TextureColorSpace::Srgb).map_err(|error| {
            ImportCacheError::InvalidSource {
                path: source.display().to_string(),
                reason: error.to_string(),
            }
        })?;
    }
    if matches!(kind, ImportKind::Gltf | ImportKind::Glb) {
        GltfAsset::load(source).map_err(|error| ImportCacheError::InvalidSource {
            path: source.display().to_string(),
            reason: error.to_string(),
        })?;
    }
    Ok(())
}

#[derive(Debug)]
pub enum ImportCacheError {
    Io(std::io::Error),
    Read { path: String, reason: String },
    UnsupportedExtension(String),
    EmptySource(String),
    InvalidSource { path: String, reason: String },
    Malformed { line: usize },
}
impl fmt::Display for ImportCacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "import cache I/O failed: {error}"),
            Self::Read { path, reason } => write!(f, "cannot read source {path}: {reason}"),
            Self::UnsupportedExtension(path) => write!(f, "unsupported asset extension: {path}"),
            Self::EmptySource(path) => write!(f, "source is empty: {path}"),
            Self::InvalidSource { path, reason } => write!(f, "invalid source {path}: {reason}"),
            Self::Malformed { line } => write!(f, "malformed import cache line {line}"),
        }
    }
}
impl Error for ImportCacheError {}
impl From<std::io::Error> for ImportCacheError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rustlua-import-cache-{name}-{}",
            std::process::id()
        ))
    }
    fn source(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::File::create(&path).unwrap().write_all(bytes).unwrap();
        path
    }
    #[test]
    fn unchanged_source_reuses_cache_and_reload_keeps_hit() {
        let root = root("hit");
        let mut cache = ImportCache::open(&root).unwrap();
        let id = AssetId::from_seed("a");
        let path = source(&root, "script.lua", b"print('a')");
        let first = cache.import_file(id, &path, "lua-1").unwrap();
        assert!(!first.reused);
        let second = cache.import_file(id, &path, "lua-1").unwrap();
        assert!(second.reused);
        let mut reloaded = ImportCache::open(&root).unwrap();
        assert!(reloaded.import_file(id, &path, "lua-1").unwrap().reused);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn recognizes_scene_extension_as_import_kind() {
        assert_eq!(
            ImportKind::from_path("scenes/main.scene").unwrap(),
            ImportKind::Scene
        );
        assert_eq!(
            ImportKind::from_path("assets/models/hero.glb").unwrap(),
            ImportKind::Glb
        );
        assert!(ImportKind::from_path("assets/unknown.xyz").is_err());
    }
    #[test]
    fn content_or_version_change_reimports() {
        let root = root("change");
        let mut cache = ImportCache::open(&root).unwrap();
        let id = AssetId::from_seed("a");
        let path = source(&root, "scene.ron", b"one");
        assert!(!cache.import_file(id, &path, "scene-1").unwrap().reused);
        assert!(!cache.import_file(id, &path, "scene-2").unwrap().reused);
        fs::write(&path, b"two").unwrap();
        assert!(!cache.import_file(id, &path, "scene-2").unwrap().reused);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn failed_import_does_not_replace_previous_valid_cache() {
        let root = root("failure");
        let mut cache = ImportCache::open(&root).unwrap();
        let id = AssetId::from_seed("a");
        let path = source(
            &root,
            "hero.png",
            include_bytes!("../../../../../game_test/assets/textures/red-1x1.png"),
        );
        let first = cache.import_file(id, &path, "png-1").unwrap();
        fs::write(&path, b"bad").unwrap();
        assert!(cache.import_file(id, &path, "png-1").is_err());
        fs::write(
            &path,
            include_bytes!("../../../../../game_test/assets/textures/red-1x1.png"),
        )
        .unwrap();
        let hit = cache.import_file(id, &path, "png-1").unwrap();
        assert!(hit.reused || hit.cache_path == first.cache_path);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn clear_removes_artifacts_and_allows_rebuild() {
        let root = root("clear");
        let mut cache = ImportCache::open(&root).unwrap();
        let id = AssetId::from_seed("a");
        let path = source(&root, "scene.json", b"{}");
        let first = cache.import_file(id, &path, "scene-1").unwrap();
        assert!(root.join(first.cache_path).exists());
        cache.clear().unwrap();
        assert_eq!(cache.len(), 0);
        assert!(
            !root
                .join(crate::core::PROJECT_WORK_DIR)
                .join("cache")
                .exists()
        );
        let rebuilt = cache.import_file(id, &path, "scene-1").unwrap();
        assert!(!rebuilt.reused);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn unsupported_and_empty_inputs_are_rejected() {
        let root = root("invalid");
        let mut cache = ImportCache::open(&root).unwrap();
        let id = AssetId::from_seed("a");
        let path = source(&root, "data.bin", b"x");
        assert!(matches!(
            cache.import_file(id, &path, "bin-1"),
            Err(ImportCacheError::UnsupportedExtension(_))
        ));
        let path = source(&root, "empty.lua", b"");
        assert!(matches!(
            cache.import_file(id, &path, "lua-1"),
            Err(ImportCacheError::EmptySource(_))
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn glb_import_is_validated_and_cached_incrementally() {
        let root = root("gltf");
        let mut cache = ImportCache::open(&root).unwrap();
        let id = AssetId::from_seed("triangle");
        let path = source(
            &root,
            "m8-test-triangle.glb",
            include_bytes!("../../../../../game_test/assets/models/m8-test-triangle.glb"),
        );
        let first = cache.import_file(id, &path, "assetc-m8-02").unwrap();
        assert!(!first.reused);
        assert!(cache.import_file(id, &path, "assetc-m8-02").unwrap().reused);
        fs::write(&path, b"invalid glb").unwrap();
        assert!(matches!(
            cache.import_file(id, &path, "assetc-m8-02"),
            Err(ImportCacheError::InvalidSource { .. })
        ));
        let _ = fs::remove_dir_all(root);
    }
}
