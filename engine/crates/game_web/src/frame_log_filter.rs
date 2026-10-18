//! **每帧日志的过滤器**：哪些 `tracing` 行默认不发。
//!
//! 单独成模块（而不是塞在 `wasm_log` 里）的唯一理由是**它要能在原生单测里跑**：
//! `wasm_log` 依赖 `tracing` 的 wasm 出口，而这里的判据是纯字符串/原子量逻辑——
//! 那正是会写错、且写错之后"控制台又被刷屏"的那部分。
//!
//! 背景（2026-10-17，用户报"这个日志重复打了好多条"）：一次页面加载实测 **654 条**控制台调用，
//! 其中 **540 条**是四条每帧日志——
//!
//! | 条数 | 行 |
//! |---|---|
//! | 236 | `[gpu] presented frames=…` |
//! | 152 | `[render] batch items=…` |
//! | 152 | `[render] mesh vertices=…` |
//! | 63 | `[simulation] fixed update returned: …` |
//!
//! 它们不是"重复"，是每帧一条；但在控制台里就是刷屏，而且会迅速吃光 `wasm_log` 的 400 条上限，
//! 把真正有用的首屏诊断（`[render] font atlas ready` / `[gpu] textures_decoded`）挤掉。
//!
//! 判据用 **(target, 消息前缀)**：这几条的正文每帧都在变（帧号、耗时、顶点数），前缀才是身份；
//! 同时也保护"同 target 下的一次性诊断"不被误伤——`[render] font atlas ready` 与
//! `[render] mesh vertices=` 同 target，只挡后者。同名读数已由宿主聚合成常驻属性
//! （`data-rustlua-engine-host` 的 `draws`/`tris`/`avg_frame_ms`/`sched_frames`），
//! 所以默认挡掉不丢信息；`?rustlua-debug=1`（页面调 `set_frame_logs(true)`）按需打开。

use std::sync::atomic::{AtomicBool, Ordering};

/// 每帧日志的开关。**默认关**：这样"页面忘了设"的后果是"少几行噪声"，而不是"又刷屏"。
static FRAME_LOGS: AtomicBool = AtomicBool::new(false);

/// 一条每帧日志的判据：**消息前缀**（必需）＋ `target`（可选限定，这里全部 `None`）。
///
/// 前缀是**插桩实测**出来的（`.temp/setter-probe.mjs` + `log-probe` feature 打出真实
/// target/message），别再靠眼睛从控制台抄——抄了两次都错：
///
/// | 控制台那一行 | 真实 target | 真实 message 前缀 |
/// |---|---|---|
/// | `[gpu] presented frames=…` | `game` | `presented frames=`（**不带** `[gpu]`，那是拼 target 补的） |
/// | `[render] mesh vertices=…` | `render` | `[render] mesh vertices=`（**自带** `[render]`） |
/// | `[render] batch items=…` | `render` | `[render] batch items=` |
/// | `[simulation] fixed update returned: …` | `game` | `[simulation] fixed update returned:` |
///
/// 前两次踩的坑：① 照控制台文案抄 ⇒ `presented frames=` 抄成 `[gpu] presented frames=`；
/// ② 以为"同一片的 target 才是判据" ⇒ 给 `mesh vertices=` 配 `target: "render"`，而那条 message
/// 里**本来就带** `[render]`，前缀因此对不上。**同一族日志里两种风格并存**（`presented` 不带、
/// `mesh`/`batch` 带），这就是必须实测的原因——也是两条前缀为什么长得不一样。
struct FrameLogRule {
    /// 原始 message 的前缀（`tracing` 里 `{}` 插值之后仍保留字面部分）。
    prefix: &'static str,
    /// 限定 target；`None` = 任何 target 都算。
    target: Option<&'static str>,
}

const FRAME_LOG_RULES: &[FrameLogRule] = &[
    FrameLogRule {
        prefix: "[render] mesh vertices=",
        target: None,
    },
    FrameLogRule {
        prefix: "[render] batch items=",
        target: None,
    },
    FrameLogRule {
        prefix: "presented frames=",
        target: None,
    },
    FrameLogRule {
        prefix: "[simulation] fixed update returned:",
        target: None,
    },
];

/// 是否发每帧日志。
#[must_use]
pub fn frame_logs_enabled() -> bool {
    FRAME_LOGS.load(Ordering::Relaxed)
}

/// 打开/关闭每帧日志（页面在 `init()` 之后、`start()` 之前调用）。
pub fn set_frame_logs_enabled(enabled: bool) {
    FRAME_LOGS.store(enabled, Ordering::Relaxed);
}

/// 这条事件是不是"每帧一条"。
///
/// `message` 传**未加 target 前缀的原始消息**（`wasm_log::event` 在拼前缀之前判）。
#[must_use]
pub fn is_frame_log(target: &str, message: &str) -> bool {
    FRAME_LOG_RULES.iter().any(|rule| {
        message.starts_with(rule.prefix) && rule.target.is_none_or(|expected| expected == target)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 认消息前缀，且**不许误伤**同 target 下的一次性诊断。
    ///
    /// 用例里的 **(target, message) 全部按插桩实测的实况写**——短 target（`game`/`render`）
    /// 与 message 里自带的 `[render]`/`[simulation]` 前缀并存，抄错任一处都会"过滤器看着对、
    /// 跑起来一条不挡"（这个错在本笔里发生了两次）。
    #[test]
    fn frame_log_filter_matches_only_the_per_frame_lines() {
        for (target, message) in [
            ("game", "presented frames=300 sim_steps=1 draw_calls=65"),
            (
                "game",
                "[simulation] fixed update returned: jianghu_youtian_day_1",
            ),
            ("render", "[render] mesh vertices=858 draws=16 paths=16"),
            ("render", "[render] batch items=0 vertices=0 draw_ranges=0"),
        ] {
            assert!(
                is_frame_log(target, message),
                "应当挡掉：{target} / {message}"
            );
        }
        for (target, message) in [
            (
                "render",
                "[render] font atlas ready path=/project/assets/fonts/x.ttf",
            ),
            (
                "render",
                "[render] 2D texture uploaded from memory key=\"a.png\"",
            ),
            (
                "game",
                "[gpu] textures_decoded ok=14 fail=1 png_bytes=57807",
            ),
            ("game", "[gpu] sim_boot_ok"),
            ("render", "[render] font prewarm chunk chunk=1 chunk_ms=3.5"),
            (
                "game",
                "[game] 设备没有存储缓冲（max_storage_buffers_per_shader_stage=0…）",
            ),
        ] {
            assert!(
                !is_frame_log(target, message),
                "不该挡：{target} / {message}"
            );
        }
        // 反向钉子一：控制台里那一行（带 target 前缀）**不是**判据的输入形态。
        assert!(
            !is_frame_log("game", "[gpu] presented frames=300"),
            "过滤器吃的是原始 message，不是控制台文案"
        );
        // 反向钉子二：前缀必须从头匹配，不许把"含这个词"的行也当成每帧日志。
        assert!(!is_frame_log("render", "extra mesh vertices=858"));
    }

    /// 开关默认关，且开/关都生效（原生的静态量是进程级的，所以这条自己复位）。
    #[test]
    fn frame_logs_default_to_off_and_toggle() {
        let initial = frame_logs_enabled();
        assert!(!initial, "默认必须是关：刷屏比少几行更难查");
        set_frame_logs_enabled(true);
        assert!(frame_logs_enabled());
        set_frame_logs_enabled(false);
        assert!(!frame_logs_enabled());
    }
}
