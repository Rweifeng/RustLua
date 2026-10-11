//! `egui::Key` ⇄ 引擎物理键的**唯一**转换层（M22/W1，编辑器侧）。
//!
//! 引擎的 winit 表在 `engine::host::keymap`；egui 这张表放在编辑器，因为只有编辑器
//! 是 egui 宿主——`engine` 不为编辑器专用的 GUI 类型引入依赖（headless/Web 目标
//! 因此不必为它付代价）。
//!
//! 在 M22/W1 之前，「egui 键 → 引擎键」在两个地方各写了一遍
//! （`game_view::PREVIEW_KEYS` 手写 60 项 + `input_bindings` 的字符/命名两张表），
//! 且 `PREVIEW_KEYS` **没有对齐测试**，加键必然漂移。现在：
//!
//! * [`egui_input_key`] 是本文件唯一的「egui → 引擎」入口（预览桥接与重绑定捕获共用）；
//! * [`preview_keys`] 不是手写清单，而是 [`KeyCode::ALL`] 在 [`egui_key`] 下的**像**；
//! * `preview_keys_match_the_engine_key_table` / `egui_round_trip_covers_every_key_code`
//!   锁住「引擎加键 → 预览自动跟随」。
//!
//! egui 0.33 的 `Key` 本身是**布局无关**的枚举（`A`..`Z`、`Num0`..`Num9`、`Arrow*`、
//! `F1`..`F35`…），语义上就是物理位置，所以这是一一映射；egui 表达不了的引擎键
//! （小键盘运算键、媒体键、`Lang*`…）返回 `None`，那是 egui 的能力边界，不是引擎
//! 键域的边界（独立 `game` 宿主全部支持）。

use eframe::egui;
use rustlua_input::input::KeyCode;

/// `egui::Key` → 引擎物理键（Play 预览桥接 + 重绑定捕获的唯一入口）。
///
/// **全函数**：egui 0.33 的每个 `Key` 都能落到一个物理键，因此没有 `_ =>` 兜底——
/// 未来 egui 加了新键，这里会编译失败（`egui::Key` 加了 `#[non_exhaustive]` 则退化为
/// 编译错误而非静默丢键），而不是悄悄吞掉玩家的输入。
#[must_use]
pub(crate) fn egui_input_key(key: egui::Key) -> Option<KeyCode> {
    use egui::Key as E;
    Some(match key {
        E::ArrowDown => KeyCode::ArrowDown,
        E::ArrowLeft => KeyCode::ArrowLeft,
        E::ArrowRight => KeyCode::ArrowRight,
        E::ArrowUp => KeyCode::ArrowUp,
        E::Escape => KeyCode::Escape,
        E::Tab => KeyCode::Tab,
        E::Backspace => KeyCode::Backspace,
        E::Enter => KeyCode::Enter,
        E::Insert => KeyCode::Insert,
        E::Delete => KeyCode::Delete,
        E::Home => KeyCode::Home,
        E::End => KeyCode::End,
        E::PageUp => KeyCode::PageUp,
        E::PageDown => KeyCode::PageDown,
        E::Copy => KeyCode::Copy,
        E::Cut => KeyCode::Cut,
        E::Paste => KeyCode::Paste,
        // 标点：egui 把同一个物理键的「带 Shift」写法也列成了独立变体，这里统一
        // 落到未按 Shift 的物理键（与 `KeyCode::v1_key_code` 的迁移口径一致）。
        E::Colon => KeyCode::Semicolon,
        E::Comma => KeyCode::Comma,
        E::Backslash => KeyCode::Backslash,
        E::Slash => KeyCode::Slash,
        E::Pipe => KeyCode::Backslash,
        E::Questionmark => KeyCode::Slash,
        E::OpenBracket => KeyCode::BracketLeft,
        E::CloseBracket => KeyCode::BracketRight,
        E::OpenCurlyBracket => KeyCode::BracketLeft,
        E::CloseCurlyBracket => KeyCode::BracketRight,
        E::Backtick => KeyCode::Backquote,
        E::Minus => KeyCode::Minus,
        E::Period => KeyCode::Period,
        E::Plus => KeyCode::Equal,
        E::Equals => KeyCode::Equal,
        E::Semicolon => KeyCode::Semicolon,
        E::Quote => KeyCode::Quote,
        E::Exclamationmark => KeyCode::Digit1,
        E::Num0 => KeyCode::Digit0,
        E::Num1 => KeyCode::Digit1,
        E::Num2 => KeyCode::Digit2,
        E::Num3 => KeyCode::Digit3,
        E::Num4 => KeyCode::Digit4,
        E::Num5 => KeyCode::Digit5,
        E::Num6 => KeyCode::Digit6,
        E::Num7 => KeyCode::Digit7,
        E::Num8 => KeyCode::Digit8,
        E::Num9 => KeyCode::Digit9,
        E::A => KeyCode::KeyA,
        E::B => KeyCode::KeyB,
        E::C => KeyCode::KeyC,
        E::D => KeyCode::KeyD,
        E::E => KeyCode::KeyE,
        E::F => KeyCode::KeyF,
        E::G => KeyCode::KeyG,
        E::H => KeyCode::KeyH,
        E::I => KeyCode::KeyI,
        E::J => KeyCode::KeyJ,
        E::K => KeyCode::KeyK,
        E::L => KeyCode::KeyL,
        E::M => KeyCode::KeyM,
        E::N => KeyCode::KeyN,
        E::O => KeyCode::KeyO,
        E::P => KeyCode::KeyP,
        E::Q => KeyCode::KeyQ,
        E::R => KeyCode::KeyR,
        E::S => KeyCode::KeyS,
        E::T => KeyCode::KeyT,
        E::U => KeyCode::KeyU,
        E::V => KeyCode::KeyV,
        E::W => KeyCode::KeyW,
        E::X => KeyCode::KeyX,
        E::Y => KeyCode::KeyY,
        E::Z => KeyCode::KeyZ,
        E::F1 => KeyCode::F1,
        E::F2 => KeyCode::F2,
        E::F3 => KeyCode::F3,
        E::F4 => KeyCode::F4,
        E::F5 => KeyCode::F5,
        E::F6 => KeyCode::F6,
        E::F7 => KeyCode::F7,
        E::F8 => KeyCode::F8,
        E::F9 => KeyCode::F9,
        E::F10 => KeyCode::F10,
        E::F11 => KeyCode::F11,
        E::F12 => KeyCode::F12,
        E::F13 => KeyCode::F13,
        E::F14 => KeyCode::F14,
        E::F15 => KeyCode::F15,
        E::F16 => KeyCode::F16,
        E::F17 => KeyCode::F17,
        E::F18 => KeyCode::F18,
        E::F19 => KeyCode::F19,
        E::F20 => KeyCode::F20,
        E::F21 => KeyCode::F21,
        E::F22 => KeyCode::F22,
        E::F23 => KeyCode::F23,
        E::F24 => KeyCode::F24,
        E::F25 => KeyCode::F25,
        E::F26 => KeyCode::F26,
        E::F27 => KeyCode::F27,
        E::F28 => KeyCode::F28,
        E::F29 => KeyCode::F29,
        E::F30 => KeyCode::F30,
        E::F31 => KeyCode::F31,
        E::F32 => KeyCode::F32,
        E::F33 => KeyCode::F33,
        E::F34 => KeyCode::F34,
        E::F35 => KeyCode::F35,
        E::BrowserBack => KeyCode::BrowserBack,
        E::Space => KeyCode::Space,
    })
}

