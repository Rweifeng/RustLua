# M8-03 Mesh / Material GPU 上传

## 状态

ACCEPTED

## 目标

将 M8-02 的 glTF CPU 导入结果转换为稳定的 GPU 友好 Mesh/Material 数据，并接入渲染器资源生命周期。

## 当前进展

- 已新增 `engine_core::mesh3d`；
- `MeshVertex` 固定为 32 字节交错布局：POSITION `[f32;3]`、NORMAL `[f32;3]`、TEXCOORD_0 `[f32;2]`；
- 已支持从 `GltfAsset` 转换静态 Mesh primitive；缺失 NORMAL 默认使用 `[0,1,0]`，缺失 UV 默认使用 `[0,0]`；
- 已增加自制 GLB 转换和默认属性测试。
- 已在 `game_runner::Renderer` 初始化期间创建测试 GLB 的顶点缓冲、索引缓冲、基础颜色 uniform 和 Material bind group；
- `cargo check -p game_runner`、严格 Clippy 和编辑器代码诊断均通过。
- `Renderer` 为每个静态 primitive 创建 Vertex Buffer、Index Buffer、Material Uniform 和 Material Bind Group；

## 下一步

- 为 `MeshVertex` 增加 GPU buffer-safe 表示和明确顶点属性偏移测试；
- 在 renderer 资源边界增加 Mesh/Index buffer 上传，不改变现有 2D texture pass；
- 增加基础 Material bind group，区分 sRGB base color 与线性数据；
- 用自制 GLB 运行最小 GPU 绘制验证，覆盖空 Mesh、越界索引和超大缓冲错误。

## 已知限制

当前已完成 M8-03 规定的 GPU buffer 和 Material bind group 上传；3D render pipeline、深度状态和实际绘制验证属于 M8-04。