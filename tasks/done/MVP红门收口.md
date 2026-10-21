# MVP 红门收口（jhyt 示例逐门转绿）

> 状态：**已收口（2026-10-21）；已归档（2026-10-21）**
> 起于：2026-10-20，`verify.ps1` 的 `$examples` 清单补齐之后做的一次全量重扫（80 个示例，
> 65 绿 / 14 红 / 1 退役红）。原则是**先分清红因再动手**：同一个红可能是判据过期、被测物
> 没了、或者真回归，三条的修法完全不同。第一批 11 门于 2026-10-20 收口（`365dc1a8`），
> 本文件记录最后 3 门的收口与全批归档。

## 1. 已收口（14 门）

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
| `jhyt_m31_pc_release` | 验收的是**打好的**发布包，清单里只会恒红 | 移入 `$releaseExamples`：有产物真跑，没有则 `[SKIP] …（不是通过）`；本批重打 `builds/pc` 后又补了两处 grouped 口径（见 §2.4） |
| `jhyt_m63_title_prefab` | 黄金表还是竖版旧门面；新门面横排并撤装饰；预设三行标签仍是旧竖排坐标（② 差 182/42/178px）；`FooterText` 空 | 重定基线 68 行；标签改成动态层同一坐标（② 差 0/0/0）；页脚恢复为 `v0.1.1`；`hint` 确认不要；④ 期望序列 16 → 10 段 |
| `jhyt_m72_pixel_font` | ② 要求真帧至少一个 1:1 档，标题门面全是 13/14/18/22/24px 非整数倍字号 | 不动字号（归 M72-10），在首次 `vertices()` 前加 12px 探针；真帧 75 个四边形 / 1:1 档 9 个 / 灰阶 0 |
| `jhyt_m17_shop` | 商店 UI 已迁进 `shop.prefab`，旧断言还在读动态层 `ui.get('shp:msg')` → nil 索引 | 改读预设 `ShopMsg` 的 `Text` 组件，断言"购买成功"；4a–4i 九段全过 |

## 2. 最后三门的收口细节（2026-10-21）

### 2.1 `jhyt_m63_title_prefab`

`873f05f3` 把门面从竖版三键改成横版三键，黄金表与新门面差 28 行。逐行核对后分三类：

- **26 行是设计变更**：三键横排（`380/420`、`560/420`、`740/420`，160×60）、logo/lantern 重排、
  四角/竹分隔/印章撤掉、装饰图不再提交。这些重导黄金表即签收。
- **1 行是真正的回归，不在原 diff 清单里**：预设的三行标签 `UiLayout` 还停在竖版坐标
  （`BtnStartLabel 580,397`、`BtnContinueLabel 592,477`、`BtnExitLabel 592,557`），而动态层
  按 `measure_text` 真实字宽居中在 `398/590/770, y=435`。② 实测差 **182 / 42 / 178px**——
  远超 4px 阈值。这不是"黄金表过期"，是预设与脚本各存了一份版面数据、改横排时只改了脚本。
  把三行 `Text` 的 `offsets` 改成动态坐标后，② 差 **0.00 / 0.00 / 0.00px**。
- **2 行是内容决定**：
  - `FooterText` 的 `text=` 是空的。恢复为 `江湖有田 v0.1.1 · RustLua 原型 · 横版`：
    版本号跟当前 `project.toml`（`0.1.1`）对齐，旧表里的 `v0.1.0` 本身已过期；
    颜色回到旧门面的米灰 `0.659,0.624,0.541`（新预设的 `0.25,0.20,0.12` 在夜空底上不可读）。
    `tools/gen_main_menu_scene.py` 的 `FOOTER_TEXT` 同步到 `v0.1.1`。
  - `hint` 被隐藏。**确认不要**，不是丢失：`jhyt_m20_gamepad` §M20-09c 明写"旧操作说明
    不应再显示"，`title_menu.lua` 每帧 `ui.hide("title:hint")`、绘制那行保持注释；
    门面只留牌匾 + 三块木牌。黄金表里记 `shown\thint\ttext\thidden`，不删探针。

黄金表重导后 68 行（shown 31 / authored 37），④ 的期望序列从竖版 16 段降到 10 段：
背景两张、水墨带、logo、灯笼、木牌、卷轴、白图、hover、`__font_atlas__`。少掉的 6 段正是
撤掉四角/竹分隔/印章，不是层序坏了。`--capture` 改成打印完 68 行就 `return`，与
`examples/game_jhyt/docs/66` §3 写的命令语义一致（此前它会继续跑 ②，实测一直红在标签差）。

