# 任务：UI 编辑器与 AI 全流程 - 总览与依赖顺序

- 任务 ID：M76-00
- 状态：已归档（2026-10-24 随 `c247fe99` 入库，与 M77 同笔；归档口径见文末 §归档记录）
- 关联里程碑：M76
- 创建日期：2026-10-24
- 归档日期：2026-10-24（由 `tasks/plan/` 移入 `tasks/done/`，文件名不变）

## 目标

把 [docs/75 UI 编辑工具与 AI 全流程方案](../../docs/75-UI编辑工具与AI全流程方案.md) 落成可执行批次：
M0 契约 → M1 运行时闭环 / M2 编译器与工具 → M3 UI Studio → M4 资产 AI → M5 修改与导入 →
M6 高级控件与三宿主终验。

## 批次与依赖

| 包 | 内容 | 依赖 | 状态 |
|---|---|---|---|
| M76-01 | M0 契约层：`rustlua-ui-spec` 接入 workspace、校验器、能力矩阵、Provider trait、UI tool registry（内部 API + MCP 同源规格） | — | 已完成（2026-10-24；MCP stdio 活体双通道同日收口，见「实现记录」末条） |
| M76-02 | M1 运行时闭环：输入路由/状态、ButtonStates、绑定、动画、裁剪/滚动、UiKey | M76-01 | 已完成（2026-10-24；键盘导航 e2e 与三宿主 golden 未测） |
| M76-03 | M2 Spec/编译器/工具：Spec→`EditorCommand`、`ui_*` 工具、提案/检查点、回放门 | M76-01 | 已完成（`ui_generate_assets` 移交 M76-05） |
| M76-04 | M3 UI Studio + Copilot：工作区、样式/令牌面板、提案箱、预览与输入注入 | M76-02、M76-03 | 完成（dock 标签/面板/层级拖拽/检查器写回/画布移动缩放/提案箱/Undo+Redo/拆分接受/令牌编辑/双通道对拍/真窗口证据均已落地；无吸附与像素级几何门） |
| M76-05 | M4 资产 AI：ImageProvider、生成审阅、切图/九宫格/图集、provenance、体积门 | M76-01、M76-04 | 完成（来源/体积门、本地+远程 Provider、切图落盘 + `ui_slice_asset`、工程级发布门、候选审阅条 + 切图表单、图集页级体积均已落地；远程活体与系统密钥库未接，图集写盘仍归 `assetc cook`） |
| M76-06 | M5 修改/导入/审查：Patch 全链路、Figma/截图导入、审查报告 | M76-03、M76-04 | 完成（拆分接受/导入/审查报告/回放/Undo+Redo/外部编辑保护/全局分流/工程级审查门/`ui/hosts.json` 证据校验/远程 Vision Provider 均已落地；真实 Vision 活体与三宿主矩阵未测） |
| M76-07 | M6 高级控件与三端：虚拟列表、富文本、输入控件、导航栈、三宿主终验 | M76-02..06 | 完成（四个宿主无关内核 + `ui.*` 运行期接线 + 编辑器 1000 项性能基线已落地；富文本绘制、宿主输入事件、三宿主 golden 矩阵与微信真机未测） |

顺序硬约束：`01 → 02/03 → 04`；`05` 与 `06` 可并行于 `04` 后半；`07` 最后。

## 双通道原则

- **内部 API 是主入口**：编辑器内 Copilot/UI Studio 直接调用 `UiToolRegistry`（进程内、强类型、零 JSON-RPC）。
- **外部 MCP 是适配层**：同一 registry 经编解码暴露给 Qoder/Claude/Cursor；不写第二套业务逻辑。
- 两条通道共用工具规格、tier、确认网关、事务/Undo、审计与回放；新增工具只加一次。
- 验收必须包含「同一请求走内部 API 与 MCP，工具名/tier/诊断/最终 diff 一致」。

## 每条包的共同验收

