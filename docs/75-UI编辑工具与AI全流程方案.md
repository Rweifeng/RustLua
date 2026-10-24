# UI 编辑工具与 AI 全流程方案（M76）

> **一句话状态**：设计定稿；M0 契约层已交付（`rustlua-ui-spec` 接入 workspace，66 项测试绿，
> 8 条 UI tool 规格进快照门，Provider 录制/回放可用）。M1–M6 尚未开工，计划包见
> [M76-00 总览](../tasks/done/M76-00-UI编辑器与AI全流程-总览与依赖顺序.md)。
>
> **本文回答什么**：主流引擎与 UI 工具各自怎么做、RustLua 抄哪条丢哪条、如何把「设计 → 出图 → 切图 →
> 布局 → 修改」做成编辑器内的一条 AI 主链路，以及这条链路上每个产物的契约、校验、权限与验收门。
>
> **本文不回答什么**：不承诺日历排期（只给相对顺序与人周量级）；不绑定任何模型/图像供应商；
> 不做游戏内运行时推理（沿用 [docs/71](71-AI融合集成编辑器开发方案.md) §3 的结论）；
> 不把 UI 做成完整的 USS/CSS 级联引擎。

---

## 1. 结论先行

1. **UI 的第一性产物是 `UiSpec`（JSON 文档），不是 `.scene` 里的组件串。** 编辑器与 AI 都改 Spec，
   Spec 经确定性编译器落到现有 ECS 组件；运行时不认识 Spec，也不依赖 AI。
2. **AI 的写权限收敛为「提案」**：AI 只能产出 `UiSpec` / `UiPatch` / `AssetPlan`，由校验器与人工闸门放行；
   任何落盘都要带 `base_revision`、diff、预览图与成本，一次接受 = 一条 Undo。
3. **样式系统采用「设计令牌 + 样式类 + 状态覆盖」**，不做选择器级联。令牌是唯一数值源，类只组合令牌，
   状态只覆盖有限属性。
4. **运行时先补齐闭环，再谈工作区**：当前运行时只分发 `click`（`engine/crates/rustlua-ui/src/ui_events.rs:142`），
   `UiAnimation` 只解析不应用（`engine/crates/rustlua-scripting/src/script/scene_spawn.rs:532`），
   `UiBindings` 与 `ButtonStates` 没有消费方。UI Studio 若先于这些落地，只能生成"看得见、点不动"的界面。
5. **交付形态是编辑器内的 UI Studio 工作区**（复用 egui/Scene Widget、History、PreviewSession，见
   `engine/crates/editor/src/dock.rs:33`），不另起外部进程编辑器。
6. **三宿主同步验收**：PC / Web / 微信每阶段都跑同一批 golden（布局矩形、绘制顺序、交互事件序列），
   不以「桌面绿」代替另外两端。
7. **内部 AI 为主，外部 MCP 为辅**：UI Studio 内的 Copilot 走**进程内 tool registry 直调**（内部 API，
   零序列化跳数、可直接读编辑器状态、默认主入口）；外部客户端（Qoder/Claude/Cursor）经 MCP 适配层
   访问**同一个 registry**。两条通道共用 tier、确认网关、事务/Undo、审计与回放，不允许出现第二套实现。

---

## 2. 市场对标：三条技术路线与可借鉴点

### 2.1 三条路线

| 路线 | 代表 | 数据模型 | 优点 | 代价 |
|---|---|---|---|---|
| A. 场景对象 + 锚点 | Unity UGUI、Cocos Creator、Defold | 节点树 + RectTransform/Widget | 直接对应运行时节点，所见即所得 | 复用靠 Prefab/变体，样式散落在实例上 |
| B. 文档 + 样式表 | Unreal UMG + Slate、Godot Control + Theme | 控件树 + 样式资源/主题 | 主题与状态覆盖强，换肤容易 | 需要样式解析与继承规则，调试链路长 |
| C. 独立编辑器 + 数据包 | FairyGUI、FGUI/Unity、Egret EUI | 包/组件/元数据，导出运行时包 | 设计-程序边界清晰，分包与多语言成熟 | 独立工具链、导出链路与版本兼容成本 |

