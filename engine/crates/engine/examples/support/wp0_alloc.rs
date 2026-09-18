//! WP0 基准共用的计数分配器。
//!
//! 只在基准二进制里挂全局分配器：统计本身带原子开销，绝不能出现在引擎运行时路径上。
//! 引擎自身的分配统计由 WP0 报告的 `cpu_alloc_*` 字段负责，两者口径不同、互不替代。
//!
//! 本文件由各基准用 `#[path]` 引入，因此"本二进制用不到某个统计入口"是正常情况。
#![allow(dead_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

static ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static PEAK_BYTES: AtomicU64 = AtomicU64::new(0);
static BASELINE_LIVE_BYTES: AtomicU64 = AtomicU64::new(0);

struct CountingAllocator;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            record(layout.size() as u64);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE_BYTES.fetch_sub(layout.size() as u64, Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            let old = layout.size() as u64;
            let new = new_size as u64;
            record(new);
            // `record` 已把新尺寸计入存活量，这里只需退回旧尺寸。
            LIVE_BYTES.fetch_sub(old, Ordering::Relaxed);
        }
        new_ptr
    }
}

fn record(size: u64) {
    ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
    ALLOC_BYTES.fetch_add(size, Ordering::Relaxed);
    let live = LIVE_BYTES.fetch_add(size, Ordering::Relaxed) + size;
    let mut peak = PEAK_BYTES.load(Ordering::Relaxed);
    while live > peak {
        match PEAK_BYTES.compare_exchange_weak(peak, live, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(current) => peak = current,
        }
    }
}

/// 清零本次采样窗口的累计计数，并把峰值基线设为当前存活字节。
pub fn reset_window() {
    ALLOC_COUNT.store(0, Ordering::Relaxed);
    ALLOC_BYTES.store(0, Ordering::Relaxed);
    let baseline = LIVE_BYTES.load(Ordering::Relaxed);
    BASELINE_LIVE_BYTES.store(baseline, Ordering::Relaxed);
    PEAK_BYTES.store(baseline, Ordering::Relaxed);
}

/// 本次窗口内的分配次数。
pub fn window_count() -> u64 {
    ALLOC_COUNT.load(Ordering::Relaxed)
}

/// 本次窗口内请求的字节总数（含 realloc 的新尺寸）。
pub fn window_bytes() -> u64 {
    ALLOC_BYTES.load(Ordering::Relaxed)
}

/// 相对窗口起点的峰值增量。
pub fn window_peak_delta_bytes() -> u64 {
    PEAK_BYTES
        .load(Ordering::Relaxed)
        .saturating_sub(BASELINE_LIVE_BYTES.load(Ordering::Relaxed))
}
