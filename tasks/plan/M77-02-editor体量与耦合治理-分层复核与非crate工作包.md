# 任务：editor 体量与耦合治理（M77-02 详版）

- 任务 ID：M77-02
- 状态：进行中（crate 级拆分这一步**已按判据裁定"不做"**，见 §0；本文件持有对它的名单级复核、
  外部评审第 12.1 条的逐条裁定，以及**那条裁定没有覆盖的两条判据**的度量方案与替代手段）
- 关联里程碑：M77
- 创建日期：2026-10-24
- 上游：[M77-00 批次总览](M77-00-宿主侧模块化与渲染器下沉-总览与依赖顺序.md) §M77-02 与其下的
  《M77-02 裁定：本批不做 crate 级拆分》（同日、并行会话产出，是本包**编译时长轴的权威结论**）

## 目标

外部评审 12.1 的实质不是"代码多"，而是七件事挤在一个改动单元里：**编译时间、变更冲突、测试隔离、
内存占用、历史快照成本、UI Studio 与主编辑器耦合、AI surface 与文档/Undo 耦合**。本包交付：

1. 用**名单级实测**复核上游那条"不做"裁定——结论是它成立，且比它自己算的更成立（可搬上限
   15.3% → 12.6% → 零改造净上限 3.8%）；
2. 把七条风险逐条对到**真实载体**，其中五条的载体不在 crate 边界上（`EditorApp` 199 字段、
   offscreen 装配前导、`recovery.rs` 整档序列化、内联测试的 741 条同批重编、`ai_host.rs` 直呼 shell），
   因此给出不建 crate 也能收的六个可执行工作包；
3. 补上上游裁定**没量**的两条判据（测试隔离、内存）——评审原话点名了它们，没有读数就是空白。

## 0. 与上游裁定的关系：认账 + 名单级复核

上游的论证（2026-10-24，主工作区 warm `target/`）：`touch panels/inspector.rs; cargo build -p editor`
= **19s**，touch 全部 115 文件 = **23s**，`cargo check` 同触发点 = 6s ⇒ 19s 里约 13s 是 codegen+link；
计划点名的 core+services **20 个模块 = 13,164 行 = 15.3%** 是可搬上限，按最乐观线性假设
0.153 × 19 ≈ 2.9s ⇒ 必低于 20% 门；且**没有任何 crate 依赖 editor**（本日复核仍为 0，
`grep -rn '^editor' engine/crates/*/Cargo.toml` 零命中）。**这条推理我认可，不重开。**

我加的是一层**按模块的耦合实测**：把上游名单里 20 个文件逐个查 egui / `EditorApp` / `wgpu::` 命中
（口径：全文件计数，含测试；复现命令见附录）：

| 分组 | 文件（行数） | 小计 | 占 editor |
|---|---|---|---|
| **零改造可搬**（egui=0 且 EditorApp=0） | `play_control` 667、`file_watcher` 705、`resource_ops` 382、`patch_history` 528、`schema_audit` 420、`selection` 336、`resource_refresh` 176、`document_space` 52 | **3,266** | **3.8%** |
| 只差 1–3 处 egui（改枚举即可） | `schema_fields` 1,844（1）、`layout` 820（3） | 2,664 | 3.1% |
| 要先解 `EditorApp` 直呼 | `command_router` 1,362（9）、`import` 724（8）、`publish` 795（4）、`history` 548（5）、`runtime_sync` 609（1）、`document` 850（1） | 4,888 | 5.7% |
| **搬不动**（GPU/egui 同生） | `preview_session` 996（egui 25 / wgpu 36）、`play_sync` 554（17 / 8 / 10）、`geometry` 194（26）、`shader_watch` 602（4 / 7 / 10） | **2,346** | **2.7%** |
| 上游名单合计 | 20 个模块 | 13,164 | 15.3% |

⇒ **修正**：上游的"15.3% 上限"里，2,346 行（`preview_session`/`play_sync`/`geometry`/`shader_watch`）
根本不在"纯数据与命令层"的口径内——把它们算进可搬面，是因为 M77-00 原名单写的是"不含 egui"而实测
这四个都有 4–36 处 egui/wgpu 命中。扣掉后**毛上限 12.6%**（10,818 行）；再分别扣掉需要先做 shell
解耦的 5.7%（4,888 行）与要先改 egui 类型的 3.1%（2,664 行），**零改造净上限只剩 3,266 行 = 3.8%**
⇒ 0.038 × 19s ≈ **0.7 秒**。净收益比上游自己算的 2.9s 还小一个单位，而那 8.8% 的前置改造成本
（`EditorApp` 直呼改 trait/参数 + egui 类型替换）**不在这次 A/B 的分子里**。

