//! wasm 上"从画布建 `wgpu::Surface`"的**唯一实现**。
//!
//! 为什么要有这个模块：web 宿主（`game_web`）与探针（`gpu-web-probe`）都要走这一步，而它是
//! 一段带 `unsafe` 的 wgpu API 边角料——两份实现就是两个"哪天 wgpu 换了写法、只修好一份"的机会。
//! 这里只收编**建 surface** 这一步；设备、适配器、限制策略仍由各自的宿主决定。
//!
//! ## 为什么不能用文档里的写法（wgpu 27 实测）
//!
//! `Instance::create_surface(canvas)` 在本版本上**用不了**：它要求
//! `HtmlCanvasElement: HasWindowHandle + HasDisplayHandle`，而 raw-window-handle 0.6.2 与 wgpu
//! 都没有为它实现这两个 trait（错误信息是
//! "the trait bound `HtmlCanvasElement: Into<SurfaceTarget<'_>>` is not satisfied"）。
//! `SurfaceTargetUnsafe` 在 wasm 上也没有 `WebCanvas` 变体，唯一通路是
//! `SurfaceTargetUnsafe::RawHandle`，把画布的 `JsValue` 指针塞进 `WebCanvasWindowHandle`。
//!
//! ## 为什么入参是 `&wasm_bindgen::JsValue` 而不是 `HtmlCanvasElement`
//!
//! 引擎这一层不该知道 DOM 类型：`HtmlCanvasElement → JsValue` 的转换由宿主（它本来就依赖
//! `web-sys`）做，这里只要求"一个活的 JS 对象指针"。代价是调用方要自己保证传入的确实是画布，
//! 收益是引擎不必为了这一步引入 `web-sys`。
//!
//! ## 为什么泄漏一个 `JsValue` 引用
//!
//! hal 侧读的是这个裸指针（`handle.obj.cast().as_ref()`），surface 存续期间它必须一直有效。
//! 宿主每个进程只建一次画布 surface，泄漏一个 JS 强引用是这里代价最小、也最不容易出错的做法；
//! 换成"把 `JsValue` 绑在 `Surface` 旁边"就要处理自引用，收益为零。

use wasm_bindgen::JsValue;

/// 从一个活的 JS 画布对象建 `wgpu::Surface`。
///
/// `canvas_js` 必须是一个**存活到进程结束**的画布对象（宿主从 `document.get_element_by_id`
/// 拿到的句柄就满足）：本函数会泄漏它的一个强引用，用来保证 hal 侧的裸指针一直有效。
pub fn canvas_surface(
    instance: &wgpu::Instance,
    canvas_js: &JsValue,
) -> Result<wgpu::Surface<'static>, String> {
    // canvas 是 JS 对象的句柄；转成 `JsValue` 之后 `&JsValue` 的地址就是 hal 侧要的那个指针。
    let js_value: JsValue = canvas_js.clone();
    let obj = std::ptr::NonNull::from(&js_value).cast();
    let canvas_handle = wgpu::rwh::WebCanvasWindowHandle::new(obj);
    // 指针已经复制进 handle，这里把所有权留在 JS 堆上，保证指针一直有效。
    std::mem::forget(js_value);

    // WebGL2 不需要 display handle 里的任何信息，空 handle 足够（与 wgpu 自己的
    // `create_surface` 对 canvas 走的分支一致）。
    let target = wgpu::SurfaceTargetUnsafe::RawHandle {
        raw_display_handle: wgpu::rwh::WebDisplayHandle::new().into(),
        raw_window_handle: canvas_handle.into(),
    };
    // SAFETY: 上面泄漏的 `JsValue` 保证画布活到进程结束，超过返回的 surface；
    // 调用方另外保证 `canvas_js` 确实是一个画布对象。
    unsafe { instance.create_surface_unsafe(target) }.map_err(|error| error.to_string())
}
