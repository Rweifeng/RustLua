# 任务：动作映射接线、Lua 输入 API 与设置持久化

- 任务 ID：M20-07
- 状态：已完成
- 关联里程碑：M20
- 创建日期：2026-09-10
- 完成日期：2026-09-13
- 前置依赖：M20-06（真实手柄设备帧）、M20-02（`ActionMap` / `ui_navigation` 抽象层）

## 目标

让已经实现但**当前无人消费**的动作映射层真正接入运行时：`ActionMap` 由宿主创建并随帧传给 Lua 运行时，手柄与键盘经同一套命名动作暴露给脚本，重绑定结果可持久化并带版本迁移。

## 影响文件

- `engine/crates/engine/src/core/input.rs`（`ActionMap` 接线增强：可读方向键绑定文本、默认动作表拆分为四个 `ui_*` 方向动作）
- `engine/crates/engine/src/core/settings.rs`（新增 `[input]` 段解析、保序写回、文件级读写）
- `engine/crates/engine/src/core/script.rs`（`ScriptState.action_map` + `input.action*` / `pad*` / 绑定管理 / 设置持久化 API，`LuaRuntime` setter 与查询）
- `docs/18-lua-api.md`（命名动作 API 表、`[input]` 设置段）
- `docs/35-输入动作映射与设置持久化.md`（契约、绑定文本、回退策略、迁移说明）
- `docs/02-technical-decisions.md`（ADR-0021）
- `CHANGELOG.md`（v0.15 段 M20-07 条目）

## 方案要点（落地结果）

1. **单一事实来源**：`ActionMap` 只有一份实例，随 `ScriptState` 走（`ScriptState::action_map`），宿主通过
   `LuaRuntime::set_action_map` / `action_map` 注入与查询；Lua 的 `input.rebind`/`unbind`/`reset_bindings`/
   `set_action_map`/`load_settings` 与宿主查询**同源**，不会出现两份映射不同步。
2. **Lua API 形状**（与计划稿逐项一致，未改形状）：
   - `input.action(name)` / `action_pressed` / `action_released` / `action_axis(negative, positive)`；
   - `input.pad_button(index)` / `pad_axis(index)`（原始手柄查询）；
   - `input.actions()` / `bindings(action)` / `rebind(action, binding)` / `unbind(action, binding)` / `reset_bindings()`；
   - `input.action_map_text()` / `set_action_map(text)` / `settings_path()` / `load_settings(path?)` /
     `save_settings(path?)` / `diagnostics()`。
3. **保留旧路径**：`input.is_down` / `just_pressed` / `just_released` / `axis` / `pressed` / `released`
   语义不变，老脚本与 headless 用例（`jhyt_m17_*`、`jhyt_m18_*`、`jhyt_m19_*`）继续直接驱动物理键。
   `input.action` 从「物理键别名」升级为「命名动作查询」——迁移说明见 `docs/35` §6。
4. **UI 导航接线**：M20-02 已把 `ui_navigation::{FocusRing,ScrollState}` 接成 `ui.focus_*` / `ui.scroll_*`；
   本任务负责让**动作层**能驱动它——默认表把上下左右拆成 `ui_up`/`ui_down`/`ui_left`/`ui_right`
   四个动作（分别绑定方向键与左摇杆对应方向），使 `input.action_axis("ui_down", "ui_up")` 能产出带符号模拟量
   （把两个方向塞进同一动作会让正负互相抵消）。面板侧接线由 M20-08 完成。
5. **持久化与迁移**：
   - `settings.toml` 的 `[input].map` 为**一行转义字符串**（版本头 `rustlua-actions-v1`）；
   - 读取失败/版本不符/绑定非法 → **回退默认映射并写可定位诊断**（`<path>: <reason>; reverting to defaults`），
     不崩溃、不静默丢弃；缺 `[input]` 段或文件缺失 → 返回 `false` 但**保留当前绑定**
     （`missing` / `... keeping current bindings`）；
   - 写入为保序读改写（`upsert_input_section`），只动 `[input]` 段的 `map` 键，
     `[audio]` 段、注释与未知键原样保留；
   - 文件中缺失的动作由 `ActionMap::merge_missing` 用默认补齐，**用户重绑定不被覆盖**。
