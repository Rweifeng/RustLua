//! 非阻塞离屏回读（WP12 阶段 4b 从 `renderer/mod.rs` 原样搬出）：槽位与结果类型，
//! 以及 `impl Renderer` 上的拷贝/轮询/提交/账本方法。
//!
//! `ReadbackMetrics` / `ReadbackStatus` 是 `host/mod.rs` 再导出的对外类型，保持 `pub` 不变，
//! 由 `renderer/mod.rs` 用 `pub use` 接回原路径。`ReadbackSlot`（`Renderer` 的字段类型）留在
//! `mod.rs`：本模块是它的后代，读写它的私有字段本来就不需要放宽任何可见性（计划 §3.1.2）。

use super::{ReadbackSlot, Renderer};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReadbackMetrics {
    pub slot_count: usize,
    pub busy_count: usize,
    pub submitted: u64,
    pub completed: u64,
    pub dropped: u64,
    pub pending: u64,
    pub cpu_bytes: usize,
    pub buffer_rebuilds: u64,
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

/// HDR 场景颜色的 CPU 回读（P3-R01）。
///
/// `pixels` 是**线性** RGBA f32（`Rgba16Float` 解码后），不做 sRGB 转换——tonemap 的输入
/// 就是线性值，验收要证明"tonemap 前确实有 >1 的颜色"。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneColorReadback {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 4]>,
}

impl SceneColorReadback {
    /// 全图 RGB 三个通道的最大值（忽略 NaN）。
    #[must_use]
    pub fn max_rgb(&self) -> f32 {
        self.pixels
            .iter()
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .filter(|value| !value.is_nan())
            .fold(0.0_f32, f32::max)
    }

    /// 按像素坐标取 RGBA；越界返回 `None`。
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[f32; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        self.pixels
            .get((y as usize) * (self.width as usize) + (x as usize))
            .copied()
    }
}

/// 把 IEEE 754 binary16 的位模式解码成 f32。
///
/// 为什么不引入 `half`：全仓只在 HDR 读回这一处需要它，而且必须能在 wasm 上编译；
/// 这段位运算与 `half::f16::to_f32` 的口径一致（含次正规数、±inf、NaN）。
#[must_use]
pub(crate) fn f16_bits_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits & 0x8000) << 16;
    let exponent = (bits >> 10) & 0x1f;
    let mantissa = u32::from(bits & 0x03ff);
    let decoded = if exponent == 0 {
        if mantissa == 0 {
            sign
        } else {
            // 次正规数：归一化到 f32 的指数范围。
            let mut mantissa = mantissa;
            let mut exponent = -14_i32;
            while mantissa & 0x0400 == 0 {
                mantissa <<= 1;
                exponent -= 1;
            }
            mantissa &= 0x03ff;
            sign | (((exponent + 127) as u32) << 23) | (mantissa << 13)
        }
    } else if exponent == 0x1f {
        // inf / NaN：保留尾数，NaN 的 payload 不丢。
        sign | 0x7f80_0000 | (mantissa << 13)
    } else {
        sign | (u32::from(exponent) + 112) << 23 | (mantissa << 13)
    };
    f32::from_bits(decoded)
}

/// 单个回读槽轮询的结果（M11-03）：未就绪 / 就绪像素 / 映射失败。
enum ReadbackPoll {
    NotReady,
    Ready(Vec<u8>, u32, u32),
    Failed(String),
}

