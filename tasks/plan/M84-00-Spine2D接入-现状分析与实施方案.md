# M84-00 Spine 2D 接入：现状分析与实施方案

> 文档性质：方案/调研（BACKLOG 候选），不是已认领的实现任务。
> 分析日期：2026-11-19。分析对象：`D:/GitHub/RustLua` 当前工作树。
> 口径：只写能在仓库里指到文件或能在官方文档里查到的事实；会变动的读数不写死。
>
> **决策已确认（2026-11-19）**：① 运行时默认 `spine2d 0.5`，P0 对拍为最终门槛，失败才换
> `chine 0.2`；② 编辑器预览放到 P3，P1 不做；③ clipping 在 P2 先“导入期明确拒绝”交付，
> 再评估 stencil；④ 冻结 Spine 4.3.x，并预留升级通道（见 §4.1）。

## 0. 结论摘要

1. **引擎没有 Spine，也没有任何 2D 骨骼运行时。** 仓库里唯一叫“骨骼动画”的能力是
   `tasks/done/M8-07-骨骼动画.md` 交付的 **glTF 3D 蒙皮**（`Animation3D` + GPU joint palette），
   它输出的是 128×mat4 骨骼调色板给 3D shader；Spine 需要的是 **CPU 逐帧算出最终三角网格**
   （顶点位置、UV、顶点色、混合模式、图集页），两者不能复用。
2. **引擎的 2D 底座足够承接 Spine，但缺一条“任意三角网格 + 混合模式”的专用通道。**
   现有 `SpriteBatch` 是“每精灵 6 顶点、同纹理同层合批、层优先排序”，会打乱 Spine 的
   slot draw order，也不支持逐 draw 混合模式、双色 tint、索引三角形。应新增并行的
   `SpineMesh` 通道，而不是把 Spine 塞进 Sprite 批。
3. **运行时推荐纯 Rust 方案，不推荐 C FFI。** 已确认默认 `spine2d 0.5`（Spine 4.3、MSRV 1.95、
   MIT/Apache、wasm32 与 WebGL2 有实证、渲染输出为 `Vertex`/`DrawList`），以 P0 官方样例骨骼
   对拍为最终门槛，失败才切换备选 `chine 0.2`（同为 4.3、依赖更轻、明确实现全 4.3 时间线）。
   `spine_runtime 0.2.2` 因 MSRV 1.99 > 本仓锁定的 1.97.1、且许可证是
   non-standard（会被 `deny.toml` 的 allow-list 拦）不选；`rusty_spine 0.8` 只支持 Spine 4.2；
   官方 `spine-c/cpp` 走 FFI 会引入跨编译与供应链复杂度，且与 web/微信的构建链冲突。
4. **分五阶段推进**：P0 无侵入 spike → P1 最小可玩闭环（运行时 + 组件 + Lua + 世界通道）→
   P2 渲染完整性（四种混合、PMA、tint black、裁剪）→ P3 资源/编辑器/平台/质量门收口 →
   P4 性能与文档。每阶段都有可复跑的验收门，不把“能编译”当“接入完成”。
5. **合规是硬前置**：Spine Runtime License 要求每个使用者持有 Spine Editor 授权，且产品不得
   替代 Spine 编辑器创建/编辑骨骼。引擎可以集成运行时，但编辑器只能播放/检视，不能做
   骨骼编辑；许可证文本与版权声明必须随发布物分发并写入 `docs/THIRD_PARTY.md`。

## 1. 引擎现状（按接入相关度）

### 1.1 工程与平台基线

| 项 | 现状 | 证据 |
|---|---|---|
| 工作区 | 36 个 member crate，Edition 2024，`rust-version = 1.97.1` | `engine/Cargo.toml` |
| 工具链 | `rust-toolchain.toml` 精确锁 `1.97.1`，升级需同步 CI 与 `tools.json` | `engine/rust-toolchain.toml` |
| GPU | wgpu 27（`rustlua-gpu/Cargo.toml`），Web 目标另开 `webgl` feature；wasm32-unknown-unknown 可编译 | 同上、`scripts/verify/verify.ps1 -Quick` |
| 平台 | PC（已验收）、WebGL2（产物层）、微信小游戏（产物层）、Android APK（本机无工具链未验） | `README.md` §平台与发布 |
| 依赖门 | `cargo-deny` 许可证白名单、只允许 crates.io 与 path 源、yanked 判红 | `engine/deny.toml` |
| 架构门 | crate 间依赖有 BUDGET 白名单，新增边要登记 | `engine/crates/rustlua-infra/tests/architecture_edges.rs` |

### 1.2 渲染：有 2D 批、有深度通道，但没有 Spine 需要的几何形态

| 能力 | 现状 | 对 Spine 的影响 |
|---|---|---|
| 基本 2D 图元 | `SpriteBatch` 每精灵 6 个非索引顶点；`SpriteVertex { position:[f32;2], uv:[f32;2], color:[f32;4] }` | Spine 是索引三角形网格，且顶点数不固定，不能直接进这个批 |
| 排序 | `SpriteBatch::build` 按 layer→texture 合批；世界通道再由 `sort_world_ranges` 按 layer→z→原序稳定重排 | Spine 的 `draw order` 是骨骼数据的一部分；必须保证同一实体内部顺序不被纹理合批打乱 |
| 深度 | `WorldSpriteVertex { position:[f32;3], uv, color }`；不透明写深度，半透明测不写；`DepthPass::{WorldSpriteOpaque,WorldSprite}` | 可复用世界通道与 z 排序；但当前半透明通道**固定** `ALPHA_BLENDING` |
| 混合 | 2D overlay/world sprite 管线固定 `wgpu::BlendState::ALPHA_BLENDING`；3D 才有 `MaterialAlphaMode` 变体 | Spine 四种 blend（Normal/Additive/Multiply/Screen）需新增管线变体 |
| 双色 tint / 裁剪 | 顶点里没有 dark color；2D 深度附件 `stencil: StencilState::default()`，没有 sprite 级 stencil/scissor | tint black 与 clipping attachment 是 P2 的明确增量 |
| pass 计划与对账 | `render_plan.rs` 用纯函数 `RenderPlan::for_frame` 决定 12 条规范 pass，`mismatch()` 与实跑对账 | 新增 Spine pass 必须同时改 `RenderPass` 枚举、计划函数、`render_passes` 执行与对账测试 |
| 图集 | `rustlua-render::atlas` 自研 `.atlas` v1（单纹理 + 命名矩形）；`SceneMesh.paths` + `AssetHandle` 提供纹理身份 | Spine 导出的是另一种 `.atlas`（多页 + 旋转/偏移/PMA），不能共用解析器，由运行时 crate 解析 |
| 坐标系 | 世界 Y 向上、屏幕左上原点、右手系（`docs/06`） | 与 Spine 的 Y-up 一致，不需要整体翻 Y；只需一个单位缩放系数 |

### 1.3 动画：三套并存，都不覆盖 Spine

- `AnimationStateMachine` / `.anim` 帧动画：轨道写 `Transform`、`Sprite.region`、UI 样式，
  由 `schedule.rs` 的 `animation_state_advance` / `animation_player_advance` 推进，事件走
  `AnimationState::push_events` → Lua `animation.events()` 取走即清。
- `Animation3D`：glTF skin + joint palette（`MAX_BONES=128`），`animation3d` Lua 表支持
  `set/list/crossfade/paused/time`。这是 3D GPU 蒙皮，与 Spine 无关。
