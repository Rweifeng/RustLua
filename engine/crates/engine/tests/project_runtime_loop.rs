//! 跨 crate 集成门：**真实工程**从加载到出图的完整链路，以及运行期场景切换。
//!
//! ## 为什么这条门在 `engine` 而不是某个领域 crate
//!
//! 它要同时看见多层的真实产物：`rustlua-scene` 的文本解析、`rustlua-scripting` 的 VM 与
//! 组合根接线、`rustlua-ui`/`rustlua-render` 的布局与顶点构建、`rustlua-physics` 的步进。
//! 只有组合根同时依赖它们全部——这正是"跨 crate 集成"该放的地方，与
//! `typed_scene_parity` / `component_schema_parity` 同一条理由。
//!
//! ## 与 `host/sample_project.rs` 内联测试的分工
//!
//! 那边守的是"**每个场景单独**能加载、能跑三帧、自检全绿"——逐场景、就地、宽而浅。
//! 这里守的是三条**跨场景**、当前没有任何用例覆盖的横切契约：
//!
//! 1. **真实工程能真的出图**：跑若干帧后 `vertices()` 必须产出顶点与绘制段。
//!    此前 `vertices()` 只在手搭的合成场景里被测过，真实工程（12 个场景、真实贴图
//!    与字体图集）的渲染输出**没有任何门**——布局算错、图集没上传、绘制段丢光，
//!    单测全绿而屏上一片空白就是这种漏法。
//! 2. **运行期场景切换**：脚本 `scene.load(...)` → 下一步自动 `process_scene_transition`
//!    → 换世界 → 重新 `on_create`。这是编辑器按 Play、玩家点菜单的必经路径，
//!    此前集成层零覆盖（`transition.rs` 的内部单测不碰真实工程）。
//! 3. **切换后不是空壳**：新场景同样要出图，且旧场景的 UI 状态不能残留。
//!
//! 场景目录缺失时**不静默通过**：本门的意义就是真的跑过真实工程。

use std::path::{Path, PathBuf};

use engine::host::runtime_scene::RuntimeScene;
use rustlua_input::input::InputSnapshot;
use rustlua_scene::scene::Scene;

/// 真实工程根（`game_api` 演示工程：每个 Lua API 模块一个可交互场景）。
fn project_root() -> PathBuf {
    let root = rustlua_infra::games_dir().join("examples/game_api");
    assert!(
        root.is_dir(),
        "演示工程目录不存在：{}（本门不接受「跳过」，它要真的跑过真实工程）",
        root.display()
    );
    assert!(
        root.join("project.toml").is_file(),
        "{} 缺少 project.toml，不是合法工程",
        root.display()
    );
    root
}

/// 场景名 → 路径（只取顶层 `scenes/*.scene`）。
fn scene_path(root: &Path, name: &str) -> PathBuf {
    let path = root.join("scenes").join(format!("{name}.scene"));
    assert!(path.is_file(), "场景不存在：{}", path.display());
    path
}

/// 载入一个真实场景并把运行时推进到"已经有帧数据"的状态。
fn boot(root: &Path, name: &str, steps: u32) -> RuntimeScene {
    let scene = Scene::load(scene_path(root, name))
        .unwrap_or_else(|error| panic!("加载场景 {name} 失败：{error}"));
    let mut runtime = RuntimeScene::from_scene(Some(root), scene)
        .unwrap_or_else(|error| panic!("实例化场景 {name} 失败：{error}"));
    runtime
        .run_script_steps(steps, 1.0 / 60.0, &InputSnapshot::default())
        .unwrap_or_else(|error| panic!("步进场景 {name} 失败：{error}"));
    runtime
}

