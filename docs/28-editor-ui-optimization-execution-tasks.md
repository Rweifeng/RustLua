# 编辑器 UI 优化执行条目

> 本文档依据 [27-editor-ui-optimization-plan.md](./27-editor-ui-optimization-plan.md) 拆分，用于指导实现、联调、测试和验收。

## 1. 执行说明

### 1.1 执行目标

在保留现有 `SceneDocument`、Prefab、`RuntimeScene` 和 `PlaySession` 的前提下，按“可靠性 → 编辑交互 → Widget Designer → 结构化数据 → 高级能力”的顺序推进，避免一次性重写编辑器或破坏已有场景格式。

### 1.2 执行原则

1. 所有文档变更必须经过统一变更入口。
2. 所有连续编辑必须通过显式编辑事务提交。
3. 编辑预览状态与已提交场景数据分离。
4. 编辑器和运行时共用同一套 UI 布局计算逻辑。
5. 新格式必须支持旧格式读取和渐进式迁移。
6. 每个阶段完成后都应保持已有场景、Prefab、Play/Stop 和 Undo/Redo 功能可用。

### 1.3 交付物约定

每个执行条目至少包含：

- 实现范围。
- 依赖项。
- 影响模块。
- 验收标准。
- 回归测试。

涉及数据格式、公共 API 或编辑器交互变化时，同时补充迁移说明和手工验证步骤。

## 2. 总体依赖关系

```text
E01 基线盘点
  ├── E02 文档变更入口
  │     ├── E03 SelectionModel
  │     ├── E04 Undo/Redo 状态同步
  │     └── E05 输入焦点与快捷键边界
  ├── E06 EditTransaction
  │     ├── E07 Scene View 拖拽事务化
  │     ├── E08 Inspector 编辑事务化
  │     └── E09 Hierarchy 重挂载事务化
  └── E10 未保存保护

E03 + E04 + E06
  └── E11 Scene View / Inspector 交互重构

E11
  └── E12 多选、对齐、分布、上下文菜单

E02 + E04 + E11
  └── E13 Play/Stop 编辑上下文恢复

E02 + E11
  └── E14 Canvas 根节点与 UiLayout
        ├── E15 Anchor + Offset
        ├── E16 Container 自动布局
        └── E17 多分辨率与 Safe Area 预览

E14 + E15 + E16 + E17
  └── E18 Widget Designer 与 Palette
        └── E19 Widget/Prefab 复用

E14 + E18
  └── E20 Component Schema 与兼容迁移
        ├── E21 结构化 UI 事件
        ├── E22 ViewModel / Property Binding
        └── E23 预览诊断

E20 + E22
  └── E24 性能优化与高级能力
```

## 3. 阶段 0：基线盘点与公共约束

### E01：建立编辑器状态和文档变更基线

**目标**：明确当前状态来源、文档修改入口、缓存刷新路径及现有测试缺口。

**执行内容**：

- 梳理场景切换、新建、Prefab 打开、项目切换和编辑器关闭的入口。
- 梳理 `SceneDocument`、运行时副本、Prefab 数据和编辑器会话状态的读写关系。
- 列出当前所有选择状态、Inspector 状态、Hierarchy 状态和 Scene View 状态。
- 记录现有 History 命令类型、保存触发点和自动保存行为。
- 为后续每个条目建立可复现的最小场景与手工测试数据。

**影响模块**：编辑器主循环、文档管理、History、Hierarchy、Scene View、Inspector、PlaySession。

**完成标准**：形成状态流转表、文档变更入口清单、现有问题复现清单；不修改运行行为。

## 4. 阶段 1：可靠性和状态同步

### E02：实现统一文档变更入口

**目标**：所有打开、编辑、Undo/Redo、Prefab 应用和运行时修改应用都经过统一刷新流程。

**执行内容**：

- 增加统一的 `on_document_changed(change)` 处理入口。
- 在入口中依次处理选择修复、Inspector 同步、Scene View 失效、Game Preview 失效和 Hierarchy 元数据刷新。
- 替换绕过统一入口的直接文档替换逻辑。
- 为删除对象、重命名对象、Prefab 应用和场景打开补充变更类型。

**依赖**：E01。

**重点文件**：[main.rs](../engine/crates/editor/src/main.rs)、[actions/mod.rs](../engine/crates/editor/src/actions/mod.rs)、[history.rs](../engine/crates/rustlua-editor-core/src/history.rs)。

**验收标准**：任意文档变更后，Hierarchy、Inspector、Scene View、运行时预览和布局缓存均显示同一份数据；不再出现 Inspector 指向已删除对象或画布显示旧数据。

**回归测试**：打开场景、删除选中节点、Prefab Apply、Undo、Redo、运行时修改应用。

### E03：引入统一 SelectionModel

**目标**：消除 Hierarchy、Scene View 和 Inspector 之间的多套选择状态。

**执行内容**：

- 增加主选中对象、多选集合和选择来源字段。
- 将 Hierarchy 点击、Scene View 点击、框选、加选、删除回退、Prefab 实例化、Undo/Redo 和 Play/Stop 接入统一模型。
- 增加 `SelectionChanged` 事件或等效通知机制。
- 统一更新 Hierarchy 高亮、Inspector 内容、Scene View 选择框和工具栏状态。
- 对对象删除、文档切换和对象 ID 失效增加选择引用清理。

**依赖**：E01、E02。

**重点文件**：[hierarchy.rs](../engine/crates/editor/src/panels/hierarchy.rs)、[scene_view/mod.rs](../engine/crates/editor/src/panels/scene_view/mod.rs)、[inspector.rs](../engine/crates/editor/src/panels/inspector.rs)。

**验收标准**：任何入口选择对象后，三个面板在同一帧内显示一致；多选集合不会因面板切换、Undo/Redo 或 Play/Stop 被无故清空。

**回归测试**：单选、多选、Ctrl/Command 加选、框选、删除后回退、Prefab 实例化和跨场景切换。

### E04：完善 Undo/Redo 后的状态同步

**目标**：Undo/Redo 不仅恢复场景数据，还恢复所有依赖状态。

**执行内容**：

- 在 Undo/Redo 完成后统一调用文档变更入口。
- 重新计算选择集合、Inspector 字段、Hierarchy 展开相关元数据和 Scene View 缓存。
- 清理指向已删除实体的引用。
- 对布局缓存和 RuntimeScene 缓存执行失效或增量刷新。
- 为批量属性修改、重挂载、Prefab 操作补充历史恢复测试。

**依赖**：E02、E03。

**验收标准**：连续执行 Undo/Redo 后，数据、选择、属性面板和画布完全一致；恢复已删除对象后可以继续编辑。

### E05：处理文本输入焦点和快捷键边界

**目标**：避免文本控件编辑期间误触发编辑器全局操作。

**执行内容**：

- 建立输入焦点分类：文本编辑、数字编辑、弹窗、Scene View、Hierarchy、普通面板。
- 文本控件获得焦点时，将 Ctrl+Z/Ctrl+Y、Delete、Escape 和 Ctrl+S 按控件优先级处理。
- 只有在无文本编辑焦点时，才执行删除对象、全局撤销和场景保存。
- 统一快捷键分发顺序并增加冲突检测。

**依赖**：E01、E03。

**验收标准**：编辑名称、脚本路径和 Text 内容时不会删除对象或改变场景选择；文本撤销与场景撤销互不串扰。

### E06：实现未保存文档切换保护

**目标**：所有可能离开当前文档的操作都提供一致的保存确认。

**执行内容**：

