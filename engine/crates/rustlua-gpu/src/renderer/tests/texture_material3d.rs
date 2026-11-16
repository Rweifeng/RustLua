//! M82-B2：3D 材质消费门——`__texture__?<id>` 生成贴图作为网格 base_color 贴图。
//!
//! 材质池与 2D 缓存是两套（`albedo_textures` vs `texture_paths`）：本门证明宿主登记进
//! `Renderer::register_generated_texture` 的像素，经 `MeshRenderer.material = "__texture__?<id>"`
//! 真的画到了网格表面（与默认材质/未登记哨兵的画面对比），且确定性、计数达标。
//!
//! 反面控制：把登记那行删掉 ⇒ 哨兵解析不到、材质退默认，`registered vs unregistered`
//! 那条会红；清空 drawable ⇒ `draw_calls>=1` 会红（计数断言在采样之前）。

use super::*;
use rustlua_camera::camera::{Camera3D, DEFAULT_3D_CLEAR, RenderView, Viewport};
use rustlua_math::math3d::Aabb3;
use rustlua_render::frame::{BoundsSource, Scene3dBounds, Scene3dDrawable};
use rustlua_render::light::Light;
use rustlua_render::primitives::PrimitiveSpec;
use rustlua_render::render_settings::{RenderSettings, ToggleMode};
use rustlua_render::texture_gen::{TextureKind, TextureSpec};

const SIZE: u32 = 64;
const EYE: Vec3 = Vec3::new(2.5, 2.0, 2.5);
const CENTER: Vec3 = Vec3::new(0.0, 0.0, 0.0);
/// 生成贴图的哨兵键（不是文件路径）。
const KEY: &str = "__texture__?gate-material";

fn box_drawable(material: Option<&str>) -> Scene3dDrawable {
    let spec = PrimitiveSpec::Box {
        w: 1.0,
        h: 1.0,
        d: 1.0,
    };
    let (min, max) = spec.bounds();
    Scene3dDrawable {
        entity: rustlua_ecs::spatial_transform::SceneEntityId(2),
        name: "TexturedBox".to_owned(),
        mesh_path: spec.mesh_path(),
        material: material.map(str::to_owned),
        mesh_handle: None,
        model: Mat4::identity(),
        palette: Vec::new(),
        instances: Vec::new(),
        bounds: Scene3dBounds {
            local_aabb: Aabb3 {
                min: Vec3::new(min[0], min[1], min[2]),
                max: Vec3::new(max[0], max[1], max[2]),
            },
            source: BoundsSource::Fallback,
        },
    }
}

fn empty_mesh() -> SceneMesh {
    SceneMesh {
        vertices: Vec::new(),
        draws: Vec::new(),
        world_sprites: Vec::new(),
        world_sprite_draws: Vec::new(),
        paths: Vec::new(),
        path_handles: Vec::new(),
        font_atlas: None,
        custom_font_atlases: Vec::new(),
        vector_vertices: Vec::new(),
        vector_glyphs: Vec::new(),
        vector_segments: Vec::new(),
    }
}

/// 亮红/亮绿棋盘：贴到盒子上必须有明显红绿区域（默认材质是灰色）。
fn checker_texture() -> rustlua_render::texture::TextureAsset {
    let mut spec = TextureSpec::default_for(TextureKind::Checker);
    spec.size = 8;
    spec.scale = 2.0;
    spec.colors = vec![[255, 0, 0, 255], [0, 255, 0, 255]];
    spec.generate().expect("generate checker")
}