- 契约/逻辑改动必须有单测或 golden，不接受只靠手工目验。
- 三宿主（PC/Web/微信）相关能力必须给出三侧读数；没跑的一端写"未测"，不写"通过"。
- 新门必须跑一次反面对照（故意破坏→必须红）。
- 每包一个提交集，提交信息写清里程碑号；依赖变动同批带 `Cargo.lock`。
- 文档编号/索引/链接满足 `scripts/verify/check-docs.ps1`。

## 影响文件

- `engine/crates/rustlua-ui-spec/**`（M0 起）
- `engine/crates/rustlua-ui/**`、`engine/crates/rustlua-scripting/**`（M1）
- `engine/crates/rustlua-editor-ai/**`、`engine/crates/editor/**`（M2/M3/M5）
- `engine/crates/editor/src/dock.rs`、`engine/crates/editor/src/ai_host.rs`（M3）
- `scripts/verify/verify.ps1` 与快照/golden 文件（逐包）
- `docs/75`、本目录 M76-01..07

## 风险

- M1 改运行时交互，可能破坏现有 UI 屏；先补 golden 再改。
- M3 工作区与现有 Dock/History/PreviewSession 耦合；按"薄客户端"接，不复制状态。
- M4 生成资产涉及外部网络/许可；默认关闭，测试用录制 provider。
- 范围大：任何一包未收口都按"只登记，不开新包"记入该包 §未完成项。

## 验收方式

```powershell
cargo test -p rustlua-ui-spec
cargo test -p rustlua-editor-ai
cargo build -p editor
scripts/verify/verify.ps1 -MvpOnly   # 涉及运行时/UI 时
```

逐包读数回填各包「验收证据」。

## 提交集（已执行：与 M77 一笔带走 `c247fe99`）

本批**没有可单独编译的 M76-only 路径子集**：M77（渲染器下沉）已在索引里带了 52 条重命名，
`engine/Cargo.toml`、`engine/crates/editor/src/main.rs`、`dock.rs`、`examples/game_api/scripts/**`
都被两批共同改动，而 M76 的编辑器侧文件全部引用 HEAD 里还不存在的 `rustlua-ui-spec`。
按路径切会同时带走别人的在途改动，所以先不提交。

实际处置（2026-10-24 22:0x 用户裁定三条：**一笔带走 M76+M77 / 快路 + 提交态三条 / 并行批次在途内容全部排除**）：
提交集不是按关键字 grep 收的，而是按 [M77-00 §M77-01 提交交接清单](M77-00-宿主侧模块化与渲染器下沉-总览与依赖顺序.md)
的桶表（①105 ②5 ③52 ④2 ⑤5 ⑥CHANGELOG）逐条按**内容归属**重建，共 **233 条路径**入笔，
工作区里剩下的 36 条并行在途（game_api 2.0 演示批、`docs/74`、assetc 拆分 5 条、`scene_shot.rs`、
M78-00 核对清单等）一条没带。`docs/README.md` 用 blob 手术只取本批两行（并行会话那行指向未入库的
`docs/74`，整文件入笔会在提交树里留一条死链）。

入库前门（当时读数，复现命令即本节末尾）：

| 门 | 读数 |
|---|---|
| `scripts/verify/check-docs.ps1` | `check_docs_ok docs=69 game_docs=46 h1=117 links=513` |
| `scripts/verify/verify.ps1 -MvpOnly` | `executed=78 expected=78` 全绿（日志 `artifacts/logs/quality/verify-20261024-223006.log`） |
| `cargo test -p rustlua-ui-spec` | 59 passed |
| `cargo test -p rustlua-editor-ai` | 65 passed / 1 ignored |
| `cargo test -p editor --lib` | 249 passed |

三条非平凡读数要留档：

- **`jhyt_m31_pc_release` 首跑红是"缺前置"，不是缺陷**：`release is missing pack/chars.txt`，
  本地 `builds/pc` 是 10-19 的陈旧产物（gitignored）。跑
  `powershell -File scripts/build/package.ps1 -ProjectRoot examples/game_jhyt -Platform pc`
  重打包后单例与整条 `-MvpOnly` 复跑全绿——没有为它改任何代码。
