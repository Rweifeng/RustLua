//! 确定性数据并行执行器（WP8）。
//!
//! 只处理**纯数据并行**：输入是共享只读切片，输出按同样的块长切开，每块独立就地写入。
//! 系统回调（`Scheduler`）不在这里并行——它的回调是 `!Send` 的 `FnMut(&mut C)`（捕获 Lua
//! 状态），无法跨线程；WP8 把并行限制在系统**内部**的无共享子任务上。
//!
//! 三条硬约束：
//! 1. **确定性**：分块只改变计算发生在哪个线程，不改变结果；`chunks` 与 `chunks_mut`
//!    顺序配对，因此并行与串行的字节完全相同（有测试逐字节断言）。
//! 2. **可回滚**：任何调用点都可以传 [`crate::par::Parallelism::SEQUENTIAL`] 退回单线程，不必改调用方
//!    代码，输出不变。
//! 3. **退化即串行**：线程创建失败、规模过小、只有一核时静默走串行，绝不 panic、绝不因此
//!    改变输出。

use std::thread;

/// 单次并行调用能用的最大线程数。
///
/// 上限存在的意义是**保留余量**：真机同时跑渲染/音频/IO，把 32 核全占满只会让帧时间抖动
/// 更大，而这类分块任务的收益早就饱和了。
pub const MAX_THREADS: usize = 8;

/// 低于该元素数**不并行**。默认阈值取自实测（`sprite_stress` 的 `parallel_ab`，Release，
/// 交替采样同一批次）：
///
/// | 精灵数 | 串行 build | 并行 build（最优线程数） | 结果 |
/// |---:|---:|---:|---|
/// | 10,000 | ~1,210 µs | 1,082 µs（4 线程） | 落在噪声内，**不划算** |
/// | 50,000 | ~5,930 µs | 4,698 µs（8 线程） | **−21%** |
/// | 100,000 | ~13,200 µs | 9,727 µs（8 线程） | **−26%** |
///
/// 收益随规模增长，而线程创建是固定成本，因此阈值取在两档之间：小场景完全不受影响，重场景
/// 才启用。
pub const MIN_PARALLEL_ITEMS: usize = 20_000;

/// 并行度配置：线程数 + 启用阈值。`threads == 0` 表示强制串行。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Parallelism {
    threads: usize,
    min_items: usize,
}

impl Parallelism {
    /// 强制串行（回滚开关，也是默认基准）。
    pub const SEQUENTIAL: Self = Self {
        threads: 0,
        min_items: 0,
    };

    /// 由调用方指定线程数，会被夹到 `[1, MAX_THREADS]`；阈值仍用 [`MIN_PARALLEL_ITEMS`]。
    #[must_use]
    pub const fn with_threads(threads: usize) -> Self {
        let threads = if threads == 0 {
            0
        } else if threads > MAX_THREADS {
            MAX_THREADS
        } else {
            threads
        };
        Self {
            threads,
            min_items: MIN_PARALLEL_ITEMS,
        }
    }

    /// 覆盖启用阈值。只有**已知自己这一批规模够大**的调用点（基准、离屏烘焙）才该用；
    /// 常规路径请依赖默认阈值。
    #[must_use]
    pub const fn with_min_items(mut self, min_items: usize) -> Self {
        self.min_items = min_items;
        self
    }

    /// 按当前机器的可用核数构造（拿不到信息时退回串行，不猜）。
    #[must_use]
    pub fn detected() -> Self {
        match thread::available_parallelism() {
            Ok(count) => Self::with_threads(count.get()),
            Err(_) => Self::SEQUENTIAL,
        }
    }

    /// 配置的线程数，`0` 表示串行。
    #[must_use]
    pub const fn threads(self) -> usize {
        self.threads
    }

    /// 当前启用阈值。
    #[must_use]
    pub const fn min_items(self) -> usize {
        self.min_items
    }

    /// 对 `len` 个元素实际会用的线程数（已计入串行退化条件）。
    #[must_use]
    pub const fn effective_threads(self, len: usize) -> usize {
        if self.threads < 2 || len < self.min_items {
            return 1;
        }
        if self.threads > len {
            return len;
        }
        self.threads
    }
}

