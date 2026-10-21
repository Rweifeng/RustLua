# Lua 模块 `ui`

> 运行期 API 面基线里本模块有 45 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

## `ui.bar(id: String, x: f32, y: f32, width: f32, height: f32, ratio: f32, r: f32, g: f32, b: f32)`

> 绑定：`engine/crates/rustlua-ui/src/ui_lua.rs:84`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.circle(id: String, cx: f32, cy: f32, radius: f32, r: f32, g: f32, b: f32, a: f32)`

> 绑定：`engine/crates/rustlua-ui/src/ui_panel_lua.rs:137`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.clear(id: String)`

> 绑定：`engine/crates/rustlua-ui/src/ui_panel_lua.rs:462`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.drag_active()`

> 绑定：`engine/crates/rustlua-ui/src/ui_panel_lua.rs:512`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.drag_begin(id: String, index: i64)`

> 绑定：`engine/crates/rustlua-ui/src/ui_panel_lua.rs:475`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.drag_cancel()`

> 绑定：`engine/crates/rustlua-ui/src/ui_panel_lua.rs:526`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.drag_end(id: String, index: i64)`

> 绑定：`engine/crates/rustlua-ui/src/ui_panel_lua.rs:493`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.focus_close(id: String)`

> 绑定：`engine/crates/rustlua-ui/src/lua_nav.rs:86`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.focus_count(id: String)`

> 绑定：`engine/crates/rustlua-ui/src/lua_nav.rs:93`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.focus_cycle(id: String, forward: Option<bool>)`

> 绑定：`engine/crates/rustlua-ui/src/lua_nav.rs:158`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.focus_index(id: String)`

> 绑定：`engine/crates/rustlua-ui/src/lua_nav.rs:117`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.focus_move(id: String, direction: String)`

> 绑定：`engine/crates/rustlua-ui/src/lua_nav.rs:132`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.focus_open(id: String, count: i64, columns: Option<i64>)`

> 绑定：`engine/crates/rustlua-ui/src/lua_nav.rs:60`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.focus_select(id: String, index: i64)`

> 绑定：`engine/crates/rustlua-ui/src/lua_nav.rs:103`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.get(id: String)`

> 绑定：`engine/crates/rustlua-ui/src/ui_lua.rs:172`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.grid(id: String, x: f32, y: f32, columns: i64, rows: i64, cell_size: f32, gap: f32)`

> 绑定：`engine/crates/rustlua-ui/src/ui_panel_lua.rs:233`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.grid_cell(id: String, index: i64, texture: Option<String>, r: f32, g: f32, b: f32, a: f32)`

> 绑定：`engine/crates/rustlua-ui/src/ui_panel_lua.rs:280`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.grid_clear(id: String)`

> 绑定：`engine/crates/rustlua-ui/src/ui_panel_lua.rs:312`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.grid_hit(id: String, x: f32, y: f32)`

> 绑定：`engine/crates/rustlua-ui/src/ui_panel_lua.rs:327`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.hide(id: String)`

> 绑定：`engine/crates/rustlua-ui/src/ui_lua.rs:131`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.image(id: String, x: f32, y: f32, width: f32, height: f32, texture: String, r: f32, g: f32, b: f32, slice: Option<String>)`

> 绑定：`engine/crates/rustlua-ui/src/ui_panel_lua.rs:162`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.image_get(id: String)`

> 绑定：`engine/crates/rustlua-ui/src/ui_panel_lua.rs:204`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.measure_text(text: String, size: Option<f32>)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/ui_measure.rs:19`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.modal(id: String, x: f32, y: f32, width: f32, height: f32, title: String)`

> 绑定：`engine/crates/rustlua-ui/src/ui_panel_lua.rs:399`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.modal_hide(id: String)`

> 绑定：`engine/crates/rustlua-ui/src/ui_panel_lua.rs:448`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.modal_show(id: String)`

> 绑定：`engine/crates/rustlua-ui/src/ui_panel_lua.rs:434`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.panel(id: String, x: f32, y: f32, width: f32, height: f32, r: f32, g: f32, b: f32, a: f32)`

> 绑定：`engine/crates/rustlua-ui/src/ui_panel_lua.rs:98`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.panel_get(id: String)`

> 绑定：`engine/crates/rustlua-ui/src/ui_panel_lua.rs:74`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.scene_text_visible(name: String, visible: Option<bool>)`

> 绑定：`engine/crates/rustlua-ui/src/ui_lua.rs:156`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.scroll_by(id: String, delta: f32)`

> 绑定：`engine/crates/rustlua-ui/src/lua_nav.rs:217`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.scroll_close(id: String)`

> 绑定：`engine/crates/rustlua-ui/src/lua_nav.rs:183`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.scroll_extents(id: String, viewport: f32, content: f32)`

> 绑定：`engine/crates/rustlua-ui/src/lua_nav.rs:190`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.scroll_lines(id: String, lines: f32, line_height: f32)`

> 绑定：`engine/crates/rustlua-ui/src/lua_nav.rs:226`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.scroll_max(id: String)`

> 绑定：`engine/crates/rustlua-ui/src/lua_nav.rs:207`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.scroll_offset(id: String)`

> 绑定：`engine/crates/rustlua-ui/src/lua_nav.rs:200`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.scroll_open(id: String, viewport: f32, content: f32)`

> 绑定：`engine/crates/rustlua-ui/src/lua_nav.rs:173`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.scroll_page(id: String, pages: f32)`

> 绑定：`engine/crates/rustlua-ui/src/lua_nav.rs:235`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.scroll_to_visible(id: String, item_top: f32, item_height: f32)`

> 绑定：`engine/crates/rustlua-ui/src/lua_nav.rs:244`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.scroll_visible(id: String)`

> 绑定：`engine/crates/rustlua-ui/src/lua_nav.rs:253`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.show(id: String)`

> 绑定：`engine/crates/rustlua-ui/src/ui_lua.rs:113`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.text(id: String, text: String)`

> 绑定：`engine/crates/rustlua-ui/src/ui_lua.rs:40`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.text_at(id: String, x: f32, y: f32, text: String, scale: Option<f32>, font_size: Option<f32>)`

> 绑定：`engine/crates/rustlua-ui/src/ui_lua.rs:57`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.text_get(id: String)`

> 绑定：`engine/crates/rustlua-ui/src/ui_lua.rs:186`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.tooltip(id: String, x: f32, y: f32, text: String, width: f32, height: f32)`

> 绑定：`engine/crates/rustlua-ui/src/ui_panel_lua.rs:350`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `ui.tooltip_hide(id: String)`

> 绑定：`engine/crates/rustlua-ui/src/ui_panel_lua.rs:385`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：
