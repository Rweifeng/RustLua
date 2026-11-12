//! `Renderer` 的逐帧 uniform/storage arena 与材质绑定组（WP12 阶段 4b 从 `renderer/mod.rs`
//! 原样搬出）：`push_frame_uniform` / `push_frame_storage` / `material_bind_group`。
//!
//! 同一类型、同 crate 的另一个固有 `impl` 块；可见性口径见 `gpu_mesh.rs` 的文件头。

use std::sync::Arc;
use std::time::Instant;

use rustlua_math::math3d::Mat4;

use super::batch_motion_gpu::BATCH_MOTION_MATRIX_BYTES;
use super::{
    FrameDraw, GpuMaterial, MATERIAL_UNIFORM_ROWS, Renderer, joint_palette_data,
    next_2d_vertex_capacity,
};

/// 无存储缓冲时材质布局 binding 2 的 64 字节：一条 draw 只有一个模型矩阵（列主序）。
///
/// 展开口径与 storage 路径同为「每模型 4 个 vec4 列」。曾被按行取 `..16`，而列只有 4 个，
/// 于是每条 draw 都落回单位矩阵——所有网格叠在原点，画面里只剩最后画的那个。
pub(super) fn single_instance_uniform(models: &[Mat4]) -> [u8; 64] {
    let columns = models
        .first()
        .map_or(super::IDENTITY_MAT4, |model| model.columns);
    let mut bytes = [0u8; 64];
    bytes.copy_from_slice(bytemuck::bytes_of(&columns));
    bytes
}

/// P3-R02：PBR 逐实例法线矩阵在帧级环形缓冲里的位置。绘制时作为顶点缓冲槽 1 绑定，
/// 与模型矩阵的存储绑定共用同一块缓冲、不同偏移。
pub(super) struct NormalMatrixSlice {
    pub buffer: wgpu::Buffer,
    pub offset: u64,
    pub size: u64,
}

/// 3D 材质绑定组的缓存键。
///
/// 绑定组**只编码**"哪块缓冲的哪一段 + 哪几个贴图视图"，不编码里面的字节；而帧级环形缓冲
/// 每帧从头按同一顺序分配，稳态下（draw 序列与实例数不变）偏移与尺寸逐帧相同，材质/调色板
/// 字节的变化靠 `write_buffer` 原地更新即可。所以只要这个键相等，复用绑定组在语义上完全等价，
/// 差别只是省掉一次 `create_bind_group`（每条 draw 一次，是本帧最大的一笔绑定组开销）。
///
/// 两个 `*_epoch` 由 arena 扩容时自增：缓冲句柄换了，旧绑定组指向旧缓冲，必须重建。
/// 贴图用**槽位号**而不是视图指针：`albedo_textures` 只追加、不替换，槽位号的视图一生不变。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct MaterialBindKey {
    material_epoch: u64,
    material_offset: u64,
    material_size: u64,
    /// 0 = 静态网格用共享的 `identity_palette`；1 = 骨骼动画，走 material arena 里的那一段。
    palette_kind: u8,
    palette_offset: u64,
    palette_size: u64,
    instance_epoch: u64,
    instance_offset: u64,
    instance_size: u64,
    albedo: Option<u32>,
    metallic_roughness: Option<u32>,
    emissive: Option<u32>,
    normal: Option<u32>,
    occlusion: Option<u32>,
}

