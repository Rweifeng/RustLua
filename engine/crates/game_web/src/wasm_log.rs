//! wasm 侧的 `tracing` 出口：把引擎的诊断行送到页面控制台。
//!
//! **为什么需要**：引擎里所有诊断都走 `tracing`（字体 `target: "render"`、音频 `"audio"`、
//! 资源 `"assets"`…），而 wasm 宿主此前**没有装订阅者**——那些行在桌面上有、在浏览器里被静默丢掉。
//! 上一轮查字体时就吃过这个亏：`[render] font atlas ready` 是现成的结论，却看不见，
//! 只能靠 DOM 属性反推"到底走到哪一步"。
//!
//! 两条约束，都不是可选的：
//! 1. **幂等**：`set_global_default` 第二次必然失败（vfs provider 同一套道理），"已装"当成功；
//! 2. **有上限**：引擎里存在每帧都会打的日志，无上限转发会把控制台和启动耗时一起拖垮。
//!    到顶之后就只报一次"已丢弃 N 条"，不再逐条转发。
//!
//! **每帧日志默认不发**（2026-10-17，用户报"日志重复打了好多条"）：一次加载实测 654 条控制台调用，
//! 其中 540 条是三条每帧日志（`[gpu] presented …` 236、`[render] batch …` 152、`[render] mesh …` 152）。
//! 它们不是"重复"，而是每帧一条——但在控制台里就是刷屏，而且会很快吃光下面的 400 条上限
//! （首屏诊断因此被挤掉）。这三条的同名读数**已经由宿主聚合成常驻属性**
//! （`data-rustlua-engine-host` 的 `draws`/`tris`/`avg_frame_ms`/`sched_frames`），
//! 所以默认挡掉、由页面用 `?rustlua-debug=1`（= `set_frame_logs(true)`）按需打开是安全的：
//! 打开的路径与关掉之前逐字相同。
//!
//! 只实现 `Subscriber` 里真正需要的那几个方法，其余用空实现：这里的目标是"看得见"，
//! 不是复刻 `tracing-subscriber` 的 span/过滤器语义。

use std::sync::atomic::{AtomicU64, Ordering};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Metadata, Subscriber};
use wasm_bindgen::prelude::*;

/// 转发上限：到顶后只留一条"已丢弃"提示。
const MAX_EVENTS: u64 = 400;

/// 每帧一行的日志（**默认挡掉**）：`(target, 消息前缀)`。
///
/// 用消息前缀而不是整串做键：这几条日志的正文每帧都在变（帧号/耗时），前缀才是身份。
///
/// 放在**原生也编得到**的位置（`crate::frame_log_filter`）：这层判据是纯字符串逻辑，
/// 而 `windows` 那组门跑不到 wasm 的控制台路径——放这里才有回归守卫。
use crate::frame_log_filter::{frame_logs_enabled, is_frame_log, set_frame_logs_enabled};

/// 打开/关闭每帧日志（页面在 `init()` 之后、`start()` 之前调用；不调用 = 关）。
#[wasm_bindgen]
pub fn set_frame_logs(enabled: bool) {
    set_frame_logs_enabled(enabled);
}

static EMITTED: AtomicU64 = AtomicU64::new(0);
static DROPPED_REPORTED: AtomicU64 = AtomicU64::new(0);

/// 安装控制台订阅者（幂等；重复调用不是错误）。
pub fn install() {
    // 返回值只有"已经有全局订阅者"一种失败可能——沿用现有订阅者即可。
    let _ = tracing::subscriber::set_global_default(ConsoleSubscriber);
}

struct ConsoleSubscriber;

/// 把一条事件的字段拼成 `键=值 键=值`（`message` 放最前，保持与桌面日志同一读法）。
#[derive(Default)]
struct FieldCollector {
    message: String,
    rest: Vec<String>,
}

impl Visit for FieldCollector {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
            // `message` 是 `&str` 时 `Debug` 会带上引号；去掉它，读起来与桌面一致。
            if self.message.len() >= 2
                && self.message.starts_with('"')
                && self.message.ends_with('"')
            {
                self.message = self.message[1..self.message.len() - 1].to_owned();
            }
        } else {
            self.rest.push(format!("{}={value:?}", field.name()));
        }
    }
}

impl Subscriber for ConsoleSubscriber {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        // 每帧一行的调度明细（`target: "scene"` 的 DEBUG）在控制台里是纯噪声，而且会很快
        // 吃光下面的上限——实测：400 条上限下只剩 28 行样本，且全落在最早的几十帧。
        // 同一份数据已经由宿主聚合成常驻读数（`sched_frames=… top_ms=[…]`），所以这里挡掉。
        !(metadata.level() == &Level::DEBUG && metadata.target() == "scene")
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut collector = FieldCollector::default();
        event.record(&mut collector);
        let target = event.metadata().target();
        // 每帧那几条在这里挡掉（实测条数与真实文案见 `crate::frame_log_filter` 的模块头）：
        // 判据是**原始 message 前缀**，正文每帧都在变；`?rustlua-debug=1` 时整条判据短路。
        // 判在前缀拼接**之前**：`line` 随后要拼 target 前缀，这里只借用原始消息。
        if !frame_logs_enabled() && is_frame_log(target, &collector.message) {
            return;
        }
        let mut line = collector.message;
        if !collector.rest.is_empty() {
            line.push(' ');
            line.push_str(&collector.rest.join(" "));
        }
        // 引擎的日志约定是"消息自带 `[render]`/`[audio]` 这类前缀"，所以只有消息没带前缀时
        // 才补上 target——否则控制台里会出现 `[render] [render] ...` 这种重复。
        let line = if target.is_empty() || line.starts_with('[') {
            line
        } else {
            format!("[{target}] {line}")
        };

        let count = EMITTED.fetch_add(1, Ordering::Relaxed);
        if count >= MAX_EVENTS {
            // 只报一次：剩下的只计数，避免"日志刷屏"变成新的性能问题。
            if DROPPED_REPORTED.swap(1, Ordering::Relaxed) == 0 {
                web_sys::console::warn_1(&wasm_bindgen::JsValue::from_str(&format!(
                    "[wasm-log] 已丢弃后续 tracing 事件（上限 {MAX_EVENTS} 条）：引擎里可能有每帧日志"
                )));
            }
            return;
        }

        let value = wasm_bindgen::JsValue::from_str(&line);
        match *event.metadata().level() {
            Level::ERROR => web_sys::console::error_1(&value),
            Level::WARN => web_sys::console::warn_1(&value),
            _ => web_sys::console::log_1(&value),
        }
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}
