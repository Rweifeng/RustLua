//! P3-R211：封口吃到**单面**网格（`set_clip_cap_both_sides`）门。
//!
//! 背景：P3-R209 的实体封口画在网格的**背面**，而单面网格（内建 glb 壳、`__ground_plane__`）
//! 的背面在第一遍就被硬件剔除，根本没有像素可画 ⇒ 只有双面网格（水面、带裙边的地形）
//! 进得了那一档。Myrmex 想剖开的是巢壳那颗 `nest.glb`，它是单面的。
//!
//! 本档在「本帧真挂了会话裁剪平面」时把主网格通道的双面轴强制为真。不新建管线：双面
//! opaque 变体已由 P3-R170 的反射预热建好，这里只是命中同一份缓存。
//!
//! 门四条（一条离线契约 + 三条两帧像素对照，几何用渲染器自己合成的 `__ground_plane__`，
//! 不依赖磁盘资源）：
//! * **契约**：主通道的选键必须真的或上这条档，而阴影通道的选键不许读它（否则背面会进阴影图）。
//! * **默认 = 显式关**：同参数两帧逐字节相同；getter 默认 `false`。
//! * **这条档挂在裁剪面上**：`clip_plane = [0;4]` 时开关必须逐字节不变——没有切面的帧
//!   把背面放回来只是白付背面 overdraw，还会让单面地面从水下凭空可见。
//! * **有切面时单面网格被剖开**：同一裁剪面下，开关相对关闭在「被切掉一侧」多出成片网格
//!   像素，网格范围之外逐字节不变；再开深封口深度，同一条带要按深度铺开（不止贴 0.08 薄皮）。
//!
//! 反面控制（已逐条执行，读数记在 Myrmex `docs/02-balance.md` §21.497）：
//! * 把主通道的双面强制在两处一起改回只读材质（`passes.rs` 的选键 + `construct.rs` 的预热；只改一边
//!   会先撞上「变体未在构造期预编译」的 panic）⇒ 契约 + 「保留侧整片盖回背面」那条像素同时红（0 像素 / 0 行）。
//! * 把 `cutaway_both_sides` 的裁剪面守卫去掉（恒为真）⇒ 「没有裁剪面必须逐字节不变」那条红（改了 6276 字节）。
//! * 把 `construct.rs` 阴影预热选键也换成强制真 ⇒ 只有契约红，**两条像素门全绿**：像素判据取的是同夹具
//!   两帧之差，阴影通道怎么变都不进这个差值。也就是说「背面不许进阴影图」这条隔离目前只有文本证据。

use super::*;
use rustlua_camera::camera::Viewport;
use rustlua_camera::camera::{Camera3D, DEFAULT_3D_CLEAR, RenderView};
use rustlua_math::math3d::Aabb3;
use rustlua_render::frame::{BoundsSource, Scene3dBounds, Scene3dDrawable};
use rustlua_render::light::Light;
use rustlua_render::mesh3d::GROUND_PLANE_SENTINEL;
use rustlua_render::render_settings::{RenderSettings, ToggleMode};

const SIZE: u32 = 64;
/// 相机在 8×8 地面下方 1.5 米、贴着 `z=3.5` 前缘，朝前上方看：
/// 抬得高 ⇒ 命中 `z > 0`（裁剪面保留侧），压低 ⇒ 命中 `z < 0`（被切掉的一侧）。
const EYE: Vec3 = Vec3::new(0.0, -1.5, 3.5);
const CENTER: Vec3 = Vec3::new(0.0, 0.0, -2.0);
/// 与地面成 45° 的裁剪面：法线 `(0, 1, 1)` 归一、过原点 ⇒ 裁剪距离 = `z / √2`。
const TILT: f32 = 1.0;
/// 裁剪线实测在第 27/28 行之间：`z > 0`（未切的一侧）落在行 0..28，`z < 0`（被切的一侧）从第 28 行起。
/// 开档先把**保留侧**的背面放回来（地面本来就 8 米宽，这 28 行是满幅）。
const BACK_ROWS: std::ops::Range<u32> = 0..28;
/// 保留侧中段：封口深度不许越过裁剪平面，所以薄带帧与深档帧在这里必须逐字节相同。
const KEPT_ROWS: std::ops::Range<u32> = 2..26;
/// 被切掉一侧的截面条带：0.08 薄带在这里盖不到（`z ∈ (-0.11, 0)` 只贴着裁剪线），
/// 深档 3.0 才把 `z ∈ (-4.24, 0)` 一路画成实体截面（实测第 28..33 行）。
const SECTION_ROWS: std::ops::Range<u32> = 28..34;
/// 地面远边之外：已经出画，任何一档都不许多画一个字节。
const EMPTY_ROWS: std::ops::Range<u32> = 40..64;
const COLS: std::ops::Range<u32> = 8..56;
/// 探针档位：向切面内侧 3 米仍然画实体截面（整段被切的半张地面都在范围内）。
const DEEP_CAP: f32 = 3.0;

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

