# Lua 模块 `mesh`

> 运行期 API 面基线里本模块有 14 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

`mesh` 给实体挂/换 3D 网格与材质覆盖。网格路径来自 `.glb/.gltf`，或引擎哨兵
`__ground_plane__`/`__terrain__`/`__water__`/`__cells__`；材质覆盖是项目相对 `.material` 路径，
`nil`/空串表示回到 glTF 内嵌材质。绑定不校验材质文件是否存在，渲染器加载失败会
告警并回退，实体不会变不可见。

还有一条**人群（crowd）入口**：`mesh.set_batch` 把 N 份实例变换收进**一个**实体，
渲染侧按实例剔除、按同网格合成一次实例化绘制。逐个建实体是"N 万只蚂蚁"的组件与
提取开销来源——组件、ECS 存储、提取遍历都按批次摊薄。批次还可以带**程序化运动**
（`motion`）：位姿由引擎按路径每帧算，脚本只写一次静态表，于是"N 万只蚂蚁"的
每帧脚本开销是 0。

## `mesh.batch_count(id: i64)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:470`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

读这个实体上的实例批次里有多少份实例变换。实体没有批次时返回 `0`（不是 `nil`）。

- 参数：`id` 实体。
- 返回：实例数（整数）。
- 失败：实体不存在/已死亡时。
- 示例：`local n = mesh.batch_count(id)`
- 注意：这是**写进去的份数**，不是"这一帧真正画出来的份数"——被视锥剔除掉的实例
  仍然计入。渲染侧读数要看待剔除后的 draw 计数。

## `mesh.batch_lod(id: i64)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:698`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

读这个实体上挂的**远距替代网格**：`{mesh=路径, distance=阈值}`；挂了第二台阶时
再带 `far_mesh` / `far_distance`；没挂时返回 `nil`。

- 参数：`id` 实体。
- 返回：一张表 `{mesh=String, distance=number, far_mesh=String?, far_distance=number?}`，
  或 `nil`。
- 失败：实体不存在/已死亡时。
- 示例：`local lod = mesh.batch_lod(crowd)`
- 注意：读回的是**引擎烘焙时看的那一份组件**，不是脚本侧副本；`clear_batch_lod`
  之后立刻是 `nil`。挂没挂、挂在什么阈值上，脚本不需要自己记。

## `mesh.batch_planar(id: i64)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:802`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

读回实例批次的**平面反射排除档**：组件缺席（默认）或 `enabled = true` 时返回
`true`，被 `mesh.set_batch_planar(id, false)` 排除后返回 `false`。

- 参数：`id` 实体。
- 返回：`boolean`（进不进水面平面反射 pass）；实体没有实例批次时返回 `nil`。
- 失败：实体不存在/已死亡时。
- 示例：`local casts = mesh.batch_planar(crowd)`
- 注意：读回的是组件本身，不是脚本侧副本；这个开关只影响**水面平面反射 pass**
  （半分辨率镜像屏幕），主视图与方向光/点光两条阴影链都不看它。

## `mesh.batch_pose(id: i64, index: i64)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:576`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

读这个实体上的实例批次里**第 `index` 个实例此刻的位姿**（世界位置 + 绕 Y 的 yaw）。

批次带程序化运动（`mesh.set_batch` 的第五个参数）时，位姿是**引擎侧求值**的：
位置来自路径上按场景时钟推进的那一点。这一条读回用的是**同一份求值**——同一个场景
时钟、同一套规则——所以脚本读到的就是这一帧渲染器用的那个位置，不是"脚本自己再算一遍"。

- 参数：`id` 实体；`index` 实例下标（从 0 起）。
- 返回：一张表 `{x=, y=, z=, yaw=}`（世界坐标 + 绕 Y 弧度）。
- 失败：实体不存在/已死亡、`index < 0`、实体没有实例批次、或该实例没有有效变换
  （缩放为 0 等）时报错。
- 示例：`local pose = mesh.batch_pose(crowd, 0)`
- 注意：没有运动的批次里这就是静态表里的那一项；静态批次的读数在所有帧上相同。
- **yaw 的口径 = 引擎摆这个模型时用的那一个**：跟随切线（`align`）时按实例自己的
  `forward` 给（`"z"` ⇒ 模型 +Z 指向切线 ⇒ yaw = atan2(dx, dz)）；其余情况是实例表里
  写下的那个 yaw。同一份位姿不会给出两个差 90° 的 yaw 读数。

## `mesh.clear(id: i64)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:335`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

移除实体的 `MeshRenderer`，3D 网格立刻不再绘制；之后 `mesh.get` 返回 `nil`。

- 参数：`id` 实体。
- 返回：`true`。
- 失败：实体不存在/已死亡，或世界不可用时报错。
- 示例：`mesh.clear(id)`

