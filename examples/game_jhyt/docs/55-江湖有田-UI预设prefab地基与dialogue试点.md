# 55-江湖有田-UI 预设（prefab）地基与 dialogue 试点（M59）

参考 Unity 的 Prefab 工作流：把界面**版面**从 Lua 里搬出来，做成运行期加载的数据文件，
调 UI 表现不再需要动玩法代码。本批只交"地基 + 一屏试点 + 对账 harness"，其余界面按批迁。

里程碑号取 M59：`docs/54` 的路线图把 M41–M58 预留给阶段 2–5 的内容里程碑（畜牧/钓鱼/矿脉
一直到 Beta 首发），UI 预设化不在那张表里，这里不跟它抢号。

---

## 1. 为什么要这一层（三条可查的事实）

改造前每个界面的版面与内容混在同一段 Lua 里。以 `world/dialogue.lua` 的 `M.render` 为例，
它一边算 `y + OPTION_DY + (i-1) * OPTION_ROW_H`、一边拼 `"> " .. text`、一边还要决定
哪一行画 8×8 标记。于是：

1. **改一个行距要先读懂整段绘制流程**，改错了只以"某行压住下一行"这种没有报错的形态坏；
2. **同一族控件在四个面板里各写一遍**（面板框 + 标题 + 角标 + 关闭 + 列表行），
   改一处样式要改四处；
3. **版面没有单一可信来源**：`ui/layout.lua` 只收得了矩形，收不了行距、字号、贴图键、
   图标尺寸——那些散在各文件顶部的 `local ROW_H = 40` 里
   （`inventory_ui.lua:70-74`、`shop.lua:52`、`quest_ui.lua:55-56`）。

## 2. 载体：JSON 数据文件，走已有的 `data.load` 通道

三条宿主事实决定了这个选择（不是口味）：

- `data.load` 是**现成的**运行期数据通道：VFS 读 → `pack::decode` → serde_json → Lua 表，
  带 schema 校验与缓存，三个宿主同源，不需要新引擎 API；
- `json` 在微信代码包后缀白名单内**且不会被编译**（`scripts/code-package-rules.js` 的
  `EXTENSION_WHITELIST` 与 `isCompiledName`：被编译的只有 `*.js` 和文件名恰为 `game.json`）；
  而 `*.lua` 在白名单外、要改名成 `.lua.txt` 才传得上去——数据文件用 `.json` 反而少一层特例；
- 命名成 `<name>.ui.json`：`assetc::is_dev_artifact` 只把**文件名恰为** `manifest.json` 的
  当开发产物裁掉，带名字的 `.ui.json` 照常进发行包。

实测（本批跑出来的）：

```
engine/target/release/assetc.exe package examples/game_jhyt -o $TMP/packchk
assetc_package_ok files=1036 compress_assets=false encrypt_code=false
manifest.txt:916  assets/ui/prefabs/dialogue.ui.json  4209  19a8436c8ae0046c
engine/target/release/assetc.exe list examples/game_jhyt   # → kind=scene status=ready
engine/target/release/assetc.exe --check-deps examples/game_jhyt  # → ok（无断链）
```

## 3. 预设长什么样

`assets/ui/prefabs/<name>.ui.json` 是一棵节点树。每个节点说清自己的类型、相对父节点的
锚点/偏移、样式与内容槽位；Lua 只喂内容取值：

```lua
prefab.render("dialogue", {
  speaker = 说话人名, body = 台词, hint = 提示串,
  options = { { text = "> 1. …", selected = true }, … },
  has_portrait = 有立绘, portrait_tex = 路径, portrait_w = 96, portrait_h = 96,
})
```

- **节点类型闭集（8 种）**：`rect`（纯布局容器）、`panel`、`circle`、`bar`、`image`、
  `text`、`list`（模板行 × 动态条数，对位 Unity 的"预制体套预制体"）、`instance`
  （引用另一个预设）。每一个都必须是现有引擎能力，预设层不开新的绘制原语。
- **几何口径与引擎同源**：`layout` 用的就是引擎 `UiRectLayout`（`rustlua-ui/src/ui_layout.rs`）
  的 `anchors/offsets/min_size/max_size/pivot`，解析式同一条 `edge = parent_size × anchor + offset`，
  锚点预设名沿用 `UiLayoutPreset::ALL`。同一份版面数据因此能被运行时与编辑器按同一语义读。
  12 个锚点值由门 ② 与引擎源码逐值对账（Lua 与 Rust 各写一份的地方只有这张表）。
- **数值规格**：常量 | `{slot=名, scale=k, add=d}` | `{value=v}`。动态尺寸靠它撑住——
  对话立绘的显示尺寸来自 `portrait.display_size()`，预设里写
  `{"slot":"portrait_h","scale":-1,"add":-6}`（立绘底边贴面板上沿、留 6 缝）。
  槽位没取值时**抛错**而不是当 0：静默当 0 的表现是"控件叠在父节点原点上"，
  正是要消掉的那类零报错坏形态。
