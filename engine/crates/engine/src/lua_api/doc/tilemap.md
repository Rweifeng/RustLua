# Lua 模块 `tilemap`

> 运行期 API 面基线里本模块有 17 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

## `tilemap.atlas_index(id: i64, x: usize, y: usize)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/tilemap.rs:119`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

读出一格**这一帧实际采样图集里第几个条目**。开启逻辑瓦解算后，格子里存的 kind 与画出来用的
条目是两回事，而解算发生在渲染器内部；这个入口让门与脚本能断言"条目序列"而不必去读像素。
它走的就是渲染器用的那一份解算（`Tilemap::resolve_atlas_index`），所以两条路不会各说各话。

- 参数：`id` — 挂 `Tilemap` 的实体；`x`/`y` — 格坐标，0 基，地图 row 0 在**最底**。
- 返回：`i64` 条目号，**1 基**（`0` 表示这一格不画）。空格、kind 超出 `tile_kinds`、解算结果
  超出图集容量、坐标越界，四种情况都返回 `0`——画不出来宁可空着，也不要画成邻居的美术。
- 失败：`invalid entity id: <id>`、`entity has no tilemap`。坐标越界**不**报错，返回 0。
- 示例：

```lua
-- 恒等档（autotile=0 且 variant_count<=1）里格子值就是条目号
assert(tilemap.atlas_index(ground, 3, 4) == tilemap.get_cell(ground, 3, 4))
```

## `tilemap.autotile_mask(id: i64, x: usize, y: usize)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/tilemap.rs:133`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

4-bit 邻居掩码：上下左右四个方向"有没有同种邻居"，解算成过渡块的槽位序号。位序按 **index**
而不是按方位命名——地图的 row 0 在**最底**、图集的 row 0 在**最顶**，同一句"上邻居"在两个
坐标系里指向相反方向，按方位命名必然错位。只有**同 kind 的非零**格算邻居；坐标越界当作
"没有邻居"（地图边界是暴露的边，不是内部瓦）。

- 参数：`id` — 挂 `Tilemap` 的实体；`x`/`y` — 格坐标。
- 返回：`i64` 0..=15。位 0 = `y-1`、位 1 = `x+1`、位 2 = `y+1`、位 3 = `x-1`。
- 失败：`invalid entity id: <id>`、`entity has no tilemap`。
- 示例：

```lua
-- 一块孤立土格的左上角：只有 +x 与 +y 两个邻居
assert(tilemap.autotile_mask(dirt, 2, 3) == 0b0011)
```

## `tilemap.clear_tint(id: i64, x: usize, y: usize)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/tilemap.rs:176`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

把一格退回**不染色**，即顶点色 `[1,1,1,1]`（乘法单位元）。阴影撤掉、季节切回常色走这条，
不要写成"涂一个白色"——后者会在调色表里多占一个条目，还把 alpha 一起改了。

- 参数：`id` — 挂 `Tilemap` 的实体；`x`/`y` — 格坐标。
- 返回：无。相邻格的染色不受影响；调色表里的条目**不回收**（回收要重编号所有引用，收益不值）。
- 失败：`tile cell (<x>, <y>) is out of bounds`、`invalid entity id: <id>`、`entity has no tilemap`。
- 示例：

```lua
for _, cell in ipairs(shadow_cells) do
    tilemap.clear_tint(ground, cell[1], cell[2])
end
```

## `tilemap.create(width: usize, height: usize, tile_size: f32, texture: Option<String>, name: Option<String>)`

> 绑定：`engine/crates/rustlua-physics/src/tilemap_lua.rs:399`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `tilemap.find_path(id: i64, from_x: usize, from_y: usize, to_x: usize, to_y: usize, diagonal: Option<bool>)`

> 绑定：`engine/crates/rustlua-physics/src/tilemap_lua.rs:552`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `tilemap.get(id: i64)`

> 绑定：`engine/crates/rustlua-physics/src/tilemap_lua.rs:157`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `tilemap.get_cell(id: i64, x: usize, y: usize)`

> 绑定：`engine/crates/rustlua-physics/src/tilemap_lua.rs:451`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `tilemap.get_meta(id: i64, x: usize, y: usize, key: String)`

> 绑定：`engine/crates/rustlua-physics/src/tilemap_lua.rs:511`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `tilemap.is_solid(id: i64, x: usize, y: usize)`

> 绑定：`engine/crates/rustlua-physics/src/tilemap_lua.rs:434`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `tilemap.set_atlas(id: i64, cols: usize, rows: usize)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/tilemap.rs:59`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

声明这张图集**有几列几行**——条目 `n`（1 基）取 `col=(n-1)%cols`、`row=(n-1)//cols` 的子矩形。
列数刻意**显式声明而不从贴图尺寸反推**：反推法让"贴图里有几列"变成美术文件的隐式契约，
换一张 PNG（哪怕只是重新导出改了尺寸）就静默重排整张地图，症状是"某天马的地看起来全乱了"
且数据一个字节没变。写完立刻做布局自洽校验，超出容量当场报错而不是画错。

- 参数：`id` — 挂 `Tilemap` 的实体；`cols`/`rows` — 图集列数与行数，都至少 1。
- 返回：无。改动会进组件串并被序列化，编辑器、运行时与 `tools/preview_scene.py` 读同一个值。
- 失败：`tile map must have a non-empty atlas grid (cols x rows)`、
  `atlas layout needs <N> entries but the tileset has only <C> (<cols>x<rows>)`、
  `tile cell index <n> is out of atlas range`（已有格子落在新布局之外）。
