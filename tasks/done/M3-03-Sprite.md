# M3-03：Sprite

- 状态：PASS
- 完成日期：2026-08-20

## 交付物

- `engine/crates/engine_core/src/sprite.rs`
- `engine/crates/game_runner/examples/sprite_demo.rs`
- `docs/08-sprite.md`

## 验收证据

- Sprite 单元测试：3 passed；
- 真实 GPU Demo：
  `sprite_demo_ok adapter="NVIDIA GeForce GT 1030" vertices=6 texture=1x1 target=64x64`；
- 完整 workspace 测试、Clippy、Debug/Release 构建及质量门日志见本任务完成后的验收记录。

## 结论

M3-03 单 Sprite 数据模型、顶点生成、UV/pivot/Transform/tint 以及真实 GPU 绘制链路已完成，允许进入 M3-04 SpriteBatch。