//! P3-R02-4：alpha mask/blend、双面与透明排序的**像素门**（`wp0-pbr-alpha-golden-v1`）。
//!
//! 门钉住的观测：
//!
//! 1. **mask 阈值两侧**：高阈值只保留左半（红），右半（绿，alpha=64）与背景逐字节一致；
//!    低阈值两半都可见，且与 opaque 材质逐像素一致（阈值以上不改变着色）。
//! 2. **blend 真的混合**：alpha=0.5 的红色 quad 像素介于背景与 alpha=1 的红色 quad 之间。
//! 3. **透明排序稳定**：两个重叠半透明 quad 的声明顺序互换后逐像素一致（远→近稳定排序）。
//! 4. **双面**：从背面看单面材质被剔除（背景），双面材质可见；从正面看两者逐像素一致。
//! 5. **mask 阴影**：opaque / mask-visible 投影使地面变暗；mask-clear 与 blend 不投影
//!    （与无遮挡地面逐字节一致）。
//! 6. **旧路径不受影响**：`pbr=off` 时 mask 与 opaque 材质逐字节一致（alpha 轴被归零）。

use std::{env, fs, path::PathBuf};

use engine::host::RuntimeScene;
use rustlua_camera::camera::{Camera3D, Viewport};
use rustlua_gpu::{OffscreenRequest, OffscreenTarget, RenderOutcome, Renderer};
use rustlua_infra::games_dir;
use rustlua_math::math3d::Vec3;
use rustlua_render::picking::screen_from_world;
use rustlua_render::render_settings::RenderSettings;
use rustlua_scene::scene::{Scene, SceneEntityId};
use serde_json::json;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;
const QUAD: &str = "assets/models/p3-r02-quad.gltf";
const UPRIGHT: [f32; 4] = [0.707_106_77, 0.0, 0.0, 0.707_106_77];

struct CameraSpec {
    position: Vec3,
    target: Vec3,
}

#[derive(Clone, Copy)]
struct Quad {
    material: &'static str,
    position: Vec3,
    rotation: [f32; 4],
    scale: f32,
}

fn quad(material: &'static str, position: Vec3, rotation: [f32; 4], scale: f32) -> Quad {
    Quad {
        material,
        position,
        rotation,
        scale,
    }
}

/// 一个已渲染帧：像素 + 相机（世界点 → 屏幕点用同一份烘焙结果）。
struct Frame {
    pixels: Vec<u8>,
    camera: Camera3D,
    viewport: Viewport,
}

impl Frame {
    fn at(&self, world: Vec3) -> [u8; 3] {
        let point = screen_from_world(self.camera, world, self.viewport)
            .unwrap_or_else(|| panic!("相机没有覆盖采样点 {world:?}"))
            .0;
        self.pixel(
            point.x.round().max(0.0) as u32,
            point.y.round().max(0.0) as u32,
        )
    }

    fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        let x = x.min(WIDTH - 1);
        let y = y.min(HEIGHT - 1);
        let index = ((y * WIDTH + x) * 4) as usize;
        [
            self.pixels[index],
            self.pixels[index + 1],
            self.pixels[index + 2],
        ]
    }
}