## `mesh.clear_batch(id: i64)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:456`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

摘掉这个实体上的实例批次：下一帧起这些实例不再绘制。实体上的普通 `MeshRenderer`
不受影响（两者是两份独立的组件）。

- 参数：`id` 实体。
- 返回：`true`。
- 失败：实体不存在/已死亡时。
- 示例：`mesh.clear_batch(id)`

## `mesh.clear_batch_lod(id: i64)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:683`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

摘掉远距替代网格：下一帧起整批回到同一张网格。批次本身（`mesh.clear_batch`）与普通
`MeshRenderer` 都不受影响。

- 参数：`id` 实体。
- 返回：`true`。
- 失败：实体不存在/已死亡时。
- 示例：`mesh.clear_batch_lod(crowd)`
- 注意：没挂替代网格时也是 `true`——这是幂等摘除，不是错误。

## `mesh.get(id: i64)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:321`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

读当前网格路径；实体没有 `MeshRenderer` 时返回 `nil`，不是空串。

- 参数：`id` 实体。
- 返回：网格路径（String）或 `nil`。
- 失败：实体不存在/已死亡时。
- 示例：`local path = mesh.get(id)`

## `mesh.set(id: i64, path: String)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:283`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

给实体挂/换 3D 网格。路径可以是项目相对 `.glb/.gltf`，也可以是内置哨兵；替换不会
清掉已有的材质覆盖。

- 参数：`id` 实体；`path` 网格路径。
- 返回：`true`。
- 失败：路径不合 `MeshRenderer` 文法、实体不存在/已死亡时。
- 示例：`mesh.set(id, "__water__?size=10&res=8&y=-0.33")`

## `mesh.set_batch(id: i64, path: String, instances: mlua::Value, material: Option<String>, motion: Option<mlua::Table>)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:426`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

把 N 份实例变换收进**一个**实体：渲染侧逐实例做视锥剔除，把留在视野里的那些合成
**一次实例化绘制**（同一个网格的多个批次也会合到同一次绘制里）。这是"N 万只蚂蚁"
这类重复几何的入口——逐个建实体的话，组件、ECS 存储、提取遍历都是 N 份。

- 参数：
  - `id` 实体（只作**批次的宿主**；它自己的 `Transform3D` 不参与摆放）；
  - `path` 网格路径，语法与 `mesh.set` 相同（含 `__ground_plane__` 等哨兵）；
  - `instances` 实例数组，每项是一张表：`{x=,y=,z=}` 位置（默认 0）、`{sx=,sy=,sz=}`
    缩放（默认 1）、`yaw=` 绕 Y 轴弧度**或** `q={x=,y=,z=,w=}` 四元数（`w` 默认 1）；
    **也可以**直接给一串**打包字节**（见下「打包实例」），N 很大时免掉逐只建表；
  - `material` 项目相对 `.material` 材质覆盖，或 `nil`/空串用 glTF 内嵌材质；
  - `motion` **可选**的程序化运动表，或 `nil`（默认，静态批次）。
- 返回：`true`。
- 失败：实体不存在/已死亡，或某项实例表里的数不是有限值时（报
  `mesh.set_batch instance #N: …`）；网格路径本身**不校验**，加载失败只告警并回退。
- 示例：
  ```lua
  mesh.set_batch(crowd, "assets/models/ant.glb", {
    { x = 0.0, y = 0.0, z = 0.0, yaw = 0.5 },
    { x = 1.2, y = 0.0, z = 0.3, sx = 0.8, sy = 0.8, sz = 0.8 },
  })
  ```
- 注意：
  - 实例坐标是**世界坐标**——批次实体自身的变换不参与摆放。
  - 可见性按**批次实体**判（实体上没有 `Visibility` 组件即视为可见），批次内不逐实例判。
  - 同一实体上的普通 `MeshRenderer` 与批次是两份独立组件：两者都在就都会画。
  - 材质是 alpha 混合、或网格带骨骼（palette）时，实例化合批不生效，渲染器退回逐个绘制
    （画面仍正确，只是 draw 数不降）。
  - 缩放为 0 的实例**不报错**，但也不进矩阵表（渲染时被跳过）；`mesh.batch_count` 仍把它算进去。

### 打包实例（`instances` 传字符串）

N 很大时，逐只建 Lua 表再逐键取值是主要开销（Myrmex 实测 3.0 µs/只，见
`docs/02-balance.md` §21.517；换成打包口径后 2.84 µs/只，§21.519）。`instances` 也可以是一串**字节**：每实例 **28 字节**、
七个**小端 `f32`**，顺序 `x, y, z, sx, sy, sz, yaw`（`yaw` 与表口径同一个绕 +Y 的弧度，
缩放 0 的处理也与表口径一致）。

