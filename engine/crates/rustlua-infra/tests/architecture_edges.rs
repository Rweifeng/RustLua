//! 架构耦合门：把"核心 crate 依赖了谁"从口头约定变成会失败的测试。
//!
//! ## 为什么需要它
//!
//! `docs/62` 的 P1 记录了两条聚合层：`rustlua-scripting` 曾经正向依赖全部 13 个兄弟 crate，
//! `rustlua-scene` 依赖 9 个（含 render/ui/physics）。这类耦合没有"坏掉"的瞬间——每加一条
//! 依赖的那次提交都是合理的，代价要等改动半径变大之后才被人感觉到。所以它必须**有预算**：
//! 新增一条边就得同时改这张表，改表的人会被迫读一遍下面的注释。
//!
//! ## 这张表是什么
//!
//! 每个 crate 允许的**库依赖**（`[dependencies]` + `[target.*.dependencies]`）。
//! `[dev-dependencies]` 不参与：测试用谁不改变产物的依赖图，也不影响增量编译的半径。
//!
//! 门的失败信息是给人看的：它要说清"多了哪条边、为什么不允许、该走哪条路"。
//!
//! ## 改这张表之前
//!
//! - 新增**领域 → 领域**的边：先问"是不是该依赖倒置"。绑定层要用别的领域的状态，
//!   走 `rustlua-lua-api` 的 `Handle`/`Registry`（见该 crate 的文档），不要直接 `use`。
//! - `rustlua-lua-api` 是叶子：它只允许 mlua。一旦它开始依赖领域 crate，所有引用它的
//!   crate 都会被拖进同一次重编，倒置就白做了。
//! - 组合根（`engine` / `game` / `game_web` / `editor`）的边数是**允许大**的——
//!   它们的职责就是把东西接起来；但它们不许被核心 crate 反向依赖。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// 核心 crate 的依赖预算。键是 crate 名，值是**允许**的库依赖（去掉 `rustlua-` 前缀）。
///
/// 目标（`docs/62` P1 依赖倒置）：`rustlua-scripting` 的条数要一路降到"只剩 VM 宿主真正
/// 需要的"（ecs/math/infra/render/scene 的宿主快照），绑定逐个下沉到所属 crate。
const BUDGET: &[(&str, &[&str])] = &[
    // ── 叶子：只允许纯基础（math/infra）与框架依赖 ───────────────────────────
    ("rustlua-math", &[]),
    // P1 新增：Lua 绑定的契约层。**只许 mlua**：它是倒置的支点，一旦它依赖领域 crate，
    // 所有领域 crate 都会被拖进同一张编译图。
    ("rustlua-lua-api", &[]),
    ("rustlua-infra", &[]),
    // P3-E04-1：编辑器插件 API。纯叶子（std only），编辑器与未来插件共用；
    // 它不认识 editor/engine，反向依赖一出现就会在这里失败。
    ("rustlua-editor-api", &[]),
    // P1 ⑤-10：`Transform`/`Entity`/`Tilemap` 的 Lua 面跟着类型进 ECS，因此多认 `lua-api`。
    ("rustlua-ecs", &["infra", "lua-api", "math"]),
    // `lua` 模块：audio 的 Lua 绑定下沉到这里（契约层只依赖 mlua，不成环）。
    ("rustlua-audio", &["infra", "lua-api"]),
    // `input` 的 Lua 绑定也下沉到本 crate（P1 阶段 3）：`mlua` 只用于装表，契约层提供
    // 能力单元与注册表；设置文件那一半经端口由 `rustlua-gameplay` 注入。
    ("rustlua-input", &["lua-api"]),
    // ── 领域 ────────────────────────────────────────────────────────────────
    // `rustlua-assets` 只剩**运行时那一半**（P1 阶段 ③，2026-10-20）：资源库 / 句柄 /
    // 依赖图 / `rltex` / `atlas_format` / `gltf_asset` / 导入契约（trait + 注册表）。
    // 导入层的实现搬去了下面的 `rustlua-assets-import`，`image`/`rodio`/`ab_glyph`/`mlua`
    // 随之离开运行时编译图。
    ("rustlua-assets", &["ecs", "infra", "math"]),
    // **导入层**：八个内置导入器 + 图集规划/装箱 + 导入缓存。只在 `assetc`/`editor` 里编译，
    // `engine`/`game_web` 不许依赖它——那条反向边会把 PNG/JPEG 编解码与整套音频解码器
    // 重新拖进浏览器产物，正是这次拆分要拆掉的东西。
    // `scene`：装配完整的导入注册表要用它的 Scene/Prefab 导入器零件（`scene_importers()`）；
    // 方向是 `assets-import → scene`，不是反过来。
    // `math`：P3-R03-1 的 equirect→cubemap 投影与 SH9 预滤波要用 `math3d::Vec3` 做球面
    // 方向运算（`environment.rs`）。`math` 是叶子 crate（谁都能依赖、它谁也不依赖），
    // 运行时侧的 `rustlua-assets`/`env_asset` 早就依赖同一个 `Vec3`，所以这条边既不开
    // 新方向，也不把任何重型依赖带进导入层。
    // `animation`：P3-E01-4b 的 `.anim` 导入器用运行时同一台解析器（`AnimClip::from_text`）。
    // `scene` 已经依赖 animation，因此这条边同样不开新方向、不新增传递 crate。
    (
        "rustlua-assets-import",
        &["animation", "assets", "infra", "math", "scene", "ui"],
    ),
    // P1 ⑤-8：相机绑定（`camera`/`camera3d`/`camera2d`）与拾取共享链都住进相机域，因此多认
    // 一个 `lua-api`（叶子契约层：`Handle`/`api_error`，谁都能依赖它）。
    ("rustlua-camera", &["ecs", "lua-api", "math"]),
    // P1 ⑤-9：物理绑定（`physics`/`physics3d`）与回写队列搬进物理域，同样多认 `lua-api`。
    ("rustlua-physics", &["ecs", "lua-api", "math"]),
    // P1 ④：表现层组件的**载荷契约**（`ComponentCodec` + 注册表 + 装表入口）。和
    // `lua-api` 同一个形状的叶子：谁都能依赖它，它谁也不依赖。`render`/`ui` 把各自的
    // `from_component_str` 包成 codec 交出来，`scene` 只查表——于是那条
    // `scene → render/ui` 的边（以及随之而来的"改 UI 重编全世界"）就消失了。
    ("rustlua-component-codec", &[]),
    // `lua-api`：`lua_nav` 的导航绑定下沉到 UI 域（P1 ⑤-3）。契约层是叶子，不违反倒置。
    ("rustlua-ui", &["component-codec", "ecs", "lua-api", "math"]),
    // P3-E03：`.vgraph` 图格式 + 校验 + 确定性 Lua 生成。纯叶子（serde/serde_json/thiserror），
    // 运行时与编辑器都依赖它，方向只能是"别人 → visual-script"，不能反向拉进场景/渲染。
    ("rustlua-visual-script", &[]),
    // P1 ⑤-10：动画组件的 Lua 面进 animation 域。
    ("rustlua-animation", &["assets", "ecs", "lua-api", "math"]),
    // M77-01：wgpu 渲染器从组合根 `engine/src/host/renderer/` 下沉成独立 crate。
    // 它的预算就是"渲染数据 + 相机/ECS 读数"那几个核心 crate：gpu 不许反向依赖 `engine`
    // （normal 面），要复用运行时装配的宿主自己把 `engine` 接上（宿主本来就是组合根）。
    // 帧结构（`frame`/`fit`）在 M77-01 里进了 `rustlua-render`，所以这里不需要 `engine`
    // 那一份共享 ABI。
    (
        "rustlua-gpu",
        &["assets", "camera", "ecs", "infra", "math", "render"],
    ),
    (
        "rustlua-render",
        &[
            "animation",
            "assets",
            "camera",
            "component-codec",
            "ecs",
            "infra",
            "lua-api",
            "math",
        ],
    ),
    // `audio_lua`：audio 绑定的组合根接线（设置端口 + 注册表）。端口要用 `infra::vfs` 读
    // `settings.toml`（wasm 无文件系统），注册表与能力单元来自 `lua-api`。
    (
        "rustlua-gameplay",
        &["audio", "ecs", "infra", "input", "lua-api"],
    ),
    // ── 聚合层：预算只减不增 ────────────────────────────────────────────────
    // `rustlua-scripting`：P1 已削掉 assets（`AssetId` 下沉到 infra::ids）、audio 与 input
    // （绑定下沉到 `rustlua-audio` / `rustlua-input`，接线在 `rustlua_gameplay::{audio_lua,input_lua}`）。
    // 剩余 11 条领域依赖 + `lua-api` 契约层里，animation/camera/gameplay/physics/ui 的绑定
    // 仍是"借 `Rc<RefCell<ScriptState>>` 这个大中枢"的老形状，下沉路线见 docs/62。
    // `visual-script`（P3-E03-3）：运行时在加载实体脚本时把 `.vgraph` 编译成同一套 Lua；
    // 它是纯叶子（不依赖任何领域 crate），不会扩大这条聚合边的传递依赖面。
    (
        "rustlua-scripting",
        &[
            "animation",
            "camera",
            "ecs",
            "gameplay",
            "infra",
            "lua-api",
            "math",
            "physics",
            "render",
            "scene",
            "ui",
            "visual-script",
        ],
    ),
    // `rustlua-scene`：**P1 ④ 已收口**（2026-10-20）——表现层组件按注册表处理：
    // `TypedScene` 对 `Sprite`/`Text`/`WorldText`/… 只存 `Opaque { payload, assets }`，
    // 文法在 `render`/`ui` 的 `scene_codec` 里，本 crate 用 `component-codec` 查表校验。
    //
    // `render`/`ui` 已从这张表里**删掉**，它们退成 dev-dependencies（只为测试装表）。
    // 让这两条边永久消失的三次搬家：`WorldText` → `rustlua-ui`、`char_collect` →
    // `rustlua-assets-import`、`scene_extract` → `engine`。
    (
        "rustlua-scene",
        &[
            "animation",
            "assets",
            "camera",
            "component-codec",
            "ecs",
            "infra",
            "math",
            "physics",
        ],
    ),
    // M79 编辑器 crate 拆分：`editor` 的无界面领域层。它是库不是组合根，因此：
    // ① 不许依赖 `engine` / `editor`（引擎运行时桥与 GUI shell 都留在上层）；
    // ② 不许依赖 `gpu` / `wgpu`——文件监听只读 `infra::paths::SHADER_SOURCE_DIR` 这个常量。
    // `assets-import` 是资源面板状态与图集预览要用的导入层，运行时 crate 不许反向依赖它。
    (
        "rustlua-editor-core",
        &[
            "animation",
            "assets",
            "assets-import",
            "camera",
            "ecs",
            "infra",
            "math",
            "physics",
            "render",
            "scene",
            "ui",
            "visual-script",
        ],
    ),
    // UI 文档宿主与 UI Studio 无头层：零 egui、零 `EditorApp`，只认契约层与场景/UI 数据。
    (
        "rustlua-editor-ui-studio",
        &["assets-import", "scene", "ui", "ui-spec"],
    ),
];

