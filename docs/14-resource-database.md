# M4-01 资源数据库（M12 升级至 Schema v2）

资源数据库位于项目根目录 `.cache/resources.db`。每条记录拥有稳定 UUID，记录源文件相对路径、资源类型、生命周期状态、导入器版本、缓存路径、内容指纹与最近一次导入时间。

## Schema v2（M12-00）

v2 由 v1 的 7 列升级为 **12 列 TSV**，文件头为 `# rustlua-resource-db-v2`：

| 列 | 字段 | 说明 |
|---|---|---|
| 1 | id | 资源稳定 UUID（路径+版本种子派生，移动/重命名不丢失） |
| 2 | source_path | 项目相对路径（正斜杠，可移植） |
| 3 | kind | 资源类型 token（`texture/audio/script/scene/prefab/atlas/model/font/other`） |
| 4 | status | 生命周期状态（见下） |
| 5 | importer_version | 导入器版本（`texture-1` 等，来自 ImporterRegistry） |
| 6 | cache_path | 缓存产物路径（项目相对，`.cache/cache/...`） |
| 7 | content_hash | 上次成功导入的内容指纹（未导入为空串） |
| 8 | last_modified | 源文件 mtime（unix 纳秒），变更检测用 |
| 9 | last_success | 上次成功导入的 unix 秒 |
| 10 | last_error_at | 上次失败导入的 unix 秒 |
| 11 | dependencies | 依赖资源 UUID 逗号分隔 |
| 12 | error | 最近一次错误信息（无错误为空串） |

### 兼容与恢复

- `load()` 先读文件头：v2 → 12 列解析；v1 → 自动迁移（状态映射 `registered→discovered`、`imported→ready`、`failed→import_failed`、`missing→missing`，kind 由路径推导，新增字段给默认值）；未知版本 → `UnsupportedSchemaVersion`；列数不符 → `Malformed`。
- `open_recoverable(root)`：`open` 失败时把损坏库改名 `.corrupt-<时间戳>`，返回空库重建（完整备份/回滚属 M12-05）。
- `save()` 先写 `.tmp` 再原子替换正式文件，避免半写入。

## 生命周期状态（ResourceState）

七种**持久化**状态 + 三种**瞬态**状态（不落盘，重启折回祖先）：

| 状态 | 含义 |
|---|---|
| `discovered` | 磁盘上已发现，尚未导入 |
| `ready` | 上次导入成功，缓存产物可用 |
| `import_failed` | 上次导入失败，`error` 字段保留诊断 |
| `stale` | 源文件在磁盘上已变化（mtime 变更），重导可修复 |
| `missing` | 源文件已不在磁盘 |
| `orphaned` | 源文件移动到扫描根之外 |
| `invalid` | 源文件没变但**依赖断了**（依赖缺失/导入失败/已变更），重导自身无法修复（M12-02） |
| `import_queued` / `importing` / `removed` | 瞬态，save 时折回 `discovered` 或丢弃 |

`invalid` 与 `stale` 语义分离：`stale` = 自己变了、重导可修复；`invalid` = 依赖断了、等依赖恢复后传播自动撤销。`invalid` 不进入编辑器的待导入队列（重导消费者无用），依赖恢复后 `propagate_invalidation` 撤销回 `ready`/`discovered`。

### 状态机（`reconcile(&[DiskAsset], scan_roots)`）

`reconcile` 只读磁盘元数据（mtime），不算 hash：

- 新增文件 → 由 `register` 建立记录，状态 `discovered`；
- mtime 变化且原状态为 `ready/stale/import_failed` → `stale`；
- mtime 变化且原状态为 `invalid` → `stale`（源内容变化时物理态优先，重导后可恢复，M12-02）；
- 路径不在扫描结果 → `missing`；不在扫描根 → `orphaned`；
- 恢复文件（missing/orphaned 重新出现）→ 有 content_hash 回 `ready`，否则 `discovered`；
- Windows 上 `collect_files` 返回反斜杠路径，`reconcile` 会归一化为正斜杠再与索引比对。

`content_hash` 是"上次成功导入的内容指纹"，`last_modified` 管变更检测、hash 管缓存复用：touch 文件 → `stale` → 导入后 hash 相同 → 复用产物 → `ready`。

## 设计约定

