---@meta
-- 生成物，请勿手改：`node scripts/docgen/gen-lua-api-docs.js`（与 `doc/*.md` 文档桩同源、同一次运行）。
-- 事实源：同目录 `lua_api_surface.json`——运行期 `_G` 探针导出的 API 面基线，由快照门钉住。
-- 参数名与类型来自绑定源码的签名推断；推断不到的（未定位到绑定点、或签名形状解析不出）发成 `---@param ... any`，本文件 3 个；另有 52 个真的不收参数。两种声明不合并（合并 = 假诊断）。
-- 返回类型在绑定源码里没有事实可依据，一律 `any`：不拿"没有信息"冒充"返回 nil"，否则
-- `local h = audio.play(x)` 会被 LuaLS 判成拿到 nil。
-- 装进工程：编辑器菜单「项目 → 导出 Lua 类型提示（LuaLS）」会把这份写到工程根 `.cache/`，
-- 并保证 `.luarc.json` 的 `workspace.library` 挂着它。

--- animation：运行期 API 面基线里的 13 个函数
animation = {}

---@param id integer
---@param component string
---@return any
function animation.attach(id, component) end

---@param id integer
---@return any
function animation.clips(id) end

---@param id integer
---@return any
function animation.ended(id) end

---@return any
function animation.events() end

---@param id integer
---@return any
function animation.frame(id) end

---@param id integer
---@return any
function animation.get(id) end

--- 暂停 `.anim` 时间线播放器（`AnimationPlayer`），保留当前时刻；暂停期间仍按当前时刻采样，`seek` 后的姿态会立刻生效。
---@param id integer
---@return any
function animation.pause(id) end

--- 从当前时刻恢复 `.anim` 时间线播放（清掉播放结束标记）；`autoplay=false` 的播放器需要它来起步。
---@param id integer
---@return any
function animation.play(id) end

--- 取走本帧 `.anim` 播放器触发的事件（一次性队列，取走即清）；与旧状态机的帧事件 `animation.events()` 是两条独立队列。
---@return any
function animation.player_events() end

--- 把 `.anim` 播放器定位到指定秒数：循环剪辑折回 `[0, duration)`，非循环夹到 `[0, duration]`，并清掉结束标记。
---@param id integer
---@param seconds number
---@return any
function animation.seek(id, seconds) end

---@param id integer
---@param name string
---@return any
function animation.set(id, name) end

--- 停止 `.anim` 播放并把时间归零（下一次 `play` 从片头开始），同时清掉结束标记。
---@param id integer
---@return any
function animation.stop(id) end

---@param id integer
---@return any
function animation.time(id) end

--- animation3d：运行期 API 面基线里的 6 个函数
animation3d = {}

---@param id integer
---@return any
function animation3d.get(id) end

---@param id integer
---@return any
function animation3d.list(id) end

---@param id integer
---@return any
function animation3d.paused(id) end

---@param id integer
---@param name string
---@return any
function animation3d.set(id, name) end

---@param id integer
---@param paused boolean
---@return any
function animation3d.set_paused(id, paused) end

---@param id integer
---@return any
function animation3d.time(id) end

--- app：运行期 API 面基线里的 3 个函数
app = {}

--- 返回当前宿主形态的标识串，用来回答"这台设备有没有键盘"。触屏宿主必须常驻屏上摇杆与虚拟按钮，
---@return any
function app.platform() end

---@return any
function app.project_root() end

---@return any
function app.quit() end

--- asset：运行期 API 面基线里的 2 个函数
asset = {}

---@param path string
---@return any
function asset.load(path) end

---@param token string
---@return any
function asset.release(token) end

--- audio：运行期 API 面基线里的 17 个函数
audio = {}

---@param text string
---@return any
function audio.apply_settings(text) end

---@param bus string
---@return any
function audio.bus_volume(bus) end

---@return any
function audio.buses() end

---@param bus string
---@return any
function audio.effective_volume(bus) end

--- 读回当前的听者位姿。
---@return any
function audio.listener() end

---@return any
function audio.load_settings() end

---@param name string
---@return any
function audio.music(name) end

---@param name string
---@return any
function audio.play(name) end

--- 在世界坐标处播放一次音效（SFX 总线），按听者位姿算距离衰减与左右声像。2D 工程省略 `z`（视作 0）。
---@param name string
---@param x number
---@param y number
---@param z number?
---@return any
function audio.play_at(name, x, y, z) end