fn main() {
    let output = parse_output();
    let root = games_dir().join("examples").join("game_api");
    let fixture = OffscreenTarget::acquire(&OffscreenRequest::new(
        "wp0-pbr-alpha-golden",
        WIDTH,
        HEIGHT,
    ))
    .expect("offscreen fixture for wp0-pbr-alpha-golden");
    let device = fixture.device();
    let queue = fixture.queue().clone();
    let mut renderer =
        Renderer::new_offscreen(device.clone(), queue, Some(root.clone()), false, true)
            .expect("offscreen renderer");
    let texture = fixture.texture();
    let view = fixture.view();

    renderer.set_render_settings(RenderSettings::default().with_pbr(true));
    assert!(renderer.pbr_pipelines_ready(), "PBR 变体没有编译");

    // ① mask：纹理左半红（alpha=255）、右半绿（alpha=64）。
    let mask_camera = CameraSpec {
        position: Vec3::new(0.0, 3.5, 6.5),
        target: Vec3::ZERO,
    };
    let mask_light = Vec3::new(0.8, -0.35, 0.2);
    let mask_points = [
        Vec3::new(-1.5, 0.0, -0.5),
        Vec3::new(-1.5, 0.0, 0.5),
        Vec3::new(1.5, 0.0, -0.5),
        Vec3::new(1.5, 0.0, 0.5),
    ];
    let horizontal = [0.0, 0.0, 0.0, 1.0];
    let mask = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        &mask_camera,
        mask_light,
        &[quad(
            "assets/materials/p3-r02-alpha-mask.material",
            Vec3::ZERO,
            horizontal,
            1.0,
        )],
    );
    let mask_low = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        &mask_camera,
        mask_light,
        &[quad(
            "assets/materials/p3-r02-alpha-mask-low.material",
            Vec3::ZERO,
            horizontal,
            1.0,
        )],
    );
    let opaque = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        &mask_camera,
        mask_light,
        &[quad(
            "assets/materials/p3-r02-alpha-opaque.material",
            Vec3::ZERO,
            horizontal,
            1.0,
        )],
    );
    let mask_samples = sample_all(&mask, &mask_points);
    let mask_low_samples = sample_all(&mask_low, &mask_points);
    let opaque_samples = sample_all(&opaque, &mask_points);
    let mask_background = mask.pixel(4, 4);
    let mask_left_delta = max_delta(&mask_samples[..2], &[mask_background; 2]);
    let mask_right_to_background = max_delta(&mask_samples[2..], &[mask_background; 2]);
    let mask_low_right_delta = max_delta(&mask_low_samples[2..], &[mask_background; 2]);
    let mask_low_vs_opaque = max_delta(&mask_low_samples, &opaque_samples);

    // ② blend 与透明排序：两块竖直 quad，远蓝近红，声明顺序互换。
    let blend_camera = CameraSpec {
        position: Vec3::new(0.0, 2.0, 6.0),
        target: Vec3::new(0.0, 2.0, 0.0),
    };
    let blend_light = Vec3::new(0.3, -0.5, -1.0);
    let blend_point = Vec3::new(0.0, 2.0, 0.0);
    let red = quad(
        "assets/materials/p3-r02-alpha-blend-a.material",
        Vec3::new(0.0, 2.0, 1.0),
        UPRIGHT,
        1.0,
    );
    let blue = quad(
        "assets/materials/p3-r02-alpha-blend-b.material",
        Vec3::new(0.0, 2.0, 0.0),
        UPRIGHT,
        1.0,
    );
    let solid = quad(
        "assets/materials/p3-r02-alpha-blend-solid.material",
        Vec3::new(0.0, 2.0, 1.0),
        UPRIGHT,
        1.0,
    );
    let blend_red_only = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        &blend_camera,
        blend_light,
        &[red],
    );
    let blend_blue_only = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        &blend_camera,
        blend_light,
        &[blue],
    );
    let blend_solid_only = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        &blend_camera,
        blend_light,
        &[solid],
    );
    let blend_forward = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        &blend_camera,
        blend_light,
        &[red, blue],
    );
    let blend_reverse = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        &blend_camera,
        blend_light,
        &[blue, red],
    );
    let blend_background = blend_red_only.pixel(4, 4);
    let red_pixel = blend_red_only.at(blend_point);
    let blue_pixel = blend_blue_only.at(blend_point);
    let solid_pixel = blend_solid_only.at(blend_point);
    let forward_pixel = blend_forward.at(blend_point);
    let reverse_pixel = blend_reverse.at(blend_point);
    let mut sorted_samples = Vec::new();
    let mut reversed_samples = Vec::new();
    for x in [-0.8_f32, 0.0, 0.8] {
        for y in [1.4_f32, 2.0, 2.6] {
            let point = Vec3::new(x, y, 0.0);
            sorted_samples.push(blend_forward.at(point));
            reversed_samples.push(blend_reverse.at(point));
        }
    }
    let sort_delta = max_delta(&sorted_samples, &reversed_samples);
    let blend_to_background = channel_delta(red_pixel, blend_background);
    let blend_to_solid = channel_delta(red_pixel, solid_pixel);
    let blend_between = channels_between(red_pixel, blend_background, solid_pixel);
    let both_to_red = channel_delta(forward_pixel, red_pixel);
    let both_to_blue = channel_delta(forward_pixel, blue_pixel);

    // ③ 双面：竖直 quad 正面朝 +Z；从 -Z 看是背面。
    let front_camera = CameraSpec {
        position: Vec3::new(0.0, 2.0, 6.0),
        target: Vec3::new(0.0, 2.0, 0.0),
    };
    let back_camera = CameraSpec {
        position: Vec3::new(0.0, 2.0, -6.0),
        target: Vec3::new(0.0, 2.0, 0.0),
    };
    let front_single = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        &front_camera,
        Vec3::new(0.3, -0.5, -1.0),
        &[quad(
            "assets/materials/p3-r02-alpha-single.material",
            Vec3::new(0.0, 2.0, 0.0),
            UPRIGHT,
            1.0,
        )],
    );
    let front_double = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        &front_camera,
        Vec3::new(0.3, -0.5, -1.0),
        &[quad(
            "assets/materials/p3-r02-alpha-double.material",
            Vec3::new(0.0, 2.0, 0.0),
            UPRIGHT,
            1.0,
        )],
    );
    let back_single = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        &back_camera,
        Vec3::new(0.3, -0.5, 1.0),
        &[quad(
            "assets/materials/p3-r02-alpha-single.material",
            Vec3::new(0.0, 2.0, 0.0),
            UPRIGHT,
            1.0,
        )],
    );
    let back_double = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        &back_camera,
        Vec3::new(0.3, -0.5, 1.0),
        &[quad(
            "assets/materials/p3-r02-alpha-double.material",
            Vec3::new(0.0, 2.0, 0.0),
            UPRIGHT,
            1.0,
        )],
    );
    let front_single_pixel = front_single.at(blend_point);
    let front_double_pixel = front_double.at(blend_point);
    let back_single_pixel = back_single.at(blend_point);
    let back_double_pixel = back_double.at(blend_point);
    let back_background = back_single.pixel(4, 4);
    let front_parity = channel_delta(front_single_pixel, front_double_pixel);
    let front_visible = channel_delta(front_single_pixel, front_single.pixel(4, 4));
    let back_culled = channel_delta(back_single_pixel, back_background);
    let back_visible = channel_delta(back_double_pixel, back_background);

    // ④ 阴影：地面 + 悬空 quad（caster）。光源 (0.2,-1,0.1) 使阴影落点约在 (0.4,0,0.2)。
    let shadow_camera = CameraSpec {
        position: Vec3::new(0.0, 6.0, 8.0),
        target: Vec3::ZERO,
    };
    let shadow_light = Vec3::new(0.2, -1.0, 0.1);
    let shadow_point = Vec3::new(0.4, 0.0, 0.2);
    let ground = quad(
        "assets/materials/p3-r02-shadow-ground.material",
        Vec3::ZERO,
        horizontal,
        2.0,
    );
    let caster = |material: &'static str| quad(material, Vec3::new(0.0, 2.0, 0.0), horizontal, 0.5);
    let ground_only = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        &shadow_camera,
        shadow_light,
        &[ground],
    );
    let shadow_opaque = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        &shadow_camera,
        shadow_light,
        &[
            ground,
            caster("assets/materials/p3-r02-shadow-opaque.material"),
        ],
    );
    let shadow_mask_clear = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        &shadow_camera,
        shadow_light,
        &[
            ground,
            caster("assets/materials/p3-r02-shadow-mask-clear.material"),
        ],
    );
    let shadow_mask_visible = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        &shadow_camera,
        shadow_light,
        &[
            ground,
            caster("assets/materials/p3-r02-shadow-mask-visible.material"),
        ],
    );
    let shadow_blend = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        &shadow_camera,
        shadow_light,
        &[
            ground,
            caster("assets/materials/p3-r02-shadow-blend.material"),
        ],
    );
    let ground_pixel = ground_only.at(shadow_point);
    let shadow_opaque_pixel = shadow_opaque.at(shadow_point);
    let shadow_mask_clear_pixel = shadow_mask_clear.at(shadow_point);
    let shadow_mask_visible_pixel = shadow_mask_visible.at(shadow_point);
    let shadow_blend_pixel = shadow_blend.at(shadow_point);
    let opaque_darkening = channel_delta(ground_pixel, shadow_opaque_pixel);
    let mask_clear_parity = channel_delta(ground_pixel, shadow_mask_clear_pixel);
    let mask_visible_darkening = channel_delta(ground_pixel, shadow_mask_visible_pixel);
    let mask_visible_vs_opaque = channel_delta(shadow_opaque_pixel, shadow_mask_visible_pixel);
    let blend_parity = channel_delta(ground_pixel, shadow_blend_pixel);

    // ⑤ 旧路径负向门：pbr=off 时 alpha 轴被归零，mask 与 opaque 逐字节一致。
    renderer.set_render_settings(RenderSettings::default());
    let legacy_mask = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        &mask_camera,
        mask_light,
        &[quad(
            "assets/materials/p3-r02-alpha-mask.material",
            Vec3::ZERO,
            horizontal,
            1.0,
        )],
    );
    let legacy_opaque = render_scene(
        &mut renderer,
        &root,
        texture,
        view,
        &mask_camera,
        mask_light,
        &[quad(
            "assets/materials/p3-r02-alpha-opaque.material",
            Vec3::ZERO,
            horizontal,
            1.0,
        )],
    );
    let legacy_mask_samples = sample_all(&legacy_mask, &mask_points);
    let legacy_opaque_samples = sample_all(&legacy_opaque, &mask_points);
    let legacy_alpha_delta = max_delta(&legacy_mask_samples, &legacy_opaque_samples);

    assert!(
        mask_left_delta >= 8,
        "mask 可见半侧与背景差 {mask_left_delta}：左半没有被画出来"
    );
    assert_eq!(
        mask_right_to_background, 0,
        "mask cutoff 以上半侧必须被丢弃；实测与背景差 {mask_right_to_background}"
    );
    assert!(
        mask_low_right_delta >= 8,
        "低阈值下右半应可见；实测与背景差 {mask_low_right_delta}"
    );
    assert_eq!(
        mask_low_vs_opaque, 0,
        "阈值以上 mask 与 opaque 必须逐像素一致；实测差 {mask_low_vs_opaque}"
    );
    assert!(
        blend_to_background >= 8,
        "半透明像素与背景差 {blend_to_background}：没有混合"
    );
    assert!(
        blend_to_solid >= 8,
        "半透明像素与不透明像素差 {blend_to_solid}：alpha 没有生效"
    );
    assert!(
        blend_between,
        "混合结果必须落在背景与不透明像素之间：red={red_pixel:?} bg={blend_background:?} solid={solid_pixel:?}"
    );
    assert!(
        sort_delta == 0,
        "透明排序不稳定：声明顺序互换后像素差 {sort_delta}"
    );
    assert!(
        both_to_red >= 8 && both_to_blue >= 8,
        "两层半透明必须都参与混合：vs red={both_to_red} vs blue={both_to_blue}"
    );
    assert_eq!(
        front_parity, 0,
        "正面单面/双面渲染不一致：差 {front_parity}"
    );
    assert!(
        front_visible >= 8,
        "正面单面 quad 不可见：与背景差 {front_visible}"
    );
    assert_eq!(
        back_culled, 0,
        "单面材质从背面必须被剔除；实测与背景差 {back_culled}"
    );
    assert!(
        back_visible >= 8,
        "双面材质从背面应可见；实测与背景差 {back_visible}"
    );
    assert!(
        opaque_darkening >= 8,
        "opaque caster 没有投下阴影：地面只差 {opaque_darkening}"
    );
    assert_eq!(
        mask_clear_parity, 0,
        "mask-clear caster 不应投影；地面差 {mask_clear_parity}"
    );
    assert!(
        mask_visible_darkening >= 8,
        "mask-visible caster 应投影；地面差 {mask_visible_darkening}"
    );
    assert_eq!(
        mask_visible_vs_opaque, 0,
        "mask-visible 与 opaque 的阴影应逐像素一致；差 {mask_visible_vs_opaque}"
    );
    assert_eq!(
        blend_parity, 0,
        "blend 材质不得进入阴影通道；地面差 {blend_parity}"
    );
    assert_eq!(
        legacy_alpha_delta, 0,
        "pbr=off 时 alpha 轴必须归零；mask/opaque 像素差 {legacy_alpha_delta}"
    );

    println!(
        "wp0_pbr_alpha_samples mask_left={mask_left_delta} mask_right_bg={mask_right_to_background} \
         mask_low_right={mask_low_right_delta} mask_low_opaque={mask_low_vs_opaque} \
         blend_bg={blend_to_background} blend_solid={blend_to_solid} sort={sort_delta} \
         front_parity={front_parity} back_culled={back_culled} back_visible={back_visible} \
         shadow_opaque={opaque_darkening} shadow_mask_clear={mask_clear_parity} \
         shadow_mask_visible={mask_visible_darkening} shadow_blend={blend_parity} \
         legacy_alpha={legacy_alpha_delta}"
    );

    let report = json!({
        "report_version": "wp0-pbr-alpha-golden-v1",
        "scene": "p3-r02 quad overrides: mask/blend/double-sided + shadow caster",
        "width": WIDTH,
        "height": HEIGHT,
        "mask_background": mask_background,
        "mask_samples": mask_samples,
        "mask_low_samples": mask_low_samples,
        "opaque_samples": opaque_samples,
        "blend": {
            "background": blend_background,
            "red_only": red_pixel,
            "blue_only": blue_pixel,
            "solid_red": solid_pixel,
            "both_forward": forward_pixel,
            "both_reverse": reverse_pixel,
            "sorted_samples": sorted_samples,
            "reversed_samples": reversed_samples,
        },
        "double_sided": {
            "front_single": front_single_pixel,
            "front_double": front_double_pixel,
            "back_single": back_single_pixel,
            "back_double": back_double_pixel,
            "back_background": back_background,
        },
        "shadow": {
            "sample_point": [shadow_point.x, shadow_point.y, shadow_point.z],
            "ground_only": ground_pixel,
            "opaque_caster": shadow_opaque_pixel,
            "mask_clear_caster": shadow_mask_clear_pixel,
            "mask_visible_caster": shadow_mask_visible_pixel,
            "blend_caster": shadow_blend_pixel,
        },
        "legacy_mask_samples": legacy_mask_samples,
        "legacy_opaque_samples": legacy_opaque_samples,
        "max_channel_delta": {
            "mask_left_vs_background": mask_left_delta,
            "mask_right_vs_background": mask_right_to_background,
            "mask_low_right_vs_background": mask_low_right_delta,
            "mask_low_vs_opaque": mask_low_vs_opaque,
            "blend_vs_background": blend_to_background,
            "blend_vs_solid": blend_to_solid,
            "blend_declaration_order": sort_delta,
            "double_sided_front": front_parity,
            "single_sided_back": back_culled,
            "double_sided_back": back_visible,
            "shadow_opaque_darkening": opaque_darkening,
            "shadow_mask_clear": mask_clear_parity,
            "shadow_mask_visible_darkening": mask_visible_darkening,
            "shadow_mask_visible_vs_opaque": mask_visible_vs_opaque,
            "shadow_blend": blend_parity,
            "legacy_alpha_axis": legacy_alpha_delta,
        },
        "checks": {
            "mask_discards_below_cutoff": mask_right_to_background == 0 && mask_left_delta >= 8,
            "mask_passes_above_cutoff": mask_low_right_delta >= 8 && mask_low_vs_opaque == 0,
            "blend_mixes_with_background": blend_to_background >= 8 && blend_to_solid >= 8 && blend_between,
            "transparent_sort_is_stable": sort_delta == 0,
            "both_blend_layers_contribute": both_to_red >= 8 && both_to_blue >= 8,
            "double_sided_removes_culling": back_culled == 0 && back_visible >= 8 && front_parity == 0,
            "shadow_respects_alpha_mask": opaque_darkening >= 8 && mask_clear_parity == 0
                && mask_visible_darkening >= 8 && mask_visible_vs_opaque == 0,
            "blend_is_excluded_from_shadow": blend_parity == 0,
            "legacy_shader_ignores_alpha": legacy_alpha_delta == 0,
        },
        "draw_calls": renderer.last_frame_metrics().draw_calls,
    });
    let text = serde_json::to_string_pretty(&report).expect("serialize report");
    if let Some(path) = output {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).expect("create output directory");
        }
        fs::write(&path, text.as_bytes()).expect("write report");
        println!("report: {}", path.display());
    }
    println!("wp0_pbr_alpha_golden_ok");
}

