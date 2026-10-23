//! compute 通道接缝（M74-03 P1）：管线缓存、绑定组缓存、dispatch、调试回读。
//!
//! ## 这不是什么
//!
//! 它不是"通用 GPU 计算 API"：没有跨帧任务图、没有异步 dispatch、没有 GPU→Lua 读回。
//! 它只服务渲染器内部的一个消费者（GPU 粒子），入口对宿主可见但**不属于脚本 API**。
//!
//! ## 三条硬约束
//!
//! 1. **无能力必须显式失败**。`capabilities.compute == false`（WebGL2/微信）或宿主调用
//!    `set_compute_enabled(false)` 时，所有入口返回 [`ComputeError::Unsupported`]，且带可读
//!    原因；绝不 panic、绝不静默空转。
//! 2. **绑定组按稳定身份缓存**。管线 key 含 shader hash + entry point + 绑定签名；绑定组 key
//!    再叠一个调用方给的稳定 id（缓冲重建时必须换 id）。"同 key 只建一次"由单测钉住。
//! 3. **回读是调试通道**。只在 native 上同步可用；wasm 上显式返回
//!    [`ComputeError::ReadbackUnsupported`]——浏览器没有阻塞式 map，假同步会把帧卡死。
//!
//! ## 提交点
//!
//! [`super::Renderer::dispatch_compute`] 自己 `submit`。宿主/消费者必须在 `render()` 之前调用，
//! 这样队列顺序天然保证 compute 先于本帧的图形 pass；不依赖隐式 barrier，也不把 dispatch
//! 塞进渲染 pass 的编码器里。

use std::collections::HashMap;

use crate::host::renderer::capabilities::RendererCapabilities;

/// compute 通道的错误。每个变体都带足以定位的读数（binding / 上限 / 原因），
/// 因为"dispatch 没生效"最难查的就是"到底哪一条不满足"。
#[derive(Debug, thiserror::Error)]
pub enum ComputeError {
    /// 设备没有 compute 能力，或宿主显式关掉了它。
    #[error("compute 不可用：{reason}")]
    Unsupported { reason: String },
    /// 本帧没有配置 GPU 消费者（例如没调用 `configure_gpu_particles`）。
    #[error("compute 消费者未配置")]
    NotConfigured,
    /// dispatch 的 workgroup 数越界（含某一维为 0）。
    #[error("workgroup 数 {workgroups:?} 非法或超过设备上限 {max}（每维）")]
    WorkgroupsOutOfRange { workgroups: [u32; 3], max: u32 },
    /// 存储缓冲绑定超过设备上限。
    #[error("存储缓冲绑定 {binding} 需要 {requested} 字节，超过设备上限 {limit}")]
    BindingTooLarge {
        binding: u32,
        requested: u64,
        limit: u64,
    },
    /// 缓冲缺少 dispatch 需要的 usage（例如没有 STORAGE）。
    #[error("缓冲绑定 {binding} 缺少 {usage:?} 用法")]
    MissingBufferUsage {
        binding: u32,
        usage: wgpu::BufferUsages,
    },
    /// 着色器编译/管线校验失败。错误文本来自 wgpu 的 validation error scope。
    #[error("compute 着色器/管线校验失败：{0}")]
    Shader(String),
    /// 参数非法（容量为 0、容量放不开发射器、非有限值等）。
    #[error("compute 参数非法：{0}")]
    InvalidConfig(String),
    /// wasm 上不做阻塞式回读。
    #[error("GPU 回读在当前平台不可用（wasm 没有阻塞式 map）")]
    ReadbackUnsupported,
    /// `map_async` 失败或回读前置条件不满足。
    #[error("GPU 回读失败：{0}")]
    Readback(String),
}

/// 一条 compute 绑定。
#[derive(Clone, Copy, Debug)]
pub enum ComputeBinding<'a> {
    /// 存储缓冲。`read_only` 进管线布局的 `BufferBindingType::Storage`。
    Storage {
        binding: u32,
        buffer: &'a wgpu::Buffer,
        read_only: bool,
    },
    /// uniform 缓冲。
    Uniform {
        binding: u32,
        buffer: &'a wgpu::Buffer,
    },
}

