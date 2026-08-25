# M8-04 Depth + 3D Render Pass

## 状态

ACCEPTED

## 目标

为 game_runner 增加可重建的 `Depth32Float` 深度附件和独立 3D Render Pass，使用 M8-01 的 Camera3D 投影绘制 M8-03 上传的测试 GLB，同时保持现有 2D pass 兼容。

## 已完成

- Renderer 初始化创建 `Depth32Float` texture/view；窗口 resize 时重建 depth view；
- 新增 Camera3D view-projection uniform bind group；
- 新增独立 Mesh3D WGSL shader 和 pipeline；
- pipeline 使用 MeshVertex 的 POSITION/NORMAL/TEXCOORD_0 布局、`LessEqual` 深度比较和深度写入；
- 3D pass 使用 index buffer + material bind group 绘制测试 GLB，随后继续现有 2D 绘制。

## 不做什么

- 不实现光照阴影、PBR、视锥剔除、实例化或场景/Lua 3D 组件；
- 不改变现有 2D pipeline 的深度状态；
- 不把深度纹理加入发布资源包。

## 验收标准

- Debug/Release 可编译；
- 3D pipeline 创建成功，测试 GLB 可提交 indexed draw；
- depth view 在初始化和 resize 后均有效；
- 前后模型遮挡由 depth compare 决定；
- 现有 2D 测试和 workspace 质量门不回归；
- 运行窗口后保留 3D 三角形与 2D UI 的人工视觉证据。

## 当前限制

人工验收已确认 3D 三角形、深度遮挡和 2D UI 叠加效果；光照和阴影属于 M8-05。

## 验收证据

- Release 人工验收包：`artifacts/builds/release/game/`；
- `game_runner.exe --pack pack --check` 目录隔离检查通过；
- M8-04 自动验收：workspace 测试、Clippy、Debug/Release 构建和项目检查通过；
- 人工验收结论：通过。