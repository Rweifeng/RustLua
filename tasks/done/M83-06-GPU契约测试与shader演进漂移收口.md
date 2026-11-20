# 任务：GPU 契约测试与 shader 演进漂移收口

- 任务 ID：M83-06
- 状态：已完成并验收（2026-11-16）——6 条全修，整跑 `rustlua-gpu --lib` 308/308 绿；已归档 `done/`
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

## 已修（4 条真红，2026-11-16 第二笔）

均**保留原判据意图**、只修正取值锚点，未删除任何断言：

| 用例 | 修法 |
|---|---|
| `shader_variants::pbr_diffuse_ibl_replaces_constant_ambient` | 新增辅助 `constant_ambient_is_cheap_fallback_only`：要求每处 `AMBIENT_FACTOR * radiance` 都排在 `if (mesh_cheap) {` 之后；无条件叠加仍判红，变体内无该子串也算通过（镜面变体走这条）。同时删掉同文件末尾那条已过时的重复断言（保留一条更严的）。 |
| `shader_variants::pbr_specular_ibl_samples_prefiltered_cube` | 同上辅助函数替换 `!contains` 断言。 |
| `water_absorption::tonemap_only_absorbs_inside_the_water_mask` | 改为「掩码之后 + `* optical_depth`」定位水面吸收式，并**反向**断言掩码之前不得出现该逐像素光程式（掩码前那处只允许 P3-R208 水下通道的界面公式）。比原来"全文第一次出现"更严。 |
| `hdr::tonemap_shader_adds_screen_space_water_reflection` | 把 P3-R207 时代的调用字面量更新为 P3-R241 的四参形状 `water_refraction(uv, params.camera_position.w, optical_depth, grazing)`，钉住完整调用（参数重排/丢项都会红）。 |

验证：`shader_variants::` 23 passed；`water_absorption::tonemap_only_absorbs_inside_the_water_mask` 1 passed；
`hdr::tonemap_shader_adds_screen_space_water_reflection` 1 passed；`cargo fmt --all -- --check` exit 0。

## 整跑复核（已完成，2026-11-16）

整跑 `cargo test -p rustlua-gpu --lib --all-features`（单线程）：

**308 passed / 0 failed / 0 ignored（2763.22 s）**，日志
`artifacts/logs/quality/m83-06-rustlua-gpu-lib-full.log`。6 条全转绿且无其它红 ⇒ 本任务可归档。

## 未完成项 / 已知限制

- 无剩余未收口项：6 条契约断言（2 条 CRLF 假红 + 4 条 shader 漂移真红）已在第二笔全部修复，
  整跑 308/308 绿（见「待复核」段，读数已回填）。（此条原先记的是"4 条真红未修"，为第二笔
  修复前的旧文字，保留修订痕迹以免误读；结论以本条与状态行为准。）
- 本机 autocrlf 检出的行尾问题**已根治（2026-11-16）**：`.gitattributes` 新增 `*.rs text eol=lf`，
  并把工作区 946 个干净 `.rs` 一次性归一为 LF（0 个 CRLF 残留；对方在途的 WIP 文件未触碰）。
  仓库 blob 本就是 LF，所以 `git add --renormalize -- '*.rs'` 无内容 diff，根治只花了一行规则。
  两条 `replace("\r\n","\n")` 仍然保留——对尚未重新检出的旧克隆是兜底，不改变判据。
  验证：`check-vendor-patches.ps1` 30/30 通过（说明 LF 归一化没有破坏字节敏感的 fork 断言）。
- `.gitattributes` 未加 `* text=auto`：`scripts/**/*.ps1` 必须是 UTF-8 BOM + CRLF（PS 5.1 口径），
  通配会一次性改写它们；因此只钉 `*.rs`。

## 归档记录（2026-11-20）

- **归档依据**：6 条契约断言全部修复且保留原判据意图（2 条 CRLF 假红用 `.gitattributes` 钉
  `*.rs eol=lf` 根治 + 断言侧 `replace` 兜底；4 条 shader 漂移只修正取值锚点、未删断言），
  整跑 `cargo test -p rustlua-gpu --lib --all-features` = **308 passed / 0 failed / 0 ignored**
  （日志 `artifacts/logs/quality/m83-06-rustlua-gpu-lib-full.log`），`cargo fmt --all -- --check` exit 0。
  M83-00 的验收清单亦记「M83-06 已完成（308/308 绿）」。
- **归档时仍未收口（只登记，不开新包）**：无。`crate` 的源码形状断言随 shader 演进仍可能漂移，
  这是契约测试的固有维护成本，非本任务残项。
- 归档动作：移入 `tasks/done/`，`tasks/README` 同步。正文旧文字（状态行"待整跑复核"、
  §未完成项"4 条真红未修"）已按整跑结果就地修订并留修订痕迹。
