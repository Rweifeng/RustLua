# E01：编辑器状态与文档变更基线盘点（只读）

> 依据 [28-editor-ui-optimization-execution-tasks.md](./28-editor-ui-optimization-execution-tasks.md) 阶段 0 / E01 执行。
> 本文档为只读基线：**不改动任何编辑器运行行为**，只做状态梳理与问题登记，供 E02–E10 在批次 A 内落地时引用。
> 涉及源码行号基于 `engine/crates/editor` 当前工作区（HEAD fbb2acc），如后续重构行号漂移，以代码为准。
> 2026-10-25 补注：`document`/`history`/`recovery` 等纯领域模块已随 crate 拆分原样搬到
> `engine/crates/rustlua-editor-core/src/`（`editor` 以 glob re-export 保持旧路径可用），
> 文中路径已指向新位置；行号未变。

---

## 0. 阅读范围与引用文件

已全文/关键段阅读（本基线结论均来自代码，非推测）：

| 模块 | 文件 | 说明 |
|---|---|---|
| 二进制 crate 主循环 | `engine/crates/editor/src/main.rs` | EditorApp 字段、update 顺序、场景视图缓存、DeleteTarget |
| 变更入口 | `engine/crates/editor/src/actions.rs` | set_current_scene_document / undo/redo / save_scene / open_scene_resource / sync_inspector / 批量对齐 |
| 文档模型 | `engine/crates/rustlua-editor-core/src/document.rs` | SceneDocument / PlaySession / ProjectDocument |
| 历史 | `engine/crates/rustlua-editor-core/src/history.rs` | CommandKind / EditCommand / execute 合并与 no-op |
| 层级 | `engine/crates/editor/src/panels/hierarchy.rs` | 选择、过滤、隐藏/锁定、create_scene、sync_hierarchy_metadata |
| Scene/Game | `engine/crates/editor/src/panels/game_view.rs` | play/stop 状态机与测试 |
| Header | `engine/crates/editor/src/panels/header.rs` | 菜单、recover_candidate、窗口标题 |
| 导入 | `engine/crates/editor/src/import.rs` | 项目/资源导入与 apply_progress_result |
| 预览会话 | `engine/crates/editor/src/preview_session.rs` | PreviewSession 生命周期与节流 |
| 恢复 | `engine/crates/rustlua-editor-core/src/recovery.rs` | RecoveryManager / AutoSaveController |
| 选择/状态结构 | `rustlua-editor-core/src/scene_tree_state.rs`、`inspector_state.rs`、`resource_browser_state.rs` | 三类选择与资源浏览 |
| 库 crate 边界 | `engine/crates/editor/src/lib.rs` | editor::* 公开面 |
| Scene View | `engine/crates/editor/src/panels/scene_view.rs`（关键交互段） | 拖拽/框选/图块/3D gizmo 提交点 |
| Inspector | `engine/crates/editor/src/panels/inspector.rs`（关键函数） | apply_inspector / rename / reparent / delete / prefab 相关 |

---

## 1. 状态来源盘点（当前所有“事实源”）

E01 要求“列出当前所有选择状态、Inspector 状态、Hierarchy 状态和 Scene View 状态”。当前状态散布在多个结构/多个字段中，**没有单一事实源、没有事件通知、没有统一清理 hook**。下表按功能分组：

### 1.1 文档与文档身份

