//! M76-07：虚拟列表、富文本、文本输入与导航栈的运行期接线。
//!
//! 四个内核原本只有单测；这里把它们挂进 `ui` 表的四个命令入口。用命令表而不是
//! 几十个扁平函数，是为了让 API 面（运行期快照门与文档桩）保持可读：
//!
//! ```lua
//! ui.virtual_list{ id = "bag", op = "open", total = 1000, viewport = 400, item_height = 32 }
//! ui.rich_text("<b>金币</b><color=#ffcc00>+12</color>")
//! ui.text_input{ id = "name", op = "open", text = "" }
//! ui.nav_stack{ op = "push", page = "bag" }
//! ```
//!
//! **边界**：本层只接线内核与状态；富文本的**绘制**仍走宿主文本排版（当前按纯文本
//! 绘制），输入焦点也还没有接桌面/Web/微信的宿主事件循环。

use std::collections::BTreeMap;

use mlua::{Lua, Table, Value};
use rustlua_lua_api::Handle;
use rustlua_lua_api::api_error;

use crate::ui_nav_stack::{BackOutcome, NavigationStack};
use crate::ui_rich_text::{RichText, parse_rich_text};
use crate::ui_state::UiState;
use crate::ui_text_input::{InputBackend, InputHost, InputPolicy, TextInputState};
use crate::ui_virtual_list::{SlotPool, VirtualList, VirtualListConfig, VirtualWindow};

/// 一个虚拟列表的绑定状态：窗口计算 + 稳定槽位池。
#[derive(Debug, Clone, PartialEq)]
pub struct VirtualListBinding {
    pub list: VirtualList,
    pub pool: SlotPool,
}

/// 四个内核共享的运行期状态，场景切换时整体清空。
#[derive(Debug, Default)]
pub struct AdvancedState {
    pub virtual_lists: BTreeMap<String, VirtualListBinding>,
    pub inputs: BTreeMap<String, TextInputState>,
    pub navigation: Option<NavigationStack>,
}

impl AdvancedState {
    pub fn clear_for_scene_transition(&mut self) {
        self.virtual_lists.clear();
        self.inputs.clear();
        self.navigation = None;
    }
}

fn command_op(command: &Table) -> mlua::Result<String> {
    command
        .get::<Option<String>>("op")?
        .map(|op| op.trim().to_ascii_lowercase())
        .filter(|op| !op.is_empty())
        .ok_or_else(|| api_error("command table is missing a non-empty `op`"))
}

fn command_id(command: &Table) -> mlua::Result<String> {
    command
        .get::<Option<String>>("id")?
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| api_error("command table is missing a non-empty `id`"))
}

fn window_table(lua: &Lua, window: &VirtualWindow) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("start", window.start as i64)?;
    table.set("end", window.end as i64)?;
    table.set("offset", window.offset)?;
    table.set("total_height", window.total_height)?;
    Ok(table)
}

fn input_state_table(lua: &Lua, input: &TextInputState) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("text", input.text().to_owned())?;
    table.set("cursor", input.cursor() as i64)?;
    table.set("selection", input.selected_text().to_owned())?;
    table.set("focused", input.is_focused())?;
    table.set("composition", input.composition().map(str::to_owned))?;
    Ok(table)
}

fn nav_state_table(lua: &Lua, nav: &NavigationStack) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("depth", nav.depth() as i64)?;
    table.set("current", nav.current_page().map(str::to_owned))?;
    table.set("modal", nav.modal().map(|modal| modal.id.clone()))?;
    table.set("accepts_input", nav.page_accepts_input())?;
    Ok(table)
}

fn back_outcome_table(lua: &Lua, outcome: &BackOutcome) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    match outcome {
        BackOutcome::ClosedModal { id, restore_focus } => {
            table.set("kind", "modal")?;
            table.set("id", id.clone())?;
            table.set("focus", restore_focus.map(|focus| focus as i64))?;
        }
        BackOutcome::PoppedPage { page, focus } => {
            table.set("kind", "page")?;
            table.set("page", page.clone())?;
            table.set("focus", focus.map(|focus| focus as i64))?;
        }
        BackOutcome::AtRoot => table.set("kind", "root")?,
        BackOutcome::Empty => table.set("kind", "empty")?,
    }
    Ok(table)
}

