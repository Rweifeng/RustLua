//! Lightweight runtime performance sampling for the game loop.

// 时钟必须走 `web-time`：`wasm32-unknown-unknown` 的 std 把 `Instant::now()` 实现成 panic
// （`sys/pal/unsupported/time.rs`）。这份采样器是游戏主循环建的，wasm 上只要建一次就会炸。
// 原生上 `web-time::Instant` 就是 `std::time::Instant` 的再导出，零行为变化。
use web_time::Instant;

/// Latest rolling performance values exposed to the debug overlay.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerformanceSnapshot {
    pub fps: f32,
    pub frame_ms: f32,
    pub update_ms: f32,
    pub lua_ms: f32,
    pub frames: u64,
}

/// Frame/update timer with a one-second rolling display window.
#[derive(Debug)]
pub struct PerformanceStats {
    frame_started: Instant,
    update_started: Option<Instant>,
    window_started: Instant,
    window_frames: u32,
    snapshot: PerformanceSnapshot,
}

impl PerformanceStats {
    #[must_use]
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            frame_started: now,
            update_started: None,
            window_started: now,
            window_frames: 0,
            snapshot: PerformanceSnapshot {
                fps: 0.0,
                frame_ms: 0.0,
                update_ms: 0.0,
                lua_ms: 0.0,
                frames: 0,
            },
        }
    }

    pub fn begin_frame(&mut self) {
        self.frame_started = Instant::now();
    }

    pub fn begin_update(&mut self) {
        self.update_started = Some(Instant::now());
    }

    pub fn end_update(&mut self) {
        if let Some(started) = self.update_started.take() {
            self.snapshot.update_ms = started.elapsed().as_secs_f32() * 1000.0;
        }
    }

    pub fn end_frame(&mut self) {
        self.snapshot.frame_ms = self.frame_started.elapsed().as_secs_f32() * 1000.0;
        self.snapshot.frames += 1;
        self.window_frames += 1;
        let elapsed = self.window_started.elapsed().as_secs_f32();
        if elapsed >= 1.0 {
            self.snapshot.fps = self.window_frames as f32 / elapsed;
            self.window_frames = 0;
            self.window_started = Instant::now();
        }
    }

    /// 记账一次 Lua `update()` 的耗时（毫秒）。调用方是宿主：
    /// `LuaRuntime::last_update_micros()` 在 Lua 体内部测量，宿主把它送进来累加。
    /// 早先的 `begin_lua/end_lua` 只能测"两次调用之间的墙钟"，跨不过
    /// `RuntimeScene` 边界，结果真实运行里 `lua_ms` 恒为 `0.000`（P4 接线）。
    pub fn record_lua_millis(&mut self, millis: f32) {
        self.snapshot.lua_ms += millis;
    }

    #[must_use]
    pub const fn snapshot(&self) -> PerformanceSnapshot {
        self.snapshot
    }

    pub fn reset_lua_sample(&mut self) {
        self.snapshot.lua_ms = 0.0;
    }
}

impl Default for PerformanceStats {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::PerformanceStats;
    use std::{thread, time::Duration};

    #[test]
    fn records_frame_update_and_lua_samples() {
        let mut stats = PerformanceStats::new();
        stats.begin_frame();
        stats.begin_update();
        thread::sleep(Duration::from_millis(1));
        stats.record_lua_millis(0.75);
        stats.record_lua_millis(0.25);
        stats.end_update();
        stats.end_frame();
        let snapshot = stats.snapshot();
        assert!(snapshot.frame_ms > 0.0);
        assert!(snapshot.update_ms > 0.0);
        assert!((snapshot.lua_ms - 1.0).abs() < f32::EPSILON);
        assert_eq!(snapshot.frames, 1);
    }

    #[test]
    fn lua_sample_can_be_reset_between_frames() {
        let mut stats = PerformanceStats::new();
        stats.record_lua_millis(0.4);
        assert!(stats.snapshot().lua_ms > 0.0);
        stats.reset_lua_sample();
        // 每帧清零：`lua_ms` 是"本帧 Lua 耗时"，不是累计值（累计值会让面板
        // 显示一个只增不减的数，看不出哪一帧卡）。
        assert_eq!(stats.snapshot().lua_ms, 0.0);
    }
}
