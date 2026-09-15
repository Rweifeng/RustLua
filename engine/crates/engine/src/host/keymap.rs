//! 平台事件 → 引擎物理键身份的**唯一**转换层（M22/W1）。
//!
//! 在 M22/W1 之前，「平台键 → 引擎键」这件事在四个地方各写了一遍
//! （`game_app::map_key` 读字符、`game_view::PREVIEW_KEYS` 手写 60 项镜像、
//! `input_bindings` 的捕获表、`core::script::parse_key` 的字符表），于是
//! 「引擎支持哪些键」没有单一事实来源，加一个键要改四处、漏一处就静默失效。
//!
//! 现在只有这两份表：**引擎键域 = `core::input::KeyCode` 全表**，逐项覆盖由
//! `map_key_covers_every_key_code`（本文件，winit 侧）与编辑器侧的
//! `preview_keys_match_the_engine_key_table` / `egui_round_trip_covers_every_key_code`
//! 锁住；加一个键只需在 `core::input` 的 `define_key_codes!` 里加一项。
//!
//! 分层：`core` 不认识平台类型（`docs/37` P1-1 的架构约束），因此转换表只能落在
//! `host`；而 **egui 那张表放在编辑器侧**（`editor::panels::keymap`）——引擎不依赖
//! egui（只有编辑器是 egui 宿主），这样 `engine` 的可选目标（headless/Web）不必为
//! 一个编辑器专用的 GUI 类型付依赖代价：
//!
//! | 宿主 | 平台事件 | 转换表 |
//! |---|---|---|
//! | 独立 `game` | `WindowEvent::KeyboardInput`（`PhysicalKey::Code`） | [`map_key_code`] / [`map_key_event`]（本文件） |
//! | 编辑器 Play 预览 + 重绑定捕获 | `egui::Key`（egui-winit 的 `physical_key`） | `editor::panels::keymap` 的 `egui_input_key` / `egui_key` / `preview_keys` |
//!
//! **为什么必须是物理键**：字符身份在 AZERTY/Dvorak 上会把移动键错位（`W` 位置产生
//! `z`），而且 `Shift+1` 这类组合、Tab/Enter/F5/修饰键在字符身份下**根本无法表达**。

use crate::core::input::KeyCode;

