# 64-江湖有田-UI 预设的运行时桥：`.prefab` 进活世界（M61）

上一批（[55-江湖有田-UI 预设（prefab）地基与 dialogue 试点](55-江湖有田-UI预设prefab地基与dialogue试点.md)，M59）
交的是**数据文件层**：版面从 Lua 搬进 `assets/ui/prefabs/<name>.ui.json`，运行时用现成的
`data.load` 读出来，由 Lua 自己画。这一批交的是**引擎层**：预设改成 `.prefab`（引擎已有的
实体树资产格式，编辑器双击就能在 Scene 视图里预览与修改），并且让脚本能在运行期把它
**实例化进活世界**——画得出、点得中、改得动、收得干净。

里程碑号取 M61：M59 之后阶段 2 的内容门占到了 M60（`jhyt_m60_tutorial`），这批往后取一号。

载体为什么从 `*.ui.json` 换成 `.prefab` 不是口味，是三条硬事实（本批实测，逐条见 §2）：
JSON 那份**只能用 Lua 自己画**（`ui.*` 动态层），因此它进不了 Scene 视图、不吃引擎的
九宫格/锚点解算、也没有点击派发；而 `.prefab` 与场景用的是同一套组件字符串，实例化走的是
与场景加载**同一个**映射点。`*.ui.json` 那份与它的 JSON 解析层会在余下界面迁完后退役（见 §8）。

---

## 1. 改造前卡住的四件事

| # | 症状 | 根因（改前） |
| - | ---- | ------------ |
| 1 | 脚本 spawn 出来的 UI 一个像素都不多：实体活着、句柄有效、屏上无痕 | `RuntimeScene::ensure_scene_ui_layout` 只吃 `ui_scene_nodes`，而这张表**只在场景加载时**从场景文档建一次（`runtime_scene/mod.rs`） |
| 2 | 同一份数据"摆在场景里会画、spawn 出来不画" | "哪些实体参与布局、`preferred_size` 怎么算"在场景侧与运行期各写一遍，两处真源 |
| 3 | 预设里的标签在运行期一个字都改不了 | 引擎没有写**场景** `Text` 组件的脚本接口；`title_menu.lua` 只能 `ui.scene_text_visible(name,false)` 把场景标签全数隐藏、再由脚本用 `ui.*` 盖一层（这个现状就是缺口的形状） |
| 4 | 动态 UI 的命中框与绘制框可以各算各的 | `compute_click_targets` 从组件现算矩形，`push_scene_ui` 从 `ui_layout_rects` 画——spawn 的东西两边都没登记 |

## 2. 这一层的形状：一条链，两个同源

```
.prefab 文本（VFS 读，三宿主同路）
  → rustlua_scene::prefab::Prefab::from_text（末尾 validate：版本行 / root 存在 / parent 边成树）
  → LuaRuntime::instantiate_prefab_state     ← 与 instantiate_scene 共用 spawn_entity_components
      · 活世界插实体（组件字符串解析口径不可能分叉）
      · 第二趟落 Parent 组件（子可能先于父 spawn）
      · 名字进 entity_state.names（⇒ entity.find 同源）
      · build_ui_scene_nodes(...) 建布局节点，key = 落进世界的**槽位号**
  → ScriptState.prefab_ui_nodes / prefab_spawns
  → RuntimeScene::sync_prefab_ui_nodes（每帧解算之前并进 ui_scene_nodes）
  → resolve_scene_ui_layout → ui_layout_rects
      ├─ push_scene_ui 画 quad
      └─ compute_click_targets 算命中框 → process_ui_clicks 派发到**全局** Lua 函数
```

两个"同源"是这批的全部要点：**建节点**只有一处定义
（`rustlua-ui::ui_scene_layout::build_ui_scene_nodes`，`UI_LAYOUT_COMPONENTS` 那张 6 元素表也
只剩一份），**读矩形**只有一张表（并进 `ui_scene_nodes` 之后，绘制与命中是同一次解算的产物）。
`UiSceneNode` 因此新增 `PartialEq`（derive 链上 `UiRectLayout`/`ContainerStyle` 本来就可比较）。