- 增加 `request_document_change()` 流程。
- 覆盖场景下拉切换、新建场景、打开 Prefab、打开其他文档、切换项目和关闭编辑器。
- 弹窗提供“保存并继续”“放弃并继续”“取消”。
- 保存失败时停留在当前文档并保留 Dirty 状态。
- 防止重复触发切换请求和重复弹窗。

**依赖**：E01、E02。

**重点文件**：[main.rs](../engine/crates/editor/src/main.rs)。

**验收标准**：Dirty 文档离开前 100% 弹出确认；选择取消后文档、选择和视图状态不变；保存成功后再执行切换。

### E07：实现 EditTransaction 基础设施

**目标**：将连续交互合并为一次可提交、可取消的历史操作。

**执行内容**：

- 增加事务 ID、标签、类型、目标对象、before 和 preview 状态。
- 提供 `begin_transaction`、`update_preview`、`commit_transaction`、`cancel_transaction`。
- 事务期间只更新预览，不写磁盘、不追加多条 History。
- 提交时生成一条带元数据的 EditCommand，并标记文档 Dirty。
- 取消时恢复事务开始前状态。
- 明确禁止嵌套事务，或定义父子事务边界。

**依赖**：E02、E04。

**验收标准**：一次连续操作最多产生一条 Undo；取消操作不产生 History、不标记 Dirty；提交后的命令可正确 Undo/Redo。

**回归测试**：拖拽、数值拖动、颜色编辑、Tilemap 连续绘制和批量属性修改。

**实现状态（E07，批次 A）**：

- ✅ `history.rs` 增加事务核心：`EditTransaction`（label/kind/target/before 快照）、
  `TransactionError`（`AlreadyActive`/`NotActive`）、`CommandHistory::begin_transaction /
  commit_transaction / cancel_transaction / finish_transaction / is_transaction_active`。
  禁止嵌套；提交按 `before == after` diff 检测空操作（不追加 History、不标记 Dirty）；
  `target` 注入 kind 以保留 Transform/Transform3D 与相邻同对象命令的合并语义。
- ✅ `EditTransactionState`（active/consumed）导出并在 `EditorApp` 持有；`EditorApp` 提供
  唯一网关：`begin_edit_transaction / commit_edit_transaction / cancel_edit_transaction /
  undo_with_transaction_guard / is_edit_transaction_active`。
- ✅ 顶层快捷键：Ctrl+Z 经 `undo_with_transaction_guard`（活动事务先取消再撤）；
  `save_scene()` 入口对活动事务兜底 `finish_transaction`（落盘前自动提交单条历史）。
- ✅ 单测：history 事务 7 项 + actions 层 E07 回归 4 项，全部通过
  （`cargo test -p editor`：lib 99、bin 81）。
- ⏳ E08 依赖说明：Scene View 拖拽/Inspector 数值拖动尚未调用 `begin_edit_transaction`
  （每帧 `commit_scene_drag`/`commit_gizmo_drag3d` 仍即时 execute + save），由 E08 接入。
  已知取舍：E07 阶段事务 before 取自当时的脏文档，一条事务 Undo 会与事务前最后一条
  离散编辑合并为一条边界；E08 以“新开独立文档快照隔离编辑”后边界即精确。

### E08：将 Scene View 和 Inspector 编辑接入事务

**目标**：消除拖拽和 Inspector 编辑过程中的频繁保存与历史污染。

**执行内容**：

- Scene View 移动、旋转、缩放、Anchor 拖动接入事务。
- Inspector 数字拖动、颜色选择器和文本提交接入事务。
- 鼠标释放、输入提交或焦点离开时提交。
- 拖动取消、Escape 或弹窗关闭时取消。
- 保存策略改为提交后异步自动保存，不在交互过程中写磁盘。

**依赖**：E07。

**重点文件**：[scene_view/mod.rs](../engine/crates/editor/src/panels/scene_view/mod.rs)、[inspector.rs](../engine/crates/editor/src/panels/inspector.rs)、[history.rs](../engine/crates/rustlua-editor-core/src/history.rs)。

**验收标准**：拖动过程中磁盘文件不持续变化；一次拖动对应一条历史记录；Inspector 输入提交后可恢复且不会产生空操作。

**实现状态（E08，批次 A）**：

- ✅ 统一提交边界模型：`EditorApp` 新增 `inspector_edit`（pending/consumed）与
  `inspector_baseline`（EditSnapshot 基线）。帧内编辑走 `mark_inspector_edit`
  （相对基线有真实差异才直写进活动事务并置 pending，不写历史、不写盘）。
- ✅ 两档提交点：`commit_inspector_edits_if_dirty`（要求 pending，Ctrl+S / Undo 前兜底）
  与 `finish_inspector_edits_if_dirty`（diff 驱动：无论 pending 是否置位都收敛；
  返回本边界是否提交了一条净差异历史）。提交后回读 `sync_inspector` + 刷新基线，
  无净差异自动 no-op（不产生空历史）。
- ✅ Inspector 面板接入：帧首不再 mark；帧末按“本帧 primary 在面板内释放 →
  `finish_inspector_edits_if_dirty`，否则 `mark_inspector_edit`”提交决策；顶层
  `flush_frame_edit_boundary` 兜底（面板外释放 / Enter）；Escape 经
  `cancel_edit_on_escape` 取消活动事务并 `clear_active_scene_drag` 清拖拽残留。
- ✅ Scene View 拖拽接入：拖动中多帧直写文档（首帧自动开 Transform 事务），松手
  `commit_scene_drag` / `commit_gizmo_drag3d` 提交为一条历史 + 落盘；保存入口
  `save_scene` 对活动事务兜底 finish。
- ✅ 空引用规范化：Sprite / mesh / Lua 引用清空时按 clear 语义归一化为 None，
  避免空串被 setter 拒绝导致整组直写失败。
- ✅ 行为回归：`e08_inspector_mark_then_finish_*`、`e08_scene_drag_*`、
  `e08_escape_cancel_*` 等新测试 + 既有回归全部通过（`cargo test -p editor`）。

> **批次 A 附带（超越条目范围的实现，登记备查）**：随 E09/E10 同批落地的部分
> 阶段 2/3 雏形——E12 多选 Inspector（共有组件提示 + Transform/Transform3D
> 批量写入 + Mixed 标注 + 锁定比例）、E13 组件注册表 `COMPONENT_REGISTRY`
> （Add Component 改为读取注册表）、E14 工具栏文字标签 + QWER 快捷键 +
> 辅助线/Safe Area/像素网格开关、E15/E19 雏形（Widget/Prefab 模式切换 +
> Widget Palette 拖拽生成 Text/Image/Button + `ui.root` 过滤）、E16 雏形
> （`UiLayout` 共享布局值对象，编辑器与运行时同一 `resolve` 路径）。这些
> 均为过渡性实现，对应条目正式封板时以其验收标准复核。

### E09：完善 Play/Stop 编辑上下文恢复

**目标**：Play/Stop 不丢失用户当前编辑上下文。

**执行内容**：

- Play 前保存选中对象、多选集合、Inspector 锁定、Hierarchy 展开、Scene View pan/zoom、当前工具、文档标签和预览分辨率。
- Stop 后按对象 ID 和文档有效性恢复上下文。
- 对运行期间被删除或失效的对象执行安全回退选择。
- 确保编辑文档与运行时副本继续隔离。

**依赖**：E03、E04。

**验收标准**：Play/Stop 前后视图、选择和工具状态保持；运行时对象不存在于编辑文档时不会产生悬空引用。

**实现状态（E09，批次 A）**：