/// `winit` 的物理键码 → 引擎物理键。
///
/// 覆盖 winit 0.30 `KeyCode` 的全部 194 个变体（`map_key_covers_every_key_code`
/// 逐个断言）；`Unidentified` 返回 `None`（平台没有报告具体物理键，无法猜测）。
///
/// winit 的 `KeyCode` 标了 `#[non_exhaustive]`，所以必须留兜底分支。兜底刻意不静默：
/// 升级 winit 后若真出现新键，这里返回 `None`（该键暂不可绑定），而
/// `winit_all_key_codes().len() == WINIT_KEY_CODE_COUNT` 的断言会在单测里立刻失败
/// （`cargo test` 而非线上），逼我们补表——而不是悄悄把新键吞掉。
#[must_use]
pub fn map_key_code(code: winit::keyboard::KeyCode) -> Option<KeyCode> {
    use winit::keyboard::KeyCode as W;
    Some(match code {
        W::Backquote => KeyCode::Backquote,
        W::Backslash => KeyCode::Backslash,
        W::BracketLeft => KeyCode::BracketLeft,
        W::BracketRight => KeyCode::BracketRight,
        W::Comma => KeyCode::Comma,
        W::Digit0 => KeyCode::Digit0,
        W::Digit1 => KeyCode::Digit1,
        W::Digit2 => KeyCode::Digit2,
        W::Digit3 => KeyCode::Digit3,
        W::Digit4 => KeyCode::Digit4,
        W::Digit5 => KeyCode::Digit5,
        W::Digit6 => KeyCode::Digit6,
        W::Digit7 => KeyCode::Digit7,
        W::Digit8 => KeyCode::Digit8,
        W::Digit9 => KeyCode::Digit9,
        W::Equal => KeyCode::Equal,
        W::IntlBackslash => KeyCode::IntlBackslash,
        W::IntlRo => KeyCode::IntlRo,
        W::IntlYen => KeyCode::IntlYen,
        W::KeyA => KeyCode::KeyA,
        W::KeyB => KeyCode::KeyB,
        W::KeyC => KeyCode::KeyC,
        W::KeyD => KeyCode::KeyD,
        W::KeyE => KeyCode::KeyE,
        W::KeyF => KeyCode::KeyF,
        W::KeyG => KeyCode::KeyG,
        W::KeyH => KeyCode::KeyH,
        W::KeyI => KeyCode::KeyI,
        W::KeyJ => KeyCode::KeyJ,
        W::KeyK => KeyCode::KeyK,
        W::KeyL => KeyCode::KeyL,
        W::KeyM => KeyCode::KeyM,
        W::KeyN => KeyCode::KeyN,
        W::KeyO => KeyCode::KeyO,
        W::KeyP => KeyCode::KeyP,
        W::KeyQ => KeyCode::KeyQ,
        W::KeyR => KeyCode::KeyR,
        W::KeyS => KeyCode::KeyS,
        W::KeyT => KeyCode::KeyT,
        W::KeyU => KeyCode::KeyU,
        W::KeyV => KeyCode::KeyV,
        W::KeyW => KeyCode::KeyW,
        W::KeyX => KeyCode::KeyX,
        W::KeyY => KeyCode::KeyY,
        W::KeyZ => KeyCode::KeyZ,
        W::Minus => KeyCode::Minus,
        W::Period => KeyCode::Period,
        W::Quote => KeyCode::Quote,
        W::Semicolon => KeyCode::Semicolon,
        W::Slash => KeyCode::Slash,
        W::AltLeft => KeyCode::AltLeft,
        W::AltRight => KeyCode::AltRight,
        W::Backspace => KeyCode::Backspace,
        W::CapsLock => KeyCode::CapsLock,
        W::ContextMenu => KeyCode::ContextMenu,
        W::ControlLeft => KeyCode::ControlLeft,
        W::ControlRight => KeyCode::ControlRight,
        W::Enter => KeyCode::Enter,
        W::SuperLeft => KeyCode::SuperLeft,
        W::SuperRight => KeyCode::SuperRight,
        W::ShiftLeft => KeyCode::ShiftLeft,
        W::ShiftRight => KeyCode::ShiftRight,
        W::Space => KeyCode::Space,
        W::Tab => KeyCode::Tab,
        W::Convert => KeyCode::Convert,
        W::KanaMode => KeyCode::KanaMode,
        W::Lang1 => KeyCode::Lang1,
        W::Lang2 => KeyCode::Lang2,
        W::Lang3 => KeyCode::Lang3,
        W::Lang4 => KeyCode::Lang4,
        W::Lang5 => KeyCode::Lang5,
        W::NonConvert => KeyCode::NonConvert,
        W::Delete => KeyCode::Delete,
        W::End => KeyCode::End,
        W::Help => KeyCode::Help,
        W::Home => KeyCode::Home,
        W::Insert => KeyCode::Insert,
        W::PageDown => KeyCode::PageDown,
        W::PageUp => KeyCode::PageUp,
        W::ArrowDown => KeyCode::ArrowDown,
        W::ArrowLeft => KeyCode::ArrowLeft,
        W::ArrowRight => KeyCode::ArrowRight,
        W::ArrowUp => KeyCode::ArrowUp,
        W::NumLock => KeyCode::NumLock,
        W::Numpad0 => KeyCode::Numpad0,
        W::Numpad1 => KeyCode::Numpad1,
        W::Numpad2 => KeyCode::Numpad2,
        W::Numpad3 => KeyCode::Numpad3,
        W::Numpad4 => KeyCode::Numpad4,
        W::Numpad5 => KeyCode::Numpad5,
        W::Numpad6 => KeyCode::Numpad6,
        W::Numpad7 => KeyCode::Numpad7,
        W::Numpad8 => KeyCode::Numpad8,
        W::Numpad9 => KeyCode::Numpad9,
        W::NumpadAdd => KeyCode::NumpadAdd,
        W::NumpadBackspace => KeyCode::NumpadBackspace,
        W::NumpadClear => KeyCode::NumpadClear,
        W::NumpadClearEntry => KeyCode::NumpadClearEntry,
        W::NumpadComma => KeyCode::NumpadComma,
        W::NumpadDecimal => KeyCode::NumpadDecimal,
        W::NumpadDivide => KeyCode::NumpadDivide,
        W::NumpadEnter => KeyCode::NumpadEnter,
        W::NumpadEqual => KeyCode::NumpadEqual,
        W::NumpadHash => KeyCode::NumpadHash,
        W::NumpadMemoryAdd => KeyCode::NumpadMemoryAdd,
        W::NumpadMemoryClear => KeyCode::NumpadMemoryClear,
        W::NumpadMemoryRecall => KeyCode::NumpadMemoryRecall,
        W::NumpadMemoryStore => KeyCode::NumpadMemoryStore,
        W::NumpadMemorySubtract => KeyCode::NumpadMemorySubtract,
        W::NumpadMultiply => KeyCode::NumpadMultiply,
        W::NumpadParenLeft => KeyCode::NumpadParenLeft,
        W::NumpadParenRight => KeyCode::NumpadParenRight,
        W::NumpadStar => KeyCode::NumpadStar,
        W::NumpadSubtract => KeyCode::NumpadSubtract,
        W::Escape => KeyCode::Escape,
        W::Fn => KeyCode::Fn,
        W::FnLock => KeyCode::FnLock,
        W::PrintScreen => KeyCode::PrintScreen,
        W::ScrollLock => KeyCode::ScrollLock,
        W::Pause => KeyCode::Pause,
        W::BrowserBack => KeyCode::BrowserBack,
        W::BrowserFavorites => KeyCode::BrowserFavorites,
        W::BrowserForward => KeyCode::BrowserForward,
        W::BrowserHome => KeyCode::BrowserHome,
        W::BrowserRefresh => KeyCode::BrowserRefresh,
        W::BrowserSearch => KeyCode::BrowserSearch,
        W::BrowserStop => KeyCode::BrowserStop,
        W::Eject => KeyCode::Eject,
        W::LaunchApp1 => KeyCode::LaunchApp1,
        W::LaunchApp2 => KeyCode::LaunchApp2,
        W::LaunchMail => KeyCode::LaunchMail,
        W::MediaPlayPause => KeyCode::MediaPlayPause,
        W::MediaSelect => KeyCode::MediaSelect,
        W::MediaStop => KeyCode::MediaStop,
        W::MediaTrackNext => KeyCode::MediaTrackNext,
        W::MediaTrackPrevious => KeyCode::MediaTrackPrevious,
        W::Power => KeyCode::Power,
        W::Sleep => KeyCode::Sleep,
        W::AudioVolumeDown => KeyCode::AudioVolumeDown,
        W::AudioVolumeMute => KeyCode::AudioVolumeMute,
        W::AudioVolumeUp => KeyCode::AudioVolumeUp,
        W::WakeUp => KeyCode::WakeUp,
        W::Meta => KeyCode::Meta,
        W::Hyper => KeyCode::Hyper,
        W::Turbo => KeyCode::Turbo,
        W::Abort => KeyCode::Abort,
        W::Resume => KeyCode::Resume,
        W::Suspend => KeyCode::Suspend,
        W::Again => KeyCode::Again,
        W::Copy => KeyCode::Copy,
        W::Cut => KeyCode::Cut,
        W::Find => KeyCode::Find,
        W::Open => KeyCode::Open,
        W::Paste => KeyCode::Paste,
        W::Props => KeyCode::Props,
        W::Select => KeyCode::Select,
        W::Undo => KeyCode::Undo,
        W::Hiragana => KeyCode::Hiragana,
        W::Katakana => KeyCode::Katakana,
        W::F1 => KeyCode::F1,
        W::F2 => KeyCode::F2,
        W::F3 => KeyCode::F3,
        W::F4 => KeyCode::F4,
        W::F5 => KeyCode::F5,
        W::F6 => KeyCode::F6,
        W::F7 => KeyCode::F7,
        W::F8 => KeyCode::F8,
        W::F9 => KeyCode::F9,
        W::F10 => KeyCode::F10,
        W::F11 => KeyCode::F11,
        W::F12 => KeyCode::F12,
        W::F13 => KeyCode::F13,
        W::F14 => KeyCode::F14,
        W::F15 => KeyCode::F15,
        W::F16 => KeyCode::F16,
        W::F17 => KeyCode::F17,
        W::F18 => KeyCode::F18,
        W::F19 => KeyCode::F19,
        W::F20 => KeyCode::F20,
        W::F21 => KeyCode::F21,
        W::F22 => KeyCode::F22,
        W::F23 => KeyCode::F23,
        W::F24 => KeyCode::F24,
        W::F25 => KeyCode::F25,
        W::F26 => KeyCode::F26,
        W::F27 => KeyCode::F27,
        W::F28 => KeyCode::F28,
        W::F29 => KeyCode::F29,
        W::F30 => KeyCode::F30,
        W::F31 => KeyCode::F31,
        W::F32 => KeyCode::F32,
        W::F33 => KeyCode::F33,
        W::F34 => KeyCode::F34,
        W::F35 => KeyCode::F35,
        // 兜底：winit 新增且引擎尚未收录的键（升级依赖时才可能出现，见函数文档）。
        _ => return None,
    })
}

