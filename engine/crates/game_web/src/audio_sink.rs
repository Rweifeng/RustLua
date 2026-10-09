//! wasm 侧的平台音频出口（[`engine::core::audio_sink`] 的实现）。
//!
//! ## 两个平台，一条 Rust 路
//!
//! - **微信小游戏**：`wx.createInnerAudioContext()`（官方推荐，支持流式，适合 BGM 与短音效）。
//!   路径解析问适配层 `__RustLuaWechat.assetSrcFor(...)`——它才知道分包/远端/包内三种形态
//!   该怎么拼。适配层不在（浏览器）或有异常时回落到包内写法。
//! - **浏览器**：`document.createElement('audio')`，路径是"页面根 + `pack/`"，与旧运行时的
//!   图片/文本加载同一套（旧运行时 `browserSrc` 也是这么拼的，扩展名次序也一致）。
//!
//! ## 为什么要"先问 vfs 有没有这个文件"
//!
//! 引擎的播放契约里"缺文件要报错"（`AudioError::Io`），而平台侧加载是**异步**的——
//! 等 `error` 事件回来时 `audio.play()` 早就返回了。所以这里用引擎自己的资源表
//! （`vfs::is_file`，与脚本/贴图同一个 provider）同步判一次存在性，把"缺文件"与
//! "平台播放失败"分开。存在的候选按 `mp3 → wav → ogg` 次序取第一个，与引擎 `sound_bytes`
//! 和旧运行时都一致。
//!
//! ## 为什么把创建出来的元素挂到全局
//!
//! `new Audio()` 创建的元素如果不留引用，会被 GC 掉——表现为"偶发没声音"。挂到
//! `__rustluaAudioElements` 上既是保命，也顺手成了**外部可读的观测点**：
//! CDP 侧数一下这个数组、读一下 `__rustluaAudioErrors`，就能独立于 wasm 自报的数字确认
//! "浏览器真的收到了 N 次播放请求"。自动播放策略拦下 `play()` 的拒绝也会记进错误数组
//! （浏览器里这是最常见的一种"没声音"，必须能被看见）。

use std::cell::RefCell;

use engine::core::audio_sink::{AudioRequest, AudioSinkProvider};
use wasm_bindgen::{JsCast, JsValue};

/// 播放统计：给宿主读数用（**这是 wasm 自报的数字**，外部证据看 `__rustluaAudioElements`）。
#[derive(Default)]
struct Stats {
    plays: u32,
    errors: u32,
    last: String,
    last_src: String,
    branch: &'static str,
    /// 当前 BGM 元素（`document.createElement('audio')` / InnerAudioContext）。
    /// 留住它有两个用处：停/暂停要能找回来，GC 掉则表现为"偶发没声音"。
    music: Option<JsValue>,
    /// BGM 状态：`none` / `playing` / `stopped`（读数里直接可断言）。
    music_state: &'static str,
}

thread_local! {
    static STATS: RefCell<Stats> = RefCell::new(Stats::default());
}

/// 候选扩展名次序：与引擎 `sound_bytes` 和旧运行时 `AUDIO_EXTS` 三方一致。
const EXTS: [&str; 3] = ["mp3", "wav", "ogg"];

/// 浏览器侧音频元素的全局寄存器（保命 + 外部观测点）。
const ELEMENTS: &str = "__rustluaAudioElements";
const ERRORS: &str = "__rustluaAudioErrors";
/// "这个元素是我们主动释放的"标记（旧运行时叫 `__rustlua_released`，同一套做法）。
const RELEASED: &str = "__rustluaReleased";

/// 一次成功播放的结果：走的是哪条平台分支 + 建出来的元素/上下文（BGM 要留引用）。
struct Played {
    branch: &'static str,
    element: JsValue,
}

struct WebAudioSink;

impl AudioSinkProvider for WebAudioSink {
    fn play(&self, request: AudioRequest<'_>) -> Result<(), String> {
        let logical = resolve_logical(&request)?;
        let result = if let Some(wx) = wechat_global() {
            play_wechat(&wx, &logical, request.volume, request.looping)
        } else {
            play_browser(&logical, request.volume, request.looping)
        };
        // 统计必须**整段在 `with` 闭包里**改：`RefMut` 借用的是 thread_local 的短期生命周期，
        // 让它作为返回值逃出闭包编译不过（也本来就不该逃）。
        STATS.with(|stats| {
            let mut stats = stats.borrow_mut();
            stats.last = request.name.to_owned();
            stats.last_src = logical.clone();
            match &result {
                Ok(played) => {
                    stats.plays += 1;
                    stats.branch = played.branch;
                    // BGM：留住元素并记下状态，`stop_music` 才找得回来（读数里也能断言）。
                    // 上一首必须先停：引擎自己的 `play_music` 就是"停掉当前曲目再起新的"
                    // （`audio.rs`），平台出口不照做就会两首 BGM 叠着响——比"没声音"更难查。
                    if request.looping {
                        stop_music_element(&mut stats);
                        stats.music = Some(played.element.clone());
                        stats.music_state = "playing";
                    }
                }
                Err(_) => stats.errors += 1,
            }
        });
        result.map(|_| ())
    }

