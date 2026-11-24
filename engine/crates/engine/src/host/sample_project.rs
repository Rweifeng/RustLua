//! `examples/game_api` 的**工程级门**：示例工程是"活文档"，它的价值全在"真的能跑"。
//!
//! 五类漂移只有跑起来才看得见，所以这里各有一条门钉住：
//!
//! | 门 | 钉住的漂移 |
//! |---|---|
//! | [`sample_project_runs_every_scene_headless`] | 场景加载不了、脚本在实例化或步进期报错 |
//! | [`sample_project_demos_self_test`] | API 还能调用，但读回语义已经错了（步骤的 `check` 失败） |
//! | [`sample_project_scripts_all_compile`] | 演示模块语法错（`require` 不到就整段失效） |
//! | [`sample_project_demos_cover_every_lua_api`] | 引擎加了 API 却没有演示 |
//! | [`sample_project_scenes_cover_every_runtime_component`] | 引擎加了组件却没进场景 |
//! | [`sample_project_index_menu_lists_every_scene`] | 新增演示场景却没进入口菜单 |
//! | [`sample_project_index_click_opens_the_demo`] | 菜单点击没有真的切场景 |
//! | [`sample_project_index_labels_are_inside_their_cards`] | 卡片文字偏出父矩形（子实体锚点是父矩形） |
//! | [`sample_project_index_labels_fit_their_cards`] | 卡片文字实测宽度顶出卡片右缘（字号/文案变长） |
//!
//! 覆盖面的**事实源是运行期**（`_G` 里的表与函数），不是任何文档或手抄清单：
//! 引擎新注册一个 Lua 函数，这条门立刻要求示例工程补上演示。

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use super::RuntimeScene;
use rustlua_input::input::{InputSnapshot, KeyCode, MouseButton};
use rustlua_scene::scene::Scene;
use rustlua_scene::typed_scene::ComponentId;
use rustlua_scripting::script::{LUA_API_PROBE, LuaRuntime};

fn project_root() -> PathBuf {
    rustlua_infra::games_dir().join("examples").join("game_api")
}

/// 入口菜单自身的场景名：它是演示的**目录**，不是演示之一，因此不进 `demos.lua`。
const MENU_SCENE: &str = "index";

/// 工程里的全部场景文件（按名字排序）。
fn scene_files(root: &Path) -> Vec<PathBuf> {
    let mut files = fs::read_dir(root.join("scenes"))
        .unwrap_or_else(|error| panic!("读取 {} 失败：{error}", root.join("scenes").display()))
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "scene")
        })
        .collect::<Vec<_>>();
    files.sort();
    assert!(!files.is_empty(), "示例工程没有任何场景");
    files
}

/// 递归收集 `.lua` 文件。
fn lua_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            lua_files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "lua") {
            out.push(path);
        }
    }
}

fn script_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    lua_files(&root.join("scripts"), &mut files);
    files.sort();
    assert!(!files.is_empty(), "示例工程没有任何脚本");
    files
}

fn scene_name(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// M81 批 3：2D 构建（无 `3d` feature）下，静态引用了**本构建面外 API** 的演示场景
/// 整场跳过并打印原因；全量构建零跳过（行为与从前逐字节一致）。
///
/// 判据与覆盖率门同一口径：剥掉注释/字符串后，`表.函数(` / `表.函数,` / `表.函数)`
/// 三种真实调用形状才算引用（提示文案里的名字不算）。函数级对照是必须的——
/// `camera`/`particles`/`render` 是混合表，整表缺席判据会漏掉 `camera.set_clip_plane`
/// 这类只裁掉个别函数的引用。场景↔脚本的对应取 `demos.lua` 的 `module` 字段，
/// 共享脚本（api_demo/lab/router 等）不算——它们被所有场景 require，算进去会全场连坐。
#[cfg(not(feature = "3d"))]
fn out_of_surface_skip(root: &Path, name: &str) -> Option<String> {
    let runtime = crate::host::script_packs::runtime(rustlua_gameplay::input_lua::registry())
        .expect("lua vm")
        .0;
    let surface = runtime
        .eval_json(LUA_API_PROBE)
        .expect("导出运行时 API 面失败");
    let present: BTreeSet<String> = surface["tables"]
        .as_array()
        .expect("tables")
        .iter()
        .flat_map(|table| {
            let name = table["name"].as_str().expect("table name").to_owned();
            table["funcs"]
                .as_array()
                .expect("funcs")
                .iter()
                .map(move |func| format!("{name}.{}", func.as_str().expect("func name")))
        })
        .collect();
    let full: serde_json::Value = serde_json::from_str(include_str!(
        "../../../rustlua-scripting/src/api_surface_snapshot/lua_api_surface.json"
    ))
    .expect("全量 API 面基线");
    let absent: Vec<String> = full["tables"]
        .as_array()
        .expect("tables")
        .iter()
        .flat_map(|table| {
            let name = table["name"].as_str().expect("table name").to_owned();
            table["funcs"]
                .as_array()
                .expect("funcs")
                .iter()
                .map(move |func| format!("{name}.{}", func.as_str().expect("func name")))
        })
        .filter(|key| !present.contains(key))
        .collect();

    let manifest = fs::read_to_string(root.join("scripts").join("demos.lua"))
        .expect("读取 scripts/demos.lua 失败");
    let mut module = None;
    let mut in_scene = false;
    for line in manifest.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("scene =") {
            in_scene = rest.contains(&format!("\"{name}\""));
        } else if in_scene && let Some(rest) = line.strip_prefix("module =") {
            module = rest.split('"').nth(1).map(|value| value.to_owned());
            break;
        }
    }
    let module = module?;
    let path = root.join("scripts").join(format!("{module}.lua"));
    let source = fs::read_to_string(&path).ok()?;
    let code = strip_lua_noise(&source);
    let hit = absent.iter().find(|key| {
        ["(", ",", ")"]
            .iter()
            .any(|suffix| code.contains(&format!("{key}{suffix}")))
    })?;
    Some(format!(
        "跳过 {name}：{module}.lua 引用了本构建面外的 {hit}（2D 构建无 3d feature）"
    ))
}

/// 全量构建下跳过规则不存在（零行为变化）；调用点用同名函数保持两维同形。
#[cfg(feature = "3d")]
fn out_of_surface_skip(_root: &Path, _name: &str) -> Option<String> {
    None
}

/// `scripts/demos.lua` 里登记的演示场景名（清单是唯一真源）。
fn registered_scenes(root: &Path) -> BTreeSet<String> {
    let text = fs::read_to_string(root.join("scripts").join("demos.lua"))
        .expect("读取 scripts/demos.lua 失败");
    let mut scenes = BTreeSet::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("scene =") else {
            continue;
        };
        if let Some(start) = rest.find('"')
            && let Some(end) = rest[start + 1..].find('"')
        {
            scenes.insert(rest[start + 1..start + 1 + end].to_owned());
        }
    }
    assert!(
        scenes.len() >= 10,
        "scripts/demos.lua 里只解析出 {} 个演示场景，清单可能被改坏了",
        scenes.len()
    );
    scenes
}

