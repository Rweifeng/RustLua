# 任务：M3 UI Studio 工作区与 Copilot 提案箱

- 任务 ID：M76-04
- 状态：完成（headless 状态层、dock 标签、面板、`.ui.json` 双击入口、接收/撤销/重做、
  拆分接受、令牌编辑、双通道对拍、层级拖拽/复制/删除/包裹、检查器写回、画布移动/缩放手柄、
  真窗口 GUI 证据均已落地；剩余限制见文末）
- 关联里程碑：M76（方案 M3）
- 创建日期：2026-10-24

## 目标

在编辑器内交付 UI Studio 工作区：画布 + 语义层级 + 五段检查器 + 令牌/样式面板 + 资产面板 + 提案箱 +
多分辨率预览。Copilot 作为提案来源之一，与外部 MCP 客户端共用同一 registry。

## 影响文件

- `engine/crates/editor/src/dock.rs`（新增 `UiStudio` 标签与默认布局）
- `engine/crates/editor/src/ui_studio/**`（新增：画布叠层、层级、检查器、样式、资产、提案箱）
- `engine/crates/editor/src/preview_session.rs`（多分辨率/安全区/输入注入）
- `engine/crates/editor/src/ai_panel.rs`（提案展示与接受入口）
- `engine/crates/editor/src/history.rs`（检查点标记）
- `engine/tests/gui_evidence/**` 或现有证据 harness（截图清单）

## 交付项

1. **工作区**：`UiStudio` dock；`.ui.json` 双击进入；与现有 Scene/Game 标签互不破坏首帧几何。
2. **画布**：锚点参考线、安全区、间距标尺、九宫格手柄；选中同步层级与检查器。
3. **层级**：语义节点树（容器/控件），拖动改父级/排序，删除/复制/包裹容器。
4. **检查器**：布局/样式/状态/事件/绑定五段；令牌与类用下拉；只读字段灰显。
5. **样式面板**：令牌增删改、类列表、状态覆盖矩阵；改动实时预览。
6. **提案箱**：diff（结构/样式/资产）、预览图、诊断、成本、接受/拒绝/拆分接受；过期提案禁用。
7. **预览**：1280×720 / 1920×1080 / 375×667 三档 + 安全区开关 + 输入事件注入。
8. **Copilot 主入口**：面板内直接调内部 API（不经 MCP）；外部客户端只作为同一 registry 的旁路，
   两者产出的提案结构、诊断与 diff 一致（有对拍测试）。

## 验收方式

- `cargo build -p editor`；UI 几何门（`show_workspace` + Shape::Rect 中心差）覆盖新面板。
- GUI 证据：走 `gui_evidence` harness 截真窗口 PNG（不接受整屏 CopyFromScreen 替代）。
- 手工 golden path：新建 `.ui.json` → 放按钮 → 改令牌 → Copilot 提案 → 接受 → Undo 一次回到原状。

## 风险

- 与 egui_dock 布局升级/首帧几何耦合：新增标签必须默认不在树里或与已有叶共存，保持旧布局。
- 提案箱需要预览图生成链路（M76-03 的 `ui_preview_spec`），未就绪前显示解析后的线框。
- 不要维护第二份文档状态：UI Studio 的编辑必须走同一个 Spec 宿主与 History。

## 实现记录（2026-10-24）

### 已交付

- **headless 状态层**（`engine/crates/editor/src/ui_studio.rs`）：`UiStudio` 持有
  `UiSpecStore`/文档/revision/选中；语义层级（深度优先）、五段检查器（只读 key/type
  恒在）、三档分辨率（1280×720 / 1920×1080 / 375×667）与安全区（手机档 `[0,44,w,h-78]`）、
  离屏预览 PNG、诊断、提案列表；接受走 store，接受前快照进撤销栈（上限 16），
  `undo()` 原字节写回并把 revision 复原。
- **dock 标签**（`dock.rs`）：`DockTab::UiStudio` 默认不在树里；View 菜单打开时插进
  中央标签组（Game 之后），不改变旧布局首帧几何；`PANEL_UI_STUDIO = "ui_studio"`
  进 `panel_visible`，布局文件跨重启记住开关。