/// 组合根：允许依赖全部核心 crate（它们的职责就是接线），但**不得被核心 crate 反向依赖**。
///
/// `assetc` **不在这里**（2026-10-20 移出）：它是资源构建的 **CLI 工具**，不是"把库接起来的
/// 应用"。它此前挂了一条 `engine` 依赖（全 crate 0 次使用），于是导入工具被迫编整条运行时
/// ——"改编辑器 → 重编 assetc"。移出后它受同一条反向边约束：工具不许把运行时当库用。
/// 它仍然可以依赖 `rustlua-assets` / `rustlua-render` / `rustlua-scene`（导入要用的解码与
/// 场景编解码），这些都是核心 crate，不是组合根。
///
/// `gpu-web-probe` 在这里（同日补上）：它是 `[lib] crate-type = ["cdylib"]` 的 wasm 探针
/// 产物，**刻意**驱动真渲染器而不是另写玩具渲染器，所以依赖 `engine` 是对的；它同时必须
/// 保持"没有任何 crate 依赖我"（扫描器现在看得见这条边了）。
///
/// `rustlua-gpu` **不在这里**（M77-01）：它是库，不是"把东西接起来的应用"——它自己就是
/// 被接的那件东西。它对宿主暴露的是具体渲染器类型，而宿主之间不互相依赖，所以它受
/// "核心 crate 不许反向依赖组合根"这条约束，也受 [`composition_roots_do_not_reach_the_gpu_stack`]
/// 那条更窄的门约束（engine 的 normal 图不许碰 wgpu）。
const COMPOSITION_ROOTS: &[&str] = &["engine", "game", "game_web", "editor", "gpu-web-probe"];

