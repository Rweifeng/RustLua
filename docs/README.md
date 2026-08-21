# P2 RustLua 游戏引擎文档索引

## 快速入口

- [使用手册](22-使用手册.md)：环境准备、构建、运行 Demo、Lua API、场景脚本和问题排查；`r`n- [工程目录功能说明](23-工程目录功能说明.md)：每个主要目录和源码文件的职责；
- [项目章程](00-project-charter.md)：项目目标、范围和协作规则；
- [技术决策](02-technical-decisions.md)：关键技术选型和变更记录；
- [验收矩阵](04-acceptance-matrix.md)：任务级验收标准和证据；
- [AI 驱动实施计划](AI驱动RustLua游戏引擎实施计划.md)：里程碑和任务总览。

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
- 当前里程碑标签：`v0.4-scripting`。
## 下一阶段

- [M5 编辑器 MVP 实施计划](24-M5编辑器MVP实施计划.md)：M5-01 至 M5-10 的依赖顺序、实施条目、验收方案和风险；
- M5 编辑器 MVP 已验收，任务记录：`tasks/done/M5-01-编辑器启动与项目打开.md` 至 `tasks/done/M5-10-编辑器MVP综合验收.md`；
- 当前基线标签：`v0.5-editor-mvp`；
- 下一里程碑 M6：2D 游戏能力 + 样板游戏；
- M6-00 已完成（运行时场景实例化 + 玩家移动），任务记录：`tasks/done/M6-00-运行时场景实例化与玩家移动.md`；
- M6-01 已完成（Tilemap 网格地图），任务记录：`tasks/done/M6-01-Tilemap.md`。
