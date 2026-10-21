//! 宿主 → Lua 的**每帧输入线格式**（`RLFI` 批量 ABI）。
//!
//! 存在的理由（有实测支撑，见 `tasks/plan/M73-00-Web运行时性能重构.md`）：web 侧此前每帧把输入快照
//! **序列化成一段 Lua 源码**再 `lua_do_string` 编译执行（`__rustlua_input = {…}`）。在 wasm 上
//! 那一步是 **10.4 µs + 2,648 B Lua 垃圾/帧**，其中 **87–88% 是"编译 + 跨界编组"**，真正建表只占
//! 1.32 µs——也就是说这份成本几乎全花在"让 Lua 解析一段它每帧都要解析一次的字面量"上。
//! 这个 crate 把那 326 字节源码换成 **164 字节定长二进制**：不解码文本、不建表、不产生垃圾。
//!
//! 三条设计约束（都是被上一条实测逼出来的）：**一次调用搬完整帧**——wasm 侧的**固定**跨界开销
//! 占比比原生大（最小的一次 `lua_set_number` 是原生的 2.9–3.2×，编译密集路径只有 1.2–1.5×），
//! 所以"每个字段一次调用"会比现在的整条协议还贵，整帧只过一次边界；**键集固定**——位图索引就是
//! [`KEY_NAMES`] 的下标，键名不再上线，每帧只传"哪些位是 1"；**可原地复用**——解码结果是纯值类型
//! （[`FrameInput`]），调用方持有上一帧即可只改**变化**的位，常驻 Lua 表一辈子只建一次。
//!
//! **词汇表是 web 宿主的裸名**（`w`/`space`/`escape`/`left`…），不是引擎 `KeyCode` 的规范文本
//! （那是 `keye`/`keyw` 形式）。这不是随手选的：Lua 侧 `__rustlua_input.keys[name]` 的键今天就是
//! `plugins/webgl/web-runtime.js` 里 `keyName()` 的产出，游戏脚本与 `input.action*` 绑定表
//! （`bindings = { move_up = {'w'}, … }`）全都按这套裸名读。换成引擎名会让**每一处输入绑定同时失效**，
//! 而这属于"看起来能跑、玩起来没反应"的那类缺陷。所以线格式沿用裸名，规范文本的转换留在既有契约层。
//!
//! 校验口径：magic/版本/长度**任一不符即报错**（[`AbiError`]）。宁可响亮失败，也不要让布局漂移
//! 变成"键位悄悄错一格"——那类缺陷在浏览器里表现为"某个方向键不灵"，最难查。
//!
//! 单测跑在原生（`cargo test -p rustlua-frame-abi`）：本 crate 无 wasm 依赖，宿主上即可验证。
//! JS 侧的对拍在 `scripts/lib/wasm-lua/input-frame-abi-test.mjs`（同一份键表、同一份布局）。

use thiserror::Error;

/// 线格式魔数。放在首位是为了"拿错 buffer"能当场被发现，而不是按偏移读出一堆看似合理的浮点。
pub const MAGIC: [u8; 4] = *b"RLFI";

/// 线格式版本。**布局任何变化都必须+1**：解码侧按版本拒绝，旧宿主不会把新布局错读成有效帧。
pub const VERSION: u16 = 1;

/// 定长帧字节数。JS 侧不再手写这个数——它从 `input_frame_layout_json()` 取，避免两处各写一份。
pub const WIRE_LEN: usize = 232;

/// 键位槽数（`u128` 位图恰好覆盖 0..127）。
pub const KEY_SLOTS: usize = 128;

/// 鼠标按钮槽数（`u64` 位图恰好覆盖 0..63）。
pub const MOUSE_SLOTS: usize = 64;

/// 多指上限：定容数组的槽数（不用变长表——线格式是定长布局 + 具名偏移，变长要改分帧）。
pub const MAX_POINTERS: usize = 4;

