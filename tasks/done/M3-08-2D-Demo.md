# M3-08：2D Demo

- 状态：PASS
- 完成日期：2026-08-20

## 交付物

- `engine/crates/game_runner/examples/demo_2d.rs`
- `docs/13-2d-demo.md`

## 验收证据

真实 Release GPU Demo：

```text
demo_2d_ok adapter="NVIDIA GeForce GT 1030" sprites=7 sprite_draws=2 text_glyphs=35 text_vertices=210 textures=2 target=256x256
```

Workspace 和项目质量门在 M3-08 实现后重新执行并通过。

## 结论

M3-08 已完成，M3 的 2D 核心能力已由一个集成 Demo 串联：图集 Sprite、SpriteBatch、Camera2D、文本图集和 wgpu 离屏绘制均可运行。