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
| `camera` | `set_active/get_active/get` | **唯一**的活动相机选择链（批 9 起不再分 2D/3D）：`set_active(id)` 只要求实体带 `Camera`，由 `projection` 决定它服务哪个 pass（透视 → 3D world，正交 → 2D overlay）；`get(id)` 读写**透视 payload**，返回 `x,y,z,tx,ty,tz,fov,near,far` |
| `light` | `set_active/get/set` | 设置/读取方向光；`set(id,dx,dy,dz,r,g,b,intensity)` |
| `physics3d` | `get_velocity/set_position/events/raycast/overlap` | 3D 物理查询：`get_velocity(id)`→`{x,y,z}`；`events()`→碰撞事件数组 `{a,b,started}`；`raycast(ox,oy,oz,dx,dy,dz,max[,groups])`→命中实体 id 或 nil；`overlap(cx,cy,cz,r[,groups])`→实体 id 数组。可选尾参 `groups = {layer=N, mask=N}` 按碰撞分组过滤（缺省全收，见下文"物理查询的分组过滤"） |

3D 组件在 `instantiate_scene` 时从场景字符串解析为 World 组件；`camera.set_active` 与 `light.set_active` 由宿主渲染器每帧读取。3D pass 在**选中的是透视相机**时用它，否则回退到排序序第一台启用的透视相机；2D 视图在**选中的是正交相机**时用它（宿主采用它的载荷作为 2D 取景基准），否则保持现状。`enabled=false` 的相机不参与选择（选中它会告警并回退）。

## M8-07 新增骨骼动画 API

| 模块 | API | 说明 |
|---|---|---|
| `animation3d` | `get/set/list/paused/set_paused/time` | 查询/切换当前剪辑；`get(id)` 返回当前剪辑名，`set(id,name)` 切换并重播，`list(id)` 列出全部剪辑；宿主每帧推进时间 |

`Animation3D` 场景组件格式：`mesh=<path>;clip=<name>;looped=true|false`，剪辑来自同一 glTF 资产的 animations。

### `animation.events` 帧事件（WP4）

`animation.events()` 返回**本帧**触发的动画帧事件数组，元素为 `{entity, name, frame}`；取走即清空
（与 `physics.events()` 同一套轮询用法）。命中判定、脚步、尘土这类"挂到具体帧上"的逻辑靠它实现：

```lua
for _, event in ipairs(animation.events()) do
  if event.name == "hit" and event.entity == player then apply_damage() end
end
```

组件串在原有五段后可选追加第六段：`clip:<名>:<表>:<列数>:<fps>:<looped>:events=<帧>:<事件名>[,<帧>:<事件名>]`。
**没有事件时不写这一段**，因此既有场景的规范字符串一字不变。

语义（都有单测）：

- **按帧到达触发、每圈每帧一次**：同一步内反复 `advance` 到同一帧不会重复触发；
- **单步跨多帧时中间的帧事件全部触发**（掉帧追帧不会漏掉命中帧）；
- **循环片段回绕后同帧事件再次触发**；回绕**跳过**的帧不算到达；
- **切换状态 = 重播**：新状态第 0 帧的事件在切换后的第一次推进就触发；
- 帧号越界、事件名为空、格式错误（`events=1`、`events=x:hit`）**一律解析报错**——静默丢弃会让
  "攻击判定帧"神秘消失，比直接报错难查得多。

### `animation.time` 播放时刻（docs/54 §4.4）

`animation.time(id)` 返回当前片段的播放时刻（秒），与 `animation3d.time(id)` 同名同义，也返回报错
（`entity has no animation`）而不是 0。它的用处是"轨道采样是**时刻 → 值**的函数"：想在脚本里按时间轴
推理（做动画工具、按时间对齐特效）需要一个可读的时钟，此前 2D 侧只能靠帧号间接猜。

引擎侧同一份时间轴由统一轨道抽象表达：`AnimationClip::tracks()` 给出一条 `SpriteRegion` 属性轨道
（Step 插值）与一条事件轨道，"帧 → UV"与"事件落在哪一步"因此各自只有一处定义（帧事件的区间语义在
`EventTrack::events_in_interval`）。详见 [docs/54](54-2D3D统一优化方案与现状对比.md) §4.4 现状。

## 第一版 API

