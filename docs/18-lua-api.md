# M4-05 Lua 游戏 API

- 状态：PASS
- 完成日期：2026-08-20

## 交付内容

- `engine/crates/engine_core/src/script.rs`
- `engine/crates/game_runner/examples/lua_api_demo.rs`
- `docs/18-lua-api.md`

## M8-06 新增 3D API

| 模块 | API | 说明 |
|---|---|---|
| `transform3d` | `attach/get/set/move` | 挂接/查询/设置/移动 3D 变换；`set(id,x,y,z[,rx,ry,rz,rw[,sx,sy,sz]])` |
| `mesh` | `set/get/clear` | 设置/读取/清除 `MeshRenderer` 网格路径 |
| `camera` | `set_active/get_active/get` | 切换/查询活动相机；`get(id)` 返回 `x,y,z,tx,ty,tz,fov,near,far` |
| `light` | `set_active/get/set` | 设置/读取方向光；`set(id,dx,dy,dz,r,g,b,intensity)` |
| `physics3d` | `get_velocity/set_position/events/raycast/overlap` | 3D 物理查询：`get_velocity(id)`→`{x,y,z}`；`events()`→碰撞事件数组 `{a,b,started}`；`raycast(ox,oy,oz,dx,dy,dz,max)`→命中实体 id 或 nil；`overlap(cx,cy,cz,r)`→实体 id 数组 |

3D 组件在 `instantiate_scene` 时从场景字符串解析为 World 组件；`camera.set_active` 与 `light.set_active` 由宿主渲染器每帧读取，未选择时回退到首个 `Camera3D` / `Light` 实体。

## M8-07 新增骨骼动画 API

| 模块 | API | 说明 |
|---|---|---|
| `animation3d` | `get/set/list/paused/set_paused/time` | 查询/切换当前剪辑；`get(id)` 返回当前剪辑名，`set(id,name)` 切换并重播，`list(id)` 列出全部剪辑；宿主每帧推进时间 |

`Animation3D` 场景组件格式：`mesh=<path>;clip=<name>;looped=true|false`，剪辑来自同一 glTF 资产的 animations。

## 第一版 API

| 模块 | API | 说明 |
|---|---|---|
| `log` | `info/warn/error` | 通过 tracing 输出脚本日志 |
| `time` | `delta` | 获取当前帧 Delta Time |
| `input` | `action/axis/pressed/released` | 键盘与鼠标输入快照 |
| `entity` | `find/spawn/is_alive/destroy` | 创建、查找、检查和销毁实体；`find(name)` 按显示名（可冲突）查 |
| `entity` | `spawn(name?, persist_id?)` / `find_id(persist_id)` / `persist_id(id)` | B-1 稳定实体 ID：`spawn` 第二参注册项目级唯一 `persist_id`，重复注册报错；`find_id` 反向取回活实体（不存在返回 nil），`persist_id(id)` 查询实体绑定的 ID；`destroy` 与切场景清理对应注册 |
| `sprite` | `set(id, 组件串)` | 设置/替换实体 Sprite 组件（`texture=...;size=...`，与场景组件串同格式） |
| `transform` | `get/set/move` | 读取和修改位置、旋转、缩放 |
| `health` | `get/set/damage` | Demo 用生命值与受伤 API |
| `particles` | `burst(x,y,count)` | 在 `(x,y)` 爆发 count 个粒子（橙色，扩散速度 120、寿命 0.6s） |
| `physics` | `overlap/raycast` | 最小圆形重叠和射线查询 |
| `scene` | `load/reload` | 场景状态切换接口 |
| `asset` | `load/release` | 资源句柄生命周期接口 |
| `ui` | `text/text_at/show/hide/get` | 最小文本 UI 状态接口；`text_at(id,x,y,text[,scale])` 可选缩放 |
| `audio` | `play/volume/music/stop_music` | 音效播放与音量；`music(name)` 循环 BGM，`stop_music()` 停止 |
| `save` | `read/write/load/commit` | 键值存档接口（load 从磁盘载入，commit 原子写入） |
| `app` | `quit` | 请求退出应用 |

## 农场 API（P0/P1 最小闭环）