- `animation_track.rs` 虽有 `TrackTarget::SkeletonPose` / `TrackValue::Pose(Vec<Mat4>)`，但它是
  3D skinned pose 的骨骼矩阵，不是 Spine 的 bone/attachment/skin/draw-order 数据，不能复用。
- 缺口：没有骨骼驱动 2D 网格的组件，也没有“动画事件来自外部运行时”的先例——但事件队列
  的模式可以原样照搬。

### 1.4 资源：可以扩，但注意扩展名冲突

- `ResourceKind::from_path`：`.json` → `Scene`，`.atlas` → `Atlas`（**自定义 v1 格式**），
  `.skel` → `Other`。Spine 导出的 `.json`/`.atlas` 会与现有分类冲突。
- `Importer` trait 提供 `version/kind/extensions/validate/produce`；导入器在
  `rustlua-assets-import::core_importers()` / `full_registry()` 注册，测试
  `full_registry_includes_core_and_scene_importers` 要求每个已知 `ResourceKind` 都有 importer。
  `ResourceDatabase` 有依赖图与失效传播，`assetc` 负责打包校验。
- 资源身份是两套：`AssetId`/`AssetRecord`（工程身份、依赖边、失效状态）与
  `AssetHandle { index, generation }`/`AssetStore`（运行时稳定句柄）。Spine 数据应两套都接。
- `assetc/package.rs` 有压缩白名单与 `bundle_plan` 分包分组规则；`.json/.skel/.atlas` 与页 PNG
  要进包并归到正确 unit，否则 Web/微信发布物缺件。
- 热重载基础存在：`editor-core/src/file_watcher.rs`（size+mtime，2 秒内一致）+ 资源库
  `reconcile/Stale/propagate_invalidation`；但没有“Spine 数据变更即重建骨架”的成品链路。
- 建议用**命名空间化导入**：工程里只登记一个 `.spine` 清单作为资源身份，清单引用
  `.skel`（二进制骨骼）、`.spineatlas`（Spine `.atlas` 改名避冲突）、页 PNG；这类衍生文件名
  在 P1 定稿，避免改 `ResourceKind` 的既有映射。

### 1.5 组件 / 场景 / 脚本 / 编辑器

- 新增组件的**顺序是明文写死的**（`rustlua-scene/src/typed_scene.rs:98`）：
  ① `ComponentId` 加变体与名字 → ② `compile_component` 加分支 → ③
  `component_schema::builtin_registry` 注册；漏第三步有测试判红。
- 场景/Prefab 是字符串载荷组件（如 `Sprite` 的 `texture=...;size=...`），编辑器 Inspector 通过
  `ComponentDescriptor` 表（`crates/editor/src/panels/inspector/mod.rs`）登记，默认值在
  `inspector/actions.rs`，基础字段由 `component_schema.rs` 的 schema 驱动显示，复杂资源选择需
  专用面板。组件校验/资源路径提取走 `rustlua-component-codec` 的 `ComponentCodecRegistry`，
  避免 scene 层反向依赖表现层 crate。
- Lua 绑定在领域 crate 里提供 `install*` 函数，由组合根 `engine/src/host/script_packs.rs` 用
  `add_deferred_installer` 装配；组件 by name 用 `add_component_applier`；切场景用
  `add_scene_transition_hook` 清状态。新增模块要同时更新：
  `api_surface_snapshot/lua_api_surface.json`、`lua_api_surface_2d.json`、`rustlua_api.d.lua`、
  `examples/game_api` 的可运行演示、`scripts/capabilities.lua` 能力项、`docs/18-lua-api.md`
  与 docgen 检查。硬门包括 `sample_project_demos_cover_every_lua_api`（运行期 `_G` 是事实源）
  与 `sample_project_scenes_cover_every_runtime_component`（`ComponentId::ALL` 每个都要在样例场景出现）。
- 编辑器与运行时共享几乎全部领域 crate；`editor/src/panels/timeline.rs` 的 `.anim` 时间线面板
  （播放头/关键帧/事件/吸附/撤销重做）是 Spine 状态面板的可复用模板，`editor-core/src/anim_ops.rs`
  的“写 tmp + rename”原子保存与安全路径校验可直接照搬。
- 调度阶段固定为 `Input→Script→Sync→Physics→Writeback→Simulation→Animation→Spatial→Transform→Render`；
  Spine 的 `advance` 应作为内建系统挂在 `Animation` 阶段，渲染数据提取在 frame_data 阶段。

## 2. Spine 侧的技术依赖

一套 Spine 资产至少包含：骨骼导出（`.json` 或二进制 `.skel`）+ atlas 描述（`.atlas`）+ 一个或多个
纹理页（PNG）。运行时每帧需要：

1. `AnimationState::update(dt)` 与 `apply`，多轨混合、队列、事件；
2. 骨架求解（FK/IK/transform/path/physics/slider 约束、deform、sequence）；
3. 输出渲染命令。以候选 crate 为例：
   - `spine2d`：`DrawList` + `Vertex { position:[f32;2], uv:[f32;2], color:[f32;4], dark_color:[f32;4] }`
     与 `Draw { texture_path, blend, premultiplied_alpha, first_index, index_count }`；
   - `chine`：`RenderCommand { positions, uvs, triangles:Vec<u16>, color, dark_color, page, blend }`，
     `RenderScratch` 可在稳态做到零分配。
4. 渲染器必须支持：索引三角形、逐顶点 light/dark 颜色、四种混合、按 atlas 页换纹理、
   PMA（预乘 alpha）开关、严格的 draw order。

混合与双色 tint 的数学（参照 `chine-web` 的 WebGL2 后端，可作为本引擎 shader 的对照实现）：

| Spine blend | PMA = true | PMA = false |
|---|---|---|
| Normal | `(ONE, ONE_MINUS_SRC_ALPHA)` | `(SRC_ALPHA, ONE_MINUS_SRC_ALPHA)` |
| Additive | `(ONE, ONE)` | `(SRC_ALPHA, ONE)` |
| Multiply | `(DST_COLOR, ONE_MINUS_SRC_ALPHA)` | 同左 |
| Screen | `(ONE, ONE_MINUS_SRC_COLOR)` | 同左 |

双色 tint（dark=0 时必须退化为普通 tint）：

```wgsl
out.a = tex.a * light.a;
out.rgb = ((tex.a - 1.0) * dark.a + 1.0 - tex.rgb) * dark.rgb + tex.rgb * light.rgb;
```

## 3. 运行时选型

| 方案 | Spine 版本 | 许可证 / MSRV | 渲染输出 | 判断 |
|---|---|---|---|---|
| **`spine2d 0.5.0`** | 4.3 | MIT OR Apache-2.0 / **1.95** | `Vertex` + `DrawList`（含 `premultiplied_alpha`） | **已确认默认**（P0 对拍为最终门槛）。纯 Rust，crates.io 可直取，wasm32/WebGL2 有 demo；core 与 wgpu 集成分离（其 wgpu 后端要 29/30，本仓是 27，**只用 core**） |
| `chine 0.2.0` | 4.3 | MIT / 未声明，依赖 glam+serde_json 很轻 | `RenderCommand`（页索引 + blend + dark） | **备选**。文档称覆盖完整 4.3 时间线与约束，有 fuzz 与 untrusted-input 保证；但发布约 1 个月、采用量小，需 spike 对拍 |
| `spine_runtime 0.2.2` | 4.3 | non-standard / **1.99** | `RenderCommand` | 不选：MSRV 高于锁定工具链；non-standard 许可证需改 `deny.toml` 例外 |
| `rusty_spine 0.8.0` | 4.2 | non-standard / 较低 | C API + Rust 薄封装 | 不选：数据版本落后一代；Rust API 不支持 Timeline/自定义附件加载；许可证同样要例外 |
| 官方 `spine-c/cpp` | 4.3 | Spine Runtime License | 官方 `SkeletonRenderer` 回调 | 不选：C++ 工具链 + 四平台交叉编译 + git 源被 `deny.toml` 禁止（要 vendor）；维护成本最高 |