---@param bus string
---@param name string
---@return any
function audio.play_on(bus, name) end

---@return any
function audio.save_settings() end

---@param bus string
---@param level number
---@return any
function audio.set_bus_volume(bus, level) end

--- 手动设定听者位姿（位置 + 前方向），上方向固定为 `+Y`。
---@param x number
---@param y number
---@param z number
---@param fx number
---@param fy number
---@param fz number
---@return any
function audio.set_listener(x, y, z, fx, fy, fz) end

---@return any
function audio.settings_path() end

---@return any
function audio.settings_text() end

---@return any
function audio.stop_music() end

---@param level number
---@return any
function audio.volume(level) end

--- calendar：运行期 API 面基线里的 3 个函数
calendar = {}

--- 改写日历的体裁尺寸：`options.days_per_season`（每季天数）与 `options.seasons_per_year`（每年季数）。
---@param options table
---@return any
function calendar.configure(options) end

---@return any
function calendar.now() end

---@param day integer
---@param season integer
---@param year integer
---@param minute integer
---@return any
function calendar.set(day, season, year, minute) end

--- camera：运行期 API 面基线里的 4 个函数
camera = {}

---@param id integer
---@return any
function camera.get(id) end

---@return any
function camera.get_active() end

---@param id integer
---@param x number
---@param y number
---@param z number
---@param tx number
---@param ty number
---@param tz number
---@param fov number?
---@return any
function camera.set(id, x, y, z, tx, ty, tz, fov) end

---@param id integer
---@return any
function camera.set_active(id) end

--- camera2d：运行期 API 面基线里的 10 个函数
camera2d = {}

---@param id integer?
---@param options table?
---@return any
function camera2d.follow(id, options) end

---@return any
function camera2d.get() end

---@return any
function camera2d.get_zoom() end

---@return any
function camera2d.mouse_world() end

---@param x number
---@param y number
---@return any
function camera2d.screen_to_world(x, y) end

---@param x number
---@param y number
---@return any
function camera2d.set(x, y) end

---@param zoom number
---@return any
function camera2d.set_zoom(zoom) end

---@param amplitude number
---@param duration number
---@return any
function camera2d.shake(amplitude, duration) end

---@return any
function camera2d.unfollow() end

---@param x number
---@param y number
---@return any
function camera2d.world_to_screen(x, y) end

--- camera3d：运行期 API 面基线里的 5 个函数
camera3d = {}

---@return any
function camera3d.get() end

---@return any
function camera3d.mouse_ray() end

---@param x number
---@param y number
---@param plane_y number
---@return any
function camera3d.pick_plane(x, y, plane_y) end

---@param x number
---@param y number
---@return any
function camera3d.ray(x, y) end

---@param x number
---@param y number
---@param z number
---@return any
function camera3d.world_to_screen(x, y, z) end

--- clock：运行期 API 面基线里的 6 个函数
clock = {}

---@param minutes number
---@return any
function clock.add(minutes) end

---@return any
function clock.now() end

---@param paused boolean
---@return any
function clock.pause(paused) end

---@param day integer
---@param minute integer
---@return any
function clock.set(day, minute) end

---@param rate number
---@return any
function clock.set_rate(rate) end

---@return any
function clock.sleep_to_morning() end

--- container：运行期 API 面基线里的 4 个函数
container = {}

---@param entity_id integer
---@return any
function container.close(entity_id) end

---@param entity_id integer
---@return any
function container.get(entity_id) end

---@param entity_id integer
---@param from_index integer
---@param to_id string
---@param to_index integer
---@return any
function container.move(entity_id, from_index, to_id, to_index) end

---@param entity_id integer
---@param capacity integer?
---@return any
function container.open(entity_id, capacity) end

--- data：运行期 API 面基线里的 4 个函数
data = {}

---@return any
function data.cache_clear() end

---@param path string
---@return any
function data.load(path) end

---@param path string
---@return any
function data.reload(path) end

---@param path string
---@param spec any?
---@return any
function data.schema(path, spec) end

--- day：运行期 API 面基线里的 2 个函数
day = {}

---@return any
function day.now() end

---@return any
function day.sleep() end

--- entity：运行期 API 面基线里的 6 个函数
entity = {}

---@param id integer
---@return any
function entity.destroy(id) end

---@param name string
---@return any
function entity.find(name) end

---@param persist_id string
---@return any
function entity.find_id(persist_id) end

---@param id integer
---@return any
function entity.is_alive(id) end

