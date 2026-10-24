# 任务：M0 契约层 - UiSpec 校验器、能力矩阵、Provider 与工具面

- 任务 ID：M76-01
- 状态：已完成（2026-10-24）
- 关联里程碑：M76（方案 M0）
- 创建日期：2026-10-24

## 目标

让 AI 与编辑器之间先有一层**可编译、可单测、可版本化**的契约：`UiSpec`/`UiPatch`/`UiProposal`/
`AssetProvenance`，加上诚实的能力矩阵与结构化 Provider trait，并把 UI 工具面登记进现有 tool 快照门。

## 影响文件

- `engine/Cargo.toml`（members + workspace.dependencies）
- `engine/crates/rustlua-ui-spec/**`（新增 crate；能力矩阵改为可序列化）
- `engine/crates/rustlua-editor-ai/src/surface.rs`（UI tool 规格）
- `engine/crates/rustlua-editor-ai/src/lib.rs`（Provider / UI tool 导出）
- `engine/crates/rustlua-editor-ai/src/ui_provider.rs`（新增：Llm/Vision/Image trait 与录制回放类型）
- `engine/crates/rustlua-editor-ai/src/ui_tools.rs`（新增：`UiToolRegistry` 强类型内部 API 与工具名常量）
- `engine/crates/rustlua-editor-ai/ai_tool_surface.json`（快照刷新）
- `engine/crates/editor/src/ai_host.rs`（执行体落地前对 `ui_*` 明确返回 `Unavailable`）

> 偏差：计划原写“`src/provider.rs` 新增”，但该文件是 M75 的 HTTP Provider；M76 的
> Provider 抽象改落 `src/ui_provider.rs`，避免把两种职责混在一个模块。

## 交付项

1. ✅ `rustlua-ui-spec` 进 workspace（members + workspace.dependencies），`cargo test -p rustlua-ui-spec` 22 passed。
2. ✅ `Capabilities::core()` 与当前运行时对齐（5 节点、click-only、无状态样式），并有测试钉住；
   能力矩阵改为 owned `String` + serde，Provider 请求与快照可携带同一份能力面。
3. ✅ `validate_spec` / `validate_patch` 覆盖：版本、key 唯一、能力门、令牌存在、图片引用、布局冲突、
   事件函数名、绑定属性白名单、patch 根字段白名单、不可删根（13 组单测）。
4. ✅ Provider trait 不依赖具体厂商：`src/ui_provider.rs` 定义 `LlmProvider`/`VisionProvider`/`ImageProvider`，
   `Recording*` 把请求/输出规范 JSON 与图像字节写入 `ProviderTrace`，`Replay*` 离线重放（5 项单测）。
5. ✅ UI tool registry 规格：工具名常量与强类型请求/返回在 `src/ui_tools.rs`，`UiToolRegistry` 为进程内主入口；
   `surface.rs` 用同一组常量生成规格，MCP 适配层只做编解码。
6. ✅ UI tool 规格进 `surface.rs`：T0 `ui_export_spec`/`ui_validate_spec`/`ui_preview_spec`/`ui_list_assets`，
   T1 `ui_apply_spec`/`ui_apply_patch`，T2 `ui_promote_asset`/`ui_generate_assets`/`ui_slice_asset`
   （后者由 M76-05 补登记）；快照已刷新。
   **本期只登记规格、tier 与内部 API 签名**；执行体在 M76-03，宿主当前对 `ui_*` 返回 `Unavailable`。

## 验收方式

```powershell
cargo test -p rustlua-ui-spec
cargo test -p rustlua-editor-ai
AI_TOOL_SURFACE_BLESS=1 cargo test -p rustlua-editor-ai tool_surface
cargo build -p editor
```

已跑读数（2026-10-24）：

- `cargo test -p rustlua-ui-spec` → 22 passed / 0 failed。
- `cargo test -p rustlua-editor-ai` → 44 passed / 0 failed。
- 快照反面对照：把基线里的 `ui_apply_patch` 改成 `ui_apply_patch_broken` →
  `surface::tests::tool_surface_matches_baseline` FAILED（exit 101）；恢复后回绿。
- 契约反面对照由单测承担：`duplicate_node_keys_are_rejected`、`unsupported_event_is_rejected`
  （`core()` 下把 `click` 换成 `hover`）。