- **面板**（`engine/crates/editor/src/panels/ui_studio.rs`）：文档路径 + 打开/刷新/
  撤销接受；分辨率下拉 + 安全区勾选；左层级、右五段检查器；预览纹理按
  (revision, 分辨率) 失效；提案箱显示 kind/diff/诊断数并支持接受/拒绝；底部显示
  revision、诊断条数、error 数与撤销深度。面板不直接写文档（架构棘轮登记 0 写）。
- **`.ui.json` 双击入口**（`panels/resource_browser.rs`）：按双段后缀识别（不是只看
  `extension()`），加载文档、打开标签并在下一帧聚焦（标签可能本帧才进树）。
- **一次 Undo**：`UiSpecStore::snapshot/restore` + `UiStudio::accept/undo`；
  还原的是**接受前原始字节**，所以 revision 与接受前完全一致。
- **画布叠层**（`panels/ui_studio.rs` + `rustlua-ui-spec::layout_rects`）：预览图上叠加
  选中节点轮廓、锚点中心十字、九宫格四边内缩线与安全区框。布局复用预览器同一套
  `preview_fit`/`resolve_rect`，手机档信箱（375×667 → 210.94 高）也按同一 `fit/offset`
  贴图；`layout_rects` 同时给出滚动容器的裁剪矩形。
- **拆分接受 UI**（`ui_spec_host.rs` + `ui_studio.rs` + `panels/ui_studio.rs`）：
  `StoredProposal` 持久化原始 `UiPatch`（旧提案文件缺该字段按 `None` 读回），
  `UiStudio::proposal_ops` 给提案箱逐条勾选，`accept_subset` 走 `submit_patch_subset`
  再按普通提案接受（revision/能力/整份重校验与撤销路径完全一致）。面板默认全选，
  「接受选中 n/m」只落选中的 op；整份 Spec 提案没有 op，仍走普通「接受」。
- **令牌编辑走提案通道**（`patch.rs` + `validate.rs` + `ui_studio.rs` + `panels/ui_studio.rs`）：
  新增 `UiPatchOp::SetToken` / `RemoveToken`（值与 `tokens` 契约一致：标量或标量数组）。
  `UiStudio::set_token/remove_token` 只生成提案；面板提供名字/值输入、令牌列表与
  样式类状态覆盖矩阵（`state_matrix()`），接受/撤销沿用既有路径。`ProposalDiff` 增加
  `tokens_changed`，令牌-only 提案不再显示成空 diff；`diff_specs` 同步统计新增/删除/改写。
- **双通道一致性**（`editor/src/ai_host.rs` 测试 `ui_tool_results_match_between_internal_api_and_mcp`）：
  同一请求分别走内部 `execute_tool` 与 MCP `tools/call`（JSON-RPC 编解码后仍调用同一执行体），
  比对工具名单、返回体、诊断、提案 diff 与 T2 确认门。覆盖 6 条调用（export/validate/
  preview/apply_patch/apply_spec/list_assets）+ 2 条 T2 拒绝。

### 本轮增量（结构编辑 + 真窗口证据，2026-10-24）

- **补丁契约新增 `Move`**（`patch.rs` + `validate.rs`）：把节点从原父级摘下插到
  `parent` 的 `index`（`None`/`-1` 追加）。校验改成**按执行到该 op 时的状态**判断：
  `nodes` 快照 + `removed` 集合，因此「同一补丁先 `Remove` 再 `Insert` 同名 key」和
  「先插空容器再 `Move` 进去」都合法；同时仍拒绝改根、搬进自己子树、把叶子控件当父级、
  越界下标（`patch.move_root` / `patch.move_into_self` / `patch.parent_leaf` /
  `patch.parent_missing` / `patch.move_index`）。应用后只在 **error 级**诊断时回滚；
  「新容器暂时为空」这类 warning 不再误拒整批。
