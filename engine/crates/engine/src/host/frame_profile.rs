//! 帧调度分档统计（M74-02 P0 度量闸门）：把每帧的 `FrameScheduleReport`
//! （[`runtime_scene::FrameScheduleReport`](crate::host::runtime_scene::FrameScheduleReport)）
//! 累积成每系统 p50/p99、每 stage 合计与"最慢 stage 占整帧比"。
//!
//! 为什么要在 `ScheduleReport` 之外再做一层：一次 `run_script_steps` 的读数只描述**一次**
//! 管线运行，而"并行调度值不值得做"问的是稳态分布——首帧、场景切换、GC 都会把单次读数
//! 带偏。这里按帧累积后取分位数，输出直接回答闸门问题：
//! [`FrameProfileSummary::gate_share_pct`]（最慢**可并行** stage 的串行系统合计占整帧多少）。
//!
//! **两种读法**：`Script`（Lua）永远串行，把它算进"并行能吃到多少"会让真实工程读出 76%
//! 的假信号；所以调用方用 [`FrameProfile::set_exclusive_stages`] 标出独占 stage，
//! 闸门读"可并行"口径，全量口径（[`FrameProfileSummary::literal_gate_share_pct`]）留在
//! 报告里作对照。
//!
//! **整帧墙钟由调用方传入**：调度器只知道自己那一段，2D/3D 提取与渲染提交都不在里面。
//! 拿调度器总耗时当分母会把占比算大好几倍，结论就从"要不要并行调度"变成"调度占调度
//! 自己多少"——那不回答任何有意义的问题。
//!
//! **调度器之外也要归因**：调用方用 [`FrameProfile::record_host_phases`] 报模拟/提取/提交三段，
//! 剩下的自动记成 `other`；渲染器自报的 CPU/GPU 帧耗时（设备支持时）一并进样本。
//! 这样"整帧 30 ms、调度器里 7 ms"不再是一句未归因的话，而是四段加 CPU/GPU 的可核对账本。

use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};

use rustlua_ecs::scheduler::ScheduleStage;

use super::runtime_scene::FrameScheduleReport;

/// P0 闸门的硬判据（任务文件 §阶段拆分）：最慢**可并行** stage 的串行系统合计**低于**整帧的这个
/// 百分比 ⇒ 并行调度的收益上限不足两成，结论是**不做**。
///
/// `Script` 等独占 stage 不计入（见 [`FrameProfile::set_exclusive_stages`]）。
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