- 长度必须是 28 的整数倍，否则报 `mesh.set_batch packed: … not a multiple of 28`；
- 每个字段必须是有限数，否则报 `mesh.set_batch packed: instance #N: <字段名> is not a finite f32`（**不静默跳过**）；
- 打包形态**不带** `motion`（位置就在字节里），两者同时给会报错；
- 空串 = 空批次（与空表同义，实体保留）。

```lua
local parts = {}
for i = 1, n do
  parts[i] = string.pack("<fffffff", x, y, z, s, s, s, yaw)
end
mesh.set_batch(crowd, "assets/models/ant.glb", table.concat(parts))
```

### 程序化运动（`motion`）

`motion` 让**每帧的 N 份位姿由引擎算**，脚本只写一次静态表。这是"N 万只蚂蚁"能上
帧率的关键：逐帧在 Lua 里重建 N 份表是 O(N) 的脚本开销，而位姿本身是
"路径上的某一点"——这条规则可以在引擎侧对每份实例各算一次，脚本每帧开销为 0。

`motion` 表的字段：

| 字段 | 含义 |
| --- | --- |
| `kind` | 运动类型，目前只认 `"path"`（省略即 `"path"`）。 |
| `path` | 折线点表，每点 `{x=,y=,z=}` 三个数都必须给且有限；至少两点。 |
| `closed` | 路径是否首尾相接（默认 `false`）。闭合路径绕回，不会在终点停住。 |
| `speed` | 批次级速度，世界单位/秒（默认 `1`）。 |

路径按**水平弧长**参数化（忽略 Y）：实例走在路径上离起点的弧长为
`offset + 实例速度 × 批次速度 × 场景时间`。每份实例可以在自己的表里加三个字段：

| 实例字段 | 含义 |
| --- | --- |
| `s` | 该实例在路径上的起始弧长偏移（默认 `0`）——用它把 N 只蚂蚁错开。 |
| `speed` | 该实例的速度倍率（默认 `1`）。 |
| `align` | 姿态是否跟随路径的**水平切线**（默认 `false`）。 |
| `forward` | `align` 时模型的前向轴：`"x"`（默认，模型 +X 指向行进方向）或 `"z"`（模型 +Z 指向行进方向）。写别的名字直接报错。 |

姿态用"绕 +Y 的 yaw"表达，而 yaw 落到哪根轴上取决于**模型自己的约定**：绕 +Y 转 θ 时
模型 +X → 世界 (cos θ, 0, −sin θ)、+Z → (sin θ, 0, cos θ)。同一段路径，`"x"` 与 `"z"`
两种口径恒差 90°——引擎不替模型猜，所以这一项由脚本写明。

- 给了 `motion` 时，实例表里**不许**再写 `x`/`y`/`z`（位置由路径决定）：写了直接报错，
  而不是悄悄丢掉一份——丢掉的那份会让"我明明设了位置"变成一句无法解释的假话。
- 路径不合法（点数不足、含非有限值）时批次退回**静态表**：画出来的东西在原地，
  而不是画到某个错误的位置。
- 场景时钟由宿主推进，与渲染提取、脚本读回（`mesh.batch_pose`）是**同一个时刻**。

示例：41 只蚂蚁沿 41 条闭合圆轨道各走各的（脚本每帧零开销）：

```lua
mesh.set_batch(crowd, "assets/models/ant.glb", instances, nil, {
  kind = "path",
  closed = true,
  speed = 3.0,
  path = ring_points,          -- 一圈折线点
})
-- instances 里每项只写 { s = 偏移, align = true, forward = "z", sx=, sy=, sz= }
```

## `mesh.set_batch_lod(id: i64, path: String, distance: f32, far_path: Option<String>, far_distance: Option<f32>)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:650`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

给实例批次挂**远距替代网格链**：离相机超过 `distance` 米的实例改画 `path`；再给
`far_path` / `far_distance` 时，超过更远阈值的实例改画 `far_path`——近/中/远三段。

只在 GPU 位姿档生效（`--gpu-motion` 且批次带运动）：近/中/远各烘焙一次、各一条 draw，
阈值判定在压缩烘焙的 kernel 里按实例位置算，三段**互斥且穷尽**（三段之和 = 原本的
存活集，不会多画也不会漏画）。只给第一台阶时退化成两段（与旧行为逐字节一致）；
关档或批次不带运动时这一档不生效，整批画批次网格。

- 参数：
  - `id` 实体；`path` 中距替代网格；`distance` 中距阈值（米，必须有限且 > 0）；
  - `far_path` / `far_distance` 可选第二台阶：必须**成对给出**，`far_distance`
    有限且严格大于 `distance`。
