//! M86：自定义特效叠加 pass 的像素级验收。
//!
//! 这些用例证明的是三件事，而不只是"管线建起来了"：
//! 1. 关档（`None`）时像素与基线逐字节相同——默认零行为变化；
//! 2. 开档后画面真的被叠加层改了颜色（不是画了个透明的东西）；
//! 3. `EffectParams.time` 真的接进了片元——同一帧、不同时间输出不同像素。

use super::*;

fn overlay_renderer() -> Option<(wgpu::Device, wgpu::Queue, Renderer)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("rustlua-effect-overlay-test-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .ok()?;
    let renderer = Renderer::new_offscreen(device.clone(), queue.clone(), None, false, false)
        .expect("offscreen renderer");
    Some((device, queue, renderer))
}

fn empty_mesh() -> rustlua_render::frame::SceneMesh {
    rustlua_render::frame::SceneMesh {
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

/// 一套最小的"目标 + 渲染 + 回读"，三个用例共用。
fn render_frame(device: &wgpu::Device, renderer: &mut Renderer, _queue: &wgpu::Queue) -> Vec<u8> {
    let (width, height) = (32u32, 32u32);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rustlua-effect-overlay-target"),
        size: wgpu::Extent3d {
            width,
            height,
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
    let outcome = renderer.render_into_texture(&empty_mesh(), None, &view, width, height, true);
    assert!(matches!(outcome, RenderOutcome::Presented));
    renderer
        .copy_texture_to_cpu(&texture, width, height)
        .expect("readback")
}

/// 恒定的红色半透明叠加：只要 pass 真的画了，画面就会明显偏红。
const RED_OVERLAY: &str = r#"
struct EffectParams {
    time: f32,
    width: f32,
    height: f32,
    _pad0: f32,
    tint: vec4<f32>,
};
@group(0) @binding(0) var<uniform> effect: EffectParams;
struct EffectOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> EffectOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0)
    );
    var output: EffectOut;
    let corner = positions[index];
    output.position = vec4<f32>(corner, 0.0, 1.0);
    output.uv = vec2<f32>((corner.x + 1.0) * 0.5, (1.0 - corner.y) * 0.5);
    return output;
}
@fragment
fn fs_main(input: EffectOut) -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 0.0, 0.0, 0.75);
}
"#;

/// 时间驱动的叠加：`time < 0.5` 画绿、否则画蓝。时间没接进片元时会两帧同色。
const TIME_OVERLAY: &str = r#"
struct EffectParams {
    time: f32,
    width: f32,
    height: f32,
    _pad0: f32,
    tint: vec4<f32>,
};
@group(0) @binding(0) var<uniform> effect: EffectParams;
struct EffectOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> EffectOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0)
    );
    var output: EffectOut;
    let corner = positions[index];
    output.position = vec4<f32>(corner, 0.0, 1.0);
    output.uv = vec2<f32>((corner.x + 1.0) * 0.5, (1.0 - corner.y) * 0.5);
    return output;
}
@fragment
fn fs_main(input: EffectOut) -> @location(0) vec4<f32> {
    if effect.time < 0.5 {
        return vec4<f32>(0.0, 1.0, 0.0, 0.8);
    }
    return vec4<f32>(0.0, 0.0, 1.0, 0.8);
}
"#;

/// 参数表驱动的叠加：`graph_params.p[0].x < 0.5` 画绿、否则画蓝。
const PARAMS_OVERLAY: &str = r#"
struct EffectParams {
    time: f32,
    width: f32,
    height: f32,
    _pad0: f32,
    tint: vec4<f32>,
};
@group(0) @binding(0) var<uniform> effect: EffectParams;
struct GraphParams { p: array<vec4<f32>, 16>, };
@group(0) @binding(3) var<uniform> graph_params: GraphParams;
struct EffectOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> EffectOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0)
    );
    var output: EffectOut;
    let corner = positions[index];
    output.position = vec4<f32>(corner, 0.0, 1.0);
    output.uv = vec2<f32>((corner.x + 1.0) * 0.5, (1.0 - corner.y) * 0.5);
    return output;
}
@fragment
fn fs_main(input: EffectOut) -> @location(0) vec4<f32> {
    if graph_params.p[0].x < 0.5 {
        return vec4<f32>(0.0, 1.0, 0.0, 0.8);
    }
    return vec4<f32>(0.0, 0.0, 1.0, 0.8);
}
"#;

