# Lua 模块 `particles`

> 运行期 API 面基线里本模块有 24 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

粒子是**视觉层**：`particles.count()` 只读得到 CPU 池；`mode="gpu"` 的 burst 交给渲染器后，
Lua 侧不再逐颗可查（设备无 compute、池预算不足或宿主未接派发时整批退回 CPU，**不丢粒子**）。
Lua 的 `burst` 没有 `rate`；持续发射用 `particles.stream3d`，它和场景里声明的
`ParticleEmitter{rate>0,mode=gpu}` 落到渲染器同一条连续流，不在 CPU 逐颗计数里。
流是**显式生命周期**：同一 `key` 重复声明只更新原点/参数，`stop_stream3d` 或
`clear_streams3d` 才停止。

## `particles.burst(x: f32, y: f32, count: u32, options: Option<mlua::Table>)`

在二维世界坐标 `(x, y)` 打一簇粒子。缺省参数保持历史行为（速度 120、寿命 0.6 秒、橙色）。

- 参数：`x, y` 世界坐标；`count` 粒子数；`options` 可选表：
  `speed`、`life`、`size`、`size_end`、`color`/`color_end`（`{r,g,b,a}`）、
  `gravity_x`/`gravity_y`、`drag`、`rotation`、`rotation_speed`、`mode="cpu"|"gpu"`、`seed`、
  `area_x`/`area_y`/`area_z`（出生盒半边长，默认 0 = 全在同一点）。
- 返回：无。
- 失败：坐标非有限、`life <= 0`、负 `drag`、非法 `mode`、负 `seed`。
- 示例：`particles.burst(player.x, player.y, 12, {speed=60, life=0.8, color={r=1,g=0.6,b=0.2,a=1}})`。

## `particles.burst3d(x: f32, y: f32, z: f32, count: u32, options: Option<mlua::Table>)`

在三维世界坐标打一簇粒子；除 `burst` 的全部选项外，额外接受 `gravity_z`、`speed_z`、
`spread_z`、`buoyancy`、`flutter`/`flutter_hz`、`curve`、`curl`/`curl_scale`/`curl_hz`。z 会参与 CPU/GPU 积分并由世界通道绘制（z 非零时即使宿主没开世界深度开关，
这一帧也会自动走世界 Sprite 路径）。

- 参数：`x, y, z` 世界坐标；`count` 粒子数；`options` 同 `burst`，另加四个 z/浮力选项。
  `area_x/y/z` 是**出生位置**的盒半边长（不是初速抖动）；默认 0 时全在同一点。
  `buoyancy` 按剩余寿命比例向上推（新粒子更热），默认 0。
  `flutter`（速度，单位/秒）/`flutter_hz`（`0..=20`，Hz）让每颗粒子用自己稳定的相位摆动，
  不依赖全局风；默认 0 = 不颤动，旧行为逐位不变。
  `curve`（弧度/秒，`-12..=12`）让水平面 (x,y) 里的速度方向逐帧转一点，转角按剩余寿命
  比例衰减（新颗粒转得最快）；只转方向不改速率，累计转角有限。默认 0 = 直线。
  `curl`（速度，单位/秒，`0..=50`）叠加程序化 curl 噪声风场：取三维势函数的旋度，
  **严格无散**（只有旋转/剪切，没有源汇），相邻粒子共享方向、整片场随时间翻滚。
  `curl_scale`（`0..=4`，1/世界单位）是空间频率（涡旋尺度约 `2π/scale`），
  `curl_hz`（`0..=5`，Hz）是翻滚速度。三者默认 `0/0.3/0.5`，`curl=0` 时逐位不变。
- 返回：无。
- 失败：任一坐标非有限；`speed_z`/`spread_z`/`buoyancy`/`flutter`/`curl` 为负、
  `flutter_hz` 超出 `0..=20`、`curl` 超出 `0..=50`、`curl_scale` 超出 `0..=4`、
  `curl_hz` 超出 `0..=5`、`curve` 超出 `-12..=12`；其余同 `burst`。
- 示例：`particles.burst3d(10, 2, 18, 24, {speed=3, speed_z=8, gravity_z=-20, life=1.4, size=0.15})`。

