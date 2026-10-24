//! P3-R05-2/3 GPU：shader 覆盖目录的**端到端**热重载门。
//!
//! 六条硬门，逐条对应计划 §6.5 的验收口径：
//!
//! 1. 改片元 → 画面真的变（覆盖源生效，不是"读到了但被丢掉"）；
//! 2. 坏源（语法/类型）→ **保留旧管线**并把原因交出来；
//! 3. 绑定接口漂移 → 拒绝重载（布局是 Rust 侧常量，不会跟着重建）；
//! 4. 删掉覆盖文件 → 回退内置源并记录在 `fell_back_to_builtin`；
//! 5. 文件没动 → 一次编译都不做（`reload_shaders_if_changed` 返回 `None`）；
//! 6. 2D 固定槽位（`textured`）同样重载，且**部分失败一条都不换**（R05-3 的"全 shader"）。
//!
//! 场景只用内建几何（`__ground_plane__`），不依赖示例工程资源。

use super::*;
use crate::RenderOutcome;
use crate::renderer::shader_hot_reload::ShaderReloadError;
use crate::shader::{MESH3D, ShaderSource, TEXTURED, VECTOR_TEXT};
use crate::shader_reload::ShaderValidateError;
use engine::host::RuntimeScene;
use rustlua_camera::camera::{FrameViews, Viewport};
use rustlua_scene::scene::{Scene, SceneEntityId};

const SIZE: u32 = 96;

/// 把内置源里 `@fragment fn fs_main` 的函数体换成一条 `return`：绑定与顶点阶段**逐字不动**，
/// 因此接口门必然通过，而画面颜色完全由这条 `return` 决定。
///
/// 前提是片元函数是这个文件的最后一个函数（8 个内置 shader 都满足，靠下面的断言钉住）。
fn fragment_replaced(shader: &ShaderSource, color: &str) -> String {
    let marker = "@fragment fn fs_main";
    let at = shader
        .source
        .find(marker)
        .unwrap_or_else(|| panic!("内置 {} 有片元入口", shader.name));
    assert_eq!(
        shader.source[at + marker.len()..].find(marker),
        None,
        "{} 只应有一个片元入口",
        shader.name
    );
    let line_end = at + shader.source[at..].find('\n').expect("入口签名独占一行");
    let head = &shader.source[..=line_end];
    assert!(
        head.trim_end().ends_with('{'),
        "入口签名行应以 `{{` 结束，实际是 {head:?}"
    );
    format!("{head}\n    return {color};\n}}\n")
}

/// 只动一个绑定号（其余逐字不变）：接口门必须认出它。
fn binding_moved() -> String {
    MESH3D.source.replace(
        "@group(1) @binding(4) var albedo_sampler: sampler;",
        "@group(1) @binding(5) var albedo_sampler: sampler;",
    )
}

/// 语法正确但类型校验不过的片元（`return 1.0` 与 `vec4<f32>` 返回值不匹配）。
fn type_error() -> String {
    fragment_replaced(&MESH3D, "1.0")
}

/// 在片元函数体最前面插一条类型错误（不截断文件）：坏源门用它，因为它只要求"必须被拒绝"。
fn bad_return_in(shader: &ShaderSource) -> String {
    let marker = ") -> @location(0) vec4<f32> {";
    assert!(
        shader.source.contains(marker),
        "内置 {} 应有带 vec4 返回值的入口",
        shader.name
    );
    shader
        .source
        .replacen(marker, &format!("{marker}\n    return 1.0;"), 1)
}

struct TempDir(std::path::PathBuf);

impl TempDir {
    /// 每次调用一个唯一目录（测试并行跑，共用目录会互相看见对方的覆盖文件）。
    fn new(tag: &str) -> Self {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "rustlua-shader-reload-{tag}-{}-{seq}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create override dir");
        Self(dir)
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.0.join(name), text).expect("write override file");
    }

    fn remove(&self, name: &str) {
        std::fs::remove_file(self.0.join(name)).expect("remove override file");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 与 `tests::ibl` 同一台设备/同一个离屏渲染器；`None` 表示这台机器没有适配器。
fn offscreen_renderer(label: &str) -> Option<(wgpu::Device, Renderer)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some(label),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .ok()?;
    let renderer = Renderer::new_offscreen(device.clone(), queue, None, false, false).ok()?;
    Some((device, renderer))
}

