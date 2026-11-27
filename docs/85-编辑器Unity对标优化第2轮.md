# 85 编辑器 Unity 对标优化（第 2 轮）

- 创建：2026-11-27（M87-00）· 基线 HEAD `7f35dd5f`
- 事实来源：对编辑器六个子系统各做一次**只读代码审计**（Hierarchy/选择、Inspector/组件 schema、
  Scene View/Gizmo、资源浏览器/导入、Console/Play/调试器、Shell/停靠/菜单/性能）。
  本文只保留能在本仓 `file:line` 复核的结论；每条缺口都注明"证据在哪"与状态。
- 与既有文档的分工：[55](55-编辑器商业引擎对标重构计划.md) 是目标架构与阶段路线，
  [58](58-编辑器重构阶段0-3收口与验收.md)/[57](57-编辑器重构进度与续跑交接.md) 是 0–3 阶段收口，
  [62](62-成熟引擎差距分析.md) §2.4 是"编辑器作为一个整体"与 Unity 的横向差距，
  [82](82-引擎能力缺口登记.md) 是**引擎侧**生产缺口。**本文是"编辑器交互层"的缺口台账 + 本轮落地**。
- 状态词：**已修（本批）** · **待办**（本轮只登记，下一批输入）· **设计不变式**（不是缺陷）。

## 1. 结论

1. 编辑器的**能力面**已经齐（Hierarchy/Inspector/Scene/Game/Console/Profiler/Prefab/UI Studio/
   动画/材质/Shading Graph/插件/MCP 工具面都在，且各有测试），本轮的差距集中在三处：
   **少动作的语义一致性**（多选、聚焦、跨面板定位）、**帧路径上的浪费**、**大信息量下的可读性**。
2. 最重的一条是性能：Console 的"虚拟滚动"只是注释里有——实现逐行建控件，
   10k 保留行时**每帧 120.96 ms**（p95），主线程直接冻结。这一批修掉了：
   **0.376 ms / 每帧 9 行**（同一台机器、同一 debug 档，−99.7%）；
   同批首轮 after 复跑是 0.529 ms，两次都在噪声内、量级一致。
3. 交互层修了三件 Unity 用户的肌肉记忆：**视口内 F 聚焦**（按包围盒适配）、
   **多选 Ctrl+D / 移动到根节点**（整批一条 Undo）、**Inspector 搜索框常驻 + 混合值可读**。
4. 剩下的缺口按"用户能否绕过"排序登记在 §5；其中**断点调试 UI** 与**层级揭示选中**是下一批的两颗大头。

## 2. 审计方法

- 六个子系统并行做只读审计，每个审计被要求：只报**能在代码/测试里复核**的缺口、给 `file:line`、
  给 Unity 对照、给成本档（S < 4h / M < 1d / L > 1d）、给"是否会破坏现有测试"。
- 本轮的新增门一律按"先补门 → 故意破坏生产代码看它判红 → 还原"的标准做（见 §4.1 的反面对照）。
- 性能读数用同一个无窗口入口 `cargo run -p editor -- --perf-baseline --perf-report <json>`，
  前后各跑一次全量 37 场景：`artifacts/metrics/m87-perf-before.json`（修前）、
  `m87-perf-after.json`（首轮修后）、`m87-perf-after-final.json`（最终复跑）。

## 3. 缺口台账

### 3.1 Hierarchy + 选择

| # | 缺口 | 证据 | Unity 对照 | 状态 |
|---|---|---|---|---|
| H1 | 多选 Ctrl+D 只复制主选中 | `hierarchy_ops.rs`（旧实现读 `primary`） | 复制整个选区 | **已修（本批）** |
| H2 | 多选「移动到根节点」只搬主选中 | `panel.rs` → `ReparentSelection` → `components.rs:reparent_selected`（旧实现只取 primary） | 整批移动 | **已修（本批）** |
| H3 | 从场景视图选中后，层级面板不展开祖先、不滚动 | 全仓无 hierarchy 的 `scroll_to_rect`；`collapsed` 只被用户动作改 | 双击/ping 自动展开居中 | **已修（M88）** |
| H4 | 锁定语义是反的：锁定的行在 Hierarchy 点不中，但删除/拖拽不受锁保护 | `panel.rs` 行按钮 `add_enabled(false)`；`components.rs` 删除无 locked 判断 | 锁只挡场景拾取 | **已修（M88）** |
| H5 | Ctrl+C/V 只搬一个实体 | `main.rs` 剪贴板 `Option<SceneEntity>`；`copy_selected_entity` 读 primary | 剪贴板含多选 | 待办 |
| H6 | 键盘无法起选、无法 Shift 扩选 | `navigation.rs` primary 为空即 return；`move_hierarchy_selection` 一律单选 | Shift+↑↓ 扩选 | 待办 |
| H7 | 锁定/隐藏标记残留：删除实体后 `editor.hidden.*` 不清理 | `hierarchy_ops.rs:persist_hierarchy_flag` | 元数据随对象走 | 待办 |
| H8 | 右键菜单缺「解包预制体」「选择子级/根」 | 菜单项清单位于 `panel.rs` | 层级右键全套 | 待办 |

