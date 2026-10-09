//! GPU 时间戳查询的槽位与状态（WP12 阶段 4a 从 `renderer/mod.rs` 原样搬出）：
//! `TimestampSlot` / `GpuTimestampState` 两个类型，以及 `impl Renderer` 上读写它们的几条方法。
//!
//! 这两个类型是 `Renderer` 的字段类型，`mod.rs` 要按短名写进结构体 ⇒ 放宽为 `pub(super)`
//! （计划 §3.1.4）；`Renderer` 本身仍定义在 `mod.rs`——固有 `impl` 块分散在同 crate 的兄弟
//! 模块即可，字段一个都不用动。

use super::Renderer;
use web_time::Instant;

/// One asynchronous whole-frame GPU timestamp readback.
pub(super) struct TimestampSlot {
    resolve_buffer: wgpu::Buffer,
    buffer: wgpu::Buffer,
    submission: Option<wgpu::SubmissionIndex>,
    map_result: std::sync::Arc<std::sync::atomic::AtomicU8>,
    map_error: std::sync::Arc<std::sync::Mutex<Option<String>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GpuTimestampState {
    Unsupported,
    Ready,
    MapError,
}

impl Renderer {
    pub(super) fn acquire_timestamp_slot(&mut self) -> Option<usize> {
        if self.timestamp_state != GpuTimestampState::Ready {
            return None;
        }
        if let Some(index) = self
            .timestamp_slots
            .iter()
            .position(|slot| slot.submission.is_none())
        {
            return Some(index);
        }
        if self.timestamp_slots.len() >= 3 {
            return None;
        }
        let map_result = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
        let map_error = std::sync::Arc::new(std::sync::Mutex::new(None));
        self.timestamp_slots.push(TimestampSlot {
            resolve_buffer: self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rustlua-frame-timestamp-resolve"),
                size: 16,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            buffer: self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rustlua-frame-timestamp-readback"),
                size: 16,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            submission: None,
            map_result,
            map_error,
        });
        Some(self.timestamp_slots.len() - 1)
    }

    pub(super) fn finish_gpu_timestamp(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        slot_index: usize,
    ) {
        let Some(query_set) = &self.timestamp_query_set else {
            return;
        };
        encoder.write_timestamp(query_set, 1);
        let slot = &self.timestamp_slots[slot_index];
        encoder.resolve_query_set(query_set, 0..2, &slot.resolve_buffer, 0);
        encoder.copy_buffer_to_buffer(&slot.resolve_buffer, 0, &slot.buffer, 0, 16);
    }

    pub(super) fn arm_timestamp_slot(&mut self, index: usize, submission: wgpu::SubmissionIndex) {
        let slot = &mut self.timestamp_slots[index];
        let result = std::sync::Arc::clone(&slot.map_result);
        let error = std::sync::Arc::clone(&slot.map_error);
        slot.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |mapped| match mapped {
                Ok(()) => result.store(1, std::sync::atomic::Ordering::Release),
                Err(cause) => {
                    result.store(2, std::sync::atomic::Ordering::Release);
                    if let Ok(mut guard) = error.lock() {
                        *guard = Some(cause.to_string());
                    }
                }
            });
        slot.submission = Some(submission);
    }

    pub(super) fn poll_gpu_timestamp(&mut self) -> Option<u64> {
        if self.timestamp_state != GpuTimestampState::Ready {
            return None;
        }
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::ZERO),
        });
        for slot in &mut self.timestamp_slots {
            if slot.submission.is_none() {
                continue;
            }
            match slot.map_result.load(std::sync::atomic::Ordering::Acquire) {
                0 => continue,
                1 => {
                    let bytes = slot.buffer.slice(..).get_mapped_range();
                    let begin =
                        u64::from_le_bytes(bytes[0..8].try_into().expect("timestamp begin"));
                    let end = u64::from_le_bytes(bytes[8..16].try_into().expect("timestamp end"));
                    drop(bytes);
                    slot.buffer.unmap();
                    slot.submission = None;
                    slot.map_result
                        .store(0, std::sync::atomic::Ordering::Release);
                    let nanos = end.saturating_sub(begin) as f64 * self.timestamp_period_ns as f64;
                    return Some((nanos / 1_000.0).round() as u64);
                }
                _ => {
                    self.timestamp_error = slot
                        .map_error
                        .lock()
                        .ok()
                        .and_then(|mut value| value.take());
                    slot.submission = None;
                    slot.map_result
                        .store(0, std::sync::atomic::Ordering::Release);
                    self.timestamp_state = GpuTimestampState::MapError;
                    return None;
                }
            }
        }
        None
    }

    /// Waits briefly for one timestamp result. Intended for offline benchmark
    /// collection; the normal render path never blocks on GPU timing.
    pub fn wait_for_gpu_timestamp(&mut self, timeout: std::time::Duration) -> Option<u64> {
        if self.timestamp_state != GpuTimestampState::Ready {
            return None;
        }
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(value) = self.poll_gpu_timestamp() {
                return Some(value);
            }
            if Instant::now() >= deadline {
                return None;
            }
            let _ = self.device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_millis(1)),
            });
        }
    }

    /// Whether this device was created with timestamp query support.
    pub fn gpu_timestamp_supported(&self) -> bool {
        self.timestamp_state != GpuTimestampState::Unsupported
    }

    /// Stable diagnostic for benchmark reports when no GPU timestamp is available.
    pub fn gpu_timestamp_reason(&self) -> Option<&str> {
        match self.timestamp_state {
            GpuTimestampState::Unsupported => Some("unsupported"),
            GpuTimestampState::MapError => self.timestamp_error.as_deref().or(Some("map_error")),
            GpuTimestampState::Ready => None,
        }
    }
}
