//! P3-R209：剖面封口深度（`set_clip_cap_depth`）门。
//!
//! 背景：P3-R54 给双面网格的切口写死了一层 0.08 m 的实体截面色带。切得浅时够用，
//! 切得深（Myrmex 想把巢体挖开一段）就只能贴着切面封口，往内又露出空壳内壁。
//! 本档把带宽交给 `camera.cap.x`（米）：`0` = 旧的 0.08 常量（默认像素逐字节不变），
//! `> 0` = 从切面向内这么深都一路画实体截面。
//!
//! 门四条（一条离线契约 + 三条两帧像素对照；像素对照依赖 P3-R207 的 `clock_secs()` 时间锁）：
//! * **契约**：带宽必须由 uniform 导出、0 档逐字回到旧常量、且封口仍在裁剪分支内；
//!   平面反射 pass 不许读到这个档位。
//! * **默认 = 显式 0**：同参数两帧逐字节相同；非法值（NaN/负）归一为关闭。
//! * **没有裁剪面就什么都不是**：`clip_plane = [0;4]` 时开大档必须逐字节不变
//!   （封口只在裁剪分支内部，档位不许单独改像素）。
//! * **深档把剖面往里推**：有裁剪面时，深档相对默认档在"被切掉一侧"的条带里多出成片像素，
//!   而切面保留侧与封口范围之外逐字节不变。
//!
//! 反面控制（手工执行、记在文档里）：
//! * 把 `mesh3d_pbr.wgsl` 的 `select(0.08, camera.cap.x, …)` 改成常量 `0.08` ⇒ 第三条必须红；
//! * 把 `band` 判据改成 `clip_distance > -band * 0.0`（等价于永不封口）⇒ 第三条必须红；
//! * 把 `construct.rs` 的 `clip_cap_depth` 默认改成 3.0 ⇒ 第一条必须红。

use super::*;
use rustlua_camera::camera::Viewport;
use rustlua_camera::camera::{Camera3D, DEFAULT_3D_CLEAR, RenderView};
use rustlua_math::math3d::Aabb3;
use rustlua_render::frame::{BoundsSource, Scene3dBounds, Scene3dDrawable};
use rustlua_render::light::Light;
use rustlua_render::render_settings::{RenderSettings, ToggleMode};

const SIZE: u32 = 64;
/// 相机在水下 2 米、贴着切线（世界 z=0）前方 2 米，抬头 14° 看过去：
/// 抬高 ⇒ 命中 `z > 0`（保留侧），压低 ⇒ 命中 `z < 0`（被切掉的一侧）。
const EYE: Vec3 = Vec3::new(0.0, -2.0, 2.0);
const CENTER: Vec3 = Vec3::new(0.0, 0.0, -6.0);
/// 与水面成 45° 的裁剪面：法线 `(0, 1, 1)` 归一。
/// 于是水面上的裁剪距离 = `0.7071 · z`，带宽 0.08 只盖住 `z ∈ (-0.11, 0)`（第 10..13 行），
/// 深档 3.0 盖到 `z ∈ (-4.24, 0)`——这段在近处，屏幕上到第 29 行为止。
const TILT: f32 = 1.0;
/// 保留侧（切面之外）的取样行：视角抬得更高 ⇒ 命中点 `z > 0`，永远进不了封口分支。
const KEPT_ROWS: std::ops::Range<u32> = 2..10;
/// 深档独有的条带行：默认档的 0.08 薄带远不到这里（实测差异落在第 14..29 行）。
const CUT_ROWS: std::ops::Range<u32> = 14..30;
/// 比深档还远的行：已经出了封口范围，两帧必须逐字节相同。
const FAR_ROWS: std::ops::Range<u32> = 40..64;
const COLS: std::ops::Range<u32> = 8..56;
/// 探针档位：向切面内侧 3 米仍然画实体截面。
const DEEP_CAP: f32 = 3.0;