### 3.2 Inspector + 组件 schema

| # | 缺口 | 证据 | Unity 对照 | 状态 |
|---|---|---|---|---|
| I1 | 搜索 0 命中时搜索框与清除按钮**一起消失**（用户被锁在空面板） | `panels/schema_inspector/panel.rs` 旧实现先 `rows.is_empty()` 早退 | 搜索框常驻 | **已修（本批）** |
| I2 | 混合值只有"混合"两个字，看不到差在哪 | `schema_fields.rs:rows_for_selection` 旧实现只置 `mixed=true` | 灰显不定态 | **已修（本批：—— 占位 + 差异列表）** |
| I3 | 手写面板组件（Collider3d/Tilemap/Animation/Container/UiEvents/UiBindings/ButtonStates/UiAnimation/Script）不参与多选 | `schema_fields.rs:HANDWRITTEN_PANEL_COMPONENTS` | 所有组件多选 | 待办（L，需逐组件批命令） |
| I4 | 组件级右键菜单 0 覆盖（只有 ↑/↓/移除按钮）；整实体粘贴会冲掉 `prefab.source` | `panels/inspector/panel.rs`；`actions/components.rs` | 每组件 Copy/Paste Values/Reset | 待办（M） |
| I5 | 引用字段只有资源路径选择器，没有实体/组件引用（`UiEvents.target` 靠手打） | `component_schema.rs` 的 `FieldKind` 无 Entity 变体 | ObjectField（拖场景对象） | 待办（L，改 schema+存储） |
| I6 | Prefab 覆盖不内联（只在下方折叠列表里用 monospace 文本展示） | `schema_fields.rs:SchemaFieldRow` 无 override 标记位 | 字段行蓝线 + 回退箭头 | 待办（M） |
| I7 | 组件不能折叠；Add Component 无搜索 | `panels/inspector/panel.rs` 全展开 + ComboBox 列举 | 每组件 foldout + 可搜 | 待办（S–M） |

### 3.3 Scene View + Gizmo

| # | 缺口 | 证据 | Unity 对照 | 状态 |
|---|---|---|---|---|
| S1 | 视口内无 F 聚焦；F 只在层级面板且只改相机 target（3D 不调距离、2D 只改 pan），2D 的 Y 符号写反 | 旧 `panels/hierarchy/navigation.rs:focus_selected_scene_view` | Frame Selected | **已修（本批）** |
| S2 | 3D 无地面网格/原点轴，无视口内统计叠加 | `draw.rs` 只有 2D 的 16px 细网格；全仓无 3D grid | Grid + Stats | 待办（M） |
| S3 | 选中表现弱：3D 只有可关闭的菱形标记，无轮廓；2D 描边不跟旋转 | `view_3d.rs`；`draw.rs` 对比 `scene_geometry.rs` | Selection Outline | **已修（M88）** |
| S4 | 轴心"中心"用硬编码 ±0.5 盒，**单选时与 Pivot 等价**，Center 开关看不出效果 | `view_3d.rs` 的 bounds 常量；`gizmo.rs` | Center = 包围盒中心 | **已修（M88）** |
| S5 | 无正交/透视切换、pitch 上限 1.4 顶不了视、无视图立方/轴向预设 | `projection.rs` 写死 `fov_y`；`view_3d.rs` 的 pitch clamp | View Cube + Ortho | 待办（L） |
| S6 | 2D 无手柄（移动同时改两轴、缩放等比、旋转只认横向拖动） | `scene_geometry.rs`；`pointer.rs`；`draw.rs` | 2D 轴箭头/旋转环 | 待办（M） |
| S7 | 3D 缺 XY/YZ 平面手柄；自由拖动被锁在 XZ 地面 | `gizmo.rs`；`view_3d.rs` | 平面方块手柄 | 待办（M） |
| S8 | 隐藏/隔离不贯穿渲染（2D 只隐藏选框、3D 完全无视 hidden） | `scene_geometry.rs`；`view_3d.rs` | Hide/Isolate 影响真实绘制 | 待办（M） |
| S9 | 3D 无框选；点选用固定 ±0.5 AABB，大网格/小物体点不准 | `view_3d.rs` | 拖拽框选 + 真实 AABB | 待办（M） |
| S10 | 缩放不吸附；移动/缩放手柄无 hover 高亮、无 X/Y/Z 字母 | `gizmo.rs`；`view_3d.rs` | 缩放吸附 + 预高亮 | 待办（S） |