/// 关档 → 开档 → 关档：像素必须回到与基线逐字节一致。
#[test]
fn effect_overlay_is_off_by_default_and_removable() {
    let Some((device, queue, mut renderer)) = overlay_renderer() else {
        return gpu_skipped("request_adapter");
    };
    let baseline = render_frame(&device, &mut renderer, &queue);
    assert!(!renderer.effect_overlay_active(), "默认必须关闭");

    renderer
        .set_effect_overlay(Some(RED_OVERLAY))
        .expect("compile red overlay");
    assert!(renderer.effect_overlay_active());
    let with_overlay = render_frame(&device, &mut renderer, &queue);
    assert_ne!(baseline, with_overlay, "叠加层必须真的改变像素");

    renderer.set_effect_overlay(None).expect("disable overlay");
    assert!(!renderer.effect_overlay_active());
    let restored = render_frame(&device, &mut renderer, &queue);
    assert_eq!(baseline, restored, "关档后像素必须回到基线");
}

/// 叠加层颜色方向正确：红色通道相对基线升高、绿/蓝通道相对下降。
#[test]
fn effect_overlay_blends_the_requested_color() {
    let Some((device, queue, mut renderer)) = overlay_renderer() else {
        return gpu_skipped("request_adapter");
    };
    let baseline = render_frame(&device, &mut renderer, &queue);
    renderer
        .set_effect_overlay(Some(RED_OVERLAY))
        .expect("compile red overlay");
    let painted = render_frame(&device, &mut renderer, &queue);
    let center = |pixels: &[u8]| {
        let pixel = (16 * 32 + 16) * 4;
        [
            pixels[pixel],
            pixels[pixel + 1],
            pixels[pixel + 2],
            pixels[pixel + 3],
        ]
    };
    let before = center(&baseline);
    let after = center(&painted);
    assert!(
        after[0] > before[0],
        "红色通道应当升高：{before:?} → {after:?}"
    );
    assert!(
        after[1] <= before[1],
        "绿色通道不应升高：{before:?} → {after:?}"
    );
    assert!(
        after[2] <= before[2],
        "蓝色通道不应升高：{before:?} → {after:?}"
    );
}

/// `EffectParams` 的参数槽真的写进了 uniform：改参数（不重编管线）就改画面。
#[test]
fn effect_overlay_reads_the_params_uniform_without_recompiling() {
    let Some((device, queue, mut renderer)) = overlay_renderer() else {
        return gpu_skipped("request_adapter");
    };
    renderer
        .set_effect_overlay(Some(PARAMS_OVERLAY))
        .expect("compile params overlay");
    let mut slots = [[0.0_f32; 4]; 16];
    slots[0] = [0.0, 0.0, 0.0, 0.0];
    renderer.set_effect_params(&slots);
    let green = render_frame(&device, &mut renderer, &queue);
    // 只改 uniform，不重编管线。
    slots[0] = [1.0, 0.0, 0.0, 0.0];
    renderer.set_effect_params(&slots);
    let blue = render_frame(&device, &mut renderer, &queue);
    assert_ne!(green, blue, "参数槽没有影响叠加层输出");
}

/// `EffectParams.time` 真的写进了 uniform：不同时间渲染出不同像素。
#[test]
fn effect_overlay_reads_the_time_uniform() {
    let Some((device, queue, mut renderer)) = overlay_renderer() else {
        return gpu_skipped("request_adapter");
    };
    renderer
        .set_effect_overlay(Some(TIME_OVERLAY))
        .expect("compile time overlay");
    renderer.set_effect_time(0.0);
    let early = render_frame(&device, &mut renderer, &queue);
    renderer.set_effect_time(1.0);
    let late = render_frame(&device, &mut renderer, &queue);
    assert_ne!(early, late, "时间 uniform 没有影响叠加层输出");
}

/// 坏 WGSL 必须返回错误，并保留上一份可用管线（画面不黑）。
#[test]
fn invalid_overlay_keeps_the_previous_pipeline() {
    let Some((device, queue, mut renderer)) = overlay_renderer() else {
        return gpu_skipped("request_adapter");
    };
    renderer
        .set_effect_overlay(Some(RED_OVERLAY))
        .expect("compile red overlay");
    let error = renderer
        .set_effect_overlay(Some("fn broken( -> vec4<f32> { return vec4<f32>(1.0); }"))
        .expect_err("坏 shader 必须报错");
    assert!(
        error.to_string().contains("特效叠加着色器不可用"),
        "错误信息应说明是叠加着色器：{error}"
    );
    assert!(
        renderer.effect_overlay_active(),
        "坏 shader 不应清掉旧叠加层"
    );
    let pixels = render_frame(&device, &mut renderer, &queue);
    assert!(
        pixels.iter().any(|&byte| byte > 0),
        "保留旧管线后画面不应变成空白"
    );
}

