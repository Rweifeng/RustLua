# M4-01 资源数据库

资源数据库位于项目根目录 `.p2/resources.db`。每条记录拥有稳定 UUID，记录源文件相对路径、导入状态、依赖 UUID、导入器版本、缓存路径和最近一次错误。

## 设计约定

- 数据库只保存项目相对路径，避免机器绝对路径导致项目不可移植；
- `move_resource` 只更新路径索引，不生成新 UUID，因此场景引用不会丢失；
- `register` 对同一路径幂等；
- 导入失败使用 `Failed` 和 `error` 字段保留诊断信息；
- 删除时返回依赖该资源的 UUID，供编辑器阻止删除或显示提示；
- `save` 先写临时文件再替换正式文件，避免半写入数据库。

## 示例

```powershell
cargo run -p game_runner --example resource_db_demo --release
```

输出应包含 `resource_db_demo_ok`、移动前后相同 UUID、失败状态和依赖者数量。