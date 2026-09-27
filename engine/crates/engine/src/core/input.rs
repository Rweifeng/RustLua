//! Platform-independent input snapshots for deterministic simulation.
//!
//! M20-02 追加**动作映射层**：把物理输入（键盘/鼠标/抽象手柄）映射到命名动作，
//! 支持重绑定与设置持久化（[`ActionMap::to_text`] / [`ActionMap::from_text`]）。
//! 手柄为**最小跨平台抽象**：宿主喂入按钮/轴即可，具体设备轮询（需要额外
//! 依赖）未接入，见任务 M20-02 的已知限制。

use std::collections::{BTreeMap, HashMap, HashSet};

// ---------------------------------------------------------------------------
// 物理键身份（M22/W1）
//
// `KeyCode` 是**物理键**（键盘上那个位置），不是字符：宿主读平台的 physical key
// （winit `PhysicalKey::Code` / egui `Key`）后经 `host::keymap` 转换成本枚举。
// 因此 AZERTY 的 `Z` 位置仍解析为 `KeyW`（移动向上），Dvorak 同理；`Shift+1` 得到
// `Digit1` 而不是某个字符；Tab/Enter/F5/修饰键都能表达（旧 `InputKey` 是按字符编码，
// 这些键**根本无法表达**）。
//
// 变体集合与顺序**逐项对齐 winit 0.30 的 `KeyCode`**（194 项，含平台相关的
// `Lang1..Lang5`/`IntlRo` 等）：`keymap` 的两张转换表与 `ALL` 全表都由本表生成，
// 新增键位只需在这里加一项，宿主两张表与编辑器预览键集合自动跟随。
//
// 文本形式是变体名的小写（`key:w`、`key:digit1`、`key:arrowup`、`key:space`），
// 与平台库的 Debug 名一一对应，稳定且无歧义（v1 的字符形式仍在 `from_text` 的
// 兼容表里，见 `v1_key_code`）。
macro_rules! define_key_codes {
    ($( $variant:ident => $text:literal ),* $(,)?) => {
        /// 物理键身份（与 winit `KeyCode` 一一对应）。
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum KeyCode {
            $( $variant, )*
        }

        impl KeyCode {
            /// 全表（顺序 = 枚举声明序）。
            pub const ALL: &'static [Self] = &[
                $( Self::$variant, )*
            ];

            /// 稳定文本形式（设置文件与 `Binding::to_text` 用）。
            ///
            /// 是变体名的小写字面量（`KeyE` → `"keye"`、`Digit1` → `"digit1"`、
            /// `ArrowUp` → `"arrowup"`、`Space` → `"space"`）：与 winit 的 Debug 名
            /// 一一对应，稳定无歧义；小写形式写进 `settings.toml` 后也更接近玩家在
            /// `input.is_down("keye")` 里手写的名字（解析侧大小写不敏感）。
            ///
            /// 文本必须**逐字等于变体名的小写**，由下面的常量断言在编译期强制：
            /// 想给某个键起别名（例如把 `Equal` 写成 `"equals"`）会编译失败，
            /// 而不是让文本形式与平台名悄悄分叉。
            #[must_use]
            pub const fn to_text(self) -> &'static str {
                match self {
                    $( Self::$variant => $text, )*
                }
            }

            /// 解析 [`Self::to_text`] 的输出（大小写不敏感）。
            ///
            /// 逐项比较 194 项而不是查表：键域很小且解析只发生在读设置文件/`rebind`
            /// 时，换来的是**表只有一份**（文本由上面的 `to_text` 唯一决定）。
            #[must_use]
            pub fn from_text(text: &str) -> Option<Self> {
                let text = text.trim();
                Self::ALL
                    .iter()
                    .copied()
                    .find(|candidate| candidate.to_text().eq_ignore_ascii_case(text))
            }
        }

        $(
            const _: () = {
                let bytes = $text.as_bytes();
                let mut index = 0;
                while index < bytes.len() {
                    assert!(
                        bytes[index].is_ascii_lowercase() || bytes[index].is_ascii_digit(),
                        "KeyCode 文本形式必须是小写字母/数字（与 winit 变体名同名）"
                    );
                    index += 1;
                }
            };
        )*
    };
}

define_key_codes!(
    Backquote => "backquote", Backslash => "backslash", BracketLeft => "bracketleft", BracketRight => "bracketright",
    Comma => "comma", Digit0 => "digit0", Digit1 => "digit1", Digit2 => "digit2",
    Digit3 => "digit3", Digit4 => "digit4", Digit5 => "digit5", Digit6 => "digit6",
    Digit7 => "digit7", Digit8 => "digit8", Digit9 => "digit9", Equal => "equal",
    IntlBackslash => "intlbackslash", IntlRo => "intlro", IntlYen => "intlyen", KeyA => "keya",
    KeyB => "keyb", KeyC => "keyc", KeyD => "keyd", KeyE => "keye",
    KeyF => "keyf", KeyG => "keyg", KeyH => "keyh", KeyI => "keyi",
    KeyJ => "keyj", KeyK => "keyk", KeyL => "keyl", KeyM => "keym",
    KeyN => "keyn", KeyO => "keyo", KeyP => "keyp", KeyQ => "keyq",
    KeyR => "keyr", KeyS => "keys", KeyT => "keyt", KeyU => "keyu",
    KeyV => "keyv", KeyW => "keyw", KeyX => "keyx", KeyY => "keyy",
    KeyZ => "keyz", Minus => "minus", Period => "period", Quote => "quote",
    Semicolon => "semicolon", Slash => "slash", AltLeft => "altleft", AltRight => "altright",
    Backspace => "backspace", CapsLock => "capslock", ContextMenu => "contextmenu", ControlLeft => "controlleft",
    ControlRight => "controlright", Enter => "enter", SuperLeft => "superleft", SuperRight => "superright",
    ShiftLeft => "shiftleft", ShiftRight => "shiftright", Space => "space", Tab => "tab",
    Convert => "convert", KanaMode => "kanamode", Lang1 => "lang1", Lang2 => "lang2",
    Lang3 => "lang3", Lang4 => "lang4", Lang5 => "lang5", NonConvert => "nonconvert",
    Delete => "delete", End => "end", Help => "help", Home => "home",
    Insert => "insert", PageDown => "pagedown", PageUp => "pageup", ArrowDown => "arrowdown",
    ArrowLeft => "arrowleft", ArrowRight => "arrowright", ArrowUp => "arrowup", NumLock => "numlock",
    Numpad0 => "numpad0", Numpad1 => "numpad1", Numpad2 => "numpad2", Numpad3 => "numpad3",
    Numpad4 => "numpad4", Numpad5 => "numpad5", Numpad6 => "numpad6", Numpad7 => "numpad7",
    Numpad8 => "numpad8", Numpad9 => "numpad9", NumpadAdd => "numpadadd", NumpadBackspace => "numpadbackspace",
    NumpadClear => "numpadclear", NumpadClearEntry => "numpadclearentry", NumpadComma => "numpadcomma", NumpadDecimal => "numpaddecimal",
    NumpadDivide => "numpaddivide", NumpadEnter => "numpadenter", NumpadEqual => "numpadequal", NumpadHash => "numpadhash",
    NumpadMemoryAdd => "numpadmemoryadd", NumpadMemoryClear => "numpadmemoryclear", NumpadMemoryRecall => "numpadmemoryrecall", NumpadMemoryStore => "numpadmemorystore",
    NumpadMemorySubtract => "numpadmemorysubtract", NumpadMultiply => "numpadmultiply", NumpadParenLeft => "numpadparenleft", NumpadParenRight => "numpadparenright",
    NumpadStar => "numpadstar", NumpadSubtract => "numpadsubtract", Escape => "escape", Fn => "fn",
    FnLock => "fnlock", PrintScreen => "printscreen", ScrollLock => "scrolllock", Pause => "pause",
    BrowserBack => "browserback", BrowserFavorites => "browserfavorites", BrowserForward => "browserforward", BrowserHome => "browserhome",
    BrowserRefresh => "browserrefresh", BrowserSearch => "browsersearch", BrowserStop => "browserstop", Eject => "eject",
    LaunchApp1 => "launchapp1", LaunchApp2 => "launchapp2", LaunchMail => "launchmail", MediaPlayPause => "mediaplaypause",
    MediaSelect => "mediaselect", MediaStop => "mediastop", MediaTrackNext => "mediatracknext", MediaTrackPrevious => "mediatrackprevious",
    Power => "power", Sleep => "sleep", AudioVolumeDown => "audiovolumedown", AudioVolumeMute => "audiovolumemute",
    AudioVolumeUp => "audiovolumeup", WakeUp => "wakeup", Meta => "meta", Hyper => "hyper",
    Turbo => "turbo", Abort => "abort", Resume => "resume", Suspend => "suspend",
    Again => "again", Copy => "copy", Cut => "cut", Find => "find",
    Open => "open", Paste => "paste", Props => "props", Select => "select",
    Undo => "undo", Hiragana => "hiragana", Katakana => "katakana", F1 => "f1",
    F2 => "f2", F3 => "f3", F4 => "f4", F5 => "f5",
    F6 => "f6", F7 => "f7", F8 => "f8", F9 => "f9",
    F10 => "f10", F11 => "f11", F12 => "f12", F13 => "f13",
    F14 => "f14", F15 => "f15", F16 => "f16", F17 => "f17",
    F18 => "f18", F19 => "f19", F20 => "f20", F21 => "f21",
    F22 => "f22", F23 => "f23", F24 => "f24", F25 => "f25",
    F26 => "f26", F27 => "f27", F28 => "f28", F29 => "f29",
    F30 => "f30", F31 => "f31", F32 => "f32", F33 => "f33",
    F34 => "f34", F35 => "f35",
);

