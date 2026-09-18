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

## 世界文字（WP4 增补）

新增 `WorldText` 组件：文字锚在实体的**世界位置**上，用 `Camera2D` 投影后走与屏幕 `Text`
**完全相同**的字形管线（含 vector / SDF 两种模式与自定义字体）。

组件串：`text=..;font_size=..;color=r,g,b[;font=..][;offset=x,y][;centered=true]`

- 文字字段原样转交 `UiText` 解析，因此字号/颜色/字体校验与转义只有一份（切分也复用其
  **转义感知**的切分器，文字里带分号不会被切坏）；
- `offset` 是投影到屏幕后的像素偏移（"头顶 12px"）；`centered=true` 按实测字宽水平居中；
  两者都**只在非默认时**写回组件串；
- 绘制层：精灵之上、屏幕 UI 之下，不参与世界深度；不受 `render_ui_overlay` 开关影响
  （它是世界内容，不是屏幕 UI）；
- 字形预热在构建任何字形网格**之前**完成，理由与屏幕 UI 文本相同（图集增长会让先构建的
  网格 UV 失效）；
- Lua 侧 `world_text.attach(id, 组件串)` / `world_text.detach(id)` 供运行期创建的实体使用
  （`entity.spawn` 只插 Transform）；
- 打包字符收集（`core/char_collect.rs`）同时扫描 `WorldText`，否则发布包会缺字。