    /// 停 BGM：暂停 + 摘掉 `src` 并 `load()`（浏览器规范：不摘 src 的话一个正在下载的音频
    /// 不会因为元素被丢弃就停）。这一步不做，"切图后旧 BGM 还在响"就会一直存在。
    fn stop_music(&self) {
        STATS.with(|stats| {
            let mut stats = stats.borrow_mut();
            let had_music = stop_music_element(&mut stats);
            stats.music_state = if had_music { "stopped" } else { "none" };
        });
    }

    fn pause_music(&self) {
        STATS.with(|stats| {
            let stats = stats.borrow();
            if let Some(music) = stats.music.as_ref() {
                call(music, "pause");
            }
        });
    }

    fn resume_music(&self) {
        STATS.with(|stats| {
            let stats = stats.borrow();
            if let Some(music) = stats.music.as_ref() {
                call(music, "play");
            }
        });
    }
}

/// 装平台音频出口（**幂等**，与资源/存档槽同一套理由）。
pub fn install() -> Result<(), String> {
    match engine::core::audio_sink::set_provider(Box::new(WebAudioSink)) {
        Ok(()) => Ok(()),
        Err(error) => {
            web_sys::console::log_1(&JsValue::from_str(&format!(
                "[audio] 平台音频出口已安装（{error}），沿用现有 provider"
            )));
            Ok(())
        }
    }
}

/// 宿主读数（拼进状态属性）：`audio=N last=<名字>@<总线> src=<路径> err=N`。
///
/// `src` 是给"到底找没找到文件"用的：缺文件时它为空，一眼能看出是"没走到平台"还是
/// "平台拒了"。这两个原因在浏览器里长得完全不一样（前者无声无息，后者多半是自动播放策略）。
pub fn note() -> String {
    STATS.with(|stats| {
        let stats = stats.borrow();
        format!(
            "audio={} last={} src={} err={} via={} music={}",
            stats.plays,
            if stats.last.is_empty() {
                "-"
            } else {
                &stats.last
            },
            if stats.last_src.is_empty() {
                "-"
            } else {
                &stats.last_src
            },
            stats.errors,
            if stats.branch.is_empty() {
                "-"
            } else {
                stats.branch
            },
            if stats.music_state.is_empty() {
                "none"
            } else {
                stats.music_state
            },
        )
    })
}

/// 名字 → 资源表的逻辑路径（`assets/audio/<name>.<ext>`，取资源表里存在的第一个）。
fn resolve_logical(request: &AudioRequest<'_>) -> Result<String, String> {
    let Some(dir) = request.asset_dir else {
        return Err("no audio asset directory configured".to_owned());
    };
    // 引擎给的是**平台虚拟根下的绝对路径**（如 `/project/assets/audio`）；资源表里的键是
    // 相对工程根的（`assets/audio/x.mp3`），所以要按工程根裁一刀。
    let dir_rel = dir
        .strip_prefix(crate::VIRTUAL_PROJECT_ROOT)
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| dir.to_string_lossy().replace('\\', "/"));
    for ext in EXTS {
        let logical = format!("{dir_rel}/{}.{ext}", request.name);
        if crate::lua_bridge::resource_exists(&logical) {
            return Ok(logical);
        }
    }
    Err(format!(
        "audio asset not found in resource table: {dir_rel}/{}.{{{}}}",
        request.name,
        EXTS.join(",")
    ))
}

fn global() -> Option<JsValue> {
    web_sys::window().map(JsValue::from)
}

fn define_array(global: &JsValue, name: &str) -> js_sys::Array {
    let existing = js_sys::Reflect::get(global, &JsValue::from_str(name)).ok();
    if let Some(array) = existing.and_then(|value| value.dyn_into::<js_sys::Array>().ok()) {
        return array;
    }
    let array = js_sys::Array::new();
    let _ = js_sys::Reflect::set(global, &JsValue::from_str(name), &array);
    array
}

