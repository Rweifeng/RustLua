//! WebGL/微信 wasm 核心的**宿主 API 编译进 wasm**（WP10 批次 A）。
//!
//! 背景：桌面引擎的 36 个宿主全局里，17 个在浏览器端缺失（docs/32 §4），由
//! `__rustlua_missing_host_global` 代理占位。WP10 方案①把宿主 API 用**纯 Rust** 实现进
//! 本 wasm 核心（wasm-bindgen 导出），fengari Lua 运行时经 `web-runtime.js` 桥接调用。
//! 本模块是**批次 A**：`app / clock / calendar / day`（纯状态、无 IO、无游戏数据）。
//!
//! 语义与桌面 `rustlua_gameplay::gameplay::GameClock` / `rustlua_scripting::script` 逐项对齐：
//! - 分钟/天：`MINUTES_PER_DAY = 1440`、清晨 `DEFAULT_WAKE_MINUTE = 360`（06:00）；
//! - 日历：28 天/季 × 4 季/年（`DAYS_PER_YEAR = 112`），season/year/day_of_season 同公式；
//! - `clock.now()` 返回 day/minute/hour/minute_of_day/rate/paused；
//! - `calendar.now()` 返回 absolute_day/day/season/year/minute；`calendar.set` 校验同桌面；
//! - `day.sleep()` 的 `on_day_end`/`on_day_start` 钩子是 fengari Lua 函数，由 JS 桥在
//!   调用 `clock_sleep` 前后触发（本模块只提供时钟状态与跨天判定）。
//!
//! 失败约定：参数非法时返回 `false`/默认值并记录原因（`last_error`），与桌面
//! `api_error` 抛错的**行为等价**——JS 桥据此把错误转成 Lua error 或静默默认值。

use js_sys::Object;
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::*;

/// Minutes in one in-game day（与 `gameplay::MINUTES_PER_DAY` 一致）。
const MINUTES_PER_DAY: u32 = 24 * 60;
/// Default wake-up minute（06:00，与 `gameplay::DEFAULT_WAKE_MINUTE` 一致）。
const DEFAULT_WAKE_MINUTE: u32 = 6 * 60;
/// Days per season（28，与 `gameplay::DAYS_PER_SEASON` 一致）。
const DAYS_PER_SEASON: u32 = 28;
/// Seasons per year（4）。
const SEASONS_PER_YEAR: u32 = 4;
/// Total days per year（28 × 4 = 112）。
const DAYS_PER_YEAR: u32 = DAYS_PER_SEASON * SEASONS_PER_YEAR;

/// 独立于墙钟的引擎游戏时钟（语义镜像 `gameplay::GameClock`）。
#[derive(Clone, Copy, Debug)]
struct HostClock {
    /// 1-based in-game day。
    day: u32,
    /// Minute of day in `0..MINUTES_PER_DAY`。
    minute: u32,
    /// Game minutes advanced per real second；`0.0` 表示不自动推进。
    rate: f64,
    paused: bool,
}

impl Default for HostClock {
    fn default() -> Self {
        Self {
            day: 1,
            minute: DEFAULT_WAKE_MINUTE,
            rate: 0.0,
            paused: false,
        }
    }
}

impl HostClock {
    /// 分钟推进（含跨天进位；负数钳到 0 分）。
    fn add_minutes(&mut self, minutes: f64) {
        if !minutes.is_finite() {
            return;
        }
        let total = self.minute as f64 + minutes;
        if total < 0.0 {
            self.minute = 0;
            return;
        }
        let total_minutes = total as u32;
        self.day += total_minutes / MINUTES_PER_DAY;
        self.minute = total_minutes % MINUTES_PER_DAY;
    }

    /// 设置绝对天与当日分钟。
    fn set(&mut self, day: u32, minute: u32) {
        self.day = day.max(1);
        self.minute = minute.min(MINUTES_PER_DAY - 1);
    }

    /// 睡到下一个清晨，返回新的一天。
    fn sleep_to_morning(&mut self) -> u32 {
        self.day += 1;
        self.minute = DEFAULT_WAKE_MINUTE;
        self.day
    }

    /// 1-based season（`1..=4`）。
    fn season(&self) -> u32 {
        ((self.day - 1) / DAYS_PER_SEASON) % SEASONS_PER_YEAR + 1
    }

    /// 1-based year。
    fn year(&self) -> u32 {
        (self.day - 1) / DAYS_PER_YEAR + 1
    }

    /// 1-based day within season（`1..=28`）。
    fn day_of_season(&self) -> u32 {
        (self.day - 1) % DAYS_PER_SEASON + 1
    }

    /// 按绝对天/分钟设置（`calendar.set` 的换算目标）。
    fn set_absolute(&mut self, day: u32, minute: u32) {
        self.day = day.max(1);
        self.minute = minute.min(MINUTES_PER_DAY - 1);
    }
}

fn set_field(object: &Object, key: &str, value: &JsValue) {
    // 构造期设置固定字段，失败不可恢复——用 unchecked 反射（字段名与类型都是静态已知的）。
    let _ = js_sys::Reflect::set(object, &JsValue::from_str(key), value);
}

fn obj(entries: &[(&str, JsValue)]) -> JsValue {
    let object = Object::new();
    for (key, value) in entries {
        set_field(&object, key, value);
    }
    JsValue::from(object)
}

