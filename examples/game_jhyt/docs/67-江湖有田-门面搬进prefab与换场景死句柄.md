# 67-江湖有田-门面搬进 `.prefab`：载体契约、换场景的死句柄与三处连带口径（M63b/M63c）

66 号交的是"迁移之前先把画面钉死"（`jhyt_m63_title_prefab` + 68 行黄金表）。这一批才是搬家本身：
`scenes/main.scene` 的 17 个 UI 实体整批搬进 `assets/prefabs/title_menu.prefab`，进门面时由
`scripts/ui/title_menu.lua` 用 `prefab.spawn` 实例化进活世界，搬完拿**同一张表**对账。

搬家途中撞出一个引擎侧的真 bug（换场景把运行期实例的登记表留在原地），它单独记为 **M63c**，
因为修在 `engine/crates/rustlua-scripting/src/script/transition.rs`，不是样例数据。

---

## 1. 这批动了什么（文件级）

| 文件 | 动它干什么 |
| ---- | ---------- |
| `examples/game_jhyt/assets/prefabs/title_menu.prefab` | 新增。18 个实体（根 + 17 块）：夜色底图 / 暗角 / 墨带 / 木匾 / 左右灯笼 / 3 块木牌按钮 + 它们的 `Text` 标签 / 卷轴贴士 / 版本号；9,660 字节 |
| `examples/game_jhyt/scenes/main.scene` | 只剩场景头 + `meta ui_reference_*`，一个实体都不留 |
| `examples/game_jhyt/scripts/ui/title_menu.lua` | `preset_spawn()` 用 `prefab.spawn(PRESET_PATH)` 铺版面；`M.hide()` 撤实例；新增 `preset_alive()`（M63c） |
| `engine/crates/rustlua-scripting/src/script/transition.rs` | **M63c**：换场景时清 `prefab_spawns` 与 `prefab_ui_nodes` |
| `engine/crates/rustlua-scene/src/prefab.rs` | **M63d**：`entity` 行的段数与 `.scene` 对齐（根实体 3 段，不再写空 parent 段），读写两边一起改 + 单测多钉三件事（§2.3） |
| `examples/game_api/assets/prefabs/slime_{green,red}.prefab` | 根行去掉尾随制表符（各 1 字节，同一个口径） |
| `engine/crates/game/examples/jhyt_m61_prefab_runtime.rs` | ⑤ 重述成"只摘自己那一批"，新增 ⑥"换场景之后运行期实例必须清零" |
| `engine/crates/game/examples/jhyt_m30_cjk_text.rs` | ⑤ 的 Text 统计加读 `assets/prefabs/*.prefab` |
| `engine/crates/game/examples/jhyt_m29_season_weather.rs` | 引用语料加读 `assets/prefabs/*.prefab`，`EXPECTED_REFERENCED` 977→978 |
| `scripts/build/first-screen-pack.js` | 新增 5b：扫 `assets/prefabs/*.prefab`，把预设本体与它引用的贴图收进首屏包 |
| `examples/game_jhyt/docs/48-…资源接线与下架清单.md` | §1 口径补 `.prefab` 一个来源，重扫数 1026→1027 / 977→978 |

---

## 2. 预设作为载体的两条契约（实测，不是偏好）

### 2.1 行序就是绘制序

宿主按**实体槽位升序**提交 UI 分段（`world.for_each::<UiImage>` 走 `TypedStorage::order`），
而 `prefab.spawn` 顺序拿槽 ⇒ 预设文件里"谁写在前面"就是"谁画在下面"。所以 `.prefab` 的
`entity` 行必须保持 id 递增且与行序一致，换序 = 换画面。这条写在预设文件的头部注释里。
（行的**段数**口径见 §2.3。）

### 2.2 同一个世界里只 spawn 一次

`docs/66` §5 只钉了"每块矩形对不对"，没钉"谁盖谁"。搬完补了门 ④（`mesh.draws` 提交序），
实测同一个世界撤了重铺会**翻层**：

```
  ⚠ 同一世界撤了再铺，层序**翻了**（6 处不一致）：
  8: 期望 title_bg_pixel.png、重铺后 ui_menu_panel_scroll_9s.png
  9: 期望 ui_menu_vignette.png、重铺后 ui_menu_btn_wuxia_9s.png
  …
```

