//! 帧调度分档统计（M74-02 P0 度量闸门）：把每帧的 [`FrameScheduleReport`] 累积成
//! 每系统 p50/p99、每 stage 合计与"最慢 stage 占整帧比"。
//!
//! 为什么要在 `ScheduleReport` 之外再做一层：一次 `run_script_steps` 的读数只描述**一次**
//! 管线运行，而"并行调度值不值得做"问的是稳态分布——首帧、场景切换、GC 都会把单次读数
//! 带偏。这里按帧累积后取分位数，输出直接回答闸门问题：
//! [`FrameProfileSummary::gate_share_pct`]（最慢 stage 的串行系统合计占整帧多少）。
//!
//! **整帧墙钟由调用方传入**：调度器只知道自己那一段，2D/3D 提取与渲染提交都不在里面。
//! 拿调度器总耗时当分母会把占比算大好几倍，结论就从"要不要并行调度"变成"调度占调度
//! 自己多少"——那不回答任何有意义的问题。

use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};

use rustlua_ecs::scheduler::ScheduleStage;

use super::runtime_scene::FrameScheduleReport;

/// P0 闸门的硬判据（任务文件 §阶段拆分）：最慢 stage 的串行系统合计**低于**整帧的这个
/// 百分比 ⇒ 并行调度的收益上限不足两成，结论是**不做**。
pub const PARALLELISM_GATE_SHARE_PCT: f64 = 20.0;

/// 一组耗时样本的分布。样本一律以纳秒存：`clock_step` 这类系统在微秒下会被截断成 0，
/// 而"它到底有没有占时间"必须看得出来（见 `TimingStats::p50_us`）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TimingStats {
    pub count: usize,
    pub min_ns: u64,
    pub mean_ns: u64,
    pub p50_ns: u64,
    pub p95_ns: u64,
    pub p99_ns: u64,
    pub max_ns: u64,
}

impl TimingStats {
    /// 从样本算分布。空样本返回全 0（`count == 0`），调用方不必先判空。
    ///
    /// 分位数用 nearest-rank：`((len × p) − 1) / 100`，与仓内既有基准示例
    /// （`wp0_scene_frame::summarize_with`）同口径，避免两处"p99"含义不同。
    #[must_use]
    pub fn from_samples(samples: &[u64]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        let at = |percent: usize| {
            ((sorted.len() * percent).saturating_sub(1) / 100).min(sorted.len() - 1)
        };
        let sum = sorted.iter().map(|value| u128::from(*value)).sum::<u128>();
        Self {
            count: sorted.len(),
            min_ns: sorted[0],
            mean_ns: u64::try_from(sum / sorted.len() as u128).unwrap_or(u64::MAX),
            p50_ns: sorted[at(50)],
            p95_ns: sorted[at(95)],
            p99_ns: sorted[at(99)],
            max_ns: sorted[sorted.len() - 1],
        }
    }

    #[must_use]
    pub fn min_us(&self) -> f64 {
        self.min_ns as f64 / 1000.0
    }

    #[must_use]
    pub fn mean_us(&self) -> f64 {
        self.mean_ns as f64 / 1000.0
    }

    #[must_use]
    pub fn p50_us(&self) -> f64 {
        self.p50_ns as f64 / 1000.0
    }

    #[must_use]
    pub fn p95_us(&self) -> f64 {
        self.p95_ns as f64 / 1000.0
    }

    #[must_use]
    pub fn p99_us(&self) -> f64 {
        self.p99_ns as f64 / 1000.0
    }

    #[must_use]
    pub fn max_us(&self) -> f64 {
        self.max_ns as f64 / 1000.0
    }

    fn json(&self) -> String {
        format!(
            "{{\"count\":{},\"min_us\":{:.3},\"mean_us\":{:.3},\"p50_us\":{:.3},\
             \"p95_us\":{:.3},\"p99_us\":{:.3},\"max_us\":{:.3}}}",
            self.count,
            self.min_us(),
            self.mean_us(),
            self.p50_us(),
            self.p95_us(),
            self.p99_us(),
            self.max_us()
        )
    }
}