/// 一帧里调度器**之外**的宿主阶段耗时。`other` 不在这里填——由整帧墙钟减去这三段得到，
/// 这样"还有多少没归因"永远是个显式读数，而不是让三段之和悄悄小于整帧。
#[derive(Clone, Debug, Default)]
pub struct HostPhases {
    /// 固定步推进（`advance_clock` + `run_script_steps`）。
    pub sim: Duration,
    /// 场景提取（2D 顶点、3D 视图、UI overlay、渲染器参数写入）。
    pub extract: Duration,
    /// 渲染提交（`Renderer::render` 到 present 返回）。
    pub submit: Duration,
    /// 渲染器自报的 CPU 帧耗时（`RenderFrameMetrics::cpu_frame_us`）。
    pub render_cpu_us: Option<u64>,
    /// 渲染器自报的 GPU 帧耗时；设备不支持时间戳时为 `None`（该帧不进样本）。
    pub gpu_frame_us: Option<u64>,
    /// `submit` 的四段拆分（P3-R181）：surface acquire。
    ///
    /// 为什么要拆：`submit` 这一段宿主墙钟包住 acquire + 命令录制 + `queue.submit` + present
    /// 四件事，只报一个总数的话「贵在录制还是贵在帧步调」无法区分，下一刀只能靠猜。
    pub render_acquire_us: Option<u64>,
    /// `submit` 的四段拆分：命令录制（`render_passes`，含阴影链与全部后处理）。
    pub render_record_us: Option<u64>,
    /// `submit` 的四段拆分：`queue.submit` 本身。
    pub render_submit_us: Option<u64>,
    /// `submit` 的四段拆分：present。必须单列，否则 vsync/帧步调会被当成录制开销。
    pub render_present_us: Option<u64>,
    /// 录制里的方向光阴影子桶（4 级联 + ESM 全屏）。
    pub shadow_record_us: Option<u64>,
    /// 录制里的点光立方体阴影子桶（≤8 盏 × 6 面，每面重录全部 drawable）。
    pub point_shadow_record_us: Option<u64>,
    /// 渲染器时间戳自报的**整帧 GPU** 耗时（`GpuPassReport::frame_us`）。
    ///
    /// 与 `gpu_frame_us` 同源但独立填：`gpu_frame_us` 是既有口径，这一项与下面的逐 pass
    /// 样本一起构成「同一帧里 pass 之和 vs 帧总数」的对账（P3-R182）。
    pub gpu_pass_frame_us: Option<u64>,
    /// 逐 pass 的 GPU 耗时（`RenderPass::as_str` 短名 → µs），按渲染器给出的顺序。
    ///
    /// 设备不支持时间戳查询时留空 ⇒ 台账里 `gpu_passes` 是空数组而不是 0 µs 的行。
    /// 只在宿主开了帧剖面时才采样，发布路径不付这份分配。
    pub gpu_passes: Vec<(&'static str, u64)>,
    /// P3-R183：本帧 3D 剔除候选数（`RenderFrameMetrics::cull_candidates`）。
    ///
    /// 与 `cull_frustum` 构成「剔掉多少实例」的读数：没有 3D 内容的帧两者皆 0，
    /// 累计桶只统计候选 > 0 的帧，避免纯 2D 帧把剔除率稀释成假低。
    pub cull_candidates: Option<u64>,
    /// P3-R183：本帧被视锥剔掉的实例数；剔除关闭时渲染器恒报 0（A/B 对照档）。
    pub cull_frustum: Option<u64>,
    /// P3-R252：本帧因整批落在距离剔除球外而**没有登记 draw** 的 GPU 位姿批次 draw 数。
    pub gpu_motion_culled: Option<u64>,
    /// P3-R254：上述被跳过批次带的实例总数（补 `cull_candidates` 的口径：候选 − 视锥剔 −
    /// 整批跳过 ≈ 提交实例）。
    pub gpu_motion_culled_instances: Option<u64>,
    /// P3-R247：本帧方向光阴影链实际提交的 draw 次数（跨级联累计）。
    ///
    /// 三者一起才能把"阴影便宜了"拆到 draw 粒度：提交 + 剔除 = drawable × 级联数。
    pub shadow_draws: Option<u64>,
    /// P3-R247：本帧阴影链里被逐级联剔除的 draw 次数（`--no-cull` 不影响这一层）。
    pub shadow_draws_culled: Option<u64>,
    /// P3-R247：本帧阴影链提交的实例数（跨级联累计）——逐实例剔除/LOD 的分母。
    pub shadow_instances: Option<u64>,
    /// P3-R248：本帧被阴影投射排除档挡在 CSM 链外的 draw 次数（跨级联累计）。
    /// 与剔除分开记：剔除只省不丢，排除档会丢阴影（观感取舍）。
    pub shadow_draws_skipped: Option<u64>,
    /// P3-R249：本帧被同一分类排除档挡在点光 6 面链外的 draw 次数（跨灯 × 面累计）。
    /// 与方向光的 `shadow_draws_skipped` 分开：两条链的尝试分母不同。
    pub point_shadow_draws_skipped: Option<u64>,
    /// P3-R256：本帧因超出阴影投射距离预算而没进 CSM 链的 draw 次数（跨级联累计）。
    /// 与 `shadow_draws_skipped` 分开记：一个是按名字分类、一个是按距离，混一列就没法
    /// 把 A/B 的收益归到具体那把刀上。
    pub shadow_draws_distance_skipped: Option<u64>,
    /// P3-R256：点光 6 面链被距离预算挡掉的 draw 次数（跨灯 × 面累计）。
    pub point_shadow_draws_distance_skipped: Option<u64>,
    /// P3-R261：点光 6 面链**实际提交**的 draw 次数（跨灯 × 面累计）。
    ///
    /// `shadow_draws` 是方向光 CSM-only 口径。P3-R260 实测「预算把 CSM 实例砍 97.7 %、
    /// shadow pass 只掉 4.5 %」，就是因为点光链的 crowd 提交一直在跑、却没有任何一列在数。
    pub point_shadow_draws: Option<u64>,
    /// P3-R261：同口径的点光链提交实例数（跨灯 × 面累计）。
    pub point_shadow_instances: Option<u64>,
    /// P3-R262：点光 6 面链里因面视锥不含包围盒被剔除的 draw 次数（跨灯 × 面累计）。
    ///
    /// 与方向光的 `shadow_draws_culled` 分开：点光链的分母是 `drawable × 灯 × 面`。
    /// 有了这一列，点光链的守恒式才能在台账上闭合：
    /// `提交 + range 剔除 + 面剔除 + 分类排除 + 距离预算 = drawable × 灯 × 面`。
    pub point_shadow_face_draws_culled: Option<u64>,
    /// P3-R262：点光 6 面链里因包围盒到灯距离超出 range 被剔的 draw 次数
    /// （跨灯 × 面累计；对应 `RenderFrameMetrics::point_shadow_draws_culled`）。
    pub point_shadow_range_culled: Option<u64>,
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
    /// 判为"不可并行"的 stage（P0 默认空；例子把 `Script` 标进来，见 [`Self::set_exclusive_stages`]）。
    exclusive_stages: Vec<ScheduleStage>,
    /// 报告的自描述键值对（P3-R205）：宿主生效后的渲染档位等，写进 JSON 的 `environment`。
    /// 引擎不猜宿主配置，空 = 调用方没填。
    environment: Vec<(String, String)>,
    /// 每帧该 stage 内**非独占**系统耗时之和——闸门读数用它，`stage_ns` 留给"全量"对照。
    stage_parallel_ns: BTreeMap<ScheduleStage, Vec<u64>>,
    /// 每帧该 stage 内非独占系统里最慢那一个。
    stage_parallel_max_ns: BTreeMap<ScheduleStage, Vec<u64>>,
    /// 宿主阶段（模拟/提取/提交/其它），见 [`HostPhases`]。
    host_sim_ns: Vec<u64>,
    host_extract_ns: Vec<u64>,
    host_submit_ns: Vec<u64>,
    host_other_ns: Vec<u64>,
    host_cpu_ns: Vec<u64>,
    host_gpu_ns: Vec<u64>,
    /// `submit` 的四段拆分与两个阴影子桶（P3-R181），单位同 `host_cpu_ns`。
    host_acquire_ns: Vec<u64>,
    host_record_ns: Vec<u64>,
    host_queue_submit_ns: Vec<u64>,
    host_present_ns: Vec<u64>,
    host_shadow_record_ns: Vec<u64>,
    host_point_shadow_record_ns: Vec<u64>,
    /// 渲染器时间戳自报的整帧 GPU 耗时样本（P3-R182）。
    gpu_pass_frame_ns: Vec<u64>,
    /// 逐 pass GPU 样本的**首见顺序**（与 `system_order` 同一条纪律：报告不随 HashMap 迭代抖）。
    gpu_pass_order: Vec<&'static str>,
    gpu_pass_ns: HashMap<&'static str, Vec<u64>>,
    /// P3-R183：剔除累计（只统计候选 > 0 的帧）。计数不是时间，不做分档，只出总和与比率。
    cull_frames: u64,
    cull_candidates_total: u64,
    cull_frustum_total: u64,
    /// P3-R252：整批跳过 draw 的累计（剔除档关时恒 0）。
    gpu_motion_culled_total: u64,
    /// P3-R254：整批跳过带走的实例累计（与上面的 draw 条数同帧累计）。
    gpu_motion_culled_instances_total: u64,
    /// P3-R247：方向光阴影的提交/剔除累计（只统计有阴影 draw 的帧）。
    shadow_frames: u64,
    shadow_draws_total: u64,
    shadow_draws_culled_total: u64,
    shadow_instances_total: u64,
    /// P3-R248：被阴影投射排除档挡掉的 draw 累计（跨级联）。
    shadow_draws_skipped_total: u64,
    /// P3-R249：点光链被分类排除档挡掉的 draw 累计（跨灯 × 面）。
    point_shadow_draws_skipped_total: u64,
    /// P3-R256：被阴影投射距离预算挡掉的 draw 累计（方向光与点光各一桶）。
    shadow_draws_distance_skipped_total: u64,
    point_shadow_draws_distance_skipped_total: u64,
    /// P3-R261：点光链提交累计（与方向光分账，见 `HostPhases::point_shadow_draws`）。
    point_shadow_draws_total: u64,
    point_shadow_instances_total: u64,
    /// P3-R262：点光链面剔除/range 剔除累计。
    point_shadow_face_draws_culled_total: u64,
    point_shadow_range_culled_total: u64,
}

impl FrameProfile {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 把某些 stage 标为"独占、不可并行"。
    ///
    /// P0 只有一项：`Script`。任务书 §风险 与 §明确不做 都写明 Lua 状态是单线程的，
    /// **Script stage 永远串行**——把它的耗时算进"并行调度能吃到多少"，会得出"该做"的
    /// 假结论（真实工程里它恰好是最大的一块）。其余 stage 的系统都只碰 `World`。
    ///
    /// 必须在 [`Self::record`] 之前调用；调用方在预热阶段已知系统名单，直接按 stage 标。
    pub fn set_exclusive_stages(&mut self, stages: impl IntoIterator<Item = ScheduleStage>) {
        self.exclusive_stages = stages.into_iter().collect();
    }

    /// 写入报告的自描述键值对（P3-R205）：宿主把**生效后**的渲染档位放进来，
    /// 台账 JSON 才不会与档位脱钩（数字被引去别的配置下比较）。
    ///
    /// 引擎只做存储与转义，不解释键名；必须在 [`Self::summary`] 之前调用。
    pub fn set_environment<K, V>(&mut self, entries: impl IntoIterator<Item = (K, V)>)
    where
        K: Into<String>,
        V: Into<String>,
    {
        self.environment = entries
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
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
        let mut per_stage_parallel_sum: BTreeMap<ScheduleStage, u64> = BTreeMap::new();
        let mut per_stage_parallel_max: BTreeMap<ScheduleStage, u64> = BTreeMap::new();
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
            if !self.exclusive_stages.contains(&timing.stage) {
                *per_stage_parallel_sum.entry(timing.stage).or_default() += elapsed;
                let parallel_max = per_stage_parallel_max.entry(timing.stage).or_default();
                *parallel_max = (*parallel_max).max(elapsed);
            }
        }
        for (stage, elapsed) in per_stage_sum {
            self.stage_ns.entry(stage).or_default().push(elapsed);
        }
        for (stage, elapsed) in per_stage_max {
            self.stage_max_ns.entry(stage).or_default().push(elapsed);
        }
        for (stage, elapsed) in per_stage_parallel_sum {
            self.stage_parallel_ns
                .entry(stage)
                .or_default()
                .push(elapsed);
        }
        for (stage, elapsed) in per_stage_parallel_max {
            self.stage_parallel_max_ns
                .entry(stage)
                .or_default()
                .push(elapsed);
        }
        // 没有系统的 stage 不落样本：报告只为**真正执行过**的系统记时间，
        // 空 stage 不该在统计里以 0 出现、把"最慢 stage"的排序搅乱。
    }

