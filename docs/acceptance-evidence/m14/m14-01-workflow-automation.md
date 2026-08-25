# M14-01 端到端工作流自动化基线

## 状态

IN PROGRESS。本文记录的是自动化基线，不代表 GUI 实机流转已全部完成归档。

## 本次新增回归

### assetc

文件：`engine/crates/assetc/src/lib.rs`

- `sample_project_import_checkdeps_and_package_pass`
  - 复制真实样板工程 `game_test_2d_production/` 到临时目录；
  - 执行 `import`；
  - 断言 `check_dependencies` 为空；
  - 执行 `package`；
  - 断言发布清单中包含 `scenes/production.scene` 与 `assets/prefabs/slime_green.prefab`。

### editor

文件：`engine/crates/editor/src/document.rs`

- `sample_project_scene_document_roundtrips_cleanly`
  - 打开样板 `scenes/production.scene`；
  - 断言场景实体与 Prefab 元数据存在；
  - 保存后重开，断言 roundtrip 一致。
- `sample_project_prefab_document_roundtrips_cleanly`
  - 打开样板 `assets/prefabs/slime_red.prefab`；
  - 断言 Prefab 文档模式成立；
  - 保存后重新加载，断言 roundtrip 一致。

### editor interaction

文件：

- `engine/crates/editor/src/panels/scene_view.rs`
- `engine/crates/editor/src/panels/inspector.rs`

- `sample_project_prefab_drop_instantiates_prefab_metadata`
  - 在样板 `menu.scene` 上模拟从资源面板拖入 `slime_green.prefab`；
  - 断言场景生成了带 `prefab.source/prefab.source_id/prefab.root` 元数据的实例；
  - 断言场景标记为 dirty，覆盖 Prefab 拖入主链。
- `sample_project_reapply_prefab_preserves_overrides`
  - 在样板 `production.scene` 中修改绿色史莱姆实例的名称和 Transform；
  - 执行“重新应用源 Prefab”；
  - 断言实例被重建后仍保留 override，覆盖 Inspector 里的回源链路。
- `sample_project_save_prefab_to_source_updates_all_instances`
  - 在样板 `production.scene` 中修改绿色史莱姆实例名称；
  - 执行“写回源 Prefab”；
  - 断言 `slime_green.prefab` 已更新，当前实例完成重建；
  - 断言同源实例仍保留各自的名称 override，覆盖写回后全场景同步更新链路。

### engine runtime

文件：`engine/crates/engine/src/host/runtime_scene.rs`

- `production_sample_menu_scene_instantiates_ui`
  - 加载 `menu.scene`；
  - 断言 2 个按钮、1 个图片、至少 3 个文本实体被实例化；
  - 断言可生成可见 2D/UI draw。
- `production_sample_scene_instantiates_gameplay_entities`
  - 加载 `production.scene`；
  - 执行一次脚本步进；
  - 断言存在 1 个 Tilemap、11 个动画实体、1 个菜单按钮；
  - 断言运行时已加载核心纹理并产生 2D draw。

### engine script

文件：`engine/crates/engine/src/core/script.rs`

- `production_sample_main_script_runs_menu_update`
  - 真实加载 `game_test_2d_production/scripts/main.lua`；
  - 在 `menu.scene` 上断言 `update()` 返回 `menu_ready`；
  - 断言菜单场景中存在 2 个按钮。
- `production_sample_start_button_loads_production_scene`
  - 在 `menu.scene` 中模拟点击开始按钮；
  - 断言 `scene.load("production")` 生效并进入生产场景；
  - 断言生产场景 update 返回 `production_ok...`，并实例化 Tilemap 与动画实体。
- `production_sample_can_save_and_return_to_menu`
  - 在生产场景中模拟 `F1` 存档；
  - 断言 save 文件真实写出；
  - 再模拟 `F4` 返回菜单，断言回到 `menu_ready` 且菜单按钮重新出现。

## 过程修复

- 在新增真实主脚本回归时发现样板工程按钮点击分发存在真实问题：
  - 运行时 `on_click(name)` 传入的是按钮实体名；
  - 样板脚本原先按按钮 label（如 `Start Production`、`Back to Menu`）判断；
  - 结果是菜单按钮和返回菜单按钮在真实点击下不会按预期切场景。
- 已修复：
  - `game_test_2d_production/scripts/menu.lua`
  - `game_test_2d_production/scripts/main.lua`
- 在新增编辑器写回源 Prefab 回归时发现另一处真实问题：
  - 从场景实例构建新 Prefab 时直接沿用了场景内实体 ID；
  - 写回后重新同步实例时，重建逻辑按 `prefab.source_id` 比对源结构；
  - 结果导致样板实例被误判为“结构不一致”，无法完成写回更新。
- 已修复：
  - `engine/crates/editor/src/panels/inspector.rs`
  - 写回阶段改为按 `prefab.source_id` 归一化实体 ID，再保存新 Prefab 并重建实例。
- 修复后全量工作区测试通过。

## 验证命令

```powershell
cargo fmt --all
cargo test -p assetc sample_project_import_checkdeps_and_package_pass -- --nocapture
cargo test -p editor sample_project_ -- --nocapture
cargo test -p editor sample_project_prefab_drop_instantiates_prefab_metadata -- --nocapture
cargo test -p editor sample_project_reapply_prefab_preserves_overrides -- --nocapture
cargo test -p editor sample_project_save_prefab_to_source_updates_all_instances -- --nocapture
cargo test -p engine production_sample_ -- --nocapture
cargo test -p engine production_sample_main_script_runs_menu_update -- --nocapture
cargo test -p engine production_sample_start_button_loads_production_scene -- --nocapture
cargo test -p engine production_sample_can_save_and_return_to_menu -- --nocapture
cargo test --workspace --all-features
```

## 结果

```text
cargo test -p assetc sample_project_import_checkdeps_and_package_pass -- --nocapture
PASS

cargo test -p editor sample_project_ -- --nocapture
PASS（5 tests：editor lib 2 + editor main 3）

cargo test -p editor sample_project_prefab_drop_instantiates_prefab_metadata -- --nocapture
PASS

cargo test -p editor sample_project_reapply_prefab_preserves_overrides -- --nocapture
PASS

cargo test -p editor sample_project_save_prefab_to_source_updates_all_instances -- --nocapture
PASS

cargo test -p engine production_sample_ -- --nocapture
PASS（2 tests）

cargo test -p engine production_sample_main_script_runs_menu_update -- --nocapture
PASS

cargo test -p engine production_sample_start_button_loads_production_scene -- --nocapture
PASS

cargo test -p engine production_sample_can_save_and_return_to_menu -- --nocapture
PASS

cargo test --workspace --all-features
PASS
- assetc: 10 passed
- editor lib: 74 passed
- editor main: 34 passed
- engine lib: 297 passed
- game: 6 passed
```

## 结论

`game_test_2d_production/` 已进入仓库自动化回归面，当前已覆盖资源链路、文档 roundtrip、编辑器内 Prefab 拖入 / 回源 / 写回、运行时场景实例化，以及真实主脚本的菜单点击 / 场景切换 / 存档 / 返回菜单工作流。后续继续做 `M14-01` GUI 实机流转时，已经有一层更接近真实编辑操作与真实玩法链路的自动化兜底。
