# M4-03 场景格式

场景使用版本化的可读文本格式，默认扩展名为 `.scene`。格式包含版本号、场景名称、实体树、组件数据、资源 UUID 和编辑器元数据。

保存会先写入同目录临时文件，再替换正式场景文件；加载失败不会修改已有文件。解析错误包含行号和字段，便于编辑器定位问题。

当前支持版本为 `2`。

## 版本

- 当前版本是 `2`，包含 2D 组件、3D 组件和分离的 `UiLayout`。
- `Scene::from_text` 不做版本迁移。低于 `2` 或高于当前版本的文件都会通过
  `SceneError::UnsupportedVersion` 明确拒绝。

实体行支持 3 字段（无父实体，`entity\t<id>\t<name>`）或 4 字段（含父实体）两种写法；序列化时无父实体不再输出空尾字段，避免场景文件产生尾随空白。

### `parent` 的语义：`meta spatial`（docs/54 §4.1）

`parent` 有两种可能的意思，场景必须**自己声明**是哪一种（元数据行）：

```text
meta	spatial	local            # parent 是空间关系：Global(child) = Global(parent) * Local(child)
meta	spatial	organizational   # parent 只是组织关系（层级/分组），不参与空间组合
```

- **未声明 = 组织关系**（v2 的历史语义）。因此一个老场景加上 `parent` 边不会突然被按父子组合
  解释——那正是"静默改变画面"最容易发生的地方。
- `spatial=local` 是**迁移工具**（`--migrate-spatial`）写进去的标记：迁移把世界坐标换算成局部
  坐标并写上它，运行时据此建立继承。
- 宿主可以覆盖（`RuntimeScene::set_spatial_inheritance`）作为回滚档，但**默认值来自场景**。
- 认不出的取值按"组织关系"处理并**告警点名**，不静默吞掉。
- 文档侧（编辑器、Gizmo、keep-world 换父）读**同一份声明**，因此编辑器手柄与画面不会一个按
  组合算、一个按局部值画。

```powershell
cargo run -p game --example scene_format_demo --release
```

## 3D 组件字符串

沿用 `key=value;` 约定，向量用逗号分隔（参考 `Collider offset=x,y`）：

- `Transform3D`：`position=x,y,z;rotation=rx,ry,rz,rw;scale=sx,sy,sz`
- `MeshRenderer`：`mesh=assets/models/<file>.glb|gltf`（`__ground_plane__` 为内置地面哨兵路径）
- `Camera3D`：`position=x,y,z;target=x,y,z;fov=<度>;near=<n>;far=<f>`（`aspect` 由渲染窗口决定），
  可选 `pose=transform`（姿态改用实体的 `Transform3D`）与 `enabled=false`（见下）
