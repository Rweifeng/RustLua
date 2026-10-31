# 编辑器插件 API（P3-E04）

本文记录 P3-E04 的插件接口边界、扩展点、故障隔离与诊断入口，并给出 Lua 项目插件的
决策门结论。实现位于 `engine/crates/rustlua-editor-api`（叶子 crate）与
`engine/crates/editor/src/plugins.rs`（宿主与内置示例）。

## 版本与加载方式

- `PLUGIN_API_VERSION = 1`；manifest 的 `api_version` 不匹配会在注册期拒绝。
- **只支持静态编译插件**：`editor/src/plugins.rs` 维护构造器清单，启动时
  register → load。v1 不加载 `.dll`/`.so`，**不承诺 Rust `cdylib` 稳定 ABI**。
  启动时会先读用户级「插件开关」（见下节），被用户关掉的插件**只 register 不 load**。
- `PluginManifest` 包含 `id/name/version/api_version/capabilities/dependencies`。
  依赖按拓扑顺序加载；缺依赖、依赖环、被依赖插件仍加载时的卸载都会被拒绝。
- 能力声明只是元数据，真正的扩展点在 `register(&mut PluginContext)` 里登记，
  卸载/禁用时整份上下文丢弃，入口随之消失。

## 宿主通道

插件拿不到 `&mut EditorApp`：

- 只读：`PluginHost::snapshot() -> &HostSnapshot`，含项目根、场景名、主选中实体、
  实体数、资源数、是否播放、可见插件面板。
- 写入：`PluginHost::submit(PluginAction)`，动作只有 `SetStatus`、
  `SetPanelVisible`、`TogglePanel`。编辑器在绘制帧末统一应用，插件改不到场景文档。
- 面板、菜单、命令、检查器段的回调都在 `catch_unwind` 里执行；panic 只记录到
  插件日志/状态栏，不会终止编辑器。

## 扩展点

| 扩展点 | 描述符/回调 | 宿主行为 |
|---|---|---|
| 面板 | `panel` + `panel_draw` | 独立 `egui::Window`，可开关；禁用后不绘制 |
| 菜单 | `menu_item` | 追加到对应顶级菜单，菜单命令必须已声明 |
| 命令 | `command` + `command_handler` | 菜单点击、命令面板与诊断页共用同一执行入口 |
| 检查器段 | `inspector_section` + `inspector_section_draw` | 有主选中实体时画在 Inspector 尾部 |
| 资源导入器 | `asset_importer` | 声明式：登记扩展名，v1 **不执行**自定义导入逻辑 |
| 可视化脚本节点 | `visual_script_node` | 描述符 + 插件在 `on_load` 调 `rustlua_visual_script::register_plugin_node` 注册真实 `NodeSpec` |

加载后统一校验：id/kind 非空、同插件内不重复、跨插件不撞名、导入器扩展名必须为
不带点的小写形式、菜单引用的命令必须已声明、回调必须挂在已声明项上。违规返回
`PluginError::Context`，插件不会进入已加载状态。

可视化脚本节点与内置节点共用一张查找表，因此编辑器调色板、图校验与 Lua 生成器
不需要区分来源；注册是进程级且没有反注册接口（禁用插件只移除描述符，节点仍留在
注册表）。格式与生成规则见[可视化脚本格式与代码生成](72-visual-script.md)。

## 诊断

「窗口 ▸ 插件诊断」列出每个已注册插件的 id、名称、版本、API 版本、能力、启用/加载
状态与最近错误，并提供启用/禁用/重新加载按钮；最近 8 条插件日志一并显示。
重新加载会卸载后重建 UI 上下文；静态插件不会重新执行二进制初始化。

每个已加载插件下面还会列出它的**命令**，每条命令行尾一个「运行」按钮——点它走的是与
菜单栏、命令面板**同一条**执行入口（`run_plugin_command` → `PluginContext::command_handler`）。
诊断页没有单独的插件命令通道。

## 内置示例

`EditorApp` 启动时注册：

- `core`：核心编辑器占位插件，无扩展点。
- `project_stats`：面板 + 菜单 + 两条命令 + 检查器段 + `.rlstats` 导入器声明 +
  `plugin.project_stats.log` 可视化脚本动作节点（模板 `print({text})`）。

示例只读快照、只提交动作，因此禁用/卸载不会改变场景文档。

## 启用状态持久化（2026-10-30）

插件的启停是**用户级**设置，落在 `~/.rustlua/editor.json` 的 `plugins` 段
（`plugin id → true/false`），和主题、改键、服务商设置同属一份文件：

- **禁用即刻落盘**：诊断页点「禁用」时把注册表里**所有**插件的当前状态一起写回
  （只写变化那一条会让内存与文件脱节）。写失败或找不到用户主目录时，启停**照样生效**，
  状态栏如实写明「本次有效，但保存失败／下次启动不保留」——UI 动作不该被磁盘挡住。
- **启动时读回**：register 之后、load 之前套用；被用户关掉的插件**根本不 load**，
  于是它的 `on_load`（例如往全局可视化脚本节点表里注册节点）不会跑。
  这条比「加载后再禁用」更干净：**运行中**禁用只移除描述符，已经注册进全局表的
  可视化脚本节点仍然留着（节点没有反注册接口，见下）。
- **范围是用户，不是工程**：换工程、带不带 `--project` 启动都保持同一份启停状态。
- **容错**：不是布尔值的条目整条丢弃（该插件回落默认）；本版本不认识的 id 原样留着，
  插件改名或下线只会多一条无用记录，不会让读它的版本丢掉别的设置。

