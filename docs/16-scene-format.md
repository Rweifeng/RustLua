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

## UI 组件字符串（屏幕空间）

屏幕空间 UI 组件（`Text` / `Image` / `UiLayout` / `UiEvents`）同样用
`key=value;` 编码。`Text` / `Image` 只保存视觉属性；几何完全由 `UiLayout`
的四边 Anchor + Offset 决定。

- `Text`：`text=<esc>;font_size=..;color=r,g,b`
  - `font_size`（px）：像素字号（SDF 字体下 em 高度）；基准
    `UI_TEXT_BASE_PX = 12`。
- `Image`：`texture=..;color=r,g,b;nine_slice=l,r,t,b`
- `UiLayout`：`anchors=l,t,r,b;offsets=l,t,r,b;min_size=w,h;max_size=w,h;pivot=x,y`
  字段均可省略并使用默认值，但完整几何必须通过本组件表达。
- `UiEvents`：`event=click;function=on_click`；多条绑定用 `|` 分隔。