fn color_hex(color: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2])
}

fn rich_text_table(lua: &Lua, rich: &RichText) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("plain", rich.plain.clone())?;
    table.set("lines", rich.line_count() as i64)?;
    let spans = lua.create_table()?;
    for (index, span) in rich.spans.iter().enumerate() {
        let entry = lua.create_table()?;
        entry.set("text", span.text.clone())?;
        entry.set("bold", span.bold)?;
        entry.set("color", span.color.map(color_hex))?;
        entry.set("icon", span.icon.clone())?;
        entry.set("line_break", span.line_break)?;
        spans.set(index + 1, entry)?;
    }
    table.set("spans", spans)?;
    let unsupported = lua.create_table()?;
    for (index, tag) in rich.unsupported.iter().enumerate() {
        unsupported.set(index + 1, tag.clone())?;
    }
    table.set("unsupported", unsupported)?;
    Ok(table)
}

fn host_from_name(name: &str) -> mlua::Result<InputHost> {
    match name.trim().to_ascii_lowercase().as_str() {
        "desktop" => Ok(InputHost::Desktop),
        "web" => Ok(InputHost::Web),
        "wechat" | "weixin" => Ok(InputHost::Wechat),
        other => Err(api_error(format!(
            "ui.text_input policy host must be desktop/web/wechat, got {other:?}"
        ))),
    }
}

fn backend_name(backend: InputBackend) -> &'static str {
    match backend {
        InputBackend::NativeComposition => "native_composition",
        InputBackend::BrowserComposition => "browser_composition",
        InputBackend::WechatNativeBridge => "wechat_native_bridge",
        InputBackend::ReadOnly => "read_only",
    }
}

pub fn install(lua: &Lua, ui: &Table, state: &Handle<UiState>) -> mlua::Result<()> {
    install_virtual_list(lua, ui, state)?;
    install_rich_text(lua, ui)?;
    install_text_input(lua, ui, state)?;
    install_nav_stack(lua, ui, state)?;
    Ok(())
}

