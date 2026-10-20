//! 六个内置导入器（assets 层）+ 注册表组装。
//!
//! 每个 [`ResourceKind`] 恰好一个内置导入器：`Texture`（`image` 解码）、`Audio`（`rodio`
//! 解码 + WAV 读头）、`LuaScript`（`mlua` 只编译不执行）、`Model`（`gltf`）、`Atlas`
//! （纯文本）、`Font`（`ab_glyph`）。Scene/Prefab 两个导入器在 `rustlua_scene::asset_import`
//! 里（它们的校验要用 `Scene`/`Prefab` 类型），由本 crate 的
//! [`crate::full_registry`] 装配成完整注册表。
//!
//! **为什么这几个实现不放在运行时 crate**：`image`/`rodio`/`ab_glyph`/`mlua` 只在导入期
//! 用得上，而运行时 crate（`rustlua-assets`）是浏览器也要编的。拆分前实测
//! `cargo tree -p game_web --target wasm32 -i rodio` 的唯一入口就是这里。

use std::path::Path;

use rustlua_assets::atlas_format::AtlasDefinition;
use rustlua_assets::gltf_asset::GltfAsset;
use rustlua_assets::importer::{Importer, ImporterRegistry, reject_bom, reject_empty};
use rustlua_assets::resource_kind::ResourceKind;

struct TextureImporter;
struct AudioImporter;
struct ScriptImporter;
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
        &["wav", "ogg", "mp3"]
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
        // WAV 头含数据长度，可强制读头；OGG/MP3 为流式格式，头部无总时长，解码器
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
        // M20-01：导入期 Lua 语法校验（只编译不执行）——错误含文件与行列定位，
        // 在编辑器资源扫描与发布前检查阶段即可拦下，而非进入运行时才失败。
        reject_empty(source, bytes)?;
        validate_lua_syntax(source, bytes)
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
        "atlas-2"
    }
    fn kind(&self) -> ResourceKind {
        ResourceKind::Atlas
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["atlas"]
    }
    fn validate(&self, source: &Path, bytes: &[u8]) -> Result<(), String> {
        reject_empty(source, bytes)?;
        reject_bom(source, bytes, "atlas 文件")?;
        let text = std::str::from_utf8(bytes).map_err(|error| {
            format!("{}: atlas 编码非法（需 UTF-8）: {error}", source.display())
        })?;
        AtlasDefinition::from_text(text)
            .map_err(|error| format!("{}: {error}", source.display()))?;
        Ok(())
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

/// 进程级的六个内置导入器注册表（**不含** Scene/Prefab）。
///
/// 只导入普通资源（纹理/音频/脚本/模型/图集/字体）时用它；要导入 `.scene` / `.prefab`
/// 必须用 [`crate::full_registry`]，否则那两类会报"不支持的资源类型"。
#[must_use]
pub fn builtin() -> &'static ImporterRegistry {
    use std::sync::OnceLock;
    static REGISTRY: OnceLock<ImporterRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| ImporterRegistry::from_importers(&core_importers()))
}

/// 六个核心导入器，顺序固定（注册表按 kind 建索引，顺序只影响可读性）。
#[must_use]
pub fn core_importers() -> [&'static dyn Importer; 6] {
    [
        &TextureImporter,
        &AudioImporter,
        &ScriptImporter,
        &ModelImporter,
        &AtlasImporter,
        &FontImporter,
    ]
}