**P0 的定案门槛**：默认采用 `spine2d 0.5`，以下任一不满足则切换 `chine 0.2` 并重新对拍：
在 1.97.1 上 `cargo check` 通过；wasm32 通过；能加载公开 4.3 示例骨骼（raptor/spineboy 等）
并输出 draw list；与另一运行时或官方导出的顶点/绘制次数对拍一致；`cargo deny` 不新增许可证例外。
切换备选只改 `rustlua-spine` 内部适配层（见 §4.1），不改组件/Lua/渲染接口。

**P0 已完成（2026-11-19）**，结果见 `M84-01-P0-Spine运行时对拍报告.md`：门槛中依赖、wasm32、
官方样例加载、draw list、官方同源 JSON/SKEL 对拍、许可证全部通过；**跨实现骨骼世界坐标
在部分动画有 1.8–24 px 偏差**（集中在 `back-shoulder → rear-upper-arm → gun/muzzle` 链），
当时两个实验实现互相对拍无法裁决谁贴近官方。

**P1 前置门已完成（2026-11-19）**，结果见 `M84-02-P1前置门-官方Golden对拍报告.md`：
以官方 spine-ts 4.3.13 runtime 在 Node 下输出逐帧骨骼世界变换作 golden，8 组样例 × 60 帧
三方对拍，`spine2d` **8/8 一致**（最大 0.035 px，多数 < 0.002 px），`chine` 4 组偏差
4.4–25.9 px。偏差方确认为 chine，`spine2d` 的默认地位不再附带未决风险；`chine` 在修复并
通过同一 golden 门之前不得替换 backend。

## 4. 目标架构与每帧数据流

```text
.spine 清单 (+ .skel / .spineatlas / page.png)
        │ 导入期：校验 + 依赖登记 + 缓存
        ▼
SpineData（骨骼数据 + atlas 页 + 动画名，Arc 共享，资源句柄化）
        │ 场景装配期：ComponentId::SpineSkeleton → ECS 组件
        ▼
ScheduleStage::Animation ── spine_advance(dt)
        │  AnimationState::update/apply → 骨架求解 → DrawList（复用 buffer）
        ▼
frame_data 提取 ── SceneMesh.spines: { vertices, indices, draws }
        │  保留 draw order；world 层按 layer/z 稳定跨实体排序
        ▼
rustlua-gpu ── spine 专用管线（blend × pma × hdr 变体，dark 走顶点属性）
        │  与 world sprite 共用相机/视口/深度；半透明测深不写深
        │  RenderPlan::for_frame 纳入 pass 计划，`mismatch()` 对账必须覆盖
        ▼
Lua：spine.set_animation / events / set_skin / bounds ...
```

**隔离原则**：Spine 数据不进 `SpriteBatch`；Spine 组件不复用 `Animation3D`；Spine 的骨骼层级
不进实体 `SpatialParent` 传播（与 docs/38 对“骨骼 palette 不并入实体空间传播”的既定边界一致）。
**排序双体系不能混用**：`SpriteSorting.layer/order` 决定同层先后，`RenderLayer::{World2D,Overlay2D,ScreenUI}`
决定渲染层归属；Spine 的 slot draw order 是第三种顺序，只在“同一实体内部”生效。

### 4.1 版本冻结与升级通道（已定：Spine 4.3.x）

- **冻结口径**：内容侧只接受 Spine Editor 4.3.x 导出的骨骼/atlas，`.spine` 清单里显式记录
  `spine_version`（精确到 4.3.x 小版本）；导入期校验，不匹配即报明确诊断，不做“尽力解析”。
- **隔离层**：`rustlua-spine` 内部拆成两层——`backend/`（运行时 crate 与数据版本适配，
  负责加载、求解、产出统一的 `SpineRenderOutput`）与 `player/`（动画状态、事件、皮肤、
  生命周期，只依赖内部中性类型）。渲染、组件、Lua 三层只认 `SpineRenderOutput`/`SpineData`，
  **不直接引用第三方 crate 的类型**。
- **升级路径**：将来升 4.4/5.x 或切换 `chine` 时，只替换 `backend/` 实现并新增一组对拍样例；
  `player/`、渲染通道、组件与 Lua API 面保持不变。每新增一个后端都要过 P0 对拍门槛。
- **证据留存**：P0 对拍报告（`M84-01-P0-Spine运行时对拍报告.md`）已记录三元组：
  `spine-runtimes 4.3 分支 commit d913caab6ead…` ↔ `spine2d 0.5.0`/`chine 0.2.0` ↔
  `spineboy-pro.json 8bfcf880…` / `.skel e10de3f2…`；复跑命令在报告 §8，作为升级回归基线。

## 5. 分阶段实施

### P0：无侵入技术验证 ✅ 已完成 2026-11-19（结果见 `M84-01-P0-Spine运行时对拍报告.md`）

- 在 `engine/target/` 之外建一个临时 spike（或 `tools/spine-spike/`，验收后删除/归档），
  先接 `spine2d 0.5`，仅在门槛不达标时才接 `chine 0.2` 复测。
- 验证：1.97.1 + wasm32 编译；加载 4.3 示例 `.skel/.json + .atlas`；逐帧输出 draw list；
  统计顶点数、三角形数、blend 分布、PMA 标记；与另一实现或官方导出做顶点对拍。
- 产物：一页对拍报告（含采用的版本、冻结的 Spine Editor 版本、示例骨骼哈希、失败项），
  并入本文件 §3，作为 §4.1 的升级回归基线。
- 验收：`cargo check --target wasm32-unknown-unknown` 通过；对拍差异可解释；确认默认运行时。

### P1：最小可玩闭环

新增/改动：

- **前置门（P0 遗留）已完成**：官方 golden 三方对拍 8/8 通过（`M84-02`），覆盖
  `back-shoulder → rear-upper-arm → gun/muzzle` 链（idle/run/walk/jump）。**P1 已放行**，
  无需再评估回退；后续每次改 backend 都跑 `python golden/run_golden_gate.py` 回归。
- 新 crate `crates/rustlua-spine`：按 §4.1 分 `backend/`（包装 `spine2d`，产出中性
  `SpineRenderOutput`）与 `player/`（`SpineData`/`SpineSkeleton` 组件、`SpinePlayer` 动画状态与
  事件队列）两层；渲染/组件/Lua 只认中性类型。只依赖 ECS/math/infra/lua-api 与运行时 crate；
  依赖边登记进 `architecture_edges.rs` 的 BUDGET。
- `rustlua-render`：新增 `spine_mesh.rs`（`SpineVertex`/`SpineIndex`/`SpineDraw`/`SpineMesh`），
  在 `SceneMesh` 加一个 `spines` 字段；空时零成本。
