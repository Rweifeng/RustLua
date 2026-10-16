//! Native startup image; no runtime scene, text or progress is needed to present it.
use std::path::{Component, Path, PathBuf};

use engine::host::{OverlayVertex, RenderOutcome, Renderer, SceneDraw, SceneMesh};

const TEXTURE_KEY: &str = "native-splash://startup-image";

fn relative_path(key: &str, value: &str) -> Result<PathBuf, String> {
    // Normalize Windows separators even when tests run on another host OS.
    let normalized = value.replace('\\', "/");
    let path = PathBuf::from(&normalized);
    if normalized.is_empty()
        || normalized.contains(':')
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
        || !path
            .components()
            .any(|part| matches!(part, Component::Normal(_)))
    {
        return Err(format!(
            "{key} 路径必须为工程内相对路径（不得含 ..）: {value}"
        ));
    }
    Ok(path)
}

pub fn load(root: &Path) -> Result<Option<image::RgbaImage>, String> {
    load_configured(root, "splash_image")
}

pub fn load_icon(root: &Path) -> Result<Option<image::RgbaImage>, String> {
    load_configured(root, "icon_image")
}

fn load_configured(root: &Path, key: &str) -> Result<Option<image::RgbaImage>, String> {
    let Some(value) = rustlua_infra::project_config::read_key(root, key) else {
        return Ok(None);
    };
    let path = root.join(relative_path(key, &value)?);
    // read_resource already unwraps assetc compression/encryption.
    let bytes = rustlua_infra::vfs::read_resource(&path)
        .map_err(|error| format!("{key} 读取失败 {}: {error}", path.display()))?;
    image::load_from_memory(&bytes)
        .map(|image| Some(image.to_rgba8()))
        .map_err(|error| format!("{key} 解码失败 {}: {error}", path.display()))
}

pub fn startup_image(root: Option<&Path>) -> Result<image::RgbaImage, String> {
    if let Some(root) = root {
        match load(root) {
            Ok(Some(image)) => return Ok(image),
            Ok(None) => {}
            Err(error) => tracing::warn!(target: "game", "{error}; 使用内置启动图"),
        }
    }
    image::load_from_memory(include_bytes!("../../../assets/engine_splash.png"))
        .map(|image| image.to_rgba8())
        .map_err(|error| format!("内置启动图解码失败: {error}"))
}

pub fn icon_image(root: Option<&Path>) -> image::RgbaImage {
    if let Some(root) = root {
        match load_icon(root) {
            Ok(Some(image)) => return image,
            Ok(None) => {}
            Err(error) => tracing::warn!(target: "window", "{error}; 使用内置图标"),
        }
    }
    image::load_from_memory(include_bytes!("../../../assets/engine_icon.png"))
        .expect("内置图标 engine/assets/engine_icon.png 解析失败")
        .to_rgba8()
}

#[derive(Default, Debug, PartialEq, Eq)]
pub enum Stage {
    #[default]
    AwaitingPresent,
    ReadyToLoad,
}

impl Stage {
    pub fn observe(&mut self, outcome: RenderOutcome) {
        if matches!(outcome, RenderOutcome::Presented) {
            *self = Self::ReadyToLoad;
        }
    }
}

pub struct Splash {
    width: u32,
    height: u32,
    pub stage: Stage,
}

impl Splash {
    pub fn upload(renderer: &mut Renderer, image: image::RgbaImage) -> Result<Self, String> {
        let (width, height) = image.dimensions();
        renderer
            .upload_texture_rgba8(TEXTURE_KEY, width, height, image.as_raw())
            .map_err(|error| format!("splash_image 上传失败: {error}"))?;
        Ok(Self {
            width,
            height,
            stage: Stage::default(),
        })
    }

    pub fn mesh(&self, width: u32, height: u32) -> SceneMesh {
        SceneMesh {
            vertices: quad(self.width, self.height, width, height).to_vec(),
            draws: vec![SceneDraw::screen_ui(1, 0, 6)],
            paths: vec![TEXTURE_KEY.to_owned()],
            path_handles: Vec::new(),
            world_sprites: Vec::new(),
            world_sprite_draws: Vec::new(),
            font_atlas: None,
            custom_font_atlases: Vec::new(),
            vector_vertices: Vec::new(),
            vector_glyphs: Vec::new(),
            vector_segments: Vec::new(),
        }
    }
}

fn quad(image_width: u32, image_height: u32, width: u32, height: u32) -> [OverlayVertex; 6] {
    let (width, height) = (width.max(1) as f32, height.max(1) as f32);
    let scale = (width / image_width as f32).min(height / image_height as f32);
    let x = image_width as f32 * scale / width;
    let y = image_height as f32 * scale / height;
    let vertex = |position, uv| OverlayVertex {
        position,
        uv,
        color: [1.0; 4],
        sdf: 0,
    };
    [
        vertex([-x, y], [0.0, 0.0]),
        vertex([x, y], [1.0, 0.0]),
        vertex([x, -y], [1.0, 1.0]),
        vertex([-x, y], [0.0, 0.0]),
        vertex([x, -y], [1.0, 1.0]),
        vertex([-x, -y], [0.0, 1.0]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contain_quad_preserves_aspect_in_landscape_and_portrait() {
        for (iw, ih, w, h) in [
            (1600, 900, 1280, 720),
            (1600, 900, 720, 1280),
            (900, 1600, 1280, 720),
            (900, 1600, 720, 1280),
        ] {
            let vertices = quad(iw, ih, w, h);
            let x = vertices[1].position[0];
            let y = vertices[1].position[1];
            assert!((x * w as f32 / (y * h as f32) - iw as f32 / ih as f32).abs() < 0.0001);
            assert!((x.max(y) - 1.0).abs() < 0.0001);
            assert!(x <= 1.0 && y <= 1.0);
            assert_eq!(vertices[0].uv, [0.0, 0.0]);
            assert_eq!(vertices[2].uv, [1.0, 1.0]);
        }
    }

    #[test]
    fn only_successful_present_unlocks_initialization() {
        let mut stage = Stage::default();
        for outcome in [
            RenderOutcome::Retry,
            RenderOutcome::Skipped,
            RenderOutcome::Fatal,
        ] {
            stage.observe(outcome);
            assert_eq!(stage, Stage::AwaitingPresent);
        }
        stage.observe(RenderOutcome::Presented);
        assert_eq!(stage, Stage::ReadyToLoad);
    }

    #[test]
    fn paths_reject_escape_and_windows_absolute_paths() {
        for path in [
            "/tmp/a.png",
            "C:\\a.png",
            "C:a.png",
            "\\\\server\\a.png",
            "..\\a.png",
            "assets/../a.png",
            "",
            ".",
        ] {
            for key in ["splash_image", "icon_image"] {
                let error = relative_path(key, path).expect_err(path);
                assert!(error.contains(key), "{error}");
            }
        }
        for key in ["splash_image", "icon_image"] {
            assert_eq!(
                relative_path(key, "assets\\art\\a.jpg").unwrap(),
                PathBuf::from("assets/art/a.jpg")
            );
        }
    }

    #[test]
    fn embedded_images_load_without_project() {
        let splash = startup_image(None).unwrap();
        let icon = icon_image(None);
        assert!(splash.width() > 0 && splash.height() > 0);
        assert!(icon.width() > 0 && icon.height() > 0);
        // Default images use exactly the same presentation gate as overrides.
        assert_eq!(Stage::default(), Stage::AwaitingPresent);
    }
}
