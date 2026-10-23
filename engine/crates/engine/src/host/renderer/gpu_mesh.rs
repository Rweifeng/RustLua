//! `Renderer` 的 GPU 网格侧（WP12 阶段 4b 从 `renderer/mod.rs` 原样搬出）：
//! 按资源身份取/建网格缓存、异步加载请求、几何与 albedo 上传、地面平面替身。
//!
//! `Renderer` 仍定义在 `renderer/mod.rs`（字段私有，搬结构体会逼着几十条字段放宽可见性），
//! 这里只是同 crate 兄弟模块里的另一个固有 `impl` 块；私有方法被父模块调用时按编译器
//! 指出的那几条加 `pub(super)`（计划 §3.1.4）。

use rustlua_assets::asset_handle::AssetHandle;
use rustlua_render::mesh3d::{
    GROUND_PLANE_SENTINEL, MaterialAsset, MaterialTexture, MeshAsset, TangentBasis,
};
use rustlua_render::texture::{TextureAsset, TextureColorSpace};
use std::path::PathBuf;
use wgpu::util::DeviceExt;

use super::{
    CachedMaterialTexture, CachedMesh, CachedPrimitive, GpuGeometry, GpuMaterial, LoadedTexture,
    MaterialTextureIndices, Renderer, compute_mesh_bounds, hash_bytes,
};

/// 五个贴图槽的路径签名：只有签名变化才重新解码/上传贴图。
fn texture_signature(material: &MaterialAsset) -> [Option<String>; 5] {
    [
        material
            .base_color_texture
            .as_ref()
            .and_then(|t| t.path.clone()),
        material
            .metallic_roughness_texture
            .as_ref()
            .and_then(|t| t.path.clone()),
        material
            .emissive_texture
            .as_ref()
            .and_then(|t| t.path.clone()),
        material
            .normal_texture
            .as_ref()
            .and_then(|t| t.path.clone()),
        material
            .occlusion_texture
            .as_ref()
            .and_then(|t| t.path.clone()),
    ]
}

/// glTF 没有给材质时的缺省值：与 R02 之前的网格默认材质逐字段一致。
fn default_mesh_material() -> MaterialAsset {
    MaterialAsset {
        base_color: [0.55, 0.7, 1.0, 1.0],
        metallic: 0.0,
        roughness: 0.6,
        ..MaterialAsset::default()
    }
}

impl Renderer {
    /// Returns the cached geometry + materials for a mesh path, loading it from
    /// the project on first use. Missing or unloadable meshes are skipped with
    /// a warning so a bad reference never crashes the renderer.
    /// docs/54 §4.4：按**资源身份**取网格缓存。
    ///
    /// 有句柄时先查身份表（O(1)），未命中才按路径加载并记下映射；路径键的 `mesh_cache` 保持不变，
    /// 因此"同一网格被两个路径引用"也不会加载两遍。没有句柄的调用（宿主手写 `Scene3d`）仍按路径。
    pub(super) fn cached_mesh(
        &mut self,
        handle: Option<AssetHandle>,
        path: &str,
    ) -> Option<&CachedMesh> {
        if let Some(handle) = handle {
            // 句柄只在**同一个** `RuntimeScene` 的 `AssetStore` 里唯一；编辑器重建运行时
            // 会从 (0, 1) 重新发号，而渲染器寿命更长。必须核对映射路径仍是本次请求的
            // 路径，否则重建后会把旧网格当成新网格画出来（路径不同、句柄相同）。
            if let Some(mapped) = self.mesh_slots.get(&handle)
                && mapped.as_str() == path
                && self.mesh_cache.contains_key(mapped.as_str())
            {
                self.mesh_identity_hits += 1;
                let key = mapped.clone();
                return self.mesh_cache.get(&key);
            }
            self.mesh_identity_misses += 1;
            if !self.mesh_cache.contains_key(path) {
                self.request_mesh_load(path);
            }
            if self.mesh_cache.contains_key(path) {
                self.mesh_slots.insert(handle, path.to_owned());
            }
            return self.mesh_cache.get(path);
        }
        // 没有身份：旧路径（行为与收敛前一致）。
        if !self.mesh_cache.contains_key(path) {
            self.request_mesh_load(path);
        }
        self.mesh_cache.get(path)
    }