fn install_virtual_list(lua: &Lua, ui: &Table, state: &Handle<UiState>) -> mlua::Result<()> {
    let handle = state.clone();
    ui.set(
        "virtual_list",
        lua.create_function(move |lua, command: Table| {
            let op = command_op(&command)?;
            let mut ui = handle.borrow_mut()?;
            let advanced = &mut ui.advanced;
            match op.as_str() {
                "open" => {
                    let id = command_id(&command)?;
                    let total = command.get::<Option<i64>>("total")?.unwrap_or(0).max(0) as usize;
                    let viewport = command.get::<Option<f32>>("viewport")?.unwrap_or(0.0);
                    let item_height = command.get::<Option<f32>>("item_height")?.unwrap_or(0.0);
                    if !item_height.is_finite() || item_height <= 0.0 {
                        return Err(api_error("ui.virtual_list open needs item_height > 0"));
                    }
                    if !viewport.is_finite() || viewport < 0.0 {
                        return Err(api_error("ui.virtual_list open needs viewport >= 0"));
                    }
                    let spacing = command.get::<Option<f32>>("spacing")?.unwrap_or(0.0);
                    if !spacing.is_finite() || spacing < 0.0 {
                        return Err(api_error("ui.virtual_list open needs spacing >= 0"));
                    }
                    let overscan = command.get::<Option<i64>>("overscan")?.unwrap_or(2).clamp(0, 64) as usize;
                    let config = VirtualListConfig {
                        item_height,
                        spacing,
                        overscan,
                    };
                    let list = VirtualList::new(config, total, viewport);
                    let window = list.window();
                    advanced.virtual_lists.insert(
                        id,
                        VirtualListBinding {
                            pool: SlotPool::default(),
                            list,
                        },
                    );
                    Ok(Value::Table(window_table(lua, &window)?))
                }
                "close" => {
                    let id = command_id(&command)?;
                    Ok(Value::Boolean(advanced.virtual_lists.remove(&id).is_some()))
                }
                "scroll" => {
                    let id = command_id(&command)?;
                    let delta = command.get::<Option<f32>>("delta")?.unwrap_or(0.0);
                    let Some(binding) = advanced.virtual_lists.get_mut(&id) else {
                        return Ok(Value::Nil);
                    };
                    Ok(Value::Number(f64::from(binding.list.scroll_by(delta))))
                }
                "scroll_to" => {
                    let id = command_id(&command)?;
                    let index = command.get::<Option<i64>>("index")?.unwrap_or(-1);
                    let Some(binding) = advanced.virtual_lists.get_mut(&id) else {
                        return Ok(Value::Nil);
                    };
                    if index < 0 {
                        return Ok(Value::Boolean(false));
                    }
                    Ok(Value::Boolean(binding.list.scroll_to_index(index as usize)))
                }
                "window" => {
                    let id = command_id(&command)?;
                    let Some(binding) = advanced.virtual_lists.get(&id) else {
                        return Ok(Value::Nil);
                    };
                    Ok(Value::Table(window_table(lua, &binding.list.window())?))
                }
                "bind" => {
                    let id = command_id(&command)?;
                    let keys = command.get::<Option<Vec<String>>>("keys")?;
                    let Some(binding) = advanced.virtual_lists.get_mut(&id) else {
                        return Ok(Value::Nil);
                    };
                    let window = binding.list.window();
                    let indices: Vec<usize> = (window.start..window.end).collect();
                    let keys = match keys {
                        Some(keys) => {
                            if keys.len() != indices.len() {
                                return Err(api_error(format!(
                                    "ui.virtual_list bind keys length {} != visible {}",
                                    keys.len(),
                                    indices.len()
                                )));
                            }
                            keys
                        }
                        None => indices.iter().map(usize::to_string).collect(),
                    };
                    let borrowed: Vec<&str> = keys.iter().map(String::as_str).collect();
                    let slots = binding.pool.bind(&borrowed);
                    let rows = lua.create_table()?;
                    let offset = window.offset;
                    for (position, index) in indices.iter().enumerate() {
                        let rect = binding.list.item_rect(*index);
                        let row = lua.create_table()?;
                        row.set("index", *index as i64)?;
                        row.set("key", keys[position].clone())?;
                        row.set("slot", slots[position] as i64)?;
                        row.set("x", rect[0])?;
                        row.set("y", rect[1] - offset)?;
                        row.set("content_y", rect[1])?;
                        row.set("width", rect[2])?;
                        row.set("height", rect[3])?;
                        rows.set(position + 1, row)?;
                    }
                    Ok(Value::Table(rows))
                }
                other => Err(api_error(format!(
                    "ui.virtual_list unknown op {other:?}（open/close/scroll/scroll_to/window/bind）"
                ))),
            }
        })?,
    )?;
    Ok(())
}

fn install_rich_text(lua: &Lua, ui: &Table) -> mlua::Result<()> {
    ui.set(
        "rich_text",
        lua.create_function(move |lua, text: String| {
            let rich = parse_rich_text(&text);
            rich_text_table(lua, &rich)
        })?,
    )?;
    Ok(())
}

