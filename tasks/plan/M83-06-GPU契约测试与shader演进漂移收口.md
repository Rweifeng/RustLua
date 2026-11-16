# 任务：GPU 契约测试与 shader 演进漂移收口

- 任务 ID：M83-06
- 状态：2 条已修（CRLF 假红）；4 条待修（真红）
- 关联里程碑：M83
- 创建日期：2026-11-16

## 目标

`verify.ps1` 全量门的 `Workspace tests` 段在 `rustlua-gpu --lib` 判红（302 passed / 6 failed）。
6 条全部是**对 shader 源码文本做断言的契约测试**，与运行期渲染逻辑无关。逐条定性后分两类：
2 条是本机 CRLF 检出导致的假红（已在 M83 内修复），4 条是 shader 演进后断言未同步的真红。

## 已修（2026-11-16，本笔）

| 用例 | 原因 | 修法 |
|---|---|---|
| `cutaway_both_sides::the_dial_forces_the_color_pass_and_leaves_shadows_alone` | `include_str!("../construct.rs")` 后做**多行** `contains("…\n…")`；Windows autocrlf 检出是 CRLF，`\n` 匹配不上 | `include_str!(..).replace("\r\n", "\n")`（沿用 `ai_tool_surface.json` 基线的既有先例） |
| `hdr::ssr_misses_fall_back_to_the_procedural_sky` | 同上（`construct.rs` 多行断言 176 B 缓冲） | 同上 |

这两条在 CI 的 LF 检出下本来就是绿的；修它只为**本机与 CI 判据一致**，避免贡献者被假红误导。

## 待修（4 条真红，均有复现命令）

复现（cwd=`engine`）：

```powershell
cargo test -p rustlua-gpu --lib --all-features `
  renderer::tests::shader_variants::pbr_diffuse_ibl_replaces_constant_ambient `
  renderer::tests::shader_variants::pbr_specular_ibl_samples_prefiltered_cube
cargo test -p rustlua-gpu --lib --all-features renderer::tests::water_absorption::tonemap_only_absorbs_inside_the_water_mask
cargo test -p rustlua-gpu --lib --all-features renderer::tests::hdr::tonemap_shader_adds_screen_space_water_reflection
```

### ① `shader_variants::pbr_diffuse_ibl_replaces_constant_ambient`（`tests/shader_variants.rs:186`）

断言 `!diffuse.source().contains("AMBIENT_FACTOR * radiance")`。
现状：`shaders/mesh3d_pbr.wgsl:1123-1126` 的 **`mesh_cheap` 廉价档**在 `IBL_DIFFUSE` 分支内
合法退回常数环境光（P3-R277 注释明写"与 IBL 关档同式"），因此该子串必然出现。
**判据意图**是"IBL 变体不得**无条件**叠常数环境光"，不是"一次都不许出现"。
修法方向：把断言改成"该子串只允许出现在 `if (mesh_cheap)` 守卫内"（例如定位
`if (mesh_cheap) {` 与其闭合块后只在该片段内搜索），而不是放宽成 `contains` 恒真——否则这条门会失去意义。

### ② `shader_variants::pbr_specular_ibl_samples_prefiltered_cube`（`tests/shader_variants.rs:212`）

同一断言在 `specular` / `full` 两个变体上各失败一次（`for source in [specular, full]`）。
根因与修法同 ①。

### ③ `water_absorption::tonemap_only_absorbs_inside_the_water_mask`（`tests/water_absorption.rs:76`）

断言 `absorb > mask`，其中 `mask = source.find("        if surface.valid {")`、
`absorb = source.find("let transmittance = exp(-params.water_abs.rgb")` —— 用的是**全文件第一次出现**。
现状：P3-R208 新增的 `underwater_radiance`（`shaders/tonemap.wgsl:675-689`）里在 681 行就有一处
`let transmittance = exp(-params.water_abs.rgb …`，位于水掩码块（989 行）**之前** ⇒ 断言恒假。
**判据意图**（吸收必须在水面像素分支内）仍然成立，只是取错了样本。
修法方向：把搜索起点放到 `mask` 之后（`source[mask..].find(absorb 串)`），或改为断言
"掩码块内存在吸收式且块外不再有以 `linear` 为输入的吸收式"。禁止直接把这两行删掉。

### ④ `hdr::tonemap_shader_adds_screen_space_water_reflection`（`tests/hdr.rs:489`）

断言 `source.contains("distorted + texel * vec2<f32>(2.0, 0.0)")` 失败：水面反射的扭曲写法已随
水面折射步进（`render.set_water_refract_steps`）改动，该字面量不再存在。
需按当前 `water_reflection` 实现重取锚点（先读 `shaders/tonemap.wgsl` 的 `fn water_reflection`
再定断言），并保留"反射必须在水平方向做一次性偏移，不许随机/多次采样"这一契约意图。

## 影响文件

- `engine/crates/rustlua-gpu/src/renderer/tests/shader_variants.rs`
- `engine/crates/rustlua-gpu/src/renderer/tests/water_absorption.rs`
- `engine/crates/rustlua-gpu/src/renderer/tests/hdr.rs`

## 风险

- 这 4 条是**源码形状断言**，改断言等于改判据；必须保留原意图（"不许无条件回退"、"吸收必须在掩码内"、
  "扭曲只有一处"），不能改成恒真或删除。
- 修改后必须整跑 `cargo test -p rustlua-gpu --lib`（单线程约 47 分钟，308 条），不能只跑改动的 4 条。

## 验收方式

```powershell
$env:RUST_TEST_THREADS='1'; cargo test -p rustlua-gpu --lib --all-features
```

期望：308 passed / 0 failed。随后 `verify.ps1` 的 `Workspace tests` 段应继续向后推进
（release build、game/editor `--check`、性能基线、GUI smoke、MVP 等）。

## 未完成项 / 已知限制

- 4 条真红未修：属水面/IBL shader 契约的拥有方工作，且需先确认 P3-R277 / P3-R208 / 折射步进的
  既定语义；本任务只提供定性与修法方向，不做猜测性改动。
- 本机 autocrlf 检出会让所有"多行 `contains`"型源码断言假红；除已修的 2 条外，其余同类测试
  目前恰好只做单行断言，暂未暴露。若要根治，应向 `.gitattributes` 增加 `*.rs text eol=lf`
  （代价是全仓一次换行归一化提交），本轮不做。
