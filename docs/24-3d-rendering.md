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
- glTF 节点坐标在导入边界按该约定转换（[gltf_asset.rs](../engine/crates/rustlua-assets/src/gltf_asset.rs)）。

## 2. 3D 场景组件

组件以 `key=value;...` 字符串形式写在 `.scene` 文件（见 [16-scene-format.md](16-scene-format.md)），运行时由 `instantiate_scene` 解析进 World。

### Transform3D

`position=x,y,z;rotation=rx,ry,rz,rw;scale=sx,sy,sz`

- `rotation` 是单位四元数（`rx,ry,rz,rw`，非单位长度报错）。
- `scale` 各分量必须为正。
- 模型矩阵 `T * R * S`（WP2/WP5 收敛后 2D 与 3D 变换同址：[transform.rs](../engine/crates/rustlua-ecs/src/transform.rs)）。

### MeshRenderer

`mesh=<项目相对路径 .glb/.gltf>`

- 只接受项目相对路径，扩展名必须为 `.glb`/`.gltf`；特殊值 `__ground_plane__` 由渲染器合成地面接收平面。
- 网格来自 [gltf_asset.rs](../engine/crates/rustlua-assets/src/gltf_asset.rs) 的 CPU 侧 glTF 2.0/GLB 导入器（`IMPORTER_VERSION = "gltf-m8-07"`）：支持多 primitive、材质（`base_color_factor` / `base_color_texture`）、skins（JOINTS_0/WEIGHTS_0）、animations、图片。

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

### Environment

`map=<项目相对 .rlenv|.hdr>[;intensity=<>=0>][;rotation=<弧度>]`（P3-R03-4，[environment.rs](../engine/crates/rustlua-render/src/environment.rs)）

- 环境是**场景级**状态：组件挂在哪个实体上都一样，`RuntimeScene` 只取**实体 id 排序后的第一个合法声明**
  （重复声明不再报错，取值确定；见 [environment.rs](../engine/crates/engine/src/host/runtime_scene/environment.rs) 的
  `environment_payload`）。渲染器只有一份环境 uniform/绑定，因此"多份环境"没有可表达的差异。
- `map` 必须项目相对（绝对路径与越出工程根都报错），扩展名只接受 `.rlenv`/`.hdr`（大小写不敏感）。
  `.hdr` 是**作者写的源文件**，运行时读的是同名 `.rlenv`（导入期烘焙产物，见 §3.7）——
  `EnvironmentComponent::resolved_rlenv_path()` 负责这次换名，编辑器写回时仍保留作者原来写的 `.hdr`。
- `intensity` 必须有限且 `>= 0`，`rotation` 必须有限（只旋转环境采样方向，绕 Y 弧度）。
  未知字段直接报错：拼错的字段名不该静默变成默认值。
- 组件串由 `rustlua-render` 的 codec 注册进场景装表（`scene_codec`），`typed_scene` 有
  `ComponentId::Environment`，`rustlua-ecs/component_schema` 提供 Inspector 字段
  （`map`/`intensity`/`rotation`），所以**没有手写面板**也无需新增 match 分支。

### Collider3D

