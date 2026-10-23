//! P3-R02：`.material` 文本材质（版本 1）的解析与编码。
//!
//! **为什么住运行时层**：导入层（`rustlua-assets-import` 的 `MaterialImporter`）校验
//! `.material` 时必须用**同一台解析器**——否则资源浏览器报「导入成功」而渲染器加载时才炸，
//! 两条校验路径还会各自漂移。但导入层不许反过来依赖 `rustlua-render`（那会把 `mlua`/`ecs`
//! 与整套管线类型拖进导入编译图）。这台解析器需要的类型全在本 crate：glTF 材质
//! （[`crate::gltf_asset::GltfMaterial`]）与内嵌贴图像素（[`crate::gltf_asset::GltfTextureImage`]），
//! 路径合法性判定用 `rustlua_infra::paths` —— 所以搬家没有新增任何依赖边。
//! `rustlua-render::mesh3d` 以 `pub use` 原样转发这些类型，渲染侧调用点不变。

use std::path::Path;

use rustlua_infra::paths::{escapes_project_root, is_absolute_path};

/// glTF / `.material` 的 alpha 模式。
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum MaterialAlphaMode {
    #[default]
    Opaque,
    Mask,
    Blend,
}

impl MaterialAlphaMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Opaque => "opaque",
            Self::Mask => "mask",
            Self::Blend => "blend",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "opaque" => Some(Self::Opaque),
            "mask" => Some(Self::Mask),
            "blend" => Some(Self::Blend),
            _ => None,
        }
    }
}

/// 一张材质贴图：`.material` 用项目相对路径，glTF 用内嵌解码像素。
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialTexture {
    pub path: Option<String>,
    pub image: Option<crate::gltf_asset::GltfTextureImage>,
}

/// 3D 材质的统一描述（glTF 与 `.material` 共用；P3-R02）。
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialAsset {
    pub name: Option<String>,
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
    pub emissive_strength: f32,
    pub base_color_texture: Option<MaterialTexture>,
    /// G = roughness，B = metallic（glTF ORM 约定）。
    pub metallic_roughness_texture: Option<MaterialTexture>,
    pub normal_texture: Option<MaterialTexture>,
    pub occlusion_texture: Option<MaterialTexture>,
    pub emissive_texture: Option<MaterialTexture>,
    pub normal_scale: f32,
    pub occlusion_strength: f32,
    pub alpha_mode: MaterialAlphaMode,
    pub alpha_cutoff: f32,
    pub double_sided: bool,
    pub ibl_intensity: f32,
}

impl Default for MaterialAsset {
    fn default() -> Self {
        Self {
            name: None,
            base_color: [1.0; 4],
            metallic: 1.0,
            roughness: 1.0,
            emissive: [0.0; 3],
            emissive_strength: 1.0,
            base_color_texture: None,
            metallic_roughness_texture: None,
            normal_texture: None,
            occlusion_texture: None,
            emissive_texture: None,
            normal_scale: 1.0,
            occlusion_strength: 1.0,
            alpha_mode: MaterialAlphaMode::Opaque,
            alpha_cutoff: 0.5,
            double_sided: false,
            ibl_intensity: 1.0,
        }
    }
}

/// `.material` 文本资产解析错误（含源路径与行号）。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("material {path}: line {line}: {detail}")]
pub struct MaterialAssetError {
    pub path: String,
    pub line: usize,
    pub detail: String,
}

