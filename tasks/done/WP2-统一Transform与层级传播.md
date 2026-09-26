# 任务：WP2 统一 Transform 与层级传播（原型 → 可接入门）

- 任务 ID：WP2（工作包级；子任务 WP2-1 ~ WP2-6 可各自认领）
- 状态：**功能与证据全部完成（2026-09-17）；负责人本轮已裁定"所有场景 parent 一律按局部空间
  解释"（含迁移写回），进入实施（2026-09-18）**
- 关联里程碑：引擎全面重构 [docs/40](../docs/40-引擎全面重构计划.md) §3 WP2；设计原型 [docs/45](../docs/45-WP1-WP2设计与原型.md)；架构目标 [docs/38](../docs/38-统一2D3D核心架构实施计划.md) §3.1
- 创建日期：2026-09-17
- 前置依赖：WP0（已关闭）；WP1-1（typed 组件覆盖，本包需 typed 父拓扑）

**第 2 步：运行时接线（本轮完成，开关默认关闭）**。

- `core/hierarchy.rs`（新）：`Parent(EntityId)` 运行时组件、`WorldTransform`/`WorldTransform3D`
  传播结果、`propagate_spatial(world, enabled)`（显式栈求父先序，不是递归——环不会栈溢出而是
  变成诊断）、`world_transform_2d/3d`（有传播结果用它，否则退回局部）、`world_matrix_3d`。
  **组合走矩阵再分解**：非均匀父缩放 × 子旋转会产生剪切，TRS 表达不了 ⇒ 该实体**保留局部值**
  并记一条 `NotRepresentable` 诊断，绝不近似。
- 实例化时把场景 `parent` 落成 `Parent` 组件（**第二趟**做：子可能先于父出现；父被跳过时
  不建立关系，宁可不继承也不指错）。
- `RuntimeScene`：`spatial_inheritance` 开关（**默认关闭 = 旧行为**）+ `set_spatial_inheritance`
  /`propagate_spatial_now`/`last_spatial_report`；传播发生在 `run_script_steps` 末尾与 `vertices()`
  帧首（3D 提取是 `&self`，不能自己触发传播，所以必须在步末做）。
- Lua：`transform.world(id)` 读世界值（`transform.get` 仍是局部值）。父子关系下这是唯一能拿到
  "屏幕上实际在哪"的接口。
- **回滚档就是开关本身**：关闭时 `propagate_spatial` 只做两次"有没有世界变换组件"的查询，
  有才清理（首次运行后即无），逐帧成本可忽略——`wp0_scene_frame` 帧阶梯在关闭状态下复测无回归
  （10k 905 µs / 50k 3,990 µs，与替换后基线同量级）。

**证据**（5 条核心 + 2 条运行时 + 1 条 Lua，engine lib 650 项）：关闭不产生世界组件且读写等价；
开启后父缩放/旋转正确作用到子（2D 组合 (3,5)、3D 绕 Y 90° 后 +X → −Z）；含剪切的组合被拒绝
且退回局部；成环逐条报告不挂死；`scene_parents_become_runtime_parent_components`；
`spatial_inheritance_switches_between_local_and_world_positions`（开关往返后与原值完全相等）；
`lua_transform_world_differs_from_local_under_inheritance`。

**第 3 步（下一步）**：把提取路径改为消费世界变换（精灵 / 世界文字 / 相机跟随 / 碰撞体调试 /
编辑器拾取），跑 `migrate_scene_to_local_space` 写回示例场景并在**开启继承**下核对 2D overlay
golden 与 3D semantic golden 逐项不变，最后把默认值翻成"开启"（回滚仍是这个开关）。

**第 3 步：提取路径消费世界变换（本轮完成）**。

开关**每帧选定一次**遍历用的组件：开启继承时整条换成 `WorldTransform` / `WorldTransform3D`，
关闭时仍走 `Transform`。刻意不做"每个实体先试世界再退回局部"——那会给每个精灵多一次 `get`
（两层哈希），WP6 已量到那正是 5 万精灵档的大头。支撑这条的是一个**不变量**：开启继承时
**每个有局部变换的实体都有世界变换**（组合不了或成环的保留局部值当世界值并记诊断），因此
整条替换不会漏画任何一个精灵。