`sync_prefab_ui_nodes` 的空表短路是刻意的：spawn 集没变时只有一次 `PartialEq` 比较、零分配，
所以现在所有游戏（0 个 spawn 实例）不为这条链付出代价。槽位被复用（场景 UI 实体先
`entity.destroy`、随后 spawn 落进同一个槽）时 spawn 的节点覆盖旧的——旧节点本来就是死实体的。

`PrefabSpawn` 刻意**不**进 `entity_state.scene_entities`：那张表随场景加载清空并参与切图重生，
混进去会让切图把 UI 实例当场景实体再重生一遍。despawn 按句柄精确摘除，同名实体也不会误删
场景那份。

## 3. 新增的脚本 API：`prefab` 表（5 个函数）

| 函数 | 返回 | 失败（都是 Lua 错误，不返回 nil） |
| ---- | ---- | -------------------------------- |
| `prefab.spawn(path)` | `i64` handle | 工程未加载 / 文件读不到 / 非 UTF-8 / 预设不合法（点名坏的那条 `parent` 边） |
| `prefab.despawn(handle)` | `true` 已拆除；`false` 句柄未知或已拆过（幂等） | 活世界写入失败 |
| `prefab.root(handle)` | 根实体 id | `没有 handle <n>` |
| `prefab.node(handle, name)` | 实例内节点实体 id | `没有 handle <n>` / `实例 <n> 里没有节点 "<name>"` |
| `prefab.set_text(handle, name, text)` | 无 | `没有 handle <n>` / 无该节点 / `节点 "<name>" 没有 Text 组件（预设里它是图片/面板/容器）` |

实体 id 的口径是 `index << 32 | generation`，与 `entity.*`、组件读写同源。踩到过的一条坑值得记：
`entity.find(name)` 查的是**名字表**（spawn 会写它），`entity.find_id(persist_id)` 查的是
**persist 表**（另一套登记），拿 `find_id` 去接 `prefab.root` 必然对不上——门里 §4 的 ① 段
就是这条的断言位。

`set_text` 只改文字，字体/字号/颜色归版面，要改就改预设文件。这条边界是"版面进预设、取值
留脚本"的可执行版本。

API 面账（本批实测）：

```
node scripts/gen-lua-api-docs.js --check
lua_api_docs mode=check tables=42 funcs=281 stubs=275 documented=6
lua_api_docs_ok 再生成是 no-op、桩数未超标、无死绑定/重复注册
```

42 张表 / 281 个函数（原 41 / 276）。`prefab.md` 五个函数的说明是**写实**的而不是文档桩，
所以待补桩仍是 275——没有因为新增 API 把桩数推高，`baseline.json` 里动的只有 `funcs` 与
`documented`。附带一处账目更正：`game_web/src/lua_bridge.rs` 内嵌基线的注释原本写"41 张表 /
275 个函数"，已跟到 42 / 281。

## 4. 验收门 `jhyt_m61_prefab_runtime`（已登记进 `verify.ps1` 的 MVP 清单）

无头跑真宿主（`engine::host::RuntimeScene` + `InputSnapshot`），fixture 写在
`examples/game_jhyt/.data/`（gitignore 的开发目录，门的末尾会删掉）：4 个实体的
`.prefab`（`M61Root` 全屏 `UiLayout` / `M61Btn` 九宫格 `Image` + `UiEvents` + `UiLayout` /
`M61Label` `Text` + `UiLayout` / `M61Bare` **只有** `Image`），加一份坏预设（子节点
`parent=99`）。

```
cd engine && cargo run -q -p game --example jhyt_m61_prefab_runtime
```

实测读数（1280×720，逐条是引擎里真正算出来的矩形，不是预设文件的复读）：

