# M16-03 工具系统 ToolAction

## 状态
DONE

## 类型
[引擎+玩法]

## 目标
统一工具概念：工具 JSON 定义、按朝向计算作用范围、UseItem、工具使用动画。工具与格子逻辑解耦，产出 `ToolAction` 供玩法消费。

## 背景
`main.lua` 的 Q/E/R/F 直接改内存状态，无统一工具概念。缺口清单 2.1 明确要「工具使用方向与作用范围」。

## 输入
- `data/tools.json`（锄/水壶/镰刀定义，含体力消耗、作用范围）
- `grid.facing_cell`（18-lua-api.md）
- M16-01 的朝向、M16-02 的交互

## 不做什么
- 不做工具等级/蓄力/升级材料（M18-00）；
- 不做作物逻辑（M16-06），工具只产出「对格子的动作请求」；
- 不做装备栏效果。

## 依赖
M16-01、M16-02、M16-11（数据管线先落）

## 验收标准
1. 工具定义进 `data/tools.json`：锄/水壶/镰刀，字段含消耗体力、作用范围、作用方向。
2. 使用工具时按朝向计算作用格（`grid.facing_cell`）。
3. 工具动画：播放对应方向的使用帧。
4. 解耦：工具动作产生 `ToolAction`（类型 + 作用格列表），由 `farm.lua` 消费，工具模块不碰作物/格子状态。
5. 工具可被 Lua 选择（快捷栏切换 + 数字键触发使用）。

## 测试命令
```powershell
cargo fmt --all -- --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test --workspace --all-features && cargo build --workspace --release
```
实机：朝向不同方向挥锄，作用格随朝向变化；选不同工具触发不同动作。

## 风险
- 作用范围形状（单格/3×3 前向）设计未定 → 用数据字段驱动，不写死；
- 工具动画资源缺失 → 阶段内占位帧，M18-09 补工作流。

## AI 输出（实施后填写）
- 已完成：`data/tools.json`（锄/水壶/镰刀：`energy/range/direction`）+ schema 校验（M16-11 管线）；`tools.lua` 按朝向经 `grid.facing_cell` 计算作用格，产出 `ToolAction{kind,energy,cells}`，**不触碰农场/格子状态**；`farm.lua` 消费 ToolAction（Q=选中工具默认锄、R=水壶、F=镰刀，E=播种）；工具作为 `kind="tool"` 物品进快捷栏（槽 2-4），数字键切换 `state.active_slot`；`tool_swing` 工具使用占位动画。
- 修改文件：`game_farm/data/tools.json`、`items.json`；`scripts/tools.lua`、`farm.lua`、`inventory.lua`、`game.lua`、`state.lua`、`data_tables.lua`。
- 验证：`farm_smoke.rs` 断言 tools.json 加载（hoe.energy=5）、slot2 选中为 hoe、ToolAction 结构 `hoe:1`；垂直切片走完整工具路径（锄→种→浇→收）；全量测试通过。
- 已知问题：作用范围目前单格（`range` 字段已驱动，3×3 待扩展）；工具动画为占位帧。
- 下一步：M18-00 工具等级/蓄力/数值。
