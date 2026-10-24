//! docs/54 §8-⑨：**编辑器路径**的可重复质量证据（`wp0-editor-preview-golden-v1`）。
//!
//! 为什么需要它：纯 2D / 纯 3D / 混合三种运行时有像素 golden，但"编辑器里改了东西之后预览画成
//! 什么样"一直只有人工目测。编辑器路径与运行时路径的差别恰好都在它自己的那几层里：
//! 文档（`SceneDocument`）→ 撤销/重做（`CommandHistory`）→ 空间语义（`scene_space`）→
//! 预览运行时（`RuntimeScene::from_scene_edit`）→ 一帧像素。这里把这条链在**无 GPU 窗口**的
//! 条件下跑通并逐条断言，因此它可以进 CI。
//!
//! 断言四件事：
//! 1. **世界保持的编辑**：拖动子实体（写回局部值）后世界位置精确等于目标，父不动；
//! 2. **撤销/重做**：`undo` 回到逐字节相同的文档，`redo` 再次得到同一份编辑结果；
//! 3. **预览真的画出来了**：离屏渲染的非背景像素数超过阈值（不是"能构建但黑屏"）；
//! 4. **可重复**：同一次编辑的两次预览渲染**逐字节相同**（跨机器可比的判据，不依赖整图 hash
//!    常量），并报告 30 次重建+提取的 p50/p95 耗时（编辑器预览的性能证据）。
//!
//! 场景在内存里构造（不依赖任何工程文件），但纹理解析走真实的 `examples/game_api` 工程根，
//! 因此预览路径的根目录处理与编辑器一致。

use std::{env, fs, path::PathBuf, time::Instant};

use editor::{CommandHistory, CommandKind, SceneDocument, document_space};
use engine::host::{OverlayVertex, RuntimeScene};
use rustlua_ecs::spatial_transform::LocalTransform;
use rustlua_ecs::transform::Vec2;
use rustlua_gpu::{RenderOutcome, Renderer};
use rustlua_infra::games_dir;
use rustlua_scene::scene::{Scene, SceneEntityId};
use serde_json::json;

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;
const PREVIEW_CYCLES: usize = 30;