/// 去掉 Lua 的注释与字符串字面量，只留**代码**。
///
/// 为什么必须去掉字符串：门的判据是"真的调用了这个 API"。有两个真实反例——
/// `map.goto(` 在 Lua 里**根本写不出来**（`goto` 是 5.2+ 保留字，真实调用只能写
/// `map["goto"](`），而屏显文案/注释里写 `map.goto(` 却毫无成本；`input.actions()` 之类的
/// 名字也常出现在提示文本里。去掉字符串后，只有真正的调用点才会命中。
/// `scripts/demos.lua` 里登记的 场景 → 演示模块 映射（能力图谱门用它找 `M.steps`）。
fn scene_modules(root: &Path) -> BTreeMap<String, String> {
    let text = fs::read_to_string(root.join("scripts").join("demos.lua"))
        .expect("读取 scripts/demos.lua 失败");
    let mut modules = BTreeMap::new();
    let mut scene = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("scene =")
            && let Some(start) = rest.find('"')
            && let Some(end) = rest[start + 1..].find('"')
        {
            scene = Some(rest[start + 1..start + 1 + end].to_owned());
        } else if let Some(rest) = line.strip_prefix("module =")
            && let Some(start) = rest.find('"')
            && let Some(end) = rest[start + 1..].find('"')
            && let Some(name) = scene.take()
        {
            modules.insert(name, rest[start + 1..start + 1 + end].to_owned());
        }
    }
    assert!(
        modules.len() >= 10,
        "scripts/demos.lua 里只解析出 {} 条场景↔模块，清单可能被改坏了",
        modules.len()
    );
    modules
}

/// 让测试用的 Lua VM 能按工程 `scripts/` 目录 `require` 演示模块。
///
/// 运行时的工程脚本走 `install_encoded_require_searcher`（发布包解码通道）；测试只需要
/// 普通文件通道，所以直接给 `package.path` 加一条前缀，与编辑器域数据面板的引导同一口径。
fn bootstrap_project_require(runtime: &mut LuaRuntime, root: &Path) {
    let scripts = root
        .join("scripts")
        .display()
        .to_string()
        .replace('\\', "/");
    runtime
        .execute_console(&format!(
            "package.path = [[{scripts}/?.lua;{scripts}/?/init.lua;]] .. package.path"
        ))
        .expect("安装工程 require 路径失败");
}

/// `scripts/capabilities.lua` 的 JSON 形态（能力图谱是演示工程 3.0 的单一真源）。
fn capability_catalog(root: &Path) -> serde_json::Value {
    let mut runtime = crate::host::script_packs::runtime(rustlua_gameplay::input_lua::registry())
        .expect("lua vm")
        .0;
    bootstrap_project_require(&mut runtime, root);
    runtime
        .eval_json("require('capabilities')")
        .unwrap_or_else(|error| panic!("读取 scripts/capabilities.lua 失败：{error}"))
}

/// 一个演示模块导出的步骤 id 集合（各 `demo_*.lua` 末尾的 `M.steps = steps`）。
fn module_step_ids(runtime: &LuaRuntime, module: &str) -> BTreeSet<String> {
    let script = format!(
        "(function() local m = require('{module}'); local ids = {{}}; \
         for _, step in ipairs(m.steps or {{}}) do ids[#ids + 1] = step.id end; return ids end)()"
    );
    let value = runtime
        .eval_json(&script)
        .unwrap_or_else(|error| panic!("读取 {module}.lua 的步骤清单失败：{error}"));
    value
        .as_array()
        .unwrap_or_else(|| panic!("{module}.steps 不是数组：{value}"))
        .iter()
        .map(|id| id.as_str().expect("step id").to_owned())
        .collect()
}

#[test]
fn sample_project_capability_catalog_is_complete() {
    let root = project_root();
    let registered = registered_scenes(&root);
    let modules = scene_modules(&root);
    let catalog = capability_catalog(&root);
    let mut runtime = crate::host::script_packs::runtime(rustlua_gameplay::input_lua::registry())
        .expect("lua vm")
        .0;
    bootstrap_project_require(&mut runtime, &root);

    let groups: BTreeSet<String> = catalog["groups"]
        .as_array()
        .expect("capabilities.groups")
        .iter()
        .map(|group| group["id"].as_str().expect("group id").to_owned())
        .collect();
    assert!(
        groups.len() >= 5,
        "能力分组太少（{}）：{groups:?}",
        groups.len()
    );

    let capabilities = catalog["capabilities"]
        .as_array()
        .expect("capabilities.capabilities");
    assert!(
        capabilities.len() >= 40,
        "能力图谱只有 {} 条，引擎能力面不可能这么小",
        capabilities.len()
    );

    let mut ids = BTreeSet::new();
    let mut scenes_with_capabilities = BTreeSet::new();
    let mut step_cache: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for capability in capabilities {
        let id = capability["id"].as_str().expect("capability id");
        assert!(
            ids.insert(id.to_owned()),
            "能力 id 重复：{id}（能力图谱要能按 id 被文档与门引用）"
        );
        let group = capability["group"]
            .as_str()
            .unwrap_or_else(|| panic!("{id} 缺 group"));
        assert!(
            groups.contains(group),
            "{id} 的 group={group} 不在 groups 里"
        );
        let kind = capability["kind"].as_str().unwrap_or("feature");
        let Some(scene) = capability["scene"].as_str() else {
            assert!(
                kind == "platform",
                "{id}：只有 platform 条目可以不带 scene（它不属于某一个演示场景）"
            );
            continue;
        };
        assert!(
            registered.contains(scene),
            "{id}：scene={scene} 没有登记在 scripts/demos.lua"
        );
        scenes_with_capabilities.insert(scene.to_owned());
        match capability["step"].as_str() {
            Some(step) => {
                let module = modules
                    .get(scene)
                    .unwrap_or_else(|| panic!("{id}：场景 {scene} 没有演示模块"));
                let steps = step_cache
                    .entry(module.clone())
                    .or_insert_with(|| module_step_ids(&runtime, module));
                assert!(
                    steps.contains(step),
                    "{id}：场景 {scene} 的模块 {module}.lua 没有步骤 {step}（现有：{steps:?}）"
                );
            }
            None => assert!(
                matches!(kind, "golden" | "platform"),
                "{id}：feature 条目必须写 step（否则读者不知道该看哪一步）"
            ),
        }
    }

    for scene in &registered {
        assert!(
            scenes_with_capabilities.contains(scene),
            "演示场景 {scene} 在能力图谱里没有任何条目：读者说不清它演示了什么能力"
        );
    }

    // `demos.lua` 的 `modules` 字段是"这个场景覆盖哪些 API 模块"的人读清单；
    // 它的并集必须等于运行期 API 面，否则清单会悄悄过期（新增模块没人认领）。
    #[cfg(feature = "3d")]
    {
        let mut listed = BTreeSet::new();
        let demos_text = fs::read_to_string(root.join("scripts").join("demos.lua"))
            .expect("读取 scripts/demos.lua 失败");
        for line in demos_text.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("modules = {") else {
                continue;
            };
            let rest = rest.trim().trim_end_matches(',').trim_end_matches('}');
            for part in rest.split(',') {
                let part = part.trim();
                if let Some(name) = part.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
                    listed.insert(name.to_owned());
                }
            }
        }
        let surface = runtime
            .eval_json(LUA_API_PROBE)
            .expect("导出运行时 API 面失败");
        let live: BTreeSet<String> = surface["tables"]
            .as_array()
            .expect("tables")
            .iter()
            .map(|table| table["name"].as_str().expect("table name").to_owned())
            .collect();
        let missing: Vec<&String> = live.difference(&listed).collect();
        let extra: Vec<&String> = listed.difference(&live).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "scripts/demos.lua 的 modules 清单与运行期 API 面不一致：缺 {missing:?}，多 {extra:?}"
        );
    }
}

