---@meta
-- 生成物，请勿手改：`node scripts/docgen/gen-lua-api-docs.js`（与 `doc/*.md` 文档桩同源、同一次运行）。
-- 事实源：同目录 `lua_api_surface.json`——运行期 `_G` 探针导出的 API 面基线，由快照门钉住。
-- 参数名与类型来自绑定源码的签名推断；推断不到的（未定位到绑定点、或签名形状解析不出）发成 `---@param ... any`，本文件 3 个；另有 71 个真的不收参数。两种声明不合并（合并 = 假诊断）。
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

--- animation3d：运行期 API 面基线里的 7 个函数
animation3d = {}

---@param id integer
---@param name string
---@param seconds number
---@return any
function animation3d.crossfade(id, name, seconds) end

--- 切换骨骼剪辑并做**交叉淡入**：目标剪辑从 0 秒起播，旧剪辑在它自己的时间轴上继续走并在
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

--- `load_settings` / `save_settings` 读写 `project.toml` 的 `[audio]` 段；除四路音量外，
--- audio：运行期 API 面基线里的 20 个函数
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

--- 在世界坐标处播放一次音效（SFX 总线），按听者位姿算距离衰减与左右声像；距离还会
---@param name string
---@param x number
---@param y number
---@param z number?
---@param occlusion number?
---@return any
function audio.play_at(name, x, y, z, occlusion) end

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

--- 把后续所有播放（含 BGM 续播、空间音效）切到单声道；`true` 时空间音效退回普通 Sink，不再重新铺成立体声。
---@param mono boolean
---@return any
function audio.set_mono(mono) end

--- 设置低/高频衰减。两段都只允许 `[-24, 0]`，`0` 表示不切；**不做 boost**。
---@param bass_db number
---@param treble_db number
---@return any
function audio.set_tone_cut(bass_db, treble_db) end

---@return any
function audio.settings_path() end

---@return any
function audio.settings_text() end

---@return any
function audio.stop_music() end

--- 读回当前无障碍音色。
---@return any
function audio.tone() end

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

--- `camera` 是**透视相机 payload** 的读写与选择链：`set_active` 只认带 `Camera` 组件的
--- camera：运行期 API 面基线里的 5 个函数
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

--- 设置/清除相机的**会话级世界空间裁剪平面**：保留 `n·x + d >= 0` 的一侧，负侧
---@param id integer
---@param nx number
---@param ny number
---@param nz number
---@param d number
---@return any
function camera.set_clip_plane(id, nx, ny, nz, d) end

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

--- cells3d：运行期 API 面基线里的 7 个函数
cells3d = {}

---@param path string
---@return any
function cells3d.bounds(path) end

---@param opts table
---@return any
function cells3d.build(opts) end

---@param path string
---@return any
function cells3d.count(path) end

---@param path string
---@param index number
---@return any
function cells3d.dig(path, index) end

---@param path string
---@param index number
---@return any
function cells3d.fill(path, index) end

---@param path string
---@param x number
---@param y number
---@param z number
---@return any
function cells3d.index(path, x, y, z) end

---@param path string
---@param x number
---@param y number
---@param z number
---@return any
function cells3d.solid_at(path, x, y, z) end

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

--- 同步调用 `name` 上的全部监听者，`data` 原样作为唯一参数（省略时是 `nil`）。
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

--- light：运行期 API 面基线里的 4 个函数
light = {}

---@param id integer
---@return any
function light.get(id) end

--- 把**点光源**的世界坐标移到 `(x,y,z)`。方向光会报错；方向仍用 `light.set` 改。
---@param id integer
---@param x number
---@param y number
---@param z number
---@return any
function light.move(id, x, y, z) end

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

--- `mesh` 给实体挂/换 3D 网格与材质覆盖。网格路径来自 `.glb/.gltf`，或引擎哨兵
--- mesh：运行期 API 面基线里的 14 个函数
mesh = {}

--- 读这个实体上的实例批次里有多少份实例变换。实体没有批次时返回 `0`（不是 `nil`）。
---@param id integer
---@return any
function mesh.batch_count(id) end

--- 读这个实体上挂的**远距替代网格**：`{mesh=路径, distance=阈值}`；挂了第二台阶时
---@param id integer
---@return any
function mesh.batch_lod(id) end

