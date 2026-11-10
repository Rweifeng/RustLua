//! P3-R162–165：现代材质模型四瓣（清漆 / SSS / 虹彩 / 绒毛）的像素门。
//!
//! 场景、相机、地面网格与光照全部固定，只换**预览材质**的四组新参数。钉三件事：
//!
//! 1. **预览通道活着**：磁盘材质与内存基线必须渲染出不同像素（否则后面三条差值全是假绿）；
//! 2. **每瓣真的着色**：单独打开一瓣时整幅图的最大逐通道差必须过阈值。阈值取实测值的
//!    一半左右（对照值是三瓣全 0 的基线），既不容忍"1 LSB 舍入假绿"，也不因驱动/后端
//!    差异抖红；
//! 3. **预览通道可复原**：清除预览后逐字节回到磁盘材质（新 uniform 行没有污染材质缓存）。
//!
//! "四瓣全 0 时逐字节等价于改动前"不在本测试里证——那需要改动前的基线；它由仓库既有的
//! pbr golden 门盯（旧材质文件不含新字段 ⇒ 解析后恒 0 ⇒ golden 必须不变）。
//!
//! 实测（2026-11-07，Windows + swiftshader 回退适配器，`RUSTLUA_REQUIRE_GPU=1`）：
//! `liveness=97 clearcoat=99 sss=14 iridescence=43 sheen=141 sheen_shape=34`，
//! 对照值（四瓣全 0）恒为 0；门槛取实测的约一半，落在两者之间。复现：
//! `cargo test -p rustlua-gpu --lib material_lobes -- --test-threads=1 --nocapture`

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

/// 蚁种甲壳的基线参数（docs/03-art-3d.md §4 Atta：roughness 0.42、metallic 0）。
fn chitin() -> MaterialAsset {
    MaterialAsset {
        name: Some("chitin".to_owned()),
        base_color: [0.16, 0.07, 0.03, 1.0],
        metallic: 0.0,
        roughness: 0.42,
        ..MaterialAsset::default()
    }
}

