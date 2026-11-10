//! P3-R213：逐格网格哨兵（`__cells__`）的渲染侧门（绘制计数 + 像素）。
//!
//! 三件事：
//! * **哨兵真的进 GPU**：`__cells__` 路径由 `cellmesh::CellMeshSpec` 在 CPU 侧合成实心格的面，
//!   渲染器按路径缓存。`triangles` 差值把「共用面剔除」这条账搬到渲染侧：3×3×3 满格 54 面 =
//!   108 个三角形（本帧读数含后期那张全屏 quad，实测 109），挖掉中心那格 *反而* 多 12 个
//!   （六块内壁），挖掉角格数量不变但轮廓改了。
//! * **空格串是有意跳过、不是静默丢 draw**：`solid_count()==0` 与规格解析失败都必须
//!   `draw_calls=0 且 dropped_draws=0`——本仓最不能接受的失败形状是「屏幕上少一块，
//!   日志里一个字都没有」，只画 `continue` 的分支必须有计数盯着。
//! * **`ds` 位真的落到 GPU 材质上**：封闭几何从外面看永远是正面，唯一能分辨单/双面的是
//!   **把相机埋进实体里**——`ds=0` 时外壳六周全是背面，`ds=1` 把它们画回来 ⇒ 两帧必须差一片；
//!   而正常机位下两档必须逐字节相同，否则「双面」就成了无代价的全量重画。
//!
//! 反面控制（跑完把读数记进 Myrmex `docs/02-balance.md` §21.499）：
//! * 把 `build_mesh` 的面剔除条件放宽（永不跳过共用面）⇒ 三角差值那条红（12 变 -108…实测为准）。
//! * 把 `double_sided: self.double_sided` 改成恒 `false` ⇒ 「ds=1 画回内壁」那条红。
//! * 把哨兵派发里 `solid_count()==0` 的跳过分支去掉 ⇒ 空格串带着一张野网格进 `draw_calls`，那条红。
//!
//! 与 P3-R211 的关系（诚实边界）：那条「封口吃到单面网格」的档**不在这些门里**。逐格几何是封闭体，
//! 剖切露出的那堵墙本来就朝相机 ⇒ 开挖视图不需要它；本文件只验 `ds` 位本身。
//!
//! `scene_content`（`passes.rs`）只看本帧真有没有 draw，所以空/坏串的帧 `hdr_active=false` 是
//! **预期读数**而不是跳过信号：那些帧只比计数，像素比较只用在有几何的帧上。

use super::*;
use rustlua_camera::camera::Viewport;
use rustlua_camera::camera::{Camera3D, DEFAULT_3D_CLEAR, RenderView};
use rustlua_math::math3d::Aabb3;
use rustlua_render::cellmesh::CellMeshSpec;
use rustlua_render::frame::{BoundsSource, Scene3dBounds, Scene3dDrawable};
use rustlua_render::light::Light;
use rustlua_render::render_settings::{RenderSettings, ToggleMode};

const SIZE: u32 = 64;
/// 斜上方看 3×3×3：挖掉靠近相机那一侧的角格才会改轮廓。
const BOX_EYE: Vec3 = Vec3::new(3.4, 2.8, 3.4);
/// 埋在实体中心的相机（eye 不能等于 target，那是退化 look-at）。
const BURIED_EYE: Vec3 = Vec3::new(0.0, 0.0, 0.0);
const BURIED_TARGET: Vec3 = Vec3::new(0.0, 0.0, -1.0);
const CENTER: Vec3 = Vec3::new(0.0, 0.0, 0.0);
/// 中心格与靠相机那侧的角格下标（`index = x + w*(z + d*y)`，w=d=3）。
const CENTRE_CELL: usize = 13;
const CORNER_CELL: usize = 26;

/// 3×3×3 立方体（居中于原点，每格 1 米）；`dig` 给要挖空的 0 基下标。
fn box_spec(dig: Option<usize>, double_sided: bool) -> CellMeshSpec {
    let mut spec = CellMeshSpec::new(1.0, [-1.5, -1.5, -1.5], [3, 3, 3]);
    spec.double_sided = double_sided;
    for index in 0..spec.total_cells() {
        spec.set_cell(index, true).expect("box cell in range");
    }
    if let Some(index) = dig {
        spec.set_cell(index, false).expect("dig in range");
    }
    spec
}

