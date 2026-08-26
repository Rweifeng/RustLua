# M14-03 Play/Stop / 恢复 / 发布回归

## 状态

PASS。

## 验收范围

- 样板工程 `game_test_2d_production/` 的 `Play/Stop` 运行态隔离；
- 保存、关闭、重开、恢复链路；
- 发布前检查、资源打包、包体校验链路。

## 预期证据

### Play/Stop

- `sample_project_play_starts_preview_and_switches_to_game_tab`
- `sample_project_stop_restores_scene_tab_and_clears_preview_input`
- 如有新增 GUI 实机证据：Game View 启动、Stop 恢复、输入清空截图或录屏

### 恢复

- 恢复文件扫描 / 恢复 / 丢弃相关命令或测试输出
- 若补 GUI：恢复提示、恢复成功、恢复失败不覆盖主文件的证据

### 发布

- `editor --check --project "E:\Codex\p2\game_test_2d_production"`
- `game --check --project "E:\Codex\p2\game_test_2d_production"`
- `assetc --check-deps "E:\Codex\p2\game_test_2d_production"`
- `assetc package ...`
- `game --pack ... --check`

## 本轮结果

日期：2026-08-26

### Play/Stop

- PASS：`cargo test -p editor sample_project_play_starts_preview_and_switches_to_game_tab -- --nocapture`
- PASS：`cargo test -p editor sample_project_stop_restores_scene_tab_and_clears_preview_input -- --nocapture`

### 恢复

- PASS：`cargo test -p editor recovery::tests::autosave_scan_and_recover_preserves_original_path -- --nocapture`
- PASS：`cargo test -p editor sample_project_recovery_roundtrip_preserves_path_and_keeps_main_scene_clean -- --nocapture`
  - 在临时复制的 `game_test_2d_production/` 上修改 `scenes/production.scene`，写入恢复快照后重新扫描；
  - 恢复结果保留原路径 `scenes/production.scene` 和修改后的实体名；
  - 正式场景文件保持未被覆盖。
- PASS：`cargo test -p editor sample_project_scene_edit_save_and_reopen_persists_changes -- --nocapture`
  - 在临时复制的 `game_test_2d_production/` 上修改 `production.scene` 的实体名和场景元数据；
  - 保存并重新打开后，修改结果仍然存在，文档状态恢复为非 dirty。
- PASS：`cargo test -p editor app_startup_scans_recovery_and_recover_candidate_restores_scene -- --nocapture`
  - 编辑器启动时可扫描到 `.temp/` 下的恢复快照；
  - 执行恢复后，当前文档切换到恢复内容，层级与 Inspector 选中状态被清空；
  - 正式场景文件保持未被恢复快照覆盖。
- PASS：GUI 实机恢复验收（2026-08-26）
  - 编辑器启动后可见恢复提示；
  - 执行恢复后，场景内容回到最近一次有效编辑；
  - 恢复后继续编辑、保存、重开正常；
  - 正式场景文件未被恢复快照异常覆盖。

### 发布

- PASS：`cargo test -p editor publish_spawn_end_to_end -- --nocapture`
- PASS：`cargo run -q -p editor -- --check --project "E:\Codex\p2\game_test_2d_production"`
- PASS：`cargo run -q -p game -- --check --project "E:\Codex\p2\game_test_2d_production"`
- PASS：`cargo run -q -p assetc -- import "E:\Codex\p2\game_test_2d_production"`
- PASS：`cargo run -q -p assetc -- --check-deps "E:\Codex\p2\game_test_2d_production"`
- PASS：`cargo run -q -p assetc -- package "E:\Codex\p2\game_test_2d_production" -o "E:\Codex\p2\artifacts\m14\game_test_2d_production_pack"`
- PASS：`cargo run -q -p game -- --pack "E:\Codex\p2\artifacts\m14\game_test_2d_production_pack" --check`

### 过程修复

- 样板工程一度存在 `scenes/production.scene → assets/prefabs/slime_red.prefab (缺失)` 断链；
- 已恢复 `game_test_2d_production/assets/prefabs/slime_red.prefab`；
- 首次恢复时因空格替代了 tab 分隔符，导致 `assetc import` 报 Prefab 第 5 行 `entity` 记录格式错误；
- 改回与基线一致的 tab 分隔格式后：

```text
assetc_import_ok total=26 imported=0 reused=26 failed=0
assetc_check_deps_ok
assetc_package_ok files=27 compress=false encrypt=false
game_check_ok project=e:\Codex\p2\artifacts\m14\game_test_2d_production_pack
```

## 当前结论

`M14-03` 已通过：自动化、命令行与 GUI 实机恢复三类证据已闭环，`Play/Stop`、保存/关闭/重开、恢复、发布检查、打包与包体验证均满足当前里程碑要求。
