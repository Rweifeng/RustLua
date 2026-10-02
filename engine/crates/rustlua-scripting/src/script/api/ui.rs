//! `ui` 的基础图元：文本、条、可见性与取值（`text` / `bar` / `show` / `get` …）。
//!
//! Lua 侧的 `ui` 表分三段装配（本文件 + `ui_panel` + `ui_nav`），三段拿到的是同一个
//! 表句柄；`globals().set("ui", …)` 由 `api::register_all` 在末尾统一执行。

// WP11-USE:BEGIN
use crate::script::LuaStateBorrow;
use crate::script::ScriptState;
use crate::script::UiBar;
use crate::script::UiText;
use crate::script::error::api_error;
use mlua::Result as LuaResult;
use mlua::{Lua, Table};
use std::cell::RefCell;
use std::rc::Rc;
// WP11-USE:END

pub(super) fn register(lua: &Lua, ui: Table, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    let s = Rc::clone(&state);
    ui.set(
        "text",
        lua.create_function(move |_, (id, text): (String, String)| {
            s.borrow_mut().ui_state.texts.insert(
                id,
                UiText {
                    text,
                    visible: true,
                    position: [10.0, 10.0],
                    scale: 1.0,
                    font_size: None,
                },
            );
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "text_at",
        lua.create_function(
            move |_,
                  (id, x, y, text, scale, font_size): (
                String,
                f32,
                f32,
                String,
                Option<f32>,
                Option<f32>,
            )| {
                s.borrow_mut().ui_state.texts.insert(
                    id,
                    UiText {
                        text,
                        visible: true,
                        position: [x, y],
                        scale: scale.unwrap_or(1.0),
                        font_size,
                    },
                );
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "bar",
        lua.create_function(
            move |_,
                  (id, x, y, width, height, ratio, r, g, b): (
                String,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
                f32,
            )| {
                s.borrow_mut().ui_state.bars.insert(
                    id,
                    UiBar {
                        position: [x, y],
                        size: [width, height],
                        ratio: ratio.clamp(0.0, 1.0),
                        color: [r, g, b, 1.0],
                    },
                );
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "show",
        lua.create_function(move |_, id: String| {
            s.borrow_mut()
                .ui_state
                .texts
                .entry(id)
                .or_insert(UiText {
                    text: String::new(),
                    visible: false,
                    position: [10.0, 10.0],
                    scale: 1.0,
                    font_size: None,
                })
                .visible = true;
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "hide",
        lua.create_function(move |_, id: String| {
            let mut state = s.lua_borrow_mut()?;
            state
                .ui_state
                .texts
                .entry(id.clone())
                .or_insert(UiText {
                    text: String::new(),
                    visible: false,
                    position: [10.0, 10.0],
                    scale: 1.0,
                    font_size: None,
                })
                .visible = false;
            // M22：`hide` 也作用于**同 id 的动态面板**（`ui.panel` 建的矩形）。
            // 之前只认文本，于是「先 clear 再重画」的面板类 UI 永远隐藏不掉，
            // 会留下上一次选中项的残留高亮框（标题菜单的实测缺陷）。
            // 只改已存在的面板，不凭空创建（保持 `hide` 对未知 id 的宽松语义）。
            if let Some(panel) = state.ui_state.panels.get_mut(&id) {
                panel.visible = false;
            }
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "scene_text_visible",
        // 场景文字实体按名显隐：`visible == false` 隐藏，其余（true / 省略）显示。
        // 语义与 web 兼容层 `__rustlua_scene_text_hidden` 对齐（title_menu 隐藏场景层
        // 按钮标签、改由脚本统一绘制，避免两层文字重叠）。
        lua.create_function(move |_, (name, visible): (String, Option<bool>)| {
            let mut state = s.lua_borrow_mut()?;
            if visible == Some(false) {
                state.ui_state.scene_text_hidden.insert(name);
            } else {
                state.ui_state.scene_text_hidden.remove(&name);
            }
            Ok(())
        })?,
    )?;
    lua.globals().set("ui", &ui)?;
    let s = Rc::clone(&state);
    ui.set(
        "get",
        lua.create_function(move |lua, id: String| {
            let state = s.lua_borrow()?;
            let Some(value) = state.ui_state.texts.get(&id) else {
                return Ok(None);
            };
            let result = lua.create_table()?;
            result.set("text", value.text.clone())?;
            result.set("visible", value.visible)?;
            Ok(Some(result))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "text_get",
        // 读回 `ui.text` / `ui.text_at` 建的动态文本（内容 + 位置 + 字号 + 可见性）；
        // 不存在返回 nil。用途（M30）：headless 里拿到"画面上真正画出来的那串字"，
        // 再用 `ui.measure_text` 量它是否超出容器——排版检查不能只查源码字面量。
        lua.create_function(move |lua, id: String| {
            let state = s.lua_borrow()?;
            let Some(text) = state.ui_state.texts.get(&id) else {
                return Ok(None);
            };
            let result = lua.create_table()?;
            result.set("text", text.text.clone())?;
            result.set("x", text.position[0])?;
            result.set("y", text.position[1])?;
            result.set("scale", text.scale)?;
            // 未显式给 font_size 时，渲染口径是 `scale × EM_PX`（`ui.text_effective_scale`）。
            result.set(
                "size",
                text.font_size
                    .unwrap_or(text.scale * rustlua_render::text::EM_PX),
            )?;
            result.set("visible", text.visible)?;
            Ok(Some(result))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "measure_text",
        // 与渲染**同口径**的文本度量（M30）：只读字形覆盖与 advance，不光栅化字形。
        // 返回 `{width, height, lines, glyphs, missing}`；`missing` 是字体（含后备字体）
        // 没有字形的字符拼接串（无缺字为空串）。
        // 为什么需要：中文排版不能按"字数 × 系数"估宽（CJK 满宽、ASCII 半宽、
        // 全角标点又不同），也不能用 `string.len` 数字节；之前 HUD 用的是
        // "CJK 记 2、其余记 1"的启发式裁剪，遇到宽拉丁字母/长数字仍会溢出。
        lua.create_function(move |lua, (text, size): (String, Option<f32>)| {
            let size_px = size.unwrap_or(rustlua_render::text::EM_PX);
            if !size_px.is_finite() || size_px <= 0.0 {
                return Err(api_error("ui.measure_text size must be positive"));
            }
            let probe = {
                let mut state = s.lua_borrow_mut()?;
                state
                    .font_probe()
                    .map_err(|error| api_error(error.to_string()))?
            };
            let Some(probe) = probe else {
                return Err(api_error(
                    "ui.measure_text: project font unavailable (check project.toml `font`)",
                ));
            };
            let measured = rustlua_render::text::measure_text(
                &probe,
                &text,
                size_px,
                rustlua_render::text::TextStyle::default().spacing,
            );
            let result = lua.create_table()?;
            result.set("width", measured.width)?;
            result.set("height", measured.height)?;
            result.set("lines", measured.lines)?;
            result.set("glyphs", measured.glyphs)?;
            result.set("missing", measured.missing_text())?;
            result.set("missing_count", measured.missing.len())?;
            Ok(result)
        })?,
    )?;
    Ok(())
}