/// 一块内建地平面 + 透视相机：中心像素完全由 3D 网格通道决定。
fn ground_center(
    renderer: &mut Renderer,
    target: &wgpu::Texture,
    view: &wgpu::TextureView,
) -> [u8; 3] {
    let mut scene = Scene::new("shader-reload");
    for (id, name) in [
        (SceneEntityId(1), "Sun"),
        (SceneEntityId(2), "Ground"),
        (SceneEntityId(3), "Camera"),
    ] {
        scene.add_entity(id, name, None).expect("add entity");
    }
    scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert(
            "Light".into(),
            "direction=0,-1,0;color=1,1,1;intensity=1".into(),
        );
    scene
        .entities
        .get_mut(&SceneEntityId(2))
        .unwrap()
        .components
        .insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
    scene
        .entities
        .get_mut(&SceneEntityId(2))
        .unwrap()
        .components
        .insert("MeshRenderer".into(), "mesh=__ground_plane__".into());
    scene
        .entities
        .get_mut(&SceneEntityId(3))
        .unwrap()
        .components
        .insert(
            "Camera".into(),
            "projection=perspective;position=0,3,6;target=0,0,0;fov=60;near=0.1;far=100".into(),
        );

    let mut runtime = RuntimeScene::from_scene_edit(None, scene).expect("reload runtime");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let extracted = runtime.scene_3d(viewport).expect("3D extraction");
    let mesh = runtime.vertices(SIZE, SIZE).expect("2D extraction");
    assert!(
        matches!(
            renderer.render_into_texture(&mesh, Some(&extracted), view, SIZE, SIZE),
            RenderOutcome::Presented
        ),
        "覆盖目录下的 3D 帧必须正常提交"
    );
    let pixels = renderer
        .copy_texture_to_cpu(target, SIZE, SIZE)
        .expect("readback");
    let index = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
    [pixels[index], pixels[index + 1], pixels[index + 2]]
}

fn target(device: &wgpu::Device, label: &'static str) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
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
    })
}

/// 门 1/2/3/5：有效覆盖改变画面；坏源与接口漂移保留旧管线；未变的目录不重编。
#[test]
fn override_changes_pixels_and_bad_sources_keep_the_old_pipeline() {
    let Some((device, mut renderer)) = offscreen_renderer("rustlua-shader-reload-device") else {
        return gpu_skipped("request_adapter");
    };
    let dir = TempDir::new("pixels");
    let target = target(&device, "rustlua-shader-reload-target");
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let builtin_center = ground_center(&mut renderer, &target, &view);
    assert!(
        renderer
            .shader_sources()
            .iter()
            .all(|source| !source.overridden),
        "构造期不应有任何覆盖源生效"
    );

    // ① 有效覆盖：片元恒红。先设目录（此时还没有覆盖文件），再写文件、再轮询。
    renderer
        .set_shader_override_dir(Some(dir.0.clone()))
        .expect("空目录=内置源，不该报错");
    dir.write(
        "mesh3d.wgsl",
        &fragment_replaced(&MESH3D, "vec4<f32>(1.0, 0.0, 0.0, 1.0)"),
    );
    let report = renderer
        .reload_shaders_if_changed()
        .expect("有效覆盖必须重载成功")
        .expect("文件变了，必须有重载报告");
    assert!(report.rebuilt > 0, "至少一条 mesh3d 变体重建：{report:?}");
    assert_eq!(
        report.rebuilt + report.unchanged,
        report.variants,
        "重建数 + 未变数必须等于变体总数：{}",
        report.summary()
    );
    let red = ground_center(&mut renderer, &target, &view);
    assert!(
        red[0] > 200 && red[1] < 60 && red[2] < 60,
        "覆盖后中心像素应是纯红，实际 {red:?}（内置档 {builtin_center:?}）"
    );

    // ② 没动文件：一次编译都不做。
    assert!(
        renderer
            .reload_shaders_if_changed()
            .expect("未变目录不该报错")
            .is_none(),
        "内容逐字节相同就不该重编"
    );

    // ③ 类型校验失败：保留旧管线（画面仍是红色），并把原因交出来。
    dir.write("mesh3d.wgsl", &type_error());
    let error = renderer
        .set_shader_override_dir(Some(dir.0.clone()))
        .expect_err("类型错误必须被拒绝");
    assert!(
        matches!(error, ShaderReloadError::Validate(_)),
        "坏源应报 WGSL 校验/解析错误，实际 {error:?}"
    );
    let after_bad = ground_center(&mut renderer, &target, &view);
    assert_eq!(
        red, after_bad,
        "坏源之后画面必须与坏源之前逐字节一致（旧管线保留）"
    );

    // ④ 绑定接口漂移：同样拒绝，画面不变。
    dir.write("mesh3d.wgsl", &binding_moved());
    let error = renderer
        .set_shader_override_dir(Some(dir.0.clone()))
        .expect_err("接口漂移必须被拒绝");
    assert!(
        matches!(
            error,
            ShaderReloadError::Validate(ShaderValidateError::InterfaceChanged { .. })
        ),
        "应是接口变化，实际 {error:?}"
    );
    assert_eq!(
        red,
        ground_center(&mut renderer, &target, &view),
        "拒绝接口漂移时不能换掉任何管线"
    );

    // ⑤ 换一份合法的绿色源：证明"拒绝"没有把渲染器卡死在旧目录上。
    dir.write(
        "mesh3d.wgsl",
        &fragment_replaced(&MESH3D, "vec4<f32>(0.0, 1.0, 0.0, 1.0)"),
    );
    let report = renderer
        .set_shader_override_dir(Some(dir.0.clone()))
        .expect("绿色源与红色源接口相同，应当允许重载");
    assert!(report.rebuilt > 0);
    let green = ground_center(&mut renderer, &target, &view);
    assert!(
        green[1] > 200 && green[0] < 60 && green[2] < 60,
        "绿色覆盖应生效，实际 {green:?}"
    );
}