- `rustlua-gpu`：新增 `renderer/spine.rs`，先只做 Normal blend、非 PMA、无 dark、无裁剪；
  走世界通道，测深不写深；draw 段按宿主给定顺序提交，不在 GPU 侧重排。若新增独立 pass，
  必须同步 `render_plan.rs` 的 `RenderPass` 枚举、`for_frame` 决策与对账测试。
- `engine`：组件装配器 `SpineSkeleton`；`spine_advance` 内建系统（Animation 阶段）；
  frame_data 提取与事件入队；场景切换清事件与玩家状态。
- Lua：新 `spine` 表首批 6–8 个函数（`set_animation/get_animation/list/set_skin/set_speed/events/set_enabled`）。
- 资源：`.spine` 清单 + 新 importer + 依赖声明；`.spineatlas` 避扩展名冲突；注册进
  `core_importers()`/`full_registry()`，否则 importer 完备性门红。
- `examples/game_api`：一个可运行 demo + 一个带 Spine 实体的场景（组件覆盖门）+ 能力图谱条目；
  `docs/18` 与三份 API 快照同步。

验收：样板工程里一个 Spine 角色能播 Idle/Run、Lua 可切换、事件能到脚本；
`game --check` / `editor --check` / `cargo test --workspace` / `cargo clippy -D warnings` /
`cargo check -p game_web --target wasm32-unknown-unknown` 全绿；API 面与文档门绿。

### P2：渲染完整性

- 四种 blend + PMA 开关（按 atlas 页标记）；tint black 的 dark 顶点属性与 shader 公式；
  多图集页纹理绑定；页纹理走 `AssetHandle` 而非裸路径。
- Spine clipping attachment：**已确认先以“导入期明确拒绝 + 诊断”交付**（禁止静默丢片）；
  stencil 裁剪作为后续增量，届时评估 2D 深度附件启用 stencil 与 WebGL2 能力探针，再实现裁剪队列。
- 跨实体排序：同一实体的 draw order 原序保留；跨实体按 layer→z 稳定排序，与
  `sort_world_ranges` 同一口径；混合模式变化时不得合并 draw。
- 验收：官方示例骨骼（含 mesh/clip/tint black/多页）与参考后端截图逐张比对；
  `rustlua-gpu` 增加 blend/PMA/dark 的像素级单测；HDR 与非 HDR 两条路径都过。

### P3：资源、编辑器、平台与发布收口

- 资源：`.spine` 清单进 `ResourceDatabase` 依赖图，改骨骼/atlas 触发失效与重导入；
  `assetc` 打包包含全部引用文件，并把 `.json/.skel/.atlas` 加进压缩白名单、把新类型纳入
  `bundle_plan` 分组；在 `file_watcher` 的变更信号上补“重建骨架运行时数据”链路；
  VFS 路径解析（wasm/微信不能走 `std::fs`）。
- 编辑器（`已确认在 P3，P1 不做`）：`ComponentDescriptor` 加 `SpineSkeleton`（默认载荷指向工程内
  示例 `.spine`），通用字段编辑器可改动画名/皮肤/缩放/排序层；资源面板显示新家族；做静态姿势
  预览（只读播放/检视）。**不做骨骼编辑/导出**（许可证边界）。
- 平台：WebGL2 与微信小游戏产物层必须出现 Spine 几何；Android 保持“未验”口径不虚报。
- 文档：`docs/18`、`docs/25`、`docs/26`、`docs/THIRD_PARTY.md`、发布清单与 SBOM 说明回写。

### P4：性能与质量门

- 基准：固定场景（1/10/100 个角色）、p50/p95/p99、每帧分配、顶点/三角形/draw 数、GPU 时间；
  与现有 10k sprite 基线同机对照，前后报告进 `artifacts/`。
- 稳态零分配：复用 draw list / vertex / index 缓冲；禁止每帧为每个 attachment 造 `String`。
- 全门：`verify.ps1`（含 `-Quick`）、`-Web`、`-Mvp`、GUI smoke、`check-doc-claims`、
  `check-api-2d.ps1`、`gen-lua-api-docs.js --check`、能力图谱门、`check-platform-parity`、
  `check-decoupling`、`check-cfg-coverage`、`cargo deny`；WebGL2 无存储缓冲路径
  （实例矩阵走 `@binding(2)` uniform）下 Spine 几何仍需正确。

## 6. 关键改动点清单（实现时按此核对）

| 层 | 文件/位置 | 改动 |
|---|---|---|
| 依赖 | `engine/Cargo.toml`、`deny.toml` | 新增运行时 crate 依赖；确认许可证在白名单；不引入 git 源 |
| 架构门 | `rustlua-infra/tests/architecture_edges.rs` | 登记 `rustlua-spine` 的依赖边 |
| 组件 | `rustlua-scene/src/typed_scene.rs` | `ComponentId::SpineSkeleton` + `ALL` + `as_str` + `compile_component` |
| 组件 schema | `rustlua-ecs/src/component_schema.rs` | `builtin_registry` 注册新组件，Inspector 基础字段自动显示 |
| 组件校验 | `rustlua-component-codec/src/lib.rs` | `ComponentCodecRegistry` 注册 Spine 载荷校验与资源路径提取 |
| 渲染数据 | `rustlua-render/src/{spine_mesh.rs,frame.rs,lib.rs}` | 新顶点/draw 类型；`SceneMesh.spines` |
| pass 对账 | `rustlua-render/src/render_plan.rs` | `RenderPass`/`for_frame`/`mismatch` 与执行侧同步 |
| GPU | `rustlua-gpu/src/renderer/{mod.rs,spine.rs,construct/*,pipeline_cache.rs}` | 混合变体、shader、索引缓冲、draw 提交与计数 |
| 宿主 | `engine/src/host/{script_packs.rs,runtime_scene/schedule.rs,runtime_scene/frame_data.rs}` | 组件装配、系统注册、每帧提取与事件 |
| Lua | `rustlua-spine/src/spine_lua.rs`、`api_surface_snapshot/*` | 新模块与三份快照/类型提示 |
| 资源 | `rustlua-assets/src/{resource_kind.rs,importer.rs,resource_db.rs,asset_handle.rs}`、`rustlua-assets-import/src/importers.rs` | 新家族/importer/依赖边/运行时句柄 |
| 打包 | `assetc/src/{scan.rs,package.rs}`、`rustlua-infra::bundle_plan` | 压缩白名单、分包归属、发布物缺件门 |
| 热重载 | `rustlua-editor-core/src/file_watcher.rs` | Spine 数据变更触发骨架重建 |
| 编辑器 | `editor/src/panels/inspector/{mod.rs,actions.rs}`、资源面板、`panels/timeline.rs`、`anim_ops.rs` | 组件与资源可视化；状态面板模板与原子保存 |
| 样例/文档 | `examples/game_api/**`、`scripts/capabilities.lua`、`docs/18/25/26/THIRD_PARTY.md` | 演示与文档门 |

## 6.1 P1 实现结果（2026-11-19 完成）

P1 已落地并通过全部门禁。**与 §6 清单的差异**如实记在这里：