已切换的提取路径：2D 精灵收集、3D drawable（model 矩阵）、世界文字锚点、瓦片层、相机跟随目标。
（碰撞体调试绘制读的是物理世界的真实姿态，本来就是世界空间；屏幕 UI 与布局是屏幕空间，不参与。）

**顶点级"迁移无损"证据**（`migrated_child_renders_at_the_same_place_with_inheritance_enabled`）：
同一份"子按世界坐标写"的场景，A 关闭继承直接渲染；对同一场景跑**真实迁移**
`migrate_scene_to_local_space` 把局部值写回，B 开启继承渲染——两帧顶点位置（<1e-4）与 UV
逐个相同，且顶点数相同。这条把"迁移 + 继承"的效果钉在顶点级，而不是只比较数值。

**第 4 步（下一步，WP2 收口）**：迁移命令（dry-run → 写回 → 备份 → 一键回滚）落到 26 个示例
场景，在**开启继承**下重跑 2D overlay golden 与 3D semantic golden 逐项核对，然后把默认值翻成
开启（回滚仍是 `set_spatial_inheritance(false)`）；同时把编辑器 2D 拾取包围框从"文档里的局部
值"改为世界值（它现在读 `TransformFields`，开启继承后会与渲染错位——默认关闭期间不影响）。

**第 4 步：迁移命令 + 默认值翻成开启（本轮完成，WP2 收口）**。

- `core/scene_migration.rs`（新）：`migrate_scene_file(path, options)`（默认 **dry-run**；`write`
  时先留 `<file>.bak` 再写回，并写 `<file>.spatial-local` 标记）、`restore_scene_backup(path)`
  （一键回滚：拷回 `.bak`、删备份与标记）、`project_scene_files(root)`（`scenes/**/*.scene`，
  排序保证报告可复现）。
- **迁移不可重复施加**（数据迁移后已是局部坐标，再按"局部=世界"解释一次会把父变换又乘一遍），
  因此写回留下标记文件：`write` 模式看到标记就跳过——命令可以安全地反复运行。
- 编辑器 CLI：`--migrate-spatial`（dry-run 报告）/ `--migrate-spatial-write`（写回）/
  `--restore-spatial`（回滚）；有实体被拒绝时输出里带 `refused=`，便于当质量门用。
- **默认值翻成开启**：`RuntimeScene::spatial_inheritance` 默认 `true`（裁定要求"所有场景 `parent`
  一律按局部空间解释"），同时新增 `spatial_active()`——**只有真的存在 `Parent` 时**才传播并让
  提取路径读世界变换；平面场景（现有全部示例场景）因此零成本、零行为变化。回滚档仍是
  `set_spatial_inheritance(false)`。

**证据**：

| 项 | 结果 |
|---|---|
| 迁移命令 dry-run/写回/备份/回滚/重复运行 | 4 条测试（含"重复运行不改文件"与"备份始终是最原始那份"） |
| 仓库级**两种语义对拍** | `shipped_example_scenes_render_the_same_under_both_parent_semantics`：`game_api`(18) + `game_jhyt`(8) 全部场景，逐实体断言"父是组织关系"与"父是局部空间"两种解释下世界位置相同 ⇒ 翻默认值不改变现有内容 |
| CLI 实测 | `editor --project examples/game_jhyt --migrate-spatial` = `files=8 converted=0 refused=0`；`game_api` = `files=18 converted=1 refused=0`（唯一一处是 `index.scene` 的 UI 子实体，父子变换都是单位阵，世界位置不变） |
| 帧阶梯（默认开启后） | 1k 170 µs / 10k 878 µs / 50k 3,960 µs、每帧分配 61 次——与关闭时同值（平面场景不产生传播成本） |
| 门 | engine lib **656 项**、editor 109 + 165、clippy/fmt/build/check-docs/`verify.ps1 -Benchmarks` 全绿 |