## `particles.clear_streams3d()`

删除**全部** Lua 连续流（场景里声明的流不受影响），用于天气切换/场景重置。

- 参数：无。
- 返回：无。
- 失败：不失败。

## `particles.clear_wind_grid()`

移除网格体积风场，粒子回到只受全局风/风场/风源影响的状态（场景重置、火灭时用）。无场时调用是空操作。

- 参数：无。
- 返回：无。
- 失败：不失败。
- 示例：`particles.clear_wind_grid()`。

## `particles.clear_wind_sources()`

清空全部 4 个局部风源槽（火灭/场景重置时用）。

- 参数：无。
- 返回：无。
- 失败：不失败。

## `particles.count()`

读回 CPU 粒子池里当前存活的粒子数（GPU 池不在这里计数，见模块说明）。

- 参数：无。
- 返回：`u32`。
- 失败：不失败。
- 示例：`local n = particles.count()`。

## `particles.set_wind(x: f32, y: f32)`

设置全局风加速度（世界单位/秒²）；CPU 粒子池与 GPU burst 共用同一份风，宿主每帧把它同步给 GPU 后端。

- 参数：`x, y` 风加速度分量。
- 返回：`(ok, reason)`；非有限值返回 `(false, 原因)`，不污染池。
- 失败：NaN/Infinity。
- 示例：`particles.set_wind(18, 4)`。

## `particles.set_wind3d(x: f32, y: f32, z: f32)`

设置三分量全局风；`x/y` 与 `set_wind` 是同一份状态。`z` 同时进入 CPU 粒子池与 GPU
burst 的 compute uniform（`Params.wind_z`），因此 GPU 视觉层的三维粒子也会被侧风吹动；
三端每帧由宿主统一同步，不需要脚本自己分模式设置。

- 参数：`x, y, z` 风加速度分量。
- 返回：`(ok, reason)`；任一非有限值返回 `(false, 原因)`。
- 失败：NaN/Infinity。
- 示例：`particles.set_wind3d(18, 4, -2)`。

## `particles.set_wind_grid(nx: u32, ny: u32, nz: u32, ox: f32, oy: f32, oz: f32, cell: f32)`

建一张覆盖世界区域的三维速度/温度网格（体积风场 + 温度浮力场）。粒子每步按所在格三线性采样成加速度，
网格自身做 Jacobi 扩散、温度浮力抬升与逐格衰减。CPU 粒子池与 GPU 粒子流读**同一份场**（宿主每帧同步）。
坐标与 `burst3d` 一致：`(x, height, z)`，`height` 是世界竖直轴。装场前粒子积分逐位不变。

- 参数：`nx, ny, nz` 三轴格数（每轴 `2..=32`）；`ox, oy, oz` 网格原点世界坐标；`cell` 格边长（`(0, 1024]`）。
- 返回：`(ok, reason)`；成功 `(true, "")`，越界/非有限返回 `(false, 原因)`，不装半成品场。
- 失败：任一轴不在 `2..=32`、`cell` 非正或 `>1024`、原点非有限。
- 示例：`particles.set_wind_grid(8, 8, 8, -4, 0, -4, 1)`。

## `particles.set_wind_profile(shear: f32, gust: f32, gust_hz: f32)`

在全局风之上加**风场**：高度剪切 + 空间阵风。倍率公式为
`max(1 + shear·h + gust·sin(h·gust_hz + x·0.11), 0)`（`h` = 世界高度/坐标 y），CPU
粒子池与 GPU burst/stream 的 compute 用同一条公式；默认 `(0, 0, 0)` 时旧行为逐位不变。

- 参数：`shear` 高度剪切（`-1..=1`，每世界单位高度）；`gust` 阵风强度（`0..=4`）；
  `gust_hz` 阵风空间频率（每世界单位，`0..=20`）。
- 返回：`(ok, reason)`；越界或非有限值返回 `(false, 原因)`，旧值保留。
- 示例：`particles.set_wind_profile(0.012, 0.35, 0.35)`。