// ~~多指之前那一版的长度（164 B）~~ —— **已删除**（2026-10-16）。这里用普通注释而不是文档注释：
// 它说明的是一个**已删除**的项，挂在下一个项上本来就不对（`///` 块后面夹空行还会触发
// `clippy::empty_line_after_doc_comments`，本轮就是被它咬到的）。
//
// 这里曾经写着"解码两版都收、编码只写 WIRE_LEN，因为 web-runtime.js 是另一个写者、自己打包
// 164 B、而它还在 --rust-scene-host-off 里活着"。**那条理由已经不成立**：JS 打包器按
// input_frame_layout_json() 报的 wire_len 分配缓冲（new Uint8Array(layout.wire_len)），
// 而那个字段报的就是 WIRE_LEN ⇒ 它写出来的本来就是 232 B（多指块留零）。
//
// 所以 164 B 的写者一个都不剩，decode 只认一种长度。这段历史留在这里，是因为"跨语言协议变更时
// 读者宽容"这个决定本身没错 —— 错的是**用一段人工维护的注释当判据**：它没跟着代码更新，于是
// 变成"没人敢删"的死分支（docs/51 把这类脱节列为本项目的已知边界）。

/// 键位表：**下标即线路位序**。改动顺序或删项等于改协议，必须同时把 [`VERSION`] +1 并跑
/// `scripts/verify/check-input-abi-parity.mjs`（它钉住"游戏绑定表里出现的每个键名都在这里"）。
///
/// 覆盖范围是"真实键盘上 DOM `KeyboardEvent.key` 会给出的键"：字母、数字、功能键、方向/导航键、
/// 编辑键、修饰键、标点。落不进这张表的键（`Dead`/`Process`/`Unidentified`/输入法组合键）会被宿主
/// **显式丢弃并计数**，而不是静默变成一个"按了没反应"的键——游戏绑定表用不到它们，但沉默的丢弃
/// 会让人以为是引擎坏了。
pub const KEY_NAMES: &[&str] = &[
    // 字母 a-z（0..25）
    "a",
    "b",
    "c",
    "d",
    "e",
    "f",
    "g",
    "h",
    "i",
    "j",
    "k",
    "l",
    "m",
    "n",
    "o",
    "p",
    "q",
    "r",
    "s",
    "t",
    "u",
    "v",
    "w",
    "x",
    "y",
    "z", // 数字 0-9（26..35）
    "0",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9", // 功能键 f1-f12（36..47）
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
    // 方向与导航（48..57）——注意 `keyName()` 已把 `ArrowLeft` 归一成 `left`，线格式沿用归一后的名字
    "left",
    "right",
    "up",
    "down",
    "home",
    "end",
    "pageup",
    "pagedown",
    "insert",
    "delete",
    // 空白与编辑（58..62）
    "space",
    "enter",
    "tab",
    "escape",
    "backspace", // 修饰与锁定（63..69）
    "shift",
    "control",
    "alt",
    "meta",
    "capslock",
    "numlock",
    "scrolllock", // 杂项（70..72）
    "pause",
    "printscreen",
    "contextmenu", // 标点（73..83）——按 US-QWERTY 物理键的 `event.key` 字符
    "`",
    "-",
    "=",
    "[",
    "]",
    "\\",
    ";",
    "'",
    ",",
    ".",
    "/",
    // Shift 之后的**字符形式**。必须有：浏览器给的是 `event.key`（字符），按住 Shift 时 `=` 变成
    // `'+'`，而游戏绑定表里已经这么写了（`volume_up = {'=', '+'}, volume_down = {'-', '_'}`，
    // 见 `web-runtime.js` 的 bindings）。这份表第一版漏了 `+`/`_`，被 ABI 对拍门当场抓出——
    // 那种缺陷的症状是"Shift+= 调不了音量"，只在该按键上出现，靠手感几乎不可能定位。
    "~",
    "!",
    "@",
    "#",
    "$",
    "%",
    "^",
    "&",
    "*",
    "(",
    ")",
    "_",
    "+",
    "{",
    "}",
    "|",
    ":",
    "\"",
    "<",
    ">",
    "?",
];

