# M8-06 场景、Lua 与编辑器接入证据

日期：2026-08-22  
GPU：NVIDIA GeForce RTX 3060 Laptop GPU  
质量门：workspace fmt/Clippy(`-D warnings`)/test/build(Debug+Release)/git diff --check

## 场景格式与迁移

- `CURRENT_SCENE_VERSION` 1 → 2；`Scene::from_text` 将 v1 场景迁移到 v2 并记录 `meta migrated_from=1`。
- 单测：`version_1_scene_migrates_to_current_and_roundtrips`、`rejects_zero_or_future_version`、`version_2_scene_roundtrips_with_3d_components`、`migration_only_runs_for_older_versions`（engine_core `scene::` 共 7 项通过）。
- 实体行支持 3/4 字段两种写法，无父实体不再输出尾随空字段（`git diff --check` 通过）。

## 3D 组件与 Lua API

- 新组件：`Transform3D`（`transform3d.rs`）、`Camera3D` 组件解析（`camera3d.rs`）、`Light`（`light.rs`）、`MeshRenderer`（`mesh3d.rs`），均有 `from_component_str`/`to_component_str` 与单测。
- Lua 新增 `transform3d` / `mesh` / `camera` / `light` 模块；`instantiate_scene` 解析 3D 组件进 World；`camera.set_active` / `light.set_active` 由宿主读取。
- 单测：`lua_3d_api_moves_mesh_and_switches_camera`（移动 + 相机切换 + 光源设置）、`lua_3d_attach_and_validation_errors`（非法缩放/非 glb 路径拒绝）。

## 编辑器 Inspector

- Inspector 支持查看/编辑 `Transform3D` / `MeshRenderer` / `Camera3D` / `Light`（无 3D Gizmo）；3D 组件仅在实体已携带时应用，避免给普通实体注入默认组件。
- 单测：`editor --lib inspector::` 32 项通过（含 transform3d/camera3d/light/mesh_renderer 各字段与非法值拒绝）。

## 真实窗口证据（game_runner，加载改造后的 game/scenes/main.scene）

`artifacts/logs/game.log`：

- `[scene] loaded name=main entities=12` —— 旧 v1 场景迁移加载，2D（7 实体）+ 3D（5 实体）共存；
- `[scene] 'main' tilemap 10x8`、`project script loaded` —— 2D 样板游戏正常；
- `[render] 3D mesh cached path=assets/models/m8-test-triangle.glb` —— 场景 `MeshRenderer` 驱动几何加载；
- `[render] 3D mesh cached path=__ground_plane__` —— 内置地面哨兵合成接收平面；
- `demo3d: switched camera to 'Camera2'` / `'Camera'`（每 4 秒）—— Lua `camera.set_active` 切换相机生效；
- `lifecycle Running` —— 窗口运行无脚本错误（仅 Vulkan 加载器无害注册表告警）。

## assetc 与发布

- `ImportKind::from_path` 增加 `.scene` → `Scene`；`IMPORTER_VERSION` 升至 `assetc-m8-06` 触发重导。
- `assetc --check`（自检冒烟）通过：`assetc_check_ok scanned=1 imported=1 reused=0`；
- `game_runner --project game --check` 通过：`game_runner_check_ok`；
- `editor --project game --check` 通过：`editor_project_ok`；
- `assetc package game -o <out> --encrypt` 通过：`assetc_package_ok files=26`；打包目录经 `game_runner --pack <out> --check` 校验通过；
- 缺失 3D 网格错误单测：`verify_project_reports_missing_3d_mesh`、`extract_asset_refs_scans_3d_components`。

## 质量门

- `cargo fmt --all --check` 通过；
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` 无告警；
- `cargo test --workspace --all-features` 全部通过（exit 0）；
- `cargo build --workspace` 与 `cargo build --workspace --release` 通过；
- `git diff --check` 通过。