/// 渲染能力的**单一真源是 `RenderSettings`**：新增/删除一个可开能力，这条门立刻要求
/// 能力图谱补上对应条目；`[render]` 的项目级键与 `.sgraph` 资产也在这里真解析一遍。
#[test]
fn sample_project_render_capabilities_are_demonstrated() {
    let root = project_root();
    let catalog = capability_catalog(&root);
    let capabilities = catalog["capabilities"].as_array().expect("capabilities");

    // 从源码解析 `pub xxx: bool` / `pub xxx: ToggleMode`（RenderSettings 的全部开档）。
    let settings_source = include_str!("../../../rustlua-render/src/render_settings.rs");
    let mut live_settings = BTreeSet::new();
    for line in settings_source.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("pub ") else {
            continue;
        };
        let Some((name, ty)) = rest.split_once(':') else {
            continue;
        };
        let ty = ty.trim().trim_end_matches(',');
        if ty == "bool" || ty == "ToggleMode" {
            live_settings.insert(name.trim().to_owned());
        }
    }
    assert!(
        live_settings.len() >= 10,
        "render_settings.rs 里只解析出 {} 个布尔开档，解析规则可能失效：{live_settings:?}",
        live_settings.len()
    );

    let mut claimed = BTreeSet::new();
    for capability in capabilities {
        if capability["group"].as_str() != Some("render") {
            continue;
        }
        if let Some(setting) = capability["setting"].as_str() {
            claimed.insert(setting.to_owned());
        }
    }
    let missing: Vec<&String> = live_settings.difference(&claimed).collect();
    assert!(
        missing.is_empty(),
        "这些渲染能力没有能力图谱条目（capability.setting）：{missing:?}"
    );

    // 项目级渲染键：`[render]` 段必须真的被解析出非默认值。
    let project_text =
        fs::read_to_string(root.join("project.toml")).expect("读取 project.toml 失败");
    let config = rustlua_render::render_config::RenderConfig::from_toml(&project_text);
    // 叠加层属于**场景**语义：`[render]` 段不该再有这一项（有的话所有场景都会带上刀光）。
    assert!(
        !project_text.contains("effect_overlay"),
        "特效叠加层应写在场景里（meta\teffect_overlay），不该留在 project.toml：{project_text}"
    );
    assert!(
        config.particle_softness > 0.0,
        "project.toml [render] particle_softness 应打开（0 = 硬边）"
    );

    // 声明叠加层的场景必须真的声明它，而且能被运行时同一条路径编译。
    // 演示工程里用炎刃斩击的那个场景（不是所有场景——这正是本次修的点）。
    let relative = ["scenes/api_fx.scene", "scenes/index.scene"]
        .iter()
        .find_map(|relative_path| {
            let scene = Scene::load(root.join(relative_path)).ok()?;
            // 先取出字符串再交出 scene：`effect_overlay()` 借的是 scene 自己，直接元组会冲突。
            let overlay = scene.effect_overlay()?.to_owned();
            Some(overlay)
        })
        .unwrap_or_else(|| {
            panic!("演示工程里没有任何场景声明 meta\teffect_overlay；炎刃斩击演示会失去特效")
        });
    let (source, compiled) =
        rustlua_visual_script::shader_graph::ShaderGraph::load_runtime_overlay(&root, &relative)
            .unwrap_or_else(|error| panic!("{relative} 无法编译成运行时叠加层：{error}"));
    assert!(
        source.contains("@binding(6)"),
        "深度描边示例应当采样场景深度（binding 6），否则它和纯程序化模板没有区别"
    );
    let named_params = compiled.named_params();
    let named: BTreeSet<&str> = named_params
        .iter()
        .map(|param| param.name.as_str())
        .collect();
    assert!(
        named.contains("edge_width"),
        "`.sgraph` 应暴露 edge_width 命名参数，现有：{named:?}"
    );
    assert!(
        named.iter().any(|name| name.starts_with("color_")),
        "`.sgraph` 应暴露描边颜色参数，现有：{named:?}"
    );
    // 炎刃斩击演示逐帧按名字调 spin/speed/frequency；资产必须真的暴露它们，
    // 否则 set_effect_param 会返回 false，演示只是"看起来在调"却没有效果。
    for required in ["spin", "speed", "frequency"] {
        assert!(
            named.contains(required),
            "炎刃斩击 `.sgraph` 应暴露命名参数 {required}（demo_3d 逐帧调它），现有：{named:?}"
        );
    }

    let (sources, _raw) = demo_sources(&root);
    for call in [
        "render.set_effect_params(",
        "render.set_effect_param(",
        "render.set_effect_time(",
    ] {
        assert!(
            sources.contains(call),
            "演示脚本缺少对 {call}) 的真实调用（M94/M95 的运行时参数面没有演示）"
        );
    }

    // M96：逻辑里按预设播特效。三条一起查——只有 `render.play_effect` 而没有预设资产，
    // 演示跑起来会一路报"加载失败"却仍然是绿的。
    for call in ["render.play_effect(", "render.stop_effect("] {
        assert!(
            sources.contains(call),
            "演示脚本缺少对 {call}) 的真实调用（特效预设面没有演示）"
        );
    }
    for preset in [
        "assets/fx/flame_blade_slash.prefab",
        "assets/fx/frost_blade_slash.prefab",
    ] {
        let effect = rustlua_visual_script::effect_prefab::EffectPrefab::load(&root, preset)
            .unwrap_or_else(|error| panic!("{preset} 无法加载为特效预设：{error}"));
        assert!(effect.overlay, "{preset} 应当挂到场景叠加层");
        assert!(
            !effect.compiled.slots.is_empty(),
            "{preset} 编译后应当有参数槽（否则脚本调参全部落空）"
        );
    }
}