### 3.4 资源浏览器 + 导入

| # | 缺口 | 证据 | Unity 对照 | 状态 |
|---|---|---|---|---|
| A1 | 改名丢导入设置：`resources.meta` 以路径为键，`perform_resource_move` 不迁移键 | `rustlua-assets/src/import_settings.rs`；`actions/layout_ops.rs` | `.meta` 随文件走 | **已修（M88）** |
| A2 | 引用图不同源：删除守卫/依赖视图只扫场景/Prefab + Lua，DB 却已连材质/图集/Spine 边 → 只被材质引用的贴图可被静默删除 | `actions/delete_guard.rs`；`actions/dependency_ops.rs`；`rustlua-assets/src/dependency.rs` | AssetDatabase 引用图 | **已修（M88）** |
| A3 | 只有路径子串过滤，无类型筛选/排序 | `resource_browser_state.rs` | `t:` 过滤 + 列排序 | **已修（M88）** |
| A4 | 单选中，无批量改名/删除/移动 | `main.rs` 的 `selected_resource: Option<PathBuf>` | 多选 + 批量 | **已修（M88）** |
| A5 | 删除不可回收、改名不可撤销 | `actions/delete_guard.rs`；`layout_ops.rs` | 回收站 / Undo | 待办（M） |
| A6 | 无网格视图，缩略图只解码 PNG | `panels/resource_browser.rs` | 网格 + 全类型缩略图 | 待办（M） |
| A7 | 无收藏/标签；文件夹不能改名/移动 | `resource_db.rs`；`resource_browser_state.rs` | 收藏夹 + 移动文件夹 | 待办（M–L） |
| A8 | 全工程搜索是手动快照、不能按类别过滤；文件监听不覆盖工程根文件 | `actions/project_index.rs`；`panels/project_search.rs`；`file_watcher.rs` | Quick Search | 待办（M） |

### 3.5 Console + Play + 调试器 + Profiler

| # | 缺口 | 证据 | Unity 对照 | 状态 |
|---|---|---|---|---|
| C1 | Console **没有**虚拟滚动（注释说只画视口行，实现逐行建控件）：10k 行时每帧 120.96 ms | `panels/console.rs` 旧 `for index in 0..row_count` | 大日志不卡 | **已修（本批：0.529 ms / 9 行）** |
| C2 | sink 文本每帧 clone 整条缓冲（profiler gauge 一处、面板一处） | `main.rs:update_profiler_gauges`；`console.rs` 旧 `snapshot()` | —— | **已修（本批：按代数缓存）** |
| C3 | 无跟随尾部（新日志到达时视图不动） | `console.rs` 旧 ScrollArea 无 `stick_to_bottom` | 自动滚底 | **已修（本批）** |
| C4 | 普通日志无 Collapse/去重；预览诊断有去重但不显次数 | `console.rs:build_console_rows`；`preview_diagnostics.rs` | Collapse + 计数 | **已修（M88）** |
| C5 | 级别过滤单选，不能「警告+错误」同显；无正则/反向/target 过滤 | `console.rs` 的 `ConsoleLogLevelFilter` | 多选 + 搜索 | **已修（M88）** |
| C6 | 崩溃现场栈帧是纯文本、不可点击跳行 | `panels/crash_report.rs` | 双击栈帧跳源码 | **已修（M88）** |
| C7 | 无 Clear-on-Play | `console.rs`；`game_view.rs:play()` | Clear on Play 开关 | 待办（S） |
| C8 | **断点调试 UI 全缺**（设/删/列断点、步入/步过/继续、Locals/Watch）：内核缺 `PauseReason::Pause`、同步 handler 与暂停期求值 | `preview_session.rs` 恒 `Continue`；`rustlua-scripting/src/debug.rs` | 断点窗 + Locals | **已修（M88）** |
| C9 | 运行态对象检查是 8 条固定只读探针，镜像不带组件载荷 | `panels/runtime_debug.rs`；`play_sync.rs` | 实时 Inspector | 待办（M–L） |
| C10 | 时间线不可缩放/拖拽/选帧，无 flame/call-tree；采集与导出同步写 UI 线程 | `trace_timeline.rs`；`performance/panel.rs` | 帧选择 + 层级视图 + 异步 | 待办（M） |

### 3.6 Shell（菜单/停靠/快捷键/通知/帧路径）

