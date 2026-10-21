//! `ui` 的基础图元：文本、条、可见性与取值（`text` / `text_at` / `bar` / `show` / `hide` /
//! `scene_text_visible` / `get` / `text_get` / `size`）——P1 ⑤-12 从脚本层搬来。
//!
//! `ui` 表仍由脚本层的 `api::register_all` 统一 `globals().set("ui", …)`：本函数与
//! `ui_panel_lua::install`、`rustlua_ui::lua_nav::install` 拿的是**同一个表句柄**。
//! 唯一留在脚本层的是 `ui.measure_text`：它要读工程字体探针并调 `rustlua_render::text`，
//! 为了一个度量函数让 UI 域依赖整个渲染 crate（含 wgpu）不划算。

use crate::ui_state::{UiBar, UiState, UiText};
use mlua::Lua;
use mlua::Result as LuaResult;
use mlua::Table;
use rustlua_lua_api::Handle;
use std::cell::{Ref, RefMut};

/// 绑定侧的借用面：一个 `Handle<UiState>` + 与脚本层同名的 `ui()` / `ui_mut()` 短写。
#[derive(Clone)]
struct Bindings {
    ui: Handle<UiState>,
}

impl Bindings {
    fn new(ui: &Handle<UiState>) -> Self {
        Self { ui: ui.clone() }
    }

    fn ui(&self) -> LuaResult<Ref<'_, UiState>> {
        self.ui.borrow()
    }

    fn ui_mut(&self) -> LuaResult<RefMut<'_, UiState>> {
        self.ui.borrow_mut()
    }
}

pub fn install(lua: &Lua, table: Table, state: &Handle<UiState>) -> LuaResult<()> {
    let ui = table;

    let s = Bindings::new(state);
    ui.set(
        "text",
        lua.create_function(move |_, (id, text): (String, String)| {
            s.ui_mut()?.texts.insert(
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
    let s = Bindings::new(state);
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
                s.ui_mut()?.texts.insert(
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
    let s = Bindings::new(state);
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
                s.ui_mut()?.bars.insert(
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
    let s = Bindings::new(state);
    ui.set(
        "show",
        lua.create_function(move |_, id: String| {
            let mut ui = s.ui_mut()?;
            ui.texts
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
    let s = Bindings::new(state);
    ui.set(
        "hide",
        lua.create_function(move |_, id: String| {
            let mut ui = s.ui_mut()?;
            ui.texts
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
            if let Some(panel) = ui.panels.get_mut(&id) {
                panel.visible = false;
            }
            Ok(())
        })?,
    )?;
    let s = Bindings::new(state);
    ui.set(
        "scene_text_visible",
        // 场景文字实体按名显隐：`visible == false` 隐藏，其余（true / 省略）显示。
        // 语义与 web 兼容层 `__rustlua_scene_text_hidden` 对齐（title_menu 隐藏场景层
        // 按钮标签、改由脚本统一绘制，避免两层文字重叠）。
        lua.create_function(move |_, (name, visible): (String, Option<bool>)| {
            let mut ui = s.ui_mut()?;
            if visible == Some(false) {
                ui.scene_text_hidden.insert(name);
            } else {
                ui.scene_text_hidden.remove(&name);
            }
            Ok(())
        })?,
    )?;
    let s = Bindings::new(state);
    ui.set(
        "get",
        lua.create_function(move |lua, id: String| {
            let ui = s.ui()?;
            let Some(value) = ui.texts.get(&id) else {
                return Ok(None);
            };
            let result = lua.create_table()?;
            result.set("text", value.text.clone())?;
            result.set("visible", value.visible)?;
            Ok(Some(result))
        })?,
    )?;
    let s = Bindings::new(state);
    ui.set(
        "text_get",
        // 读回 `ui.text` / `ui.text_at` 建的动态文本（内容 + 位置 + 字号 + 可见性）；
        // 不存在返回 nil。用途（M30）：headless 里拿到"画面上真正画出来的那串字"，
        // 再用 `ui.measure_text` 量它是否超出容器——排版检查不能只查源码字面量。
        lua.create_function(move |lua, id: String| {
            let ui = s.ui()?;
            let Some(text) = ui.texts.get(&id) else {
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
                    .unwrap_or(text.scale * rustlua_math::text_metrics::TEXT_BASE_PX),
            )?;
            result.set("visible", text.visible)?;
            Ok(Some(result))
        })?,
    )?;
    Ok(())
}