impl KeyCode {
    /// 人可读标签（编辑器捕获面板、日志、`input.bindings()` 的展示列）。    ///
    /// 与 [`Self::to_text`]（稳定序列化文本）刻意分开：标签可以随排版变化，
    /// 文本形式进设置文件因此必须稳定。表覆盖全部变体，`key_code_label` 的
    /// `_ => name` 分支只是兜底（未收录的键显示其文本名，不会 panic 或空白）。
    #[must_use]
    pub const fn display_label(self) -> &'static str {
        match self {
            Self::KeyA => "A",
            Self::KeyB => "B",
            Self::KeyC => "C",
            Self::KeyD => "D",
            Self::KeyE => "E",
            Self::KeyF => "F",
            Self::KeyG => "G",
            Self::KeyH => "H",
            Self::KeyI => "I",
            Self::KeyJ => "J",
            Self::KeyK => "K",
            Self::KeyL => "L",
            Self::KeyM => "M",
            Self::KeyN => "N",
            Self::KeyO => "O",
            Self::KeyP => "P",
            Self::KeyQ => "Q",
            Self::KeyR => "R",
            Self::KeyS => "S",
            Self::KeyT => "T",
            Self::KeyU => "U",
            Self::KeyV => "V",
            Self::KeyW => "W",
            Self::KeyX => "X",
            Self::KeyY => "Y",
            Self::KeyZ => "Z",
            Self::Digit0 => "0",
            Self::Digit1 => "1",
            Self::Digit2 => "2",
            Self::Digit3 => "3",
            Self::Digit4 => "4",
            Self::Digit5 => "5",
            Self::Digit6 => "6",
            Self::Digit7 => "7",
            Self::Digit8 => "8",
            Self::Digit9 => "9",
            Self::Numpad0 => "小键盘 0",
            Self::Numpad1 => "小键盘 1",
            Self::Numpad2 => "小键盘 2",
            Self::Numpad3 => "小键盘 3",
            Self::Numpad4 => "小键盘 4",
            Self::Numpad5 => "小键盘 5",
            Self::Numpad6 => "小键盘 6",
            Self::Numpad7 => "小键盘 7",
            Self::Numpad8 => "小键盘 8",
            Self::Numpad9 => "小键盘 9",
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F3 => "F3",
            Self::F4 => "F4",
            Self::F5 => "F5",
            Self::F6 => "F6",
            Self::F7 => "F7",
            Self::F8 => "F8",
            Self::F9 => "F9",
            Self::F10 => "F10",
            Self::F11 => "F11",
            Self::F12 => "F12",
            Self::F13 => "F13",
            Self::F14 => "F14",
            Self::F15 => "F15",
            Self::F16 => "F16",
            Self::F17 => "F17",
            Self::F18 => "F18",
            Self::F19 => "F19",
            Self::F20 => "F20",
            Self::F21 => "F21",
            Self::F22 => "F22",
            Self::F23 => "F23",
            Self::F24 => "F24",
            Self::F25 => "F25",
            Self::F26 => "F26",
            Self::F27 => "F27",
            Self::F28 => "F28",
            Self::F29 => "F29",
            Self::F30 => "F30",
            Self::F31 => "F31",
            Self::F32 => "F32",
            Self::F33 => "F33",
            Self::F34 => "F34",
            Self::F35 => "F35",
            Self::Space => "空格",
            Self::Escape => "Esc",
            Self::Enter => "回车",
            Self::NumpadEnter => "小键盘回车",
            Self::Tab => "Tab",
            Self::Backspace => "退格",
            Self::Delete => "Del",
            Self::Insert => "Ins",
            Self::Home => "Home",
            Self::End => "End",
            Self::PageUp => "PgUp",
            Self::PageDown => "PgDn",
            Self::ArrowUp => "↑",
            Self::ArrowDown => "↓",
            Self::ArrowLeft => "←",
            Self::ArrowRight => "→",
            Self::ShiftLeft => "Shift 左",
            Self::ShiftRight => "Shift 右",
            Self::ControlLeft => "Ctrl 左",
            Self::ControlRight => "Ctrl 右",
            Self::AltLeft => "Alt 左",
            Self::AltRight => "Alt 右",
            Self::SuperLeft => "Win 左",
            Self::SuperRight => "Win 右",
            Self::CapsLock => "CapsLock",
            Self::NumLock => "NumLock",
            Self::ScrollLock => "ScrollLock",
            Self::PrintScreen => "PrtSc",
            Self::Pause => "Pause",
            Self::ContextMenu => "菜单键",
            Self::Backquote => "`",
            Self::Minus => "-",
            Self::Equal => "=",
            Self::BracketLeft => "[",
            Self::BracketRight => "]",
            Self::Backslash => "\\",
            Self::Semicolon => ";",
            Self::Quote => "'",
            Self::Comma => ",",
            Self::Period => ".",
            Self::Slash => "/",
            Self::IntlBackslash => "\\ (ISO)",
            Self::IntlRo => "ろ",
            Self::IntlYen => "¥",
            Self::NumpadAdd => "小键盘 +",
            Self::NumpadSubtract => "小键盘 -",
            Self::NumpadMultiply => "小键盘 *",
            Self::NumpadDivide => "小键盘 /",
            Self::NumpadDecimal => "小键盘 .",
            Self::NumpadComma => "小键盘 ,",
            Self::NumpadEqual => "小键盘 =",
            Self::NumpadStar => "小键盘 *（星号）",
            other => other.to_text(),
        }
    }

    /// 把**字符**解析为物理键（`input.is_down("w")` 与 v1 设置文件的兼容路径）。
    ///
    /// 按 **US-QWERTY 位置**换算：字符是「在那个布局下会产生该字符的键」。因此
    /// 这条路径**依赖布局假设**，只用于兼容入口；宿主事件一律走物理键
    /// （[`crate::host::keymap`]），不受玩家布局影响。无法对应到任何物理键时返回
    /// `None`——调用方必须报错，不得回落到某个「未知键」（旧实现在这里静默落
    /// `Other(0)`，导致拼错的键名与真实按键无法区分）。
    ///
    /// **带 Shift 的字符落回未按 Shift 的物理键**：`'+'`→`Equal`（`=` 那个键）、
    /// `':'`→`Semicolon`、`'!'`→`Digit1`……因为物理键身份**不含 Shift**（Shift 是
    /// 独立键位，将来可作修饰）。这与 v1 设置文件迁移（[`Self::v1_key_code`]）同一
    /// 口径，迁移时会对这种换算产出诊断。
    #[must_use]
    pub const fn from_char(character: char) -> Option<Self> {
        match character {
            'a' | 'A' => Some(Self::KeyA),
            'b' | 'B' => Some(Self::KeyB),
            'c' | 'C' => Some(Self::KeyC),
            'd' | 'D' => Some(Self::KeyD),
            'e' | 'E' => Some(Self::KeyE),
            'f' | 'F' => Some(Self::KeyF),
            'g' | 'G' => Some(Self::KeyG),
            'h' | 'H' => Some(Self::KeyH),
            'i' | 'I' => Some(Self::KeyI),
            'j' | 'J' => Some(Self::KeyJ),
            'k' | 'K' => Some(Self::KeyK),
            'l' | 'L' => Some(Self::KeyL),
            'm' | 'M' => Some(Self::KeyM),
            'n' | 'N' => Some(Self::KeyN),
            'o' | 'O' => Some(Self::KeyO),
            'p' | 'P' => Some(Self::KeyP),
            'q' | 'Q' => Some(Self::KeyQ),
            'r' | 'R' => Some(Self::KeyR),
            's' | 'S' => Some(Self::KeyS),
            't' | 'T' => Some(Self::KeyT),
            'u' | 'U' => Some(Self::KeyU),
            'v' | 'V' => Some(Self::KeyV),
            'w' | 'W' => Some(Self::KeyW),
            'x' | 'X' => Some(Self::KeyX),
            'y' | 'Y' => Some(Self::KeyY),
            'z' | 'Z' => Some(Self::KeyZ),
            '0' => Some(Self::Digit0),
            '1' => Some(Self::Digit1),
            '2' => Some(Self::Digit2),
            '3' => Some(Self::Digit3),
            '4' => Some(Self::Digit4),
            '5' => Some(Self::Digit5),
            '6' => Some(Self::Digit6),
            '7' => Some(Self::Digit7),
            '8' => Some(Self::Digit8),
            '9' => Some(Self::Digit9),
            ' ' => Some(Self::Space),
            '-' => Some(Self::Minus),
            '=' | '+' => Some(Self::Equal),
            '[' | '{' => Some(Self::BracketLeft),
            ']' | '}' => Some(Self::BracketRight),
            '\\' | '|' => Some(Self::Backslash),
            ';' | ':' => Some(Self::Semicolon),
            '\'' | '"' => Some(Self::Quote),
            ',' | '<' => Some(Self::Comma),
            '.' | '>' => Some(Self::Period),
            '/' | '?' => Some(Self::Slash),
            '`' | '~' => Some(Self::Backquote),
            ')' => Some(Self::Digit0),
            '!' => Some(Self::Digit1),
            _ => {
                // `@`/`#`/`$`/`%`/`^`/`&`/`*`/`(` 是 Shift + 数字键。
                if let Some(digit) = shift_digit_key(character) {
                    Some(digit)
                } else if character == '_' {
                    Some(Self::Minus)
                } else {
                    None
                }
            }
        }
    }

    /// 该物理键在 **US-QWERTY** 下的「未按 Shift」字符（[`Self::from_char`] 的逆）。
    ///
    /// 只用于 v1 设置文件迁移的诊断文本，不参与运行期判定。
    #[must_use]
    pub const fn qwerty_character(self) -> Option<char> {
        match self {
            Self::KeyA => Some('a'),
            Self::KeyB => Some('b'),
            Self::KeyC => Some('c'),
            Self::KeyD => Some('d'),
            Self::KeyE => Some('e'),
            Self::KeyF => Some('f'),
            Self::KeyG => Some('g'),
            Self::KeyH => Some('h'),
            Self::KeyI => Some('i'),
            Self::KeyJ => Some('j'),
            Self::KeyK => Some('k'),
            Self::KeyL => Some('l'),
            Self::KeyM => Some('m'),
            Self::KeyN => Some('n'),
            Self::KeyO => Some('o'),
            Self::KeyP => Some('p'),
            Self::KeyQ => Some('q'),
            Self::KeyR => Some('r'),
            Self::KeyS => Some('s'),
            Self::KeyT => Some('t'),
            Self::KeyU => Some('u'),
            Self::KeyV => Some('v'),
            Self::KeyW => Some('w'),
            Self::KeyX => Some('x'),
            Self::KeyY => Some('y'),
            Self::KeyZ => Some('z'),
            Self::Digit0 => Some('0'),
            Self::Digit1 => Some('1'),
            Self::Digit2 => Some('2'),
            Self::Digit3 => Some('3'),
            Self::Digit4 => Some('4'),
            Self::Digit5 => Some('5'),
            Self::Digit6 => Some('6'),
            Self::Digit7 => Some('7'),
            Self::Digit8 => Some('8'),
            Self::Digit9 => Some('9'),
            Self::Space => Some(' '),
            Self::Minus => Some('-'),
            Self::Equal => Some('='),
            Self::BracketLeft => Some('['),
            Self::BracketRight => Some(']'),
            Self::Backslash => Some('\\'),
            Self::Semicolon => Some(';'),
            Self::Quote => Some('\''),
            Self::Comma => Some(','),
            Self::Period => Some('.'),
            Self::Slash => Some('/'),
            Self::Backquote => Some('`'),
            _ => None,
        }
    }

    /// v1 设置文件里「字符形式」→ 物理键的**唯一**换算表（M22/W1 迁移路径）。
    ///
    /// v1 把键编码成字符：`key:e`、`key:1`、`key:up`、`key:+`、`key:other:1048577`。
    /// 字符与物理键**不是一一对应**（v1 本身就是这么丢信息的），换算只能按
    /// US-QWERTY 猜：
    ///
    /// * 字母/数字/未按 Shift 的符号 → 对应物理键；
    /// * `+`/`:`/`?`/`!`/`{`/`}`/`|` 这类**带 Shift 的字符**在同一次迁移里只能落回
    ///   未按 Shift 的物理键（`+` → `Equal` 即 `=` 那个键）。这是唯一的**有损**
    ///   映射，返回值第二项是给玩家的中文诊断（见 [`ActionMap::from_text_with_notes`]）；
    /// * `other:<码位>` 只识别仓库里真实产生过的方向键码位与 F1/F2/F4。
    ///
    /// 返回 `None` = 该文本在 v1 里也没有对应物理键 → 调用方报错，**不静默丢弃**。
    #[must_use]
    pub fn v1_key_code(text: &str, notes: &mut Vec<String>) -> Option<Self> {
        let lowered = text.trim().to_ascii_lowercase();
        // 方向键：v1 的可读名与内部码位两种写法。
        let named = match lowered.as_str() {
            "space" => Some(Self::Space),
            "escape" | "esc" => Some(Self::Escape),
            "up" => Some(Self::ArrowUp),
            "down" => Some(Self::ArrowDown),
            "left" => Some(Self::ArrowLeft),
            "right" => Some(Self::ArrowRight),
            "f1" => Some(Self::F1),
            "f2" => Some(Self::F2),
            "f4" => Some(Self::F4),
            _ => None,
        };
        if let Some(code) = named {
            return Some(code);
        }
        if let Some(index) = lowered.strip_prefix("other:") {
            // v1 的内部码位：方向键 0x0010_0001..4 与 F1/F2/F4（1/2/4）。
            return match index.trim().parse::<u32>().ok() {
                Some(0x0010_0001) => Some(Self::ArrowUp),
                Some(0x0010_0002) => Some(Self::ArrowDown),
                Some(0x0010_0003) => Some(Self::ArrowLeft),
                Some(0x0010_0004) => Some(Self::ArrowRight),
                Some(1) => Some(Self::F1),
                Some(2) => Some(Self::F2),
                Some(4) => Some(Self::F4),
                _ => None,
            };
        }
        // 单字符：带 Shift 的字符先记诊断（v1 没有独立的 Shift 键身份），
        // 其余交给 `from_char` 的 US-QWERTY 换算。
        let mut chars = lowered.chars();
        let (Some(character), None) = (chars.next(), chars.next()) else {
            return None;
        };
        let core = Self::from_char(character)?;
        if shift_digit_key(character).is_some() || matches!(character, '_' | '<' | '>' | '"') {
            // 这些字符在 US-QWERTY 上**没有**独立的未按 Shift 写法（`_` 是 Shift+`-`，
            // `<`/`>` 是 Shift+`,`/`.），v1 却把它们当独立键位，只能落回物理键。
            notes.push(shifted_note(text, core));
            return Some(core);
        }
        if let Some(unshifted) = core.qwerty_character()
            && unshifted != character
        {
            notes.push(shifted_note(text, core));
        }
        Some(core)
    }
}

/// `v1_key_code` 的迁移诊断文本（`+` 落到 `=` 键这类有损换算）。
fn shifted_note(text: &str, core: KeyCode) -> String {
    format!(
        "key:{text} 是 v1 的带 Shift 字符写法，v2 只记录物理键，已迁移为 key:{}（{} 键）；\
         如需保留原字符请改绑为 Shift+{} 组合",
        core.to_text(),
        core.display_label(),
        core.display_label(),
    )
}

/// Shift + 主键区数字产生的字符 → 对应数字物理键（`!`→`Digit1`、`@`→`Digit2`…）。
///
/// US-QWERTY 的固定对应关系；只用于**字符 → 物理键**的兼容换算与迁移诊断。
#[must_use]
pub const fn shift_digit_key(character: char) -> Option<KeyCode> {
    match character {
        '!' => Some(KeyCode::Digit1),
        '@' => Some(KeyCode::Digit2),
        '#' => Some(KeyCode::Digit3),
        '$' => Some(KeyCode::Digit4),
        '%' => Some(KeyCode::Digit5),
        '^' => Some(KeyCode::Digit6),
        '&' => Some(KeyCode::Digit7),
        '*' => Some(KeyCode::Digit8),
        '(' => Some(KeyCode::Digit9),
        ')' => Some(KeyCode::Digit0),
        _ => None,
    }
}
/// Mouse buttons exposed by the engine input layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Other(u16),
}

/// 手柄轴方向（M20-02 抽象手柄绑定用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AxisDirection {
    Positive,
    Negative,
}

impl AxisDirection {
    /// 方向符号（+1 / -1）。
    #[must_use]
    pub const fn sign(self) -> f32 {
        match self {
            Self::Positive => 1.0,
            Self::Negative => -1.0,
        }
    }

    /// 文本形式（诊断与测试用）。
    #[must_use]
    pub const fn to_text(self) -> &'static str {
        match self {
            Self::Positive => "+",
            Self::Negative => "-",
        }
    }
}