RustLua 现状：数据层接近 A（`rustlua-ui` 的锚点/容器/两遍布局），编辑器有 schema Inspector 与
History，但缺独立 UI 工作区与样式层。**选 A 的运行时 + B 的样式表达 + C 的打包/组件边界**。

补充参照 **LayaAir 3.0**：它把 AI 素材/草稿直接生成 UI（画布 → 控件树 → 资源），强项是“生成结果
本身就是编辑器里可继续编辑的结构”，而不是一张截图。本文借用这条原则，但落点是 `UiSpec` 而非
专有格式；Unity AI 的 Figma→UI、检查点回滚、生成资产标记与权限确认，分别对应本文 §4.1、§6.3、
§9 与 §6.4。

### 2.2 值得抄的十点

1. **锚点 + 容器两套布局并存**（UGUI/Godot）：自由摆放用锚点，成组排列用 VBox/HBox/Grid；两者由
   `LayoutItem` 统一成约束求解，不互相排斥。
2. **控件 = 逻辑 + 视觉 + 状态**（UGUI Selectable / Godot Theme）：状态只覆盖可枚举属性，不做任意层叠。
3. **Prefab/变体作为复用单位**（Unity Prefab Variant / Godot Scene inherit）：UI 组件嵌套必须可覆盖局部字段。
4. **九宫格与图集是一等公民**（FairyGUI）：切图产物直接声明 `nine_slice` 与 `atlas#region`，避免运行期拉伸失真。
5. **设计令牌先行**（Figma Variables / Material Tokens）：颜色、字号、间距、圆角、动效时长全部具名。
6. **UI 也要有时间线**（UMG Animation / FairyGUI Transition）：入场、状态切换、提示气泡用同一套关键帧模型。
7. **数据绑定显式化**（UGUI Text + ViewModel、FGUI Controller）：绑定是文档里的声明，不是隐藏在脚本里的赋值。
8. **预览与真机同源**（Unity Device Simulator / Cocos Preview）：预览必须跑真实布局与真实宿主适配，不做"近似画布"。
9. **AI 走工具面而不是自由文本**（Unity MCP / Unity-MCP）：模型只调用结构化 tool，编辑器负责校验与回滚。
10. **生成资产必须带来源**（Unity Generators / 各家水印与许可）：prompt、模型、种子、许可、审核状态缺一不可。

### 2.3 明确不抄的四点

1. **不做完整 USS/CSS 级联与选择器**：收益低、调试成本高，且与确定性编译器冲突。
2. **不做独立外部编辑器为主**：第二套渲染/撤销/资源系统，维护成本翻倍。
3. **不让 AI 直接写 `.scene`、组件字符串或 Lua**：无法稳定校验，也无法保证一次 Undo 可回滚。
4. **不做运行时 LLM 推理**：wasm/微信体积与延迟不可接受，沿用 M75 的既定结论。

---

## 3. 总体架构

```
            ┌──────────────────────────── UI Studio（编辑器内，主入口） ───────────────────┐
            │  画布(Scene Widget) │ 层级 │ 检查器 │ 样式/令牌 │ 资源/切图 │ 提案箱 │ 预览   │
            └───────────────▲──────────────────────────────▲──────────────────────────────┘
                            │ 编辑/预览                     │ 提案审阅、接受/拒绝
                     ┌──────┴───────┐              ┌────────┴─────────┐
                     │ UiSpec 文档  │◄────────────►│ UiProposal       │
                     │ (JSON, 版本) │              │ diff+preview+cost│
                     └──────┬───────┘              └────────▲─────────┘
                            │ 确定性编译                          │
                     ┌──────▼───────┐              ┌────────┴─────────┐
                     │ SceneCompiler│              │ 内部 AI Copilot  │
                     │ → EditorCmd  │              │ Llm/Vision/Image │
                     └──────┬───────┘              │ Provider 抽象    │
                            │ 事务+Undo             └────────▲─────────┘
                     ┌──────▼───────┐                       │ 内部 API（进程内直调）
                     │ ECS 组件     │              ┌────────┴─────────┐
                     │ (.scene 落盘)│              │ UiToolRegistry   │
                     └──────────────┘              │ 唯一能力层       │
                                                   └────────▲─────────┘
                                                            │ MCP 适配（JSON-RPC）
                                                   ┌────────┴─────────┐
                                                   │ 外部 AI 客户端   │
                                                   └──────────────────┘
```

