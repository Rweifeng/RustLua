# Lua 模块 `transform`

> 运行期 API 面基线里本模块有 8 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

## `transform.attach(id: i64, space: String)`

> 绑定：`engine/crates/rustlua-ecs/src/transform_lua.rs:280`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `transform.get(id: i64)`

> 绑定：`engine/crates/rustlua-ecs/src/transform_lua.rs:161`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `transform.move(id: i64, dx: f32, dy: f32)`

> 绑定：`engine/crates/rustlua-ecs/src/transform_lua.rs:409`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `transform.position(id: i64)`

> 绑定：`engine/crates/rustlua-ecs/src/transform_lua.rs:202`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

读取实体的**局部**位置，返回两个数 `x, y`，不创建 table。2D 实体走 `Transform`，
3D 实体退回 `Transform3D` 的 XY。热循环只关心位置时用它，比 `transform.get` 少一次
6/10 字段表的构造与填值。

- 参数：`id` 实体句柄。
- 返回：`x: number, y: number`。
- 失败：死句柄报 `invalid entity id: …`；两种变换都没有报 `entity has no transform (2d or 3d)`。
- 示例：`local x, y = transform.position(id)`。

## `transform.set(id: i64, x: f32, y: f32, rotation: Option<f32>, scale_x: Option<f32>, scale_y: Option<f32>)`

> 绑定：`engine/crates/rustlua-ecs/src/transform_lua.rs:357`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `transform.space(id: i64)`

> 绑定：`engine/crates/rustlua-ecs/src/transform_lua.rs:257`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `transform.world(id: i64)`

> 绑定：`engine/crates/rustlua-ecs/src/transform_lua.rs:218`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `transform.world_position(id: i64)`

> 绑定：`engine/crates/rustlua-ecs/src/transform_lua.rs:211`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

读取实体的**世界**位置，返回两个数 `x, y`，不创建 table。语义与 `transform.world` 相同：
有空间继承时是父链组合后的位置，否则等于局部位置。适合只需要屏幕/世界坐标的热循环。

- 参数：`id` 实体句柄。
- 返回：`x: number, y: number`。
- 失败：死句柄报 `invalid entity id: …`；没有任何变换报 `entity has no transform`。
- 示例：`local wx, wy = transform.world_position(id)`。