| 模块 | API | 说明 |
|---|---|---|
| `log` | `info/warn/error` | 通过 tracing 输出脚本日志 |
| `time` | `delta` | 获取当前帧 Delta Time |
| `input` | `action/axis/pressed/released` | 键盘与鼠标输入快照 |
| `entity` | `find/spawn/is_alive/destroy` | 创建、查找、检查和销毁实体；`find(name)` 按显示名（可冲突）查 |
| `entity` | `spawn(name?, persist_id?)` / `find_id(persist_id)` / `persist_id(id)` | B-1 稳定实体 ID：`spawn` 第二参注册项目级唯一 `persist_id`，重复注册报错；`find_id` 反向取回活实体（不存在返回 nil），`persist_id(id)` 查询实体绑定的 ID；`destroy` 与切场景清理对应注册 |
| `prefab` | `spawn(path)` / `despawn(handle)` / `root(handle)` / `node(handle,name)` / `set_text(handle,name,text)` | 运行期把 `.prefab` 实体树实例化进**活世界**，是 UI 预设的运行时载体（M61，详见 [examples/game_jhyt/docs/64](../examples/game_jhyt/docs/64-江湖有田-UI预设运行时桥prefab进活世界.md)）。读取走 VFS（三宿主同一条路，直连 `fs` 在发布包里读不到）；解析末尾做完整性校验（版本行 / root 存在 / `parent` 边成树），坏预设是**加载即报错**而不是静默少一层。带 `UiLayout` 的节点会登记进宿主的 UI 布局快照，于是"画得出"与"点得中"对 spawn 的实体与场景里摆的一律成立；**带 UI 组件却没有 `UiLayout` 的实体既不会被画也不会被点中**。`UiEvents` 绑的是**全局函数名**，函数必须在 `spawn` 之前已存在，否则点击静默无派发。`despawn` 撤实体也撤布局节点（幂等：句柄未知或已拆过返回 `false`）；实例不进场景簿记，切图不会把 UI 实例当场景实体重生。`set_text` 只改 `Text` 组件的文字，字体/字号/颜色属版面（要改就改预设文件）。**尚未接**：逐预设引用 `CanvasConfig`，spawn 出来的 UI 用的是宿主当前画布配置 |
| `sprite` | `set(id, 组件串)` | 设置/替换实体 Sprite 组件（`texture=...;size=...`，与场景组件串同格式）；`texture` 支持 `图集路径#区域名` 引用，装配期解析为图集纹理 + 区域 UV（`docs/63`） |
| `transform` | `get/set/move` | 读取和修改位置、旋转、缩放 |
| `health` | `get/set/damage` | Demo 用生命值与受伤 API |
| `particles` | `burst(x,y,count)` | 在 `(x,y)` 爆发 count 个粒子（橙色，扩散速度 120、寿命 0.6s） |
| `physics` | `overlap/raycast` | **碰撞体**圆形重叠与射线查询（与 `physics3d` 同义，可选 `{layer=N,mask=N}` 分组过滤）；实体位置宽相另有 `positions_in_radius(x,y,r)` |
| `scene` | `load/reload` | 场景状态切换接口 |
| `asset` | `load/release` | 资源句柄生命周期接口 |
| `ui` | `text/text_at/show/hide/get` | 最小文本 UI 状态接口；`text_at(id,x,y,text[,scale])` 可选缩放 |
| `audio` | `play/volume/music/stop_music` | 音效播放与音量；`music(name)` 循环 BGM，`stop_music()` 停止 |
| `save` | `read/write/load/commit` | 键值存档接口（load 从磁盘载入，commit 原子写入） |
| `app` | `quit` / `project_root` | `quit()` 请求退出应用；`project_root()` 返回**工程根目录的绝对路径**（正斜杠，未配置时为空串）——`io.open` 的相对路径按进程工作目录解析，而 `--check` 不 chdir 到工程根，因此"跨文件契约"校验器（要读 `scenes/*.scene`、`project.toml`）必须靠它拼绝对路径，否则会静默跳过检查 |

## 农场 API（P0/P1 最小闭环）

以下模块用于把 2D 农场循环下沉为稳定的 Lua API。所有接口都运行在主线程共享状态中，错误转换为 Lua 错误而不是 panic。

