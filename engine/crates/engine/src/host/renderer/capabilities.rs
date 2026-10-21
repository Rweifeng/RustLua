//! 渲染器**设备能力**的单一采集点（M74-03 P0）。
//!
//! 为什么要有这个模块：渲染器里已经散着两套"这台机器能不能用某条路"的判断——
//! `storage_capable`（`device.limits().max_storage_buffers_per_shader_stage > 0`）和
//! `timestamp_enabled`（`device.features()`）。compute 如果各自再写一套，就会出现
//! "构造期说能、调度期说不能"或反过来的分叉。这里把**本批次用到的能力**收成一份只读快照：
//! 构造期采一次，之后不变，`Renderer::capabilities()` 与所有消费点都读同一份。
//!
//! 口径（与 `docs/62` §6 / `tasks/done/M74-03` 的 P0 一致）：
//!
//! - `compute` 必须同时满足**后端声明**与**限制值非零**。WebGL2 的
//!   `DownlevelFlags::COMPUTE_SHADERS` 不置位，且 `Limits::downlevel_webgl2_defaults()` 里
//!   `max_compute_workgroup_size_*` 全为 0——两条独立证据都指向 false，不能只看其一。
//! - 离屏渲染器（编辑器/eframe）拿到的是**外部 device**，没有 adapter 可查 downlevel
//!   flags：这时只按 `device.limits()` 判定，并在 `backend` 里保留 `None`（"外部设备"）。
//! - 这个结构体只描述**设备能做什么**，不描述**用户开没开**。后者是
//!   [`super::Renderer::set_compute_enabled`] 的策略位，两者不能混成一个字段。

#[cfg(test)]
use wgpu::Limits;
use wgpu::{Backend, DownlevelFlags};

/// 构造期采集的设备能力快照（只读，之后不变）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RendererCapabilities {
    /// 后端。`None` = 设备由调用方提供（离屏渲染器），没有可查询的 adapter。
    pub backend: Option<Backend>,
    /// 是否具备 compute 通道（后端 + 限制值双条件）。
    pub compute: bool,
    /// 是否有存储缓冲（WebGL2 上为 false；矢量文字/3D 实例环/GPU 粒子都依赖它）。
    pub storage_buffers: bool,
    /// 是否启用了 GPU 时间戳查询。
    pub timestamp: bool,
    /// 计算着色器各维度的 workgroup 上限（不可用时全 0）。
    pub max_compute_workgroup_size: [u32; 3],
    /// 单个 workgroup 的调用数上限。
    pub max_compute_invocations_per_workgroup: u32,
    /// 单次 dispatch 的 workgroup 数上限（每维）。
    pub max_compute_workgroups_per_dimension: u32,
    /// 单个存储缓冲绑定可覆盖的字节数上限。
    pub max_storage_buffer_binding_size: u64,
}

impl RendererCapabilities {
    /// 采集一份能力快照。
    ///
    /// `adapter` 为 `None` 时（外部 device）跳过 downlevel flag 判定——那种 device 不是本
    /// 渲染器创建的，拿不到 adapter，只能相信 `device.limits()`。这也是编辑器离屏路径的现状。
    #[must_use]
    pub(crate) fn probe(device: &wgpu::Device, adapter: Option<&wgpu::Adapter>) -> Self {
        let limits = device.limits();
        let backend = adapter.map(|adapter| adapter.get_info().backend);
        // WebGL2 的两条证据：downlevel flag 不置位、compute 限制全 0。两条都要，缺一不可。
        let downlevel_compute = adapter.is_none_or(|adapter| {
            adapter
                .get_downlevel_capabilities()
                .flags
                .contains(DownlevelFlags::COMPUTE_SHADERS)
        });
        let limits_compute = limits.max_compute_workgroup_size_x > 0
            && limits.max_compute_workgroup_size_y > 0
            && limits.max_compute_workgroup_size_z > 0
            && limits.max_compute_invocations_per_workgroup > 0
            && limits.max_compute_workgroups_per_dimension > 0;
        let storage_buffers = limits.max_storage_buffers_per_shader_stage > 0;
        Self {
            backend,
            compute: downlevel_compute && limits_compute && storage_buffers,
            storage_buffers,
            timestamp: device.features().contains(
                wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS,
            ),
            max_compute_workgroup_size: [
                limits.max_compute_workgroup_size_x,
                limits.max_compute_workgroup_size_y,
                limits.max_compute_workgroup_size_z,
            ],
            max_compute_invocations_per_workgroup: limits.max_compute_invocations_per_workgroup,
            max_compute_workgroups_per_dimension: limits.max_compute_workgroups_per_dimension,
            max_storage_buffer_binding_size: u64::from(limits.max_storage_buffer_binding_size),
        }
    }

