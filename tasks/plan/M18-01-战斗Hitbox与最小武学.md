# 任务：战斗 Hitbox 与最小武学

- 任务 ID：M18-01
- 状态：待办
- 关联里程碑：M18
- 创建日期：2026-09-09

## 目标
实现固定时间步、可调试、命中去重的近战战斗底座，并用 Lua 完成玩家普攻、受击、死亡和基础武学循环。

## 影响文件

- `engine/crates/engine/src/core/` 战斗命令/事件与查询接口
- `engine/crates/engine/src/host/runtime_scene.rs`（仅在接入调度阶段需要）
- `examples/game_jhyt/scripts/` 下新增 `combat.lua`、`skills.lua`、`enemy.lua`
- `examples/game_jhyt/scripts/data/tools.lua`
- `examples/game_jhyt/scripts/data/skills.lua`
- 战斗 headless 测试和灰盒场景

## 风险

- 不重复实现 Rapier；优先复用 overlap/raycast，必要时采用显式矩形/网格判定。
- 命中事件必须携带攻击者、目标、招式 ID 和帧序号，并保证同一攻击单次命中。
- 固定输入在 30/60/120 FPS 下必须得到一致结果。

## 验收方式

- 玩家可移动、普攻、受击、死亡和重新开始。
- 敌人可通过 A* 追击并攻击玩家。
- 装备练习剑后伤害属性真实生效。
- 同一攻击不会重复伤害同一目标。
- 战斗日志可追踪关键命中和死亡事件。

## 实现记录

待实现。

## 验收证据

- [ ] `cargo fmt --all -- --check` 通过
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过
- [ ] 相关测试通过
- [ ] Debug 构建通过
- [ ] Demo / 手工步骤可复现
- [ ] 文档与 CHANGELOG 已更新

## 未完成项 / 已知限制

- 暂不实现 Boss、复杂连招、网络同步和高级物理材质。