**仍未做（登记）**：编辑器 2D 拾取包围框读的是文档里的局部值（开启继承后会与渲染错位）；
编辑器自身目前不打开继承，所以不影响当前使用。收口方式是让拾取走同一份 `world_transform_2d`。

## 裁定后的实施进度（2026-09-18）

**第 1 步：迁移覆盖 2D 与 3D（本轮完成）**。原 `migrate_v2_hierarchy` 只认 `Transform3D`，
而真实场景里 **2D 实体占多数**——只看 3D 会把它们全部当成"无变换"跳过，等于"所有场景按局部
空间解释"这条裁定落不到 2D 上。本轮补齐：

- `scene_entity_local(entity)`：2D `Transform` 优先、其次 `Transform3D`，`migrate_v2_hierarchy`
  与新的写回路径共用这一份判定；
- `migrate_scene_to_local_space(scene) -> (SpatialMigration, MigrationReport)`：dry-run 语义，
  直接给出**每个实体应写回的组件串**（`Transform` / `Transform3D` 各自的规范编码）以及
  `detached`（必须清掉 `parent` 的实体）；
- `Transform::to_component_str()`：与 `Transform3D::to_component_str()` 对称的 2D 编码器
  （9 位小数，与编辑器落盘格式一致，避免迁移产生无谓文本差异）；
- `plane_local(local)`：3D 局部变换 → 2D `Transform` 的"压得平才转"判定（z 位移/缩放、非绕 Z
  旋转一律拒绝）。

**证据**（3 条新测试，engine lib 642 项）：

| 测试 | 断言 |
|---|---|
| `two_d_migration_keeps_world_positions` | 父 `x=10;scale=2`、子世界 `x=3` ⇒ 子局部 **x=-3.5**（= (3-10)/2），把迁移结果当局部值做父先序传播后世界位置仍是 (3, 5)；根实体也写回规范串；`detached` 为空 |
| `two_d_migration_refuses_sheared_children_and_keeps_them_as_roots` | 父旋转 45°+非均匀缩放 ⇒ 子世界矩阵含剪切 ⇒ **拒绝迁移**、进 `detached`、保持为根（世界位置不变），不做近似 |
| `migration_preserves_world_positions_on_project_scenes`（既有，已随 2D 覆盖扩大） | 26 个真实源场景逐实体世界位置不漂移 |

**下一步（第 2 步）**：运行时接线——`RuntimeScene` 持 `TransformTree`（2D/3D 同树）、提取
（精灵/世界文字/相机/碰撞体/拾取）改为消费**世界**变换，入口开关 `set_spatial_inheritance(bool)`；
随后跑 `migrate_scene_to_local_space` 写回示例场景，并在**开启继承**的前提下核对 2D overlay
golden 与 3D semantic golden 逐项不变（这就是"迁移无损"的最终证据）。

## 负责人裁定（本轮）

**所有场景的 `parent` 一律按局部空间解释**（不是"仅新场景显式声明"）。执行要求：① 运行时
提取与拾取消费 `GlobalTransform`（父先序传播），2D/3D 语义一致；② 迁移必须先行——`parent`
变成局部空间后，既有场景里"父子位置都按世界坐标写"的实体世界位置会变，因此**先跑
`migrate_v2_hierarchy` 并把结果写回**（被拒绝/跳过的实体必须清掉 `parent`，否则它们的世界
位置会在新语义下改变，破坏"迁移无损"）；③ 证据：26 个真实源场景的迁移前后世界位置对拍 +
3D semantic golden + 2D overlay golden + 全部 engine/editor 测试；④ 回滚档：`parent` 解释
方式保留一个运行时开关（`set_spatial_inheritance(bool)`），关掉即回到"parent 仅组织关系"。

## 目标

把 Local/Global 变换从"正确性优先的原型"推进到可接入运行时：父先序传播在 2D/3D 等价、可诊断、可回滚，并且通过 3D 不回退门后才允许接线。

## 0. 现状核实（2026-09-17，以源码为准）