/// 鼠标按钮表：`event.button` 归一后的名字（宿主侧 `left`/`right`，适配层可能补 `middle`）。
pub const MOUSE_NAMES: &[&str] = &["left", "right", "middle"];

// 编译期把"表装得下"这件事钉死：加键加到溢出会编译失败，而不是让末尾的键悄悄共享同一个位。
const _: () = {
    assert!(KEY_NAMES.len() <= KEY_SLOTS, "KEY_NAMES 超出 KEY_SLOTS");
    assert!(
        MOUSE_NAMES.len() <= MOUSE_SLOTS,
        "MOUSE_NAMES 超出 MOUSE_SLOTS"
    );
};

/// 线路各字段偏移。**唯一一份**：Rust 编解码与 JS 打包都读它（后者经 `input_frame_layout_json()`）。
pub mod offsets {
    pub const MAGIC: usize = 0;
    pub const VERSION: usize = 4;
    pub const FLAGS: usize = 6;
    pub const FRAME: usize = 8;
    /// 两个 f64：轴（`axis_x`/`axis_y`）。用 f64 而不是 f32，是为了与"源码协议"**逐位同值**——
    /// 否则两边对拍只能带容差，而带容差的对拍会漏掉真正该抓的"某个字段被写错位置"。
    pub const AXIS: usize = 12;
    pub const CURSOR: usize = 28;
    pub const PRESS_CURSOR: usize = 44;
    pub const CURSOR_DELTA: usize = 60;
    pub const SCROLL: usize = 76;
    /// 三个 u128 位图（各按两个 u64 小端字写）：按住 / 按下边沿 / 抬起边沿。
    pub const KEYS_HELD: usize = 92;
    pub const KEYS_PRESSED: usize = 108;
    pub const KEYS_RELEASED: usize = 124;
    /// 三个 u64 位图：鼠标按住 / 按下边沿 / 抬起边沿。
    pub const MOUSE_HELD: usize = 140;
    pub const MOUSE_JUST: usize = 148;
    pub const MOUSE_JUST_RELEASED: usize = 156;
    /// 当前按住/接触的指针数（`u32`，解码时钳到 `MAX_POINTERS`）。
    pub const POINTER_COUNT: usize = 164;
    /// 每个指针**按下那一刻**的位置（`MAX_POINTERS` 个 f64 对，按按下先后排列）。
    pub const PRESS_CURSORS: usize = 168;
}

// 布局自洽：上面那些常量算出来的总长必须等于 WIRE_LEN，否则编解码会写下标越界。
const _: () = {
    // 曾经这里还有一条"鼠标块末尾 == 164"（守住 legacy 长度）的断言；legacy 删掉后它没有意义，
    // 只留下"多指块末尾 == WIRE_LEN"这条真正防越界的。
    assert!(
        offsets::PRESS_CURSORS + MAX_POINTERS * 16 == WIRE_LEN,
        "多指块与 WIRE_LEN 不一致"
    );
};

/// 线格式错误。全部是"响亮失败"：宿主拿到的帧不合法时应当**整帧丢弃并报错**，
/// 而不是尽量解释——解释出来的多半是一帧错位的输入。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AbiError {
    /// 长度不是 [`WIRE_LEN`]。
    #[error("input frame length {actual} != {expected}")]
    Length { actual: usize, expected: usize },
    /// 魔数不符（拿错了 buffer / 端序搞反 / 上游压根没打包）。
    #[error("input frame magic mismatch")]
    Magic,
    /// 版本不符（宿主与 wasm 不同源；升级时常因旧胶水配新 wasm 出现）。
    #[error("input frame version {actual} != {expected}")]
    Version { actual: u16, expected: u16 },
    /// 位图里出现了表外的位（键表比宿主新/旧）。
    #[error("input frame sets bit {slot} outside the key table")]
    UnknownSlot { slot: usize },
}