```
门面场景 = "main"
⓪ scene click((640.0, 412.0)) → ["BtnStart"]
① handle = 0
   prefab.spawn('.data/jhyt_m61_broken.prefab') → 报错点名 "jhyt_m61_broken.prefab"
   prefab.spawn('.data/jhyt_m61_nope.prefab')   → 报错点名 "jhyt_m61_nope"
② rects = M61Root=(0,0,1280,720) M61Btn=(60,600,200,60) M61Label=(180,617,80,30)
   （M61Bare 无 UiLayout ⇒ 不参与布局解算，故不在读数里）
③ 内 → ["M61Btn"]，外 → []（一次点击只派发一个回调）
④ 写入生效，三条误用各自点名
⑤ 撤干净；场景 UI 命中框仍派发 ["BtnStart"]
```

② 的 `M61Btn=(60,600,200,60)` 是父链解算后的值（预设里写的是 `offsets=60.00,600.00,260.00,660.00`
即右下角，减去全屏父框后宽高为 200×60），③ 的命中框就是同一个矩形——**绘制框与命中框同源**
这条不是设计承诺，是这一行读数。

⑤ 的口径要说清：despawn 之后不仅断言"矩形读数变空、`entity.is_alive` 变 false、重复 despawn 返回
false"，还要断言**派发列表为空**，然后再点一次门面场景自己的按钮，确认它仍然派发 `BtnStart`——
即动态层的进出不会污染场景那一份。为此门在 ⓪ 就先把全局 `on_click` 换成记录器（`main.scene`
的按钮绑的正是它，让它跑真身会当场开一局新游戏、重建世界，之后所有读数读的是下一屏），跑完换回。

反面对照（新门必跑一次，证明它会红）：把 `ensure_scene_ui_layout` 开头的
`self.sync_prefab_ui_nodes();` 注释掉再跑，门在 ② 就断：

```
M61Root 没有解析出矩形（spawn 的 UI 不会被画）；本帧读数 = <空>
```

## 5. 门自己绊倒过两次，这两条已写进门注释

1. **负例探针不能只数"恰好一个"**：`click()` 第一版对每次点击都断言"派发数 == 1"，而
   `矩形外` 与 `despawn 之后` 两路探针期望的正是**空**，于是门红在 ③"一次点击派发了 0 个回调"，
   顺着这条去查宿主命中框、给 `process_ui_clicks` 插打印、再加 `debug_click_targets`——查出来的
   结论是派发一直是对的。现在 `click()` 返回**完整名单**（含空），断言在调用方按期望分支。
   临时诊断（宿主方法 + `eprintln!`）已全部回退，`git diff` 里只剩本批预期的改动。
2. **Lua 的 `..` 优先级高于 `~=`**：`tostring(ok) .. '|' .. x ~= nil` 解析成
   `(整串拼接) ~= nil`，于是"报错内容应当点名坏文件"这条负例读成一个光秃秃的 `true`，等于没测。
   负例统一走 `query_err`（被接受就直接红），错误串里必须出现坏文件名。

## 6. 连带改动

- `runtime_scene/mod.rs`：删掉本地那份 90 行的建节点实现，改调 `build_ui_scene_nodes`；
  `RuntimeScene` 多一个字段 `applied_prefab_ui_nodes`。
- `runtime_scene/scene.rs`：新增 `pub fn prefab_ui_rects(viewport)`——它调的就是绘制前那次
  `ensure_scene_ui_layout`，所以读的是渲染消费的那一份，而不是第二套布局计算。
- `rustlua-scripting`：`ScriptState` 转 `pub(crate)` 并加 `prefab_spawns` / `next_prefab_handle` /
  `prefab_ui_nodes`；`LuaRuntime` 加 `prefab_ui_nodes()` / `prefab_spawn_names()`（返回**克隆**，
  因为宿主解算布局时还要 `with_world`，借用期间不能再借用同一个 `RefCell`）；
  新增 `script/api/prefab.rs`。