**这不是"我又测了一遍"**：上游按行数算上限，我按每行的耦合类型算可达性。两者结论同向，
且本表同时给出了"哪 3,266 行是免费的"——这是未来真要切时唯一不需要前置改造的部分。

## 1. 现状实测（2026-10-24）

复现命令见附录。行数口径 = `engine/crates/editor/src/**/*.rs`。**本日两次读数相差 5 行**
（86,081 vs 上游 86,076），是并行会话在途编辑所致，不是口径分歧。

### 1.1 体量与构成

| 项 | 读数 |
|---|---|
| editor src | **86,081 行 / 115 文件**（全仓第一；第二名 `engine` 21,778、第三名 `rustlua-gpu` 20,136） |
| `src/panels/` | 35,550 行 / 45 文件 = **41.3%**；前 5：`inspector` 3,043、`performance` 2,563、`schema_inspector` 2,378、`hierarchy` 2,354、`domain_data` 2,190 |
| `src/main.rs` | 3,528 行（`EditorApp` 定义 + 4 个 `impl EditorApp` 块） |
| `src/actions/` | 4,755 行 / 14 文件（非测试 egui 命中仅 20 次 ⇒ 命令层已是半纯逻辑） |
| 最大顶层单文件 | `ui_spec_host` 2,035、`ai_host` 1,875、`schema_fields` 1,844、`perf_baseline` 1,796、`framegate` 1,731 |
| 测试 | **741 个 `#[test]`**（src 740 + `tests/ui_asset_gate.rs` 1），106 处 `#[cfg(test)]`；目录式测试仅 `panels/scene_view/tests/` 2,509 与 `actions/tests/` 1,369 |
| 测试行占比 | **≈33%（估算，非实测）**：花括号配对统计 `#[cfg(test)]` 块 ≈23,856 行 + 目录式测试 ≈4.5K。**认领时先脚本化转成实测** |

### 1.2 真实耦合载体是 `EditorApp`，不是 crate 边界

| 事实 | 位置 |
|---|---|
| `struct EditorApp`：闭合约 923 行 ⇒ **约 199 个字段**（22 处 `#[cfg]` 门控） | `src/main.rs:507` |
| 非测试引用 **66 文件 / 296 次**（顶层 27、panels 37、actions 12） | `grep -rln EditorApp` |
| **没有独立中心状态结构**：面板就是 `impl EditorApp` 的绘制方法（`panels/mod.rs:1` 明写） | — |
| 跨模块 `impl`：命令路由把方法写在 main 的类型上 | `src/command_router.rs:24` |
| AI 接缝同形：`impl ToolHost for EditorAiHost<'_>` 借 `&mut EditorApp` | `src/ai_host.rs:873` |
| GPU device 只有一份（eframe 的 device/queue），Scene View 与 Play 共用 | `src/main.rs:736` |

⇒ **推论**：只要 199 字段的共享可变状态是面板的唯一入口，切 crate 得到的是**同一批耦合的跨 crate
`pub` 改写**。所以本包把"字段分组 + 接缝契约化"排在任何搬文件之前（WP-C/WP-D）。

### 1.3 扇入实测（符号级）

`lib.rs` 把各模块类型**平铺再导出**（`pub use document::{DiskStatus, DocumentError, PlaySession,
ProjectDocument, SceneDocument}` 等），按 `crate::document::` 形态 grep 会严重漏数（实测
`crate::selection::` 命中 1 文件，而 `SelectionModel` 命中 8）。**下表一律按符号名统计**：

| 符号 / 切面 | 扇入文件数（非测试） | 出向 | 判定 |
|---|---|---|---|
| `SceneDocument` | **35** | `file_watcher`、`RecoveryManager` | 底座；只可能"往下切"，不可能独立成小 crate |
| `CommandKind`（history/patch_history） | **16** | `document_space` | 与 document 同层，单切成环 |
| `PlayController` | 8 | 零 egui、零 EditorApp、零 wgpu | **全仓最干净的 667 行**，免费的 core 候选 |
| `SelectionModel` | 8 | 无 | 免费 core 候选 |
| `RecoveryManager` | 6 | `SceneDocument` | 与 document 同生 |
| `UiStudio` + `ui_spec_host` | 7 个消费文件（`ai_host`/`command_router`/`gui_evidence`/`main`/`panels/resource_browser`/`panels/ui_studio`/`review`） | `ui_studio.rs:7` 只 `use crate::ui_spec_host::{UiHostError, UiHostErrorCode, UiHostResult, UiSpecStore}`；`ui_spec_host.rs:7` 注释自证"刻意不依赖 `EditorApp` / egui：编辑器 shell、MCP 适配层与测试共用同一份实现" | **最干净的整块切面（3,635 行）**，但它已经"事实上是一个 crate"，只差边界 |
| `EditorLayout` | 3 | 3 处 egui | 改枚举后可搬 |