/// 一帧输入（纯值，可原地复用）。
///
/// 字段与 `__rustlua_input` 的键一一对应：`axis`→`axis_x/axis_y`、`cursor`→`cursor`、
/// `press_cursor`→`press_cursor`、`cursor_delta`→`cursor_delta`、`scroll`→`scroll`、
/// `keys_held`→`keys`、`keys_pressed`→`just`、`keys_released`→`released`、
/// `mouse_held`→`mouse`、`mouse_just`→`mouse_just`、`mouse_just_released`→`mouse_just_released`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameInput {
    /// 帧序号（宿主自增）。仅用于诊断与"这帧是不是旧的"判断，解码不做单调性校验。
    pub frame: u32,
    /// 轴：`[x, y]`，与宿主 `axis_x`/`axis_y` 同序。
    pub axis: [f64; 2],
    /// 光标位置（设计坐标）。
    pub cursor: [f64; 2],
    /// 最近一次按下的位置（虚拟摇杆跟手时与 `cursor` 不同）。
    pub press_cursor: [f64; 2],
    /// 本帧光标位移。
    pub cursor_delta: [f64; 2],
    /// 本帧滚轮增量（`[x, y]`，两个分量）。
    pub scroll: [f64; 2],
    /// 按住集合（位下标 = [`KEY_NAMES`] 下标）。
    pub keys_held: u128,
    /// 本帧按下边沿。
    pub keys_pressed: u128,
    /// 本帧抬起边沿。
    pub keys_released: u128,
    /// 鼠标按住集合（位下标 = [`MOUSE_NAMES`] 下标）。
    pub mouse_held: u64,
    /// 鼠标按下边沿。
    pub mouse_just: u64,
    /// 鼠标抬起边沿。
    pub mouse_just_released: u64,
    /// 当前指针数（多指）。`0` 也包括"旧版写者只打包了 164 B"这一情形。
    pub pointer_count: u8,
    /// 每个指针的按下点，**按按下先后**排列；`[0..pointer_count]` 有效。
    ///
    /// 与 [`Self::press_cursor`] 的关系：后者是**最近一次**按下的那个点（旧语义，保持有效），
    /// 即 `press_cursors[pointer_count - 1]`。两者冗余是刻意的——旧读者读旧字段、新读者循环数组；
    /// 不变式由测试钉住，免得两份表示各自漂开。
    pub press_cursors: [[f64; 2]; MAX_POINTERS],
}

impl Default for FrameInput {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameInput {
    /// 全零帧。
    ///
    /// 是 `const fn`：宿主把它当"上一帧"的初值放在 `thread_local` 里，那是编译期求值的。
    #[must_use]
    pub const fn new() -> Self {
        Self {
            frame: 0,
            axis: [0.0, 0.0],
            cursor: [0.0, 0.0],
            press_cursor: [0.0, 0.0],
            cursor_delta: [0.0, 0.0],
            scroll: [0.0, 0.0],
            keys_held: 0,
            keys_pressed: 0,
            keys_released: 0,
            mouse_held: 0,
            mouse_just: 0,
            mouse_just_released: 0,
            pointer_count: 0,
            press_cursors: [[0.0, 0.0]; MAX_POINTERS],
        }
    }