/// 最近一次产生输入的**设备类别**（M22/W3）。
///
/// 键鼠同属 [`InputDevice::Keyboard`]（UI 提示只需要区分「键鼠说明」与「手柄说明」）。
/// 由宿主/设备层在**真实边沿**发生时写入，而不是每帧根据「有轴值」推断——手柄空闲
/// 时轴也每帧上报 0.0，按值推断会把提示永远锁在手柄上。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum InputDevice {
    /// 键盘或鼠标。
    #[default]
    Keyboard,
    /// 抽象手柄（按钮或摇杆）。
    Gamepad,
}

impl InputDevice {
    /// Lua `input.last_device()` 的返回文本。
    #[must_use]
    pub const fn to_text(self) -> &'static str {
        match self {
            Self::Keyboard => "keyboard",
            Self::Gamepad => "gamepad",
        }
    }
}

/// 一个命名动作的绑定项（M20-02；M22/W1 起键位是**物理键**）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Binding {
    /// 物理键（[`KeyCode`]）——不是字符，见 `KeyCode` 的模块说明。
    Key(KeyCode),
    Mouse(MouseButton),
    /// 抽象手柄按钮索引（宿主喂入；设备轮询未接入）。
    GamepadButton(u8),
    /// 抽象手柄轴索引 + 方向（宿主喂入；设备轮询未接入）。
    GamepadAxis(u8, AxisDirection),
}

impl Binding {
    /// 稳定文本形式（设置持久化：`key:KeyE`、`mouse:left`、`pad:0`、`padaxis:1+`）。
    ///
    /// M22/W1 起键位文本是**物理键名**（[`KeyCode::to_text`]，即 winit 变体名的小写）
    /// 而不是字符：v1 的 `key:e` 在 AZERTY 上会被玩家理解成「字符 e 那个键」，而
    /// 引擎实际需要的是 US-QWERTY 的 E 位置。展示用文本见
    /// [`Self::display_label`]，两者刻意分开。
    #[must_use]
    pub fn to_text(self) -> String {
        match self {
            Self::Key(key) => format!("key:{}", key.to_text()),
            Self::Mouse(MouseButton::Left) => "mouse:left".to_owned(),
            Self::Mouse(MouseButton::Right) => "mouse:right".to_owned(),
            Self::Mouse(MouseButton::Middle) => "mouse:middle".to_owned(),
            Self::Mouse(MouseButton::Other(index)) => format!("mouse:other:{index}"),
            Self::GamepadButton(index) => format!("pad:{index}"),
            Self::GamepadAxis(index, AxisDirection::Positive) => format!("padaxis:{index}+"),
            Self::GamepadAxis(index, AxisDirection::Negative) => format!("padaxis:{index}-"),
        }
    }

    /// 人可读标签（编辑器重绑定面板、冲突提示、日志）。
    #[must_use]
    pub fn display_label(self) -> String {
        match self {
            Self::Key(key) => key.display_label().to_owned(),
            Self::Mouse(MouseButton::Left) => "鼠标左键".to_owned(),
            Self::Mouse(MouseButton::Right) => "鼠标右键".to_owned(),
            Self::Mouse(MouseButton::Middle) => "鼠标中键".to_owned(),
            Self::Mouse(MouseButton::Other(index)) => format!("鼠标 {index}"),
            Self::GamepadButton(index) => format!("手柄按钮 {index}"),
            Self::GamepadAxis(index, direction) => {
                format!("手柄轴 {index}{}", direction.to_text())
            }
        }
    }

    /// 解析 [`Self::to_text`] 的输出，同时接受 **v1 的键位写法**。
    ///
    /// 键位字段先按 v2 物理键名解析，失败再走 [`KeyCode::v1_key_code`]（`key:e`、
    /// `key:1`、`key:up`、`key:other:1048577`、`key:+`），因此
    /// `input.rebind("interact", "key:e")` 这类既有 Lua 调用与 v1 设置文件都能继续
    /// 工作。v1 的带 Shift 字符（`+`）在迁移时落回未按 Shift 的物理键，诊断文本由
    /// [`ActionMap::from_text_with_notes`] 产出（本函数静默，因为它是单条绑定的
    /// 兼容入口，没有「迁移」这个语境）。
    pub fn parse(text: &str) -> Result<Self, ActionMapError> {
        let (kind, rest) = text
            .split_once(':')
            .ok_or_else(|| ActionMapError::InvalidBinding(text.to_owned()))?;
        match (kind, rest) {
            ("key", value) => {
                if let Some(key) = KeyCode::from_text(value) {
                    return Ok(Self::Key(key));
                }
                let mut notes = Vec::new();
                KeyCode::v1_key_code(value, &mut notes)
                    .map(Self::Key)
                    .ok_or_else(|| ActionMapError::InvalidBinding(text.to_owned()))
            }
            ("mouse", value) => match value {
                "left" => Ok(Self::Mouse(MouseButton::Left)),
                "right" => Ok(Self::Mouse(MouseButton::Right)),
                "middle" => Ok(Self::Mouse(MouseButton::Middle)),
                other => other
                    .strip_prefix("other:")
                    .and_then(|index| index.parse::<u16>().ok())
                    .map(|index| Self::Mouse(MouseButton::Other(index)))
                    .ok_or_else(|| ActionMapError::InvalidBinding(text.to_owned())),
            },
            ("pad", value) => value
                .parse::<u8>()
                .map(Self::GamepadButton)
                .map_err(|_| ActionMapError::InvalidBinding(text.to_owned())),
            ("padaxis", value) => {
                let (index, sign) = value
                    .split_at_checked(value.len().saturating_sub(1))
                    .ok_or_else(|| ActionMapError::InvalidBinding(text.to_owned()))?;
                let index = index
                    .parse::<u8>()
                    .map_err(|_| ActionMapError::InvalidBinding(text.to_owned()))?;
                match sign {
                    "+" => Ok(Self::GamepadAxis(index, AxisDirection::Positive)),
                    "-" => Ok(Self::GamepadAxis(index, AxisDirection::Negative)),
                    _ => Err(ActionMapError::InvalidBinding(text.to_owned())),
                }
            }
            _ => Err(ActionMapError::InvalidBinding(text.to_owned())),
        }
    }
}

/// 动作映射错误（重绑定/设置持久化）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionMapError {
    EmptyAction,
    DuplicateBinding { action: String, binding: String },
    InvalidBinding(String),
    UnsupportedVersion(String),
}

impl std::fmt::Display for ActionMapError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyAction => formatter.write_str("action name must not be empty"),
            Self::DuplicateBinding { action, binding } => {
                write!(formatter, "action {action} already binds {binding}")
            }
            Self::InvalidBinding(text) => write!(formatter, "invalid binding: {text}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported action map version: {version}")
            }
        }
    }
}

impl std::error::Error for ActionMapError {}

/// 跨动作冲突：一个物理输入被两个及以上动作占用（M20-08）。
///
/// `actions` 至少两项，按动作名字典序（[`ActionMap::actions`] 的顺序）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionConflict {
    /// 被重复占用的物理输入。
    pub binding: Binding,
    /// 占用它的动作名（≥2 项）。
    pub actions: Vec<String>,
}

impl ActionConflict {
    /// 人可读的一行诊断（编辑器面板与日志共用；中文与 `ActionMapError` 的英文
    /// 消息分开，前者面向玩家、后者面向 API 调用方）。
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "{} 被多个动作同时绑定：{}（请先解绑其中一个）",
            self.binding.to_text(),
            self.actions.join("、")
        )
    }

    /// 转成既有的 [`ActionMapError::DuplicateBinding`]，便于复用同一诊断出口。
    #[must_use]
    pub fn as_error(&self) -> ActionMapError {
        ActionMapError::DuplicateBinding {
            action: self
                .actions
                .first()
                .cloned()
                .unwrap_or_else(|| "unknown".to_owned()),
            binding: self.binding.to_text(),
        }
    }
}

/// 动作映射的持久化版本头（M22/W1：v1 → v2）。
///
/// v1 的键位是**字符**（`key:e`、`key:1`、`key:up`、`key:+`、`key:other:1048577`），
/// v2 是**物理键名**（`key:keye`、`key:digit1`、`key:arrowup`、`key:equal`）。写入
/// 一律输出 v2；读取同时接受 v1（[`ActionMap::from_text_with_notes`] 给出迁移诊断），
/// 因此玩家在 v1 里的改键不会静默丢失或错绑。
pub const ACTION_MAP_VERSION: &str = "rustlua-actions-v2";

/// 兼容读取的 v1 版本头（只读，不再写出）。
pub const ACTION_MAP_VERSION_V1: &str = "rustlua-actions-v1";

/// 命名动作 → 绑定集合（M20-02）。表内按动作名稳定排序，便于对比与展示。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActionMap {
    actions: BTreeMap<String, Vec<Binding>>,
}

impl ActionMap {
    /// 空映射。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 引擎默认映射：与当前 `game_jhyt` 键位一致，并附基础手柄默认绑定。
    ///
    /// 游戏/编辑器用 [`Self::merge_missing`] 把它作为「未重绑定项的兜底」，
    /// 保证用户重绑定后其余动作仍可玩。
    ///
    /// M20-08 扩表：`game_jhyt` 剩余的物理键直读（建造 B / 放置 F / 拆除 X /
    /// 使用·升级 U / 任务 L / 重开 R / 选种子 C / 音量 ± / 数字键直选 1..9）
    /// 全部升级为命名动作，玩家在编辑器「项目设置 → 输入」里可以整体改键。
    /// 数字键用 `slot_1..slot_9` 而不是一个动作多绑定，因为「按了第几个」本身
    /// 就是动作语义。
    ///
    /// **每个物理输入只出现在一个动作里**（唯一例外是同一动作的多绑定）：
    /// 否则默认表自身就会命中 [`Self::conflicts`]，编辑器一打开面板就报冲突。
    /// 游戏里 F/U/Q/I/P/L 等键的**上下文复用**（同一动作在不同模式下有不同
    /// 语义）由玩法层决定，不在这里重复绑定。
    #[must_use]
    pub fn with_defaults() -> Self {
        let mut map = Self::new();
        let defaults: &[(&str, &[Binding])] = &[
            ("move_up", &[key('w')]),
            ("move_down", &[key('s')]),
            ("move_left", &[key('a')]),
            ("move_right", &[key('d')]),
            ("interact", &[key('e')]),
            ("attack", &[key('j')]),
            ("skill", &[key('k')]),
            ("gift", &[key('g')]),
            ("sleep", &[key('t')]),
            ("practice", &[key('q')]),
            ("inventory", &[key('i')]),
            ("pause", &[key('p')]),
            // 建造/采集/任务/重开（M20-08）：game_jhyt 迁移动作层后的新动作。
            ("build", &[key('b')]),
            ("place", &[key('f')]),
            ("demolish", &[key('x')]),
            ("use", &[key('u')]),
            ("quest", &[key('l')]),
            ("respawn", &[key('r')]),
            ("seed", &[key('c')]),
            // 音量：`+`/`-` 是**字符**语义（v1 的写法），落到 US-QWERTY 的 `=`/`-`
            // 两个物理键上。玩家在 AZERTY 上按到的是同一个物理位置，这正是 W1 想
            // 要的：默认绑定不再随布局漂移。
            ("volume_up", &[key('+')]),
            ("volume_down", &[key('-')]),
            // 数字键 1..9 直选（对话选项/商店行/面板行）——键盘专属快捷方式，
            // 手柄侧走 ui_* + confirm。
            ("slot_1", &[key('1')]),
            ("slot_2", &[key('2')]),
            ("slot_3", &[key('3')]),
            ("slot_4", &[key('4')]),
            ("slot_5", &[key('5')]),
            ("slot_6", &[key('6')]),
            ("slot_7", &[key('7')]),
            ("slot_8", &[key('8')]),
            ("slot_9", &[key('9')]),
            (
                "confirm",
                &[Binding::Key(KeyCode::Space), Binding::GamepadButton(0)],
            ),
            (
                "cancel",
                &[Binding::Key(KeyCode::Escape), Binding::GamepadButton(1)],
            ),
            // 菜单/焦点导航（M20-02 面板 + M20-07 动作层）：方向键与左摇杆
            // **按方向拆成四个动作**，这样 `input.action_axis("ui_down","ui_up")`
            // 能产出带符号的模拟量（把两个方向塞进同一动作会让正负相互抵消；
            // `action_axis` 的**首参是负方向动作**，垂直轴即 down→up）。
            // M20-08 追加 DPad 按钮 12..15（M20-06 的设备契约），键盘手感不变。
            (
                "ui_up",
                &[
                    Binding::Key(KeyCode::ArrowUp),
                    Binding::GamepadAxis(1, AxisDirection::Positive),
                    Binding::GamepadButton(12),
                ],
            ),
            (
                "ui_down",
                &[
                    Binding::Key(KeyCode::ArrowDown),
                    Binding::GamepadAxis(1, AxisDirection::Negative),
                    Binding::GamepadButton(13),
                ],
            ),
            (
                "ui_left",
                &[
                    Binding::Key(KeyCode::ArrowLeft),
                    Binding::GamepadAxis(0, AxisDirection::Negative),
                    Binding::GamepadButton(14),
                ],
            ),
            (
                "ui_right",
                &[
                    Binding::Key(KeyCode::ArrowRight),
                    Binding::GamepadAxis(0, AxisDirection::Positive),
                    Binding::GamepadButton(15),
                ],
            ),
        ];
        for &(action, bindings) in defaults {
            // bindings 是 &&[Binding]（defaults 本身是切片引用），这里要 iter() 自动解引用，
            // 直接 `for binding in bindings` 会编不过（&&[T] 不是 IntoIterator）。
            for binding in bindings.iter() {
                // 默认表由常量构造，重复即代码缺陷；忽略返回值会让问题静默。
                map.actions
                    .entry(action.to_owned())
                    .or_default()
                    .push(*binding);
            }
        }
        debug_assert!(
            map.conflicts().is_empty(),
            "默认表不得含跨动作重复绑定：{:?}",
            map.conflicts()
        );
        map
    }