**没动、但记一笔**：logo 矩形是 `350,180,580×180`，贴图 `ui_title_logo.png` 是 512×256，
非等比拉伸。这是新门面的设计取值，门只钉"画在这块矩形里"，不判断这张图该不该重画；
若美术线要 1:1，另开美术任务，不在本门改。

### 2.2 `jhyt_m72_pixel_font`

原红点：② 要求"真帧里至少一个四边形落在 1:1 档"，而标题门面全是 13/14/18/22/24px
（`tex_scale = size/12` 非整数），1:1 档 0 个。字号口径归 M72-10，不在本门改字号；
本门补一个 **12px 探针**（`ui.text_at('m72:one_to_one', 1000, 700, '开始新游戏', 1.0, 12)`），
让真帧仍有 1:1 覆盖，断言不降级。

这里踩到一个**时绿时红的坑**：探针放在 `frame()` 之后、靠"探针触发图集版本变化"来取首帧，
在 `RAYON_NUM_THREADS=2` 下会红在"首帧的字体图集 pixels 为空"——图集预热集合随线程数变化，
不能把"首帧"押在一次增量上。最终改成：

1. 在**第一次 `vertices()` 之前**画探针；
2. `first = vertices()` 一定是首帧整张图集上传（含探针字形），② 的直方图、1:1 计数、
   `sdf=2` 检查全读 `first`；
3. `frame()` 步进一帧，`settle = vertices()` 消费更新带来的图集变化；
4. `scene = vertices()` 才是稳态，断言 `font_atlas.pixels` 为空（增量协议）。

读数：`75 个 UI 字形四边形：亮 texel=2807 全暗=4503 灰阶=0 alpha!=255=0；1:1 档 9 个，
非整数倍字号 66 个`。M72-10（把门面字号统一到 12px 整数倍）仍是独立任务，不在本门假装解决。

### 2.3 `jhyt_m17_shop`

失败语句是 `assert_str(&mut runtime, "ui.get('shp:msg').text", "购买成功")`，
`ui.get('shp:msg')` 返回 nil。根因：M73 把商店 UI 迁进 `assets/prefabs/ui/shop.prefab` 后，
反馈文案由 `systems/shop.lua` 的 `prefab.set_text(handle, "ShopMsg", ...)` 写进预设节点的
`Text` 组件，动态层 id `shp:msg` 已不存在；旧断言没跟着迁移。

改法：新增 `prefab_text(runtime, name)`：`entity.find(name)` 拿实体 id（预设节点与场景实体
共用同一张名字表），`with_world` 读 `UiText.text`，断言 `ShopMsg == "购买成功"`。
这条仍验"玩家看到的反馈文案"，不是改读内部状态。4a–4i 九段全过，读数
`jhyt_m17_shop_ok … shops=3 money=925 seed_lingzhi=13 rice=2 energy=30`。
临时加的 `[trace]` 全部删除。

### 2.4 `jhyt_m31_pc_release` 的 grouped 口径（重打产物后暴露）

全量跑到 release 段时，`builds/pc` 是 2026-10-20 的旧包，先红在 `pack/chars.txt` 缺失。
重打后连红两处，都是**发布门对 grouped 容器（docs/67，PC 默认）的假设过期**：

- `chars.txt` 被 assetc 收进 `core` 容器，磁盘上没有这个文件；但
  `runtime_scene/fonts.rs` 按 `<pack>/chars.txt` 的真实路径读它，发布门 ① 也点名要它。
  在 `package.ps1` 的容器分支补 `--keep chars.txt`（几 KB），与字体同一条不变量：
  boot 期按真实路径读的东西不进容器。
- `scripts/main.lua` / `ui_text.lua` / `text_fit.lua` 也在 `core` 容器里，①/⑥ 却要求它们是
  真实文件。`scripts/main.lua` **不能** keep——assetc 明确拒绝"启动入口不在 `unit=main`
  组里"（Web 上 require 同步，放进目录会启动即失败）。所以改门：脚本按
  `bundle_plan.json` 的逻辑路径验，`project.toml` / `manifest.txt` / `chars.txt` / 字体
  仍必须是真实文件；⑥ 的文本管线清单同样接受 `bundle_plan.json`。

重打后的读数：`release_entries=11 stray=0`；②③④⑤ 不变；⑥
`manifest_ok chars=1612 cjk=1473`；`jhyt_m31_pc_release_ok`。

