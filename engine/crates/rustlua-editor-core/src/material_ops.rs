//! P3-E02-2：`.material` 读写——工程内路径约束 + 保存前 dry-run + 临时文件原子替换。
//!
//! 与 `anim_ops` 同一套口径：只接受工程内普通相对路径，扩展名必须匹配；先写
//! `*.material.tmp` 再替换，失败时清理临时文件、旧文件保持原样。保存前用
//! `MaterialAsset::from_text` 回读一遍（与导入器/运行时同一台解析器），坏内容不落盘。

use std::path::{Component, Path, PathBuf};

use rustlua_assets::mat_asset::MaterialAsset;

#[derive(Debug, thiserror::Error)]
pub enum MaterialIoError {
    #[error("材质路径必须是工程内相对路径，got `{0}`")]
    InvalidPath(String),
    #[error("材质必须使用 .material 扩展名，got `{0}`")]
    NotMaterial(String),
    #[error("材质内容校验失败（未写盘）：{0}")]
    Invalid(#[from] rustlua_assets::mat_asset::MaterialAssetError),
    #[error("材质含内嵌贴图，保存为 .material 会丢失贴图")]
    EmbeddedTexture,
    #[error("材质文件只读，拒绝覆盖 `{path}`：先去掉只读属性再保存")]
    ReadOnly { path: String },
    #[error("{action} `{path}` 失败：{source}")]
    Io {
        action: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// 解析工程内相对路径。调用方拿到的路径保证在 `root` 之下，且是 `.material`。
pub fn resolve_material_path(root: &Path, relative: &str) -> Result<PathBuf, MaterialIoError> {
    let candidate = Path::new(relative);
    let is_plain_relative = !relative.is_empty()
        && relative.trim() == relative
        && candidate
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if !is_plain_relative {
        return Err(MaterialIoError::InvalidPath(relative.to_owned()));
    }
    let is_material = candidate
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("material"));
    if !is_material {
        return Err(MaterialIoError::NotMaterial(relative.to_owned()));
    }
    Ok(root.join(candidate))
}

/// 读取并解析工程内的 `.material`。错误带文件路径与行号（解析器直接产出）。
pub fn load_material(root: &Path, relative: &str) -> Result<MaterialAsset, MaterialIoError> {
    let path = resolve_material_path(root, relative)?;
    let text = std::fs::read_to_string(&path).map_err(|source| MaterialIoError::Io {
        action: "读取材质",
        path: path.display().to_string(),
        source,
    })?;
    Ok(MaterialAsset::from_text(relative, &text)?)
}

/// 把材质规范文本原子写入工程内 `.material`，返回最终绝对路径。
pub fn save_material(
    root: &Path,
    relative: &str,
    material: &MaterialAsset,
) -> Result<PathBuf, MaterialIoError> {
    let path = resolve_material_path(root, relative)?;
    if path.exists()
        && path
            .metadata()
            .is_ok_and(|metadata| metadata.permissions().readonly())
    {
        return Err(MaterialIoError::ReadOnly {
            path: path.display().to_string(),
        });
    }
    let text = material.to_text();
    // dry-run：坏内容（含内嵌贴图）不覆盖旧文件。
    let reparsed = MaterialAsset::from_text(relative, &text)?;
    if reparsed != *material {
        return Err(MaterialIoError::EmbeddedTexture);
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| MaterialIoError::InvalidPath(relative.to_owned()))?;
    std::fs::create_dir_all(parent).map_err(|source| MaterialIoError::Io {
        action: "创建材质目录",
        path: parent.display().to_string(),
        source,
    })?;
    let temp = path.with_extension("material.tmp");
    if let Err(source) = std::fs::write(&temp, text) {
        let _ = std::fs::remove_file(&temp);
        return Err(MaterialIoError::Io {
            action: "写材质",
            path: temp.display().to_string(),
            source,
        });
    }
    if let Err(source) = std::fs::rename(&temp, &path) {
        let _ = std::fs::remove_file(&temp);
        return Err(MaterialIoError::Io {
            action: "替换材质",
            path: path.display().to_string(),
            source,
        });
    }
    Ok(path)
}

/// 由实体名生成一个不重复的工程内材质路径（`assets/materials/<slug>.material`）。
///
/// 只在面板点「创建覆盖」时使用；同名冲突加 `_2`、`_3`… 后缀，不覆盖已有文件。
#[must_use]
pub fn suggest_override_path(root: &Path, entity_name: &str) -> String {
    let mut slug = String::new();
    for ch in entity_name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.ends_with('_') {
            slug.push('_');
        }
    }
    let slug = slug.trim_matches('_');
    let slug = if slug.is_empty() { "material" } else { slug };
    for index in 1..=9999 {
        let relative = if index == 1 {
            format!("assets/materials/{slug}.material")
        } else {
            format!("assets/materials/{slug}_{index}.material")
        };
        if !root.join(&relative).exists() {
            return relative;
        }
    }
    format!("assets/materials/{slug}_new.material")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_root(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "rustlua-material-ops-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");
        root
    }