    /// 跨动作的物理输入冲突：同一绑定被两个及以上动作占用（M20-08）。
    ///
    /// 与 [`Self::bind`] 的 `DuplicateBinding`（同一动作内重复绑定同一输入）互补：
    /// 那个是「无意义重复」，这个是「同一按键触发两个动作」，玩法上几乎总是配置
    /// 错误，编辑器重绑定面板必须能看见并阻止保存。返回顺序按绑定文本排序，
    /// 保证 UI/测试稳定。
    #[must_use]
    pub fn conflicts(&self) -> Vec<ActionConflict> {
        // `Binding` 没有 `Ord`：先用文本做键（也顺便让输出有序），再映射回绑定本身。
        let mut by_text: BTreeMap<String, (Binding, Vec<String>)> = BTreeMap::new();
        for action in self.actions() {
            for binding in self.bindings(action) {
                let entry = by_text
                    .entry(binding.to_text())
                    .or_insert_with(|| (*binding, Vec::new()));
                entry.1.push(action.to_owned());
            }
        }
        by_text
            .into_values()
            .filter(|(_, actions)| actions.len() > 1)
            .map(|(binding, actions)| ActionConflict { binding, actions })
            .collect()
    }

    /// 保存前校验：存在跨动作冲突时返回首个冲突（可读诊断）——沿用
    /// [`ActionMapError::DuplicateBinding`]，消息形态与 `input.rebind` 一致。
    pub fn validate_bindings(&self) -> Result<(), ActionMapError> {
        match self.conflicts().into_iter().next() {
            None => Ok(()),
            Some(conflict) => Err(conflict.as_error()),
        }
    }

    /// 绑定一个物理输入到动作；同一动作重复绑定同一输入报错。
    pub fn bind(&mut self, action: &str, binding: Binding) -> Result<(), ActionMapError> {
        if action.trim().is_empty() {
            return Err(ActionMapError::EmptyAction);
        }
        let bindings = self.actions.entry(action.to_owned()).or_default();
        if bindings.contains(&binding) {
            return Err(ActionMapError::DuplicateBinding {
                action: action.to_owned(),
                binding: binding.to_text(),
            });
        }
        bindings.push(binding);
        Ok(())
    }

    /// 解绑；返回是否确实移除。
    pub fn unbind(&mut self, action: &str, binding: Binding) -> bool {
        let Some(bindings) = self.actions.get_mut(action) else {
            return false;
        };
        let before = bindings.len();
        bindings.retain(|candidate| *candidate != binding);
        let removed = bindings.len() != before;
        if bindings.is_empty() {
            self.actions.remove(action);
        }
        removed
    }

    /// 清空一个动作的全部绑定。
    pub fn clear(&mut self, action: &str) {
        self.actions.remove(action);
    }

    /// 动作的绑定列表（未定义时为空）。
    #[must_use]
    pub fn bindings(&self, action: &str) -> &[Binding] {
        self.actions.get(action).map_or(&[], Vec::as_slice)
    }

    /// 全部动作名（字典序）。
    #[must_use]
    pub fn actions(&self) -> Vec<&str> {
        self.actions.keys().map(String::as_str).collect()
    }

    /// 是否没有任何绑定。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    /// 仅补齐缺失动作（不覆盖用户已有的重绑定）——用于「恢复默认」之外的兜底。
    pub fn merge_missing(&mut self, defaults: &ActionMap) {
        for (action, bindings) in &defaults.actions {
            self.actions
                .entry(action.clone())
                .or_insert_with(|| bindings.clone());
        }
    }

    /// 序列化为设置文本（含版本头，逐行 `action=binding,binding`）。
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut text = String::from(ACTION_MAP_VERSION);
        for (action, bindings) in &self.actions {
            if bindings.is_empty() {
                continue;
            }
            let list = bindings
                .iter()
                .map(|binding| binding.to_text())
                .collect::<Vec<_>>()
                .join(",");
            text.push('\n');
            text.push_str(action);
            text.push('=');
            text.push_str(&list);
        }
        text
    }

    /// 解析 [`Self::to_text`] 的输出（v2 头）或 **v1 设置文本**（`rustlua-actions-v1`）。
    ///
    /// v1 会被迁移成物理键并**丢弃迁移诊断**；需要诊断（「`+` 落到了 `=` 键」这类
    /// 有损换算）请用 [`Self::from_text_with_notes`]，Lua 侧 `input.load_settings()`
    /// 走的就是带诊断的那条路径。
    pub fn from_text(text: &str) -> Result<Self, ActionMapError> {
        Self::from_text_with_notes(text).map(|(map, _)| map)
    }

    /// 同 [`Self::from_text`]，同时返回**迁移诊断**（每条一行中文，v2 输入时为空）。
    ///
    /// 版本头判定：`v2` = 当前格式；`v1` = 旧格式，键位按
    /// [`KeyCode::v1_key_code`] 换算后读入（**读 v1、写 v2**）；其他头一律
    /// [`ActionMapError::UnsupportedVersion`]。
    pub fn from_text_with_notes(text: &str) -> Result<(Self, Vec<String>), ActionMapError> {
        let mut lines = text.lines();
        let header = lines.next().unwrap_or_default().trim();
        let legacy = match header {
            ACTION_MAP_VERSION => false,
            ACTION_MAP_VERSION_V1 => true,
            other => return Err(ActionMapError::UnsupportedVersion(other.to_owned())),
        };
        let mut notes = Vec::new();
        let mut map = Self::new();
        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let (action, list) = line
                .split_once('=')
                .ok_or_else(|| ActionMapError::InvalidBinding(line.to_owned()))?;
            for item in list.split(',').filter(|item| !item.trim().is_empty()) {
                let item = item.trim();
                let binding = if legacy {
                    v1_binding(item, &mut notes)?
                } else {
                    Binding::parse(item)?
                };
                map.bind(action.trim(), binding)?;
            }
        }
        if legacy && notes.is_empty() {
            // 即使没有单项诊断，也要让玩家知道这份文件已经被升级：v1 的键位名
            // （`key:e`）与 v2（`key:keye`）读起来不同，下次写盘就是 v2 了。
            notes.push(
                "输入映射已从 v1 升级到 v2：键位改为物理键名，写盘后即输出 v2 格式".to_owned(),
            );
        }
        Ok((map, notes))
    }
}

/// v1 绑定文本 → v2 [`Binding`]（迁移路径，见 [`ActionMap::from_text_with_notes`]）。
///
/// v1 与 v2 只有**键位**字段不同：`mouse:*`/`pad:*`/`padaxis:*` 三种写法两版一致，
/// 直接交给 [`Binding::parse`]。键位走 [`KeyCode::v1_key_code`]，无法换算时报
/// [`ActionMapError::InvalidBinding`]——**不静默丢弃**（旧实现会落 `Other(0)`，
/// 于是拼错的键名与真按键无法区分）。
fn v1_binding(text: &str, notes: &mut Vec<String>) -> Result<Binding, ActionMapError> {
    let Some(value) = text.strip_prefix("key:") else {
        return Binding::parse(text);
    };
    KeyCode::v1_key_code(value, notes)
        .map(Binding::Key)
        .ok_or_else(|| ActionMapError::InvalidBinding(text.to_owned()))
}

/// 默认绑定表用的常量构造助手：把 **US-QWERTY 字符**换算成物理键绑定。
///
/// 默认表是常量表，用 `key('e')` 比逐个写 `Binding::Key(KeyCode::KeyE)` 更难写错，
/// 也把「默认绑定按 QWERTY 位置定义」这一事实写在构造点上。传入的字符全部来自本
/// 文件里写死的字面量（不是用户输入），因此这里的 `expect` 是**编译期常量错误**
/// 的运行时兜底，正常路径永不触发；`default_binding_characters_map_to_physical_keys`
/// 单测逐个验证这些字面量确实能换算。
fn key(character: char) -> Binding {
    Binding::Key(
        KeyCode::from_char(character)
            .unwrap_or_else(|| panic!("默认绑定表里的字符 {character:?} 没有对应物理键")),
    )
}

/// 谓词（模拟量）：手柄轴取模拟值，其余绑定为 0/1。
fn binding_axis_value(
    binding: Binding,
    held_keys: &HashSet<KeyCode>,
    held_mouse: &HashSet<MouseButton>,
    gamepad_buttons: &HashSet<u8>,
    gamepad_axes: &HashMap<u8, f32>,
    axis_digital: &HashMap<(u8, AxisDirection), bool>,
) -> f32 {
    match binding {
        Binding::GamepadAxis(index, direction) => {
            let raw = gamepad_axes.get(&index).copied().unwrap_or(0.0);
            (raw * direction.sign()).max(0.0)
        }
        other => f32::from(u8::from(binding_is_down(
            other,
            held_keys,
            held_mouse,
            gamepad_buttons,
            gamepad_axes,
            axis_digital,
        ))),
    }
}

/// 谓词：绑定当前是否处于「按下」状态。
///
/// 手柄轴优先读**滞回数字态**（设备层每帧写入，见
/// [`InputSnapshot::set_gamepad_axis_digital`]）；没有该状态时（测试直接
/// `set_gamepad_axis` 的路径）回落到 `|value| >= AXIS_DIGITAL_ON`，语义与 M20 一致。
fn binding_is_down(
    binding: Binding,
    held_keys: &HashSet<KeyCode>,
    held_mouse: &HashSet<MouseButton>,
    gamepad_buttons: &HashSet<u8>,
    gamepad_axes: &HashMap<u8, f32>,
    axis_digital: &HashMap<(u8, AxisDirection), bool>,
) -> bool {
    match binding {
        Binding::Key(key) => held_keys.contains(&key),
        Binding::Mouse(button) => held_mouse.contains(&button),
        Binding::GamepadButton(index) => gamepad_buttons.contains(&index),
        Binding::GamepadAxis(index, direction) => match axis_digital.get(&(index, direction)) {
            Some(down) => *down,
            None => gamepad_axes
                .get(&index)
                .is_some_and(|value| *value * direction.sign() >= AXIS_DIGITAL_ON),
        },
    }
}

/// A stable input state consumed by simulation and scripts.
#[derive(Debug, Clone, PartialEq)]
pub struct InputSnapshot {
    held_keys: HashSet<KeyCode>,
    pressed_keys: HashSet<KeyCode>,
    released_keys: HashSet<KeyCode>,
    /// 虚拟按键（触屏摇杆注入）
    virtual_keys: HashSet<KeyCode>,
    held_mouse: HashSet<MouseButton>,
    pressed_mouse: HashSet<MouseButton>,
    released_mouse: HashSet<MouseButton>,
    /// 抽象手柄按钮（M20-02：宿主喂入，设备轮询见 M20-06）。
    held_gamepad: HashSet<u8>,
    pressed_gamepad: HashSet<u8>,
    released_gamepad: HashSet<u8>,
    /// 抽象手柄轴值（-1..=1）。
    gamepad_axes: HashMap<u8, f32>,
    /// 轴的**滞回数字态**（M22/W3）：(轴索引, 方向) → 当前是否越过阈值。
    ///
    /// 设备层每帧写全量（与按钮的按住状态同一套约定）；没有条目的 (轴, 方向)
    /// 回落到按模拟值判定，保证直接 `set_gamepad_axis` 的既有调用语义不变。
    axis_digital: HashMap<(u8, AxisDirection), bool>,
    /// 轴的数字按下/松开边沿（M22/W3，只在变化帧写入）。
    pressed_axes: HashSet<(u8, AxisDirection)>,
    released_axes: HashSet<(u8, AxisDirection)>,
    /// 最近一次产生真实输入的设备类别（M22/W3）。
    last_device: InputDevice,
    cursor_position: (f64, f64),
    cursor_delta: (f64, f64),
    scroll_delta: (f32, f32),
    focused: bool,
}

impl Default for InputSnapshot {
    fn default() -> Self {
        Self {
            held_keys: HashSet::new(),
            pressed_keys: HashSet::new(),
            released_keys: HashSet::new(),
            virtual_keys: HashSet::new(),
            held_mouse: HashSet::new(),
            pressed_mouse: HashSet::new(),
            released_mouse: HashSet::new(),
            held_gamepad: HashSet::new(),
            pressed_gamepad: HashSet::new(),
            released_gamepad: HashSet::new(),
            gamepad_axes: HashMap::new(),
            axis_digital: HashMap::new(),
            pressed_axes: HashSet::new(),
            released_axes: HashSet::new(),
            last_device: InputDevice::default(),
            cursor_position: (0.0, 0.0),
            cursor_delta: (0.0, 0.0),
            scroll_delta: (0.0, 0.0),
            focused: true,
        }
    }
}

