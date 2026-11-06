//! 进程内存采样：F3 overlay 的 `MEM / PEAK / PRIV` 行。
//!
//! 只用宿主已有的平台依赖（Windows 走 `windows-sys`，Linux 读 `/proc`），
//! 不引入 `sysinfo` 之类的整棵依赖树。采样失败返回 `None`，overlay 显示 `--`，
//! 不让诊断面板成为新的失败点。

/// 一次进程内存读数，单位 MB。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemoryStats {
    /// 当前工作集 / 常驻内存（Windows `WorkingSetSize`，Linux `VmRSS`）。
    pub working_set_mb: f32,
    /// 峰值工作集（Windows `PeakWorkingSetSize`，Linux `VmHWM`）。
    pub peak_working_set_mb: f32,
    /// 私有提交（Windows `PagefileUsage`，Linux `RssAnon` 的近似）。
    pub private_mb: f32,
}

/// 当前平台的进程内存；不支持或读取失败时为 `None`。
#[must_use]
pub fn sample() -> Option<MemoryStats> {
    imp::sample()
}

#[cfg(windows)]
mod imp {
    use super::MemoryStats;
    use std::mem::size_of;
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    pub(super) fn sample() -> Option<MemoryStats> {
        let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        counters.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        // SAFETY: `counters` 是本栈上的有效可写结构，`cb` 已按结构大小填好；
        // 句柄是当前进程的伪句柄，调用期间不会失效。
        let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
        if ok == 0 {
            return None;
        }
        const MB: f32 = 1024.0 * 1024.0;
        Some(MemoryStats {
            working_set_mb: counters.WorkingSetSize as f32 / MB,
            peak_working_set_mb: counters.PeakWorkingSetSize as f32 / MB,
            private_mb: counters.PagefileUsage as f32 / MB,
        })
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::MemoryStats;
    use std::fs;

    fn kb(line: &str) -> Option<f32> {
        let rest = line.split_once(':')?.1;
        let value = rest.split_whitespace().next()?.parse::<f32>().ok()?;
        Some(value / 1024.0)
    }

    pub(super) fn sample() -> Option<MemoryStats> {
        let status = fs::read_to_string("/proc/self/status").ok()?;
        let mut working = None;
        let mut peak = None;
        let mut private = None;
        for line in status.lines() {
            if line.starts_with("VmRSS:") {
                working = kb(line);
            } else if line.starts_with("VmHWM:") {
                peak = kb(line);
            } else if line.starts_with("RssAnon:") {
                private = kb(line);
            }
        }
        let working_set_mb = working?;
        Some(MemoryStats {
            working_set_mb,
            peak_working_set_mb: peak.unwrap_or(working_set_mb),
            private_mb: private.unwrap_or(working_set_mb),
        })
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    use super::MemoryStats;

    pub(super) fn sample() -> Option<MemoryStats> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::sample;

    #[test]
    fn sample_is_plausible_when_the_platform_supports_it() {
        if let Some(memory) = sample() {
            assert!(memory.working_set_mb > 0.0);
            assert!(memory.peak_working_set_mb >= memory.working_set_mb);
            assert!(memory.private_mb > 0.0);
            // 任何真实进程都不该在这三行上超过 1 PB；防单位换算写错。
            assert!(memory.working_set_mb < 1024.0 * 1024.0 * 1024.0);
        }
    }
}
