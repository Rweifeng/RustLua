//! E06-3：逐渲染通道的 GPU timestamp 记录与解析。
//!
//! 整帧时间（[`super::timestamp`]）只能回答"这一帧 GPU 花了多久"；"阴影与网格各占多少"
//! 需要每个通道一对查询。做法与整帧一致：`write_timestamp` 包住通道、resolve + 异步回读，
//! 设备不支持时整条路径不产生查询（报告里是 `supported=false`，不伪造 0）。
//!
//! 两处诚实性约束：
//!
//! - 通道必须**先 drop 再写结束时间戳**：render pass 还活着时 `write_timestamp` 是校验错误。
//! - 2D overlay 与 ScreenUI 共用一次 `begin_render_pass`（同管线、同附件），没有
//!   inside-pass 时间戳能力就无法拆开；整段记在 `Legacy2DOverlay` 上，ScreenUI 不单列。
//!   报告里的 `pass_sum` 因此不会重复计这一段。

use rustlua_render::render_plan::RenderPass;

use super::PassGpuTiming;

/// 最多能包住的通道对数。`RenderPass::ALL` 有 11 条，其中 ScreenUI 与 overlay 共用一对，
/// 因此实际最多 10 对；留一条余量给被放弃的通道（例如粒子 pass 最终没画）。
pub(super) const MAX_PASS_PAIRS: usize = RenderPass::ALL.len();
/// 查询总数：帧开始 + 每个通道的起止 + 一条被放弃通道的空洞 + 帧结束。
///
/// 空洞来自"写了起点、但通道最终没执行"（目前只有粒子通道会这样）：放弃时该索引不再使用，
/// 因此计数要把它算进去，否则帧结束时间戳会越界。
pub(super) const TIMESTAMP_QUERY_COUNT: u32 = 2 + 2 * MAX_PASS_PAIRS as u32 + 1;
/// resolve / readback 缓冲字节数（一个 timestamp = 8 字节）。
pub(super) const TIMESTAMP_BUFFER_BYTES: u64 = (TIMESTAMP_QUERY_COUNT as u64) * 8;
/// per-pass 和与整帧的一致性预算（docs/55：±5%）。
pub(super) const CONSISTENCY_TOLERANCE_PCT: f32 = 5.0;

/// 一次提交里实际写入的查询对。索引显式记录，因此中途放弃的通道不会让后面的对错位。
#[derive(Clone, Debug, Default)]
pub(super) struct PassTimestampLayout {
    pub pairs: Vec<(RenderPass, u32, u32)>,
    pub frame_end: u32,
}

/// 在一帧的编码过程中累积查询。`query_set=None`（设备不支持）时所有方法都是空操作。
pub(super) struct PassTimestamps<'a> {
    query_set: Option<&'a wgpu::QuerySet>,
    next: u32,
    open: Option<u32>,
    pairs: Vec<(RenderPass, u32, u32)>,
}

impl<'a> PassTimestamps<'a> {
    pub(super) fn new(query_set: Option<&'a wgpu::QuerySet>) -> Self {
        Self {
            query_set,
            next: 1,
            open: None,
            pairs: Vec::new(),
        }
    }

    /// 在通道开始前写起点。返回 `true` 表示这一对真的写进了查询集。
    pub(super) fn begin(&mut self, encoder: &mut wgpu::CommandEncoder) -> bool {
        self.open = None;
        let Some(query_set) = self.query_set else {
            return false;
        };
        if self.pairs.len() >= MAX_PASS_PAIRS {
            return false;
        }
        encoder.write_timestamp(query_set, self.next);
        self.open = Some(self.next);
        self.next += 1;
        true
    }

    /// 通道结束后写终点并登记。没有配对的 `begin` 时是空操作。
    pub(super) fn end(&mut self, encoder: &mut wgpu::CommandEncoder, pass: RenderPass) {
        let Some(begin) = self.open.take() else {
            return;
        };
        let Some(query_set) = self.query_set else {
            return;
        };
        encoder.write_timestamp(query_set, self.next);
        self.pairs.push((pass, begin, self.next));
        self.next += 1;
    }

    /// 放弃已经写了起点的通道（它最终没执行）。留下的查询空洞不参与解析。
    pub(super) fn abandon(&mut self) {
        self.open = None;
    }

    pub(super) fn finish(self) -> PassTimestampLayout {
        PassTimestampLayout {
            pairs: self.pairs,
            frame_end: self.next,
        }
    }
}