fn install_text_input(lua: &Lua, ui: &Table, state: &Handle<UiState>) -> mlua::Result<()> {
    let handle = state.clone();
    ui.set(
        "text_input",
        lua.create_function(move |lua, command: Table| {
            let op = command_op(&command)?;
            if op == "policy" {
                let host = command.get::<Option<String>>("host")?.unwrap_or_default();
                let bridge = command.get::<Option<bool>>("bridge")?.unwrap_or(false);
                let policy = InputPolicy::for_host(host_from_name(&host)?, bridge);
                let table = lua.create_table()?;
                table.set("backend", backend_name(policy.backend))?;
                table.set("supports_selection", policy.supports_selection)?;
                table.set("note", policy.note)?;
                return Ok(Value::Table(table));
            }
            let id = command_id(&command)?;
            let mut ui = handle.borrow_mut()?;
            let advanced = &mut ui.advanced;
            match op.as_str() {
                "open" => {
                    let text = command.get::<Option<String>>("text")?.unwrap_or_default();
                    let mut input = TextInputState::with_text(text);
                    input.focus();
                    let cursor = input.cursor() as i64;
                    advanced.inputs.insert(id, input);
                    Ok(Value::Integer(cursor))
                }
                "close" => Ok(Value::Boolean(advanced.inputs.remove(&id).is_some())),
                "state" => match advanced.inputs.get(&id) {
                    Some(input) => Ok(Value::Table(input_state_table(lua, input)?)),
                    None => Ok(Value::Nil),
                },
                "edit" => {
                    let action = command
                        .get::<Option<String>>("action")?
                        .map(|action| action.trim().to_ascii_lowercase())
                        .ok_or_else(|| api_error("ui.text_input edit needs action"))?;
                    let value = command.get::<Option<String>>("value")?;
                    let index = command.get::<Option<i64>>("index")?;
                    let Some(input) = advanced.inputs.get_mut(&id) else {
                        return Ok(Value::Nil);
                    };
                    match action.as_str() {
                        "insert" => input.insert_str(value.as_deref().unwrap_or_default()),
                        "backspace" => input.backspace(),
                        "delete" => input.delete_forward(),
                        "left" => input.move_left(),
                        "right" => input.move_right(),
                        "home" => input.home(),
                        "end" => input.end(),
                        "select_all" => input.select_all(),
                        "set_cursor" => {
                            let cursor = index.unwrap_or(0);
                            if cursor < 0 {
                                return Err(api_error("ui.text_input set_cursor needs index >= 0"));
                            }
                            input.set_cursor(cursor as usize);
                        }
                        "focus" => input.focus(),
                        "blur" => input.blur(),
                        other => {
                            return Err(api_error(format!(
                                "ui.text_input unknown edit action {other:?}"
                            )));
                        }
                    }
                    Ok(Value::Table(input_state_table(lua, input)?))
                }
                "composition" => {
                    let action = command
                        .get::<Option<String>>("action")?
                        .map(|action| action.trim().to_ascii_lowercase())
                        .ok_or_else(|| api_error("ui.text_input composition needs action"))?;
                    let value = command.get::<Option<String>>("value")?;
                    let Some(input) = advanced.inputs.get_mut(&id) else {
                        return Ok(Value::Nil);
                    };
                    let committed = match action.as_str() {
                        "begin" => {
                            input.begin_composition(value.as_deref().unwrap_or_default());
                            None
                        }
                        "update" => {
                            input.update_composition(value.as_deref().unwrap_or_default());
                            None
                        }
                        "commit" => input.commit_composition(),
                        "cancel" => {
                            input.cancel_composition();
                            None
                        }
                        other => {
                            return Err(api_error(format!(
                                "ui.text_input unknown composition action {other:?}"
                            )));
                        }
                    };
                    let table = input_state_table(lua, input)?;
                    table.set("committed", committed)?;
                    Ok(Value::Table(table))
                }
                other => Err(api_error(format!(
                    "ui.text_input unknown op {other:?}（open/close/state/edit/composition/policy）"
                ))),
            }
        })?,
    )?;
    Ok(())
}