**双通道规则**：

- **内部 API 是主入口**：`UiToolRegistry` 以 Rust trait/函数直接暴露给 Copilot 与 UI Studio 面板，
  不经 JSON-RPC，不复制状态；推理结果直接进提案箱。
- **MCP 是同一 registry 的适配层**：只做参数/结果编解码与传输，tier、确认、事务、审计、回放全部下沉到
  registry，不允许 MCP 侧另写业务逻辑。
- 两条通道的**工具清单、tier、诊断文案**由同一份规格生成；快照门只认这一份。

**边界规则**：

- `rustlua-ui-spec` 只依赖 `serde`/`serde_json`，不依赖 editor、egui、运行时（现状即如此）。
- 编译器是 `rustlua-ui-spec` 的纯函数 + 编辑器侧适配层，输入 Spec/能力矩阵，输出命令列表；同输入必须同输出。
- AI Provider 只负责「产出结构化内容」，不持有编辑器状态；所有落盘经提案与事务。
- 运行时只消费编译产物（现有组件 + 新增组件），不读 Spec 文件。

### 3.1 四个核心契约

| 契约 | 作用 | 关键字段 | 现状 |
|---|---|---|---|
| `UiSpec` | 可编辑、可版本化的 UI 文档 | `spec_version`、`document`、`tokens`、`styles`、`root`、`assets`、`bindings` | 已定义于 `engine/crates/rustlua-ui-spec/src/spec.rs`，22 项单测绿 |
| `UiPatch` | AI 的增量修改，避免全量重写 | `base_revision`、`ops[]`（Replace/Insert/Remove/SetEvent/SetStyle） | `.../src/patch.rs` |
| `UiProposal` | 人工审阅单位 | `proposal_id`、`kind`、`base_revision`、`diagnostics`、`diff`、`preview_png`、`cost`、`expires_at` | `.../src/proposal.rs` |
| `AssetProvenance` | 生成资产的可追溯记录 | `asset_id`、`source`、`provider`、`model`、`prompt_hash`、`seed`、`license`、`reviewed`、`used_by` | `.../src/asset.rs` |

### 3.2 引用语法

- **令牌**：`@color.primary`、`@space.md`、`@font.title.size`。只允许 `@` 开头的两级以上路径；未知令牌是 error。
- **资产**：`asset:<key>` 指向 `assets[]`；图集区域写 `atlas#region`。绝对路径、`..`、反斜杠、盘符一律拒绝。
- **补丁路径**：JSON Pointer 风格，根字段白名单（`document`/`tokens`/`styles`/`root`/`assets`/`bindings`），
  节点内只允许 `layout.*` 与 `style`、`events.*` 等白名单字段。

### 3.3 能力矩阵（诚实边界）

`Capabilities::core()` 只声明今天运行时真的支持的能力：5 种节点、`click`、无状态样式、无绑定/裁剪/富文本/
虚拟列表。`Capabilities::planned()` 是目标态。**校验器以传入的能力为准**：AI 产出目标态字段时，
在 M1/M4 未落地前会被稳定拒绝，而不是"看起来能跑"。

---

## 4. 全流程六段

### 4.1 设计（Layout / Structure）

- 入口：Copilot 输入自然语言、参考图/草稿图、Figma 导出 JSON（M5）。
- 产物：`UiSpec` 骨架 + `diagnostics` + 线框预览。
- 规则：先结构后视觉；容器优先于绝对定位；每个节点必须有稳定 `key`（编译器据此生成 `UiKey`）。
- 验收：`validate_spec` 0 error；同 prompt + 同 seed 的两次生成结构等价。

### 4.2 出图（Image Generation）