| 模块 | API | 说明 |
|---|---|---|
| `grid` | `world_to_cell(x,y,tile_size)` / `cell_to_world(cx,cy,tile_size)` / `facing_cell(x,y,facing,tile_size)` / `in_bounds` | 世界坐标与格子坐标互转；`facing` 支持 `up/down/left/right` 及 `n/s/w/e` |
| `tilemap` | `create(width,height,tile_size[,texture][,name])` / `get(id)` / `is_solid(id,x,y)` / `get_cell(id,x,y)` / `set_cell(id,x,y,value)` / `set_solid(id,x,y,bool)` / `set_texture(id,texture)` / `get_meta(id,x,y,key)` / `set_meta(id,x,y,key,value)` / `find_path(id,fx,fy,tx,ty,diagonal?)` | 运行时创建空 Tilemap 实体（用于数据驱动的农田网格，headless 也适用），或对既有 Tilemap 写视觉格、碰撞层和每格字符串元数据；`set_meta` 空 value 删除键；格子值 `0`=空、`1..=6`=贴图集列（纹理列采样，见 `assets/textures/tileset.png`）；`find_path` 以 solid 为阻挡做 A* 四/八向寻路（可选 diagonal，禁穿角），成功返回含两端的 `{x,y}` 路径表，失败返回 `nil, reason`。**M29**：`get(id)` 的返回表新增 `texture` 字段；`set_texture(id, texture)` 运行期换整张地表贴图（**只动 `Tilemap::texture`，不动 tiles/solid/每格 meta**），渲染器每帧重读该组件、变化时重建这一层网格（下一帧生效）。空串/纯空格纹理、实体没有 Tilemap 组件都直接报 Lua 错误。典型用途：按季节换地表（`examples/game_jhyt` 的 `world/season_visual.lua`） |
| `clock` | `now` / `set_rate` / `pause` / `add` / `set` / `sleep_to_morning` | 独立游戏时钟，默认第 1 天 06:00；`set_rate(n)` 表示每秒推进 n 个游戏分钟，宿主在固定步进时调用 |
| `calendar` | `now` / `set` | 日期/季节/年（M16-08）。`now()` 返回 `absolute_day/day/season/year/minute`（`day` 为季节内第几天，1~28，28 天/季 × 4 季/年）；`set(day,season,year,minute)` 按绝对天数写入 |
| `day` | `sleep` / `now` | 每日生命周期（M16-08）。`sleep()` 推进到次日 06:00 并触发全局 `on_day_end` → `on_day_start` 回调（脚本定义了才调用）；时钟自然跨午夜时宿主也会触发 `on_day_start`。`now()` 返回 `day/minute` |
| `tag` | `get` / `set` | 给实体附加字符串标签；空 value 删除标签 |
| `interact` | `query(x,y,radius)` / `prompt(text)` / `clear_prompt` / `get_prompt` | 按距离和 `priority` 排序查询可交互实体，显示/清除交互提示 |
| `item` | `define(id,def)` / `def(id)` | 注册和查询物品定义（`name/description/stack/buy/sell/texture/kind`） |
| `inv` | `create/hotbar/capacity/exists/add/remove/count/get/slot/set/swap/move/equip/equip_get/equip_list` | 固定容量背包、堆叠与快捷栏；槽位从 1 开始。`equip(inv,slot,item_id)` 写入/清除（nil 清除）装备槽（M16-07 骨架，暂无属性效果），`equip_get` 查询单槽，`equip_list` 返回槽→物品表 |
| `container` | `open(entity_id,capacity)` / `get` / `close` / `move` | 把实体 ID 关联为箱子背包，并可与玩家背包互转 |
| `save` | `blob_write(key,value)` / `blob_read(key)` / `slot(name)` / `slot_get` / `clear()` | 任意 Lua 值（表/数字/字符串/布尔）经 `__blob:<key>` JSON 存入键值存档，可用 `slot(name)` 切换 `save_<slot>.sav`。持久化走 `commit()`（原子写盘）、读档走 `load()`（见上表）。注意（兼容层收口后）：**无 `version()`、无 `on_migrate` 迁移钩子**——`load()` 接受 `save_version=1`（旧原始行格式，原样读入、下次保存升级）与 `save_version=2`（转义行格式，写入格式），缺失版本头 / 版本 0 / 高于当前版本返回 false 且不覆盖原文件。`load()` 返回 `(ok, reason)`，`reason ∈ {"no_save_path","missing","corrupt: <err>"}`，用于区分「没有存档」与「存档损坏」。`clear()` 丢弃内存中的全部键值并返回删除条数（新游戏必须调用，否则上一局的键会带进新档） |
| `map` | `go(name,x,y)` / `enter(name,x,y)` / `current` / `spawn` / `set_spawn` / `schema(name,spec)` / `state(name[,value])` / `state_clear([name])` | 带出生点切图、当前图查询、分地图 blob 持久化；`schema` 注册分地图状态的轻量结构校验器，`schema(name,nil)` 清除；`state(name,value)` 写入前校验，错误定位到地图名与字段；`state_clear(name)` 删除单张地图状态、`state_clear()` 删除全部 `__mapstate:*`，均返回删除条数（开新游戏用） |
| `shop` | `set_catalog` / `price` | 商店目录与价格服务（M16-10）。`set_catalog(name,spec)` 注册目录（`{item_id:{buy,sell}}`），`price(name,item_id)` 返回 `{buy,sell}` 或 nil；买卖交易与资金在 Lua 玩法层完成 |
| `ui` | `panel/image/grid/grid_cell/grid_clear/grid_hit/tooltip/modal/modal_show/modal_hide/clear/drag_begin/drag_end/drag_active/drag_cancel` | 运行时动态面板、图片、背包网格、tooltip、模态层与拖拽回调 |
| `ui` | `panel_get(id)` | 读回 `ui.panel` 建的动态面板：`{x,y,width,height,r,g,b,a,visible}`；不存在返回 `nil`（M22，UI 动画/可见性的 headless 断言用） |
| `ui` | `image_get(id)` | 读回 `ui.image` 建的动态图片：`{x,y,width,height,texture,r,g,b}`；不存在返回 `nil`（M29，与 `panel_get` 同构——让"画出来的东西"可以被 headless 直接断言，例如资源条宽度 = 真实数值比例、天气图标是晴还是雨） |
| `ui` | `text_get(id)` | 读回 `ui.text_at` 建的动态文本：`{text,x,y,scale,size,visible}`；不存在返回 `nil`。`size` 是**渲染用的像素字号**（`ui.text_at` 给了 `font_size` 就是它，否则按 `scale × 12` 折算，与渲染口径一致）。用途：headless 验收读回"真正画出来的那串字"（M30 的排版审计就是逐条量它的宽度） |
| `ui` | `measure_text(text, size)` | **与渲染同口径**的文字度量：`{width, height, lines, glyphs, missing, missing_count}`。同一个工程字体、同一套 advance 与字间距（`TextStyle::default().spacing = 1.0`，即每个字符后加 1px），只做度量不做光栅化，因此 headless 也能用、且很便宜。`missing` 是字体**没有字形**的字符（例如 emoji），直接用于"缺字/豆腐块"检查；`size` 必须为正数，否则报 Lua 错误。用途：中文排版按真实字宽裁剪/换行，而不是按字数估宽（`examples/game_jhyt` 的 `ui/text_fit.lua` + `ui/ui_text.lua`，验收 `jhyt_m30_cjk_text`） |

