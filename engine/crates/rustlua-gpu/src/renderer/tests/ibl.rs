//! P3-R03-3：镜面 IBL 的 GPU 像素门——粗糙度必须真的选到不同的预过滤 mip。
//!
//! 场景刻意做成"只剩间接镜面"：金属度 1（没有漫反射）、方向光强度 0、SH 全零。
//! 环境 cubemap 的三级 mip 分别是纯红/纯绿/纯蓝，于是一个金属地平面的中心像素颜色
//! 就直接暴露它采样了哪一级 mip：
//!
//! - roughness=0.05 → lod≈0.1 → 红；
//! - roughness=0.5  → lod=1.0 → 绿；
//! - roughness=1.0  → lod=2.0 → 蓝。
//!
//! 这条用例同时验证上传/绑定/`EnvBRDFApprox`/mip 映射四段接线；它与 R03-4 的
//! `wp0_ibl_golden`（真实 `studio.rlenv` + tier 报告）分工不同。

use super::*;
use crate::RenderOutcome;
use engine::host::RuntimeScene;
use rustlua_assets::env_asset::{EnvironmentAsset, EnvironmentMip};
use rustlua_camera::camera::Viewport;
use rustlua_infra::games_dir;
use rustlua_render::render_settings::{EnvironmentSettings, IblMode, RenderSettings, ToggleMode};
use rustlua_scene::scene::{Scene, SceneEntityId};

const SIZE: u32 = 96;
const METAL_MATERIALS: [&str; 3] = [
    "assets/materials/p3-r03-metal-smooth.material",
    "assets/materials/p3-r03-metal-mid.material",
    "assets/materials/p3-r03-metal-rough.material",
];

/// 三级 mip 分别为红/绿/蓝的常量环境；`radiance` 是线性值，LDR 输出会再编码到 sRGB。
fn mip_gradient_environment() -> EnvironmentAsset {
    let colors = [
        [1.0, 0.0, 0.0, 1.0],
        [0.0, 1.0, 0.0, 1.0],
        [0.0, 0.0, 1.0, 1.0],
    ];
    let face_size = 4;
    let mips = colors
        .iter()
        .enumerate()
        .map(|(level, color)| {
            let size = face_size >> level;
            let texels = (size * size) as usize;
            EnvironmentMip::new(size, std::array::from_fn(|_| vec![*color; texels]))
        })
        .collect();
    EnvironmentAsset {
        source_hash: 0x1b1,
        face_size,
        sh: [0.0; 27],
        mips,
    }
}

/// 渲染一次金属地面并读回中心像素。中心像素对应相机目标点 `(0,0,0)`，必然落在地面上。
fn render_metal_center(
    renderer: &mut Renderer,
    root: &std::path::Path,
    target: &wgpu::Texture,
    view: &wgpu::TextureView,
    material: &str,
) -> [u8; 3] {
    let mut scene = Scene::new("ibl-roughness");
    let light = SceneEntityId(1);
    let ground = SceneEntityId(2);
    let camera = SceneEntityId(3);
    for (id, name) in [(light, "Sun"), (ground, "Ground"), (camera, "Camera")] {
        scene.add_entity(id, name, None).expect("add entity");
    }
    // 强度 0：把直射光与阴影从画面里拿掉，读回只反映间接镜面。
    scene.entities.get_mut(&light).unwrap().components.insert(
        "Light".into(),
        "direction=0,-1,0;color=1,1,1;intensity=0".into(),
    );
    scene.entities.get_mut(&ground).unwrap().components.insert(
        "Transform".into(),
        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    scene.entities.get_mut(&ground).unwrap().components.insert(
        "MeshRenderer".into(),
        format!("mesh=__ground_plane__;material={material}"),
    );
    scene.entities.get_mut(&camera).unwrap().components.insert(
        "Camera".into(),
        "projection=perspective;position=0,3,6;target=0,0,0;fov=60;near=0.1;far=100".into(),
    );

    let mut runtime = RuntimeScene::from_scene_edit(Some(root), scene).expect("IBL runtime");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let extracted = runtime.scene_3d(viewport).expect("IBL 3D extraction");
    let mesh = runtime.vertices(SIZE, SIZE).expect("IBL 2D extraction");
    assert!(matches!(
        renderer.render_into_texture(&mesh, Some(&extracted), view, SIZE, SIZE),
        RenderOutcome::Presented
    ));
    let pixels = renderer
        .copy_texture_to_cpu(target, SIZE, SIZE)
        .expect("IBL readback");
    let index = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
    [pixels[index], pixels[index + 1], pixels[index + 2]]
}

/// P3-R03-3 GPU：roughness 0.05/0.5/1 必须分别采到红/绿/蓝 mip。
#[test]
fn specular_ibl_roughness_selects_the_prefiltered_mip() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let Some(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()
    else {
        return gpu_skipped("request_adapter");
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rustlua-ibl-pixel-test-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let root = games_dir().join("examples").join("game_api");
    let mut renderer =
        Renderer::new_offscreen(device.clone(), queue, Some(root.clone()), false, false)
            .expect("offscreen renderer");
    renderer.set_render_settings(RenderSettings {
        pbr: true,
        ibl: ToggleMode::On,
        ..RenderSettings::default()
    });
    let environment = mip_gradient_environment();
    renderer
        .set_environment_asset(
            &environment,
            EnvironmentSettings {
                intensity: 1.0,
                rotation: 0.0,
            },
        )
        .expect("upload mip gradient environment");
    assert_eq!(
        renderer.ibl_mode(),
        IblMode::Full,
        "有 cubemap 时镜面 IBL 必须处于 Full 档"
    );

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-ibl-pixel-target"),
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
    let colors: Vec<[u8; 3]> = METAL_MATERIALS
        .iter()
        .map(|material| render_metal_center(&mut renderer, &root, &target, &view, material))
        .collect();
    let [smooth, mid, rough] = [colors[0], colors[1], colors[2]];
    assert!(
        smooth[0] > smooth[1] + 40 && smooth[0] > smooth[2] + 40,
        "smooth 应采到红色 mip0：{smooth:?}"
    );
    assert!(
        mid[1] > mid[0] + 40 && mid[1] > mid[2] + 40,
        "mid 应采到绿色 mip1：{mid:?}"
    );
    assert!(
        rough[2] > rough[0] + 40 && rough[2] > rough[1] + 40,
        "rough 应采到蓝色 mip2：{rough:?}"
    );
}
