//! P3-E06-4：可复现的性能采集包、重放对比与差分。
//!
//! 为什么需要它：E06-1..E06-3 给的是"此刻这一帧"的读数。同一段代码改前改后到底慢没慢，
//! 不能靠两次目视折线图——CPU 频率、缓存与后台任务都会漂。这里把一次采集需要的全部输入
//! 固定成**有版本的 JSON**：场景指纹、固定步长、随机种子、逐帧输入日志、帧/系统/通道
//! 的 p50/p95、内存分项，以及**最终状态指纹**。重放用同一份输入跑同一场景；对比时先比
//! 场景与输入是否真的一致，再比读数，状态指纹不同直接判 `drift`。
//!
//! 边界（不假装）：
//! - 计时天然有噪声。默认容差 10%，只对 **p50** 判越界；p95 一并记录但不参与判定。
//! - `input_log` 记录宿主喂进 `InputSnapshot` 的原始状态，不反推动作绑定。
//! - 设备不支持的读数用 `None`，不是 0（Lua 堆、进程 RSS、GPU 通道时间都如此）。
//!
//! 数据面与证据面分离：本模块只做模型 + 纯函数；跑场景、写文件、打印读数在
//! `wp0_capture_replay` 例子里。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// 采集包格式版本。字段增删必须改这个串，读取侧据此拒绝旧/新格式混用。
pub const CAPTURE_REPORT_VERSION: &str = "p3-capture-v1";
/// 采集包记录的引擎版本。取引擎 crate 的版本，而不是宿主的版本。
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
/// 差分报告格式版本。
pub const CAPTURE_DIFF_VERSION: &str = "p3-capture-diff-v1";
/// 默认的重放容差（百分点）：p50 差值超过它才判 `drift`。
pub const DEFAULT_DRIFT_THRESHOLD_PCT: f32 = 10.0;
/// 低于这个 p50 的系统不参与稳定判定：本机计时分辨率约 100 ns，0.1–2 µs 的系统
/// 在一次运行内就能翻倍，把它当回归等于让噪声决定结论。仍会完整记录并出现在 `systems`。
pub const MIN_GATED_P50_US: f64 = 10.0;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// 一次采集的完整输入与读数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptureBundle {
    pub report_version: String,
    pub engine_version: String,
    /// 采集时的 Git commit；运行时无法得知时是 `"unknown"`，不是编造的值。
    pub engine_commit: String,
    pub captured_at_unix_ms: u64,
    pub scenario: CaptureScenario,
    /// 采集结束时场景状态的指纹（十六进制）。重放必须逐位相同。
    pub state_hash: String,
    pub frames: FrameStats,
    pub memory: MemorySnapshot,
    /// 逐系统耗时（每帧一个样本）。
    pub systems: Vec<TimingStat>,
    /// 逐通道 GPU 时间；设备不支持时为空且 `gpu_available=false`。
    pub gpu_passes: Vec<TimingStat>,
    pub gpu_available: bool,
    /// 没有计时读数时的原因（例如 headless 例子不创建渲染器）。
    pub gpu_note: Option<String>,
}

/// 重放所需的场景与输入描述。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptureScenario {
    pub name: String,
    /// 场景文件路径（相对工程根或绝对路径）；程序生成的场景为 `None`。
    pub scene_path: Option<String>,
    /// 场景内容指纹（文件字节或生成器的规范描述）。
    pub scene_hash: String,
    pub viewport: [u32; 2],
    pub fixed_dt_seconds: f64,
    pub seed: u64,
    pub warmup_frames: u32,
    pub frames: u32,
    /// 每个采样帧一条，按帧号升序。
    pub input_log: Vec<CapturedInput>,
}

/// 一帧输入的稳定记录。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapturedInput {
    pub frame: u32,
    pub cursor: [f64; 2],
    pub cursor_delta: [f64; 2],
    pub scroll: [f32; 2],
    pub focused: bool,
    pub held_keys: Vec<String>,
    pub pressed_keys: Vec<String>,
    pub released_keys: Vec<String>,
    pub virtual_keys: Vec<String>,
    pub held_mouse: Vec<String>,
    pub pressed_mouse: Vec<String>,
    pub released_mouse: Vec<String>,
    pub held_gamepad: Vec<u8>,
    pub pressed_gamepad: Vec<u8>,
    pub released_gamepad: Vec<u8>,
    pub gamepad_axes: Vec<[f32; 2]>,
    pub press_cursor: [f64; 2],
    pub press_cursor_points: Vec<[f64; 2]>,
}

