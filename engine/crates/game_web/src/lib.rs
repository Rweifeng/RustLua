#[cfg(target_arch = "wasm32")]
mod wasm {
    use wasm_bindgen::{JsCast, prelude::*};
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{HtmlCanvasElement, Request, RequestInit, RequestMode, Response};

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
        canvas.set_width(1280);
        canvas.set_height(720);
        let manifest = fetch_text("pack/manifest.txt").await?;
        let project = fetch_text("pack/project.toml").await?;
        let startup_scene = project_key(&project, "startup_scene")
            .unwrap_or_else(|| "scenes/main.scene".to_owned());
        let scene = fetch_text(&format!("pack/{startup_scene}")).await?;
        web_sys::console::log_1(&JsValue::from_str(&format!(
            "P2 WebGL/WASM started: {title}; project bytes={}; manifest files={}; startup_scene={}; scene bytes={}",
            project.len(),
            manifest.lines().skip(1).count(),
            startup_scene,
            scene.len()
        )));
        Ok(())
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