根因是 `spawn()` 优先复用回收槽（LIFO），第二批拿到的槽位顺序与文件行序脱钩。最坏的形状就是
那张 1280×720 无 alpha 的 `title_bg_pixel.png` 排到最后 ⇒ 整块门面被盖成一张纯背景。
脚本因此 `preset_spawn()` 幂等（`M.reset()` 不重铺），离开门面走 `M.hide()` 撤实例；
**回门面是换场景 = 新世界**，从干净的空闲表按行序拿槽，顺序与首次启动一致。

④ 本帧 15 个分段与迁移前逐段一致（读数见 §6），这条是**每次跑都打印**的账。

> **2026-10-12 更正（旧说法 → 新实测）**：这条账今天打印 **16 个分段**。上面那句"15 个分段"在
> 它写作的当天是对的——那时工程字档还是 `text_render_mode="vector"`；M72-03 的像素字
> （`assets/fonts/FusionPixel-12px-Monospaced-zhHans.ttf` + `sdf`）让字形经 `__font_atlas__`
> 这张图集**进 `mesh.draws`**，排在全部图块/色块之后（`frame_data.rs:878-902`：
> `push_ui_text`/`push_scene_ui_text` 是 UI 趟里最后两趟）⇒ ④ 从此连"文字整体盖在图块之上"一起钉。
> `vector` 档的字形走 `SceneMesh.vector_*`（不进 `draws`），所以那个字档下仍是 15 段。
> 复现（`cd engine`）：`cargo run -q -p game --example jhyt_m63_title_prefab` ⇒ exit 0、
> `文字分段：__font_atlas__ 分段 15..15（顶点 348..858）`、`层序与迁移前实测一致（16 个分段）`；
> 反面对照把工程 `text_render_mode` 换回 `vector` 后同一道门 ⇒ exit 1、
> `分段数 期望 16、实测 15`（本次用 `git archive HEAD` 的工程拷贝跑出，两侧同代码同表）。
> 期望序列为什么多出这一段写在门里 `ORDER_GOLDEN` 的注释。

### 2.3 根实体行的段数口径（尾随制表符提交不进去）

`.prefab` 的 `entity` 行今天与 `.scene` 同一条口径：**根实体 3 段、子实体 4 段**（parent 缺省
就不写那一段）。原先 `Prefab` 的解析器只收 4 段、写出时给根实体补一个空字段，于是根行以制表符
结尾——`git diff --check` 与 CI 的 'Diff check' 判它脏，这份数据在钩子下**提交不进去**
（`examples/game_api/assets/prefabs/` 那两份既有预设其实一直带着这个尾随制表符，本批一并去掉，
各 1 字节）。
改在 `crates/rustlua-scene/src/prefab.rs`：读、写两边必须一起改，只改一边会让
`crates/editor/examples/migrate_project.rs` 的"解析再序列化必须逐字节相等"那条幂等判定当场红。
守这条的是既有单测 `creates_instantiates_and_round_trips`，它现在多钉三件事——根行必须是 3 段、
整份文本不许有行尾制表符、`entity` 只有 2 段必须仍然拒（段数没被放宽成"随便几段"）。

---

## 3. M63c：换场景把运行期实例的登记表留在原地

这是搬家途中把 M20 撞红的东西，不是样例数据问题。

**复现**（修复前）：`jhyt_m20_gamepad` 走到 M20-09c，在标题界面点按钮离开时——

```
prefab.despawn 0: Lua API error: invalid entity handle: EntityId(0:g1)
```

`transition.rs:39` 的 `state.world = World::new()` 把 ECS 世界整个重建，清理清单里有
`ui_state` / `inventory` / `schema_defs`，**没有** `prefab_spawns` / `prefab_ui_nodes`。
于是两处后果：

1. 旧 handle 再 `prefab.despawn` 会拿死句柄去新世界删实体 ⇒ 直接抛错（就是 M20-09c）；
2. 更静默的一处：`preset_spawn()` 原来只判 `if not menu_handle`，句柄非 nil 但实例已经没了 ⇒
   回到门面**不再铺**，整屏 UI 消失，一行日志都没有。