impl MaterialAsset {
    /// 解析 `.material` 文本。严格校验版本/字段/路径/数值；未写出的字段取 glTF 默认值。
    pub fn from_text(path: impl AsRef<Path>, text: &str) -> Result<Self, MaterialAssetError> {
        let path = path.as_ref().display().to_string();
        let error = |line: usize, detail: String| MaterialAssetError {
            path: path.clone(),
            line,
            detail,
        };
        let mut material = Self::default();
        let mut version: Option<String> = None;
        for (index, raw_line) in text.lines().enumerate() {
            let line_number = index + 1;
            let line = raw_line.trim_end_matches('\r').trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (raw_key, raw_value) = line
                .split_once('=')
                .ok_or_else(|| error(line_number, format!("expected key=value, got `{line}`")))?;
            let key = raw_key.trim();
            let value = raw_value.trim();
            if value.is_empty() && key != "name" {
                return Err(error(line_number, format!("`{key}` is empty")));
            }
            let number = |field: &'static str| -> Result<f32, MaterialAssetError> {
                let parsed = value.parse::<f32>().map_err(|_| {
                    error(line_number, format!("`{field}` is not a number: `{value}`"))
                })?;
                if !parsed.is_finite() {
                    return Err(error(
                        line_number,
                        format!("`{field}` must be finite: `{value}`"),
                    ));
                }
                Ok(parsed)
            };
            match key {
                "version" => {
                    if version.replace(value.to_owned()).is_some() {
                        return Err(error(line_number, "duplicate `version`".to_owned()));
                    }
                }
                "name" => {
                    if value.is_empty() || value.contains(['\n', '\r']) {
                        return Err(error(line_number, "`name` must be non-empty".to_owned()));
                    }
                    material.name = Some(value.to_owned());
                }
                "base_color" => material.base_color = parse_vec4(value, line_number, &path)?,
                "metallic" => material.metallic = number("metallic")?,
                "roughness" => material.roughness = number("roughness")?,
                "emissive" => {
                    let [r, g, b] = parse_vec3(value, line_number, &path)?;
                    material.emissive = [r, g, b];
                }
                "emissive_strength" => material.emissive_strength = number("emissive_strength")?,
                "normal_scale" => material.normal_scale = number("normal_scale")?,
                "occlusion_strength" => {
                    material.occlusion_strength = number("occlusion_strength")?;
                }
                "alpha_cutoff" => material.alpha_cutoff = number("alpha_cutoff")?,
                "ibl_intensity" => material.ibl_intensity = number("ibl_intensity")?,
                "alpha_mode" => {
                    material.alpha_mode = MaterialAlphaMode::parse(value).ok_or_else(|| {
                        error(
                            line_number,
                            format!("`alpha_mode` must be opaque|mask|blend, got `{value}`"),
                        )
                    })?;
                }
                "double_sided" => {
                    material.double_sided = match value {
                        "true" => true,
                        "false" => false,
                        _ => {
                            return Err(error(
                                line_number,
                                format!("`double_sided` must be true|false, got `{value}`"),
                            ));
                        }
                    };
                }
                "base_color_texture" => {
                    material.base_color_texture =
                        Some(texture_from_path(value, line_number, &path)?);
                }
                "metallic_roughness_texture" => {
                    material.metallic_roughness_texture =
                        Some(texture_from_path(value, line_number, &path)?);
                }
                "normal_texture" => {
                    material.normal_texture = Some(texture_from_path(value, line_number, &path)?);
                }
                "occlusion_texture" => {
                    material.occlusion_texture =
                        Some(texture_from_path(value, line_number, &path)?);
                }
                "emissive_texture" => {
                    material.emissive_texture = Some(texture_from_path(value, line_number, &path)?);
                }
                _ => {
                    return Err(error(line_number, format!("unknown field `{key}`")));
                }
            }
        }
        let version = version.ok_or_else(|| error(1, "missing required `version=1`".to_owned()))?;
        if version != "1" {
            return Err(error(
                1,
                format!("unsupported version `{version}` (expected 1)"),
            ));
        }
        if !(0.0..=1.0).contains(&material.metallic) {
            return Err(error(1, "`metallic` must be in 0..=1".to_owned()));
        }
        if !(0.0..=1.0).contains(&material.roughness) {
            return Err(error(1, "`roughness` must be in 0..=1".to_owned()));
        }
        if !(0.0..=1.0).contains(&material.alpha_cutoff) {
            return Err(error(1, "`alpha_cutoff` must be in 0..=1".to_owned()));
        }
        if material.emissive_strength < 0.0 || material.normal_scale < 0.0 {
            return Err(error(
                1,
                "`emissive_strength`/`normal_scale` must be non-negative".to_owned(),
            ));
        }
        if material.occlusion_strength < 0.0 || material.ibl_intensity < 0.0 {
            return Err(error(
                1,
                "`occlusion_strength`/`ibl_intensity` must be non-negative".to_owned(),
            ));
        }
        Ok(material)
    }

    /// 编码回 `.material` 文本（全字段；往返测试与编辑器另存共用）。
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut out = String::from("version=1\n");
        if let Some(name) = &self.name {
            out.push_str(&format!("name={name}\n"));
        }
        out.push_str(&format!(
            "base_color={},{},{},{}\n",
            self.base_color[0], self.base_color[1], self.base_color[2], self.base_color[3]
        ));
        out.push_str(&format!("metallic={}\n", self.metallic));
        out.push_str(&format!("roughness={}\n", self.roughness));
        out.push_str(&format!(
            "emissive={},{},{}\n",
            self.emissive[0], self.emissive[1], self.emissive[2]
        ));
        out.push_str(&format!("emissive_strength={}\n", self.emissive_strength));
        for (key, texture) in [
            ("base_color_texture", &self.base_color_texture),
            (
                "metallic_roughness_texture",
                &self.metallic_roughness_texture,
            ),
            ("normal_texture", &self.normal_texture),
            ("occlusion_texture", &self.occlusion_texture),
            ("emissive_texture", &self.emissive_texture),
        ] {
            if let Some(path) = texture.as_ref().and_then(|texture| texture.path.as_deref()) {
                out.push_str(&format!("{key}={path}\n"));
            }
        }
        out.push_str(&format!("normal_scale={}\n", self.normal_scale));
        out.push_str(&format!("occlusion_strength={}\n", self.occlusion_strength));
        out.push_str(&format!("alpha_mode={}\n", self.alpha_mode.as_str()));
        out.push_str(&format!("alpha_cutoff={}\n", self.alpha_cutoff));
        out.push_str(&format!("double_sided={}\n", self.double_sided));
        out.push_str(&format!("ibl_intensity={}\n", self.ibl_intensity));
        out
    }

    /// `.material` 引用的贴图路径（进资产依赖图；glTF 内嵌贴图不在其中）。
    #[must_use]
    pub fn texture_paths(&self) -> Vec<&str> {
        [
            &self.base_color_texture,
            &self.metallic_roughness_texture,
            &self.normal_texture,
            &self.occlusion_texture,
            &self.emissive_texture,
        ]
        .into_iter()
        .filter_map(|texture| texture.as_ref().and_then(|texture| texture.path.as_deref()))
        .collect()
    }
}