impl ComputeBinding<'_> {
    fn binding(self) -> u32 {
        match self {
            Self::Storage { binding, .. } | Self::Uniform { binding, .. } => binding,
        }
    }

    fn kind(self) -> BindingKind {
        match self {
            Self::Storage {
                read_only: true, ..
            } => BindingKind::StorageRead,
            Self::Storage {
                read_only: false, ..
            } => BindingKind::StorageWrite,
            Self::Uniform { .. } => BindingKind::Uniform,
        }
    }
}

/// 绑定签名里的类型（**不含缓冲指针**：同一份 shader + 同样类型才能复用管线）。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum BindingKind {
    StorageRead,
    StorageWrite,
    Uniform,
}

/// 一次 dispatch 的请求。
pub struct ComputeRequest<'a> {
    /// 调试标签（错误与统计用）。
    pub label: &'a str,
    /// WGSL 源。管线缓存按内容 hash 命中，因此同一份字面量重复传不会重复编译。
    pub shader: &'a str,
    /// 入口点名。
    pub entry_point: &'a str,
    /// 绑定（顺序无关，内部按 binding 排序）。
    pub bindings: &'a [ComputeBinding<'a>],
    /// workgroup 数（三维，每维必须 > 0）。
    pub workgroups: [u32; 3],
    /// 绑定组的稳定身份。`None` = 本调用不缓存绑定组。
    ///
    /// 为什么需要调用方给：wgpu 的 `Buffer` 没有可哈希的稳定 id，缓冲一旦重建，
    /// 用旧 key 命中的绑定组会绑到旧缓冲上——那是"画面不动但也没报错"的经典坑。
    /// 调用方在重建缓冲时换一个 id 即可（GPU 粒子用容量/代数拼 key）。
    pub bind_group_key: Option<u64>,
}

/// 一次 dispatch 的读数。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ComputeDispatch {
    /// 实际提交的 workgroup 数（三维）。
    pub workgroups: [u32; 3],
    /// 本次是否新建了管线（`false` = 命中缓存）。
    pub pipeline_compiled: bool,
    /// 本次是否新建了绑定组（`false` = 命中缓存或未请求缓存）。
    pub bind_group_created: bool,
}

/// compute 通道的累计读数（诊断/验收用）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ComputeStats {
    /// 成功提交的 dispatch 次数。
    pub dispatches: u64,
    /// 累计 workgroup 数。
    pub workgroups: u64,
    /// 累计新建管线数。
    pub pipeline_compiles: u64,
    /// 管线缓存命中次数。
    pub pipeline_cache_hits: u64,
    /// 累计新建绑定组数。
    pub bind_group_creates: u64,
    /// 绑定组缓存命中次数。
    pub bind_group_cache_hits: u64,
}

/// 管线缓存 key：shader 内容 hash + 入口点 + 绑定签名。
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct ComputePipelineKey {
    shader_hash: u64,
    entry_point: String,
    layout: Vec<(u32, BindingKind)>,
}

pub(super) struct CachedComputePipeline {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
}

/// 渲染器持有的 compute 缓存与计数（`Renderer` 的一个字段）。
#[derive(Default)]
pub(super) struct ComputeState {
    pub(super) pipelines: HashMap<ComputePipelineKey, CachedComputePipeline>,
    pub(super) bind_groups: HashMap<(ComputePipelineKey, u64), wgpu::BindGroup>,
    pub(super) stats: ComputeStats,
    /// 宿主策略位：`false` 时所有入口返回 `Unsupported`（回滚档）。
    pub(super) enabled: bool,
}

impl ComputeState {
    pub(super) fn new() -> Self {
        Self {
            enabled: true,
            ..Self::default()
        }
    }

    /// 有效 compute（硬件能力 + 宿主开关）。
    pub(super) const fn active(&self, capabilities: &RendererCapabilities) -> bool {
        capabilities.compute && self.enabled
    }

    /// 清空缓存（回滚/重配时用；不改变统计计数）。
    pub(super) fn clear(&mut self) {
        self.pipelines.clear();
        self.bind_groups.clear();
    }

    /// 剔除某个调用方 id 的绑定组缓存条目（消费者回收缓冲时用）。
    ///
    /// 只清绑定组、不动管线：管线 key 不含缓冲身份，回收一个消费者不该让别的消费者重编译。
    /// 没有这一步，"每帧爆发一次"的 GPU 粒子池会把绑定组缓存无限撑大。
    pub(super) fn retire_bind_group(&mut self, id: u64) {
        self.bind_groups.retain(|(_, key_id), _| *key_id != id);
    }
}

