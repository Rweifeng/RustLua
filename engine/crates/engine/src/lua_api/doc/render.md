# Lua 模块 `render`

> 运行期 API 面基线里本模块有 14 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名不要手改**；在每个小节里把 `TODO(文档槽)` 换成真实说明即可。

## `render.set_fog(density: f32)`

覆盖渲染器的距离雾密度（`RenderSettings.fog_density`）。宿主每帧读一次，只在值变化时
重建渲染设置；`density` 必须有限且在 `0..=0.05`。

- 参数：`density` 雾密度（0 关闭雾；默认场景常用 0.0015）。
- 返回：`(ok, reason)`；非法值返回 `(false, 原因)`，不改变当前值。
- 失败：NaN/Infinity 或越界。
- 示例：`render.set_fog(0.0032)`（雨天）。

## `render.set_fog_color(r: f32, g: f32, b: f32)`

覆盖渲染器的雾颜色（线性 RGB，`0..=1`）。宿主每帧读一次并应用到
`RenderSettings.fog_color`；非法值不再改当前颜色。

- 参数：`r, g, b` 各 `0..=1`。
- 返回：`(ok, reason)`。
- 失败：任一非有限或越界。
- 示例：`render.set_fog_color(0.52, 0.60, 0.70)`（雨天冷灰）。

## `render.set_cloud_coverage(coverage: f32)`

覆盖云量（0..1）：地面/水面/世界 Sprite 的云影阈值按它偏移，天空体积云密度也按它偏移。
宿主每帧读一次并写入渲染器；`0.5` 为默认覆盖度。

- 参数：`coverage` 0..1。
- 返回：`(ok, reason)`。
- 失败：非有限或越界。
- 示例：`render.set_cloud_coverage(0.92)`（暴雨）。

## `render.set_cloud_speed(speed: f32)`

覆盖云层漂移速度倍率（0..4，默认 1）。地面/水面/Sprite 云影与天空体积云共用同一速度；
宿主每帧读一次写进渲染器。

- 参数：`speed` 0..4。
- 返回：`(ok, reason)`。
- 失败：非有限或越界。
- 示例：`render.set_cloud_speed(1.8)`（大风）。

## `render.set_cloud_base(base: f32)`

覆盖云底高度（世界 y，40..200，默认 100）。天空体积云的采样区间变为
`[base, base+140]`；水面云反射的虚拟云平面也用它偏移。宿主每帧读一次写进渲染器。

- 参数：`base` 40..200。
- 返回：`(ok, reason)`。
- 失败：非有限或越界。
- 示例：`render.set_cloud_base(80)`（雨天低云）。

## `render.set_cloud_shadow_strength(strength: f32)`

覆盖云影强度（0..0.6，默认 0.26）：地面/水面/Sprite 云影按
`1 − cloud·strength` 压暗；阴雨可调大，晴天可调小。

- 参数：`strength` 0..0.6。
- 返回：`(ok, reason)`。
- 失败：非有限或越界。
- 示例：`render.set_cloud_shadow_strength(0.38)`（暴雨）。

## `render.set_flash(intensity: f32)`

设置屏幕闪电闪光（0..1）；Tonemap 在雾之后、色调映射之前叠加冷白色。脚本每帧
写 0/1 即可做闪电脉冲；`0` 关闭。

- 参数：`intensity` 0..1。
- 返回：`(ok, reason)`。
- 失败：非有限或越界。
- 示例：`render.set_flash(1.0)`（闪电帧）。

## `render.set_water_tint(r: f32, g: f32, b: f32, turbidity: f32)`

设置水体染色 rgb 与浑浊度 w（各 0..1）。水体先把基色乘 `rgb` 再按 0.65 混合；
浑浊度抬高最低不透明度，让水更“脏/不透明”。

- 参数：`r, g, b, turbidity` 各 0..1。
- 返回：`(ok, reason)`。
- 失败：任一非有限或越界。
- 示例：`render.set_water_tint(0.72, 0.76, 0.68, 0.45)`（雨后浑水）。

## `render.set_quality(level: u32)`

运行期画质档（1=低、2=中、3=高）。低档关 SSAO/SSR/接触阴影/泛光/点光阴影；
中档开这些但关神光；高档全开。宿主每帧只在档位变化时重建 `RenderSettings`。

- 参数：`level` 1..=3。
- 返回：`(ok, reason)`。
- 失败：越界。
- 示例：`render.set_quality(1)`（低配）。

## `render.set_surface_cover(snow: f32, wet: f32)`

设置地面覆盖（各 0..1）：`snow` 把地形颜色拉向冷白，`wet` 压暗地表。宿主每帧读一次；
只影响地形材质分支，不影响水/天空。

- 参数：`snow, wet` 各 0..1。
- 返回：`(ok, reason)`。
- 失败：任一非有限或越界。
- 示例：`render.set_surface_cover(0.9, 0.2)`（雪后）。

## `render.set_exposure(ev: f32)`

设置曝光偏移（EV，-4..4），叠加在 Tonemap 的 `exposure_ev` 上；用于雨天压暗、
雪天提亮等天气观感。宿主每帧读一次。

- 参数：`ev` -4..=4。
- 返回：`(ok, reason)`。
- 失败：非有限或越界。
- 示例：`render.set_exposure(-0.35)`（雨天）。

## `render.set_ripple(x: f32, z: f32)`

在世界 `(x,z)` 触发一圈水面涟漪（消费一次）：半径随时间扩到约 15，1.5 秒内淡出。
位置打包进相机 `surface.zw`，水面片元按世界距离画环；不改变水位/玩法。

- 参数：`x, z` 世界坐标。
- 返回：`(ok, reason)`。
- 失败：非有限。
- 示例：`render.set_ripple(12.0, -4.0)`（落水）。

## `render.set_post_fx(vignette: f32, grain: f32)`

设置暗角/颗粒强度（各 0..1）：暗角在 Tonemap 后按屏幕径向压暗，
颗粒叠加确定性像素噪声；`0,0` 关闭。宿主每帧读一次。

- 参数：`vignette, grain` 各 0..=1。
- 返回：`(ok, reason)`。
- 失败：非有限或越界。
- 示例：`render.set_post_fx(0.35, 0.08)`。

## `render.stats()`

返回上一帧渲染统计表：

- `draw_calls`、`vertices`、`triangles`、`set_pass_calls`；
- 首帧/无渲染时全为 0；
- 数据由宿主在每次 `Renderer::render` 后写回 `ScriptState`。