- `api_surface_snapshot/`：`lua_api_surface.json` / `lua_api_contract.lua` / transcript 三份重导，
  `engine/src/lua_api.rs` 与新增的 `lua_api/doc/prefab.md` 由生成器产出。

## 7. 代价与买不到的部分

跑过（全部单目标、`-j 2`，没有开全量）：

```
cargo run -q -p game  --example jhyt_m61_prefab_runtime      # ok（格式化后重跑一次，仍 ok）
cargo run -q -p game  --example jhyt_m59_ui_prefab           # ok golden_lines=102 anchors=12
cargo test  -q -p engine --test host_pub_surface             # 1 passed（新 pub 面没破宿主公开面钉）
node scripts/gen-lua-api-docs.js --check                     # ok，见 §3
node scripts/check-web-api-overrides.mjs                     # ok overrides=56 wholesale=23 dropped=72
cargo fmt --all -- --check → 29 个文件仍脏，本批 4 个文件已 rustfmt --edition 2024 洗白（0 命中）
```

没跑、因此**不声称**成立的部分：

- `cargo clippy --workspace --all-features -D warnings` 与 `verify.ps1` 全量门没跑（工作区里
  29 个在途文件不属本批，fmt 一红全量就不跑）。下次跑全量时这批的 Rust 代码只过了编译与门。
- **web / 微信**：`prefab` 表在 wasm 里是同一段 Rust 注册的，所以活体 API 面对拍
  （`scripts/verify-webgl-bundle.ps1`）下次会读到 42 张表——**未跑**，包也没重打。
  `plugins/webgl/lua_api_overrides.json` 的账是补丁层口径，`prefab` 不在里头，不需要补丁，
  但这条要靠下次真机门证实，不是本批结论。
- ~~`assets/prefabs/` 还没进 `scripts/first-screen-pack.js` 的首屏清单~~（M63b 已补：新增 5b
  扫 `assets/prefabs/*.prefab`，预设本体与它 `texture=` 引用的图一起收进首屏包，账见 `docs/67` §5）。
  `.prefab` 后缀在**打包侧**已实测（`assetc package` `files=1142`，manifest 有
  `assets/prefabs/title_menu.prefab` 一行）；**微信代码包白名单里的行为仍未测**，web/微信活体的
  `prefab.spawn` 对拍也没跑（上面那条仍然有效）。
- 逐预设引用 `CanvasConfig` 还没接：spawn 出来的 UI 用的是宿主当前的画布配置，预设自己声明
  的那份不参与解算。这是作者化 `title_menu` 之前必须补的最后一块（见 §8）。
- 10k 实体那类分配量测（`wp0-v4` 的口径）没有扩到 spawn 路径。本批只在**逻辑上**保证空表零分配
  （`PartialEq` 短路），没量过非空场景下的每帧成本。

## 8. 下一批

1. ~~`title_menu` 作者化成 `assets/prefabs/title_menu.prefab`~~（M63b 已做，见 `docs/67`；
   `ui.scene_text_visible(...,false)` 那批调用**没有**被替换掉——三行标签今天仍由脚本每帧画，
   预设里那三份 `Text` 照抄迁移前场景并保持隐藏，理由见 `docs/67` §6）。
2. 逐预设 `CanvasConfig` 引用。
3. 黄金对账门按 55 号 §6 的口径建（id→矩形/文本/字号，黄金表捕获自迁移**前**的 HEAD），
   并把 `assets/prefabs/` 加进微信首屏清单、跑 `assetc package/list/check-deps` 三条。
4. `dialogue` 及余下界面从 `*.ui.json` 迁到 `.prefab`，迁完退役 `scripts/ui/prefab.lua` 的
   JSON 解析层与 `dialogue.ui.json`。
5. 编辑器资源列表展示 `.prefab` 预设（用户报过的那条：目录与 `.prefab` 不在资源列表里展示）。
