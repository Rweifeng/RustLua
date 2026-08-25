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

六种**持久化**状态 + 三种**瞬态**状态（不落盘，重启折回祖先）：

| 状态 | 含义 |
|---|---|
| `discovered` | 磁盘上已发现，尚未导入 |
| `ready` | 上次导入成功，缓存产物可用 |
| `import_failed` | 上次导入失败，`error` 字段保留诊断 |
| `stale` | 源文件在磁盘上已变化（mtime 变更） |
| `missing` | 源文件已不在磁盘 |
| `orphaned` | 源文件移动到扫描根之外 |
| `import_queued` / `importing` / `removed` | 瞬态，save 时折回 `discovered` 或丢弃 |

### 状态机（`reconcile(&[DiskAsset], scan_roots)`）

`reconcile` 只读磁盘元数据（mtime），不算 hash：

- 新增文件 → 由 `register` 建立记录，状态 `discovered`；
- mtime 变化且原状态为 `ready/stale/import_failed` → `stale`；
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

## 示例

```powershell
cargo run -p engine --example resource_db_demo --release
```

输出应包含 `resource_db_demo_ok`、移动前后相同 UUID、失败状态和依赖者数量。

## M12-00 相关提交

- C1 `f2529a4`：统一 `ResourceKind`（engine core）；
- C2 `82a1212`：`ResourceState` 替换 `ImportStatus` + DB TSV v2 迁移；
- C3 `9e98deb`：`content_hash` + `reconcile`；
- C5 `38aaf4b`：reconcile 归一化反斜杠扫描路径（Windows 修正）。
