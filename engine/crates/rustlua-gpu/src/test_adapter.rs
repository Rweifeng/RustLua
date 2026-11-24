//! 测试用 wgpu 适配器策略：**唯一入口**，不在每个测试文件里各写一份。
//!
//! ## 为什么需要它
//!
//! 仓库里曾有 10 个 `test_device()` 各自写死 `force_fallback_adapter: true`。它的本意是
//! "CI 上没有显卡时测试不至于直接失败"——这个意图是对的。但在有独显的开发机上，它把
//! 每个 GPU 测试都推到 `Microsoft Basic Render Driver`（D3D12 的软件光栅化）上跑，
//! 实测单测试从 **0.1s 级涨到 20s 级**，327 个测试串跑必然超时被杀。
//!
//! 于是现象看起来像"GPU 测试跑不完/不稳定"，实际是**测试从来没碰过显卡**。
//!
//! ## 策略
//!
//! | `RUSTLUA_TEST_GPU` | 行为 | 用途 |
//! |---|---|---|
//! | 未设置 / `software` | 强制软件回退（旧行为） | CI、无显卡机器；默认值，保持不变 |
//! | `hardware` | 用真实 GPU；拿不到就**回退到软件**并打印一行 | 有独显的开发机 |
//! | `strict` | 用真实 GPU；拿不到直接 panic | 需要确认"这机器真能跑硬件路径" |
//!
//! 默认保持 `software` 是有意的：**不改默认行为**就不会让既有的 CI 结果发生漂移。
//! 想用显卡就显式声明，这种"可见的开关"比悄悄提速更容易解释。

use std::sync::OnceLock;

/// 本机适应器选择模式的快照（进程内只读一次环境变量）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestAdapterMode {
    /// 强制软件回退。CI 默认。
    Software,
    /// 优先真实 GPU，拿不到退回软件。
    Hardware,
    /// 必须真实 GPU，拿不到 panic。
    Strict,
}

impl TestAdapterMode {
    /// 从 `RUSTLUA_TEST_GPU` 读取；未设置或无法识别时按 `Software` 处理。
    #[must_use]
    pub fn from_env() -> Self {
        match std::env::var("RUSTLUA_TEST_GPU")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "hardware" => Self::Hardware,
            "strict" => Self::Strict,
            _ => Self::Software,
        }
    }

    /// 进程内缓存的模式（避免每个测试都读一次环境变量）。
    #[must_use]
    pub fn cached() -> Self {
        static MODE: OnceLock<TestAdapterMode> = OnceLock::new();
        *MODE.get_or_init(Self::from_env)
    }

    /// 这一档是否**要求**软件回退（仅 `Software` 档为真）。
    #[must_use]
    pub fn forces_software(self) -> bool {
        matches!(self, Self::Software)
    }
}

impl std::fmt::Display for TestAdapterMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Software => "software",
            Self::Hardware => "hardware",
            Self::Strict => "strict",
        };
        f.write_str(label)
    }
}

/// 按 [`TestAdapterMode`] 申请一个适配器，返回适配器与其描述。
///
/// 这是 10 处 `test_device()` 的**共享实现**：它们只差 `tag`（device label），
/// 申请逻辑不该各写一份——`force_fallback_adapter` 这种"一改就影响全部测试"的开关，
/// 散落 10 份就是 10 个漂移点。
///
/// `pollster::block_on` 在这里而不是调用方：适配器申请是同步等待，调用方只关心结果。
#[must_use]
pub fn request_test_adapter() -> (Option<wgpu::Adapter>, TestAdapterMode) {
    let mode = TestAdapterMode::cached();
    // 每次申请都新建 instance：`request_adapter` 借用 instance，而这里可能申请两次
    // （先试硬件、再退回软件），共享一个 instance 就得处理借用周期，不值得。
    let request = |fallback: bool| {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            // 硬件档用 HighPerformance：低功耗偏好会让某些平台挑到核显。
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: fallback,
        }))
        .ok()
    };

    if mode.forces_software() {
        return (request(true), mode);
    }

    // hardware / strict：先要真实 GPU；`force_fallback_adapter: false` 可能仍返回软件设备
    // （某些驱动把它报成可用适配器），所以再按 `device_type` 筛一遍。
    let hardware = request(false)
        .filter(|adapter| !matches!(adapter.get_info().device_type, wgpu::DeviceType::Cpu));
    if hardware.is_some() {
        return (hardware, mode);
    }

    if matches!(mode, TestAdapterMode::Strict) {
        panic!(
            "RUSTLUA_TEST_GPU=strict 但没拿到真实 GPU 适配器；\
             用 `cargo run -p rustlua-gpu --example gpu_probe` 看本机有哪些适配器"
        );
    }
    // hardware 档拿不到就退回软件：开发机上"能跑"比"跑到硬件路径"更重要。
    eprintln!("RUSTLUA_TEST_GPU=hardware 但没拿到真实 GPU，退回软件渲染器");
    (request(true), mode)
}

#[cfg(test)]
mod tests {
    use super::TestAdapterMode;

    #[test]
    fn unknown_or_absent_env_means_software() {
        // 直接测解析函数在给定字符串下的行为，避免改动进程环境（测试并发不安全）。
        assert_eq!(parse(""), TestAdapterMode::Software);
        assert_eq!(parse("nonsense"), TestAdapterMode::Software);
        assert_eq!(parse("SOFTWARE"), TestAdapterMode::Software);
        assert_eq!(parse(" hardware "), TestAdapterMode::Hardware);
        assert_eq!(parse("STRICT"), TestAdapterMode::Strict);
    }

    fn parse(raw: &str) -> TestAdapterMode {
        match raw.trim().to_ascii_lowercase().as_str() {
            "hardware" => TestAdapterMode::Hardware,
            "strict" => TestAdapterMode::Strict,
            _ => TestAdapterMode::Software,
        }
    }
}
