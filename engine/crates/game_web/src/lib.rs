//! 浏览器（WebGL / 微信小游戏）wasm 入口与宿主 API（WP10）。
//!
//! Lua 桥接线（`run_lua_once`）：把 Web 宿主拿到的项目资源（.lua 脚本 /
//! require 模块 / data JSON）注入 `rustlua_infra::vfs` 的资源提供者，再启动
//! `rustlua_scripting::script::LuaRuntime` 执行项目脚本并跑一帧 `update()`。
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

/// WP8 引擎宿主骨架：调用方把**自己的**画布递进来，wasm 侧建 surface/device 并按
/// `rustlua_infra::frame_clock` 的固定步长策略逐步 + 呈现（微信没有 document/window，
/// 这条通道不碰任何浏览器构造器）。与 `gpu_host` 一样只在 `gpu2d` 档存在。
#[cfg(all(target_arch = "wasm32", feature = "gpu2d"))]
mod host_runtime;

/// P1 共享核心：画布 → device → Renderer → `RustScene` → present 的整条渲染链。
/// `gpu_host`（浏览器外壳）与 `host_runtime`（引擎宿主外壳）共用这一条路；
/// 只在 `gpu2d` 档存在（内含 `wgpu`）。
#[cfg(all(target_arch = "wasm32", feature = "gpu2d"))]
mod engine_core;

/// 统一宿主的帧时钟配置：纯数据、无 wgpu/web_sys 依赖，门是 `any(target_arch = "wasm32", test)`
/// ——桌面单测据此守住"默认值 = (60, 8, 0.25)"与"喂 Lua 的步长必须 f64 现算"两条契约，
/// 而原生非测试构建里它没有消费者，编译进来只会变成死代码。
#[cfg(any(target_arch = "wasm32", test))]
mod host_config;

/// P1 第二片：两个平台**同一台宿主**（时钟 + 共享核心 + 帧泵）。
///
/// `engine_core` 统一了"一帧怎么画"，这一层统一"一帧由谁驱动、该跑几步"——
/// 抽取前两张计划表逐格同形却各写一遍（见模块文档里的对照表）。
#[cfg(all(target_arch = "wasm32", feature = "gpu2d"))]
mod engine_host;

/// 平台音频出口：wasm 上没有可读文件系统也没有音频设备，播放交给浏览器/微信的
/// "用 URL 播放"通道（浏览器 `HTMLAudioElement`、微信 `InnerAudioContext`）。
#[cfg(target_arch = "wasm32")]
mod audio_sink;

/// 批量帧输入（`RLFI` 线格式）→ 引擎 `InputSnapshot`。
///
/// **刻意不门到 wasm**：门是 `any(target_arch = "wasm32", test)`，所以桌面测试照样守住
/// "每个键名都有归宿""按住不动不产生按下边沿"这两条契约——而这两条恰恰是"某个键按了没反应"
/// 与"边沿分支每帧触发"的成因。
///
/// 为什么门里还要留 wasm 那一半：**原生非测试构建里这一层没有消费者**（消费者全是 wasm 侧的
/// `gpu_host` / `host_runtime` / `rust_scene`），不门就会让整层在原生 lib 里变成死代码。
#[cfg(any(target_arch = "wasm32", test))]
mod frame_input;

/// 两层共用的几何：设计分辨率 → 画布等比适配盒（含 contain 黑边）。
///
/// 同样**刻意不门到 wasm**（`any(target_arch = "wasm32", test)`）：纯数学 + 纯 `u32`，
/// 桌面单测守住"适配盒与设计分辨率同比"这条契约。它一旦错，GPU 层会被非等比拉伸，
/// 而画布属性/帧率读数**全都正常**——这种错只能靠小测试抓（见模块内 `tests`）。
#[cfg(any(target_arch = "wasm32", test))]
mod fit;

/// 启动状态属性（`data-rustlua-boot`）的归属判据：**只补不盖** —— `failed` 绝不能被盖成 `ok`。
///
/// 与 `host_config` / `fit` 同样刻意不门到 wasm：纯判据、不碰 `web_sys`，桌面单测据此守住那条
/// 负例 —— 它一旦错，"启动真失败"会被读成成功，而这正是诊断与回归门依赖的属性。
#[cfg(any(target_arch = "wasm32", test))]
mod boot_state;