/// 门 4：删掉覆盖文件 → 回退内置源，并在报告里点名（不能悄悄换）。
#[test]
fn deleted_override_falls_back_to_builtin_and_says_so() {
    let Some((device, mut renderer)) = offscreen_renderer("rustlua-shader-fallback-device") else {
        return gpu_skipped("request_adapter");
    };
    let dir = TempDir::new("fallback");
    let target = target(&device, "rustlua-shader-fallback-target");
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let builtin_center = ground_center(&mut renderer, &target, &view);

    dir.write(
        "mesh3d.wgsl",
        &fragment_replaced(&MESH3D, "vec4<f32>(1.0, 0.0, 0.0, 1.0)"),
    );
    renderer
        .set_shader_override_dir(Some(dir.0.clone()))
        .expect("有效覆盖必须重载成功");
    assert!(
        ground_center(&mut renderer, &target, &view)[0] > 200,
        "覆盖必须先证明已生效，否则后面的回退断言没有意义"
    );

    dir.remove("mesh3d.wgsl");
    let report = renderer
        .reload_shaders_if_changed()
        .expect("删除覆盖不是错误")
        .expect("删除改变了生效源，必须有报告");
    assert!(
        report
            .fell_back_to_builtin
            .iter()
            .any(|name| name == "mesh3d"),
        "回退必须点名：{:?}",
        report.fell_back_to_builtin
    );
    assert!(
        report
            .sources
            .iter()
            .any(|source| source.name == "mesh3d" && !source.overridden),
        "生效源清单应标为内置：{}",
        report.sources_text()
    );
    assert_eq!(
        builtin_center,
        ground_center(&mut renderer, &target, &view),
        "回退后的画面必须与内置档一致"
    );
}

/// 空目录（只设了目录、还没有任何覆盖文件）等价于内置行为：不报错、不重编。
#[test]
fn empty_override_dir_is_builtin_behavior() {
    let Some((_device, mut renderer)) = offscreen_renderer("rustlua-shader-empty-device") else {
        return gpu_skipped("request_adapter");
    };
    let dir = TempDir::new("empty");
    let report = renderer
        .set_shader_override_dir(Some(dir.0.clone()))
        .expect("目录不存在/为空都不该报错");
    assert_eq!(report.rebuilt, 0, "没有覆盖文件就不该重编");
    assert_eq!(report.override_dir.as_deref(), Some(dir.0.as_path()));
    assert!(
        renderer
            .shader_sources()
            .iter()
            .all(|source| !source.overridden)
    );
}