- **几何助手**（`rustlua-ui-spec::preview`）：`layout_rects` 的每项带 `parent`；
  新增 `ResizeHandle`（8 向 + `anchor()`）、`resize_rect`（只动该手柄的两条边，夹最小尺寸）、
  `offsets_for_rect`（与 `resolve_rect` 互逆）。画布拖动写回的是**相对父级**的
  offsets，并清掉与之互斥的 `size`/`position`（`spec.layout_conflict` 的正面对照）。
- **层级编辑走提案**（`ui_studio.rs`）：`remove_node` / `move_node` / `duplicate_node`
  （整棵子树复制，key 加 `_copy` 后缀）/ `wrap_node`（一次补丁内插容器 + 移入）。
  容器白名单显式列出（Panel/Container/ScrollView/List/Grid/Tabs/Modal/SafeArea）：
  `UiNodeKind::is_leaf()` 不含 Button，不能直接拿来当父级判断。
- **检查器写回**（`InspectorEdit` + `set_field`/`set_style`/`set_event`）：布局/样式/事件
  三段有输入框 + 「应用」，按段解析成对应补丁 op；状态/绑定仍只读（绑定行显式
  `read_only: true`）。`reload()` 现在保留选中，接受提案后画布不跳回根节点。
- **面板交互**（`panels/ui_studio.rs`）：层级行可拖拽，落点按目标行上/中/下带映射成
  「插到前面 / 成为子节点 / 插到后面」；层级操作行提供复制/删除/包裹；画布支持拖动
  移动与 8 个手柄缩放（松手才生成提案），叠层新增 100px 刻度尺、父级间隙标签与手柄方块。
  画布已移到层级/检查器**之前**——面板高度有限时先保证画布可见。
- **真窗口证据**（`gui_evidence.rs`）：新增 `ui_studio` 步骤
  `dpi1.5-1280x720-ui-studio`，采集前把演示文档写进工程 `.cache/gui_evidence/`
  （不碰 `ui/`），打开标签并聚焦；`summary.txt` / `manifest.json` 增加 `studio` 字段，
  单测断言步骤表里存在该步骤。

### 实测读数

| 命令 | 结果 |
| --- | --- |
| `cargo test -p editor --lib ui_studio` | 10 passed（接受→撤销→重做、拆分接受只落选中 op、令牌编辑、层级/检查器/画布全部经提案） |
| `cargo test -p editor --lib` | 244 passed / 0 failed（含 `schema_fields`：`ScrollView` 进通用编辑器名单） |
| `cargo test -p rustlua-ui-spec` | 57 passed（含 `Move` 正/反例、`resize_rect` 只动自己的边、`offsets_for_rect` 与 `resolve_rect` 互逆） |
| `cargo test -p editor --bin editor panels::ui_studio` | 4 passed（两帧 egui + 设计坐标→屏幕映射 + 手柄命中 + 层级落点分带） |
| `cargo test -p editor --bin editor gui_evidence::tests` | 7 passed（含「证据脚本必须含 UI Studio 步骤」） |
| `cargo test -p editor --bin editor ai_host::tests::ui_tool_results_match_between_internal_api_and_mcp` | 1 passed（内部 API ↔ MCP 对拍） |
| `cargo test -p editor --bin editor dock::tests` | 16 passed |
| `cargo test -p editor --bin editor menu::tests` | 10 passed |
| `cargo test -p editor --bin editor panels::architecture` | 2 passed（新面板登记 0 直接写） |
| `cargo test -p editor --bin editor panels::resource_browser` | 9 passed（含 `.ui.json` 双段后缀反例） |
| `cargo test -p editor --bin editor` | 486 passed / 1 failed（唯一红是 `scene_view::ui_source_layout` 基线：见下） |
| 真窗口证据 `RUSTLUA_EDITOR_GUI_EVIDENCE=artifacts/acceptance/m76-04/ui-studio cargo run -q -p editor -- --project ../examples/game_jhyt` | 13 张 PNG，`failed=0`；`13-dpi1.5-1280x720-ui-studio.png` 实测 ppi=1.5、1920×1080、`lit=1.000` |

