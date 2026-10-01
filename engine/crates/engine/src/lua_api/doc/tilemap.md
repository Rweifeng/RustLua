# Lua 模块 `tilemap`

> 运行期 API 面基线里本模块有 10 个函数。本页由 `scripts/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

## `tilemap.create(width: usize, height: usize, tile_size: f32, texture: Option<String>, name: Option<String>)`

> 绑定：`engine/crates/engine/src/core/script/api/tilemap.rs:67`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `tilemap.find_path(id: i64, from_x: usize, from_y: usize, to_x: usize, to_y: usize, diagonal: Option<bool>)`

> 绑定：`engine/crates/engine/src/core/script/api/tilemap.rs:195`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `tilemap.get(id: i64)`

> 绑定：`engine/crates/engine/src/core/script/api/tilemap.rs:23`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `tilemap.get_cell(id: i64, x: usize, y: usize)`

> 绑定：`engine/crates/engine/src/core/script/api/tilemap.rs:111`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `tilemap.get_meta(id: i64, x: usize, y: usize, key: String)`

> 绑定：`engine/crates/engine/src/core/script/api/tilemap.rs:161`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `tilemap.is_solid(id: i64, x: usize, y: usize)`

> 绑定：`engine/crates/engine/src/core/script/api/tilemap.rs:98`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `tilemap.set_cell(id: i64, x: usize, y: usize, value: u8)`

> 绑定：`engine/crates/engine/src/core/script/api/tilemap.rs:124`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `tilemap.set_meta(id: i64, x: usize, y: usize, key: String, value: String)`

> 绑定：`engine/crates/engine/src/core/script/api/tilemap.rs:174`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `tilemap.set_solid(id: i64, x: usize, y: usize, solid: bool)`

> 绑定：`engine/crates/engine/src/core/script/api/tilemap.rs:141`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `tilemap.set_texture(id: i64, texture: String)`

> 绑定：`engine/crates/engine/src/core/script/api/tilemap.rs:45`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：