#[derive(Debug)]
struct SystemSamples {
    stage: ScheduleStage,
    samples: Vec<u64>,
}

/// 跨帧累积的调度分档统计。每帧调用一次 [`Self::record`]，结束时用 [`Self::summary`] 出报告。
#[derive(Debug, Default)]
pub struct FrameProfile {
    frames: u64,
    /// 每帧整段墙钟（含提取与提交，见模块注释）。
    frame_ns: Vec<u64>,
    /// 每帧调度器自报的管线总耗时。
    schedule_ns: Vec<u64>,
    /// 系统的首次执行顺序（报告与 CLI 都按它稳定输出，不随 HashMap 迭代顺序抖动）。
    system_order: Vec<String>,
    systems: HashMap<String, SystemSamples>,
    /// 每帧该 stage 内所有系统耗时之和。
    stage_ns: BTreeMap<ScheduleStage, Vec<u64>>,
    /// 每帧该 stage 内**最慢单个系统**的耗时（并行后的关键路径下界）。
    stage_max_ns: BTreeMap<ScheduleStage, Vec<u64>>,
}

impl FrameProfile {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一帧。`frame_elapsed` 是这一帧的整段墙钟（调用方计时）。
    ///
    /// 一个 `FrameScheduleReport` 可能含**多个**固定步（`report.steps > 1`）：此时每个系统的
    /// 耗时是这一步数内的累计，stage 合计也是。这与"这一帧确实付了这么多"一致，不做平均——
    /// 平均会把追帧帧的真实成本抹掉。
    pub fn record(&mut self, report: &FrameScheduleReport, frame_elapsed: Duration) {
        self.frames += 1;
        self.frame_ns.push(nanos(frame_elapsed));
        self.schedule_ns.push(nanos(report.total_elapsed));

        let mut per_stage_sum: BTreeMap<ScheduleStage, u64> = BTreeMap::new();
        let mut per_stage_max: BTreeMap<ScheduleStage, u64> = BTreeMap::new();
        for timing in &report.timings {
            let elapsed = nanos(timing.elapsed);
            if !self.systems.contains_key(&timing.name) {
                self.system_order.push(timing.name.clone());
            }
            self.systems
                .entry(timing.name.clone())
                .or_insert_with(|| SystemSamples {
                    stage: timing.stage,
                    samples: Vec::new(),
                })
                .samples
                .push(elapsed);
            *per_stage_sum.entry(timing.stage).or_default() += elapsed;
            let max = per_stage_max.entry(timing.stage).or_default();
            *max = (*max).max(elapsed);
        }
        for (stage, elapsed) in per_stage_sum {
            self.stage_ns.entry(stage).or_default().push(elapsed);
        }
        for (stage, elapsed) in per_stage_max {
            self.stage_max_ns.entry(stage).or_default().push(elapsed);
        }
        // 没有系统的 stage 不落样本：报告只为**真正执行过**的系统记时间，
        // 空 stage 不该在统计里以 0 出现、把"最慢 stage"的排序搅乱。
    }

