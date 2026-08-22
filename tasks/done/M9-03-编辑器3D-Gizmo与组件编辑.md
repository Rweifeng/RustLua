# M9-03 编辑器 3D Gizmo 与组件编辑

## 状态

ACCEPTED

## 目标

编辑器支持选中 3D 实体并拖动平移 Gizmo；Inspector 查看/编辑 Transform3D、MeshRenderer、Camera3D、Light、碰撞体组件；可添加 3D 实体并保存/加载 roundtrip；2D 编辑与保存不回归。

## 背景

M8-06 已把 3D 组件接入 Inspector（Transform3D/Camera3D/Light/MeshRenderer 数值编辑），但明说「无 3D Gizmo」。M9-03 补平移 Gizmo 与 Collider3d 编辑，使 3D 关卡可以在编辑器内摆放。ADR-0010 已批准首版只做平移。

## 输入

- `docs/27-M9-3D可玩垂直切片实施计划.md`（M9-03）
- `docs/02-technical-decisions.md`（ADR-0010）
- `engine/crates/editor/src/main.rs`（场景视图/inspector 面板）
- `engine/crates/editor/src/inspector.rs`（组件模型）
- `engine/crates/editor/src/history.rs`（撤销/重做）
- `engine_core/src/physics3d.rs`（ColliderDesc3d 组件串）

## 不做什么

- 不做 3D Gizmo 旋转/缩放（后置）；
- 不做 3D 视口渲染（无透视场景预览）；
- 不做地形编辑器；不破坏 2D 编辑路径与场景格式。

## 技术方案

- **场景视图 2D/3D 模式**：`SceneViewMode`（默认 2D），场景视图工具栏切换。3D 模式按 Transform3D 的 XZ 俯视投影绘制 3D 实体（菱形标记 + 名称），沿用网格/pan/zoom。
- **平移 Gizmo**：选中 3D 实体 + Move/Select 工具时绘制三轴手柄 —— X（红）/Z（蓝）约束单轴、Y（绿）调高度、拖实体自由 XZ。拖拽用独立 `GizmoDrag` 状态（不触碰 2D drag），预览写入 `scene_state.preview3d`。
- **写回**：松手 `commit_gizmo_drag3d` 经 `history.execute(CommandKind::Transform3D(id))` 写 Transform3D 组件（仅改 position，保留旋转/缩放），与 2D Transform 一样合并连续编辑。
- **Inspector Collider3d**：`Collider3dFields` + `set_collider3d`/`clear_collider3d` + 编解码/校验（复用 `ColliderDesc3d`）；面板含形状下拉、尺寸、sensor/dynamic、offset、添加/移除按钮。
- **添加 3D 实体**：层级面板「添加 3D 实体」创建带 Transform3D 组件的实体（history 可撤销）。

## 修改范围

- `engine/crates/editor/src/main.rs`（SceneViewMode/preview3d/3D 场景视图/Gizmo/添加 3D 实体/Collider3d 面板）
- `engine/crates/editor/src/inspector.rs`（Collider3dFields/set_collider3d/校验）
- `engine/crates/editor/src/history.rs`（CommandKind::Transform3D + 合并）

## 验收标准

- 编辑器可选中 3D 实体、拖动平移 Gizmo、Inspector 数值同步；
- 保存后重新打开 3D 实体位置一致；
- Inspector 可编辑 Collider3d 组件（校验非法值）；
- 播放/停止不污染编辑场景；2D 编辑与保存不回归；
- Debug/Release 质量门通过。

## 测试命令

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace
cargo build --workspace --release
git diff --check
```

## 验收证据

- editor 单测日志（Collider3d roundtrip、Transform3D 组件写回、拖拽逻辑）；
- 手动：编辑器摆 3D 实体→保存→重开一致；
- workspace 质量门日志。

## 风险

- Gizmo 与 2D 画布交互冲突：独立 drag3d 状态，不触碰 2D drag；
- Inspector 快照/应用遗漏 Collider3d：同步 snapshot/apply/set_*；
- 播放污染：沿用 is_playing 锁。

## AI 输出

已完成：

- **3D 场景视图**：`SceneViewMode` 2D/3D 切换；3D 模式 XZ 俯视投影绘制 Transform3D 实体，点选同步树/Inspector；
- **平移 Gizmo**：X/Z 轴手柄（红/蓝）、Y 高度手柄（绿）、拖实体自由 XZ；独立 `GizmoDrag` 状态；
- **写回**：`commit_gizmo_drag3d` 经 `CommandKind::Transform3D`（history 可撤销、合并连续编辑）写回 Transform3D 组件，仅改 position；
- **Inspector Collider3d**：`Collider3dFields` 结构化编辑（形状/尺寸/sensor/dynamic/offset），复用 `ColliderDesc3d` 校验，含添加/移除按钮；
- **添加 3D 实体**：层级面板按钮创建带 Transform3D 的实体；
- 2D 编辑路径零改动。

修改文件：

- `engine/crates/editor/src/main.rs`（3D 场景视图/Gizmo/添加 3D 实体/Collider3d 面板/SceneViewMode/preview3d）
- `engine/crates/editor/src/inspector.rs`（Collider3dFields/set/clear/编解码/校验）
- `engine/crates/editor/src/history.rs`（CommandKind::Transform3D + 合并）
- `engine/crates/editor/src/lib.rs`（导出 Collider3dFields）

验证结果：

- `cargo fmt --all -- --check`：通过；
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`：通过；
- `cargo test --workspace --all-features`：通过（workspace 共 280：editor 72 含 Collider3d roundtrip/非法拒绝/场景持久化、Transform3D 写回、history Transform3D 合并；engine_core 197；game_runner 6）；
- Debug/Release 构建、`game_runner --check`、`git diff --check`：通过；
- `editor --check` 通过；编辑器 GUI 启动运行 10s 无崩溃（自动恢复文件已清理，无项目文件污染）。

已知问题：

- 无 P0/P1。3D Gizmo 平移交互为 XZ 俯视平面；Gizmo 渲染效果需负责人实机视觉验收。

建议下一步：

- M9-03 提交（`feat(M9-03): 编辑器 3D Gizmo 与 Collider3d 编辑`）后，负责人实机验收 3D 场景视图，再进入 M9-04 3D 样板游戏。

## 已知限制

首版 3D Gizmo 仅平移，且为 XZ 俯视平面交互（Y 高度通过 Y 手柄与 Inspector 调整）；不做旋转/缩放 Gizmo、不做 3D 透视视口。
