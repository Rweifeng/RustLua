# M6-03：Rapier 物理适配

- 状态：ACCEPTED
- 验收日期：2026-08-22
- 所属里程碑：M6 2D 游戏能力与样板游戏
- 前置任务：M6-02 动画状态机、M6-01 Tilemap（solid 层）

## 目标

接入成熟物理中间件 Rapier 2D：刚体、碰撞体、传感器触发器、碰撞层与掩码、碰撞事件进入 Lua、物理在固定时间步执行。不自研碰撞求解器。核心玩法价值：玩家速度驱动移动并被地图 solid 层真实阻挡。

## 背景

M6-00~02 玩家用 `transform.move` 直接位移（无碰撞），Tilemap 的 `solid` 层只是可查询数据。M6-03 引入真实物理，让玩家被地图墙阻挡，碰撞/触发器事件进入 Lua。

## 输入

- `engine/crates/engine_core/src/physics.rs`（新模块，封装 Rapier）
- `engine/crates/engine_core/src/script.rs`（Lua API 与场景实例化）
- `engine/crates/game_runner/src/main.rs`（固定步推进）
- `game/scenes/main.scene`、`game/scripts/main.lua`

## 不做什么

- 3D 物理、车辆/马达等高级约束
- 物理材质摩擦/弹力精细调参（用默认）
- 编辑器碰撞体可视化编辑

## 技术方案

1. **PhysicsWorld**（`physics.rs`）：封装 Rapier 2D 零重力世界——`PhysicsPipeline` + 各 set + `QueryPipeline`；`add_body`（动态/固定 + 多个 ColliderDesc）；`set_velocity`/`velocity`；`step(dt)` 步进 + 收集碰撞事件 + 返回移动动态体新位置；`drain_contacts`。
2. **碰撞事件**：`CollisionCollector`（`EventHandler` 用 `&self` + `Mutex` 满足 Send+Sync）收集 `CollisionEvent::Started/Stopped`；`collider_entities` 把碰撞体 handle 映射回引擎实体 → `PhysicsContact{a,b,started}`。注意：接触在 `step` 累积，由宿主 `drain_contacts` 读取清空（修掉了"每步 clear 覆盖上一帧"的 bug）。
3. **Collider 组件**：`shape=ball|radius` / `shape=cuboid|half_width|half_height`，可选 `sensor`、`offset`、`layer`、`mask`（映射 Rapier `InteractionGroups`）。
4. **场景集成**：`instantiate_scene` 解析 Collider → 动态体；Tilemap solid 单元格 → 一个固定体 + 多个静态 cuboid（偏移按单元格中心）。实体位置取自 ECS Transform。
5. **Lua API**：`physics.set_velocity`、`physics.get_velocity`、`physics.events()`（返回 `{a,b,started}` 数组）；保留既有 `physics.overlap`（ECS 距离查询，兼容 M4 示例）。
6. **固定步推进**：`LuaRuntime::step_physics(dt)` 步进 Rapier + 把移动动态体位置写回 ECS Transform；game_runner 每固定步 call_update 后调用。
7. **游戏工程**：Player 加 Collider（ball r16，出生点从 -55 移到 -40 避免出生即与左墙重叠）、新增 Trigger 传感器实体；main.lua 用 `physics.set_velocity` 驱动移动，`physics.events()` 记录 wall contact / trigger entered。

## 修改范围

- `engine/crates/engine_core/Cargo.toml`：+`rapier2d 0.22`、`crossbeam-channel`
- `engine/crates/engine_core/src/physics.rs`：新增（5 个单测）
- `engine/crates/engine_core/src/lib.rs`、`script.rs`：模块注册、实例化、Lua API、集成测试
- `engine/crates/game_runner/src/main.rs`：固定步 `step_physics`
- `game/scenes/main.scene`、`game/scripts/main.lua`

## 验收标准（对照主计划 M6-03）

- [x] 刚体、碰撞体、触发器：玩家动态球体、tilemap 静态碰撞体、Trigger 传感器（`sensor=true`）
- [x] 碰撞层和掩码：`layer`/`mask` → `InteractionGroups`；`collision_layers_and_masks_filter_contacts` 单测验证被掩码排除的层不产生接触
- [x] 碰撞事件进入 Lua：`physics.events()`；窗口运行 wall contact 多次、trigger entered 3 次；单测断言事件到达 Lua
- [x] 物理更新在固定时间步执行：game_runner 每固定步 `step_physics`，Rapier `IntegrationParameters.dt` 取固定 dt
- [x] 不自研碰撞求解器：Rapier 2D
- [x] 窗口运行：玩家按 D 走进右墙停在 x≈48（墙缘 64 − 半径 16），位置被真实阻挡
- [x] 质量门：fmt / clippy -D warnings / 全量测试（123 engine_core + 48 editor）/ Debug / Release 全过

## 测试命令

```powershell
cargo test -p engine_core --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo run -p game_runner -- --project ../game --log-filter debug
# 操作：WASD 移动（被地图墙阻挡）、空格攻击；Trigger 位于 (16,16) 附近
# 画面视觉验收待项目负责人
```

## 风险与应对

| 风险 | 应对 |
|---|---|
| 事件每步被覆盖 | `contacts` 在 step 累积，`drain_contacts` 读取清空（单测捕获此 bug） |
| 出生点在墙内 | 玩家出生点移到墙内 (-40,0)，避免初始重叠被求解器推出 |
| Rapier API 版本差异 | 事件处理器 `&self`+Mutex、step 参数（CCDSolver、Option<&mut QueryPipeline>、hooks）按 0.22 调整 |
| 动态体与 ECS Transform 失步 | `step_physics` 每步把移动体位置回写 Transform |

## AI 输出

已完成：Rapier 封装、刚体/碰撞体/触发器、layer/mask、碰撞事件进 Lua、固定步推进、速度驱动移动、游戏工程接入。

验证结果：格式检查通过；静态检查通过；单元测试 174 通过 0 失败；Debug/Release 构建通过；窗口运行玩家被右墙阻挡（x≈48）+ 碰撞/触发器事件实测；画面视觉验收待项目负责人。

## 已知限制

- 碰撞层/掩码为位字段（默认全开），编辑器无可视化配置
- 物理材质（摩擦/弹力）用默认值
- 无物理查询 API 接入 Rapier 空间查询（`physics.overlap` 仍为 ECS 距离查询，供 M4 示例兼容）
- 玩家精灵 128px 大于 32px 图块（视觉与碰撞体不一致，属美术调参）

## 建议下一步

- M6-04 UI/音频/粒子（血条、攻击音效、受击粒子）——碰撞/触发事件已就绪，可驱动玩法反馈
- 或 M6-05 存档与场景切换
