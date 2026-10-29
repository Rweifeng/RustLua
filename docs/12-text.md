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

## 抗锯齿与 gamma（清晰度修复）

小字号（11~14px）下"字糊"是两个独立原因叠加，两条字形路径分工不同：

1. **抗锯齿斜坡太宽**（`vector_text.wgsl`）。旧版用 `smoothstep(-aa, aa, sd)`、`aa≈2`
   物理像素；11px 字号笔画本身才 1 像素，边缘是均匀灰、笔画永远到不了满亮度（实测峰值
   238/255）。现在用 **1 物理像素宽**的线性斜坡：
   `coverage = clamp(sd / px + 0.5, 0, 1)`，`px = fwidth(local)` 就是 1 物理像素在 em
   空间的长度，与原生光栅器的面积覆盖同一口径。
2. **混合空间不对**（两条路径都有）。渲染目标是 sRGB 格式，管线用 `ALPHA_BLENDING`，
   硬件在**线性**空间混合；原生 GDI/DirectWrite 在 gamma 空间按覆盖率插值。同一 alpha
   下线性混合的浅色字边缘明显更亮——就是肉眼看到的"光晕"。`gamma_coverage` 反解出线性
   混合要用的覆盖率：已知前景亮度 `luma`、假设背景（亮字按 0.16 深底、暗字按 0.72 浅底，
   按 `luma` smoothstep 过渡），求 alpha 使线性混合后的 sRGB 编码值等于
   `bg + (fg - bg) * coverage`。

原生 hinting 会把笔画吸附到整像素，覆盖率恒为 1；未 hinting 的轮廓做不到，所以
`vector_text.wgsl` 再按 `STEM_DARKEN = 0.25` 物理像素做一次膨胀补重量（42px 标题上不到
半个像素，字号越大影响越小）。

SDF 路径（`textured.wgsl`，WebGL 与 `text_render_mode = "sdf"` 共用）只加 `gamma_coverage`：
它的 4 点象限超采样本来就是为"细笔画整条落在采样点之间而消失"加的，再叠同样的膨胀会
让笔画发胖发浑（同尺寸实测：加 0.25 后实心像素 126、中间灰却到 234，而同尺寸 vector 只有
94，肉眼看是一团胖字），所以那条路径保持原斜坡与超采样。

离屏验收（`royale_text_probe` 按物理像素渲染整帧，同尺寸同一段 `AUTOPLAY`）：

```powershell
# 541×961 物理像素、vector 路径；第四个参数可换 sdf / auto
cargo run -q -p engine --example royale_text_probe -- E:/Qoder/ClashRoyale E:/Qoder/ClashRoyale/artifacts/text-crisp-541.png 541 961 vector
```

字盒 58×8 px 内（亮度 >200 记实心、60–200 记中间灰）：旧版峰值 238 / 实心 88 / 中间灰
157 → 新版 255 / 126 / 94；FreeType 把同一字体按 12px 光栅作参考是 255 / 119 / 126。

## 整物理 ppem 的 TrueType hinting 档（`text_render_mode = "hinted"`）

`vector` 档的 `STEM_DARKEN` 膨胀只是"补重量"的权宜：笔画宽度仍按比例缩放，11px 下
中灰像素占大头。原生 GDI/DirectWrite 锐利的真正来源是字体自带的 TrueType 指令
（`fpgm`/`prep`/`cvt `）：它们在**整数 ppem** 上把笔画宽度、笔位吸附到像素网格，
覆盖率因此能整块取 1。这一档就是把这套字节码原样跑一遍，产出 8-bit 覆盖率图集。

- **数据**：`rustlua_render::hinted::HintedAtlas`，`swash::scale::Render` 带
  `hint(true)` 光栅化，图集每字形一张覆盖率掩膜（RGBA 四通道同值）。
- **排版**：`TextLayout::build_hinted(text, origin, style, atlas, to_physical)`。
  `origin`/`style`/返回的 `size` 都是**设计像素**；内部按 `to_physical`（设计盒 →
  实际附件的 contain 比例）换算到物理像素排版，em = 图集 ppem，字形四边形与基线落在
  整物理像素上，最后顶点再除回设计像素。调用方必须令
  `ppem = round(设计字号 × to_physical)`——任何非整数缩放都会重新引入模糊。