/// UI 能力的覆盖判据分两层：运行时**组件**（场景 UI）与 `ui.*` **控件/状态 API**。
#[test]
fn sample_project_ui_capabilities_are_demonstrated() {
    let root = project_root();
    let catalog = capability_catalog(&root);
    let capabilities = catalog["capabilities"].as_array().expect("capabilities");

    // 运行时 UI 组件（与 `ComponentId::ALL` 同源：组件删了这里会先报"已不存在"）。
    let expected_components = [
        "Text",
        "Image",
        "UiLayout",
        "Container",
        "LayoutItem",
        "UiEvents",
        "ButtonStates",
        "Mask",
        "CanvasGroup",
        "Toggle",
        "Slider",
        "Progress",
        "Scrollbar",
        "ScrollView",
        "UiVirtualList",
    ];
    let live_components: BTreeSet<&str> = ComponentId::ALL.iter().map(|id| id.as_str()).collect();

    let mut claimed_components = BTreeSet::new();
    let mut claimed_api = BTreeSet::new();
    let mut ui_capabilities = 0usize;
    for capability in capabilities {
        let group = capability["group"].as_str().unwrap_or("");
        if !group.starts_with("ui") {
            continue;
        }
        ui_capabilities += 1;
        if let Some(components) = capability["components"].as_array() {
            for component in components {
                claimed_components.insert(component.as_str().expect("component").to_owned());
            }
        }
        if let Some(items) = capability["api"].as_array() {
            for item in items {
                claimed_api.insert(item.as_str().expect("api").to_owned());
            }
        }
    }
    assert!(
        ui_capabilities >= 12,
        "UI 能力条目只有 {ui_capabilities} 条，覆盖不了场景 UI + 运行期 UI 两层"
    );
    for component in expected_components {
        assert!(
            live_components.contains(component),
            "能力图谱认领的组件 {component} 已不在 ComponentId::ALL 里"
        );
        assert!(
            claimed_components.contains(component),
            "运行时 UI 组件 {component} 没有被任何 UI 能力条目认领"
        );
    }
    for api in [
        "ui.panel",
        "ui.text_at",
        "ui.image",
        "ui.measure_text",
        "ui.grid",
        "ui.drag_begin",
        "ui.tooltip",
        "ui.modal",
        "ui.focus_open",
        "ui.nav_stack",
        "ui.scroll_by",
        "ui.scrollbar",
        "ui.virtual_list",
        "ui.rich_text",
        "ui.text_input",
        "ui.view_model_set",
        "ui.toggle",
        "ui.slider",
        "ui.progress",
    ] {
        assert!(
            claimed_api.contains(api),
            "UI 控件/状态 API {api} 没有被任何 UI 能力条目认领"
        );
    }
}

fn strip_lua_noise(source: &str) -> String {
    #[derive(PartialEq)]
    enum State {
        Code,
        LineComment,
        LongComment(usize),
        Text(u8),
        LongText(usize),
    }

    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut state = State::Code;
    let mut index = 0;
    // 长括号的数量：`[[` / `[=[` / `[==[` …（注释与长字符串共用同一套定界符）。
    let long_bracket = |index: usize| -> Option<(usize, usize)> {
        if bytes.get(index) != Some(&b'[') {
            return None;
        }
        let mut level = 0;
        let mut cursor = index + 1;
        while bytes.get(cursor) == Some(&b'=') {
            level += 1;
            cursor += 1;
        }
        (bytes.get(cursor) == Some(&b'[')).then_some((level, cursor))
    };
    let closes_long = |index: usize, level: usize| -> Option<usize> {
        if bytes.get(index) != Some(&b']') {
            return None;
        }
        let mut cursor = index + 1;
        for _ in 0..level {
            if bytes.get(cursor) != Some(&b'=') {
                return None;
            }
            cursor += 1;
        }
        (bytes.get(cursor) == Some(&b']')).then_some(cursor)
    };

    while index < bytes.len() {
        let byte = bytes[index];
        match state {
            State::Code => {
                if byte == b'-' && bytes.get(index + 1) == Some(&b'-') {
                    state = match long_bracket(index + 2) {
                        Some((level, _)) => State::LongComment(level),
                        None => State::LineComment,
                    };
                    out.push(' ');
                    index += 2;
                    continue;
                }
                if byte == b'"' || byte == b'\'' {
                    state = State::Text(byte);
                    out.push(' ');
                    index += 1;
                    continue;
                }
                if let Some((level, end)) = long_bracket(index) {
                    state = State::LongText(level);
                    out.push(' ');
                    index = end + 1;
                    continue;
                }
                out.push(byte as char);
                index += 1;
            }
            State::LineComment => {
                if byte == b'\n' {
                    state = State::Code;
                    out.push('\n');
                }
                index += 1;
            }
            State::LongComment(level) => {
                if let Some(end) = closes_long(index, level) {
                    state = State::Code;
                    index = end + 1;
                } else {
                    index += 1;
                }
            }
            State::Text(quote) => {
                if byte == b'\\' {
                    index += 2;
                    continue;
                }
                if byte == quote {
                    state = State::Code;
                }
                index += 1;
            }
            State::LongText(level) => {
                if let Some(end) = closes_long(index, level) {
                    state = State::Code;
                    index = end + 1;
                } else {
                    index += 1;
                }
            }
        }
    }
    out
}

/// 全部演示脚本的比对文本：`(去注释去字符串, 只去注释)`。
///
/// 为什么要两份：`map["goto"](...)` 的正确性判据需要**看得到**下标里的那个字符串
/// （`goto` 是保留字，`map.goto(` 写不出来），但普通的 `t.f(` 又必须排除"只在提示文案里
/// 出现过"的假命中。因此普通调用看剥掉字符串的那份，下标调用看保留字符串的那份。
fn demo_sources(root: &Path) -> (String, String) {
    let mut stripped = String::new();
    let mut raw = String::new();
    for path in script_files(root) {
        let source = fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("读取 {} 失败：{error}", path.display());
        });
        stripped.push_str(&strip_lua_noise(&source));
        stripped.push('\n');
        raw.push_str(&strip_lua_comments_only(&source));
        raw.push('\n');
    }
    (stripped, raw)
}

/// 只去掉注释（行注释与长注释），字符串字面量原样保留。
///
/// 与 [`strip_lua_noise`] 是同一个扫描骨架的"轻量版"：这里**不**跟踪字符串的结束位置，
/// 因为唯一目的是把注释抹掉；字符串里的 `--` 极少见，且误判只会让比对文本更宽松一点点。
fn strip_lua_comments_only(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'-' && bytes.get(index + 1) == Some(&b'-') {
            // 长注释 `--[[ … ]]` / `--[=[ … ]=]`
            let mut level = 0;
            let mut cursor = index + 2;
            if bytes.get(cursor) == Some(&b'[') {
                cursor += 1;
                while bytes.get(cursor) == Some(&b'=') {
                    level += 1;
                    cursor += 1;
                }
                if bytes.get(cursor) == Some(&b'[') {
                    cursor += 1;
                    loop {
                        if cursor >= bytes.len() {
                            index = cursor;
                            break;
                        }
                        if bytes[cursor] == b']' {
                            let mut probe = cursor + 1;
                            let mut seen = 0;
                            while seen < level && bytes.get(probe) == Some(&b'=') {
                                probe += 1;
                                seen += 1;
                            }
                            if seen == level && bytes.get(probe) == Some(&b']') {
                                index = probe + 1;
                                break;
                            }
                        }
                        cursor += 1;
                    }
                    out.push(' ');
                    continue;
                }
            }
            // 行注释：吃到行尾（换行保留，行号不错位）。
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        out.push(byte as char);
        index += 1;
    }
    out
}

fn lua_string_literal(source: &str) -> String {
    assert!(
        !source.contains("]==========]"),
        "脚本里出现长括号定界符，换一个定界符再编译"
    );
    source.to_owned()
}

#[test]
fn sample_project_runs_every_scene_headless() {
    let root = project_root();
    let mut stepped = 0;
    for path in scene_files(&root) {
        let name = scene_name(&path);
        if let Some(reason) = out_of_surface_skip(&root, &name) {
            println!("{reason}");
            continue;
        }
        let scene = Scene::load(&path).unwrap_or_else(|error| panic!("加载 {name} 失败：{error}"));
        let mut runtime = RuntimeScene::from_scene(Some(&root), scene)
            .unwrap_or_else(|error| panic!("实例化 {name} 失败：{error}"));
        // 三步足够让"每帧都跑的 update()"暴露参数错、缺实体、越界这类问题。
        runtime
            .run_script_steps(3, 1.0 / 60.0, &InputSnapshot::default())
            .unwrap_or_else(|error| panic!("步进 {name} 失败：{error}"));
        stepped += 1;
    }
    assert!(
        stepped >= 10,
        "示例工程的场景太少（{stepped}），门没有实际意义"
    );
}

