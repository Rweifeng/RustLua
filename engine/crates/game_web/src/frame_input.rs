//! 批量帧输入（`RLFI` 线格式）→ 引擎输入快照（`InputSnapshot`）。
//!
//! ## 为什么要有这一层
//!
//! 两个宿主（WebGL / 微信小游戏）每帧拿到的是**线格式**（[`FrameInput`]，WP1 的批量 ABI），
//! 而引擎的模拟入口 `RuntimeScene::run_script_steps(steps, delta, &InputSnapshot)` 要的是引擎快照。
//! 此前两边各写一份：JS 补丁层那条链把线格式应用给 **Lua 表**（`lua_input_frame`），
//! 而 GPU 模拟那一侧**根本没有输入**——于是"画面里的角色会动、模拟里的不动"，
//! 而且这种分叉在读数上完全看不出来。
//!
//! 这一层把"线格式 → 引擎快照"固化成一个可测的函数：两条链吃同一份输入，模拟才谈得上是"一个"。
//!
//! ## 两条不变量（都由测试守着）
//!
//! 1. **快照跨帧保留，只改变化的那几位**。整份重建会把"一直按住"变成"每帧都刚按下"，
//!    于是脚本里的 `just_pressed` 分支每帧都触发——这类错法在画面上往往"看着还行"，很难回头查。
//! 2. **名字必须全部有归宿**。[`KEY_NAMES`] / [`MOUSE_NAMES`] 里任何一个名字映射不出引擎键位，
//!    都说明宿主与 wasm 的键表不同源——那正是"某个键按了没反应"的经典成因，宁可测试里响亮失败。
//!
//! ## 一处刻意的取舍
//!
//! 按住集合（`keys_held`）被当作**权威**：每帧把快照收敛到线上真值，边沿只在真的发生变化时才产生。
//! 线格式里的 `pressed`/`released` 只用来补一种情况——**同一帧内按下又抬起**的点按
//! （它不会出现在按住集合里）。这样即使宿主的边沿位漏了一帧，状态也不会永久错下去。

use rustlua_frame_abi::{FrameInput, KEY_NAMES, MOUSE_NAMES};
use rustlua_input::input::{InputSnapshot, KeyCode, MouseButton};

/// 本帧真正改动的输入项数量（键、鼠标按钮与摇杆轴）。
///
/// 读数用它回答"输入到底有没有落到模拟里"：恒为 0 说明宿主与 wasm 对"哪些位该变"理解不一致，
/// 而不是"玩家没操作"。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AppliedInput {
    /// 按住集合里**真实发生变化**的键数。
    pub key_changes: u32,
    /// 鼠标按钮的变化数。
    pub mouse_changes: u32,
    /// 同一帧内按下又抬起的点按次数（只存在于边沿位里，按住集合看不到）。
    pub same_frame_taps: u32,
    /// 摇杆轴值发生变化的轴数。
    pub axis_changes: u32,
}

impl AppliedInput {
    /// 是否改动过任何输入状态。**只给测试用**：让断言能一句话说清"按住不动不该产生变化"，
    /// 生产路径按字段累加读数（`rust_scene`），不需要这个聚合。
    #[cfg(test)]
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.key_changes == 0
            && self.mouse_changes == 0
            && self.same_frame_taps == 0
            && self.axis_changes == 0
    }
}

/// 键名 → 引擎键位。
///
/// **顺序与脚本侧 `parse_key_strict` 一致**（先物理名、再 v1 兼容名），这不是风格问题：
/// 两条链（Lua 表 / 引擎快照）必须对同一个名字给出**同一个键**，否则同一个按键在"脚本看到的输入"
/// 与"引擎模拟看到的输入"里会变成两个键。`"shift"`/`"control"`/`"alt"` 就是这条顺序救回来的
/// ——`v1_key_code` 不认识它们，而线格式的键表里有这三项（`KEY_NAMES`），只走 v1 会静默丢掉
/// 整个 Shift/Ctrl/Alt。
#[must_use]
pub fn key_for_name(name: &str) -> Option<KeyCode> {
    if let Some(key) = KeyCode::from_text(name) {
        return Some(key);
    }
    let mut notes = Vec::new();
    if let Some(key) = KeyCode::v1_key_code(name, &mut notes) {
        return Some(key);
    }
    // `KEY_NAMES` 里有 `"shift"` / `"control"` / `"alt"`，而引擎的键名是 `"shiftleft"` 这一族
    // （`to_text()` 的输出）——两边对不上，于是这两个修饰键此前是**静默失效**的：
    // 宿主按 `event.key` 送出 `"shift"`，线格式认它（在表里），引擎却解析不出键位。
    //
    // 线格式每个修饰键只有**一个**槽位，分不出左右，所以这里定到左键。
    // 这不是"随便挑一个"：等 ABI 下一版把左右拆成两个槽位时，**要改的就是这一处**。
    modifier_alias(name)
}

/// 修饰键的线格式名 → 引擎键位（见 [`key_for_name`] 里关于左右不分的说明）。
fn modifier_alias(name: &str) -> Option<KeyCode> {
    match name {
        "shift" => Some(KeyCode::ShiftLeft),
        "control" | "ctrl" => Some(KeyCode::ControlLeft),
        "alt" => Some(KeyCode::AltLeft),
        "super" | "meta" => Some(KeyCode::SuperLeft),
        _ => None,
    }
}

/// 鼠标按钮名 → 引擎按钮（[`MOUSE_NAMES`] 只有 left/right/middle）。
#[must_use]
pub fn mouse_for_name(name: &str) -> Option<MouseButton> {
    match name {
        "left" => Some(MouseButton::Left),
        "right" => Some(MouseButton::Right),
        "middle" => Some(MouseButton::Middle),
        _ => None,
    }
}

/// 把一帧线格式**应用**到快照上（快照由调用方跨帧保留）。
pub fn apply_frame(snapshot: &mut InputSnapshot, frame: &FrameInput) -> AppliedInput {
    let mut applied = AppliedInput::default();

    // ① 按住集合是权威：逐位与快照对照，只在不一致时产生边沿。
    for (slot, name) in KEY_NAMES.iter().enumerate() {
        let Some(key) = key_for_name(name) else {
            continue;
        };
        let held_on_wire = frame.keys_held & (1u128 << slot) != 0;
        if held_on_wire != snapshot.is_down(key) {
            snapshot.key_event(key, held_on_wire);
            applied.key_changes += 1;
        }
    }

    // ② 同一帧内的点按：按住集合里看不到，只能从两个边沿位同时出现来判断。
    for (slot, name) in KEY_NAMES.iter().enumerate() {
        let bit = 1u128 << slot;
        if frame.keys_pressed & bit == 0 || frame.keys_released & bit == 0 {
            continue;
        }
        let Some(key) = key_for_name(name) else {
            continue;
        };
        snapshot.key_event(key, true);
        snapshot.key_event(key, false);
        applied.same_frame_taps += 1;
    }

    for (slot, name) in MOUSE_NAMES.iter().enumerate() {
        let Some(button) = mouse_for_name(name) else {
            continue;
        };
        let held_on_wire = frame.mouse_held & (1u64 << slot) != 0;
        if held_on_wire != snapshot.mouse_is_down(button) {
            snapshot.mouse_event(button, held_on_wire);
            applied.mouse_changes += 1;
        }
    }

    // ③ 绝对量每帧覆盖：光标、滚轮、虚拟摇杆轴（0/1 = 左摇杆 X/Y，与设备层同序）。
    snapshot.cursor_moved(frame.cursor[0], frame.cursor[1]);
    // 按下位置：**这条线此前没接**——引擎侧连字段都没有，于是 web/微信只能靠补丁层自造一个同名
    // `input.press_position`，而 native 引擎上 `ui/virtual_pad.lua` 的 `pcall` 永远走回退分支
    // （退化成实时光标 ⇒"按住摇杆＋另一手点按钮"命中错位，不报错）。补上它两端才同义。
    snapshot.press_cursor_moved(frame.press_cursor[0], frame.press_cursor[1]);
    // 多指：线格式里的按下点数组。**旧版写者没有这一段**（`pointer_count = 0`）⇒ 空切片 ⇒
    // 主按下点照旧可用，旧路径不会因为新增能力而失灵。
    let points: Vec<(f64, f64)> = (0..usize::from(frame.pointer_count))
        .filter_map(|slot| frame.press_cursors.get(slot))
        .map(|point| (point[0], point[1]))
        .collect();
    snapshot.set_press_cursors(&points);
    if frame.scroll[0] != 0.0 || frame.scroll[1] != 0.0 {
        snapshot.scroll_event(frame.scroll[0] as f32, frame.scroll[1] as f32);
    }
    for (axis, value) in frame.axis.iter().enumerate() {
        if snapshot.gamepad_axis(axis as u8) != *value as f32 {
            applied.axis_changes += 1;
        }
    }
    snapshot.set_gamepad_axis(0, frame.axis[0] as f32);
    snapshot.set_gamepad_axis(1, frame.axis[1] as f32);

    applied
}

