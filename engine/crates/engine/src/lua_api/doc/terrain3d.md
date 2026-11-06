# Lua 模块 `terrain3d`

> 运行期 API 面基线里本模块有 7 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

地形是**程序化高度场**，不是资产：`spec` 就是 `MeshRenderer.mesh` 里的
`__terrain__?size=240&res=96&amp=2.4&seed=7`。同一份 `spec` 既生成可见网格，也回答下面这些查询，
因此脚本摆放道具不会和画出来的地面错位。坐标单位与场景一致，`x/z` 以地形中心为原点、
范围 `[-size/2, size/2]`，`y` 是世界高度（不叠加实体 `Transform` 的平移；实体有位移时先自行加上）。

## `terrain3d.bounds(spec: String)`

返回地形高度场的世界空间 AABB，供相机取景、裁剪和粗略拾取用。

- 参数：
  - `spec`：地形 spec 字符串（`__terrain__?...`）。
- 返回：6 个数的数组 `{min_x, min_y, min_z, max_x, max_y, max_z}`。
- 失败：spec 非法（未知字段、越界、格式错误）时报 Lua 错误。
- 示例：

```lua
local minx, miny, minz, maxx, maxy, maxz =
    table.unpack(terrain3d.bounds("__terrain__?size=240&res=96&amp=2.4&seed=7"))
```

## `terrain3d.height_at(spec: String, x: f32, z: f32)`

可见地面的高度查询；返回值和网格顶点来自同一个 `TerrainSpec::height`。

- 参数：
  - `spec`：地形 spec。
  - `x, z`：世界坐标；超出地形范围时按公式**外推**（不做边界夹取），需要边界判断时先看 `bounds`。
- 返回：`y` 高度（number）。
- 失败：spec 非法、`x/z` 非有限数时报 Lua 错误。
- 示例：

```lua
local y = terrain3d.height_at(spec, 12.5, -8.0)
```

## `terrain3d.normal_at(spec: String, x: f32, z: f32)`

返回单位表面法线，用于让道具、草叶沿坡面倾斜。

- 参数：
  - `spec`：地形 spec。
  - `x, z`：世界坐标。
- 返回：三个数 `nx, ny, nz`（Lua 多返回值，不是表）。
- 失败：spec 非法或坐标非有限数时报错；退化坡度回退到 `(0, 1, 0)`。
- 示例：

```lua
local nx, ny, nz = terrain3d.normal_at(spec, 12.5, -8.0)
```

## `terrain3d.raise(spec: String, x: f32, z: f32, radius: f32, delta: f32)`

在高度场上加一个局部平滑土堆/坑，返回**新的地形 spec 字符串**；原 spec 不变。

- 参数：
  - `spec`：地形 spec。
  - `x, z`：编辑中心的世界坐标。
  - `radius`：影响半径，必须 `> 0` 且 `<= 4000`。
  - `delta`：中心高度增量；正数堆土，负数挖坑，`|delta| <= 200`。
- 返回：新的 `__terrain__?...&edit=x:z:radius:delta` 路径（String）。
- 上限：一个 spec 最多 **32** 个 edit，超出报错；edit 是**不可变 spec 数据**，不是可回写的
  高度网格。要让画面变化，把返回的字符串赋给实体的 `MeshRenderer.mesh`；只调用
  `height_at` 不会改现有网格。
- 失败：spec 非法、参数非有限、半径/增量越界、edit 已满时报错。
- 示例：

```lua
local raised = terrain3d.raise(terrain_spec, 12.0, -8.0, 6.0, 1.2)
mesh.set(terrain_entity, "mesh=" .. raised)
```

## `terrain3d.raycast(spec: String, ox: f32, oy: f32, oz: f32, dx: f32, dy: f32, dz: f32, max: f32)`

把一条射线打到高度场上，返回最近命中点；给鼠标点地面、放挖掘工具与 `camera3d.ray` 配套使用。

- 参数：
  - `spec`：地形 spec。
  - `ox, oy, oz`：射线起点（世界坐标）。
  - `dx, dy, dz`：射线方向；内部会归一化，长度不要求为 1。
  - `max`：最大距离（必须为正数）。
- 返回：命中时返回 7 个数 `{x, y, z, nx, ny, nz, distance}`；未命中返回 `nil`。
  `distance` 沿归一化方向计算。起点已在地表以下（且在地形 AABB 内）时返回起点处的命中。
- 失败：spec 非法、参数非有限数、`max <= 0` 时报 Lua 错误；方向为零向量按未命中返回 `nil`。
- 示例：

```lua
local ox, oy, oz, dx, dy, dz = table.unpack(camera3d.mouse_ray())
local hit = terrain3d.raycast(spec, ox, oy, oz, dx, dy, dz, 500.0)
if hit then
    -- hit[1..3] 世界命中点，hit[4..6] 法线，hit[7] 距离
    place_prop(hit[1], hit[2], hit[3])
end
```

## `terrain3d.slope_at(spec: String, x: f32, z: f32)`

返回坡度角（度，0 = 平地），常用于判断某点能不能筑巢/行走。

- 参数：
  - `spec`：地形 spec。
  - `x, z`：世界坐标。
- 返回：`0..90` 的角度（number）。
- 失败：spec 非法或坐标非有限数时报错。
- 示例：

```lua
if terrain3d.slope_at(spec, x, z) < 18.0 then
    -- 缓坡，可放置巢口
end
```

## `terrain3d.splat_at(spec: String, x: f32, z: f32)`

返回该点的 4 通道程序化地表权重（草 / 苔 / 落叶 / 裸土），可用于选植被、粒子颜色或
放置规则。权重归一化到和为 1，只由 `seed` 与坐标决定；**不是贴图 splatmap**。

- 参数：
  - `spec`：地形 spec。
  - `x, z`：世界坐标。
- 返回：四个数 `grass, moss, litter, bare`（Lua 多返回值，和为 1）。
- 失败：spec 非法或坐标非有限数时报错。
- 示例：

```lua
local grass, moss, litter, bare = terrain3d.splat_at(spec, x, z)
if grass > 0.45 then
    -- 草甸：可以撒草籽/放蚜虫
end
```
