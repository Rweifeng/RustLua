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

## UI 组件字符串（屏幕空间）

屏幕空间 UI 组件（`Text` / `Image` / `UiLayout` / `UiEvents`）同样用 `key=value;` 编码，坐标原点为
视口左上角。`space` 支持 `screen`（原始像素）与 `canvas`（1280x720 参考分辨率缩放）。

- `Text`：`text=<esc>;x=..;y=..;font_size=..;color=r,g,b;space=..;anchor=x,y`
  - `font_size`（px）：像素字号（SDF 字体下 em 高度）；`font_size=24` 表示
    2 倍渲染 scale（基准 `UI_TEXT_BASE_PX = 12`）。未配置字体文件的
    项目走内置位图回退时显示约为字号的 7/12。
  - 旧数据的 `scale` 仍可读取，但会在读取时换算为 `font_size`，保存时只写
    `font_size`。
- `Image`：`texture=..;x=..;y=..;width=..;height=..;color=r,g,b;space=..;anchor=x,y`
- `UiLayout`：四边 Anchor + Offset，详见 [28-editor-ui-optimization-execution-tasks.md](28-editor-ui-optimization-execution-tasks.md)。
- `UiEvents`：`event=click;function=on_click`；多条绑定用 `|` 分隔。
  旧 `clickEvent` 标记只在场景实例化时转换为默认 Click 绑定，运行时不再使用。