/// 微信分支：适配层解析路径 + `wx.createInnerAudioContext()`。
fn play_wechat(wx: &JsValue, logical: &str, volume: f32, looping: bool) -> Result<Played, String> {
    let create = js_sys::Reflect::get(wx, &JsValue::from_str("createInnerAudioContext"))
        .ok()
        .filter(|value| value.is_function())
        .ok_or_else(|| "wx.createInnerAudioContext 不可用".to_owned())?;
    let context = js_sys::Function::from(create)
        .call0(wx)
        .map_err(|error| format!("createInnerAudioContext: {}", text(&error)))?;
    let src = wechat_src(logical);
    set(&context, "src", &JsValue::from_str(&src))?;
    // 微信的音量也是 0..1；设失败（老版本只读）不该让整次播放失败。
    let _ = set(&context, "volume", &JsValue::from_f64(f64::from(volume)));
    if looping {
        let _ = set(&context, "loop", &JsValue::TRUE);
    }
    let play = js_sys::Reflect::get(&context, &JsValue::from_str("play"))
        .ok()
        .filter(|value| value.is_function())
        .ok_or_else(|| "InnerAudioContext.play 不可用".to_owned())?;
    js_sys::Function::from(play)
        .call0(&context)
        .map_err(|error| format!("InnerAudioContext.play: {}", text(&error)))?;
    // 挂住它：微信的音频上下文被 GC 掉会直接断音。
    define_array(&global().unwrap_or(JsValue::NULL), ELEMENTS).push(&context);
    attach_wechat_error_log(&context, &src);
    Ok(Played {
        branch: "wx",
        element: context,
    })
}

/// 微信的错误回调：与浏览器的 `catch` 走**同一条**外部可读通道。
///
/// 不挂它，微信侧"没声音"就完全不可见（`InnerAudioContext` 的失败只在 `onError` 里报，
/// 不会抛到调用点）。这一条没有环境验，但接口形状按现有微信 API 写，且与浏览器分支对称。
fn attach_wechat_error_log(context: &JsValue, src: &str) {
    let Some(global) = global() else {
        return;
    };
    let Ok(on_error) = js_sys::Reflect::get(context, &JsValue::from_str("onError")) else {
        return;
    };
    if !on_error.is_function() {
        return;
    }
    let errors = define_array(&global, ERRORS);
    let src = src.to_owned();
    let callback =
        wasm_bindgen::closure::Closure::<dyn FnMut(JsValue)>::new(move |error: JsValue| {
            let message = js_sys::Reflect::get(&error, &JsValue::from_str("errMsg"))
                .ok()
                .and_then(|value| value.as_string())
                .unwrap_or_else(|| text(&error));
            errors.push(&JsValue::from_str(&format!("{src}: {message}")));
        });
    let _ = js_sys::Function::from(on_error).call1(context, callback.as_ref().unchecked_ref());
    // 回调由页面持有：不 forget 会立刻 drop（之后再也不会触发）。
    callback.forget();
}

/// 微信路径解析：优先问适配层（只有它知道分包/远端/包内该怎么拼）。
fn wechat_src(logical: &str) -> String {
    let packed = format!("pack/{logical}");
    let Some(global) = global() else {
        return packed;
    };
    let Ok(adapter) = js_sys::Reflect::get(&global, &JsValue::from_str("__RustLuaWechat")) else {
        return packed;
    };
    let Ok(route) = js_sys::Reflect::get(&adapter, &JsValue::from_str("assetSrcFor")) else {
        return packed;
    };
    if !route.is_function() {
        return packed;
    }
    match js_sys::Function::from(route).call1(&adapter, &JsValue::from_str(&packed)) {
        Ok(value) => value.as_string().unwrap_or(packed),
        Err(_) => packed,
    }
}

/// 浏览器分支：`pack/<logical>` 交给 `HTMLAudioElement`。
fn play_browser(logical: &str, volume: f32, looping: bool) -> Result<Played, String> {
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| "没有 document（不在浏览器里？）".to_owned())?;
    let element = document
        .create_element("audio")
        .map_err(|error| format!("createElement(audio): {}", text(&error)))?
        .dyn_into::<web_sys::HtmlAudioElement>()
        .map_err(|_| "createElement(audio) 不是 HTMLAudioElement".to_owned())?;
    // 与旧运行时同一套 URL：页面根 + `pack/`。空格等字符要做百分号编码（旧运行时用 encodeURI）。
    element.set_src(&encode_uri(&format!("pack/{logical}")));
    element.set_volume(f64::from(volume.clamp(0.0, 1.0)));
    element.set_loop(looping);
    let global = global().unwrap_or(JsValue::NULL);
    define_array(&global, ELEMENTS).push(&element);
    // `play()` 返回 Promise：异步失败（最常见的是自动播放策略 NotAllowedError）不在这里抛，
    // 所以挂一个 catch 把原因记进错误数组——外部探针读它，就能看见"为什么没声音"。
    match element.play() {
        Ok(promise) => attach_error_log(&promise, &element, logical),
        Err(error) => return Err(format!("audio.play(): {}", text(&error))),
    }
    Ok(Played {
        branch: "web",
        element: element.into(),
    })
}

