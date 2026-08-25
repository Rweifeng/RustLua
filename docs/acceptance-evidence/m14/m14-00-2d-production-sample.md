# M14-00 2D 生产样板工程验收

## 结论

PASS。已创建独立样板工程 `game_test_2d_production/`，并完成资源导入、依赖检查、编辑器检查、运行时检查与发布包检查。

## 样板内容

- 主菜单：`scenes/menu.scene`
  - `Text / Image / Button` 组件直接构成入口 UI；
  - `Start Production` 与 `Load Save` 由 `scripts/main.lua` / `scripts/menu.lua` 分发。
- 生产主场景：`scenes/production.scene`
  - 100x100 Tilemap；
  - 玩家三态动画（idle/move/attack）；
  - 两类敌人 Prefab：`assets/prefabs/slime_green.prefab`、`assets/prefabs/slime_red.prefab`；
  - 10 个拾取物；
  - HUD、BGM/音效、Lua 玩法、存档与返回菜单按钮。

## 自动化结果

```text
cargo fmt --all -- --check
PASS

cargo test --workspace --all-features
PASS

cargo run -q -p assetc -- scan "E:\Codex\p2\game_test_2d_production"
PASS
assetc_scan_ok total=27 supported=26 unsupported=1 failed=0

cargo run -q -p assetc -- import "E:\Codex\p2\game_test_2d_production"
PASS
assetc_import_ok total=26 imported=26 reused=0 failed=0

cargo run -q -p assetc -- --check-deps "E:\Codex\p2\game_test_2d_production"
PASS
assetc_check_deps_ok

cargo run -q -p editor -- --check --project "E:\Codex\p2\game_test_2d_production"
PASS
editor_project_ok root=\\?\E:\Codex\p2\game_test_2d_production baseline=v0.10-editor

cargo run -q -p game -- --check --project "E:\Codex\p2\game_test_2d_production"
PASS
game_check_ok project=E:\Codex\p2\game_test_2d_production

cargo run -q -p assetc -- package "E:\Codex\p2\game_test_2d_production" -o "E:\Codex\p2\artifacts\m14\game_test_2d_production_pack"
PASS
assetc_package_ok files=27 compress=false encrypt=false

cargo run -q -p game -- --pack "E:\Codex\p2\artifacts\m14\game_test_2d_production_pack" --check
PASS
game_check_ok project=E:\Codex\p2\artifacts\m14\game_test_2d_production_pack
```

## 过程问题

- 首轮导入时暴露了两个格式问题：
  - `production.scene` 由生成脚本写成带 BOM 的 UTF-8，触发场景首行解析失败；
  - 两个 `.prefab` 文件的根实体行缺少空父字段，触发 Prefab 解析失败。
- 两个问题均已修正后重新执行 `assetc clean/import/check-deps`，结果通过。

## 后续使用建议

- `M14-01` 直接围绕 `game_test_2d_production/` 走完整编辑器操作链：导入、TilePaint、Prefab 实例化/写回、动画编辑、UI 调整、保存重开、Play/Stop；
- `M14-02` 在此工程上做资源重命名、删除、断链与修复回归；
- `M14-03` 继续使用同一工程跑恢复与发布回归，避免样本漂移。