--- 读回实例批次的**平面反射排除档**：组件缺席（默认）或 `enabled = true` 时返回
---@param id integer
---@return any
function mesh.batch_planar(id) end

--- 读这个实体上的实例批次里**第 `index` 个实例此刻的位姿**（世界位置 + 绕 Y 的 yaw）。
---@param id integer
---@param index integer
---@return any
function mesh.batch_pose(id, index) end

--- 移除实体的 `MeshRenderer`，3D 网格立刻不再绘制；之后 `mesh.get` 返回 `nil`。
---@param id integer
---@return any
function mesh.clear(id) end

--- 摘掉这个实体上的实例批次：下一帧起这些实例不再绘制。实体上的普通 `MeshRenderer`
---@param id integer
---@return any
function mesh.clear_batch(id) end

--- 摘掉远距替代网格：下一帧起整批回到同一张网格。批次本身（`mesh.clear_batch`）与普通
---@param id integer
---@return any
function mesh.clear_batch_lod(id) end

--- 读当前网格路径；实体没有 `MeshRenderer` 时返回 `nil`，不是空串。
---@param id integer
---@return any
function mesh.get(id) end

--- 给实体挂/换 3D 网格。路径可以是项目相对 `.glb/.gltf`，也可以是内置哨兵；替换不会
---@param id integer
---@param path string
---@return any
function mesh.set(id, path) end

--- 把 N 份实例变换收进**一个**实体：渲染侧逐实例做视锥剔除，把留在视野里的那些合成
---@param id integer
---@param path string
---@param instances any
---@param material string?
---@param motion table?
---@return any
function mesh.set_batch(id, path, instances, material, motion) end

--- 给实例批次挂**远距替代网格链**：离相机超过 `distance` 米的实例改画 `path`；再给
---@param id integer
---@param path string
---@param distance number
---@param far_path string?
---@param far_distance number?
---@return any
function mesh.set_batch_lod(id, path, distance, far_path, far_distance) end

--- 把实例批次**排除出水面平面反射 pass**：`enabled = false` 后这一批不再参与水面
---@param id integer
---@param enabled boolean
---@return any
function mesh.set_batch_planar(id, enabled) end

--- 就地替换批次里从 `start` 起的一段实例：只解析被改的那 K 份、只动这一段，**数量与
---@param id integer
---@param start integer
---@param instances any
---@return any
function mesh.set_batch_range(id, start, instances) end

--- 给已有 `MeshRenderer` 的实体设置 `.material` 覆盖；这是 X-ray/半透明外壳一类
---@param id integer
---@param path string?
---@return any
function mesh.set_material(id, path) end

--- nav：运行期 API 面基线里的 11 个函数
nav = {}

--- 清空整张网格的**动态**阻挡层；`set_blocked` 写入的静态墙原样保留。
---@param id integer
---@return any
function nav.clear_dynamic(id) end

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

--- 查询某个世界位置朝流场目标前进的**单位方向**。只有该格可走、能到达目标、且流场
---@param id integer
---@param x number
---@param z number
---@return any
function nav.flow_vector(id, x, z) end

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

--- 在**动态覆盖层**标记单格阻挡，与静态层相互独立。单位、召唤物、临时路障这类会消失的
---@param id integer
---@param x integer
---@param y integer
---@param blocked boolean
---@return any
function nav.set_dynamic_blocked(id, x, y, blocked) end

--- 设置流场目标（世界坐标，换算到所在格）。两个参数都省略表示**清空目标**并停掉流场；
---@param id integer
---@param gx number?
---@param gz number?
---@return any
function nav.set_flow_goal(id, gx, gz) end

--- 按预算推进一步流场构建，返回当前状态：`"idle"`（没设目标）、`"building"`（还在建）、
---@param id integer
---@param budget integer?
---@return any
function nav.update_flow(id, budget) end

--- 一条到中继服务器（`ClashRoyale/server/server.mjs`）的长连接。桌面走裸 TCP
--- net：运行期 API 面基线里的 10 个函数
net = {}

--- 这个进程/页面是否被要求"启动就联机"。桌面读环境变量 `RUSTLUA_NET_AUTO`，
---@return any
function net.auto_start() end

--- 主动断开当前连接（没有连接时是空操作）。连接中/等待中/已配对都会转为 `closed`；
---@return any
function net.close() end