以下模块用于把 2D 农场循环下沉为稳定的 Lua API。所有接口都运行在主线程共享状态中，错误转换为 Lua 错误而不是 panic。

| 模块 | API | 说明 |
|---|---|---|
| `grid` | `world_to_cell(x,y,tile_size)` / `cell_to_world(cx,cy,tile_size)` / `facing_cell(x,y,facing,tile_size)` / `in_bounds` | 世界坐标与格子坐标互转；`facing` 支持 `up/down/left/right` 及 `n/s/w/e` |
| `tilemap` | `create(width,height,tile_size[,texture][,name])` / `get(id)` / `is_solid(id,x,y)` / `get_cell(id,x,y)` / `set_cell(id,x,y,value)` / `set_solid(id,x,y,bool)` / `get_meta(id,x,y,key)` / `set_meta(id,x,y,key,value)` / `find_path(id,fx,fy,tx,ty,diagonal?)` | 运行时创建空 Tilemap 实体（用于数据驱动的农田网格，headless 也适用），或对既有 Tilemap 写视觉格、碰撞层和每格字符串元数据；`set_meta` 空 value 删除键；格子值 `0`=空、`1..=6`=贴图集列（纹理列采样，见 `assets/textures/tileset.png`）；`find_path` 以 solid 为阻挡做 A* 四/八向寻路（可选 diagonal，禁穿角），成功返回含两端的 `{x,y}` 路径表，失败返回 `nil, reason` |
| `clock` | `now` / `set_rate` / `pause` / `add` / `set` / `sleep_to_morning` | 独立游戏时钟，默认第 1 天 06:00；`set_rate(n)` 表示每秒推进 n 个游戏分钟，宿主在固定步进时调用 |
| `calendar` | `now` / `set` | 日期/季节/年（M16-08）。`now()` 返回 `absolute_day/day/season/year/minute`（`day` 为季节内第几天，1~28，28 天/季 × 4 季/年）；`set(day,season,year,minute)` 按绝对天数写入 |
| `day` | `sleep` / `now` | 每日生命周期（M16-08）。`sleep()` 推进到次日 06:00 并触发全局 `on_day_end` → `on_day_start` 回调（脚本定义了才调用）；时钟自然跨午夜时宿主也会触发 `on_day_start`。`now()` 返回 `day/minute` |
| `tag` | `get` / `set` | 给实体附加字符串标签；空 value 删除标签 |
| `interact` | `query(x,y,radius)` / `prompt(text)` / `clear_prompt` / `get_prompt` | 按距离和 `priority` 排序查询可交互实体，显示/清除交互提示 |
| `item` | `define(id,def)` / `def(id)` | 注册和查询物品定义（`name/description/stack/buy/sell/texture/kind`） |
| `inv` | `create/hotbar/capacity/exists/add/remove/count/get/slot/set/swap/move/equip/equip_get/equip_list` | 固定容量背包、堆叠与快捷栏；槽位从 1 开始。`equip(inv,slot,item_id)` 写入/清除（nil 清除）装备槽（M16-07 骨架，暂无属性效果），`equip_get` 查询单槽，`equip_list` 返回槽→物品表 |
| `container` | `open(entity_id,capacity)` / `get` / `close` / `move` | 把实体 ID 关联为箱子背包，并可与玩家背包互转 |
| `save` | `blob_write(key,value)` / `blob_read(key)` / `slot(name)` / `slot_get` | 任意 Lua 值（表/数字/字符串/布尔）经 `__blob:<key>` JSON 存入键值存档，可用 `slot(name)` 切换 `save_<slot>.sav`。持久化走 `commit()`（原子写盘）、读档走 `load()`（见上表）。注意（兼容层收口后）：**无 `version()`、无 `on_migrate` 迁移钩子**——`load()` 只接受 `save_version=1`，缺失或版本不符返回 false 且不覆盖原文件 |
| `shop` | `set_catalog` / `price` | 商店目录与价格服务（M16-10）。`set_catalog(name,spec)` 注册目录（`{item_id:{buy,sell}}`），`price(name,item_id)` 返回 `{buy,sell}` 或 nil；买卖交易与资金在 Lua 玩法层完成 |
| `map` | `go(name,x,y)` / `enter(name,x,y)` / `current` / `spawn` / `set_spawn` / `schema(name,spec)` / `state(name[,value])` | 带出生点切图、当前图查询、分地图 blob 持久化；`schema` 注册分地图状态的轻量结构校验器，`schema(name,nil)` 清除；`state(name,value)` 写入前校验，错误定位到地图名与字段 |
| `ui` | `panel/image/grid/grid_cell/grid_clear/grid_hit/tooltip/modal/modal_show/modal_hide/clear/drag_begin/drag_end/drag_active/drag_cancel` | 运行时动态面板、图片、背包网格、tooltip、模态层与拖拽回调 |
| `ui` | `panel_get(id)` | 读回 `ui.panel` 建的动态面板：`{x,y,width,height,r,g,b,a,visible}`；不存在返回 `nil`（M22，UI 动画/可见性的 headless 断言用） |

