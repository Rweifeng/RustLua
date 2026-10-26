//! 宿主侧的**脚本热重载节律**与结果文案（编辑器 Play 与桌面 `game` 共用一份）。
//!
//! 为什么需要这一层：引擎早就有 `LuaRuntime::reload_if_changed` / `RuntimeScene::reload_scripts`
//! （候选 VM 先立、旧 VM 后破，语法错保旧状态），但两个宿主都没有调用点——"改完 Lua 要 Stop
//! 再 Play"于是变成一条**能力已具备、入口不存在**的体验断层。Shader 热重载走的是同一条形状
//! （去抖轮询 + 一次一份结果，见 `rustlua-gpu::shader_library::SHADER_RELOAD_DEBOUNCE`），
//! 这里把那个形状抽成可单测的纯逻辑，避免两个宿主各写一遍、各自漂移。
//!
//! 刻意**不持有时钟**：调用方每帧把"距上一次的间隔"喂进来。于是这条节律在 wasm32 上也能编译
//! （std 的 `Instant::now()` 在那里是 panic），且单测不需要等真实时间。

use std::time::Duration;

/// 脚本热重载的轮询节奏。
///
/// 比 shader 的 150ms 略宽：一次保存 Lua 文件同样会先截断再写入，而一次 `reload_scripts`
/// 要重建**工程级 + 全部实体脚本**的候选状态，比编译一份 shader 更贵；250ms 既盖得住
/// 编辑器的原子写（先写临时文件再 rename，通常一次就读到终态），也不会让"改完等半天"。
pub const SCRIPT_RELOAD_INTERVAL: Duration = Duration::from_millis(250);

/// 一次重载结果在画面上停留的帧数（按宿主的节流节奏折算，编辑器 ≈ 2 秒）。
pub const SCRIPT_NOTICE_FRAMES: u32 = 120;

/// 去抖节律：每 [`SCRIPT_RELOAD_INTERVAL`] 放行一次检查。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReloadCadence {
    interval: Duration,
    elapsed: Duration,
    /// `true` = 下一次 `advance` 放行（首帧立即检查一次，之后按间隔）。
    ready: bool,
}

impl ReloadCadence {
    /// 新建节律：首帧立即放行一次（把启动前就已发生的磁盘改动读到），随后按 `interval` 去抖。
    #[must_use]
    pub const fn new(interval: Duration) -> Self {
        Self {
            interval,
            elapsed: Duration::ZERO,
            ready: true,
        }
    }

    /// 默认脚本节律（[`SCRIPT_RELOAD_INTERVAL`]）。
    #[must_use]
    pub const fn script_default() -> Self {
        Self::new(SCRIPT_RELOAD_INTERVAL)
    }

    /// 推进一个帧间隔，返回本帧**是否**该做检查。
    ///
    /// 放行后内部计时归零：一次慢帧（比如切标签页回来）不会连着放行多次检查。
    pub fn advance(&mut self, frame_elapsed: Duration) -> bool {
        if self.ready {
            self.ready = false;
            self.elapsed = Duration::ZERO;
            return true;
        }
        self.elapsed += frame_elapsed;
        if self.elapsed >= self.interval {
            self.elapsed = Duration::ZERO;
            return true;
        }
        false
    }
}

/// 一次 `RuntimeScene::reload_scripts` 的结果，折成宿主可直接显示的一行。
///
/// 返回 `None` 表示**什么都没变**：这类"Unchanged"绝不能上屏，否则 overlay 会被一条
/// 永不自愈的"已热重载 0 个文件"糊住。
#[must_use]
pub fn script_reload_note(project_reloaded: bool, entity_files: usize) -> Option<String> {
    match (project_reloaded, entity_files) {
        (false, 0) => None,
        (true, 0) => Some("Lua 脚本已热重载：工程级入口".to_owned()),
        (false, files) => Some(format!("Lua 脚本已热重载：{files} 个实体脚本")),
        (true, files) => Some(format!("Lua 脚本已热重载：工程级入口 + {files} 个实体脚本")),
    }
}

/// 重载失败时的常驻提示。
///
/// 口径与引擎侧一致：**旧 VM 原封不动**，Play 继续跑改坏之前的那份逻辑。这句话必须写出来，
/// 因为容错档下"游戏还在动"很容易被读成"我刚才那次保存生效了"。
#[must_use]
pub fn script_reload_failure_note(error: &str) -> String {
    format!("Lua 热重载失败（旧脚本继续运行）：{error}")
}

#[cfg(test)]
mod tests {
    use super::{
        ReloadCadence, SCRIPT_RELOAD_INTERVAL, script_reload_failure_note, script_reload_note,
    };
    use std::time::Duration;

    const FRAME: Duration = Duration::from_millis(16);

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn first_check_is_immediate_then_debounced() {
        let mut cadence = ReloadCadence::script_default();
        assert!(cadence.advance(Duration::ZERO), "首帧必须放行一次检查");
        // 250ms 窗口内连续 60fps 的帧都不该再检查。
        for _ in 0..10 {
            assert!(!cadence.advance(FRAME));
        }
        assert!(
            cadence.advance(ms(SCRIPT_RELOAD_INTERVAL.as_millis() as u64)),
            "跨过窗口必须放行"
        );
        assert!(!cadence.advance(FRAME), "放行后计时要归零");
    }

    #[test]
    fn slow_frame_does_not_fire_a_burst() {
        // 切标签页回来一次喂 3 秒：只放行一次，不能把欠的都"补检查"。
        let mut cadence = ReloadCadence::new(ms(100));
        assert!(cadence.advance(Duration::ZERO));
        assert!(cadence.advance(ms(3_000)));
        assert!(!cadence.advance(Duration::ZERO));
        assert!(!cadence.advance(ms(99)));
        assert!(cadence.advance(ms(1)));
    }

    #[test]
    fn unchanged_reports_nothing_and_each_change_reports_its_own_scope() {
        assert_eq!(script_reload_note(false, 0), None, "没变不许上屏");
        assert_eq!(
            script_reload_note(true, 0).as_deref(),
            Some("Lua 脚本已热重载：工程级入口")
        );
        assert_eq!(
            script_reload_note(false, 3).as_deref(),
            Some("Lua 脚本已热重载：3 个实体脚本")
        );
        let both = script_reload_note(true, 2).unwrap_or_default();
        assert!(both.contains("工程级入口") && both.contains("2"), "{both}");
    }

    #[test]
    fn failure_note_claims_only_what_the_engine_guarantees() {
        let note = script_reload_failure_note("syntax error near 'end'");
        assert!(note.starts_with("Lua 热重载失败"), "{note}");
        assert!(
            note.contains("旧脚本继续运行"),
            "必须说清旧 VM 还在跑：{note}"
        );
        assert!(note.contains("syntax error"), "{note}");
    }
}
