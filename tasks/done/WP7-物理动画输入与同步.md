# 任务：WP7 2D 物理、动画、输入与同步

对应 [docs/40](../../docs/40-引擎全面重构计划.md) §3 WP7。状态：**完成**——collider 调试绘制、
连续碰撞配置（CCD）、`PhysicsAuthority` 与统一 scheduler 阶段全部落地。

## 已做：统一 scheduler 阶段 + `PhysicsAuthority`（本轮）

### 类型化的阶段（`core/scheduler.rs`）

新增三个阶段，把此前只存在于注释与调用顺序里的三格变成类型：

| 阶段 | rank | 含义 |
| --- | --- | --- |
| `Script` | 1 | 脚本 `update()` 与 UI 点击派发 |
| **`Sync`** | 2 | 物理步**之前**的静态/运动学同步 |
| `Physics` | 3 | 物理推进（2D + 3D） |
| **`Writeback`** | 4 | 物理步**之后**的动态回写 |
| `Simulation` / `Animation` | 5 / 6 | 粒子、时钟、场景切换 / 动画推进 |
| **`Spatial`** | 7 | `GlobalTransform` 传播（父链组合） |

帧内顺序因此是裁定要求的：**脚本 → 同步 → 物理 → 回写 →（模拟/动画）→ 世界变换 →
提取**。`spatial_propagate` 放在动画**之后**是有意的：动画也改局部变换，传播必须是最后一步，
提取才能看到本帧最终的世界位置。

### 回写时机真的改了

`step_physics` / `step_physics3d` 现在**只推进**，把被移动的动态刚体暂存进
`ScriptState::pending_writeback_{2d,3d}`，由 `Writeback` 阶段的 `physics_writeback` 统一写回。
可观察差异：**2D 与 3D 的所有推进都发生在任何回写之前**——此前 2D 的回写夹在 3D 推进之前，
混合 2D/3D 场景里"谁先看到谁的结果"取决于注册顺序。

### `PhysicsAuthority`（`core/physics.rs`）

```rust
pub enum PhysicsAuthority { Physics /* 默认 */, Script }
```
- `Physics`（默认）：**模拟是世界位置的真源**，步后回写 ECS（与既有行为逐字节一致）；
- `Script`：**ECS 是权威**，步前把 ECS 位置推给模拟（`sync_kinematic_bodies` → 只取动态刚体、
  按实体排序保证确定性），步后**跳过**回写。

### 回滚档

- `RuntimeScene::set_physics_stage_order(PhysicsStageOrder::{Unified, Legacy})`：`Legacy` 回到
  "物理步进即回写"的单系统顺序；
- `RuntimeScene::set_physics_authority(PhysicsAuthority::{Physics, Script})`。

### 证据

| 项 | 结果 |
|---|---|
| 权威 A/B（新） | `physics_authority_decides_who_owns_the_transform`：同一场景里脚本改 ECS 到 (100,0)——`Physics` 权威下回写把它改回 0（模拟说了算），`Script` 权威下保持 100（步前同步 + 不回写），两种权威结果必须不同 ⇒ 证明同步与回写两格真的接在流水线上 |
| 阶段顺序（旧新对拍） | `builtin_pipeline_preserves_legacy_stage_order` 的期望序更新为统一序（`physics_sync` → `physics2d_step`/`physics3d_step` → `physics_writeback` → … → `spatial_propagate`），并断言用户探针（Script/Physics 阶段）的相对次序不变 |
| 物理玩法回归 | 方块阻挡、下落着地、碰撞事件、collider 调试绘制、CCD 隧穿对照全部原样通过（engine lib **662 项**） |
| 不回退 | 2D overlay golden 与 3D semantic golden 逐字节不变（`verify.ps1 -Benchmarks` 全绿） |


## 负责人裁定（本轮）

**一次性统一 scheduler 阶段为**：脚本 → 控制器 → 静态/运动学同步 → 物理 → 动态回写 →
`GlobalTransform` → RenderSnapshot，**并同时改物理回写时机**（不采用"先声明 authority 类型、
回写顺序不变"的保守方案）。执行要求：① `PhysicsAuthority` 与阶段顺序都进类型与文档；
② 回写从"物理步之后立即写 ECS"改为统一阶段，且必须与 WP2 的 `GlobalTransform` 传播、WP6 的
RenderSnapshot 顺序一致（三者是同一条帧内流水线，不能各排各的）；③ 证据：物理玩法回归
（方块阻挡、下落着地、碰撞事件、collider 调试绘制、CCD 隧穿对照）与 golden 全绿；
④ 回滚档：`set_physics_stage_order(legacy|unified)`，`legacy` 即回到当前时序。

## 已做：collider 调试绘制（docs/40 §3 WP7 ②）

docs/40 要求"2D 提供 collider debug draw、查询、触发器、层/掩码、连续碰撞配置"。其中
查询/触发器/层掩码早已由 Lua `physics` 表提供（`get_velocity/set_position/events/raycast/overlap`
以及组件串的 `layer/mask`），**缺的是调试绘制**——此前引擎里搜不到任何 debug draw 代码。

### 数据源：物理世界，而不是组件串

`PhysicsWorld::debug_shapes()` 读 Rapier 里碰撞体的**当前**姿态与形状：

- 组件串 `Collider` 只在实例化时被解析一次、之后无人更新，而动态刚体每步都在动；
  调试绘制要回答的是"物理到底把它放到哪了"，所以必须读模拟侧。