/// DOM `KeyboardEvent.key` → 线格式键名（**与 JS 宿主逐条同规则**）。
///
/// 为什么不能只做 `to_ascii_lowercase()` 了事：JS 宿主（`web-runtime.js`）在送进线格式前有两条
/// 改写——空格键的 `event.key` 是 `" "` 而要写成 `"space"`，方向键的 `event.key` 是 `"ArrowLeft"`
/// 而要写成 `"left"`。这两条不改写，宿主送出的名字就落不进 [`KEY_NAMES`]（表里是 `"space"`/`"left"`），
/// 于是**空格与全部方向键静默失效**——而这两组恰好是游戏最主要的操作键。
///
/// 返回值落在 [`KEY_NAMES`] 的静态项上，所以这条路上一次分配都不做。
/// 落不进表的键（`Dead`/`Process`/`Unidentified`/输入法组合）返回 `None`：调用方**显式计数丢弃**，
/// 不静默变成"按了没反应"。
#[must_use]
pub fn wire_key_name(dom_key: &str) -> Option<&'static str> {
    // **不能 trim**：空格键的 `event.key` 就是一个空格，trim 掉就变成空串，
    // 于是空格键静默失效。JS 宿主也没 trim（`String(event.key||'').toLowerCase()`），
    // 两边必须逐条同规则——这条是写测试时才发现的（第一版就是 trim 的）。
    let lower = dom_key.to_ascii_lowercase();
    let normalized = if lower == " " {
        "space"
    } else if let Some(rest) = lower.strip_prefix("arrow") {
        rest
    } else {
        lower.as_str()
    };
    KEY_NAMES
        .iter()
        .copied()
        .find(|candidate| candidate.eq_ignore_ascii_case(normalized))
}

/// DOM 事件累积器：把一个渲染帧内发生的 DOM 事件攒成**一份** [`FrameInput`]。
///
/// ## 为什么需要它（退休清单上的第一项）
///
/// 线格式这一层一直只有**消费者**：JS 世界（`web-runtime.js`）采集事件、编码成 `RLFI`、
/// 经 `lua_input_frame` 写进常驻表，GPU 宿主再读出来。也就是说**没有 JS 世界就没有输入**——
/// Rust 那条路能自己启动、自己跑脚本、自己出画面（WP6-03 已证），但玩家按不动它。
/// 这个类型就是那条路的**生产者**：DOM 监听器（wasm 侧薄薄一层）把事件丢进来，每帧取走一份线格式。
///
/// ## 两条不变量（都由测试守着）
///
/// 1. **边沿只对状态变化产生**。同一个键重复 `keydown`（浏览器会因按住而重复派发）不产生新的按下边沿；
///    没按过的 `keyup`（宿主可能补发）也不产生抬起边沿。边沿错法的症状是"点按变连发"或"松手还一直走"。
/// 2. **每帧取走即清空**。按下/抬起边沿、滚轮、光标位移都是"本帧量"，取走后必须归零，
///    否则一次点按会在之后每一帧重新触发。
#[derive(Debug, Default)]
pub struct FrameInputProducer {
    frame: u32,
    keys_held: u128,
    keys_pressed: u128,
    keys_released: u128,
    mouse_held: u64,
    mouse_just: u64,
    mouse_just_released: u64,
    cursor: [f64; 2],
    press_cursor: [f64; 2],
    scroll: [f64; 2],
    axis: [f64; 2],
    /// 自上次取帧以来是否收到过指针事件（决定合并时谁的光标是"新的"）。
    pointer_fresh: bool,
    /// 多指：`(id, 按下点, 当前位置)`，**按按下先后**排列。
    ///
    /// 为什么要有它：线格式里 `press_cursor` 只有一个槽，"两指同时按两个按钮"时第二指的按下点
    /// 无处可写（`single_press_point_is_the_pre_multitouch_degradation` 曾把那个退化钉在案上）。
    /// **顺序即语义**：`press_cursors[0..count]` 按按下先后排，最后一个就是旧字段 `press_cursor`。
    pointers: Vec<(u64, [f64; 2], [f64; 2])>,
    /// 落不进 [`KEY_NAMES`] 的键数。**要能被读到**：静默丢弃会让人以为引擎坏了。
    dropped_keys: u32,
}

