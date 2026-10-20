# Lua 模块 `audio`

> 运行期 API 面基线里本模块有 17 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

## `audio.apply_settings(text: String)`

> 绑定：`engine/crates/rustlua-audio/src/lua.rs:150`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `audio.bus_volume(bus: String)`

> 绑定：`engine/crates/rustlua-audio/src/lua.rs:112`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `audio.buses()`

> 绑定：`engine/crates/rustlua-audio/src/lua.rs:90`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `audio.effective_volume(bus: String)`

> 绑定：`engine/crates/rustlua-audio/src/lua.rs:121`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `audio.listener()`

> 绑定：`engine/crates/rustlua-audio/src/lua.rs:257`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

读回当前的听者位姿。

- 参数：无。
- 返回：`{x, y, z, fx, fy, fz}`（位置 + 前方向）；宿主还没接线时返回**空表**。
- 失败：不失败。
- 用途：调试"声音为什么偏了"时看宿主灌进来的到底是什么；也可给 HUD 画一个听者朝向标。
- 示例：`local l = audio.listener(); if #l == 0 then log("听者还没接线") end`

## `audio.load_settings()`

> 绑定：`engine/crates/rustlua-audio/src/lua.rs:174`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `audio.music(name: String)`

> 绑定：`engine/crates/rustlua-audio/src/lua.rs:201`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `audio.play(name: String)`

> 绑定：`engine/crates/rustlua-audio/src/lua.rs:69`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `audio.play_at(name: String, x: f32, y: f32, z: Option<f32>)`

> 绑定：`engine/crates/rustlua-audio/src/lua.rs:226`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

在世界坐标处播放一次音效（SFX 总线），按听者位姿算距离衰减与左右声像。2D 工程省略 `z`（视作 0）。

- 参数：`name` 同 `audio.play`（不带扩展名，按 `mp3 → wav → ogg` 找工作区音频目录）；
  `x, y, z` 声源的世界坐标。
- 返回：`ok, gain, pan` 三个值。`ok` 与 `audio.play` 同口径（"这次播放发生了没有"），
  后两个是引擎算出的增益（0..1）与声像（-1 全左、+1 全右）——要做"越远越轻"的自定义淡出时读它们。
- 失败：宿主还没接线（听者缺失）、参数非有限值、模型非法时 `ok=false` 并记一条 warn；
  缺文件同样是 `ok=false`。**不会**静默按原点衰减。
- 注意：听者由宿主每帧从活动相机灌入，正常游戏不需要自己设；纯逻辑用例才用 `audio.set_listener`。
- 示例：`local ok, gain, pan = audio.play_at("sfx_campfire", 12, 3)`（2D：`x=12, y=3`，
  `y` 参与距离、`x` 决定左右）。

## `audio.play_on(bus: String, name: String)`

> 绑定：`engine/crates/rustlua-audio/src/lua.rs:130`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `audio.save_settings()`

> 绑定：`engine/crates/rustlua-audio/src/lua.rs:189`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `audio.set_bus_volume(bus: String, level: f32)`

> 绑定：`engine/crates/rustlua-audio/src/lua.rs:102`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `audio.set_listener(x: f32, y: f32, z: f32, fx: f32, fy: f32, fz: f32)`

> 绑定：`engine/crates/rustlua-audio/src/lua.rs:243`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

手动设定听者位姿（位置 + 前方向），上方向固定为 `+Y`。

- 参数：`x, y, z` 听者位置；`fx, fy, fz` 前方向（不必归一化，不能为零、不能与 `+Y` 平行）。
- 返回：无。
- 失败：当场**不**校验——非法值（NaN、零向量、与前方向平行）会在下一次 `audio.play_at`
  时由 `spatial::evaluate` 统一拒绝（`ok=false` + 一条 warn）。校验只留一处，避免两套口径。
- 用途：**宿主每帧已经从活动相机灌入听者**，所以正常游戏不该调它。它给的是两种场合：
  ① 没有相机的纯逻辑用例（测试、菜单音效）；② 想做"比相机更靠前"的调试视角。
- 示例（2D，听者固定在屏幕中心、朝 +Y 俯视）：
  `audio.set_listener(160, 90, 0, 0, 1, 0)`。

## `audio.settings_path()`

> 绑定：`engine/crates/rustlua-audio/src/lua.rs:163`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `audio.settings_text()`

> 绑定：`engine/crates/rustlua-audio/src/lua.rs:144`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `audio.stop_music()`

> 绑定：`engine/crates/rustlua-audio/src/lua.rs:212`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `audio.volume(level: f32)`

> 绑定：`engine/crates/rustlua-audio/src/lua.rs:80`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：
