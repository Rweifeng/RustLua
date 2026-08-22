# M4-03 场景格式

场景使用版本化的可读文本格式，默认扩展名为 `.scene`。格式包含版本号、场景名称、实体树、组件数据、资源 UUID 和编辑器元数据。

保存会先写入同目录临时文件，再替换正式场景文件；加载失败不会修改已有文件。解析错误包含行号和字段，便于编辑器定位问题。

当前支持版本为 `2`。

## 版本与迁移

- 版本 `1`：2D 组件（`Transform` / `Sprite` / `Tilemap` / `Animation` / `Collider`）。
- 版本 `2`：新增 3D 组件（`Transform3D` / `MeshRenderer` / `Camera3D` / `Light`）。

组件负载是不透明字符串，因此 v1 → v2 迁移无需改写数据：`Scene::from_text` 在解析 v1 场景时提升版本为 2，并在 `meta` 中记录 `migrated_from=1` 以便追溯。不支持的旧版本或未来版本会通过 `SceneError::UnsupportedVersion` 明确拒绝。

实体行支持 3 字段（无父实体，`entity\t<id>\t<name>`）或 4 字段（含父实体）两种写法；序列化时无父实体不再输出空尾字段，避免场景文件产生尾随空白。

```powershell
cargo run -p game_runner --example scene_format_demo --release
```

## 3D 组件字符串

沿用 `key=value;` 约定，向量用逗号分隔（参考 `Collider offset=x,y`）：

- `Transform3D`：`position=x,y,z;rotation=rx,ry,rz,rw;scale=sx,sy,sz`
- `MeshRenderer`：`mesh=assets/models/<file>.glb|gltf`（`__ground_plane__` 为内置地面哨兵路径）
- `Camera3D`：`position=x,y,z;target=x,y,z;fov=<度>;near=<n>;far=<f>`（`aspect` 由渲染窗口决定）
- `Light`：`direction=x,y,z;color=r,g,b;intensity=<i>`（方向光）