    /// 把已解析的 `MeshAsset` 转成 GPU 侧 `CachedMesh`（上传几何 + albedo）。
    /// 同步与异步路径共用这一份转换，保证两条路径的 GPU 产物完全一致。
    fn cached_from_meshes(&mut self, path: &str, meshes: &[MeshAsset]) -> Option<CachedMesh> {
        let mut primitives = Vec::new();
        for mesh in meshes {
            for primitive in &mesh.primitives {
                if primitive.tangent_basis == TangentBasis::Degenerate {
                    tracing::warn!(
                        target: "render",
                        path = %path,
                        "[render] tangent generation failed (degenerate or mirrored UVs); \
                         normal maps fall back to flat normals"
                    );
                }
                let source = mesh.materials.get(primitive.material.unwrap_or(usize::MAX));
                let material = source.cloned().unwrap_or_else(default_mesh_material);
                let textures = self.upload_embedded_textures(&material);
                primitives.push(CachedPrimitive {
                    geometry: self.upload_geometry(&primitive.vertices, &primitive.indices),
                    // docs/54 §4.3：glTF 的 PBR 参数装进**统一的** `MaterialAsset`，`.material`
                    // 覆盖与内嵌材质因此走同一条 GPU 解释路径。
                    material: GpuMaterial::from_material(&material, textures, false),
                    bounds: compute_mesh_bounds(&primitive.vertices),
                });
            }
        }
        if primitives.is_empty() {
            tracing::warn!(target: "render", path = %path, "[render] 3D mesh has no primitives; skipped");
            return None;
        }
        Some(CachedMesh { primitives })
    }

    /// P0-②：网格 MISS 的异步请求 + 收完成品上传。Async 下资源后台解码、就绪后入缓存；
    /// Sync 下 request 同步完成、同一调用内即入缓存。
    fn request_mesh_load(&mut self, path: &str) {
        if path == GROUND_PLANE_SENTINEL {
            // 地面哨兵不走文件加载（也没有异步文件）；直接同步合成并缓存。
            self.mesh_cache
                .insert(path.to_owned(), self.ground_plane_cached_mesh());
            return;
        }
        let resolved = self
            .project_root
            .as_ref()
            .map(|root| root.join(path))
            .unwrap_or_else(|| PathBuf::from(path));
        self.async_loader.request_gltf(path, &resolved);
        let drained = self.async_loader.drain_ready();
        self.upload_decoded(drained);
    }

