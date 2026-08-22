# M10-09 Prefab 编辑器集成

## 状态

ACCEPTED

## 目标

编辑器 Prefab 集成：从选中实体创建预制体、拖放预制体到场景实例化、资源浏览器显示 Prefab（已有）。

## 背景

阶段二 P2 第一项。引擎 M4-04 已有 Prefab 数据层（from_scene/save/load/instantiate），编辑器此前无 Prefab UI。

## 技术方案

- Hierarchy「创建预制体」按钮 → `Prefab::from_scene`（AssetId::from_seed 由名称生成）+ 保存 `assets/prefabs/<name>.prefab` + 刷新资源；
- 资源浏览器 Prefab 条目可拖放（复用纹理 dnd）；
- 2D 场景视图 `dnd_drop_zone` 分发：`.prefab` → `Prefab::load` + `instantiate`（history 可撤销）；其他 → 创建精灵。

## 修改范围

- `engine/crates/editor/src/main.rs`

## 验收标准

- 选中实体可创建预制体并出现在资源浏览器；
- 拖放预制体到场景实例化（可撤销）；
- 质量门通过。

## AI 输出

已完成：

- `create_prefab_from_selected`（from_scene + save + 刷新资源）；
- `spawn_prefab_from_drop`（load + instantiate，history 可撤销）；`spawn_dropped_resource` 分发；
- 资源浏览器 Prefab 可拖放。

修改文件：

- `engine/crates/editor/src/main.rs`

验证结果：

- fmt/clippy -D warnings/全量测试/Debug/Release/--check 通过；编辑器启动正常；
- 创建/实例化 Prefab 由负责人实机确认。

已知问题：

- 无 P0/P1。Prefab 覆盖值编辑（set_override）未接 UI，拖放实例化为默认覆盖。

## 已知限制

Prefab 覆盖值编辑 UI 未实施（默认覆盖实例化）。
