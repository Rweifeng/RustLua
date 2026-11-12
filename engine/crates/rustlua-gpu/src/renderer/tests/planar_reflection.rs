//! P3-R170：水面平面反射的离线数学门与 GPU 像素门。
//!
//! 离线两条（不启动 GPU，纯矩阵性质）：
//!
//! 1. **水面平面上的点不动**：`y = level` 的点经 `VP·R` 后裁剪坐标与主矩阵逐分量相同
//!    ⇒ 水面像素的反射内容就落在它自己的 uv 上，Tonemap 用当前 uv 采样（不是 `1-u`）；
//! 2. **镜像点对应**：`(VP·R)·p` 的裁剪坐标 = `VP·(R·p)`，即镜像图里装的确实是镜像几何。
//!    逐分量比较（含 x），把"偷偷补一次 x 取反"变成必然红。
//!
//! 绕序不设离线门：镜像是反向变换（det = −1），屏幕绕序对一部分几何翻转、对另一部分
//! 保持（透视项参与），不存在单一剔除模式能服务所有几何 —— 实测单面管线覆盖 0、
//! 双面 244 像素。因此反射 pass 直接用双面变体，绕序交给像素门兜住。
//!
//! GPU 门分两相钉两件事：
//!
//! * **只画水面之上的内容**：水面 + 沉在水下的地平面 ⇒ pass 执行、draws > 0，但镜像图
//!   一个不透明像素都没有（水面高度裁剪面把水下几何丢了）。这是裁剪面的反面控制：
//!   去掉裁剪面，同样的几何会被镜像到水面之上并留下覆盖；
//! * **真的把几何画进离屏色图**：再加一块高出水面的基座 ⇒ alpha 命中掩码有覆盖且不是
//!   全黑。pass 跑了、draws 也数了但矩阵把几何推到视锥外时，只有像素回读能发现。
//!
//! 桌面构图本身（反射看起来对不对）仍走 Myrmex 实机截图。

use super::*;
use rustlua_camera::camera::Viewport;
use rustlua_camera::camera::{Camera3D, DEFAULT_3D_CLEAR, RenderView};
use rustlua_math::math3d::Aabb3;
use rustlua_render::frame::{BoundsSource, Scene3dBounds, Scene3dDrawable};
use rustlua_render::light::Light;
use rustlua_render::render_plan::RenderPass;
use rustlua_render::render_settings::{RenderSettings, ToggleMode};

const LEVEL: f32 = 0.75;
/// 基座顶面高出水面 —— 镜像图里唯一应当有覆盖的几何。
const PLINTH_HEIGHT: f32 = 1.5;
const SIZE: u32 = 64;

/// 非对称、带透视项的视图投影：恒等式会把符号错误蒙过去。
fn sample_vp() -> Mat4 {
    Mat4 {
        columns: [
            [1.3, 0.0, 0.2, 0.0],
            [0.1, 1.7, 0.0, 0.4],
            [0.0, 0.0, 1.1, 0.0],
            [0.25, 1.0, -6.0, 0.5],
        ],
    }
}

fn clip(matrix: &Mat4, point: Vec3) -> [f32; 4] {
    let v = [point.x, point.y, point.z, 1.0];
    let mut out = [0.0_f32; 4];
    for (row, value) in out.iter_mut().enumerate() {
        *value = (0..4)
            .map(|column| matrix.columns[column][row] * v[column])
            .sum();
    }
    out
}

fn assert_close(left: f32, right: f32, what: &str) {
    assert!(
        (left - right).abs() < 1e-5,
        "{what}: {left} vs {right}（差 {}）",
        (left - right).abs()
    );
}

#[test]
fn mirror_leaves_water_plane_points_in_place() {
    let main = sample_vp();
    let reflected = crate::renderer::planar_reflection_view_projection(&main, LEVEL);
    let point = Vec3::new(1.0, LEVEL, -2.0);
    let original = clip(&main, point);
    let mirrored = clip(&reflected, point);
    assert!(
        original[0].abs() > 1e-3,
        "采样点不能落在 x=0 上，否则符号错误也看不出来"
    );
    for (axis, what) in [(0, "x"), (1, "y"), (2, "z"), (3, "w")] {
        assert_close(
            mirrored[axis],
            original[axis],
            &format!("水面点的裁剪 {what}"),
        );
    }
}