impl Default for Parallelism {
    fn default() -> Self {
        Self::detected()
    }
}

/// 某个分块内**第一个**失败位置（输入切片的绝对下标）。
pub type ChunkFailure = usize;

/// 把 `items` 与 `output` 按同样的块长切开、逐块并行调用 `worker` 就地写入 `output`，
/// 并报告本次是否真的并行了。
///
/// 为什么是"就地写"而不是"返回 `Vec<O>` 再拼接"：拼接要把整份输出再拷一遍，还要为每个分块
/// 各分配一次缓冲。10k 精灵实测（`sprite_stress` 的 `parallel_ab`）——返回式实现无论起 2 个
/// 还是 8 个线程都稳定慢约 **700 µs**，且与线程数无关，正说明拖后腿的是这份额外分配与拷贝，
/// 不是线程创建。`output.chunks_mut` 天然给出**互不重叠**的 `&mut`，因此这里全是安全代码。
///
/// 约定：
/// - `output.len()` 必须是 `items.len()` 的整数倍（即"每项固定产出数"），由调用方按自己的
///   比例开好；
/// - `worker` 只读自己的输入块、只写自己的输出块，因此是纯数据并行；
/// - 任一块失败时整体返回**最小**失败位置，与串行时"第一个失败"一致——否则错误信息会随
///   线程调度变化。
pub fn map_chunks_in_place<I, O, F>(
    items: &[I],
    parallelism: Parallelism,
    output: &mut [O],
    worker: F,
) -> (Result<(), ChunkFailure>, bool)
where
    I: Sync,
    O: Send,
    F: Fn(&[I], &mut [O], usize) -> Result<(), ChunkFailure> + Send + Sync,
{
    let threads = parallelism.effective_threads(items.len());
    if threads < 2 {
        return (worker(items, output, 0), false);
    }
    let per_item = output.len() / items.len();
    debug_assert_eq!(output.len(), items.len() * per_item);
    let chunk_items = items.len().div_ceil(threads);
    let chunk_output = chunk_items * per_item;
    let mut failures: Vec<Option<ChunkFailure>> = (0..threads).map(|_| None).collect();
    // 线程只借 `worker`（`&F: Send + Copy`）；直接 `move` 它会被第一个线程吃掉。
    let worker_ref = &worker;
    let parallel = thread::scope(|scope| {
        let mut handles = Vec::with_capacity(threads);
        for (index, (chunk, out)) in items
            .chunks(chunk_items)
            .zip(output.chunks_mut(chunk_output))
            .enumerate()
        {
            let offset = index * chunk_items;
            match thread::Builder::new()
                .name("rustlua-par".to_string())
                .spawn_scoped(scope, move || (index, worker_ref(chunk, out, offset)))
            {
                Ok(handle) => handles.push(handle),
                // 线程起不来就退回串行：这是"失败自动降级"的落点，不是错误。
                Err(_) => return false,
            }
        }
        for handle in handles {
            let (index, outcome) = handle.join().unwrap_or_else(|payload| {
                std::panic::resume_unwind(payload);
            });
            failures[index] = outcome.err();
        }
        true
    });
    if !parallel {
        return (worker(items, output, 0), false);
    }
    (
        match failures.into_iter().flatten().min() {
            Some(position) => Err(position),
            None => Ok(()),
        },
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::{MAX_THREADS, MIN_PARALLEL_ITEMS, Parallelism, map_chunks_in_place};

    fn parallel() -> Parallelism {
        Parallelism::with_threads(4).with_min_items(1)
    }

    /// 每项产出 2 个输出：`worker` 把 `[value, value * 3]` 写进自己的输出块。
    ///
    /// 注意它**不把 `offset` 掺进输出**：`offset` 只用于错误定位。掺进去就成了随分块位置
    /// 变化的函数，串行与并行必然不同——本测试的第一版正是这样写错的。
    fn double_worker(
        chunk: &[u32],
        out: &mut [u32],
        _offset: usize,
    ) -> Result<(), super::ChunkFailure> {
        for (index, value) in chunk.iter().enumerate() {
            out[index * 2] = *value;
            out[index * 2 + 1] = value * 3;
        }
        Ok(())
    }

    #[test]
    fn sequential_and_parallel_produce_identical_bytes() {
        let items: Vec<u32> = (0..5_000).collect();
        let mut sequential = vec![0u32; items.len() * 2];
        let mut concurrent = vec![0u32; items.len() * 2];
        let (sequential_outcome, parallel_used) = map_chunks_in_place(
            &items,
            Parallelism::SEQUENTIAL,
            &mut sequential,
            double_worker,
        );
        let (concurrent_outcome, concurrent_used) =
            map_chunks_in_place(&items, parallel(), &mut concurrent, double_worker);
        assert_eq!(sequential_outcome, Ok(()));
        assert!(!parallel_used);
        assert!(concurrent_used);
        assert_eq!(concurrent_outcome, Ok(()));
        assert_eq!(sequential, concurrent);
    }

    #[test]
    fn small_inputs_and_single_thread_stay_sequential() {
        let items: Vec<u32> = (0..MIN_PARALLEL_ITEMS - 1)
            .map(|value| value as u32)
            .collect();
        let mut output = vec![0u32; items.len() * 2];
        let default_threads = Parallelism::with_threads(4);
        let (_, used) = map_chunks_in_place(&items, default_threads, &mut output, double_worker);
        assert!(!used, "低于默认阈值不该起线程");
        let (_, used) =
            map_chunks_in_place(&items, Parallelism::SEQUENTIAL, &mut output, double_worker);
        assert!(!used);
        assert_eq!(default_threads.effective_threads(100_000), 4);
        assert_eq!(Parallelism::with_threads(1).effective_threads(100_000), 1);
        assert_eq!(Parallelism::SEQUENTIAL.effective_threads(100_000), 1);
    }

    #[test]
    fn threshold_can_be_overridden_explicitly() {
        let parallelism = Parallelism::with_threads(4).with_min_items(8);
        assert_eq!(parallelism.min_items(), 8);
        assert_eq!(parallelism.effective_threads(64), 4);
        assert_eq!(parallelism.effective_threads(4), 1);
        // 覆盖阈值不影响别的实例：默认阈值下小规模照样串行。
        assert_eq!(Parallelism::with_threads(4).effective_threads(64), 1);
    }

    #[test]
    fn thread_count_is_capped_and_never_exceeds_the_item_count() {
        assert_eq!(Parallelism::with_threads(999).threads(), MAX_THREADS);
        assert_eq!(Parallelism::with_threads(0).effective_threads(100_000), 1);
        let default_threads = Parallelism::with_threads(8);
        assert_eq!(default_threads.effective_threads(100_000), 8);
        assert_eq!(default_threads.effective_threads(MIN_PARALLEL_ITEMS), 8);
        // 元素数少于线程数时不该起多余线程。
        assert_eq!(
            Parallelism::with_threads(8)
                .with_min_items(1)
                .effective_threads(3),
            3
        );
    }

    #[test]
    fn lowest_failing_position_wins_regardless_of_chunk_order() {
        let items: Vec<u32> = (0..5_000).collect();
        let mut output = vec![0u32; items.len() * 2];
        // 每个分块都报自己块内的第一个失败，整体必须取最小位置（= 串行时的"第一个失败"）。
        let (outcome, used) = map_chunks_in_place(
            &items,
            parallel(),
            &mut output,
            |chunk, _, offset| match chunk.iter().position(|value| value % 1000 == 0) {
                Some(index) => Err(offset + index),
                None => Ok(()),
            },
        );
        assert!(used);
        let expected = items.iter().position(|value| value % 1000 == 0);
        assert_eq!(outcome, expected.map_or(Ok(()), Err));
    }

    #[test]
    fn empty_input_is_sequential_and_empty() {
        let items: Vec<u32> = Vec::new();
        let mut output: Vec<u32> = Vec::new();
        let (outcome, used) = map_chunks_in_place(&items, parallel(), &mut output, double_worker);
        assert!(!used);
        assert_eq!(outcome, Ok(()));
        assert!(output.is_empty());
    }
}