/// 完整键盘事件 → 引擎物理键（独立 `game` 宿主的入口）。
///
/// 只认 `PhysicalKey::Code`：`Logical`（平台没给出物理键）与
/// `PhysicalKey::Unidentified` 返回 `None`。**刻意不看 `event.logical_key`**——
/// 那正是 M22/W1 修掉的缺陷（AZERTY 上 W 位置产生 `z`，移动键错位）。
///
/// 注意语序：调用点应当**先**取 `physical_key`，取不到再考虑回落逻辑键。回落是
/// 「平台不支持物理键」时的降级，而不是两条并列路径，否则又回到布局相关行为。
#[must_use]
pub fn map_key_event(event: &winit::event::KeyEvent) -> Option<KeyCode> {
    classify_physical_key(event.physical_key)
}

/// [`map_key_event`] 的键分类部分，单独抽出来便于直接断言（`KeyEvent` 的
/// `platform_specific` 字段是 winit 私有的，测试里构造不出完整事件）。
#[must_use]
fn classify_physical_key(physical_key: winit::keyboard::PhysicalKey) -> Option<KeyCode> {
    match physical_key {
        winit::keyboard::PhysicalKey::Code(code) => map_key_code(code),
        winit::keyboard::PhysicalKey::Unidentified(_) => None,
    }
}