## `particles.set_wind_source(index: u32, x: f32, y: f32, z: f32, radius: f32, radial: f32, lift: f32, swirl: Option<f32>)`

设置一个**局部风源**（4 个槽，`index` 0..=3）。坐标与 `burst3d` 一致：`x/y` 是水平面
（`y` = 高度），`z` 是深度；在半径内按 `(1−d/radius)²` 衰减，`radial` 沿“源 → 粒子”
三维方向外推（负值向内），`lift` 沿世界高度方向向上推，`swirl` 绕世界高度轴切向旋转
（正=逆时针）。CPU 与 GPU 用同一公式；省略 `swirl` 等价于 `0`。

- 参数：`radius` `0..=512`（`0` 关闭该槽）；`radial`/`lift`/`swirl` `-64..=64`，世界单位/秒²。
- 返回：`(ok, reason)`；索引越界或非有限值返回 `(false, 原因)`，旧值保留。
- 示例：`particles.set_wind_source(0, 12, 0.35, -8, 7, 0.6, 6.0, 1.2)`。

## `particles.stop_stream3d(key: String)`

停止并按 key 删除一条 Lua 连续流。

- 参数：`key`。
- 返回：`true` 表示该 key 存在并被删除；`false` 表示本来就没有。
- 失败：不失败。
- 示例：`particles.stop_stream3d("rain")`。

## `particles.stream3d(key: String, x: f32, y: f32, z: f32, rate: f32, options: Option<mlua::Table>)`

声明/更新一条**持续 3D 发射器**（粒子/秒）。同一 `key` 再次调用只更新原点与参数；宿主每帧
把它同步给渲染器环形流。`rate` 必须 `0 < rate <= 10000`，选项同 `burst3d`（`life` 必填为
正数；`speed_z`/`buoyancy` 不能为负，向下的雨用 `gravity_z`）。

- 参数：`key` 1–64 个可见字符；`x,y,z` 世界坐标；`rate` 每秒粒子数；`options` 同 `burst3d`。
  `area_x/y/z` 让一个流覆盖一片体积（雨幕/雪幕），否则全部从同一点发射。
- 返回：无。
- 失败：key 非法、坐标非有限、`rate` 越界、选项非法。
- 后端没有连续流时，宿主按 `rate * dt` 近似回退到 CPU 池，而不是静默不画。
- 示例：

```lua
particles.stream3d("rain", cam.x, cam.y + 18, cam.z, 240,
    {life = 1.1, gravity_z = -18, size = 0.08,
     area_x = 40, area_z = 40,
     color = {r = 0.62, g = 0.72, b = 0.86, a = 0.35}})
```

## `particles.stream_count()`

读回当前 Lua 连续流的条数（不是 GPU 粒子数，也不是场景流条数）。

- 参数：无。
- 返回：`u32`。
- 失败：不失败。

## `particles.wind()`

读回当前全局风加速度。

- 参数：无。
- 返回：`x, y` 两个数。
- 失败：不失败。
- 示例：`local wx, wy = particles.wind()`。

## `particles.wind3d()`

读回三分量全局风（含 CPU 池的 z）。

- 参数：无。
- 返回：`x, y, z` 三个数。
- 失败：不失败。
- 示例：`local wx, wy, wz = particles.wind3d()`。

## `particles.wind_grid_add_velocity(x: u32, y: u32, z: u32, vx: f32, vy: f32, vz: f32)`

往格 `(x,y,z)` **累加**速度增量（脉冲注入，如火柱每步往上推一点）；扩散会把它抹开到邻格。

- 参数：`x, y, z` 格索引（`0..<n`）；`vx, vy, vz` 速度增量分量。
- 返回：`(ok, reason)`；越界/非有限/无场返回 `(false, 原因)`。
- 失败：格索引越界、增量非有限、未先 `set_wind_grid`。
- 示例：`particles.wind_grid_add_velocity(4, 0, 4, 0, 6, 0)`。

## `particles.wind_grid_heat(x: f32, y: f32, z: f32, value: f32)`

按**世界坐标**钉一处温度（火源灌热用）：引擎把世界点映到**最近节点**（偏差不超过半格），
点在采样域外一律拒绝——不静默落到边缘节点造出一处假热源。格号是场的内部布局（原点 + 格距），
游戏侧不该自己算。