fn empty_spec() -> CellMeshSpec {
    CellMeshSpec::new(1.0, [-1.5, -1.5, -1.5], [3, 3, 3])
}

fn cells_drawable(path: String) -> Scene3dDrawable {
    let (min, max) = empty_spec().bounds();
    Scene3dDrawable {
        entity: rustlua_ecs::spatial_transform::SceneEntityId(2),
        name: "Cells".to_owned(),
        mesh_path: path,
        material: None,
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

/// 解析不了的串也要走同一条跳过分支（base64url 字母表外的字符）。
fn bogus_path() -> String {
    empty_spec().mesh_path().replace("&bits=", "&bits=%%%")
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

struct Frame {
    pixels: Vec<u8>,
    draw_calls: u64,
    triangles: u64,
    dropped: u64,
    hdr: bool,
}

impl Frame {
    fn counts(&self) -> [u64; 3] {
        [self.draw_calls, self.triangles, self.dropped]
    }
}

struct Fixture {
    renderer: Renderer,
    viewport: Viewport,
    target: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Fixture {
    /// 按 mesh 路径渲染一帧，回读紧凑 RGBA8 与本帧计数（时钟已在建夹具时钉住）。
    fn render(&mut self, path: &str, eye: Vec3, center: Vec3) -> Frame {
        let camera = Camera3D::perspective(eye, center, std::f32::consts::FRAC_PI_2, 0.1, 200.0)
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
            drawables: vec![cells_drawable(path.to_owned())],
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
            "逐格网格帧必须出图"
        );
        let metrics = self.renderer.last_frame_metrics();
        let pixels = self
            .renderer
            .copy_texture_to_cpu(&self.target, SIZE, SIZE)
            .expect("cells readback");
        Frame {
            pixels,
            draw_calls: metrics.draw_calls,
            triangles: metrics.triangles,
            dropped: metrics.dropped_draws,
            hdr: metrics.hdr_active,
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
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rustlua-test-cells-device"),
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
    })
}

/// 两帧之间取值不同的字节个数。
fn byte_diff(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).filter(|(x, y)| x != y).count()
}

/// 有多少行出现了差异（成片像素 vs 单点噪声）。
fn rows_diff(a: &[u8], b: &[u8]) -> usize {
    (0..SIZE)
        .filter(|row| {
            let start = (*row * SIZE * 4) as usize;
            let end = start + (SIZE * 4) as usize;
            a[start..end]
                .iter()
                .zip(&b[start..end])
                .any(|(x, y)| x != y)
        })
        .count()
}

/// 共用面剔除进得了 GPU 的三角形数；空/坏串都是有意跳过，不是静默丢 draw。
#[test]
fn cells_face_culling_shows_up_in_triangle_counts_and_empty_specs_are_skipped() {
    let Some(mut fixture) = fixture("rustlua-test-cells-counts") else {
        return gpu_skipped("request_adapter");
    };
    let full = box_spec(None, false);
    let dug_centre = box_spec(Some(CENTRE_CELL), false);
    let dug_corner = box_spec(Some(CORNER_CELL), false);
    assert_eq!(full.face_count(), 54, "3x3x3 满格只出外表面");
    assert_eq!(dug_centre.face_count(), 60, "挖中心那格多出六块内壁");
    assert_eq!(dug_corner.face_count(), 54, "挖角格少三块外壁、多三块内壁");

    let full_frame = fixture.render(&full.mesh_path(), BOX_EYE, CENTER);
    let centre_frame = fixture.render(&dug_centre.mesh_path(), BOX_EYE, CENTER);
    let empty_frame = fixture.render(&empty_spec().mesh_path(), BOX_EYE, CENTER);
    let bogus_frame = fixture.render(&bogus_path(), BOX_EYE, CENTER);
    println!(
        "cells_counts full={:?} dug_centre={:?} empty={:?} bogus={:?}",
        full_frame.counts(),
        centre_frame.counts(),
        empty_frame.counts(),
        bogus_frame.counts()
    );
    if !full_frame.hdr || !centre_frame.hdr {
        return gpu_skipped("hdr_active");
    }

    assert!(full_frame.draw_calls >= 1, "满格立方体必须产生 draw");
    assert_eq!(full_frame.dropped, 0, "draw 不许被静默丢弃");
    assert!(
        full_frame.triangles >= 108,
        "54 面 = 108 三角形必须全进这一帧，实测 {}",
        full_frame.triangles
    );
    assert_eq!(
        centre_frame.triangles as i64 - full_frame.triangles as i64,
        12,
        "挖掉中心那格只该多出六块内壁（12 个三角形）：共用面没被剔除的话这里会少一大截"
    );
    for (label, frame) in [("空格串", &empty_frame), ("坏串", &bogus_frame)] {
        assert_eq!(
            frame.counts(),
            [0, 0, 0],
            "{label} 必须走「跳过并留痕」：draw/三角形/丢弃计数都归零，实测 {:?}",
            frame.counts()
        );
    }

    // 面数不变但轮廓改了：挖角格在像素侧必须可见，否则「剔除共用面」只是一句注释。
    let corner_frame = fixture.render(&dug_corner.mesh_path(), BOX_EYE, CENTER);
    if !corner_frame.hdr {
        return gpu_skipped("hdr_active");
    }
    let corner_diff = byte_diff(&full_frame.pixels, &corner_frame.pixels);
    let corner_rows = rows_diff(&full_frame.pixels, &corner_frame.pixels);
    println!("cells_dig_corner bytes={corner_diff} rows={corner_rows}");
    // 64×64 的目标上一格角缺口只有 ~30 个像素（实测 114 字节 / 7 行），判据取它的一半：
    // 这条只管「轮廓真的改了」，剔除这条账的强度在上面那条 12 三角形差值里。
    assert!(
        corner_diff > 60 && corner_rows >= 5,
        "挖掉靠相机这侧的角格必须改轮廓，实测 {corner_diff} 字节 / {corner_rows} 行"
    );
}

/// `ds` 位真的进到材质：埋在实体里时双面画回外壳内面，正常机位两档逐字节相同。
#[test]
fn cells_double_sided_bit_reaches_the_material_only_where_back_faces_are_seen() {
    let Some(mut fixture) = fixture("rustlua-test-cells-buried") else {
        return gpu_skipped("request_adapter");
    };
    assert!(
        !fixture.renderer.clip_cap_both_sides(),
        "本门不碰 P3-R211 的档，默认必须关"
    );
    let single = box_spec(None, false);
    let double = box_spec(None, true);
    assert!(!single.double_sided, "默认必须是单面");

    let buried_single = fixture.render(&single.mesh_path(), BURIED_EYE, BURIED_TARGET);
    let buried_double = fixture.render(&double.mesh_path(), BURIED_EYE, BURIED_TARGET);
    let outside_single = fixture.render(&single.mesh_path(), BOX_EYE, CENTER);
    let outside_double = fixture.render(&double.mesh_path(), BOX_EYE, CENTER);
    println!(
        "cells_buried single={:?} double={:?} hdr={}{}",
        buried_single.counts(),
        buried_double.counts(),
        buried_single.hdr,
        buried_double.hdr
    );
    if !buried_single.hdr || !buried_double.hdr || !outside_single.hdr || !outside_double.hdr {
        return gpu_skipped("hdr_active");
    }

    let buried_diff = byte_diff(&buried_single.pixels, &buried_double.pixels);
    let buried_rows = rows_diff(&buried_single.pixels, &buried_double.pixels);
    let outside_diff = byte_diff(&outside_single.pixels, &outside_double.pixels);
    println!(
        "cells_ds buried_bytes={buried_diff} buried_rows={buried_rows} \
         outside_bytes={outside_diff}"
    );

    assert_eq!(
        buried_single.counts(),
        buried_double.counts(),
        "ds 只改剔除状态，不改提交量：draw/三角形计数必须同值"
    );
    assert!(
        buried_diff > 400 && buried_rows > 8,
        "相机埋在实体里时 ds=1 必须把外壳内面画回来，实测 {buried_diff} 字节 / {buried_rows} 行"
    );
    assert_eq!(
        outside_diff, 0,
        "正常机位下 ds 两档必须逐字节相同：这一档只影响看得见的背面（实测 {outside_diff} 字节）"
    );
}
