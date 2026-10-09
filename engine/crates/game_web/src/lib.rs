//! 浏览器（WebGL / 微信小游戏）wasm 入口与宿主 API（WP10）。
//!
//! Lua 桥接线（`run_lua_once`）：把 Web 宿主拿到的项目资源（.lua 脚本 /
//! require 模块 / data JSON）注入 `engine::core::vfs` 的资源提供者，再启动
//! `engine::core::script::LuaRuntime` 执行项目脚本并跑一帧 `update()`。
//! 平台无关的核心逻辑（资源表注入 + Lua 执行）可在桌面直接跑测试。

/// WP10 批次 A：`app / clock / calendar / day` 宿主 API 的 Rust 实现（编译进 wasm）。
#[cfg(target_arch = "wasm32")]
pub mod host_api;

/// WP10 迁移：Lua 执行层从 fengari 迁移到 mlua-wasm 的 wasm Lua 桥
/// （`lua_boot` / `lua_do_string` / `lua_call_global` / `lua_register_js_callback` /
/// `lua_missing_globals`，见模块内文档与 web-runtime.js 改造点）。
#[cfg(target_arch = "wasm32")]
mod lua_bridge;

/// Web 宿主的 GPU 渲染第一片：把引擎渲染器接到画布上（`?rustlua-gpu=1` 时启用）。
/// 默认**不编译**：`wgpu` 的 GL 后端要占发布产物体积，见 `Cargo.toml` 的 `gpu2d` 特性。
#[cfg(all(target_arch = "wasm32", feature = "gpu2d"))]
mod gpu_host;

/// Rust 侧统一模拟的第一片：wasm 里跑工程运行时场景（给 GPU 通道提供真场景）。
#[cfg(all(target_arch = "wasm32", feature = "gpu2d"))]
mod rust_scene;

/// 平台音频出口：wasm 上没有可读文件系统也没有音频设备，播放交给浏览器/微信的
/// "用 URL 播放"通道（浏览器 `HTMLAudioElement`、微信 `InnerAudioContext`）。
#[cfg(target_arch = "wasm32")]
mod audio_sink;

/// 批量帧输入（`RLFI` 线格式）→ 引擎 `InputSnapshot`。
///
/// **刻意不加 wasm 门**：这一层是纯数据换算（两边的表都在 host 上可编译），
/// 不加门才能在桌面用测试守住"每个键名都有归宿""按住不动不产生按下边沿"这两条契约——
/// 而这两条恰恰是"某个键按了没反应""边沿分支每帧触发"的成因。
mod frame_input;

/// 两层共用的几何：设计分辨率 → 画布等比适配盒（含 contain 黑边）。
///
/// 同样**刻意不加门**：纯数学 + 纯 `u32`，不加门才能在桌面单测里守住"适配盒与设计分辨率同比"
/// 这条契约。它一旦错，GPU 层会被非等比拉伸，而画布属性/帧率读数**全都正常**——
/// 这种错只能靠小测试抓（见模块内 `tests`）。
mod fit;

/// wasm 侧诊断的**唯一安装点**：panic hook（"炸了"）+ `tracing` 出口（"走到哪一步了"）。
///
/// 两件事都收在这里，是因为它们必须同时在场才有用：只有 panic hook 时，事故现场只剩一句
/// `unreachable`；只有日志时，trap 之后什么都不再打。各入口统一调这一个函数。
#[cfg(target_arch = "wasm32")]
pub(crate) fn install_panic_hook() {
    wasm_log::install();
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&wasm_bindgen::JsValue::from_str(&info.to_string()));
    }));
}

/// wasm 侧的 `tracing` 出口：引擎的诊断行（字体/资源/渲染…）送进页面控制台。
///
/// 与 panic hook 配对：panic hook 管"炸了"，它管"走到哪一步了"——上一轮查字体时，
/// 引擎里现成的 `[render] font atlas ready` 在浏览器里根本看不见。
#[cfg(target_arch = "wasm32")]
mod wasm_log;

/// 项目根（虚拟）：Web 端没有真实文件系统，脚本/数据都挂在 `/project/` 下，
/// 与桌面端的 `project_root` 语义一致（`data.load` 用 `root.join(path)`）。
pub const VIRTUAL_PROJECT_ROOT: &str = "/project";

