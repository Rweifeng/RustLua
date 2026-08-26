# M14-03 Play/Stop / 恢复 / 发布回归

## 状态

ACCEPTED，2026-08-26。

## 目标

围绕 `game_test_2d_production/` 验证 `M14` 的运行态隔离、恢复和发布主链：

- `Play/Stop` 在真实样板工程上稳定工作，不污染编辑态；
- 保存、关闭、重启、恢复链路可用，异常中断后能保留最近一次有效编辑；
- 发布前检查、资源打包、包体校验在样板工程上可重复通过。

## 输入

- 样板工程：`game_test_2d_production/`
- 已归档任务：
  - `tasks/done/M14-00-新建2D生产样板工程.md`
  - `tasks/done/M14-01-端到端编辑器工作流.md`
  - `tasks/done/M14-02-资源变更断链修复回归.md`
- 规划文档：`docs/30-编辑器可靠性-2D工作流-资源系统闭环实施计划.md`

## 当前计划

- 复用现有样板 `Play/Stop` 自动化回归，扩展到关闭/重开/恢复口径；
- 用命令行补一轮发布前检查、打包、包体校验链路；
- 真实样板工程上已补恢复回归：编辑 `production.scene` 后写入恢复快照，扫描/恢复结果保留原路径与修改内容，且不覆盖正式场景文件；
- 真实样板工程上已补保存/关闭/重开回归：修改 `production.scene` 后保存并重开，实体修改与场景元数据都能持久化；
- 编辑器恢复菜单接线已补自动化回归：启动时能扫描到恢复快照，执行恢复后会把当前场景切回恢复内容，并清理选择状态；
- 已补 GUI 实机恢复验收：恢复提示可见、恢复后场景内容回到最近一次有效编辑、正式场景文件未被异常覆盖。

## 当前进展

- 已完成：
  - `Play/Stop` 自动化回归通过：
    - `sample_project_play_starts_preview_and_switches_to_game_tab`
    - `sample_project_stop_restores_scene_tab_and_clears_preview_input`
  - 恢复库层回归通过：
    - `recovery::tests::autosave_scan_and_recover_preserves_original_path`
    - `document::tests::sample_project_recovery_roundtrip_preserves_path_and_keeps_main_scene_clean`
    - `document::tests::sample_project_scene_edit_save_and_reopen_persists_changes`
    - `panels::header::tests::app_startup_scans_recovery_and_recover_candidate_restores_scene`
  - 发布链路端到端测试通过：
    - `publish_spawn_end_to_end`
  - 命令行检查结果：
    - `editor --check --project "E:\Codex\p2\game_test_2d_production"` 通过；
    - `game --check --project "E:\Codex\p2\game_test_2d_production"` 通过；
    - `assetc import "E:\Codex\p2\game_test_2d_production"` 通过；
    - `assetc --check-deps "E:\Codex\p2\game_test_2d_production"` 通过；
    - `assetc package ...` 与 `game --pack ... --check` 通过。
  - 已处理样板工程现状问题：
    - 恢复 `game_test_2d_production/assets/prefabs/slime_red.prefab`；
    - 修正恢复文件的 tab 分隔格式，使其重新可被 `assetc import` 正常解析。
  - GUI 实机恢复验收通过（2026-08-26）：
    - 编辑器启动后可见恢复提示；
    - 执行恢复后，场景内容回到最近一次有效编辑；
    - 恢复后继续编辑、保存、重开正常；
    - 正式场景文件未被恢复快照异常覆盖。

## 优先验证项

1. `Play/Stop`
   - 菜单场景可进入 `Play`；
   - `Stop` 后恢复 Scene 标签与编辑快照；
   - 输入状态清空，不残留运行态修改。
2. 恢复
   - 编辑后生成恢复快照；
   - 关闭/重开后可扫描恢复候选；
   - 恢复失败不覆盖主场景文件。
3. 发布
   - `editor --check`、`game --check`、`assetc --check-deps` 通过；
   - `assetc package` 输出可运行包；
   - `game --pack --check` 通过。

## 验证入口

```powershell
cargo fmt --all -- --check
cargo test --workspace --all-features
cargo test -p editor sample_project_play_starts_preview_and_switches_to_game_tab -- --nocapture
cargo test -p editor sample_project_stop_restores_scene_tab_and_clears_preview_input -- --nocapture
cargo test -p editor publish_spawn_end_to_end -- --nocapture
cargo run -q -p editor -- --check --project "E:\Codex\p2\game_test_2d_production"
cargo run -q -p game -- --check --project "E:\Codex\p2\game_test_2d_production"
cargo run -q -p assetc -- --check-deps "E:\Codex\p2\game_test_2d_production"
cargo run -q -p assetc -- package "E:\Codex\p2\game_test_2d_production" -o "E:\Codex\p2\artifacts\m14\game_test_2d_production_pack"
cargo run -q -p game -- --pack "E:\Codex\p2\artifacts\m14\game_test_2d_production_pack" --check
```

## 预期输出

- 任务记录：`tasks/M14-03-PlayStop恢复发布回归.md`
- 验收证据：`docs/acceptance-evidence/m14/m14-03-play-stop-recovery-publish.md`
- 如有新增自动化：对应 `editor` / `game` / `assetc` 测试用例

## 当前结论

`M14-03` 已完成并验收：`Play/Stop`、保存/关闭/重开、恢复、发布检查、打包与包体验证，以及 GUI 实机恢复都已通过。下一步可以转入 `M14-04` 做最终垂直验收。
