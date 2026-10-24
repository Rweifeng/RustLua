//! P3-E02：材质编辑器的**实时预览**通道。
//!
//! 编辑器在保存前把内存里的 `MaterialAsset` 交给渲染器，渲染器用它与从磁盘读到的
//! `.material` 走同一条 `GpuMaterial` 路径。这条门钉住两件事：
//!
//! 1. 预览确实参与着色（同一场景、同一相机，只有预览材质不同 → 像素必须变）；
//! 2. 清除预览后**逐字节回到磁盘内容**（预览条目不能污染材质缓存）。

use super::*;
use crate::RenderOutcome;
use engine::host::RuntimeScene;
use rustlua_camera::camera::Viewport;
use rustlua_infra::games_dir;
use rustlua_render::mesh3d::MaterialAsset;
use rustlua_render::render_settings::RenderSettings;
use rustlua_scene::scene::{Scene, SceneEntityId};

const SIZE: u32 = 96;
const MATERIAL: &str = "assets/materials/p3-r02-flat.material";

fn preview_asset() -> MaterialAsset {
    MaterialAsset {
        name: Some("editor-preview".to_owned()),
        base_color: [0.95, 0.1, 0.1, 1.0],
        metallic: 0.0,
        roughness: 0.15,
        ..MaterialAsset::default()
    }
}

/// 渲染一次挂 `MATERIAL` 的地面，读回中心像素。
fn render_ground(
    renderer: &mut Renderer,
    root: &std::path::Path,
    target: &wgpu::Texture,
    view: &wgpu::TextureView,
) -> [u8; 3] {
    let mut scene = Scene::new("material-preview");
    let light = SceneEntityId(1);
    let ground = SceneEntityId(2);
    let camera = SceneEntityId(3);
    for (id, name) in [(light, "Sun"), (ground, "Ground"), (camera, "Camera")] {
        scene.add_entity(id, name, None).expect("add entity");
    }
    scene.entities.get_mut(&light).unwrap().components.insert(
        "Light".into(),
        "direction=0.8,-0.35,0.2;color=1,1,1;intensity=1".into(),
    );
    scene.entities.get_mut(&ground).unwrap().components.insert(
        "Transform".into(),
        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    scene.entities.get_mut(&ground).unwrap().components.insert(
        "MeshRenderer".into(),
        format!("mesh=__ground_plane__;material={MATERIAL}"),
    );
    scene.entities.get_mut(&camera).unwrap().components.insert(
        "Camera".into(),
        "projection=perspective;position=0,3,6;target=0,0,0;fov=60;near=0.1;far=100".into(),
    );

    let mut runtime = RuntimeScene::from_scene_edit(Some(root), scene).expect("preview runtime");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let extracted = runtime.scene_3d(viewport).expect("preview 3D extraction");
    let mesh = runtime.vertices(SIZE, SIZE).expect("preview 2D extraction");
    assert!(matches!(
        renderer.render_into_texture(&mesh, Some(&extracted), view, SIZE, SIZE),
        RenderOutcome::Presented
    ));
    let pixels = renderer
        .copy_texture_to_cpu(target, SIZE, SIZE)
        .expect("preview readback");
    let index = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
    [pixels[index], pixels[index + 1], pixels[index + 2]]
}

#[test]
fn in_memory_material_preview_shades_and_clears_cleanly() {
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
        label: Some("rustlua-material-preview-test-device"),
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
    renderer.set_render_settings(RenderSettings::default().with_pbr(true));
    assert!(renderer.pbr_pipelines_ready(), "PBR 变体没有编译");

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-material-preview-target"),
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
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    let disk = render_ground(&mut renderer, &root, &texture, &view);
    let preview = preview_asset();
    renderer.set_material_preview(Some((MATERIAL, &preview)));
    let previewed = render_ground(&mut renderer, &root, &texture, &view);
    let delta = (0..3)
        .map(|channel| u32::from(disk[channel].abs_diff(previewed[channel])))
        .max()
        .unwrap_or(0);
    assert!(
        delta >= 8,
        "内存预览没有参与着色：磁盘 {disk:?} vs 预览 {previewed:?}"
    );

    renderer.set_material_preview(None);
    let restored = render_ground(&mut renderer, &root, &texture, &view);
    assert_eq!(
        restored, disk,
        "清除预览后必须逐字节回到磁盘材质：磁盘 {disk:?} vs 清除后 {restored:?}"
    );
    println!("material_preview_ok disk={disk:?} preview={previewed:?} delta={delta} restored=true");
}
