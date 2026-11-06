# Lua 模块 `water3d`

> 运行期 API 面基线里本模块有 3 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

水面是**程序化透明片**，不是水体模拟：spec 就是 `MeshRenderer.mesh` 里那条
`__water__?size=10&res=8&y=-0.33&amp=0.02&seed=3&flow=1.2`。`y` 是静水面高度，
`amp` 是涟漪幅度，`foam=0..1` 控制 UV 边缘泡沫强度（默认 `0.6`），
`flow=-4..4` 让涟漪/泡沫沿局部 +x 做视觉平流（默认 `0`，不是水体模拟；每片水的 yaw
由调用方对齐河流方向）。可选 `bed=<terrain-spec>` 把嵌套地形高度场烘进水面顶点：
嵌套字段用 `~` 代替 `&`，再配 `bed_x/bed_z/bed_yaw/bed_sx/bed_sz` 把局部顶点
映射到世界坐标；着色器据此按水深做深浅色/岸线。它仍是**逐顶点深度**，不是深度纹理
或水体模拟。水面高度查询不叠加实体 `Transform` 的平移；调用方有位移时自行相加。

## `water3d.depth_at(water_spec: String, terrain_spec: String, x: f32, z: f32)`

计算某点“水面高度 - 地形高度”，正数表示有水，负数表示露出水面的干地。

- 参数：
  - `water_spec`：水面 spec。
  - `terrain_spec`：地形 spec；两块高度场都用世界坐标 `x/z`。
  - `x, z`：世界坐标。
- 返回：水深（number，可为负）。
- 失败：任一 spec 非法或坐标非有限数时报错。
- 示例：

```lua
local depth = water3d.depth_at(water_spec, terrain_spec, 12.5, -8.0)
if depth > 0.2 then
    -- 可淹水/可涉水
end
```

## `water3d.height_at(spec: String, x: f32, z: f32)`

返回水面在该点的瞬时高度（静水面 + 低频涟漪），与水面网格顶点同源。

- 参数：
  - `spec`：水面 spec。
  - `x, z`：世界坐标。
- 返回：世界 `y` 高度（number）。
- 失败：spec 非法或坐标非有限数时报错。
- 示例：

```lua
local y = water3d.height_at(water_spec, 12.5, -8.0)
```

## `water3d.level(spec: String)`

返回静水面高度 `y`，适合做洪水线/岸线判定的基准，不含涟漪。

- 参数：
  - `spec`：水面 spec。
- 返回：静水面高度（number）。
- 失败：spec 非法时报错。
- 示例：

```lua
local level = water3d.level(water_spec)
```
