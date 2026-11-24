//! 十一个内置导入器（assets 层）+ 注册表组装。
//!
//! 每个 [`ResourceKind`] 恰好一个内置导入器：`Texture`（`image` 解码）、`Audio`（`rodio`
//! 解码 + WAV 读头）、`LuaScript`（`mlua` 只编译不执行）、`Model`（`gltf`）、`Atlas`
//! （纯文本）、`Font`（`ab_glyph`）、`Environment`（HDR 解码 / `.rlenv` 校验）、`Material`
//! （`.material` 文本，用运行时同一台解析器 `rustlua_assets::mat_asset::MaterialAsset`）、
//! `Animation`（`.anim` 文本，用 `rustlua_animation::anim_asset::AnimClip::from_text`）、
//! `ShaderGraph`（`.sgraph` JSON，用 `rustlua_visual_script::shader_graph::ShaderGraph`）。
//! Scene/Prefab 两个导入器在 `rustlua_scene::asset_import`
//! 里（它们的校验要用 `Scene`/`Prefab` 类型），由本 crate 的
//! [`crate::full_registry`] 装配成完整注册表。
//!
//! **为什么这几个实现不放在运行时 crate**：`image`/`rodio`/`ab_glyph`/`mlua` 只在导入期
//! 用得上，而运行时 crate（`rustlua-assets`）是浏览器也要编的。拆分前实测
//! `cargo tree -p game_web --target wasm32 -i rodio` 的唯一入口就是这里。

use std::path::{Component, Path, PathBuf};

use rustlua_animation::anim_asset::AnimClip;
use rustlua_assets::atlas_format::AtlasDefinition;
use rustlua_assets::env_asset::EnvironmentAsset;
use rustlua_assets::gltf_asset::GltfAsset;
use rustlua_assets::importer::{Importer, ImporterRegistry, reject_bom, reject_empty};
use rustlua_assets::mat_asset::MaterialAsset;
use rustlua_assets::resource_kind::ResourceKind;
use rustlua_visual_script::shader_graph::ShaderGraph;

struct TextureImporter;
struct AudioImporter;
struct ScriptImporter;
struct ModelImporter;
struct AtlasImporter;
struct FontImporter;
struct EnvironmentImporter;
struct MaterialImporter;
struct AnimImporter;
struct ShaderGraphImporter;
struct SpineImporter;

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

impl Importer for EnvironmentImporter {
    fn version(&self) -> &'static str {
        "environment-1"
    }
    fn kind(&self) -> ResourceKind {
        ResourceKind::Environment
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["rlenv", "hdr"]
    }
    fn validate(&self, source: &Path, bytes: &[u8]) -> Result<(), String> {
        reject_empty(source, bytes)?;
        let extension = source
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if extension.eq_ignore_ascii_case("rlenv") {
            EnvironmentAsset::decode(bytes)
                .map_err(|error| format!("{}: 环境资产解析失败: {error}", source.display()))?;
            return Ok(());
        }
        crate::environment::decode_hdr(bytes)
            .map(|_| ())
            .map_err(|error| format!("{}: HDR 解码失败: {error}", source.display()))
    }
}

impl Importer for MaterialImporter {
    fn version(&self) -> &'static str {
        "material-1"
    }
    fn kind(&self) -> ResourceKind {
        ResourceKind::Material
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["material"]
    }
    fn validate(&self, source: &Path, bytes: &[u8]) -> Result<(), String> {
        reject_empty(source, bytes)?;
        reject_bom(source, bytes, "材质定义")?;
        let text = std::str::from_utf8(bytes)
            .map_err(|error| format!("{}: 材质编码非法（需 UTF-8）: {error}", source.display()))?;
        // 用运行时的同一台解析器（`rustlua-assets::mat_asset`）：渲染器读到的字段与导入期
        // 校验的字段必须是同一份规则，否则资源浏览器说"导入成功"而物体在场景里悄悄坏掉。
        MaterialAsset::from_text(source, text).map_err(|error| error.to_string())?;
        Ok(())
    }
}

