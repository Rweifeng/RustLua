//! GPU 时间戳查询的槽位与状态（WP12 阶段 4a 从 `renderer/mod.rs` 原样搬出；
//! E06-3 扩展为整帧 + 逐通道）：
//! `TimestampSlot` / `GpuTimestampState` 两个类型，以及 `impl Renderer` 上读写它们的几条方法。
//!
//! 一个查询集同时承载三样东西：帧起点（索引 0）、每条通道的起止、帧终点。槽位保存
//! [`PassTimestampLayout`]（显式索引），因此中途放弃的通道不会让后面的配对错位；异步回读
//! 完成后一次解析出整帧与各通道时长，写进 [`GpuPassReport`]。
//!
//! 这两个类型是 `Renderer` 的字段类型，`mod.rs` 要按短名写进结构体 ⇒ 放宽为 `pub(super)`
//! （计划 §3.1.4）；`Renderer` 本身仍定义在 `mod.rs`——固有 `impl` 块分散在同 crate 的兄弟
//! 模块即可，字段一个都不用动。

use super::pass_timestamps::{
    PassTimestampLayout, TIMESTAMP_BUFFER_BYTES, consistency_delta_pct, parse_pass_timings,
    within_tolerance,
};
use super::{GpuPassReport, Renderer};
use web_time::Instant;

/// One asynchronous whole-frame GPU timestamp readback.
pub(super) struct TimestampSlot {
    resolve_buffer: wgpu::Buffer,
    buffer: wgpu::Buffer,
    submission: Option<wgpu::SubmissionIndex>,
    map_result: std::sync::Arc<std::sync::atomic::AtomicU8>,
    map_error: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// 这次提交里每条通道的查询索引；回读时按它解析。
    layout: PassTimestampLayout,
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
                size: TIMESTAMP_BUFFER_BYTES,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            buffer: self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rustlua-frame-timestamp-readback"),
                size: TIMESTAMP_BUFFER_BYTES,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            submission: None,
            map_result,
            map_error,
            layout: PassTimestampLayout::default(),
        });
        Some(self.timestamp_slots.len() - 1)
    }

    pub(super) fn finish_gpu_timestamp(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        slot_index: usize,
        layout: &PassTimestampLayout,
    ) {
        let Some(query_set) = &self.timestamp_query_set else {
            return;
        };
        encoder.write_timestamp(query_set, layout.frame_end);
        let slot = &self.timestamp_slots[slot_index];
        encoder.resolve_query_set(query_set, 0..layout.frame_end + 1, &slot.resolve_buffer, 0);
        encoder.copy_buffer_to_buffer(
            &slot.resolve_buffer,
            0,
            &slot.buffer,
            0,
            TIMESTAMP_BUFFER_BYTES,
        );
    }

    pub(super) fn arm_timestamp_slot(
        &mut self,
        index: usize,
        submission: wgpu::SubmissionIndex,
        layout: PassTimestampLayout,
    ) {
        let slot = &mut self.timestamp_slots[index];
        slot.layout = layout;
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
                    let parsed = {
                        let bytes = slot.buffer.slice(..).get_mapped_range();
                        let timestamps = bytes
                            .chunks_exact(8)
                            .map(|chunk| {
                                u64::from_le_bytes(chunk.try_into().expect("timestamp word"))
                            })
                            .collect::<Vec<_>>();
                        drop(bytes);
                        parse_pass_timings(&timestamps, &slot.layout, self.timestamp_period_ns)
                    };
                    slot.buffer.unmap();
                    slot.submission = None;
                    slot.map_result
                        .store(0, std::sync::atomic::Ordering::Release);
                    let Some((frame_us, passes)) = parsed else {
                        continue;
                    };
                    let pass_sum_us: u64 = passes.iter().map(|timing| timing.gpu_us).sum();
                    let delta_pct = consistency_delta_pct(frame_us, pass_sum_us);
                    self.last_gpu_pass_report = GpuPassReport {
                        supported: true,
                        reason: None,
                        frame_us: Some(frame_us),
                        passes,
                        pass_sum_us,
                        delta_pct,
                        consistent: delta_pct.map(within_tolerance),
                    };
                    return Some(frame_us);
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