    /// 已记录的帧数。
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// 汇总成可序列化/可断言的报告。
    #[must_use]
    pub fn summary(&self) -> FrameProfileSummary {
        let frame = TimingStats::from_samples(&self.frame_ns);
        let schedule = TimingStats::from_samples(&self.schedule_ns);
        let frame_p50 = frame.p50_ns as f64;

        let mut systems = Vec::with_capacity(self.system_order.len());
        for name in &self.system_order {
            let Some(samples) = self.systems.get(name) else {
                continue;
            };
            let elapsed = TimingStats::from_samples(&samples.samples);
            systems.push(SystemProfile {
                name: name.clone(),
                stage: samples.stage,
                elapsed,
                share_pct: percent(elapsed.p50_ns as f64, frame_p50),
            });
        }

        let mut stages = Vec::with_capacity(self.stage_ns.len());
        for (stage, samples) in &self.stage_ns {
            let elapsed = TimingStats::from_samples(samples);
            let critical_path = TimingStats::from_samples(
                self.stage_max_ns.get(stage).map_or(&[][..], Vec::as_slice),
            );
            let systems_in_stage = systems.iter().filter(|entry| entry.stage == *stage).count();
            // 并行的收益上限：最慢 stage 里除最慢那一个系统之外的部分都能被重叠掉。
            // 用 p50 相减（而不是各取最大值）保持"典型帧"的口径。
            let gain_ns = elapsed.p50_ns.saturating_sub(critical_path.p50_ns);
            stages.push(StageProfile {
                stage: *stage,
                systems: systems_in_stage,
                elapsed,
                share_pct: percent(elapsed.p50_ns as f64, frame_p50),
                critical_path,
                gain_ceiling_pct: percent(gain_ns as f64, frame_p50),
            });
        }
        let worst_stage = stages
            .iter()
            .max_by(|left, right| {
                left.share_pct
                    .total_cmp(&right.share_pct)
                    // 百分比相等时按阶段序（rank 小的在前）稳定取第一个。
                    .then_with(|| right.stage.rank().cmp(&left.stage.rank()))
            })
            .cloned();

        FrameProfileSummary {
            frames: self.frames,
            frame,
            schedule,
            serial_share_pct: percent(schedule.p50_ns as f64, frame_p50),
            stages,
            systems,
            worst_stage,
        }
    }
}

/// 单个系统的分档。
#[derive(Clone, Debug, PartialEq)]
pub struct SystemProfile {
    pub name: String,
    pub stage: ScheduleStage,
    pub elapsed: TimingStats,
    /// 该系统每帧耗时占整帧 p50 的百分比。
    pub share_pct: f64,
}

/// 单个 stage 的分档。
#[derive(Clone, Debug, PartialEq)]
pub struct StageProfile {
    pub stage: ScheduleStage,
    /// 该 stage 里登记过的系统数。
    pub systems: usize,
    /// 每帧该 stage 内所有系统耗时**之和**的分布。
    pub elapsed: TimingStats,
    /// stage 合计（p50）占整帧 p50 的百分比——P0 闸门读的就是它的最大值。
    pub share_pct: f64,
    /// 该 stage 内最慢单个系统的分布（并行后的关键路径）。
    pub critical_path: TimingStats,
    /// 该 stage 并行化的收益上限占帧比：`(stage 合计 − 关键路径) / 帧`。
    pub gain_ceiling_pct: f64,
}

/// 一份汇总报告。
#[derive(Clone, Debug, PartialEq)]
pub struct FrameProfileSummary {
    pub frames: u64,
    pub frame: TimingStats,
    pub schedule: TimingStats,
    /// 全部调度系统之和（p50）占整帧 p50 的百分比。
    pub serial_share_pct: f64,
    /// 按 stage rank 升序。
    pub stages: Vec<StageProfile>,
    /// 按首次执行顺序。
    pub systems: Vec<SystemProfile>,
    /// 占帧比最高的 stage（没有系统时为 `None`）。
    pub worst_stage: Option<StageProfile>,
}

impl FrameProfileSummary {
    /// 闸门读数：最慢 stage 的串行系统合计占整帧的百分比。没有系统时为 0。
    #[must_use]
    pub fn gate_share_pct(&self) -> f64 {
        self.worst_stage.as_ref().map_or(0.0, |stage| stage.share_pct)
    }

    /// 硬判据：`true` = 收益上限不足两成，**不做**并行调度（以实测结论收口）。
    ///
    /// 边界取"低于 20%"：恰好 20.0 不算通过闸门（有理由继续），因为判据原文是"< 20%"。
    #[must_use]
    pub fn passes_gate(&self) -> bool {
        self.gate_share_pct() < PARALLELISM_GATE_SHARE_PCT
    }