`ui.hide(id)` 作用于**同 id 的动态面板与文本**（M22 起也隐藏面板；此前只认文本，导致「先 clear 再重画」
的面板类 UI 隐藏不掉、上一次选中项的残留高亮框会留在画面上）。注意宿主是在脚本 `update()` 返回后
才取本帧顶点，因此「先 `update()`（内部 clear + 按输入改状态）再 `render()`」才是能看见的绘制顺序。
| `data` | `load(path)` / `reload(path)` / `cache_clear` / `schema(path,spec)` | 从项目根读取 JSON 数据表并缓存为 Lua 表。`schema(path,spec)` 注册校验器（M16-11）：`spec` 为 `{["$kind"]="map"|"object"|"array", ...}` 的轻量 schema，注册后 `load/reload` 先校验再缓存，非法数据报错定位到文件与字段；`schema(path,nil)` 清除。裸对象 `{name="string",...}` 视为隐式 object schema |
| `event` | `emit(name,data?)` / `on(name,cb)` / `subscribe(name,cb)` / `off(name,cb?)` / `unsubscribe(name,cb?)` | 通用命名事件总线（B-5）。`emit` 依序调用该事件全部订阅回调（传 `data`，缺省 nil），回调可重入 emit/订阅/取消；`off`/`unsubscribe` 不传回调时清空该事件全部订阅。订阅随 VM 生命周期，热重载后由 `on_create` 重订阅 |

`ui.drag_end(to_id,to_index)` 会调用全局 `on_drag(from_id,from_index,to_id,to_index)`（若脚本定义了）。`map.schema(name,spec)` 注册分地图状态校验器，`map.schema(name,nil)` 清除；`map.state(name)` 读取，`map.state(name,value)` 写入对应地图的持久化状态并在写入前校验。

`tilemap.set_solid` 会立即重建该 Tilemap 实体的固定物理碰撞体；Tilemap 的 tile、solid、meta 快照由脚本按约定写入 `map.state`，并可通过 `save.commit/load` 跨场景、重启恢复。

## M20-02 音频分组、工程设置文件与 UI 焦点/滚动导航

### `audio` 音量分组（总线）

| 模块 | API | 说明 |
|---|---|---|
| `audio` | `buses()` | 总线名数组（顺序固定）：`master`、`bgm`、`sfx`、`ui` |
| `audio` | `set_bus_volume(bus, v)` | 设置某总线音量（`0..=1`，超界钳制）；未知总线名报 Lua 错误（便于定位拼写错误） |
| `audio` | `bus_volume(bus)` | 该总线的**独立**音量（不含主音量；`master` 即主音量） |
| `audio` | `effective_volume(bus)` | 实际生效音量 = 主音量 × 分组音量（`master` 等于主音量） |
| `audio` | `play_on(bus, name)` | 在指定总线播一次音效，返回 `ok, reason`；**无输出设备时静默降级**（返回 `true` 只记日志），缺文件/解码失败才返回 `false` |
| `audio` | `settings_text()` | 四路音量的版本化文本（版本头 `rustlua-audio-v1`，逐行 `bus=0.000`） |
| `audio` | `apply_settings(text)` | 应用设置文本；版本头不符返回 `false, reason`，未知总线/非法数值忽略 |
| `audio` | `settings_path()` | `<project>/settings.toml` 绝对路径（无工程根返回 `nil`） |
| `audio` | `load_settings()` | 读取 `settings.toml` 的 `[audio]` 段并应用，返回 `ok, detail`；文件缺失返回 `false, "missing"`（不算错误） |
| `audio` | `save_settings()` | 把当前四路音量**保序**写回 `settings.toml`（保留注释与其他段），返回 `ok, detail` |

