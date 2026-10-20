# 编辑器 UI 编辑体验完善与优化方案

## 1. 文档目标

本文档用于规划当前 RustLua 游戏引擎编辑器的 UI 编辑体验改造，目标是对标 Unity、Unreal UMG 和 Godot Control 的成熟工作流，提升 UI 创建、布局、预览、复用和调试效率。

当前编辑器已具备 Scene View、Hierarchy、Inspector、Prefab、Tilemap、Play/Stop 和 Undo/Redo 等基础能力，但 UI 编辑仍存在以下核心问题：

- UI 与普通场景实体混合，缺少独立的 Widget/Canvas 编辑模型。
- Text/Image 的位置数据与普通 Transform 分离，编辑逻辑复杂。
- Inspector、Hierarchy、Scene View 存在多套选择状态。
- Inspector 和画布拖拽可能频繁提交历史或保存文件。
- UI 组件主要以字符串存储，类型、校验和扩展能力不足。
- 缺少 Anchor + Offset、Container 自动布局、安全区域和多分辨率预览。
- Play/Stop、Prefab、场景切换和 Undo/Redo 的状态恢复不够完整。

总体策略：先修复可靠性和状态同步，再重构编辑交互，随后建设独立 Widget Designer，最后迁移结构化组件和数据绑定系统。

---

## 2. 对标主流引擎的目标能力

### 2.1 Unity

重点借鉴：

- Hierarchy + Scene View + Inspector 的对象编辑工作流。
- Canvas 作为 UI 编辑根节点。
- 移动、旋转、缩放、聚焦和快捷键操作。
- 多对象 Inspector 和 Mixed Value。
- Prefab 独立编辑、上下文编辑和 Override 管理。

### 2.2 Unreal UMG

重点借鉴：

- Palette、Hierarchy、Visual Designer、Details、Animations 分区。
- Widget Blueprint 作为独立 UI 资产。
- 拖拽式控件创建。
- UI 设计与运行逻辑分离。
- ViewModel 和可视化数据绑定。
- UI 动画时间轴。

### 2.3 Godot Control

重点借鉴：

- Control 节点作为 UI 基础类型。
- Anchor + Offset 的响应式布局。
- VBox、HBox、Grid、Margin、Center、Panel、Scroll 等 Container。
- Minimum Size、Expand、Fill、Stretch Ratio 等布局约束。
- Theme、Focus 和 UI 输入事件体系。

---

## 3. 总体目标架构

### 3.1 编辑器界面

```text
Editor Shell
├── Asset Browser
├── Scene / Widget Tabs
├── Hierarchy
├── Palette
├── Inspector
├── Canvas / Scene View
├── Animation Timeline
├── Console
└── Preview / Game View
```

### 3.2 数据层

```text
SceneDocument
├── Scene Nodes
├── UI Widget Assets
├── Prefab Assets
├── Component Schema
├── Layout Constraints
├── Event Bindings
└── Editor Metadata
```

### 3.3 编辑状态层

```text
SelectionModel
HistoryManager
EditTransaction
SceneViewState
InspectorState
PreviewSession
```

### 3.4 状态分层

明确区分以下四类状态：

```text
Scene Data
Editor Persistent Settings
Editor Session State
Transient Interaction State
```

| 状态 | 是否进入场景文件 |
|---|---|
| UI 节点位置、尺寸和布局 | 是 |
| Prefab Override | 是 |
| UI 节点隐藏状态 | 按产品需求决定 |
| Hierarchy 展开状态 | 否，保存到编辑器配置 |
| Scene View 缩放和相机 | 否，保存到编辑器会话 |
| 当前选中对象 | 否，属于会话状态 |
| 拖拽中的临时位置 | 否，属于临时状态 |
| Inspector 搜索关键词 | 否，属于用户偏好 |

---

## 4. P0：可靠性和状态同步

## 4.1 场景切换增加未保存确认

场景切换、新建场景、打开 Prefab 或打开其他文档前必须检查当前文档是否有未保存修改。

统一流程：

```text
request_document_change()
    ├── 当前无修改 → 直接切换
    ├── 已保存 → 直接切换
    └── 未保存
        ├── 保存并切换
        ├── 放弃修改并切换
        └── 取消
```

必须覆盖：

- 场景下拉切换。
- 新建场景。
- 打开 Prefab。
- 打开其他编辑文档。
- 切换项目。
- 关闭编辑器。