- **pre-commit hook（workspace clippy `-D warnings`）两次拒笔**，M76 文件里共 7 个 lint 点
  （`collapsible_if`×2、`needless_borrows_for_generic_args`、`map_err` 闭包×2、
  `unnecessary_lazy_evaluations`、`type_complexity`、`derivable_impls`）逐个真修；
  没有用 `RUSTLUA_SKIP_PRECOMMIT=1`，也没有 `--no-verify`。
  `type AssetDetails` 第一版写在 `impl` 块里得到 E0658（inherent associated types 未稳定），
  模块级别名才是可编译形状。
- 提交后 `main` 又前进两笔（`7b27d21c`、`309db4d0`，并行会话的 `ui_create_preset` 与 T1 白名单），
  本批的三条提交态复跑跑的是 `c247fe99` 那棵树，不是最新 HEAD。

```powershell
# 复现（本批门）
scripts/verify/check-docs.ps1
scripts/verify/verify.ps1 -MvpOnly
cargo test -p rustlua-ui-spec; cargo test -p rustlua-editor-ai; cargo test -p editor --lib
```

## 实现记录

- 2026-10-24：设计定稿 docs/75；M76-01..07 起稿。
- 2026-10-24：**M76-01 收口**。`rustlua-ui-spec` 接入 workspace（22 项单测绿）；
  `rustlua-editor-ai` 新增 `ui_provider`（Llm/Vision/Image + 录制回放，44 项测试中含 5 项）
  与 `ui_tools`（8 条工具名、强类型请求/返回、`UiToolRegistry`）；`surface.rs` 登记 4×T0、
  2×T1、2×T2 并刷新快照；`ai_host` 对未实现 `ui_*` 返回 `Unavailable`。
  证据与命令见 [M76-01](M76-01-契约层-UiSpec与工具面.md)「验收方式」。
- 2026-10-24：**M76-03 收口**；M76-02/04/05/06/07 按包登记"已交付 + 未接"（状态表同上）。
  跨包读数（同日实测）：`rustlua-ui-spec` 44、`rustlua-editor-ai` 59、`rustlua-ui` 126、
  `editor --lib` 233，`cargo check -p editor --bin editor` 通过。各包范围与未完成项以
  包内文档为准，本表只做索引；未跑的宿主一律登记"未测"。
- 2026-10-24：**M76-05/06 发布门收口**。`review_project` 按 `project.toml` 逐工程扫
  `.ui.json`，坏 JSON/缺资产文件/未审阅生成资产被引用/超体积都是 error；provenance 边车
  从 gitignore 的 `.cache` 移到 `ui/provenance`；`verify.ps1` 新增 "UI asset provenance &
  review gate"。实测：`editor --lib` 235、`ui_asset_gate` 2 工程/1 文档/91 B/0 error，
  删资产文件 → exit 101，改 generated+未审阅 → exit 101/error 4（跑完已恢复）。
  样本：`examples/game_api/ui/shop.ui.json` + `assets/ui/shop_bg.png`。
- 2026-10-24：**M76-05 切图落盘**。`slice_rgba`（纯数据）+ `decode_png_rgba`（导入层）+
  `UiSpecStore::slice_asset`：源 PNG → 裁像素 → 逐张写 PNG + `slices.json`，清单最后写、
  失败清掉本次新建文件。实测：`rustlua-ui-spec` 47、`rustlua-assets-import` 56、
  `editor --lib ui_spec_host` 20；反面对照覆盖越界/重名/空区域/路径逃逸/坏 PNG。
- 2026-10-24：**M76-06 UI Studio Redo**。接受/撤销/重做都以原始字节还原（revision 完全复原），
  新接受清空重做栈；全局菜单/快捷键在 UI Studio 聚焦时优先消费 UI 栈，栈空回落场景历史。
  实测：`editor --lib ui_studio` 5、`editor --bin editor focused_ui_studio` 2。
- 2026-10-24：**M76-04 提案箱拆分接受 UI**。提案持久化原始 ops，面板逐条勾选
  （默认全选），`accept_subset` 复用子补丁提案 + 普通接受路径；只落选中 op，
  撤销/重做同样生效。实测：`editor --lib ui_studio` 6、`ui_spec_host` 21、
  `editor --bin editor panels::ui_studio` 2，`editor --lib` 240。