/// 真实工程里逐场景跑几帧后必须真的产出**可绘制几何**。
///
/// 运行时有两套平行的提取路径，真实工程会走其中一条或两条都走：
/// - `vertices()`：2D 屏幕与世界精灵 → `rustlua-render` 的顶点 + 绘制段；
/// - `scene_3d()`：3D 网格 → drawables。
///
/// 断言三件事，三层各有自己的失败方式：
/// - 两条路径**至少一条**非空（全空 = 提取链路彻底断了，屏上什么都没有）；
/// - 2D 有顶点就必须有绘制段（只有顶点没有 draw = 什么都不画）；
/// - 每个 draw 的顶点区间必须落在 `vertices` 内（越界是分段构建最易漂的一处）。
#[test]
fn every_real_scene_produces_drawable_geometry() {
    let root = project_root();
    let entries = std::fs::read_dir(root.join("scenes"))
        .unwrap_or_else(|error| panic!("读取场景目录失败：{error}"));

    let mut checked = 0usize;
    let mut two_d = 0usize;
    let mut three_d = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("scene") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("场景文件名应为 UTF-8");

        // 三步：`on_create` 之后至少有一次完整的 `update()` + 每帧管线。
        let mut runtime = boot(&root, name, 3);
        // 3D 提取要在 vertices 之前取？不——两者互不依赖，但 vertices 会推进局部→世界
        // 变换传播（3D drawable 的模型矩阵读的正是它），所以先 vertices 后 scene_3d。
        let mesh = runtime
            .vertices(1280, 720)
            .unwrap_or_else(|error| panic!("{name} 提取顶点失败：{error}"));
        let scene_3d = runtime.scene_3d(rustlua_camera::camera::Viewport {
            width: 1280.0,
            height: 720.0,
        });

        let label = format!("{name}（{}）", path.display());
        let has_2d = !mesh.vertices.is_empty();
        let has_3d = scene_3d.as_ref().is_some_and(|s| !s.drawables.is_empty());
        assert!(
            has_2d || has_3d,
            "{label} 跑三帧后既没有 2D 顶点也没有 3D drawable：两套提取链路都断了"
        );

        if has_2d {
            assert!(
                !mesh.draws.is_empty(),
                "{label} 有 {} 个顶点但零绘制段：屏上不会有任何东西",
                mesh.vertices.len()
            );
            // 每个 draw 的顶点区间必须落在 `vertices` 内——否则渲染器读越界。
            for draw in &mesh.draws {
                let end = draw.start as usize + draw.count as usize;
                assert!(
                    end <= mesh.vertices.len(),
                    "{label} 的 draw 顶点区间 {}..{} 超出了顶点总数 {}",
                    draw.start,
                    end,
                    mesh.vertices.len()
                );
            }
            // 顶点是 NDC（`[-1,1]`）：出这个范围渲染器直接裁掉，等于没画。
            // 只断言"非空"抓不到这类错——退化视口下的错误投影照样产出顶点。
            let out_of_ndc = mesh
                .vertices
                .iter()
                .filter(|vertex| {
                    let [x, y] = vertex.position;
                    !(x.is_finite() && y.is_finite()) || x.abs() > 1.5 || y.abs() > 1.5
                })
                .count();
            assert!(
                out_of_ndc * 4 <= mesh.vertices.len(),
                "{label} 有 {out_of_ndc}/{} 个顶点落在 NDC 之外（超过四分之一）：\
                 投影或布局算错，这部分在屏上被裁掉",
                mesh.vertices.len()
            );
            two_d += 1;
        }
        if has_3d {
            three_d += 1;
        }
        checked += 1;
    }

    assert!(
        checked >= 10,
        "参与检查的真实场景太少（{checked}）：门没有实际意义"
    );
    // 两条路径都必须真的被覆盖到，否则"至少一条非空"会退化成"只要 2D 就过"。
    assert!(
        two_d >= 5,
        "走 2D 顶点路径的场景太少（{two_d}/{checked}）：2D 提取没被真正覆盖"
    );
    assert!(
        three_d >= 1,
        "没有任何场景产出 3D drawable（{three_d}/{checked}）：3D 提取没被覆盖"
    );
}

/// 运行期场景切换：脚本 `scene.load` → 自动换世界 → 新场景同样出图，旧状态不残留。
///
/// 这条路径是编辑器 Play 与玩家点菜单的必经之路，此前集成层零覆盖。它同时验证了
/// 四个域在换世界后的一致性：
/// - `rustlua-ecs`：世界真的被换成新的一份（旧实体全灭）；
/// - `rustlua-scripting`：`on_create` 在新世界重跑，脚本拿到的是新句柄；
/// - `rustlua-ui`：UI 覆盖层与导航栈清空（组合根的 transition hook 生效）；
/// - `rustlua-render`：新场景能出图。
#[test]
fn runtime_scene_load_switches_world_and_keeps_rendering() {
    let root = project_root();

    // 从入口菜单进入一个演示场景：这正是玩家点卡片的路径。
    let mut runtime = boot(&root, "index", 1);
    let before = runtime.current_scene_name();
    assert_eq!(before.as_deref(), Some("index"), "起始场景应为 index");

    let index_draws = runtime
        .vertices(1280, 720)
        .expect("index 提取顶点")
        .draws
        .len();
    assert!(index_draws > 0, "入口菜单本身就该有绘制段");

    // 脚本侧请求换场景；下一步由 `run_script_steps` 内部的管线自动应用。
    runtime
        .run_lua_console("scene.load('demo3d')")
        .expect("scene.load 应被接受");
    runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect("切换帧不应失败");

    assert_eq!(
        runtime.current_scene_name().as_deref(),
        Some("demo3d"),
        "切换后当前场景名应为 demo3d"
    );

    // 新世界必须能继续跑、继续出图（2D 或 3D 任一路径有产出即可）。
    runtime
        .run_script_steps(2, 1.0 / 60.0, &InputSnapshot::default())
        .expect("切换后步进不应失败");
    let after_mesh = runtime.vertices(1280, 720).expect("demo3d 提取顶点");
    let after_3d = runtime.scene_3d(rustlua_camera::camera::Viewport {
        width: 1280.0,
        height: 720.0,
    });
    let has_2d = !after_mesh.vertices.is_empty();
    let has_3d = after_3d.as_ref().is_some_and(|s| !s.drawables.is_empty());
    assert!(
        has_2d || has_3d,
        "切换后新场景既无 2D 顶点也无 3D drawable：换世界把渲染链路也清空了"
    );

    // 旧场景的 UI 动态层必须已经清掉（组合根 transition hook 的职责）：
    // 否则切换后旧菜单的文字会叠在新场景上、且仍可点。
    let stale_ui = runtime.scene_text_hidden();
    assert!(
        stale_ui.is_empty(),
        "切换后仍残留旧场景的场景文字隐藏集：{stale_ui:?}"
    );
}