## 4.2 Undo/Redo 后统一刷新

Undo/Redo 不应只替换 `SceneDocument`，还应统一刷新：

- Hierarchy 选择和节点状态。
- Inspector 当前实体和字段。
- Scene View 预览状态。
- UI 布局缓存。
- RuntimeScene 缓存。
- 多选集合。
- 已被删除对象的选择引用。

建议建立统一入口：

```text
on_document_changed(change)
    ├── reconcile_selection()
    ├── sync_inspector()
    ├── invalidate_scene_view()
    ├── invalidate_game_preview()
    └── refresh_hierarchy_metadata()
```

所有文档变更都必须经过该入口，包括打开场景、Prefab 应用、批量属性修改、运行时修改应用等。

## 4.3 统一选择模型

当前存在多套选择状态，建议引入统一的 `SelectionModel`：

```rust
struct SelectionModel {
    primary: Option<SceneEntityId>,
    entities: BTreeSet<SceneEntityId>,
    source: SelectionSource,
}
```

统一处理以下来源：

- Hierarchy 点击。
- Scene View 点击。
- 框选。
- Ctrl/Command 加选。
- 删除后的回退选择。
- Prefab 实例化。
- Undo/Redo。
- Play/Stop。

选择变化统一触发：

```text
SelectionChanged
    ↓
Hierarchy 更新
Inspector 更新
Scene View 更新
工具栏状态更新
```

## 4.4 处理输入焦点

当用户正在编辑实体名称、脚本路径、Text 内容等文本字段时：

- Ctrl+Z/Ctrl+Y 应优先交给文本控件。
- Ctrl+S 不应干扰文本输入。
- Delete 不应删除场景对象。
- Escape 应优先取消当前文本编辑或弹窗。

---

## 5. P0：编辑事务与保存策略

## 5.1 统一编辑事务

所有连续交互统一采用以下流程：

```text
Begin
  ↓
Preview Update
  ↓
Commit / Cancel
  ↓
Single History Entry
```

建议引入：

```rust
struct EditTransaction {
    id: TransactionId,
    label: String,
    kind: EditKind,
    targets: Vec<SceneEntityId>,
    before: SceneDocument,
    preview: SceneDocument,
}
```

接口：

```text
begin_transaction()
update_preview()
commit_transaction()
cancel_transaction()
```

适用场景：

- Scene View 拖拽。
- Inspector 数值拖动。
- 颜色选择器。
- Tilemap 连续绘制。
- Anchor 拖动。
- 多选对齐和分布。
- 批量组件修改。

## 5.2 编辑过程中不保存磁盘

拖拽和数值编辑期间只更新临时 Preview，不执行磁盘写入。

在鼠标释放、输入提交或焦点离开时：

1. 提交一条 History 命令。
2. 标记文档 Dirty。
3. 由自动保存机制异步写入。

目标：一次拖拽只产生一条 Undo，不在拖拽过程中持续克隆完整文档和写文件。

## 5.3 增强 History 元数据

在保留快照型 History 的前提下，为命令增加：

```rust
struct EditCommand {
    label: String,
    kind: CommandKind,
    targets: Vec<SceneEntityId>,
    component: Option<String>,
    property: Option<String>,
    transaction_id: Option<TransactionId>,
    before: SceneDocument,
    after: SceneDocument,
}
```

历史记录示例：

```text
Move 3 Objects
Change Button Width
Set Text Color
Reparent MainMenu
Add Image Component
```

Transform 合并必须以事务边界为准，而不是仅根据相邻命令类型判断。

---

## 6. UI Widget 和 Canvas 模型

## 6.1 引入独立 Widget 资产

建议支持独立 UI 文档：

```text
MainMenu.widget
├── Canvas
│   ├── Background
│   ├── Title
│   ├── ButtonGroup
│   │   ├── StartButton
│   │   └── ExitButton
│   └── VersionLabel
```

双击 Widget 资产进入：

```text
Widget Designer
```

第一阶段可以继续复用现有 `SceneEntity` 作为底层节点，但编辑器层和序列化层需要增加 UI 专属语义。

推荐先支持：

```text
SceneDocument
    ├── World Entities
    └── UiRoot Entities
```

后续再逐步迁移为真正独立的 Widget 资源。

## 6.2 Canvas 根节点

Canvas 应定义：