---@param id integer
---@return any
function entity.persist_id(id) end

---@param name string?
---@param persist_id string?
---@return any
function entity.spawn(name, persist_id) end

--- event：运行期 API 面基线里的 5 个函数
event = {}

---@param name string
---@param data any?
---@return any
function event.emit(name, data) end

---@param name string
---@param callback fun(...)?
---@return any
function event.off(name, callback) end

---@param name string
---@param callback fun(...)
---@return any
function event.on(name, callback) end

---@param name string
---@param callback fun(...)
---@return any
function event.subscribe(name, callback) end

---@param name string
---@param callback fun(...)?
---@return any
function event.unsubscribe(name, callback) end

--- grid：运行期 API 面基线里的 4 个函数
grid = {}

---@param cx integer
---@param cy integer
---@param tile_size number
---@return any
function grid.cell_to_world(cx, cy, tile_size) end

---@param x number
---@param y number
---@param facing string
---@param tile_size number
---@return any
function grid.facing_cell(x, y, facing, tile_size) end

---@param cx integer
---@param cy integer
---@param width integer
---@param height integer
---@return any
function grid.in_bounds(cx, cy, width, height) end

---@param x number
---@param y number
---@param tile_size number
---@return any
function grid.world_to_cell(x, y, tile_size) end

--- health：运行期 API 面基线里的 3 个函数
health = {}

---@param id integer
---@param amount number
---@return any
function health.damage(id, amount) end

---@param id integer
---@return any
function health.get(id) end

---@param id integer
---@param value number
---@return any
function health.set(id, value) end

--- input：运行期 API 面基线里的 40 个函数
input = {}

---@param name string
---@return any
function input.action(name) end

---@param negative string
---@param positive string
---@return any
function input.action_axis(negative, positive) end

---@return any
function input.action_map_text() end

---@param name string
---@return any
function input.action_pressed(name) end

---@param name string
---@return any
function input.action_released(name) end

---@param name string
---@return any
function input.action_repeat(name) end

---@return any
function input.actions() end

---@param axis string
---@return any
function input.axis(axis) end

---@param name string
---@return any
function input.bindings(name) end

---@return any
function input.cursor_delta() end

---@return any
function input.cursor_position() end

---@return any
function input.diagnostics() end

---@param key string
---@return any
function input.is_down(key) end

---@param key string
---@return any
function input.just_pressed(key) end

---@param key string
---@return any
function input.just_released(key) end

---@return any
function input.last_device() end

---@param path string?
---@return any
function input.load_settings(path) end

---@return any
function input.migration_notes() end

---@param button string
---@return any
function input.mouse_is_down(button) end

---@param button string
---@return any
function input.mouse_just_pressed(button) end

---@param button string
---@return any
function input.mouse_just_released(button) end

---@param path string?
---@return any
function input.nav_settings(path) end

---@param index integer
---@return any
function input.pad_axis(index) end

---@param index integer
---@return any
function input.pad_button(index) end

---@return any
function input.pointer_count() end

---@param index integer?
---@return any
function input.press_position(index) end

---@param key string
---@return any
function input.pressed(key) end

---@param name string
---@param binding string
---@return any
function input.rebind(name, binding) end

---@param key string
---@return any
function input.released(key) end

---@return any
function input.reset_bindings() end

---@param low number
---@param high number
---@param ms integer
---@return any
function input.rumble(low, high, ms) end

---@param path string?
---@return any
function input.save_settings(path) end

---@return any
function input.scroll_delta() end

---@param text string
---@return any
function input.set_action_map(text) end

---@param threshold number
---@param delay_ms integer
---@param repeat_ms integer
---@return any
function input.set_nav_settings(threshold, delay_ms, repeat_ms) end

---@return any
function input.settings_path() end

--- 读取本帧宿主送来的**文本**（输入法组合串与上屏字符），同时可声明"这个字段在等字"。键位与文本是两条通道：`input.is_down("w")` 问的是键，`input.text_events()` 问的是打出来的字——中文由输入法转出来，键名通道永远拿不到"你"这个字。
---@param command table?
---@return any
function input.text_events(command) end

---@param name string
---@param binding string
---@return any
function input.unbind(name, binding) end

---@param name string
---@param down boolean
---@return any
function input.virtual_key(name, down) end

---@param name string
---@return any
function input.virtual_pressed(name) end

--- interact：运行期 API 面基线里的 4 个函数
interact = {}

---@return any
function interact.clear_prompt() end