- ✅ `EditorContextSnapshot`（main.rs）：Play 前捕获选择、工作区标签页、场景工具、
  Scene View 模式/缩放/平移、Hierarchy 折叠状态、预览分辨率、Game 相机中心；
  `play()` 时 `capture_editor_context`，`stop()` 时经 `restore_editor_context` 恢复。
- ✅ Stop 路径：`on_document_changed(PlaybackStopped)` 先清空/修复选择（沿用 E02
  网关），随后快照恢复覆盖回原选择/视图/工具；编辑文档与运行时副本继续隔离
  （`set_current_scene_document(session.stop())` 不变）。
- ✅ 快照缺失兜底：无快照（异常路径）时以当前视图参数构造等价快照，不产生悬空引用。
- ✅ 行为回归：`sample_project_stop_restores_editor_context_and_clears_preview_input`
  验证 Stop 后选择/工具/缩放/平移/标签页与 Play 前一致；全部测试通过。

### E10：修复 Hierarchy 过滤、重挂载和上下文操作

**目标**：提升层级树搜索、重排和常用操作效率。

**执行内容**：

- 过滤采用 `matches(node) || has_matching_descendant(node)`。
- 匹配后代时保留祖先路径并自动展开。
- 支持拖拽插入前、插入后和作为子节点三种目标提示。
- 对循环引用和非法目标显示红色提示并禁止提交。
- 重挂载使用单个 EditTransaction。
- 增加创建子节点、创建同级节点、重命名、复制、粘贴、删除、移动到根、Prefab 操作和 Scene View 聚焦菜单。
- 持久化展开状态到编辑器配置，不写入场景数据。

**依赖**：E03、E07。

**重点文件**：[hierarchy.rs](../engine/crates/editor/src/panels/hierarchy.rs)。

**验收标准**：可搜索任意深度节点并保留完整祖先路径；非法重挂载无法提交；上下文菜单操作可 Undo/Redo。

**实现状态（E10，批次 A）**：

- ✅ 过滤祖先路径：`hierarchy_filter_matches` 递归 `matches(node) ||
  has_matching_descendant(node)`；过滤激活时匹配后代的节点强制展开
  （`force_expand` 覆盖折叠态），祖先链完整可见。
- ✅ 三位重挂载：Hierarchy 行支持拖拽（`dnd_drag_source` + 内嵌 `dnd_drop_zone`），
  按指针落点 y 三分位判定 Before / After / Child；合法目标画绿色插入线/子级框，
  非法目标（自环、子孙、锁定）画红色提示且 `validate_reparent_target` 拒绝提交。
- ✅ `SceneTreeState::reparent_at(document, id, target, position)`：同层重排序、
  跨父移动、根层排序（根顺序持久化到场景 `metadata["editor.root_order"]`，
  保存/重开后顺序保持）；循环/非法目标返回 `SceneTreeError`。
- ✅ 重挂载事务化：拖放释放经 `begin_edit_transaction → reparent_at →
  commit_edit_transaction`（E07/E08 事务网关），失败自动 `cancel`。
- ✅ 上下文菜单（行尾 ⋮）：新建子节点/同级节点、重命名、复制/粘贴为子节点
  （`hierarchy_clipboard` + `duplicate_subtree` 全新 ID）、创建/重应用 Prefab、
  聚焦 Scene View（2D 平移居中 / 3D 相机 target）、设为 UI 根节点、移动到根、删除。
  全部走 History，可 Undo/Redo。
- ✅ UI Root 标记：实体 `metadata["ui.root"]="true"` 经 History 提交，Hierarchy 行
  与 Inspector 显示 `[UI Root]` 徽标；Widget Mode（E15 雏形）以此过滤 UI 子树。
- ✅ 单测：`reparent_at_updates_sibling_order`、`reparent_at_persists_root_order`、
  `duplicate_subtree_gets_fresh_ids_and_preserves_data`、
  `hierarchy_filter_keeps_matching_ancestor_path_visible` 等；全部测试通过。

## 5. 阶段 2：Scene View、Inspector 和日常编辑效率

### E11：拆分 Scene View 交互和渲染职责

**目标**：为 Scene Mode、Widget Mode 和 Prefab Mode 建立可扩展结构。

**执行内容**：

- 将输入采集、命中测试、选择、工具控制器、预览状态和历史提交分层。
- 按职责拆分 2D/3D 渲染、交互、选择、变换工具、Gizmo、Tilemap、UI 布局和拖放处理。
- 定义统一的 `InteractionEvent` 和工具状态流转。
- 保持现有 2D、3D、Tilemap 功能行为不变。

**依赖**：E03、E07、E08。

**验收标准**：绘制逻辑和交互逻辑可独立测试；Scene View 现有功能无回归；新增 Widget Mode 不需要修改普通 2D/3D 工具核心逻辑。

**实现状态（E11，批次 B 首步）**：

- ✅ 交互词汇表落地：新增 `scene_interaction.rs`——`InteractionEvent`
  （PointerDown/Drag/Up、Cancel、ToolSelected、TileStroke）、`ToolPhase`
  （Idle/Dragging/Marquee/Painting）、`ToolStateMachine` 纯状态机（工具切换
  取消进行中交互、重复 Down 忽略、Escape 取消），5 项单测脱离 egui/GPU 通过。
- ⏳ 待收口：scene_view.rs 命中测试/渲染/文档写入按此词汇表逐块迁移
  （150KB 文件不建议一次性重写；后续条目每接入一块删除一块旧路径）。

### E12：完善多选 Inspector 和统一 Transform 编辑器

**目标**：支持多对象批量修改和一致的 Transform 操作。

**执行内容**：

- 多选时显示对象数量和共有组件。
- 不同值显示 Mixed Value，共有字段修改批量写入。
- Transform 支持轴颜色、单位、Reset、复制粘贴、输入、拖拽、默认值恢复、表达式和字段级 Undo。
- 3D 旋转优先显示欧拉角，底层仍按现有数据格式转换。
- 增加锁定比例和不可编辑字段状态。

**依赖**：E03、E07、E08。

**重点文件**：[inspector.rs](../engine/crates/editor/src/panels/inspector.rs)。

**验收标准**：多选修改只影响共有字段且一次操作一条 History；Mixed Value 显示准确；Transform 修改不会造成坐标系或单位混乱。

### E13：建立注册式 Inspector 组件机制

**目标**：让新组件通过注册自动进入 Inspector 和 Add Component。

**执行内容**：

- 定义组件名称、分类、可添加/移除能力、绘制、读取、写入和验证接口。
- 为现有 Transform、Text、Image、Button 等组件建立描述器。
- 将 Add Component 改为读取注册表。
- 统一组件菜单的 Reset、Copy、Paste、Remove、Move Up、Move Down。
- 保留暂未迁移组件的兼容绘制路径，但不新增第二套长期接口。

**依赖**：E08、E12。

**验收标准**：新增测试组件只需注册描述器即可出现在 Add Component；字段写入、校验、Undo/Redo 和序列化入口一致。

**实现状态（E13，批次 B）**：

- ✅ 注册表升级：`ComponentDescriptor` 增加 `category`（玩法/渲染/3D/UI 四类），
  Add Component 下拉按类别分组展示。
- ✅ 新组件注册：`UiLayout`（E16 布局）与 `Container`（E17 容器）入册，添加时
  写入类型化默认值（中心点锚 200×100 / VBox 无内边距），经 History 可撤销。
- ⏳ 待收口：Reset/Copy/Paste/Move Up/Down 统一组件菜单与字段级校验回调
  （依赖 E12 收尾与 E20 结构化迁移一并复核）。

### E14：增加工具栏标签、快捷键和常用编辑辅助

**目标**：降低工具识别和高频操作成本。

**执行内容**：