impl Importer for ShaderGraphImporter {
    fn version(&self) -> &'static str {
        "shader-graph-1"
    }
    fn kind(&self) -> ResourceKind {
        ResourceKind::ShaderGraph
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["sgraph"]
    }
    fn validate(&self, source: &Path, bytes: &[u8]) -> Result<(), String> {
        reject_empty(source, bytes)?;
        reject_bom(source, bytes, "Shader Graph 定义")?;
        let text = std::str::from_utf8(bytes).map_err(|error| {
            format!(
                "{}: Shader Graph 编码非法（需 UTF-8）: {error}",
                source.display()
            )
        })?;
        ShaderGraph::from_json(text)
            .map(|_| ())
            .map_err(|error| format!("{}: Shader Graph 解析失败: {error}", source.display()))
    }
}

impl Importer for AnimImporter {
    fn version(&self) -> &'static str {
        "anim-1"
    }
    fn kind(&self) -> ResourceKind {
        ResourceKind::Animation
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["anim"]
    }
    fn validate(&self, source: &Path, bytes: &[u8]) -> Result<(), String> {
        reject_empty(source, bytes)?;
        reject_bom(source, bytes, "动画片段")?;
        let text = std::str::from_utf8(bytes).map_err(|error| {
            format!(
                "{}: 动画片段编码非法（需 UTF-8）: {error}",
                source.display()
            )
        })?;
        // 与运行时共用 `AnimClip::from_text`：导入期放过的文件，运行时也必须读得出来。
        AnimClip::from_text(source, text).map_err(|error| error.to_string())?;
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

impl Importer for SpineImporter {
    fn version(&self) -> &'static str {
        "spine-1"
    }
    fn kind(&self) -> ResourceKind {
        ResourceKind::Spine
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["spine", "skel", "spineatlas"]
    }
    fn validate(&self, source: &Path, bytes: &[u8]) -> Result<(), String> {
        reject_empty(source, bytes)?;
        let extension = source
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if extension.eq_ignore_ascii_case("spine") {
            // `.spine` 清单：版本冻结在 4.3.x，解析器与运行时同一份（`SpineManifest::parse`）。
            reject_bom(source, bytes, "Spine 清单")?;
            let text = std::str::from_utf8(bytes).map_err(|error| {
                format!(
                    "{}: Spine 清单编码非法（需 UTF-8）: {error}",
                    source.display()
                )
            })?;
            rustlua_spine::SpineManifest::parse(text).map_err(|error| error.to_string())?;
            return Ok(());
        }
        if extension.eq_ignore_ascii_case("spineatlas") {
            reject_bom(source, bytes, "Spine 图集")?;
            let text = std::str::from_utf8(bytes).map_err(|error| {
                format!(
                    "{}: Spine 图集编码非法（需 UTF-8）: {error}",
                    source.display()
                )
            })?;
            // 用运行时的同一台解析器（经 `rustlua-spine` 的中性入口）：导入期放过的图集，
            // 运行时也必须读得出来。导入层不直接依赖 `spine2d`，backend 隔离不外泄。
            rustlua_spine::validate::atlas_text(text)
                .map_err(|error| format!("{}: {error}", source.display()))?;
            return Ok(());
        }
        // `.skel`：二进制骨骼。走运行时同一个 `from_skel_bytes`，脊柱数据格式不匹配会在
        // 导入期就报出来，而不是等玩家进游戏才白屏。
        rustlua_spine::validate::skeleton_bytes(bytes)
            .map_err(|error| format!("{}: {error}", source.display()))?;
        // M84 P2-①：clipping 附件当前不实现（静默放行 = "该裁没裁"的画面错误）。
        // 这一步必须在解析成功之后——解析器本身是支持 clipping 的，错的是渲染管线。
        rustlua_spine::validate::reject_unsupported_features(bytes, true)
            .map_err(|error| format!("{}: {error}", source.display()))
    }

    fn validate_project_context(
        &self,
        project_root: &Path,
        source: &Path,
        bytes: &[u8],
    ) -> Result<(), String> {
        if !source
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("spine"))
        {
            return Ok(());
        }
        validate_spine_manifest_dependencies(project_root, source, bytes)
    }
}

