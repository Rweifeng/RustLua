//! 编辑器快捷键：**唯一动作表** + 分发匹配 + 改键规则。
//!
//! 在 M20-08 之后，工程内的**游戏输入**可以在「项目设置」里改键；编辑器的**快捷键**
//! 此前是 `main.rs` 里散落的 `if i.key_pressed(...)` 硬编码，菜单里写死的提示字符串
//! 与实际键位只靠一条测试对齐（`menu_shortcuts_match_the_documented_ones`），玩家
//! 改不了、菜单也不会跟着变。本模块把这份清单收成一张表：
//!
//! * [`EditorShortcut`]：动作 id（`editor.json` 的 `shortcuts` 段键名，**稳定**）、
//!   中文名、默认键、是否需要场景工作台、是否被文本焦点拦截、对应的菜单动作；
//! * [`binding`] / [`pressed`]：从用户改键表取出生效键位并匹配 `egui` 输入；
//! * 菜单提示（`menu.rs`）与快捷键窗口都从同一张表取**生效**键位，改键后菜单不会撒谎。
//!
//! 键位文本的解析、规范写法与冲突检测在 `rustlua_editor_core::shortcuts`（无 egui）。

use eframe::egui;
use rustlua_editor_core::{ShortcutBindings, ShortcutSpec};

use crate::menu::MenuAction;

/// 可改键的编辑器动作。
///
/// `id` 写进用户配置，**改了等于把用户的改键丢掉**；增删动作可以，改 id 不行。
/// 主键名沿用 `egui::Key::name()` 的写法（`P`、`Delete`、`F5`…）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditorShortcut {
    Undo,
    Redo,
    Save,
    SaveLayout,
    Palette,
    Duplicate,
    Copy,
    Paste,
    Delete,
    SelectAll,
    Search,
    ToolSelect,
    ToolMove,
    ToolRotate,
    ToolScale,
}

impl EditorShortcut {
    /// 显示与校验顺序（也是快捷键窗口的行顺序）。
    pub(crate) const ALL: [Self; 15] = [
        Self::Undo,
        Self::Redo,
        Self::Save,
        Self::SaveLayout,
        Self::Palette,
        Self::Duplicate,
        Self::Copy,
        Self::Paste,
        Self::Delete,
        Self::SelectAll,
        Self::Search,
        Self::ToolSelect,
        Self::ToolMove,
        Self::ToolRotate,
        Self::ToolScale,
    ];

    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::Save => "save_scene",
            Self::SaveLayout => "save_layout",
            Self::Palette => "command_palette",
            Self::Duplicate => "duplicate_entity",
            Self::Copy => "copy_entity",
            Self::Paste => "paste_entity",
            Self::Delete => "delete_entity",
            Self::SelectAll => "select_all",
            Self::Search => "project_search",
            Self::ToolSelect => "tool_select",
            Self::ToolMove => "tool_move",
            Self::ToolRotate => "tool_rotate",
            Self::ToolScale => "tool_scale",
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Undo => "撤销",
            Self::Redo => "重做",
            Self::Save => "保存场景",
            Self::SaveLayout => "保存布局",
            Self::Palette => "命令面板",
            Self::Duplicate => "复制实体",
            Self::Copy => "复制到剪贴板",
            Self::Paste => "粘贴为子节点",
            Self::Delete => "删除实体",
            Self::SelectAll => "全选实体",
            Self::Search => "全工程搜索",
            Self::ToolSelect => "工具：选择",
            Self::ToolMove => "工具：移动",
            Self::ToolRotate => "工具：旋转",
            Self::ToolScale => "工具：缩放",
        }
    }

    /// 默认键位必须能被 `ShortcutSpec::parse` 解析，且两两不冲突（有测试盯住）。
    pub(crate) const fn default_spec(self) -> &'static str {
        match self {
            Self::Undo => "Ctrl+Z",
            Self::Redo => "Ctrl+Y",
            Self::Save => "Ctrl+S",
            Self::SaveLayout => "Ctrl+Shift+L",
            Self::Palette => "Ctrl+Shift+P",
            Self::Duplicate => "Ctrl+D",
            Self::Copy => "Ctrl+C",
            Self::Paste => "Ctrl+V",
            Self::Delete => "Delete",
            Self::SelectAll => "Ctrl+A",
            Self::Search => "Ctrl+Shift+F",
            Self::ToolSelect => "Q",
            Self::ToolMove => "W",
            Self::ToolRotate => "E",
            Self::ToolScale => "R",
        }
    }
}