impl Renderer {
    /// Copies an RGBA8 texture back to the CPU (synchronous readback), used by
    /// the editor to display the embedded preview. The texture must have
    /// `COPY_SRC` usage.
    pub fn copy_texture_to_cpu(
        &self,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
    ) -> anyhow::Result<Vec<u8>> {
        // `bytes_per_row` must be a multiple of 256 for `copy_texture_to_buffer`.
        let row_bytes = 4 * width;
        let bytes_per_row = row_bytes.div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-preview-readback"),
            size: (bytes_per_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("rustlua-preview-copy"),
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
        self.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        let map_result = rx
            .recv()
            .map_err(|error| anyhow::anyhow!("preview readback channel error: {error}"))?;
        map_result.map_err(|error| anyhow::anyhow!("preview texture map failed: {error}"))?;
        // 去掉行尾填充，返回紧凑 RGBA8（每行 `row_bytes`）。
        let padded = slice.get_mapped_range();
        let mut pixels = Vec::with_capacity((row_bytes * height) as usize);
        for row in 0..height as usize {
            let start = row * bytes_per_row as usize;
            pixels.extend_from_slice(&padded[start..start + row_bytes as usize]);
        }
        Ok(pixels)
    }

    /// P3-R01：把 HDR 场景颜色目标（`Rgba16Float`）同步回读到 CPU。
    ///
    /// 返回的是**线性** f32（不做 sRGB 转换）——Tonemap 的输入就是线性值，验收要证明
    /// `pre_tonemap_max > 1`。没有创建过 HDR 目标时返回错误而不是空数据：调用方必须能区分
    /// "这一帧没走 HDR"与"HDR 读了但全是 0"。
    pub fn read_scene_color_to_cpu(&self) -> anyhow::Result<SceneColorReadback> {
        let texture = self
            .hdr_scene_texture
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("HDR scene color target has not been created yet"))?;
        let (width, height) = self.hdr_scene_size;
        if width == 0 || height == 0 {
            anyhow::bail!("HDR scene color target has zero size ({width}x{height})");
        }
        let row_bytes = 8 * width;
        let bytes_per_row = row_bytes.div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-hdr-scene-readback"),
            size: u64::from(bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("rustlua-hdr-scene-copy"),
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
        self.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        let map_result = rx
            .recv()
            .map_err(|error| anyhow::anyhow!("HDR scene readback channel error: {error}"))?;
        map_result.map_err(|error| anyhow::anyhow!("HDR scene texture map failed: {error}"))?;
        let padded = slice.get_mapped_range();
        let mut pixels = Vec::with_capacity((width * height) as usize);
        for row in 0..height as usize {
            let start = row * bytes_per_row as usize;
            for column in 0..width as usize {
                let offset = start + column * 8;
                let channel = |index: usize| {
                    f16_bits_to_f32(u16::from_le_bytes([
                        padded[offset + index * 2],
                        padded[offset + index * 2 + 1],
                    ]))
                };
                pixels.push([channel(0), channel(1), channel(2), channel(3)]);
            }
        }
        drop(padded);
        buffer.unmap();
        Ok(SceneColorReadback {
            width,
            height,
            pixels,
        })
    }

    /// 非阻塞离屏回读：总是提交一次纹理拷贝（写进环中下一空闲槽），同时尝试
    /// 读取之前提交的结果。就绪返回 `Ready(pixels)`，未就绪返回 `Pending`——
    /// 主线程永不阻塞等待 GPU。纹理须有 `COPY_SRC` usage。
    ///
    /// M11-03（R-04）：映射失败的回读槽现在会返回 `Failed` 而非当作就绪返回脏数据；
    /// 失败槽会清空并允许下帧重试。
    pub fn readback_frame(
        &mut self,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
    ) -> ReadbackStatus {
        // 先排空任意就绪槽（滞后一帧的结果）；失败槽上报错误（也允许重试）。
        let mut ready = None;
        let mut failed = None;
        for index in 0..self.readback_slots.len() {
            match self.poll_readback(index) {
                ReadbackPoll::Ready(pixels, width, height) => {
                    ready = Some((pixels, width, height));
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
        self.submit_readback(texture, width, height);
        self.readback_pending_count += 1;
        if let Some((pixels, width, height)) = ready {
            ReadbackStatus::Ready {
                pixels,
                width,
                height,
            }
        } else if let Some(error) = failed {
            ReadbackStatus::Failed(error)
        } else {
            ReadbackStatus::Pending
        }
    }

    pub fn readback_slot_count(&self) -> usize {
        self.readback_slots.len()
    }

    pub fn readback_busy_count(&self) -> usize {
        self.readback_slots
            .iter()
            .filter(|slot| slot.submission.is_some())
            .count()
    }

    pub fn readback_metrics(&self) -> ReadbackMetrics {
        ReadbackMetrics {
            slot_count: self.readback_slots.len(),
            busy_count: self.readback_busy_count(),
            submitted: self.readback_submitted,
            completed: self.readback_completed,
            dropped: self.readback_dropped,
            pending: self.readback_pending_count,
            cpu_bytes: self
                .readback_slots
                .iter()
                .map(|slot| slot.pixels.capacity())
                .sum(),
            buffer_rebuilds: self.readback_buffer_rebuilds,
        }
    }
    /// 是否有在途（已提交未读）的回读等待排空。
    pub fn readback_in_flight(&self) -> bool {
        self.readback_slots
            .iter()
            .any(|slot| slot.submission.is_some())
    }

    /// 仅排空待读的回读（内容未变、无需新提交时调用），返回就绪像素或映射错误。
    pub fn readback_pending(&mut self) -> ReadbackStatus {
        for index in 0..self.readback_slots.len() {
            match self.poll_readback(index) {
                ReadbackPoll::Ready(pixels, width, height) => {
                    return ReadbackStatus::Ready {
                        pixels,
                        width,
                        height,
                    };
                }
                ReadbackPoll::Failed(error) => return ReadbackStatus::Failed(error),
                ReadbackPoll::NotReady => {}
            }
        }
        ReadbackStatus::Pending
    }

    /// 非阻塞检查一个回读槽的映射结果：未就绪 / 就绪（读出行数据并置回空闲）/
    /// 映射失败（上报错误并把槽置回空闲以便重试）。
    fn poll_readback(&mut self, index: usize) -> ReadbackPoll {
        let Some(slot) = self.readback_slots.get(index) else {
            return ReadbackPoll::NotReady;
        };
        let Some(_) = slot.submission else {
            return ReadbackPoll::NotReady;
        };
        // 非阻塞 maintain：处理所有已完成的提交与 `map_async` 回调（超时即返回，
        // 主线程不阻塞）。就绪与否由 `map_result` 标志判断。
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::ZERO),
        });
        let result = slot.map_result.load(std::sync::atomic::Ordering::Acquire);
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
            let slot = &mut self.readback_slots[index];
            slot.submission = None;
            slot.map_result
                .store(0, std::sync::atomic::Ordering::Release);
            return ReadbackPoll::Failed(error);
        }
        // 映射成功：读出行数据并置回空闲。
        let (width, height, row_bytes, bytes_per_row) =
            (slot.width, slot.height, slot.row_bytes, slot.bytes_per_row);
        let buffer = slot.buffer.clone();
        let padded = buffer.slice(..).get_mapped_range();
        let mut pixels = self.readback_slots[index].pixels.clone();
        pixels.clear();
        pixels.reserve((row_bytes * height) as usize);
        for row in 0..height as usize {
            let start = row * bytes_per_row as usize;
            pixels.extend_from_slice(&padded[start..start + row_bytes as usize]);
        }
        drop(padded);
        buffer.unmap();
        // 槽置回空闲，允许复用。
        let slot = &mut self.readback_slots[index];
        slot.submission = None;
        slot.map_result
            .store(0, std::sync::atomic::Ordering::Release);
        self.readback_completed = self.readback_completed.saturating_add(1);
        ReadbackPoll::Ready(pixels, width, height)
    }