/// 着色器与宿主的契约：带宽必须来自 uniform、0 档必须逐字回到旧常量。
#[test]
fn cutaway_cap_scales_the_section_band() {
    let source = include_str!("../../../shaders/mesh3d_pbr.wgsl");
    assert!(
        source.contains("cap: vec4<f32>,"),
        "Camera 结构必须有封口深度这一行，否则宿主没有地方写档位"
    );
    assert!(
        source.contains("let band = select(0.08, camera.cap.x, camera.cap.x > 0.0);"),
        "带宽必须由 `camera.cap.x` 导出，且 0 档逐字回到 P3-R54 的 0.08"
    );
    // 封口必须仍然只在裁剪分支里：没有裁剪面时档位不许单独改像素。
    // 结构判据（按着色器的四空格缩进）：从分支起点到 band 之间不许出现函数层的闭合花括号。
    let clip = source
        .find("if (dot(camera.clip.xyz, camera.clip.xyz) > 1e-8) {")
        .expect("会话裁剪分支");
    let band = source.find("let band = select(0.08").expect("封口带宽");
    assert!(
        band > clip && !source[clip..band].contains("\n    }"),
        "封口带宽必须落在裁剪分支内部（分支起点 {clip}，band 在 {band}）"
    );

    let host = include_str!("../passes.rs");
    assert!(
        !host.contains("clip_cap_depth"),
        "平面反射 pass 的裁剪面是**水位**不是剖面，不许把会话封口深度带进去"
    );
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

/// 双面水面（`water.rs` 里 `double_sided: true`）：相机在水下 ⇒ 屏上每个水面像素都是背面。
fn water_drawable() -> Scene3dDrawable {
    Scene3dDrawable {
        entity: rustlua_ecs::spatial_transform::SceneEntityId(2),
        name: "Water".to_owned(),
        mesh_path: "__water__?size=64&res=8&y=0".to_owned(),
        material: None,
        mesh_handle: None,
        model: Mat4::identity(),
        palette: Vec::new(),
        instances: Vec::new(),
        bounds: Scene3dBounds {
            local_aabb: Aabb3 {
                min: Vec3::new(-32.0, -0.1, -32.0),
                max: Vec3::new(32.0, 0.1, 32.0),
            },
            source: BoundsSource::Fallback,
        },
    }
}

struct Fixture {
    renderer: Renderer,
    viewport: Viewport,
    target: wgpu::Texture,
    view: wgpu::TextureView,
    clipped: bool,
}

impl Fixture {
    fn with_clip(&mut self, clipped: bool) {
        self.clipped = clipped;
    }

    /// 渲染一帧并回读紧凑 RGBA8（时钟已在建夹具时钉住）。
    fn render(&mut self) -> Option<Vec<u8>> {
        let mut camera =
            Camera3D::perspective(EYE, CENTER, std::f32::consts::FRAC_PI_2, 0.1, 200.0)
                .expect("camera");
        let norm = (1.0 + TILT * TILT).sqrt();
        camera.clip_plane = if self.clipped {
            [0.0, 1.0 / norm, TILT / norm, 0.0]
        } else {
            [0.0; 4]
        };
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
            drawables: vec![water_drawable()],
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
            "剖切帧必须正常出图"
        );
        let pixels = self
            .renderer
            .copy_texture_to_cpu(&self.target, SIZE, SIZE)
            .expect("cutaway readback");
        if !self.renderer.last_frame_metrics().hdr_active {
            return None;
        }
        Some(pixels)
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
        label: Some("rustlua-test-cutaway-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let mut renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false)
        .expect("offscreen renderer");
    renderer.set_render_settings(
        RenderSettings::default()
            .with_pbr(true)
            .with_hdr(ToggleMode::On),
    );
    // 后期颗粒与折射都会自己改像素，留着只会给像素差再添两个来源。
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
        clipped: true,
    })
}

/// 两帧之间取值不同的字节个数（限定行）。
fn byte_diff_rows(a: &[u8], b: &[u8], rows: &std::ops::Range<u32>) -> usize {
    let mut count = 0usize;
    for row in rows.clone() {
        let start = (row * SIZE * 4) as usize;
        let end = start + (SIZE * 4) as usize;
        count += a[start..end]
            .iter()
            .zip(&b[start..end])
            .filter(|(x, y)| x != y)
            .count();
    }
    count
}