    /// 记录本帧调度器之外的宿主阶段。与 [`Self::record`] 一一对应（每帧各一次）；
    /// `other` 用**已记录的那一帧**墙钟减去三段，因此必须在同帧的 `record` 之后调用。
    pub fn record_host_phases(&mut self, phases: HostPhases) {
        let sim = nanos(phases.sim);
        let extract = nanos(phases.extract);
        let submit = nanos(phases.submit);
        let frame = self.frame_ns.last().copied().unwrap_or(0);
        self.host_sim_ns.push(sim);
        self.host_extract_ns.push(extract);
        self.host_submit_ns.push(submit);
        self.host_other_ns
            .push(frame.saturating_sub(sim.saturating_add(extract).saturating_add(submit)));
        // 渲染器自报的是微秒；样本统一按纳秒存，`TimingStats::p50_us` 才是同一个单位。
        if let Some(cpu) = phases.render_cpu_us {
            self.host_cpu_ns.push(cpu.saturating_mul(1_000));
        }
        if let Some(gpu) = phases.gpu_frame_us {
            self.host_gpu_ns.push(gpu.saturating_mul(1_000));
        }
        for (value, samples) in [
            (phases.render_acquire_us, &mut self.host_acquire_ns),
            (phases.render_record_us, &mut self.host_record_ns),
            (phases.render_submit_us, &mut self.host_queue_submit_ns),
            (phases.render_present_us, &mut self.host_present_ns),
            (phases.shadow_record_us, &mut self.host_shadow_record_ns),
            (
                phases.point_shadow_record_us,
                &mut self.host_point_shadow_record_ns,
            ),
        ] {
            if let Some(us) = value {
                samples.push(us.saturating_mul(1_000));
            }
        }
        if let Some(frame_us) = phases.gpu_pass_frame_us {
            self.gpu_pass_frame_ns.push(frame_us.saturating_mul(1_000));
        }
        for (name, us) in phases.gpu_passes {
            if !self.gpu_pass_ns.contains_key(name) {
                self.gpu_pass_order.push(name);
            }
            self.gpu_pass_ns
                .entry(name)
                .or_default()
                .push(us.saturating_mul(1_000));
        }
        // P3-R183：剔除累计。候选为 0 的帧（纯 2D / 旧调用方没填）不进桶——
        // 否则「剔除率」会被没有 3D 内容的帧稀释，A/B 两档都失真。
        if let (Some(candidates), Some(frustum)) = (phases.cull_candidates, phases.cull_frustum)
            && candidates > 0
        {
            self.cull_frames += 1;
            self.cull_candidates_total = self.cull_candidates_total.saturating_add(candidates);
            self.cull_frustum_total = self.cull_frustum_total.saturating_add(frustum);
        }
        // P3-R252：整批跳过不依赖候选分母（跳过发生在剔除计数之后），单独累计。
        if let Some(culled) = phases.gpu_motion_culled {
            self.gpu_motion_culled_total = self.gpu_motion_culled_total.saturating_add(culled);
        }
        if let Some(culled) = phases.gpu_motion_culled_instances {
            self.gpu_motion_culled_instances_total = self
                .gpu_motion_culled_instances_total
                .saturating_add(culled);
        }
        // P3-R261：点光链的提交与方向光无关（没有方向光的帧也可能有立方体阴影），
        // 因此不进上面「有 CSM draw 的帧」那个桶，按帧独立累计。
        if let Some(draws) = phases.point_shadow_draws {
            self.point_shadow_draws_total = self.point_shadow_draws_total.saturating_add(draws);
        }
        if let Some(instances) = phases.point_shadow_instances {
            self.point_shadow_instances_total =
                self.point_shadow_instances_total.saturating_add(instances);
        }
        // P3-R262：点光链的五项计数必须同帧累计，否则「提交 + range + 面剔除 + 分类排除
        // + 距离预算 = drawable × 灯 × 面」在无方向光的帧上会缺项。P3-R249/R256 时这两项
        // 借 CSM 桶累计是权宜，现在和提交一起无条件记。
        if let Some(culled) = phases.point_shadow_range_culled {
            self.point_shadow_range_culled_total =
                self.point_shadow_range_culled_total.saturating_add(culled);
        }
        if let Some(culled) = phases.point_shadow_face_draws_culled {
            self.point_shadow_face_draws_culled_total = self
                .point_shadow_face_draws_culled_total
                .saturating_add(culled);
        }
        self.point_shadow_draws_skipped_total = self
            .point_shadow_draws_skipped_total
            .saturating_add(phases.point_shadow_draws_skipped.unwrap_or(0));
        self.point_shadow_draws_distance_skipped_total = self
            .point_shadow_draws_distance_skipped_total
            .saturating_add(phases.point_shadow_draws_distance_skipped.unwrap_or(0));
        // P3-R247：阴影提交/剔除累计。只统计真有阴影工作的帧——没有方向光或纯 2D 帧
        // 各项皆 0，把它们算进分母会让"被剔占比"变成假低。P3-R248 的排除档也算"有工作"
        // （全被排除时提交为 0，但那不是"没有方向光"）。
        if let (Some(draws), Some(culled), Some(instances)) = (
            phases.shadow_draws,
            phases.shadow_draws_culled,
            phases.shadow_instances,
        ) {
            let skipped = phases.shadow_draws_skipped.unwrap_or(0);
            if draws + skipped > 0 {
                self.shadow_frames += 1;
                self.shadow_draws_total = self.shadow_draws_total.saturating_add(draws);
                self.shadow_draws_culled_total =
                    self.shadow_draws_culled_total.saturating_add(culled);
                self.shadow_instances_total = self.shadow_instances_total.saturating_add(instances);
                self.shadow_draws_skipped_total =
                    self.shadow_draws_skipped_total.saturating_add(skipped);
                self.shadow_draws_distance_skipped_total = self
                    .shadow_draws_distance_skipped_total
                    .saturating_add(phases.shadow_draws_distance_skipped.unwrap_or(0));
            }
        }
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
            let parallel_elapsed = TimingStats::from_samples(
                self.stage_parallel_ns
                    .get(stage)
                    .map_or(&[][..], Vec::as_slice),
            );
            let parallel_critical_path = TimingStats::from_samples(
                self.stage_parallel_max_ns
                    .get(stage)
                    .map_or(&[][..], Vec::as_slice),
            );
            let parallel_gain_ns = parallel_elapsed
                .p50_ns
                .saturating_sub(parallel_critical_path.p50_ns);
            stages.push(StageProfile {
                stage: *stage,
                systems: systems_in_stage,
                elapsed,
                share_pct: percent(elapsed.p50_ns as f64, frame_p50),
                critical_path,
                gain_ceiling_pct: percent(gain_ns as f64, frame_p50),
                parallel_elapsed,
                parallel_share_pct: percent(parallel_elapsed.p50_ns as f64, frame_p50),
                parallel_critical_path,
                parallel_gain_ceiling_pct: percent(parallel_gain_ns as f64, frame_p50),
            });
        }
        let worst_stage = pick_worst(&stages, |stage| stage.share_pct);
        let worst_parallel_stage = pick_worst(&stages, |stage| stage.parallel_share_pct);
        let host_of = |samples: &[u64]| {
            let stats = TimingStats::from_samples(samples);
            let share = percent(stats.p50_ns as f64, frame_p50);
            (stats, share)
        };
        let (sim, sim_share_pct) = host_of(&self.host_sim_ns);
        let (extract, extract_share_pct) = host_of(&self.host_extract_ns);
        let (submit, submit_share_pct) = host_of(&self.host_submit_ns);
        let (other, other_share_pct) = host_of(&self.host_other_ns);
        let (render_cpu, _) = host_of(&self.host_cpu_ns);
        let (gpu_frame, _) = host_of(&self.host_gpu_ns);
        let (render_acquire, _) = host_of(&self.host_acquire_ns);
        let (render_record, _) = host_of(&self.host_record_ns);
        let (render_submit, _) = host_of(&self.host_queue_submit_ns);
        let (render_present, _) = host_of(&self.host_present_ns);
        let (shadow_record, _) = host_of(&self.host_shadow_record_ns);
        let (point_shadow_record, _) = host_of(&self.host_point_shadow_record_ns);
        let (gpu_pass_frame, _) = host_of(&self.gpu_pass_frame_ns);
        let mut gpu_passes = Vec::with_capacity(self.gpu_pass_order.len());
        for &name in &self.gpu_pass_order {
            let Some(samples) = self.gpu_pass_ns.get(name) else {
                continue;
            };
            let elapsed = TimingStats::from_samples(samples);
            gpu_passes.push(GpuPassProfile {
                name,
                elapsed,
                share_pct: percent(elapsed.p50_ns as f64, frame_p50),
            });
        }