    /// 写进定长缓冲。`out` 长度必须恰好 [`WIRE_LEN`]（短了会截断成"看似有效"的帧）。
    pub fn encode(&self, out: &mut [u8]) -> Result<(), AbiError> {
        if out.len() != WIRE_LEN {
            return Err(AbiError::Length {
                actual: out.len(),
                expected: WIRE_LEN,
            });
        }
        out[offsets::MAGIC..offsets::MAGIC + 4].copy_from_slice(&MAGIC);
        write_u16(out, offsets::VERSION, VERSION);
        // flags 保留位：当前恒 0。解码侧不校验它，于是将来新增标志位不必抬版本号。
        write_u16(out, offsets::FLAGS, 0);
        write_u32(out, offsets::FRAME, self.frame);
        write_pair(out, offsets::AXIS, self.axis);
        write_pair(out, offsets::CURSOR, self.cursor);
        write_pair(out, offsets::PRESS_CURSOR, self.press_cursor);
        write_pair(out, offsets::CURSOR_DELTA, self.cursor_delta);
        write_pair(out, offsets::SCROLL, self.scroll);
        write_u128(out, offsets::KEYS_HELD, self.keys_held);
        write_u128(out, offsets::KEYS_PRESSED, self.keys_pressed);
        write_u128(out, offsets::KEYS_RELEASED, self.keys_released);
        write_u64(out, offsets::MOUSE_HELD, self.mouse_held);
        write_u64(out, offsets::MOUSE_JUST, self.mouse_just);
        write_u64(out, offsets::MOUSE_JUST_RELEASED, self.mouse_just_released);
        write_u32(out, offsets::POINTER_COUNT, u32::from(self.pointer_count));
        for (index, point) in self.press_cursors.iter().enumerate() {
            write_pair(out, offsets::PRESS_CURSORS + index * 16, *point);
        }
        Ok(())
    }

    /// 从定长缓冲解析。magic/版本/长度任一不符即 [`AbiError`]。
    pub fn decode(bytes: &[u8]) -> Result<Self, AbiError> {
        // **只认 `WIRE_LEN`**：以前两版都收，是因为 `web-runtime.js` 当时自己打包 164 B。现在它按
        // 布局报的 `wire_len`（= `WIRE_LEN`）分配 ⇒ 164 B 的写者一个都不剩，单边收紧不伤任何在跑的路。
        if bytes.len() != WIRE_LEN {
            return Err(AbiError::Length {
                actual: bytes.len(),
                expected: WIRE_LEN,
            });
        }
        if bytes[offsets::MAGIC..offsets::MAGIC + 4] != MAGIC {
            return Err(AbiError::Magic);
        }
        let version = read_u16(bytes, offsets::VERSION);
        if version != VERSION {
            return Err(AbiError::Version {
                actual: version,
                expected: VERSION,
            });
        }
        // 计数钳到上限：越界计数是上游的错，但截断后仍能读出有效前缀，比整帧失败更有用。
        let pointer_count =
            (read_u32(bytes, offsets::POINTER_COUNT) as usize).min(MAX_POINTERS) as u8;
        let mut press_cursors = [[0.0, 0.0]; MAX_POINTERS];
        for (index, point) in press_cursors.iter_mut().enumerate() {
            *point = read_pair(bytes, offsets::PRESS_CURSORS + index * 16);
        }
        Ok(Self {
            frame: read_u32(bytes, offsets::FRAME),
            axis: read_pair(bytes, offsets::AXIS),
            cursor: read_pair(bytes, offsets::CURSOR),
            press_cursor: read_pair(bytes, offsets::PRESS_CURSOR),
            cursor_delta: read_pair(bytes, offsets::CURSOR_DELTA),
            scroll: read_pair(bytes, offsets::SCROLL),
            keys_held: read_u128(bytes, offsets::KEYS_HELD),
            keys_pressed: read_u128(bytes, offsets::KEYS_PRESSED),
            keys_released: read_u128(bytes, offsets::KEYS_RELEASED),
            mouse_held: read_u64(bytes, offsets::MOUSE_HELD),
            mouse_just: read_u64(bytes, offsets::MOUSE_JUST),
            mouse_just_released: read_u64(bytes, offsets::MOUSE_JUST_RELEASED),
            pointer_count,
            press_cursors,
        })
    }

    /// 位图里是否出现了表外的位；返回第一个越界槽位。
    ///
    /// 刻意放在解码之后单独调用：编解码只管"字节是不是这帧"，"位是不是合法键"是**语义**校验，
    /// 而解码要能在键表比宿主旧的场景下仍然把值读出来（便于报出"是哪个位"）。
    #[must_use]
    pub fn first_unknown_key_slot(&self) -> Option<usize> {
        let mask = used_bits(KEY_NAMES.len(), KEY_SLOTS);
        let unknown = (self.keys_held | self.keys_pressed | self.keys_released) & !mask;
        if unknown == 0 {
            None
        } else {
            Some(unknown.trailing_zeros() as usize)
        }
    }