- **宿主契约**：桌面 `game_app` 每帧按窗口尺寸算 `contain_rect` 的宽度比、
  `game_web` 按后备缓冲/`fit_now` 的 contain 盒、编辑器/离屏宿主默认 1.0，统一经
  `RuntimeScene::set_render_scale` 声明。比例变化会清空旧 ppem 的图集与上报版本。
- **度量同口径**：hinted 档的 `ui.measure_text` 经 `set_text_hint_scale` 拿到设计→物理
  比例，把字号量化成 `round(字号 × 比例)` 的整数 ppem 再除回设计像素
  （`rustlua_render::text::measure_text_with_hint_scale`），宽度与 `build_hinted` 的
  `mesh.size[0]` 同口径（单测按 0.05px 容差对账）。不量化时实画推进按 ppem 算、度量按
  设计字号算，差 3~4%：
  调用方按更窄的宽度居中，整行向右溢（实测打包窗口里菜单底部提示被右边缘裁掉）。
  高度仍是设计字号的近似——探针（ab_glyph）与光栅器（swash）读的 ascent/descent 表
  不同，行盒高度以绘制为准。切换档位/比例时宿主立刻重新声明；脚本加载期（`main.lua`
  顶层就调 `ui.measure_text`）也已按当时的 `render_scale` 生效。
- **图集命名与上传**：纹理路径 `font://hinted/<ppem>/<asset>`（`asset` 为空写
  `__default__`）。它落在 `font://` 命名空间里，渲染器复用自定义字体上传通道
  （`Rgba8Unorm` + Nearest 绑定），shader 按顶点 `sdf=3` 走"覆盖率直采"分支
  （`textured.wgsl`：`gamma_coverage(sampled.r) * color.a`，不再阈值化）。
- **预热顺序**：`prewarm_frame_hinted_glyphs` 在本帧**任何**文字网格构建之前把世界
  文字 / Lua 动态 UI / 场景实体 Text 三条通道的字形一次性光栅化。图集增长只重缩放已
  缓存字形的 UV、不回改已写进顶点缓冲的 UV，后一趟撑大图集就会让前一趟的 UV 失效。
- **回退**：字体读不到、或没有 `fpgm`/`prep`/`cvt `（如 Noto Sans SC VF）、或
  `world_sprite_depth` 打开（世界深度通道只接 SDF）、或像素字体（`is_pixel_font`）
  时，该段文字回退矢量 → SDF/点阵档，只告警一次。
- **字体要求**：需要一份**带 TrueType 指令**的可商用字体。Sarasa UI SC（SIL OFL 1.1）
  是目前验证过的选择；`subset_ttf` 必须在子集化时保留 `fpgm`/`prep`/`cvt `/`gasp`，
  否则发布包里的字体等于没 hinting。

11 物理像素下同一句中文（统一 alpha 度量，峰值 255）：

| 路径 | 实心像素占比 | 墨量 Σcoverage |
| --- | --- | --- |
| 引擎 hinted（Sarasa） | 44% | 288.8 |
| 引擎 vector（`STEM_DARKEN=0.25`） | 36% | 417.1（+44%） |
| 引擎 vector（`STEM_DARKEN=0`） | 22% | 270.2 |
| swash 未 hinting | 19% | 271.4 |

hinting 用接近未 hinting 的墨量拿到 44% 实心像素；0.25px 膨胀只能买到 36%，代价是
多 44% 的墨。14/18/24px 下膨胀的墨量代价分别是 +41%/+33%/+27%。

离屏复现（探针本体与参考光栅逐像素对账见 `engine/examples/text_path_probe.rs`）：

```powershell
cargo run -q -p engine --example text_path_probe -- E:/Qoder/fontprobe/fonts/SarasaUiSC-Regular.ttf 11 hinted out.png
```

运行时集成测试（需要一份带指令表的字体；含 `ui.measure_text` 跟 ppem 量化的
端到端门，以及 `rustlua-render` 里宽度与 `build_hinted` 对账的单测）：

```powershell
$env:RUSTLUA_HINTED_TEST_FONT = "E:/Qoder/fontprobe/fonts/SarasaUiSC-Regular.ttf"
cargo test -p engine --lib host::runtime_scene::tests::custom_fonts
cargo test -p rustlua-render --lib hint_quantized_measure_matches_build_hinted_size
Remove-Item Env:RUSTLUA_HINTED_TEST_FONT
```