    /// 结论词（写进 JSON 与单行摘要，便于脚本断言）。
    #[must_use]
    pub fn verdict(&self) -> &'static str {
        if self.passes_gate() { "skip" } else { "proceed" }
    }

    /// 单行摘要（给 stdout / CI 断言读）。
    #[must_use]
    pub fn summary_line(&self) -> String {
        let worst_stage = self
            .worst_stage
            .as_ref()
            .map_or_else(|| "none".to_owned(), |stage| stage.stage.to_string());
        format!(
            "m74_frame_profile frames={} frame_p50_us={:.3} schedule_p50_us={:.3} \
             serial_share_pct={:.3} worst_stage={} worst_stage_p50_us={:.3} \
             worst_stage_share_pct={:.3} gain_ceiling_pct={:.3} gate_threshold_pct={:.1} \
             verdict={}",
            self.frames,
            self.frame.p50_us(),
            self.schedule.p50_us(),
            self.serial_share_pct,
            worst_stage,
            self.worst_stage
                .as_ref()
                .map_or(0.0, |stage| stage.elapsed.p50_us()),
            self.gate_share_pct(),
            self.worst_stage
                .as_ref()
                .map_or(0.0, |stage| stage.gain_ceiling_pct),
            PARALLELISM_GATE_SHARE_PCT,
            self.verdict()
        )
    }

    /// 台账 JSON（`m74-frame-profile-v1`）。手写而不是引入 serde：本仓基准报告一贯如此，
    /// 字段固定、只需一层对象，引依赖不划算。
    #[must_use]
    pub fn to_json(&self) -> String {
        let stages = self
            .stages
            .iter()
            .map(|stage| {
                format!(
                    "{{\"stage\":\"{}\",\"rank\":{},\"systems\":{},\"elapsed\":{},\
                     \"share_pct\":{:.3},\"critical_path\":{},\"gain_ceiling_pct\":{:.3}}}",
                    stage.stage,
                    stage.stage.rank(),
                    stage.systems,
                    stage.elapsed.json(),
                    stage.share_pct,
                    stage.critical_path.json(),
                    stage.gain_ceiling_pct
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let systems = self
            .systems
            .iter()
            .map(|system| {
                format!(
                    "{{\"name\":\"{}\",\"stage\":\"{}\",\"elapsed\":{},\"share_pct\":{:.3}}}",
                    system.name,
                    system.stage,
                    system.elapsed.json(),
                    system.share_pct
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"report_version\":\"m74-frame-profile-v1\",\"frames\":{},\
             \"frame\":{},\"schedule\":{},\"serial_share_pct\":{:.3},\
             \"gate\":{{\"share_pct\":{:.3},\"threshold_pct\":{:.1},\"verdict\":\"{}\"}},\
             \"worst_stage\":\"{}\",\"stages\":[{}],\"systems\":[{}]}}",
            self.frames,
            self.frame.json(),
            self.schedule.json(),
            self.serial_share_pct,
            self.gate_share_pct(),
            PARALLELISM_GATE_SHARE_PCT,
            self.verdict(),
            self.worst_stage
                .as_ref()
                .map_or_else(|| "none".to_owned(), |stage| stage.stage.to_string()),
            stages,
            systems
        )
    }
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

/// 百分比；分母为 0 时返回 0（还没跑过帧，不是"占比无穷大"）。
fn percent(numerator: f64, denominator: f64) -> f64 {
    if denominator > 0.0 {
        numerator * 100.0 / denominator
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{FrameProfile, PARALLELISM_GATE_SHARE_PCT, TimingStats};
    use crate::host::runtime_scene::FrameScheduleReport;
    use rustlua_ecs::scheduler::{ScheduleStage, SystemTiming};

    fn timing(name: &str, stage: ScheduleStage, micros: u64) -> SystemTiming {
        SystemTiming {
            name: name.to_owned(),
            stage,
            elapsed: Duration::from_micros(micros),
        }
    }

    fn report(total_micros: u64, timings: Vec<SystemTiming>) -> FrameScheduleReport {
        FrameScheduleReport {
            steps: 1,
            total_elapsed: Duration::from_micros(total_micros),
            timings,
        }
    }

    /// 分位数口径：nearest-rank，与 `wp0_scene_frame` 同一条公式。样本 1..=100 时
    /// p50/p95/p99 必须正好是 50/95/99——差一位就说明两处的"p99"不是同一个东西。
    #[test]
    fn percentiles_use_nearest_rank() {
        let samples = (1..=100u64).collect::<Vec<_>>();
        let stats = TimingStats::from_samples(&samples);
        assert_eq!(stats.count, 100);
        assert_eq!(stats.min_ns, 1);
        assert_eq!(stats.max_ns, 100);
        assert_eq!(stats.p50_ns, 50);
        assert_eq!(stats.p95_ns, 95);
        assert_eq!(stats.p99_ns, 99);
        assert_eq!(stats.mean_ns, 50);
        // 空样本不 panic，且计数为 0（调用方据此判断"没有读数"）。
        assert_eq!(TimingStats::from_samples(&[]).count, 0);
    }

    /// stage 合计只加同一 stage 的系统；占帧比的分母是**整帧**（含提取），不是调度总耗时。
    #[test]
    fn stage_totals_and_shares_use_the_whole_frame() {
        let mut profile = FrameProfile::new();
        profile.record(
            &report(
                400,
                vec![
                    timing("lua_update", ScheduleStage::Script, 100),
                    timing("entity_scripts", ScheduleStage::Script, 300),
                    timing("physics2d_step", ScheduleStage::Physics, 200),
                    timing("spatial_propagate", ScheduleStage::Spatial, 50),
                ],
            ),
            Duration::from_micros(1_000),
        );

        let summary = profile.summary();
        assert_eq!(summary.frames, 1);
        assert!((summary.frame.p50_us() - 1000.0).abs() < 1e-9);
        // 调度器合计 400 µs / 帧 1000 µs = 40%。
        assert!((summary.serial_share_pct - 40.0).abs() < 1e-9);
        // 最慢 stage 是 Script（100 + 300 = 400 µs）。
        let worst = summary.worst_stage.expect("有系统就必须有最慢 stage");
        assert_eq!(worst.stage, ScheduleStage::Script);
        assert!((worst.share_pct - 40.0).abs() < 1e-9);
        assert_eq!(worst.systems, 2);
        // 关键路径 = 300 µs，收益上限 = (400 − 300) / 1000 = 10%。
        assert!((worst.critical_path.p50_us() - 300.0).abs() < 1e-9);
        assert!((worst.gain_ceiling_pct - 10.0).abs() < 1e-9);
        // stage 按 rank 升序（Script=1 → Physics=3 → Spatial=7）。
        assert_eq!(
            summary
                .stages
                .iter()
                .map(|stage| stage.stage)
                .collect::<Vec<_>>(),
            vec![
                ScheduleStage::Script,
                ScheduleStage::Physics,
                ScheduleStage::Spatial
            ]
        );
        // 系统按首次执行顺序。
        assert_eq!(
            summary
                .systems
                .iter()
                .map(|system| system.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "lua_update",
                "entity_scripts",
                "physics2d_step",
                "spatial_propagate"
            ]
        );
    }

    /// 闸门边界：严格小于 20% 才判"不做"。恰好 20.0 不算通过——判据原文是"< 20%"。
    #[test]
    fn gate_boundary_is_strictly_below_twenty_percent() {
        let mut below = FrameProfile::new();
        below.record(
            &report(
                199,
                vec![timing("lua_update", ScheduleStage::Script, 199)],
            ),
            Duration::from_micros(1_000),
        );
        let summary = below.summary();
        assert!((summary.gate_share_pct() - 19.9).abs() < 1e-9);
        assert!(summary.passes_gate(), "19.9% < 20% ⇒ 不做");
        assert_eq!(summary.verdict(), "skip");

        let mut exact = FrameProfile::new();
        exact.record(
            &report(
                200,
                vec![timing("lua_update", ScheduleStage::Script, 200)],
            ),
            Duration::from_micros(1_000),
        );
        let summary = exact.summary();
        assert!((summary.gate_share_pct() - 20.0).abs() < 1e-9);
        assert!(!summary.passes_gate(), "恰好 20% 不满足 \"< 20%\"");
        assert_eq!(summary.verdict(), "proceed");
        assert_eq!(PARALLELISM_GATE_SHARE_PCT, 20.0);
    }

    /// 多帧时用分位数而不是单帧：一帧抖动不能改变结论。
    #[test]
    fn multi_frame_summary_uses_percentiles_not_single_samples() {
        let mut profile = FrameProfile::new();
        for frame in 0..5u64 {
            // 一帧 900 µs 的离群值，其余 100 µs。
            let stage_micros = if frame == 2 { 900 } else { 100 };
            profile.record(
                &report(
                    stage_micros,
                    vec![timing(
                        "lua_update",
                        ScheduleStage::Script,
                        stage_micros,
                    )],
                ),
                Duration::from_micros(1_000),
            );
        }
        let summary = profile.summary();
        assert_eq!(summary.frames, 5);
        // 5 个样本的 p50 是第 3 个（下标 2），仍是 100（离群值在 p95 才出现）。
        assert!((summary.gate_share_pct() - 10.0).abs() < 1e-9);
        assert!((summary.worst_stage.expect("stage").elapsed.p99_us() - 900.0).abs() < 1e-9);
    }

    /// 空档：没跑过帧时不算"通过"以外的任何结论，且 JSON 仍合法（不出现 NaN）。
    #[test]
    fn empty_profile_is_well_formed() {
        let summary = FrameProfile::new().summary();
        assert_eq!(summary.frames, 0);
        assert_eq!(summary.gate_share_pct(), 0.0);
        assert!(summary.passes_gate());
        assert!(summary.worst_stage.is_none());
        let json = summary.to_json();
        assert!(json.contains("\"report_version\":\"m74-frame-profile-v1\""));
        assert!(json.contains("\"verdict\":\"skip\""));
        assert!(!json.contains("NaN"));
        assert!(summary.summary_line().contains("worst_stage=none"));
    }

    /// JSON 与单行摘要必须带同一组闸门读数：脚本按单行断言、台账按 JSON 存档，
    /// 两者不一致会让"存档通过、CI 失败"这类矛盾无法解释。
    #[test]
    fn json_and_line_expose_the_same_gate_numbers() {
        let mut profile = FrameProfile::new();
        profile.record(
            &report(
                250,
                vec![
                    timing("lua_update", ScheduleStage::Script, 150),
                    timing("entity_scripts", ScheduleStage::Script, 100),
                ],
            ),
            Duration::from_micros(1_000),
        );
        let summary = profile.summary();
        let json = summary.to_json();
        let line = summary.summary_line();
        for needle in [
            "\"frames\":1",
            "\"share_pct\":25.000",
            "\"threshold_pct\":20.0",
            "\"verdict\":\"proceed\"",
            "\"worst_stage\":\"Script\"",
            "\"name\":\"lua_update\"",
        ] {
            assert!(json.contains(needle), "JSON 缺少 {needle}: {json}");
        }
        for needle in [
            "frames=1",
            "gate_threshold_pct=20.0",
            "verdict=proceed",
            "worst_stage=Script",
        ] {
            assert!(line.contains(needle), "单行摘要缺少 {needle}: {line}");
        }
    }
}