`play/volume/music/stop_music` 语义不变：`play` 等价于 SFX 总线，`music` 使用 BGM 总线且改音量即时生效。

### 工程设置文件 `<project>/settings.toml`（可选，M20-02）

```toml
# RustLua 工程设置（由编辑器「项目设置」面板写入；缺失即默认）
[audio]
master = 1.0
bgm = 1.0
sfx = 1.0
ui = 1.0

[input]
# 命名动作映射（M20-07）。单行转义字符串，首行为版本头；由
# `input.save_settings()` 写入、`input.load_settings()` 读取。
map = "rustlua-actions-v1\ninteract=key:e,pad:0\n..."
```

- 文件**可选**：缺失时全部使用默认值（音频 `1.0`、输入用引擎默认动作表），不影响启动；当前定义 `[audio]` 与 `[input]` 两段。
- 解析宽松：未知段、未知键、非法数值一律忽略（保留默认），不 panic。
- 写入保序：只替换/补齐 `[audio]` 段内的四个键与 `[input]` 段内的 `map` 键，注释与其他段原样保留。
- `game_jhyt` 在 boot（`scripts/core/audio.lua` → `audio.load_settings()`）应用该文件；暂停菜单「设置」页 `-/+` 调整四路音量后按「保存设置」写回。**主音量**同时随存档 provider `audio` 持久化（M19-02 语义），分组音量以本文件为准。

### `ui` 焦点环与滚动（运行时 UI 导航）

焦点环（`core/ui_navigation.rs` 的 `FocusRing`）用于列表/网格的方向键选择，序号统一 **0 基**；滚动状态（`ScrollState`）用于把选中行滚入视口。

| 模块 | API | 说明 |
|---|---|---|
| `ui` | `focus_open(id, count[, columns])` | 建立/重建 `id` 的焦点环（`count` 项，按 `columns` 列布局，默认 1 列），返回项数；已有环保留当前焦点（项仍存在时） |
| `ui` | `focus_select(id, index)` | 按 0 基序号指定焦点；越界返回 `false` |
| `ui` | `focus_index(id)` | 当前焦点序号（无焦点/无环返回 `nil`） |
| `ui` | `focus_move(id, "up"\|"down"\|"left"\|"right")` | 方向移动（到边界**环绕**），返回新序号；未知方向报错，无环返回 `nil` |
| `ui` | `focus_cycle(id[, forward])` | Tab 式循环（默认正向） |
| `ui` | `focus_count(id)` / `focus_close(id)` | 项数 / 删除焦点环 |
| `ui` | `scroll_open(id, viewport, content)` / `scroll_close(id)` | 建立/删除滚动状态（像素） |
| `ui` | `scroll_extents(id, viewport, content)` | 更新视口/内容高度并重新钳制偏移，返回当前偏移 |
| `ui` | `scroll_by(id, delta)` / `scroll_lines(id, lines, line_h)` / `scroll_page(id, pages)` | 按像素/行/页滚动，返回**实际生效**的增量（已钳制，不会出现空白滚动） |
| `ui` | `scroll_offset(id)` / `scroll_max(id)` | 当前偏移 / 最大偏移（无该 id 返回 `nil`） |
| `ui` | `scroll_to_visible(id, item_top, item_height)` | 最小滚动让该项进入视口，返回是否发生滚动 |
| `ui` | `scroll_visible(id)` | 当前可见区间 `{ start = , stop = }`（字段名避开 Lua 关键字 `end`） |

面板侧的用法见 `examples/game_jhyt/scripts/ui/panel_nav.lua`（1 基 ↔ 0 基换算与降级）与 `inventory_ui/quest_ui/shop/pause_menu/dialogue`。

