# MVP 红门收口（jhyt 示例逐门转绿）

> 状态：进行中（第一批已合，剩 3 门待决定）
> 起于：2026-10-20，`verify.ps1` 的 `$examples` 清单补齐之后做的一次全量重扫（80 个示例，
> 65 绿 / 14 红 / 1 退役红）。原则是**先分清红因再动手**：同一个红可能是判据过期、被测物
> 没了、或者真回归，三条的修法完全不同。

## 已收口（11 门）

| 门 | 红因 | 处置 |
| --- | --- | --- |
| `jhyt_m23_data_validation` | `ui/prefab.lua` 的 `validate_all` 遍历 M71 清空的 `NAMES`，永远返回绿 | 删掉入口（不是登记进 `validate_data.lua`），写明重启前提是"校验器能真的失败" |
| `jhyt_m30_cjk_text` | 预设搬进 `assets/prefabs/ui/` 后多一层，平铺扫描漏掉 8 份 / 154 个 `Text` | 递归收集（168 个）；`MIN_REGISTERED_TEXT_SITES` 按逐提交复算降到 27 |
| `jhyt_m66_pause_prefab` | 黄金表音量读数还是 100%，`project.toml [audio]` 是 50% / 25% | `--capture` 回写，diff 只有 5 行 |
| `jhyt_m24_numeric_alignment` | §3 打坐单元格改写成「恢复5点内力，钳经脉动态上限」，旧断言逐字比对带空格的旧文案 | 语义比对 + 新增"代码侧真的用 `meridian.max_inner_power()` 钳制"断言 |
| `jhyt_m34_status` | §10 「治疗」改名「体力恢复」，归属表没有该行 | 补归属（40 体力出处 + 经脉上限出处） |
| `jhyt_m22_save_isolation` | `PLOT_CELL="19,4"` 与 `PLOT_X/Y=16,4` 各说各话，断言读的是隔壁空地 | 键做唯一真源；失败时打印地块账本 + FarmGrid 实际非零格 |
| `jhyt_m61_prefab_runtime` | 门面 `BtnStart` 命中点写死 `(640,412)`，新门面按钮在 160×60 @ (380,420) | 从 `title_menu.prefab` 的 `UiLayout.offsets` 现算中心 |
| `jhyt_m62_prefab_write` | 同上 | 同上 |
| `jhyt_m72_tile_variant_determinism` | ⑥ 读已删除的 `plugins/webgl/web-runtime.js` | 整段退役（不是跳过），保留确定性断言 |
| `jhyt_m72_autotile_mask` | ⑧ 同上 | 同上 |
| `jhyt_m31_pc_release` | 验收的是**打好的**发布包，清单里只会恒红 | 移入 `$releaseExamples`：有产物真跑，没有则 `[SKIP] …（不是通过）` |

## 待决定（3 门）

1. **`jhyt_m63_title_prefab`**：黄金表与新门面（`873f05f3`）差 28 行。26 行是设计变更，
   另外 2 行是**内容丢失**：版本页脚 `FooterText` 的 `text=` 为空（全工程再无 `v0.1.0`）、
   键位提示 `hint` 被隐藏。要决定"恢复文案"还是"确认不要"，再回写黄金表——否则等于把
   丢失当设计签收。
2. **`jhyt_m72_pixel_font`**：图集断言已修好（自己先取首帧，另加"稳态帧不得再带整张图集"），
   红点后移到 §② 的"至少一个四边形落在 1:1 档"，而当前门面全是 13/14/18/22/24px 这类
   非整数倍字号。归 M72-10（字号口径），不是点阵管线坏了。
3. **`jhyt_m17_shop`**：`<console>` 里 `[C]: in metamethod 'index'` 的 nil 索引，已定位到
   `M17-04d 商店 UI` 那一段，未定位具体语句；该文件与 `input`/`audio` 迁出内置由另一会话
   在改，本批没碰。

## 顺带的坑

- 编辑工具改写 `.ps1` 会丢 UTF-8 BOM，PowerShell 5.1 按 ANSI 解码中文即报假语法错。
  本批补回了 `verify.ps1` 与并发会话刚改过的 `apply-bundle-layout.ps1` / `package.ps1` /
  `verify-wechat-minigame.ps1`。
- `cargo clippy -p game --examples` 当前仍红在别处的 `needless_borrow`（`press(runtime, &host, …)`
  这类，`new_runtime` 签名改动后的残留），与这批改动无关；若同一会话继续动示例，顺手清掉。
