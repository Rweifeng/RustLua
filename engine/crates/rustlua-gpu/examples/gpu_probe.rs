//! 诊断工具：列出本机所有 wgpu 适配器，并对比真实 GPU 与软件回退的可用性。
//!
//! 为什么需要它：仓库里 80 处 `test_device` 都写死 `force_fallback_adapter: true`，
//! 于是测试永远跑软件光栅化（WARP/lavapipe）。在只有集成显卡或没显卡的 CI 上这是必要的
//! （否则测试会因"没有适配器"直接失败），但在有独显的开发机上，它让每个测试慢到几十秒。
//!
//! 这个工具不改任何行为，只**把事实打出来**：本机有哪些适配器、哪些能被 wgpu 用、
//! 各自的 backend 与软/硬属性。清理测试策略前先用它确认基线。
//!
//! 用法：`cargo run -p rustlua-gpu --example gpu_probe`

fn main() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapters = instance.enumerate_adapters(wgpu::Backends::all());
    println!("enumerate_adapters -> {} 个", adapters.len());
    for (index, adapter) in adapters.iter().enumerate() {
        let info = adapter.get_info();
        println!(
            "  [{index}] name={:?} backend={:?} device_type={:?} driver={:?} driver_info={:?}",
            info.name, info.backend, info.device_type, info.driver, info.driver_info
        );
    }

    // `force_fallback_adapter: true`：只要有软件回退就选它，独显会被完全跳过。
    let fallback = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: true,
    }));
    match &fallback {
        Ok(adapter) => {
            let info = adapter.get_info();
            println!(
                "\nforce_fallback_adapter=true  -> {:?} ({:?}, {:?})",
                info.name, info.backend, info.device_type
            );
        }
        Err(error) => println!("\nforce_fallback_adapter=true  -> 失败: {error}"),
    }

    // 默认偏好（不强制回退）：应当拿到真实 GPU。
    let preferred = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }));
    match &preferred {
        Ok(adapter) => {
            let info = adapter.get_info();
            println!(
                "force_fallback_adapter=false -> {:?} ({:?}, {:?})",
                info.name, info.backend, info.device_type
            );
            println!("\n=> 测试改走真实 GPU 可行");
        }
        Err(error) => println!("\nforce_fallback_adapter=false -> 失败: {error}"),
    }
}