impl InputSnapshot {
    /// Records a keyboard state transition without losing a quick press/release.
    pub fn key_event(&mut self, key: KeyCode, is_down: bool) {
        if is_down {
            if self.held_keys.insert(key) {
                self.pressed_keys.insert(key);
                // M22/W3：真实按下才切换「最后使用的设备」，供 UI 提示选说明文案。
                self.last_device = InputDevice::Keyboard;
            }
        } else if self.held_keys.remove(&key) {
            self.released_keys.insert(key);
        }
    }

    /// 注入/释放虚拟按键（触屏摇杆用）。
    pub fn set_virtual_key(&mut self, key: KeyCode, down: bool) {
        if down {
            self.virtual_keys.insert(key);
        } else {
            self.virtual_keys.remove(&key);
        }
    }

    /// 虚拟按键产生一次按下边沿。
    pub fn set_virtual_pressed(&mut self, key: KeyCode) {
        self.virtual_keys.insert(key);
        self.pressed_keys.insert(key);
    }

    /// 沿用上一份快照里由 [`Self::set_virtual_key`] 注入的虚拟键按住态（M22 触屏摇杆修复）。
    ///
    /// 虚拟键是**跨帧保持**的按住态：脚本（触屏摇杆/虚拟按钮）注入 `true` 之后每一帧
    /// 不重发，只在抬起时注入 `false`。而宿主每帧都用设备层重新采样的新快照调
    /// [`crate::core::script::ScriptState::set_input`]，整份替换会把上一帧注入的虚拟键
    /// 一起丢掉——摇杆于是永远读回 0（同帧注入、下一帧被清空，`input.action_axis`
    /// 恒为 0，表现为"摇杆能动、角色不动"）。
    ///
    /// 这里把旧快照的虚拟键集合并入新快照；宿主级输入（键/鼠标/手柄/边沿）仍以新快照
    /// 为准，因此不会把物理输入"粘住"。
    pub fn inherit_virtual_keys(&mut self, previous: &Self) {
        self.virtual_keys
            .extend(previous.virtual_keys.iter().copied());
    }

    /// Records a mouse-button state transition without losing a quick click.
    pub fn mouse_event(&mut self, button: MouseButton, is_down: bool) {
        if is_down {
            if self.held_mouse.insert(button) {
                self.pressed_mouse.insert(button);
                self.last_device = InputDevice::Keyboard;
            }
        } else if self.held_mouse.remove(&button) {
            self.released_mouse.insert(button);
        }
    }

    /// Updates the cursor position and accumulates movement until the next frame boundary.
    pub fn cursor_moved(&mut self, x: f64, y: f64) {
        self.cursor_delta.0 += x - self.cursor_position.0;
        self.cursor_delta.1 += y - self.cursor_position.1;
        self.cursor_position = (x, y);
    }

    /// Updates focus. Losing focus clears held buttons to prevent stuck input.
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        if !focused {
            self.held_keys.clear();
            self.held_mouse.clear();
            // M20-02：失焦同时清手柄按住状态，避免焦点回来后「粘键」。
            self.held_gamepad.clear();
            self.gamepad_axes.clear();
            // M22/W3：轴的数字态同样清空（含边沿）；设备层会在下一帧按实际杆位
            // 重建数字态，因此回来后仍按住的杆会重新产生一次按下边沿。
            self.axis_digital.clear();
            self.pressed_axes.clear();
            self.released_axes.clear();
        }
    }

    /// 喂入抽象手柄按钮状态（M20-02；设备轮询由宿主负责）。
    ///
    /// 语义是**帧内事件**：从松开到按下产生 `just_pressed`，按下到松开产生
    /// `just_released`。宿主每帧重建快照时（编辑器 Play 预览），重复喂「按住」
    /// 会反复产生按下边沿；这种「每帧断言当前按住状态」请用
    /// [`Self::set_gamepad_button_held`]。
    pub fn gamepad_button_event(&mut self, button: u8, is_down: bool) {
        if is_down {
            if self.held_gamepad.insert(button) {
                self.pressed_gamepad.insert(button);
                // M22/W3：真实按下才切换设备类别（空闲帧不写，避免提示被锁死）。
                self.last_device = InputDevice::Gamepad;
            }
        } else {
            self.held_gamepad.remove(&button);
            // 宿主每帧重建快照时「按住」状态本来就是空的，此时仍必须产生松开边沿；
            // 重复喂同一松开事件会重复触发，由设备层只在变化帧喂事件来保证唯一。
            self.released_gamepad.insert(button);
        }
    }

    /// 直接设置手柄按钮的**按住状态**（不产生按下/松开边沿）。
    ///
    /// `down = true` 只保证 `gamepad_button`/`action_is_down` 为真；`down = false`
    /// 清掉按住状态但不产生 `just_released`（松开边沿由
    /// [`Self::gamepad_button_event`] 负责）。设备层用它在同一帧里「重建」持续
    /// 按住的按钮，避免每帧重复触发一次性动作。
    pub fn set_gamepad_button_held(&mut self, button: u8, down: bool) {
        if down {
            self.held_gamepad.insert(button);
        } else {
            self.held_gamepad.remove(&button);
        }
    }

    /// 喂入抽象手柄轴值（钳制到 -1..=1）。
    ///
    /// M22/W3 起轴也有**数字态与边沿**：本方法只写模拟量，
    /// [`Self::set_gamepad_axis_digital`] / [`Self::gamepad_axis_event`] 负责数字态
    /// 与边沿（设备层每帧调用，见 `host::gamepad::GamepadPoller`）。只调用本方法的
    /// 既有测试路径按 `|value| >= AXIS_DIGITAL_ON` 回落判定，语义不变。
    pub fn set_gamepad_axis(&mut self, axis: u8, value: f32) {
        self.gamepad_axes.insert(axis, value.clamp(-1.0, 1.0));
    }

    /// 直接设置轴的**数字按住状态**（不产生边沿；设备层每帧写全量）。
    pub fn set_gamepad_axis_digital(&mut self, axis: u8, direction: AxisDirection, down: bool) {
        self.axis_digital.insert((axis, direction), down);
    }

    /// 喂入轴的按下/松开**边沿**（M22/W3；设备层只在变化帧调用）。
    ///
    /// 同时更新数字按住状态与边沿集合：`is_down = true` 产生 `just_pressed`，
    /// `false` 产生 `just_released`。语义与 [`Self::gamepad_button_event`] 一致
    /// （快速推杆/回中不会丢边沿，边沿保留到本逻辑帧被消费）。
    pub fn gamepad_axis_event(&mut self, axis: u8, direction: AxisDirection, is_down: bool) {
        if is_down {
            if !self
                .axis_digital
                .get(&(axis, direction))
                .copied()
                .unwrap_or(false)
            {
                self.pressed_axes.insert((axis, direction));
            }
            self.axis_digital.insert((axis, direction), true);
            self.last_device = InputDevice::Gamepad;
        } else {
            self.axis_digital.insert((axis, direction), false);
            self.released_axes.insert((axis, direction));
        }
    }

    /// 当前轴数字态（无条目时为 `None`，表示按模拟值回落判定）。
    #[must_use]
    pub fn gamepad_axis_digital(&self, axis: u8, direction: AxisDirection) -> Option<bool> {
        self.axis_digital.get(&(axis, direction)).copied()
    }

    /// 最近一次产生真实输入的设备类别（M22/W3）。
    #[must_use]
    pub const fn last_device(&self) -> InputDevice {
        self.last_device
    }

    #[must_use]
    pub fn gamepad_button(&self, button: u8) -> bool {
        self.held_gamepad.contains(&button)
    }

    #[must_use]
    pub fn gamepad_axis(&self, axis: u8) -> f32 {
        self.gamepad_axes.get(&axis).copied().unwrap_or(0.0)
    }

    /// 动作是否处于按下状态（任一绑定命中）。
    #[must_use]
    pub fn action_is_down(&self, map: &ActionMap, action: &str) -> bool {
        map.bindings(action).iter().any(|binding| {
            binding_is_down(
                *binding,
                &self.held_keys,
                &self.held_mouse,
                &self.held_gamepad,
                &self.gamepad_axes,
                &self.axis_digital,
            ) || matches!(binding, Binding::Key(k) if self.virtual_keys.contains(k))
        })
    }

    /// 动作本帧是否刚按下（任一绑定命中）。
    ///
    /// M22/W3 起**手柄轴也能产生按下边沿**（设备层按滞回阈值合成），因此
    /// `input.action_pressed("ui_up")` 对键盘方向键、DPad 与左摇杆同义。
    #[must_use]
    pub fn action_just_pressed(&self, map: &ActionMap, action: &str) -> bool {
        map.bindings(action).iter().any(|binding| match binding {
            Binding::Key(key) => self.pressed_keys.contains(key),
            Binding::Mouse(button) => self.pressed_mouse.contains(button),
            Binding::GamepadButton(index) => self.pressed_gamepad.contains(index),
            Binding::GamepadAxis(index, direction) => {
                self.pressed_axes.contains(&(*index, *direction))
            }
        })
    }

    /// 动作本帧是否刚松开（任一绑定命中）。
    #[must_use]
    pub fn action_just_released(&self, map: &ActionMap, action: &str) -> bool {
        map.bindings(action).iter().any(|binding| match binding {
            Binding::Key(key) => self.released_keys.contains(key),
            Binding::Mouse(button) => self.released_mouse.contains(button),
            Binding::GamepadButton(index) => self.released_gamepad.contains(index),
            Binding::GamepadAxis(index, direction) => {
                self.released_axes.contains(&(*index, *direction))
            }
        })
    }

    /// 动作是否由**模拟量绑定**（手柄轴过阈值）保持按下（M22/W3）。
    ///
    /// `input.action_repeat` 用它区分「摇杆按住 → 连发」与「键盘/DPad 按住 → 只走
    /// 一步」：数字量没有持续量，保持连发会让一次按键在菜单里连续滚动。
    #[must_use]
    pub fn action_held_by_analog(&self, map: &ActionMap, action: &str) -> bool {
        map.bindings(action).iter().any(|binding| match binding {
            Binding::GamepadAxis(index, direction) => {
                match self.axis_digital.get(&(*index, *direction)) {
                    Some(down) => *down,
                    None => self
                        .gamepad_axes
                        .get(index)
                        .is_some_and(|value| *value * direction.sign() >= AXIS_DIGITAL_ON),
                }
            }
            _ => false,
        })
    }

    /// 两个动作合成的轴（-1..=1）；手柄轴绑定取模拟量，键位取数字量。
    #[must_use]
    pub fn action_axis(&self, map: &ActionMap, negative: &str, positive: &str) -> f32 {
        let mut value = 0.0f32;
        for binding in map.bindings(negative) {
            value -= binding_axis_value(
                *binding,
                &self.held_keys,
                &self.held_mouse,
                &self.held_gamepad,
                &self.gamepad_axes,
                &self.axis_digital,
            );
            if let Binding::Key(k) = binding
                && self.virtual_keys.contains(k)
            {
                value -= 1.0;
            }
        }
        for binding in map.bindings(positive) {
            value += binding_axis_value(
                *binding,
                &self.held_keys,
                &self.held_mouse,
                &self.held_gamepad,
                &self.gamepad_axes,
                &self.axis_digital,
            );
            if let Binding::Key(k) = binding
                && self.virtual_keys.contains(k)
            {
                value += 1.0;
            }
        }
        value.clamp(-1.0, 1.0)
    }

    /// Records a scroll-wheel event (accumulated until the next frame).
    pub fn scroll_event(&mut self, dx: f32, dy: f32) {
        self.scroll_delta.0 += dx;
        self.scroll_delta.1 += dy;
    }

    /// Clears edge transitions and cursor/scroll deltas after the frame.
    pub fn finish_frame(&mut self) {
        self.pressed_keys.clear();
        self.released_keys.clear();
        self.pressed_mouse.clear();
        self.released_mouse.clear();
        self.pressed_gamepad.clear();
        self.released_gamepad.clear();
        self.pressed_axes.clear();
        self.released_axes.clear();
        self.cursor_delta = (0.0, 0.0);
        self.scroll_delta = (0.0, 0.0);
    }

    /// Clears held and transient state, used when pausing or losing focus.
    pub fn clear_all(&mut self) {
        self.held_keys.clear();
        self.held_mouse.clear();
        self.held_gamepad.clear();
        self.gamepad_axes.clear();
        self.axis_digital.clear();
        self.clear_transitions();
    }

    /// Clears transient transitions, used when entering a paused state.
    pub fn clear_transitions(&mut self) {
        self.pressed_keys.clear();
        self.released_keys.clear();
        self.pressed_mouse.clear();
        self.released_mouse.clear();
        self.pressed_gamepad.clear();
        self.released_gamepad.clear();
        self.pressed_axes.clear();
        self.released_axes.clear();
        self.cursor_delta = (0.0, 0.0);
        self.scroll_delta = (0.0, 0.0);
    }

    #[must_use]
    pub fn is_down(&self, key: KeyCode) -> bool {
        self.held_keys.contains(&key)
    }

    #[must_use]
    pub fn just_pressed(&self, key: KeyCode) -> bool {
        self.pressed_keys.contains(&key)
    }

    #[must_use]
    pub fn just_released(&self, key: KeyCode) -> bool {
        self.released_keys.contains(&key)
    }

    #[must_use]
    pub fn mouse_is_down(&self, button: MouseButton) -> bool {
        self.held_mouse.contains(&button)
    }

    #[must_use]
    pub fn mouse_just_pressed(&self, button: MouseButton) -> bool {
        self.pressed_mouse.contains(&button)
    }

    #[must_use]
    pub fn mouse_just_released(&self, button: MouseButton) -> bool {
        self.released_mouse.contains(&button)
    }

    #[must_use]
    pub fn axis(&self, axis: &str) -> f32 {
        match axis {
            "horizontal" => {
                self.is_down(KeyCode::KeyD) as i32 as f32
                    - self.is_down(KeyCode::KeyA) as i32 as f32
            }
            "vertical" => {
                self.is_down(KeyCode::KeyW) as i32 as f32
                    - self.is_down(KeyCode::KeyS) as i32 as f32
            }
            _ => 0.0,
        }
    }

    #[must_use]
    pub const fn cursor_position(&self) -> (f64, f64) {
        self.cursor_position
    }

    #[must_use]
    pub const fn cursor_delta(&self) -> (f64, f64) {
        self.cursor_delta
    }

    #[must_use]
    pub const fn scroll_delta(&self) -> (f32, f32) {
        self.scroll_delta
    }

    #[must_use]
    pub const fn is_focused(&self) -> bool {
        self.focused
    }
}

