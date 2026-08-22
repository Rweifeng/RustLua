# RustLua 游戏引擎文档索引

## 快速入口

- [使用手册](22-使用手册.md)：环境准备、构建、运行 Demo、Lua API、场景脚本和问题排查；`r`n- [工程目录功能说明](23-工程目录功能说明.md)：每个主要目录和源码文件的职责；
- [项目章程](00-project-charter.md)：项目目标、范围和协作规则；
- [技术决策](02-technical-decisions.md)：关键技术选型和变更记录；
- [验收矩阵](04-acceptance-matrix.md)：任务级验收标准和证据；
- [AI 驱动实施计划](AI驱动RustLua游戏引擎实施计划.md)：里程碑和任务总览。
- [M9 3D 可玩垂直切片实施计划](27-M9-3D可玩垂直切片实施计划.md)：基于 `v0.8-3d-foundation` 的 3D 可玩游戏垂直切片与发布验收顺序；
- [M8 轻量 3D 实施计划](26-M8轻量3D实施计划.md)：已完成并归档（基于 `v0.7` 的 3D 能力、资源管线和发布验收顺序）。

## 能力文档

- [坐标和颜色约定](06-coordinate-and-color-conventions.md)
- [纹理加载和上传](07-texture-loading-and-upload.md)
- [Sprite](08-sprite.md)
- [SpriteBatch](09-sprite-batch.md)
- [Camera2D](10-camera2d.md)
- [图集](11-texture-atlas.md)
- [文本](12-text.md)
- [2D Demo](13-2d-demo.md)
- [资源数据库](14-resource-database.md)
- [导入缓存](15-import-cache.md)
- [场景格式](16-scene-format.md)
- [预制体](17-prefab.md)
- [Lua API](18-lua-api.md)
- [Lua 热重载](19-lua-hot-reload.md)
- [Lua 脚本调试](20-lua-script-debug.md)
- [场景脚本 Demo](21-scene-script-demo.md)

## 里程碑记录

- `tasks/done/M0-*`：M0 基线；
- `tasks/done/M1-*`：M1 基础运行时；
- `tasks/done/M2-*`：M2 ECS；
- `tasks/done/M3-*`：M3 2D；
- `tasks/done/M4-*`：M4 资产、场景与脚本；
- `tasks/done/M5-*`：M5 编辑器 MVP；
- `tasks/done/M6-*`：M6 2D 游戏能力与样板游戏；
- `tasks/done/M7-*`：M7 工程化与发布（assetc/发布配置/崩溃日志/性能基线/干净机器/发布候选/压缩加密/体积优化/编辑器进度面板）；
- `tasks/done/M8-*`：M8 轻量 3D（glTF/Math3D/Camera3D/Mesh/Depth/光照阴影PBR/骨骼动画/剔除实例化/3D Demo 与发布验收）；
- `tasks/active/M9-*`：M9 3D 可玩垂直切片（当前里程碑，计划见 [27-M9-3D可玩垂直切片实施计划.md](27-M9-3D可玩垂直切片实施计划.md)）；
- 当前里程碑标签：`v0.8-3d-foundation`。

## 当前状态与下一阶段

- M8 已全部实施并验收（2026-08-22，`v0.8-3d-foundation`），任务记录见 `tasks/done/M8-*`；[26-M8轻量3D实施计划.md](26-M8轻量3D实施计划.md)保留为历史计划；
- 下一阶段为 M9 3D 可玩垂直切片，实施方案见 [27-M9-3D可玩垂直切片实施计划.md](27-M9-3D可玩垂直切片实施计划.md)；
- 发布包：`artifacts/builds/release/p2-game/`（约 14MB，压缩+加密默认开启；`game_runner.exe --pack pack`）；3D 性能基线：`docs/acceptance-evidence/m8/`；
- 2D 样板游戏（俯视角 Roguelite）与 M8 3D Demo 均可运行；脚本按职责拆分（`main.lua` 入口 + 各模块）；
- 第三方资源许可：[THIRD_PARTY.md](THIRD_PARTY.md)；
- 里程碑验收证据：`docs/acceptance-evidence/`（`artifacts/` 目录为可再生输出）；
- 后续里程碑：M9 3D 可玩垂直切片。
