# M12 阶段 1 验收证据（资源状态模型 + Importer 增量导入）

日期：2026-08-25
基线：`354750c`（M11 验收矩阵收尾）
范围：按 `docs/30-编辑器可靠性-2D工作流-资源系统闭环实施计划.md` 与已批准计划
（M12 阶段 1，5 个 C 提交）执行 M12-00 资源状态模型 + M12-01 Importer 增量导入。

## 总状态

**代码/自动化侧：PASS（5 提交质量门全过，workspace 测试全绿）。**
**编辑器后台导入实机 GUI 交互：PENDING（待负责人实机复核「刷新 → 自动后台导入 → 状态/错误可见、进度窗取消」）。**

## 提交清单（5 个 C）

| 提交 | 内容 |
|---|---|
| `f2529a4` | C1 统一 `ResourceKind` 到 engine core，editor/assetc 切换共用 |
| `82a1212` | C2 `ResourceState` 九态状态机 + 资源库 TSV v2（v1 迁移与损坏恢复） |
| `9e98deb` | C3 `content_hash` + `reconcile` 磁盘变更检测 |
| `6d45401` | C4 Importer trait 注册表 + ImportCache 重构 + assetc 状态回写 |
| `38aaf4b` | C5 编辑器后台资源导入（pending 驱动 + 进度面板 + 取消） |

## M12-00 资源状态模型

- `engine/src/core/resource_kind.rs`：`ResourceKind` 9 族（Texture/Audio/LuaScript/Scene/Prefab/Atlas/Model/Font/Other），
  `from_path` 永不 Err、`label` 中文、`token` 持久化、`parse` 反解，`Hash` derive。
- `engine/src/core/resource_db.rs`：`ResourceState` 6 持久化 + 3 瞬态；`AssetRecord` 扩展
  kind/content_hash/last_modified/last_success/last_error_at；DB 头 `# rustlua-resource-db-v2`（12 列 TSV）；
  v1 自动迁移（registered→discovered、imported→ready、failed→import_failed、missing→missing）；
  `open_recoverable` 损坏库改名 `.corrupt-<ts>` 重建；`register` 已存在路径刷新 kind/version；
  `mark_ready`/`mark_failed` 回写。
- `reconcile`：mtime 变更→Stale、路径消失→Missing、移出扫描根→Orphaned、恢复→Ready/Discovered；
  Windows 反斜杠扫描路径归一化对齐 register 正斜杠键（测试 `reconcile_normalizes_backslash_scan_paths`）。
- 自动化测试：`resource_kind` 映射全表/大小写/Other/token roundtrip、`ResourceState` roundtrip/瞬态拒绝、
  DB v2/v1 迁移/损坏重建/未知版本拒绝、reconcile 各迁移路径。

## M12-01 Importer 增量导入

- `engine/src/core/importer.rs`：`Importer` trait + 8 内置 importer + `ImporterRegistry::builtin()`（OnceLock 单例）；
  `run_import_batch(project, db, cache, registry, items, cancel) -> ImportSummary`——单坏项不 abort、
  逐项查 cancel、末尾 save 一次、`elapsed_ms`。
- `engine/src/core/import_cache.rs`：`import_file(id, source, &dyn Importer)`，指纹+版本+kind 命中复用，
  validate→produce→原子写全部 artifact；删除 ImportKind/validate_source 分派。
- `engine/src/core/hash.rs`：`content_hash`（FNV-1a 派生 128 位，32 十六进制）与 DB 共用。
- `assetc/src/lib.rs`：`PACKAGE_VERSION = "assetc-package-1"`；import() 走 batch + 回写 + 每项失败不 abort。
- 编辑器：`resource_browser_state` refresh 产出 `pending_imports`；`publish.rs` 后台线程跑 batch
  （自开 DB/缓存实例，原子写盘）；`progress.rs` `ProgressKind::ResourceImport` + `cancel`（关进度窗置 `AtomicBool`）；
  完成后 `apply_progress_result` 按 kind 分发 → 重扫读取最终状态。
- 自动化测试：各 importer 好/坏夹具（含 JPG 内联字节、WAV RIFF 头、OGG 流式 lenient）、batch 计数/取消/单坏不阻断、
  `refresh_produces_pending_imports_for_discovered_files`、`staled_files_are_pending_even_after_attempt`、
  `failed_imports_are_not_automatically_retried_after_attempt`。

## 实机资产验证

`assetc import game_test`（C4 归档验证）：31 个资源全部 `ready`，`imported=0 reused=31 failed=0`——
真实项目全量缓存命中，导入链路闭合。

## 质量门结果（每 C 提交后执行）

| 项 | 结果 |
|---|---|
| cargo fmt --all -- --check | 通过 |
| cargo clippy --workspace --all-targets --all-features -- -D warnings | 通过（0 警告） |
| cargo test --workspace --all-features | 通过（engine 262 + editor 90 + game 6 + assetc 等全绿） |
| cargo build --workspace --release | 通过 |
| git diff --check | 通过 |

## 实机待办（PENDING，需负责人）

- 编辑器打开 game_test 项目 → 资源面板刷新 → 观察自动后台导入进度窗 → 状态列显示 Ready/错误；
- 关闭进度窗触发取消（后台任务尽早停止、不写半成品缓存）；
- 修改/新增资源文件后刷新 → 状态转 Stale/Discovered → 自动重导 → Ready。

## 后续阶段（不在本次）

M12-02 依赖图失效传播、M12-03 资源浏览器生产化、M12-04 校验报告（含 webp/mp3）、M12-05 迁移恢复框架。
