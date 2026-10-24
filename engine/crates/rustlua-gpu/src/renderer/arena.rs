//! `Renderer` 的逐帧 uniform/storage arena 与材质绑定组（WP12 阶段 4b 从 `renderer/mod.rs`
//! 原样搬出）：`push_frame_uniform` / `push_frame_storage` / `material_bind_group`。
//!
//! 同一类型、同 crate 的另一个固有 `impl` 块；可见性口径见 `gpu_mesh.rs` 的文件头。

use rustlua_math::math3d::Mat4;

use super::{GpuMaterial, Renderer, joint_palette_data, next_2d_vertex_capacity};

/// P3-R02：PBR 逐实例法线矩阵在帧级环形缓冲里的位置。绘制时作为顶点缓冲槽 1 绑定，
/// 与模型矩阵的存储绑定共用同一块缓冲、不同偏移。
pub(super) struct NormalMatrixSlice {
    pub buffer: wgpu::Buffer,
    pub offset: u64,
    pub size: u64,
}

impl Renderer {
    /// 本设备上"存储缓冲"用法位：没有存储缓冲的设备（WebGL2）返回空位集。
    ///
    /// 规则只该有一处：构造期（`construct.rs` 的 `instance_arena`）与这里的扩容都按它取，
    /// 免得出现"构造时没带 STORAGE、扩容时又带上了"这种只在扩容后才炸的分叉。
    fn storage_usage(&self) -> wgpu::BufferUsages {
        if self.storage_capable {
            wgpu::BufferUsages::STORAGE
        } else {
            wgpu::BufferUsages::empty()
        }
    }

    /// P0-①：把一块 uniform 数据写入材质/调色板帧级环形缓冲，返回
    /// （所属缓冲句柄, 起始偏移, 字节数）。句柄单独返回是为了覆盖"本帧中途扩容"
    /// 的情形：扩容前已写出的块仍在旧缓冲里，旧句柄克隆保证它们存活且指向正确。
    fn push_frame_uniform(&mut self, contents: &[u8]) -> (wgpu::Buffer, u64, u64) {
        let mask = self.uniform_align - 1;
        let size = contents.len() as u64;
        let mut offset = self.material_arena_used.wrapping_add(mask) & !mask;
        if offset + size > self.material_arena.size() {
            let grown = (self.material_arena.size() * 2).max(offset + size);
            self.material_arena = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rustlua-material3d-frame-arena"),
                size: grown,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            offset = 0;
        }
        self.queue
            .write_buffer(&self.material_arena, offset, contents);
        self.material_arena_used = offset + size;
        (self.material_arena.clone(), offset, size)
    }

    /// P0-①：实例模型矩阵帧级环形缓冲的写入端，语义与 [`Self::push_frame_uniform`] 相同。
    fn push_frame_storage(&mut self, contents: &[u8]) -> (wgpu::Buffer, u64, u64) {
        let mask = self.storage_align - 1;
        let size = contents.len() as u64;
        let mut offset = self.instance_arena_used.wrapping_add(mask) & !mask;
        if offset + size > self.instance_arena.size() {
            let grown = (self.instance_arena.size() * 2).max(offset + size);
            self.instance_arena = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rustlua-instance-models-frame-arena"),
                size: grown,
                // 与构造期同一条规则：没有存储缓冲的设备上就不带 STORAGE 用法
                // （这条路径只在能力足够时才会被走到，但规则只该有一处）。
                usage: self.storage_usage()
                    | wgpu::BufferUsages::VERTEX
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            offset = 0;
        }
        self.queue
            .write_buffer(&self.instance_arena, offset, contents);
        self.instance_arena_used = offset + size;
        (self.instance_arena.clone(), offset, size)
    }