- 返回：`true`。
- 失败：实体不存在/已死亡、实体没有实例批次、`distance` 非有限或 ≤ 0、`path` 为空、
  `path` 与批次网格相同、只给远路径或只给远阈值、`far_distance ≤ distance`、
  `far_path` 为空、`far_path` 与中距/批次网格相同（任何一条都等于"两侧画同一张脸"，
  是静默空档）时报错。
- 示例：
  - 两段：`mesh.set_batch_lod(crowd, "assets/models/ant_low.glb", 25.0)`
  - 三段：`mesh.set_batch_lod(crowd, "assets/models/ant_low.glb", 25.0, "assets/models/ant_lowest.glb", 60.0)`
- 注意：代价是每批多一次烘焙 dispatch 与一条 draw（三档是两条）；密度档
  （`--gpu-motion-thin`）仍先于 LOD 判定——被密度档剔掉的实例三段都不画。

## `mesh.set_batch_planar(id: i64, enabled: bool)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:778`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

把实例批次**排除出水面平面反射 pass**：`enabled = false` 后这一批不再参与水面
镜像屏幕的第二遍绘制；主视图、方向光阴影、点光阴影照旧画它。

- 参数：
  - `id` 实体（必须已有 `mesh.set_batch` 建的批次）；
  - `enabled` `false` = 不进反射；`true` = 恢复（等同组件缺席）。
- 返回：`true`。
- 失败：实体不存在/已死亡、实体没有实例批次（静默插组件会让下一帧什么都没发生，
  调用方却以为生效了）。
- 示例：
  ```lua
  -- 6 万只的 crowd 在反射图里只占几个像素，却要为整批几何再跑一遍顶点与像素
  mesh.set_batch_planar(crowd, false)
  ```
- 注意：反射图是半分辨率的镜像屏幕，远处小实例在里面的可见面积很小；这是"视觉上
  换帧时间"的显式取舍，不是无损优化。开关按**实体**生效：静态合批组里只有全组都
  被排除时整组才跳过。计数落在帧剖面的 `cull.planar_skipped`（本帧被排除档挡掉的
  draw 数），与镜像视锥剔除（`RenderFrameMetrics::planar_draws_culled`）分开记。

## `mesh.set_batch_range(id: i64, start: i64, instances: mlua::Value)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:614`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

就地替换批次里从 `start` 起的一段实例：只解析被改的那 K 份、只动这一段，**数量与
顺序都不变**，其余实例原样保留。`mesh.set_batch` 每次都要重建整批 N 份；"每帧只改
几只蚂蚁"用这个入口，脚本开销是 O(K) 而不是 O(N)。

- 参数：
  - `id` 实体（必须已有 `mesh.set_batch` 建的批次）；
  - `start` 起始下标（**0 起**，与 Lua 表下标不同）；
  - `instances` 新的一段，口径与 `mesh.set_batch` 相同（表或打包字符串）。
- 返回：被替换的实例个数。
- 失败：
  - 实体不存在/已死亡、实体没有实例批次；
  - `start < 0`；
  - `start + #instances` 超出已有实例数——局部更新**不扩批**（加实例请重建批次，
    否则"第 k 个"的指代会漂移）；
  - 批次带 `motion`（位置来自路径，改实例表不会改变画出来的姿态）。
- 示例：
  ```lua
  -- 只把第 3 只挪到 (1.5, 0, -2) 并转 0.5 弧度
  mesh.set_batch_range(crowd, 3, {
    { x = 1.5, y = 0.0, z = -2.0, yaw = 0.5 },
  })
  ```
- 注意：它省的是"没被改到的 N−K 份"的脚本与分配开销，不改变"被改的 K 份仍由 Lua
  逐个给出"这一事实；每帧 N 只全要动时整批打包（见「打包实例」）或把运动搬进引擎
  （见「程序化运动」）才是正确形状。

## `mesh.set_material(id: i64, path: Option<String>)`

> 绑定：`engine/crates/rustlua-render/src/renderable_lua.rs:299`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

给已有 `MeshRenderer` 的实体设置 `.material` 覆盖；这是 X-ray/半透明外壳一类
逐实例观感的入口。传 `nil` 或空串恢复 glTF 内嵌材质。

- 参数：`id` 实体；`path` 项目相对 `.material` 路径，或 `nil`。
- 返回：`true`。
- 失败：实体没有 `MeshRenderer`（不能只改材质而没有网格），或实体不存在/已死亡时。
- 示例：
  ```lua
  mesh.set_material(nest, "assets/materials/nest_xray.material")
  mesh.set_material(nest, nil) -- 回退
  ```
- 注意：渲染器在首帧加载材质；路径写错只告警并回退，不报错。