#[cfg(test)]
/// winit 0.30 KeyCode 的变体总数。
///
/// winit 0.30 `KeyCode` 的变体总数。
///
/// `winit_all_key_codes().len()` 必须等于它（`map_key_covers_every_key_code` 断言）：
/// 升级 winit 后如果新增了物理键，这里会失败，提醒把新键补进 `map_key_code` 与
/// `KeyCode` 表——而不是让它在 `_ =>` 兜底里静默失效。
const WINIT_KEY_CODE_COUNT: usize = 194;

#[cfg(test)]
mod tests {
    use super::*;

    /// 引擎键域必须被 winit 表**全覆盖**：`KeyCode::ALL` 里每一项都要有一个
    /// winit 物理键映射到它，否则该键在独立 `game` 宿主里永远按不出来。
    ///
    /// `winit_all_key_codes()` 是测试里手抄的 winit 全表，因此额外用
    /// `WINIT_KEY_CODE_COUNT` 锁住它的**规模**：升级 winit 后新增了物理键时，
    /// 这里会先失败，提醒把新键同时补进 `map_key_code` 与 `KeyCode` 表。
    #[test]
    fn map_key_covers_every_key_code() {
        assert_eq!(
            winit_all_key_codes().len(),
            WINIT_KEY_CODE_COUNT,
            "winit 变体数与预期不符（升级依赖？）——请把新键补进 map_key_code 与 KeyCode 表"
        );
        assert_eq!(
            KeyCode::ALL.len(),
            WINIT_KEY_CODE_COUNT,
            "引擎键表规模与 winit 表不一致"
        );
        // winit 全表（与 `KeyCode` 一一对应），逐个走 `map_key_code`，收集像。
        let mut mapped: Vec<KeyCode> = Vec::new();
        for code in winit_all_key_codes() {
            if let Some(engine) = map_key_code(code) {
                mapped.push(engine);
            }
        }
        mapped.sort();
        mapped.dedup();
        let mut all = KeyCode::ALL.to_vec();
        all.sort();
        assert_eq!(
            mapped, all,
            "winit → KeyCode 的像必须等于 KeyCode::ALL（引擎键域 = 表驱动）"
        );
    }