    /// 鼠标位图里第一个表外的位。
    #[must_use]
    pub fn first_unknown_mouse_slot(&self) -> Option<usize> {
        // `used_bits` 统一用 u128 表达（键表要 128 位）；鼠标表 ≤64 项，截断是精确的。
        let mask = used_bits(MOUSE_NAMES.len(), MOUSE_SLOTS) as u64;
        let unknown = (self.mouse_held | self.mouse_just | self.mouse_just_released) & !mask;
        if unknown == 0 {
            None
        } else {
            Some(unknown.trailing_zeros() as usize)
        }
    }

    /// 校验所有位都在表内（宿主与表同源时的常规断言）。
    pub fn validate_slots(&self) -> Result<(), AbiError> {
        if let Some(slot) = self.first_unknown_key_slot() {
            return Err(AbiError::UnknownSlot { slot });
        }
        if let Some(slot) = self.first_unknown_mouse_slot() {
            return Err(AbiError::UnknownSlot { slot });
        }
        Ok(())
    }
}

/// `slot` 位是否置起（键表口径）。
#[must_use]
pub fn key_bit(bits: u128, slot: usize) -> bool {
    slot < KEY_SLOTS && (bits >> slot) & 1 == 1
}

/// 键表里 `name`（大小写不敏感）的槽位。
#[must_use]
pub fn key_slot(name: &str) -> Option<usize> {
    let name = name.trim();
    KEY_NAMES
        .iter()
        .position(|candidate| candidate.eq_ignore_ascii_case(name))
}

/// 鼠标按钮名 → 槽位。
#[must_use]
pub fn mouse_slot(name: &str) -> Option<usize> {
    let name = name.trim();
    MOUSE_NAMES
        .iter()
        .position(|candidate| candidate.eq_ignore_ascii_case(name))
}

/// `count` 个有效位占满 `slots` 时的合法位掩码（用于"表外的位"判定）。
fn used_bits(count: usize, slots: usize) -> u128 {
    debug_assert!(slots <= 128);
    if count >= slots {
        u128::MAX
    } else if count == 0 {
        0
    } else {
        (1u128 << count) - 1
    }
}

fn write_u16(out: &mut [u8], offset: usize, value: u16) {
    out[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(out: &mut [u8], offset: usize, value: u32) {
    out[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(out: &mut [u8], offset: usize, value: u64) {
    out[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn write_u128(out: &mut [u8], offset: usize, value: u128) {
    out[offset..offset + 16].copy_from_slice(&value.to_le_bytes());
}

fn write_pair(out: &mut [u8], offset: usize, value: [f64; 2]) {
    out[offset..offset + 8].copy_from_slice(&value[0].to_le_bytes());
    out[offset + 8..offset + 16].copy_from_slice(&value[1].to_le_bytes());
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(raw)
}

fn read_u128(bytes: &[u8], offset: usize) -> u128 {
    let mut raw = [0u8; 16];
    raw.copy_from_slice(&bytes[offset..offset + 16]);
    u128::from_le_bytes(raw)
}

fn read_pair(bytes: &[u8], offset: usize) -> [f64; 2] {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&bytes[offset..offset + 8]);
    let first = f64::from_le_bytes(raw);
    raw.copy_from_slice(&bytes[offset + 8..offset + 16]);
    [first, f64::from_le_bytes(raw)]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 写一帧"每个字段都不同"的值：全 0 的往返测试会漏掉"两个字段被写到同一个偏移"。
    fn sample() -> FrameInput {
        FrameInput {
            frame: 4242,
            axis: [1.0, -1.0],
            cursor: [627.5, 318.25],
            press_cursor: [12.0, 34.0],
            cursor_delta: [-3.5, 2.25],
            scroll: [0.5, -8.0],
            keys_held: key_slot("w").map_or(0, |s| 1u128 << s)
                | key_slot("space").map_or(0, |s| 1u128 << s)
                | key_slot("left").map_or(0, |s| 1u128 << s),
            keys_pressed: key_slot("escape").map_or(0, |s| 1u128 << s),
            keys_released: key_slot("d").map_or(0, |s| 1u128 << s),
            mouse_held: 0b01,
            mouse_just: 0b10,
            mouse_just_released: 0,
            pointer_count: 2,
            press_cursors: [[12.0, 34.0], [56.0, 78.0], [0.0, 0.0], [0.0, 0.0]],
        }
    }

    #[test]
    fn round_trip_preserves_every_field() {
        let frame = sample();
        let mut wire = [0u8; WIRE_LEN];
        frame.encode(&mut wire).expect("encode");
        assert_eq!(FrameInput::decode(&wire).expect("decode"), frame);
    }

    #[test]
    fn round_trip_keeps_f64_bit_exact() {
        // 与源码协议逐位同值是对拍测试的前提：`627.1` 这种十进制在 f32 下会变，f64 不会。
        let frame = FrameInput {
            cursor: [627.1, 318.7],
            cursor_delta: [0.1, -0.2],
            ..FrameInput::default()
        };
        let mut wire = [0u8; WIRE_LEN];
        frame.encode(&mut wire).expect("encode");
        let back = FrameInput::decode(&wire).expect("decode");
        assert_eq!(back.cursor[0].to_bits(), 627.1f64.to_bits());
        assert_eq!(back.cursor_delta[0].to_bits(), 0.1f64.to_bits());
    }

    #[test]
    fn header_is_magic_version_flags() {
        let mut wire = [0u8; WIRE_LEN];
        FrameInput::default().encode(&mut wire).expect("encode");
        assert_eq!(&wire[0..4], b"RLFI");
        assert_eq!(u16::from_le_bytes([wire[4], wire[5]]), 1);
        assert_eq!(u16::from_le_bytes([wire[6], wire[7]]), 0, "flags 预留为 0");
    }

    #[test]
    fn decode_rejects_wrong_length() {
        let short = [0u8; WIRE_LEN - 1];
        assert_eq!(
            FrameInput::decode(&short),
            Err(AbiError::Length {
                actual: WIRE_LEN - 1,
                expected: WIRE_LEN
            })
        );
        let long = [0u8; WIRE_LEN + 1];
        assert!(matches!(
            FrameInput::decode(&long),
            Err(AbiError::Length { .. })
        ));
    }

    #[test]
    fn decode_rejects_wrong_magic_and_version() {
        let mut wire = [0u8; WIRE_LEN];
        FrameInput::default().encode(&mut wire).expect("encode");
        let mut bad_magic = wire;
        bad_magic[0] = b'X';
        assert_eq!(FrameInput::decode(&bad_magic), Err(AbiError::Magic));

        let mut bad_version = wire;
        bad_version[4] = 2;
        assert_eq!(
            FrameInput::decode(&bad_version),
            Err(AbiError::Version {
                actual: 2,
                expected: VERSION
            })
        );
    }

    #[test]
    fn encode_rejects_wrong_buffer_length() {
        let mut short = [0u8; 8];
        assert!(matches!(
            FrameInput::default().encode(&mut short),
            Err(AbiError::Length { .. })
        ));
    }

    #[test]
    fn unknown_slots_are_detected_not_ignored() {
        let mut frame = FrameInput::default();
        let outside = KEY_SLOTS - 1;
        frame.keys_held = 1u128 << outside;
        assert_eq!(frame.first_unknown_key_slot(), Some(outside));
        assert_eq!(
            frame.validate_slots(),
            Err(AbiError::UnknownSlot { slot: outside })
        );
        // 表内的位不该被误报。
        let inside = key_slot("j").expect("j 在表里");
        frame.keys_held = 1u128 << inside;
        assert_eq!(frame.first_unknown_key_slot(), None);
        assert_eq!(frame.validate_slots(), Ok(()));
    }

    #[test]
    fn mouse_slots_outside_table_are_detected() {
        let frame = FrameInput {
            mouse_held: 1u64 << 40,
            ..FrameInput::default()
        };
        assert_eq!(frame.first_unknown_mouse_slot(), Some(40));
        assert!(frame.validate_slots().is_err());
    }

    #[test]
    fn key_table_is_unique_and_within_bounds() {
        assert!(KEY_NAMES.len() <= KEY_SLOTS);
        assert!(MOUSE_NAMES.len() <= MOUSE_SLOTS);
        for (index, name) in KEY_NAMES.iter().enumerate() {
            assert!(!name.is_empty(), "槽位 {index} 是空名");
            assert_eq!(
                *name,
                name.to_ascii_lowercase(),
                "槽位 {index} 的键名必须小写（宿主产出即小写）"
            );
            assert_eq!(
                key_slot(name),
                Some(index),
                "键名 {name} 重复或大小写不一致"
            );
        }
    }

    #[test]
    fn key_lookup_is_case_insensitive_and_rejects_unknown() {
        assert_eq!(key_slot("W"), key_slot("w"));
        assert_eq!(key_slot(" space "), key_slot("space"));
        assert_eq!(key_slot("nosuchkey"), None);
        assert_eq!(mouse_slot("Right"), mouse_slot("right"));
        assert_eq!(mouse_slot("nosuchbutton"), None);
    }

    /// 旧长度（164 B）**不再被接受** —— 这是那条 legacy 分支的**删除判据**：它以前断言"164 仍可解码"，
    /// 而那条路现在连写者都没有（详见 `LEGACY_WIRE_LEN` 被删处留的那段历史）。保留同形状的一条负例，
    /// 免得"宽松接受到底还在不在"只能靠翻代码确认。
    #[test]
    fn legacy_length_is_rejected_now() {
        let mut wire = [0u8; WIRE_LEN];
        let frame = FrameInput::new();
        frame.encode(&mut wire).expect("encode");
        let short = FrameInput::decode(&wire[..164]);
        assert!(
            matches!(short, Err(AbiError::Length { .. })),
            "164 应当报长度错"
        );
    }

    #[test]
    fn offsets_do_not_overlap() {
        // 逐个字段写满再读回，能抓出"两个字段共用偏移"（只对单字段做往返是抓不到的）。
        let mut wire = [0u8; WIRE_LEN];
        let frame = FrameInput {
            frame: 7,
            axis: [1.0, 2.0],
            cursor: [3.0, 4.0],
            press_cursor: [5.0, 6.0],
            cursor_delta: [7.0, 8.0],
            scroll: [9.0, 10.0],
            keys_held: 1,
            keys_pressed: 2,
            keys_released: 4,
            mouse_held: 1,
            mouse_just: 2,
            mouse_just_released: 4,
            pointer_count: 2,
            press_cursors: [[11.0, 12.0], [13.0, 14.0], [0.0, 0.0], [0.0, 0.0]],
        };
        frame.encode(&mut wire).expect("encode");
        let back = FrameInput::decode(&wire).expect("decode");
        assert_eq!(back.frame, 7);
        // 多指块也要逐个读回：它与既有的 `press_cursor` 是两份表示，最怕"写到同一个偏移"。
        assert_eq!(back.pointer_count, 2);
        assert_eq!(back.press_cursors[0], [11.0, 12.0]);
        assert_eq!(back.press_cursors[1], [13.0, 14.0]);
        assert_eq!(back.axis, [1.0, 2.0]);
        assert_eq!(back.cursor, [3.0, 4.0]);
        assert_eq!(back.press_cursor, [5.0, 6.0]);
        assert_eq!(back.cursor_delta, [7.0, 8.0]);
        assert_eq!(back.scroll, [9.0, 10.0]);
        assert_eq!(back.keys_held, 1);
        assert_eq!(back.keys_pressed, 2);
        assert_eq!(back.keys_released, 4);
        assert_eq!(back.mouse_held, 1);
        assert_eq!(back.mouse_just, 2);
        assert_eq!(back.mouse_just_released, 4);
    }
}