/// 行为自检：每个非夹具场景的 `M.self_test()` 必须全绿。
///
/// 名字/覆盖率这两类门只能证明"API 被调用过"，证明不了读回语义——
/// 例如 `transform.set` 没有真的生效、`inv.add` 没计数、`calendar.configure`
/// 没改上界。2.0 的每个步骤都声明自己的 `check`，这里把它们统一跑一遍。
#[test]
fn sample_project_demos_self_test() {
    let root = project_root();
    let mut checked = 0;
    let mut skipped = 0;
    for path in scene_files(&root) {
        let name = scene_name(&path);
        // 入口菜单不是演示；两个夹具由渲染 golden 钉住，没有步骤。
        if name == "index" || name == "demo3d" || name == "mixed" {
            continue;
        }
        if let Some(reason) = out_of_surface_skip(&root, &name) {
            println!("{reason}");
            skipped += 1;
            continue;
        }
        let scene = Scene::load(&path).unwrap_or_else(|error| panic!("加载 {name} 失败：{error}"));
        let mut runtime = RuntimeScene::from_scene(Some(&root), scene)
            .unwrap_or_else(|error| panic!("实例化 {name} 失败：{error}"));
        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .unwrap_or_else(|error| panic!("首帧步进 {name} 失败：{error}"));
        let value = runtime
            .script
            .eval_json("__api_demo_self_test()")
            .unwrap_or_else(|error| panic!("{name} 自检调用失败：{error}"));
        let result = value
            .as_str()
            .unwrap_or_else(|| panic!("{name} 自检应返回字符串，实际 {value}"));
        assert!(result.starts_with("ok "), "{name} 行为自检失败：\n{result}");

        let mut steps = 0usize;
        let mut checks = 0usize;
        for part in result.split_whitespace() {
            if let Some(value) = part.strip_prefix("steps=") {
                steps = value.parse().unwrap_or(0);
            }
            if let Some(value) = part.strip_prefix("checks=") {
                checks = value.parse().unwrap_or(0);
            }
        }
        assert!(
            steps >= 3,
            "{name} 只有 {steps} 个步骤（非夹具场景至少 3 步）：{result}"
        );
        assert!(
            checks >= 3,
            "{name} 只有 {checks} 条断言（非夹具场景至少 3 条）：{result}"
        );
        checked += 1;
    }
    // 跳过是静态判定的（面外引用），全量构建 skipped 恒 0、门槛与从前一致；
    // 2D 构建按实际参与数下限，防"跳过规则吞掉一切"的空洞通过。
    let floor = if skipped == 0 { 13 } else { 9 };
    assert!(
        checked >= floor,
        "参与行为自检的演示太少（{checked}，跳过 {skipped}），门没有实际意义"
    );
}

#[test]
fn sample_project_scripts_all_compile() {
    let root = project_root();
    let mut runtime = crate::host::script_packs::runtime(rustlua_gameplay::input_lua::registry())
        .expect("lua vm")
        .0;
    runtime.set_project_root(root.clone());
    let files = script_files(&root);
    for path in &files {
        let source = fs::read_to_string(path).expect("读取脚本失败");
        let wrapped = format!(
            "local chunk, err = load([==========[\n{}]==========], \"@{name}\")\n\
             if not chunk then error(err, 0) end\n\
             return \"ok\"",
            lua_string_literal(&source),
            // 块名要转义反斜杠：Lua 字符串里的 `\C` 是非法转义序列（Windows 路径必踩）。
            name = path.display().to_string().replace('\\', "/")
        );
        runtime
            .execute_console(&wrapped)
            .unwrap_or_else(|error| panic!("{} 编译失败：{error}", path.display()));
    }
    assert!(files.len() >= 10, "示例工程的脚本太少（{}）", files.len());
}

/// `scripts/api_demo.lua` 的共享取景框（状态行 + 按键提示）所占的屏幕带：贴屏幕底部往上排，
/// 因此最坏情况（多行状态 + 多行提示）会一直占到 y≈600。
///
/// 它们都是 **Lua 动态 UI**。场景实体里的 `Text` 永远画在动态 UI **之上**（`push_scene_ui_text`
/// 排在 `push_ui_text` 之后，受字体图集约束不能前挪），所以"场景文字压住取景框"至今是这类
/// 工程最常见的可见缺陷：脚本照常跑、数值照常写，屏幕上却什么都没有。
///
/// 场景 `Image` 自 2026-10-07 起画在动态 UI **之下**（骨架在下、脚本每帧写的在上），已经盖不住
/// 读数了；这里仍按矩形大面积相交一刀切，是因为要放宽判据就得连"哪些实体只有图没有字"一起建模，
/// 而那会牵动既有场景数据——不在这次范围内，宁可继续拦得住假布局也不放过。
///
/// 判据刻意只抓**大面积**覆盖（铺满屏幕的背景、整条横栏）：小控件（按钮、色块、把手）
/// 贴到状态行附近是合理的布局选择，一刀切会把它们全判红。容器子实体的 rect 在
/// `UiRectLayout::resolve` 里是**相对父矩形**的，因此先按"是否落在视口内"过滤掉。
const OVERLAY_BAND: (f32, f32, f32, f32) = (16.0, 600.0, 1264.0, 710.0);
/// 覆盖宽/高达到这个量级才算"挡住了读数"。
const OVERLAY_COVER_MIN: (f32, f32) = (400.0, 20.0);