    fn sample() -> MaterialAsset {
        MaterialAsset {
            name: Some("rusted".to_owned()),
            metallic: 0.25,
            roughness: 0.6,
            ..MaterialAsset::default()
        }
    }

    #[test]
    fn save_and_load_round_trip_without_leaving_a_temp_file() {
        let root = temp_root("roundtrip");
        let relative = "assets/materials/rusted.material";
        let path = save_material(&root, relative, &sample()).expect("save");
        assert_eq!(path, root.join(relative));
        assert!(!path.with_extension("material.tmp").exists());
        let loaded = load_material(&root, relative).expect("load");
        assert_eq!(loaded, sample());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), sample().to_text());
    }

    #[test]
    fn save_replaces_existing_content_and_creates_directories() {
        let root = temp_root("replace");
        let relative = "assets/materials/new/deep.material";
        save_material(&root, relative, &sample()).expect("first");
        let mut second = sample();
        second.metallic = 1.0;
        save_material(&root, relative, &second).expect("second");
        assert_eq!(load_material(&root, relative).unwrap(), second);
    }

    #[test]
    fn invalid_or_escaping_paths_are_rejected_before_touching_disk() {
        let root = temp_root("reject");
        let relative = "assets/materials/keep.material";
        save_material(&root, relative, &sample()).expect("seed");
        let old = std::fs::read_to_string(root.join(relative)).unwrap();

        for bad in [
            "../escape.material",
            "assets/../../escape.material",
            "assets\\..\\escape.material",
            "",
            " assets/materials/keep.material",
            "/abs/keep.material",
        ] {
            assert!(
                resolve_material_path(&root, bad).is_err(),
                "必须拒绝 `{bad}`"
            );
        }
        assert!(matches!(
            save_material(&root, "assets/materials/keep.txt", &sample()),
            Err(MaterialIoError::NotMaterial(_))
        ));
        // dry-run 失败也不得覆盖旧文件。
        let mut bad = sample();
        bad.metallic = 9.0;
        assert!(matches!(
            save_material(&root, relative, &bad),
            Err(MaterialIoError::Invalid(_))
        ));
        assert_eq!(
            std::fs::read_to_string(root.join(relative)).unwrap(),
            old,
            "被拒绝的保存不得触碰已有文件"
        );
    }

    #[test]
    fn embedded_textures_are_rejected_instead_of_silently_dropped() {
        let root = temp_root("embedded");
        let mut material = sample();
        material.base_color_texture = Some(rustlua_assets::mat_asset::MaterialTexture {
            path: None,
            image: Some(rustlua_assets::gltf_asset::GltfTextureImage {
                width: 1,
                height: 1,
                rgba: vec![255, 255, 255, 255],
            }),
        });
        let error = save_material(&root, "assets/materials/embedded.material", &material)
            .expect_err("内嵌贴图必须被拒绝");
        assert!(matches!(error, MaterialIoError::EmbeddedTexture));
        assert!(!root.join("assets/materials/embedded.material").exists());
    }

    #[test]
    fn load_reports_parser_line_numbers() {
        let root = temp_root("parse");
        std::fs::create_dir_all(root.join("assets/materials")).unwrap();
        std::fs::write(
            root.join("assets/materials/bad.material"),
            "version=1\nmetallic=oops\n",
        )
        .unwrap();
        let error = load_material(&root, "assets/materials/bad.material").expect_err("bad");
        assert!(error.to_string().contains("line 2"), "{error}");
    }

    #[test]
    fn read_only_target_is_refused_and_left_untouched() {
        let root = temp_root("readonly");
        let relative = "assets/materials/locked.material";
        let path = save_material(&root, relative, &sample()).expect("seed");
        let original_permissions = std::fs::metadata(&path).unwrap().permissions();
        let mut permissions = original_permissions.clone();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();

        let mut changed = sample();
        changed.metallic = 0.9;
        let error = save_material(&root, relative, &changed).expect_err("只读文件必须拒绝");
        assert!(matches!(error, MaterialIoError::ReadOnly { .. }), "{error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        assert!(!path.with_extension("material.tmp").exists());

        std::fs::set_permissions(&path, original_permissions).unwrap();
    }

    #[test]
    fn suggested_override_paths_are_slugged_and_do_not_clobber() {
        let root = temp_root("suggest");
        assert_eq!(
            suggest_override_path(&root, "Rusted Ball!"),
            "assets/materials/rusted_ball.material"
        );
        std::fs::create_dir_all(root.join("assets/materials")).unwrap();
        std::fs::write(root.join("assets/materials/rusted_ball.material"), "x").unwrap();
        assert_eq!(
            suggest_override_path(&root, "Rusted Ball!"),
            "assets/materials/rusted_ball_2.material"
        );
        assert_eq!(
            suggest_override_path(&root, "!!!"),
            "assets/materials/material.material"
        );
    }
}
