# M9-05 3D 性能、发布与验收证据

日期：2026-08-22
GPU：NVIDIA GeForce RTX 3060 Laptop GPU
场景：game3d.scene（Ground + 4 墙 + Player3d + 2 敌人 + 3 拾取 + Camera + Sun，entities=13）

## 3D 性能基线（真实窗口）

Debug 构建：

| 指标 | 值 |
|---|---|
| 3D baseline | draws=32 / instances=32-34（剔除+实例化开启） |
| fps | ~112 |
| frame_ms | ~8-9（偶发 17.9 尖峰） |
| update_ms | ~0.02-1.3（含 physics3d 步进） |
| 场景加载 | `[scene] loaded name=game3d entities=13` |

Release 构建：

| 指标 | 值 |
|---|---|
| 3D baseline | draws=32 / instances=34 |
| fps | 330（vsync/Mailbox 上限） |
| frame_ms | ~1.4-1.7 |
| update_ms | ~0.003 |

与 M8 对比：demo3d 实例网格 64 个（draws=12），game3d 场景为真实游戏场景（32 个 draw，
因多个不同 Mesh 实例：warrior×3、triangle×3、ground），Release 帧耗时与 M8 相当
（~1.5ms / 330fps），Debug ~112fps 可玩。

## 发布验收

- `package.ps1 -SelfTest -Encrypt`：Release 构建 + assetc package（31 数据文件，加密）→
  `artifacts/builds/release/p2-game`（38 文件，15,151 KB）。
- 目录隔离自测：复制到含空格临时目录（`...\p2 release test 214427\p2-game`），
  `game_runner.exe --pack <dir> --check` 通过，无源码/target/.cache 依赖。
- 发布包实际运行：`p2-game/game_runner.exe` 独立运行，main.scene（2D）正常渲染，
  无 GPU/pipeline 校验错误；game3d.scene 与 kaykit-warrior.glb 等模型均在 pack 中
  （F4 循环可进入 3D 游戏）。

## 无 P0/P1

- 2D 样板游戏与 M8 3D demo 不回归；
- game3d 场景长时间运行无崩溃/panic；
- 缺失资源均走告警/明确错误，无 GPU 资源错误只能 panic 的路径。

## 质量门

`cargo fmt --all --check` / `cargo clippy --workspace --all-targets --all-features -- -D warnings`
/ `cargo test --workspace --all-features`（282 测试）/ `cargo build --workspace` / `--release`
/ `git diff --check` 全部通过。

## 标签

`v0.9-3d-game` 已创建。
