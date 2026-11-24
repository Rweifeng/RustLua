//! 离屏回读的**对外门面**：同步截图（`copy_texture_to_cpu`、PNG 捕获）与
//! 非阻塞回读的委托方法、对外指标类型。
//!
//! 非阻塞回读的**实现**在 [`super::async_readback`]（P3 起与 Shader 预览共用同一份
//! 环形槽逻辑）；本模块只保留 `impl Renderer` 上的委托与对外类型 `ReadbackMetrics` /
//! `ReadbackStatus`（后者由 `async_readback` 定义、这里 `use` 进来再
//! 经 `renderer/mod.rs` 的 `pub use` 接回原路径，对外路径不变）。

use super::Renderer;
use super::async_readback::ReadbackStatus;
use rustlua_render::frame::{Scene3d, SceneMesh};
use std::path::Path;

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

/// 在 PNG 内侧画一圈相框。照片模式用，不参与窗口逐帧路径。
fn paint_frame(image: &mut image::RgbaImage, style: u8) {
    let (width, height) = image.dimensions();
    let short = width.min(height);
    let border = ((short as f32) * 0.035).round().clamp(3.0, 24.0) as u32;
    let (r, g, b) = match style {
        2 => (178, 142, 74),
        3 => (96, 124, 88),
        4 => (22, 22, 22),
        _ => (78, 54, 36),
    };
    let color = image::Rgba([r, g, b, 255]);
    for y in 0..height {
        for x in 0..width {
            if x < border || y < border || x >= width - border || y >= height - border {
                image.put_pixel(x, y, color);
            }
        }
    }
    // 第二道细线让相框不像纯色块；只对 2/3/4 档画。
    if style >= 2 {
        let inner = border + 3;
        let line = image::Rgba([
            r.saturating_add(30),
            g.saturating_add(26),
            b.saturating_add(18),
            255,
        ]);
        for y in inner..height.saturating_sub(inner) {
            for x in inner..width.saturating_sub(inner) {
                if x == inner || y == inner || x + 1 == width - inner || y + 1 == height - inner {
                    image.put_pixel(x, y, line);
                }
            }
        }
    }
}

/// 右下角烧一个 5×7 点阵的 `MYRMEX` 标记；只给照片模式，不依赖 UI 字体。
fn paint_mark(image: &mut image::RgbaImage) {
    const GLYPHS: [[&str; 7]; 6] = [
        [
            "10001", "11011", "10101", "10101", "10001", "10001", "10001",
        ], // M
        [
            "10001", "10001", "01010", "00100", "00100", "00100", "00100",
        ], // Y
        [
            "11110", "10001", "10001", "11110", "10100", "10010", "10001",
        ], // R
        [
            "10001", "11011", "10101", "10101", "10001", "10001", "10001",
        ], // M
        [
            "11111", "10000", "10000", "11110", "10000", "10000", "11111",
        ], // E
        [
            "10001", "10001", "01010", "00100", "01010", "10001", "10001",
        ], // X
    ];
    let (width, height) = image.dimensions();
    let border = (((width.min(height) as f32) * 0.035).round() as u32).clamp(3, 24);
    let scale = if width.min(height) < 200 { 1 } else { 2 };
    let gap = scale;
    let glyph_w = 5 * scale;
    let text_w = GLYPHS.len() as u32 * glyph_w + (GLYPHS.len() as u32 - 1) * gap;
    let text_h = 7 * scale;
    if width < text_w + border + 16 || height < text_h + border + 16 {
        return;
    }
    let x0 = width - border - 8 - text_w;
    let y0 = height - border - 8 - text_h;
    let color = image::Rgba([240, 232, 210, 255]);
    for (g, glyph) in GLYPHS.iter().enumerate() {
        let gx = x0 + g as u32 * (glyph_w + gap);
        for (row, line) in glyph.iter().enumerate() {
            for (col, ch) in line.chars().enumerate() {
                if ch != '1' {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let x = gx + col as u32 * scale + dx;
                        let y = y0 + row as u32 * scale + dy;
                        if x < width && y < height {
                            image.put_pixel(x, y, color);
                        }
                    }
                }
            }
        }
    }
}

/// 抓帧失败时的占位 PNG：深绿底 + 黄铜框 + MYRMEX 标记，避免存档缩略图全白。
pub fn write_capture_placeholder(path: &Path) -> anyhow::Result<()> {
    let mut image = image::RgbaImage::from_pixel(320, 180, image::Rgba([18, 22, 18, 255]));
    paint_frame(&mut image, 2);
    paint_mark(&mut image);
    image
        .save_with_format(path, image::ImageFormat::Png)
        .map_err(|error| anyhow::anyhow!("save capture placeholder failed: {error}"))
}

