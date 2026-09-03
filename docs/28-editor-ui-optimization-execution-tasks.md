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

**重点文件**：[main.rs](../engine/crates/editor/src/main.rs)、[actions.rs](../engine/crates/editor/src/actions.rs)、[history.rs](../engine/crates/editor/src/history.rs)。

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

**重点文件**：[hierarchy.rs](../engine/crates/editor/src/panels/hierarchy.rs)、[scene_view.rs](../engine/crates/editor/src/panels/scene_view.rs)、[inspector.rs](../engine/crates/editor/src/panels/inspector.rs)。

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

### E08：将 Scene View 和 Inspector 编辑接入事务

**目标**：消除拖拽和 Inspector 编辑过程中的频繁保存与历史污染。

**执行内容**：

- Scene View 移动、旋转、缩放、Anchor 拖动接入事务。
- Inspector 数字拖动、颜色选择器和文本提交接入事务。
- 鼠标释放、输入提交或焦点离开时提交。
- 拖动取消、Escape 或弹窗关闭时取消。
- 保存策略改为提交后异步自动保存，不在交互过程中写磁盘。

**依赖**：E07。

**重点文件**：[scene_view.rs](../engine/crates/editor/src/panels/scene_view.rs)、[inspector.rs](../engine/crates/editor/src/panels/inspector.rs)、[history.rs](../engine/crates/editor/src/history.rs)。

**验收标准**：拖动过程中磁盘文件不持续变化；一次拖动对应一条历史记录；Inspector 输入提交后可恢复且不会产生空操作。

### E09：完善 Play/Stop 编辑上下文恢复

**目标**：Play/Stop 不丢失用户当前编辑上下文。

**执行内容**：

- Play 前保存选中对象、多选集合、Inspector 锁定、Hierarchy 展开、Scene View pan/zoom、当前工具、文档标签和预览分辨率。
- Stop 后按对象 ID 和文档有效性恢复上下文。
- 对运行期间被删除或失效的对象执行安全回退选择。
- 确保编辑文档与运行时副本继续隔离。

**依赖**：E03、E04。

**验收标准**：Play/Stop 前后视图、选择和工具状态保持；运行时对象不存在于编辑文档时不会产生悬空引用。

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

### E27：扩展多窗口 Inspector 和编辑器插件接口

**目标**：为大型项目和团队扩展保留稳定入口。

**执行内容**：

- 支持独立 Inspector 窗口及锁定不同选择上下文。
- 将组件注册、Palette 注册、Inspector 绘制和诊断扩展点整理为插件接口。
- 增加插件生命周期、版本兼容和错误隔离规则。
- 插件失败时不影响核心编辑器启动和已有场景读取。

**依赖**：E13、E19、E21、E25。

**验收标准**：多个 Inspector 可独立锁定；外部组件或控件可通过扩展接口注册；插件异常不会破坏核心编辑流程。

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