fn parse_vec3(value: &str, line: usize, path: &str) -> Result<[f32; 3], MaterialAssetError> {
    let values = parse_floats::<3>(value, line, path)?;
    Ok(values)
}

fn parse_vec4(value: &str, line: usize, path: &str) -> Result<[f32; 4], MaterialAssetError> {
    parse_floats::<4>(value, line, path)
}

fn parse_floats<const N: usize>(
    value: &str,
    line: usize,
    path: &str,
) -> Result<[f32; N], MaterialAssetError> {
    let mut out = [0.0; N];
    let parts: Vec<&str> = value.split(',').map(str::trim).collect();
    if parts.len() != N {
        return Err(MaterialAssetError {
            path: path.to_owned(),
            line,
            detail: format!("expected {N} comma-separated numbers, got `{value}`"),
        });
    }
    for (index, part) in parts.into_iter().enumerate() {
        let parsed = part.parse::<f32>().map_err(|_| MaterialAssetError {
            path: path.to_owned(),
            line,
            detail: format!("`{part}` is not a number"),
        })?;
        if !parsed.is_finite() {
            return Err(MaterialAssetError {
                path: path.to_owned(),
                line,
                detail: format!("`{part}` must be finite"),
            });
        }
        out[index] = parsed;
    }
    Ok(out)
}

fn texture_from_path(
    value: &str,
    line: usize,
    path: &str,
) -> Result<MaterialTexture, MaterialAssetError> {
    let extension = Path::new(value)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if is_absolute_path(value)
        || escapes_project_root(value)
        || !matches!(extension.as_str(), "png" | "jpg" | "jpeg")
    {
        return Err(MaterialAssetError {
            path: path.to_owned(),
            line,
            detail: format!("invalid texture path `{value}` (project-relative png/jpg required)"),
        });
    }
    Ok(MaterialTexture {
        path: Some(value.to_owned()),
        image: None,
    })
}