- 入口：`AssetPlan`（角色、尺寸、风格令牌、九宫格需求、预算）。
- 通道：`ImageProvider` 抽象（本地/远程/MCP 均可），密钥只从环境变量或系统密钥库读取。
- 产物：候选图 + `AssetProvenance`（provider/model/prompt_hash/seed/license/reviewed）。
- 规则：先生成、后审阅、再入库；未审阅资产不得被 Spec 引用为发布资源（警告 + 发布门拦截）。

### 4.3 切图（Slice / Atlas / 9-slice）

- 入口：已审阅的源图 + 切图预案（等分/透明边界/网格/手动框）。
- 产物：区域清单、九宫格边距、`atlas#region` 引用、体积报告。
- 规则：九宫格边距之和必须小于对应尺寸；切图结果写回 `assets[]`，不直接改节点。
- 验收：图集打包可复现（同输入同字节序），运行时贴图引用全部可解析。

### 4.4 布局（Layout Compile）

- 入口：`UiSpec` + 能力矩阵 + 目标分辨率/安全区。
- 产物：`EditorCommand` 批次（一次事务）、布局矩形快照、三宿主预览。
- 规则：编译必须确定性；布局求解复用运行时同源的锚点/容器算法；禁止在编译期塞运行时未支持的属性。
- 验收：Spec → 场景 → 回读 Spec 的语义等价；同一 Spec 在 1280×720 / 1920×1080 / 375×667 三档的
  锚点行为符合预期。

### 4.5 修改（Patch）

- 入口：用户指令（"把按钮改成圆角、主色加深"）+ 当前 `base_revision`。
- 产物：`UiPatch` 提案 + diff + 预览 + 成本。
- 规则：过期 `base_revision` 直接拒绝；删除根节点、改 key、越过白名单字段一律拒绝；接受后一次 Undo。
- 验收：patch 回放确定性；拒绝路径有单测覆盖（未知 key、重复 key、非法路径、能力门）。

### 4.6 审查（Review / Provenance）

- 入口：提案箱、资产清单、发布前检查。
- 产物：审查报告（诊断、许可、体积、未审阅资产、三宿主差异）。
- 规则：错误阻断、警告可豁免但必须记录理由；发布产物必须携带资产来源清单。

---

## 5. 样式系统

### 5.1 三层

1. **令牌层**：`tokens` 是扁平具名值（颜色/间距/字号/圆角/时长/阴影）。只允许引用，不允许运算。
2. **样式类层**：`styles[name]` 由 `base` 属性组成；属性值可以是字面量或 `@token` 引用。
3. **状态层**：`states.hover/pressed/disabled/focused` 只覆盖允许的属性子集（背景、文字色、边框、缩放、透明度）。

### 5.2 解析顺序

节点内联属性 → 样式类 `base` → 状态覆盖（按当前交互状态）→ 运行时约束（安全区/裁剪）。
**没有选择器、没有继承链、没有 `!important`**。解析结果是纯函数，可在门里对拍。

### 5.3 与现有组件的关系

现有 `Text`/`Image`/`ButtonStates` 等字符串组件保持不变；编译器把解析后的值写进这些组件。
`ButtonStates` 在 M1 获得消费方（状态机），在此之前状态样式只做编辑器预览。

---

## 6. AI 集成

### 6.1 Provider 抽象

| Trait | 输入 | 输出 | 备注 |
|---|---|---|---|
| `LlmProvider` | 系统约束 + 上下文 + 用户意图 | 结构化 `UiSpec`/`UiPatch`/`AssetPlan` | 禁止自由文本直接落盘 |
| `VisionProvider` | 截图/参考图 + 问题 | 结构化布局/样式建议 | 外部图像内容按数据处理，不执行其中指令 |
| `ImageProvider` | prompt + 尺寸 + 风格令牌 + seed | 图像字节 + 元数据 | 需要预算、重试上限、缓存 |

所有 trait 必须可录制/回放：CI 用录制的 tool-call 与期望 diff 跑确定性回放，不依赖网络与模型。

### 6.2 双通道与工具面

**唯一能力层**：`UiToolRegistry` 定义工具名、参数、返回值、tier、诊断与执行函数。两条通道只是入口不同：