fn shader_hash(shader: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    shader.hash(&mut hasher);
    hasher.finish()
}

impl super::Renderer {
    /// 构造期写进 `Renderer` 的能力快照（M74-03 P0）。
    #[must_use]
    pub fn capabilities(&self) -> RendererCapabilities {
        self.capabilities
    }

    /// compute 累计读数。
    #[must_use]
    pub fn compute_stats(&self) -> ComputeStats {
        self.compute_state.stats
    }

    /// 绑定组缓存里还有多少条目（P3-R07-2 的回收观测点）。
    ///
    /// GPU 粒子池每个系统一份绑定组，回收时必须按调用方 id 精确剔除；这条读数是那个
    /// "剔除了没有"的唯一证据。它只增不减 ⇒ 池在漏缓存，长跑必然撑大显存侧的宿主对象。
    #[must_use]
    pub fn compute_bind_group_cache_len(&self) -> usize {
        self.compute_state.bind_groups.len()
    }

    /// 宿主策略位：关掉后所有 compute 入口返回 `Unsupported`，消费者走 CPU 路径。
    ///
    /// 这是**回滚档**，不改硬件能力读数（[`Self::capabilities`] 仍反映设备真实能力）。
    pub fn set_compute_enabled(&mut self, enabled: bool) {
        if self.compute_state.enabled == enabled {
            return;
        }
        self.compute_state.enabled = enabled;
        // 关掉时清缓存：下次打开重新编译，避免拿着旧绑定组（缓冲可能已重建）。
        if !enabled {
            self.compute_state.clear();
        }
        tracing::info!(
            target: "render",
            enabled,
            backend = self.capabilities.backend_name(),
            "[render] compute 通道开关"
        );
    }

    /// 当前是否真的走 compute（硬件能力 + 宿主开关）。
    #[must_use]
    pub fn compute_enabled(&self) -> bool {
        self.compute_state.active(&self.capabilities)
    }

    /// 提交一次 compute dispatch。
    ///
    /// **无能力/被关掉时返回 `Unsupported`**，不建管线、不提交、不 panic。调用方必须在
    /// `render()` 之前调用（队列顺序保证先 compute 后图形）。
    ///
    /// # Errors
    ///
    /// 见 [`ComputeError`]：能力、workgroup 越界、绑定超限、usage 不符、管线/着色器校验失败。
    pub fn dispatch_compute(
        &mut self,
        request: &ComputeRequest<'_>,
    ) -> Result<ComputeDispatch, ComputeError> {
        if !self.compute_state.active(&self.capabilities) {
            return Err(self.compute_not_available());
        }
        let capabilities = self.capabilities;
        if request.bindings.is_empty() {
            return Err(ComputeError::InvalidConfig(
                "compute dispatch 至少需要一条绑定".to_owned(),
            ));
        }
        for workgroups in request.workgroups {
            if workgroups == 0 {
                return Err(ComputeError::WorkgroupsOutOfRange {
                    workgroups: request.workgroups,
                    max: capabilities.max_compute_workgroups_per_dimension,
                });
            }
            if workgroups > capabilities.max_compute_workgroups_per_dimension {
                return Err(ComputeError::WorkgroupsOutOfRange {
                    workgroups: request.workgroups,
                    max: capabilities.max_compute_workgroups_per_dimension,
                });
            }
        }
        for binding in request.bindings {
            let buffer = match binding {
                ComputeBinding::Storage { buffer, .. } | ComputeBinding::Uniform { buffer, .. } => {
                    buffer
                }
            };
            let required_usage = match binding {
                ComputeBinding::Storage { .. } => wgpu::BufferUsages::STORAGE,
                ComputeBinding::Uniform { .. } => wgpu::BufferUsages::UNIFORM,
            };
            if !buffer.usage().contains(required_usage) {
                return Err(ComputeError::MissingBufferUsage {
                    binding: binding.binding(),
                    usage: required_usage,
                });
            }
            // 上限只对存储缓冲有意义；uniform 的 64 KiB 限制由 wgpu 自己校验。
            if matches!(binding, ComputeBinding::Storage { .. })
                && buffer.size() > capabilities.max_storage_buffer_binding_size
            {
                return Err(ComputeError::BindingTooLarge {
                    binding: binding.binding(),
                    requested: buffer.size(),
                    limit: capabilities.max_storage_buffer_binding_size,
                });
            }
        }

        let mut layout: Vec<(u32, BindingKind)> = request
            .bindings
            .iter()
            .map(|binding| (binding.binding(), binding.kind()))
            .collect();
        layout.sort_unstable_by_key(|(binding, _)| *binding);
        if layout.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(ComputeError::InvalidConfig(format!(
                "compute 绑定重复：{:?}",
                layout
            )));
        }
        let key = ComputePipelineKey {
            shader_hash: shader_hash(request.shader),
            entry_point: request.entry_point.to_owned(),
            layout,
        };