- **单位 = 设计像素，不缩放**：`layout.scale()`/`font()` 目前没有任何界面在调
  （全仓只有定义处），预设解析出的矩形就是屏幕像素。真要加缩放，改一处
  （`prefab.lua` 的 `canvas_rect()`）而不是改八个界面。

引擎的 `data.schema` 只支持 array/object/map 与基础标量（`rustlua-ecs/src/data_schema.rs`），
表达不了"节点树 + 递归 children + type 决定哪些字段必填"。所以外壳交给 schema，
树内逐节点校验在 `prefab.validate()` 做，**报错指到节点路径**（`panel/options/item/marker`）。

## 4. 收口：retained UI 最容易漏的那一类坏

引擎的 `UiState` 是 retained 的（只有切场景才清空），"这一帧没画的那条记录"会一直留在屏上。
现状是每个界面手写一串 `ui.clear`/`ui_text.hide`（`dialogue.lua` 的 `clear_ui` 就是 14 行），
漏一条 = "上一段对话的选项还挂在屏上"。预设知道**整棵树的 id 全集**，所以
`prefab.render(..., {hidden=true})`（等价 `SetActive(false)`）和 `prefab.clear()` 都是
遍历出来的收口，不靠人记。

隐藏路径**不解析几何**：收口只需要 id 全集，若复用绘制路径就被迫要求调用方把动态尺寸槽位
喂齐（`portrait_h` 没给就抛错），而隐藏态根本没有东西要画。

## 5. dialogue 试点：画面一个像素都没变

`world/dialogue.lua` 删掉了 `BOX/PAD/OPTION_DY/OPTION_ROW_H/TEXT_*` 那批版面常量和整段
手工坐标计算，`M.render` 现在只做"取内容 → `prefab.render`"（文件顶部只留一句指向预设的注释）。
面板矩形仍指回 `ui/layout.lua`（预设声明 `layout_source = "dialogue"`）：迁移期那份矩形的
唯一定义处还在 `layout.PANEL`（`layout.audit()` 与 `TOUCH_KEEP_CLEAR` 都指它），
预设里另写了一份，两边必须相等由 `audit()` ② 守。

试点顺手修掉一条真缺陷：旧 `M.render` 只给选中行画标记，**从不收掉上一次选中的那条**，
所以 W/S 走一遍之后每行都留着一个箭头。这条在迁移前的代码上是红的，黄金表因此只取
"开场、selected=1"的状态，而"选中态只留一个标记"单独钉在门 ⑤。

## 6. 验收门 `jhyt_m59_ui_prefab`（已登记进 `verify.ps1` 的 MVP 清单）

```
cargo run -p game --example jhyt_m59_ui_prefab        # 全绿
cargo run -p game --example jhyt_m59_ui_prefab -- --capture   # 在迁移前的树上重生成黄金表
```

覆盖六件事：① 预设真的从文件读出来 + 结构校验（改盘上的 JSON 会改变解析结果）；
② 12 个锚点与引擎 `UiLayoutPreset::ALL` 逐值对账；③ `audit()` + `ui/prefab` 已登记进
`validate_data`（漏登记 = 这份 JSON 从没被任何检查读过）；④ **黄金对账**：逐条对话开场
节点、逐 id 读回引擎里真正画出来的矩形/文本/字号，与 `support/jhyt_m59_dialogue_golden.tsv`
逐行相等——黄金文件是在**迁移前的 HEAD 树**上用 `--capture` 采的，不是新实现自己吐的数；
⑤ 收口（关面板后每个 id "不存在或不可见"、选中态只留一个标记）；
⑥ 发行可见性（文件名不命中 `assetc` 的三条裁除规则）。

实测读数（2026-10-06，本机）：

```
golden_lines=102（6 条对话 × 17 个 id）  anchor_presets=12  prefab_root_children=7
prefab_file=dialogue.ui.json resource_kind=Scene
```

### 6.1 反面对照（这道门真的会红）

新门必须证明它咬得住，三条都在本机跑过（改完即复原）：

| 改了什么 | 门的读数 |
| --- | --- |
| 预设 `options.spacing` 28→30 | `黄金对账第 7 行不一致：期望 …264.00,578.00,18… 实得 …264.00,580.00,18…` |
| 删掉 `walk` 里"这一帧没画就收口"那支 | `stale:1,2`（第 3 项选中，1、2 项的标记还贴着） |
| `prefab.PRESETS.bottom_center` 的 b 改成 0.9 | `锚点预设 bottom_center 与引擎不一致：Lua=0.5,1,0.5,0.9 引擎=0.5,1,0.5,1` |

