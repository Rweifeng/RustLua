# 任务：M27 编辑器体验升级 — 总览与排期

- 任务 ID：M27-00 ~ M27-03
- 状态：四期全部交付，**待项目负责人验收**（M27-00 阶段 0/1/2 与收尾 ✅ 2026-10-02、
  M27-01 ✅、M27-02 ✅、M27-03 验收与裁定 ✅ 2026-10-02；代码未提交，
  五文已随收口移入 `tasks/done/`；验收结论与剩余边界逐条记在 `M27-03` 的 §实现记录 / §未完成项）
- 关联里程碑：M27（编辑器体验升级，源自复核清单第 12 项）
- 创建日期：2026-10-01

## 目标

把编辑器从"固定面板布局"升级为"可停靠多标签工作台"：egui_dock 多标签面板、
资产拖放（外部 OS 拖入 + 内部拖放收口）、Scene View 可视化编辑查漏增强，并对
"是否迁移 UI 框架"给出书面裁定。

## 现状基线（2026-10-01 盘点）

- 编辑器 crate：`engine/crates/editor`，约 31k 行，eframe/egui **0.33**（`engine/Cargo.toml`）。
- 布局为**硬编码**固定面板：`main.rs:2098`（inspector）、`:2247-2264`（resource/tree/inspector/central），
  面板实体在 `panels/*.rs`（console/hierarchy/resource_browser/scene_view/inspector/…）。
- 布局持久化已有版本化容错文件：`layout.rs`（`EditorLayout`，**v2**，存 `<project>/.cache/editor/layout.json`，
  契约：100 次 save/load 字节幂等；v2 起整棵 dock 树以 `dock_tree` 原样存取，v1 文件缺该键即"没存过"）。
- **全仓库无 egui_dock 引用**；配套版本已实测钉死为 **`egui_dock = "=0.18.0"`**
  （`0.18↔egui 0.33`、`0.19↔0.34`、`0.20↔0.35`、`0.21↔0.36`；用错版本会静默拉进第二份 egui）。
  M27-00 阶段 0/1/2 与收尾（View 菜单逐项可切全部 DockTab）均已完成，详见该文件 §阶段 0/1/2；
  剩余（M27 之后）：面板标题去重、`panel_widths` 出库、叶子最小尺寸。
- 资产拖放**部分存在**：Scene View 已接 egui 内部 drop（`panels/scene_view.rs:57-87`，
  `spawn_dropped_resource` `:2827`）；**从 OS 拖文件进窗口未接**（eframe `RawInput::dropped_files` 无消费点）。
- Gizmo/多选/框选**大部分已实现**：`gizmo.rs`、`scene_drag.rs`（提交层）、`selection.rs`（统一选择模型，
  含 `SceneView2D` 框选来源）、`layout.rs` 已持久化 `gizmo_space/gizmo_pivot`；M27-03 只做查漏。
- 架构约束（必须遵守，M27-00 起效）：
  - `panels/architecture_tests.rs` 棘轮：面板只做绘制/命中/事件采集，**提交逻辑走 typed action**；
  - Play 模式锁编辑；拖动预览只改运行时 ECS，松手恰好一条 Undo + 一次保存。

## 分期与依赖

| 期 | 文件 | 依赖 | 预估 |
|---|---|---|---|
| 0+1 | `M27-00-dock化基线与多标签面板布局.md` | 无 | ~2 周 |
| 2 | `M27-01-资产拖放.md` | M27-00（dock 后 drop 目标区变化） | 3-5 天 |
| 3 | `M27-02-gizmo增强与框选查漏.md` | 可与 M27-01 并行 | 2-4 天 |
| 收口 | `M27-03-验收性能与UI框架裁定.md` | 前三期全部 | 2-3 天 |

串行主线：M27-00 → M27-01 → M27-03；M27-02 可在 M27-00 合入后随时插入。

## 风险（跨期）

- egui_dock 与 egui 小版本**硬绑定**：不匹配直接编译失败；备选为 vendor 一份 egui_dock
  （仓库已有 `engine/vendor/` 先例）。
- dock 化改动触及每帧 UI 主路径：以 `perf_baseline.rs` + `gui_evidence.rs` 现有基准守住帧预算。
- 工作区常有大量在途改动：各期提交一律按路径 stage，不做全量 `git add -A`。

## 验收方式

见 M27-03；每期各自带模板质量门（fmt / clippy / tests / Debug 构建 / 手工步骤）。