- 数据库只保存项目相对路径，避免机器绝对路径导致项目不可移植；
- `move_resource` 只更新路径索引，不生成新 UUID，因此场景引用不会丢失；
- `register(source, kind, importer_version)` 对同一路径幂等，且**已存在路径也会刷新 kind/version**（修复旧版版本漂移）；
- `mark_ready(id, cache_path, content_hash)` / `mark_failed(id, error)` 回写导入结果；
- 导入失败使用 `import_failed` 和 `error` 字段保留诊断信息；
- 删除时返回依赖该资源的 UUID，供编辑器阻止删除或显示提示。

## 依赖图与失效传播（M12-02）

### 依赖边

`dependencies` 列存依赖资源的 UUID（逗号分隔）。`recompute_edges(db, read_source)` 每次对全部 Scene/Prefab/LuaScript 记录重新提取引用并**全量覆盖**依赖边（幂等）；未解析的引用返回为 `UnresolvedRef`，调用方按断链处理，但不建边。

提取规则（`dependency.rs` 纯函数，引号感知状态机，天然排除注释）：

- **Scene/Prefab**：扫 `assets/` 字面量，向后到 `;`/`:`/`\n`/`\r` 截止（与运行时终止符一致），排除 `__ground_plane__`，按出现顺序去重；
- **LuaScript**：`require("x")` → scripts/x.lua；`scene.load("x")`（仅字面量）→ scenes/x.scene；变量参数（`scene.load(state.scene_name)`）→ `Dynamic` 不建边；字符串内 `assets/` 字面量 → 直接引用；
- 大小写回退：精确 `find_by_path` 未命中时 `find_by_path_ci` 兜底（Windows 书写大小写差异）；
- 自环排除：`id == self_id` 的引用跳过。

### 失效传播（`propagate_invalidation`）

rank-max 不动点，单向（依赖→消费者）、幂等、可撤销。状态等级（越大越差）：

| 等级 | 状态 |
|---|---|
| 7 | missing / orphaned（自身源缺失，物理态最强） |
| 6 | invalid（断链）/ 依赖侧 missing/orphaned/import_failed 折叠的 broken-link |
| 5 | import_failed |
| 4 | stale |
| 3 | discovered |
| 2 | ready |

- `final_rank = max(自身物理态, 最差依赖)`，落到 invalid/import_failed/stale/discovered 时改写状态并写 error 文案；
- 仅当**依赖**才是拖后腿因子（`dep_worst > 自身物理态 rank`）才写依赖 error，否则保留原 error（导入失败诊断、循环依赖文案）；
- 依赖全部健康时撤销 `invalid`：有 content_hash 回 `ready`，否则回 `discovered`；
- 调用方约束：必须在 `reconcile` 或 `run_import_batch`（内部已 mark_ready/mark_failed）之后立即调用，保证快照即物理态。

### 循环依赖（`detect_cycles`）

三色 DFS + 显式路径栈，rotate-to-min-id 规范化去重。循环是图健康告警而非断链本身：cycle error 先写入，传播只在状态变化时写 error，不覆盖循环文案。

### 挂接点

- 编辑器 `refresh()`：`register → reconcile → recompute_edges → propagate_invalidation → save`；`pending_imports` 排除 `invalid`（断链重导无用，等依赖恢复撤销）；
- 编辑器 `spawn_resource_import`：批次后 `propagate_invalidation + save`（导入恢复依赖 → 消费者从 invalid 撤销）；
- assetc `scan()`：与编辑器 refresh 同时序，重建条目反映 invalid/missing；`import()` 批次后传播+保存；
- assetc `--check-deps`：`check_dependencies(project)` 对每个 invalid 消费者沿依赖下探到断链根，输出 `scenes/main.scene → assets/textures/player.png (缺失)` 完整链路；成环截断标注「循环依赖」。

## 示例

```powershell
cargo run -p engine --example resource_db_demo --release
```

输出应包含 `resource_db_demo_ok`、移动前后相同 UUID、失败状态和依赖者数量。

## M12 相关提交

- 阶段 1：C1 `f2529a4`（统一 `ResourceKind`）、C2 `82a1212`（`ResourceState` + DB TSV v2 迁移）、C3 `9e98deb`（`content_hash` + `reconcile`）、C5 `38aaf4b`（reconcile 归一化反斜杠扫描路径）；
- 阶段 2：C1 `2cebeea`（`invalid` 状态 + `iter`/`consumers_of`/`find_by_path_ci`/`propagate_invalidation`）、C2 `dbdc067`（依赖提取器）、C3 `57df5c4`（传播 + 循环检测）、C4 `fb7a312`（editor/assetc 接线 + UI 状态图标）、C5 `9e7502e`（`--check-deps`）。
