//! Importer trait, builtin registry, and batch runner for M12-01.
//!
//! Each [`ResourceKind`] has exactly one builtin [`Importer`] that validates
//! source files and produces cache artifacts. [`run_import_batch`] drives a
//! batch of imports, writes per-item state back to the
//! [`ResourceDatabase`](crate::core::resource_db::ResourceDatabase), and never
//! aborts the whole batch because of one bad item.

use crate::core::{
    gltf_asset::GltfAsset,
    import_cache::ImportCache,
    prefab::Prefab,
    resource_db::{AssetId, ResourceDatabase},
    resource_kind::ResourceKind,
    scene::Scene,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

/// One artifact an importer wants written to the cache.
#[derive(Clone, Debug)]
pub struct ImportArtifact {
    /// Project-root-relative path (e.g. `.cache/cache/{id}-{hash}.bin`).
    pub path: PathBuf,
    /// File contents of this artifact.
    pub bytes: Vec<u8>,
}

/// Validates a source file and produces cache artifacts for one resource kind.
pub trait Importer: Send + Sync {
    /// Importer version for this kind; bumping it forces a re-import of every asset.
    fn version(&self) -> &'static str;
    /// The unified resource family this importer handles.
    fn kind(&self) -> ResourceKind;
    /// Lowercase extensions this importer accepts, without the leading dot.
    fn extensions(&self) -> &'static [&'static str];
    /// Rejects malformed or unsupported sources before they reach the cache.
    fn validate(&self, source: &Path, bytes: &[u8]) -> Result<(), String>;
    /// Produces the cached artifacts. The default copies the raw source bytes
    /// into a single artifact; formatters may override this to transform.
    fn produce(
        &self,
        bytes: &[u8],
        id: AssetId,
        hash: &str,
    ) -> Result<Vec<ImportArtifact>, String> {
        Ok(vec![ImportArtifact {
            path: PathBuf::from(crate::core::PROJECT_WORK_DIR)
                .join("cache")
                .join(format!("{id}-{hash}.bin")),
            bytes: bytes.to_vec(),
        }])
    }
}

/// Rejects empty sources uniformly across builtin importers.
fn reject_empty(source: &Path, bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() {
        Err(format!("{}: 源文件为空", source.display()))
    } else {
        Ok(())
    }
}

struct TextureImporter;
struct AudioImporter;
struct ScriptImporter;
struct SceneImporter;
struct PrefabImporter;
struct ModelImporter;
struct AtlasImporter;
struct FontImporter;

impl Importer for TextureImporter {
    fn version(&self) -> &'static str {
        "texture-1"
    }
    fn kind(&self) -> ResourceKind {
        ResourceKind::Texture
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["png", "jpg", "jpeg"]
    }
    fn validate(&self, source: &Path, bytes: &[u8]) -> Result<(), String> {
        reject_empty(source, bytes)?;
        image::load_from_memory(bytes)
            .map_err(|error| format!("{}: 图像解码失败: {error}", source.display()))?;
        Ok(())
    }
}

impl Importer for AudioImporter {
    fn version(&self) -> &'static str {
        "audio-1"
    }
    fn kind(&self) -> ResourceKind {
        ResourceKind::Audio
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["wav", "ogg"]
    }
    fn validate(&self, source: &Path, bytes: &[u8]) -> Result<(), String> {
        reject_empty(source, bytes)?;
        use rodio::Source;
        let decoder = rodio::Decoder::new(std::io::Cursor::new(bytes.to_vec()))
            .map_err(|error| format!("{}: 音频解码失败: {error}", source.display()))?;
        let extension = source
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        // WAV 头含数据长度，可强制读头；OGG 为流式格式，头部无总时长，解码器
        // 初始化成功即视为有效。
        if extension == "wav" && decoder.total_duration().is_none() {
            return Err(format!("{}: 无法读取音频时长", source.display()));
        }
        Ok(())
    }
}