---@return any
function interact.get_prompt() end

---@param text string
---@return any
function interact.prompt(text) end

---@param x number
---@param y number
---@param radius number
---@return any
function interact.query(x, y, radius) end

--- inv：运行期 API 面基线里的 15 个函数
inv = {}

---@param id string
---@param item_id string
---@param count integer
---@return any
function inv.add(id, item_id, count) end

---@param id string
---@return any
function inv.capacity(id) end

---@param id string
---@param item_id string
---@return any
function inv.count(id, item_id) end

---@param id string
---@param capacity integer?
---@return any
function inv.create(id, capacity) end

---@param id string
---@param slot string
---@param item_id string?
---@return any
function inv.equip(id, slot, item_id) end

---@param id string
---@param slot string
---@return any
function inv.equip_get(id, slot) end

---@param id string
---@return any
function inv.equip_list(id) end

---@param id string
---@return any
function inv.exists(id) end

---@param id string
---@return any
function inv.get(id) end

---@param id string
---@param size integer?
---@return any
function inv.hotbar(id, size) end

---@param from_id string
---@param from_index integer
---@param to_id string
---@param to_index integer?
---@param count integer?
---@return any
function inv.move(from_id, from_index, to_id, to_index, count) end

---@param id string
---@param item_id string
---@param count integer
---@return any
function inv.remove(id, item_id, count) end

---@param id string
---@param index integer
---@param item_id string?
---@param count integer?
---@return any
function inv.set(id, index, item_id, count) end

---@param id string
---@param index integer
---@return any
function inv.slot(id, index) end

---@param from_id string
---@param from_index integer
---@param to_id string
---@param to_index integer
---@return any
function inv.swap(from_id, from_index, to_id, to_index) end

--- item：运行期 API 面基线里的 2 个函数
item = {}

---@param id string
---@return any
function item.def(id) end

---@param id string
---@param definition table
---@return any
function item.define(id, definition) end

--- light：运行期 API 面基线里的 3 个函数
light = {}

---@param id integer
---@return any
function light.get(id) end

---@param id integer
---@param dx number
---@param dy number
---@param dz number
---@param r number
---@param g number
---@param b number
---@param intensity number
---@return any
function light.set(id, dx, dy, dz, r, g, b, intensity) end

---@param id integer
---@return any
function light.set_active(id) end

--- log：运行期 API 面基线里的 3 个函数
log = {}

---@param message string
---@return any
function log.error(message) end

---@param message string
---@return any
function log.info(message) end

---@param message string
---@return any
function log.warn(message) end

--- map：运行期 API 面基线里的 9 个函数
map = {}

---@return any
function map.current() end

---@param name string
---@param x number
---@param y number
---@return any
function map.enter(name, x, y) end

---@param name string
---@param x number?
---@param y number?
---@return any
function map.go(name, x, y) end

---@type fun(name: string, x: number?, y: number?): any
map["goto"] = function(name, x, y) end

---@param name string
---@param spec any?
---@return any
function map.schema(name, spec) end

---@param x number
---@param y number
---@return any
function map.set_spawn(x, y) end

---@return any
function map.spawn() end

---@param name string
---@param value any?
---@return any
function map.state(name, value) end

---@param name string?
---@return any
function map.state_clear(name) end

--- mesh：运行期 API 面基线里的 3 个函数
mesh = {}

---@param id integer
---@return any
function mesh.clear(id) end

---@param id integer
---@return any
function mesh.get(id) end

---@param id integer
---@param path string
---@return any
function mesh.set(id, path) end

--- nav：运行期 API 面基线里的 6 个函数
nav = {}

--- 创建（或按名字替换）一张全可走的矩形导航网格，返回 1 基网格 id。
---@param name string
---@param width integer
---@param height integer
---@param cell_size number
---@param origin_x number
---@param origin_z number
---@return any
function nav.create_grid(name, width, height, cell_size, origin_x, origin_z) end

--- 销毁一张网格并释放它占用的槽位。
---@param id integer
---@return any
function nav.destroy_grid(id) end

--- 在格坐标上跑 A*，返回**未平滑**的格子路径（含起点与终点）。
---@param id integer
---@param sx integer
---@param sy integer
---@param gx integer
---@param gy integer
---@param diagonal boolean?
---@return any
function nav.find_path(id, sx, sy, gx, gy, diagonal) end

