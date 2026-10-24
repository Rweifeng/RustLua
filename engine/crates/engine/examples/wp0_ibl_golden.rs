//! P3-R03-4：IBL 的**端到端像素门**（报告 `wp0-ibl-golden-v1`）。
//!
//! 与 R03-3 的 `specular_ibl_roughness_selects_the_prefiltered_mip`（合成红/绿/蓝 mip）
//! 分工不同：这里走完整的**场景 → `Environment` 组件 → `.hdr` 映射 `.rlenv` → 渲染器**链路，
//! 用的是仓库里真实提交的 `studio.hdr` / `studio.rlenv` 夹具与自造的金属球。
//!
//! 四组观测：
//!
//! 1. **开关隔离**：同一场景，`Environment` 未声明 vs 已声明但 `ibl=off`，在 LDR 默认设置下
//!    两帧必须逐字节相同——证明"没有环境/关掉 IBL 时保持 R02 输出"。
//! 2. **反射随 roughness 变宽**：三颗金属球（metallic=1）roughness 0.05/0.5/1，同一 studio
//!    环境下统计球面区域相邻像素亮度差（总变差）。镜面球的太阳高光边缘锐利 → TV 高；
//!    粗糙球的预过滤 mip 更糊 → TV 低。原始 TV 在 mid/rough 之间会被 mip4 的块状采样噪声
//!    干扰，所以「变宽」用 6×6 盒式平均后的 coarse TV 判定，并同时断言峰值单调变暗。
//! 3. **Tier C 的 SH-only 分支**：只上传 SH9、不上传 cubemap（`ibl_mode=Diffuse`，与无浮点
//!    设备的回退档同一条着色路径），画面必须与 Off/Full 都不同。
//! 4. **回退可观测**：`EnvironmentApplyReport` 必须带 tier 与回退原因；没有环境时
//!    `apply` 只清理、不改渲染设置。
//!
//! 另外在导入侧重新烘焙 `studio.hdr` 并断言与提交的 `.rlenv` 逐字节一致，报告 `bake_ms`、
//! `env_bytes`（debug 构建下约半秒，可接受）。

use std::{env, fs, path::PathBuf};

use engine::host::{RuntimeScene, apply_scene_environment};
use rustlua_assets_import::environment::{BakeOptions, bake_hdr_bytes};
use rustlua_camera::camera::Viewport;
use rustlua_gpu::{OffscreenRequest, OffscreenTarget, RenderOutcome, Renderer};
use rustlua_infra::games_dir;
use rustlua_math::math3d::Vec3;
use rustlua_render::frame::{Scene3d, SceneMesh};
use rustlua_render::picking::screen_from_world;
use rustlua_render::render_settings::{IblMode, RenderSettings, ToggleMode};
use rustlua_scene::scene::{Scene, SceneEntityId};
use serde_json::json;

const WIDTH: u32 = 480;
const HEIGHT: u32 = 480;
/// 三颗球的 X 位置与材质：左=镜面、中=中等、右=粗糙。
const SPHERES: [(f32, &str, f32); 3] = [
    (-1.9, "assets/materials/p3-r03-metal-smooth.material", 0.05),
    (0.0, "assets/materials/p3-r03-metal-mid.material", 0.5),
    (1.9, "assets/materials/p3-r03-metal-rough.material", 1.0),
];
/// 只统计球心附近的正方形：完全落在球面内，不会把地面/背景算进来。
const SAMPLE_HALF: u32 = 36;