    /// 纯函数口径的构造入口（单测用来钉死"WebGL2 必须读到 false"这条硬门）。
    #[cfg(test)]
    #[must_use]
    pub(crate) fn from_limits(
        limits: &Limits,
        downlevel_flags: Option<DownlevelFlags>,
        backend: Option<Backend>,
        timestamp: bool,
    ) -> Self {
        let downlevel_compute =
            downlevel_flags.is_none_or(|flags| flags.contains(DownlevelFlags::COMPUTE_SHADERS));
        let limits_compute = limits.max_compute_workgroup_size_x > 0
            && limits.max_compute_workgroup_size_y > 0
            && limits.max_compute_workgroup_size_z > 0
            && limits.max_compute_invocations_per_workgroup > 0
            && limits.max_compute_workgroups_per_dimension > 0;
        let storage_buffers = limits.max_storage_buffers_per_shader_stage > 0;
        Self {
            backend,
            compute: downlevel_compute && limits_compute && storage_buffers,
            storage_buffers,
            timestamp,
            max_compute_workgroup_size: [
                limits.max_compute_workgroup_size_x,
                limits.max_compute_workgroup_size_y,
                limits.max_compute_workgroup_size_z,
            ],
            max_compute_invocations_per_workgroup: limits.max_compute_invocations_per_workgroup,
            max_compute_workgroups_per_dimension: limits.max_compute_workgroups_per_dimension,
            max_storage_buffer_binding_size: u64::from(limits.max_storage_buffer_binding_size),
        }
    }

    /// 后端短名（日志/诊断用；外部 device 为 `"external"`）。
    #[must_use]
    pub const fn backend_name(&self) -> &'static str {
        match self.backend {
            Some(Backend::Noop) => "noop",
            Some(Backend::Vulkan) => "vulkan",
            Some(Backend::Metal) => "metal",
            Some(Backend::Dx12) => "dx12",
            Some(Backend::Gl) => "gl",
            Some(Backend::BrowserWebGpu) => "webgpu",
            None => "external",
        }
    }

    /// 没有 compute 时的**可读原因**（有 compute 时为 `None`）。
    ///
    /// 为什么要返回字符串而不是一个 bool：降级路径必须能说清"为什么慢/为什么没画"，
    /// 否则日志里只剩"没效果"。
    #[must_use]
    pub fn compute_unavailable_reason(&self) -> Option<String> {
        if self.compute {
            return None;
        }
        let mut reasons = Vec::new();
        if !self.storage_buffers {
            reasons.push("max_storage_buffers_per_shader_stage=0".to_owned());
        }
        if self.max_compute_workgroup_size == [0, 0, 0] {
            reasons.push("max_compute_workgroup_size_*=0".to_owned());
        }
        if self.max_compute_invocations_per_workgroup == 0 {
            reasons.push("max_compute_invocations_per_workgroup=0".to_owned());
        }
        if self.max_compute_workgroups_per_dimension == 0 {
            reasons.push("max_compute_workgroups_per_dimension=0".to_owned());
        }
        if reasons.is_empty() {
            reasons.push("backend does not expose COMPUTE_SHADERS".to_owned());
        }
        Some(format!(
            "backend={} 不支持 compute（{}）",
            self.backend_name(),
            reasons.join("; ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **硬门**（M74-03 P0）：WebGL2 的默认限制必须推出 `compute=false`。
    ///
    /// 这条若失败，说明对 WebGL2 的判断错了，整条 compute 排期都要重写——所以它先于任何
    /// GPU 用例，且不依赖真实适配器。
    #[test]
    fn webgl2_limits_report_no_compute() {
        let capabilities = RendererCapabilities::from_limits(
            &Limits::downlevel_webgl2_defaults(),
            Some(DownlevelFlags::empty()),
            Some(Backend::Gl),
            false,
        );
        assert!(
            !capabilities.compute,
            "WebGL2 必须读到 compute=false：{capabilities:?}"
        );
        assert!(!capabilities.storage_buffers);
        let reason = capabilities
            .compute_unavailable_reason()
            .expect("无能力必须给出原因");
        assert!(reason.contains("gl"), "{reason}");
        assert!(reason.contains("storage"), "{reason}");
    }

    /// 只有限制值、没有 downlevel flags 时（外部 device）也必须按限制值判 false。
    #[test]
    fn external_device_with_zero_compute_limits_reports_no_compute() {
        let capabilities = RendererCapabilities::from_limits(
            &Limits::downlevel_webgl2_defaults(),
            None,
            None,
            false,
        );
        assert!(!capabilities.compute);
        assert_eq!(capabilities.backend_name(), "external");
    }

    /// 桌面默认限制必须推出 `compute=true`（否则 P1 的接缝没有可验证的档位）。
    #[test]
    fn desktop_default_limits_report_compute() {
        let capabilities = RendererCapabilities::from_limits(
            &Limits::default(),
            Some(DownlevelFlags::COMPUTE_SHADERS),
            Some(Backend::Vulkan),
            true,
        );
        assert!(capabilities.compute, "{capabilities:?}");
        assert!(capabilities.storage_buffers);
        assert_eq!(capabilities.max_compute_workgroup_size[0], 256);
        assert!(capabilities.compute_unavailable_reason().is_none());
    }

    /// 两条证据缺一不可：downlevel 置位但限制全 0，仍然不可用。
    #[test]
    fn flag_and_limits_are_both_required() {
        let capabilities = RendererCapabilities::from_limits(
            &Limits::downlevel_webgl2_defaults(),
            Some(DownlevelFlags::COMPUTE_SHADERS),
            Some(Backend::Gl),
            false,
        );
        assert!(!capabilities.compute);
    }
}