/// 挂 `play()` 的失败回调，把原因记进外部可读的错误数组。
///
/// **故意停掉/换掉的元素要豁免**：我们主动 `pause()` + 摘 `src` 时，浏览器会把那次未完成的
/// `play()` 以 `AbortError: The play() request was interrupted by a call to pause()` 拒掉。
/// 那不是故障（旧运行时为同一件事也立过"已释放"标记，否则被淘汰的音效会被误记成"文件不存在"）。
/// 不豁免的话，错误数组里混进噪声，`errors=0` 就失去意义——**真正的**自动播放策略拒绝
/// （`NotAllowedError`）会被淹掉。
fn attach_error_log(promise: &JsValue, element: &web_sys::HtmlAudioElement, logical: &str) {
    let Some(global) = global() else {
        return;
    };
    let errors = define_array(&global, ERRORS);
    let logical = logical.to_owned();
    let element = element.clone();
    let catch = wasm_bindgen::closure::Closure::<dyn FnMut(JsValue)>::new(move |error: JsValue| {
        if released(&element) {
            return;
        }
        errors.push(&JsValue::from_str(&format!("{logical}: {}", text(&error))));
    });
    let promise = js_sys::Promise::from(promise.clone());
    let _ = promise.catch(&catch);
    // 回调由页面持有：不 forget 会立刻 drop（catch 之后再也不会触发）。
    catch.forget();
}

/// 元素是否已被我们主动释放（读页面上的标记属性，与旧运行时的 `__rustlua_released` 同一套做法）。
fn released(element: &web_sys::HtmlAudioElement) -> bool {
    js_sys::Reflect::get(element.as_ref(), &JsValue::from_str(RELEASED))
        .map(|value| value.is_truthy())
        .unwrap_or(false)
}

fn wechat_global() -> Option<JsValue> {
    let global = global()?;
    let wx = js_sys::Reflect::get(&global, &JsValue::from_str("wx")).ok()?;
    if wx.is_undefined() || wx.is_null() {
        return None;
    }
    // 微信环境里 `wx` 一定有 createInnerAudioContext；浏览器里没有 `wx` 这个全局。
    let create = js_sys::Reflect::get(&wx, &JsValue::from_str("createInnerAudioContext")).ok()?;
    create.is_function().then_some(wx)
}

/// 停掉并释放当前 BGM 元素，返回"本来有没有在播的 BGM"。
///
/// 摘 `src` 之后还要 `load()`：浏览器规范里，正在下载的音频不会仅仅因为元素被丢弃就停。
fn stop_music_element(stats: &mut Stats) -> bool {
    let Some(music) = stats.music.take() else {
        return false;
    };
    // 先立"已释放"标记再动手：下面 pause() 会让那次未完成的 play() 以 AbortError 被拒，
    // 那是我们自己造成的，不该进错误数组。
    let _ = set(&music, RELEASED, &JsValue::TRUE);
    call(&music, "pause");
    let _ = set(&music, "src", &JsValue::from_str(""));
    call(&music, "load");
    true
}

/// 调一个可能不存在的方法（跨平台属性/方法名不完全一致，缺了就静默跳过）。
fn call(target: &JsValue, method: &str) {
    if let Ok(value) = js_sys::Reflect::get(target, &JsValue::from_str(method))
        && value.is_function()
    {
        let _ = js_sys::Function::from(value).call0(target);
    }
}

fn set(target: &JsValue, key: &str, value: &JsValue) -> Result<(), String> {
    js_sys::Reflect::set(target, &JsValue::from_str(key), value)
        .map(|_| ())
        .map_err(|error| format!("set {key}: {}", text(&error)))
}

fn text(value: &JsValue) -> String {
    value.as_string().unwrap_or_else(|| format!("{value:?}"))
}

/// 与旧运行时同规则的百分号编码：只处理会破坏 URL 的字符（资源名都是 ASCII）。
fn encode_uri(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b'/'
            | b':'
            | b'?'
            | b'&'
            | b'='
            | b'#'
            | b'%'
            | b'@'
            | b'+'
            | b'$'
            | b','
            | b';' => {
                out.push(char::from(byte));
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