## 3. 顺带的坑

- 编辑工具改写 `.ps1` 会丢 UTF-8 BOM，PowerShell 5.1 按 ANSI 解码中文即报假语法错。
  第一批补回了 `verify.ps1` 与并发会话刚改过的 `apply-bundle-layout.ps1` / `package.ps1` /
  `verify-wechat-minigame.ps1`。
- `--capture` 只打印、不早退的写法会让"捕获"也红在后续断言；m63 这次一并修成早退。
- 示例里"先 `frame()` 再取首帧"不能假设图集一定有变化：线程数会改变预热集合，
  首帧必须取**第一次** `vertices()`。
- `cargo clippy -p game --examples` 的存量 `needless_borrow`（`press(runtime, &host, …)` 这类）
  与本批无关；本批只保证改过的示例 `cargo clippy -p game --example <name>` 无新增命中。

## 4. 验证

逐门：

```
cd engine
cargo run -q -p game --example jhyt_m63_title_prefab            # ok golden_lines=68；② 0/0/0；④ 10 段
cargo run -q -p game --example jhyt_m63_title_prefab -- --capture # 68 行，与入库黄金表逐字节相同
cargo run -q -p game --example jhyt_m72_pixel_font             # ok；1:1 档 9 个
cargo run -q -p game --example jhyt_m17_shop                   # ok；4a–4i
cargo run -q -p game --example jhyt_m20_gamepad                # ok；09c 仍断言 title:hint 不显示
cargo run -q -p game --example jhyt_m31_pc_release             # ok；重打 builds/pc 后 ①–⑥ 全过
```

全量：

```
powershell -ExecutionPolicy Bypass -File scripts\verify\verify.ps1 -MvpOnly
→ MVP headless regression: executed=78 expected=78
→ jhyt_m31_pc_release_ok / Clean release directory self test 通过
→ MVP gate passed（2026-10-21 重跑）
```

第一批的配套门（引擎/音频/脚本文档等）在 `365dc1a8` 已全绿。本次改了三个目标示例，
外加 `jhyt_m30_cjk_text` 的注释修正、`jhyt_m31_pc_release` 的 grouped 口径与
`package.ps1` 的 `chars.txt --keep`；没有动引擎与脚本 API。

## 5. 没做的（登记，不另开包）

- **M72-10 字号口径**：门面仍是 13/14/18/22/24px，12px 探针只是保住"1:1 覆盖"这条断言，
  不等于字号已统一。
- **logo 非等比**：见 §2.1 末段，设计取值，门不判。
- **`cargo clippy -p game --examples` 存量红**：见 §3，不属本批。
- **Web/微信发布包未重打**：本批只重打了 `builds/pc`；`builds/webgl` / `builds/wechat-minigame`
  未重打，跨平台产物仍可能是旧门面。
- **三宿主目验**：本批只跑 headless 门，编辑器 Play / 桌面 / Web 的标题画面没有重新目验。

## 6. 归档记录

- 归档日期：2026-10-21。
- 归档口径：**随批归档**（不是"验收通过"）。14 门已转绿，但 §5 的五条仍开着。
- 本批提交：`c913fc7f`（归档主提交；本行哈希由紧随其后的回填提交补上）。
- 主要改动文件：
  - `engine/crates/game/examples/jhyt_m63_title_prefab.rs`
  - `engine/crates/game/examples/support/jhyt_m63_title_menu_golden.tsv`（重导，68 行）
  - `engine/crates/game/examples/jhyt_m72_pixel_font.rs`
  - `engine/crates/game/examples/jhyt_m17_shop.rs`
  - `engine/crates/game/examples/jhyt_m31_pc_release.rs`（grouped 口径）
  - `engine/crates/game/examples/jhyt_m30_cjk_text.rs`（hint 注释修正）
  - `scripts/build/package.ps1`（`chars.txt --keep`）
  - `examples/game_jhyt/assets/prefabs/ui/title_menu.prefab`（三行标签坐标 + 页脚）
  - `examples/game_jhyt/tools/gen_main_menu_scene.py`（页脚 `v0.1.1`）
  - `examples/game_jhyt/docs/66-江湖有田-门面画面的黄金对账门.md`（新门面读数）
- 回滚档：`git checkout <commit>^ -- <文件>`。黄金表与 `include_str!` 一起回滚；
  只回滚 `.rs` 不回滚 `.tsv` 会红在 ①。