/// 批次 A 宿主 API：`app / clock / calendar / day`（WP10）。
///
/// JS 侧用法：`new HostApi(publish_name)` → 把方法挂到 fengari 全局表。
#[wasm_bindgen]
pub struct HostApi {
    clock: HostClock,
    name: String,
    last_error: String,
}

#[wasm_bindgen]
impl HostApi {
    #[wasm_bindgen(constructor)]
    pub fn new(name: String) -> HostApi {
        Self {
            clock: HostClock::default(),
            name,
            last_error: String::new(),
        }
    }

    // ---- app ----

    /// `app.name()`：发布名。
    pub fn app_name(&self) -> String {
        self.name.clone()
    }

    /// `app.platform()`：固定 `"webgl"`（web/微信两条路径共用同一运行时）。
    pub fn app_platform(&self) -> String {
        "webgl".to_owned()
    }

    /// `app.project_root()`：浏览器没有文件系统，恒返回空串（桌面有文件系统，
    /// 返回绝对路径；Lua 侧按空串自行跳过——语义与桌面一致）。
    pub fn app_project_root(&self) -> String {
        String::new()
    }

    /// `app.quit()`：浏览器端无进程可退；记录一次调用（JS 桥可据 last_error 提示）。
    pub fn app_quit(&mut self) {
        self.last_error = "app.quit: no process to quit in web runtime (no-op)".to_owned();
    }

    // ---- clock ----

    /// `clock.now()` → { day, minute, hour, minute_of_day, rate, paused }。
    pub fn clock_now(&self) -> JsValue {
        let c = &self.clock;
        obj(&[
            ("day", JsValue::from(c.day as f64)),
            ("minute", JsValue::from(c.minute as f64)),
            ("hour", JsValue::from((c.minute / 60) as f64)),
            ("minute_of_day", JsValue::from(c.minute as f64)),
            ("rate", JsValue::from(c.rate)),
            ("paused", JsValue::from(c.paused)),
        ])
    }

    /// `clock.set(day, minute)`；非法参数返回 false 并记录原因（桌面抛错）。
    pub fn clock_set(&mut self, day: i32, minute: i32) -> bool {
        if day < 1 || minute < 0 {
            self.last_error = "clock.set day must be >= 1 and minute >= 0".to_owned();
            return false;
        }
        self.clock
            .set(day as u32, (minute as u32).min(MINUTES_PER_DAY - 1));
        true
    }

    /// `clock.set_rate(rate)`；非法（NaN/负数）返回 false。
    pub fn clock_set_rate(&mut self, rate: f64) -> bool {
        if !rate.is_finite() || rate < 0.0 {
            self.last_error = "clock.set_rate must be finite and non-negative".to_owned();
            return false;
        }
        self.clock.rate = rate;
        true
    }

    /// `clock.pause(paused)`。
    pub fn clock_pause(&mut self, paused: bool) {
        self.clock.paused = paused;
    }

    /// `clock.add(minutes)`；NaN 返回 false。
    pub fn clock_add(&mut self, minutes: f64) -> bool {
        if !minutes.is_finite() {
            self.last_error = "clock.add minutes must be finite".to_owned();
            return false;
        }
        self.clock.add_minutes(minutes);
        true
    }

    /// `clock.sleep_to_morning()` → 新的一天。
    pub fn clock_sleep_to_morning(&mut self) -> i32 {
        self.clock.sleep_to_morning() as i32
    }

    // ---- calendar ----

    /// `calendar.now()` → { absolute_day, day, season, year, minute }。
    pub fn calendar_now(&self) -> JsValue {
        let c = &self.clock;
        obj(&[
            ("absolute_day", JsValue::from(c.day as f64)),
            ("day", JsValue::from(c.day_of_season() as f64)),
            ("season", JsValue::from(c.season() as f64)),
            ("year", JsValue::from(c.year() as f64)),
            ("minute", JsValue::from(c.minute as f64)),
        ])
    }

    /// `calendar.set(day, season, year, minute)`；范围校验同桌面，失败返回 false。
    pub fn calendar_set(&mut self, day: i32, season: i32, year: i32, minute: i32) -> bool {
        if day < 1 || day > DAYS_PER_SEASON as i32 {
            self.last_error = format!("calendar.set day must be in 1..={DAYS_PER_SEASON}");
            return false;
        }
        if season < 1 || season > SEASONS_PER_YEAR as i32 {
            self.last_error = format!("calendar.set season must be in 1..={SEASONS_PER_YEAR}");
            return false;
        }
        if year < 1 {
            self.last_error = "calendar.set year must be >= 1".to_owned();
            return false;
        }
        let absolute =
            (year as u32 - 1) * DAYS_PER_YEAR + (season as u32 - 1) * DAYS_PER_SEASON + day as u32;
        self.clock.set_absolute(absolute, minute.max(0) as u32);
        true
    }

    // ---- day ----

    /// `day.now()` → { day, minute }。
    pub fn day_now(&self) -> JsValue {
        obj(&[
            ("day", JsValue::from(self.clock.day as f64)),
            ("minute", JsValue::from(self.clock.minute as f64)),
        ])
    }

    /// `day.sleep()` 的时钟部分：睡到清晨并返回新的一天。
    /// `on_day_end` / `on_day_start` 钩子由 JS 桥在调用前后触发（fengari 函数）。
    pub fn day_sleep(&mut self) -> i32 {
        self.clock.sleep_to_morning() as i32
    }

    // ---- diagnostics ----

    /// 最近一次非法参数的原因（空串表示无）。
    pub fn last_error(&self) -> String {
        self.last_error.clone()
    }
}