### 1.4 GPU / wgpu 接缝面（非测试命中）

| 文件 | `wgpu::` | 用途 |
|---|---|---|
| `preview_session.rs` | **36** | Play 离屏 target 纹理 + egui 原生纹理绑定 |
| `panels/scene_view/render.rs`（652 行） | 18 | Scene View 离屏 target + 缓存失效 |
| `shader_watch.rs` / `play_sync.rs` / `panels/game_view.rs` | 10 / 10 / 10 | 覆盖目录、帧同步、Game 视图纹理 |
| `main.rs` / `panels/scene_view/mod.rs` | 6 / 4 | `gpu` 字段与纹理释放 |

⇒ `editor/Cargo.toml` 的 `wgpu = "27"` **删不掉**（editor 自己建纹理）。这是 §2 里否掉
`editor-preview` 的直接依据。

### 1.5 offscreen 的真实现状（M77-01 之后，口径要改）

两个调用点**都走 `rustlua-gpu` 的 `Renderer::new_offscreen`**（`crates/rustlua-gpu/src/renderer/construct.rs:62`，
复用调用方传入的 eframe device/queue）⇒ **不是两个 device、不是两套渲染器**。剩下重复的是
**宿主侧装配前导**，两段各 ≈60–90 行、共 ≈150 行、含 5 条必须同步的行为约定：
`new_offscreen` → shader 覆盖目录接入（"Scene View 与 Play 读同一份源"）→ `apply_scene_environment`
（"环境只在会话创建时应用一次"）→ `set_particle_compute_available` → `device.create_texture`
（两张描述符只差 label：`rustlua-scene-view` vs `rustlua-editor-preview`）→ 原生 egui 纹理 ID 注册
与"纹理换尺寸必须释放旧 ID + 作废渲染缓存"（`panels/scene_view/render.rs:8-24` 的失效注释就是这条的坑）。

### 1.6 评审清单里**已经在 crate 外**的三项

| crate | 行数 | 证据 |
|---|---|---|
| `rustlua-editor-ai` | 6,415（provider 1,325 / surface 672 / vision_provider 639 / import_tools 581 / image_provider 581 / bridge 569 / ui_provider 543 / ui_tools 436 / lua_check 350 / replay 345 / mcp 316） | **不依赖 editor**（manifest 仅 `rustlua-ui-spec` + serde）；editor 用 optional + `ai-bridge` feature 接它，且被 `verify.ps1:615` 的 `cargo tree --no-default-features` 门钉住。editor 侧只剩 `ai_host.rs` 1,875 行 `impl ToolHost`（`:873`）与 `AiBridge` 字段（`main.rs:919`） |
| `rustlua-editor-api` | 1,776（单 lib） | 只依赖 `egui`，是 `PluginRegistry` 的宿主契约叶子；`main.rs:62`、`plugins.rs:8` 消费 ⇒ 评审的"plugins 与主编辑器耦合"也已在 crate 外 |
| `rustlua-visual-script` | 1,854 | 可视化脚本领域层在 core 侧；editor 只剩 `graph_ops.rs` 664 + `panels/graph.rs` |

## 2. 对评审五分法的逐条裁定

| 建议 crate | 裁定 | 依据 | 去处 |
|---|---|---|---|
| `editor-core` | ❌ **不建 crate** | 上游编译判据已过；本日净上限 3.8%（§0）。**但**"纯数据层与 UI 层之间没有边界"这条是真的，用可见性 + 门来收 | WP-B（分层可见性 + 门），免费清单 = §0 第一组 3,266 行 |
| `editor-ui-studio` | ⚠️ **暂不建，条件已写明** | 3,635 行、零 egui、零 `EditorApp`、出向只认 `ui_spec_host`——它事实上已是独立层。不建 crate 的理由**不是**编译（上游已判），而是 `docs/75:28` 的"UI Studio 是 editor 的 dock 面板"与 `M76-06:118` 已预期的目录化：先目录化 + 收紧 `pub` 面，冲突面收益当场拿到，crate 化留着当触发条件成立时的低成本一步（目录边界 = 未来的 crate 边界） | WP-B2；触发条件见 §5 |
| `editor-preview` | ❌ **不建** | `preview_session` 36 处 `wgpu::` + 25 处 egui，与 egui 纹理绑定同生（§1.4）；搬进 lib 只是把 GPU 接缝换个地方放 | WP-A（合并 offscreen 装配前导，落点在 `rustlua-gpu`） |
| `editor-history` | ❌ **不建** | `history`(548)/`patch_history`(528)/`recovery`(399) 共 1,475 行，`CommandKind` 扇入 16、`SceneDocument` 扇入 35、三者互引 ⇒ 单切必成环。评审的"历史快照成本"实测指向 `recovery.rs:110 save_document` / `:123 autosave` 的**整份文档序列化**，是实现问题不是边界问题 | WP-F（快照成本度量，允许判"不是问题"） |
| `editor-ai-panel` | ✅ **已存在**（`rustlua-editor-ai`） | §1.6。评审的"AI surface 与文档/Undo 耦合"发生在 `ai_host.rs` 对 `EditorApp` 的直呼（1,875 行、`impl ToolHost` 借 `&mut EditorApp`），**再建 crate 治不了** | WP-C（接缝契约化 + 直呼面清单） |

