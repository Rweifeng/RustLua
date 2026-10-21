//! `ui.measure_text`：与渲染**同口径**的文本度量（P1 ⑤-12 后 `ui` 表里唯一留在脚本层的函数）。
//!
//! 为什么它没跟着搬去 `rustlua-ui`：这一支要读工程字体探针（`project.toml` 的 `font`）并调用
//! `rustlua_render::text::measure_text`。为了让 UI 域拿到它，就得让 `rustlua-ui` 依赖整个渲染
//! crate（含 wgpu）——一个度量函数不值得把重量级依赖塞进 UI 域。`ui` 的其余九个函数（文本、
//! 条、可见性、取值、尺寸）都已搬去 `rustlua_ui::ui_lua` / `ui_panel_lua`，本文件只往那张已经
//! 建好的表上补一个键。

use crate::script::LuaStateBorrow;
use crate::script::ScriptState;
use crate::script::error::api_error;
use mlua::Result as LuaResult;
use mlua::{Lua, Table};
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn register(lua: &Lua, ui: Table, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
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
