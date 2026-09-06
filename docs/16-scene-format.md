# M4-03 场景格式

场景使用版本化的可读文本格式，默认扩展名为 `.scene`。格式包含版本号、场景名称、实体树、组件数据、资源 UUID 和编辑器元数据。

保存会先写入同目录临时文件，再替换正式场景文件；加载失败不会修改已有文件。解析错误包含行号和字段，便于编辑器定位问题。

当前支持版本为 `2`。

## 版本

- 当前版本是 `2`，包含 2D 组件、3D 组件和分离的 `UiLayout`。
- `Scene::from_text` 不做版本迁移。低于 `2` 或高于当前版本的文件都会通过
  `SceneError::UnsupportedVersion` 明确拒绝。

实体行支持 3 字段（无父实体，`entity\t<id>\t<name>`）或 4 字段（含父实体）两种写法；序列化时无父实体不再输出空尾字段，避免场景文件产生尾随空白。

```powershell
cargo run -p game --example scene_format_demo --release
```

## 3D 组件字符串

沿用 `key=value;` 约定，向量用逗号分隔（参考 `Collider offset=x,y`）：

- `Transform3D`：`position=x,y,z;rotation=rx,ry,rz,rw;scale=sx,sy,sz`
- `MeshRenderer`：`mesh=assets/models/<file>.glb|gltf`（`__ground_plane__` 为内置地面哨兵路径）
- `Camera3D`：`position=x,y,z;target=x,y,z;fov=<度>;near=<n>;far=<f>`（`aspect` 由渲染窗口决定）
- `Light`：`direction=x,y,z;color=r,g,b;intensity=<i>`（方向光）

## UI 组件字符串（屏幕空间）

屏幕空间 UI 组件（`Text` / `Image` / `UiLayout` / `UiEvents`）同样用
`key=value;` 编码。`Text` / `Image` 只保存视觉属性；几何完全由 `UiLayout`
的四边 Anchor + Offset 决定。

- `Text`：`text=<esc>;font_size=..;color=r,g,b`
  - `font_size`（px）：像素字号（SDF 字体下 em 高度）；基准
    `UI_TEXT_BASE_PX = 12`。
- `Image`：`texture=..;color=r,g,b;nine_slice=l,r,t,b`
- `UiLayout`：`anchors=l,t,r,b;offsets=l,t,r,b;min_size=w,h;max_size=w,h;pivot=x,y`
  字段均可省略并使用默认值，但完整几何必须通过本组件表达。
- `UiEvents`：`event=click;function=on_click`；多条绑定用 `|` 分隔。