impl Importer for ScriptImporter {
    fn version(&self) -> &'static str {
        "script-1"
    }
    fn kind(&self) -> ResourceKind {
        ResourceKind::LuaScript
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["lua"]
    }
    fn validate(&self, source: &Path, bytes: &[u8]) -> Result<(), String> {
        // 语法校验留 M12-04；此处仅要求非空。
        reject_empty(source, bytes)
    }
}

impl Importer for SceneImporter {
    fn version(&self) -> &'static str {
        "scene-1"
    }
    fn kind(&self) -> ResourceKind {
        ResourceKind::Scene
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["scene", "ron", "json"]
    }
    fn validate(&self, source: &Path, bytes: &[u8]) -> Result<(), String> {
        reject_empty(source, bytes)?;
        let extension = source
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if extension == "scene" {
            let text = std::str::from_utf8(bytes)
                .map_err(|error| format!("{}: 场景编码非法: {error}", source.display()))?;
            Scene::from_text(text)
                .map_err(|error| format!("{}: 场景解析失败: {error}", source.display()))?;
        }
        // .ron/.json 宽松校验：可能是任意数据文件，本阶段仅要求非空。
        Ok(())
    }
}

impl Importer for PrefabImporter {
    fn version(&self) -> &'static str {
        "prefab-1"
    }
    fn kind(&self) -> ResourceKind {
        ResourceKind::Prefab
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["prefab"]
    }
    fn validate(&self, source: &Path, bytes: &[u8]) -> Result<(), String> {
        reject_empty(source, bytes)?;
        let text = std::str::from_utf8(bytes)
            .map_err(|error| format!("{}: 预制体编码非法: {error}", source.display()))?;
        Prefab::from_text(text)
            .map_err(|error| format!("{}: 预制体解析失败: {error}", source.display()))?;
        Ok(())
    }
}

impl Importer for ModelImporter {
    fn version(&self) -> &'static str {
        "model-1"
    }
    fn kind(&self) -> ResourceKind {
        ResourceKind::Model
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["gltf", "glb"]
    }
    fn validate(&self, source: &Path, bytes: &[u8]) -> Result<(), String> {
        reject_empty(source, bytes)?;
        GltfAsset::load(source).map_err(|error| format!("{}: {error}", source.display()))?;
        Ok(())
    }
}

impl Importer for AtlasImporter {
    fn version(&self) -> &'static str {
        "atlas-1"
    }
    fn kind(&self) -> ResourceKind {
        ResourceKind::Atlas
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["atlas"]
    }
    fn validate(&self, source: &Path, bytes: &[u8]) -> Result<(), String> {
        // 占位：阶段 1 尚无 atlas 解析器，仅校验非空。
        reject_empty(source, bytes)
    }
}

impl Importer for FontImporter {
    fn version(&self) -> &'static str {
        "font-1"
    }
    fn kind(&self) -> ResourceKind {
        ResourceKind::Font
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["ttf", "otf"]
    }
    fn validate(&self, source: &Path, bytes: &[u8]) -> Result<(), String> {
        reject_empty(source, bytes)?;
        ab_glyph::FontArc::try_from_vec(bytes.to_vec())
            .map_err(|error| format!("{}: 字体解析失败: {error}", source.display()))?;
        Ok(())
    }
}

/// The set of builtin importers, keyed by resource kind.
pub struct ImporterRegistry {
    map: HashMap<ResourceKind, &'static dyn Importer>,
}

impl ImporterRegistry {
    /// The process-wide registry of builtin importers.
    pub fn builtin() -> &'static Self {
        use std::sync::OnceLock;
        static REGISTRY: OnceLock<ImporterRegistry> = OnceLock::new();
        REGISTRY.get_or_init(Self::new)
    }

    fn new() -> Self {
        let importers: [&'static dyn Importer; 8] = [
            &TextureImporter,
            &AudioImporter,
            &ScriptImporter,
            &SceneImporter,
            &PrefabImporter,
            &ModelImporter,
            &AtlasImporter,
            &FontImporter,
        ];
        let mut map = HashMap::with_capacity(importers.len());
        for importer in importers {
            map.insert(importer.kind(), importer);
        }
        Self { map }
    }

    /// Returns the importer for a resource kind, if one is built in.
    pub fn get(&self, kind: ResourceKind) -> Option<&'static dyn Importer> {
        self.map.get(&kind).copied()
    }

    /// Returns the importer matching a file path's extension, or `None` for
    /// kinds that are not yet importable (`ResourceKind::Other`).
    pub fn from_path(&self, path: &Path) -> Option<&'static dyn Importer> {
        self.get(ResourceKind::from_path(path))
    }
}