- 设计参考分辨率。
- 缩放模式。
- 屏幕适配策略。
- Safe Area。
- 输入层级。
- 渲染层级。
- 是否受相机影响。

建议配置：

```text
Canvas Settings
├── Reference Resolution: 1280 × 720
├── Scale Mode: Match Width / Match Height / Expand
├── Safe Area: Enabled
├── Pixel Snap: Enabled
└── Render Layer: UI
```

当前引擎已经具备 Screen/Canvas 空间和参考分辨率，可在此基础上扩展。

## 6.3 Anchor + Offset

从单点 Anchor 升级为四边约束：

```text
anchor_left
anchor_top
anchor_right
anchor_bottom
offset_left
offset_top
offset_right
offset_bottom
```

提供常用预设：

```text
Top Left
Top Center
Top Right
Center
Bottom Left
Bottom Center
Bottom Right
Full Rect
Stretch Horizontal
Stretch Vertical
```

这样可以支持固定边距、全屏拉伸、中心弹窗、右上角状态栏等常见布局。

## 6.4 Container 自动布局

第一批实现：

| 组件 | 用途 |
|---|---|
| VBox | 垂直菜单和设置列表 |
| HBox | 顶部栏和按钮行 |
| Grid | 背包、技能栏和选项网格 |
| Margin | 内边距 |
| Center | 弹窗居中 |
| Panel | 背景和边框 |
| Scroll | 长列表和日志 |
| AspectRatio | 保持图片或视频比例 |

Container 下的子节点应提供：

- Minimum Size。
- Expand。
- Fill。
- Alignment。
- Padding。
- Stretch Ratio。

当父级 Container 控制子节点位置时，Inspector 应明确提示：

```text
该节点的位置由父级 VBox 控制，手动 Position 不生效。
```

## 6.5 统一 UI Layout 与普通 Transform

建议引入统一的 UI 布局组件：

```rust
struct UiLayout {
    anchors: [f32; 4],
    offsets: [f32; 4],
    min_size: [f32; 2],
    max_size: Option<[f32; 2]>,
    pivot: [f32; 2],
    rotation: f32,
    scale: [f32; 2],
}
```

Text 和 Image 只负责视觉属性：

```text
Text
├── content
├── font
├── font_size
├── color
└── alignment

Image
├── texture
├── tint
└── nine_slice
```

统一后的布局组件可以收敛：

- 选择框。
- 移动和旋转。
- 缩放。
- 对齐和分布。
- 多选编辑。
- Anchor 编辑。
- Inspector 展示。

---

## 7. Widget Designer 和 Scene View

## 7.1 编辑模式

Scene View 建议提供：

```text
Scene Mode
Widget Mode
Prefab Mode
```

Widget Mode 默认：

- 使用正交视图。
- 显示设计分辨率。
- 显示 Anchor 辅助线。
- 显示 Safe Area。
- 显示 Margin/Padding。
- 显示 UI 层级。
- 显示 Container 布局边界。
- 支持不同分辨率预览。

## 7.2 推荐工具栏

```text
[Select] [Move] [Rotate] [Scale]
[Anchor] [Layout]
[Snap]
[Preview Resolution]
[Show Safe Area]
[Show Guides]
```

高频操作应提供文字标签和快捷键，不能只依赖符号图标和 Tooltip。

## 7.3 Scene View 拆分

当前 Scene View 职责过重，建议拆分为：

```text
scene_view/
├── mod.rs
├── renderer_2d.rs
├── renderer_3d.rs
├── interaction.rs
├── selection.rs
├── transform_tool.rs
├── gizmo.rs
├── tilemap.rs
├── ui_layout.rs
├── hit_test.rs
└── drop_handler.rs
```

推荐交互管线：

```text
输入采集
    ↓
InteractionEvent
    ↓
HitTest
    ↓
Selection / Tool Controller
    ↓
Preview State
    ↓
Edit Transaction
    ↓
History Commit
```

## 7.4 编辑辅助

建议增加：

- 对象边界框。
- Pivot 点。
- Anchor 点。
- 四边拉伸手柄。
- 旋转手柄。
- 对齐辅助线。
- 间距提示。
- 选中对象名称。
- 锁定对象半透明显示。
- 隐藏对象轮廓显示。
- 父子层级连接线。
- 像素网格。
- 吸附步长显示。

## 7.5 多分辨率预览

至少提供以下预览预设：

```text
1280 × 720
1920 × 1080
2560 × 1440
3440 × 1440
1080 × 1920
800 × 600
```