- 2026-10-24：**M76-04 令牌编辑**。新增 `SetToken`/`RemoveToken` 补丁 op（值与
  `tokens` 契约一致），令牌增删改走提案→接受→撤销；`ProposalDiff.tokens_changed`
  让令牌-only 提案有读数；面板加令牌列表、提交/删除输入与样式类状态矩阵。
  实测：`rustlua-ui-spec` 50、`editor --lib` 241、`editor --lib ui_studio` 7、
  `editor --bin editor panels::ui_studio` 2；反面对照覆盖删被引用令牌、空名/对象值、
  删不存在令牌（整批拒绝且文档字节不变）。
- 2026-10-24：**M76-04 结构编辑 + 真窗口证据**。补丁契约新增 `Move`（校验按"执行到
  该 op 时"的存在性判断，先删后插/插容器再移入合法，改根/进自己子树/叶子当父级仍然拒绝）；
  几何助手 `ResizeHandle`/`resize_rect`/`offsets_for_rect`；层级拖拽、复制/删除/包裹、
  检查器布局/样式/事件写回、画布拖动与 8 手柄缩放全部走提案；`gui_evidence` 新增
  `dpi1.5-1280x720-ui-studio` 步骤（演示文档写进 `.cache/`）。实测：`rustlua-ui-spec` 57、
  `editor --lib` 244、`panels::ui_studio` 4、`gui_evidence::tests` 7、真窗口 13 张 PNG /
  `failed=0`（studio 步骤 lit=1.000）。既有红一处：`ui_source_layout` 基线被并行会话的
  `index.scene`/`api_prefab.scene` 在途改动拉开（源码与基线无 diff，未刷基线）。
- 2026-10-24：**M76-06 撤销/重做的外部编辑保护**。栈项同时记目标字节与"写回前必须
  仍是这些字节"，外部改过就 `StaleRevision`、不动文件、不丢栈。实测：
  `editor --lib ui_studio` 8、`editor --lib` 242；反面对照覆盖撤销/重做两个方向。
- 2026-10-24：**M76-05 切图登记成 MCP tool**。`ui_slice_asset`（T2）接手工矩形 →
  `UiSpecStore::slice_asset`，与出图共用确认门；工具面快照刷新（9 条 UI tool）。
  实测：`rustlua-editor-ai` 59、`editor --bin editor "ai_host::tests::ui_tool"` 3；
  反面对照：越界区域在落盘前拒绝且不创建输出目录。
- 2026-10-24：**M76-06 宿主证据 + 远程 Vision；M76-07 运行期接线 + 性能基线**。
  M76-06 新增 `ui/hosts.json` 证据通道（命令/检查项/产物/ revision 四件套齐全且产物自证
  才把宿主翻成 `tested`，否则保持 `untested` 并给 `review.host_*` 诊断）与
  `RemoteVisionProvider`（OpenAI/Anthropic 兼容、密钥只从环境读、429/5xx 重试、活体用例
  默认 ignore）。M76-07 把四个内核挂进 `ui` 表（`virtual_list`/`rich_text`/`text_input`/
  `nav_stack` 命令表），状态进 `UiState.advanced` 并随场景切换清空；
  `editor --perf-baseline` 新增 `ui_virtual_list_1k`。实测：`rustlua-editor-ai` 63+1ignored、
  `editor --lib` 248、`editor --lib review` 11、`rustlua-ui` 132、
  `rustlua-scripting --lib ui::` 10、`--perf-baseline` 32/0/ok（1000 项 p50 0.021ms、
  34 项/帧、槽位池 35）、Lua API 快照 308 函数与文档门 `lua_api_docs_ok`。
  既有红一处：并行会话把 `examples/game_api/scripts/main.lua` 改走 `lab.lua`，
  `rustlua-scripting` 的 `sample_project_*` 2 项仍断言旧链（123/2，与本节改动无关）。
