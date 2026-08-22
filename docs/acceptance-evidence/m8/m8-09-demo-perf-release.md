# M8-09 3D Demo、性能与发布验收证据

日期：2026-08-22  
GPU：NVIDIA GeForce RTX 3060 Laptop GPU  
窗口：1280×720，Mailbox；场景：main.scene（entities=13）+ demo3d.lua 实例网格

## 3D Demo（可复现要素）

- 角色移动：骷髅战士绕场行走（`transform3d.set` 驱动）；三角形摆动（`transform3d.move`）。
- 骨骼动画：战士剪辑每 6 秒切换（Idle→Running_A→1H_Melee_Attack_Chop→Idle）。
- 相机切换：每 4 秒 Camera↔Camera2（`camera.set_active`）。
- 光照与阴影：方向光 + shadow map（M8-05）。
- 深度遮挡：3D depth pass（M8-04）。
- 实例化 + 剔除：8×8 默认 / `P2_INSTANCE_COUNT` 可调实例网格。
- 调试 overlay：fps/frame/update/lua 四项（M1-07）。

## 性能基线（真实窗口）

Debug 构建：

| 实例数 | draws | 可见实例 | fps | frame_ms | update_ms |
|---|---|---|---|---|---|
| 64（默认） | 12 | 48–55 | ~127 | ~7.0 | ~0.1 |
| 100 | 12 | 70 | ~130 | ~7.0 | ~0.15 |
| 1000 | 12 | 367 | ~47 | ~21 | ~1.7 |

Release 发布包（64 实例）：

| 配置 | draws | instances | fps | frame_ms |
|---|---|---|---|---|
| 默认（剔除+实例化） | 12 | 55 | ~330 | ~1.3 |
| `--no-cull --no-instance` | 76 | 76 | ~279 | ~3.9 |

- Draw Call：相同 triangle 实例合并为 1 次 instanced draw；1000 实例仅 12 次绘制。
- CPU/GPU 帧耗时：1000 实例（367 可见）debug ~21ms、update ~1.7ms；Release ~1.3ms。
- 场景加载：`[scene] loaded name=main entities=13`（含蒙皮战士与 3D 组件），`[render] initialized` 后约 0.3s 完成实例网格生成。

## 发布验收

- `package.ps1 -SelfTest -Encrypt`：Release 构建 + assetc package（28 数据文件，加密）→ `artifacts/builds/release/p2-game`（35 文件，14.2 MB）。
- 目录隔离自测：复制到含空格临时目录（`...\p2 release test 181204\p2-game`），`game_runner --pack <dir> --check` 通过，无源码/无 target/.cache 依赖。
- 发布包实际运行：`p2-game/game_runner.exe` 独立运行，3D 场景渲染（draws=12/instances=55）、330fps、无 GPU/pipeline 校验错误。
- `game_runner --project game --check`、`assetc --check`、`editor --check` 通过。

## 无 P0/P1 检查

- 2D 样板游戏不回归（tilemap/sprite/脚本正常加载）；
- 1000 实例下无崩溃/panic（debug 掉帧但稳定运行）；
- 无 GPU 资源错误只能 panic 的路径；缺失资源均走告警/明确错误。

## 质量门

- `cargo fmt --all --check` / `cargo clippy --workspace --all-targets --all-features -- -D warnings` / `cargo test --workspace --all-features`（exit 0）/ `cargo build --workspace` / `--release` / `git diff --check` 全部通过。

## 标签

- `v0.8-3d-foundation` 已创建。
