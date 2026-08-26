# RustLua 游戏引擎文档索引

## 快速入口

- [使用手册](22-使用手册.md)：环境准备、构建、运行 Demo、Lua API、场景脚本和问题排查；
- [工程目录功能说明](23-工程目录功能说明.md)：每个主要目录和源码文件的职责；
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
- `tasks/done/M9-*`：M9 3D 可玩垂直切片（已验收，计划见 [27-M9-3D可玩垂直切片实施计划.md](27-M9-3D可玩垂直切片实施计划.md)）;
- `tasks/done/M10-*`：M10 编辑器强化基线与功能任务（计划见 [28-编辑器强化实施计划.md](28-编辑器强化实施计划.md)）；
- `tasks/done/M10-11-*`、`M10-12-*`：`v0.10-editor` 基线之后的编辑器布局、优化与 SDF 字体增量；
- `tasks/done/M11-*`：M11 编辑器可靠性收口（PlaySession 状态机/输入快照/offscreen 缓存一致性/场景提取统一/恢复与字体内存，任务记录见 [M11-编辑器可靠性收口.md](../tasks/done/M11-编辑器可靠性收口.md)）；
- `docs/acceptance-evidence/m11/`：M11 实机证据（G01/G04 PASS，G02/G03 待负责人）；
- `tasks/done/M12-*`：M12 资源系统闭环已归档（本次归档范围为 M12-00~M12-02：状态模型 + Importer 增量导入 + 依赖图与失效传播），任务总览见 [M12-资源系统闭环](../tasks/done/M12-资源系统闭环.md)，证据见 [docs/acceptance-evidence/m12/](acceptance-evidence/m12/)；
- `tasks/done/M13-2D内容生产工作流.md`：M13 2D 内容生产工作流已归档（当前范围覆盖 Sprite、Tilemap、Animation、2D 排序、Prefab、UI 的最小生产闭环），证据见 [docs/acceptance-evidence/m13/](acceptance-evidence/m13/)；
- `tasks/done/M14-00-新建2D生产样板工程.md`：M14-00 已完成，新增独立样板工程 `game_test_2d_production/`，证据见 [docs/acceptance-evidence/m14/](acceptance-evidence/m14/)；
- `tasks/done/M14-01-端到端编辑器工作流.md`：M14-01 阶段性归档，已补样板工程自动化工作流回归基线，GUI 实机链路待继续；
- `tasks/done/M14-02-资源变更断链修复回归.md`：M14-02 已归档，样板工程上的资源改名、删除、断链传播、恢复原文件与修复引用链路已完成自动化、GUI 与命令行三层验证；
- `tasks/done/M14-03-PlayStop恢复发布回归.md`：M14-03 已归档，样板工程上的 Play/Stop、保存/关闭/重开、恢复、发布检查与包体验证已完成自动化、命令行与 GUI 实机闭环；
- `tasks/done/M14-04-垂直验收.md`：M14-04 已归档，`game_test_2d_production/` 已完成最终垂直验收，`M14-00 ~ M14-03` 的结果已收束为统一结论；
- M9 基线标签：`v0.9-3d-game`（指向 M9-05 验收提交）；
- M10 基线标签：`v0.10-editor`（指向 M10-10 验收提交）；

## 当前状态与下一阶段

- M0–M8 路线图已完成；M9 3D 可玩垂直切片以 `v0.9-3d-game` 标记，包含 2D 样板游戏 + 3D 可玩样板游戏（`game3d.scene`）；
- M10-00..M10-10 形成 `v0.10-editor` 编辑器强化基线：引擎内嵌游戏预览（Play 实时运行）、3D 透视视口 + 轨道导航、2D/3D 场景实时渲染、组件增删、Gizmo（Move/Rotate/Scale + 吸附）、资源缩略图与拖放、多选对齐、Prefab 集成；
- `v0.10-editor` 之后的增量提交为 M10-11（面板拆分与四行布局）、M10-12（编辑器优化与 TMP 风格 SDF 字体）和 M10-13（质量门与 GUI/GPU 证据规范），任务记录见 [M10-11](../tasks/done/M10-11-编辑器面板拆分与四行布局.md)、[M10-12](../tasks/done/M10-12-编辑器优化与SDF字体.md) 和 [M10-13](../tasks/done/M10-13-质量门与编辑器实机证据.md)，证据见 [docs/acceptance-evidence/m10/](acceptance-evidence/m10/)；M10-13 的真实 GUI/GPU、DPI/resize、Play/Stop 原始产物仍需负责人归档；发布包字体路径为 `fonts/NotoSansSC-VF.ttf`。
- 发布包：`artifacts/builds/release/game/`（约 15MB，加密开启；`game_runner.exe --pack pack`）；3D 性能基线：`docs/acceptance-evidence/m9/`；
- 里程碑验收证据：`docs/acceptance-evidence/`（`artifacts/` 目录为可再生输出）；
- M11 性能修复：Game 预览 3D 场景帧率 24→50fps（`cbbb09b`，`PREVIEW_FRAME_INTERVAL` 33ms→16ms）；
- M13 已归档：编辑器当前已具备不手改 scene/prefab 文件完成基础 2D 内容生产的最小闭环；
- M14 已启动：`game_test_2d_production/` 作为端到端样板工程已创建并完成自动化基线验收；
- M14-01 已有自动化底座：样板工程已进入 `assetc`、`editor`、`engine runtime` 回归面，任务记录以阶段性归档形式保留在 `tasks/done/`；
- M14-02 已完成：资源变更、断链、恢复原文件与修复引用在真实样板工程上均已得到自动化、GUI 与命令行验证；
- M14-03 已完成：Play/Stop、保存/关闭/重开、恢复、发布检查与包体验证在真实样板工程上均已闭环；
- M14-04 已完成：`M14` 当前范围的样板工程主链已完成垂直验收；
- 当前下一步：进入 `M15-00` 性能基线；
- 后续里程碑：继续推进 `M15-00 ~ M15-04` 的性能、稳定性、干净机器发布与发布候选；若继续补资源闭环深化，则推进 M12-03 资源浏览器生产化、M12-04 校验报告、M12-05 迁移恢复框架。