        FrameProfileSummary {
            frames: self.frames,
            environment: self.environment.clone(),
            frame,
            schedule,
            serial_share_pct: percent(schedule.p50_ns as f64, frame_p50),
            stages,
            systems,
            worst_stage,
            worst_parallel_stage,
            host: HostPhaseSummary {
                sim,
                extract,
                submit,
                other,
                sim_share_pct,
                extract_share_pct,
                submit_share_pct,
                other_share_pct,
                render_cpu,
                gpu_frame,
                render_acquire,
                render_record,
                render_submit,
                render_present,
                shadow_record,
                point_shadow_record,
                gpu_pass_frame,
            },
            gpu_passes,
            cull: CullSummary {
                frames: self.cull_frames,
                candidates: self.cull_candidates_total,
                frustum_culled: self.cull_frustum_total,
                gpu_motion_skipped: self.gpu_motion_culled_total,
                gpu_motion_skipped_instances: self.gpu_motion_culled_instances_total,
                culled_pct: percent(
                    self.cull_frustum_total as f64,
                    self.cull_candidates_total as f64,
                ),
            },
            shadow: ShadowSummary {
                frames: self.shadow_frames,
                draws: self.shadow_draws_total,
                draws_culled: self.shadow_draws_culled_total,
                instances: self.shadow_instances_total,
                skipped: self.shadow_draws_skipped_total,
                point_skipped: self.point_shadow_draws_skipped_total,
                distance_skipped: self.shadow_draws_distance_skipped_total,
                point_distance_skipped: self.point_shadow_draws_distance_skipped_total,
                point_draws: self.point_shadow_draws_total,
                point_instances: self.point_shadow_instances_total,
                point_range_culled: self.point_shadow_range_culled_total,
                point_face_culled: self.point_shadow_face_draws_culled_total,
                draws_culled_pct: percent(
                    self.shadow_draws_culled_total as f64,
                    (self.shadow_draws_total + self.shadow_draws_culled_total) as f64,
                ),
            },
        }
    }
}

/// 最小 JSON 字符串转义（P3-R205）：`environment` 的值来自宿主配置（档位、shader 目录），
/// 路径里的 `"`/`\` 或换行会把整份台账 JSON 弄坏，这里的转义是写入前的唯一防线。
fn json_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out
}

/// 取百分比最大的 stage；相等时按 rank 升序稳定取第一个。
///
/// 只比较读数**大于 0** 的项：没有系统、或（对可并行口径而言）全部系统都被标为独占的
/// stage，不该以 0% 参与"最慢"的评选——那会把"没有可并行工作"误报成"它就是最慢的"。
fn pick_worst(
    stages: &[StageProfile],
    metric: impl Fn(&StageProfile) -> f64,
) -> Option<StageProfile> {
    stages
        .iter()
        .filter(|stage| metric(stage) > 0.0)
        .max_by(|left, right| {
            metric(left)
                .total_cmp(&metric(right))
                .then_with(|| right.stage.rank().cmp(&left.stage.rank()))
        })
        .cloned()
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
///
/// 两组读数并存：`elapsed`/`share_pct` 是**全量**（含独占系统），`parallel_elapsed`/
/// `parallel_share_pct` 是去掉独占系统后的**可并行**口径。闸门判的是后者——任务书明写
/// Script stage 永远串行，把它算进"并行能吃到多少"会得出"该做"的假结论。
#[derive(Clone, Debug, PartialEq)]
pub struct StageProfile {
    pub stage: ScheduleStage,
    /// 该 stage 里登记过的系统数。
    pub systems: usize,
    /// 每帧该 stage 内所有系统耗时**之和**的分布。
    pub elapsed: TimingStats,
    /// stage 合计（p50）占整帧 p50 的百分比。
    pub share_pct: f64,
    /// 该 stage 内最慢单个系统的分布（并行后的关键路径）。
    pub critical_path: TimingStats,
    /// 该 stage 并行化的收益上限占帧比：`(stage 合计 − 关键路径) / 帧`。
    pub gain_ceiling_pct: f64,
    /// 去掉独占系统后，每帧该 stage 内可并行系统耗时之和的分布。
    pub parallel_elapsed: TimingStats,
    /// 可并行合计（p50）占整帧 p50 的百分比——P0 闸门读的就是它的最大值。
    pub parallel_share_pct: f64,
    /// 可并行系统里最慢那一个的分布。
    pub parallel_critical_path: TimingStats,
    /// 可并行部分的收益上限占帧比：`(可并行合计 − 其关键路径) / 帧`。
    pub parallel_gain_ceiling_pct: f64,
}

/// 一份汇总报告。
#[derive(Clone, Debug, PartialEq)]
pub struct FrameProfileSummary {
    pub frames: u64,
    /// 宿主填的自描述键值对（P3-R205，保持写入顺序）；空 = 调用方没填。
    pub environment: Vec<(String, String)>,
    pub frame: TimingStats,
    pub schedule: TimingStats,
    /// 全部调度系统之和（p50）占整帧 p50 的百分比。
    pub serial_share_pct: f64,
    /// 按 stage rank 升序。
    pub stages: Vec<StageProfile>,
    /// 按首次执行顺序。
    pub systems: Vec<SystemProfile>,
    /// 占帧比最高的 stage（全量口径，含独占；没有系统时为 `None`）。
    pub worst_stage: Option<StageProfile>,
    /// 占帧比最高的**可并行** stage（闸门判据；没有可并行系统时为 `None`）。
    pub worst_parallel_stage: Option<StageProfile>,
    /// 调度器之外的宿主阶段（模拟/提取/提交/其它 + 渲染器自报的 CPU/GPU）。
    pub host: HostPhaseSummary,
    /// 逐 pass 的 GPU 耗时（P3-R182），按首见顺序；设备不支持时间戳时为空。
    pub gpu_passes: Vec<GpuPassProfile>,
    /// 3D 视锥剔除的累计读数（P3-R183）；没有候选 > 0 的帧时 `frames = 0`。
    pub cull: CullSummary,
    /// 方向光阴影的提交/剔除累计（P3-R247）；没有阴影 draw 的帧时 `frames = 0`。
    pub shadow: ShadowSummary,
}

/// 方向光阴影链的累计计数（P3-R247）。
///
/// **为什么是计数而不是时间**：`shadow` pass 的 GPU 时间随场景/驱动波动，而 draw 与实例数
/// 是确定量。批次被拆成多条 draw 后，"阴影便宜了"只有靠「提交 + 剔除 = drawable × 级联」
/// 这条恒等式才能落到某一条具体原因上。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShadowSummary {
    /// 有阴影 draw 的帧数。
    pub frames: u64,
    /// 实际提交的 draw 次数（跨级联累计）。
    pub draws: u64,
    /// 因落在级联裁剪体外被剔除的 draw 次数（跨级联累计）。
    pub draws_culled: u64,
    /// 实际提交的实例数（跨级联累计）。
    pub instances: u64,
    /// 被阴影投射排除档挡掉的 draw 次数（P3-R248；跨级联累计）。这不是剔除：
    /// 剔除只省不丢，排除档会丢阴影，所以单独一列、不进 `draws_culled_pct` 的分子。
    pub skipped: u64,
    /// 被同一分类排除档挡在点光 6 面链外的 draw 次数（P3-R249；跨灯 × 面累计）。
    /// 与 `skipped` 分开统计：点光链的尝试分母是 `灯 × 面`，不能和方向光的
    /// `提交 + 剔除` 守恒式混用。
    pub point_skipped: u64,
    /// 被阴影投射距离预算挡掉的 draw 次数（P3-R256；跨级联累计）。同样是观感取舍，
    /// 不进 `draws_culled_pct`：预算档打开后 `提交 + 剔除 + 分类排除 + 距离预算`
    /// 才等于 `drawable × 级联`。
    pub distance_skipped: u64,
    /// 点光链被距离预算挡掉的 draw 次数（P3-R256；跨灯 × 面累计）。
    pub point_distance_skipped: u64,
    /// P3-R261：点光链实际提交的 draw 次数（跨灯 × 面累计）。
    /// 方向光的守恒式是 `drawable × 级联`，点光的是 `drawable × 灯 × 面`，不能混算。
    pub point_draws: u64,
    /// P3-R261：同口径的点光链提交实例数。
    pub point_instances: u64,
    /// P3-R262：点光链因包围盒超出灯 range 被剔的 draw 次数（跨灯 × 面累计）。
    pub point_range_culled: u64,
    /// P3-R262：点光链因面视锥不含包围盒被剔的 draw 次数（跨灯 × 面累计）。
    /// 与 `point_range_culled` 分开：一个是灯的距离、一个是该面的视锥，
    /// 调 `--no-cull` 只影响后者。
    pub point_face_culled: u64,
    /// 被剔 draw 占尝试次数（提交 + 剔除）的百分比；分母为 0 时为 0。
    pub draws_culled_pct: f64,
}