fn workspace_crates() -> PathBuf {
    // crates/rustlua-infra → crates → engine
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .expect("CARGO_MANIFEST_DIR 必须在 <workspace>/crates/<crate> 下")
        .to_path_buf()
}

/// 极简 `Cargo.toml` 扫描器：只认"依赖段里的 `key = ...`"。
///
/// 为什么不用 `toml` crate：这条门要能在**任何** crate 的测试里跑，而新增一个解析依赖
/// 只为读几行清单不值当；这些清单是 cargo 生成的、格式稳定（没有多行表、没有内联注释里
/// 的假 key）。认不出的写法（`[dependencies.foo]` 子表）按依赖处理——**宁可多报**，
/// 漏报会让门变成装饰。
fn lib_dependencies(manifest_path: &Path) -> BTreeSet<String> {
    let text = std::fs::read_to_string(manifest_path)
        .unwrap_or_else(|error| panic!("读不了 {}：{error}", manifest_path.display()));
    let mut deps = BTreeSet::new();
    let mut in_deps = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            let header = line.trim_start_matches('[').trim_end_matches(']');
            // `[dependencies]` / `[target.'cfg(...)'.dependencies]` 算库依赖；
            // `[dev-dependencies]` / `[build-dependencies]` 不算。
            in_deps = header == "dependencies"
                || (header.ends_with(".dependencies") && header.starts_with("target."));
            // `[dependencies.rustlua-x]` 这种子表写法：直接把名字记上。
            if let Some(name) = header.strip_prefix("dependencies.") {
                deps.insert(name.trim_matches('"').to_owned());
            }
            continue;
        }
        if !in_deps {
            continue;
        }
        let Some((key, _)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || key.starts_with('"') {
            continue;
        }
        // 依赖段里带点的键是"给某个依赖设一个字段"：`engine.workspace = true` 就是
        // `engine = { workspace = true }`，点之前那段才是 crate 名。
        //
        // 旧版把带点键整行跳过（当时的假设是"点号键只出现在内联表的字段上"），可工作区里
        // `engine`/`editor`/`game`/`game_web` 恰恰都是用 `engine.workspace = true` 引进来的
        // —— 于是**组合根一条边都扫不到**，那条"核心 crate 不许反向依赖组合根"的测试
        // 一直是恒绿的装饰（2026-10-20 修）。
        let name = key.split('.').next().unwrap_or(key).trim();
        if name.is_empty() {
            continue;
        }
        deps.insert(name.to_owned());
    }
    deps
}