| 层 | 实际落点 | 与计划的差异 |
|---|---|---|
| 依赖 | `engine/Cargo.toml`（workspace 成员 + `spine2d 0.5` 只开 `json`/`binary`） | 未引入其 wgpu 后端（它要 wgpu 29/30，本仓是 27） |
| 架构门 | `architecture_edges.rs` BUDGET 登记 `rustlua-assets-import → spine` | 另加 `rustlua-scripting` 的 **dev-dependency**（快照门要装同名表） |
| 组件 | `typed_scene.rs`：`ComponentId::SpineSkeleton` + `ALL` + `as_str` + `component_id_for_opaque` | 一致 |
| 组件 schema | `component_schema.rs`：`SPINE_SKELETON_FIELDS`（10 字段）+ 注册 | 一致 |
| 组件校验 | **`rustlua-spine/src/scene_codec.rs`**（不是 `rustlua-component-codec`） | 校验函数住归属 crate，注册表只存函数指针——与 `render`/`ui` 同构 |
| 渲染数据 | `rustlua-render/src/spine_mesh.rs` + `frame.rs` 的 `SceneMesh.spines` | 一致；u16 索引以兼容 WebGL2 |
| pass 对账 | **未新增 `RenderPass::Spine`** | 折进 `WorldSprite` 通道后半段，避免动 12 条 pass、时间戳布局与构造期顺序门；`world_sprites` 事实加上 `spines.draws.len()` |
| GPU | `rustlua-gpu/src/renderer/world_sprite.rs`（复用 world sprite 管线/绑定组/深度） | 懒创建 VERTEX/INDEX buffer，空帧零成本；非 Normal/PMA 明确 `record_dropped_draw` + warn |
| 宿主 | `script_packs.rs`（`spine` 表 + 装配器 + 切场景钩子）、`schedule.rs`（`spine_advance`）、`frame_data.rs`（`collect_spine_mesh`） | 一致 |
| Lua | `rustlua-spine/src/spine_lua.rs` 8 个函数 + 两份基线 + 两份 `.d.lua` + `docs/18` + `doc/spine.md` | `bounds` 返回 table（非多返回值），避免 `local b = spine.bounds(id)` 只拿到第一个值 |
| 资源 | `resource_kind.rs`：`ResourceKind::Spine`（`.spine`/`.skel`/`.spineatlas`）、`dependency.rs` 的 `extract_spine_manifest_refs`、`SpineImporter` | 图集改名 `.spineatlas`，避开引擎自有 `.atlas` |
| 打包/热重载/编辑器 | **P1 未做** | 按决策 2，编辑器预览归 P3；打包白名单随 P2 一起补 |
| 样例/文档 | `examples/game_api`：`api_spine.scene` + `demo_spine.lua`（5 步）+ 资产 3 件 + `capabilities.lua` 2 条 + `demos.lua` | 一致；示例资产取官方 spineboy（`4.3.75-beta`） |

**P1 门禁实测**

| 门 | 结果 |
|---|---|
| `cargo check --workspace` | 通过 |
| `cargo check -p game_web --target wasm32-unknown-unknown` | 通过 |
| `cargo check -p game --no-default-features` | 通过 |
| `rustlua-spine` / `assets-import` / `render` / `assets` 库测试 | 458 passed / 0 failed |
| `engine` / `scripting` / `ecs` / `scene` / `infra` 库测试 | 749 passed / 0 failed |
| `lint`（`rustlua-spine` clippy `-D warnings`） | 通过 |
| `architecture_edges` | 7 passed（新边已登记理由） |
| `component_schema_parity` / `typed_scene_parity` | 通过 |
| API 面快照（全量 + 2D） | 52 表 / 442 函数，与基线一致 |
| `check-api-2d.ps1`（4 段） | 全绿 |
| `gen-lua-api-docs --check` | no-op、桩数 282 不超标、无死绑定 |
| `sample_project`（示例工程 7 条门，全量与 2D） | 18 passed / 0 failed |
| `check-doc-claims.mjs` | tables=52 / funcs=442 / components=32 |

**已知遗留**（均与 Spine 无关，属先前会话未完成的其他工作）：
`assetc` 有 2 条用例卡在 `flame_blade_slash.prefab` 的 `kind=effect` 记录不被解析器接受；
`rustlua-visual-script/src/effect_prefab.rs` 有 5 条 clippy 报错。二者都不在 M84 范围内，
Spine 侧不依赖它们。

## 6.2 P2 实现结果（2026-11-19 完成）

### ① clipping 导入期拒绝

`rustlua-spine` 新增 `validate::reject_unsupported_features`，`SpineImporter` 在校验 `.skel` 后调用。
诊断带 `skin / slot / attachment` 三处定位与手动修复建议，是**导入期硬错**而非运行时静默跳过。

关键判断：`spine2d` 的解析器**完全支持** clipping（能读能建 skin），错的是我们的渲染管线。
所以筛查必须在解析**之后**按类型枚举，不能指望解析器报错——否则会误伤"能读但画不对"这一档。

**示例资产因此改用它自身的 ess 版**：官方 spineboy 的 pro 版带一个把头部框进 `head-bb` 的
clipping 附件，接上拒绝后它会被正确地拒绝。ess 版只含 region + boundingbox，是当前管线能吃下的
最小完整骨骼（动画集相同：aim/death/hit/idle/jump/run/shoot/walk）。

### ② tint black 语义判定（结论：P1 跳过是对的）

读 `spine2d::render::slot_dark_color_rgba` 得到明确约定，并用官方示例实测确认：

| 情形 | dark_color | 含义 |
|---|---|---|
| 无 dark color | `(0, 0, 0, 1)` | alpha=1 是**关闭标记**，shader 的 dark 项必须 no-op |
| 有 dark + PMA | `rgb × final_alpha`, `a = 1` | 已预乘 |
| 有 dark + 非 PMA | `rgb` 原值, **`a = 0`** | 未预乘，`a=0` 才是"需要 tint black"的判据 |

实测 spineboy-ess 全部 draw 的 dark_color 恒为 `0xFF000000`。**因此 P1 忽略 dark color 对
不含 tint black 的骨架是正确的，没有 bug**。真正需要变的是含 dark 的骨架，判据是 `alpha`
而不是 rgb 是否为黑。已固化为 `tests/tint_black_semantics.rs`（含顶点步长 36 字节的契约断言）。

### ③ blend 变体（Additive 已实现）

- `two_d_pipeline_with_blend` / `build_world_sprite_pipeline_with_blend` 把混合状态参数化，
  原 `two_d_pipeline` 保持"标准 alpha"语义不变（6 个既有调用方零改动）。
- 新增 `world_sprite_additive_pipeline`（+ HDR 变体），**热重载路径同步重建**——否则
  "首帧与重载后"是两条不同混合路径，画面对不上且极难查。
- 混合方程按官方 GL 语义：`src*src_alpha + dst*1`，alpha 通道同样相加。已加单测钉住，
  并加"必须与 `ALPHA_BLENDING` 可区分"的反面对照。
- 绘制循环按 blend 切管线（同实体通常只切一次）。**PMA 仍明确拒绝并计数**（需要预乘变体
  shader，不在本轮）；Multiply/Screen 同理，都不静默错色。

**验证资产**：示例骨骼的 additive slot 恰好没挂附件（不产几何），因此构造了
`tests/fixtures/additive_slot_sample.json`——把 region 挂到 additive slot 上，
证明非 Normal 的 draw 真的能出现在中性输出里。

### ④ 多图集页与 AssetHandle

发现并修掉一个**真实 bug**：atlas 里的页名是**相对 atlas 文件**的（`spineboy.png`），
而引擎纹理表按**工程内相对路径**索引。原实现直接透传页名——示例能跑只是因为 atlas 与 png
同目录且路径恰好能被工程根找到。若工程根下恰好也有同名文件，会**静默贴错图**。

