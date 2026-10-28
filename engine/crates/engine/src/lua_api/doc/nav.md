# Lua 模块 `nav`

> 运行期 API 面基线里本模块有 6 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

# Lua 模块 `nav`

> 运行期 API 面基线里本模块有 6 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保持） -->

导航网格：在 XZ 平面上做 A* 寻路、视线平滑与路径跟随查询。网格按**名字**登记，
热重载重建 VM 后用同名 `nav.create_grid` 会替换旧网格，不会每次 reload 漏一张。
格子 `(x, y)` 的 `y` 对应 3D 战场的世界 `z`；`find_world_path` 返回的 `{x=, z=}`
可以直接喂给 `transform3d.set`。本模块只算路径，不移动实体——移动节奏由游戏脚本
或 `rustlua_navigation::NavAgent` 决定。

```lua
local grid = nav.create_grid("arena", 18, 30, 1.0, -9.0, -15.0)
nav.set_blocked(grid, 8, 4, true)                       -- 塔基不可走
local waypoints = nav.find_world_path(grid, -7.5, 12.5, 0.5, -12.5, true)
for _, point in ipairs(waypoints) do
    -- transform3d.set(unit, point.x, 0, point.z, ...)
end
```

## `nav.create_grid(name: String, width: i64, height: i64, cell_size: f64, origin_x: f64, origin_z: f64)`

> 绑定：`engine/crates/rustlua-gameplay/src/nav_lua.rs:124`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

创建（或按名字替换）一张全可走的矩形导航网格，返回 1 基网格 id。

- 参数：
  - `name`：非空标识；同名再次调用会**原地替换**网格内容并返回同一个 id。
  - `width` / `height`：格子数，必须 ≥ 1。
  - `cell_size`：每格边长（世界单位），必须为有限正数。
  - `origin_x` / `origin_z`：网格左下角的世界坐标；格心 = `origin + (i + 0.5) * cell_size`。
- 返回：`id: integer`。
- 失败：名字为空、尺寸为 0、`cell_size` 非有限/非正、原点非有限时抛 Lua 错误。
- 示例：`local arena = nav.create_grid("arena", 18, 30, 1.0, -9.0, -15.0)`

## `nav.destroy_grid(id: i64)`

> 绑定：`engine/crates/rustlua-gameplay/src/nav_lua.rs:143`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

销毁一张网格并释放它占用的槽位。

- 参数：`id`：`create_grid` 返回的 id。
- 返回：`true` 表示这次真的销毁了；id 已失效时返回 `false`。
- 失败：`id <= 0` 时报错；未知 id 返回 `false`（幂等）。
- 示例：`assert(nav.destroy_grid(arena))`

## `nav.find_path(id: i64, sx: i64, sy: i64, gx: i64, gy: i64, diagonal: Option<bool>)`

> 绑定：`engine/crates/rustlua-gameplay/src/nav_lua.rs:176`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

在格坐标上跑 A*，返回**未平滑**的格子路径（含起点与终点）。

- 参数：
  - `id`：网格 id。
  - `sx, sy` / `gx, gy`：起点/终点格坐标，必须在界内且可走。
  - `diagonal`：可选，默认 `false`；为 `true` 时允许八向移动，且**不穿墙角**
    （对角步要求两个正交邻格都可走）。
- 返回：数组 `{ {x=, y=}, … }`；起点等于终点时返回单元素数组。
- 失败：id 未知、坐标越界、端点被阻挡、无路可达时抛 Lua 错误
  （错误串分别含 `unknown navigation grid id` / `out of bounds` / `blocked` / `no route`）。
- 示例：`local cells = nav.find_path(arena, 0, 0, 17, 29, true)`

## `nav.find_world_path(id: i64, sx: f64, sz: f64, gx: f64, gz: f64, diagonal: Option<bool>)`

> 绑定：`engine/crates/rustlua-gameplay/src/nav_lua.rs:195`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

世界坐标版本的寻路：先换算到格、跑 A*，再用视线检测做**字符串拉直**，
返回可直接用于 3D 移动的路径点。

- 参数：`id`；起点世界坐标 `sx, sz`；终点世界坐标 `gx, gz`；`diagonal` 同上。
- 返回：数组 `{ {x=世界x, z=世界z}, … }`，首尾是起点/终点所在格的**格心**（不是原始浮点输入）。
  每次转折都经过 `line_of_sight` 校验，平滑不会制造穿墙捷径。
- 失败：同 `find_path`；非有限世界坐标报 `world point must be finite`。
- 示例：`local points = nav.find_world_path(arena, -7.5, 12.5, 0.5, -12.5, true)`

## `nav.grid_info(id: i64)`

> 绑定：`engine/crates/rustlua-gameplay/src/nav_lua.rs:149`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

读取网格尺寸与原点，供脚本做坐标换算或自检。

- 参数：`id`：网格 id。
- 返回：`{ width=, height=, cell_size=, origin_x=, origin_z= }`。
- 失败：id 未知时报 `unknown navigation grid id`。
- 示例：`local info = nav.grid_info(arena); assert(info.width == 18)`

## `nav.set_blocked(id: i64, x: i64, y: i64, blocked: bool)`

> 绑定：`engine/crates/rustlua-gameplay/src/nav_lua.rs:165`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

标记单格是否阻挡。塔基、静态障碍在战斗开始前写成 `true`；单位不占用导航格
（单位间避让由移动层处理）。

- 参数：`id`；格坐标 `x, y`；`blocked = true` 阻挡、`false` 恢复可走。
- 返回：无。
- 失败：id 未知或格坐标越界（错误串含 `out of bounds`）。
- 示例：`nav.set_blocked(arena, 8, 4, true)`