6. **变更记录**：已在 `docs/02-technical-decisions.md` 追加 ADR-0021（公共 Lua API 与设置格式变更，
   含原因、影响、备选、迁移/回滚与是否需重新验收）。

## 实现记录

- **动作映射接线**（`core/input.rs`、`core/script.rs`）：`ScriptState` 新增 `action_map: ActionMap` 与
  `input_diagnostics: Vec<String>`（上限截断，`INPUT_DIAGNOSTIC_LIMIT`），`ScriptState::new` 以
  `ActionMap::with_defaults()` 初始化；`InputSnapshot::{action_is_down, action_just_pressed,
  action_just_released, action_axis}` 同时考虑键鼠与手柄绑定（M20-06 的设备帧在
  `RuntimeScene::run_script_steps` 合并进同一 `InputSnapshot`，因此 Lua 侧无需区分设备）。
- **默认动作表**（18 项）：`move_up/down/left/right`、`interact`、`attack`、`skill`、`gift`、`sleep`、
  `practice`、`inventory`、`pause`、`confirm`、`cancel`、`ui_up/down/left/right`；键盘绑定与 `game_jhyt`
  现行键位一致，`confirm`/`cancel`/`ui_*` 另带手柄默认绑定（`pad:0`/`pad:1`/`padaxis:1±`/`padaxis:0±`）。
- **绑定文本**（`Binding::{to_text, parse}`）：`key:<字符/名字>`、`mouse:<left|right|middle>`、
  `pad:<序号>`、`padaxis:<序号><+|->`。方向键序列化为**可读名字** `key:up`/`key:down`/`key:left`/`key:right`
  （与 Lua 侧 `input.is_down("up")` 同名，便于设置文件阅读与 M20-08 的重绑定面板展示），
  旧文件里的内部码位写法 `key:other:1048577`…`1048580` 仍可解析并在重新保存时归一化
  （新增 `InputKey::arrow_name`，覆盖单测 `arrow_keys_use_readable_binding_text_and_round_trip`）。
- **`[input]` 设置段**（`core/settings.rs`）：`parse_input_section`（宽松：解析 TOML 单行转义字符串、
  返回 `InputSectionParse { map, reason }`，非法转义/未知绑定/版本不符给出原因）、
  `upsert_input_section`（保序、幂等）、`load_input_section`（文件级，返回 `(map, diagnostic)`）、
  `save_input_section`、`escape_toml_string`；不引入新依赖。
- **Lua 侧设置持久化**（`core/script.rs`）：`input.settings_path/load_settings/save_settings` 与
  `audio.*` 同规则（缺省 `<project_root>/settings.toml`；无工程根且未给路径 → `false, "no_project_root"`）；
  `save_settings` 走引擎既有 `atomic_write`（原子替换，避免半写文件）；
  `resolve_input_settings_path` 处理相对/绝对路径；`push_input_diagnostic` 累积诊断，
  `LuaRuntime::input_diagnostics()` 供宿主与 headless 断言。
- **宿主查询入口**：`LuaRuntime::{set_action_map, action_map, input_diagnostics, eval_json}`——
  宿主可读回 Lua 侧改键结果（M20-08 面板与验收脚本使用）。
- **文档**：`docs/35-输入动作映射与设置持久化.md`（三层模型、API 表、绑定文本、`[input]` 段与回退策略、
  与旧物理键 API 的迁移说明、已知限制）；`docs/18-lua-api.md` 新增「`input` 命名动作、手柄与绑定持久化」
  小节并把 `settings.toml` 示例补上 `[input]`；ADR-0021。

## 验收方式