        let cached_pipeline = self
            .compute_state
            .pipelines
            .get(&key)
            .map(|entry| entry.pipeline.clone());
        let (pipeline_compiled, pipeline_ptr) = if let Some(pipeline) = cached_pipeline {
            self.compute_state.stats.pipeline_cache_hits += 1;
            (false, pipeline)
        } else {
            let (pipeline, bind_group_layout) = self.create_compute_pipeline(&key, request)?;
            let pipeline_ptr = pipeline.clone();
            self.compute_state.pipelines.insert(
                key.clone(),
                CachedComputePipeline {
                    pipeline,
                    bind_group_layout,
                },
            );
            self.compute_state.stats.pipeline_compiles += 1;
            (true, pipeline_ptr)
        };

        // 绑定组：只有调用方给了稳定 key 才缓存。
        let (bind_group_created, bind_group) = {
            let cache_key = request.bind_group_key.map(|id| (key.clone(), id));
            let cached = cache_key
                .as_ref()
                .and_then(|cache_key| self.compute_state.bind_groups.get(cache_key).cloned());
            if let Some(bind_group) = cached {
                self.compute_state.stats.bind_group_cache_hits += 1;
                (false, bind_group)
            } else {
                let layout = &self
                    .compute_state
                    .pipelines
                    .get(&key)
                    .expect("pipeline just inserted")
                    .bind_group_layout;
                let entries: Vec<wgpu::BindGroupEntry<'_>> = request
                    .bindings
                    .iter()
                    .map(|binding| wgpu::BindGroupEntry {
                        binding: binding.binding(),
                        resource: match binding {
                            ComputeBinding::Storage { buffer, .. }
                            | ComputeBinding::Uniform { buffer, .. } => buffer.as_entire_binding(),
                        },
                    })
                    .collect();
                let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(request.label),
                    layout,
                    entries: &entries,
                });
                if let Some(cache_key) = cache_key {
                    self.compute_state
                        .bind_groups
                        .insert(cache_key, bind_group.clone());
                }
                self.compute_state.stats.bind_group_creates += 1;
                (true, bind_group)
            }
        };

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("rustlua-compute-encoder"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(request.label),
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline_ptr);
            // 绑定组只有一个（group 0）；接缝 v1 不做多组，保持接口窄。
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(
                request.workgroups[0],
                request.workgroups[1],
                request.workgroups[2],
            );
        }
        self.queue.submit([encoder.finish()]);
        let total: u64 = request
            .workgroups
            .iter()
            .map(|value| u64::from(*value))
            .product();
        self.compute_state.stats.dispatches += 1;
        self.compute_state.stats.workgroups += total;
        Ok(ComputeDispatch {
            workgroups: request.workgroups,
            pipeline_compiled,
            bind_group_created,
        })
    }

    /// 把存储缓冲内容读回 CPU（**调试通道**）。
    ///
    /// 只在 native 上可用；wasm 返回 [`ComputeError::ReadbackUnsupported`]。源缓冲必须有
    /// `COPY_SRC` 用法且大小按 4 字节对齐。
    ///
    /// # Errors
    ///
    /// 能力/usage/平台/映射失败。
    pub fn read_storage_buffer(
        &self,
        buffer: &wgpu::Buffer,
        size: u64,
    ) -> Result<Vec<u8>, ComputeError> {
        if !self.compute_state.active(&self.capabilities) {
            return Err(self.compute_not_available());
        }
        if !buffer.usage().contains(wgpu::BufferUsages::COPY_SRC) {
            return Err(ComputeError::Readback(format!(
                "缓冲缺少 COPY_SRC 用法（usage={:?}）",
                buffer.usage()
            )));
        }
        if size == 0 || size > buffer.size() {
            return Err(ComputeError::Readback(format!(
                "回读大小 {size} 非法（缓冲 {} 字节）",
                buffer.size()
            )));
        }
        if !size.is_multiple_of(4) {
            return Err(ComputeError::Readback(format!(
                "回读大小 {size} 不是 4 的倍数"
            )));
        }
        self.read_storage_buffer_platform(buffer, size)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn read_storage_buffer_platform(
        &self,
        buffer: &wgpu::Buffer,
        size: u64,
    ) -> Result<Vec<u8>, ComputeError> {
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rustlua-compute-readback"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("rustlua-compute-readback-encoder"),
            });
        encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, size);
        self.queue.submit([encoder.finish()]);
        let slice = staging.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            // 接收端在本函数返回前一直活着；send 失败只可能是调用方已放弃，忽略即可。
            let _ = sender.send(result);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| ComputeError::Readback(format!("device.poll: {error}")))?;
        let result = receiver
            .recv()
            .map_err(|error| ComputeError::Readback(format!("map_async 回调丢失: {error}")))?;
        result.map_err(|error| ComputeError::Readback(format!("map_async: {error}")))?;
        let data = slice.get_mapped_range();
        let bytes = data.to_vec();
        drop(data);
        staging.unmap();
        Ok(bytes)
    }

    #[cfg(target_arch = "wasm32")]
    fn read_storage_buffer_platform(
        &self,
        _buffer: &wgpu::Buffer,
        _size: u64,
    ) -> Result<Vec<u8>, ComputeError> {
        Err(ComputeError::ReadbackUnsupported)
    }

    pub(super) fn compute_not_available(&self) -> ComputeError {
        if let Some(reason) = self.capabilities.compute_unavailable_reason() {
            return ComputeError::Unsupported { reason };
        }
        ComputeError::Unsupported {
            reason: "compute 已被 set_compute_enabled(false) 关闭".to_owned(),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn create_compute_pipeline(
        &self,
        key: &ComputePipelineKey,
        request: &ComputeRequest<'_>,
    ) -> Result<(wgpu::ComputePipeline, wgpu::BindGroupLayout), ComputeError> {
        let entries: Vec<wgpu::BindGroupLayoutEntry> = request
            .bindings
            .iter()
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding: binding.binding(),
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: match binding {
                    ComputeBinding::Storage { read_only, .. } => wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage {
                            read_only: *read_only,
                        },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    ComputeBinding::Uniform { .. } => wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                },
                count: None,
            })
            .collect();
        let bind_group_layout =
            self.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("rustlua-compute-bind-group-layout"),
                    entries: &entries,
                });
        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("rustlua-compute-pipeline-layout"),
                bind_group_layouts: &[&bind_group_layout],
                push_constant_ranges: &[],
            });
        let shader = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(request.label),
                source: wgpu::ShaderSource::Wgsl(request.shader.into()),
            });
        // 校验错误走 error scope：没有它，wgpu 的默认 handler 在 wasm 上是 abort、在 native 上
        // 是 panic——两种都不该由一条坏 shader 触发。这里把错误变成 `ComputeError::Shader`。
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let pipeline = self
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(request.label),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(request.entry_point),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });
        let error = pollster::block_on(self.device.pop_error_scope());
        if let Some(error) = error {
            return Err(ComputeError::Shader(format!(
                "{}（entry={}, shader_hash={:x}）",
                error, key.entry_point, key.shader_hash
            )));
        }
        Ok((pipeline, bind_group_layout))
    }

    #[cfg(target_arch = "wasm32")]
    fn create_compute_pipeline(
        &self,
        _key: &ComputePipelineKey,
        _request: &ComputeRequest<'_>,
    ) -> Result<(wgpu::ComputePipeline, wgpu::BindGroupLayout), ComputeError> {
        // wasm 只有 WebGPU 后端可能走到这里（WebGL2 在 `capabilities.compute` 就被挡住），
        // 而本仓的 web 产物固定是 WebGL2，因此这条路径不参与产品发布。真要支持时，
        // 这里需要把管线创建错误改成异步 error scope——不能拿 `pollster::block_on` 硬等。
        Err(ComputeError::Unsupported {
            reason: "wasm 构建不提供 compute 管线创建（web 产物走 WebGL2）".to_owned(),
        })
    }
}
