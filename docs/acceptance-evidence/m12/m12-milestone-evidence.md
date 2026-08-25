# M12 里程碑验收证据（资源系统闭环）

日期：2026-08-25
基线：`354750c`（M11 验收矩阵收尾）之后的 M12 阶段 1 / 阶段 2 提交
范围：本次归档仅覆盖 `M12-00`、`M12-01`、`M12-02`，即资源状态模型、Importer 增量导入、依赖图与失效传播。

## 总状态

**代码 / 自动化侧：PASS。**
**实机侧：PASS（阶段 1 后台导入链路 + 阶段 2 `assetc --check-deps` + editor 断链视觉复核均已归档）。**
**未纳入本次归档：M12-03 / M12-04 / M12-05。**

## 阶段 1：M12-00 + M12-01

详见 [m12-01-resource-system-phase1.md](m12-01-resource-system-phase1.md)。

- 5 个 C 提交：`f2529a4` / `82a1212` / `9e98deb` / `6d45401` / `38aaf4b`
- 关键结果：
  - 统一 `ResourceKind` 与 `ResourceState`
  - 资源数据库升级到 v2，支持迁移与损坏恢复
  - Importer 注册表 + batch 导入闭合 assetc / editor 两条链路
  - `game_test` 31 项资源实机导入为 ready
- 补充修复：`d39e6fc` 消除 importer version 双源漂移

## 阶段 2：M12-02

详见 [m12-02-dependency-graph.md](m12-02-dependency-graph.md)。

- 5 个 C 提交：`2cebeea` / `dbdc067` / `57df5c4` / `fb7a312` / `9e7502e`
- 关键结果：
  - Scene / Prefab / Lua 依赖提取器落地
  - `Invalid` 断链状态、失效传播、恢复撤销、循环检测完成
  - editor 资源面板可显示断链状态
  - `assetc --check-deps` 可输出完整 consumer → dependency 链路
- 实机补充：
  - 移走 `assets/textures/player.png` 后，`game_test` 可稳定输出完整断链路径
  - 编辑器 GUI 断链图标 / 红字视觉呈现实机复核 PASS

## 归档结论

M12 当前可归档的范围已经形成闭环：

1. 资源状态模型统一；
2. 增量导入链路闭合；
3. 依赖断链可见、可传播、可检查。

这意味着 editor / assetc / game 现在已经共享同一套资源事实来源，后续可以把重心切到：

- `M13` 2D 内容生产工作流；
- 或拆分继续推进 `M12-03` 资源浏览器生产化、`M12-04` 校验报告、`M12-05` 迁移恢复框架。

## 未纳入本次归档

- M12-03：资源浏览器生产化（右键菜单 / 依赖面板 / 缩略图增强）
- M12-04：统一 ValidationReport / 发布前检查 / webp / mp3
- M12-05：资源数据库迁移、备份与恢复框架

因此本文件只证明：**M12-00 ~ M12-02 已归档**，不代表原始 M12 全部子项已全部完成。
