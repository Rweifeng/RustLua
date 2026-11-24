//! 通用的**非阻塞离屏回读器**：离屏纹理 → staging buffer → CPU 像素。
//!
//! 为什么单独抽出来：仓里有**两处**需要"把渲染结果读回 CPU"——
//! 1. 编辑器 Scene View 的离屏回读（`Renderer::readback_frame`）；
//! 2. Shader 预览 / 特效缩略图（`editor::panels::shader_graph_preview`）。
//!
//! 这两处以前各写一套：`Renderer` 里是带环形槽 + 原子完成标志的完善实现，
//! 预览侧是"mpsc 通道 + 阻塞 poll"的手写版——后者在 UI 线程上等 GPU 画完，
//! 直接把编辑器帧率锁在 GPU 往返时间上（P3 的问题来源）。
//!
//! 现在两边共用这一份：**主线程永不阻塞等待 GPU**，调用方每次渲染提交一次拷贝、
//! 同时排空之前就绪的结果。合并后预览侧也自动获得环形槽、失败可重试、
//! 溢出丢弃计数这些原本只有 `Renderer` 才有的行为。
//!
//! 槽的设计（与 `Renderer` 原实现一致，行为逐字节不变）：
//! - 两个槽轮转，允许"上一帧还在飞、这一帧照常提交"；
//! - 完成信号用原子 `u8` 而不是 mpsc：`poll` 只需读一个原子量，
//!   不必持有通道端点，也不会有"端点被 drop 后永远收不到"的边角；
//! - 尺寸变化时重建 staging buffer（记 `buffer_rebuilds` 便于诊断）。

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

/// 一个回读槽：staging buffer + 在途状态 + 可复用的 CPU 输出缓冲。
pub(crate) struct ReadbackSlot {
    buffer: wgpu::Buffer,
    /// `None` = 槽空闲可复用；`Some` = 有在途提交。
    submission: Option<wgpu::SubmissionIndex>,
    /// `map_async` 回调完成标志：0=未就绪，1=映射成功，2=映射失败。
    map_result: Arc<AtomicU8>,
    /// `map_async` 失败时的错误消息（仅 `map_result == 2` 时有意义）。
    map_error: Arc<Mutex<Option<String>>>,
    width: u32,
    height: u32,
    row_bytes: u32,
    bytes_per_row: u32,
    /// 可复用的紧凑 CPU 输出缓冲（避免每次回读都新分配）。
    pixels: Vec<u8>,
    /// staging buffer 重建时递增，便于诊断。
    generation: u64,
}

/// 单次 `poll_slot` 的结果。
enum ReadbackPoll {
    /// 尚未就绪（主线程未阻塞等待 GPU）。
    NotReady,
    /// 就绪的紧凑 RGBA8 像素 + 实际尺寸。
    Ready(Vec<u8>, u32, u32),
    /// 映射失败（槽已置回空闲，允许下一帧重试）。
    Failed(String),
}

/// 回读器自身的工作量读数（给诊断面板/基线用）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AsyncReadbackMetrics {
    pub slot_count: usize,
    pub busy_count: usize,
    pub submitted: u64,
    pub completed: u64,
    pub dropped: u64,
    pub pending: u64,
    pub cpu_bytes: usize,
    pub buffer_rebuilds: u64,
}

/// 非阻塞回读器。持有 device/queue 的克隆与两个轮转槽。
///
/// `device`/`queue` 是 `wgpu` 的轻量句柄（内部 `Arc`），克隆不会复制 GPU 资源，
/// 因此这里自带一份比让调用方每次传入更简单，也让 `Renderer` 与 `ShaderPreview`
/// 可以各自持有、互不干扰。
pub struct AsyncReadback {
    device: wgpu::Device,
    queue: wgpu::Queue,
    slots: Vec<ReadbackSlot>,
    submitted: u64,
    completed: u64,
    dropped: u64,
    pending: u64,
    buffer_rebuilds: u64,
}