// ---------------------------------------------------------------------------
// 手柄死区与轴判定（M22：自 `host::gamepad` 下沉）
//
// 这些是**纯值类型与纯函数**，不依赖任何平台 API；原先放在 `host/gamepad.rs`
// 导致 `core::settings` 反向依赖 `host`（docs/37 P1-1）。下沉后 core 可以独立编译
// 到 wasm/headless-only 目标，`host::gamepad` 只做重导出以保持既有路径可用。
// ---------------------------------------------------------------------------

/// 左/右摇杆默认死区。
pub const STICK_DEADZONE: f32 = 0.15;
/// 扳机默认死区。
pub const TRIGGER_DEADZONE: f32 = 0.05;
/// 死区可配置上限：必须严格小于 1（等于 1 会把满量程也归零）。
pub const MAX_DEADZONE: f32 = 0.9;

/// 轴判定的按下阈值（模拟量绝对值达到它才算「按下」）。
///
/// 与 [`AXIS_DIGITAL_OFF`] 一起构成**滞回**：进入用 ON、退出用 OFF，避免摇杆在
/// 阈值附近抖动时反复产生按下/松开边沿（M22 起轴也有数字边沿，见
/// [`InputSnapshot::gamepad_axis_event`]）。
pub const AXIS_DIGITAL_ON: f32 = 0.5;
/// 轴判定的松开阈值（严格小于 [`AXIS_DIGITAL_ON`]）。
pub const AXIS_DIGITAL_OFF: f32 = 0.4;
/// 滞回宽度：可配置进入阈值时，退出阈值 = 进入阈值 − 本常量（下限 0）。
pub const AXIS_HYSTERESIS: f32 = AXIS_DIGITAL_ON - AXIS_DIGITAL_OFF;

/// 摇杆/扳机死区（M20-08：可在 `settings.toml` 的 `[input]` 段读写；默认值仍是
/// [`STICK_DEADZONE`] / [`TRIGGER_DEADZONE`]）。
///
/// 语义与 [`apply_stick_deadzone`] / [`apply_stick_deadzone_radial`] /
/// [`apply_trigger_deadzone`] 一致：死区内归零（消除回中漂移），死区外按比例
/// 重映射到 0..1。非有限值与超界值在构造时就被钳制，因此从设置文件读进来的脏
/// 数据不会产生非法曲线。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GamepadDeadzone {
    /// 左/右摇杆死区（0..=MAX_DEADZONE）。
    pub stick: f32,
    /// 扳机死区（0..=MAX_DEADZONE）。
    pub trigger: f32,
}

impl GamepadDeadzone {
    /// 引擎默认死区（0.15 / 0.05）。
    pub const DEFAULT: Self = Self {
        stick: STICK_DEADZONE,
        trigger: TRIGGER_DEADZONE,
    };

    /// 构造并钳制到 `0..=MAX_DEADZONE`（非有限值回落默认值）。
    #[must_use]
    pub fn new(stick: f32, trigger: f32) -> Self {
        Self {
            stick: sanitize_deadzone(stick, STICK_DEADZONE),
            trigger: sanitize_deadzone(trigger, TRIGGER_DEADZONE),
        }
    }

    /// 与默认值比较（编辑器「死区是否被改过」判断用）。
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::DEFAULT
    }
}

impl Default for GamepadDeadzone {
    fn default() -> Self {
        Self::DEFAULT
    }
}

fn sanitize_deadzone(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, MAX_DEADZONE)
    } else {
        fallback
    }
}

/// 单轴死区：死区内归零，死区外按比例映射到 0..1（保持符号）。
#[must_use]
pub fn apply_stick_deadzone(value: f32, deadzone: f32) -> f32 {
    apply_axis_deadzone(value, deadzone)
}

/// 扳机死区（输入 0..1）。
#[must_use]
pub fn apply_trigger_deadzone(value: f32, deadzone: f32) -> f32 {
    apply_axis_deadzone(value, deadzone)
}

fn apply_axis_deadzone(value: f32, deadzone: f32) -> f32 {
    let clamped = value.clamp(-1.0, 1.0);
    if clamped.abs() <= deadzone {
        return 0.0;
    }
    let span = (1.0 - deadzone).max(f32::EPSILON);
    let magnitude = (clamped.abs() - deadzone) / span;
    magnitude.clamp(0.0, 1.0) * clamped.signum()
}