--- 连接中继服务器并加入房间。已有连接会先关闭；房间名必须匹配 `[A-Za-z0-9_-]{1,32}`。
---@param server string
---@param room string
---@return any
function net.connect(server, room) end

--- 连接 UDP 中继并加入房间（本端身份由 `player` 指定）。桌面专属；浏览器构建返回 `false`。
---@param server string
---@param room string
---@param player integer
---@return any
function net.connect_udp(server, room, player) end

--- 默认房间码：桌面取 `RUSTLUA_NET_ROOM`，浏览器取查询参数 `?room=`，都没有时是 `royale`。
---@return any
function net.default_room() end

--- 默认服务器地址：桌面取 `RUSTLUA_NET_SERVER`，默认 `tcp://127.0.0.1:8787`；
---@return any
function net.default_server() end

--- 单调毫秒时钟（桌面取进程启动后的 `Instant`，浏览器取 `performance.now()`）。给消息打时间戳、
---@return any
function net.now() end

--- 取一条已经到达的二进制消息（FIFO）。空字符串也是一条消息；没有消息时返回 `nil`。
---@return any
function net.recv() end

--- 发送一条二进制消息（对端 `net.recv` 原样收到）。只在 `online` 状态可发；
---@param bytes string
---@return any
function net.send(bytes) end

--- 读取连接状态表，并把传输层已到达的事件并入状态（桌面侧在此刻收事件）。
---@return any
function net.status() end

--- 粒子是**视觉层**：`particles.count()` 只读得到 CPU 池；`mode="gpu"` 的 burst 交给渲染器后，
--- particles：运行期 API 面基线里的 24 个函数
particles = {}

--- 在二维世界坐标 `(x, y)` 打一簇粒子。缺省参数保持历史行为（速度 120、寿命 0.6 秒、橙色）。
---@param x number
---@param y number
---@param count integer
---@param options table?
---@return any
function particles.burst(x, y, count, options) end

--- 在三维世界坐标打一簇粒子；除 `burst` 的全部选项外，额外接受 `gravity_z`、`speed_z`、
---@param x number
---@param y number
---@param z number
---@param count integer
---@param options table?
---@return any
function particles.burst3d(x, y, z, count, options) end

--- 删除**全部** Lua 连续流（场景里声明的流不受影响），用于天气切换/场景重置。
---@return any
function particles.clear_streams3d() end

--- 移除网格体积风场，粒子回到只受全局风/风场/风源影响的状态（场景重置、火灭时用）。无场时调用是空操作。
---@return any
function particles.clear_wind_grid() end

--- 清空全部 4 个局部风源槽（火灭/场景重置时用）。
---@return any
function particles.clear_wind_sources() end

--- 读回 CPU 粒子池里当前存活的粒子数（GPU 池不在这里计数，见模块说明）。
---@return any
function particles.count() end

--- 设置全局风加速度（世界单位/秒²）；CPU 粒子池与 GPU burst 共用同一份风，宿主每帧把它同步给 GPU 后端。
---@param x number
---@param y number
---@return any
function particles.set_wind(x, y) end

--- 设置三分量全局风；`x/y` 与 `set_wind` 是同一份状态。`z` 同时进入 CPU 粒子池与 GPU
---@param x number
---@param y number
---@param z number
---@return any
function particles.set_wind3d(x, y, z) end

--- 建一张覆盖世界区域的三维速度/温度网格（体积风场 + 温度浮力场）。粒子每步按所在格三线性采样成加速度，
---@param nx integer
---@param ny integer
---@param nz integer
---@param ox number
---@param oy number
---@param oz number
---@param cell number
---@return any
function particles.set_wind_grid(nx, ny, nz, ox, oy, oz, cell) end

--- 在全局风之上加**风场**：高度剪切 + 空间阵风。倍率公式为
---@param shear number
---@param gust number
---@param gust_hz number
---@return any
function particles.set_wind_profile(shear, gust, gust_hz) end

--- 设置一个**局部风源**（4 个槽，`index` 0..=3）。坐标与 `burst3d` 一致：`x/y` 是水平面
---@param index integer
---@param x number
---@param y number
---@param z number
---@param radius number
---@param radial number
---@param lift number
---@param swirl number?
---@return any
function particles.set_wind_source(index, x, y, z, radius, radial, lift, swirl) end

--- 停止并按 key 删除一条 Lua 连续流。
---@param key string
---@return any
function particles.stop_stream3d(key) end