/// 把 `rustlua-foo` 归一成 `foo`；外部依赖（serde 等）原样返回。
fn short_name(name: &str) -> &str {
    name.strip_prefix("rustlua-").unwrap_or(name)
}

fn graph() -> BTreeMap<String, BTreeSet<String>> {
    let crates_dir = workspace_crates().join("crates");
    let mut graph = BTreeMap::new();
    let mut manifests = Vec::new();
    let entries = std::fs::read_dir(&crates_dir)
        .unwrap_or_else(|error| panic!("读不了 {}：{error}", crates_dir.display()));
    for entry in entries {
        let path = entry.expect("crates/ 下的目录项").path();
        let manifest = path.join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .expect("目录名")
            .to_string_lossy()
            .into_owned();
        manifests.push((short_name(&name).to_owned(), manifest));
    }
    // **workspace 内部**的伙伴集合：目录名 = 包名（`rustlua-scene`、`engine`、`assetc`…）。
    // 早先这里只认 `rustlua-` 前缀的依赖，于是 `engine`/`editor`/`game`/`assetc` 这些
    // **组合根**在边上完全不可见 —— 组合根那条检查（"核心 crate 不许反向依赖组合根"）
    // 因此恒绿，`assetc → engine` 这种死边挂了很久没人看见（2026-10-20 修）。
    let partners: BTreeSet<String> = manifests.iter().map(|(name, _)| name.clone()).collect();
    for (crate_name, manifest) in manifests {
        let deps: BTreeSet<String> = lib_dependencies(&manifest)
            .into_iter()
            .map(|dep| short_name(&dep).to_owned())
            .filter(|dep| partners.contains(dep))
            .collect();
        graph.insert(crate_name, deps);
    }
    assert!(
        graph.contains_key("scripting"),
        "没扫到 rustlua-scripting，扫描器失效了（门不能静默变绿）"
    );
    // 同样的道理：只认 `rustlua-` 前缀就等于对组合根闭眼。这里钉住"至少看见几条
    // 通向组合根的边"，否则扫描器退化成装饰时整条测试会安静地变绿。
    let root_edges: usize = graph
        .values()
        .map(|deps| {
            deps.iter()
                .filter(|dep| COMPOSITION_ROOTS.contains(&dep.as_str()))
                .count()
        })
        .sum();
    assert!(
        root_edges >= 3,
        "只扫到 {root_edges} 条指向组合根的边（engine/editor/game/game_web 至少各被一个应用依赖）；\
         扫描器多半又只认 `rustlua-` 前缀了"
    );
    graph
}