- `cargo test -p engine --lib`：新增动作层与设置层单测——
  `input_action_api_matches_keyboard_and_gamepad_bindings`（同一动作分别由键盘键与手柄按钮触发，断言
  `input.action` / `action_pressed` / `action_axis` 三者结果一致）、
  `input_action_map_rebind_unbind_and_text_round_trip`（重绑定后旧绑定失效、新绑定生效、非法绑定文本
  返回明确错误、`set_action_map` 版本不符不改动当前映射）、
  `input_action_settings_round_trip_and_version_fallback`（`to_text` → 写盘 → 读盘 → 与内存映射相等、
  版本头不符回退默认且产生可定位诊断、用户重绑定不被 `merge_missing` 覆盖、`[audio]` 段与注释保留）、
  `input_actions_drive_ui_focus_navigation`（`ui_*` 动作驱动焦点环移动）、
  `input_action_map_defaults_are_available_to_lua`、`action_queries_follow_bindings_and_survive_rebinding`、
  `action_map_round_trips_through_settings_text`、`arrow_keys_use_readable_binding_text_and_round_trip`；
  `settings.rs` 三个 `input_section_*` 单测覆盖转义/宽松解析/文件往返与回退。
- 回归：`cargo test -p game`、`cargo test -p editor`、全部 `jhyt_*` headless 示例证明旧物理键路径未被破坏。
- `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、
  Debug 构建通过。
- 手工步骤：改绑 `interact` 为手柄 A（`input.rebind("interact", "pad:0")`）→ 进游戏按 A 能交互、按 E 不再交互。

## 验收证据

- [x] `cargo fmt --all -- --check` 退出 0（含删除早期并行任务遗留的空探针示例 `jhyt_m20_probe_m19ui.rs` 后复跑）
- [x] `cargo clippy -p engine --all-targets --all-features -- -D warnings` 退出 0（`settings.rs`/`script.rs` 的未用导入与 `script.rs` 匹配臂类型错误已修）
- [x] `cargo test -p engine --lib --offline`：**475 passed / 0 failed**（M20-07 前为 474，新增方向键绑定文本往返 1 项）
- [x] 动作层集成测试覆盖「键盘与手柄触发同一动作结果一致」「重绑定后旧键失效」「非法绑定文本明确报错」
- [x] 设置往返测试覆盖「写盘→读盘相等」「版本头不符回退默认 + 可定位诊断」「用户重绑定不被默认覆盖」「`[audio]` 段与注释保留」
- [x] `cargo test -p editor`：106 + 136（1 ignored）通过，退出 0；`cargo test -p game` 通过（旧物理键路径未破坏）
- [x] 文档已更新：`docs/35-输入动作映射与设置持久化.md`、`docs/18-lua-api.md`、`docs/02-technical-decisions.md`（ADR-0021）、`CHANGELOG.md`
- [ ] 手工步骤「改绑后旧键失效」在**真实窗口**中未执行（本环境无交互桌面会话）；等价断言由
      `input_action_map_rebind_unbind_and_text_round_trip` 在 headless 下覆盖
- [x] `scripts/verify.ps1` 相关门禁由里程碑收口统一复跑（M20-05/收尾轮）

## 未完成项 / 已知限制

- 本任务只提供「绑定文本 + 解析 + 读写 + Lua API」，**不提供**编辑器可视化重绑定面板与 `game_jhyt`
  从物理键到命名动作的整体迁移（见 M20-08）。
- 面板/玩法侧的 `ui_*` 动作消费（焦点环移动）由 M20-08 迁移；本任务只保证动作层可驱动 `ui.focus_move`。
- 绑定文本是**公开持久化格式**：后续新增绑定种类只能新增前缀，不能改变既有前缀语义；
  `key:other:<码位>` 需长期保持可解析（旧设置文件兼容）。
- 触屏输入、输入录制/回放、网络同步仍未实现（沿用 M1-06 的已知限制）。
