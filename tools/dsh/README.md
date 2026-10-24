# DSH × RustLua：一句话创建 UI 预设

这条流水线把 **DSH（DeepSeek Harness）** 接到 RustLua 编辑器的 MCP 能力层上。接好之后，在 DSH 里说一句
「做一个酒馆对话界面」即可：DSH 的模型读 `rustlua://ui/guide`、产出 `UiSpec`、调用 `ui_validate_spec` /
`ui_preview_spec` / `ui_create_preset`，编辑器负责校验、预览、落盘并返回 revision。

## 1. 前置

```powershell
cd engine
cargo build -p editor
```

产物：`engine/target/debug/editor.exe`。编辑器必须能打开目标工程（默认 `examples/game_api`）。

## 2. 注册 MCP server

DSH 的 MCP 客户端是 `@deepseek-ai/dsh-mcp-client`，每个 server 一条配置。仓库里的安装器会往
`%DSH_PROFILE_DIR%\cordis.patch.yml`（默认 `C:\Users\<你>\.dsh\profiles\desktop\cordis.patch.yml`）
写一段带标记的配置，重复运行先删旧块再写新块：

```powershell
node tools/dsh/install-mcp.mjs
# 换工程/换二进制：
node tools/dsh/install-mcp.mjs --project examples/game_jhyt --editor engine/target/debug/editor.exe
# 撤销：
node tools/dsh/install-mcp.mjs --remove
```

写入的形状（由安装器生成，路径会转成 `C:/...`）：

```yaml
- id: mcp-rustlua
  name: '@deepseek-ai/dsh-mcp-client'
  config:
    serverName: 'rustlua'
    transport: stdio
    command: 'C:/GitHub/RustLua/engine/target/debug/editor.exe'
    args: ['--project', 'C:/GitHub/RustLua/examples/game_api', '--mcp-stdio']
    cwd: 'C:/GitHub/RustLua/engine'
    env:
      RUSTLUA_EDITOR_AI_AUTO_TOOLS: 'ui_create_preset'
    toolCallTimeoutMs: 120000
    failOnStartupError: false
```

DSH 会重载这个配置项并拉起编辑器；如果工具没出现，重启 DSH。工具名形如
`mcp__rustlua__ui_create_preset`（与 Claude Code / Codex 的命名一致）。

> **窗口**：`editor --mcp-stdio` 仍然是完整的 egui 编辑器，会打开一个窗口；这是确认网关的宿主。
> 不要关掉它，关掉等于断开 MCP 连接。

## 3. 一句话怎么走

用户说：

> 在 game_api 里做一个酒馆对话 UI 预设，深色木质风，标题「酒馆」，三个选项：麦酒、炖肉、离开。

DSH 侧的调用顺序（由 MCP `initialize.instructions` 与 tool 描述引导，不需要用户记）：

1. `resources/read rustlua://ui/guide` —— 拿 `UiSpec` 契约、能力矩阵与完整示例；
2. `ui_validate_spec { spec }` —— 修到 `diagnostics` 里 0 个 error；
3. `ui_preview_spec { spec }`（可选）—— 看布局；
4. `ui_create_preset { document, spec, preview: true }` —— 落盘并返回 revision + 预览 PNG。

`ui_create_preset` 的边界：

- **只新建**：`document` 已存在时返回错误，提示改用 `ui_export_spec` + `ui_apply_patch`，绝不覆盖；
- 路径必须是工程内 `.ui.json` 相对路径（拒绝绝对路径、`..`、盘符）；
- 与 UI Studio 共用 `submit_spec → accept` 的同一条写路径，落盘前过能力矩阵与校验器；
- 默认预览 640×360，可用 `width/height/scale` 调整；`preview: false` 可省掉预览图。

## 4. 权限与安全

- 该 server 的 `RUSTLUA_EDITOR_AI_AUTO_TOOLS=ui_create_preset` **只**自动放行这一个 T1
  tool；其它 T1（场景编辑、`ui_apply_patch`）仍会排队等确认。