#[test]
fn mirror_maps_a_world_point_onto_its_mirrored_position() {
    let main = sample_vp();
    let reflected = crate::renderer::planar_reflection_view_projection(&main, LEVEL);
    let point = Vec3::new(1.0, 3.0, -2.0);
    let mirrored_point = Vec3::new(point.x, 2.0 * LEVEL - point.y, point.z);
    let original = clip(&main, mirrored_point);
    let mirrored = clip(&reflected, point);
    assert!(
        original[0].abs() > 1e-3,
        "镜像点不能落在 x=0 上，否则 x 取反与否都一样"
    );
    for (axis, what) in [(0, "x"), (1, "y"), (2, "z"), (3, "w")] {
        assert_close(
            mirrored[axis],
            original[axis],
            &format!("镜像点的裁剪 {what}"),
        );
    }
}

/// 着色器侧的契约：水面像素才采样、按当前 uv 采样、未命中回退屏幕空间反射。
#[test]
fn tonemap_shader_samples_the_planar_map_at_the_current_uv() {
    let source = include_str!("../../../shaders/tonemap.wgsl");
    assert!(source.contains("fn planar_reflection"));
    assert!(source.contains("params.planar.x < 0.5"));
    assert!(source.contains("uv + normal.xz * 0.004"));
    assert!(
        !source.contains("1.0 - uv.x"),
        "镜像图已是屏幕朝向，再翻转一次 x 会把反射采到左右相反的位置"
    );
    assert!(source.contains("textureSampleLevel(planar_map, planar_sampler"));
    assert!(source.contains("if weight < 0.01"));
    assert!(source.contains("select(screen, planar, length(planar) > 0.001)"));
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

fn water_drawable() -> Scene3dDrawable {
    Scene3dDrawable {
        entity: rustlua_ecs::spatial_transform::SceneEntityId(2),
        name: "Water".to_owned(),
        mesh_path: "__water__?size=40&res=8&y=0".to_owned(),
        material: None,
        mesh_handle: None,
        model: Mat4::identity(),
        palette: Vec::new(),
        instances: Vec::new(),
        bounds: Scene3dBounds {
            local_aabb: Aabb3 {
                min: Vec3::new(-20.0, -0.1, -20.0),
                max: Vec3::new(20.0, 0.1, 20.0),
            },
            source: BoundsSource::Fallback,
        },
    }
}

/// 沉在水面之下的地平面：世界裁剪面应当把它挡在镜像图外。
fn ground_drawable() -> Scene3dDrawable {
    Scene3dDrawable {
        entity: rustlua_ecs::spatial_transform::SceneEntityId(3),
        name: "Ground".to_owned(),
        mesh_path: "__ground_plane__".to_owned(),
        material: None,
        mesh_handle: None,
        model: Mat4::translation(Vec3::new(0.0, -1.0, 0.0)),
        palette: Vec::new(),
        instances: Vec::new(),
        bounds: Scene3dBounds {
            local_aabb: Aabb3 {
                min: Vec3::new(-10.0, -0.1, -10.0),
                max: Vec3::new(10.0, 0.1, 10.0),
            },
            source: BoundsSource::Fallback,
        },
    }
}

/// 高出水面的基座：它才是镜像图里该出现的东西。
fn plinth_drawable() -> Scene3dDrawable {
    Scene3dDrawable {
        entity: rustlua_ecs::spatial_transform::SceneEntityId(4),
        name: "Plinth".to_owned(),
        mesh_path: "__ground_plane__".to_owned(),
        material: None,
        mesh_handle: None,
        model: Mat4::translation(Vec3::new(0.0, PLINTH_HEIGHT, 0.0)),
        palette: Vec::new(),
        instances: Vec::new(),
        bounds: Scene3dBounds {
            local_aabb: Aabb3 {
                min: Vec3::new(-10.0, -0.1, -10.0),
                max: Vec3::new(10.0, 0.1, 10.0),
            },
            source: BoundsSource::Fallback,
        },
    }
}

struct Fixture {
    renderer: Renderer,
    camera: Camera3D,
    scene_view: RenderView,
    target: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Fixture {
    fn scene(&self, drawables: Vec<Scene3dDrawable>) -> Scene3d {
        Scene3d {
            camera: self.camera,
            view: self.scene_view,
            light: Light::default(),
            point_lights: Vec::new(),
            drawables,
        }
    }

    /// 渲染一帧并回读，返回本帧 HDR 是否真的生效（swiftshader 上没有 HDR 时整组跳过）。
    fn render(&mut self, scene: &Scene3d) -> bool {
        let mesh = empty_mesh();
        let outcome =
            self.renderer
                .render_into_texture(&mesh, Some(scene), &self.view, SIZE, SIZE, true);
        assert!(
            matches!(outcome, RenderOutcome::Presented),
            "平面反射帧必须正常出图"
        );
        // 回读既证明这一帧真的跑完（swiftshader 上不读回时队列在析构阶段等不到完成），
        // 也保证后面的离屏色图回读拿到的是本帧内容。
        self.renderer
            .copy_texture_to_cpu(&self.target, SIZE, SIZE)
            .expect("planar readback");
        self.renderer.last_frame_metrics().hdr_active
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
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rustlua-test-planar-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let camera = Camera3D::perspective(
        Vec3::new(0.0, 3.0, 6.0),
        Vec3::ZERO,
        std::f32::consts::FRAC_PI_2,
        0.1,
        100.0,
    )
    .expect("camera");
    let scene_view = RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR);
    renderer.set_frame_views(FrameViews {
        main: Some(scene_view),
        overlay: None,
    });
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
    Some(Fixture {
        renderer,
        camera,
        scene_view,
        target,
        view,
    })
}

#[test]
fn planar_reflection_only_draws_geometry_above_the_water() {
    let Some(mut fixture) = fixture("rustlua-test-planar-target") else {
        return gpu_skipped("request_adapter");
    };
    let scene = fixture.scene(vec![water_drawable(), ground_drawable()]);
    fixture.renderer.set_render_settings(
        RenderSettings::default()
            .with_pbr(true)
            .with_hdr(ToggleMode::On)
            .with_ssr(true)
            .with_ssao(true),
    );
    if !fixture.render(&scene) {
        return gpu_skipped("hdr_active");
    }
    let metrics = fixture.renderer.last_frame_metrics();
    assert!(
        fixture
            .renderer
            .last_frame_passes()
            .contains(&RenderPass::PlanarReflection),
        "水面 + HDR + PBR + SSR 必须执行 PlanarReflection，实际 {:?}",
        fixture.renderer.last_frame_passes()
    );
    assert!(metrics.planar_reflection_active);
    assert!(
        metrics.planar_reflection_draws > 0,
        "反射 pass 至少要走一遍地平面的绘制"
    );

    // 地平面在水下：水面高度裁剪面必须把它挡掉，镜像图保持全透明。
    // 这条同时是裁剪面的反面控制 —— 去掉裁剪面，同一块几何会被镜像到水面之上。
    let planar = fixture
        .renderer
        .read_planar_reflection_to_cpu()
        .expect("planar map readback");
    assert_eq!(
        (planar.width, planar.height),
        (SIZE / 2, SIZE / 2),
        "平面反射目标应是主目标的半分辨率"
    );
    let submerged_hits = planar.pixels.iter().filter(|pixel| pixel[3] >= 0.5).count();
    assert_eq!(
        submerged_hits, 0,
        "水下几何不该出现在镜像图里（裁剪面失效会把河床镜像到水面之上）"
    );

    // 加上高出水面的基座：镜像图必须有覆盖且不是全黑。
    let scene = fixture.scene(vec![water_drawable(), plinth_drawable(), ground_drawable()]);
    if !fixture.render(&scene) {
        return gpu_skipped("hdr_active");
    }
    let metrics = fixture.renderer.last_frame_metrics();
    assert!(metrics.planar_reflection_active);
    assert!(
        metrics.planar_reflection_draws > 0,
        "反射 pass 至少要把基座画进离屏色图"
    );
    let planar = fixture
        .renderer
        .read_planar_reflection_to_cpu()
        .expect("planar map readback");
    let covered = planar.pixels.iter().filter(|pixel| pixel[3] >= 0.5).count();
    assert!(
        covered > 0,
        "镜像图里一个不透明像素都没有：pass 跑了但几何没落进离屏色图"
    );
    assert!(
        planar
            .pixels
            .iter()
            .any(|pixel| pixel[0] + pixel[1] + pixel[2] > 0.01),
        "镜像图像素全黑，Tonemap 采样拿不到任何辐射度"
    );
}

#[test]
fn planar_reflection_stays_off_without_water_or_ssr() {
    let Some(mut fixture) = fixture("rustlua-test-planar-negative-target") else {
        return gpu_skipped("request_adapter");
    };

    // ① 没有水面：即使 HDR/PBR/SSR 全开也不该有反射 pass。
    let no_water = fixture.scene(vec![ground_drawable()]);
    fixture.renderer.set_render_settings(
        RenderSettings::default()
            .with_pbr(true)
            .with_hdr(ToggleMode::On)
            .with_ssr(true),
    );
    if !fixture.render(&no_water) {
        return gpu_skipped("hdr_active");
    }
    let metrics = fixture.renderer.last_frame_metrics();
    assert!(!metrics.planar_reflection_active);
    assert!(
        !fixture
            .renderer
            .last_frame_passes()
            .contains(&RenderPass::PlanarReflection)
    );

    // ② 关掉 SSR：水面在、HDR 在，但平面反射的开关也跟着关。
    let with_water = fixture.scene(vec![water_drawable(), plinth_drawable(), ground_drawable()]);
    fixture.renderer.set_render_settings(
        RenderSettings::default()
            .with_pbr(true)
            .with_hdr(ToggleMode::On)
            .with_ssr(false),
    );
    if !fixture.render(&with_water) {
        return gpu_skipped("hdr_active");
    }
    let metrics = fixture.renderer.last_frame_metrics();
    assert!(
        !metrics.planar_reflection_active,
        "SSR 关闭时不得执行平面反射"
    );
    // 目标只在 pass 真跑时按需创建：两条反例里仍是构造期的 1×1 透明占位图。
    let placeholder = fixture
        .renderer
        .read_planar_reflection_to_cpu()
        .expect("placeholder readback");
    assert_eq!(
        (placeholder.width, placeholder.height),
        (1, 1),
        "没有执行反射 pass 时不该建半分辨率目标"
    );
    assert!(
        placeholder.pixels.iter().all(|pixel| pixel[3] == 0.0),
        "占位图必须全透明，否则 Tonemap 会把它当命中"
    );
}

/// P3-R191：平面反射按**镜像相机视锥**剔除——远场块（x=2000）不在镜像视野里，
/// 不进反射 draw；水面上的基座一个都不许误剔（`culled` 恰好等于远场块一份）。
/// 反射像素正确性由上面两条 GPU 门守护。
#[test]
fn planar_pass_culls_draws_outside_the_mirror_frustum() {
    let Some(mut fixture) = fixture("rustlua-test-planar-cull-target") else {
        return gpu_skipped("request_adapter");
    };
    let mut far = ground_drawable();
    far.model = Mat4::translation(Vec3::new(2000.0, -1.0, 0.0));
    let scene = fixture.scene(vec![water_drawable(), plinth_drawable(), far]);
    fixture.renderer.set_render_settings(
        RenderSettings::default()
            .with_pbr(true)
            .with_hdr(ToggleMode::On)
            .with_ssr(true)
            .with_ssao(true),
    );
    if !fixture.render(&scene) {
        return gpu_skipped("hdr_active");
    }
    let metrics = fixture.renderer.last_frame_metrics();
    assert!(
        metrics.planar_reflection_active && metrics.planar_reflection_draws > 0,
        "反射 pass 应照常出图：{metrics:?}"
    );
    assert_eq!(
        metrics.planar_draws_culled, 1,
        "只该剔掉镜像视锥外的远场块，基座不许误剔：{metrics:?}"
    );
}

/// P3-R275：排除集合里的实体整条 draw 不进反射 pass——计数走 `skipped` 而不是 `culled`，
/// 镜像图回到全透明。跳过的 draw 必须真的短路，否则计数照涨而像素不变。
#[test]
fn planar_pass_skips_entities_marked_in_the_skip_set() {
    let Some(mut fixture) = fixture("rustlua-test-planar-skip-target") else {
        return gpu_skipped("request_adapter");
    };
    fixture.renderer.set_render_settings(
        RenderSettings::default()
            .with_pbr(true)
            .with_hdr(ToggleMode::On)
            .with_ssr(true)
            .with_ssao(true),
    );

    let scene = fixture.scene(vec![water_drawable(), plinth_drawable()]);
    if !fixture.render(&scene) {
        return gpu_skipped("hdr_active");
    }
    let baseline = fixture.renderer.last_frame_metrics();
    assert!(
        baseline.planar_reflection_draws > 0 && baseline.planar_draws_skipped == 0,
        "基线：基座必须进反射、排除数为 0：{baseline:?}"
    );
    let covered = fixture
        .renderer
        .read_planar_reflection_to_cpu()
        .expect("planar map readback")
        .pixels
        .iter()
        .filter(|pixel| pixel[3] >= 0.5)
        .count();
    assert!(covered > 0, "基线镜像图必须有覆盖，否则这条门没有反面可测");

    let plinth_entity = plinth_drawable().entity.0;
    fixture.renderer.set_planar_skip_entities(&[plinth_entity]);
    let scene = fixture.scene(vec![water_drawable(), plinth_drawable()]);
    if !fixture.render(&scene) {
        return gpu_skipped("hdr_active");
    }
    let metrics = fixture.renderer.last_frame_metrics();
    assert_eq!(
        metrics.planar_draws_skipped, 1,
        "排除集合里的基座必须恰好记一次跳过：{metrics:?}"
    );
    assert_eq!(
        metrics.planar_draws_culled, 0,
        "排除先于视锥剔除判，跳过的 draw 不算被剔除：{metrics:?}"
    );
    assert_eq!(
        metrics.planar_reflection_draws,
        baseline.planar_reflection_draws - 1,
        "被排除的 draw 不许再进反射 pass：{metrics:?}"
    );
    let planars = fixture
        .renderer
        .read_planar_reflection_to_cpu()
        .expect("planar map readback")
        .pixels
        .iter()
        .filter(|pixel| pixel[3] >= 0.5)
        .count();
    assert_eq!(
        planars, 0,
        "唯一的反射源被排除后镜像图必须回到全透明（计数与像素两处同时绿才算跳过生效）"
    );
}

/// P3-R196：平面反射目标分辨率除数生效——4 档把 64² 目标压到 16²，pass 照常执行；
/// 默认（不调 setter）仍是 2 档半分辨率，既有像素门守护。
#[test]
fn planar_divisor_resizes_the_reflection_target() {
    let Some(mut fixture) = fixture("rustlua-test-planar-divisor-target") else {
        return gpu_skipped("request_adapter");
    };
    fixture.renderer.set_planar_reflection_divisor(4);
    let scene = fixture.scene(vec![water_drawable(), plinth_drawable()]);
    fixture.renderer.set_render_settings(
        RenderSettings::default()
            .with_pbr(true)
            .with_hdr(ToggleMode::On)
            .with_ssr(true)
            .with_ssao(true),
    );
    if !fixture.render(&scene) {
        return gpu_skipped("hdr_active");
    }
    assert_eq!(
        fixture.renderer.planar_reflection_size,
        (SIZE / 4, SIZE / 4),
        "除数 4 应把 {SIZE}² 目标压到 {}²",
        SIZE / 4
    );
    assert!(
        fixture
            .renderer
            .last_frame_metrics()
            .planar_reflection_active
    );
}

/// P3-R196 复核：1× 档（全分辨率）确实生效——目标从默认 32² 变为 64²。
/// 活体里 1× 与 2× 的 planar 时间几乎相同（§21.482 补测），这条先把「旗标没生效」
/// 这个最容易的假绿排除掉，剩下的平台期是 GPU 侧问题。
#[test]
fn planar_divisor_one_restores_full_resolution_target() {
    let Some(mut fixture) = fixture("rustlua-test-planar-full-target") else {
        return gpu_skipped("request_adapter");
    };
    fixture.renderer.set_planar_reflection_divisor(1);
    let scene = fixture.scene(vec![water_drawable(), plinth_drawable()]);
    fixture.renderer.set_render_settings(
        RenderSettings::default()
            .with_pbr(true)
            .with_hdr(ToggleMode::On)
            .with_ssr(true)
            .with_ssao(true),
    );
    if !fixture.render(&scene) {
        return gpu_skipped("hdr_active");
    }
    assert_eq!(
        fixture.renderer.planar_reflection_size,
        (SIZE, SIZE),
        "除数 1 应恢复全分辨率目标"
    );
}
