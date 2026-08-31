//! Incremental, atomic asset import cache for M4-02.
//!
//! The cache stores a content fingerprint per asset UUID and reuses the cached
//! artifact when neither content nor importer version changed. Validation and
//! artifact production are delegated to an [`Importer`].

use crate::core::{importer::Importer, resource_db::AssetId};
use std::{
    collections::HashMap,
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

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
    /// Project-root-relative path of the primary cached artifact.
    pub cache_path: PathBuf,
    /// Content fingerprint of the imported source bytes.
    pub fingerprint: String,
    /// True when the cache hit and no new artifact was written.
    pub reused: bool,
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
        importer: &dyn Importer,
    ) -> Result<ImportReport, ImportCacheError> {
        let source = source.as_ref();
        let bytes = fs::read(source).map_err(|source_error| ImportCacheError::Read {
            path: source.display().to_string(),
            reason: source_error.to_string(),
        })?;
        let fingerprint = crate::core::hash::content_hash(&bytes);
        let kind = importer.kind();
        let importer_version = importer.version();
        if let Some(record) = self.records.get(&id)
            && record.fingerprint == fingerprint
            && record.importer_version == importer_version
            && record.kind == kind.token()
            && self.root.join(&record.cache_path).is_file()
        {
            return Ok(ImportReport {
                cache_path: record.cache_path.clone(),
                fingerprint,
                reused: true,
            });
        }
        importer
            .validate(source, &bytes)
            .map_err(|reason| ImportCacheError::InvalidSource {
                path: source.display().to_string(),
                reason,
            })?;
        let artifacts = importer
            .produce(&bytes, id, &fingerprint)
            .map_err(|reason| ImportCacheError::InvalidSource {
                path: source.display().to_string(),
                reason,
            })?;
        if artifacts.is_empty() {
            return Err(ImportCacheError::InvalidSource {
                path: source.display().to_string(),
                reason: "importer 未生成任何产物".to_owned(),
            });
        }
        let relative_cache = artifacts[0].path.clone();
        for artifact in &artifacts {
            let absolute_cache = self.root.join(&artifact.path);
            if let Some(parent) = absolute_cache.parent() {
                fs::create_dir_all(parent)?;
            }
            let temporary = absolute_cache.with_extension("tmp");
            fs::write(&temporary, &artifact.bytes)?;
            fs::rename(&temporary, &absolute_cache)?;
        }
        self.records.insert(
            id,
            CacheRecord {
                fingerprint: fingerprint.clone(),
                importer_version: importer_version.to_owned(),
                kind: kind.token().to_owned(),
                cache_path: relative_cache.clone(),
            },
        );
        self.save()?;
        Ok(ImportReport {
            cache_path: relative_cache,
            fingerprint,
            reused: false,
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

#[derive(Debug)]
pub enum ImportCacheError {
    Io(std::io::Error),
    Read { path: String, reason: String },
    InvalidSource { path: String, reason: String },
    Malformed { line: usize },
}
impl fmt::Display for ImportCacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "import cache I/O failed: {error}"),
            Self::Read { path, reason } => write!(f, "cannot read source {path}: {reason}"),
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
    use crate::core::{importer::Importer, resource_kind::ResourceKind};
    use std::io::Write;

    fn red_png() -> Vec<u8> {
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(1, 1, image::Rgb([255, 0, 0])))
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    fn fixture(relative_path: &str) -> Option<Vec<u8>> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_path);
        fs::read(path).ok()
    }

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

    /// Test-only importer that never rejects a source and reports a fixed version.
    struct TestImporter {
        version: &'static str,
        kind: ResourceKind,
    }
    impl Importer for TestImporter {
        fn version(&self) -> &'static str {
            self.version
        }
        fn kind(&self) -> ResourceKind {
            self.kind
        }
        fn extensions(&self) -> &'static [&'static str] {
            &["lua", "scene", "png", "glb"]
        }
        fn validate(&self, _source: &Path, _bytes: &[u8]) -> Result<(), String> {
            Ok(())
        }
    }

    fn scene_importer() -> TestImporter {
        TestImporter {
            version: "scene-1",
            kind: ResourceKind::Scene,
        }
    }

    #[test]
    fn unchanged_source_reuses_cache_and_reload_keeps_hit() {
        let root = root("hit");
        let mut cache = ImportCache::open(&root).unwrap();
        let id = AssetId::from_seed("a");
        let path = source(&root, "script.lua", b"print('a')");
        let importer = scene_importer();
        let first = cache.import_file(id, &path, &importer).unwrap();
        assert!(!first.reused);
        let second = cache.import_file(id, &path, &importer).unwrap();
        assert!(second.reused);
        let mut reloaded = ImportCache::open(&root).unwrap();
        assert!(reloaded.import_file(id, &path, &importer).unwrap().reused);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn content_or_importer_version_change_reimports() {
        let root = root("change");
        let mut cache = ImportCache::open(&root).unwrap();
        let id = AssetId::from_seed("a");
        let path = source(&root, "scene.ron", b"one");
        let v1 = scene_importer();
        let v2 = TestImporter {
            version: "scene-2",
            kind: ResourceKind::Scene,
        };
        assert!(!cache.import_file(id, &path, &v1).unwrap().reused);
        assert!(!cache.import_file(id, &path, &v2).unwrap().reused);
        fs::write(&path, b"two").unwrap();
        assert!(!cache.import_file(id, &path, &v2).unwrap().reused);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn failed_import_does_not_replace_previous_valid_cache() {
        let root = root("failure");
        let mut cache = ImportCache::open(&root).unwrap();
        let id = AssetId::from_seed("a");
        let path = source(&root, "hero.png", &red_png());
        let importer = crate::core::importer::ImporterRegistry::builtin()
            .get(ResourceKind::Texture)
            .unwrap();
        let first = cache.import_file(id, &path, importer).unwrap();
        fs::write(&path, b"bad").unwrap();
        assert!(cache.import_file(id, &path, importer).is_err());
        fs::write(&path, red_png()).unwrap();
        let hit = cache.import_file(id, &path, importer).unwrap();
        assert!(hit.reused || hit.cache_path == first.cache_path);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn clear_removes_artifacts_and_allows_rebuild() {
        let root = root("clear");
        let mut cache = ImportCache::open(&root).unwrap();
        let id = AssetId::from_seed("a");
        let path = source(&root, "scene.json", b"{}");
        let importer = scene_importer();
        let first = cache.import_file(id, &path, &importer).unwrap();
        assert!(root.join(first.cache_path).exists());
        cache.clear().unwrap();
        assert_eq!(cache.len(), 0);
        assert!(
            !root
                .join(crate::core::PROJECT_WORK_DIR)
                .join("cache")
                .exists()
        );
        let rebuilt = cache.import_file(id, &path, &importer).unwrap();
        assert!(!rebuilt.reused);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn empty_sources_are_rejected_before_caching() {
        let root = root("invalid");
        let mut cache = ImportCache::open(&root).unwrap();
        let id = AssetId::from_seed("a");
        let path = source(&root, "empty.lua", b"");
        let importer = crate::core::importer::ImporterRegistry::builtin()
            .get(ResourceKind::LuaScript)
            .unwrap();
        assert!(matches!(
            cache.import_file(id, &path, importer),
            Err(ImportCacheError::InvalidSource { .. })
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn glb_import_is_validated_and_cached_incrementally() {
        let root = root("gltf");
        let mut cache = ImportCache::open(&root).unwrap();
        let id = AssetId::from_seed("triangle");
        let Some(bytes) = fixture("../../../../games/game_test/assets/models/m8-test-triangle.glb")
        else {
            return;
        };
        let path = source(&root, "m8-test-triangle.glb", &bytes);
        let importer = crate::core::importer::ImporterRegistry::builtin()
            .get(ResourceKind::Model)
            .unwrap();
        let first = cache.import_file(id, &path, importer).unwrap();
        assert!(!first.reused);
        assert!(cache.import_file(id, &path, importer).unwrap().reused);
        fs::write(&path, b"invalid glb").unwrap();
        assert!(matches!(
            cache.import_file(id, &path, importer),
            Err(ImportCacheError::InvalidSource { .. })
        ));
        let _ = fs::remove_dir_all(root);
    }
}
