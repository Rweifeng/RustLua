# RustLua 游戏引擎文档索引

## 开发验收规则

- **修改引擎/编辑器时，验收只需 `cargo build -p editor`（debug 构建）通过**，无需构建 release。
- **release 构建由负责人需要时手动执行**（`cargo build --release -p editor`，耗时约 3 分钟），不纳入日常修改的自动验收流程。

## 快速入口

- [使用手册](22-使用手册.md)：环境准备、构建、运行 Demo、Lua API、场景脚本和问题排查；
- [工程目录功能说明](23-工程目录功能说明.md)：每个主要目录和源码文件的职责；
- [项目章程](00-project-charter.md)：项目目标、范围和协作规则；
- [技术决策](02-technical-decisions.md)：关键技术选型和变更记录。

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
- [3D 渲染与能力参考](24-3d-rendering.md)

## 编辑器与发布

- [编辑器使用指南](25-editor-guide.md)：面板、工具栏、Play/Stop、导入/发布、崩溃恢复
- [资源管线与发布（assetc）](26-assetc-and-publish.md)：`project.toml`、CLI、打包格式、存档格式
- [使用手册](22-使用手册.md)：环境、构建、运行 Demo、Lua 入门
- [Demos 总览](demos/README.md)

## 第三方素材

- [第三方资源与许可](THIRD_PARTY.md)

## 版本历史

- [CHANGELOG](../CHANGELOG.md)

## 里程碑记录

各里程碑（M0–M15）的任务与验收记录已归档至 `tasks/done/`；规划中的 M16–M20（BACKLOG）见 `tasks/`。CHANGELOG 规划版本映射为 M16 → v0.12、M17 → v0.13、M18 → v0.14、M19 → v0.15；基线标签：`v0.9-3d-game`（M9）、`v0.10-editor`（M10）。