- **T2/T3 仍会弹编辑器确认窗**：删除实体、写 Lua、切换工程、Play 控制都需要人工点「批准」。
- 无头/Docker 里没有确认通道时，T2/T3 会被拒绝而不是挂死；要跑纯无头验收用
  `scripts/ai/mcp-ui-preset-smoke.mjs`（它只调用被白名单放行的 `ui_create_preset`）。
- 密钥、模型 Provider 与 MCP 无关：DSH 只通过 stdio 传结构化 tool 参数。

## 5. 验收

不经过 LLM 的确定性验收（同一 MCP 通道、同一 tool）：

```powershell
node scripts/ai/mcp-ui-preset-smoke.mjs
# → MCP_UI_PRESET_OK document=ui/mcp_smoke.ui.json revision=... preview_bytes~=... cleaned=1
```

保留产物以便打开查看：

```powershell
node scripts/ai/mcp-ui-preset-smoke.mjs --keep
```

Rust 侧的门：

```powershell
cd engine
cargo test -p rustlua-editor-ai --lib            # tool 面快照、指南示例校验
cargo test -p editor --bin editor ui_create_preset
cargo test -p editor --lib ui_spec_host           # 新文档提案 + 竞态
cargo test -p editor --test ui_asset_gate         # 新预设不破坏工程级资产门
```

### 本次实跑读数（2026-10-24 22:4x）

| 项 | 读数 |
|---|---|
| `cargo test -p rustlua-editor-ai --lib` | 70 passed / 0 failed / 1 ignored |
| `cargo test -p editor --bin editor ui_create_preset` | 1 passed |
| `cargo test -p editor --lib ui_spec_host` | 25 passed |
| `cargo test -p editor --test ui_asset_gate` | 1 passed（仓库内 `.ui.json` 全过） |
| `node scripts/ai/mcp-ui-preset-smoke.mjs` | `MCP_UI_PRESET_OK`，preview ~922 KB，cleaned=1 |
| DSH 活体 | `tools=45`、`resources=6`；`ui/forge.ui.json` revision `755675877262958` |
| 预览回执 | `content=[text,image]`，text/structuredContent 均不含 base64；默认 640×360 |

> 预览 PNG 约 900 KB 是当前 `render_preview` 的编码结果（近未压缩）；它走 image block，
> 不再计入文本上下文。要更小可传 `preview:false` 或降 `width/height`。

## 6. 排错

| 现象 | 检查 |
|---|---|
| DSH 工具列表里没有 `mcp__rustlua__*` | `cordis.patch.yml` 是否有 `mcp-rustlua` 块；重启 DSH；看 DSH 日志里 `dsh-mcp-client` 的连接错误 |
| 连接后立刻断开 | 先手动跑 `editor.exe --project <工程> --mcp-stdio`，确认窗口能开、工程能加载 |
| `ui_create_preset` 报「已存在」 | 换文件名，或 `ui_export_spec` 拿 revision 后用 `ui_apply_patch` |
| 模型生成的字段被拒 | 读 `rustlua://ui/guide` 的属性白名单；`container.direction` 是 `v_box`/`h_box`，不是 `vbox` |
| 预览太大 | 传 `preview: false`，或 `width/height` 降到 640×360 |
| 工具调用超时 | 首次启动编辑器要加载工程，`toolCallTimeoutMs` 已设 120s；仍超时看窗口是否卡在弹窗 |

## 7. 为什么不是「给编辑器加个聊天框」

这条链路复用的是 M75/M76 已有的能力层：43+ 条 typed tool、T0–T3 风险分层、确认网关、事务/Undo、
`ai_tool_surface.json` 快照门、无 LLM 回放门。DSH 只是又一个 MCP 客户端；它不绕过 `EditorCommand`，
也不直接改 `.ui.json`。新增的 `ui_create_preset` 只是把「新文档的提案 → 接受」收成一个 T1 动作，
写路径仍与 UI Studio 相同。
