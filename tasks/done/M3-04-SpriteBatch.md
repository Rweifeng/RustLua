# M3-04：SpriteBatch

- 状态：PASS
- 完成日期：2026-08-20

## 交付物

- `engine/crates/engine_core/src/sprite_batch.rs`
- `engine/crates/game_runner/examples/sprite_batch_demo.rs`
- `docs/09-sprite-batch.md`

## 验收证据

- SpriteBatch 专项测试：4 passed；
- Workspace 测试、Clippy、Debug/Release 构建和质量门通过；
- 真实 GPU Demo：
  `sprite_batch_demo_ok adapter="NVIDIA GeForce GT 1030" sprites=1000 vertices=6000 draw_calls=3 texture=1x1 target=256x256`。

## 结论

M3-04 已完成 Sprite 提交、稳定排序、相同纹理/图层合批、图集 UV 传递和 1000 Sprite GPU 绘制，允许进入后续 Camera2D/图集任务。