/// 纯蓝半透明叠加：用来验证"后 push 的层画在上层"。
const BLUE_OVERLAY: &str = r#"
struct EffectParams {
    time: f32,
    width: f32,
    height: f32,
    _pad0: f32,
    tint: vec4<f32>,
};
@group(0) @binding(0) var<uniform> effect: EffectParams;
struct EffectOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> EffectOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0)
    );
    var output: EffectOut;
    let corner = positions[index];
    output.position = vec4<f32>(corner, 0.0, 1.0);
    output.uv = vec2<f32>((corner.x + 1.0) * 0.5, (1.0 - corner.y) * 0.5);
    return output;
}
@fragment
fn fs_main(input: EffectOut) -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 1.0, 0.75);
}
"#;

/// 多层叠加：push 顺序 = 绘制顺序；`set_effect_overlay` 仍是整层替换；清空回基线。
#[test]
fn multiple_overlays_stack_in_push_order() {
    let Some((device, queue, mut renderer)) = overlay_renderer() else {
        return gpu_skipped("request_adapter");
    };
    let baseline = render_frame(&device, &mut renderer, &queue);
    renderer
        .push_effect_overlay(RED_OVERLAY)
        .expect("red overlay");
    assert_eq!(renderer.effect_overlay_count(), 1);
    let red = render_frame(&device, &mut renderer, &queue);
    renderer
        .push_effect_overlay(BLUE_OVERLAY)
        .expect("blue overlay");
    assert_eq!(renderer.effect_overlay_count(), 2, "第二层应追加而不是替换");
    let both = render_frame(&device, &mut renderer, &queue);
    assert_ne!(red, both, "叠上第二层后像素必须变化");

    // 单层 API 语义保持：整层替换。
    renderer
        .set_effect_overlay(Some(BLUE_OVERLAY))
        .expect("replace with one layer");
    assert_eq!(renderer.effect_overlay_count(), 1);
    let single_blue = render_frame(&device, &mut renderer, &queue);
    assert_ne!(both, single_blue, "替换成单层后不应再叠红色的那一层");

    renderer.clear_effect_overlays();
    assert_eq!(renderer.effect_overlay_count(), 0);
    assert_eq!(
        baseline,
        render_frame(&device, &mut renderer, &queue),
        "清空后像素必须回到基线"
    );
}

/// 采样场景色（binding 4/5）：当前占位是 1×1 白贴图，输出应接近纯白。
/// 这条门证明"场景色绑定真的进了管线"；接入离屏画面后换成对比画面内容。
const SCENE_OVERLAY: &str = r#"
struct EffectParams {
    time: f32,
    width: f32,
    height: f32,
    _pad0: f32,
    tint: vec4<f32>,
};
@group(0) @binding(0) var<uniform> effect: EffectParams;
@group(0) @binding(4) var scene_color: texture_2d<f32>;
@group(0) @binding(5) var scene_sampler: sampler;
struct EffectOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> EffectOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0)
    );
    var output: EffectOut;
    let corner = positions[index];
    output.position = vec4<f32>(corner, 0.0, 1.0);
    output.uv = vec2<f32>((corner.x + 1.0) * 0.5, (1.0 - corner.y) * 0.5);
    return output;
}
@fragment
fn fs_main(input: EffectOut) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(scene_color, scene_sampler, input.uv).rgb, 1.0);
}
"#;

/// 场景色绑定 4/5：占位白贴图 → 叠加层输出近似纯白。
#[test]
fn overlay_can_sample_the_scene_color_binding() {
    let Some((device, queue, mut renderer)) = overlay_renderer() else {
        return gpu_skipped("request_adapter");
    };
    let baseline = render_frame(&device, &mut renderer, &queue);
    renderer
        .set_effect_overlay(Some(SCENE_OVERLAY))
        .expect("compile scene overlay");
    let pixels = render_frame(&device, &mut renderer, &queue);
    let center = (16 * 32 + 16) * 4;
    let rgba = [
        pixels[center],
        pixels[center + 1],
        pixels[center + 2],
        pixels[center + 3],
    ];
    let before = [
        baseline[center],
        baseline[center + 1],
        baseline[center + 2],
        baseline[center + 3],
    ];
    // 叠加层把采样到的画面原样输出 ⇒ 必须与无叠加层时的同一像素一致。
    assert_eq!(
        rgba, before,
        "场景色应当等于回拷前的画面：{rgba:?} vs {before:?}"
    );
}