    /// P0-②：主线程把本轮完成的异步解码产物做 GPU 上传并写入缓存。
    ///
    /// 这是"后台解码、主线程小步上传"的上传侧：`wgpu` 的提交本身不阻塞，真正卡主线程的
    /// CPU 解码已全部下移到 worker。失败资源只记 warning（调用方已有占位回退），不崩溃。
    pub(super) fn upload_decoded(
        &mut self,
        assets: Vec<rustlua_render::async_loader::DecodedAsset>,
    ) {
        for asset in assets {
            if let Some(error) = &asset.error {
                tracing::warn!(
                    target: "render",
                    path = %asset.key,
                    error = %error,
                    "[render] async asset load failed; fallback used"
                );
                continue;
            }
            match asset.kind {
                rustlua_render::async_loader::LoadKind::TexturePng => {
                    if let Some(texture) = asset.texture {
                        if self.texture_paths.contains_key(asset.key.as_str()) {
                            continue; // 已上传过（并发请求去重后的兜底）。
                        }
                        match self.upload_texture(&texture, &asset.key) {
                            Some(bind) => {
                                self.texture_paths
                                    .insert(asset.key.clone(), self.textures.len());
                                self.textures.push(LoadedTexture {
                                    bind_group: bind,
                                    bytes: u64::from(texture.width())
                                        * u64::from(texture.height())
                                        * 4,
                                });
                                tracing::info!(
                                    target: "render",
                                    path = %asset.key,
                                    dimensions = ?(texture.width(), texture.height()),
                                    "[render] texture loaded (async)"
                                );
                            }
                            None => {
                                tracing::warn!(
                                    target: "render",
                                    path = %asset.key,
                                    "[render] texture GPU upload failed; fallback used"
                                );
                            }
                        }
                    }
                }
                rustlua_render::async_loader::LoadKind::GltfMesh => {
                    if let Some(meshes) = asset.meshes {
                        if self.mesh_cache.contains_key(&asset.key) {
                            continue;
                        }
                        if let Some(cached) = self.cached_from_meshes(&asset.key, &meshes) {
                            self.mesh_cache.insert(asset.key.clone(), cached);
                            tracing::info!(
                                target: "render",
                                path = %asset.key,
                                "[render] 3D mesh cached (async)"
                            );
                        }
                    }
                }
            }
        }
    }

    /// 上传 glTF 内嵌材质贴图。base color / emissive 走 sRGB，metallic-roughness 走线性，
    /// 否则 ORM 通道会被 gamma 曲线扭曲。
    fn upload_embedded_textures(&mut self, material: &MaterialAsset) -> MaterialTextureIndices {
        let upload = |renderer: &mut Self, texture: &Option<MaterialTexture>, space| {
            texture
                .as_ref()
                .and_then(|texture| texture.image.as_ref())
                .map(|image| {
                    renderer.upload_material_texture(&image.rgba, image.width, image.height, space)
                })
        };
        MaterialTextureIndices {
            base_color: upload(self, &material.base_color_texture, TextureColorSpace::Srgb),
            metallic_roughness: upload(
                self,
                &material.metallic_roughness_texture,
                TextureColorSpace::Linear,
            ),
            emissive: upload(self, &material.emissive_texture, TextureColorSpace::Srgb),
            // 法线与遮蔽是数据贴图：必须走线性，否则解码出的法线方向会被 gamma 拉歪。
            normal: upload(self, &material.normal_texture, TextureColorSpace::Linear),
            occlusion: upload(self, &material.occlusion_texture, TextureColorSpace::Linear),
        }
    }