| 事实 | 依据 |
|---|---|
| 原型是**未被引用的死代码** | `engine/crates/engine/src/core/mod.rs:92`（`spatial_transform`） |
| **全仓没有任何世界矩阵传播**：提取器用单实体 TRS 构造 model | `core/scene_extract.rs:89` `transform.model_matrix()` |
| `parent` 在引擎里只被用到一次，且与空间无关（UI 节点键映射） | `host/runtime_scene.rs:2245` |
| 场景只维护结构关系（parent/children），不表达局部空间 | `core/scene.rs:100,276` |
| 校验只覆盖 finite / scale>0 / 单位四元数，**没有剪切判定、没有 TRS 反解、没有 reparent、没有 dirty/version** | `core/spatial_transform.rs:44-53` |
| 深度 100 只有正确性测试，**没有 benchmark** | `core/spatial_transform.rs:141-164` |

## 子任务清单

### WP2-1 深度 1 / 10 / 100 传播 benchmark（约 1 人日）

沿用 WP0 统一 schema（min/average/p50/p95/p99/max + 分配与峰值），补齐 [docs/45](../docs/45-WP1-WP2设计与原型.md) 门槛 1。

验收：三档深度报告落 `artifacts/baselines/`；同机重复误差进比较脚本口径。

### WP2-2 旧 / 新空间对拍（约 1 人日）

对拍维度：全部 3D drawable 的旧 `model_matrix` vs 新 `GlobalTransform`；2D sprite 的屏幕矩形（`core/sprite.rs:400` 走 `Camera2D::world_to_screen`）。

验收：3D 模型矩阵与骨骼 palette 逐项在阈值内；2D 屏幕矩形不漂移（对齐 [docs/40](../docs/40-引擎全面重构计划.md) §8「层级 roundtrip 误差 ≤1e-5」）。

### WP2-3 reparent 保持世界位置 + 诊断贯通（约 1 人日）

新增 reparent keep-world API；循环父链、缺失父、非法局部变换的诊断要贯通到编辑器 Hierarchy 与 Lua。

验收：换父后世界位置保持；循环父链有可定位诊断（entity + 名称），不 panic、不静默丢弃。

### WP2-4 剪切与非均匀缩放策略显式化（约 1 人日）

非均匀父缩放 + 子旋转会产生剪切，无法无损反解回 TRS。必须显式裁定：**无法无损表达时拒绝迁移/拒绝反解**，不得静默近似。同时保证非法矩阵**不上传 GPU**（[docs/39](../docs/39-引擎核心先进性与性能等级复核.md) 校验项）。

验收：剪切场景有明确诊断与拒绝路径；负缩放/非有限值有测试；GPU 上传入口有防御性断言或测试。

### WP2-5 dirty/version 增量传播 prototype（约 1 人日）

在 WP2-1 基准证明全量传播确为瓶颈后，才做增量版本；所有 `get_mut` 写入路径必须覆盖失效机制（[docs/38](../docs/38-统一2D3D核心架构实施计划.md) §P1 要求）。

同时落地关键兼容裁定：**旧 v2 的 `parent` 不自动解释为局部空间**，保持原有世界位置；提供显式"保持世界位置并建立空间继承"迁移选项与差异报告。

验收：增量结果与全量传播逐实体一致；深度 1/10/100 优于全量；迁移幂等且无法无损表达的实体被明确拒绝。

### WP2-6 接入前硬回归（约 1 人日）

[docs/45](../docs/45-WP1-WP2设计与原型.md) 门槛 6：3D golden、物理回写、骨骼 palette 全量回归通过后才允许接入 runtime。

验收：`scripts/verify.ps1 -Benchmarks` 的 3D semantic golden 与 `wp0-3d-stress` 几何门全部通过；3D 任一指标回退即阻塞本包合并（[docs/40](../docs/40-引擎全面重构计划.md) §0「3D 不回退门」）。

## 影响文件

