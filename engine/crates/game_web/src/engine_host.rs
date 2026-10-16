//! 统一 Rust engine host（P1 第二片）：两个平台跑**同一台宿主**。
//!
//! ## 这一片补的是哪一段
//!
//! P1 第一片（[`crate::engine_core`]）把"画布 → device → Renderer → `RustScene` →
//! step/mesh/贴图/取景/render/present"收成了一份，但**帧的驱动**还在两个外壳里各写一遍：
//!
//! | 抽取前 | 浏览器 `gpu_host` | 微信 `host_runtime` |
//! | --- | --- | --- |
//! | 时钟 | 模块常量 → `FrameClock::new` | `engine_host_begin` options → `FrameClock::new` |
//! | 每帧 | `simulation_clock.plan(elapsed)` → `core.frame(plan.steps, plan.delta_seconds, input)` | `clock.plan(dt_ms/1000)` → `core.frame(plan.steps, plan.delta_seconds, input)` |
//!
//! 两张表逐格同形。**同形却各写一遍**就是漂移的入口：一处改了上限、另一处没改，
//! 读数上完全看不出来（帧率、draw 计数、世界坐标全都"正常"）。
//!
//! 现在这一段只有 [`EngineHost`] 一份：它**持有**时钟与共享核心，并**唯一**地定义
//! "一帧 = 规划固定步 → 把这份输入交给模拟 → 渲染并 present"。
//!
//! ## 留在这层之外的（平台外壳各自的事）
//!
//! | 外壳 | 仍然自己管 |
//! | --- | --- |
//! | `gpu_host`（浏览器） | rAF、两层几何 `fit_now`、DOM 输入采集与剧本、URL 开关、`data-rustlua-gpu` 状态属性、世界读数探针 |
//! | `host_runtime`（微信/引擎宿主） | JS 帧驱动、RLFI 批量解码入口、`tick=lua` 通道、pixel_sample 回读、定长 ABI 与 JSON 状态 |
//!
//! 判据很清楚：**"这份差异是不是宿主能力差异"**。rAF 与 `wx.requestAnimationFrame` 是；
//! "一帧该跑几步、步长多少"不是。
//!
//! ## 失败的分段命名
//!
//! 两个外壳的失败文案不同（浏览器写状态属性、微信要与 `probe:` 同一族具名），所以这里
//! **不**拼文案，只报 [`HostFrameStage`] + 原始原因，由外壳决定怎么命名。
//! 这是刻意的：把文案也统一进来，就得让两边都接受一个更差的说法。

use rustlua_frame_abi::FrameInput;
use rustlua_infra::frame_clock::{FrameClock, FramePlan};

use crate::engine_core::{CoreArgs, EngineCore, FrameOutput};
use crate::host_config::HostConfig;

/// 一帧失败发生在哪一段。外壳据此拼自己的具名错误（`host: present: …` / `scene-frame: …`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostFrameStage {
    /// 取当前表面纹理失败（`get_current_texture`）：这一帧**没有**呈现。
    Present,
    /// 渲染/取网格/推进模拟这条链失败（`EngineCore::frame` 的其它错误）。
    SceneFrame,
}

/// 一帧失败：哪一段 + 原始原因（未加外壳前缀）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostFrameError {
    pub stage: HostFrameStage,
    pub detail: String,
}

impl HostFrameError {
    /// 判定分段：`get_current_texture` 前缀是 [`EngineCore::frame`] 自己给的
    /// （见 `engine_core.rs` 的同名字符串），**只在这一处**解释它。
    fn classify(detail: String) -> Self {
        let stage = if detail.starts_with("get_current_texture") {
            HostFrameStage::Present
        } else {
            HostFrameStage::SceneFrame
        };
        Self { stage, detail }
    }
}

/// 一帧的产出：规划（固定步/夹取/累加器）+ 渲染结果。
///
/// `plan` 必须一起交出去，而不是只给 `out`：两侧的定长 ABI 与状态读数都要报
/// `steps` / `frame_index` / `accumulator`，那些值只有计划里有。
pub struct HostFrame {
    pub plan: FramePlan,
    pub out: FrameOutput,
}

/// 一帧里宿主**真正消费到**的那份输入的结构化读数。
///
/// 为什么要有它：删掉旧 JS 世界之后"输入还有没有进到宿主"是这条路上风险最高的一条——旧运行时
/// 不跑 ⇒ 宿主每帧拿 `FrameInput::default()` ⇒ **不崩溃、不报错、`presented` 照涨，只是人钉死**。
/// 而门要断言的是"某个数随事件变化"；从日志串里正则抠数字是最脆的做法。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameInputReadout {
    pub frame: u32,
    pub axis: [f64; 2],
    pub cursor: [f64; 2],
    /// 按下那一刻的位置（与 `cursor` 分开读，见 `InputSnapshot::press_cursor`）。
    pub press_cursor: [f64; 2],
    /// 每根手指的按下点（`[0..pointer_count]` 有效）。
    pub press_cursors: [[f64; 2]; rustlua_frame_abi::MAX_POINTERS],
    pub pointer_count: u8,
    pub held: u32,
    pub pressed: u32,
    pub mouse_held: u32,
}