fn install_nav_stack(lua: &Lua, ui: &Table, state: &Handle<UiState>) -> mlua::Result<()> {
    let handle = state.clone();
    ui.set(
        "nav_stack",
        lua.create_function(move |lua, command: Table| {
            let op = command_op(&command)?;
            let mut ui = handle.borrow_mut()?;
            let advanced = &mut ui.advanced;
            match op.as_str() {
                "open" => {
                    let root = command
                        .get::<Option<String>>("root")?
                        .filter(|root| !root.trim().is_empty())
                        .ok_or_else(|| api_error("ui.nav_stack open needs a non-empty root"))?;
                    let nav = NavigationStack::new(root);
                    let depth = nav.depth() as i64;
                    advanced.navigation = Some(nav);
                    Ok(Value::Integer(depth))
                }
                "state" => match advanced.navigation.as_ref() {
                    Some(nav) => Ok(Value::Table(nav_state_table(lua, nav)?)),
                    None => Ok(Value::Nil),
                },
                "push" => {
                    let page = command
                        .get::<Option<String>>("page")?
                        .filter(|page| !page.trim().is_empty())
                        .ok_or_else(|| api_error("ui.nav_stack push needs a non-empty page"))?;
                    let restore = command.get::<Option<i64>>("restore_focus")?;
                    let Some(nav) = advanced.navigation.as_mut() else {
                        return Ok(Value::Nil);
                    };
                    nav.push(page, restore.map(|focus| focus.max(0) as u64));
                    Ok(Value::Table(nav_state_table(lua, nav)?))
                }
                "modal" => {
                    let id = command_id(&command)?;
                    let restore = command.get::<Option<i64>>("restore_focus")?;
                    let Some(nav) = advanced.navigation.as_mut() else {
                        return Ok(Value::Nil);
                    };
                    nav.push_modal(id, restore.map(|focus| focus.max(0) as u64));
                    Ok(Value::Table(nav_state_table(lua, nav)?))
                }
                "back" => {
                    let Some(nav) = advanced.navigation.as_mut() else {
                        return Ok(Value::Nil);
                    };
                    let outcome = nav.back();
                    Ok(Value::Table(back_outcome_table(lua, &outcome)?))
                }
                "close_modal" => {
                    let id = command_id(&command)?;
                    let Some(nav) = advanced.navigation.as_mut() else {
                        return Ok(Value::Nil);
                    };
                    match nav.close_modal(&id) {
                        Ok(focus) => Ok(Value::Integer(focus.map_or(-1, |focus| focus as i64))),
                        Err(message) => Err(api_error(message)),
                    }
                }
                other => Err(api_error(format!(
                    "ui.nav_stack unknown op {other:?}（open/state/push/modal/back/close_modal）"
                ))),
            }
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua;

    fn installed() -> (Lua, Table, Handle<UiState>) {
        let lua = Lua::new();
        let ui = lua.create_table().unwrap();
        let state = UiState::handle();
        install(&lua, &ui, &state).unwrap();
        lua.globals().set("ui", &ui).unwrap();
        (lua, ui, state)
    }

    #[test]
    fn virtual_list_binds_a_stable_window_and_reuses_slots() {
        let (lua, _ui, state) = installed();
        lua.load(
            r#"
            local w = ui.virtual_list{ id = "bag", op = "open", total = 1000, viewport = 200, item_height = 20 }
            assert(w.start == 0 and w["end"] > 10, "window covers the viewport")
            local rows = ui.virtual_list{ id = "bag", op = "bind" }
            assert(#rows > 10 and #rows < 30, "only the window is bound")
            assert(rows[1].slot == 0 and rows[1].y == 0.0, "first row uses slot 0")
            local first_key = rows[1].key
            ui.virtual_list{ id = "bag", op = "scroll", delta = 200.0 }
            local moved = ui.virtual_list{ id = "bag", op = "bind" }
            assert(moved[1].key ~= first_key, "scrolling changes visible keys")
            local pool = 0
            for _, row in ipairs(moved) do pool = math.max(pool, row.slot + 1) end
            assert(pool <= #moved + 2, "slot pool follows the window, not 1000 items")
            "#,
        )
        .exec()
        .unwrap();
        let ui = state.borrow().unwrap();
        let binding = &ui.advanced.virtual_lists["bag"];
        assert_eq!(binding.list.total(), 1000);
        assert!(binding.pool.bound() <= 30, "pool stays window-sized");
    }

    #[test]
    fn rich_text_exposes_spans_and_keeps_unknown_tags_literal() {
        let (lua, _ui, _state) = installed();
        let value: Table = lua
            .load(
                r#"return ui.rich_text("<b>金币</b> <color=#ffcc00>+12</color><br/><icon=coin><script>x")"#,
            )
            .eval()
            .unwrap();
        assert_eq!(value.get::<String>("plain").unwrap(), "金币 +12\n<script>x");
        assert_eq!(value.get::<i64>("lines").unwrap(), 2);
        let spans: Table = value.get("spans").unwrap();
        assert!(spans.raw_len() >= 3);
        let first: Table = spans.get(1).unwrap();
        assert!(first.get::<bool>("bold").unwrap());
        let colored = (1..=spans.raw_len())
            .map(|index| spans.get::<Table>(index).unwrap())
            .find(|span| span.get::<String>("text").unwrap() == "+12")
            .expect("+12 span");
        assert_eq!(
            colored.get::<Option<String>>("color").unwrap().as_deref(),
            Some("#ffcc00")
        );
        let unsupported: Table = value.get("unsupported").unwrap();
        assert_eq!(unsupported.get::<String>(1).unwrap(), "script");
    }

    #[test]
    fn text_input_edits_in_characters_and_honours_host_policy() {
        let (lua, _ui, state) = installed();
        lua.load(
            r#"
            assert(ui.text_input{ id = "name", op = "open", text = "角色" } == 2)
            local s = ui.text_input{ id = "name", op = "edit", action = "insert", value = "名" }
            assert(s.text == "角色名" and s.cursor == 3, "中文按字符插入")
            s = ui.text_input{ id = "name", op = "edit", action = "backspace" }
            assert(s.text == "角色" and s.cursor == 2, "退格删一个字符而不是一个字节")
            s = ui.text_input{ id = "name", op = "composition", action = "begin", value = "ni" }
            assert(s.composition == "ni" and s.text == "角色", "预编辑不进正文")
            s = ui.text_input{ id = "name", op = "composition", action = "commit" }
            assert(s.committed == "ni" and s.text == "角色ni", "提交才落正文")
            local wechat = ui.text_input{ op = "policy", host = "wechat", bridge = false }
            assert(wechat.backend == "read_only", "无桥必须降级只读")
            local desktop = ui.text_input{ op = "policy", host = "desktop" }
            assert(desktop.supports_selection, "桌面完整支持选区")
            "#,
        )
        .exec()
        .unwrap();
        assert!(state.borrow().unwrap().advanced.inputs.contains_key("name"));
    }

    #[test]
    fn nav_stack_closes_modal_before_popping_and_restores_focus() {
        let (lua, _ui, _state) = installed();
        lua.load(
            r#"
            assert(ui.nav_stack{ op = "open", root = "title" } == 1)
            ui.nav_stack{ op = "push", page = "bag", restore_focus = 7 }
            ui.nav_stack{ op = "modal", id = "confirm", restore_focus = 9 }
            local state = ui.nav_stack{ op = "state" }
            assert(state.depth == 2 and state.modal == "confirm", "模态不增加页深")
            assert(not state.accepts_input, "模态存在时底层页面不收输入")
            local out = ui.nav_stack{ op = "back" }
            assert(out.kind == "modal" and out.id == "confirm" and out.focus == 9)
            out = ui.nav_stack{ op = "back" }
            assert(out.kind == "page" and out.page == "bag" and out.focus == 7)
            out = ui.nav_stack{ op = "back" }
            assert(out.kind == "root", "根页返回不动栈")
            "#,
        )
        .exec()
        .unwrap();
    }

    #[test]
    fn unknown_ops_and_missing_lists_surface_as_nil_or_errors() {
        let (lua, _ui, _state) = installed();
        // 读操作找不到 id → nil，不崩。
        let value: Value = lua
            .load(r#"return ui.virtual_list{ id = "nope", op = "window" }"#)
            .eval()
            .unwrap();
        assert!(matches!(value, Value::Nil));
        // 未知 op → 带可用清单的报错。
        let error = lua
            .load(r#"ui.virtual_list{ id = "x", op = "explode" }"#)
            .exec()
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("explode") && error.contains("open"),
            "{error}"
        );
    }
}