**既有红（不是本批引入）**：`ui_source_scene_layout_matches_golden` 的 `index.scene`
（基线 nodes=34 / ui=34，实际 64 / 64）与 `api_prefab.scene`（基线 7，实际 8）对不上。
`git status` 显示两个 `.scene` 在工作区被改动（`index.scene` mtime 09:43，本批未碰），
`scene_view` 源码与基线文件均无 diff；实际值恰好等于改动后新增的 UI 实体数，
因此按「并行会话在途数据」处理，**不**刷基线（刷了会把别人未提交的改动烘成金标）。

反面对照（都写成断言）：未接受提案不进撤销栈；撤销后 revision 回到原值且事件消失；
空栈撤销返回 `false` 且不改文件；`shop.json` / `ui.json` 不算 `.ui.json`；
整份 Spec 提案没有 op（`proposal_ops` 为空，拆分接受报 `InvalidArgument`）；
只选第 0 条 op 时第 1 条的 `SetEvent` 不落盘；删除仍被样式引用的令牌 →
`spec.token_missing` 且文档字节不变；空令牌名 / 对象值 → `spec.token_name` / `spec.token_shape`；
删除不存在的令牌 → `patch.token_missing`（整批拒绝，新增令牌也不落盘）；
`Move` 改根节点 / 搬进自己子树 / 把叶子当父级 / 越界下标分别报上述错误码；
同一补丁先删后插同名 key 与「插容器再移入」通过；提交/接受前 revision 不变，
撤销后偏移回到原值；画布拖动写入的是父级坐标系 offsets（父级原点非 0 时也对）；
手柄命中顺序是四角 → 四边 → 画布本体；层级落点中线上下是兄弟、中间才是子级。

## 未完成项 / 已知限制

- **拖拽没有吸附与插入指示**：画布移动/缩放没有对齐辅助线、网格吸附或数值读出；
  层级拖拽靠离散带判断父子，拖动过程中没有插入线，松手才生成提案（拖拽本身不可撤销）。
- **标尺只画刻度**：每 100 设计像素一格、每 200 标数字，父级间隙标签是静态文本；
  没有测量工具或拖拽量距。
- **检查器只覆盖三段**：布局/样式/事件可编辑，且布局是**JSON 文本**输入而不是分字段表单；
  状态/绑定仍只读（绑定行显式标记 `read_only`）。
- **样式面板仍是"提案式编辑"**：令牌增删改与类/状态矩阵已接，但编辑不直接改画布——
  先提交提案，接受后预览才刷新（与"不维护第二份文档状态"一致）；矩阵本身只读展示。
- **拆分接受没有并排 diff**：每个 op 的具体字段变化只在悬停提示里，勾选框本身只显示
  序号 + 变体名；拆分靠「接受选中」重新生成子提案，原提案在 `.cache` 里保留。
- **缩放下限 8 设计像素**：`resize_rect` 只夹最小尺寸，不做等比缩放、多选或旋转；
  画布拖动一次只处理一个节点。
- **真窗口证据只覆盖静态渲染**：`13-dpi1.5-1280x720-ui-studio.png` 证明画布/层级/检查器
  同框且预览已生成，但**不包含交互回放**（拖拽、接受提案的像素证据仍靠单测）。
- **没有像素级几何门**：egui 几何门（`show_workspace` + `Shape::Rect` 中心差）只覆盖
  预览加载与面板进入，不校验画布叠层坐标；叠层坐标靠纯函数单测。
- **MCP 对拍范围**：只覆盖协议翻译与执行体一致性，不覆盖真实 stdio 线程与确认队列
  （那部分由 `rustlua-editor-ai` 的 bridge 单测覆盖）；Copilot 面板本身仍只经宿主
  内部调用，不走 MCP。
- **首帧布局**：UiStudio 默认隐藏，默认树未变（dock 16 项测试绿）；打开后与 Game 同叶，
  用户可再拖走。