- 2026-10-24：**M76-05 候选审阅 + 切图入口 + 图集页体积**。UI Studio 新增候选缩略图条、
  来源详情（provider/model/seed/prompt/许可/used_by）、许可输入与「标记已审阅」（写
  `ui/provenance`）、手框切图表单（`name,x,y,w,h`）与资产缓存；`atlas_page_costs` 用
  `atlas.json` 真实装箱得到页级字节，`AssetEntry.atlas_page` 让同页多区域只计一次，
  页 id 与 cook 文件名同源（修正了首版未清洗键导致查不到页成本的缺陷）。实测：
  `rustlua-ui-spec` 58、`rustlua-assets-import` 56、`rustlua-editor-ai` 59、
  `editor --lib` 247、`ui_spec_host` 22、`ui_studio` 12、`panels::ui_studio` 6、
  `ui_asset_gate` 1；`editor --bin editor ui_` 36 + 1 既有红（并行会话在途的
  `index.scene`/`api_prefab.scene` 基线漂移，未刷基线）。
- 2026-10-24：**MCP stdio 活体接入 `examples/game_api`（交付收口）**。连接器拉起
  `editor --project <路径> --mcp-stdio`，实测 43 条 tool（9 条 `ui_*`；tier 分布 T0=14、T1=15、
  T2=8、T3=6）与 5 个 resource。T0 读通：`ui_export_spec` 4ms 返回 `ui/shop.ui.json` 全文与
  `revision=4421840451047808`，`ui_list_assets` 158ms 带 provenance。修掉两条只有真实 JS 客户端
  才能暴露的缺陷：
  - **revision 超出 JSON 安全整数**：u64 内容哈希过 `JSON_SAFE_REVISION_LIMIT`（2^53）时被 f64
    舍入，客户端回传的 `base_revision` 必然判 `StaleRevision`，`ui_apply_patch`/`ui_apply_spec`
    在 MCP 通道上永远写不进。新增 `revision_hash()`（`compiler.rs`）压进安全位宽 +
    `revision_hash_stays_inside_the_json_safe_range`、`revision_survives_a_json_number_round_trip`
    两条门；活体复跑 → 提案 `uip_793631c4d8a22f71`、`nodes_changed=1`。
  - **一条未确认的写挂死整条 stdio 通道**：协议线程无限 `recv()` 等主线程，队列里只要有一条没
    批准，后续所有调用（含只读）全部超时。改为 `DEFAULT_REPLY_WAIT`（30s）有界等待 +
    `ToolError::QueuedForConfirmation`（按 tier 给读数），动作仍在 GUI「AI 动作确认」批准时执行；
    新增 `unconfirmed_write_times_out_as_queued_instead_of_wedging_the_transport` 与
    `interactive_policy_queues_the_write_until_the_host_approves_it`（含显式放行策略的反面对照）。
  - 确认网关在 stdio 上按设计生效（实测读数）：不带环境变量时 T1/T2/T3 四条写全部只排队、
    只读照常 3ms 应答；`RUSTLUA_EDITOR_AI_AUTO_T1=1` 时 T1 `create_entity`/`ui_apply_patch` 当帧
    执行、T2 `ui_promote_asset` 与 T3 `new_scene` 仍排队——启动行把四个策略位打进 stderr，
    读数不再靠猜。`--help` 同批登记这条开关。
  - 门读数（同日复跑）：`rustlua-ui-spec` 59、`rustlua-editor-ai` 65（+1 ignored）、
    `rustlua-ui` 132、`editor --lib` 249 全绿；`editor --bin editor ui_` 36/1（仍是并行批次的
    `.scene` 基线漂移）。
  - 提交状态：本批与 M77 同笔入 `c247fe99`（切不出能单独编译的 M76-only 子集，见本文「提交集」）。

## 未完成项 / 已知限制

- M76-02..07 均未完全收口，未接项按各包「未完成项 / 已知限制」为准：
- M76-02：已完成输入路由/状态、绑定、动画发布与 ScrollView 裁剪滚动（引擎侧 12 项 UI
  测试含 3 项滚动门）；键盘导航端到端与三宿主 golden 未测；