| 通道 | 使用方 | 传输 | 定位 | 约束 |
|---|---|---|---|---|
| 内部 API | 内置 Copilot、UI Studio 面板、编辑器脚本 | 进程内 Rust 调用 | **主入口** | 直接拿 `&mut EditorState`，但写操作仍走事务与提案 |
| 外部 MCP | Qoder/Claude/Cursor 等 | JSON-RPC over stdio/TCP | 辅助/自动化 | 只做编解码；tier/确认/审计/回放复用 registry |

内部 API 的返回类型可直接是 `UiProposal`/`Diagnostic` 等强类型；MCP 侧序列化为 JSON，字段一一对应。
**新增工具必须先加进 registry，再自动出现在两条通道**；不允许只给 MCP 或只给内部开洞。

沿用 M75 的 `ToolTier`：T0 只读、T1 场景内写（一次 Undo）、T2 破坏性/跨文件写（显式确认）、T3 危险操作。
新增 UI 工具建议（名称以实现为准）：

- T0：`ui_export_spec`、`ui_validate_spec`、`ui_preview_spec`、`ui_list_assets`、`ui_asset_census`
- T1：`ui_apply_spec`、`ui_apply_patch`、`ui_set_tokens`、`ui_set_styles`
- T2：`ui_promote_asset`、`ui_write_asset_manifest`、`ui_delete_ui_document`
- 出图/切图：`ui_plan_assets`（T0）、`ui_generate_assets`（T2，逐项确认与预算）、`ui_slice_asset`（T1 元数据，T2 落盘）

**M0 已登记 8 条**（其余按 M76-03/04/05 逐批登记）：
`ui_export_spec`/`ui_validate_spec`/`ui_preview_spec`/`ui_list_assets`（T0）、
`ui_apply_spec`/`ui_apply_patch`（T1）、`ui_promote_asset`/`ui_generate_assets`（T2）。
规格在 `engine/crates/rustlua-editor-ai/src/surface.rs`，强类型内部 API 在 `src/ui_tools.rs`
（`UiToolRegistry` trait），两者共用同一组工具名常量。**执行体在 M76-03**：在此之前调用会返回
`Unavailable`，不会静默成功，也不存在第二套写路径。

工具面快照门沿用 `ai_tool_surface.json` 的刷新方式：
`AI_TOOL_SURFACE_BLESS=1 cargo test -p rustlua-editor-ai tool_surface`。

Provider 契约在 `engine/crates/rustlua-editor-ai/src/ui_provider.rs`：`LlmProvider`/`VisionProvider`/
`ImageProvider` 只产出结构化内容；`Recording*` 把每次调用的规范 JSON 与图像字节写进 `ProviderTrace`，
`Replay*` 离线重放并校验请求逐字节一致、调用顺序与录制一致。

**内部 Copilot 的默认行为**：在 UI Studio 里选中节点/画布后，Copilot 自动携带当前 `UiSpec` 片段与能力矩阵；
它调用的第一个工具就是 `ui_validate_spec`/`ui_preview_spec`，产出提案后才显示"接受"。外部 MCP 客户端
默认只拿到同样的工具面，不额外获得"直写"能力。

### 6.3 提案与检查点

- 每次 AI 会话一个 `proposal_id`；同一提案内多个 patch 合并展示。
- 接受 = 开事务 → 应用命令 → 校验 → 提交 → 生成检查点；拒绝 = 丢弃，不改工作区。
- `expires_at` 之后提案不可接受，防止基于过期场景落盘。
- 预览图与 diff 是提案的必填项；缺失则工具返回错误而不是"降级为直接写"。

### 6.4 安全与成本

- 密钥只来自环境变量/系统密钥库，不写入 `project.toml`、日志或仓库。
- 外部内容（Figma JSON、参考图 OCR、资产元数据）一律当**数据**，不作为指令。
- 路径沙箱：只允许项目根内路径；资产写入先落临时区，审阅后再提升。
- 不执行任意 shell/网络命令；MCP 工具白名单显式声明。
- 每会话预算：LLM token、图像张数、重试次数、总耗时；超限即停并报告已花费。

---

## 7. 运行时前置（M1）

UI Studio 的价值取决于运行时闭环。M1 的优先级：