门：`plugins::tests::disabled_plugin_stays_disabled_across_a_restart`（关掉 → 重启仍是禁用
且没 load 过 → 菜单与命令都不出现 → 再打开 → 下次启动仍是打开）、
`rustlua-editor-core` 的 `user_config::tests::plugin_states_round_trip_alongside_other_sections`
（往返、非布尔丢弃、别的段不被抹掉）。

## 命令面板接入（2026-10-30）

插件声明的条目也进**命令面板**（`Ctrl+Shift+P`）。之前面板只摊平内置菜单树，插件的命令
只能从插件诊断页点——扩展等于半截。现在候选集 = 内置菜单叶子 + **已加载**插件的声明：

| 插件声明的东西 | 面板里的样子 | 执行 |
|---|---|---|
| 菜单条目（挂了命令） | `插件 / 项目统计`（按插件声明的菜单分组） | `run_plugin_command` |
| 菜单条目（没挂命令） | `插件 / 面板示例` | `toggle_plugin_panel`（开关同 id 的面板，与菜单栏同口径） |
| 命令（没被菜单条目引用） | `插件 / 项目统计 / 刷新项目统计`（分组用 manifest 的名字） | `run_plugin_command` |

- **没有第二条命令通道**：面板执行的就是菜单栏点同一条菜单时执行的同一个回调
  （`PluginContext::command_handler`）。插件不需要为"能被搜到"额外做任何事。
- **不重复**：已被菜单条目引用的命令不再单独出现一条。
- **只认"已加载"**：禁用/卸载的插件不在注册表的已加载集合里，面板里立刻消失。
- 插件命令声明的 `shortcut` 会显示在条目右边，但**不会被真的触发**——编辑器只处理内置
  动作的键位；v1 也没有命令参数与上下文条件。

门：`plugins::tests::plugin_entries_join_the_command_palette`（数据层：条目在、过滤同一套、
禁用后消失）、`panels::command_palette::tests::plugin_commands_are_searchable_and_runnable_from_the_palette`
（无头 egui：面板里画出来、`Enter` 真执行、面板开关真打开）、
`panels::command_palette::tests::disabled_plugin_entries_leave_the_palette`。

## 诊断页与面板可见性（2026-10-30）

- **诊断页能运行命令**：每个已加载插件的命令后面有「运行」按钮，走与菜单栏、命令面板
  同一条执行入口（`run_plugin_command`）。这之前文档写着诊断页能"运行"插件命令，但按钮
  并不存在——现在文档与实现对齐了。
- **面板可见性是用户级设置**：落在 `~/.rustlua/editor.json` 的 `panels` 段
  （`panel id → true/false`），与 `plugins` 段同属一份文件。打开或关掉面板即刻落盘
  **整张已知表（含 `false`）**；启动时只恢复记为"可见"的（默认是关的）。换工程也记住
  ——记的是"我开着哪个插件面板"，不是工程布局。
- 可见性只记面板 id，**不记**窗口尺寸与位置（那些仍属工程布局）。记为可见但插件已被
  禁用的 id 会留在文件里（不画也不报错）。

门：`plugins::tests::plugin_commands_can_be_run_from_the_diagnostics_page`（无头 egui
**真点击**：把「运行」按钮的文字位置换成指针事件走 egui 自己的命中判定，断言命令真执行）、
`plugins::tests::plugin_panel_visibility_survives_a_restart`（打开 → 落盘 → 重启仍开着；
关掉 → 落 `false` → 再启动不自己弹回来）、
`rustlua-editor-core` 的 `user_config::tests::panel_states_round_trip_alongside_other_sections`。

## Lua 项目插件决策门（v2）

**结论：P3 不做 Lua 项目插件。** 理由与触发条件：

- 现有 `rustlua-scripting` 是游戏运行时 VM，不是编辑器扩展沙箱；直接复用会让项目
  脚本拿到编辑器进程的完整文件/网络能力，权限模型、超时与内存配额都不存在。
- 编辑器 UI 只接受 Rust 回调；开放任意 egui 绘制会绕过 `PluginAction` 单一写入口，
  与故障隔离目标冲突。
- 若未来出现**至少两个真实项目插件需求**，且要求“不改编辑器二进制即可发布”，
  再启动 v2；前置条件是：项目根内文件访问白名单、网络默认拒绝、单回调超时/内存
  配额、错误与审计日志、仅开放菜单/命令/受限对话框（不开放任意绘制）。
- 进程外插件（JSON-RPC）属于 v3 决策门；只有重型工具或跨语言复用时才评估，P3 不做。

## 验收与未完成项

已跑：`cargo test -p rustlua-editor-api`、`cargo test -p editor plugins`、
`cargo test -p editor`、`cargo test -p rustlua-infra --test architecture_edges`、
clippy（editor-api 与 editor，`-D warnings`）、`cargo fmt --all -- --check`、
`scripts/verify/check-docs.ps1`。

未完成/未验证：没有开窗 GUI 目视（诊断窗口与面板只有无头 egui 绘制断言）；
导入器不执行自定义导入；节点无反注册；插件命令的 `shortcut` 只显示未绑定；诊断页的「运行」
没有确认步骤；插件无外部加载入口；`verify.ps1`、workspace
全量测试、Web/wasm 与真实 GPU 未跑。P3-E04 的计划与逐条读数见
[P3 渲染与编辑器能力计划](../tasks/done/P3-渲染与编辑器能力.md)。