**引擎侧修**：`clear_for_scene_transition` 那段之后补两行 `state.prefab_spawns.clear();`
`state.prefab_ui_nodes.clear();`。句柄计数器 `next_prefab_handle` **故意不复位**——复位会让
旧 handle 认到新实例，那是比报错更坏的错。

**脚本侧修**：`preset_alive()` 用 `pcall(prefab.root(menu_handle))` 探实例是否还属于当前世界，
`preset_spawn()` 因此能在"句柄还在、世界已换"时重铺，`M.hide()` 只在真活着时才 despawn。
两边是同一条口径的两半：引擎清表，脚本不假设表没清。

---

## 4. m61 ⑤ 为什么改写成"只摘自己那一批"

旧 ⑤ 断言 `prefab.despawn` 之后 `prefab_ui_rects` 为空。门面接了预设之后，场上**同时存在两批
实例**（门面的 18 块 + 门自己的夹具），"清空"这个断言就不再是真相——它红的是"别批还在"，
而别批还在恰恰是应该的。新 ⑤ 钉两格：本批矩形归零，别批矩形**逐值不动**。

---

## 5. 三处连带口径（搬家不只管搬家）

| 口径 | 不补会怎样 | 补了什么 | 实测 |
| ---- | ---------- | -------- | ---- |
| 首屏包 | 预设与它引用的两张贴图不在启动集合里 ⇒ 打包版进门面**白屏/缺图**，而首屏自检一路绿 | `first-screen-pack.js` 新增 5b：扫 `assets/prefabs/*.prefab`，预设本体进包、其 `texture=` 行喂进同一条闭包（行格式与 `.scene` 一致），未解析只 warn 不 throw | HEAD 版脚本 `entries=171 bytes=2133405` vs 现在 `entries=174 bytes=2161244 prefabs=1 prefab_textures=7 warnings=0` ⇒ 差的正是 3 个文件 / **+27,839 B**（预设 9,660 + 两张图 18,179） |
| `docs/48` 引用语料 | `title_bg_pixel.png` 从"已引用"掉成"内容缺口"（画面一个像素没变，纯统计变瞎） | 语料加 `assets/prefabs/*.prefab`；§1 表格行同步 | 同一份工程：关语料 `977 / 47 / 3`，开语料 `978 / 47 / 2`（总数 1027） |
| m30 字号统计 | `场景 Text 只有 14 个（<21）` 直接红，而字号一个都没改 | `scene_font_sizes` 一起读预设（同一套 `component\t…\tText\tfont_size=` 行） | `场景/预设 Text 21 个，最小 14px` |

`.prefab` 本身在 `code-package-rules.js` 的 `EXTENSION_WHITELIST` 里（不改名、打包版可读），
`assetc package` 实测 `files=1142`，manifest 行 `assets/prefabs/title_menu.prefab 9660 1f93f0348116f689`。

---

## 6. 验收与登记

```
cd engine
cargo run -q -p game --example jhyt_m63_title_prefab -- <game_jhyt-root>
cargo run -q -p game --example jhyt_m61_prefab_runtime -- <game_jhyt-root>
cargo run -q -p game --example jhyt_m20_gamepad       -- <game_jhyt-root>
```

m63 的关键读数（迁移后重跑，表未重导）：

```
golden_lines=68
  ② 三行标签 实体=(580/592/592) 动态=(578/590/590) 差 2.00px ⇒ 今天由动态层画
  ③ 实体层 14 块、动态层 13 块、未画 4 块
  ④ 本帧提交 15 个分段 … 层序与迁移前实测一致（15 个分段）
```

（这段是当天那份捕获，原样保留；工程换像素字档后 ④ 打印 16 个分段，多出来的那一段是字形图集，
判据与复现见 §2.2 末尾那条 2026-10-12 更正。）

