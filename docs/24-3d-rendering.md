# 3D 渲染与能力参考

- 适用范围：`engine/crates/engine` 的 3D 能力（glTF 导入、网格/材质、相机、光照、骨骼动画、3D 物理与渲染管线）。
- 对应里程碑：M8（3D 基础）、M9（3D 可玩垂直切片）、M10（编辑器 3D 视口）。
- 配套文档：[坐标和颜色约定](06-coordinate-and-color-conventions.md)、[Lua API](18-lua-api.md)、[场景格式](16-scene-format.md)。

## 1. 坐标与单位约定

参考 [ADR-0007](02-technical-decisions.md#adr-0007m8-3d-坐标单位和深度约定)：

- **右手系**，`+Y` 向上、`+Z` 指向观察者；长度单位为**米**。
- 矩阵按列向量语义组合，渲染使用 `clip = projection * view * model * position`。
- 透视相机使用 `fov_y`（场景组件中写**度**，内部转弧度）、正的 `near`/`far`（`near > 0`，`far > near`）。
- 深度纹理 `Depth32Float`，清除值 `1.0`，比较函数 `LessEqual`；3D pass 启用深度测试与写入，2D pass 保持无深度行为。
- 颜色纹理按 sRGB 采样；法线/金属度/粗糙度与深度数据按线性处理。
- glTF 节点坐标在导入边界按该约定转换（[gltf_asset.rs](../engine/crates/engine/src/core/gltf_asset.rs)）。

## 2. 3D 场景组件

组件以 `key=value;...` 字符串形式写在 `.scene` 文件（见 [16-scene-format.md](16-scene-format.md)），运行时由 `instantiate_scene` 解析进 World。

### Transform3D

`position=x,y,z;rotation=rx,ry,rz,rw;scale=sx,sy,sz`

- `rotation` 是单位四元数（`rx,ry,rz,rw`，非单位长度报错）。
- `scale` 各分量必须为正。
- 模型矩阵 `T * R * S`（WP2/WP5 收敛后 2D 与 3D 变换同址：[transform.rs](../engine/crates/engine/src/core/transform.rs)）。

### MeshRenderer

`mesh=<项目相对路径 .glb/.gltf>`

- 只接受项目相对路径，扩展名必须为 `.glb`/`.gltf`；特殊值 `__ground_plane__` 由渲染器合成地面接收平面。
- 网格来自 [gltf_asset.rs](../engine/crates/engine/src/core/gltf_asset.rs) 的 CPU 侧 glTF 2.0/GLB 导入器（`IMPORTER_VERSION = "gltf-m8-07"`）：支持多 primitive、材质（`base_color_factor` / `base_color_texture`）、skins（JOINTS_0/WEIGHTS_0）、animations、图片。

### Camera3D

`position=x,y,z;target=x,y,z;fov=<度>;near=<n>;far=<f>`

- `fov` 为垂直视场角（度），内部转弧度；宽高比**不在组件里**——它每帧由**视口**给出
  （`Viewport::aspect`，全仓库唯一的推导处），因此组件串里从来没有、也不会有 `aspect` 字段。
  投影/NDC/射线/视锥一族 API 都要一个 `Viewport`：只有拿到本帧视口才可能算出矩阵，
  于是"组件里存一份比例、宿主每帧写回它"那类两份状态从类型上不可表达（拾取与渲染因此必然同源）。
- `up` 未在组件中给出，look-at 由渲染器/运行时推导；退化（相机看向自身位置、零向量）报错。
- 实体上可放多个相机，`camera.set_active` 切换活动相机，未设置时回退到首个 `Camera3D` 实体。

### Light

`direction=x,y,z;color=r,g,b;intensity=<i>`

- 方向光，`direction` 是**光传播方向**（着色器取反得到表面→光源向量 `l = normalize(-direction)`）；`color` 为线性 RGB 分量 `0..1`；`intensity` 非负。
- **符号约定（写反的代价很大，因此在这里写死）**：天空里的太阳是**自上而下**照，所以
  `direction` 的 **y 分量为负**：`direction=0.4,-0.8,0.5`。写成 y 为正等于"光往上照"，
  朝上的表面 `diffuse = 0`，整个场景只剩 `0.15` 环境光（而且阴影也看不见）。
- **阴影相机与光照同源**：`renderer::light_view_projection` 把阴影相机放在 **`-direction`**
  一侧（光源那一侧）。两处符号必须一致，否则"阴影贴图算出的遮挡关系"和"着色器算出的明暗"
  互相打架。这两个约定由单测
  `host::renderer::tests::shadow_camera_sits_on_the_light_side_of_the_documented_direction`
  与像素门 `engine/crates/engine/examples/wp0_lighting_golden.rs` 同时钉住。
- 默认 `direction=0.4,-0.8,0.5`、`color=1,1,1`、`intensity=1`。
- 未设置活动灯时回退到首个 `Light` 实体。

### Animation3D

`mesh=<路径>;clip=<剪辑名>;looped=true|false`

- 剪辑来自同一 glTF 资产的 `animations`；支持 `translation/rotation/scale` 通道（`STEP` 保持、其余线性插值）。
- 骨骼蒙皮：每帧渲染器推进剪辑时间，产出关节调色板（最多 `MAX_BONES = 128`）上传 GPU。
- Lua 侧见 `animation3d` 模块（[18-lua-api.md](18-lua-api.md)）。

### Collider3D

- 3D 物理使用 [Rapier](https://rapier.rs)（[ADR-0008](02-technical-decisions.md#adr-0008m9-采用-rapier3d-作为-3d-物理后端)）。
- 形状（[physics3d.rs](../engine/crates/engine/src/core/physics3d.rs)）：
  - `shape=ball;radius=<r>`
  - `shape=cuboid;half_x=<x>;half_y=<y>;half_z=<z>`
  - `shape=capsule;half_height=<h>;radius=<r>`（沿本地 Y 轴）
  - 可选 `;sensor=true`（触发器）。
- 与 Transform3D 双向同步，固定时间步推进；碰撞事件进 Lua `physics3d.events()`。

## 3. 渲染管线

宿主渲染器（[host/renderer.rs](../engine/crates/engine/src/host/renderer.rs)、[host/shader.rs](../engine/crates/engine/src/host/shader.rs)）在每帧按顺序执行：

1. **阴影 pass**（`SHADOW_SHADER`）：从**光源那一侧**（`-light.direction`）渲染深度到阴影贴图（`texture_depth_2d`）。
2. **3D pass**（`MESH3D_SHADER`）：
   - 实例化绘制：所有 3D 网格用同一个绘制调用，实例模型矩阵存在 `instance_models` storage buffer。
   - 材质 uniform：`base_color`（RGBA）、`surface`（`x=metallic`、`y=roughness`、`z=skinned`）。
   - 光照：方向光（`light.direction` 取反做漫反射）、`specular = pow(max(dot(reflect(-l,n), view),0), 2/(roughness^2))`、金属度混合镜面与漫反射；阴影贴图采样 `textureSampleCompare`，带 `0.002` 深度偏移。
   - 骨骼蒙皮：`material.surface.z > 0.5` 时用 `joint_palette`（128 骨骼调色板）做 GPU 蒙皮。
   - 环境基础光 `0.15`，最终 `brightness = (0.15 + (diffuse*(1-metallic) + specular*mix(0.04,1,metallic)) * shadow) * intensity`。
3. **2D pass**：Sprite/Tilemap/Text/UI（`TEXTURED_SHADER`，含 MSDF 字形），无深度。

顶点格式（[mesh3d.rs](../engine/crates/engine/src/core/mesh3d.rs)）：

| 属性 | 类型 | 说明 |
|---|---|---|
| `position` | `vec3<f32>` | 局部坐标 |
| `normal` | `vec3<f32>` | 法线 |
| `texcoord` | `vec2<f32>` | UV |
| `joints` | `vec4<u32>` | 骨骼索引；非蒙皮顶点 `[0;4]` |
| `weights` | `vec4<f32>` | 骨骼权重；非蒙皮顶点 `[1,0,0,0]` |

## 4. Lua API 概览

3D 相关全局表（完整签名见 [18-lua-api.md](18-lua-api.md)）：

| 表 | 常用函数 |
|---|---|
| `transform3d` | `attach/get/set/move` |
| `mesh` | `set/get/clear` |
| `camera` | `set_active/get_active/get` |
| `light` | `set_active/get/set` |
| `animation3d` | `get/set/list/paused/set_paused/time` |
| `physics3d` | `get_velocity/set_position/events/raycast/overlap` |

## 5. 编辑器支持

- 编辑器 3D 视口（透视）支持轨道导航与 Gizmo（平移已实现，旋转/缩放后置，见 [ADR-0010](02-technical-decisions.md#adr-0010m9-编辑器-3d-gizmo-首版只做平移)）。
- Inspector 可编辑 `Transform3D`、`MeshRenderer`、`Camera3D`、`Light`、`Collider3D` 字段。
- 编辑器内嵌游戏预览复用同一 wgpu 设备 + offscreen 渲染目标（[ADR-0012](02-technical-decisions.md#adr-0012编辑器引擎内嵌采用单一-wgpu-设备--offscreen-渲染目标)）。