/// 场景声明的 UI 实体不得大面积压住共享取景框。
#[test]
fn sample_project_shared_overlay_is_not_covered() {
    let root = project_root();
    let viewport = [1280.0, 720.0];
    let (band_left, band_top, band_right, band_bottom) = OVERLAY_BAND;
    let mut offenders = Vec::new();
    for path in scene_files(&root) {
        let scene = Scene::load(&path).unwrap_or_else(|error| panic!("加载场景失败：{error}"));
        for entity in scene.entities.values() {
            let Some(payload) = entity.components.get("UiLayout") else {
                continue;
            };
            let Ok(layout) = rustlua_ui::ui_layout::UiRectLayout::from_component_str(payload)
            else {
                continue;
            };
            let rect = layout.resolve(viewport);
            // 容器子实体的 rect 是相对父矩形的，落不进视口；这类不参与判定。
            let inside_viewport = rect.x >= -1.0
                && rect.y >= -1.0
                && rect.right() <= viewport[0] + 1.0
                && rect.bottom() <= viewport[1] + 1.0;
            if !inside_viewport {
                continue;
            }
            let overlap_w = rect.right().min(band_right) - rect.x.max(band_left);
            let overlap_h = rect.bottom().min(band_bottom) - rect.y.max(band_top);
            if overlap_w >= OVERLAY_COVER_MIN.0 && overlap_h >= OVERLAY_COVER_MIN.1 {
                offenders.push(format!(
                    "{}: 实体 {} 的 UI 矩形 ({:.0},{:.0})-({:.0},{:.0}) 挡住状态/提示行 {:.0}x{:.0}px",
                    scene_name(&path),
                    entity.name,
                    rect.x,
                    rect.y,
                    rect.right(),
                    rect.bottom(),
                    overlap_w,
                    overlap_h
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "共享取景框被场景 UI 覆盖（脚本的读数会看不见）：\n  - {}\n\
         修法：把铺满屏幕的背景从场景 `Image`/`Text` 换成 `Quad`（世界通道）或脚本里的 \
         `ui.panel`（动态面板在动态文本之下、世界内容之上）。",
        offenders.join("\n  - ")
    );
}

/// 场景里声明的 `Text` / `WorldText` 必须能用工程字体完整画出来。
///
/// 判据用 `ui.measure_text`（与渲染同口径：同一种字体、同一套 advance），
/// `missing_count > 0` 就意味着屏幕上会出现豆腐块/方框——示例工程的文案是给人看的，
/// 缺字等于文案没写。
#[test]
fn sample_project_scene_text_has_every_glyph() {
    let root = project_root();
    let mut runtime = crate::host::script_packs::runtime(rustlua_gameplay::input_lua::registry())
        .expect("lua vm")
        .0;
    runtime.set_project_root(root.clone());
    let mut offenders = Vec::new();
    let mut checked = 0;
    for path in scene_files(&root) {
        let scene = Scene::load(&path).unwrap_or_else(|error| panic!("加载场景失败：{error}"));
        for entity in scene.entities.values() {
            for (component, payload) in &entity.components {
                let text = match component.as_str() {
                    "Text" => rustlua_ui::ui::UiText::from_component_str(payload)
                        .ok()
                        .map(|text| text.text),
                    "WorldText" => rustlua_ui::world_text::WorldText::from_component_str(payload)
                        .ok()
                        .map(|text| text.text.text),
                    _ => None,
                };
                let Some(text) = text else { continue };
                if text.trim().is_empty() || text.contains("]==========]") {
                    continue;
                }
                checked += 1;
                let probe = format!(
                    "(function() local m = ui.measure_text([==========[{text}]==========], 14) \
                     return {{ missing_count = m.missing_count, missing = m.missing }} end)()"
                );
                let value = runtime.eval_json(&probe).unwrap_or_else(|error| {
                    panic!("度量 {}.{} 失败：{error}", scene_name(&path), entity.name)
                });
                let missing = value["missing_count"].as_u64().unwrap_or(0);
                if missing > 0 {
                    offenders.push(format!(
                        "{}: 实体 {} 的 {} 有 {} 个缺字（工程字体没有字形）：{}",
                        scene_name(&path),
                        entity.name,
                        component,
                        missing,
                        value["missing"].as_str().unwrap_or("?")
                    ));
                }
            }
        }
    }
    assert!(checked >= 20, "只检查到 {checked} 段文本，门没有实际意义");
    assert!(
        offenders.is_empty(),
        "场景文案在工程字体下缺字（屏幕上会是方框；改写文案或给 assets/fonts 补字形）：\n  - {}",
        offenders.join("\n  - ")
    );
}

#[test]
fn sample_project_demos_cover_every_lua_api() {
    let root = project_root();
    let runtime = crate::host::script_packs::runtime(rustlua_gameplay::input_lua::registry())
        .expect("lua vm")
        .0;
    let surface = runtime
        .eval_json(LUA_API_PROBE)
        .expect("导出运行时 API 面失败");
    let (sources, raw_sources) = demo_sources(&root);

    let mut total = 0;
    let mut missing = Vec::new();
    for table in surface["tables"].as_array().expect("tables") {
        let name = table["name"].as_str().expect("table name");
        for func in table["funcs"].as_array().expect("funcs") {
            let func = func.as_str().expect("func name");
            total += 1;
            // 直接调用 `ui.panel(...)` 与**把函数本身交给受控调用**
            // （`api.try(camera2d.get, ...)` / `pcall(light.set, ...)`，Lua 里这是惯用的
            // "调用它但容忍宿主帧缺失"写法）都算演示；用局部变量间接持有则不算。
            // `map["goto"](...)` 也必须算：`goto` 是 Lua 5.2+ 的**保留字**，
            // `map.goto(` 语法上根本写不出来，索引写法是唯一能调用它的方式。
            let accepted = [
                format!("{name}.{func}("),
                format!("{name}.{func},"),
                format!("{name}.{func})"),
            ];
            let bracket = format!("{name}[\"{func}\"](");
            if !accepted.iter().any(|form| sources.contains(form))
                && !raw_sources.contains(&bracket)
            {
                missing.push(format!("{name}.{func}"));
            }
        }
    }
    assert!(
        total >= 250,
        "运行时 API 面只有 {total} 个函数，探针可能失效了"
    );
    assert!(
        missing.is_empty(),
        "示例工程缺少 {} 个 API 的演示（共 {total} 个）：{missing:#?}\n\
         补演示的位置：scripts/demo_*.lua（每个模块的演示都要求真的调用一次，注释不算）",
        missing.len()
    );
}

#[test]
fn sample_project_scenes_cover_every_runtime_component() {
    let root = project_root();
    let mut text = String::new();
    for path in scene_files(&root) {
        text.push_str(&fs::read_to_string(&path).expect("读取场景失败"));
        text.push('\n');
    }
    let mut missing = Vec::new();
    for component in ComponentId::ALL {
        let name = component.as_str();
        // 组件行形如 `component\t<实体 id>\t<组件名>\t<载荷>`。
        if !text.contains(&format!("\t{name}\t")) {
            missing.push(name);
        }
    }
    assert!(
        missing.is_empty(),
        "示例工程没有任何场景用到这些运行时组件：{missing:?}"
    );
}

#[test]
fn sample_project_index_menu_lists_every_scene() {
    let root = project_root();
    let registered = registered_scenes(&root);
    let on_disk: BTreeSet<String> = scene_files(&root)
        .iter()
        .map(|path| scene_name(path))
        // 入口菜单自己不是演示：它不进清单（否则菜单里会出现"进入菜单"的卡片）。
        .filter(|name| name != MENU_SCENE)
        .collect();
    assert_eq!(
        registered, on_disk,
        "scripts/demos.lua 与 scenes/ 不一致：清单是入口菜单、路由与测试的共同真源"
    );

    let index = fs::read_to_string(root.join("scenes").join("index.scene")).expect("读取入口场景");
    let entities: BTreeSet<String> = index
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            match (fields.next(), fields.next(), fields.next()) {
                (Some("entity"), Some(_id), Some(name)) => Some(name.to_owned()),
                _ => None,
            }
        })
        .collect();
    for name in &registered {
        assert!(
            entities.contains(name),
            "入口菜单缺少 {name} 的卡片（实体名必须与场景名一致，点击回调直接用它 scene.load）"
        );
    }
    let cards = index.matches("UiEvents").count();
    assert!(
        cards >= registered.len(),
        "入口菜单只有 {cards} 个 UiEvents 卡片，但有 {} 个演示场景",
        registered.len()
    );
}

#[test]
fn sample_project_index_click_opens_the_demo() {
    let root = project_root();
    let registered = registered_scenes(&root);

    // 逐个走一遍"菜单点击 → scene.load → 过渡 → 新场景的演示模块 on_create"。
    // **每轮都重新实例化入口场景**：点击后 `active` 模块会变成目标场景的演示模块，
    // 而"点击菜单卡片"这件事只有入口场景的模块负责。
    for name in &registered {
        if let Some(reason) = out_of_surface_skip(&root, name) {
            println!("{reason}");
            continue;
        }
        let scene = Scene::load(root.join("scenes").join("index.scene")).expect("加载入口场景");
        let mut runtime = RuntimeScene::from_scene(Some(&root), scene).expect("实例化入口场景");
        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect("入口场景步进");
        runtime
            .script
            .execute_console(&format!("on_click('{name}')"))
            .unwrap_or_else(|error| panic!("点击 {name} 的卡片失败：{error}"));
        runtime
            .script
            .process_scene_transition()
            .unwrap_or_else(|error| panic!("切到 {name} 失败：{error}"));
        let current = runtime
            .script
            .execute_console("return tostring(scene.reload())")
            .expect("读当前场景");
        assert_eq!(
            current.trim(),
            name.as_str(),
            "点击 {name} 的卡片后应当切到该场景"
        );
        // 新场景的工程脚本已重新 `on_create`，再步进一帧确认它的 update 能跑。
        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .unwrap_or_else(|error| panic!("{name} 的演示步进失败：{error}"));
    }
}

/// 入口菜单的每张卡片都要能**被鼠标真的点到**：矩形来自 `UiLayout` 的真实解算
/// （`scene_entity_ui_layout_rects` 就是渲染用的那一份），点击走
/// `UiEvents` → 全局 `on_click` → 路由 → `scene.load` 的完整链路。
///
/// 只断言"有 UiEvents 组件"是不够的：卡片的 anchors/offsets 写错时组件仍在，
/// 但点在卡片上什么都不会发生。
#[test]
fn sample_project_index_cards_are_clickable() {
    let root = project_root();
    let registered = registered_scenes(&root);
    let viewport = [1280.0, 720.0];

    for name in &registered {
        if let Some(reason) = out_of_surface_skip(&root, name) {
            println!("{reason}");
            continue;
        }
        let scene = Scene::load(root.join("scenes").join("index.scene")).expect("加载入口场景");
        let card = scene
            .entities
            .values()
            .find(|entity| entity.name == *name)
            .unwrap_or_else(|| panic!("入口场景缺少名为 {name} 的卡片实体"))
            .id;
        let mut runtime = RuntimeScene::from_scene(Some(&root), scene).expect("实例化入口场景");
        let rects = runtime.scene_entity_ui_layout_rects([card], viewport);
        let rect = *rects
            .get(&card.0)
            .unwrap_or_else(|| panic!("{name} 的卡片没有解算出矩形（UiLayout 缺失或非法）"));
        assert!(
            rect.w > 0.0 && rect.h > 0.0,
            "{name} 的卡片矩形是空的：{rect:?}"
        );

        let point = (
            (rect.x + rect.w / 2.0) as f64,
            (rect.y + rect.h / 2.0) as f64,
        );
        // click 在**抬起**时派发（M80-07）：按下与抬起要分两帧喂，
        // 且抬起快照自己带一次"先按下再清边沿"，才有 `just_released`。
        let mut press = InputSnapshot::default();
        press.cursor_moved(point.0, point.1);
        press.mouse_event(MouseButton::Left, true);
        runtime
            .run_script_steps(1, 1.0 / 60.0, &press)
            .unwrap_or_else(|error| panic!("按下 {name} 的卡片失败：{error}"));
        let mut release = InputSnapshot::default();
        release.cursor_moved(point.0, point.1);
        release.mouse_event(MouseButton::Left, true);
        release.finish_frame();
        release.mouse_event(MouseButton::Left, false);
        runtime
            .run_script_steps(1, 1.0 / 60.0, &release)
            .unwrap_or_else(|error| panic!("抬起 {name} 的卡片失败：{error}"));
        runtime
            .script
            .process_scene_transition()
            .unwrap_or_else(|error| panic!("点击 {name} 后切场景失败：{error}"));
        let current = runtime
            .script
            .execute_console("return tostring(scene.reload())")
            .expect("读当前场景");
        assert_eq!(
            current.trim(),
            name.as_str(),
            "在 {name} 卡片中心按左键后应当切到该场景（矩形 {rect:?}）"
        );
    }
}

/// 入口菜单每张卡片的三行文字必须**落在自己那张卡片里**，并且自上而下、左对齐。
///
/// 为什么单独一条门：`UiLayout` 的锚点矩形是**父实体的矩形**（Unity 式），而卡片文字正是
/// 卡片的子实体。偏移一旦按"屏幕坐标"写（2.0 第一版就是这么错的），每张卡的文字会整体偏出
/// 父矩形——场景照样加载、卡片照样点得中、`sample_project_index_cards_are_clickable` 照样绿，
/// 只有眼睛看得出来。这条门把"眼睛看得出来"翻译成断言。
#[test]
fn sample_project_index_labels_are_inside_their_cards() {
    let root = project_root();
    let registered = registered_scenes(&root);
    let scene = Scene::load(root.join("scenes").join("index.scene")).expect("加载入口场景");
    let ids: Vec<_> = scene.entities.keys().copied().collect();
    let by_name: BTreeMap<String, u64> = scene
        .entities
        .values()
        .map(|entity| (entity.name.clone(), entity.id.0))
        .collect();
    let mut runtime = RuntimeScene::from_scene(Some(&root), scene).expect("实例化入口场景");
    let rects = runtime.scene_entity_ui_layout_rects(ids, [1280.0, 720.0]);

    let mut checked = 0;
    for name in &registered {
        let card = *by_name
            .get(name.as_str())
            .unwrap_or_else(|| panic!("入口菜单没有名为 {name} 的卡片实体"));
        let card_rect = *rects
            .get(&card)
            .unwrap_or_else(|| panic!("{name} 的卡片没有解算出矩形"));
        assert!(
            card_rect.x >= 0.0
                && card_rect.y >= 0.0
                && card_rect.x + card_rect.w <= 1280.0
                && card_rect.y + card_rect.h <= 720.0,
            "{name} 的卡片跑出视口：{card_rect:?}"
        );

        let mut previous_y = f32::MIN;
        let mut left_edge = None;
        for suffix in ["_title", "_detail", "_meta"] {
            let key = format!("{name}{suffix}");
            let id = *by_name
                .get(key.as_str())
                .unwrap_or_else(|| panic!("入口菜单缺少 {key}"));
            let rect = *rects
                .get(&id)
                .unwrap_or_else(|| panic!("{key} 没有解算出矩形（UiLayout 缺失或非法）"));
            // 点锚（w=h=0）：矩形就是文字左上角落在屏幕上的那个点。留 24px 右边距
            // 是因为文字从这里往右长，锚点贴到卡片右缘就意味着文字必然出界。
            assert!(
                rect.x > card_rect.x
                    && rect.x + 24.0 < card_rect.x + card_rect.w
                    && rect.y >= card_rect.y
                    && rect.y + 8.0 < card_rect.y + card_rect.h,
                "{key} 的文字锚点偏出卡片：锚点=({}, {}) 卡片={card_rect:?}",
                rect.x,
                rect.y
            );
            assert!(
                rect.y > previous_y,
                "{key} 不在上一行之下（三行必须自上而下）：锚点 y={} 上一行 y={previous_y}",
                rect.y
            );
            previous_y = rect.y;
            match left_edge {
                None => left_edge = Some(rect.x),
                Some(x) => assert!(
                    (x - rect.x).abs() < 0.5,
                    "{key} 没有和同卡其它文字左对齐：x={} 应为 {x}",
                    rect.x
                ),
            }
        }
        checked += 1;
    }
    assert!(
        checked >= 13,
        "只核对了 {checked} 张卡片的文字，入口菜单结构可能被改坏"
    );
}

/// 卡片三行的**实测宽度**必须留在卡片里（右缘至少留 8 px）。
///
/// 上一条门只看锚点，看不见文字往右长多远：字号调大或文案调长时锚点门照样全绿，
/// 屏幕上却是最后一行顶出卡片右缘。判据用 `ui.measure_text`——与渲染同一套 advance，
/// 不是"字数 × 系数"的估算。
#[test]
fn sample_project_index_labels_fit_their_cards() {
    let root = project_root();
    let registered = registered_scenes(&root);
    let scene = Scene::load(root.join("scenes").join("index.scene")).expect("加载入口场景");
    let ids: Vec<_> = scene.entities.keys().copied().collect();
    let mut by_name: BTreeMap<String, u64> = BTreeMap::new();
    let mut label_of: BTreeMap<String, (String, f32)> = BTreeMap::new();
    for entity in scene.entities.values() {
        by_name.insert(entity.name.clone(), entity.id.0);
        for (component, payload) in &entity.components {
            if component != "Text" {
                continue;
            }
            if let Ok(text) = rustlua_ui::ui::UiText::from_component_str(payload) {
                label_of.insert(entity.name.clone(), (text.text, text.font_size));
            }
        }
    }
    let mut runtime = RuntimeScene::from_scene(Some(&root), scene).expect("实例化入口场景");
    let rects = runtime.scene_entity_ui_layout_rects(ids, [1280.0, 720.0]);
    let mut probe = crate::host::script_packs::runtime(rustlua_gameplay::input_lua::registry())
        .expect("lua vm")
        .0;
    probe.set_project_root(root.clone());

    let mut checked = 0;
    for name in &registered {
        let card_id = *by_name
            .get(name.as_str())
            .unwrap_or_else(|| panic!("入口菜单没有名为 {name} 的卡片实体"));
        let card = *rects
            .get(&card_id)
            .unwrap_or_else(|| panic!("{name} 的卡片没有解算出矩形"));
        for suffix in ["_title", "_detail", "_meta"] {
            let key = format!("{name}{suffix}");
            let label_id = *by_name
                .get(key.as_str())
                .unwrap_or_else(|| panic!("入口菜单缺少 {key}"));
            let anchor = *rects
                .get(&label_id)
                .unwrap_or_else(|| panic!("{key} 没有解算出矩形"));
            let (text, font_size) = label_of
                .get(key.as_str())
                .unwrap_or_else(|| panic!("{key} 没有 Text 组件"))
                .clone();
            assert!(
                !text.contains("]==========]"),
                "{key} 的文案里有长括号终止串，门的取值会失真"
            );
            let script = format!(
                "(function() return ui.measure_text([==========[{text}]==========], {font_size}) end)()"
            );
            let value = probe
                .eval_json(&script)
                .unwrap_or_else(|error| panic!("度量 {key} 的 {text:?} 失败：{error}"));
            let width = value["width"].as_f64().unwrap_or(0.0) as f32;
            let used = anchor.x - card.x + width;
            assert!(
                used <= card.w - 8.0,
                "{key} 的文字顶出卡片：文案 {text:?} 在 {font_size}px 下实测 {width:.1}px，\
                 从卡片左缘 {inset:.1}px 起共占 {used:.1}px，卡片宽 {}px（右缘只剩 {:.1}px）",
                card.w,
                used - card.w,
                inset = anchor.x - card.x,
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 39,
        "只量了 {checked} 行卡片文字（15 张卡 × 3 行 = 45），门没有实际意义"
    );
}

/// 入口菜单按 `A` 打开能力图谱时，**菜单自己的场景文字必须全部让位**。
///
/// 场景 `Text` 永远画在动态 UI 之上（`push_scene_ui_text` 排在 `push_ui_text` 之后），
/// 所以 A 覆盖层只能盖住场景 `Image`，盖不住卡片三行文字与页眉/页脚；不显式隐藏就会
/// 出现"两层菜单文字叠在一起"（2026-11-17 实机踩到）。这条门把"打开时藏、收起时恢复"
/// 变成可回归的断言。
#[test]
fn sample_project_index_atlas_hides_menu_layers() {
    let root = project_root();
    let scene = Scene::load(root.join("scenes").join("index.scene")).expect("加载入口场景");
    let mut runtime = RuntimeScene::from_scene(Some(&root), scene).expect("实例化入口场景");
    runtime
        .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
        .expect("入口场景首帧");

    let mut press = InputSnapshot::default();
    press.key_event(KeyCode::KeyA, true);
    runtime
        .run_script_steps(1, 1.0 / 60.0, &press)
        .expect("按下 A 打开能力图谱");
    let atlas = runtime
        .script
        .eval_json("return ui.panel_get('menu_atlas') ~= nil")
        .expect("读取能力图谱面板");
    assert_eq!(
        atlas.as_bool(),
        Some(true),
        "按下 A 后应画出 menu_atlas 面板"
    );
    let hidden = runtime.scene_text_hidden();
    for name in ["Title", "Subtitle", "Footer"] {
        assert!(
            hidden.contains(name),
            "能力图谱打开时场景文字 {name} 必须隐藏，否则会叠在覆盖层上（实际隐藏集：{hidden:?}）"
        );
    }
    for card in registered_scenes(&root) {
        for suffix in ["_title", "_detail", "_meta"] {
            let name = format!("{card}{suffix}");
            assert!(
                hidden.contains(&name),
                "能力图谱打开时卡片文字 {name} 必须隐藏（实际隐藏集：{hidden:?}）"
            );
        }
    }

    // 松开 A 后再按一次：覆盖层关闭，场景文字全部恢复。
    let mut release = InputSnapshot::default();
    release.key_event(KeyCode::KeyA, true);
    release.finish_frame();
    release.key_event(KeyCode::KeyA, false);
    runtime
        .run_script_steps(1, 1.0 / 60.0, &release)
        .expect("松开 A");
    let mut close = InputSnapshot::default();
    close.key_event(KeyCode::KeyA, true);
    runtime
        .run_script_steps(1, 1.0 / 60.0, &close)
        .expect("再按 A 收起能力图谱");
    let atlas = runtime
        .script
        .eval_json("return ui.panel_get('menu_atlas') ~= nil")
        .expect("读取能力图谱面板");
    assert_eq!(
        atlas.as_bool(),
        Some(false),
        "再按一次 A 应收起 menu_atlas 面板"
    );
    let hidden = runtime.scene_text_hidden();
    assert!(
        hidden.is_empty(),
        "收起能力图谱后场景文字应全部恢复，仍在隐藏集里的：{hidden:?}"
    );
}
