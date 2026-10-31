//! 导入设置里的"模型缩放"真的乘进了**画出去的模型矩阵**。
//!
//! 为什么不能只测"读盘读到了缩放"：`resources.meta` → `model_scale_for` → 模型矩阵
//! 三段里，只有最后一段错了才是最阴的——设置读到了、面板显示对了，而画出来还是原尺寸，
//! 所有"读数"都是绿的。所以这里判的是**矩阵**：缩放后基向量被乘了、平移列原样保留。
//!
//! 网格用的是**地面哨兵**（`GROUND_PLANE_SENTINEL`）：它是渲染器自己合成的，
//! 不需要磁盘上真的有一份 `.glb`，因此这条门不依赖任何工程资源。

use super::*;
use rustlua_assets::import_settings::{ModelImportSettings, ResourceMeta};
use rustlua_camera::camera::{Camera3D, DEFAULT_3D_CLEAR, RenderView, Viewport};
use rustlua_math::math3d::{Mat4, Vec3};
use rustlua_render::mesh3d::GROUND_PLANE_SENTINEL;
use std::path::{Path, PathBuf};

const SIZE: u32 = 32;
/// 登记在元数据段里的缩放倍数。
const SCALE: f32 = 0.5;

fn temp_root(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or(0);
    dir.push(format!(
        "rustlua-modelscale-{tag}-{}-{nonce}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时工程根");
    dir
}

/// 走**生产端**写这份文件（`ResourceMeta` 自己）：门因此同时覆盖"写出来的东西运行时读得回"。
fn write_scale(root: &Path, key: &str, scale: f32) {
    let mut meta = ResourceMeta::open(root).expect("打开元数据段");
    meta.set_model_settings(key, ModelImportSettings::new(scale))
        .expect("写入模型缩放");
    meta.save().expect("保存元数据段");
}

/// 缩放只动**基向量**（前 3 列），平移列原样保留——这正是 `model × diag(s,s,s,1)`。
#[test]
fn the_scale_only_touches_the_basis_columns() {
    let model = Mat4::translation(Vec3::new(1.0, 2.0, 3.0));
    let scaled = scaled_model(model, SCALE);
    assert!((scaled.columns[0][0] - SCALE).abs() < 1e-6, "基向量 x");
    assert!((scaled.columns[1][1] - SCALE).abs() < 1e-6, "基向量 y");
    assert!((scaled.columns[2][2] - SCALE).abs() < 1e-6, "基向量 z");
    assert_eq!(
        scaled.columns[3], model.columns[3],
        "平移列不该被缩放（否则模型会朝原点缩）"
    );
    // 1.0 是恒等：没登记过的模型不许变形。
    assert_eq!(scaled_model(model, 1.0).columns, model.columns);
}

/// 剔除球按**缩放后**的矩阵算：放大两倍，包围球半径也得跟着长两倍。
///
/// 这条钉住"缩放发生在剔除之前"：缩放后再剔除，缩大了的模型才不会被按旧尺寸剔掉。
#[test]
fn the_culling_radius_follows_the_scaled_matrix() {
    let model = Mat4::translation(Vec3::new(0.0, 0.0, -1.0));
    let doubled = scaled_model(model, 2.0);
    assert!(
        (max_model_scale(doubled) - 2.0).abs() < 1e-6,
        "剔除球半径没跟着缩放长：{}",
        max_model_scale(doubled)
    );
}

/// 端到端：登记在 `resources.meta` 里的缩放真的落到了这一帧的模型矩阵上。
#[test]
fn the_documented_scale_reaches_the_model_matrix() {
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
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("rustlua-model-scale-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");

    let root = temp_root("scale");
    write_scale(&root, GROUND_PLANE_SENTINEL, SCALE);
    let mut renderer = Renderer::new_offscreen(
        device.clone(),
        queue.clone(),
        Some(root.clone()),
        false,
        false,
    )
    .expect("offscreen renderer");
    let size = SIZE;
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 0.0, 4.0),
        Vec3::ZERO,
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("perspective camera");
    let viewport = Viewport::new(size as f32, size as f32).expect("viewport");
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light::default(),
        point_lights: Vec::new(),
        drawables: vec![rustlua_render::frame::Scene3dDrawable {
            entity: rustlua_scene::scene::SceneEntityId(1),
            name: "Ground".to_owned(),
            mesh_path: GROUND_PLANE_SENTINEL.to_owned(),
            material: None,
            mesh_handle: None,
            model: Mat4::translation(Vec3::new(0.0, 0.0, -1.0)),
            palette: Vec::new(),
            bounds: rustlua_render::frame::Scene3dBounds {
                local_aabb: rustlua_math::math3d::Aabb3 {
                    min: Vec3::new(-4.0, -4.0, -0.1),
                    max: Vec3::new(4.0, 4.0, 0.1),
                },
                source: rustlua_render::frame::BoundsSource::Fallback,
            },
        }],
    };

    let draws = renderer.frame_3d_draws(&scene);
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(draws.len(), 1, "地面网格应当出一个 draw");
    let model = draws[0].models[0];
    assert!(
        (model.columns[0][0] - SCALE).abs() < 1e-6,
        "缩放没进模型矩阵：x={}",
        model.columns[0][0]
    );
    assert!((model.columns[1][1] - SCALE).abs() < 1e-6, "y");
    assert!((model.columns[2][2] - SCALE).abs() < 1e-6, "z");
    assert!(
        (model.columns[3][2] - (-1.0)).abs() < 1e-6,
        "平移列不该被缩放：{:?}",
        model.columns[3]
    );
}

/// 没登记过的模型**不许**变形（默认 1.0）。
#[test]
fn an_undocumented_model_keeps_its_size() {
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
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("rustlua-model-scale-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");

    let root = temp_root("undocumented");
    let mut renderer = Renderer::new_offscreen(
        device.clone(),
        queue.clone(),
        Some(root.clone()),
        false,
        false,
    )
    .expect("offscreen renderer");
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 0.0, 4.0),
        Vec3::ZERO,
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("perspective camera");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let scene = Scene3d {
        camera,
        view: RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
        light: Light::default(),
        point_lights: Vec::new(),
        drawables: vec![rustlua_render::frame::Scene3dDrawable {
            entity: rustlua_scene::scene::SceneEntityId(1),
            name: "Ground".to_owned(),
            mesh_path: GROUND_PLANE_SENTINEL.to_owned(),
            material: None,
            mesh_handle: None,
            model: Mat4::translation(Vec3::new(0.0, 0.0, -1.0)),
            palette: Vec::new(),
            bounds: rustlua_render::frame::Scene3dBounds {
                local_aabb: rustlua_math::math3d::Aabb3 {
                    min: Vec3::new(-4.0, -4.0, -0.1),
                    max: Vec3::new(4.0, 4.0, 0.1),
                },
                source: rustlua_render::frame::BoundsSource::Fallback,
            },
        }],
    };
    let draws = renderer.frame_3d_draws(&scene);
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(draws.len(), 1);
    let model = draws[0].models[0];
    assert!(
        (model.columns[0][0] - 1.0).abs() < 1e-6,
        "没登记过的模型不许变形：{}",
        model.columns[0][0]
    );
}