## 3. 七条风险 → 真实载体 → 手段

| 评审点名的风险 | 实测载体 | 不用 crate 边界的手段 | 包 |
|---|---|---|---|
| 编译时间 | 19s 单文件周期，其中 ~13s codegen+link（上游实测，本日未复跑） | **接受现状**；唯一真能降的是 link 侧（`[profile.dev] debug`、分拆 bin 不在本包）；先取 M5 读数再谈 | 度量 M5 |
| 变更冲突 | `EditorApp` 199 字段 + 66 文件 296 次直呼；`dock.rs` 1,381 与 `panels/mod.rs` 是多面板共改面 | UI Studio 目录化 + 面板注册表化（新面板不再改 main 的类型）；冲突面按文件而不是按 crate 管理 | WP-B2、WP-D |
| 测试隔离 | 741 个 `#[test]` 全在同 crate 内联；改任一行要重编全部测试；GPU/egui 依赖的测试与纯数据测试混在同一批 | 先量 M3（构建时长 + 可分离条数），再按 `--` 过滤/feature 档把"无 GPU 子集"做成一条快门；**这条与 crate 无关** | 度量 M3、WP-E |
| 内存占用 | `gui_evidence.rs:648/860/1024` 已在逐秒采 `working_set_mib`，但**没有基线读数** | 取一份"启动→开项目→Play→Stop"曲线并归因（预判：主因是场景资源与字体图集，不是 crate 数——要证明或推翻） | 度量 M4 |
| 历史快照成本 | `recovery.rs:110/123` 整档序列化 | 量一次"编辑一次 → autosave 字节/毫秒"；若成本成立，做增量快照（数据问题） | WP-F |
| UI Studio 与主编辑器耦合 | 出向只有 `ui_spec_host` 四个符号；入向 7 文件 | 已经几乎为零——目录化 + `pub` 面白名单把它钉死，防止 M76 后续把它改回直呼 shell | WP-B2 |
| AI surface 与文档/Undo 耦合 | `ai_host.rs:873 impl ToolHost for EditorAiHost<'_>`（1,875 行借 `&mut EditorApp`） | 把直呼的字段/方法收成显式 trait（`ToolHost` 已在 `rustlua-editor-ai::bridge:95`，缺 editor 侧实现不再摊开 199 字段）+ 确认网关与 Undo 的接缝保持单向 | WP-C |

## 4. 可执行工作包（都不依赖"翻案 crate 拆分"）

### WP-A：offscreen 装配前导合并进 `rustlua-gpu`（3–5 天，**唯一被上游记为"仍要做"的 GPU 项**）

1. `crates/rustlua-gpu/src/renderer/` 增一个 offscreen target 单元：`new_offscreen` + shader 覆盖接入
   + 环境应用 + compute 能力上报 + 按尺寸重建纹理（label 由调用方给）+ 纹理失效回调；
   editor 两处调用点（`preview_session.rs`、`panels/scene_view/render.rs`）改直连，**不留兼容 shim**。
2. **上游已记的前置仍然成立**：`editor/src/preview_session.rs`、`panels/scene_view/mod.rs`、
   `editor/Cargo.toml` 是 M76 的在途脏文件（M77-00 §M77-02 裁定段），本包第 1 步的 editor 侧改动
   必须排在 M76-02/04 合入之后；**但第 3 步不碰 editor，可即刻做**。