/// 以 `.spine` 清单为上下文校验 JSON skeleton；单独的 `.json` 仍保留通用 JSON/Scene 分类。
fn validate_spine_manifest_dependencies(
    project_root: &Path,
    source: &Path,
    bytes: &[u8],
) -> Result<(), String> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        format!(
            "{}: Spine 清单编码非法（需 UTF-8）: {error}",
            source.display()
        )
    })?;
    let manifest = rustlua_spine::SpineManifest::parse(text)
        .map_err(|error| format!("{}: {error}", source.display()))?;

    let skeleton_path =
        resolve_spine_project_file(project_root, source, "skeleton", &manifest.skeleton)?;
    let skeleton_extension = skeleton_path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    let is_binary = if skeleton_extension.eq_ignore_ascii_case("skel") {
        true
    } else if skeleton_extension.eq_ignore_ascii_case("json") {
        false
    } else {
        return Err(format!(
            "{}: skeleton 仅支持 .skel 或 Spine JSON：{}",
            source.display(),
            manifest.skeleton
        ));
    };
    let skeleton_bytes = std::fs::read(&skeleton_path).map_err(|error| {
        format!(
            "{}: 无法读取 skeleton {}：{error}",
            source.display(),
            skeleton_path.display()
        )
    })?;
    if is_binary {
        rustlua_spine::validate::skeleton_bytes(&skeleton_bytes)
    } else {
        rustlua_spine::validate::skeleton_json_bytes(&skeleton_bytes)
    }
    .map_err(|error| {
        format!(
            "{}: Spine skeleton {} 无效：{error}",
            source.display(),
            manifest.skeleton
        )
    })?;
    rustlua_spine::validate::reject_unsupported_features(&skeleton_bytes, is_binary).map_err(
        |error| {
            format!(
                "{}: Spine skeleton {}：{error}",
                source.display(),
                manifest.skeleton
            )
        },
    )?;

    let atlas_path = resolve_spine_project_file(project_root, source, "atlas", &manifest.atlas)?;
    let atlas_bytes = std::fs::read(&atlas_path).map_err(|error| {
        format!(
            "{}: 无法读取 atlas {}：{error}",
            source.display(),
            atlas_path.display()
        )
    })?;
    let atlas_text = std::str::from_utf8(&atlas_bytes).map_err(|error| {
        format!(
            "{}: Spine atlas {} 编码非法（需 UTF-8）：{error}",
            source.display(),
            manifest.atlas
        )
    })?;
    rustlua_spine::validate::atlas_text(atlas_text).map_err(|error| {
        format!(
            "{}: Spine atlas {} 无效：{error}",
            source.display(),
            manifest.atlas
        )
    })?;

    for page in &manifest.pages {
        resolve_spine_project_file(project_root, source, "page", page)?;
    }
    Ok(())
}

fn resolve_spine_project_file(
    project_root: &Path,
    manifest_source: &Path,
    label: &str,
    value: &str,
) -> Result<PathBuf, String> {
    let relative = safe_project_relative_path(value).ok_or_else(|| {
        format!(
            "{}: {label} 必须是工程内相对路径（不得含绝对路径或 ..）：{value}",
            manifest_source.display()
        )
    })?;
    let canonical_root = std::fs::canonicalize(project_root).map_err(|error| {
        format!(
            "{}: 无法解析工程根目录 {}：{error}",
            manifest_source.display(),
            project_root.display()
        )
    })?;
    let canonical_file =
        std::fs::canonicalize(canonical_root.join(&relative)).map_err(|error| {
            format!(
                "{}: 无法解析 {label} 引用的资源 {}：{error}",
                manifest_source.display(),
                relative.display()
            )
        })?;
    if !canonical_file.starts_with(&canonical_root) || !canonical_file.is_file() {
        return Err(format!(
            "{}: {label} 引用必须是工程内文件：{}",
            manifest_source.display(),
            relative.display()
        ));
    }
    Ok(canonical_file)
}

fn safe_project_relative_path(value: &str) -> Option<PathBuf> {
    let normalized = value.replace('\\', "/");
    let path = Path::new(&normalized);
    if normalized.trim().is_empty()
        || normalized.contains(':')
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return None;
    }
    Some(path.to_path_buf())
}