fn main() {
    let output = parse_output();
    let root = games_dir().join("examples").join("game_api");
    let (bake_ms, env_bytes, source_bytes) = verify_committed_environment(&root);

    let fixture = OffscreenTarget::acquire(&OffscreenRequest::new("wp0-ibl-golden", WIDTH, HEIGHT))
        .expect("offscreen fixture for wp0-ibl-golden");
    let device = fixture.device();
    let queue = fixture.queue().clone();
    let mut renderer =
        Renderer::new_offscreen(device.clone(), queue, Some(root.clone()), false, true)
            .expect("offscreen renderer");
    let tier = renderer.ibl_tier();
    assert!(
        tier.supports_specular(),
        "golden 需要 Tier A/B 设备来验证预过滤镜面；当前 {tier:?}"
    );
    let texture = fixture.texture();
    let view = fixture.view();
    let viewport = Viewport::new(WIDTH as f32, HEIGHT as f32).expect("viewport");

    // ---------- 1. 未声明 Environment + LDR 默认设置：R02 基线 ----------
    renderer.set_render_settings(RenderSettings::default());
    let mut plain = RuntimeScene::from_scene_edit(Some(&root), scene(false)).expect("plain scene");
    let plain_mesh = plain.vertices(WIDTH, HEIGHT).expect("plain 2D extraction");
    let plain_3d = plain.scene_3d(viewport).expect("plain 3D extraction");
    let baseline = render(&mut renderer, &plain_mesh, &plain_3d, texture, view);

    // ---------- 2. 声明 Environment 但 ibl=off：必须与基线逐字节相同 ----------
    let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene(true)).expect("IBL scene");
    let report = runtime
        .environment_report()
        .expect("scene declares Environment");
    assert_eq!(report.error, None, "{:?}", report.error);
    let mesh = runtime.vertices(WIDTH, HEIGHT).expect("IBL 2D extraction");
    let extracted = runtime.scene_3d(viewport).expect("IBL 3D extraction");
    upload_environment_only(&mut renderer, &runtime);
    assert_eq!(renderer.ibl_mode(), IblMode::Off);
    let env_off = render(&mut renderer, &mesh, &extracted, texture, view);
    assert_eq!(
        baseline, env_off,
        "ibl=off 且声明了 Environment 时必须保持 R02 像素（LDR 默认设置）"
    );

    // ---------- 3. 应用场景环境：HDR + PBR + IBL，三档 roughness ----------
    let applied = apply_scene_environment(&mut renderer, &runtime, &report);
    assert_eq!(applied.ibl_mode, IblMode::Full, "{applied:?}");
    assert_eq!(
        applied.map_size,
        Some((128, 5)),
        "cubemap 应为 128px/5mip：{applied:?}"
    );
    assert_eq!(renderer.ibl_mode(), IblMode::Full);
    let full = render(&mut renderer, &mesh, &extracted, texture, view);
    let (full_vs_off, full_vs_off_at) = max_channel_delta(&full, &env_off);
    assert!(
        full_vs_off >= 8,
        "Full 与 Off 画面几乎相同（最大差 {full_vs_off}@{full_vs_off_at}）：环境没有进着色"
    );

    let roughness = SPHERES
        .iter()
        .map(|(x, material, roughness)| {
            let center = screen_from_world(extracted.camera, Vec3::new(*x, 1.3, 0.0), viewport)
                .unwrap_or_else(|| panic!("球心 {x} 不在相机视野内"))
                .0;
            let energy = region_texture_energy(
                &full,
                center.x.round() as u32,
                center.y.round() as u32,
                SAMPLE_HALF,
            );
            json!({
                "x": x,
                "material": material,
                "roughness": roughness,
                "screen_center": [center.x, center.y],
                "texture_energy": energy.tv,
                "coarse_texture_energy": energy.coarse_tv,
                "laplacian_energy": energy.laplacian,
                "bright_pixels": energy.bright,
                "peak_luma": energy.peak,
                "spread_pixels": energy.spread,
            })
        })
        .collect::<Vec<_>>();
    let tv: Vec<u64> = roughness
        .iter()
        .map(|entry| entry["texture_energy"].as_u64().expect("tv"))
        .collect();
    let laplacian: Vec<u64> = roughness
        .iter()
        .map(|entry| entry["laplacian_energy"].as_u64().expect("laplacian"))
        .collect();
    let coarse_tv: Vec<u64> = roughness
        .iter()
        .map(|entry| entry["coarse_texture_energy"].as_u64().expect("coarse tv"))
        .collect();
    let peak: Vec<u64> = roughness
        .iter()
        .map(|entry| entry["peak_luma"].as_u64().expect("peak"))
        .collect();
    assert!(
        tv[0] > tv[2],
        "镜面球的反射应比粗糙球锐利（TV {tv:?}）：roughness 没有放宽反射"
    );
    assert!(tv[0] > tv[1], "镜面球的反射应比中等粗糙球锐利（TV {tv:?}）");
    assert!(
        coarse_tv[0] > coarse_tv[1] && coarse_tv[1] > coarse_tv[2],
        "低通后的反射轮廓应随 roughness 单调变宽（coarse TV {coarse_tv:?}，raw TV {tv:?}，Laplacian {laplacian:?}）"
    );
    assert!(
        peak[0] > peak[1] && peak[1] > peak[2],
        "镜面高光的峰值应随 roughness 单调变暗（{peak:?}）"
    );

    // ---------- 4. Tier C 的 SH-only 分支：无 cubemap，只做漫反射 ----------
    let lighting = runtime
        .environment_lighting()
        .expect("environment lighting");
    renderer.clear_environment();
    renderer.set_render_settings(RenderSettings {
        hdr: ToggleMode::On,
        pbr: true,
        ibl: ToggleMode::On,
        variants: false,
    });
    renderer.set_environment(lighting);
    assert_eq!(
        renderer.ibl_mode(),
        IblMode::Diffuse,
        "只有 SH 时 IBL 必须落在 Diffuse（Tier C 的回退档）"
    );
    assert_eq!(renderer.environment_map_info(), None);
    let sh_only = render(&mut renderer, &mesh, &extracted, texture, view);
    let (sh_vs_full, sh_vs_full_at) = max_channel_delta(&sh_only, &full);
    assert!(
        sh_vs_full >= 8,
        "SH-only 与 Full 画面几乎相同（最大差 {sh_vs_full}@{sh_vs_full_at}）：镜面没有关掉"
    );

    // ---------- 5. 再关掉 IBL：回到与基线完全相同的 LDR 像素 ----------
    renderer.set_render_settings(RenderSettings::default());
    let restored = render(&mut renderer, &mesh, &extracted, texture, view);
    assert_eq!(
        baseline, restored,
        "ibl=off 必须逐字节恢复 R02 输出（环境 uniform 不参与 Off 变体）"
    );

    let fallback_reason = applied
        .fallback_reason
        .clone()
        .unwrap_or_else(|| "none".to_owned());
    println!(
        "wp0_ibl_golden tier={} mode={} env_bytes={env_bytes} bake_ms={bake_ms:.1} \
         full_vs_off={full_vs_off} tv={tv:?} coarse_tv={coarse_tv:?} peak={peak:?} \
         sh_vs_full={sh_vs_full}",
        tier.as_str(),
        applied.ibl_mode.as_str(),
    );

    let report_json = json!({
        "report_version": "wp0-ibl-golden-v1",
        "scene": "three metal spheres + ground (built in memory)",
        "width": WIDTH,
        "height": HEIGHT,
        "environment": {
            "declared": "map=assets/env/studio.hdr;intensity=1.0;rotation=0.0",
            "resolved": report.resolved_path,
            "source_bytes": source_bytes,
            "env_bytes": env_bytes,
            "bake_ms": bake_ms,
            "face_size": report.face_size,
            "mip_count": report.mip_count,
            "ibl_tier": tier.as_str(),
            "ibl_mode": applied.ibl_mode.as_str(),
            "map_size": applied.map_size,
            "fallback_reason": fallback_reason,
        },
        "roughness": roughness,
        "deltas": {
            "full_vs_off": full_vs_off,
            "full_vs_off_at": full_vs_off_at,
            "sh_only_vs_full": sh_vs_full,
            "sh_only_vs_full_at": sh_vs_full_at,
        },
        "sh_only": {
            "ibl_mode": IblMode::Diffuse.as_str(),
            "map_size": null,
        },
        "checks": {
            "env_ibl_off_equals_r02": baseline == env_off,
            "ibl_off_restores_r02": baseline == restored,
            "reflection_widens_with_roughness": tv[0] > tv[1]
                && tv[0] > tv[2]
                && coarse_tv[0] > coarse_tv[1]
                && coarse_tv[1] > coarse_tv[2]
                && peak[0] > peak[1]
                && peak[1] > peak[2],
            "full_changes_pixels": full_vs_off >= 8,
            "sh_only_changes_pixels": sh_vs_full >= 8,
            "tier_supports_specular": tier.supports_specular(),
        },
        "draw_calls": renderer.last_frame_metrics().draw_calls,
        "triangles": renderer.last_frame_metrics().triangles,
    });
    let text = serde_json::to_string_pretty(&report_json).expect("serialize report");
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
    println!("wp0_ibl_golden_ok");
}

