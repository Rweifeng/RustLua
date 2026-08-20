# M3-07：文本

## 目标

提供一个无需外部字体文件即可运行的第一版 2D 文本能力：内置 ASCII 位图字体图集、确定性的字符串布局、换行、缩放、字符间距、行间距和颜色，并通过 wgpu 上传图集后绘制文本。

## 设计

- `TextAtlas`：生成 128×48 RGBA8 内置字体图集，每个字符占 8×8 单元；当前覆盖可打印 ASCII，未知字符回退到 `?`。
- `TextStyle`：控制缩放、字符间距、行间距和顶点颜色。
- `TextLayout::build`：把字符串转换为 6 顶点一个字符的三角形列表，支持 `\n` 换行并返回字符数、行数和包围尺寸。
- 坐标使用现有 M3-01 的左上角屏幕像素约定，UV 使用纹理左上角原点。
- 当前版本定位为稳定的 ASCII/调试文本基础，不宣称支持字体文件、Unicode shaping、抗锯齿或复杂脚本；这些能力留给后续字体资源任务。

## 验收

专项测试：

```powershell
cargo test -p engine_core text
```

真实 GPU Demo：

```powershell
cargo run -p game_runner --example text_demo --release
```

预期输出包含 `glyphs=17 lines=2 vertices=102 texture=128x48 target=128x128`，表示文本已完成布局、图集上传并提交 GPU 绘制。