### 方向键（物理键）

- 宿主把平台的 Arrow Up/Down/Left/Right 映射为引擎物理键 `InputKey::ARROW_UP/DOWN/LEFT/RIGHT`（独立游戏窗口 `game_app.rs` 与编辑器 Play 预览 `game_view.rs` 同源）；Lua 侧用 `input.is_down("up")` / `input.just_pressed("down")` / `"left"` / `"right"` 读取。
- 这是**物理键**层，与命名动作（`input.action*`、手柄 DPad/摇杆）无关；命名动作见下节。

### `input` 命名动作、手柄与绑定持久化（M20-07 → M22）

三层模型：**物理输入**（键鼠 + 宿主每帧喂入的手柄设备帧，见 `docs/33`）→ **命名动作**（`ActionMap`，`ScriptState` 内只有一份实例）→ **动作消费**（下表 API）。动作查询同时考虑键盘与手柄绑定，因此脚本不必区分玩家用哪种设备。

| 模块 | API | 说明 |
|---|---|---|
| `input` | `action(name)` | 任一绑定当前按下（含手柄按钮与手柄轴过阈值），返回布尔 |
| `input` | `action_pressed(name)` / `action_released(name)` | 本逻辑帧的按下/松开边沿（键盘、手柄按钮与**手柄轴越阈值**的边沿都保留到被消费；M22/W3 起轴也有边沿，滞回进入 0.5 / 退出 0.4） |
| `input` | `action_repeat(name)` | 菜单导航专用（M22/W3）：**按下边沿**立即返回 `true` 一步；模拟量绑定（摇杆）按住超过 `nav_delay_ms` 后每 `nav_repeat_ms` 再返回一次；数字键/DPad 按住不连发（保持「一次按下 = 一步」） |
| `input` | `action_axis(negative, positive)` | 合成轴：手柄摇杆取模拟量（−1..1）、键位取 −1/0/1。**首参为负方向动作**，例如 `input.action_axis("ui_down", "ui_up")`（上为正）、`input.action_axis("ui_left", "ui_right")` |
| `input` | `pad_button(index)` / `pad_axis(index)` | 原始手柄查询（调试与特殊玩法用）：按钮 0..15、轴 0..5，索引契约见 `docs/33` |
| `input` | `last_device()` | `"keyboard"` / `"gamepad"`：最近一次产生**真实按下边沿**的设备（M22/W3）。空闲帧不改变，用它给 UI 提示选文案，不必再每帧扫描绑定 |
| `input` | `rumble(low, high, ms)` | 手柄震动（M22/W2）：左右马达强度 0..1、时长毫秒（上限 5000，`0` = 立即停止）。只登记请求，宿主在帧末交给设备层；无手柄时静默无效 |
| `input` | `actions()` | 全部动作名的数组（字典序，1 基表） |
| `input` | `bindings(action)` | 该动作的绑定文本数组，形如 `{"key:e", "pad:0", "padaxis:1+"}` |
| `input` | `rebind(action, binding)` | **清空并重设**该动作的绑定，返回 `ok, err`；绑定文本非法时返回 `false, 原因`（原绑定保留） |
| `input` | `unbind(action, binding)` | 移除单个绑定，返回是否确实移除 |
| `input` | `reset_bindings()` | 恢复引擎默认动作表 |
| `input` | `action_map_text()` | 当前映射的持久化文本（首行版本头 `rustlua-actions-v1`，逐行 `动作=绑定,绑定`） |
| `input` | `set_action_map(text)` | 用文本替换当前映射，返回 `ok, err`；**版本头不符不改动当前映射**，返回 `false, 原因` |
| `input` | `settings_path()` | `settings.toml` 实际路径（无工程根返回 `nil`） |
| `input` | `load_settings([path])` | 读取 `[input].map` 并应用（M22/W3 起**同时应用导航手感**：阈值 + 连发节奏）；文件缺失返回 `false, "missing"`（保留当前绑定）；版本/绑定非法则**回退默认映射**并记录诊断；缺 `map` 键只诊断、在位绑定保持不变；已有内容用 `merge_missing` 补齐缺失动作（**不覆盖用户重绑定**） |
| `input` | `save_settings([path])` | 保序读改写 `settings.toml` 的 `[input]` 段（原子写、保留注释与 `[audio]` 段），返回 `ok, err` |
| `input` | `diagnostics()` | 输入设置加载/保存产生的诊断消息数组（可定位到文件与原因） |
| `input` | `nav_settings([path])` | 读取 `[input]` 段的菜单导航手感，返回 `{threshold, delay_ms, repeat_ms}`；文件缺失/无工程根返回 `nil`（调用方保留默认 0.5 / 350ms / 90ms）。解析宽松 + 钳制：阈值 0..1、时长 ≤5000ms、间隔 ≥16ms。旧的 `nav_delay_frames`/`nav_repeat_frames`（M20-09）仍可读，按 60Hz 换算成毫秒（M22/W3） |
| `input` | `set_nav_settings(threshold, delay_ms, repeat_ms)` | 把导航手感推给引擎（M22/W3）：阈值成为设备层轴判定阈值，两个时长供 `action_repeat` 使用；返回钳制后的生效值 |