`ui.clear(id)` 会同时删掉同 id 的动态**面板、图片、网格**（以及 `<id>:tooltip` / `<id>:title` 文本），
而 `ui.hide(id)` 只覆盖文本与面板——要让一个图片元素消失必须 `ui.clear(id)`。

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
| `audio` | `settings_path()` | 设置文件绝对路径（无工程根返回 `nil`）；**发布包形态指向用户数据目录**（`%APPDATA%\<publish_name>\settings.toml`，`RUSTLUA_USER_DIR` 可覆盖），开发工程形态为 `<project>/settings.toml`（M31 起，见 `docs/50`） |
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
# 命名动作映射（M20-07；M22/W1 起为物理键格式 v2）。单行转义字符串，首行为版本头；
# 由 `input.save_settings()` 写入、`input.load_settings()` 读取。键位是**物理键名**
# （`key:keye`/`key:digit1`/`key:arrowup`/`key:space`），与 winit `KeyCode` 变体名一一对应。
map = "rustlua-actions-v2\ninteract=key:keye,pad:0\n..."
```

- 文件**可选**：缺失时全部使用默认值（音频 `1.0`、输入用引擎默认动作表），不影响启动；当前定义 `[audio]` 与 `[input]` 两段。
- 解析宽松：未知段、未知键、非法数值一律忽略（保留默认），不 panic。
- 写入保序：只替换/补齐 `[audio]` 段内的四个键与 `[input]` 段内的 `map` 键，注释与其他段原样保留。
- **读 v1、写 v2**（M22/W1）：`rustlua-actions-v1`（`key:e`/`key:1`/`key:up`/`key:+`/`key:other:1048577`）
  仍可读入并迁移成物理键，落盘即写 `rustlua-actions-v2`；有损换算（如 `key:+` 只能落到 `=` 那个
  物理键）会记进 `input.migration_notes()` 与 `input.diagnostics()`，不静默改键位。
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

### 物理键身份（M22/W1）

- 引擎的键身份是**物理键**（键盘上那个位置），不是字符。`KeyCode` 与 winit 0.30 的
  `KeyCode` **逐项对齐（194 项）**：字母/数字/符号、方向键、`Tab`/`Enter`/`Backspace`、
  `F1..F35`、左右修饰键、小键盘运算键、媒体键、`Lang1..Lang5`/`IntlRo` 等。
- 宿主只读平台的 physical key（独立游戏窗口与 `game` 宿主走 `engine::host::keymap`，
  编辑器 Play 预览走 `editor::panels::keymap`），因此**同一物理位置在任何键盘布局下都是
  同一个引擎键**：AZERTY 的 W 位置仍是 `KeyW`，Dvorak 同理。逻辑字符不参与判定。
- 文本形式是**变体名小写**：`key:keye`（字母 E）、`key:digit1`（主键区 1）、
  `key:arrowup`、`key:space`、`key:escape`、`key:shiftleft`；解析大小写不敏感。
- Lua 侧 `input.is_down/just_pressed/just_released/pressed/released` 同时接受
  **v2 物理键名**（`"keye"`/`"arrowup"`）与 **v1 可读名**（`"e"`/`"1"`/`"up"`/`"esc"`/`"space"`）。
  v1 名称按 **US-QWERTY 位置**换算，因此是「兼容入口」而不是运行时的身份来源。
- **未知名是明确错误**，并把原值回显在错误里（旧实现把 `parse_key` 失败静默落成
  `Other(0)`，拼错的键名与真实按键无法区分）。空字符串同样报错。
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
| `input` | `bindings(action)` | 该动作的绑定文本数组，形如 `{"key:keye", "pad:0", "padaxis:1+"}`（M22/W1 起键位是物理键名） |
| `input` | `rebind(action, binding)` | **清空并重设**该动作的绑定，返回 `ok, err`；绑定文本非法时返回 `false, 原因`（原绑定保留）。键位文本 v1（`key:e`）与 v2（`key:keye`）都接受，读回总是 v2 |
| `input` | `unbind(action, binding)` | 移除单个绑定，返回是否确实移除 |
| `input` | `reset_bindings()` | 恢复引擎默认动作表 |
| `input` | `action_map_text()` | 当前映射的持久化文本（首行版本头 `rustlua-actions-v2`，逐行 `动作=绑定,绑定`） |
| `input` | `set_action_map(text)` | 用文本替换当前映射，返回 `ok, err`；**版本头不符不改动当前映射**，返回 `false, 原因`（`v1` 与 `v2` 都接受） |
| `input` | `migration_notes()` | v1 → v2 迁移说明数组（M22/W1）：无有损换算时是一条「已升级」提示，有 `key:+` 这类带 Shift 字符写法时逐条说明落到了哪个物理键；v2 文件为空表 |
| `input` | `settings_path()` | `settings.toml` 实际路径（无工程根返回 `nil`）；发布包形态指向用户数据目录，开发工程形态为工程根（M31 起） |
| `input` | `load_settings([path])` | 读取 `[input].map` 并应用（M22/W3 起**同时应用导航手感**：阈值 + 连发节奏）；文件缺失返回 `false, "missing"`（保留当前绑定）；版本/绑定非法则**回退默认映射**并记录诊断；缺 `map` 键只诊断、在位绑定保持不变；已有内容用 `merge_missing` 补齐缺失动作（**不覆盖用户重绑定**） |
| `input` | `save_settings([path])` | 保序读改写 `settings.toml` 的 `[input]` 段（原子写、保留注释与 `[audio]` 段），返回 `ok, err` |
| `input` | `diagnostics()` | 输入设置加载/保存产生的诊断消息数组（可定位到文件与原因） |
| `input` | `nav_settings([path])` | 读取 `[input]` 段的菜单导航手感，返回 `{threshold, delay_ms, repeat_ms}`；文件缺失/无工程根返回 `nil`（调用方保留默认 0.5 / 350ms / 90ms）。解析宽松 + 钳制：阈值 0..1、时长 ≤5000ms、间隔 ≥16ms。旧的 `nav_delay_frames`/`nav_repeat_frames`（M20-09）仍可读，按 60Hz 换算成毫秒（M22/W3） |
| `input` | `set_nav_settings(threshold, delay_ms, repeat_ms)` | 把导航手感推给引擎（M22/W3）：阈值成为设备层轴判定阈值，两个时长供 `action_repeat` 使用；返回钳制后的生效值 |

- 路径规则与 `audio.settings_path/load_settings/save_settings` 一致：缺省为**设置根**下的 `settings.toml`
  （开发工程 = 工程根；发布包 = 用户数据目录 `%APPDATA%\<publish_name>`，`RUSTLUA_USER_DIR` 可覆盖，
  见 `docs/50`），相对路径相对设置根解析；无工程根时无参调用返回 `false, "no_project_root"`。
- 绑定文本语法（M22/W1 起键位是**物理键名**）：
  `key:<物理键名>`（如 `key:keye`、`key:digit1`、`key:arrowup`、`key:space`、`key:shiftleft`）、
  `pad:<按钮序号>`（如 `pad:0`）、`padaxis:<轴序号><+|->`（如 `padaxis:1+`）。
  v1 的字符写法（`key:e`、`key:1`、`key:up`、`key:+`、`key:other:1048577`）仍可解析，
  按 US-QWERTY 换算成物理键后归一化为 `key:keye`/`key:digit1`/`key:arrowup`/`key:equal`。
- 默认动作表（`ActionMap::with_defaults()`，36 项）：移动 `move_up/down/left/right`；玩法
  `interact`/`attack`/`skill`/`gift`/`sleep`/`practice`/`inventory`/`pause`/`build`/`place`/`demolish`/
  `use`/`quest`/`respawn`/`seed`；音量 `volume_up`/`volume_down`；数字直选 `slot_1`…`slot_9`；
  菜单 `confirm`/`cancel`；焦点导航 `ui_up/down/left/right`。其中 `confirm`/`cancel` 与 `ui_*`
  额外带手柄默认绑定（`pad:0`/`pad:1`、左摇杆 `padaxis:0±`/`padaxis:1±`、DPad `pad:12..15`）。
  **键盘绑定一个物理键只属于一个动作**（`ActionMap::conflicts()` 无 `Key` 项）；手柄侧
  `pad:0` 同时服务 `confirm`/`interact`、左摇杆同时服务 `move_*` 与 `ui_*` 是刻意的设备别名。
  音量默认落在 `key:minus`/`key:equal`（US-QWERTY 的 `-`/`=` 位置），因此不随键盘布局漂移。
- 旧物理键 API（`input.is_down/just_pressed/just_released/axis/pressed/released`）语义**不变**
  （仍是物理键查询），但**未知名从静默变成报错**，且键名同时接受 v1/v2 两种写法；
  `input.action` 已从「物理键别名」升级为「命名动作查询」（仓库内除引擎自身测试外无原调用方）。
- `game_jhyt` 从物理键迁移到命名动作由 **M20-08** 完成；物理键身份收口由 **M22/W1** 完成，
  本任务只提供引擎层 API 与持久化。

### `world_text` 世界文字（WP4）

文字锚在实体的世界位置上（相机移动时跟随世界），与屏幕 UI 的 `Text` 是两套东西。

| 调用 | 说明 |
| --- | --- |
| `world_text.attach(id, 组件串)` | 给实体挂/覆盖世界文字。组件串：`text=..;font_size=..;color=r,g,b[;font=..][;offset=x,y][;centered=true]`。非法串返回错误且**不破坏已有组件** |
| `world_text.detach(id)` | 移除世界文字，返回是否真的移除了（重复调用返回 `false`） |

`entity.spawn` 只插入 `Transform`，因此运行期创建的实体（伤害数字、名牌）必须用 `attach`；
场景文件里也可以直接写 `WorldText` 组件。

### `camera2d` 2D 相机控制（WP4）

此前 Lua **完全无法控制 2D 相机**（`camera` 表只管 3D 的 `Camera3D` 组件）；大地图与跟随镜头
只能靠宿主代码。相机数据在宿主而不在 Lua 状态里，因此 `set`/`set_zoom` 是**登记请求、宿主下一帧
消费**；`get`/`get_zoom` 读宿主回填的真实值。

**2D 相机"用哪台"由 `camera.set_active` 决定**（批 9 起与 3D 共用一条选择链）：选中的是**正交**
相机时，宿主在选中项变化时采用它的载荷（`center`/`zoom`/`rotation`/`bounds`）作为 2D 取景基准，
之后 center/zoom 仍归 `camera2d.*` 的控制器（跟随 / 死区 / 平滑 / 抖动 / 请求）；选中的是透视
相机或什么都没选时，2D 取景**保持现状**（切 3D 相机不会把 2D 相机弹回作者值）。没有显式选中
时，初始 2D 相机是排序序第一台启用的正交相机。

| 调用 | 说明 |
| --- | --- |
| `camera2d.set(x, y)` | 设置相机中心（必须有限） |
| `camera2d.get()` | 返回 `{x, y}`（宿主实际在用的值） |
| `camera2d.set_zoom(z)` | 缩放，`z > 0` |
| `camera2d.get_zoom()` | 当前缩放 |
| `camera2d.follow(id [, opts])` | 跟随实体；`opts = {offset_x, offset_y, dead_zone_x, dead_zone_y, smoothing}`，死区必须 ≥ 0、其余必须有限 |
| `camera2d.unfollow()` | 取消跟随 |
| `camera2d.shake(amplitude, duration)` | 抖动，`duration > 0` |
| `camera2d.screen_to_world(x, y)` | 屏幕像素 → 世界坐标，返回 `{x, y}`（**数组**，与 `input.cursor_position()` 同形） |
| `camera2d.world_to_screen(x, y)` | 世界坐标 → 屏幕像素，返回 `{x, y}` |
| `camera2d.mouse_world()` | 鼠标当前位置的世界坐标（等价于对 `input.cursor_position()` 做换算） |

换算用宿主每帧回填的相机快照与 `Camera2D` **自己的**投影函数——与精灵、瓦片、世界文字走的是
同一份实现（`math2d` 里只有这一份），因此"点到哪"和"看到哪"不会错位；这也满足 docs/40 §3 WP4 ⑧
"输入统一 screen→world"。脚本在**首帧渲染之前**调用换算会明确报错，而不是拿一个默认相机去猜。

`follow` 期间相机的中心由控制器决定；`unfollow` 之后 `set` 的效果才不会被覆盖。

### `camera3d` 3D 屏↔世界投影链（WP9 / docs/54 §8-⑦）

此前 Lua 只能自己拼一条射线喂给 `physics3d.raycast`——"屏幕坐标 → 相机射线"这段投影数学
（fov / 视口 / 姿态）脚本够不着，而编辑器 3D 点选早就在用 `Camera3D::ray_from_screen`。
现在两侧共用同一条链：

| 接口 | 说明 |
| --- | --- |
| `camera3d.ray(x, y)` | 屏幕像素 → 世界射线，返回 `{ox, oy, oz, dx, dy, dz}`（可直接喂 `physics3d.raycast`） |
| `camera3d.mouse_ray()` | 鼠标当前位置的射线（等价于对 `input.cursor_position()` 做换算） |
| `camera3d.world_to_screen(x, y, z)` | 世界坐标 → 屏幕像素，返回 `{x, y, visible}`；相机背后的点 `visible = false` |
| `camera3d.get()` | 本帧生效的相机姿态与参数：`{px, py, pz, tx, ty, tz, fov, near, far, width, height}` |

宿主每帧把 **3D pass 实际用的那台相机**（Lua `camera.set_active` 选中，否则排序序第一台，
再否则默认相机）与视口回填进来，因此"点中哪个"和"画了哪个"用的是同一台相机。脚本在
**首帧渲染之前**调用这些接口会明确报错，而不是拿一台默认相机去猜。

### `particles` 发射参数（WP4）

`particles.burst(x, y, count [, opts])`。**此前速度/寿命/颜色被写死在引擎里**（120 / 0.6 / 橙色），
Lua 无法区分尘土、水花、落叶；`opts` 让每次爆发自带观感。缺省的 `opts` 与旧行为一致，
因此三参数老写法零改动。

| 字段 | 说明 |
| --- | --- |
| `speed` | 初速上限（世界单位/秒），每个粒子在 25%–100% 随机 |
| `life` | 基准寿命（秒），每个粒子在 50%–100% 随机 |
| `size` / `size_end` | 边长曲线：出生 → 消亡（世界单位）。只给 `size` 即恒定大小；`size` 加 `size_end` 可做收缩/膨胀 |
| `color` / `color_end` | 颜色曲线（`{r,g,b,a}`，缺省通道为 1）。只给 `color` 即恒定颜色 |
| `gravity_x` / `gravity_y` | 世界重力（单位/秒²） |
| `drag` | 每秒速度衰减比例，`>= 0` |
| `rotation` / `rotation_speed` | 初始弧度与每秒角速度 |

另有只读的 `particles.count()` 返回当前存活粒子数。

参数非有限、`life <= 0`、`drag < 0` 一律**报错且不生成粒子**；粒子按固定步推进（`step_particles`），
抖动/爆发用确定性 LCG，因此同一脚本序列可复现。

### `transform` 统一 2D/3D 变换（WP9）

同一个 `transform` 表按实体**实际拥有**的组件分派：有 2D `Transform` 走 2D，否则有
`Transform3D` 走 3D，两者都没有则报错。旧脚本零改动——2D 实体的取值一字不变，
`space` 只是新增字段；而此前"3D 实体调用 `transform` 直接报错"的情形现在会正常落到 3D 分支。

| API | 说明 |
| --- | --- |
| `transform.get(id)` | `{space='2d'\|'3d', x, y[, z, rx, ry, rz, rw], scale_x, scale_y[, scale_z]}`；2D 另有 `rotation`（弧度）。**局部**值 |
| `transform.world(id)` | 同形，但读**世界**值：开启空间继承（WP2）时是父链组合的结果，否则等于局部值。父子关系下这是唯一能拿到"屏幕上实际在哪"的接口 |
| `transform.set(id, x, y[, rotation, scale_x, scale_y])` | 位置必填；**3D 下只改 XY，Z 保留** |
| `transform.move(id, dx, dy)` | 2D/3D 都按 XY 增量移动 |
| `transform.space(id)` | 返回 `'2d'` / `'3d'` / `nil` |
| `transform.attach(id, '2d'\|'3d')` | 缺组件时新建并返回 `true`，已存在返回 `false`（**不覆盖**）。新建走**统一的提升/压平**：2D→3D 保留旋转（转成绕 Z 四元数）与缩放；3D→2D 只在**真能无损压平**时才建（四元数只有 z/w 分量、z 位移为 0、z 缩放为 1），否则**报错并且不插入组件**——手抄一个 XY 会把旋转与缩放悄悄丢掉 |

两处刻意报错而非猜测：**3D 下 `set` 带标量 `rotation`**（2D 的弧度与 3D 的四元数不是一回事，
静默按绕 Z 处理属于语义近似），以及未知的 `space` 名。被拒绝的调用不留半写状态。
`transform3d` / `camera` 表保持原样，作为空间专属的 facade。

### `visibility` 实体可见层（docs/54 §4.2）

`RenderView.visibility` 以前是个没有消费者的字段；现在视图侧的遮罩由宿主声明，Lua 只读写**实体侧**，
于是"脚本把东西藏起来"和"视图决定看哪些层"不会演化成两件事。判定是相交：
`实体 layers ∩ 视图 visibility != 0` 才画。

| API | 说明 |
| --- | --- |
| `visibility.get(id)` | 该实体的 `layers`；**没有组件时返回 `visibility.all()`**（4294967295），不是 0 |
| `visibility.set(id, layers)` | 写入并返回新值；缺组件时**补上**组件。负数、越界（> 4294967295）、非整数与未知实体一律**报错** |
| `visibility.all()` | 全可见常量（`u32::MAX`），用来表达"恢复默认" |

被遮掉的实体是**不存在于这个视图**（不产顶点、不占默认绘制层序号），不是"透明"。遮罩只作用于有实体
身份的内容：世界精灵/瓦片/3D Mesh 看 world 遮罩，`UiImage`/`UiText` 看 overlay 遮罩；按名字注册的画布
UI 不是实体，永远照画。缺组件 = 全可见，既有场景与脚本零改动。

### `quad` 一等矩形（docs/54 §4.3）
`Quad` 是几何（`size`/`pivot`）+ 材质（`color` + 可选 `texture`）+ 深度/排序（`z`/`layer`/`order`）
的一等声明，位置/旋转/缩放来自实体的 `Transform`。与 `sprite` 表分开是刻意的：两者语义不同
（`Quad` 没有纹理区域、不参与帧动画），脚本想要"一个纯色矩形"时不该被迫写 `texture=__white__`。

| API | 说明 |
| --- | --- |
| `quad.get(id)` | `{size_x, size_y, pivot_x, pivot_y, color_r, color_g, color_b, color_a, texture, z, component}`；缺组件或未知实体报错。`texture` 给出**实际会采样**的路径——没写贴图时是内置白纹理哨兵 `__white__` |
| `quad.set(id, "<组件串>")` | 与场景里的 `Quad` 组件串**同一套语法**（`size=120,8;color=1,0,0,0.5;texture=..;z=..;layer=..;order=..`）；拼错字段、缺 `size`、零尺寸、非法数值一律报错 |

渲染上它走与精灵完全相同的批处理路径（同一个 `RenderPlan` 通道、同一套排序/剔除/深度/可见遮罩），
因此"把血条底从 `Sprite` 改成 `Quad`"不会改变它在画面里的层次规则。

### 每实体脚本的钩子（`Script` 组件，docs/54 §4.1）

场景里给实体写 `Script  file=scripts/ai/patrol.lua` 之后，那个文件会被加载并收到两个钩子
（都可以只实现一个）：

```lua
-- scripts/ai/patrol.lua
local state = {}           -- 注意：这是**按文件**共享的（同文件的所有实体共用一份）

