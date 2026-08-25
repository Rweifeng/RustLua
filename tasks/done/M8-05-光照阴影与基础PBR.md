# M8-05 光照、阴影与基础 PBR

## 状态

ACCEPTED

## 当前进展

- glTF Material 已读取 base color、metallic factor、roughness factor；
- 3D shader 已使用方向光 uniform；
- 已加入 Lambert 漫反射、环境光和基于 roughness/metallic 的简化高光；
- Material uniform 已扩展为 32 字节，包含 base color 与 surface 参数；
- 主相机深度和 Shadow Map 使用两个独立的 `Depth32Float` 资源；
- 已加入固定 2048x2048 `Depth32Float` Shadow Map、ClampToEdge 比较采样器和光源相机矩阵；
- 已加入 shadow-only 深度 pass，主 3D pass 通过 `textureSampleCompare` 使用阴影可见度；
- 主 pass 对阴影坐标做 NDC 到 UV 转换，并对阴影图外区域回退为完全可见。
- 方向光使用固定范围正交光空间投影，避免使用透视光源造成阴影尺度变化。

## 验收目标

- 方向光改变时模型明暗可观察变化；
- metallic/roughness 参数影响高光；
- shadow map 能产生角色/场景遮挡阴影；
- 颜色纹理按 sRGB，法线/金属度/粗糙度按线性数据处理；
- 2D pass 和现有深度测试不回归。

## 下一步

无。

## 当前验证

- `cargo check -p game_runner` 通过；
- game_runner 严格 Clippy 通过；
- Shadow Map 代码诊断无错误；
- `Depth32Float`、`LessEqual`、比较采样器和 shadow-only pass 已进入 Renderer。
- 测试场景已加入固定接收平面，与自制 GLB 三角形一起进入真实窗口 3D 绘制；发布日志记录 `shadow scene ready: occluder plus receiver plane`、`Running` 和 `Exited`。
- 等价 GPU 资源检查已归档至 `docs/acceptance-evidence/m8/m8-05-shadow-gpu-check.md`；RenderDoc 1.45 已安装并完成采集尝试。
- 发布包已重建，`artifacts/builds/release/game/build-info.txt` 的时间为 `2026-08-22 15:54:02`。
- 完整 workspace 质量门与 Release 包回归由负责人完成验收，M8-05 通过。