| # | 缺口 | 证据 | Unity 对照 | 状态 |
|---|---|---|---|---|
| P1 | 布局自动保存每帧两次深克隆 + `clamp()` 内每个宽度键构造整份 `EditorLayout::default()` | `main.rs:maybe_persist_layout_on_change`；`layout.rs:clamp` | —— | 待办（实测约 100 次分配/帧，**本批判定收益不足**，见 §4.7） |
| P2 | 命令面板打开时每帧重建 63 条候选并逐条 `to_lowercase` | `panels/command_palette.rs`；`menu.rs` | Quick Search | 待办（S） |
| P3 | `effect_overlay_slots` 每帧 clone | `main.rs` | —— | 待办（S） |
| P4 | 无通知中心 / Toast，反馈只有单行 status | `panels/header.rs`；`progress.rs` | 通知可点重试 | **已修（M88）** |
| P5 | 命令面板跳过动态行（面板开关/工作台/视图模式/最近项目/"加组件"） | `menu.rs` 的过滤 | 全菜单可搜 | **已修（M88）** |
| P6 | 可改键动作只有 15 条；Play/Stop/Pause/Step 无键 | `shortcuts.rs` | Play 键 + F2 | 待办（S） |
| P7 | Scene View 无右键菜单（Create/Align） | 全仓 `context_menu` 只在层级与资源面板 | Scene 右键 | 待办（M） |
| P8 | `density` 是死设置（持久化但无人读）；布局无预设 | `layout.rs`；`main.rs:apply_layout` | Preferences + Layout 预设 | 待办（S/M） |
| P9 | 进度单通道：`start()` 覆盖上一个任务 | `panels/progress.rs` | 多任务进度 | 待办（M） |
| P10 | 菜单条目纯文本无图标；只有 8 处显示快捷键 | `menu.rs` | 图标 + 快捷键列 | 待办（S） |

> 计数：台账 53 条（H8 + I7 + S10 + A8 + C10 + P10），本轮**已修 8 条**（H1/H2/I1/I2/S1/C1/C2/C3），
> 其余 45 条按 §5 的顺序进入下一批输入。这个比例是诚实的：一批只做能验证完的。

## 4. 本轮落地

### 4.1 Console：真虚拟化 + 快照按代数缓存（本条是最大的一笔）

**问题**：模块头注释写"`show_rows` 只画视口内的行"，实现却是
`for index in 0..row_count { draw_log_line(...) }`；同时每帧 `log_sink.snapshot()` 复制整条缓冲。
100k 行推送（保留上限 10k）的实测：**每帧 10000 行、p95 = 120.96 ms**——"大日志不会冻主线程"
这句话当时只在注释里成立。

**改法**：

1. 滚动体自己拥有 `ScrollArea`，用 `show_rows(ui, row_height, row_count, ..)` 只画视口行；
2. `stick_to_bottom(true)` 给跟随尾部（用户往上滚后不再抢位置）；
3. sink 文本快照存进 `ConsoleRowCache`，按 `LogSink::generation` 失效：
   稳态帧 `mem::take` 一次 O(1)，不再 clone 上万条字符串；
4. `perf_baseline` 的 `console_panel_frame_100k` 补**真断言**：`rows_drawn > 0 && < 200`
   （以前那条"只画视口行"只是注释）。

**读数**（同一台机器、debug 档、`--perf-baseline` 全量 37 场景）：

| 指标 | 修前 | 修后（首轮） | 修后（最终复跑） |
|---|---:|---:|---:|
| `console_panel_frame_100k` p95 | **120.960 ms** | 0.529 ms | **0.376 ms** |
| 相对修前 | — | −99.6% | **−99.7%（321×）** |
| 每帧绘制行数 | 10000（全量） | 9（视口内） | **9** |
| 索引缓存 hit/miss（30 帧） | 30 / 0 | 30 / 0 | 30 / 0 |

> 三次读数都来自同一个 `--perf-baseline` 入口、同一台机器、同一 debug 档：
> `artifacts/metrics/m87-perf-before.json`、`m87-perf-after.json`、`m87-perf-after-final.json`。

**反面对照**（测试层）：`console_sink_snapshot_is_cached_by_generation` 在"代数没变却把缓存清空"时
必须拿到空——如果实现回落到每帧 `snapshot()`，这条会拿回两行并判红。

### 4.2 profiler gauge 不再每帧 clone 日志缓冲

`update_profiler_gauges` 原来用 `self.log_sink.snapshot().len()` 只为取一个数字，等于每帧
把 1 万条 `String` 复制一遍；profiler 默认开启（`enabled: true`），所以这是**每帧都付**的成本。
改成 `self.log_sink.len()`（`rustlua-infra` 已有不 clone 的实现）。

### 4.3 视口内 F 聚焦（Unity `Frame Selected`）

**问题**：`F` 只在层级面板生效；3D 只改 `editor_camera.target`（距离不变 → 选中对象仍在视口外），
2D 只改 `pan`（zoom 不变），且 2D 的 Y 写成 `+position_y * zoom`——与
`editor_scene_camera` 的 `pan = -center * zoom` 差一个符号，聚焦点会跑到两倍偏移处。