/// 限定区域内的像素差个数（用于"封口把多少像素换成了实体截面"）。
fn pixel_diff_patch(
    a: &[u8],
    b: &[u8],
    rows: &std::ops::Range<u32>,
    cols: &std::ops::Range<u32>,
) -> usize {
    let mut count = 0usize;
    for row in rows.clone() {
        for column in cols.clone() {
            let index = ((row * SIZE + column) * 4) as usize;
            if a[index..index + 4] != b[index..index + 4] {
                count += 1;
            }
        }
    }
    count
}

#[test]
fn deeper_cut_keeps_the_section_closed_and_stays_off_by_default() {
    let Some(mut fixture) = fixture("rustlua-test-cutaway-target") else {
        return gpu_skipped("request_adapter");
    };

    // ① 默认 == 显式 0：两帧逐字节相同。
    let Some(default_frame) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };
    fixture.renderer.set_clip_cap_depth(0.0);
    let Some(off_frame) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };
    let changed = byte_diff_rows(&default_frame, &off_frame, &(0..SIZE));
    assert_eq!(
        changed, 0,
        "显式关闭的档与默认帧不一致（{changed} 字节不同）：默认路径被改了"
    );
    // 档位口径：默认关闭；非法值归零；正值是米，原样保留（不截断）。
    assert_eq!(
        fixture.renderer.clip_cap_depth(),
        0.0,
        "默认封口深度必须是 0（回到 P3-R54 的 0.08 薄带）"
    );
    fixture.renderer.set_clip_cap_depth(f32::NAN);
    assert_eq!(fixture.renderer.clip_cap_depth(), 0.0);
    fixture.renderer.set_clip_cap_depth(-1.0);
    assert_eq!(fixture.renderer.clip_cap_depth(), 0.0);
    fixture.renderer.set_clip_cap_depth(DEEP_CAP);
    assert_eq!(fixture.renderer.clip_cap_depth(), DEEP_CAP);
    fixture.renderer.set_clip_cap_depth(0.0);

    // ② 没有裁剪面 ⇒ 档位不许单独改像素。
    fixture.with_clip(false);
    fixture.renderer.set_clip_cap_depth(0.0);
    let Some(nocap_noclip) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };
    fixture.renderer.set_clip_cap_depth(DEEP_CAP);
    let Some(deep_noclip) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };
    let free_changed = byte_diff_rows(&nocap_noclip, &deep_noclip, &(0..SIZE));
    assert_eq!(
        free_changed, 0,
        "没有裁剪面时开大档改了 {free_changed} 个字节：封口不该脱离裁剪平面单独生效"
    );
    fixture.with_clip(true);

    // ③ 深档把剖面往里推：被切掉的一侧必须多出成片像素，保留侧一字节都不许动。
    fixture.renderer.set_clip_cap_depth(0.0);
    let Some(thin_frame) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };
    fixture.renderer.set_clip_cap_depth(DEEP_CAP);
    let Some(deep_frame) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };
    let kept_changed = byte_diff_rows(&thin_frame, &deep_frame, &KEPT_ROWS);
    assert_eq!(
        kept_changed, 0,
        "切面保留侧（行 {KEPT_ROWS:?}）被改了 {kept_changed} 个字节：封口越过了裁剪平面"
    );
    let far_changed = byte_diff_rows(&thin_frame, &deep_frame, &FAR_ROWS);
    assert_eq!(
        far_changed, 0,
        "封口范围之外的行（{FAR_ROWS:?}）被改了 {far_changed} 个字节：档位把整侧背面都涂成了截面"
    );
    let band_pixels = pixel_diff_patch(&thin_frame, &deep_frame, &CUT_ROWS, &COLS);
    // 多出的是不是一整条带：只改几行说明档位没换算成带宽、只动了边缘。
    // 实测（swiftshader）深档比默认薄带多盖 16 行、CUT_ROWS 内 272 个像素。
    let band_rows = (0..SIZE)
        .filter(|row| pixel_diff_patch(&thin_frame, &deep_frame, &(*row..*row + 1), &(0..SIZE)) > 0)
        .count();
    println!("cutaway pixels={band_pixels} rows={band_rows} kept={kept_changed} far={far_changed}");
    assert!(
        band_pixels > 150 && band_rows > 8,
        "深档相对默认档只多封了 {band_pixels} 个像素 / {band_rows} 行：\
         封口深度没有真的换算成带宽，切深了照样露空壳"
    );
}