/// 进程级的十个内置导入器注册表（**不含** Scene/Prefab）。
///
/// 只导入普通资源（纹理/音频/脚本/模型/图集/字体/环境/材质/动画）时用它；要导入 `.scene` / `.prefab`
/// 必须用 [`crate::full_registry`]，否则那两类会报"不支持的资源类型"。
#[must_use]
pub fn builtin() -> &'static ImporterRegistry {
    use std::sync::OnceLock;
    static REGISTRY: OnceLock<ImporterRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| ImporterRegistry::from_importers(&core_importers()))
}

/// 十一个核心导入器，顺序固定（注册表按 kind 建索引，顺序只影响可读性）。
#[must_use]
pub fn core_importers() -> [&'static dyn Importer; 11] {
    [
        &TextureImporter,
        &AudioImporter,
        &ScriptImporter,
        &ModelImporter,
        &AtlasImporter,
        &FontImporter,
        &EnvironmentImporter,
        &MaterialImporter,
        &AnimImporter,
        &ShaderGraphImporter,
        &SpineImporter,
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
            ResourceKind::Environment,
            ResourceKind::Material,
            ResourceKind::Animation,
            ResourceKind::ShaderGraph,
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
    fn shader_graph_importer_validates_json_and_rejects_bom() {
        let root = root("shader-graph");
        let importer = builtin().get(ResourceKind::ShaderGraph).unwrap();
        let graph = ShaderGraph {
            version: rustlua_visual_script::shader_graph::SHADER_GRAPH_VERSION,
            name: "smoke".into(),
            nodes: vec![rustlua_visual_script::shader_graph::ShaderNode {
                id: "out".into(),
                kind: rustlua_visual_script::shader_graph::ShaderNodeKind::Output,
                position: [0.0, 0.0],
                params: Default::default(),
            }],
            edges: vec![],
        };
        let path = write_source(
            &root,
            "assets/shaders/smoke.sgraph",
            graph.to_json().as_bytes(),
        );
        importer
            .validate(&path, &fs::read(&path).unwrap())
            .expect("valid sgraph");
        let bad = write_source(&root, "assets/shaders/bad.sgraph", b"{\"version\":1}");
        assert!(importer.validate(&bad, &fs::read(&bad).unwrap()).is_err());
        let bom = write_source(&root, "assets/shaders/bom.sgraph", b"\xEF\xBB\xBF{}");
        let error = importer
            .validate(&bom, &fs::read(&bom).unwrap())
            .expect_err("BOM");
        assert!(error.contains("BOM"));
        let _ = fs::remove_dir_all(&root);
    }

    /// P3-R02 补漏：`.material` 以前只有 `ResourceKind`、没有导入器，于是示例项目里每一份
    /// 材质都报「不支持的资源类型」。这条门钉住补上后的三件事：合法材质通过、字段/版本/贴图
    /// 路径非法的**在导入期**就报错（诊断含路径与行号，用的是运行时那台解析器）、BOM 被拦。
    #[test]
    fn material_importer_validates_with_the_runtime_parser() {
        let root = root("material");
        let importer = builtin().get(ResourceKind::Material).unwrap();

        let good = write_source(
            &root,
            "assets/materials/rusted.material",
            b"version=1\nname=rusted\nmetallic=0.9\nroughness=0.2\nbase_color_texture=assets/textures/rust.png\n",
        );
        importer
            .validate(&good, &fs::read(&good).unwrap())
            .expect("合法材质必须通过导入校验");

        for (bytes, expected) in [
            ("version=1\nmetallic=banana\n", "not a number"),
            ("metallic=0.5\n", "missing required `version=1`"),
            ("version=2\nmetallic=0.5\n", "unsupported version"),
            ("version=1\nfoo=1\n", "unknown field"),
            (
                "version=1\nbase_color_texture=../escape.png\n",
                "invalid texture path",
            ),
        ] {
            let bad = write_source(&root, "assets/materials/bad.material", bytes.as_bytes());
            let error = importer
                .validate(&bad, &fs::read(&bad).unwrap())
                .expect_err("非法材质必须在导入期报错");
            assert!(
                error.contains(expected),
                "源文本 `{bytes}`：诊断 `{error}` 应包含 `{expected}`"
            );
            assert!(
                error.contains("bad.material: line"),
                "诊断需含文件路径与行号: {error}"
            );
        }

        let bom = write_source(
            &root,
            "assets/materials/bom.material",
            b"\xEF\xBB\xBFversion=1\n",
        );
        let error = importer
            .validate(&bom, &fs::read(&bom).unwrap())
            .expect_err("bom material must be rejected");
        assert!(error.contains("BOM"), "诊断需含 BOM 提示: {error}");

        let _ = fs::remove_dir_all(&root);
    }

    /// P3-E01-4b：`.anim` 进入资源家族。导入器必须用运行时同一台解析器：合法片段通过，
    /// 坏字段/缺版本/坏轨道在导入期就带文件与行号报错，BOM 被拦。否则资源浏览器会登记一个
    /// 「导入成功」而 `AnimationPlayer` 在场景加载时读不出的片段。
    #[test]
    fn anim_importer_validates_with_the_runtime_parser() {
        let root = root("anim");
        let importer = builtin().get(ResourceKind::Animation).unwrap();

        let good = write_source(
            &root,
            "assets/anim/spin.anim",
            b"version=1\nname=spin\nduration=1\nlooped=false\ntrack=Transform.position;interp=linear;keys=0:0,0;1:4,0\n",
        );
        importer
            .validate(&good, &fs::read(&good).unwrap())
            .expect("合法动画片段必须通过导入校验");

        for (bytes, expected) in [
            ("version=1\nfoo=1\n", "unknown field"),
            ("name=spin\nduration=1\n", "missing `version=1`"),
            (
                "version=1\nname=spin\nduration=1\ntrack=Bogus.position;interp=linear;keys=0:0,0\n",
                "unsupported track path",
            ),
        ] {
            let bad = write_source(&root, "assets/anim/bad.anim", bytes.as_bytes());
            let error = importer
                .validate(&bad, &fs::read(&bad).unwrap())
                .expect_err("非法动画片段必须在导入期报错");
            assert!(
                error.contains(expected),
                "源文本 `{bytes}`：诊断 `{error}` 应包含 `{expected}`"
            );
            assert!(
                error.contains("bad.anim: line"),
                "诊断需含文件路径与行号: {error}"
            );
        }

        let bom = write_source(&root, "assets/anim/bom.anim", b"\xEF\xBB\xBFversion=1\n");
        let error = importer
            .validate(&bom, &fs::read(&bom).unwrap())
            .expect_err("bom anim must be rejected");
        assert!(error.contains("BOM"), "诊断需含 BOM 提示: {error}");

        let _ = fs::remove_dir_all(&root);
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
    fn spine_manifest_context_validates_json_skeleton_and_dependencies_on_cache_hits() {
        let root = root("spine-json-context");
        let _ = fs::remove_dir_all(&root);
        let manifest_path = write_source(
            &root,
            "assets/spine/hero.spine",
            b"spine_version=4.3.75-beta\nskeleton=assets/spine/hero.json\natlas=assets/spine/hero.spineatlas\npage=assets/spine/page_a.png\npage=assets/spine/page_b.png\n",
        );
        write_source(
            &root,
            "assets/spine/hero.json",
            include_bytes!("../../rustlua-spine/tests/fixtures/no_clipping_sample.json"),
        );
        write_source(
            &root,
            "assets/spine/hero.spineatlas",
            include_bytes!("../../rustlua-spine/tests/fixtures/two_page.atlas"),
        );
        write_source(&root, "assets/spine/page_a.png", b"atlas page fixture");
        write_source(&root, "assets/spine/page_b.png", b"atlas page fixture");

        // Generic .json routing remains unchanged, while manifest context parses it as Spine JSON.
        assert_eq!(
            ResourceKind::from_path("assets/spine/hero.json"),
            ResourceKind::Scene
        );
        let registry = crate::full_registry();
        assert_eq!(
            registry
                .from_path(Path::new("assets/spine/hero.json"))
                .map(|importer| importer.kind()),
            Some(ResourceKind::Scene)
        );
        let manifest_importer = registry.get(ResourceKind::Spine).unwrap();
        let manifest_bytes = fs::read(&manifest_path).unwrap();
        manifest_importer
            .validate_project_context(&root, &manifest_path, &manifest_bytes)
            .expect("Spine JSON skeleton and referenced files validate");

        let mut db = ResourceDatabase::open(&root).unwrap();
        let id = db
            .register("assets/spine/hero.spine", ResourceKind::Spine, "spine-1")
            .unwrap();
        let mut cache = ImportCache::open(&root).unwrap();
        let cancel = AtomicBool::new(false);
        let items = vec![(id, manifest_path.clone())];
        let first = run_import_batch(&root, &mut db, &mut cache, &registry, &items, &cancel);
        assert_eq!(first.failed, 0, "{:?}", first.results);
        let cached = run_import_batch(&root, &mut db, &mut cache, &registry, &items, &cancel);
        assert_eq!(
            cached.reused, 1,
            "valid unchanged manifest should hit cache"
        );

        fs::remove_file(root.join("assets/spine/page_b.png")).unwrap();
        let missing_page = run_import_batch(&root, &mut db, &mut cache, &registry, &items, &cancel);
        assert_eq!(missing_page.failed, 1);
        assert!(
            missing_page.results[0]
                .error
                .as_deref()
                .is_some_and(|error| error.contains("page"))
        );

        let escaping_manifest = b"spine_version=4.3.75-beta\nskeleton=../outside.json\natlas=assets/spine/hero.spineatlas\npage=assets/spine/page_a.png\n";
        let error = manifest_importer
            .validate_project_context(&root, &manifest_path, escaping_manifest)
            .expect_err("manifest paths must not escape the project root");
        assert!(error.contains("工程内相对路径"), "{error}");

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

    /// P3-R03-4：导入 `.hdr` 必须顺带烘出运行时真正读取的同名 `.rlenv`。
    ///
    /// 组件串里作者写的是 `.hdr` 源文件，但 `RuntimeScene` 读的是 `.rlenv`；只有离线工具
    /// 会烘焙的话，设计者把 `.hdr` 丢进编辑器就会得到"读取 xxx.rlenv 失败"。
    #[test]
    fn batch_import_of_hdr_bakes_the_runtime_rlenv_sidecar() {
        let root = root("hdr-bake");
        let pixels = vec![[1.0_f32, 0.5, 0.25]; 8 * 4];
        let image = crate::environment::HdrImage::new(8, 4, pixels).expect("hdr image");
        let hdr = crate::environment::encode_radiance(&image);
        let source = write_source(&root, "assets/env/test.hdr", &hdr);
        let product = root.join("assets/env/test.rlenv");
        assert!(!product.exists(), "导入前不该已经存在产物");

        let mut db = ResourceDatabase::open(&root).unwrap();
        let id = db
            .register(
                "assets/env/test.hdr",
                ResourceKind::Environment,
                "environment-1",
            )
            .unwrap();
        let mut cache = ImportCache::open(&root).unwrap();
        let cancel = AtomicBool::new(false);
        let items = vec![(id, source.clone())];
        let summary = run_import_batch(&root, &mut db, &mut cache, builtin(), &items, &cancel);
        assert_eq!(summary.failed, 0, "{:?}", summary.results);
        assert_eq!(db.get(id).unwrap().status, ResourceState::Ready);

        let baked = fs::read(&product).expect("导入 .hdr 必须烘出同名 .rlenv");
        let asset = EnvironmentAsset::decode(&baked).expect("产物必须是合法 .rlenv");
        assert_eq!(asset.face_size, crate::environment::DEFAULT_FACE_SIZE);
        assert_eq!(asset.mip_count(), crate::environment::DEFAULT_MIP_COUNT);

        // 再跑一次（缓存命中也一样）：产物必须逐字节不变，不能被反复重写。
        let again = run_import_batch(&root, &mut db, &mut cache, builtin(), &items, &cancel);
        assert_eq!(again.failed, 0, "{:?}", again.results);
        assert_eq!(fs::read(&product).expect("second read"), baked);
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