修法：`BackendData` 记录 `atlas_dir`（由宿主从清单的 `atlas=` 取出），输出时把页名解析成
`<atlas_dir>/<页名>`；页名已带目录时原样保留（有些导出工作流会把目录写进 atlas）。
双页夹具验证两张页各自指向自己的路径；宿主侧每条 draw 各自走 `texture_id_of`，
因此每页都拿到独立的 `AssetHandle`。

### P2 门禁实测

| 门 | 结果 |
|---|---|
| `cargo check --workspace` | 通过 |
| `cargo check -p game_web --target wasm32-unknown-unknown` | 通过 |
| `rustlua-spine` 全部测试（含 5 个新测试文件） | 通过 |
| `sample_project`（7 条门） | 18 passed |
| API 面快照 / `gen-lua-api-docs --check` | 通过 |
| `architecture_edges` | 7 passed |
| `check-doc-claims` | 52/442/32 |
| `rustlua-spine` clippy `-D warnings` | 通过 |
| scripting/ecs/scene/render/assets 库测试 | 808 passed |

**新增测试**：`clipping_gate.rs`、`tint_black_semantics.rs`、`blend_output.rs`、`multi_page.rs`、
`construct::additive_blend_tests`（GPU 侧）。

**环境说明**：`rustlua-gpu` 的 327 个测试在本机走软件渲染（单个测试约 30 秒），全量串跑
会超时/被 SIGTERM，属环境限制而非代码问题；已逐个验证与本次改动相关的关键路径（HDR 能力
判定、帧视图、depth_sort、additive 方程）均通过。

## 6.3 P3 实现结果（2026-11-19 完成）

### P3-0 先验证，结论与预期相反（重要修正）

计划里写"打包可能不含 Spine 资产，会在真机才炸"。**实测不成立**，如实记：

| 检查 | 结果 |
|---|---|
| `assetc scan` / `list` | Spine 四件套**全部正确注册**（`.spine`/`.skel`/`.spineatlas` → `spine`，`.png` → `texture`） |
| `assetc package --bundle single` | 6 条 Spine 条目全在包内（117 entries） |
| `assetc package --bundle grouped` | 全在，落在 `assets` 组 |
| `assetc closure` | `assetc_closure_ok`，闭包完整 |
| 依赖边 | `.spine` 清单的 `skeleton=`/`atlas=`/`page=` 全部成边（已加单测钉住） |
| 失效传播 | 改 `.spineatlas` 内容 → 该文件 `imported`、其余按指纹 `reused` ✔ |

**真正的问题在压缩白名单**：`is_compressible_asset` 不含 `.skel` / `.spineatlas` / `.spine`，
所以二进制骨骼以原始字节进包。实测收益（官方 spineboy）：

| 文件 | 原始 | gzip | 收益 |
|---|---|---|---|
| `.skel` | 20,440 | 13,922 | **-32%** |
| `.spineatlas` | 1,636 | 627 | **-62%** |
| `.spine` | 791 | 586 | -26% |

大型工程骨骼动辄数百 KB，这是实打实的浪费。已加入白名单，并实测压缩产物
（`.skel` 20440→13957、`.spineatlas` 1636→647）。

### P3-1 压缩白名单

`assetc/src/package.rs::is_compressible_asset` 增加 `skel` / `spineatlas` / `spine`。
三者都是 gzip 通用压缩，**不含格式知识**——运行时按清单标志解压，不需要认识 Spine 类型。

刻意**不**加 `.atlas`：那是引擎自有图集格式，有自己的 cook 通道；Spine 图集改名
`.spineatlas` 正是为了不被这里误伤（已在测试里钉住这条区别）。

**新增门** `rustlua-spine/tests/pack_round_trip.rs`：`pack::encode` → `decode` → 真实解析 → 出 draw。
只看往返不看体积——压缩链断掉的症状是"进游戏黑屏"，不是"体积小了"。

### P3-2 依赖失效与热重载

- 依赖边：`extract_spine_manifest_refs` 已覆盖三类路径行；新增单测
  `spine_manifest_refs_skeleton_atlas_and_pages` 并断言 `spine_version` **不**成边。
- 失效传播已实测生效（见 P3-0 表格最后一行）。
- `file_watcher` 早已监听 `assets/`，改骨骼/图集会产事件，无需改监听范围。
- **骨架运行时重建**与编辑器预览是同一件事，归编辑器的只读预览一并处理。

### P3-3 VFS 路径解析

**P1 就已满足**，无需改动：Spine 的清单、骨骼、图集全部经
`rustlua_infra::vfs::read_resource` 读取，该函数内部走 `pack::decode`（自动解压）且
wasm32 可用。已由 `pack_round_trip` 测试覆盖往返，`game_web --target wasm32` 编译通过。

### P3-4 编辑器 / 平台 / 文档

**编辑器（只读预览）**：
- `COMPONENT_REGISTRY` 登记 `SpineSkeleton`（类别 `Rendering`），Add Component 菜单与
  「组件」菜单自动出现该项。
- `insert_default_component` 默认载荷 `data=assets/spine/spineboy.spine`——**不用**
  `::default()`（`data` 为空串会在装配期直接报错，"添加组件即可见"是面板的基本契约）。
- 通用字段编辑器自动接管（`SPINE_SKELETON_FIELDS` 十个字段在 P1 已注册）；
  `schema_fields` 的期望清单同步加入 `SpineSkeleton`。
- **不做**骨骼编辑/关键帧编辑/`.spine` 导出——见下节许可边界。

**许可边界固化**：新增 `rustlua-spine/tests/license_boundary.rs`，用源码扫描断言两件事：
① 运行时不含 `write_skel`/`export_skel`/`save_skel`/`write_atlas`/`export_atlas` 之类的导出 API；
② 编辑器只造 `SpineSkeletonComponent` 载荷串，不直接触 `SkeletonData` / `SpineData::from_bytes` /
`spine2d::`。这类"不该存在的功能"只能用"不该出现的符号"来表达，靠人工 review 太晚。

**文档**：`docs/THIRD_PARTY.md` 新增 Spine 两节（运行时库 + 示例资产），并写明
Spine Runtimes License 的三条约束与它导致的实现边界；记录示例为何用 **essence** 版而非 pro。

**平台**：`game_web --target wasm32-unknown-unknown` 编译通过。微信/WebGL2 的运行时几何
验证需真机，本轮**未验**——保持如实口径，不虚报。

### P3 门禁实测

| 门 | 结果 |
|---|---|
| `cargo check --workspace` | 通过（0 error） |
| `game_web --target wasm32-unknown-unknown` | 通过 |
| `rustlua-spine` 全部测试（8 个测试文件） | 18 passed |
| `rustlua-assets` 库测试 | 105 passed |
| `editor` 库测试 | 42 passed |
| `sample_project`（7 条门） | 18 passed |
| API 面快照 / 架构边 / `check-doc-claims` | 全部通过 |
| `assetc package`（single / grouped / 压缩） + `closure` | 全部 `_ok` |
| `rustlua-spine` clippy `-D warnings` | **0 告警** |

### 遗留（**非 M84**，未动）