    /// 上传 RGBA8 材质贴图并返回槽位。内容相同的贴图只上传一次（glTF 多个图元常共用
    /// 同一张 ORM 图）；sRGB 与线性分别落在 `albedo_textures` / `linear_textures`。
    fn upload_material_texture(
        &mut self,
        rgba: &[u8],
        width: u32,
        height: u32,
        color_space: TextureColorSpace,
    ) -> u32 {
        let width = width.max(1);
        let height = height.max(1);
        let srgb = color_space == TextureColorSpace::Srgb;
        let key = (hash_bytes(rgba), width, height, srgb);
        if let Some(&index) = self.texture_content_cache.get(&key) {
            return index;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rustlua-material-texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: if srgb {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            },
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
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let cached = CachedMaterialTexture {
            texture,
            view,
            bytes: u64::from(width) * u64::from(height) * 4,
        };
        let index = if srgb {
            self.albedo_textures.push(cached);
            (self.albedo_textures.len() - 1) as u32
        } else {
            self.linear_textures.push(cached);
            (self.linear_textures.len() - 1) as u32
        };
        self.texture_content_cache.insert(key, index);
        tracing::info!(
            target: "render",
            index,
            width,
            height,
            srgb,
            "[render] 3D material texture uploaded"
        );
        index
    }

    fn resolve_project_path(&self, path: &str) -> PathBuf {
        self.project_root
            .as_ref()
            .map(|root| root.join(path))
            .unwrap_or_else(|| PathBuf::from(path))
    }

    /// 取（并缓存）`.material` 覆盖。解析失败只告警一次并返回 `None`，由绘制侧回退到
    /// glTF 内嵌材质——"覆盖文件坏了"绝不能变成"这个物体不见了"。
    pub(super) fn material_override(&mut self, path: &str) -> Option<GpuMaterial> {
        if let Some(cached) = self.material_cache.get(path) {
            return *cached;
        }
        let resolved = self.load_material_override(path);
        self.material_cache.insert(path.to_owned(), resolved);
        resolved
    }

    fn load_material_override(&mut self, path: &str) -> Option<GpuMaterial> {
        let resolved = self.resolve_project_path(path);
        let bytes = match rustlua_infra::vfs::read(&resolved) {
            Ok(bytes) => bytes,
            Err(reason) => {
                tracing::warn!(
                    target: "render",
                    material = %path,
                    error = %reason,
                    "[render] .material load failed; glTF material used"
                );
                return None;
            }
        };
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) => {
                tracing::warn!(
                    target: "render",
                    material = %path,
                    error = %error,
                    "[render] .material is not valid UTF-8; glTF material used"
                );
                return None;
            }
        };
        let material = match MaterialAsset::from_text(path, &text) {
            Ok(material) => material,
            Err(error) => {
                tracing::warn!(
                    target: "render",
                    material = %path,
                    error = %error,
                    "[render] .material rejected; glTF material used"
                );
                return None;
            }
        };
        let textures = self.load_override_textures(path, &material);
        Some(GpuMaterial::from_material(&material, textures, false))
    }

    fn load_override_textures(
        &mut self,
        material_path: &str,
        material: &MaterialAsset,
    ) -> MaterialTextureIndices {
        MaterialTextureIndices {
            base_color: self.load_override_texture(
                material_path,
                &material.base_color_texture,
                TextureColorSpace::Srgb,
            ),
            metallic_roughness: self.load_override_texture(
                material_path,
                &material.metallic_roughness_texture,
                TextureColorSpace::Linear,
            ),
            emissive: self.load_override_texture(
                material_path,
                &material.emissive_texture,
                TextureColorSpace::Srgb,
            ),
            normal: self.load_override_texture(
                material_path,
                &material.normal_texture,
                TextureColorSpace::Linear,
            ),
            occlusion: self.load_override_texture(
                material_path,
                &material.occlusion_texture,
                TextureColorSpace::Linear,
            ),
        }
    }

    /// P3-E02：编辑器实时预览——用内存中的材质替换某个 `.material` 路径的解析结果，
    /// **不写盘、不改场景文档**。传 `None` 清除预览，下一次取用重新读盘。
    ///
    /// 预览条目直接住进 `material_cache`，因此绘制侧与从磁盘加载的覆盖走完全相同的
    /// `GpuMaterial` 路径（同一台解析器、同一套贴图/管线逻辑）。贴图按五个路径签名复用：
    /// 拖数值滑杆只重建 `GpuMaterial`，不做 PNG 解码/纹理上传。
    pub fn set_material_preview(&mut self, preview: Option<(&str, &MaterialAsset)>) {
        let Some((path, material)) = preview else {
            if let Some(previous) = self.material_preview_path.take() {
                self.material_cache.remove(&previous);
            }
            self.material_preview_textures = None;
            self.material_preview_indices = None;
            return;
        };
        let textures = texture_signature(material);
        if self.material_preview_path.as_deref() != Some(path) {
            if let Some(previous) = self.material_preview_path.take() {
                self.material_cache.remove(&previous);
            }
            self.material_preview_textures = None;
            self.material_preview_indices = None;
        }
        let indices = match self.material_preview_indices {
            Some(indices) if self.material_preview_textures.as_ref() == Some(&textures) => indices,
            _ => {
                let indices = self.load_override_textures(path, material);
                self.material_preview_textures = Some(textures);
                self.material_preview_indices = Some(indices);
                indices
            }
        };
        self.material_cache.insert(
            path.to_owned(),
            Some(GpuMaterial::from_material(material, indices, false)),
        );
        self.material_preview_path = Some(path.to_owned());
    }

    /// 单张贴图失败只丢这一张（回退白图），材质其余字段照常生效。
    fn load_override_texture(
        &mut self,
        material_path: &str,
        texture: &Option<MaterialTexture>,
        color_space: TextureColorSpace,
    ) -> Option<u32> {
        let path = texture.as_ref()?.path.as_deref()?;
        let resolved = self.resolve_project_path(path);
        match TextureAsset::load_png(&resolved, color_space) {
            Ok(asset) => Some(self.upload_material_texture(
                asset.pixels(),
                asset.width(),
                asset.height(),
                color_space,
            )),
            Err(error) => {
                tracing::warn!(
                    target: "render",
                    material = %material_path,
                    texture = %path,
                    error = %error,
                    "[render] .material texture load failed; default texture used"
                );
                None
            }
        }
    }

    /// Synthesizes the built-in ground receiver plane for the
    /// `__ground_plane__` MeshRenderer sentinel so shadows have a surface.
    fn ground_plane_cached_mesh(&self) -> CachedMesh {
        let vertices = [
            rustlua_render::mesh3d::MeshVertex {
                position: [-4.0, 0.0, -4.0],
                normal: [0.0, 1.0, 0.0],
                texcoord: [0.0, 0.0],
                tangent: [1.0, 0.0, 0.0, -1.0],
                joints: [0, 0, 0, 0],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
            rustlua_render::mesh3d::MeshVertex {
                position: [4.0, 0.0, -4.0],
                normal: [0.0, 1.0, 0.0],
                texcoord: [1.0, 0.0],
                tangent: [1.0, 0.0, 0.0, -1.0],
                joints: [0, 0, 0, 0],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
            rustlua_render::mesh3d::MeshVertex {
                position: [4.0, 0.0, 4.0],
                normal: [0.0, 1.0, 0.0],
                texcoord: [1.0, 1.0],
                tangent: [1.0, 0.0, 0.0, -1.0],
                joints: [0, 0, 0, 0],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
            rustlua_render::mesh3d::MeshVertex {
                position: [-4.0, 0.0, 4.0],
                normal: [0.0, 1.0, 0.0],
                texcoord: [0.0, 1.0],
                tangent: [1.0, 0.0, 0.0, -1.0],
                joints: [0, 0, 0, 0],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
        ];
        let material = MaterialAsset {
            base_color: [0.25, 0.3, 0.38, 1.0],
            metallic: 0.0,
            roughness: 0.9,
            ..MaterialAsset::default()
        };
        CachedMesh {
            primitives: vec![CachedPrimitive {
                geometry: self.upload_geometry(&vertices, &[0, 2, 1, 0, 3, 2]),
                material: GpuMaterial::from_material(
                    &material,
                    MaterialTextureIndices::default(),
                    false,
                ),
                bounds: compute_mesh_bounds(&vertices),
            }],
        }
    }

    fn upload_geometry(
        &self,
        vertices: &[rustlua_render::mesh3d::MeshVertex],
        indices: &[u32],
    ) -> GpuGeometry {
        let vertex_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("rustlua-mesh3d-vertices"),
                contents: bytemuck::cast_slice(vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let index_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("rustlua-mesh3d-indices"),
                contents: bytemuck::cast_slice(indices),
                usage: wgpu::BufferUsages::INDEX,
            });
        let vertex_bytes = std::mem::size_of_val(vertices) as u64;
        let index_bytes = std::mem::size_of_val(indices) as u64;
        GpuGeometry {
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32,
            vertex_count: vertices.len() as u32,
            bytes: vertex_bytes + index_bytes,
        }
    }
}