/// 菜单动作 ↔ 快捷键动作：菜单标签要在改键后显示**生效**键位。
pub(crate) const fn for_menu_action(action: MenuAction) -> Option<EditorShortcut> {
    Some(match action {
        MenuAction::SaveScene => EditorShortcut::Save,
        MenuAction::Undo => EditorShortcut::Undo,
        MenuAction::Redo => EditorShortcut::Redo,
        MenuAction::DuplicateSelection => EditorShortcut::Duplicate,
        MenuAction::DeleteSelection => EditorShortcut::Delete,
        MenuAction::SelectAll => EditorShortcut::SelectAll,
        MenuAction::SaveLayout => EditorShortcut::SaveLayout,
        MenuAction::OpenProjectSearch => EditorShortcut::Search,
        _ => return None,
    })
}

/// 动作当前生效的键位：用户改键 → 默认键。
///
/// 磁盘上的坏值在 `ShortcutBindings::from_entries` 已丢弃；这里再解析一次是因为
/// `ShortcutBindings` 也允许外部塞入未经 `set` 的文本，解析失败就回落默认。
pub(crate) fn binding(bindings: &ShortcutBindings, action: EditorShortcut) -> ShortcutSpec {
    bindings
        .get(action.id())
        .and_then(|text| ShortcutSpec::parse(text).ok())
        .or_else(|| ShortcutSpec::parse(action.default_spec()).ok())
        .expect("默认快捷键必须是可解析的（有测试盯住）")
}

/// 改键冲突：`spec` 是否已被**别的动作的生效键位**占用（含默认键，不只是用户改过的）。
pub(crate) fn conflicting_action(
    bindings: &ShortcutBindings,
    action: EditorShortcut,
    spec: &ShortcutSpec,
) -> Option<EditorShortcut> {
    EditorShortcut::ALL
        .iter()
        .copied()
        .find(|other| *other != action && binding(bindings, *other).same_as(spec))
}

/// 某条键位是否已被**内置动作的生效键位**占用。
///
/// 插件命令不能用同一个组合抢内置动作（内置优先）；返回占用的动作，供诊断页说明
/// 插件那条快捷键为什么不生效。
pub(crate) fn builtin_owner(
    bindings: &ShortcutBindings,
    spec: &ShortcutSpec,
) -> Option<EditorShortcut> {
    EditorShortcut::ALL
        .iter()
        .copied()
        .find(|action| binding(bindings, *action).same_as(spec))
}

/// 本帧是否按下了该键位（修饰键精确匹配：多按了 Shift 不算）。
///
/// 看的是**按键事件自带的修饰键**（`egui::Event::Key.modifiers`），不是帧末的
/// `InputState::modifiers`：同帧里先按 Ctrl 再松 Shift 的顺序不会影响判定，也与
/// 改键捕获用同一份数据。长按的重复事件不算（避免按住 Ctrl+S 连续保存）。
pub(crate) fn pressed(input: &egui::InputState, spec: &ShortcutSpec) -> bool {
    let Some(key) = egui::Key::from_name(&spec.key) else {
        return false;
    };
    input.events.iter().any(|event| {
        matches!(
            event,
            egui::Event::Key {
                key: pressed_key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } if *pressed_key == key
                && modifiers.command == spec.ctrl
                && modifiers.shift == spec.shift
                && modifiers.alt == spec.alt
        )
    })
}