`assetc` 的 2 条用例（`sample_project_import_checkdeps_and_package_pass` /
`sample_project_rename_breaks_dependencies_until_consumers_are_updated`）卡在
`examples/game_api/assets/fx/flame_blade_slash.prefab` 的 `kind=effect` 记录不被
`rustlua-scene` 的 prefab 解析器接受。该格式由 **M88（`rustlua-visual-script::effect_prefab`）**
引入，解析支持属于 M88 的范围；`rustlua-visual-script` 另有 5 条 clippy 报错同属该批工作。
M84 不依赖它们，也未顺手修改以避免与并行工作冲突。

## 6.4 P4 实现结果（2026-11-20 完成）

### P4-0 前置：GPU 测试"跑不完"的根因（先修工具再谈性能）

P4 要出性能读数，前提是基准环境可信。上一轮曾把"GPU 测试跑不完"归因为环境限制，**判断错了**。
实测（`cargo run -p rustlua-gpu --example gpu_probe`）：

```
[0] NVIDIA GeForce GT 1030  Vulkan  DiscreteGpu     <- 独显一直在
[2] Microsoft Basic Render Driver  Dx12  Cpu        <- 软件光栅化
force_fallback_adapter=true  -> Microsoft Basic Render Driver (Cpu)
force_fallback_adapter=false -> NVIDIA GeForce GT 1030 (Vulkan)
```

仓库里 10 处 `test_device()` 硬编码 `force_fallback_adapter: true`，**每个 GPU 测试都在软件
渲染上跑**，独显全程闲置。已收口为 `RustLuaGPU::test_adapter::request_test_adapter()`，
由 `RUSTLUA_TEST_GPU` 控制（`software` 默认 / `hardware` / `strict`），**默认行为不变**。

| 条件 | depth_sort（2 个测试） |
|---|---|
| 软件（旧） | 18.2 s |
| 硬件 | 2.7 s |

另有一条独立原因：GT 1030 只有 2 GB 显存，默认并行度下几十个测试同时建设备+纹理，
显存与驱动队列争用，单测试从 27 s 涨到 >60 s 未结束。实测 `water` 模块：
`--test-threads 8` 跑不完，`--test-threads 2` **101 s 全过**。正确跑法见
`scripts/verify/gpu-tests.md`。

**顺带修掉一个假门**：`hdr::ssr_misses_fall_back_to_the_procedural_sky` 断言 SSR 参数缓冲在
`construct/mod.rs`，但拆分批后已搬到 `construct/post_process.rs`——这条断言在**软件档和硬件档
都恒定失败**，被"整体跑不完"掩盖。已修正。

### P4-1 基准（`crates/rustlua-spine/examples/spine_bench.rs`）

固定场景 1 / 10 / 100 个角色，样本 300 帧（预热 30 帧），release 构建。
骨架用官方 spineboy（essence 版，region 72 顶点、单页图集）——**刻意不用手写夹具**：
夹具很容易让 region 名与 atlas 页对不上，那样每帧都走"页表查不到"的兜底分支，
量到的就不是生产路径（这个坑本轮踩过一次，见下）。

| 角色数 | draw | 顶点 | p50 | p95 | p99 | 每帧分配 | 其中上游 | 我们 |
|---|---|---|---|---|---|---|---|---|
| 1 | 1 | 72 | 7 µs | 7 µs | 8 µs | 110 | 110 | **0** |
| 10 | 10 | 720 | 79 µs | 104 µs | 139 µs | 1,100 | 1,100 | **0** |
| 100 | 100 | 7,200 | **801 µs** | 1,360 µs | 1,439 µs | 11,000 | 11,000 | **0** |

**60 fps 预算 16,667 µs**：100 个角色占 **4.8%**（p50），p99 占 8.6%。骨架采样本身不是瓶颈。

报告：`artifacts/m84-spine-bench.json`（`report_version: m84-spine-bench-v1`）。

### P4-2 稳态零分配：**部分达成，且达成的是我们这侧**

基准把每帧分配拆成三段（`state_only` / `draw_list_only` / `copy_only`）：

| 来源 | 单角色每帧 | 能否消除 |
|---|---|---|
| `spine2d` 状态机 + 骨骼采样 | 73 | **不能**（上游库内部） |
| `spine2d::append_draw_list_with_atlas` | 37 | **不能**（上游库内部） |
| 我们的 `copy_to_output` | **0** | 已达成 |

做法：
1. `SpineDraw.texture` 由 `String` 改为 `Arc<str>`，页路径在**加载期**解析并驻留
   （`BackendData::page_paths`），逐帧只查表 + 引用计数自增。
2. `copy_to_output` 不再 `out.clear()`（那会把 len 归零、失去复用槽位），改成
   "按需覆盖 + 尾部截断"，稳态下完全不碰分配器。

于是把目标如实修正为：**我们自己那侧稳态零分配（已达成）；上游 `spine2d` 的每帧 110 次分配
不可消除**，除非替换上游库。写"Spine 路径稳态零分配"是错的，不能这么报。

**顺带发现并修掉**：页表查不到时，原实现每帧现场 `format!` 拼路径——既多分配又会**静默贴错页**。
现在改为 `debug_assert` 点名（开发期立刻暴露）+ 截断该 draw，不静默画错。

### P4-3 全量质量门

| 门 | 结果 |
|---|---|
| `verify.ps1 -Quick` | **Quick gate passed**（含文档一致性/声明、vendor 30/30、工具链 50/50、解耦、fmt、平台一致性、闭包 10 例、微信体积、schema 审计 772 组件 0 错） |
| `cargo check --workspace` | 0 error |
| `cargo fmt --all -- --check` | 通过（本轮代码已格式化） |
| `cargo clippy -p rustlua-spine --all-targets -- -D warnings` | **0 告警** |
| `rustlua-spine` 全部测试 | 15 passed |
| `engine` 库测试 | 267 passed |
| `cargo deny check` | advisories / bans / licenses / sources **全 ok**（含 spine2d 许可证） |
| `game_web --target wasm32-unknown-unknown` | 0 error |
| `cargo build --release`（spine + engine） | 通过 |

**本机环境阻塞（非代码问题）**：`verify.ps1` 里 `test-analyze-soak.ps1` 会因 WorkBuddy
同时注入大写与小写代理变量（`HTTP_PROXY`/`http_proxy`）而 `Start-Process` 字典冲突报错。
清掉重复变量后 `soak_analyze_tests_ok cases=5` 通过。CI 无此问题。

### 遗留（**非 M84**）

- `assetc` 2 条用例卡在 `flame_blade_slash.prefab` 的 `kind=effect`（M88 引入的格式），
  `rustlua-visual-script` 的 5 条 clippy 同属该批。
- `cfg_inventory_drift items=43`：全部是既有存量（hierarchy / shader_graph 等），
  **无一条与 spine 相关**，本轮改动未新增任何 cfg 项。

## 6.5 收口：三条历史遗留的清理（2026-11-20）

P4 完成后剩三条与 M84 无关的红门，会干扰后续任何资源相关验证。已逐条清掉，
记录在此是为了让"当时的判断"可复查。

### ① effect prefab 导入失败（assetc 2 条用例长期判红）

**根因**：`.prefab` 一个扩展名承载两种格式——实体预制体（`entity`/`component` 记录）与
特效预设（`kind=effect` + `graph=`/`overlay=`/`param<TAB>名<TAB>值`）。`PrefabImporter::validate`
无条件按实体预制体解析，于是每个特效预设都在导入期报 "unknown record"。