3. **可即刻做的子集**（不碰 editor）：`engine/crates/engine/examples/` 里 22 个自建设备的 `wp0_*`
   例子搬进同一公共路径，并摘掉 `engine` 的 `wgpu`/`pollster`/`bytemuck` dev 依赖
   （`docs/62:279` 明写"随 M77-02 的 offscreen 合并收掉"）。
   **→ 2026-10-24 本日已交付（engine 侧），两处与计划的偏差按实测写明**：
   ①落点是 `crates/rustlua-gpu/src/offscreen.rs`（crate 根的 `#[cfg(not(wasm32))]` 模块），
   不是计划写的 `src/renderer/` 子目录——offscreen 装的是"设备与纹理的获取"，与 renderer 的
   pass 编排无共享私有项，放进 `renderer/` 只会让那棵树多一个 `pub` 出口；对外符号面门因此
   记到 `rustlua_gpu::offscreen::*` 四条新增（1237→1241，已 `--update` 重刷）。
   ②**dev 依赖摘不掉**：22 个例子里 9 个整份不再摸 `wgpu`，13 个仍引用这个名字——11 个只剩
   `texture: &wgpu::Texture` / `view: &wgpu::TextureView` 形参，2 个是真用法
   （`wp0_compute_readback` 的 STORAGE buffer、`wp0_gpu_particles_50k` 的 `device.poll`）；
   `pollster` 的例子侧使用者已归零，唯一使用者是在途未入库的 `examples/scene_shot.rs`。
   三条都是 dev 依赖，不进下游编译面，故留原位、不影响任何门读数；`engine/crates/engine/Cargo.toml`
   [dev-dependencies] 段的说明已按这个实测改写，`docs/62:279` 那句"随 offscreen 合并收掉"已同批
   降级为"合并已做、dev 依赖摘除按实测不成立"，同一行还补了"合并后 `rustlua_gpu` 符号面 1237→1241"。**逐例读数、字段级 A/B 与验收门读数全部在
   [M77-00 §M77-02](M77-00-宿主侧模块化与渲染器下沉-总览与依赖顺序.md) 的交付段**，本处不重复记数。
4. 验收：`wp0-editor-preview-golden`（`verify.ps1:1899`、判据 `:2371-2391` 的 7 项含
   `history.undo_restores_original`）逐字节对账；`editor --gui-smoke` 与 `--perf-baseline` 输出判据
   字符串不变；GPU 组测试一律 `--test-threads=1` 复核（并发假红见项目记忆）。
   **→ 第 3 步不涉及 editor 产物，所以第 4 步这条 editor 金标门仍属第 1–2 步的验收，未跑。**

### WP-B：分层可见性 + 门（不建 crate，2–3 天）

- 目标形状：在**单 crate 内**把"core 候选池"与 UI 层的关系写成可判的约束，替代 crate 边界。
- 手段：§0"零改造可搬"的 3,266 行 + 只差 1–3 处 egui 的 2,664 行，其 `pub` 面在 `lib.rs` 集中再导出
  （现状已是这个形态），本包做两件事：① 给这批模块补一条**方向门**——`panels/*` 与 `main.rs` 可
  依赖它们，反之**不许**（现无任何机制保证：`history.rs` 有 5 处 `EditorApp`、`import.rs` 8 处，
  即反向依赖已经在长）；② 把 WP-B 的"反向边"计数登记成只减不增的棘轮，照
  `panels/architecture_tests.rs` 的 `PINNED_WRITES` 同一套方法（`:52` 表 + `:101` 按
  `env!("CARGO_MANIFEST_DIR")/src/…` 定位根，不硬编码仓库路径）。
- **反面对照（必跑）**：在 `selection.rs` 里加一行 `crate::EditorApp` 引用 → 新门必须红。

### WP-B2：UI Studio 目录化 + `pub` 面白名单（3 天，排在 M76-02/04 之后）

- `ui_studio.rs`(1,600) → `ui_studio/`、与 `M76-06:118` 已预期的 `ui_studio/proposal.rs` 同批，避免两次搬；
  `ui_spec_host.rs`(2,035) 同步目录化。
- 出向白名单钉死：当前只有 `ui_spec_host` 四个符号（`ui_studio.rs:7`）；门断言"出向命中数 = 现状"，
  任何新增对 `crate::EditorApp` / egui 的引用当场红 ⇒ 这是评审"UI Studio 与主编辑器耦合"的**直接门**，
  比建 crate 更便宜且现在就有效。
- crate 化留着：见 §5 触发条件（目录边界即未来的 crate 边界，届时搬运量 = 3,635 行、接缝只有 4 个符号）。

### WP-C：AI 接缝契约化（3–5 天）

- 先出**直呼面清单**：`ai_host.rs` 对 `EditorApp` 的字段/方法读取逐个列出（1,875 行、`impl ToolHost`
  在 `:873`），标注哪些是文档读写、哪些是 Undo/历史、哪些是 UI 状态；
- 只把"文档读写 + Undo"两类收进显式 trait，其余留在 `EditorAiHost`；`rustlua-editor-ai::bridge::ToolHost`
  （`:95`）已是外部契约，**不新建 crate、不改 tool 面**（`verify.ps1:610` 的 `tool_surface_matches_baseline`
  快照门与 34 tool 面必须零 diff）。
- 验收：`cargo test -p rustlua-editor-ai --lib surface::tests` 零 diff；`verify.ps1:728` 的
  `--bin editor --all-features ai_host::tests::replay_fixture_is_deterministic_across_fresh_sessions`
  仍然绿（这条依赖隐式 bin 名 `editor`，见 §6）。

### WP-D：`EditorApp` 字段分组度量（2 天，只读数不改造）