fn main() {
    let output = parse_output();
    let root = games_dir().join("examples").join("game_api");
    let (mut document, rig, child) = editable_document();
    let original_text = document.scene.to_text();
    let original_world =
        document_space::world_transform_2d(&document.scene, child).expect("child world transform");
    let rig_world_before =
        document_space::world_transform_2d(&document.scene, rig).expect("rig world transform");

    // ---------- 1. 世界保持的编辑：把子实体在**世界**空间里移动 (1.5, 0) ----------
    let target = LocalTransform::from_2d(rustlua_ecs::transform::Transform {
        position: original_world.position + Vec2::new(1.5, 0.0),
        rotation: original_world.rotation,
        scale: original_world.scale,
    })
    .expect("target local transform");
    let mut targets = std::collections::BTreeMap::new();
    targets.insert(child, target);
    let mut history = CommandHistory::new();
    let executed = history
        .execute(
            &mut document,
            "move child in world space",
            CommandKind::Transform(child),
            |document| -> Result<(), String> {
                let locals = document_space::locals_for_world_targets(&document.scene, &targets);
                let local = locals
                    .get(&child)
                    .ok_or_else(|| "child local unsolved".to_owned())?;
                let entity = document
                    .scene
                    .entities
                    .get_mut(&child)
                    .ok_or_else(|| "child entity missing".to_owned())?;
                entity
                    .components
                    .insert("Transform".into(), document_space::encode_transform(*local));
                Ok(())
            },
        )
        .expect("execute edit");
    assert!(executed, "编辑必须被记录进历史");
    let moved_world =
        document_space::world_transform_2d(&document.scene, child).expect("moved world transform");
    let edit_error =
        (moved_world.position - Vec2::new(target.translation.x, target.translation.y)).length();
    assert!(
        edit_error < 1e-4,
        "世界保持的编辑没到位：目标 {:?}，实际 {:?}",
        target.translation,
        moved_world.position
    );
    let rig_world_after =
        document_space::world_transform_2d(&document.scene, rig).expect("rig world transform");
    assert_eq!(
        rig_world_before.position, rig_world_after.position,
        "父实体不该被这次编辑影响"
    );
    // 局部值确实变了（否则上面那条"世界没变"就是空转）。
    let edited_text = document.scene.to_text();
    assert_ne!(edited_text, original_text, "编辑必须真的改了文档");

    // ---------- 2. 撤销 / 重做 ----------
    history.undo(&mut document).expect("undo");
    let undo_matches_original = document.scene.to_text() == original_text;
    assert!(undo_matches_original, "撤销必须回到逐字节相同的文档");
    history.redo(&mut document).expect("redo");
    let redo_matches_edit = document.scene.to_text() == edited_text;
    assert!(redo_matches_edit, "重做必须回到同一份编辑结果");

    // ---------- 3. 预览一帧：离屏渲染 + 像素证据 ----------
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .expect("a fallback GPU adapter is required for the editor preview golden");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("wp0-editor-preview-golden"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request golden device");
    let mut renderer =
        Renderer::new_offscreen(device.clone(), queue, Some(root.clone()), false, true)
            .expect("offscreen renderer");
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("wp0-editor-preview-golden-target"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
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

    let render_preview = |renderer: &mut Renderer, scene: &Scene| -> (Vec<u8>, usize, usize) {
        let mut runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), scene.clone())
            .expect("preview runtime");
        let mesh = runtime.vertices(WIDTH, HEIGHT).expect("preview extraction");
        let vertices = mesh.vertices.len();
        let draws = mesh.draws.len();
        assert!(matches!(
            renderer.render_into_texture(&mesh, None, &view, WIDTH, HEIGHT),
            RenderOutcome::Presented
        ));
        (
            renderer
                .copy_texture_to_cpu(&texture, WIDTH, HEIGHT)
                .expect("preview readback"),
            vertices,
            draws,
        )
    };

    let (pixels_a, vertices, draws) = render_preview(&mut renderer, &document.scene);
    let background = [pixels_a[0], pixels_a[1], pixels_a[2]];
    let painted = count_non_background(&pixels_a, WIDTH, HEIGHT, background);
    assert!(
        painted > 200,
        "编辑器预览几乎什么都没画（{painted} 个非背景像素）：这条链断了"
    );
    assert!(vertices > 0 && draws > 0, "预览必须产出顶点与 draw range");
    let (pixels_b, _, _) = render_preview(&mut renderer, &document.scene);
    assert_eq!(
        pixels_a, pixels_b,
        "同一份文档的两次预览必须逐字节相同（可重复性是这份证据的全部意义）"
    );

    // ---------- 4. 重复性 + 耗时（编辑器预览的性能证据） ----------
    let mut samples_us = Vec::with_capacity(PREVIEW_CYCLES);
    let mut digests = Vec::with_capacity(PREVIEW_CYCLES);
    for _ in 0..PREVIEW_CYCLES {
        let started = Instant::now();
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), document.scene.clone())
                .expect("preview runtime");
        let mesh = runtime.vertices(WIDTH, HEIGHT).expect("preview extraction");
        samples_us.push(started.elapsed().as_micros() as u64);
        digests.push(digest(&mesh.vertices));
    }
    assert!(
        digests.iter().all(|value| *value == digests[0]),
        "重复构建预览必须产出同一份几何（否则编辑器会闪）"
    );
    samples_us.sort_unstable();
    let percentile = |percent: f64| -> u64 {
        let index = ((percent / 100.0) * (samples_us.len() - 1) as f64).round() as usize;
        samples_us[index.min(samples_us.len() - 1)]
    };
    let p50_us = percentile(50.0);
    let p95_us = percentile(95.0);
    assert!(p50_us > 0 && p95_us >= p50_us, "耗时采样异常");

    let report = json!({
        "report_version": "wp0-editor-preview-golden-v1",
        "scene": "editor-preview-golden (built in memory)",
        "entities": document.scene.entities.len(),
        "edit": {
            "kind": "transform2d_world_preserving",
            "world_before": [original_world.position.x, original_world.position.y],
            "world_after": [moved_world.position.x, moved_world.position.y],
            "preserved": edit_error < 1e-4,
            "parent_unmoved": rig_world_before.position == rig_world_after.position,
        },
        "history": {
            "executed": executed,
            "undo_restores_original": undo_matches_original,
            "redo_reapplies": redo_matches_edit,
            "undo_len": history.undo_len(),
            "redo_len": history.redo_len(),
        },
        "preview": {
            "vertices": vertices,
            "draws": draws,
            "non_background_pixels": painted,
            "deterministic": pixels_a == pixels_b,
            "digest": format!("{:016x}", digests[0]),
        },
        "timing": {
            "cycles": PREVIEW_CYCLES,
            "p50_us": p50_us,
            "p95_us": p95_us,
            "max_us": samples_us[samples_us.len() - 1],
        },
    });
    let text = serde_json::to_string_pretty(&report).expect("serialize report");
    if let Some(path) = output {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(&path, text.as_bytes()).expect("write report");
        println!("report: {}", path.display());
    }
    println!(
        "wp0_editor_preview_golden_ok painted={painted} vertices={vertices} draws={draws} p50_us={p50_us} p95_us={p95_us}"
    );
}

