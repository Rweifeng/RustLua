# M4-03 场景格式

场景使用版本化的可读文本格式，默认扩展名为 `.scene`。第一版包含版本号、场景名称、实体树、组件数据、资源 UUID 和编辑器元数据。

保存会先写入同目录临时文件，再替换正式场景文件；加载失败不会修改已有文件。解析错误包含行号和字段，便于编辑器定位问题。

当前支持版本为 `1`。不支持的旧版本或未来版本会明确拒绝，后续迁移器应接入 `Scene::from_text` 与 `SceneError::UnsupportedVersion` 边界。

```powershell
cargo run -p game_runner --example scene_format_demo --release
```