--- 世界坐标版本的寻路：先换算到格、跑 A*，再用视线检测做**字符串拉直**，
---@param id integer
---@param sx number
---@param sz number
---@param gx number
---@param gz number
---@param diagonal boolean?
---@return any
function nav.find_world_path(id, sx, sz, gx, gz, diagonal) end

--- 读取网格尺寸与原点，供脚本做坐标换算或自检。
---@param id integer
---@return any
function nav.grid_info(id) end

--- 标记单格是否阻挡。塔基、静态障碍在战斗开始前写成 `true`；单位不占用导航格
---@param id integer
---@param x integer
---@param y integer
---@param blocked boolean
---@return any
function nav.set_blocked(id, x, y, blocked) end

--- particles：运行期 API 面基线里的 2 个函数
particles = {}

---@param x number
---@param y number
---@param count integer
---@param options table?
---@return any
function particles.burst(x, y, count, options) end

---@return any
function particles.count() end

--- physics：运行期 API 面基线里的 10 个函数
physics = {}

---@param enabled boolean
---@return any
function physics.debug_draw(enabled) end

---@return any
function physics.events() end

---@param id integer
---@return any
function physics.get_ccd(id) end

---@param id integer
---@return any
function physics.get_velocity(id) end

---@param x number
---@param y number
---@param radius number
---@param opts table?
---@return any
function physics.overlap(x, y, radius, opts) end

---@param x number
---@param y number
---@param radius number
---@return any
function physics.positions_in_radius(x, y, radius) end

---@param x number
---@param y number
---@param dx number
---@param dy number
---@param max_distance number
---@param opts table?
---@return any
function physics.raycast(x, y, dx, dy, max_distance, opts) end

---@param id integer
---@param enabled boolean
---@return any
function physics.set_ccd(id, enabled) end

---@param id integer
---@param x number
---@param y number
---@return any
function physics.set_position(id, x, y) end

---@param ... any
---@return any
function physics.set_velocity(...) end

--- physics3d：运行期 API 面基线里的 9 个函数
physics3d = {}

---@param enabled boolean
---@return any
function physics3d.debug_draw(enabled) end

---@return any
function physics3d.events() end

---@param id integer
---@return any
function physics3d.get_ccd(id) end

---@param id integer
---@return any
function physics3d.get_velocity(id) end

---@param cx number
---@param cy number
---@param cz number
---@param radius number
---@param opts table?
---@return any
function physics3d.overlap(cx, cy, cz, radius, opts) end

---@param ox number
---@param oy number
---@param oz number
---@param dx number
---@param dy number
---@param dz number
---@param max_distance number
---@param opts table?
---@return any
function physics3d.raycast(ox, oy, oz, dx, dy, dz, max_distance, opts) end

---@param id integer
---@param enabled boolean
---@return any
function physics3d.set_ccd(id, enabled) end

---@param id integer
---@param x number
---@param y number
---@param z number
---@return any
function physics3d.set_position(id, x, y, z) end

---@param ... any
---@return any
function physics3d.set_velocity(...) end

--- 把 `.prefab` 实体树在运行期实例化进活世界，是 UI 预设的运行时载体：版面（层级、锚点、
--- prefab：运行期 API 面基线里的 7 个函数
prefab = {}

--- 销毁一次 spawn 的整棵实例，并把它并进宿主布局快照的节点一起撤掉。撤干净的标准是：实体不
---@param handle integer
---@return any
function prefab.despawn(handle) end

--- 按预设里写的实体名取实例内的某个节点 id。同一个预设 spawn 两次会得到两套不同的实体，所以
---@param handle integer
---@param name string
---@return any
function prefab.node(handle, name) end

--- 取实例根实体的 id，用于整棵树的显隐、位移或挂组件——预设文件里 `root` 那一行写的是谁，
---@param handle integer
---@return any
function prefab.root(handle) end

--- 运行期改一个节点的**四边**——列表行按数据排位、进度条按数值改宽度就靠它，于是"版面在预设里、
---@param handle integer
---@param name string
---@param left number
---@param top number
---@param right number
---@param bottom number
---@return any
function prefab.set_rect(handle, name, left, top, right, bottom) end

--- 改实例内某节点 `Text` 组件的文字——写的是渲染真正读的那一份，所以这一条补上之后，"版面进
---@param handle integer
---@param name string
---@param text string
---@return any
function prefab.set_text(handle, name, text) end

--- 隐藏/显示实例里的一个节点**及其整棵子树**。一屏里"这一刻不该出现的那些东西"靠它收起来：
---@param handle integer
---@param name string
---@param visible boolean
---@return any
function prefab.set_visible(handle, name, visible) end