提供比例快速切换：

```text
[16:9] [16:10] [21:9] [4:3] [9:16]
```

同时显示：

- 当前设计分辨率。
- 当前预览分辨率。
- 缩放比例。
- 内容裁剪区域。
- Safe Area。
- Anchor 边界。

---

## 8. Hierarchy 优化

## 8.1 修复过滤逻辑

过滤应支持祖先路径保留：

```text
matches(node) || has_matching_descendant(node)
```

显示规则：

- 当前节点匹配：显示整个子树。
- 当前节点不匹配但子节点匹配：显示当前节点作为路径并自动展开。
- 当前节点和子树都不匹配：隐藏。

## 8.2 增加拖拽重挂载

拖拽时显示：

```text
蓝色线：插入目标前
蓝色线：插入目标后
高亮框：作为目标子节点
红色提示：循环引用或非法目标
```

释放后使用一次 Edit Transaction 包装 Reparent 操作。

## 8.3 增加上下文菜单

右键节点提供：

```text
Create Child
Create Sibling
Rename
Duplicate
Delete
Move to Root
Set as Parent
Create Prefab
Open Prefab
Unpack Prefab
Copy
Paste
Focus in Scene View
```

## 8.4 状态持久化

建议：

- 折叠状态保存到编辑器配置。
- 视图相机保存到编辑器会话。
- 选择状态保存到编辑器会话。
- 场景对象、布局和 Prefab Override 保存到场景文件。
- 隐藏和锁定状态根据是否属于场景制作语义决定是否持久化。

---

## 9. Inspector 优化

## 9.1 组件注册式 Inspector

建议引入组件描述和注册表：

```rust
struct ComponentDescriptor {
    name: &'static str,
    category: ComponentCategory,
    can_add: bool,
    can_remove: bool,
    draw: fn(&mut Ui, &mut ComponentContext),
    read: fn(&SceneEntity) -> ComponentResult,
    write: fn(&mut SceneEntity, &ComponentValue) -> ComponentResult,
    validate: fn(&ComponentValue) -> Vec<ValidationMessage>,
}
```

新组件接入流程应变为：

```text
注册组件
    ↓
定义字段
    ↓
定义序列化
    ↓
定义 Inspector 绘制
    ↓
自动进入 Add Component
```

## 9.2 Inspector 结构

```text
Inspector
├── Selection Header
│   ├── 对象名称
│   ├── 类型
│   ├── Active
│   ├── Prefab 状态
│   └── 锁定 Inspector
├── Common
│   ├── Transform
│   ├── Visibility
│   ├── Layer
│   └── Tags
├── UI Layout
├── Appearance
├── Interaction
├── Script
└── Add Component
```

每个组件提供：

```text
[启用] Component Name       [菜单]
```

菜单包括：

```text
Reset
Copy
Paste
Remove
Move Up
Move Down
```

## 9.3 多选 Inspector

多选时显示：

```text
3 Objects Selected
```

共有组件正常显示，不同值显示 Mixed：

```text
Position X     —
Scale          1.0
Color          Mixed
```

修改共有字段时批量写入所有选中对象。

## 9.4 Transform 编辑体验

建议：

```text
Position
[X] 0.0    [Y] 0.0    [Z] 0.0

Rotation
[Angle] 0°

Scale
[X] 1.0    [Y] 1.0    [锁] 保持比例
```

要求：

- X/Y/Z 使用轴颜色。
- 旋转明确显示单位。
- 3D 不直接暴露四元数，优先显示欧拉角。
- 支持 Reset。
- 支持复制粘贴。
- 支持输入和拖拽两种方式。
- 支持右键恢复默认值。
- 支持表达式输入。
- 支持 Mixed Value。
- 支持字段级 Undo。

---

## 10. Prefab 和组件复用

强化现有 Prefab 能力，支持：

```text
Prefab 编辑面包屑
Scene > HUD > MainMenu > ButtonGroup
```

Prefab 实例提供：

```text
Open Source
Apply Overrides
Revert Overrides
Unpack
Override List
```

UI 组件应支持保存为可复用 Widget/Prefab，并提供标准状态：

```text
Button
├── Normal
├── Hover
├── Pressed
├── Disabled
└── Focused
```

Prefab 编辑支持两种模式：

- Isolation：只显示 Prefab 内容。
- Context：显示外部场景，但外部对象锁定不可编辑。

