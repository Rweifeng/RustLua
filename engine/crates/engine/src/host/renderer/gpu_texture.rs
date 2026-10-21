//! `Renderer` 的 GPU 纹理与绑定组（WP12 阶段 4b 从 `renderer/mod.rs` 原样搬出）：
//! 矢量文本缓冲扩容、SDF/图集/字体图集/自定义字体的按需上传、白纹理、材质绑定组缓存、
//! 以及通用的 `upload_texture`。
//!
//! 同一类型、同 crate 的另一个固有 `impl` 块；可见性口径见 `gpu_mesh.rs` 的文件头。

use crate::host::frame::{
    ATLAS_TEXTURE_PATH, FONT_ATLAS_TEXTURE_PATH, FontAtlasPixels, WHITE_TEXTURE_PATH,
    font_asset_from_texture_path, is_font_texture_path,
};
use rustlua_assets::asset_handle::AssetHandle;
use rustlua_render::text::TextAtlas;
use rustlua_render::texture::{TextureAsset, TextureColorSpace};
use std::path::PathBuf;

use super::{CustomFontUpload, LoadedTexture, Renderer, hash_bytes};

impl Renderer {
    pub(super) fn ensure_vector_buffer_capacity(
        &mut self,
        vertex_size: u64,
        glyph_size: u64,
        segment_size: u64,
    ) {
        fn grown(current: u64, required: u64) -> u64 {
            if required <= current {
                current
            } else {
                required.max(256).next_power_of_two()
            }
        }
        let mut buffers_changed = false;
        let vertex_capacity = grown(self.vector_vertex_buffer.size(), vertex_size);
        if vertex_capacity != self.vector_vertex_buffer.size() {
            self.vector_vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rustlua-vector-text-vertex-buffer"),
                size: vertex_capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            buffers_changed = true;
        }
        let glyph_capacity = grown(self.vector_glyph_buffer.size(), glyph_size);
        if glyph_capacity != self.vector_glyph_buffer.size() {
            self.vector_glyph_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rustlua-vector-text-glyph-buffer"),
                size: glyph_capacity,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            buffers_changed = true;
        }
        let segment_capacity = grown(self.vector_segment_buffer.size(), segment_size);
        if segment_capacity != self.vector_segment_buffer.size() {
            self.vector_segment_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rustlua-vector-text-segment-buffer"),
                size: segment_capacity,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            buffers_changed = true;
        }
        if buffers_changed {
            self.vector_bind_group = None;
            self.vector_glyph_cache.clear();
            self.vector_segment_cache.clear();
            self.vector_glyph_hash = 0;
            self.vector_segment_hash = 0;
        }
    }

    /// Picks the SDF bind group for a draw: the default-atlas bind for the the
    /// `FONT_ATLAS_TEXTURE_PATH` sentinel, or the per-custom-font bind for a
    /// namespaced `font://...` path. Returns `None` when the target atlas hasn't
    /// been uploaded this frame yet (caller skips the draw).
    pub(super) fn sdf_bind_for(&self, texture: u32, paths: &[String]) -> Option<wgpu::BindGroup> {
        if texture == 0 {
            return None;
        }
        let index = texture as usize - 1;
        let path = paths.get(index)?;
        if path == FONT_ATLAS_TEXTURE_PATH {
            return self.font_atlas_sdf_bind.clone();
        }
        if is_font_texture_path(path) {
            return self
                .custom_fonts
                .get(path)
                .and_then(|font| font.sdf_bind.clone());
        }
        None
    }