- 工具栏增加 Select、Move、Rotate、Scale、Anchor、Layout、Snap、Preview Resolution、Safe Area、Guides 的文字标签。
- 建立快捷键注册和冲突提示。
- 增加边界框、Pivot、Anchor 点、拉伸手柄、旋转手柄、对齐线、间距提示、名称、锁定/隐藏轮廓、父子连接线和像素网格。
- 辅助线显示不写入场景数据。

**依赖**：E05、E11、E12。

**验收标准**：所有高频工具有可见标签和稳定快捷键；辅助信息可独立开关，不影响场景运行和序列化结果。

## 6. 阶段 3：Canvas、布局与 Widget Designer

### E15：引入 Canvas 根节点和 UI 编辑模式

**目标**：在现有 SceneEntity 基础上建立 UI 专属编辑语义。

**执行内容**：

- 增加 `UiRoot` 或等效 Canvas 根节点。
- 支持参考分辨率、缩放模式、屏幕适配、Safe Area、输入层级、渲染层级和相机影响配置。
- 增加 Scene Mode、Widget Mode、Prefab Mode 切换。
- Widget Mode 默认使用正交视图并显示设计分辨率。
- 明确 Canvas 配置保存位置和默认值。

**依赖**：E11、E13。

**验收标准**：UI 根节点可以独立识别和编辑；普通场景实体不受 Canvas 配置影响；编辑器与运行时读取同一套 Canvas 参数。

**实现状态（E15，批次 B 首步）**：

- ✅ Canvas 数据层：`CanvasConfig`（参考分辨率/缩放模式 none|fit_width|
  fit_height|fit_both/Safe Area insets）与 `scale(viewport)` 落地引擎核心，
  编辑器与运行时可共享同一配置对象；模式选择补齐了旧实现只有非均匀缩放的缺口。
- ⏳ 待收口：Canvas 配置保存到 UiRoot 实体 metadata 的编辑 UI（metadata 键
  约定 `canvas.reference` / `canvas.scale_mode` / `canvas.safe_area`，
  解析入口 `CanvasConfig::from_metadata` 已备好）。

### E16：实现 UiLayout 与 Anchor + Offset

**目标**：支持响应式 UI 的四边约束和统一布局编辑。

**执行内容**：

- 增加 anchors、offsets、min/max size、pivot、rotation、scale 等字段。
- Text 和 Image 仅保留视觉属性，布局统一由 UiLayout 管理。
- 实现 Top Left、Top Center、Top Right、Center、Bottom Left、Bottom Center、Bottom Right、Full Rect、Stretch Horizontal、Stretch Vertical 预设。
- Inspector 支持数值编辑、预设选择和四边手柄拖动。
- 实现编辑器与运行时共享的布局计算函数。

**依赖**：E15、E07。

**验收标准**：在不同父节点尺寸和预览分辨率下，Anchor + Offset 结果稳定；布局修改可 Undo/Redo；旧 Text/Image 位置数据可读取。

**实现状态（E16，批次 B）**：

- ✅ 引擎核心：`core::ui_layout::UiRectLayout`——四边 anchors/offsets（归一化
  0..1 + 像素偏移）、min/max size（0 = 不约束）、pivot（裁剪时保持枢轴点不动）、
  `UiLayoutPreset` 12 种预设（含要求的 10 种）、组件串解析/序列化与校验。
- ✅ 兼容旧数据：`UiRectLayout::from_point` 无损映射旧 anchor+position+size
  三元组（单测验证与旧 resolve 数学逐位一致）；旧 Text/Image 无 UiLayout 时
  行为完全不变。
- ✅ 运行时接入：场景加载解析 `UiLayout` 组件入 ECS；`push_scene_ui` 与
  `compute_click_targets` 均以布局矩形优先（视觉属性仍在 Text/Image），编辑器
  预览与运行时共享同一 resolve 路径（回归测试
  `ui_layout_component_overrides_inline_image_rect`）。
- ✅ Inspector 编辑组：预设下拉（自动匹配当前 anchors）、Anchor/Offset LTRB
  数值拖动、Min/Max、Pivot、移除；写入走 E08 事务管线（一次交互一条历史）。
- ⏳ 待收口：Scene View 四边手柄拖动与父级链解析（当前 parent = viewport，
  Canvas 容器父级解析随 E18/E19 深化）。

### E17：实现基础 Container 自动布局

**目标**：提供常见菜单、列表、网格和弹窗布局能力。

**执行内容**：

- 首批实现 VBox、HBox、Grid、Margin、Center、Panel、Scroll、AspectRatio。
- 支持 Minimum Size、Expand、Fill、Alignment、Padding、Stretch Ratio。
- 明确父级 Container 控制子节点位置时的只读或警告状态。
- 布局只重算受影响的子树。
- 为每种 Container 定义最小可运行示例和边界条件。

**依赖**：E16。

**验收标准**：至少 5 种 Container 可在编辑器创建、配置、预览和运行；受父级控制的 Position 不会无提示地产生无效编辑。

**实现状态（E17，批次 B）**：

- ✅ 引擎核心：`ContainerKind`（VBox/HBox/Grid/Margin/Center 首批 5 种）、
  `ContainerStyle`（padding/spacing/columns/stretch/fill/align）、
  `ChildLayout`（expand 权重 + fill）、`layout_container` 纯函数（相对容器
  原点返回子矩形；Grid 列宽/行高按子级测量并支持 stretch 均摊、fill 填满）。
- ✅ 组件串解析/序列化与校验（`kind=vbox;...`），Container 添加入册带默认值，
  Inspector 编辑组（类型下拉/内边距/间距/列数/拉伸/填满/对齐/移除）走 E08 管线。
- ✅ 边界条件单测：空容器、零尺寸容器、Grid stretch、HBox expand 权重分配。
- ⏳ 待收口：Scroll/AspectRatio 两类、受父级控制 Position 的只读警告 UI、
  运行时 `push_scene_ui` 的容器递归布局（纯函数已具备，接线路径随 E18）。

**P3-E05-1 嵌套两遍布局（2026-10-23）**：

- ✅ `resolve_scene_ui_layout` 改为「自底向上测量 + 自顶向下递归摆放」：容器子项先测固有尺寸
  （子容器 = 子项内容 + padding/spacing，覆盖 VBox/HBox/Grid/Margin/Center），父容器摆放后再把
  矩形递归传给子容器；测量结果在一次解算内按子树缓存。
- ✅ 容器摆放优先于子项自身 `UiLayout`，且与实体 id 顺序无关；父链成环时递归终止并保留
  `CircularParent` 诊断。
- ✅ 顺带修正无 `UiLayout` 子节点把父偏移加两次的问题（父矩形非零时坐标翻倍）。
- 验收：`cargo test -p rustlua-ui` **82/82**（新增三层嵌套 + id 乱序、无布局子节点、环终止）；
  engine lib **313/313**、editor **203 lib + 450 bin / 2 ignored**；clippy/fmt/docs 全绿。
- ⏳ E05-4（编辑器字段/overlay/预设）未开工；E05-2/E05-3 见下。
  本轮没有 3 分辨率截图 golden，也没有开窗 GUI 目视。

**P3-E05-2 wrap / justify / LayoutItem 约束（2026-10-23）**：

- ✅ `ContainerKind::Wrap`：按行贪心换行，行高取行内最高子项；`Justify` 六种主轴分布
  （Start/Center/End/SpaceBetween/SpaceAround/SpaceEvenly）在 VBox/HBox/Wrap/Grid 生效。
