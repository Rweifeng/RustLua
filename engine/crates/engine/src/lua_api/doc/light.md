# Lua 模块 `light`

> 运行期 API 面基线里本模块有 4 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

## `light.get(id: i64)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:371`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `light.move(id: i64, x: f32, y: f32, z: f32)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs`（直写）；签名由
> `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

把**点光源**的世界坐标移到 `(x,y,z)`。方向光会报错；方向仍用 `light.set` 改。
篝火/火堆/闪电这类“光源要跟着物体走”的表现层用它，静态场景不必每帧调。

- 参数：`id` 光源实体；`x,y,z` 世界坐标。
- 返回：无。
- 失败：实体不存在/没有光源/不是 Point/坐标非有限值。
- 示例：`light.move(entity.find("FireGlow"), fx, 0.6, fz)`

## `light.set(id: i64, dx: f32, dy: f32, dz: f32, r: f32, g: f32, b: f32, intensity: f32)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:428`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `light.set_active(id: i64)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:352`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：