impl FrameInputProducer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 一根手指按下（多指）。
    ///
    /// 与 [`Self::mouse_down`] 的关键区别：**它可以在别的指针已按下时再次调用**。后者在"键已按下"
    /// 时提前返回，于是第二根手指的按下点根本没处可写——那正是被钉住的那个退化。同一 `id` 重复
    /// 按下只更新按下点、不新增指针（真机上 `touchstart` 对同一指可能重复到达）。
    pub fn pointer_down(&mut self, id: u64, position: [f64; 2]) {
        self.cursor = position;
        self.press_cursor = position;
        self.pointer_fresh = true;
        if let Some(entry) = self.pointers.iter_mut().find(|(other, _, _)| *other == id) {
            entry.1 = position;
            entry.2 = position;
        } else {
            self.pointers.push((id, position, position));
        }
    }

    /// 某根手指移动。`id` 不存在时**忽略**——抬起之后的移动事件是常态，不该凭空造出一根手指。
    pub fn pointer_move(&mut self, id: u64, position: [f64; 2]) {
        // 光标跟着"最近动的那根"走：这是鼠标语义与触摸语义的共同点。
        self.cursor = position;
        self.pointer_fresh = true;
        if let Some(entry) = self.pointers.iter_mut().find(|(other, _, _)| *other == id) {
            entry.2 = position;
        }
    }

    /// 某根手指抬起。**不动 `press_cursor`**：与 `mouse_up` 的既有语义一致（按下点留到最后一次按下）。
    pub fn pointer_up(&mut self, id: u64) {
        self.pointers.retain(|(other, _, _)| *other != id);
    }

    /// 落不进键表的键数（读数用）。
    #[must_use]
    pub fn dropped_keys(&self) -> u32 {
        self.dropped_keys
    }

    /// 自上次取帧以来收到过指针事件。
    #[must_use]
    pub fn pointer_fresh(&self) -> bool {
        self.pointer_fresh
    }

    /// 键按下。返回 `false` 表示这个名字落不进线格式（已计数丢弃）。
    pub fn key_down(&mut self, dom_key: &str) -> bool {
        let Some(name) = wire_key_name(dom_key) else {
            self.dropped_keys = self.dropped_keys.saturating_add(1);
            return false;
        };
        let Some(slot) = rustlua_frame_abi::key_slot(name) else {
            self.dropped_keys = self.dropped_keys.saturating_add(1);
            return false;
        };
        let bit = 1u128 << slot;
        // 重复按下（浏览器按住会重复派发）不产生新边沿。
        if self.keys_held & bit == 0 {
            self.keys_pressed |= bit;
        }
        self.keys_held |= bit;
        true
    }

    /// 键抬起。没按过的抬起不产生边沿。
    pub fn key_up(&mut self, dom_key: &str) -> bool {
        let Some(name) = wire_key_name(dom_key) else {
            self.dropped_keys = self.dropped_keys.saturating_add(1);
            return false;
        };
        let Some(slot) = rustlua_frame_abi::key_slot(name) else {
            self.dropped_keys = self.dropped_keys.saturating_add(1);
            return false;
        };
        let bit = 1u128 << slot;
        if self.keys_held & bit != 0 {
            self.keys_released |= bit;
        }
        self.keys_held &= !bit;
        true
    }

    /// 鼠标/指针按钮按下（`"left"`/`"right"`/`"middle"`）。
    pub fn mouse_down(&mut self, button: &str) {
        let Some(slot) = rustlua_frame_abi::mouse_slot(button) else {
            return;
        };
        let bit = 1u64 << slot;
        if self.mouse_held & bit == 0 {
            self.mouse_just |= bit;
            self.press_cursor = self.cursor;
            // 鼠标也是**一根手指**：把主按下点登记进指针表。少了这一步，"鼠标按住 + 一根手指"
            // 会被读成"只有一根手指"——多指计数与逐指按下点都会少一项（浏览器门实测踩到过）。
            if !self
                .pointers
                .iter()
                .any(|(id, _, _)| *id == MOUSE_POINTER_ID)
            {
                self.pointers
                    .push((MOUSE_POINTER_ID, self.cursor, self.cursor));
            }
        }
        self.mouse_held |= bit;
    }

    /// 鼠标/指针按钮抬起。
    pub fn mouse_up(&mut self, button: &str) {
        let Some(slot) = rustlua_frame_abi::mouse_slot(button) else {
            return;
        };
        let bit = 1u64 << slot;
        if self.mouse_held & bit != 0 {
            self.mouse_just_released |= bit;
        }
        self.mouse_held &= !bit;
    }

    /// 指针移动到**设计坐标**（调用方负责把 CSS 像素换算成设计像素——那是几何的事，
    /// 换算错了与键位错了一样是"点哪儿都不对"，所以不在这里重做一遍）。
    pub fn pointer_moved(&mut self, design: [f64; 2]) {
        self.cursor = design;
        self.pointer_fresh = true;
    }

    /// 滚轮/触控板滚动增量（本帧量）。
    pub fn scrolled(&mut self, delta: [f64; 2]) {
        self.scroll[0] += delta[0];
        self.scroll[1] += delta[1];
    }

    /// 设置虚拟摇杆轴（0 = X，1 = Y），并限制到设备轴的有效范围。
    pub fn gamepad_axis(&mut self, axis: usize, value: f64) {
        if let Some(slot) = self.axis.get_mut(axis) {
            *slot = value.clamp(-1.0, 1.0);
        }
    }

    /// Release held controls after focus/visibility loss so key-up events missed in the background
    /// cannot leave the simulation stuck moving.
    pub fn release_all(&mut self) {
        self.keys_released |= self.keys_held;
        self.keys_held = 0;
        self.mouse_just_released |= self.mouse_held;
        self.mouse_held = 0;
        self.axis = [0.0; 2];
        self.scroll = [0.0; 2];
        self.pointer_fresh = false;
    }

    /// 取走本帧线格式，并清空本帧量（边沿、滚轮、光标新鲜度）。
    pub fn take_frame(&mut self, out: &mut FrameInput) {
        self.frame = self.frame.wrapping_add(1);
        // 光标位移由"本帧的光标"与"上一帧发出的光标"之差得到；`out` 里存的就是上一帧。
        let previous = out.cursor;
        out.frame = self.frame;
        out.axis = self.axis;
        out.cursor = self.cursor;
        out.press_cursor = self.press_cursor;
        out.cursor_delta = [self.cursor[0] - previous[0], self.cursor[1] - previous[1]];
        out.scroll = self.scroll;
        out.keys_held = self.keys_held;
        out.keys_pressed = self.keys_pressed;
        out.keys_released = self.keys_released;
        out.mouse_held = self.mouse_held;
        out.mouse_just = self.mouse_just;
        out.mouse_just_released = self.mouse_just_released;
        // 多指块：按**按下先后**写进定容数组。
        // 鼠标键全松 ⇒ 那根"手指"抬起。放在这里而不是 `mouse_up` 里：抬起是多键语义
        // （还有别的键按着时不算），而这里正好是"本帧鼠标按住状态的最终值"。
        if self.mouse_held == 0 {
            self.pointers.retain(|(id, _, _)| *id != MOUSE_POINTER_ID);
        }
        let mut count = self.pointers.len().min(rustlua_frame_abi::MAX_POINTERS);
        out.press_cursors = [[0.0, 0.0]; rustlua_frame_abi::MAX_POINTERS];
        for (index, (_, press, _)) in self.pointers.iter().take(count).enumerate() {
            out.press_cursors[index] = *press;
        }
        // **桌面鼠标也算一根手指**：它是"有按下点、但没走多指 API"的那条路。少了这一步，
        // 单指环境的 `pointer_count` 会是 0，Lua 侧按索引取按下点就会全都取不到。
        if count == 0 && self.mouse_held != 0 {
            out.press_cursors[0] = self.press_cursor;
            count = 1;
        }
        out.pointer_count = count as u8;
        // 旧字段与"最后一个指针的按下点"**同源**：这份冗余表示的锚，测试钉住它。
        if count > 0 {
            out.press_cursor = out.press_cursors[count - 1];
        }

        self.keys_pressed = 0;
        self.keys_released = 0;
        self.mouse_just = 0;
        self.mouse_just_released = 0;
        self.scroll = [0.0, 0.0];
        self.pointer_fresh = false;
    }
}

/// 确定性输入剧本：`<键名|axis0±|axis1±>:<起帧>-<止帧>`，多段用逗号分隔。
///
/// **为什么需要它**：验证"输入 → 世界改变"这条链，用真人/探针按键必然带时序抖动
/// （探针按键的帧号取决于事件循环与无头环境的调度）。而"按住 w 从第 30 帧到第 150 帧"
/// 是**可复现**的：同一份产物、同一个 URL，跑到第 N 帧时世界必然处在同一个状态。
/// 于是它可以当门禁用，也可以用来做性能基线（固定输入 ⇒ 固定负载）。
///
/// 它与 DOM 采集**不是二选一**：DOM 那条路证明"事件能进来"（第 24 轮），
/// 这条证明"输入进去之后世界真的会变"（本轮的缺口）。两条一起才闭合。
/// 两份 `MAX_POINTERS` 必须一致：`rustlua-input` 不依赖 ABI crate，常量只能是两份。
/// 漂开的症状是"按下点按一个上限写、按另一个上限读"——高位手指**静默丢失**，
/// 所以在这里用编译期断言钉住，而不是靠注释提醒。
const _: () = assert!(
    rustlua_input::input::MAX_POINTERS == rustlua_frame_abi::MAX_POINTERS,
    "rustlua-input 与 rustlua-frame-abi 的 MAX_POINTERS 必须一致"
);

#[derive(Debug, Clone, Default, PartialEq)]
pub struct InputScript {
    windows: Vec<(String, u64, u64)>,
    axis_windows: Vec<(usize, i8, u64, u64)>,
    /// `at:x,y:from-to`：指针这段区间里的位置（**设计坐标**）。
    pointer_windows: Vec<(u64, f64, f64, u64, u64)>,
    /// `hold:left:from-to`：这段区间里按住某个鼠标键。
    press_windows: Vec<(u64, String, u64, u64)>,
}

/// 解析 `from-to` 片段（三类指令共用），一处说清错误而不是各自"猜"。
/// 鼠标占用的指针槽。用 `u64::MAX` 而不是 0/1：DOM 的 `pointerId` 与微信的 `identifier` 都从 0/1
/// 起算，占一个"正常不可能出现"的值才不会和真手指撞号。
const MOUSE_POINTER_ID: u64 = u64::MAX;

/// 解析 `at`/`hold` 后的可选指针号：空 = 1（旧行为），否则必须落在 1..=MAX_POINTERS。
///
/// 上限在这里就卡住，不留给下游截断：脚本里写了 5 号手指却没进数组，症状是"那条断言永远不亮"，
/// 查起来比当场报错贵得多。
fn parse_pointer_id(id_part: &str, part: &str) -> Result<u64, String> {
    let trimmed = id_part.trim();
    if trimmed.is_empty() {
        return Ok(1);
    }
    let id: u64 = trimmed
        .parse()
        .map_err(|_| format!("指针号不是数字：{part}"))?;
    if id < 1 || id > rustlua_frame_abi::MAX_POINTERS as u64 {
        return Err(format!(
            "指针号必须在 1..={}：{part}",
            rustlua_frame_abi::MAX_POINTERS
        ));
    }
    Ok(id)
}

fn parse_script_range(range: &str, part: &str) -> Result<(u64, u64), String> {
    let (from, to) = range
        .split_once('-')
        .ok_or_else(|| format!("输入剧本片段缺少 '-' 区间：{part}"))?;
    let from: u64 = from
        .trim()
        .parse()
        .map_err(|_| format!("输入剧本起帧不是数字：{part}"))?;
    let to: u64 = to
        .trim()
        .parse()
        .map_err(|_| format!("输入剧本止帧不是数字：{part}"))?;
    if to <= from {
        return Err(format!("输入剧本区间必须止帧大于起帧：{part}"));
    }
    Ok((from, to))
}