- ✅ `ChildLayout` 扩展：`grow`（不依赖 `stretch`）、`shrink`（溢出按权重收缩到 min）、
  `min_size`/`max_size`、`align_self`、`aspect_ratio`、`visible`、`percent`。
  `ChildLayout::sized` 保持 grow/shrink=0，旧调用零行为变化。
- ✅ `LayoutItem` 组件串：`min_size/max_size/preferred_size/size_percent/grow/shrink/align_self/
  aspect_ratio/visible`，全部可选；百分比先做「相对父容器内边距后尺寸」，Auto = 用宿主测量值。
- ✅ 溢出诊断：`layout_container_with_report` 返回 `LayoutOverflow { axis, needed, available }`；
  场景解算把它变成 `UiLayoutReason::Overflow` 诊断，不静默裁剪。
- ✅ 根容器无 `UiLayout` 时填满父矩形（根=画布），普通元素仍用固有尺寸。
- 验收：`cargo test -p rustlua-ui` **97/97**（六种 justify、wrap 行高、grow/shrink/min/max、
  aspect、百分比、隐藏子项、align_self、LayoutItem 往返/校验、溢出诊断，以及
  `three_resolution_layout_rect_golden` 在 1280×720/1920×1080/1080×1920 的 rect 读数）；
  `rustlua-ecs` schema **16/16**；engine lib **313/313**；editor **203 lib + 450 bin / 2 ignored**；
  clippy/fmt/docs 全绿。
- ⏳ 未收口：`LayoutItem` 尚未注册进 `typed_scene` 的 `ComponentId`/组件表（编辑器暂时只能通过
  组件字符串看到它）；运行时 `push_scene_ui` 尚未接容器递归布局；文本/图片 intrinsic 与布局缓存
  统计见 E05-3；Inspector 子项约束段与场景 overlay 属 E05-4；3 分辨率是 rect golden，
  **没有截图 golden**，也没有开窗 GUI 目视。

**P3-E05-3 文本 intrinsic / 缓存统计（2026-10-23）**：

- ✅ `rustlua-render::text::measure_builtin_text(text, scale)`：内置 5×7 位图度量返回
  `(size, pixel_bounds)`（与 `TextMesh` 同口径），给没有字体图集上下文的宿主共用。
- ✅ `rustlua-ui::ui_scene_layout::preferred_size_for(components, layout, measure_text)`：固有
  尺寸规则唯一入口——`Image` 取 `UiLayout` 矩形（UI 贴图按矩形拉伸，PNG 像素不是布局输入）、
  `Text` 取宿主度量、其余 64×64。
- ✅ 运行期场景与 `prefab.spawn` 都改走 `build_ui_scene_nodes_with_intrinsic`（内置位图尺子）；
  编辑器 `scene_ui_nodes` 与 UI 预览诊断复用同一函数，不再用 `[128,32]`/`[64,64]` 两套占位。
- ✅ 缓存统计：`resolve_scene_ui_layout_with_stats` 返回 `MeasureStats { hits, misses }`；旧
  `resolve_scene_ui_layout` 签名不变。命中来自「父容器测量子容器 → 摆放子容器时再测一次」。
- 验收：`cargo test -p rustlua-ui` **99/99**（新增 `measure_stats_count_container_cache_reuse`
  实际读数 `hits=1/misses=3`、`preferred_size_prefers_text_intrinsic_then_image_layout`）；
  `cargo test -p rustlua-render text` **18/18**（新增内置度量与 `TextMesh` 逐值一致）；
  engine lib **314/314**（新增 `container_text_child_uses_measured_intrinsic_size`：HBox
  padding 10 下 Text 子项矩形 `(10,10,18,9.33)`，宽度对同一把尺子）；editor **203 lib +
  451 bin / 2 ignored**（新增编辑器节点表同款断言）；scripting **124 + 1 集成**；clippy/fmt 全绿。
- ⏳ 未收口：未做图片的**纹理像素**固有尺寸（当前语义就是作者矩形）；自定义字体在布局期仍按
  内置位图度量（渲染走 MSDF，可能一两个像素差异）；编辑器尚无 `MeasureStats` 面板读数
  （随 E05-4 overlay/预设）；没有开窗 GUI 目视。

**P3-E05-4a/E05-4b Container 作者化与 28 场景回归（2026-10-23）**：

- ✅ Container 检查器补齐 `wrap` 类型与 `justify` 六模式下拉（HBox/VBox/Grid/Wrap 显示；
  Margin/Center 的主轴分布无意义，不显示）。此前 E05-2 记录称「加 justify 下拉」但代码里
  实际只有 align，本轮修正了这处文档/实现漂移。
- ✅ `ContainerPreset`（行 Row/列 Column/网格 Grid/换行 Wrap）只改 Inspector 缓冲
  （kind/spacing，Grid 空列数补 3），提交仍由 E08 提交点落成一条 Undo，不另开写入路径。
- ✅ 子项 `LayoutItem` 检查器段：min/max/preferred/percent（四组 DragValue）、grow/shrink、
  align_self（继承容器/起始/居中/末尾）、宽高比（0 = 关闭）、visible、移除按钮；写回走
  `LayoutItemFields::to_layout`（引擎解析器是唯一校验门），并接入 Inspector 快照/直写/E08
  事务基线，因此一次编辑仍是一条 Undo。`LayoutItem` 同时进入 Add Component 的 UI 分组，
  默认载荷为引擎 `LayoutItem::default()`。
- ✅ Widget 模式「布局调试」开关（工具栏 → 覆盖层）：所有 UI 元素画布局 bounds，Container
  画 padding 与主轴相邻子项 gap 带（HBox/VBox/Wrap），`PreviewErrorKind::Layout` 诊断命中的
  实体标「溢出」。gap 带只覆盖主轴上真实存在的间隙且宽度不超过 spacing×缩放。
- ✅ `LayoutItem` 已进运行时组件表：`rustlua-ui::scene_codec` 注册（渲染层 6 + UI 层 8 = 14 个
  codec）、`rustlua-scene::typed_scene::ComponentId`（`ALL`/`as_str`/`component_id_for_opaque`/
  `SOURCE_SCENE_COMPONENTS`）与 `rustlua-ecs::component_schema`（九个可选字段；`align_self`
  为 start/center/end，尺寸/增长类字段非负）；`examples/game_api/scenes/ui_test.scene` 增加
  默认 `LayoutItem` 样本（默认值布局中性，golden 零 diff）。
- ✅ 28 个 UI 源场景/预制体回归：`editor::panels::scene_view::tests::ui_source_layout_matches_golden`
  在 1280×720 / 1920×1080 / 1080×1920 下解算 28 个源文件（`.prefab` 先实例化），对每个 UI
  实体输出 `rect + 诊断` 的 FNV-1a 值（84 行 golden，`diags=0`；20 个文件三分辨率不变，
  8 个随分辨率变化）。基线是 E05-1..E05-3 之后的值（E05-3 文本 intrinsic 属有意变更，
  E05 前数值不可重建）；`UPDATE_UI_LAYOUT_GOLDEN=1 cargo test -p editor
  ui_source_scene_layout_matches_golden` 重新生成。
- ✅ overlay 溢出角标改按当前预览分辨率解算：`layout_diagnostics` 接收 `preview_resolution`，
  场景视图绘制前 `refresh_layout_diagnostics_if_resolution_changed` 比对已解算分辨率并重算，
  切预览后不再沿用固定 1280×720 的旧几何。