/// Result of importing one item within a batch.
#[derive(Clone, Debug)]
pub struct ImportItemResult {
    /// Absolute path of the source that was imported.
    pub source: PathBuf,
    /// True when the cache hit and no new artifact was written.
    pub reused: bool,
    /// Cache artifact path when the item succeeded, project-root relative.
    pub cache_path: Option<PathBuf>,
    /// Error message when the item failed, otherwise `None`.
    pub error: Option<String>,
}

/// Aggregate counts for a batch run; one bad item never aborts the batch.
#[derive(Clone, Debug, Default)]
pub struct ImportSummary {
    pub total: usize,
    pub imported: usize,
    pub reused: usize,
    pub failed: usize,
    pub skipped: usize,
    /// Wall-clock time of the whole batch in milliseconds.
    pub elapsed_ms: u64,
    /// Per-item results, in the same order as `items`.
    pub results: Vec<ImportItemResult>,
}

/// Runs a batch of imports, writing per-item state back to the database.
///
/// Items are imported one at a time; a failing item is recorded via
/// `mark_failed` and the batch continues. `cancel` is checked between items so
/// a caller can stop the batch cleanly. The database is saved once at the end.
pub fn run_import_batch(
    project: &Path,
    db: &mut ResourceDatabase,
    cache: &mut ImportCache,
    registry: &ImporterRegistry,
    items: &[(AssetId, PathBuf)],
    cancel: &AtomicBool,
) -> ImportSummary {
    run_import_batch_with_progress(project, db, cache, registry, items, cancel, |_, _| {})
}