- 示例：

```lua
-- 12 列 × 8 行：一张图能摆 96 个条目，够 4 种瓦各 16 格过渡块
tilemap.set_atlas(ground, 12, 8)
```

## `tilemap.set_cell(id: i64, x: usize, y: usize, value: u8)`

> 绑定：`engine/crates/rustlua-physics/src/tilemap_lua.rs:468`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `tilemap.set_kinds(id: i64, kinds: usize, variants: usize, autotile: Option<bool>, seed: Option<u32>)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/tilemap.rs:84`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

切换**格子值的语义**：`variants<=1` 且 `autotile` 关时格子值**就是**图集条目号（恒等档，
仓库里所有既有场景与 `set_cell` 调用点都在这一档）；否则格子值是**逻辑瓦 kind**（`1..=kinds`），
渲染时由「确定性变体哈希 + 4-bit 邻居掩码」解算成条目。格子数据一个字节不改，变的只是怎么读它。
图集布局随之固定为：`1..kinds*V` 是各 kind 的变体块，其后每 kind 占 16 格过渡槽。

- 参数：`id` — 挂 `Tilemap` 的实体；`kinds` — 逻辑瓦种数；`variants` — 每种的变体数（至少 1）；
  `autotile` — 是否启用 4-bit 自动瓦片，默认 `false`；`seed` — 变体哈希种子，不传沿用现值。
- 返回：无。`seed` 会序列化：编辑器、运行时与预览工具必须读同一个值，否则预览图与实机不一致。
  变体序号是跨进程可复现的纯函数（FNV-1a + splitmix64 收尾混洗），**不随帧变化**。
- 失败：`invalid tile map component string: variant_count must be at least 1`、
  `atlas layout needs <N> entries but the tileset has only <C> (<cols>x<rows>)`——占用按
  `kinds*(max(variants,1)+16*autotile)` 算，装不下当场报错而不是画错。
- 示例：

```lua
-- 4 种草/土/石/水，每种先给 1 个变体，开自动瓦片 → 需要 4*(1+16)=68 个条目
tilemap.set_atlas(ground, 12, 8)
tilemap.set_kinds(ground, 4, 1, true, 0x12345678)
```

## `tilemap.set_meta(id: i64, x: usize, y: usize, key: String, value: String)`

> 绑定：`engine/crates/rustlua-physics/src/tilemap_lua.rs:528`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `tilemap.set_solid(id: i64, x: usize, y: usize, solid: bool)`

> 绑定：`engine/crates/rustlua-physics/src/tilemap_lua.rs:488`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `tilemap.set_texture(id: i64, texture: String)`

> 绑定：`engine/crates/rustlua-physics/src/tilemap_lua.rs:374`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

TODO(文档桩)：一句话说明这个函数做什么、什么时候该用它。

- 参数：
- 返回：
- 失败：
- 示例：

## `tilemap.set_tint(id: i64, x: usize, y: usize, r: f32, g: f32, b: f32, a: Option<f32>)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/tilemap.rs:149`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

给一格涂**乘法**顶点色：季节/湿润的整体色调、建筑的阴影压暗都走这条路，而不是换贴图或盖一层
半透明 sprite（前者要为每个色调出一张图，后者会多一个绘制段并挡住同一层上后续的 tile）。
颜色进的是按颜色全等去重的调色表，序列化只记"哪些格非白"（`tints=`/`tints_rgb=`），920 格的
地图为了 16 格阴影只写 16 项。改动当帧生效：块的脏判定同时盯着格子值与染色 id。

- 参数：`id` — 挂 `Tilemap` 的实体；`x`/`y` — 格坐标；`r`/`g`/`b` — 乘数（1.0 表示不改色，
  0.72 这类值是"压暗"而不是"染色"）；`a` — alpha，缺省 `1.0`。
- 返回：无。与现值同色是 no-op；与调色表里已有的颜色全等则复用那个条目，不新增。
- 失败：`tile cell (<x>, <y>) is out of bounds`、
  `invalid tile map component string: tint table is full (255 colors)`、
  `invalid entity id: <id>`、`entity has no tilemap`。
- 示例：

```lua
-- 水田那一片整片偏冷
tilemap.set_tint(ground, 1, 0, 0.72, 0.84, 1.0)
```

## `tilemap.tint(id: i64, x: usize, y: usize)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/tilemap.rs:193`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

读回一格当前的染色，与画进顶点色的是同一份数据（门用它对账"解算里的 id 有没有真的落到 mesh"）。
**没染色的格子返回 `[1,1,1,1]`**——乘法单位元，不是全 0；把它当"这格没画"读会得出相反的结论。

- 参数：`id` — 挂 `Tilemap` 的实体；`x`/`y` — 格坐标。
- 返回：`{r=,g=,b=,a=}` 四个数（命名字段，不是数组）。坐标越界同样返回全 1。
- 失败：`invalid entity id: <id>`、`entity has no tilemap`。
- 示例：

```lua
local shade = tilemap.tint(ground, 5, 2)
assert(math.abs(shade.b - 0.42) < 1e-6)
```