**改法**：新增 `panels/scene_view/frame.rs`（纯几何，零文档访问）：

- `FocusBounds`：`around` / `union` / `center` / `radius`（半对角线，按**球**装进视锥）；
- `framed_distance(radius, fov_y, aspect)`：取垂直/水平视场中**更紧**的一侧（竖屏按水平兜底）；
- `framed_zoom_2d(size, viewport, margin)`：与 `fit_scene_view_to_design` 同一套边距与上下限；
- `pan_centered_on(center, zoom)`：`pan = -center * zoom`，与 `editor_scene_camera` 同一口径；
- `EditorApp::frame_selection_in_scene_view()`：2D/3D 都按**选中集合并集包围盒**适配；
  2D 的尺寸与 `entity_2d_screen_box` 同源（Sprite `size` / Tilemap 网格 / 默认 112×44）。

接线：层级面板 `F`、2D 视口 `F`、3D 视口 `F` 三处**同一个入口**；`fit_scene_view_to_design`
也改走 `framed_zoom_2d` + `pan_centered_on`，两条路径不会再分叉。

**测试**（5 条，无 GPU）：包围盒并集/半径、竖屏比横屏需要更远、zoom 装进边距、
"聚焦点必须投影到面板中心"的往返（这条同时钉住符号约定）、设计与聚焦共用同一 zoom 规则。

### 4.4 多选批量：Ctrl+D 与「移动到根节点」

**问题**：拖拽会整批移动顶层根，但 `reparent_selected`（右键「移动到根节点」/命令层）只搬
`primary`；Ctrl+D 同样只复制 `primary`。同一个"移动/复制"动作出现两套语义。

**改法**：

- 抽出 `ReparentTarget { Parent(..), At { target, position } }` 与
  `commit_reparent_batch(batch, target)`：**整批校验 → 一条事务 → keep-world → 提交或整体回滚**；
  拖放与右键菜单都走它（`handle_hierarchy_drop` 因此从 ~80 行缩成 ~20 行）；
- `duplicate_selected_entities`：按可见顺序取 `top_most_selected_ids()`，逐个复制到原父、
  紧随源节点，复制体成为新的选中集合；
- 命令层 `DuplicateSelection` 与测试同步改名。

**测试**（3 条新增）：多选复制（两个父级各有一个复制体、一步 Undo 全回滚）、
多选换父（两个目标都搬、一步 Undo 还原）、多选移到根（Root 的子节点清空）。

### 4.5 Inspector：搜索框常驻 + 混合值差异

**问题 1**：`draw_schema_driven_components` 先判 `rows.is_empty()` 再画搜索框——用户搜到一个
不存在的词后，搜索框和清除按钮**一起消失**，面板里再没有任何可操作控件。

**改法**：先算全量行 `all_rows`，只要"有行或有过滤词"就画搜索区；0 命中时给空状态
「没有匹配的字段：换个关键词，或用右侧 × 清除过滤」。

**问题 2**：混合值只显示"混合"两个字。**改法**：`SchemaFieldRow` 追加 `mixed_values: Vec<String>`
（主选中在前、去重、上限 8 条），视图把它当 `——` 占位的悬停说明列出全部取值。
输入框仍以主选中为基准（提交写全部目标），但"差在哪"不用再挨个点选。

**测试**：`multi_selection_edit_writes_all_targets_in_one_undo` 增断言
`mixed_values == ["64,32", "8,8"]`。

### 4.6 架构棘轮同步

新增面板文件 `scene_view/frame.rs` 触发 `panels_only_emit_actions` 的登记要求：已在
`PINNED_WRITES` 登记为 **0 处直接写入**（它是纯几何 + 只读选择，不碰文档）。

### 4.7 审计里"看起来是热点"但本批**判定不做**的两条

- **布局每帧深克隆**（P1）：`layout.clone()` ×2 + `clamp()` 内每键构造 `EditorLayout::default()`。
  实测量级：约 100 次小分配/帧 ≈ 个位数微秒，相比 4.1 的 120 ms 差 4 个数量级；
  改它要重排 `maybe_persist_layout_on_change` 的借用与 `clamp` 的默认值来源，
  收益与风险不成比例。**登记，不本批做**（下次做时先加 `shell_idle_frame` 场景拿读数）。
- **快捷键每帧重解析**（P6 的邻居）：15 次 `ShortcutSpec::parse` ≈ 30 次小分配/帧，同样是微秒级。
  要缓存就得给 `ShortcutBindings` 加代数并保证所有改键路径都失效——为一个微秒级成本引入
  状态同步风险不划算。**登记**。