/// 引擎物理键 → `egui::Key`（[`egui_input_key`] 的规范逆，供预览键集合生成）。
///
/// 与 [`egui_input_key`] 的关系满足
/// `egui_input_key(egui_key(k)) == Some(k)`（`egui_round_trip_covers_every_key_code`
/// 对全表断言）；egui 表达不了的引擎键返回 `None`。
#[must_use]
pub(crate) fn egui_key(code: KeyCode) -> Option<egui::Key> {
    use egui::Key as E;
    Some(match code {
        KeyCode::ArrowDown => E::ArrowDown,
        KeyCode::ArrowLeft => E::ArrowLeft,
        KeyCode::ArrowRight => E::ArrowRight,
        KeyCode::ArrowUp => E::ArrowUp,
        KeyCode::Escape => E::Escape,
        KeyCode::Tab => E::Tab,
        KeyCode::Backspace => E::Backspace,
        KeyCode::Enter => E::Enter,
        KeyCode::Insert => E::Insert,
        KeyCode::Delete => E::Delete,
        KeyCode::Home => E::Home,
        KeyCode::End => E::End,
        KeyCode::PageUp => E::PageUp,
        KeyCode::PageDown => E::PageDown,
        KeyCode::Copy => E::Copy,
        KeyCode::Cut => E::Cut,
        KeyCode::Paste => E::Paste,
        KeyCode::Semicolon => E::Semicolon,
        KeyCode::Comma => E::Comma,
        KeyCode::Backslash => E::Backslash,
        KeyCode::Slash => E::Slash,
        KeyCode::BracketLeft => E::OpenBracket,
        KeyCode::BracketRight => E::CloseBracket,
        KeyCode::Backquote => E::Backtick,
        KeyCode::Minus => E::Minus,
        KeyCode::Period => E::Period,
        // `=` 与 `+` 是同一物理键：规范逆取**未按 Shift** 的 `Equals`（与
        // `egui_input_key` 把两者都落到 `Equal` 一致，往返因此唯一）。
        KeyCode::Equal => E::Equals,
        KeyCode::Quote => E::Quote,
        KeyCode::Digit0 => E::Num0,
        KeyCode::Digit1 => E::Num1,
        KeyCode::Digit2 => E::Num2,
        KeyCode::Digit3 => E::Num3,
        KeyCode::Digit4 => E::Num4,
        KeyCode::Digit5 => E::Num5,
        KeyCode::Digit6 => E::Num6,
        KeyCode::Digit7 => E::Num7,
        KeyCode::Digit8 => E::Num8,
        KeyCode::Digit9 => E::Num9,
        KeyCode::KeyA => E::A,
        KeyCode::KeyB => E::B,
        KeyCode::KeyC => E::C,
        KeyCode::KeyD => E::D,
        KeyCode::KeyE => E::E,
        KeyCode::KeyF => E::F,
        KeyCode::KeyG => E::G,
        KeyCode::KeyH => E::H,
        KeyCode::KeyI => E::I,
        KeyCode::KeyJ => E::J,
        KeyCode::KeyK => E::K,
        KeyCode::KeyL => E::L,
        KeyCode::KeyM => E::M,
        KeyCode::KeyN => E::N,
        KeyCode::KeyO => E::O,
        KeyCode::KeyP => E::P,
        KeyCode::KeyQ => E::Q,
        KeyCode::KeyR => E::R,
        KeyCode::KeyS => E::S,
        KeyCode::KeyT => E::T,
        KeyCode::KeyU => E::U,
        KeyCode::KeyV => E::V,
        KeyCode::KeyW => E::W,
        KeyCode::KeyX => E::X,
        KeyCode::KeyY => E::Y,
        KeyCode::KeyZ => E::Z,
        KeyCode::F1 => E::F1,
        KeyCode::F2 => E::F2,
        KeyCode::F3 => E::F3,
        KeyCode::F4 => E::F4,
        KeyCode::F5 => E::F5,
        KeyCode::F6 => E::F6,
        KeyCode::F7 => E::F7,
        KeyCode::F8 => E::F8,
        KeyCode::F9 => E::F9,
        KeyCode::F10 => E::F10,
        KeyCode::F11 => E::F11,
        KeyCode::F12 => E::F12,
        KeyCode::F13 => E::F13,
        KeyCode::F14 => E::F14,
        KeyCode::F15 => E::F15,
        KeyCode::F16 => E::F16,
        KeyCode::F17 => E::F17,
        KeyCode::F18 => E::F18,
        KeyCode::F19 => E::F19,
        KeyCode::F20 => E::F20,
        KeyCode::F21 => E::F21,
        KeyCode::F22 => E::F22,
        KeyCode::F23 => E::F23,
        KeyCode::F24 => E::F24,
        KeyCode::F25 => E::F25,
        KeyCode::F26 => E::F26,
        KeyCode::F27 => E::F27,
        KeyCode::F28 => E::F28,
        KeyCode::F29 => E::F29,
        KeyCode::F30 => E::F30,
        KeyCode::F31 => E::F31,
        KeyCode::F32 => E::F32,
        KeyCode::F33 => E::F33,
        KeyCode::F34 => E::F34,
        KeyCode::F35 => E::F35,
        KeyCode::BrowserBack => E::BrowserBack,
        KeyCode::Space => E::Space,
        _ => return None,
    })
}