/// M20-01：Lua 源文件语法校验（只编译不执行，不改变合法脚本的运行语义）。
///
/// 诊断包含文件路径与 Lua 报错定位（`[string "<path>"]:<line>: <message>`），
/// 因此在导入/资源扫描/发布前检查阶段即可拦下语法错误，而不是等运行时才炸。
fn validate_lua_syntax(source: &Path, bytes: &[u8]) -> Result<(), String> {
    reject_bom(source, bytes, "Lua 源文件")?;
    let text = std::str::from_utf8(bytes).map_err(|error| {
        format!(
            "{}: Lua 源文件编码非法（需 UTF-8）: {error}",
            source.display()
        )
    })?;
    let lua = mlua::Lua::new();
    let chunk = lua.load(text).set_name(source.display().to_string());
    chunk
        .into_function()
        .map_err(|error| format!("{}: Lua 语法错误: {error}", source.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::run_import_batch;
    use crate::import_cache::ImportCache;
    use rustlua_assets::resource_db::{AssetId, ResourceDatabase, ResourceState};
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;
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

    #[test]
    fn registry_maps_every_builtin_kind() {
        let registry = builtin();
        for kind in [
            ResourceKind::Texture,
            ResourceKind::Audio,
            ResourceKind::LuaScript,
            ResourceKind::Model,
            ResourceKind::Atlas,
            ResourceKind::Font,
        ] {
            let importer = registry.get(kind).expect("builtin importer exists");
            assert_eq!(importer.kind(), kind);
            assert!(!importer.extensions().is_empty());
        }
        // core 拆分后 Scene/Prefab 导入器在 rustlua-scene（`asset_import`），builtin 不含它们。
        assert!(registry.get(ResourceKind::Scene).is_none());
        assert!(registry.get(ResourceKind::Prefab).is_none());
        assert!(registry.get(ResourceKind::Other).is_none());
    }

    #[test]
    fn script_importer_validates_lua_syntax_with_located_errors() {
        let root = root("script-syntax");
        let importer = builtin().get(ResourceKind::LuaScript).unwrap();

        let good = write_source(
            &root,
            "good.lua",
            b"local state = require(\"state\")\nfunction update() return state.mode end\n",
        );
        importer
            .validate(&good, &std::fs::read(&good).unwrap())
            .unwrap();

        // 语法错误：定位必须包含文件路径与行号（EOF 处报第 3 行缺少 `end`）。
        let bad = write_source(&root, "bad.lua", b"function update()\n  return 1\n");
        let error = importer
            .validate(&bad, &std::fs::read(&bad).unwrap())
            .expect_err("syntax error must be rejected");
        assert!(error.contains("bad.lua"), "诊断需含文件路径: {error}");
        assert!(error.contains("语法错误"), "诊断需标明语法错误: {error}");
        assert!(error.contains(":3:"), "诊断需含行号: {error}");

        // 非 UTF-8 源文件：给出编码诊断而不是进入运行时才失败。
        let binary = write_source(&root, "binary.lua", &[0xff, 0xfe, 0x00]);
        let error = importer
            .validate(&binary, &std::fs::read(&binary).unwrap())
            .expect_err("non-utf8 lua must be rejected");
        assert!(error.contains("编码非法"), "诊断需标明编码问题: {error}");

        // UTF-8 BOM：给出可操作诊断（Lua 自身只会报乱码符号）。
        let bom = write_source(&root, "bom.lua", b"\xEF\xBB\xBFreturn 1\n");
        let error = importer
            .validate(&bom, &std::fs::read(&bom).unwrap())
            .expect_err("bom lua must be rejected with actionable message");
        assert!(
            error.contains("BOM") && error.contains("无 BOM"),
            "诊断需含可操作的 BOM 提示: {error}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn registry_resolves_by_path() {
        let registry = builtin();
        assert_eq!(
            registry.from_path(Path::new("a.png")).unwrap().kind(),
            ResourceKind::Texture
        );
        assert_eq!(
            registry.from_path(Path::new("b.wav")).unwrap().kind(),
            ResourceKind::Audio
        );
        // core 拆分后 Scene/Prefab 导入器在 rustlua-scene（`asset_import`），builtin 不解析它们。
        assert!(registry.from_path(Path::new("c.prefab")).is_none());
        assert!(registry.from_path(Path::new("s.scene")).is_none());
        assert!(registry.from_path(Path::new("d.bin")).is_none());
    }

    #[test]
    fn texture_importer_validates_png_and_jpeg() {
        let root = root("texture-good");
        let png = write_source(&root, "red.png", &red_png());
        let importer = builtin().get(ResourceKind::Texture).unwrap();
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
        let importer = builtin().get(ResourceKind::Texture).unwrap();
        assert!(importer.validate(&path, b"not an image").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn atlas_importer_validates_text_format() {
        let root = root("atlas-format");
        let importer = builtin().get(ResourceKind::Atlas).unwrap();
        let good = write_source(
            &root,
            "hero.atlas",
            b"atlas_version=1\ntexture=sprites/hero.png\nregion\tidle\t0\t0\t64\t64\n",
        );
        importer
            .validate(&good, &fs::read(&good).unwrap())
            .expect("合法 .atlas 必须通过");

        // 语法损坏必须定位到文件路径与行号。
        let bad = write_source(
            &root,
            "bad.atlas",
            b"atlas_version=1\ntexture=a.png\nregion\tx\t0\t0\t0\t8\n",
        );
        let error = importer
            .validate(&bad, &fs::read(&bad).unwrap())
            .expect_err("零尺寸 region 必须拒绝");
        assert!(error.contains("bad.atlas"), "诊断需含文件路径: {error}");
        assert!(error.contains("第 3 行"), "诊断需含行号: {error}");

        // 占位期语义（任意非空即通过）必须不再放行。
        let junk = write_source(&root, "junk.atlas", b"not an atlas");
        assert!(importer.validate(&junk, b"not an atlas").is_err());
        assert!(importer.validate(&junk, b"").is_err());

        // BOM 与 Lua/JSON 同一约定：给出可操作诊断。
        let bom = write_source(
            &root,
            "bom.atlas",
            b"\xEF\xBB\xBFatlas_version=1\ntexture=a.png\nregion\ta\t0\t0\t1\t1\n",
        );
        let error = importer
            .validate(&bom, &fs::read(&bom).unwrap())
            .expect_err("BOM 必须拒绝");
        assert!(error.contains("BOM"), "诊断需含 BOM 提示: {error}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn audio_importer_validates_wav_and_rejects_bad_bytes() {
        let root = root("audio");
        let good = write_source(&root, "ok.wav", &wav_bytes());
        let importer = builtin().get(ResourceKind::Audio).unwrap();
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
        let Some(bytes) = fixture("../../../examples/assets/audio/attack.ogg") else {
            return;
        };
        let ogg = write_source(&root, "attack.ogg", &bytes);
        let importer = builtin().get(ResourceKind::Audio).unwrap();
        importer
            .validate(&ogg, &std::fs::read(&ogg).unwrap())
            .unwrap();
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn audio_importer_validates_mp3_streams() {
        let root = root("audio-mp3");
        let Some(bytes) = fixture("../../../examples/game_jhyt/assets/audio/sfx_hit.mp3") else {
            return;
        };
        let mp3 = write_source(&root, "sfx_hit.mp3", &bytes);
        let importer = builtin().get(ResourceKind::Audio).unwrap();
        importer
            .validate(&mp3, &std::fs::read(&mp3).unwrap())
            .unwrap();
        let bad = write_source(&root, "bad.mp3", b"not audio");
        assert!(importer.validate(&bad, b"not audio").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn font_importer_validates_ttf_and_rejects_bad_bytes() {
        let root = root("font");
        let Some(bytes) = fixture("../../../examples/assets/fonts/NotoSansSC-VF.ttf") else {
            return;
        };
        let good = write_source(&root, "NotoSansSC-VF.ttf", &bytes);
        let importer = builtin().get(ResourceKind::Font).unwrap();
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
        let Some(bytes) = fixture("../../../examples/assets/models/m8-test-triangle.glb") else {
            return;
        };
        let good = write_source(&root, "m8-test-triangle.glb", &bytes);
        let importer = builtin().get(ResourceKind::Model).unwrap();
        importer
            .validate(&good, &std::fs::read(&good).unwrap())
            .unwrap();
        let bad = write_source(&root, "bad.glb", b"invalid glb");
        assert!(importer.validate(&bad, b"invalid glb").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_sources_are_rejected_by_every_builtin() {
        let registry = builtin();
        for kind in [
            ResourceKind::Texture,
            ResourceKind::Audio,
            ResourceKind::LuaScript,
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
        let hash = rustlua_assets::hash::content_hash(&bytes);
        let importer = builtin().get(ResourceKind::LuaScript).unwrap();
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
        let registry = builtin();
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
        let registry = builtin();
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
        let registry = builtin();
        let cancel = AtomicBool::new(false);
        let items = vec![(id, path)];
        let summary = run_import_batch(&root, &mut db, &mut cache, registry, &items, &cancel);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.total, 1);
        assert_eq!(db.get(id).unwrap().status, ResourceState::Discovered);
        let _ = fs::remove_dir_all(&root);
    }
}