---

## 11. UI 事件和数据绑定

## 11.1 结构化事件绑定

将当前 `clickEvent` 标记升级为结构化配置：

```rust
struct UiEventBinding {
    event: UiEventType,
    target: Option<EntityReference>,
    function: String,
    arguments: Vec<UiValue>,
}
```

事件类型：

```text
Pointer Enter
Pointer Exit
Pointer Down
Pointer Up
Click
Focus
Submit
Value Changed
Drag
```

Inspector 中显示：

```text
Events
├── On Click
│   ├── Target
│   ├── Function
│   └── Arguments
├── On Hover
└── On Focus
```

## 11.2 ViewModel 和 Binding

建议引入：

```text
Game Data
    ↓
ViewModel
    ↓
Binding
    ↓
Widget Property
```

示例：

```text
Player.Health
    ↓
HudViewModel.HealthPercent
    ↓
ProgressBar.Value
```

第一版只需支持简单属性绑定：

```text
Text.text       ← Player.Name
Image.visible   ← Inventory.HasItem
Progress.value  ← Player.HealthPercent
```

---

## 12. Play/Stop 和预览优化

现有 PlaySession 的编辑文档与运行时副本隔离方案应保留。

## 12.1 保存和恢复编辑上下文

Play 前保存：

- 当前选中对象。
- 多选集合。
- Inspector 锁定状态。
- Hierarchy 展开状态。
- Scene View pan/zoom。
- 当前编辑工具。
- 当前文档标签。
- 预览分辨率。

Stop 后恢复，而不是简单清空选择。

## 12.2 Scene/Game 双视图

建议支持：

```text
Scene
Game
Split
```

Split 模式用于一边编辑布局，一边实时观察运行效果。

## 12.3 运行时差异查看

Play 结束后提供：

```text
Discard Runtime Changes
Apply Selected Changes
Apply All Changes
```

可以显示：

```text
Runtime Changes
├── Player.Position
├── Enemy.Health
├── Score.Value
└── Spawned Entity
```

默认仍采用丢弃运行时修改的安全策略。

## 12.4 结构化预览诊断

建议建立错误类型：

```rust
enum PreviewError {
    SceneLoad,
    ScriptRuntime,
    AssetMissing,
    Render,
    Input,
    Layout,
}
```

Game View 中按类型显示：

```text
[Layout] ButtonGroup exceeds parent width
[Asset] Missing texture: ui/menu.png
[Script] OnClick handler not found
```

---

## 13. 结构化组件和序列化演进

当前组件以 `BTreeMap<String, String>` 保存，短期可以保留，但应逐步引入统一 Schema。

### 13.1 第一阶段：保留旧格式

增加：

- 字段类型定义。
- 默认值。
- 统一解析器。
- 统一序列化器。
- 统一验证器。
- 错误位置和错误信息。
- 旧格式兼容。

优先迁移：

- Transform。
- UiLayout。
- Text。
- Image。
- Button/Event。

### 13.2 第二阶段：结构化场景组件

目标模型：

```text
Entity
├── Component Type
├── Version
├── Typed Properties
├── References
└── Validation State
```

组件迁移需要支持版本号和旧数据升级，避免一次性破坏已有场景。

---

## 14. 分阶段实施路线

## 阶段 1：安全和基础体验修复

目标：不改变场景格式，快速提升可靠性。

任务：

1. 场景切换未保存确认。
2. Undo/Redo 后统一刷新状态。
3. 统一 SelectionModel。
4. 修复 Hierarchy 过滤。
5. 文本输入焦点处理。
6. Scene View 拖拽一次提交。
7. Inspector 编辑结束后提交和保存。
8. Stop 恢复选择和视图状态。
9. 工具栏增加文字标签和快捷键提示。

重点文件：

```text
engine/crates/editor/src/main.rs
engine/crates/editor/src/actions/（mod.rs + 9 个命令模块）
engine/crates/editor/src/history.rs
engine/crates/editor/src/panels/hierarchy.rs
engine/crates/editor/src/panels/scene_view/（mod.rs + 10 个模块）
engine/crates/editor/src/panels/inspector.rs
```

## 阶段 2：Scene View 和 Inspector 重构

目标：提升日常编辑效率。

任务：