fn micros(begin: u64, end: u64, period_ns: f32) -> u64 {
    let nanos = end.saturating_sub(begin) as f64 * f64::from(period_ns);
    (nanos / 1_000.0).round() as u64
}

/// 从回读的原始时间戳解析每通道时长与整帧时长。`raw` 长度不足时返回 `None`。
pub(super) fn parse_pass_timings(
    raw: &[u64],
    layout: &PassTimestampLayout,
    period_ns: f32,
) -> Option<(u64, Vec<PassGpuTiming>)> {
    let frame_end = layout.frame_end as usize;
    if frame_end >= raw.len() {
        return None;
    }
    let frame_us = micros(raw[0], raw[frame_end], period_ns);
    let mut timings = Vec::with_capacity(layout.pairs.len());
    for &(pass, begin, end) in &layout.pairs {
        let (begin, end) = (begin as usize, end as usize);
        if begin >= raw.len() || end >= raw.len() {
            return None;
        }
        timings.push(PassGpuTiming {
            pass,
            gpu_us: micros(raw[begin], raw[end], period_ns),
        });
    }
    Some((frame_us, timings))
}

/// `(pass_sum - frame) / frame * 100`。整帧为 0 时没有百分比可言。
pub(super) fn consistency_delta_pct(frame_us: u64, pass_sum_us: u64) -> Option<f32> {
    if frame_us == 0 {
        return None;
    }
    Some((pass_sum_us as f64 - frame_us as f64) as f32 * 100.0 / frame_us as f32)
}

pub(super) fn within_tolerance(delta_pct: f32) -> bool {
    delta_pct.abs() <= CONSISTENCY_TOLERANCE_PCT
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(pairs: &[(RenderPass, u32, u32)], frame_end: u32) -> PassTimestampLayout {
        PassTimestampLayout {
            pairs: pairs.to_vec(),
            frame_end,
        }
    }

    #[test]
    fn parses_each_pass_and_frame_from_the_same_query_set() {
        // 0 = 帧起点；Shadow 1..2；DepthOpaque 3..4；帧终点 5。
        let raw = [0, 1_000_000, 3_000_000, 4_000_000, 9_900_000, 10_000_000];
        let layout = layout(
            &[(RenderPass::Shadow, 1, 2), (RenderPass::DepthOpaque, 3, 4)],
            5,
        );
        let (frame_us, passes) =
            parse_pass_timings(&raw, &layout, 1.0).expect("complete timestamps");
        assert_eq!(frame_us, 10_000);
        assert_eq!(passes.len(), 2);
        assert_eq!(passes[0].pass, RenderPass::Shadow);
        assert_eq!(passes[0].gpu_us, 2_000);
        assert_eq!(passes[1].pass, RenderPass::DepthOpaque);
        assert_eq!(passes[1].gpu_us, 5_900);
    }

    #[test]
    fn abandoned_pair_does_not_shift_later_passes() {
        // 1 是放弃的起点；2..3 才是真正的第一个通道。
        let raw = [0, 500, 1_000_000, 2_000_000, 3_000_000, 3_000_000];
        let layout = layout(&[(RenderPass::Clear, 2, 3)], 4);
        let (frame_us, passes) = parse_pass_timings(&raw, &layout, 1.0).expect("timestamps");
        assert_eq!(frame_us, 3_000);
        assert_eq!(passes[0].pass, RenderPass::Clear);
        assert_eq!(passes[0].gpu_us, 1_000);
    }

    #[test]
    fn incomplete_readback_is_rejected() {
        let raw = [0, 1, 2];
        let layout = layout(&[(RenderPass::Shadow, 1, 2)], 3);
        assert!(parse_pass_timings(&raw, &layout, 1.0).is_none());
    }

    #[test]
    fn consistency_uses_five_percent_boundary() {
        assert_eq!(consistency_delta_pct(10_000, 10_000), Some(0.0));
        let inside = consistency_delta_pct(10_000, 10_400).expect("non-zero frame");
        assert!(within_tolerance(inside), "4% 应在预算内：{inside}");
        let outside = consistency_delta_pct(10_000, 10_600).expect("non-zero frame");
        assert!(!within_tolerance(outside), "6% 应判红：{outside}");
        assert_eq!(consistency_delta_pct(0, 0), None);
    }
}