**判断**：它从来没打算被 `Prefab` 认识——运行时走 `EffectPrefab::load`
（`engine::host::sample_project`），根本不经过解析器。这是个**误判**，不是资产问题。

**排除的两种修法**：
- 让 `Prefab::from_text` 认识 effect 格式 → 一个类型承载两套语义，改一处会踩另一处；
- 让 `rustlua-scene` 依赖 `rustlua-visual-script` → scene 是运行时也要编的（模块文档有约束）。

**实做**：在 `PrefabImporter::validate` 里按**内容**的 `kind=effect` 分流放行（扩展名区分不了，
只能看内容）。语义校验留给运行时 `EffectPrefab::load`（自带行号诊断）。补了双向测试：
`kind=entity` 仍必须走实体校验，防两种格式混为一谈。

**结果**：assetc **68 passed / 0 failed**（此前 66/2）。

### ② visual-script clippy 5 条

`effect_prefab.rs` 两处 `tabs_in_doc_comments`（文档里用制表符示例真实格式 → 换等价空格）、
一处 `double_must_use`（`Result` 已自带 `must_use`）。现在 `-D warnings` **0 告警**。

### ③ cfg 覆盖率门（`cfg_inventory_drift items=43`）

**分类结论：43 条全部是清单过期，零代码问题。**

| 类型 | 数量 | 性质 |
|---|---|---|
| added | 30 | 新增 cfg 站点（`test` 418→431、`feature` 90→94） |
| removed | 13 | **全是文件重构换路径**：`hierarchy.rs`→`hierarchy/mod.rs`、`construct.rs`→`construct/mod.rs`、`script/api/mod.rs` 的 3D 注册收敛进 `domain` 别名（`domain` 是 `pub(crate) use rustlua_ecs as ecs` 门面，那些 `feature="3d"` 实际在 `rustlua_ecs`） |

`cargo check --workspace` 0 error 是佐证：真删了东西编译会炸。

**但重新生成清单后，第二环抓出了真问题**：

```
[RED] rustlua-ui-host-harness 有 target_arch="wasm32" 分支，
      但不在任何 wasm cell 的包/闭包
```

`rustlua-ui-host-harness` 是 Web/微信 UI 证据 harness 的 cdylib 独立入口
（`scripts/verify/ui-host-evidence.ps1` 确实会 `cargo build --target wasm32`），
但它**从没被任何门编译过**——4 个 wasm 分支等于不存在。与 `gpu-web-probe` 同类：
独立 wasm 包，不在 `game_web` 闭包，必须显式列出。

已加进三处（必须逐字对齐，`check-platform-parity.ps1` 会判红）：
`metrics/wasm-cells.json` 的 `wasm-default`、`.github/workflows/matrix.yml` 的 3 个 wasm cell、
`scripts/verify/verify.ps1` 的 wasm check。

**结果**：`cfg_coverage_ok wasm_attrs=74 covered=74`（此前 70/73，1 红）、`platform_parity_ok`。

**教训（值得单记）**：看 cfg 门不能只看第一环 `cfg-inventory --check`。清单对齐只是让
"账面一致"，真正的问题在第二环"这些分支有没有被编译过"。

## 7. 风险与边界

1. **许可证**：运行时集成免费不等于无义务。发布物必须带 Spine Runtime License 与版权声明；
   每个使用者需自有 Spine Editor 授权；编辑器不得提供骨骼创建/编辑能力。法务口径以
   `https://esotericsoftware.com/spine-runtimes-license` 最新文本为准（2026-11-14 版）。
2. **工具链锁定**：不要为了新 crate 升 Rust 版本；升版本会连带 CI、`tools.json`、性能基线
   重签。若 P0 发现候选 crate 实际 MSRV > 1.97.1，直接换备选而不是升工具链。
   同理不要走 C FFI：仓库当前只有 vendored Lua 一处 C 依赖，业务 crate 没有 `cc`/`bindgen`；
   wasm32 走了 `+exception-handling,+reference-types` rustflags 与 Zig 编译的 Lua shim，
   再叠一套 C 库要重验 wasm-bindgen glue（web 与微信两种 target）与包体配额，成本远高于收益。
3. **运行时成熟度 / 版本边界**：已定冻结 Spine 4.3.x 与“Spine Editor 小版本 ↔ 运行时 crate
   版本 ↔ 示例骨骼哈希”三元组（§4.1）。两个纯 Rust 4.3 实现都自称实验性，故不承诺向前兼容
   未来 4.4/5.x 数据；升级按 §4.1 的 backend 隔离层逐步替换并对拍，不推断兼容性。
   **P0 实证风险已裁决**：spine2d 与 chine 曾在此差 1.8–24 px（idle/run/walk/jump），
   P1 前置门用官方 runtime golden 证明 spine2d 一致、chine 为偏差方（`M84-02`）。
   升级到 4.4/5.x 时仍须重跑 golden 门，不能把当前结论外推到新版本。
4. **渲染复杂度**：stencil 裁剪会动到 2D 深度附件格式与 WebGL2 能力探针。按已定决策，
   P2 先以“导入期明确拒绝 clip attachment + 诊断”交付，stencil 留作后续增量；未实现前
   不能画一半，也不得静默丢片。
5. **排序语义**：Spine 的 draw order 与引擎的 layer/z 排序属于不同维度。约定“实体内原序不可变，
   实体间按 layer/z”，并写进单测，否则换纹理合批时会静默错乱。
6. **状态生命周期**：场景切换、资源热重载、实体销毁后必须释放骨架实例并清事件队列，
   沿用 `AnimationState::clear_for_scene_transition` 的模式。
7. **质量门成本**：新增 Lua API 会同时触发 API 面快照、2D 面快照、类型提示、示例演示、
   能力图谱与文档门；这些是硬门，必须同批更新，不能留到“收尾”。

## 8. 决策记录（2026-11-19 已确认）

| # | 决策点 | 结论 | 对方案的影响 |
|---|---|---|---|
| 1 | 运行时定案 | 默认 `spine2d 0.5`，P0 对拍为最终门槛；失败换 `chine 0.2` | §3 表格与定案门槛已改；切换只动 `backend/`，不动上层接口 |
| 2 | 编辑器预览范围 | 放到 **P3**，P1 不做 | P1 只需播放 + Lua 切换 + 事件；P3 做只读静态姿势预览 |
| 3 | clipping | P2 先“导入期明确拒绝 + 诊断”，stencil 留作后续增量 | 导入期必须有明确报错；不要静默丢片，也不要在 P2 写半套 stencil |
| 4 | Spine 版本 | 冻结 **4.3.x**，预留升级通道 | 新增 §4.1：清单记 `spine_version`、`backend/player` 分层、三元组回归基线；不承诺 4.4/5.x |

以上四点均已确认，无待拍板项；实现阶段若需偏离（例如 P0 对拍失败），按 §3/§4.1 的既定
回退路径执行并回写本表，不另开决策。

## 附录：外部事实来源

- Spine 官方运行时仓库与许可：<https://github.com/EsotericSoftware/spine-runtimes>、
  <https://esotericsoftware.com/spine-runtimes-license>
- `spine2d 0.5`：<https://crates.io/crates/spine2d>、<https://docs.rs/spine2d/0.5.0/spine2d/>
- `chine 0.2`：<https://crates.io/crates/chine>、<https://docs.rs/chine/0.2.0/chine/>
- `spine_runtime 0.2.2` / `rusty_spine 0.8`：crates.io 对应页面（MSRV 与许可证依据）