/// 游戏画布的**横版设计分辨率**。游戏画布把它当 backing store 尺寸（见 `start()`），
/// GPU 画布也必须用它——引擎的 2D 取景是按设计分辨率算的，两者不一致就会拉成另一个宽高比。
///
/// 必须与 `scripts/ui/layout.lua` 的 DESIGN_W/DESIGN_H、`project.toml` 的
/// window_width/height、以及微信适配层的 VIEW_WIDTH/VIEW_HEIGHT 一致：
/// Lua 动态 UI 画在**屏幕像素空间**、不做缩放，画布尺寸不对就是面板溢出画布。
pub const DESIGN_WIDTH: u32 = 1280;
pub const DESIGN_HEIGHT: u32 = 720;

/// 资源读取器的跨端形态：桌面 `vfs::set_provider` 走 `OnceLock` 要求 `Send + Sync`；
/// wasm 单线程（`thread_local`），允许捕获非 `Send` 句柄（如 `js_sys::Function`）。
#[cfg(not(target_arch = "wasm32"))]
type ResourceReader = dyn Fn(&str) -> Option<Vec<u8>> + Send + Sync + 'static;
#[cfg(target_arch = "wasm32")]
type ResourceReader = dyn Fn(&str) -> Option<Vec<u8>> + 'static;

/// 启动 Lua 桥并执行一帧：注入资源表 → 新建 Lua 运行时 → 执行
/// `scripts/main.lua` → 调用全局 `update()`，返回其字符串结果。
///
/// `read_resource` 按**项目相对路径**（如 `"scripts/main.lua"`、`"data/items.json"`）
/// 返回资源字节；`None` = 资源不存在。需要 `'static`：provider 会进 vfs 的全局
/// 存储（桌面 `OnceLock` / wasm `thread_local`）。桌面上 provider 优先于真实
/// 文件系统（`engine::core::vfs` 的语义），因此本函数在桌面测试与 Web 宿主行为一致。
#[cfg(not(target_arch = "wasm32"))]
pub fn run_lua_once<F>(read_resource: F) -> anyhow::Result<String>
where
    F: Fn(&str) -> Option<Vec<u8>> + Send + Sync + 'static,
{
    run_lua_once_shared(Box::new(read_resource))
}

#[cfg(target_arch = "wasm32")]
pub fn run_lua_once<F>(read_resource: F) -> anyhow::Result<String>
where
    F: Fn(&str) -> Option<Vec<u8>> + 'static,
{
    run_lua_once_shared(Box::new(read_resource))
}

/// P2-④：跨端唯一实现体（此前 impl/body 两层 × 两份 cfg，共四份近似重复代码）。
/// 差异只在 `ResourceReader` 这一处 bound，被 cfg 类型别名吸收。
fn run_lua_once_shared(read_resource: Box<ResourceReader>) -> anyhow::Result<String> {
    let root = std::path::PathBuf::from(VIRTUAL_PROJECT_ROOT);
    let root_for_provider = root.clone();
    let provider = move |path: &std::path::Path| {
        let key = path
            .strip_prefix(&root_for_provider)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_string_lossy().into_owned());
        // 资源表 key 统一用 `/`（Web 语义）；Windows 桌面 PathBuf 可能混合
        // 反斜杠（如 require 的 script_dir.join("helper.lua")）。
        let key = key.replace('\\', "/");
        read_resource(&key)
    };
    // `set_provider` 的失败是 `&'static str`（非 `StdError`），保留显式构造以维持原文案。
    engine::core::vfs::set_provider(Box::new(provider)).map_err(|e| anyhow::anyhow!("{e}"))?;
    lua_once_finish(root)
}