/// 深度绑定 6/7：空场景深度是远平面 1.0，叠加层把深度当灰度输出应接近纯白。
#[test]
fn overlay_can_sample_the_scene_depth_binding() {
    let source = r#"
struct EffectParams {
    time: f32,
    width: f32,
    height: f32,
    _pad0: f32,
    tint: vec4<f32>,
};
@group(0) @binding(0) var<uniform> effect: EffectParams;
@group(0) @binding(6) var scene_depth: texture_depth_2d;
@group(0) @binding(7) var depth_sampler: sampler;
struct EffectOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> EffectOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0)
    );
    var output: EffectOut;
    let corner = positions[index];
    output.position = vec4<f32>(corner, 0.0, 1.0);
    output.uv = vec2<f32>((corner.x + 1.0) * 0.5, (1.0 - corner.y) * 0.5);
    return output;
}
@fragment
fn fs_main(input: EffectOut) -> @location(0) vec4<f32> {
    let depth = textureSample(scene_depth, depth_sampler, input.uv);
    return vec4<f32>(vec3<f32>(depth), 1.0);
}
"#;
    let Some((device, queue, mut renderer)) = overlay_renderer() else {
        return gpu_skipped("request_adapter");
    };
    renderer
        .set_effect_overlay(Some(source))
        .expect("compile depth overlay");
    let pixels = render_frame(&device, &mut renderer, &queue);
    let center = (16 * 32 + 16) * 4;
    // 空场景深度是均匀远平面：整屏同色、alpha=1、三通道相等 ⇒ 绑定真的采到了深度值。
    assert_eq!(pixels[center + 3], 255, "片元 alpha 应为 1");
    assert_eq!(pixels[center], pixels[center + 1]);
    assert_eq!(pixels[center + 1], pixels[center + 2]);
    let depth_byte = pixels[center];
    assert!(
        pixels
            .chunks(4)
            .all(|pixel| pixel[0] == depth_byte && pixel[3] == 255),
        "空场景深度应当均匀，实测中心={depth_byte}"
    );
}

/// 快路径判据：纯程序化叠加层不该触发"离屏 + 回拷"，采样场景色/深度的才触发。
#[test]
fn scene_route_is_only_taken_when_the_overlay_samples_scene_or_depth() {
    let Some((_device, _queue, mut renderer)) = overlay_renderer() else {
        return gpu_skipped("request_adapter");
    };
    renderer
        .set_effect_overlay(Some(RED_OVERLAY))
        .expect("compile red overlay");
    assert!(
        !renderer.effect_overlays_need_scene(),
        "纯颜色叠加层不应触发离屏回拷"
    );
    renderer
        .set_effect_overlay(Some(SCENE_OVERLAY))
        .expect("compile scene overlay");
    assert!(
        renderer.effect_overlays_need_scene(),
        "采样场景色的叠加层必须走离屏回拷"
    );
}

/// M93 快路径的结构性证据：采样场景色的叠加层比纯叠加层多一次全屏回拷 draw。
#[test]
fn scene_sampling_overlay_costs_one_extra_fullscreen_draw() {
    let Some((device, queue, mut renderer)) = overlay_renderer() else {
        return gpu_skipped("request_adapter");
    };
    renderer
        .set_effect_overlay(Some(RED_OVERLAY))
        .expect("compile red overlay");
    let _ = render_frame(&device, &mut renderer, &queue);
    let fast_draws = renderer.last_frame_metrics().draw_calls;
    renderer
        .set_effect_overlay(Some(SCENE_OVERLAY))
        .expect("compile scene overlay");
    let _ = render_frame(&device, &mut renderer, &queue);
    let scene_draws = renderer.last_frame_metrics().draw_calls;
    assert_eq!(
        scene_draws,
        fast_draws + 1,
        "场景色路径应只多一次恒等回拷 draw（快={fast_draws}，场景={scene_draws}）"
    );
}