- 验收：`cargo test -p editor` **206 lib + 459 bin / 2 ignored**（新增
  `ui_source_scene_layout_matches_golden`，84 行断言；此前四条作者化/overlay 用例保留）；
  `rustlua-scene` **46 lib + 1 + 4 集成**（codec 表 **14**，`LayoutItem` 编成 `Opaque` 且原文保留）；
  `rustlua-ui` **99/99**；`rustlua-ecs` **161/161**；engine lib **314/314**；
  `component_schema_parity` **1/1**、`typed_scene_parity` **1/1**、示例场景组件覆盖 **1/1**；
  clippy **exit 0**、fmt check **0**、check-docs **exit 0**
  （`docs=66 game_docs=46 h1=114 links=497 ps1_bom_ok=57 ps1_parse_ok=57`）。
- ⏳ 未收口：overlay 的 gap 只取 children 前两项且仅 HBox/VBox/Wrap，Grid/Margin/Center
  不画 gap；28 文件回归是**布局数值 golden**（rect + 诊断），不是截屏 baseline，E05 前数值
  不可重建；没有开窗 GUI 目视（overlay/角标/预设只在测试里驱动）；`verify.ps1`、workspace
  全量、Web/wasm、真实 GPU 未跑。

### E18：实现多分辨率预览和 Safe Area

**目标**：在编辑阶段发现不同设备比例下的布局问题。

**执行内容**：

- 提供 1280×720、1920×1080、2560×1440、3440×1440、1080×1920、800×600 预设。
- 提供 16:9、16:10、21:9、4:3、9:16 比例切换。
- 同时显示设计分辨率、预览分辨率、缩放比例、裁剪区域、Safe Area 和 Anchor 边界。
- Safe Area 支持开关和可视化边界。
- 预览切换不修改场景数据，不重复创建 GPU 资源。

**依赖**：E15、E16、E17。

**验收标准**：布局在各预设下可稳定重算；编辑器结果与运行时结果一致；Safe Area 外溢和裁剪区域清晰可见。

**实现状态（E18，批次 B 首步 + 2026-10-23 收口）**：

- ✅ 预设补齐：预览分辨率扩为 800×600、1280×720、1920×1080、2560×1440、
  3440×1440、1080×1920（覆盖 4:3/16:9/21:9/9:16 主流比例）。
- ✅ Safe Area 数据层：`CanvasConfig.safe_area`（像素 insets）与
  `safe_rect(viewport)`（越界钳制到零尺寸），供 Game View 可视化与运行时共用。
- ✅ 比例切换快捷组：16:9、16:10、21:9、4:3、9:16 五个比例收在**分辨率下拉**里
  （下拉选中文本带当前比例，如 `1280×720 · 16:9`），顶栏不再常驻一排按钮；
  `resolution_for_aspect` 保持当前长边换另一条边（1280×720 → 16:10 得 1280×800、
  21:9 得 1280×549、9:16 得 720×1280），`resolution_matches_aspect` 用 0.5% 容差
  识别当前比例、`aspect_label_for` 给出标签。切换只改预览目标，不写场景数据。
- ✅ Safe Area 可视化：Scene 视图改为读取场景 UI Root 的 `canvas.safe_area`
  （`canvas_config_from_scene` 与选中项无关），用 `CanvasConfig::safe_rect` 按设计
  分辨率钳制后走设计框同一映射；删掉固定 5% 估算，insets 全 0 时不重复画框。
- ✅ 画布 HUD：Scene 视图在设计分辨率标签下、Game 视图在预览徽标内，共用
  `CanvasHud`。常驻只画一行 `宽×高 · 缩放%`（Game 视图 1:1 时补 ` · 1:1`），
  设计分辨率、预览分辨率、屏幕物理像素、显示缩放、裁剪与 Safe Area 的完整读数
  收进该行的悬停提示（`CanvasHud::details`），不再三行常驻压画布；Anchor 边界沿用
  Phase 5c 的选中实体锚点手柄。Game 视图等比 letterbox，裁剪恒为 0。
- ✅ GPU 资源复用：切分辨率只调用现有 `PreviewSession::resize`（保留运行时与
  渲染器），同尺寸帧不再重建渲染目标；GPU 测试断言会话指针不变、尺寸立即生效。
- 验收：`cargo test -p editor` **203 lib + 457 bin / 2 ignored**（新增 6 条：比例
  两条、Safe Area 映射/裁剪两条、HUD 字符串一条、GPU 复用一条）；clippy/fmt 全绿；
  `check-docs.ps1` 退出 0。
- ⏳ 未收口：裁剪读数只描述 Scene 视图设计框被面板裁掉的部分（不是 Canvas 缩放
  模式的理论裁剪）；`CanvasConfig.reference/scale_mode` 尚未接入运行时 UI 布局
  （运行时以预览分辨率作为 UI 参考），HUD 因而不展示未生效的缩放投影；Anchor
  边界只覆盖选中实体；没有开窗 GUI 目视；`verify.ps1`、workspace 全量、Web/wasm
  未跑。另：首次并行跑 `cargo test -p editor` 时 bin 测试进程出现过一次
  `0xc0000005 STATUS_ACCESS_VIOLATION`（当时 lib 203/203 已过），立即重跑
  457/457 全过；疑为多个 GPU 测试并行 teardown 抖动，尚未定位到具体用例。

### E19：实现 Widget Designer 与 Palette

**目标**：建立独立、可复用的 UI 创建工作流。

**执行内容**：

- 支持独立 Widget 资产和双击打开 Widget Designer。
- Widget Designer 包含 Canvas、Hierarchy、Palette、Inspector、Canvas View 和 Preview/Game View。
- Palette 支持拖拽创建基础 UI 节点和 Container。
- 支持 Widget 与 SceneDocument 中 `UiRoot` 的挂载关系。
- Widget 编辑提供 Isolation 和 Context 两种模式。
- 建立打开、保存、关闭、Dirty 检查和错误恢复流程。

**依赖**：E15、E16、E17、E18。

**验收标准**：可以创建 Widget、拖入控件、修改布局、保存、重新打开并挂载到场景；Widget 与普通 Scene 文档切换时未保存保护有效。

**实现状态（E19，批次 D）**：

- ✅ Widget Mode 已有基础 Palette：Text / Image / Button 支持点击创建和拖放到 Scene View；
  创建路径复用统一场景文档、Inspector 与 History。
- ✅ 核心层新增 `ui_scene_layout`：按父链解析 Canvas / UiLayout / Container，
  输出容器受控矩形与 MissingParent / CircularParent 诊断，供编辑器和运行时共用。
- ✅ E19 收口：新增独立 `.widget` 扩展名识别、保存/加载与 `SceneDocument::new_widget`；
  工具栏 New Widget 走 E06 未保存文档切换保护，创建带 Canvas 根的 Widget 文档并
  自动进入 Widget Mode。Widget 数据仍使用 SceneDocument 实体模型，便于复用
  History / Inspector / Scene View 管线。
- ⏳ 后续扩展：独立 Widget Designer 工作区和 Isolation / Context 模式不阻塞本轮
  Widget 资产验收，登记到后续编辑器工作区重构。

### E20：完善 UI Widget/Prefab 复用

**目标**：支持按钮、面板、菜单等 UI 组件的复用和变体管理。

**执行内容**：

- 增加 Prefab 编辑面包屑和来源定位。
- 实现 Open Source、Apply Overrides、Revert Overrides、Unpack 和 Override List。
- Button 支持 Normal、Hover、Pressed、Disabled、Focused 状态配置。
- 明确 Widget 资产与 Prefab 资产的边界和引用规则。
- 对实例覆盖、源资源修改和删除引用提供状态提示。

**依赖**：E19、现有 Prefab 能力。

**验收标准**：实例可查看覆盖列表并执行应用、还原和解包；源资源修改后实例更新符合预期；状态资源可在编辑器和运行时一致显示。

**实现状态（E20，批次 D）**：