- M76-04：吸附/对齐辅助线、量距工具、状态/绑定段写回、交互回放的像素证据；
  - M76-05：审阅只有缩略图条（无大图对比）、切完仍需人工登记 `assets[]`、图集写盘归
    `assetc cook`、远程活体与系统密钥库未接；
  - M76-06：真实 Vision 活体（已有真实 HTTP 实现与重试，但未联网跑）、三宿主产物
    （通道已就绪，仓库内没有真实 PC/Web/微信证据，报告照旧 `untested`）；
  - M76-07：富文本绘制（目前只解析）、虚拟列表的宿主绘制循环、宿主输入事件/IME 桥、
    三宿主 golden 矩阵、微信真机；性能只有逻辑层读数（无堆计数器、无发布档）。
- M76-01 只交付契约与工具面；`ui_*` 执行体在 M76-03，`ui_generate_assets` 在 M76-05。
- 双通道（MCP stdio）活体收口后仍剩的口子：
  - 排队写的回话是**有界**的：30s 到点先回「已排队」，之后 GUI 批准时结果送回早已丢弃的通道，
    客户端拿不到最终 diff/失败原因；批准前重复提交只会在队列里多排一条。超时值目前编译期固定
    （`DEFAULT_REPLY_WAIT`），没有按客户端能力协商的通道。
  - `tools/list` 帧里没有 tier（MCP 规格本身不带），客户端只能从 `ai_tool_surface.json` 或
    `rustlua://lua-api/surface` 对齐；tier 语义对客户端不可见＝确认网关对客户端是"看不见的一层"。
  - `ui_preview_spec` 一次返回约 4.9 MB base64 PNG 帧，没有缩略图/资源 URI 分页；真实客户端会
    把这帧整份吃进内存。
  - 项目级 custom MCP 注册：Qoder 设置工具对 `scopeKind=project` 直接 `MCP_AGENT_INPUT_INVALID`
    （同载荷去掉 scope 在用户级成功），所以本批连接器只能建在**用户级**；跨工程共享同一条
    `--project` 路径，不是任务原定的项目级形态。新注册的 tool 面要**新会话**才可见。
  - 三宿主：`ui/hosts.json` 只有消费端（校验四件套），仓库里没有生产端，PC/Web/微信在任何
    报告里都只会是 `untested`；这条与 M76-06/07 的"未测"是同一个缺口，不要重复计数。

## 归档记录

- 2026-10-24 归档：M76-00..07 八份随批移入 `tasks/done/`，代码在 `c247fe99`。归档依据是
  M77-00 定下的**提交态三条**已在该提交树上复跑（①`cargo check -p engine -p editor -p game_web
  --all-targets` = `CHECK_EXIT=0`；②`RUSTLUA_REQUIRE_GPU=1 cargo test -p rustlua-gpu -p rustlua-render
  -- --test-threads=1` = 138 + 153 + 1 全绿、`GPU_TEST_EXIT=0`；③反面对照见 M77-00 §归档记录）。
- **归档时没收口什么**（口径仍是「只登记，不开新包」，逐条在各包 §未完成项）：键盘导航 e2e、
  三宿主 golden 矩阵与 `ui/hosts.json` 生产端、吸附/量距、富文本绘制循环、宿主输入事件/IME 桥、
  图集写盘（归 `assetc cook`）、远程 Provider 与系统密钥库活体、`-Web` 与 `-Benchmarks` 全量段。
- **本批没做、留给下一批的一件事**：编辑器侧 offscreen renderer 合并（M77-02 唯一不依赖度量判据的
  硬收益项）仍阻塞在 `editor/src/preview_session.rs` 与 `panels/scene_view/mod.rs` 上。
- 文内的 `tasks/plan/*` 字样按 M72 那一批的口径处理：剩下的命中都是**批号通配或历史引文**，
  可打开的旧文件路径 0 处。复现（判"可打开的旧文件路径"）：
  `grep -c "tasks/plan/M7[67]-[0-9][0-9]*-[^ )、，。]*\.md" tasks/done/M76-00-*.md tasks/done/M77-00-*.md`。