- 路径规则与 `audio.settings_path/load_settings/save_settings` 一致：缺省为工程根下 `settings.toml`，相对路径相对工程根解析；无工程根时无参调用返回 `false, "no_project_root"`。
- 绑定文本语法：`key:<字符/名字>`（如 `key:e`、`key:space`、`key:up`）、`pad:<按钮序号>`（如 `pad:0`）、`padaxis:<轴序号><+|->`（如 `padaxis:1+`）。
- 默认动作表（`ActionMap::with_defaults()`，36 项）：移动 `move_up/down/left/right`；玩法
  `interact`/`attack`/`skill`/`gift`/`sleep`/`practice`/`inventory`/`pause`/`build`/`place`/`demolish`/
  `use`/`quest`/`respawn`/`seed`；音量 `volume_up`/`volume_down`；数字直选 `slot_1`…`slot_9`；
  菜单 `confirm`/`cancel`；焦点导航 `ui_up/down/left/right`。其中 `confirm`/`cancel` 与 `ui_*`
  额外带手柄默认绑定（`pad:0`/`pad:1`、左摇杆 `padaxis:0±`/`padaxis:1±`、DPad `pad:12..15`）。
- 旧物理键 API（`input.is_down/just_pressed/just_released/axis/pressed/released`）语义**完全不变**，老脚本继续可用；`input.action` 已从「物理键别名」升级为「命名动作查询」（仓库内除引擎自身测试外无原调用方）。
- `game_jhyt` 从物理键迁移到命名动作由 **M20-08** 完成；本任务只提供引擎层 API 与持久化。

## 安全与错误策略

- Entity ID 使用 `index/generation` 编码，销毁后旧 ID 不会指向新实体；
- 非法、负数、代际为零或已销毁的 Entity ID 返回 Lua 运行时错误；
- Transform、生命值、物理查询拒绝非法浮点数和负数参数；
- API 错误转换为 Lua 错误，不通过 panic 终止宿主；
- API 运行在主线程共享状态中，后续可替换为 ECS Command 队列以支持安全跨系统修改；
- 当前 `scene/asset/ui/save` 是可测试的内存状态适配层，真实持久化和渲染绑定留给后续 M4/M5 任务。

## 脚本组织

- 项目脚本（`scripts/main.lua`）支持 `require` 加载同目录模块（`package.path` 已包含脚本目录，如 `require("player")` → `scripts/player.lua`）；
- 样板游戏将玩法按职责拆分：`game.lua` 编排、`state.lua` 共享状态、`player/enemies/coins/save_manager/hud` 各司其职，`main.lua` 仅作入口转发生命周期回调；
- 热重载会重建 Lua VM，模块随脚本一并重新加载（闭包/协程/全局状态不跨重载迁移，见 [19-lua-hot-reload.md](19-lua-hot-reload.md)）。

## 验收

```text
cargo clippy -p engine_core --all-targets --all-features -- -D warnings
通过

cargo test -p engine_core --all-features
94 passed; 0 failed

cargo run -p game_runner --example lua_api_demo --release
lua_api_demo_ok update=lua-api-ok quit=false
```