/// 改键规则：拒绝会毁掉日常输入的键位，返回原因。
///
/// 裸字母/数字（没有 Ctrl/Alt/Shift）会在文本框里被当成打字，因此不允许；`Delete`、
/// `F5`、`Escape` 这类命名键没有这个歧义。
pub(crate) fn binding_problem(spec: &ShortcutSpec) -> Option<&'static str> {
    let single_alnum = spec.key.len() == 1 && spec.key.chars().all(|ch| ch.is_ascii_alphanumeric());
    if !spec.has_modifier() && single_alnum {
        return Some("单个字母/数字键需要搭配 Ctrl、Alt 或 Shift");
    }
    if egui::Key::from_name(&spec.key).is_none() {
        return Some("egui 不认识这个键名");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_parse_and_never_conflict() {
        // 默认键里允许裸字母（Q/W/E/R 工具切换），它们由调用点的文本焦点门槛保护；
        // `binding_problem` 只拦**改键**，不拦出厂默认。
        let mut seen = Vec::new();
        for action in EditorShortcut::ALL {
            let spec = ShortcutSpec::parse(action.default_spec())
                .unwrap_or_else(|error| panic!("{} 的默认键位无法解析：{error}", action.label()));
            assert!(
                egui::Key::from_name(&spec.key).is_some(),
                "{} 的默认键位 egui 不认识：{}",
                action.label(),
                spec.key
            );
            assert!(
                !seen.iter().any(|other: &ShortcutSpec| other.same_as(&spec)),
                "{} 的默认键位与别的动作重复：{}",
                action.label(),
                spec.display()
            );
            seen.push(spec);
        }
    }

    #[test]
    fn ids_and_labels_are_unique_and_stable() {
        let mut ids = Vec::new();
        for action in EditorShortcut::ALL {
            assert!(!ids.contains(&action.id()), "动作 id 重复：{}", action.id());
            assert!(!action.label().is_empty());
            ids.push(action.id());
        }
        // 写进用户配置的 id 改了就等于丢用户的改键；逐个钉住。
        assert_eq!(
            ids,
            vec![
                "undo",
                "redo",
                "save_scene",
                "save_layout",
                "command_palette",
                "duplicate_entity",
                "copy_entity",
                "paste_entity",
                "delete_entity",
                "select_all",
                "project_search",
                "tool_select",
                "tool_move",
                "tool_rotate",
                "tool_scale",
            ]
        );
    }

    #[test]
    fn user_bindings_override_defaults() {
        let mut bindings = ShortcutBindings::new();
        assert_eq!(binding(&bindings, EditorShortcut::Save).display(), "Ctrl+S");
        bindings
            .set(EditorShortcut::Save.id(), "ctrl+shift+s")
            .unwrap();
        assert_eq!(
            binding(&bindings, EditorShortcut::Save).display(),
            "Ctrl+Shift+S"
        );
        bindings.remove(EditorShortcut::Save.id());
        assert_eq!(
            binding(&bindings, EditorShortcut::Save).display(),
            "Ctrl+S",
            "删掉改键就回到默认"
        );
    }

    /// 冲突要连**默认键**一起看：用户从没改过命令面板，Ctrl+Shift+P 也仍然被占用。
    #[test]
    fn conflicts_consider_default_bindings() {
        let bindings = ShortcutBindings::new();
        let palette = ShortcutSpec::parse("Ctrl+Shift+P").unwrap();
        assert_eq!(
            conflicting_action(&bindings, EditorShortcut::Save, &palette),
            Some(EditorShortcut::Palette)
        );
        let free = ShortcutSpec::parse("Ctrl+Alt+F5").unwrap();
        assert_eq!(
            conflicting_action(&bindings, EditorShortcut::Save, &free),
            None
        );
        // 动作自己的默认键不算冲突（否则任何动作都无法保持默认）。
        let own = ShortcutSpec::parse("Ctrl+S").unwrap();
        assert_eq!(
            conflicting_action(&bindings, EditorShortcut::Save, &own),
            None
        );
    }

    #[test]
    fn menu_actions_map_back_to_shortcuts() {
        assert_eq!(
            for_menu_action(MenuAction::SaveScene),
            Some(EditorShortcut::Save)
        );
        assert_eq!(
            for_menu_action(MenuAction::DeleteSelection),
            Some(EditorShortcut::Delete)
        );
        assert_eq!(for_menu_action(MenuAction::About), None);
    }

    #[test]
    fn bare_letters_are_rejected_but_named_keys_are_fine() {
        let ctrl_p = ShortcutSpec::parse("Ctrl+P").unwrap();
        assert!(binding_problem(&ctrl_p).is_none());
        let bare_q = ShortcutSpec::parse("Q").unwrap();
        assert!(binding_problem(&bare_q).is_some(), "裸字母会吃掉打字输入");
        let delete = ShortcutSpec::parse("Delete").unwrap();
        assert!(binding_problem(&delete).is_none());
        let f5 = ShortcutSpec::parse("F5").unwrap();
        assert!(binding_problem(&f5).is_none());
    }

    /// 真按一次：`pressed` 只看键位，不看焦点——焦点门槛由调用方决定。
    ///
    /// `Modifiers::command` 由后端按平台填在**按键事件**上（Windows/Linux = Ctrl，
    /// macOS = Cmd），这里的合成事件照后端的口径一起填。
    #[test]
    fn pressed_matches_exact_modifiers() {
        let spec = ShortcutSpec::parse("Ctrl+Shift+P").unwrap();
        let run = |modifiers: egui::Modifiers| {
            let ctx = egui::Context::default();
            let raw = egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::P,
                    physical_key: Some(egui::Key::P),
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            };
            ctx.begin_pass(raw);
            let pressed = ctx.input(|input| pressed(input, &spec));
            let _ = ctx.end_pass();
            pressed
        };
        assert!(run(egui::Modifiers {
            ctrl: true,
            shift: true,
            command: true,
            ..Default::default()
        }));
        assert!(
            !run(egui::Modifiers {
                ctrl: true,
                command: true,
                ..Default::default()
            }),
            "少按 Shift 不能命中"
        );
        assert!(
            !run(egui::Modifiers {
                ctrl: true,
                shift: true,
                alt: true,
                command: true,
                ..Default::default()
            }),
            "多按 Alt 不能命中"
        );
    }
}