/// 一整块覆盖视口的四边形（内建白贴图、`sdf = 0` ⇒ 走 `pipeline` 而不是 SDF 那条）。
fn ui_mesh() -> rustlua_render::frame::SceneMesh {
    let corner = |x: f32, y: f32, u: f32, v: f32| OverlayVertex {
        position: [x, y],
        uv: [u, v],
        color: [1.0; 4],
        sdf: 0,
    };
    let vertices = vec![
        corner(-0.8, -0.8, 0.0, 0.0),
        corner(0.8, -0.8, 1.0, 0.0),
        corner(0.8, 0.8, 1.0, 1.0),
        corner(-0.8, -0.8, 0.0, 0.0),
        corner(0.8, 0.8, 1.0, 1.0),
        corner(-0.8, 0.8, 0.0, 1.0),
    ];
    rustlua_render::frame::SceneMesh {
        vertices,
        draws: vec![rustlua_render::frame::SceneDraw::screen_ui(0, 0, 6)],
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

fn set_ui_views(renderer: &mut Renderer) {
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let camera =
        Camera2D::new(Vec2::new(SIZE as f32 * 0.5, SIZE as f32 * 0.5), 1.0).expect("camera2d");
    renderer.set_frame_views(FrameViews::layered(
        RenderView::main_2d(camera, viewport, [0.0, 0.0, 0.0, 1.0]),
        RenderView::overlay_2d(camera, viewport),
    ));
}

/// 只画 2D 叠层时的中心像素（3D 通道传 `None`，中心完全由 `textured` 槽位决定）。
fn ui_center(renderer: &mut Renderer, target: &wgpu::Texture, view: &wgpu::TextureView) -> [u8; 3] {
    let mesh = ui_mesh();
    assert!(
        matches!(
            renderer.render_into_texture(&mesh, None, view, SIZE, SIZE),
            RenderOutcome::Presented
        ),
        "2D 覆盖帧必须正常提交"
    );
    let pixels = renderer
        .copy_texture_to_cpu(target, SIZE, SIZE)
        .expect("readback");
    let index = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
    [pixels[index], pixels[index + 1], pixels[index + 2]]
}

/// 门 6（R05-3 的"全 shader"）：2D 固定槽位与 3D 变体在同一次重载里原子提交。
///
/// 两条断言各有分工：**改 `textured.wgsl` 画面真的变**（否则"全 shader"只是文档措辞），
/// 以及**一族坏、另一族好时一条管线都不换**（半替换会让"画面为什么一半新一半旧"变成
/// 无法复现的问题，而这是热重载最容易发生、也最难查的失败形态）。
#[test]
fn two_d_slot_overrides_reload_and_a_partial_failure_swaps_nothing() {
    let Some((device, mut renderer)) = offscreen_renderer("rustlua-shader-2d-device") else {
        return gpu_skipped("request_adapter");
    };
    let dir = TempDir::new("two-d");
    let target = target(&device, "rustlua-shader-2d-target");
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    set_ui_views(&mut renderer);
    // 先打开覆盖目录（此时还没有覆盖文件 = 内置行为），再写文件、再轮询。
    renderer
        .set_shader_override_dir(Some(dir.0.clone()))
        .expect("空目录=内置源，不该报错");
    let builtin = ui_center(&mut renderer, &target, &view);
    assert!(
        builtin[0] > 200 && builtin[1] > 200 && builtin[2] > 200,
        "内置叠层应画白块（黑底 × 白贴图 × 白顶点），实际 {builtin:?}"
    );

    // ① 有效 2D 覆盖：片元恒红。
    dir.write(
        "textured.wgsl",
        &fragment_replaced(&TEXTURED, "vec4<f32>(1.0, 0.0, 0.0, 1.0)"),
    );
    let report = renderer
        .reload_shaders_if_changed()
        .expect("有效 2D 覆盖必须重载成功")
        .expect("文件变了，必须有重载报告");
    assert!(
        report
            .sources
            .iter()
            .any(|source| source.name == "textured" && source.overridden),
        "生效源清单要点名 textured：{}",
        report.sources_text()
    );
    let red = ui_center(&mut renderer, &target, &view);
    assert!(
        red[0] > 200 && red[1] < 60 && red[2] < 60,
        "覆盖后中心应是纯红，实际 {red:?}（内置档 {builtin:?}）"
    );

    // ② 跨槽位部分失败：好的 textured（改成蓝）+ 坏的 vector_text（返回值类型不匹配）。
    dir.write(
        "textured.wgsl",
        &fragment_replaced(&TEXTURED, "vec4<f32>(0.0, 0.0, 1.0, 1.0)"),
    );
    dir.write("vector_text.wgsl", &bad_return_in(&VECTOR_TEXT));
    let error = renderer
        .set_shader_override_dir(Some(dir.0.clone()))
        .expect_err("同批里的坏源必须整批拒绝");
    assert!(
        matches!(error, ShaderReloadError::Validate(_)),
        "坏源应报 WGSL 校验/解析错误，实际 {error:?}"
    );
    assert_eq!(
        red,
        ui_center(&mut renderer, &target, &view),
        "拒绝后 textured 必须还是红色：一条槽位都不许先换"
    );

    // ③ 失败没有卡死渲染器：删掉坏文件后，那份还没提交的蓝色源应正常重载。
    dir.remove("vector_text.wgsl");
    let report = renderer
        .reload_shaders_if_changed()
        .expect("坏文件已删除，应能重载")
        .expect("蓝色 textured 在②里没有提交，这次必须重编");
    assert!(report.rebuilt > 0, "{:?}", report.sources_text());
    let blue = ui_center(&mut renderer, &target, &view);
    assert!(
        blue[2] > 200 && blue[0] < 60 && blue[1] < 60,
        "蓝色覆盖应生效，实际 {blue:?}"
    );
}
