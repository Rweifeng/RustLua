# Lua 模块 `inv`

> 运行期 API 面基线里本模块有 15 个函数。本页由 `scripts/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

## `inv.add(id: String, item_id: String, count: i64)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/inventory.rs:133`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `inv.capacity(id: String)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/inventory.rs:113`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `inv.count(id: String, item_id: String)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/inventory.rs:177`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `inv.create(id: String, capacity: Option<i64>)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/inventory.rs:83`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `inv.equip(id: String, slot: String, item_id: Option<String>)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/inventory.rs:301`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `inv.equip_get(id: String, slot: String)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/inventory.rs:320`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `inv.equip_list(id: String)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/inventory.rs:331`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `inv.exists(id: String)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/inventory.rs:126`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `inv.get(id: String)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/inventory.rs:189`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `inv.hotbar(id: String, size: Option<i64>)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/inventory.rs:98`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `inv.move(from_id: String, from_index: i64, to_id: String, to_index: Option<i64>, count: Option<i64>)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/inventory.rs:276`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `inv.remove(id: String, item_id: String, count: i64)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/inventory.rs:163`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `inv.set(id: String, index: i64, item_id: Option<String>, count: Option<i64>)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/inventory.rs:242`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `inv.slot(id: String, index: i64)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/inventory.rs:217`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `inv.swap(from_id: String, from_index: i64, to_id: String, to_index: i64)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/inventory.rs:267`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：