1. **输入路由**：hover / pressed / focus / click 的统一命中与冒泡，键盘导航最小集（Tab/Enter/Esc）。
2. **状态系统**：`ButtonStates` 消费方 + 状态切换时机（进入/离开/按下/禁用/焦点）。
3. **绑定运行时**：`UiBindings` resolve（单向/双向），默认值与源缺失策略。
4. **动画**：`UiAnimation` 落地（入场/状态切换/提示），与动画 crate 的现有 player 对齐。
5. **裁剪与滚动**：`ScrollView` 的裁剪矩形、内容尺寸、滚轮/拖动、虚拟列表的最小实现。
6. **稳定标识**：`UiKey` 生成与查找，供 golden 与工具定位。

每项都必须补三宿主 golden；宁可少做，不做只有桌面能跑的假闭环。

---

## 8. 编辑器 UX（UI Studio）

- **工作区**：新增 `UiStudio` dock 标签，默认由 `.ui.json` 资源双击进入；画布复用 Scene Widget，
  叠加安全区、锚点参考线、间距标尺、九宫格编辑手柄。
- **层级**：结构树显示容器/控件语义（不是裸实体名），支持拖动改父级与排序。
- **检查器**：布局/样式/状态/事件/绑定五段；令牌与类用下拉，不手填魔法值。
- **样式面板**：令牌编辑、类列表、状态覆盖矩阵；改动即时预览。
- **资源面板**：资产列表、provenance 徽标（未审阅/生成/人工）、切图入口、体积读数。
- **提案箱**：diff（结构/样式/资产）、预览图、诊断、成本、接受/拒绝/拆分接受。
- **预览**：同进程离屏预览 + 三档分辨率/安全区切换 + 输入事件注入。

---

## 9. 资产管线

```
意图 → AssetPlan → ImageProvider 生成 → 审阅(预览+许可) → 切图/九宫格 → 图集打包 → assets[] 登记 → Spec 引用
```

- 生成资产必须带 `AssetProvenance`；缺 `license` 警告，未 `reviewed` 警告，缺 provider/model/prompt_hash 错误。
- 体积门：单图/图集/总包上限写进计划，超限阻断发布而不是只提示。
- 资产键稳定：重生成不得改变 `key`，否则引用断裂；改为新 key + 迁移提案。
- 版权：`license` 必填枚举 + 自由文本备注；发布清单包含全部生成资产来源。

---

## 10. 三宿主一致性与验收门

| 门 | 量什么 | 通过标准 |
|---|---|---|
| 契约单测 | Spec/Patch/Proposal/Provenance 序列化与校验 | `cargo test -p rustlua-ui-spec` 22 项全绿（2026-10-24） |
| 编译确定性 | 同 Spec 两次编译命令序列 | 字节等价 |
| 布局 golden | 三档分辨率下节点矩形 | PC/Web/微信逐字段一致（误差 0） |
| 绘制顺序 | 重叠层级的绘制序列 | 三宿主一致 |
| 交互序列 | 注入 hover/press/click | 事件与状态序列一致 |
| 回放门 | 录制 tool-call → 期望 diff | 不联网、稳定通过 |
| Provider 回放 | 录制 Llm/Vision/Image 请求与输出 | 请求逐字节一致、顺序一致、无网络（`ui_provider` 5 项单测） |
| UI 工具面快照 | 8 条 UI tool 的名称/tier/schema | `ai_tool_surface.json` 逐字节一致；篡改后必红（2026-10-24 已对照） |
| 双通道一致性 | 同一请求分别走内部 API 与 MCP 适配 | 工具名、tier、诊断、最终 diff 完全一致 |
| 内部 API 契约 | Copilot 直调 registry 的强类型返回 | 与 MCP JSON 字段一一对应，无第二套逻辑 |
| 资产门 | provenance、许可、体积 | 0 error；警告有记录 |
| 文档门 | 新文档编号/索引/链接 | `scripts/verify/check-docs.ps1` 绿 |

**反面对照**：每个新门必须跑一次"故意破坏"验证它会红，否则视为假绿。

---

## 11. 路线图（M0–M6）

