//! P3-R05 热重载验收样例（`wp0-shader-hot-reload-v1`）：覆盖目录 → 画面 → 拒绝 → 回退。
//!
//! 计划 §6.5 要求这条样例报告 `before/after` 像素、`reload_ms`、`rejected_reason`，并在
//! **任一负向门失败时以非 0 退出**。五步各自对应一条硬要求：
//!
//! 1. 写红色 `mesh3d.wgsl` → 中心像素变红（覆盖源真的进了管线，不是只读到）；
//! 2. 同一目录再查一次 → 没有任何重编（"同一内容不重复编译"）；
//! 3. 写类型错误的源 → 拒绝，且像素与第 1 步**逐字节相同**（旧管线保留）；
//! 4. 写绑定漂移的源 → 拒绝并给出接口 diff（布局不会跟着重建）；
//! 5. 删掉覆盖文件 → 回退内置源，`fell_back_to_builtin` 点名，像素回到第 0 步的读数。
//!
//! 运行：`cargo run -p engine --example wp0_shader_hot_reload -- --output artifacts/p3/shader_reload.json`
//! 没跑就是没跑：这台机器没有可用适配器时以 **exit 3 / BLOCKED** 结束，绝不报成功。

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use engine::host::RuntimeScene;
use rustlua_camera::camera::Viewport;
use rustlua_gpu::shader::MESH3D;
use rustlua_gpu::{OffscreenRequest, OffscreenTarget, RenderOutcome, Renderer};
use rustlua_scene::scene::{Scene, SceneEntityId};
use serde_json::json;

const SIZE: u32 = 128;