/// 重新烘焙 `.hdr` 并断言与提交的 `.rlenv` 一致；报告导入期耗时与字节数。
fn verify_committed_environment(root: &std::path::Path) -> (f64, usize, usize) {
    let source_path = root.join("assets/env/studio.hdr");
    let asset_path = root.join("assets/env/studio.rlenv");
    let source = fs::read(&source_path).expect("studio.hdr fixture");
    let committed = fs::read(&asset_path).expect("studio.rlenv fixture");
    let baked = bake_hdr_bytes(&source, BakeOptions::default()).expect("bake studio");
    assert_eq!(
        baked.encode().expect("encode"),
        committed,
        "studio.rlenv 与 studio.hdr 不同步：请重跑 bake_environment --fixture"
    );
    (baked.bake_ms, committed.len(), source.len())
}

/// 只上传环境（不打开 ibl 策略），用于证明"声明了环境但 ibl=off 仍是 R02 像素"。
fn upload_environment_only(renderer: &mut Renderer, runtime: &RuntimeScene) {
    let lighting = runtime.environment_lighting().expect("lighting");
    renderer
        .set_environment_asset(
            runtime.environment_asset().expect("cubemap asset"),
            lighting.settings,
        )
        .expect("upload environment");
}

fn scene(with_environment: bool) -> Scene {
    let mut scene = Scene::new("ibl-golden");
    let sun = SceneEntityId(1);
    let ground = SceneEntityId(2);
    let camera = SceneEntityId(5);
    let environment = SceneEntityId(9);
    for (id, name) in [
        (sun, "Sun"),
        (ground, "Ground"),
        (camera, "Camera"),
        (environment, "Environment"),
    ] {
        scene.add_entity(id, name, None).expect("add entity");
    }
    if with_environment {
        scene
            .entities
            .get_mut(&environment)
            .unwrap()
            .components
            .insert(
                "Environment".into(),
                "map=assets/env/studio.hdr;intensity=1.0;rotation=0.0".into(),
            );
    }
    // 直射光强度 0：让球面读数只反映环境（IBL）与常数环境项。
    scene.entities.get_mut(&sun).unwrap().components.insert(
        "Light".into(),
        "direction=0,-1,0;color=1,1,1;intensity=0".into(),
    );
    scene.entities.get_mut(&ground).unwrap().components.insert(
        "Transform".into(),
        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    scene.entities.get_mut(&ground).unwrap().components.insert(
        "MeshRenderer".into(),
        "mesh=__ground_plane__;material=assets/materials/p3-r03-metal-mid.material".into(),
    );
    for (index, (x, material, _)) in SPHERES.iter().enumerate() {
        let id = SceneEntityId(10 + index as u64);
        scene
            .add_entity(id, format!("Sphere{index}"), None)
            .expect("sphere");
        let entity = scene.entities.get_mut(&id).unwrap();
        entity.components.insert(
            "Transform".into(),
            format!("position={x},1.3,0;rotation=0,0,0,1;scale=1,1,1"),
        );
        entity.components.insert(
            "MeshRenderer".into(),
            format!("mesh=assets/models/p3-sphere.glb;material={material}"),
        );
    }
    scene.entities.get_mut(&camera).unwrap().components.insert(
        "Camera".into(),
        "projection=perspective;position=0,1.6,7.4;target=0,1.0,0;fov=50;near=0.1;far=100".into(),
    );
    scene
}

fn render(
    renderer: &mut Renderer,
    mesh: &SceneMesh,
    scene3d: &Scene3d,
    texture: &wgpu::Texture,
    view: &wgpu::TextureView,
) -> Vec<u8> {
    assert!(matches!(
        renderer.render_into_texture(mesh, Some(scene3d), view, WIDTH, HEIGHT),
        RenderOutcome::Presented
    ));
    renderer
        .copy_texture_to_cpu(texture, WIDTH, HEIGHT)
        .expect("golden readback")
}

fn max_channel_delta(left: &[u8], right: &[u8]) -> (u32, usize) {
    assert_eq!(left.len(), right.len());
    let mut best = (0, 0);
    for (index, (left, right)) in left.chunks_exact(4).zip(right.chunks_exact(4)).enumerate() {
        for channel in 0..3 {
            let delta = u32::from(left[channel].abs_diff(right[channel]));
            if delta > best.0 {
                best = (delta, index);
            }
        }
    }
    best
}

/// 球心附近的锐度读数。预过滤 mip 越糊，相邻差（TV / 粗粒度 TV）与峰值的下降越明显。
///
/// 原始 TV 在 mid 与 rough 之间可能接近打平，Laplacian 甚至会被 mip4 的双线性块状噪声
/// 抬高；所以「反射变宽」的判据用 6×6 盒式平均后的 coarse TV：先压掉逐像素采样噪声，
/// 再比较反射的大尺度明暗轮廓。
struct RegionEnergy {
    tv: u64,
    coarse_tv: u64,
    laplacian: u64,
    bright: u64,
    peak: u32,
    spread: u64,
}

fn region_texture_energy(pixels: &[u8], center_x: u32, center_y: u32, half: u32) -> RegionEnergy {
    let min_x = center_x.saturating_sub(half).min(WIDTH - 1);
    let max_x = center_x.saturating_add(half).min(WIDTH - 1);
    let min_y = center_y.saturating_sub(half).min(HEIGHT - 1);
    let max_y = center_y.saturating_add(half).min(HEIGHT - 1);
    let luma = |x: u32, y: u32| -> u32 {
        let index = ((y * WIDTH + x) * 4) as usize;
        (u32::from(pixels[index]) + u32::from(pixels[index + 1]) + u32::from(pixels[index + 2])) / 3
    };
    let mut tv = 0_u64;
    let mut laplacian = 0_u64;
    let mut bright = 0_u64;
    let mut peak = 0_u32;
    let mut spread = 0_u64;
    let mut values = Vec::new();
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let value = luma(x, y);
            values.push(value);
            peak = peak.max(value);
            if value > 200 {
                bright += 1;
            }
            if x < max_x {
                tv += u64::from(value.abs_diff(luma(x + 1, y)));
            }
            if y < max_y {
                tv += u64::from(value.abs_diff(luma(x, y + 1)));
            }
        }
    }
    for y in (min_y + 1)..max_y {
        for x in (min_x + 1)..max_x {
            let neighbors = luma(x - 1, y) + luma(x + 1, y) + luma(x, y - 1) + luma(x, y + 1);
            laplacian += u64::from((4 * luma(x, y)).abs_diff(neighbors));
        }
    }
    const BLOCK: u32 = 6;
    let mut blocks: Vec<Vec<u64>> = Vec::new();
    let mut y = min_y;
    while y <= max_y {
        let mut row = Vec::new();
        let mut x = min_x;
        while x <= max_x {
            let mut sum = 0_u64;
            let mut count = 0_u64;
            for by in y..=(y + BLOCK - 1).min(max_y) {
                for bx in x..=(x + BLOCK - 1).min(max_x) {
                    sum += u64::from(luma(bx, by));
                    count += 1;
                }
            }
            row.push(sum / count.max(1));
            x += BLOCK;
        }
        blocks.push(row);
        y += BLOCK;
    }
    let mut coarse_tv = 0_u64;
    for (row_index, row) in blocks.iter().enumerate() {
        for (column, value) in row.iter().enumerate() {
            if let Some(right) = row.get(column + 1) {
                coarse_tv += value.abs_diff(*right);
            }
            if let Some(below) = blocks.get(row_index + 1).and_then(|next| next.get(column)) {
                coarse_tv += value.abs_diff(*below);
            }
        }
    }
    let threshold = peak.saturating_sub(24);
    for value in values {
        if value >= threshold {
            spread += 1;
        }
    }
    RegionEnergy {
        tv,
        coarse_tv,
        laplacian,
        bright,
        peak,
        spread,
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
                println!("Usage: wp0_ibl_golden [--output PATH]");
                std::process::exit(0);
            }
            _ => panic!("unknown argument: {flag}"),
        }
    }
    output
}
