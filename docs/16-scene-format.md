# M4-03 场景格式

场景使用版本化的可读文本格式，默认扩展名为 `.scene`。格式包含版本号、场景名称、实体树、组件数据、资源 UUID 和编辑器元数据。

保存会先写入同目录临时文件，再替换正式场景文件；加载失败不会修改已有文件。解析错误包含行号和字段，便于编辑器定位问题。

当前支持版本为 `2`。

## 版本

- 当前版本是 `2`，包含 2D 组件、3D 组件和分离的 `UiLayout`。
- `Scene::from_text` 不做版本迁移。低于 `2` 或高于当前版本的文件都会通过
  `SceneError::UnsupportedVersion` 明确拒绝。

实体行支持 3 字段（无父实体，`entity\t<id>\t<name>`）或 4 字段（含父实体）两种写法；序列化时无父实体不再输出空尾字段，避免场景文件产生尾随空白。

```powershell
cargo run -p game --example scene_format_demo --release
```

## 3D 组件字符串

沿用 `key=value;` 约定，向量用逗号分隔（参考 `Collider offset=x,y`）：

- `Transform3D`：`position=x,y,z;rotation=rx,ry,rz,rw;scale=sx,sy,sz`
- `MeshRenderer`：`mesh=assets/models/<file>.glb|gltf`（`__ground_plane__` 为内置地面哨兵路径）
- `Camera3D`：`position=x,y,z;target=x,y,z;fov=<度>;near=<n>;far=<f>`（`aspect` 由渲染窗口决定）
- `Light`：`direction=x,y,z;color=r,g,b;intensity=<i>`（方向光）。`direction` 是**光传播方向**：
  天空里的太阳自上而下照，因此写作 `direction=0.4,-0.8,0.5`（**y 为负**）；写成 y 为正等于
  "光往上照"，朝上的表面拿不到漫反射（只剩 0.15 环境光）。详见 [docs/24](24-3d-rendering.md#light)。

## 2D 相机与粒子（实体组件，docs/54 §8-①）

同一个场景既能声明透视相机（`Camera3D`），也能声明**正交相机**与**粒子发射器**——它们和
Sprite/Mesh/文字/UI 一样是实体组件，不再只能由宿主状态或脚本表达。

- `Camera2D`：`center=x,y;zoom=<像素/世界单位>` 必填，`rotation=<弧度>`、
  `bounds=min_x,min_y,max_x,max_y` 可选。声明后它成为运行时的**初始** 2D 相机；Lua
  `camera2d.*` 与相机控制器之后照旧可以移动它（不是第二份姿态真源）。未声明的场景保持历史默认，
  因此既有工程逐字节不变。
- `ParticleEmitter`：`count=<每次爆发的粒子数>`、`rate=<粒子/秒，0=只爆发>`、`speed`、`life`、
  `size`、`size_end`、`color=r,g,b,a`、`color_end`、`gravity=x,y`、`drag`、`rotation`、
  `rotation_speed`。`size`/`color` 同时设置曲线起止值，`size_end`/`color_end` 可单独覆盖终点。
  `rate > 0` 时发射器按 `rate × dt` **连续**发射，位置取实体的世界位置（父链下会跟着父一起动）；
  累计器保留分数部分，因此帧率变化不改变平均发射量，长帧最多补发 512 个（不会一步倾泻）。
  未知键与非法数值一律报错，不静默忽略。

`examples/game_api/scenes/unified.scene` 是一份"六类内容同场"的样例：Sprite + Mesh +
`Camera3D` + `Camera2D` + `ParticleEmitter` + `WorldText` + `Text`/`UiLayout`。

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
