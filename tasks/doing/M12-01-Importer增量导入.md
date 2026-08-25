# M12-01 Importer 增量导入

## 状态

ACCEPTED（代码与自动化验证）；编辑器后台导入实机 GUI 观察待专项实机确认（资源面板刷新 → 自动后台导入 → 状态/错误可见）。

## 目标

建立 `Importer` trait 与注册表，让导入链路闭合：assetc 增量导入回写 DB、每项失败不阻断批次；编辑器在资源刷新后自动后台导入待导入项，不阻塞 UI，可取消，完成后重扫读取最终状态。

## 背景

M12-00 统一了状态模型，但导入仍硬编码分派：编辑器从不调用 `ImportCache::import_file`（只注册不导入），`assetc::import` 单坏资源让整批 `return Err` 且不回写 DB。M12-01 引入 Importer 抽象与批量运行器，闭合 assetc 与编辑器两条导入路径。

## 输入

- `docs/15-import-cache.md`（本次升级）；
- `engine/crates/engine/src/core/importer.rs`；
- `engine/crates/engine/src/core/import_cache.rs`；
- 计划文件 M12 阶段 1（C4/C5）。

## 不做什么

- 不做语法级 Lua 校验（M12-04）；不做 webp/mp3 支持（M12-04）；
- 不做依赖图失效传播（M12-02）；不做资源浏览器生产化（M12-03）；
- 不引入多写者并发锁（单写者串行 + 原子 rename）。

## 技术方案

- `Importer` trait：`version()`/`kind()`/`extensions()`/`validate(source, bytes)`/`produce(bytes, id, hash) -> Vec<ImportArtifact>`（默认原样拷贝为单个 `.cache/cache/{id}-{hash}.bin`）；
- 8 个内置 importer（Texture=image 真实解码、Audio=rodio 解码+WAV 读头、Script 非空、Scene `.scene` 严格/ron/json 宽松、Prefab 严格、Model=GltfAsset、Atlas 占位、Font=FontArc）；
- `ImporterRegistry::builtin()`（OnceLock 单例）+ `get(kind)` + `from_path(path)`；
- `run_import_batch(project, db, cache, registry, items, cancel) -> ImportSummary`：逐项导入/回写、单坏项不 abort、每项间查 cancel、末尾 save 一次、elapsed_ms；
- `ImportCache::import_file(id, source, &dyn Importer)`：指纹+版本+kind 命中复用，validate→produce→原子写全部 artifact；
- assetc `import()` 走 batch + 回写 + 每项失败不 abort，`PACKAGE_VERSION` 从 registry 取版本；
- 编辑器后台导入：refresh 产出 `pending_imports`（kind≠Other 且状态 ∈ {Discovered, Stale, ImportFailed 且未尝试过}），每帧无任务时后台线程跑 batch（自开 DB/缓存实例），`ProgressPanel` 显示进度、关窗置 `AtomicBool` 取消，完成后重扫。

## 修改范围

- `engine/crates/engine/src/core/importer.rs`（新增）；
- `engine/crates/engine/src/core/import_cache.rs`（重构，删 ImportKind/validate_source）；
- `engine/crates/engine/src/core/hash.rs`（content_hash 共用）；
- `engine/crates/assetc/src/lib.rs`（PACKAGE_VERSION + batch 闭合）；
- `engine/crates/editor/src/resource_browser_state.rs`、`progress.rs`、`publish.rs`、`main.rs`。

## 验收标准

- 各 importer 好/坏夹具验证（含 JPG 内联字节、WAV RIFF 头、OGG 流式 lenient）；
- batch 二次全 reused；单坏项不阻断；cancel 可停；
- assetc import 后 DB 状态 ready（真实项目 31 项全 ready，reused=31）；
- 编辑器 pending 列表正确（新增/Stale 待导入，ImportFailed 不无限重试）；
- 实机 GUI：刷新资源 → 自动后台导入 → 状态/错误可见，进度窗可关闭取消（待负责人确认）。

## 测试命令

```powershell
cd engine
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace --release
git diff --check
```

## 验收证据

- 提交：C4 `6d45401`、C5 `38aaf4b`；
- 测试覆盖：各 importer 好/坏夹具、batch 计数/取消/单坏不阻断、`refresh_produces_pending_imports_for_discovered_files`、`staled_files_are_pending_even_after_attempt`、`failed_imports_are_not_automatically_retried_after_attempt`；
- 实机：game_test 31 个资源全部 Ready（imported=0 reused=31 failed=0）；
- workspace 测试全绿（engine 262 + editor 90 + game 6 + assetc 等）。

## 风险

- `.ron/.json` 宽松校验避免遗留场景全挂；坏资源被跳过而非阻断；
- ImportFailed 不无限自动重试（`import_attempted` HashSet；文件变化转 Stale 后重新 pending）——设计取舍，避免静默死循环；
- 并发：单写者串行 + 原子 rename，无锁。

## AI 输出

- 已完成：Importer trait/注册表/batch、ImportCache 重构、assetc 闭合（PACKAGE_VERSION）、编辑器后台导入（pending 驱动 + 进度面板 + 取消）；
- 验证：质量门全过，实机 game_test 全 ready；
- 未完成：编辑器后台导入的实机 GUI 交互、进度窗取消的实机点击（需负责人实机复核）。

## 已知限制

不得将未完成内容描述为完成。实机 GUI 验收未做，状态不标 PASS；ImportFailed 自动重试边界为设计取舍，已在测试中固化。