    /// Returns the bind group for a draw: the white texture for `texture == 0`
    /// or the white sentinel path, the built-in glyph atlas for the atlas
    /// sentinel path, the per-custom-font bind for a `font://...` path, otherwise
    /// the cached (or freshly loaded) asset at `paths[texture - 1]`.
    pub(super) fn bind_group_for(
        &mut self,
        texture: u32,
        paths: &[String],
        handles: &[AssetHandle],
    ) -> Option<wgpu::BindGroup> {
        if texture == 0 {
            return self.ensure_white();
        }
        // 越界不再 panic：隔壁 `sdf_bind_for` 一直是 `paths.get(...)?`，这里跟上同一口径。
        // 越界只可能来自宿主造 mesh 的内部 bug，返回 `None` 会让它**进 `dropped_draws` 并留下告警**
        // （见 `passes.rs` 的记账）——比整帧崩掉更有信息量，也不会把游戏一起带走。
        let path = paths.get(texture.checked_sub(1)? as usize)?;
        if path == WHITE_TEXTURE_PATH {
            return self.ensure_white();
        }
        if path == ATLAS_TEXTURE_PATH {
            return self.ensure_atlas();
        }
        if path == FONT_ATLAS_TEXTURE_PATH {
            return self.font_atlas_bind.clone();
        }
        if is_font_texture_path(path) {
            return self
                .custom_fonts
                .get(path)
                .and_then(|font| font.bind.clone());
        }
        // docs/54 §4.4：宿主给了资源身份就按**句柄**查缓存（O(1)，并保证"同路径恒同句柄"）。
        // 宿主手写 `SceneMesh` 时没有身份（`path_handles` 为空/短），此时回退到按路径线性扫描，
        // 行为与收敛前完全一致。
        if let Some(handle) = handles.get(texture as usize - 1).copied() {
            if let Some(slot) = self.texture_slots.get(&handle).copied()
                && let Some(entry) = self.textures.get(slot)
            {
                self.texture_identity_hits += 1;
                return Some(entry.bind_group.clone());
            }
            self.texture_identity_misses += 1;
        }
        // P0-③：按路径查缓存走 HashMap（O(1)）。`textures` 只追加，索引恒有效。
        if let Some(&slot) = self.texture_paths.get(path.as_str()) {
            if let Some(handle) = handles.get(texture as usize - 1).copied() {
                self.texture_slots.insert(handle, slot);
            }
            return Some(self.textures[slot].bind_group.clone());
        }
        let resolved = self
            .project_root
            .as_ref()
            .map(|root| root.join(path))
            .unwrap_or_else(|| PathBuf::from(path));
        // P0-②：MISS → 异步请求（幂等）并收走本轮完成的解码产物上传。
        // Async 下若本路径未就绪，先回退白贴图，就绪后后续帧自动命中真实贴图；
        // Sync 下请求同步完成、同一调用内即命中，golden 逐字节不变。
        self.async_loader.request_texture(path, &resolved);
        let drained = self.async_loader.drain_ready();
        if !drained.is_empty() {
            self.upload_decoded(drained);
        }
        if let Some(&slot) = self.texture_paths.get(path.as_str()) {
            if let Some(handle) = handles.get(texture as usize - 1).copied() {
                self.texture_slots.insert(handle, slot);
            }
            return Some(self.textures[slot].bind_group.clone());
        }
        tracing::debug!(target: "render", path = %path, "[render] texture not ready yet; white fallback this frame");
        self.ensure_white()
    }

    pub(super) fn ensure_white(&mut self) -> Option<wgpu::BindGroup> {
        if let Some(white) = &self.white {
            return Some(white.clone());
        }
        let asset =
            TextureAsset::from_rgba8(1, 1, vec![255, 255, 255, 255], TextureColorSpace::Srgb)
                .ok()?;
        let bind = self.upload_texture(&asset, "rustlua-white-1x1")?;
        self.white = Some(bind.clone());
        Some(bind)
    }

    fn ensure_atlas(&mut self) -> Option<wgpu::BindGroup> {
        if let Some(atlas) = &self.atlas {
            return Some(atlas.clone());
        }
        let built_in = TextAtlas::built_in();
        let asset = TextureAsset::from_rgba8(
            built_in.width(),
            built_in.height(),
            built_in.pixels().to_vec(),
            TextureColorSpace::Srgb,
        )
        .ok()?;
        let bind = self.upload_texture(&asset, "rustlua-builtin-text-atlas")?;
        self.atlas = Some(bind.clone());
        Some(bind)
    }

