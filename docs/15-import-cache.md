# M4-02 导入缓存（M12-01 升级为 Importer 注册表 + 批量导入）

导入缓存位于项目根目录 `.cache/cache`，元数据位于 `.cache/import-cache.db`。缓存键由资源 UUID、源文件内容指纹、导入器版本和资源类型共同决定。

## Importer 抽象（M12-01）

每种 `ResourceKind` 对应一个内置 `Importer`（`rustlua_assets::importer::ImporterRegistry::builtin()`），职责分三段：

```rust
pub trait Importer: Send + Sync {
    fn version(&self) -> &'static str;   // 如 "texture-1"，升级强制全量重导
    fn kind(&self) -> ResourceKind;      // 统一资源族
    fn extensions(&self) -> &'static [&'static str];
    fn validate(&self, source: &Path, bytes: &[u8]) -> Result<(), String>;
    fn produce(&self, bytes: &[u8], id: AssetId, hash: &str)
        -> Result<Vec<ImportArtifact>, String>;  // 默认原样拷贝为单个 .bin
}
```

### 内置校验要点

| Kind | 扩展名 | validate |
|---|---|---|
| Texture | png, jpg, jpeg | `image::load_from_memory` 真实解码 |
| Audio | wav, ogg | `rodio::Decoder::new`；WAV 强制 `total_duration` 读头，OGG 流式格式解码器初始化成功即视为有效 |
| LuaScript | lua | `validate_lua_syntax` 只编译不执行，错误含行列定位（非空检查为其前置） |
| Scene | scene, ron, json | `.scene` 严格 `Scene::from_text`；`.ron/.json` 宽松仅非空 |
| Prefab | prefab | 严格 `Prefab::from_text` |
| Model | gltf, glb | `GltfAsset::load` 真实解析 |
| Atlas | atlas | 严格 `AtlasDefinition::from_text`（`atlas-2`）：缺版本/缺 `texture`/无 region/零尺寸/空名/语法损坏都定位到行 |
| Font | ttf, otf | `ab_glyph::FontArc::try_from_vec` |

`ImportCache::import_file(id, source, &dyn Importer)`：读源字节 → `content_hash`（FNV-1a 派生 128 位，32 位十六进制）→ 缓存命中（指纹 + importer 版本 + kind token 全匹配且产物文件在盘）直接复用 → 否则 `validate` → `produce` → 原子写入所有 artifact（`.tmp` + rename）。`webp/mp3` 本阶段归 `Other`，支持留 M12-04。

## 批量导入（`run_import_batch`）

`rustlua_assets::importer::run_import_batch(project, db, cache, registry, items, cancel) -> ImportSummary`：

- 逐项 `get(ResourceKind::from_path)` → `skipped`（Other）或 `import_file`；
- 成功 → `mark_ready(id, cache_path, content_hash)`；失败 → `mark_failed(id, error)`；
- **单坏项不 abort**，继续下一项；每项间检查 `cancel: &AtomicBool` 可干净停止；
- 结束后 DB `save()` 一次；返回 `ImportSummary { total, imported, reused, failed, skipped, elapsed_ms, results }`。

`PACKAGE_VERSION`（当前 `assetc-package-2`）替代旧 `IMPORTER_VERSION`；assetc 与编辑器统一从 registry 取 importer 版本，消除版本双源漂移。

## 增量语义

- 未变化资源（指纹 + 版本 + kind 全命中）复用缓存，`reused=true`；
- 源文件或 importer 版本变化 → 重新导入；
- 失败不替换有效缓存（validate 通过才写盘，原子 rename）；
- 清空缓存可重建；首批资源扩展名可识别。

## 验收命令

```powershell
cargo test --workspace --all-features
cargo run -p engine --example import_cache_demo --release
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

示例应显示首次导入、缓存命中、源文件变化后的重新导入，以及清空缓存后的重建均未复用旧结果。

## M12-01 相关提交

- C1 `f2529a4`：统一 `ResourceKind`；
- C3 `9e98deb`：`content_hash` 指纹迁移至 `core/hash.rs`；
- C4 `6d45401`：Importer trait + 注册表 + ImportCache 重构 + assetc 闭合（真实项目 31 项全部 ready，reused=31）；
- C5 `38aaf4b`：编辑器后台批量导入（pending_imports → run_import_batch → 进度面板）。