- ✅ Prefab 根实例已有 Open Source、Apply/Reapply、Override List、写回源并更新实例。
- ✅ 新增 Unpack：一次 History 移除实例子树中的 `prefab.*` 元数据，保留当前
  组件/覆盖数据并转为普通实体；编辑锁定/隐藏标记保留。
- ✅ E20 收口：Prefab Inspector 提供 Revert Overrides，通过 History 移除实例并
  重新实例化源资源；新增 `ButtonStateSet` 支持 Normal/Hover/Pressed/Disabled/
  Focused 五状态纹理解析、序列化与 Normal 兜底。
- ✅ 资产边界：Widget 使用 `.widget` 扩展名与文档元数据，Prefab 继续使用
  `.prefab` 资产和 `prefab.source` 引用链。

## 7. 阶段 4：结构化组件、事件和数据绑定

### E21：建立 Component Schema 第一阶段兼容层

**目标**：在不破坏旧场景格式的情况下，为组件提供类型、默认值、解析、序列化和验证能力。

**执行内容**：

- 定义字段类型、默认值、统一解析器、统一序列化器和验证器。
- 为错误提供字段路径、组件名称和可读消息。
- 优先迁移 Transform、UiLayout、Text、Image、Button/Event。
- 读取旧 `BTreeMap<String, String>` 数据并转换为类型化值。
- 写入策略保持向后兼容，明确何时使用旧格式、何时写入新格式。
- 为组件增加版本号和升级入口。

**依赖**：E13、E16、E20。

**验收标准**：旧场景可正常打开、编辑和运行；非法字段能定位到具体路径；升级失败不会静默丢失数据。

### E22：实现结构化 UI 事件绑定

**目标**：将字符串形式的 `clickEvent` 升级为可校验、可编辑的事件配置。

**执行内容**：

- 定义事件类型、目标实体引用、函数名和参数值。
- 首批支持 Pointer Enter、Pointer Exit、Pointer Down、Pointer Up、Click、Focus、Submit、Value Changed、Drag。
- Inspector 提供 Events 分组、Target、Function 和 Arguments 编辑。
- 对目标不存在、函数不存在和参数类型不匹配提供诊断。
- 提供旧 `clickEvent` 数据读取和转换。

**依赖**：E21、E20。

**验收标准**：事件可创建、修改、删除、序列化和运行；错误绑定在编辑器中可见，不因运行时才发现。

### E23：实现最小 ViewModel 和 Property Binding

**目标**：支持常用 UI 属性与游戏数据之间的单向绑定。

**执行内容**：

- 定义 Game Data、ViewModel、Binding 和 Widget Property 的关系。
- 首批支持 Text.text、Image.visible、Progress.value 等简单属性。
- 支持绑定路径解析、类型转换和路径错误提示。
- 编辑器显示绑定来源和目标属性。
- 运行时更新绑定值，避免编辑器预览与运行时使用两套规则。

**依赖**：E21、E22、E19。

**验收标准**：示例绑定可在预览和运行时更新；路径错误、类型不匹配和目标属性缺失有明确诊断；绑定配置可保存并重新加载。

### E24：增加结构化预览诊断

**目标**：把布局、资源、脚本、渲染和输入错误集中呈现。

**执行内容**：

- 定义 `PreviewError` 分类：SceneLoad、ScriptRuntime、AssetMissing、Render、Input、Layout。
- Game View 和 Console 统一显示分类、对象路径和错误详情。
- 对 Container 溢出、缺失纹理、事件函数不存在和布局计算失败提供可定位信息。
- 错误不阻塞无关 UI 的继续预览。

**依赖**：E18、E21、E22、E23。

**验收标准**：预览错误按类型显示并可定位到节点或组件；同一错误不会被每帧重复刷屏；修复后诊断自动清除。

**实现状态（E21–E24，批次 E）**：

- ✅ E21 `component_schema`：Transform / Text / Image / UiLayout 的类型化解析、
  序列化、校验和版本号；错误带组件与字段路径。
- ✅ E22 `ui_events`：9 类 UI 事件、目标/函数/参数绑定、编辑器校验；旧
  `clickEvent` 只在场景实例化边界转换为默认 Click 绑定，运行时只消费
  `UiEvents`。
- ✅ E23 `ui_binding`：Text.text / Image.visible / Progress.value 的最小单向绑定、
  路径解析、类型转换与诊断。
- ✅ E24 `preview_diagnostics`：六类预览错误、去重、发生次数统计和整体替换。
- ✅ E21–E24 收口：Inspector 提供结构化 UiEvents / UiBindings 编辑；Game View
  和 Console 消费同一份去重诊断，显示错误类型、实体、组件、消息和发生次数；
  文档变更后全量重建诊断，修复源即可自动清除。

## 8. 阶段 5：性能和高级能力

### E25：优化 History、布局和大场景刷新

**目标**：支持大型场景和大量 UI 节点编辑。

**执行内容**：

- 评估 Transform 和 Property 从完整快照迁移到 Patch History 的边界。
- 对大场景采用局部更新和受影响子树刷新。
- 增加 UI 布局缓存、失效规则和命中统计。
- 预览分辨率变化时复用 GPU 纹理资源，降低资源抖动。
- 增加编辑器性能采样和可视化指标。

**依赖**：E04、E07、E16、E17、E18。

**验收标准**：大场景拖拽无明显卡顿；普通属性修改不触发每帧保存；布局修改只重算受影响子树；Undo 内存增长可控。

### E26：实现 UI 动画和状态机预览

**目标**：补齐 Widget Designer 的动态交互能力。

**执行内容**：

- 增加动画轨道、关键帧、播放头和预览控制。
- 支持 Button 状态与动画状态关联。
- 明确动画预览状态与场景数据提交边界。
- 为动画编辑复用 EditTransaction 和 History。
- 在 Game View 中提供静态布局与动态预览切换。

**依赖**：E19、E20、E25。

**验收标准**：动画可以创建、预览、保存、撤销和运行；预览播放不会污染场景数据。

**实现状态（E26，批次 F 数据层）**：

- ✅ `ui_animation`：关键帧轨道、线性/Step/SmoothStep 采样、clip 校验和
  隔离式 Preview 播放；播放头状态不写回 SceneDocument。
- ✅ E26 收口：Inspector 提供轨道/关键帧/插值编辑和播放、停止、归零控制；
  编辑提交走 History，顶层主循环推进隔离预览，播放时间不写回场景数据。
- ⏳ 后续扩展：Button 五状态与动画状态机的显式关联，以及 Game View 的静态/
  动态模式专用开关。

### E27：扩展多窗口 Inspector 和编辑器插件接口

**目标**：为大型项目和团队扩展保留稳定入口。

**执行内容**：

- 支持独立 Inspector 窗口及锁定不同选择上下文。
- 将组件注册、Palette 注册、Inspector 绘制和诊断扩展点整理为插件接口。
- 增加插件生命周期、版本兼容和错误隔离规则。
- 插件失败时不影响核心编辑器启动和已有场景读取。

**依赖**：E13、E19、E21、E25。

**验收标准**：多个 Inspector 可独立锁定；外部组件或控件可通过扩展接口注册；插件异常不会破坏核心编辑流程。

**实现状态（E25/E27，批次 F 数据层）**：

- ✅ E25 `ui_layout_cache`：布局矩形缓存、命中/未命中/失效统计、实体与子树
  失效、全量失效；为局部刷新提供稳定失效规则。
- ✅ E27 `editor_plugins`：版本化插件接口、加载/卸载生命周期、重复和版本校验、
  失败与 panic 隔离。