fn max_channel_diff(a: &[u8], b: &[u8]) -> u8 {
    a.iter()
        .zip(b)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

struct Frame {
    pixels: Vec<u8>,
    draw_calls: u64,
    dropped: u64,
}

struct Fixture {
    renderer: Renderer,
    viewport: Viewport,
    target: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Fixture {
    fn render(&mut self, drawables: Vec<Scene3dDrawable>) -> Frame {
        let camera = Camera3D::perspective(EYE, CENTER, std::f32::consts::FRAC_PI_2, 0.1, 200.0)
            .expect("camera");
        let view = RenderView::main_3d(camera, self.viewport, DEFAULT_3D_CLEAR);
        self.renderer.set_frame_views(FrameViews {
            main: Some(view),
            overlay: None,
        });
        let scene = Scene3d {
            camera,
            view,
            light: Light::default(),
            point_lights: Vec::new(),
            drawables,
        };
        let outcome = self.renderer.render_into_texture(
            &empty_mesh(),
            Some(&scene),
            &self.view,
            SIZE,
            SIZE,
            true,
        );
        assert!(
            matches!(outcome, RenderOutcome::Presented),
            "材质门帧必须出图"
        );
        let metrics = self.renderer.last_frame_metrics();
        let pixels = self
            .renderer
            .copy_texture_to_cpu(&self.target, SIZE, SIZE)
            .expect("readback");
        Frame {
            pixels,
            draw_calls: metrics.draw_calls,
            dropped: metrics.dropped_draws,
        }
    }
}

fn fixture(label: &str) -> Option<Fixture> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("rustlua-test-texture-material3d-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let mut renderer =
        Renderer::new_offscreen(device.clone(), queue, None, false, false).expect("offscreen");
    renderer.set_render_settings(
        RenderSettings::default()
            .with_pbr(true)
            .with_hdr(ToggleMode::On),
    );
    renderer.set_post_fx(0.0, 0.0);
    renderer.set_water_refraction_gain(0.0);
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    renderer.freeze_clock_for_tests(4.2);
    Some(Fixture {
        renderer,
        viewport,
        target,
        view,
    })
}

#[test]
fn generated_texture_drives_3d_material() {
    let Some(mut fx) = fixture("rustlua-test-texture-material3d") else {
        return gpu_skipped("request_adapter");
    };
    // 默认材质对照帧（盒子是灰色）。
    let base = fx.render(vec![box_drawable(None)]);
    assert!(base.draw_calls >= 1, "对照帧必须产生 draw");

    fx.renderer
        .register_generated_texture(KEY, checker_texture());
    let textured = fx.render(vec![box_drawable(Some(KEY))]);
    // 计数判据在采样之前：清空 drawable ⇒ draw_calls==0 要红。
    assert!(textured.draw_calls >= 1, "纹理材质帧必须产生 draw");
    assert_eq!(textured.dropped, 0, "纹理材质不许被静默丢弃");

    let delta = max_channel_diff(&base.pixels, &textured.pixels);
    assert!(
        delta >= 32,
        "生成贴图必须真的改变网格表面（红绿 vs 灰）：max_delta={delta}"
    );

    // 确定性：同材质再渲染一帧逐字节相同（时钟已冻结）。
    let again = fx.render(vec![box_drawable(Some(KEY))]);
    assert_eq!(textured.pixels, again.pixels, "同材质两帧必须逐字节相同");
}

#[test]
fn unregistered_sentinel_falls_back_and_differs_from_registered() {
    let Some(mut fx) = fixture("rustlua-test-texture-material3d-unreg") else {
        return gpu_skipped("request_adapter");
    };
    // 未登记的哨兵：解析不到像素 → 降级（告警）但照常画，不崩。
    let unregistered = fx.render(vec![box_drawable(Some("__texture__?999"))]);
    assert!(
        unregistered.draw_calls >= 1,
        "未登记哨兵也必须画（降级默认材质，不是消失）"
    );

    // 登记同一 id 的真实像素后画面必须变——证明「登记」这件事是生效条件，不是空操作。
    fx.renderer
        .register_generated_texture("__texture__?999", checker_texture());
    let registered = fx.render(vec![box_drawable(Some("__texture__?999"))]);
    let delta = max_channel_diff(&unregistered.pixels, &registered.pixels);
    assert!(
        delta >= 32,
        "登记前后画面必须不同（登记确实是生效条件）：max_delta={delta}"
    );
}