- `Light`：`direction=x,y,z;color=r,g,b;intensity=<i>`（方向光）。`direction` 是**光传播方向**：
  天空里的太阳自上而下照，因此写作 `direction=0.4,-0.8,0.5`（**y 为负**）；写成 y 为正等于
  "光往上照"，朝上的表面拿不到漫反射（只剩 0.15 环境光）。详见 [docs/24](24-3d-rendering.md#light)。

## 2D 相机与粒子（实体组件，docs/54 §8-①）

同一个场景既能声明透视相机（`Camera3D`），也能声明**正交相机**与**粒子发射器**——它们和
Sprite/Mesh/文字/UI 一样是实体组件，不再只能由宿主状态或脚本表达。

- `Camera2D`：`center=x,y;zoom=<像素/世界单位>` 必填，`rotation=<弧度>`、
  `bounds=min_x,min_y,max_x,max_y`、`enabled=false` 可选。声明后它成为运行时的**初始** 2D 相机；Lua
  `camera2d.*` 与相机控制器之后照旧可以移动它（不是第二份姿态真源）。未声明的场景保持历史默认，
  因此既有工程逐字节不变。
- **`enabled=false`**（docs/54 §4.2，`Camera2D`/`Camera3D` 同义）：这台相机**不参与选帧**——既不会
  成为初始相机，也不会被 `camera.set_active` 选中（选中一台已禁用的相机时渲染会告警并回退到其它
  候选）。开关默认 `true`，且**启用时不写出该字段**，所以既有场景串逐字节不变；只有显式禁用才会
  写成 `;enabled=false`。
- `ParticleEmitter`：`count=<每次爆发的粒子数>`、`rate=<粒子/秒，0=只爆发>`、`speed`、`life`、
  `size`、`size_end`、`color=r,g,b,a`、`color_end`、`gravity=x,y`、`drag`、`rotation`、
  `rotation_speed`。`size`/`color` 同时设置曲线起止值，`size_end`/`color_end` 可单独覆盖终点。
  `rate > 0` 时发射器按 `rate × dt` **连续**发射，位置取实体的世界位置（父链下会跟着父一起动）；
  累计器保留分数部分，因此帧率变化不改变平均发射量，长帧最多补发 512 个（不会一步倾泻）。
  未知键与非法数值一律报错，不静默忽略。

## 一等矩形 `Quad`（docs/54 §4.3）

`Quad` 是**几何 + 材质**的一等声明：`size=w,h` 必填，可选 `pivot=x,y`（默认 `0.5,0.5`）、
`color=r,g,b[,a]`、`texture=<路径>`、`z=<f32>`、`layer=<名>`、`order=<i32>`、`flip_x`/`flip_y`。

与 `Sprite` 的分工只有一句话：**`Quad` 没有纹理区域**。

- 不给 `texture` 就用内置 1×1 白纹理，于是 `color` 就是最终颜色——纯色矩形、血条底、进度条、
  调试色块不再需要写 `texture=__white__` 这种内部哨兵（那个哨兵只在渲染器内部有定义）。
- 反过来它**不参与 `Animation`**：帧动画动的是 UV 区域，而矩形恰恰没有区域。需要帧动画就用 `Sprite`。
- 位置/旋转/缩放来自实体的 `Transform`（与精灵同源），`Quad` 只描述几何与材质。

渲染上它**不是第二条通道**：提取阶段降级成与精灵完全相同的批处理项，因此排序、剔除、深度、
可见遮罩、世界/overlay 两条路径都与精灵同源；分段按层升序绘制（层越大越靠上），未写 `layer`/`order`
时用与精灵同一套"按场景顺序"的默认层。

未知键、缺 `size`、零或非有限尺寸、轴心越界、颜色分量数不对、`texture=` 为空等一律**报错**——
把 `color` 拼成 `colour` 被静默忽略，与"颜色没生效"在画面上完全一样。

`examples/game_api/scenes/unified.scene` 是一份"七类内容同场"的样例：Sprite + `Quad` + Mesh +
`Camera3D` + `Camera2D` + `ParticleEmitter` + `WorldText` + `Text`/`UiLayout`。

## 每实体脚本 `Script`（docs/54 §4.1）

```text
entity  9  Patrol
component  9  Transform  x=640;y=360;rotation=0;scale_x=1;scale_y=1
component  9  Script     file=scripts/ai/patrol.lua
```

- `Script`：`file=<相对工程根的路径>` 必填，可选 `enabled=false`（组件仍在，只是不加载也不调用；
  启用时不写出该字段，因此既有场景串不变）。
- **路径只能在工程根内**：绝对路径与含 `..` 的路径在解析期就报错。脚本是可执行代码，场景文件
  能让它去读工程外的任何文件就不是"配置"而是"漏洞"。
- 脚本归属因此从"工程级一份"下放到实体：不必再在 `scripts/main.lua` 里遍历实体、按名字查、
  自己维护分派表。**一个脚本文件一份 Lua 状态，实体是实例**：`file` 相同的实体共享同一份 Lua
  状态（顶层局部变量因此按文件共享），脚本用收到的实体 `id` 区分自己。
- 钩子与工程级脚本**不重名**：每实体用 `on_spawn(id)` / `on_update(id, dt)`（都可省略），工程级仍是
  `on_create()` / `update()` / `on_destroy()`。详见 [docs/18](18-lua-api.md) 与
  [docs/19](19-lua-hot-reload.md)（热重载入口 `RuntimeScene::reload_scripts()`）。
- 加载时机：场景实例化末尾就会把脚本读进来并调用 `on_spawn(id)`，因此**文件不存在/编译失败在
  加载期报错**，而不是等到第一次 update 才发现脚本没跑。运行时新生成的实体下一帧自动补上
  `on_spawn`/`on_update`。调用顺序是确定的：每步在工程级 `update()` 之后按实体 id 升序各调用一次。

## 显式 2D 显示模式 `RenderMode`（docs/54 §4.3）

```text
entity  7  HudIcon
component  7  Transform   x=640;y=360;rotation=0;scale_x=1;scale_y=1
component  7  Sprite      texture=assets/textures/hud.png;size=24x24
component  7  RenderMode  mode=screen
```

- `RenderMode`：`mode=world|overlay|screen` 必填，未知取值**报错**（`mode=wrold` 被当成 overlay 会让
  "我写了 world 却没进世界通道"无从查起）。
- 三种模式的含义（§4.3 的三个名字）：
  - `world`：世界坐标 + 世界相机（可选深度）；
  - `overlay`：旧 2D overlay，屏幕空间、保持旧排序与像素行为；
  - `screen`：屏幕空间 UI，不受世界相机与世界 Transform 影响。
- **不声明 = 跟随全局开关**（`RuntimeScene::set_world_sprite_depth`），因此既有场景一字不改、像素
  不变——这是回滚档。
- `overlay` / `screen` 是**退出世界通道**：即使这一帧走世界通道，它们也留在屏幕空间，分别落在
  `Overlay2D` 与 `ScreenUI` 桶（HUD 画在世界内容之上，且 ScreenUI 又被其它 2D 内容盖不住）。
- `world` 是**请求世界通道**：只要场景里有实体这样声明，这一帧就走世界通道（与宿主开关打开同一
  效果）。注意通道是**帧级**的——默认实体（没写模式的）也会跟着进世界通道；这条是刻意的，写在这里
  免得"我只给一个实体开了 world，怎么别的也变了"变成意外。
- 三种模式只影响**产出路径**，不改变排序/深度/可见遮罩的既有规则（那些仍由 `SpriteSorting`、`z`
  与 `Visibility` 表达）。

## 可见性组件 `Visibility`（docs/54 §4.2）

`RenderView::visibility`（可见层遮罩）过去是个**没有消费者**的字段；现在由实体侧组件驱动：

- `Visibility`：`layers=<遮罩>`，接受十进制、`0x` 十六进制与 `0b` 二进制（位遮罩写十六进制最可读）。
  未知键、空串、非整数一律**报错**——静默当"全可见"会让遮罩看起来失效，静默当 0 会让内容整体消失，
  两种都只在真机上才看得出来。

判定是**相交**而非相等：

```text
实体 Visibility.layers  ∩  视图 RenderView.visibility  != 0  ⇒  本视图画它
```

边界（刻意的几条）：

- 实体**没有** `Visibility` 组件 ⇒ 全可见。既有场景因此不用改一个字、像素不变。
- 位怎么分配由项目决定，引擎不预设"第 0 位是世界内容"。需要的是视图与实体用**同一套位**。
- 被遮掉的实体是**不存在于这个视图**（视口剔除），不是"透明"：不产顶点、不占默认绘制层序号、不参与计数。
  每帧被遮掉的实体数记在 `RuntimeScene::view_filtered`，所以"东西没画出来"能一眼分清是遮罩拦的还是没提取到。
- 遮罩只作用于**有实体身份**的内容：世界精灵/瓦片/3D Mesh 用 world 遮罩，`UiImage`/`UiText` 用 overlay 遮罩；
  按名字注册的画布 UI（面板、血条、调试网格）不是实体，永远照画。
- 宿主用 `RuntimeScene::set_view_visibility(world, overlay)` 设置视图遮罩，`view_visibility()` 读回。
  默认 `u32::MAX`（全可见），遮罩为 0 表示"这个视图什么都不画"。

## 物理组件字符串（docs/54 §8-⑥）

2D 与 3D 的物理声明是同构的：`Collider` / `Collider3d` 都吃 `key=value;`，`layer`/`mask`
是同一套位域，`dynamic=false` 在两侧都表示**固定刚体**（地面、墙）：

- `Collider`：`shape=ball;radius=<r>` 或 `shape=cuboid;half_width=..;half_height=..`，可选
  `sensor=true`、`dynamic=false`、`offset=x,y`、`layer=N`、`mask=N`、`ccd=true`。
- `Collider3d`：`shape=ball;radius=<r>` / `shape=cuboid;half_width=..;half_height=..;half_z=..`
  / `shape=capsule;half_height=..;radius=..`，可选字段与 2D 同名同义，`offset` 为 `x,y,z`。

布尔字段只接受 `true`/`false`，拼错**报错**而不是静默当 false（`ccd` 被静默关掉会让高速子弹
重新穿墙且毫无线索）。2D 的 `Tilemap` 实体仍会自动生成静态单元格碰撞体，不需要写 `Collider`。

## 瓦片映射 `Tilemap`（M72-01 / M72-02）

`Tilemap` 也是 `key=value;` 编码，字段顺序固定为
`tile_size` → `width` → `height` → `texture` → `tiles_format` → `atlas_cols` → `atlas_rows`
→ `tile_kinds` → `variant_count` → `autotile` → `variant_seed` → `tiles` → `solid`
→（仅在有染色时）`tints` → `tints_rgb`。写出端只写这些，读入端**未知键一律报错**
（`tile_kind=` 少写一个 s 被静默忽略，症状是"逻辑瓦全变成图集索引"，画面会错得很难看）。

```text
component  1  Tilemap  tile_size=32x32;width=40;height=23;texture=assets/art/tilesets/manor/manor_ground.png;tiles_format=1;atlas_cols=6;atlas_rows=1;tiles=4,4,4,3,...;solid=0001...
```

### 两种 tiles 编码

| `tiles_format` | 写法 | 一格能表达的值 |
| --- | --- | --- |
| `0`（旧，仍能读） | 一个 ASCII 数字，无分隔符 | `0..9`，实际受旧图集 6 列限制为 `0..6` |
| `1`（当前） | 逗号分隔十进制 | `0..atlas_cols*atlas_rows` |

- `0` 表示空格，两种编码同义。
- 写出端**恒写 `1`**：旧串读进来重存即升级，数据本身逐字段不变（唯一变的是版本号）。
- 迁移工具：`cargo run -p game --example tilemap_resave --release -- [<project-root>...]`，
  只重写 `component\t<id>\tTilemap\t<载荷>` 这一行，其余行按原始字节回写（含 CRLF）。
  幂等，重跑得到同一份字节。
- 迁移正确性的判据不是"看起来一样"而是**像素一样**：
  `py tools/preview_scene.py scenes/<name>.scene` 在迁移前后必须逐字节相同。

### 图集寻址：`atlas_cols` / `atlas_rows`

条目 `n`（1 起）取图集里第 `col=(n-1) % cols`、`row=(n-1) // cols` 个子矩形，
UV = `[col/cols, row/rows] .. [(col+1)/cols, (row+1)/rows]`。

- **显式声明，不从贴图尺寸反推**：改贴图尺寸就静默重排所有格子，等于把整张地图换一遍。
  代价是"贴图几列"由数据说话，`py tools/preview_scene.py` 会拿 `atlas_cols*tile_size`
  与 PNG 实际宽高对账，对不上直接报错。
- `atlas_rows == 1` 时 V 恰好是 `0..1`，与旧实现的"整幅拉伸"数值相同——所以旧图集在新代码下
  不需要重新导出。
- `TILE_PALETTE`（`engine/crates/rustlua-ecs/src/tilemap.rs`）现在只是**没有贴图时的调试色板**，
  不再是列数真源。

### 格子值的两种语义：图集索引 / 逻辑瓦

由 `autotile` 与 `variant_count` 决定，`engine/crates/rustlua-ecs/src/tilemap.rs`
`Tilemap::resolve_atlas_index` 一处解算：

- `autotile=0` 且 `variant_count<=1`（默认）：格子值**就是**图集条目号，恒等映射。
  仓库里所有既有场景与 Lua `tilemap.get_cell/set_cell` 的调用点都在这一档，语义未变。
- 否则格子值是**逻辑瓦 kind**（`1..tile_kinds`），图集按下面布局排：

```text
条目 0                = 空格
1 .. kinds*V          = 每种瓦的 V 个纯变体（V = max(variant_count,1)）
kinds*V+1 + (kind-1)*16 + mask  = 该瓦的 4-bit 自动瓦片过渡槽
```

- **变体序号**必须是跨进程可复现的：FNV-1a 覆盖 `(x, y, variant_seed, 两个轴盐)`，
  常量是 `VARIANT_HASH_OFFSET` / `VARIANT_HASH_PRIME` / `MIX_AXIS_X` / `MIX_AXIS_Y`。
  **不许换成 `std` 的 `DefaultHasher`**——它按进程播种，症状是地表每帧抖一次，
  所有画面比对门全红。改这四个常量中的任何一个 = 全图重排。
- FNV-1a 之后还要过一遍 **splitmix64 的收尾混洗**（`avalanche`：`>>30 · *A · >>27 · *B · >>31`）
  再取模。这一步不是美化：原始 FNV 的低比特周期等于模数，`variant_count=4` 时第一行就是
  `3,2,1,0,3,2,1,0` 的周期点阵——确定性有、质感没有。
- **掩码位序按 index 命名，不按方位命名**：地图的 row 0 在**最底**，图集的 row 0 在**最顶**，
  "上邻居"在两个坐标系里指向相反的方向。位 0 = `y-1`、位 1 = `x+1`、位 2 = `y+1`、位 3 = `x-1`；
  越界当作"没有邻居"。同 kind 才算邻居。
- `variant_seed` 会序列化：编辑器与运行时读同一个值，否则预览图与实机不一致。
- **容量对账**：这套布局占用 `tile_kinds * (max(variant_count,1) + 16*autotile)` 个条目，
  必须 `<= atlas_cols * atlas_rows`，少一列就是**加载报错**而不是画错（`Tilemap::validate_layout`）。
  恒等寻址那一档不做这项检查（占用记为 0），因为它本来就没有扩容。
- 解算结果超出容量时返回 `0`（不画）。宁可行空，也不要画成邻居的美术——后者在场景里
  看起来"只是某块田花纹不对"，查起来是整图寻址表错位。

### 每格染色 `tints` / `tints_rgb`

顶点色是**乘法**染色（`engine/crates/engine/src/host/runtime_scene/tilemap.rs` 的
`append_tilemap_layer` / `append_tilemap_layer_world` 都把 `Tilemap::tint_at` 写进顶点色）。
存储是稀疏的：

- `tints=<cellIndex>:<tintId>,...`——只记**不是白色**的格子，`tintId` 指向调色表；
- `tints_rgb=r,g,b,a|r,g,b,a|...`——调色表，下标 1 起。

不用定长数字串是因为 920 格的地图为了 16 格阴影要写 920 个数字，而数字串把 tint id
卡死在 `0..9`。`cellIndex` 越界、`tintId` 超出调色表长度都**报错**，不做截断。
Lua 侧入口：`tilemap.set_tint(id, x, y, r, g, b[, a])` / `tilemap.clear_tint(id, x, y)`。

- 调色表按**颜色全等去重**（`Tilemap::register_tint`），同一个颜色重复注册拿回同一个 id；
  上限 255 个颜色，超了**报错**。
- 没染色的格子读回 `[1,1,1,1]`（乘法单位元），不是 `[0,0,0,0]`。
- 首版只有两个来源用这条路：**季节/湿润的整体色调**与**阴影压暗**。逐格手涂没有编辑器 UI
  支撑，因此没做。

### 相关的验证入口

```powershell
cargo run -p game --example jhyt_m72_tileset_atlas --release   # 多行寻址 + 编码迁移 + 旧语义不变
cargo run -p game --example tilemap_resave --release            # 重存工程内所有 .scene
py tools/preview_scene.py scenes/manor.scene                    # 迁移前后的像素级对账（在工程根运行）
cargo run -p game --example jhyt_m72_tile_variant_determinism --release  # 变体跨帧确定 + 哈希常量钉死
cargo run -p game --example jhyt_m72_autotile_mask --release             # 4-bit 掩码解出的条目 == 真画出来的
cargo run -p game --example jhyt_m72_tile_tint --release                 # 染色落到顶点色（读 mesh 不读像素）
node scripts/check-tile-hash-parity.mjs                         # web 宿主的 JS/Lua 镜像与 Rust 金标逐位一致
```

## UI 组件字符串（屏幕空间）

屏幕空间 UI 组件（`Text` / `Image` / `UiLayout` / `UiEvents`）同样用
`key=value;` 编码。`Text` / `Image` 只保存视觉属性；几何完全由 `UiLayout`
的四边 Anchor + Offset 决定。

- `Text`：`text=<esc>;font_size=..;color=r,g,b`
  - `font_size`（px）：像素字号（SDF 字体下 em 高度）；基准
    `UI_TEXT_BASE_PX = 12`。
  - `color` 只有三段：文字**没有** alpha 通道（淡入淡出走 `ui.*` 动态层，见下）。
- `Image`：`texture=..;color=r,g,b[,a];nine_slice=l,r,t,b`
  - `color` 的第四段是整块四边形的不透明度（直接落到顶点的 alpha 上）；省略即不透明，
    写出端在 alpha=1 时也只写三段，因此仓库既有的 120 行 `color=` 数据不会被保存动作重排。
    schema 侧用 `FieldKind::Color`（3 或 4 段都合法）而不是 `Vec3`/`Vec4`（那两个判"恰好 N 段"）。
- `UiLayout`：`anchors=l,t,r,b;offsets=l,t,r,b;min_size=w,h;max_size=w,h;pivot=x,y`
  字段均可省略并使用默认值，但完整几何必须通过本组件表达。
- `UiEvents`：`event=click;function=on_click`；多条绑定用 `|` 分隔。