impl FrameInputReadout {
    #[must_use]
    pub fn of(input: &FrameInput) -> Self {
        Self {
            frame: input.frame,
            axis: input.axis,
            cursor: input.cursor,
            press_cursor: input.press_cursor,
            press_cursors: input.press_cursors,
            pointer_count: input.pointer_count,
            held: input.keys_held.count_ones(),
            pressed: input.keys_pressed.count_ones(),
            mouse_held: input.mouse_held.count_ones(),
        }
    }

    /// 这一帧是否有输入活动（累计计数器的判据）。
    #[must_use]
    pub fn is_active(self) -> bool {
        self.held != 0 || self.mouse_held != 0 || self.axis != [0.0, 0.0]
    }

    /// 进状态读数的 `input` 块。`seen_frames` 是**累计**量：剧本窗口跑完就回到 0，
    /// 抓快照会把"输入进了宿主"读成"没进"（假阴性）。
    #[must_use]
    pub fn to_json(
        self,
        seen_frames: u64,
        press_distinct_frames: u64,
        multipoint_frames: u64,
    ) -> serde_json::Value {
        serde_json::json!({
            "frame": self.frame,
            "axis": [self.axis[0], self.axis[1]],
            "cursor": [self.cursor[0], self.cursor[1]],
            "press_cursor": [self.press_cursor[0], self.press_cursor[1]],
            "press_cursors": self.press_cursors,
            "pointer_count": self.pointer_count,
            "multipoint_frames": multipoint_frames,
            // **累计**"按下点与光标真的不同"的帧数：单帧采样会读到抬手之后的静止值（两者相等），
            // 把"按下点被记录了"读成"没记录"——与 `seen_frames` 同一条理由。
            "press_distinct_frames": press_distinct_frames,
            "held": self.held,
            "pressed": self.pressed,
            "mouse_held": self.mouse_held,
            "seen_frames": seen_frames,
        })
    }
}

/// 一帧的渲染侧事实（从 `FrameOutput` 抄下来的轻量副本）。
///
/// 为什么要在宿主里落账而不是让外壳攥着 `out` 现拼：状态读数是**帧外**调的
/// （`engine_host_state` 与 `data-rustlua-engine-host` 都不在帧路径上），两个外壳各自现拼
/// 就是"同一台宿主两套读数"的成因。
#[derive(Debug, Clone, Default)]
pub struct FrameFacts {
    pub builtin: bool,
    pub draws: u64,
    pub tris: u64,
    pub scene_note: String,
}

impl FrameFacts {
    #[must_use]
    pub fn of(out: &FrameOutput) -> Self {
        Self {
            builtin: out.builtin,
            draws: out.draw_calls,
            tris: out.triangles,
            scene_note: out.scene_note.clone(),
        }
    }
}

/// 两个平台共用的宿主：**持有**时钟与共享核心，并唯一地驱动一帧。
pub struct EngineHost {
    /// 共享核心（GPU 栈 + Renderer + `RustScene`）。
    ///
    /// 公开是刻意的：外壳要读它的读数（`sim_note` / `texture_stats` / 三分耗时），
    /// 也要在**观测时机**（不是热路径）经 `sim_mut` 刷世界读数——这是既有口径，
    /// 本片只搬动所有权，不改读取时机。**不要**绕过 [`EngineHost::frame`] 直接调它出帧：
    /// 时钟与渲染顺序的全部保证都在 `frame` 里。
    pub core: EngineCore,
    clock: FrameClock,
    /// 最近一帧宿主消费到的输入（状态读数的 `input` 块）。
    last_input: FrameInputReadout,
    /// 累计"这一帧真的有输入"的帧数。**累计**是刻意的（见 `FrameInputReadout::to_json`）。
    input_seen_frames: u64,
    /// 累计"按下点 ≠ 光标"的帧数（见 `FrameInputReadout::to_json`）。
    press_distinct_frames: u64,
    /// 累计"同时有两个以上指针"的帧数（多指的门用它）。
    multipoint_frames: u64,
    /// 最近一帧的渲染事实（状态读数用）。
    last_frame: FrameFacts,
    /// **取不到表面纹理**而失败的帧数。只有这一族计入：渲染链自身的失败是另一回事，
    /// 混在一起会让"设备丢了上下文"和"我们的场景坏了"在读数上无法区分。
    failed_presents: u64,
}