/// 帧时间分布（毫秒）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FrameStats {
    pub samples: u32,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}

/// 一组耗时样本的 p50/p95/最大值（微秒）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimingStat {
    pub name: String,
    pub samples: u32,
    pub p50_us: f64,
    pub p95_us: f64,
    pub max_us: f64,
    pub total_us: f64,
}

impl TimingStat {
    /// 从逐帧微秒样本得到统计量；空样本返回 `None`（不是全 0 的假条目）。
    pub fn from_micros(name: impl Into<String>, samples: &[f64]) -> Option<Self> {
        if samples.is_empty() {
            return None;
        }
        let mut sorted = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        Some(Self {
            name: name.into(),
            samples: sorted.len() as u32,
            p50_us: percentile(&sorted, 50),
            p95_us: percentile(&sorted, 95),
            max_us: *sorted.last().expect("non-empty"),
            total_us: sorted.iter().sum(),
        })
    }
}

impl FrameStats {
    pub fn from_millis(samples: &[f64]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        let mut sorted = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        Self {
            samples: sorted.len() as u32,
            p50_ms: percentile(&sorted, 50),
            p95_ms: percentile(&sorted, 95),
            max_ms: *sorted.last().expect("non-empty"),
        }
    }
}

/// 内存分项。取不到的项是 `None`，面板/报告显示"不可用"。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MemorySnapshot {
    /// 进程工作集（Windows `GetProcessMemoryInfo`）；其他平台 `None`。
    pub process_memory_mib: Option<f64>,
    /// Lua VM 实际占用的堆字节（mlua `used_memory`）。
    pub lua_heap_bytes: Option<u64>,
    /// 已上传贴图的估算字节。
    pub texture_bytes: Option<u64>,
    /// 已上传网格的估算字节。
    pub mesh_bytes: Option<u64>,
    /// 2D 顶点缓冲容量（字节）。
    pub vertex_buffer_bytes: Option<u64>,
    /// 回读路径的 CPU 缓冲字节（不是资源解码缓冲）。
    pub readback_cpu_bytes: Option<u64>,
    pub asset_entries: Option<u64>,
    pub scene_entities: Option<u64>,
    pub runtime_entities: Option<u64>,
}