| 里程碑 | 内容 | 依赖 | 量级 |
|---|---|---|---|
| M0 契约层（已交付） | `rustlua-ui-spec` 接入 workspace；校验器；能力矩阵；Provider trait；UI tool 面契约与快照门 | — | 1–2 周 |
| M1 运行时闭环 | 输入路由/状态、绑定、动画、裁剪/滚动、UiKey + 三宿主 golden | M0 | 3–4 周 |
| M2 Spec/编译器/工具 | Spec→`EditorCommand` 编译器、`ui_*` 工具、提案/检查点、回放门 | M0 | 2–3 周 |
| M3 UI Studio + Copilot | 工作区、样式/令牌面板、提案箱、预览与输入注入 | M1、M2 | 3–4 周 |
| M4 资产 AI | ImageProvider、生成审阅、切图/九宫格/图集、provenance、体积门 | M0、M3 | 3–4 周 |
| M5 修改/导入/审查 | Patch 全链路、Figma/截图导入、审查报告 | M2、M3 | 2 周 |
| M6 高级控件与三端 | 虚拟列表、富文本、输入控件、导航栈、三宿主终验 | M1–M5 | 2–3 周 |

合计约 15–17 周（与方案 v1 的 MVP-C 估算一致）。**顺序不可压缩的是 M0→M1/M2→M3**；
M4 可与 M3 后半并行，M5 可与 M4 并行，M6 必须最后。

**M0–M6 的逐包验收与文件清单**见 `tasks/done/M76-01` 至 `M76-07`。

---

## 12. 风险与缓解

| 风险 | 表现 | 缓解 |
|---|---|---|
| 编译器与运行时布局算法漂移 | 编辑器预览与真机不一致 | 编译器复用运行时布局 crate；golden 对拍同一批 Spec |
| 样式系统膨胀成 CSS | 规则复杂、无法确定性回放 | 三层模型 + 属性白名单 + 校验器拒绝未知字段 |
| AI 提案不可复现 | 同 prompt 结果漂移 | 录制回放门；生成参数与种子落 provenance |
| 资产许可风险 | 发布物来源不明 | provenance 必填 + 发布清单 + 未审阅阻断 |
| 运行时改动回归 | 现有 UI 屏破坏 | 先补 golden 再改；一批一提交，三宿主跑门 |
| 工作区拖垮编辑器性能 | 大树/大图时卡顿 | 虚拟化树、增量布局、PreviewSession 离屏 |
| 成本失控 | 反复重生成 | 预算 + 缓存 + 逐项确认 + 成本进提案 |

---

## 13. 不做什么（边界）

- 不做完整 CSS/USS 级联、选择器、伪类表达式。
- 不做独立外部 UI 编辑器（不维护第二套渲染/撤销）。
- 不让 AI 直接写 `.scene`、组件字符串、Lua 或任意文件。
- 不做游戏内运行时 LLM 推理。
- 不在 M1 之前给 UI Studio 承诺"能用的交互"——先闭环，再工作区。
- 不引入依赖运行时未支持能力的数据字段（No-Dead-Data）。

---

## 14. 实施台账