--- 读一个 `.prefab` 并把它整棵实体树实例化进活世界，返回实例 handle。路径相对工程根，读取走
---@param path string
---@return any
function prefab.spawn(path) end

--- quad：运行期 API 面基线里的 2 个函数
quad = {}

---@param id integer
---@return any
function quad.get(id) end

---@param id integer
---@param component string
---@return any
function quad.set(id, component) end

--- save：运行期 API 面基线里的 9 个函数
save = {}

---@param key string
---@return any
function save.blob_read(key) end

---@param key string
---@param value any
---@return any
function save.blob_write(key, value) end

---@return any
function save.clear() end

---@return any
function save.commit() end

---@return any
function save.load() end

---@param key string
---@return any
function save.read(key) end

---@param slot string?
---@return any
function save.slot(slot) end

---@return any
function save.slot_get() end

---@param key string
---@param value string
---@return any
function save.write(key, value) end

--- scene：运行期 API 面基线里的 2 个函数
scene = {}

---@param name string
---@return any
function scene.load(name) end

---@return any
function scene.reload() end

--- shop：运行期 API 面基线里的 2 个函数
shop = {}

---@param name string
---@param item_id string
---@return any
function shop.price(name, item_id) end

---@param name string
---@param spec any
---@return any
function shop.set_catalog(name, spec) end

--- sprite：运行期 API 面基线里的 2 个函数
sprite = {}

---@param id integer
---@return any
function sprite.get(id) end

---@param id integer
---@param component string
---@return any
function sprite.set(id, component) end

--- tag：运行期 API 面基线里的 2 个函数
tag = {}

---@param id integer
---@param key string
---@return any
function tag.get(id, key) end

---@param id integer
---@param key string
---@param value string
---@return any
function tag.set(id, key, value) end

--- tilemap：运行期 API 面基线里的 17 个函数
tilemap = {}

--- 读出一格**这一帧实际采样图集里第几个条目**。开启逻辑瓦解算后，格子里存的 kind 与画出来用的
---@param id integer
---@param x integer
---@param y integer
---@return any
function tilemap.atlas_index(id, x, y) end

--- 4-bit 邻居掩码：上下左右四个方向"有没有同种邻居"，解算成过渡块的槽位序号。位序按 **index**
---@param id integer
---@param x integer
---@param y integer
---@return any
function tilemap.autotile_mask(id, x, y) end

--- 把一格退回**不染色**，即顶点色 `[1,1,1,1]`（乘法单位元）。阴影撤掉、季节切回常色走这条，
---@param id integer
---@param x integer
---@param y integer
---@return any
function tilemap.clear_tint(id, x, y) end

---@param width integer
---@param height integer
---@param tile_size number
---@param texture string?
---@param name string?
---@return any
function tilemap.create(width, height, tile_size, texture, name) end

---@param id integer
---@param from_x integer
---@param from_y integer
---@param to_x integer
---@param to_y integer
---@param diagonal boolean?
---@return any
function tilemap.find_path(id, from_x, from_y, to_x, to_y, diagonal) end

---@param id integer
---@return any
function tilemap.get(id) end

---@param id integer
---@param x integer
---@param y integer
---@return any
function tilemap.get_cell(id, x, y) end

---@param id integer
---@param x integer
---@param y integer
---@param key string
---@return any
function tilemap.get_meta(id, x, y, key) end

---@param id integer
---@param x integer
---@param y integer
---@return any
function tilemap.is_solid(id, x, y) end

--- 声明这张图集**有几列几行**——条目 `n`（1 基）取 `col=(n-1)%cols`、`row=(n-1)//cols` 的子矩形。
---@param id integer
---@param cols integer
---@param rows integer
---@return any
function tilemap.set_atlas(id, cols, rows) end

---@param id integer
---@param x integer
---@param y integer
---@param value integer
---@return any
function tilemap.set_cell(id, x, y, value) end

--- 切换**格子值的语义**：`variants<=1` 且 `autotile` 关时格子值**就是**图集条目号（恒等档，
---@param id integer
---@param kinds integer
---@param variants integer
---@param autotile boolean?
---@param seed integer?
---@return any
function tilemap.set_kinds(id, kinds, variants, autotile, seed) end

---@param id integer
---@param x integer
---@param y integer
---@param key string
---@param value string
---@return any
function tilemap.set_meta(id, x, y, key, value) end