/// 编辑器 Play 预览每帧桥接的 egui 键集合：**引擎键表的像**，不是手写清单。
///
/// 列表必须固定：预览宿主每帧把整个集合喂进快照（按住的送按住、其余送松开），
/// 少了任何一项该键的松开边沿就永远不来（粘键）。因此这里对 [`KeyCode::ALL`] 求像并
/// 按 egui 的 `name()` 排序去重——「引擎加了键」自动出现在预览里，不会再出现
/// 「引擎表已扩、预览清单漏项」的静默漂移。
#[must_use]
pub(crate) fn preview_keys() -> Vec<egui::Key> {
    let mut keys: Vec<egui::Key> = KeyCode::ALL
        .iter()
        .filter_map(|code| egui_key(*code))
        .collect();
    keys.sort_by_key(|key| key.name());
    keys.dedup();
    keys
}

/// egui 里**额外**的键名别称：与 [`preview_keys`] 中的某个键落到**同一物理键**。
///
/// egui-winit 交给 egui 的是**逻辑**键，而引擎键身份是**物理**键，于是同一个键在不同
/// 修饰下会以两个名字出现：`Shift+\``=` 在按住 Shift 时报的是 [`egui::Key::Plus`]，
/// 未按 Shift 时报 [`egui::Key::Equals`]。规范集合里只能放一个（否则
/// [`egui_key`]/[`egui_input_key`] 的往返就不唯一），另一个必须作为**别称**参与按住
/// 判定，否则「Shift + `=`」在预览里会被当成「没按」——按住的物理键还会因为规范名
/// 不成立而收到一条虚假松开边沿。
///
/// 别称不进 [`preview_keys`]：那个集合是逐帧**全量写状态**的说明（每项都要送按住/松开），
/// 别称只用于把「按住」的判定做宽，不单独产生事件。
#[must_use]
pub(crate) fn egui_key_aliases() -> &'static [(egui::Key, KeyCode)] {
    &[(egui::Key::Plus, KeyCode::Equal)]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 别称必须真的落到规范集合里已有的物理键，且**不与**规范集合重复。
    #[test]
    fn preview_aliases_only_widen_held_state() {
        let canonical = preview_keys();
        for (alias, code) in egui_key_aliases() {
            assert_eq!(
                egui_input_key(*alias),
                Some(*code),
                "别称 {alias:?} 必须映射到 {code:?}"
            );
            assert!(
                !canonical.contains(alias),
                "别称 {alias:?} 不得同时出现在规范集合里（否则桥接会重复送边沿）"
            );
            assert!(
                canonical
                    .iter()
                    .any(|key| egui_input_key(*key) == Some(*code)),
                "别称 {alias:?} 指向的 {code:?} 必须有一个规范键名"
            );
        }
    }

    /// 预览键集合必须**恰好**是引擎键表在 egui 上的像：漏一项会粘键，多一项会在
    /// `egui_input_key` 里落 `None`（每帧送无效键）。
    #[test]
    fn preview_keys_match_the_engine_key_table() {
        let preview = preview_keys();
        for key in &preview {
            assert!(
                egui_input_key(*key).is_some(),
                "预览键 {key:?} 必须能转成引擎键"
            );
        }
        let mut expected: Vec<egui::Key> = KeyCode::ALL
            .iter()
            .filter_map(|code| egui_key(*code))
            .collect();
        expected.sort_by_key(|key| key.name());
        expected.dedup();
        assert_eq!(preview, expected);
        assert_eq!(preview.len(), expected.len());
        // 关键子集必须在场（防「表被删到只剩几项」而不报警）。
        for required in [
            egui::Key::A,
            egui::Key::W,
            egui::Key::Z,
            egui::Key::Num0,
            egui::Key::Num9,
            egui::Key::ArrowUp,
            egui::Key::ArrowDown,
            egui::Key::ArrowLeft,
            egui::Key::ArrowRight,
            egui::Key::Space,
            egui::Key::Escape,
            egui::Key::Tab,
            egui::Key::Enter,
            egui::Key::Backspace,
            egui::Key::F1,
            egui::Key::F5,
            egui::Key::F12,
            egui::Key::Minus,
            egui::Key::Equals,
            egui::Key::Backslash,
        ] {
            assert!(preview.contains(&required), "预览键集合缺 {required:?}");
        }
    }

    /// 两张表互为逆：`egui_input_key(egui_key(k)) == Some(k)`，对引擎全表成立。
    #[test]
    fn egui_round_trip_covers_every_key_code() {
        let mut covered = 0_usize;
        for code in KeyCode::ALL {
            if let Some(key) = egui_key(*code) {
                assert_eq!(
                    egui_input_key(key),
                    Some(*code),
                    "{code:?} 经 egui 往返后必须回到自身"
                );
                covered += 1;
            }
        }
        // egui 表达得了的键远多于「字母 + 数字 + 方向 + F 键」，给一个下限防止表被
        // 误删到只剩几项。
        assert!(covered >= 100, "egui 覆盖数偏低：{covered}");
    }

    /// M22/W1：Tab/Enter/Backspace/F5 与数字键现在**可捕获、可绑定**——旧实现里
    /// Tab/Enter 根本不在键域内（`preview_input_key` 返回 `None`），数字键只有
    /// `Shift` 未按时才等价。
    #[test]
    fn special_keys_are_in_the_preview_domain() {
        for (egui_key_value, engine_key_value) in [
            (egui::Key::Tab, KeyCode::Tab),
            (egui::Key::Enter, KeyCode::Enter),
            (egui::Key::Backspace, KeyCode::Backspace),
            (egui::Key::F5, KeyCode::F5),
            (egui::Key::Num1, KeyCode::Digit1),
            (egui::Key::Exclamationmark, KeyCode::Digit1),
            (egui::Key::Plus, KeyCode::Equal),
            (egui::Key::Equals, KeyCode::Equal),
        ] {
            assert_eq!(egui_input_key(egui_key_value), Some(engine_key_value));
        }
    }
}