- ✅ E25/E27 收口：编辑器应用持有 `UiLayoutCache` 与 `PluginRegistry`，启动时
  注册并加载内置 fault-isolated core 插件；文档变更失效布局缓存；Scene View
  性能 HUD 显示缓存条目、命中、未命中、命中率和失效次数。
- ⏳ 后续扩展：完整 Patch History 迁移、预览纹理 GPU 复用和多窗口锁定 Inspector
  保持开放，分别登记到阶段 5 后续批次。

## 9. 测试与验收执行清单

### 9.1 自动化测试

- 文档切换 Dirty 状态和保存确认流程。
- SelectionModel 单选、多选、删除回退和无效引用清理。
- Undo/Redo 后各编辑器状态同步。
- EditTransaction 提交、取消、重复提交和单条 History。
- Hierarchy 过滤祖先路径、循环引用检查和重挂载。
- UiLayout Anchor + Offset 计算。
- Container 最小尺寸、Expand、Fill、Padding 和 Stretch Ratio。
- Canvas 缩放模式、Safe Area 和多分辨率计算。
- Component Schema 旧格式解析、验证、序列化和版本升级。
- UI Event Binding 路径、参数和目标校验。
- Property Binding 路径解析和类型转换。

### 9.2 手工回归场景

1. 创建包含 World Entity 和 UiRoot 的场景，编辑并保存。
2. 在 Hierarchy 深层节点中搜索、重命名、复制、粘贴和重挂载。
3. 框选多个节点，在 Inspector 中修改共有属性。
4. 拖动节点、取消拖动、提交拖动并连续 Undo/Redo。
5. 编辑 Text、脚本路径和数字字段，验证快捷键焦点行为。
6. Play 后修改运行时数据，Stop 后确认编辑上下文和运行时修改策略。
7. 创建 Widget，使用 Palette 添加控件，切换 Isolation/Context 并保存。
8. 在 16:9、21:9、4:3、9:16 下检查 Anchor、Container 和 Safe Area。
9. 创建 Prefab 实例，修改覆盖并执行 Apply、Revert、Unpack。
10. 删除纹理、删除事件目标或输入错误绑定，检查结构化诊断。

### 9.3 验收门槛

| 类别 | 必须满足 |
|---|---|
| 数据安全 | Dirty 文档切换有确认；Undo/Redo 状态一致；Play/Stop 不丢编辑上下文 |
| 操作效率 | 一次连续编辑一条 Undo；编辑期间不持续写磁盘；多选可批量修改 |
| UI 布局 | Anchor + Offset 可用；至少 5 种 Container 可用；编辑器和运行时计算一致 |
| 预览能力 | 支持 16:9、21:9、4:3、9:16；Safe Area 和裁剪区域可视化 |
| 兼容性 | 旧场景、旧 Prefab 和旧事件数据可读取；迁移错误可定位 |
| 性能 | 大场景无明显卡顿；布局局部重算；GPU 资源不频繁抖动 |

## 10. 建议执行批次

### 批次 A：可靠性封板

包含 E01～E10。完成后应冻结文档切换、选择、History、保存和 Play/Stop 的基础行为，作为后续 UI 重构的稳定基线。

### 批次 B：编辑器交互封板

包含 E11～E14。完成后应具备稳定的 Scene View 工具管线、多选 Inspector、上下文菜单和快捷键体系。

### 批次 C：布局能力封板

包含 E15～E18。完成后应能在现有场景中创建 Canvas、配置 Anchor + Offset、使用基础 Container 并进行多分辨率预览。

### 批次 D：Widget 资产封板

包含 E19～E20。完成后应能独立编辑和复用 Widget，并支持 Prefab 覆盖管理。

### 批次 E：数据能力封板

包含 E21～E24。完成后应完成核心 UI 组件结构化、事件配置、简单绑定和预览诊断。

### 批次 F：性能与扩展

包含 E25～E27。仅在前序能力稳定、测试覆盖达到门槛后执行。

## 11. 风险与控制措施

| 风险 | 控制措施 |
|---|---|
| 统一选择模型影响现有面板 | 先保留适配层，逐个入口迁移并增加一致性断言 |
| 事务化修改导致预览与提交状态混淆 | 明确 Scene Data、Preview State 和 Transient Interaction State 的边界 |
| UI 布局迁移破坏旧场景 | 旧格式只读兼容，新增版本号和可回滚升级流程 |
| Container 与手动 Position 语义冲突 | Inspector 明确只读/警告状态，并统一由布局系统计算最终位置 |
| Prefab、Widget 和 Scene 引用关系复杂 | 先定义资源边界、所有权和 Override 优先级，再实现 UI 复用 |
| 运行时和编辑器布局结果不一致 | 共享布局计算模块，并加入跨环境快照测试 |
| 大场景性能下降 | 先采样基线，再实施局部刷新、缓存和 Patch History |
| 插件或外部组件破坏编辑器 | 插件接口版本化，加载和绘制错误隔离 |

## 12. 最小可交付版本

第一轮实现建议只纳入以下内容：

1. E02 统一文档变更入口。
2. E03 SelectionModel。
3. E04 Undo/Redo 状态同步。
4. E05 输入焦点处理。
5. E06 未保存保护。
6. E07～E08 编辑事务和拖拽/Inspector 接入。
7. E09 Play/Stop 上下文恢复。
8. E10 Hierarchy 过滤和基础重挂载。
9. E15 Canvas 根节点。
10. E16 Anchor + Offset。
11. E17 中的 VBox、HBox、Grid、Margin、Center 五种 Container。
12. E18 基础多分辨率预览和 Safe Area。

该版本完成后即可验证计划中最优先的五项目标，同时不要求立即切换到全量结构化组件格式。后续 Widget 资产、事件绑定、ViewModel、动画和插件系统应在该基础稳定后继续推进。

## 13. 2026-09-06 验收记录

### 13.1 收口范围

本轮按 E19～E27 的验收目标补齐运行时和编辑器消费链路：

1. 运行时场景初始化时构建 UI 节点关系，并接入 `ui_scene_layout` 容器布局
   解析；布局节点以运行时 spawn 序号作为稳定 key，避免场景文件 id 与运行时
   实体映射错位。
2. Scene View 对容器受控的 Image / Text 使用共享解析矩形；Game View 和
   Console 显示去重后的结构化预览诊断。
3. New Widget 创建带 Canvas 根的 `.widget` 文档，并复用未保存文档切换保护。
4. UI 动画隔离预览由编辑器主循环按稳定 delta time 推进；布局缓存统计通过
   Scene View 性能 HUD 展示。

### 13.2 自动化验收

| 命令 | 结果 |
|---|---|
| `cargo fmt --all` | 通过 |
| `cargo build -p editor` | 通过 |
| `cargo test -p editor --lib` | 104 / 104 通过 |
| `cargo test -p editor --bin editor` | 91 / 93 通过；`import_spawn_end_to_end`、`publish_spawn_end_to_end` 为既有失败 |
| `cargo test -p engine --lib` | 406 / 410 通过；`production_sample_scene_instantiates_gameplay_entities` 为既有失败 |

本轮接入运行时容器布局时发现的 3 个布局相关回归已修复并复测通过：

1. `ui_layout_component_overrides_inline_image_rect`
2. `canvas_ui_follows_ui_reference_on_resolution_change`
3. `ui_demo_project_renders_ui_and_dispatches_clicks`

### 13.3 结论

E19～E27 规划中的核心数据和消费链路已收口，未完成项已在对应任务中登记为
后续扩展。既有失败与本轮编辑器 UI 优化改动无关，需继续在独立的引擎导入/
发布和玩法场景实例化任务中排查。