---@param id integer
---@param x integer
---@param y integer
---@param solid boolean
---@return any
function tilemap.set_solid(id, x, y, solid) end

---@param id integer
---@param texture string
---@return any
function tilemap.set_texture(id, texture) end

--- 给一格涂**乘法**顶点色：季节/湿润的整体色调、建筑的阴影压暗都走这条路，而不是换贴图或盖一层
---@param id integer
---@param x integer
---@param y integer
---@param r number
---@param g number
---@param b number
---@param a number?
---@return any
function tilemap.set_tint(id, x, y, r, g, b, a) end

--- 读回一格当前的染色，与画进顶点色的是同一份数据（门用它对账"解算里的 id 有没有真的落到 mesh"）。
---@param id integer
---@param x integer
---@param y integer
---@return any
function tilemap.tint(id, x, y) end

--- time：运行期 API 面基线里的 1 个函数
time = {}

---@return any
function time.delta() end

--- transform：运行期 API 面基线里的 6 个函数
transform = {}

---@param id integer
---@param space string
---@return any
function transform.attach(id, space) end

---@param id integer
---@return any
function transform.get(id) end

---@param id integer
---@param dx number
---@param dy number
---@return any
function transform.move(id, dx, dy) end

---@param id integer
---@param x number
---@param y number
---@param rotation number?
---@param scale_x number?
---@param scale_y number?
---@return any
function transform.set(id, x, y, rotation, scale_x, scale_y) end

---@param id integer
---@return any
function transform.space(id) end

---@param id integer
---@return any
function transform.world(id) end

--- transform3d：运行期 API 面基线里的 5 个函数
transform3d = {}

---@param id integer
---@return any
function transform3d.attach(id) end

---@param id integer
---@return any
function transform3d.get(id) end

---@param id integer
---@param dx number
---@param dy number
---@param dz number
---@return any
function transform3d.move(id, dx, dy, dz) end

---@param ... any
---@return any
function transform3d.set(...) end

---@param id integer
---@return any
function transform3d.world(id) end

--- ui：运行期 API 面基线里的 56 个函数
ui = {}

---@param id string
---@param x number
---@param y number
---@param width number
---@param height number
---@param ratio number
---@param r number
---@param g number
---@param b number
---@return any
function ui.bar(id, x, y, width, height, ratio, r, g, b) end

---@param id string
---@param cx number
---@param cy number
---@param radius number
---@param r number
---@param g number
---@param b number
---@param a number
---@return any
function ui.circle(id, cx, cy, radius, r, g, b, a) end

---@param id string
---@return any
function ui.clear(id) end

---@return any
function ui.drag_active() end

---@param id string
---@param index integer
---@return any
function ui.drag_begin(id, index) end

---@return any
function ui.drag_cancel() end

---@param id string
---@param index integer
---@return any
function ui.drag_end(id, index) end

---@param id string
---@return any
function ui.focus_close(id) end

---@param id string
---@return any
function ui.focus_count(id) end

---@param id string
---@param forward boolean?
---@return any
function ui.focus_cycle(id, forward) end

---@param id string
---@return any
function ui.focus_index(id) end

---@param id string
---@param direction string
---@return any
function ui.focus_move(id, direction) end

---@param id string
---@param count integer
---@param columns integer?
---@return any
function ui.focus_open(id, count, columns) end

---@param id string
---@param index integer
---@return any
function ui.focus_select(id, index) end

---@param id string
---@return any
function ui.get(id) end

---@param id string
---@param x number
---@param y number
---@param columns integer
---@param rows integer
---@param cell_size number
---@param gap number
---@return any
function ui.grid(id, x, y, columns, rows, cell_size, gap) end

---@param id string
---@param index integer
---@param texture string?
---@param r number
---@param g number
---@param b number
---@param a number
---@return any
function ui.grid_cell(id, index, texture, r, g, b, a) end

---@param id string
---@return any
function ui.grid_clear(id) end

---@param id string
---@param x number
---@param y number
---@return any
function ui.grid_hit(id, x, y) end

---@param id string
---@return any
function ui.hide(id) end

---@param id string
---@param x number
---@param y number
---@param width number
---@param height number
---@param texture string
---@param r number
---@param g number
---@param b number
---@param slice string?
---@return any
function ui.image(id, x, y, width, height, texture, r, g, b, slice) end

---@param id string
---@return any
function ui.image_get(id) end

---@param text string
---@param size number?
---@return any
function ui.measure_text(text, size) end