impl EngineHost {
    /// 先造时钟再建核心：**非法配置要在碰 GPU 之前就失败**。
    ///
    /// 反过来的话，一个写错的 `updates_per_second` 会先把 device/surface 建起来、
    /// 再去报错，现场看起来像"GPU 起不来"。
    pub async fn build(args: CoreArgs<'_>, config: HostConfig) -> Result<Self, String> {
        let clock = config
            .clock()
            .map_err(|error| format!("FrameClock::new: {error}"))?;
        let core = EngineCore::build(args).await?;
        Ok(Self {
            core,
            clock,
            last_input: FrameInputReadout::default(),
            input_seen_frames: 0,
            press_distinct_frames: 0,
            multipoint_frames: 0,
            last_frame: FrameFacts::default(),
            failed_presents: 0,
        })
    }

    /// 一帧的全部逻辑（**唯一实现**）：规划固定步 → 把 `input` 交给模拟与渲染。
    ///
    /// `raw_seconds` 是**墙钟间隔**（浏览器 = rAF 时间戳之差，微信 = JS 报的 `dt_ms / 1000`）。
    /// 夹取、容差、追帧上限全在 `FrameClock::plan` 里（`rustlua_infra::frame_clock`），
    /// 这一层不再重做任何一条策略。
    ///
    /// 失败的帧**不计** present，也**不**冒充一次成功：`EngineCore::frame` 在取纹理失败时
    /// 提前返回、并没有呈现；调用方按 [`HostFrameError::stage`] 分开记账，才能把
    /// "设备丢了上下文"与"我们的场景坏了"两件事分开读。
    pub fn frame(
        &mut self,
        raw_seconds: f64,
        input: &FrameInput,
    ) -> Result<HostFrame, HostFrameError> {
        let plan = self.clock.plan(raw_seconds);
        match self.core.frame(plan.steps, plan.delta_seconds, input) {
            Ok(out) => {
                // 在这里落账，而不是让外壳各自现拼读数（见 `FrameFacts` 的说明）。
                self.last_input = FrameInputReadout::of(input);
                if self.last_input.is_active() {
                    self.input_seen_frames += 1;
                }
                if self.last_input.pointer_count > 1 {
                    self.multipoint_frames += 1;
                }
                if self.last_input.press_cursor != self.last_input.cursor {
                    self.press_distinct_frames += 1;
                }
                self.last_frame = FrameFacts::of(&out);
                Ok(HostFrame { plan, out })
            }
            Err(detail) => {
                let error = HostFrameError::classify(detail);
                if error.stage == HostFrameStage::Present {
                    self.failed_presents += 1;
                }
                Err(error)
            }
        }
    }

    /// 时钟读数（`frame_index` / `total_steps` / `accumulator` / `clamped` 都从这里取）。
    #[must_use]
    pub fn clock(&self) -> &FrameClock {
        &self.clock
    }

    /// 丢掉半截墙钟时间。**暂停/恢复必须调**：否则隐藏期间攒下的累加器会在恢复的第一帧
    /// 换成一步模拟（表现是"回到前台时世界忽然跳了一下"）。
    pub fn reset_accumulator(&mut self) {
        self.clock.reset_accumulator();
    }

    /// 因取不到表面纹理而失败的帧数。
    #[must_use]
    pub fn failed_presents(&self) -> u64 {
        self.failed_presents
    }

    /// **两个外壳共用的** scene 档状态读数（`input` 块就是 N2 的原料）。
    ///
    /// 为什么必须有这一处：浏览器外壳此前自己拼了一份（`data-rustlua-engine-host`），微信外壳拼的
    /// 是另一份（`engine_host_state().scene`），而两份**已经开始漂**——同一个 `textures` 在一边是
    /// 数组 `[ok, fail]`、另一边是字符串 `"ok/fail"`。同一台宿主的同一件事有两套形状，正是本项目
    /// 反复吃过的"读数分叉"：两侧都看着正常，直到有人拿一边的字段去读另一边。
    ///
    /// `world` 与 `last_scene` 由外壳给（各自的世界观测 / 场景自报）；**空串表示"这个外壳这一
    /// 形态下没有这项读数"**——留空而不是省字段，是为了让门能断言"字段在不在"。
    #[must_use]
    pub fn scene_state_json(&self, world: &str, last_scene: &str) -> serde_json::Value {
        serde_json::json!({
            "mode": "scene",
            "sim": self.core.sim_note,
            "textures": [self.core.texture_stats.0, self.core.texture_stats.1],
            "texture_note": self.core.texture_note,
            "boot_ms": self.core.boot_ms,
            "first_frame_ms": self.core.first_frame_ms.is_finite().then_some(self.core.first_frame_ms),
            "world": world,
            "last": last_scene,
            "scene": self.last_frame.scene_note,
            "builtin": self.last_frame.builtin,
            "draws": self.last_frame.draws,
            "tris": self.last_frame.tris,
            "input": self.last_input.to_json(self.input_seen_frames, self.press_distinct_frames, self.multipoint_frames),
        })
    }
}
