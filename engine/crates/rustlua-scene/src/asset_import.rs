//! Scene / Prefab importers（M12-01 集成）。
//!
//! `rustlua-assets` 不能依赖 `rustlua-scene`（其 importer 注册表刻意不含场景数据），
//! 因此 Scene/Prefab 导入器放在本模块，只提供**零件**：`scene_importers()`。
//!
//! **装配点不在本 crate**（2026-10-20 移动）：完整注册表由
//! `rustlua_assets_import::full_registry()` 组装。原因是六个核心导入器（纹理/音频/脚本/
//! 模型/图集/字体）要 `image`/`rodio`/`ab_glyph`/`mlua`，只该在 `assetc`/`editor` 里编译；
//! 而本 crate 是运行时也要编的，一旦它来装配，浏览器产物就会被拖进整套编解码器。
//! 现在的方向是 `assets-import → scene`，而不是反过来（`docs/62` P1 第 3 条）。

use crate::prefab::Prefab;
use crate::scene::Scene;
use rustlua_assets::{importer::Importer, resource_kind::ResourceKind};
use std::path::Path;

struct SceneImporter;
struct PrefabImporter;

impl Importer for SceneImporter {
    fn version(&self) -> &'static str {
        // 与拆分前（rustlua_assets::importer）保持一致：`scene-1` 是资源库/缓存里
        // 记录的版本号，改动它会强制全量重导入——本次只迁实现、不改语义。
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
        } else if extension == "json" {
            // M20-01：领域数据表按严格 JSON 校验（错误带行列）。
            validate_json_data(source, bytes)?;
        } else if extension == "ron" {
            // M20-01：RON 结构校验（无 ron 依赖，见 validate_ron_structure 说明）。
            validate_ron_structure(source, bytes)?;
        }
        Ok(())
    }
}

impl Importer for PrefabImporter {
    fn version(&self) -> &'static str {
        // 与拆分前保持一致：`prefab-1`（缓存里的版本号，不随 crate 拆分改动）。
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
        // `.prefab` 这一个扩展名承载了**两种不同的东西**：
        //
        // * 实体预制体（缺省）：`entity` / `component` 记录，解析成 `Prefab`；
        // * 特效预设（`kind=effect`）：`graph=` / `overlay=` / `param<TAB>名<TAB>值`，
        //   由 `rustlua-visual-script::effect_prefab` 解析，运行时走 `EffectPrefab::load`
        //   （`engine::host::sample_project`），**完全不经过本模块**。
        //
        // 之前这里无条件按实体预制体解析，于是每个特效预设都在导入期报
        // "unknown record"——而它其实从来没打算被 `Prefab` 认识。判据只能是内容里的
        // `kind=`，因为扩展名区分不了。
        //
        // 这里**不**做 effect 的语义校验：那需要 `rustlua-visual-script`，而本 crate 是
        // 运行时也编译的（见模块文档），不能反向依赖它。effect 的格式错误由加载
        // `EffectPrefab` 的那一侧点名（`effect_prefab::from_text` 的诊断带行号）。
        if declares_effect_kind(text) {
            return Ok(());
        }
        Prefab::from_text(text)
            .map_err(|error| format!("{}: 预制体解析失败: {error}", source.display()))?;
        Ok(())
    }
}

/// 判断这份 `.prefab` 是否自称特效预设（`kind=effect`）。
///
/// 只看**第一个非注释、非空行之后**的 `kind=` 声明之前的部分不够稳；这里逐行找
/// `kind=`，与 `effect_prefab::from_text` 的口径一致（它也是整篇扫描）。
/// 注释行以 `#` 开头，与两种格式的约定相同。
fn declares_effect_kind(text: &str) -> bool {
    text.lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .find_map(|line| line.strip_prefix("kind="))
        .is_some_and(|value| value.trim() == "effect")
}

/// 场景层的内置导入器，固定顺序。
#[must_use]
pub fn scene_importers() -> [&'static dyn Importer; 2] {
    [&SceneImporter, &PrefabImporter]
}

/// Rejects empty sources uniformly across builtin importers.
fn reject_empty(source: &Path, bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() {
        Err(format!("{}: 源文件为空", source.display()))
    } else {
        Ok(())
    }
}

/// M20-01：UTF-8 BOM 检查。
///
/// Lua 解析器不接受 BOM（报 `unexpected symbol near '<\239>'`），而文本编辑器
/// 「另存为 UTF-8」常带 BOM；这里给出可操作的诊断，而不是让错误在运行时以
/// 乱码形式出现（场景加载器已自带 BOM 容忍，脚本/数据文件按无 BOM 约定）。
fn reject_bom(source: &Path, bytes: &[u8], label: &str) -> Result<(), String> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Err(format!(
            "{}: {label}含 UTF-8 BOM，请另存为「UTF-8 无 BOM」",
            source.display()
        ));
    }
    Ok(())
}

