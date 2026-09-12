# 任务：战斗 Hitbox 与最小武学

- 任务 ID：M18-01
- 状态：已完成
- 关联里程碑：M18
- 创建日期：2026-09-09

## 目标
实现固定时间步、可调试、命中去重的近战战斗底座，并用 Lua 完成玩家普攻、受击、死亡和基础武学循环。

## 影响文件

- `engine/crates/engine/src/core/` 战斗命令/事件与查询接口（实际未改：现有 Lua API 能力足够，见实现记录）
- `engine/crates/engine/src/host/runtime_scene.rs`（仅在接入调度阶段需要，未触碰）
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

- 引擎侧零改动：复用 `health.*`（调试镜像）、`physics.overlap`（Transform 圆查询宽相）、`tilemap.find_path`（A*）、`transform.get/set`（敌人无刚体，逐帧推进）、`inv.equip/equip_get`（weapon 装备槽骨架）。战斗规则全部落在 Lua，区分公共引擎能力与领域规则。
- `data/skills.lua`（纯数据，version=1）：`strike` 普攻（基础伤害 2、冷却 24 步、前摇 6、判定窗 12、range 44/width 40、uses_weapon）与 `palm_gust` 千风掌（伤害 6、内力 10、冷却 60）+ `validate_all`（枚举/整数步/范围校验）。
- `data/enemies.lua`（纯数据，version=1）：训练木人（不索敌不移动的确定性靶）与灰狼（追击+攻击），字段含 map/home/hp/speed/damage/aggro_radius/attack_range/attack_cooldown/skill 引用 + `validate_all`（地图/招式引用与数值范围）。
- `combat.lua` 战斗底座：固定步帧号（60Hz 步计数，无 wall-time）；`attack_id = attacker:skill:序号`；朝向矩形 hitbox（anchor = 中心 + dir×range/2，含攻击者身体半径 14、目标容差 10），overlap 宽相 + AABB 精判；判定窗内每步扫描但**同一攻击对同一目标只结算一次**（hits 集合）；冷却/内力门槛；玩家气血账本在 Lua（引擎 health 表跨图实体重建会失效，仅作镜像）；死亡→战败冻结 + R 重开（回出生点 + 敌人复活）；环形战斗日志（64 条，attack/hit/kill/death/respawn，含帧号/攻击者/目标/招式/伤害）；`hitbox_rect` 纯函数供调试/编辑器预览。enemy.lua 通过 `combat.register_enemy` 注入目标协议，避免循环依赖。
- `enemy.lua` 敌人 AI：persist_id 去重生成为主（仿 npc.lua）；idle/chase/attack 状态机；aggro 内 A* 追击（repath 节流 20 步）+ speed×dt 预算推进；进入 attack_range 且冷却就绪 → `combat.attack`（伤害用敌人自有 damage 字段）；玩家战败后停手（combat 侧同样拦截）；死亡销毁实体；`capture/restore`（hp/存活/格/坐标）与 `reset`（新档/重开复活）。
- `skills.lua` 运行时：武器加成读取 weapon 槽（`data.tools.training_sword.damage=8`），basic 招式伤害 = 基础 + 武器；martial 固定伤害。
- 接入：`main.lua` update 在 `npc.tick()` 后加 `combat.tick()`/`enemy.tick()`；存档新增 `combat`/`enemy` provider（快照 version=2 仅加 provider，旧档兼容）；`validate_content` 注册 skills/enemies 校验；`new_game` 调 `combat.reset()`/`enemy.reset()`；`enter_playable_map` 调 `enemy.on_enter`。`input.lua`：J 普攻、K 武学、战败后仅 R 重开（复用 Playing 态，不新增状态机模式）；HUD 文案带气血。
- 灰盒场景 `scenes/combat_arena.scene`（20x12 封闭场地）+ `data/maps.lua` 注册 `combat_arena`（spawn (9,5)、出口回山庄），仅服务战斗验收。
- headless 验收 `engine/crates/game/examples/jhyt_m18_combat.rs`（十章，照抄 jhyt_m17_shop 骨架）。

## 验收证据

- [x] `cargo fmt --all -- --check` 通过（exit 0）
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过（**预存在失败**：`runtime_scene.rs` 7 个与本任务无关的错误——3×redundant_field_names、3×too_many_arguments、1×needless_borrow，M17-05 已记录；本任务新增示例 `cargo clippy -p game --example jhyt_m18_combat --all-features -- -D warnings -A clippy::too_many_arguments -A clippy::redundant_field_names -A clippy::needless_borrow` exit 0，自身零告警，M17-02 先例）
- [x] 相关测试通过：`cargo test -p game` 7/7；headless `jhyt_m18_combat` 十章全过（`jhyt_m18_combat_ok`，exit 0）
- [x] Debug 构建通过：`cargo build --workspace`（exit 0）
- [x] Demo / 手工步骤可复现：`cargo run -p game --example jhyt_m18_combat`；M17 回归（`jhyt_smoke`/`jhyt_m17`/`jhyt_m17_npc`/`jhyt_m17_dialogue`/`jhyt_m17_shop`）全部 exit 0
- [x] 文档与 CHANGELOG 已更新（v0.14 段 M18-01 标记已实现；未改公共 Rust/资源格式，无需迁移文档）

验收点对照：①移动/普攻/受击/死亡/重开 → 01c/01g/01h 章；②A* 追击并攻击 → 01g 章；③练习剑伤害生效（2→10）→ 01d 章；④同一攻击单次命中 → 01c 章（判定窗内目标持续贴着仅掉一次血）；⑤战斗日志可追踪 → 01c/01f/01h 章（hit/kill/death 事件含帧号）。

## 未完成项 / 已知限制

- 暂不实现 Boss、复杂连招、网络同步和高级物理材质。
- 战斗规则全在 Lua，引擎侧战斗命令/事件接口未下沉（现有 API 足够，待 M18-03 统一 Scheduler 时再评估）。
- `farm_smoke` 示例预存在损坏（依赖示例重组前移除的 `fixtures/game_farm`），与本任务无关，未修复。
- workspace clippy 门仍被预存在的 `runtime_scene.rs` 调试插桩 lint 阻塞（见验收证据）。
- 检测到另一并行会话在本仓库做角色美术任务（`data/characters.lua`、docs 36-38 等），其 main.lua/场景改动与本任务改动共存且回归全绿；提交需由用户统一裁决。