**买不到的部分**：黄金表只取"开场、`selected=1`"的状态——条件分支后的节点（`cond` 依赖
任务/变量/关系档位，`choose` 会改这些状态）不在表里，那部分仍由 m17/m19/m20/m34 的玩法门
覆盖；这张表证明的是"版面数值没漂"，不是"对话逻辑没漂"。另外它比的是**引擎记录**
（`ui.panel_get`/`ui.image_get`/`ui.text_get`），不是屏幕像素——像素级的跨宿主对账是另一套
（见 `docs/49` 与 web 侧 harness）。

## 7. 连带改动一：字号下限的数据侧（`jhyt_m30_cjk_text` ⑤）

M30 ⑤ 静态扫 `scripts/**` 里每个 `ui_text.draw*` 调用的第 5 个实参，必须解析得出数值且 ≥ 11px。
`ui/prefab.lua` 传的是 `node.font`，源码里没有字面量可解析——它是这条扫描的**唯一例外**，
例外由两路补平：

- 读回每个已登记预设的每条 `text` 节点比同一条下限（实测：`UI 预设 text 节点 8 个，最小 14px`；
  一个都没扫到就红，防"例外清单白列"）；
- 常驻负例：`validate()` 必须拒绝 10px 的 text 节点。删掉 `check_node` 里那条下限，
  这条门当场红（实测 `= "accepted", want "caught"`）——所以上面的例外不是洞。

其余读数不变：`Lua 绘制 58 处，最小 11px`、`场景 Text 21 个，最小 14px`。

## 8. 连带改动二：微信首屏清单（这条是真风险，不是打磨）

微信分包形态下 `lua_boot_with_reader(wechatReadFile)` 是**同步** `readText`，它不等分包
（`wechat-adapter.js` 注释写得很硬："首屏资源全在主包里，boot 必须立刻用它们把标题画出来"）。
而 `main.lua:306` 的 `validate_content()` 在 `on_create` 里跑启动门，`ui/prefab` 登记在门里
⇒ **启动期就要读那份 JSON**。文件若只躺在分包 `pack/` 里，首帧读不到 →
`validate_content` 当场 `error` → 白屏。这正是 `validate_data.lua` 里记着的那条历史教训
（"登记一个需要 `io` 的校验器会让两条 web 发布形态的启动当场失败"）的同族。

修法：把 `assets/ui/prefabs/` 加进 `scripts/first-screen-pack.js` 的
`DEFAULT_INCLUDE_PREFIXES`——"被启动门读的东西"按定义就是首屏资源。

实测（对同一份 pack 树重算清单，与已发布的 131 条比）：

```
node scripts/first-screen-pack.js $TMP/packchk --out $TMP/mainpack-new.json
first_screen_pack_ok entries=132 …
新增条目：assets/ui/prefabs/dialogue.ui.json、scripts/ui/prefab.lua
主包新增字节：4,209（预设本体，manifest.txt 那一行）+ 37,889（新脚本，脚本整目录本来就在首屏）
主包总量：1,711,336 B（微信主包线 4 MB）
```

**代价**：主包多带 42 KB。**买不到的部分**：本批没跑微信活体（那要重打包 + node harness，
跑的是 `scripts/verify-wechat-minigame.ps1` 的 `subpackage-never/delay` 两档）；
上面证的是"清单归属对了"，没证"真机首帧读到了"。下次出微信包时那两档就是这条的验收。

这条耦合在本批意外撞到过一次真实读数：重构 `prefab.lua` 时有一处漏改（调用已改名的函数），
三个门（`jhyt_m17` / `jhyt_m28_loop` 等）当场一起红在启动那一步——

```
main.lua:170: domain data validation failed:
[prefab] 校验执行失败: scripts/ui.prefab.lua:777: attempt to call a nil value (global 'checked_values')
  in upvalue 'validate_content'  in upvalue 'boot_game'  in function 'on_create'
```

也就是说"预设读不出来"在**任何宿主**都是整局起不来，不是某个面板少画一块。这是有意选择的
口径（宁可起不来，不要静默画歪），但把微信那条首屏要求变成了硬前提。

## 9. 下一批

- 其余七屏迁移：`hud`、`inventory_ui`、`shop`、`quest_ui`、`pause_menu`、
  `virtual_pad`（触屏三族控件）、`title_menu`。迁完才删 `layout.PANEL.dialogue` 那份副本，
  届时 `layout_source` 这条迁移期待遇一起删。
- 编辑器接 UI 预设（M59 后续批）。已知的第一颗钉子：`ResourceKind::from_path` 现在把
  `*.ui.json` 归为 **Scene**（门 ⑥ 就打印了这个读数），资产面板会把它当场景导入，
  必须先给它一条独立归类。