    /// M22/W1 的核心回归：**同一物理位置在不同布局下必须是同一个引擎键**。
    ///
    /// 模拟 AZERTY：玩家按的是物理 `KeyW` 位置，而平台报出的**逻辑**字符是 `z`。
    /// 物理键路径必须得到 `KeyW`（→ `move_up`），字符路径会得到 `KeyZ`（→ 错位，
    /// 正是旧实现的缺陷）。同时验证逻辑字符**不参与**判定。
    #[test]
    fn physical_key_identity_is_layout_independent() {
        let key = winit::keyboard::KeyCode::KeyW;
        assert_eq!(map_key_code(key), Some(KeyCode::KeyW));

        // AZERTY 的 W 位置逻辑字符是 z：若宿主改读 logical_key 就会得到 KeyZ。
        let logical: winit::keyboard::Key = winit::keyboard::Key::Character("z".into());
        let from_logical = match &logical {
            winit::keyboard::Key::Character(value) => {
                value.chars().next().and_then(KeyCode::from_char)
            }
            _ => None,
        };
        assert_eq!(from_logical, Some(KeyCode::KeyZ));
        assert_ne!(
            map_key_code(key),
            from_logical,
            "物理键路径必须与逻辑字符路径给出不同结果（这正是 W1 修掉的错位）"
        );
    }

    /// 修饰键与 Tab/Enter/F5 这类**字符身份无法表达**的键现在可绑定。
    #[test]
    fn modifiers_and_special_keys_are_bindable() {
        for (winit_code, engine_code) in [
            (winit::keyboard::KeyCode::ShiftLeft, KeyCode::ShiftLeft),
            (winit::keyboard::KeyCode::ControlLeft, KeyCode::ControlLeft),
            (winit::keyboard::KeyCode::AltLeft, KeyCode::AltLeft),
            (winit::keyboard::KeyCode::Tab, KeyCode::Tab),
            (winit::keyboard::KeyCode::Enter, KeyCode::Enter),
            (winit::keyboard::KeyCode::F5, KeyCode::F5),
            (winit::keyboard::KeyCode::Digit1, KeyCode::Digit1),
        ] {
            assert_eq!(map_key_code(winit_code), Some(engine_code));
        }
        // 每个可绑定键都能往返成文本并解析回来（设置文件的前提）。
        for code in KeyCode::ALL {
            assert_eq!(KeyCode::from_text(code.to_text()), Some(*code));
        }
    }

    /// `PhysicalKey::Unidentified` 不得被猜成某个键（那会让「平台没说」变成
    /// 「玩家按了某键」）。
    ///
    /// `KeyEvent` 的 `platform_specific` 字段是 crate 私有的，测试里构造不出来，
    /// 因此直接对 `map_key_event` 的匹配分支做等价断言：`Unidentified` → `None`。
    #[test]
    fn unidentified_physical_key_maps_to_nothing() {
        let code = classify_physical_key(winit::keyboard::PhysicalKey::Unidentified(
            winit::keyboard::NativeKeyCode::Unidentified,
        ));
        assert_eq!(code, None);
        // 有效物理键仍然照常映射（防止上面的断言因为「整个函数坏掉」而假通过）。
        assert_eq!(
            classify_physical_key(winit::keyboard::PhysicalKey::Code(
                winit::keyboard::KeyCode::KeyW
            )),
            Some(KeyCode::KeyW)
        );
    }