function on_spawn(id)      -- 实体生成时调用一次
  state[id] = { dir = 1 }
end

function on_update(id, dt) -- 每个固定步一次（在工程级 update() 之后，按实体 id 升序）
  local s = state[id]
  local t = transform.get(id)
  transform.set(id, t.x + s.dir * 60 * dt, t.y)
end
```

| 钩子 | 何时调用 | 参数 |
| --- | --- | --- |
| `on_spawn(id)` | 实体（或它所属的场景）生成时一次 | 实体 id |
| `on_update(id, dt)` | 每个固定步一次 | 实体 id、本步秒数 |

- `id` 与 `transform`/`quad`/`physics` 这些表收的实体 id 是**同一套编码**，脚本因此可以直接用
  这些表操作"自己"。
- **一个文件一份 Lua 状态、实体是实例**：同文件的所有实体共享顶层局部变量，每实体状态请写成上面
  那种以 `id` 为键的表（或放在实体组件里）。这样上万实体不会变成上万份 Lua 状态。
- 钩子名与工程级脚本**不重名**：工程级仍是 `on_create()` / `update()` / `on_destroy()`。
- 钩子里报错会带上脚本路径与钩子名，运行时会中止当步并报出来，不静默吞掉。
- `enabled=false` 的 `Script` 组件既不加载也不调用；文件缺失或编译失败在**场景加载期**报错。
- 热重载：`RuntimeScene::reload_scripts()` 同时处理工程级脚本与每实体脚本，返回
  `(状态, 重载的文件数)`；重载会清掉该文件的 Lua 状态（顶层局部变量/模块缓存），已生成过的实体
  不会重复触发 `on_spawn`。详见 [docs/19](19-lua-hot-reload.md)。

### `physics.debug_draw` collider 调试绘制（WP7）

`physics.debug_draw(enabled)` 开关 2D 碰撞体轮廓的绘制（默认关闭，返回传入的布尔值）。轮廓画在
世界内容之上、屏幕 UI 之下，动态体绿色、固定体（`;dynamic=false`）青色、`sensor` 黄色——
三者物理响应完全不同，颜色不分辨会误判。开一次即持续生效，不需要每帧调用。

3D 侧有**同名的独立开关** `physics3d.debug_draw(enabled)`：混合场景里"只想看一侧"是常态，
因此两条通道互不干扰（各自取走即清空，与 `camera2d` 的请求/消费模式一致）。3D 线框用本帧
3D pass 实际使用的那台相机投影（`picking::screen_from_world_unclipped`），因此"看到的位置"
与 `camera3d.ray` 命中的位置必然一致。球用三条正交圆环（各 16 段）、胶囊用上下圆环加 4 条
母线——**段数固定**，宿主可以据此断言顶点数。

数据源是**物理世界里碰撞体的当前姿态**（`PhysicsWorld::debug_shapes` /
`PhysicsWorld3d::debug_shapes`），不是场景里的组件串：后者只在实例化时解析一次，而动态刚体
每步都在动——调试绘制要看的正是"物理到底把它放到哪了"。球体轮廓用 24 边形（3D 为圆环）近似；
认不出的形状（胶囊/多边形）**不画**，不做静默近似。

### 物理查询的分组过滤（`layer` / `mask`）

`physics` 与 `physics3d` 的 `raycast` / `overlap` 都接受一个**可选尾参** `{ layer = N, mask = N }`：

```lua
local wall = physics.raycast(0, 0, 1, 0, 100, { layer = 2, mask = 2 })
local near = physics3d.overlap(x, y, z, 3.0, { layer = 2 })
```

缺省（或空表）= 全收，老写法零改动。只给一侧时另一侧取 `0xFF`（"我在这一层，谁都看得见"
是那一侧最常用的意图）。判定与碰撞体自身的 `layer`/`mask` 用**同一套两位测试**——查询看得见谁
和谁能撞到谁因此不会变成两条各自演化的规则。位域是 `u8`：越界（如 `layer = 256`）、负数或
非整数一律**报错**，不静默截断。

### `physics.set_ccd` / `get_ccd` 连续碰撞检测（WP7）

高速物体一步跨过薄墙（隧穿）在 2D 玩法里很常见（子弹、快速下落物）。现在两种声明方式
（**3D 侧 `physics3d.set_ccd` / `get_ccd` 与 `Collider3d` 的 `;ccd=true` 完全对称**）：

| 方式 | 写法 |
| --- | --- |
| 场景组件串 | `Collider`（3D 为 `Collider3d`）加 `;ccd=true`（默认关闭；拼错报错而不是静默当 false） |
| Lua | `physics.set_ccd(id, enabled)` 运行时开关；`physics.get_ccd(id)` 读**物理世界的真实状态**（不是引擎侧的影子记录），未知实体报错 |

Rapier 的 CCD 是**刚体**属性而不是碰撞体属性，因此一个刚体上只要有任一 `Collider` 声明
`ccd=true`，该刚体即开启连续碰撞检测（`PhysicsWorld::add_body` 的说明里写明了这一点）。
行为证据见 2D 的 `ccd_stops_a_fast_body_from_tunneling_through_a_thin_wall` 与 3D 的
`ccd_stops_a_fast_body_from_tunneling_in_3d`：同样一步跨越整堵墙的高速小球，不开 CCD 时穿过去、
开了就停在墙前。

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