fn sample_all(frame: &Frame, points: &[Vec3]) -> Vec<[u8; 3]> {
    points.iter().map(|point| frame.at(*point)).collect()
}

fn channel_delta(left: [u8; 3], right: [u8; 3]) -> u32 {
    (0..3)
        .map(|channel| u32::from(left[channel].abs_diff(right[channel])))
        .max()
        .unwrap_or(0)
}

fn max_delta(left: &[[u8; 3]], right: &[[u8; 3]]) -> u32 {
    assert_eq!(left.len(), right.len());
    left.iter()
        .zip(right)
        .map(|(left, right)| channel_delta(*left, *right))
        .max()
        .unwrap_or(0)
}

fn channels_between(mixed: [u8; 3], low: [u8; 3], high: [u8; 3]) -> bool {
    (0..3).all(|channel| {
        let min = low[channel].min(high[channel]).saturating_sub(1);
        let max = low[channel].max(high[channel]).saturating_add(1);
        (min..=max).contains(&mixed[channel])
    })
}

#[allow(clippy::too_many_arguments)]
fn render_scene(
    renderer: &mut Renderer,
    root: &std::path::Path,
    texture: &wgpu::Texture,
    view: &wgpu::TextureView,
    camera: &CameraSpec,
    light: Vec3,
    quads: &[Quad],
) -> Frame {
    let mut scene = Scene::new("pbr-alpha-probe");
    let light_id = SceneEntityId(1);
    let camera_id = SceneEntityId(2);
    scene.add_entity(light_id, "Sun", None).expect("add entity");
    scene
        .add_entity(camera_id, "Camera", None)
        .expect("add entity");
    scene
        .entities
        .get_mut(&light_id)
        .unwrap()
        .components
        .insert(
            "Light".into(),
            format!(
                "direction={},{},{};color=1,1,1;intensity=1",
                light.x, light.y, light.z
            ),
        );
    scene
        .entities
        .get_mut(&camera_id)
        .unwrap()
        .components
        .insert(
            "Camera".into(),
            format!(
                "projection=perspective;position={},{},{};target={},{},{};fov=60;near=0.1;far=100",
                camera.position.x,
                camera.position.y,
                camera.position.z,
                camera.target.x,
                camera.target.y,
                camera.target.z
            ),
        );
    for (index, quad) in quads.iter().enumerate() {
        let id = SceneEntityId(10 + index as u64);
        let name = format!("Quad{index}");
        scene.add_entity(id, &name, None).expect("add entity");
        scene.entities.get_mut(&id).unwrap().components.insert(
            "Transform".into(),
            format!(
                "position={},{},{};rotation={},{},{},{};scale={},{},{}",
                quad.position.x,
                quad.position.y,
                quad.position.z,
                quad.rotation[0],
                quad.rotation[1],
                quad.rotation[2],
                quad.rotation[3],
                quad.scale,
                quad.scale,
                quad.scale
            ),
        );
        scene.entities.get_mut(&id).unwrap().components.insert(
            "MeshRenderer".into(),
            format!("mesh={QUAD};material={}", quad.material),
        );
    }

    let mut runtime = RuntimeScene::from_scene_edit(Some(root), scene).expect("alpha runtime");
    let viewport = Viewport::new(WIDTH as f32, HEIGHT as f32).expect("viewport");
    let extracted = runtime.scene_3d(viewport).expect("alpha 3D extraction");
    let mesh = runtime
        .vertices(WIDTH, HEIGHT)
        .expect("alpha 2D extraction");
    assert!(matches!(
        renderer.render_into_texture(&mesh, Some(&extracted), view, WIDTH, HEIGHT),
        RenderOutcome::Presented
    ));
    let pixels = renderer
        .copy_texture_to_cpu(texture, WIDTH, HEIGHT)
        .expect("alpha readback");
    Frame {
        pixels,
        camera: extracted.camera,
        viewport,
    }
}

fn parse_output() -> Option<PathBuf> {
    let mut output = None;
    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--output" => {
                output = Some(PathBuf::from(
                    args.next().expect("--output requires a path"),
                ))
            }
            "--help" | "-h" => {
                println!("Usage: wp0_pbr_alpha_golden [--output PATH]");
                std::process::exit(0);
            }
            _ => panic!("unknown argument: {flag}"),
        }
    }
    output
}
