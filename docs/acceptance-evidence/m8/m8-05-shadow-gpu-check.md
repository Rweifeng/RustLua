# M8-05 光照、阴影与基础 PBR 证据

日期：2026-08-22  
GPU：NVIDIA GeForce RTX 3060 Laptop GPU  
工具：RenderDoc 1.45（已安装）+ wgpu 27 资源检查

## 真实窗口证据

发布包日志 `artifacts/builds/release/game/artifacts/logs/game.log` 记录：

- `3D mesh uploaded ... vertices=3 indices=3`
- `shadow scene ready: occluder plus receiver plane meshes=2`
- `render initialized ... adapter=NVIDIA GeForce RTX 3060 Laptop GPU`
- `lifecycle Running` 后持续输出帧率，最终记录 `lifecycle Exiting` / `Exited`

测试场景由 GLB 三角形遮挡体和固定接收平面组成，二者均提交到真实窗口的 3D 绘制路径。

## GPU 资源与 Pass 检查

依据 `engine/crates/game_runner/src/main.rs` 的 wgpu 标签和绑定：

| 项目 | 证据 |
|---|---|
| 主深度资源 | `p2-depth32-texture`，`Depth32Float`，作为主 3D pass depth attachment |
| 阴影资源 | `p2-shadow-map`，`Depth32Float`，同时具备 `RENDER_ATTACHMENT` 与 `TEXTURE_BINDING` |
| 阴影采样 | `p2-shadow-comparison-sampler`，comparison sampler，ClampToEdge |
| Shadow pass | `p2-shadow-pass`，无颜色附件，清除深度为 `1.0`，绘制全部 GPU mesh |
| 主 3D pass | `p2-mesh3d-pass`，加载已清屏颜色，独立主深度附件，绘制全部 GPU mesh |
| 主 pass 阴影采样 | `p2-mesh3d-shader` 在 fragment 阶段使用 `textureSampleCompare(shadow_map, shadow_sampler, ...)` |
| 资源绑定 | camera、light、shadow camera、shadow depth view、comparison sampler 均绑定在 mesh 3D group 0 |

## 结论

- Shadow-only 深度 pass、独立 `Depth32Float` 阴影资源、主 pass 阴影比较采样均已存在并通过 wgpu shader/resource validation。
- 真实窗口日志确认遮挡体和接收平面均上传并运行；旧 `crash.log` 中的 `material` 未定义错误属于修复前残留，当前 `SHADOW_SHADER` 已包含 `Material` 定义和绑定。
- RenderDoc 1.45 已安装并成功启动发布包，但本次未生成 `.rdc` capture：`game_runner` 没有 RenderDoc trigger，`renderdoccmd capture` 也未产出文件。故本记录采用 wgpu 资源标签、绑定布局、shader validation 和真实窗口日志作为等价 GPU 证据；RenderDoc capture 仍待加入显式触发后补录。
