# M8-06 场景、Lua 与编辑器接入

## 状态

ACCEPTED

## 目标

扩展场景模型和版本迁移，加入 `Transform3D`、`MeshRenderer`、`Camera3D`、`Light` 组件；为 Lua 提供创建/查询/变换/相机和光源的最小 API；编辑器 Inspector 先支持查看和编辑，不做 3D Gizmo；assetc package 和 `--pack` 保持兼容。

## 验收目标

- 旧场景（v1）可加载，迁移到 v2 后 roundtrip；
- 新场景保存/加载 roundtrip 与格式版本/迁移测试齐全；
- Lua 可移动角色（transform3d.move/set）和切换相机（camera.set_active）；
- 编辑器 `--check`、发布包和缺失资源错误均通过；
- 3D 渲染由场景/World 驱动（相机、光源、Mesh 均来自实体组件）。

## 设计

### 组件字符串格式（沿用 `key=value;`，向量用逗号分隔，参考 Collider `offset=x,y`）

- `Transform3D`：`position=x,y,z;rotation=rx,ry,rz,rw;scale=sx,sy,sz`
- `MeshRenderer`：`mesh=assets/models/<file>.glb`（`__ground_plane__` 为内置地面哨兵路径）
- `Camera3D`：`position=x,y,z;target=x,y,z;fov=<deg>;near=<n>;far=<f>`（aspect 由窗口决定）
- `Light`：`direction=x,y,z;color=r,g,b;intensity=<i>`（方向光）

### 场景格式版本

- `CURRENT_SCENE_VERSION` 1 → 2；v1 加载时迁移（组件为不透明字符串，无需数据变换，仅版本提升并记录 `meta migrated_from=1`）；
- 迁移接入 `Scene::from_text`，沿用 `SceneError::UnsupportedVersion` 边界。

### Lua API（script.rs 新增模块）

| 模块 | API | 说明 |
|---|---|---|
| `transform3d` | `attach/get/set/move` | 挂接/查询/设置/移动 3D 变换 |
| `mesh` | `set/get/clear` | MeshRenderer 路径 |
| `camera` | `set_active/get_active/get` | 切换/查询活动相机 |
| `light` | `set_active/get/set` | 光源参数 |

### game_runner

- Renderer 改为场景驱动：相机 uniform、光源 uniform、shadow 相机每帧从 World 读取并 `write_buffer` 更新；
- Mesh 几何按路径缓存（`HashMap<String, CachedMesh>`），实体 `Transform3D + MeshRenderer` 每帧生成 model 矩阵与材质 bind group 绘制。

### 编辑器

- Inspector 增加 Transform3D / MeshRenderer / Camera3D / Light 查看与编辑（DragValue + 文本路径），不做 Gizmo。

### assetc

- `ImportKind::from_path` 增加 `.scene` 识别为 `Scene`，导入缓存正常化；package 已整体复制 scenes/scripts/assets，`--pack` 兼容。

## 质量门

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace
cargo build --workspace --release
git diff --check
cargo run -p assetc -- --project C:\GitHub\p2\game --check
cargo run -p game_runner -- --project C:\GitHub\p2\game --check
```

## 当前验证

- 场景 v2 迁移与 roundtrip、3D 组件单测通过；实体行 3/4 字段兼容且无尾随空白；
- Lua `transform3d/mesh/camera/light` API 单测通过（移动/相机切换/光源设置/非法值拒绝）；
- 编辑器 Inspector 查看/编辑 3D 组件，`editor --lib inspector::` 32 项通过；
- 真实窗口：`entities=12` 加载（旧 v1 + 3D 共存）、3D 网格按场景缓存、`demo3d` 每 4 秒切换相机、2D 样板游戏不回归；
- assetc 识别 `.scene`，`--check` / `game_runner --check` / `editor --check` / package+`--pack --check` 全部通过；
- workspace fmt / Clippy(-D warnings) / test / Debug / Release / `git diff --check` 通过。

## 下一步

无。

## 备注

- M8 计划质量门中 `assetc -- --project <root> --check` 的命令有误：assetc 的 `--check` 是自检冒烟（不接受 `--project`），正确命令为 `assetc --check`；`game_runner --project <root> --check` 才是真实项目校验。
- 3D 演示实体与 `demo3d.lua` 已加入 `game/` 工程；M8-09 将整理为独立 3D Demo 与性能基线。