- 3D 物理使用 [Rapier](https://rapier.rs)（[ADR-0008](02-technical-decisions.md#adr-0008m9-采用-rapier3d-作为-3d-物理后端)）。
- 形状（[physics3d.rs](../engine/crates/rustlua-physics/src/physics3d.rs)）：
  - `shape=ball;radius=<r>`
  - `shape=cuboid;half_x=<x>;half_y=<y>;half_z=<z>`
  - `shape=capsule;half_height=<h>;radius=<r>`（沿本地 Y 轴）
  - 可选 `;sensor=true`（触发器）。
- 与 Transform3D 双向同步，固定时间步推进；碰撞事件进 Lua `physics3d.events()`。

## 3. 渲染管线

宿主渲染器（[host/renderer/](../engine/crates/engine/src/host/renderer/mod.rs)、[host/shader.rs](../engine/crates/engine/src/host/shader.rs)）每帧执行的通道由 [`RenderPlan`](../engine/crates/rustlua-render/src/render_plan.rs) **统一决定**（纯函数，单测可断言顺序；渲染循环不再各自写 `if`）：

```text
Clear → WorldSpriteOpaque → Shadow → DepthOpaque → WorldSprite → Particle
      → Tonemap → Legacy2DOverlay → ScreenUI → VectorText → PostProcess
```

1. **阴影 pass**（`SHADOW_SHADER`）：从**光源那一侧**（`-light.direction`）渲染深度到阴影贴图（`texture_depth_2d`）。
2. **3D pass**（`MESH3D_SHADER`）：
   - 实例化绘制：所有 3D 网格用同一个绘制调用，实例模型矩阵存在 `instance_models` storage buffer。
   - 材质 uniform（P3-R02 起 5×`vec4`）：`base_color`、`surface`（`x=metallic`、`y=roughness`、`z=skinned`）、
     `emissive`（已乘强度）、`params`（`x=有 MR 贴图`、`y=有 emissive 贴图`、`z=有 normal 贴图`、`w=normal_scale`）、
     `flags`（`x=有 occlusion 贴图`、`y=occlusion_strength`、`z=alpha_mode`（0=opaque/1=mask/2=blend）、`w=alpha_cutoff`）。旧 `mesh3d.wgsl` 只读前两个 `vec4`，缓冲区扩大不影响旧像素。
   - 光照：方向光（`light.direction` 取反做漫反射）、`specular = pow(max(dot(reflect(-l,n), view),0), 2/(roughness^2))`、金属度混合镜面与漫反射；阴影贴图采样 `textureSampleCompare`，带 `0.002` 深度偏移。
   - 骨骼蒙皮：按 draw 的 `skinned` 选 `SKIN` 变体；开启时用 `joint_palette`（128 骨骼调色板）做 GPU 蒙皮，关闭时顶点着色器不含权重求和。
   - 环境基础光 `0.15`，最终 `brightness = (0.15 + (diffuse*(1-metallic) + specular*mix(0.04,1,metallic)) * shadow) * intensity`。
3. **世界 Sprite / 粒子**：与 3D 网格共用相机、视口与深度附件（docs/54 §8-③/④）。
4. **Tonemap（P3-R01，可选）**：把 HDR 场景色映射回 LDR，见 §3.1。
5. **2D / UI / 矢量文字**：Sprite/Tilemap/Text/UI（`TEXTURED_SHADER`，含 MSDF 字形），无深度；**始终画在 Tonemap 之后**。
6. **后处理（可选）**：曝光/色调/暗角/最终输出。

### 3.1 HDR 与 Tonemap（P3-R01）

- **开关**：`rustlua_render::render_settings::RenderSettings` 是唯一配置源（`hdr/pbr/ibl/variants`），默认全关——旧 2D/3D 画面逐字节不变。
- **启用条件**（四件事同时成立，纯函数 `hdr_activation`）：策略允许（`Auto`/`On`）+ 设备探针通过（`Rgba16Float` 可渲染、可读回、可线性过滤）+ 本帧有 3D/世界内容 + `PostProcess.exposure == 1.0`（或无后处理）。纯 2D 帧不创建 HDR 目标。
- **目标**：场景画进 `Rgba16Float` 线性附件（可保存 >1 的高光）；`Tonemap` 用 ACES（默认）或 Reinhard 先乘 `2^exposure_ev` 再映射到 8 位 sRGB 目标。UI/矢量文字/后处理仍写 LDR 目标，因此曲线不会改 UI 颜色。
- **曝光归属**：HDR 帧的曝光只走 `PostProcess.exposure_ev`（`exposure` 必须为 `1.0`，否则该帧回退 LDR 并告警）；LDR 帧把 EV 折进后处理的曝光增益。
- **降级留痕**：能力不足时 `hdr_active=false`、告警一次并给出 `hdr_unavailable_reason()`；`RenderFrameMetrics` 暴露 `hdr_active/scene_format/tonemap`，性能面板与「复制报告」可查。
- **验收**：`cargo test -p engine hdr`；HDR golden 报告 `pre_tonemap_max>1`、`post_tonemap_max≤1`、`ui_delta≤1`（见 `baselines/p3/hdr`）。

顶点格式（[mesh3d.rs](../engine/crates/rustlua-render/src/mesh3d.rs)）：

| 属性 | 类型 | 说明 |
|---|---|---|
| `position` | `vec3<f32>` | 局部坐标 |
| `normal` | `vec3<f32>` | 法线 |
| `texcoord` | `vec2<f32>` | UV |
| `tangent` | `vec4<f32>` | `xyz` 切线、`w` 手性（±1）；`w=0` 表示无可用切线基（UV 退化/镜像） |
| `joints` | `vec4<u32>` | 骨骼索引；非蒙皮顶点 `[0;4]` |
| `weights` | `vec4<f32>` | 骨骼权重；非蒙皮顶点 `[1,0,0,0]` |

`MeshVertex` 共 80 字节；`tangent` 位于 `@location(5)`，骨骼仍在 3/4，旧着色器不读 5。
PBR 变体额外使用顶点缓冲槽 1 传逐实例法线矩阵（4×`vec4<f32>`，`Instance` 步进，stride 64，`@location(6..9)`），不属于 `MeshVertex`。

### 3.2 Shader 源文件与预处理（P3-R04）

- **源文件**：8 份 WGSL 在 [engine/shaders/](../engine/shaders/)，由 [host/shader.rs](../engine/crates/engine/src/host/shader.rs) 的 `include_str!` 在**编译期**嵌入二进制；运行时不读文件，部署不需要附带 shader 目录。旧的 `*_SHADER` 常量保留为别名，调用点不变。
- **身份**：`ShaderSource` 提供 `name` / 仓库内 `path` / 64 位 FNV-1a `hash` / `source`；`ALL_SHADERS` 与 `find(name)` 是唯一注册表。新增 shader 必须同时注册——单测钉住数量、名字/路径唯一性与 hash。
- **预处理器**：`host::shader_preprocess` 支持 `//#if` / `//#elif` / `//#else` / `//#endif` / `//#include "x.wgsl"`：
  - feature 必须出现在调用方给的 `known_features` 里，拼错（如 `SKN`）直接报错，不会静默走 `#else`；未激活分支同样校验；
  - 嵌套 16 层、include 链 8 层上限；include 循环报完整链；重复 `#else`/游离 `#endif` 都是错误；
  - 输出逐行记录 `line_map`（原文件 + 行号），wgpu 编译错误可回指；
  - 不读文件系统：include 源由调用方提供，单测不需要临时目录。
- **变体（R04-3/4）**：`host::renderer::pipeline_cache` 是唯一入口。
  - `ShaderVariantKey { family, shading, skin, target, alpha, double_sided }` 是固定轴结构体（`Copy + Hash`）；`mesh3d.wgsl` / `mesh3d_pbr.wgsl` / `shadow.wgsl` 用 `//#if SKIN` 在**编译期**分叉，运行时不再读 `material.surface.z`。
  - 构造 `Renderer` 时预热矩阵：网格静态/蒙皮 × LDR/HDR（HDR 能力不足时省略），阴影静态/蒙皮；每条键只编译一次，存放在 `HashMap<ShaderVariantKey, Arc<RenderPipeline>>`。
  - 绘制循环按 `draw.material.skinned` + 当前颜色目标 + `alpha_mode`/`double_sided` 选键，**只在键变化时** `set_pipeline`（同一材质的连续 draw 不重复设置）。alpha/双面组合在首次出现时按需编译（见 §3.6）。
  - 硬上限：每个 family 32、全局 64；超限返回带键的错误，**不淘汰**旧变体。统计 `hits/misses/compile_ms/variants` 进 `RendererResourceStats` 与性能面板「Shader 变体」三行，复制报告的 `shaders:` 行同源。
  - 启动预编译预算 `VARIANT_COMPILE_BUDGET_MS = 250`；超出只告警并保留已编译变体，避免把卡顿挪到首帧。
  - 设备无存储缓冲（WebGL2）时只放占位管线，3D/阴影通道本来就不在计划里；真实变体一个也不编译。
  - **尚未接线**的轴：`shadow`（R03 后续接入）；`normal_map` 不设变体轴——是否采样由材质 uniform 在片元里分支。R02-4 已把 `alpha`/`double_sided` 接入（§3.6），R03-2 已把 `ibl` 接入（§3.8，仅 PBR 网格；Legacy/阴影恒为 Off）。现在不伪造这些读数。
- **验收**：`cargo test -p engine host::shader`（4 条）、`cargo test -p engine shader_preprocess`（10 条）、`cargo test -p engine shader_variants`（4 条，含真实设备上的变体矩阵编译）、`wp0_variant_golden`（`variant_count/compile_ms/cache_hits` 与预算）、`wp0_3d_golden`/`wp0_2d_golden` 报告零 diff。

### 3.3 PBR 着色（P3-R02-1）

- **开关**：`RenderSettings.pbr`（默认 `false`）。关闭时网格通道仍走 `mesh3d.wgsl`（旧 Blinn-Phong 路径），`wp0_2d_golden`/`wp0_3d_golden`/`wp0_lighting_golden` 读数逐字段不变；打开后网格通道改用 `mesh3d_pbr.wgsl`，阴影仍由 `shadow.wgsl` 生成。
- **BRDF**：Cook-Torrance GGX（Trowbridge-Reitz NDF、Smith height-correlated visibility、Schlick Fresnel）；`F0 = mix(0.04, baseColor, metallic)`、`kD = (1-F)(1-metallic)`；方向光 + 阴影，环境项暂为常数 `0.15`（IBL 在 R03 接入）。CPU 参考实现在 `host::renderer::pbr`（`direct_brdf`），与 GPU 公式逐项对应。
- **几何修正**：法线乘 `transpose(inverse(mat3(model)))`，逆转置在 CPU 侧按实例算（WGSL 无 `inverse` 内建）；相机视线用 `normalize(camera.position - world_position)`，不再是固定 `(0,0,1)`。蒙皮法线先过 joint palette 再过法线矩阵。
- **法线矩阵传递**：顶点缓冲槽 1（`@location(6..9)`，4×`vec4<f32>`、`Instance` 步进、stride 64），与模型矩阵共用帧级环形缓冲的不同偏移。不用存储绑定，是因为在软件适配器上新增一个 bind group 布局条目会把启动编译从 233ms 推到 255–276ms，越过 250ms 预算。
- **相机 uniform**：`view_projection` + `position`（80 字节）；旧 `mesh3d.wgsl` 只读前 64 字节，缓冲区扩大不影响旧像素。
- **变体**：`ShaderVariantKey` 增加 `shading: Legacy | Pbr` 轴；PBR 变体在首次 `pbr=true` 时编译（静态/蒙皮 × LDR/HDR），不占默认启动预算，编译耗时单独告警。设备无存储缓冲（WebGL2）时 PBR 保持占位并告警。
- **验收**：`cargo test -p engine pbr`（8 条：非均匀缩放/刚体旋转/奇异矩阵/5×5 金属度-粗糙度网格/退化输入/旋转不变性）；`wp0_pbr_golden` 读数 `switch=34`、PBR 刚体旋转差 `0`（旧路径 `57`）、PBR 相机环绕差 `8`（旧路径 `0`）；engine lib 254 条测试通过。

### 3.4 材质资产与 ORM/自发光（P3-R02-2）

- **单一资产类型**：glTF 材质与 `.material` 都落到 `rustlua_assets::mat_asset::MaterialAsset`
  （base color / metallic-roughness / emissive / 五类贴图 / normal_scale / occlusion / alpha /
  double_sided / ibl_intensity）。GPU 侧只有 `GpuMaterial::from_material` 一条解释路径。
- **`.material` 格式**：`version=1` + 严格 `key=value`（未知字段、重复 `version`、非有限数、
  越界值、非法路径都报「文件:行号:原因」）。解析器与编码器同在 `rustlua-assets/src/mat_asset.rs`，
  往返测试覆盖全字段；贴图路径必须是工程相对的 png/jpg/jpeg。
  **为什么住在运行时层而不是渲染层**：导入层的 `MaterialImporter` 必须用同一台解析器校验
  （否则资源浏览器说「导入成功」而物体在场景里悄悄坏掉），而导入层不许依赖 `rustlua-render`；
  `mat_asset` 只需要 glTF 类型与 `infra::paths`，搬家没有新增依赖边。`rustlua-render::mesh3d`
  以 `pub use` 原样转发，渲染侧调用点不变。
- **覆盖语义**：`MeshRenderer` 组件串可写 `mesh=...;material=assets/materials/x.material`。
  覆盖**整体替换**内嵌材质（未写字段取 glTF 默认值）。解析/读取失败只告警一次并回退内嵌材质，
  不会让物体消失；单张贴图失败只回退该张贴图（告警含材质路径与贴图路径）。
- **glTF 导入**：`gltf_asset.rs` 读取 `base_color_texture`、`metallic_roughness_texture`（G=粗糙度、
  B=金属度）、`emissive_texture` / `emissive_factor` / `KHR_materials_emissive_strength`、
  `normal_texture`、`occlusion_texture`、`alpha_mode/cutoff`、`double_sided`。R02-2 先把 ORM 与
  emissive 接到着色器；normal/occlusion 的取样与切线生成见 §3.5（R02-3）。
- **GPU 上传**：base color / emissive 用 `Rgba8UnormSrgb`，ORM 用 `Rgba8Unorm`（数据贴图不能过
  gamma）。贴图按「内容 hash + 尺寸 + 色彩空间」去重；缺省绑定为 1×1 白（因子直通）。
- **着色**：PBR 片元 `metallic = factor × MR.b`、`roughness = factor × MR.g`（下限 0.045），
  `emissive = emissive_factor × strength × emissive_texture`，最后加在直接光/环境项之后。
  旧 `mesh3d.wgsl` 不声明绑定 5/6，`pbr=off` 的像素门逐字节不变。
- **资产图**：`ResourceKind::Material`（扩展名 `material`，族名「材质」）**并注册了
  `MaterialImporter`**（`core_importers()` 八项之一）——只有 kind 没有导入器时，扫描会把项目里
  每一份 `.material` 报成「不支持的资源类型」（这一步就漏过，见 P3 计划 §10 的补漏行）。
  `.material` 的 `assets/` 贴图行自动成为依赖边，`#` 注释行不参与扫描。
  唯一的**故意坏掉的**夹具 `assets/materials/_p3-r02-invalid.material` 用 `_` 前缀走
  「开发期文件」约定（`assetc::is_dev_artifact`）：不进扫描/导入/发行包，因此负向门不会把
  示例项目的导入结果打成失败，而 `wp0_pbr_material_golden` 照样读得到它。
- **验收**：`cargo test -p rustlua-assets mat_asset`（4 条：往返、缺省值、14 条非法输入、glTF 映射）；
  `cargo test -p rustlua-assets-import material_importer`（合法通过 / 非法逐条报错含行号 / BOM 拦下）；
  `cargo test -p assetc`（示例项目整体导入 `errors=0`）；
  `wp0_pbr_material_golden` 读数 `override=60`、`orm_vs_flat=13`、`emissive=191`、
  `invalid_fallback=0`、`missing_fallback=0`、`legacy_emissive=0`；`wp0_pbr_golden` /
  2D/3D/lighting golden 读数不变；变体启动预算 `compile_ms=240.8 < 250`。

### 3.5 法线/遮蔽贴图与切线（P3-R02-3）

- **绑定**：PBR 材质 bind group 在 6（emissive）之后增加 7 = normal map、8 = occlusion map，均为
  `Rgba8Unorm`（线性数据，不能过 sRGB 解码）；缺省绑定是 1×1 白。旧 `mesh3d.wgsl` 不声明 7/8，
  `pbr=off` 的像素门仍逐字节不变。
- **TBN 法线映射**：片元只在 `params.z > 0.5`（材质有 normal map）且 `tangent.w != 0` 时启用；
  `B = cross(N, T) * w`，采样值 `*2-1` 后按 `normal_scale` 缩放 x/y，再与几何法线合成并归一化。
  条件不满足时直接使用几何法线——退化数据永远退回平光照，不会产生 NaN。
- **遮蔽**：occlusion 只乘**环境项**（`ambient = base * 0.15 * radiance * mix(1, ao, strength)`），
  不改直接光与高光，与 glTF 的 ORM 语义一致。
- **切线生成**：glTF 没有 `TANGENT` 时按三角形 UV 行列式累积生成；`|det| ≤ 1e-12`、非有限值，
  或同一网格同时出现正/负手性（镜像 UV）判为 `TangentBasis::Degenerate`，写全零切线（`w=0`）
  并由渲染器 `tracing::warn!` 告警一次。文件自带 `TANGENT` 时归一化 xyz、`w` 归一到 ±1，
  零长度同样退化。
- **glTF 导入**：`GltfPrimitive.tangents` 与 POSITION 数量不一致或含非有限值直接报错；
  夹具 `p3-r02-material.gltf` 断言 TANGENT `[1,0,0,-1]`、ORM/emissive/alpha mask 与法线缩放。
- **网格缓存**：`AssetStore` 句柄只在单个 `RuntimeScene` 内唯一，渲染器命中 `mesh_slots` 时
  同时核对路径；编辑器重建运行时（句柄从 (0,1) 重新发号）不会把旧网格当成新网格。
- **验收**：`cargo test -p rustlua-render mesh3d`（17 条，含正交化/手性/退化/镜像/glTF 夹具）、
  `cargo test -p rustlua-assets gltf`（5 条，含 PBR 夹具）、`wp0_pbr_normal_golden` 读数
  `normal=32@2 scale_zero=0@0 degenerate=0@0 occlusion=36@2 legacy_normal=0 baseline=0`；
  2D/3D/lighting 报告与 R02-2 逐字节相同；变体启动预算 `compile_ms=218.7 < 250`。

### 3.6 Alpha、双面与透明排序（P3-R02-4）

- **材质字段**：`alpha_mode = opaque | mask | blend`、`alpha_cutoff ∈ [0,1]`（默认 0.5）、
  `double_sided`。glTF 的 `alphaMode/alphaCutoff/doubleSided` 与 `.material` 同名落地；
  有效 alpha 是 `base_color.a × albedo.a`。
- **mask**：有效 alpha `< cutoff` 时片元 `discard`；opaque 强制 alpha=1；blend 保留 alpha 交给
  固定管线混合。alpha 模式编码在材质 uniform `flags.z`（0/1/2），阈值在 `flags.w`。
- **blend**：网格管线用 `ALPHA_BLENDING`（`src×α + dst×(1−α)`）并**关闭深度写入**，深度测试仍开。
  半透明 draw 不参与实例合批，单独排队后按实例平移的相机平方距离**从远到近稳定排序**再绘制；
  等距时保留场景声明顺序，因此同一场景互换声明顺序像素逐字节一致（golden gate）。
- **双面**：`double_sided=true` 使用独立剔除状态（`cull_mode=None`）的网格/阴影管线；片元用
  `@builtin(front_facing)` 翻转几何法线，背面光照方向正确。正面渲染与单面逐字节一致。
- **阴影**：shadow pass 增加只做 alpha 剔除的片元阶段——mask 低于阈值 `discard`（不写深度），
  opaque 分支不采样贴图；blend 材质**不进入**阴影通道。双面材质的阴影使用同样禁剔除的管线。
- **变体**：`ShaderVariantKey` 增加 `alpha` 与 `double_sided` 两轴；启动期只预热 opaque 单面，
  mask/blend/双面组合在首次出现时按需编译（网格/阴影各一条），仍受 family 32 / 全局 64 上限
  与预算统计约束；Legacy 着色把两轴归零，`pbr=off` 不因材质多编译或改变像素。
- **验收**：`cargo test -p engine --lib`（256 条，含 alpha/双面变体键单测）；
  `wp0_pbr_alpha_golden` 读数 `mask_left=79 mask_right_bg=0 mask_low_right=78 mask_low_opaque=0
  blend_bg=71 blend_solid=40 sort=0 front_parity=0 back_culled=0 back_visible=111
  shadow_opaque=65 shadow_mask_clear=0 shadow_mask_visible=65 shadow_blend=0 legacy_alpha=0`；
  2D/3D golden 报告与 R02-3 逐字节相同；变体 golden `compile_ms=243.4 < 250`、`variant_count=6`。

### 3.7 环境资产与 `.rlenv`（P3-R03-1）

- **分层**：`.hdr`（Radiance RGBE）解码、equirect→cubemap、SH9 投影与 GGX 预过滤只在
  `rustlua-assets-import::environment`（构建工具/编辑器）编译；运行时与 wasm 只读
  [env_asset.rs](../engine/crates/rustlua-assets/src/env_asset.rs) 的 `.rlenv`，不引入 `image`。
- **源校验**：HDR 最大 8192×4096 且 ≤16MP（解码前经 `image::Limits` 预检），像素必须有限、非负；
  坏 magic/截断/超限都返回带原因的错误。
- **`.rlenv` 布局**（little-endian，版本 1）：magic `RLEN`、format=1（RGBA16F）、`source_hash`
  （原始 `.hdr` 字节的 FNV-1a）、`face_size`、`mip_count`、27 个 SH9 f32、mip-major 的 6 面
  RGBA16F 像素、尾部 FNV-1a 校验和。面顺序固定 `+X,-X,+Y,-Y,+Z,-Z`；未知版本/格式、坏校验和、
  长度不符、非有限或负 texel 一律拒绝。
- **烘焙**：equirect 双线性 → cubemap mip0；SH9 按 cubemap texel 均匀立体角累加后乘
  `A_l/π`（`A=[1,2/3,1/4]`），因此 `sh_irradiance` 直接给出可乘 albedo 的 `E/π`；mip>0 用
  确定性 Hammersley 序列做 GGX importance sampling，`roughness = mip/(mip_count-1)`，每 texel
  固定 64 样本。默认 128px/5mip，单 env 约 1 MiB，预算 ≤10s/≤8MB。
- **工具**：`cargo run -p rustlua-assets-import --example bake_environment -- --fixture` 生成
  `examples/game_api/assets/env/studio.hdr`（64×32，带 HDR 太阳）并烘焙 `studio.rlenv`；
  也可传 `<in.hdr> <out.rlenv> [face_size] [mip_count]`。同一份烘焙逻辑（`bake_hdr_file`）
  已接进批量导入，工程里的 `.hdr` 在导入时就会补齐同名产物（P3-R03-4，见 §3.10）。
- **验收**：`cargo test -p rustlua-assets-import hdr`（11 条：往返/坏文件/超大/NaN/负值/版本/
  校验和/确定性/常量白 SH/面朝向/夹具同步）；`studio` 烘焙 `1,047,696` 字节、debug
  `bake_ms=434.2`；同一输入两次烘焙逐字节相同。SH/镜面采样进 shader 与 `Environment` 组件在
  R03-2..4 接线。

### 3.8 SH9 漫反射 IBL 与 `ibl` 变体轴（P3-R03-2）

- **运行时形态**：`EnvironmentLighting { sh: [f32; 27], settings }`；`settings` 为 `intensity`
  与 `rotation`（弧度，绕 Y 轴）。`uniform(mode)` 打成 40 个 f32（160 字节）：9×`vec4`
  （rgb=SH 系数，a=0）+ `params = [intensity, rotation, mode.code(), 0]`。多出的 a 通道让
  WGSL 用 `array<vec4<f32>, 9>` 直接寻址，不存在跨槽读取。
- **渲染器接口**：`set_environment/clear_environment/environment`；`ibl_mode()` 先看
  `RenderSettings.pbr`，再看是否有环境，最后经 `resolve_ibl(settings, has_environment,
  specular_available)` 得到 `Off/Diffuse/Specular/Full`。R03-2 的 `specular_available=false`，
  因此 `On/Auto` + 环境只落到 `Diffuse`；R03-3 接入镜面后改为真实能力。未设置环境时
  uniform 写全零 SH + `mode=Off`，不残留上一帧。
- **着色器**：PBR 管线 layout 增加 group 2（binding 0，160 字节 uniform）。`//#if IBL_DIFFUSE`
  分支新增 `rotate_y` 与 `sh_irradiance`（基函数/常数与
  `rustlua_assets::env_asset::sh_irradiance` 逐项一致），环境项改为
  `base_rgb * sh_irradiance(rotate_y(n, -rotation)) * intensity * (1-metallic) * occlusion`；
  `//#else` 分支保持 R02 的常数环境 `0.15`，逐字节不变。
- **管线分界**：Legacy 网格仍用 2 组 layout（camera/material），只有 PBR 网格绑定 group 2；
  阴影不看着色模型也不绑环境。`ShaderVariantKey.ibl` 只在 `Shading::Pbr && Mesh3d` 时生效，
  Legacy/阴影恒归一到 `Off`，不会为它们重复编译 IBL 变体。基础 6 变体仍在构造期预热；
  IBL 变体在首次绘制时按需编译，标签后缀 `-ibl-sh/-ibl-spec/-ibl-full`。
- **验收**：`RUSTLUA_REQUIRE_GPU=1 cargo test -p engine --lib` 260 条通过，含
  `ibl_is_a_pbr_mesh_variant_axis`（Off 默认、PBR 键分叉、Legacy/阴影归一到 Off、标签）、
  `pbr_diffuse_ibl_replaces_constant_ambient`（Off 分支保留 `AMBIENT_FACTOR * radiance`，
  Diffuse 分支含 SH 常数/`rotate_y`/`(1.0 - metallic)`）、
  `ibl_diffuse_variant_compiles_with_environment_binding`（真实设备上编译带环境绑定的变体）、
  `constant_environment_sh_evaluates_to_unit_irradiance`（常量白环境任意方向 = 1，uniform
  布局与 params 模式码）；`cargo test -p rustlua-render --lib` 144 条通过。回归：
  `wp0_3d_golden`/`wp0_pbr_golden` 报告与 R02-4 逐字节相同（无环境时零变化）；
  `wp0_variant_golden` `variant_count=6`、`cache_hits=0`、`compile_ms=226.6 < 250`。

### 3.9 镜面 IBL：预过滤 cubemap + EnvBRDFApprox + mip 选择（P3-R03-3）

- **GPU 上传**（`host/renderer/environment.rs`）：`.rlenv` 的 6 面 / 逐 mip 像素直接写成一张
  `Rgba16Float` cube 纹理（`TEXTURE_BINDING | COPY_DST`），不重新过滤、不重排 mip。上传前校验
  面尺寸为非零 2 的幂、不超过 `max_texture_dimension_2d`、mip 数不超过 `log2(face_size)+1`、
  每级尺寸/mip 面 texel 数匹配且全为有限值——失败返回带原因的错误，**不改动**当前环境。
  `set_environment_asset(asset, settings)` 成功后才替换视图、SH 与参数；`clear_environment`
  同时卸载 cubemap 并恢复 1×1 黑 cube。
- **group 2 布局**：binding 0 = 160 B 环境 uniform，1 = `texture_cube<f32>`，2 = sampler。
  1×1 黑色 fallback cube 让 PBR 管线布局只有一种，IBL 开关不触发重编译。设备支持浮点线性
  过滤时纹理/采样器按 `filterable + Linear` 声明；否则按 `NonFiltering + Nearest`（Tier B），
  避免布局校验失败。
- **着色器**：`//#if IBL_SPECULAR` 分支用 Karis `EnvBRDFApprox`（常数与 CPU 参考
  `host::renderer::pbr::env_brdf_approx` 逐项一致，负权重夹到 0）计算 F0/roughness/N·V 的权重；
  反射方向 `reflect(-v, n)` 先按环境旋转转 `-θ`，再以
  `lod = roughness * params.w`（`params.w = mip_count - 1`）采样预过滤 cubemap。
  间接项 = 强度 × 遮蔽 × 预过滤辐射 × BRDF 权重。`Specular`-only 档不保留常数环境漫反射，
  `Full` = SH 漫反射 + 镜面，`Off` 仍逐字节保持 R02 公式。
- **档位**：`ibl_mode()` 的镜面能力 = 已上传 cubemap 且设备可绑定 `Rgba16Float`
  （`capabilities.hdr_color`）；`On/Auto` + 环境 → `Full`，仅 SH → `Diffuse`。
- **验收**：`RUSTLUA_REQUIRE_GPU=1 cargo test -p engine --lib` 265 条通过，其中
  `specular_ibl_roughness_selects_the_prefiltered_mip` 在真实设备上渲染金属地面
  （metallic=1、方向光强度 0、SH 全零、三级 mip 红/绿/蓝），中心像素随 roughness
  0.05/0.5/1 依次为红/绿/蓝，直接证明上传、绑定、BRDF 与 mip 映射四段接线；
  `pbr_specular_ibl_samples_prefiltered_cube` 离线钉住预处理分支；
  `env_brdf_approx_is_energy_bounded_and_monotonic_in_f0` 与
  `specular_lod_maps_roughness_across_prefiltered_mips` 钉住 CPU 参考。
  `wp0_3d_golden`/`wp0_pbr_golden` 与 R02-4 逐字节相同；`wp0_variant_golden`
  `variant_count=6`、`compile_ms=216.1 < 250`。新素材：
  `examples/game_api/assets/materials/p3-r03-metal-{smooth,mid,rough}.material`。

### 3.10 `Environment` 组件到 GPU：场景装配、tier 与回退（P3-R03-4）

- **装载点只有一个**：`RuntimeScene::init` 调 `runtime_scene::environment::load(project_root, &scene)`，
  把结果存成 `RuntimeScene.environment`。编辑器 Play 预览、编辑器 3D 视口与独立 `game.exe`
  都从这一份读数走（`environment_report()` / `environment_lighting()` / `environment_asset()`），
  所以"场景里声明的环境"不可能在不同宿主里解析成不同结果。
- **失败是可见状态，不是静默忽略**：组件无效 / 没有工程根目录 / 文件缺失 / `.rlenv` 解码失败，
  都会留下带原因的 `RuntimeSceneEnvironment.error`（`status_line()` 写成一行中文读数），
  光照与资产为 `None`。读文件先 `std::fs`、失败再退到 `rustlua_infra::vfs`（发布包/Web 同一口径）。
- **应用**（`apply_scene_environment` = `environment::apply_to_renderer`）：
  - 场景**没有**声明 `Environment` → 只 `clear_environment()`，**不动**渲染设置，旧场景像素逐字节不变；
  - 声明了 → 打开 `hdr=On; pbr=true; ibl=On`（保留宿主原来的 `variants` 策略），优先上传 cubemap；
    上传失败或没有资产时退 `set_environment(lighting)`（SH-only），原因写进
    `EnvironmentApplyReport.fallback_reason`；
  - Tier C（`capabilities.hdr_color == false`）即使 cubemap 上传成功也不可能有镜面，
    报告里追加"Tier C：Rgba16Float cubemap 不可用，镜面 IBL 关闭（保留 SH9 漫反射）"。
- **tier 是纯函数**：`ibl_tier(hdr_color, float_linear_filter)` → `A`（浮点立方体 + 浮点线性过滤）/
  `B`（浮点立方体、`NonFiltering` 采样）/ `C`（无浮点颜色，SH-only）；`supports_specular()` 只对 `C` 为假。
  `resolve_ibl(settings, has_environment, specular_available)` 决定 `Off/Diffuse/Specular/Full`
  （见 §3.8、§3.9）。`On` 与 `Auto` 的差别只在"没有环境"时体现：两者都不凭空造环境，
  `On` 遇到设备不支持镜面时**显式回退 `Diffuse` 并报原因**，不静默回到 `Off`。
- **资源侧**：`.hdr`/`.rlenv` 都是 `ResourceKind::Environment`（中文名"环境"），由
  `EnvironmentImporter`（`version=environment-1`）校验：`.rlenv` 走 `EnvironmentAsset::decode`，
  `.hdr` 走 Radiance 解码。批量导入在源项就绪后调用
  `rustlua_assets_import::environment::bake_hdr_file`，把 `.hdr` 就地烘成**同名** `.rlenv`
  （先写 `.tmp` 再 `rename`，内容已是最新时不重写）——所以设计者只丢一个 `.hdr` 进
  `assets/env`，运行时读的那份就存在了；烘焙失败会让整项导入判失败并把原因写进资源库
  （`ResourceState::ImportFailed`），而不是留下一个运行时必坏的空壳。编辑器把该-kind 的
  拖放目标目录定为 `assets/env`。离线重烘工具仍在：
  `cargo run -p rustlua-assets-import --example bake_environment -- --fixture`。
- **编辑器可见**：Inspector 用通用 schema 编辑器画 `map/intensity/rotation`，`map` 带
  `ProjectPath{extensions=["rlenv","hdr"]}` 规则，因此走既有的资产选择器（候选来自资源库、
  当前值即使缺失也保留在列表里）；Play 预览在创建渲染器时应用一次环境，并把 `EnvironmentApplyReport`
  缓存下来；性能面板新增两行——`IBL`（`mode（Tier X）`）与`环境`（`<map>`，失败时显示原因并标警示），
  文本报告同一口径输出 `ibl: mode=… tier=… map=… status=…`。3D Scene 视图左上角状态行的尾部
  直接挂当前档位（`IBL full（Tier A）`；场景没声明环境时写 `环境未声明 · Tier A`，避免把
  "没环境"读成"设备不行"），回退原因同时进视图诊断与控制台。
- **验收**：`RUSTLUA_REQUIRE_GPU=1 cargo run -p engine --example wp0_ibl_golden`
  （报告 `wp0-ibl-golden-v1`，`artifacts/p3/ibl`）在真实 WARP 设备上给出
  `tier=A`、`mode=full`、`map_size=128×5mip`、`env_bytes=1047696`、`bake_ms≈425.8`、
  `fallback_reason=none`、`draw_calls=8`、`triangles=6914`，并钉住五组观测：
  ① 未声明环境与"声明环境但 `ibl=off`"逐字节相同（LDR 默认设置保持 R02）；
  ② `Full` 相对 `Off` 最大通道差 255（环境真的进了着色）；
  ③ 三颗金属球（`metallic=1`，roughness 0.05/0.5/1）的反射随粗糙度变宽：
  6×6 盒式平均后的 coarse TV `1702 > 1080 > 894`、高光峰值 `210 > 193 > 183`、
  原始 TV `10554 > 6727 ≈ 6698`（mid/rough 接近打平，且 roughness=1 走 16px mip 时
  双线性会引入块状噪声，所以判据用低通后的轮廓而不是高频能量）；
  ④ 只上传 SH9、不上传 cubemap 时 `ibl_mode=Diffuse`、`environment_map_info()=None`，
  画面与 `Full` 最大差 255（Tier C 分支真的有 golden）；
  ⑤ 关掉 IBL 后再次逐字节回到基线。
  导入侧同时重新烘焙 `studio.hdr` 并断言与提交的 `studio.rlenv` **逐字节一致**。
  新增夹具：`examples/game_api/assets/models/p3-sphere.glb`（自造 UV 球，48×24、
  1225 顶点 / 2304 三角，`gen_p3_sphere_glb` 可重生）与
  `scenes/api_3d.scene` 里的 `StudioEnvironment` 实体。
  离线门：`host::runtime_scene::environment::tests` 6 条（首个声明生效、`.hdr`→`.rlenv`、
  缺文件/坏文件/无工程根/坏组件都只报原因、`RuntimeScene` 真的带出环境）。

### 3.11 Shader 源覆盖与热重载（P3-R05）

- **一条解析口径**：`host::shader_library::shader_override_dir(显式参数, RUSTLUA_SHADER_DIR, 兜底)`，
  优先级 参数 > 环境变量 > 兜底；空字符串/空白值按"没给"处理（脚本里 `RUSTLUA_SHADER_DIR=` 不该改变行为）。
  两个宿主只差兜底：
  - 编辑器兜底 **项目 `shaders/`** —— 开发工具默认就是开发态，始终监听；
  - 桌面运行时兜底 **`None`** —— 只有 `game --shader-dir <dir>` 或 `RUSTLUA_SHADER_DIR` 显式开启才监听，
    发布形态一次 shader 文件 IO 都不做（单测 `shader_hot_reload_is_off_until_a_directory_is_given` 钉住）。
- **覆盖文件**：`<dir>/<内置文件名>.wgsl`，只认直接子文件（子目录里的永远读不到，也就不该为它付全量校验的钱）。
  缺文件 = 回退内置源并在报告里点名；空文件或读不到 = **错误**，不静默回退（否则"我改了但没生效"会表现为回到内置外观）。
- **两条入口**：`Renderer::set_shader_override_dir(Option<PathBuf>)`（首次应用，返回完整报告）与
  `reload_shaders_if_changed()`（轮询，内容哈希没变返回 `Ok(None)`，一次编译都不做）。
  重载范围是**全部 8 个 shader**（R05-3）：3D 走 `PipelineCache` 变体，2D 走 5 个固定槽位；
  任一预处理/校验/编译失败 ⇒ 整批不 swap，旧管线继续画。
- **诊断文本自带位置**：`ShaderReloadError`/`ShaderValidateError` 的 `Display` 就是给用户看的一行，
  naga 错误带 `文件:行:列`（例：`WGSL 解析失败：… mesh3d.wgsl:41:12`），接口变更带旧→新的绑定槽位 diff。
  行号来自预处理器的 `line_map`（§3.2），所以指向**原始文件**而不是展开后的缓冲。
- **编辑器接线**（[shader_watch.rs](../engine/crates/editor/src/shader_watch.rs)）：
  - `WatchScope::default()` 的根多了 `shaders`（沿用既有 500ms 轮询观察器，项目相对路径）；
  - 命中 `shaders/*.wgsl` → 把 150ms 去抖窗口推到 `now + 150ms`（一次保存的多个中间态并成一次重载）；
    去抖到期时主动 `request_repaint_after`，否则空闲编辑器要等下一次鼠标动才变画面；
  - `--shader-dir` 指到项目**外**时观察器给不了事件，改由每帧自轮询（此时不约帧，避免空闲编辑器永远 150ms 重绘一次）；
  - 成功：状态栏 `shader 重载 R/V 个变体 · Xms` + Console 摘要（列出被覆盖的源、回退告警）+
    **作废 Scene View 的 2D/3D 视图缓存**（管线换了，离屏纹理里那张画面已经过期）+ 约一帧；
    Profiler 记 `shader_watch_hits` / `shader_reloads` / `shader_reload_failures` 三个计数器与
    `shader_reload_validate_ms` / `shader_reload_compile_ms` 两个 gauge（性能面板「计数器/Gauge」两节）；
  - 失败：Console 一行 + Scene View 浮层，同一条原因只播一次（观察器每轮都会重新命中同一份坏文件），
    状态栏 `shader 重载失败`；**不**作废视图缓存（管线一个都没换）；
  - 恢复：下一次检查「没有变化」就是恢复信号——作者删掉坏覆盖文件后，生效源回到内置、而管线里编译着的
    本来就是内置源，于是一次重编都不做；这时清掉自己写的那句失败诊断（按前缀认领，不动拖拽/框选的诊断）、
    Console 留一行「已回到可用状态」、状态栏 `shader 源正常`、约一帧让浮层真的消失。
- **运行时接线**：`GameApp::redraw` 开头一次到期检查（默认形态是一次 `None` 判断即返回），
  成功写 `tracing::info!` 摘要，失败按文本去重写 `tracing::error!`；初始应用失败**不**阻止启动，
  也**不**关轮询——作者修好文件后下一次就恢复。
- **Play 预览**：`PreviewSession::create` 多一个 `shader_dir` 参数，会话创建时应用与 Scene View **同一个**目录
  （两条路径不许读不同的源），但不每帧轮询——改完文件要重 Play 才进预览（刻意的取舍：预览线程不该带文件 IO 节奏）。
- **Web/release 无入口**：`shader_library`、`shader_reload`、`renderer::shader_hot_reload` 三个模块
  与渲染器的覆盖字段都是 `#[cfg(not(target_arch = "wasm32"))]`；wasm 目标只有内置源，
  `cargo check -p game_web -p gpu-web-probe -p wasm-lua-spike --target wasm32-unknown-unknown` 是这条门的形状证明。
- **不在范围内**：`renderer/compute.rs` 的计算管线由设备创建时编译，覆盖目录不影响它（§3.12）。
- **验收**：`cargo test -p engine --lib shader`（50 条）、`cargo test -p editor`（bin 422 条 / lib 190 条，
  含 8 条 `shader_watch`）、`cargo test -p game`（24 条）、`cargo run -p engine --example wp0_shader_hot_reload`
  （`variants=11 rebuilt=4 unchanged=7`、`validate 16.2ms + compile 13.1ms`、`[146,159,177] → [255,0,0]`）。
  其中 `shader_watch::tests::a_broken_project_shader_reports_its_source_line_and_recovers` 走**真实离屏渲染器**：
  项目 `shaders/textured.wgsl` 写坏 → Console 那行含 `textured.wgsl:4:`、状态栏 `shader 重载失败`、
  Scene View 浮层非空；删掉覆盖文件 → 下一次检查是「没有变化」，失败状态与浮层一起消失。
  这条没有 GPU 时直接 `return`，所以在 `RUSTLUA_REQUIRE_GPU=1` 下改成硬失败——没跑不许记成通过。

### 3.12 Compute 接缝与 GPU 粒子（P3-R06 / P3-R07-1）

本仓只有**一条** compute 通道，消费者是 GPU 粒子。两条线各自的地盘要分清楚：接缝管"能不能算"，
粒子层管"算的是不是同一件事"。

- **能力判据只有一处**：`Renderer::capabilities().compute` = `DownlevelFlags::COMPUTE_SHADERS`
  **与** compute 限制值（workgroup 三维 > 0）两条都满足。`Renderer::set_compute_enabled(false)` 是
  策略位（回滚档），不改硬件能力读数；关掉后 `dispatch_compute`、`read_storage_buffer`、
  `step_gpu_particles`、`configure_gpu_particles` 一律返回 `ComputeError::Unsupported`，
  **不 panic、不把失败当成"这一帧没有粒子"**。
- **接缝 API**（`host::renderer::compute`）：`ComputeRequest{label,shader,entry_point,bindings,workgroups,bind_group_key}`
  → `ComputeDispatch{workgroups,pipeline_compiled,bind_group_created}`；管线缓存按 shader 内容 hash
  + 入口 + 绑定签名命中，绑定组缓存的**身份由调用方给**（wgpu 的 `Buffer` 没有可哈希稳定 id，
  缓冲重建后必须换 key，否则"画面不动但也没报错"）。`ComputeStats` 记 dispatch/编译/命中。
- **消费者**：`configure_gpu_particles` 用 CPU 的**同一套** `ParticleEmitter` 语义生成初始 burst
  （随机数不进 shader，两档对拍比的才是积分而不是 RNG），compute 每步只把**状态**写进 48 B/槽的
  存储缓冲，渲染侧零回读。v1 只支持 burst（`rate > 0` 显式拒绝）；`read_gpu_particles` 仅给验收/调试。
  绘制侧从 **R07-3** 起是独立的 instanced 管线（见 §3.14）；R07-2 之前它每步额外展开
  `6 × WorldSpriteVertex` 写进 `STORAGE | VERTEX` 缓冲、复用世界 Sprite 管线画进 `RenderPass::Particle`。
- **场景侧的 `mode`/`seed`（R07-1）**：`ParticleEmitter` 增 `mode=cpu|gpu`（默认 cpu）与可选 `seed`；
  落点判定是纯函数 `placement(compute_available)`，条件与 `configure_gpu_particles` 的拒绝条件逐条一致
  （要 gpu、有 compute、burst 语义）。不满足就**用同一份参数回 CPU**，原因 `no_compute` /
  `continuous_rate` / `not_wired` 进运行时落点报告 `ParticlePlacementReport`，每种原因只喊一层日志。
  **R07-1 只有判定与回退，没有派发**：`mode=gpu` 的场景粒子今天仍全部在 CPU 池里，画面与旧粒子
  golden 逐字节相同。语义边界写死：GPU 粒子是**视觉层**，Lua 与玩法读不到它的位置；需要读位置的
  玩法用 `cpu`。场景字段与 Lua `opts` 的取值必须逐字命中（`mode=GPU` 报错，不静默退回默认）。
- **为什么能力由宿主下推**：`RuntimeScene::step_scene_emitters` 跑在调度系统里，拿不到渲染器
  （渲染器属于 `GameApp.renderer` / 编辑器 `scene_renderer`），所以运行时只有
  `set_particle_compute_available(bool)`，由 GameApp 启动、Play 预览会话、Scene View 三处各推一次——
  与 §3.10 的 `apply_scene_environment` 同一条先例，三条路径判出的落点必须是同一个答案。
- **验收**：`renderer/tests/compute.rs` 6 条（5 条真设备门：回读逐项正确 + 两级缓存命中、CPU/GPU 对拍
  `max_position_error < 1e-4`、连续发射显式拒绝、像素可见、无 compute 档 `Unsupported`；另 1 条钉住
  48/36 字节布局常量）；
  接缝独立门 `RUSTLUA_REQUIRE_GPU=1 cargo run -p engine --example wp0_compute_readback`
  （256 槽整数核逐值相等 + 双绑定浮点核容差 1e-6 + 同 key 二次 dispatch 命中缓存 + 关闭后
  dispatch/读回都 `Unsupported`；本机 WARP `Dx12 / Microsoft Basic Render Driver`，
  `workgroup=[256,256,64]`、`invocations=256`、`workgroups_per_dimension=65535`、
  `storage_binding=128MiB`、`float_max_abs_error=0`、`dispatches=2 pipeline_compiles=1 cache_hits=1`）；
  M74-03 侧 `cargo run -p game --example jhyt_m95_gpu_particles` 在真实 GPU 上跑 10k × 120 步，
  CPU/GPU 位置差 `9.155e-5`。WebGL2/微信上 compute=false 的回退是**纯函数单测的形状证明**，
  本轮没有跑 `gpu-web-probe`。

### 3.13 GPU 粒子派发通道（P3-R07-2）

R07-1 只回答了"该不该上 GPU"，本节回答"什么时候交给渲染器"。通道是**一条每帧的抽取**：
`RuntimeScene::dispatch_gpu_particle_bursts(&mut Renderer, delta_seconds) -> ParticleDispatchReport`，
调用点在脚本步进**之后**、图形提交**之前**（compute 必须赶在本帧 render pass 前提交）。
三条跑脚本的宿主各接一处：`game_app::run_simulation_steps`、编辑器 Play 预览
`preview_session::update_due`、Web `engine_core` 的步进尾部。Scene View 的**编辑态**不跑 Lua，
因此没有派发需求（它只推能力位）。

- **不丢粒子是硬约束**，三条回退都用**同一份参数**退回 CPU 池，照旧模拟、照旧被画：
  ① 渲染器预算拒绝 → `dispatch_rejected`；② 脚本侧单帧队列超限
  （`MAX_GPU_BURSTS_PER_FRAME = 32`）→ 溢出批次**就地**进 CPU 池并计数；③ 宿主没接派发通道 →
  队列里的残批在 `run_script_steps` **入口**倒回 CPU 池，报告记 `deferred` + `not_wired`
  （最多晚一帧出现，语义与 CPU 档一致）。
- **能力位每帧从渲染器重取**：`set_particle_compute_available` 是宿主启动时的一次性下推，
  设备重建或后端切换后只有派发这一步能把它拉回来。所以"启动时报了 GPU 可用但设备已丢 compute"
  不会继续谎报——测试 `gpu_bursts_reach_the_renderer_pool_and_stay_out_of_the_cpu_pool` 正是
  用"场景先被告知 `false`、渲染器实际有 compute"来钉住这个优先级。
- **GPU 粒子是视觉层**：渲染器收下的一批不进 CPU 池，`particles.count` 因此只数玩法可见的那些。
  场景（rate 驱动）发射器**永远**不可能走 GPU：`take_steps` 在 `rate <= 0` 时返回 0，而 GPU 路径
  拒绝 `rate > 0`——所以本节的通道是 burst 专用，`ParticlePlacementReport` 里的 `Gpu → not_wired`
  映射留着只是防御。
- **预算与回收**：`MAX_GPU_PARTICLE_SYSTEMS = 16`、`MAX_GPU_PARTICLE_SLOTS = 65_536`
  （instanced 之后每槽只有一份 48 B 状态 ⇒ 65_536 槽约 3.1 MB；R07-3 之前是 264 B/槽，见 §3.14）。
  `count` 超过 `u32` 时先饱和成 `u32::MAX`，于是"capacity ≥ count"的校验把
  **整批**拒掉，而不是截断成一小撮。系统 `elapsed > emitter.life` 即回收——这条是可证明的安全：
  CPU 侧 `max_life` 归一化到 `life × 0.5..=1.0`，所以寿命到点的系统里不可能还有活粒子。回收同时
  作废该系统的绑定组，`compute_bind_group_cache_len()` 是"缓存真的缩小了"的唯一可读证据。
- **每种回退原因只喊一层**：`particle_warned` 是位图，同一原因不因换发射器、换帧或换通道而刷屏；
  `fallback_reason` 只报第一个命中的原因，位图给全部。
- **验收**：`renderer/tests/particle_dispatch.rs` 5 条真设备门（两条 GPU 请求 `gpu=2`、CPU 池
  `0`、`systems=2`、`gpu_particles=6`、`read_gpu_particles().len()=6`；关 compute 后
  `fallback=Some(NoCompute)` 且 60 步后 CPU 池仍是 5；未派发的 4 粒晚一帧倒回并报 `not_wired`；
  预算满时 1 批 6 粒整批回 CPU 且 `systems` 停在 16；空报告默认值逐字段相等），
  `renderer/tests/compute.rs` 新增 4 条池语义门（两批互不干扰：64 下落 + 32 静态，静态批
  `x=100` 偏差 <1e-4、聚合 `capacity=96 spawned=96 steps=30`；两条预算各自拒绝；过期系统
  逐步回收，绑定组缓存 `2 → 1 → 0`，清空后 `read_gpu_particles` 报 `NotConfigured`；
  1k 粒子首帧与 CPU 参考逐项对拍 `max_position_error=0`、`state_bytes=48000`
  （R07-2 当轮还报过 `vertex_bytes=216000`——那是"每步展开 6 顶点"的旧路径读数，R07-3 起门里
  不再有这一项，见 §3.14），120 步后全部 `life > 0`）。
  `verify.ps1 -Quick`、旧粒子 golden 与 2D/3D/HDR 系列的读数见 P3 计划 §10 实施台账
  （`tasks/done/P3-渲染与编辑器能力.md`）。

### 3.14 GPU 粒子 instanced 绘制（P3-R07-3）

R07-2 的粒子是"compute 算、CPU 管线下发"：每步把每个粒子展开成 6 个 `WorldSpriteVertex`
（36 B/顶点 ⇒ **216 B/粒子/帧**的写带宽），1k 粒子实测 `vertex_bytes=216000`。R07-3 把它换成
一条真正的 instanced 通道，`RenderPass::Particle` 的入口不变，变的是谁生成四边形。

- **数据流**：`48 B/槽状态缓冲 → compute（只积分状态，死槽一位不写）→ 同一块缓冲当实例流
  （group2 只读存储）→ instanced 管线：4 顶点 triangle strip、无顶点缓冲、无索引缓冲`。
  几何（尺寸曲线 `size_start→size_end`、旋转、颜色插值、寿命淡出）全在顶点着色器里按
  `life/max_life` 算出来，所以每粒子每帧的写带宽从 216 B 降到**只在活着时写 48 B 状态**。
- **相机与贴图复用世界 Sprite**：`group0`（相机 uniform）与 `group1`（白贴图 + 采样器）直接绑
  `world_sprite_camera_bind_group` 与 `ensure_white()` 的绑定组，深度策略（`DepthPass::WorldSprite`：
  `LessEqual`、不写深度）与 alpha 混合逐字段相同——半透明叠加的观感与切换管线之前一致。
  粒子自己的资源在 `group2`（参数 uniform + 状态存储），所以复用前两组不产生绑定冲突。
- **两份 WGSL、一份声明**：compute 与 draw 是两个 shader 模块（绑定接口不同），但 `struct Particle`
  与常量由 `macro_rules! particle_common!` 同一段文本展开，两边不可能各写一份布局；
  单测 `compute_and_draw_share_one_state_declaration` 用文本前缀相等 + `struct` 计数钉住这件事，
  `gpu_layouts_match_wgsl_sizes` 钉住 Rust 侧 48 B 状态 / 64 B 参数与 WGSL 一致。
  WGSL **留在 `gpu_particles.rs` 内联**，不进 R04 的 `ShaderSource` 注册表，因此不在热重载范围内。
- **管线按目标格式惰性编译**：HDR 帧目标是 `Rgba16Float`、LDR 帧是 `self.config.format`，
  缓存 `Option<(TextureFormat, RenderPipeline)>` 一条，格式变了才重编译。**不做"用 LDR 管线画进
  HDR 附件"**——那会被 wgpu 校验层判成错误绘制；格式对不上或编译失败时记 `dropped_draws` 并喊一层，
  不 panic（粒子是视觉层，丢一帧比崩掉整局可接受）。惰性编译也是为了不碰 R04 的 250 ms 启动预算。
- **死粒子由顶点着色器剔除**：实例数恒等于 `capacity`，所以每帧不需要"还活着几个"的 GPU→CPU 读数；
  `life <= 0` 的实例在顶点着色器里退化成零面积四边形。代价是 `vertices = 4×capacity`、
  `triangles = 2×capacity` 是**常数**，`instances` 与 `vertices` 从此是两个量而不是 6 倍关系。
- **验收**：设备门 `RUSTLUA_REQUIRE_GPU=1 cargo test -p engine --all-features --lib -- gpu_particle`
  8 条，其中 `dead_gpu_particle_instances_draw_nothing_and_stay_frozen` 用 `elapsed == life` 这个精确
  窗口（池还在、粒子全死）钉两件事：死槽状态逐位不变（compute 一位没写）与画面回到纯背景
  （`changed_pixels=0`），同时活帧读数 `draw_calls=1 instances=1024 vertices=4096 triangles=2048`。
  场景级门 `RUSTLUA_REQUIRE_GPU=1 cargo run -p engine --release --example wp0_gpu_particles_golden`
  （10k 粒子、固定 seed、`dt=1/60`；首帧与 CPU burst **逐位**相同、0.5 s 对拍、9 s 状态在界、
  instanced 记账、两帧像素校验和、关掉 compute 的回滚档），报告进 `baselines/p3/`，
  语义契约由 `verify.ps1 -Benchmarks` 的 golden 段判红。读数见计划 §10 的 P3-R07-3 行。

### 3.15 GPU 粒子 3D billboard 与 50k 基准（P3-R07-4）

R07-3 的 instanced quad 一直在世界 XY 平面上展开：2D 正交相机下正确，但换成 3D 透视相机后
粒子是"躺在地上"的——正对 +Z 看还行，侧看投影面积趋近零。本节补上 billboard 分支，并用 50k
规模回答"默认档该不该换 GPU"。

- **billboard 不新增 uniform**：透视分支把 quad 的两个展开轴换成相机的世界右/上基，基向量直接
  取自 `view_projection` 的第 0/1 行——透视投影对该两行只有对角缩放，归一化之后就是视图矩阵
  的相机基，因此不会与相机矩阵不同步。**正交（2D）保持历史行为**（世界 XY 平面内展开），
  否则带旋转的 2D 相机会让既有粒子画面整体翻转。
- **透视/正交判别式看齐次 w 行的线性部分**：
  `|vp[0][3]| + |vp[1][3]| + |vp[2][3]| > 0.5`。不能用 `vp[3][3]`：正交下它是 1，而透视相机
  恰好位于原点时它是 0——与"透视/正交"没有对应关系；透视的 w 行线性部分是相机前向，正交恒为 0。
  判别式写错不会崩，只会静默画成"永远 2D"或"永远 billboard"，所以设备门用**侧视相机**
  （世界 XY 平面上的 quad 侧影面积为零）钉住：只有 billboard 分支才画得出像素。
- **50k 基准**（`wp0_gpu_particles_50k`）：renderer 级、同一发射器参数、固定 seed/dt。CPU 档 =
  模拟 + 发射 + `SpriteBatch::build_world_into` 顶点展开；GPU 档 = compute 积分 + instanced draw
  编码 + `device.poll` 的墙钟。报告版本 `wp0-gpu-particles-50k-v1`，由 `verify.ps1 -Benchmarks`
  按结构契约判红：规模 50k、状态 2 400 000 B + params 64 B、内存 ≤16 MiB、帧间不增长、
  一次 draw / 50k 实例 / 200k 顶点 / 100k 三角形、无非有限值、默认档与门槛读数自洽。
  报告里含中文的 `default_mode_reason`，脚本**显式按 UTF-8** 读（Windows PowerShell 5.1 的
  `Get-Content` 默认 ANSI 会把 UTF-8 字节截断成非法 JSON）。
- **本机读数（WARP，`Dx12 / Microsoft Basic Render Driver`）**：CPU 档 `step=0.385 ms`、
  `emit=6.298 ms`、`frame=6.684 ms`；GPU 档 `dispatch=0.150 ms`、`draw=0.185 ms`、
  `cpu_frame=0.335 ms`、`wall=206.717 ms`；`cpu_side_ratio=0.050`、`total_ratio=30.928`。
  CPU 侧（主线程提交成本）GPU 档便宜约 20×，但 WARP 的光栅化把总帧拉到 200 ms 量级，
  总帧门槛不成立。
- **默认档结论：仍是 `cpu`**（计划 R07-4 的规则："未达 1.5× 收益则默认仍 cpu"）。
  `default_mode=gpu` 只在 `(cpu_side_ratio ≤ 0.5 || total_ratio ≤ 0.8) && total_ratio ≤ 1.0`
  时为真；报告把本机为什么是 cpu 写进 `default_mode_reason`（WARP 总帧不代表真实 GPU）。
  验收门只判"结论与读数自洽"，不强迫任何一台机器选 gpu——真实 GPU 上的复测仍待补。
- **未跑**：真实 GPU 复测、`gpu-web-probe -Stage backend|render`、WebGL2/微信上的 50k 档。
  Web 端 compute=false 时 `mode=gpu` 走 CPU 回退，这条语义有单测覆盖，但本轮没有实测读数。
- **验收**：设备门 `gpu_particle_quads_billboard_to_a_perspective_camera`（侧视相机
  `changed_pixels=1344`，中心像素为粒子颜色；2D 路径由 10k golden 的逐像素校验和钉住）；
  基准 `RUSTLUA_REQUIRE_GPU=1 cargo run -p engine --release --example wp0_gpu_particles_50k`
  （`count=50000`、`state_bytes=2400000`、`content_pixels=578`、`default_mode=cpu`）；
  报告候选进 `baselines/p3/particle-r07-4`。读数见计划 §10 的 P3-R07-4 行。

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
- Inspector 可编辑 `Transform3D`、`MeshRenderer`、`Camera3D`、`Light`、`Collider3D` 字段；
  `Environment`（P3-R03-4）注册了 typed schema，由**通用** schema 编辑器接管（没有手写面板，
  也没有 match 分支），字段为 `map`（工程内相对路径 + `.rlenv`/`.hdr` 资产选择器）、
  `intensity`（非负）、`rotation`（绕 Y 轴弧度）。
- Play 预览与 3D 视口都在创建渲染器时应用一次场景环境；3D 视口的左上角状态行尾显示当前档位
  `IBL <mode>（Tier X）`（场景没声明环境时写 `环境未声明 · Tier X`）；性能面板（P3-R03-4）多两行读数
  `IBL = mode（Tier X）` 与 `环境 = <map>`（读取/上传失败时显示原因并标警示），
  文本报告同步输出 `ibl: mode=… tier=… map=… status=…`。
- 编辑器内嵌游戏预览复用同一 wgpu 设备 + offscreen 渲染目标（[ADR-0012](02-technical-decisions.md#adr-0012编辑器引擎内嵌采用单一-wgpu-设备--offscreen-渲染目标)）。