---@param id string
---@param x number
---@param y number
---@param width number
---@param height number
---@param title string
---@return any
function ui.modal(id, x, y, width, height, title) end

---@param id string
---@return any
function ui.modal_hide(id) end

---@param id string
---@return any
function ui.modal_show(id) end

--- 页面导航栈与模态遮罩。返回键语义固定：先关最上层模态，再弹出一页，根页不响应返回；
---@param command table
---@return any
function ui.nav_stack(command) end

---@param id string
---@param x number
---@param y number
---@param width number
---@param height number
---@param r number
---@param g number
---@param b number
---@param a number
---@return any
function ui.panel(id, x, y, width, height, r, g, b, a) end

---@param id string
---@return any
function ui.panel_get(id) end

--- 读写 `Progress` 组件的进度值；进度条是**只读展示型控件**，指针与键盘不会改它。
---@param id integer
---@param value number?
---@return any
function ui.progress(id, value) end

--- 解析受限富文本。只认 `<b>`、`<color=#rrggbb>`、`<icon=name>`、`<br/>` 四种标签；
---@param text string
---@return any
function ui.rich_text(text) end

---@param name string
---@param visible boolean?
---@return any
function ui.scene_text_visible(name, visible) end

---@param id string
---@param delta number
---@return any
function ui.scroll_by(id, delta) end

---@param id string
---@return any
function ui.scroll_close(id) end

---@param id string
---@param viewport number
---@param content number
---@return any
function ui.scroll_extents(id, viewport, content) end

---@param id string
---@param lines number
---@param line_height number
---@return any
function ui.scroll_lines(id, lines, line_height) end

---@param id string
---@return any
function ui.scroll_max(id) end

---@param id string
---@return any
function ui.scroll_offset(id) end

---@param id string
---@param viewport number
---@param content number
---@return any
function ui.scroll_open(id, viewport, content) end

---@param id string
---@param pages number
---@return any
function ui.scroll_page(id, pages) end

---@param id string
---@param item_top number
---@param item_height number
---@return any
function ui.scroll_to_visible(id, item_top, item_height) end

---@param id string
---@return any
function ui.scroll_visible(id) end

--- 读写 `ScrollBar` 组件的 0..=1 滚动位置；滑块长度由组件的 `size` 决定。
---@param id integer
---@param value number?
---@return any
function ui.scrollbar(id, value) end

---@param id string
---@return any
function ui.show(id) end

--- 读写 `Slider` 组件值；写入先夹到 `min..=max`，`whole_numbers` 为真时再吸附到最近整数。
---@param id integer
---@param value number?
---@return any
function ui.slider(id, value) end

---@param id string
---@param text string
---@return any
function ui.text(id, text) end

---@param id string
---@param x number
---@param y number
---@param text string
---@param scale number?
---@param font_size number?
---@return any
function ui.text_at(id, x, y, text, scale, font_size) end

---@param id string
---@return any
function ui.text_get(id) end

--- 文本输入状态与 IME 最小支持。光标/选区都以**字符**下标表示（不是字节），中文与 emoji
---@param command table
---@return any
function ui.text_input(command) end

--- 读写 `Toggle` 组件状态；写入走单选组语义，`group` 非空且置 `true` 时会关掉同组其他开关。
---@param id integer
---@param value boolean?
---@return any
function ui.toggle(id, value) end

---@param id string
---@param x number
---@param y number
---@param text string
---@param width number
---@param height number
---@return any
function ui.tooltip(id, x, y, text, width, height) end

---@param id string
---@return any
function ui.tooltip_hide(id) end

---@param path string
---@return any
function ui.view_model(path) end

---@param path string?
---@return any
function ui.view_model_clear(path) end

---@param path string
---@param value any
---@return any
function ui.view_model_set(path, value) end

--- 虚拟列表：把上千项按固定行高映射成「可见窗口 + 有限槽位」。布局只绑定窗口内的行，
---@param command table
---@return any
function ui.virtual_list(command) end

--- visibility：运行期 API 面基线里的 3 个函数
visibility = {}

---@return any
function visibility.all() end

---@param id integer
---@return any
function visibility.get(id) end

---@param id integer
---@param layers integer
---@return any
function visibility.set(id, layers) end

--- world_text：运行期 API 面基线里的 2 个函数
world_text = {}

---@param id integer
---@param component string
---@return any
function world_text.attach(id, component) end

---@param id integer
---@return any
function world_text.detach(id) end