| 日期 | 内容 | 位置 |
|---|---|---|
| 2026-10-24 | `rustlua-ui-spec` 契约 crate 起稿：spec/capabilities/asset/patch/proposal/validate | `engine/crates/rustlua-ui-spec/src/` |
| 2026-10-24 | 设计定稿与 M76 计划批次 | 本文 + `tasks/plan/M76-00..07` |
| 2026-10-24 | M0 收口：能力矩阵可序列化、Provider 抽象与录制回放、8 条 UI tool 规格进快照门 | ui-spec 22 passed；editor-ai 44 passed；快照篡改反面对照 exit 101→恢复 0；`cargo build -p editor` 42.44s；`check-docs.ps1` 绿 |
| 2026-10-24 | M1–M6 交付面：运行时输入路由/`ButtonStates`/绑定/动画/裁剪滚动、`ui_*` 执行体与提案门、UI Studio（层级拖拽·检查器写回·画布缩放·令牌·提案箱拆分接受·Undo/Redo）、出图/切图/provenance/工程级发布门、补丁与导入审查、四个高级控件内核 + `ui.*` 运行期接线 | `cargo test -p rustlua-ui-spec` 59/0、`-p rustlua-editor-ai` 65/0(+1 ignored)、`-p rustlua-ui` 132/0、`cargo test -p editor --lib` 249/0；`cargo test -p editor --bin editor ui_` 36/1（那 1 条是并行批次的 `index.scene`/`api_prefab.scene` 基线漂移，未刷基线） |
| 2026-10-24 | MCP stdio 活体接入 `examples/game_api`（连接器拉起 `editor --project <路径> --mcp-stdio`）：43 tool / 9 条 `ui_*`、tier 分布 T0=14 T1=15 T2=8 T3=6；T0 读通且 revision 原样往返、T1 写生成提案、T2/T3 停在「AI 动作确认」。修掉两条只有真实 JS 客户端才能暴露的缺陷：revision 是 u64 内容哈希、超出 2^53 会被 f64 舍入导致补丁永远判过期（`revision_hash` 压进安全位宽 + 单测）；一条未确认的写会把整条 stdio 通道连同后续只读调用一起挂死（`DEFAULT_REPLY_WAIT` 有界等待 + `QueuedForConfirmation`，批准后动作照常执行） | 读数：`ui_export_spec` 4ms → `revision=4421840451047808`（2^53=9007199254740992）；`ui_apply_patch`（T1，`RUSTLUA_EDITOR_AI_AUTO_T1=1`）25ms → 提案 `uip_793631c4d8a22f71`、`nodes_changed=1`；同一次会话里 `ui_promote_asset`（T2）30001ms → 「已排队」，随后 T0 仍 3ms 应答；不带该环境变量时 T1/T2/T3 四条写全部排队、只读照常——门读数由 `bridge::tests::interactive_policy_queues_the_write_until_the_host_approves_it` 与 `unconfirmed_write_times_out_as_queued_instead_of_wedging_the_transport` 锁住 |
| 2026-10-24 | **入库**：M76 与 M77 一笔带走 `c247fe99`（233 个文件、30248 插入 / 2677 删除），并行批次在途内容（game_api 2.0 演示、`docs/74`、assetc 拆分、`scene_shot.rs`、M78-00）全部排除 | 门读数：`check-docs.ps1` → `docs=69 game_docs=46 h1=117 links=513`；`verify.ps1 -MvpOnly` → `executed=78 expected=78`；`cargo test -p rustlua-ui-spec` 59、`-p rustlua-editor-ai` 65(+1 ignored)、`-p editor --lib` 249。pre-commit 的 workspace clippy `-D warnings` 两次拒笔，M76 文件里 7 个 lint 点逐个真修（没有 `-A`、没有 `--no-verify`）。提交态三条（`--all-targets` / GPU 单线程 138+153 / 反面对照 7×E0432）见 [M77-00 §归档记录](../tasks/done/M77-00-宿主侧模块化与渲染器下沉-总览与依赖顺序.md)。**没买到的**：`-Web`、`-Benchmarks` 两段没跑；三宿主 golden 与 `ui/hosts.json` 生产端仍缺 |
| 2026-10-24 | **归档**：`tasks/plan/M76-00..07` 与 `tasks/plan/M77-00`、`M77-02` 移入 `tasks/done/`，路径引用同笔更新 | 未收口项按「只登记，不开新包」保留在各包 §未完成项 |
| 2026-10-24 | 本批提交状态：M76 与并行的 M77（渲染器下沉，52 条已在索引里的重命名）以及 `examples/game_api` 的 Lua 迁移同处一个工作区，`engine/Cargo.toml`、`main.rs`、`dock.rs`、game_api 样例都被两边共同改动 | 因此**没有可单独编译的 M76-only 路径子集**，本批按路径切提交会同时带走别人的在途改动；收口先落在文档与本批门读数，提交集等 M77 落地后按 `tasks/done/M76-00`「提交集」的命令重建（当日即按此执行，见下两行） |

后续每批落地在本表追加一行：日期、提交、门读数（命令 + 结果），不写"已完成"四个字了事。