/// M20-01：`.json` 数据表严格校验；serde_json 的错误自带行/列定位。
fn validate_json_data(source: &Path, bytes: &[u8]) -> Result<(), String> {
    reject_bom(source, bytes, "JSON 文件")?;
    let text = std::str::from_utf8(bytes)
        .map_err(|error| format!("{}: JSON 编码非法（需 UTF-8）: {error}", source.display()))?;
    serde_json::from_str::<serde_json::Value>(text)
        .map_err(|error| format!("{}: JSON 解析失败: {error}", source.display()))?;
    Ok(())
}

/// M20-01：`.ron` 结构化校验（分隔符/字符串配平 + 定位到行列）。
///
/// 说明：工作区未引入 `ron` 依赖（离线环境无法新增 crates），此处做结构级
/// 校验以拦截截断/损坏文件；完整 RON 语法解析留待引入依赖后替换。
fn validate_ron_structure(source: &Path, bytes: &[u8]) -> Result<(), String> {
    reject_bom(source, bytes, "RON 文件")?;
    let text = std::str::from_utf8(bytes)
        .map_err(|error| format!("{}: RON 编码非法（需 UTF-8）: {error}", source.display()))?;
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut line = 1usize;
    let mut column = 1usize;
    for (index, character) in text.char_indices() {
        if character == '\n' {
            line += 1;
            column = 1;
            continue;
        }
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            column += 1;
            continue;
        }
        match character {
            '"' => in_string = true,
            '(' | '[' | '{' => stack.push((character, index)),
            ')' | ']' | '}' => {
                let expected = match character {
                    ')' => '(',
                    ']' => '[',
                    _ => '{',
                };
                match stack.pop() {
                    Some((open, _)) if open == expected => {}
                    Some((open, open_index)) => {
                        let (open_line, open_column) = line_column(text, open_index);
                        return Err(format!(
                            "{}: RON 结构错误: 第 {line} 行第 {column} 列的 '{character}' 与第 {open_line} 行第 {open_column} 列的 '{open}' 不匹配",
                            source.display()
                        ));
                    }
                    None => {
                        return Err(format!(
                            "{}: RON 结构错误: 第 {line} 行第 {column} 列出现多余的 '{character}'",
                            source.display()
                        ));
                    }
                }
            }
            _ => {}
        }
        column += 1;
    }
    if in_string {
        return Err(format!(
            "{}: RON 结构错误: 字符串未闭合（起始于第 {line} 行）",
            source.display()
        ));
    }
    if let Some((open, open_index)) = stack.pop() {
        let (open_line, open_column) = line_column(text, open_index);
        return Err(format!(
            "{}: RON 结构错误: 第 {open_line} 行第 {open_column} 列的 '{open}' 未闭合",
            source.display()
        ));
    }
    Ok(())
}