- 199 字段 × "读写它的文件数"分组表 ⇒ 这是评审"变更冲突"的对症读数，也是**下一次真要动 shell
  时唯一的排期依据**。交付：分组表 + 前三大热点 + 判"要不要拆 shell 状态"的书面结论（允许判不做）。

### WP-E：架构门加固（1 天，与本包是否拆分**无关**的独立缺陷）

- `crates/rustlua-infra/tests/architecture_edges.rs:307` 的循环是 `for (crate_name, allowed) in BUDGET`
  ⇒ **只遍历表里已有的行**，且没有"每个 `crates/*` 目录必须有 BUDGET 行"的完整性断言 ⇒
  任何漏登记的 crate **静默逃出耦合预算门**（现状 `wasm-lua-spike` 就在表外）。
- 交付：完整性断言（组合根按 `:178` 的 `COMPOSITION_ROOTS` 名单豁免，豁免理由写进注释）。
- **反面对照（必跑）**：临时删掉一个已有 BUDGET 行 → 门必须红（证明它真的在看目录而不是只看表）。

### WP-F：recovery 快照成本（2 天，允许判"不是问题"）

- 量：一次编辑触发 `recovery.rs:110 save_document` / `:123 autosave` 的序列化字节与毫秒，对
  `examples/game_jhyt` 的真实文档取一份；与"增量快照"的理论收益对比。
- 判"不是问题"要写读数（例如 autosave 周期 vs 单次耗时的比值），不许留空。

## 5. 触发条件（满足任一条才重开 crate 级动作）

上游三条**照单继承**，本日再加两条：

1. `editor` 出现**第二个消费者**（headless 编辑器 / CLI 复用 document+history+command_router）
   —— 判据从"编译时长"换成"复用面"，15.3%/3.8% 这个上限不再适用；
2. `cargo build -p editor` 单文件重编周期涨到 **≥60s**（现 19s，link 约 13s）；
3. `panels/` 之外出现 ≥15,000 行、且被 UI 层与测试 harness 共同依赖的新层；
4. **（本包新增）UI Studio 出现 editor 之外的第二个消费者**（MCP/CLI 或 Web 端 UI 编辑要直接驱动
   `ui_studio` + `ui_spec_host`）——此时建 `editor-ui-studio`，搬运量 3,635 行、出向接缝只有 4 个符号，
   是全部候选里唯一"边界已经画好、只差 Cargo.toml"的一条；
5. **（本包新增）M3 读数证明测试隔离的收益与 crate 无关**（例如"无 GPU 子集"用过滤即可分离），
   则本包的 WP-B/E 就是终态，不再讨论拆分；反之若 M3 显示分离必须靠边界，才回到 §0 重算上限。

**翻案实验**（上游已设计，本包沿用，不要再用最小桩）：把 `document`+`history`+`patch_history`+
`command_router` 真实搬进 `editor-core` lib，在**仓库外独立 `CARGO_TARGET_DIR`** 测
`touch panels/inspector.rs; cargo build -p editor` 前后差；只有 ≥20% 才继续。
**本包的补充约束**：那组里 `command_router.rs` 有 9 处 `EditorApp`、`history.rs` 5 处
⇒ 实验前必须先做 WP-C/WP-D 的接缝改造，否则搬出去的是耦合而不是层，测出来的差值不可解释。

## 6. 门的连带面（真要 crate 化时的必改清单；本日全部已核）

| 类别 | 位置 | 备注 |
|---|---|---|
| 包名硬编码 | `verify.ps1:615` `628` `728` `817` `835` `862` `894` `924` `1899` | `main.rs`/`tests/`/`examples/` 必须留 `editor`；`:728` 用 `--bin editor`，**依赖无 `[[bin]]` 声明的自动发现** |
| 发布/CI | `release.yml:91`、`quality.yml:279`、`package-engine.ps1:59,67,139` | 新 crate 自动被 `--workspace` 覆盖；bin 名不变即无需改 |
| 产物体积棘轮 | `measure-artifacts.mjs:72`、`metrics/policy.json:46` | `editor.exe` 容差 +3%/256KiB |
| 覆盖率棘轮 | `metrics/baselines/coverage.json:26`（`name: "editor"`，计数行 43,225） | 行数骤降会**报红**；新 crate 无基线行则**不进棘轮**（覆盖率盲区） |
| cfg 站点基线 | `metrics/inventory/cfg-sites.json`（104 条 `editor\|crates/editor/src/…`）、`cfg-inventory.mjs:12` | 搬文件必判漂移 → `--check` 同批重生 |
| 预算静默绕过 | `architecture_edges.rs:307` + `:178` | 见 WP-E（本包唯一"不加固就等于没门"的项） |
| 面板棘轮 | `panels/architecture_tests.rs:52/101/222` | 搬面板必须同批改扫描根，否则覆盖面静默缩小 |
| 仓库根推导 | `tests/ui_asset_gate.rs:15-19`（`ancestors().nth(3)`） | 绑死 `engine/crates/<crate>` 三级布局，深度一变即 panic |
| 字体资产 | `crates/editor/fonts/`（`examples/game_api/README.md:218,226`、`examples/game_jhyt/docs/35:119,125` 带 SHA-256） | core 不需要字体，留 `editor` |
| 度量工具 | `scripts/metrics/measure-build.ps1:124-126`（增量目标硬编码 `-p game`） | 加 `-Target`（M5 的前置） |
| 文档行锚 | `docs/71:130,136,183-184`、`docs/56:16-27,420`、`docs/75:28`、`docs/62:279-280`、`M76-04:120`、`M76-06:74,118`、`M77-00` 本段 | `check-docs.ps1` **不校验 `rs:NN`** ⇒ 无门可抓，只能人工同步 |