/// 3D 视锥剔除累计（P3-R183）：「剔掉多少实例」的门读数。
///
/// 只统计候选 > 0 的帧；`culled_pct` 的分母是**候选总数**而不是帧数——
/// 实例数逐帧波动时，按帧平均会把大场景与小场景等权，口径失真。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CullSummary {
    /// 有 3D 剔除候选的帧数。
    pub frames: u64,
    /// 候选实例总数（drawable×primitive，进入剔除判定）。
    pub candidates: u64,
    /// 被视锥剔掉的实例总数。
    pub frustum_culled: u64,
    /// P3-R252：整批落在距离剔除球外而从 draw 列表拿掉的批次 draw 数（累计）。
    pub gpu_motion_skipped: u64,
    /// P3-R254：这些被跳过批次带的实例总数（累计）。
    pub gpu_motion_skipped_instances: u64,
    /// 剔除率（%）；`candidates = 0` 时为 0。
    pub culled_pct: f64,
}

/// 宿主阶段汇总。`other` = 整帧 − 模拟 − 提取 − 提交，饱和到 0；
/// 所有 `count` 为 0 表示调用方没有记录宿主阶段（旧调用方不受影响）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HostPhaseSummary {
    pub sim: TimingStats,
    pub extract: TimingStats,
    pub submit: TimingStats,
    pub other: TimingStats,
    /// 各段 p50 占整帧 p50 的百分比。
    pub sim_share_pct: f64,
    pub extract_share_pct: f64,
    pub submit_share_pct: f64,
    pub other_share_pct: f64,
    /// 渲染器自报的 CPU 帧耗时（微秒样本）。
    pub render_cpu: TimingStats,
    /// 渲染器自报的 GPU 帧耗时（微秒样本）；设备不支持时 `count = 0`。
    pub gpu_frame: TimingStats,
    /// `submit` 的四段拆分（P3-R181）：surface acquire。调用方没填时 `count = 0`。
    pub render_acquire: TimingStats,
    /// `submit` 的四段拆分：命令录制。四段之和与 `submit` 的差就是宿主自己那层包装。
    pub render_record: TimingStats,
    /// `submit` 的四段拆分：`queue.submit`。
    pub render_submit: TimingStats,
    /// `submit` 的四段拆分：present（vsync/帧步调都在这里，不在 `render_cpu` 里）。
    pub render_present: TimingStats,
    /// 录制里的方向光阴影子桶（级联 + ESM）。
    pub shadow_record: TimingStats,
    /// 录制里的点光立方体阴影子桶。
    pub point_shadow_record: TimingStats,
    /// 渲染器时间戳自报的整帧 GPU 耗时；与 `gpu_passes` 各项之和的对账见 [`GpuPassProfile`]。
    pub gpu_pass_frame: TimingStats,
}

/// 单个渲染 pass 的 GPU 耗时分档（P3-R182）。
///
/// 为什么要有 `share_pct`：逐 pass 的绝对微秒在不同分辨率下没有可比性，占整帧的百分比才是
/// 「砍这一刀能拿回多少帧」的口径。
#[derive(Clone, Debug, PartialEq)]
pub struct GpuPassProfile {
    /// `RenderPass::as_str` 的稳定短名。
    pub name: &'static str,
    pub elapsed: TimingStats,
    /// 该 pass 的 p50 占整帧 p50 的百分比。
    pub share_pct: f64,
}

impl FrameProfileSummary {
    /// 闸门读数：最慢**可并行** stage 的串行系统合计占整帧的百分比。没有可并行系统时为 0。
    #[must_use]
    pub fn gate_share_pct(&self) -> f64 {
        self.worst_parallel_stage
            .as_ref()
            .map_or(0.0, |stage| stage.parallel_share_pct)
    }

    /// 同样的读数但**不去掉独占系统**（对照用，不参与判定）。真实工程里它会被 Lua 占满，
    /// 而 Lua 状态是单线程的——这正是不能拿它当判据的原因。
    #[must_use]
    pub fn literal_gate_share_pct(&self) -> f64 {
        self.worst_stage
            .as_ref()
            .map_or(0.0, |stage| stage.share_pct)
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
        if self.passes_gate() {
            "skip"
        } else {
            "proceed"
        }
    }