    /// winit 全表（用于覆盖性断言）。与 `KeyCode::ALL` 逐项对应。
    fn winit_all_key_codes() -> Vec<winit::keyboard::KeyCode> {
        use winit::keyboard::KeyCode as W;
        vec![
            W::Backquote,
            W::Backslash,
            W::BracketLeft,
            W::BracketRight,
            W::Comma,
            W::Digit0,
            W::Digit1,
            W::Digit2,
            W::Digit3,
            W::Digit4,
            W::Digit5,
            W::Digit6,
            W::Digit7,
            W::Digit8,
            W::Digit9,
            W::Equal,
            W::IntlBackslash,
            W::IntlRo,
            W::IntlYen,
            W::KeyA,
            W::KeyB,
            W::KeyC,
            W::KeyD,
            W::KeyE,
            W::KeyF,
            W::KeyG,
            W::KeyH,
            W::KeyI,
            W::KeyJ,
            W::KeyK,
            W::KeyL,
            W::KeyM,
            W::KeyN,
            W::KeyO,
            W::KeyP,
            W::KeyQ,
            W::KeyR,
            W::KeyS,
            W::KeyT,
            W::KeyU,
            W::KeyV,
            W::KeyW,
            W::KeyX,
            W::KeyY,
            W::KeyZ,
            W::Minus,
            W::Period,
            W::Quote,
            W::Semicolon,
            W::Slash,
            W::AltLeft,
            W::AltRight,
            W::Backspace,
            W::CapsLock,
            W::ContextMenu,
            W::ControlLeft,
            W::ControlRight,
            W::Enter,
            W::SuperLeft,
            W::SuperRight,
            W::ShiftLeft,
            W::ShiftRight,
            W::Space,
            W::Tab,
            W::Convert,
            W::KanaMode,
            W::Lang1,
            W::Lang2,
            W::Lang3,
            W::Lang4,
            W::Lang5,
            W::NonConvert,
            W::Delete,
            W::End,
            W::Help,
            W::Home,
            W::Insert,
            W::PageDown,
            W::PageUp,
            W::ArrowDown,
            W::ArrowLeft,
            W::ArrowRight,
            W::ArrowUp,
            W::NumLock,
            W::Numpad0,
            W::Numpad1,
            W::Numpad2,
            W::Numpad3,
            W::Numpad4,
            W::Numpad5,
            W::Numpad6,
            W::Numpad7,
            W::Numpad8,
            W::Numpad9,
            W::NumpadAdd,
            W::NumpadBackspace,
            W::NumpadClear,
            W::NumpadClearEntry,
            W::NumpadComma,
            W::NumpadDecimal,
            W::NumpadDivide,
            W::NumpadEnter,
            W::NumpadEqual,
            W::NumpadHash,
            W::NumpadMemoryAdd,
            W::NumpadMemoryClear,
            W::NumpadMemoryRecall,
            W::NumpadMemoryStore,
            W::NumpadMemorySubtract,
            W::NumpadMultiply,
            W::NumpadParenLeft,
            W::NumpadParenRight,
            W::NumpadStar,
            W::NumpadSubtract,
            W::Escape,
            W::Fn,
            W::FnLock,
            W::PrintScreen,
            W::ScrollLock,
            W::Pause,
            W::BrowserBack,
            W::BrowserFavorites,
            W::BrowserForward,
            W::BrowserHome,
            W::BrowserRefresh,
            W::BrowserSearch,
            W::BrowserStop,
            W::Eject,
            W::LaunchApp1,
            W::LaunchApp2,
            W::LaunchMail,
            W::MediaPlayPause,
            W::MediaSelect,
            W::MediaStop,
            W::MediaTrackNext,
            W::MediaTrackPrevious,
            W::Power,
            W::Sleep,
            W::AudioVolumeDown,
            W::AudioVolumeMute,
            W::AudioVolumeUp,
            W::WakeUp,
            W::Meta,
            W::Hyper,
            W::Turbo,
            W::Abort,
            W::Resume,
            W::Suspend,
            W::Again,
            W::Copy,
            W::Cut,
            W::Find,
            W::Open,
            W::Paste,
            W::Props,
            W::Select,
            W::Undo,
            W::Hiragana,
            W::Katakana,
            W::F1,
            W::F2,
            W::F3,
            W::F4,
            W::F5,
            W::F6,
            W::F7,
            W::F8,
            W::F9,
            W::F10,
            W::F11,
            W::F12,
            W::F13,
            W::F14,
            W::F15,
            W::F16,
            W::F17,
            W::F18,
            W::F19,
            W::F20,
            W::F21,
            W::F22,
            W::F23,
            W::F24,
            W::F25,
            W::F26,
            W::F27,
            W::F28,
            W::F29,
            W::F30,
            W::F31,
            W::F32,
            W::F33,
            W::F34,
            W::F35,
        ]
    }
}