| 状态 | 位置 | 说明 |
|---|---|---|
| `ProjectDocument` | `document`（EditorApp 字段） | 项目级：内含 `current_scene: Option<SceneDocument>`、最近保存等信息（[document.rs](../engine/crates/rustlua-editor-core/src/document.rs#L147-L155)） |
| `SceneDocument` | `document.current_scene` | 当前编辑文档；含 `path` / `scene` / `dirty` / `revision` |
| `PlaySession`（1:1） | `play_session: Option<PlaySession>` | `editor_snapshot` + `runtime_document`，见 [document.rs](../engine/crates/rustlua-editor-core/src/document.rs#L114-L141) |
| `preview: Option<PreviewSession>` | `preview` | GPU/运行时预览会话（Game 视图消费） |

### 1.2 场景数据 vs 会话状态

- **场景数据（写盘部分）**：`document.current_scene.scene`（`Scene`：entities/children/metadata）+ `document`。
- **运行时副本**：`play_session.runtime_document()`（Play 期间 `current_scene()` 优先返回它，[main.rs](../engine/crates/editor/src/main.rs#L845-L850)）。
- **编辑会话状态（内存态）**：
  - `tree.selected: Option<SceneEntityId>`（Hierarchy 单选）
  - `inspector.selected: Option<SceneEntityId>`
  - `scene_state.selected: BTreeSet<SceneEntityId>`（真正多选工作区）
  - `selected_resource: Option<PathBuf>`（资源面板单选）
  - `delete_target: DeleteTarget`（None/Hierarchy/Resource，决定 Delete 快捷键打哪）

### 1.3 SceneEditorState（[main.rs](../engine/crates/editor/src/main.rs#L512-L530)）

| 字段 | 含义 |
|---|---|
| `pan: egui::Vec2` | 2D 视图平移 |
| `drag` / `box_select_start` | 拖拽/框选过程状态 |
| `preview` / `preview_ui` / `preview3d_transform` | 未提交的编辑预览（拖拽中间态） |
| `gizmo_drag` | 3D gizmo 过程态 |
| `collapsed` / `hidden` / `locked` | Hierarchy 折叠 / 隐藏 / 锁定集合 |
| `selected` | 工作多选集合（框选/加选来源） |
| `tile_paint_*` | 图块绘制过程态 |

### 1.4 编辑器快照字段（Inspector 渲染用，EditorApp 成员）

Inspector 输入框不直接读写场景，而是读写 EditorApp 上的**快照字段**：`entity_name`、`transform`、`transform3d`、`mesh_renderer`、`camera3d`、`light`、`collider3d`、`text`、`image`、`sprite_reference`、`sprite_component`、`tilemap`、`animation` 等（[main.rs](../engine/crates/editor/src/main.rs#L601-L627)）。`sync_inspector(id)` 是唯一把 InspectorState.selected + 场景 → 这些快照字段的同步点（[actions/document_io.rs](../engine/crates/editor/src/actions/document_io.rs)）。

### 1.5 场景视图缓存

| 字段 | 含义 |
|---|---|
| `scene_runtime` / `scene_runtime_revision` | 编辑器 2D 渲染用运行时 |
| `scene_view_cache_2d/3d` | 视图缓存，key 用 `revision: Option<u64>`（`None` 区分“无场景”与“revision=0”新场景） |
| `last_metadata_key: Option<(Option<PathBuf>, u64)>` | hierarchy hidden/locked 元数据重同步的 (path, revision) 键 |
| `scene_2d_tex`/`scene_3d_tex`/`scene_screen_texture*`/`scene_font_atlas` | GPU 纹理与字体 |
| `preview_diagnostic` | 预览诊断 |
| `scene_view_repaint_pending` / `scene_view_fit_pending` | 重绘 / fit 标志 |

---

## 2. 文档变更入口清单（含各入口的清理力度）

“所有打开、编辑、Undo/Redo、Prefab 应用和运行时修改应用”目前分散在多个入口。E02 目标是把它们收敛到一个统一 `on_document_changed(change)`。下表按**替换/修改文档的位置**登记现状：

### 2.1 替换 `document.current_scene`（文档级切换）

| 入口 | 位置 | 做了什么 | 清理力度 | 备注 |
|---|---|---|---|---|
| `set_current_scene_document` | [actions/document_io.rs](../engine/crates/editor/src/actions/document_io.rs) | `current_scene = Some(document)` + `invalidate_scene_view_state()` | **弱**：只做 scene view 缓存/预览清理 | 这是所有入口的“统一底座”，但**不清任何选择/元数据/资源选择** |
| `open_scene_resource` | [actions/document_io.rs](../engine/crates/editor/src/actions/document_io.rs) | `open_scene_document` + history.clear + autosave.reset + play_session=None + tree.clear + delete_target=None + inspector.clear + clear_resource_selection + scene_state.selected/collapsed.clear + `sync_hierarchy_metadata()` | **最强（参考实现）** | 唯一同时清理两处选择 + delete_target + collapsed 并手动 sync metadata 的入口；Prefab 打开同样走这里 |
| `create_scene` | [hierarchy.rs](../engine/crates/editor/src/panels/hierarchy.rs#L243-L264) | `set_current_scene_document(SceneDocument::new)` + history.clear + autosave.reset + tree.clear + inspector.clear | **中** | **不清 scene_state.selected/collapsed/hidden/locked/delete_target** |
| `recover_candidate` | [header.rs](../engine/crates/editor/src/panels/header.rs#L169-L177) | `set_current_scene_document(recovery.recover())` + history.clear + autosave.reset + tree.clear + inspector.clear | **中** | 同上，恢复的文档 `dirty=true, revision=0` |
| Play `stop()` | [game_view.rs](../engine/crates/editor/src/panels/game_view.rs#L60-L79) | `set_current_scene_document(session.stop())` + tree.clear + delete_target(Hierarchy→None) + inspector.clear + preview_input.clear + 回 Scene tab | **中** | 丢弃运行态，恢复 editor_snapshot |
| 项目导入完成 | [import.rs](../engine/crates/editor/src/import.rs#L100-L131) | 整块替换 project/document/recovery/resources + history.clear + autosave.reset + lua_runtime=None + play_session=None + tree.clear + inspector.clear + 缩略图/字体图集清空 | **强（整体换项目）** | apply_progress_result 内联，无统一入口 |

### 2.2 就地修改 `document.current_scene.scene`（编辑操作）

| 入口 | 位置 | 通过什么修改 | 是否进 History |
|---|---|---|---|
| `history.execute(...)` 全系列 | 各面板 | 传入 `&mut SceneDocument`（**注意：EditorApp 大多传 `document.current_scene.as_mut()`**） | 是 |
| `scene_tree_state` 内部（clone 到闭包里的 tree） | delete/rename/reparent | `tree.delete_selected(&mut scene...)` | 由调用方 `history.execute` 包住 |
| InspectorState 各 `set_*` | `inspector_state.rs` | 直接 `entity.components.insert` + `mark_dirty`，绕过 History | **否**（调用方通常包 history.execute，见 inspector.rs apply_inspector） |
| `persist_hierarchy_flag`（隐藏/锁定） | [hierarchy.rs](../engine/crates/editor/src/panels/hierarchy.rs#L204-L214) | 直接改 `scene.scene.metadata` + mark_dirty | **否（永远）** |
| Scene View 提交点 | `commit_scene_drag` / `commit_gizmo_drag3d` / `commit_tilemap_paint` / 资源拖放 | `history.execute` + 成功后 `save_scene()` | 是（**同时立刻保存**） |
| Tilemap 图块绘制 | `apply_tile_paint_cell` / `commit_tilemap_paint` | 内存 Tilemap 编辑，释放/切工具时一次性 `history.execute` | 是（合并单条） |

### 2.3 运行时 / 其它数据来源

- `current_scene()`（[main.rs](../engine/crates/editor/src/main.rs#L845-L850)）：Play 期间返回 `runtime_document()`，否则 `document.current_scene`。
- `ensure_scene_runtime`：Scene View 渲染前把当前编辑场景构建成编辑器运行时（只读渲染用途）。
- `update_preview`（Game 视图，game_view.rs）：每帧把编辑器输入桥接进 PreviewSession。

---

## 3. History 命令类型与保存/自动保存行为

### 3.1 History 命令类型

见 [history.rs](../engine/crates/rustlua-editor-core/src/history.rs)：

| CommandKind | 场景 |
|---|---|
| `Generic` | 兜底 |
| `Transform(id)` | 2D 拖拽提交（连续同类合并 after） |
| `Transform3D(id)` | 3D gizmo 提交（连续同类合并 after） |
| `Create` / `Delete` / `Rename` / `Reparent` | 结构编辑 |
| `Property` | 批量属性修改、Tilemap Paint、Prefab Apply、Inspector 提交等 |

关键语义：
- `execute`：clone before → 执行闭包 → 若 `*document == before` 判定 no-op **不记录**（PartialEq 忽略 revision，[document.rs](../engine/crates/rustlua-editor-core/src/document.rs#L20-L36)）；否则 clone after、清空 redo；**仅相邻同类 Transform/Transform3D 命令合并**（用新 after 覆盖前一条的 after）。
- `undo/redo`：只 clone-swap 文档（含 before/after 快照），**不触发任何状态同步 / selection repair / inspector refresh / cache invalidation**（[history.rs](../engine/crates/rustlua-editor-core/src/history.rs) undo/redo 实现）。
- no-op 检测的副作用：纯 revision 递增不产生命令（例如先 mark_dirty 再做结构不变编辑）。

### 3.2 保存触发点

| 触发 | 位置 | 说明 |
|---|---|---|
| Ctrl+S / 菜单“保存项目” | 主循环 [main.rs](../engine/crates/editor/src/main.rs#L942-L946) + header [header.rs](../engine/crates/editor/src/panels/header.rs#L40-L43) | 无条件执行（不区分文本焦点） |
| 保存实体 | `save_scene()` | [actions/document_io.rs](../engine/crates/editor/src/actions/document_io.rs)：无 path 时写 `scenes/<name>.scene`；有 path 走 `recovery.save_document`（validate → save → clear_for）；Prefab 走独立分支 |
| 场景视图拖拽提交 | scene_view.rs `commit_scene_drag`/`commit_gizmo_drag3d`（[scene_view/pointer.rs](../engine/crates/editor/src/panels/scene_view/pointer.rs)） | **history.execute 成功后立刻 `save_scene()`**（“拖一下存一下”） |
| Tilemap Paint / 批量属性 / Prefab Apply | 各提交点 | 部分提交点同样带 `save_scene`（scene_view）；Inspector/tree 走 apply 后手动保存与否不一致 |
| 自动保存 | 主循环 [main.rs](../engine/crates/editor/src/main.rs#L915-L919) | `autosave.tick`（10s 周期，dirty 才写 recovery 快照）；AutoSaveController 在 [recovery.rs](../engine/crates/rustlua-editor-core/src/recovery.rs#L19-L71) |
| 崩溃快照 | `RecoveryManager::autosave/save_document/clear_for` | [recovery.rs](../engine/crates/rustlua-editor-core/src/recovery.rs#L123-L228)：正式保存后清对应 recovery 快照 |
| 编辑器关闭 | — | 依赖 autosave tick 兜底 + 启动时 scan 弹恢复 |

### 3.3 自动保存 / 恢复

- `AutoSaveController`：`last_autosave` + interval（EditorApp new 中 `Duration::from_secs(10)`）。
- `RecoveryManager`：根目录 `.temp/`，文件名按 path/name 的 fnv1a hash → `recovery-<hash>.recovery`；内容含 version/timestamp/original_path/checksum/payload。`scan()` 排序返回候选；`recover()` 返回 `dirty=true, revision=0` 的 SceneDocument；**不覆写正式场景文件**。
- 启动：`EditorApp::new` 即 `recovery.scan()`；header 顶栏“⚠ 恢复”下拉列出候选（header.rs L112-L129），选择后走 `recover_candidate`。

---

## 4. 场景切换 / 打开 / 新建 / Prefab / 项目切换 / 关闭 入口梳理

### 4.1 场景切换（Hierarchy 顶栏下拉）

[hierarchy.rs](../engine/crates/editor/src/panels/hierarchy.rs#L34-L70)：
1. 下拉列出 `scenes/` 目录（`scene_files()` 2s 缓存）。
2. 选中其它文件 → `open_scene_resource(path)`（同 Prefab 打开）。
3. **没有未保存确认**；当前 dirty 文档直接丢弃。

### 4.2 新建场景

- Header “编辑 → 场景名 + 新建场景” → `create_scene()`（[header.rs](../engine/crates/editor/src/panels/header.rs#L50-L59)）。
- `create_scene` 走 `set_current_scene_document(SceneDocument::new(name))`，随后 history.clear / autosave.reset / tree.clear / inspector.clear（[hierarchy.rs](../engine/crates/editor/src/panels/hierarchy.rs#L243-L264)）。

### 4.3 打开场景 / Prefab

- 资源面板：点场景/`.prefab` → `open_scene_resource(relative)`。
- Hierarchy 下拉：同上。
- `open_scene_document`（磁盘加载）→ `set_current_scene_document`；`open_scene_resource` 再补全清理（见 2.1 最强入口）。
- 双击/拖放资源在 Scene View 内也有实例化入口（`spawn_prefab_from_drop` 等，scene_view.rs L1799-L1953）。

### 4.4 项目切换 / 打开项目

- Header “项目 → 打开项目 / 最近项目” → `start_import()`（后台线程）。
- 完成后 `apply_progress_result(ImportProject)` 整体替换（见 2.1），自动关闭进度面板。

### 4.5 编辑器关闭

- eframe 关闭；未看到显式的 save-on-exit 流程，依赖 recovery 兜底（下一次启动扫描 .temp）。

### 4.6 Play / Stop

见 [game_view.rs](../engine/crates/editor/src/panels/game_view.rs#L20-L79)：`Editing → Starting → Playing / StartFailed → Stopping → Editing`。Stop 丢弃运行态（编辑器文档回到 editor_snapshot），清 tree/inspector/输入，回 Scene tab；**不清 scene_state.selected/collapsed/hidden/locked**。

---

## 5. 状态流转表

> 约定：`A ⇒ B` 表示从 A 状态变到 B。状态轴：文档身份（dirty/path/revision）、History（栈状态）、Autosave/Recovery、PlaySession、各选择集、collapsed/hidden/locked、SceneView 缓存。

### 5.1 编辑操作（就地修改当前文档）

| 操作 | History | dirty/revision | Autosave | 选择 | Inspector 快照 | SceneView | 元数据 | 说明 |
|---|---|---|---|---|---|---|---|---|
| history.execute 成功（通用编辑） | +1 条 | dirty，revision+1 | tick 处理 | 不变 | 调用点选择性 sync_inspector | 视调用点；无统一失效 | 不自动 sync | — |
| history.execute no-op | 不加 | 视闭包是否 mark_dirty | — | 不变 | — | — | — | PartialEq 忽略 revision |
| undo | -1，恢复到 before | 变回 before | — | **不变（可能悬空）** | **过期（无刷新）** | **不过期** | **不重同步** | 无任何后同步 |
| redo | +1，恢复到 after | 变回 after | — | 同 undo | 同 undo | 同 undo | 同 undo | — |
| Inspector 输入提交（apply_inspector） | 由 history 包住则 +1 | dirty | — | 不变 | 刷新 | — | — | set_* 幂等守卫 |
| hidden/locked 切换 | **不进 History** | dirty | — | — | — | — | 直接写 metadata + mark_dirty | persist_hierarchy_flag |
| Tilemap 绘制 | 单条合并 | dirty | — | — | — | 视 tile 工具 | — | 释放/切工具提交 |
| 2D/3D 拖拽提交 | 同类相邻合并 | dirty | — | 不变 | 刷新首个 | **不失效（直接同帧渲染）** | — | 成功后**同步 save_scene** |

### 5.2 文档级切换

| 入口 | 文档身份 | History | Autosave | PlaySession | tree/inspector | scene_state 多选 | collapsed | hidden/locked | delete_target | selected_resource | 元数据 sync | SceneView |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `open_scene_resource`（场景/Prefab） | 替换 | clear | reset | =None | clear×2 | clear | clear | **保留** | =None | clear | **手动 sync** | 失效 |
| `create_scene`（新建） | 替换（新 dirty 无 path） | clear | reset | 未动（Editing 下通常 None） | clear×2 | **保留** | **保留** | **保留** | **保留** | **不调** | 失效 |
| `recover_candidate`（恢复） | 替换（dirty，path=original） | clear | reset | 未动 | clear×2 | **保留** | **保留** | **保留** | **保留** | **不调** | 失效 |
| Play `stop()` | 替换为 editor_snapshot | 不变（History 里是编辑快照） | — | 销毁 | clear×2 | **保留** | **保留** | **保留** | Hierarchy→None | 保留 | 失效 |
| 项目导入完成 | 整体换项目 | clear | reset | =None | clear×2 | —（项目全新） | — | — | — | — | — | 失效 |
| 纯 `set_current_scene_document` | 替换 | 不变 | 不变 | 不变 | 不变 | 不变 | 不变 | 不变 | 不变 | 不变 | 不调 | 失效 |

> **结论**：各入口清理力度不齐，唯 `open_scene_resource` 做“全清理”。E02 需要把“文档被替换后：selection repair + inspector 刷新 + SceneView 失效 + GameView 失效 + hierarchy metadata 刷新”固化成统一 `on_document_changed(change)`，并补上 E02 列出的变更类型（删除对象、重命名、Prefab 应用、场景打开）。

### 5.3 Play 会话对 current_scene() 的影响

| 阶段 | current_scene() 来源 | 可编辑 | 编辑器文档 | 说明 |
|---|---|---|---|---|
| Editing | document.current_scene | 是 | 当前 | — |
| Starting | document.current_scene | 由 is_playing() 防呆 | 当前 | — |
| Playing | runtime_document | 否（is_playing） | editor_snapshot 保持不变 | Scene/Hierarchy/Inspector 面板会解析到 runtime_document（不可编辑，但读的是运行时数据） |
| Stopping/Stop | editor_snapshot（session.stop() 返回） | 是 | 恢复 | 运行态修改被丢弃 |

---

## 6. 现有问题复现清单（供 E02–E10 引用）

> 每条带编号、影响面、复现路径、所属修复条目。**本清单不修复**。

### P1：切换/新建/恢复入口不清理多选与 collapsed（E02/E06）
- 复现：多选若干实体（scene_state.selected 非空）或折叠部分节点后，Header“编辑 → 新建场景”，或恢复崩溃快照；`create_scene`/`recover_candidate` 只 clear tree/inspector，`scene_state.selected/collapsed` 仍保留旧 id。
- 影响：切到无该实体场景后，多选集合与 collapsed 指向不存在的实体；后续 delete/hierarchy 高亮读旧 id。
- 修复归属：E02（统一入口清理）+ E06（切换保护）。

### P2：undo/redo 不做任何后同步（E04）
- 复现：删除一个实体后 Ctrl+Z；`history.undo` 只 clone-swap 文档。若此前 inspector 指向被删实体或 scene_state.selected 含被删 id，undo 后不自动刷新；redo/undo 后 Inspector 面板显示陈旧快照、场景视图缓存不过期。
- 影响：Inspector 指向已删除对象 / 画布显示旧数据（正是 E02 验收要消灭的现象）；undo/redo 后连续编辑基于过期 inspector 字段。
- 修复归属：E04（undo/redo 后统一调 on_document_changed）；E03 提供 selection repair。

### P3：`set_current_scene_document` 不 reset scene_state（含 preview_ui）/resource/delete_target（E02）
- 位置：[actions/document_io.rs](../engine/crates/editor/src/actions/document_io.rs) + `invalidate_scene_view_state`（L29-L41）。
- `invalidate_scene_view_state` 清理了 preview / preview3d_transform / gizmo_drag / scene_runtime / cache，但 **不清理 `preview_ui`（SceneEditorState 的 BTreeMap 字段）**、selected/collapsed/hidden/locked、delete_target、resource selection、tree/inspector。
- 影响：任何直接调 `set_current_scene_document` 的路径（recover/stop/create_scene/undo 内部不调）都存在悬空；若文档被替换为不含旧实体集合的场景，preview_ui 残留旧 id。
- 修复归属：E02（把清理收敛进 on_document_changed，按 change 类型决定清理范围）。

### P4：metadata 持久化绕过 History 与保存路径（E02/E04 边界）
- 位置：[hierarchy.rs](../engine/crates/editor/src/panels/hierarchy.rs#L204-L214) `persist_hierarchy_flag`。
- 复现：toggle 某实体 hidden/locked → 直接 `scene.scene.metadata` insert/remove + `mark_dirty()`。**不进 History**（undo 无法撤销），且属于“编辑配置写进场景数据”的边界问题。
- 影响：hidden/locked 不可撤销；save 时随场景写盘（与 E10“持久化展开状态到编辑器配置、不写入场景数据”冲突方向相同）。
- 修复归属：E10（区分场景数据与编辑配置）+ 批次内对“该不该进 History”统一口径。

### P5：`sync_hierarchy_metadata` 只在 (path, revision) 变化时重跑，且多个替换入口漏调（E02/E04）
- 位置：[hierarchy.rs](../engine/crates/editor/src/panels/hierarchy.rs#L170-L202)。
- `last_metadata_key` 命中后直接 return。切换场景后若 key 相同不会自动重跑（例如两个刚打开的 revision=0 文档、或 stop 后恢复的文档 revision 与 key 撞上），因此部分入口（create_scene/recover/stop）没有手动调用它，hidden/locked 集合可能与新文档不符。
- 影响：hidden/locked 状态陈旧。
- 修复归属：E02（on_document_changed 统一 sync）+ E04（undo/redo 后同步）。

### P6：拖拽/绘制/对齐等提交后立即 save_scene，与“编辑期间不持续写盘”目标相反（E08）
- 位置：[scene_view/pointer.rs](../engine/crates/editor/src/panels/scene_view/pointer.rs)、[scene_view/pointer.rs](../engine/crates/editor/src/panels/scene_view/pointer.rs)（scene_transform / 3D gizmo 提交后 `self.save_scene()`）。
- 复现：拖动实体一下 → 每帧/每次 release 都 `history.execute` + `save_scene`（整份场景 validate+atomic 写盘 + clear_for）。
- 影响：大场景拖动卡顿、磁盘频繁写、自动保存语义被“手动保存”抢占；undo 到拖前状态后磁盘却已是拖后内容。
- 修复归属：E08（拖拽/数值拖动/颜色/Tilemap 接入 EditTransaction，提交后再异步自动保存；交互中不写盘）。

### P7：快捷键无焦点分类（E05）
- 位置：[main.rs](../engine/crates/editor/src/main.rs#L932-L952)。
- 复现：在 Inspector 文本编辑框（entity_name/脚本路径/Text 内容）内按 Ctrl+Z / Ctrl+Y / Ctrl+S → 无条件触发 `undo()/redo()/save_scene()`（`wants_keyboard_input` 未用于 gating）；Delete 仅当 `!context.wants_keyboard_input()` 才执行。
- 影响：文本控件编辑时误触发全局撤销/保存；文本内撤销与场景撤销互相串扰；Delete 的 gate 方式与其他键不一致（`wants_keyboard_input` 是全局启发式，无法区分“编辑框内 Delete 应删字符”）。
- 修复归属：E05（焦点分类：文本/数字/弹窗/Scene/Hierarchy/普通面板；无文本焦点才执行对象级操作）。

### P8：Dirty 文档离开无确认（E06）
- 位置：场景下拉切换（hierarchy.rs L56-L58）、新建场景（header.rs L56-L58）、恢复（header.rs L169-L177）、项目切换（import.rs start_import / 打开项目）。
- 复现：对当前场景做编辑（dirty=true）后直接在下拉切到另一个场景 / 新建场景 → 当前 dirty 被静默丢弃；无“保存并继续 / 放弃并继续 / 取消”。
- 影响：数据丢失。
- 修复归属：E06（`request_document_change()` 流程覆盖场景切换/新建/打开 Prefab/其它文档/切换项目/关闭编辑器）。

### P9：Play/Stop 丢失编辑上下文（E09）
- 位置：[game_view.rs](../engine/crates/editor/src/panels/game_view.rs#L60-L79) stop()。
- 复现：Play 前选中实体 A、折叠/展开某些节点、工具为 Move；Play→Stop 后 tree/inspector 被 clear，`scene_state.selected` 若还保留则又可能指向运行时 id（编辑文档无该对象时悬空）。
- 影响：Play/Stop 前后视图与选择不一致（违反 E09 目标）；但当前测试基线把“stop 后 tree/inspector = None”锁成语义（见 game_view 测试 `sample_project_stop_restores_scene_tab_and_clears_preview_input` 等），升级需同步改测试。
- 修复归属：E09（保存编辑上下文、按 id 与文档有效性恢复、运行期删除对象安全回退）；依赖 E03/E04。

### P10：Hierarchy 过滤早期 return 会隐藏“含匹配后代”的祖先（E10）
- 位置：[hierarchy.rs](../engine/crates/editor/src/panels/hierarchy.rs#L97-L103)。
```rust
if !self.hierarchy_filter.trim().is_empty()
    && !name.to_lowercase().contains(&self.hierarchy_filter.to_lowercase())
{
    return;   // ← 直接整棵子树隐藏
}
```
- 复现：结构 A/B，B 名称含关键字，过滤关键字 → A 因为自身不匹配被 return，B 永远显示不出来。
- 影响：深层节点搜索不到。
- 修复归属：E10（`matches(node) || has_matching_descendant(node)`，匹配后代时保留祖先并自动展开）。

### P11：删除后 selection repair 分散且不完整（E03/E04）
- 位置：`delete_selected_entity`（[inspector.rs](../engine/crates/editor/src/panels/inspector.rs#L1261-L1291)）删除成功后才对 tree.selected 做一次回退检查；`scene_state.selected` 内残留已删 id 无统一清理；undo 恢复已删实体时选择也不回补。
- 影响：删除某实体后，多选集合/资源选择/Inspector 高亮可能仍引用已删 id；再框选或加选时行为异常。
- 修复归属：E03（统一 SelectionModel + 引用清理 + SelectionChanged）+ E04。

### P12：Play 期间面板读 runtime_document 但无统一编辑锁/引用护栏（E09/E02）
- `current_scene()` 在 Play 时返回 `runtime_document`；面板 UI（Hierarchy/Inspector/Scene View）共用这套解析。多数编辑入口用 `is_playing()` 防呆，但并非所有读路径都做“运行时不存在的 id”护栏（尤其 scene_state.selected / collapsed 来自编辑上下文却读运行时）。
- 影响：潜在悬空引用与编辑/读取不一致。
- 修复归属：E09（隔离编辑文档与运行时副本，选择上下文带来源与有效性）。

### P13：Prefab / Inspector 批量提交带 select 状态但元数据（hidden/locked/展开）不随 Undo 恢复（E04/E10）
- 部分批量编辑（Prefab Apply、add_entity_with_component 等）成功后只 sync_inspector/select，undo/redo 恢复的是纯 scene 数据；与 hidden/locked 相关的元数据在 undo 后由 P5 一并陈旧。
- 影响：状态与数据不同帧一致（违反 E02 验收“同一份数据”）。

---

## 7. 测试与验收基线（供后续回归引用）

### 7.1 现有单测中会被 E02–E10 触碰的“基线锁”

| 测试 | 文件 | 锁定的语义 | 说明 |
|---|---|---|---|
| `sample_project_stop_restores_scene_tab_and_clears_preview_input` | game_view.rs 测试 | stop 后 `tree.selected=None`、`inspector.selected=None`、回 Scene tab、输入清空 | E09 需保留或按新语义升级 |
| `play_stop_can_repeat_one_hundred_times` 等 | document.rs 测试 | PlaySession 1:1 生命周期 | E09 不得破坏 |
| `mark_dirty_bumps_revision_but_equality_ignores_it` | document.rs 测试 | PartialEq 忽略 revision | E04/E07 依赖此 no-op 判定 |
| `open_scene_resource_invalidates_scene_view_state` | actions.rs 测试 | open_scene_resource 后 Scene View 缓存重建 | E02 统一入口需保持 |
| `app_startup_scans_recovery_and_recover_candidate_restores_scene` | header.rs 测试 | recover 后 tree/inspector 空、状态含“已恢复崩溃快照”、不覆写正式文件 | E02/E06 需保持恢复后一致性 |
| hierarchy/delete/reparent 单测 | scene_tree_state.rs | delete 回退到父级、reparent 环检测 | E03/E10 不得破坏 |

### 7.2 E02–E10 建议新增回归方向（对应 docs/28 §9.1）

1. 文档切换 Dirty 状态与保存确认（E06）。
2. 统一变更入口后：删除选中节点 / Prefab Apply / Undo / Redo / 运行时修改应用后 Hierarchy+Inspector+SceneView+GameView 同帧一致（E02/E04）。
3. SelectionModel 单选/多选/删除回退/无效引用清理/跨场景（E03）。
4. 连续 Undo/Redo 后数据+选择+Inspector+画布一致（E04）。
5. 文本焦点内 Ctrl+Z/Y/S 与 Delete 不触发对象级操作（E05）。
6. EditTransaction：一次连续操作 ≤1 条 Undo、取消不产生历史与 dirty（E07/E08）。
7. Play/Stop 上下文恢复与安全回退（E09）。
8. Hierarchy 过滤祖先路径、循环引用检查与重挂载 Undo（E10）。

---

## 8. 结论（给批次 A 的开工建议）

1. **E02 的参考实现已存在**：`open_scene_resource` 是全清理范本；把“文档替换/修改”统一进 `on_document_changed(change)`，并按 P3/P5/P11 补选择修复 + metadata 重同步。
2. **E04 是纯增量**：在 undo/redo 后补一次 on_document_changed 即可，风险集中在“undo 恢复出的 id 是否仍在场景”的 selection repair 语义。
3. **E05 独立且低风险**：只影响主循环快捷键 gate。
4. **E08 需要先决定“拖拽中写盘”策略**（P6）：当前 commit 即 save 的行为被多个手测/单测路径依赖，改成“提交后自动保存”要同步调整场景视图与 Inspector 的保存调用。
5. **E09 会触碰现存旧测试基线**（见 7.1），需连同测试一起升级。
6. **E10 从 P10 出发**：先修过滤祖先路径，再叠重挂载事务与上下文菜单。

（本文档不改动任何编辑器代码；后续条目按 E02→E03/E04/E05→E06→E07→E08→E09→E10 顺序实施。）