/// 一份带父子关系的可编辑文档：父（Rig）有非单位缩放，子（Part）挂在它下面。
///
/// 刻意用非单位父缩放：世界保持的编辑只有在"把父变换正确反解掉"时才成立，父为单位阵时
/// 这条断言会退化成"什么都没检验"。用 2D 内容（`Transform` + `Sprite` + 正交 `Camera`），
/// 因为这是编辑器场景视图最常见的预览形态（世界内容也真的会被画出来）。
fn editable_document() -> (SceneDocument, SceneEntityId, SceneEntityId) {
    let mut scene = Scene::new("editor-preview-golden");
    // docs/54 §4.1：这份夹具断言的是"父子组合 + 世界保持的编辑"，因此必须显式声明局部空间语义
    // （未声明 ⇒ 组织关系，父子不参与组合，keep-world 编辑也就没有意义）。
    scene.metadata.insert("spatial".into(), "local".into());
    let rig = SceneEntityId(1);
    let part = SceneEntityId(2);
    scene.add_entity(rig, "Rig", None).unwrap();
    scene.add_entity(part, "Part", Some(rig)).unwrap();
    scene.entities.get_mut(&rig).unwrap().components.insert(
        "Transform".into(),
        "position=10,0,0;rotation=0,0,0,1;scale=2,2,1".into(),
    );
    // 子实体的局部值 = 反解出来的结果（世界 12,0 ⇒ 局部 1,0）。
    scene.entities.get_mut(&part).unwrap().components.insert(
        "Transform".into(),
        "position=1,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    scene.entities.get_mut(&part).unwrap().components.insert(
        "Sprite".into(),
        "texture=assets/textures/red-1x1.png;size=2x2;pivot=0.500,0.500;\
         tint=1.000,1.000,1.000,1.000;sorting_layer=default;order=0"
            .into(),
    );
    // 正交相机（统一 `Camera`）：让预览把世界内容画到画面中央（与编辑器场景视图同一套初始相机）。
    let camera = SceneEntityId(3);
    scene
        .add_entity(camera, "OrthographicCamera", None)
        .unwrap();
    scene.entities.get_mut(&camera).unwrap().components.insert(
        "Camera".into(),
        "projection=orthographic;center=12.0000,0.0000;zoom=64.000000".into(),
    );
    let mut document = SceneDocument::new_scene("editor-preview-golden");
    document.scene = scene;
    (document, rig, part)
}

fn count_non_background(pixels: &[u8], width: u32, height: u32, background: [u8; 3]) -> usize {
    let mut count = 0usize;
    for y in 0..height {
        for x in 0..width {
            let index = ((y * width + x) * 4) as usize;
            let difference = (i16::from(pixels[index]) - i16::from(background[0])).unsigned_abs()
                + (i16::from(pixels[index + 1]) - i16::from(background[1])).unsigned_abs()
                + (i16::from(pixels[index + 2]) - i16::from(background[2])).unsigned_abs();
            if difference > 24 {
                count += 1;
            }
        }
    }
    count
}

/// 顶点缓冲的 FNV-1a 摘要：只用于"两次构建是否逐字节相同"，不是密码学哈希。
///
/// 按字段取 `f32` 的位模式逐字节喂进哈希（不引入 `bytemuck` 依赖）——浮点相等不能用来比较
/// "逐字节相同"，但位模式可以，而且跨进程稳定。
fn digest(vertices: &[OverlayVertex]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut mix = |bytes: [u8; 4]| {
        for byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for vertex in vertices {
        for value in vertex.position.iter().chain(vertex.uv.iter()) {
            mix(value.to_bits().to_le_bytes());
        }
        for value in vertex.color.iter() {
            mix(value.to_bits().to_le_bytes());
        }
        mix(vertex.sdf.to_le_bytes());
    }
    hash
}

fn parse_output() -> Option<PathBuf> {
    let mut output = None;
    let mut values = env::args().skip(1);
    while let Some(value) = values.next() {
        if value == "--output" {
            output = values.next().map(PathBuf::from);
        }
    }
    output
}