/// **径向**摇杆死区（M22）：按 (x, y) 的模长判定，而不是逐轴判定。
///
/// 逐轴死区会让斜向比正向灵敏（x、y 各自刚过死区就同时输出），径向死区按杆的
/// 实际偏移量处理：模长 ≤ `deadzone` 归零，否则以 `(mag - dz) / (1 - dz)` 缩放并
/// **保持方向**。返回值已钳制到 `-1..=1`。
#[must_use]
pub fn apply_stick_deadzone_radial(x: f32, y: f32, deadzone: f32) -> (f32, f32) {
    let x = x.clamp(-1.0, 1.0);
    let y = y.clamp(-1.0, 1.0);
    let magnitude = (x * x + y * y).sqrt();
    if magnitude <= deadzone {
        return (0.0, 0.0);
    }
    let span = (1.0 - deadzone).max(f32::EPSILON);
    let scale = ((magnitude - deadzone) / span).clamp(0.0, 1.0) / magnitude;
    ((x * scale).clamp(-1.0, 1.0), (y * scale).clamp(-1.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::{
        ACTION_MAP_VERSION, AXIS_DIGITAL_OFF, AXIS_DIGITAL_ON, ActionMap, ActionMapError,
        AxisDirection, Binding, GamepadDeadzone, InputDevice, InputSnapshot, KeyCode, MouseButton,
        apply_stick_deadzone_radial,
    };

    /// M22/W1：字符 → 物理键是**US-QWERTY 位置**换算，大小写不敏感。
    #[test]
    fn from_char_gives_each_letter_a_distinct_key() {
        let key =
            |character: char| KeyCode::from_char(character).expect("QWERTY 字符必须有对应物理键");
        assert_eq!(key('w'), KeyCode::KeyW);
        assert_eq!(key('a'), KeyCode::KeyA);
        assert_eq!(key('s'), KeyCode::KeyS);
        assert_eq!(key('d'), KeyCode::KeyD);
        // 其余字母键各自对应**独立的物理键**，Q/E/R/T/F/G 互不相同。
        assert_eq!(key('q'), KeyCode::KeyQ);
        assert_eq!(key('e'), KeyCode::KeyE);
        assert_eq!(key('t'), KeyCode::KeyT);
        assert_ne!(key('q'), key('e'));
        // 大小写同一物理键。
        assert_eq!(key('Q'), key('q'));
        // 数字键走主键区数字（`Digit1`），不再是 ASCII 码位。
        assert_eq!(key('1'), KeyCode::Digit1);
        // 无法对应物理键的字符返回 `None`（调用方必须报错，不得回落「未知键」）。
        assert_eq!(KeyCode::from_char('中'), None);
        assert_eq!(KeyCode::from_char('\u{1}'), None);
    }

    /// M22/W1：`to_text` / `from_text` 对**全表 194 项**逐项往返。
    ///
    /// 这是「引擎键域 = 表驱动」的直接证据：任何一项的文本形式写错、重复或漏掉，
    /// 这里立刻失败。
    #[test]
    fn every_key_code_round_trips_through_text() {
        assert_eq!(KeyCode::ALL.len(), 194, "物理键域与 winit 0.30 对齐");
        let mut texts: Vec<&str> = KeyCode::ALL.iter().map(|key| key.to_text()).collect();
        texts.sort_unstable();
        let unique = {
            let mut deduped = texts.clone();
            deduped.dedup();
            deduped.len()
        };
        assert_eq!(
            unique,
            texts.len(),
            "文本形式必须两两不同（否则设置文件有歧义）"
        );
        for key in KeyCode::ALL {
            let text = key.to_text();
            assert_eq!(
                KeyCode::from_text(text),
                Some(*key),
                "{text} 必须解析回自己"
            );
            // 解析大小写不敏感（玩家手写 `KeyE` 也要认）。
            assert_eq!(KeyCode::from_text(&text.to_uppercase()), Some(*key));
            // 带 `key:` 前缀的完整绑定文本也要往返。
            let binding = Binding::Key(*key);
            assert_eq!(Binding::parse(&binding.to_text()), Ok(binding));
        }
        assert_eq!(KeyCode::from_text("not-a-key"), None);
        assert_eq!(KeyCode::from_text(""), None);
    }

    /// M22/W1：v1 设置文本里的键位名要**逐项**映射到物理键，带 Shift 的字符记诊断。
    #[test]
    fn v1_key_texts_migrate_to_physical_keys() {
        // v1 可读名 → 物理键。
        for (v1, expected) in [
            ("w", KeyCode::KeyW),
            ("E", KeyCode::KeyE),
            ("1", KeyCode::Digit1),
            ("up", KeyCode::ArrowUp),
            ("down", KeyCode::ArrowDown),
            ("left", KeyCode::ArrowLeft),
            ("right", KeyCode::ArrowRight),
            ("space", KeyCode::Space),
            ("esc", KeyCode::Escape),
            ("escape", KeyCode::Escape),
            ("-", KeyCode::Minus),
            ("=", KeyCode::Equal),
            (";", KeyCode::Semicolon),
            ("'", KeyCode::Quote),
            ("\\", KeyCode::Backslash),
            // v1 的码位写法（方向键，同 HEAD 里的 `ARROW_UP/DOWN/LEFT/RIGHT`）。
            ("other:1048577", KeyCode::ArrowUp),
            ("other:1048578", KeyCode::ArrowDown),
            ("other:1048579", KeyCode::ArrowLeft),
            ("other:1048580", KeyCode::ArrowRight),
            // v1 的 F1/F2/F4 码位。
            ("other:1", KeyCode::F1),
            ("other:2", KeyCode::F2),
            ("other:4", KeyCode::F4),
            // 带 Shift 的字符 → 未按 Shift 的物理键。
            ("+", KeyCode::Equal),
            (":", KeyCode::Semicolon),
            ("?", KeyCode::Slash),
            ("!", KeyCode::Digit1),
            ("{", KeyCode::BracketLeft),
            ("}", KeyCode::BracketRight),
            ("|", KeyCode::Backslash),
            ("~", KeyCode::Backquote),
            ("<", KeyCode::Comma),
            (">", KeyCode::Period),
            ("\"", KeyCode::Quote),
            ("_", KeyCode::Minus),
            ("(", KeyCode::Digit9),
            (")", KeyCode::Digit0),
        ] {
            let mut notes = Vec::new();
            assert_eq!(
                KeyCode::v1_key_code(v1, &mut notes),
                Some(expected),
                "v1 文本 {v1} 必须映射到 {expected:?}"
            );
        }
        // 未知名 → None（不是某种「未知键」）。
        let mut notes = Vec::new();
        assert_eq!(KeyCode::v1_key_code("not-a-key", &mut notes), None);
        assert!(notes.is_empty(), "失败路径不该写诊断：{notes:?}");
    }

    /// M22/W1：有损换算必须有诊断（`+` 只记 `=` 那个物理键），无有损换算时只提醒升级。
    #[test]
    fn v1_migration_reports_lossy_shift_conversions() {
        let (map, notes) =
            ActionMap::from_text_with_notes("rustlua-actions-v1\nvolume_up=key:+\n").unwrap();
        assert_eq!(map.bindings("volume_up"), [Binding::Key(KeyCode::Equal)]);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("key:+"), "{notes:?}");
        assert!(notes[0].contains("key:equal"), "{notes:?}");
        assert!(notes[0].contains("Shift"), "诊断要给出补救方式：{notes:?}");

        // 无有损换算：只留一条「已升级到 v2」的提示。
        let (map, notes) =
            ActionMap::from_text_with_notes("rustlua-actions-v1\nmove_up=key:w\n").unwrap();
        assert_eq!(map.bindings("move_up"), [Binding::Key(KeyCode::KeyW)]);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].contains("v1") && notes[0].contains("v2"),
            "{notes:?}"
        );

        // v2 输入不产生任何诊断（幂等重读不刷屏）。
        let (_, notes) = ActionMap::from_text_with_notes(&map.to_text()).unwrap();
        assert!(notes.is_empty(), "{notes:?}");

        // 未知版本头仍然明确报错，不会被当成 v1/v2。
        assert_eq!(
            ActionMap::from_text_with_notes("rustlua-actions-v9\nmove_up=key:w"),
            Err(ActionMapError::UnsupportedVersion(
                "rustlua-actions-v9".to_owned()
            ))
        );
    }

    /// M22/W1：**AZERTY/Dvorak 场景**——同一物理位置在不同布局下是同一引擎键。
    ///
    /// 引擎层不需要模拟布局：`KeyCode::KeyW` 就是 `KeyW`，`from_char('z')` 只是
    /// **兼容入口**的 US-QWERTY 假设（AZERTY 玩家按 W 位置，平台报的字符是 `z`，
    /// 旧实现读字符就会得到「按下 Z」）。这里锁住两件事：
    /// ① 物理键身份不经过字符；② 兼容路径的假设是文档化的 US-QWERTY。
    #[test]
    fn physical_binding_identity_ignores_layout() {
        // ① 物理键路径：绑定 `KeyW` 只认 `KeyW`，`KeyZ` 不触发。
        let map = ActionMap::with_defaults();
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::KeyW, true);
        assert!(input.action_is_down(&map, "move_up"));
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::KeyZ, true);
        assert!(
            !input.action_is_down(&map, "move_up"),
            "AZERTY 的 Z 字符位置不得被当成 W 物理键"
        );
        // ② 兼容路径是 US-QWERTY 假设：字符 `z` → 物理键 `KeyZ`（不是 `KeyW`）。
        assert_eq!(KeyCode::from_char('z'), Some(KeyCode::KeyZ));
        assert_eq!(KeyCode::from_char('w'), Some(KeyCode::KeyW));
        // ③ v1 文本同理：`key:z` 迁到 `KeyZ`，不会因为布局而变。
        let (migrated, _) =
            ActionMap::from_text_with_notes("rustlua-actions-v1\nmove_up=key:z\n").unwrap();
        assert_eq!(migrated.bindings("move_up"), [Binding::Key(KeyCode::KeyZ)]);
    }

    /// M22/W1：默认绑定表里的每个字符键都落在**主键区字母/数字**物理键上，
    /// 且**键盘侧零跨动作冲突**。
    ///
    /// 手柄侧的刻意别名不算冲突（`pad:0` 同时服务 `confirm`/`interact`、左摇杆同时
    /// 服务 `move_*` 与 `ui_*`，见 [`ActionMap::with_defaults`]），所以这里过滤掉
    /// 非键盘绑定，只钉住「一个物理键只属于一个动作」这条键盘不变量。
    #[test]
    fn default_bindings_use_primary_physical_keys() {
        let map = ActionMap::with_defaults();
        for (action, expected) in [
            ("move_up", KeyCode::KeyW),
            ("move_down", KeyCode::KeyS),
            ("move_left", KeyCode::KeyA),
            ("move_right", KeyCode::KeyD),
            ("interact", KeyCode::KeyE),
            ("place", KeyCode::KeyF),
            ("slot_1", KeyCode::Digit1),
            ("slot_9", KeyCode::Digit9),
            ("volume_up", KeyCode::Equal),
            ("volume_down", KeyCode::Minus),
            ("confirm", KeyCode::Space),
            ("cancel", KeyCode::Escape),
            ("ui_up", KeyCode::ArrowUp),
            ("ui_down", KeyCode::ArrowDown),
            ("ui_left", KeyCode::ArrowLeft),
            ("ui_right", KeyCode::ArrowRight),
        ] {
            assert!(
                map.bindings(action).contains(&Binding::Key(expected)),
                "{action} 应绑定 {expected:?}，实际 {:?}",
                map.bindings(action)
            );
        }
        let key_conflicts: Vec<_> = map
            .conflicts()
            .into_iter()
            .filter(|conflict| matches!(conflict.binding, Binding::Key(_)))
            .collect();
        assert_eq!(
            key_conflicts,
            Vec::new(),
            "默认表的键盘绑定必须一个物理键只属于一个动作"
        );
        // 默认表里不再出现任何「字符码位」痕迹：所有键盘绑定都是 `key:<物理键名>`。
        for line in map.to_text().lines().skip(1) {
            for binding in line.split_once('=').expect("k=v").1.split(',') {
                if let Some(rest) = binding.strip_prefix("key:") {
                    assert!(
                        KeyCode::from_text(rest).is_some(),
                        "默认表写出了无法解析的键位文本：{binding}"
                    );
                    assert!(!rest.contains("other:"), "v2 文本不得含 v1 码位：{binding}");
                }
            }
        }
    }

    #[test]
    fn preserves_press_and_release_until_frame_boundary() {
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::Space, true);
        input.key_event(KeyCode::Space, false);
        assert!(input.just_pressed(KeyCode::Space));
        assert!(input.just_released(KeyCode::Space));
        assert!(!input.is_down(KeyCode::Space));
        input.finish_frame();
        assert!(!input.just_pressed(KeyCode::Space));
        assert!(!input.just_released(KeyCode::Space));
    }

    #[test]
    fn computes_wasd_axes_from_held_state() {
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::KeyD, true);
        input.key_event(KeyCode::KeyS, true);
        assert_eq!(input.axis("horizontal"), 1.0);
        assert_eq!(input.axis("vertical"), -1.0);
        input.key_event(KeyCode::KeyA, true);
        assert_eq!(input.axis("horizontal"), 0.0);
    }

    #[test]
    fn focus_loss_clears_held_state_and_pause_clears_edges() {
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::KeyW, true);
        input.mouse_event(MouseButton::Left, true);
        input.set_focused(false);
        assert!(!input.is_down(KeyCode::KeyW));
        assert!(!input.mouse_is_down(MouseButton::Left));
        input.key_event(KeyCode::Space, true);
        input.clear_transitions();
        assert!(!input.just_pressed(KeyCode::Space));
    }

    #[test]
    fn accumulates_and_resets_scroll_delta() {
        let mut input = InputSnapshot::default();
        input.scroll_event(0.0, 1.0);
        input.scroll_event(0.0, 2.0);
        assert_eq!(input.scroll_delta(), (0.0, 3.0));
        input.finish_frame();
        assert_eq!(input.scroll_delta(), (0.0, 0.0));
    }

    #[test]
    fn accumulates_cursor_delta() {
        let mut input = InputSnapshot::default();
        input.cursor_moved(10.0, 20.0);
        input.cursor_moved(13.0, 18.0);
        assert_eq!(input.cursor_position(), (13.0, 18.0));
        assert_eq!(input.cursor_delta(), (13.0, 18.0));
        input.finish_frame();
        assert_eq!(input.cursor_delta(), (0.0, 0.0));
    }

    #[test]
    fn default_action_map_covers_game_keys_and_gamepad() {
        let map = ActionMap::with_defaults();
        assert_eq!(map.bindings("interact"), [Binding::Key(KeyCode::KeyE)]);
        assert_eq!(map.bindings("move_up"), [Binding::Key(KeyCode::KeyW)]);
        // confirm 同时绑定空格与手柄 A 键（基础手柄抽象）。
        assert!(
            map.bindings("confirm")
                .contains(&Binding::Key(KeyCode::Space))
        );
        assert!(map.bindings("confirm").contains(&Binding::GamepadButton(0)));
        // 菜单导航按方向拆成四个动作：方向键 + 左摇杆方向 + DPad（可合成带符号轴）。
        assert_eq!(map.bindings("ui_up").len(), 3);
        assert!(
            map.bindings("ui_up")
                .contains(&Binding::Key(KeyCode::ArrowUp))
        );
        assert!(
            map.bindings("ui_up")
                .contains(&Binding::GamepadAxis(1, AxisDirection::Positive))
        );
        assert!(
            map.bindings("ui_up").contains(&Binding::GamepadButton(12)),
            "DPad 上 = 按钮 12（M20-06 契约）"
        );
        assert!(
            map.bindings("ui_down")
                .contains(&Binding::Key(KeyCode::ArrowDown))
        );
        assert!(
            map.bindings("ui_down")
                .contains(&Binding::GamepadButton(13))
        );
        assert!(
            map.bindings("ui_left")
                .contains(&Binding::GamepadButton(14))
        );
        assert!(
            map.bindings("ui_right")
                .contains(&Binding::GamepadButton(15))
        );
        assert!(map.actions().contains(&"pause"));
        // M20-08 扩表：建造/采集/任务/重开/选种子/音量/数字键全部成为命名动作，
        // 且默认表**自身零冲突**（否则编辑器面板一打开就报冲突）。
        for action in [
            "build",
            "place",
            "demolish",
            "use",
            "quest",
            "respawn",
            "seed",
            "volume_up",
            "volume_down",
            "slot_1",
            "slot_9",
        ] {
            assert!(
                !map.bindings(action).is_empty(),
                "M20-08 默认动作缺失：{action}"
            );
        }
        assert_eq!(
            map.bindings("build"),
            [Binding::Key(
                KeyCode::from_char('b').expect("QWERTY 字符必须有对应物理键")
            )]
        );
        assert_eq!(
            map.bindings("volume_up"),
            [Binding::Key(
                KeyCode::from_char('+').expect("QWERTY 字符必须有对应物理键")
            )]
        );
        assert_eq!(
            map.bindings("slot_3"),
            [Binding::Key(
                KeyCode::from_char('3').expect("QWERTY 字符必须有对应物理键")
            )]
        );
        assert_eq!(map.actions().len(), 36, "默认动作总数");
        assert_eq!(map.conflicts(), Vec::new(), "默认绑定不得跨动作重复");
        assert_eq!(map.validate_bindings(), Ok(()));
    }

    #[test]
    fn cross_action_duplicate_bindings_are_reported_and_block_validation() {
        let mut map = ActionMap::with_defaults();
        // 同动作内重复 → `bind` 直接报 DuplicateBinding（既有语义）。
        assert_eq!(
            map.bind("interact", Binding::Key(KeyCode::KeyE)),
            Err(ActionMapError::DuplicateBinding {
                action: "interact".into(),
                binding: "key:keye".into(),
            })
        );
        // 跨动作重复 → `bind` 允许（一次 rebind 覆盖后可能撞车），但 conflicts/
        // validate_bindings 必须报出来，编辑器据此阻止保存。
        map.bind("attack", Binding::Key(KeyCode::KeyE))
            .expect("跨动作重复本身不阻止写入");
        let conflicts = map.conflicts();
        assert_eq!(conflicts.len(), 1, "{conflicts:?}");
        assert_eq!(conflicts[0].binding, Binding::Key(KeyCode::KeyE));
        assert_eq!(conflicts[0].actions, vec!["attack", "interact"]);
        assert!(
            conflicts[0].describe().contains("key:keye"),
            "{conflicts:?}"
        );
        assert_eq!(
            map.validate_bindings(),
            Err(ActionMapError::DuplicateBinding {
                action: "attack".into(),
                binding: "key:keye".into(),
            })
        );
        // 解绑冲突项后校验恢复通过；输出顺序按绑定文本稳定排序。
        map.bind("skill", Binding::Key(KeyCode::KeyE))
            .expect("第二处重复");
        assert!(map.unbind("attack", Binding::Key(KeyCode::KeyE)));
        let conflicts = map.conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].actions, vec!["interact", "skill"]);
        assert!(map.unbind("skill", Binding::Key(KeyCode::KeyE)));
        assert!(map.conflicts().is_empty());
        assert_eq!(map.validate_bindings(), Ok(()));
    }

    #[test]
    fn action_queries_follow_bindings_and_survive_rebinding() {
        let mut map = ActionMap::with_defaults();
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::KeyE, true);
        assert!(input.action_is_down(&map, "interact"));
        assert!(input.action_just_pressed(&map, "interact"));
        assert!(!input.action_is_down(&map, "attack"));

        // 重绑定：把 interact 从 E 改到 F，旧键失效、新键生效。
        assert!(map.unbind("interact", Binding::Key(KeyCode::KeyE)));
        map.bind("interact", Binding::Key(KeyCode::KeyF)).unwrap();
        assert!(!input.action_is_down(&map, "interact"));
        input.key_event(KeyCode::KeyF, true);
        assert!(input.action_is_down(&map, "interact"));
        assert!(input.action_just_pressed(&map, "interact"));

        // 重复绑定同一输入 → 明确错误。
        assert_eq!(
            map.bind("interact", Binding::Key(KeyCode::KeyF)),
            Err(ActionMapError::DuplicateBinding {
                action: "interact".into(),
                binding: "key:keyf".into(),
            })
        );
        assert_eq!(
            map.bind("  ", Binding::Key(KeyCode::KeyW)),
            Err(ActionMapError::EmptyAction)
        );
    }

    #[test]
    fn action_map_round_trips_through_settings_text() {
        let mut map = ActionMap::with_defaults();
        map.unbind("interact", Binding::Key(KeyCode::KeyE));
        map.bind("interact", Binding::Key(KeyCode::KeyF)).unwrap();
        map.bind(
            "menu_confirm",
            Binding::GamepadAxis(1, AxisDirection::Negative),
        )
        .unwrap();
        let text = map.to_text();
        assert!(text.starts_with(ACTION_MAP_VERSION));
        let parsed = ActionMap::from_text(&text).unwrap();
        assert_eq!(parsed, map);

        // 版本头不符 / 绑定非法 → 明确错误，不静默放过。
        assert_eq!(
            ActionMap::from_text("rustlua-actions-v0\ninteract=key:e\n"),
            Err(ActionMapError::UnsupportedVersion(
                "rustlua-actions-v0".into()
            ))
        );
        assert!(matches!(
            ActionMap::from_text("rustlua-actions-v1\ninteract=joystick:1\n"),
            Err(ActionMapError::InvalidBinding(_))
        ));

        // 部分映射 + 默认兜底：用户重绑定不被覆盖。
        let mut partial = ActionMap::new();
        partial
            .bind("interact", Binding::Key(KeyCode::KeyF))
            .unwrap();
        partial.merge_missing(&ActionMap::with_defaults());
        assert_eq!(partial.bindings("interact"), [Binding::Key(KeyCode::KeyF)]);
        assert_eq!(partial.bindings("move_up"), [Binding::Key(KeyCode::KeyW)]);
    }

    /// M22/W1：方向键的绑定文本是**物理键名**（`key:arrowup`），v1 的
    /// `key:up` / `key:other:1048577` 仍可读入并归一化为同一物理键。
    #[test]
    fn arrow_keys_use_physical_binding_text_and_round_trip() {
        let map = ActionMap::with_defaults();
        assert_eq!(map.bindings("ui_up")[0].to_text(), "key:arrowup");
        assert_eq!(map.bindings("ui_down")[0].to_text(), "key:arrowdown");
        assert_eq!(map.bindings("ui_left")[0].to_text(), "key:arrowleft");
        assert_eq!(map.bindings("ui_right")[0].to_text(), "key:arrowright");

        // 文本往返后仍是同一映射（M20-08 起 ui_* 还带 DPad 按钮 12..15）。
        let text = map.to_text();
        assert!(text.starts_with(ACTION_MAP_VERSION), "{text}");
        assert!(
            text.contains("ui_up=key:arrowup,padaxis:1+,pad:12"),
            "{text}"
        );
        assert_eq!(ActionMap::from_text(&text).unwrap(), map);

        // 旧设置文件（v1）里的两种方向键写法都能解析，并归一化为物理键。
        let legacy = ActionMap::from_text("rustlua-actions-v1\nui_up=key:up\n").unwrap();
        assert_eq!(legacy.bindings("ui_up"), [Binding::Key(KeyCode::ArrowUp)]);
        let legacy_code =
            ActionMap::from_text("rustlua-actions-v1\nui_up=key:other:1048577\n").unwrap();
        assert_eq!(
            legacy_code.bindings("ui_up"),
            [Binding::Key(KeyCode::ArrowUp)]
        );
        // 读 v1、写 v2：迁移后的文本头是新版本。
        assert_eq!(legacy.to_text(), "rustlua-actions-v2\nui_up=key:arrowup");
    }

    #[test]
    fn gamepad_bindings_drive_actions_and_axes() {
        let map = ActionMap::with_defaults();
        let mut input = InputSnapshot::default();
        input.gamepad_button_event(0, true);
        assert!(input.action_is_down(&map, "confirm"));
        assert!(input.action_just_pressed(&map, "confirm"));
        input.finish_frame();
        input.gamepad_button_event(0, false);
        assert!(input.action_just_released(&map, "confirm"));
        assert!(!input.action_is_down(&map, "confirm"));

        // 轴：0.5 判定阈值，符号由绑定方向决定（默认映射按方向拆分动作）。
        input.set_gamepad_axis(1, 0.8);
        assert!(input.action_is_down(&map, "ui_up"));
        assert!(!input.action_is_down(&map, "ui_down"));
        input.set_gamepad_axis(1, 0.2);
        assert!(!input.action_is_down(&map, "ui_up"));
        input.set_gamepad_axis(1, 2.5);
        assert_eq!(input.gamepad_axis(1), 1.0, "轴值需钳制到 -1..=1");

        // 摇杆合成轴：一对相反方向的轴动作相加（模拟量，而非 0/1）——
        // 这正是默认 `ui_up`/`ui_down` 的用法。
        let mut stick_map = ActionMap::with_defaults();
        stick_map.clear("ui_up");
        stick_map.clear("ui_down");
        stick_map
            .bind("stick_up", Binding::GamepadAxis(1, AxisDirection::Negative))
            .unwrap();
        stick_map
            .bind(
                "stick_down",
                Binding::GamepadAxis(1, AxisDirection::Positive),
            )
            .unwrap();
        input.set_gamepad_axis(1, 0.8);
        assert!((input.action_axis(&stick_map, "stick_up", "stick_down") - 0.8).abs() < 1e-6);
        input.set_gamepad_axis(1, -0.6);
        assert!((input.action_axis(&stick_map, "stick_up", "stick_down") + 0.6).abs() < 1e-6);

        // 默认映射直接可用：不清理绑定也能得到带符号的垂直轴。
        // 约定：`action_axis(negative, positive)` 首参是**负向**动作，
        // 因此垂直方向写作 ("ui_down", "ui_up")（上为 +）。
        input.set_gamepad_axis(1, 0.7);
        assert!((input.action_axis(&map, "ui_down", "ui_up") - 0.7).abs() < 1e-6);
        input.set_gamepad_axis(1, -0.7);
        assert!((input.action_axis(&map, "ui_down", "ui_up") + 0.7).abs() < 1e-6);
        // 水平方向：左负右正，直接按 (left, right) 组合。
        input.set_gamepad_axis(0, 0.6);
        assert!((input.action_axis(&map, "ui_left", "ui_right") - 0.6).abs() < 1e-6);
    }

    #[test]
    fn focus_loss_clears_gamepad_state_and_key_axes_combine() {
        let map = ActionMap::with_defaults();
        let mut input = InputSnapshot::default();
        input.gamepad_button_event(0, true);
        input.set_gamepad_axis(1, 0.9);
        input.set_focused(false);
        assert!(!input.action_is_down(&map, "confirm"));
        assert_eq!(input.gamepad_axis(1), 0.0);
        assert!(!input.is_focused());

        // 键位轴：相反方向同时按下归零，单方向为 ±1。
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::KeyD, true);
        assert_eq!(input.action_axis(&map, "move_left", "move_right"), 1.0);
        input.key_event(KeyCode::KeyA, true);
        assert_eq!(input.action_axis(&map, "move_left", "move_right"), 0.0);
        input.key_event(KeyCode::KeyD, false);
        assert_eq!(input.action_axis(&map, "move_left", "move_right"), -1.0);
    }

    /// M22/W3：轴的**数字态 + 边沿**驱动动作，语义与键盘/按钮一致。
    #[test]
    fn axis_digital_state_and_edges_drive_actions() {
        let map = ActionMap::with_defaults();
        let mut input = InputSnapshot::default();

        // 设备层：轴值 + 越过阈值的数字态与按下边沿。
        input.set_gamepad_axis(1, 0.8);
        input.gamepad_axis_event(1, AxisDirection::Positive, true);
        assert!(input.action_is_down(&map, "ui_up"));
        assert!(input.action_just_pressed(&map, "ui_up"));
        assert!(!input.action_is_down(&map, "ui_down"));
        assert_eq!(
            input.gamepad_axis_digital(1, AxisDirection::Positive),
            Some(true)
        );

        // 按住期间：边沿只出现一次（快照保留到本逻辑帧被消费）。
        input.finish_frame();
        assert!(input.action_is_down(&map, "ui_up"));
        assert!(!input.action_just_pressed(&map, "ui_up"));

        // 回中：松开边沿，动作不再按下。
        input.set_gamepad_axis(1, 0.1);
        input.gamepad_axis_event(1, AxisDirection::Positive, false);
        assert!(input.action_just_released(&map, "ui_up"));
        assert!(!input.action_is_down(&map, "ui_up"));
        input.finish_frame();
        assert!(!input.action_just_released(&map, "ui_up"));

        // 负方向同理（轴值 -0.9 → ui_down）。
        input.set_gamepad_axis(1, -0.9);
        input.gamepad_axis_event(1, AxisDirection::Negative, true);
        assert!(input.action_is_down(&map, "ui_down"));
        assert!(input.action_just_pressed(&map, "ui_down"));
        assert!(!input.action_is_down(&map, "ui_up"));
    }

    /// 没有设备层数字态时（测试/宿主直接 `set_gamepad_axis`），回落按阈值判定，
    /// 且**不产生边沿**——M20 的既有语义不变。
    #[test]
    fn axis_without_device_state_falls_back_to_analog_threshold() {
        let map = ActionMap::with_defaults();
        let mut input = InputSnapshot::default();
        input.set_gamepad_axis(1, 0.8);
        assert!(input.action_is_down(&map, "ui_up"));
        assert!(!input.action_just_pressed(&map, "ui_up"));
        assert_eq!(input.gamepad_axis_digital(1, AxisDirection::Positive), None);

        input.set_gamepad_axis(1, 0.49);
        assert!(!input.action_is_down(&map, "ui_up"), "0.49 未过 0.5 阈值");
        // 阈值常量与 Lua 侧文档一致（编译期断言：避免被当成运行时条件）。
        const { assert!(AXIS_DIGITAL_ON == 0.5) };
        const { assert!(AXIS_DIGITAL_OFF < AXIS_DIGITAL_ON) };
    }

    /// M22/W3：径向死区按 (x, y) 模长判定，消除「斜向比正向灵敏」。
    #[test]
    fn radial_deadzone_scales_by_magnitude_and_keeps_direction() {
        // 死区内（含斜向小位移）归零。
        assert_eq!(
            apply_stick_deadzone_radial(0.0, 0.0, GamepadDeadzone::DEFAULT.stick),
            (0.0, 0.0)
        );
        assert_eq!(apply_stick_deadzone_radial(0.1, 0.1, 0.15), (0.0, 0.0));
        // 正向：与逐轴结果一致（模长 = 单轴绝对值）。
        let (x, y) = apply_stick_deadzone_radial(0.6, 0.0, 0.15);
        assert!((x - 0.529_411_8).abs() < 1e-4, "x={x}");
        assert_eq!(y, 0.0);
        // 斜向：与同模长的单轴输入得到相同**幅度**（逐轴死区下斜向会更大）。
        let diagonal = (0.6_f32, 0.6_f32);
        let magnitude = (diagonal.0 * diagonal.0 + diagonal.1 * diagonal.1).sqrt();
        let (dx, dy) = apply_stick_deadzone_radial(diagonal.0, diagonal.1, 0.15);
        let (axis_x, _) = apply_stick_deadzone_radial(magnitude, 0.0, 0.15);
        let diagonal_magnitude = (dx * dx + dy * dy).sqrt();
        assert!(
            (diagonal_magnitude - axis_x).abs() < 1e-4,
            "斜向模长 {diagonal_magnitude} 应与同模长单轴 {axis_x} 一致"
        );
        assert!((dx - dy).abs() < 1e-6, "方向保持（45° 输入仍 45°）");
        // 极值不越界：满量程斜向仍落在单位圆内。
        let (mx, my) = apply_stick_deadzone_radial(1.0, 1.0, 0.15);
        assert!(mx * mx + my * my <= 1.0 + 1e-6, "({mx}, {my}) 不得越界");
    }

    /// M22/W3：`last_device` 只在**真实边沿**上切换，空闲帧（轴每帧上报 0.0）不改写。
    #[test]
    fn last_device_follows_real_input_edges_only() {
        let mut input = InputSnapshot::default();
        assert_eq!(input.last_device(), InputDevice::Keyboard, "默认键鼠");

        input.gamepad_button_event(0, true);
        assert_eq!(input.last_device(), InputDevice::Gamepad);

        input.key_event(KeyCode::KeyW, true);
        assert_eq!(input.last_device(), InputDevice::Keyboard);
        assert_eq!(input.last_device().to_text(), "keyboard");

        // 轴事件（真实推杆）切到手柄；单纯写模拟量不算输入。
        input.gamepad_axis_event(1, AxisDirection::Positive, true);
        assert_eq!(input.last_device(), InputDevice::Gamepad);
        input.set_gamepad_axis(1, 0.0);
        input.gamepad_axis_event(1, AxisDirection::Positive, false);
        assert_eq!(input.last_device(), InputDevice::Gamepad, "松开不切回键鼠");
        assert_eq!(input.last_device().to_text(), "gamepad");

        input.mouse_event(MouseButton::Left, true);
        assert_eq!(input.last_device(), InputDevice::Keyboard, "鼠标算键鼠一侧");
    }

    /// M22/W3：失焦清空轴数字态与边沿（不粘杆）。
    #[test]
    fn focus_loss_clears_axis_digital_state() {
        let map = ActionMap::with_defaults();
        let mut input = InputSnapshot::default();
        input.set_gamepad_axis(1, 0.9);
        input.gamepad_axis_event(1, AxisDirection::Positive, true);
        assert!(input.action_is_down(&map, "ui_up"));

        input.set_focused(false);
        assert_eq!(input.gamepad_axis_digital(1, AxisDirection::Positive), None);
        assert!(!input.action_is_down(&map, "ui_up"), "失焦后不粘杆");
        assert!(!input.action_just_pressed(&map, "ui_up"));
    }
}
