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
| `entity` | `find/spawn/is_alive/destroy` | 创建、查找、检查和销毁实体 |
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
| `tilemap` | `create(width,height,tile_size[,texture][,name])` / `get(id)` / `is_solid(id,x,y)` / `get_cell(id,x,y)` / `set_cell(id,x,y,value)` / `set_solid(id,x,y,bool)` / `get_meta(id,x,y,key)` / `set_meta(id,x,y,key,value)` | 运行时创建空 Tilemap 实体（用于数据驱动的农田网格，headless 也适用），或对既有 Tilemap 写视觉格、碰撞层和每格字符串元数据；`set_meta` 空 value 删除键；格子值 `0`=空、`1..=6`=贴图集列（纹理列采样，见 `assets/textures/tileset.png`） |
| `clock` | `now` / `set_rate` / `pause` / `add` / `set` / `sleep_to_morning` | 独立游戏时钟，默认第 1 天 06:00；`set_rate(n)` 表示每秒推进 n 个游戏分钟，宿主在固定步进时调用 |
| `calendar` | `now` / `set` | 日期/季节/年（M16-08）。`now()` 返回 `absolute_day/day/season/year/minute`（`day` 为季节内第几天，1~28，28 天/季 × 4 季/年）；`set(day,season,year,minute)` 按绝对天数写入 |
| `day` | `sleep` / `now` | 每日生命周期（M16-08）。`sleep()` 推进到次日 06:00 并触发全局 `on_day_end` → `on_day_start` 回调（脚本定义了才调用）；时钟自然跨午夜时宿主也会触发 `on_day_start`。`now()` 返回 `day/minute` |
| `tag` | `get` / `set` | 给实体附加字符串标签；空 value 删除标签 |
| `interact` | `query(x,y,radius)` / `prompt(text)` / `clear_prompt` / `get_prompt` | 按距离和 `priority` 排序查询可交互实体，显示/清除交互提示 |
| `item` | `define(id,def)` / `def(id)` | 注册和查询物品定义（`name/description/stack/buy/sell/texture/kind`） |
| `inv` | `create/hotbar/capacity/exists/add/remove/count/get/slot/set/swap/move/equip/equip_get/equip_list` | 固定容量背包、堆叠与快捷栏；槽位从 1 开始。`equip(inv,slot,item_id)` 写入/清除（nil 清除）装备槽（M16-07 骨架，暂无属性效果），`equip_get` 查询单槽，`equip_list` 返回槽→物品表 |
| `container` | `open(entity_id,capacity)` / `get` / `close` / `move` | 把实体 ID 关联为箱子背包，并可与玩家背包互转 |
| `save` | `blob_write(key,value)` / `blob_read(key)` / `slot(name)` / `slot_get` / `version` | 任意 Lua 值（表/数字/字符串/布尔）存入现有存档；按槽位写入 `save_<slot>.sav`。`version()` 返回最近加载存档的版本号（M16-09）；`load()` 读到旧版本存档时自动调用全局 `on_migrate(from,to)`，无版本头视为 v0 |
| `shop` | `set_catalog` / `price` | 商店目录与价格服务（M16-10）。`set_catalog(name,spec)` 注册目录（`{item_id:{buy,sell}}`），`price(name,item_id)` 返回 `{buy,sell}` 或 nil；买卖交易与资金在 Lua 玩法层完成 |
| `map` | `go(name,x,y)` / `enter(name,x,y)` / `current` / `spawn` / `set_spawn` / `state(name[,value])` | 带出生点切图、当前图查询、分地图 blob 持久化。`map.goto` 保留为 `map["goto"]`（Lua 5.4 保留字），源码推荐用 `map.go` |
| `ui` | `panel/image/grid/grid_cell/grid_clear/grid_hit/tooltip/modal/modal_show/modal_hide/clear/drag_begin/drag_end/drag_active/drag_cancel` | 运行时动态面板、图片、背包网格、tooltip、模态层与拖拽回调 |
| `data` | `load(path)` / `reload(path)` / `cache_clear` / `schema(path,spec)` | 从项目根读取 JSON 数据表并缓存为 Lua 表。`schema(path,spec)` 注册校验器（M16-11）：`spec` 为 `{["$kind"]="map"|"object"|"array", ...}` 的轻量 schema，注册后 `load/reload` 先校验再缓存，非法数据报错定位到文件与字段；`schema(path,nil)` 清除。裸对象 `{name="string",...}` 视为隐式 object schema |

`ui.drag_end(to_id,to_index)` 会调用全局 `on_drag(from_id,from_index,to_id,to_index)`（若脚本定义了）。`map.state(name)` 读取、`map.state(name,value)` 写入对应地图的持久化状态。

`tilemap.set_solid` 会立即重建该 Tilemap 实体的固定物理碰撞体；当前 tilemap meta 仍为运行时状态，跨场景持久化由 `map.state` / `save.blob_*` 由脚本自行保存。

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