- `cargo build -p editor` → 通过（42.44s）。
- `scripts/verify/check-docs.ps1` → `check_docs_ok docs=69 game_docs=46 h1=117 links=507 ps1_bom_ok=57 ps1_parse_ok=57`。
- `cargo fmt -p rustlua-ui-spec -p rustlua-editor-ai -p editor -- --check` → 通过。
- `cargo clippy -p rustlua-ui-spec -p rustlua-editor-ai --all-targets -- -D warnings` → 通过（3.31s）。

## 风险

- `UiNode` 同时 `flatten` 内部标签枚举与 `extra` 映射，serde 可能把 `type` 捕进 extra 或拒绝；
  若冲突，改为显式 `#[serde(flatten)]` 顺序或自定义 `Deserialize`。
- 工具面快照是既有门，新增规格必须同批刷新快照，否则 `rustlua-editor-ai` 测试红。
- 空实现不要登记成"已实现"：Provider trait 只有类型与录制载体，真实网络实现留给 M76-05。

## 实现记录

- 2026-10-24：新增 `rustlua-ui-spec` crate（spec/capabilities/asset/patch/proposal/validate）与 22 项单测；
  接入 workspace（members + workspace.dependencies），能力矩阵改为可序列化。
- 2026-10-24：新增 `rustlua-editor-ai/src/ui_provider.rs`（Llm/Vision/Image trait + `ProviderTrace` 录制/回放，
  5 项单测）；`src/ui_tools.rs` 定义 8 条工具名、强类型请求/返回与 `UiToolRegistry`。
- 2026-10-24：`surface.rs` 登记 8 条 UI tool（4×T0、2×T1、2×T2）并刷新 `ai_tool_surface.json`；
  `ai_host.rs` 对 `ui_*` 返回 `Unavailable`（M76-03 前无执行体）。
- 门读数：`cargo test -p rustlua-ui-spec` 22/0；`cargo test -p rustlua-editor-ai` 44/0；
  快照篡改反面对照 exit 101，恢复后 exit 0；clippy `-D warnings`、fmt `--check` 均通过。
  期间按 clippy 修正：`UiPatchOp::Insert` 的 `node` 装箱（避免大枚举变体），三处嵌套 `if` 收敛为 let chain。
- 2026-10-24：**双通道在真实 stdio 上对拍收口**。Qoder 连接器拉起 `editor --project <路径>
  --mcp-stdio` 实测 `examples/game_api`：43 条 tool（9 条 `ui_*`，T0=14/T1=15/T2=8/T3=6）与 5 个
  resource 全部拉到；`tools/list` 的名字/输入模式与 `ai_tool_surface.json` 一致，进程内的
  `ui_tool_results_match_between_internal_api_and_mcp` 覆盖工具名/tier/诊断/diff 同源。
  两条只有真实客户端才能暴露的缺陷在本批修掉：契约层 revision 压进 JSON 安全整数位宽
  （`revision_hash` + 2 条门），传输层对排队写做有界等待并回明确读数（`QueuedForConfirmation`
  + 2 条门，含"显式放行策略必须当帧执行、一条都不进队列"的反面对照）。
  读数：`rustlua-ui-spec` 59/0、`rustlua-editor-ai` 65/0（+1 ignored）、`editor --lib` 249/0。

## 未完成项 / 已知限制

- `ui_*` 执行体（Spec 编译、预览、提案、资产生成）在 M76-03 落地；当前工具面已对 MCP 客户端可见，
  调用会得到明确 `ToolError::Unavailable`，不是静默成功。
- `ui_export_spec` 等的 MCP 结果编码（PNG 字节 → base64/资源 URI）留给 M76-03 的适配层；
  本期内部 API 直接传 `Vec<u8>`。
- MCP `tools/list` 帧按规格不带 tier：客户端要判断一条写动作会不会被确认网关拦下，只能读
  `ai_tool_surface.json` 或 `rustlua://lua-api/surface`。tier 对客户端不可见＝确认网关是客户端
  看不见的一层，只能在读数（`QueuedForConfirmation`）里事后知道。
- `ui_preview_spec` 把整帧 PNG 以 base64 塞进一次 tool 返回（实测约 4.9 MB），没有缩略图或
  资源 URI 分页；剩余通道侧限制见 `M76-00` §未完成项。