/// 共用尾部：建运行时、执行 main.lua、跑一帧 update()。
fn lua_once_finish(root: std::path::PathBuf) -> anyhow::Result<String> {
    // 存档槽在**任何**进入引擎 Lua 的入口都要装（幂等）：JS 世界那条路也跑引擎的 Lua，
    // 少了它，同一次会话里 save.commit() 会因为入口不同而有时能存、有时存不上。
    // 只在 wasm 下有这一层：`lua_bridge` 本身只编译进 wasm，桌面走文件系统。
    #[cfg(target_arch = "wasm32")]
    let _ = crate::lua_bridge::install_storage_slot();
    #[cfg(target_arch = "wasm32")]
    let _ = crate::audio_sink::install();
    let mut runtime = engine::core::script::LuaRuntime::new()?;
    runtime.set_project_root(root.clone());
    runtime.execute_project_script(&root.join("scripts/main.lua"))?;
    Ok(runtime.call_update()?)
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use wasm_bindgen::{JsCast, prelude::*};
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{HtmlCanvasElement, Request, RequestInit, RequestMode, Response};

    /// Web 入口：把 JS 侧同步资源读取器接进 Lua 桥并跑一帧。
    ///
    /// `read_file: (path: string) -> string | null`——由宿主提供（打包资源
    /// 表 / fetch 预加载表皆可）。返回 `update()` 的字符串结果，或错误文本。
    #[wasm_bindgen]
    pub fn start_lua(read_file: &js_sys::Function) -> Result<String, JsValue> {
        super::install_panic_hook();
        // 闭包 move 持有 read_file（JsValue 可 clone）；wasm 端 vfs provider 存
        // thread_local，不要求 Send/Sync。
        let read_file = read_file.clone();
        super::run_lua_once(move |key| {
            let value = read_file
                .call1(&JsValue::NULL, &JsValue::from_str(key))
                .ok()?;
            value.as_string().map(|s| s.into_bytes())
        })
        .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// 临时探针：逐级定位 wasm 启动路径（验证后移除）。
    #[wasm_bindgen]
    pub fn probe_stage(stage: u32) -> Result<(), JsValue> {
        super::install_panic_hook();
        match stage {
            0 | 1 | 2 => {
                engine::core::script::probe_startup_stage(stage as u8)
                    .map_err(|e| JsValue::from_str(&e))?;
            }
            3 => {
                let _ = engine::core::script::LuaRuntime::new()
                    .map_err(|e| JsValue::from_str(&e.to_string()))?;
            }
            4 | 5 => {
                // P2-④：probe 4/5 的 setup 完全相同，收敛为一个函数；5 多跑一帧 update。
                let mut runtime = probe_boot()?;
                if stage == 5 {
                    runtime
                        .call_update()
                        .map_err(|e| JsValue::from_str(&e.to_string()))?;
                }
            }
            _ => return Err(JsValue::from_str("unknown stage")),
        }
        Ok(())
    }

    /// probe 专用：注入 mock 资源 provider，建运行时并执行 `scripts/main.lua`。
    fn probe_boot() -> Result<engine::core::script::LuaRuntime, JsValue> {
        let root = std::path::PathBuf::from(super::VIRTUAL_PROJECT_ROOT);
        let root_for_key = root.clone();
        let _ = engine::core::vfs::set_provider(move |path: &std::path::Path| {
            let key = path
                .strip_prefix(&root_for_key)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| path.to_string_lossy().into_owned())
                .replace('\\', "/");
            match key.as_str() {
                "scripts/main.lua" => Some(br#"function update() return 'probe' end"#.to_vec()),
                "scripts/helper.lua" => {
                    Some(br#"return { greet = function() return 'ok' end }"#.to_vec())
                }
                _ => None,
            }
        });
        let mut runtime = engine::core::script::LuaRuntime::new()
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        runtime.set_project_root(root);
        let main = std::path::PathBuf::from(super::VIRTUAL_PROJECT_ROOT).join("scripts/main.lua");
        runtime
            .execute_project_script(&main)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(runtime)
    }

    /// 浏览器（webgl 发布）入口：`document` / `window` / `fetch` 都是浏览器原生形态。
    /// 启动流程 = 预加载项目资源 → Lua 桥（main.lua + 一帧 update()）→ 诊断。
    #[wasm_bindgen]
    pub async fn start(canvas_id: &str, title: String) -> Result<(), JsValue> {
        super::install_panic_hook();
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("missing window"))?;
        let document = window
            .document()
            .ok_or_else(|| JsValue::from_str("missing document"))?;
        document.set_title(&title);
        let canvas = document
            .get_element_by_id(canvas_id)
            .ok_or_else(|| JsValue::from_str("missing canvas"))?
            .dyn_into::<HtmlCanvasElement>()?;
        // 画布尺寸 = 游戏的横版设计分辨率（见 `DESIGN_WIDTH`/`DESIGN_HEIGHT`：GPU 画布用同一对常量）。
        canvas.set_width(super::DESIGN_WIDTH);
        canvas.set_height(super::DESIGN_HEIGHT);
        let manifest = fetch_text("pack/manifest.txt").await?;
        let project = fetch_text("pack/project.toml").await?;
        let startup_scene = project_key(&project, "startup_scene")
            .unwrap_or_else(|| "scenes/main.scene".to_owned());
        let scene = fetch_text(&format!("pack/{startup_scene}")).await?;

        // 预加载清单内的全部资源 → 内存资源表（迁移后存 wasm 全局，`lua_boot()`
        // 的 vfs provider 从这里读；与微信端"打包资源表"语义一致）。`pack::decode`
        // （压缩/加密自动解开）由引擎侧 `read_project_script` 内部完成，因此这里
        // 直接存打包原始字节。
        let mut resources: std::collections::HashMap<String, Vec<u8>> = Default::default();
        for relative in manifest_paths(&manifest) {
            match fetch_bytes(&format!("pack/{relative}")).await {
                Ok(bytes) => {
                    resources.insert(relative, bytes);
                }
                Err(e) => {
                    web_sys::console::warn_1(&JsValue::from_str(&format!(
                        "preload failed for {relative}: {}",
                        js_error_text(&e)
                    )));
                }
            }
        }
        let preloaded = resources.len();
        super::lua_bridge::set_resources(resources);

        web_sys::console::log_1(&JsValue::from_str(&format!(
            "RustLua WebGL/WASM started: {title}; project bytes={}; manifest files={}; startup_scene={}; scene bytes={}; preloaded={}",
            project.len(),
            manifest.lines().skip(1).count(),
            startup_scene,
            scene.len(),
            preloaded,
        )));
        // GPU 渲染路径（WP5 第五片）：**只在 `?rustlua-gpu=1` 时**建，默认路径一行不动。
        // 放在 Rust 侧而不是页面脚本里，是为了不碰 `plugins/webgl/` 下的既有改动。
        #[cfg(feature = "gpu2d")]
        if gpu_requested(&window) {
            // 失败不往上抛：Canvas2D 那条路仍然可用，GPU 只是**附加**通道。
            // 成败都在 `data-rustlua-gpu` 里（门与人工排查同一处读）。
            // 启动场景路径一并交下去：Rust 侧场景要自己从预加载资源表里读**同一份**场景字节
            // （引擎自己的 loader 读不到文件时会静默退回内置诊断场景，那样"起场景成功"是假象）。
            let scene_path = sim_scene_override(&window).unwrap_or_else(|| startup_scene.clone());
            if let Err(error) = super::gpu_host::gpu_host_run(canvas_id, &scene_path).await {
                web_sys::console::error_1(&JsValue::from_str(&format!(
                    "[gpu] host failed: {}",
                    js_error_text(&error)
                )));
            }
        }
        Ok(())
    }

    /// 微信小游戏入口（**尚未接进发布链路**，状态见下）。
    ///
    /// ## 它想解决什么
    ///
    /// 小游戏里没有浏览器那一套宿主：`window.fetch` 不存在、`Response` 没有 `arrayBuffer()`、
    /// `HTMLCanvasElement` 也不是那个构造器。wasm-bindgen `--target web` 的胶水按**浏览器
    /// 构造器**做 `instanceof` 与原型调用，所以浏览器版的 [`start`] 不能原样搬过去。
    /// 这个入口把宿主面收窄成四个参数（`String` + 两个 `&js_sys::Function`），读取与回报
    /// 都由适配层提供（见 `plugins/wechat-minigame/`）。
    ///
    /// 签名刻意**不含宿主对象反射**（`Reflect::get` 取字段那种）：实测反射版本在
    /// `--target no-modules` 胶水下表现为"函数静默返回 undefined、既不读文件也不抛错"，
    /// 是最难查的一类失败。基本类型与函数参数由 wasm-bindgen 直接编解码，路径更短。
    ///
    /// 启动流程 = 验证三必需文件 → 复用 [`start_lua`] 的读取器跑 Lua 桥（main.lua +
    /// 一帧 update()）→ 逐条 report 诊断。
    ///
    /// ## 当前状态：**本入口未接线**（微信侧 Lua 交互已改走 `__RUSTLUA_LUA_BRIDGE` 的
    /// `lua_boot_with_reader`，不经本函数）
    ///
    /// * 它能编译进 `wasm32-unknown-unknown`，但发布链路**不调用**本函数：微信适配层
    ///   走的是 `lua_boot_with_reader(readText)` 桥（见 `plugins/wechat-minigame/
    ///   wechat-adapter.js` 的 `loadWasmCoreOnce`），Lua 执行经桥完成，本入口保留作
    ///   无 DOM 直启的备选路径。
    /// * 早期"新增导出在 no-modules 下静默 undefined"的疑虑**已证伪**：根因是验证脚本
    ///   拿 `initSync()` 返回的**原始 wasm 导出**（不解码）断言，而 JS 解码包装层挂在
    ///   胶水函数对象（`glue.lua_*`）上；微信侧 `result.host.lua_*` 才是正确对象。
    ///   `artifacts/verify-no-modules-bridge.mjs` 30/30 通过（含多值协议、patch 注入、
    ///   缺失全局、回调注册、错误传播）。
    ///
    /// | 参数 | 语义 |
    /// |---|---|
    /// | `title` | 发布名（进启动诊断） |
    /// | `canvas_info` | 画布来源/尺寸摘要（"画布拿得到"的证据） |
    /// | `read_file` | `string -> string \| null`，接到适配层同一套读取（含分包等待/远端回落） |
    /// | `report` | `(message: string) -> void`，收下这行启动诊断 |
    #[wasm_bindgen]
    pub fn start_wechat(
        title: String,
        canvas_info: String,
        read_file: &js_sys::Function,
        report: &js_sys::Function,
    ) -> Result<(), JsValue> {
        super::install_panic_hook();
        // 三个必需文件：缺任何一个都说明资源形态不对，直接失败并说清是哪一个。
        let manifest = read_required(read_file, "pack/manifest.txt")?;
        let project = read_required(read_file, "pack/project.toml")?;
        let startup_scene = project_key(&project, "startup_scene")
            .unwrap_or_else(|| "scenes/main.scene".to_owned());
        let scene = read_required(read_file, &format!("pack/{startup_scene}"))?;
        let summary = format!(
            "RustLua WeChat/WASM started: {title}; project bytes={}; manifest files={}; startup_scene={}; scene bytes={}; canvas={canvas_info}",
            project.len(),
            manifest.lines().skip(1).count(),
            startup_scene,
            scene.len()
        );
        report.call1(&JsValue::NULL, &JsValue::from_str(&summary))?;
        // Lua 桥：同一读取器跑 scripts/main.lua + 一帧 update()（start_lua 内部
        // 注入 vfs provider，与 start() 的浏览器路径等价）。
        match start_lua(read_file) {
            Ok(update) => report.call1(
                &JsValue::NULL,
                &JsValue::from_str(&format!("Lua update: {update}")),
            )?,
            Err(e) => report.call1(
                &JsValue::NULL,
                &JsValue::from_str(&format!("Lua startup error: {}", js_error_text(&e))),
            )?,
        };
        Ok(())
    }

    fn read_required(read_file: &js_sys::Function, path: &str) -> Result<String, JsValue> {
        let value = read_file.call1(&JsValue::NULL, &JsValue::from_str(path))?;
        value
            .as_string()
            .ok_or_else(|| JsValue::from_str(&format!("wechat wasm: cannot read {path}")))
    }

    /// JsValue → 可读文本（as_string 优先，兜底用 Debug 表示）。
    fn js_error_text(e: &JsValue) -> String {
        e.as_string().unwrap_or_else(|| format!("{e:?}"))
    }

    /// 页面是否显式要求走 GPU 渲染路径（`?rustlua-gpu=1`）。
    ///
    /// 为什么是"显式要求"而不是"有 WebGL2 就走"：这条通道还在逐片搭建中，默认必须是
    /// 既有 Canvas2D 路径——否则一次误判就会让所有平台的画面一起变。
    #[cfg(feature = "gpu2d")]
    fn gpu_requested(window: &web_sys::Window) -> bool {
        location_search(window).is_some_and(|search| search.contains("rustlua-gpu=1"))
    }

    /// URL 位 `rustlua-sim-scene=<名字>`：Rust 侧运行时场景改用这个场景起（缺省 = 工程启动场景）。
    ///
    /// 存在的理由是一个实测读数：这个工程的 `scenes/main.scene` 只有 meta、**零实体**——
    /// 画面内容全由 Lua 现造。所以"引擎能不能画出工程的真场景"不能拿启动场景去问
    /// （零实体是正确、但什么也证明不了的读数），得换一个**有实体**的场景。
    ///
    /// 只接受纯场景名：这是从 URL 进到"读资源"的入口，别让它变成任意路径读取。
    #[cfg(feature = "gpu2d")]
    fn sim_scene_override(window: &web_sys::Window) -> Option<String> {
        const PREFIX: &str = "rustlua-sim-scene=";
        let search = location_search(window)?;
        let start = search.find(PREFIX)? + PREFIX.len();
        let rest = &search[start..];
        let end = rest.find('&').unwrap_or(rest.len());
        let value = rest[..end].trim();
        if value.is_empty()
            || !value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '/')
        {
            return None;
        }
        Some(if value.ends_with(".scene") {
            value.to_owned()
        } else {
            format!("scenes/{value}.scene")
        })
    }

    #[cfg(feature = "gpu2d")]
    fn location_search(window: &web_sys::Window) -> Option<String> {
        window.location().search().ok()
    }

    fn project_key(text: &str, key: &str) -> Option<String> {
        text.lines().find_map(|line| {
            let line = line.trim().trim_start_matches('\u{feff}');
            line.strip_prefix(&format!("{key} ="))
                .map(|value| value.trim().trim_matches('"').to_owned())
                .filter(|value| !value.trim().is_empty())
        })
    }

    /// 解析 `assetc package` 产出的 `manifest.txt`：
    /// 首行是 `assetc_manifest=1\t版本` 头，后续每行 `路径\t大小\t校验和`。
    /// 返回全部相对路径（Tab 分隔，取首字段；跳过空行）。
    fn manifest_paths(text: &str) -> Vec<String> {
        text.lines()
            .skip(1)
            .map(|line| line.split('\t').next().unwrap_or("").trim().to_owned())
            .filter(|path| !path.is_empty())
            .collect()
    }

    /// 单个 GET 请求 → 已校验 ok 的 `Response`（P2-④：fetch_bytes/fetch_text 共用）。
    async fn fetch_response(url: &str) -> Result<Response, JsValue> {
        let init = RequestInit::new();
        init.set_method("GET");
        init.set_mode(RequestMode::SameOrigin);
        let request = Request::new_with_str_and_init(url, &init)?;
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("missing window"))?;
        let response = JsFuture::from(window.fetch_with_request(&request))
            .await?
            .dyn_into::<Response>()?;
        if !response.ok() {
            return Err(JsValue::from_str(&format!("HTTP {}", response.status())));
        }
        Ok(response)
    }

    async fn fetch_bytes(url: &str) -> Result<Vec<u8>, JsValue> {
        let response = fetch_response(url).await?;
        let buffer = JsFuture::from(response.array_buffer()?).await?;
        Ok(js_sys::Uint8Array::new(&buffer).to_vec())
    }

    async fn fetch_text(url: &str) -> Result<String, JsValue> {
        let response = fetch_response(url).await?;
        let text = JsFuture::from(response.text()?).await?;
        Ok(text.as_string().unwrap_or_default())
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn native_stub() {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// run_lua_once 走完整接线：provider 注入 → Lua 运行时 → 执行 main.lua →
    /// 一帧 update()。资源全在内存表里（与 Web 宿主一致，不落盘）。
    #[test]
    fn run_lua_once_executes_project_script_with_modules_and_data() {
        let mut table: HashMap<String, Vec<u8>> = HashMap::new();
        table.insert(
            "scripts/main.lua".to_owned(),
            br#"
                local helper = require('helper')
                function update()
                    local items = data.load('data/items.json')
                    return 'ok:' .. helper.greet() .. ':' .. items.name
                end
            "#
            .to_vec(),
        );
        table.insert(
            "scripts/helper.lua".to_owned(),
            br#"return { greet = function() return 'hi' end }"#.to_vec(),
        );
        table.insert(
            "data/items.json".to_owned(),
            br#"{"name":"sword"}"#.to_vec(),
        );

        let result = run_lua_once(move |key| table.get(key).cloned());
        assert_eq!(result.expect("run_lua_once must succeed"), "ok:hi:sword");
    }
}