    /// 单行摘要（给 stdout / CI 断言读）。
    ///
    /// 同时给出可并行口径（判据）与全量口径（对照），并把两者都写进 JSON——只报一个数
    /// 会让下游没法复核"为什么 73% 的帧却判不做"。
    #[must_use]
    pub fn summary_line(&self) -> String {
        let worst_stage = self
            .worst_stage
            .as_ref()
            .map_or_else(|| "none".to_owned(), |stage| stage.stage.to_string());
        let worst_parallel_stage = self
            .worst_parallel_stage
            .as_ref()
            .map_or_else(|| "none".to_owned(), |stage| stage.stage.to_string());
        format!(
            "m74_frame_profile frames={} frame_p50_us={:.3} schedule_p50_us={:.3} \
             serial_share_pct={:.3} worst_stage={} worst_stage_p50_us={:.3} \
             worst_stage_share_pct={:.3} worst_parallel_stage={} parallel_p50_us={:.3} \
             gate_share_pct={:.3} parallel_gain_ceiling_pct={:.3} \
             gate_threshold_pct={:.1} verdict={}",
            self.frames,
            self.frame.p50_us(),
            self.schedule.p50_us(),
            self.serial_share_pct,
            worst_stage,
            self.worst_stage
                .as_ref()
                .map_or(0.0, |stage| stage.elapsed.p50_us()),
            self.literal_gate_share_pct(),
            worst_parallel_stage,
            self.worst_parallel_stage
                .as_ref()
                .map_or(0.0, |stage| stage.parallel_elapsed.p50_us()),
            self.gate_share_pct(),
            self.worst_parallel_stage
                .as_ref()
                .map_or(0.0, |stage| stage.parallel_gain_ceiling_pct),
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
                     \"share_pct\":{:.3},\"critical_path\":{},\"gain_ceiling_pct\":{:.3},\
                     \"parallel_elapsed\":{},\"parallel_share_pct\":{:.3},\
                     \"parallel_critical_path\":{},\"parallel_gain_ceiling_pct\":{:.3}}}",
                    stage.stage,
                    stage.stage.rank(),
                    stage.systems,
                    stage.elapsed.json(),
                    stage.share_pct,
                    stage.critical_path.json(),
                    stage.gain_ceiling_pct,
                    stage.parallel_elapsed.json(),
                    stage.parallel_share_pct,
                    stage.parallel_critical_path.json(),
                    stage.parallel_gain_ceiling_pct
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
        let host = format!(
            "{{\"sim\":{},\"sim_share_pct\":{:.3},\"extract\":{},\"extract_share_pct\":{:.3},\
             \"submit\":{},\"submit_share_pct\":{:.3},\"other\":{},\"other_share_pct\":{:.3},\
             \"render_cpu\":{},\"gpu_frame\":{},\
             \"render_acquire\":{},\"render_record\":{},\"render_submit\":{},\
             \"render_present\":{},\"shadow_record\":{},\"point_shadow_record\":{},\
             \"gpu_pass_frame\":{}}}",
            self.host.sim.json(),
            self.host.sim_share_pct,
            self.host.extract.json(),
            self.host.extract_share_pct,
            self.host.submit.json(),
            self.host.submit_share_pct,
            self.host.other.json(),
            self.host.other_share_pct,
            self.host.render_cpu.json(),
            self.host.gpu_frame.json(),
            self.host.render_acquire.json(),
            self.host.render_record.json(),
            self.host.render_submit.json(),
            self.host.render_present.json(),
            self.host.shadow_record.json(),
            self.host.point_shadow_record.json(),
            self.host.gpu_pass_frame.json()
        );
        let gpu_passes = self
            .gpu_passes
            .iter()
            .map(|pass| {
                format!(
                    "{{\"name\":\"{}\",\"elapsed\":{},\"share_pct\":{:.3}}}",
                    pass.name,
                    pass.elapsed.json(),
                    pass.share_pct
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let environment = self
            .environment
            .iter()
            .map(|(key, value)| format!("\"{}\":\"{}\"", json_escape(key), json_escape(value)))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"report_version\":\"m74-frame-profile-v1\",\"frames\":{},\
             \"environment\":{{{}}},\"frame\":{},\"schedule\":{},\"serial_share_pct\":{:.3},\
             \"gate\":{{\"share_pct\":{:.3},\"literal_share_pct\":{:.3},\
             \"threshold_pct\":{:.1},\"verdict\":\"{}\"}},\
             \"worst_stage\":\"{}\",\"worst_parallel_stage\":\"{}\",\
             \"host\":{},\"gpu_passes\":[{}],\
             \"cull\":{{\"frames\":{},\"candidates\":{},\"frustum_culled\":{},\
             \"gpu_motion_skipped\":{},\"gpu_motion_skipped_instances\":{},\"culled_pct\":{:.3}}},\
             \"shadow\":{{\"frames\":{},\"draws\":{},\"draws_culled\":{},\"instances\":{},\
             \"skipped\":{},\"point_skipped\":{},\"distance_skipped\":{},\
             \"point_distance_skipped\":{},\"point_draws\":{},\"point_instances\":{},\
             \"point_range_culled\":{},\"point_face_culled\":{},\
             \"draws_culled_pct\":{:.3}}},\
             \"stages\":[{}],\"systems\":[{}]}}",
            self.frames,
            environment,
            self.frame.json(),
            self.schedule.json(),
            self.serial_share_pct,
            self.gate_share_pct(),
            self.literal_gate_share_pct(),
            PARALLELISM_GATE_SHARE_PCT,
            self.verdict(),
            self.worst_stage
                .as_ref()
                .map_or_else(|| "none".to_owned(), |stage| stage.stage.to_string()),
            self.worst_parallel_stage
                .as_ref()
                .map_or_else(|| "none".to_owned(), |stage| stage.stage.to_string()),
            host,
            gpu_passes,
            self.cull.frames,
            self.cull.candidates,
            self.cull.frustum_culled,
            self.cull.gpu_motion_skipped,
            self.cull.gpu_motion_skipped_instances,
            self.cull.culled_pct,
            self.shadow.frames,
            self.shadow.draws,
            self.shadow.draws_culled,
            self.shadow.instances,
            self.shadow.skipped,
            self.shadow.point_skipped,
            self.shadow.distance_skipped,
            self.shadow.point_distance_skipped,
            self.shadow.point_draws,
            self.shadow.point_instances,
            self.shadow.point_range_culled,
            self.shadow.point_face_culled,
            self.shadow.draws_culled_pct,
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

    use super::{FrameProfile, HostPhases, PARALLELISM_GATE_SHARE_PCT, TimingStats};
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
            &report(199, vec![timing("lua_update", ScheduleStage::Script, 199)]),
            Duration::from_micros(1_000),
        );
        let summary = below.summary();
        assert!((summary.gate_share_pct() - 19.9).abs() < 1e-9);
        assert!(summary.passes_gate(), "19.9% < 20% ⇒ 不做");
        assert_eq!(summary.verdict(), "skip");

        let mut exact = FrameProfile::new();
        exact.record(
            &report(200, vec![timing("lua_update", ScheduleStage::Script, 200)]),
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
                    vec![timing("lua_update", ScheduleStage::Script, stage_micros)],
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

    /// 独占 stage 的耗时**不能**进闸门读数：真实工程里 Script（Lua）恰好是最大的一块，
    /// 而 Lua 状态是单线程的。判据用可并行口径，全量口径只作对照保留。
    #[test]
    fn exclusive_stages_are_removed_from_the_gate_reading() {
        let mut profile = FrameProfile::new();
        profile.set_exclusive_stages([ScheduleStage::Script]);
        profile.record(
            &report(
                700,
                vec![
                    timing("lua_update", ScheduleStage::Script, 700),
                    timing("physics2d_step", ScheduleStage::Physics, 100),
                    timing("physics3d_step", ScheduleStage::Physics, 50),
                ],
            ),
            Duration::from_micros(1_000),
        );

        let summary = profile.summary();
        // 全量口径：Script 占 70%，是"最慢 stage"。
        assert_eq!(
            summary.worst_stage.as_ref().expect("全量最慢").stage,
            ScheduleStage::Script
        );
        assert!((summary.literal_gate_share_pct() - 70.0).abs() < 1e-9);
        // 可并行口径：Physics 15%，判定读它。
        let worst_parallel = summary.worst_parallel_stage.as_ref().expect("可并行最慢");
        assert_eq!(worst_parallel.stage, ScheduleStage::Physics);
        assert!((summary.gate_share_pct() - 15.0).abs() < 1e-9);
        assert!(summary.passes_gate(), "15% < 20% ⇒ 不做");
        assert_eq!(summary.verdict(), "skip");
        // Script 自己的可并行合计是 0（全被标为独占）。
        let script = summary
            .stages
            .iter()
            .find(|stage| stage.stage == ScheduleStage::Script)
            .expect("Script stage");
        assert_eq!(script.parallel_elapsed.count, 0);
        assert_eq!(script.parallel_share_pct, 0.0);
        assert!(
            summary
                .summary_line()
                .contains("worst_parallel_stage=Physics")
        );
    }

    /// 全部系统都被标为独占时，闸门读数必须是 0 而不是"没有数据 → 无穷大"。
    #[test]
    fn all_exclusive_systems_leave_no_parallel_work() {
        let mut profile = FrameProfile::new();
        profile.set_exclusive_stages([ScheduleStage::Script, ScheduleStage::Simulation]);
        profile.record(
            &report(
                900,
                vec![
                    timing("lua_update", ScheduleStage::Script, 700),
                    timing("particles_step", ScheduleStage::Simulation, 200),
                ],
            ),
            Duration::from_micros(1_000),
        );
        let summary = profile.summary();
        assert!((summary.literal_gate_share_pct() - 70.0).abs() < 1e-9);
        assert_eq!(summary.gate_share_pct(), 0.0);
        assert!(summary.worst_parallel_stage.is_none());
        assert!(summary.passes_gate());
        assert!(summary.summary_line().contains("worst_parallel_stage=none"));
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

    /// P3-R205：`environment` 按写入顺序出现在 JSON 里，空时是 `{}`；值里的反斜杠与引号
    /// 必须转义——宿主会把 shader 目录（可能含 `\`）填进来，破了 JSON 台账就作废。
    #[test]
    fn environment_is_serialized_in_order_and_escaped() {
        let mut profile = FrameProfile::new();
        assert!(profile.summary().to_json().contains("\"environment\":{}"));

        profile.set_environment([
            ("quality", "medium"),
            ("planar_divisor", "4"),
            ("shader_dir", r"C:\tmp"),
            ("quote", "a\"b"),
        ]);
        let summary = profile.summary();
        assert_eq!(summary.environment.len(), 4);
        let json = summary.to_json();
        assert!(
            json.contains(
                "\"environment\":{\"quality\":\"medium\",\"planar_divisor\":\"4\",\
                 \"shader_dir\":\"C:\\\\tmp\",\"quote\":\"a\\\"b\"}"
            ),
            "{json}"
        );
        assert!(!json.contains("NaN"));
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

    /// 宿主阶段：`other` 必须是整帧减三段（而不是"剩下的没人管"），且缺 GPU 时间戳时
    /// 只让 `gpu_frame.count = 0`，不影响其余读数。
    #[test]
    fn host_phases_attribute_time_outside_the_scheduler() {
        let mut profile = FrameProfile::new();
        profile.record(
            &report(
                400,
                vec![timing("physics2d_step", ScheduleStage::Physics, 400)],
            ),
            Duration::from_micros(1_000),
        );
        profile.record_host_phases(HostPhases {
            sim: Duration::from_micros(300),
            extract: Duration::from_micros(200),
            submit: Duration::from_micros(400),
            render_cpu_us: Some(380),
            gpu_frame_us: None,
            render_acquire_us: Some(10),
            render_record_us: Some(200),
            render_submit_us: Some(30),
            render_present_us: Some(150),
            shadow_record_us: Some(40),
            point_shadow_record_us: Some(90),
            gpu_pass_frame_us: Some(900),
            gpu_passes: vec![("clear", 100), ("depth_opaque", 800)],
            ..HostPhases::default()
        });
        let summary = profile.summary();
        assert_eq!(summary.host.sim.p50_us(), 300.0);
        assert_eq!(summary.host.extract.p50_us(), 200.0);
        assert_eq!(summary.host.submit.p50_us(), 400.0);
        assert_eq!(summary.host.other.p50_us(), 100.0);
        assert!((summary.host.other_share_pct - 10.0).abs() < 1e-9);
        assert_eq!(summary.host.render_cpu.p50_us(), 380.0);
        assert_eq!(summary.host.gpu_frame.count, 0);
        // P3-R181：六个桶必须**各自**落到自己的样本里。挂错桶（例如把点光段累加进方向光段）
        // 在画面上完全看不出来，只有这里的逐桶读数能抓住。
        assert_eq!(summary.host.render_acquire.p50_us(), 10.0);
        assert_eq!(summary.host.render_record.p50_us(), 200.0);
        assert_eq!(summary.host.render_submit.p50_us(), 30.0);
        assert_eq!(summary.host.render_present.p50_us(), 150.0);
        assert_eq!(summary.host.shadow_record.p50_us(), 40.0);
        assert_eq!(summary.host.point_shadow_record.p50_us(), 90.0);
        assert_eq!(summary.host.render_record.count, 1);
        assert!(
            summary.host.shadow_record.p50_us() + summary.host.point_shadow_record.p50_us()
                <= summary.host.render_record.p50_us(),
            "两个阴影子桶之和不能超过总录制：{} + {} > {}",
            summary.host.shadow_record.p50_us(),
            summary.host.point_shadow_record.p50_us(),
            summary.host.render_record.p50_us()
        );
        let json = summary.to_json();
        assert!(
            json.contains("\"host\":{\"sim\":"),
            "JSON 缺少 host: {json}"
        );
        assert!(json.contains("\"gpu_frame\":{\"count\":0"));
        for key in [
            "render_acquire",
            "render_record",
            "render_submit",
            "render_present",
            "shadow_record",
            "point_shadow_record",
        ] {
            assert!(
                json.contains(&format!("\"{key}\":{{\"count\":1")),
                "JSON 缺少宿主桶 {key}: {json}"
            );
        }
        // P3-R182：逐 pass GPU 归因。整帧在这条用例里是 1000 µs，所以占比可以直接口算。
        assert_eq!(summary.host.gpu_pass_frame.p50_us(), 900.0);
        let passes = &summary.gpu_passes;
        assert_eq!(
            passes.iter().map(|pass| pass.name).collect::<Vec<_>>(),
            vec!["clear", "depth_opaque"],
            "逐 pass 报告必须保持渲染器给出的首见顺序，否则两次运行没法对着看"
        );
        assert_eq!(passes[0].elapsed.p50_us(), 100.0);
        assert_eq!(passes[1].elapsed.p50_us(), 800.0);
        assert!(
            (passes[0].share_pct - 10.0).abs() < 1e-9,
            "{}",
            passes[0].share_pct
        );
        assert!(
            (passes[1].share_pct - 80.0).abs() < 1e-9,
            "{}",
            passes[1].share_pct
        );
        let pass_sum: f64 = passes.iter().map(|pass| pass.elapsed.p50_us()).sum();
        assert!(
            pass_sum <= summary.host.gpu_pass_frame.p50_us(),
            "逐 pass 之和 {pass_sum} 超过了时间戳自报的整帧 GPU {}：口径错了或 pass 之间有重叠",
            summary.host.gpu_pass_frame.p50_us()
        );
        assert!(
            json.contains("\"gpu_passes\":[{\"name\":\"clear\""),
            "JSON 缺少 gpu_passes: {json}"
        );
        assert!(json.contains("\"gpu_pass_frame\":{\"count\":1"));

        // 旧调用方（只填 sim/extract/submit）不能让新桶以 0 混进统计：`count = 0` 才是
        // 「这一帧没有这个读数」，0 µs 会被读成「这段不花钱」。
        let mut legacy = FrameProfile::new();
        legacy.record(&report(1, vec![]), Duration::from_micros(100));
        legacy.record_host_phases(HostPhases {
            sim: Duration::from_micros(40),
            extract: Duration::from_micros(30),
            submit: Duration::from_micros(20),
            ..HostPhases::default()
        });
        let legacy_summary = legacy.summary();
        let legacy_host = legacy_summary.host;
        assert_eq!(legacy_host.render_record.count, 0);
        assert_eq!(legacy_host.point_shadow_record.count, 0);
        assert_eq!(legacy_host.render_present.count, 0);
        assert_eq!(legacy_host.gpu_pass_frame.count, 0);
        assert!(
            legacy_summary.gpu_passes.is_empty(),
            "设备不支持时间戳时必须是空数组，不能是一排 0 µs 的行"
        );
        assert_eq!(
            legacy_summary.cull.frames, 0,
            "旧调用方没填剔除读数时累计桶必须是 0 帧，而不是把 0 候选当样本"
        );

        // 三段超过整帧（计时抖动/异步 present）时 `other` 饱和到 0，不出现负数/NaN。
        let mut skewed = FrameProfile::new();
        skewed.record(&report(1, vec![]), Duration::from_micros(100));
        skewed.record_host_phases(HostPhases {
            sim: Duration::from_micros(80),
            extract: Duration::from_micros(80),
            submit: Duration::from_micros(80),
            ..HostPhases::default()
        });
        assert_eq!(skewed.summary().host.other.p50_us(), 0.0);
    }

    /// P3-R183：剔除累计数的是**实例**不是帧，且 0 候选帧不进桶。
    ///
    /// 两条口径各自能抓一种假绿：① 把 0 候选帧也算进分母 ⇒ 剔除率被纯 2D 帧稀释；
    /// ② 按帧平均而不是按候选总数算比率 ⇒ 大场景与小场景等权，A/B 读数失真。
    #[test]
    fn cull_totals_count_instances_and_skip_empty_frames() {
        let mut profile = FrameProfile::new();
        let mut frame = |candidates: u64, culled: u64, gpu_skipped: u64| {
            profile.record(&report(1, vec![]), Duration::from_micros(1_000));
            profile.record_host_phases(HostPhases {
                sim: Duration::from_micros(100),
                extract: Duration::from_micros(100),
                submit: Duration::from_micros(100),
                cull_candidates: Some(candidates),
                cull_frustum: Some(culled),
                gpu_motion_culled: (gpu_skipped > 0).then_some(gpu_skipped),
                gpu_motion_culled_instances: (gpu_skipped > 0).then_some(gpu_skipped * 60),
                ..HostPhases::default()
            });
        };
        frame(1_000, 250, 40);
        frame(0, 0, 0); // 纯 2D 帧：不进桶。
        frame(500, 100, 0);
        let summary = profile.summary();
        assert_eq!(summary.cull.frames, 2, "0 候选帧不许进桶");
        assert_eq!(summary.cull.candidates, 1_500);
        assert_eq!(summary.cull.frustum_culled, 350);
        assert_eq!(
            summary.cull.gpu_motion_skipped, 40,
            "整批跳过数独立累计（与候选桶无关）"
        );
        assert_eq!(
            summary.cull.gpu_motion_skipped_instances, 2_400,
            "跳过的实例数必须与 draw 条数分开累计"
        );
        let expected = 350.0 * 100.0 / 1_500.0;
        assert!(
            (summary.cull.culled_pct - expected).abs() < 1e-9,
            "剔除率分母必须是候选总数：{} != {expected}",
            summary.cull.culled_pct
        );
        let json = summary.to_json();
        assert!(
            json.contains(&format!(
                "\"cull\":{{\"frames\":2,\"candidates\":1500,\"frustum_culled\":350,\"gpu_motion_skipped\":40,\"gpu_motion_skipped_instances\":2400,\"culled_pct\":{expected:.3}}}"
            )),
            "JSON 缺少剔除累计或口径不对: {json}"
        );
    }

    /// P3-R247：阴影计数只统计"真有 draw"的帧，比率分母是尝试次数（提交 + 剔除）。
    ///
    /// 抓三种假绿：① 无方向光的帧被算进分母 ⇒ 剔除率被稀释；② 只报提交不报剔除 ⇒
    /// 看不出逐级联剔除有没有生效；③ 把剔除率按帧平均 ⇒ 大小场景等权。
    #[test]
    fn shadow_totals_skip_frames_without_draws() {
        let mut profile = FrameProfile::new();
        let mut frame = |draws: u64,
                         culled: u64,
                         instances: u64,
                         skipped: u64,
                         point: u64,
                         distance: u64,
                         point_distance: u64,
                         point_draws: u64,
                         point_instances: u64,
                         point_range: u64,
                         point_face: u64| {
            profile.record(&report(1, vec![]), Duration::from_micros(1_000));
            profile.record_host_phases(HostPhases {
                sim: Duration::from_micros(100),
                extract: Duration::from_micros(100),
                submit: Duration::from_micros(100),
                shadow_draws: Some(draws),
                shadow_draws_culled: Some(culled),
                shadow_instances: Some(instances),
                shadow_draws_skipped: Some(skipped),
                point_shadow_draws_skipped: Some(point),
                shadow_draws_distance_skipped: Some(distance),
                point_shadow_draws_distance_skipped: Some(point_distance),
                point_shadow_draws: Some(point_draws),
                point_shadow_instances: Some(point_instances),
                point_shadow_range_culled: Some(point_range),
                point_shadow_face_draws_culled: Some(point_face),
                ..HostPhases::default()
            });
        };
        frame(40, 4, 80_000, 6, 30, 2, 10, 12, 240, 500, 700);
        frame(0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0); // 无方向光 / 纯 2D 帧：不进 CSM 桶。
        frame(36, 8, 60_000, 3, 12, 1, 5, 7, 140, 300, 400);
        frame(0, 0, 0, 9, 4, 4, 0, 5, 100, 200, 100); // P3-R248：全被排除档挡掉也照样是"有阴影工作"的帧。
        let summary = profile.summary();
        assert_eq!(summary.shadow.frames, 3, "0 draw 且 0 排除的帧不许进桶");
        assert_eq!(summary.shadow.draws, 76);
        assert_eq!(summary.shadow.draws_culled, 12);
        assert_eq!(summary.shadow.instances, 140_000);
        assert_eq!(summary.shadow.skipped, 18, "排除档单独计数");
        assert_eq!(summary.shadow.point_skipped, 46, "P3-R249：点光链单独计数");
        assert_eq!(
            summary.shadow.distance_skipped, 7,
            "P3-R256：距离预算单独计数"
        );
        assert_eq!(
            summary.shadow.point_distance_skipped, 15,
            "P3-R256：点光链的距离预算也单独一列"
        );
        // P3-R261：点光链的提交按帧独立累计（第 2 帧 point 两项都是 0，但它照样是"有提交"
        // 的帧——纯 2D 帧里点光链一次也没录，所以这里不影响结果，只用非零帧验证加总口径）。
        assert_eq!(summary.shadow.point_draws, 24, "点光链提交 draw 单独累计");
        assert_eq!(
            summary.shadow.point_instances, 480,
            "点光链提交实例单独累计"
        );
        // P3-R262：点光链两项剔除无条件累计（第 2 帧全 0 不影响加总），守恒式的另外两列。
        assert_eq!(
            summary.shadow.point_range_culled, 1000,
            "点光链 range 剔除单独累计"
        );
        assert_eq!(
            summary.shadow.point_face_culled, 1200,
            "点光链面剔除单独累计"
        );
        let expected = 12.0 * 100.0 / 88.0;
        assert!(
            (summary.shadow.draws_culled_pct - expected).abs() < 1e-9,
            "剔除率分母必须是尝试次数：{} != {expected}",
            summary.shadow.draws_culled_pct
        );
        let json = summary.to_json();
        assert!(
            json.contains(&format!(
                "\"shadow\":{{\"frames\":3,\"draws\":76,\"draws_culled\":12,\"instances\":140000,\"skipped\":18,\"point_skipped\":46,\"distance_skipped\":7,\"point_distance_skipped\":15,\"point_draws\":24,\"point_instances\":480,\"point_range_culled\":1000,\"point_face_culled\":1200,\"draws_culled_pct\":{expected:.3}}}"
            )),
            "JSON 缺少阴影计数或口径不对: {json}"
        );
    }
}