    /// Creates a material bind group carrying one primitive's file material,
    /// the joint palette (identity for static meshes, animated for skinned
    /// drawables), and the per-instance model matrices. P0-①：数据落在帧级环形
    /// 缓冲里按偏移引用，每次调用不再新建任何 GPU buffer。
    pub(super) fn material_bind_group(
        &mut self,
        material: GpuMaterial,
        palette: &[Mat4],
        models: &[Mat4],
        normals: bool,
    ) -> (wgpu::BindGroup, Option<NormalMatrixSlice>) {
        let material_data: [[f32; 4]; 5] = [
            material.base_color,
            [
                material.metallic,
                material.roughness,
                if material.skinned { 1.0 } else { 0.0 },
                0.0,
            ],
            // 自发光已乘强度（alpha 恒 0）。
            [
                material.emissive[0],
                material.emissive[1],
                material.emissive[2],
                0.0,
            ],
            [
                material.metallic_roughness.map_or(0.0, |_| 1.0),
                material.emissive_texture.map_or(0.0, |_| 1.0),
                material.normal_texture.map_or(0.0, |_| 1.0),
                material.normal_scale,
            ],
            [
                material.occlusion_texture.map_or(0.0, |_| 1.0),
                material.occlusion_strength,
                // P3-R02-4：alpha_mode 编码进 flags.z，阈值进 flags.w；旧着色器不读这一行。
                match material.alpha_mode {
                    rustlua_render::mesh3d::MaterialAlphaMode::Opaque => 0.0,
                    rustlua_render::mesh3d::MaterialAlphaMode::Mask => 1.0,
                    rustlua_render::mesh3d::MaterialAlphaMode::Blend => 2.0,
                },
                material.alpha_cutoff,
            ],
        ];
        let (material_buffer, material_offset, material_size) =
            self.push_frame_uniform(bytemuck::cast_slice(&material_data));
        let (palette_buffer, palette_offset, palette_size) =
            if material.skinned && !palette.is_empty() {
                self.push_frame_uniform(bytemuck::cast_slice(&joint_palette_data(palette)))
            } else {
                let size = self.identity_palette.size();
                (self.identity_palette.clone(), 0, size)
            };
        let mut instance_data = Vec::with_capacity(models.len() * 4);
        for model in models {
            instance_data.extend_from_slice(&model.columns);
        }
        let (instance_buffer, instance_offset, instance_size) =
            self.push_frame_storage(bytemuck::cast_slice(&instance_data));
        // P3-R02：法线矩阵必须按每个实例各算一份（非均匀缩放下模型矩阵的 3x3 不能直接当
        // 法线变换用）。只在 PBR 路径写入；旧路径一个字节都不多写，`pbr=off` 的旧 golden
        // 因此仍逐字节一致。走顶点缓冲而不是新增存储绑定：多一个布局条目会明显拉长启动期
        // 管线编译。
        let normal_slice = if normals {
            let mut normal_data = Vec::with_capacity(models.len() * 16);
            for model in models {
                normal_data.extend_from_slice(&super::pbr::normal_matrix(*model).columns);
            }
            let (buffer, offset, size) =
                self.push_frame_storage(bytemuck::cast_slice(&normal_data));
            Some(NormalMatrixSlice {
                buffer,
                offset,
                size,
            })
        } else {
            None
        };
        let albedo = material
            .albedo
            .and_then(|index| self.albedo_textures.get(index as usize))
            .map_or(&self.white_albedo_view, |cached| &cached.view);
        let metallic_roughness = material
            .metallic_roughness
            .and_then(|index| self.linear_textures.get(index as usize))
            .map_or(&self.white_albedo_view, |cached| &cached.view);
        let emissive = material
            .emissive_texture
            .and_then(|index| self.albedo_textures.get(index as usize))
            .map_or(&self.white_albedo_view, |cached| &cached.view);
        // 缺省视图可以是白图：着色器用 `flags.z` 判断是否采样法线贴图，
        // 没有贴图时走几何法线，不会读到这里的占位内容。
        let normal = material
            .normal_texture
            .and_then(|index| self.linear_textures.get(index as usize))
            .map_or(&self.white_albedo_view, |cached| &cached.view);
        let occlusion = material
            .occlusion_texture
            .and_then(|index| self.linear_textures.get(index as usize))
            .map_or(&self.white_albedo_view, |cached| &cached.view);
        fn buffer_binding<'a>(
            buffer: &'a wgpu::Buffer,
            offset: u64,
            size: u64,
        ) -> wgpu::BindingResource<'a> {
            wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset,
                size: std::num::NonZeroU64::new(size),
            })
        }
        let entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer_binding(&material_buffer, material_offset, material_size),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: buffer_binding(&palette_buffer, palette_offset, palette_size),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: buffer_binding(&instance_buffer, instance_offset, instance_size),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(albedo),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(&self.albedo_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(metallic_roughness),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(emissive),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(normal),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::TextureView(occlusion),
            },
        ];
        if let Some(normal) = &normal_slice {
            debug_assert_eq!(normal.size, models.len() as u64 * 64);
        }
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-material3d-frame-bind-group"),
            layout: &self._material_bind_group_layout,
            entries: &entries,
        });
        (bind_group, normal_slice)
    }
}

impl Renderer {
    /// 确保 2D 顶点缓冲能容纳 `required` 字节；不足则按 2 的幂扩容。
    ///
    /// 返回扩容后的容量。2D 顶点缓冲不参与任何 bind group（只 `set_vertex_buffer`），
    /// 因此中途重建是安全的。
    pub(super) fn ensure_2d_vertex_capacity(&mut self, required: u64) -> u64 {
        self.vertex_bytes_peak = self.vertex_bytes_peak.max(required);
        if required <= self.vertex_buffer.size() {
            return self.vertex_buffer.size();
        }
        let capacity = next_2d_vertex_capacity(required);
        tracing::warn!(
            target: "render",
            required,
            previous = self.vertex_buffer.size(),
            capacity,
            growths = self.vertex_buffer_growths + 1,
            "[runtime] 2D vertex buffer grew: {} -> {} bytes (required {})",
            self.vertex_buffer.size(),
            capacity,
            required
        );
        self.vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-vertex-buffer"),
            size: capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.vertex_buffer_growths += 1;
        capacity
    }
}