## 7. 排期与验收

**顺序**：WP-E（1 天，独立，随时可做）→ WP-A 第 3 步（22 个 wp0 例子，不碰 editor）→
WP-B（2–3 天）→ 度量 M3/M4/M5（3 天，可与 WP-B 并行）→ WP-B2/WP-C（排 M76-02/04 之后）→
WP-D/WP-F（各 2 天，允许判不做）。批次总量估 **2–3 人周**（原 crate 拆分方案是 4–7 人周的一半）。

**共同验收**：
- 每个 WP 一个提交集；依赖变动同批带 `engine/Cargo.lock`（项目记忆：删 dev-dep 会让 cargo 重写锁）。
- 新门**必跑一次反面对照**并在证据里写"它红了"的读数（WP-B、WP-E 各一条，已写明形状）。
- 741 条 `#[test]` 为测试分母，**任何包不得让总数减少**；`tool_surface_matches_baseline` 与
  `--check`/`--gui-smoke`/`--perf-baseline`/`editor_preview_golden` 四条 CLI 判据输出逐字不变。
- 判"不做"的工作包交付书面结论（读数 + 名单 + 触发条件），照 M74-02 / M77-02 上游裁定的先例。
- `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`
  （**不带 `--keep-going` 只报第一个失败目标**）、`scripts/verify/check-docs.ps1`。

## 影响文件（预估）

- `engine/crates/rustlua-gpu/src/renderer/**`（WP-A）、`engine/crates/engine/examples/wp0_*.rs` +
  `engine/crates/engine/Cargo.toml`（WP-A 第 3 步）
- `engine/crates/editor/src/**`（WP-B 的门与可见性、WP-B2 的 `ui_studio/`+`ui_spec_host/` 目录化、
  WP-C 的 `ai_host.rs`）、`engine/crates/editor/src/lib.rs`（再导出面）
- `engine/crates/rustlua-infra/tests/architecture_edges.rs`（WP-E 完整性断言）
- `engine/crates/editor/src/panels/architecture_tests.rs`（WP-B 棘轮登记）
- `scripts/metrics/measure-build.ps1`（`-Target`）、`metrics/baselines/*`、`metrics/inventory/cfg-sites.json`
- `docs/62` §6 残项行、`docs/66` §架构治理、`docs/56`/`docs/71`/`docs/75` 的行锚、`tasks/README.md`、本目录

## 风险

- **M76 在途（最大）**：`preview_session.rs`、`panels/scene_view/mod.rs`、`editor/Cargo.toml`、
  `ui_studio*` 都是 M76 的脏文件（上游裁定段已记），M77-01 的提交集也被同一批 hunk 钉住
  （`M77-00:468` 记了 6 处）。开工前看 mtime，按部分提交纪律只按路径带走自己的内容。
- **本日并行会话正在改 editor**：两次行数读数差 5 行 ⇒ 认领时先复测，别引用本文件的具体行数当合同。
- **共享 `target/` 污染**：翻案实验必须在仓库外独立 `CARGO_TARGET_DIR`（上游已写明，项目记忆里有
  worktree 污染导致 `editor.exe` 中文方框与 `cargo test -p editor` 成批假红的先例）。
- **`--bin editor` 隐式依赖**：`verify.ps1:728` 靠自动发现的 bin 名；任何把 `main.rs` 挪走或显式声明
  `[[bin]]` 的动作都要同批改这条门。
- **WP-B 的门可能被"先解耦再搬"的乐观假设架空**：反向边（`history` 5 处 / `import` 8 处 `EditorApp`）
  若不登记基线，棘轮会把现状当"允许"。必须先出读数表再立门。

## 附录：读数复现命令