#[test]
fn core_crates_stay_within_their_dependency_budget() {
    let graph = graph();
    let mut problems = Vec::new();
    for (crate_name, allowed) in BUDGET {
        let short = short_name(crate_name);
        let Some(actual) = graph.get(short) else {
            problems.push(format!("预算表里的 {crate_name} 在 crates/ 下不存在"));
            continue;
        };
        let allowed: BTreeSet<&str> = allowed.iter().copied().collect();
        for dep in actual {
            if !allowed.contains(dep.as_str()) {
                problems.push(format!(
                    "{crate_name} 新增了依赖 rustlua-{dep}：核心 crate 的耦合预算已满。\n     \
                     先判断能不能依赖倒置（rustlua-lua-api 的 Handle/Registry，见 docs/62 P1）；\n     \
                     确实要加，就在 crates/rustlua-infra/tests/architecture_edges.rs 的 BUDGET 里\n     \
                     写清理由再放开。"
                ));
            }
        }
        // 预算也不能虚高：表里列了但实际没有的边，说明表已经烂了，会掩盖真新增。
        for dep in &allowed {
            if !actual.contains(*dep) {
                problems.push(format!(
                    "{crate_name} 的预算里列了 rustlua-{dep}，但它已经不再是依赖——\
                     请把这条从 BUDGET 删掉（否则预算虚高，新增边会被它盖住）"
                ));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "架构耦合门未通过（{} 条）：\n  - {}",
        problems.len(),
        problems.join("\n  - ")
    );
}

#[test]
fn no_dependency_cycles() {
    let graph = graph();
    // 迭代 DFS：核心 crate 只有二十来个，递归也不会爆栈，但显式栈更好读错误信息。
    let mut visiting: Vec<&str> = Vec::new();
    let mut done: BTreeSet<&str> = BTreeSet::new();
    let mut stack: Vec<(&str, Vec<&str>)> = Vec::new();
    for start in graph.keys() {
        if done.contains(start.as_str()) {
            continue;
        }
        stack.push((
            start.as_str(),
            graph[start].iter().map(String::as_str).collect(),
        ));
        visiting.push(start.as_str());
        while let Some((_, children)) = stack.last_mut() {
            match children.pop() {
                Some(child) => {
                    if visiting.contains(&child) {
                        let mut cycle: Vec<&str> = visiting
                            .iter()
                            .copied()
                            .skip_while(|item| *item != child)
                            .collect();
                        cycle.push(child);
                        panic!("依赖成环：{}", cycle.join(" → "));
                    }
                    if done.contains(child) {
                        continue;
                    }
                    visiting.push(child);
                    let next: Vec<&str> = graph
                        .get(child)
                        .map(|deps| deps.iter().map(String::as_str).collect())
                        .unwrap_or_default();
                    stack.push((child, next));
                }
                None => {
                    let (finished, _) = stack.pop().expect("非空");
                    visiting.retain(|item| *item != finished);
                    done.insert(finished);
                }
            }
        }
    }
}

#[test]
fn core_crates_are_not_depended_on_by_their_peers_in_the_wrong_direction() {
    let graph = graph();
    // 组合根不许被核心 crate 依赖：那会让"主机"变成"库"，`game` 依赖 editor 之类的边
    // 肉眼看不出来，但会让产物多编一整个应用。
    let mut problems = Vec::new();
    for (crate_name, deps) in &graph {
        if COMPOSITION_ROOTS.contains(&crate_name.as_str()) {
            continue;
        }
        for root in COMPOSITION_ROOTS {
            if deps.contains(*root) {
                problems.push(format!("核心 crate {crate_name} 反向依赖了组合根 {root}"));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn lua_contract_crate_stays_a_leaf() {
    let graph = graph();
    let deps = graph
        .get("lua-api")
        .expect("rustlua-lua-api 必须在 workspace 里");
    assert!(
        deps.is_empty(),
        "rustlua-lua-api 必须保持叶子（只依赖 mlua），否则依赖倒置的支点就没了；\
         现在它依赖了：{deps:?}"
    );
}

/// 第二个倒置支点（`docs/62` P1 ④）：`rustlua-component-codec` 也必须是叶子。
///
/// 它和 `lua-api` 是同一个形状、同一条理由：`render`/`ui` 要能把"组件文法"交出去，
/// `scene` 要能查表，三者共同的落点就必须谁都不依赖。一旦它悄悄依赖了
/// `render`/`ui`/`scene` 中任何一个，那张编译图立刻闭合成环，这次拆分就白做了。
///
/// `BUDGET` 里的空预算已经能抓"新增依赖"，这里再写一遍是因为**这条边值得一条专属断言**：
/// 它失败时的信息要直接说清"倒置支点没了"，而不是让人去读预算表。
#[test]
fn component_codec_crate_stays_a_leaf() {
    let graph = graph();
    let deps = graph
        .get("component-codec")
        .expect("rustlua-component-codec 必须在 workspace 里");
    assert!(
        deps.is_empty(),
        "rustlua-component-codec 必须保持叶子（只依赖 thiserror），否则 scene/render/ui \
         的倒置支点就没了；现在它依赖了：{deps:?}"
    );
}

/// GPU 栈的成员名（外部 crate，不在 `partners` 里，因此只出现在清单扫描那一步）。
///
/// 为什么是这两个：`wgpu` 带着整套原生后端（vulkan/metal/gl，`cargo tree` 里那一坨
/// `ash`/`metal`/`hal`）与浏览器侧的 `web-sys` 面，`naga` 带着 WGSL 编译器——M77-01 之后
/// 它们只许住在 `rustlua-gpu`。**不**列 `pollster`：它只是一个 `block_on`，不拉任何设备
/// 代码，把它算进这条门只会让读数变吵，抓不到真回归。
const GPU_STACK: &[&str] = &["wgpu", "naga"];

/// `crates/` 下每个 crate 的清单路径，键和 [`graph()`] 用同一套短名。
fn crate_manifests() -> BTreeMap<String, PathBuf> {
    let crates_dir = workspace_crates().join("crates");
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(&crates_dir)
        .unwrap_or_else(|error| panic!("读不了 {}：{error}", crates_dir.display()))
    {
        let path = entry.expect("crates/ 下的目录项").path();
        let manifest = path.join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .expect("目录名")
            .to_string_lossy()
            .into_owned();
        out.insert(short_name(&name).to_owned(), manifest);
    }
    out
}

/// M77-01 的验收门：**组合根 `engine` 的 normal 依赖图里不许出现 wgpu/naga**。
///
/// 这条门是那次搬迁的全部意义所在。搬迁之前，渲染器住在 `engine/src/host/renderer/`，
/// 于是"engine 是唯一持有 wgpu 实现的地方"——任何宿主想要渲染器就得把整个组合根（连带
/// 脚本、物理、音频、场景装配）拖进编译图。搬迁之后 `engine` 只做装配，GPU 那半边在
/// `rustlua-gpu`：改 PBR/粒子不再重编 engine 的下游，`gpu-web-probe` 也不再为拿渲染器
/// 而依赖 engine。
///
/// 查法是**传递闭包**而不是只看 `engine/Cargo.toml` 那一行：往 engine 的 normal 依赖里加
/// `rustlua-gpu` 同样会把 wgpu 拖回来，而那正是这条门要拦的形状（BUDGET 抓不到它——
/// `gpu` 是合法的核心 crate，只是它带着 wgpu）。
///
/// `[dev-dependencies]` 不算（`lib_dependencies` 的口径）：engine 的 23 个 `wp0_*_golden.rs`
/// 基准例子仍然自己建 adapter/device/离屏纹理，那段 GPU 管子等 M77-02 的"两套 offscreen
/// renderer 合并"收进 `rustlua-gpu` 之后才会从 engine 消失；dev 依赖不进任何下游 crate 的
/// 编译面，因此它既不改变产物依赖图，也不影响增量编译半径——本的门保护的就是这两件事。
#[test]
fn composition_roots_do_not_reach_the_gpu_stack() {
    let graph = graph();
    let manifests = crate_manifests();
    let mut offenders: Vec<String> = Vec::new();
    // `reach` 既是已访问集合，也记"engine 经哪条 normal 边走到这里"。
    let mut reach: BTreeMap<&str, String> = BTreeMap::new();
    reach.insert("engine", "engine（组合根本身）".to_owned());
    let mut queue = std::collections::VecDeque::from(["engine"]);
    while let Some(crate_name) = queue.pop_front() {
        let here = reach[crate_name].clone();
        if let Some(manifest) = manifests.get(crate_name) {
            for dep in lib_dependencies(manifest) {
                if GPU_STACK.contains(&short_name(&dep)) {
                    offenders.push(format!("{here}：normal 依赖里有 {dep}"));
                }
            }
        }
        for next in graph.get(crate_name).into_iter().flatten() {
            if !reach.contains_key(next.as_str()) {
                reach.insert(next.as_str(), format!("{here} → {next}"));
                queue.push_back(next.as_str());
            }
        }
    }
    assert!(
        reach.contains_key("scripting") && reach.contains_key("render"),
        "没从 engine 走到 rustlua-scripting/rustlua-render，扫描器失效了（门不能静默变绿）"
    );
    assert!(
        offenders.is_empty(),
        "M77-01：engine 的 normal 依赖图不许出现 GPU 栈（wgpu/naga），现在命中 {n} 条：\n  - {list}\n\
         渲染器实现住在 crates/rustlua-gpu；宿主想用 GPU 就**直接**依赖它（game/editor/game_web/gpu-web-probe \
         都是这么接的），不许经 engine 转发，也不许把 rustlua-gpu 加回 engine 的 [dependencies]。\n\
         复现读数：cargo tree -p engine -e normal -i wgpu（交付后应零命中）",
        n = offenders.len(),
        list = offenders.join("\n  - ")
    );
}
