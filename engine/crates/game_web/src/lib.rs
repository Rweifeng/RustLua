#[cfg(target_arch = "wasm32")]
mod wasm {
    use wasm_bindgen::{JsCast, prelude::*};
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{HtmlCanvasElement, Request, RequestInit, RequestMode, Response};

    /// 浏览器（webgl 发布）入口：`document` / `window` / `fetch` 都是浏览器原生形态。
    #[wasm_bindgen]
    pub async fn start(canvas_id: &str, title: String) -> Result<(), JsValue> {
        install_panic_hook();
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("missing window"))?;
        let document = window
            .document()
            .ok_or_else(|| JsValue::from_str("missing document"))?;
        document.set_title(&title);
        let canvas = document
            .get_element_by_id(canvas_id)
            .ok_or_else(|| JsValue::from_str("missing canvas"))?
            .dyn_into::<HtmlCanvasElement>()?;
        // 画布尺寸 = 游戏的**竖版设计分辨率**（720×1280，M32 起）。
        // 必须与 `scripts/ui/layout.lua` 的 DESIGN_W/DESIGN_H、`project.toml` 的
        // window_width/height、以及微信适配层的 VIEW_WIDTH/VIEW_HEIGHT 一致：
        // Lua 动态 UI 画在**屏幕像素空间**、不做缩放，画布尺寸不对就是"面板横向溢出画布"。
        canvas.set_width(720);
        canvas.set_height(1280);
        let manifest = fetch_text("pack/manifest.txt").await?;
        let project = fetch_text("pack/project.toml").await?;
        let startup_scene = project_key(&project, "startup_scene")
            .unwrap_or_else(|| "scenes/main.scene".to_owned());
        let scene = fetch_text(&format!("pack/{startup_scene}")).await?;
        web_sys::console::log_1(&JsValue::from_str(&format!(
            "RustLua WebGL/WASM started: {title}; project bytes={}; manifest files={}; startup_scene={}; scene bytes={}",
            project.len(),
            manifest.lines().skip(1).count(),
            startup_scene,
            scene.len()
        )));
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
    /// ## 当前状态：**未验证、未接线（不得据它声称 wasm 在微信侧跑起来了）**
    ///
    /// * 它能编译进 `wasm32-unknown-unknown`，但**没有任何产物在跑它**：发布链路目前只把
    ///   `pkg/game_web.js`（`--target no-modules` 的 CommonJS 化胶水）+ `pkg/game_web_bg.wasm`
    ///   打进代码包，并在适配层做**实例化 + 导出清单**自证，不调用本函数。
    /// * 实测记录（no-modules 胶水 + Node 无 DOM 环境）：浏览器版的 [`start`] 在那个环境里
    ///   **能被调用**，而同一进程里**新增的导出**（本函数、以及当时用于定位的探针函数）
    ///   一律"返回 undefined、回调不被调用、也不抛错"。这条绑定层差异**没有定位到根因**，
    ///   所以它只能算"写好了待验证"，不能算能用。
    /// * 接手第一步：在 no-modules 产物上写最小用例，确认"新增导出 + 函数参数"这条路径
    ///   是否可用；确认之后再把本函数接进适配层的启动流程，并给它一个 harness 用例。
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
        install_panic_hook();
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
        Ok(())
    }

    fn read_required(read_file: &js_sys::Function, path: &str) -> Result<String, JsValue> {
        let value = read_file.call1(&JsValue::NULL, &JsValue::from_str(path))?;
        value
            .as_string()
            .ok_or_else(|| JsValue::from_str(&format!("wechat wasm: cannot read {path}")))
    }

    fn project_key(text: &str, key: &str) -> Option<String> {
        text.lines().find_map(|line| {
            let line = line.trim().trim_start_matches('\u{feff}');
            line.strip_prefix(&format!("{key} ="))
                .map(|value| value.trim().trim_matches('"').to_owned())
                .filter(|value| !value.trim().is_empty())
        })
    }

    async fn fetch_text(url: &str) -> Result<String, JsValue> {
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
        let text = JsFuture::from(response.text()?).await?;
        Ok(text.as_string().unwrap_or_default())
    }

    fn install_panic_hook() {
        std::panic::set_hook(Box::new(|info| {
            web_sys::console::error_1(&JsValue::from_str(&info.to_string()));
        }));
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn native_stub() {}