--- 声明/更新一条**持续 3D 发射器**（粒子/秒）。同一 `key` 再次调用只更新原点与参数；宿主每帧
---@param key string
---@param x number
---@param y number
---@param z number
---@param rate number
---@param options table?
---@return any
function particles.stream3d(key, x, y, z, rate, options) end

--- 读回当前 Lua 连续流的条数（不是 GPU 粒子数，也不是场景流条数）。
---@return any
function particles.stream_count() end

--- 读回当前全局风加速度。
---@return any
function particles.wind() end

--- 读回三分量全局风（含 CPU 池的 z）。
---@return any
function particles.wind3d() end

--- 往格 `(x,y,z)` **累加**速度增量（脉冲注入，如火柱每步往上推一点）；扩散会把它抹开到邻格。
---@param x integer
---@param y integer
---@param z integer
---@param vx number
---@param vy number
---@param vz number
---@return any
function particles.wind_grid_add_velocity(x, y, z, vx, vy, vz) end

--- 按**世界坐标**钉一处温度（火源灌热用）：引擎把世界点映到**最近节点**（偏差不超过半格），
---@param x number
---@param y number
---@param z number
---@param value number
---@return any
function particles.wind_grid_heat(x, y, z, value) end

--- 读回场的几何（各轴格数、原点、格距）；只读，不改场。无场返回 `nil`。
---@return any
function particles.wind_grid_info() end

--- 配置网格模拟系数；全部有限且在范围内才整批写入，否则一个都不改。
---@param diffusion number
---@param velocity_decay number
---@param temperature_decay number
---@param buoyancy number
---@param iterations integer
---@return any
function particles.wind_grid_params(diffusion, velocity_decay, temperature_decay, buoyancy, iterations) end

--- 按**世界坐标**往最近节点**累加**速度增量（火柱每帧往上推一点，热气与上升气流就长起来）；
---@param x number
---@param y number
---@param z number
---@param vx number
---@param vy number
---@param vz number
---@return any
function particles.wind_grid_push(x, y, z, vx, vy, vz) end

--- 只读采样世界点 `(x,height,z)` 的加速度 `(ax,ay,az)`（三线性插值的速度 + 温度浮力）；不改场。
---@param x number
---@param y number
---@param z number
---@return any
function particles.wind_grid_sample(x, y, z) end

--- 设置格 `(x,y,z)` 的温度（绝对值，如把火源格钉在某温度）；浮力按 `buoyancy·温度` 抬升该格速度，扩散把热量抹开。
---@param x integer
---@param y integer
---@param z integer
---@param value number
---@return any
function particles.wind_grid_set_temperature(x, y, z, value) end

--- 读回当前风场 `(shear, gust, gust_hz)`。
---@return any
function particles.wind_profile() end

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

--- physics3d：运行期 API 面基线里的 11 个函数
physics3d = {}

--- 给实体挂上**逐格体素的碰撞体**（P3-R215）。`opts` 就是 `cells3d.build(opts)` 那张表
---@param id integer
---@param opts table
---@return any
function physics3d.add_cells(id, opts) end

--- 为实体添加一块**静态地形高度场**（P3-R55）。`heights` 行主序、`rows × cols` 个数；
---@param id integer
---@param x number
---@param y number
---@param z number
---@param rows integer
---@param cols integer
---@param sx number
---@param sy number
---@param sz number
---@param heights table
---@return any
function physics3d.add_heightfield(id, x, y, z, rows, cols, sx, sy, sz, heights) end

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

--- render：运行期 API 面基线里的 18 个函数
render = {}

---@param depth number
---@return any
function render.set_clip_cap(depth) end

---@param on any
---@return any
function render.set_clip_cap_both_sides(on) end

--- 覆盖云底高度（世界 y，40..200，默认 100）。天空体积云的采样区间变为
---@param base number
---@return any
function render.set_cloud_base(base) end

--- 覆盖云量（0..1）：地面/水面/世界 Sprite 的云影阈值按它偏移，天空体积云密度也按它偏移。
---@param coverage number
---@return any
function render.set_cloud_coverage(coverage) end

--- 覆盖云影强度（0..0.6，默认 0.26）：地面/水面/Sprite 云影按
---@param strength number
---@return any
function render.set_cloud_shadow_strength(strength) end

