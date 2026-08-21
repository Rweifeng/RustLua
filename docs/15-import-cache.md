# M4-02 导入缓存

导入缓存位于项目根目录 `.cache/cache`，元数据位于 `.cache/import-cache.db`。缓存键由资源 UUID、源文件内容指纹、导入器版本和资源类型共同决定。

## 支持类型

- PNG；
- WAV/OGG；
- GLTF/GLB；
- Lua；
- RON/JSON 场景；
- Prefab 预制体。

第一版除 PNG 外只验证文件读取和缓存流程；具体格式解析由后续导入器任务提供。

## 验收命令

```powershell
cargo test --workspace --all-features
cargo run -p game_runner --example import_cache_demo --release
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

示例应显示首次导入、缓存命中、源文件变化后的重新导入，以及清空缓存后的重建均未复用旧结果。