/// 单面地面（`ground_plane_cached_mesh` 的材质没开 `double_sided`）：正面朝 +y，
/// 相机在水下 ⇒ 屏上每个地面像素都本该是**背面**，默认整张不可见。
fn ground_drawable() -> Scene3dDrawable {
    Scene3dDrawable {
        entity: rustlua_ecs::spatial_transform::SceneEntityId(2),
        name: "Ground".to_owned(),
        mesh_path: GROUND_PLANE_SENTINEL.to_owned(),
        material: None,
        mesh_handle: None,
        model: Mat4::identity(),
        palette: Vec::new(),
        instances: Vec::new(),
        bounds: Scene3dBounds {
            local_aabb: Aabb3 {
                min: Vec3::new(-4.0, -0.1, -4.0),
                max: Vec3::new(4.0, 0.1, 4.0),
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
            drawables: vec![ground_drawable()],
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
            .expect("cutaway both-sides readback");
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
        label: Some("rustlua-test-cutaway-both-device"),
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

/// 限定区域内的像素差个数（用于「开关把多少背面放回了画面」）。
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

/// 某帧相对「什么都没画」的基帧，在 CUT_ROWS 里有多少行多出了网格像素。
fn mesh_rows(base: &[u8], frame: &[u8]) -> usize {
    (0..SIZE)
        .filter(|row| pixel_diff_patch(base, frame, &(*row..*row + 1), &(0..SIZE)) > 0)
        .count()
}

/// 离线契约：主通道或上这条档、阴影通道不许读它。
///
/// 判的是源文本，代价是「改这条档的语义但保留字面量」不会红——会被下面三条像素门接住的是
/// 选键不同式（运行期直接 panic）和守卫缺失（第三条红），这条只盯「别把背面塞进阴影图」。
#[test]
fn the_dial_forces_the_color_pass_and_leaves_shadows_alone() {
    let color = include_str!("../passes.rs");
    assert!(
        color.contains("draw.material.double_sided || cutaway_both_sides"),
        "主网格通道的双面轴必须或上本帧的裁剪面守卫，否则这条档换了管线也没人画背面"
    );
    let construct = include_str!("../construct.rs");
    assert!(
        construct.contains("let double_sided = draw.material.double_sided || cutaway_both_sides;"),
        "构造期的预热选键必须和主通道同式，否则运行期撞上「变体未在构造期预编译」的 panic"
    );
    assert!(
        construct.contains(
            "let shadow_key = ShaderVariantKey::shadow_with_alpha(\n                    skin,\n                    draw.material.alpha_mode,\n                    draw.material.double_sided,\n                );"
        ),
        "阴影选键必须仍然只看材质自身的双面档：剖面放回来的背面一进阴影图，\
         P3-R54 起的阴影几何就被这条档改掉了"
    );
}

#[test]
fn single_sided_meshes_get_a_cut_only_when_a_plane_is_set() {
    let Some(mut fixture) = fixture("rustlua-test-cutaway-both-target") else {
        return gpu_skipped("request_adapter");
    };

    // ① 默认 == 显式关：两帧逐字节相同，且 getter 默认关。
    let Some(default_frame) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };
    assert!(
        !fixture.renderer.clip_cap_both_sides(),
        "默认必须关闭：单面网格的背面剔除是既有语义"
    );
    fixture.renderer.set_clip_cap_both_sides(false);
    let Some(off_frame) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };
    let changed = byte_diff_rows(&default_frame, &off_frame, &(0..SIZE));
    assert_eq!(
        changed, 0,
        "显式关闭的档与默认帧不一致（{changed} 字节不同）：默认路径被改了"
    );

    // ② 这条档挂在裁剪面上：没有裁剪面时开关不许单独改像素。
    //    这一条同时证明守卫不是空话——没有它，水下的单面地面会因为「背面回来了」凭空可见。
    fixture.with_clip(false);
    fixture.renderer.set_clip_cap_both_sides(false);
    let Some(noclip_off) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };
    fixture.renderer.set_clip_cap_both_sides(true);
    let Some(noclip_on) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };
    let free_changed = byte_diff_rows(&noclip_off, &noclip_on, &(0..SIZE));
    assert_eq!(
        free_changed, 0,
        "没有裁剪面时开这条档改了 {free_changed} 个字节：背面放回来了，可画面里并没有切面"
    );
    fixture.with_clip(true);

    // ③ 有切面时单面网格被剖开：基帧（开关关闭）里这张地面一像素都没有，
    //    开了以后必须在被切掉一侧多出成片像素，而网格范围之外逐字节不变。
    fixture.renderer.set_clip_cap_both_sides(false);
    fixture.renderer.set_clip_cap_depth(0.0);
    let Some(base) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };
    fixture.renderer.set_clip_cap_both_sides(true);
    let Some(thin_skin) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };

    // ④ 与 P3-R209 的深度档相乘：同一张单面地面，深封口档要把条带按深度铺开，
    //    而不是只贴一层 0.08 薄皮。
    fixture.renderer.set_clip_cap_depth(DEEP_CAP);
    let Some(deep) = fixture.render() else {
        return gpu_skipped("hdr_active");
    };

    // ③ 有切面时单面网格被剖开：开关相对关闭，在裁剪面保留一侧盖回成片背面像素，
    //    而出画之处逐字节不动（这条档只是把背面放回来，不是重画整屏）。
    let outside = byte_diff_rows(&base, &thin_skin, &EMPTY_ROWS);
    assert_eq!(
        outside, 0,
        "地面之外（行 {EMPTY_ROWS:?}）被改了 {outside} 个字节：这条档把整屏都重画了"
    );
    let back_pixels = pixel_diff_patch(&base, &thin_skin, &BACK_ROWS, &COLS);
    println!(
        "cutaway both-sides back faces: pixels={back_pixels} rows={}",
        mesh_rows(&base, &thin_skin)
    );
    assert!(
        back_pixels > 600 && mesh_rows(&base, &thin_skin) >= BACK_ROWS.len() - 1,
        "开档只在保留侧盖回 {back_pixels} 个像素 / {} 行：单面网格的背面没被整片放回来，\
         封口分支仍然拿不到像素（切面照样露空壳）",
        mesh_rows(&base, &thin_skin)
    );

    // ④ 深档把剖面往里推：被切一侧多出成片截面，而保留侧与出画处一字节都不许动。
    let kept_changed = byte_diff_rows(&thin_skin, &deep, &KEPT_ROWS);
    assert_eq!(
        kept_changed, 0,
        "裁剪面保留侧（行 {KEPT_ROWS:?}）被深档改了 {kept_changed} 个字节：封口越过了裁剪平面"
    );
    let far_changed = byte_diff_rows(&thin_skin, &deep, &EMPTY_ROWS);
    assert_eq!(
        far_changed, 0,
        "封口范围之外（行 {EMPTY_ROWS:?}）被深档改了 {far_changed} 个字节"
    );
    let thin_section = pixel_diff_patch(&base, &thin_skin, &SECTION_ROWS, &COLS);
    let deep_section = pixel_diff_patch(&base, &deep, &SECTION_ROWS, &COLS);
    println!(
        "cutaway both-sides section band: thin={thin_section} deep={deep_section} \
         rows thin={} rows deep={}",
        mesh_rows(&base, &thin_skin),
        mesh_rows(&base, &deep)
    );
    assert!(
        deep_section > 150 && deep_section > thin_section + 100,
        "被切一侧的截面带深档只盖了 {deep_section} 个像素（薄带 {thin_section}）：\
         封口深度没换算到单面网格上，切深了仍然只有贴着切面的一层皮"
    );
    fixture.renderer.set_clip_cap_both_sides(false);
    fixture.renderer.set_clip_cap_depth(0.0);
}
