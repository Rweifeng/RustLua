# Changelog

版本采用 git 标签（`git tag`）；本文只记录**能力增量**——做了什么、为什么重要、回滚档是什么。

度量数字（µs、分配字节、测试计数）、逐条证据、golden 断言与实施细节留在能力文档与任务文件里，不在本文重复：
重构过程见 [docs/53](docs/53-WP0增量场景帧基准与每帧分配.md) 与 [docs/40](docs/40-引擎全面重构计划.md)，已完成任务的验收记录见 `tasks/done/`。

## Unreleased — 地表还是"5 块平色 + 直线边界"：图集重做、色阶表成为唯一取色源，顺带把"同屏有几种瓦"的尺子量对（M72-04）

- **症状**：M72-02 把解算能力做齐了，`manor` 摆出来还是像 demo。三条根因都在**资产与规范**层，
  不在引擎：① `manor_ground.png` 实测 192×32、**1135 字节**——解算能指向 108 个条目，图集里只有
  6 格平色瓦，过渡块全是死数据；② `57` §1.1 锁了 8 个色板 token 但**每 token 只有一档**，
  "同族 3–5 档明度"这条星露谷口径（`53` §2）从来没落成表，所以"两张图是不是一个世界"没有尺子可查；
  ③ 交界永远是直线，因为瓦片本身没有压边像素可采。
- **先定预算再画**：计划的前置写着"`51` 定的是每图 ≤1 张 1024×1024"——**这条前提不存在**，`51` 是
  美术资源规范，真正的预算条款在 `57` §1 与 `54` §8。口径写进了该在的两处（`57` §1 新增"图集预算与
  寻址"行：每图 ≤4 张地表图集、贴图尺寸必须 == `cols×rows×tile`、空槽全透明；`51` §2.1 补引用行）。
  四季变体按**同尺寸换贴图**计额度（切季节只换 `texture=`），这一条不写清楚，M72-08 接完线才发现超预算。
- **色阶表是唯一取色源**：新增 `tools/terrain_ramp.json`（六族 × 4 档 + utility 四色 + 两档顶点色）
  与契约层 `tools/terrain_kit.py`（族色阶、明度单调、条目寻址、`tilemap_component()` 序列化、
  `observed_levels`）。生成器一律从表取色，**不许写 hex 字面量**——`gen_manor_art.py` 里旧的地表段
  整段删除（−134 行）。新图集 12×9=384×288：6 个 kind 的满块瓦 + 每 kind 16 格过渡块（102 条），
  尾部 103..108 留全透明（越界不许画成邻居的美术）。
- **四季变体是"换色"而不是"重画"**：按**族**做调色板替换，一族要么整族 `KEEP`、要么一次给满四档，
  **中间不留 `None`**（明度梯度是整体搬的，压掉一档就断一档）。新增覆盖率门要求图集里每个不透明色
  要么有换色目标、要么所属族是 `KEEP`、要么登记为季节中性——三处都不沾就红，于是"色阶表新增了东西
  而季节表没跟上"会在生成期显形，而不是变成某个季节里一格没换色的白斑。
- **门里最关键的一次判断是"同屏瓦种类"怎么量**：`resolve_atlas_index` 的变体分支在 `else` 里，
  **autotile 与 variant 互斥**，所以开自动瓦片后内芯格（mask `0b1111`）只能取该 kind 的**一个**条目
  （实测草内芯 185 格全取条目 22），重复只靠 per-tile 顶点色打散。按**纯条目**数，最坏的 20×12 窗口
  只有 12 种——"≥24"这条会永远红，而它红的不是美术。所以判据改成**外观等价类（图集条目 × 顶点色档）**：
  两者全同才是"两片一模一样的像素"。这不是放宽阈值，是把"屏幕上看着有几种"与"图集里被指向几格"
  分开写清楚，**两个数门都打出来**。
- **实测**：整层去重条目 **5 → 59**；实机一屏 252 格 = **64 种外观**（纯条目 31）；数据侧扫过全部
  20×12 窗口，最小 **25**；**0 格**交界取到满块瓦；天然地貌（草/土/水）之间同外观最长直边 5 格。
  色阶门实测的是**图集里真出现了几档**（`observed_levels`）而不是表里写了几档：
  `dirt/grass/roof/stone/water` 各 4 档；四季图先做同位置色**双射还原**再同判——这一步顺带证明了
  变体确实没改构图（几何、tile 边界、alpha 全同，只有一个颜色置换）。
- **三处反面对照真跑红**：合成"草地嵌 8×8 规则土块"报出 8 格同外观直边（证明那条断言不是恒绿）；
  草族整族压成单档让色阶门红；从门内色表删掉 `#6F8F52`，§⑤ 报出 1 个 offender 并点名它。
- **顺手三条**：① `combat_arena.scene` 与 `gen_arena_scene.py` 一起升到 12×9 声明（格子值一格没改，
  只改编码，与盘上对拍 0 处组件差异）；② `preview_scene.py` **无需改动**就吃对新布局（M72-01 起它按
  组件声明的 `cols/rows` 解 UV），这正好反过来证明"显式声明列数"比"从贴图尺寸反推"好修；
  ③ 色阶表在门内是**手抄副本**（`crates/game` 没有 serde_json，为一个门加依赖不值），漂移的表现是
  §⑤ 报"表外色"，不会静默放过。
- **已知限制**（写进计划，不藏）：`2→1` 最长直边 5 格**正好贴着阈值**、零余量；路/墙与其他 kind 的
  交界只报数不断言（人工结构直的是应该的，实测 `2→3` 有 8 格）；真正的"同族多外观"要等引擎把
  autotile 与 variant 复合；地表 `solid` 里 x33..36 × y14..20 有一片悬空碰撞格，留给 M72-05 摆装饰时
  一并处理。另有两条全量门的红**不属本包**（`gpu-web-probe` 在宿主目标 clippy 红、
  `rustlua-scripting/progress.rs` 的私有项文档链让 `cargo doc -D warnings` 红），属未提交的 M73-00 在建工作。
- **回滚档**：单笔 revert。数据侧只动了 `manor` 地表层与 `combat_arena` 的图集声明，玩法承重语义
  （`farm.lua` 的 `tile==2`、`building.lua` 的 `tile==3`、`buildings.lua` 的 `ground_tiles∈1..6`）
  一个没改。

## Unreleased — 中文终于不是压在像素美术上的平滑字：Fusion Pixel 点阵接入，顺带修掉一道"搬完家就变瞎"的排版门（M72-03）

- **症状（`docs/71` 第 3 项，观感收益最大的一条）**：`project.toml` 写的是
  `text_render_mode = "vector"` + `NotoSansSC-VF.ttf`，中文走 `ab_glyph`→MSDF，一律带抗锯齿灰边；
  同一屏的地表是硬像素，文字却是软的，两边一对照就"像 demo"。引擎当时只有 `auto/vector/sdf` 三档，
  **没有"外部位图字体"这条路**——着色器里那条硬阈值档（`shader.rs` 的 `sdf==2` + Nearest 采样）
  今天只服务内置 ASCII 5×7。
- **字体入库（授权有一处必须纠正的错）**：缝合像素字体 Fusion Pixel **12px monospaced zh-Hans**，
  release tag `2026.09.25`。表结构核验（纯 stdlib 读 sfnt 目录）：`glyf` 有 / `CFF ` 无、
  `unitsPerEm=1200`（12px 网格 = 100 units 一格）、无 `EBDT/EBLC` 位图表、无 `fvar/gvar`、
  `fsType=4`（允许嵌入+子集化）、advance 只有 600/1200 两档（半角/全角，PUA 的 3000 那 4 个字形
  工程取不到）。⚠ **不是 MIT**：GitHub API 的 `license.spdx_id=MIT` 说的是**源码仓库**，
  发布包内附的是 `OFL.txt`，TTF 的 nameID 13/14 自证 **SIL Open Font License 1.1**；
  上游版权行**没有** Reserved Font Name，所以子集化后改名不受条款 3 约束，而衍生品必须继续 OFL-1.1。
  台账与 `docs/71` 已按 OFL-1.1 回写，许可证正文 + 核验记录随字体入库
  （`examples/game_jhyt/assets/fonts/LICENSE-FusionPixel.txt`，条款 2 要求每份副本带着）。
- **体积**：子集 597,932 B（上游全量 7,040,712 B / 原用的 Noto 17,773,244 B = **1/29.7**），
  `jhyt_m72_pixel_font` ④ 的实测读数：工程用到的字符 **1,605 个**（`char_collect` 侧计数 1,606 个）、
  缺字 0、空字模 0。复现：
  `cargo run -q -p engine --example subset_project_font -- examples/game_jhyt --source engine/crates/editor/fonts/FusionPixel-12px-Monospaced-zhHans.ttf`
  （按当前 `char_collect` 收字重裁并**原地覆盖** `project.toml` 的 `font =`；`--source` 必须给全量字体，
  给成目标本身会直接拒——子集不可逆）。
  **上游全量那份 7 MB 不入库**（仓里已有一份 17.7 MB 的 Noto 做对照，再加一份
  纯子集源不值）：`jhyt_m72_pixel_font` 需要它时**报错并打印取回命令**，不静默跳过。
- **点阵身份判定按"栅格化覆盖率恰好二值"，不按轮廓坐标 GCD**：plan 原本想用 GCD（更省），
  实测**判据失效**——470 个字形里 429 个 GCD=100，另有 35 个带非整数坐标、6 个落在 50/25/10/2 的
  更细网格上，整体 GCD 被拉到 1。现由 `rustlua-render/src/font.rs::detect_pixel_size` 栅格两档探针
  （设计像素与 `RASTER_SCALE/2`，尺度无关）判身份，命中即把 `raster_scale` 从 96 改成设计像素高度
  （12），布局据此算 `tex_scale`。写死 96 的表现是"点阵显示成 8 倍大"。
- **两条只有真跑一帧才暴露的洞**：
  ① `mesh_ui.rs` 的 UI 文字通道按 `forced_vector` 无条件走矢量档——像素字被矢量重采样，硬边又糊回去；
  现加 `is_pixel_font` 挡板（`use_vector &&` 那条）。这条是单测逼出来的：断言"UI 文字段的四边形
  顶点总数"时先失败在 **510 顶点**（预设骨架也在这段）而不是预期的 24，说明探针根本没隔离到文字；
  收窄到 24 顶点后，同一测在旧实现下红、新实现下绿（`pixel_font_stays_out_of_the_vector_channel_even_in_forced_vector_mode`）。
  ② 整数吸附必须**逐边独立**取整：`x0=round(x+s0)`、`x1=round(x+s1)` 各自吸附，
  四边角全整数 ⇒ 每个 texel 恰好被一个设备像素采到。
- **验收门**：`jhyt_m72_pixel_font`（已登记进 `scripts/verify.ps1`）
  ① 字体身份（子集与全量同判 12px、Noto 判不出来）② 真跑一帧走 `sdf=2` 阈值档：85 个 UI 字形四边形
  亮 texel=3111 / 全暗=4966 / **灰阶=0**，反面对照同一批字在 Noto/MSDF 档灰阶成片
  （'山' 3603 个灰阶 texel vs 点阵档 0）③ 1:1 覆盖：12px 下逐 texel 采样次数恒为 1，
  把 0..1 的分数偏移全扫一遍只有 0 能全 1 ④ 缺字：用到的 1,605 字逐字断言覆盖且有点阵，
  🙂 判为缺字并仍画得出 39 个亮 texel 的占位方框 ⑤ 预算：全部工程字一次性预热 0.01s
  （两次跑 0.01/0.02s，量级远小于"首帧卡顿"这条线）、图集 1024×1024，
  点阵汉字单元 11×11 texel（同字 MSDF 档 75×87 = 54 倍面积）。
  复现：`cargo run -q -p game --example jhyt_m72_pixel_font`
- **顺手关掉另一条已经瞎掉的门（`jhyt_m30_cjk_text`）**：症状是"帮助页小标题 12px，want 18"。
  根因不是字号，是**载体搬家**：M66/B3 把暂停菜单从动态层（`ui_text.draw*`）搬进实体层
  （`.prefab` + `prefab.set_text`）之后，这道门读的 `ui.text_get(...)` 只剩 `ui.hide` 留下的
  **默认值占位**（12px、`visible=false` 且永远如此），另一条 `hud:tutorial` 那个 id 更是早已不存在
  ——断言在跑，但量的不是画面。修法是**一份登记表两种载体**：新增 `ui_text.record(owner,id,size,budget,text)`
  （只登记宽度契约、不产生绘制调用），面板搬进预设后仍然按迁移前那批预算被量宽；门的读回改走实体层
  （`entity.find` 换 `EntityId` → `UiText` 组件拿文字/字号，`prefab_ui_nodes` 是否含该键 = 这一帧画不画）。
  读数变化：量宽条数 `pse` 由 7 回到 **16**、帮助页 **33** 条此前**完全量不到**；
  `场景/预设 Text 168 个最小 14px`、`Lua 登记 28 处最小 11px`。
  ⑤ 的字号扫描同时覆盖两种载体（`draw*` 字号在第 5 实参、`record` 在第 3），按**整段函数名**匹配——
  `ui_text.recorded()`（登记表访问器）含 `record` 前缀，按前缀匹配会把它当成一处登记调用并报解析失败；
  报错现在带 `文件:行号`。M71 退役 JSON 预设层之后，⑤ 的数据侧那道不再要求"至少一个 text 节点"
  （那是要求一具尸体呼吸），改判"登记数与节点数不能一侧非 0"。
- **五处反面对照（都是实测，不是推理）**：① 把 `pause_menu.lua` 里一条 `ui_text.record` 注释掉 ⇒
  `只扫到 27 个 ui_text 登记调用（draw*/record，≥28）`；② 往退役的 `ui.prefab.NAMES` 里塞一份无 text
  节点的预设 ⇒ `UI 预设登记了 1 份却一个 text 节点都没扫到`；③ 把预设里一条 `Text` 的 `font_size`
  改成 10.00 ⇒ `场景/预设 Text 存在 10px 字号（下限 11px）`；④ 把 `PseHRow1` 改成 16.00 ⇒
  `PseHRow1 字号 = 16，want 18`（证明 ① 读的是真组件不是占位）；⑤ 把换页的
  `prefab.set_visible(handle, root, key == page)` 改成恒 `true` ⇒
  `帮助页打开时主菜单的行还在画：两页文字重影`。
- **未验证 / 已知红（都写明复现口径）**：非整数倍字号（14/18/22px 相对 12px 设计像素）下
  85 个四边形里 **80 个**不在 1:1 档，点阵被重排——归 M72-10「全 UI 换像素字」一并解决；
  HUD 在 M70 搬进预设时没补 `ui_text.record`，那批文案今天仍量不到，
  所以 `MIN_REGISTERED_TEXT_SITES` 只能从 30 降到 28（补齐后**上调**，不许留在这个数）；
  `jhyt_m31_pc_release` 红在 `release layout ... must be detected as packaged`，原因是
  `examples/game_jhyt/builds/pc` 是**加 `release = true` 之前**打的旧包（该目录 gitignore，不入库），
  复跑 `powershell -File scripts/package.ps1 -Platform pc` 即消——归 M72-12 的打包/目验一轮做，
  本轮不重打（`scripts/package.ps1` 此刻被 M73-00 改着）；三宿主目验按桌面形态与两个 web 形态的
  开发运行时完成，**微信真机与正式包未验**。

## Unreleased — tilemap 只能寻址 6 列 1 行：图集寻址、`.scene` 编码迁移与三宿主解算对齐（M72-01）

- **症状**：M72 第 2 轮美术照搬星露谷的图块规范画完，塞进游戏仍像 demo。根因不在画笔：
  一张图集**只能有 6 列、且只能有 1 行**——`tile_uv(column)` 的 y 恒为 `0.0..1.0`，
  列数真源是 `TILE_PALETTE` 那个字面 `6`，`set_cell` 与解析器双双拒绝 `value > 6`。
- **最贵的一块不是 UV，是序列化**：`to_component_str` 每格只写**一个 ASCII 数字**，
  解析端 `to_digit(10)` ⇒ `.scene` 格式**表达不了 >9**。字段类型 `tiles: Vec<u8>` 本来有
  0..255 的空间，是编码把它压成了十进制的一位。
- **改法**：① 组件新增显式声明的 `tiles_format`/`atlas_cols`/`atlas_rows`，**不从贴图尺寸
  反推**（隐式推法让"贴图里有几列"变成美术文件的隐式契约，改一下 PNG 尺寸就静默重排整张
  地图）；② `tiles_format=1` 用逗号分隔十进制，写出端恒写 1，`tiles_format=0` 仍能读且语义
  不变（6 列切 U、V 整幅拉伸、只认 `1..=6`）；③ 未知键一律报错，`tile_kind=` 少个 s 不许
  静默当 false；④ 同一份解算在三个宿主各写一遍并**对拍**：Rust 是真源，`web-runtime.js`
  里的 Lua 宿主补丁自带一份 Lua 镜像 + 一份 JS 镜像，逐帧用 `L`/`I`/`J` 三种 op 行同步；
  ⑤ 编辑器笔刷上界改由布局算（`max_brush`/`clamp_brush`），调试色板不再拿 6 色当列数真源；
  ⑥ 迁移工具 `tilemap_resave` 只重写 `Tilemap` 那一行载荷、其余字节原样回写（含 CRLF），幂等。
- **实测**：判据是**像素一样**而不是"看起来一样"——7 张带 Tilemap 的场景重存前后
  `py tools/preview_scene.py` 合成图 SHA-256 逐字节相同（7/7），换用迁移前的旧工具复算仍相同。
  新门 `jhyt_m72_tileset_atlas` 绿（12×8 图集 18 格逐格对上子矩形、索引 0 不产 quad、
  `manor_cells=920`），内建两处反面对照，另跑一次**外置**对照：临时工程里把 `atlas_cols=6`
  改成 2 ⇒ 门红在 §⑤ 并点名 `tile cell index 4 is out of atlas range`。三宿主解算对拍
  15 张随机地图 / 750 格（恒等、纯变体、纯 autotile、变体+autotile 四档）0 处不一致，
  变体序号另跑 1600 格 0 处不一致——这一条逼出了 Lua 的 `%` 是带符号 floor-mod、
  必须补 `u64_mod` 的坑。十条 prefab 黄金表逐道读数：九绿一红（`jhyt_m66` 红在陈旧黄金表，
  HEAD 上即如此，归因写在 `tasks/plan/M72-01-*.md`）。新增七个 `tilemap.*` API 的演示补齐，
  `sample_project` 11 道全绿。
- **顺手两条**：`scripts/check-docs.ps1` 的链接扫描把 CommonMark 合法的 `](<含空格路径>)`
  整串当路径喂给 `Test-Path`，在 `$ErrorActionPreference="Stop"` 下**崩掉整道门**——它后面的
  索引可达/历史路径/UTF-8 三类检查从来没跑过；L3 行为契约门在 `core.autocrlf=true` 的克隆上
  假红，`.gitattributes` 给 api_surface 快照目录钉 `text eol=lf`。
- **回滚档**：两笔——`engine`/`web`/docs/台账/门 一笔，7 张 `.scene` 重存单独一笔。
  旧编码仍可读，所以**只回滚代码不会坏数据**；只回滚数据会让新代码读到 6×1 布局（同样正确）。
  无存档格式变更（`.save` 不写 Tilemap 载荷）。

## Unreleased — 建造模式看不见自己选的是哪座：七日炼药目标卡死

- **症状**：第 7 天目标「炼成 1 剂药并收取」停在 0/3，提示区写着「没有对应设施」。存档里
  材料是够的（`lingzhi ×2` 正好是 `pill_refine` 的全部输入，建造本身不要钱），缺的只是一座
  `station="herb"` 的药庐——而玩家按 B→F 建出来的却是两座畜棚。
- **根因**（三条叠在一起，没有一条是玩法数值）：
  ① `set_build_mode` 的默认选中写死 `building_data.ids()[1]`，`ids()` 是字典序 ⇒ 畜棚，
  药庐在 10 座里排第 6，要按 5 次 Q 才轮得到；
  ② `建造[畜棚] 可放置` 这句此前**只**进 `hud.text()`（headless 单行摘要），屏上四类文字槽
  （`HudLine1..6`/`HudFeedback`/`HudGoals`/`HudTutorial`）里没有它，建造模式只多一个绿/红幽灵
  小图标 ⇒ 玩家没有任何线索知道自己选错了；
  ③ 引导第⑨步文案只写了「按 B → F 放置药庐」，没写换选那一步。
  顺带查出同一条路径上的第二个缺陷：`cell_conflict` 不检查玩家自己站的格，实测存档里玩家
  坐标正落在自己建出的畜棚 3×2 footprint 中心，而那些格已被 `set_solid`（引擎没有脱困逻辑）。
- **改法**：`hud.lua` 把那句状态短语抽成 `build_verdict()`，两个消费者共用——`M.text()` 逐字
  不动（`input.update():match('building')` 那批既有断言读它），`feedback_line()` 改为**追加**
  而不是顶掉（`建造[药庐] …（Q 换建筑 · F 放置）`，保留 `jhyt_m19_ui` 按子串断言的中文冲突原因）；
  `building.lua` 的占用枚举加 `player_here`，优先级排在 `ground` 之前（站在能建的地表上才是
  这条会命中的场景），两张中文字表同步补；引导第⑨步补换选那一步。
- **实测**：`jhyt_m18_build` 新增 M18-02h′ 一节钉三件事——建造模式下提示行带当前建筑名与 Q、
  换选后同一行跟着变成「膳房」、玩家自己那一格 `can_place`/`place` 都拒。两条反面对照各红一次：
  摘掉 `player_here` 判据 ⇒ `reason:ground@18,0`，摘掉提示行追加 ⇒ `name:› 进入建造模式`。
  既有面 `jhyt_m19_ui`/`m26_hud_feedback`/`m27_goals`/`m19_slice`/`m34_teahouse`/`m41_greenhouse`/
  `m44_animal`/`m47_cooking`/`m54_clinic`/`m34_save_gates` 全绿；`jhyt_m30_cjk_text` 是既有红
  （`pse:h:row1` 字号 12≠18，读的是暂停菜单预设，与本次三个文件无交集）。
- **回滚档**：单笔 revert 即可，无存档格式变更（`player_here` 只是拒绝原因，不落盘）。

## Unreleased — 预设骨架画在动态层之上：招式图标被不透明卡底整块埋掉

- **症状**：编辑器 Play 的 HUD 上，7 张招式卡只有中文名、没有图标；右上角那个头像框是空的。
  两条不同账：图标是**绘制序**缺陷——M66–M71 把卡底搬进预设之后就在，两个宿主（原生 + web）
  一起瞎，而状态层读数照样自报 `shown`；头像框是 docs/45 第 10 条的欠账 ②（从来没有一张主角脸）。
- **根因**：`push_scene_ui` 把场景/预设实体 UI（`Image`+`Text`+`Button`）**整趟**排在 Lua 动态层
  之后提交。预设卡底 `ui_skill_card_9s.png` 主体全不透明，于是压在它上面的 24×24 动态
  `ui.image` 招式图标被逐像素盖掉。web 侧 `drawUi` 是同一条规则的另一份实现，同样反着排。
- **改法**：原生把那一趟拆成两半——`push_scene_ui_images` 提到 `push_ui_panels` **之前**（骨架在下），
  `push_scene_ui_text` 仍留**最后**（`prewarm_frame_glyphs` 必须是本帧最后一个会撑大字体图集的人：
  图集增长只重缩放已缓存字形的 UV、不回改已写进顶点缓冲的 UV，所以文字趟挪不动）；布局解算
  由调用方每帧做一次（`ensure_scene_ui_layout`，否则求解翻倍）。web 侧 `drawUi` 把预设图趟
  挪到函数开头，文字趟留末尾，与原生同一口径。
- **门模型补了"层"这一维**：7 张预设黄金门的 `covering_account` 从"谁盖谁"升级为
  `slot_of(role, reading)` 六档（实体图 0 < 动态面板 1 < 动态图 2 < 动态字 3 < 实体字 4 < 缺席 5），
  档内再排序（动态层按 id 字典序、实体层按槽位升序）。原来那条 `submission_order`（"白图段必须在
  贴图段之前"）在整屏同层下只是槽位序的产物、不是层序规则，留着会在任何一屏重刷表时假红 ⇒
  6 张面板屏改成**只报段序读数、不做背书**；真正钉层序的是 `jhyt_m63` 的段序表和 `jhyt_m70`
  新增的层档腿（卡底与图标都必须在场且卡底先提交，缺任一边判"前置塌了"、宁可红）。
- **实测**：`jhyt_m70` 层档读数 = 预设卡底段 `#28` 在动态招式图标段 `#31` 之前提交（本帧 38 个分段）。
  离屏 1280×720 采样 6 格 24×24 图标区的颜色直方图：改前每格都是 `#262C33=576/576`（纯卡底、
  图标 0 像素），改后同点位卡底色降到 346–515/576，也就是每格露出 61–230 个图标色像素
  （`#C9A05A`/`#E8C49A`/`#A9C28F` 这些正是生成器那批招式图的配色）。
- **反面对照（三条各自红过一次）**：①把 `push_scene_ui_images` 挪回 `push_ui_text` 之后 ⇒
  `jhyt_m70` 报 `Error: "③ 的分层规则被实测否了：预设卡底段 #36 在动态招式图标段 #15 之后提交"`、
  exit=1，还原后回绿（golden 138 行）；②注释掉 `hud.lua` 的 `draw_avatar_face()` ⇒ 同一门红在
  golden 第 70 行（`hidden` 对不上 `shown`）；③把微信 launcher 的 `web-runtime.js` 换成
  HEAD 那份（未修的 `drawUi`）⇒ `--check-ui-layer-order` 报 `14/14 组"卡底压图标"：卡底 #986
  画在图标 #949 之后`、exit=1。微信侧修好的读数：
  `ui_layer_order="ok,card_ops=72,icon_ops=7,overlap_pairs=14,last_card=1041,first_icon=1026"`。
- **头像（欠账 ②）**：`tools/gen_portraits_art.py` 的 `PORTRAIT_IDS` 补 `player_male`/`player_female`
  × 3 表情 = 6 张 128×128 立绘（调色板与发型直接复用 `gen_character_sprites.CHARACTERS`，所以头像
  和场景里那个小人是同一个人；15 张既有 NPC 立绘逐字节未变）。`ui/portrait.lua` `SHEETS` 15→21；
  `ui/hud.lua` 每帧按 `state.appearance` 取 calm 那张画成动态 `ui.image("hud:avatar_face", 928, 20, 40, 40)`
  ——矩形是预设 `HudAvatarFrame`（924,16,972,64）内缩 4px，正好落在金线以内，框与脸互不遮挡。
  留动态层而不进预设：贴图是运行期取值（`jianghu_set_appearance` 能换形象、随存档持久化），
  引擎没有运行期换贴图 API，与对话立绘同一个理由。docs/48 §1 已引用 978→**984**、
  `m29` 的 `EXPECTED_REFERENCED` 同步。
- **买不到的部分**：①这条增量在**今天量不到**——`jhyt_m29` 的 §1 资源审计在走到之前先红在雨天
  遮罩那一腿（`ui.panel_get('hud:rain')` 读不到：M70/B7 把雨罩搬进预设 `HudRain` 后那个动态面板
  已不存在，`RAIN_OVERLAY` 常量还留在 `hud.lua` 里没人用），所以 984 是算出来的，修那条的人会
  直接看到实测数；②微信全门跑不到我这条 leg——门在 leg 5（`--image-src-frozen` 本该响亮失败却
  `boot=running`）就 throw，而我的用例排在 leg 8 之后；那条红是既有的，用 HEAD 的 harness + HEAD 的
  运行时打同一 launcher 复现出**逐字相同**的两行失败，与本次改动无关，我这条 leg 的证据是上面
  直接 node 跑的读数与反面对照；③`verify-webgl-bundle.ps1 -DriveJhyt` 同样红在既有的
  `phase=nav-exit timeout`（HEAD 运行时在临时目录里复现同一读数）；④屏幕目验只覆盖了 HUD
  （离屏采样 + `m70` 读数），另外六屏没有逐屏看像素；⑤六张面板屏的 `cover` 行**没有**重刷——
  层维改动只让 `m70` 表体 +2 行和 `m63` 的段序常量重刷，其余表一个字节没动，这本身也是"层维
  只影响这一处"的旁证。
- 顺带记下的既有红/残留（本次不修，只报）：`jhyt_m66` 第 562 行 golden 期望"工程 settings.toml 的
  [input] 段…"，而 `pause_menu.lua:138` 现在写的是 `project.toml`（`68ffcde` 引入的文案漂移）；
  `mesh_ui.rs` 里三条 `TEMP DEBUG` 的 `tracing::warn!` 也是 HEAD 既有。
- 回滚档：`engine/crates/engine/src/host/runtime_scene/{frame_data.rs,mesh_ui.rs}`、
  `plugins/webgl/web-runtime.js`、`engine/crates/game/examples/jhyt_m6{3,5,6,7,8,9}_*_prefab.rs` +
  `jhyt_m7{0,1}_*_prefab.rs` + `support/jhyt_m70_hud_golden.tsv`、`jhyt_m29_season_weather.rs`、
  `scripts/{verify.ps1,verify-wechat-minigame.ps1,wechat-minigame-harness.js,gen-api-showcase-index.ps1}`、
  `examples/game_jhyt/{scripts/ui/hud.lua,scripts/ui/portrait.lua,tools/gen_portraits_art.py,assets/art/portraits/player_*.png}`、
  `engine/crates/engine/src/host/sample_project.rs`、`examples/game_jhyt/docs/{45,48}*.md`、本文件。

## Unreleased — 引擎图标统一：齿轮 + Lua「L」，窗口图标与三个 exe 的文件图标同源

- **症状**：编辑器标题栏/任务栏是 eframe 的默认白底「e」，`game.exe` 的标题栏是 Windows
  默认应用图标，而 `editor.exe`/`game.exe`/`assetc.exe` 在 Explorer 里是白纸——四个入口四种
  身份，发布包里看不出这是同一个引擎。
- **改法**：新增 `engine/tools/gen_engine_icon.py`（纯标准库：SDF 栅格化 + 4× 超采样，
  **一张规格表**同时出窗口图标 `engine/assets/engine_icon.png`（256²）和多尺寸
  `engine/assets/engine_icon.ico`（16/24/32/48/256，≤48 走 32bpp DIB、256 走内嵌 PNG），
  外加 5 档 × 明暗两行的目验拼图）。带两条门：`--check` 的字节幂等（复算不一致就红），
  以及"字必须整个收进齿轮内孔"的设计约束（`glyph_max_extent()` 0.189 < `GEAR_HOLE` 0.205，
  负面对照 `GLYPH_HW=0.075` 当场 FAIL）。接线三处：编辑器走
  `ViewportBuilder::with_icon`（eframe 内部按 `SM_CXICON`/`SM_CXSMICON` 各做一次 Lanczos3，
  所以直接喂 256 那张）；`game` 走 `Window::default_attributes().with_window_icon(...)`，
  按主显 DPI 自己把 256 缩到 `16 × scale_factor`——**winit 只发一次
  `WM_SETICON(ICON_SMALL)` 且按 HICON 原尺寸交给 DWM**，直接喂 256 会把齿和 1.3px 的笔画糊掉；
  三个 bin crate 各一行 `include!("../../tools/winicon_build.rs")`，用 rc.exe（找不到退回
  `engine/.tools` 里的 `zig rc`）把 .ico 编成 .res 交给 `link.exe`。
- **失败口径是两档**：没找到资源编译器只 `cargo:warning` 后跳过（缺的只是 Explorer 里那个文件
  图标，不该让没装 SDK 的人编不过）；找到了但编译失败直接 panic（那是我们的 .rc/.ico 坏了）。
  非 `-windows-msvc` 目标早退（gnu 要 windres，本仓库没那个目标）。
- **新增核验工具** `engine/tools/check_exe_icon.py`：build.rs 只能证明"rc 编出了 .res 并交给了
  链接器"，`link.exe` 收下不等于 PE 里真有 `RT_GROUP_ICON`，所以它把图标**从资源目录读回来**
  与入库 .ico 逐字节对账。读数（仓库根）：
  `PYTHONIOENCODING=utf-8 py engine/tools/check_exe_icon.py engine/target/{debug,release}/{editor,game,assetc}.exe`
  → 六个产物全 `OK ... RT_GROUP_ICON + 5 张 RT_ICON（16/24/32/48/256px），逐字节等于 engine/assets/engine_icon.ico`。
  两条反面对照都红：把 .ico 第 3 项的 imageOffset 挪 1 字节 →
  `FAIL editor.exe：32px 像素字节与入库 .ico 不一致`；拿无图标的 `C:\Windows\System32\ping.exe` →
  `FAIL ... 没有 RT_GROUP_ICON`。生成器侧：`--check` 报 `OK ... 规格指纹 cf39e8e2be6e`
  （PNG 7,874 B、ICO 25,432 B）。
- **买不到的部分**：①OS 画的标题栏/任务栏图标没做屏幕目验——`gui_evidence` 只 dump egui 客户区，
  能给的只有目验拼图和"从 PE 读回的字节"；②Explorer 的图标缓存会在原地显示旧图标，同一台机器上
  要换目录看或重启 explorer 才可信，这条我没验；③非 msvc 目标与"没有资源编译器"那条分支只看了
  代码路径，没产过物；④16px 档的 L 只有约 1.3 像素粗，是被"整个收进内孔"压出来的取舍，比之前那版
  大字 L 弱。
- 回滚档：`engine/tools/gen_engine_icon.py`、`engine/tools/check_exe_icon.py`、
  `engine/tools/winicon_build.rs`、`engine/assets/engine_icon*`、
  `engine/crates/{editor,game,assetc}/build.rs`、`engine/crates/editor/src/main.rs`、
  `engine/crates/game/src/game_app.rs`、本文件。

## Unreleased — 搬门关掉写死绝对路径的调试残留：跑一次门不再脏仓库根

- **症状**：`git status` 在跑完 `jhyt_m70_hud_prefab` 后总挂一条 `?? .tmp_m70_got_dump.txt`
  （7,111 B），下一条来自 m71 的同族。它不在 `.gitignore` 里（`/.tmp_*/` 只挡目录），于是每跑一次
  门就多一个未跟踪文件，容易被读成"上一批没提交干净"。
- **归因**：`68ffcde` 给两个对账门的非 capture 分支各留了一行
  `std::fs::write("C:\\GitHub\\RustLua\\.tmp_m7?_got_dump.txt", &dump).unwrap_or_default()`
  ——写死的单机绝对路径（Linux/CI 上静默失败），且表体本来就有 `--capture` 这条正通道，
  重刷黄金表也从没读过这个文件。是脚手架，不是门的一部分。
- **改法**：删掉这两行（`jhyt_m70_hud_prefab.rs:445`、`jhyt_m71_dialogue_prefab.rs:244`），
  `compare_golden(&dump)?` 原位不动；顺手改掉两处会跟着画面漂移的行数注释
  （`scripts/verify.ps1:1037`、`hud.prefab:6` 的"黄金表 124 行"→ 136）。
- **读数**：两门删后各跑一次仍绿——`golden_lines=136` + `jhyt_m70_hud_prefab_ok`、
  `jhyt_m71_dialogue_prefab_ok`；跑完 `ls .tmp_m7*.txt` 报无此文件、`git status` 只剩本批四个改动文件。
- **买不到的部分**：这次没给"红的时候打印整张实得表"补通道（`compare_golden` 仍只报第一条不一致的
  排序行），要看全表得自己带 `--capture` 跑一遍——够用，因为它本来就是重刷通道。同族的
  `probe_title_render.rs:157` 也写死绝对路径（`engine\title_offscreen.png`），但那是给人眼看的离屏
  截图、被 `engine/*.png` 忽略、不脏 status，本批有意不动。
- 回滚档：`engine/crates/game/examples/jhyt_m7{0,1}_*_prefab.rs` + `scripts/verify.ps1` +
  `examples/game_jhyt/assets/prefabs/hud.prefab` + 本文件。

## Unreleased — 招式图标补齐：`SKILL_ICON_KEYS` 从 3/7 到 7/7（走生成器出图，不凭空画）

- **症状**：HUD 招式栏 7 张卡里只有 3 张有图标（普攻/追风步/守心诀），千风掌、弦月斩、铁砂掌、
  豹跃四格是空卡底。`docs/45` §"它买不到的部分 ①" 从 M70 那批就登记过这条，并写明补法要
  走 `docs/35`/`docs/48` 的生成器出图，不在那条里凭空画——本批就是那笔欠账。
- **改法**：`tools/gen_ui_art.py` 的 `skill_icon()` 加四个 kind（`palm_gust` 同心风弧 + 立掌、
  `crescent_slash` 双弧相减的月牙刃、`iron_palm` 铁色正掌 + 朱腕带、`leopard_step` 豹掌印 +
  速度线），skills 模块 6 → 10 张、全套切图 96 张；`ui/skin.lua` 登记四个 key、
  `ui/hud.lua` 的 `SKILL_ICON_KEYS` 补齐到 7 式。图标名跟数据层的招式 id 对齐
  （**`leopard_step` 而不是"豹跃"的直译**：第一版按 `leopard_leap` 命名，查 `M.binding.cast`
  才发现真实 id，改名重出）。
- **重出是幂等的**：`py tools/gen_ui_art.py` 会重写全部切图 + manifest + 两张预览，跑完
  `git status` 只有 4 张新增 + `manifest.json`/两张 `_preview_*.png` 三处变化，其余 92 张
  既有 PNG 逐字节不变 ⇒ 生成器仍然可复现，没有夹带别人的手改。
- **原生门跟着记账**：`jhyt_m70_hud_prefab` 的图标探针原本只有 `skill_icon1..2`（那两条就是
  当时的现状），现按 `txt/skill_name1..8` 的排法补到 `skill_icon1..8`，黄金表 124 → 136 行；
  第 52-58 行是七张各不相同的贴图路径，第 59 行 `hidden`（第 8 格没有招式槽，那儿画的是触屏
  「更多」键）。**先红后刷**：加映射不改表时它红在
  `黄金对账第 65 行不一致：期望 A\thidden\timg/skill_icon2`，说明这条门真的盯得住卡底内容。
- **读数**：`verify-webgl-bundle.ps1 -DriveJhyt` ⇒ `ui elements: 34 → 38`（四张新动态图标，
  这条读数只数动态层所以看得见本批）、`prefab_slice: 81/9`、`prefab_text: 22/22`；
  `verify-wechat-minigame.ps1` ⇒ 19 条 `harness_ok`、`prefab_slice` 标题屏 `36/4` / 玩法图 `63/7`。
  两包用 `package.ps1 -SkipBuild` 重打（webgl 与 launcher 的 `web-runtime.js` 仍是 159,148 B
  逐字节相同；微信 `main_kb=2,091 / 51.0%`、`total_kb=6,786 / 33.1%`），pack 里 10 张招式贴图齐全。
- **买不到的部分**：黄金表只记"这一格画了哪张路径"，不记图标长得对不对——贴错图（把铁砂掌
  挂成弦月斩）它照样绿；图标是 32×32 源画进 24×24 卡位，web 侧走 canvas 平滑、原生走 wgpu
  采样，缩放观感本来就有差，本批没做像素比对；`docs/30` §8.2 还写着旧的五式阵容，与
  `M.binding` 的七式已经漂移（那是文档账，不在本批里改）。
- **既有红（与本批无关，都验过）**：`jhyt_m23_data_validation`（`ui.prefab defines validate_all
  but is not registered`——那两个文件都在 HEAD 状态、本批没碰）、`jhyt_m30_cjk_text`
  （`pse:h:row1 size=12 want 18`——把本批的 `hud.lua`/`skin.lua` 临时还原成 HEAD 复跑，同样红）、
  `drive_jhyt nav-exit timeout`、微信 case 5 `--image-src-frozen`。
- 回滚档：`examples/game_jhyt/tools/gen_ui_art.py` + `assets/art/ui/{skills/*.png,manifest.json,
  _preview_*.png}` + `scripts/ui/{skin.lua,hud.lua}` + `docs/45` +
  `engine/crates/game/examples/jhyt_m70_hud_prefab.rs` + `support/jhyt_m70_hud_golden.tsv` + 本文件。

## Unreleased — 预设贴图的 `nine_slice` 解析完没人读：屏幕 UI 整张拉伸

- **症状**：webgl/微信宿主上，凡是由 `.prefab` 声明的面板与卡底都"边框糊成一大片、里面发黑"——
  标题屏的三份 `ui_menu_btn_wuxia_9s`、玩法 HUD 的 `HudSkillCard1..8`、背包/商店/暂停的
  `ui_panel_main_9s` 尤其明显。Lua 侧 `skin.panel()` 走 `ui.image` 的第 10 参那条 9-slice 是好的，
  所以只有"预设里的图"看着不对，容易被读成贴图坏了。
- **归因**：`web-runtime.js:246` 把 `Image.nine_slice` 解析进 `entity.image.nineSlice` 之后
  **全文件再没有第二个引用点**（`grep -n nineSlice` 只有那一行），`drawSceneUiEntity` 的贴图趟直接
  `ctx.drawImage(texture, rect.x, rect.y, rect.w, rect.h)` 把整张拉进目标矩形。原生不是这么画的：
  `mesh_ui.rs:727-775` 用 `nine_slice_cells` 把目标矩形拆成 3×3（角部固定纹理像素、边单向拉伸、
  中心双向拉伸），`push_scene_ui`（`mesh_ui.rs:515`）把这个字段一并带给场景/预设实体。
  量化：`ui_panel_main_9s.png` 是 96×96、边距 16 px，画进 720×640 的面板矩形时 16 px 边框被吹成
  约 120 px；`ui_skill_card_9s.png` 是 128×64（2:1），压进方形卡底 ⇒ 卡框横向挤扁。
- **改法**：预设贴图趟改走已有的 `drawNineSliceImage`（动态层本来就用它），判据与原生同一条
  （四个边距有限、非负、且至少一个 > 0）；`drawNineSliceImage` 顺带返回实发的 `drawImage` 次数。
- **新增读数**：`data-rustlua-prefab-slice=切片次数/该切实体数`（每帧）。上一条刚立过教训——
  `drawn`/`ui-elements` 不计屏幕 UI 实体，所以"整张拉伸"在那两条读数上是**完全静默**的，这次直接
  报一对。两条门各接三条：缺字段红、`cells < entities*2` 红（= 每实体只发一次 drawImage）、
  进玩法图后 `entities == 0` 红；webgl 侧仍排在 drive 判定之前。
- **读数**：`verify-webgl-bundle.ps1 -DriveJhyt` ⇒ `prefab_slice: 81/9`、`prefab_text: 22/22`；
  `verify-wechat-minigame.ps1` ⇒ 19 条 `harness_ok`，标题屏 `prefab_slice=36/4`、
  `--drive-new-game` 后 `63/7`（7 张卡 × 9 格，逐格对得上）。活体浏览器同一读数
  （`36/4` → 进图 `63/7`），标题屏与山庄图各存一张截图目验边框。**反面对照**：把该分支换回
  单张 `drawImage` ⇒ 微信门 exit=1，报"声明 nine_slice 的预设贴图 4 个只发了 0 次 drawImage"。
  产物：`builds/{webgl,wechat-minigame/launcher}/web-runtime.js` 由 `engine/.tools/terser` 重生成
  （158,669 → 159,148 B，+479，两包逐字节相同）；重打包前核对过上一节的产物字节，没夹带在途改动。
- **买不到的部分**：这条只断"切没切"，不断边距数值对不对、也不断切出来的九格落在哪——
  现网所有 `nine_slice` 都是 `l=r=t=b` 的对称值，所以"边距顺序写反"这类错它抓不到；也没有与原生
  做像素比对（原生是 wgpu 三角、web 是 canvas 2D，本来就对不齐）。卡底中心发黑**另一半**原因是
  内容缺口：`hud.lua:495-499` 的 `SKILL_ICON_KEYS` 只覆盖 7 个绑定招式里的 3 个，另外 4 个
  （`palm_gust`/`crescent_slash`/`iron_palm`/`leopard_leap`）既没有映射也没有对应贴图，PC 侧
  golden `jhyt_m70_hud_golden.tsv:52-53` 早就把 `img/skill_icon2` 记成 `hidden`——那不是宿主缺陷，
  本修没动，要补的是美术与映射表。
- **既有红（与本修无关）**：`drive_jhyt: phase=nav-exit timeout frames=901`、微信 case 5
  `--image-src-frozen`（前几节已记为"归下一摊"）。
- 回滚档：`plugins/webgl/web-runtime.js`（`drawSceneUiEntity` 贴图趟 + `LAST_PREFAB_SLICE` +
  `drawNineSliceImage` 返回值 + `data-rustlua-prefab-slice`）+ `scripts/verify-webgl-bundle.ps1` +
  `scripts/wechat-minigame-harness.js` + 本文件。

## Unreleased — 换场景后 prefab 正文整批上不了屏：`sceneTextSuppressed` 只该管场景文件声明的实体

- **症状**：webgl/微信宿主进玩法地图后，HUD 与所有面板只剩暗底——`HudBox`/`HudChip1..6`/血条/面板
  底图照画，`HudLine1-6`、目标、提示、背包容量与行标签、详情正文一个字都没有。表现就是屏幕上几块
  "黑框"。启动场景（标题屏）完全正常，所以看着像字体或贴图坏了。
- **归因**：`web-runtime.js` 的 `applySceneSwitch` 给每个换进来的场景置 `sceneTextSuppressed = true`，
  而 `drawSceneUiEntity` 的文字趟用 `!suppressText` 把**所有**屏幕 UI 实体一起挡掉。这道门的本意只
  是挡场景文件自带的 `HudTitle/HudHint`（原生 `ui_scene_nodes` 布局快照换场景后不重建，会叠成两层
  字），但 `prefab.spawn` 出来的实例是 Lua 换场景后重新生成、每帧 `prefab.set_text` 填字的，快照
  失效与它无关，被连带打死。`set_text` 返回 `true`，画面上却没有那行字——两头都不报错。
- **改法**：`(!suppressText || entity.prefabHandle !== undefined)`。prefab 实例带 `prefabHandle`、
  场景文件实体没有，所以豁免精确，原来要挡的重影一个字没放开（左上实测无重影）。
- **旧门为什么守不住**：`data-rustlua-drawn`/`ui-elements` 只数 Lua 每帧 `ui.*` 的动态行，屏幕 UI
  实体一个都不计入 ⇒ 修复前后 `drawn samples: 5713 / ui elements: 34` **逐位相同**。
- **新增读数**：`data-rustlua-prefab-text=画/应有`（每帧），只统计运行期 prefab 实例里带非空 Text
  的实体。分母**不含**被 `ui.scene_text_visible(name,false)` 逐名接管给脚本绘制的（`title_menu` 的
  三份 `Btn*Label`）——第一版把这三份算进了分母，微信门立刻报 `4/7`，那是正确行为被冤枉，不是缺陷。
  `verify-webgl-bundle.ps1`（缺字段/画不满/进图后应有为 0 三条，且排在 drive 判定**之前**，否则 drive
  本来就红，断言永远轮不到跑）与 `wechat-minigame-harness.js`（与瓦片层自对账同段）各接一条。
- **读数**：`node scripts/wechat-minigame-harness.js ... --frames 12` 三形态全 `harness_ok`；
  `verify-webgl-bundle.ps1 -DriveJhyt` 为 `prefab_text: 22/22`。**反面对照**（只回退豁免、保留读数）
  ⇒ `prefab_text: 0/22` + `[RED] webgl_bundle_prefab_text_suppressed drawn=0 expected=22`，证明这条
  断言真的盯得住。产物：`builds/{webgl,wechat-minigame/launcher}/web-runtime.js` 由
  `engine/.tools/terser` 重生成（158,669 B，两包逐字节相同）；重生成前核对过 `terser(HEAD 源码)` 与
  旧产物 `cmp` 相同，所以没夹带在途改动。
- **买不到的部分**：这条只回答"该画的 prefab 文字有没有走到 `fillText`"，不回答字画在对的位置、
  对的尺寸上；假 ctx 不栅格化字形，微信侧 `drawn` 依旧看不见纯文字画面。面板该不该盖住技能图标
  （换场景后 `screenUiEntities` 按 `id` 升序排，prefab 句柄基数大 → 后画）是另一摊，本修没动。
- **既有红（与本修无关）**：`verify-webgl-bundle.ps1 -DriveJhyt` 仍红在
  `drive_jhyt: phase=nav-exit timeout frames=901`，`git show HEAD` 版产物复跑读数一字不差；
  `verify-wechat-minigame.ps1` case 5 `--image-src-frozen` 同理（上一节已记为"归下一摊"）。
- 回滚档：`plugins/webgl/web-runtime.js`（豁免条件 + `LAST_PREFAB_TEXT` + `data-rustlua-prefab-text`）
  + `scripts/verify-webgl-bundle.ps1` + `scripts/wechat-minigame-harness.js` + 本文件。

## Unreleased — 标题屏 `drawn=0` 是量具瞎：harness 的假 ctx 补了仿射变换栈（附独立复算）

- **归因翻案**：`docs/52` §6.2 把这条红记成"标题门面进 `.prefab` 之后的渲染差异"，是错的。三条实测
  指向量具本身：① 同一次 harness 跑里 `ui_elements=13`、`images: ok=7 fail=0`、每帧绘制调用严格递增
  （游戏确实在画，贴图一张没失败）；② **同一份产物**在真浏览器过 `verify-webgl-bundle.ps1` 读
  `drawn samples: 251 ui elements: 13`，`data-rustlua-ui-summary` 与微信侧逐字一致；③ 差的只是口径——
  运行时每帧先做 contain 适配（`fitS = min(canvas.width/1280, canvas.height/720)`），而假 2D 上下文把
  `save/restore/translate/scale/setTransform/transform` 一律设成 `noop`，于是绘制记在**设计坐标**
  （标题元素全在 x=378…1240）、`sampleDrawnPixels` 的窗口开在**设备坐标**（harness 画布 375×667，
  `fitS=0.293`）⇒ 恒 0。manor 能读到几千纯属"瓦片从设计 x=0 铺起"的歪打正着。反方向同样错：真机被
  letterbox 裁掉的内容它会算成"有画面"。
- **改法**：假 ctx 实现真仿射矩阵（`a,b,c,d,e,f` + `save/restore` 压栈，`transform` 按
  `new = current ∘ arg` 复合），`fillRect`/`drawImage` 入 `painted` 前把矩形四角落过矩阵取轴对齐包围盒；
  `getImageData` 不过矩阵（真 canvas 的像素回读本就在设备空间）。断言阈值一个字没改。
- **读数**：标题屏 0→60 转绿；这个 60 与"把同一批矩形按同一套 contain 数学独立重算一遍采样"的结果
  **逐位相同**（`{"fitS":0.29297,"fitOx":0,"fitOy":228.03,"samples":4720,"expected_drawn":60}`）；
  比例对照 真浏览器 251/17389=1.44% vs harness 60/4720=1.27%（修前 0%）；manor 侧同一条断言
  4625→3050（换的是口径不是阈值，设备面积本就 = 设计面积 × `fitS²` = 8.6%）。
  `verify-wechat-minigame.ps1` 到 case 5 之前共 19 组 `harness_ok/harness_negative_ok`，case 5 之后
  的 12 个用例逐个手跑全 `exit 0`。
- **买不到的部分**：假 ctx 不栅格化字形，`fillText` 只进 `drawnTexts` 不产像素 ⇒ 纯文字画面在 harness
  里依旧读 0；`drawImage` 记的是写死的 `[200,120,60]`、不看位图真实内容 ⇒ `drawn` 只回答"有没有东西
  画进可视区"，不回答"画得对不对"。2.5 MB 单文件上传上限那条未知也没被这次改动动到。
- **解锁后新撞上的红（归下一摊）**：gate case 5 `--image-src-frozen`（subpackage 形态）期望"宿主不让
  改写 `Image.src` ⇒ 响亮失败"，实测 `boot=running`、`接管方式=own-accessor`、`images ok=7`，适配层
  一句报告都没有。用 `git show HEAD` 版 harness 复跑，红字一字不差 ⇒ 与本修无关，它只是从没被跑到过
  （case 3 的 `drawn=0` 一直把门挡在它前面）。
- 回滚档：`scripts/wechat-minigame-harness.js`（假 ctx 变换栈那一段）+ `docs/52` §6.2 的归因更正与新增
  §6.3 + `plugins/wechat-minigame/README.md` 的 `drawn` 口径条目 + 本文件。

## Unreleased — 6 条"部分修"的余量逐条量化并给出结论；顺手补掉 3D `overlap` 缺的校验

- **补掉一条真差异**：`physics3d.overlap` 此前**不校验** `radius`（2D 校验有限性与非负）——同名 API
  不该一个守一个放。本机补上，并在既有的参数校验用例里加了两条断言（NaN / 负半径）。
  这是 §5.2 那 3 对"并非同形"里**唯一不动运行期 API 面**就能补齐的；另两处（`raycast` 返回形状、
  `get_velocity` 数字下标）都要改 API 面，仍阻塞在并发会话的 `api_surface_snapshot`。
- **6 条部分修的余量逐条量化**（写进 `docs/60` §10.6，每条给出精确余量 + 为什么不做）：
  §4.1（35 个平铺字段，收益只是可读性，且文件正被并发会话动）· §5.1（剩 4 段约 58 行，抽 helper
  省 15–20 行，收益/风险比与 §5.3 同量级）· §5.2（见上）· §6.2（4 个 accessor 仍在 clone，但宿主侧
  只剩**非帧**调用点，改 `with_*` 要动公开面）· §7.1（16.9 MB 可变字体，离线绿色版必须自带）·
  §7.2（`cargo audit`/`deny` 本机无法验证）。
- 这样账本里**没有一条留在"说不清为什么没做"的状态**：要么已修，要么有量化的余量 + 明确理由 + 可推翻条件。
- 读数：`rustlua-scripting --lib` **110 通过 0 失败**；fmt / clippy 干净。
- 回滚档：`script/api/physics.rs` + `script/tests/physics.rs` + `docs/60` §10.6 + 本文件。
## Unreleased — §5.2 尾款：`set_velocity` 合成一处，其余三对**不是同形**（实测宏化净 +2 行）

债单说余下 5 对绑定"结构完全相同、约 200 行可消除"。逐行复核后的真相：

- **只有 `set_velocity` 一对真的同形**（都是 `entity_from_lua` → 借 state → `require_alive` → 转发到
  物理世界），已用**分量重复**合成 `bind_set_velocity_pair!`（`(x, y)` / `(x, y, z)` 两个调用点）。
  `api_surface_snapshot` 逐字通过——**运行期 API 面一字未变**，这就是"宏化没改行为"的证据。
- **其余三对并非同形**：`overlap`（2D 校验 `radius`、3D 不校验）、`raycast`（2D 返回
  `(entity, distance)`、3D 只返回实体）、`set_position`（2D 的 `Transform`/`Vec2` 对 3D 的
  `Transform3D`/`Vec3`）。宏只能靠"再加参数"把这些差异盖住——**那是把差异藏起来**，比留两份更危险，
  所以**不做**。这三条差异已逐条写进宏旁的注释与账本。
- **实测收益要说清**：这一对的宏化让 `physics.rs` 从 **436 → 438 行（净 +2）**——省下 28 行调用点，
  宏定义加注释 30 行。所以它的价值是**"2D/3D 不再各修一份"**，不是省行数；债单那句"约 200 行可消除"
  在逐行复核后不成立。
- 写宏时踩到一个 macro_rules 限制并记进注释：参数类型不能写成 `(i64 $(, f32)+)`——那个重复**不含
  元变量**，macro_rules 直接报错；改成不写类型、由 `createFuncion` 的 `A: FromLuaMulti` 约束推断。
- 读数：`rustlua-scripting --lib` **110 通过 0 失败**；fmt / clippy（`--all-targets -- -D warnings`）干净。
- 回滚档：`script/api/physics.rs` + `docs/60` §5.2 + 本文件。
## Unreleased — 关掉那条不对称：世界精灵的丢弃也有正面用例了

上一笔修 `world_sprite.rs` 的静默丢弃时留了个缺口（只有"不回归"，没有正面用例）。这条补上：
在既有的世界精灵画家顺序用例里追加一条**贴图 id 越界**的 draw，断言 `dropped_draws == 1`、
`draw_calls == 2`（其余两条照画）、**像素不变**。

- **反面对照**：摘掉 `world_sprite.rs` 里那行 `record_dropped_draw` → 用例立刻红（`left: 0, right: 1`）。
  这条探针同时确认了此前"还需先确认"的那一点：**世界精灵通道在 `scene3d = None` 时照样跑**。
- 读数：`engine --lib` **167 通过 0 失败**（注：160 → 167 是并发会话在工作树里加的用例，
  本次改动只扩了一条既有用例，没有新增）。fmt / clippy 干净。
- 回滚档：`renderer/tests/world_sprite.rs` + `docs/60` §10.4 + 本文件。
## Unreleased — 世界精灵的丢弃也留痕了；顺带把 9 个"跳过/回退"点逐个定性

主题：**同一类静默失败，换个通道又出现一次**。上一笔修完 2D 叠加通道的 draw 级丢弃记账后，
按"顺着帧路径找静默的东西"把 `gpu_mesh.rs` / `world_sprite.rs` 的 9 个跳过/回退点逐个读完：

- **只有一处是真静默**：`world_sprite.rs` 的 world sprite 绑定组拿不到时裸 `continue`——世界里少一个
  sprite，日志一个字都没有。已按同一条口径修（`record_dropped_draw` + `warn!`，带 texture /
  vertex_start / vertex_count）。
- **一处是"看着像、其实不该算"**：3D 实体的网格不在缓存时也裸 `continue`，但它绝大多数是**异步预热的
  头几帧**（下一帧就有），真正的硬失败已在加载侧留痕（`gpu_mesh.rs` 的 `async asset load failed`）。
  这里改成 `debug!` 而**刻意不计入 `dropped_draws`**——否则正常帧也会非零，把"0 = 这一帧什么都没丢"
  这个含义毁掉。**这条取舍写进了代码注释**。
- **其余 6 处合法**：`gpu_mesh.rs` 的三条失败路径**早就都有 `warn!` + fallback**（不是静默）、两处并发去重；
  `world_sprite.rs` 的"该档无 sprite"（opaque/translucent 分两趟，某一趟为空是正常的）、深度视图选择、
  清屏取舍、**视锥剔除**（剔除是**应该不画**，不是失败）、无主相机时回落到 `explicit` 矩阵（文档化回退）。
- 读数：`engine --lib` **160 通过 0 失败**；fmt / clippy（`--all-targets -- -D warnings`）干净。
- **诚实缺口**：world sprite 那处丢弃**没有正面用例**（要造"贴图 id 越界的 world sprite draw"，
  而世界精灵通道在不同 `scene3d` 下的计划取舍得先确认），本轮只做到"不回归 + 记账已接上"——
  与 2D 叠加通道那处**已有正面用例**不对称，已写进 `docs/60` §10.4。
- 回滚档：`renderer/world_sprite.rs`（两处 + import）+ `docs/60` §10.4 + 本文件。
## Unreleased — GPU 用例的"跳过"不再冒充通过：`gpu_skipped` 标记 + `RUSTLUA_REQUIRE_GPU=1` 硬失败

主题：渲染器的 GPU 用例在没有适配器时是裸 `return; // no GPU available; skip`，而 **`cargo test` 对早退的
用例照样报 `ok`**——于是"engine --lib 160 通过"**可能**包含十几个一行没跑的用例，正是本仓最忌讳的
"跳过当通过"。更麻烦的是：**CI runner 没有 GPU**，这类用例在 CI 上**恒为跳过**，所以渲染器的 GPU 路径
只有本地门能覆盖；而本地门此前同样是静默跳过。

- **修法**：13 处裸跳过（7 个文件）统一走 `gpu_skipped(reason)`（`renderer/tests/mod.rs`）。它做两件事：
  ① 打印 `gpu_skipped reason=...`（配 `--nocapture` 可见、可被门统计）；② **`RUSTLUA_REQUIRE_GPU=1`
  时跳过即 panic**——在"应该有 GPU"的机器与发布门上把"悄悄没跑"变成硬失败。
- **两个探针（当场复现问题本身）**：给一个 GPU 用例插一行无条件跳过 →
  ① 默认跑：`test result: ok. 1 passed` + 打出 `gpu_skipped reason=probe`（**这就是假绿**）；
  ② 同一个用例加 `RUSTLUA_REQUIRE_GPU=1`：**exit 101 panic** ✓。探针已还原。
- **顺带得到一个真读数**：在本机用 `--nocapture` 数标记 = **0**——即这 13 个 GPU 用例在**本机真的都跑了**，
  所以本轮以及先前那些"engine --lib 通过"的读数在本机是**有 GPU 覆盖撑着的**（此前只是假设，现在可验）。
- 读数：`engine --lib` **160 通过 0 失败**；fmt / clippy 干净。
- 回滚档：7 个测试文件（`renderer/tests/*.rs`）+ `docs/34` + 本文件。
## Unreleased — 收尾上一笔：越界的贴图下标不再 panic，改走同一条丢弃记账

- `bind_group_for` 之前是 `paths[texture - 1]` **直接下标**（隔壁 `sdf_bind_for` 一直是
  `paths.get(...)?`），越界**直接 panic**。上一笔刚把 draw 级失败做成"记账 + 告警"，这里就补上同一口径：
  越界返回 `None` → **进 `dropped_draws` 并留告警**，整帧照出（越界只可能来自宿主造 mesh 的内部 bug，
  让它留在读数里比把游戏带走更有用）。
- **反面对照**：把那一行改回直接下标 → 用例立刻 panic
  （`index out of bounds: the len is 1 but the index is 98`）——证明修前确实是崩、这条用例是有效守卫。
- 读数：`engine --lib` **160 通过 0 失败**；fmt / clippy（`--all-targets -- -D warnings`）干净。
- 回滚档：`renderer/gpu_texture.rs` + `renderer/tests/passes.rs`。
## Unreleased — 渲染器"画不出来"不再静默：draw 级丢弃记账 + 根因告警 + 一条真实帧用例

主题：**通道级早有对账，draw 级没有**。`RenderPlan::mismatch` 会告警、`last_frame_passes` 可直接断言，
但 `passes.rs` 里两个 `let Some(bind) = ... else { continue }` **既不计数也不告警**；更上游
`ensure_font_atlas` / `ensure_custom_font` 的返回值还被 `let _ =` 丢掉——**一次字体图集上传失败
= 所有 SDF 文字静默消失，日志里一个字都没有**。而黄金门比的是**布局矩形**，比不出"像素少一块"。

- **修法**：`RenderFrameMetrics` 新增 **`dropped_draws`**（readback 侧早有 `dropped` 的同款先例，
  这里只是把同一把尺补到 draw 级）；两个丢弃分支都记账 + `tracing::warn!`（带 texture / path / layer，
  SDF 分支另带 bitmap 区分 bitmap/MSDF）；字体图集与自定义字体上传失败改为留下**根因**告警。
  **刻意不计入 `draw_calls`**：两者分账才构成"`draw_calls` 偏少 + `dropped_draws > 0`"这个完整证据。
- **防回归**：新增真实离屏帧用例，构造 `sdf = true` 但贴图指向普通资源路径的 draw
  （`sdf_bind_for` **不像** `bind_group_for` 那样回退白贴图，所以这条真的丢得掉）→ 断言
  `dropped_draws == 1` 且 `draw_calls == 0`。
  **反面对照**：只摘掉该分支那一行记账 → 用例立刻红（`left: 0, right: 1`）。这条探针同时排除了
  **"无 GPU 静默跳过"的假绿**——该用例真建了设备（耗时 2s+，而跳过路径是 0s），所以红了就是真红了。
- **读数**：`engine --lib` **160 通过 0 失败**（159 → 160）；fmt 与 clippy（`--all-targets -- -D warnings`）干净。
- **顺手记一条未修的相邻问题**（写进 `docs/60` §10.4）：`gpu_texture.rs` 的 `bind_group_for` 用
  `paths[texture - 1]` **直接下标**（隔壁 `sdf_bind_for` 用的是 `paths.get(...)?`），越界会 panic。
  越界只可能来自宿主内部 bug（Lua 触不到），"快速失败"可辩护——但它与本条"失败要留痕"的口径不一致，
  列此待定而不是顺手改掉。
- **回滚档**：三个文件（`renderer/mod.rs`、`renderer/passes.rs`、`renderer/tests/passes.rs`）+ `docs/60` §10.4。
## Unreleased — 微信主包被 4 MB 门挡下：wasm 内核搬进 pack/ 分包，boot 改为等挂载

主题：**第一次被微信体积门真拦住的修法**。搬迁前实测 `main=4,547KB main_of_limit=111.0%`（超
451 KB），而"收窄首屏清单"那条直觉修法方向是反的——首屏清单是把资源**从分包挪进主包**的那张表。
本轮按"能整块搬走、又不需要重写路由"的唯一候选动手：内核 `game_web_bg.wasm`（实测 2,580,513 字节
= 2,520 KB，占主包 55%）搬进 `pack/`，胶水 `pkg/game_web.js` **留在主包**（它由同步 `require()`
加载，而 `WXWebAssembly` 只有异步 `instantiate(路径, imports)`，"搬胶水"要换一套 require 语义）。

- **启动次序随之反转**：原来是"标题画面不等分包"，现在是 `start()` 先 `await` 分包挂载、挂载成功
  才实例化内核、然后才出标题。等待期间进度条由**适配层自己**用 `setTimeout` 链驱动并自绘——不能用
  `wx.requestAnimationFrame`（那条队列属于还没起跑的运行时，harness 的 id 与槽位又对不上，实测把
  游戏的**第一帧回调**取消掉，症状是 `boot=booted / 未执行任何帧 / raf 队列为空`）；也不能调
  `afterFrame`（它推 `frameCount`，而 `frameCount` 正是被测的挂载帧读数——等于让被测代码改判据）。
- **失败不再静默**：`ensurePackForBoot` 在实例化前自动重试 1 次（每次尝试自带 20 s 无回调超时），
  仍失败才判 boot 失败，屏上那句话点名是哪个包（"游戏内核就在这个分包里"）。
- **门**（四条，每条都跑过反面对照证明它会红）：① 分包形态下 `pkg/game_web_bg.wasm` 存在即拒绝
  发布（`package.ps1` + 验收脚本各一条；两份同名内核迟早漂移，所以是"拒绝"而不是"取其一"）；
  ② harness 证据行新增 `wasm_denied_before_mount`，成功用例要求它为 0——把 `await` 摘掉立刻红
  （"分包挂载前有 2 次 wasm 内核读取被挡下"）；③ 失败用例下 wasm 的失败原因必须**点名**
  `pack/game_web_bg.wasm`，只判 `status!=ok` 会把无关失败洗成"预期失败"；④ `--subpackage delay`
  下适配层挂载帧改判"必须为 0 游戏帧"（宿主记的是驱动 tick，两者币种不同，硬相等是假红）。
  另补 `pumpSubpackageDuringBoot`：boot 悬着那段由它按 16 ms 一 tick 顶上挂载，否则 `delay` 用例
  结构死锁（挂载等帧、帧等 boot、boot 等挂载，实测 30 s 不 settle 后撞 20 s 超时）。
- **读数**：`wechat_minigame_static_ok main=2,087KB subpackage=4,689KB total=6,776KB
  main_of_limit=51.0% subpackage_of_limit=22.9% total_of_limit=33.1%`（复现：
  `package.ps1 -Platform wechat-minigame -SkipBuild` + `verify-wechat-minigame.ps1 -SkipRuntime`）。
  分包用例矩阵 `delay / delay-move / never / fail-once` **exit 0**，`ok / no-game-json` 各剩一条与本批
  无关的既有红（见下）。形态判定全部只看产物本身：`bundle-discovery.ps1` / `verify-*.ps1` 的内核
  路径按 `mainpack.json` 是否存在切换；启动日志实测 `wasm=ok:ok:path-instantiate` +
  `核心实例化成功（WXWebAssembly/path-instantiate，路径 pack/game_web_bg.wasm…）`。
- **本机证明不了的**（写清楚，别当已验证）：单文件 2.5 MB 上传上限（内核现在是 2,520 KB 的**单个
  文件**，只有上传器/工具面板能给数）、真机 `WXWebAssembly.instantiate` 的分包路径语义、重试的
  实际救回概率、以及形态选择的必然代价——冷下载时玩家第一眼是进度条而不是标题。
- **未闭合（阻塞整条微信运行时门）**：`data-rustlua-drawn=0` 这一条红停在通用断言上，**不属于本批**：
  同一产物加 `--drive-new-game` 进 `manor` 后 `drawn=4720`（取样机制在跑，只有**标题屏**采不到
  非背景像素）；用 `git show HEAD` 的适配层 / harness 分别回退到旧形态复跑，读数一样是 0；启动
  日志无任何贴图或读取失败警告。指向标题门面进 `.prefab` 之后的渲染差异，归 prefab 那摊。

## Unreleased — §6.3 收口（3D 补上查询脏标记）+ §5.2 的前提经复核不成立

主题：**两条"照抄一遍"的量级，其中一条抄起来发现前提是错的。**

- **§6.3：3D 补上 2D 早就有的 `query_dirty`**。原状是 `add_body` / `remove_body` 每次都立即
  `query_pipeline.update(...)`——场景加载时 N 个物体就是 N 次全量重建。现在与 2D 同构：增删只**置脏**，
  查询前由 `refresh_queries()` 兑现，`step` 之后清标记（Rapier 在内部重建）。
  **不是纯照抄**：3D 的四个查询方法原本是 `&self`（2D 是 `&mut self`），兑现脏标记必须可变，所以签名
  一起改了，下游 2 处 Lua 绑定（3D 的 `raycast` / `overlap`）随之改取可变借用——顺手把 2D 那句
  "查询可能要兑现延迟的 BVH 重建，故取可变借用"的注释也补到了 3D 侧。
  **防回归**：新增白盒用例断言三条契约（增删只置脏 / 查询兑现 / step 也算兑现）。
  **反面对照**：只把 `raycast_filtered` 里那一行 `refresh_queries()` 去掉 → **三条用例同时红**
  （新用例 + 既有的 `raycast_and_overlap_queries_name_entities` + 奇偶校验的
  `physics_parity::queries_hit_colliders_and_respect_groups_in_both_worlds`）——契约是三重覆盖的。
- **§5.2：账上说余下 5 对绑定"结构完全相同"，逐对读完只有 3 对同形**（`set_velocity` / `overlap` /
  `raycast`）。另外 2 对（`set_position` / `get_velocity`）**不是重复，而是"有一边漏了"**：2D 拦 NaN/Inf
  并同步 ECS `Transform`、`get_velocity` 还给数字下标 `[1]/[2]`——3D 三样都没有。于是同一件事"把 NaN
  喂给 Rapier"只有 2D 守，而且 `physics3d.set_position` 之后渲染/Lua 读到的位置会与刚体不一致直到
  下一次 step。本轮补了**校验 + ECS `Transform3D` 同步**（不改 API 形状）；**数字下标未补**——那会改
  运行期 API 面，而 `api_surface_snapshot/*` 正被并发会话改着，不该在此时动它。宏化留给下一轮
  （现在 4 对已同形，可一次折进宏）。
- **读数**：`rustlua-physics` **24 通过 0 失败**；`rustlua-scripting` **110 通过**（109 → 110）；
  `engine --lib` **159 通过**；fmt（仅本次改过的文件）与 clippy（`--all-targets -- -D warnings`）干净；
  **五个 prefab 门 + `game --check` 全通过**。
- **账本**：§6.3 由 ◐ 升为 ✅ → **13 已修 / 6 部分修 / 1 已评估决定不做**；§5.2 的**前提被更正**
  （当初记账时把 2 对行为缺口误标成了"结构重复"）。
- **诚实边界**：§5.2 补的两项里，**ECS 同步那一半没有专门测试**——它需要带 `Collider3d` 的场景实体，
  本次只测了参数校验那一半（不需要真造 3D 刚体，校验发生在 `require_alive` 之前），
  所以用例注释里写明了"另一半留给黄金门与既有 3D 物理测试覆盖"。

## Unreleased — §5.3 绑定样板：实测后决定不做（4.6% 行数，代价是掉出格式门 + 失去可审计性）

主题：债单里最后一项"该不该做"的**评估结论**。清单原本建议引入 `lua_fn!` 宏消除
`let s = Rc::clone(&state);` 样板，**预估减少 20–25% 代码量**。实测下来这个数字高了 2.5–5 倍，而代价落在
本仓最在意的两件事上，所以**决定不做**——并把数据、理由与"什么条件下要重算"一起留档。

- **实测**：`api/` 合计 **5,638 行**，`let s = Rc::clone(&state);` 占 **260 行 = 4.6%**；连
  `lua.create_function(...)` 的外壳一起折进宏，上限也只有 **~9%**。
- **不做的最硬理由**：**宏体不再被 rustfmt 覆盖**（本仓没有 `rustfmt.toml`，默认行为就是不格式化宏调用体），
  于是几百行绑定会**掉出 `cargo fmt --check` 这道门的覆盖范围**。在一个以"门不说谎"为身份的仓库里，
  主动把代码移出格式门是净损失。
- **第二条理由**：逐处的借用与绑定形态不再可 grep——而 **§4.3 那 104 处审计正是靠"每处借用都写在明面上"
  才做得完的**（连 `subs.borrow_mut()` 被我误伤都能当场发现并撤回）。用可审计性换 4.6% 行数不划算。
- **硬约束**：`'static` 闭包必须把 `Rc` 的所有权移进去，所以这行样板**消不掉**——与 §4.3 不同，那里有
  `try_borrow` 这个现成替代，这里只剩"用宏藏起来"或"留着"两个选项。
- **可推翻条件**（已写进账本）：若将来这些绑定不再需要 `'static`（例如换掉 mlua 的回调生命周期约束），
  或本仓引入 `rustfmt.toml` 并启用 `format_macro_bodies`，就用同一把尺（行数占比 + 是否掉出格式门）重算。
- **回滚档**：纯文档（`docs/60` §5.3 行与 §10.1/§10.2）。**没有代码改动**——这是一条决策记录，不是一次修复。

## Unreleased — §4.3 收口：104 处 Lua API 借用全部改成可捕获错误，重入不再 panic（wasm 上不再硬 abort）

主题：**债单里剩下的那条实质未收口**。`lua_borrow(_mut)` 这套"把 RefCell panic 降级为 Lua 可捕获错误"的
机制**早就写好了**，却只转了一部分——而 `mod.rs` 的注释写着"Lua API 闭包**统一**用 `try_borrow(_mut)`"。
先改注释（改成实测口径），再把差的补上：**两个方向一起转**，因为只转外层等于没转（外层转了、内层没转，
内层照样 panic）。

- **为什么值钱**：wasm32 上 `panic = abort`，重入触发的是**硬 abort**（浏览器/微信里表现为裸 `unreachable`，
  连行号都拿不到），而这条路径**已知可达**——机制本身就是因为 mlua 会经 `__index` 等元方法重入 Rust 才建的。
- **做法**：`s.borrow_mut()` → `s.lua_borrow_mut()?`、`s.borrow()` → `s.lua_borrow()?`。18 个 `api/*.rs` 本来
  就都 `use crate::script::LuaStateBorrow;`，所以是纯机械改动；编译器把两类特例指了出来：① 11 处 `?` 打断了
  deref coercion（`&s.lua_borrow()?` → `&*s.lua_borrow()?`）；② 5 处**我自己脚本的子串替换误伤**——
  `subs.borrow_mut()` 里含 `s.borrow_mut()`，被改成了 `subs.lua_borrow_mut()?`，已撤回（批量替换必须带词边界）。
- **账目更正**：原记的"109 处"里有 5 处其实是 `subs.borrow*()`（**另一个 RefCell**，与 state 重入无关），
  是被 `s.` 的子串匹配算进来的。真实要转的是 **104 处**；转换后全仓 `lua_borrow*` 调用点 **277 = 173 + 104** ✓ 对得上。
- **验收：两条方向相反的用例**（新增 `script/tests/reentrancy.rs`）：① 持住 `state.borrow_mut()` 再调 Lua API，
  必须收到带 `already borrowed` 的 **Lua error**；② 非重入路径必须照常工作（少了 ②，"把所有 API 改成永远报错"
  也能让 ① 变绿）。**反面对照**：把 `health.get` 一处改回 `s.borrow()` → ① 立刻在 `api/entity.rs:110` **panic**
  （正是改造前的行为），② 仍绿。
- **残留的 70 处 panic 版 state 借用不改，理由写进账本**：它们全在 `impl LuaRuntime` 的 `&self` / `&mut self`
  方法里，是宿主侧的**外层**借用（重入的**来源**，不是受害者），且**从 Lua 回调里不可达**——这是结构性的：
  API 闭包只捕获 `Rc<RefCell<ScriptState>>`，拿不到 `&LuaRuntime`（`api/*.rs` 里 `runtime.` 出现 0 次）；
  那 4 处 `LuaRuntime::` 全是**关联函数**调用，收的是已借出的 `&mut ScriptState`。
  `require.rs` / `values.rs` / `guard.rs` / `error.rs` 一个借用都不持有（实测）。
- **读数**：`rustlua-scripting` **109 通过 0 失败**（107 → 109）；`engine --lib` **159 通过**；fmt（仅本次改过的
  文件）与 clippy `--all-targets -D warnings` 干净；**五个 prefab 门 + `game --check` 全通过**
  （`jhyt_m59/m61/m62/m63/m65_*_ok` + `game_check_ok`）。
- **回滚档**：17 个文件（12 个 `api/*.rs` + `script/mod.rs` + 4 个既有 tests + 新增 `tests/reentrancy.rs`）
  \+ `docs/60` §4.3/§10 的状态与账目更正。
- **诚实边界**：**没做逐调用点的可达性证明**，只有上面那条结构论证；**没跑 wasm 目标**（本机不做那种重构建），
  所以"panic → Lua error"是在 native 测试里验的，wasm 上的收益按"panic 即 abort"推。另外**没裸跑 `cargo fmt`**
  （那会碰到并发会话正在改的 11 个编辑器文件），只对本次改过的文件跑了 rustfmt。

## Unreleased — `prefab_ui_nodes` 每帧整表 clone 收掉：宿主改成先比修订号

主题：**债单复核当天修掉的第二条**（[docs/60](docs/60-全面复盘与优化项清单.md) §10.3 第 2 条）。
`sync_prefab_ui_nodes` 的写法是 `let spawned = self.script.prefab_ui_nodes();` 之后才
`if spawned == self.applied_prefab_ui_nodes`——**克隆发生在比较之前**，所以"没变就早退"只在分配
已经付过之后才成立。UI 预设化之后这张表是非空的（每个面板的布局节点都在里头），于是那份整表拷贝
加一次 O(n) 比较**每帧都付**。

- **修法**：把表与**修订号**封进 `PrefabUiNodes`（`script/mod.rs`），改动只能经 `clear` / `remove` /
  `extend` 三个方法、修订号在方法里自增；宿主侧改成**先比修订号**（新增 `applied_prefab_ui_revision`），
  未变即早退——**一次 clone 与一次 O(n) 比较都不做**。
- **修订号不做"内容没变就不加"的优化**：多一次同步只多付一次合并，漏一次却会让画面与命中框停在旧版上
  ——两者代价不对称，所以宁多勿漏。这条取舍写在类型注释里，不靠读者猜。
- **顺带改正三处"注释比代码承诺得多"**（本会话找到的第三、四、五处）：`mesh_ui.rs` 的"没变时…不产生
  分配"、`runtime_scene/mod.rs` 的"没变就一个分配都不做"、以及 `prefab_ui_nodes()` 的"空表 = 不付额外
  拷贝成本"——三句在实现上都是假的，因为克隆发生在比较之前。
- **防回归**：`script/tests/scene_spawn.rs` 新增
  `prefab_ui_nodes_revision_bumps_on_every_mutation_and_only_then`——三个改动入口各加一次，
  而**只读快照不加**（后者若加，宿主每帧都误判"变了"，正好退回被修掉的那条）。
- **读数**：`rustlua-scripting` **107 通过 0 失败**（106 → 107）；`engine --lib` **159 通过**；两个 crate 的
  fmt 与 clippy（`--all-targets -- -D warnings`）干净；**五个已入库的 prefab 门复跑全通过**
  （`jhyt_m59_ui_prefab` / `jhyt_m61_prefab_runtime` / `jhyt_m62_prefab_write` / `jhyt_m63_title_prefab` /
  `jhyt_m65_shop_prefab`）——它们走的正是 `prefab.spawn` → 布局并表这条链。
- **回滚档**：六个文件（`script/mod.rs`、`script/tests/scene_spawn.rs`、`host/runtime_scene/mesh_ui.rs`、
  `host/runtime_scene/mod.rs`、`host/runtime_scene/scene.rs`）+ `docs/60` §6.2/§10.3 的状态更新。
- **诚实边界**：**没实测帧耗时**；宿主侧不为此加计数探针（跨 crate 计数做不到——`cfg(test)` 不跨 crate），
  所以端到端那一层由上面五个黄金门兜，而不是合成用例。

## Unreleased — 修掉 pre-commit 里 clippy 归属判断的假阴性：暂存文件自己 clippy 红过不了门

主题：**上一批自己引入的缺陷**——`captured_names_staged_file` 拿 git 的**正斜杠**暂存路径
（`crates/a/b.rs`）去比 cargo 在 Windows 上输出的**反斜杠**诊断路径（`crates\a\b.rs`），**永远比不上**。
后果不是"偶尔漏判"，而是 **clippy 这一半从没拦过任何东西**：任何 clippy 失败都被归成"提交之外"，
门打印一句"clippy 判红，但诊断里没有出现本次提交的文件"然后放行。上一批改的正是这个归属判断，
而**修错了对象比不修更危险**——它会让人以为 clippy 在守门。

- **修法**：比对前先 `tr '\\' '/'` 归一化诊断输出。一行，但它决定"clippy 算不算我的"。
- **反面对照（本条的价值在这里）**：新用例 **B11** 先暂存一份自己带告警的 `lib.rs`、再断言提交
  **必须**判红。修之前跑：**红**（`本次提交自己的 clippy 报错必须判红` 与 `判红后 HEAD 未移动`
  两条失败——即那次提交真的被放行了）；修之后跑：**绿**。用例里还特意 `git clean -fd` 清掉前面几例
  留下的未跟踪 `.rs`——否则它们会被 `add --all` 一起暂存，红在 fmt 而不是 clippy，**用例会空转**
  （这个坑本次踩过一次）。
- **读数**：`scripts/test-git-hooks.ps1` **61 例全绿**（58 → 61）；`sh -n git-hooks/pre-commit` 通过；
  hook 仍是 LF 无 BOM。
- **回滚档**：`git-hooks/pre-commit`（一处归一化）+ `scripts/test-git-hooks.ps1`（B11）+ `docs/34` 计数。
- **诚实边界**：B11 用 `let unused = 1;` 触发的是 rustc 的 `unused_variables`（`-D warnings` 下升级为
  错误），**没有单独构造 clippy 专有 lint**——所以这条用例证明的是"诊断路径能被认领"，不是"clippy 的
  每条 lint 都能被认领"。两者共用同一套路径输出，结论可迁移，但口径要说清。

## Unreleased — 实体名反查从「线性扫全表」改成 O(1)：宿主每帧那条 O(n²) 没了

主题：**债单复核当天就发现并修掉的第一条**（[docs/60](docs/60-全面复盘与优化项清单.md) §10.3 第 1 条）。
`entity_name(id)` 的实现是 `by_name.iter().find(...)`——O(n) 线性扫，而宿主在**遍历实体的循环里**调它
（`host/runtime_scene/mesh_ui.rs` 每个实体一次），叠加 `is_entity_scene_text_hidden` 对每个文字扫同一张表：
实体数 n 时**每帧 O(n²)**。这不是清单里的项，是复核顺着热路径读出来的新发现。

- **修法不是"再加一张反向表"，而是把两个方向合成一个类型**：`EntityNames`（`script/mod.rs`）里正向
  `by_name` 是唯一真源、也是 `entity_names_by_slot` 这类**确定性输出**所依赖的有序性来源；反向 `by_entity`
  是**惰性派生 + 脏标记**，与 `rustlua-ecs` 的 `TypedStorage::ensure_order` 同一惯用法。两张表各自维护
  必然漂移（而漂移的后果是名字张冠李戴，比慢一点严重得多），所以改名的入口只有该类型的方法，脏标记在
  方法里置位——**没有"忘了同步反向表"这种可能**。
- **语义逐字保留**：`entity_name` 在「同一实体挂多个名字」时取 `by_name` 键序里的第一个（与旧
  `iter().find` 同结果）；`insert` / `remove_entity` 的既有契约（同名改指向、旧名字仍指向同一实体）一字未动。
  两个 getter 仍走 `&self` + 内部可变性，**没有给 Lua 重入多开一个可变借用的口子**（这在正要收口的 §4.3 上
  尤其要紧）。
- **防回归：两条方向相反的断言**（`script/tests/entity.rs`）：① 改动必须让缓存失效（否则反查返回过期名字）；
  ② 稳态 64 次反查**一次都不许重建**（每次重建正是被修掉的 O(n²)）。**两条都跑了反面对照**：去掉置脏 →
  ① 红（`left: []` vs `right: ["foe", "hero"]`）；改成每次都重建 → ② 红（`left: 2` vs `right: 1`）。
  探针已回退（`grep 'RC 探针'` = 0）。
- **读数**：`cargo test -p rustlua-scripting --lib` **106 通过 0 失败**（105 → 106）；`cargo test -p engine --lib`
  **159 通过**（宿主就是每帧调这两个方法的那一侧）；`cargo fmt -p rustlua-scripting -- --check` 干净；
  `cargo clippy -p rustlua-scripting --all-targets -- -D warnings` 零告警；两个黄金门
  `jhyt_m63_title_prefab_ok` / `jhyt_m65_shop_prefab_ok` 复跑通过。
- **回滚档**：四个文件（`script/mod.rs`、`script/api/entity.rs`、`script/scene_spawn.rs`、`script/tests/entity.rs`）
  \+ `docs/60` §6.1/§10 的状态更新；无格式 / 数据迁移。
- **诚实边界**：**没有实测帧耗时**——"O(n²) → O(n)"是按**调用点与数据结构**推的，不是量出来的；反查缓存的
  收益随实体数增长，小场景本来也看不出来。`editor --check` 本次**跑不起来**：工作树里编辑器当前编译不过
  （5 个错误全在并发会话正在改的 `panels/resource_browser.rs` 与 `util.rs`），且 `editor.exe` 被一个在跑的
  编辑器进程占着——**与本批无关**（错误位置逐一核过，本批文件命中 0 次）。
- **账本同步**：`docs/60` §6.1 由 ◐ 升为 ✅，总数变 **11 已修 / 8 部分修 / 1 仍存在**（复核当时的 10/9/1 是
  提交 `d305d17` 的时点读数，保留不改——**每个数字都带时点**，这是本批"对齐自述"的同一原则）。

## Unreleased — web/微信宿主补上 `__white__` 内建白贴图分支

主题：**`texture=__white__` 在 native 是内建 1×1 白纹理、在 web 宿主曾被当成文件路径去读**，于是纯色块
（prefab 的面板、`npcs.lua` 的 tint 灰盒）在微信上一律读盘失败。这不是体积问题也不是渲染细节：
它是**跨宿主语义缺口**，而且只有 web 门能抓到——native 渲染器在
`engine/src/host/renderer/gpu_texture.rs:110` 就地造白纹理（`frame.rs:173` 的 `WHITE_TEXTURE_PATH`），
从不查盘；`plugins/webgl/web-runtime.js` 里此前**一个 `white` 字都没有**。
实测代价：10-06 那份含 prefab 载体的包，240 帧内 **234 张贴图读取失败**（样本全是 `pack/__white__`），
整条微信运行时用例红——而 `ensureTexture` 不缓存失败，所以一个缺失路径**每帧重试一次**，
3 个色块就够把断言刷成一堵墙。

- **改了什么**：① `loadImage` 认哨兵并直接返回 null（绝不进加载器）；② 场景 UI 的 `Image`
  走 `fillRect` + `color` 四段（第四段就是 alpha，M64a 那套顶点色的 canvas 等价写法）；③ 世界实体的
  `Sprite` 走 `fillRect` + 新解析的 `tint`（原生 `Sprite.tint: [f32;4]`，缺省 `[1,1,1,1]`），
  **不再退成"没贴图就画圆点"**——圆点是按实体名猜色的兜底，不是白图的语义。
- **红→绿的 A/B 是同包单文件差分**，命令逐字符相同：
  `node scripts/wechat-minigame-harness.js <包> --drive-new-game --expect-runtime-spawns --check-save-persist --frames 240`
  → 改前 234 张贴图失败 / exit 1；改后 **`wechat_minigame_harness_ok`、`save_persist="ok,probe=518,game=18718"`、
  `draw_ops` 247,411→247,576**（多出来的就是纯色块）。
- **对照组证明零回归**：同一份改后运行时放进 10-05 的包，读数与它自己的基线**逐字符相同**
  （`draw_ops=247411 images=117 save_persist="ok,probe=518,game=15074"`）。
- **反面对照证明守卫有牙**：只把发布件 `web-runtime.js` 里的哨兵字面量改成 `__white__NOT__`（其余字节不动），
  234 张失败立刻回来、exit 1。
- **验的就是要发出去的那份字节**：`git show HEAD:plugins/webgl/web-runtime.js` 经**同一条 terser 命令**
  （`terser <in> -o <out>`，binaryen 侧同口径）产出 152,334 B，与发布件 `cmp` 逐字节相同——
  所以"手工替换发布件里那一份"做的验证等价于流水线产物；改后 152,864 B（**+530 B**，主包口径内可忽略）。
- **真浏览器侧另走一遍流水线**：`package.ps1 -Platform webgl -SkipBuild` 出包（webgl 没有 4 MB 门）后
  `verify-webgl-bundle.ps1 -Port 8141 -Drive` → `boot=running missing_host_api=0`、`drive: done`、
  `scene_verify: done`、`webgl_bundle_ok`。
- **单点取证（假 ctx 记录绘制序列，期望值独立算）**：`__white__`+`color=0.08,0.09,0.12,0.62` →
  恰好一条 `fillRect rgba(20,23,31,0.62)`；三段 `color=` → `rgba(255,255,255,1)`（缺第四段不能变透明）；
  真贴图路径仍 `drawImage` 且 `globalAlpha=0.5` 未被吃掉；`loadImage('__white__')` 返回 null 且
  **一个 `Image` 都不构造**，`loadImage('a.png')` 照样构造。活体侧也对上一次：
  `npcs.lua` 的 `tint=0.30,0.50,0.66,1.0` 真的以 `rgba(77,128,168,255)` 落进了假 ctx 的 fillRect 记录。
- **有意留下的缺口（不是漏了）**：动态 `ui.image` 的 `'I'` 记录那条路径没加分支——今天**没有任何内容**
  往里传 `__white__`，而它的色只有 r/g/b 三段、与 prefab 的四段口径不同，硬套会引入一处没人验证过的语义。
  代价要说清：一旦哪天有内容那么写，它会**静默不画**，而且 harness 的贴图失败断言抓不到（根本不发请求）。
  纹理×tint 的乘色在 web 上依旧没有（canvas 2D 没有 tint），本轮不动。
- **同一份包上的另一条红，与本条无关**：`--bag` 在 10-06 的内容上是
  "背包面板没有渲染（`ui-summary` 无 `bp:title`）"及其连带的 DPad 高亮失败——那是 prefab 迁移自己的断链，
  归 prefab 那摊；本条改的只是宿主对白图的处理。
- **回滚档**：`plugins/webgl/web-runtime.js` 单文件 4 处、+37/−6，`git revert` 即可；无数据迁移、无 API 变化。


## Unreleased — 债单复核：20 项里 10 已修 / 9 部分修 / 1 仍存在，另发现两条原清单没有的逐帧缺陷

主题：**docs/60 那份 20 项的复盘清单，自己变成了误导源**。它的 **16 项文件路径已失效**（引擎拆成 14 个
`rustlua-*` crate，`host/renderer.rs` 与 `host/runtime_scene.rs` 被 WP12 拆成目录），而"这一项到底修没修"
文档里没有答案。代价是实测过的：本轮的两次分析都从它出发得出过错误前提（差一点把两个**已被删除**的文件
当成最大结构债推荐；也差点按它的旧路径去找代码）。本次逐项回到**当前代码**找证据，把清单从"待办列表"
变成"已清偿的账 + 一条明确的尾巴"。

- **复核结果（2026-10-06）**：**10 项已修 / 9 项部分修 / 1 项仍存在**，三个 P0 全部修完。每条给了现行的
  file:line 证据，并**尽量配符号名**（函数 / 字段 / 宏名）——行号会随提交漂移，符号名不会。
- **两条实质未收口**：§4.3 重入借用（机制早就写好了，但 `lua_borrow*` 只转了 **173** 处，仍剩 **63 处
  `borrow_mut()` + 46 处 `borrow()`**；wasm 上 panic 是硬 abort，而这条路径是已知可达的）与 §5.3
  `Rc::clone(&state)` **277 处**（比清单当时的 ~150 还涨了，`lua_fn!` 未引入）。两者是**同一批文件、同一类
  机械改动**，建议一次做，一次编译验证覆盖两件。
- **本次新发现两条原清单没有、也从未被后续批次修掉的逐帧缺陷**（都在帧路径上、随实体数增长）：
  ① `names` 是 `BTreeMap<String, EntityId>`，而反向查找 `entity_name` 用的是 `iter().find`，被
  `host/runtime_scene/mesh_ui.rs:68` 在**实体循环里**调用 → **每帧 O(n²)**；
  ② `mesh_ui.rs:147` 的 `prefab_ui_nodes()` 每帧把整张表 clone 出来，只为做一次 `==`。
  这两条比清单里剩下的多数条目更值得先动。
- **顺带修正一处"注释比代码承诺得多"**（本仓库最在意的那一类）：`rustlua-scripting/src/script/mod.rs`
  的重入借用注释原文写"Lua API 闭包**统一**用 `try_borrow(_mut)`"，实测只做到 61%。已改成实测口径并写明
  收口方向——**过期的说法放在代码里，比放在文档里更危险**。
- **同时对齐另三处会误导的自述**：`docs/62`（"最大文件"表列的是两个**已被 WP12 删除**的文件而 §6 仍在建议
  "先拆它们"；Lua 行数把 `examples/**/builds/` 的生成物算进源码规模，高估手写量 2.3 倍；测试 / 提交 /
  API 面计数过期）、`docs/README`（`tasks/done` 146 → **158**；"`tasks/plan/` 当前为空" → 实际有 M65–M71
  的在途工作包）、`README`（"当前状态（**M32**）"落后 38 个里程碑 → 改成**只写可一条命令复核的事实或指向
  权威来源**，并把 `-Quick` 与 pre-commit 作用域两条新契约写进质量门表）。
- **回滚档**：单次提交，六个文件（四个 `.md` + 一处 `.rs` 注释），无格式 / 数据迁移。
- **诚实边界**：**没跑 benchmark**（release 构建 + 压力样例与当前机器的内存余量冲突），所以"性能收益"那一栏
  只核到**代价结构变没变**（是否还在帧路径上分配、是否还把 O(n) 放进 O(n) 循环），**没核绝对耗时**；
  10.3 那两条"每帧"来自**调用点位置**而非实测帧耗时；§6.2 四个 clone 的宿主调用点是否真不在帧路径上、
  §6.1 是否还有别的映射在帧路径上——**本次没核完，已在 docs/60 §10.4 逐条写明**。

## Unreleased — pre-commit 量错了对象：从「整棵工作树」改成「本次提交」

主题：**本地门禁在检查工作树，CI 在检查提交**。CI 的 lint job 先 checkout 那个 commit 再看内容；而
`git-hooks/pre-commit` 跑的是 `cargo fmt --all -- --check` / `cargo clippy --workspace`——看的是**整棵
工作树**。单人独占一棵树时两者几乎等价，所以这个错位藏了很久；但本仓的实际工作方式是**多会话共享一棵
树**，于是它变成了假红工厂：另一个会话的在途/未跟踪文件（`engine/crates/game/examples/jhyt_m6x_*.rs`）
格式不合格，**我的**提交就被挡住——而 CI 对我这个提交是绿的。假红的代价不是"多等一会儿"，而是训练所有人
去按 `RUSTLUA_SKIP_PRECOMMIT=1`；门禁一旦被习惯性绕过就名存实亡，而"没人跑得起门"正是缺陷能在 HEAD 里
活过多个提交的土壤（docs/34 §6 记过一次同款事故「门禁波动（并行会话）」，当时的处置只是记录在案）。

- **判据改成"失败的文件在不在本次提交里"**：fmt 用 `capture()` 捕获输出，把 `Diff in <路径>:行:` 归一化成
  仓库相对路径（`tr '\\' '/'` + 去掉 UNC 前缀 + 去仓库根前缀），再与 `git diff --cached --name-only` 的
  暂存集比对，同一个文件的多行 hunk 去重后点名；clippy 因为编译天生是整仓的、没法只看暂存内容，改成
  "诊断里有没有出现本次提交的文件"。命中 → 照旧判红；只在提交之外 → 逐条点名 + `PARTIAL` 措辞，
  **不冒充"通过"，也不越权替别人的在途活儿判红**。
- **开关 `rustlua.scope`（默认 `commit`）**：`worktree` 恢复旧口径（工作树里任何一处失败都拦提交）。
  非法值显式报警并回落 `commit`，不静默降级——与既有 `rustlua.clippy` 档位同口径。`install-git-hooks.ps1
  -Status` 一并报出该档位：没人知道存在的开关等于不存在，体检仍严格只读。
- **真机端到端（不是只在临时仓库里过）**：在**真实仓库**上用临时索引 + `GIT_CONFIG_*` 覆盖跑真 hook
  （真索引与仓库配置一个字节没动）。工作树此刻有 6 个属于并发会话的未跟踪未格式化文件、暂存的是我自己
  格式合格的改动：`commit` 口径 **exit 0** 并把那 6 个文件逐条点名进"不在本次提交里"；同一次暂存换
  `worktree` 口径 **exit 1** 拦住。假红消除与旧口径可恢复，都是实测读数。
- **反面对照四条（`test-git-hooks.ps1` §B/§A，46 → 58 例）**：B7 提交之外的未格式化 `.rs` **不拦**且断言
  提交真的完成；B8 同一场景 `scope=worktree` **重新拦住**；B9 同时摆一份提交之外的失败，暂存区自己的未
  格式化 `.rs` **仍然判红并点名文件**（证明分类器在分辨，而不是"一有失败就放行"）；A7 非法 `scope` 值
  报警并回落。B7 的失败样本特意放在 `examples/` 而不是 `src/scratch.rs`——`cargo fmt --all` 只看**目标
  的**文件，摆在 `src/` 下但没被 `lib.rs` 引用的 `.rs` 根本不会被检查，那会让用例空转。
- **顺带两处**：删掉因此变成死代码的 `step()`（fmt/clippy 都改走 `capture()`）；新增工具前置（0a 段）——
  缺 `grep/sed/tr` 时 hook 会**静默退化**成"没碰 Rust 就跳过"（实测：直接 `sh git-hooks/pre-commit` 时
  grep 不在，两次运行都以"跳过"通过），所以判红而不是放行。
- **B10 的诚实边界**：想构造"缺 grep"的用例时发现**构造不出来**——实测 git 执行 hook 时会把自带
  `/usr/bin` 放进 PATH 头部（探针读到 `/usr/bin/grep`）。所以 0a 那段是**纵深防御**（只在手工调 `sh` 时
  可达），B10 改成钉住这条前提本身：哪天 git 不再自带这些工具，B10 会先红并指向真正的原因。
- **回滚档**：`git-hooks/pre-commit`、`scripts/test-git-hooks.ps1`、`scripts/install-git-hooks.ps1`、
  `docs/34` §3.5。`git config --unset rustlua.scope` 回默认，`= worktree` 回旧口径，无需改代码。
- **验证**：`test-git-hooks.ps1` **58 例全绿**；`sh -n git-hooks/pre-commit` 通过；hook 仍 LF 无 BOM；
  `check-docs.ps1` 的 `scripts/*.ps1` BOM 与解析门通过（30/30）。

## Unreleased — `for_each_mut` 补上漏掉的脏守卫：稳态帧不再每帧白排一次 O(n log n)

主题：**同一个惰性重建设计写了两遍，其中一遍忘了判断**。`TypedStorage` 的有序 id 索引契约写得很清楚
（`rustlua-ecs/src/world.rs` 的 `order` 字段注释）：插入/删除只**标脏**，下一次有序访问才整体重排，
「稳态帧（无增删）因此零分配、零哈希」。但两条借用路径只有 `&self` 那一半遵守它——`ensure_order`
先看脏标记，而 `&mut self` 的 `for_each_mut` 调的是**无条件**的 `rebuild_order`，于是稳态帧每帧做一次
O(n log n) 排序，正好违反它自己写下的那句话。生产调用点两个，都在每帧路径上
（`host/runtime_scene/schedule.rs:258,299` 的 AnimationStateMachine 与 Animation3D）。

- **修法**：`rebuild_order` 改名 `ensure_order_mut` 并补上与 `ensure_order` **同一条**「先看脏标记」的
  守卫。改名是为了把两条路径的对称性写在名字上——本次事故的成因正是"这两条看起来是两件事"。
  加守卫的前提是"插入/删除真的标了脏"，已逐条核对：`insert`（只在新增时标，替换不标是对的）、
  `remove`、`remove_entity` 三处都在。**这个前提值得单独说**：漏标会让守卫读到过期顺序，而旧的
  "无条件重排"恰好会把漏标掩盖掉——所以先查前提，再动刀。
- **反向保护**：不许用「干脆不排」换绿灯。新增 `for_each_mut_rebuilds_order_only_when_dirty` 断言
  三个方向——稳态 16 次遍历**一次都不许重排**；插入后必须恰好重排一次、新实体出现在遍历里、仍按
  `EntityId` 升序；删除后同样恰好一次且被删实体不再出现。原有的
  `iteration_stays_sorted_after_interleaved_insert_and_remove` 继续钉住"dense 打乱后仍有序"。
- **反面对照**：把守卫临时改回旧行为后新用例实测红，读数是 **17 次重排**（1 次首次 + 16 次稳态遍历）
  对期望的 1 次——浪费被落到数字上，而不是"看起来更快"。
- **为什么测得出**：重排产出的 `order` 与重排前**逐字节相同**，从外部观察不到。所以加了一个
  `#[cfg(test)]` 的 `thread_local` 重排计数器（不用全局计数：`cargo test` 并行跑用例会互相污染），
  它不进生产构建。
- **顺带修正一处已失真的注释**：`engine/examples/wp0_ecs_storage.rs` 的 `HashStorage::for_each_mut`
  原注释称「与 `World::for_each_mut` 一致：每次调用克隆整份实体表」——`World` 侧走的是 `mem::take`
  移出再放回（不克隆），本批之后连重排也只在该做的时候做。该注释改为说明它是**替换前形状的建模**，
  `for_each_mut_allocs` 的 1 vs 0 才是可比的那对读数。
- **入库基线不用重采**：`wp0_ecs_storage` 基准测的是它自己的两个合成 storage（`HashStorage` /
  `DenseStorage`），**不调用**真实 `World`，所以 `artifacts/baselines/wp0-ecs-storage.json` 不受影响。
- **诚实边界**：本批**没有**跑 `verify.ps1 -Benchmarks`（release 构建 + ≥14 个压力样例，与当前机器的
  内存余量冲突），所以"省掉一次 O(n log n)"的证据是**重排次数 17 → 1**，不是墙钟数；要看墙钟请在有余量的
  机器上单跑 `cargo run --release -p engine --example wp0_ecs_storage`。
- **回滚档**：`rustlua-ecs/src/world.rs`（守卫 + 用例 + 计数器）与 `wp0_ecs_storage.rs` 的一处注释；
  无格式迁移、无数据迁移。
- **验证**：`cargo test -p rustlua-ecs` **137 通过 0 失败**；`cargo clippy -p rustlua-ecs --all-targets
  -- -D warnings` 零告警；`cargo fmt -p rustlua-ecs -- --check` 干净。

## Unreleased — `verify.ps1 -Quick`：把「这棵树现在红不红」压到 6.6 秒

主题：**门的覆盖面与门的成本错配**。pre-commit hook 只镜像 CI 的 lint job（`git diff --cached --check`
+ fmt + clippy），而真正咬人的**数据级工程校验**（`game --check` / `editor --check`：场景引用的资源、
脚本语法、`validate_data.lua`、组件 schema）此前只存在于总门里；总门要几十分钟到几小时，于是没人愿意
为「看一眼树是不是红的」付这个成本。代价已经记过两次：`scenes/main.scene` 的一句中文注释让
`game --check` 判红并在本地 HEAD 里活了 **5 个提交**（见下一条），以及 docs/34 §6 的
「门禁波动（并行会话）」——并行会话改写示例时总门因别人的半成品判红，读的人分不清是回归还是在途。

- **跑什么**：文档门 / vendor 补丁门 / `cargo metadata` / fmt / Lua API 文档桩 / Web API 覆盖账 /
  `git diff --check` / 两个判据自测 / `game --check` / `editor --check`（两工程）。
- **跳过什么（每一步都在结尾逐个打印）**：clippy、rustdoc、对外 Rust 符号面门（**与 rustdoc 一起跳**——
  它读的正是 `cargo doc` 刚生成的产物，只跳 doc 会拿上一次的旧产物当证据）、pre-commit hook 回归、
  全量测试、构建、空间迁移报告、编辑器性能基线、GUI smoke。本仓库口径是「跳过绝不等于通过」，
  所以它不打印 `Quality gate passed`，只打印 `Quick gate passed (PARTIAL)`。
- **清单不可能与 guard 漂移**：跳过判定走 `Test-QuickSkip "<步骤名>"`，步骤名写在 guard 那一行上、
  登记是它的副作用；脚本中途判红时清单会**自曝**「只登记到脚本走到的位置，清单并不完整」。
- **顺带修正**：`cargo metadata` 往 stdout 倒的整份 workspace JSON（几十 KB 机器可读噪音）改为成功时
  抑制、判红时照原样打出（`Invoke-Checked -QuietOutput`）——它此前会把质量日志里真正的红淹掉。
- **回滚档**：单文件（`scripts/verify.ps1`）、纯增量开关；不带 `-Quick` 时行为不变（唯一例外是上面那条
  metadata 输出抑制）。与 `-Mvp`/`-MvpOnly`/`-Benchmarks`/`-Web`/`-WasmLua` 互斥，混用当场判错。
- **验证**：`-Quick -Mvp` 混用立即判错且不跑任何 cargo；在本机工作树上实测 **6.6 秒**判红并退出 1，
  红在 `cargo fmt --all -- --check`——元凶是并行会话的 3 个未跟踪示例文件（`jhyt_m66_pause_prefab.rs`
  / `jhyt_m67_inventory_prefab.rs` / `probe_title_render.rs`），不是本次改动；quick 段其余每一步
  （文档门 / vendor 门 29 条 / metadata / Lua doc stubs / Web API 覆盖账 / `git diff --check` /
  两个自测 / `game --check` / `editor --check` 两工程）已逐条单独跑过，全绿。

## Unreleased — 场景注释不再冒充依赖：`game --check` 的幻影资源引用

主题：**门面场景的一句中文说明让整条质量门判红**。`scenes/main.scene:8`（M63b 的搬迁说明）
把 `assets/prefabs/title_menu.prefab` 写进了反引号、以全角逗号收尾；而场景/预制的依赖提取是
"任意位置扫全文"，终止符只有 `;` `:` 换行三种，于是截出的"引用"自带反引号与全角逗号，
`verify_project` 的 `is_file()` 必然为假——`cargo run -p game -- --project examples/game_jhyt --check`
报"场景引用的资源缺失"并以 1 退出，`verify.ps1` 与 CI 的 Game project check 同步判红。
为什么值得单独记一笔：这条红**不在任何代码改动里**，它是"注释里提了一句文件名"造成的，
从 diff 上完全看不出来。

- **注释不是数据**：`extract_scene_assets` 改为逐行扫描并跳过 `#` 行首注释，判定与两个真正的
  解析器逐字同源（`Scene::from_text` 与 prefab 解析器都跳过 `line.starts_with('#')`）。图集那条
  路径早就按同一原则处理（注释里的 `texture=assets/x.png` 不算边），本次只是把场景/预制补齐。
- **反面对照**：新增 `scene_comment_lines_do_not_produce_refs`（注释行整行跳过 + 同文件里的真实
  引用一个不能少 + prefab 同款）与 `comment_only_when_hash_leads_the_line`（行内 `#` 是图集区域
  后缀、不是注释）。把修复临时改回旧行为后该用例实测红，红出来的 `left` 逐字节就是门报的那条
  幻影路径（`assets/prefabs/title_menu.prefab` + 反引号 + 全角逗号）。
- **回滚档**：单文件单函数（`engine/crates/rustlua-assets/src/dependency.rs`），连同两条用例一起
  revert 即可；无格式迁移、无数据迁移。
- **同类隐患**：`examples/game_api` 的 `api_tilemap.scene` 与 `ui_test.scene` 注释里也有裸
  `assets/…` 路径，同一刀一并覆盖。

## Unreleased — 微信/web 的存档第一次有"真落盘"读数：宿主 shim 把写失败的真相报出来

主题：`save` 在 web/微信宿主上**不是引擎那份** `std::fs` 后端，而是 `plugins/webgl/web-runtime.js`
里 Lua 补丁自写的**第三份**（键值行编码 + 任意值长度前缀树 + 宿主存储读写回调）。它坏掉的形状是
"全部成功、零报错"：`commit` 只改内存就返回 true，`save_manager.save()` 照样打 `[save] committed`，
玩家看到的却是"存了但重开没恢复"，控制台一个字不打。这批把"落盘"变成一条机检读数。

- **补齐四个缺失名字**：`blob_read`/`blob_write`/`slot`/`slot_get` 此前走缺失代理（可调用、返回真值替身），
  `plugins/webgl/lua_api_overrides.json` 里 `save` 那条的 `dropped` 从 4 个归零。payload 与引擎
  `save.rs` 的 v2 编码逐字符同形（`save_version=2` 头 + 转义的 `key<TAB>value` 行 + 同排序键）；
  值层用长度前缀而不是 JSON，比引擎**更保真**——稀疏整数键原样回来（引擎的 `lua_table_to_json` 会
  退化成字符串键）。落点 `localStorage` / `wx.setStorageSync`，槽位键名镜像 `save_path_for_slot`。
- **契约层**：`wechat-adapter.js` 的 `setItem` 现在把成败**返回**给运行时（原先只 `warn`）——只 warn
  不上报时运行时拿到的永远是"写成功"。判据写的是 `=== false`，因为真浏览器的 `setItem` 返回
  `undefined`，不能当失败。这是上一轮"shim 静默失败"同一个形状的第二例。
- **门**：`scripts/wechat-minigame-harness.js --check-save-persist`（必须配 `--drive-new-game`，标题界面
  上快照恒空）写独立槽位、跑真实 `save_manager` 的存-清-读往返，断言宿主存储里真有 `save_version=2`
  的 payload、键名与引擎对齐、排序确定性、缺档/坏档四种损坏各自的返回值；`stats.failStorage` 是宿主写
  失败的**注入开关**（反面对照，没它这条门只会证明自己跑过）。读数
  `save_persist="ok,probe=518,game=15074"`——`game=` 是真实游戏提交上去的字节数，<1000 直接 throw，
  防的是"探针自己玩自己"。该字段已并进 `wechat_minigame_package_ok` 汇总行。
  复现：`powershell -NoProfile -File scripts/verify-wechat-minigame.ps1`（2026-10-06 20:4x 实测 exit 0）。
- **买不到的部分**：跑的是 Node 伪造的 `wx`（`stats.storage` 是个 `Map`），**真机未验**；存档只在**本宿主**
  内持久，PC 的 `.sav` 与浏览器 `localStorage` 互相看不见；`commit` 是整份 payload 覆写，没有增量。
  逐条断言、编码细节与限制见 [docs/51](docs/51-平台插件适配.md)「存档落盘」一节。回滚：revert 本批 8 个路径，
  并撤掉汇总行里的 `save_persist=ok`——那行是这条门唯一的对外痕迹。

## Unreleased — WP12：`host/` 两个上帝文件拆成子目录，单文件全部 ≤ 1,500 行

主题：**`runtime_scene.rs`（10,458 行）与 `renderer.rs`（6,089 行）不再存在**，各自变成
`host/runtime_scene/`（11 个源文件 + `tests/` 18 个）与 `host/renderer/`（10 个源文件 +
`tests/` 8 个）。这批**只搬位置**：不修 bug、不优化、不合并重复、不拆函数体，最大的两件事
（`new_async` 1,070 行、`render_passes` 446 行）整段原样搬。为什么值得做：这两个文件占了
`host/` 的 80%，改一行渲染顺序要在 6 千行里滚两轮、两份 5 千行的测试块和实现混在同一个文件里；
拆分后"读哪一段"由文件名决定。逐阶段账目、行数对照与全部读数见
[tasks/done/WP12-host拆分.md](tasks/done/WP12-host拆分.md) §12/§14。

- **可证明的"纯搬运"**：每一刀都用脚本按段证明（去注释+去空白后，搬出的每个行段必须原样出现在
  目标文件里，且留在父文件的段按原序命中）。口径必须去空白是因为 rustfmt 会因 `pub(super)` 把
  签名折成竖排并补尾逗号——逐行 diff 在这种改动下必假红。
- **唯一允许的文字改动是可见性放宽，且全部由编译器点名**：全套 **61 处 `pub(super)`**
  （`renderer/` 30 + `runtime_scene/` 31；`new_async` 那一刀是 **0**），方向只有"子模块 ↔ 父/兄弟模块"，
  **没有一处放宽到 `pub`**——这一条不是口头保证，而是由符号面门兜住：私有项若被放宽成 `pub`，
  `cargo doc` 就多一条锚点页，门必红。子模块是父模块的**后代**，按私有字段构造结构体字面量本就
  合法，所以搬 `Self { … }` 那一大段不需要任何放宽；`Renderer`/`RuntimeScene` 的结构体与字段
  也仍留在各自 `mod.rs`（搬走结构体会逼出几十条字段放宽）。
- **测试面没增没减**：拆分前 `host/` 的 `#[test]` 逐文件实测 100 + 23 + 22 + 4 + 9 = **158**，
  拆分后 `cargo test -p engine` 的 unittests 仍是 **158 passed**（`engine` 这个 crate 的单测
  100% 落在 `host/`，所以这两个数必须相等）。
- **对外面零变化**：`host/mod.rs` 的 `pub use` 一字未动；`engine` 的对外 Rust 符号面与阶段 0 基线
  一致（`node scripts/engine-pub-surface.mjs --check` → `items=2131 moved_into_submodules=6`，
  多出的 6 条是定义搬进子模块后的 rustdoc 页面，原路径仍可解析）；
  `git diff --name-only 42a565c..HEAD -- engine/crates/editor engine/crates/game engine/crates/assetc engine/crates/game_web`
  = **空**，`plugins/` 与 `lua_api_overrides.json` 也不在本包 8 个搬运 commit 的 diff 里。
- **`verify.ps1 -Mvp`**：Quality 段**全绿**（fmt / `clippy --workspace --all-targets --all-features -D warnings`
  零告警 / rustdoc 零警告 / 符号面 / 文档 / pre-commit hook 门 46 例 / 全量测试 **1,481 通过 0 失败
  2 跳过 44 目标** / 两个工程的 `--check` / 编辑器 32 场景 + GUI 41 例）。**MVP 段红一条**：
  `jhyt_m17_shop` 断言商店数 `= 1`，而 `1cd624b`（2026-10-01，本包起点的**祖先**）已把示例工程
  扩到 3 家店；那两个文件在本包区间里一个字都没改（复现命令在计划 §12 第 10 项），
  且断言住在不许动的 `crates/game` 里（§7 的人工核对），所以**本包不修**。
- **画面/几何读数**：六份像素 golden 与 2026-09-20 留档逐字段对照 **209 项里 1 项不同**
  （`content_bounds[3] 179 -> 205`），那 1 项对得上 `0de935f`（2026-09-21）把 `demo3d.scene` 里
  `ShadowTriangle` 从 `y=0.7` 改成 `y=0`——物体下移、内容下边界外扩，方向与量级都吻合；
  `wp0_scene_frame` 与 09-26 留档对照 **1,057 项里 6 项不同**，全是三个 `.scene` 的实体数，逐条对得上
  内容提交。几何与分配那档（`compare-baseline.ps1 -GeometryOnly` 对入库基线）**12 项 0 回退**。
- **回滚档**：每个阶段一个 commit（`378e7ad` 守卫 → `faa943b` 测试 → `2ede51c` 自由函数 →
  `ad130aa`/`69323e8`/`ef30c52` 三刀 → `2e451f3` 4a+4b → `2b707a2` 4c → `2057aeb` 4d），
  任一刀可单独 revert。**一处已知的偏差**：4a 与 4b 落在同一个 commit——4a 的文件在 add 与
  commit 之间被同仓另一路提交扫走，没有改写历史（共享工作树），4a 的账目在计划 §14.6。
- **诚实边界**：`render_passes` / `new_async` / `vertices` / `register_builtin_frame_systems`
  **仍是单个大函数**，只是换了文件；`Renderer` 的 90 个字段与 `RuntimeScene` 的 40 个字段仍是扁平
  结构——"上帝对象"这一半的债本批不解（拆开它要动数千个 `self.x` 访问点，diff 就不再能证明纯搬运）。
  搬运中发现的 10 条问题（冗余容量字段、重复的扩容逻辑、测试靠 glob 拿父模块 use 绑定、模块重名
  遮蔽路径、`docs/37`/`docs/38`/`docs/53` 里的行号引用已失效、`verify -Mvp` 的 MVP 段本来就红在
  示例商店数据上……）逐条登记在计划 §12，**本批不修**。

## Unreleased — 微信真机 EH 探针：把"设备引擎认不认 wasm 异常处理"变成一条机检读数

主题：**剥完 tag 导入之后，剩下的决定性问题从"认不认 tag 导入"换成"引擎实没实现 EH"**。
上一条批次（`786cfa5`）已用 binaryen-133 把两个 externref tag 导入从产物里剥掉（polyfill tag 被
宿主实测拒绝，见用例 1a3 的 `polyfill_tag_probe=rejected`），但产物里**仍然留着**本地 tag 段
`$tag$0` + legacy `try/catch`（实测 `throw`/`catch $tag$0` 各 1 条、`try=92`/`catch_all=501`/
`rethrow=92`）——那是 Rust `catch_unwind` / Lua `pcall` 的载体，整块剥 EH 会把 play 路径打成
`Error: unreachable`。于是真机上唯一没证的就是"微信 `WXWebAssembly` 底下那台引擎到底认不认 EH"，
而本机 V8 支持 EH、永远测不出"设备不支持"那一侧——这条只能真机读，本地能做的把它做成非空转守卫。

- **隔离探针，不 compile 主产物**：`wasm-bridge.js` 的 `probeEHAcceptance` 经 `WXWebAssembly.compile`
  编译一份 53 字节极小模块 `pkg/eh-probe.wasm`（零导入 + 一个本地 tag + legacy `try/catch`，由发布
  链路**同一个** binaryen-133 `wasm-as` 汇编，字节级确定）。不用主产物当探针是因为它顺带还用了
  GC/reference-types/bulk-memory 一片特性，compile 失败分不清是不是 EH 的锅；极小模块把变量收到只剩 EH。
- **并发、不挡 boot**：探针与实例化并发跑、自带 3 秒超时、永不 reject，读数经适配层落进
  `__RUSTLUA_DIAG().wasm.ehProbe`（`eh_probe=accepted` 说明引擎实现 EH；`eh_probe=compile-reject[…]=`
  说明连本地 tag 段都拒，只能换"根本不用 EH"的产物形态）。旧的、从没被调用的 `probeTagAcceptance`
  一并删掉。
- **发布链路带上探针**：`package.ps1` 微信分支在剥 tag 之后写 `pkg/eh-probe.wasm`（那 53 字节以十六
  进制常量入库、随包带上），否则真机上 `compile` 没有可读的代码包路径。
- **两条非空转正对照**：harness 的假 `WXWebAssembly` 补上 path-only 的 `compile`，让这条设备通道在本机
  真的跑起来；`wechat-minigame-harness.js` 汇出 `eh_probe=`（compile 通道，本机 V8 必 accepted）与
  `eh_probe_exec=7`（直接实例化探针调 `f()`，只有真跑过 `throw→catch` 才返回 7，挡住"探针其实是份不含
  EH 的桩、compile 当然过"的假绿）。`verify-wechat-minigame.ps1` 用例 1a3 断言这两条，`eh_probe=` 附进
  最终 `package_ok` 行。整门绿（含 no-wasm/分包四态/远端反向等负例）。
- **回滚档**：删掉 `probeEHAcceptance` 与其在 `loadWasmCore`/适配层诊断的接线、去掉 `package.ps1` 那句
  写探针、还原 harness 的 `compile` 复刻即回到"设备 EH 支持面只有一句口头判断、无机检读数"。
- **诚实边界**：本机 `eh_probe=accepted` 只是**正对照**（证明探针接对），不等于微信引擎接受——
  "工具上那份 `compile` 到底兑现 accepted 还是 reject"、以及 iOS（比这里更紧，连 Global 导出都整块拒）
  的 EH 支持面，都仍待真机/开发者工具跑一次读数才算证。

## Unreleased — 微信包"胶水没打进包"是假象：标准 WebAssembly 域名改由宿主转发

主题：**开发者工具里的 `wasm lua bridge not loaded` 报错了层**。屏幕上那句
`[wasm=glue-missing/WXWebAssembly detail=WebAssembly is not defined]` 说"胶水没打进包"，而胶水
就在包里（本次产物 52,225 字节）——真正的缺口是 `--target no-modules` 的产物在 **require 求值期**
就无条件读标准域名（`const __wbindgen_jstag_polyfill = new WebAssembly.Tag(...)`），而小游戏宿主
只注入 `WXWebAssembly`。实测与四层修法见 [docs/52](docs/52-微信小游戏分包发布.md) R13。

- **facade 只转发、不伪造**：`wasm-bridge.js` 在 `requireGlue` **之前**把宿主真有的构造器
  （Tag/Exception/Memory/Table/Global）按标准域名转出来；宿主没有 `Tag` 时直接判
  `webassembly-api-incomplete`，detail 带 `facade missing` + 宿主键名清单 + 求值期全局读数。
  为什么不能伪造：那两条 tag 是 wasm 的**导入**（56 条导入 = 54 function + 2 tag），假对象在
  instantiate 时必被宿主拒——伪造等于把撒谎的报错换成另一句撒谎的报错。
- **域名来源变成可读证据**：`__RUSTLUA_DIAG().wasm.facade`（转发的构造器清单 / `host-standard`），
  进 harness 证据行 `wasm=ok:ok:path-instantiate:Tag+Exception+Memory+Table+Global`。
- **本机门开始复刻工具形态**：harness 的假 `WXWebAssembly` 给出 Tag/Exception，并在装好之后
  `delete globalThis.WebAssembly`——Node 一直留着真全局正是这条漏到工具里的根因。判据钉两条：
  通道必须 `path-instantiate`、facade 必须是转发的。整门绿（`wechat_minigame_package_ok`，
  含 no-wasm / 分包四态 / 远端反向 / 画布与全局锁定等负例）。
- **发布期漂移守卫**：`package.ps1` 微信分支扫胶水里出现的每个 `WebAssembly.<名字>`，任一不在
  "facade 转发 + 通道逻辑覆盖"清单里就**发布失败**，与既有三条胶水补丁同一套"写法变了就响亮失败"。
- **回滚档**：删掉 facade 与 harness 那句 `delete` 即回到"本机有真全局所以本机绿"。
- **诚实边界**：facade 转发只覆盖"宿主真有那份构造器"的形态，而**开发者工具实测没有**——它的
  `WXWebAssembly` 域名是 `Global,Instance,Memory,Module,Table,compile,instantiate`，缺 `Tag`/`Exception`，
  所以本轮把 wasm 的报错从撒谎的 `glue-missing` 换成如实的 `webassembly-api-incomplete`，wasm 本身在这份
  宿主上仍起不来。缺 Tag 时 bridge 再问一次 `compile(代码包路径)`（3 秒超时——这个 Promise 排在 boot
  之前，挂不得），把 `probe=compile-ok` 或 `probe=compile-fail[宿主原话]` 写进同一行 detail：前者还有救
  （用一个只导出 tag 的极小模块经同一条通道取回真 Tag），后者要换不带 JS 异常桥 tag 的产物形态并重新验
  Lua pcall 的 unwind。iOS 更紧（连 Global 导出都整块拒），两处都仍待真机读数。

## Unreleased — WebGL 浏览器音频后端：包内音乐音效第一次真的响

主题：**WebGL 包此前全程静音**。`audio` 的 web 补丁层只有一句"浏览器没有音频后端"然后返回
`false`——游戏侧照旧把每条 `bus:name` 记进 failed 表，于是包能玩、剧本能跑完、门能绿，而玩家一句
台词都听不见。"门绿着而产物是哑的"正是产物层验收该拦的那类事，实测与逐条实现见
[docs/51](docs/51-平台插件适配.md) 的"浏览器音频后端"，读数口径见
[docs/32](docs/32-webgl-浏览器平台验收.md) §3。

- **发声换成 HTML5 `Audio`**：与微信小游戏共用同一套音效池（可重叠、48 条淘汰）、单例 BGM（`loop`）、
  音量与销毁函数，只换两处——"怎么发声"和"文件在不在"。存在性**同步**问 `pack/manifest.txt`
  （扩展名次序对齐引擎 `sound_bytes` 的 mp3→wav→ogg），因为游戏侧的 failed 表要求 `play_on` 当场
  返回 false；只靠 `onerror` 的话第一次一定乐观成功，那期间账已经记错。
- **自动播放闸门**：首次用户手势前 `play()` 一律 `NotAllowedError`。被拦的 BGM（标题曲是唯一在手势前
  就该响的）存进 `pendingMusicEl`，首次 `pointerdown`/`keydown` 补播一次；音效不排队——补播一个三秒前
  的点击声比不响更怪，直接淘汰。
- **进门的是读数、不是断言**：后端状态抄进 `window.__RUSTLUA_AUDIO` 并同步写 DOM 的
  `data-rustlua-audio`（产物层只 dump DOM，没这个属性就永远进不了门），浏览器侧另给 `musicProbe()`
  ——`currentTime` 随墙钟前进才等于"真的在放"，`play()` resolve 只等于"开始了"。
- **回滚档**：摘掉浏览器分支（`playBrowserSfx`/`playBrowserMusic` 与手势补播）就回到"浏览器返回
  false"的静音占位；微信侧的池与音量实现没换，本轮只补了统计计数和 `destroyCtx` 里的
  `pause()`＋摘 src（不摘的话被淘汰的音效会继续下载）。
- **诚实边界**：`played>0` **没有**被断言（剧本里的输入是 JS 合成事件、不构成浏览器手势，断言会把策略
  问题报成产品问题）；门只证到"元素在实时前进"，不证扬声器出声；L3 行为契约仍**没有音频用例**，
  `audio.play_on` 缺文件的返回值两侧都还没钉。

## Unreleased — P0：脚本错误按宿主分档处置（严格 / 容错）

主题：**一个 Lua 分支写错不再等于玩家这一局结束**。此前 `update()` 里未捕获的错误会沿
`SystemError → SchedulerError::SystemFailed` 让调度器放弃本步**剩余全部系统**（物理、粒子、时钟、
场景切换一并跳过），宿主随即终止进程——既丢掉了与脚本无关的推进，也让"脚本偶发错误"和
"引擎故障"在退出码上无法区分。详见 [docs/64](docs/64-Lua脚本错误处置策略.md) 与
[ADR-0023](docs/02-technical-decisions.md)。

- **`ScriptErrorPolicy`（默认 `Strict`）**：严格档＝首错即上报，与既有语义逐字一致；
  容错档＝错误进结构化诊断与计数器、固定步继续跑。交互宿主（`game.exe`、编辑器 Play）显式选容错，
  无头样例/soak/性能基线与 Web 宿主保持严格——**26 项样例门与"脚本报错必须判红"的强度不变**。
- **按回调的错误预算**：`update()`、UI 点击分发、每实体 `on_spawn`/`on_update`（按 `EntityId`）
  各自独立；连续报错达 60 步（≈1 秒 @60Hz）的回调被停用，此后不再进 Lua，日志也不会被同一个错误淹没。
  **成功一次即清零**，因此只在特定输入下才出现的错误仍能自愈。每实体预算随实体消亡剪除
  （`EntityId` 含 generation 的键永不重复命中，不剪就是恒定泄漏）。
- **容错 ≠ 静默**：`script_error_total()` 与 `script_disabled_callbacks()` 是宿主的两个断言出口；
  日志按 `path+phase+message` 去重，每个不同错误只打一条，停用时单独再吼一条。
- **回滚档**：把宿主的 `set_script_error_policy(Contain)` 一行去掉即回到首错即停；`guard.rs` 与
  两个计数器是纯增量，`Strict` 路径不经过它们。
- **诚实边界**：不提供 Lua 沙箱（`io`/`os` 仍开放）、不做内存/指令预算；停用是**一次性判定**
  （解除只能靠热重载、换场景或重开一局），依赖被停用回调的玩法会静默降级，日志与
  `script_disabled_callbacks()` 是唯一信号。

## Unreleased — 优化项 10：rustdoc 站点发布与 Lua API 文档桩

主题：文档第一次成为**可发布、可验证**的产物。此前 `cargo doc` 只在有人手动跑时存在，且带着 78 条
失效 intra-doc 链接——fmt/clippy/test 全绿的同时文档可以一直在烂，因为没有任何门看它。

- **`scripts/gen-rustdoc.ps1`**：`cargo doc --no-deps --workspace --lib` → 可直接发布的静态站点目录。
  关键不是跑 cargo，而是补上 cargo 缺的那块——多 crate workspace 下它**不写根 `index.html`**，
  直接把 `target/doc` 推上托管就是 404 的站点根；脚本补根导航页（Lua API 参考置顶）、`.nojekyll`
  （GitHub Pages 的 Jekyll 会吞掉下划线目录），并自检"根页与每个 crate 的 index 都在"才报成功。
  默认带 `RUSTDOCFLAGS=-D warnings`：有断链就不发布，宁可不更新站点。
- **`.github/workflows/rustdoc.yml`**：push `main` / 手动 dispatch 时生成并 `deploy-pages`。
  一次性前提是仓库 Settings → Pages → Source 改成 GitHub Actions，这一步只能人工做（本机无 `gh`、
  无凭据），没改时 `deploy-pages` 显式报错而不是静默不发。
- **告警 78 → 0，并按 rustdoc 的解析规则修而不是靠猜**：从私有子模块 `pub use` 上来的名字不进链接
  解析作用域（`[`Camera2D`]` 无效，必须写公开路径），且**文档里不能用自己的 crate 名起头**
  （`engine::…` 只在 editor / game_web 里成立，engine 自己得写 `crate::…`）。哪些路径真公开没有
  人肉判断——`target/doc/**` 里**含 `index.html` 的目录才是公开模块**，据此索引机械限定 120 处，
  剩 10 处按性质逐条处理（私有子模块名与私有方法降级成代码块、已改名组件指向 `GlobalTransform`）。
  **没有为了消警把私有项改 `pub`**，那是用扩大 API 面来掩盖文档问题。
- **`scripts/gen-lua-api-docs.js` + `engine::lua_api::<模块>` 文档页**：41 张模块表 / 275 个绑定函数
  各生成一节 markdown 桩，经 `#![doc = include_str!]` 挂进 rustdoc。事实源是**运行期 API 面基线**
  `lua_api_surface.json` 而不是绑定源码——源码里 `create_function` 267 处、运行期暴露 275 个，
  只有以基线为主才能同时抓出「死绑定」（注册了但 `_G` 里没有，docs/37 的 `input.action` 类事故）
  和「解析器漏形状」。签名从闭包参数表推断，需人工核对成 Lua 口径。
- **门**：`--check` 断言再生成是 no-op、桩数不超基线、无死绑定；与 `RUSTDOCFLAGS=-D warnings` 的
  rustdoc 门一起进 `verify.ps1` 与 `quality.yml` 的 lint job。桩数下降也要显式 `--update-baseline`，
  否则"变好"会变成没人核对的漂移。缺 node 时打印 `[SKIP] …（不是通过）`，不伪造绿灯。
- **回滚档**：删掉 `rustdoc.yml` 即停止发布，本地 `gen-rustdoc.ps1` 与文档桩页不影响任何运行期行为
  （`lua_api.rs` 是只带文档的空模块）；告警清零那批改动全是注释行，可按 `[`crate::` 逐条反查。
- **诚实边界**：275 个函数目前**全是桩**（`documented=0`）；`-D warnings` 只覆盖 `--lib`；
  本批只动注释行，提交树上 `cargo fmt --all -- --check` 实测干净；在途批次带来的格式差异没有顺手改掉（那会把无关 diff 混进这次提交）。
  详见 [docs/61](docs/61-rustdoc站点发布与lua_api文档桩.md)。

## Unreleased — 优化项 11：本地 pre-commit 门（提交前跑 CI 的 fmt/clippy），门自身进 CI

主题：CI 的 `lint` job 是 `build-test` / `gates` / `web-evidence` 三个 job 的 `needs` 前置，它一红整条
流水线什么都不跑。而 lint 失败（格式、`clippy -D warnings`、行尾空白）在本地零成本就能判定——推到
CI 才发现的代价是"提交—等—红—改—再推"一整轮。本次把这三条搬到 `git commit` 之前。

- **`git-hooks/pre-commit`**：版本库内的原生 git 钩子，用 `core.hooksPath` 指过去，因此它**被跟踪、能
  review、改了所有人跟着生效**，而不是各人 `.git/hooks/` 下互不可见的副本。跑 CI lint job 的同一批命令；
  暂存内容没碰 `engine/**` 的 `.rs/.toml/.lock` 时整段跳过（改文档不该等编译门）。
- **没有选 cargo-husky / pre-commit 框架**，是当场比过本机事实后否掉的：cargo-husky 要把第三方
  build script 塞进 workspace 成员，而 `engine/Cargo.toml` 目前是虚拟清单；pre-commit 框架要求队友先
  `pip install`（本机 Python 在、`pre-commit` 不在），且仓库所有门禁的既有口径是 `scripts/*.ps1` + 退出码。
  代价说清楚：原生 hook 要每人跑一次安装脚本，装没装从结果上看不出来——所以配了 `-Status` 只读体检。
- **档位与逃生门都留了口子，但措辞按"跳过"而不是"通过"来写**：clippy 默认 `fast`（不带 `--all-features`，
  冷缓存时那一档要重编整个 feature 图），`git config rustlua.clippy full` 与 CI 逐字对齐；
  `RUSTLUA_SKIP_PRECOMMIT=1` / `rustlua.hooks off` / `--no-verify` 都会打印 `SKIPPED` 并说明失败留给 CI。
  PATH 上找不到 cargo 时**判红而不是放行**——跑不了的门禁不能算测过。
- **`scripts/test-git-hooks.ps1`**：门自身的回归，进 CI 的 lint job 与 `verify.ps1` 常规门。它在临时 git
  仓库里**真的执行提交**，用退出码和 HEAD 是否移动断言拦与放（含"修好格式后同一份改动必须放行"，
  否则门会把人焊死在里面），另外反查 `quality.yml` 的命令原文比对——CI 改了 clippy 参数而 hook 没跟上
  会直接判红。为什么非要真提交：只 grep 脚本文本的话，"暂存区过滤规则写错导致门永远跳过"这种最坏形态
  恰好测不出来。
- **`.gitattributes`（新增）**：`git-hooks/* text eol=lf`。Git for Windows 装在
  `C:\Program Files\Git\etc\gitconfig` 的 `core.autocrlf=true`（本机实测就在这）会把 sh 钩子检出成 CRLF，
  表现为"hook 装了却每次提交都报 `bad interpreter: /bin/sh^M`"。刻意不加 `* text=auto`：那会把
  `scripts/*.ps1` 既有的 UTF-8 BOM + CRLF 口径一次性改成满屏脏 diff。安装脚本也会顺手把工作区里的
  CRLF 修回 LF。
- 装完第一次真跑就抓到东西：`engine/crates/engine/src/core/lod.rs` 当前未提交的改动里有一条超行宽的
  `#[error("…")]`，`cargo fmt --all -- --check` 判红——正是本项要消掉的那类循环。

细节与已知边界（cargo 读工作区而非暂存区、CI 的 `git diff --check` 在干净检出上恒真所以本地改用
`--cached`、GUI 客户端 PATH 里没有 cargo 怎么办）记在 [docs/34](docs/34-质量门与发布清单.md) §3.5，
目录职责见 [docs/23](docs/23-工程目录功能说明.md)。

回滚档：本提交之前。回滚只影响提交前的本地等待与 CI 里那一 step，不影响任何运行行为
（`git config --unset core.hooksPath` 或跑 `install-git-hooks.ps1 -Uninstall` 即可停用钩子）。

## Unreleased — 优化项 12：thiserror + anyhow 统一错误处理

主题：错误处理原来有两套各自最费笔墨的写法——库层 72 个错误类型逐个手写
`impl Display`（`match` + `write!`）再补一行空的 `impl Error for T {}`，宿主层则用
`Result<T, String>` 把类型全部抹掉。本次按层分工收口：**类型化错误归 thiserror，错误聚合归 anyhow**。

- **72 个错误类型转 `#[derive(thiserror::Error)]`**：手写 `Display`/`Error` 实现共 **1,110 行**
  换成 **321 行** `#[error("…")]` 属性（样板少约 71%）。消息文案逐字照抄，不改写：本项目有
  golden 断言与 JS/Lua 侧按错误文本做的匹配（`assetc` 的「输出目录不能与项目目录相同」、
  `LuaBridgeError` 进 `ScriptDebugger` 的归并键），换个说法就是行为变更。
- **`Error::source` 从"约定"变成"结构"**：`CommandError` 原先手写 `fn source()` 返回
  `World`/`Failed` 的子错误，现在由名为 `source` 的字段自动接上，链路一致；88 行手工
  `impl From<..> for *Error` 里能等价表达的都折成 `#[from]`（14 处），剩下的刻意保留——
  `SceneError`/`PrefabError` 的 `From<io::Error>` 要把错误塞进 `Io { path, reason }` 两个字段，
  `From<WorldError> for TransformSystemError` 内部对 `WorldError` 做了 match，都不是单变体构造，
  `#[from]` 表达不了。
- **宿主层 55 个签名改 `anyhow::Result<T>`**（`host/runtime_scene.rs`、`host/runtime_scene/text.rs`、
  `host/renderer.rs`，加 `assetc` 与 `game_web` 两个入口）。最大的一笔收益是 **51 处
  `.map_err(|error| error.to_string())` 全部删掉**：库层错误现在真的实现 `std::error::Error`，
  `?` 直接就能升成 `anyhow::Error`，不需要那条把类型丢掉的转接。
- **两条边界按角色划清，不是随手铺开**：`anyhow` 只出现在 `host/`、`assetc/`、`game_web/`，
  `core/**` 里一处都没有（`core_does_not_depend_on_host` 测试仍绿），库层继续暴露可 `match` 的
  具体错误类型；签名一律写全限定 `anyhow::Result<T>` 而**不** `use anyhow::Result`，因为那个
  别名只吃一个类型参数，import 进来会和同文件里 `Result<T, E>` 的 std 写法打架。
- **唯一一处 API 面变化，是被 thiserror 的规则逼出来的**：三个字段原名 `source` 却装 `String`
  （`EngineError.source`、`TextureError::Read/Write.source`、`LuaBridgeError::Load/Runtime.source`），
  thiserror 见到 `source` 就认定它是错误源并对其要求 `Error`，`String` 满足不了。这三个字段本来
  携带的是**已格式化的文本**而不是错误对象，于是改名：`EngineError.source` → `cause`（与它自己
  输出的 `"; cause: …"` 对齐），另两处 → `reason`（与同族 `SceneError`/`GltfAssetError` 既有的
  `reason` 字段对齐）。渲染出来的文案一字未变。
- 前一次转换（见上条优化项的 lod.rs 记录）已经证明 fmt 门会盯着 `#[error("…")]` 的超行宽，
  本次同样以 `cargo fmt --all -- --check` 为准；`cargo clippy --workspace --all-targets
  --all-features -- -D warnings` 与 `RUSTDOCFLAGS=-D warnings cargo doc --no-deps --workspace
  --lib` 均零告警。

**未量的代价**：`anyhow` 进了 `game_web` 的 wasm 依赖树，其错误装箱/格式化机器会给核心包体积
带来增量（docs/52 记录的口径是 wasm-opt -Oz 后 2,371 KB）。`cargo check -p game_web
--target wasm32-unknown-unknown` 通过，但**没有跑发布 wasm 构建量体积**——需要体积数字时按
docs/52 的 `-Platform webgl` 路径实测再对比。

回滚档：本提交之前。运行行为零变更（错误文案、`Error::source` 链、跨 JS 边界的字符串都不变），
唯一需要一起回退的是三个错误字段改名；`engine/Cargo.toml` 的 `thiserror` / `anyhow` 两个
workspace 依赖已在 lock 里（mlua 本来就带 anyhow），回滚不需要联网取包。

## Unreleased — jhyt v0.2：音频统一 MP3（分包省 5.4 MB）+ 81 MB 内嵌字体出库 + 主线四/五段与四结局

主题：《江湖有田》往微信小游戏分包里收体积，同时把主线推到结局分支。两处体积来源都是"仓库替运行时
背了不该背的账"——音频为了 iOS 不支持 ogg 而备双格式，字体为了构建机方便而把完整 TTF 直接入库。

- **音频从 OGG+WAV 双格式收口成单一 MP3**：InnerAudioContext 的官方格式矩阵里 MP3 在 iOS 与 Android
  均支持（ogg 仅 iOS ✗，wav 体积过大），源文件按 96 kbps / 22050 Hz / 单声道直接产出。`assets/audio`
  的 16 个 `.ogg` 换成 `.mp3`，分包内该目录 6,905,036 → 1,497,215 字节（占资源 77.4% → 49.8%，见
  [docs/52](docs/52-微信小游戏分包发布.md)）。连带删掉构建期的格式转换整条链路：`scripts/package.ps1`
  的 ogg2wav 段与 `engine/crates/engine/examples/ogg2wav.rs` 移除，`web-runtime.js` 不再读
  `getSystemInfoSync().platform` 选后缀，`srcCandidates` 固定 `.mp3`。
- **引擎与工具链接受 mp3**：`AudioImporter` 扩展名表、`ResourceKind::from_path`、`core/audio.rs` 的
  `sound_bytes` 解析顺序（`.mp3` → `.wav` → `.ogg`，仍按主干名解析，游戏侧 Lua 一字未改）、编辑器
  资源浏览器的格式识别用例，以及 `scripts/serve-static.js` 的 MIME。校验用例拿工程内真实 mp3 当 fixture。
- **assetc 压缩白名单补 mp3**（本批收口时补上）：mp3 虽是压缩格式，实测这 16 个低码率单声道 clips 走
  pack 的 gzip 默认档仍从 1,497,215 降到 1,441,547（-3.7%，帧头与填充字节可压）。新增
  `compressible_assets_cover_every_packaged_asset_format` 钉住"资产格式全在白名单里"这条口径——漏一个
  只会让包悄悄变大，要到发布期量体积才暴露。
- **81 MB 内嵌字体出库**（[docs/60](docs/60-全面复盘与优化项清单.md) 治理项 7.1）：`editor/fonts` 的
  `SourceHanSerifSC-Regular.ttf`（57.1 MB）与 `LXGWWenKaiGB-Regular.ttf`（7.3 MB）连同两份 LICENSE 移出
  版本库。运行期不受影响：工程渲染走 `assets/fonts/NotoSansSC-VF.ttf`，发布包里是 assetc 的子集产物，
  完整字体本来就不随包分发。**没有**顺手动 `NotoSansSC-VF.ttf` 那份 17 MB——它仍被仓库测试与 assetc
  子集化路径按相对位置引用，出库要单独一批。
- **v0.2 玩法内容**：对话注册表 3 → 6 条（新增陆归藏「真相」、苏晚晴「承业」、玄鹤「雪恨」三份文件，
  顾长风/沈青萝的既有对话扩出玄明/断念/归田节点）；任务注册表 9 → 15 条（主线四/五段 + 归田/承业/断念/雪恨
  四个结局）；items 22 → 29（炼金/铁匠/厨房扩展），`jhyt_m23_data_validation` 的计数门同步；
  `[input]` 追加 `skill_2/3/4`（V/N/M → 弦月斩/铁砂掌/豹跃），HUD 与 `ui/input.lua` 接上失败原因提示。

**未量的代价**：MP3 一律按官方格式矩阵推定，**没有 iOS 真机播放实测**；分包体积数字是 assetc 产物层
口径，**没有跑 `-Platform wechat-minigame` 的完整发布构建**复核切包结果。要承诺真机可玩需补这两项。

回滚档：本提交之前。回滚不是纯代码操作——源音频已是 `.mp3`，退回 ogg 口径要把 16 个文件换回 `.ogg`
并恢复 `package.ps1` 的 ogg2wav 段；字体出库只减文件、不改任何引用路径（运行时字体路径没变）。

## Unreleased — M27-00 编辑器 dock 化：五块固定面板区进 egui_dock 树，布局树持久化到 layout.json v2

主题：复核清单第 12 项（编辑器体验升级）的第一批。此前编辑器的面板位置写死在
`main.rs::draw_frame` 里——资源/层级在左、属性在右、控制台在底、Scene/Game 在中央，用户改不了，
"把属性面板挪到控制台旁边"这类需求只能等引擎侧排期。本批把整块工作区换成一棵可拖拽重排的
`egui_dock` 树，并且**让用户摆的样子活过重启**——这两件事合起来才有意义：能拖但存不住，等于没拖。

- **`egui_dock = "=0.18.0"` 是逐版硬绑定的**：crates.io 实测映射 `0.18↔egui 0.33`、`0.19↔0.34`、
  `0.20↔0.35`、`0.21↔0.36`。用错版本不会报"版本不匹配"，而是**静默拉进第二份 egui**（`cargo check`
  只说"找不到 trait"），所以钉死并在 `engine/Cargo.toml` 写下"升级 eframe 必须同步"。
- **默认树 = 迁移前布局，并且用几何断言钉住**：`dock::tests::default_layout_matches_the_previous_fixed_panels`
  渲染一帧后读叶子矩形，断言资源 200 / 层级 240 / 属性 360 / 控制台 150 的比例。这条比截图早抓到
  一个静默 bug：`Tree::split` 的 `fraction` 属于**左/上子节点**而不是新节点，`Split::Right/Below`
  里第一个子节点是*旧*节点——照字面传"新面板要多大"会得到反过来互补的值（控制台吃掉 79% 高度）。
- **面板侧只切"外壳/主体"，逻辑零改动**：`draw_console_body`/`draw_resource_browser_body`/
  `draw_hierarchy_body` 拿到普通 `&mut Ui`；`panels/architecture_tests.rs` 的棘轮（面板不得写
  `DockState`）继续成立。借用冲突（`DockArea` 要 `&mut DockState`、`TabViewer` 要 `&mut EditorApp`）
  用 `dock_state: Option<…>` 每帧 `take`/放回解决，舞蹈只留在 `dock::show_workspace` 一处。
- **`workspace_tab` 降级为派生态**：Scene/Game 标签在绘制自己时写回它，所有程序化切换必须走
  `EditorApp::focus_workspace_tab`。直接赋值会**静默失效**（阶段 1 就是这么把"Play 后自动切 Game 视图"
  弄坏的），这条现在有接线测试守着。
- **`LAYOUT_VERSION` 1→2，新增 `dock_tree`**：`layout.rs` 对这份 payload **只按"是不是对象"接受、
  原样存取、绝不解读**，所以 v1 立下的三条契约（未知键保留、save→load→save 字节相同、损坏回落默认
  且不覆盖源文件）一条没破，100 次往返幂等测试已含 `dock_tree`。v1 文件只是缺这个键，读回来就是
  "没存过"，`clamp()` 把版本号盖章成 2——**没有迁移步骤**。"重置布局"同样零额外代码：默认布局的
  `dock_tree` 是 `None`，据此重建默认树，切工程/重启/重置共用一个入口。
- **落盘频率是被 egui_dock 自己的行为逼出来的**：它的 `show_separator` **每帧**把 `split.fraction`
  重新夹紧到"两侧各留 175px"，于是**拖动窗口边缘本身就是连续的真实结构变化**，直接序列化就是每帧写一次盘。
  做法是两步：变化检测用**不含矩形**的结构指纹（`dock::signature`），落盘等**稳定 300ms**
  （`DOCK_LAYOUT_SETTLE`）。首次观察只登记基线，所以打开项目不会顺手写盘。
- **两个"宁可这次不存"的守卫**：① `Rect::NOTHING` 里的 ±∞ 经 `serde_json` 会变成 `null`，写进文件
  就再也读不回来 ⇒ `serialize` 序列化后立刻试解析一次，失败返回 `None`（连带后果：没画过一帧的树
  不可序列化）；② 同一标签出现两份 ⇒ 整棵树按损坏拒绝，因为 `sync_visibility` 按"树里有没有"决定
  补还是删，重复标签会让它判错。两种情况都推一条控制台提示，不静默回落。
- **浮动窗口比原计划更早持久化**：计划写的是"第一期不存 floats"，实测 `DockState` 本来就把浮动窗
  当额外表面、serde 一并带走，于是照做并补了 `a_detached_float_window_survives_the_round_trip`
  （含"不得被 `sync_visibility` 重复补回主表面"）。

**诚实边界**：重排这一步是**程序化**测的，真实鼠标拖分隔条/拖标签没有自动化覆盖（只在真实窗口里
复核了"存下来的非默认树重启后仍在"：`artifacts/tmp-m27-phase2/` 11 张截图，控制台在右列属性上方）；
View 菜单仍只能开关资源面板与控制台两项，六个 DockTab 逐项可切没做（Scene/Game 做成可隐藏需要
"至少保留一个中央视图"的守卫）；`panel_widths` 在生产路径已是死数据（宽度改由 split 自管），
出库要连带改 `perf_baseline` 的合成帧外壳；面板内标题与 dock tab 标题重复未处理；浮动窗矩形是
绝对坐标，换显示器可能落在工作区外。同机 A/B（基准 2026-09-27、同 debug profile）：`perf_baseline`
32 场景 `failed=0`，dock 相关场景 p50 全部持平或变好，最差一条是与 dock 无关的
`schema_field_edit_1k` +8.4%，而同 profile 的 run-to-run 摆动本身就有 ±20%。

回滚档：本批之前。回滚不是纯代码操作——`layout.json` 已被写成 v2（多一个 `dock_tree` 键），退回 v1
构建会**忽略**该键并回到默认树（不会报错，但用户摆好的布局从此不再恢复）；`egui_dock` 的 `serde`
feature 会连带打开 `egui/serde`，去掉依赖才能彻底还原编译图。

## Unreleased — M27-01 资产拖放：外部文件拖入编辑器 + 内部拖放收进同一张落点矩阵

主题：把"从资源管理器拖一个 png 进编辑器"变成一条动作。此前拖放只有 Scene View 一处，且只认
资源浏览器发出的内部载荷；外部文件完全不支持，导入必须先点菜单、再在系统文件对话框里把路径找回来。
更要紧的是两套拖放各写各的判定——加第二块落点时"谁接、接了做什么、拒绝说什么"会散成四份。
本批把它们并成一个协议：**载荷类型、落点判定、反馈文案出自同一处**，面板只画自己、收集事件。

- **`dnd.rs` 是一张纯函数矩阵**：`DropPayload`（资产/控件/实体）× `DropZone`（Scene 画布 / Hierarchy 行 /
  资源浏览器 / Inspector 资源引用行）→ `DropAction`，合法与非法都带一句中文文案（"这个字段要的是纹理/
  模型/预制体"）。写盘与写文档仍只在 `import` 与 `scene_ops`，架构棘轮里"面板不持文档引用"照旧成立。
- **系统拖放没有坐标**，这是本批真正的设计约束：`RawInput::dropped_files` 只在事件那一帧出现，而 drop zone
  要的是指针位置。于是生命周期改成帧内三段——帧首归一（剥 `\\?\` verbatim 前缀、递归限深、跳过链接、
  只有字节没有路径的条目显式拒绝）、每块落点**先到先得地认领**、帧末清扫没落点的并说明原因，
  而不是让它静默消失。
- **Inspector 那一行不能直接用 `dnd_drop_zone`**：它会把内容包进 `Frame` 并改变行高，而 Inspector 有
  "内容不得高于行高"的门槛断言。改成用它释放判定的等价物（hover 响应 + 松手取载荷），并且**只在确实
  有拖拽进行时**才登记这块 hover 区——一个常年盖在输入框上的 hover 区会抢掉下面控件的 tooltip。
- **三条不变量各自有测试守着**：一次拖入 N 个文件只产生**恰好一条 Undo**（任一资产读不进就整批不写）；
  **绝不覆盖**同名资产（落到 `hero-1.png`）；已在工程内的文件只引用不复制；Play 模式在任何写之前拒绝。
- **与原计划的偏差说明**：拖到资源浏览器目录节点本想做"移动/复制到该文件夹"，那需要跨目录移动与冲突
  合并语义，本批判成显式 Reject 而不是悄悄做一半；同理外部文件不设 `ExternalFiles` 载荷变体——它走事件
  通道，内部拖拽才走载荷，两条通道不该在协议里假装是一件事。

**诚实边界**：真实窗口的**拖放丢失率没有测**（tao 在 Windows 焦点路径下会丢 drop 事件是已知问题），
这需要人连拖几十次，归到 M27-03 的 soak；文件夹拖入只到限定深度为止、链接一律跳过；Inspector 只认
schema 里的资源引用行，普通文本行收到拖放是拒绝；Hierarchy 行没有"插到前/后"的分界判定，落到行上就是子节点。

回滚档：本批之前。删掉 `dnd.rs` 与四块落点的接线即回到"仅 Scene View 一处内部拖放"；不改任何磁盘格式，
外部导入复用既有导入管线，没有新开写盘路径。详见 `tasks/done/M27-01-资产拖放.md`。

## Unreleased — M27-02 Gizmo 增强与框选查漏：3D 三环与分轴把手、2D 旋转圆环、提交层的度/弧度修正

主题：把 Scene View 的可视化编辑按缺项清单逐条**先验证是否真缺**再动手（M10-07/M10-08 已交付过一轮
gizmo 与多选，七项里有三项实测已在）。真正补上的是三处，外加一个洞在提交层的换算缺陷——那三处共同
的毛病不是"功能没有"，而是**画面上画的东西抓不到、抓到的东西跟屏幕对不上**：3D 有三支环的影子但
只有绕 Y 的一支能用，2D 的旋转在画面上根本没有对应物（横着拖就转），scale 只有一个对角把手。

- **3D 旋转：三支环各自可用**。原码 `pick_gizmo_axis` 的 Rotate 分支对整个圆环一律返回 `GizmoAxis::X`、
  `rotation_delta` 恒用 `Vec3::Y`，于是"看到三支环、只有一支能用"。现在每支环按**真实三维圆采样再投影**
  （屏幕上是真椭圆，不是画个圆糊过去），命中按**到折线的最近距离**在三支里决胜负，角度用**射线与环面的
  交点**量出来——这样"指针在屏幕上指哪儿"就是"绕哪根轴转多少"。
- **3D 缩放：把手按用途分层**。等比搬到手柄**中心方框**，三支臂端各改一轴，两臂中段改平面；
  配套**轴掩码**，且**位置偏移同样只按掩码缩放**——否则拖 X 轴把手会把实体顺着 Y/Z 一起推走。
- **本轮的硬约束：绘制与命中读同一份函数返回值**（`scale_handles`/`ring_polyline` 一份数据两边用）。
  不这么做就会退回"画一整圈、只有一小段能抓"。环几何、射线求交、角度量、吸附、轴掩码、浮标文案
  全部是 `gizmo.rs` 里的纯函数，`scene_view.rs` 只多了两段绘制与一个事件转发——面板对文档零写入的
  架构棘轮照旧成立。
- **2D 旋转：画面上有了圆环，而且圆心就是你真正绕的那个点**。环心取自这一趟的转轴语义
  （Pivot=主选实体、Center=拖动集合平均），所以切轴心模式时环会跟着挪，不会出现"绕着看不见的点转"；
  投影走 WP5 那份唯一映射的逆（新增 `design_to_panel_pos`），没有另写一套 `pan + world*zoom`。
- **角度吸附独立步长**（默认 15°，工具栏 1..=90°）：与位移吸附共用开关，但单位必须是度数——
  拿 0.5 的位移步长当角度用（0.5°）不叫吸附，叫抖。**轨道角与写进文档的角是同一个数**，
  先吸附再喂轨道，否则"吸到 45°"和"存下 44.7°"会同时发生。
- **拖动中坐标浮标**：报的是用户刚改的那个数（位置=当前值、旋转=转了多少度、3D 还报绕的是哪根轴），
  2D 挂在指针旁、3D 挂在手柄旁，同一份字号。
- **查漏的三条裁定（不做，理由写进任务文件）**：① 框选"相交/包含"**已经存在**，形态是**方向规则**
  （左→右=完全包含、右→左=相交，Unity 同款）；再加一个是非开关等于给同一个手势两套规则。
  ② 起手势整组拖动已存在（按在已选实体上不清空集合，拖动 id 全量携带）。③ 多选聚合轴心两边都在，
  但 3D=`并集包围盒中心`、2D=`位置平均`，本轮**不统一**：2D 要算并集就得逐实体解析 Sprite 的
  size/pivot，把圆心挪到贴图上去，反而偏离"绕纹理中心转"的直觉。
- **顺带修掉的真实缺陷**（不是本轮功能，但不修则 2D 旋转圆环不可用）：`TransformFields.rotation` 是
  编辑器口径的**度**，引擎的平面 `Transform` 与 `Quat::from_axis_angle` 一律按**弧度**，而三个桥接点
  （`geometry::encode_transform` / `parse_transform`、`scene_drag::scene_drag_locals`）**都没换算**。
  后果是"绕 Z 拖到 45°"被存成"绕 Z 转 45 弧度"（四元数折回主值 ≈58.3°，读回来显示 1.0177），松手即跳。
  这个洞自 M10-07 起就在——此前没人拿真实角度对拍，是本轮新增的合成指针用例当场抓出的。换算现在只发生
  在编解码这一对函数里。
  **对存量场景的影响说清楚**：过去保存过的 2D 旋转，四元数里一直是"度数当弧度"，也就是说**运行期朝向
  与编辑器显示的框本来就不一致**；本轮只让新写入变正确，没有批量改写老场景，存量修复归入 M27-03 之后。
- **与原计划的偏差**：`snap_angle_step` 与整族吸附状态一样**不落 `layout.json`**（该文件历史上只持久化
  `gizmo_space`/`gizmo_pivot`），会话内生效、重启回 15°。

**诚实边界**：环的角度只在"射线打得到环面"时精确，相机侧视环面（`|dot| < 0.02`）时退回水平像素增量，
与旧行为一致；**3D 环半径来自屏幕臂长、没有透视除法**，远端实体的环在世界尺度上偏大（命中与绘制同源，
所以"点得到"没问题）；浮标只在主选实体上显示；2D 与 3D 的浮标锚点口径不同；**环和把手的真实手感未经
人眼验收**——本轮证据是纯逻辑单测 + 合成指针 + 真实窗口的文档对拍，连续拖动的观感（环是否好抓、
三支环重叠处如何取舍）归 M27-03 的窗口浸泡。用法与规则见 [docs/25](docs/25-editor-guide.md) 第 17 节，
逐条缺项裁定与证据见 `tasks/done/M27-02-gizmo增强与框选查漏.md`。

回滚档：本批之前。不改任何磁盘格式，纯编辑期交互，且**gizmo 增强与度/弧度修正是可分的**——换算只活在
`geometry::encode_transform`/`parse_transform` 与 `scene_drag_locals` 三处，回滚前者时若把后者一起退回，
"拖到 45°"会重新变成"转 45 弧度"（松手即跳），那是把一个已知缺陷当基线。

## Unreleased — M27-03 M27 收口：把"验收"变成可重跑的产物 + UI 框架不迁移的书面裁定

主题：M27 整期的收口批。这一批**不改编辑器行为**，改的是"凭什么说这期没把编辑器弄坏"这件事——
三块此前只存在于口头判断的验收（长会话、帧预算、落点矩阵）各自落成可重跑的机制与登记表，
再对"是否迁移更富表现力的 UI 框架"出一份带触发条件的 ADR。

- **soak 从"挂机"升级成"带着本期改动的路径挂机"**（`gui_evidence.rs`）：`RUSTLUA_EDITOR_SOAK` 之外
  加 `_CHURN_SECONDS` / `_DROP_EVERY` / `_DROP_SOURCE` 三个开关。每轮先 `dock::churn` 真的把树重排一步
  （拆浮动窗 → 并回 → 合并 → 拆回，六步循环，走的是鼠标拖拽改的同一套 `DockState` 变更 API），
  到点的那轮再走一次"外部文件导入 → 建实体 → **Undo 掉实体** → 下一轮开头删掉入库副本"。
  撤销与删副本不是为了测 Undo，是为了把工程大小钉成**常量**：每轮真留一个新文件，资源库、缩略图、
  `.cache` 就随轮数单调变大，那条内存曲线量到的是"资产变多"而不是"同一份工作重复两百遍"。
  删除放在下一轮开头，免得打断后台导入。每个采样点带 `churn_rounds`，报告头带
  `rounds / dock_churns / drop_imports / drop_cleanups / skipped`——**`skipped>0` 就说明有轮次在空转**
  （Play 锁定或树正被表面借走），不能读成"重排过这么多遍"。
  **实跑结论**：7500 s / 249 轮 / 748 采样 / `skipped=0`，`analyze-soak.ps1` 判
  `no_sustained_growth`。回归斜率：预热段 +6.82 MiB/min，修剪前长平台 +0.224，修剪后 +0.061，
  最后四分之一 +0.045 —— **斜率随时间往下走**。判定行里那个 `tail_growth=-209.8 MiB` 不作结论依据：
  它是下面那次系统回收白捡的，同一格的 `tail_noise` 被顶到 212.4 MiB（阈值才 71.6），信噪比是坏的；
  站得住的是回收之后那 266 个点自己（谷值 264.9→267.4、p50 270.8→272.8，期间又跑了 88 轮重排 +
  88 次导入，纹理账全程钉在 3.7/4.8 MiB 两档）。
  中途 t=4837 s 那一格 −214 MiB **归因为操作系统回收工作集**，不是编辑器释放：同一采样点上
  纹理数、纹理字节、实体数、undo 数、控制台行数、帧增量全部没动，日志也没有事件。
  这条同时给判据划边界——`WorkingSet64` 量的是**驻留**不是提交量，所以结论的正确读法是
  "驻留没有向上外推"；反过来说真泄漏一定会把被反复触碰的新页顶进驻留（预热段 +6.82 MiB/min
  那条台阶就是这么来的），新增自测的"每点 +4 MiB 必须判红 / 谷值不动的大锯齿不得误报"两格
  钉的就是这个灵敏度边界。落盘核对：249 轮跑完 `assets/` 21 MB、副本同时在飞 2 份
  （机制按轮删除，设计值 1 份；若完全失效这里是 249×4.86 MB≈1.17 GB），但
  `.cache/cache` 长出 56 个 blob / 9.87 MiB ⇒ "工程大小恒定"对 `assets/` 成立、对导入缓存不成立，
  两条一起记在任务文件的已知限制里。
- **soak 的判据工具在 HEAD 上就是坏的**，本期第一次运行才暴露：`scripts/analyze-soak.ps1` 首行少了
  前括号（`6bcdabd` 起，上一版还是好的——一个摇杆/作物批用文件工具改写时吃掉了首行第一个字符），
  PowerShell 在解析阶段就退出 1（docs/55 阶段 0 立下的 soak 判据**坏了五天没人知道**），
  补回之后又倒在第二步——`Get-Content -Raw | ConvertFrom-Json` 没写 `-Encoding UTF8`，
  PS 5.1 按 ANSI 解码 `soak.json` 里的中文面板标题，GBK 尾字节落成 `0x5C` 就成了非法转义
  （与 `scripts/package.ps1:697` 记的是同一个坑）。同一修法给了 `scripts/compare-baseline.ps1`。
  **根因不是这两个文件，是判据工具的调用方只有"验收当天手点的人"**：`check-docs.ps1` 第 ⑦ 项验 BOM，
  首行少一个字符它在字节层看不见，而这类脚本几个月才有人碰一次，坏着也能一直"看起来存在"。
  两道门补上：⑧ 用 `[Parser]::ParseFile` 把 `scripts/*.ps1` 全部**只解析不执行**地读一遍，
  任一失败判红并带行号（故意丢括号的负例验证过确实拦截）；新增 `scripts/test-analyze-soak.ps1`
  把判据的四种判读钉住（稳态退 0、每点 +4 MiB 的真泄漏退 2、谷值不动的大锯齿不得误报、
  坏输入不静默退 0），fixture 写成不带 BOM 的 UTF-8 以复现同一字节布局，并接进 `verify.ps1` 的
  "Soak analyzer self test" 阶段。判据行新增 `ps1_parse_ok=`，命令矩阵同步改。
- **一条口径修正，且是修正不是放宽**：计划里"p95 帧时劣化 ≤3%"在本仓库不可执行——perf harness 的
  UI 场景是 `n=30`，同一份二进制跑三次 p95 的极差就有 24%–95%，门槛落在噪声下面一个数量级。
  判据改成"同机同 profile 各 3 次、比 p50 中位数、劣化不得超过**该场景自身的重复极差**"，
  并补一条引擎侧满实体帧的存档对照。能这么换是因为新判据仍在问同一个问题（超出自己的噪声没有），
  而不是把门槛抬高；旧口径连同为什么放弃它一起写在任务文件里，避免下一个人重新踩。
- **布局鲁棒补到 shell 级**：`layout.rs` 的库层门（缺文件/损坏/v1 升级/类型不符）认得字段，
  不等于编辑器启动后画得出来。新增两条 `EditorApp` 级测试：三种启动态各**真画一帧**并断言回落到
  默认树、六视图齐全、回落后还能再序列化；以及一条**有意的裁定**——比当前构建更高的 `layout.json`
  逐键照读、版本号原样保留，而不是整份判损坏（整份判损坏会让用户升级一次就白丢一遍布局）。
  "首启不许报警、静默丢用户布局那一态必须留一句控制台提示"也是这两条测试钉住的。
- **拖放矩阵逐格登记**（`docs/25` §16.1）：{外部文件、内部资产、prefab、实体、控件}×{画布、层级、
  资源浏览器、Inspector 字段} 每格记"结算动作 / 拖拽期间的角标文案 / 证据测试名"，
  **拒绝格与 Play 锁定格一起记**——文案也是契约的一部分，用户看到的"引用 x.prefab"和实际的
  实例化行为分叉时，只有登记表能发现。prefab 从"资产"里单拆一行，因为它与纹理的差别全在落地之后。
  真实帧补三条断言（实体→行重排、往自身后代上放被拒并说明原因、外部混合批次→行建子节点），
  这三格原先只有 `dnd::plan` 的计划层单测，接线的三个参数（父级、行三条带、环校验）没人验过。
- **UI 框架裁定**（`docs/02` ADR）：**不迁移，保留 egui/eframe**，论据与三条重估触发条件（富动画级
  编辑器 UX 需求 / egui 停维或 wgpu 主线断裂 / wasm 下不可接受的输入缺陷）落文；章程里程碑表登记 M27。
- **两条必须登记的存档事实**（本批只记不改，因为动了就是改质量门）：①
  `artifacts/baselines/editor-baseline.txt` / `editor-gui-smoke.txt` 被 gitignore 排除又被 `verify.ps1`
  **原地重写** ⇒ 每次回归都在销毁上一份基线，"合入前"能重建只因为改动尚未提交，提交之后永久丢失；
  ② `scripts/compare-baseline.ps1` 只认带顶层 `stairs`/`mesh_count` 的 3D 压力报告，把 `wp0-v4` 的
  场景帧报告喂进去会输出 `checks=0` 并**退出 0**——一条会静默放行的门。

**诚实边界**：`DockArea` 的**真实鼠标**抓分隔条/拖标签没有被负载覆盖（churn 是程序化改树，不是真鼠标）；
Gizmo 的观感（环好不好抓、三支环重叠怎么取舍）与框选手感**未经人眼**，本轮自动化只钉得住"画与命中读
同一份函数"；两格拖放仍只有计划层单测（控件→Widget 画布要瓦片地图上下文、Inspector 行 hover 区要按
schema 字段 id 反查行矩形）；长会话的判据是"波谷不外推"而非零增长，短窗锯齿能被误读成增长；
老场景里"度数当弧度"的存量数据仍未扫描修复。帧预算那一边**依赖 M27 尚未提交**这个前提，
提交后同机对照无法重建。用法与规则见 [docs/25](docs/25-editor-guide.md) 第 16 节，ADR 见
[docs/02](docs/02-technical-decisions.md)，逐条数据、口径推导与未完成项见 `tasks/done/M27-03-验收性能与UI框架裁定.md`。

回滚档：本批之前。生产路径零改动（soak 负载只在环境变量存在时生效），回滚不影响编辑器任何行为；
但 `docs/25` §16.1 与 `docs/02` 的 ADR 是**唯一记录"哪些格没被自动化覆盖"**的地方，
删掉它们等于把验收结论一起删掉——下一次"看起来全绿"的判断会失去对照组。

## Unreleased — WP11：`core/script.rs` 单文件拆成 `core/script/` 子目录（行为零变化）+ Lua API 面快照门

主题：`script.rs` 是仓库最大单文件，痛点集中在两处——`register_api` 一个函数覆盖 41 个 Lua 模块表
的全部绑定，测试同样占掉文件的三分之一。这类形态让「重复注册 / 漏注册 / 顺序覆盖」成为固有故障模式
（`input.action` 曾真被注册两次、后者覆盖前者，M22 批 1 才发现），人眼 diff 抓不住。
本次先造守卫再动手：把 API 面（表名 + 每表函数名）固化成基线快照门，然后把文件按 **Lua 模块表 /
运行时职责 / 测试域** 三条轴拆开。拆分是纯搬运，**Lua 侧 API 面与 Rust 侧对外符号一个字节都没改**，
`editor`/`game`/`game_web`/`assetc` 零改动。逐阶段验收证据与偏差记录见
[tasks/plan/WP11-script-rs拆分.md](tasks/plan/WP11-script-rs拆分.md) §13。

- **`core/api_surface_snapshot/`**：长期资产，也是本次的安全网。用 `LUA_API_PROBE` 在真 VM 上枚举
  全局表与函数名，与签入的 `lua_api_surface.json` 逐字节比对；今后新增 Lua 绑定必须显式重跑
  `cargo run -p engine --example dump_lua_api -- --write` 更新基线，等价于一次 API review。
- **`core/script/`**：`mod.rs` 保留 `ScriptState` 与跨模块共享 helper（私有字段对后代模块天然可见，
  因此**属性零改动**，这是把结构体留在原地而非塞进 `state.rs` 的唯一理由）；`error.rs`/`values.rs`/
  `require.rs` 收错误、值转换与 require 链；`runtime.rs`/`scene_spawn.rs`/`step.rs`/`transition.rs`
  按运行时职责切开 `impl LuaRuntime`；`api/*` 一个文件一组 Lua 模块表，`register_api` 变成顺序调度。
- **`api/mod.rs` 的调度顺序逐字保持现状**：各表彼此独立，但 `globals().set` 的先后就是现状语义。
  `ui` 表由三段共享同一个 `Table` 句柄（mlua 是引用语义），只在三段填完后注册一次，Lua 侧无感知。
- **`core/script/tests/`**：测试按域分文件，脚手架与共享 `use` 留 `tests/mod.rs`，域文件只写一行
  `use super::*;`——glob 把祖先的私有项一并带下来（私有项对后代可见且 glob 传递）。测试项数不减。
- `core/mod.rs` 的架构门 `core_does_not_depend_on_host` 现在跳过名为 `tests` 的目录：本门只约束生产
  代码，而拆分后测试是目录形态。
- `docs/37` 顶部加了行号口径说明：台账里的 `script.rs:NNNN` 此后是**历史坐标**，不可跳转；
  `check-docs.ps1` 只校验历史检出路径、不校验源码行号，所以这条门不会变红，纯粹是给人读的约定。
  新引用一律写符号路径（`script::api::save`）。

回滚档：拆分前最后一个提交。回滚只影响可读性与 diff 粒度，不影响任何运行行为。

## Unreleased — web 工具链（zig / wasm-bindgen CLI / wasm-opt+terser）进工程：按版本解析、按需安装、不写死本机路径

主题：webgl 与微信小游戏的构建依赖此前**不在工程里**——zig 的路径被写死成某台机器的绝对
路径（`engine/.cargo/config.toml` 的 `[env] ZIG`），wasm-bindgen CLI 只判断"PATH 上有没有"，
两个压缩工具（`wasm-opt`、terser）同样是单机路径且**找不到就只警告**。换一台机器（或本机装过
旧版 CLI）就编不过，而失败信息被 `cargo ... | Out-Null` 吞掉，看起来像"WASM build failed"而不是
"缺工具链"。本次把三者变成工程自己会解析、会按需安装的依赖。

- **`scripts/zig-toolchain.ps1`**：解析顺序 `$env:ZIG` → `engine\.tools\zig` → PATH → 按需
  下载固定版本 0.16.0 到 gitignore 的 `engine\.tools\zig\`。仓库只留脚本，不留二进制。
  每个候选都要真跑一次 `zig version` 才算命中——**旧口径直接把 config.toml 里的路径交给
  build.rs**，那个路径不存在时表现为 mlua-sys panic `run zig cc: NotFound`；现在这种陈旧
  路径会被识破并跳过，继续往下解析。
- **`scripts/wasm-bindgen.ps1`**：版本事实源改为 `engine/Cargo.lock` 的 `wasm-bindgen` 条目
  （CLI 与 wasm 元数据 schema 必须精确一致），逐个候选校验 `--version`，全不匹配时
  `cargo install --root engine\.tools\wasm-bindgen`——**不污染全局 `~\.cargo\bin`**。
- **`package.ps1`**：web 分支构建前显式设 `$env:ZIG`；`rustup target add wasm32-unknown-unknown`
  补 `--toolchain 1.88.0`（wasm 必须用 1.88 编，而原命令只给默认 stable 装目标，1.88.0 依旧
  报 `can't find crate for 'core'`）；两处 `wasm-bindgen` 调用改走解析出的 exe；
  WASM 构建失败时打印可手动复现的命令与三类常见原因，不再只留一行"failed"。
- **`verify-wasm-lua.ps1`**：删掉自带的第二份 zig 解析（原来读 config.toml 的 `[env]`），
  与 `package.ps1` 共用同一套；质量门只探测不下载，缺 zig 仍按"环境受限跳过（exit 3）"，
  不把跳过说成通过。
- **`scripts/minify-toolchain.ps1`**：发布期两个压缩工具按同一口径管起来。`wasm-opt` 锁
  **binaryen `version_133`**——不是"新版本更好"，而是任意 pass 都可能把 1.88 产物的纯
  `try_table` 改回 legacy `try`（新旧 EH 混合 → V8 拒绝编译），所以候选要真跑 `--version`
  复核、版本不符就当没找到；下载的文件还要对上**写死在脚本里的官方 sha256**（只信官方
  `.sha256` 等于让下载通道自证），对不上就删掉并失败。terser 锁主版本 5，`npm install
  --prefix engine\.tools\terser`。解压显式用 `%SystemRoot%\System32\tar.exe`：从 Git Bash 里
  跑脚本时 PATH 先命中 GNU tar，它把 `C:\...` 的盘符当远程主机名，报
  `Cannot connect to C: resolve failed`。
- **`package.ps1`**：`Optimize-WasmBin` / `Minify-JsFile` 的两处单机路径
  （`$env:USERPROFILE\.tools\binaryen\...`、`$env:APPDATA\npm\terser.cmd`）换成上面的解析器。
  这两个工具不是可选美化：微信主包 4 MB 上限里它们占约 800 KB，**缺席只警告、最后才由体积门
  炸**——实测 2026-10-01 产出的主包 4,660 KB > 4,096 KB。
- **`package.ps1`（编译期体积）**：wasm 构建临时覆盖 `CARGO_PROFILE_RELEASE_OPT_LEVEL=z`
  （用完恢复）。workspace 的 `opt-level = "s"` 不动，pc/editor 一点不变，代价只落在 web 两个
  平台上：**代码速度换体积**，而微信主包 4 MB 门就差这 47 KB。覆盖后 `-Oz` 产物
  2,853,068 → 2,424,558 字节，`wasm_host_ok` 与 `strip_wasm_globals_ok` 两道守卫照旧通过。
- **`engine/.cargo/config.toml`**：移除写死的 `[env] ZIG`，注释里说明现在由谁负责、手工编
  wasm 的人要怎么自备 zig。文档 `docs/32` 同步（此前它教人装 `0.2.127`，而 crate 已是
  `0.2.129`，照着装就把两条 web 发布路径一起挂住）；`docs/52` 里"装 terser"那条手工步骤改成
  工程内按需安装。
- **验证**：`zig-toolchain.ps1` 三种情形实测（无 → `-NoDownload` 返回未找到 / 显式路径命中 /
  陈旧 Administrator 路径被识破后继续解析）；`wasm-bindgen.ps1` 实测拒绝 PATH 上的 0.2.127、
  按 Cargo.lock 装出 0.2.129 到 `engine\.tools`、二次解析 0 秒命中缓存；binaryen 走
  `version_133` 下载→摘要→bsdtar 解压→`wasm-opt --version` 复核，terser 装出 5.51.2；
  微信打包里两者都真跑了（`wasm-opt -Oz`、`strip_wasm_globals_ok`、
  `wasm_host_ok … exports=121 globals=0 bridge_exports=7`、terser 100 KB + 63 KB），主包从
  4,660 KB 降到 4,143 KB——**还差 47 KB**，最后一段缺口出在编译期：`package.ps1` 给 wasm 构建
  临时覆盖 `CARGO_PROFILE_RELEASE_OPT_LEVEL=z`（workspace profile 的 `s` 保持不变，pc/editor
  不受影响），同一份代码 `-Oz` 后 2,853,068 → 2,424,558 字节，主包落到 **3,843 KB / 4,096 KB
  （93.8%）**、`wechat_minigame_package_ok` 过门并完成原子交换；`git check-ignore` 确认
  `engine/.tools/**` 不入库。
  踩到的三个 PowerShell 坑都写进了脚本注释：参数名叫 `$Args` 会和自动变量撞车（splat 变空表、
  版本探测静默失败，于是每次重下 106 MB）；从 Git Bash 起的 PowerShell 里 PATH 先命中 GNU tar，
  它把 `C:\` 当远程主机，必须显式用 `%SystemRoot%\System32\tar.exe`；binaryen 资产解出来的
  顶层目录是 `binaryen-version_133/` 而不是 `version_133/`。
- **回滚档**：`git revert` 本次改动即可；`engine/.tools/` 是纯缓存目录，删掉不影响任何产物
  （代价只是下次重新下载/编译）。手工替代方案是原有的 `cargo install wasm-bindgen-cli
  --version <Cargo.lock 里的版本>` + 自己设 `$env:ZIG`。

## Unreleased — WP10 批次 B/C 宿主 API（web 兼容层）+ 微信侧 HostApi 接线

主题：WP10 剩余三块收口——批次 B（event/data/asset/tag/light/interact/container/shop
编译进 wasm 语义、web 侧以 Lua 补丁 + JS 桥实现，标注"需存储后端与回调桥"）、批次 C
（item/inv/container/shop/map 数据单向喂入，web 补丁区已有 item 表）、微信小游戏侧接线
（wasm-bridge 返回 glue → adapter 实例化 HostApi → 挂 window.__P2_HOST_API）。

- **web 兼容层**（plugins/webgl/web-runtime.js）：methodCompat 模板补齐 7 个宿主表
  （event/tag/light/interact/container/shop/asset；item 此前已有）——event 快照分发
  （回调里再 emit/订阅不影响本次分发）、tag/light 实体状态（__p2_require_alive 同引擎）、
  interact.query 距离升序同距 priority 降序、container 复用背包机制（键 "container:<id>"）、
  shop 目录与 price、asset 句柄（"asset:<path>"）；installRustHostApiBridge 新增 data 表 JS 桥
  （同步 XHR 读 pack/<path>，微信无同步 XHR 平台降级报"不可读"；schema 简化校验
  string/number/integer/boolean/table 支持嵌套）。**执行顺序修正**：methodCompat 先于
  missing 清单生成，避免刚补的表被 __p2_missing_globals 误标为缺失（脚本层判能力零噪声）。
- **微信侧接线**（wasm-bridge.js / wechat-adapter.js）：loadWasmCore 返回 glue
  （wasm-bindgen 导出载体），adapter 在 wasm 实例化成功后 new HostApi（发布名
  jianghu_youtian）挂 window.__P2_HOST_API；状态进 __P2_DIAG().wasm.hostApi，导出缺失或
  实例化失败如实记录（不假装成功、不影响 Lua 启动）。
- **无头门**（scripts/wechat-minigame-harness.js）：模拟宿主 WXWebAssembly（Node 真实
  WebAssembly），新增断言"wasm 核心实例化成功 + hostApi=ok"——微信侧批次 A（app/clock/
  calendar/day）落地的直接证据；ok 行输出 wasm=status:hostApi。
- **验证**：webgl 包 20,827KB/688 文件 verify-webgl-bundle 过（boot=running、
  missing_host_api=0、世界 17 实体、绘制 16130 采样）；微信包 8,467KB/709 文件
  （分包形态，main 44.4%/分包 32.5%）verify-wechat-minigame 全门过（path styles×4、
  分包 6 例、玩法/摇杆位移非零、渲染自证、宿主全局、wasm=ok:ok）。
- **已登记未决**：data 在微信侧无同步 XHR → load 报"不可读"（平台降级，与
  app.project_root 空串同风格）；input.nav_settings/set_nav_settings/last_device 为历史
  遗留缺口（不在本批批次清单，trap 通道如实点名）。

## Unreleased — 引擎侧注册 `ui.scene_text_visible`（与 web 兼容层同步）

主题：`ui.scene_text_visible(name, visible)` 从 web 兼容层（fengari/web-runtime.js）
补到引擎 mlua 侧，消除标题菜单单层文字改动（并行批次，见 a4675c5）在引擎 headless 回归上的 3 项红。

- **引擎实现**（`engine/crates/engine/src/core/script.rs`）：`ui` 表注册
  `scene_text_visible`（false 登记、其余移除）；`ScriptState` 新增
  `scene_text_hidden`（按实体名隐藏集）；`runtime_scene.rs` 的
  `push_scene_ui` 收集 UiText 时按实体名跳过（实体仍在场景与 UiLayout 布局里，
  仅不绘制文字——与 web 端 `__p2_scene_text_hidden` 语义一致）。
- **验证**：单测 `ui_scene_text_visible_toggles_hidden_set`；引擎 lib 891 测试全绿
  （含此前 3 项红 `scene_declared_camera_bounds_refresh_after_scene_switch` /
  `title_scene_stick` / `gamepad_navigation` 复绿，以及示例工程演示覆盖门
  补 `demo_ui.lua` v 键）；cargo fmt / clippy 过。

## Unreleased — WP4 ④ Tilemap chunk 级脏重建（提前实现：触发条件未出现）

主题：docs/40 §3 WP4 ④ 的最后一块——Tilemap 编辑代价从 O(总格数) 降为 O(块数)。
文档口径触发条件为"出现 1000×1000 量级真实地图"（1000×1000 整层重建十几毫秒、远超 <4ms 目标），
核查 game_jhyt 最大真实地图仅 40×22 → 条件未触发，按指示为未来大图铺路提前实现（WP4 待办段与
docs/53 §3.9 已如实登记）。

- **实现**（`engine/crates/engine/src/host/runtime_scene.rs`）：缓存结构由整层 sprite 列表改为
  32×32 格块行主序缓存（`TileChunk`）；收集未命中时逐块比对 `tiles` 行段、只重建
  变化的块（`rebuild_dirty_chunks`）；append 两处（overlay/world）按块行主序消费，
  顶点顺序与块序绑定、像素不变。
- **语义边界**（有单测）：`solid`/`meta` 变化与 `position`/`rotation` 变化零重建
  （不参与 sprite 生成；`center_offset` 相对层原点）；`scale` 变化全块重建
  （`center_offset` 预乘 scale）。
- **验证**：单测 `tilemap_chunk_tests` 7 项全绿（含 1000×1000 改一格只重建 1 块
  `million_cell_edit_is_chunk_bounded`）；引擎 lib 887 测试全绿；wp0_2d_golden
  `tilemap_culling.pixels_identical=true`（剔除开/关像素逐字节相同）；wp0-v4 场景帧契约与
  tilemap_stress 100×100 量尺通过。

## Unreleased — WP10 Phase 1：宿主 API 编译进 wasm（批次 A，缺失 17→13）

主题：docs/32 §4 backlog 4-24 方案①落地第一步——17 个缺失宿主全局开始以**纯 Rust 实现编译进
`game_web` wasm**（fengari Lua 经 `web-runtime.js` 桥接），WebGL/微信两条路径共用同一份运行时。

- **工具链结论**：mlua-sys 的 build 用 `cc` 编 Lua C 源码，`wasm32-unknown-unknown` 无 C 编译器
  → "整引擎 script.rs 进 wasm"不可行；方案① = 宿主 API 纯 Rust 实现进 wasm + fengari 桥
  （设计见 `tasks/plan/WP10-WebGL宿主API编译进wasm.md`）。
- **批次 A 落地**：`engine/crates/game_web/src/host_api.rs`（新建）实现 `app/clock/calendar/day`
  四全局；时钟/日历语义与 `engine::core::gameplay` 逐项对齐（1440 分/天、06:00 醒、28 天/季 × 4 季）；
  wasm-bindgen 导出 `HostApi`；`web-runtime.js` 新增 `installRustHostApiBridge`（wasm 导出缺失时
  回退旧"自报家门"代理，旧包向后兼容）；`index.html` 接线（`new HostApi(publishName)`）。
- **验证**：`HostApi` wasm 冒烟 34 项全过（默认态/跨天进位/日历换算/非法参数拒绝/`day.sleep`
  新一天）；webgl 包重打包后 `missing_host_api` 17→13，`verify-webgl-bundle` 复核；微信侧
  适配器接线与 B/C 批次为后续工作包。

## Unreleased — 微信小游戏真机首跑验收（Android）+ 摇杆链路接入门禁

- **真机验收（2026-09-27，Android）**：启动进山庄、虚拟摇杆移动、按钮交互全部正常。
  README 平台表「真机首次跑通前不作平台承诺」摘除，改为已真机验收。
- **摇杆用例接入门禁**：`verify-wechat-minigame.ps1` 新增 `play-path-touchstick` 用例
  （`--touch-stick-move`：触摸→指针事件→virtual_pad 命中→键注入→动作层→世界移动，
  位移 d≠0 才通过）。此前该链路只能手动驱动 harness 验证，回归会静默漏过。

## Unreleased — game_jhyt docs/48 全部内容缺口清零：特效条带 + 对话立绘 + UI 换肤（93→0）

主题：docs/48 收尾——`effects 8` + `portraits 15` + `ui` 换肤集全部接线，内容缺口 93→0，
等待用户验收（横屏方案 A 与上一批接线均未 commit，本段也不 commit）。

- **特效系统**（新建 `scripts/systems/effects.lua`，8 条带）：短生命周期特效实体
  （`entity.spawn` → 播完自毁，排序层 actor），触发点接 farm 翻土/浇水（hoe_dust/water_splash）、
  combat 玩家攻/受击（blade_arc/dash_streak/hurt_flash/hit_spark）、enemy 受击（hurt_flash）、
  回复药（heal_motes）、毒草采集（poison_motes）。
- **立绘系统**（新建 `scripts/ui/portrait.lua`，15 张）：5 角色 × 3 表情字面路径映射
  （m29 按源码文件名判定引用，故不可用变量拼接），`world/dialogue.lua` 按 speaker+mood
  画到面板上方（96×96 + portrait_frame 128×128），无立绘角色不画。
- **UI 换肤**（新建 `scripts/ui/skin.lua`，75 键注册表）：引擎 `ui.image` 新增可选
  nine_slice 第 10 参（`script.rs`，向后兼容）；hud 接纸面背景/头像框/行图标/状态图标/
  条图标/招式快捷栏（skill_card+5 招式图标）/建造 ghost；背包/任务/商店/暂停/标题各面板
  接面板背景/分隔/页签/滑块/步进/滚动箭头/按钮/印章；`ui_weather_cloud.png` 无天气取值，
  按 `_` 前缀改名归入开发产物（46→47）。
- **验收**：m29 扫描 562 已引用 + 47 开发产物 + 0 内容缺口；m29/m19_ui/m20_audio 等样例
  全绿（M19-02a 的 .wav 断言待用户音频 .ogg 迁移同步，属工作区既有中间态，非本段引入）。
- **docs/48**：基准 609=562+47+0、决定行 effects/portraits/ui 归零、合计 118、§3 加 cloud 改名。

## Unreleased — game_jhyt 内容推进：斧镐工具 + 山道伐木点 + 寒泉洞灶台（docs/48 接线）

主题：docs/48 下一批接线——items 图标 `axe`/`pick` 与内景灶台 `interior_stove` 全部落地，
附带玩法闭环（工具前置采集 + 新档发放），累计接线 22→25、内容缺口 96→93。

- **工具**（`data/tools.lua` + `player/inventory.lua`）：新增「伐木斧 axe」「矿镐 pick」
  （icon 引用 `items/axe.png`、`items/pick.png`），新档直接发放（与 water_can 同先例：
  商店不售工具、无任务奖励）。
- **采集**（`data/gather.lua` + `scenes/mountain_path.scene`）：
  - 矿石点 `wild_ore_1` requires 由 "hoe" 占位改为真实矿镐 "pick"（缺前置 reason=需要矿镐）；
  - 新增松木桩伐木点 `wild_timber_1`（cell 10,4，requires=axe，木料 ×2 日刷），
    山道实体 32 TreeStump（复用 `props/wilds_pine.png`，PineA 东侧，站 (10,5) 面向采）。
- **内景**（`scenes/cold_spring_cave.scene`）：寒泉洞中部铺灶台 Stove（cell 10,6，
  `furniture/interior_stove.png` 48×40），内景家具首次以实体方式接线。
- **验收**：m19_slice 矿石 gate（需要矿镐）+ 伐木点真实 E 键采集（gather_timber ×2）
  + 山道采集点数 9→10；m29 扫描 470 已引用 + 46 开发产物 + 93 内容缺口、决定行 25；
  26/26 样例 + 发行自测全绿。
- **docs/48**：扫描基准/表格/合计同步（items 行拆成 `axe.png`/`pick.png` 具体接线行——
  目录前缀不能标接线，m29 校验要求接线行 is_file）。

## Unreleased — game_jhyt 横屏视觉检查：右栏收窄 400→280 + 交互实体左移（方案 A）

主题：横屏迁移后按用户要求做 6 图实体排布视觉检查（数据层扫描，RuntimeScene 无截图 API，
改扫 Sprite size+Transform position），发现 11 处交互实体被右侧信息栏遮挡；用户拍板方案 A
——右栏收窄到 280px（世界区 0..1000）+ 4 组交互实体左移，全部落盘并过质量门
（26/26 样例 + 发行自测 + 目录隔离）。

- **右栏几何（ui/layout.lua）**：hud_info{880,8,400,124}→{1000,8,280,124}、
  dialogue{880,320,400,240}→{1000,320,280,240}；注释同步「0..1000 / 右侧 280 为信息栏」。
- **实体左移（3 场景 + gather/m19 联动）**：
  - manor：BuildHerbHut(1056,448)→(704,448) cell 22,14、BuildFoundation/BuildScaffold(1056,608)
    →(512,608) cell 16,19；随后消除药庐与练功房重叠：BuildHerbHut→(512,448) cell 16,14
    （与 BuildTrainingHall(640,512) 贴边，重叠 80×48 清零）。
  - village：VillageShop(992,480)→(672,192) cell 21,6、VillageTeaStall(992,320)→(672,384) cell 21,12。
  - mountain_path：BanditCamp(928,224)→(800,224) cell 25,7、LotusPond(1056,160)→(928,160) cell 29,5
    （gather.lua wild_lotus_1 cell 33,5→29,5 + jhyt_m19_slice 莲子采集 teleport(1072,144)→(944,144)）。
  - 重扫（BAR_L=1000）：交互实体遮挡清零，仅剩装饰被盖/越界（方案 A 范围外，竖屏时代遗留）。
- **键位提示通栏（ui/hud.lua）**：右栏 280 物理放不下 11 个键位项（256px 预算折行把
  「J 攻击」拆成两行，m20 键鼠说明断言拦下）→ 键位行改世界底部通栏 1 行
  （x=layout.ROW.top_x、宽 layout.row_width()、max_lines=1），11 项整行放得下。
- **换行分词（ui/text_fit.lua）**：wrap 改为「按标点分词、空格并入词内」——「1/2/3/4/5 直选」
  「J 攻击」这类带空格短标签整体换行不再被拆成两半（m20 面板提示断言）；
  无空格长中文按标点分句断行（m30 负例的宽度/行首禁则/省略号断言全部保持）。
- **对话提示行（world/dialogue.lua）**：HINT_W 放宽到 BOX.w−16=264 + draw_wrapped max_lines 3
  兜底——窄栏下提示完整登记不截断（m20 面板提示「空格 确认 / Esc 结束」断言全过）。
- **验收**：26/26 样例过（m19_slice 采集链路 + m20_gamepad 键鼠/手柄两套说明 + m30 排版审计
  零溢出）；editor release 编译通过（6m32s）+ 发行自测（目录隔离、用户数据目录）过；
  日志 artifacts/logs/quality/verify-20260927-034955.log。
- 回滚档：恢复 layout.lua 几何 + 三场景实体原位 + hud.lua 键位行回右栏 + text_fit/dialogue
  换行逻辑即可（纯数据/脚本驱动，无引擎改动）。

## Unreleased — game_jhyt 转横屏：左世界 + 右侧信息栏（1280×720）

主题：用户把工程从竖屏 720×1280 改为横屏 1280×720（main.scene / project.toml 已手改），本段落
成整套横屏迁移并过质量门——版面几何重排、HUD 右栏适配、全部验收样例视口同步。

- **版面几何（ui/layout.lua）**：设计分辨率 1280×720；hud_info{880,8,400,124}、
  dialogue{880,320,400,240} 右侧信息栏上下排布，modal 四面板{160,40,720,640} 居中偏左，
  title_btn{460,380,360,64} 与 main.scene 三按钮逐字对齐（gap 24→16）；`M.audit()` 三处
  契约（场景 ui_reference + project.toml window == layout.DESIGN）由 layout_check 守住。
- **HUD 右栏适配（ui/hud.lua）**：资源条从"面板上方"移到面板正下方（y0 = PANEL.y+PANEL.h+6）、
  引导行随之移到资源条下方；行宽预算从全宽 row_width 改为 PANEL.w−24（右栏 376px）。
- **玩法场景**：6 张玩法地图 ui_reference 720×1280→1280×720，HudTitle/HudHint 从顶部
  通栏改左上角（24,12,500,36 / 24,40,560,64）。
- **对话面板提示折行**：右栏 400 宽下提示行预算 352px，`ui_text.draw` 裁剪会吞掉
  "空格 确认"（m20 拦下）→ 改 `draw_wrapped` 最多 2 行，完整文案登记。
- **验收同步**：26 处样例 `set_ui_viewport` 720×1280→1280×720；m29 雨遮罩阈值（700/1200
  →1200/700）；m32 相机断言按横屏几何重写（视口宽==地图宽 ⇒ 相机 x 恒 640 恰好铺满、
  y 仍钉 352，clamp_x=[640,640]）；gen_main_menu_scene.py 同步门面坐标与"横版"文案。
- 回滚档：恢复 layout.lua/hud.lua 竖屏几何 + 场景 ui_reference + 样例视口即可
  （数据/脚本驱动，无引擎改动）。

## Unreleased — game_jhyt 内容推进：青云山道茶丛/野果树采集点（docs/48 接线）

主题：docs/48 决定表第二组"做内容时直接接线"资源落成可玩内容——山道新增茶丛/野果树
两个日刷采集点（茶叶 ×2、野果 ×2），配套 2 个材料物品（`tea_leaf`/`wild_fruit`，野采物
不入商店），场景新增 2 个道具格（TeaGrove/WildFruitTree，**贴图复用** `gather/wilds_herb.png`
`gather/wilds_berry.png`，不新增资产文件）。

- **纯数据 + 场景 + 验收接线，零引擎改动**：`data/items.lua` 新增茶叶/野果（`buy=0`、
  `sell=12/8`）；`data/gather.lua` 新增 `wild_tea_1`(19,10)、`wild_fruit_1`(6,16) 两节点
  （`respawn_days=1` 日刷、`minutes` 计入劳作）；`scenes/mountain_path.scene` 追加
  entity 30/31（TeaGrove pos 608,320 / WildFruitTree pos 192,512，`sorting_layer=world;order=22`）。
- **docs/48 契约同步**：已引用 465→467、内容缺口 98→96（开发产物 46 不变）；`jhyt_m29`
  审计常量同步；`jhyt_m19_slice` 山道采集点断言 7→9，并新增两段真实 E 键采集断言
  （`gather_tea_leaf`/`gather_wild_fruit`，含 `E: 采集 茶叶/野果` prompt 校验）；`jhyt_m23`
  item 校验 12→14。
- 回滚档：删除 items/gather 新增条目与场景两实体即可（数据驱动，无代码路径改动）。

## Unreleased — game_jhyt 内容推进：青云山道莲子塘/芦苇丛采集点（docs/48 接线）

主题：把 docs/48 内容缺口决定表里"做内容时直接接线"的两组资源落成可玩内容——山道新增两个日刷
采集点（莲子 ×2、芦苇 ×3），配套 2 个材料物品（`lotus_seed`/`reed`，野采物不入商店），场景新增
2 个道具格（LotusPond/ReedPatch，`gather/wilds_lotus.png`、`gather/wilds_reed.png`）。

- **纯数据 + 场景 + 验收接线，零引擎改动**：`data/items.lua` 新增莲子/芦苇（`buy=0`、`sell=18/6`，
  与狼皮/兽肉"野采不入商店"口径一致）；`data/gather.lua` 新增 `wild_lotus_1`(33,5)、`wild_reed_1`(5,13)
  两节点（`respawn_days=1` 日刷、`minutes` 计入劳作）；`scenes/mountain_path.scene` 在游戏相机后追加
  entity 28/29 两个道具格（`sorting_layer=world;order=22`，与既有采集点一致）。
- **docs/48 契约同步**：已引用 461→465、内容缺口 102→98（开发产物 46 不变）；`jhyt_m29_season_weather`
  审计常量同步；`jhyt_m19_slice` 山道采集点断言 5→7，并新增两段真实 E 键采集断言
  （`gather_lotus_seed`/`gather_reed`，含 `E: 采集 莲子/芦苇` prompt 校验）。
- 回滚档：删除 items/gather 新增条目与场景两实体即可（数据驱动，无代码路径改动）。

## Unreleased — 引擎收口三件套 + game_jhyt 数值/体验收尾（WP9/WP6/4-14/4-15/4-18）

主题：把上一轮复盘里 A、B 两组的可独立完成项全部落地并过质量门。

- **WP9 组件改名弃用告警**（docs/40 §3）：`editor/src/schema_audit.rs` 增 `COMPONENT_RENAMES` 改名表（Transform3D→Transform、Camera2D→Camera、Camera3D→Camera），汇总行 `renamed_components=N` + 明细行 `component_renamed legacy=.. current=.. count=.. hint=migrate_project`；旧组件名先查改名表再进 `unknown`，`editor --check` 对 `game_jhyt`/`game_api` 仍 `schema_errors=0`。
- **WP6 `scene_3d()` 去分配**：收集闭包先 `handle_of` 零分配查找、未命中才 `intern`；`mesh_path` 直接 move 进 drawable，去掉每帧克隆。
- **4-14 数值案收口**（docs/33 ↔ 代码，实现为准）：`jhyt_m24_numeric_alignment` 扩到 ①–⑧ 八段——初始属性（120/50/上限 100）、打坐效果单元格（恢复 5 点内力）、基础刀法（伤害 2 / 内力 0 / 冷却 0.4s）、建筑升级成本（仅木材 4/6/3）、订单奖励区间 [40,100] ⊇ 60/80/90；`docs/33` 已按实现修正（初始铜钱 300→120、内力 60→50、基础刀法 12→2、升级成本 4/6/3）。新增解析助手 `markdown_bullets_after`/`range_from_cell`。
- **4-15 pcall 静默消除**：`main.lua` gather 跨天异常 → `log.warn`；`enemy.lua` `art` 挂载失败 → `report(actor,"art:…")` 可见诊断。
- **4-18 礼物每日限次**：`player/relationship.lua` 新增 `gifted_day`（npc→day）持久化表，当日重复送礼返回 `gift_today` 不扣物不加分，`capture/restore` 存档往返。
- **顺手修一处 HUD 回归**（`3f84072` 引入）：键位提示 13→16px 后单行放不下 11 个键位项，被引擎按宽裁剪出省略号、`I 背包/任务/暂停` 不可见（`jhyt_m20_gamepad` 拦下）——改用 `ui_text.draw_wrapped` 折成最多 2 行，16px 与全量键位都保留，`jhyt_m30` 逐面板审计零溢出。
- 判据：`scripts/verify.ps1 -MvpOnly` 26/26 全绿（含 editor --check 双工程 `schema_errors=0` 与 `jhyt_m31_pc_release` 真实发布目录）；`schema_audit` 6 单测全绿；`cargo fmt` / `clippy --workspace --all-targets --all-features -D warnings` 干净（本轮 `-p engine -p editor -p game --examples` 实测）。

## Unreleased — WP 计划收尾（WP4⑨ 裁定 + WP6/WP9 收口）

主题：`tasks/plan/` 三个活动文件全部收口；纯裁定 + 文档状态翻新，无渲染语义改动。

- **WP4⑨ World2D 分层裁定（达成）**：docs/40 §3 WP4 ⑨ 的三显式层实由 WP6 2D/3D 统一批次落地——
  `RenderLayer::{World2D,Overlay2D,ScreenUI}` + `OVERLAY_DRAW_ORDER` 分桶（层序由渲染器保证）、
  `runtime_scene.rs` 的 `mode=overlay/screen/world` 显式显示模式、`RenderPass::Legacy2DOverlay`
  （World2D+Overlay2D 两桶）+ ScreenUI 独立通道；旧项目默认走 Legacy2DOverlay 且 `wp0_2d_golden`
  逐字节不变，`wp0-hud-golden-v1` HUD 隔离门在 verify.ps1。附带两处同源核对均一致：① Tile 画笔
  行序（编辑器 `row=floor((world.y−pos.y)/cell_h)`、落盘 `index=y*width+x`，行 0 = 世界 Y 最小 =
  屏幕最下方，与运行时约定一致）；② 编辑器 picking 投影（pan/zoom/ppp → `editor_scene_camera` =
  `Camera2D` 实例，与运行时同一份投影）。
- **WP6 收口**：提取路径去分配（8 刀）+ 世界 Sprite 深度通道全部完成并过门；深度通道宿主默认关闭
  为最终态（回滚档即默认态，测试断言「宿主开关刻意保持关闭」），需要世界深度时
  `set_world_sprite_depth(true)` 即可，无需改引擎。
- **WP9 收口**：迁移命令写回经核实无需施加（40 场景仅 1 处单位阵父子关系，迁移与否世界位置不变，
  CLI 三命令随时可用）；组件改名弃用告警已做（见上段）。
- 回滚档：三处均为计划/文档状态翻新，无代码语义改动；深度通道回滚 = 开关关闭（默认）。

## Unreleased — WP 帧时间对照（docs/40 §9.2 正式基线）

主题：把 WP6 的帧时间收益从「单次/三次记录」升级为 §9.2 门槛下的正式基线。

- **3 × 30 采样基线**：`wp0_scene_frame --cycles 30` 串行 3 次（release、1280×720、并行 auto），
  与 WP0 原始基线（docs/53）对照：Sprite 1k **−31.5%**、10k **−74.2%**、50k **−79.0%**（p50 中位），
  均远超 5% 门槛，WP6 八刀累计帧时间收益成立；10k/50k 档跨次 spread 4.6%/5.3%，显著低于
  §3.5 记录的 ±15–25% 单次噪声。
- **分配可复现**：`alloc_count_p50` 三档均 82/帧（与精灵数无关）、`alloc_bytes_p50` 三次逐字节一致
  ——「2D 帧内基本零分配」不再是单次读数。
- **如实登记**：本次测量高于第八刀单次记录（208/1612/9688），归因跨运行噪声与测量时段负载，
  代码帧路径无回退性改动；历史单次值不作基线，正式基线 = 本次 3×30 采样。
- 产物：`artifacts/baselines/wp0-scene-frame-20260926.json` / `-r2` / `-r3`；今后变更对照以此为准。

## Unreleased — 编辑器 Inspector 的 schema 路径量尺（批 10：把最后一条"估算"变成"测量"）

主题：相机系列（批 1–9）里留下一条长期"有意未做"的偏离——**"编辑器 Inspector 的每帧解析不做
缓存"**，理由是"量级是每帧常数、**约微秒**"。那是**估算**，而仓库的规矩是"只判测量是活的"。
这一批不猜：给这条路径配一把可复跑的量尺，把估算换成数字，并顺手发现**钱不在被命名的那一处**。

- 新场景 `inspector_schema_path`（`editor --perf-baseline`，进门时会跑）：把同一条 Inspector
  schema 路径**拆账**——`camera_notes`（只读说明：解析 `Camera`/`Transform` 载荷 + 若干
  `format!`）vs `rows_for_selection`（逐字段行构建）vs **纯解析**本身。
- **实测**（同一台机，两种 profile）：**release**：`Camera::from_component_str` = **565 ns**、
  `camera_notes`（`pose=transform`，最重分支）= **1 570 ns**、`rows_for_selection` = **24 879 ns**；
  **debug**（`verify.ps1` 走的正是它）：`7 286 / 16 792 / 154 156 ns`。⇒ 解析只占行构建的
  **2.3%（release）/ 4.7%（debug）**。
- 结论（决定了偏离保持不变）：`camera_notes` 每帧只对**主选中实体**调一次，整条 1.6 µs
  （release）/ 16.8 µs（debug）、其中解析 0.57 / 7.3 µs——把它缓存起来，省下的量与"按键比较"
  的成本同量级（键必须含载荷文本，否则正是那条偏离里担心的陈旧风险）。**因此仍然不做**，
  但现在这个"不做"有数字、有可复跑的量尺。
- 顺手更正了两处口径：①原偏离文字写的"选中 N 个实体则 N 次"不准确——`schema_component_notes`
  只取 `selection.primary()`（实测一次）；②这条路径上真正的开销是**行构建**（每帧 25~154 µs，
  逐字段 `String` 分配 + schema 查表），它与"解析"是两种不同形状的问题：真要优化得按
  "载荷文本 + 选择 + 资源索引版本"做键（含失效契约），不是顺手加个备忘录——留作独立批次。
- 场景自带判据（`ok`）：解析与只读说明都必须**远小于**行构建（`parse × 10 < rows`、
  `notes × 3 < rows`）。**刻意不钉绝对时间**：时间是构建与机器相关的量（debug 比 release 慢
  7~8 倍，同一次 debug 跑里 p95 就能到 15 ms、max 68 ms）——第一版判据写成
  `notes < 10 µs`，在 `verify.ps1` 的 debug 路径上当场假红，这一条就是这么改回来的。
  判据一旦不成立（例如有人把载荷解析搬进逐字段循环），场景立刻变红。
- 判据：`editor --perf-baseline` → **debug 与 release 双 profile** 均 `scenarios=32 failed=0
  result=ok`；引擎 854、编辑器 176 全绿；`cargo fmt --check` / `clippy -D warnings` / `check-docs`
  干净；编辑器预览 golden 与批 9 逐值相同（本批不改任何运行时行为，只加量尺与文档）。

## Unreleased — 相机模块拆分与每帧烘焙（批 1–9）

主题：`core/camera.rs` 曾把四件事装在一个 2565 行的文件里——相机类型与数学、组件串编解码、
2D 跟随策略、每帧渲染视图契约，外加 3D 几何。同时**同一组相机量被逐精灵反复重算**：
`Camera2D::world_to_screen` 每次调用都要 `validate()` + `constrained_center()`（内部再算一次
`sin_cos`）+ 自己的 `sin_cos`，而 `Sprite::quad` 每个精灵调 4 次、剔除再 1 次、一帧还要走 3 遍
overlay 构建；瓦片层为此写了一份**与 `Camera2D` 重复的** `TilemapCamera`，世界文字则是
**每个文字实体**各算一次 `view_projection` + `Mat4::inverse()`。

### 批 1：按职责拆成 `core/camera/`（纯搬家，零行为变化）

- `camera/{mod,component,codec,controller,selection,view,error,bake,tests}.rs`：类型与数学 /
  组件串编解码 / 2D 控制器策略 / 选择规则 / 每帧视图契约 / 共用错误 / 每帧烘焙 / 测试。
- **公开路径一字不变**：`camera/mod.rs` 统一 `pub use`，因此引擎内 26 个文件与编辑器/示例的
  调用点**零改动**（这正是"纯搬家"的判据）；判据是既有相机测试一条不改地通过。

### 批 2：3D 几何外迁 `core/geom3d.rs`

- `Ray3` / `Frustum` / `Plane` / `FrustumClassification` 搬去 `core/geom3d.rs`（纯几何：消费方是
  `core::picking`、编辑器 gizmo 与渲染剔除）；`core::camera` 保留兼容 re-export，新代码直接
  `use crate::core::geom3d::…`。
- `Frustum` 字段保持私有并新增 `Frustum::from_view_projection`：视锥只能由某份视图投影矩阵导出，
  "随手塞一个矩阵进去"从此不可表达。

### 批 3：每帧烘焙层（性能）

- 新增 `camera::CameraView` 与 `camera::OrthoTerms`：把"只依赖相机 + 视口"的量（中心、`sin/cos`、
  zoom、可见域、视图投影矩阵）**每帧算一次**，之后逐点换算只剩乘加，且**不可失败**
  （非法相机在烘焙时就被拒，而不是在逐精灵路径上每次重判）。
- 接线：`Sprite::quad_with_terms`（逐精灵 4~5 次 `validate` + 三角函数 → 0）、
  `SpriteBatch::build*`（每帧烘焙一次；公开签名不变，冷路径照旧）、瓦片层（**删掉
  `TilemapCamera` 那份重复数学**）、世界文字（每个实体一次求逆 → 每帧一次）、2D/3D 调试线框、
  `picking::{screen_from_view, screen_from_view_unclipped}`、`active_camera3d`（每帧 2~3 次
  `Vec<(u64, Camera3D)>` 分配 + 全实体扫描 → 单趟、零分配）。
- 语义保持：非有限变换仍然"保留并走原报错路径"，不被剔除静默吞掉（这条在替换 `world_to_screen`
  的 `Result` 时显式补了守卫）。

### 批 4：运行期也只有**一个**相机组件

- 世界存储从"按投影分支落进 `Camera2D` / `Camera3D` 两个类型"改为**只存 `Camera`**；
  `CameraComponent` 因此更名 `Camera`（与 docs/54 §4.2 蓝图同名），消费方按 `as_orthographic` /
  `as_perspective` 取值，`as_perspective_mut` 让"只有透视相机能被 `camera.set` 改写"由**类型**保证。
- `camera.set_active` / `camera.get` / `camera.set` 照旧**拒绝正交相机**（错误串不变）；
  `declared_camera_2d_in` 与 `active_camera3d` 改为单趟遍历那一个存储。

### 批 5：`RenderView` 不再自带一份矩阵

- `RenderView` 内部持有本次烘焙的 `CameraView`，公开字段 `view_projection: Option<Mat4>` 移除，
  改为 `matrix()` / `baked()`："投影参数与矩阵互相矛盾"从类型上不可表达。
- 宿主改清屏/绘制顺序走 `with_clear` / `with_order`（不再用结构体更新语法改框架字段）。
- 退化相机仍可表达（`RenderView::degenerate`）：它正是渲染器"声明了视图却没有矩阵"那条历史
  回退分支的触发条件。

### 批 6：宽高比归**视口**（`Camera3D::aspect_ratio` 降级为"无视口时的回退值"）

- 病灶：宿主每帧把窗口比例**写回** `Camera3D::aspect_ratio`，于是"组件里存的比例"与"渲染/拾取
  实际用的比例"是两份状态——读代码的人分不清某个时刻哪一份是权威；而 `picking`（Lua
  `camera3d.*`、编辑器点选、调试线框）过去直接读组件字段，一旦宿主忘了写回，画面与拾取就会
  按不同比例各算各的（症状是"点中的位置和看到的位置差一点"，且没有任何报错）。
- 新增 `Camera3D::{projection_matrix_for, view_projection_matrix_for}(aspect)`；
  `CameraView::from_camera3d` 改用**视口**比例烘焙矩阵并新增 `CameraView::aspect()`。
  成帧后 `Camera3D::aspect_ratio` **不参与**，它只服务**没有视口**的调用方：宿主未声明
  `FrameViews` 时的历史回退、光照/阴影相机（`light_view_projection` 只用姿态矩阵）、
  以及只做姿态变换的路径。`Camera3D::projection_matrix()` / `view_projection_matrix()` 保持原签名
  （= 用组件自带比例），因此这些调用方一行未改。
- 拾取同源是**顺带**拿到的：批 3 已把 `picking::{screen_from_world, screen_from_world_unclipped,
  ray_from_screen}` 改成内部烘焙 `CameraView`，所以这一次改 `from_camera3d` 就让"渲染用的矩阵"与
  "拾取用的矩阵"同时只由视口决定。
- **判据（先钉基线，再改，再对拍）**：改前先把 8 份 golden 报告跑一遍存到
  `artifacts/aspect-before/`，改后再跑一遍存 `artifacts/aspect-after/`，逐值比较
  **721 项语义/像素字段全部相同**（含 `wp0-lighting-golden` 的 `lit/shadow` 采样像素
  `[143,155,172]` / `[55,60,67]`、`wp0-mixed-golden` 的遮挡采样、`wp0-hud-golden` 的分层像素、
  3D golden 的几何计数）。唯一出现的差异是**计时与分配计数**，且已用同版本重跑两次证明它们是噪声
  （`wp0_3d_stress` 的 `cpu_alloc_count` 在**同一份二进制**上重跑得 24570/24572/24575，
  而最大的那一档四次都是 bit 相同的 1807361）。
- 引擎测试 **843**（842 + 新增 `camera_view_takes_aspect_from_the_viewport`）全绿：这条测试断言
  "成帧矩阵 == 按视口比例算的那一份"且"≠ 组件自带比例的那一份"，并顺带钉住屏幕中心射线指向
  相机目标。

### 批 7：宽高比的唯一权威落到**类型**（`Camera3D` 里不再有比例字段）

- 病灶（批 6 只治了一半）：批 6 让**成帧**走视口，但 `Camera3D::aspect_ratio` 还在，而且宿主
  每帧仍往它里面写窗口比例（`scene_3d_with_camera` 的 `c.aspect_ratio = aspect`、
  `active_camera3d` 结尾的 `camera.aspect_ratio = aspect`）；同时 `projection_matrix` /
  `view_projection_matrix` / `world_to_ndc` / `ndc_to_world` / `ray_from_screen` / `frustum`
  这六个 API 仍按**组件自带**比例算。于是"组件里存的比例"与"渲染/拾取实际用的比例"依旧是两份
  状态：`picking::ray_from_screen` 一边收 `viewport`、一边从相机取比例，两边恰好相等**只因为
  宿主记得写回**——忘了写回就是"点中的位置和看到的位置差一点"，而且没有任何报错。
- **判据（这个字段能不能删）**：它**从来不在载荷文法里**——`Camera3D::from_component_str` 只读
  `position/target/fov/near/far/pose/enabled`，`to_component_str` 一个字节都不写它（新测试钉住
  "组件串里不出现 `aspect`"）。所以删字段对场景/Prefab/Lua 组件串、编辑器 schema 与
  `builtin_schemas_match_engine_encoders` / `shipped_data_uses_only_unified_components` 零影响。
- 落法：删字段；`Camera3D::perspective` 去掉形参；六个矩阵/换算 API 全部改成**接受
  `Viewport`**，并新增 `Viewport::aspect()` 作为全仓库**唯一**的比例推导处 ⇒
  "拿相机自带的比例算矩阵"这件事**不可表达**。宿主两处写回随之删除，
  `scene_3d(aspect)` → `scene_3d(viewport)`、`scene_3d_with_camera(aspect, …)` →
  `scene_3d_with_camera(viewport, …)`。
- 回退档的语义没变，来源变硬：`Scene3d` 现在带 `view: RenderView`（**提取处**按视口成帧），
  于是"宿主未声明 `FrameViews`"时渲染器吃 `scene.view.matrix()`、视锥剔除吃
  `scene.view.baked().frustum()`；宿主声明主视图时（`game_app` / 编辑器 Scene 视图）直接用
  同一份 `scene3d.view`——同一个视口只烘焙一次，声明与回退不可能给出两个比例。
- **逐条说明"哪些调用方的 aspect 是有意与视口不同"：一个都没有**。①光照/阴影相机
  （`light_view_projection`）只消费 `view_matrix()`，它自己拼正交式投影，比例从来不参与——
  那个 `1.0` 是没人读的占位，随字段一起消失；②编辑器取景相机的"面板比例"**就是**它的视口
  （`render_viewport(render_w, render_h)`；`project_to_screen` 按 `rect` 的浮点尺寸建视口，
  与旧公式同一个比例、不经整数化）；③`Renderer::new` 的首帧 uniform 初值取目标视口（0 尺寸
  仍是被拒绝的非法输入，与旧版"比例非法即报错"同一结果）。
- **判据（先钉基线，再改，再逐值对拍）**：在 pristine HEAD（`518acc4`）上把 8 份 golden 跑一遍
  存 `artifacts/camera-viewport-before/`，改后同机再跑存 `artifacts/camera-viewport-after/`，
  用新增的 `scripts/compare-golden.ps1` 递归摊平叶子后对拍 **1353 个字段**：**语义与像素 0 处
  不同**（含 lighting 的 `lit=[143,155,172]`/`shadow=[55,60,67]`、mixed 的遮挡采样、hud 的分层
  像素、3D golden 的几何计数与编辑器预览的 `painted=65536`），另有 543 个叶子落在白名单噪声键
  （计时、时间占比、分配**计数**）。
- 白名单外只剩 6 处：`wp0-3d-stress` 三档的 `cpu_live_bytes` / `cpu_peak_bytes`。它们是 docs/54 §7
  的红线，因此**刻意不加进白名单**（要看见，不能被静默放行），改由同二进制重跑证明它们是噪声：
  n=100 档的 `cpu_live_bytes` 在 **同一份二进制**上得 `517696 / 551386 / 534849`（±3%），改前那次
  `517128` 落在这个区间里；n=10000 档在 repeat-2/repeat-3 上与**改动前**逐位相同
  （`cpu_alloc_bytes=337605858`、`cpu_live_bytes=136`、`cpu_peak_bytes=5167623`）。仓库自己的门
  `compare-baseline.ps1 -GeometryOnly`（几何量 + 内存峰值 ±10%）对提交进版本库的基线给出
  `baseline_compare_ok checks=12 regressions=0`。
- 该脚本自带正/负向自测：把一个采样像素 `[143,155,172]` 改成 `[144,…]` → 判红并点名
  `root.samples.lit[0]`（退出 1）；只改 `p95_us` / `alloc_count_p50` → 放行（计入 ignored_noise）。
- 引擎测试 **844**（843 + 新增 `aspect_ratio_only_comes_from_the_viewport`）全绿：新测试钉住
  ①组件串往返里没有 `aspect` 字段、②同一台相机换视口就换矩阵（`view_projection == projection ×
  view`）、③`Viewport::aspect()` 就是宽高比；`scene_3d_with_camera` 那条改成**负向断言**
  （相机载荷与传入值逐字段相同 + 视图比例来自视口 + 覆盖前后视图矩阵一致）。编辑器 283、
  `assetc` 176 亦全绿；`cargo fmt --check`、`clippy -D warnings`、`git diff --check`、
  `game --check`、`editor --check`（game_jhyt 与 game_api，`schema_errors=0`）、`check-docs`
  （`ps1_bom_ok` 含新脚本）全通过。
- 兼容契约未动：载荷文法、Lua 19 个相机函数的名字/参数/错误串/返回表形状（`camera3d.get()`
  本来就不返回比例，返回的是 `width`/`height`）、`fov` 用度、`math2d::CoordinateError` 的
  re-export 全部逐字不变；改的是 Rust 侧的签名与**谁拥有比例**。

### 批 8：**帧级 3D 视图**（同帧只解析一次生效相机）+ 烘焙次数账本

- 病灶（批 7 之后剩下的那条结构性重复）：同一帧里"生效 3D 相机"被解析**两次**——`vertices()`
  解析一次回填给 Lua（`camera3d.*`），`scene_3d_with_camera()` 再解析一次给渲染。两份结果一致
  靠的是"同帧两步之间没有代码改过世界"这个**隐含约定**：宿主只要在两步之间插入一次步进或
  拖拽预览，画面与拾取就会按两台不同的相机各算一套，而且没有任何报错——与批 7 治好的
  "同一事实两处算"是同一类病。
- 落法：`RuntimeScene` 持一个**帧级 3D 视图**（`FrameView3d { camera, viewport, view }`）。
  `vertices()` 是帧边界：清上一帧的值 → 解析一次 `active_camera3d()` → 烘焙一次 `RenderView`
  → **同一份**同时给 Lua 回填与 `Scene3d.view`；3D 调试线框改走 `frame_camera_view()`。
  缓存键含视口：**视口不同只重新烘焙、不再解析相机**（相机不依赖视口）。宿主没走 `vertices()`
  就先调 `scene_3d`（示例与 golden 里就有这种顺序）时没有定帧值 ⇒ 按需解析，行为逐字节不变。
- **失效这一步才是"不变量"的牙齿**：`run_script_steps`（脚本可能换选中相机 / 移动父链 / 换场景）、
  `set_local_transform`、`set_scene_entity_transform_preview` 之后定帧值作废。用一个有名字的
  `invalidate_frame_view_3d()` 而不是撒一行 `= None`——漏一处就是一条静默的陈旧路径，名字让
  "这里发生了相机相关的世界变更"写在调用点上。代价也写明：作废后再消费要多一次 O(#相机) 解析
  （账本上是 2 次而不是 1 次），这是"宁可多解析一次，也不拿步进前的相机渲染步进后的世界"。
- **新量尺**（`core/camera/ledger.rs`，`#[cfg(test)]`，release 里整个模块被编译掉，线程局部计数
  以免疫测试并行）：把"一帧里相机被解析/烘焙了几次"变成**确定的数字**。它是**口径断言**，不是
  性能断言——测试里的时间不可信，时间的判定仍归 `wp0_*` 基准与 `compare-baseline.ps1` 的阈值。
- 账本给出的**实测口径**（一帧典型路径：`vertices()` → 宿主声明 overlay 视图 → `scene_3d()`，
  场景含精灵 + 瓦片层 + 世界文字 + 透视相机与网格）：
  `camera3d_resolutions=1`、`view_3d=1`、`view_2d=1`、`ortho_terms=4`。
  这同时更正了批 3 那句"宿主每帧烘焙一次"：现实是**每个消费者每帧各烘焙一次**——2D 的 4 次分别
  是 overlay 批、瓦片层、世界文字、宿主的 overlay 视图。
- **有意保留（附数字与理由）**：2D 那 4 次**不做**帧级共享——单次烘焙是常数级（`validate` +
  `sin_cos` + 几次乘法），4 次远低于一帧预算，而共享要改 `SpriteBatch::build_*` 的公开签名，
  收益与风险不成比例；编辑器 2D 逐点投影（面板里的 `Camera2D::world_to_screen`）同样保留：
  那不是发布路径，随编辑器批次处理。
- 判据（引擎测试 844 → **850**）：①`one_frame_resolves_the_active_3d_camera_once`（一帧只解析
  一次，且 `Scene3d.view.matrix()` 与定帧值逐位相同）；②`a_different_viewport_rebakes_without_
  resolving_the_camera_again`（换视口只重烘焙：`view_3d=2` 而 `camera3d_resolutions=1`）；
  ③两条**负向**门 `local_transform_write_invalidates_the_frame_view` 与
  `script_steps_invalidate_the_frame_view`（定帧值不得盖过此后的世界变更）；
  ④`scene_3d_without_a_frame_still_resolves_on_demand`（golden 那种"不先 `vertices()`"的顺序）；
  ⑤账本门 `frame_bake_ledger_is_pinned` 把上面那组数字钉住——任何增减都得有人解释是有意还是回退。
- 8 份 golden 与批 7 之后逐值对拍：**语义/像素 0 处不同**（`scripts/compare-golden.ps1`
  `-Current artifacts/batch8-after -Previous artifacts/camera-viewport-after`；1353 个叶子，白名单外
  仍只有 `wp0-3d-stress` 那 6 个 `cpu_live_bytes`/`cpu_peak_bytes`）。这一跑还**顺手加固了批 7 的
  噪声结论**：n=10000 档这次得 `cpu_live_bytes=136`、`cpu_peak_bytes=5167623`，与批 7 的
  repeat-2/repeat-3 **以及 pristine HEAD 那次**逐位相同——同一个数字跨三种代码状态出现，
  更说明"clean 值"是确定的，差异来自运行环境而不是代码。

### 批 9：**"谁生效"收成一条选择链**（`camera.set_active` 认任何相机，投影决定服务哪个 pass）

- 病灶（批 8 之后唯一剩下的架构级分叉）：3D 的"谁生效"是 ECS 选择（`ScriptState.active_camera`，
  Lua 可切），2D 的"谁生效"是宿主所有权（`RuntimeScene.camera` + 控制器请求/回填链），
  **脚本够不着"2D 用哪台"**——场景里声明的第一台正交相机永久生效，换台只能靠改场景或宿主代码。
  同一条"哪台相机生效"的问题因此有两个答案，`camera.set_active` 还明确**拒绝**正交相机。
- 落法：只留**一个** `active_camera`。`camera.set_active(id)` 只要求实体带 `Camera`，
  **由 `projection` 决定它服务哪个 pass**：透视 → 3D world 取景；正交 → **2D 相机的来源**
  （宿主在选中项变化时采用它的载荷 `center`/`zoom`/`rotation`/`bounds` 一次作为 2D 取景基准）。
  `camera2d.set_active` 这类"第二条选择入口"**不做**——那只会把分叉换个名字。
- 每 pass 仍**按投影过滤候选**，`enabled=false` 的选中项**告警并回退**到下一台可用相机
  （2D 与 3D 同一规则、同一告警形状）；`camera.get` / `camera.set` 仍只认透视 payload。
- 与 3D 侧**唯一的不对称**（有意，且是 2D 多了控制器这一层的结果）：**选中的是透视相机或什么
  都没选时，2D 取景保持现状**——切换 3D 相机是常见操作，它不该把 2D 相机弹回作者写的初始值；
  同一台重复选中也不重复采用载荷（否则 `camera2d.follow` 与 `shake` 会被逐帧抹平）。
- 示例工程同步：`examples/game_api/scripts/demo_camera.lua` 把"正交相机不在这条选择链上"那段
  旧说法改成新语义，并新增 **B 键**（把 2D 相机切回场景声明的正交相机）——这也是新能力在示例里
  的真实调用点。
- 判据：4 条新测试——`camera_set_active_takes_any_projection`（正交/透视都能选中；无相机实体报新
  错误串；`camera.get`/`camera.set` 仍拒绝正交）、`camera_set_active_on_an_orthographic_camera_
  adopts_the_2d_payload`（采用一次、同一台不重复、切 3D 相机不动 2D）、`camera_set_active_on_a_
  disabled_2d_camera_falls_back`（禁用回退）、`camera_selection_serves_each_pass_by_projection`
  （投影分工的**双向**负向断言）。引擎测试 **850 → 854**；`sample_project_demos_cover_every_lua_api`
  等 14 条示例工程门全过（含 `sample_project_runs_every_scene_headless` 跑新 demo）；
  8 份 golden 语义零差异；MVP 头曼回归全过。

### 兼容契约（变更：`camera.set_active`，本系列第一次）

其余 18 个相机函数与全部场景/Prefab 文法**未动**；改的只有 `camera.set_active` 这一条：

- **放宽**：选中一台**正交**相机不再报 `entity has no camera3d`，而是成为 2D 相机的来源。
  （批 9 之前它必须报错，因为那时"2D 用哪台"根本没有选择链。）
- **错误串**：对**完全没有 `Camera` 组件**的实体，错误串由 `entity has no camera3d` 改为
  `entity has no camera`——"一条选择链"下前者已经不是真的。`camera.get` / `camera.set`
  （透视 payload 的读写）**保持** `entity has no camera3d` 逐字不变。
- 影响面：仓库内没有任何脚本/测试依赖"选中正交相机报错"这条行为（示例工程与全部测试已核对），
  `docs/18` 的 `camera` 表与 `camera2d` 小节、`docs/54` §11.2/§12 同步改写。

### 有意保留的偏离（已评估、未做，附理由）

- **编辑器 Inspector 的每帧解析不做缓存**：`schema_fields::camera_notes` 每帧解析一次 `Camera`
  载荷（**只对主选中实体一次**，不是"选中 N 个就 N 次"），量级是**每帧常数**。批 10 已把这条从
  估算变成测量（见上方「编辑器 Inspector 的 schema 路径量尺」一节）：解析本身 **604 ns**、
  `camera_notes` 整条 **1.68 µs**；而同一帧同一条路径的 `rows_for_selection` 是 **26~53 µs**，
  其中解析只占 **2.3%**——所以"缓存解析"省下的是亚微秒，键比较的成本与它同量级，仍然不做；
  真要动的是行构建，形状完全不同（需要按 载荷文本 + 选择 + 资源索引版本 做键，另外成批）。
- **宿主未声明 `FrameViews` 时**（`preview_session` 与示例 golden 的回滚档）渲染器仍不自己造
  视图，而是吃 `Scene3d.view`——那是**提取时**按视口烘焙好的同一份契约（批 7）。这条路径仍是
  "不调用 `set_frame_views` 即回到收敛前"的保证，但它不再是"渲染器回查相机"：矩阵与视口都来自
  那一次烘焙，而"未声明"（回退可用）与"相机退化"（`baked() == None`，保持上一帧矩阵并告警）
  两种语义仍然分开。**批 6 里"这条路是 `Camera3D::aspect_ratio` 唯一还有实际作用的地方"这条
  偏离随批 7 消失**：那个字段不在载荷文法里，已整个删除。

### 兼容契约（未动）

场景 / Prefab / Lua 的组件名与载荷文法、`fov` 用度、`enabled=false` 只在禁用时写出、
`pose=transform` 只在开启时写出、`math2d::CoordinateError` 的 re-export、Lua 19 个相机函数的名字/
参数/错误串/返回表形状，全部逐字不变；`shipped_data_uses_only_unified_components`、
`builtin_schemas_match_engine_encoders`、`editor --check`（`schema_errors=0`）与
2D overlay / 3D semantic golden 照旧通过。

批 7 删掉 `Camera3D::aspect_ratio` 之所以不动契约：**它从来不在载荷文法里**（组件串只有
`position/target/fov/near/far/pose/enabled`，编码器不写它、解析器不读它），也从未出现在
Lua 返回表（`camera3d.get()` 给的是 `width`/`height`）。它是纯运行期暂存值——正是"宿主每帧写回"
才让它看起来像配置。

## Unreleased — 2D 相机跟随与取景边界（场景声明的相机不再只在启动场景生效）

主题：`examples/game_jhyt` 的相机此前是**默认相机**（中心 `(640,360)`、`zoom=1`、无 bounds）——
没有任何场景声明相机，Lua 侧也没有一处 `camera2d.follow`，因此"相机跟随角色"从未真的接线。
现在相机跟随角色、角色始终在视野正中，且取景由场景声明的 `bounds` 锁在地图矩形内。
设计取舍与竖屏下的几何事实见
[`examples/game_jhyt/docs/52`](examples/game_jhyt/docs/52-江湖有田-竖版720x1280改造.md) §4.1，
发现过程与引擎侧缺陷见
[`examples/game_jhyt/docs/34`](examples/game_jhyt/docs/34-引擎能力缺口与执行backlog.md)（对齐记录 1.0）。

### 引擎：修掉"场景声明的相机只在启动场景生效"

- `process_scene_transition` 会重建整个世界（`ScriptState::world = World::new()` 后重新实例化），
  因此换图后场景声明的 `Camera` 是**新的一份**；而宿主只在 `RuntimeScene::init` 读过一次
  `declared_camera` ⇒ 换图后 `self.camera` 的 center/zoom/**bounds 全部停在旧值**。
  症状隐蔽：每张地图各声明一份 bounds 的工程里，只有开工那张图受约束（没有报错，只是画面不对）。
- 新增内置固定步系统 `scene_camera_sync`（与 `scene_transition` 同格、显式依赖它），换图后重读
  新场景声明的相机并把 **bounds** 同步给宿主；**不取 center/zoom**——换图时相机姿态的权威仍是
  每帧的 `camera2d.*` 请求，重置回新场景的作者中心会在换图那一帧闪一下。新 bounds 在同一个
  固定步内就被 `Camera2dController::resolve_camera` 夹取，渲染看不到越界的一帧。

### 引擎：新增 `app.project_root()`，修掉"跨文件契约校验器静默空跑"

- Lua 此前**没有任何办法**拿到工程根：`io.open` 的相对路径按**进程工作目录**解析，而
  `game --check` 并不 chdir 到工程根（在 `engine/` 下跑
  `--check --project ../examples/game_jhyt` 时 `io.open("scenes/main.scene")` 返回 nil，
  `debug.getinfo` 又被 Lua 沙箱剥掉）。于是所有"读 `scenes/*.scene` 或 `project.toml`
  再核对"的领域校验器都**静默跳过**——`examples/game_jhyt` 的 `ui/layout_check.lua`
  正是如此：它的设计分辨率契约（场景 `meta ui_reference_*` / `project.toml` 的
  `window_width/height` 必须等于设计尺寸）**从未真正跑过**，两个门都是恒绿。
- 新增 `app.project_root()`：返回工程根的绝对路径（正斜杠；未配置时为 `""`，Web 一类没有
  文件系统的宿主据此跳过）。两个校验器改用它拼绝对路径，其余口径不变（`io` 缺失仍静默跳过）。
- **有效性证据（负向测试）**：把 `scenes/manor.scene` 的 `ui_reference_width` 改成 700，
  `game --check` 现在报 `[layout] scenes/manor.scene:meta ui_reference_width/height = nil×1280，
  应为 720×1280`；把相机 `bounds` 的 max_y 改成 700，报
  `[camera_view] scenes/manor.scene:Camera bounds = 0.0,0.0,1280.0,700.0`。改回即 `game_check_ok`。
- 门：`app_exposes_the_project_root_for_cwd_independent_file_checks`（含"未配置时返回空串"）；
  `examples/game_api` 的 `demo_core` 补上真实调用点，满足 API 覆盖率门。

### 游戏侧（`examples/game_jhyt`）

- 8 个场景各自新增一台 `GameCamera` 实体，声明
  `Camera projection=orthographic;center=<地图中心>;zoom=1;bounds=0,0,W*32,H*32`；
  由生成器 `tools/pixel_kit.camera_line` 单点产出（改地图尺寸只需重跑生成器）。
- 新增 `scripts/world/camera_view.lua`：每次进图后 `camera2d.follow(player)`，offset/死区/平滑
  全 0 ⇒ 玩家始终在视野正中；回门面/新档 `unfollow`（不留指向已销毁实体的跟随目标）。
- 新增数据门 `world.camera_view.validate_all`（登记进 `scripts/validate_data.lua`）：每张玩法地图
  恰好 1 台正交相机、`zoom=1`、且 `bounds` 逐值等于该场景 GroundMap 的真实地图矩形——这条契约
  横跨 Python 生成器 → `.scene` → 引擎夹取三层，漂移的表现同样是"相机看着怪但零报错"。
- 竖屏（视口 720×1280，地图 1280×704）下的边界事实：**x 轴**夹取区间 `[360, 920]` 有效，
  "跟随"与"不出图"同时成立；**y 轴**视口比地图高 ⇒ 该轴不可夹取，引擎按边界中点取值，
  相机钉在地图纵向中心（几何上不可避免，`docs/52` §7 记为已知偏差并写明最小改法）。

### 验收

- `cargo run -p game --example jhyt_m32_camera`：7 节——门面不跟随、进图居中、逐点跟随、
  贴东/西墙夹取到 `[360,920]`、**四角反投影回世界的可见矩形仍在地图内**、换图重绑 bounds
  （含 20×12 室内图两轴退化）、回门面取消跟随。
- `game --check --project examples/game_jhyt`：新增相机契约门随领域数据一起跑（`game_check_ok`）。

## Unreleased — 示例工程 `game_api` 按当前 API 重建 + API 覆盖率门

主题：示例工程 `examples/game_api` 从"能力展示"改成 **"每个 Lua API 都有可运行演示"的活文档**，
并把"覆盖到了没有"变成可复现的自动化门；逐条证据与边界见
[docs/59](docs/59-示例工程game_api与API覆盖率门.md)，演示模块的写法见
[`examples/game_api/README.md`](examples/game_api/README.md)。

### 覆盖：40 个模块表 / 270 个 Lua 函数 / 21 个运行时组件

- 全新场景集：入口菜单 `index` + 2D 综合 `main` + 全渲染类别 `unified` + 每个 API 模块组一个
  `api_*.scene`（core / gameplay / input / audio / ui / camera / 3d / tilemap / script / prefab）
  + 场景 UI 事件 `ui_test`；旧的玩法型场景（`arena`/`production`/`menu`/`game3d`/`ui_layout`/
  `tilemap`/`effects`/`stress`/`resources`/`persistence`/`data_inventory`/`gameplay_api`/
  `ui_dynamic`/`physics3d`）删除——**玩法样板继续由 `game_jhyt` 承担**，两个示例不再职责重叠。
- 演示清单 `scripts/demos.lua` 是**唯一真源**（场景 ↔ 模块 ↔ 菜单文案）；入口菜单
  `scenes/index.scene` 由 `scripts/gen-api-showcase-index.ps1` 从清单生成，卡片实体名就是场景名。
- `demo3d` / `mixed` 两个**渲染夹具**按原样保留：它们被 `wp0_3d_golden` 与 `wp0_mixed_golden`
  逐项/逐像素断言，价值恰恰在"内容一像素不变"。

### 门：`host::sample_project`（`cargo test -p engine`）

- **API 覆盖率**：运行期导出 `_G` 里的模块表与函数名（不依赖任何文档或手抄清单），逐个要求在
  演示脚本里找到**真实调用点**（注释与字符串先剥掉）；引擎新增一个 Lua 函数，示例工程就必须补演示。
- **组件覆盖率**：`ComponentId::ALL` 的每个运行时组件都要在某个场景里出现。
- **逐场景 headless**：场景文件从 `scenes/` 目录读（不再手抄名单），逐个加载 + 实例化 + 步进；
  新增演示脚本一律过一遍"只编译不执行"的语法门。
- **入口菜单接线**：清单 ↔ 场景目录 ↔ 菜单卡片三方一致；逐个"点击卡片 → `scene.load` → 过渡 →
  新场景的 `on_create` + 一帧 `update`"，并用**真实解算出的卡片矩形**做一次鼠标命中。
- **共享取景框不被覆盖**：场景 `Image`/`Text` 永远画在 Lua 动态 UI 之上，铺满屏幕的场景背景会把
  脚本读数整片盖住——这条门抓这类覆盖（含"只抓大面积"的阈值，避免误伤贴边的小控件）。
- **字形门**：场景里的 `Text`/`WorldText` 用 `ui.measure_text`（与渲染同口径）逐个检查，缺字判红。

### 修掉的真问题（都由门或实测抓到）

- 入口菜单的铺屏 `Backdrop` 是场景 `Image`：脚本的标题/状态/提示与焦点框被整片盖住。背景改为
  `ui.panel`，并加门兜底。
- 覆盖率门对"函数引用"写法（`api.try(light.set, …)`）与 `map["goto"](…)`（`goto` 是 Lua 保留字）
  的误判：门现在接受这三种合法调用形态，注释/文案里的名字仍然不算。
- 工程字体是**旧文案**的子集（242KB → 按实际用字重子集化 76KB），新中文文案不再缺字。
- 场景 `Text` 的 `text=` 里出现裸 `;` 会切坏组件串（整场景实例化失败）、
  `world_text.attach` 传"人话"会抛 `malformed WorldText field`：都由逐场景 headless 门当场抓到。
- 一批引擎/编辑器测试把示例场景的**实体数量 / 名字 / 剪辑名**写死，场景一重建就假失败；
  改成运行期从文档里取（数量、首个带 `Animation` 的实体、第一个根实体…），
  编辑器侧样例场景从 `menu`/`production` 迁到 `ui_test`/`api_prefab`/`api_gameplay`。
- `verify.ps1` 的 wp0-v4 报告契约改成"场景数 == 两个示例工程 `scenes/` 里的场景文件数"，
  不再写死下界（写死既挡不住漏扫，也会在场景增减时误报）。

### 顺带新增的小能力

- `LuaRuntime::scene_name()`：宿主/测试读"当前场景名"的只读入口（与 Lua `scene.reload()` 同源）。
- `RuntimeScene::script` 的可见性放宽到 crate 内：工程级门需要驱动 `on_click` / 切场景 / 控制台表达式。
- 两个开发工具：`engine --example lua_check`（Lua 脚本秒级语法门）、
  `engine --example subset_project_font`（按工程用字重新子集化字体）。

## Unreleased — 编辑器绿色版（免安装包）与中文字体查找修正

- **绿色版引擎包**（`scripts/package-engine.ps1`）：一条命令产出
  `artifacts/packages/RustLua-Engine-<版本>-win64/`（+ 同名 `.zip`），内容为 release 版
  `editor.exe` / `game.exe` / `assetc.exe` + 中文字体 + 两个样例工程 +
  三个双击即用的 bat（含"便携模式"：`RUSTLUA_USER_DIR` 指到包内 `userdata\`，不写 `%APPDATA%`）
  + `运行说明.txt` / `build-info.txt` / `文件清单.manifest.txt`（逐文件 SHA256）。
  先写 staging 再原子替换，失败不留半份包。
- **修正编辑器查找中文字体的方式**（跨机可用性的前提）：原先只认编译期绝对路径
  （`env!("CARGO_MANIFEST_DIR")/fonts/…`，即**构建机**上的路径），绿色包拷到别的 Windows 主机后
  中文会全部变成豆腐块。现在按 `RUSTLUA_FONT` → **exe 同目录 `/fonts/`** → exe 同目录 →
  源码树 的顺序查找，并把**实际命中的路径**（或全部候选路径）打到 stderr——跨机验收时那行日志就是结论。
- 两个踩坑记录（已写进 docs/50 §8）：
  ① **`.bat` 必须是 CRLF + 纯 ASCII + 无 BOM**。打包脚本自身是 LF，here-string 会带出 LF，
  而 cmd.exe 按 CR 找行尾——**LF-only 的 bat 遇到 GBK 多字节注释行会把注释行切碎**，
  用户双击后实际看到 `'_api: 2D' 不是内部或外部命令`（注释里 `game_api：2D` 的残片被当命令执行）。
  现在写文件前一律过 `ConvertTo-Crlf`，bat 内容只用 ASCII（中文说明在 `运行说明.txt`）。
  ② `Start-Process -ArgumentList @(...)` 拼接参数**不加引号**，含空格的路径会被切开，
  游戏报 `argument error: unknown argument: <路径片段>`——自测脚本要传完整且自带引号的整串。

## Unreleased — 编辑器重构阶段 0-3 收口（docs/55）

主题：按 [docs/55](docs/55-编辑器商业引擎对标重构计划.md) 的阶段路线，把**阶段 0/1/2/3** 的交付物与验收
条款全部关门；逐条结论、证据路径与"仍然存在的边界"见
[docs/58](docs/58-编辑器重构阶段0-3收口与验收.md)。

### 质量门：格式化归一化

- `cargo fmt --all`（40 个文件 / 242 处 hunk）。这一步在本次改动**之前**就不绿：本机 rustfmt 版本下
  HEAD 已有大量差异（docs/57 §7.10 记录为 78 处），`verify.ps1` 会停在 `cargo fmt --check`。
  要"阶段 0-3 收口 + 质量门全绿"就必须先做掉它，因此本轮把它作为**一次独立、机械的归一化**记录在此，
  不混进任何功能语义。归一化后 `cargo test --workspace`（1307 项）与 clippy `-D warnings` 仍然全绿。

### 阶段 3：外部改动不再被静默覆盖（§8「数据损坏」硬门槛）

- `scenes/` 与 `scripts/` 本来就是外部工具会写的目录（生成器、git checkout、另一个编辑器），
  但保存路径过去是**无条件覆盖**——用户的 Ctrl+S 会悄悄吃掉别人的改动，且没有任何提示。
  这正是 docs/55 §8 把"数据损坏"列为**直接阻断**项要防的事故。
- `SceneDocument.disk_signature`（长度 + mtime，与观察器同一判据、**不读内容**）+ `DiskStatus`：
  未命名/无基准（崩溃恢复）恒为 `Untracked`——宁可保守放行，也不在没有基准时把用户挡在门外。
- `EditorApp::save_scene()` 在 `Modified`/`Removed` 时**拒绝写入**（不是提示，是真的不写），
  文档保持 dirty，并沿用 E06「保存并继续」的失败语义；显式出口
  `save_scene_force()` + 菜单「强制保存（覆盖外部修改）」保证用户不被卡死；
  覆盖后签名刷新，不会陷在"永远冲突"。
- 观察器一旦发现外部改动**命中当前打开的文档**就立刻提示（不必等到按 Ctrl+S 才知道）。
- 由 `save_refuses_to_overwrite_an_externally_modified_scene`（五个环节）与
  `unnamed_document_never_reports_a_disk_conflict` 钉住。

### ER-03：patch 原型（历史内存从"有上界"变成"有数字"）

- `editor/crates/editor/src/patch_history.rs`：把一次编辑表达成**补丁**（`FieldChange::{Component, Name}`
  带 before/after），undo/redo 只回放补丁而不克隆文档。范围刻意划清：**只覆盖字段级编辑**
  （长编辑会话里占比最高的操作），**不覆盖**结构变更（新建/删除/换父）——那些需要单独的补丁形态，
  原型不假装支持；生产历史仍是 `CommandService`。
- `--perf-baseline` 新增 `patch_history_memory_1k_edits`（1,000 实体场景 / 1,000 次字段编辑）：
  补丁历史保留 **216,000 字节** vs 快照历史下界 **397,890,000 字节**，**比值 0.054%（约 1,840×）**；
  单条补丁 p95 0.005 ms；并且"应用后文档逐字节相同 / 全部撤销回起点 / 全部重做回终态"三项全部成立。
- 最后一项才是重点：docs/55 §8 把"Undo 不等价"列为**直接阻断**项，省内存只有在等价性成立时才有意义。
  这条场景第一版判红过（预算 200 < 编辑 1,000，两条历史都撤不回起点）——那是在测预算而不是测补丁，
  已修并把预算上限交给各自的专项测试。

### 阶段 0：GUI smoke 与真实窗口证据

- **GUI smoke 自动化**（`editor/crates/editor/src/framegate.rs`、`editor --gui-smoke`）：
  把 `EditorApp::draw_frame(&egui::Context)` 从 `eframe::App::update` 里抽出来（`update` 需要真实
  `eframe::Frame`，无法在测试里构造，于是"整帧"在自动化里一直不可达）。现在**合成输入驱动整帧**：
  29 个用例覆盖 `docs/55 §8` 质量门第 3 条枚举的全部七项——DPI、resize、拖放、停靠、输入焦点、
  多选、弹窗。已进 `scripts/verify.ps1`（失败退出码 2）。
- **它当场抓到三个真实缺陷**：① 小尺寸精灵拖不动（`drag_started` 那一帧的指针位置已经移出包围框，
  命中测试用错了坐标 → 改用 `press_origin`）；② Gizmo 中心把手在"线段命中"后不可达
  （三条臂都从锚点出发 → 中心把手改为优先判定）；③ 拖放生成的精灵没有成为选中
  （其余创建入口都会选中新实体，只有拖放这条路径漏了 → `spawn_sprite_from_drop` 提交后
  `selection.set_single` + `sync_inspector`）。
- **真实窗口 GUI/GPU 证据**（`gui_evidence.rs`、`P2_EDITOR_GUI_EVIDENCE`）：3 档 DPI（100/150/200%）
  × 3 档窗口尺寸 + 3D 视口 + Play，逐张落 PNG + `manifest.json`；报告同时记**请求 DPI 与实测 DPI**
  （第一版只记请求值，200% 那档其实是 150% 渲染的）与非黑像素比例，采集前等待启动资源导入结束
  （否则归档的是"导入中"的弹窗）。
- **长会话 soak / Play-Stop 循环**（`P2_EDITOR_SOAK`）：真实窗口循环 Play/Stop，定时采样进程工作集
  （Windows `GetProcessMemoryInfo`）、帧数、实体数、历史深度、日志行数，给出首尾窗口均值差。
  30 分钟 Play/Stop 报告已归档；8 小时档命令就绪但**本轮未跑满**（docs/58 §9 点名）。
- `TextureAsset::save_png_rgba8`：截图归档的编码侧入口（此前只有解码侧，证据出不了内存）。

### 阶段 0：长会话判据从"均值差"改成"谷值趋势"

- 第一版 soak 报告只给"首尾各 10% 采样的均值差"，而进程工作集是**锯齿**的（缓存涨落、
  分配器保留页、Windows 主动 trim），均值差完全取决于取到锯齿的哪一相位——30 分钟那一档
  同时出现了"均值 -258 MiB"和"谷值 +55 MiB"两个相反结论。
- 现在采样里加记 **egui 纹理账**（数量 / 字节，`num_allocated` + `bytes_used` 求和）用于归因；
  报告给整段按时长切 8 段的 min/p50/max、**预热后谷值趋势**与窗口内幅度。
- 新增 `scripts/analyze-soak.ps1 -Path <soak.json> [-Windows N]`：读归档 JSON 按同一口径复算，
  任何一次 soak 都能事后重算（含切成更多窗口）。
- **脚本第一版判据当场误报**：拿"第一个窗口的谷值"当基准，而那个窗口还包含启动预热
  （加载工程 + 导入 706 个资源 + 建字体图集），于是 30 分钟档被算成 `+54.9 MiB` 持续增长；
  改成只取**后半段**作基准后，同一份数据是 `+4.7 MiB`（阈值 37 MiB）→ `no_sustained_growth`。
- **两次并发长档失败已归因**（8 小时档 49 分钟 + 90 分钟归因档 8 分钟）：两者都是我**同时**
  跑起来的，Play/Stop 每轮重载约 10 MB 贴图，两个编辑器在 2 GB 显存（GT 1030）上把显存压到边界，
  WDDM 回退到系统内存导致工作集涨到 1.8-2.5 GiB，随后 `Acquiring a texture failed`，
  egui-wgpu 在 `Queue::write_buffer_with` 返回 `None` 时 **panic**（`renderer.rs:958`），编辑器退出。
  新加的 **egui 纹理账全程恒为 1.0 MiB**，从而排除了"egui 纹理在涨"这一支。
  结论：单实例 30 分钟档是**唯一有效**的长会话证据（无持续增长）；并发那两次是**无效样本**。
  同时登记一个**真实健壮性缺口**：GPU 分配失败以 panic 收场、没有降级与现场落盘。

### 阶段 2：工具控制器拆分 + Gizmo 多选与精确命中

- **`scene_tools::ToolMode`**：2D/3D/Widget/Prefab 各自的工具能力表（允许的工具、默认工具、
  工具语义提示），被三处消费——工具面板生成、`set_scene_tool` 兜底并在状态栏说明、切换视图模式时重校验。
  修掉的是"3D 里选中 Tile 画笔什么都不会发生"这类静默无效状态。
- **3D Gizmo 多选**：拖拽目标从"首个选中实体"改为**整个选中集合**；锚点在 `Center` 模式下取
  选中集合并集包围盒中心；提交时整批世界→局部反解，任一失败就**整批拒绝**（不写一半），
  一次拖拽 = 一条 Undo 覆盖全部目标。
- **精确命中**：命中从"点到端点圆"改为"点到线段距离"，并新增 `gizmo.rs` 纯函数层
  （可脱离帧循环单测）。文档：docs/58 §6。

### 阶段 3：Prefab 往返 hash 门槛

- `--perf-baseline` 新增 `prefab_roundtrip_hash`：`to_text → from_text → to_text` 逐字节一致、
  写盘往返字节一致、逐字段覆盖重开场景后逐字保持、重新 diff 出的覆盖集合幂等。
  该门当场抓到"用名字找实例"的脆弱写法（场景里有两个同名实例时断言取错对象）。



主题：按 [docs/54](docs/54-2D3D统一优化方案与现状对比.md) §8「最终完成定义」的 10 条判据逐条收口，
每条都带可执行证据、可回滚开关与不变量测试。进度对照表见 docs/54 §10。
§8 十条全部达成后转入 §4「目标架构」逐条对齐（批 A 起，见同表末尾各批记录）。

### 目标架构批 S：逐实体增量传播（§4.1 最后一条；docs/54 §10 / docs/53 §3.4f）

- **两级判据**：批 R 的指纹只说"有东西变了"，一变就全量。批 S 缓存上一帧的**逐实体输入快照**
  （id / 父边 / 局部值，逐字段精确比较而非哈希）与上一帧的**父先序求解顺序**，于是能回答"**哪些**
  实体动了"并复用计划：结构变了或上一帧有诊断 ⇒ 全量（与批 R 之前同一份代码）；只有值变了 ⇒ 只重算
  "变化的实体 + 它们的后代"（脏父的世界值直接从 `GlobalTransform` 组件读）；什么都没变 ⇒ 整趟跳过。
- **实测（1,000 / 10,000 实体、深度 8 父链）**：单叶动的传播 **9,623 → 3,188 µs**、整帧
  **13,623 → 6,940 µs**、**重算实体数 10,000 → 1**；全动一档基本不变（8,816 µs——该算的全算），
  静态档不变（0.51 ms，指纹哈希的地板）。即"改得少就快、改得多就照旧"。
- **回滚档**：`RuntimeScene::set_spatial_incremental(false)`（默认开启，返回旧值）。
- **门（确定量，不受跨机速度影响）**：静态相位重算数 = 0；单叶动重算数**恰好 1** 且确实走过增量路径；
  全动重算数 ≥ 实体数 / 2；同进程内单叶动传播耗时 × 2 ≤ 全动。
- **不变量测试**：脏子树大小（链尾/链中/链根 = 1/4/6）、增量与全量逐实体比对**视图与矩阵**、结构变化
  退回全量、有诊断场景一律全量、真实帧里只挪链尾只重算 1 个（引擎 789 → **795**）。
- 文档：docs/53 §3.4f、docs/54 §11.1 状态改为"达成" / §11.5 批次表 / §10 批 S 记录、CHANGELOG。
### 目标架构批 R：传播的场景级跳过 + 三相位量尺（docs/54 §10 / docs/53 §3.4e）

- **"没变就不传播"**：`hierarchy::local_fingerprint` 把全部 `Parent`/`Transform`/`Transform3D` 的实体
  id、代际与局部数值哈希成 `(u64, 实体数, 边数)`；`propagate_spatial_now` 在指纹与 `spatial_active`
  都不变时整趟跳过。默认开启，`set_spatial_skip_unchanged(bool)` 可关（返回旧值），
  `take_spatial_skipped_frames()` 报出跳过次数；`static_hierarchy_skips_propagation_until_something_changes`
  钉住"静止时跳过、一改就更新世界值"。
- **静态档实测（批 Q → 批 R）**：10,000 实体整帧 **31,525 → 4,215 µs**（约 7.5×），传播占比
  **29.4% → 11.4%**（剩下的 0.48 ms 是指纹检查自身的成本），每帧分配 **155 → 80**；1,000 实体
  整帧 3,045 → 613 µs、占比 26.2% → 7.8%、分配 138 → 75。
- **阶梯扩成三个相位**（同一场景同一进程）：静态 / 单叶动 / 全动。结果把"要不要做逐实体增量"变成
  数字——**单叶动与全动同价**（10,000 实体只挪一个叶子仍是 9,623 µs，全动 9,724 µs），因为运行时
  路径是全量重算，而 `TransformTree` 早有逐实体增量语义（深度阶梯 `leaf_recomputes`）。§11.1 因此从
  "增量待定"改为"**场景级跳过已实施；逐实体 dirty/version 已量化、待实施**"。
- **门**：静态相位 `spatial_skips_p50 > 0`；两个运动相位的 `spatial_propagate_us` 必须严格大于静态
  相位（指纹不许吞掉真改动）；分配上限 400 扩到三相位全查。时间阈值仍不进门。
- 文档：docs/53 §3.4e、docs/54 §11.1 状态更新 / §11.5 批次表 / §10 批 R 记录、CHANGELOG。
### 目标架构批 Q：传播路径的索引与分配（docs/54 §10 / docs/53 §3.4d）

- **按批 P 的定位修掉真瓶颈**：`collect_hierarchy` 与 `parent_first_plan` 各用 `BTreeSet`/`BTreeMap`
  逐实体插树节点，外加每条链一个 `Vec`。现在排序 `Vec` + 平行 `state: Vec<u8>` + `binary_search`，
  链上暂存整趟复用。**语义与顺序都不变**（`BTreeSet` 本就按 id 升序），既有测试一条未改而全过。
- **实测（1,000 / 10,000 实体、深度 8 父链）**：每帧分配 **4,602 → 138** / **44,951 → 155**
  （从"每实体 4.5 次"降到与实体数无关），传播 p50 **1,228 → 801 µs** / **14,131 → 9,283 µs**，
  整帧 p50 **4,252 → 3,045 µs** / **44,835 → 31,525 µs**（约 −30%）。
- **占比仍约 29%，这是信息不是失败**：省掉的是分配固定开销，剩下的是真实工作（每实体 3 次组件查找 +
  矩阵组合 + 写入）。§11.1 状态更新为"分配已修、增量待定"，是否接 dirty/version 由批 R 按静态层级
  场景的占比决定。
- **门收紧**：层级档新增 `alloc_count_p50 ≤ 400`、两档之差 ≤ 100（分配不许随实体数增长）；时间阈值
  仍不进门。
- 文档：docs/53 §3.4d、docs/54 §11.1 状态更新与 §10 批 Q 记录、CHANGELOG。
### 目标架构批 P：传播成本的实测阶梯（docs/54 §11.1 / docs/53 §3.4c）

- **把一条判断变成证据**：§11.1 曾把 dirty/version 增量传播写成"有意未实施（未被证明是瓶颈）"——
  那是判断，不是测量。`wp0_scene_frame` 新增一档**层级帧阶梯**（`spatial=local`、1,000 / 10,000 个
  实体按深度 8 组成父链），从每帧调度报告取 `spatial_propagate` 的耗时与整帧占比。
- **测出来的结论与判断相反**：1,000 实体帧 4,252 µs / 传播 1,228 µs（**28.6%**）、10,000 实体帧
  44,835 µs / 传播 14,131 µs（**31.5%**），每实体每帧约 **4.5 次分配**。分配来源已定位：
  `collect_hierarchy` 与 `parent_first_plan` 各用 `BTreeSet`/`BTreeMap` 逐实体插树节点。
- **门只判"测量是活的"**：`verify.ps1` 断言阶梯 2 档、`spatial_propagate_us > 0`、占比 0..100；
  时间阈值不进门（跨机不可比）。§11.1 的状态改成"**已测量：是真瓶颈，待实现**"，并注明旧结论是判断
  而非证据。
- **下一步**：批 Q 修分配与索引结构（排序 `Vec` + 二分取代 `BTreeSet`/`BTreeMap`、链上暂存复用），
  再按修后占比决定是否需要 dirty/version 增量。
- 文档：docs/53 §3.4c（含表格）、docs/54 §11.1 状态更正与 §10 批 P 记录、CHANGELOG。
### 目标架构批 O：动画分配门（docs/54 §4.4 / docs/53 §3.4b）

- **补上门覆盖不到的洞**：批 N 的性能回归是**读代码发现的**，因为 WP0 阶梯里没有动画实体。现在
  `wp0_scene_frame` 单列一档**动画精灵阶梯**（500 / 5,000 实体，同一片段），`verify.ps1` 判红：
  ① 两档分配次数之差 ≤ 20（每帧分配不许随实体数增长）；② 最大 `alloc_count_p50` ≤ 200（两档一起
  退化也判红）。实测 500 实体 **79 次**、5,000 实体 **80 次**（每实体 0.116 → 0.0116），是平的。
- **修掉批 N 自己引入的浪费**：登记片段身份时先收集 `Vec<String>` 再查表 = 每帧每实体一次 `String`
  分配；改成就地先 `handle_of` 再 `intern`（命中路径零分配），并在片段无帧事件时跳过事件采样。
  3D 骨骼那条刻意保留"每帧组合一次键"（骨骼实体少、`Animation3D` 不知道网格路径），差异写进注释
  与 docs/53。
- **证据**：`verify.ps1 -Benchmarks -SkipTests` 全绿（含新断言）；`wp0_scene_frame_ok ...
  animation_stairs=2 animation_alloc_p50_max=80`；docs/53 §3.4b 记两档实测与门槛理由。引擎 788、
  编辑器 170+111、assetc 19、game 13 全绿。
- 文档：docs/53 §3.4b 与 §4（门契约新增两条）、docs/54 §10 批 O 记录、CHANGELOG。

### 目标架构批 N：动画片段身份 + 轨道缓存（docs/54 §4.4）

- **`AnimationClip` / `Skeleton` 有了运行时身份**：片段没有文件路径，身份因此是**内容签名**——
  `AnimationClip::resource_key()`（贴图 + 帧数 + 帧率位模式 + 循环 + 事件）与
  `Animation3D::resource_key(mesh_path)`（网格 + 片段名）；推进动画时 intern 进与贴图/网格同一张
  身份表，kind 是新增的 `ResourceKind::Animation`。先 `handle_of` 再 `intern`，热路径只有一次哈希
  查找、不分配字符串。至此 §4.4 列的六类资源**全部**有了运行时身份说法。
- **同批修掉批 H 引入的真实性能回归**：轨道视图是 `(clip)` 的纯函数，但 `advance` 与
  `current_frame_uv` 每次调用都现算（`cols` 个关键帧 + 事件 Vec）⇒ 对一群动画实体每帧每实体一次分配。
  WP0 基准场景恰好都是静态精灵，性能门没抓到（**是读代码时发现的**）。现在轨道视图与身份键按片段
  缓存（换片段才重建）、`advance` 在无帧事件时跳过事件采样、`current_frame_uv` 采样缓存。
- **证据**：`clip_track_cache_is_reused_across_frames`（数据指针证明缓存复用；换状态后重建、切回去
  回到原键）、`clip_resource_key_is_content_addressed`（名字不参与身份，帧数/帧率/循环/贴图/事件任一
  不同即不同键）、`animation_clip_identity_is_stable_across_frames`（真实场景：kind 是 `Animation`、
  相邻两步同句柄、第二帧不重复登记）。引擎 785 → 788，编辑器 170+111、assetc 19、game 13 全绿。
- **仍未达成（刻意的）**：精灵 tint 仍在顶点里烘焙——只有这样"同贴图不同 tint"才能合进一个 draw；
  搬进 bind group 会按 tint 拆批。
- 文档：docs/54 §4.4 现状、§10 批 N 记录、§11.4 表格更新、CHANGELOG。
### 目标架构批 M：`RenderView.order` 接线 + §4 收尾对照（docs/54 §4.2 与 §11）

- **`RenderView::order` 终于有了消费者**：与批 A 的 `visibility` 同类问题——字段存在、`FrameViews::ordered()`
  只在测试里被读过，清屏硬编码取 `main`。现在顺序规则只有一处（`FrameViews::ordered`），**清屏由顺序最前
  的已声明视图给出**：声明一个 `order = -1` 且带 `ClearPolicy::Color` 的 2D 背景视图，这一帧就由它清屏。
  唯一的语义变化是"只声明 overlay"的场景（此前退回历史默认色、现在由该视图的策略决定）——仓库里没有这样
  的宿主（编辑器与运行时都声明 `main`），并把这条钉在 `frame_views_declare_clear_and_draw_order` 里。
- **§4 收尾对照**：docs/54 新增 §11，逐条列出 §4.1–§4.4 的蓝图条目、状态与证据，并写明**两处有意不逐字
  实现**的理由（dirty/version 增量传播未被证明是瓶颈；`RenderView.inverse` 在拾取时按需算更省）；同一节
  把 §4 目标的 A–M 批次的对应关系列成可追溯的一段。
- 文档：docs/54 §11（新增）、CHANGELOG。

### 目标架构批 L：`LocalTransform` 规范表示（docs/54 §4.1）

- **读/写各一个入口**：`RuntimeScene::local_transform_of(scene_id)` 把 ECS 的 `Transform`/`Transform3D`
  统一转成 `LocalTransform`（3D 优先）；`set_local_transform` 写回实体**自己的**空间——只有 2D
  `Transform` 时**仅在真能无损压平时**才写，否则**报错**并保持原值不变（手抄 XY 会丢掉旋转与缩放）。
- **表示层对拍门**：`runtime_and_document_agree_on_the_canonical_local_transform` 对真实场景逐实体
  比对运行时导出的 `LocalTransform` 与 `scene_space::local_transforms`（文档侧解析），并断言写回
  round-trip——"两份真源"最容易被引入的地方现在有门。
- **非平面值被拒绝**且不留半写状态（`non_planar_local_transform_is_refused_for_2d_entities`）。
- **存储的边界是刻意的**：运行时 ECS 仍按空间存 `Transform`/`Transform3D`（序列化边界格式 + 2D
  快路径，10k 精灵档实测过），`LocalTransform` 是**规范表示**（组合/反解/换父/迁移都基于它）；
  两者之间的转换只有一处实现，且有上面的对拍门钉住。把 2D 也换成四元数存储会牺牲已量过的快路径。
- **证据**：两条新测试 + 既有的仓库级门（56 份示例场景逐份断言"局部变换真源唯一"）。引擎
  783 → 785，编辑器 170+111、assetc 19、game 13 全绿。
- 文档：docs/54 §4.1 现状与 §10 批 L 记录、CHANGELOG。
### 目标架构批 K：单一 `Renderable` / `Material`（docs/54 §4.3）

- **`Renderable` 是单一的**：`core/renderable.rs` 的 `Renderable::classify` 从一个实体的组件名集合
  给出 `Single` / `None` / `Conflict`；冲突带**优先级**（`Sprite → Quad → Mesh → 文字 → 图片 → 粒子`），
  因此"两者都挂时画哪一个"可预测、可断言。此前"`Sprite` + `Quad` 同挂"会**画两遍**，既没有诊断也
  没有优先级。
- **冲突被点名两次**：加载期收集进 `scene_renderable_conflicts`
  （`RuntimeScene::renderable_conflicts()` 读回并 warn），提取阶段按优先级只画一种（Quad 收集时若该
  实体还有 Sprite 就跳过）。
- **`Material` 是单一的**：`Material { base_color, texture, metallic, roughness, unlit }`，精灵/Quad 用
  `unlit(...)`、网格用 glTF `pbr(...)`；渲染器 `CachedPrimitive` 直接持有它，`GpuMaterial` 由
  `from_material` 构造——"缺省材质"（基色/金属度/粗糙度）从此只有一处定义。
- **刻意的边界**：`Renderable` 是**分类视图**，不是第二份真源（不序列化、不参与渲染决策）；精灵 tint
  仍在顶点里烘焙，`Material` 统一的是**描述**而非 bind group 参数。
- **证据**：`core::renderable` 三条单测（分类、冲突消解与参数顺序无关、材质校验与缺省）；
  `conflicting_renderables_are_reported_and_only_one_draws`（真实场景：冲突被点名、只画精灵 6 顶点）。
  引擎 779 → 783，编辑器 170+111、assetc 19、game 13 全绿。
- **仍未达成**：`LocalTransform` 作为唯一运行时局部变换存储；精灵 tint 的材质化。
- 文档：docs/54 §4.3 现状与 §10 批 K 记录、CHANGELOG。
### 目标架构批 J：三种显式 2D 显示模式（docs/54 §4.3）

- **显示模式从"宿主选路径"下放到实体**：新增 `RenderMode` 组件 `mode=world|overlay|screen`。
  此前"这段几何走哪条路"是**全局开关**（`set_world_sprite_depth`）决定的——整帧要么全走世界通道、
  要么全走 NDC overlay，同一场景里没法混（"3D 场景里挂一个不受相机影响的 HUD 精灵"只能靠另开 UI
  通道或干脆不做）。
- **语义**：未声明 = 跟随全局开关（既有场景一字不改、像素不变，这是回滚档）；`overlay` / `screen`
  = **退出世界通道**（屏幕空间，分别落 `Overlay2D` / `ScreenUI` 桶，画在世界内容之上且 ScreenUI
  不被其它 2D 内容盖住）；`world` = **请求世界通道**（通道是帧级的，默认实体也跟着进——写进
  docs/16 免得变成意外）。未知取值报错，不静默当成 overlay。
- **顺带修掉一个真实缺陷**：`merge_adjacent_draws` 的合并条件漏了 `render_layer`：相邻两段纹理
  相同、顶点连续时会被并成一段并沿用前一段的层，于是 ScreenUI 的几何悄悄变成 Overlay2D（几何还在、
  层变了），§8-⑤ 的 HUD 隔离门在有显式显示模式之后会失效。现在层一起比，并加了回归测试。
- **证据**：`explicit_render_modes_route_geometry_to_the_right_channel`（真实场景：默认实体进世界
  通道 6 顶点、overlay/screen 分别落对应桶、路由计数 `(0,1,1)`；宿主开关关着但声明 `mode=world`
  时照样走世界通道）；`merging_adjacent_draws_never_crosses_render_layers`；`core::render_mode`
  两条单测。引擎 775 → 779，编辑器 170+111、assetc 19、game 13 全绿。
- **仍未达成**：`Renderable`/`Material` 仍不是单一类型；`LocalTransform` 作为唯一运行时局部变换存储。
- 文档：docs/16（`RenderMode` 组件与三种模式的边界）、docs/54 §4.3 现状与 §10 批 J 记录、CHANGELOG。
### 目标架构批 I：网格/材质的运行时身份（docs/54 §4.4）

- **网格也走同一套身份**：`Scene3dDrawable` 新增 `mesh_handle`，3D 提取时把网格路径 intern 进与
  贴图同一个身份表（`ResourceKind::Model`）。3D 提取是 `&self`，因此运行时身份表改成 `RefCell`
  （`asset_entries()` 相应返回克隆）；同一网格跨帧恒同句柄。
- **渲染器按句柄查网格缓存**：`cached_mesh(handle, path)` 先查身份表（**O(1)**），未命中才按路径
  加载并记下映射；`mesh_identity_stats()` / `cached_mesh_count()` 把"命中"与"同一网格只解析一次"
  变成可断言的事实。路径键的 `mesh_cache` 保持不变，因此同一网格被两个路径引用也不会加载两遍；
  宿主手写 `Scene3d`（`mesh_handle: None`）时回退到按路径解析，行为与收敛前一致。
- **材质身份由网格身份派生**（`(网格句柄, primitive 下标)`，载体 `CachedMesh::primitives`）：glTF
  的材质与底图嵌在网格资产里，没有独立路径可 intern，另编一个材质路径键会变成第二套假身份。
- **§4.4 的六类资源现在都有说法**：Texture / 网格 → 运行时句柄；Material → 网格句柄 + primitive；
  SpriteRegion / AnimationClip / Skeleton → 统一轨道（批 H）。
- **证据**：`mesh_identity_is_stable_across_frames`（真实场景两帧同句柄、不同网格不同句柄、身份表
  的 `ResourceKind` 是 `Model`）；`mesh_identity_cache_is_hit_on_the_second_frame`（真实离屏帧：
  第一帧未命中、第二帧命中、同一网格只解析一次、`mesh_handle: None` 时统计不动）。引擎 773 → 775，
  编辑器 170+111、assetc 19、game 13 全绿。
- **仍未达成**：`AnimationClip` / `Skeleton` 仍以网格/贴图路径间接标识（没有独立 clip 句柄）；
  `LocalTransform` 作为唯一运行时局部变换存储；§4.3 的三种显式 2D 显示模式与 `Renderable`/`Material`
  单一类型。
- 文档：docs/54 §4.4 现状与 §10 批 I 记录、CHANGELOG。

### 目标架构批 H：统一动画轨道抽象（docs/54 §4.4）

- **"一条动画 = 若干条按时间采样的轨道"从说法变成类型**：`core/animation_track.rs` 的
  `PropertyTrack`（目标 + 关键帧 + 插值方式）与 `EventTrack`（按时间点触发的事件）合成
  `AnimationTracks`；四类目标 `SpriteRegion` / `PlaneTransform` / `SpatialTransform` /
  `SkeletonPose` 在同一套类型里，`TrackValue::matches` 保证值与目标匹配。
- **离散目标只允许 `Step`**：对 UV 区域或骨骼 pose 插值没有意义，`validate()` 直接报
  `InterpolationNotAllowed`（静默允许会让人看到中间的杂乱帧）。连续目标支持 `Linear`：位置/缩放
  线性、旋转按**最短弧**（点积为负先取反再归一化插值），结果保证是单位四元数。
- **事件不写"第几帧"**：帧只是 Sprite region 轨道上的量化，`EventMarker` 因此带 `time` + 源数据
  序号 `order`（供"每圈只触发一次"去重）；`events_in_interval` 的区间是闭区间，循环回绕时按
  "两段都到达过"处理。
- **抽象是消费方，不是并列的第二套**：`AnimationClip::tracks()` 生成轨道，
  `AnimationStateMachine::current_frame_uv()` 采样 `SpriteRegion` 轨道（`Cow` 借用，帧动画热路径
  零克隆），`advance()` 的帧事件改走 `EventTrack::events_in_interval`（`fired` 只负责去重）。
  "帧 → UV"与"事件落在哪一步"因此各自只有一处定义；3D 骨骼 palette 采样器继续专用（§4.4 允许）。
- **新增 Lua `animation.time(id)`**（与 `animation3d.time` 同名同义，缺组件报错而非返回 0）：
  轨道采样是"时刻 → 值"的函数，脚本按时间轴推理需要一个可读时钟。
- **证据（等价性优先）**：`clip_tracks_agree_with_the_state_machine`（1/32 秒步长、80 步、跨两圈
  以上，逐步比对轨道采样与渲染真正用的 UV）；`clip_tracks_carry_the_frame_events_in_order`；
  `core::animation_track` 六条单测（Step 保持/夹取、平面线性、3D 最短弧、事件区间与回绕、校验四类
  错误、Step 采样借用）。**既有帧事件语义测试一条未改而全过**——收敛没有改变行为。引擎 765 → 773。
- **仍未达成**：Mesh/Material/AnimationClip/Skeleton 仍以路径字符串为运行时身份（轨道抽象统一了
  "动画是什么"，还没统一"它是哪个资源"）；`LocalTransform` 作为唯一运行时局部变换存储。
- 文档：docs/18（`animation.time` 与轨道抽象的关系）、docs/54 §4.4 现状与 §10 批 H 记录、CHANGELOG。

### 目标架构批 G：v2 `parent` 的组织语义（docs/54 §4.1）

- **默认语义从"宿主猜"改成"场景说"**：新增 `meta spatial=local | organizational` 声明
  （`core::scene_space::SpatialMode`）。**未声明 ⇒ 组织关系**（v2 历史语义）：`parent` 只表达
  层级/分组，不参与空间组合；`spatial=local` ⇒ `Global(child) = Global(parent) * Local(child)`
  （迁移工具写入的就是它，`MIGRATION_META_KEY` 现在与运行时读同一个常量）。
- **修掉一处真实的静默语义变化**：此前 `RuntimeScene` 的继承开关默认是**开**的，判据只看"场景里
  有没有 `Parent`"——任何带 `parent` 边的旧场景都会被自动按父子组合解释。现在默认值来自场景声明，
  宿主 `set_spatial_inheritance` 退化为覆盖（回滚档），`RuntimeScene::spatial_mode()` 让"为什么
  继承/不继承"有据可查；认不出的取值按默认处理并**告警点名**。
- **文档侧读同一份声明**：`scene_space::world_matrices` 在组织关系下返回各实体的局部矩阵，
  `parent_world_matrix` 返回 `None`（"在父之下保持世界矩阵"自然退化为"保持局部值"）。编辑器
  Gizmo / keep-world 换父 / 运行时因此不会一个按组合算、一个按局部值画。
- **代价是显式**：断言父子组合的测试夹具必须声明 `spatial=local`（本批改了 4 处）。仓库里唯一带
  `parent` 边的示例场景（`game_api/index.scene`）子实体局部变换为 0，两种解释数值等价 ⇒ 画面不变。
- **证据**：`unmarked_v2_scenes_treat_parent_as_organizational_only`（未声明 ⇒ 不组合、子实体世界
  值 = 局部值、`Parent` 边仍在 ECS 里）、`explicit_and_invalid_spatial_declarations_are_reported`、
  `organizational_mode_ignores_parents_on_both_sides`、
  `spatial_inheritance_switches_between_local_and_world_positions`。引擎 762 → 765，
  编辑器 170+111、assetc 19、game 13 全绿。
- **仍未达成**：`LocalTransform` 作为唯一运行时局部变换存储（`Transform`/`Transform3D` 仍是边界
  格式与 2D 快路径）；Mesh/Material/AnimationClip/Skeleton 的运行时身份；统一动画轨道抽象。
- 文档：docs/16（`meta spatial` 语义）、docs/54 §4.1 现状与 §10 批 G 记录、CHANGELOG。

### 目标架构批 F：运行时 `AssetId`/`Handle` 资源身份（docs/54 §4.4）

- **运行时第一次有了资源身份**：`core/asset_handle.rs` 的 `AssetHandle`（`(下标, 世代)`）+
  `AssetStore`（`路径 → 句柄` 注册表）。`intern` 幂等 ⇒ 同一路径在进程内恒同句柄；释放后世代前进
  ⇒ **旧句柄失效**（`get` 返回 `None`），而不是拿到别人放在同一个下标上的资源——那正是最危险的
  一类错误（画面上出现一张"莫名其妙"的贴图）。此前 `RuntimeScene::texture_paths` 每帧清空重建、
  `texture = 下标`，同一个下标在相邻两帧可以指两个不同的资源。
- **身份有真实消费方**：`SceneMesh::path_handles`（与 `paths` 同序）把身份交给渲染器，渲染器按
  句柄查贴图缓存（**O(1)**，此前是每 draw 一次线性路径扫描），`Renderer::texture_identity_stats()`
  与 `loaded_texture_count()` 把命中/未命中、以及"同一资源只加载一次"变成可断言的事实。
  宿主**手写** `SceneMesh`（没有身份）时回退到按路径解析，行为与收敛前一致。
- **两张表各司其职**：`assets`（跨帧、不清空）是身份；`texture_paths` + `texture_lookup`（每帧
  重建）是本帧的注册顺序与去重，两者都是 O(1) 哈希——批量精灵档不会因为引入身份而多出线性扫描
  （精灵/Quad/瓦片/粒子四条产出路径都走同一入口 `texture_id_of`）。
- **与工程身份分开**：`core::resource_db` 的 `AssetId` 是 16 字节内容指纹（编辑器/assetc 的工程
  身份），运行时句柄回答"这一帧引用的是哪个已加载资源"；两者都不是对方的副本。
- **证据**：`core::asset_handle` 四条单测（幂等、释放后失效且复用下标也不别名、重复释放 no-op、
  顺序稳定）；`texture_identity_is_stable_across_frames`（真实场景两帧同句柄、不同资源不同句柄、
  身份表记录路径与 `ResourceKind`）；`texture_identity_cache_is_hit_on_the_second_frame`（第一帧
  未命中、第二帧命中、同一资源只加载一次、没有身份时统计不动）。引擎 756 → 762。
- **仍未达成**：Mesh / Material / AnimationClip / Skeleton 仍以路径字符串为运行时身份；动画的统一
  轨道抽象尚未落地；输入边界已由 `core/picking` 统一，但还不是类型强制的。
- 文档：docs/54 §4.4 现状与 §10 批 F 记录、CHANGELOG。

### 目标架构批 E：per-entity `Script` 组件（docs/54 §4.1）

- **脚本归属从"工程级一份"下放到实体**：新增 `Script` 组件——场景写
  `component 9 Script file=scripts/ai/patrol.lua` 即可，不必再在 `scripts/main.lua` 里遍历实体、
  按名字查、自己维护分派表。`file` 必填，可选 `enabled=false`（启用时不写出字段，既有场景串不变）。
- **一个文件一份 Lua 状态、实体是实例**：同文件的实体共享顶层局部变量，脚本用收到的实体 `id`
  区分自己（`on_spawn(id)` / `on_update(id, dt)`）。上万实体因此不会变成上万份 Lua 状态。钩子名与
  工程级脚本**不重名**（工程级仍是 `on_create/update/on_destroy`），同一个文件被两边加载也不互相踩。
- **加载期就报错**：场景实例化末尾读入脚本并调用 `on_spawn(id)`；文件缺失、编译失败、以及"有脚本
  却没有工程根"都在**加载期**报错，否则"脚本没跑"会变成没有线索的谜题。运行时新生成的实体下一帧
  自动补上钩子。
- **路径限制在工程根内**：绝对路径与含 `..` 的路径在解析期报错——脚本是可执行代码，场景文件不该
  能把可执行代码指到工程外。
- **进固定步流水线**：新增内建 step 系统 `entity_scripts`（`ScheduleStage::Script`，依赖
  `ui_click_dispatch`，在工程级 `update()` 之后按实体 id 升序调用），
  `RuntimeScene::last_entity_script_calls()` 让"这一步驱动了几个实体"可断言。内建系统名单测试已同步。
- **热重载覆盖两者**：`RuntimeScene::reload_scripts()` 返回 `(工程级脚本状态, 重载的实体脚本文件数)`
  ——工程级没变而实体脚本改了时前者是 `Unchanged`、后者 `> 0`，两个数字分开报；已生成过的实体不会
  因重载重复触发 `on_spawn`。
- **证据**：`core::entity_script` 三条单测；`entity_script_component_drives_its_own_entity`；
  `entity_script_reports_missing_files_and_missing_project_root`；
  `entity_script_component_runs_inside_the_step_pipeline`；`typed_scene` 的来源组件集合加入
  `ComponentId::Script`。引擎 750 → 756，编辑器 170+111 不变。
- **仍未达成**：`LocalTransform` 作为唯一运行时局部变换存储、v2 `parent` 组织语义、运行时
  `AssetId`/`Handle`、统一动画轨道抽象、§4.4 输入边界收口。
- 文档：docs/16（`Script` 组件）、docs/18（每实体脚本钩子）、docs/19（热重载边界）、docs/54
  §4.1 现状与 §10 批 E 记录。

### 目标架构批 D：一等 `Quad`（docs/54 §4.3）

- **§4.3 的分解第一次有了独立类型**：`Sprite = Quad Geometry + TextureRegion + Material +
  Transform + Depth/Sorting` 此前只是代码里的既成事实——`Sprite` 组件**必须**给贴图（缺 `texture=`
  报 `MissingTexture`），于是"一个纯色矩形"只能写 `texture=__white__`，而那个哨兵只在渲染器内部
  有定义。新增 `Quad` 组件（`core/quad.rs`）：`size` 必填，`pivot`/`color`/`z`/`layer`/`order`/
  `flip_x`/`flip_y` 可选，`texture` **可选**（缺省用内置 1×1 白纹理 ⇒ `color` 就是最终颜色）。
  位置/旋转/缩放来自实体的 `Transform`，与精灵同源。
- **不是第二条渲染通道**：提取阶段 `Quad::to_sprite()` 给出几何 + 整张纹理区域，随后走与精灵**完全
  相同**的 `SpriteBatchItem` 路径（同一套排序、剔除、深度、可见遮罩、世界/overlay 两个通道），
  所以"把血条底从 Sprite 改成 Quad"不改变层次规则。分段按层升序，未写 `layer`/`order` 时用与精灵
  同一套"按场景顺序"的默认层。
- **组件留在 ECS 里**：`Quad` 不会被降级成 `Sprite`（测试断言 `get::<Sprite>` 为 `None`，否则会画
  两遍）；Lua 新增 `quad.get/set`（与场景组件串同一套语法，拼错字段/缺 `size`/零尺寸/非法数值报错）。
- **刻意的边界**：`Quad` 不参与 `Animation`——帧动画动的是 UV 区域，矩形没有区域；需要帧动画就用
  `Sprite`。这条写进 docs/16 与类型文档，而不是靠"动画查不到 Sprite 就跳过"。
- **证据**：`core::quad` 四条单测；`quad_is_a_first_class_renderable_sharing_the_sprite_path`
  （真实场景提取：两个 Quad 各 6 顶点、无贴图者解析到白纹理、`Quad` 在 ECS 且没有 `Sprite`、层序
  按升序分段）；`lua_quad_round_trips_and_validates`；`unified.scene` 增补 `HealthBar`
  （`Transform` + `Quad`），`one_scene_expresses_every_renderable_category` 把 `ComponentId::Quad`
  列入必需集合。引擎 744 → 750，编辑器 170+111 不变。
- **仍未达成**：`Renderable`/`Material` 仍不是单一类型；三种 2D 显示模式仍由宿主选择调用路径。
- 文档：docs/16（`Quad` 组件）、docs/18（`quad` 表）、docs/54 §4.3 现状与 §10 批 D 记录。

### 目标架构批 C：`PostProcess` 通道落地（docs/54 §4.3）

- **后处理从蓝图里的一行变成真通道**：`Renderer::set_post_process(Some(settings))` 打开后，场景先
  画进**离屏颜色纹理**，最后由全屏三角形 pass 采样它写进目标附件（WGSL 顶点由
  `@builtin(vertex_index)` 生成，不需要顶点缓冲）。参数只有曝光 / 色调 / 暗角三个。
- **回滚档是 `None`（默认）**：不创建离屏纹理、不产生额外 pass，本帧所有通道直接画进目标附件，
  像素与收敛前逐字节相同。注意 `Some(identity)` **不是**回滚档——它会多一次 8 位量化往返。
- **非法参数当场拒绝**：非有限值、负曝光、负色调、暗角越界都是 `PostProcessError`，不静默夹紧
  （"我调了参数但画面没变"是最难查的一类问题）。
- **`ClearPolicy::None` × 后处理**：离屏附件上没有"上一帧内容"可叠，因此开后处理时本帧强制用默认
  清屏色并 `tracing::warn!` 一次；计划函数把"开后处理 ⇒ 一定有 `Clear`"编码成事实，`PostProcess`
  永远是最后一条通道。静默改写比报错更难查。
- **像素证据**（真实离屏帧读回）：不开后处理时计划里没有 `PostProcess` 且执行与计划逐条一致；
  恒等参数不变（≤2 LSB）；曝光 0.5 压暗；`tint=[1,0,0]` 把绿蓝压到 ≤1；暗角 1.0 时角落是
  `[0,0,0,255]` 而中心更亮。另有 `core::post_process` 三条参数校验单测。
- **仍未达成**：`Renderable`/`Material` 仍不是单一类型；三种 2D 显示模式仍由宿主选择调用路径。
- 文档：docs/54 §4.3 现状与 §10 批 C 记录。

### 目标架构批 B：`RenderPlan` 成为类型并被渲染器消费（docs/54 §4.3）

- **"这一帧跑哪些通道"此前只存在于渲染循环的一串 `if` 里**：决策与执行混在一起，因此没法在
  不启动 GPU 的情况下断言它，"顺序对不对"只能读代码。现在 `core/render_plan.rs` 把决策抽成纯函数
  `RenderPlan::for_frame(FrameFacts)`——输入只有一帧内容的计数与布尔（世界精灵数、有没有 3D 网格、
  三个 `RenderLayer` 桶各多少 draw……），输出是通道的**规范顺序**列表。
- **渲染器按计划执行**：每个通道的开关从就地判断改成 `plan.contains(...)`；执行完与计划核对
  （`RenderPlan::mismatch`），不一致即 `tracing::warn!`。`Renderer::last_frame_plan()` /
  `last_frame_passes()` 让"到底跑了哪些通道"成为可直接断言的事实（此前只能数 draw call 反推）。
  硬性质 `is_canonical()`（同一通道至多一次、严格规范顺序）对全部输入组合穷举成立。
- **`RenderPass` 的声明顺序就是规范执行顺序**：`Clear → WorldSpriteOpaque → Shadow → DepthOpaque
  → WorldSprite → Legacy2DOverlay → ScreenUI → VectorText`。与蓝图的有意偏离写在类型文档里：
  不透明世界 Sprite 排在 `Depth/Opaque` **之前**（共用深度附件时"精灵遮挡网格"要求它先写深度）；
  蓝图里的 `Transparent` 就是 `WorldSprite` 的半透明分段；`Clear` 与 `VectorText` 是蓝图未单列的
  两条（清屏是附件操作；矢量文字是独立管线）。
- **`layer_indices`**：取单个 `RenderLayer` 桶的索引，与既有 `layered_indices` 共用同一套"层怎么比"
  的规则；`Legacy2DOverlay` 覆盖 `World2D + Overlay2D`，`ScreenUI` 独立成通道，两者仍共用一次
  `begin_render_pass`（同管线，拆两个 pass 只多 CPU 开销、不写像素）。
- **零像素变化**：计划是**派生**的（从帧内容算出，不是第二份真源），只决定跑不跑与顺序，通道内部
  行为逐字未动；`verify.ps1 -Benchmarks` 的 2D/3D/混合/HUD/光照 golden 全绿。
- **仍未达成**：`PostProcess` 通道还不存在（下一批）。
- 引擎测试 733 → 739（`core::render_plan` 五条 + 真实离屏帧上"计划=执行"一条）；文档：docs/54 §4.3
  现状与 §10 批 B 记录。

### 目标架构批 A：`VisibilityMask` 与 `Camera.enabled` 接线（docs/54 §4.2）

- **`RenderView::visibility` 此前是个没有读者的字段**——"字段存在"被当成了"能力存在"。
  现在实体侧新增 `Visibility` 组件（`layers=<遮罩>`，收十进制/`0x`/`0b`，未知键、空串、非整数
  **报错**），视图侧遮罩仍由宿主用 `RuntimeScene::set_view_visibility(world, overlay)` 声明，
  判定是**相交**：`实体 layers ∩ 视图 visibility != 0` 才画。
- **零改动语义**：实体**没有** `Visibility` 组件 ⇒ 全可见，因此既有场景一字不改、像素不变。
  被遮掉的实体是"不存在于这个视图"（不产顶点、不占默认绘制层序号、不计入计数），每帧被遮掉的
  数量记在 `RuntimeScene::view_filtered`——"东西没画出来"因此能一眼分清是遮罩拦的还是没提取到。
- **遮罩只作用于有实体身份的内容**：世界精灵与 3D Mesh 吃 world 遮罩，`UiImage`/`UiText` 吃
  overlay 遮罩；按名字注册的画布 UI（面板/血条/调试网格）不是实体，永远照画。
  三条测试各钉一条边界：`view_visibility_mask_filters_world_sprites` /
  `..._3d_meshes` / `..._entity_backed_ui_only`。
- **Lua 只读写实体侧**：新增 `visibility.get/set/all`（`get` 对缺组件返回 `all()` 而不是 0——0 是
  "这个视图什么都不画"的另一个含义）。视图侧遮罩属于宿主声明，脚本改不到，否则"脚本把东西藏起来"
  与"视图决定看哪些层"会演化成两件事。
- **`Camera.enabled`**：`Camera2D`/`Camera3D` 新增 `enabled`（默认 `true`，且**启用时不写出字段**，
  既有场景串因此逐字节不变；禁用才写 `;enabled=false`）。禁用的相机不参与选帧：不会成为场景声明的
  初始 2D 相机、不会被 `camera.set_active` 选中（选中已禁用相机时 `tracing::warn!` 告警并回退到
  其它候选）。Inspector 补了复选框与"已禁用"摘要——否则用户会看到一台参数正常却完全不参与渲染的
  相机；`camera3d_enabled_round_trips` 钉住编解码不丢开关。
- **本批仍未接线**：`Camera.Order` / `ClearPolicy` 仍是宿主持有（顺序由 `RenderLayer`/overlay 表达）；
  编辑器侧尚未给实体声明 `Visibility`（需先设置再提取，否则有一帧延迟）。
- 文档：[docs/16](docs/16-scene-format.md)（`Visibility` 组件、`Camera2D`/`Camera3D` 的 `enabled`）、
  [docs/18](docs/18-lua-api.md)（`visibility` 表）、docs/54 §4.2 现状与 §10 批 A 记录。

### 3D 光照与阴影：修掉符号缺陷并立起像素门（§8-⑩）

- **缺陷**：`Light::direction` 的约定是"光传播方向"（`core/light.rs`、`docs/24`、着色器
  `l = normalize(-light.direction)` 三处一致），但 `renderer::light_view_projection` 把阴影相机放在
  **`+direction`** 一侧——符号相反；示例场景又都写成 `direction=0.4,0.8,0.5`（y>0 ＝ 光往上照）。
  后果：朝上的表面 `diffuse = 0`，整个 3D 场景只有 `0.15` 环境光，**阴影完全不可见**。
  实测（像素探针）：demo3d 地面恒为环境色 `(55,60,67)`，阴影落点与受光点都是 `(55,60,67)`。
- **修复**：阴影相机改为放在 `-direction` 一侧（光源那一侧）；`Light::default()`、渲染器初始
  uniform、编辑器默认灯与五个示例场景（demo3d / game3d / physics3d / mixed / unified）的太阳方向
  改为 y 分量为负（mixed/unified 的墙面改为朝相机方向来光）。现在地面受光 `(143,155,172)`，
  阴影落点 `(55,60,67)`。
- **立门**（防止再次写反）：
  - 单测 `host::renderer::tests::shadow_camera_sits_on_the_light_side_of_the_documented_direction`
    ——用"离光源更近的点在光空间深度更小"钉住相机一侧，并验证方向取反后关系反转；
  - 像素门 `engine/crates/engine/examples/wp0_lighting_golden.rs`（报告 `wp0-lighting-golden-v1`）：
    ① 漫反射随光照方向变化（把**光**取反，而不是把地面翻转——翻转会连带翻转绕序被背面剔除，
    本轮踩到过）；② 阴影把地面压暗（亮度 59.4 vs 153.7）；③ 阴影的**世界足迹**必须落在
    "遮挡物足迹 + 光照偏移"处（x∈(-0.62,1.25)、z∈(-0.75,1.12)），这一条顺带抓出了"只缩 x/y 的
    ±4 面当 2×2 用"的建模错误。三者都在 `verify.ps1 -Benchmarks` 里逐条判红。
- 文档：`docs/24` 写清符号约定与阴影相机所在侧、`docs/16` 的 `Light` 条目补上"太阳 y 为负"。

### Web 证据补齐：真发布包在浏览器里跑起来（§8-⑨ 收尾）

- **WASM 产物层现在是真证据，不再是"环境受限跳过"**。`rustup target add wasm32-unknown-unknown`
  之前一直 403 失败，原因不是网络：rustup 下载已装 toolchain 的组件时用的是那份 toolchain
  **manifest 里烘死的绝对 URL**（`mirrors.tuna.tsinghua.edu.cn/...`，该文件 403），
  `RUSTUP_DIST_SERVER` 对它无效。把 manifest 的镜像前缀换成 `https://static.rust-lang.org/dist/`
  后目标装成，`wasm-bindgen-cli 0.2.127` 由 `package.ps1` 自动安装。
- 发布 `examples/game_jhyt/builds/webgl/jianghu_youtian`（675 文件 / 21.8 MB）并在无头 Edge 里实测：
  `wasm module ok: 39.6 KB`、`boot=running`、`missing_host_api=0`、canvas 采样 15,987、UI 3 个、
  世界自报 `scene=17 map=main alive=17`、状态行 `jianghu_youtian_day_1` —— **Lua 在浏览器里真的跑起来了**。
- **门加硬**：`verify.ps1 -Web` 的两层都要求**本次**产生的摘要（`LastWriteTime >= 子进程启动时刻`），
  并抄录子进程 stderr；产物层断言 `boot ∈ {ok,booted,running}` 且 `drawnSamples > 0`。
  这三条正是本轮踩到的坑：`verify-webgl.ps1` 我给它加中文提示时丢了 UTF-8 BOM，WinPS 按 GBK 解码
  吃掉了字符串终止符 → 解析失败、stdout 空，而门当时只看退出码 + 目录里最新的摘要，于是把**上一轮的
  陈旧摘要**当成了本次通过。
- **没有任何 webgl 产物时 `-Web` 会就地发布一份**（wasm 构建 + wasm-bindgen 胶水）再验证，
  因此 CI 上这一步不再必然 skip；已有产物（含多候选歧义）时不动它。
- 文档同步：docs/32 的"已知限制"按实测更新（宿主 API 子集不再是启动阻塞、fengari 已随包分发不再
  依赖 CDN、wasm32 目标的真实卡点与修法）；docs/54 §10 第 ⑨ 行补上产物层实测数字与上述门规则。

### 性能门的 A/B 真正跑起来（§8-⑩）

- **提交进版本库的结构性基线**：新增 `baselines/wp0-3d-stress.json`（+ `baselines/README.md`
  说明口径与更新流程）。`verify.ps1 -Benchmarks` 每次都把新鲜的 3D 报告与它比一次——
  `compare-baseline.ps1` 新增 `-GeometryOnly`：几何量（draw/instances/triangles）与分配峰值
  **判红**（跨机可比），时间（p95/p99）只打印不判定（机器相关）。此前阈值逻辑只有自测，
  **从来没有和真实基线比过**，"3D 没有超阈回退"因此没有可执行依据；同机 A/B 仍走完整模式。
- 阈值自测从 4 例扩到 7 例（新增 `-GeometryOnly` 三例：跳过时间、内存仍判红、几何量仍判红）。
- **顺带发现一个真实缺陷（未修，已登记）**：`Light::direction` 的约定是"光照传播方向"
  （`core/light.rs` 与 `docs/24` 一致，着色器取反得到表面→光源向量），而
  `renderer::light_view_projection` 把阴影相机放在 `+direction` 一侧——两者符号相反；示例场景又都
  写成 `direction=0.4,0.8,0.5`（y>0 ＝ 光往上照），于是朝上的表面 `diffuse = 0`、3D 场景实际只有
  0.15 的环境光，**阴影不可见**。证据：demo3d 地面像素恒为环境色 `(55,60,67)`；专门构造的俯视
  小场景里阴影落点 `(55,60,67)` 与受光点 `(55,60,68)` 相同。修法（下一批）：统一符号 + 把示例
  场景的太阳方向改成 y<0，再补"材质/阴影"像素门。

### 五种配置的可重复质量证据（§8-⑨）

- **编辑器路径有了可重复证据**：新增 `crates/editor/examples/editor_preview_golden.rs`
  （报告 `wp0-editor-preview-golden-v1`）。它在**无窗口**条件下把编辑器的链跑通：`SceneDocument`
  → `CommandHistory`（撤销/重做）→ `scene_space`（世界保持的编辑）→ `RuntimeScene` 预览一帧 →
  像素；断言"世界保持的编辑精确到位且父实体不动""撤销回到逐字节相同的文档""重做回到同一份编辑
  结果""预览的非背景像素数超阈值""同一份文档的两次预览逐字节相同"，并报告 30 次重建的 p50/p95
  （编辑器预览的性能证据）。进 `verify.ps1 -Benchmarks` 并逐条判红。
- **Web 平台证据进门**：`verify.ps1 -Web`（CI 新增同名步骤）依次跑浏览器契约层
  （`verify-webgl.ps1`：真实 Edge 无头 + Node 静态服务器跑资源获取/解析、WebGL2 能力、存储降级、
  缺失资源诊断；判据是摘要 JSON 的 `status=ok` 而不只是退出码）与产物启动层
  （`verify-webgl-bundle.ps1`：真发布包的 wasm 引导）。退出码语义严格区分：1 = 真失败（判红）、
  2/3 = 环境受限/无产物（打印原因后跳过，绝不把跳过说成通过）。
- **一个真实的坑**：这两个 Web 脚本会起 node 静态服务器与无头浏览器（长命孙进程），把它们的 stdout
  接进质量门的管道会让门永远等不到 EOF。门改用 `Start-Process -Wait` + 独立日志文件取退出码。
- 环境限制（不伪造）：本机 `rustup target add wasm32-unknown-unknown` 因镜像 403 失败，因此
  **WASM 产物层**只能打印 `webgl_bundle_skipped`；浏览器契约层实测通过。

### 空间真源与父先序遍历收敛（§8-②）

- **父先序遍历只剩一个驱动**：`spatial_transform::parent_first_plan`（泛型于实体标识类型）负责父链
  上溯、成环判定与坏数据分类；运行时的容错传播与加载期的严格求解都调用它。此前两处各写一遍，
  于是同一份坏数据在两条路上有**两套说法**（差别只剩"遇到问题怎么办"：保留局部值并诊断 vs 直接失败）。
- **坏数据分类精确化**：新增 `MissingParent`（悬空的 `parent`）、`ParentWithoutLocal`（父没有局部
  变换）、`UnresolvableAncestor`（祖先不可解）。此前"父没有局部变换"被报成 `Cycle`——让作者去找
  一个不存在的环；而"悬空父"被**静默**当成链根，子树因此少一层位移却毫无线索。
- **确定性**：成环时"环上成员与它们的后代都保留局部值"不再依赖实体 id 的大小顺序（旧实现里后代
  先被访问到就会被算进"环"）。链根语义（缺父 ⇒ 世界 = 自己的局部）与收敛前一致，只是现在会点名。
- **局部真源的审计**：`scene_space::{local_transform_truth, ambiguous_local_transform_entities,
  parent_link_issues}`；加载期把结论变成 `RuntimeScene::scene_ambiguous_transforms()` 与新增的
  `scene_parent_issues()`，并各记一条 warn。
- **仓库级判据**：`shipped_scenes_carry_exactly_one_local_transform_truth` 逐份检查随仓库发布的
  场景——无实体同存 `Transform`/`Transform3D`、无实体手写派生组件 `GlobalTransform`、无悬空
  `parent`（当前 56 份全绿）。`Transform`/`Transform3D` 的定位因此明确为**序列化边界格式**，
  引擎内的唯一权威值是 `LocalTransform`。

### 渲染层顺序与 HUD 隔离（§8-⑤）

- **`RenderLayer` 从标注变成契约**：2D 叠加通道不再照顶点缓冲的先后绘制，而是按
  `OVERLAY_DRAW_ORDER`（World2D → Overlay2D → ScreenUI）**分桶**，桶内保持原序
  （`core::layer::layered_indices`）。宿主本来就按这个顺序产出顶点，因此现有画面像素逐字节不变
  （2D golden 的泄漏/区域/行序/剔除断言全部仍然通过）；变的是"以后悄悄错序"再也不可能——
  HUD 被一段插到它后面的调试线盖住，此前类型检查抓不到。
- **HUD 隔离的像素级 golden**：新增 `wp0_hud_golden`（报告 `wp0-hud-golden-v1`）。同一个场景
  渲染两次：关闭屏幕 UI 时碰撞体轮廓所在像素是静态体的青色（证明那段叠加内容确实在那里），
  打开后同一像素必须变成 HUD 的颜色（证明 ScreenUI 在 Overlay2D 之后）；另加"HUD 矩形之外的
  世界内容不受影响""关闭 UI 时 HUD 区域露出世界精灵"两条防呆。`verify.ps1` 逐条判红。

### 迁移命令：报告、备份、回滚与弃用告警（§8-⑧）

- **幂等标记不再是第二步**：标记写进场景**自身**（`meta\tspatial\tlocal`），与迁移后的组件数据
  在同一次原子写里落盘。此前"先写场景、再写标记文件"之间存在窗口：进程在中间死掉会留下
  "已迁移但没标记"的场景，下次运行会二次施加、把子树又乘一遍父变换。回滚把 `.bak` 拷回去，
  标记随之消失；旧版旁路标记 `<scene>.spatial-local` 仍被认（已用旧命令迁过的工程不受影响）。
- **原子写 + 读回校验**：唯一临时名（带进程 id）→ 写 → `sync_all` → rename → **读回逐字节比对**，
  写盘前还有一次"能被自己解析回来且再序列化一致"的自检。任何一步失败都放弃写回并报错，源文件
  保持原样；`Scene::save` 的 `<path>.tmp` 共享临时名在并发迁移下会互相覆盖，因此迁移走自己的路径。
- **备份语义收紧**：`.bak` 存在但不是文件时报错，而不是当成"已有备份"继续（那会让用户以为有
  回滚点、其实没有）；`.bak` 仍只保留**最原始**的一份，迁移可反复运行。
- **退出码门**：有实体无法无损迁移或有权处理失败时，报告行变成 `spatial_migration_blocked` 并以
  退出码 2 结束（报告照样完整打印/落盘，不是静默失败）。
- **报告 artifact**：`--migrate-report <路径>` 把报告原样落盘；`verify.ps1` 每次跑示例工程的
  dry-run 并断言 `spatial_migration_ok` / `refused=0` / `removed=0`。
- **弃用告警**：按 docs/30《兼容性设计清单》的"已删除的旧设计"表扫描场景，逐条报出
  `<组件>.<旧字段> → 现在写在哪`（例如 `Text.x → UiLayout 的四边 anchors/offsets`）。此前旧场景
  只会以 `malformed field: x=5` 失败，作者看不出搬家方向。
- **失败路径有测试**：备份失败、写回失败、源文件缺失、空工程、被拒实体阻断各自断言源文件不被
  改动、且不留半写文件。

### 物理契约统一（§8-⑥）

- **3D 也有了权威与步前同步**。`PhysicsWorld3d::kinematic_positions`（只取动态刚体，位置读
  **ECS**）与 `sync_kinematic_bodies_3d`；宿主每步只做一次 `physics_sync`，2D/3D 一起同步，
  `last_physics_sync()` 把"这一步同步了几个"变成可断言的事实。`PhysicsStageOrder::Legacy`
  由**死开关**变成真回滚档：`Legacy` 下不做统一同步、2D 回写留在 `physics2d_step`（即收敛前的
  顺序），回归测试 `legacy_stage_order_actually_changes_the_pipeline`。
- **2D 查询不再是"另一件事"**。`physics.raycast` / `physics.overlap` 改为 Rapier **碰撞体**查询
  （形状、偏移、sensor 都参与），与 `physics3d.raycast/overlap` 同义；原先的"实体位置距离判定"
  改名为 `physics.positions_in_radius`。`physics.get_velocity` 与 3D 同名同形（`{x=,y=}`，
  数值下标保留）。示例工程 `examples/game_jhyt` 的调用点已同步改名。
- **查询吃分组**：两侧 `raycast`/`overlap` 新增可选尾参 `{layer=, mask=}`。缺省全收（老写法零改动），
  只给一侧时另一侧取 `0xFF`；判定与碰撞体自身的 layer/mask 用**同一套两位测试**。位域是 `u8`，
  越界/负数/非整数一律报错，不静默截断。
- **2D 场景能声明静态墙**：`Collider` 支持 `;dynamic=false`（与 `Collider3d` 同名同义）。此前写出来
  只会得到 "unknown field" 报错，静态体只能靠 `Tilemap` 或代码里的 `add_body(.., false, ..)` 表达。
- **调试契约两侧对齐**：`ColliderDebugShape` 与新的 `ColliderDebugShape3d` 都报出**世界姿态 +
  `sensor` + 是否动态**；碰撞体线框只在形状类型上定义一次（`wireframe_edges` → `wireframe_segments`），
  宿主不再内联形状数学；配色改为动态绿 / 固定青 / sensor 黄（三者物理响应不同）。2D 的线框
  顶点与收敛前逐字节一致（同样的 4 条边 / 24 边形）。3D 新增 `physics3d.debug_draw`——与 2D
  开关**互相独立**，线框用本帧 3D pass 实际用的相机投影到 overlay，因此"看到的位置"与
  `camera3d.ray` 命中的位置必然一致。
- **对拍测试**：`core/physics_parity` 用**同一个场景**在 2D/3D 两个世界上各跑一遍，逐项断言
  停靠位置、碰撞事件、同步来源（只同步动态体且取自 ECS）、查询命中碰撞体、分组过滤、调试字段
  与线框段数一致。"名字一样、语义不一样"这类漂移此前只能靠人眼发现。
- 新增 `Camera2D` / `ParticleEmitter` 组件串、`Collider` 的 `dynamic` 字段与查询分组参数已写入
  [docs/16](docs/16-scene-format.md) / [docs/18](docs/18-lua-api.md)。

### 渲染契约与混合遮挡（§8-③ / §8-④）

- **`RenderView` 现在携带视图投影矩阵**（`RenderView::view_projection`）。此前它只有投影**参数**
  （fov/near/far），视图矩阵仍由渲染器回查 `Scene3d.camera`、世界 Sprite 又从宿主单独取一份 2D 矩阵，
  "同一世界视图"没有单一来源。现在三个构造器（`main_3d` / `main_2d` / `overlay_2d`）各自从相机算出
  矩阵并随视图一起下发；渲染器优先消费声明里的那一份，只在宿主**没有声明视图**时才回退到
  `Scene3d.camera`（trace 级诊断，不刷屏）。相机退化时矩阵为 `None`，渲染器保留上一帧 uniform 并给出
  warning，不猜一个矩阵。
- **世界 Sprite 与 Mesh 共用相机、视口与深度附件**。此前世界 Sprite 通道绑的是**自己的**深度附件、
  用自己的 2D 矩阵、在网格 pass **之后**画——网格永远不可能遮挡精灵。现在只要宿主声明了主视图
  （`FrameViews::main`），世界 Sprite 就用主视图的矩阵与视口，并绑 3D 的那一份深度附件。
- **深度策略按"这一段是否全不透明"下发**：新增 `SpriteDrawBatch::depth_write`（由顶点颜色 alpha 决定，
  `tint_is_opaque`）。不透明世界内容（瓦片层、alpha=1 的精灵）在网格 pass **之前**写深度 ⇒
  **两个方向的遮挡都成立**；半透明内容（含世界文字）在网格 pass 之后测深度不写深度，按画家顺序叠加。
  新增管线 `DepthPass::WorldSpriteOpaque`；`depth_policies_are_pinned_per_pass` 逐条锁死四条管线。
- **像素级证据**（`world_sprites_and_meshes_occlude_each_other_in_one_view`，只用内建
  `__ground_plane__` 几何，不依赖示例工程）：精灵在网格前面时可见、移到网格**后面**时被网格遮住；
  半透明精灵仍能叠加。收敛前这两条不可能同时成立。
- **回滚档**：`set_world_sprite_depth(false)`（默认）仍走 overlay 路径；宿主不声明 `FrameViews` 时
  世界 Sprite 回到"自己的深度附件 + 显式矩阵 + 画家顺序"，与收敛前逐字节一致（既有像素测试不变）。
- **一个被 golden 抓到的真实误丢**：世界通道的可见域当时仍按 **2D 相机**（设计分辨率像素空间，
  默认中心 `(640,360)`）裁剪，而 3D 世界视图里 `(-40, 0)` 之类坐标清晰可见——场景一有 3D 内容，
  世界内容就会在**收集阶段**被静默丢掉。现在"世界通道的可见域 = 宿主声明的世界视图"：有 3D
  内容时世界通道一律不按 2D 相机剔除（保守不剔除，绝不误丢），纯 2D 画面仍按 2D 视口裁剪。
  回归测试 `world_channel_does_not_cull_by_the_2d_camera_when_the_scene_has_3d_content`。

### 编辑器与 Lua 的统一空间语义（§8-⑦）

- **空间语义收进引擎**：新增 `core::scene_space`（文档世界变换、`目标世界 → 新局部`父先序反解、
  换父 keep-world、2D/3D 组件串编码）与 `core::picking`（屏幕 → 视口 → 相机射线 → AABB/平面查询）。
  编辑器侧的 `document_space` 与 3D 点选现在只是这两个模块的转发，**同一语义只有一处实现**。
- **`pose=transform` 的相机改读世界变换**：运行时用 `world_transform_3d_lazy`（有传播结果走快路径，
  否则沿 `Parent` 链现算），因此不依赖"调用方本帧已经跑过传播"这个隐含顺序；文档侧
  `extract_serialized_scene_3d` 同样按文档层级求解。挂在父下的相机不再"看不清它在看哪里"。
- **Scene 视图 3D 覆层改读世界变换**：标记、点选与 Gizmo 的**初值**与 3D 画面同源，父链下的实体
  不再错位；Gizmo / 2D 拖拽写回都改成"目标世界变换 → 局部变换"（工具只表达世界意图）；
  层级面板拖拽换父提供 keep-world 语义，反解不了时拒绝写回/取消事务，不静默近似。
- **Lua**：`camera3d.ray/mouse_ray/pick_plane/world_to_screen/get`（与编辑器 picking 同入口）、
  `transform3d.world(id)`（3D 实体的世界 TRS，`transform3d.get` 仍是局部值）；新增公开的
  `script::entity_lua_id`（宿主把实体交给 Lua 时的唯一编码）。
- **三向对拍测试**：`document_runtime_and_lua_agree_on_the_world_transform` 对同一实体同时断言
  文档侧（`scene_space`）、运行时侧（`GlobalTransform`）与 Lua 侧（`transform3d.world`）的世界位置
  一致——任何一处退回局部值都会被抓住。

### 编辑器与 Lua 的统一空间语义（§8-⑦，第一批）

- **编辑态空间语义收进一处**：新增 `editor::document_space`（文档世界变换、`目标世界 → 新局部`
  父先序反解、换父 keep-world、2D/3D 组件串编码）。编辑态没有 ECS，此前标记/Gizmo 与运行时各算
  一套，于是出现"手柄在人不在的地方"和"拖完跳一下"。
- **Scene 视图 3D 覆层改读世界变换**：标记、点选与 Gizmo 的**初值**现在与 3D 画面同源
  （运行时可用时取它，否则按文档层级求解），父链下的实体不再错位。
- **Gizmo / 拖拽写回按父链反解**：2D 拖拽预览（→运行时与→文档）与 3D Gizmo 提交都改成
  "目标世界变换 → 局部变换"；父与子同批移动时按父先序求解，不会把父的位移叠加两次。
  反解不了（剪切/镜像/退化）时**拒绝写回**并给出提示，不静默近似。
- **换父提供 keep-world 语义**：层级面板拖拽换父时先记下世界矩阵、改完父再反解局部值，实体在
  画面上不跳走；拒绝时整笔事务取消（不留半成品）。
- **Lua 补上 3D 屏↔世界投影链**：`camera3d.ray(x, y)` / `camera3d.mouse_ray()` /
  `camera3d.world_to_screen(x, y, z)`（带 `visible`）/ `camera3d.get()`。宿主每帧把 3D pass
  **实际用的那台相机**与视口回填给 Lua，因此运行时、Lua 与编辑器 picking 走同一条
  `Camera3D::ray_from_screen` / `world_to_ndc`；没渲染过任何一帧时报错，不猜相机。

### 空间真源收敛（§8-②）

- **派生真源收敛为一份 `GlobalTransform`**：原先 2D/3D 各有一个派生组件
  （`WorldTransform(Transform)` / `WorldTransform3D(Transform3D)`），"谁是权威"取决于消费端读
  哪一个。现在只有一个 `GlobalTransform`，且它直接带**矩阵**（`Global(parent) * Local(child)` 的
  真值）与 2D/3D 两个**视图**。剪切/镜像的父子组合下矩阵依然精确（旧路径此时只能退回局部值），
  因此 3D 提取直接用派生矩阵，不再从分解出来的 TRS 重算一遍模型矩阵。
- **视图退化规则不变**（压不成 TRS 时退回局部值并记 `NotRepresentable`），所以 2D 像素行为与
  收敛前逐字节一致；关闭空间继承仍是回滚档（清掉派生组件、消费者退回局部值）。
- **"两份局部真源"必须被点名**：同一实体同时带 `Transform` 与 `Transform3D` 时，运行时传播报
  `AmbiguousLocalTransform`（矩阵按 3D 视图取，因为 3D 是 2D 的超集空间），加载期
  `RuntimeScene::scene_ambiguous_transforms()` 直接给出实体名并记 warn——不静默挑一个当权威。
- **两侧求解必须一致**：`runtime_propagation_agrees_with_the_scene_side_solver` 把"世界侧容错
  传播"与"加载侧严格求解"的世界矩阵逐元素对齐钉住，任何一侧改了组合规则立刻判红。

### 实体模型补全（§8-①）

- **`Camera2D` 成为实体组件**：`center=x,y;zoom=z`（+ 可选 `rotation` / `bounds`）与 `Camera3D`
  并列。声明了它的场景用它作为**初始** 2D 相机（Lua `camera2d.*` 与相机控制器之后照旧能移动它），
  没声明的场景保持历史默认——既有工程逐字节不变。此前"正交相机"只存在于宿主状态与 Lua 里，
  "同一 Entity 模型表达正交/透视相机"因此不成立。
- **`ParticleEmitter` 成为实体组件**：`count` / `rate` / `speed` / `life` / `size(_end)` /
  `color(_end)` / `gravity` / `drag` / `rotation(_speed)`，未知键与非法数值一律报错。新增
  `rate`（粒子/秒）表达**连续发射**：新增流水线格 `scene_emitters_step`（`particles_step` 依赖它）
  按 `rate × dt` 累计，发射位置取实体的**世界**位置；累计器保留分数部分（帧率变化不改变平均
  发射量），长帧最多补发 512 个。Lua `particles.burst` 不读 `rate`，因此既有脚本行为不变。
- **一份场景表达六类内容**：新增 `examples/game_api/scenes/unified.scene`（Sprite + Mesh +
  `Camera3D` + `Camera2D` + `ParticleEmitter` + `WorldText` + `Text`/`UiLayout`），
  测试 `one_scene_expresses_every_renderable_category` 用真实场景文件逐类断言——格式、Entity 模型、
  typed 编译三处任何一处漏了都会判红。

### 混合 2D/3D 质量证据与性能门（§8-⑨ / §8-⑩）

- **新增混合 golden**（`wp0_mixed_golden` + `examples/game_api/scenes/mixed.scene`）：一个透视主视图里
  同时有网格墙与三个精灵，逐像素断言三条**同时**成立——墙前面的不透明精灵可见、墙后面的精灵被墙
  完全遮住（像素与墙面参考值一致）、半透明精灵是叠加结果。只断言"可见"或只断言"被遮住"都不足以
  证明深度策略按不透明性下发，因此三条一起进 `verify.ps1` 契约（`wp0-mixed-golden-v1`）。
- **性能红线从 warning 改为阻断**：`compare-baseline.ps1` 现在把 p95 > +5%、p99 > +10%、
  内存峰值 > +10%、以及 draw/instance/triangle 变化一律判红（`exit 2`），新增内存阈值比较。
  它的自测（`test-compare-baseline.ps1`）**改正了一条把红线定义成成功的反向断言**：此前它要求
  "+6% / +11% 必须通过"，现在要求超阈必须失败（4 个用例）。两条脚本同时补上 UTF-8 BOM——它们由
  `powershell -File`（Windows PowerShell 5.1）以 ANSI 解码，无 BOM 的中文注释会把参数表解析坏。
- **基准与 golden 进 CI**：`.github/workflows/quality.yml` 新增 `-Benchmarks` 与
  `test-compare-baseline.ps1` 两步；`verify.ps1` 的 3D 契约新增"必须报出 `cpu_peak_bytes`"——
  没有测量就没有判定。

## Unreleased — 引擎全面重构（WP0 关闭 + WP1/WP2 原型）

主题：把 2D 与 3D 收敛到**一套**变换、相机、调度与提取管线上。每项能力都带可回滚开关与像素级证据，不使用静默近似。
WP0（基准与证据门）已正式关闭，WP1/WP2 进入生产接线。

### 变换与场景（WP1 / WP2 / WP5 / WP9）

- **Transform 收敛**：`transform3d.rs` 并入 `transform.rs`，变换只有一个住处。新增 2D ↔ 3D 无损互转——`Transform::to_3d()` 永远无损，`Transform3D::to_2d()` 只在**真能无损压平**时成立，带倾斜 / 绕 X-Y 旋转 / z 位移或 z 缩放的变换一律拒绝，而不是悄悄丢掉一个分量。
- **空间继承**：场景 `parent` 一律按**局部空间**解释（默认开启）。组合走矩阵再分解；非均匀父缩放 × 子旋转产生的剪切无法用 TRS 表达，该实体保留局部值并记 `NotRepresentable` 诊断——绝不近似。回滚档 `set_spatial_inheritance(false)`，关闭时不产生任何世界变换组件，与旧行为逐字节一致。
- **迁移命令**：`--migrate-spatial`（报告）/ `--migrate-spatial-write`（写回，先留 `.bak`）/ `--restore-spatial`（回滚）。迁移**不可重复施加**（标记文件存在即跳过，因此可安全反复运行）；无法无损建立继承的实体被拒绝并清掉 `parent`、世界位置不变，且计入 `refused`，可当质量门。
- **Lua 变换 API 统一**：`transform` 表按实体**实际拥有**的组件分派，2D/3D 实体共用 `get`/`set`/`move`，新增 `space(id)`、`attach(id, '2d'|'3d')` 与 `transform.world(id)`——后者在父子关系下是唯一能拿到"屏幕上实际在哪"的接口。`set` 的标量 `rotation` 在 3D 下刻意报错（2D 是绕 Z 弧度、3D 是四元数），被拒绝的调用不留半写状态。
- **相机收敛**：2D 相机、3D 相机与 2D 控制器合并到 `core/camera.rs`。新增统一投影层 `Projection` / `ClearPolicy` / `RenderView` / `CameraSelection`，渲染器消费视图描述而不回查 ECS；2D 控制器的"请求消费 → 跟随 → advance → 边界钳制"次序固定在类型内部，宿主与控制器各自的钳制副本被删除——夹取规则只有一份。
- **`Camera3D pose=transform`**（默认关闭）：相机位置 = 实体 `Transform3D` 位置、视线 = 旋转后的 -Z、上方向 = 旋转后的 +Y；缺 `Transform3D` 或姿态退化时回退组件姿态并留诊断。
- **编辑器与运行时同一套投影**：Scene 视图原先**五处**各自手写的面板映射收敛为直接调用运行时 `Camera2D`；拾取包围框改用世界变换（带父子关系的实体不再"框在人不在的地方"）；多相机选择收口到 `CameraSelection`。
- **Inspector 单一 Transform 面板**：只有一种变换就按拥有者显示，两种都有时给出「编辑：2D / 3D」模式开关，并**始终**标出"空间：2D / 3D / 2D + 3D"——被切走的那种只是没在编辑，不是消失了。
- **Camera 面板只读摘要**：以统一 `Projection` 表述投影，并给出生效姿态。此前 `pose=transform` 打开时面板上那两组"位置/目标"其实不生效，却看不出区别。
- **Typed Scene（WP1）**：`ComponentId`/`TypedComponent` 覆盖真实源场景出现过的**全部 17 个组件名**，26 个源场景无诊断编译且与旧提取路径逐一一致。`MeshRenderer.asset` 走资源库真实身份，解析不到就是"未解析"，不再用按路径播种的假 ID 顶替。编辑器 3D 视图按 `SceneDocument.revision` 缓存编译结果，逐帧字符串解析点至此全部消除。
  - **范围修正（按证据，非跳过）**：原计划的"`RuntimeScene` 加 feature flag 双路径"经核实不应实施——运行时装载本就在加载边界只解析一次，并列挂 typed 编译会让装载期解析翻倍，替换 spawn 则帧内收益为 0 而改动面与 WP2/WP9 重叠。真正的替换式改造降级为 TODO，由装载期基准裁量。

### 渲染与提取（WP4 / WP6）

- **世界 Sprite 深度通道**（默认关闭）：世界坐标顶点 + 深度决定遮挡，替代"纯绘制顺序"。**精灵、瓦片层与世界文字都产出世界坐标顶点**（瓦片层先前被挡在通道外、世界文字先前是唯一仍留在屏幕空间的世界内容，世界内容一半参与深度、一半不参与，遮挡必然错；瓦片版与 overlay 版共用同一份几何，世界文字走 `view_projection` 的**逆映射**，因此屏幕观感与 overlay 路径逐个像素一致）。**排序契约（按负责人要求改为"测深度但不写深度 + 按 z 排序"）**：宿主把世界内容整体排成**画家顺序**——先 `layer`（作者声明的排序层压过深度）、同层内 z 从远到近、z 相同再按纹理归并；分段保证**深度一致**（跨 z 不合并），因此"一个分段一个 z"的前提成立。这样半透明精灵之间可以正常叠加，而不再有"远处那个透不出来"的代价。渲染器管线改为不写深度（`DepthPass::WorldSprite`），深度附件保留以便将来与 3D 共享深度缓冲。回滚档 `set_world_sprite_depth(false)`。**一个被门抓住的真实缺陷**：接线时把瓦片的世界绘制分段写进了 overlay 的 draw 列表——顶点数全对，但瓦片一个 draw range 都没登记，**2D overlay golden 立刻判红**；测试因此补上"分段必须落在世界列表里"的断言（"顶点对不对"与"有没有人画"是两件事）。
- **层描述显式化**：`RenderLayer::{World2D, Overlay2D, ScreenUI}` 让每个产出点显式声明所属渲染层，把"藏在调用点里的约定"变成类型与测试。文档写明 `layer`（同层内先后）≠ `render_layer`（属于哪个渲染层）。
- **视口外剔除**（瓦片与精灵各自可关）：不生成视口外的顶点与四边形。判定用外接圆半径，因此**与旋转无关**，旋转/负缩放/任意 pivot 都被保守覆盖；跨边缘的项一律保留。回滚档 `set_tilemap_culling(false)` / `set_sprite_culling(false)`。正确性由 2D golden 的**像素等价**证明（两个剔除开关的四种组合各渲染一次、逐字节相同），而不是推理。
- **提取路径去分配**：删掉逐帧构造的中间表与逐精灵的字符串克隆；收集循环改用 `for_each2`，消掉每精灵一次两层哈希查询；顶点**直接以 NDC** 追加进最终缓冲（`core` 仍不知道宿主顶点布局，架构测试继续通过）。场景 UI 布局解算不再为纯精灵实体建节点，且每帧少解算一次。
- **世界空间文字**：新增 `WorldText` 组件，锚在实体的世界位置、用 `Camera2D` 投影后走与屏幕 `Text` **完全相同**的字形管线。跨模块接线（场景实例化、Lua 挂载、typed 场景、**打包字符收集**）一并补齐——漏掉最后一项会表现为发布包缺字。
- **Sprite 翻转**：`flip_x` / `flip_y` 只改**采样方向**而不改 UV 范围，因此 `uv_min < uv_max` 校验仍成立；只在为 true 时写回组件串，既有场景规范字符串不变。
- **粒子发射参数与曲线**：速度/寿命/颜色的起止曲线、重力、阻力、自转与角速度，全部有确定性种子与退化参数校验（非有限 / `life<=0` / `drag<0` 一律报错且不生成粒子）。`particles.burst` 新增可选 `opts`，**缺省值与旧行为一致**，老脚本零改动。
- **动画帧事件**：`AnimationClip` 支持 `:events=<帧>:<名>`，命中判定、脚步、尘土这类必须挂在具体帧上的逻辑不再靠脚本数帧（改帧率或换贴图就会对不上）。单步跨多帧时中间帧事件**全部触发**（掉帧追帧不漏命中帧），循环回绕后同帧事件再次触发。
- **collider 调试绘制**：读 Rapier 中碰撞体的**当前**姿态与形状，而非场景里的 `Collider` 组件串（后者只在实例化时解析一次，而动态刚体每步都在动）。局部偏移随刚体旋转叠加——直接拿 `collider.translation()` 会把父体位移算两遍。sensor 用不同颜色（它不产生物理响应，颜色相同会误判），只多 1 个 draw call。

### 深度与排序策略的一处重要修正（本轮）

- **世界通道从"写深度"改成"测深度但不写深度 + 按 z 排序"**（按负责人要求）：宿主把世界内容整体排成画家顺序——先 `layer`（作者声明的排序层压过深度）、同层内 z 从远到近、z 相同再按纹理归并，分段保证**深度一致**（跨 z 不合并）。半透明精灵之间因此可以正常叠加，不再有"更远的精灵透不出来"的代价；代价是**顺序成为宿主的责任**，两条渲染器像素测试把这份契约钉住（画家顺序⇒近处胜出；顺序反了⇒远处胜出，说明深度不兜底）。
- **同时修掉一个此前被漏掉的真实回退**：三条管线（3D 网格 / 阴影 / 世界 Sprite）的 `depth_write_enabled` 曾是散落的字段字面量，一次脚本化编辑把 **3D 网格与阴影**误改成不写深度——3D 前后遮挡静默失效，而当时的 golden 只断言可绘制对象、实例/三角形与非背景像素，**断言不了深度顺序**，所以全门绿灯。现在深度策略收在 `DepthPass` 类型里，并由 `depth_policies_are_pinned_per_pass` 逐条锁死（网格与阴影必须写、世界 Sprite 不写、三者比较函数一致），要改必须先改类型、改错立刻判红。
### 物理与调度（WP7）

- **统一 scheduler 阶段**：新增 `Sync` / `Writeback` / `Spatial` 三格，把此前只存在于注释与调用顺序里的约定变成类型。帧内顺序固定为 **脚本 → 同步 → 物理 → 回写 →（模拟/动画）→ 世界变换 → 提取**。**回写时机真的改了**：`step_physics`/`step_physics3d` 只推进，回写由 `Writeback` 统一做，因此 2D 与 3D 的所有推进都发生在任何回写之前（此前 2D 的回写夹在 3D 推进之前，混合场景里"谁先看到谁"取决于注册顺序）。回滚档 `set_physics_stage_order(Unified|Legacy)`。
- **`PhysicsAuthority`**：声明物理模拟与 ECS 谁是权威（默认 `Physics` = 模拟权威，步后回写 ECS，与既有行为逐字节一致；`Script` = ECS 权威，步前推给模拟、步后跳过回写）。回滚档 `set_physics_authority(...)`。
- **连续碰撞配置（CCD）**：`Collider`/`Collider3d` 组件串可选 `;ccd=true`（默认关闭，与 Rapier 一致；拼错报错而非静默当 false），Lua `physics.set_ccd/get_ccd` 与 `physics3d.*`。`get` 读的是 Rapier 里的**真实状态**而非引擎侧影子记录，因此"声明到底生效没有"可直接验证。行为证据是 2D/3D 各一条 A/B——不开 CCD 时小球穿过薄墙，开了停在墙前。

### 性能与存储（WP0 / WP3 / WP8）

- **ECS dense 存储**：生产 `World` 组件存储从 `HashMap` + 排序实体表换成 dense 值数组 + sparse 槽位 + **惰性重建**的有序 id 索引。用惰性而非"插入时二分插入"，因为后者每次插入要 O(n) memmove。对外契约逐条不变（升序遍历、陈旧句柄报 `InvalidEntity`、增删查语义），稳态帧零分配、零哈希。
- **合批顶点并行**：新增 `core/par.rs` 的数据并行执行器——输入输出按块配对后**就地写入**，全程安全代码，且串行与并行输出**逐字节相同**。只并行"每精灵顶点生成"这一段（读只读条目、写自己的 6 个顶点，无跨精灵状态），排序与 draw range 合并仍在调用线程按原序做。阈值 2 万项以下完全走原串行路径，行为与引入前逐字节一致。
- **测量基础收口（WP0，已关闭）**：固定 ECS/Sprite/Tilemap 基准入口与统一报告口径、公开逐帧渲染指标、3D 与 2D semantic golden（**不使用跨 GPU 精确整图 hash**）、可选整帧 GPU timestamp（不支持时输出 `null` 加原因，**绝不以 CPU 时间冒充 GPU 时间**）。粒子预算已进质量门：1 万 CPU 粒子 < 4 ms，实测余量约 2 倍，超预算直接判红。
  - **口径修正（决定后续所有对照量尺）**：实测运行帧内 `from_component_str` 为 **0**——真正每帧付费的是**分配与克隆**。因此 WP1-5、WP3、WP6 的对照量尺是**每帧分配**，而不是"帧内字符串解析次数"，否则会得到一个不证明任何收益的假绿灯。详见 [docs/53](docs/53-WP0增量场景帧基准与每帧分配.md)。
- **2D overlay 语义 golden**：断言分三层——结构精确值 / 位置 / **"预期矩形之外的非背景像素必须为 0"**。第三条不依赖任何整图 hash 却能把 NDC 与偏移错误直接钉死。顺带固定了一条从未被文档写过的隐含约定：**`tiles` 数组的第 0 行画在地图最下方**——以后若有人改成"第 0 行在上"，门会立刻判红，把一次静默的视觉翻转变成必须显式决定的行为变更。

### 治理与已知限制

- **四项范围裁定**（范围决策门全部关闭，按最彻底方案执行）：`parent` 一律按局部空间解释；`World` 存储整体替换为 dense；世界 Sprite 移入带深度的 pass；一次性统一 scheduler 阶段并同时改物理回写时机。裁定原文与验收证据清单见 `tasks/plan/` 各 WP 文件。
- **计划合并**：声明 [docs/38](docs/38-统一2D3D核心架构实施计划.md)（P0–P5）与 [docs/40](docs/40-引擎全面重构计划.md)（WP0–WP9，主计划）的对应关系，避免两份计划各自派任务；WP 任务文件进入 `tasks/plan/`。
- **如实登记的已知限制**：同机重复误差阈值目前是比较脚本 warning、尚未升级为 blocking；semantic golden 不使用跨 GPU 精确 hash；GPU bytes 不等于驱动实际驻留显存；GPU timestamp 样本异步滞后一帧，首帧可为 pending；无跨后端统一显存口径，已明确排除。世界文字仍是屏幕空间顶点（文字排版是像素空间的，转世界坐标需按 zoom 折算字号与偏移），是深度通道里唯一尚未迁移的一类。

## v0.15 — 规划（M19）；本节汇总 v0.15–v0.21 的未打标签增量

### 玩法垂直切片（M19）

- **M19-01 MVP 垂直切片**：在山庄 / 云泽村 / 青云山道三张地图上串成**可独立完成的 PC 流程**（新档 → 种田 → 进村 → 接单/主线 → 山道采集与战斗 → 交付 → 制药 → 睡觉 → 第二天读档 → 主线收束）。新增纯数据采集系统（`scripts/data/gather.lua` + `scripts/gather.lua`）、三个订单与三段主线。顺带修掉"站在农田旁与 NPC 说话时 E 被 `farm` 抢走"的交互候选排序缺陷。设计见 `examples/game_jhyt/docs/46-*`。
- **M19-02 游戏 UI、音频与新手引导**：状态 HUD（气血/体力/内力/时间/天气/金钱/地图/快捷栏）、背包/任务/暂停三个面板接入状态机、`tools/gen_audio.py` 纯标准库离线合成 15 个 WAV 并全部接线、十步新手引导剧本随存档恢复。补齐 MVP 闭环缺口——新档发放水壶（此前 `watering_can` 无获取途径且天气永不转雨，作物无法成长导致交付/制作剧本不可完成）。
- **M19-04 游戏级端到端回归与性能基线**：帧率一致性（30/60/120 Hz 下规则状态逐字段相同，证明固定步模拟下表现层不影响规则）、30 天长跑、20 次真实读档 + 100 次磁盘往返 + 3000 次快照恢复、发布目录隔离自测（复制到含空格且无源码的目录后 `--check` 退出 0）。文档 `docs/36`。
- **演武场场景重制**（内容项）：20×12 战斗灰盒 → 40×22 山庄练武场，复用山庄 tileset 且**无夯土 tile**（防 `farm.lua` 误垦）；战斗验收锚点一格未动。

### 输入与手柄（M20-06 ~ M22）

- **M20-06 手柄设备接入**：新增设备层 `GamepadBackend` / `GamepadFrame`（按钮 0..15、轴 0..5 以常量固化并与 `ActionMap::with_defaults` 一致），宿主接入与正式 game / 编辑器 Play / headless **共用同一路径**，`set_gamepad_backend` 供 headless 注入假后端。**依赖方案偏离**：原计划落地 `gilrs`，实测离线环境无法解析（探针工程验证 sparse 与 git 两种镜像均失败），为不阻塞目标改为零新增依赖的 XInput 实现并**保留后端抽象**，联网后新增 `GilrsBackend` 即可无缝替换。详见 `docs/33` 第 4 节。
- **M20-07 命名动作映射与 Lua 输入 API**：`ActionMap`（版本头 `rustlua-actions-v1`）与 `Binding`，默认 18 个动作；键盘与手柄在 `InputSnapshot` 层**合并**后统一判定；`[input].map` 段保序读写、保留注释与其他段。契约见 `docs/35`，ADR-0021。
- **M20-08 手柄重绑定面板与命名动作迁移**：默认动作表 18 → 36 项，新增**跨动作冲突诊断**（默认表保持"一个物理输入只属于一个动作"，`conflicts()` 恒空并有 `debug_assert` 防回归）；编辑器新增输入重绑定面板（捕获下一个键盘按键/手柄按钮/摇杆轴方向，捕获期间编辑器快捷键不可达）；`game_jhyt` 完成从物理键直读到命名动作的整体迁移，键盘行为与迁移前**逐项等价**。
- **M20-09 标题界面手柄/键盘导航**：焦点环持有选中项，方向键 / DPad / 左摇杆 / 数字键均可改选中，导航手感复用 `ui.input.ui_step()`（与游戏内面板同源）；`confirm` 与鼠标点击走**同一个入口**。同步修掉"改键与导航手感只在进入玩法地图时应用、标题菜单仍走默认绑定"的问题。
- **两套操作说明 + 菜单导航手感可配**：顶部提示与各面板底部提示改为按**最近使用的设备**二选一（判定只看"哪个输入在动"而非手柄是否连接），键位名从**当前绑定**推导，改键后两套提示自动同步。新增 `scripts/ui/hints.lua` 承载这块共享逻辑（面板不能 `require` `ui.input`——`input → dialogue` 会成环）。
- **M22 输入层收口 · 批 1（W5 + W3）**：输入分层与手感收口。
  - **分层修正**：死区常量与纯函数从 `host::gamepad` 下沉到 `core::input`，新增架构回归测试 `core_does_not_depend_on_host` 防止再次倒置。
  - **轴的数字边沿**：为每个轴方向维护滞回数字态，因此 `input.action_pressed("ui_up")` 对键盘方向键、DPad 与左摇杆**同义**。
  - **径向死区**：摇杆按模长判定并保持方向，消除逐轴死区造成的"斜向比正向灵敏"。
  - **时间制连发**：`input.action_repeat(name)` 按逻辑时间累加（与渲染帧率无关）；数字键/DPad 按住不连发，以保持"一次按下 = 一步"。
  - **设备判定下沉**：`ui/hints.lua` 因此删掉每帧 36 动作 × `bindings()` + 6 轴 + 16 按钮的全量扫描。
- **M22 输入层收口 · 批 2（W4 + W2）**：手柄采样管线统一 + 槽位热插拔 + 震动。
  - **0 步帧吞边沿**（真实缺陷）：帧率高于逻辑步时 `run_script_steps(0, …)` 仍会 poll 手柄，边沿被写进随后丢弃的快照克隆而**永久丢失**（`poller.last` 已推进，下一帧不再产生同一变化），而键鼠边沿在宿主侧会保留到下一有效帧。现改为只在 `steps > 0` 时采样设备，两条路径语义一致。
  - **槽位 / 热插拔**：后端新增 `poll_all() -> [Option<GamepadFrame>; 4]`，轮询器改为逐槽位跟踪并维护**粘性**活动槽位；切换/拔线时释放旧槽位仍按住的按钮与轴方向（修掉设备切换时的假释放/假按下）。
  - **震动**：`set_rumble` + XInput `XInputSetState` + Lua `input.rumble(low, high, ms)`。上限 5000ms、`duration_ms = 0` 立即停止，到期由 `poll_into` 每帧归零——因为 XInput 震动是"设置即持续"。
  - **仍未接线**：本地双人（`InputSnapshot` 仍是单个活动槽位的一份按钮/轴状态）；真实手柄实机步骤未执行。
- **M22 输入层收口 · 批 3（W1）：键身份从「字符」改为「物理键」**。
  - **为什么**：旧 `InputKey` 把键存成 `char`（可打印字符写字符本身、其余写 `Other(码位)`），于是 `Shift+1` 与 `1` 是两个键，`Tab`/`Enter`/`F5`/修饰键**根本无法表达**（只能借内部码位硬塞）；而同一串文本在 Lua 侧静默落 `Other(0)`、在 Rust 侧却报错——**同一份文本两套严格度**。
  - **改了什么**：`KeyCode` 是与 winit 0.30 **逐项对齐的 194 项物理键枚举**，由一张表同时生成枚举、`KeyCode::ALL` 与 `to_text`/`from_text`，因此**同一物理位置在任何键盘布局下都是同一个引擎键**（AZERTY 的 W 位置仍是 `KeyW`）。宿主转换表收敛为两张、都从引擎表派生，原先三份手写表全部删除（含编辑器预览的 60 项 `PREVIEW_KEYS`）。
  - **迁移与报错**：设置文件读 v1 写 v2，按 US-QWERTY 换算旧键名，重新保存即归一化；带 Shift 的字符在 v1 里**没有**独立键身份，这类**有损**映射逐条记进 `input.migration_notes()`，**不静默改键位**。未知名从静默落 `Other(0)` 改为**报错并回显原值**（旧实现下拼错的键名与真实按键无法区分）。
  - 任务文档 [tasks/done/M22-输入层收口.md](tasks/done/M22-输入层收口.md)（W1–W5 全部落地并过质量门），`docs/35` §8。

### 编辑器与调试工具

- **M19-03 生活模拟编辑器生产工具**：领域数据面板（复用工程统一校验入口 `scripts/validate_data.lua` 的 `check({ strict = true })`，与游戏启动校验、`game --check`、`editor --check` 同源）、PersistId 工作流（汇总全工程 `persist_id` 形成注册表，报告重复定义/同名冲突/悬空引用，Inspector 增删改并入单条 Undo）、领域只读覆盖层（2D 场景视图绘制出口格/spawn/地点格）、Tilemap 批量填充与区域复制粘贴（一次操作 = 单条 Undo）、Play 期间运行时调试面板。**限制**：出口/地点的可视化编辑降级为只读覆盖层 + 校验 + `文件:行` 定位——Lua 源文本外科回写无法并入以场景快照为单位的单条 Undo，见 `docs/25` §14.7。
- **M20-01 引擎资源校验与运行时诊断**：导入期 Lua 语法校验（只编译不执行）；**UTF-8 BOM 可操作诊断**（原只报 `unexpected symbol near '<\239>'`，现提示"含 UTF-8 BOM，请另存为 UTF-8 无 BOM"——PowerShell/记事本另存导致的真实问题）；领域数据统一校验入口由 `main.lua` 启动校验 / `game --check` / `editor --check` / 编辑器面板**共用同一实现**，规则仍全部来自各模块 `validate_all`，不在 Rust 侧硬编码 NPC/任务语义；发布前 `--check` 关键资源缺失以非零退出码阻断。**未完成**：RON 只做分隔符/字符串配平 + 行列定位，完整解析待引入 `ron` 依赖（离线环境无法拉取 crates）。
- **调试 overlay 改为带标签的性能面板**：原先 F3 overlay 只有一块绿色面板 + 四条无标注量条（既不知道哪条是什么，也读不到具体数值）。现为逐行标签 + 实时数值 + 量条，`FRAME/UPDATE/LUA` 超过 20ms 数值变红；文字用引擎**内置 5x7 位图字模**，因此 overlay 不依赖项目字体。
- **发布版默认关闭帧率 overlay**：独立 `game` 宿主原先**默认开启**，发布包启动即带开发者读数。改为默认关闭，按 **F3** 开关；关闭只影响绘制，`[perf]` 数值仍每帧统计写日志，因此性能基线与开关无关。

### 发布与质量门

- **M20-02 音频分组混音与 UI 焦点/滚动导航**：音量**总线**（实际音量 = 主 × 分组）、`play_on(bus, name)`、`settings.toml` 的 `[audio]` 段宽松解析 + 保序写回；`AudioSystem` 在**无输出设备**时改为静默降级（只记日志），保住"`audio.sfx` 永不抛错"语义。UI 焦点环与滚动窗口接入运行时（支持网格布局、边界环绕、偏移钳制），场景切换时清空。
- **M20-03 发布原子替换与失败回滚**：`package.ps1` 改为 **staging 优先构建**——全部内容生成在临时目录内完成，最终目录在整个构建期不被触碰；**原子替换**用同卷重命名（先整体重命名旧内容为 backup 再移入 staging），任一步失败则新内容退回 staging、旧内容从 backup 放回原位，构建阶段失败则丢弃 staging 且最终目录完全不受影响；遗留 `.staging-`/`.backup-` 目录默认按 24 小时阈值自动清理。新增 `-SkipBuild` 供快速重复发布。最终目录布局与资源包格式**均未改**。
- **M20-04 WebGL 浏览器集成回归（部分完成）**：新增零依赖浏览器契约检查页与 `scripts/verify-webgl.ps1`（检测 wasm32 目标 → 组装 web 包 → 起静态服务 → 无头 Edge/Chrome）。验证 WASM 入口依赖的平台契约：manifest 按 TAB 解析、`startup_scene` 可获取、WebGL2 探测、存储不可用时降级内存且不抛错、**缺失资源必须得到可诊断 404**（不得静默成功）。**环境阻塞的未完成项**：本机离线无法 `rustup target add wasm32-unknown-unknown`，未产出 `pkg/`，因此**浏览器内完整剧本未执行**，仅契约层验证；见 `docs/32`。
- **M20-05 Alpha/Beta/RC 质量门与发布清单**：定义**缺陷分级与阻断条件**（P0 阻断任何阶段 / P1 阻断出门 / P2 记录放行，且不允许用表现层修补掩盖规则错误、不得靠删测试降级通过）、三阶段门槛、命令矩阵、性能基线格式与 P2 已知限制清单。见 `docs/34`。
  - 顺带修掉两处让门禁必然失败的脚本缺陷：`verify.ps1` 把默认工程写死为 `examples`（示例重组后根目录已无 `project.toml`）；用数组 splat 调 `package.ps1` 时 PowerShell 按位置传参，`-SelfTest` 被当成 `-Platform` 的值触发 ValidateSet 失败。

### 缺陷修复（M22 追加）

- **标题界面按 W/S 完全无效**：顶部提示写着「W/S 或 方向键 选择」，但 `ui/title_menu.lua` 只读 `game.ui_step()`（默认绑方向键/DPad/左摇杆），而默认绑定表里 `move_up = W`——所有面板都是"先 `action_pressed("move_up")`、再退回 `ui_step()`"，标题菜单漏了这一层。
- **编辑器 Play 里游戏慢放**：`PreviewSession` 每个到期帧固定推进 `1/60` 秒，编辑器帧率低于 60fps 时逻辑时间走得比墙上时钟慢（发布版用固定步追帧，所以只有编辑器会这样）。改用与运行器同一个 `FixedTimeStep`，按**真实经过时间**决定本帧步数。
- **地图静态操作提示与 HUD 反馈叠字**：每张可玩地图的静态 `hud_hint` 实体与 `hud.lua` 的动态反馈/引导两行正好落在同一纵向区间（庄园的反馈压在山庄提示上，其余 6 张图的引导压在各自主提示上）。7 张地图的 `hud_hint` 统一移到地图标题正下方；改动同时落在 5 个场景生成器与 7 个 `.scene` 上，**重新生成后与提交的场景逐字节一致（仅这 7 行不同）**，证明生成器与场景未脱钩。
- **编辑器 Play 里键盘导航/点击偶发没反应**（而发布版正常）：根因是预览把**没跑过逻辑步的帧**也当成"帧边界"去消费输入边沿，而 `key_event` 只在"松开 → 按下"的跳变上产生边沿、**仍按住的键不会补发**，于是那一下**永久丢失**（按得再久也没用，必须松开重按）。编辑器渲染帧率越高，被节流跳过的帧占比越大，丢失概率越高——与"发布版没问题"完全一致。修复三处：只在本帧真的跑过固定步时才 `finish_frame`；轮询式桥接补一条 down + up 边沿以保持"一次按下 = 一步"；`action_repeat` 不再把"一步之内已松开"的点按整帧丢掉。`docs/35` §7 增补"边沿消费时机（四个宿主统一）"不变量，防止再漏一个宿主。
- **`main.scene` 按钮出现重叠文字（重影）**：门面按钮的静态文字实体与 `title_menu.lua` 的 `render()` **各画了一遍标签**，脚本又为让 `> ` 前缀落进左侧留白把 x 左移 22px——两层错位恰好 22px，加上脚本画纯白、场景是米色，肉眼就是"一明一暗两遍字"。修复：标签**单一来源** = 场景 Text 实体，脚本只补选中箭头（保持原 `> ` 位置），并收起老版本的 `title:label:*`（兼容热重载残留）。这是脚本层缺陷，编辑器与独立 `game` 走同一条路径，两种宿主都已修好。
- **标题界面选中高亮/标签看不见**（动画代码在跑但画不出来，两个叠加缺陷）：① `ui.hide(id)` 只作用于文本、对 `ui.panel` 建的动态面板**无效**，于是标题菜单"先 clear 再重画"隐藏不掉面板，上一次选中项的高亮框会一直留着；② 门面分支是 `render()` → `update()`，而 `update()` 开头的 `clear()` 会把本帧刚画的标签与顶部提示**隐藏**，宿主又是在 `update()` 返回后才取本帧顶点。修复：`ui.hide` 现在同时隐藏同 id 的动态面板（只改已存在的，不凭空创建）+ 门面改为 **update → render**；新增只读 `ui.panel_get(id)`，把"画出来了没有"变成可断言的事实。
- **菜单选中反馈缺失**：用方向键/DPad/摇杆换项时只有"高亮框换个位置"，**没有任何声音或动画**，手感上像"按了没选中"。补齐两重反馈——换项音效（`ui_move.wav` 离线合成，接线放在所有面板共用的 `panel_nav.lua`，**只在选中项真的变化时播一次**，按住连发不会每帧乱响）与高亮弹跳 + 呼吸动画（按固定步 `time.delta()` 推进，与帧率无关）；新增 `audio.played()` / `audio.played_recently(name)` 以诊断"界面没声音是没调用还是被静音"。
- **编辑器 Play 预览里手柄摇杆没反应**：`GamepadPoller::poll_into` 原语义是"只在按钮/轴**变化**时写入快照"——独立 `game` 宿主跨帧复用同一快照所以看不出问题，而编辑器预览每帧新建快照，于是推住摇杆只有**第一帧**有轴值、按住按钮从第二帧起就没有"按住"状态。改为**按住状态与轴值每帧写全量**、按下/松开边沿仍只在变化帧喂事件，断连清理语义不变。

### 环境阻塞登记

- **2026-09-13 多次蓝屏重启**（非代码缺陷）：Windows 系统日志为 bugcheck `0x116`（VIDEO_TDR_FAILURE，两次）与 `0x7e`（四次），伴随 `nvlddmkm` 事件 153；本机为 Intel Iris Xe + NVIDIA RTX 3060 Laptop 混合显卡。由 **GPU 显示驱动**引起（wgpu 负载 + 反复启停编辑器/headless 渲染进程是触发条件），与当轮修复无关（其中一次发生在 `cargo build` 期间，当时未轮询 XInput）。收敛措施：同一台机器不要并行跑多个 wgpu 进程，并在复测前更新显卡驱动。

- 对应任务：`M19-01` 至 `M19-04`、`M20-01` 至 `M20-08` **全部实现并归档至 `tasks/done/`**（`tasks/plan/` 已无 M19/M20 收尾项；M22 输入层专题亦已于 2026-09-15 归档）。

## v0.14 — 规划（M18）

- 战斗 Hitbox 与最小武学（M18-01，已实现）：纯数据招式与敌人注册表（`data/skills.lua`/`data/enemies.lua`，version=1 + `validate_all`）；`combat.lua` 战斗底座——60Hz 固定步帧号计时、朝向矩形 hitbox（`physics.overlap` 宽相 + AABB 精判）、`attack_id=攻击者:招式:序号` 与判定窗内**同一攻击对同一目标单次命中**、冷却/内力门槛、玩家气血 Lua 账本（引擎 health 表仅镜像调试）、战败冻结 + R 重开（回出生点复活敌人）、环形战斗日志（attack/hit/kill/death/respawn，含帧号）与 `hitbox_rect` 调试接口；`enemy.lua` idle/chase/attack 状态机（persist_id 去重生成、A* 追击、transform 逐帧推进、伤害用敌人自有字段）；`skills.lua` 武器加成（练习剑 damage=8 经 weapon 槽真实生效）；`main.lua`/`input.lua` 接入（J 普攻/K 武学、存档新增 combat/enemy provider、加载期校验扩展）；灰盒场景 `combat_arena.scene` + `data/maps.lua` 注册。headless（`jhyt_m18_combat`）十章验收纯数据注册与错误定位、进入演武场、普攻/去重/冷却、武器加成、武学内力、击杀、敌人追击攻击、战败重开、存档恢复气血与敌人账本、固定步一致性（批量/逐帧推进同结果）。
- 建筑占用、制作与升级（M18-02，已实现）：纯数据注册表 `data/buildings.lua`（w×h footprint、可建地表白名单、等级贴图、升级消耗、station）+ `data/recipes.lua`（station/材料/产出/游戏分钟耗时/等级门槛，version=1 + `validate_all` 交叉校验引用）；`building.lua` 多格占用与建造——逐格冲突原因 `out_of_bounds`/`farm_plot`/`building`/`ground`/`solid`/`exit`/`road`（顺序确定）、`preview` 供 HUD 与 headless 显示 footprint 与非法原因、放置失败零副作用、拆除（制作中拒绝 `crafting_pending`）、升级（满级/材料不足幂等拒绝）、运行期 Sprite 实体 + GroundMap 全格阻挡、`map.state("buildings")` 版本化快照与 `on_enter` 重放（实体 `is_alive` 判活，杜绝陈旧 id）；`crafting.lua` 制作设施——`clock` 游戏分钟推导完成时刻（跨天进位）、start 原子校验（unknown_recipe/no_station/station_level/busy/not_enough 均无副作用）、collect 用 `inv.add` leftover 判定背包满并保留任务、`map.state("crafting")` 持久化与读档重推剩余时间；接入 interaction（设施 priority 10 的 E 交互：开工/收取/剩余分钟 prompt）、input（B 建造模式、F 放置、X 拆除、U 升级、建造模式 Q 切换建筑、HUD 追加 footprint 预览）、`data/items.lua` 木料 timber + 货摊上架、main.lua 存档 provider/加载期校验/`on_enter` 接线；附带加固 `farm.redraw_all` 对陈旧 farm_id 的防御。headless（`jhyt_m18_build`）十章验收纯数据注册与运行期追加建筑+配方、七类冲突枚举、放置/拆除/重放三方一致、制作链路与剩余时间、原子性（材料不足/等级不足/背包满/拆除保护）、升级解锁高阶配方、建造模式与 HUD 预览文案、存档往返与长跑快照稳定。
- 统一运行时 Scheduler 接入（M18-03，已实现）：`core/scheduler.rs` 系统上下文泛型化（`Scheduler<C = World>`，回调不再要求 `Send`，运行时单线程）并新增 `Simulation` 阶段与 panic payload 诊断；`RuntimeScene::run_script_steps` 重写为共享帧调度管线——每固定步 `input_apply → lua_update → ui_click_dispatch → physics2d/3d_step → particles_step → clock_step → scene_transition`，调用尾 `integrate_velocity → animation_state_advance → animation3d_advance` 各执行一次，顺序与旧手工编排逐项一致；正式 game（`game_app.rs`）、编辑器 Play 预览（`preview_session.rs`）与 headless 执行共用这一入口；新增玩法系统经 `add_step_system`/`add_frame_system` 按阶段+依赖注册（重复名注册期拒绝，缺依赖/循环/panic 在运行期给出含系统名与阶段的可定位诊断），逐系统耗时经 `last_schedule_report()` 输出。附带收口：修复示例重组后 runtime_scene/editor/assetc 测试样例路径失配（39 个编辑器测试失败 → 恢复通过），修复 script 测试临时脚本时间戳碰撞的偶发互相覆盖。headless（engine 单元测试）验收内置阶段顺序、注册 API、依赖缺失/循环/panic 诊断与调度统计。
- 装备属性与关系声望模型（M18-04，已实现）：`stats.lua` 三层属性模型——`base`（角色基础，玩家 attack=2 保持 M18-01 数值锚点）+ `equipment`（装备槽求和，兼容旧 `damage` 字段视为 attack）+ `buff`（按游戏日过期的临时增益，同 stat+amount 幂等），`query(actor,key)` 统一出口，`skills.lua` 武学伤害改走装备+Buff 层（练习剑 strike 2+8=10 不变，Buff 即时生效）；`data/relationships.lua`（version=1 + `validate_all`：阵营范围/档位递增/NPC 与物品引用完整性）+ `relationship.lua` 运行时——NPC 关系 0..100 与阵营声望 [min,max] 双向 clamp、有界来源流水、`reward_once` 幂等奖励锚点、`gift`/`gift_facing` 送礼（偏好分值、not_giftable/not_enough/no_npc_near 明确拒绝）；接入：`quest.lua` 奖励新增 relationship/reputation kind、`apply_effects` 新增两类副作用（对话选项与 on_complete 共用，dialogue 校验白名单同步）、`inventory.use` 支持 `use.buff`（先应用后消耗）、input G 键面向送礼、`data/herb_tonic` 行气散（attack+3 持续 2 天）、main.lua 存档新增 stats/relationship provider（旧档缺省静默回退）、加载期校验扩展、`on_day_start` 驱动 Buff 过期。headless（`jhyt_m18_stats`）十章验收：纯数据注册、三层不重复叠加、换装可验证变化（13→10→2→10）、礼物与拒绝路径、对话/任务改关系声望、重复交付与 reward_once 幂等、边界 clamp、Buff 过期、存档往返与旧档缺 provider 兼容。
- 对应任务：`M18-01` 至 `M18-04`（均已实现并归档至 `tasks/done/`）。

## v0.13 — 规划（M17）

- 多地图与场景生命周期（M17-01，已实现）：新增 2D 传送 API `physics.set_position` 与 `map.goto` 落点/出生点语义；`data/maps.lua` 地图注册表 + `map_manager.lua` 统一 `on_enter`/出口注册；`main.lua` 收敛为标题分发与按存档地图 continue；跨图 `scene.map`/`__mapstate:*` 存档恢复，切图期锁输入。headless（`jhyt_m17`）验收山庄↔云泽村双向切图、入口落点、背包跨图不清空、农田明细恢复与跨 runtime continue。
- NPC 日程与路径执行（M17-02，已实现）：纯数据 NPC 注册表（`data/npcs.lua` 7 人）+ 活动地点（`data/places.lua`）+ 7 份日程（`data/schedules/*`）；`schedule.lua` 解析跨午夜区间与雨天替代地点，`npc.lua` 按 `clock` 分钟解析目标、用 A* 逐帧 `transform.set` 跟随路径（Lua 侧无物理体 API）、跨图按稳定 PersistId 去重重建；不可达/未知地点/时段缺失输出含 NPC、时间段、地图、目标点的去重诊断；接入 `main.lua` 的 `on_enter`/`tick`/存档 provider。headless（`jhyt_m17_npc`）验收 7 天日程稳定、跨图重建与读档后位置/活动一致。
- 对话系统与任务状态机（M17-03，已实现）：纯数据对话与任务（`data/dialogues/*`、`data/quests/*`，注册表 `index.lua`）；`quest.lua` 提供 not_started/in_progress/deliverable/completed/failed 五态状态机、item/var/flag 三类目标判定、**先落完成态再发奖**的幂等交付（重复交付返回 `already_completed`）、与对话共用的副作用解释器；`dialogue.lua` 提供节点推进、`quest`/`var`/`item` 选项条件过滤、`set_var`/`add_var`/`add_item`/`money` 等副作用与即时 UI 面板；`state.lua` 新增 `vars`/`var_events` 容器（写入携带来源与事件、有界 32 条、随存档持久化）；`main.lua`/`input.lua` 接入存档 provider、boot 加载期校验与 Dialogue 态输入分发。headless（`jhyt_m17_dialogue`）十章验收纯数据注册、条件隐藏、五态流转、重复交付幂等、条件不满足、副作用可回溯、读档恢复与 failed 态。
- 商店经济与物品使用闭环（M17-04，已实现）：纯数据商店注册表（`data/shops.lua`，商品/单价/每日限购/收购清单/营业窗口）+ `shop.lua` 运行时；营业时间以 `clock.now().minute_of_day` 左闭右开判定；购买/出售**原子执行**——`inv.add` 的 leftover 判定背包容量、`inv.remove` 的实际移除数判定持有量，任一条件失败即回滚并返回 `closed`/`poor`/`full`/`limit`/`not_sold`/`not_bought`/`not_enough` 等明确原因；每日限购按游戏日 `on_day_start` 清零并随存档持久化；`stock` 与 `buys` 按物品互斥以阻断套利；`data/items.lua` 补 `buy`/`sell`/`kind` 与 `use` 使用效果，`inventory.M.use` 改为「应用效果后再消耗」，无效果/已满拒绝且不消耗（硬风险③）；纯键盘商店面板（E 开、数字键选择、F 切页、+/- 数量、空格确认、Esc 关）与 `shop_keeper` 交互目标接入 `main.lua`/`input.lua`。headless（`jhyt_m17_shop`）九章验收纯数据注册、营业边界、购买/出售、UI 交互、拒绝路径原子性、重复交易、物品使用、每日刷新与读档恢复。
- 领域数据 Schema 与加载校验（M17-05，已实现）：地图/地点/NPC/日程/任务/对话/商店数据统一增加 `version = 1`，任务与对话注册表改为 `{ version, ids }`；各加载模块提供 `validate_all` 并由 `main.lua` 在进入玩法前聚合执行，覆盖默认字段、枚举、范围、对象 ID 唯一性与 NPC/地点/地图/物品/任务/对话引用完整性，任一错误以含数据文件、对象 ID 或数组路径、字段路径的诊断 `error` 终止启动；领域数据保持纯 Lua，新增内容无需修改 Rust 核心。headless（`jhyt_m17`/`jhyt_m17_dialogue`/`jhyt_m17_shop`）补充默认值、枚举、范围与引用错误检查。
- 对应任务：`M17-01` 至 `M17-05`（均已实现并归档至 `tasks/done/`）。

## v0.12 — 2026-09-09（M16）

- 游戏输入上下文与玩法状态机：`state.lua` 提供 Playing/Dialogue/Inventory/Shop/Pause/Transition 六态与 `get_mode`/`is_mode`/`set_mode`/`toggle_pause`/`toggle_inventory`；`input.lua` 以 `p/i` 小写边沿键切暂停/背包，非 Playing 状态停止移动与 E/Q/T 交互、睡觉、修炼；状态字段入档并对旧存档回退 Playing。headless 验证：非 Playing 输入屏蔽、切换抑制。
- 通用世界交互框架：新增 `interaction.lua` 统一目标注册、面向查询、稳定排序、`can_interact`/`prompt`/`interact` 与失败原因；农田目标注册后 E 键统一走 `interaction.interact` → `farm.act`，保持原有翻土/播种/浇水/收获结果。headless 验证：面向农田格按 E 触发劳作并扣体力。
- 农田规则与日历天气基础：`calendar.lua` 按 28 天周期推导季节；`weather.lua` 支持晴/雨；雨天作物自动浇水且不耗体力、手动重复浇水返回 `already_watered`；种子由 `selected_seed` 决定并按固定快捷栏顺序回退；锄头/水壶/土地类型/作物季节前置校验；劳作时间记入 `labor_minutes` 不推进 clock。headless 验证：灵芝 7 天全流程（翻土→播种→浇水→雨天→成熟→收获 lingzhi×1）。修复：`farm.lua` 工具查询改走引擎 `inv.equip_list`、移除遮蔽引擎全局 `inv` 的局部别名。
- 游戏级存档增强与长跑验证：快照版本提升至 2，旧版明确拒绝；保存 state/inventory/clock/farm/scene（含地图状态摘要）；`summary`/`last_summary`/`validate_long_run(30,100)` 接口；玩家位置在场景实例化后经 `JianghuPlayer` 名称绑定恢复；commit 失败不更新摘要。headless 验证：day8 存档后第二 runtime 经 title→BtnContinue 恢复 main 场景与数值一致。
- 质量门：`cargo fmt -p game -- --check`、`cargo clippy -p game --all-targets --all-features`、`cargo test -p game cli::tests`（7/7）、`cargo build -p game` 与 `cargo run -p game --example jhyt_smoke` 全部通过。
- 对应任务：`M16-01` 至 `M16-04`（已 headless 验收，见 `tasks/`）。

## v0.11-editor — 2026-09-09

- 修复：编辑器修改实体属性后 Scene 视图 zoom/pan 被重置为适配值——「适配到设计
  分辨率包围框」收敛到文档整表切换（打开/新建/恢复）时触发，同文档编辑提交与
  Play/Stop 恢复快照不再重置用户滚轮/拖拽设置的视图状态。
- 项目标识改为 RustLua：新建项目脚手架与示例项目的 `project.toml` 中
  `engine = "p2"` 统一改为 `engine = "RustLua"`（该键目前无消费方，仅标识用途）。
- 项目设置面板：项目菜单新增「项目设置…」（未加载项目时禁用），独立弹出窗口编辑
  project.toml（名称/入口脚本/启动场景/字体/后备字体/字重；版本与引擎只读展示）。
  保存采用保序写回（保留注释与未知键），字体相关键变化时重建共享字体图集；
  引用的资源路径缺失时窗口内提示。
- Text 组件可指定字体：新增可选 `font` 字段（项目字体资源相对路径），Inspector
  下拉枚举项目字体；运行时按字体懒加载独立 SDF 图集，同一场景多 Text 可各用不同
  字体（同屏多图集），未指定/字体缺失回退项目默认字体；旧场景文件字节不变。
- 编辑器皮肤默认深色：固定 egui 深色主题，不随系统深浅色自动切换。
- 兼容层收口：新增 `docs/30-compatibility-designs.md` 统一兼容边界；UI 事件
  运行时只消费 `UiEvents` 并按绑定函数分发，旧 `clickEvent` 仅在场景实例化时
  转换；`Text.scale` 只读兼容并换算为最新 `font_size`，保存不再输出旧字段。
- 编辑器 UI 优化批次 E/F 收口：运行时场景接入共享容器布局解析，New Widget 走
  未保存保护并创建 Canvas 根文档；Game View / Console 消费去重预览诊断；
  Scene View 增加布局缓存性能 HUD；UI 动画隔离预览由主循环推进。
- 编辑器 UI 优化批次 E/F 数据层（E19–E27 部分）：
  - 新增 `component_schema` / `ui_events` / `ui_binding` / `preview_diagnostics` /
    `ui_animation`，覆盖类型化组件校验、结构化 UI 事件、单向属性绑定、预览诊断
    和隔离式动画预览。
  - 新增 `ui_scene_layout`：Canvas/UiLayout/Container 父链递归解析，容器受控
    子节点返回完整矩形，并诊断缺失/循环父级。
  - 新增 `ui_layout_cache`（实体/子树失效与命中统计）和 `editor_plugins`
    （版本化插件生命周期、加载/卸载与 panic 隔离）。
  - Prefab Inspector 新增 Unpack：一条 History 清理实例子树 `prefab.*` 元数据并
    保留当前覆盖数据。
- 编辑器 UI 优化批次 B（E11–E18 首轮）：
  - E16 引擎核心 `UiRectLayout`：四边 Anchor+Offset、Min/Max、Pivot、12 种预设；
    运行时 Text/Image 布局优先走 `UiLayout` 组件（旧内联数据完全兼容），
    Inspector 提供预设/数值编辑并接入编辑事务。
  - E17 容器自动布局首批 5 种（VBox/HBox/Grid/Margin/Center）纯函数布局引擎，
    Expand/Fill/对齐/内边距/间距，Inspector 可视化编辑。
  - E15 Canvas 数据层：参考分辨率 + 缩放模式（含均匀缩放）+ Safe Area。
  - E18 预览分辨率补齐 800×600 / 2560×1440 / 3440×1440 / 1080×1920。
  - E13 组件注册表分组（玩法/渲染/3D/UI）并注册 UiLayout/Container。
  - E11 交互词汇表与工具状态机抽出为独立单测模块（scene_interaction）。
- 引擎动态 UI 数据模型（panel/grid/image）与农场玩法 Lua API（`clock/calendar/day/grid/tilemap/item/inv/container/shop/map/data/save.blob_*`）。
- B-1 稳定实体 PersistId：`entity.spawn(name?, persist_id?)` / `entity.find_id(id)` / `entity.persist_id(e)`；`persist_id` 项目级唯一、重复注册报错，destroy/切场景自动清理（`game_jhyt/scripts/player.lua` 已用 `"player"` 接入）。
- Tilemap 单格字符串元数据 `meta`。
- WebGL 发布支持（`game_web` + wasm-bindgen，`package.ps1 -Platform webgl`）。
- 编辑器资源浏览器后台缩略图解码与资源树缓存、启动/导入与 Windows 体验优化。
- 导入/发布确定进度条与 Play/Stop 单按钮。
- 编辑器场景操作延迟修复：RuntimeScene 字体图集改惰性加载 + 场景视图/Play 预览共享已预热图集（场景编辑重建不再重复 MSDF 预热 ~2s）；MSDF 预热跨线程并行（debug 下 ~2.3s → ~0.7s）。

## v0.10-editor — 2026-08-23

- M10 编辑器强化基线：引擎内嵌游戏预览（Play 实时运行）、3D 透视视口 + 轨道导航、2D/3D 场景实时渲染、组件增删、Gizmo（Move/Rotate/Scale + 吸附）、资源缩略图与拖放、多选对齐、Prefab 集成。
- M10-11 面板拆分与四行布局；M10-12 编辑器优化与 TMP 风格 SDF 字体；M10-13 质量门与 GUI/GPU 证据规范。

## v0.9-3d-game — 2026-08-22

- M9 3D 可玩垂直切片：采用 rapier3d 3D 物理（[ADR-0008](docs/02-technical-decisions.md)）、渲染边界重构、编辑器 3D Gizmo（首版平移）、3D 样板游戏（玩家/敌人/拾取/血条/胜负，玩法全 Lua 驱动）。

## v0.8-3d-foundation — 2026-08-22

- M8 轻量 3D：glTF 2.0/GLB CPU 导入、Math3D（右手系 +Y 上）、Transform3D/Camera3D/MeshRenderer/Light 组件、深度缓冲、方向光阴影 + 金属粗糙度着色、骨骼动画（animation3d，GPU 蒙皮）、实例化与剔除、3D Demo 与发布验收。

## v0.7 — 2026-08-22

- M7 工程化与发布：assetc 资源构建 CLI（scan/import/clean/list/benchmark/package/check-deps）、发布打包（P2PK1 gzip + ChaCha20Poly1305）、崩溃日志、性能基线、干净机器测试、发布候选（压缩/加密/体积优化）、编辑器进度面板。

## v0.6-2d-gameplay — 2026-08-22

- M6 2D 游戏能力与样板游戏：运行时场景实例化与玩家移动、Tilemap、精灵与图块纹理、动画状态机、Rapier 2D 物理、UI/音频/粒子、存档与场景切换、样板游戏垂直切片。

## v0.5-editor-mvp — 2026-08-21

- M5 编辑器 MVP：项目打开/新建、场景树、Inspector、资源浏览器与刷新、Lua Console、Play/Stop 运行态隔离（场景副本快照）。

## v0.4-scripting — 2026-08-20

- M4 资产、场景与脚本：资源数据库、导入缓存、场景格式（版本化文本）、预制体、Lua API（mlua 5.4）、Lua 热重载、Lua 脚本调试与诊断。

## v0.3-2d — 2026-08-20

- M3 2D 渲染：纹理加载与上传、Sprite、SpriteBatch、Camera2D、图集、文本（SDF 字形）、2D Demo。

## v0.2-ecs — 2026-08-20

- M2 ECS 核心：World/Entity/Component、调度器。

## v0.1-foundation — 2026-08-20

- M1 基础运行时：窗口与应用生命周期、wgpu 初始化与清屏、Lua VM 桥接。

## v0.0.1-baseline — 2026-08-20

- M0 基线：工程结构、质量门与自动化检查、首次基线构建与版本标签。