    /// 向一个空闲回读槽提交离屏纹理 → staging buffer 的拷贝（buffer 尺寸不符时重建）。
    fn submit_readback(&mut self, texture: &wgpu::Texture, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        let row_bytes = 4 * width;
        let bytes_per_row = row_bytes.div_ceil(256);
        let bytes_per_row = bytes_per_row * 256;
        let size_bytes = (bytes_per_row * height) as u64;
        tracing::debug!(target: "render", width, height, "[readback] submit");
        // 惰性创建双缓冲槽。
        while self.readback_slots.len() < 2 {
            self.readback_slots.push(ReadbackSlot {
                buffer: self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("rustlua-editor-readback"),
                    size: size_bytes.max(4),
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                submission: None,
                map_result: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0)),
                map_error: std::sync::Arc::new(std::sync::Mutex::new(None)),
                width,
                height,
                row_bytes,
                bytes_per_row,
                pixels: Vec::with_capacity((row_bytes * height) as usize),
                generation: 0,
            });
        }
        // 找一个空闲槽（无在途提交）。
        let Some(index) = self
            .readback_slots
            .iter()
            .position(|slot| slot.submission.is_none())
        else {
            self.readback_dropped += 1;
            return; // 全部在途，跳过本次提交
        };
        // 尺寸不符时重建 buffer，避免越界写入。
        let slot = &mut self.readback_slots[index];
        if slot.width != width || slot.height != height || slot.buffer.size() < size_bytes {
            slot.buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rustlua-editor-readback"),
                size: size_bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            slot.width = width;
            slot.height = height;
            slot.row_bytes = row_bytes;
            slot.bytes_per_row = bytes_per_row;
            slot.generation = slot.generation.saturating_add(1);
            self.readback_buffer_rebuilds = self.readback_buffer_rebuilds.saturating_add(1);
        }
        let buffer = slot.buffer.clone();
        let map_result = slot.map_result.clone();
        let map_error = slot.map_error.clone();
        slot.map_result
            .store(0, std::sync::atomic::Ordering::Release);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("rustlua-editor-readback-copy"),
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
        // 先提交拷贝，再请求映射（与同步回读一致：map_async 会把 buffer 标记为
        // mapped-pending，若在 copy 提交前调用会触发 "Buffer is still mapped"）。
        let submission = self.queue.submit([encoder.finish()]);
        // M11-03（R-04）：捕获 `map_async` 结果。映射失败不再被当作"已就绪"返回脏数据，
        // 而是记录错误（map_result=2 + map_error），由 poll_readback 上报 Failed 并清槽重试。
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| match result {
                Ok(()) => map_result.store(1, std::sync::atomic::Ordering::Release),
                Err(error) => {
                    map_result.store(2, std::sync::atomic::Ordering::Release);
                    if let Ok(mut guard) = map_error.lock() {
                        *guard = Some(error.to_string());
                    }
                }
            });
        slot.submission = Some(submission);
        self.readback_submitted = self.readback_submitted.saturating_add(1);
    }
}