- 参数：`x, y, z` 世界坐标（`y` 是高度）；`value` 绝对温度（有限且 `|t| <= 1e6`）。
- 返回：`(ok, reason)`；域外/非有限/无场返回 `(false, 原因)`。
- 失败：点在采样域外、温度非有限或 `|t| > 1e6`、未先 `set_wind_grid`。
- 示例：`particles.wind_grid_heat(12, 0.35, -8, 1.0)`。

## `particles.wind_grid_info()`

读回场的几何（各轴格数、原点、格距）；只读，不改场。无场返回 `nil`。

- 参数：无。
- 返回：表 `{nx, ny, nz, ox, oy, oz, cell}`；无场返回 `nil`。
- 失败：不失败。
- 示例：`local g = particles.wind_grid_info()`。

## `particles.wind_grid_params(diffusion: f32, velocity_decay: f32, temperature_decay: f32, buoyancy: f32, iterations: u32)`

配置网格模拟系数；全部有限且在范围内才整批写入，否则一个都不改。

- 参数：`diffusion` 扩散系数（`0..=0.16`，0 = 不扩散）；`velocity_decay` 速度每秒衰减比例（`0..=64`）；
  `temperature_decay` 温度每秒衰减比例（`0..=64`）；`buoyancy` 温度→上升加速度增益（`-256..=256`）；
  `iterations` 每步 Jacobi 扩散迭代次数（`1..=8`）。
- 返回：`(ok, reason)`；越界/非有限/无场返回 `(false, 原因)`，旧系数保留。
- 失败：任一项越界或非有限、未先 `set_wind_grid`。
- 示例：`particles.wind_grid_params(0.12, 0.5, 1.0, 4.0, 2)`。

## `particles.wind_grid_push(x: f32, y: f32, z: f32, vx: f32, vy: f32, vz: f32)`

按**世界坐标**往最近节点**累加**速度增量（火柱每帧往上推一点，热气与上升气流就长起来）；
与 `wind_grid_heat` 同口径：格号由引擎映射，域外一律拒绝。

- 参数：`x, y, z` 世界坐标（`y` 是高度）；`vx, vy, vz` 速度增量分量（有限）。
- 返回：`(ok, reason)`；域外/非有限/无场返回 `(false, 原因)`。
- 失败：点在采样域外、增量非有限、未先 `set_wind_grid`。
- 示例：`particles.wind_grid_push(12, 0.35, -8, 0, 2, 0)`。

## `particles.wind_grid_sample(x: f32, y: f32, z: f32)`

只读采样世界点 `(x,height,z)` 的加速度 `(ax,ay,az)`（三线性插值的速度 + 温度浮力）；不改场。
粒子积分用的就是这条采样，脚本可用它做调试或自定义受力。

- 参数：`x, y, z` 世界坐标（`y` 是高度）。
- 返回：三个数 `ax, ay, az`；无场返回 `(0, 0, 0)`。
- 失败：不失败（场外按边界格钳制采样）。
- 示例：`local ax, ay, az = particles.wind_grid_sample(0, 1, 0)`。

## `particles.wind_grid_set_temperature(x: u32, y: u32, z: u32, value: f32)`

设置格 `(x,y,z)` 的温度（绝对值，如把火源格钉在某温度）；浮力按 `buoyancy·温度` 抬升该格速度，扩散把热量抹开。

- 参数：`x, y, z` 格索引（`0..<n`）；`value` 温度（有限且 `|t| <= 1e6`）。
- 返回：`(ok, reason)`；越界/非有限/无场返回 `(false, 原因)`。
- 失败：格索引越界、温度非有限或 `|t| > 1e6`、未先 `set_wind_grid`。
- 示例：`particles.wind_grid_set_temperature(4, 0, 4, 20)`。

## `particles.wind_profile()`

读回当前风场 `(shear, gust, gust_hz)`。

- 参数：无。
- 返回：三个数。
- 失败：不失败。
- 示例：`local shear, gust, hz = particles.wind_profile()`。
