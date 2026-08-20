//! Lightweight runtime performance sampling for the game loop.

use std::time::Instant;

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
    lua_started: Option<Instant>,
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
            lua_started: None,
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

    pub fn begin_lua(&mut self) {
        self.lua_started = Some(Instant::now());
    }

    pub fn end_lua(&mut self) {
        if let Some(started) = self.lua_started.take() {
            self.snapshot.lua_ms += started.elapsed().as_secs_f32() * 1000.0;
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
        stats.begin_lua();
        thread::sleep(Duration::from_millis(1));
        stats.end_lua();
        stats.end_update();
        stats.end_frame();
        let snapshot = stats.snapshot();
        assert!(snapshot.frame_ms > 0.0);
        assert!(snapshot.update_ms > 0.0);
        assert!(snapshot.lua_ms > 0.0);
        assert_eq!(snapshot.frames, 1);
    }

    #[test]
    fn lua_sample_can_be_reset_between_frames() {
        let mut stats = PerformanceStats::new();
        stats.begin_lua();
        stats.end_lua();
        assert!(stats.snapshot().lua_ms >= 0.0);
        stats.reset_lua_sample();
        assert_eq!(stats.snapshot().lua_ms, 0.0);
    }
}