- `engine/crates/engine/src/core/spatial_transform.rs`（主要改动）
- `engine/crates/engine/src/core/scene_extract.rs`（共享空间提取）
- `engine/crates/engine/src/core/transform.rs`（统一 TRS 与校验入口；`transform3d.rs` 已并入，见下）
- `engine/crates/engine/src/host/runtime_scene.rs`（接线与回退开关）
- `engine/crates/editor/src/panels/hierarchy.rs`（换父与诊断显示）

## 风险

- **旧层级语义被误解**：把 v2 `parent` 当局部空间会让旧项目世界位置突变——默认不启用空间继承，只提供显式迁移。
- **剪切无法无损分解**：拒绝而非近似；负缩放另立任务（[docs/38](../docs/38-统一2D3D核心架构实施计划.md) §6）。
- **物理回写绕过单一真源**：物理当前直接改写 `Transform.position`（`core/script.rs:807-810,819-822`），若先于 WP7 接线统一空间，会出现两套写入路径。WP2-6 必须把物理回写纳入回归。
- 相机姿态仍与 Transform 分离（相机自带 position/target/up），统一空间后存在冲突风险，属 WP5 范围，需在接线前声明边界。

## 验收方式

```powershell
cd engine
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace
```

外加：深度阶梯报告、旧新对拍报告、3D golden（`scripts/verify.ps1 -Benchmarks`）。

## 实现记录

### WP2-1 / WP2-2 / WP2-3 / WP2-4 / WP2-5（2026-09-17）

全部落在 `engine/crates/engine/src/core/spatial_transform.rs`，新增 9 个测试（模块内共 12 个）：

- **WP2-4 TRS 反解与拒绝策略**：`LocalTransform::from_matrix`（Gram–Schmidt 取缩放 + 相对剪切判定 + 行列式判定）。含剪切、镜像/负缩放、退化缩放、非仿射一律**明确拒绝**（`DecomposeError`），不做静默近似；容差按缩放归一，避免大缩放下误报。
- **WP2-3 换父保持世界位置**：`GlobalTransform::local_relative_to` + `TransformTree::reparent_keep_world`。拒绝成环（新父是自己或后代）且**拒绝时树不变**；反解失败同样不改动树。
- **WP2-5 脏标记增量传播**：`TransformTree` 维护 `children`/`dirty`/`globals`，`global()` 沿父链只补算脏节点；`recomputes()` 计数暴露实际工作量；`recompute_all()` 作为全量参照。叶子变动只重算 1 个节点。
- **WP2-2 旧新对拍**：`unified_matrix_matches_legacy_model_matrix_on_project_scenes` 对 26 个真实场景的每个 `Transform3D`，逐元素比对统一矩阵与旧 `model_matrix()`（1e-6）。
- **WP2-1 深度阶梯基准**：并入 `wp0-v4`（`transform_depth` 段，纳秒口径，200 cycles）。Release 实测：

| 深度 | 全量重算 | 叶子变动（增量） | 根变动（增量） |
|---:|---:|---:|---:|
| 1 | 400 ns / 6 次分配 | 200 ns / 2 | 200 ns / 2 |
| 10 | 2,800 ns / 24 | 300 ns / 2 | 1,900 ns / 4 |
| 100 | 34,700 ns / 229 | **300 ns / 2** | 21,900 ns / 23 |

深度 100 时增量叶子变动比全量快 ~116×、分配少 ~115×，且耗时与深度无关（真 O(1)）。根变动必须重算整条链，如实报告、不冒充 O(1)。`verify.ps1` 已把两条不变量写进门：叶子变动必须恰好重算 1 个节点、根变动必须恰好重算 `depth` 个。

**对 WP3 决策门的输入**：深度 100 全量传播 34.7 µs、229 次分配，而 `game_jhyt` 的实际父链深度只有 2–4 层——**目前没有证据表明变换传播是瓶颈**。这条数据支持"先不扩大 WP3 改造范围"。

### WP2 迁移工具（2026-09-17）

`migrate_v2_hierarchy(&Scene) -> (TransformTree, MigrationReport)`：把 v2 组织树显式迁移成空间层级，**保持每个实体的世界位置不变**（把父世界矩阵的逆烘进子局部变换，而不是把旧局部值当成相对父节点的坐标）。只读输入、只返回新数据（dry-run 语义），写回由 WP9 决定。无法无损表达为 TRS 的实体**拒绝迁移**并逐条报告，且退回根——世界位置因此仍然不变。