1. 拆分 Scene View 输入和绘制。
2. 引入 EditTransaction。
3. 增强多选操作。
4. 实现 Mixed Value。
5. 建立 Inspector 组件注册表。
6. 统一 Transform 编辑器。
7. Hierarchy 拖拽重挂载。
8. 对齐、分布、聚焦、复制粘贴完善化。
9. 增加对象上下文菜单。
10. 增加快捷键系统。

## 阶段 3：UI Widget Designer

目标：建立接近 Unreal UMG 和 Godot 的 UI 编辑工作流。

任务：

1. 独立 Widget 资产。
2. Canvas 根节点。
3. Widget Designer 模式。
4. Palette 面板。
5. Anchor + Offset。
6. VBox/HBox/Grid/Margin/Panel。
7. Safe Area。
8. 多分辨率预览。
9. UI 专用命中测试。
10. UI Prefab/Widget 复用。

## 阶段 4：结构化组件和数据绑定

目标：解决字符串组件的长期维护问题。

任务：

1. Component Schema。
2. 结构化序列化。
3. 字段类型和验证。
4. 组件迁移版本。
5. 结构化 UI 事件。
6. ViewModel。
7. Property Binding。
8. 运行时诊断。

## 阶段 5：性能和高级能力

目标：支持大型项目。

任务：

1. Transform 和 Property 使用 Patch History。
2. 大场景局部更新。
3. UI 布局缓存。
4. GPU 纹理池。
5. 可视化性能分析。
6. UI 动画时间轴。
7. 状态机预览。
8. 多窗口 Inspector。
9. 编辑器插件扩展。

---

## 15. 验收指标

### 数据安全

- 切换未保存场景时 100% 弹出确认。
- Undo/Redo 后 Inspector、Hierarchy 和画布状态一致。
- 资源删除具备确认和恢复机制。
- Play/Stop 不丢失编辑上下文。

### 操作效率

- 常用操作可通过快捷键完成。
- 一次拖拽只产生一条 Undo。
- Inspector 数值拖动期间不持续写磁盘。
- 多选对象可以批量修改共有属性。
- Hierarchy 搜索可以找到任意深度的节点。

### UI 布局

- UI 支持 Anchor + Offset。
- UI 支持至少 5 种 Container。
- 支持 16:9、21:9、4:3、9:16 预览。
- 编辑器和运行时使用同一套布局计算逻辑。
- Container 控制的子节点不会出现无提示的无效编辑。

### 性能

- Inspector 普通修改不触发每帧保存。
- Scene View 拖拽无明显卡顿。
- 大场景 Undo 内存增长可控。
- 调整预览分辨率时 GPU 资源不会频繁抖动。
- UI 布局修改后只重算受影响的子树。

---

## 16. 推荐执行顺序

```text
未保存保护
    ↓
统一 SelectionModel
    ↓
Undo/Redo 和缓存同步
    ↓
显式编辑事务
    ↓
Scene View / Inspector 重构
    ↓
Canvas + Anchor/Offset
    ↓
Container 自动布局
    ↓
独立 Widget Designer
    ↓
结构化组件 Schema
    ↓
事件绑定和 ViewModel
    ↓
动画、性能和插件扩展
```

不建议一开始重写整个 UI 系统。推荐保留现有 `SceneDocument`、Prefab、RuntimeScene 和 PlaySession，先解决数据安全、状态同步和编辑事务，再逐步引入新的 UI 布局模型。

最优先执行的五项：

1. 未保存场景切换保护。
2. 统一 SelectionModel。
3. 拖拽和 Inspector 的显式事务提交。
4. Undo/Redo 后统一刷新所有编辑器状态。
5. 引入 Canvas + Anchor/Offset + 基础 Container。

---

## 17. 参考资料

- [Unity Scene View](https://docs.unity.com/en-us/unity-studio/develop/ui-refs/scene-view)
- [Unity Editing in Prefab Mode](https://docs.unity3d.com/2021.1/Documentation/Manual/EditingInPrefabMode.html)
- [Unreal Widget Blueprints](https://dev.epicgames.com/documentation/en-us/unreal-engine/widget-blueprints-in-umg-for-unreal-engine)
- [Unreal UMG ViewModel](https://dev.epicgames.com/documentation/fr-fr/unreal-engine/umg-viewmodel-for-unreal-engine)
- [Godot Containers](https://godot-ko.readthedocs.io/ko/4.x/_sources/tutorials/ui/gui_containers.rst.txt)
- [Godot Control](https://trinovantes.github.io/godot-docs/classes/class_control)