--- 覆盖云层漂移速度倍率（0..4，默认 1）。地面/水面/Sprite 云影与天空体积云共用同一速度；
---@param speed number
---@return any
function render.set_cloud_speed(speed) end

--- 设置曝光偏移（EV，-4..4），叠加在 Tonemap 的 `exposure_ev` 上；用于雨天压暗、
---@param ev number
---@return any
function render.set_exposure(ev) end

--- 设置屏幕闪电闪光（0..1）；Tonemap 在雾之后、色调映射之前叠加冷白色。脚本每帧
---@param intensity number
---@return any
function render.set_flash(intensity) end

--- 覆盖渲染器的距离雾密度（`RenderSettings.fog_density`）。宿主每帧读一次，只在值变化时
---@param density number
---@return any
function render.set_fog(density) end

--- 覆盖渲染器的雾颜色（线性 RGB，`0..=1`）。宿主每帧读一次并应用到
---@param r number
---@param g number
---@param b number
---@return any
function render.set_fog_color(r, g, b) end

--- 设置暗角/颗粒强度（各 0..1）：暗角在 Tonemap 后按屏幕径向压暗，
---@param vignette number
---@param grain number
---@return any
function render.set_post_fx(vignette, grain) end

--- 运行期画质档（1=低、2=中、3=高）。低档关 SSAO/SSR/接触阴影/泛光/点光阴影；
---@param level integer
---@return any
function render.set_quality(level) end

--- 在世界 `(x,z)` 触发一圈水面涟漪（消费一次）：半径随时间扩到约 15，1.5 秒内淡出。
---@param x number
---@param z number
---@return any
function render.set_ripple(x, z) end

--- 把阴影投射者的距离预算调到 `meters`（0..=1000 的整数米；`0` = 关）。相机到 drawable 超过预算的投射者
---@param meters any
---@return any
function render.set_shadow_caster_distance(meters) end

--- 设置地面覆盖（各 0..1）：`snow` 把地形颜色拉向冷白，`wet` 压暗地表。宿主每帧读一次；
---@param snow number
---@param wet number
---@return any
function render.set_surface_cover(snow, wet) end

--- 把水面折射的 Snell 步进数调到 `steps`（0..=32 的整数；0 = 关 = 回退到 P3-R207 的「光程加权偏移」）。
---@param steps any
---@return any
function render.set_water_refract_steps(steps) end

--- 设置水体染色 rgb 与浑浊度 w（各 0..1）。水体先把基色乘 `rgb` 再按 0.65 混合；
---@param r number
---@param g number
---@param b number
---@param turbidity number
---@return any
function render.set_water_tint(r, g, b, turbidity) end

--- 返回上一帧渲染统计表：
---@return any
function render.stats() end

--- 预测与回滚账本。它**不碰模拟**：快照是脚本自己序列化的字节串（整张世界表、场景状态都行），
--- rollback：运行期 API 面基线里的 9 个函数
rollback = {}

--- 收下一条**远端**输入，并返回账本的判定。
---@param player integer
---@param frame integer
---@param payload string
---@return any
function rollback.accept(player, frame, payload) end

--- 确认这一帧已模拟完并推进 `next_frame`，同时淘汰窗口外的快照。
---@param frame integer
---@return any
function rollback.commit(frame) end

--- 取这一帧所有玩家要用的输入：已确认的用真值，远端还没到的沿用**上一帧的输入作为预测**。
---@param frame integer
---@return any
function rollback.inputs(frame) end

--- 当前应该模拟的帧号（`0` 起；回滚后会把光标退回到 `from`）。
---@return any
function rollback.next_frame() end

--- 登记本地玩家这一帧的输入（会和 `inputs()` 里本地那一项一致）。
---@param frame integer
---@param payload string
---@return any
function rollback.record_local(frame, payload) end

--- 建/重设账本。热重载保留账本对象，但显式 `reset` 会清空——开新局时调用。
---@param players integer
---@param local_ integer
---@param window integer
---@return any
function rollback.reset(players, local_, window) end

--- 取出 `from` 帧**模拟前**的快照字节，并把 `next_frame` 退到 `from`；调用方负责把世界恢复成这份快照再重跑。
---@param from integer
---@return any
function rollback.rollback_to(from) end

--- 保存这一帧**模拟前**的世界快照，供将来回滚；窗口外的旧快照会被自动淘汰。
---@param frame integer
---@param bytes string
---@return any
function rollback.save_snapshot(frame, bytes) end