> 这两条写进本文，是因为审计报告把它们列为"热点"。**不是所有被叫热点的都值得修**；
> 判据是同一台机器上的读数，而不是代码形状。

## 5. 下一批输入（**已全部由 M88 落地，逐条见 §8**）

> 2026-11-27 第二批（M88）把下面 9 条全部做完。每条的状态、改法与读数在 §8，
> 本文 §3 台账里的对应行已同步改成"已修（M88）"。

1. **C8 断点调试 UI**：v1 = 断点集合 + 继续/单帧步进 + 命中现场（栈/局部变量）。
   边界：真·按行步进/条件断点/watch 仍需内核"原地挂起 VM"，见 §8.1。
2. **A1 + A2 资源正确性**：改名迁移 `resources.meta` 键；删除守卫与依赖视图补扫
   `.material`/`.atlas`/`.spine`。
3. **H3 层级揭示选中**：主选中变化时展开祖先并滚动到目标行。
4. **H4 锁定语义**：定为"锁只挡场景拾取"，层级里仍可选中/删除。
5. **S4 + S3 选中与轴心**：局部包围盒按网格规格 × 世界缩放算，Center 轴心真正生效；
   2D 选中轮廓跟旋转。
6. **C4 + C5 + C6 Console 收尾**：折叠 + ×N、级别多选、崩溃栈帧可点击跳转。
7. **P4 + P5 通知与命令面板**：通知中心（有界/去重/可点跳转）+ 面板开关进命令面板。
8. **A3 + A4 资源浏览器**：`t:`/`type:` 类型筛选、多选与批量删除。
9. **预制体重名静默覆盖**：同名取 `_1` 后缀，不覆盖原文件。


## 6. 验收读数（2026-11-27）

| 项 | 命令 | 读数 |
|---|---|---|
| 编译 | `cargo check -p editor --all-targets` | 通过（0 error / 0 warning） |
| 格式 | `cargo fmt --all -- --check` | exit 0（`view_3d.rs` 的历史 CRLF 同笔转 LF，语义零 diff） |
| 静态检查 | `cargo clippy -p editor --all-targets --all-features -- -D warnings` | exit 0（0 warning） |
| 聚焦纯几何 | `cargo test -p editor --bin editor panels::scene_view::frame` | 5 passed / 0 failed |
| 层级批量 | `cargo test -p editor --bin editor panels::hierarchy::tests` | 21 passed / 0 failed |
| Inspector | `cargo test -p editor --bin editor panels::schema_inspector` | 29 passed / 0 failed |
| 架构棘轮 | `cargo test -p editor --bin editor panels::architecture_tests` | 3 passed / 0 failed |
| 全量 | `cargo test -p editor` | **816 passed / 0 failed / 2 ignored**（M87 收口时 796） |
| GUI smoke | `cargo run -q -p editor -- --gui-smoke` | **cases=43 failed=0 result=ok**（exit 0） |
| 性能基线 | `cargo run -q -p editor -- --perf-baseline` | 37 场景 `failed=0 result=ok` |
| Console 场景 | 同上，`console_panel_frame_100k` | p95 **0.354 ms**（M88 复跑；修前 120.960 ms），绘制 **9** 行 |
| 资源/核心 | `cargo test -p rustlua-assets import_settings`、`cargo test -p rustlua-editor-core resource_browser` | 11 / 21 passed（含 M88 新增的类型筛选与 meta 迁移） |
| 文档门 | `scripts\verify\check-docs.ps1` | `check_docs_ok docs=79 h1=81 links=509 ps1_bom_ok=84` |
| 仓级快速门 | `scripts\verify\verify.ps1 -Quick` | **Quick gate passed (PARTIAL)**，exit 0；`editor_project_ok schema_errors=0`、`lua_api_surface tables=52 funcs=444` |

> **诚实边界**：`-Quick` 跳过 clippy/全量 workspace 测试/release 构建/覆盖率/度量/rustdoc/GUI smoke
> （跳过不等于通过，脚本自己也这么打印）。本批已单独跑过编辑器侧的 clippy、全量测试、GUI smoke 与
> 性能基线；**整个 workspace** 的 `cargo test --all-features` 与 `verify.ps1` 全量段没有在本批重跑，
> 因为本批只动 `editor` crate 的 20 个文件、未触及 engine/渲染/wasm。

## 8. 第二批落地（M88，2026-11-27）

本批把 §5 的 9 条全部做完。下面逐条写"问题 → 改法 → 证据/测试"，边界单列。

### 8.1 C8 Lua 断点调试 v1

**问题**：编辑器只有"崩溃一键复现"这一条调试路径（单断点、恒 `Continue`），没有断点列表、
没有继续/步进、没有命中现场。

