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
- [编辑器 UI 优化方案](27-editor-ui-optimization-plan.md) / [执行任务](28-editor-ui-optimization-execution-tasks.md) / [E01 状态基线](29-editor-e01-state-baseline.md) / [层级面板优化](31-hierarchy-panel-optimization.md)
- [兼容性设计](30-compatibility-designs.md)
- [使用手册](22-使用手册.md)：环境、构建、运行 Demo、Lua 入门
- [Demos 总览](demos/README.md)

## 质量、平台与输入

- [WebGL 浏览器平台验收](32-webgl-浏览器平台验收.md)：浏览器契约检查页、无头验收脚本、产物层启动验收（`verify-webgl-bundle.ps1`）与已知限制
- [手柄设备接入与输入契约](33-手柄设备接入与输入契约.md)：按钮/轴索引契约、XInput 后端、Fake 后端与死区
- [质量门与发布清单](34-质量门与发布清单.md)：Alpha/Beta/RC 门槛、缺陷分级、性能基线与发布清单；文档一致性门 `scripts/check-docs.ps1`（编号/索引/链接/历史路径/编码）
- [输入动作映射与设置持久化](35-输入动作映射与设置持久化.md)：`ActionMap`、Lua `input.action*`、`[input]` 设置段
- [游戏级回归与性能基线](36-游戏级回归与性能基线.md)：核心剧本、帧率确定性、长跑/读档与发布目录隔离自测
- [引擎与游戏工程复核及优化完善计划](37-引擎与游戏工程复核及优化完善计划.md)：复核发现的缺口、执行顺序与各阶段完成状态
- [统一 2D/3D 核心架构实施计划](38-统一2D3D核心架构实施计划.md)：统一运行时的架构目标、迁移路径与阶段验收
- [引擎核心先进性与性能等级复核](39-引擎核心先进性与性能等级复核.md)：按模块给出的能力等级、与成熟引擎的差距和证据要求
- [引擎全面重构计划](40-引擎全面重构计划.md)：WP0–WP9 工作包、3D 不回退门、2D 专项工作表与技术选择原则
- [WP0 第一批基准入口与初始画像](42-WP0第一批基准入口与初始画像.md)：固定工作负载、历史性能证据与第一批边界
- [WP0 第二批统一报告与渲染指标](43-WP0第二批统一报告与渲染指标.md)：`wp0-v2` 统计口径、JSON 基线与 Renderer 指标
- [WP0 第三批 3D 基准与证据](41-WP0第三批3D基准与证据.md)：纯 3D 阶梯、CPU 分配与渲染指标证据
- [WP0 第四批 GPU 时间戳与 3D Golden](44-WP0第四批GPU时间戳与3D Golden.md)：GPU timestamp、语义 golden 与重复基准比较
- [WP1/WP2 设计与最小原型](45-WP1-WP2设计与原型.md)：Typed Scene 与 Local/Global Transform 原型
- [WP0 正式关闭报告](46-WP0关闭报告.md)：四批交付矩阵、退出判定与已知限制
- [WP0 增量：场景装载、typed 编译与运行时帧分配](53-WP0增量场景帧基准与每帧分配.md)：`wp0-v4` 报告、每帧分配量尺（WP1-5/WP3/WP6 的前后对照基线）与质量门接入
- [发布包与用户数据目录](50-发布包与用户数据目录.md)：发布包/开发工程两套数据根、玩家设置播种与升级、验收与 FAQ
- [平台插件适配](51-平台插件适配.md)：`plugins/` 下各平台（微信小游戏 / WebGL）的运行时文件与发布脚本的取用关系
- [微信小游戏分包发布](52-微信小游戏分包发布.md)：主包 + 单分包（默认形态）的切分机制、运行时路由、体积账与验收
- [2D/3D 统一优化方案与现状对比](54-2D3D统一优化方案与现状对比.md)：统一原则、当前事实盘点、与彻底统一目标的差距表与实施优先级
- [编辑器商业引擎对标重构计划](55-编辑器商业引擎对标重构计划.md)：结合 Unity、Unreal 与当前实现的模块重构、UI/UX、性能预算、阶段路线和验收门
- [编辑器重构阶段 0-1 基线与验收](56-编辑器重构阶段0-1基线与验收.md)：性能 fixture、可重复基线报告、已落地服务边界与剩余缺口
- [编辑器重构进度与续跑交接](57-编辑器重构进度与续跑交接.md)：一句话状态、验证命令组、下一步顺序、未提交改动如何带走、踩过的坑

## 第三方素材

- [第三方资源与许可](THIRD_PARTY.md)

## 版本历史

- [CHANGELOG](../CHANGELOG.md)

## 里程碑记录

各里程碑（M0–M15）以及 M16-01~04、M17-01~05、M18-01~04、M19-01~04、M20-01~08、**M22 输入层专题（W1–W5）**的任务与验收记录**已全部归档**至 `tasks/done/`（133 份）。`tasks/plan/` 现有两类可认领任务：`RustLua-game_jhyt-执行计划`（`docs/37` 阶段计划的执行台账）与**引擎全面重构的 WP 任务文件**（`WP1-TypedScene与资源句柄.md`、`WP2-统一Transform与层级传播.md`）；重构现状与下一步见 [引擎全面重构计划](40-引擎全面重构计划.md) 与 [WP1/WP2 设计与原型](45-WP1-WP2设计与原型.md)。M22 的 W6（上下文分组动作表 / 鼠标相对运动 / 录制回放 / Web 输入）是**已裁定不做**的 backlog，决策与触发条件留在 `tasks/done/M22-输入层收口.md` §9，不在 `plan/` 另立文件。CHANGELOG 版本映射：M16 → v0.12、M17 → v0.13、M18 → v0.14、M19/M20 → v0.15 起（收口延续到 v0.21）；基线标签：`v0.9-3d-game`（M9）、`v0.10-editor`（M10）。

### 文档编号约定（两个命名空间）

编号**按目录各自独立**，同一编号在两个目录里指向不同文档，引用时必须带目录：

| 命名空间 | 目录 | 职责 | 区段 |
|---|---|---|---|
| 引擎文档 | `docs/NN-*.md` | 引擎能力、技术决策、质量门与发布 | `00`–`50`（本目录） |
| 游戏文档 | `examples/<project>/docs/NN-*.md` | 单个游戏项目的内容设计、美术、数值、交付记录 | `29`–`50`（`game_jhyt`） |

规则：① 同一目录内编号唯一（`scripts/check-docs.ps1` 会在质量门里强制）；
② 跨目录引用写全路径（`examples/game_jhyt/docs/46-...` 或 `docs/50-...`），不写裸 `docs/46`；
③ 新增文档取该目录**当前最大号 +1**，不回收已删除的号；④ 区段重叠是**有意的**——两个目录扫描、索引、
引用各自独立，物理上不可能混（早期 `docs/37-引擎…` 与 `game_jhyt/docs/37-...山庄主场景美术` 同号即按此约定裁定）。