```bash
# 体量与构成
cd engine/crates/editor && bash -c 'find src -name "*.rs" | xargs wc -l | tail -1'
bash -c 'cd engine/crates/editor/src && for d in */; do echo "$(find "$d" -name "*.rs" | xargs cat|wc -l) $d"; done | sort -rn; \
  for f in *.rs; do echo "$(wc -l < $f) $f"; done | sort -rn'
grep -rn "#\[test\]" engine/crates/editor/src --include=*.rs | wc -l   # 740（src）+ tests/ui_asset_gate.rs 1

# §0 的耦合逐文件实测（本包新增，上游按行数算、这里按耦合类型算）
cd engine/crates/editor/src && for f in document document_space history patch_history command_router \
  schema_fields schema_audit selection geometry layout import resource_refresh resource_ops file_watcher \
  shader_watch publish play_sync runtime_sync play_control preview_session; do \
  echo "$f lines=$(wc -l < $f.rs) egui=$(grep -c 'egui\|eframe' $f.rs) EditorApp=$(grep -c EditorApp $f.rs) wgpu=$(grep -c 'wgpu::' $f.rs)"; done

# 扇入（必须按符号名；lib.rs 平铺再导出会让 crate::module:: 形态漏数）
for s in SceneDocument CommandKind SelectionModel PlayController RecoveryManager UiStudio EditorLayout; do
  echo "$s <- $(grep -rl "\b${s}\b" engine/crates/editor/src --include=*.rs | grep -v tests | wc -l) files"; done
grep -rl "ui_studio::\|ui_spec_host::" engine/crates/editor/src --include=*.rs | grep -v tests

# 依赖方向与 offscreen 单实现证据
cargo tree -p editor -e normal | head -40
cargo tree -p editor --no-default-features -e normal | grep -c rustlua-editor-ai   # 必须 0
grep -rn '^editor' engine/crates/*/Cargo.toml                                      # 必须 0 命中（无人依赖 editor）
grep -rn "Renderer::new_offscreen" engine/crates/editor/src engine/crates/rustlua-gpu/src/renderer/construct.rs

# 编译度量现状（为什么上游不用 compile-time.json 当分母）
grep -n "duration_ms\|incremental_crate\|target_dir\|check_cold" metrics/baselines/compile-time.json | head
sed -n '119,130p' scripts/metrics/measure-build.ps1
# 复跑上游 A/B 的正确姿势（独立 target 盘，勿污染共享 target）：
#   $env:CARGO_TARGET_DIR = "D:/tmp/m77-02-target"; cargo build -p editor   # 预热后
#   (Get-Item engine/crates/editor/src/panels/inspector.rs).LastWriteTime = Get-Date; cargo build -p editor

# 预算门遍历形状（WP-E）
sed -n '303,330p' engine/crates/rustlua-infra/tests/architecture_edges.rs
sed -n '176,180p' engine/crates/rustlua-infra/tests/architecture_edges.rs   # COMPOSITION_ROOTS
```

## 实现记录

- 2026-10-24：起详版。**不重开**上游的"crate 级拆分不做"裁定，而是给它加一层名单级耦合复核
  （15.3% 毛上限 → 12.6% 扣掉 GPU/egui 同生的 2,346 行 → 3.8% 零改造净上限），并把评审七条风险
  逐条对到真实载体与非 crate 手段（WP-A..F）。同日实测口径全部登记在附录命令里；
  测试行占比（≈33%）与 `EditorApp` 字段数（≈199，由闭合区间推得）**标注为估算**，认领时脚本化复核。

## 验收证据

- 待回填：WP-E 与 WP-B 的门读数（各含一次反面对照）、M3/M4/M5 读数、WP-A 第 3 步的
  `wp0-editor-preview-golden` 对账、WP-D/WP-F 的书面结论。

## 未完成项 / 已知限制

- **上游的 19s/23s/6s/33s 四条编译读数本日未复跑**（需在仓库外独立 `CARGO_TARGET_DIR`，
  且共享 target 会被 worktree 冷编污染）；本包只在 §0 做**推理与可达性复核**，不复测秒数。
  ⇒ 若那组秒数本身失真，§0 的"3.8% × 19s ≈ 0.7s"要按 M5 重采后再算。
- M3（测试隔离）与 M4（内存）两条判据**未跑** ⇒ 评审原话里的这两项目前是"方案 + 无读数"，
  不得写成已结论。
- `editor` 拆分对 **Web/微信三宿主**的影响：本包只动编辑器与 GPU 装配，但 WP-A 若改到
  `render_into_texture` 路径需补一次 `-Web` 门与包体积读数并归因；未跑的一端写"未测"。
- 评审 12.1 的"变更冲突"若最终落在 `dock.rs`(1,381) 与 `panels/mod.rs` 的共改上，那是**负责人/排期**
  问题（crate 与目录都帮不上），本包只交 WP-D 的分组读数，不承诺解掉。