--- 回滚账本的累计统计（给 HUD/诊断用，也可在无头对拍里断言）。
---@return any
function rollback.stats() end

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

--- 地形是**程序化高度场**，不是资产：`spec` 就是 `MeshRenderer.mesh` 里的
--- terrain3d：运行期 API 面基线里的 7 个函数
terrain3d = {}

--- 返回地形高度场的世界空间 AABB，供相机取景、裁剪和粗略拾取用。
---@param spec string
---@return any
function terrain3d.bounds(spec) end

--- 可见地面的高度查询；返回值和网格顶点来自同一个 `TerrainSpec::height`。
---@param spec string
---@param x number
---@param z number
---@return any
function terrain3d.height_at(spec, x, z) end

--- 返回单位表面法线，用于让道具、草叶沿坡面倾斜。
---@param spec string
---@param x number
---@param z number
---@return any
function terrain3d.normal_at(spec, x, z) end

--- 在高度场上加一个局部平滑土堆/坑，返回**新的地形 spec 字符串**；原 spec 不变。
---@param spec string
---@param x number
---@param z number
---@param radius number
---@param delta number
---@return any
function terrain3d.raise(spec, x, z, radius, delta) end

--- 把一条射线打到高度场上，返回最近命中点；给鼠标点地面、放挖掘工具与 `camera3d.ray` 配套使用。
---@param spec string
---@param ox number
---@param oy number
---@param oz number
---@param dx number
---@param dy number
---@param dz number
---@param max number
---@return any
function terrain3d.raycast(spec, ox, oy, oz, dx, dy, dz, max) end

--- 返回坡度角（度，0 = 平地），常用于判断某点能不能筑巢/行走。
---@param spec string
---@param x number
---@param z number
---@return any
function terrain3d.slope_at(spec, x, z) end

--- 返回该点的 4 通道程序化地表权重（草 / 苔 / 落叶 / 裸土），可用于选植被、粒子颜色或
---@param spec string
---@param x number
---@param z number
---@return any
function terrain3d.splat_at(spec, x, z) end

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

--- transform：运行期 API 面基线里的 8 个函数
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

--- 读取实体的**局部**位置，返回两个数 `x, y`，不创建 table。2D 实体走 `Transform`，
---@param id integer
---@return any
function transform.position(id) end

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

--- 读取实体的**世界**位置，返回两个数 `x, y`，不创建 table。语义与 `transform.world` 相同：
---@param id integer
---@return any
function transform.world_position(id) end

--- transform3d：运行期 API 面基线里的 7 个函数
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

--- 读取 3D 实体的**局部**位置，返回 `x, y, z` 三个数，不创建 table；只认 `Transform3D`，
---@param id integer
---@return any
function transform3d.position(id) end

---@param ... any
---@return any
function transform3d.set(...) end

---@param id integer
---@return any
function transform3d.world(id) end

--- 读取 3D 实体的**世界**位置，返回 `x, y, z` 三个数，不创建 table。已有传播结果时直接取；
---@param id integer
---@return any
function transform3d.world_position(id) end

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

--- 按 id 新增/覆盖一张屏幕空间的动态贴图，用来画会随状态变化的图标、条、框与卡面。同一 id 重复调用
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

--- 水面是**程序化透明片**，不是水体模拟：spec 就是 `MeshRenderer.mesh` 里那条
--- water3d：运行期 API 面基线里的 3 个函数
water3d = {}

--- 计算某点“水面高度 - 地形高度”，正数表示有水，负数表示露出水面的干地。
---@param water_spec string
---@param terrain_spec string
---@param x number
---@param z number
---@return any
function water3d.depth_at(water_spec, terrain_spec, x, z) end

--- 返回水面在该点的瞬时高度（静水面 + 低频涟漪），与水面网格顶点同源。
---@param spec string
---@param x number
---@param z number
---@return any
function water3d.height_at(spec, x, z) end

--- 返回静水面高度 `y`，适合做洪水线/岸线判定的基准，不含涟漪。
---@param spec string
---@return any
function water3d.level(spec) end

--- world_text：运行期 API 面基线里的 2 个函数
world_text = {}

---@param id integer
---@param component string
---@return any
function world_text.attach(id, component) end

---@param id integer
---@return any
function world_text.detach(id) end