impl InputScript {
    /// 解析 `w:30-150,axis1-:10-90` 这类剧本。空串 = 没有剧本（正常路径一行不改）。
    ///
    /// 严格报错而不是"猜"：帧号写反、缺冒号、键名不认识，都在这里一次说清。
    /// 静默丢掉一段剧本，现象是"按了没反应"——正是本轮要消灭的那类失败。
    pub fn parse(spec: &str) -> Result<Self, String> {
        let mut windows = Vec::new();
        let mut axis_windows = Vec::new();
        let mut pointer_windows = Vec::new();
        let mut press_windows = Vec::new();
        for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            // 指针/按键两类指令先分流：它们的"名字"后面还有一段（坐标或按钮名），
            // 不能走下面的 `名:区间` 泛型切分。
            // `at`/`hold` 后可跟可选指针号（`at2:` / `hold2:left:`）；不写 = 1，旧行为不变。
            // 只在 rest 以 ':' 或数字开头时进这个分支，否则一个恰好以 "at" 开头的键名会被误吞。
            if let Some(rest) = part.strip_prefix("at").filter(|rest| {
                rest.starts_with(':') || rest.starts_with(|c: char| c.is_ascii_digit())
            }) {
                let (id_part, rest) = rest
                    .split_once(':')
                    .ok_or_else(|| format!("指针指令缺 ':'：{part}"))?;
                let id = parse_pointer_id(id_part, part)?;
                let (coords, range) = rest
                    .split_once(':')
                    .ok_or_else(|| format!("指针指令缺坐标/区间分隔 ':'：{part}"))?;
                let (x, y) = coords
                    // **不能用逗号**：`,` 是片段分隔符，`at:100,200:...` 会被切成两个片段。
                    .split_once('x')
                    .ok_or_else(|| format!("指针坐标应是 xXy（如 100x200）：{part}"))?;
                let x: f64 = x
                    .trim()
                    .parse()
                    .map_err(|_| format!("指针 x 不是数字：{part}"))?;
                let y: f64 = y
                    .trim()
                    .parse()
                    .map_err(|_| format!("指针 y 不是数字：{part}"))?;
                let (from, to) = parse_script_range(range, part)?;
                pointer_windows.push((id, x, y, from, to));
                continue;
            }
            if let Some(rest) = part.strip_prefix("hold").filter(|rest| {
                rest.starts_with(':') || rest.starts_with(|c: char| c.is_ascii_digit())
            }) {
                let (id_part, rest) = rest
                    .split_once(':')
                    .ok_or_else(|| format!("按键指令缺 ':'：{part}"))?;
                let id = parse_pointer_id(id_part, part)?;
                let (button, range) = rest
                    .split_once(':')
                    .ok_or_else(|| format!("按键指令缺按钮/区间分隔 ':'：{part}"))?;
                let (from, to) = parse_script_range(range, part)?;
                press_windows.push((id, button.trim().to_owned(), from, to));
                continue;
            }
            let (name, range) = part
                .split_once(':')
                .ok_or_else(|| format!("输入剧本片段缺少 ':'：{part}"))?;
            let name = name.trim();
            let axis_binding = match name.strip_prefix("axis") {
                Some(binding) => {
                    let mut chars = binding.chars();
                    let axis = chars
                        .next()
                        .and_then(|digit| digit.to_digit(10))
                        .map(|digit| digit as usize)
                        .ok_or_else(|| {
                            format!("输入剧本摇杆轴格式应为 axis0+/axis0- 或 axis1+/axis1-：{name}")
                        })?;
                    let sign = chars.as_str();
                    if axis > 1 {
                        return Err(format!("输入剧本摇杆轴编号只能是 0 或 1：{name}"));
                    }
                    let value = match sign {
                        "+" => 1,
                        "-" => -1,
                        _ => {
                            return Err(format!("输入剧本摇杆轴方向只能是 + 或 -：{name}"));
                        }
                    };
                    Some((axis, value))
                }
                None => {
                    if wire_key_name(name).is_none() {
                        return Err(format!("输入剧本里的键名不在线格式键表里：{name}"));
                    }
                    None
                }
            };
            let (from, to) = range
                .split_once('-')
                .ok_or_else(|| format!("输入剧本片段缺少 '-' 区间：{part}"))?;
            let from: u64 = from
                .trim()
                .parse()
                .map_err(|_| format!("输入剧本起帧不是数字：{part}"))?;
            let to: u64 = to
                .trim()
                .parse()
                .map_err(|_| format!("输入剧本止帧不是数字：{part}"))?;
            if to <= from {
                return Err(format!("输入剧本区间必须止帧大于起帧：{part}"));
            }
            if let Some((axis, value)) = axis_binding {
                if axis_windows
                    .iter()
                    .any(|(other_axis, other_value, other_from, other_to)| {
                        *other_axis == axis
                            && *other_value != value
                            && from < *other_to
                            && *other_from < to
                    })
                {
                    return Err(format!(
                        "输入剧本摇杆轴不能在重叠区间内同时驱动正反方向：{part}"
                    ));
                }
                axis_windows.push((axis, value, from, to));
            } else {
                windows.push((name.to_owned(), from, to));
            }
        }
        Ok(Self {
            windows,
            axis_windows,
            pointer_windows,
            press_windows,
        })
    }

    /// 这份剧本是否为空（空剧本不该改变任何行为）。
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
            && self.axis_windows.is_empty()
            && self.pointer_windows.is_empty()
            && self.press_windows.is_empty()
    }

    /// 第 `frame` 帧**应当按住**的键集合（按剧本里的顺序，去重交给调用方）。
    pub fn held_at(&self, frame: u64) -> Vec<&str> {
        self.windows
            .iter()
            .filter(|(_, from, to)| frame >= *from && frame < *to)
            .map(|(name, _, _)| name.as_str())
            .collect()
    }
}

/// 把 [`InputScript`] 逐帧喂给生产者：**按集合差**发边沿，而不是按行发。
///
/// 为什么要按集合差：剧本允许区间重叠或首尾相接（`w:30-150,a:150-180`），
/// 逐行发会在重叠处产生"松一下再按住"，模拟里就多出一次按下边沿——那是剧本作者没写的东西。
#[derive(Debug, Default)]
pub struct ScriptDriver {
    script: InputScript,
    held: Vec<String>,
    frame: u64,
    /// `hold:` 是否处于按住态：边沿必须跨帧记住，否则每帧都发一次 `mouse_down`
    /// （按下点会被反复刷新成"当前光标"，按下位置就丢了）。
    pressed: bool,
    /// 多指（id ≥ 2）上一帧的状态：(id, 位置, 是否按住)，用来算按下/抬起边沿。
    extra: Vec<(u64, [f64; 2], bool)>,
}

