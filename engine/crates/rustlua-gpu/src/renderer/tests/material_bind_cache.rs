//! P4-①：3D 材质绑定组缓存（`arena::MaterialBindKey`）的回归门。
//!
//! 这条门钉两件事，缺一不可：
//!
//! 1. **稳态零创建**：同一场景连续渲染时，第二帧起不应再 `create_bind_group`（`created == 0`
//!    且全部命中复用）。这是"缓存真的接上了"的证据——只看画面看不出来，省的是 CPU 侧开销。
//! 2. **画面对得上**：第 3 帧与第 2 帧逐字节相同。缓存复用最怕的是"键漏了一个输入"，
//!    导致绑到上一帧的贴图/缓冲；只有像素等价才说明键覆盖了所有影响绑定的输入。
//!
//! 用软件适配器（`force_fallback_adapter`）跑，与同目录其它 GPU 用例一致：没有 GPU 的机器上
//! 按 `gpu_skipped` 记录跳过，而不是静默通过。

use super::*;
use crate::RenderOutcome;
use engine::host::RuntimeScene;
use rustlua_camera::camera::Viewport;
use rustlua_infra::games_dir;
use rustlua_render::render_settings::RenderSettings;
use rustlua_scene::scene::{Scene, SceneEntityId};

const SIZE: u32 = 64;
const MATERIAL: &str = "assets/materials/p3-r02-flat.material";

fn build_runtime(root: &std::path::Path) -> RuntimeScene {
    let mut scene = Scene::new("material-bind-cache");
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
    RuntimeScene::from_scene_edit(Some(root), scene).expect("bind-cache runtime")
}

fn render_once(
    renderer: &mut Renderer,
    runtime: &mut RuntimeScene,
    viewport: Viewport,
    target: &wgpu::Texture,
    view: &wgpu::TextureView,
) -> Vec<u8> {
    let extracted = runtime.scene_3d(viewport).expect("3D extraction");
    let mesh = runtime.vertices(SIZE, SIZE).expect("2D extraction");
    assert!(matches!(
        renderer.render_into_texture(&mesh, Some(&extracted), view, SIZE, SIZE),
        RenderOutcome::Presented
    ));
    renderer
        .copy_texture_to_cpu(target, SIZE, SIZE)
        .expect("readback")
}

#[test]
fn material_bind_groups_are_reused_on_steady_frames() {
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
        label: Some("rustlua-material-bind-cache-test-device"),
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
        label: Some("rustlua-material-bind-cache-target"),
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
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let mut runtime = build_runtime(&root);

    // 第 1 帧：缓存冷，必须真的创建（同时把懒加载的贴图/材质推进稳态）。
    let _first = render_once(&mut renderer, &mut runtime, viewport, &texture, &view);
    let first_created = renderer.material_bind_created;
    let first_draws = first_created + renderer.material_bind_reused;
    assert!(
        first_draws > 0,
        "这一帧一条 3D draw 都没有，测不到材质绑定组"
    );
    assert_eq!(
        first_created, first_draws,
        "首帧缓存应为冷（created={first_created} draws={first_draws}）"
    );

    // 第 2、3 帧：布局不变 ⇒ 键相同 ⇒ 一条绑定组都不该新建。
    let second = render_once(&mut renderer, &mut runtime, viewport, &texture, &view);
    assert_eq!(
        renderer.material_bind_created, 0,
        "稳态帧仍在创建材质绑定组（reused={}）",
        renderer.material_bind_reused
    );
    assert_eq!(
        renderer.material_bind_reused, first_draws,
        "复用次数与 draw 数对不上：另有 draw 走了未计数的分支"
    );
    assert!(
        renderer.material_bind_reported,
        "缓存读数日志没有打出（真机上就分不清命没命中）"
    );

    let third = render_once(&mut renderer, &mut runtime, viewport, &texture, &view);
    assert_eq!(
        renderer.material_bind_created, 0,
        "第三帧又开始重建绑定组（arena 代次/键不稳定）"
    );
    // 画面等价：键漏了输入（贴图槽、缓冲段）时这里会先红。
    assert_eq!(
        second, third,
        "复用绑定组后画面发生变化：缓存键漏了某个影响绑定的输入"
    );
    // 画面得真的画了东西：全画面只有一个颜色说明场景没出图，上面的等价断言就成了空话。
    let distinct = third
        .chunks_exact(4)
        .map(|pixel| (pixel[0], pixel[1], pixel[2]))
        .collect::<std::collections::BTreeSet<_>>();
    assert!(
        distinct.len() > 1,
        "目标只有一种颜色（{:?}），场景没画出来",
        distinct.iter().next()
    );
    println!(
        "material_bind_cache_ok draws={first_draws} first_created={first_created} steady_created=0 colors={} pixels={}",
        distinct.len(),
        third.len()
    );
}