`docs/66` §4 留给 M63b 的两个决定，这里的落法是**照抄迁移前场景那一份**（`580/592/592` + 米色
`0.949,0.918,0.847`）：那三行 `Text` 在迁移前后都被 `ui.scene_text_visible` 逐名藏掉、字仍由脚本
每帧画，所以照抄不构成改画面。§4 说的"抄画面那一份"前提是**改由实体层画字**，这批没做那个前提，
因此不动它——要动的时候得连 `shown` 的文字读数一起重导，不该藏在一次"搬家"提交里。

verify.ps1 的 MVP 清单这批补登记了 `jhyt_m62_prefab_write` 与 `jhyt_m63_title_prefab`：
92a605e（M62）与 db85f65（M63a）各登记过一次，HEAD 3a78336 提交时两条一起没了（那次按
"HEAD 内容 + 本批增量"重建 verify.ps1，没带上这两行）。现在这两道 prefab 门在 MVP 里会跑。

---

## 7. 反面对照（每条都跑过，跑完全部回退）

| # | 注入 | 红在哪 |
| - | ---- | ------ |
| A | `menu_before` 里抹掉一个 `Logo`（模拟"despawn 顺手摘了别批"） | m61 ⑤：`despawn 之后别批的矩形数量从 17 变成 18` |
| B | `Logo.rect.x += 1.0`（模拟别批矩形被改动） | m61 ⑤：`第 0 项（0=x…）= 384，despawn 之前是 385` |
| C | 注释掉 `prefab_spawns.clear()` | m61 ⑥ 的 despawn 报 `invalid entity handle: EntityId(21:g2)` —— 与 M20-09c 同一形状 |
| D | 只注释 `prefab_ui_nodes.clear()` | **绿**（见 §8） |
| E | 关掉 m29 的预设语料 | ⑦：`已引用 977 个，与 docs/48 §1 的 978 不符`，且 `title_bg_pixel.png` 变回内容缺口 |
| F | m30 只数场景 | ⑤：`场景与预设的 Text 组件只有 14 个（≥21）` |
| G | 用 HEAD 版 `first-screen-pack.js` 跑同一个包根 | `entries=171 bytes=2133405`，少的三个文件正是预设 + 两张图 |

---

## 8. 代价与买不到的部分

- **`prefab_ui_nodes` 的清理没有可观察的红**（对照 D 是绿的）。预设重铺时新批次恰好拿回同一批
  槽位、把陈旧节点整顶覆盖，这条通道在本场景下量不出差别。两行 clear 是**按约定**清的
  （槽位号索引的表跟着世界重建是同一口径），但门不声称守着它。
- **④ 看不见文字与文字之间谁盖谁**（本条按 2026-10-12 的字档收窄过：写作时它连"文字整体"都看不见，
  因为那条读数通道那一帧不出文字分段、门里打印"文字分段：无"；工程换 `sdf` + 像素字后字形经
  `__font_atlas__` 进了 `draws`，"文字盖在全部图块之上"从此在账上）。仍然买不到的是**同一段内部**
  的顺序：全部字形合并成一个分段（实测顶点 348..858），两行字谁压谁不在这张表里。
- **首屏包对未解析贴图只 warn 不 throw**：预设写错路径时不会拦下构建，只在 warnings 里露头；
  throw 留给真正会误伤开发产物的那一类。
- **web / 微信宿主没跑**：首屏清单这批补了 5b（§5），`assetc package` 也过了，但
  `prefab.spawn` 在 wasm 宿主上的活体对拍（`scripts/verify/verify-webgl-bundle.ps1`）没跑、包没重打，
  `.prefab` 在微信代码包白名单里的行为仍未测——`docs/64` §7 那两条开放项**继续有效**，
  本批不声称三宿主都验过。
- 这批所有绿都是在 `.cache/m20_proj` 这份工程拷贝上量的：工作区里
  `assets/ui/prefabs/dialogue.ui.json` 正被在途改动删除，而 `UiLayoutPreset::ALL` 还登记着它，
  真工程在 `validate_data` 就 throw（6 个门一起红在同一行，与本次搬家无关）。拷贝 = 工作区 +
  从 HEAD 恢复那一份 `.ui.json` + 同步 `docs/`，其余文件与真工程一致。dialogue 接 `.prefab`
  与 JSON 预设层退役是另一批（任务 #15）。