**内核侧**（`preview_session.rs`）：新增 `DebugControl { pending, pause, hits }`，用
`Rc<RefCell<..>>` 与 VM 回调共享。

- `arm_lua_debugger()` 装回调：命中时存 `PauseSnapshot`、消费一次 `pending`（没有就 `Continue`）；
- `request_debug_action()` 排队下一次动作；`set_lua_breakpoints()` 覆盖式同步断点；
- `take_lua_pause()` 取现场；`clear_lua_debugger()` 在 Stop 时清断点并装恒 `Continue` 的回调。

**线程模型与"暂停"的语义**（本批最重要的设计约束）：Lua 与 egui 在**同一线程**，回调在
Lua 行事件里同步执行，不能借编辑器状态、也不能阻塞等 UI。所以回调只记现场 + 排队动作，
真正的冻结发生在**帧边界**（`play.pause()`）。由此：
**"继续" = 恢复 Play；"单帧步进" = 前进一帧**（Unity 的 Step 同样是一帧）；
真·按行步进需要内核支持原地挂起 VM，属下一包。

**面板**（`panels/lua_debugger.rs`，窗口 + `窗口 ▸ Lua 调试`）：断点增删（`路径:行`，
也接受 `路径 行`，统一成正斜杠）、全部清空、继续 / 单帧步进 / 停止、命中现场的调用栈与局部
变量、每行"打开源码"（复用 `open_lua_source`）。

**两个容易踩的坑，本批都堵了**：

1. **逐行回调是有代价的**：内核只要"有断点**或**有回调"就逐行触发钩子。所以只在
   **用户真的在用调试器**（有断点，或面板打开）时才装（`armed` 开关），Play 中打开面板/加断点
   会懒装上。没有这个开关，所有用户都会为一局不调试的 Play 付每行一次 Rust 回调。
2. **Stop 必须卸载**：断点与回调住在 Lua 侧 thread_local 里，不随预览会话释放；不清的话
   下一局的启动脚本会带着旧断点跑（`actions/document_gate.rs::stop` 已调用 `clear_lua_debugger`）。

**测试**：断点输入解析 3 例（含 Windows 盘符路径、`路径 行`、行号 0/非数字拒绝）；
无会话时所有入口都是空操作 1 例（不 panic、不丢断点、不谎称已装）。
**边界**：真实命中并冻结 Play 需要 GPU 会话（`--gui-smoke` 不跑 Play），本批**未做端到端验证**，
登记为下一批的实机项；条件断点 / hit count / 暂停期求值（watch）也没做。

### 8.2 A1 改名迁移导入设置键

`ResourceMeta::rename_path(from, to)`：四张表（贴图/模型/音频/字体）一起搬键，目标键已存在时
以源为准；`perform_resource_move` 在文件搬完后调用，失败**不阻断**改名但写进状态栏
（"导入设置未迁移：…"）。测试：assets crate 的"改键 + 落盘 + 旧键消失"一条；
`resource_browser` 的改名端到端测试里补了"设置跟着新路径走"的断言。

### 8.3 A2 引用图同源（材质/图集/Spine）

新增 `delete_guard::descriptor_documents(root)`（`material`/`atlas`/`spine` 三类描述文档），
删除守卫与依赖视图**共用**这份枚举，匹配走 `dependency_ops::references_target`（内部即
`rustlua_assets::dependency::extract_refs`，与资源数据库同一台扫描器）。
测试：只被材质引用的贴图**拦住删除**；图集与 Spine 清单都会被扫到；既有的
"守卫数到的条数 == 改指会改的条数"仍然绿。

### 8.4 H3 层级揭示选中

`maybe_reveal_primary_selection()`：**主选中变化**时清掉全部祖先的折叠位并登记
`hierarchy_scroll_target`；面板 `show_rows` 里命中目标行时 `scroll_to_cursor(Center)` 并消费掉请求。
只看"主选中是否变化"是有意的——否则用户手动折叠一个正在选中的节点，下一帧就会被抢回去。
测试：折叠 Root 后选中子节点 → 祖先展开 + 有滚动请求；同一选中再跑一次不产生新请求；
换主选中才再次揭示。

### 8.5 H4 锁定语义（定为"只挡场景拾取"）

层级行不再因为锁定而 `add_enabled(false)`（旧行为让锁住的实体在层级里点不中，与语义相反）；
锁只作用于场景视图拾取与 Gizmo 拖动（既有实现），悬停时说明这条语义。
测试：锁定实体在层级里可选中、可显式删除。

### 8.6 S4 + S3 选中包围盒与轴心、2D 轮廓跟旋转