/// Build-time static dependency index selection for the synchronous VFS
/// provider. Kept host-testable so closure and fallback behavior are verified
/// without a browser.
#[cfg(any(target_arch = "wasm32", test))]
mod resource_index;

/// 发布包（RLB 容器）在 wasm 宿主上的挂载（`docs/67` §7.3，批 B）。
/// 只有 wasm 编译它：它要装 `vfs` 的 lister，而桌面的 lister 要求 `Send + Sync`
/// （那台机器上由 `crates/game` 自己装）。纯字符串那部分在 `resource_index::bundle_files`，
/// 桌面上也有单测。
#[cfg(target_arch = "wasm32")]
mod bundle_mount;

/// wasm 线性内存当前大小（字节）。docs/65 P0 的阶段遥测用它把"300MB 峰值属于哪一段"
/// 落成逐阶段读数。
///
/// **只在 wasm 上存在**：调用方全是 wasm 侧（`start` 的预载遥测、`engine_core` 与两个宿主外壳）。
/// 原先还给了一条"非 wasm 恒 0"的实现，但桌面上没有任何消费者——留着它只会让这条通道在原生
/// 构建里变成死代码，而"桌面调试器本来就能看内存"这件事不需要一个恒 0 的函数来表达。
#[cfg(target_arch = "wasm32")]
pub(crate) fn wasm_memory_bytes() -> usize {
    std::arch::wasm32::memory_size(0) * 65_536
}

/// wasm 侧诊断的**唯一安装点**：panic hook（"炸了"）+ `tracing` 出口（"走到哪一步了"）。
///
/// 两件事都收在这里，是因为它们必须同时在场才有用：只有 panic hook 时，事故现场只剩一句
/// `unreachable`；只有日志时，trap 之后什么都不再打。各入口统一调这一个函数。
#[cfg(target_arch = "wasm32")]
pub(crate) fn install_panic_hook() {
    use std::sync::atomic::{AtomicBool, Ordering};
    // 必须只装一次：`set_hook` 在**正在 panic 的栈**里再被调用会二次 panic
    // （"cannot modify the panic hook from a panicking thread"，实测 2026-10-12 微信模拟器：
    // GPU 入口的 wgpu dyn_into 失败先炸，入口里那句 install 再炸，两层 panic 把整局启动掀翻）。
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
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

/// 每帧日志的过滤器（哪些 `tracing` 行默认不发 + 页面开关）。
///
/// **刻意不门到 wasm**（`any(target_arch = "wasm32", test)`）：判据是纯字符串逻辑，
/// `windows` 那组门跑不到 wasm 的控制台路径——放这里才有回归守卫（模块内单测）。
/// 原生非测试构建里它没有消费者（消费者是 wasm 侧的 `wasm_log`），不门就会变成死代码。
#[cfg(any(target_arch = "wasm32", test))]
mod frame_log_filter;

/// 页面侧的 `set_frame_logs(enabled)`：打开/关闭**每帧日志**（`[gpu] presented …` /
/// `[render] mesh …` / `[render] batch …` / `[simulation] fixed update …`）。
///
/// 默认关：一次加载实测这四条是 654 条控制台调用里的 540 条，刷屏还会吃光 `wasm_log`
/// 的 400 条上限、把首屏诊断挤掉；同名读数已由 `data-rustlua-engine-host` 常驻提供。
/// 页面由 `?rustlua-debug=1` 驱动（`plugins/webgl/index.html` 在 `init()` 之后调用）——
/// 与既有的"左上加载行 / 底部运行行"同一个开关，不给玩家多一个要记的旋钮。
#[cfg(target_arch = "wasm32")]
pub use wasm_log::set_frame_logs;

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
/// 文件系统（`rustlua_infra::vfs` 的语义），因此本函数在桌面测试与 Web 宿主行为一致。
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
    rustlua_infra::vfs::set_provider(Box::new(provider)).map_err(|e| anyhow::anyhow!("{e}"))?;
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
    let mut runtime = rustlua_scripting::script::LuaRuntime::with_installers(
        rustlua_gameplay::input_lua::registry(),
    )?;
    runtime.set_project_root(root.clone());
    runtime.execute_project_script(&root.join("scripts/main.lua"))?;
    Ok(runtime.call_update()?)
}

