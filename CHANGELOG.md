# Changelog

版本采用 git 标签（`git tag`）；本文只记录**能力增量**——做了什么、为什么重要、回滚档是什么。

度量数字（µs、分配字节、测试计数）、逐条证据、golden 断言与实施细节留在能力文档与任务文件里，不在本文重复：
重构过程见 [docs/53](docs/53-WP0增量场景帧基准与每帧分配.md) 与 [docs/40](docs/40-引擎全面重构计划.md)，已完成任务的验收记录见 `tasks/done/`。

## Unreleased — wasm-only 探针不再拖红全量 clippy：`gpu-web-probe` 补目标位闸门、`Navigator` 自己声明

- **症状**：`cargo clippy --workspace --all-targets`（本地 pre-commit 与 CI 的 Clippy 步同口径）在
  `crates/gpu-web-probe` 上固定红：`E0433` 找不到 `engine::host::web_surface`、`E0277` ×2
  `map_async` 的回调不满足 `WasmNotSend`、`E0599` `Window::navigator` 不存在。此前它被当作
  "别人的在建工作"绕开——`tasks/done/M72-04` §8 明记 "`--exclude gpu-web-probe` 后 clippy 零告警"。
- **根因不在 feature 表**：这个 crate 通篇是浏览器里的异步问答——`web_surface` 在 engine 侧就是
  `#[cfg(target_arch = "wasm32")]`，而 `map_async` 的回调必须捕获 `Rc<RefCell<…>>`，非 wasm 目标上
  `WasmNotSend` 退化成 `Send` 就直接不成立。也就是说**宿主目标上它本来就无法编译**，补 feature、
  换 `Arc` 都治不到根。
- **改法**：① `src/lib.rs` 顶部加 crate 级 `#![cfg(target_arch = "wasm32")]`，注释里写清三条实测错误码、
  代价（宿主侧不再检查本 crate）与"仍留在 workspace members"的理由（`cargo tree -p game_web`
  那条"发布包不含本探针"的门要靠 members 才查得到）；② `Cargo.toml` 的 web-sys 特性补 `"Navigator"`
  ——`window.navigator()` 是探针自己调的，以前编得过是靠同一次构建里 engine 的 web 依赖把这项
  特性**合并**进来，属于借来的绿灯。
- **实测**：`cargo clippy -j 2 -p gpu-web-probe --all-targets -- -D warnings` 与带 `--all-features`
  同一条都 exit 0；正向对照（证明闸门没把探针变成空壳）`cargo build -j 2 -p gpu-web-probe --target
  wasm32-unknown-unknown` exit 0、debug 产物 324,920,099 字节，`gpu_probe` / `gpu_render_probe` /
  `storage_buffer` 仍在符号表里。只加 `Navigator`、不加闸门那一步的读数是 E0599 消失、E0433+E0277 剩
  3 条——这条差量就是"特性合并掩盖了缺声明"的证据。`cargo fmt -p gpu-web-probe -- --check` exit 0。
- **代价与未做**：宿主目标上这个 crate 从此不被任何 lint 检查，它的编译门只剩 wasm 那条构建路径
  （`scripts/gpu-web-probe.ps1` 走的就是它）；没有改 CI 的 `--workspace` 命令（改命令去迁就一个红，
  等于给所有 crate 开逃生门）。**同一轮回归里 `game_web` 仍红 15+2 条 dead_code**（`frame_input.rs`
  的 `apply_frame`/`FrameInputProducer`/`InputScript`/`ScriptDriver` 等、`fit.rs` 的 `contain_box`/
  `css_to_design`），它们只被 `gpu_host.rs`/`rust_scene.rs` 这两个
  `#[cfg(all(target_arch = "wasm32", feature = "gpu2d"))]` 模块使用，属同一类"宿主目标看不见消费者"
  的红——但那份工作正在被另一路改（`game_web/src/lib.rs` 有未提交在途内容），本批不碰。

## Unreleased — 同格 NPC 不再叠成一个人：日程落位在 32px 格内按 persist_id 错开

- **症状**：新档一开局（`main.lua` 的 `clock.set(1, 0)` = 00:00）山庄厢房 (12,6) 只看得见"一个人"，
  实际是顾长风与玄鹤完全重叠；云泽村赵宅 (16,8) 同档（阿七睡在赵铁算盘家）。
- **根因不在日程**：`data/places.lua` 一个地点只有一个 cell，而日程允许两人在同一时段引用同一地点。
  本次排查用一段临时脚本扫全量（16 份日程 × 1440 分钟 × 晴/雨两态）：晴天 18 个格出现 2~6 人同格
  （最挤是 19:00 云泽村茶摊 (32,8) 6 人、18:00 广场 (20,5) 3 人），雨天另有 10 组。
  逐个改点位既挡不住"集市/茶摊本来就多人"的语义，也要重排 `jhyt_m17_npc` 的点位断言。
- **改法**：[examples/game_jhyt/scripts/world/npc.lua](examples/game_jhyt/scripts/world/npc.lua) 每帧
  tick 末尾加一趟 `spread_standing`——把"已停在同一格"的本图 NPC 按 `npc_list()` 的 persist_id 升序
  发槽位，在格心之上叠一张封顶 ±12 的固定偏移表。只碰停下来的（`path` 已空且当前格 == 目标格），
  走路中的仍按格心走，否则每帧和 waypoint 抢位置。
- **实测**：`cargo run -p game --example jhyt_m17_npc` 新增断言读数 `400.0,208.0;388.0,206.0`
  （顾长风在格心、玄鹤偏西南 12,2），两人 `npc.query` 回读的 cell 仍是 12,6；
  反面对照：把 `spread_standing(here)` 一行注掉重跑 → `Error: "同格 NPC 未错开落位：400.0,208.0;400.0,208.0"`。
  连带 `jhyt_m35_depth_sort`（order 恒等于脚位 Y 的桶，场上 6 个 actor）、`jhyt_m48_npc`、
  `jhyt_m17_dialogue`、`jhyt_m18_stats` 四门绿。
- **代价 / 未测**：±12 是"必须留在本格内"（`grid.world_to_cell` 要读回同一格）逼出来的上界，
  所以 6 人同格时身子的贴图仍会部分重叠——效果是"看得出是几个人"，不是"每人独占一格"；
  表只给 11 个槽，第 12 人起回落格心（16 人全挤一格才会碰到）。遮挡先后仍由 `depth.tick` 按脚位 Y 重算，
  纵向偏移会改变同格两人谁盖谁（这是既有规则的正常输入，不是新增状态）。
  **没有**跑浏览器/真机目验，只看了 headless 坐标；web/微信宿主要在下一次打包时才拿到这份 Lua 改动。
- **回滚档**：revert 本次提交即可——日程与 `places.lua` 一字未动，`jhyt_m17_npc` 的那段断言随提交一起回退。

## Unreleased — web/微信投影贴片不再画成不透明白块：世界精灵吃 `Sprite.tint`

- **症状**：PC 上建筑投影贴片（白 blob 贴图 + `tint=0.122,0.137,0.157,0.280`）是半透深色，
  WebGL 与微信小游戏两个宿主上却是**不透明白块**（实机 2026-10-12，山庄）。
- **根因**：两宿主共用的 `plugins/webgl/web-runtime.js`（打包时复制）里，世界精灵绘制循环对带贴图的
  精灵只 `drawImage` 原图、**从不读 `Sprite.tint`**；原生片元是「贴图色 × tint、alpha 也相乘」，
  于是 web 侧 tint 整批丢失——白 blob 贴图原样上屏。带 tint 的装饰精灵（暖冷色变化）同样在 web 丢色。
- **改法**：tint 非默认时改走瓦片已有的两趟染色离屏画布（`multiply` 上色 + `destination-in` 回乘 alpha
  + tint.a 相乘），按「贴图+tint」缓存、首帧后只取缓存；默认 tint 仍走直绘快路。染色离屏画布盖
  `_src` = 源贴图路径：微信层带门（`--check-foreground-order`）按 drawImage 的 src 序列认投影贴片与
  遮挡物，不盖则主画布那次 drawImage 是空串 src、门整批失明；缓存外层预算 8→32（sprite tint 是
  每件小图一个外层键，单场景贴片+装饰+图集过十，8 会每帧淘汰=每帧重合成）。
- **实测**：微信无头 harness `wechat_minigame_harness_ok`（drawn=1486）；`--check-foreground-order` 绿
  （manor 16 张投影贴片全在主角前 `castMax=495 < playerIdx=547`、遮挡物在后、反面对照翻转）；
  浏览器像素 A/B（对照 = 同发布目录 + `git show HEAD:` 旧运行时）：旧运行时近白像素 18429、
  ≥40px 长白 run 177 行（纯 255,255,255），修复后 287/0；阴影像素 (63,76,103) 与邻石 (78,93,125)
  精确等于 `0.72×石色 + 0.28×tint` 的半透混合。
- **回滚档**：只动运行时 `plugins/webgl/web-runtime.js` 一个文件，引擎本体零改动；还原该文件重打包
  即回旧行为（白块复现，回滚可验）。

## Unreleased — 第二张图照金标规范跑通：云泽村从"6 种平色瓦"变成 102 槽 autotile + 326 件散布（M72-11 ①village）

- **这一包是 M72-11 的第一张**：plan 点名 7 张图，本包按"一张做满再一次 commit"只做满 `village`，
  剩下 5 张真场景 + `combat_arena` 图集解耦 + `main` 门面逐张另提。§目标 的两条硬约束守住了：
  **规范一字未改**（`docs/51` / `docs/57` 没动），**引擎逻辑一字未改**（`engine/**` 里只有
  `examples/jhyt_m72_building_footprint.rs` 这一把尺从"只量 manor"改成逐图量）。
- **症状**：村庄盘上是 `village_ground.png` 192×32（6 列 ×1 行、整幅拉伸），`.scene` 里 `atlas_cols=6`、
  `autotile=0` ⇒ 一屏只有 6 种瓦、边界全直线；8 栋建筑的"整块 solid"是手填的；屋顶与本体同层，
  人走到屋前不会被压住；装饰 0 件。
- **地表**：重做成 384×288 = 12 列 ×9 行 = 108 槽，前六列族本体语义与山庄逐列对齐，第七列起是
  6 族 ×17 档 autotile 过渡（寻址 102 槽 ≤ 108，也在 `tiles: Vec<u8>` 的 255 硬上限内）。取色全部走
  `terrain_ramp.json:scenes.village` 的 `tea_soil / flagstone / plaster / slate` 四族，出图侧
  "每个不透明像素必须出自表内"那条门因此测得到跨场景串色。盘上 kind 直方图
  `{墙160 草498 石板192 水38 夯土32}`、per-tile tint 非白 618 格。
- **碰撞不再手填**：`tools/building_kit.py` 按不透明像素紧包围盒算占地、每轴至少压进 1/4 格（8px）才算
  覆盖；道具分 `root / box / walk` 三档（石桥是 `walk`——按框标实心等于把唯一的过河通道封死）；门洞按
  规则挖（牌坊 (19,1)(19,2)、山门 (10,20)，每列最低那格实测不透明占比 0.156 ≤ 0.35）。对拍读数
  `solid 328 == 应实心 328，缺 0 / 悬空 0`；Rust 侧独立复算同一件事：
  `jhyt_m72_building_footprint_ok scenes=2 … village(solid=328 buildings=8 splits=7 shadows=12)`。
- **拆层与后处理**：7 栋拆成本体 `world` + 屋顶 `foreground` 遮断件（配对按**实体名前缀**而不是 role，
  否则屋檐那几格会被判成"悬空碰撞"而删掉实心，人能走进屋里）、12 件地面投影（够不够格不是名单：总高 ×
  `shadow_reach 0.12` ≥ ¼ 瓦 ⇒ 村里三栋 64px 矮屋落在门槛外不挂）、326 件确定性散布、4 张四季地表变体，
  合计 379 个实体。
- **泛化改到的公共件**（都是"原来只认 manor"的硬编码，不是新机制）：新增 `tools/layer_kit.py` 收
  `LAYER_SPLITS / OWNER_OF_STEM / split_stem`（原先住在 `gen_manor_art` 而被 interior 两份 import）；
  `building_kit.SCENES`、`set_scatter_decor.RESERVED_BY_SCENE`、`gen_season_variants` 改成按场景档；
  `gen_manor_art.tiled_roof` 加 `palette` 参数，山庄一侧不传即逐字节原样。
- **美术把数据逼着改的五处**（"画着一道门却走不进去"级别，不是口味）：`maps.lua` 的 `village_to_forge`
  出口格 (26,10)→(27,10)（旧值在铁匠铺自己 8 格占地的**里面**）、`forge_to_village` 落点同格；
  `places.lua` 三个 NPC 家搬出铺面 solid；山门贴图挪一行让它压上北墙、门洞落到出口格；
  `village_clinic` (5,14)→(8,14)、`village_gate` (19,2)→(19,3)。
- **新查出来并写进生成器的契约**：`scripts/world/map_manager.lua:register_exit_target` 给出口
  `priority = 20`，高于对话/商店的 10，而它的 query **既接受"站在出口格上"也接受"面向的那一格是出口格"**
  ⇒ 地点格与出口格四邻相邻时，按 E 会被切场景抢走。红法实录：`village_clinic` 贴着 `herb_door` (5,13)
  放时 `jhyt_m18_stats` 当场报 `mode = "Transition", want "Dialogue"`。现在 `gen_village_scene.py`
  自检第 6 条对每对 (place, exit) 判曼哈顿距离。
- **台账里的一条假账就地更正**：`docs/58` §1.3 写"金标地表只有 `manor` 一张、剩下 6 套待 M72-11"，
  本包之后是**两张**（`manor` + `village`），剩下 5 套仍是旧 6×1；同一张表的 village 行从
  `6×1 / autotile=0` 改成 `12×9 / autotile=1 / tint 618 格`。村庄文档 `docs/39` 的 §2–§6 整段换代，
  §5 那张分区表的"地表"列换成盘上实测直方图。
- **提交前自己抓到的一条落后账**：`set_scatter_decor --check` 报"盘上装饰段与重算不一致：3 行不同"
  （entity 249 盘上 `Decor_MossPatch`、重算 `Decor_Pebbles`）。散布的输入是 tiles/solid/tints 与留白表，
  而这些在本包后段都改过，段没跟着重跑 ⇒ 是**派生物落后于产出它的表**。按管线顺序重跑
  `set_scatter_decor / set_foreground_occluders / set_cast_shadows` 三条 `--apply` 后全绿，读数不变
  （326 件、窗口 58/94/128）；规矩同时写进 `docs/39` §6 与 plan。
- **新门与登记**：`tools/scene_readings.py` 是"逐图量化读数"那把尺（同屏可见瓦种类 ≥24、每屏装饰
  median ≥40；分档表 `MIN_DECOR_OBJECTS` 里没登记的场景**直接红**，工具不替室内猜一个数）。
  `verify.ps1` 的 Python 段现在对 `manor` + `village` 跑 `building_kit --check` + `scene_readings --gate`，
  另加村庄那条散布确定性门，`[SKIP]` 文案跟着改成四把尺。
- **读数**：最坏 20×12 窗口 37 种瓦外观（条目 21 + tint 16，判据 ≥24；manor 是 25）；每屏装饰
  min/median/max = 58/94/128（manor 7/41/75）；592 个非实心格里 589 可达，进不去 3 格
  （金标 manor 自己 4 片 11 格，所以这条判的是"与金标持平"）。D5 复核对两图都跑：8 片 ≥24 格同条目
  满块全落在 tint 读不出来的族上（0.29×~0.45×），详见 plan §D5。
- **红归因（工作树压着别的包，这一步不能省）**：`git archive HEAD` 的数据 + 本包覆盖层做成 `.temp/mine`
  全量扫 69 条门，红 8 条；这 8 条对**纯 HEAD** 树逐条复跑**同样全红** ⇒ HEAD=OK 而 MINE=RED 的计数为 0。
  其中 `jhyt_m31_pc_release` 两侧都是隔离树缺 `builds/pc` 的测台产物缺失，不是产品红；工作树里
  `jhyt_m17_npc` 的红归另一包的 `examples/game_jhyt/project.toml` 换字体（单文件换根复验判出）。
  两条隔离树都要额外放一份仓库根 `plugins/`，否则 `jhyt_m72_autotile_mask` ⑧ 与
  `jhyt_m72_tile_variant_determinism` 会报"读不到 `web-runtime.js` 路径"而不是数据红。
- **没有做的部分写清**：整屏目验在编辑器 / Web / 微信三个宿主这一轮一次没跑，本包只做到原生 headless 门 +
  桌面宿主预览图，收口在 M72-12；D5 的根因（抬 `ground_tints` 或让 autotile 分支查 `variant_for`）
  分别属于**规范**与**引擎**，按 §目标 登记不动手，交 M72-12 裁定要不要回炉 M72-07。
- **回滚档**：revert 本包即回到旧 6×1 村庄 + 手填 solid；色阶表、散布库、拆层表都在，照 `docs/39` §6
  那串命令可复现，不需要手工回滚二进制。

## Unreleased — 把"当时绿"和"现在绿"分开：9 份台账/规范回写实测读数，更正三处被当现状读的历史快照（M72-13）

- **症状**：M72 一到九每包都留下了自己的读数，但没有一处负责把它们收回文档。于是同一个数字在
  仓库里有两套——`docs/33` §0 的 3.0 条目与 `docs/60` §3 仍写"基线 OK 112 / MISSING 107"、
  "图标 ≈130"、"64 张纯色占位图"，而盘上实测是 **OK 205 / MISSING 13、图标 92 张、纯色块 0 张**；
  `docs/71` 写"`licenses/ASSET_REGISTER.tsv` 仍未建"，而那张表已经在库里；`docs/51` 还在用
  "4 方向"描述角色，而 `jhyt_m21_appearance` 的门禁写的是 **8 向 × 7 动作 = 56 clip**
  （`clips != 56` 即红，注释里那句"8 向 × 7 动作"就在断言旁边）。
  这类句子的共同点不是"写错"，是**写对过一次之后再没被更新**，而读者按绿灯读它们。
  （一处例外要如实分开：`docs/71` 那句"`licenses/ASSET_REGISTER.tsv` 仍未建"**不是假账**——建表正是本包做的，
  它在本包里从"没建"变成"建了并随本包这一笔入库"，旧路径那份 6 行的表早随 `0f5b168f 移除游戏项目` 删掉了；
  写这几行文档时它还只是工作树上的 untracked 文件（`git cat-file -e HEAD:examples/game_jhyt/licenses/ASSET_REGISTER.tsv` 不成立），
  所以各处统一写"**已建，随本包入库**"而不是"已入库"。）
- **本包是纯文档包，零源码改动**：动的是 `examples/game_jhyt/docs/` 的 51/53/54/57/58/59/60/49 八份 +
  仓库根 `docs/16-scene-format.md`，外加 `docs/33` §0 的 3.0 条目、`docs/70` §17.4、`docs/71` §8-B
  三处快照更正，和新建的 `examples/game_jhyt/licenses/ASSET_REGISTER.tsv`。`verify.ps1`、`scripts/**`、`engine/**` 一字未动。
- **回写的规则**：每处数字后面挂**复现命令 + 测量口径**，方案段落写"这条路买不到的部分"；
  历史条目不覆盖，改成"旧说法 → 新实测 → 证据"三行式（`docs/51` §4、`docs/57` §10、`docs/58` §15、
  `docs/33` §0 的 3.0 条目都是这个格式）。判据是"照着文档跑一遍能不能得到文档写的数"，
  不是"文档看起来自洽"。
- **口径分歧逐条裁定，不留开放偏离**：帧数/方向**以实现为准**（主角 7 动作 × 8 向 = 56、NPC 2/4/2，
  复现是读 PNG IHDR 宽高比 `strip_width ÷ frame_height`，命令写在 `docs/57` §3.2）；
  音频容器以实现为准（**MP3 / 22.05 kHz**，不是规范里写的 `.ogg`——那批 `.ogg` 已被 `1cd624b8` 删掉），
  但 BGM 盘上 30.07/30.09 s ≠ `gen_audio.py` 声明的 14/16 s ⇒ **登记为链路未闭合**而不是改规范数字；
  经脉的打坐/读经/顿悟/药膳与"时机判定小游戏"**以实现为准登记为未实现**（冲穴实为 LCG 掷骰）。
- **两处推断至今仍是推断，没有升格成结论**：前景遮断的 `max_cover_ratio = 0.65`（实测最差 0.604）
  与投影 `shadow_reach = 0.12`——它们是"门里写死的判据值"，不是从星露谷量来的规则；
  `docs/53` §4.1 那两处（1280×1040 多出的 16 px、遮挡时被遮物是否加深阴影）照旧标"待实机"。
- **授权表落点故意偏离规范文本**：放 `examples/game_jhyt/licenses/` 而不是 `docs/58` 里写的
  `assets/licenses/`。原因是 `jhyt_m29_season_weather` ⑦ 会重扫 `assets/**` 逐文件核对 48 号决定表，
  登记表一旦落进 `assets/` 就是一条无人认领的文件 ⇒ 门直接压红；`is_dev_artifact`（该例 :726）
  只放行 `manifest.json` / `*preview*.png` / `_` 前缀三样。打包器是白名单式
  （`assetc/src/lib.rs:709-728`）所以 `licenses/` 本就不进发布包，但**清点门不看白名单**。
- **复现这一轮的读数以"换根"为前提**：工作树压着别的包的在制品（`project.toml` 被临时改成
  Noto + `text_render_mode="vector"`，这会让像素字体门报红），所以全部读数取自
  `git archive HEAD examples/game_jhyt | tar -x -C <tmp>` 的副本，**且必须把仓库级 `plugins/webgl/`
  一起放进那个根**——`jhyt_m72_tile_variant_determinism` 与 `jhyt_m72_autotile_mask` 要读 web 宿主的
  JS 镜像，只解工程子树会得到"os error 3"，那不是引擎回归。逐门读数与归属包登记在 `docs/60` §8。
- **回滚档**：单笔 revert 即可，全是 `.md` / `.tsv`；没有需要成对回退的编码或数据。
- **已知限制（写给 M72-10/11/12）**：本包自己没有做目验——复跑门时只做到"原生 headless 门 + web 宿主
  JS 镜像**源文件可读**"，那不是宿主目验。目验侧的真实现状归 M72-12 的前哨轮（入库 `6f0e913b`，读数在
  `tasks/plan/M72-12-三宿主目验与试玩反馈.md` 的「实现记录」）：**桌面 + 编辑器两宿主已出读数（金标 `manor`）**，
  Web / 微信两宿主**本轮没跑**（不是"从未跑过"：M72-06 在微信**假宿主**上对拍过绘制序列，但那用的是 M72-07/08 之前的旧包、只算解算机制层，口径分层见 `docs/60` §8 第 1 条）；绝对帧率受同机并发编译/打包污染只有形状读数，内部试玩未组织；
  `qa_full.ps1` 的全量计时从未完整执行（清单基数也已从 58 涨到含 M72 十门）；
  动物 6 种与宣媒 13 件仍属外部美术委托；字体子集那两个分母（`docs/49` §4 的 1,118 / 4,641 / 1,612）
  之间的 ⊆ 关系是**设计意图**，没有任何一条门在核对它。因此本包**不归档任务文件**、
  不把 `docs/59` 的任何一行整体升绿——状态行是拆开的（"#5 内部一半闭合"、"#9 待内部 + 待外部"）。

## Unreleased — 29 种作物各有各的长相：122 张阶段图 + 一张 8×29 的 CropGrid，农田三态拆成"土 + 湿痕 × tint"（M72-08）

- **症状**：`docs/57` §4 自评的最大视觉缺口。29 种作物共用 `manor_farm.png`（192×32）里的
  SPROUT/GROWING/RIPE 三列，**灵芝和稻谷在田里长得一模一样**，而且只有 3 档；
  `assets/art/crops/` 整个目录不存在（`58` 记 0/29）。所谓"农田三态"实际是两张全幅土面加一块
  306 B 的近纯色湿润块——浇水这件事在画面上读不出来。
- **档数按作物类分，不是一刀切**（`docs/33` 生长节奏）：田粮 4 档 ×13、灵药 5 档 ×11、果树 3 档
  ×5 = **122 张**，全部由 `tools/crop_kit.py` + `tools/gen_crop_art.py` 从 M72-04 那份色阶表出图，
  不手画。颜色只借一层：`terrain_ramp.json` 新增 `crops.families` **16 个"作物身份色"族**，
  只覆盖果实/穗/花；叶子仍走 `families.grass/hedge`、木质茎走 `utility.wood`——区分靠形态，
  不靠 hue。
- **"别换个色冒充另一种"落成两条互不相干的判据**：① `crop_kit.signature()` 取 21 个形态键
  （archetype/stages/height/stems/leaf/head/canopy/crown/spacing/accent + 11 个开关，**不含任何
  颜色**）29 行两两不全等；② 比的是**图集里量化之后的条目字节哈希**——两张画法不同的源图被
  色阶表吸附成同一格才是真事故，判未量化的单体图会漏掉这一类。门 `jhyt_m72_crop_stages` §①
  是判据②的第二份实现（自己解 PNG，不 import Python），反面对照把 `rice.stage_2` 配成
  `lingzhi.stage_2` 必须报红——实测红 1 条并点名"字节完全相同（换个色也没用）"。
- **渲染载体新开一层，而不是把 `manor_farm` 加宽**：CropGrid（entity 31，`8 列 × 29 行 = 232` 槽，
  每行 5 个阶段槽 + 3 帧收成抖动槽），瓦值 = 图集条目号（1 基），号只在生成的
  `scripts/data/crop_art.lua` 里算一处。FarmGrid 退成两列（1=犁地 2=干涸，192×32 → 64×32），
  湿润**不是第三列**而是 FarmMoist 的半透明叠痕 × M72-02 的 per-tile 顶点色 tint 三档
  （越浇越淡，撤痕必须同格撤色，否则下一轮串色）。u8 的瓦值顺手给这张表封了顶：8 列最多
  **31 行**，第 32 种作物必须先减列或另起一张表。
- **只有收成做动画**（`docs/51` §2.3 明说生长阶段切换保持静态）：4 帧 @100ms 抖动 + `harvest_sparkle`
  （`effects.lua` 第 9 条带）。门 §④ 逐帧采样 CropGrid：写入帧号 `[7,13,19,25]`（间隔 7/6/6/6，
  判据 5–7 帧 ≈ 100ms @60fps）、瓦值 `[14,15,16,0]`、第 12 帧闪光在场、播完落回**已垦空地**
  而不是生土、净收 2 == `crops.lua` 的 `yield`。
- **数值零改动是有条件的**：`crops.lua` 的 diff 只有 **29 行 `stages`**（24 种 3→4/5、5 果树 1→3），
  `growth_days`/`yield`/`season`/`price` 一行未动，`stages` 也不参与任何数值计算；
  `jhyt_m24_numeric_alignment`（crops=24 / rice_yield=2）、`jhyt_m36_regrow`（薄荷首茬 2、二茬 2，
  地块 `mint|0|1`→`mint|0|2`）、`jhyt_m19_slice`（money=521）改前改后读数一致。代价是 `stages`
  从此与图集行槽位同源对拍（门 §③ 拿门内手抄表 / `crop_art.lua` / `crops.lua` 三方比），
  改了图必须改它。
- **中途被并发写回旧版，于是给三态门补了贴图侧那一半**：`gen_manor_scene.py` 的副本一度回到
  "FarmGrid 6 列、没有 CropGrid"，`manor_farm.png` 同步飘回 192×32 那张旧表——而运行期读到的
  瓦值照样对，门当时**全绿**。两件事一起修：① 作物层的取号与列数只写在生成器里（在装饰之前
  取 id 31，与 M72-07 的 `set_scatter_decor.next_id` 不再互相顶位）；② 门的 §② 分两半，贴图侧
  校"场景 `atlas_cols` == `farm_soil.lua` 的列数 == 盘上 PNG 宽 ÷ 32"、两列土面**字节不同**
  （把干涸配成犁地必须红）、湿痕那一列 1024 个像素全有内容而满不透明只有 5 个
  （≥10% 即判"这等于第三张全幅土面，会把垄沟抹平"）。
- **回滚档**：单笔 revert。纯新增物是 `assets/art/crops/**`（29 个目录 122 张 + 图集 19,014 B +
  两张预览）、`tools/crop_kit.py`/`gen_crop_art.py`/`gen_farm_soil.py`、
  `scripts/data/crop_art.lua`/`farm_soil.lua` 与门 `jhyt_m72_crop_stages.rs`；改动面是
  `scripts/data/crops.lua`（29 行 `stages`）、`scripts/systems/farm.lua`（三层绘制 + 收成定步长）、
  `scenes/manor.scene`（CropGrid + 两列土面 + 装饰段顺移一位）、`gen_manor_scene.py`、
  `terrain_ramp.json`（`crops` 段）、`effects.lua`（第 9 条带）、`verify.ps1` 与 `58` 台账 §3/§14。
- **已知限制（写给 M72-11/13）**：枯萎态 `stage_wither` 与品质角标未接（按开工时"核心 + 果树 3 态"
  的范围裁定登记，接的时候优先占行内那 23 个空槽而不是加行）；果树的"成树但未结果"没有独立槽，
  靠 `farm.lua` 的 `tree_not_fruiting` 逻辑闸拦收获，画面仍读成挂着果的树；32px 画框里的成树比
  `assets/art/buildings/` 的树屋件小一圈，发行级大果树属外部委托。

## Unreleased — 常驻 HUD 不再是"半透明色块 + 一条真图"：9 个组件换九宫格切图，遮挡口径跟着换成"色与图相乘"（M72-09）

- **症状**：弹窗用的是达标切图（`common/ui_panel_main_9s.png` 96×96，木框 + 四角金括号），
  而常驻 HUD 的**面板底、金色收口轨、六枚行分类色标全是 `__white__` 染色矩形**——实测 10 块
  （`grep -c "^component.*texture=__white__" assets/prefabs/hud.prefab`；不加 `^component` 会把
  头部注释那一行也算进去）。两套质感并排就是"游戏贴了张引擎调试面板"。素材不缺，缺的是接线。
- **换成的载体**：面板底与血/蓝条底 → `ui_panel_main_9s` + `nine_slice=16,16,16,16`（面板底那条
  **0.620 的半透明染色按裁定原样保留**）；六枚分类色标 → 新生成的 `hud/ui_hud_chip_9s.png`
  （8×8，边距 **2** 而不是 common 那批的 6：目标只有 6px 宽，6+6>6 会整块塌成边框、中心染色
  一格不剩）；金色收口轨 → 新生成的 `hud/ui_hud_accent_9s.png`（16×3，边距 4,4,1,1，行方向 1:1
  所以 3px 高不被竖向拉伸糊掉，金值取自 docs/35 §3 色板，与 panel_main 的金角码同源）。
  `__white__` **10 → 1**：唯一保留的是全屏雨罩 `HudRain`——它是一层 a=0.22 的整屏染色遮罩，
  不是 UI 面板，配任何带边框的切图都会往屏幕四周画出一条不该存在的 16px 木框。
- **定下的口径：贴图与染色相乘才算不透明度**。遮挡账（门 ③）以前只看"贴图中心 alpha=255"，
  换真图之后这条会**假绿**——一张中心不透明的木框图，染上 0.62 就盖不住任何人。于是
  `Reading::opaque()` 改成"贴图在 `OPAQUE_TEXTURES` 里 **且** Image 染色 alpha ≥ 0.99"。
  落地的可核对结果：换贴图前后**"被盖住"都是 3 块**，一条没多一条没少，证明这轮换皮没有偷偷
  改变遮挡事实。同时门 ① 的 Panel 读数补了 `texture,nine_slice` 两列——不补，"画面真的换了
  贴图"在黄金表里是**不可见的**（这次重刷 138 行里有 8 行就是因为这两列才出现的差异）。
- **本包唯一一处行序改动，以及它为什么必须另立一档实测**：`entity 2`/`entity 3` 槽位对调 ⇒
  金色收口轨画在面板底**上面**（迁移前是被半透明面板底糊住的）。黄金表**量不到这件事**——
  ① 是排序集合比对、③ 是"整块盖住"谓词，而 300×180 与 300×3 谁也不整块包住谁。所以门里加了
  一档走真实 `mesh.draws` 的**导轨档**（面板底段必须提交在金轨段之前，两段缺一即报错）。
  它的价值由反面对照 B 钉死：**把槽位换回原序，黄金表整表绿、导轨档红**。
  另一条反面对照 A（把 frame 挪到 fill 之前 ⇒ ③ 遮挡账红）守的是老的那条层序。
- **新增一件工具，因为它补的是"门看不见的那半边"**：`tools/preview_prefab.py`——无 GPU 的预设
  合成器，按槽位升序 source-over 合成整张 prefab，并逐块打印**九宫格边距契约读数**
  （chip 6×16 目标 → 拉伸芯 2×12、accent 300×3 → 292×1、面板底 300×180 → 268×148）。
  它只回答"预设里写了什么、按边距会拉成什么样"，不回答"引擎真的画成什么样"，所以与门是两条
  独立证据；本包的 HUD 目验靠这两条交叉。
- **回滚档**：`git revert` 本提交即可；两张新切图与 `preview_prefab.py` 是纯新增物，黄金表
  回到重刷前的 138 行、`hud.prefab` 回到 10 块 `__white__`。几何一行没为门绿而动过。
- **已知限制（写给 M72-10/12/13）**：① 三宿主只目验了桌面——九宫格切分只有一份实现
  （`rustlua-ui/src/ui.rs` 的 `nine_slice_cells`），三宿主共用，但**采样过滤/纹理上传那层的
  差异没看过一眼**，而那恰是九宫格最容易翻车的地方；② 边距契约仍归 M72-10，本包留给它的输入是
  "6px 宽的目标矩形装不下任何 6px 边距的九宫格"；③ `jhyt_m29_season_weather` 的三条遗留红
  （④ 雨罩 `hud:rain` 读不到、⑤ `last_action` 实测 `sleep`、⑦ 资源审计三基准数 984/47/2 对不上
  实测 1048/51/128）**一条没动，只登记**——本包取那次实测数用的是临时早退，取完已把该文件完整
  还原。逐条读数与复现命令在 `tasks/plan/M72-09-HUD换九宫格切图.md`。

## Unreleased — 建筑不再"撞空气"：占地按不透明像素重算、南门挖出门洞、水池改成不规则岸线（M72-07）

- **症状**：`solid` 串是按 sprite 的 **quad 矩形**标的，而建筑贴图是 224×224 的一大张、quad 四角
  几乎总是全透明（屋顶是梯形、檐口是斜的）。玩家因此在屋外两步远的地方撞一堵看不见的墙；另一边
  药庐整个穿模（视觉 16 格、碰撞 0 格）。更糟的是**生成器与盘上分叉**：药庐与地基从 x33 挪到 x16，
  `solid` 留在原位 ⇒ 21 格悬空碰撞。这条是 M72-04 交给 M72-05、M72-05 没接的账。
- **定下的口径（`tools/building_kit.py`，P1 金标规范）**：每一格实心都必须有可视依据——那一格的
  地表本身就是墙/水（瓦画的），或有一个 sprite 的**不透明像素**压在上面。建筑 = 本体∪同名拆层屋顶
  − 门洞，树/竹/单件道具 = 单格根碰撞，装饰不参与碰撞。屋顶并进并集的理由是层秩
  `world(−1) < actor(+1)`：墙永远遮不住身后的人，那些格不实心就等于让人走进墙里。
- **拆层不许动摇玩法**：并集口径白送一条可断言的性质——"整图算的格集 == 本体∪屋顶"逐格相同
  （门 §②）。M72-06 只拆了正房，本包把同一条 `LAYER_SPLITS` 推广到柴房与五座可建造建筑
  （12 张新 body/roof，`assets/art/buildings/` 29→41 张），并给建筑侧补上左上光源
  （屋顶左亮右暗一条梯度 + 屋檐在墙顶压出暗带）。
- **门洞是数据不是感觉**：南门 `(16,1)(16,2)` 是山庄↔云泽村的出口格，声明为洞就带三条反查——
  洞格必须落在自己的视觉框内（改了位置而表没跟着改就红）、盘上那一格必须真的空着、贴图在
  "人站着穿过的那一格"实测不透明占比 ≤ 0.35（重画前 0.94：匾额正好挂在通道上；重画后 0.16），
  并且从出生点按 4 邻域 BFS 走得到。
- **水池从矩形改成岸线**：`POND_ROWS` 逐行给宽度 ⇒ 27 格水 / 外接框 40 格（填充率 0.675，
  旧版是 30/30=1.000 的蓝方盒），水格的 4 邻域里仍是草地的刷成石板做石砌岸；同时删掉
  "给建筑与水池整框刷 DIRT"的写法——DIRT 是 `farm.lua` 的可开垦判据，那等于在房子底下留一片能种的地。
- **改地面会挤红别的门，补的是概率不是门槛**：石砌岸与石板晒场把草地换成石板，每屏装饰滑窗中位数
  从 43 掉到 36（门槛 40）。上调的是 `decor_kit.BASE_P`（草 0.70→0.75、石板 0.40→0.55、水 0.55→0.85），
  理由写在工具注释里；睡莲 `decor_lily` 是目录里唯一带 `WATER` 权重的形状，所以"装饰可以落在水面"
  这一格只让它进水。改后 204 件 / 13 种形状全出现 / 中位数 41。
- **门 `jhyt_m72_building_footprint` 五段**，是 `building_kit` 的**第二份独立实现**（不 import Python，
  自己解 `.scene` 文本与每张 PNG 的 alpha）：① 逐格复算 vs 盘上 `solid`（187 格天然 + 183 格像素依据
  ⇒ 359 == 359，缺 0 悬空 0）、② 拆层不变量、③ 门洞三条反查 + BFS、④ 每栋有且仅有一件投影贴片
  （平地基的 `BuildFoundation` 必须没有）、⑤ 运行期 `tilemap.is_solid` 位图逐格 == 盘上串。
  反面对照三条且都走同一个函数：竹子按 quad 框标 ⇒ 悬空 5 格、摘掉拆层屋顶 ⇒ 缺碰撞 64 格、
  把洞填实 ⇒ 出口当场断。**"按框"那条为什么落在道具而不是建筑**：盘上 10 栋建筑的 quad 框与不透明
  紧包围盒逐格相同（贴图都铺满了画框），拿建筑做这条对照是空转——读数登记，不删对照、不降判据。
- **顺带修掉三条会咬后来的包的东西**：① M72-08 的 `CropGrid` 追加在装饰之后（id 235），把就地工具
  `set_scatter_decor.next_id` 从 31 顶到 236，重跑生成器后 816 行装饰段整体改号——改成作物层在装饰
  **之前**取号（id 31，仍追加在文件最后，绘制序两条约束都不破）；② `building_kit.opening_holes` 与
  门侧的"格→图像行"映射都写成了 `h−wy0−TILE−y`，正确是 `y+h−wy0−TILE`，只在南门 `y==0` 时碰巧对，
  两侧同法改正；③ 微信 harness 的 `OCCLUDER_TEXTURE` 认遮断物只写了两张图名，遮断物从 5 件涨到
  11 件后 min/max 只量到一部分——改成按后缀（`_roof`/`_canopy`）认。
- **P1 金标收口**：四条硬口径回写 `57` §1.3 第 10 款 + §2.3 第 6 条、`51` §2.1 两行，读数与清单
  入账 `58` §13，`44` 新增 §4"山庄 = P1 金标场景"。**M72-11（把规范铺到其余 7 张场景）到此解锁**。
- **回滚档**：单笔 revert。数据侧动的是 `scenes/manor.scene`（`solid` 串 + 装饰段 + 拆层件 + 门洞）、
  `assets/art/buildings/**`（新增 12 张分块、重画 5 张整图与南门）、`assets/art/decor/decor_lily*`、
  `assets/art/shadows/blob_95x27.png`（替下 `blob_91x25.png`）；代码侧是 `tools/building_kit.py`（新）、
  `gen_manor_scene.py`、`gen_manor_art.py`、`decor_kit.py` 与门 `jhyt_m72_building_footprint.rs`。
  读数随数据漂移而就地更正的三处注释（`lighting.lua` 的交集、`depth.lua` 的件数、
  `verify.ps1` 的 360/560）一起回。

## Unreleased — 前景终于压得住人、一批图只剩一个光向：5 件遮断物、16 件投影贴片与一份 lighting.lua（M72-06）

- **症状**：`51` §2.1 早就要求"前景能压住玩家、所有物体有方向一致的投影"，**至今一条都没实现**——
  `preview_scene.py` 的 `LAYER_RANK` 有 `foreground` 这一档，但八张场景里一件实体用它：机制在，
  内容零。画面因此读起来是"一张贴纸铺在另一张贴纸上"，人走过树冠前面没有任何东西盖住他。
- **光向只有一份事实源**：新增 `scripts/data/lighting.lua`（`sun`、`shadow_reach`、`shadow_tint`、
  `cast_shadow`、`actor_shadow`、`occlusion`、`foreground_order`）与只按文本解析它的
  `tools/light_kit.py`，生成器侧和验收门侧从同一份公式取偏移向量。**文件读不到就抛
  `FileNotFoundError`，不静默回退**——回退等于每个生成器回到自己那套偏移，正是本包要消灭的状态。
- **一句脚本报错句也要重裁字体**：`collect_project_chars` 收的是**全部 Lua 字符串字面量**，不分
  界面文案与 `err("…")`，所以新增的 `lighting.lua` 让工程用字从 1,605 涨到 1,612，
  `jhyt_m72_pixel_font` ④ 当场红 6 个字（又/溢/特/秩/零/颜，逐字归因全在这个新文件里）。按
  `docs/49/58` 的既有口径处置=重裁子集（`subset_project_font`），没有改文案去迁就资产：
  子集 **597,932 → 599,264 B**、`cmap` **1,606 → 1,612**，并逐码位对拍确认**只增不减**
  （新增集合恰好等于门点名的那 6 个、删除集为空）——重裁是不可逆操作，这一步不核对就等于把
  "有没有别人在用的字被裁掉"留给下一个跑门的人。
- **一个几何写法把"单一光向"从近似变成恒等式**：贴片尺寸写成 `2r+1`（恒奇数），贴片中心就落在
  一个整数 texel 上；`sun=(1,-1)` 时 |dx| 与 |dy| 连取整都等量 ⇒ 中心相对接触点必然是 `(h,-h)`，
  角度**恰好**等于光向角。门的读数是"16 件、最大偏离 0.000000°"，而不是"方差 ≤ 某个经验阈值"。
  附带一条口径要说清：方差与"落点复算"是同一事实的两种单位，写反的阴影先红在落点那条
  （容差 0.001），所以方差这条不给阈值、钉 0。
- **0.40 的判据守不住时改的是美术，不是数字**：树冠切顶 57 行时最深覆盖 **0.861**（人被吞一半）。
  **没有放宽上界**（那等于把守卫改成迎合美术），而是把切点改到 40 行——露腿而头在树后，最深降到
  0.604 ⇒ 上界取 0.65。反例一并记下：切 26 行最深处只有 0.357，**数字上过得了 0.40，画面却是
  "人站在树前面"的错读**，那正是"靠放宽数字装没事"会奖励的结果。要真守住 0.40 得把树干从 88
  加长到 116 并整片树冠进前景（改一张 PNG + 三张生成器表 + 重存两张引用整图的场景），代价与
  收益不成比例，登记为限制。**整张 `manor_tree.png` 一个字节没动**，只换切点。
- **`solid` 是物理的**（推翻本包前半的措辞）：原先写"本项目没把 solid 做成碰撞体"，凭的是只 grep
  场景文件；`scene_spawn.rs` 的 `solid_cell_colliders` 会给**每个实心格**在 tilemap 的静态刚体上挂
  一个 cuboid。40×23 = 920 格逐格发一发自 r=4 的 `physics.overlap`，把它量成读数：360 个实心格各
  恰好命中 1 个 GroundMap、560 个空格 0 命中 ⇒ 遮挡门说的"可达"就是物理上站得住的地方，那条
  teleport 落位断言从此有了牙。
- **接触阴影 ≠ 投影，门两码**：脚下椭圆在任何光向下同形、本身不携带光源信息，所以光向门**只采
  投影贴片**的样本，对脚下阴影只断言"不往左上跑"。角色继续把椭圆烘在帧里：贴片要么逐帧配偏移
  （多一份会漂移的数据），要么固定偏移（走起来脱脚）；烘在帧里天然跟脚、三宿主同幅生效、零运行期
  开销。建筑/树/道具实测**一个半透明像素都没有** ⇒ 投影必须是新贴片，而贴片要保持 alpha 只有
  0/255（软边契约），暗与透全交给 `tint` 的 rgba ⇒ "影子多暗"在盘上只有 lighting 一处。
- **修掉一条镜像顺序的真 bug**：脚下阴影原先烘进帧里再整帧 `mirrored()`，`shift_x=2` 会在 w/sw/nw
  三个朝向上翻成"往左上投影"。改成整帧镜像之后再 `blit_behind`（只写透明像素 ⇒ 脚永远压住阴影）。
  门 §⑦ 两侧都读：32 帧（8 朝向 × 4 帧）质心全在身体中心右侧 38.28..38.67，旧写法那三族会读成
  25.33。代价是 `assets/art/characters/**` 全量重烘。
- **"被遮地面压暗"这条通道被实测撤销**（计划原文走 M72-02 的 per-tile tint）：Tilemap 恒在一切
  sprite **之前**画 ⇒ 被盖住那格只在遮断物自己的透明缝隙里露得出来，压暗会从零散缝隙漏成花斑；
  交集也量了——5 件遮断物矩形覆盖的 44 格地面 ∩ 16 张贴片的椭圆覆盖 = **0 格**，而那 44 格全在
  本体贴图之下。两条通道管的是不相交的两块地，不存在二选一，也不存在叠两层墨。
- **跨宿主钉的是"层带生效"，不是"顺序碰巧对"**：微信宿主读假 ctx 记下的 `drawImage` 序列
  （`--check-foreground-order`），三段各钉一件事——探针的层秩先后、盘上真实数据（16 张贴片的 max
  < 主角 < 5 件遮断物的 min，两带零交叠）、**只改层名不改 order** 的反面对照（遮断物从 1146 掉到
  920 ⇒ 排除"其实是 order 在起作用、层带是死数据"那种假绿）。`depth_order` 同帧仍绿。
- **顺带修掉一条会命中所有"产物放非系统盘"打包者的 harness bug**：产物在 E:、临时目录在 C: 时
  `path.relative` 不返回 `..` 前缀而是**直接返回绝对路径**，于是 `wx.downloadFile` 落进系统临时
  目录的字节被当成"产物内的代码包路径"，后缀不在上传白名单 ⇒ `permission denied` ⇒ 适配层报
  `pack/project.toml HTTP 404`，而本地服务给的是 200（preload 钩 `http.get` 量的）。判别实验：
  TEMP 与产物同盘必绿、异盘必红，与产物内容无关。
- **就地更正两条死引用**：`57` §9 那条"独立基线 `scripts/check_asset_baseline.ps1`"引用的脚本
  **从未入库**（`git log --all --` 空 ⇒ 不是被删，是从来没提交过），连带 docs/33/58/60 里
  "OK 112 / MISSING 107"那句是复现不出来的读数，已划掉并给出可复现替代；`57` §1.3 新增第 8 款
  （投影门）里记一条量出来的限制——城门那片影子 16/25 行在图外（本体贴地图下边界 ⇒ 贴片必然出图，
  相机纵向只剩 736−720=16 单位可动），其余 15 片全在图内，登记不修。
- **门 `jhyt_m72_foreground_occlusion` 八段**（光向事实源自检 + 四条取值对照、拆层结构契约、
  可达覆盖率、层带反面对照、投影落点与角度、脚下椭圆 32 帧、Y 容差两侧），已登记进 `verify.ps1`
  MVP 清单。
- **77 条 headless 门逐道跑完并按名归因**：共享工作树里不能靠 stash/reset 做对照，用的办法是换根
  （`RUSTLUA_GAMES_DIR` 指向一份临时副本 = 工作树全部数据 + 只把 `examples/game_jhyt/project.toml`
  换成 `git show HEAD:` 那份）。工作树原样 47 绿 / 30 红，仅换那一个文件就 67 绿 / 10 红——23 条
  翻绿全因那份在制品（M20-08 手柄验收，文件里写着"示例结束时按原样恢复"而尚未恢复：删了
  `release = true`、字体换成 Noto + `text_render_mode=vector`、输入表的键盘绑定全换成 `pad:`）。
  剩下 10 条红里：2 条是**控制根自己的缺陷**（那两段要读 `<root>/plugins/webgl/web-runtime.js`，
  补上 `plugins/` 即绿）、1 条红在 gitignore 的打包产物（`builds/pc/pack/project.toml` 没有
  `release = true`，归 M72-03 §未完成项 5）、5 条纯 HEAD 数据同样红、2 条纯 HEAD 绿而工作树红
  （点名的是他包的 `ui/input.lua` practice 分支与 `docs/33` §10 状态效果表）。**唯一一条属于本包
  的红**是上面那条字体子集缺字，已按 `docs/49` 的口径重裁修掉。
  受绘制序影响的三条黄金表门逐道读过：m35 与 m65（它会 `scene.load('manor')`）绿；m63 在纯 HEAD
  数据与控制根下都红在"分段数 期望 15、实测 16"，多出的那段是 `__font_atlas__`——**是 M72-03 换
  字体时漏刷了那张层序表**（已登记为它的未完成项 8），而工作树里那条绿是在制品的 `vector` 模式
  把它盖住。**没有**为了绿灯刷别人的黄金表。
- **已知限制**：只有 manor 接了全局光（其余七张按 M72-07/11 重做时接同一张 `LAYER_SPLITS` 表；
  门读场景文件，换场景不改门；不是工具限制——`set_cast_shadows.py` 按目录工作）；0.65 这一档仍
  允许"头肩全没、只剩腿"的瞬间，正确解法是被完全遮断时让前景淡出（要逐帧覆盖率的运行期读数），
  登记为后续项而不是再放宽上界；主包体积红在 17,773,244 字节的 Noto 被 `first-screen-pack.js`
  扫进主包，归属裁定在 M72-03、执行在 M72-13，本包用 `-AssetPack remote` 绕开跑跨宿主读数。
- **回滚档**：单笔 revert。数据侧只在 `manor.scene` **追加** 5 件遮断物（保留 id 段 5001..5005）与
  16 件投影（5101..5116）+ 9 张纯白贴片 PNG，地表层、玩法承重与碰撞一个字节没改；
  `assets/art/characters/**` 与 `manifest.json` 是 C-1 的全量重烘，revert 即回到旧条带；
  重裁的字体子集（`assets/fonts/FusionPixel-12px-Monospaced-zhHans.ttf`）与记它字节/SHA 的两处
  台账（同目录 `LICENSE-FusionPixel.txt`、`docs/58` §11）是同一次改动的三份记录，revert 一起回。

## Unreleased — 地面终于有"东西"：碎屑库 24 张、确定性散布与一条不参与物理的装饰层（M72-05）

- **症状**：M72-04 把地表做厚了，`manor` 还是像沙盘照片——一屏（640×360、zoom 2）里数得出
  **对象**的地方只有约 2 处。观感差距的头号来源不是贴图质量，是**画面里没有东西**。
- **先量性能再画**：本包第一件事不是美术，是"300+ 静态 sprite 烘进 `.scene` 到底付多少"。
  读数 A（30 实体基线）load p50 168µs；裁定"写进 `.scene`"附带一条硬条件——加完装饰必须**复测**，
  不破 3ms 才算成立。读数 B（205 实体）：**p50 778µs / p99 1.3ms**，判据未破 ⇒ 不做运行期生成、
  也不借 tilemap 通道当装饰载体（那两条各自要付"每帧重算"或"改地表语义"的价）。
- **库与散布各只留一个事实源**：`tools/decor_kit.py` 一份实现喂三个入口（写进 `.scene`、
  `--check` 对拍、预览），12 种形状 × 2 面镜像 = 24 张 PNG。**镜像必须是独立文件**：引擎
  `Sprite::to_component_str` 不发 `flip_x`，两个 web 宿主也不支持它，靠组件字段翻面的话，
  web 上会悄悄变成"所有灌木都朝同一个方向"。取色一律走 M72-04 的 `terrain_ramp`，库自己的门
  （`gen_decor_art.py --check`）钉条目齐全、尺寸 == 目录、每个不透明色都在色族内、alpha 只有 0/255。
- **确定性口径**：逐格独立哈希（不是一条顺序流——那样改一格会重排全图，"同一输入同一张图"
  就只是巧合）；疏密按 2×2 块走（`PATCH_BANDS`）而不是逐格，才读得出"这一片草密、那一片是土路"；
  格内子偏移锁在本格范围内，同带 sprite 互不重叠；临水出香蒲、边界出草（`EDGE_BONUS=12`）。
- **门 `jhyt_m72_scatter_decor` 五段，其中两条判据口径值得单独记**：
  ① "每屏 ≥40" 取的是**所有可停靠窗口的中位数**（median 43 / min 5 / max 71）。min 不能当判据：
  最坏窗口 (448,88) 是药田，它**必须**留空，撒满反而是错的。
  ② "装饰不参与物理"的探针坐标必须从**运行期**取（`entity.find` → `transform.get`）：Rapier 刚体
  建在 `Transform.position` 上，而文件里的贴图中心还得再过一遍 pivot 与 y 翻转。第一版拿后者去问
  碰撞体，正向对照当场失手——那种写法下装饰真长出 collider 也照样绿。最终读数：175 件全部查得到、
  命中装饰贴图 0、命中别物 16（地表实心格），正向对照打到 `JianghuPlayer` 取到 1 个且含其自身；
  载体两侧对账为整层 175==175（剔除关）、默认相机一屏 31==31（剔除开）。目录改 1px 与装饰进
  `actor` 带两处反面对照均按预期红。
- **路上撞掉三个真 bug**：① `preview_scene.py` 不认 `position=x,y,z` 这种组件串（M72-04 之后
  预览与实机**静默**不符）；② `gen_manor_scene.py` 的留白契约输入 `places.lua` 按 **cwd** 定位，
  从 `engine/` 下跑 `--check` 时"文件读不到"被宽容成"0 个地点"继续跑，盘上 24 行对不上却看不出
  为什么；③ `gen_decor_art.py` 的 `OUT_DIR` 同类，在 `tools/` 下跑会把一张没少的库报成"24 张全缺"。
  ②③ 改成按文件位置定位、读不到就抛 `FileNotFoundError`——宽容的默认路径不是鲁棒，是**换个目录
  就跑出不同结论**。同类隐患还剩 `gen_village_scene.py:327` 与 `gen_interior_scenes.py:265`。
- **顺手修了一条本里程碑的陈旧门**：`jhyt_m72_tileset_atlas` §⑤ 把 `atlas_rows=1` 的特例
  （6×1 整幅拉伸、V 上限恒 1.0）写成了通则，M72-04 换成 12×9 之后它一直红着——那轮回归跑的是
  fmt/clippy/test/doc/哈希 parity，没跑 76 条 headless 门，所以没人看见。改成按声明行数分带
  （跨度 `= 1/rows`、上下边落在 `rows` 等分线上），rows=1 时退化回 0..1，§④ 的旧编码语义一字未动。
- **全量回归**：76 条逐条跑（`-MvpOnly` 的"多余即失败"守卫在 pristine HEAD 上就会被挡住——
  `jhyt_m59_ui_prefab.rs` 已随 M59 提交进仓库却从未登记进清单，跑之前就 throw），67 绿 / 9 红。
  红里除上面那条陈旧门外全部**不属本包**；
  判别办法是把 `manor.scene` 临时换成 HEAD 版（155 行、一件装饰都没有）重跑三条最可疑的门
  （商店 UI、雨季 HUD、图集门），报错文本**逐字相同** ⇒ 与装饰在场无关。与本包判据直接相关的绿：
  `jhyt_m35_depth_sort`（绘制序契约没被动）与 `jhyt_m65_shop_prefab`（它会 `scene.load('manor')`，
  116 行黄金表没被 175 件装饰改动）。
- **已知限制**：中景档只有 16 件（920 格里 ~1.7%、一屏约 1 件），"这一片有什么可看"仍未解决——
  真正的解法是树与多格建筑，属 M72-07；前景压人与 Y 遮挡属 M72-06；M72-04 交下来的悬空碰撞格
  （`solid` 的 x33..36 × y14..20）本包没动，转交 M72-07。
- **回滚档**：单笔 revert。数据侧只往 `manor.scene` **追加** 175 件装饰实体（id 31..205），
  地表层、玩法承重与任何 `scripts/` 逻辑一个字节没改；图集门那处改动纯在断言侧。

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

## Unreleased — Web 运行时：时间步与 Rust 权威对齐、微信资源路由与缓存换版、生产诊断开销移出（M73-00）

- **症状（四条各自独立、都表现为"发布出去才发现"）**：
  ① Web 侧按 rAF 回调次数当步数，而 Rust 权威是 `FixedTimeStep::new(60, 8)`
  （`engine/crates/rustlua-infra/src/time_step.rs`）——掉帧时两边跑的不是同一段时间，
  位移与计时在长帧下对不上；
  ② 微信首选 WASM 通道失败、兜底通道**成功**时，`wasm-bridge.js` 无条件返回
  `{failed:[first,second]}`，把"兜底成功"判成启动失败；
  ③ 微信音频自己拼 `pack/...` 三种写法，而分包形态下首屏音频在构建期被**搬**进 `mainpack/`、
  远端形态下资源根本不在包里——两种形态都指向不存在的文件，表现是"画面正常但没声音"；
  ④ 远端缓存键只有逻辑路径且命中即返回（校验只在新下载时做），发新版本后旧缓存**永久**遮蔽
  新内容：改了 Lua/贴图，真机上还是老样子，且玩家不清缓存就永不自愈。
- **时间步**：Web 改为与 Rust 同一套规则（60 Hz 固定步、`elapsed` 累积、最多 8 步追赶、
  超出丢弃并计数、`visibilitychange` 后重置时钟）；每步注入输入并在 step 0 之后清边沿
  （新增 `__rustlua_finish_step_input`，对应 `InputSnapshot::finish_frame`），
  帧阶段用 `delta × steps` 作为 `time.delta()`。为此新增 wasm 导出 `lua_set_number`
  （`engine/crates/game_web/src/lua_bridge.rs`），逐帧写 `__rustlua_dt` 不再每次编译一个 Lua chunk。
  判决写进 `data-rustlua-timing`（steps/dropped/acc），掉步不再是隐形的。
- **微信资源路由**：新增适配层出口 `assetSrcFor`（主体路由 → 分包挂载 → 远端 URL），
  音频与贴图**共用同一套规则**而不是各拼一套；结果按音效名缓存，避免每次播报都探一次盘。
- **缓存换版**：缓存路径加**发布级**标识（清单文本指纹）目录，并在换版时清掉上一版目录；
  清单自身恒走不带标识路径（否则"先落盘清单再算标识"会把新清单写进旧键、标识永远停在上一版）。
  代价是远端形态每版多一次旧目录清理，收益是"换版必须清缓存"这条人工约定可以取消。
- **生产诊断开销**：全画布 `getImageData` 与 `willReadFrequently: true` 此前无条件生效
  （后者等于让主画布每帧走软件光栅化）。现由 `?rustlua-diagnostics=1` /
  `__RUSTLUA_DIAGNOSTICS` 显式开启，两个验收入口已显式打开，验收读数不变而发布包不再付这笔钱。
- **启动可观测**：微信看门狗原本在 wasm 兑现**之后**才 arm、实例化本身无超时——分包挂载或
  `WXWebAssembly.instantiate` 半死时是黑屏 + 0 error + 无任何提示。现覆盖整条启动链并报阶段
  （`data-rustlua-boot-phase`），实例化按独立超时（默认 15s）结算为失败而不是永久悬挂。
- **另外修掉一条只涨不落的保留链**：染色瓦片缓存的外层键是图集 `Image` 对象，而 `loadImage`
  每次新建 Image——每换一次场景就多一条永不再用的条目，各持有最多 512 张离屏画布。
  现加外层预算并在换场景时显式释放。
- **新增宿主 API 是四处改动，不是一处**（本轮由端到端跑出来才发现的坑，值得单独记）：
  `window.__RUSTLUA_LUA_BRIDGE` 不是 wasm-bindgen 模块本身，而是每个平台自己手写的**显式白名单**
  （webgl 在 `plugins/webgl/index.html` 的 import + 对象字面量，微信在 `wechat-adapter.js`）。
  只加 Rust 侧 `#[wasm_bindgen]` 导出与运行时调用、漏掉这两份名单，编译期一切正常，
  运行期表现为**启动即失败**且原始报错只有一句 `is not a function`。
  本轮补齐两处名单，并把 `lua_set_number` 加进两个 wasm 导出门的必需清单
  （`verify-wasm-host.mjs`、`verify-no-modules-bridge.mjs`，`bridge_exports` 7 → 8），
  以后同类漏项会在**打包前**被拦下。
- **验收**：`cargo +1.88.0 check -p game_web --release --target wasm32-unknown-unknown` 通过；
  真实浏览器端到端（`package.ps1`（发布口径，zig 到位）或 `… -SkipBuild` + `verify-webgl-bundle.ps1`）——
  **`webgl_bundle_ok`**（`scene_verify=done`、`drawn=5621`、`prefab_text=4/4`）、
  **`api_contract: ok cases=33`**（PC transcript 与活体页面逐行对拍）、
  真实页面上的固定步读数 `data-rustlua-timing="steps=1 dropped=0 acc=0.0096"`
  （约 900+ 帧零丢步、累积器稳态）。
  新增四条**不需要 wasm 产物**的回归门并接入 `scripts/verify.ps1`——
  `scripts/wechat-asset-routing-test.js`（6 例）、`scripts/web-fixed-step-test.js`（10 例：
  追帧上限 8/丢弃按整数步/10 分钟长跑不漂移/144Hz 与 30Hz 输入都得到 60 步/秒/长卡顿恢复/rAF 抖动序列）、
  `scripts/wasm-bridge-fallback-test.js`（4 例）与 `scripts/web-u64-hash-test.js`（7203 例，见下条）。
  **未验证**：微信小游戏端到端与真机全部项。
  `verify-wasm-lua.ps1` 已补跑并在**不带** `-SkipBuild` 时全绿（`wasm_lua_ok spike=pass bridge=pass`，
  42/42）——注意 `-SkipBuild` 会连胶水生成一起跳过，"新 wasm + 旧胶水"能跑出 11 个像真回归的失败。
  `-DriveJhyt` 仍卡在 `phase=nav-exit timeout frames=901`，与既有红读数逐字相同（非本次引入）。
- **补上 wasm 侧的跨边界成本基线**（新增 `scripts/wasm-lua/measure-lua-boundary-wasm.mjs`）：
  在 Node/V8 里加载**发布口径的真 wasm**（no-modules 胶水，与微信同形态），把原生那份
  `lua_boundary_cost` 的操作逐条量一遍（µs + Lua 堆字节，停用增量 GC 后量增长）。
  结论落进 WP3 的设计判据，逐条数字见 `tasks/plan/M73-00-Web运行时性能重构.md`：
  现行每帧输入协议的开销里**绝大多数是"编译 + 跨界编组"，建表本身只占一小部分**——
  也就是说批量 ABI 要消除的是"每帧编译一小段源码"，只优化建表几乎没有收益；
  而**固定跨界开销在 wasm 上占比更大**，所以批量 ABI 必须摊到"每帧一次调用"而不是每个字段一次。
  该脚本**故意不挂进 `verify.ps1`**：它是测量不是判据，计时类读数当门只会抖出假红。

- **顺带修掉两条"让微信平台打包不出来"的既有缺陷**（都是被彼此掩盖着、逐个暴露的）：
  ① `scripts/package.ps1` 的 no-modules 胶水守卫在**纯 try_table**（rustc 1.88）产物上误报：
  当前 wasm-bindgen 0.2.129 的胶水里 `call_guard`/`__instance_terminated`/`WebAssembly.Tag`
  一处都没有（与 2026-09-30 那次成功打包的产物形态一致），而守卫要求必须命中那段文本否则抛错
  ——**微信打包自 10-03 起一直是断的**。守卫改成三分支：命中就补、胶水确实不引用就跳过、
  引用了但写法不认识才响亮失败（保护意图不变）。
  ② 修完①才暴露：出厂 `web-runtime.js` 的 64 位瓦片变体哈希用了 **BigInt 字面量**（`1n`），
  而微信上传器的解析器只到 ES2019，官方报 `Identifier directly after number`——**整包上传会被拒**。
  改为 `[hi, lo]` 两个无符号 32 位的手写算术（乘法/右移/取模），热路径零分配且更快
  （同一台机 20000 次：19.4 ms → 46.7 ms 的旧实现，即约 2.4×）。正确性用**旧 BigInt 实现留作
  oracle** 对拍：新增 `scripts/web-u64-hash-test.js` 7203 例 + 20 万次乘法抽查，
  并与 JS↔Lua 一致性门（56 格金标、6/6 常数）同跑，`drawn=6171` 等渲染读数与改动前逐位相同。
  踩坑记录：16 位分段的进位**不能**用 `>>> 16`（`c1`/`c2` 超过 2^32 时 `>>>` 先 ToUint32 会丢进位，
  20 万次里 7% 错一个字，且前 5 步 FNV 恰好都对），必须 `Math.floor(x / 0x10000)`。
  **仍未收尾**：微信主包超限 `19,673 KB > 4,096 KB`（首屏 60 张预制体贴图约 19.7 MB 被算进主包，
  是 M72 美术进来后的资源范围问题）——出路是收窄首屏清单或用 `-AssetPack remote`，
  属内容/发布决策，本批未擅自改。
- **每帧输入协议换成批量二进制线格式（WP3 首片）**：新增 crate `rustlua-frame-abi` 定义 `RLFI`
  定长帧（帧号、两条 f64 轴、四张 f64 双项表、键位图 + 鼠标位图，小端、顺序即位序），
  `game_web` 新增 `input_frame_layout_json`（把布局与键表交给 JS——JS 因此**不手写任何偏移**，
  偏移写错一个字在 JS 里不报错，只会让某个字段读到另一个字段的值）与 `lua_input_frame`
  （解码 → 校验 → **原地改常驻表**，只动变化的位）。运行时删掉"每帧把快照拼成一段 Lua 源码
  再编译"的整条路径，改为复用同一块缓冲、每帧零分配打包、**每帧一次跨界**（固定跨界开销在
  wasm 上占比更大，按字段各调一次会比整条旧协议还贵）。
  差分语义与旧协议逐字符对齐：**按住集合消失写 `false`，边沿集合消失写 `nil`**——否则就是
  "松开方向键后角色还在走"或"点一次触发好几次"。位图写成 u32 小端字而**不用 `setBigUint64`**
  （那要 BigInt，即 ES2020，而微信上传器只到 ES2019，本批已因此被拒过一次整包）。
  缺导出时按 `lua_set_number` 同一口径**响亮失败**：产物换了一半属于发布故障，静默退回旧协议
  只会把它降级成"手感有点怪"。新增常驻读数 `data-rustlua-input`（`frames`/`bits`/`unmapped`）：
  `bits` 恒为 0 就是"宿主与 wasm 对哪些位该变理解不一致"的第一手线索，`unmapped` 列出落不进
  键表的键名——"某个键按了没反应"要能一眼分辨是"宿主没送到"还是"线格式不认这个名"。
- **换协议时跑出来的三个缺口（各自代表一类缺陷，值得记住）**：
  ① **预置输入表漏了 `press_cursor`**：旧协议每帧**整份替换**表，第一帧就把它补上了，所以这个
  缺口一直不会显形；改成原地改表后立刻报 `error converting Lua nil to table`。预置表补齐，
  应用器改成**报出缺的是哪张子表**（mlua 的原生报错指不出这一点，而修的地方在 Lua 预置表）。
  ② **线路键表漏了 `'+'` 和 `'_'`**，而游戏绑定表里就有（`volume_up = {'=', '+'}`、
  `volume_down = {'-', '_'}`）——症状是"Shift+= 调不了音量"，只在该按键、该平台出现。
  补齐 Shift 之后的**字符形式**（浏览器给的是 `event.key`），并新增**静态门**
  `scripts/check-input-abi-parity.mjs`：把 Rust 键表、预置层 `bindings` 表、宿主 DOM 键名经
  `keyName()` 归一后的结果三份来源对起来，**构建前即可判红、不需要产物**（已接入 `verify.ps1`）。
  ③ **语义对拍必须"两边都像出厂"**：新增 `scripts/wasm-lua/input-frame-abi-test.mjs`，以**旧协议
  作 oracle** 逐步比对 Lua 侧可观察内容（含畸形帧负例），并接入 `verify-wasm-lua.ps1`（`abi=pass`）。
  第一版测试每帧新建快照对象，于是"多键同时松开"那组冒出假差异——真实运行时的 `state.keys`
  是**常驻对象**、松开后保留 `false` 且此后每帧都带它；是"对拍的一方不像被测物"，协议本身没问题。
- **验收**：Rust 单测 / fmt / clippy 全绿；`verify-wasm-lua.ps1` 三套测试全绿
  （`wasm_lua_ok spike=pass bridge=pass abi=pass`）；`package.ps1` 发布口径构建
  `bridge_exports=10`（两个新导出进了产物）；真实浏览器端到端 **`webgl_bundle_ok`**
  （`scene_verify=done`、`api_contract: ok cases=33`），页面读数
  `data-rustlua-input="frames=… bits=… unmapped=0"`——**浏览器实际送来的每一个键名都落进了线路表**。
  完整玩法剧本 `-DriveJhyt` 本轮**走完全部里程碑**（M1 新游戏 → M2 按键移动 → M3 背包 → M4 任务 →
  M5/M6 跨场景往返），其中移动/开背包/开任务都是**按键驱动**，即输入协议在真实页面上确实驱动了玩法。
  （此前记录的 `nav-exit` 超时本轮不再出现；工作区含用户在改的内容，故**不把这条归因于输入改造**。）
  微信侧打包链走到**既有的主包体积阻塞**才停，此前全通过：`bridge_exports=10`、
  `no call guard present`、`upload_ecma_ok files=6 ecma=2019`、`wechat_minigame_split_ok`；
  微信适配层的 `__RUSTLUA_DIAG()` 增加 `input:` 字段（微信侧没有 DevTools，
  这是现场判断"输入有没有送到 Lua"的唯一凭据）。
  改后的开销与语义等价证据逐条见 `tasks/plan/M73-00-Web运行时性能重构.md` 的 WP3 一节。
- **回滚档补充**：输入批量线格式若要回退，须**整组一起退**——新 crate、两个 wasm 导出、
  两份平台门面与两处导出门清单（`bridge_exports` 回到 8）、运行时的打包器与预置表的
  `press_cursor = {0, 0}`；只退一半会得到"启动即失败"或"输入永远不动"。
- **每帧源码编译清零（WP3 第二片）**：第一片换掉输入快照那条之后，热路径上还剩"为调一个无参函数
  而编译源码"的写法——**每个固定步**把 `__rustlua_apply_virtual_input()`（以及首步之后的
  `__rustlua_finish_step_input()`）编译一遍，而这段源码永不变。新增桥导出
  `lua_call_global_void(name)`：取全局函数直接调用，不收参数 JSON、不字符串化返回值；
  这两个钩子改走它。同时把**无参调用**的参数从 `"[]"` 改成空串（Rust 侧空串即"没有参数"），
  省掉每帧 4~5 次纯为 `[]` 的 JSON 解析。Lua 侧那两份实现仍是唯一事实源，全局不是函数时照旧**报错**
  （旧写法也会报 `attempt to call a nil value`），所以"预置层没跑到 / 宿主缓存了旧运行时"不会被
  降级成"虚拟按键偶尔不生效"。
  - **顺带纠正一处判断**：计划里"把 `lua_call_global` 的 JSON 参数折进批量帧接口"这句是错的——
    `lua_call_global` 在 Rust 侧本来就没有编译源码，它的开销是参数解析与返回值字符串化，
    所以正确的做法是"去掉解析"，而不是"再加一层协议"。
  - **结果**：`web-runtime.js` 里 `doString` 只剩启动/场景绑定那 5 处，**帧循环内一处都没有**；
    `lua_set_number` / `lua_input_frame` / `lua_call_global_void` 三条热路径都不编译源码，
    每帧的 Lua 分配随之归零（逐条 µs/字节读数见 `tasks/plan/M73-00-Web运行时性能重构.md` 的
    WP3 两节）。新增的桥导出同步进了 web / 微信两份门面、`index.html` 白名单与两个 wasm 门面清单。
  - **验收**：`verify-wasm-lua.ps1` 全绿（`spike=pass bridge=pass abi=pass`，桥面门新增该导出可调用的
    硬断言）、测量脚本 8 条断言全过、`package.ps1` 发布成功、真浏览器完整玩法剧本
    **`drive_jhyt: done`**（M1→M6 全里程碑，其中按键移动与两个面板开合正是每步必经这两个钩子的路径）。
- **引擎渲染器接进真 web 宿主：`game_web` 第一次有了 GPU 通道（WP5 第五片）**。前四片都在探针里，
  这一片把它接进出货宿主——`game_web` 原本**连 `engine::host::Renderer` 都没有**（只有 `web_sys`
  画布交给 JS 的 Canvas2D）。现在它建 surface → 适配器/设备 → 引擎渲染器 → 把 **Rust 侧产出的场景**
  渲进画布纹理 → `present`，连渲 3 帧，结果写进 `data-rustlua-gpu`。
  - **开关的形状是刻意选的**：Rust 侧读 `?rustlua-gpu=1` 才启用（所以**不改** `plugins/webgl/`
    下任何既有脚本），构建侧 `package.ps1 -Gpu2d`（`game_web --features gpu2d`），
    门是 `verify-webgl-bundle.ps1 -Gpu2d`（断言 `data-rustlua-gpu=presented*`）。三处默认都关。
  - **踩到的坑（本片最值钱的部分）**：**一张画布只能有一个上下文**。第一版把 GPU 渲染器直接建在
    游戏画布上，启用后页面立刻 `boot=failed … detail=2D canvas context is not available`——
    WebGL2 先建起，`web-runtime.js` 的 `getContext('2d')` 随后拿到 `null`（浏览器只回 null、不抛错），
    **新通道把既有画面链路整条打死了**，而现场那句错误信息指向的是 2D 那一侧。修法：GPU 用自己的
    画布（`#rustlua-gpu-canvas`，`pointer-events:none` 把输入留给游戏画布），门的顺序也改成
    GPU 自检在前、两者读数同框。
  - 顺带把"从画布建 surface"那段带 `unsafe` 的边角料**收成引擎里的唯一实现**
    （`host/web_surface.rs`），探针与宿主共用；收编后探针读数逐项不变。
  - **边界**：这一片画的是**手工场景**（两块内建白四边形 + 一块贴图四边形，贴图优先取工程资源里
    第一张 PNG、拿不到才用内建棋盘），**不是游戏内容**；GPU 帧也只在启动时画 3 帧，还没接进
    JS/Lua 的每帧循环。换成 Rust 侧模拟产出的真场景属于 WP3。
  - **体积账（同一打包环境、同一份 `-Oz` 出货文件）**：wasm **2,842,215 B → 4,911,053 B**
    （**+2,068,838 B，+72.8%**）。**光 wasm 一项就超过微信主包 4 MB 的上限**——所以这条通道在微信侧
    要么进分包（WP4），要么继续走 Canvas2D；webgl 侧没有这个限制。这不是"顺手加个开关"的量级。
  - 真浏览器读数（`verify-webgl-bundle.ps1 -Gpu2d`，`exit=0`）：
    `data-rustlua-gpu="presented frames=3 draw_calls=2 triangles=6 texture=resource:assets/art/buildings/build_forge_stand.png (843 B) canvas=#rustlua-gpu-canvas 1280x720"`
    ——其中 `draw_calls=2` 说明第三片的保序合并在真宿主里同样生效，贴图走的是**真工程资源**
    （打包字节 → `pack::decode` → GPU 上传）；同一次 dump 里既有链路没有回归
    （`boot=running` / `scene-verify=done` / `drawn=5621` / `ui-elements=13` / `input frames=53`）。
- **Rust-only 宿主：证明 Rust 侧自足，并给出退休的"功能清单"判据（WP6-03）**。上一片把退休判据改成
  "先做到世界状态的单一来源"，这一片回答其中最难的一条：**Rust 那条路自己能不能把游戏跑全**。
  - **不用改任何用户文件就能试出来**：产物 `index.html` 的启动链是显式两步——`await start(...)`（Rust）
    之后 `await window.rustluaRunLuaGame(status)`（JS 世界）；GPU 宿主是 Rust 的 `start()` 自己读
    `?rustlua-gpu=1` 拉起的，资源表也是 Rust 侧自己预载的。于是把 184 KB 的 `web-runtime.js` 换成
    一个只定义 `rustluaRunLuaGame = async () => {}` 的替身，`index.html`/wasm/pack **一行不改**，
    跑起来就是"只有 Rust"的那条路（产物拷到临时目录再替换，不污染构建输出）。
  - **Rust 侧确实自足**：Rust-only 运行里工程脚本照跑（`[script] 进入 village`）、场景 31 个实体、
    240 帧全出图、几何门通过，`exit=0`。
  - **最硬的一条**：正常产物与 Rust-only 的截图**逐字节相同**（16060 字节，同一个 SHA-256），
    帧统计逐项相同 ⇒ **GPU 模式下玩家看到的那幅图已经是 100% Rust 画的**，JS 世界一个像素没贡献
    （它被压在不透明的 GPU 内容盒下面）。
  - **代价那一面**：30 帧处 `avg_frame_ms` 10.62（JS 世界在跑）vs 10.07（不启动）⇒ 差 0.55 ms/帧；
    含 rAF 等待且只有单次采样，只作方向——但方向明确：JS 世界在这条路上是**纯浪费**。
  - **下一步**：列"JS 世界仍独占的功能清单"（真实输入事件源/音频/存档/交互流程）并逐项找 Rust 等价物。
  - **坑**：`data-rustlua-scene-verify` 是 JS 世界写的门，Rust-only 跑法必须配 `-NoSceneVerify`；
    新增代码块曾放在 `Write-Log` 定义之前（PS 顺序执行）而报"无法识别的命令"。
- **量化"移除旧热路径"要认领什么：旧热路径在 Lua 层的净增量是 0（WP7-04）**。WP8 是删
  `plugins/webgl/web-runtime.js`（源 285,696 B / 5,352 行；打包经 terser 后出货形态 188,749 B——
  第 33 轮这里写的"4,766 行 / 188 KB"是把**压缩后的字节数**当成了规模，第 35 轮按文件实测改正），
  先回答"删了 Lua 侧会少什么"：
  - **方法**：静态抽取注入的 Lua 段落（扫模板串 → 挑 Lua → 按行首列定义点）＋ 引擎自报的 API 表面
    快照对账；快照新鲜度由 `docs-check` 守着（本轮实跑 `tables=42 funcs=290 stubs=275` + `ok`，退出码 0）。
  - **读数**：注入 4 段（最大 79,040 字节）、定义点 188（计入 56）；**引擎没有的表 0 张 / 0 个成员**、
    **表在成员缺 0 张 / 0 个成员**；两边都有 56 个；反向引擎有而补丁没碰的 34 张表 / 234 个函数
    ⇒ **没有单侧成员**（与第 31 轮 `expr=` 位串对拍"UI 77 名、0 个单侧"相互印证）。
  - **56 个遮蔽分两类**：**无条件覆盖 46 个**（引擎实现被换掉，删补丁会改行为 ⇒ WP8 的行为对拍
    清单）／**`if not` 兜底 10 个**（引擎有就不生效）。补丁里唯二"引擎没有"的 `api.load_settings`/
    `api.save_settings` 是**误报**：`api` 是 `for _, api in ipairs({audio, input})` 的循环变量，
    而引擎的 `audio`/`input` 都已有这两个成员。
  - **结论方向被改写**：删不掉旧热路径的原因不是 Lua API 覆盖，而是①渲染（画面 100% 由
    `web-runtime.js` 用 Canvas2D 画，引擎真渲染器要 WebGL2，**微信侧能力本轮未验证**）②宿主服务
    （存档编解码／虚拟键翻译／音量表各有一份 Lua/JS 实现）③那 46 处行为覆盖。
  - 顺带修三个工具链坑并写进审计文档 §五：PowerShell 5.1 的 `>` 会把 JSON 写成 **UTF-16**（Node
    读不动）、自写 JS 扫描器必须认**正则字面量**（否则第一版只找到 5 个小模板串、79 KB 的
    `luaPrelude` 一个没找到）、Lua 的 `for` 循环变量与 `local` 一样是变量。
  - **行为对拍（不只证存在性）**：探针工程 `artifacts/fixtures/lua-override-probe` 把 25 个代表成员各
    包 `pcall`、结果写进全局 `probe_summary()`，由驱动的只读表达式位读回；路径 A = 默认（预置表遮蔽）
    ／路径 B = `-StubJsRuntime`（引擎独跑），两次都 `-Gpu2d` ⇒ 99 字符指纹**逐字相同**（`A ≡ B`）。
    `E`（抛错）在两条路径上同样出现（`tag.set`/`light.set`/`container.*`）⇒ 那些覆盖大多是**把调用
    转发回引擎**。局限：25/56 个成员，"调用+返回指纹"不等于逐状态等价。
  - 环境坑（已修，也记进计划 WP7-04）：默认端口 8137 上挂着上一轮遗留的静态服务器（服务
    `examples/game_api`），新服务器绑不上、页面照旧打旧的 ⇒ 读数看起来"跑的是别的工程"；改用
    `-Port 8141` 解决，没有动那个不属于本轮的进程。
  - **同一轮内的自我更正（重要）**：门 ALL-GREEN，但门日志里 `[确定性]` **0 命中** ⇒
    **这条断言没有被门执行**（它挂在 `if ($DriveJhyt)` 段里，而门的 verify 步骤没传 `-DriveJhyt`；
    门里带驱动的是另一条用例 `subpackage-delay-move`，不经过这段）。
    ⇒ 准确结论是：**断言已写入且语法正确，但"确定性"目前还没被门守住**；下一步必须二选一：
    ① 运行器给 verify 步骤加 `-DriveJhyt`；② 把断言挪到总会执行的驱动用例旁；然后**重跑门并确认
    日志出现 `[确定性]` 行**。这正是"用例跑了但没生效"（第 77/120 轮）的同类，**必须查证据而非靠加代码**。- **七步门加"确定性"断言：同一输入剧本两遍读数必须逐项相同（WP7-89）**。
  - **为什么**：WP8 接管（引擎宿主/GPU 渲染）要做 A/B，前提是"同一输入序列 ⇒ 同一读数"；否则两遍差异
    分不清是"接管造成的"还是"这一局本来就不一样"。门里此前**没有**任何确定性/重复断言。
  - **加了什么**：`-DriveJhyt` 段末把既有驱动剧本（`--frames 120 --drive-new-game --drive-move 12`）
    **再跑一遍**，逐项比较 **12 个画布无关计数**（`state/tlines/ui/uiSame/uiP/xc/tiles/culled/drawn/
    draw_ops/alive/frames`），任一不同即抛出并**逐项列出两侧取值**；**故意不比时间**（`*Us`/`slow2ms`
    本机抖动约 5%，比它等于自找红灯）。
  - **结果**：七步门 **ALL-GREEN**（SUMMARY 七项皆 0），门日志里出现
    `[确定性] 同一输入剧本两遍读数逐项相同（12 个计数）` ⇒ 断言**真的执行了**（第 77 轮与第 120 轮
    都吃过"加了代码没跑到"的亏）。
  - **状态**：门的判据已连续两轮增强（WP7-88 契约 + 本轮确定性）；运行时源码未改。- **更正两处错判 + 把 33 条 L3 行为契约接进七步门（WP7-88）**。
  - **更正**：上一轮说"带 wasm 时契约对拍不执行"与"实跑没有契约输出"**都错** ——
    第 3707 行是 `if (checkApiContract && noWasm) { skipped }`，`else if` 分支才是执行契约；
    且契约一直**全绿**（证据行含 `api_contract=ok,cases=33`，逐行对桌面基线，含 event 四条）。
    看错的原因：那条证据行极长（该字段在约 1,400 字符后），我**截断到前 160 字符**再肉眼判断。
  - **教训（本轮最值一条）**：**长单行证据不能用眼睛扫，要按字段检索**——判"有没有 X"应
    `Select-String -Pattern 'X='` 抓字段，而不是截断整行去看。
  - **真实空白与改动**：契约离线可跑且全绿，但七步门里**没有**它（`verify-wechat-minigame.ps1` 中
    `--check-api-contract` **0 处命中**）⇒ 本轮在门的 harness 参数里加上 `--check-api-contract`。
    **意义**：门原本能比名字（L1）与读数，却抓不到"名字都在、返回值形状/状态归属分叉"这类补丁层典型缺陷
    （如 `clock.now()` 少三个字段）；现在由 33 条契约用例在门里守着。
  - **门结果**：**ALL-GREEN**（`SUMMARY` 七项皆 0），且契约字段出现在门的日志里 ⇒ 确实在门里跑了，
    不是"加了开关没生效"。
  - **状态**：运行时源码未改；门的参数已加强（本轮交付物）。- **工单②也早已完成：台账过期率 2/2；契约的离线入口被条件挡住（WP7-87）**。
  - **`event` 契约用例早就在**：`lua_api_contract.lua`（339 行）里已有 `event.emit.order`（= 台账所说
    "回调顺序"）、`event.emit.payload`、`event.off_alias`、`event.unsubscribe.by_callback`（≈141–170 行），
    桌面基线 transcript（33 行）第 14–17 行正是这四行（`a>b` / `seen=payload` / `count=0` / `count=1`）。
    ⇒ 台账 `event` 那条"没有自动化保证，只能靠契约用例钉"与 `clock` 那条同样**是过期描述**；
    **我从台账推出的 2 条"可行动项"，2 条都已过期（2/2）** ⇒ 台账的"该删/没有自动化保证"分类是
    **历史判断而非待办**，用它之前必须**逐条对着代码核实**。
  - **契约离线入口的现状（只陈述事实）**：契约头注写明 web 侧可用
    `node scripts/wechat-minigame-harness.js <发布目录> --check-api-contract`；实跑退出 0，
    但输出是普通 12 帧运行、**无任何契约输出**。查证：开关存在（harness 第 242 行），
    但执行处是 `if (checkApiContract && noWasm)`（第 3707 行）⇒ **带 wasm 的正常跑法不执行契约对拍**，
    且不报错。另一入口是 CDP（需浏览器）。⇒ 下一轮读清该条件意图：若可修，**离线（无需浏览器）
    就能拿到 L3 行为契约**，正好补上门目前抓不到的那块（返回值形状与状态归属的分叉）。
  - **改动**：台账 `event` 组 note 改为"**【已有契约用例】**+ 用例名与基线行号"；改后报告工具仍可解析
    （24/14/3 不变）、无 BOM/纯 CRLF。
  - **状态**：运行时未改（冻结树门禁全绿）；**连续第六轮无运行时交付物**（如实记录）。- **核实 `clock` 死覆盖早已删除，并修正过期台账条目（WP7-86）**。
  - **上一轮我误读了一处注释**：第 117 轮把那三条"契约实测"当成"覆盖仍在、只是行为分叉"，实际那段的
    开头第 5997 行写的是"**clock 兜底已删除**（L3 行为契约实测出来的）" ⇒ 三条实测是**删除依据**。
    **教训**：读注释要读整段，尤其"已删除/已修复"这类**状态词在段落开头**时，别把"历史证据"当"现状"。
  - **核实三条证据**：① 补丁层已无 `clock` 覆盖（`clock =` / `function clock.` **0 处**）；
    ② 删除依据脚本 `scripts/web-api-contract.js` 存在（4,224 B），头部原文记着引擎版与补丁版
    `clock.now()` 的字段差异；③ 发包工程**确实在用** clock（`main.lua` 的 `clock.now().day`、
    `clock.set(1,0)`、`clock.set_rate(2/0)`）⇒ 删除已被真实使用覆盖。
  - **实际改动（台账准确性）**：`lua_api_overrides.json` 的 `clock` 组仍以旧口径记着"**属于该删的死覆盖**"
    （已删却写"该删"）⇒ 改为"**【已删除】**（依据 `web-api-contract.js` 的契约实测，
    说明见 `web-runtime.js` 的 genericCompat 段）"；单命中锚点替换，改后仍可被报告工具解析、无 BOM/无裸 LF。
    **危害说明**：过期条目会让后来者（包括本轮的我）把它当待办去"再删一次"或不再信任台账。
  - **工单表更新**：① 删 `clock` 覆盖 = **已完成**（移出工单）；② `event` 契约用例（回调顺序 + 重入 emit）
    = **仍未做**，保持为下一件活。
  - **状态**：运行时未改（冻结树门禁全绿）；**连续第五轮无运行时交付物**（如实记录），
    但本轮有可验证改动（台账条目 + 三条核实证据）。- **更正前提：登记表是"偏差台账"，不是"缺口工单"（WP7-85）** —— 我第 113–116 轮一直在错误前提上推进。
  - **读 note 原文后的分类**：所有 `dropped` 名字都**写明了不该补的理由**——`animation`（web 帧循环不调
    `LuaRuntime::step` ⇒ 无状态源）、`camera`（真值在 JS 渲染侧不回 Lua ⇒ 补了只能给**假数**）、
    `transform`（只有平面 x/y，无层级/无局部世界两套 ⇒ **补不出语义**）、`mesh`/`particles`/`transform3d`
    （纯占位、Lua 侧无状态）、`tilemap`（三条**未被发包工程引用**）、`ui` 20 条（依赖引擎 UI 层焦点与布局，
    补丁只有局部 `scroll_state`；`focus_cycle` 在 `game_jhyt` 里**只在注释**里出现）。
  - **为什么这个更正重要**：照"补缺"去做，要么做不出来、要么给假数、要么**造出第二份状态所有者**
    （"两套状态"正是本项目反复在打的 bug 类型）。
  - **台账自己给出的真工单只有两条**：① **删掉 `clock` 死覆盖**（原文：fengari 时代兜底，wasm 已有真实现，
    **属于该删**，且**行为已分叉**）⇒ 同时命中"移除旧热路径"与"修正确性"；② **给 `event` 补契约用例**
    （回调顺序 + 重入 emit；原文："没有自动化保证，只能靠契约用例钉"）。两者都**离线可验证**。
  - **新增**：`artifacts/scratch/round117-actions.js`（按 note 关键词把台账分成
    该删/无自动化保证/纯占位/未引用四类）⇒ 下一轮并进报告工具的 `--actions` 模式。
  - **状态**：运行时未改（冻结树门禁全绿）；**连续第四轮无代码交付物**，但方向已由原文证据掰回。- **登记表工具修好并自证：工单第一次可用（WP7-84）**。
  - **schema（读原文得来）**：顶层有**两个**区块 —— `groups[]`（`{module, note, apis:[名字字符串]}`，
    只记"覆盖了哪些 API"，**没有** dropped/invented）与 `replacements[]`（逐条缺口登记，含
    `module/dropped/invented/line`）。**缺口在 `replacements`** ⇒ 上一轮工具只走 `groups`，故"条目总数 0"。
  - **自证**：`node scripts/lua-api-overrides-report.mjs` ⇒ 条目 24 / 有缺口 **14** / 自造名 3，且第 114 轮
    文本层看到的缺口名（`drag_begin` 等）**全部命中**；归属取自每条自身的 `module` ⇒ 结构性归属。
  - **工单（名字数）**：`ui` 20、`input` 14（**平台性差异**：web 无手柄）、`physics`/`physics3d` 各 6
    （**平台性差异**）、`animation3d` 5、`audio` 4（多为平台性）、`animation` 3、`transform` 3、
    `tilemap` 3、`camera` 2、`map` 2（多为平台性：web 无文件系统/无 ECS）、`mesh` 2、`particles` 1、
    `transform3d` 1；自造名 3（`input.press_position`、`inv.open`、`log.debug`）。
  - **工具不做什么（避免又被当结论）**：不判断"平台性差异算不算问题"；**未**交叉核对
    `groups[].apis` 与 `replacements[].dropped`（该交叉核对值得加，本轮未加）；上表的"平台性"一列
    是**人读原文摘的**，不是工具结论。
  - **下一步**：读 `ui`/`transform`/`tilemap` 的 `note` 原文判定真缺口 ⇒ 挑名字最少且纯数据的一件
    做"实现 + 门断言" ⇒ 七步门 ALL-GREEN，并把该条的 `dropped` 清空（工具输出贴进轮次文档）。- **登记表结构三次猜错：报告工具已落地但**尚不可用**（WP7-83，如实标注）**。
  - **产出**：`scripts/lua-api-overrides-report.mjs`（只读报告工具，无 BOM、纯 CRLF、退出码恒 0）
    —— 但**条目总数 0** ⇒ 目前查不到任何条目，**不得当工单使用**。
  - **三次假设全错**：① 文本按序配对 `module`↔`dropped`（35 vs 24 ⇒ 错位）；② `groups[]` 元素自带
    `dropped`（11 组全 0）；③ 缺口在 `groups[].replacements[]`（该键不存在，组的键只有 `module/note/apis`）；
    ④ 缺口在 `groups[].apis[]`（仍 0 条）。
  - **根因**：**一直在猜 schema，却从未读过这 420 行 JSON 的原文** ⇒ 与第 90/106/107 轮
    "人工点数跨界调用连错三次"是同一毛病的变体：**该读事实的时候在选择猜**。
  - **下一步（顺序固定）**：先读原文 40 行并贴进轮次文档 → 按事实修工具并**用已知缺口自证**
    （如 `particles.drag_begin`、`ui.go`）→ 再挑真缺口做"实现 + 门断言"。
  - **状态**：运行时源码未改（冻结树仍为门禁全绿）；**连续第三轮缺代码交付物** ⇒ 下一轮必须结束该状态。- **找到权威 API 缺口工单；确认存档落盘门早已在七步门里（WP7-82）**。
  - **候选"存档丢档"已被覆盖**：`lua_api_overrides.json` 的 `save` 条目披露了一条**已修复的真实丢档 bug**
    （`blob_read/blob_write/slot/slot_get` 曾走"缺失代理"⇒ `save_manager.save()` 照打 `[save] committed`
    而实际没落盘，玩家重开全丢），且它有活体门 `--check-save-persist`——**该门早已接进七步门**
    （`scripts/verify-wechat-minigame.ps1` 第 1166–1169 行，失败即 throw）⇒ 无需重复劳动。
  - **候选"`ui.scroll_*`"不成立**：全仓零命中 ⇒ 不是"实现了没接线"，而是根本不在 API 面上 ⇒ 属设计问题而非 bug。
  - **权威工单**：`plugins/webgl/lua_api_overrides.json`（11 组、35 个 module 条目、24 份 `dropped` 列表，
    字段 `module`/`note`/`dropped`/`invented`/`line`）⇒ 后续"补 API 缺口"以此为准；
    非空缺口涉及 asset/clock/container/event/light/ui/animation/animation3d/inv/map/mesh/particles 等模块。
  - **不可信处（已标注）**：本轮用"按出现顺序配对 module 与 dropped"统计，而两者数量不等（35 vs 24）
    ⇒ **配对错位**，名字列表真实但**归属不可信** ⇒ 下一轮改用 **Node `JSON.parse`** 重做
    （PowerShell 处理这份嵌套 JSON 已连错三次）。
  - **纪律**：独立跑 harness 时漏了必需参数（`--check-save-persist` 必须配 `--drive-new-game`）
    —— 本次会话 harness 参数错误第三次 ⇒ 以后**只从门脚本复制**现成调用，不凭记忆拼参数。- **`fixed` 通道"2 → 1"不是机械合并（WP7-81）：它要搬走固定步长的所有权**。
  - **读到的语义**：两次调用在**同一个函数** `advanceFixedSteps(elapsedSeconds)` 内顺序执行
    （`lua_fixed_frame` 问"本帧几步" → `lua_fixed_run_steps(steps, FIXED_DELTA)` 真跑），
    故每帧恰好 2 次（读数 `xc=` 第二项三档均为 **2.00**，与语义吻合）。
  - **为什么要停下来**：`FIXED_STEPS_PER_SECOND = 60` / `FIXED_DELTA` 目前**由 JS 持有并每次传入**
    ⇒ 合并等于把**固定步长策略的所有权**搬进 Rust。那是"统一 Rust/WASM 状态"主线的活，
    必须**单独实施 + 单独验收**（判据：`xc=` 第二项 2 → 1、`timing` 仍 `tick=runtime`、
    固定步累加器与 `[玩法] d=0,28` 逐项不变），不能顺手做。
  - **顺带算清批量余地**：生产口径每帧 8 次 = `call_global` 3（三处时间点/语义不同，不可合）
    + `fixed` 2（可合但要搬所有权）+ `input` 1（已是 `RLFI` 批量线格式，无可再合）
    + `set_number` 2（相机批量 1 + `dt` 1，时间点不同）⇒ **JS 侧"合并小调用"这条路已走到头**。
  - **方向结论**：努力应回到**引擎宿主 / GPU 接管**主线（每帧消费与绘制仍是最大头；
    第 101 轮分段：tilemap ≈68%、精灵+文字+叠加 ≈31%、实体收集 ≈0.5%）；该主线的推进受第 105 轮
    四条设备验证限制约束，证据需标注等级。- **门面漏名变读数自证（WP7-80）+ 冻结树上的干净七步门**。
  - **`br=` 探针**：boot 后第一次拼读数时检查运行时视野里七个关键桥导出是否齐全，输出 `ok` /
    `missing:<名单>`；用 `typeof === 'undefined'` 判定（`input_frame_layout_json` 返回字符串，
    用 `'function'` 判会假报缺失）。**结果：生产与进图均 `br=ok`**。
  - **为什么留着**："门面漏名"是只有改了 Rust 才会出现、报错却落在真机的故障（此前为此烧掉两轮）；
    现在它变成一行读数，且检查的是运行时**实际拿到**的对象 ⇒ 天然覆盖两个平台各自的名单。
  - **干净门**：本轮在**冻结树**上重跑七步门（上一轮是门运行期间改源码、非单一树验收）⇒
    **ALL-GREEN**，`SUMMARY` 七项皆 0；批量仍生效（`xlang`/帧 12.33 / 12.11，`xc=85/24/12/27`、
    `836/236/118/239`）；回归逐项不变（`d=0,28`、载荷字节与形状、失败串 0）；产物默认档 2,654,116 B。- **批量数值入口真正生效（WP7-79）：每帧跨界 14 → 12，行为逐项不变**。
  - **根因**：桥对象由宿主适配层的**显式白名单门面**搭建（`wechat-adapter.js:3814` 与
    `plugins/webgl/index.html` 第 53/126 行各一份，必须同步）⇒ 补上 `lua_set_numbers` 后即通。
    上一轮查不出根因是因为只查了 `web-runtime.js`；而门面注释里早就写明了这个失败模式。
  - **收益（三档一致）**：`xlang`/帧 14.33→**12.33**、14.11→**12.11**、14.05→**12.05**；
    `set_number` 分量 4.25→**2.25**、4.03→**2.03**、4.01→**2.01**（相机三元组 3 次单值 → 1 次批量；
    `dt` 必须在固定步前写 ⇒ 留在原时间点）。
  - **自证**：`snName` 里相机三项消失、只剩 `__rustlua_dt` ⇒ 单值通道确实被批量取代，而非"少算"。
  - **回归逐项不变**：玩法位移、`tlines`、载荷字节与形状、`uiSame`、生产 `ui` 读数全同，失败串 0。
  - **门七步 ALL-GREEN**；但我在门运行期间改了源码 ⇒ 严格说不是单一树的原子验收
    （第 1 步覆盖改前、第 2..7 步覆盖改后）⇒ **下一轮在冻结新树上重跑一次干净的门**。- **批量数值入口被"运行时看不到导出"挡住（WP7-78）：原因未定，运行时已恢复可用**。
  - **事实链**：重新打包后 **wasm 字节与 wasm-bindgen 胶水都含** `lua_set_numbers`，
    但运行时在相机处拿到的 `bridge.lua_set_numbers` **仍是 undefined**（表现：`wasm 缺 lua_set_numbers 导出`、
    只跑 1/120 帧、`drawn=0`）。
  - **两个待查（下一轮）**：① `window.__RUSTLUA_LUA_BRIDGE` 的赋值处（若为"逐名摘取"就补上该名）；
    ② 打包是否改写 `web-runtime.js` —— 若改写，则"产物里有没有我的改动"必须用**与实际写法无关**的判据，
    本轮按单引号匹配源码片段得到过一条**误判**（同一份产物里 `function setLuaNumbers` 又能匹配到）。
  - **纪律动作**：把相机调用**撤回三次单值写**并重打包验证 ⇒ 运行时恢复（harness 120 帧、
    读数与上一轮逐项相同）；Rust 入口与 JS 包装**保留但未被调用**。
  - **状态**：运行时可用；批量**未启用**；**七步门未跑**（源码相对上次门禁只多两处未被调用的代码）。- **批量数值入口落地一半（WP7-77）：Rust 侧已进产物，JS 侧已就位但未启用**。
  - **做成**：Rust 新增 `lua_set_numbers(pairs_json)`（`[["name", value], …]`，与逐个 `lua_set_number`
    同语义、同"缺导出不静默兜底"口径）⇒ **新打包的 wasm 里含该导出**（字节串检索命中，wasm 体积随之增大）；
    JS 侧写好 `setLuaNumbers(pairs)` 包装（含"批量也是一次跨界"的计数，避免读数假装变少）。
  - **未启用**：相机三连的批量化写法写好后又撤回 ⇒ **本轮没有把 "4 → 2" 落地**，读数与上一轮逐项相同
    （`xc=836/236/118/475`、`ui=308679`、`uiSame=115`、`tiles=34385/423200 culled=71415`）。
  - **踩坑①（作用域）**：把 `setLuaNumbers` 插进了 `setLuaNumber` **函数体内** ⇒ 相机处
    `ReferenceError` ⇒ 只跑了 1 帧。**教训**：插代码前要看锚点**所在的块**，不能只看行。
  - **踩坑②（陈旧 wasm）**：修好作用域后重打包仍报"缺导出"，而下一次同样的打包就有了
    ⇒ 那一次 wasm 是陈旧的（打包跳过了重建）。**教训**：要确认"新导出进产物"，得**直接查产物字节**，
    不能只看打包步骤的退出码。
  - **状态**：运行时已恢复并经验证（harness 120 帧、`tlines=30705`、无 `T 行`失败）；
    源码相对上一轮只多了 Rust 入口与未启用的 JS 包装；**七步门未跑**（下一轮补）。- **按名字拆开跨界调用（WP7-76）：14 次里约 4 次是诊断探针 ⇒ 生产口径约 10 次/帧**。
  - **点名（进图 118 帧）**：`call_global` 836 次 = `export_ui` 122 + `export_state` 120 +
    `take_pending_scene` 119 + **`world_summary`/`tilemap_layers`/`binding_probe`/`export_ui_texts`
    各 117（≈1 次/帧，全是诊断探针）** + `export_scene_text_hidden` 4 + `on_create` 2 + `binding_applied_probe` 1；
    `set_number` 475 次 = `cam_x`/`cam_y`/`cam_zoom`/`dt` **各 118（每帧各一次）** + 三个 `nav_*` 启动各 1。
  - **口径澄清（推翻混用）**：生产路径每帧约 **10** 次跨界（call_global 3 + fixed 2 + input 1 + set_number 4）；
    前两轮报的"14 次/帧"是**开诊断**的数字（多出的约 4 次全是探针）⇒ **两把尺子不能混用**。
    台账的"7 次"与实测的"14 次"差得离谱，根因正是"台账记生产意图、实测跑诊断口径"。
  - **附带代价**：**诊断模式每帧多约 4 次 Lua→JS 字符串往返** ⇒ 凡开着诊断测出的每帧数字都含这 4 次，
    与生产口径不可直接比较（此后读数必须标明口径）。
  - **第一个批量 ABI 靶子**：`cam_x`/`cam_y`/`cam_zoom`/`dt` 是**每帧 4 次独立标量跨界**（同一批语义）
    ⇒ 合并成一次往返可把该通道 4 → 1、生产口径 10 → 7；**判据天然可断言**（`xc=` 第四项 4 → 1，其余读数不变）。
  - **无副作用**：其余读数逐项相同，`T 行`失败串 0，门**七步 ALL-GREEN**；本轮只加诊断统计。- **14 次跨界拆成四分量（WP7-75）：一条不变量被证实，两处台账被证伪**。
  - **分量表（12/118/238 帧三档完全一致）**：`call_global` **≈7.08 次/帧**、`fixed` **2.00**、
    `input` **1.00**、`set_number` **≈4.03**，合计 **≈14.05 次/帧**（读数 `xc=a/b/c/d`）。
  - **证实**：`lua_input_frame` **恰好每帧一次** ⇒ 既有约定"输入注入每推进帧恰好一次"在离线长跑上
    **成立**；第 106 轮"它按固定步调用"的怀疑被**自己的数据证伪**。此后这条约定有读数守着。
  - **证伪**：台账写 `call_global`×2 + `fixed`×1 + `input`×1 + `set_number`×1 = 7；
    实测 `call_global` **多约 5 次**、`set_number` **多 3 次**（`fixed` 2、`input` 1 可对上）。
    人工点数跨界三次错三次（第 90 轮"6→5"、第 106 轮"7"、本轮两处分量）⇒ **台账作废，只认读数**。
  - **为什么重要**：`call_global` 是**字符串**通道（Lua→JS 只回字符串、每次带 JSON 编解码），
    它才是每帧跨界里最该动的一块 ⇒ 下一步按名字拆开这 7 次与 4 次，找出多出来的调用是谁，
    再谈合并/批量（**先知道是谁，再谈怎么合**）。
  - **无副作用**：其余读数逐项相同，`T 行`失败串 0，门**七步 ALL-GREEN**。- **跨语言调用计数落地（WP7-74）：实测每帧约 14 次 —— 把台账里的"7 次"证伪了**。
  - **做法**：在 5 个**真实**热路径桥调用点计数（`lua_call_global`（生产唯一入口）、
    `lua_fixed_frame`、`lua_fixed_run_steps`、`lua_input_frame`、`lua_set_number`），**诊断路径不算**；
    读数新增 `xlang=`，与既有 `frame=` 同行 ⇒ 每帧均摊可直接算。命名只说"跨语言调用"，
    不宣称覆盖全部通道（回调方向、诊断专用、一次性启动调用都不计）。
  - **结果（自我证伪）**：三档场景每帧均摊 **14.33 / 14.11 / 14.05** ⇒ ① 每帧跨界次数**是常数、
    不随帧数增长**（这正是要钉住的性质，现在有数了）；② **台账里的"7 次/帧"不成立**，实测约为其**两倍**。
  - **为什么这件事重要**："宿主接管后减少跨界"是主线收益的论据之一，**基数错了收益就会被算成两倍**；
    而人工点数跨语言调用已经错过两次（第 90 轮的"6→5"、这次的"7"）⇒ 台账即日起作废，改为引用读数。
  - **下一步**：把聚合的 14 拆成四个分量（`call_global / fixed / input / setnum`），
    重点查"7 次差额是谁在重复调用"（怀疑 `input_frame` 按固定步而非每帧一次、`set_number` 一帧多次）。
  - **无副作用**：其余读数与上一轮逐项相同（载荷、形状、`uiSame`、`tiles`/`culled`、`tlines`、玩法），
    `T 行`失败串 0，门**七步 ALL-GREEN**。- **钉清引擎宿主"接入"的边界：设备验证限制写成四条（WP7-73）**。
  - **三态各自被判据钉住**（验收脚本已断言）：默认关必须 `off/switch-off` 且固定步不得被接管；
    打开开关 + 默认内核必须 `unavailable/kernel-without-gpu2d`；GPU 档必须 `failed` **且以引擎原文
    `host: ` 开头**（该档用 `-ExpectGpuKernel` ⇒ 退化成 `unavailable` 直接判失败）；任何离线场景
    出现 `running` 即判"读数可疑"。
  - **关键证据（说明"接入"是真的通了）**：GPU 档下宿主报出**引擎自己的**错误——
    `source=wx.createOffscreenCanvas`、`reason="host: canvas_surface: Failed to create surface for any
    enabled backend: {Gl: InstanceError { message: "canvas.getContext() returned null" …`。
    即：开关→适配层→**Rust 侧画布/表面创建**这条路是通的；失败原因来自引擎原文、且正是"离线没有 WebGL2"。
  - **四条设备验证限制**：① `running` 与非零 `frames/steps/presented` 只能在真机取得，离线门禁止其出现；
    ② 离线上能验证的是"通路 + 归类 + 失败不吞 + 固定步只有一个主人"；③ 性能与稳定性（宿主每帧耗时、
    GC 压力、接管净收益）**不能离线验证**；④ 因此"接管后提速多少"必须标注为**设备侧待验证**。
  - **下一步**：把"每帧跨界次数"（当前生产路径 7 次）从文字台账变成**可断言的读数**，
    使"宿主接管是否真的减少跨界"在设备上有上下可比的一组数。- **消掉瓦片绘制的每格对象分配（WP7-72）：等价性逐位验证通过，离线收益不可测**。
  - **做法（最小面）**：瓦片循环原来每格调一次 `toCanvas`（每次返回新对象），改为把同一公式**就地算**
    并写进**循环外复用**的一个对象；**裁剪与绘制的调用点一行未改**。等价性来自"同一公式、同一
    相机字段、同一个 `camZoom`（其定义就是 `camera ? camera.zoom : 1`）"，且循环内无人保留该对象。
  - **验证判据 = 画出来的东西一模一样**：进图实画/考察与裁剪格数**逐位相同**、240 帧组相同、
    生产档相同、载荷与形状/`uiSame`/`tlines`/玩法读数逐项相同、`T 行`失败串 0、门 **ALL-GREEN**。
  - **收益如实记录为"离线不可测"**：tilemap 段变化约 −3%、绘制端约 +1%，都在约 5% 运行间噪声带内。
    这次省掉的是真机上每帧约 287 次短命对象的分配（约 1.7 万次/秒）⇒ 属**设备侧**结论，门内无法验证。
  - **后续方向**：`drawScene` 其余两段（精灵约 31%、收集约 0.5%）的绝对量已小于"假画布单次 drawImage"
    的量级 ⇒ JS 侧微优化性价比很低，力气应转回**引擎宿主/GPU 接管**（也正是能验证这条 GC 收益的地方）。- **tilemap 视野裁剪（WP7-71）：每帧实画格数降到约三分之一**。
  - **做法**：瓦片循环里算出画布坐标后加一段视野裁剪——画布已在 contain 的 scale 变换**内**，
    坐标系就是设计空间 1280×720，越界即 `continue`；**留一格余量** ⇒ 可见像素逐点不变，
    也不给取整差留边界隐患。新增 `tiles=实画/考察 culled=裁剪` 读数 ⇒ 是否生效可断言。
  - **效果（进图）**：每帧实画 **882 → 287 格（−67%）**，被裁 595 格；tilemap 段 **−69%**、
    整个绘制端 **−52%**、单帧最坏绘制 **−49%**、慢帧数 −15%；240 帧组同向。
  - **口径**：毫秒数来自门外**假画布** ⇒ 真机收益幅度不可知；**"每帧绘制调用 882 → 287"与画布无关**，
    才是这次改动真正的落脚点。
  - **无副作用（逐项对照）**：载荷与形状读数、`uiSame`、状态字节、`tlines`、玩法位移全部逐项不变，
    `T 行`失败串 0；生产档（无地图）`culled=0 tiles=0/0` —— 这同时是插桩自证。
    门**七步 ALL-GREEN**（含像素/上屏检查）⇒ 画面未被改掉。- **修掉打包脚本的"原生 stderr ⇒ 终止性错误"（WP7-70）：门回到七步全绿**。
  - **根因**：`package.ps1` 全局 `$ErrorActionPreference = "Stop"`，而 `cargo tree … 2>$null` 在
    包缓存被上一步 gpu2d 打包占用时把"Blocking waiting for file lock"写到 **stderr** ⇒ 被当成
    终止性错误 ⇒ 该步 exit=1。cargo 本身随后成功，所以这不是断言失败，而是**收尾没跑完**，
    产物被留在 gpu2d 档（上一轮的红与事故同源）。
  - **修法**：照本文件已有的手法（binaryen 那处）——只在这条命令上把 `$ErrorActionPreference`
    降级为 `Continue`、之后立刻还原；并补一条**可见但不终止**的提示：`deps.txt` 是**清单**不是判据，
    落空要看得见，但不该把一次成功的打包判成失败。
  - **验证**：七步 **ALL-GREEN**；上一轮红的那一步 `package-default-restore=0` 且其日志里有
    `wechat_minigame_package_ok` ⇒ 恢复真的跑完；`launcher/deps.txt` 非空；产物 `game_web_bg.wasm`
    回到**默认档**尺寸 ⇒ 产物树停在正确档位，这条由门自己守住。
  - 分段读数无回归：瓦片计数与上一轮逐位相同，`ui`/`uiSame`/`tlines`/玩法读数均未变，`T 行`失败串 0。- **绘制端分段（WP7-69）：tilemap 逐格循环占绘制端约 68%；并记下一次"假失败"引发的产物档位事故**。
  - **实测（进图 120 帧、267 实体）**：绘制端每帧约 21 ms，其中 **tilemap 逐格循环约 68%**、
    实体收集+排序约 0.5%，其余（精灵+世界文本+覆盖层）约 31%；240 帧组同向（约 68%）。
    瓦片计数：每帧**考察 3,527 格、实画 882 格**（**75% 是空格被提前跳过**）。
  - **限制**：门外是**假画布**，`drawImage` 是桩实现 ⇒ 这段耗时**不能当真机绘制耗时**。
    能立的只有①**相对占比**；②**调用次数**（与画布无关，是离线上可优化的抓手）。
  - **事故（已处理）**：最后一步"恢复默认档"红了，根因是 `package.ps1` 里的 `cargo tree … 2>$null`
    在上一步 gpu2d 打包仍占锁时把锁等待写到 **stderr**，而 PowerShell 把原生 stderr 当**终止性错误**
    ⇒ 该步 exit=1（cargo 本身很可能成功），**恢复没跑完 ⇒ 产物树被留在 gpu2d 档**。
    已用默认档重打包修复（`wechat_minigame_package_ok`、wasm 回到默认尺寸）。
  - **待修**：打包脚本不要让原生 stderr 触发终止性错误；恢复步骤要等上一步 cargo 进程退出后再开始。- **把第 93–98 轮的改动全部重做并复原（WP7-68）：源码与产物重新对齐**。
  - **方式（吸收上一轮教训）**：改用手改工具而不是按行号手术——它要求**唯一的精确匹配**，
    匹配不上或匹配到多处就拒绝执行，结构不可能被拼坏（本轮第一次尝试就在"两处相同文本"上被拒绝，
    正是它该做的事）；每次改完立刻 `node --check`；**写盘前断言行数**；完成后另存原样备份。
  - **重做四项**：UI 载荷形状读数 + 冗余度、两个导出的实测开销、UI 签名 + 同串缓存、
    产出/消费拆分 + 慢帧分布。
  - **复原判据逐项对照事故前**：生产 `ui` 载荷逐字节相同、形状（P/I/B/X 的行数与字节）逐项相同、
    `uiSame` 生产 0 / 进图 115 相同、进图 `ui` 载荷逐字节相同、`tlines=30705`、`d=0,28`、
    `draw_ops=665`、`drawn=1486`、`ui_elements=13`、`prefab_text=4/4`、七步 **ALL-GREEN**。
    UI 导出耗时与事故前同口径相差约 6%（噪声带内），与**优化前**相比仍是约 −66% ⇒ 那次变更确实复原。
  - **一处口径差（必须注明）**：状态导出的计时窗口比事故前**更窄**（本次把记录点放在 Lua 调用之后、
    `syncLuaScene` 之前，而事故前包含后者）⇒ 状态开销读数不能与前两轮直接比，这是口径变化不是性能变化。
  - 磁盘上的小游戏包已由本次门重新打包 ⇒ **上一轮"重做完成前不要跑会重新打包的门"的限制解除**。- **一次自伤事故的现场与恢复（WP7-67）：源码被工具脚本写坏，已恢复到第 92 轮末状态**。
  - **一句话**：想按行号给 `web-runtime.js` 注入计时，脚本两个缺陷叠加把文件写坏了（最终只剩 2 行）；
    恢复源找到了，**丢失的是第 93–98 轮对该文件的所有改动**（全部有文档，可机械重做）。
  - **根因 ①**：pwsh 里的"在循环中 return"的辅助函数，其输出是**累积**的 ⇒ 赋值端拿到数组而非索引，
    `Insert` 于是插到了文件开头且反复插。**根因 ②**：写盘前没有校验切分结果——整理阶段对一个
    已经是**裸 LF** 的文件按 CRLF 切分，只得到 1 行，却直接 join 回写 ⇒ 之后每次"修复"都在更小的
    残骸上继续切。
  - **硬规则（写进文档）**：循环里 return 的辅助函数一律 `return ,$i` + 调用端断言 `Count -eq 1`；
    **写盘前先断言行数**，切分失败即中止；**大文件手工编辑前先留原样备份**。
  - **恢复源判定**：`.temp/m7213close/head/webgl_stage/web-runtime.js`（源码原样，含 WP7-61、
    不含 WP7-62+ ⇒ 第 92 轮末）；包内那几份 254–260 KB 的都是**压缩过的**产物，不能当源码。
    恢复后 7,188 行、`node --check` 通过，并另存了一份**原样备份**。
  - **产物未受害**：磁盘上的小游戏包仍是第 98 轮构建（比恢复后的源码新）⇒ **重做完成前不要再跑
    会重新打包的门**（否则包会退回第 92 轮行为，验收脚本里读新读数的打印会变空）。
  - **待重做清单**（逐项有文档与判据）：WP7-62 载荷形状读数 / WP7-64 两个导出的计时 /
    WP7-65 UI 签名+同串缓存 / WP7-66 产出与消费拆分 + 慢帧计数。
  - **本轮没有推进性能目标本身**——如实记录；本轮的价值是把一次自伤控制在"有据可恢复"范围内，
    并把恢复路径写成可照着做的清单。- **把消费端也纳入计时（WP7-66）：帧时间的大头不在载荷上——优先级必须换**。
  - **做法**：① 帧循环里**先取 UI 载荷、再开始计时**，让 `drawUs` 只含消费端（解析 + 绘制）
    ——初版把计时器放在 `drawScene` 调用之前，会把产出端一起算进去（同轮自查改正）；
    ② 补**慢帧分布**（阈值取一帧预算的八分之一），读数 `slow2ms=s…/u…/d…`：
    第 96 轮只有一个"单帧最大"，不知道多常发生。
  - **实测（进图 120 帧、267 实体）**：状态产出每帧平均约 0.5 ms、UI 产出约 0.1 ms，
    而**消费 + 绘制每帧平均约 20 ms**、单帧最大约 32 ms，且**120 帧里有 102 帧超过 2 ms**；
    另一组 240 帧同向（约 22.6 ms/帧、223/240 帧超 2 ms）。
  - **结论**：消费/绘制那条路径是两个导出加起来的约 **33 倍**——把两份载荷全砍掉也换不回一帧。
    第 92–97 轮在载荷上投入是对的（那是当时**没有量过**的部分，也确实拿到了三分之二的收益），
    但**优先级现在必须换**：帧时间的大头在绘制/消费端，正是本目标里"GPU 渲染 / 引擎宿主接管"
    那一项要解决的问题。
  - **附带校准（重要）**：同一份代码、同一个用例，`stateUs` 在三轮里是 59,408 / 61,995 / 58,139
    ⇒ **运行间波动约 5%**。所以往后**小于 10% 的"改进"一律当噪声**，不能当证据；
    本轮 UI 那条（323 → 102 µs/帧 量级）远大于噪声才算数。
  - **限制**：门跑在**假画布**上，`drawUs` 是"JS 侧到假画布调用"这一段，**不等于真机绘制耗时**；
    能立的只有**相对**结论。有一帧标题画面 UI 单帧超 2 ms（12 帧样本里一次），更像 GC 落进窗口，
    不当作稳定特征。
- **UI 变更推送（WP7-65）：载荷逐字节等价，进图那条路径耗时降约三分之二**。
  - **做法**：产出端先走一遍**便宜的签名遍历**（不 `tostring`、不拼串），与上一帧的签名逐项比较；
    相同就把上一帧那份串**原样**交回（零分配），签名表用交换引用轮换、不拷贝。
    **正确性靠构造保证**：签名覆盖的字段与拼串字段逐项同序、同默认值（P 10 / I 10 / B 9 / X 6），
    所以"签名相同 ⇒ 载荷必然逐字节相同"；"签名不同但载荷相同"只会多拼一次，**不会漏发**。
  - **与第 91 轮那次被回归打回的区别（本轮最关键的判断）**：那次缓存的是**"渲染器持有什么"的推测**
    （影子状态，换场景后 JS 实体被重置成场景初值、影子仍留旧值 ⇒ 差值永远不会被发出 ⇒
    门的原话是"主角在 (528,159)、渲染器却在 (528,112)"）。这次缓存的**就是载荷本身**，
    渲染器每帧拿到的仍是完整的一份。**同一条思路，成败取决于缓存的是载荷还是推断。**
  - **实测（进图 120 帧、复用了 115 帧）**：UI 载荷累计字节与改动前**逐字节相同**、
    `uiSame` 不变、玩法读数不变，而该路径累计耗时**降约三分之二**、单帧最大降约三分之一；
    对照组的**状态导出**（未改）保持同一量级 ⇒ 差异来自本次改动而非环境。
  - **证伪链条**：缓存没生效 ⇒ 时间不会降；缓存生效但签名漏了字段 ⇒ 字节必然变化且玩法会像
    第 91 轮那样分家。两者同时成立才算数。
  - **保留意见**：标题画面 `uiSame=0`（每帧真变 ⇒ 仍全量重拼），那里的平均耗时下降**不归因于复用**
    （12 帧样本的方差与分配压力下降）；本节结论只依赖进图那一组。另外本轮只省了**产出端**序列化，
    消费端每帧仍要拆行解析整份载荷——那条成本还没单独量过。
- **两个回写导出的实测开销（WP7-64）：把"值不值得优化"从估算变成数据，并否掉我自己开头的假设**。
  - **动机**：前面几轮量的都是**规模**（字节、行数），从规模推开销只能估。于是直接在两个回写的
    唯一所有者里计时（`performance.now()`，**只在诊断开启时**做），量的是**整条路径**：
    Lua 侧 `tostring`/`concat` + 跨界（+ 状态那条的 `syncLuaScene` 回写）；**不含** UI 消费端的拆行解析——它在 `drawScene(...)` 里、窗口之外（第 97 轮复核改正）。
  - **实测（进图，120 帧、267 实体）**：状态导出平均约 **495 µs/帧**、**单帧最大 1.86 ms**；
    UI 导出平均约 **323 µs/帧**、单帧最大 0.77 ms；两个合计平均约占帧预算的 **5%**，
    而**最慢帧接近六分之一**。标题画面的大头相反：**UI 导出占约 85%**，状态只占约 15%。
  - **假设被否掉**：本轮开头我判断"这些开销在噪声里、不值得再动"——实测不支持。
    而且"看平均"会直接得出错误结论（平均值只有最慢帧的三分之一）；**必须看单帧最大**。
  - **两个场景的大头不同**：进图后状态导出占两者约 60%（对应每帧都在变的全量 `T` 行），
    标题画面 UI 导出占约 85%（一直在重发那 13 个元素）。所以"该先动哪块"取决于场景，
    只测一个场景就会选错目标——这正是第 95 轮补洞的价值。
  - **由此定下的顺序**：① UI 变更推送（进图后约 96% 的帧 UI 载荷逐字节相同 ⇒ 白做）；
    ② 状态 `T` 行——第 91 轮试过"按变化发"被真实回归打回（换场景后 Lua 侧的影子值与渲染器
    持有的值分家，差值永远不会被发出），要动它必须先有"渲染器侧现持有值的权威来源"，
    这正与 WP8（产出端进 Rust）是同一件事。
  - **限制**：时间是**桌面 Node + 假画布**，**不是真机**（真机更慢、倍率未知）⇒ 只能用于相对判断；
    单帧最大取自一次 120 帧运行，最慢帧的出现频率未测。
- **进图载荷基线（WP7-63）：补上一个会误导下一步的洞，并反转上一轮的结论**。
  - **先纠两笔账**。① 第 90 轮那张"每帧跨界账"**漏了一项**：`__rustlua_take_pending_scene`
    （帧回调开头，正常路径每帧都跑）⇒ 真实是 **7 项/帧**，那次"隐藏文本集并入状态载荷"是
    **8 → 7**，不是我当时写的 6 → 5。数字错了、结论没错，但账必须是对的。
    ② `engine_host_state` 从"未完成"清单里**撤下**：查了全仓库，它**没有任何代码消费者**
    （只有我自己的计划在提它）⇒ 没有消费者就没有每帧成本，把它改成二进制是空转；
    把它挂着当 TODO 才是真问题——它会让人以为那里有活可干。
  - **补上第 91 轮留下的洞**：玩法驱动用例里原来只印 `tlines`，于是"进图之后状态/UI 载荷多大"
    一直没量——而这正是判定"要不要换二进制 / 要不要改成变更推送"的依据。现在补印 `state=/ui=/uiSame=`。
  - **进图后的实测（120 帧、267 个存活实体）**：状态载荷每帧量级比标题画面大约**两个数量级**；
    而 UI 载荷的 `uiSame` 是 **115/120 帧（约 96%）逐字节相同**。
  - **结论被数据反转**：第 94 轮基于**标题画面**的 `uiSame=0` 判过"UI 变更推送买不到收益"
    （当时把"只覆盖标题画面"的限制写在文档里）。进图数据推翻它——**UI 变更推送值得做**：
    进图后绝大多数帧的 UI 一模一样，每帧全量重序列化 + 全量解析纯属白做。
  - **状态载荷仍是最大的一块**，且几乎全是每帧都在变的 `T` 行 ⇒ 靠"按变化发"省不掉（实体真的在动），
    这与第 91 轮撤掉那条改动的根因一致：要省它得先换结构（产出端进 Rust）。
  - **教训**：一个场景的基线不能代表另一个场景；把基线只印在标题画面用例里，等于给自己埋一个
    会误导下一步的读数。所以本轮的洞不是"少印了一个数"，而是"少印了一个**场景**"。
- **UI 载荷的形状量清了，以及一条"此路不通"的证据（WP7-62）**。
  第 92 轮量到 UI 载荷是同一帧里最大的一块（远超状态载荷），按承诺这一轮把它拆开看形状，
  并顺手把"要不要换二进制批量"这个问题查到底。
  - **只测量、不建模**：`data-rustlua-export` 增 `uiP=/uiI=/uiB=/uiX=/uiO=`（按行首类型
    P 面板 / I 图片 / B 进度条 / X 文本分别记行数与字节）。实现方式是**扫一遍字符串**，
    刻意不建任何模型——这样它不可能改变语义，也就不可能重演第 91 轮"影子状态与消费者
    分家"那类事故。测量的代价（每帧一次字符串扫描）与消费者本来就要做的整串解析同量级。
  - **产出端的形状**：`__rustlua_export_ui` 每帧对**所有可见项**重发全部字段
    （`P` 面板 10 个字段、`I` 图片 11 个、`B` 进度条 10 个、`X` 文本 7 个），每个数字都过一次
    `tostring`（浮点→字符串），消费端每帧把整串拆行再逐个 `Number()`。
    也就是说它的成本在**路径**（每帧全量重序列化 + 全量解析），不在于字节数本身。
  - **"换二进制批量"这条路，用桥的实际能力否掉**（有证据，不是推测）：
    Lua→JS 的调用通道**只回字符串**——`lua_call_global`、`lua_fixed_step`、`lua_fixed_run_steps`
    的返回类型都是 `Result<Option<String>, JsValue>`；JSPI 侧的批量通道是反方向的
    （`lua_input_frame(bytes: &[u8])` 是 JS→Lua，`engine_host_frame_into(dt, input, out: &mut [f64])`
    是 Rust 填 JS 的数组）。既然世界状态与 UI 状态**由 Lua 侧产出**，就没有"Lua 填一个类型化数组"
    的通道可用 ⇒ 想走二进制批量，前提是先把产出端搬进 Rust（那是"统一 Rust/WASM 状态"的终局，
    不是一次能顺手做完的事）。
  - **可达的那条路是事件推送**：桥提供 `lua_register_js_callback(name, callback)`——Lua 可以**直接
    调 JS**，不必每帧把整份状态拉一遍。
  - **但这条路也被读数否掉了**：新增的 `uiSame=`（这一帧的 UI 载荷与上一帧逐字节相同的帧数）
    实测为 **0**——**没有任何相邻两帧相同**，这份载荷每帧都在真变。于是"只在变更时推"省不到东西：
    变更本身每帧都要过。结论是 UI 载荷保持现状（交叉验过：同一份日志里 UI 载荷累计字节数除以帧数
    不是整数 ⇒ 各帧长度本就不同，与 `uiSame=0` 同向）。
  - **两条"否掉"合起来**：同一帧里能动的两个方向都有证据地关掉了——二进制批量不可达（通道只回
    字符串）、变更推送无收益（每帧都真变）。跨界次数 6→5、载荷规模已量清 ⇒ JS 世界在"每帧载荷"
    这一层的杠杆基本用尽；再往下要动的是结构（WP8：把旧 JS 世界换成 Rust 侧状态）。
- **状态回写与 UI 回写各只有一个所有者（WP7-61）**。
  第 91 轮的教训是"增量发要等状态回写有唯一所有者"；这一轮先把那个所有者立起来，
  同时补上一直没量过的 UI 载荷基线。
  - **收敛**：`__rustlua_export_state` 原有 **3** 个调用点（启动只读快照 `[false]`、换场景只读快照
    `[false]`、帧循环）、`__rustlua_export_ui` 原有 **5** 个调用点，现在各只剩一个入口
    （`exportWorldState(L, state, advance)` / `exportUiPayload(L)`）。归一的是**调用点，不是语义**：
    为真时照旧不带参数调用，为假时照旧传 `[false]`（快照不推进积分与动画）。
  - **为什么这算修问题而不只是整理**：`__rustlua_export_state` 一进来就把 Lua 侧的 op 队列清空，
    所以"谁取走了哪些 op"此前取决于调用点的先后顺序——多一个调用点就可能吞掉别人要用的 op。
    现在只有一个入口，隐藏文本集的来源判定（`H` 行优先、覆盖不了才回退老路）也跟它绑在一起。
  - **新读数（常驻、纯 JS 计数器）**：`data-rustlua-export` 增加 `exports=/frame=/snapshots=/ui=`。
    `frame` 是帧循环那次导出，**每帧至多一次**；`ui` 是 UI 载荷累计字节数（同一帧里唯一没量过的一块）。
  - **门 1a11**：`frame ≤ 帧数`（多出来就是"第二份实现"）；并且当 `run_steps == 帧数`
    （每帧都走到了帧级阶段）时加强为**恰好一次**——否则说明有帧漏了回写。这两条把
    "每帧恰好一次"从口头约定变成可断言的读数。
  - **自己踩到并抓住的坑**：用全局字符串替换收敛调用点时，模式**命中了定义体自身**，
    `exportUiPayload` 体内一度变成 `const payload = exportUiPayload(L);`——`node --check` 全绿、
    运行时无限递归。教训：全局替换必须框住范围，替换完逐处核对定义体；语法检查抓不到自递归。
- **状态载荷基线 + 一次被门拦下的增量优化（WP7-60）**。
  第 90 轮把每帧跨界的账算清并砍掉一次；这一轮先去量同一份状态载荷里**最大的一块**：
  存活实体的 `T`（Transform）行。
  - **先量基线（常驻、纯 JS 计数器、无跨界）**：`data-rustlua-export` 扩成 `pulls=/state=/tlines=`
    ——`state` 是每帧状态载荷的累计字节数，`tlines` 是真正发出的 `T` 行数。
  - **基线给出反直觉事实**：默认验收档（标题画面）实测 `state` 只有**几百字节**（12 帧累计）、
    `tlines=0`，而 world 读数写 `alive=0`——那 18 个实体来自 `.scene` 文件、由 JS 侧装载，
    **Lua 侧的变换表里一个都没有**。也就是说：同一帧里真正没量过的是 UI 载荷；
    "把状态导出换成二进制批量"这个方向**没有**被这份基线支持。
  - **试过的优化（已撤）**：Lua 侧按 id 存"上一次发出的变换"的影子表、**变了才发**（值精确，不改精度）。
    门里的玩法驱动用例当场抓到真回归：「Lua 世界里的主角在 (528,159)，渲染器侧却在 (528,112)
    （回写通道的 T 行没把位置同步给渲染器）」——第 90 轮同一用例 0 次失败，这次 1 次。
  - **根因（留下来的教训）**：影子表假设了"我们上次发出去的值 == 渲染器手上持有的值"。
    换场景/延迟挂包这类路径会让渲染器侧的实体被**重新装载**（回到场景文件里的初值），
    影子表却还记着旧值 ⇒ 这个差值**永远发不出去**。结论：**增量发必须等状态回写有了唯一所有者**
    （每帧恰好一次、结果一定被消费）之后再做，否则就是在赌生产者与消费者对同一份状态的理解永不分离。
    撤掉的原因写在代码那一行旁边，免得下次有人再试一遍。
  - **门变强了（保留）**：`-DriveJhyt` 进了默认门运行器 ⇒ 玩法通路（`--drive-new-game --drive-move`）
    每轮都被机器判据检查；`tlines` 的断言改成**因果绑定**：`alive==0 ⇒ tlines==0`；
    `alive>0 ⇒ 0 < tlines < 帧数 × 实体数`。
  - **一个空洞断言的教训**：最初的版本（`tlines < 帧数 × 实体数`）在标题画面上**无论机制有没有生效
    都会绿**（`0 < 216`）——"空洞通过"比红灯危险。现在这个前提-断言表把它堵掉了。
- **隐藏文本集跟着状态载荷走（WP7-59）**。
  第 88/89 轮把"谁推进世界""谁注入输入"各收到一个所有者名下；这一轮处理每帧跨界里最后
  一项明显可以省掉的。
  - **每帧跨界的账**：正常路径（宿主未接管）每帧固定 6 次跨 wasm 调用——输入批量、
    固定步时钟查询、固定步执行、帧级 `__rustlua_dt`、状态回写 `__rustlua_export_state`、
    隐藏文本集 `__rustlua_export_scene_text_hidden`、UI 回写。这一轮的落点是第 6 项。
  - **为什么能省**：`__rustlua_scene_text_hidden` 是 Lua 侧的表，全项目**只有一个写入点**
    （`ui.scene_text_visible(name, false)`）。既然"谁改"是唯一的，"每帧都拉一次"就纯属浪费
    ——拉回来还要重建一个 `Set`。改成跟着 `__rustlua_export_state` 的返回一起发
    （同一个 Lua 调用、零额外跨界），新增一行 `H<TAB>名字,名字…`。
  - **两个必须写对的地方**：① **空集也要发这一行**——消费侧靠它覆盖旧集合，只在非空时发
    就会出现"脚本把文字恢复可见了、屏幕上却还空着"；② **兼容层可能覆盖导出函数**
    （于是没有 `H` 行）⇒ 必须回退到老路单独拉一次，否则隐藏文本集会静默失效。
  - **回退必须可读**：常驻 `data-rustlua-export`（`pulls=` 纯 JS 计数器，无跨界），正常恒为 0，
    走老路就持续上涨。**门 1a9** 断言 `pulls < 帧数`——因为"少一次跨界"和"文字不再被隐藏"
    在画面上长得一模一样（都得看画面才能分辨），所以把"老路还在不在"变成一个可断言的数。
  - **自己踩到的坑**：`__rustlua_export_state` 是**写在 JS 模板字符串里的 Lua 代码**，
    我在 Lua 注释里写了反引号 ⇒ 直接把模板截断 ⇒ `node --check` 报语法错。差点造成
    "整个运行时加载失败"级别的故障。两条纪律因此更硬：改完立刻 `node --check`；
    模板字符串里的 Lua 注释中禁止反引号（已在那段代码里写了提示）。
- **输入注入也只有一个所有者（WP7-58）**。
  上一轮把"谁推进世界"变成了状态，这一轮把"**谁注入输入**"也归到一个所有者手里——并因此
  翻出上一轮自己留下的一个真 bug。
  - **那个 bug**：输入注入原本写在 `if (!hostTick && steps > 0)` 里 ⇒ 宿主一旦接管固定步，
    运行时整段跳过 ⇒ 适配层的"留一份批量给宿主"**从没发生过** ⇒ 宿主每帧收到 `undefined`
    ⇒ **整局对按键没反应**。它不报错、不崩、画面照旧（世界还在被宿主推进），离线门也照不出来
    （离线宿主的帧循环压根不跑）。
  - **定所有者**：输入注入 = **运行时**（设备层就在 JS 侧，天然只有它能采样：每帧采样一次 →
    打包一份 `RLFI` → 跨界注入一次）；世界推进 = 宿主（`__RUSTLUA_ENGINE_HOST_OWNS_TICK`）或
    运行时；两者**同源**。适配层只记账（`runtime_input_frames`）**不再转交**批量，
    `engineHostLastBatch` 这个变量被彻底删掉——留着它就是留着第二个所有者。
  - **顺序天然成立**：运行时帧先注入，宿主的帧循环在 `afterFrame()` 里后跑 ⇒ 输入一定在
    它那批固定步**之前**落地。
  - **门（1a7）**对两种口径各断言两件事：`runtime_input_frames > 0`（运行时确实在注入——
    正是那个 bug 的反面，谁再"顺手跳过"门立刻红）与 `input_applied = 0`（宿主一次都没注入）；
    两条合起来 = "注入来自恰好一处"。离线宿主起不来 ⇒ 第二条恒真，它的作用在真机。
  - **门（1a8）按不变量收紧**（先测出真实关系，再收紧）：推进者是运行时（`tick=runtime`）时，
    注入与推进**同条件**（都是 `steps > 0`）⇒ 两个计数**必须相等**。这不是"实测凑巧相等"，
    而是同一条判断的两个分支——谁把注入挪进别处，门立刻红。实测 `frames=12 /
    run_steps=12 / input_frames=12`（接管用例 30/30）。真机上宿主接管时两者不再相等
    （注入每帧都做、推进归宿主），所以这条只在 `tick=runtime` 的产物上断言。
- **一个世界只能有一个 tick（WP7-57）**。
  "宿主接管固定步"此前只是个**读数**（记一笔 `tick_conflict`），这一轮把它变成**状态**，
  并给最危险的那条退路上保险。
  - **三方一个标记**：适配层在宿主 `begin` **成功**且要求 `tick: "lua"` 时置
    `__RUSTLUA_ENGINE_HOST_OWNS_TICK`；运行时见到它就**不跑** `update`（也不重复注入输入批量，
    那一步由宿主做），帧级 dt 与相机抖动改用宿主刚报的步数（`__RUSTLUA_ENGINE_HOST_STEPS`）；
    宿主每帧推进 `plan.steps` 步并把 `steps` 写进定长 ABI。
  - **每一条降级路径都清标记**（画布/设备/管线/布局/帧内异常）：否则"标记交出去了、宿主却死了"
    会表现成**脚本照画、`update` 再也不跑**——画面完全正常，最难查。
  - **修正一个错的仪表**：第 86 轮拿 `lua_fixed_frame` 的调用次数当冲突证据，而它只是"这一帧该走
    几步"的**查询**（运行时每帧都要问，丢帧读数靠它）⇒ 宿主一接管冲突就恒为 `yes`。正确证据是
    `lua_fixed_run_steps`（**真的跑了一步**）的调用次数；适配层改包这一层（`runtime_run_steps`），
    并把它当判据：宿主接管时若非零 ⇒ 判 `failed` + 具名理由 `host: tick-conflict: …` + 把 tick
    还给运行时（宁可回到"老世界单跑"，也不要双步或冻住）。
  - **常驻读数**：`data-rustlua-timing` 扩成 `steps=/dropped=/acc=/sim=/tick=`——`sim=` 是本帧
    **真正推进世界**的步数、`tick=` 是 `host`/`runtime`，"一个世界一个 tick"不开开关就能读。
  - **门（1a6）**对两种口径各断言两件事：`tick_conflict` 不得为 `yes`（不可达状态）；
    `runtime_run_steps` **必须为正**（世界必须有人推进）。离线假宿主起不来 ⇒ 标记根本没交出去
    ⇒ 运行时照常推进 ⇒ 这两条恰好一起证明"宿主起不来时运行时会接管"这条降级路径是通的；
    真机上反过来（宿主起来 ⇒ 运行时停 tick ⇒ 冲突仍为 `no`）。harness 自报还带上 `timing=` 原文，门因此能直接断言离线时 `tick=runtime`（实测读数 `steps=1 dropped=0 acc=-0.0000 sim=1 tick=runtime`）。实测：生产 `runtime_run_steps=12`、
    接管用例 `runtime_run_steps=30`（各自 = 帧数），且冲突恒为 `no`。
  - **未完成**：真机上"宿主真的起来并接管"那一侧仍无读数（无设备）；`engine_host_state` 仍是 JSON。
- **双实现归一：删掉 JS 那份固定步顺序（WP7-56）**。
  第 86 轮把固定步顺序搬进了 Rust，但 JS 那份**还在**，所以那只是"多了一份实现"。这一轮把 JS 那份
  整份删掉，并给它补上此前**根本没有**的离线契约。
  - **删掉的旧热路径**：`web-runtime.js` 里每步的 JS 循环（`__rustlua_finish_step_input` /
    `__rustlua_apply_virtual_input` / `setLuaNumber('__rustlua_dt')` / `callGlobal(L,'update')`）、
    只服务于它的 `callLuaHook` 包装（连同那段"别为调钩子编译源码"的说明）、
    JS 累积器兜底（`stepAccumulator` + 1e-9 容差 + 追帧上限 + 丢帧计数）、以及只写不读的 `result` 遗留。
    现在运行时的固定步阶段只有一句 `runFixedSteps(steps)`，顺序只有
    `lua_bridge.rs::lua_fixed_run_steps` 一份实现；`advanceFixedSteps` 只负责"取宿主桥 + 断言两个
    导出都在 + 返回步数"，`acc=` 读数也改成读宿主的累积器（读数只认一个来源）。
  - **一条刻意的行为变化**：缺 `lua_fixed_frame` 的旧产物以前会退回 JS 累积器继续跑，现在**抛错**。
    这与项目对 `lua_set_number` / `lua_input_frame` / `lua_call_global` 的一贯口径一致
    （"产物换了一半"不许降级成手感问题），也是"移除旧热路径"的直接结果。
  - **`lua_fixed_run_steps` 返回最后一个 `update()` 的脚本结果**（不是步数）：步数调用方本来就知道，
    而"这一刻"的脚本返回值是被报出来的读数（`data-rustlua-script-result`）。旧 JS 循环当年也是
    "只保留最后一个固定步的返回值"——补步过程中的中间值没有意义。
  - **补上此前缺失的离线契约**（`input-frame-abi-test.mjs`）：六个固定步导出都在胶水上；
    **0 步不碰 Lua**；Lua 侧记录调用序列，`lua_fixed_run_steps(3, 1/60)` 必须产出 `AUFAUAU|3|1`
    （`A` = 注入虚拟键、`U` = `update()`、`F` = 吃一次性边沿）；**`F` 只出现一次**（补步不得重放
    `just_pressed`）；脚本看到的 `__rustlua_dt` **逐位等于** double 的 `1/60`（不是 f32 取整的
    0.016666668）；`lua_fixed_step` 单跑一步 = `AU` 且不吃边沿。顺序断言写成整串相等而不是包含子串
    ——顺序才是契约。
  - **门抓到的两个自己的错**：① 桥白名单是**逐个列名**的手写清单，两处（`wechat-adapter.js` 门面、
    `index.html` 的 import + 门面）都漏了 `lua_fixed_run_steps` ⇒ 三个微信门全红，而**具名错误**
    （`data-rustlua-boot=failed` + "wasm 缺 lua_fixed_run_steps 导出…"）一行就定位；两处已补齐。
    ② 我删 `result` 之前只 grep 了一个过窄的行区间，漏掉它在 `data-rustlua-script-result` 里的用法
    ⇒ `ReferenceError: result is not defined`；修正方式是让共用 tick 把最后一个 `update()` 结果带回来
    （比"删掉这个读数"更对）。
  - **验证**：wasm 门三绿（`[GREEN] wasm_lua_ok spike=pass bridge=pass abi=pass probe=pass` +
    `input_frame_abi_ok`，新增的固定步断言逐条 ok）；微信默认档与 GPU 档 + 还原后默认档
    **ALL-GREEN**，且默认档玩法读数与上一轮**逐项相同**（`draw_ops=665 draws_per_frame=5
    drawn=1486 ui_elements=13 prefab_text=4/4`）⇒ 逻辑确实走了新的单一实现且行为不变。
- **引擎宿主真的推进模拟 + 每帧定长帧 ABI（WP7-55）**。
  上一轮宿主接上了启动链，但固定步**只推进时钟**；这一轮让它能真的跑 Lua 固定步，并把每帧状态从
  "JSON 字符串 + `JSON.parse`"换成**定长数组**。
  - **固定步搬进 Rust，成为唯一实现**：`lua_bridge` 新增 `lua_fixed_step` / `lua_fixed_run_steps`，
    把 `web-runtime.js` 固定步阶段那三句（`__rustlua_apply_virtual_input()` → `__rustlua_dt = delta`
    → `update()`）搬成 Rust，并保留两条踩过坑的顺序（**虚拟键必须在输入批量落地之后**并回来、
    `__rustlua_dt` 必须写在 `update()` **之前**），以及 `step == 1` 时吃掉一次性边沿
    （补步不得重放 `just_pressed`）。
  - **谁跑固定步是个显式开关**：`options.tick` 只接受 `"none"`/`"lua"`（别的值具名拒绝），默认
    `"none"`。原因是老 JS 世界每帧也在跑固定步，**两边同时跑就是把世界推进两次**——这类故障两边
    各自都正常、只有画面不对劲，所以：① 默认关；② 适配层把 `lua_fixed_frame` 也包一层计数，
    宿主接管后老世界还在跑就报 `tick_conflict=yes` 并打警告，把看不见的双步变成读数。
  - **步长按 f64 现算**（`1.0 / updates_per_second`）：`FrameClock` 的步长是 f32（0.016666668），
    而 JS 喂给 Lua 的是 `1/60` 的 double——同一个"1/60"在两个类型下不是同一个数，接管模拟时
    按 f64 现算才不会悄悄改掉脚本看到的 dt。
  - **每帧定长帧 ABI**：一帧的全部逻辑收进唯一实现 `frame_core`，两个门面（`engine_host_frame_into`
    写 `Float64Array`、`engine_host_frame` 出 JSON）**共用同一份结果** ⇒ 不可能报出不一样的读数；
    布局由 `engine_host_frame_layout_json()`（`{abi, version, len, slots}`）交给 JS，是偏移的
    **唯一来源**（适配层不再自己维护字段表）。16 个槽位含新增的 `tick_steps_total`/`tick_failures`。
    热路径因此没有字符串、没有序列化、没有 `JSON.parse`；另外定长槽位让"字段在第几格"成为**可断言
    契约**——JSON 时代键名打错的表现是"字段恒 0"，属于最难查的一类仪器故障。
  - **失败面**：`host: frame-out`（`out` 太短，**先于**一切状态检查）/ `dt-ms` / `input` / `not-begun` /
    `input-frame` / `tick` / `present`；固定步失败**不呈现**这一帧（计入 `tick_failures`），呈现失败
    保留宿主并计数。适配层若发现 `engine_host_frame_into` 缺席或写入长度与布局不符 ⇒ **具名失败**
    （不退回 JSON：同一份产物里两个入口都在，退回等于把版本不匹配藏起来）。
  - **门与测试**：`verify-wechat-minigame.ps1` 新增 1a5 段（生产口径 `engine_host_tick` 必须是 `none`
    ——默认绝不允许出现第二个 tick；`--engine-host-tick` 之下结论仍必须落在 `unavailable`/`failed`
    之一且失败带 `host: ` 引擎原文）；`verify-canvas-host-probe.mjs` 新增布局自证（`abi` 自报、
    版本为正、`len == slots.length`、适配层读的槽位全在表里、`frame_index` 是 0 号）与定长入口的
    入参契约（`frame-out` 先于状态检查、与 JSON 入口同一条失败路径）⇒ 契约用例 **40 → 54 条全过**；
    harness 新增 `--engine-host-tick` 与 `engine_host_abi`/`_tick`/`_tick_conflict`/`_sim_steps`/
    `_runtime_fixed_frames` 自报字段。
  - **真机仍未验**：`tick: "lua"` 下 `sim_steps` 真的在涨、`begin` 的成功读数，只能由设备回答；
    老 JS 世界那份固定步顺序还在（所以默认必须是 `none`），下一步是让浏览器宿主也走
    `lua_fixed_run_steps` 再删掉 JS 那份——那时"两份实现"才真正归一。
- **引擎宿主接进微信适配层真实启动链 + 两平台共用帧时钟（WP7-54）**。
  上一轮把 API 与门做出来，这一轮把它接上真实调用点，并把浏览器宿主的定步长也换成同一个策略模块。
  - **A：适配层真的起宿主并按帧驱动**（默认关：`__RUSTLUA_ENGINE_HOST` / `wx.getStorageSync('rustlua_engine_host')`）：
    探针与宿主共用一个"专用画布工厂"（**每次新开一张**：一张画布只能有一个上下文，且两者不能共用
    同一 GL 上下文，否则一个失败会污染另一个结论）；按**上屏尺寸**起栈；`afterFrame()` 里每帧
    `engine_host_frame`（挂在覆盖层那段提前 return **之前**，两件事无关）；异常一律降级成 `failed`
    并停帧（诊断通道不许掀翻主循环）；状态进 diag、`data-rustlua-engine-host` 与日志。
  - **同一份输入协议**：适配层把 `lua_input_frame` 包一层，把那一帧的 `RLFI` 批量原样转给宿主；
    wasm 侧 `engine_host_frame` 的入参由字符串改成 `JsValue`（`Uint8Array` = 本帧批量，
    `undefined`/`null` = 无输入，其它类型具名拒绝），宿主用**同一个** Rust 解码入口——不发明第二套输入协议。
    新增读数 `input_frames`/`input_bytes`/`input_applied`/`input_note`。
  - **B：浏览器宿主换到共用帧时钟**：`lua_bridge` 的固定步时钟由 `FixedTimeStep` 换成
    `rustlua_infra::frame_clock::FrameClock`（容差显式传入并与该模块常量做 `debug_assert_eq!` 对齐），
    新增单帧墙钟上限 0.25 s（切标签页/断点/长 GC 的秒级间隔先夹住再谈追帧）。
    **正常帧逐位不变**（间隔远小于上限、追帧上限与容差同值），只有畸形间隔的记账方式变了
    （同样 8 步封顶，但丢弃量与累积器不再把挂起的几十秒算进去）⇒ "两个平台步进节奏一致"从约定变成结构。
    `FrameClock` 新增 `with_tolerance` 构造器 + 单测 ⇒ 原生单测 **7 → 8 个全过**。
  - **门与测试**：`verify-wechat-minigame.ps1` 新增 1a4 段（生产口径必须 `engine_host=off`；开关用例按
    产物档位落在两种具名结果之一——默认档 `unavailable`+`kernel-without-gpu2d`、GPU 档 `failed`+
    以 `host: ` 开头的引擎原文；`running` 在离线宿主上出现即判失败）；`verify-canvas-host-probe.mjs`
    的宿主契约随签名更新（用例 **37 → 40 条全过**）；harness 新增 `--engine-host` 与
    `engine_host`/`_source`/`_frames`/`_presented`/`_steps`/`_reason` 自报字段。
  - **抓到自己两个错**：① 就绪入口复用了**开关名**，模块加载时把 harness 置的 `true` 覆盖成函数 ⇒
    开关永远读成 `switch-off`；抓它的方法是**两个开关一起开的差分实验**（同一进程里探针
    `unavailable`、宿主 `switch-off`，一眼劈开"全局没传进来"与"这段代码有问题"），入口已改名
    `…_READY`。② `js_sys::Uint8Array::unchecked_into(input)` 编不过（E0308：`unchecked_into` 是
    `JsCast` 的方法、接收者在前），而 `package.ps1` 把 cargo 输出 `| Out-Null` 了、只吐一句
    `WASM build failed` ⇒ 手动复现才看到真错误。
  - **门脚本自身的缺陷（已修）**：第一版"一轮跑全套门"用了 `$ErrorActionPreference = 'Stop'`，
    而门判失败是往 **stderr** 抛 ⇒ 第 3 步一红就把后面的 GPU 打包/验收/还原全吞掉（本轮第一次跑
    正是如此）。已改成每步自己捕获 exit、最后统一汇总。
  - **真机仍未验**：`engine_host_begin` 的成功读数与每帧 `presented` 增量只能由设备回答；模拟 tick 与
    输入**尚未接进**宿主的定步长循环（批量已送到宿主，但只统计未解码进模拟）；老 JS 世界仍是微信生产路径。
- **GPU 档纳入验收门 + WP8 引擎宿主骨架（两平台共用帧时钟）（WP7-53）**。
  两件事：把上轮**手动**跑出来的 GPU 档证据变成**门级**判据；起 WP8 主线的引擎宿主骨架。
  - **A：验收门按产物档位断言**。`verify-wechat-minigame.ps1` 原来只认"默认档 ⇒ `unavailable`"，
    现在同一条断言落在**两种具名结果**之一（默认档 `unavailable` + `kernel-without-gpu2d`；
    GPU 档 `failed` + 以 `probe: ` 开头的**引擎原文**），因此同一份脚本在两种产物上都成立；
    另加 `-ExpectGpuKernel`：要求"这份产物必须带 GPU 内核"，报 `unavailable` 直接判失败。
    实测 `package.ps1 -Gpu2d` → `verify-wechat-minigame.ps1 -ExpectGpuKernel` **exit=0**，
    门自己打印 `引擎画布探针：status=failed reason=probe: canvas_surface: …` ⇒ 上轮那条手动证据
    现在是门级判据（跑完已还原默认档产物）。
  - **B：两平台共用帧时钟**（新增 `rustlua_infra::frame_clock::FrameClock`）：`time_step` 管算术，
    这一层管**策略**（浏览器 rAF 宿主与微信引擎宿主最容易漂移的地方）——夹住秒级间隔并计数、
    容忍"正好一帧"的子 ulp 偏差（与浏览器路径同一个 1e-9）、如实报出被丢的步数；`NaN/±Inf/负`
    一律按 0（否则步进循环一步不走且永远排不空，属静默故障）⇒ 7 个原生单测全过。
  - **B：引擎宿主骨架**（新增 `game_web::host_runtime`，仅 `gpu2d` 档）：`engine_host_begin/frame/state/end`。
    契约：**JS 拥有帧源**（rAF/`wx.requestAnimationFrame`），**wasm 拥有定步长策略**；失败一律
    `host: <stage>: <detail>`；入参校验**先于** GPU 工作；失败**不留半个宿主**（begin 失败后 `state`
    仍报 `not-begun`）；画布尺寸按属性直读；仍然不碰任何浏览器构造器。
    **边界**：固定步目前只推进时钟，模拟 tick 仍由 `lua_fixed_frame` 驱动，输入批量（`RLFI`）原样
    带过并计数、尚未在此解码。
  - **离线契约测试**：`verify-canvas-host-probe.mjs` 增补第 ⑥ 节（导出两侧存在、没 begin 就
    frame/state 具名、坏 JSON/坏数值/坏 clear 在碰画布前失败、null 与零尺寸画布具名、普通对象
    画布走到 GL 层且无 `not of type`/`instanceof` 措辞、失败后无半安装宿主）⇒ 该用例 **14 → 37 条**
    全过（`[GREEN] wasm_lua_ok spike=pass bridge=pass abi=pass probe=pass`）。
  - **自我纠正**：① `ZIG` **不能**排除（冷构建必需，见 WP7-51 条已改正；计划里早有一条正确记录说
    "缺 zig 能验证的前提是沿用既有产物"，我第 82 轮的判断与它冲突）；② `Start-Job` 不能起长任务
    （父 `pwsh -Command` 一退出子作业就没了，打包跑一半被带走——所幸产物未被改动，核对后重跑）。
  - **真机仍未验**：`begin` 的成功读数与每帧 `steps/presented` 只能由设备回答；适配层**尚未**调用
    `engine_host_*`（接线是下一片）；老 JS 世界仍是微信生产路径。
- **引擎画布探针接进微信适配层的真实调用点（默认关，结论进自报）（WP7-52）**。
  上一片只证明"打包管线不会弄坏这条路径"，这一片证明**它真的会被调用**：适配层（`0.7.5+engine-probe`）
  新增 `probeEngineCanvas()`，开关默认关（`globalThis.__RUSTLUA_GPU_PROBE` 或
  `wx.setStorageSync('rustlua_gpu_probe', 1)`），画布**必须新开一张专用离屏画布**（绝不碰主画布：
  一张画布只能有一个上下文），读数走属性 + `__RUSTLUA_DIAG().engine_probe` + 控制台一行，
  且**不 await**（不拖住启动），验证侧用 `__RUSTLUA_ENGINE_PROBE()` 等它落位。
  - **三条读数各自钉一件事**（都在打包产物上、30 帧）：生产口径 `off`（探针不跑，老读数逐项不变）；
    默认档内核 + 开关 `unavailable` + 具名理由 `kernel-without-gpu2d`（"没编进去"≠"编进去但宿主机不给"）；
    `-Gpu2d` 档内核 + 开关 ⇒ **引擎自己给出的具名失败**
    （`probe: canvas_surface: … webgl2 not available`）——证明链路 `适配层 → 专用画布 → wasm 导出 →
    wgpu 实例/表面/适配器` 真的走通，且三个用例的老读数（`drawn=1486` 等）完全相同。
  - **门已加**：`verify-wechat-minigame.ps1` 断言生产口径必须 `engine_probe=off`，并新增用例
    `gpu-probe-switch` 断言默认档内核在开探针时给出 `unavailable` + 具名理由。
  - **自我纠正（四处，都是我的错）**：① 第一版把 `off`/`unavailable` 做成提前 return、不写属性不打日志
    ⇒ 验证侧读到 `none`，与"这段代码没接线"完全同形，现改为**所有出口都写读数**；
    ② 探针原先在 `reportViewAgreement()` 处同步调用，而宿主引用是内核实例化 Promise 内部才存的
    ⇒ 真机会把 `gpu2d` 内核误报成 `unavailable`，现挂到 `wasmReady` 之后并新增
    `kernel-not-loaded-yet` 把这种时机错误变成可读状态；③ harness 没模拟宿主离屏画布 ⇒ 端到端用例
    第一次只走到"守卫拒绝主画布"（守卫是对的），现只在 `--gpu-probe` 下提供 `wx.createOffscreenCanvas`；
    ④ 自报里把 JSON 串截断在转义中间，改为先切原文再序列化。
  - **顺手否掉一条旧说法**：`package.ps1 -SkipBuild` 打包**失败**
    （`strip_wasm_tags_failed: wasm-as … unrecognized instruction`），让打包器自己构建就成功
    ⇒ "只要特性对，复用盘上 wasm 就等价"是错的（差别在构建入口的环境，不只是特性开关），
    门里第 80 轮的注释与本条一起改正：**打包一律让打包器构建**。
  - **还原纪律**：本轮我先用"修正前"的快照还原了产物，验收门随即在这份旧适配层上如实报红
    （`engine_probe=none`）——门没冤枉我，是我拿早取的快照覆盖了产物。快照已重取（修正版,
    重打包后），并记下：验收门读的是**盘上现成产物**，所以"源码已改但产物还是旧的"是一种真实故障
    ⇒ 改完源码必须重打包再跑门，快照必须在**所有源码改动之后**再取。
  - **真机仍未验**：微信给不给 WebGL2、设备上的 `ok` 读数只能由设备回答；本环境无真机、无开发者工具。
- **微信侧"带 GPU 路径"的形状实测：体积装得下、老路径零差异、打包产物上探针全绿（WP7-51）**。
  - **体积账**（打包器自报 + 产物实测）：把 `gpu2d` 编进微信产物后，主包占上限 **70.2% → 72.8%**、
    分包占上限 **37.6%**、打包器体积预检与 `wechat_minigame_package_ok` 一致 ⇒ **体积不是 WP8 的拦路虎**；
    内核本来就在 `pack/` 分包里（不计入主包），所以 wasm 近乎翻倍也没有顶到闸门。
  - **同宿主 A/B**：同一 harness 跑 30 帧，只换 wasm 档位 ⇒ `wechat_minigame_harness_ok` 自报**逐字段完全相同**
    （`draw_ops`/`drawn`/`ui_elements`/`prefab_text`/`upload`/`world`/`boot` 全等）⇒ 把 GPU 路径编进 wasm
    **不改变微信上老 JS 世界的行为**，因此不必先分两套包。
  - **探针打到打包后的胶水**：指向真正会被上传的那一份（已过微信补丁 → `downgrade_noextern` →
    `strip-wasm-tags` → terser），14 项契约**全绿**且与门里逐条一致，含"普通 JS 对象一路进到 wgpu 的
    WebGL 后端"这条关键判据 ⇒ 打包管线没有弄坏这条路径。
  - **自我纠正**：我一度拿 cargo 目标目录里的中间产物尺寸推断档位，判别实验后**否掉了自己第一版的解释** ——
    真正原因是**工具链不同**（`rust-toolchain.toml` 声明 `stable`，门/打包用裸 `cargo`，而我按旧习惯写
    `cargo +1.88.0`）⇒ **任何尺寸/性能读数都要走门或打包脚本的入口**，容量类结论只用打包产物说话；
    已排除"特性泄漏"（默认档构建几秒内换回另一份更小的产物 ⇒ 特性分离有效）。
    **但 `ZIG` 没被排除**：第 84 轮一次冷构建直接失败（`failed to run custom build command for
    mlua-sys`），设好仓库钉住的 zig 立刻通过——当时那句"设了结果不变"是 0.32 s 无操作的重建，
    什么都没证明；而"不进 cargo 指纹"恰恰危险（cargo 会原样复用另一套环境编出的 C 对象）。
    另修掉自己测试里的一处缺陷：`createRequire` 是按**测试文件所在目录**解析相对说明符的，路径须先转绝对。
  - **真机未验 + 未接线**：微信给不给 WebGL2 仍只能由设备回答；探针尚未被适配层调用，
    老 JS 世界仍是微信上的生产路径。
- **WP8 微信半边开工：引擎能在"没有浏览器构造器的宿主"上被问"能不能画"（WP7-50）**。
  - **先抓出一个门缺口**：`gpu2d` 是可选特性（`dep:wgpu` 只在它下面，发布时才由 `package.ps1 -Gpu2d` 打开），
    而 `scripts/verify-wasm-lua.ps1` 只构建**默认档** ⇒ "GPU 那条路上的导出"在门里**根本不存在**，
    新用例第一次跑就报"不可调用"——那是**门自己造成的假红**。改为**两档都构建**（默认档 + `--features gpu2d`），
    两档产物分开输出（`no-modules-test` / `no-modules-gpu`），并在收尾把默认档 wasm **还原**回磁盘
    （否则 `package.ps1 -SkipBuild` 会打包到 GPU 档，而且它不会自己报错）。
  - **新导出 `probe_canvas_host(canvas, report)`**：不碰 `window`/`document`/`location`，画布由调用方给，
    用 `unchecked_into` 收下（⇒ 只做属性/方法访问，不做构造器判定）；只建实例/表面/适配器并读能力
    （渲染器要的颜色格式给不给一并回答），**不**建设备、不建渲染器、不画帧。失败一律 `probe: …` 具名
    返回并同时走 `report` 回调 ⇒ 真机上控制台不可靠时仍有可读证据。
  - **离线契约由新用例钉住**（Node 里连 `HTMLCanvasElement` 全局都没有）：导出可调用、零尺寸与 `null`
    画布都给具名原因、**普通 JS 对象能一路进到 wgpu 的 WebGL 后端**（读数 `canvas_surface: … 
    canvas.getContext() returned null; webgl2 not available`）⇒ 这条路径里**没有** `dyn_into` 那种构造器判定，
    这正是微信切换的**前提**。
  - 自我纠正一处**死代码**：第一版用 `canvas_size()` 判零尺寸，而它自带 `FALLBACK_SIZE` 回退 ⇒ 该分支
    永不触发（被上面那条用例抓出来），改为直接读画布宽高。
  - **真机未验**：微信给不给 WebGL2 仍只能由设备回答（适配层 `glCapability` 已备好）；本入口**尚未接线**
    进小游戏发布链路，也没有定步长循环与呈现——那是 WP8 后续片的内容。
- **WP8 的边界用代码钉住 + 微信侧玩法驱动验收打通（WP7-49）**。
  - **平台归属（读代码，不靠叙述）**：webgl 默认**引擎宿主**（`plugins/webgl/index.html:163-167`，
    不请求 JS 宿主就打印 `legacy JS world skipped (engine host owns the frame)`）；微信产物入口
    `launcher/game.js:32-33` 是 `adapter.start(status => win.rustluaRunLuaGame(status))` ⇒
    **旧 JS 世界在微信上仍是生产路径**（第 48 轮在 webgl 上已证撤掉后画面逐字节相同）。
  - **微信切引擎宿主的真实障碍**（不是"接线漏了"）：引擎渲染入口 `start(canvas_id, title)` 按
    **浏览器构造器**做 `instanceof`/原型调用（`pkg/game_web.js:599-601`）⇒ 不能搬进小游戏宿主；
    无 DOM 的 `start_wechat()` 存在且能编进 wasm，但流程只到「三必需文件 → Lua 桥 → **一帧**
    update」、**不含定步长循环与 GPU 呈现**，且未接线（`game_web.js:595-618`、`lib.rs:311-317`）。
    ⇒ 缺的是**不依赖浏览器构造器的"引擎运行时+GPU 呈现"入口**。适配层已备好设备侧仪器
    （`glCapability`，`wechat-adapter.js:1451-1521`：「微信给不给 WebGL2 直接决定 WP8 在微信上
    能不能沿用同一条渲染通道」，只能在真机/开发者工具回答）。
  - **本轮交付：微信侧玩法驱动验收**。harness 的宿主驱动本来就有（`--drive-new-game` +
    `--drive-move N`，零位移即报红），缺的是验收脚本次通道（微信没有 URL 旋钮）⇒ 加 `-DriveJhyt`。
    **两层断言**（第 77 轮教训：用例"跑了但没生效"必须红灯）：①`exit=0`；②自报里必须解析出
    **非零** `move=… d=dx,dy`。
  - **验收**：`[玩法] keys=w before=528,112 after=528,140 d=0,28`（微信宿主上主角真的走了 28 像素）；
    同条自报世界读数 `map=manor alive=267 player=JianghuPlayer@528,112 tilemaps=4/4` ⇒
    进的是玩法地图。整跑 `verify exit=0`、自报 **30** 条（加这条前 29 条）。开关是 opt-in ⇒
    默认行为不变（第 78 轮全绿记录仍是基线）。
  - **顺带核清**：微信侧**没有**双份模拟（日志里 `gpu_host`/`wgpu`/`RuntimeScene` 命中 0，
    `web-runtime:` 115 条）⇒ 微信上的性能问题不能靠"关掉另一个宿主"解决。
  - **限制**：无真机 ⇒ `glCapability` 仍未知，WP8 的微信半边**仍未开始**（本轮只补验收前置）；
    驱动只测一条轴（`w`，12 帧）与非零位移，**不作**性能/帧率声明；旧 JS 世界在微信上仍是
    生产路径，本轮未改变，也不该被任何读数掩盖。
- **`--remote silent`：把「看门狗被真正验证」覆盖到远端形态（WP7-48）**。
  - **关掉的是第 77 轮自己声明的缺口**（full/remote 主包形态无异步可静默资源）。
  - **技术点：静默本身不够**。适配层的 `remote-assets.json` 带着 `timeoutMs`（harness 原本固定写
    3000）⇒ 只让宿主静默，**客户端自己的 3s 超时会把"挂起"终结掉**，boot 照样结算、用例换个地方
    继续空转。所以 silent 必须两件事一起做：①宿主收下 `request`/`downloadFile` 却永不回调
    （发起前就返回 task，进度回调也不投递）；②配置 `timeoutMs` 放大到 **120s**，配合
    `--boot-timeout-ms 1000` 正好是"看门狗先响、客户端还没放弃"的真机样子。
  - **另一处易错**：`down`/`corrupt` 那组会把路径形式默认改成 `missing-pack`（包内读不到 pack/），
    **silent 不能跟这一组** —— 它模拟的是"骨架在包里、重资源那一次下载挂起"，整体禁掉包内读取
    会让挂起发生在骨架那一步，验的就不是重资源远端这条链了。
  - **验收三条（全部实测）**：①远端形态单跑卡死用例 `negative_ok case=boot-stall boot=null
    pending_images=0 watchdog_ms=1000 on_screen=yes`（真卡住 + 看门狗**画到画布上**）；
    ②**远端（瘦首包）形态完整验收 exit=0**、25 用例/21 条自报、`asset_pack=thin remote_fallback=ok`，
    卡死用例在这个完整验收里被真正走到（同轮 `checksum-corrupt`/`remote-unreachable` 也各有证据行）；
    ③分包形态回归（打回 subpackage）`exit=0`、26 用例/29 条、卡死用例同样 `on_screen=yes`。
  - **仍未验证的一条（据实记录）**：full 形态的 `[跳过]` 分支 —— 想造 full 产物时打包器**当场拒绝**
    `微信小游戏主包超限: 7,907 KB > 4,096 KB`。该形态对本工程**原理上不可达**，那条分支是
    "资源小到能全塞进主包"的工程才走的安全网，**不声称已验证**。
  - **限制**：silent 下客户端不超时（配置 120s）⇒ 验的是**看门狗**，不是"客户端超时后如何收场"
    （后者由 `--remote down` 覆盖，仍在绿）；`timeoutMs=120s` 是保证"截止时间内不放弃"的建模选择；
    瘦首包验收必须用 `-AssetPackRoot` 指出导出的 CDN 目录。
- **否定用例不许空转：把「启动卡死」造在 boot 真等的那一环上，微信验收转全绿（WP7-47）**。
  - **症状**：第 76 轮留下的红 `wechat_minigame_harness_negative_vacuous case=boot-stall
    reason=boot_settled_before_deadline pending_images=7 images_loaded=0 boot=booted` ——
    该用例本该让启动卡住、再断言看门狗把「启动超时」**画到画布上**；实际 7 张贴图全挂着、
    boot 照样结算 ⇒ 用例**空转**（没有验证任何东西，却看起来跑了）。
  - **先查清 boot 到底等什么**：适配层启动链是「分包挂载 → wasm → 字体 → Lua boot」，
    **贴图不在其中**（首屏不等资源是刻意设计）。`--image-src-silent` 曾经能卡住 boot，
    首屏优化之后就不能了 —— 用例前提失效，而除了它自己的空转自报，没有任何东西提醒我们。
  - **修法**：分包形态下内核就在 `pack/` 里 ⇒ 启动必须等这条下载 ⇒ 新增
    `--subpackage silent`（宿主收下 `loadSubpackage` 与进度注册、**永不回调**，
    与 `--image-src-silent` 同一模型，也是弱网/CDN 挂起的真机样子）；`silent` 与 `never`
    一起归入"按设计取不到分包"的候选路径，成功路径的分包闸门不再冲它开火。
    验收脚本按产物 `game.json` 自身判定形态：有分包就带 `--subpackage silent`；
    没有分包的形态保留贴图静默，并在真空转时**显式记下原因**（而不是放宽断言）。
  - **验收**：单跑该用例 `negative_ok case=boot-stall watchdog_ms=1000 on_screen=yes
    pending_images=0 boot=null`（看门狗不只是报警，**画到了画布上**）；
    完整微信验收 `verify exit=0`、26 用例/29 条自报全通过、三个否定用例各自给出证据行。
    **本会话第一次把微信平台验收跑到全绿**（上次整跑是第 56 轮）。
  - **方法论**：①否定用例空转比没有用例更坏（给出"已验证"的假象）——本例靠 harness 主动报
    `negative_vacuous` + 验收脚本拒绝接受空转才定位得到；②卡死用例必须造在"boot 真等的东西"上，
    加载路径一变就要重新问"这条回调还在等待集合里吗"；③"形态不支持"必须明写并记入计划，
    不能把断言放宽成"空转也通过"。
  - **已知覆盖缺口（不是通过）**：full/remote 主包形态的 boot 等待集合全是本地快速路径，
    今天没有可静默的异步资源 ⇒ 那条形态的看门狗仍未被验证，需新增 `--remote silent` 才能覆盖。
- **补齐三个 `ui` API（判定性验收：不带 `-Drive` 的 game_api 由红转绿）+ 微信侧两道闸门修复（WP7-46）**。
  - **要补什么**：第 75 轮第一次"不带 `-Drive` 跑 game_api"红在启动 ——
    `demo_index.lua:130: bad argument #6 to 'format' (number expected, got table)` +
    `missing_api_paths=5: ui.scroll_max, ui.scroll_visible(.start/.stop), ui.text_get`。
    链条：游戏把 `ui.scroll_max()` 当**数字**用 → 引擎**有**（`api/ui_nav.rs:187`）而本侧**缺** →
    缺失陷阱代理成"可调用、返回一张表" → **游戏自己**的 `string.format` 抛错 → 启动失败。
  - **按引擎契约逐个抄**：`scroll_max(id)` = `max(content-viewport, 0)`，**无滚动区返回 nil**
    （不是 0 —— 调用方写的是 `or 0`，而陷阱此前给的是**表**，`or 0` 完全不生效）；
    `scroll_visible(id)` = `{start, stop}`（字段是 **stop 不是 end**，引擎注释专门写了原因），
    `stop` 钳到 content；`text_get(id)` = `{text,x,y,scale,size,visible}`，其中 **size 是渲染口径**
    （显式字号优先，否则 `scale × EM_PX`，`EM_PX = 12.0`），并返回**新表**只含这六个字段。
  - **判定性验收**（比任何单元断言都硬）：`verify exit=0`、`missing_api_paths=0`（原 5）、
    `degraded_api_paths=0`、`boot=running` —— game_api **不带 `-Drive`** 转绿。
    同轮 `rustlua-lua-do` 预登记断言逐条命中：`tg_size=24.0`（2×12）、`tg_explicit_size=30`、
    `tg_missing=nil`、`sm_open=300.0`、`sv_open=0.0..100.0`、`sv_after=300.0..400.0`、
    `sm_missing=nil sv_missing=nil`。jhyt 回归：`verify exit=0`、`missing=0`、降级仍 2、
    `-DriveJhyt` 全里程碑通过。
  - **同类事故第 9 次**：新写的 Lua 注释里又用了反引号（7 行），这次被**预检拦在打包之前**
    （`webgl_runtime_syntax_error` + `--注释内反引号=7`），机械修（成对换「」）后为 0。
    同时暴露我自己的流程漏洞：`node --check` 打印了 `exit=1` 但命令**继续打包**了 ——
    检查到位、闸门没接上；本轮后半段起语法不过即 `exit 1` 中止。
  - **微信侧（同一份 web-runtime）**：整跑验收（16 用例/22 条自报），修好两道**既有**红：
    ① 豁免白名单过期 —— 活体探测抓到 `input.nav_settings`/`set_nav_settings`/`last_device`
    三条**已实现**却仍列为缺失（`stale=3`），清表后通过（`last_device` 仍走**降级**登记：
    "降级"与"缺失"是两条读数，混表会让缺失清单失去意义）；② `--no-wasm` 用例被两条无关闸门
    判死（无解释器时"没读 pack/""探不明路径形式"是**预期失败的后果**而非缺陷）⇒ 给两条检查
    加 `!noWasm` 守卫，用例遂按契约 exit 0 并自报 `wasm=no-webassembly`。
  - **第三道红（已暴露、未修，下轮第一件事）**：否定用例 `boot-stall` **空转** ——
    让 `Image.src` 静默以制造卡死，现在 boot 在 `pending_images=7 images_loaded=0` 时照样
    `boot=booted` ⇒ `wechat_minigame_harness_negative_vacuous`，验收脚本（正确地）拒绝放行
    "看门狗自证没有被验证"。该路径本轮未碰 ⇒ **既有缺口**（上次整跑微信验收是第 56 轮）。
    待查：boot settle 条件是否在首屏优化后不再等图；若是，需换一种真能卡住的阻断方式，
    **不能删掉这个用例**（它守的是"卡死时玩家至少看到超时提示"）。
- **验收门不许猜工程类型；顺带被"不许猜"逼出一个启动失败的根因（WP7-45）**。
  - **问题**：`verify-webgl-bundle.ps1 -Drive`（标题确认→进玩法地图→位移非零）只对玩法工程有意义，
    原先用**工程名**分辨（`$pageIdentity -notmatch 'jhyt'`）：多一个展示型工程就假红，而玩法工程
    真的没响应 confirm 时会被**静默跳过**（假绿，最坏的一种）。
  - **第一次尝试被实测推翻**：改成"页面自报没观察到场景切换就跳过"，但 game_api 的启动场景
    **确实**消费了 confirm（切到 `main`），卡在后面"按住的键不驱动位移" ⇒ 仍会走进跳过。
    **根因**：运行时**原理上分不出**「本来就没有可走动玩法」与「可走动玩法坏了」——两者观测完全
    一样（没有输入驱动的位移）。**既然观测分不出，就不许猜**。
  - **最终形态**：唯一例外是"本次没要求 JS 宿主"（页面自报 skipped，这是事实不是推断）；
    其余任何失败一律 **RED**，消息带 ①detail 原文 ②按 detail 报出**卡在哪一步**（没观察到场景
    切换 / 读不到 player / 位移为 0）③可操作提示（展示型工程别用 `-Drive`，改用 lua-do/WorldDo
    断言；玩法动线用 `-DriveJhyt`）。报"卡在哪一步"是**报告观测**，不是替调用方下结论。
  - **让红灯可被造出来**：剧本三个参数原本硬编码，失败只能靠碰巧 ⇒ 新增 URL 旋钮
    `rustlua-drive-wait` / `rustlua-drive-move` / `rustlua-drive-move-frames`（默认值不变），
    并清掉一处 `if (driveAuto) { }` 死代码。验收：jhyt `-Drive` = GREEN；
    jhyt + `rustlua-drive-move=p` = **RED**（故意造出的红灯，证明"严格"不是嘴上说说）；
    game_api `-Drive` = RED 且消息指出卡在『位移非零』。
  - **副产品（新发现，不是本轮回归）**：本轮第一次"不带 `-Drive` 跑 game_api"，当场红在**启动**：
    `demo_index.lua:130: bad argument #6 to 'format' (number expected, got table)` +
    `missing_api_paths=5: ui.scroll_max, ui.scroll_visible(.start/.stop), ui.text_get`。
    根因链：游戏把 `ui.scroll_max(...)` 当数字塞进 `%.0f`（`demo_index.lua:136`）→ 引擎**有**这个
    API（`api/ui_nav.rs:187`，返回数字）而本侧**没有** → 缺失方法陷阱把它代理成"可调用、返回一张表"
    → 游戏自己的 `string.format` 抛错 → 启动失败。即"缺口代理把症状推得很远"的可复现实例。
    **为什么以前没看见**：以前对 game_api 都带 `-Drive`，剧本按空格切走了 `index` 场景，那段
    update 从没跑过 ⇒ 由此得到方法论：**`missing_api_paths` 是调用驱动的**（同第 72 轮降级清单），
    换场景路径就会变，"上次是 0"不能当成"这次也该是 0"。
  - **下一步（判定性验收）**：补齐 `ui.scroll_max` / `ui.scroll_visible` / `ui.text_get`，然后
    "不带 `-Drive` 的 game_api 验收"必须转绿。
  - **限制**：严格门要求"对没有可走动玩法的工程不要用 `-Drive`"（脚本只能给提示，判断在调用方）；
    三分法只覆盖当前三种失败文案，新形态落进"未能定位"。
- **键名词汇表补全 + 有损合并计数 + 正向/反向不对称（被新夹具当场抓出）（WP7-44）**。
  - **为什么要扩**：设备层取 `event.key` 小写，引擎用 winit 物理键名。第 73 轮接通了**翻译**，
    词汇表的**宽度**成为瓶颈：`key:shiftleft`/`key:f1`/`key:capslock`/`key:backquote`/`key:numpad3`/
    `key:numpadadd` 全落进 `unmapped` ⇒ 工程师写了合法绑定、web 上当作没写。本轮补齐锁定/编辑/
    系统键、符号键（`backquote`/`bracketleft`/… → 对应字符）、功能键 `f1..f24`、修饰键与
    小键盘同形键。
  - **有损合并必须可测**：`event.key` 分不出左右修饰键与主/小键盘同形键 ⇒ 合并是唯一可行做法，
    但"绑 `key:shiftleft` 时右 Shift 也会触发"这件事既不报错、也不属于 API 降级清单 ⇒ 改为
    **逐条计数并写进读数**：`aliased=4:shiftleft|numpad3|shiftright|numpadadd`。计数是**精确**的
    （同一份映射里 `key:f1`/`key:capslock`/`key:backquote` 不计入；jhyt 真实工程算出 `aliased=0`）。
  - **测试不动用户工程**：把 `game_api` 整份复制到 `artifacts/scratch/inputmap-aliased/`（副本夹具），
    只在副本的 `project.toml` 里加一段含**有损键 + 新词汇**的 `[input].map` ⇒
    `source=project lua_applied=7 unmapped=0 aliased=4`（修前其中 4 条会 unmapped）。
  - **夹具当场抓出不对称**：绑定确实应用了，但 `input.bindings('move_up')` 读出 **nil** ——
    反向映射 `keyNameToBindingText` 没跟着扩，新名字返回 null 被过滤成空表；后果不止读不出来：
    `action_map_text()` 用同一份反查 ⇒ **往返会把这几个动作整个丢掉**。修法：反查改为扫描
    同一张正向表（简单名优先、有损合并名在后、合并名取**左**变体）。修后
    `bind_move_up=key:shiftleft`、`bind_interact=key:f1`、`map_has_move_up=true`、`roundtrip=true`；
    其中 **`bind_attack=key:shiftleft`**（工程写的是 `right`）让有损合并在**读回**方向也看得见。
  - **探针自身的缺陷也记录**：首跑读出 `zzz=true`，查下去是**探针把 `zzz` 也注入了一遍**再问它
    —— 代码没问题、断言写错了；顺带修正第 73 轮"透传"的表述（透传 = 名字按原样使用，
    **不**推出"未知键名为 false"），补上正确断言 `never_injected=false`。
  - **验收**：夹具 `verify exit=0`，`shiftleft/shiftright/f1/capslock/backquote/numpad3/numpadadd`
    全部 `=true`、`never_injected=false`、四项绑定往返一致；jhyt 回归 `verify exit=0`、
    `missing_api_paths=0`、降级未变、剧本门通过、`aliased=0`（与夹具 `aliased=4` 互为对照）。
  - **限制**：左右修饰键、主/小键盘同形键在 web 上无法区分（要区分得改用 `event.code`，
    那会把整套键名从"字符"改成"物理码"，风险另算）；读回统一给左变体。
- **修好查询入口的物理键名（一个活了很久的静默 bug）+ 前置门加一条显式不变量（WP7-43）**。
  - **bug**：引擎的键位查询收**物理键名**（`input.is_down("keyw")`/`just_pressed("keyp")`/
    `is_down("arrowleft")`），而本侧设备层内部用**裸名**（`w`/`p`/`left`）—— 查询入口却把调用方的
    名字直接丢进裸名表 ⇒ `keys["keyw"]` 恒为 nil ⇒ 这些入口在浏览器里**恒为 false**：不报错、
    不缺 API、不进降级清单。**不是假想**：`examples/game_api/scripts/demo_input.lua` 的
    `KEYS_HELD` 就是物理键名（第 130 行 `input.is_down(key)`、309 行 `just_pressed("keyp")`、
    319/320 行 `is_down("arrowleft"/"arrowright")`）⇒ 那个输入演示面板在 web 上从来没显示过
    按住状态。注入侧此前只修了一半（`__rustlua_virtual_name` 只认 `^key([a-z])$` 与 `cancel`，
    `digit7`/`arrowleft` 全漏）。
  - **修法：一份映射两个方向共用**。宿主里已有的权威映射 `physicalKeyToName`（同时用于解析工程
    文件的 `[input].map`）经新回调 `__rustlua_physical_to_web` 交给 Lua；`__rustlua_virtual_name`
    改成"先问宿主、认不出退回原名"。三条纪律：**认不出要透传**（补丁层旧调用方传裸名，不能误伤）、
    **`cancel` 留在本地**（宿主映射表里没有 `cancel`，它是引擎的动作键名、web 对应 Escape）、
    **查询与注入共用同一个函数**。共替换五处 `[tostring(key):lower()]`；鼠标三处是按钮号，未动。
  - **验收（断言带内建反事实）**：`phys_w=true bare_w=true raw_phys_slot=nil raw_bare_slot=true
    phys_arrow=true cancel=true unknown_passthrough=true just_phys_digit7=true pressed_bare_7=true
    after_release=false released_phys=true` —— 其中 `__rustlua_input.keys['keyw']` 为 **nil**
    是关键反事实：原始表里根本没有这个槽位，**正是这次翻译让它工作**（旧代码读的就是这个 nil）。
    两工程 `verify exit=0`、`missing_api_paths=0`、**降级清单未变**（修的是"有实现但错了"，
    本就不在降级清单里）、剧本门通过。
  - **前置门加一条显式不变量**：除 `node --check` 外，机械扫描"`--` 开头的行里不许有反引号"
    （RED + 行号）。**原本的理由没能复现**：两次专门设计的注入实验（跨行成对、单行内成对包一个
    裸词）都让 `node --check` 失败，连同此前 7 次真实事故 **8/8 全被语法检查拦住** ⇒ 这条扫描的
    定位是**防御性冗余**（把不变量显式写下来，不再依赖"JS 恰好会炸"），代价近零、当前零命中。
  - **限制**：`physicalKeyToName` 认不出 `shiftleft`/功能键等 ⇒ 这些键在 web 上仍按不出边沿
    （要真正覆盖需同时扩展设备层 `keyName()`，列入待办）；端到端只证到**调用语义**层
    （`demo_input` 用的那几种形态现在返回 true），没截到"面板画出按住的键"的 UI 读数。
- **`input` 表补齐到引擎的 31/31：手柄三 API、设置文件、诊断与迁移说明（WP7-42）**。
  - **缺口是量出来的**：引擎 `api/input.rs` 注册 31 个成员，本侧此前 23 个 —— 缺
    `pad_button`/`pad_axis`/`rumble`/`settings_path`/`load_settings`/`save_settings`/
    `diagnostics`/`migration_notes`。全部按引擎契约补齐，**类型与返回形状逐条对齐**：
    没有手柄轮询就返回 `false`/`0.0`（不是 nil）、`rumble` 静默不报错（同引擎"无手柄时静默无效"）、
    `load_settings` 无参给 `(true,'loaded')`（启动时确实已从工程文件应用过）、
    `save_settings` 给 `(false,'read_only_bundle')` —— `read_only_bundle` 是**刻意扩展**的原因词：
    形状与引擎一致，但宁可给一个可判定的新原因，也不谎报 `saved`（谎报会让"改键保存了没有"
    变成查不出的谎）。`diagnostics()` 返回**已登记的降级路径**（这台宿主最真实的诊断信息），
    `migration_notes()` 在 v1 头时给出引擎那句原文。两个"环境事实"（map 版本头、包内设置路径）
    与绑定表**共用一个启动 chunk**，少一次跨边界调用。
  - **发现：实现一个 API 会让降级清单变长，而这可以是正确方向**。同一份 jhyt 代码，干净轮降级
    从第 70 轮的 1 项变成 2 项，多出的 `input.load_settings` 不是回归 —— **游戏自己的启动代码真的
    调它**，以前那个名字是可调用但静默无效的"缺口代理"（既不在缺口清单也不在降级清单里）。
    判断标准不是数字变小，而是**每一项都能对上一条被调用过的路径**：断言轮 `+6` 恰好等于探针
    调的六个（`load_settings`/`load_settings(path)`/`save_settings`/`pad_button`/`pad_axis`/`rumble`），
    game_api 干净 7 → 断言 13 差值同样恰好 6，且没有 `input.last_device`（它从不调）。
  - **降级是调用驱动的**（`diag_before=0`）：把 `diagnostics()` 在探针里前后各调一次，
    调用前为**空表** ⇒ 这张表不是启动预填的清单，而是真的被走到才登记 —— 预填清单会让
    "某条弱实现这局到底有没有被用到"永远分不出来。
  - **验收**：jhyt / game_api 均 `verify exit=0`、`missing_api_paths=0`、剧本门通过；
    断言串给出 `members=31 missing=0`、`settings_path=pack/project.toml`、`load=true:loaded`、
    `load_path=read_only_bundle`、`save=false:read_only_bundle`、`pad_button=boolean:false`、
    `pad_axis=number:0.0`、`rumble=nil`、`notes_v2=0`、`notes_v1=1`（v1 分支靠翻转 map 版本头做
    单元断言、跑完还原）、`diag_before=0`/`diag_after=6`。
  - **顺带修仪器**：`data-rustlua-bindings` 的读数上限 400 太紧，本轮断言串在 `diag=` 处被截断
    （**仪器**丢证据、不是被测对象丢，且已是第二次）⇒ 放宽到 700。
  - **限制**：手柄三 API 是登记在案的弱实现（真机接手柄仍无反应）；`save_settings` 恒 `false`
    ⇒ 运行期改键不落盘；`settings_path()` 给的是包内路径而非文件系统绝对路径。
- **绑定编辑族补齐（`rebind`/`unbind`/`reset_bindings`/`action_map_text`/`set_action_map`）+ `input.bindings` 改为引擎物理键名 + JS 宿主的诊断 Lua 通道 + 验收前置语法门（WP7-41）**。
  - **补的是什么**：第 70 轮把键位权威交给工程的 `project.toml` 之后，本侧能读键位、还改不了它。
    引擎 `api/input.rs:236-295` 的那一族里 `input.bindings` 早就有，缺的是其余五个 —— 而 game_api 的
    `scripts/demo_input.lua` **正在用**它们（`input.rebind("interact","key:digit7")`、每个分支
    `reset_bindings()` 收尾、文本无损回灌）⇒ 契约早就写在工程里。
  - **逐条照抄引擎**：`rebind` 返回 `(ok, reason)` 且是**单一主绑定**（先清空该动作全部旧绑定再绑这
    一条；绑定失败时动作会**空掉** —— 这个怪癖照抄，替引擎「修好」会让两边又不一致）；`unbind` 返回
    bool；`reset_bindings` 回到**默认表**（本侧那张硬编码表与 `ActionMap::with_defaults` 的动作/键位
    一一对应，落点天然正确）；`action_map_text`/`set_action_map` 收 v1/v2 两种版本头、无损往返，
    出错**整体不动**，错误串与引擎同形。
  - **`input.bindings` 的对外名字**从 `key:e`/`key:7` 改成引擎的**物理键名** `key:keye`/`key:digit7`
    （`Binding::to_text` 同形）。名字换算只留一份实现（宿主回调 `__rustlua_binding_text` ↔
    `keyNameToBindingText`），Lua 侧不猜。
  - **我原本的判断被自己的实验证伪了**：我以为 jhyt `ui/hints.lua` 那张「按物理键名索引」的表因此
    一条都命中不了、按键说明会静默变空。直接调工程自己的函数一验：`label_interact=E`、
    `label_move_up=W`、`pad_only_label=J`，而 **`old_format_probe=E`** ⇒ hints 里有 v1 别名兜底，
    旧格式也不会变空 ⇒「用户可见的 bug」不成立，本轮改动的正确定位是**契约对齐**而非修故障。
  - **顺带补上两件基础设施**：①**JS 宿主的诊断 Lua 通道** `?rustlua-lua-do=<lua>`
    （引擎 `-WorldDo` 的对等物；此前 JS 宿主没有任何「送一段 Lua 进去跑」的入口，引擎宿主能验的
    东西在这边只能靠改工程文件）—— 只在 `rustlua-diagnostics=1` 时生效，失败不吞、状态进读数。
    ②**验收脚本的前置语法自检** `node --check plugins\webgl\web-runtime.js`：运行时是一大段嵌在
    JS 模板里的 Lua，「Lua 注释里写反引号」这类事故已重复 6 次（第 60/62/67/70/71 轮）。这道门自己
    也自证过两次（第一版用 `Write-Log` 时它还没定义、且没管 `$ErrorActionPreference="Stop"` ⇒ 报错
    被 trap 拦成通用 abort、真正原因丢失），自证方式是**注入语法错误看它是否拦下**；随后它在真实
    事故里当场生效（第 6 次反引号在打包前被挡下，省掉一个约 7 分钟的打包+CDP 周期）。
  - **验收（先登记期望，逐条命中）**：真实 wasm-Lua 世界里 `rebind=true`、回读 `key:digit7`、
    回退键形状 `key:keyw`、`unbind=true`、`invalid=false:invalid binding: banana`、
    `header=rustlua-actions-v2`、`roundtrip=true`、
    `badversion=false:unsupported action map version: rustlua-actions-v9`、`after_reset=key:e`
    （且坏输入后状态未改动）；两个工程 `verify exit=0`、`missing_api_paths=0`、降级清单不变
    （jhyt 1 / game_api 7）、剧本门仍通过。
  - **这轮踩到的三个坑（`node --check` 全查不出）**：①Lua 里 `'\n'` 没加倍 ⇒ 模板先生效 ⇒ 生成出
    真换行 ⇒ `unfinished string` ⇒ **整页启动失败**（靠日志里 `engine-host-method-compat:361:
    unfinished string` 定位，顺带确认这段 Lua 在 method-compat 分块而非 `luaPrelude`）；
    ②启动闭包里引用了别的闭包的局部名 `search` ⇒ 每次启动都会抛 `ReferenceError`（读代码才发现）；
    ③`data-rustlua-bindings` 原本只在启动期写一次 ⇒ 运行期的 rebind/通道结果永远看不见，改成每帧
    诊断时刷新。
  - **限制**：`rustlua-lua-do` 是诊断专用通道（发布包不认这条参数），能力等价于游戏脚本自身、
    不是安全边界；生成的 Lua 文本**没有**离线语法检查（`node --check` 通过而 Lua 编译失败本轮就
    发生过一次），目前靠「启动即失败」暴露。
- **键位权威交给工程的 `project.toml`：`[input].map` 为准，运行时硬编码表降级为"无手柄回退"；并修好一台抖动门（WP7-40）**。
  - **起点**：第 69 轮查清本侧有**两处键位真相** —— 工程 `project.toml` 的 `[input].map`（jhyt 里是
    36 个动作的内联串）与运行时前奏里**硬编码**的 `bindings` 表。引擎侧键位由 `ActionMap` 决定
    （Lua 只读），本侧不选一处就会"同一个工程在桌面与 web 上是两套键位"。
  - **规则**：工程的 `map` 是**权威**，硬编码表只在"工程没给这个动作的键盘键位"时逐动作回退。
    逐动作是关键 —— jhyt 的 `move_up/down/left/right` 在工程里**只绑手柄轴**（`padaxis:1+`），
    本侧没有手柄轮询，整表覆盖会让移动直接失效。
  - **实现**：解析放 JS（`parseInputMap`，与引擎 `parse_input_section` + `from_text_with_notes`
    同语义：只认 `[input]` 段、同名键取首次、map 是 TOML 基本字符串、首行版本头、条目
    `key:<物理键名>`/`mouse:`/`pad:N`/`padaxis:N±`），启动期编成一小段 Lua 赋值交给前奏内部
    （`local bindings` 之后）的 `__rustlua_apply_project_bindings()` 合并 —— 在别处生成的 chunk
    够不着那个 local。位置同 nav 回填：`lua_boot()` 之后、`lua_execute_main()` 之前。
  - **靠推理发现、没有任何测试会报的保真度缺陷**：引擎的 `key:equal` 认的是**物理键**，按住 Shift
    也命中；第一版只映射成 `=` ⇒ "Shift+等号调音量"会**静默失效**（不报错、不缺 API、只是按了没
    反应）。修正为 `equal → ['=','+']`、`minus → ['-','_']`。
  - **验收（先登记期望，逐条命中）**：jhyt `verify exit=0`、`source=project+fallback`、
    `project_keyed=26 / project_pad_only=10 / project_actions=36 / unmapped=0`
    —— 与第 69 轮手工数出来的数字**独立吻合**（两种实现互证）；`lua_applied=26` 是**Lua 侧**回读
    （证明覆盖进了表，而不是只证明"我们解析了文件"）；剧本门仍通过（`d=0,2 samples=2`）
    —— 它是"逐动作回退生效"的唯一直接证据。game_api 给出状态机另一端：`source=fallback-only
    lua_applied=0 header=none`（它的工程没有 `map`）。两台离线自检另验了未映射 0 项与物理键别名。
  - **诚实的限制**：对 jhyt 这次切换**行为上不可见** —— 第 69 轮的归一化对照表明两处在 26 个键盘
    动作上完全一致（音量键在修正别名后也一致）⇒ 没改变任何一帧画面；它的证据只有来源读数、
    `lua_applied=26`、game_api 的另一种状态与离线自检。**没有**造"工程与硬编码表不同"的真实工程
    做端到端覆盖优先级证明（那要改用户的工程文件）。
  - **顺带修好一台抖动门**：game_api 复验时 `webgl_bundle_tile_layers_empty` 报红
    （`world=scene=34 map=index tilemaps=0/0`），而**同一份产物未重新打包**再跑一次是绿的
    （`world=scene=22 map=main tilemaps=1/1`）⇒ 红灯那次的 `script_result` 是 `index_open scene=main`，
    剧本还停在**启动场景（菜单）**，根本没进玩法场景，是等待器落定时机抖动。修法：观测场景 ==
    页面自报 `startup_scene` 时判**黄跳过**并写明原因，取不到任一项仍保持红灯。离线用红灯那次的
    真实串复核：`map=index`→跳过、`map=main`→不进该分支、**构造的 `map=manor`（真玩法场景却 0 层）
    →仍红灯**（守卫不会把"进了玩法场景却没有瓦片层"放过去）。
  - **读数通道**：验收脚本新增 `key_bindings:` 一行并进 `bundle-summary-*.json` 的 `bindingsInfo`
    字段 —— 键位差异不崩、不报缺 API，只会"按了没反应"，是最难自查的一类，值得留可断言的读数。
- **输入三缺口清零：`input.last_device` / `input.nav_settings` / `input.set_nav_settings`；`nav_settings` 是**真读** `pack/project.toml` 而非记默认值；并查清键位权威的真实分歧（WP7-39）**。
  - **起点**：第 68 轮给 jhyt 复验时首次报出 `missing_api_paths=3: input.last_device,
    input.nav_settings, input.set_nav_settings`（它的产物此前是旧的，所以这三个缺口一直没被看见）。
  - **照抄引擎契约，不发明**：`last_device()` 只返回 `"keyboard"`/`"gamepad"`（键鼠同属 keyboard，
    由真实边沿记录，`rustlua-input/src/input.rs:553-576`）；`nav_settings([path])` 读
    `<project>/project.toml` 的 `[input]` 段、路径解析不出来返回 nil（`api/input.rs:431-453`）；
    `set_nav_settings` 按 `NavSettings::new` 钳制（阈值非有限→0.5、钳 0..1；延迟≤5000ms；
    重复间隔钳 16..=5000ms）并**回返钳制后的三元组**。解析只认 `[input]` 段、同名键取首次出现、
    毫秒键优先于旧帧键（60Hz 换算）、段外同名键不生效。
  - **`nav_settings` 为什么能真读**：打包产物里本来就有 `pack/project.toml`，而 JS 世界启动时
    **已经 fetch 了它** ⇒ 不需要新增取文件逻辑：启动时按上面的规则解析好、写进 Lua 全局，
    脚本调用只是读它。显式传 `path` 时返回 nil（浏览器没有同步文件系统，与"路径解析不出来"同语义）。
  - **一个 `node --check` 查不出来的错**：`lua_set_number` 必须落在 `bridge.lua_boot()` **之后**
    （Lua 状态此时才存在）且 `lua_execute_main()` **之前**（main.lua 里 `apply_nav_settings()`
    立刻要读）。第一版写在 boot 之前，靠读顺序才发现。实测顺序：boot → 前奏 → 回填 → execute_main。
  - **验收（先登记期望再实测，逐条命中）**：`missing_api_paths=0`；降级清单**只多一项**
    `input.last_device` —— 本侧设备层只有键鼠与触摸、没有手柄轮询，真值永远是 `"keyboard"`，
    假装能报 gamepad 会让 UI 提示在手柄接入时永远不切。`nav_settings`/`set_nav_settings`
    **不进降级**：给定无手柄模拟量这一结构事实，它们与引擎行为一致（引擎也只在模拟量绑定上连发）。
    解析器另做了离线自检（真实文件 / 段外键+脏值 / 毫秒与帧键并存 / 无段），四组全部与引擎规则一致。
  - **顺手查清两件事**：①**键位权威**：jhyt 把 36 个动作写在 `project.toml` 的 `map` 内联串里，
    而 JS 世界用的是前奏里**硬编码**的 `bindings` 表 —— 归一化对照后：两侧动作集一致（各 36）、
    只有音量键 JS 收超集（良性），但有 **10 个动作在工程文件里只绑手柄**（`move_*`/`attack`/
    `interact`/`inventory`/`pause`/`respawn`/`skill`）而 JS 自造了键盘替代 ⇒ 本侧剧本门（发键盘
    `w/s`）**依赖**这套自造键位；真接入手柄轮询时必须改成"以 `map` 为准、键盘表作为无手柄回退"。
    ②jhyt 剧本门的位移量是**抖动**：同一份产物连跑分别得 `d=0,2` / `d=0,5`，而 round68 是
    `d=0,19`，与 harness 的 `samples` 同步变化 ⇒ 不能把 `19 → 2` 当成改动引起的回归。
    **注意**：这次对照的第一版脚本有 bug（把表声明行当成一个动作、漏了 `move_up`），报出 27 个
    "缺失"的**假警报**；修好工具后才是上面这份可信结论。
- **本侧 `camera2d`：脚本读到的相机 = 画出来的相机（原来读到的是引擎空场景的 `(0,0)/1.0`）；渲染侧成为唯一真源，每帧用 `lua_set_number` 回填（WP7-38）**。
  - **上一轮读数照出来的静默差异**：画面由 JS 世界渲染真实场景，而脚本问 `camera2d.get()` 拿到的是
    **引擎侧**（空世界）的相机 ⇒ 永远 `center=(0.0,0.0) zoom=1.0`。不崩、不报错、也不进缺口清单
    （剧本用 `safe_call` 包着）⇒ 典型的"看起来没事"。要害不是缺 API，而是**两个世界各说各话**：
    点选、HUD 定位、`world_to_screen` 全部基于错的数。
  - **口径由引擎注释写死**（`api/camera.rs:240`）：请求写进状态、下一帧消费；`get`/`get_zoom` 读
    **宿主每帧回填的真实值**。实现照此：相机由 `computeCamera` 一份解算产出（渲染侧唯一真源），
    脚本请求落成 `camera2dState`；每帧在 `update` **之前**用 `lua_set_number` 回填三个数
    （不编译源码、不跨界传 JSON）；`drawScene` 经**本帧新鲜闸门**复用同一份，不走剧本的直绘帧
    照旧现算 ⇒ 与改动前逐字一致。
  - **表面覆盖 10 个成员**：`set`/`get`/`set_zoom`/`get_zoom`/`world_to_screen`/`screen_to_world`/
    `mouse_world`/`shake`/`follow`/`unfollow`，错误文案与引擎逐条一致。`world_to_screen` 与
    `drawScene` 的 `toCanvas` 共用同一份算术（1280×720 设计空间）——写两遍就会分叉。操作行用新字母
    **`E`**（插入前核对了整套 if/else 链，`E` 未被占用）。
  - **验收数字先预算后实测、逐位命中**：8×8 格 × 64px 地图、半视口 640/360，两轴都 `min > max`
    ⇒ 夹取退化成取中值 = `(256, 256)`。实测 `camera2d center=(256.0,256.0) zoom=1.0`；
    本轮之前是 `(0.0,0.0)` ⇒ 差异关闭、且不是巧合。
  - **行为变更做了复验**：`camera2d.follow` 现在真的接管取景 ⇒ jhyt 重打包后跑剧本门：
    `verify exit=0`、`[drive] done scene=manor before=528,112 after=528,131 d=0,19 samples=3`、
    `world: … player=JianghuPlayer@528,131 tilemaps=4/4` ⇒ 没有破坏玩法工程。
    顺带**首次**报出 jhyt 的 3 个真实缺口：`input.last_device`、`input.nav_settings`、
    `input.set_nav_settings`（下一轮）。
  - **保真度细节**：`unfollow()` 与 `follow(nil)` 在引擎里是同一件事（`api/camera.rs:359` 的
    `None => None`）⇒ 本侧两者都把相机**钉在当前帧位置**，而不是回落到默认启发式（否则"解除跟随"
    看起来毫无效果）。**诚实登记的差异**：`dead_zone`/`smoothing` 收下但不改变取景；`shake` 用
    "剩余时间线性衰减 + 正弦相位"的包络，不与引擎随机序列逐位相同；`set` 的中心是持久的
    （引擎是"请求—下一帧消费"，可观察差别只出现在"只调用一次 set 之后"）。
- **剧本自己说的话变成可断言的读数：`script_result` / `script_ui_texts` 两条通道，一次运行读完整个剧本；并修掉一条让所有 DOM 中文证据乱码的读回路径（WP7-37）**。
  - **要解决的盲区**：剧本把「每个 API 模块这一帧干了什么」聚合成一行文本用 `ui.text_at` **画在 canvas 上**
    ——DOM 里没有、宿主 API 也查不到 ⇒ "剧本跑到第几节、哪一节报错"只能靠**崩在哪一行**反推
    （第 62~66 轮就是这么一轮一个缺口地往前挪）。现在补两条通道，**都不需要改工程代码**：
    `data-rustlua-ui-texts`（本侧新增导出 `__rustlua_export_ui_texts()`，把脚本画的**具名**文本原样导出）
    与 `data-rustlua-script-result`（剧本 `update()` 的返回值——`demo_main` 返回的正是"全部小节"的聚合串）。
  - **读数一到位，第 61~66 轮的成果立刻变成可引用的数字**（过去只能靠"没崩"间接推断）：`all=0xFFFFFFFF`
    证明第 64 轮那个被 PowerShell 插值吞掉的 `${ALL_LAYERS}` 确实修对了；`physics overlap=1 raycast=2
    positions_in_radius=9` 三个数字各不相同 ⇒ 第 65 轮的**解析法几何查询真的命中**，不是退化实现；
    `velocity (0.0,0.0) -> (90.0,0.0) ccd false -> true` ⇒ 读写闭环；`world_text.detach -> true`；
    `contacts=0 / events=none` ⇒ 与 `physics.events` 被登记为**降级**一致（恒空表，不是假装有接触）。
    读数之间**互相印证**。
  - **顺手暴露一个新的静默差异**：`camera2d center=(0.0,0.0) zoom=1.0` —— 画面是 JS 世界渲染的真实场景，
    而脚本问 `camera2d.get()` 拿到的是**引擎侧**（空场景）的相机。不崩、不报错、也不进缺口清单
    （剧本用 `safe_call` 包着它）⇒ 典型的"看起来没事"。已定位为下一步。
  - **修掉一条让所有 DOM 中文证据乱码的读回路径**：PS 5.1 的 `Get-Content` 默认按 ANSI（cp936）解码，
    而 DOM dump 是 UTF-8 ⇒ `返回 nil：实体 2 没有 Sprite 组件` 被读成 `杩斿洖 nil锛氬疄浣?2 …`。
    乱码不是"显示问题"：读数无法引用、无法 grep、无法与源码对照，等于把刚修好的通道又堵回去。
    已显式 `-Encoding UTF8`（先用同一文件按 UTF-8/cp936 各读一次对照，确认文件本身没问题，再收敛到读回路径）。
  - **一条证据稳定性教训**：`script_result` 第一版写**当帧**返回值 ⇒ 同一产物两次运行一次有值、一次**空**
    （部分帧 `update` 合法返回 nil，而 CDP 只抓落定后的**某一帧**）。改成**只记最后一次非空值**后稳定。
    验收读数不能依赖"恰好抓到哪一帧"，否则红绿会漂。
- **「降级」变成可断言的事实：与「缺口」分开列、同一条 DOM+console 通道；`camera` 静默空实现改为引擎同语义；并修掉一个会掩盖真正错误的 catch（WP7-36）**。
  - **缺口 vs 降级必须分开**：缺口 = 宿主**没有**这个方法（陷阱代理顶成"可调用且返回一张表"，症状离原因很远）；
    降级 = 我们**有**实现但语义弱于引擎（只记录开关、恒空表…）。前者要补实现，后者要么补真实现、
    要么在设计上接受并**保持可断言**。两者各写一个 DOM 属性 + `console.log`，并共用同一条自证纪律：
    初值 `null` ⇒ **第一次 `markBoot` 必定打一行**（哪怕 0 项），"没有降级"与"探针没跑"因此不再同形。
  - **挂了 7 个降级标记**（都是已明写的语义弱化）：`particles.burst`/`particles.count`/`physics.events`/
    `physics.set_ccd`/`physics.get_ccd`/`physics.debug_draw`/`animation.events`。验收读数
    `degraded_api_paths=7: …` + `missing_api_paths=0` + `boot=running` + `verify exit=0`。
    **这份清单本身就是证据**：证明剧本第 8/9/10 节与 `animation.events` 真的被执行到（含
    `physics.debug_draw` ⇒ 第 10 节确实跑过），第 65 轮"0 缺口"由此获得独立旁证。
  - **`camera` 不再是静默空实现**（原来 `set/set_active` 返回 true 却什么都不做）。逐条对齐
    `api/camera.rs`：`get_active()` 返回实体号或 nil（**引擎注册名就是 `get_active`**，我第一版误写成
    `active`，读引擎源码时抓出）；`set_active(id)` 要求实体真有 `Camera` 组件，否则与引擎同一句
    `entity has no camera`；`get`/`set` 是透视 payload 读写、只认 `camera3d`，本侧 2D 世界与引擎
    **同一条报错**。**行字母差点造成静默回归**：新指令行初版用 `C`，而 `C` 早被"格子值"占用，
    `if/else if` 链会遮蔽它、让瓦片格更新静默失效 ⇒ 查清已占用字母后改用 `K`。
  - **修掉一个会掩盖真正错误的 catch**：`const hostGlobals` 声明在 `try` 块内，`catch` 里却直接引用它 ⇒
    异常路径二次抛 `hostGlobals is not defined`，把**真正的**错误盖掉（实测：页面只报这一句，原错其实是
    前奏里一个 Lua 字符串没闭合）。现在探针失败退化成哨兵值 `__probe_error__`（`typeof` 守卫），
    **原错照抛**。异常路径的第一原则是让原错露出来 —— 这条修复立刻换来了根因的一次定位。
  - **同一轮记录的写错现场**（都靠读回抓到）：JS 模板字面量里的 `'\n'` 会被 JS 先吃掉 ⇒ Lua 收到真换行
    ⇒ `unfinished string`（正确写法 `'\\n'`，与既有两处一致）；PowerShell 双引号串里的反引号转义
    把注释劈成两行、ASCII 双引号截断字符串；here-string 终止符未独占一行 ⇒ 后续编辑的**源代码文本**
    被写进 Lua 前奏（已逐行清除，残留 0、7 个标记齐备）。
- **JS 世界的 `physics` 缺口一次清零：三条做真几何、四条做"诚实降级"，并用差分证据证明调用真的发生过（WP7-35）**。
  - **能真做的就真做**：本侧没有物理世界，但 `Collider` 组件字段（`shape`/`ox,oy`/`hx,hy`，与引擎
    `ColliderDesc::from_component_str` 同源）足够做几何真判——新增纯 Lua 助手（圆vs圆、圆vs轴对齐矩形、
    解析法射线求交 + 矩形 slab 法，返回最近正向距离）。其上实现 `physics.overlap`（sensor **参与**查询，
    与引擎一致；半径非有限/负与引擎同一句报错；`opts.groups` 本侧无碰撞组概念 ⇒ **忽略并记录**）与
    `physics.raycast`（返回**最近**命中实体，未命中 **nil**，方向先归一化）。
    注释里写清它与既有 `positions_in_radius` **不是**同一件事（那个只看 Transform 位置）。
  - **做不了的绝不假装**：`get_velocity` **真做**（与引擎同形：`[1]/[2]` 与 `x/y` 都在）；
    `set_ccd`/`get_ccd` **只记录开关**（本侧无刚体/无 CCD，明写不改变碰撞行为）；`events()` **恒空表**
    （形状一致、不返回 nil）；`debug_draw` **记录开关 + 一次性告警**（本侧没有物理调试层）。
  - **验证用的是差分，不是"看起来没事"**：缺口清单是**增量**读数，"0 项"也可能是"根本没访问"。
    证据链：`demo_sweep`（`demo_main.lua:513-534`）在**同一帧里依次调用全部小节**、
    由 `update` 每帧触发 ⇒ 第 64 轮同一门下这 8 条被**记录**（证明调用发生过）⇒ 第 65 轮同一帧路径
    **0 条**且无运行期报错（证明已被真实现接管）。读数：`宿主缺失 API 0 项`、`boot=running`、
    `scene_verify: done`、`drawn samples: 1931`、`tilemaps=1/1`、`verify exit=0`。
  - **新增一条仪器局限（必须记住）**：一次性告警走 `log.warn` ⇒ `console.warn`，而 harness **只抓
    `console.log`/`console.error`** ⇒ 日志里一条都没有 ⇒ **告警不能当证据**（"没出现"既可能是没触发、
    也可能是没抓）。下一步要么改走被捕获的通道，要么写进 DOM 读数，让"降级"变成**可断言**的事实。
  - **下一发现**：前奏里 `camera.set/set_active` 是一对**静默空实现**（返回 true 却什么都不做），
    正在剧本最后一节 `demo_camera` 的路径上 ⇒ 下一轮按同一口径处理。
- **`visibility` 纳入 JS 世界（含按层过滤），致命错误消失、剧本首次跑到尾；同时修掉一条假红门（WP7-34）**。
  - **逐条对齐引擎语义**（`api/entity.rs:304`）：`set` 写 u32 并**返回写入值**、`get` 对**没有**组件的实体
    返回 `ALL_LAYERS`（**不是 0**）、`all` = `u32::MAX`；越界/非整数与引擎同样报错。
  - **渲染侧真按层过滤**（引擎是"不产顶点"，不是画成透明）：判定照 `visibility.rs:117` 的
    `entity_visible_in`——有组件 ⇒ `layers & view_mask != 0`、无组件 ⇒ `view_mask != 0`；
    在瓦片层、世界实体归类、世界文字 pass 三处生效；屏幕空间 UI 不动。遮罩**默认 = 引擎默认**
    `ALL_LAYERS`，本侧暂无设置入口（差异已明写，接入只需改一个变量）。
  - **读回复核抓到一个真 bug**：注入 `__rustlua_all_layers` 时 `${ALL_LAYERS}` 在 PowerShell 双引号串里
    被当变量插值成空 ⇒ 盘上是 `= `（`all()` 会返回 nil，剧本再拿它 `string.format('%X')` 就是一条
    离原因很远的报错）。当场修成 `= 4294967295`。教训：含 `$`/反引号的注入一律单引号字面量或显式转义，且必须读回。
  - **验证**：同一门下从 `boot=failed`（`demo_main.lua:356` `invalid entity id`）变为
    **`boot=running` + `scene_verify: done` + `drawn samples: 1926`**（这一系列里第一次跑到尾）。
  - **缺口清单前进到第二类**：`宿主缺失 API 8 项：physics.debug_draw / get_ccd / get_velocity /
    get_velocity.x / get_velocity.y / overlap / raycast / set_ccd`——证明"记录式缺口"不会崩帧
    （剧本跑完），且清单能给出**嵌套路径**（记录点在取值而非调用）。
  - **修掉一条假红门（验证诚实性）**：`-Drive` 的剧本是"按确认 → 进玩法地图 → 位移非零"，只有玩法工程
    写得出来；`examples/game_api` 是按键触发的 API 展示、没有玩法地图 ⇒ 强加 JS 宿主时它必然红，
    **而红的原因不是运行时坏了**（前几轮把它读成"还没修好"是误读）。现在按页面自报身份显式跳过并写明原因，
    复验 `verify exit=0`；判据仍是启发式（更干净的做法是产物自报"已实现驱动剧本"，已记入计划）。
- **把 `world_text` 纳入 JS 世界（含世界空间渲染通道），并看清这条路径的战略地位（WP7-33）**。
  - **做法**（`plugins/webgl/web-runtime.js`）：`applyComponents` 按引擎结构
    `WorldText { text: UiText, offset, centered }` 解析出 `entity.worldText`，**组件不在时删掉它**
    （`detach` 必须真的停止绘制）；`drawScene` 加**世界文字 pass**（用现成的 `toCanvas` 把
    "实体世界位置 + offset"映到设计空间，`centered` 决定对齐，画在屏幕空间 UI 之前，与引擎层序一致）；
    op 协议加一行 `W`（空串 = 摘掉），与 `S`/`Q` 同构；前奏新增 `world_text.attach/detach`
    （`attach` 恒返回 true、`detach` 返回**是否真的移除了**，没挂过就不发 op）。
  - **验证（同一条命令前后对照）**：失败点从 `EntityId(0:g10)`（`demo_main.lua:330` `world_text.detach`
    落到引擎）**前进到** `EntityId(0:g11)`（`:356` `visibility.get`）⇒ `world_text` 通了；
    同时 `宿主缺失 API 0 项`、`drawn samples: 1756`、`tilemaps=1/1`。
  - **下一个缺口的契约已读清（下一轮可直接做）**：`visibility` 有组件 ⇒ `layers & view_mask != 0`、
    无组件 ⇒ `view_mask != 0`；遮罩是宿主 `view_world_mask`（默认 `ALL_LAYERS`）。JS 侧还**没有**
    任何视图遮罩概念 ⇒ 要加"按层跳过绘制"（引擎是"不产顶点"，不是画成透明）；默认遮罩下与引擎
    一致，差异只在有人改遮罩时出现，而 JS 宿主暂时没有这个入口——须明写。
  - **战略判断（本轮真正的收获）**：这些缺口**全部属于 JS 世界（遗留兼容模式）**——页面默认路径已经是
    `legacy JS world skipped (engine host owns the frame)`，引擎宿主自己拥有整帧、有的是真 API；
    驱动 JS 世界的只有 `-Drive` 这一道验收门。⇒ 继续把 JS 世界的 API 逐个追平引擎**收益递减**，
    更该做的是让验收门不再依赖遗留世界（或把 JS 世界的 API 面收敛成"诚实的子集"并只报只读观测）。
  - **已知地雷（记录未修）**：`mesh` / `light` 在 JS 世界同样缺失，而
    `examples/game_api/scripts/demo_unified.lua` 明确调用它们 ⇒ 同一类崩溃；本轮路径不触发。
  - **渲染是近似（明写）**：引擎用 SDF/位图字形与 `scale`，本侧用矢量字形；`font_size` 缺省时本侧取 24
    （与 UI Text 同默认）。世界锚定/offset/centered/颜色这些可断言部分按引擎语义实现。
- **把"一次跑一个缺口"变成"一次跑一张清单"：接出宿主缺失 API 读数，并据此一次补齐 5 个缺失方法（WP7-32）**。
  - **接出已有机制**：运行时本来就有 `__rustlua_record_missing` + `__rustlua_export_missing_api()`
    （陷阱代理每碰到一个缺失方法就记一笔），但**定义了没有任何调用点**。现在 `markBoot()` 每次状态变化
    都把清单写进 `data-rustlua-missing-api` 并用 `console.log` 打一行，验收脚本读同一属性打印
    `missing_api_paths=N: …`（0 绿、非 0 黄）——缺口从"撞一个修一个"变成一次列清。
  - **第一次翻车翻在自己的仪器上（已留住）**：第一版用 `bridge.lua_do_string(...)` 读清单，而它
    返回 `Result<(), JsValue>`（`lua_bridge.rs:446`）**拿不到 Lua 返回值** ⇒ 永远读到空表，
    日志出现 `missing_api_paths=0`，**看着像"没有缺口"**，差点被当成"`particles.count` 确实存在"的
    证据（并用它否定一条其实正确的推断）。修法：走 `lua_call_global`（返回 `Option<String>`）；
    读失败给**哨兵值**而非空表；初值用 `null` 保证第一次必定留读数。
  - **修好后的读数**：`宿主缺失 API 5 项：animation.clips, animation.events, animation.time,
    particles.count, ui.scroll_open` ⇒ `:316` 的 `bad argument #5 … got table` 因果闭合：
    缺失方法被陷阱代理顶成"可调用且返回一张表"，于是 `%d` 收到表。
  - **一次补齐 5 个**（全部按引擎契约，差异明写不静默）：`animation.clips`（1 基名数组，本侧排序后给出确定结果）、
    `animation.events`（恒空表：本侧不收集帧事件，形状与引擎一致）、`animation.time`、
    `ui.scroll_open`（按本侧 `scroll_state` 结构登记，与已有 scroll_* 同一套）、
    `particles.count`（恒 0 + **一次性告警**：本侧没有粒子模拟）。
  - **失败点再前进，暴露第二类缺口**：`invalid entity id: EntityId(0:g10)`，
    traceback 指到 `engine-host-prelude:464 in field 'detach'` ← `demo_main.lua:330 demo_world_text`
    ⇒ 这一类**不是**缺失方法（清单 0 项），而是"前奏把 `world_text` 转发给了引擎"（第 60 轮那层
    id 翻译壳的映射为空）。与 `quad` 同一个病，修法同 `quad`：把 `world_text`/`mesh`/`light` 纳入本侧；
    JS 渲染侧 `applyComponents` 目前只认 9 类组件，真要画出来还得加渲染通道（下一轮）。
  - **顺手查清第 61 轮的一个边界（诚实记录）**：`components.Quad` 全文件只有我加的两处
    ⇒ JS 渲染侧**没有 quad 绘制通道** ⇒ `quad.get/set` 是 API 层正确（这正是 `:215` 解开的原因），
    但 `quad.set` 在 JS 世界**不改变画面**，不能当成"已完整接入"。
- **把 `quad` 纳入 JS 世界：JS 宿主形态下那个卡了三轮的阻塞点修好了，剧本前进约 100 行（WP7-31）**。
  - **做法**（全部在 `plugins/webgl/web-runtime.js`，两平台同一份）：场景绑定多给一张
    Quad 组件串镜像（`__rustlua_quads` + `__rustlua_white_texture`，贴图哨兵只留一个来源）；
    前奏新增 `quad.get` / `quad.set`，规则**逐条对齐引擎**（默认 size=64,64 / pivot=0.5,0.5 /
    color=1,1,1,1 / z=0、缺省贴图给 `__white__`、size 必填、未知字段与非法值一律报错）；
    新增 `Q` 行走既有 op 协议交给渲染侧**同一份** `applyComponents`（与 `sprite.set` 的 `S` 行同构，
    缺省值不在 Lua 侧另立一套）。
  - **验证（同一条命令，改动前后对照）**：失败点从 `demo_main.lua:215`（`demo_quad`，
    `invalid entity id: EntityId(0:g13)`）**推进到** `demo_main.lua:316`（`demo_particles`）——
    日志里 JS 世界切到 `main`、随后进到粒子那一节才失败 ⇒ `quad.*` 这条链在 JS 宿主形态下已经可用。
  - **本轮自己抓到的两个错**：①批量替换把 `Q` 行分支插进了 **UI 行解析器**（那里 `'X'` 是文本、
    且没有 `entity`/`applyComponents`）⇒ 一跑就 `ReferenceError`；**读回插入结果时发现**并撤掉
    （教训：同一锚点在 5 千行文件里可能有两处，替换后必须读回）；②同类插入的反引号会提前闭合
    JS 模板字符串，在浏览器里表现为"整页不启动"。
  - **新阻塞点与其"被排除的解释"**：`demo_main.lua:316` 报
    `bad argument #5 to 'format' (number expected, got table)`，即 `particles.count()` 交出了表
    （引擎侧返回数字）。四种解释都被查证排除：桩里没有 `count`（那应报"调用 nil"）、工程无第二份定义、
    首屏注入器无命中、产物里 `particles` 只出现一次。⇒ 静态模型在某处是错的，
    按纪律**不靠猜补一个 `count`**，下一轮先动态打出 `type(particles)`、`type(particles.count)`
    与返回值类型再决定修法。
- **查出 webgl/微信息 JS 宿主"两个 id 空间"这条根因（WP7-30），并证伪了自己第一版的接法**。
  - **根因（源码级、逐位对上）**：JS 宿主的场景解析把 **`.scene` 文件里的作者 id** 当实体 id
    （`web-runtime.js:70` 取 `parts[1]`，`sceneBindings` 直接写进 `__rustlua_names`），
    而引擎把 Lua 侧的 i64 按 `index << 32 | generation` 解包
    （`rustlua-scripting/src/script/values.rs:5/:67-68`）。于是场景里 `entity\t13\tScaleBar`
    的作者 id `13` 解成 `index = 13 >> 32 = 0`、`generation = 13` ⇒ 报错正是
    `invalid entity id: EntityId(0:g13)`。JS 宿主覆盖了 `entity`/`sprite`/`transform`/`tilemap`/
    `animation`（所以这些调用到不了引擎），**没**覆盖 `quad`/`mesh`/`light`/`world_text`
    （`renderable.rs:167/:218/:220/:295`）⇒ 两个 id 空间在**第一个 `quad.*` 调用**处撞车，
    正好是 `demo_main.lua:215`。
  - **本轮落地的能力（有测试）**：引擎侧新增 `LuaRuntime::entity_handle_lua(name) -> Option<i64>`
    与 `#[wasm_bindgen] lua_entity_handle(name) -> i64`（找不到给 0，0 在引擎里本就不是合法句柄）；
    JS 侧场景绑定同时产出 `__rustlua_engine_ids` 并给那四张未覆盖的 API 加了元表 id 翻译层；
    两处门面（`index.html` 的 ESM 导入与桥对象、微信适配器白名单）同步，对用户正在编辑的适配器
    **只做加法**。回归面：`rustlua-scripting` 单测 **118/118**、`node --check` 两个 JS 文件通过。
  - **被测量证伪的部分（必须留住）**：预期"翻译层能让失败点越过 `:215`"——实测**报错逐字未动**
    （连续三跑同一条，而产物确实更新：wasm 导出数 134 → 135）。加一行一次性诊断后读数
    `引擎句柄映射 0/34（场景 index；出口=有）` ⇒ **桥里那个 Lua VM 根本没有引擎侧场景**
    （JS 世界是 JS 按 `.scene` 自己建的，引擎的 `entity_state.names` 在该 VM 里从未填充）
    ⇒ 该形态下**不存在可翻译的引擎句柄**，翻译层必然退化为空操作 ⇒ 这条路是死路。
  - **由此得到的定论**：JS 世界在该形态下**就是权威**（覆盖 9 张 API 表里的 5 张、渲染也由它驱动）
    ⇒ 正确修法是**把没覆盖的那几张也纳入 JS 世界**（`quad` 优先），而不是喂它引擎句柄；
    "退掉 JS 世界改用引擎"（WP8）是终局方向，但那是大改，不能当这一处的修法。
- **`entity.find` 只交活着的句柄（WP7-29）**。此前它直接返回名字表里的句柄、**不校验死活**：
  名字表里留着一个已经死掉的句柄是可能的，于是调用方拿到它、直到真正使用时才吃一句
  `invalid entity id`（第 58 轮 webgl 产物上实测到的那条报错就是这个形状）。改法与同文件
  `is_alive` 的既有语义（"不合法/不在了"就是 `nil`）、以及 `persist_id` 注册路径（那里已经先问
  `world.is_alive`）对齐。回归测试**绕过** `entity.destroy`（它会顺手清名字表）直接摘掉实体，
  复刻"名字表留着死句柄"的状态并断言 `find` 交 `nil`；**自证伪**：把守卫临时改回不校验，
  这条测试立刻红在指定断言上 ⇒ 测试确实钉住了这次改动。回归面：`rustlua-scripting` 单测
  117/117、`engine` 单测 179/179。
- **一条被证伪的归因（同 WP7-29，值得单独记）**：我原以为上面这条守卫就能修掉浏览器里
  `game_api` 的 `-Drive` 红（`demo_main.lua:215` 的死句柄）。重打包后**失败点逐字未动** ——
  而 wasm 体积确实变了（改动进了产物），所以不是"没编译进去"。查下去才发现真实层次：
  `plugins/webgl/web-runtime.js` 的 Lua 前奏**把引擎的 `entity` 表整个替换掉了**，
  用自己的一套表（名字/存活/persist/transform + 自己的 id 分配器）实现实体系统 ⇒
  这条路径上引擎的 `find` 根本不会被调用；而且两个 id 空间在混用（报错里的 `EntityId(0:g13)`
  是引擎格式句柄，前奏的 `is_alive` 问的却是它自己那张表）。**本轮刻意不顺手改前奏**：
  实测证据还不足以确定该问谁——表里若存的是前奏自己发明的 id，拿它们去问引擎会把**合法 id 判死**，
  那比现状更坏。下一轮先instrument（把 id、前奏的死活判断、引擎的死活判断一起打出来），再决定
  是"对引擎来源的 id 问引擎"还是"整层退掉改用引擎的"（与 WP8 同向）。
- **把「没有写者的门」与「真的坏了」分开：WebGL 验证改读页面自报的宿主形态，并把产物身份钉到页面自报（WP7-28）**。
  - **问题**：webgl 页面按既有设计**默认不启动 JS 世界**（引擎宿主独占这一帧），而场景切换门 / 剧本门 /
    世界读数 / 像素诊断**全部由 JS 宿主写**。于是在默认路径（也就是玩家实际拿到的形态）上，那盏门
    永远是红的——真正的回归反而会被这盏常亮红灯盖住。而页面其实**早就自报了**这件事
    （`[host] legacy JS world skipped (engine host owns the frame)`）。
  - **改法**：验证脚本读页面自报——未请求 JS 宿主时把 JS 宿主专属的门显式标为「跳过（没有写者）」；
    **请求了** JS 宿主却仍自报 skipped ⇒ 红灯（那是宿主开关真的坏了）。同时补一条**更强的引擎侧断言**：
    页面自报的产物名必须与待验产物 `build-info.txt` 的 `publish_name` 一致、且必须报出
    `startup_scene`——这条把「浏览器真在跑待验的那份产物」也钉住了。
  - **验证**：默认路径 exit=0（`webgl_bundle_ok`，并打印 `page identity ok`）；显式请求 JS 宿主时
    exit=0 且场景切换门**照旧真验**（`scene_verify: done`）⇒ 只把没有写者的门改判，没有放松任何判据。
- **修掉 `examples/game_api` 演示脚本的一处真实缺陷：`sprite.get` 的返回值未判空（`demo_main.lua`）**。
  `need(PLAYER)` 只保证实体存在、不保证它带 Sprite 组件（引擎侧读的是 `components["Sprite"]`，缺了就是
  nil），原写法直接 `before.texture` ⇒ 整条 update 抛错，一次 webgl 驱动验证因此得到一个与被验改动
  无关的假红（读数上只剩一句 `index a nil value`）。改法遵循该文件自身的既有约定（缺实体时也只写原因、
  不崩）：把「这个场景本来就没有精灵」与「脚本坏了」在读数上分开。
- **修掉"验证验的是另一个产物"这条静默错验：WebGL 产物验证现在必须自证服务根（WP7-27）**。
  - **实测经过**：`verify-webgl-bundle.ps1 -BundleRoot …game_jhyt\builds\webgl` 判红，日志首行也写着
    jhyt 的路径，看起来就是"jhyt 的 webgl 产物红了"。但同一份日志里 `project bytes=329` 与
    `game_jhyt` 产物的配置体积不符，DOM 快照里出现的又是**另一个工程**的脚本名 ⇒ 浏览器当时被
    **上一次运行残留的静态服务**（占着脚本默认端口）应答了；这次为正确产物起的服务绑定失败且失败被吞，
    于是全部读数说的都是另一个产物。**只看红绿就会得出反向结论。**
  - **封堵**：起服务前端口必须空闲（占用则报出占用者 PID + 命令行）；起服务后等就绪、进程已退出或
    始终不应答即抛（附服务日志尾部，取代"睡 1.2 秒然后假设它在跑"）；就绪后取金丝雀文件比对
    **HTTP 字节与本地待验产物逐字节同 SHA256** ⇒ "服务根 == 待验产物根"由证据自证。
    三道闸只加断言，不放宽任何既有判据。
  - **方法教训**：验证工具必须自证"它验的是哪个产物"，否则端口/缓存/残留进程任一处复用都会让
    红绿指向别处。
- **webgl 侧不带 `-SkipBuild` 重打包：又是一个陈旧产物（WP7-13 收尾）**。旧 webgl 产物里的 wasm
  体积约为本次全新构建的两倍，与微信侧同一形态；重打包后 `boot=ok`、`detail=game_jhyt` 且在
  **身份自证过的**产物上通过，即 WP7-13 在 webgl 侧最有风险的一环（页面新增的 ESM 具名导入）
  已被证明可用。
- **把固定步时钟从 `web-runtime.js` 搬进宿主 wasm，让定步长权威只剩一份（WP7-13）**。
  - **为什么**：同一份产物里此前有**两套定步长实现**——引擎 GPU 独跑路径（webgl）用
    `rustlua_infra::FixedTimeStep`，而**微信侧真正跑的是 JS 宿主**（`game.js` → `win.rustluaRunLuaGame`
    = `web-runtime.js`，本轮读产物入口确认），它自持一份累积器 + 落帧容差 + 追帧上限 + 丢帧计数。
    两套实现只要有一处调整，两个平台的追帧/丢帧口径就会分叉，而分叉在读数上只表现为
    "某个平台的动画快一点/慢一点"，极难定位。
  - **怎么搬**：`FixedTimeStep` 新增**可选**落帧容差（默认 0 ⇒ 原生语义逐位不变，既有测试原样通过）
    与三个只读访问器；wasm 侧新增"这一帧跑几步 / 累计丢帧 / 累积器余量 / 重置时钟"四个导出，
    常量与 JS 旧实现逐条对齐；JS 每帧只问步数，丢帧读数也读宿主的；宿主没有该导出时
    **说清是哪一边旧**并退回逐条等价的兜底，不静默降级。
  - **两份门面同批登记**（webgl 的 ESM import + bridge 对象、微信适配层的显式白名单）——
    适配层注释里那句"少一项就是'这个平台启动即失败'"正是为了避免这次改动漏登记。
  - **验证**：`rustlua-infra` 的 `time_step` 单测 6/6（含"默认容差保持逐位严格"与"容差吸收亚 ulp 帧间隔"
    两条新钉子）；`cargo check -p game_web --target wasm32-unknown-unknown` exit=0；两个 Rust 文件
    `rustfmt --check` 零差异；产物重建后按既有整链驱动复验。
- **定死 `Image.src` 冻结用例那条既有红灯的根因（三个原因叠在一起），并修掉上一轮自己埋的遮蔽 bug（WP7-26）**。
  - **现象**：宿主不允许改写 `Image.src` 时（旧基础库），需要改写 src 的发布形态必须"响亮失败"，
    否则就是"整批贴图静默消失、日志里什么都没有"。这条负向用例连续两轮判红，而我上一轮加的"响亮失败"
    补丁**行为上一点没变**。
  - **根因一（我的 bug）**：`catch (error)` 的形参把我新加的 `error()` 助手**遮蔽**了，
    `error(msg)` 实际在"调用一个错误对象"，抛 TypeError ⇒ 日志没打、后面的 `throw` 也没到。
    改名 `logError()` 后消息立刻出现。**教训**：上一轮我验的是"用例是否变绿"，而不是"那行日志是否
    真的执行"——没变绿被我归因成"分支没走到"，其实是那行自己炸了。
  - **根因二**：贴图加载在运行时的 **Promise 执行器**里（`new Promise((resolve) => { const image = new Image(); … })`），
    适配层抛的异常只让那个 Promise 变 rejected，而运行时"以第一次 onerror 为准"⇒ 屏幕少一批贴图、
    `data-rustlua-boot` 仍是 running、日志一条都没有。所以除了抛，还要把 boot 判失败并画错误页。
  - **根因三**：判失败的标记第一版写在了适配层内部 stub 上，而 `data-rustlua-boot` 的权威读者是
    **宿主 body**（运行时往那里写 running），标记被无视。改成"宿主元素 + stub 都写"，并在每帧结束时
    重申——失败状态不允许被后续盖章盖回"看起来成功"。
  - **结果**：这条负向用例第一次通过（响亮失败 + boot 判失败 + 一张贴图都没加载的读数）。
    同时给这条用例加了**永久自证读数**（我们的 setter 被调用几次、描述符还在不在、快照与**实时**
    boot 属性各是什么）——上一轮正是被"快照看起来正常"骗过去的。
  - **形态适配**：新发布形态下这条用例必须带远端，否则 boot 先因"包内只有骨架"404，根本走不到贴图路径。
  - **顺带修掉紧随其后的第二条红灯（看门狗用例空转）**：越过冻结用例后，驱动停在 `--expect-boot-stall`——
    harness 退出 0 却只输出"正常启动"，因为看门狗那一支**只在 boot 超时那一刻才跑**，而新形态下 boot
    在截止时间前就结算了。没有靠推断，而是给 harness 加了一行**显式空转自证**
    （`negative_vacuous case=boot-stall reason=boot_settled_before_deadline pending_images=0 …`），
    驱动据此分流：拿到自证且是新形态 ⇒ 记 `[跳过]` 并继续；**其它形态遇到空转直接抛**（不放宽断言）。
  - **结果**：整条微信验证链**第一次跑到 exit=0**（31 条 harness 用例全绿，负向用例清单里含
    `image-src-frozen`）。这一轮的距离用"驱动推进量"量化：改动前停在第 7 条用例，第 54 轮推进到第 17 条，
    本轮越过冻结用例与看门狗用例一路到底。
  - **局限**：验证跑在"只替换了适配器的 launcher 副本"上（terser 单文件路线），不是重新打包的产物
    ⇒ 这三处改动尚未进入任何已发布产物；错误页是否真的画在真机屏幕上，只有真机/开发者工具能确认。
    证据：`artifacts/logs/gpu-web-probe/round55-frozen-src-root-cause.md`。
- **让"内核入分包 + 重资源远端"这一新发布形态可被验证套件识别（WP7-25）**。
  - **问题**：上一轮打出了产物（主包降到门的一成上下），但验证套件按**旧教义**把它判红——
    "启动期间读不到 pack/"、"首屏清单是空的"、"瘦首包应从包内读到骨架"。这三条对**经典**分包形态
    是对的，对这一形态则不成立：它的内核与骨架就在分包里、重资源在 CDN，启动**故意**排在挂载之后。
  - **做法**：不加标记文件、不放宽阈值，按产物自带的三个事实（有远端配置、首屏清单 `entries` 为空、
    内核在 `pack/` 里）识别这一形态，然后把三条断言换成各自的**等价物**：清单必须声明 `pack` 分包且
    不许有首屏条目；被挡的 pack/ 读取必须**全部发生在挂载之前**（即"等分包"覆盖了启动期每一次读取）；
    分包不可达的用例里骨架读不到正是被测行为本身。三种形态的互斥断言一条没松动。
  - **顺带**：驱动里"远端回落下沉用例组"原来对分包形态整体跳过，现在只跳过**经典**分包形态——
    新形态的远端就是主路径，跳过等于把校验和、缓存复用、服务不可达三条守卫全丢掉。
  - **实测**：方言 6 条用例（root/slash/sub-root/no-utf8/no-wasm/wasm-tag）与分包轴 5 条用例
    （ok/delay/never/fail-once/不依赖 `game.json`，均带远端）全部转绿；产物证据为分包**第 0 帧**挂载、
    启动期被挡的读取全部发生在挂载之前，以及上一轮修的 latch 在本产物上生效（校验失败计数为 0）。
  - **适配层（纯增量）**：新增一个走 `console.error` 的日志档，并在"宿主不允许改写 `Image.src`"的
    两处 `throw` 之前先把它写出来——原因很实在：`new Image()` 在运行时的 **Promise 执行器**里，
    裸 `throw` 只让那个 Promise 变 rejected，屏幕上少一批贴图而日志里一条都没有，"响亮失败"名不副实。
  - **仍未完成**：`--image-src-frozen` 这条负向用例仍红，且它**与本形态无关**（经典瘦首包走同一条
    代码路径）。已排除两个假设：压缩把路径弄坏（用当前源码重打一份跑，行为完全相同）、
    "抛了但没人打出来"（在两处 `throw` 前加日志，行为未变 ⇒ 那两处**根本没被执行到**）。
    下一步应直接在 harness 的冻结 setter 里计数打印，而不是继续读代码推理。
  - **局限**：本轮的驱动验证跑在"只替换了适配器的 launcher 副本"上（terser 单文件路线），不是重新
    打包的产物 ⇒ 那处适配层增量尚未进入任何已发布产物，需要下一次打包才生效；无真机与开发者工具；
    新形态仍是默认关的开关。证据：`artifacts/logs/gpu-web-probe/round54-form-aware-verification.md`。
- **把"内核入分包"做成打包器开关：微信小游戏包**第一次**打出来（主包从超门降到门的一成上下）（WP7-24）**。
  - **背景**：微信主包硬门 4 MB，而 wasm 内核**一个人就超过整包配额**；上一轮量清"唯一不动引擎本体的
    出路是把内核交给分包"（`subpackage` 形态本来就这么做，`remote` 形态没做）。
  - **新开关 `-WasmInSubpackage`**（只对 `wechat-minigame` + `remote` 有意义，**默认关**、行为不变）：
    内核搬进 `pack/` 分包，`game.json` 声明分包并写 `pack/game.js` 入口，体积门按"主包 = 总量 − 分包"
    算，并给这份包发一份 `mainpack.json`（`layout.subpackage_root=pack`、`entries` 为空）——适配层就是
    靠它判断"内核在分包"，entries 为空正是 remote 需要的语义（首屏资源在 CDN）⇒ **本轮对
    `plugins/wechat-minigame/wechat-adapter.js` 零改动**。隔离副本自检同步扩展（内核位置判据 + 两条
    清单断言）。
  - **产物结果**：打包首次成功，主包从超门（约 1.4 倍门）降到**门的 9.4%**，分包与总量都在配额内；
    内核进分包后 wasm 仍是可发布形态，`==> Atomic release swap` 落地。
  - **顺带修掉发布链两个既有真 bug**：① `wasm-opt -Oz` 在当前产物上**一直失败**（含 bulk-memory/EH
    的模块需要 binaryen 显式开关），只在日志里留一行警告 ⇒ 发布的 wasm **带着 name/debug 自定义段**
    上线（占了产物近三分之一）；现在失败会带显式特性开关重试，两次都失败才降级并留 binaryen 日志路径。
    修的过程中还踩到：`*>` 会把原生 stderr 转成 ErrorRecord，而脚本顶上是 `Stop` ⇒ 首次失败的
    validator 长文会**终止发布**，必须临时降成 `Continue`。② `remote` 的瘦身哨兵把搬进来的内核当成
    "骨架" ⇒ 误报"裁剪未生效"；现在内核从骨架读数里扣除，那道门仍然只管骨架。
  - **本机已验证到的启动链**：分包第 0 帧挂载 → **内核从 `pack/game_web_bg.wasm` 实例化成功**
    （113 个导出）→ 重资源走 CDN 回落并落进**带版本号**的校验缓存 ⇒ 上一轮修的 `releaseKey`/latch
    在这份**真产物**里生效（此前只能在 terser 单文件副本上验）。
  - **仍未完成**：驱动在 harness 阶段还报两条**旧教义断言**（"启动期间 48 次 pack/ 读取被未挂载的分包
    挡下"、"首屏清单是空的"）——本形态故意让 boot 等在挂载之后，那些被挡的读取随后由重试/远端回落补齐；
    下一轮给这两条断言加形态分支再跑绿。
  - **局限**：本形态默认关；`-SkipBuild` 会复用构建树里遗留的 wasm（本次用的是既有已胶合的那份，与
    上一轮的 `-Gpu2d` 陷阱同类）；无真机与开发者工具 ⇒ 真机分包时序、首帧等待、内存与配额未验证，
    "主包很小"是本机口径；"包内命中 + 远端回落共存"的失败可观测性是否等价于旧形态尚未评估。
    证据：`artifacts/logs/gpu-web-probe/round53-wasm-in-subpackage-unblock.md`。
- **把微信主包超限的完整成因量清，并给出三形态阻塞地图（WP7-23）**。
  - **更正第 51 轮"已 strip"**：当时只把文件**前一小段**转字符串搜 `.debug_` 就下结论"已 strip"。逐节解析实测
    该产物带 `name`、`__wasm_bindgen_unstable` 与四类 `.debug_*` 自定义段（都在**文件尾部**）⇒ **没被 strip**。
    又是"测量窗口设错 ⇒ 得出反向结论"这一类（本轮已第三次）。
  - **`gpu2d` 是错的产物（已确认的成因之一）**：构建树那份 wasm 里 `wgpu`/`naga`/`gpu_host`/
    `WebGl2RenderingContext`/`wgpu_hal` 特征串全都在，而**现网微信产物**这五项全为 0；`game_web/Cargo.toml`
    与打包器都写明 `gpu2d` 默认关、`-Gpu2d` 才开 ⇒ 那份是带 `-Gpu2d` 编过之后留下的，`-SkipBuild` 复用了它。
    按打包器自己的命令**不带** `--features gpu2d` 重建后体积降到原来的约六成（再经同版本 `wasm-opt -Oz`
    后仍见下条）。
  - **去掉 `gpu2d` 也仍超门（关键结论）**：微信主包硬门是 4 MB，而主包除 wasm 还要放子集字体、JS 胶水与
    运行时、`pack/` 骨架 ⇒ 按权威重建值算，主包仍超门约三分之一，**wasm 一个人就超过整包配额**。
    为此写了两个可复跑脚本，对发布产物做**逐节**（code/data/自定义段）与**逐 crate 函数体**体积归因。
  - **归因结果**：占比最大的是 `gpu2d` 专属的 naga/wgpu（不带 `-Gpu2d` 就不进产物）；本项目必需的是
    mlua+Lua 内嵌 C 与 `rustlua_scripting`/`engine`；**最大的一块可省项是物理引擎（rapier2d/3d + parry2d/3d）**，
    其次是 `zune_jpeg`（素材是 PNG）。但把物理与 JPEG 全砍掉也补不上缺口 ⇒ **不是"再挤一挤 wasm"能解决的**。
  - **三形态阻塞地图**：`remote` 卡主包门（内核留在 `pkg/`）；`subpackage` 过主包门（打包器会把内核搬进
    `pack/`，仓库代码与 harness 都按"内核在主包/分包"分叉）但卡总量 20 MB（按现有素材体积估算）；
    `full` 卡主包门。真正需要的那条组合——**重资源走 CDN + 内核在分包**——**当前没有实现**
    （`package.ps1` 明写 `remote` 会把 `pack/` 裁成骨架、内核留在主包）。
  - **候选出路（都带代价，需裁决）**：①实现"remote + 内核入分包"（两门皆过，代价是首帧等分包挂载，
    harness 已有这条不变式与读数）；②`subpackage` + 素材大幅瘦身；③`remote` + 把 wasm 砍掉约三成
    （已量到的可省项不够）；④只缩非 wasm 占用补不上缺口。
  - **本轮未改任何发布/构建配置**：只做测量、归因与更正。证据与复跑命令见
    `artifacts/logs/gpu-web-probe/round52-wasm-feature-and-pack-unblock.md`；
    **局限**：`subpackage` 的总量是按现有体积估算（打包在 `remote` 的主包门就抛错、没跑到总量门），逐 crate
    归因在带函数名段的产物上做 ⇒ 绝对占比不等于发布配置；仍无真机与微信开发者工具。
- **量出并修掉微信侧"静默未校验"的确定性根因（自锁 latch）；撤回并更正上一轮关于尺寸门的错误结论（WP7-22）**。
  - **一行插桩换来定论**：在 `noteUnverified` 的样本里附内部标志后，三个首屏文件全部是
    `清单未就绪［manifest=0 lookupFailed=1 download=0 loading=0 configLoaded=0］`。判据是 **`download=0`**
    —— `manifestDownload` 为空 ⇒ 清单**从未被尝试下载** ⇒ 标记不可能来自"下载失败"那一支
    （`ensureManifest` 的 `.then`，只在下载流程内触发）⇒ 只能来自 `loadManifestChecksums()` 里
    "读清单时 `remoteConfig` 还是 null"那一支。**这是自锁**：`ensureManifest()` 一见标记就直接返回不下载
    ⇒ 清单永远下不来 ⇒ 而唯一能清标记的 `resetManifestDerived()` 只在**清单下载完成**时被调用 ⇒
    标记永远为真 ⇒ 该会话内所有远端口资源无据可依（实测 3 个首屏文件静默放行，含被翻转一字节的
    `pack/project.toml`，随后被写进缓存并被当成好文件用）。上一轮记的"竞态"只对一半：是否触发取决于
    `loadRemoteConfig()`（惰性）与"启动自检/缓存路径解析先读清单"的先后。
  - **修法（换判据、保住原意）**：`if (!remoteConfig)` → `if (remoteConfigLoaded && !remoteConfig)`
    （`remoteConfigLoaded` 早在既有代码里）。原意图是"只有**根本不会有远端**时才记这次失败"，而
    `!remoteConfig` 只表示"还没轮到读配置"；纯包内形态的原有效果不变（配置读完后照样 latch，不会每次
    读清单都重试）。
  - **四面验收**：`--remote corrupt` 由 exit 1 变 **exit 0**
    （`negative_ok case=checksum-corrupt boot=failed download_attempts=4 checksum_mismatch=2
    discard_warns=2 remote_failed=2`）；`--remote thin` **exit 0** 且
    `remote_downloads=178 verified=178 checksum_unverified=0 cache_hits=363`（与旧产物**完全一致**，无回归）；
    `--remote down` **exit 0**（仍要求启动失败，`discard_warns=0` ⇒ 新计数不恒真）；thin/down 跑未校验告警均为 0 条。
  - **补两道门**（第 49 轮教训：只有内部计数不算"可见"）：①`corrupt` 分支断言 `checksumMismatch>=1` 时
    `consoleWarns` 里**必须**有 `远端文件未通过清单校验，已丢弃`；②远端形态（`auto`/`thin`）断言
    `checksumUnverified === 0`，失败时把样本路径报出来。背景更正：harness 把 `console.warn` 收进
    `consoleWarns` 且**不打印** ⇒ 上一轮文档里"warn 行可直接搜到"的说法**不成立，已更正**。
  - **撤回上一轮的尺寸门结论**：原文断言"代码与自己的注释矛盾"不成立。实测构建树里的
    `release/game_web.wasm` 明显大于现网产物内的 `launcher/pkg/game_web_bg.wasm`（相差一倍以上，
    具体数值见任务计划 WP7-22 与证据文档）⇒ 报出的主包体积就是**主包本体**（wasm + JS + 骨架），
    门没有算错。真因是 **wasm 体积翻倍**，而微信主包硬门是 4 MB ⇒ `remote` 与 `subpackage`
    两形态都因 wasm 单独超限而打不出包（成因与出路已在 WP7-23 量清）。
    **本轮未改任何构建配置**，只报告不猜测。旁证（未验证，仅记录）：打包器报错文案称
    "opt-level=z + lto=fat + codegen-units=1 已全部生效"，而 `engine/Cargo.toml` 的 `[profile.release]` 实为
    `opt-level="s"`、`lto=false`、`codegen-units=16`。
  - **验收方式边界（与上一轮相同）**：产物内适配器是压缩过的（72,376 B vs 源 194,999 B），重打包被尺寸门
    挡住 ⇒ 仍用**打包器同一套 terser 流程**把改动压进 `round50-launcher/` **副本**（77,853 B）做行为验证
    —— 是"源码改动的行为验证"，**不是**完整重打包的产体验证；微信驱动脚本因此仍对旧产物跑并继续变红。
  - **证据**：`artifacts/logs/gpu-web-probe/round51-latch-and-pack-size.md`、`round51-corrupt-flags.log`、
    `round51-corrupt-final.log`、`round51-thin-gated.log`、`round51-down-final.log`、`round51-corrupt-gated.log`。
    **局限**：仍无真机/微信开发者工具；本轮**未产出**新微信产物；wasm 体积翻倍的成因未查（只做测量与
    撤回错误结论）。
- **把微信侧"静默未校验"变成可见并把窗口缩到一个具体位置；顺带撞出微信打包的尺寸门阻塞（WP7-21）**。
  - **改动**（`plugins/wechat-minigame/wechat-adapter.js`，三处**加法式插桩**，不改语义）：新增
    `assetStats.checksumUnverified` + `unverifiedSamples`；新增 `noteUnverified(path, reason)`
    （`warn('未校验就放行 <path>（<原因>）')` + 计数）挂在 `verifyDownloaded` 的两条静默放行分支
    （`清单未就绪` / `清单里没有这一项`）；`downloadToCache` 里"非清单文件却 `verify=false`"也记一笔
    （理由 `这条下载路径没有请求校验`；清单自己按设计不校验，显式排除）。harness 侧：负面对照失败消息带
    `checksumUnverified=<n>` 与 `未校验样本=<path:原因,…>`，正常路径 ok 行新增 `checksum_unverified=<n>`。
    （事后补记：`verifyDownloaded` 里那句"下面会记一笔'未校验'"的注释此前**并不存在**对应实现。）
  - **两面对照验收**：`--remote thin` 旧产物 `remote_downloads=178 verified=178`；同一用例插桩版
    `remote_downloads=178 verified=178 checksum_unverified=0` ⇒ **数字完全一致，插桩零行为改变**；
    `--remote corrupt` 插桩版 `checksumMismatch=0（未拦下）；remote_downloads=180 verified=176
    checksumUnverified=3`，并**指名** `pack/project.toml`、字体、`pack/scenes/main.scene` 三条
    （`未校验就放行` 的 warn 行可直接搜到）⇒ **第 49 轮"180 − 176 = 4"的账对上了**：3 个资源未校验
    + 清单自己按设计不校验 = 4。
  - **窗口缩到哪里**：日志顺序显示首屏三个文件**先于** `pack/manifest.txt` 被"清单未就绪"放行；下载清单的
    唯一入口 `ensureManifest()` 在 `manifestChecksums || checksumLookupFailed` 为真时直接返回不下载
    （`:1044`），而 `checksumLookupFailed` 只有两处写（`:832` 远端配置不存在 / `:1048` 清单下载失败）、
    只有 `resetManifestDerived()`（`:743`）会清。**形态确定**（清单未就绪 ⇒ 静默放行），那一刻内部三标志
    的状态尚未直接量到 ⇒ 下一轮一行即可定论；另有独立证据说明它是**竞态**而非永久失效（同代码 thin=0、
    corrupt=3）。
  - **撞出的阻塞（真实、非本轮引入）——微信包当前打不出来**：`-AssetPack remote -RemotePackBaseUrl …
    -SkipBuild` 重打时形态处理全部正确（瘦首包骨架 79 KB、改名、CommonJS、upload_ecma 均 OK），但尺寸门抛
    `微信小游戏主包超限: 5,763 KB > 4,096 KB（-AssetPack remote：pack/ 不是分包，整个包都算主包；当前
    pack/ 79 KB）`；`Assert-WeChatPackageVolume` 让 `remote`/`full` 共用一条 `else`（`package.ps1:312-316`），
    只要 `$TotalBytes`（`Get-ChildItem $distRoot -Recurse`，`:1519`）超 4 MB 就抛。`-AssetPack subpackage` 同样
    过不去（`assets/` 21.2 MB + 包内内容撞总量 20 MB 门）。旧产物被原子交换保住
    （`Release failed before swap; existing release kept`）。
    （**第 51 轮更正**：本段原写的"代码与自己的注释不一致"**不成立、已撤回**——构建树 wasm 明显大于现网产物
    ⇒ 报出的主包体积就是主包本体，尺寸门没算错；真因是 wasm 体积翻倍，详见 WP7-22 段；完整成因与三形态
    阻塞地图见 WP7-23 段。）
  - **验收方式的边界（必须说清）**：产物里的适配器是**压缩过的**（远小于源码，不能手改），而重打包
    被尺寸门挡住 ⇒ 本轮用**打包器同一套 terser 流程**（`engine\.tools\terser\node_modules\.bin\terser.cmd`，
    与 `Minify-JsFile` 同一条 `<in> -o <out>`）把改动压进 `round50-launcher/` **副本**再做行为验证。这是
    "源码改动的行为验证"，**不是**"完整重打包的产体验证"。
  - **判断偏差**：把"计数差 4"当成"4 个资源"（实际 3 资源 + 清单自己）；先想直接从代码推出是哪条分支
    （`releaseKey` latch / `readText` 缓存顺序 / `fetchShim` 首屏跳过都自圆其说），最后仍靠**插桩 + 日志顺序**
    定论 ⇒ 多个分支都能解释同一现象时，静态推理必须让位给测量。
  - **证据**：`artifacts/logs/gpu-web-probe/round50-unverified-visibility-and-pack-blocker.md`、
    `round50-wechat-repack.log`、`round50-corrupt-instr.log`、`round50-thin-instr.log`、`round50-thin-old.log`。
    **局限**：本轮**没有**产出新产物（被尺寸门挡住）⇒ 没有"新产物全绿"这类读数；插桩只改可见性，**未**改变
    "未校验就放行"的策略（先确保清单 vs 拒绝未校验仍是待裁决的产品选择）；仍无真机。
- **微信小游戏侧首次建立运行基线；修掉两个"把真问题掩盖掉"的门，并钉死一个资源管线缺陷（WP7-20）**。
  - **为什么做微信**：`verify-wechat-minigame.ps1` 自己写明运行时验证交给 `wechat-minigame-harness.js`
    （Node 伪造 `wx`，真实执行 `game.js → wechat-adapter.js → web-runtime.js`）⇒ 微信侧**只有旧 JS 世界**
    这一条路，与 webgl 侧（引擎宿主）分叉；此前几十轮我一次都没在微信侧跑过验收。
  - **首次量到的基线**（`builds/wechat-minigame`：`launcher/` 17 文件 3.0 MB + `assets/` 1,201 文件 21.2 MB，
    内含 `pack/` 1008 张 PNG、153 个 `.lua`、`manifest.txt` 1,201 行）：旧 JS 世界**真的在绘制**
    （`frames=12 draw_ops=438 draws_per_frame=5 drawn=1486 ui_elements=13 prefab_text=4/4 prefab_slice=36/4`）；
    wasm 内核**真的实例化**（`WXWebAssembly/path-instantiate，pkg/game_web_bg.wasm，导出 116 项`，外加
    `TextEncoder/TextDecoder` polyfill 与 `WebAssembly` facade）；`eh_probe=accepted`；瘦首包远端回落**真的在取**
    （字体 597932B / `main.scene` 809B / `manifest.txt` 74613B，`verifyChecksum=true`）；负面对照 `--remote down`
    ⇒ `negative_ok case=remote-unreachable boot=failed download_attempts=2 remote_failed=2 elapsed_ms=58`。
  - **门缺陷一（假失败）**：`--no-wasm` 用例在瘦首包上必然失败——它按设计不启动游戏（`boot=failed`、`frames=0`），
    而"二进制探针""字体已注册"两条断言是无条件的，瘦首包 `pack/` 只有骨架 ⇒ 报 `发布包 pack/ 里没有 PNG`
    + `字体未注册（fontFamily=null）` ⇒ 驱动抛 `wasm 缺失时未按契约响亮失败`。**修**：照既有
    `skipped(pack-unreachable)` 风格显式标注 ⇒ `binary=skipped(no-wasm)`、`font=skipped(no-wasm)`；
    验收该用例 `exit 0` 且 ok 行里 `wasm=no-webassembly:undefined:none:none` 仍在（诊断没被削弱）。
  - **门缺陷二（空转的断言）**：缓存污染检查只看 `userRoot/pack/project.toml`，而适配层缓存**实测**也落在
    `userRoot/v-<发布标识>/pack/…` ⇒ 旧布局之后命中不了。**修**：改成"缓存字节 vs 资源包真字节"逐字节比、
    `v-*` 与新路径都扫。**这一修立刻兑现**：改后驱动真的抓到
    `被篡改的文件仍留在缓存里: C:\…\rustlua-wx-user-5GC6wp\pack\project.toml`。
  - **门缺陷三（断言混用掩盖真问题）**：`corrupt` 与 `down` 共用"必须启动失败"。两者契约不同：`down`
    （连 wasm 内核都取不到）确实必须中止（复跑仍 `boot=failed` ✓）；`corrupt` 的被测契约是"**校验和必须
    把脏字节拦下**"（用例自己的注释就是这么写的）。**修**：`down` 保留启动失败断言，`corrupt` 断言
    `checksumMismatch ≥ 1` 并把计数打进消息 —— **这一修把真问题顶了出来**（下一条）。
  - **钉死的资源管线缺陷（下一轮首要项）**：远端可达时，被翻转一个字节的 `pack/project.toml` **没有被拦下**、
    **写进了缓存**、**被当成好文件继续用**（`boot=running`）。改后驱动的三条证据：
    `期望校验和不符被拦下（boot=running、启动是否 reject=false），实际 checksumMismatch=0`；
    `checksumMismatch=0（未拦下）；remote_downloads=180 verified=176`；
    `被篡改的文件仍留在缓存里: …\rustlua-wx-user-5GC6wp\pack\project.toml` ⇒ **180 次远端下载里 4 次根本没走校验**。
    实锤还有一条：`wechat-adapter.js:850` 的注释承诺 `// 清单不可得时不阻断，但下面会记一笔"未校验"`，
    而整套日志里**没有任何一处**"未校验"记录 ⇒ 这个跳过不只是存在，还是**静默**的（`checksumMismatch`
    只记"拦下了但内容不符"，不记"压根没校验"）。**待裁决的产品策略**（`wechat-adapter.js:847-867`）：
    ①"清单未就绪 ⇒ 先确保清单再校验"（贴合现有惰性加载设计，需照顾 `checksumLoading` 重入）或
    ②"没校验过的清单内文件一律不采信"（更严，会影响首包启动时序）。**本轮不擅自改**：该方法在用户正在
    编辑的文件里，且属于产品策略选择。
  - **我自己犯的两个测量错误**：①**拿"没接通远端"的单跑去判产品**：单跑 `--remote corrupt` 得
    `checksumMismatch=0 remote_downloads=0`，差点写成"校验和从来拦不下脏字节"——但 `remote_downloads=0`
    说明那一跑**根本没走远端**（`remote-assets.json` 还是发布用配置）⇒ 权威测量是驱动那一跑（它把远端指向
    自起的本地服务，日志里 `远端取得 pack/project.toml (3872B) <- http://127.0.0.1:10202/…` 就是刺激送达的
    证据）。反面用例必须先自证刺激送达，否则读数与"什么都没发生"不可分。②又一次差点**用"缺失即证据"**
    （据"失败清单里没有 `checksumMismatch<1` 那条"推断"拦下了"），被单跑直接反证 ⇒ 所以改法把计数**打进消息**。
  - **证据**：`artifacts/logs/gpu-web-probe/round49-wechat-baseline-and-gates.md` 与
    `round49-wechat-baseline.log`、`round49-nowasm-{a,a2,b}.log`、`round49-remote-down{,2}.log`、
    `round49-remote-corrupt{,2}.log`、`round49-wechat-{full,after-fix}.log`。**局限**：全部是 Node 伪造 `wx`
    的读数（无真机、无开发者工具）；微信侧仍**只有旧 JS 世界**（引擎/GPU 未接入）⇒"两平台一致"仍不成立。
- **旧 JS 世界默认不再作为第二宿主启动——同一份 webgl 产物里两个宿主并行跑，引擎早已独占这一帧；性能差异测不出（WP7-19）**。
  - **查清的实况**：旧 JS 世界（`web-runtime.js`，5352 行：自建**第二个 Lua 状态**——WP10 起 Lua 已是 wasm 的
    mlua，不再是 fengari；自己在 JS 里 `parseScene`；自己一个 60 Hz 追帧循环；自己画到**同一张**
    `#rustlua-canvas` 的 2D 上下文）与引擎宿主（自带 Lua 状态、`RuntimeScene::run_script_steps`、GPU 呈现）
    在同一份产物里**并行**跑。三条独立证据：①不 stub 时 `-DriveJhyt` 跑出
    `drive_jhyt: done detail: milestones=M1:new-game(map=manor,alive=231),M2:move(528,112->528,131),M3:bag-open…`，
    而那段明细只由 `web-runtime.js` 写；②`-StubJsRuntime`（JS 世界完全不启动）的画面与不 stub **逐字节相同**
    ⇒ 玩家看到的始终是引擎画面；③同段日志里 jhyt 的 `[input] project.toml: true (loaded)` 出现两次 ⇒
    两个宿主各启动了一次游戏。
  - **改动**：`plugins/webgl/index.html` 把 `await window.rustluaRunLuaGame(status)` 改为**默认不调用**，
    只在 `?rustlua-js-host=1`（或启动前 `window.__RUSTLUA_JS_HOST = true`）时调用，否则打印
    `[host] legacy JS world skipped (engine host owns the frame)`；`markBoot('ok')` 照旧 ⇒ `data-rustlua-boot=ok`
    不受影响（git diff 29 增 1 删）。驱动新增 `-UrlQuery <原始查询串>`，并在 `-DriveJhyt` 注释里写明它驱动的
    是旧世界、必须配合该开关。
  - **三向验收（同一产物）**：①默认 ⇒ exit 0、引擎 `UPD_G=210`、闸门日志 1 条、**无**任何 JS 明细、画面
    `29,669 B / A3D0197713F54611`（与第 46/47 轮**逐字节相同** ⇒ 零回归）；②正对照
    （`-UrlQuery rustlua-js-host=1` + `-DriveJhyt`）⇒ exit 0、**`drive_jhyt: done` + 全部里程碑
    `M1:new-game … M4:quest-close`**，旧世界完整回来；③负对照（仅 `-DriveJhyt`）⇒ exit 1、
    **`drive_jhyt: absent`** ⇒ `[RED] … boot=ok drive=absent`（同一次运行 `boot=ok` 说明页面本身健康）。
  - **性能：测不出差异（自己的第二个样本把预期打掉，如实记录）**：首样本无闸门 `9.39 ms / 106.5 fps` vs
    有闸门 `8.08 ms / 123.8 fps`（看着像快 14%），第二样本无闸门 `9.29 / 107.6` vs 有闸门 `10.56 / 94.7`
    ⇒ 均值 **9.34 vs 9.32**、组内跨度 **2.5 ms** ⇒ 夹具分辨不出。原因：`avg_frame_ms` 是引擎自报的**呈现循环**
    耗时，旧世界的额外工作在同一个 rAF tick 的另一段、未必落进该窗口；夹具还用 CDP 泵帧并开着诊断用的
    `willReadFrequently`。⇒ 本轮**不做性能提升声明**，只声明结构上旧世界已不在帧路径（③ 已证）；真结论待真机。
  - **本轮自己犯并修好的错（记下来）**：①**把 DOM 属性当 console 日志 grep**——先前用"日志里没有
    `data-rustlua-timing` / 没有 `[lua] WebGL Lua runtime started`"推断"旧世界从未启动"，而前者是**页面属性**
    （根本不会出现在日志里）、后者在**页面加载瞬间**打印而 CDP 加载后才附上 ⇒ 两个 0 都无意义；救回来的是
    `-DriveJhyt` 的明细。教训：判定"某段 JS 有没有跑"要用它自己写的、外部可读的读数（属性/里程碑）。
    ②**编辑 `.ps1` 破坏编码**：用编辑工具写回无 BOM 的 UTF-8，`powershell.exe`（5.1）按 ANSI 解码 ⇒
    中文乱码（`(鏈嚜鎶?`）⇒ 解析器报 `Missing closing '}'`，三次验收全 exit=1 且无截图；既有规则
    "`.ps1` = UTF-8 **with BOM** + CRLF"此前只用于**新建**，本轮证明**编辑**同样必须遵守（读成 UTF-8 →
    统一 CRLF → 带 BOM 写回，再用 `PSParser::Tokenize` 校验语法 0 错）。③内联探针引号嵌套写错 ⇒
    改为落一个带 BOM 的探针脚本执行。
  - **待办**：①微信路径未动（`wechat-adapter.js` 有自己的入口与 Lua 门面，`index.html` 注释要求两份门面
    同步）⇒ 是否同样加闸门，连同**真机**一起验；②旧世界文件仍在包里（188,749 B）⇒ 删除是 WP8 收尾；
    ③性能结论待真机；④引擎独跑路径的**存档持久化**仍未做。
  - **证据**：`artifacts/logs/gpu-web-probe/round48-legacy-js-host-gated.md` 与 `round48-*.log/.png`
    （改前基线、默认、正/负对照、改前 `-DriveJhyt` 活体证明、repack、两个探针）。**局限**：只验 webgl 产物
    （微信与真机未验）；性能不作结论；旧世界现在必须显式开关才能启动。
- **清掉引擎独跑 web 路径上最后两条"裸 fs"告警——`operation not supported on this platform` 归零；标题对照逐字节不变（WP7-18）**。
  - **音频设置读**：链路是 jhyt 的 `scripts/core/audio.lua` → `audio.load_settings()` →
    `rustlua-scripting/src/script/api/audio.rs` → `ProjectSettings::load_from` →
    `rustlua-gameplay/src/settings.rs` 的 `std::fs::read_to_string` → wasm `ENOSYS` → 游戏自己打
    `[audio] project.toml: false (operation not supported…)`（同一段日志里 `[input] project.toml: true (loaded)`
    是第 41 轮修好的同族，两行并排，差别只在读法）。改法与第 41 轮**同形**：`api/input.rs` 的
    `read_settings_text`（内部 `vfs::read_resource`）改 `pub(super)` 供同级复用；`audio.load_settings` 改成
    `ProjectSettings::path_in` → `read_settings_text` → `NotFound` 仍是 `(false,"missing")` →
    `from_toml` → `apply_to(&mut …audio)`（三者本来就 `pub`，未新增公开 API）。**没有改 `load_from` 本身**：
    `rustlua-gameplay` 依赖里没有 `rustlua-infra`，且该 crate 头写明"不引入任何新依赖"；改完后 `load_from`
    的调用者只剩编辑器面板与它自己的测试（桌面侧）。
  - **存档目录**：`scene.rs` → `user_dir.rs::ensure_save_dir` 的 `fs::create_dir_all`。改为没有文件系统的宿主
    直接 `Ok(())`：`cfg!(target_arch = "wasm32") || save_slot::is_installed()`。**"或"是必要的**：`save_slot`
    provider 只在 `game_web/src/lua_bridge.rs` 装（JS 世界那条路径），引擎独跑那条（本轮受测路径）根本没装槽；
    桌面不装槽 ⇒ 桌面行为一字不变。**顺带发现（推断，非实测）**：该路径既无文件系统又无槽 ⇒ `save.commit`
    无处可落，即**目前没有持久化**（本轮只观察到 `[save] cleared 0 in-memory key(s)` 这类内存操作）⇒ 列入待办。
  - **验收**（同剧本同端口族，只差本片修复）：`[audio] project.toml:` 由
    `false (operation not supported…)` 变 **`true (loaded)`**；`无法创建存档目录 …` 告警 1 → **0**；全日志该类
    告警 2 → **0**（两次运行都 0）；告警**形态**由 2 种降为 **1 种且无新形态**（消失的正是存档那条）；标题对照 PNG
    与上一片**逐字节相同**（783 色、主色 `#6B4F3A` 20.8%）⇒ 两处都不涉渲染、**零回归**；玩法画面 561 色、主色
    **同为** `#7E5D3E` 18.6%、异步贴图 41 成功/0 失败、`entities=226`、`script_errors=0` 均同。剩下唯一一条告警是
    **已知平台能力说明**（非缺陷）：WebGL2 的 `max_storage_buffers_per_shader_stage=0` ⇒ 矢量文字与 3D 网格/阴影
    两条路不构造、其 draw 计入 `dropped_draws`。
  - **验收材料**：`check`=0、三个 crate 测试全绿、`fmt --all --check`=0、clippy=0；产物自报 `wasm_host_ok`
    （platform=webgl）与 `webgl_package_ok`；`artifacts/logs/gpu-web-probe/round47-wasm-fs-residuals.md` 与两组
    PNG/日志。**局限**：仍只验 webgl + 引擎独跑这条路径（JS 世界在场那条与微信真机未验）；"该路径无持久化"是
    推断、未构造真实 commit 证伪；`cfg!(target_arch = "wasm32")` 是编译期判断，以后若给 wasm 接虚拟 FS 需同步改。
- **修掉按需贴图的裸 fs 读——玩法画面从"5 色白屏"变成真正渲染；标题侧出现一处确定性像素变化并已归因（WP7-17）**。
  - **修了什么（4 处，同一形状：裸 `std::fs` → `vfs`）**：`rustlua-render/src/texture.rs` 的
    `TextureAsset::load_png`（`std::fs::read`；`async_loader::decode_job` 调的正是它，是那 34 条告警的元凶）、
    `engine/.../runtime_scene/tilemap.rs` 的 `texture_dimensions`（`fs::read`，manor 地表 tileset 走这里取尺寸）、
    `rustlua-render/src/atlas.rs` 的 `load_atlas`、`rustlua-assets/src/atlas_format.rs` 的
    `AtlasDefinition::load`（`fs::read_to_string` → `vfs::read` + `String::from_utf8`，`.atlas` 在包里是明文、
    不做 `pack::decode`）。全部用 `vfs::read` 而非 `read_resource`：紧随其后的 `pack::decode` 不动，故"原始字节 →
    解码"与原来逐字节一致；桌面无 provider 时 `vfs::read` 就是 `fs::read`。与 WP7-16 的 `transition.rs` 同形。
  - **端到端验收**：同剧本 `space:20-400`，上一片是 34 条贴图 `ENOSYS` 告警 + 0 条异步成功 + 画面 5 色白屏
    （`#FFFFFE` 39%）；本片是 **0 条贴图告警 + 41 条 `texture loaded (async)`（41 个不同路径）+ 画面 563 色**
    （主色 `#7E5D3E` 棕土地表）。41 条成功的路径正是此前失败那组（地表/三个角色 `idle_s`/装饰/建筑/技能图标）。
    `entities=226`、`script_errors=0`、系统运行图（标题菜单退场、`ui.input`/`ui.virtual_pad`/`systems.effects`
    各 210、`mode=new`）与上一片完全一致 ⇒ 只有画面变好，行为不变。
  - **反证项响了，如实记录并归因**：登记过的"对照与修前逐字节相同"**没有成立**——第 44 轮基线 vs 本片对照有
    **4,235/333,396 像素（1.270%）**不同，最大通道差 184，包围盒 (462,390,756,433) 即 294×43 的右下横带；
    **本片两次运行 0 像素差**（`7F88C26E4AA66674` 精确复现）⇒ 是确定性差异。归因：`tilemap.rs::texture_dimensions`
    在 wasm 上此前恒失败（裸 `fs` → `ENOSYS`）⇒ 回落"单四边形"，走这条查询的标题 UI 元素画错尺寸；现在读到真实
    尺寸 ⇒ 按正确尺寸绘制。⇒ **第 44/45 轮记的标题基线哈希作废**，新基线 `7F88C26E4AA66674`。
    **方法学**：玩法画面不是逐帧确定的（同剧本两次运行相差 203,080 像素，`mode=new` 的新游戏流程里有计时/动画）
    ⇒ 玩法侧只比读数与聚合统计（实体数/告警数/唯一色数量级/系统计数），只有标题、暂停这类静态界面才做逐字节对照。
  - **同族残余（已定位，下一片）**：`[save] 无法创建存档目录 /project/.cache: operation not supported on this
    platform`（wasm 上有 `save_slot` 的 provider，但"目录"这一步没有对应物）与 `[audio] project.toml: false
    (operation not supported on this platform)`（音频设置读取仍是裸 `fs`，第 41 轮就列在待办）；两者两次运行都在
    ⇒ 先存问题，不影响画面与玩法推进。WP7-13 的工作项不变（输入帧喂入 + dt/时钟推进 + 虚拟键钩子搬进宿主/wasm）。
  - **验收**：`check`=0（含 `engine`/`rustlua-render`/`rustlua-assets`）、三个 crate 测试全绿、`fmt --all --check`=0、
    clippy=0；产物自报 `wasm_host_ok`（platform=webgl）与 `webgl_package_ok`；
    `artifacts/logs/gpu-web-probe/round46-texture-vfs.md` 与四组 PNG/日志。局限：只验 webgl + 引擎独跑这一条路径；
    `atlas*` 两处是同族按构造修（jhyt 不走该路径，只做到编译 + 桌面测试全绿）。
- **修掉"每一次场景切换都被静默忽略"——jhyt 首次在引擎独跑的 wasm 路径上进入玩法；并当场量出白屏的第二处同族缺陷（WP7-16）**。
  - **修了什么（4 处，全是运行期 crate）**：`rustlua-scripting/src/script/mod.rs` 的 `request_scene_load` 用裸
    `Path::is_file()` 判场景文件存在；`script/api/world_data.rs` 的 `map.goto`/`map.go`/`map.enter` 把同一段
    裸 fs stat 复制了三遍；`script/transition.rs` 的 `process_scene_transition` 用 `Scene::load`（内部 `fs::read`）。
    wasm 上没有文件系统（`is_file()` 恒假、`fs::read` 恒 `ENOSYS`），于是 `scene.load()` 的"文件在不在"这道门
    **每次都判假**、切换被静默忽略。改为唯一判据 `scene_file_exists` → `rustlua_infra::vfs::is_file`，读侧改为
    `vfs::read` + `Scene::from_packed_bytes`（与 `Scene::load` **逐字节等价**；桌面无 provider 时 vfs 回落文件系统，
    行为一个字不变）。启动路径 `runtime_scene/mod.rs:520-538` 本来就写了同一条回退链，**运行期切换与那四个门被漏掉**。
  - **端到端验收（正对照）**：同剧本 `space:20-400`，修前 `[map] scene file not found; load ignored name=manor`
    （`ui.title_menu`=210、`ui.input`=0）；修后 `transition queued name=manor` → `[scene] switched name=manor
    entities=226`，系统运行图变成 `ui.title_menu=0, ui.input=210, ui.virtual_pad=210, systems.effects=210`、
    `mode=new`、`script_errors=0`、`steps=240`。对照 `w:20-400` 仍停在标题。**零回归**：修后对照的截图与修前
    对照**逐字节相同**（哈希 `641BCC1996C67BE2`）。⇒ jhyt 第一次在引擎独跑的 wasm 路径上走到玩法。
  - **同时量出第二处同族缺陷（白屏真因，已定位到行）**：修后画面近白（`#FFFFFE` 39% + `#FFFFFF` 36.5%，仅 5 色），
    日志有 **34 条/34 个不同路径** `[render] async asset load failed; fallback used … error=could not read PNG
    …png: operation not supported on this platform`（= wasm 上的 `std::fs`）；读方在
    `engine/src/host/renderer/gpu_mesh.rs:123` 与 `rustlua-render/src/texture.rs:196`。对照分明：**15 张**
    `2D texture uploaded from memory`（宿主预推的标题 UI 美术）走通 × **34 张**按需加载（地表/三个角色 idle/
    装饰/建筑/技能图标）全回落内建白贴图；这些文件**在产物清单里都存在** ⇒ 不是打包缺文件，是**读的通道**问题。
  - **下一片**：把 `gpu_mesh.rs` 的按需贴图读入改走 `vfs`（同形改法），期望 34 条告警清零、画面唯一色数 5→百级、
    标题与对照仍逐字节不变；WP7-13 工作项不变（输入帧喂入 + dt/时钟推进 + 虚拟键钩子搬进宿主/wasm）；
    微信侧共用同一 wasm 代码路径但真机未验。
  - **验收**：`check`=0、测试全绿、`fmt --all --check`=0、clippy=0（仅既有 1 条测试警告）；
    产物自报 `wasm_host_ok`（platform=webgl）与 `webgl_package_ok`；
    `artifacts/logs/gpu-web-probe/round45-scene-transition-vfs.md` 与四张 PNG / 四份日志。
- **输入链端到端验收通过（带正对照）；"游戏没反应"的真因是 sim 停在标题界面——我瞄错了游戏状态（WP7-15）**。
  - **端到端验收**（路径 B，引擎独跑）：`title-confirm`（剧本 `space:150-400`）游戏自己的逻辑问
    `input.action_pressed("confirm")` **210 次、拿到为真 1 次**；`title-ctrl`（剧本 `w:150-400`）**210 次、0 次**
    ——其余五个动作（`move_up`/`move_down`/`slot_1..3`）两次都 `210/0`（无串扰）。`confirm` 在 jhyt 的
    `project.toml` 里是 `key:space,pad:0`（产物与仓库文件都验过），拿到为真的那一次正是按下边沿 ⇒ 输入链
    一路走到游戏自己的每帧逻辑：`-InputScript` → `ScriptDriver` → RLFI → `FrameInputProducer` →
    `apply_frame` → `InputSnapshot` → 动作映射（来自 pack 的 `[input]`）→ Lua `input.action_pressed` →
    jhyt 正在运行的那个系统（`ui.title_menu.update`）。
  - **真因（量出来的）**：`-WorldDo` 遍历 `package.loaded` 给每个带 `update` 的模块装计数器（`#wrapped=5`）：
    `ui.title_menu=210`、`_G=210`、**`ui.input=0`**、`systems.effects=0`、`ui.virtual_pad=0`。`ui.input` 就是管
    玩法 UI（`pause`/`inventory`/`quest`）的模块 ⇒ 一次没跑；而 `ui/input.lua:461` 的 `M.update()` 在 `:493`
    是**无条件**调 `update_context()`（前六个模式分支都不返回，`get_mode()` 又是 `Playing`）⇒ "一次没跑"是
    实打实的 ⇒ **这个模拟停在标题界面**，`get_mode()` 的 `Playing` 是游戏自己的模式串、标题是它上面的 UI 层。
    ⇒ WP7-14 里按 `l`（`quest`）期待任务面板，**瞄的是当时并不存在的游戏状态**：错在探针，不在输入链。
  - **本轮我自己的三个探针错误（记进方法）**：①命名空间混用——`-InputScript` 用线格式键名（`l`；
    `rustlua-frame-abi` 文档明写"词汇表是 web 宿主的裸名"），我写的是动作映射拼法 `keyl` ⇒ 整条剧本被判非法；
    ②读数取第一次出现——`expr=`/`steps=`/`input_edges=` 每帧刷新，第一次是第 30 帧（脚本第 60 帧才按下）⇒
    必然全假，**一律取最后一次**；③把"某次调用"当成"那次调用"——用 `input.bindings(a)` 记第一次调用的参数
    得到 `padaxis:1+`（那其实是 `move_up`），改成只对 `a=='quest'` 记绑定后才看清 `quest` 一次没被问过。
  - **可复用工具（本片最有价值的副产品）——"系统运行图"**：`-WorldDo` 遍历 `package.loaded` 给每个带 `update`
    的模块装计数器，再包一层 `input.action_pressed` 记**每个动作**的"问了几次/真了几次"，`-WorldExpr` 用
    立即调用函数把两张表排序拼成一条字符串。一次运行即可回答：场景跑到哪一步、哪些系统在跑、某个动作的边沿
    有没有真送到游戏逻辑（问/真成对，天然带正对照）——正是"旧热路径退役后游戏还跑不跑得起来"最需要的尺子。
  - **下一片**：①把 sim 推过标题（看标题确认是否触发 `scene.load`、`ui.input` 是否随之开始跑），进入玩法态后
    再验 `pause`/`inventory`/`quest`；②顺带回答"sim 只走到标题"是启动链在 Rust 场景里只到这一步，还是需要
    宿主推一把；③WP7-13 的工作项不变（把输入帧喂入 + dt/时钟推进 + 虚拟键钩子从 `web-runtime.js` 搬进宿主/wasm）。
    局限：本轮未改任何生产代码；无真机；`package.loaded` 里只有 5 个模块带 `update` 字段，以别的名字导出的系统
    不在视野里。
- **输入这条链端到端打通（有正对照）；第 41 轮"投递半失败"的结论被自己推翻；引擎独跑确实在逐步进游戏 Lua（WP7-14）**。
  - **改判**：第 41 轮"按键无反应、`is_down` 恒假"的**每一份**脚本运行日志里都躺着
    `input_script_error="输入剧本里的键名不在线格式键表里：keyl"` + `input_edges=0`。`-InputScript` 用的是
    **线格式键名表**（`rustlua-frame-abi::KEY_NAMES`；`lib.rs:12-16` 明写"词汇表是 **web 宿主的裸名**
    （`w`/`space`/`escape`/`left`…），**不是**引擎 `KeyCode` 的规范文本"），而我写的是**动作映射**里的拼法
    `keyl`（`project.toml` 的 `quest=key:keyl`）⇒ 引擎把整条剧本判非法，连里面合法的 `w:60-400` 也一次没跑。
    第二个错是**我自己解析日志**的口径：取"第一次出现的 `expr=`/`steps=`"=第 30 帧快照（脚本第 60 帧才按下）
    ⇒ 必然全假、`steps=30`——又是"在错误的时刻读数"，这次发生在我的解析脚本里。
  - **用合法键名 + 落定时刻读数，输入层是通的**（路径 B，无 JS 世界）：`diag-l`（`l:60-400`）
    `input_edges=1 steps=240 is_down('l')=true is_down('keyl')=true action('quest')=true`；
    `diag-w`（**对照** `w:60-400`）三项全 false ⇒ **有正对照的判别**。整条链每一环都被点亮：
    `pack/project.toml` → `vfs`（WP7-12 的修复）→ 动作映射 → `ScriptDriver` → `FrameInputProducer`
    （`frame_input.rs:319` 把常驻 `keys_held` 写进线格式）→ `apply_frame`（`frame_input.rs:104-113`
    "按住集合是权威"）→ `InputSnapshot`（`rustlua-input/src/input.rs:1467` 的 `finish_frame` **只清边沿**）
    → Lua `input.is_down` / `input.action`。
  - **第 42 轮未定项有答案**：引擎独跑确实在逐步进游戏 Lua（路径 A 239 帧/`steps=240`，路径 B 209 帧/`steps=210`；
    `rust_scene.rs:220-242` 的 `RustScene::step` → `RuntimeScene::run_script_steps(steps, delta, &self.input)`，
    注释自称"与桌面运行器、编辑器预览、headless 运行器**同一个入口**"）⇒ 就"步进"这一点，第 42 轮"删掉
    `web-runtime.js` 会让游戏彻底不再逐帧运行"的担心是**错的**。同时钉死读数归属：`rustlua-world-expr/do`
    由 `gpu_host.rs:82-94` 解析、`:652-662` 执行、落到 `rust_scene.rs:278-298` ⇒ 跑在**GPU 宿主自己的 Rust
    运行时场景**里，与 JS 世界的 Lua 状态不是同一个（这也是 `-WorldDo` 注入虚拟键读不到的原因）。
  - **还没打通的一段**：`e2e-quest`（`l:60-400`）`mode=Playing`/`action('quest')=true`；`e2e-ctrl`（`w:60-400`）
    `mode=Playing`/`action('quest')=false` ⇒ **输入层为真、游戏逻辑没反应**。下一片逐条排：①jhyt
    `ui/input.lua:171-179` 用的是**边沿**（`action_pressed`），按下在第 60 帧，若那一刻游戏侧每帧 UI 输入还没接上，
    边沿就白过去了（`mode` 是持久量，错过不补）；②游戏那套每帧入口在 Rust 场景里是否真被调到（Lua 侧挂自增
    计数器、两个长度的时间窗读）；③也许要先把场景推到玩法地图。
  - **工具经验**：`-WorldExpr` 可以写**立即调用函数** `(function() … end)()`，一条读数回答多个问题；读数**一律取
    日志里最后一次出现**；驱动那行 `webgl_bundle_rust_input_ok script=…` 在给了 `-ExpectWorld` 正则时
    **不构成输入证据**。本轮未改任何生产代码。
- **投递那半再挖一层：逐帧步进游戏 Lua 的循环也住在待退役的文件里；一次判据失效如实记下（WP7-13）**。
  - **唯一调用点**：批量输入 ABI `lua_input_frame` 全仓库只在 `web-runtime.js:3900` 被真正调用；
    `index.html:53,121-129` 只把导出**转交**给 `window.__RUSTLUA_LUA_BRIDGE`（显式白名单，自己不调），
    `wechat-adapter.js:3304-3309` 同样只转交。
  - **循环也在同一文件**：`web-runtime.js:5154-5182` 的 `FIXED_STEPS_PER_SECOND = 60` /
    `advanceFixedSteps()`（注释自称"Rust `FixedTimeStep::advance` 的等价物"），而 `applyInputFrame`
    的注释（`:3914`）写明"**调用点只有一个：固定步循环之前**"⇒ 输入帧与固定步进是同一条循环的两半，
    **都在预置层**；宿主页只做 `await init(); await start(...)`。
  - **引擎侧时钟无内部推进点**：`game_web/src/host_api.rs` 有 `HostClock`（`clock.now()` →
    `day/minute/hour/minute_of_day/rate/paused`），但全文件无 `advance/tick/update/step`，`game_web` 内
    也无第二处引用；文档写"JS 侧用法：`new HostApi(publish_name)` → 把方法挂到全局表"。
  - **判据失效（如实记）**：拿"游戏时钟是否随帧推进"当心跳，短跑（200 帧）与长跑（`-DriveJhyt`，901 帧）
    比较，三次读数完全相同 `clock=true/分钟=360/天=1/rate=0.0`（路径 A 短跑、路径 A 长跑、路径 B）——
    `rate = 0.0`，**这个时钟是暂停的**，步进与否都不走 ⇒ **分辨率是零**（又是先挑读数再想它能不能分辨）。
    顺带两条有用事实：`clock` 在**两条路径上都存在**（不是预置层提供的）；路径 A 跑满 901 帧后仍是
    `day=1/06:00/rate=0`。
  - **仍未定**："引擎独跑时游戏 Lua 有没有被步进"——静态证据指向**没有**（唯一固定步循环在预置层，
    `-StubJsRuntime` 换成 12 行替身，替身只设 `window.__RUSTLUA_RUST_ONLY_HOST`，
    `scripts/webgl-rust-only-stub.js:15`）；但 WP7-12 的 `d5` 又像**有**（`input.virtual_key('l', true)` 之后
    读到 `is_down('l') = false`，而虚拟键是逐帧状态）。下一片第一件事：在游戏侧挂自增计数器、用两个不同
    长度的时间窗读它。
  - **工作项改写得更准（WP8）**：不是"补一根喂键的线"，而是**把固定步循环从 `web-runtime.js` 搬进宿主/wasm**
    ——每帧 ①`lua_input_frame(RLFI)` ②按 60Hz 有界追帧调用游戏 Lua 步进
    ③`lua_call_global_void('__rustlua_apply_virtual_input')` ④推进 `clock` ⑤写 `__rustlua_dt`；引擎侧已有
    可对接等价物（`rustlua-infra::FixedTimeStep`、`RuntimeScene::run_script_steps(steps, delta,
    &InputSnapshot)`）。**在此之前删掉 `web-runtime.js` 会让游戏彻底不再逐帧运行**——比"键位映射丢失"
    严重，也解释了 WP7-11 的 `drive=absent`。本轮未改任何生产代码。
- **把 `[input]` 段修进 wasm：动作绑定表终于能从资源包里读到；投递那半隔离出来，并量到一条换轨风险（WP7-12）**。
  - **修**：`rustlua-scripting/src/script/api/input.rs` 的 `input.load_settings()` 与 `input.nav_settings()` 两处
    **读**工程设置原是裸 `std::fs::read_to_string`，wasm 上恒 `ENOSYS` ⇒ 动作表整表丢失。改成走
    `rustlua_infra::vfs::read_resource`（`vfs.rs:168-182` 的文档就叫它"**引擎里读工程文件的唯一入口**"，并明写
    "别自己先 `fs::read` 再 vfs"）。宿主本就把 `pack/project.toml` 预加载进资源表（`lib.rs:240-265`，
    `manifest.txt:1038` 有它），provider 把 `/project/project.toml` 归一成资源键（`lua_bridge.rs:245-258`）。
    **写侧没动**（那是"保存设置"，另一件事）。
  - **非像素判据**（重新打包 jhyt webgl，`wasm_host_ok … bytes=5783047`）：`[input] project.toml` 从
    `false (operation not supported…)` 变成 **`true`**（A、B 两条路径都是）；`input.action_map_text()` 里
    **含 `quest=key:keyl`** 且 **含 `pause=pad:5`**——后者是 jhyt 文件特有绑定（引擎默认是 `key('p')`，
    `rustlua-input/src/input.rs:785`）⇒ 证明表是**从 `pack/project.toml` 装进来的**，不是默认值兜的。
  - **预注册预期被证伪**：我先写"路径 B 会翻到 `Quest`、路径 A 不变"，实测两条都是 `Playing`、图与无输入对照
    **逐字节相同**。原因是我把两条输入通道当成了同一条：`-InputScript` 喂的是 **Rust 侧仿真**
    （`frame_input.rs` 模块注释：`RLFI` → `InputSnapshot` → `RuntimeScene::run_script_steps`；日志
    `[gpu] presented … scene=rust`），而 `-WorldExpr` 读的是**游戏自己的 Lua 世界**⇒ `is_down('w'/'l'/'s')`
    全 `false`；`-WorldDo "input.virtual_key(...)"` 也读不到——虚拟键是**逐帧**状态而 `-WorldDo` 是一次性求值，
    落定后又跑 200 帧（**这条探针本身设计错了**：在错误的时刻读数）。
  - **结论改写得更准**：引擎侧**有**注入接口（`input.virtual_key`/`virtual_pressed`，`input.rs:161-179`），缺的是
    **宿主每帧把 DOM/触摸喂进游戏那个 Lua 世界的输入快照**——正是旧热路径在做的事（`web-runtime.js:728` 听
    `keydown`、`:1648-1691` 维护 `__rustlua_input`、`:4512-4522` 自带键表、`:4527-4557` 覆盖 `input.action*`）。
  - **换轨风险（新量到）**：两条路径的键表**不是同一张**——`move_up/down/left/right`、`pause`、`interact`、
    `inventory`、`attack`、`skill`、`respawn` 在 jhyt 的 `project.toml` 里是**纯手柄**绑定，而引擎默认表/预置层
    硬编码表把它们绑在 `w/s/a/d`、`p`、`e`、`i`、`j`、`k`、`r`；`load_settings` 只对**文件里没写**的动作补默认
    （`input.rs:349` 的 `merge_missing`）⇒ 修复后引擎独跑照 `project.toml` 办事（`quest` 有效、`pause`/`move_*`
    键盘无效），旧路径相反。**换轨时必须显式决定**。
  - **回归安全**：`cargo test -p rustlua-scripting` 117 个测试全绿（116 单元 + 1 集成）；`cargo fmt` 干净；wasm
    release 构建通过（打包日志 `wasm_host_ok`）。局限：只修"读设置走 vfs"，写侧与 `audio` 侧同类读写未动；
    端到端"按键→开任务面板"未在无头环境打通；微信侧 `project.toml` 是否随主包落地未验。
- **渲染四层都对上了，但输入/玩法没对上：旧热路径仍在替引擎做"键→动作"，根因钉到代码行（WP7-11）**。
  - **探针 1**（合成按键 L = jhyt 的 `quest` 动作，`project.toml:20` 的 `quest=key:keyl`）：`-InputScript
    keyl:60-70` 两条路径的整页图都是 `8C57FAB1B1EB6094`（= **无输入待机基线**的哈希）、`get_mode()=="Playing"`；
    而 `ui/quest_ui.lua:353` 的 `M.open()` 会**无条件** `state.set_mode("Quest")` ⇒ 它根本没被调用 ⇒ 按键没到动作层。
  - **根因（两条路径都读不出来）**：日志里 `[input] /project/project.toml: read failed: operation not supported
    on this platform` + `[input] project.toml: false (…)`——`script/api/input.rs:318-353` 的 `input.load_settings()`
    用 `fs::read_to_string(<root>/project.toml)`，wasm 只读 VFS 上没有这个文件 ⇒ 走 `read failed` 分支、**项目
    自定义动作绑定整表丢失（只剩默认绑定）**；jhyt 的动作层包的正是引擎这一层。
  - **以前是谁在替**：`web-runtime.js:4554` 自己实现 `input.action_pressed`（注释"必须实现而非缺失"）、`:728`
    监听 `window.keydown`、`:5062`/`:4843` 的 `dispatchDriveKey` 与 `?rustlua-drive=jhyt`（"驱动走**真实输入
    路径**"）。
  - **探针 2（驱动自带的完整玩法剧本）**：路径 A **启动了**并真的驱动了游戏（`drive settled: failed`，
    `phase=nav-exit frames=901 world=manor map=manor alive=231 player=J…`，截图 220,858 B 的真实画面；`nav-exit`
    段超时属 WP7-04 记过的既有抖动）；路径 B 是 `timeout waiting for drive`／`drive_jhyt: absent`——剧本**根本
    没启动**，因为它是预置层 JS 消费的。
  - **对 WP8 的硬信息**：渲染可以照删；**输入/玩法不能**。要补两件事：①让 `[input]` 段在 wasm 里能到引擎
    （随资源清单发过去，或给 wasm 宿主加"设置文本"通道，`game_web` 已有 `__RUSTLUA_HOST_API` 这类注入先例）；
    ②宿主把键事件真正喂进引擎快照（`frame_input.rs` 的 `InputScript` 已证明引擎侧这条路是通的）。
  - **两个"读数本身不可用"的坑**：①`-KeyProbe` 读的 `frames/dom_events/input_edges` 取快照在**落定之前**
    （两次都是 `frames=0`），回答的不是我要问的时刻；②`-InputScript '-'` 不是"不给输入"的写法，会被当脚本
    解析（`exit=2`、日志 1.3 KB 结束）——空值就该不传参数（第 39 轮已吃过空串那次）。本轮未改任何生产代码。
- **jhyt 自己的复杂 UI（暂停／帮助／设置）在有无旧热路径下逐字节相同；正对照改用游戏自己的公开查询（WP7-10）**。
  - **做法**：入口从源码读出来（不是编的）——`ui/pause_menu.lua` 导出 `M.open()`(976)／`M.open_help()`(720)／
    `M.open_settings()`(518)，而 `ui/input.lua:173` 正是玩家那条路；用 `-WorldDo` 调它们，两条路径各截一张整页图。
  - **读数**：暂停根页 12,343 B／`D4E1BA6317C533D6`、帮助页 14,025 B／`ACE11D9EFB95B256`、设置页
    12,343 B／`D4E1BA6317C533D6`——**三个状态两条路径全部同哈希**；三张都与"无 WorldDo 基线"（20,835 B）不同
    ⇒ 探针不空转；帮助页与暂停页哈希不同 ⇒ 这层 UI 确实**状态相关**。
  - **正对照升级为非像素**（第 38 轮教训：写通道签名一错就静默什么都不做）：用 `ui/pause_menu` 自己导出的
    `settings_active()/help_active()/ui_active()` 回读——帮助页 `false/true/true`、设置页 `true/false/true`
    （路径 A、B 一致）⇒ 状态真的切了，且**路径 B 也能正常 require 并查询 jhyt 的模块**。
  - **如实记下的异常**：`open_settings()` 回读确认 `settings_active()=true`，但截图与暂停根页**逐字节相同**；
    两种可能本轮无法分辨（设置子页要等一次输入步进才重绘／这一帧两页确实一样）。两条路径在这件事上同样一致，
    不影响"旧热路径画面贡献为零"，但**不能**据此说"设置页渲染正确"。
  - **又一个参数坑**：基线那次传 `-WorldDo ""`（空串对照），PowerShell 向原生程序传参**丢空串** ⇒ CDP 位置参数
    整体错位 ⇒ 那次运行 `exit=1` 且 `expr` 读数**根本没出现**（`webgl-cdp-wait.js:28` 早写过同类警告）；
    与 WP7-07 同族——**读数缺失 ≠ 读数为负**。
  - **意义与局限**：浏览器侧已测到四层（场景／UI 面板／矢量文字／jhyt 自己的多页面 UI）撤掉旧热路径后画面
    逐字节不变；局限是只走了"打开某一页"这一帧（没推进交互）、设置页异常未解释、微信仍无真机。本轮未改任何
    生产代码。
- **UI 层也对齐了：面板与矢量文字在有无旧热路径下逐像素等价；顺带记下两个"写通道静默空转"的陷阱（WP7-09）**。
  - **做法**：用驱动自带的 `-WorldDo`（经 URL 查询串 `rustlua-world-do=` 在**页面内**生效）造确定性 UI 元素，
    两条路径各截一张整页图比。**每个探针必须自带正对照**——这个写通道不设防，Lua 签名一错就静默什么都不做，
    没有对照时"画面没变化"会被误读成"这一层引擎不支持"。
  - **面板探针** `ui.panel('r38',60,60,400,200,1,0,0,1)`：基线两侧纯红像素都是 **0**（判据不空转），加探针后
    两侧都是 **79,829** 像素、包围盒都是 **400×200**（A @(60,68)、B @(76,60)）⇒ 像素数完全相同（连块内被其它
    UI 压住的部分也一样），唯一差别是 bbox 偏移 **(16,−8)**——与 WP7-08 从另一条路测到的"运行间视口抖动"
    同一个量，互相独立地确认了它。
  - **矢量文字探针**（引擎一直 warning"设备没有存储缓冲…矢量文字与 3D 网格/阴影两条路不构造"）：一次
    `-WorldDo` 里放绿面板（正对照）+ `ui.text_at('r38t',80,320,'RUSTLUA38',3.0)`（签名读自 `ui.rs:38` 与
    `web-runtime.js:2753`）⇒ 绿对照两侧同为 **60×40 @(5,13)／2,295 像素**，整页图两侧同为 756×441／21,306 B、
    **MAD 0.000**；再用"只画绿面板"的运行做差集，**两侧都是 1,278 个差异像素、足迹 (80,334)–(247,358)** ⇒
    文字在两条路径上都真的画出来了，位置与像素逐项相同，那句 warning 没有让文字消失。
  - **两个空转陷阱（本轮自己踩到并记下）**：① 第一版文字探针按 9 个参数调用（真签名 4+2），两次运行与基线
    **哈希完全相同**——只看"文字没出现"就会得出"引擎不支持文字"的错误结论，判它空转的依据是"与基线逐字节
    相同"；② 用 `-WorldExpr` 读 `__rustlua_ui_texts['r38t'].text` 两侧都回 `no-prelude-store`——它是预置层
    chunk 的**局部变量**（第 35 轮同一课），探针要看像素/读数，别碰实现内部的名字。
  - **意义与局限**：旧热路径在浏览器上对画面的贡献，测过的三层（场景／UI 面板／矢量文字）**全是零**；局限是
    只有启动场景、UI 探针是简单元素（jhyt 自己的复杂 UI 没进过）、微信仍无真机。本轮未改任何生产代码。
- **像素级对拍：最重的一屏上撤掉整条旧热路径，画面逐字节不变；顺带否证自己"屏上全是 Canvas2D"的旧说法（WP7-08）**。
  - **做法**：同一份 jhyt 产物、同一场景与路由、落定 240 帧，用驱动自带的 `-Shot` 抓**合成后的整页**
    （`webgl-cdp-wait.js:337`；WebGL 画布默认 `preserveDrawingBuffer:false`，只有合成器留最后一帧）。路径 A =
    默认（旧热路径在场），路径 B = `-StubJsRuntime -NoSceneVerify`（后者是第 36 轮的教训）。
  - **读数（5 次运行／2 条路径）**：`jsworld2` / `engine` / `engine2` / `village-*` 四张 **SHA256 完全相同**
    （`8C57FAB1…`、756×441、20,835 B）；配对小矩阵里 A 第二次 vs B 第一次、B 两次之间全是 **MAD 0.000**
    ⇒ 这一屏上"有没有整条旧热路径"**逐像素无差别**。
  - **例外恰好是最好的对照**：A **第一次**运行视口是 756×**407**（其余 441），它与**同路径**的 A 第二次之间
    也是同一差异（最佳平移 (16,−8)、MAD 20.456→1.225，94% 的差被一个整数平移解释掉）⇒ **运行间视口抖动**，
    不是路径差异。教训：截图抓的是视口且尺寸运行间会变，像素比对要么钉死窗口、要么"平移搜索 + 同路径对照"。
  - **两处自我否证**：① 我此前重复过的"屏幕上画的全是 web-runtime.js 的 Canvas2D"在本屏被推翻（撤掉整个
    JS 世界，画面逐字节相同）；② `-SimScene village` 那组是**空转**——六次运行的启动日志全是
    `startup_scene=scenes/main.scene`（`-SimScene` 管的是 JS 世界的仿真场景），是靠把五张图**哈希**并排看
    才发现的，不是凭"尺寸差不多"。
  - **没覆盖**：只有启动场景；jhyt 的交互页（帮助/背包/对话这类 Canvas2D UI）没进过 ⇒ 该结论**只对这一屏
    成立**。下一轮已定：用页面内生效的 `rustlua-world-do` 造一个确定性 `ui.panel` 大红块，两条路径各截一张，
    看那层 UI 到底有没有落到合成图上。本轮未改任何生产代码。
- **存档"换路径会不会丢档"被实测划掉：键／行层／值层／平台槽四项两侧一致（WP7-07）**。
  - **假设与推翻**：`web-runtime.js:2875-2885` 的注释说值层"引擎侧走 serde_json，这里用长度前缀记法
    **而不是 JSON**"，代码侧确实如此（`script/api/save.rs:62-68`）⇒ 推断"换路径读不回旧存档"。
    **实测推翻**：探针 `probe_save_format()` 让两侧各写固定表、再用 `save.read('__blob:<key>')` 取
    **原始行**（不经解码）：`{n=1,s='x'}` 两侧都是长度 15、前 24 字节 `7B226E223A312C2273223A2278227D`
    —— 那正是 `{"n":1,"s":"x"}`，**预置层那侧写的也是 JSON**，注释已过时。
  - **风险形状也一致**：`{n=1,f=1.5,s='a\tb\nc',cjk='江湖',[3]='three',[-1]='neg'}` 两侧长度 67／校验和
    `142A`，读回后 `t[3]`/`t['3']`/`t[-1]`/`type(t.f)`/制表符换行/CJK 全部相同 ⇒ 整数键退化在**两侧一样**
    （"web 比 PC 更保真"只对 PC/std::fs 那条通道成立，对本次迁移不成立）。
  - **端到端**：从**浏览器侧**独立读平台槽（不信 wasm 自报的 `commit=true`），两条路径都是
    `storage probe: rustlua:save=15hdr:ok`、`exit=0` ⇒ 键（与 `lua_bridge.rs:204`"与旧运行时逐字相同"
    一致）、长度、格式头全同；两侧都是**引擎自己**在写，经 `SaveSlotProvider`（wasm 上 `LocalStorageSlot`
    → 浏览器 `localStorage`／微信 `wx.setStorageSync`）。
  - **坑（记进门）**：第一次跑引擎路径时日志里**没有** `storage probe:` 行，看着像"存档没落盘"，其实是
    `-StubJsRuntime` 下 JS 世界不启动、`data-rustlua-scene-verify` 缺失 ⇒ 驱动在
    `webgl_bundle_scene_verify_failed` 处**提前结束**，探针压根没跑；补 `-NoSceneVerify` 后两条路径都
    `exit=0`。⇒ **"没有这一行"不等于"这件事没问题"**，读数缺失必须与读数为负分开报。
  - **影响**：存档从"宿主服务待交接"里划掉（不需要转换层）；WP7-06 那个待改嫁的
    `__rustlua_save_storage_key` 也更简单——门要的槽键就是 `rustlua:save*`。
  - **局限**：只覆盖小载荷（15／67 字节）；jhyt 量级真实存档未测；`save.slot()` 两侧都返回 `nil`；
    微信侧仍是适配层模拟、无真机。本轮未改任何生产代码。
- **真项目活体对拍：把整条旧热路径换成 12 行替身，jhyt 读数逐字不变；门对预置层的隐藏依赖只剩 1 个名字（WP7-06）**。
  - **为什么**：WP7-04 证的是"预置 Lua 相对引擎的 API 净增量 = 0"（静态对账），还差最硬的一问：
    **把整条旧热路径换掉，真项目（jhyt，175 个 Lua 文件）还跑不跑得起来、跑得一样不一样？**
  - **被测对象就是出货形态**：`package.ps1:936` 复制 `plugins\webgl\web-runtime.js`（源 **285,696 B** /
    5,352 行，git-clean）后由 terser 压缩（`:942`）⇒ 产物里是 **188,749 B** 那份；两次读的都是产物里的。
  - **读数**（同一份产物、同一套驱动参数，路径 A 默认 vs 路径 B `-StubJsRuntime`——正主换成 12 行 /
    1,452 B 替身）：启动轨迹（`started: game_jhyt; manifest files=1200; startup_scene=scenes/main.scene`、
    `sim_boot_route=engine-startup`、`[scene] loaded name=main entities=0`、`[script] 主入口门面就绪：main`）
    与世界/脚本指纹（`draw_calls=16 triangles=206 draws=16 script=true steps=30 spawned=18
    script_errors=0 world=no-player step_errors=0 scene_delta=+0,+0,+0 textures=14/15`）**逐字相同**；
    `avg_lua_ms` 0.32 → **0.22**。路径 B 唯一的红是 `webgl_bundle_scene_verify_failed scene_verify=absent`
    ——按示例场景形状写的断言，不是行为差异。
  - **静态审计**：预置层的 **111** 个疑似全局名（已剔表成员与 `local`/`for` 变量）与 175 个用户 Lua 文件的
    **真·全局引用**交集 = **0**；候选命中全是用户自己的局部变量/表字段。**自我否证**：第一版按任意标识符
    计数得出"用户重度依赖预置层"（`get 270 / count 523 / state 722` 处）的假结论——`x.get(...)` 的 `get`
    被当成全局引用，与 WP7-04 把 `for` 变量当成"引擎缺表"同一类自伤 ⇒ 计数口径必须能对**行原文**自证。
  - **隐藏依赖**：门/适配层引用的 `__rustlua_*` 里由预置层定义成全局的只有 **1 个**——
    `__rustlua_save_storage_key` ×3（运行门读存档槽键）；适配层唯一命中 `__rustlua_apply_virtual_input`
    是**注释**；`wasm-bridge.js`／`verify-webgl-bundle.ps1`／`web-api-contract.js` 均为 0。4 个测试导出
    钩子（`__rustlua_export_ui`/`world_summary`/`tilemap_layers`/`export_scene_text_hidden`）由预置层定义
    （`web-runtime.js:2778/2836/3373/3403`）但仓库里没有真实调用方。
  - **对 WP8 的收敛**：Lua 侧剩余代价 = ①1 个名字改嫁 ②46 处无条件覆盖的行为对拍（已 25/56 + 真项目整体）
    ③渲染与宿主服务（微信 WebGL2 等设备读数）。
  - **局限**：只跑 30 步、只跑默认脚本路由；jhyt 的驱动用例（移动/触摸摇杆/新游戏/存档持久化/帮助页/层序）
    **没有**在路径 B 上跑；替身只证明"这条 JS 与它注入的 Lua 层对上述读数不必要"，不是逐行为等价。
- **把"微信给不给 WebGL2"做成设备上一次运行就能读到的读数；顺带量出 GPU 通道进微信的 wasm 成本（WP7-05）**。
  - **为什么**：WP7-04 把 WP8 的卡点收敛到"微信上引擎渲染器能不能跑"，而 `gpu_host.rs` 要 WebGL2；
    harness 的假画布只实现 `2d`，这个问题在无头环境里**问不出来**。于是不猜，改成仪器。
  - **做法**（`plugins/wechat-minigame/wechat-adapter.js`：`0.7.3+fs-route` → `0.7.4+gl-probe`）：新增
    `probeGlCapability()`，按 `webgl2` → `webgl` 各用**一张各自的探针画布**试 `getContext()`，拿到就
    顺带读 `VERSION`/`MAX_TEXTURE_SIZE`/`MAX_RENDERBUFFER_SIZE`/`WEBGL_debug_renderer_info`；读数进
    `diagSnapshot().gl`，并导出 `__RustLuaWechat.glCapability()` 供真机控制台直接取。
  - **两条硬约束**：**绝不用主画布试 GL**（一张画布只能有一个上下文——WP5 实测，在主画布上试
    `webgl2` 会把已经到手的 `2d` 上下文毁掉，画面直接黑）；探针画布优先
    `wx.createOffscreenCanvas({type:'webgl'})`，拿不到才退回"再调一次 `wx.createCanvas()`"，都拿不到
    就如实留 `note`，不猜。
  - **读数**：默认形态 `{"probed":false,"main_canvas_skipped":true,"note":"probe-canvas-is-main",…}`
    ——**安全守卫按设计生效**；`--host-canvas offscreen`
    `{"probed":false,"source":"wx.createCanvas(offscreen)","webgl2":false,"webgl":false,"note":"",…}`
    ——**真去探测了**，假画布只实现 2d 所以如实报 false。两次 `harness exit=0`、`boot=running`，
    `wechat_asset_routing_test_ok cases=6`。
  - **外部证据（只到"有这回事"）**：官方文档有一页
    [WebGL2.0渲染支持说明](https://developers.weixin.qq.com/minigame/dev/guide/game-engine/unity-webgl-transform/Design/WebGL2.html)
    （正文 JS 渲染、抓不到），社区问答有"旧版本微信不支持 WebGL2，提示请升级微信"⇒ 判断是**平台支持、
    与客户端版本/机型绑定**，所以交仪器而不是交结论。
  - **顺带量到的成本**：同一个烟测工程，第 32 轮微信分包 wasm `wasm_host_ok bytes=2641639`（2,580 KB），
    本轮 `pack/game_web_bg.wasm = 5,335,507 B`（分包 25.4%）——第 33 轮 `-Gpu2d` 构建的核心更大、被
    打包带上。⇒ GPU 通道进微信的 wasm 成本**翻倍（+2.55 MB）**。
  - **局限**：真机 `gl.webgl2` 仍**未读**（无设备）；无头侧只证明两条分支都会走且不崩，不证明平台能力。
  - **顺带量到的一条硬约束**：`-AssetPack full`（非分包）形态现在**直接失败**——
    `微信小游戏主包超限: 5,683 KB > 4,096 KB`，gpu2d 核心一进主包就超限。⇒ 微信侧要带引擎 GPU 通道
    **必须**走分包；同工程分包读数 `main_kb=486`（11.9%）／`subpackage_kb=5,211`（25.4%）、
    `harness exit=0`、`boot=running`、`fs_route={"tried":2,"hits":2,"misses":0}`。本轮**不主张**全量布局
    的门读数（打包在写 launcher 前就被限额拦下，那次跑的产物不完整）。
- **微信运行门在出货（分包）形态下第一次全绿：统一资源管线 + 解掉首屏清单加载闭环（WP7-03）**。
  - **症状**：分包形态下 harness 报"启动期 6 次 `pack/` 读取被未挂载的分包挡下"，而适配层自检同一
    时刻读的是 `mainpack/manifest.txt`、报 `ok@mainpack`——同一个文件、两条读者、两套语义。
  - **根因**（用临时诊断副本抓到的完整栈）：被挡的 6 次**全部**来自适配层自己的
    `releaseKey → loadManifestChecksums → readText('pack/manifest.txt')`，且发生在**首屏清单的加载
    窗口内**：`cachePathFor`（user-data 缓存候选）要算发布标识、发布标识要读 `manifest.txt`、而它读
    哪一侧**取决于正在加载的清单** ⇒ 闭环；内层看到 `firstScreenLoaded=true` 但 `firstScreen=null`
    ⇒ 原样去打 `pack/manifest.txt` ⇒ 分包挂载前被挡。诊断计数 `fs_route.skipped_no_manifest`：
    修前 2、修后 0。
  - **修复**（`plugins/wechat-minigame/wechat-adapter.js`，v0.7.2 → v0.7.3+fs-route）：① 给
    `wx.getFileSystemManager()` 加一层**路由包装**（同步 `readFileSync`/`accessSync`，按首屏清单把
    `pack/<x>` 路由到 `mainpack/<x>`；候选全失败**回落原调用**、错误文案逐字不变），使
    `web-runtime.js` 里直接问宿主 FS 的读取也走同一张表，而不是只有适配层自己的通道走路由；
    ② 清单加载窗口内跳过 `user-data` 候选，并在安装时**立即**加载一次清单。
    包装状态不写在宿主对象上：harness 会枚举 FileSystemManager 的成员并拒绝未实现的名字
    （"不能让它静默降级"）——第一版把标记记在 FS 对象上，当场被这条守卫抓住，**守卫自己成了定位
    工具**。`plugins/webgl/web-runtime.js`（旧热路径本体）与用户正在编辑的 harness 一行未动。
  - **读数**：切分（出货）布局 `harness exit=0`、`wechat_minigame_harness_ok path_style=root
    frames=120 draw_ops=483 draws_per_frame=7 pack_reads=2`、`boot=running`、
    `fs_route={"tried":2,"skipped_no_manifest":0,"hits":2,"misses":0}`；整包（full）布局同样
    `exit=0`（回归）；仓库自带单元测试 `wechat_asset_routing_test_ok cases=6`（含"无清单时用旧路径"
    与"读清单不得无限递归"）。
  - 烟测工程瘦身到启动链所需：`main_kb=379`（占上限 9.3%）、分包 2,581 KB、首屏 `entries=6`。
- **微信启动链第一次跑起来：`boot=running`；顺带纠正两个测量错误（WP7-02）**。上一片把"微信上跑
  不起来"归给了预置表覆盖不足，本轮把它拆成可判定读数，结论反过来。
  - **仪器错误（撤回）**：`--probe-host-api` 用 `rawget` 逐段走 `_G`，看不见元表上的 `__index`
    回退 ⇒ "absent" 只说明"不是自有键"；我加的 `ui.__never_exists_zzz__=absent` 对照**区分不了代理**
    （代理挂在元表上时 rawget 同样报 absent），那条"`ui` 是真表、不是代理"的结论因此撤回。
  - **两路径逐位对拍**：同一份 `examples/game_api` 包，77 个被调用的 `ui.*`，引擎路径
    （`-StubJsRuntime`）与 JS 世界路径**两侧都是 45/77、位串逐位相同、零差异**；32 个名字
    （theme/container/layout/signals 家族）在引擎代码与引擎文档里都不存在 ⇒ 那个示例工程与当前引擎
    **不同步**，它在两条路径上以**逐字相同**的错误失败（`demo_index.lua:130 bad argument #6 to
    'format'`）。机制：`ui.scroll_max` 在引擎侧是真函数，而预置表没注册它 ⇒ 两侧都落到回退上，
    而回退（`__rustlua_missing_proxy`，`web-runtime.js:4254-4267`）**可调用且返回表**——
    "调用没实现的 API 不报错、悄悄拿到一张表"。
  - **微信里跑起来了**：新建 4.9 MB 烟测工程（只用 5 个预置表覆盖内的 `ui.*`），打包
    `main_kb=394`（占上限 9.6%），harness `--frames 120` 拿到 **`"boot":"running"`、120 帧**；
    断言只剩一条红。顺带修掉烟测工程自己的 `font = ""`（真工程写法是资源路径）。
  - **又一次自证伪**：那条红看着像"首屏闭包漏列资源"。我在 `scripts/` 放临时诊断副本量出被挡的
    读取，结果是 **`6 x pack/manifest.txt`**——同一个文件读 6 次，而它**就在** `mainpack.json` 的
    entries 里 ⇒ **闭包是对的**，对不上的是**读的那一侧**（`pack/manifest.txt` 没被路由到
    `mainpack/manifest.txt`）。要修的是适配层的主包路由/读取次序，不是首屏打包器。
  - 另记：`game_web::start_wechat`（`lib.rs:331-365`）只做诊断、不渲染（自述"未接线"）⇒ 微信端画面
    目前仍 100% 由 JS 世界承担，这是 WP8 的正题。
- **微信包"跑起来"的第一份读数 + 首屏打包器放行引擎内置哨兵（WP7-01）**。计划里"微信侧没有
  '在微信里跑起来'的证据"这一条，本轮用**小工程**打通了链路（不动工程/发布取舍）。
  - **真 bug**：首屏打包器把启动场景里的 `__white__` 当成**悬空引用**直接拒绝发布，于是
    "工程里只要有一个没贴图的纯色 Quad，微信包就永远发不出来"（`examples/game_api` 实测卡死）。
    `__white__` 是引擎内置 1×1 白纹理（`render/frame.rs` 的 `WHITE_TEXTURE_PATH`），渲染器就地合成，
    清单里本来就不该有它。修法：`scripts/first-screen-pack.js` 放行引擎内置哨兵（`__name__` 约定）。
    定点验证两向都做了：`__white__` ⇒ `first_screen_pack_ok`/`exit 0`；真缺图
    ⇒ `first_screen_pack_warn`/**`exit 1`**（对照真的红，门没有被削弱）。
    ——**顺带记一次假绿**：第一版 fixture 的场景行行首写成 `entity`（打包器要求 `component`），
    判定根本没走到，主张与对照都"通过"了。
  - **第一次跑通微信包**：静态门全绿（`wechat_minigame_package_ok main_kb=633`，占上限 15.5%；
    分包切分／代码包白名单／ECMA 门／配额都过），`-SelfTest` 真把 `game.js` 跑了起来。
  - **运行门是红的，红在工程脚本里**：`data-rustlua-boot=failed` +
    `scripts/demo_index.lua:130: bad argument #6 to 'format' (number expected, got table)`。
    同帧事实：`#6` 对应 `ui.scroll_max(...) or 0`；引擎该函数返回**数**（`ui_nav.rs:187`），
    JS 预置表**没有**注册 `ui.scroll_max` ⇒ 两侧都没有"返回表"的实现，**这条没查到底**，
    下一步在真 VM 里取 `type(ui.scroll_max)`。
  - **卡点量清（未擅自改）**：jhyt 微信主包 `19,715 KB > 4,096 KB`（记录基线 `19,673 KB`，
    **+42 KB** 全部来自这一系列 wasm 侧改动，占首屏 19.3 MB 的 0.2%，不是回归）；主包构成是
    工程首屏 238 个文件（60 张预制体贴图）；既有产物已不可验证（早于"内核搬进分包"那次改动）。
- **实体句柄的跨运行时歧义：把"旧存档的实体引用"变成可见读数（WP6-09）**。上一片查出的架构事实是：
  `entity`/`transform` 这套在 web 旧运行时里由**它自己的 Lua 预置表**实现（实体 id 从 1 开始发号），
  遮蔽引擎同名表；而工程把实体 id 存进 `map.state`（`building.lua` 的 `rec.entity`），读档时靠
  `if rec.entity and entity.is_alive(rec.entity) then` 判断。于是关键问题不是"少哪个 API"，
  而是**同一个 API 在两边的边界语义**。
  - **一次被否证的推断**：先按句柄编码推断"小整数 id 会抛 `invalid entity id`"，据此写的单测在空世界里
    通过；真 wasm + 真工程脚本一跑就翻成 `true`——`generation` 只占低 32 位、`INITIAL_GENERATION = 1`
    ⇒ **引擎自己的第一个实体句柄就是 1**，与外来小整数**逐位重合**。单测没抓到、浏览器抓到了。
  - **修的部分（只动谓词）**：`entity.is_alive` / `entity.persist_id` 改用宽容解码——负数与
    `generation == 0` 的整数回答 `false` / `nil`，而不是把存档哨兵值（`0` / `-1`）变成脚本崩溃；
    **写操作继续严格**（`transform.set(0,0,0)` 仍报错）。行号漂移按仓库做法跑生成器对齐 Lua API 文档桩
    （`--check` 转绿，正文未动）。
  - **不能单方面修的（决策项）**：旧存档的实体 id 与引擎 `index == 0` 的句柄无法区分，
    `is_alive` 返回 `true` 可能是**另一个实体**。改句柄编码属破坏性变更（现有 Rust 侧存档里的句柄会失效）；
    更稳的出路是工程把实体引用换成 `persist_id` 字符串（`entity.find_id` 已支持）。本片只把歧义
    变成可断言读数，不擅自改编码。
- **平台音频出口：让引擎在 wasm 上真的响（WP6-08）**。退休清单里的"音频"与"存档"是同一类缺口，
  而且更深：引擎的播放入口基于 rodio——先 `std::fs::read` 把音频读成字节，再 `ensure_device()` 找设备。
  **wasm 上两头都不成立**（没有可读文件系统、也没有 cpal 设备），于是 `audio.play()` / `audio.music()`
  在 web/微信上恒失败，**游戏全程静音**（旧运行时自带 `new Audio()` / `wx.createInnerAudioContext` 两套通道绕过去）。
  - **做法**：把"声音交给谁放"从引擎里剥出来做成平台出口——新增 `rustlua-audio::sink`
    （`AudioSinkProvider::play(AudioRequest)`，请求里只有"引擎知道、平台需要"的东西：名字、总线、
    已算好的有效音量、资源目录、是否循环）。`play_on`/`play_music` 在最前面分叉，装了出口就**整段交给平台**，
    不再读字节、不再找设备；`stop_music`/`pause_music`/`resume_music` 同样分叉，写法与 `vfs`/`save_slot` 一致。
  - **一份 Rust 代码覆盖两个平台**：微信走 `wx.createInnerAudioContext()` + 适配层已暴露的
    `__RustLuaWechat.assetSrcFor()`；浏览器走 `document.createElement('audio')`。扩展名次序
    `mp3 → wav → ogg` 与 URL 规则 `pack/<逻辑路径>` 与旧运行时**逐字一致**（否则同一个名字两边响的不是同一个文件）。
  - **存在性用引擎自己的资源表同步判**：平台加载是异步的，等 `error` 事件回来时 `audio.play()` 早就返回了——
    同步判一次才能把"缺文件"与"平台播放失败"分开（前者是 `Io`，后者是 `Device`）。
  - **两个互相独立的读数**：wasm 自报 `audio=<次数> last=<名字> src=<路径> err=<次数> via=<web|wx> music=<playing|stopped|none>`；
    CDP 从**页面本身**读 `window.__rustluaAudioElements` / `__rustluaAudioErrors`（`-AudioProbe any|none`）。
    元素数组既是保命引用（`new Audio()` 被 GC 掉表现为"偶发没声音"），也是外部证据；自动播放策略的拒绝
    会记进错误数组——浏览器里最常见的"没声音"必须能被看见，而不是静悄悄。
  - **证据**：Rust-only 路径真播一个工程音效，浏览器侧 `elements=2 errors=0`，其中一个是
    `.../pack/assets/audio/bgm_title.mp3|paused=false|loop=true|vol=1`——**工程自己的 BGM**，
    即 `core/audio.lua` 的播放请求经 Rust 引擎落到了浏览器；读数侧 `last=sfx_harvest src=assets/audio/sfx_harvest.mp3 err=0 via=web`。
    另加引擎侧集成测试：资源目录**故意指向不存在的地方**，`play_on`/`play_music` 仍必须成功
    （只要还有代码偷偷读文件就会红），并钉住有效音量 = 主音量 × 总线音量、总线与循环标记、资源目录。
  - **顺带补掉两处"只播不停"**：`stop_music` 原先只停 rodio sink（wasm 上恒为 None ⇒ 切图后旧 BGM 一直响），
    以及第二次 `play_music` 不会停上一首（两首 BGM 叠着响）——两者在平台出口里都按引擎既有语义做了。
  - **探针立刻抓到一处噪声**：主动停掉正在播放的元素时，浏览器会把那次未完成的 `play()` 以
    `AbortError` 拒掉（旧运行时为同一件事立过 `__rustlua_released` 标记）。照做豁免之后，错误数组
    才是"真出问题"的信号——否则真正要看的自动播放策略拒绝（`NotAllowedError`）会被噪声淹掉。
    微信侧同时补了 `onError` 挂载。
  - **未完成**：① **"听得见听不见"无法在无头环境验证**（自动播放策略/音量/设备都取决于真机），
    本片验的是"播放请求确实到了平台、元素在加载/播放且没有报错"；② 微信分支**没有环境验**
    （`wx` 只在真机/开发者工具里存在），只做到"编译通过 + 浏览器分支实测"；③ 引擎的 `audio.play`
    仍不给 Lua 返回值（旧运行时会回 `(false, 原因)`），属行为对齐的后续项。
- **平台存档槽：让引擎的存档在 wasm 上真的落盘（WP6-07）**。退休清单里的"存档"实际缺口比"少一个 API"深：
  引擎的存档是"内存表 + `commit`/`load` 落盘"两段，落盘那段直接走 `std::fs` —— 桌面没问题，
  **wasm 上没有可写的文件系统**，于是 `save.commit()` 恒 `false`、`load()` 恒 `false,"missing"`，
  玩家进度永远存不下来。
  - **做法**：把"存到哪儿"从引擎里剥出来做成平台字符串槽——新增 `rustlua-infra::save_slot`
    （`SaveSlotProvider` 只做两件事：读一个键、写一个键）。装了槽走槽，没装走文件系统 ⇒ 桌面**零行为变化**。
    存储位置按平台分叉，写法与 `vfs` 一致（桌面 `OnceLock`+`Send+Sync`；wasm `thread_local`，
    因而可捕获 `web_sys::Storage`）。`game_web` 装 `localStorage` 槽：微信侧由 `wechat-adapter`
    把 `localStorage` 映射到 `wx.setStorageSync`，**一份代码覆盖两个平台**。
  - **键名与旧运行时逐字相同**（`rustlua:save` / `rustlua:save_<slot>`，sanitizer 同
    `save_path_for_slot`）——不同就等于"换了通道，玩家浏览器里的旧档读不出来"；写了单测钉住
    （正是这条单测抓出我自己两处期望值写错）。
  - **两个验证通道**，与只读读数**分开命名**（读数只读、动作显式）：`rustlua-world-do=<Lua>`（驱动
    `-WorldDo`，只跑一次、允许副作用，等价一条控制台命令）与驱动 `-StorageProbe`（落定后从
    **浏览器侧**独立读 `localStorage`：键 + 长度 + 格式头）。
  - **证据（三条串行）**：A 主张（Rust-only，无 JS 世界）`storage probe: rustlua:save=24hdr:ok` +
    往返 `value="v1"`（探针是 write → commit → `save.clear()` 清内存 → load → 读回，所以那个值
    只能来自"真的出过进程又读回来"）+ `exit=0`；B 对照（不 commit）`(no rustlua:save entry)` ⇒ 红 `exit=1`；
    C 产品路径（有 JS 世界）同样全绿 `exit=0`。另加一条引擎侧集成测试（独立进程，避免 provider
    污染同进程的其它存档测试）：装了槽后 commit/load 往返为真、且不碰文件系统。
  - **探针抓到第二处缺口**：装上槽后第一次仍红，读数写着 `do={"committed":false,"why":"no_save_path"}`
    —— `commit` 在到达槽之前就因"没配存档路径"被忽略（引擎按既有语义只在"跑脚本 + 有工程根"时配路径，
    见 `runtime_scene/scene.rs` 的 `init`）。这是既有语义而非缺陷（渲染-only 模式不该存档），
    因此**没改引擎**，而是把门开到真跑脚本那一档（`-SimRoute script`）。
  - **未完成**：微信侧无真机验证（`localStorage` 由适配层提供）；键名与域/应用无关（同域两个工程共用一把键，
    要修需配迁移）；跨运行时互读只验了 payload 格式头一致。
- **只读工程状态查询通道 + 交互门（WP6-06）**。上一片的位移门只覆盖"按键 → 走路"一条通路，
  退休清单里还压着"玩家交互流程"（开背包、开任务、切图）。这一片把"能问任意工程状态"做成基础设施。
  - **一次否证**：第一版想用场景输出差值（`draws=`/`vertices=`）当判据，实测不干净——对照剧本
    `q:30-150`（游戏没用到的键）什么都不按，`scene_delta` 仍有 `+0,+60,+0`：待机动画/HUD 自己在动。
    输出差值分不清"输入驱动"与"世界自己在动"，当门会**假绿**。`scene_delta=` 保留作旁证，不作判据。
  - **基础设施**：`rustlua-world-expr=<Lua>`（驱动 `-WorldExpr` + `-ExpectWorld <正则>`）。表达式在
    **工程同一个 Lua 状态**里求值（`with_script` + `eval_json`），能 `require` 工程模块；**契约只读**。
    刷新点与内置世界读数同处（只在写状态属性那一帧，热路径零成本）。
  - **判据用工程自己的状态机**（`require('core.state').get_mode()`，`main.lua` 本来就用它分支）：
    Rust-only 宿主按 `i` ⇒ `expr="Inventory"`（`exit=0`）；按 `q` ⇒ 仍 `"Playing"`（反向断言也绿）；
    正常产物同一剧本同样翻转 ⇒ 通道在产品路径上通；位移门 `moved=0,280` 回归绿。
    旁证：`i` 那次 `draw_calls` 42→46~54（背包面板真的多画了东西）。
  - **途中修的两个真问题**：① 读数帧不确定（同一命令两次分别读到 `frames=30` 与 `frames=240`，
    落定瞬间正好在按 `i` 那一刻）⇒ 等待器加 `settleFrames`，**等到帧数推上去再写 DOM**，
    其余路径行为不变；② PowerShell 传参吃掉正则里的引号（`expr="Inventory"` → `expr=Inventory`）
    ⇒ 改用无引号正则，与第 24 轮"空字符串参数被丢弃"同类。
  - **未完成**：只读无护栏；背包内容/任务进度/存档落盘未验（存档要 wasm 存储后端）；触摸/指针无等价剧本。
- **Rust 侧世界读数与"输入 → 世界改变"位移门（WP6-05）**。上一片让 Rust 宿主收到 DOM 事件，
  但 `input_edges` 只证明"边沿进了快照"，证明不了游戏逻辑真的因此移动了主角。
  - **读数从引擎自己的世界读**：主角 = 场景文档里**名字含 `player`** 的实体（与渲染器判主角同一启发，
    不写死工程名）→ `entity.find` → `transform.get`，经 `with_script` + `eval_json`（只读契约）。
    顺带查出一件架构事实：`transform`/`spawn`/`__rustlua_transforms` 这套实体 API 是
    **`web-runtime.js` 用自己的 Lua 预置表实现的**，即 **JS 世界在遮蔽引擎同名 API**——
    "退休 JS 世界"的清单因此要加上"web 平台的实体 API 实现"这一项。
  - **确定性输入剧本**：URL `rustlua-input-script=<键>:<起帧>-<止帧>[,...]`（驱动侧 `-InputScript`），
    区间半开、逐帧按集合差发边沿（首尾相接的多段不会多出假边沿）。用 URL 而不是探针按键，
    是因为探针落点取决于事件循环调度，而位移读数要与帧号对齐才有可比性；确定之后它还能当性能基线。
  - **证据**：Rust-only 宿主（无 `web-runtime.js`）按住 `w` 120 帧 ⇒
    `webgl_bundle_rust_input_ok script=w:30-150 world=JianghuPlayer@624,360 moved=0,280`、`exit=0`；
    对照组换游戏没用到的键 `q` ⇒ `moved=0,0` 且门**变红**（门要能红才算门）；正常产物读数逐字相同。
  - **一个被自己推翻的假设**：第一版把读数写成"`step()` 里每 30 步自动刷"，随后 `-DriveJhyt`
    从 2/2 通过变成 5/5 失败，我判断"读数抢了 JS 世界帧预算"；改成按需刷新后仍 2/2 失败，
    且 `-GpuFrames 30`（几乎不执行新代码）那次也红、相位换成预算更紧的 `quest-close`（70 帧）
    ⇒ 归因错误。"按需刷新"保留：**观测不该在热路径上**。
  - **本轮真结论**：带 GPU 通道时驱动剧本今天 10 次运行 **2 绿 8 红**（失败相位 `nav-exit` 900 帧 /
    `quest-close` 70 帧），**不带 GPU 通道同一剧本 2/2 绿**，同期本机 CPU 负载 57%
    （微信开发者工具等都在跑）⇒ 是"GPU 通道与 JS 世界抢同一份 CPU/帧预算"，未修；
    对退休方向反而是正面证据：JS 世界在 GPU 模式下抢帧且零像素贡献（第 23 轮已证逐字节相同）。
  - **未完成**：剧本只有键盘（触摸/指针无等价剧本）；`moved` 是相对量；`EngineStartup` 档无场景文档
    ⇒ 读数退化为 `world=no-player`。
- **Rust 侧自己的输入采集：退休清单第一项落地（WP6-04）**。WP6-03 证明"没有 JS 世界，Rust 也能启动、
  跑脚本、出画面"，但玩家**按不动它**——线格式输入此前只有消费者（`web-runtime.js` 采集事件 → `RLFI`
  → `lua_input_frame` → 宿主读表），没有 JS 世界就没有输入。
  - **逻辑与薄壳分离**：归一、边沿、每帧量、两路合并全部放进 `game_web` 里**不依赖 wasm 的模块**
    （`frame_input::wire_key_name` / `FrameInputProducer` / `merge_into`，加 `fit::css_to_design` 让
    触点映射与画面取景共用 `fit_now` 写下的同一个盒子）；wasm 侧只挂 window 监听器把事件攒进生产者。
  - **键名归一与 JS 宿主逐条同规则**：小写、`" "`→`space`、`arrowX`→`X`、表外键返回 `None` 并**计数**，
    不静默丢弃。坑：第一版先 `trim()` 再比空格，空格键静默失效——单测第一次跑就抓到。
  - **两路输入并集**：Rust 侧 DOM 采集 **并上** JS 世界送来的 `RLFI`（它那里有屏幕摇杆与补丁层虚拟键），
    位图按位或、滚轮相加、光标听"本帧真的动过"的那一路，丢掉任何一边都会让某种操作方式失灵。
  - **证据（Rust-only 宿主，没有 `web-runtime.js`）**：按键探针读数
    `dom_events 0 -> 14 -> 20 ; input_edges 0 -> 2`，与"12 次按下 + 2 次页内自测 + 6 次抬起"、
    "一次按下 + 一次抬起"逐一对上，`webgl_bundle_gpu_fit_ok` 同过、`exit=0`。
  - **坑（三个，各自都伪装成"链路坏了"）**：① PowerShell 向原生程序传参会**丢掉空字符串**，
    于是 `-KeyProbe` 的 `1` 落到 shot 的位置、探针静默不跑而驱动照样 `exit=0`（改成 `-` 占位）；
    ② 状态属性每 30 帧才刷、**240 帧跑完循环就冻结**，探针读到 `frames 240 -> 240 -> 240` 像是
    "事件没到"，实际是宿主已收工（加 URL `rustlua-gpu-frames=<n>`，`0` = 不设上限）；
    ③ 驱动剧本在**负载下偶发**超时：`phase=nav-exit timeout frames=901`（JS 侧相位），
    原样命令在安静机器上连跑两次均通过、里程碑 M1–M6 齐全 ⇒ 与输入改动无关（Rust 侧监听器只读事件、
    全文无 `prevent_default`/`stop_propagation`，线格式又是 JS → Rust 单向），`-GpuFrames 0` 只是加剧它。
  - **未完成**：微信侧没有 DOM，触摸要走适配层（但它本来就把输入编码成同一份线格式，可并存）；
    `input_edges` 只证到"边沿进了模拟"，Rust-only 那条路还没有等价于 `-DriveJhyt` 的"位移非零"断言。
- **两层画面的客观对比：像素证明"同框"，并推翻了原来的退休判据（WP6-02）**。上一片只保证了**几何**一致，
  这一片把**画面**变成读数。
  - **取图方式**（关键在"同一次运行、同一时刻"，分两次跑不可比，动画相位不同）：等 GPU 帧数推到
    240 帧后 `Page.captureScreenshot`；再由 JS 把 GPU 画布临时 `display:none` 拍第二张"只有 Canvas2D 层"；
    然后恢复。必须走 CDP 截图——WebGL 画布默认 `preserveDrawingBuffer:false`，合成后用 JS 读是空白。
  - **像素独立证实了 WP6-01**：两张图的内容盒外的边缘行都是 `(21,25,35)`=`#151923`（2D 层黑边色），
    内容盒包围盒正好 `(0,8)-(755,432)` = **756×425**，与 `css=756x425+0+8` 对得上。
  - **但两层画的是不同的世界**：89.56% 的像素差异超阈值；GPU 内容盒只有 47 种量化颜色、9 个分区均值
    几乎一样（一片浅褐），而 Canvas2D 有 182 种、是村子。
  - **这个差异不能读成"GPU 渲染器画错了"**：两层是**两套独立的模拟**——Rust 侧跑工程脚本、相机跟 Rust
    侧玩家；Canvas2D 侧是 `web-runtime.js`+补丁层跑的另一份状态、相机跟 JS 侧玩家。从未共享世界状态，
    "画面不同"是必然。
  - **退休判据因此改写**（本片最重要的产出）：原计划把"两层画面一致"当作退休旧热路径的前置证据，
    这个判据**本身不成立**。正确顺序是：① Rust 侧成为**世界状态的单一来源**（JS 层不再自己推进世界）；
    ② 之后同一次运行的两张图才有可比语义（`-ShotUnder` 流程已就绪）；③ 到那时像素差异才能当判据并给门限。
  - **新增可复用工具**：`verify-webgl-bundle.ps1 -Shot/-ShotUnder`、`scripts/compare-shots.ps1`
    （包围盒/整图差异/分区均值色/黑边色/颜色种类数/差异热图）。
  - **顺带第二次踩到同一个编码坑**：工具写出的临时 `.ps1` 无 BOM，PS 5.1 按 ANSI 解析中文后**吞掉了函数体**
    ——脚本"跑完了"却什么都没做。已在取图流程里加了编码自检（`.ps1` 必须 BOM、`.js` 必须无 BOM）。
- **两层同框：GPU 画布改用 Canvas2D 层的等比适配盒（WP6-01，接入两平台第一片）**。Canvas2D 层每帧把
  1280×720 设计空间 **contain** 进游戏画布（`web-runtime.js` 的 `fitS = min(w/1280, h/720)` + 居中黑边），
  而 GPU 层此前是"固定 1280×720 后备缓冲 + CSS 铺满视口"——**两层画在两个不同的几何盒子里**。
  实测无头窗口的游戏画布是 756×441（不是 16:9），旧行为下 GPU 内容被浏览器**非等比拉伸约 +7%**；
  2D 层则居中留了 8px 黑边。
  - **为什么一直没发现**：画布属性、`frames=`、`draws=`、`avg_*_ms=` **一个都不异常**。
    这类几何错只能靠把两层几何都写进读数才抓得到——所以新增 `viewport= fit= css=` 读数。
  - **做法**（只在宿主侧，公开面无新增）：每帧读游戏画布的后备缓冲尺寸与 `getBoundingClientRect()`，
    算 `contain_box`（同比取整 + 居中），同步画布 `width/height` → `surface.configure`（复用配置，只改宽高）
    → `RustScene::set_viewport`，CSS 按同一比例摆位且**只在变化时**写 DOM。DPR/页面缩放/画布被 CSS 挪位
    都不需要单独处理——像素数来自后备缓冲、位置来自 rect，两者之比就是真实换算。
  - **实测**：`viewport=756x441 fit=756x425 css=756x425+0+8`，与 2D 层自己算的完全一致
    （`720×0.5906 = 425.25`、`(441−425.25)/2 ≈ 8`）。`draws` 50→42 是**正确结果**：取景盒变小，
    不再画看不见的地块（旧行为等于"按 1280×720 取景再塞进 756×441"）。
  - **门禁能钉住它**（错法不可见）：① `verify-webgl-bundle.ps1 -Gpu2d` 新增几何断言，
    要求适配盒比例等于设计比例（1% 容差）且不超出游戏画布，通过时打 `webgl_bundle_gpu_fit_ok`；
    ② `game_web/src/fit.rs` 三条原生单测（比例恒等/黑边在不贴合的那一轴/退化输入不 panic 不给 0），
    **反向验证**：把 `scale` 改回"铺满"立刻 2 条红，还原后 9 条绿且文件字节一致。
  - **顺带修掉一个工具链坑**：编辑工具写出无 BOM 的 `.ps1`，PS 5.1 按 ANSI 解析中文 ⇒ 乱码 +
    语法错。补回 BOM 后正常；已记进计划 WP0 附（改完 `.ps1` 必查 `EF BB BF`；`.md` 相反，不带 BOM）。
  - **带出的新待办**：两层同框了，但**像素是否一致**还没比过（下一片：同帧 CPU/GPU 取图对比，
    也是旧热路径退休的前置证据）；2D 背景色与渲染器清屏色是否一致未断言；微信侧尺寸策略不同、无环境验证。
- **Lua 净耗时进 Web 读数，顺带修掉一个"两个平台结论相反"的假读数（WP0 续四）**。上一片点名
  "每帧 64% 在 `lua_update`"，但那还是调度系统的口径（含借出调度器、查全局、错误包装）；
  要分清"脚本自己贵"还是"引擎围着它转的那些动作贵"，得在 Lua `update()` 前后卡表。
  - **发现真缺陷**：引擎本来就在量（`last_update_micros` → 公开出口 `take_script_micros()`），
    但在 wasm 上被 `#[cfg(target_arch = "wasm32")]` 整段跳过、直接记 0——那是"std 时钟在 wasm 上会 panic"
    时代的权宜，而仓库后来已有 `web_time`（ecs/infra/scripting 都换过）。后果不是"少一个数"，
    而是**同一份代码在两个平台给出相反结论**：Web 上 Lua 永远显示零成本。改用 `web_time::Instant` 后
    两边同一段代码，这条读数在 Web 上首次为真。
  - **三级归因**（都进常驻属性，不依赖日志限额）：`avg_lua_ms`（Lua 净）1.97 ms、
    `lua_update`（调度系统）2.07 ms、`avg_step_ms`（整个逻辑步）2.45 ms ⇒ **系统外围只有 0.10 ms**；
    每帧工作量 3.29 ms 里 **Lua 自己占 59.9%**，抽网格 0.41 + 渲染 0.43。
  - **交叉验证**：对照路（不跑脚本）`avg_lua_ms=0` 而 `lua_update=0.08`（纯外围）⇒ 没有把"不跑脚本"
    算成成本；脚本路外围 0.10 ms 与之同量级；各项之和与总量在采样噪声内一致。
  - **结论**：**帧循环里已经没有"引擎侧热路径"可摘**——每帧 60% 就是工程脚本自己的 `update()`，
    引擎围着它的全部动作合计 0.10 ms（5%）。要再压这条线，对象是游戏脚本自身或每次 Lua→Rust 调用的
    桥接成本，而不是引擎的帧循环。
  - **带出的新待办**：桥接 vs 游戏逻辑仍未分开（工程每帧按实体逐个调 API，31 个实体）；
    分开需要在 API 注册处加计数（几百处，机械改动）或给引擎加"跑 Lua 片段"的入口做 shim。
- **每帧成本三分 + 步内热点：大头是工程脚本的每帧 `update`，不是渲染器（WP0 续三）**。上一片确认了
  "钱花在每帧"，但每帧只有一个总数，"该优化渲染器还是逻辑"仍然只能猜。本片把它拆开。
  - **拆法**（零新增公开 API）：宿主侧三段计时——`avg_step_ms`（输入 + `run_script_steps`）、
    `avg_mesh_ms`（抽网格）、`avg_render_ms`（渲染+呈现+一次 poll）；步内明细则复用引擎**已有的公开出口**
    `RuntimeScene::last_schedule_report()`（编辑器与原生测试都在用），在宿主累加取均值，前 4 名进常驻属性。
    两条口径：**首帧不计入**三个均值（含一次性建图集/建管线，已另有 `first_frame_ms` 承载）；
    步内明细**不进日志**。
  - **为什么步内明细不再打日志**：原来那条 DEBUG 级 `[schedule]` 是每帧一行，在浏览器里会吃光
    日志出口上限——实测 400 条上限下只剩 **28 行**样本，且全落在最早的几十帧（最不具代表性的一段）。
    现在宿主聚合 + 订阅者挡掉该 target 的 DEBUG，限额留给真正一次性的事件。
  - **读数**（240 帧，SwiftShader/headless，1280×720）：脚本路每帧墙钟 9.06 ms（fps 110），其中
    **工作量只有 2.92 ms**（逻辑 2.21 + 抽网格 0.35 + 渲染 0.36），其余 ≈6.1 ms 是等下一次回调的空等；
    对照路（不跑脚本）工作量 0.85 ms。步内：**`lua_update` 1.88 ms = 步内的 64%**，
    `physics2d_step` 0.19，其余 16 个系统合计 ≤0.08。
  - **四条结论**：① 每帧大头是**工程脚本的 `lua_update`**（脚本带来约 2.1 ms/帧），抽网格+渲染合计
    只有 0.71 ms；② **当前场景规模下 GPU 批渲染几乎没有可省空间**（50 个四边形 = 0.7 ms）⇒ 下一步
    应对准逻辑/脚本这条线，批渲染的收益要等更大规模或真机 GPU——这修正了"优化就是继续加码批渲染"的直觉；
    ③ 引擎自带系统都不是热点；④ 每帧 DEBUG 日志不能当性能出口（限额截断 + 样本偏差），常驻聚合才行。
  - **带出的新待办**：`lua_update` 内部未归因（脚本自身逻辑 vs 每次 Lua→Rust 调用的桥接成本）。
- **wasm 首帧/每帧成本基线：字体不是瓶颈，顺手补上宿主缺失的日志出口（WP0 续二）**。上一片收尾留的待办是
  "字体解析耗时未量"，当时的假设是"MB 级字体解析可能很慢，所以分包/按需加载有价值"。本片把它量了，
  **假设不成立**。
  - **先补能力缺口**：引擎诊断全走 `tracing`，而 wasm 宿主此前**没装订阅者**——`[render] font atlas ready`
    这类现成结论在桌面上有、在浏览器里被静默丢掉（上一轮查字体只能靠 DOM 属性反推）。新增一个约 90 行、
    不引新依赖的控制台订阅者，并把安装点与 panic hook 合并成一个入口：panic hook 管"炸了"，
    它管"走到哪一步了"。两条约束写在代码里：**幂等**（与资源 provider 同一套）、**有上限**（400 条，
    到顶只报一次"已丢弃"——否则"日志刷屏"会变成新的性能问题）。
  - **分段计时**（都用 `web-time`）：宿主侧 `sim_boot_ms`（起场景 + 装载脚本）、`first_frame_ms`
    （首步 + 首次取网格，含首次文本图集与首次管线/贴图上传）、每帧 `avg_frame_ms`/`fps`；
    引擎侧 probe 装载、图集 `decode_ms`/`build_ms`、`chars.txt` 预热的 `elapsed_ms`。前两条同时进
    控制台与最终属性（阶段属性会被后续覆盖，only 属性留不下来）。
  - **读数**（本机 SwiftShader/headless，1280×720）：启动 = 脚本路 `sim_boot_ms=75.6` +
    `first_frame_ms=45.1` ≈ **121 ms**；对照路 14.9 + 39.7 ≈ **55 ms**；字体合计 ≈**17–24 ms**
    （probe 1.5–4.5、图集 decode 0.6–0.8 + build 2.0–4.5、**预热 1605 字 13.9–19.0**）；
    每帧 `avg_frame_ms` 6.6–7.6（11 draws）～11.4（50 draws）。
  - **三条结论**：① 字体**不是**瓶颈（整条链 ≈17–24 ms，大头是 `chars.txt` 的 1605 字预热；
    读盘+解包+解析加起来不到 6 ms）⇒ **为字体做分包/按需加载没有收益**，真要动就动字符集大小；
    ② 启动大头在**脚本装载**（脚本路比对照路多 ≈66 ms）；③ 每帧成本与画面规模线性相关，可作后续
    批处理优化的同机对比基线。
  - **带出的新待办**：启动那 ≈66 ms 没拆到具体环节；`first_frame_ms` 是复合数；真 GPU/真机仍无读数。
- **"读工程文件"收成一条通道，并第一次让它能被原生测试证明（WP4 第二片）**。上一片为了把字体接上 wasm，
  在三处各写了一份"先文件系统、再 vfs"的助手，并留下"出现第四个调用方时再下沉"。本片提前做掉，
  因为那份顺序本身是错的。
  - **一个只在宿主环境下才现形的分叉**：资源提供者（provider）的契约是"装了就当权威、读先问它"，
    而三处助手写的是"先问文件系统"——Web 上看不出来（`std::fs` 恒失败，两条路结果一样），
    但桌面上一旦有宿主装了 provider，文件系统会把它抢先，与契约正好相反。查过全仓库：目前只有
    wasm 宿主与测试会装 provider，所以今天还没有受害路径——**这正是现在收口的理由**。
  - **收口方式**：新增 `vfs::read_resource`（provider 优先 + 统一解码，读不到/解码失败都报 `Err`），
    工程配置、字体字节、图集/自定义字体/`chars.txt` 的存在性与读取全部改走它；
    两处 `use std::fs` 随之删掉。
  - **借测试进程才证明得了它**：provider 是**进程级一次安装**的，在 `src/` 单元测试里装它会污染同进程
    其他用例（前两轮"只有浏览器侧证据"就是这个原因）。Cargo 给每个集成测试文件单独一个进程，
    于是补出两条原生回归：资源通道一条（provider 字节自动解码、`read` 保持未解码语义、回落文件系统、
    缺失报错、`project.toml` 只在 provider 里也能读出键、`is_file` 问 provider），
    字体一条（**工程根在磁盘上不存在**时配置链 + 探针 + 中文四字形度量）。后者就是上一片那条修复的
    原生复现——以前只有浏览器读数，现在 `cargo test` 就能证明。
  - **验收读数**：门禁 9 项全绿；浏览器三条路全部 `exit=0`、`boot=running`、`step_errors=0`，
    读数与上一片逐字一致（`draws=50 paths=54` / `draws=11 paths=22`）⇒ 这次收口在 wasm 上
    **行为中性**。
  - **带出的新待办**：字体解析/建图集的**耗时**仍未单独量过（首帧 11–13 s 里含多少）；
    微信侧读取器仍把文件当字符串返回（二进制会被编码破坏）；`read_resource` 整文件解码，
    将来分包里的大资源需要"按需解"的通道。
- **工程字体进 wasm：脚本 `update` 的每步报错清零，顺带拔掉第二个 wasm trap（WP4 第一片）**。
  第七片让模拟每帧往前走之后，`step_errors` 每步一条，全是同一句
  `ui.measure_text: project font unavailable`。本片把它清成 0，过程中撞上并修掉一颗更硬的雷。
  - **根因不是"字体没打包"**：`pack/manifest.txt` 里就有工程字体，`start()` 也早已把 manifest
    全部条目预载进 wasm 的资源表。断点在三处**只认 `std::fs`** 的读盘——Web/微信上工程随宿主资源表走，
    `std::fs` 恒失败，于是 `project.toml` 整个读不到（所有键静默退化成缺省值）、字体度量探针恒为
    `None`、渲染器字体图集建不起来。三处统一成同一判据：**先文件系统、再 vfs**，vfs 里是打包原始
    字节所以再解码（明文原样返回，重复解码安全）。桌面走前半句短路，**行为零变化**。
  - **读到之后立刻炸**：`panicked at std/thread/scoped.rs` + wasm 直接 trap。字体图集的并行预热用
    `std::thread::scope`，而 **wasm 上没有线程**——`scope` 是 `unreachable`，连"起不来就降级"的机会
    都没有。改成编译期分岔：wasm 串行逐字光栅化，原生保持按核数分块并行（**字形内容与顺序一致，
    图集布局逐字节不变**）。同一颗雷也堵在公共入口上：`par` 的并行度在 wasm 一律压成串行。
  - **两次修的中间态更糟，值得记一笔**：只补 vfs 那版是 `boot=failed`、GPU 停在取网格、驱动超时——
    字体一旦真能读到，trap 就把整个 wasm 实例打死，把 Canvas2D 那条路一起带红。
    **"资源读不到"与"资源读到之后炸了"必须分开验**，否则会把后者当成前者没修好。
  - **验收读数**（同一份产物）：工程场景 + 工程脚本从 `step_errors=240`、驱动 94 s 超时红，变成
    `exit=0` / 13 s / **`step_errors=0`**、`draws=50 vertices=4266 paths=54`；引擎启动场景那条同样
    **`step_errors=0`**、`exit=0` / 11 s。`ui.measure_text` 是唯一会抛这条错的地方，它归零即
    "探针拿到真字体、按真实字形度量"。
  - **带出的新待办**：微信侧的读取器把文件当**字符串**返回，字体这类二进制会被编码破坏
    （Web 走预载字节表所以没这个问题）；要两侧一起改，且本机没有微信环境无法验证，故未动。
    大字体的**解析耗时**也还没量过——本片只证明"能读到、能量对"。
- **wasm 里的模拟开始"真的往前走"：接上每帧推进与宿主输入，并把三处会 trap 的时钟换掉（WP3 第七片）**。
  上一片证明"工程脚本在 Web 上确实会执行"，但那只说明**起场景那一刻**跑到了脚本；每帧该发生的逻辑
  一步都没发生——`vertices`/`draws` 逐帧不变，也没有任何输入。本片把这条缝补上：
  - **一条调度入口，两个宿主**：wasm 侧的 `RustScene::step` 直接调
    `RuntimeScene::run_script_steps`（桌面运行器、编辑器预览、headless 运行器用的是同一个），
    每帧一步、定长 `1/60`。**按帧固定步而不是按真实时间**：wasm 上 `Instant::now()` 是 panic 路径，
    而读数要可复现——读第 N 帧就能反推模拟到哪一步。
  - **输入只有一个来源**：宿主的线格式（`FrameInput`）新增一条到引擎 `InputSnapshot` 的换算，
    与 Lua 那条链**吃同一份输入**（`lua_input_frame` 解码后就存下来，两条链都取它）。此前
    "画面里的角色会动、模拟里的不动"这种分叉在读数上完全看不出来。快照**跨帧保留、只改变化位**：
    整份重建会把"一直按住"变成"每帧都刚按下"，脚本里的边沿分支于是每帧触发。
  - **契约测试当场抓到两个真缺陷**：① 宿主按 `event.key` 送出的 `"shift"`/`"control"`/`"alt"`
    在引擎侧**根本解析不出键位**（`from_text` 与 v1 两条路都不认），于是这两个修饰键一直是静默失效；
    现在按线格式的槽位定到左键，并把"线格式分不出左右"记成 ABI 下一版的事。② 键名解析的顺序必须与
    脚本侧 `parse_key_strict` 一致（先物理名、再 v1 兼容名），否则同一个按键在"脚本看到的输入"与
    "引擎模拟看到的输入"里会变成两个键。
  - **三处时钟会直接 trap**（`wasm32-unknown-unknown` 的 std 把 `Instant::now()` 实现成 panic）：
    帧调度器给**每个系统**计时、手柄层每帧做"震动到期"、性能采样器在主循环构造——都在
    `run_script_steps` 这条路上。统一换成 `web-time`（原生上就是 `std::time::Instant` 的再导出，
    行为为零变化），与渲染器此前已有的处理同源。**推进失败不再拆掉模拟**：脚本报错照旧记账继续走，
    否则画面退回内建场景，现象与"根本没起场景"一模一样。
  - **验收读数**（同一份产物）：不跑脚本那条 `steps=240 step_errors=0`，读数与帧数逐帧对齐；
    用仓库自带的驱动剧本（`-DriveJhyt`，走真实输入路径）时模拟侧 `input_edges=13`——宿主键盘 →
    线格式 → 引擎快照 → `run_script_steps` 这条链在浏览器里**真的通了**；跑脚本那条
    `spawned` 从 0 涨到 56/135（脚本按输入生成实体）。
  - **带出的新阻塞**：工程 `main.lua` 的 `update` 在 wasm 上每步都报
    `ui.measure_text: project font unavailable`——`project.toml` 指到的工程字体没进 wasm 资源管线，
    所以"模拟开始往前走"之后紧接着撞上的是**字体管线**（归 WP4）。
  - **一条反向结论**：CDP 的 `Input.dispatchKeyEvent` 与 `Runtime.evaluate` 里派发 `KeyboardEvent`
    在这个 headless 配置下都进不了页面的输入状态（`data-rustlua-input` 的 `bits` 恒为 0），
    而页面**内部**的剧本走同一条事件路径却有效。所以验收输入只能用页面内剧本，不用 CDP 注入；
    这条试出来的开关已经删掉，不留一个"看着生效、其实没送"的假杠杆。
- **Web 侧"脚本卡死"定性：是 std 的 wasm 不支持路径 panic，与脚本无关（WP3 第六片）**。上一片把
  `RuntimeScene` 搬进 wasm 时留下一个卡点，并把它记成"跑工程 `main.lua` 会卡死"。本片查到底，结论不是脚本：
  - **卡点与脚本无关**：把 `scripts/main.lua` 换成**一行 0 字节**再跑，一样停住；给 Lua 装上指令预算钩子
    （默认 10 亿条指令，正常工程在千万量级）也没被触发——真有 Lua 死循环，它早该报错。
  - **不是"慢"**：同一条路给到 5 分钟超时仍未完成。
  - **不在实例化那一步**：新增的 `-SimRoute scene-script-noroot`（宿主场景字节 + 跑脚本 + **不给工程根**）
    能跑满帧循环、31 个实体；而 `init` 里"存档/音频/执行工程脚本"整块都挂在 `let Some(root)` 上，
    范围因此收敛到那几个调用。
  - **真相是 panic 后 trap**：给 CDP 补上页面控制台与异常采集后拿到
    `panicked at library/std/src/sys/pal/unsupported/os.rs:66:5` + `RuntimeError: unreachable`。
    那一行是 std 的 `temp_dir()` → `panic!("no filesystem on this platform")`，而 wasm 上 `cfg!(windows)`
    为假、`HOME`/`XDG_DATA_HOME` 都不存在，`user_dir::platform_data_dir()` 必然走到 `env::temp_dir()`。
    `panic=abort` 之下 wasm 当场 trap，宿主的状态属性永远停在 `sim_boot_route=…`——**在 DOM 快照里，
    "炸了"与"死循环"完全同形，不采控制台就永远分不出来**。
  - **修法**：`platform_data_dir()` 在 wasm 上返回虚拟数据根，不再碰 `temp_dir()`；平台差异只留在这一个函数里。
  - **另一处一直在静默失效的地方**：`scripts/main.lua` 的存在性判断原来只认 `std::fs::is_file()`，
    而 Web/微信上发布包在宿主的资源表里，它恒为 false——**"跑工程脚本"这件事在 Web 上从未真正发生**。
    改成"先看文件系统、再看 vfs"：桌面走前半句就短路，行为不变。
  - **验收读数**（同一份产物、四条路）：原本停住的两条路都 `exit=0`、`boot=running`；宿主场景 + 跑脚本
    比不跑脚本多出 `draws 11→18`、`textures 20→26`、`vertices 2136→2196`——这就是**工程脚本在 wasm 上
    真的执行了**留下的增量（此前它在 Web 上从未被执行过）。读数取自驱动判定 `boot=running` 的那一刻，
    是快照而不是终态。
  - **带出的新待办**：wasm 上存档/设置改走虚拟根后，`create_dir_all` 只会留一条警告，落盘语义需要
    宿主的 vfs 写通道；`vfs` 先于 `std::fs` 的这条判断目前只有浏览器侧证据（vfs provider 是进程级一次安装，
    原生测试里装它会波及其他用例）。
- **GPU 通道转逐帧循环、画布对齐设计分辨率（WP3 第五片）**。上一片证明了"引擎能画出工程真场景"，
  但那条通道只在启动时画 3 帧、画布还跟着页面把游戏画布缩成 756×441。本片把它做成宿主该有的形状：
  - **持续渲染**：帧循环不再同步跑完（那会把 JS 启动链摁住几百毫秒），而是派到事件循环上跑
    240 帧，每 30 帧把状态属性改写一次——`frames=` 因此成了"这条通道还活着"的心跳，抓拍在任意
    时刻都有可读读数。失败也当场写进属性并收工：沉默地停掉会让"通道坏了"看起来像"跑完了"。
  - **画布按设计分辨率**：GPU 画布改用与游戏画布同一对常量（1280×720）。页面脚本会把游戏画布
    按窗口缩放，而引擎 2D 取景按设计分辨率算——两者不一致时画面会被拉成另一个宽高比，
    而这种错法在属性读数里**完全看不出来**。
  - **一个可疑读数被定性成误读**：`vertices` 从 894 涨到 2136、`draws` 从 6 涨到 11，看着像
    "提取每帧攒东西"。查下来是**视口**变了：新增的原生测试证明同一份世界不跑逻辑步连提 8 次
    逐通道一致；停 60 ms 再提也一致；而同一份 `village.scene` 换视口就是 `756x441 → 786/6`、
    `1280x720 → 2028/11`。两个数字都对——变的是画布，而画布正是本片修的那件事。
  - **新增原生测试守着一条以前没人守的契约**：桌面每帧是"跑逻辑步 → `vertices()` → 渲染"，
    所以"不跑步、只提网格"这条路**只在新宿主上出现**（web 的 GPU 通道现在就是逐帧 `vertices()`）。
    `frame_extraction` 域用合成场景与工程真场景各钉一条：同一份世界连提多次必须逐通道相同。
  - **仍然不是"游戏画面"**：循环里跑的是不执行 Lua 的静态世界——没有输入、没有逻辑步、没有场景
    切换、没有与 Canvas2D 的像素比对；脚本卡死照旧。SwiftShader 上 `frames=` 不是帧率读数。
- **引擎在 wasm 里画出了工程的真场景：GPU 通道第一次有"游戏内容"（WP3 第四片）**。上一片把
  `RuntimeScene` 搬进 wasm，拿到的是一个卡点（跑工程 `main.lua` 会卡死）。本片不掩盖它，
  而是把两件事**拆开**：先证"引擎能把工程的**场景文件**画出来"，再谈"工程的 Lua 能不能在没有
  JS 补丁层的 VM 里跑"。
  - **让宿主交字节，而不是让引擎的 loader 去猜**：宿主手里本来就有启动场景路径与全部预加载资源，
    于是把场景路径一路传进 Rust 侧场景，由 `resource_raw`（表里的**原始**字节）→
    `Scene::from_packed_bytes`（它内部才 `pack::decode`）→ `from_scene_edit`（实例化场景、不跑脚本）。
    读表因此拆成两个入口并各自说明理由：**场景要原始字节、贴图要已解包字节**——解包责任在读它的人
    手上，先解一次会让场景被解两遍，症状是"文件读到了、解析出来是垃圾"。
  - **"换一个场景"是必需的读数，不是凑数**：默认用工程启动场景时读到
    `main entities=0 vertices=0 draws=0`。去产物里数了才确认 **`scenes/main.scene` 本来就是空的**
    （809 B、14 行、0 个实体，只有 meta）——它是这个工程的"主入口门面"，画面内容全由 Lua 现造。
    所以 `entities=0` 是如实读数，同时也是一个**什么也证明不了**的读数（管线对错都会得到 0）。
    于是新增只接受纯场景名（值做白名单校验：这是从 URL 进到"读资源"的入口）的 URL 位
    `rustlua-sim-scene=` 与驱动开关 `-SimScene`。
  - **读数**：`scenes/village.scene` 31 个实体 → 894 顶点 / 6 个 draw call / 298 个三角形，
    22 条贴图路径里 **20 张全部**从打包资源表上传成功。对照上一片手工搭的几何（2 / 6）：
    画出来的已经不是那三块四边形，而是**工程自己的场景**。
  - **仍然不是"游戏画面"**：这条读数跑的是**不执行 Lua** 的场景实例化，所以"GPU 通道显示的是当前
    游戏状态"这句话还不成立；脚本卡死未解；GPU 帧仍是启动 3 帧、无输入/场景切换/像素比对；
    GPU 画布尺寸取游戏画布当次尺寸（未按设计分辨率对齐）。默认路径与 JS Canvas2D 那条路不受影响。
- **Rust 侧运行时场景进 wasm：能起来、能读真场景，但跑工程脚本会卡死（WP3 第三片）**。WP3 的终点是
  "一个模拟"——场景与每帧网格由 Rust 侧产出，而不是 JS 自己拼 Canvas2D。本片第一次把桌面运行器那条
  路（`RuntimeScene`）搬进 wasm，并把"跑不跑得起来"变成状态属性里的一行数据。
  - **起得来、也读得动场景**：`RuntimeScene` 在 wasm 里能编译、能构造、能实例化实体。它原先读场景走
    `std::fs`（web 上没有文件系统），所以现在**文件系统读取失败时改走 vfs provider**——字节由宿主的
    预加载资源表提供，解码与解析仍是同一套 `Scene::from_text`（不是第二条解析实现），桌面语义不变。
  - **顺带修掉一处引擎自身的缺陷**：独立引擎模式（无工程根）用的兜底诊断场景里，Sprite 组件写的是
    裸路径 `assets/textures/red-1x1.png`，而当前校验器要求第一个字段就是 `texture=…`——于是"没有工程"
    这条路上，引擎一跑脚本就报 `invalid sprite field`。这是引擎自己违反自家校验器，与 web 无关。
  - **本片最大的收获是一个卡点**：真场景能载入之后，`RuntimeScene::init` 会去跑工程的 `main.lua`，
    而**整页就停在那里**（不是 panic、不是 wasm trap，是同步长任务卡住，JS 的启动链一起被挡在
    `Loading …`）。给 GPU 宿主加**阶段自报**（`sim_boot_begin` → `sim_boot_ok` → `mesh_begin` →
    `textures_begin` → `textures_done=n/m`）之后，dump 里最后落定的属性是 `gpu=sim_boot_begin`，
    卡点因此钉在 `RuntimeScene::new` 内部，而不是"某处大概有问题"。
  - **没有渲染出任何游戏内容**：GPU 那 3 帧画的仍是内建场景。本片交付的是**接线 + 两处引擎修复 +
    一个可复现的卡点**，以及"失败必须变成数据"的诊断形状（连挂起都能从属性里读出来）。
  - **开关仍是三处默认关**：`-Gpu2d` 才编进这条线，URL 带 `?rustlua-gpu=1` 才启用；默认产物与
    JS Canvas2D 那条路**完全不受影响**（同一次 dump 里 `boot=running` / `scene-verify=done` /
    `drawn=5621` / `world=scene=18` 全绿）。
  - **下一步（按可验证性排序）**：① 先用不跑工程脚本的 `from_scene_edit` 路线把"真场景实体 → GPU 帧"
    打通（低风险、收益直观，且能把"能画真场景"与"Lua VM 统一"解耦）；② 再逐 `require`/逐宿主 API
    打阶段定位脚本卡死；③ 只有 ① 绿了，才谈得上 JS Canvas2D 那条路退休（WP8）。
- **真渲染器直接渲进画布纹理并 present：Web 宿主的接入形状实测钉死（WP5 第四片）**。前三片都停在
  离屏纹理上，而 web 宿主必须把这一帧送到画布。查下来根因比"没验过"更硬：`game_web` 里**连
  `engine::host::Renderer` 都没有**（只拿 `HtmlCanvasElement` 交给 JS 的 Canvas2D）。
  - **先问格式**：渲染器的颜色附件格式是构造时烘死的 `Rgba8UnormSrgb`，管线与附件不匹配会被
    wgpu 校验拒绝，而 wasm 是 `panic=abort`。实测画布给的是 **`Rgba8UnormSrgb` / `Rgba8Unorm` /
    `Rgba16Float`** —— 烘死的那个**正好可用**，所以**不需要改引擎**（也不必为 web 做格式参数化）。
    接入形状就是一句：`get_current_texture().texture` → `render_into_texture` → `present()`，
    实测**零校验错误**。
  - **踩到的坑（本片最大收获）**：第一版顺手在 `SurfaceConfiguration` 里申请了 `COPY_SRC`，
    结果 wgpu 在**未捕获**的校验错误上直接 panic，`panic=abort` 让整个 wasm 模块死掉，
    现场只有一个 `Uncaught RuntimeError: unreachable`，连"哪一步被拒"都报不出来。改成
    ①`usage` 只申请 `RENDER_ATTACHMENT`、②把配置与"渲染+present"整段放进
    `push_error_scope(Validation)`（失败变数据、不再 abort）之后立刻拿到读数。
    顺带钉住一条平台事实：**WebGL2 画布纹理只支持 `RENDER_ATTACHMENT`，当不了 copy 源**。
  - **证据边界（明确记录）**：画布纹理读不回来，所以"present 之后画布上是什么"无法在页内逐像素
    对拍；逐像素判据仍取自离屏那一帧（7 个采样点逐通道精确），画布这侧的证据是"配得起来、渲得进去、
    present 出去且零校验错误"。要连画布像素一起钉死得走页面外 CDP 截图那条路，留给需要它的那一片。
  - **下一步**：`game_web` 有画布、有 `start()`，缺的正是这一步接线（建 surface → 把设备交给
    `Renderer` → 每帧把 **Rust 侧产出的场景**渲进画布纹理并 present），之后再拆
    `plugins/webgl/web-runtime.js` 的 Canvas2D 热路径（WP8）；场景与逻辑目前仍在 JS/Lua 两侧，
    所以 WP3「统一 Rust 模拟」必须先行动。
- **2D 叠加通道保序合并：提交次数从"每个 draw 一次"降到"每个同键连续段一次"（WP5 第三片）**。
  原来 2D 主路径是每个 `SceneDraw` 一次 `pass.draw` + 一次 `set_pipeline`/`set_bind_group`，
  一千个相邻精灵就是一千次提交。
  - **做法**：相邻、键相同（贴图下标 + 是否 SDF + 是否位图）、且顶点**首尾相接**
    （`start + count == 下一个的 start`）的 draw 合成一次提交。纯渲染器侧改动，宿主与场景格式未改。
  - **为什么不算拿正确性换性能**：合并后的三角形顺序与混合顺序与逐个画完全一致，像素必须逐字节
    相同；合并不跨层边界，2D 半透明叠加依赖的顺序没动。两条边界各有测试钉住：键变了不合并、
    顶点不连续不合并（中间那段没人引用的顶点一旦合进去就会被画出来）。
  - **最容易漏的安全条件**：键相同**不**等于内容相同——异步贴图可能在同一帧内完成上传，
    同一贴图键第一次解析拿到白兜底、第二次拿到真贴图。`wgpu` 27 的 `BindGroup` 没有身份查询
    （`global_id` 是更晚版本才有），因此用**解析纪元**（贴图缓存长度、自定义字体图集数、
    内建图集是否已绑定）做代理，纪元一变先冲刷批次——于是"批次内键相同 ⇒ 内容相同"是证明过的。
  - **读数**：真浏览器 WebGL2 上探针 `draw_calls` **3 → 2**（`triangles` 仍 6），7 个采样点
    （含 4 个内存贴图纹素）仍逐通道精确、白色像素 5202/5202；原生 1000 块相邻同贴图四边形
    **1 次提交**（`instances=1000`、`triangles=2000`）；200 块**交替**贴图仍是 200 次——
    **局限一并钉住**：保序合并只吃同键连续段，要再压就得让产场景一端按图集排布（Tilemap 那一片），
    不能在这里按贴图重排（重排等于改画面）。
  - **回归**：原生渲染器测试 31 项全过（严格 GPU 模式、0 跳过），既有像素金标（tetris、编辑器预览、
    world sprite、frame views）全部照旧——合并没有改动任何一个像素。
- **贴图能"交进去"了：图集/精灵/Tilemap 在 web 上的地基（WP5 第二片）**。第一片只画了内建白
  贴图（`texture == 0`），完全没碰资源解析；而 2D 贴图原本只有一条来路——按路径 `std::fs::read`。
  web/微信上没有文件系统，那条路必然落白兜底，**而"白"恰好也是"什么都没画"的样子**：图集画对
  没有，在浏览器里根本无从判断。所以这一片不是"顺手加个 API"，而是把一条**不可验证**的路径变成
  可验证的路径。
  - `Renderer` 新增 `upload_texture_rgba8`（已在内存的 RGBA8 像素）与 `upload_texture_png`
    （字节解码），按 `key` 登记；**同 key 重复上传是原地替换**（槽位不变），流式图集靠这条——
    图集更新不该让已按资源身份缓存的绘制集体失效。解析路径一行未改：宿主照旧把 `key` 放进
    `SceneMesh::paths`、用 1-based 下标引用。
  - **判据设计**：探针上传一张 2×2 四色棋盘（红/绿/蓝/黄，**没有白**），键取 `probe://checker2x2`
    ——一个**不存在的路径**。上传若失败就必然落白兜底，于是两处同时露馅（四个纹素采样全白；
    白色像素面积超出解析值约 4%，远超 2% 容差）。反过来，采样点逐通道等于上传字节，只可能是
    "上传真的进了缓存并被采样到了"。
  - **结果**：WebGL2 上 7 个采样点**全部逐通道精确命中**（含 4 个纹素 = 上传的红/绿/蓝/黄），
    `draw_calls=3`/`triangles=6`/`dropped_draws=0`、`loaded_texture_count=1`、
    `texture_mismatches=0`、白色像素 5202/期望 5202。原生新增两条测试（含非法输入必须在
    **进缓存之前**被拒），渲染器测试 26 项全过；`verify-wasm-lua` 仍绿且 `wasm` 尺寸**逐字未变**。
  - **已知限制**：`upload_texture_png` 只验了原生侧的拒绝分支，**未在 WebGL2 上端到端跑过**；
    web 运行时的真实资源是打包格式，所以下一片要补 `upload_texture_packed` 与流式图集重传。
- **真渲染器在 WebGL2 上"能测了"，而且真的画出来了（WP5 第一片）**。WP2 只回答了
  "wgpu 的 GL 后端能不能用"，这一片开始问 WP5 真正的问题：本仓那台按 WebGPU 规格写的渲染器
  接到 WebGL2 上能不能画对。做法是**不做玩具渲染器**，在浏览器里直接跑 `engine::host::Renderer`
  离屏渲一帧，用像素与它自己的帧计数判据。
  - **先补两处"跑不起来"**：wasm 的 std 没有时钟（`Instant::now()` 直接 panic），渲染器的帧计时
    改用 `web-time`（原生上就是 `std::time::Instant` 的再导出，行为零变化）；渲染器新增
    `new_offscreen_async` 入口，把等待方式交给调用方——wasm 上没有阻塞能力，
    `pollster::block_on` 里的 `Condvar::wait` 在那边是 `panic!`，原来的同步入口能过只算运气。
  - **得到一把能量这件事的尺子**：`scripts/gpu-web-probe.ps1 -Stage render`（扩的是已有的
    `gpu-web-probe` 探针，不新建 crate），页面 `render.html` 手工合成最小 2D 场景后读回像素，
    与解析算出的期望值对拍。构造期包在 error scope 里、每步写"最后到达的阶段"——两处都是为了让
    **失败也带信息**：不包 scope 的话 wgpu 的校验错误会走默认 handler，wasm 上直接 abort，
    连"哪一条被拒"都报不出来。
  - **结论与修复**：渲染器一上来就在 `render_into_texture` 之前被拒。原话是 `create_bind_group_layout`
    的 `rustlua-vector-text-bind-group-layout` 要 2 个片元存储缓冲、而设备上限是 0（WebGL2 没有存储
    缓冲，这在 WP2 已经量到）。`construct.rs` 在构造路径上无条件建三处存储缓冲，所以**这是 WP5 的
    第一个真阻塞点**。修法按**设备能力**（不是按平台 `cfg`）分叉：`!storage_capable` 时矢量文字布局
    给空绑定表、材质布局去掉实例矩阵那一条、三个缓冲去掉 `STORAGE` 用法位，三条管线换成**占位管线**，
    并把 `FrameFacts` 里对应的两个通道置假——**计划里没有这两个通道，占位管线就不可能被绑上**；
    代价可见（丢弃的 draw 进 `dropped_draws` + 构造期告警），不做静默降级。原生那条路因为能力足够
    完全不进新分支。
  - **结果**：真渲染器在 WebGL2 上**建起来了并按解析期望值精确画对**——`draw_calls=2`、`triangles=4`、
    `dropped_draws=0`、白色像素 5202/期望 5202、采样不符 0 处（同一把尺子复跑一致）。这一步只证明
    "管线在这条后端上跑通且 2D 像素正确"，**不证明性能**。
  - **顺带钉住的 wasm 事实**：目标纹理必须是管线烘死的 `Rgba8UnormSrgb`；`copy_texture_to_cpu`
    在 wasm 上会**死等**（只能 `readback_frame` 一次 + `readback_pending` 轮询，并让出宏任务）；
    世界精灵不声明视图会用单位矩阵投影（不报错也画不出来）；非哨兵纹理路径在浏览器里必然
    `std::fs::read` 失败→白兜底，所以自定义贴图要先开一个公开上传入口。
  - **验收**：`-Stage render` 绿且门已接进 `scripts/verify.ps1 -Web`（0=通过/1=真回归/3=给不出画布，
    并已按门的契约复验状态文件）；原生 `cargo test -p engine --lib host::renderer` 24 项全过（能力
    足够的路径逐字未变）；`-Stage backend` 与 wasm-lua 门复跑仍绿（`wasm=3984559` 未变）；
    `cargo tree -p game_web` 里查不到探针，出货产物未受影响。
- **GPU 后端技术验证（WP2 第二片）：现有 wgpu 渲染器在 web 上"搬得过去"，但要改三处**。
  这一片不写渲染器，只把 WP5 的地基量清楚，结论全是跑出来的：
  - **wasm 上 wgpu 默认只有 WebGPU，没有 WebGL2**（`cargo tree -e features` 里 wgpu 的
    特性链是 `default → webgpu`），而**微信小游戏没有 WebGPU**——等于现状在微信上起不来渲染。
    修法做成 `engine` 的按需特性 `webgl = ["wgpu/webgl"]`，**不默认打开**：后端不该由"恰好
    依赖了 engine"的产物替所有人埋单，发布包体积是微信的硬约束（代价实测见台账）。
  - **`Limits::default()` 在 WebGL2 上建不出设备**（真浏览器读数：被拒在
    `max_compute_workgroups_per_dimension` 这一项），必须按适配器实际能力夹取；而直接写
    `downlevel_webgl2_defaults()` 又会把 2D 贴图上限从设备实际支持的 8,192 **压到 2,048**，
    把字体图集（按长到 4,096 设计）无辜降级——所以是"请求 `adapter.limits()` 再压掉用不到的项"。
  - **wasm 上必须显式要 `Backends::GL`**：默认/`all()` 那套在浏览器里**一个适配器都拿不到**
    （WebGPU 栈报 `webgpu found no adapters`，并把 GL 记成 `not compiled in`）；wgpu 27 在 wasm 上
    按 `backends` 位选**一族**后端，"WebGPU 优先、失败退 WebGL2"要自己写两段逻辑。
  - **web 上不能走存储缓冲**：现有渲染器一共 3 处只读存储缓冲（矢量文字 2 处、3D 蒙皮 1 处），
    都**不在 2D 主路径上**——2D 的精灵批次/图集/Tilemap/位图字/UI 不需要存储缓冲也不需要计算，
    所以 WP5 是"接过去"而不是"重写"。
  - **进了门**：新增独立 crate `engine/crates/gpu-web-probe`（不依赖 engine，发布包体积不涨）+
    `scripts/gpu-web-probe.ps1`，接进 `scripts/verify.ps1 -Web`，位置在"页面级 WebGL2 能力"之后、
    产物层之前——页面能拿到 `getContext('webgl2')` 和 **wgpu** 能用这条后端是**两件事**。
    门自带红/绿/受限三分（surface 都建不出来=环境受限并打印原因；能建 surface 却拿不到适配器=判红），
    并断言"后端必须是 `Gl`、清屏绿必须读回、只读存储缓冲绑定必须被拒"，挡住"其实跑在 WebGPU 上"的自欺。
  - **限制**：探针读数来自无头 + **软件光栅（SwiftShader）**，`device_type=Cpu`，不是真机 GPU，
    更不是微信 iOS/Android；性能读数不得从此环境取，真机矩阵仍属 WP0 未验证项。
    逐条读数、限制表与两条 API 坑（`create_surface(canvas)` 在 wgpu 27 的 wasm 上编译不过；
    `device.poll` 在 wasm 上是空操作）见 `tasks/plan/M73-00-Web运行时性能重构.md` 的 WP2 一节。


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

## Unreleased — 地表还是"每块完全相同 + 交界是直线"：逻辑瓦解算、确定性变体、4-bit 自动瓦片与每格染色（M72-02）

- **症状**：M72-01 把"图集能寻址"打通之后，把星露谷的截图摆在一起看，差距没变：地面**没有变体**
  （同一种草铺满屏，相邻两块永远一模一样）、地形**交界是直线**（土块边缘四格全是同一号压边瓦）、
  **没有 per-tile 颜色**（阴影压不进地表，只有整层乘一个色）。三条在代码里都查得出根：
  `tile_uv` 是存储值的纯函数，每个非空格恰好映射一个图集位置；tint 结构上免费（顶点格式早就带
  `color:[f32;4]`），但 tilemap 侧两处赋值硬编码成不透明白。
- **改法的关键不是加功能，是把语义分成两层**：格子里存**逻辑瓦 id**（草/土/石/水），渲染时由
  「邻居掩码 + 确定性哈希」解算成**图集条目**。如果直接把图集索引写进格子，"autotile"就退化成
  "美术手工摆 16 个变体"，等于没做。所以解算只有一处真源（`Tilemap::resolve_atlas_index`），
  编辑器笔刷与 Lua `set_cell` 都刷 kind 而不是条目。
- **变体哈希必须是确定性的**：`DefaultHasher` 按进程播种，症状是地表每帧抖一次、所有画面类门全红。
  现用 FNV-1a 覆盖 `(x, y, variant_seed, 两个轴盐)`，收尾再过 splitmix64 的混洗。**混洗不是美化**——
  原始 FNV 的低比特周期等于模数，`variant_count=4` 时第一行实测就是 `3,2,1,0,3,2,1,0` 的周期点阵，
  确定性有、质感没有；这一条是门里逼出来的，改任一常量 = 全图重排，所以常量本身进了金标。
- **首版只做 4-bit 掩码（上下左右 4 位 = 16 槽），并且不假装它够用**：星露谷那种 47 变体全表与
  不规则犬牙岸线要 8-bit 掩码或手工装饰层，岸线的不规则感交给 M72-05 的 sprite 压边。
  位序按 **index** 命名不按方位命名（地图 row 0 在最底、图集 row 0 在最顶，同一句"上邻居"在两个
  坐标系里指向相反），这是这类表最容易错位且**看不出来**的地方。
- **per-tile tint 走稀疏载荷**：`tints=<cellIndex>:<tintId>,…` + `tints_rgb=…|…`，而不是逐格定长
  数字串——920 格的地图为了 16 格阴影要写 920 个数字，且数字串把 tint id 卡死在 `0..9`。
  首版只接两个来源（季节/湿润整体色调、阴影压暗），逐格手涂**没做**，因为没有编辑器 UI 支撑。
- **实测**：三条新门绿——`jhyt_m72_tile_variant_determinism`（100 帧条目序列与 UV 序列逐元素全等，
  4 变体在 32 格里分布 `[5,11,8,8]` 而非周期点阵）、`jhyt_m72_autotile_mask`（8×8 草地嵌 4×4 土块，
  交界格取到转角/T 形瓦、内芯 `0b1111`，64 格 UV 逐格等于「手写掩码→条目→子矩形」）、
  `jhyt_m72_tile_tint`（读 mesh 顶点不读像素，两个赋值点各看一遍）。
  **六个反面对照是真跑红的**：去掉雪崩收尾红在金标、掩码 bit0/bit2 互换红在 32/64 格、
  overlay 与世界通道**分别**改回硬编码白各红一次（说明两条赋值点不是被同一条断言糊住的）、
  web 垫片常数末位改 1 与 offsets 互换红在跨宿主对照。四份实现（Rust / web 的 JS / web 内嵌 Lua
  垫片 / `preview_scene.py`）同值，其中两份由门自动重测：原样切出宿主发给 Lua 的那段文本喂进引擎
  真 VM，判的是两份**解算实现**而不是两份抄本互相印证；JS 那一份由
  `node scripts/check-tile-hash-parity.mjs` 钉住。图集容量按
  `tile_kinds * (max(variant_count,1) + 16*autotile)` 校验，超出当场报错而不是画错——解算越界一律
  返回 0（不画），宁可行空也不要画成邻居的美术。
- **顺手三条**：① `SceneDraw.texture` 是 **1 基**（0 = 内建白）而世界通道的
  `SpriteDrawBatch.texture_id` 是 `paths` 的 **0 基**下标，门里按错的口径读 mesh 会得出
  "世界通道一格都没画"，症状像渲染 bug 其实是读数 bug——已在三道门里按通道分别解析并注释钉住。
  ② 两道基线门本批开工时是红的，红在上一次提交没重跑生成器：`gen-lua-api-docs.js` 的待补桩
  `282 > 275`（M72-01 新增七个 `tilemap.*` API 没补说明）与 `engine-pub-surface.mjs` 的
  `added=8`（同一批公开常量）。前者按门自己写的口径**补文档而不是放宽基线**（桩数落回 275），
  后者跑 `--update` 到 2192 项；顺带把 `684c01e7`/`68ffcdef` 遗留的行号漂移与
  `RELEASE_TOML → release_flag` 陈旧条目一起清进基线。
  ③ `cargo doc` 在 `-D warnings` 下会因**文档注释里指向私有函数**的 intra-doc 链接判红
  （`variant_for` 链接私有的 `avalanche`），clippy 抓不到这一类——改 `.scene` 之外的公开 Rust
  符号时，`cargo doc`、`gen-lua-api-docs.js`、`engine-pub-surface.mjs` 这三道是连着的。
- **回滚档**：单笔 revert。数据侧**一条都没启用**（7 张场景的载荷里 `autotile` 为 0、
  `variant_count<=1`、无 `tints=`），所以回滚代码不会坏数据，地表观感也不会变——本包交付的是
  引擎能力 + 门，地表明显长出样子属于 M72-04（图集与色阶表）→ M72-05（散布）。

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
- **【定论·三次更正后】确定性断言真的进门前后的完整证据链（WP7-89 收尾）**：运行器确实传
  `-DriveJhyt`（第 38/40/42 行）；同一份门日志 `[玩法]`/`[载荷]` 各 1 处、`[确定性]` 0 处 ⇒
  `Write-Host` **是**被捕获的、驱动段确实跑了 ⇒ `[确定性]` 缺失的唯一原因是**我那 21 行被插进了
  `if ($driveLine -match …) { … } else { throw … }` 的 else 失败分支（死代码）**。
  **已修正**（移到 `[开销]` 之后、`} else {` 之前），PSParser 0 错误，**重跑七步门 ALL-GREEN 且三个
  verify 日志各命中 1 处 `[确定性] 同一输入剧本两遍读数逐项相同（12 个计数）`** ⇒ 断言这次真的执行并通过。
  **教训**：①锚点要按"**所在分支**"确认（插在失败路径 `throw` 前 = 死代码，与第 109 轮"插进函数体内部"同类）；
  ②"日志里没有" ≠ "没执行"，先用同段里已知会打印的行做对照；③断言的落点决定它守不守得住东西。
- **门的 engine-host 用例补上"单一 tick 所有者"不变式（WP7-90）**。
  - **空白**：两个开关用例只查"宿主落在哪个具名结果"，没查"世界有没有被推进两次"——而这是接管/交还
    （WP8）的前提（两套状态各推一步 = 帧数看着对、世界跑快了，读数上最难发现）。
  - **改动**：门里加 `Assert-SingleTickOwner`，对两个开关用例逐条判：`engine_host_tick_conflict` 必须恰好
    `no`；`engine_host_input_applied` 必须恰好 `0`；`engine_host_tick` 必须是具名值 `none/runtime/host`。
    三条取自已报字段、判据是**具名取值**，并**各打印一行 `[单一tick] …`**（第 121 轮教训：断言必须在日志里
    留下**可检索**的证据行）。
  - **结果**：七步门 **ALL-GREEN**；日志含两行 `[单一tick]` ⇒ 断言执行且不变式成立。
  - **一处操作失误（如实记录）**：本轮运行器重写时 `WriteAllText` 参数写错（4 个）⇒ 运行器没改成 round122，
    门把日志写回了 `round121-*.log`（覆盖第 121 轮运行器日志；决定性证据 `round121b-*` 未受影响）。
    **教训**：改完脚本要立刻回读确认改动生效，别只看 exit code。
  - **状态**：门的判据连续三轮增强（契约 33 条 / 确定性 12 计数 / 单一 tick 所有者）；运行时源码未改。- **接管/交还的准确定位（WP7-91，含一处行号归属更正）**。
  - **更正**：上轮我写的 `engineHostEnabled()` 1648 / `engineHostTickEnabled()` 1662 /`engineHostOwnsTick(on)` 1676 /
    `engineHostFinish()` 1683 是**适配层**（`plugins/wechat-minigame/wechat-adapter.js`）的行号，**不是 harness 的**
    （harness 里 `function engineHost*` **0 命中**）。⇒ 又一次"别信记忆里的行号，去查"（本轮第 3 次同类）。
  - **harness 只有两个启动期开关**：`--engine-host`（206 行）、`--engine-host-tick`（207 行）；
    **没有**运行中触发器（`engine_host_off|handback|交还|engineHostOff` 0 命中）⇒ 表达不了"接管→交还"。
  - **但交还机制本来就有**：适配层 `engineHostOwnsTick(false)` **4 处**（1737/1759/1817/1854 行）、
    `engineHostFinish()` 6 处 ⇒ 缺的只是**测试触发器**。往返用例成本因此从"设计交还机制"降到
    "接一个开关到既有释放路径"。
  - **下一轮**：实现 `--engine-host-off-at N`（harness 解析 + 适配层调用 + 门用例），判据 =
    第 122 轮 `Assert-SingleTickOwner` + 第 121 轮 12 个计数（交还后与"从未接管"一致），
    跑门 ALL-GREEN 并**在门日志里检索到新证据行**。
  - **状态**：运行时源码未改。- **交还语义定死 + 发现一个"够不着"问题（WP7-92）**。
  - **语义（读源码）**：`engineHostOwnsTick(on)` 是"**世界由谁推进**"的标记 —— 起宿主前默认运行时拥有
    （1737），**只有 `begin` 成功之后**才交给宿主（1748，注释"现在才把'世界由它推进'的标记交给它"），
    失败/降级**必须还回**（1759，注释"否则世界会冻在'脚本还在画、update 不再跑'的状态"）。
    而 `engineHostFinish()` 是**宿主收尾/拆除**。⇒ **运行中交还 = 只调 `engineHostOwnsTick(false)`，
    不调 `engineHostFinish()`**（宿主继续渲染，只是不再推进固定步）。
  - **实现前提（本轮提前发现，省一轮返工）**：`engineHostOwnsTick` 是**适配层内部闭包**，不是 `window`
    公开入口 ⇒ harness **够不着**。往返用例必须先由适配层开一个**测试入口**，否则 harness 的开关是空壳。
  - **往返用例真正要回答的**：**运行时是不是每帧都在读这个标记**？若只在启动时读一次 ⇒ 中途还回标记
    **不会**真的交回固定步 ⇒ "接管→交还"在当前实现下**根本不成立**（这正是要被用例抓出来的）；
    若每帧都读 ⇒ 交还后 `timing` 应以 `tick=runtime` 收尾、`input_frames==run_steps` 重新成立。
  - **状态**：运行时/适配层源码未改；门的判据维持三轮增强状态。- **"谁拥有 tick"的标记是每帧读的 ⇒ 往返可表达，且不需要新增适配层 API（WP7-93）**。
  - **上一轮的决定性问题**（运行时是否每帧读标记？只读一次则"接管→交还"根本不成立）答案：**每帧都读**。
    证据：适配层 `wechat-adapter.js:1677` 把标记写成**全局** `globalThis.__RUSTLUA_ENGINE_HOST_OWNS_TICK`；
    运行时 `web-runtime.js:7167-7169` 的 `hostOwnsFixedTick()` 读它，并在 **7248 行每帧调用**，
    据此决定 `simSteps`（7249）、输入/dt 路径（7267/7272）与读数 `tick=host|runtime`（7312）。
  - **成本再降**（第 123 轮"接触发器" → 本轮"改一个全局 + 一条门用例"）：**不需要**在适配层开测试入口
    （标记本来就是 `globalThis` 上的，第 124 轮担心的"够不着"不成立）、**不需要**碰 `engineHostFinish()`。
  - **下一轮判据**：`--engine-host-tick --engine-host-off-at 15` 下自报出现 `engine_host_handback_frame=15`；
    交还后 `tick=runtime` 且 `timing` 以它收尾；`input_frames == run_steps` 重新成立；
    `Assert-SingleTickOwner` 全程成立；12 个计数与"从未接管"一致；跑门 ALL-GREEN 且日志能检索到这些字段。
  - **未验证**：静态只证了"每帧读"；"交还后确实由运行时推进且没被推进两次"要等用例跑出来；
    真机侧（宿主真起来后交还）仍不可离线验证（离线档位宿主从未 `running`）。
  - **状态**：源码未改；**连续 3 轮无代码改动**（123/124/125），下一轮必须落地。- **"接管 → 交还"往返用例落地，门 ALL-GREEN（WP7-94）**。
  - **改动三处**：harness 加 `engineHostOffAt` 与 `--engine-host-off-at N`（同时开 `engineHost`/`engineHostTick`），
    并在**泵帧循环内**（第 1784 行附近）于第 N 帧把 `globalThis.__RUSTLUA_ENGINE_HOST_OWNS_TICK` 置 `false`
    且打一行 `wechat_minigame_harness_handback frame=N`；门新增用例 `--frames 30 --engine-host-off-at 15`
    （标签 `engine-host-handback`）。
  - **断言**：日志里**真的出现**交还行（否则 = 开关没生效）＋ `Assert-SingleTickOwner`（conflict=no /
    input_applied=0 / tick 具名）＋ `timing` 段以 **`tick=runtime`** 收尾。
  - **关键事实（读源码得出、本轮实测确认）**：运行时**每帧**读该标记（`web-runtime.js` 的
    `hostOwnsFixedTick()` 在 7248 行每帧调用）⇒ 交还只需改一个全局，**不需要**新适配层 API、
    **不需要**碰 `engineHostFinish()`。
  - **实际结果（含重要更正）**：**第一次跑门是 RED**（`verify-default=1 / verify-gpu2d=1 /
    verify-default-restore=1`），而运行器仍打印 `runner exit=0` ⇒ **运行器退出码不等于门结果**，
    门只能读 `SUMMARY`/`ALL-GREEN`/`RED:`。根因：我插入交还逻辑的泵帧循环**只在驱动路径上跑**
    （围绕 `driveFrame`）⇒ 必须带 `--drive-new-game`；补上后默认与 restore 档转绿，
    但 **`verify-gpu2d` 仍红且原因未取到**（运行器日志名固定，第二次运行把第一次的日志**覆盖**了）。
    为不留红灯：**门里那条用例已回退**，重跑门 ⇒ **ALL-GREEN**。
  - **因此本轮准确交付**：harness 侧 `--engine-host-off-at N` **已落地且实测有效**
    （诊断：`--frames 30 --engine-host-off-at 15` ⇒ 交还行 0；加 `--drive-new-game` ⇒ 交还行 1、
    `wechat_minigame_harness_handback frame=15`、`tick=runtime`）；**门里的往返用例未落地** ⇒
    往返**尚未**被门守住；树的门状态 ALL-GREEN。顺带**实测确认**了"运行时每帧读 tick 标记"。
  - **下一步**：①重跑并**立刻**读 gpu2d 日志（每次用不同前缀，教训：证据会被同名日志覆盖）；
    ②若与档位语义有关则**按档位分别断言**而非删用例；③用例补上后重跑门，判据含 `SUMMARY` 全 0。
  - **证明/不证明**：证明标记语义、单一 tick 拥有者、输入不被应用两次、开关真的生效；
    **不证明**真宿主交还（离线档位宿主从未 `running`）⇒ 属设备侧待验（已写死在用例注释里）。
  - **三处操作摩擦（都被"写盘前抛错"拦住，未造成坏改动）**：①锚点未容前导空白 ⇒ 0 命中；
    ②`^\s*stats.frames\+\+;` 命中 9 次 ⇒ 判重拦下；③第一版漏了参数解析。
    教训：锚点必须容前导空白并**先证明唯一**（本轮做成脚本内自动挑唯一锚点）。
  - **状态**：运行时源码未改（改的是 harness 与门）；门的判据累计四项（契约/确定性/单一 tick/往返交还）。- **gpu2d 档失败的复现与原始输出（WP7-95）**：重打 GPU2D 包（`package-gpu2d exit=0`）后，用**与门里完全相同**
  的参数跑 `--frames 30 --drive-new-game --engine-host-off-at 15` ⇒ `harness exit=0`，但
  **`wechat_minigame_harness_handback` 0 处**，且**连 `wechat_minigame_harness_ok` 都没输出** ⇒
  这一档下 harness **没走到正常结算段**（与默认档"正常输出 OK 行 + 交还行"不同）。
  ⇒ 同一条用例**不能**用同一套参数覆盖两个档位；要么**按档位分别断言**，要么先弄清 gpu2d 档为何不结算
  （这可能本身就是**真问题**：同一开关在 GPU 档下让流程走岔）。轮次文档记了四条由复现脚本打印的观测（`harness exit=0`、交还行 0、无 OK 行、默认档已恢复）；**更正**：文档里那段"逐字输出"其实是该日志尾部=恢复默认包那一段（打包器输出 UTF-16 且排最后），不是 harness 输出段，已在文档里标明。
  **状态**：门仍 ALL-GREEN（用例未进门）；默认档包已恢复（`package-default-restore exit=0`）。- **gpu2d 档根因查明（含一处假阴性更正）+ 修它的尝试失败并已回滚（WP7-96）**。
  - **根因**：把 GPU 档 harness 输出单独落盘（UTF-8）后事实相反 —— `wechat_minigame_harness_ok`
    **存在**，三条单 tick 断言**全过**（`conflict=no` / `input_applied=0` / `timing … tick=runtime`），
    唯一缺的是 **`wechat_minigame_harness_handback`（0 处）**；该档下 `move=not-driven` ⇒ 我把交还逻辑
    放在**驱动路径的泵帧循环**里，那段没跑 ⇒ 门红。**该档位没有语义问题**。
  - **上一轮的假阴性**：用 `& node … *>> $log`（PowerShell 写成 **UTF-16**）却用 `Select-String -Encoding UTF8`
    搜 ⇒ 搜不到，误判"没有 OK 行"。**教训：写日志与搜日志的编码必须一致**。
  - **修复尝试失败**：改挂到 `requestAnimationFrame` 垫片（路径无关，思路对），但**把语句插进了对象字面量
    内部**（`requestAnimationFrame: (callback) => {…}` 是属性位置）⇒ `SyntaxError: Unexpected token ':'`
    ⇒ 门 RED（三个 verify 全红）。**已回滚**：harness 恢复到本轮之前可用备份（`node --check`=0）、撤掉门里用例、
    重跑门 ⇒ **ALL-GREEN**。净结果：**往返仍未入门**（harness 那个开关也已随备份移除）。
  - **教训（第 4 次"锚点上下文"类错误）**：①第 109 轮插进函数体内部；②第 121 轮插进 `else` 失败分支（死代码）；
    ③本轮插进对象字面量（语法错误）⇒ **插入 JS 前必须先判定插入点是"语句位置"还是"表达式/属性位置"**；
    JS 用 `node --check`、PowerShell 用 `PSParser` 立刻自检（本轮正是它抓到的）。
  - **状态**：门 ALL-GREEN（无残留红灯）；运行时源码未改。- **接管→交还往返用例进七步门，门 ALL-GREEN（WP7-97）**。
  - **两个要点（前几轮踩出来的）**：①触发点必须是**语句位置**（挂在 `requestAnimationFrame` 回调体内部，
    用**字符串替换**而非按行插入——第 128 轮按行插进**对象字面量属性位置**直接把 harness 插成 `SyntaxError`）；
    ②触发点必须**路径无关**（第 127 轮放在**驱动路径泵帧循环**里，GPU 档 `move=not-driven` 触发不到）。
  - **本轮自己抓到的坑**：条件写成 `… && __RUSTLUA_ENGINE_HOST_OWNS_TICK !== false` ⇒ GPU 档下宿主机启动即失败、
    标记早就是 `false` ⇒ **守卫把"留痕"也吞掉** ⇒ 交还行 0。去掉守卫（赋值幂等，**留痕无条件**）后两档都出现
    `wechat_minigame_harness_handback frame=15` 且 `timing` 以 `tick=runtime` 收尾。
    **教训：断言/留痕不要写成"只在状态变化时记录"**——那会把"本就处于目标状态"变成"没有证据"；
    **证据无条件产生，判断留给断言**。
  - **门的用例**（标签 `engine-host-handback`，`--frames 30 --engine-host-off-at 15`）：断言 ①日志真的出现交还行
    （否则=开关没生效）；②`Assert-SingleTickOwner`（conflict=no / input_applied=0 / tick 具名）；③`timing` 以
    `tick=runtime` 收尾。**结果：ALL-GREEN**，三个 verify 步骤日志均可检索到交还证据行。
  - **证明/不证明**：证明标记语义、单一 tick 拥有者、输入不双份、开关真生效，且**默认与 GPU 两个档位都成立**；
    **不证明**真宿主交还（离线宿主从未 `running`）⇒ 设备侧待验（写死在用例注释里）。
  - **状态**：运行时源码未改；门判据累计四项（L3 契约 / 确定性 / 单一 tick / 往返交还）。- **长帧/追赶上限只存在于宿主路径；运行时路径待查（WP7-98）**。
  - **硬事实**：`max_catch_up_steps: 8` / `max_frame_seconds: 0.25` / `updates_per_second: 60`
    **只出现在适配层交给宿主的 options 里**（`wechat-adapter.js:1722-1724`）；`web-runtime.js` 全文检索
    `max_frame`/`catch_up`/`clamp` **没有任何 dt/长帧截断**（命中的 `clamp`/`Math.min` 全是颜色、坐标轴、
    音量、UI 环索引之类）。
  - **要害**：`web-runtime.js:7152` 注释自述"累积器读数同样来自宿主（**JS 那份已删**）"，但
    **7249 行**仍是 `hostTick ? hostFixedSteps() : steps`、**7272 行**仍是 `if (!hostTick && steps > 0)`
    ⇒ **运行时自己推进时 `steps`/`droppedSteps` 由谁算、有没有上限，必须看代码才算数**。
    若存在**第二份 JS 实现且无上限**，那正是"统一 Rust/WASM 状态、移除旧热路径"要消灭的东西
    （长帧 ⇒ 一次推进几十步 ⇒ 卡顿/穿模）。
  - **下一步（一次读即定论）**：读 `web-runtime.js` 第 7225–7300 行，确认 ①`steps`/`droppedSteps` 定义处
    ②`!hostTick` 时是重算还是缓存 ③有无无上限追赶。判定后：是命名残留 ⇒ 改注释/命名；
    是真第二份实现 ⇒ 删残留或补与宿主一致的上限，并在门里加"长帧被截断"用例。
  - **状态**：运行时源码未改；门判据维持四项。- **第 130 轮的两点担忧都已解除：固定步"归一"（WP7-56）早就做完了（WP7-99）**。
  - **逐行结论**：`web-runtime.js:7243` `const steps = advanceFixedSteps(elapsedSeconds);` ⇒ 这个数**不是**
    JS 自己算的；7246-7247 注释写明 `lua_fixed_frame` 是"这一帧本该走几步"的**权威读数**（含丢帧计数）
    ⇒ 由**宿主时钟**给出；7249 `hostTick ? hostFixedSteps() : steps`；**7273-7275** 注释原文
    "**自 WP7-56 起只有 wasm 一份实现**：`lua_bridge.rs::lua_fixed_run_steps`。**这里以前是一段等价的
    JS 循环，删掉它才是"归一"**"；7276 `result = runFixedSteps(steps)` ⇒ 运行时路径也是**调用 wasm**。
  - ⇒ ①**不是**第二份实现（JS 循环 WP7-56 已删）；②**上限单源**：`max_catch_up_steps: 8` /
    `max_frame_seconds: 0.25` 在宿主时钟里，而 `lua_fixed_frame` 这个读数就来自同一时钟
    ⇒ 两条路径拿到**同一个已被限幅的数**，运行时侧不需要第二份 clamp。
  - **唯一剩余（低风险、下一步做）**：代码对但"数是谁算的"看不出来 ⇒ 给 `steps` 加注释/改成更直白的名字
    （如 `authoritativeSteps`），并在 `advanceFixedSteps` 定义处标明"不自己攒累加器——累加器只有宿主一份"，
    让"一个时钟、一个实现"在代码里一眼可见。
  - **状态**：运行时源码未改；门判据维持四项；**固定步这条线（统一状态/移除旧热路径）达标**。- **固定步"单源"被结构性证实；存档不含实体 id（混叠风险不成立）（WP7-100）**。
  - **固定步**：`web-runtime.js:7111 advanceFixedSteps` 开头就 `fixedClockBridge()`，缺失即
    `throw new Error('wasm 缺 lua_fixed_frame 导出…')`（7119 再对 `lua_fixed_run_steps` 抛一次，
    原文点名"固定步的执行顺序自 WP7-56 起只有 wasm 一份实现"）⇒ **没有"拿不到宿主时钟就自己攒一个"的退路**
    ⇒"JS 侧第二份实现"在**结构上不可能存在**（硬失败，不是注释约定）；第 131 轮"上限单源"结论加强成立。
    另注：7126-7127 说明那行时钟日志是**门断言的原文**，只能追加、不能改写。
  - **实体 id 与存档**：JS 侧检索 `nextEntityId`/`entityId =`/`spawnEntity`/`freeIds`/`recycle` ⇒ **0 命中**；
    实体 id 是引擎侧 `SceneEntityId(u64)`；`rustlua-gameplay/src/save.rs` 与
    `rustlua-scripting/src/script/api/save.rs` 检索 `entity`/`id:`/`alive`/`generation`/`reuse` ⇒ **0 命中**
    ⇒ **存档格式里没有实体 id** ⇒"恢复旧存档后新实体复用仍存活 id"**无从发生** ⇒ 本条风险**在现有存档路径上不成立**。
    存档落盘出口只有一个字符串槽（`__SAVE_STORAGE_PREFIX = 'rustlua:save'`，与 v2 编码"同形"）。
  - **边界**：只覆盖这两个 `save.rs` 定义的格式；编辑器/工具的文档保存（有 `doc_id`）**未检查**；
    Lua 兼容层 `__rustlua_save` 只存字符串，内容仍由 v2 决定。
  - **状态**：运行时与引擎源码均未改；门判据维持四项；本轮排除了两条"疑似正确性问题"。- **`keyName()`→`event.code` 查清：不是 bug，是文档化的有意设计；本轮落地注释澄清（WP7-101）**。
  - **查清**：`web-runtime.js:152-155` 原文写明"本侧键名取自 `event.key`，它分不出左右修饰键、也分不出
    小键盘与主键盘同形键 ⇒ 这些名字只能**合并**……但**不能默默做**"，且有 `LOSSY_PHYSICAL_KEYS`
    单列 + 未映射计入 `unmapped`（无静默）⇒ 现状是**有损合并被显式枚举并有计数**。
    所以"改用 `code`"是**改设计**（缩小已知有损集合），**不是修 bug**；**单方面改 `keyName()` 会让绑定表
    与键名错位**（注释指明名字换算只有一份权威：宿主 `physicalKeyToName` ↔ `keyNameToBindingText`）。
  - **正确做法（下一步）**：①只用 `code` 拆分**已枚举**的有损合并（左右修饰键、小键盘同形键），其余仍用
    `event.key`；②**与 `keyNameToBindingText` 同改**；③harness 假输入**本来就带 `code`**
    （`{key:'ArrowUp',code:'ArrowUp'}`、`{key:' ',code:'Space'}`）⇒ 门里可断言"左右 Shift 不再互相触发"
    且 `unmapped` 计数下降（两条都是可检索证据）。
  - **本轮落地**：`web-runtime.js` 两处**纯注释** —— `const steps = advanceFixedSteps(…)` 上方写明 `steps`
    来自**宿主时钟**（含丢帧计数与追赶上限 8 步 / 0.25 秒）；`function advanceFixedSteps` 上方写明
    "**不自己攒累加器**，累加器与上限只有宿主一份；拿不到就**抛错**而不退回 JS"。
    **验证**：`node --check`=0、函数签名未变、**七步门 ALL-GREEN**（`SUMMARY` 七项全 0）；改前已备份。
  - **状态**：运行时仅注释变化（无逻辑改动）；门判据仍四项；键盘那条**属改设计**，按上述三步走。