impl Renderer {
    /// 把当前帧额外渲染进离屏纹理并存成 PNG（M-screenshot）。
    ///
    /// 复用窗口渲染器的全部帧视图/环境/管线；离屏目标的格式与 surface 一致，
    /// 因此 BGRA surface 会在这里换回 RGBA 再编码。调用方只在用户按 F2 时走一次，
    /// 不进入逐帧路径。
    /// `frame` 是照片模式相框档位（0 = 不烧边框）；1..4 用不同颜色画一圈内边框。
    pub fn capture_surface_png(
        &mut self,
        mesh: &SceneMesh,
        scene3d: Option<&Scene3d>,
        path: &Path,
        frame: u8,
        include_ui: bool,
    ) -> anyhow::Result<()> {
        let (width, height) = (self.config.width, self.config.height);
        if width == 0 || height == 0 {
            anyhow::bail!("surface has zero size ({width}x{height})");
        }
        let format = self.config.format;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rustlua-capture-frame"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.render_into_texture(mesh, scene3d, &view, width, height, include_ui);
        let mut pixels = self.copy_texture_to_cpu(&texture, width, height)?;
        // Surface 常用 Bgra8UnormSrgb；PNG 要的是 RGB 顺序。
        if matches!(
            format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        ) {
            for px in pixels.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
        }
        let mut image = image::RgbaImage::from_raw(width, height, pixels)
            .ok_or_else(|| anyhow::anyhow!("capture pixel buffer size mismatch"))?;
        if frame > 0 && !include_ui {
            paint_frame(&mut image, frame);
            paint_mark(&mut image);
        }
        image
            .save_with_format(path, image::ImageFormat::Png)
            .map_err(|error| anyhow::anyhow!("save capture PNG failed: {error}"))?;
        Ok(())
    }

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

    /// P3-R170：把平面反射的镜像色图（`Rgba16Float`，半分辨率）同步回读到 CPU。
    ///
    /// 只在平面反射 pass 真跑过之后才有意义：没跑过的帧里目标仍是构造期的 1×1 透明占位图，
    /// 返回的尺寸就是 `(1, 1)`。alpha 是命中掩码，`alpha >= 0.5` 才说明几何真的光栅化进去了。
    pub fn read_planar_reflection_to_cpu(&self) -> anyhow::Result<SceneColorReadback> {
        let (width, height) = self.planar_reflection_size;
        if width == 0 || height == 0 {
            anyhow::bail!("planar reflection target has zero size ({width}x{height})");
        }
        let row_bytes = 8 * width;
        let bytes_per_row = row_bytes.div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-planar-reflection-readback"),
            size: u64::from(bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("rustlua-planar-reflection-copy"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.planar_reflection_texture,
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
        let map_result = rx.recv().map_err(|error| {
            anyhow::anyhow!("planar reflection readback channel error: {error}")
        })?;
        map_result
            .map_err(|error| anyhow::anyhow!("planar reflection texture map failed: {error}"))?;
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
    /// P3：具体实现搬到 [`super::async_readback::AsyncReadback`]，与 Shader 预览
    /// 共用同一份环形槽逻辑；这里只是委托，行为与搬出前逐字节一致。
    pub fn readback_frame(
        &mut self,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
    ) -> ReadbackStatus {
        self.readback.readback_frame(texture, width, height)
    }

    pub fn readback_slot_count(&self) -> usize {
        self.readback.metrics().slot_count
    }

    pub fn readback_busy_count(&self) -> usize {
        self.readback.metrics().busy_count
    }

    pub fn readback_metrics(&self) -> ReadbackMetrics {
        let m = self.readback.metrics();
        ReadbackMetrics {
            slot_count: m.slot_count,
            busy_count: m.busy_count,
            submitted: m.submitted,
            completed: m.completed,
            dropped: m.dropped,
            pending: m.pending,
            cpu_bytes: m.cpu_bytes,
            buffer_rebuilds: m.buffer_rebuilds,
        }
    }

    pub fn readback_in_flight(&self) -> bool {
        self.readback.has_inflight()
    }

    /// 测试专用：见 [`super::async_readback::AsyncReadback::inject_failed_slot`]。
    #[cfg(test)]
    pub(super) fn inject_failed_readback_slot(
        &mut self,
        submission: wgpu::SubmissionIndex,
        error: &str,
        width: u32,
        height: u32,
    ) {
        self.readback
            .inject_failed_slot(submission, error, width, height);
    }

    pub fn readback_pending(&mut self) -> ReadbackStatus {
        // 只"排空"、不提交新拷贝：取到就绪槽返回 Ready，映射失败返回 Failed，
        // 否则 Pending（含"当前没有在途槽"）。
        self.readback.take_ready()
    }
}
