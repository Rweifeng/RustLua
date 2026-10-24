# 编辑器插件 API（P3-E04）

本文记录 P3-E04 的插件接口边界、扩展点、故障隔离与诊断入口，并给出 Lua 项目插件的
决策门结论。实现位于 `engine/crates/rustlua-editor-api`（叶子 crate）与
`engine/crates/editor/src/plugins.rs`（宿主与内置示例）。

## 版本与加载方式

- `PLUGIN_API_VERSION = 1`；manifest 的 `api_version` 不匹配会在注册期拒绝。
- **只支持静态编译插件**：`editor/src/plugins.rs` 维护构造器清单，启动时
  register → load。v1 不加载 `.dll`/`.so`，**不承诺 Rust `cdylib` 稳定 ABI**。
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
| 命令 | `command` + `command_handler` | 菜单点击与诊断页共用同一执行入口 |
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

## 内置示例

`EditorApp` 启动时注册：

- `core`：核心编辑器占位插件，无扩展点。
- `project_stats`：面板 + 菜单 + 两条命令 + 检查器段 + `.rlstats` 导入器声明 +
  `plugin.project_stats.log` 可视化脚本动作节点（模板 `print({text})`）。

示例只读快照、只提交动作，因此禁用/卸载不会改变场景文档。

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
导入器不执行自定义导入；节点无反注册；快捷键只显示未绑定；`verify.ps1`、workspace
全量测试、Web/wasm 与真实 GPU 未跑。P3-E04 的计划与逐条读数见
[P3 渲染与编辑器能力计划](../tasks/done/P3-渲染与编辑器能力.md)。