impl ScriptDriver {
    pub fn new(script: InputScript) -> Self {
        Self {
            script,
            held: Vec::new(),
            frame: 0,
            pressed: false,
            extra: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.script.is_empty()
    }

    /// 推进到下一帧：先把该松的松掉，再把该按的按上。
    ///
    /// 松在前：同一帧内"换键"（w 松、a 按）会让模拟看到两个独立边沿，
    /// 顺序固定才能逐帧复现。
    pub fn tick(&mut self, producer: &mut FrameInputProducer) {
        self.frame = self.frame.wrapping_add(1);
        // **指针先落位，再判按下**：`mouse_down` 把*当时的*光标记成按下点，顺序反了按下点
        // 就会是上一帧的位置（症状是"命中差一帧"，几乎看不出来）。
        let mut pointer: Option<[f64; 2]> = None;
        for (id, x, y, from, to) in &self.script.pointer_windows {
            // id 1 = 主指针，走下面那条**旧路**（光标 / mouse_* / press_cursor 的语义逐字不变）。
            if *id != 1 {
                continue;
            }
            if self.frame >= *from && self.frame < *to {
                pointer = Some([*x, *y]);
            }
        }
        if let Some(at) = pointer {
            producer.pointer_moved(at);
        }
        let want_press = self
            .script
            .press_windows
            .iter()
            .any(|(id, button, from, to)| {
                *id == 1 && button == "left" && self.frame >= *from && self.frame < *to
            });
        if want_press && !self.pressed {
            producer.mouse_down("left");
            self.pressed = true;
        } else if !want_press && self.pressed {
            producer.mouse_up("left");
            self.pressed = false;
        }
        // **多指（id ≥ 2）**：每个 id 自己维护按下/移动/抬起的边沿。
        //
        // 为什么与上面那条分开而不是合并：上面那条的语义被现有门依赖（mouse_* 位、press_cursor、
        // pointer_fresh），合并会牵动一串既有读数；这条只负责第二根手指的按下点，可独立加、独立验。
        let mut wanted: Vec<(u64, [f64; 2], bool)> = Vec::new();
        for (id, x, y, from, to) in &self.script.pointer_windows {
            if *id < 2 || self.frame < *from || self.frame >= *to {
                continue;
            }
            let held = self
                .script
                .press_windows
                .iter()
                .any(|(bid, _, bfrom, bto)| {
                    *bid == *id && self.frame >= *bfrom && self.frame < *bto
                });
            if let Some(entry) = wanted.iter_mut().find(|(wid, _, _)| *wid == *id) {
                entry.1 = [*x, *y];
                entry.2 = held;
            } else {
                wanted.push((*id, [*x, *y], held));
            }
        }
        // 仍按住但本帧没有位置窗口的 id：沿用上一帧位置（真机上手指可以停在原地）。
        for (id, position, held) in &self.extra {
            if *held && !wanted.iter().any(|(wid, _, _)| wid == id) {
                wanted.push((*id, *position, true));
            }
        }
        for (id, position, held) in &wanted {
            let was_held = self
                .extra
                .iter()
                .find(|(eid, _, _)| eid == id)
                .is_some_and(|(_, _, held)| *held);
            match (was_held, *held) {
                (false, true) => producer.pointer_down(*id, *position),
                (true, true) => producer.pointer_move(*id, *position),
                (true, false) => producer.pointer_up(*id),
                (false, false) => {}
            }
        }
        self.extra = wanted;
        let want: Vec<&str> = self.script.held_at(self.frame);
        for name in &self.held {
            if !want.contains(&name.as_str()) {
                producer.key_up(name);
            }
        }
        for name in &want {
            if !self.held.iter().any(|held| held == name) {
                producer.key_down(name);
            }
        }
        self.held = want.into_iter().map(str::to_owned).collect();
        let mut axes = [0.0; 2];
        for (axis, value, from, to) in &self.script.axis_windows {
            if self.frame >= *from && self.frame < *to {
                axes[*axis] = f64::from(*value);
            }
        }
        producer.gamepad_axis(0, axes[0]);
        producer.gamepad_axis(1, axes[1]);
    }
}

/// 把另一路宿主（JS 世界的 `RLFI`）的输入**并进**本帧。
///
/// 为什么是"并"而不是"选一个"：两条来源都合法——浏览器里 Rust 侧直接从 DOM 采集，
/// 而 JS 世界还会送来它自己那套 UI 的产物（屏幕虚拟摇杆的轴、补丁层的虚拟键）。
/// 丢掉任何一边都会让某一种操作方式失灵，且症状（"手柄/摇杆没反应"）离原因很远。
/// 位图的槽位语义两边同源于 [`KEY_NAMES`]，所以按位或就是正确的并集。
///
/// 光标与滚轮是**绝对量/增量**，不能按位并：光标取"本帧真的动过"的那一路
/// （`js_cursor_wins`；本侧动过就传 `false`），滚轮取两者之和。
pub fn merge_into(dst: &mut FrameInput, src: &FrameInput, js_cursor_wins: bool) {
    dst.keys_held |= src.keys_held;
    dst.keys_pressed |= src.keys_pressed;
    dst.keys_released |= src.keys_released;
    dst.mouse_held |= src.mouse_held;
    dst.mouse_just |= src.mouse_just;
    dst.mouse_just_released |= src.mouse_just_released;
    dst.scroll[0] += src.scroll[0];
    dst.scroll[1] += src.scroll[1];
    // 轴：本侧没有虚拟摇杆，所以有值就以 JS 侧为准（0 是"没推"，不是"推回中位"之外的语义）。
    if src.axis[0] != 0.0 || src.axis[1] != 0.0 {
        dst.axis = src.axis;
    }
    if js_cursor_wins {
        dst.cursor = src.cursor;
        dst.press_cursor = src.press_cursor;
    } else if dst.cursor[0] == 0.0 && dst.cursor[1] == 0.0 {
        // 本侧从没收到指针事件时，别把 JS 侧算好的光标抹成 (0,0)。
        dst.cursor = src.cursor;
        dst.press_cursor = src.press_cursor;
    }
    if src.cursor_delta != [0.0, 0.0] {
        dst.cursor_delta = [
            dst.cursor_delta[0] + src.cursor_delta[0],
            dst.cursor_delta[1] + src.cursor_delta[1],
        ];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_frame_abi::{KEY_SLOTS, MOUSE_SLOTS, key_slot};

    /// 契约：线格式里的**每个**键名都要能映射到引擎键位。
    ///
    /// 这条测试是"宿主与 wasm 键表同源"的守卫：任何一侧加了名字而另一侧没跟上，这里就红。
    #[test]
    fn every_wire_key_name_maps_to_an_engine_key() {
        let missing: Vec<&str> = KEY_NAMES
            .iter()
            .copied()
            .filter(|name| key_for_name(name).is_none())
            .collect();
        assert!(missing.is_empty(), "这些键名映射不出引擎键位：{missing:?}");
        assert!(
            KEY_NAMES.len() <= KEY_SLOTS,
            "名字表比位表还长（{} > {KEY_SLOTS}）",
            KEY_NAMES.len()
        );
    }

    /// 契约：鼠标名同理（表只有三个，但新增时必须有归宿）。
    #[test]
    fn every_wire_mouse_name_maps_to_an_engine_button() {
        assert!(MOUSE_NAMES.len() <= MOUSE_SLOTS);
        for name in MOUSE_NAMES {
            assert!(mouse_for_name(name).is_some(), "鼠标名没有归宿：{name}");
        }
    }

    /// **核心不变量**：按住不动的一帧不能产生 `just_pressed` 边沿。
    ///
    /// 这正是"整份重建快照"会踩的坑：脚本里的 `just_pressed` 分支每帧都会触发。
    #[test]
    fn holding_a_key_produces_no_repeated_press_edge() {
        let key = key_for_name("w").expect("w");
        let slot = key_slot("w").expect("w slot");
        let mut snapshot = InputSnapshot::default();

        let pressed = FrameInput {
            keys_held: 1u128 << slot,
            keys_pressed: 1u128 << slot,
            ..FrameInput::new()
        };
        let applied = apply_frame(&mut snapshot, &pressed);
        assert_eq!(applied.key_changes, 1, "第一次按下应当记一次变化");
        assert!(snapshot.is_down(key));
        assert!(snapshot.just_pressed(key));
        snapshot.finish_frame();

        // 第二帧：线上仍然按住，但没有新的边沿位。
        let held = FrameInput {
            keys_held: 1u128 << slot,
            ..FrameInput::new()
        };
        let applied = apply_frame(&mut snapshot, &held);
        assert!(applied.is_empty(), "按住不动不该产生任何变化：{applied:?}");
        assert!(snapshot.is_down(key), "按住状态必须保持");
        assert!(
            !snapshot.just_pressed(key),
            "按住不动又报了一次刚按下——脚本的边沿分支会每帧触发"
        );

        snapshot.finish_frame();
        // 第三帧：抬起。
        let released = FrameInput {
            keys_released: 1u128 << slot,
            ..FrameInput::new()
        };
        let applied = apply_frame(&mut snapshot, &released);
        assert_eq!(applied.key_changes, 1);
        assert!(!snapshot.is_down(key));
        assert!(snapshot.just_released(key));
    }

    /// 一次按键跨 0 步帧必须**还活着**：边沿只在跳变上产生，0 步帧里没有 `update()` 消费它。
    ///
    /// 宿主侧的规则（`rust_scene::step` / `game_app` / 编辑器预览三处同一个守卫）：
    /// **只有真跑了固定步的帧才 `finish_frame()`**。浏览器的 0 步帧是常态（rAF 快于固定步、
    /// 或追帧被 `FrameClock` 夹住；实测 `frames=240` 对 `sim_steps=152` ⇒ 88 帧一步不跑），
    /// 无条件清边沿会把落在这种帧里的确认**永久吞掉**——症状是"标题界面按空格有时不响应，
    /// 而且把按键窗口拉长也没用"（丢的只是**下沿那一次**，之后一直是按住状态）。
    /// 这里钉的是这条链的下半场：`apply_frame` 产生的边沿在没有 `finish_frame()` 的那一帧之后仍在。
    #[test]
    fn a_press_edge_survives_a_frame_without_fixed_steps() {
        let key = key_for_name("space").expect("space");
        let slot = key_slot("space").expect("space slot");
        let mut snapshot = InputSnapshot::default();

        // 帧 A：按下，但本帧没有固定步 ⇒ 宿主不调 `finish_frame()`（Lua 一次都没跑）。
        let pressed = FrameInput {
            keys_held: 1u128 << slot,
            keys_pressed: 1u128 << slot,
            ..FrameInput::new()
        };
        assert_eq!(apply_frame(&mut snapshot, &pressed).key_changes, 1);
        assert!(snapshot.just_pressed(key));

        // 帧 B：仍然按住、没有新边沿，本帧才跑固定步 ⇒ 脚本必须看到"刚按下"。
        let held = FrameInput {
            keys_held: 1u128 << slot,
            ..FrameInput::new()
        };
        assert!(
            apply_frame(&mut snapshot, &held).is_empty(),
            "按住不动不该补发边沿——那一下只能靠帧 A 留下的状态"
        );
        assert!(
            snapshot.just_pressed(key),
            "0 步帧清掉了边沿：这一次按下永远不会被 update() 看到"
        );

        // 消费之后照旧清干净：按住不能再变成每帧都刚按下。
        snapshot.finish_frame();
        assert!(!snapshot.just_pressed(key));
        assert!(snapshot.is_down(key));
    }

    /// 同一帧内按下又抬起：按住集合看不到，必须由两个边沿位还原成一次点按。
    #[test]
    fn same_frame_tap_becomes_press_and_release() {
        let key = key_for_name("space").expect("space");
        let slot = key_slot("space").expect("space slot");
        let mut snapshot = InputSnapshot::default();
        let tap = FrameInput {
            keys_pressed: 1u128 << slot,
            keys_released: 1u128 << slot,
            ..FrameInput::new()
        };
        let applied = apply_frame(&mut snapshot, &tap);
        assert_eq!(applied.same_frame_taps, 1);
        assert!(snapshot.just_pressed(key), "点按要留下按下边沿");
        assert!(snapshot.just_released(key), "点按要留下抬起边沿");
        assert!(!snapshot.is_down(key), "点按结束后不能还是按住");
    }

    /// 鼠标与绝对量：按住/边沿 + 光标 + 滚轮 + 摇杆轴都能落到快照上。
    #[test]
    fn mouse_cursor_scroll_and_axis_land_on_the_snapshot() {
        let mut snapshot = InputSnapshot::default();
        let frame = FrameInput {
            mouse_held: 1,
            mouse_just: 1,
            cursor: [120.0, 34.0],
            scroll: [0.0, 2.0],
            axis: [-0.5, 0.25],
            ..FrameInput::new()
        };
        let applied = apply_frame(&mut snapshot, &frame);
        assert_eq!(applied.mouse_changes, 1);
        assert!(snapshot.mouse_is_down(MouseButton::Left));
        assert!(snapshot.mouse_just_pressed(MouseButton::Left));
        assert_eq!(snapshot.gamepad_axis(0), -0.5);
        assert_eq!(snapshot.gamepad_axis(1), 0.25);
        // 光标位移**没有公开读取口**（`cursor_delta` 是私有字段，只有脚本侧 `input.cursor()` 看得到），
        // 所以这里只钉"绝对量每帧都写进去"这件事本身：把光标挪走再应用一帧，第二次不能报"有变化"。
        snapshot.finish_frame();
        let moved = FrameInput {
            // 鼠标仍然按住：真实宿主会**每帧**都送按住集合（线格式的约定），
            // 所以这里保持按住，才能验"没有变化就不该报变化"。
            mouse_held: 1,
            cursor: [150.0, 34.0],
            axis: [-0.5, 0.25],
            ..FrameInput::new()
        };
        let again = apply_frame(&mut snapshot, &moved);
        assert!(
            again.is_empty(),
            "光标移动（或按住不变）不该被算成键/鼠标变化：{again:?}"
        );
    }

    /// 归一规则必须与 JS 宿主逐条一致——不一致就是"某几个键在 Rust 路上没反应"。
    ///
    /// 三条改写各对应一类真实故障：空格、方向键、大小写。
    #[test]
    fn dom_key_names_normalize_exactly_like_the_js_host() {
        // 大小写：DOM 在按住 Shift 时给 "W"，JS 侧也是先 toLowerCase。
        assert_eq!(wire_key_name("W"), Some("w"));
        assert_eq!(wire_key_name("w"), Some("w"));
        // 空格：DOM 给 " "，线格式表里是 "space"。
        assert_eq!(wire_key_name(" "), Some("space"));
        // 方向键：DOM 给 "ArrowLeft"，线格式表里是 "left"。
        assert_eq!(wire_key_name("ArrowLeft"), Some("left"));
        assert_eq!(wire_key_name("ArrowUp"), Some("up"));
        // 数字与功能键：原样（只在表里查得到）。
        assert_eq!(wire_key_name("0"), Some("0"));
        assert_eq!(wire_key_name("F5"), Some("f5"));
        // 修饰键：DOM 给 "Shift"，表里有 "shift"（`key_for_name` 再把它定到左键）。
        assert_eq!(wire_key_name("Shift"), Some("shift"));
        // 落不进表的：显式 None，由调用方计数丢弃，不静默变成"按了没反应"。
        assert_eq!(wire_key_name("Dead"), None);
        assert_eq!(wire_key_name("Unidentified"), None);
        assert_eq!(wire_key_name(""), None);
    }

    /// 生产者的核心不变量：按住不动只产生**一次**按下边沿（浏览器按住会重复派发 `keydown`）。
    #[test]
    fn producer_emits_one_press_edge_for_a_held_key() {
        let mut producer = FrameInputProducer::new();
        let mut frame = FrameInput::new();
        let slot = key_slot("w").expect("w slot");
        let bit = 1u128 << slot;

        assert!(producer.key_down("w"));
        producer.take_frame(&mut frame);
        assert_eq!(frame.keys_pressed, bit, "第一帧应有按下边沿");
        assert_eq!(frame.keys_held, bit);

        // 浏览器因按住而重复派发 keydown，再取两帧都不该有新边沿。
        producer.key_down("w");
        producer.take_frame(&mut frame);
        assert_eq!(frame.keys_pressed, 0, "按住不动不该再产生按下边沿");
        assert_eq!(frame.keys_released, 0);
        assert_eq!(frame.keys_held, bit, "按住集合要跨帧保留");

        producer.key_up("w");
        producer.take_frame(&mut frame);
        assert_eq!(frame.keys_released, bit);
        assert_eq!(frame.keys_held, 0);
        // 没按过的抬起（宿主可能补发）不该产生边沿。
        producer.key_up("w");
        producer.take_frame(&mut frame);
        assert_eq!(frame.keys_released, 0);
    }

    /// 端到端：生产者的"同帧点按"要能被 `apply_frame` 认出来。
    ///
    /// 同一帧内按下又抬起**不会**出现在按住集合里，只存在于两个边沿位同时为真——
    /// 这条链（生产者 → 线格式 → `apply_frame` → 快照）正是最容易漏掉点按的地方。
    #[test]
    fn producer_same_frame_tap_reaches_the_snapshot() {
        let mut producer = FrameInputProducer::new();
        let mut frame = FrameInput::new();
        producer.key_down(" ");
        producer.key_up(" ");
        producer.take_frame(&mut frame);

        assert_eq!(frame.keys_held, 0, "点按后按住集合应为空");
        assert_eq!(frame.keys_pressed, frame.keys_released);
        assert_ne!(frame.keys_pressed, 0);

        let mut snapshot = InputSnapshot::default();
        let applied = apply_frame(&mut snapshot, &frame);
        assert_eq!(applied.same_frame_taps, 1, "同帧点按要被认出来");
        assert_eq!(applied.key_changes, 0, "按住集合没变，不该报变化");
    }

    #[test]
    fn producer_releases_controls_on_lifecycle_loss() {
        let mut producer = FrameInputProducer::new();
        assert!(producer.key_down("w"));
        producer.mouse_down("left");
        producer.gamepad_axis(0, 0.75);
        producer.release_all();

        let mut frame = FrameInput::new();
        producer.take_frame(&mut frame);
        assert_eq!(frame.keys_held, 0);
        assert_ne!(frame.keys_released, 0);
        assert_eq!(frame.mouse_held, 0);
        assert_ne!(frame.mouse_just_released, 0);
        assert_eq!(frame.axis, [0.0, 0.0]);
    }

    /// 本帧量取走即清空：边沿、滚轮、光标新鲜度都不能留到下一帧。
    #[test]
    fn take_frame_clears_per_frame_quantities() {
        let mut producer = FrameInputProducer::new();
        let mut frame = FrameInput::new();
        producer.key_down("a");
        producer.pointer_moved([10.0, 20.0]);
        producer.scrolled([0.0, 3.0]);
        producer.mouse_down("left");
        producer.take_frame(&mut frame);
        assert_eq!(frame.cursor, [10.0, 20.0]);
        assert_eq!(frame.press_cursor, [10.0, 20.0]);
        assert_eq!(frame.scroll, [0.0, 3.0]);
        assert_ne!(frame.mouse_just, 0);
        assert_eq!(frame.frame, 1);

        // 第二帧：没有新事件，只剩"按住"。
        producer.take_frame(&mut frame);
        assert_eq!(frame.scroll, [0.0, 0.0], "滚轮是本帧量");
        assert_eq!(frame.keys_pressed, 0);
        assert_eq!(frame.mouse_just, 0);
        assert_eq!(frame.frame, 2, "帧序号自增");
        assert_eq!(
            frame.keys_held,
            1u128 << key_slot("a").expect("a slot"),
            "按住集合不因取帧而清空"
        );
        assert_eq!(frame.cursor_delta, [0.0, 0.0], "光标没动就没有位移");
    }

    /// 落不进键表的键要被**计数**，而不是静默丢弃。
    #[test]
    fn producer_counts_keys_it_cannot_encode() {
        let mut producer = FrameInputProducer::new();
        assert!(!producer.key_down("Dead"));
        assert!(!producer.key_up("Process"));
        assert_eq!(producer.dropped_keys(), 2);
        assert!(producer.key_down("w"));
        assert_eq!(producer.dropped_keys(), 2, "能编码的键不该增加丢弃计数");
    }

    /// 合并：两路输入的按键是**并集**，滚动量相加，光标由"本帧动过"的那一路说了算。
    #[test]
    fn merging_two_sources_unions_keys_and_adds_scroll() {
        let mut dst = FrameInput {
            keys_held: 1,
            keys_pressed: 1,
            scroll: [0.0, 1.0],
            cursor: [0.0, 0.0],
            ..FrameInput::new()
        };
        let src = FrameInput {
            keys_held: 2,
            keys_released: 2,
            mouse_held: 4,
            scroll: [1.0, 0.0],
            cursor: [640.0, 360.0],
            press_cursor: [640.0, 360.0],
            axis: [0.0, -1.0],
            ..FrameInput::new()
        };
        merge_into(&mut dst, &src, false);
        assert_eq!(dst.keys_held, 3, "两路按住的键都要在");
        assert_eq!(dst.keys_pressed, 1);
        assert_eq!(dst.keys_released, 2);
        assert_eq!(dst.mouse_held, 4);
        assert_eq!(dst.scroll, [1.0, 1.0]);
        assert_eq!(dst.axis, [0.0, -1.0], "轴只有 JS 侧有（虚拟摇杆）");
        assert_eq!(
            dst.cursor,
            [640.0, 360.0],
            "本侧从没收到指针事件时，不能把 JS 侧算好的光标抹成 (0,0)"
        );

        // 本侧有新的指针事件时（`js_cursor_wins = false`），光标以本侧为准。
        let mut dst2 = FrameInput {
            cursor: [5.0, 6.0],
            ..FrameInput::new()
        };
        merge_into(&mut dst2, &src, false);
        assert_eq!(dst2.cursor, [5.0, 6.0], "本侧动过指针就听本侧的");

        // JS 侧动过（或本侧坐标还没建立）时以 JS 侧为准。
        let mut dst3 = FrameInput {
            cursor: [5.0, 6.0],
            ..FrameInput::new()
        };
        merge_into(&mut dst3, &src, true);
        assert_eq!(dst3.cursor, [640.0, 360.0]);
        assert_eq!(dst3.press_cursor, [640.0, 360.0]);
    }

    #[test]
    fn input_script_reports_what_is_wrong_instead_of_guessing() {
        assert!(
            InputScript::parse("").unwrap().is_empty(),
            "空串 = 没有剧本（正常路径一行不改）"
        );
        assert!(InputScript::parse("w:30-150").is_ok());
        // 每一类都必须是**报错**：静默丢一段剧本，现象就是"按了没反应"。
        for (spec, why) in [
            ("w30-150", "缺冒号"),
            ("w:30", "缺区间"),
            ("w:abc-150", "起帧不是数字"),
            ("w:30-xyz", "止帧不是数字"),
            ("w:150-30", "止帧不大于起帧"),
            ("w:30-30", "零长度区间"),
            ("nosuchkey:30-150", "键名不在线格式键表里"),
            ("axis2+:30-150", "摇杆轴编号超出范围"),
            ("axis1x:30-150", "摇杆轴方向非法"),
            ("axis1+:30-150,axis1-:40-50", "摇杆轴正反向重叠"),
            ("space:30-150", "空格键在线表里（'space'），但 ' ' 不在"),
        ] {
            let got = InputScript::parse(spec);
            // 空格键那条是**反例**：`space` 是合法键名，写 `' '` 才是错的。
            if spec == "space:30-150" {
                assert!(got.is_ok(), "'space' 应当合法：{why}");
                continue;
            }
            assert!(got.is_err(), "{spec} 应当报错（{why}）");
        }
    }

    #[test]
    fn input_script_windows_are_half_open() {
        let script = InputScript::parse("w:30-150,a:150-180").unwrap();
        assert!(script.held_at(29).is_empty());
        assert_eq!(script.held_at(30), vec!["w"], "起帧算按住");
        assert_eq!(script.held_at(149), vec!["w"]);
        assert_eq!(
            script.held_at(150),
            vec!["a"],
            "止帧算松开；同一帧开始的下一段立刻接过（半开区间的好处）"
        );
        assert_eq!(script.held_at(151), vec!["a"]);
        assert_eq!(script.held_at(179), vec!["a"]);
        assert!(script.held_at(180).is_empty());
    }

    #[test]
    fn script_driver_sets_and_clears_gamepad_axes_on_frame_boundaries() {
        let script = InputScript::parse("axis1-:2-4,axis0+:4-6").unwrap();
        let mut driver = ScriptDriver::new(script);
        let mut producer = FrameInputProducer::new();
        let mut frame = FrameInput::new();
        let mut axes = Vec::new();

        for _ in 0..6 {
            driver.tick(&mut producer);
            producer.take_frame(&mut frame);
            axes.push(frame.axis);
        }

        assert_eq!(
            axes,
            [
                [0.0, 0.0],
                [0.0, -1.0],
                [0.0, -1.0],
                [1.0, 0.0],
                [1.0, 0.0],
                [0.0, 0.0],
            ],
            "轴输入应在半开区间生效，并在窗口外回中"
        );

        let mut snapshot = InputSnapshot::default();
        let applied = apply_frame(&mut snapshot, &frame);
        assert_eq!(applied.axis_changes, 0, "末帧轴值已回中");
        frame.axis[0] = 0.5;
        let applied = apply_frame(&mut snapshot, &frame);
        assert_eq!(applied.axis_changes, 1);
        assert_eq!(snapshot.gamepad_axis(0), 0.5);
        assert!(apply_frame(&mut snapshot, &frame).is_empty());
    }

    #[test]
    fn script_driver_emits_edges_by_set_difference_not_by_row() {
        let mut producer = FrameInputProducer::new();
        let mut driver = ScriptDriver::new(InputScript::parse("w:2-4,a:4-6").unwrap());
        let mut out = FrameInput::new();
        let mut edges = Vec::new();
        for _ in 0..7 {
            driver.tick(&mut producer);
            producer.take_frame(&mut out);
            edges.push((out.keys_held, out.keys_pressed, out.keys_released));
        }
        // 帧 1：什么都没有。
        assert_eq!(edges[0], (0, 0, 0));
        // 位图是**按槽位**的（`w` 在槽 22，不是第 0 位），所以期望值必须由 ABI 的 `key_slot` 算，
        // 不能手写 `1`/`0b10`——手写就等于在测试里复制了一份槽位表，槽位表一变测试就假绿。
        let w = 1u128 << rustlua_frame_abi::key_slot("w").expect("w 在线表里");
        let a = 1u128 << rustlua_frame_abi::key_slot("a").expect("a 在线表里");
        // 帧 2：w 按下。帧 3：按住不再是边沿。
        assert_eq!(edges[1], (w, w, 0), "第 2 帧应当按下 w");
        assert_eq!(edges[2], (w, 0, 0), "按住不该每帧都产生按下边沿");
        // 帧 4：w 松、a 按——**同一帧换键也必须是两个独立边沿，且顺序固定**。
        // 注意 `w:2-4` 是半开区间：第 4 帧 w **已经松开**，所以此刻按住集合只剩 a。
        assert_eq!(edges[3], (a, a, w), "第 4 帧应当松开 w、按下 a");
        // 帧 6：a 松开。
        assert_eq!(edges[5], (0, 0, a), "第 6 帧应当松开 a");
    }

    /// 契约：`pointer_fresh` 只在"本帧真的收到过指针事件"时为真，取走一帧即归零。
    ///
    /// 它决定 `merge_into` 里"谁的光标算数"：本侧刚动过 ⇒ 本侧的光标赢；没动过 ⇒
    /// 让 JS 世界送来的光标留着。判错的两个方向都很难查——恒真是"点哪都不对"，
    /// 恒假是"JS 世界不送光标时鼠标彻底失灵"。
    #[test]
    fn pointer_fresh_is_true_only_after_a_pointer_event_and_clears_with_the_frame() {
        let mut producer = FrameInputProducer::new();
        let mut out = FrameInput::new();
        assert!(!producer.pointer_fresh(), "还没收到指针事件就不该是新鲜的");
        producer.pointer_moved([10.0, 20.0]);
        assert!(producer.pointer_fresh());
        producer.take_frame(&mut out);
        assert_eq!(out.cursor, [10.0, 20.0], "本帧的光标就是刚送进来的那个");
        assert!(
            !producer.pointer_fresh(),
            "取走即归零，否则会永远抢走 JS 世界的光标"
        );
    }

    /// 契约：`mouse_up` 清掉按住位，且**只**在真的按着时才产生"刚抬起"边沿。
    ///
    /// 宿主可能补发抬起（焦点丢失、指针在画布外抬起），所以"没按过的抬起"必须静默——
    /// 否则会凭空多出一次释放边沿，`just_released` 分支的脚本会在没点过的时候触发。
    #[test]
    fn mouse_up_clears_held_and_only_edges_when_the_button_was_down() {
        let mut producer = FrameInputProducer::new();
        let mut out = FrameInput::new();
        let left = 1u64 << rustlua_frame_abi::mouse_slot("left").expect("left 在线表里");

        // 没按过就抬起：不产生任何边沿。
        producer.mouse_up("left");
        producer.take_frame(&mut out);
        assert_eq!(out.mouse_held, 0);
        assert_eq!(out.mouse_just_released, 0, "没按过的抬起不该产生释放边沿");

        // 按下 → 抬起：按住位清零，且这一帧落在"刚抬起"位上。
        producer.mouse_down("left");
        producer.take_frame(&mut out);
        assert_eq!(out.mouse_held, left);
        producer.mouse_up("left");
        producer.take_frame(&mut out);
        assert_eq!(out.mouse_held, 0, "抬起必须清掉按住位");
        assert_eq!(out.mouse_just_released, left, "这一帧应当有释放边沿");
    }

    /// 契约：`ScriptDriver::is_empty` 与"剧本有没有给窗口"一致。
    ///
    /// 浏览器宿主每帧问一次它来决定"整段剧本要不要跳过"（见 `gpu_host` 的调用点）：
    /// 空剧本判假会白跑一次集合比较，非空判真会把一整段剧本静默丢掉——后者正是
    /// "按了没反应"里最贵的那个错法。
    #[test]
    /// 指针剧本的**跨帧**语义：按下点停在按下那一刻，光标继续跟着指针走。
    ///
    /// 这是「按住摇杆＋另一只手点按钮」的输入侧形状，也是 `input.press_position` 成立的前提——
    /// 若驱动把按下点每帧刷新成当前光标，或 `mouse_down` 与指针落位的顺序反了，这两个值就会相等，
    /// 命中测试随即退化成「跟着手指跑」，而**任何读数都不报错**。
    fn pointer_and_press_script_keeps_press_point_distinct_from_cursor() {
        let script = InputScript::parse("at:100x200:10-21,hold:left:15-40,at:600x400:21-60")
            .expect("script parse");
        let mut producer = FrameInputProducer::new();
        let mut driver = ScriptDriver::new(script);
        let mut out = rustlua_frame_abi::FrameInput::new();
        for _ in 0..30 {
            driver.tick(&mut producer);
            producer.take_frame(&mut out);
        }
        assert_eq!(out.press_cursor, [100.0, 200.0], "按下点必须停在按下那一刻");
        assert_eq!(out.cursor, [600.0, 400.0], "光标必须跟着指针走");
        assert_ne!(out.press_cursor, out.cursor, "两者必须是两个不同的值");
    }

    /// **多指门**：两根手指各留自己的按下点。
    ///
    /// 这条用例是上一片那条"钉住单值退化"的门**转正**后的样子：真机上"两指同时按两个按钮"是常态，
    /// 而旧线格式只有一个 `press_cursor` 槽——第二指的按下点**根本没处可写**（`mouse_down` 在已按下
    /// 时提前返回），于是第二个按钮**永远**无法用自己的按下点命中，且不报任何错。
    #[test]
    fn two_pointers_keep_their_own_press_points() {
        let mut producer = FrameInputProducer::new();
        let mut out = rustlua_frame_abi::FrameInput::new();
        // 第一指按在 A。
        producer.pointer_down(1, [100.0, 200.0]);
        producer.take_frame(&mut out);
        assert_eq!(out.pointer_count, 1, "一根手指占一个槽");
        assert_eq!(out.press_cursors[0], [100.0, 200.0]);
        assert_eq!(
            out.press_cursor,
            [100.0, 200.0],
            "旧字段 = 最后一个指针的按下点"
        );
        // 第二指按在 B：**A 的按下点必须还在**——这正是旧线格式做不到的事。
        producer.pointer_down(2, [600.0, 400.0]);
        producer.take_frame(&mut out);
        assert_eq!(out.pointer_count, 2);
        assert_eq!(
            out.press_cursors[0],
            [100.0, 200.0],
            "第一指的按下点没有被顶掉"
        );
        assert_eq!(out.press_cursors[1], [600.0, 400.0]);
        assert_eq!(out.press_cursor, [600.0, 400.0], "旧字段跟最近一次按下");
        // 移动不改变按下点：按下点是「按下那一刻」的位置，不是当前手指位置。
        producer.pointer_move(2, [700.0, 500.0]);
        producer.take_frame(&mut out);
        assert_eq!(
            out.press_cursors,
            [[100.0, 200.0], [600.0, 400.0], [0.0, 0.0], [0.0, 0.0]],
            "移动不得改写任何按下点"
        );
        // 抬起第一指 ⇒ 数组按按下先后**压实**（不留洞），否则按索引取按下点会取到空槽。
        producer.pointer_up(1);
        producer.take_frame(&mut out);
        assert_eq!(out.pointer_count, 1);
        assert_eq!(out.press_cursors[0], [600.0, 400.0], "抬起后压实，不留洞");
        // 桌面鼠标也算一根手指：没有多指事件时 `pointer_count` 至少为 1，Lua 按索引取值才不落空。
        let mut mouse = FrameInputProducer::new();
        let mut wire = rustlua_frame_abi::FrameInput::new();
        mouse.pointer_moved([40.0, 50.0]);
        mouse.mouse_down("left");
        mouse.take_frame(&mut wire);
        assert_eq!(wire.pointer_count, 1, "鼠标按住 = 一根手指");
        assert_eq!(wire.press_cursors[0], [40.0, 50.0]);
    }

    #[test]
    fn script_driver_is_empty_exactly_when_no_window_was_given() {
        assert!(ScriptDriver::new(InputScript::default()).is_empty());
        let script = InputScript::parse("w:10-20").expect("剧本可解析");
        assert!(!ScriptDriver::new(script).is_empty());
    }
}
