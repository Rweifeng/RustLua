# M12 阶段 2 验收证据（依赖图与失效传播）

日期：2026-08-25
基线：`57df5c4`（M12-02 C2 依赖提取器）起 5 个提交
范围：按已批准计划 `M12 阶段 2`（5 个 C 提交）执行 M12-02 依赖图与失效传播。

## 总状态

**代码/自动化侧：PASS（5 提交质量门全过，workspace 测试全绿）。**
**实机 `assetc --check-deps`：PASS（2026-08-25，game_test 断链完整链路输出，见下）。**
**编辑器 GUI 断链图标/错误红字视觉呈现：待负责人实机复核（自动化测试已覆盖，无截图/真实指针环境限制）。**

## 提交清单（5 个 C）

| 提交 | 内容 |
|---|---|
| `2cebeea` | C1 `ResourceState::Invalid` 持久化 + DB 遍历/反向 API（`iter`/`consumers_of`/`find_by_path_ci`/`propagate_invalidation`） |
| `dbdc067` | C2 依赖提取器（`dependency.rs`：`DependencyRef`/`extract_refs`/`resolve_refs`/`recompute_edges`）+ `verify_project` 去重 |
| `57df5c4` | C3 失效传播（`propagate` rank-max 不动点）+ 循环检测（`detect_cycles` 三色 DFS）——代码随用户提交合并 |
| `fb7a312` | C4 editor/assetc 接线 + UI 状态图标 + entries 重建修复（含 Lua char boundary panic 修复） |
| `9e7502e` | C5 `assetc --check-deps` 发布检查完整链路 |

## M12-02 依赖图与失效传播

- `engine/src/core/dependency.rs`：
  - 提取器：Scene/Prefab 扫 `assets/` 字面量（`;`/`:`/`\n`/`\r` 终止，排除 `__ground_plane__`）；Lua 引号感知状态机（`require("x")`→scripts/x.lua、`scene.load("x")`→scenes/x.scene 仅字面量、`assets/` 字面量、`--` 注释跳过）；`resolve_refs` 大小写回退 + 自环排除；`recompute_edges` 全量覆盖依赖边（幂等）。
  - 传播：rank-max 不动点（单向、幂等、可撤销）；`Missing/Orphaned=7`、`Invalid=6`、`ImportFailed=5`、`Stale=4`、`Discovered=3`、`Ready=2`；依赖侧 Missing/Orphaned/ImportFailed 折叠 broken-link 6；仅依赖拖后腿时写 error 文案；依赖健康时撤销 Invalid。
  - 循环检测：三色 DFS + 显式路径栈，rotate-to-min-id 去重；cycle error 先写、传播不覆盖。
- `editor`：`refresh()` 时序 register→reconcile→recompute_edges→propagate_invalidation→save，reconcile 后用最新状态重建 entries（修复文件删除后 UI 不显示 Missing 的旧缺陷）；`pending_imports` 排除 Invalid（断链重导自身无用）；`spawn_resource_import` 批次后传播+保存（导入恢复依赖→消费者撤销）；资源面板状态图标（Stale 黄▲ / Missing 灰✕ / ImportFailed 红✕ / Invalid 红「断链」）。
- `assetc`：`scan()` 同时序重建条目（输出反映 invalid/missing）；`import()` 批次后传播+保存；`check_dependencies(project)` 对每个 Invalid 沿依赖下探到断链根输出完整链路，成环截断标注。
- 修复：`extract_lua_refs` 曾把字符索引当字节索引切片（`text[i..]`），含中文 Lua 源码 + `scene.load` 组合触发 char boundary panic——接线后由真实 game_test 文件暴露，改为 chars 切片比较（C4 内修复，`import_spawn_end_to_end` 回归）。

## 自动化测试

- 提取器：`scene_refs_sprite_image_animation_tilemap_mesh`（真实场景行）、`prefab_uses_same_format`、`lua_require_modules`、`lua_scene_load_literal_only`（变量→Dynamic）、`lua_asset_paths`、`excludes_ground_plane`、`comment_aware_scan`、`dedupe_preserves_order`、`resolve_self_cycle_excluded`、`recompute_edges_*`；
- 传播：`propagate_missing_dep_invalidates_consumer`、`propagate_import_failed_dep_invalidates`、`propagate_stale_dep_marks_stale`、`propagate_restore_revokes_invalid`、`propagate_self_missing_keeps_missing`、`propagate_chain_a_to_c`、`propagate_cycle_terminates`、`cycle_error_preserved_when_ready`、`cycle_detection_reports_chain`；
- 接线：`refresh_marks_scene_invalid_when_dependency_missing`、`pending_imports_exclude_invalid`、`refresh_restores_ready_when_dependency_returns`、`assetc::scan_reports_invalid_when_dependency_missing`；
- `--check-deps`：`check_deps_reports_full_chain_consumer_to_root`（scene→prefab→纹理三层）、`check_deps_truncates_on_cycle`（成环截断标注）、`check_deps_empty_when_healthy`。

## 实机验证记录（`assetc --check-deps`，game_test，2026-08-25）

debug assetc 二进制（`cargo run -q -p assetc`）实测，恢复后项目完好：

| 场景 | 结果 |
|---|---|
| 健康 game_test | `assetc_check_deps_ok`（无断链） |
| 移走 `assets/textures/player.png` | `assetc_check_deps_found=2`，输出 `scenes/arena.scene → assets/textures/player.png (缺失)` 与 `scenes/main.scene → assets/textures/player.png (缺失)` 完整链路 |
| 恢复 player.png | 回到 `assetc_check_deps_ok` |

## 质量门结果（每 C 提交后执行，不含 release——负责人要求 debug 验证）

| 项 | 结果 |
|---|---|
| cargo fmt --all -- --check | 通过 |
| cargo clippy --workspace --all-targets --all-features -- -D warnings | 通过（0 警告） |
| cargo test --workspace --all-features | 通过（engine 288 + game 6 + assetc/editor 全绿） |
| git diff --check | 通过 |

## 待负责人实机复核（PENDING）

- 编辑器 GUI：资源面板断链图标（Invalid 红「断链」）与错误红字的实际视觉呈现；删除/恢复依赖后刷新 → Invalid/Ready 切换的实机确认。

## 后续阶段（不在本次）

M12-03 资源浏览器生产化（右键菜单/依赖面板/缩略图）、M12-04 校验报告（含 webp/mp3）、M12-05 迁移恢复框架；发布依赖跟随/裁剪。
