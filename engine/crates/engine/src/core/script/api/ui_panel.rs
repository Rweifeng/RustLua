//! `ui` 的面板与栅格：`panel` / `circle` / `image` / `grid` / `tooltip` / `modal` / `drag`。
//!
//! 九宫格边距解析（`parse_nine_slice`）只有 `image` 用，故随本段自持。

// WP11-USE:BEGIN
use crate::core::script::LuaStateBorrow;
use crate::core::script::error::api_error;
use crate::core::script::values::{finite_color4, finite_ui_rect};
use crate::core::script::{ScriptState, UiDrag, UiText};
use crate::core::ui::{DynamicUiImage, UiGrid, UiGridCell, UiPanel};
use mlua::Result as LuaResult;
use mlua::{Lua, Table, Value};
use std::cell::RefCell;
use std::rc::Rc;
// WP11-USE:END

pub(super) fn register(lua: &Lua, ui: Table, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    /// 解析 "left,right,top,bottom" 九宫格边距（像素，单位是作者空间像素）；空/非法返回 None。
    fn parse_nine_slice(value: &str) -> Option<[f32; 4]> {
        let parts: Vec<&str> = value.split(',').map(str::trim).collect();
        if parts.len() != 4 {
            return None;
        }
        let mut out = [0.0_f32; 4];
        for (i, part) in parts.iter().enumerate() {
            let v: f32 = part.parse().ok()?;
            if !v.is_finite() || v < 0.0 {
                return None;
            }
            out[i] = v;
        }
        if out.iter().all(|v| *v <= 0.0) {
            return None;
        }
        Some(out)
    }

    let s = Rc::clone(&state);
    ui.set(
        "panel_get",
        // M22：读回 `ui.panel` 建的动态面板（几何 + 颜色 + 可见性）；不存在返回 nil。
        // 用途：UI 动画/可见性的 headless 断言（例如「选中高亮确实画出来了且在弹跳」），
        // 以及运行期调试面板。只读，不改状态。
        lua.create_function(move |lua, id: String| {
            let state = s.lua_borrow()?;
            let Some(panel) = state.ui_state.panels.get(&id) else {
                return Ok(None);
            };
            let result = lua.create_table()?;
            result.set("x", panel.position[0])?;
            result.set("y", panel.position[1])?;
            result.set("width", panel.size[0])?;
            result.set("height", panel.size[1])?;
            result.set("r", panel.color[0])?;
            result.set("g", panel.color[1])?;
            result.set("b", panel.color[2])?;
            result.set("a", panel.color[3])?;
            result.set("visible", panel.visible)?;
            Ok(Some(result))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "panel",
        lua.create_function(
            move |_,
                  (id, x, y, width, height, r, g, b, a): (
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
                if !finite_ui_rect(x, y, width, height) {
                    return Err(api_error("ui.panel rect must be finite and positive"));
                }
                let color = [r, g, b, a];
                if !finite_color4(color) {
                    return Err(api_error("ui.panel color must be finite"));
                }
                s.borrow_mut().ui_state.panels.insert(
                    id,
                    UiPanel {
                        position: [x, y],
                        size: [width, height],
                        color,
                        visible: true,
                        space: crate::core::ui::UiLayoutSpace::Screen,
                        anchor: [0.0, 0.0],
                        shape: 0,
                    },
                );
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "circle",
        lua.create_function(
            move |_,
                  (id, cx, cy, radius, r, g, b, a): (
                String, f32, f32, f32, f32, f32, f32, f32,
            )| {
                let color = [r, g, b, a];
                s.borrow_mut().ui_state.panels.insert(
                    id,
                    UiPanel {
                        position: [cx - radius, cy - radius],
                        size: [radius * 2.0, radius * 2.0],
                        color,
                        visible: true,
                        space: crate::core::ui::UiLayoutSpace::Screen,
                        anchor: [0.0, 0.0],
                        shape: 1,
                    },
                );
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "image",
        lua.create_function(
            move |_,
                  (id, x, y, width, height, texture, r, g, b, slice): (
                String,
                f32,
                f32,
                f32,
                f32,
                String,
                f32,
                f32,
                f32,
                Option<String>,
            )| {
                if !finite_ui_rect(x, y, width, height) {
                    return Err(api_error("ui.image rect must be finite and positive"));
                }
                // docs/48 ui 换肤：可选第 10 参 `slice`（"left,right,top,bottom" 像素边距）
                // → 九宫格渲染（DynamicUiImage.nine_slice）；空/非法回退普通四边形（向后兼容）。
                let nine_slice = slice.as_deref().and_then(parse_nine_slice);
                s.borrow_mut().ui_state.images.insert(
                    id,
                    DynamicUiImage {
                        texture,
                        position: [x, y],
                        size: [width, height],
                        color: [r, g, b],
                        space: crate::core::ui::UiLayoutSpace::Screen,
                        anchor: [0.0, 0.0],
                        nine_slice,
                    },
                );
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "image_get",
        // M29：读回 `ui.image` 建的动态图片（几何 + 贴图 + 颜色）；不存在返回 nil。
        // 与 `panel_get` 同一用途：让"确实画出来了"成为可 headless 断言的事实
        // （第一个真实消费者是雨天的天气图标）。
        lua.create_function(move |lua, id: String| {
            let state = s.lua_borrow()?;
            let Some(image) = state.ui_state.images.get(&id) else {
                return Ok(None);
            };
            let result = lua.create_table()?;
            result.set("x", image.position[0])?;
            result.set("y", image.position[1])?;
            result.set("width", image.size[0])?;
            result.set("height", image.size[1])?;
            result.set("texture", image.texture.clone())?;
            result.set("r", image.color[0])?;
            result.set("g", image.color[1])?;
            result.set("b", image.color[2])?;
            result.set(
                "nine_slice",
                image
                    .nine_slice
                    .map(|s| format!("{},{},{},{}", s[0], s[1], s[2], s[3])),
            )?;
            Ok(Some(result))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "grid",
        lua.create_function(
            move |_,
                  (id, x, y, columns, rows, cell_size, gap): (
                String,
                f32,
                f32,
                i64,
                i64,
                f32,
                f32,
            )| {
                if columns <= 0 || rows <= 0 {
                    return Err(api_error("ui.grid columns/rows must be positive"));
                }
                if !finite_ui_rect(x, y, cell_size, cell_size) || !gap.is_finite() || gap < 0.0 {
                    return Err(api_error("ui.grid rect/gap must be finite and valid"));
                }
                let columns = columns as usize;
                let rows = rows as usize;
                let cell_count = columns * rows;
                s.borrow_mut().ui_state.grids.insert(
                    id,
                    UiGrid {
                        position: [x, y],
                        columns,
                        rows,
                        cell_size: [cell_size, cell_size],
                        gap,
                        background: [0.10, 0.10, 0.13, 0.92],
                        cells: vec![
                            UiGridCell {
                                texture: None,
                                color: [0.22, 0.23, 0.27, 0.95],
                            };
                            cell_count
                        ],
                        space: crate::core::ui::UiLayoutSpace::Screen,
                        anchor: [0.0, 0.0],
                    },
                );
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "grid_cell",
        lua.create_function(
            move |_,
                  (id, index, texture, r, g, b, a): (
                String,
                i64,
                Option<String>,
                f32,
                f32,
                f32,
                f32,
            )| {
                let mut state = s.lua_borrow_mut()?;
                let Some(grid) = state.ui_state.grids.get_mut(&id) else {
                    return Err(api_error(format!("ui.grid {id} not found")));
                };
                if index <= 0 || (index as usize) > grid.cells.len() {
                    return Err(api_error(format!(
                        "ui.grid_cell index {index} out of bounds for {id}"
                    )));
                }
                let color = [r, g, b, a];
                if !finite_color4(color) {
                    return Err(api_error("ui.grid_cell color must be finite"));
                }
                grid.cells[(index - 1) as usize] = UiGridCell { texture, color };
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "grid_clear",
        lua.create_function(move |_, id: String| {
            let mut state = s.lua_borrow_mut()?;
            let Some(grid) = state.ui_state.grids.get_mut(&id) else {
                return Err(api_error(format!("ui.grid {id} not found")));
            };
            grid.cells.fill(UiGridCell {
                texture: None,
                color: [0.22, 0.23, 0.27, 0.95],
            });
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "grid_hit",
        lua.create_function(move |_, (id, x, y): (String, f32, f32)| {
            let state = s.lua_borrow()?;
            let Some(grid) = state.ui_state.grids.get(&id) else {
                return Err(api_error(format!("ui.grid {id} not found")));
            };
            for index in 1..=grid.columns * grid.rows {
                let Some((origin, size)) =
                    grid.cell_rect(index, state.ui_state.viewport, state.ui_state.reference)
                else {
                    continue;
                };
                if x >= origin[0]
                    && x <= origin[0] + size[0]
                    && y >= origin[1]
                    && y <= origin[1] + size[1]
                {
                    return Ok(Some(index as i64));
                }
            }
            Ok(None)
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "tooltip",
        lua.create_function(
            move |_, (id, x, y, text, width, height): (String, f32, f32, String, f32, f32)| {
                if !finite_ui_rect(x, y, width, height) {
                    return Err(api_error("ui.tooltip rect must be finite and positive"));
                }
                let mut state = s.lua_borrow_mut()?;
                state.ui_state.panels.insert(
                    id.clone(),
                    UiPanel {
                        position: [x, y],
                        size: [width, height],
                        color: [0.08, 0.09, 0.12, 0.94],
                        visible: true,
                        space: crate::core::ui::UiLayoutSpace::Screen,
                        anchor: [0.0, 0.0],
                        shape: 0,
                    },
                );
                state.ui_state.texts.insert(
                    format!("{id}:tooltip"),
                    UiText {
                        text,
                        visible: true,
                        position: [x + 8.0, y + 4.0],
                        scale: 1.0,
                        font_size: Some(14.0),
                    },
                );
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "tooltip_hide",
        lua.create_function(move |_, id: String| {
            let mut state = s.lua_borrow_mut()?;
            if let Some(panel) = state.ui_state.panels.get_mut(&id) {
                panel.visible = false;
            }
            if let Some(text) = state.ui_state.texts.get_mut(&format!("{id}:tooltip")) {
                text.visible = false;
            }
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "modal",
        lua.create_function(
            move |_, (id, x, y, width, height, title): (String, f32, f32, f32, f32, String)| {
                if !finite_ui_rect(x, y, width, height) {
                    return Err(api_error("ui.modal rect must be finite and positive"));
                }
                let mut state = s.lua_borrow_mut()?;
                state.ui_state.panels.insert(
                    id.clone(),
                    UiPanel {
                        position: [x, y],
                        size: [width, height],
                        color: [0.12, 0.13, 0.17, 0.96],
                        visible: true,
                        space: crate::core::ui::UiLayoutSpace::Screen,
                        anchor: [0.0, 0.0],
                        shape: 0,
                    },
                );
                state.ui_state.texts.insert(
                    format!("{id}:title"),
                    UiText {
                        text: title,
                        visible: true,
                        position: [x + 12.0, y + 8.0],
                        scale: 1.0,
                        font_size: Some(20.0),
                    },
                );
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "modal_show",
        lua.create_function(move |_, id: String| {
            let mut state = s.lua_borrow_mut()?;
            if let Some(panel) = state.ui_state.panels.get_mut(&id) {
                panel.visible = true;
            }
            if let Some(text) = state.ui_state.texts.get_mut(&format!("{id}:title")) {
                text.visible = true;
            }
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "modal_hide",
        lua.create_function(move |_, id: String| {
            let mut state = s.lua_borrow_mut()?;
            if let Some(panel) = state.ui_state.panels.get_mut(&id) {
                panel.visible = false;
            }
            if let Some(text) = state.ui_state.texts.get_mut(&format!("{id}:title")) {
                text.visible = false;
            }
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "clear",
        lua.create_function(move |_, id: String| {
            let mut state = s.lua_borrow_mut()?;
            state.ui_state.panels.remove(&id);
            state.ui_state.images.remove(&id);
            state.ui_state.grids.remove(&id);
            state.ui_state.texts.remove(&format!("{id}:tooltip"));
            state.ui_state.texts.remove(&format!("{id}:title"));
            Ok(())
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "drag_begin",
        lua.create_function(move |_, (id, index): (String, i64)| {
            if index <= 0 {
                return Err(api_error("ui.drag_begin index must be positive"));
            }
            let mut state = s.lua_borrow_mut()?;
            if !state.ui_state.grids.contains_key(&id) {
                return Err(api_error(format!("ui.grid {id} not found")));
            }
            state.ui_state.drag = Some(UiDrag {
                from_id: id,
                from_index: index as usize,
            });
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "drag_end",
        lua.create_function(move |lua, (id, index): (String, i64)| {
            if index <= 0 {
                return Err(api_error("ui.drag_end index must be positive"));
            }
            let drag = s.borrow().ui_state.drag.clone();
            let Some(drag) = drag else {
                return Ok(false);
            };
            s.borrow_mut().ui_state.drag = None;
            let callback: Value = lua.globals().get("on_drag")?;
            if let Value::Function(function) = callback {
                function.call::<()>((drag.from_id, drag.from_index as i64, id, index))?;
            }
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "drag_active",
        lua.create_function(move |lua, ()| {
            let state = s.lua_borrow()?;
            let Some(drag) = &state.ui_state.drag else {
                return Ok(Value::Nil);
            };
            let result = lua.create_table()?;
            result.set("id", drag.from_id.clone())?;
            result.set("index", drag.from_index as i64)?;
            Ok(Value::Table(result))
        })?,
    )?;
    let s = Rc::clone(&state);
    ui.set(
        "drag_cancel",
        lua.create_function(move |_, ()| {
            s.borrow_mut().ui_state.drag = None;
            Ok(())
        })?,
    )?;
    Ok(())
}