    /// Uploads the CJK font atlas texture when its version changed.
    pub(super) fn ensure_font_atlas(&mut self, font: &FontAtlasPixels) -> Option<wgpu::BindGroup> {
        if font.pixels.is_empty() {
            // 空像素 = 上游声明"自上次上传未变化"。此时若本渲染器从未上传过该
            // 版本（新渲染器 / 上次版本被空网格帧消费），不能续用旧 bind（内容
            // 不匹配），也不得以空数据 write_texture 整张图集（wgpu 校验崩溃）；
            // 返回 None 让调用方跳过本帧 SDF 绘制，等下一帧全量像素再上传。
            return if self.font_atlas_uploaded_version == font.version {
                self.font_atlas_bind.clone()
            } else {
                None
            };
        }
        let pixels_hash = hash_bytes(&font.pixels);
        if font.version == self.font_atlas_uploaded_version
            && pixels_hash == self.font_atlas_uploaded_hash
        {
            return self.font_atlas_bind.clone();
        }
        // 上传前仍复核像素量与纹理面积一致（纵深防御；上游协议破坏时降级为
        // 跳过本帧，而不是 wgpu 校验 panic 闪退）。
        if (font.pixels.len() as u64) != (font.width as u64 * font.height as u64 * 4) {
            tracing::warn!(
                target: "render",
                expected = font.width as u64 * font.height as u64 * 4,
                actual = font.pixels.len() as u64,
                "[render] font atlas pixel size mismatch; skipping upload this frame"
            );
            return None;
        }
        // SDF 距离场不能走 sRGB 解码（会非线性化距离斜坡），用 Rgba8Unorm。
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let texture = if self.font_atlas_texture.as_ref().is_some_and(|texture| {
            let size = texture.size();
            size.width == font.width && size.height == font.height
        }) {
            self.font_atlas_texture.as_ref().unwrap().clone()
        } else {
            self.font_atlas_bind = None;
            self.font_atlas_sdf_bind = None;
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("rustlua-font-atlas"),
                size: wgpu::Extent3d {
                    width: font.width,
                    height: font.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.font_atlas_texture = Some(texture.clone());
            texture
        };
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &font.pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(font.width * 4),
                rows_per_image: Some(font.height),
            },
            wgpu::Extent3d {
                width: font.width,
                height: font.height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-font-atlas-bind"),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.font_atlas_uploaded_version = font.version;
        self.font_atlas_uploaded_hash = pixels_hash;
        self.font_atlas_bind = Some(bind_group.clone());
        // SDF 文本用 Linear 采样绑定（同一纹理，不同采样器）。
        let sdf_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-font-atlas-sdf-bind"),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sdf_sampler),
                },
            ],
        });
        self.font_atlas_sdf_bind = Some(sdf_bind);
        Some(bind_group)
    }

    /// Uploads/re-binds one per-Text custom font atlas when its version changed.
    /// Mirrors [`Self::ensure_font_atlas`] but keyed by the namespaced `font://...`
    /// id, so each custom font samples its own SDF texture.
    pub(super) fn ensure_custom_font(&mut self, key: &str, font: &FontAtlasPixels) -> Option<()> {
        if font.pixels.is_empty() {
            // 空像素 = 上游声明"自上次上传未变化"。与默认图集同一防线：只有当本
            // 渲染器**已经上传过该版本**时才可以续用现有 bind。否则（新渲染器、
            // 或上一帧是空网格帧而被整体跳过）必须丢弃过期 bind，让本帧跳过该字
            // 体的 SDF 绘制，等下一次全量像素到达再上传——否则会永久画出上一次
            // 的字形，表现为"改了文案/字号却一直显示旧字"或整段缺字。
            let stale = self
                .custom_fonts
                .get(key)
                .is_none_or(|entry| entry.uploaded_version != font.version);
            if stale && let Some(entry) = self.custom_fonts.get_mut(key) {
                let had_bind = entry.sdf_bind.is_some();
                entry.bind = None;
                entry.sdf_bind = None;
                if had_bind {
                    tracing::debug!(
                        target: "render",
                        key,
                        version = font.version,
                        uploaded = entry.uploaded_version,
                        "[render] custom font atlas version changed without pixels; \
                         dropping stale bind for this frame"
                    );
                }
            }
            return Some(());
        }
        let pixels_hash = hash_bytes(&font.pixels);
        if self.custom_fonts.get(key).is_some_and(|entry| {
            entry.uploaded_version == font.version && entry.uploaded_hash == pixels_hash
        }) {
            return Some(());
        }
        // 上传前复核像素量与纹理面积一致（纵深防御，见 ensure_font_atlas）。
        if (font.pixels.len() as u64) != (font.width as u64 * font.height as u64 * 4) {
            tracing::warn!(
                target: "render",
                key,
                expected = font.width as u64 * font.height as u64 * 4,
                actual = font.pixels.len() as u64,
                "[render] custom font atlas pixel size mismatch; skipping upload this frame"
            );
            return Some(());
        }
        // SDF 距离场不能走 sRGB 解码，用 Rgba8Unorm（与默认图集一致）。
        let format = wgpu::TextureFormat::Rgba8Unorm;
        // 尺寸未变则复用现有纹理（只重写像素）；尺寸变了重建并丢弃旧 bind。
        let reuse = self.custom_fonts.get(key).and_then(|entry| {
            let texture = entry.texture.as_ref()?;
            let size = texture.size();
            (size.width == font.width && size.height == font.height).then(|| texture.clone())
        });
        let texture = match reuse {
            Some(texture) => texture,
            None => self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(&format!(
                    "rustlua-custom-font-{}",
                    font_asset_from_texture_path(key)
                )),
                size: wgpu::Extent3d {
                    width: font.width,
                    height: font.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            }),
        };
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &font.pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(font.width * 4),
                rows_per_image: Some(font.height),
            },
            wgpu::Extent3d {
                width: font.width,
                height: font.height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-custom-font-bind"),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let sdf_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-custom-font-sdf-bind"),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sdf_sampler),
                },
            ],
        });
        self.custom_fonts.insert(
            key.to_owned(),
            CustomFontUpload {
                texture: Some(texture),
                bind: Some(bind),
                sdf_bind: Some(sdf_bind),
                uploaded_version: font.version,
                uploaded_hash: pixels_hash,
            },
        );
        Some(())
    }

    /// 把一个**已经在内存里**的 RGBA8 贴图登记进 2D 贴图缓存。
    ///
    /// 为什么需要这个入口：2D 贴图原本只有一条来路——`texture_paths` 里查不到就按路径
    /// `std::fs::read`（就是下面 `bind_group_for` 的末段）。web/微信上没有文件系统，那条路必然
    /// 落到白兜底，于是"图集/精灵能不能画对"在浏览器里**无从验证**：画出来是白的，而白色恰好
    /// 也是"什么都没有"的样子。有了它，宿主（web 运行时、图集打包器、探针）可以把已经在内存
    /// 里的像素交进来——之后照常把 `key` 放进 `SceneMesh::paths`、用 1-based 下标引用，
    /// 解析路径一行都不用改。
    ///
    /// 同一个 `key` 重复上传是**原地替换**（槽位不变）：已经按资源身份缓存过该槽位的绘制仍然
    /// 有效。这对流式图集是必须的——图集更新不该让所有已缓存的身份失效。
    pub fn upload_texture_rgba8(
        &mut self,
        key: &str,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> anyhow::Result<u32> {
        let asset = TextureAsset::from_rgba8(width, height, rgba.to_vec(), TextureColorSpace::Srgb)
            .map_err(|error| {
                anyhow::anyhow!("RGBA8 贴图尺寸不合法（{width}x{height}）: {error}")
            })?;
        self.register_texture_asset(key, &asset)
    }

    /// PNG（或打包格式）**字节** → 2D 贴图缓存的公开入口：web 端手里只有字节，没有文件可读，
    /// 解码是唯一的路。语义与 [`Self::upload_texture_rgba8`] 完全相同（含原地替换）。
    pub fn upload_texture_png(&mut self, key: &str, bytes: &[u8]) -> anyhow::Result<u32> {
        let asset = TextureAsset::from_png_bytes(bytes, TextureColorSpace::Srgb)
            .map_err(|error| anyhow::anyhow!("贴图字节解码失败（{key}）: {error}"))?;
        self.register_texture_asset(key, &asset)
    }

    /// 两个公开入口的共用实现：上传 GPU 资源 + 按 `key` 登记/替换槽位，返回槽位号。
    fn register_texture_asset(&mut self, key: &str, asset: &TextureAsset) -> anyhow::Result<u32> {
        let bind_group = self
            .upload_texture(asset, key)
            .ok_or_else(|| anyhow::anyhow!("贴图 GPU 上传失败（{key}）"))?;
        let slot = if let Some(&slot) = self.texture_paths.get(key) {
            self.textures[slot].bind_group = bind_group;
            self.textures[slot].bytes = u64::from(asset.width()) * u64::from(asset.height()) * 4;
            slot
        } else {
            self.textures.push(LoadedTexture {
                bind_group,
                bytes: u64::from(asset.width()) * u64::from(asset.height()) * 4,
            });
            let slot = self.textures.len() - 1;
            self.texture_paths.insert(key.to_owned(), slot);
            slot
        };
        tracing::info!(
            target: "render",
            key,
            width = asset.width(),
            height = asset.height(),
            slot,
            "[render] 2D texture uploaded from memory"
        );
        Ok(slot as u32)
    }

    pub(super) fn upload_texture(
        &self,
        asset: &TextureAsset,
        label: &str,
    ) -> Option<wgpu::BindGroup> {
        let format = match asset.color_space() {
            TextureColorSpace::Srgb => wgpu::TextureFormat::Rgba8UnormSrgb,
            TextureColorSpace::Linear => wgpu::TextureFormat::Rgba8Unorm,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: asset.width(),
                height: asset.height(),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            asset.pixels(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(asset.bytes_per_row()),
                rows_per_image: Some(asset.height()),
            },
            wgpu::Extent3d {
                width: asset.width(),
                height: asset.height(),
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        Some(bind_group)
    }
}