impl From<&crate::gltf_asset::GltfMaterial> for MaterialAsset {
    fn from(material: &crate::gltf_asset::GltfMaterial) -> Self {
        use crate::gltf_asset::{GltfAlphaMode, GltfTextureImage};
        let texture = |image: &Option<GltfTextureImage>| {
            image.clone().map(|image| MaterialTexture {
                path: None,
                image: Some(image),
            })
        };
        Self {
            name: material.name.clone(),
            base_color: material.base_color_factor,
            metallic: material.metallic_factor,
            roughness: material.roughness_factor,
            emissive: material.emissive_factor,
            emissive_strength: material.emissive_strength,
            base_color_texture: texture(&material.base_color_image),
            metallic_roughness_texture: texture(&material.metallic_roughness_image),
            normal_texture: texture(&material.normal_image),
            occlusion_texture: texture(&material.occlusion_image),
            emissive_texture: texture(&material.emissive_image),
            normal_scale: material.normal_scale,
            occlusion_strength: material.occlusion_strength,
            alpha_mode: match material.alpha_mode {
                GltfAlphaMode::Opaque => MaterialAlphaMode::Opaque,
                GltfAlphaMode::Mask => MaterialAlphaMode::Mask,
                GltfAlphaMode::Blend => MaterialAlphaMode::Blend,
            },
            alpha_cutoff: material.alpha_cutoff,
            double_sided: material.double_sided,
            ibl_intensity: 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MATERIAL_TEXT: &str = "\
version=1
name=rusted_metal
base_color=0.8,0.7,0.6,1
metallic=1.0
roughness=0.35
emissive=0.1,0.05,0.0
emissive_strength=2.0
base_color_texture=assets/textures/rust_albedo.png
metallic_roughness_texture=assets/textures/rust_orm.png
normal_texture=assets/textures/rust_normal.png
occlusion_texture=assets/textures/rust_ao.jpg
emissive_texture=assets/textures/rust_emissive.jpeg
normal_scale=0.8
occlusion_strength=0.6
alpha_mode=mask
alpha_cutoff=0.4
double_sided=true
ibl_intensity=0.75
";

    #[test]
    fn material_text_parser_round_trips_every_field() {
        let material = MaterialAsset::from_text("assets/materials/rusted.material", MATERIAL_TEXT)
            .expect("示例材质必须可解析");
        assert_eq!(material.name.as_deref(), Some("rusted_metal"));
        assert_eq!(material.base_color, [0.8, 0.7, 0.6, 1.0]);
        assert_eq!(material.metallic, 1.0);
        assert_eq!(material.roughness, 0.35);
        assert_eq!(material.emissive, [0.1, 0.05, 0.0]);
        assert_eq!(material.emissive_strength, 2.0);
        assert_eq!(material.normal_scale, 0.8);
        assert_eq!(material.occlusion_strength, 0.6);
        assert_eq!(material.alpha_mode, MaterialAlphaMode::Mask);
        assert_eq!(material.alpha_cutoff, 0.4);
        assert!(material.double_sided);
        assert_eq!(material.ibl_intensity, 0.75);
        assert_eq!(
            material.texture_paths(),
            vec![
                "assets/textures/rust_albedo.png",
                "assets/textures/rust_orm.png",
                "assets/textures/rust_normal.png",
                "assets/textures/rust_ao.jpg",
                "assets/textures/rust_emissive.jpeg",
            ]
        );
        let reparsed =
            MaterialAsset::from_text("assets/materials/rusted.material", &material.to_text())
                .expect("to_text 的输出必须可回读");
        assert_eq!(reparsed, material);
    }

    #[test]
    fn material_text_defaults_match_gltf_when_fields_are_omitted() {
        let material =
            MaterialAsset::from_text("m.material", "version=1\nmetallic=0.25\n").unwrap();
        assert_eq!(material.base_color, [1.0; 4]);
        assert_eq!(material.metallic, 0.25);
        assert_eq!(material.roughness, 1.0);
        assert_eq!(material.emissive, [0.0; 3]);
        assert_eq!(material.emissive_strength, 1.0);
        assert_eq!(material.alpha_mode, MaterialAlphaMode::Opaque);
        assert_eq!(material.alpha_cutoff, 0.5);
        assert!(material.texture_paths().is_empty());
    }

    #[test]
    fn material_text_rejects_bad_versions_fields_paths_and_values() {
        let cases = [
            ("name=x\n", "missing required `version=1`"),
            ("version=2\n", "unsupported version"),
            ("version=1\nfoo=bar\n", "unknown field `foo`"),
            ("version=1\nmetallic=abc\n", "not a number"),
            ("version=1\nmetallic=1.5\n", "`metallic` must be in 0..=1"),
            (
                "version=1\nroughness=-0.1\n",
                "`roughness` must be in 0..=1",
            ),
            (
                "version=1\nalpha_cutoff=2\n",
                "`alpha_cutoff` must be in 0..=1",
            ),
            ("version=1\nalpha_mode=cutout\n", "alpha_mode"),
            ("version=1\ndouble_sided=yes\n", "double_sided"),
            ("version=1\nbase_color=1,2,3\n", "expected 4"),
            (
                "version=1\nbase_color_texture=../escape.png\n",
                "invalid texture path",
            ),
            (
                "version=1\nbase_color_texture=/abs/tex.png\n",
                "invalid texture path",
            ),
            (
                "version=1\nbase_color_texture=assets/tex.bmp\n",
                "invalid texture path",
            ),
            ("version=1\nversion=1\n", "duplicate `version`"),
        ];
        for (text, expected) in cases {
            let error = MaterialAsset::from_text("assets/bad.material", text)
                .expect_err("非法材质必须报错");
            assert!(
                error.to_string().contains(expected),
                "error `{error}` should contain `{expected}`"
            );
            assert!(error.to_string().contains("assets/bad.material"));
        }
    }

    #[test]
    fn gltf_material_maps_orm_emissive_alpha_and_textures() {
        use crate::gltf_asset::{GltfAlphaMode, GltfMaterial, GltfTextureImage};
        let image = GltfTextureImage {
            width: 1,
            height: 1,
            rgba: vec![255, 128, 64, 255],
        };
        let source = GltfMaterial {
            name: Some("gold".to_owned()),
            base_color_factor: [0.9, 0.8, 0.2, 0.75],
            base_color_texture: Some(0),
            base_color_image: Some(image.clone()),
            metallic_factor: 0.95,
            roughness_factor: 0.2,
            metallic_roughness_texture: Some(1),
            metallic_roughness_image: Some(image.clone()),
            normal_texture: Some(2),
            normal_image: Some(image.clone()),
            normal_scale: 0.5,
            occlusion_texture: Some(3),
            occlusion_image: Some(image.clone()),
            occlusion_strength: 0.7,
            emissive_texture: Some(4),
            emissive_image: Some(image.clone()),
            emissive_factor: [1.0, 0.4, 0.0],
            emissive_strength: 3.0,
            alpha_mode: GltfAlphaMode::Blend,
            alpha_cutoff: 0.3,
            double_sided: true,
        };
        let material = MaterialAsset::from(&source);
        assert_eq!(material.base_color, source.base_color_factor);
        assert_eq!(material.metallic, 0.95);
        assert_eq!(material.roughness, 0.2);
        assert_eq!(material.emissive, [1.0, 0.4, 0.0]);
        assert_eq!(material.emissive_strength, 3.0);
        assert_eq!(material.normal_scale, 0.5);
        assert_eq!(material.occlusion_strength, 0.7);
        assert_eq!(material.alpha_mode, MaterialAlphaMode::Blend);
        assert_eq!(material.alpha_cutoff, 0.3);
        assert!(material.double_sided);
        assert_eq!(material.ibl_intensity, 1.0);
        for texture in [
            &material.base_color_texture,
            &material.metallic_roughness_texture,
            &material.normal_texture,
            &material.occlusion_texture,
            &material.emissive_texture,
        ] {
            assert_eq!(
                texture.as_ref().and_then(|texture| texture.image.as_ref()),
                Some(&image)
            );
            assert_eq!(
                texture.as_ref().and_then(|texture| texture.path.as_ref()),
                None
            );
        }
    }
}
