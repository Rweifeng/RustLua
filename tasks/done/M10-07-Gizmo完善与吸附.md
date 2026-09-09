# M10-07 Gizmo 完善与吸附

## 状态

ACCEPTED

## 目标

编辑器 3D Gizmo 完善：旋转（绕 Y）/ 缩放（等比）Gizmo + 平移网格吸附；Gizmo 拖拽实时预览（渲染场景即时反映）。

## 背景

M10-04 实现了 3D 透视视口 + 平移 Gizmo。M10-07 补旋转/缩放 Gizmo 与网格吸附（阶段二 P1）。

## 技术方案

- `GizmoDrag.initial` 改为完整 `Transform3D`（位置/旋转/缩放）；`preview3d` 改为 `preview3d_transform`；
- Move/Select：X/Z/Y/Free 平移 + 网格吸附（`snap_enabled`/`snap_step`，工具栏勾选）；Rotate：绕 Y 旋转（水平拖拽→角度）；Scale：等比缩放；
- `commit_gizmo_drag3d` 写完整 Transform3D（position/rotation/scale）；`build_scene3d` 与标记渲染用预览变换（拖拽即时反映）；
- 移除被取代的 `parse_transform3d`/`encode_transform3d_position`。

## 修改范围

- `engine/crates/editor/src/main.rs`

## 验收标准

- 3D 视口 Move/Rotate/Scale 工具可用；网格吸附可开关；
- 拖拽时渲染场景即时预览；提交后保存重开一致；
- 质量门通过。

## AI 输出

已完成：

- 3D Gizmo 支持 Move/Rotate/Scale + 平移网格吸附；
- Gizmo 拖拽实时预览（preview3d_transform 驱动 build_scene3d 与标记）；
- commit 写完整 Transform3D（旋转/缩放可持久化）。

修改文件：

- `engine/crates/editor/src/main.rs`

验证结果：

- fmt/clippy -D warnings/全量测试/Debug/Release/--check 通过；编辑器启动正常（含 Gizmo 路径，无崩溃）；
- Gizmo 交互与吸附由负责人实机确认。

已知问题：

- 无 P0/P1。旋转 Gizmo 仅绕 Y（3D 视口近似，非射线拾取）；吸附仅在 3D 平移生效（2D 平移吸附待后续）。

## 已知限制

旋转仅绕 Y；Gizmo 交互为投影近似；2D 平移吸附待后续。