/// Runs a batch of imports with a per-item progress callback.
///
/// Same behavior as [`run_import_batch`], but `on_progress(processed, total)`
/// is called after each item so a caller can drive a determinate progress bar
/// from real work. `processed` is the number of items already examined
/// (including the current one); `total` is the batch size.
pub fn run_import_batch_with_progress<F>(
    _project: &Path,
    db: &mut ResourceDatabase,
    cache: &mut ImportCache,
    registry: &ImporterRegistry,
    items: &[(AssetId, PathBuf)],
    cancel: &AtomicBool,
    mut on_progress: F,
) -> ImportSummary
where
    F: FnMut(usize, usize),
{
    let started = Instant::now();
    let mut summary = ImportSummary {
        total: items.len(),
        ..Default::default()
    };
    let total = items.len();
    for (index, (id, source)) in items.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(importer) = registry.from_path(source) else {
            summary.skipped += 1;
            summary.results.push(ImportItemResult {
                source: source.clone(),
                reused: false,
                cache_path: None,
                error: Some("不支持的资源类型".to_owned()),
            });
            continue;
        };
        match cache.import_file(*id, source, importer) {
            Ok(report) => {
                if report.reused {
                    summary.reused += 1;
                } else {
                    summary.imported += 1;
                }
                let cache_path = report.cache_path.clone();
                let hash = report.fingerprint.clone();
                match db.mark_ready(*id, cache_path, hash) {
                    Ok(()) => summary.results.push(ImportItemResult {
                        source: source.clone(),
                        reused: report.reused,
                        cache_path: Some(report.cache_path),
                        error: None,
                    }),
                    Err(error) => {
                        summary.failed += 1;
                        summary.results.push(ImportItemResult {
                            source: source.clone(),
                            reused: false,
                            cache_path: None,
                            error: Some(format!("{}: {error}", source.display())),
                        });
                    }
                }
            }
            Err(error) => {
                summary.failed += 1;
                let message = format!("{}: {error}", source.display());
                summary.results.push(ImportItemResult {
                    source: source.clone(),
                    reused: false,
                    cache_path: None,
                    error: Some(message.clone()),
                });
                if let Err(db_error) = db.mark_failed(*id, message) {
                    tracing::warn!("mark_failed({id}) 失败: {db_error}");
                }
            }
        }
        on_progress(index + 1, total);
    }
    summary.elapsed_ms = started.elapsed().as_millis() as u64;
    if let Err(error) = db.save() {
        tracing::warn!("导入后保存资源数据库失败: {error}");
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::resource_db::ResourceState;
    use std::{fs, io::Write};

    fn root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rustlua-importer-{name}-{}", std::process::id()))
    }

    fn write_source(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::File::create(&path).unwrap().write_all(bytes).unwrap();
        path
    }

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

    fn jpeg_bytes() -> Vec<u8> {
        use image::{Rgb, RgbImage};
        let mut img = RgbImage::new(1, 1);
        img.put_pixel(0, 0, Rgb([255, 0, 0]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Jpeg,
            )
            .expect("jpeg encode");
        bytes
    }

    /// 44-byte PCM WAV with one silent frame (RFC 2361 compatible header).
    fn wav_bytes() -> Vec<u8> {
        let mut bytes = vec![0x52, 0x49, 0x46, 0x46, 0x22, 0x00, 0x00, 0x00]; // RIFF, size 34
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&[0x10, 0x00, 0x00, 0x00]); // chunk size 16
        bytes.extend_from_slice(&[0x01, 0x00]); // PCM
        bytes.extend_from_slice(&[0x01, 0x00]); // mono
        bytes.extend_from_slice(&0xac44u32.to_le_bytes()); // 44100 Hz
        bytes.extend_from_slice(&88_200u32.to_le_bytes()); // byte rate
        bytes.extend_from_slice(&[0x02, 0x00]); // block align
        bytes.extend_from_slice(&[0x10, 0x00]); // 16 bit
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&[0x02, 0x00, 0x00, 0x00]); // data size 2
        bytes.extend_from_slice(&[0x00, 0x00]); // one silent frame
        bytes
    }

    fn scene_bytes() -> Vec<u8> {
        b"scene_version=1\nname=main\n".to_vec()
    }

    fn prefab_bytes() -> Vec<u8> {
        b"prefab_version=1\nasset_id=00000000-0000-4000-8000-000000000000\nname=hero\nroot=1\nentity\t1\troot\t\n"
            .to_vec()
    }

    #[test]
    fn registry_maps_every_builtin_kind() {
        let registry = ImporterRegistry::builtin();
        for kind in [
            ResourceKind::Texture,
            ResourceKind::Audio,
            ResourceKind::LuaScript,
            ResourceKind::Scene,
            ResourceKind::Prefab,
            ResourceKind::Model,
            ResourceKind::Atlas,
            ResourceKind::Font,
        ] {
            let importer = registry.get(kind).expect("builtin importer exists");
            assert_eq!(importer.kind(), kind);
            assert!(!importer.extensions().is_empty());
        }
        assert!(registry.get(ResourceKind::Other).is_none());
    }

    #[test]
    fn registry_resolves_by_path() {
        let registry = ImporterRegistry::builtin();
        assert_eq!(
            registry.from_path(Path::new("a.png")).unwrap().kind(),
            ResourceKind::Texture
        );
        assert_eq!(
            registry.from_path(Path::new("b.wav")).unwrap().kind(),
            ResourceKind::Audio
        );
        assert_eq!(
            registry.from_path(Path::new("c.prefab")).unwrap().kind(),
            ResourceKind::Prefab
        );
        assert!(registry.from_path(Path::new("d.bin")).is_none());
    }

    #[test]
    fn texture_importer_validates_png_and_jpeg() {
        let root = root("texture-good");
        let png = write_source(&root, "red.png", &red_png());
        let importer = ImporterRegistry::builtin()
            .get(ResourceKind::Texture)
            .unwrap();
        importer
            .validate(&png, &std::fs::read(&png).unwrap())
            .unwrap();
        let jpg = write_source(&root, "red.jpg", &jpeg_bytes());
        importer
            .validate(&jpg, &std::fs::read(&jpg).unwrap())
            .unwrap();
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn texture_importer_rejects_bad_bytes() {
        let root = root("texture-bad");
        let path = write_source(&root, "bad.png", b"not an image");
        let importer = ImporterRegistry::builtin()
            .get(ResourceKind::Texture)
            .unwrap();
        assert!(importer.validate(&path, b"not an image").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn audio_importer_validates_wav_and_rejects_bad_bytes() {
        let root = root("audio");
        let good = write_source(&root, "ok.wav", &wav_bytes());
        let importer = ImporterRegistry::builtin()
            .get(ResourceKind::Audio)
            .unwrap();
        importer
            .validate(&good, &std::fs::read(&good).unwrap())
            .unwrap();
        let bad = write_source(&root, "bad.wav", b"not audio");
        assert!(importer.validate(&bad, b"not audio").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn audio_importer_validates_ogg_streams() {
        let root = root("audio-ogg");
        let Some(bytes) = fixture("../../../game_showcase/assets/audio/attack.ogg") else {
            return;
        };
        let ogg = write_source(&root, "attack.ogg", &bytes);
        let importer = ImporterRegistry::builtin()
            .get(ResourceKind::Audio)
            .unwrap();
        importer
            .validate(&ogg, &std::fs::read(&ogg).unwrap())
            .unwrap();
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn font_importer_validates_ttf_and_rejects_bad_bytes() {
        let root = root("font");
        let Some(bytes) = fixture("../../../game_showcase/assets/fonts/NotoSansSC-VF.ttf") else {
            return;
        };
        let good = write_source(&root, "NotoSansSC-VF.ttf", &bytes);
        let importer = ImporterRegistry::builtin().get(ResourceKind::Font).unwrap();
        importer
            .validate(&good, &std::fs::read(&good).unwrap())
            .unwrap();
        let bad = write_source(&root, "bad.ttf", b"not a font");
        assert!(importer.validate(&bad, b"not a font").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn model_importer_validates_glb_and_rejects_bad_bytes() {
        let root = root("model");
        let Some(bytes) = fixture("../../../game_showcase/assets/models/m8-test-triangle.glb")
        else {
            return;
        };
        let good = write_source(&root, "m8-test-triangle.glb", &bytes);
        let importer = ImporterRegistry::builtin()
            .get(ResourceKind::Model)
            .unwrap();
        importer
            .validate(&good, &std::fs::read(&good).unwrap())
            .unwrap();
        let bad = write_source(&root, "bad.glb", b"invalid glb");
        assert!(importer.validate(&bad, b"invalid glb").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scene_importer_is_strict_for_scene_but_lenient_for_ron_json() {
        let root = root("scene");
        let good = write_source(&root, "main.scene", &scene_bytes());
        let importer = ImporterRegistry::builtin()
            .get(ResourceKind::Scene)
            .unwrap();
        importer
            .validate(&good, &std::fs::read(&good).unwrap())
            .unwrap();
        let bad_scene = write_source(&root, "bad.scene", b"scene_version=1\nname\nbad\n");
        assert!(
            importer
                .validate(&bad_scene, b"scene_version=1\nname\nbad\n")
                .is_err()
        );
        let ron = write_source(&root, "data.ron", b"Scene(version=1)");
        importer.validate(&ron, b"Scene(version=1)").unwrap();
        let json = write_source(&root, "data.json", b"{\"a\":1}");
        importer.validate(&json, b"{\"a\":1}").unwrap();
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn prefab_importer_validates_prefab() {
        let root = root("prefab");
        let good = write_source(&root, "hero.prefab", &prefab_bytes());
        let importer = ImporterRegistry::builtin()
            .get(ResourceKind::Prefab)
            .unwrap();
        importer
            .validate(&good, &std::fs::read(&good).unwrap())
            .unwrap();
        let bad = write_source(&root, "bad.prefab", b"prefab_version=1\nname\nbad\n");
        assert!(
            importer
                .validate(&bad, b"prefab_version=1\nname\nbad\n")
                .is_err()
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_sources_are_rejected_by_every_builtin() {
        let registry = ImporterRegistry::builtin();
        for kind in [
            ResourceKind::Texture,
            ResourceKind::Audio,
            ResourceKind::LuaScript,
            ResourceKind::Scene,
            ResourceKind::Prefab,
            ResourceKind::Model,
            ResourceKind::Atlas,
            ResourceKind::Font,
        ] {
            let importer = registry.get(kind).unwrap();
            assert!(
                importer.validate(Path::new("empty.bin"), b"").is_err(),
                "kind {kind:?} must reject empty sources"
            );
        }
    }

    #[test]
    fn default_produce_copies_raw_bytes_into_single_artifact() {
        let root = root("produce");
        let path = write_source(&root, "a.lua", b"print('x')");
        let bytes = std::fs::read(&path).unwrap();
        let id = AssetId::from_seed("a");
        let hash = crate::core::hash::content_hash(&bytes);
        let importer = ImporterRegistry::builtin()
            .get(ResourceKind::LuaScript)
            .unwrap();
        let artifacts = importer.produce(&bytes, id, &hash).unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].bytes, bytes);
        assert!(
            artifacts[0]
                .path
                .to_string_lossy()
                .contains(&id.to_string())
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn batch_imports_counts_and_does_not_abort_on_bad_item() {
        let root = root("batch");
        let png = write_source(&root, "good.png", &red_png());
        let bad = write_source(&root, "bad.png", b"not an image");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let good_id = db
            .register("good.png", ResourceKind::Texture, "texture-1")
            .unwrap();
        let bad_id = db
            .register("bad.png", ResourceKind::Texture, "texture-1")
            .unwrap();
        let mut cache = ImportCache::open(&root).unwrap();
        let registry = ImporterRegistry::builtin();
        let cancel = AtomicBool::new(false);
        let items = vec![(good_id, png.clone()), (bad_id, bad)];
        let first = run_import_batch(&root, &mut db, &mut cache, registry, &items, &cancel);
        assert_eq!(first.total, 2);
        assert_eq!(first.imported, 1);
        assert_eq!(first.failed, 1);
        assert_eq!(first.skipped, 0);
        assert_eq!(db.get(good_id).unwrap().status, ResourceState::Ready);
        assert_eq!(db.get(bad_id).unwrap().status, ResourceState::ImportFailed);
        assert!(!db.get(good_id).unwrap().content_hash.is_empty());
        let second = run_import_batch(&root, &mut db, &mut cache, registry, &items, &cancel);
        assert_eq!(second.reused, 1);
        assert_eq!(second.imported, 0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn batch_respects_cancel_flag() {
        let root = root("cancel");
        let path = write_source(&root, "a.png", &red_png());
        let mut db = ResourceDatabase::open(&root).unwrap();
        let id = db
            .register("a.png", ResourceKind::Texture, "texture-1")
            .unwrap();
        let mut cache = ImportCache::open(&root).unwrap();
        let registry = ImporterRegistry::builtin();
        let cancel = AtomicBool::new(true);
        let items = vec![(id, path)];
        let summary = run_import_batch(&root, &mut db, &mut cache, registry, &items, &cancel);
        assert_eq!(summary.total, 1);
        assert!(summary.results.is_empty(), "cancelled before any import");
        assert_eq!(db.get(id).unwrap().status, ResourceState::Discovered);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn batch_skips_unsupported_kinds() {
        let root = root("skip");
        let path = write_source(&root, "notes.txt", b"hello");
        let mut db = ResourceDatabase::open(&root).unwrap();
        let id = db
            .register("notes.txt", ResourceKind::Other, "none")
            .unwrap();
        let mut cache = ImportCache::open(&root).unwrap();
        let registry = ImporterRegistry::builtin();
        let cancel = AtomicBool::new(false);
        let items = vec![(id, path)];
        let summary = run_import_batch(&root, &mut db, &mut cache, registry, &items, &cancel);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.total, 1);
        assert_eq!(db.get(id).unwrap().status, ResourceState::Discovered);
        let _ = fs::remove_dir_all(&root);
    }
}