/// 资源预载的在途请求上限。
///
/// 8 的来路：HTTP/1.1 的浏览器每源连接上限通常就是 6，超过它只是排队、不增加并发度；
/// 留一点余量让"上一个响应体还在解析时下一个已经发出"。调大没有收益，调小直接线性变慢。
///
/// 桌面单测只喂 `preload_launch_batch` 的显式 `limit`，所以原生构建下它没有消费者。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const PRELOAD_CONCURRENCY: usize = 8;

/// 每一步该发起哪些下标：把在途请求数填到 `limit`（其余参数相同则行为与既有的串行版语义一致——
/// 每个下标恰好发起一次，不重不漏）。
///
/// 抽成纯函数是为了能在**桌面**单测它：`windows` 那组门跑不到 wasm 的 fetch 路径，
/// 而"在途数不越界、下标恰好覆盖一遍"是这段调度里唯一会写错的东西。
///
/// 原生构建下没有消费者（wasm 侧 `mod wasm` 用；桌面单测在 `tests` 里直接喂它），
/// 与上面的 `PRELOAD_CONCURRENCY` 同一处理。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn preload_launch_batch(
    queued: usize,
    in_flight: usize,
    total: usize,
    limit: usize,
) -> std::ops::Range<usize> {
    let room = limit.saturating_sub(in_flight);
    let end = queued.saturating_add(room).min(total);
    queued..end
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use std::collections::HashMap;

    use futures_util::stream::{FuturesUnordered, StreamExt};
    use wasm_bindgen::{JsCast, prelude::*};
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{HtmlCanvasElement, Request, RequestInit, RequestMode, Response};

    use super::{PRELOAD_CONCURRENCY, preload_launch_batch};

    /// 限并发预载 `pack/` 下的全部选中资源，返回成功取回的字节总数。
    ///
    /// 失败的条目只告警、不中断（与串行版逐字相同的行为）：缺一个 sfx 不该让整个页面起不来，
    /// 而缺关键脚本会在后面 Lua 启动阶段给出更有指向性的错误。
    async fn preload_all(selected: &[String], resources: &mut HashMap<String, Vec<u8>>) -> usize {
        let mut pending: Vec<(String, Result<Vec<u8>, JsValue>)> = Vec::new();
        let mut in_flight = FuturesUnordered::new();
        let mut queued = 0usize;
        loop {
            let launch =
                preload_launch_batch(queued, in_flight.len(), selected.len(), PRELOAD_CONCURRENCY);
            queued = launch.end;
            for index in launch {
                in_flight.push(async move {
                    let relative = &selected[index];
                    let bytes = fetch_bytes(&format!("pack/{relative}")).await;
                    (relative.clone(), bytes)
                });
            }
            match in_flight.next().await {
                Some(result) => pending.push(result),
                None => break,
            }
        }
        let mut bytes_total = 0usize;
        for (relative, result) in pending {
            match result {
                Ok(bytes) => {
                    bytes_total += bytes.len();
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
        bytes_total
    }

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
            0..=2 => {
                rustlua_scripting::script::probe_startup_stage(stage as u8)
                    .map_err(|e| JsValue::from_str(&e))?;
            }
            3 => {
                let _ = rustlua_scripting::script::LuaRuntime::with_installers(
                    rustlua_gameplay::input_lua::registry(),
                )
                .map_err(|e| JsValue::from_str(&e.to_string()))?;
            }
            4 | 5 => {
                // P2-④：probe 4/5 的 setup 完全相同，收敛为一个函数；5 多跑一帧 update。
                let runtime = probe_boot()?;
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
    fn probe_boot() -> Result<rustlua_scripting::script::LuaRuntime, JsValue> {
        let root = std::path::PathBuf::from(super::VIRTUAL_PROJECT_ROOT);
        let root_for_key = root.clone();
        let _ = rustlua_infra::vfs::set_provider(move |path: &std::path::Path| {
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
        let mut runtime = rustlua_scripting::script::LuaRuntime::with_installers(
            rustlua_gameplay::input_lua::registry(),
        )
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
        // 容器形态（`docs/67` §7.3 批 B）：清单里有 `bundles/*.rlb` 就只抓容器、不抓条目。
        // 挂载之后 `project.toml` 与启动场景改走 `read_virtual`（容器优先、HTTP 兜底）。
        let bundle_paths = super::resource_index::bundle_files(&manifest);
        let mounted_entries = if bundle_paths.is_empty() {
            0
        } else {
            // 只同步挂 `unit = main` 的组（启动必需），`sub` 组后台补挂：
            // 启动路径上少下载/少解析几 MB，标题画面出来的时间不再被整包装载拖着。
            let (boot_paths, deferred) = split_by_unit(&bundle_paths).await;
            let mut bundle_bytes: std::collections::HashMap<String, Vec<u8>> = Default::default();
            let fetched = preload_all(&boot_paths, &mut bundle_bytes).await;
            let entries: Vec<(String, Vec<u8>)> = boot_paths
                .iter()
                .filter_map(|path| bundle_bytes.remove(path).map(|bytes| (path.clone(), bytes)))
                .collect();
            let count =
                super::bundle_mount::mount(entries).map_err(|error| JsValue::from_str(&error))?;
            web_sys::console::log_1(&JsValue::from_str(&format!(
                "[resources] bundles={} (boot) deferred={} entries={} bundle_bytes={fetched}",
                boot_paths.len(),
                deferred.len(),
                count
            )));
            if !deferred.is_empty() {
                // 后台补挂。**并发发起**而不是串行等：浏览器本来就能并发 fetch，
                // 串行等待只会把总时长加起来（与逐条目预载那条注释同一个理由）。
                super::bundle_mount::set_pending(deferred.len());
                wasm_bindgen_futures::spawn_local(async move {
                    let mut rest: std::collections::HashMap<String, Vec<u8>> = Default::default();
                    let bytes = preload_all(&deferred, &mut rest).await;
                    let entries: Vec<(String, Vec<u8>)> = deferred
                        .iter()
                        .filter_map(|path| rest.remove(path).map(|data| (path.clone(), data)))
                        .collect();
                    let collected = entries.len();
                    match super::bundle_mount::mount_more(entries) {
                        Ok(added) => web_sys::console::log_1(&JsValue::from_str(&format!(
                            "[resources] bundles_deferred mounted={collected} entries=+{added} bundle_bytes={bytes}"
                        ))),
                        Err(error) => web_sys::console::error_1(&JsValue::from_str(&format!(
                            "[resources] 延迟挂载失败：{error}"
                        ))),
                    }
                    super::bundle_mount::set_pending(0);
                });
            }
            count
        };
        let project = read_virtual_text("project.toml").await?;
        let startup_scene = project_key(&project, "startup_scene")
            .unwrap_or_else(|| "scenes/main.scene".to_owned());
        let scene = read_virtual_bytes(&startup_scene).await?;

        // 同步 VFS 仍需启动前取得资源字节；新包按静态依赖闭包预载，动态/未知
        // 引用、坏索引和旧包保守回退到完整 manifest。`pack::decode` 由引擎读取侧完成。
        let mut resources: std::collections::HashMap<String, Vec<u8>> = Default::default();
        let all_paths = manifest_paths(&manifest);
        let mut roots = vec![
            startup_scene.clone(),
            project_key(&project, "main_script").unwrap_or_else(|| "scripts/main.lua".to_owned()),
        ];
        roots.extend(
            ["project.toml", "chars.txt", "settings.toml"]
                .iter()
                .filter(|path| all_paths.iter().any(|entry| entry == **path))
                .map(|path| (*path).to_owned()),
        );
        roots.extend(
            ["font", "font_fallback"]
                .iter()
                .filter_map(|key| project_key(&project, key))
                .filter(|path| all_paths.contains(path)),
        );
        // Lua require is synchronous, so every packaged module remains in the
        // initial set even when it is not reachable by a literal require edge.
        roots.extend(
            all_paths
                .iter()
                .filter(|path| path.starts_with("scripts/") && path.ends_with(".lua"))
                .cloned(),
        );
        let (selected_paths, full_fallback) = if mounted_entries > 0 {
            // 容器形态：容器里的条目不再逐个 fetch——同步读由容器提供。
            // 但清单里还有**容器外**的骨架文件：`assets/fonts/*` 被 `--keep` 有意留在目录里
            // （微信 `wx.loadFont` 只吃真实文件路径，见 docs/67 §11），`project.toml`/计划表同理。
            // 它们不在任何容器里，不预载就没有同步通道能读到——实测症状是
            // `font probe ok=false` ⇒ 每帧 `ui.measure_text: project font unavailable`、
            // 标题按钮文字全空。所以这里预载"清单减去容器"的那几项（字体 288 KB + KB 级骨架）。
            let loose: Vec<String> = all_paths
                .iter()
                .filter(|path| !bundle_paths.contains(path))
                .cloned()
                .collect();
            (loose, false)
        } else {
            match fetch_text("pack/dependencies.json").await {
                Ok(index) => {
                    match super::resource_index::select_resources(&index, &all_paths, &roots) {
                        Ok(selection) => {
                            // **故意不是 warn**：全量预载是正确行为（见 `select_resources` 的文档），
                            // 用 `full preload fallback` 这种措辞打 warn 会让每个读者以为出了故障——
                            // 2026-10-17 那一族静默失败正是被"这看起来像兜底"误导出来的。
                            if let Some(reason) = &selection.fallback_reason {
                                web_sys::console::log_1(&JsValue::from_str(&format!(
                                    "[resources] {reason}"
                                )));
                            }
                            (selection.paths, selection.full_fallback)
                        }
                        Err(error) => {
                            web_sys::console::warn_1(&JsValue::from_str(&format!(
                                "[resources] 依赖索引不可用（{error}）⇒ 预载全量（正确行为，见 select_resources 文档）"
                            )));
                            (all_paths.clone(), true)
                        }
                    }
                }
                Err(error) => {
                    web_sys::console::warn_1(&JsValue::from_str(&format!(
                        "[resources] dependency index unavailable; full preload: {}",
                        js_error_text(&error)
                    )));
                    (all_paths.clone(), true)
                }
            }
        };
        web_sys::console::log_1(&JsValue::from_str(&format!(
            "[resources] selected={} manifest={} full_fallback={full_fallback}",
            selected_paths.len(),
            all_paths.len(),
        )));
        // 资源预载阶段遥测（docs/65 P0：峰值要能归因到"全量预载"还是"字体图集"）：
        // 前后各取一次线性内存与耗时。
        //
        // **并发化（2026-10-17）**：这一段原先是一条 `for … .await` 的串行链——一个包 1,300+ 个
        // 小文件，桌面实测 13.7 s、平均每个请求 ~10 ms，读数说明瓶颈是**请求个数**而不是字节数；
        // 手机经 WiFi 每请求 30–50 ms 的往返乘上去就是几十秒的闪屏。`fetch` 是浏览器原生并发
        // 的，串行等待纯粹是我们自己排的队，所以这里只改**发起顺序**：始终维持
        // [`PRELOAD_CONCURRENCY`] 个在途请求，回来的字节仍按路径插进同一张资源表
        // （表是 map，插入顺序不参与语义）。
        let preload_started = web_time::Instant::now();
        let preload_mem_before = crate::wasm_memory_bytes();
        let preload_bytes = preload_all(&selected_paths, &mut resources).await;
        web_sys::console::log_1(&JsValue::from_str(&format!(
            "[resources] preload_ms={:.1} preload_bytes={preload_bytes} mem_before={preload_mem_before} mem_after={} concurrency={PRELOAD_CONCURRENCY}",
            preload_started.elapsed().as_secs_f64() * 1000.0,
            crate::wasm_memory_bytes(),
        )));
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
    /// 多根布局（微信）：boot 时只挂**当前已挂载根**里的容器，缺的留给
    /// [`mount_bundles_from_reader`] 在对应分包根 ready 后补挂。
    #[wasm_bindgen]
    pub fn mount_from_reader_partial(
        manifest: String,
        read_file: &js_sys::Function,
    ) -> Result<usize, JsValue> {
        super::bundle_mount::mount_from_reader_partial(&manifest, read_file)
    }

    /// 补挂一个分包根的容器：`paths_json` 是 `["bundles/x.rlb", ...]`（计划表里的逻辑路径）。
    #[wasm_bindgen]
    pub fn mount_bundles_from_reader(
        paths_json: String,
        read_file: &js_sys::Function,
    ) -> Result<usize, JsValue> {
        let paths: Vec<String> = serde_json::from_str(&paths_json)
            .map_err(|error| JsValue::from_str(&format!("mount_bundles paths JSON: {error}")))?;
        super::bundle_mount::mount_bundles_from_reader(&paths, read_file)
    }

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
        // 容器形态（`docs/67` §7.3）：清单里有 `bundles/*.rlb` 就用适配层读取器把它们读进来挂上；
        // 之后 `project.toml` 与启动场景走 `read_required_virtual`（容器优先、读取器兜底）。
        let mounted = super::bundle_mount::mount_from_reader(&manifest, read_file)?;
        if mounted > 0 {
            report.call1(
                &JsValue::NULL,
                &JsValue::from_str(&format!(
                    "[bundle] mounted entries={mounted} bundles={}",
                    super::bundle_mount::bundle_files(&manifest).len()
                )),
            )?;
        }
        let project = read_required(read_file, "pack/project.toml")?;
        let startup_scene = project_key(&project, "startup_scene")
            .unwrap_or_else(|| "scenes/main.scene".to_owned());
        let scene = read_required_virtual(read_file, &startup_scene)?;
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

    /// 读启动场景：容器优先，未命中再问适配层读取器。
    ///
    /// 容器形态下 `scenes/*.scene` **不在代码包里**，`read_file("pack/scenes/main.scene")`
    /// 必然失败——所以这条读取必须容器优先（`docs/67` §7.3）。
    fn read_required_virtual(
        read_file: &js_sys::Function,
        relative: &str,
    ) -> Result<String, JsValue> {
        let full = std::path::Path::new(super::VIRTUAL_PROJECT_ROOT).join(relative);
        if let Some(bytes) = super::bundle_mount::lookup(&full) {
            return Ok(String::from_utf8_lossy(&bytes).into_owned());
        }
        read_required(read_file, &format!("pack/{relative}"))
    }

    /// JsValue → 可读文本（as_string 优先，兜底用 Debug 表示）。
    fn js_error_text(e: &JsValue) -> String {
        e.as_string().unwrap_or_else(|| format!("{e:?}"))
    }

    /// 页面是否走 GPU 渲染路径：**默认开**，`?rustlua-gpu=0` 显式关。
    ///
    /// 为什么不再是"显式要求才走"：原注释的前提是"默认走既有 Canvas2D 路径"，但第 48 轮已经
    /// 把旧 JS 世界改成**默认不启动**（避免两个宿主同帧双跑），于是这条通道成了默认页面
    /// **唯一的帧所有者**——它再默认关，页面 `boot=ok`、控制台一条错都没有、却一个像素都不画。
    /// 2026-10-11 实测（发布包、同一次改动前后只差 URL）：默认 URL 截图 1,967 B 纯背景，
    /// `?rustlua-gpu=1` 48,820 B 的标题画面。两个门都只走"显式带开关"的路径，所以这条缝
    /// 一直没被覆盖——门里加不回来的东西，只能靠默认值本身对。
    ///
    /// 显式请求旧 JS 世界时**让位**（两层同框是显式验收场合，默认不同时跑两套模拟与两套绘制）；
    /// 那时要 GPU 就两个开关都写。
    #[cfg(feature = "gpu2d")]
    fn gpu_requested(window: &web_sys::Window) -> bool {
        let search = location_search(window).unwrap_or_default();
        if search.contains("rustlua-gpu=0") {
            return false;
        }
        if search.contains("rustlua-js-host=1") {
            return search.contains("rustlua-gpu=1");
        }
        true
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

    /// 按 `bundle_plan.json` 的 `unit` 把容器清单切成（启动必需, 后台补挂）。
    ///
    /// 保守优先：**没有计划表 / 解析失败 / 组名对不上文件名** → 全部算启动必需。
    /// 这条兜底的代价只是回到"整包同步挂"（也就是这一步之前的行为），
    /// 而猜错的代价是启动期读不到资源——两害相权只能选前者。
    async fn split_by_unit(paths: &[String]) -> (Vec<String>, Vec<String>) {
        let Ok(plan) = fetch_text("pack/bundle_plan.json").await else {
            return (paths.to_vec(), Vec::new());
        };
        let Ok(plan) = serde_json::from_str::<serde_json::Value>(&plan) else {
            return (paths.to_vec(), Vec::new());
        };
        let Some(groups) = plan.get("groups").and_then(|value| value.as_array()) else {
            return (paths.to_vec(), Vec::new());
        };
        // `bundle` 是计划里的容器相对路径（`bundles/core-1a2b3c4d.rlb`），
        // 与 manifest 里那行同源，所以用后缀匹配而不是重新拼文件名（拼法一改就错）。
        let mut boot = Vec::new();
        let mut deferred = Vec::new();
        for path in paths {
            let matched = groups.iter().find(|group| {
                group
                    .get("bundle")
                    .and_then(|value| value.as_str())
                    .is_some_and(|bundle| path.ends_with(bundle))
            });
            let unit = matched
                .and_then(|group| group.get("unit"))
                .and_then(|value| value.as_str())
                .unwrap_or("main");
            let name = matched
                .and_then(|group| group.get("name"))
                .and_then(|value| value.as_str())
                .unwrap_or("");
            // `unit = "main"` 是"进主包"的判据；这里判的是"**boot 期就要挂**"，两者不是一回事：
            // 音频在微信是分包（1.4 MB 不该占 4 MB 主包额度，适配层有 ensurePackForBoot 兜底），
            // 但 webgl 上 BGM 在 boot 阶段就 `play()`，延迟挂载会让它退化成 `pack/<x>` 的 404。
            // 所以：main 单元 ∨ 音频组 → boot 期挂载。
            let is_main = unit == "main" || name.starts_with("audio");
            // 匹配不上（计划与清单不同步）也当启动必需，见上面"保守优先"。
            if is_main || matched.is_none() {
                boot.push(path.clone());
            } else {
                deferred.push(path.clone());
            }
        }
        (boot, deferred)
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

    /// 读一个**工程相对路径**：容器优先，未挂载/不在容器里才走 HTTP。
    ///
    /// 为什么 `start()` 里 `project.toml` 与启动场景不能直接 `fetch("pack/<x>")`：
    /// 容器形态下它们**不在磁盘上**（`project.toml` 是骨架所以还在，场景一定在容器里）。
    /// 这条函数就是"发布形态下读一个工程文件"的单一入口（`docs/67` §7.3）。
    pub(super) async fn read_virtual_bytes(relative: &str) -> Result<Vec<u8>, JsValue> {
        let full = std::path::Path::new(super::VIRTUAL_PROJECT_ROOT).join(relative);
        if let Some(bytes) = super::bundle_mount::lookup(&full) {
            return Ok(bytes);
        }
        fetch_bytes(&format!("pack/{relative}")).await
    }

    pub(super) async fn read_virtual_text(relative: &str) -> Result<String, JsValue> {
        let bytes = read_virtual_bytes(relative).await?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
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

    /// 预载调度的契约：**每个下标恰好发起一次**、在途数从不超过上限。
    ///
    /// 这条守的是"响应回来的顺序**不**用于决定下一个发谁"——一开始就是按集合大小排满的，
    /// 因此关掉并发（`limit=1`）时读数必须与旧的串行链逐字同序（回归对照）。
    #[test]
    fn preload_launch_batch_covers_every_index_once_within_the_in_flight_limit() {
        for (total, limit) in [(1372usize, 8usize), (1, 8), (0, 8), (5, 1), (9, 3), (4, 4)] {
            let mut launched: Vec<usize> = Vec::new();
            let mut queued = 0usize;
            let mut in_flight = 0usize;
            loop {
                let batch = preload_launch_batch(queued, in_flight, total, limit);
                in_flight += batch.len();
                launched.extend(batch.clone());
                queued = batch.end;
                if in_flight == 0 {
                    break;
                }
                assert!(
                    in_flight <= limit,
                    "在途数越界：total={total} limit={limit} in_flight={in_flight}"
                );
                // 完成一个响应：这就是"每步减一"的真实形状。
                in_flight -= 1;
            }
            assert_eq!(
                launched,
                (0..total).collect::<Vec<_>>(),
                "下标必须恰好覆盖一遍且保持原序：total={total} limit={limit}"
            );
        }
    }
}
