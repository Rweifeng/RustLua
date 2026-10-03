//! `Renderer` 的逐帧 uniform/storage arena 与材质绑定组（WP12 阶段 4b 从 `renderer/mod.rs`
//! 原样搬出）：`push_frame_uniform` / `push_frame_storage` / `material_bind_group`。
//!
//! 同一类型、同 crate 的另一个固有 `impl` 块；可见性口径见 `gpu_mesh.rs` 的文件头。

use crate::core::math3d::Mat4;

use super::{GpuMaterial, Renderer, joint_palette_data, next_2d_vertex_capacity};

impl Renderer {
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
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
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
    ) -> wgpu::BindGroup {
        let material_data: [[f32; 4]; 2] = [
            material.base_color,
            [
                material.metallic,
                material.roughness,
                if material.skinned { 1.0 } else { 0.0 },
                0.0,
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
        let albedo = material
            .albedo
            .and_then(|index| self.albedo_textures.get(index as usize))
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
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-material3d-frame-bind-group"),
            layout: &self._material_bind_group_layout,
            entries: &[
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
            ],
        })
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