/// 缓存条目：键相等即复用 `bind`。
pub(super) struct CachedMaterialBind {
    pub key: MaterialBindKey,
    pub bind: wgpu::BindGroup,
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
            // 缓冲句柄换了：所有引用旧缓冲的材质绑定组必须重建（缓存键里的代次对不上）。
            self.material_arena_epoch += 1;
            offset = 0;
        }
        self.queue
            .write_buffer(&self.material_arena, offset, contents);
        self.material_arena_used = offset + size;
        (self.material_arena.clone(), offset, size)
    }

    /// P3-R245：只**预留**一块实例环形成员（64 B 对齐），返回（缓冲, 偏移, 字节数），
    /// 不写任何数据。给"compute 自己往这块区域写"的路径用（矩阵与法线各预留一段）。
    fn reserve_frame_storage(&mut self, size: u64) -> (wgpu::Buffer, u64, u64) {
        let mask = self.storage_align - 1;
        let mut offset = self.instance_arena_used.wrapping_add(mask) & !mask;
        if offset + size > self.instance_arena.size() {
            let grown = (self.instance_arena.size() * 2).max(offset + size);
            self.instance_arena = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rustlua-instance-models-frame-arena"),
                size: grown,
                usage: self.storage_usage()
                    | wgpu::BufferUsages::VERTEX
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_arena_epoch += 1;
            offset = 0;
        }
        self.instance_arena_used = offset + size;
        (self.instance_arena.clone(), offset, size)
    }

    /// P0-①：实例模型矩阵帧级环形缓冲的写入端，语义与 [`Self::push_frame_uniform`] 相同。
    fn push_frame_storage(&mut self, contents: &[u8]) -> (wgpu::Buffer, u64, u64) {
        let (buffer, offset, size) = self.reserve_frame_storage(contents.len() as u64);
        self.queue.write_buffer(&buffer, offset, contents);
        (buffer, offset, size)
    }

    /// Creates a material bind group carrying one primitive's file material,
    /// the joint palette (identity for static meshes, animated for skinned
    /// drawables), and the per-instance model matrices. P0-①：数据落在帧级环形
    /// 缓冲里按偏移引用，每次调用不再新建任何 GPU buffer。
    pub(super) fn material_bind_group(
        &mut self,
        index: usize,
        material: GpuMaterial,
        palette: &[Mat4],
        models: &[Mat4],
        normals: bool,
    ) -> (wgpu::BindGroup, Option<NormalMatrixSlice>) {
        // 没有存储缓冲的设备（WebGL2）：binding 2 是 64 字节 uniform，每条 draw 只装一个
        // 模型矩阵。实例化合批在构造期已按能力关掉，`frame_3d_draws` 因此不会产出多条
        // 实例合批（每条 draw 的 `models.len()` 恒为 1）——这里取首个模型就是全部。
        let instance = if self.storage_capable {
            let mut instance_data = Vec::with_capacity(models.len() * 4);
            for model in models {
                instance_data.extend_from_slice(&model.columns);
            }
            self.push_frame_storage(bytemuck::cast_slice(&instance_data))
        } else {
            debug_assert_eq!(models.len(), 1, "无存储缓冲时每条 3D draw 只该有一个模型");
            self.push_frame_uniform(&single_instance_uniform(models))
        };
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
        self.material_bind_group_with_slices(index, material, palette, instance, normal_slice)
    }

    /// P3-R245：绑定组创建的两条来源之一——实例数据**已经**在 arena 里（CPU 上传，或
    /// compute 刚写好的 GPU 位姿），只差材质/调色板与绑定组本身。
    fn material_bind_group_with_slices(
        &mut self,
        index: usize,
        material: GpuMaterial,
        palette: &[Mat4],
        instance: (wgpu::Buffer, u64, u64),
        normal_slice: Option<NormalMatrixSlice>,
    ) -> (wgpu::BindGroup, Option<NormalMatrixSlice>) {
        // 第 6 个 vec4 是程序化表面开关：-1 内建水面，-2 内建地形。
        // 负 normal_scale 不会进入法线采样（两种内建表面都没有法线贴图）。
        let animated = if material.normal_scale < -1.5 {
            2.0
        } else if material.normal_scale < 0.0 {
            1.0
        } else {
            0.0
        };
        // P3-R197：水面廉价光照档——材质声明或宿主全局覆盖（A/B）任一为真即开启。
        let water_cheap = material.water_cheap || self.water_cheap_override;
        // P3-R203：地形廉价细节档，同上。
        let terrain_cheap = material.terrain_cheap || self.terrain_cheap_override;
        let material_data: [[f32; 4]; MATERIAL_UNIFORM_ROWS] = [
            material.base_color,
            [
                material.metallic,
                material.roughness,
                if material.skinned { 1.0 } else { 0.0 },
                // Built-in terrain detail atlas tile size in world units; 0 = off.
                material.detail_scale,
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
            [
                animated,
                if animated > 1.5 {
                    material.occlusion_strength
                } else {
                    0.0
                },
                // P3-R197：z = 1 表示水面廉价光照档（仅 animated=1 的水面读取）。
                // P3-R277：普通网格（animated=0）复用同一槽表示廉价着色档。
                if animated < 0.5 {
                    if self.crowd_cheap_override { 1.0 } else { 0.0 }
                } else if water_cheap && animated > 0.5 {
                    1.0
                } else {
                    0.0
                },
                // P3-R203：w = 1 表示地形廉价细节档（仅 animated=2 的地形读取）。
                if terrain_cheap && animated > 1.5 {
                    1.0
                } else {
                    0.0
                },
            ],
            // P3-R162：清漆层（factor, roughness）。旧着色器不读第 7..10 个 vec4，
            // 全 0 时新瓣在 shader 里精确退化为旧路径（golden 逐字节一致）。
            [material.clearcoat, material.clearcoat_roughness, 0.0, 0.0],
            // P3-R163：SSS 近似（strength, wrap/thickness）。
            [material.sss_strength, material.sss_thickness, 0.0, 0.0],
            // P3-R164：薄膜虹彩（factor, ior, thickness_min, thickness_max）。
            [
                material.iridescence,
                material.iridescence_ior,
                material.iridescence_thickness_min,
                material.iridescence_thickness_max,
            ],
            // P3-R165：绒毛瓣（sheen rgb, roughness）。颜色全 0 时该项精确为 0
            // 且基层衰减为 1 ⇒ 与旧路径逐字节相同。
            [
                material.sheen_color[0],
                material.sheen_color[1],
                material.sheen_color[2],
                material.sheen_roughness,
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
        // 没有存储缓冲的设备（WebGL2）：binding 2 是 64 字节 uniform，每条 draw 只装一个
        let (instance_buffer, instance_offset, instance_size) = instance;
        // P4-①：稳态复用。环形缓冲每帧按同一顺序分配 ⇒ 键相同就意味着"绑定组语义完全等价"，
        // 直接返回上一帧那个（wgpu 资源是 Arc，clone 只加一次引用计数）。
        let key = MaterialBindKey {
            material_epoch: self.material_arena_epoch,
            material_offset,
            material_size,
            palette_kind: if material.skinned && !palette.is_empty() {
                1
            } else {
                0
            },
            palette_offset,
            palette_size,
            instance_epoch: self.instance_arena_epoch,
            instance_offset,
            instance_size,
            albedo: material.albedo,
            metallic_roughness: material.metallic_roughness,
            emissive: material.emissive_texture,
            normal: material.normal_texture,
            occlusion: material.occlusion_texture,
        };
        let reused = self
            .material_bind_cache
            .get(index)
            .filter(|cached| cached.key == key)
            .map(|cached| cached.bind.clone());
        if let Some(bind) = reused {
            self.material_bind_reused += 1;
            return (bind, normal_slice);
        }
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
            debug_assert_eq!(normal.size, instance_size);
        }
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rustlua-material3d-frame-bind-group"),
            layout: &self._material_bind_group_layout,
            entries: &entries,
        });
        self.material_bind_created += 1;
        // 按序位缓存：同一帧的 draw 序列稳定 ⇒ 下一帧同序位大概率命中。draw 数变少时多出来的
        // 旧条目留在尾部不参与匹配（它们的偏移下一帧未必还在用），不主动清，免得反复重建。
        let entry = CachedMaterialBind {
            key,
            bind: bind_group.clone(),
        };
        if index < self.material_bind_cache.len() {
            self.material_bind_cache[index] = entry;
        } else {
            debug_assert_eq!(
                index,
                self.material_bind_cache.len(),
                "material_bind_group 必须按 draw 序位调用"
            );
            self.material_bind_cache.push(entry);
        }
        (bind_group, normal_slice)
    }

    /// P3-R245：按 `FrameDraw` 建绑定组——带 `gpu_motion` 的 draw 先让 compute 把位姿写进
    /// 实例 arena（PBR 时连同法线矩阵），再按同一份切片建绑定组；烘焙失败/无 compute 能力
    /// 时回退到 CPU `models`（画面与开档前一致，不静默丢 draw）。
    /// P3-R268：给「逐 draw 建材质绑定组」整段循环计时。这段在任何 pass 录制之前跑，
    /// `render_record_us` 减掉两条阴影子桶后剩下的固定面主要就在这里；不拆出来的话
    /// 「record 贵」只能归到 pass 上，而真正贵的可能是烘焙与上传（R268 实测关掉整条
    /// planar + 后处理 + 点光阴影只减 record 0.07–0.32 ms，pass 侧不是大头）。
    ///
    /// `index == 0` 时复位：循环恒从 0 起，于是不必在 `render_passes` 里另加复位点。
    /// 读数由宿主 `take_*` 取走（取完归零），没有 3D draw 的帧报 0 而不是上一帧残值。
    pub(super) fn material_bind_group_for_draw(
        &mut self,
        index: usize,
        draw: &FrameDraw,
        normals: bool,
    ) -> (
        wgpu::BindGroup,
        Option<NormalMatrixSlice>,
        Option<wgpu::Buffer>,
    ) {
        if index == 0 {
            self.frame_bind_build_us = 0;
            self.frame_bind_bake_us = 0;
        }
        let started = Instant::now();
        let built = self.material_bind_group_for_draw_inner(index, draw, normals);
        self.frame_bind_build_us += started.elapsed().as_micros() as u64;
        built
    }

    fn material_bind_group_for_draw_inner(
        &mut self,
        index: usize,
        draw: &FrameDraw,
        normals: bool,
    ) -> (
        wgpu::BindGroup,
        Option<NormalMatrixSlice>,
        Option<wgpu::Buffer>,
    ) {
        if let Some(motion) = draw.gpu_motion.as_ref() {
            // P3-R246：画调用按 `draw.instance_count()`（= 批次数）取实例，烘焙也按同一口径写；
            // `draw.models` 只可能是空的（宿主跳过 CPU 求值），非空时仍按它兜底 CPU 路径。
            let count = motion.batch.instances.len() as u64;
            let size = count * BATCH_MOTION_MATRIX_BYTES;
            // 先预留再烘焙：预留可能让 arena 扩容换缓冲，烘焙必须写进换代后的那一块。
            let instance = self.reserve_frame_storage(size);
            let normal_slice = if normals {
                let (buffer, offset, size) = self.reserve_frame_storage(size);
                Some(NormalMatrixSlice {
                    buffer,
                    offset,
                    size,
                })
            } else {
                None
            };
            // P3-R250：距离剔除档（默认 0 = 关）——球心是当帧相机位置，超出半径的实例
            // 由 compute 写零矩阵；烘焙失败回退 CPU 时不补剔除（回退路径本来就是诊断路径）。
            let cull = (self.gpu_motion_cull_distance > 0.0)
                .then_some((self.camera_world_position, self.gpu_motion_cull_distance));
            // P3-R255：逐实例视锥档（当帧由 `render_passes` 打包）。它单独开也要走压缩
            // 通道——零矩阵档只让三角形退化，实例底价照付。
            let frustum = self.bake_frustum;
            // P3-R266：远程密度档与距离剔除共用相机原点；单开 thin 也要走压缩烘焙
            // （留下/走掉的实例数必须由 indirect 参数带出去，退化零矩阵省不了顶点底价）。
            let thin = self
                .gpu_motion_thin
                .map(|(meters, stride)| (self.camera_world_position, meters, stride));
            // P3-R267：第二远带只在第一带已开且确实更远时生效（半径写反就丢弃）。
            let thin_far = match (self.gpu_motion_thin, self.gpu_motion_thin_far) {
                (Some((near_meters, _)), Some((far_meters, stride)))
                    if far_meters > near_meters =>
                {
                    Some((self.camera_world_position, far_meters, stride))
                }
                _ => None,
            };
            let normal_slices = normal_slice
                .as_ref()
                .map(|slice| (&slice.buffer, slice.offset));
            // P3-R273：网格 LOD 链——2~3 条 draw 各烘一次，球心与剔除档共用当帧相机
            // 位置。单开 LOD 也必须走压缩通道：每侧只画自己那一侧的实例，存活数得由
            // indirect 参数带出去。P3-R274：`(第一台阶, 第二台阶或 0, 侧)`。
            let lod = draw.gpu_motion_lod.map(|(distance, far_distance, side)| {
                (self.camera_world_position, distance, far_distance, side)
            });
            // P3-R251：剔除档开时走**压缩**烘焙——存活实例写前缀、indirect 参数带真实
            // instance_count；零矩阵档的顶点底价不再随批次全量支付。两档都关走原路径。
            let bake_started = Instant::now();
            let baked = match (cull, frustum, thin, thin_far, lod) {
                (None, None, None, None, None) => self
                    .bake_batch_motion_with_normals_and_cull(
                        &motion.batch,
                        motion.time,
                        &instance.0,
                        instance.1,
                        normal_slices,
                        motion.model_scale,
                        None,
                    )
                    .map(|written| (u64::from(written), None)),
                (cull, frustum, thin, thin_far, lod) => {
                    if self.bake_cache_enabled {
                        self.bake_batch_motion_compact_cached_with_lod(
                            &motion.batch,
                            motion.time,
                            &instance.0,
                            instance.1,
                            normal_slices,
                            motion.model_scale,
                            cull,
                            draw.geometry.index_count,
                            frustum,
                            draw.gpu_motion_local_radius,
                            thin,
                            thin_far,
                            lod,
                            // P3-R284：目标与法线槽都出自上面那次 `reserve_frame_storage`
                            // ⇒ 就是本帧 arena，绑定组可以按输入集身份 + arena 代数缓存。
                            true,
                        )
                        .map(|args| (count, args))
                    } else {
                        self.bake_batch_motion_compact_with_lod(
                            &motion.batch,
                            motion.time,
                            &instance.0,
                            instance.1,
                            normal_slices,
                            motion.model_scale,
                            cull,
                            draw.geometry.index_count,
                            frustum,
                            draw.gpu_motion_local_radius,
                            thin,
                            thin_far,
                            lod,
                        )
                        .map(|args| (count, args))
                    }
                }
            };
            self.frame_bind_bake_us += bake_started.elapsed().as_micros() as u64;
            match baked {
                Ok((written, args)) if written == count => {
                    let (bind, normal_slice) = self.material_bind_group_with_slices(
                        index,
                        draw.material,
                        &draw.palette,
                        instance,
                        normal_slice,
                    );
                    return (bind, normal_slice, args);
                }
                Ok((written, _)) => {
                    tracing::warn!(
                        target: "render",
                        written,
                        count,
                        "[render] GPU 位姿烘焙只写了部分实例，回退 CPU 矩阵"
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        target: "render",
                        %error,
                        "[render] GPU 位姿烘焙失败，回退 CPU 矩阵"
                    );
                }
            }
            // 回退也要按 `count` 给出矩阵：`draw.models` 为空时现场求值一次（O(N)，只在
            // 设备没有 compute 能力或输入非法时走到）。非法实例给零矩阵（退化、不画），
            // 保证实例数与 `instance_count()` 一致——多一个少一个都会读进邻居的矩阵。
            let models: Vec<Mat4> = if draw.models.is_empty() {
                // 与 CPU 档同口径：`matrices_at` 自己处理"没有运动/路径非法 ⇒ 静态表"。
                // 非法实例在 `matrices_at` 里被跳过，补零矩阵把数量对齐回 `count`（零矩阵退化，
                // 不画；不能少给，否则多出来的那一段会读进邻居的矩阵）。
                let mut models: Vec<Mat4> = motion
                    .batch
                    .matrices_at(motion.time)
                    .into_iter()
                    .map(|model| super::scaled_model(model, motion.model_scale))
                    .collect();
                models.resize_with(count as usize, || Mat4 {
                    columns: [[0.0; 4]; 4],
                });
                models
            } else {
                draw.models.clone()
            };
            let (bind, normal_slice) =
                self.material_bind_group(index, draw.material, &draw.palette, &models, normals);
            return (bind, normal_slice, None);
        }
        let (bind, normal_slice) =
            self.material_bind_group(index, draw.material, &draw.palette, &draw.models, normals);
        (bind, normal_slice, None)
    }

    /// 打一行**每台渲染器只打一次**的缓存读数：第 5 个有 3D draw 的帧，或首次全命中时。
    ///
    /// 为什么要固定打一行而不是只在全命中时打：绑定组复用省的是 CPU 侧创建开销，画面上看不出来。
    /// 只打"全命中"的话，真机上一帧都没命中的场景在日志里**和缓存不存在时长得一模一样**——
    /// 那等于没有观测。这一行把 `draws/created/reused` 三个数摆出来，命中率是几一眼可见。
    pub(super) fn note_material_bind_frame(&mut self, draws: u64) {
        // P3-R270：绑定组循环到此结束——把攒下的烘焙 compute 封好（R289 起随图形一次提交，
        // 队列顺序不变），再清掉已经没有活引用的批次缓存。封好必须在下面那个「只打一次
        // 日志」的提前返回之前，否则第 5 帧之后就再也不封了。
        self.seal_pending_compute();
        // P3-R273：LOD 链让**同一条批次**占两份输入集（近/远各一份参数与参数缓冲），
        // 单条 `strong_count > 1` 会把这一对互相撑着、永远清不掉。按"同一指针还有几个
        // 缓存持有者"扣掉自持计数：活引用只剩缓存本身时才回收。
        let mut holders: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
        for inputs in self.bake_inputs.values() {
            *holders
                .entry(Arc::as_ptr(&inputs.batch) as usize)
                .or_default() += 1;
        }
        let mut retired_binds = Vec::new();
        self.bake_inputs.retain(|_, inputs| {
            let ptr = Arc::as_ptr(&inputs.batch) as usize;
            let alive = Arc::strong_count(&inputs.batch) > holders.get(&ptr).copied().unwrap_or(1);
            // P3-R284：输入集回收时把它那份 compute 绑定组也清掉，否则缓存随批次数无限长。
            if !alive && inputs.bind_serial != 0 {
                retired_binds.push(inputs.bind_serial);
            }
            alive
        });
        for serial in retired_binds {
            self.compute_state.retire_bind_group(serial);
        }
        self.material_bind_frames += 1;
        let warm = self.material_bind_created == 0;
        if self.material_bind_reported || (self.material_bind_frames < 5 && !warm) {
            return;
        }
        self.material_bind_reported = true;
        tracing::info!(
            target: "render",
            draws,
            created = self.material_bind_created,
            reused = self.material_bind_reused,
            warm,
            "[render] material bind cache stats"
        );
    }

    /// P3-R268：取走本帧「逐 draw 建绑定组」循环的 CPU µs（取完归零）。
    pub fn take_frame_bind_build_us(&mut self) -> u64 {
        std::mem::take(&mut self.frame_bind_build_us)
    }

    /// P3-R268：取走本帧该循环里 GPU 位姿烘焙（compute 派发）那一段的 CPU µs（取完归零）。
    /// 恒 `<= take_frame_bind_build_us()`；两者之差 = 绑定组创建 + arena 上传 + CPU 兜底。
    pub fn take_frame_bind_bake_us(&mut self) -> u64 {
        std::mem::take(&mut self.frame_bind_bake_us)
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

#[cfg(test)]
mod tests {
    use super::single_instance_uniform;
    use rustlua_math::math3d::Mat4;

    /// WebGL2 的 3D 路径把模型矩阵塞进 64 字节 uniform：这份字节必须真是**这条 draw 的模型**。
    /// 曾经按行取 `..16`（列只有 4 个）⇒ 恒落回单位矩阵，所有网格叠在原点。
    #[test]
    fn single_instance_uniform_carries_the_model_columns() {
        let model = Mat4 {
            columns: [
                [2.0, 0.0, 0.0, 4.5],
                [0.0, 1.0, 0.0, 0.02],
                [0.0, 0.0, 3.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        };
        let bytes = single_instance_uniform(&[model]);
        assert_eq!(bytes.len(), 64);
        let decoded: [[f32; 4]; 4] = bytemuck::cast(bytes);
        assert_eq!(decoded, model.columns);
        assert_ne!(decoded, super::super::IDENTITY_MAT4);
    }

    /// 没有模型（不该发生，但底线要明确）：落单位矩阵，而不是零矩阵（零矩阵会把网格缩成不可见）。
    #[test]
    fn single_instance_uniform_falls_back_to_identity() {
        let decoded: [[f32; 4]; 4] = bytemuck::cast(single_instance_uniform(&[]));
        assert_eq!(decoded, super::super::IDENTITY_MAT4);
    }
}
