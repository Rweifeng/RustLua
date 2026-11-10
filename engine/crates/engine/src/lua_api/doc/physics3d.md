# Lua 模块 `physics3d`

> 运行期 API 面基线里本模块有 11 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

## `physics3d.add_cells(id: i64, opts: Table)`

给实体挂上**逐格体素的碰撞体**（P3-R215）。`opts` 就是 `cells3d.build(opts)` 那张表
（`cell/x/y/z/w/h/d/solids` 七个字段同名同义），两侧吃同一份位图 ⇒ 不会出现「画出来一堵墙、
物理里没有」。位图先贪心并块成尽量少的轴对齐长方体，再作为**同一个固定刚体**上的多个 `Cuboid`
插入 ⇒ 碰撞体数量只跟着表面的块状程度走，不跟着格数走（Myrmex 尺寸 3888 格实测并到 4 个）。

- 参数：`w/h/d` 各 `1..=64`，总格数 `≤ 32768`，`cell ∈ 0.05..=64` 米；`solids` 是 0 基平铺
  下标 `x + w*(z + d*y)` 的数组（与 `CellMeshSpec:index` 同式），不许重复或越界。
- 返回：`integer` —— 并块后的碰撞体个数（给门的读数，不是格数）。
- 失败：实体不存在、字段缺失或非整数、单轴/总格/格边长越限、下标重复或出界。
  **语义是替换**：实体已有刚体时先删旧的再插新的（Lua 侧没有 physics remove 通路，
  「挖一格」的唯一做法就是重建位图），所以重复调用不会叠加碰撞体。
- **摘除是延迟的**（P3-R217）：`entity.destroy(id)` 只把实体记进待摘表，刚体要等下一次
  `step_physics` 里的 `drain_pending_physics_removals` 才统一 `remove_body` ⇒ 销毁那一帧的射线
  仍然撞得上这具巢。门用三步读数钉住「延迟」本身：`added 1 true` → `destroyed true` →
  `after_step false`（`cargo test -q -p rustlua-scripting --lib destroying_the_owner`）；
  只测最后一步的话「立刻摘」和「延迟摘」都绿。
- 上面示例里的 `mesh.set(nest_entity, path)` 会先过 `mesh3d::validate_mesh_path`：
  这张哨兵串是 P3-R218 才补进放行名单的（此前只有 `__ground_plane__`/`__terrain__`/`__water__`），
  在那之前逐格巢体在真引擎上根本建不出实体，而 lupa 桩的 `mesh.set` 是 no-op ⇒ 桩门看不见。

- `physics3d.debug_draw(true)` 会把并块后的每一块画成 Cuboid 线框（形状是普通长方体，
  与只出三角面的高度场不同）⇒ 计数对不上时有线框可看。
- 示例：
```lua
local opts = { cell = 0.5, x = -5, y = -4, z = -5, w = 21, h = 9, d = 21, solids = solids }
local path = cells3d.build(opts)          -- 渲染：__cells__ 规格串
mesh.set(nest_entity, path)               -- 同一个实体出图
physics3d.add_cells(nest_entity, opts)    -- 同一个实体挡射线与动态体
```

## `physics3d.add_heightfield(id: i64, x: f32, y: f32, z: f32, rows: u32, cols: u32, sx: f32, sy: f32, sz: f32, heights: Table)`

为实体添加一块**静态地形高度场**（P3-R55）。`heights` 行主序、`rows × cols` 个数；
局部 XZ 由 `sx/sz` 铺开，高度乘 `sy`。高度场是固定体，可被射线/动态刚体碰撞，
不参与 `physics3d.debug_draw` 的线框（不是简单形状）。

- 参数：`rows/cols` `2..=1025` 且 `heights` 长度必须等于 `rows*cols`；`sx/sy/sz > 0`。
- 返回：无。
- 失败：实体不存在/已有刚体、行数/高度数/尺度非法。
- 示例：
```lua
local heights = {}
for i = 1, 33 * 33 do heights[i] = 0 end
physics3d.add_heightfield(terrain_entity, 0, 0, 0, 33, 33, 480, 1, 480, heights)
```

## `physics3d.debug_draw(enabled: bool)`

> 绑定：`engine/crates/rustlua-physics/src/physics_lua.rs:166`（宏展开）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `physics3d.events()`

> 绑定：`engine/crates/rustlua-physics/src/physics_lua.rs:142`（宏展开）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `physics3d.get_ccd(id: i64)`

> 绑定：`engine/crates/rustlua-physics/src/physics_lua.rs:121`（宏展开）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `physics3d.get_velocity(id: i64)`

> 绑定：`engine/crates/rustlua-physics/src/physics_lua.rs:384`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `physics3d.overlap(cx: f32, cy: f32, cz: f32, radius: f32, opts: Option<Table>)`

> 绑定：`engine/crates/rustlua-physics/src/physics_lua.rs:620`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `physics3d.raycast(ox: f32, oy: f32, oz: f32, dx: f32, dy: f32, dz: f32, max_distance: f32, opts: Option<Table>)`

> 绑定：`engine/crates/rustlua-physics/src/physics_lua.rs:571`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `physics3d.set_ccd(id: i64, enabled: bool)`

> 绑定：`engine/crates/rustlua-physics/src/physics_lua.rs:103`（宏展开）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `physics3d.set_position(id: i64, x: f32, y: f32, z: f32)`

> 绑定：`engine/crates/rustlua-physics/src/physics_lua.rs:410`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `physics3d.set_velocity(((id $(, $component)+)))`

> 绑定：`engine/crates/rustlua-physics/src/physics_lua.rs:185`（宏展开）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：
