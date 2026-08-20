# M3-08：2D Demo

## 目标

用一个真实 wgpu 离屏渲染 Demo 集成 M3 阶段的 2D 基础能力：Camera2D、Sprite、SpriteBatch、纹理图集和文本图集。

## Demo 内容

- 程序生成 4×4 四色 Sprite 图集；
- 注册 `red`、`green`、`blue`、`yellow` 四个命名区域；
- 创建 7 个 Sprite，分为两个图层；
- 使用 Camera2D 将世界坐标转换为 256×256 屏幕像素；
- SpriteBatch 按图层和纹理排序，生成两个绘制范围；
- 上传 Sprite 图集和内置字体图集两张 GPU 纹理；
- 渲染两行 ASCII 文本；
- 使用真实 wgpu RenderPass 提交到 256×256 离屏目标。

## 验收

```powershell
cargo run -p game_runner --example demo_2d --release
```

预期输出：

```text
demo_2d_ok adapter="..." sprites=7 sprite_draws=2 text_glyphs=35 text_vertices=210 textures=2 target=256x256
```

该输出同时证明 Sprite、SpriteBatch、Camera2D、图集、文本布局、两张纹理上传和 GPU 绘制均已进入同一条可运行链路。该 Demo 是离屏自动验收样例，不替代后续窗口交互 Demo。