- 局部偏移取 `collider.position_wrt_parent()` 再按刚体旋转叠加。**直接拿
  `collider.translation()` 会把父体位移算两遍**（它是世界位置）——实现时踩到，已由
  `debug_shapes_report_world_pose_offset_and_sensor` 钉住（含 90° 旋转下 (2,0) → (0,2) 的换算）。
- 结果按 `(entity, center)` 排序：`entity_bodies` 是 `HashMap`，不排序会让调试顶点顺序随哈希
  种子变化（任何基于它的对拍都会随机失败）。

### 绘制

- `RuntimeScene::set_collider_debug_draw(bool)`，默认**关闭**；Lua `physics.debug_draw(enabled)`
  登记请求、宿主每帧取走（与 `camera2d` 的 pending 语义一致：取走即清空，否则开关会被反复应用）。
- 实心绿 / `sensor` 黄：sensor 不产生物理响应，颜色不同才不会误判。
- box 4 段；球 24 边形近似（段数固定 ⇒ 顶点数可断言）。全部用白色纹理，因此只多 **1 个 draw call**；
  画在世界内容之上、屏幕 UI 之下。
- **认不出的形状（胶囊/多边形）不画**，不做静默近似。

### 证据（像素级）

2D overlay golden（[docs/53](../../docs/53-WP0增量场景帧基准与每帧分配.md) §3.6）里给实心块挂了一个
与精灵矩形**完全重合**的 box 碰撞体，于是轮廓正好落在精灵边界上（在泄漏检查的 ±2px 容差内）：

| 断言 | 实测 |
|---|---|
| 默认关闭 | `collider_debug_draw() == false` |
| 开启后顶点数 | **60 + 24**（box 4 段 × 6），其余内容不变 |
| 轮廓像素（矩形左边界外侧） | 绿占优（`[124,255,160]`：绿明显高于红/蓝） |
| 实心块内部 | 仍是纯红 `(255,0,0)`（没被盖住） |
| 矩形外泄漏 | **0**（轮廓与碰撞体位置一致，没有偏移） |
| draw call | 比关闭时多 |

`verify.ps1 -Benchmarks` 已把上述条款写成契约门。单测：核心 1 个（姿态/偏移/旋转/sensor）+
Lua 1 个（请求登记与取走即清空、场景 `Collider` → 调试形状）。

## 已做：连续碰撞配置（CCD，docs/40 §3 WP7 "连续碰撞配置"）

高速物体一步跨过薄墙（隧穿）此前无解：`Collider` 组件串与 Lua 都没有 CCD 开关，只能靠调高
物理步频或把速度改小来绕开，而两者都会改变玩法手感。现在 **2D 与 3D 对称**：

- **组件串**：`Collider` / `Collider3d` 新增可选 `;ccd=true`（默认关闭，与 Rapier 一致）；
  `ccd=yes` 这类拼错**报错**而不是静默当 false——静默忽略会让"高速穿墙"重新出现且日志里毫无线索。
- **Lua**：`physics.set_ccd(id, enabled)` / `physics.get_ccd(id)` 与
  `physics3d.set_ccd(id, enabled)` / `physics3d.get_ccd(id)`。`get` 读的是 **Rapier 里的真实
  状态**（`body.is_ccd_enabled()`），不是引擎侧的影子记录，所以"声明到底生效没有"可以被直接
  验证；未知实体报错。
- **归属说明写进类型**：Rapier 的 CCD 是**刚体**属性而不是碰撞体属性，因此 `ColliderDesc.ccd`
  的语义是"这个刚体要不要开 CCD"——一个刚体上任一 collider 声明即对该刚体生效（两个 `add_body`
  的文档注释里都写明）。

**行为证据（不是只断言一个布尔值）**：2D 与 3D 各有一条 A/B——
`ccd_stops_a_fast_body_from_tunneling_through_a_thin_wall`（一步 1000 世界单位、墙厚 2）与
`ccd_stops_a_fast_body_from_tunneling_in_3d`（一步 1000 世界单位、墙厚 0.1）：同一条测试里跑
两次对照，**不开 CCD 时穿过墙（x > 0），开了之后停在墙前（x < 0）**。另有核心 2 条 + Lua 2 条
覆盖声明解析、运行时开关与未知实体报错。

## 待裁定：同步阶段与 `PhysicsAuthority`

docs/40 §3 WP7 的主体是"定义 `PhysicsAuthority` 与同步策略，统一 scheduler 阶段为
脚本 → 控制器 → 静态/运动学同步 → 物理 → 动态回写 → GlobalTransform → RenderSnapshot"。

现状已有可用骨架：`PhysicsWorld::step` 返回被移动动态刚体的新位置，宿主把它们回写到 ECS；
`set_position` 是"运动学传送"且明确要求调用方自己同步 ECS。**但没有显式的 authority 声明**，
因此"谁有权写 Transform"仍靠约定。

这属于要项目负责人裁定的范围决策（改回写时机可能造成玩法漂移），因此按目标要求**先推进不依赖
它的部分**（调试绘制正是其中之一），裁定前不动同步阶段。证据与建议：现引擎内 2D 只有
"固定/动态"两类刚体、没有运动学体的实际用例，先把 authority 写进文档与类型、再逐步迁移，
风险低于一次性改动回写顺序。

## 未完成

- `PhysicsAuthority` 与统一同步阶段（待裁定，见上）。