impl AsyncReadback {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self {
            device: device.clone(),
            queue: queue.clone(),
            slots: Vec::new(),
            submitted: 0,
            completed: 0,
            dropped: 0,
            pending: 0,
            buffer_rebuilds: 0,
        }
    }

    /// 非阻塞回读：总是提交一次纹理拷贝（写进环中下一空闲槽），同时排空之前提交的结果。
    ///
    /// 就绪返回 `ReadbackStatus::Ready`，未就绪返回 `Pending`——主线程永不阻塞等待 GPU。
    /// 纹理须有 `COPY_SRC` usage。
    ///
    /// 注意返回值可能是**上一次**提交的结果（滞后一帧）——这是环形槽的固有语义，
    /// 也正是"不阻塞"的代价与收益所在：调用方直接用这份像素即可，尺寸随结果附带。
    pub fn readback_frame(
        &mut self,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
    ) -> ReadbackStatus {
        // 先排空任意就绪槽（滞后一帧的结果）；失败槽上报错误（也允许重试）。
        let mut ready = None;
        let mut failed = None;
        for index in 0..self.slots.len() {
            match self.poll_slot(index) {
                ReadbackPoll::Ready(pixels, w, h) => {
                    ready = Some((pixels, w, h));
                    break;
                }
                ReadbackPoll::Failed(error) => {
                    if failed.is_none() {
                        failed = Some(error);
                    }
                }
                ReadbackPoll::NotReady => {}
            }
        }
        // 无论是否排空到结果，都提交本次拷贝，保证每个渲染帧都会被回读。
        self.submit(texture, width, height);
        if let Some((pixels, w, h)) = ready {
            ReadbackStatus::Ready {
                pixels,
                width: w,
                height: h,
            }
        } else if let Some(error) = failed {
            ReadbackStatus::Failed(error)
        } else {
            ReadbackStatus::Pending
        }
    }

    /// 只提交一次拷贝、**不排空**已就绪的结果。
    ///
    /// 与 [`Self::readback_frame`] 的分工：`readback_frame` 是"每帧都要结果"的场景
    /// （Scene View 离屏），提交与取回一体；这里是"提交与取回分开"的场景
    /// （Shader 预览），调用方自己决定何时 `take_ready`。
    ///
    /// 两者共用同一套槽与完成标志，只是排空时机不同。
    pub fn submit_copy(&mut self, texture: &wgpu::Texture, width: u32, height: u32) {
        self.submit(texture, width, height);
    }

    /// 只排空已就绪的结果、**不提交新拷贝**（预览这类"按需渲染"的场景用）。
    ///
    /// 返回 `Pending` = 暂时没有可取的像素（也含"没有在途槽"）。
    /// 映射失败的槽会在这里上报 `Failed` 并被清空，允许下一帧重试。
    pub fn take_ready(&mut self) -> ReadbackStatus {
        let mut failed = None;
        for index in 0..self.slots.len() {
            match self.poll_slot(index) {
                ReadbackPoll::Ready(pixels, width, height) => {
                    return ReadbackStatus::Ready {
                        pixels,
                        width,
                        height,
                    };
                }
                ReadbackPoll::Failed(error) => {
                    if failed.is_none() {
                        failed = Some(error);
                    }
                }
                ReadbackPoll::NotReady => {}
            }
        }
        match failed {
            Some(error) => ReadbackStatus::Failed(error),
            None => ReadbackStatus::Pending,
        }
    }

    /// 是否有槽处于在途（UI 可据此决定要不要继续请求重绘）。
    pub fn has_inflight(&self) -> bool {
        self.slots.iter().any(|slot| slot.submission.is_some())
    }

    /// 阻塞取出：等所有在途槽完成并返回第一个就绪的像素。
    ///
    /// 只给"必须立刻拿到图才能继续"的一次性场景（测试、导出截图）。**UI 每帧路径禁用**，
    /// 否则又回到 P3 要消除的"UI 线程等 GPU"。
    pub fn take_blocking(&mut self) -> Option<(Vec<u8>, u32, u32)> {
        if !self.has_inflight() {
            return None;
        }
        // 循环推进直到有槽就绪：`poll(Wait)` 保证已提交的 GPU 工作完成，但
        // `map_async` 的回调是在事件处理阶段被调用的，个别后端需要再推进一次
        // 才能看到 `map_result` 翻转。只 poll 一次在软件适配器上会偶发取不到。
        for _ in 0..64 {
            let _ = self.device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            });
            for index in 0..self.slots.len() {
                if let ReadbackPoll::Ready(pixels, w, h) = self.poll_slot(index) {
                    return Some((pixels, w, h));
                }
            }
            if !self.has_inflight() {
                break;
            }
        }
        None
    }

    pub fn metrics(&self) -> AsyncReadbackMetrics {
        AsyncReadbackMetrics {
            slot_count: self.slots.len(),
            busy_count: self.slots.iter().filter(|s| s.submission.is_some()).count(),
            submitted: self.submitted,
            completed: self.completed,
            dropped: self.dropped,
            pending: self.pending,
            cpu_bytes: self.slots.iter().map(|s| s.pixels.capacity()).sum(),
            buffer_rebuilds: self.buffer_rebuilds,
        }
    }

    /// 清空所有槽的在途状态与 CPU 缓冲（设备重建 / 切换目标时用）。
    pub fn clear(&mut self) {
        self.slots.clear();
    }

    /// 测试专用：注入一个"`map_async` 已失败"的槽，用来验证失败路径上报 `Failed`
    /// 而不是把脏数据当就绪返回（M11-03 / R-04 那条门的注入手段）。
    ///
    /// 真实路径下这种状态由 `map_async` 回调写 `map_result=2` 产生；测试无法让真实
    /// 映射失败，只能按同一份状态直接构造。
    #[cfg(test)]
    pub(crate) fn inject_failed_slot(
        &mut self,
        submission: wgpu::SubmissionIndex,
        error: &str,
        width: u32,
        height: u32,
    ) {
        let row_bytes = 4 * width;
        let bytes_per_row = row_bytes.div_ceil(256) * 256;
        let size_bytes = u64::from(bytes_per_row * height);
        let buffer = create_staging_buffer(&self.device, size_bytes);
        self.slots.push(ReadbackSlot {
            buffer,
            submission: Some(submission),
            map_result: Arc::new(AtomicU8::new(2)),
            map_error: Arc::new(Mutex::new(Some(error.to_owned()))),
            width,
            height,
            row_bytes,
            bytes_per_row,
            pixels: Vec::new(),
            generation: 0,
        });
    }

    fn poll_slot(&mut self, index: usize) -> ReadbackPoll {
        let Some(slot) = self.slots.get(index) else {
            return ReadbackPoll::NotReady;
        };
        if slot.submission.is_none() {
            return ReadbackPoll::NotReady;
        }
        // 非阻塞 maintain：处理所有已完成的提交与 `map_async` 回调（超时即返回，
        // 主线程不阻塞）。就绪与否由 `map_result` 标志判断。
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::ZERO),
        });
        let result = slot.map_result.load(Ordering::Acquire);
        if result == 0 {
            return ReadbackPoll::NotReady;
        }
        if result == 2 {
            // 映射失败：上报错误并把槽置回空闲（buffer 未映射，不能 unmap）。
            let error = slot
                .map_error
                .lock()
                .ok()
                .and_then(|mut guard| guard.take())
                .unwrap_or_else(|| "GPU 回读映射失败".to_owned());
            let slot = &mut self.slots[index];
            slot.submission = None;
            slot.map_result.store(0, Ordering::Release);
            return ReadbackPoll::Failed(error);
        }
        // 映射成功：读出行数据并置回空闲。
        let (width, height, row_bytes, bytes_per_row) =
            (slot.width, slot.height, slot.row_bytes, slot.bytes_per_row);
        let buffer = slot.buffer.clone();
        let padded = buffer.slice(..).get_mapped_range();
        let mut pixels = self.slots[index].pixels.clone();
        pixels.clear();
        pixels.reserve((row_bytes * height) as usize);
        for row in 0..height as usize {
            let start = row * bytes_per_row as usize;
            pixels.extend_from_slice(&padded[start..start + row_bytes as usize]);
        }
        drop(padded);
        buffer.unmap();
        let slot = &mut self.slots[index];
        slot.submission = None;
        slot.map_result.store(0, Ordering::Release);
        self.completed = self.completed.saturating_add(1);
        ReadbackPoll::Ready(pixels, width, height)
    }

    /// 向一个空闲槽提交离屏纹理 → staging buffer 的拷贝（buffer 尺寸不符时重建）。
    fn submit(&mut self, texture: &wgpu::Texture, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        let row_bytes = 4 * width;
        let bytes_per_row = row_bytes.div_ceil(256) * 256;
        let size_bytes = (bytes_per_row * height) as u64;
        // 惰性创建双缓冲槽。
        if self.slots.is_empty() {
            for _ in 0..2 {
                self.slots.push(self.new_slot(width, height, size_bytes));
            }
        }
        // 找一个空闲槽（无在途提交）。
        let Some(index) = self.slots.iter().position(|slot| slot.submission.is_none()) else {
            self.dropped = self.dropped.saturating_add(1);
            return; // 全部在途，跳过本次提交
        };
        // 尺寸不符时重建 buffer，避免越界写入。
        let device = self.device.clone();
        let slot = &mut self.slots[index];
        if slot.width != width || slot.height != height || slot.buffer.size() < size_bytes {
            slot.buffer = create_staging_buffer(&device, size_bytes);
            slot.width = width;
            slot.height = height;
            slot.row_bytes = row_bytes;
            slot.bytes_per_row = bytes_per_row;
            slot.generation = slot.generation.saturating_add(1);
            self.buffer_rebuilds = self.buffer_rebuilds.saturating_add(1);
        }
        let buffer = slot.buffer.clone();
        let map_result = slot.map_result.clone();
        let map_error = slot.map_error.clone();
        slot.map_result.store(0, Ordering::Release);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("rustlua-async-readback-copy"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        // 先提交拷贝，再请求映射（与同步回读一致：`map_async` 会把 buffer 标记为
        // mapped-pending，若在 copy 提交前调用会触发 "Buffer is still mapped"）。
        let submission = self.queue.submit([encoder.finish()]);
        // 捕获 `map_async` 结果。映射失败不再被当作"已就绪"返回脏数据，
        // 而是记录错误（map_result=2 + map_error），由 `poll_slot` 上报并清槽重试。
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| match result {
                Ok(()) => map_result.store(1, Ordering::Release),
                Err(error) => {
                    map_result.store(2, Ordering::Release);
                    if let Ok(mut guard) = map_error.lock() {
                        *guard = Some(error.to_string());
                    }
                }
            });
        self.slots[index].submission = Some(submission);
        self.submitted = self.submitted.saturating_add(1);
        self.pending = self.pending.saturating_add(1);
    }

    fn new_slot(&self, width: u32, height: u32, size_bytes: u64) -> ReadbackSlot {
        ReadbackSlot {
            buffer: create_staging_buffer(&self.device, size_bytes),
            submission: None,
            map_result: Arc::new(AtomicU8::new(0)),
            map_error: Arc::new(Mutex::new(None)),
            width,
            height,
            row_bytes: 4 * width,
            bytes_per_row: (4 * width).div_ceil(256) * 256,
            pixels: Vec::with_capacity((4 * width * height) as usize),
            generation: 0,
        }
    }
}

fn create_staging_buffer(device: &wgpu::Device, size_bytes: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rustlua-async-readback"),
        size: size_bytes.max(4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    })
}

/// 非阻塞离屏回读的结果。
pub enum ReadbackStatus {
    /// 就绪的紧凑 RGBA8 像素（每行 `row_bytes`，无行尾填充），附实际尺寸。
    /// 尺寸可能与当前渲染尺寸不同（排空的是上一帧不同分辨率的槽）。
    Ready {
        pixels: Vec<u8>,
        width: u32,
        height: u32,
    },
    /// 尚未就绪（主线程未阻塞等待 GPU）；调用方沿用上一帧纹理即可。
    Pending,
    /// 回读失败。
    Failed(String),
}