新增 3 个测试：迁移保持世界位置（并显式断言不得漂移到"父当作局部"的 x=16）、剪切场景拒绝且世界位置不变、26 个真实场景的世界位置逐实体不变。

---

## 模块收敛（负责人裁定，2026-09-18）

`transform3d.rs` 已并入 `transform.rs`：2D 与 3D 变换（含各自的组件串解析错误类型）现在同址，
并新增 **2D ↔ 3D 无损互转**：

- `Transform::to_3d()` 永远无损（z = 0、绕 Z 旋转、z 缩放 = 1，四元数按半角直接构造）；
- `Transform3D::to_2d()` 只在**真能无损压平**时返回 `Some`——四元数只有 z/w 分量、z 位移为 0、
  z 缩放为 1；带倾斜 / 绕 X-Y 旋转 / 带 z 位移或 z 缩放一律 `None`，退化四元数也拒绝；
  `is_planar()` 供调用方先问再转。

这是 docs/40 §3 WP2/WP4 ① "统一 Transform 与 XY 便利 API"的实质部分：此前两个空间的变换各自一套
表示、互相之间连转换入口都没有。**目前没有生产调用点**——它们的用途是给编辑器与 Lua 提供
"跨空间搬运"的单一实现（下一步接入 Inspector 与 Lua 统一 `transform` 表）。

## 决策点：运行时是否启用空间继承（需要负责人裁定）

**证据（实测，2026-09-17）**：

- 26 个源场景里共 16 个 `Transform3D` 实体，其中**有父节点的为 0**；因此把 v2 内容迁到空间层级是**完全的空操作**（0 转换、0 拒绝），接线对既有项目**不产生任何画面变化**。
- 运行时代价：深度 100 全量传播 34.7 µs / 229 次分配，深度 1–10 只有 0.4–2.8 µs；增量传播叶子变动恒为 300 ns / 2 次分配。当前内容深度为 0，代价为 0。

**建议：暂不接线，等第一个真实消费者出现时再接。** 理由：

1. 现在接线对任何既有内容都**没有可观测效果**（没有父子对），因此换不来任何功能或性能收益；
2. 一旦启用，父子关系就从"组织关系"变成"空间语义"，这会改变**今后**在编辑器里拉父子时的行为——属于产品语义决策，不该由实现顺手决定；
3. docs/40 §9.2 明确要求"新增复杂度必须有实测收益支撑"。

**触发条件（满足任一条即应接线）**：① WP4/WP6 要落地 World2D 世界精灵与层级遮挡；② 编辑器要提供"换父保持世界位置"或父节点变换带动子树的编辑能力；③ 出现需要 3D 骨骼/挂点层级与实体层级统一的内容。

**若裁定接线**：按 docs/40 §3 WP2，必须默认关闭、提供显式迁移开关与差异报告（`MigrationReport` 已可直接用于此），并先跑 3D golden + 物理回写 + 骨骼 palette 回归（WP2-6）。

## 验收证据

- [ ] `cargo fmt --all -- --check` 通过
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过
- [ ] 相关测试通过（含深度 1/10/100、换父、循环、剪切拒绝、增量一致性）
- [ ] Debug 构建通过
- [ ] Demo / 手工步骤可复现（编辑器换父 + `demo3d` 预览）
- [ ] 注释符合 [章程 §9.7 代码与注释规范](../docs/00-project-charter.md#97-代码与注释规范)（精简中文）
- [ ] 文档与 CHANGELOG 已更新

## 未完成项 / 已知限制

- 本包不统一相机姿态与投影（属 WP5），接线后相机仍走 `Camera3D` 自带字段。
- 负缩放、世界空间 UI、二物理合并均不在本包范围。
- WP2-5 的增量传播**以 WP2-1 基准为前置**；基准未证明瓶颈时不得引入增量机制。