新增 `panels/scene_view/bounds_3d.rs`：局部包围盒按**网格规格**取真实范围
（terrain/water/cells/primitives 与**宿主渲染提取同一批解析器**，glTF 与宿主一样是 0.5 兜底），
再按 Transform 缩放放大（负缩放重排 min/max）。3D 点选、选中范围与 `gizmo_anchor(Center)`
因此都用真实盒子：Center 轴心在偏心盒（巢体从 `origin` 起算）上与 Pivot 真正分离。
2D 选中轮廓改用 `rotated_corners` 画闭合折线，点击命中改走 `rotated_contains`——
轮廓、命中与渲染三者终于同一套旋转判定。
测试 4 条：规格真实尺寸、缩放/负缩放、未知网格兜底、偏心盒 Center≠Pivot。

### 8.7 C4 + C5 + C6 Console 收尾

- **折叠**：`console_collapse`（默认开）+ `ConsoleRowRef.repeat`；索引里只保留连续重复的第一条，
  显示时加 ` (×N)`；关掉回到逐行（旧行为）。`×N` 只在**显示文本**上，索引原文不变（过滤/跳转仍准）。
- **级别多选**：`ConsoleLogLevelFilter` 从四选一枚举改成三个布尔位 + "全部"快捷位；
  旧枚举表达不了的"警告+错误同显"现在可以了；三个都关时面板给"未选择任何级别"。
- **崩溃栈帧可点**：每帧一个无边框按钮，点击走 `CrashAction::OpenSource(path, line)`（与顶部
  "查看源码"同一条通道），悬停说明；`frame_label` 有单测钉住"路径:行确实在标签里"。
- 测试：级别多选 1、折叠 1、栈帧标签 1；`perf_baseline` 的 100k 场景显式关掉折叠，保持既有读数口径。

### 8.8 P4 + P5 通知中心与命令面板动态行

- **通知中心**（`src/notifications.rs`）：有界 24 条、连续同文去重（`×N`）、右下角最多显示 3 条、
  可点跳转 / 单条关闭。接入三处真正的失败：布局文件损坏回落、布局自动保存失败、用户设置写盘失败。
  以前这些只有"下一帧就被覆盖的状态栏"或埋在日志里的 `warn!`——用户拖面板时布局保存失败是看不见的。
- **命令面板动态行**：`dynamic_palette_items()` 把三类运行时数据放进面板——十个面板开关、
  两个工作台页签、三个视图模式（面包屑与菜单同形 `窗口 / 面板 / 控制台`、`窗口 / 视图模式 / 3D`）。
  动作仍是同一个 `MenuAction`，**不新增命令通道**；视图模式新增单一入口
  `set_scene_view_mode`（菜单/工具栏/面板共用，避免"模式变了但工具没收敛"）。
- 测试：通知有界/去重/最新在前、单条关闭 + 带位置的通知；面板候选里能搜到"面板 控制台"且
  动作等于 `ToggleConsole`；动态条目共 15 条。

### 8.9 预制体重名不静默覆盖

`create_prefab_from_selected` 改用 `unique_prefab_stem`：同名文件存在时取 `Name_1`、`Name_2`…
（Unity 的同名后缀语义），状态与 Console 都报**最终路径**（状态栏随后会被资源刷新覆盖，
所以 Console 那条是回查依据）。测试 2 条：同名创建落到 `Hero_1.prefab` 且原文件字节不变；
去重函数在空目录/已有 `_1` 时的返回。

### 8.10 本批边界（未做，下一批输入）

1. **断点调试端到端**：真实命中并冻结（需要 GPU/窗口会话）；条件断点、hit count、
   暂停期求值（watch）、真·按行步进（需内核原地挂起 VM）。
2. **断点持久化**：断点只活在本次进程（编辑器侧状态），重启后清空。
3. **资源浏览器**：批量移动/批量重命名、文件夹批量删除、网格视图仍未做（本批只做类型筛选 +
   多选 + 批量删除）。
4. **Console**：正则/target 过滤、Clear-on-Play 仍未做。
5. **通知中心**：只收失败与警告，没有"任务型通知"（进度/取消/重试属于进度面板）。
6. **命令面板**：最近项目与"加组件"仍是各自面板的入口（数据驱动、且已有专门 UI）；
   工具栏勾选项留在菜单里（它们是设置行，不是命令）。

## 9. 维护约定
- 台账里的每条缺口都带 `file:line` 或可复核的模块位置；修掉时把状态改成 **已修（日期/批号）**，
  **不删行**（删了就没人知道这里曾经缺过）。
- 新发现的缺口编号顺延（H9、I8、S11、A9、C11、P11…），同一张表内保持主题。
- 性能条目必须给**前后两次读数**（同一个 `--perf-baseline` 入口、同一台机器、同一档）；
  只给"看起来更快"的条目一律不进本表。
- "判定不做"的条目要写清**读数依据**（见 §4.7），否则下一轮会被重新当成热点调研一遍。