/// 覆盖目录：进程内唯一，Drop 时删除。
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let dir = env::temp_dir().join(format!(
            "rustlua-wp0-shader-reload-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create override dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// 把内置 `mesh3d` 的片元函数体换成一条 `return`：绑定与顶点阶段逐字不动，接口门必然通过。
fn fragment_replaced(color: &str) -> String {
    let marker = "@fragment fn fs_main";
    let at = MESH3D.source.find(marker).expect("内置 mesh3d 有片元入口");
    let line_end = at + MESH3D.source[at..].find('\n').expect("入口签名独占一行");
    let head = &MESH3D.source[..=line_end];
    assert!(head.trim_end().ends_with('{'), "入口签名行应以 `{{` 结束");
    format!("{head}\n    return {color};\n}}\n")
}

/// 只改一个绑定号：接口门必须认出它。
fn binding_moved() -> String {
    MESH3D.source.replace(
        "@group(1) @binding(4) var albedo_sampler: sampler;",
        "@group(1) @binding(5) var albedo_sampler: sampler;",
    )
}

/// 一块内建地平面 + 定向光 + 透视相机：中心像素完全由 3D 网格通道决定，不依赖示例工程资源。
fn ground_scene() -> Scene {
    let mut scene = Scene::new("wp0-shader-reload");
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
    scene
}

/// 渲染一帧并取中心像素（`None` 场景 = 只走 3D 通道）。
fn center_pixel(
    renderer: &mut Renderer,
    texture: &wgpu::Texture,
    view: &wgpu::TextureView,
) -> [u8; 3] {
    let mut runtime = RuntimeScene::from_scene_edit(None, ground_scene()).expect("runtime scene");
    let viewport = Viewport::new(SIZE as f32, SIZE as f32).expect("viewport");
    let extracted = runtime.scene_3d(viewport).expect("3D extraction");
    let mesh = runtime.vertices(SIZE, SIZE).expect("2D extraction");
    assert!(
        matches!(
            renderer.render_into_texture(&mesh, Some(&extracted), view, SIZE, SIZE),
            RenderOutcome::Presented
        ),
        "帧必须正常提交"
    );
    let pixels = renderer
        .copy_texture_to_cpu(texture, SIZE, SIZE)
        .expect("readback");
    let index = ((SIZE / 2 * SIZE + SIZE / 2) * 4) as usize;
    [pixels[index], pixels[index + 1], pixels[index + 2]]
}

fn main() -> ExitCode {
    let output = parse_output();
    let fixture =
        match OffscreenTarget::acquire(&OffscreenRequest::new("wp0-shader-hot-reload", SIZE, SIZE))
        {
            Ok(fixture) => fixture,
            Err(_) => {
                eprintln!(
                    "BLOCKED: no GPU adapter available; shader hot reload cannot run (没跑 ≠ 通过)"
                );
                return ExitCode::from(3);
            }
        };
    let device = fixture.device();
    let queue = fixture.queue().clone();
    let info = fixture.adapter_info();
    let mut renderer =
        Renderer::new_offscreen(device.clone(), queue, None, false, false).expect("offscreen");
    let texture = fixture.texture();
    let view = fixture.view();

    // 第 0 步：内置档读数。
    let before = center_pixel(&mut renderer, texture, view);
    let dir = TempDir::new();
    renderer
        .set_shader_override_dir(Some(dir.0.clone()))
        .expect("空目录 = 内置行为，不该报错");

    // ① 有效覆盖：片元恒红。
    write_file(
        &dir.0,
        "mesh3d.wgsl",
        &fragment_replaced("vec4<f32>(1.0, 0.0, 0.0, 1.0)"),
    );
    let report = match renderer.reload_shaders_if_changed() {
        Ok(Some(report)) => report,
        Ok(None) => {
            eprintln!("FAIL: 写入有效覆盖后没有任何重载（覆盖目录没生效？）");
            return ExitCode::FAILURE;
        }
        Err(error) => {
            eprintln!("FAIL: 有效覆盖被拒绝：{error}");
            return ExitCode::FAILURE;
        }
    };
    let reload_ms = report.validate_ms + report.compile_ms;
    let after = center_pixel(&mut renderer, texture, view);

    // ② 文件没动：一次编译都不做。
    let idle = renderer
        .reload_shaders_if_changed()
        .expect("未变目录不该报错");

    // ③ 类型校验失败：拒绝，画面保持红色。
    write_file(&dir.0, "mesh3d.wgsl", &fragment_replaced("1.0"));
    let rejected_type = renderer
        .set_shader_override_dir(Some(dir.0.clone()))
        .err()
        .map(|error| error.to_string());
    let pixels_after_type_error = center_pixel(&mut renderer, texture, view);

    // ④ 绑定接口漂移：拒绝，并给出 diff 文本。
    write_file(&dir.0, "mesh3d.wgsl", &binding_moved());
    let rejected_interface = renderer
        .set_shader_override_dir(Some(dir.0.clone()))
        .err()
        .map(|error| error.to_string());
    let pixels_after_interface_error = center_pixel(&mut renderer, texture, view);

    // ⑤ 删掉覆盖：回退内置源并点名。
    fs::remove_file(dir.0.join("mesh3d.wgsl")).expect("remove override file");
    let fallback = renderer
        .reload_shaders_if_changed()
        .expect("删除覆盖不是错误")
        .expect("删除改变了生效源，必须有报告");
    let pixels_after_fallback = center_pixel(&mut renderer, texture, view);

    let report_json = json!({
        "report_version": "wp0-shader-hot-reload-v1",
        "adapter": { "backend": format!("{:?}", info.backend), "name": info.name },
        "size": [SIZE, SIZE],
        "before_pixel": before,
        "after_pixel": after,
        "reload_ms": reload_ms,
        "validate_ms": report.validate_ms,
        "compile_ms": report.compile_ms,
        "variants": report.variants,
        "rebuilt": report.rebuilt,
        "unchanged": report.unchanged,
        "idle_rebuilt_on_unchanged_dir": idle.as_ref().map(|report| report.rebuilt),
        "rejected_reason_type_error": rejected_type,
        "rejected_reason_interface": rejected_interface,
        "pixels_after_rejects": [pixels_after_type_error, pixels_after_interface_error],
        "fell_back_to_builtin": fallback.fell_back_to_builtin,
        "pixels_after_fallback": pixels_after_fallback,
        "sources": report.sources_text(),
    });
    let text = serde_json::to_string_pretty(&report_json).expect("serialize golden") + "\n";
    match &output {
        Some(path) => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create output dir");
            }
            fs::write(path, &text).expect("write shader reload golden");
        }
        None => print!("{text}"),
    }

    // 负向门：任何一条不成立都以非 0 退出（报告已经打印，读数可直接对照）。
    let mut failures: Vec<String> = Vec::new();
    if !(after[0] > 200 && after[1] < 60 && after[2] < 60) {
        failures.push(format!(
            "覆盖没改变画面：before={before:?} after={after:?}（期望红色）"
        ));
    }
    if idle.is_some() {
        failures.push("内容逐字节相同的目录仍然重编了管线".to_owned());
    }
    if rejected_type.is_none() {
        failures.push("类型错误的片元没有被拒绝".to_owned());
    }
    if rejected_interface.is_none() {
        failures.push("绑定接口漂移没有被拒绝".to_owned());
    }
    if pixels_after_type_error != after || pixels_after_interface_error != after {
        failures.push(format!(
            "拒绝之后画面变了：红色={after:?} 拒绝后={pixels_after_type_error:?}/{pixels_after_interface_error:?}"
        ));
    }
    if !fallback
        .fell_back_to_builtin
        .iter()
        .any(|name| name == "mesh3d")
    {
        failures.push(format!(
            "删除覆盖后没有点名回退内置源：{:?}",
            fallback.fell_back_to_builtin
        ));
    }
    if pixels_after_fallback != before {
        failures.push(format!(
            "回退后的画面与内置档不一致：内置={before:?} 回退后={pixels_after_fallback:?}"
        ));
    }
    if !failures.is_empty() {
        for failure in &failures {
            eprintln!("FAIL: {failure}");
        }
        return ExitCode::FAILURE;
    }
    println!(
        "OK: shader hot reload 五门通过 · before={before:?} → after={after:?} · reload {reload_ms:.2}ms · 回退 {:?}",
        fallback.fell_back_to_builtin
    );
    ExitCode::SUCCESS
}

fn write_file(dir: &Path, name: &str, text: &str) {
    fs::write(dir.join(name), text).expect("write override file");
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
                println!("Usage: wp0_shader_hot_reload [--output PATH]");
                std::process::exit(0);
            }
            _ => panic!("unknown argument: {flag}"),
        }
    }
    output
}