/// 渲染一次挂 `MATERIAL` 的地面，返回整幅 RGBA8 像素（不只中心点：清漆/虹彩在
/// 掠射角最强，地面远端边界才是信号最大的地方）。
fn render_ground(
    renderer: &mut Renderer,
    root: &std::path::Path,
    target: &wgpu::Texture,
    view: &wgpu::TextureView,
) -> Vec<u8> {
    let mut scene = Scene::new("material-lobes");
    let light = SceneEntityId(1);
    let ground = SceneEntityId(2);
    let camera = SceneEntityId(3);
    for (id, name) in [(light, "Sun"), (ground, "Ground"), (camera, "Camera")] {
        scene.add_entity(id, name, None).expect("add entity");
    }
    scene.entities.get_mut(&light).unwrap().components.insert(
        "Light".into(),
        // 主光与相机关于地面法线**镜像对齐**（camera z=+6 ⇒ l=(0,0.447,-0.894)），
        // 于是画面中心像素的 n·h = 1。清漆瓣的 GGX α = roughness² 很小，针尖高光只在
        // n·h≈1 处有值；随便给个斜光会让这一瓣恒等于 0，门就成了假绿。
        "direction=0,-0.4472,0.8944;color=1,1,1;intensity=1".into(),
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

    let mut runtime = RuntimeScene::from_scene_edit(Some(root), scene).expect("lobes runtime");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let extracted = runtime.scene_3d(viewport).expect("lobes 3D extraction");
    let mesh = runtime.vertices(SIZE, SIZE).expect("lobes 2D extraction");
    assert!(matches!(
        renderer.render_into_texture(&mesh, Some(&extracted), view, SIZE, SIZE, true),
        RenderOutcome::Presented
    ));
    renderer
        .copy_texture_to_cpu(target, SIZE, SIZE)
        .expect("lobes readback")
}

fn max_channel_delta(a: &[u8], b: &[u8]) -> u32 {
    assert_eq!(a.len(), b.len(), "两幅图必须同尺寸");
    a.iter()
        .zip(b)
        .map(|(x, y)| u32::from(x.abs_diff(*y)))
        .max()
        .unwrap_or(0)
}

/// 用预览材质渲染一次（独立 fn 而不是闭包：闭包会把 `renderer` 的可变借用钉死，
/// 后面"清除预览"的那次渲染就借不到了）。
fn shade(
    renderer: &mut Renderer,
    root: &std::path::Path,
    target: &wgpu::Texture,
    view: &wgpu::TextureView,
    material: MaterialAsset,
) -> Vec<u8> {
    renderer.set_material_preview(Some((MATERIAL, &material)));
    render_ground(renderer, root, target, view)
}

#[test]
fn clearcoat_sss_and_iridescence_lobes_shade_and_degrade_to_zero() {
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
        label: Some("rustlua-material-lobes-test-device"),
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
        label: Some("rustlua-material-lobes-target"),
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

    // 磁盘材质（无预览）：用于 ① 的"预览通道确实接上了"对照与 ⑤ 的可复原对账。
    renderer.set_material_preview(None);
    let disk = render_ground(&mut renderer, &root, &texture, &view);
    // 基线：三瓣全 0 的甲壳材质（`chitin()` 的新字段取默认 0）。
    // "零值不扰动旧路径"由仓库既有的 pbr golden 门盯（本测试的 ⑤ 另证预览通道可复原）。
    let off = shade(&mut renderer, &root, &texture, &view, chitin());

    // ① 活性对照：磁盘材质（白 / metallic 1 / roughness 1）与内存甲壳（暗棕 / 非金属）
    // 必须渲染出不同像素，否则下面三瓣的差值全是"预览没生效"的假绿。
    let liveness = max_channel_delta(&disk, &off);
    assert!(
        liveness >= 8,
        "预览材质通道没接上：磁盘与内存基线逐字节相同（最大通道差 {liveness}）"
    );

    // ② 清漆：factor 0.9 + 低粗糙度 ⇒ 镜面高光处叠加一层尖锐 GGX 瓣。
    let clearcoat = shade(
        &mut renderer,
        &root,
        &texture,
        &view,
        MaterialAsset {
            clearcoat: 0.9,
            clearcoat_roughness: 0.12,
            ..chitin()
        },
    );
    let clearcoat_delta = max_channel_delta(&off, &clearcoat);
    assert!(
        clearcoat_delta >= 40,
        "清漆瓣没有参与着色：实测最大通道差 {clearcoat_delta}"
    );

    // ③ SSS：wrap 漫反射把整片地面的漫反射项抬高（地面法线恒定 ⇒ 全图变亮）。
    let sss = shade(
        &mut renderer,
        &root,
        &texture,
        &view,
        MaterialAsset {
            sss_strength: 0.8,
            sss_thickness: 1.0,
            ..chitin()
        },
    );
    let sss_delta = max_channel_delta(&off, &sss);
    assert!(
        sss_delta >= 7,
        "SSS 瓣没有参与着色：实测最大通道差 {sss_delta}"
    );

    // ④ 虹彩：薄膜 F0 随视角变色 ⇒ 高光/掠射角出现彩色偏移。
    let iridescence = shade(
        &mut renderer,
        &root,
        &texture,
        &view,
        MaterialAsset {
            iridescence: 1.0,
            iridescence_ior: 1.8,
            iridescence_thickness_max: 620.0,
            ..chitin()
        },
    );
    let iridescence_delta = max_channel_delta(&off, &iridescence);
    assert!(
        iridescence_delta >= 20,
        "虹彩瓣没有参与着色：实测最大通道差 {iridescence_delta}"
    );

    // ⑤ 绒毛：ASA/Charlie 瓣在掠射角起绒边，同时按 max(sheen_color) 衰减基层。
    let sheen = shade(
        &mut renderer,
        &root,
        &texture,
        &view,
        MaterialAsset {
            sheen_color: [0.9, 0.85, 0.8],
            sheen_roughness: 0.6,
            ..chitin()
        },
    );
    let sheen_delta = max_channel_delta(&off, &sheen);
    assert!(
        sheen_delta >= 70,
        "绒毛瓣没有参与着色：实测最大通道差 {sheen_delta}"
    );

    // ⑤b 分布项活性：两次的 max(sheen_color) 相同 ⇒ 基层衰减逐字节相同，差值只可能
    // 来自 ASA 分布随粗糙度的变化（窄瓣把能量推向轮廓，宽瓣摊平）。只测 ⑤ 的话，
    // `distribution` 整项写错也能靠衰减蒙混过关。
    let sheen_narrow = shade(
        &mut renderer,
        &root,
        &texture,
        &view,
        MaterialAsset {
            sheen_color: [0.9, 0.85, 0.8],
            sheen_roughness: 0.15,
            ..chitin()
        },
    );
    let sheen_wide = shade(
        &mut renderer,
        &root,
        &texture,
        &view,
        MaterialAsset {
            sheen_color: [0.9, 0.85, 0.8],
            sheen_roughness: 0.9,
            ..chitin()
        },
    );
    let sheen_shape_delta = max_channel_delta(&sheen_narrow, &sheen_wide);
    assert!(
        sheen_shape_delta >= 17,
        "绒毛瓣的分布项没随粗糙度变化：实测最大通道差 {sheen_shape_delta}"
    );

    // ⑥ 清除预览后逐字节回到磁盘内容（预览条目不污染材质缓存）。
    renderer.set_material_preview(None);
    let restored = render_ground(&mut renderer, &root, &texture, &view);
    assert_eq!(restored, disk, "清除预览后必须逐字节回到磁盘材质");

    println!(
        "material_lobes_ok liveness={liveness} clearcoat={clearcoat_delta} sss={sss_delta} iridescence={iridescence_delta} sheen={sheen_delta} sheen_shape={sheen_shape_delta} restored=true"
    );
}