/// 由字节偏移估算 (line, column)（仅用于 RON 结构诊断，允许 O(n) 回扫）。
fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut column = 1usize;
    for (index, character) in text.char_indices() {
        if index >= offset {
            break;
        }
        if character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_assets::importer::ImporterRegistry;
    use rustlua_assets::resource_kind::ResourceKind;
    use std::fs;
    use std::path::PathBuf;

    fn root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rustlua-scene-asset-import-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write_source(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = root.join(name);
        fs::write(&path, bytes).expect("write source");
        path
    }

    fn scene_bytes() -> Vec<u8> {
        format!(
            "scene_version={}\nname=main\n",
            crate::scene::CURRENT_SCENE_VERSION
        )
        .into_bytes()
    }

    fn prefab_bytes() -> Vec<u8> {
        b"prefab_version=1\nasset_id=00000000-0000-4000-8000-000000000000\nname=hero\nroot=1\nentity\t1\troot\t\n".to_vec()
    }

    /// 本 crate 两个零件的注册表（**不含**六个核心导入器 —— 它们在导入层，本 crate 不依赖）。
    fn scene_registry() -> ImporterRegistry {
        ImporterRegistry::from_importers(&scene_importers())
    }

    #[test]
    fn scene_importer_validates_json_and_ron_structure() {
        let root = root("data-json-ron");
        let importer = scene_registry().get(ResourceKind::Scene).unwrap();

        let good_json = write_source(&root, "good.json", b"{\"a\": [1, 2, 3]}");
        importer
            .validate(&good_json, &std::fs::read(&good_json).unwrap())
            .unwrap();

        let bad_json = write_source(&root, "bad.json", b"{\"a\": [1, 2,}");
        let error = importer
            .validate(&bad_json, &std::fs::read(&bad_json).unwrap())
            .expect_err("invalid json must be rejected");
        assert!(error.contains("bad.json"), "诊断需含文件路径: {error}");
        assert!(
            error.contains("JSON 解析失败"),
            "诊断需标明 JSON 失败: {error}"
        );
        assert!(error.contains("line 1"), "诊断需含行列定位: {error}");

        let good_ron = write_source(&root, "good.ron", b"(name: \"demo\", items: [1, 2])");
        importer
            .validate(&good_ron, &std::fs::read(&good_ron).unwrap())
            .unwrap();

        let bad_ron = write_source(&root, "bad.ron", b"(name: \"demo\", items: [1, 2)");
        let error = importer
            .validate(&bad_ron, &std::fs::read(&bad_ron).unwrap())
            .expect_err("unbalanced ron must be rejected");
        assert!(error.contains("bad.ron"), "诊断需含文件路径: {error}");
        assert!(
            error.contains("RON 结构错误"),
            "诊断需标明结构错误: {error}"
        );
        assert!(error.contains("第 1 行"), "诊断需含行列定位: {error}");

        let unclosed = write_source(&root, "unclosed.ron", b"(name: \"demo\"");
        let error = importer
            .validate(&unclosed, &std::fs::read(&unclosed).unwrap())
            .expect_err("unclosed ron must be rejected");
        assert!(error.contains("未闭合"), "诊断需说明未闭合: {error}");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scene_importer_is_strict_for_scene_but_lenient_for_ron_json() {
        let root = root("scene");
        let good = write_source(&root, "main.scene", &scene_bytes());
        let importer = scene_registry().get(ResourceKind::Scene).unwrap();
        importer
            .validate(&good, &std::fs::read(&good).unwrap())
            .unwrap();
        let bad_scene = write_source(&root, "bad.scene", b"scene_version=0\nname\nbad\n");
        assert!(
            importer
                .validate(&bad_scene, b"scene_version=0\nname\nbad\n")
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
        let importer = scene_registry().get(ResourceKind::Prefab).unwrap();
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

    /// `.prefab` 承载两种格式；`kind=effect` 的必须在实体预制体校验里**放行**。
    ///
    /// 这条门防的是回归：这段分流曾经缺失，于是每个特效预设在导入期都报
    /// "unknown record"，而它根本没打算被 `Prefab` 认识（运行时走 `EffectPrefab::load`）。
    /// assetc 的两条用例因此长期判红——一个"看起来像资产有问题"的假信号。
    #[test]
    fn prefab_importer_lets_effect_presets_through() {
        let importer = scene_registry().get(ResourceKind::Prefab).unwrap();
        let path = Path::new("assets/fx/slash.prefab");

        // 真实格式：注释 + kind=effect + graph/overlay/param（制表符分隔）。
        let effect = "# 特效预设\nprefab_version=1\nkind=effect\nname=炎刃斩击\n\
                      graph=assets/fx/slash.sgraph\noverlay=true\nparam\tspin\t2.4\n";
        importer
            .validate(path, effect.as_bytes())
            .expect("特效预设不该被实体预制体的解析器拦下");

        // 反向：普通实体预制体仍然必须通过实体校验（分流不能把两种格式混为一谈）。
        // `kind=` 不是 `effect` 时按实体预制体处理，这里缺 entity 记录，必须报错。
        let not_effect = "prefab_version=1\nkind=entity\nname=x\n";
        assert!(
            importer.validate(path, not_effect.as_bytes()).is_err(),
            "kind 不是 effect 时应走实体预制体校验"
        );
    }

    /// 完整注册表（六个核心导入器 + 本 crate 两个）的装配测试**搬到了导入层**
    /// （`rustlua-assets-import`）：装配点不在本 crate 了，因为核心导入器要
    /// `image`/`rodio`/`ab_glyph`/`mlua`，而本 crate 是浏览器也要编的。
    /// 这里只钉住"本 crate 提供的两个零件本身可用、且没有偷带别的 kind"。
    #[test]
    fn scene_importers_are_resolvable_by_kind_and_path() {
        let registry = scene_registry();
        for kind in [ResourceKind::Scene, ResourceKind::Prefab] {
            let importer = registry.get(kind).expect("scene importer exists");
            assert_eq!(importer.kind(), kind);
            assert!(!importer.extensions().is_empty());
        }
        assert!(
            registry.get(ResourceKind::Texture).is_none(),
            "本 crate 不该带核心导入器（那是导入层的事）"
        );
        assert_eq!(
            registry.from_path(Path::new("c.prefab")).unwrap().kind(),
            ResourceKind::Prefab
        );
        assert_eq!(
            registry.from_path(Path::new("s.scene")).unwrap().kind(),
            ResourceKind::Scene
        );
    }

    #[test]
    fn scene_and_prefab_importers_reject_empty_sources() {
        let registry = scene_registry();
        for kind in [ResourceKind::Scene, ResourceKind::Prefab] {
            let importer = registry.get(kind).unwrap();
            assert!(
                importer.validate(Path::new("empty.bin"), b"").is_err(),
                "kind {kind:?} must reject empty sources"
            );
        }
    }
}
