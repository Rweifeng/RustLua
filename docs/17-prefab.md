# M4-04 预制体

预制体是可重复实例化的场景实体子树。它保存实体结构、组件值、资源 UUID 和元数据；实例化时会为每个实例分配新的实体 ID，避免多个实例互相覆盖。

实例覆盖使用 `(源实体 ID, 字段)` 键保存：

- `name`：覆盖实体名称；
- `component:<名称>`：覆盖组件值；
- `metadata:<名称>`：覆盖编辑器元数据。

源预制体更新策略采用显式重建：调用 `PrefabInstance::rebuild` 创建新实例并重新应用原有覆盖值。当前不做隐式热更新，避免编辑器修改被静默覆盖。

```powershell
cargo run -p game_runner --example prefab_demo --release
```