/// 两个采集包之间的差异。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptureDiff {
    pub report_version: String,
    pub baseline: CaptureIdentity,
    pub candidate: CaptureIdentity,
    pub threshold_pct: f32,
    /// 场景/步长/种子/视口/帧数是否一致；不一致时读数对比没有意义。
    pub scenario_match: bool,
    pub scenario_differences: Vec<String>,
    pub state: StateDiff,
    pub frames: FrameDelta,
    pub systems: Vec<TimingDelta>,
    pub gpu_passes: Vec<TimingDelta>,
    /// p50 变慢最多的系统（最多 5 个）。
    pub slowest: Vec<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// 场景一致 + 状态指纹一致 + 无系统变慢/增删 + 帧 p50 在容差内。
    pub stable: bool,
    /// `stable` / `drift`。
    pub verdict: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureIdentity {
    pub scenario: String,
    pub engine_version: String,
    pub engine_commit: String,
    pub captured_at_unix_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateDiff {
    pub baseline: String,
    pub candidate: String,
    pub equal: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FrameDelta {
    pub baseline_p50_ms: f64,
    pub candidate_p50_ms: f64,
    pub delta_pct: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimingStatus {
    Stable,
    Faster,
    Slower,
    Added,
    Removed,
    NoBaseline,
}

impl TimingStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Faster => "faster",
            Self::Slower => "slower",
            Self::Added => "added",
            Self::Removed => "removed",
            Self::NoBaseline => "no_baseline",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimingDelta {
    pub name: String,
    pub baseline_p50_us: Option<f64>,
    pub baseline_p95_us: Option<f64>,
    pub candidate_p50_us: Option<f64>,
    pub candidate_p95_us: Option<f64>,
    pub delta_p50_pct: Option<f32>,
    pub delta_p95_pct: Option<f32>,
    /// 是否参与 `stable` 判定。增删的条目恒为 `true`；p50 低于
    /// [`MIN_GATED_P50_US`] 的条目为 `false`（只记录，不判越界）。
    pub gated: bool,
    pub status: TimingStatus,
}

impl CaptureBundle {
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("capture bundle is serializable")
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let bundle: Self =
            serde_json::from_str(text).map_err(|error| format!("采集包 JSON 解析失败：{error}"))?;
        if bundle.report_version != CAPTURE_REPORT_VERSION {
            return Err(format!(
                "采集包版本 {}，期望 {CAPTURE_REPORT_VERSION}",
                bundle.report_version
            ));
        }
        Ok(bundle)
    }

    /// 与另一份采集对比。`threshold_pct` 只作用于系统/帧的 **p50**。
    pub fn diff(&self, candidate: &Self, threshold_pct: f32) -> CaptureDiff {
        let scenario_differences = scenario_differences(&self.scenario, &candidate.scenario);
        let scenario_match = scenario_differences.is_empty();
        let state_equal = self.state_hash == candidate.state_hash;
        let systems = diff_timings(&self.systems, &candidate.systems, threshold_pct);
        let gpu_passes = diff_timings(&self.gpu_passes, &candidate.gpu_passes, threshold_pct);
        let slowest = systems
            .iter()
            .filter(|delta| delta.gated && delta.status == TimingStatus::Slower)
            .take(5)
            .map(|delta| delta.name.clone())
            .collect::<Vec<_>>();
        let added = systems
            .iter()
            .filter(|delta| delta.gated && delta.status == TimingStatus::Added)
            .map(|delta| delta.name.clone())
            .collect::<Vec<_>>();
        let removed = systems
            .iter()
            .filter(|delta| delta.gated && delta.status == TimingStatus::Removed)
            .map(|delta| delta.name.clone())
            .collect::<Vec<_>>();
        let frames = FrameDelta {
            baseline_p50_ms: self.frames.p50_ms,
            candidate_p50_ms: candidate.frames.p50_ms,
            delta_pct: pct_delta(self.frames.p50_ms, candidate.frames.p50_ms),
        };
        let frame_within = frames
            .delta_pct
            .is_none_or(|delta| delta.abs() <= threshold_pct);
        let within = |delta: &TimingDelta| {
            !delta.gated || matches!(delta.status, TimingStatus::Stable | TimingStatus::Faster)
        };
        let timings_within = systems.iter().all(within) && gpu_passes.iter().all(within);
        let stable = scenario_match && state_equal && frame_within && timings_within;
        CaptureDiff {
            report_version: CAPTURE_DIFF_VERSION.to_owned(),
            baseline: self.identity(),
            candidate: candidate.identity(),
            threshold_pct,
            scenario_match,
            scenario_differences,
            state: StateDiff {
                baseline: self.state_hash.clone(),
                candidate: candidate.state_hash.clone(),
                equal: state_equal,
            },
            frames,
            systems,
            gpu_passes,
            slowest,
            added,
            removed,
            stable,
            verdict: if stable { "stable" } else { "drift" }.to_owned(),
        }
    }

    fn identity(&self) -> CaptureIdentity {
        CaptureIdentity {
            scenario: self.scenario.name.clone(),
            engine_version: self.engine_version.clone(),
            engine_commit: self.engine_commit.clone(),
            captured_at_unix_ms: self.captured_at_unix_ms,
        }
    }

    /// 人读的一行结论（例子/面板共用）。
    pub fn summary_line(&self, diff: &CaptureDiff) -> String {
        format!(
            "capture scenario={} frames={} state={} p50={:.3}ms systems={} gpu_passes={} \
             verdict={} slowest={} state_equal={}",
            self.scenario.name,
            self.scenario.frames,
            self.state_hash,
            self.frames.p50_ms,
            self.systems.len(),
            self.gpu_passes.len(),
            diff.verdict,
            if diff.slowest.is_empty() {
                "none".to_owned()
            } else {
                diff.slowest.join(",")
            },
            diff.state.equal
        )
    }
}

fn scenario_differences(baseline: &CaptureScenario, candidate: &CaptureScenario) -> Vec<String> {
    let mut differences = Vec::new();
    if baseline.scene_hash != candidate.scene_hash {
        differences.push("scene_hash".to_owned());
    }
    if baseline.viewport != candidate.viewport {
        differences.push("viewport".to_owned());
    }
    if baseline.fixed_dt_seconds != candidate.fixed_dt_seconds {
        differences.push("fixed_dt".to_owned());
    }
    if baseline.seed != candidate.seed {
        differences.push("seed".to_owned());
    }
    if baseline.frames != candidate.frames {
        differences.push("frames".to_owned());
    }
    if baseline.warmup_frames != candidate.warmup_frames {
        differences.push("warmup_frames".to_owned());
    }
    if baseline.input_log.len() != candidate.input_log.len() {
        differences.push("input_log.len".to_owned());
    } else if let Some((index, _)) = baseline
        .input_log
        .iter()
        .zip(candidate.input_log.iter())
        .enumerate()
        .find(|(_, (left, right))| left != right)
    {
        differences.push(format!("input_log[{index}]"));
    }
    differences
}

fn diff_timings(
    baseline: &[TimingStat],
    candidate: &[TimingStat],
    threshold_pct: f32,
) -> Vec<TimingDelta> {
    let baseline = baseline
        .iter()
        .map(|stat| (stat.name.as_str(), stat))
        .collect::<BTreeMap<_, _>>();
    let candidate = candidate
        .iter()
        .map(|stat| (stat.name.as_str(), stat))
        .collect::<BTreeMap<_, _>>();
    let names = baseline
        .keys()
        .chain(candidate.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    names
        .into_iter()
        .map(|name| {
            let before = baseline.get(name).copied();
            let after = candidate.get(name).copied();
            let delta_p50_pct = match (before, after) {
                (Some(before), Some(after)) => pct_delta(before.p50_us, after.p50_us),
                _ => None,
            };
            let delta_p95_pct = match (before, after) {
                (Some(before), Some(after)) => pct_delta(before.p95_us, after.p95_us),
                _ => None,
            };
            let status = match (before, after, delta_p50_pct) {
                (None, Some(_), _) => TimingStatus::Added,
                (Some(_), None, _) => TimingStatus::Removed,
                (Some(_), Some(_), None) => TimingStatus::NoBaseline,
                (Some(_), Some(_), Some(delta)) if delta > threshold_pct => TimingStatus::Slower,
                (Some(_), Some(_), Some(delta)) if delta < -threshold_pct => TimingStatus::Faster,
                _ => TimingStatus::Stable,
            };
            // 增删恒参与判定；两侧都有的条目按较大 p50 与计时分辨率闸值比较。
            let gated = match (before, after) {
                (Some(before), Some(after)) => before.p50_us.max(after.p50_us) >= MIN_GATED_P50_US,
                _ => true,
            };
            TimingDelta {
                name: name.to_owned(),
                baseline_p50_us: before.map(|stat| stat.p50_us),
                baseline_p95_us: before.map(|stat| stat.p95_us),
                candidate_p50_us: after.map(|stat| stat.p50_us),
                candidate_p95_us: after.map(|stat| stat.p95_us),
                delta_p50_pct,
                delta_p95_pct,
                gated,
                status,
            }
        })
        .collect()
}

fn pct_delta(baseline: f64, candidate: f64) -> Option<f32> {
    if !baseline.is_finite() || !candidate.is_finite() || baseline <= 0.0 {
        return None;
    }
    Some((((candidate - baseline) / baseline) * 100.0) as f32)
}

/// 最近秩百分位（与 `wp0_profile_capture` 同口径）。
fn percentile(sorted: &[f64], percent: usize) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let at = ((sorted.len() * percent).saturating_sub(1) / 100).min(sorted.len() - 1);
    sorted[at]
}

/// 顺序稳定的 FNV-1a 指纹累加器：把帧产物逐项喂进来，得到可跨进程比较的十六进制串。
#[derive(Clone, Debug)]
pub struct FingerprintBuilder {
    hash: u64,
}

impl Default for FingerprintBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl FingerprintBuilder {
    pub fn new() -> Self {
        Self { hash: FNV_OFFSET }
    }

    pub fn push_bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.hash ^= u64::from(*byte);
            self.hash = self.hash.wrapping_mul(FNV_PRIME);
        }
    }

    pub fn push_u64(&mut self, value: u64) {
        self.push_bytes(&value.to_le_bytes());
    }

    pub fn push_f32(&mut self, value: f32) {
        self.push_u64(u64::from(value.to_bits()));
    }

    pub fn finish_hex(self) -> String {
        format!("{:016x}", self.hash)
    }
}

