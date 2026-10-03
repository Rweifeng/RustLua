//! `Renderer` 的 GPU 网格侧（WP12 阶段 4b 从 `renderer/mod.rs` 原样搬出）：
//! 按资源身份取/建网格缓存、异步加载请求、几何与 albedo 上传、地面平面替身。
//!
//! `Renderer` 仍定义在 `renderer/mod.rs`（字段私有，搬结构体会逼着几十条字段放宽可见性），
//! 这里只是同 crate 兄弟模块里的另一个固有 `impl` 块；私有方法被父模块调用时按编译器
//! 指出的那几条加 `pub(super)`（计划 §3.1.4）。

use crate::core::{
    asset_handle::AssetHandle,
    mesh3d::{GROUND_PLANE_SENTINEL, MeshAsset},
};
use std::path::PathBuf;
use wgpu::util::DeviceExt;

use super::{
    CachedAlbedoTexture, CachedMesh, CachedPrimitive, GpuGeometry, LoadedTexture, Renderer,
    compute_mesh_bounds,
};

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
            if let Some(mapped) = self.mesh_slots.get(&handle)
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
                let material = mesh.materials.get(primitive.material.unwrap_or(usize::MAX));
                let albedo = material
                    .and_then(|value| value.base_color_image.as_ref())
                    .map(|image| self.upload_albedo(&image.rgba, image.width, image.height));
                primitives.push(CachedPrimitive {
                    geometry: self.upload_geometry(&primitive.vertices, &primitive.indices),
                    // docs/54 §4.3：glTF 的 PBR 参数装进**统一的** `Material`（精灵/Quad 用的是同一个
                    // 类型的无光照变体），因此"缺省材质"只有一处定义。
                    material: crate::core::renderable::Material::pbr(
                        material
                            .map(|value| value.base_color_factor)
                            .unwrap_or([0.55, 0.7, 1.0, 1.0]),
                        // glTF 的底图内嵌在资产里（没有独立路径）：材质里记 `None`，真实贴图身份由
                        // 网格身份 + `albedo` 下标表达（批 I 的结论）。
                        None,
                        material.map(|value| value.metallic_factor).unwrap_or(0.0),
                        material.map(|value| value.roughness_factor).unwrap_or(0.6),
                    ),
                    albedo,
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
    pub(super) fn upload_decoded(&mut self, assets: Vec<crate::core::async_loader::DecodedAsset>) {
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
                crate::core::async_loader::LoadKind::TexturePng => {
                    if let Some(texture) = asset.texture {
                        if self.texture_paths.contains_key(asset.key.as_str()) {
                            continue; // 已上传过（并发请求去重后的兜底）。
                        }
                        match self.upload_texture(&texture, &asset.key) {
                            Some(bind) => {
                                self.texture_paths
                                    .insert(asset.key.clone(), self.textures.len());
                                self.textures.push(LoadedTexture { bind_group: bind });
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
                crate::core::async_loader::LoadKind::GltfMesh => {
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

    /// Uploads decoded base-color pixels (RGBA8) as an sRGB GPU texture and
    /// returns its index in `albedo_textures`.
    fn upload_albedo(&mut self, rgba: &[u8], width: u32, height: u32) -> u32 {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rustlua-albedo-texture"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
                bytes_per_row: Some(4 * width.max(1)),
                rows_per_image: Some(height.max(1)),
            },
            wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.albedo_textures
            .push(CachedAlbedoTexture { texture, view });
        tracing::info!(
            target: "render",
            index = self.albedo_textures.len() - 1,
            width,
            height,
            "[render] 3D albedo texture uploaded"
        );
        (self.albedo_textures.len() - 1) as u32
    }

    /// Synthesizes the built-in ground receiver plane for the
    /// `__ground_plane__` MeshRenderer sentinel so shadows have a surface.
    fn ground_plane_cached_mesh(&self) -> CachedMesh {
        let vertices = [
            crate::core::mesh3d::MeshVertex {
                position: [-4.0, 0.0, -4.0],
                normal: [0.0, 1.0, 0.0],
                texcoord: [0.0, 0.0],
                joints: [0, 0, 0, 0],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
            crate::core::mesh3d::MeshVertex {
                position: [4.0, 0.0, -4.0],
                normal: [0.0, 1.0, 0.0],
                texcoord: [1.0, 0.0],
                joints: [0, 0, 0, 0],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
            crate::core::mesh3d::MeshVertex {
                position: [4.0, 0.0, 4.0],
                normal: [0.0, 1.0, 0.0],
                texcoord: [1.0, 1.0],
                joints: [0, 0, 0, 0],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
            crate::core::mesh3d::MeshVertex {
                position: [-4.0, 0.0, 4.0],
                normal: [0.0, 1.0, 0.0],
                texcoord: [0.0, 1.0],
                joints: [0, 0, 0, 0],
                weights: [1.0, 0.0, 0.0, 0.0],
            },
        ];
        CachedMesh {
            primitives: vec![CachedPrimitive {
                geometry: self.upload_geometry(&vertices, &[0, 2, 1, 0, 3, 2]),
                material: crate::core::renderable::Material::pbr(
                    [0.25, 0.3, 0.38, 1.0],
                    None,
                    0.0,
                    0.9,
                ),
                albedo: None,
                bounds: compute_mesh_bounds(&vertices),
            }],
        }
    }

    fn upload_geometry(
        &self,
        vertices: &[crate::core::mesh3d::MeshVertex],
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
        GpuGeometry {
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32,
        }
    }
}
