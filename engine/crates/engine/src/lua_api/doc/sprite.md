# Lua 模块 `sprite`

> 运行期 API 面基线里本模块有 3 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

## `sprite.get(id: i64)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:370`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `sprite.set(id: i64, component: String)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:320`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `sprite.set_texture(id: i64, tex_id: i64)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:423`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

把实体的精灵贴图换成一张程序化贴图句柄：只写 `SpriteAsset` 为哨兵键 `__texture__?<tex_id>`，
尺寸/轴心/染色仍由 `sprite.set` 决定。实体需先有 Sprite 组件（由 `sprite.set` 建好），单写贴图不新建组件。

- 参数：`id`——实体 id；`tex_id`——`texture.generate` 返回的整型句柄。
- 返回：无。
- 失败：实体 id 非法或已销毁、或 `tex_id` 为负时抛错。句柄未生成或被 `free` 时画面退白兜底（不报错）。
- 示例：`local tex = texture.generate{kind='checker'}; sprite.set_texture(entity, tex)`