/// 一段字节的指纹（场景文件/规范描述用）。
pub fn fingerprint_bytes(bytes: &[u8]) -> String {
    let mut builder = FingerprintBuilder::new();
    builder.push_bytes(bytes);
    builder.finish_hex()
}

/// Unix 毫秒时间戳；系统时钟早于纪元时为 0（不 panic）。
pub fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scenario(name: &str) -> CaptureScenario {
        CaptureScenario {
            name: name.to_owned(),
            scene_path: Some("scenes/demo.scene".to_owned()),
            scene_hash: "0123456789abcdef".to_owned(),
            viewport: [1280, 720],
            fixed_dt_seconds: 1.0 / 60.0,
            seed: 7,
            warmup_frames: 5,
            frames: 120,
            input_log: vec![CapturedInput {
                frame: 0,
                cursor: [10.0, 20.0],
                cursor_delta: [10.0, 20.0],
                scroll: [0.0, 0.0],
                focused: true,
                held_keys: vec!["keyd".to_owned()],
                pressed_keys: vec!["keyd".to_owned()],
                released_keys: Vec::new(),
                virtual_keys: Vec::new(),
                held_mouse: Vec::new(),
                pressed_mouse: Vec::new(),
                released_mouse: Vec::new(),
                held_gamepad: Vec::new(),
                pressed_gamepad: Vec::new(),
                released_gamepad: Vec::new(),
                gamepad_axes: Vec::new(),
                press_cursor: [0.0, 0.0],
                press_cursor_points: Vec::new(),
            }],
        }
    }

    fn bundle(state_hash: &str, frame_p50: f64, systems: &[(&str, f64)]) -> CaptureBundle {
        CaptureBundle {
            report_version: CAPTURE_REPORT_VERSION.to_owned(),
            engine_version: "0.1.0".to_owned(),
            engine_commit: "deadbeef".to_owned(),
            captured_at_unix_ms: 1,
            scenario: scenario("unit"),
            state_hash: state_hash.to_owned(),
            frames: FrameStats {
                samples: 120,
                p50_ms: frame_p50,
                p95_ms: frame_p50 * 1.2,
                max_ms: frame_p50 * 2.0,
            },
            memory: MemorySnapshot {
                lua_heap_bytes: Some(1024),
                ..MemorySnapshot::default()
            },
            systems: systems
                .iter()
                .map(|(name, us)| TimingStat::from_micros(*name, &[*us]).expect("sample"))
                .collect(),
            gpu_passes: Vec::new(),
            gpu_available: false,
            gpu_note: Some("headless".to_owned()),
        }
    }

    #[test]
    fn capture_round_trips_through_json() {
        let bundle = bundle("abc", 16.0, &[("movement", 120.0)]);
        let text = bundle.to_json_pretty();
        let parsed = CaptureBundle::from_json(&text).expect("parse");
        assert_eq!(parsed, bundle);
        assert!(text.contains("\"report_version\": \"p3-capture-v1\""));
        assert!(text.contains("\"lua_heap_bytes\": 1024"));
    }

    #[test]
    fn from_json_rejects_unknown_version() {
        let mut value = serde_json::to_value(bundle("abc", 16.0, &[])).expect("value");
        value["report_version"] = serde_json::json!("p3-capture-v99");
        let error = CaptureBundle::from_json(&value.to_string()).expect_err("version mismatch");
        assert!(error.contains("p3-capture-v99"), "{error}");
    }

    #[test]
    fn diff_flags_slower_added_and_removed_systems() {
        let baseline = bundle(
            "same",
            16.0,
            &[("movement", 100.0), ("physics", 50.0), ("legacy", 20.0)],
        );
        let mut candidate = bundle(
            "same",
            16.5,
            &[("movement", 140.0), ("physics", 50.0), ("new_system", 5.0)],
        );
        // 场景一致（否则差分先判场景不一致）。
        candidate.scenario = baseline.scenario.clone();
        let diff = baseline.diff(&candidate, 10.0);
        assert!(diff.scenario_match, "{:?}", diff.scenario_differences);
        assert!(diff.state.equal);
        assert_eq!(diff.slowest, vec!["movement"]);
        assert_eq!(diff.added, vec!["new_system"]);
        assert_eq!(diff.removed, vec!["legacy"]);
        assert!(!diff.stable);
        assert_eq!(diff.verdict, "drift");
        let movement = diff
            .systems
            .iter()
            .find(|delta| delta.name == "movement")
            .expect("movement delta");
        assert_eq!(movement.status, TimingStatus::Slower);
        assert!((movement.delta_p50_pct.expect("delta") - 40.0).abs() < 0.01);
    }

    #[test]
    fn diff_is_stable_when_state_and_timings_agree() {
        let baseline = bundle("same", 16.0, &[("movement", 100.0), ("physics", 50.0)]);
        let mut candidate = bundle("same", 16.4, &[("movement", 104.0), ("physics", 48.0)]);
        candidate.scenario = baseline.scenario.clone();
        let diff = baseline.diff(&candidate, 10.0);
        assert!(diff.stable, "{diff:?}");
        assert_eq!(diff.verdict, "stable");
        assert!(diff.slowest.is_empty());
        assert!(diff.added.is_empty() && diff.removed.is_empty());
    }

    #[test]
    fn sub_resolution_systems_are_reported_but_do_not_gate() {
        let baseline = bundle("same", 16.0, &[("tiny", 0.2), ("work", 100.0)]);
        let mut candidate = bundle("same", 16.0, &[("tiny", 0.9), ("work", 100.0)]);
        candidate.scenario = baseline.scenario.clone();
        let diff = baseline.diff(&candidate, 10.0);
        let tiny = diff
            .systems
            .iter()
            .find(|delta| delta.name == "tiny")
            .expect("tiny delta");
        assert_eq!(tiny.status, TimingStatus::Slower, "仍然记录越界读数");
        assert!(!tiny.gated, "低于计时分辨率闸值，不参与稳定判定");
        assert!(diff.slowest.is_empty());
        assert!(diff.stable, "{diff:?}");
    }

    #[test]
    fn state_or_scenario_mismatch_forces_drift() {
        let baseline = bundle("same", 16.0, &[("movement", 100.0)]);
        let mut candidate = bundle("different", 16.0, &[("movement", 100.0)]);
        candidate.scenario = baseline.scenario.clone();
        let state_diff = baseline.diff(&candidate, 10.0);
        assert!(!state_diff.state.equal);
        assert!(!state_diff.stable);

        let mut candidate = bundle("same", 16.0, &[("movement", 100.0)]);
        candidate.scenario.scene_hash = "fedcba9876543210".to_owned();
        let scenario_diff = baseline.diff(&candidate, 10.0);
        assert!(!scenario_diff.scenario_match);
        assert_eq!(scenario_diff.scenario_differences, vec!["scene_hash"]);
        assert!(!scenario_diff.stable);
    }

    #[test]
    fn fingerprint_is_order_sensitive_and_stable() {
        let a = fingerprint_bytes(b"abc");
        let b = fingerprint_bytes(b"abc");
        let c = fingerprint_bytes(b"acb");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 16);

        let mut builder = FingerprintBuilder::new();
        builder.push_f32(1.5);
        builder.push_u64(7);
        let first = builder.clone().finish_hex();
        builder.push_u64(8);
        assert_ne!(first, builder.finish_hex());
    }

    #[test]
    fn timing_stat_reports_percentiles_and_rejects_empty() {
        assert!(TimingStat::from_micros("empty", &[]).is_none());
        let stat = TimingStat::from_micros("system", &[10.0, 30.0, 20.0, 40.0]).expect("stat");
        assert_eq!(stat.samples, 4);
        assert_eq!(stat.p50_us, 20.0);
        assert_eq!(stat.p95_us, 40.0);
        assert_eq!(stat.max_us, 40.0);
        assert_eq!(stat.total_us, 100.0);
    }
}
