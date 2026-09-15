# RustLua / game_jhyt 执行任务计划

## 执行原则
1. 先修质量信号，再修数据安全，再做引擎性能与游戏内容。
2. 每项任务必须有代码证据、回归测试或可复现验收。
3. 不重复已完成能力；以当前源码为准，历史文档仅作线索。
4. 每个阶段独立可验收，遇到环境限制记录，不以静默跳过代替通过。

## 阶段一：质量门与失败传播（P0）
- Q1 修正 `.github/workflows/quality.yml` 的 game_jhyt 路径与 `examples/**` 触发范围。
- Q2 将 `scripts/verify.ps1` 的 headless 清单改为缺失即失败，并纳入所有当前样例。
- Q3 将 game/editor 运行器的 Lua、场景、渲染初始化失败传播为非零退出码。
- 验收：故意缺失样例或注入错误时质量门失败；正常 MVP 全部执行。

## 阶段二：存档与新游戏隔离（P0）
- S1 实现 save key/value escape/unescape，兼容旧格式并区分损坏/无存档。
- S2 provider 恢复前全量校验，失败不留下半恢复状态；补版本门/迁移策略。
- S3 审计并测试 new_game 对 map.state、save_data、实体、farm/gather 等状态的完整清理。
- 验收：多行/CJK 存档往返；故障 provider 回滚；继续后新游戏无旧局状态。

## 阶段三：引擎与工具可靠性/性能（P1）
- E1 Scheduler 执行序缓存，修复注册失败丢系统。
- E2 2D 顶点缓冲动态增长、溢出告警和计数。
- E3 自定义字体图集 uploaded_version/空帧防线。
- E4 assetc staging 原子发布、输出目录包含校验、不可信输入限额。
- E5 编辑器恢复错误可见；Inspector 脏标记与 Prefab 缓存；关键面板热点优化。
- 验收：相关单测、基准或计数器；无新增 clippy 警告。

## 阶段四：game_jhyt 核心体验（P1）— 已完成
- G1 实现可验证的 7 日目标/里程碑与日结算联动。**（已完成，见执行记录）**
- G2 最小战斗 Hitbox/AttackHit/命中去重/击退/硬直/无敌帧。**（已完成）**
- G3 战斗、掉落、死亡回庄、任务和主线订单端到端可达。**（已完成：掉落表 + 掉落入库 + 战败回庄 + 猎获订单 + `jhyt_m28_loop`）**
- G4 扩展 validate_data 至 crops/items/tools/farm/gather/enemy_art 及资源引用。**（已完成）**
- G5 对齐数值设计案与 Lua 数据，建立 invariant/headless 断言。**（已完成）**
- G6 统一 HUD 事件/失败反馈并补中文兜底。**（已完成）**

## 阶段五：Alpha 产品化与收口（P2）
- P1 已有资源接线或下架；天气/季节可见变化。**（已完成：四季地表 + 按日天气 + 雨天可见变化 + HUD 资源条 + `docs/48` 资源决定表，验收 `jhyt_m29_season_weather`）**
- P2 中文排版、缺字、溢出检查。**（已完成：`ui.measure_text`/`ui.text_get` + `ui/text_fit.lua`/`ui/ui_text.lua` 唯一切口 + 全面板按真实字宽排版 + 首包缺字覆盖，验收 `jhyt_m30_cjk_text`，契约 `docs/49`）**
- P3 PC 干净发布、用户配置目录、真实手柄与 WebGL 完整验证。**（已完成：`core::user_dir` 用户数据目录 + 播种/升级不覆盖 + `set_settings_root` + `core::project_config`，验收 `jhyt_m31_pc_release`，契约 `docs/50`；WebGL 产物层实测暴露出运行时缺 17 个宿主 API 全局，见执行记录与 backlog 4-24）**
- P4 文档路径/编号/状态、未跟踪文件和死代码收口。**（已完成：`scripts/check-docs.ps1` 文档门进 `verify.ps1`；历史路径 39 处清零；重复 42 号重编为 50；编号命名空间写入 `docs/README.md`；死代码 `lua_ms`/`plugin_status`/`RecoveryManager::clear_all` 接线；`docs/40` 编码事故重建）**

## 阶段出口
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- `game --check` / `editor --check`
- `scripts/verify.ps1 -Mvp`
- 记录无法执行的 GPU/WASM/实机项及原因。

---

## 执行记录

### 阶段一（质量门与失败传播）：已完成

| 项 | 落地 | 证据 |
|---|---|---|
| Q1 | `.github/workflows/quality.yml`：`paths` 覆盖 `engine/**`、`examples/**`、`scripts/**`、`docs/**`、`tasks/**`；两处工程校验改为 `--project "${{ github.workspace }}\examples\game_jhyt"`；新增 `Game headless regression suite`（`scripts/verify.ps1 -MvpOnly`） | 工作流文件 |
| Q2 | `scripts/verify.ps1`：headless 清单成为唯一事实来源——清单文件缺失 `throw`、盘上存在未登记的 `jhyt_*.rs` 也 `throw`、执行数量必须等于清单数量，并打印 `executed=N expected=N` | 探针实测：临时放入 `jhyt_zzz_probe.rs` → 质量门立刻以「not in the MVP list」失败（探针已删除） |
| Q3 | `game_app.rs` 记录首个失败原因（`fail()` + `failure()`），`main.rs` 把失败转成 `Err` → **非零退出码**；覆盖 Lua 固定步异常、`RenderOutcome::Fatal`、渲染器初始化失败、运行时场景初始化失败 | `cargo test -p game` 中 `runtime_failure_must_not_exit_zero`、`failure_message_is_attached_to_the_process_error` |

### 阶段二（存档与新游戏隔离）：已完成

| 项 | 落地 | 证据 |
|---|---|---|
| S1 | `save.rs` 升到 `CURRENT_SAVE_VERSION = 2`：写入转义行（`\\`/`\n`/`\r`/`\t`/其它控制字符 `\xNN`），读取同时接受 v1（原始行格式）与 v2；无版本头 / 版本 0 / 高于当前版本明确失败且不覆盖原文件 | `cargo test -p engine --lib core::save`：9 项含多行 CJK 往返、v1 旧档读取、坏转义拒绝、仿冒版本头的键仍按数据处理 |
| S2 | `save_manager.lua`：restore 前逐 provider `validate` 预校验 → 捕获回滚快照 → 全量应用并**收集**失败（`false, reason` 也算失败）→ 失败整体回滚；快照 v1→v2 **迁移**（不再硬拒绝）；`save.load()` 返回 `(ok, reason)` 区分 `missing` 与 `corrupt:`，损坏档保留不覆盖 | 新增 `jhyt_m22_save_isolation` ⑥章：注入故障 provider → `load()` 返回 false 且 day/seeds 回滚不变；游戏侧 `save_manager.migrate` 使 v1 快照可恢复（`jhyt_m18_stats` 旧断言同步更新） |
| S3 | 新增引擎侧 `save.clear()` / `map.state_clear([name])`；`new_game()` 先清 `__map:*`/`__mapstate:*`/`__blob:*`，再重置 farm/gather/inventory/UI/时钟 | `jhyt_m22_save_isolation` ⑤章：耕作+播种+浇水+采集+睡到第 2 天后开新游戏 → day=1、体力 100、种子 8、地块 nil、FarmGrid 全 0、采集点 0、`map.spawn()` nil |

### 阶段三（引擎与工具可靠性/性能）：已完成

| 项 | 落地 | 证据 |
|---|---|---|
| E1 | `Scheduler` 缓存拓扑执行序（注册变化才失效）；`RuntimeScene` 的 step/frame 调度器**跨帧保留**（借出→运行→放回），不再每帧重建、每步重算；修掉「注册失败丢系统」（原 `drain` + `break` 会让剩余系统随 `Drain` 一起消失） | `core::scheduler` 新增 `cached_execution_order_is_invalidated_by_registration`、`rejected_registration_leaves_the_schedule_intact`；`runtime_scene` 新增 `rejected_registration_never_drops_registered_systems`（断言失败注册后内建系统一个不少且仍执行） |
| E2 | 2D 顶点缓冲按需扩容（`next_2d_vertex_capacity`：≥ 初始 1 MiB、按 2 的幂），扩容写 warn 日志并计数；替掉「固定 1 MiB + 无 else 分支」的静默丢帧 | `two_d_vertex_capacity_grows_to_powers_of_two`（容量始终 ≥ 请求且为 2 的幂）；新增 `vertex_buffer_growths/capacity/peak` 只读诊断 |
| E3 | 自定义字体图集空像素帧补版本墓碑：仅当已上传过该版本才续用 bind，否则丢弃过期 bind 跳过本帧；`snapshot_custom_fonts(will_draw)` 与默认图集同一判据，空网格帧不再推进「已上报」版本 | 默认图集防线同源（此前已修）；`runtime_scene` 相关单测全绿 |
| E4 | assetc：输出目录等于/位于项目内部、项目位于输出内部一律拒绝（规范化后比较，`..` 绕不过）；符号链接解析后必须仍在扫描根内（越界报错而非跟随复制）；清单 `strip_prefix` 失败返回可定位错误 | `assetc` 17 项含 `package_rejects_output_inside_or_equal_to_project`、`canonical_or_absolute_resolves_missing_paths`、`path_containment_detects_escapes`、`collect_files_rejects_symlink_escaping_the_project`（本机无符号链接权限时自动跳过） |
| E5 | 编辑器恢复扫描失败不再 `unwrap_or_default()` 静默成「没有待恢复内容」：启动/导入写入状态栏或进度日志，运行期 `refresh_recovery_scan()` 保留上一份结果并记 error；源 Prefab 按 (长度, mtime) 缓存，Inspector Override 摘要不再每帧 `Prefab::load` | `prefab_source_cache_hits_and_invalidates_on_file_change`（命中不重读、外部改动后重载） |

### 阶段四（G1–G6 全部完成）

阶段四进度：G1 ✓ / G2 ✓ / G3 ✓ / G4 ✓ / G5 ✓ / G6 ✓。G3（战斗→掉落→死亡回庄→任务/主线订单
端到端可达）是最后完成的一项；阶段四至此收官，后续进入阶段五（P1–P4）。

#### G3 战斗/掉落/死亡回庄/任务端到端可达：已完成

此前这条链是断的：`data/enemies.lua` 没有掉落字段（打赢没有任何产出），`combat.respawn()` 是"原地
复活"（`docs/33` §6.3 要求的"回山庄 + 扣未入库掉落 + 扣铜钱"一条都没实现），任务链里也没有任何吃战斗
产出的订单。本轮把设计案 §6.2/§6.3 第一次落到实现里，并用一条真实玩法路径的验收锁住。

| 项 | 落地 | 证据 |
|---|---|---|
| 掉落表 | 新增 `scripts/data/drops.lua`（`version=1`）：一条敌人一条记录，字段闭集 `enemy`/`money{min,max}`/`items[]{id,min,max,chance?}`；`validate_all()` 拒绝结构漂移（敌人/物品必须真实存在、同一敌人不允许两条记录、`0 <= min <= max` 且整数、`chance ∈ (0,1]`、多写的键直接报错、**"写了记录但什么都不产出"也报错**），已登记进 `validate_data.lua`（校验器 20→21） | `jhyt_m28_loop` ①：表结构 + 排序 + 6 个负例（未知敌人/未知物品/区间倒置/概率越界/多写键/零产出）+ `get('enemy_dummy') == nil`（训练木人是练功靶，不该产出资源）；`jhyt_m23_data_validation` 的登记覆盖断言自动变 21/21 |
| 掉落物 | `data/items.lua` 新增狼皮 `wolf_pelt`、兽肉 `boar_meat`、山魈内丹 `ape_core`（主线物品，`buy/sell = 0` 卖不掉）；图标由 `tools/gen_item_icons.py` 补三个画法后生成（`--check` 21 个 32×32 全部通过） | `jhyt_m23_data_validation` ⑥：31 个资源引用逐个 `is_file()`（含三张新图标）；①章物品数 7→10 |
| 掉落运行时 | 新增 `scripts/systems/loot.lua`：铜钱**当场入账**、物品进"**当日待入库**"袋（`state.vars` 的 `loot_p_<item>` 标量，随既有 state provider 进出存档，不需要新 provider），跨天 `bank()` 一次性入库；随机序列是可持久化的确定性 LCG（`loot_rng`），同一存档同一击杀顺序 → 同一掉落，`set_seed()` 供测试固定序列；HUD 反馈 `loot_<item>` →「战利品：狼皮」、只掉钱走 `loot_money` | `jhyt_m28_loop` ②：**真实 J 键**打死灰狼 → 铜钱 +2（种子 1）、`pending('wolf_pelt')=1`、`last_action=loot_wolf_pelt`、`hud.feedback` 出中文、**背包里此刻还没有这张皮**（"未入库"是字面意思）、木人打不出任何掉落；③：**真实 T 键**睡觉跨天 → 皮入库、袋清零；⑧：存档往返后待入库袋与随机序列都还原 |
| 战败结算 | 新增 `scripts/systems/death.lua`：损失待入库掉落 50%（向下取整）+ 当前铜钱 5% → `combat.respawn()` → 体力/内力回满 → `map.goto('manor', 出生点)` → HUD 一行中文明细（`death_penalty_` 前缀按原样拼接）；`penalty()` 是只读预算、`last_penalty()` 是结算明细；上限改由 `player/inventory.lua` 的 `max_energy()`/`max_inner_power()` 单一来源提供（`main.on_day_start` 同步改用它，不再写死 100） | `jhyt_m28_loop` ⑤⑥：先站到玄冥会徒身边再把气血压到 3 点 → **真的被咬死**（`stats().deaths=1`，`death.penalty()` 此刻还没落账）；**真实 R 键** → 回山庄出生点 `528,48`、气血/体力/内力回满、铜钱 142→135（−5%）、待入库 4→2、HUD 明细含「战败回庄」、战斗日志含 `death`+`respawn`、敌人已复活；已入库的狼皮与 `order_hunt=in_progress` 不受影响（§6.3 的"保留"项逐条断言） |
| 接进战斗与任务 | `systems/enemy.lua` 死亡分支调 `loot.on_kill`（无掉落记录不写状态、`pcall` 兜住、不影响战斗结算）；`main.on_day_start` 先 `loot.bank()` 再结算目标；新增订单**山道猎获 `order_hunt`**（狼皮 ×1 + 兽肉 ×2 → 90 文 + 林小满关系 6），对话扩进林小满那一条文件（`dialogue.find_for_npc` 每个 speaker 只取第一条对话） | `jhyt_m28_loop` ④⑦：山道真实击杀野猪（兽肉 ×2）与山贼（木料 ×2）→ 接单 → 战败后重打野猪 → 跨天入库 → 向林小满**真实对话交付**：`order_hunt=completed`、+90 文、狼皮/兽肉被消耗、关系 +6；任务数 8→9（`jhyt_m19_slice` 同步） |
| 文档口径对齐 | `docs/33` §6.2 拆成「Alpha 目标口径」+「**MVP 落地口径**」两张表（毒蝎条目写的是"草药"——注册表里没有这件物品，落地为 `lingzhi`；山贼的"木材"落地为 `timber`），§6.3 每条死亡规则标注实现位置 | `jhyt_m28_loop` ①b **直接解析 §6.2 的 MVP 表**，与 `data/enemies.lua`（名称/生命/攻击力/攻击间隔）和 `data/drops.lua`（掉落）逐格比对，且行数必须等于敌人数量（新增敌人不补表即失败）；`jhyt_m18_combat` 的"R 回出生点"改为"R 回山庄 + 状态回满 + 敌人复活"（行为变更本身），`jhyt_m23_data_validation` 物品数 7→10 |
| 顺带修掉两个质量门抖动（都是真实缺陷） | ① `editor/src/panels/domain_data.rs` 的夹具清理原本是 `fs::remove_dir_all(root).expect("cleanup")`，Windows 上会因 Lua 运行时未释放文件句柄偶发 `错误 32`，全量并行跑测试时随机判失败 → 改为有界重试的 `cleanup()`（清理失败只留带纳秒后缀的临时目录，绝不判测试失败）。② 引擎资源数据库保存用**固定名**临时文件 `resources.db.tmp`：两个写入者同时保存同一工程时，先完成的 rename 会取走临时文件，后一个 rename 立刻以「系统找不到指定的文件（os error 2）」失败（实测在 `import_spawn_end_to_end` 上随机复现）→ 临时名改为带进程号 + 自增序号的 `unique_temp_path`（与编辑器 `document.rs`/`util.rs` 的唯一临时名约定对齐），失败路径收掉临时文件 | 连续 3 次 `cargo test -p editor --bin editor` 全绿；新增 `resource_db::tests::concurrent_saves_on_the_same_database_both_succeed`（双线程并发保存都要成功、且不留残留临时文件）；全量 `cargo test --workspace` 819 passed / 0 failed |

**已知口径与限制**（写进代码注释）：铜钱掉落当场入账（只有物品分"待入库/已入库"，因为惩罚项是
"当天没搬回庄的猎物"）；`bank()` 在跨天时统一入库，因此"今天打、明天入账"是刻意设计；战败明细在 HUD
只占一行（完整清单走日志与 `death.last_penalty()`，留给后续战败界面）；敌人不会自然刷新，重打依赖
死亡复活时的账本重置（与 `M18-01h` 一致）。

#### G4 数据校验扩展：已完成

| 项 | 落地 | 证据 |
|---|---|---|
| 登记漏项 | `validate_data.lua` 补登记 `data.gather`、`data.enemy_art`（两者早就写了 `validate_all`，从未登记 ⇒ 校验从没跑过）；登记表增加第三个字段（`require` 路径）并新增 `M.registered()` 供覆盖断言使用 | `jhyt_m23_data_validation` ②章枚举 `scripts/**/*.lua`，断言"带 `validate_all` 的模块都在登记表里"（18/18，且断言枚举数 ≥12 防止枚举本身失效） |
| 纯表注册表校验 | 新增 `scripts/data/registry_check.lua`：`data/items.lua`、`data/tools.lua`、`data/crops.lua` 返回的是**注册表本身**（`player/inventory.lua` 会 `pairs()` 逐个 `item.define`），塞 `validate_all`/`version` 会被注册成物品；规则集中到该模块并由 `validate_data` 登记 | ①章断言三张表里没有 `version`/`validate_all`/`get` 泄漏键；`data/items.lua` 条目数 7 |
| 跨文件不变量 | 种子/收获物必须存在且分类正确（seed/crop）；`crops[].land` 必须被 `data/farm.lua` 的区划覆盖（否则永远种不下）；`crops[].sell_price` 必须等于 `items[harvest].sell`；工具 id 不得与物品同名（消费方 `items[id] or tools[id]` 会互相遮蔽）；武器必须有 `damage`、工具必须有 `energy`；`use`/`use.buff` 闭集与 `use` 空效果拒绝；地块格必须在地图内且不重复、`stage` 与作物阶段自洽；敌人 `art` 引用必须存在，且每个美术条目都必须被 `data/enemies.lua` 引用（"生成了美术但没接线"从此是加载期错误） | ④章 15 个负向注入用例逐条报出含文件/对象/字段路径的诊断（如 `data/crops.lua:rice.sell_price: 31 != data/items.lua:rice.sell 30`），注入后全部还原并复查全量校验为空 |
| 资源引用 | 物品 `icon` 与敌人美术贴图引用的文件必须真实存在 | ⑥章 28 个引用逐个 `is_file()`（并用 `game --check` 实测：临时把 `rice.sell_price` 改 31 → 门禁以 `[registries]` 诊断失败，改回即通过） |

由此修出的两个真实缺陷（非"顺手改"，是被新校验挡住后必须处理）：

1. `herb_tonic`（M18-04 行气散）与 `timber`（M18-02 木料）的 `icon` 指向**不存在**的文件
   （`assets/art/items/herb_tonic.png`、`timber.png` 都没有）⇒ 背包/商店只会显示缺图兜底。
   已给 `tools/gen_item_icons.py` 补两个画法并重新生成（其余图标字节不变，只有 `_preview.png` 因新增条目变化）。
2. 演武场的训练木人、灰狼有 `assets/art/enemies/<id>/` 全套美术却没有 `art` 字段（一直是灰盒贴图、无序列帧）
   ⇒ 已接线（贴图路径与原灰盒串一致，只是多挂 `animation.attach`）。
3. 顺手收掉一处"直通校验"：`player/skills.lua` 的 `M.validate_all` 只是转调 `data.skills.validate_all`，
   会让"带 validate_all 的模块都要登记"出现例外并重复诊断 ⇒ 删除（其数据校验仍在 `data.skills`，已登记）。
#### G5 数值设计案 ↔ Lua 数据对齐：已完成

| 项 | 落地 | 证据 |
|---|---|---|
| 真实缺陷：`crop.yield` 缺失 | `systems/farm.lua:323` 收获读 `crop.yield or 1`，而 `data/crops.lua` **没有** `yield` 字段 ⇒ 每茬恒收 1（稻谷按设计案应为 2，少收一半且无任何报错）。现在 `data/crops.lua` 显式声明 `yield`（稻谷 2 / 灵芝 1，取设计案 §4.1 口径），`data/registry_check.lua` 把 `yield` 列为**必填正整数** | `jhyt_m24_numeric_alignment` ③④：真实耕作链路（翻土→播种→逐日浇水→睡觉→换锄头收获）断言 `实际收获 == crops[].yield`（灵芝 1、稻谷 2）；⑤：把 `rice.yield` 临时改成 3，同样流程立刻收 3（证明收获数量真的走数据，不是硬编码）；⑥：抹掉 `yield` 时 `registry_check.validate_all()` 报出 `data/crops.lua:rice.yield` |
| 文档与数据互相漂移 | `jhyt_m24_numeric_alignment` ①②**直接解析** `docs/33-江湖有田-数值设计案.md` 的 markdown 表格：§4.1 作物表（种子价/成长天数/基础产量/基础售价/推荐土地）逐格比对 `data/crops.lua` + `data/items.lua`；§5.2 商店价格表按商品名映射到物品比对 `buy`（未在数据里的行必须在**显式白名单**里，新增一行而不补数据会直接失败） | ①章打印 `稻谷(rice): 种子价=12 成长=4天 产量=2 售价=30 土地=field` / `灵芝(lingzhi): 种子价=25 成长=7天 产量=1 售价=80 土地=medicinal`；②章比对 4 行已落地商品价 |
| 文档侧修正（实现为准） | 设计案稻谷基础售价 18 → **30**（M17-04 商店经济 + M19-01 MVP 剧本按 30 调平，`items.rice.sell = 30`）；商店价格表木材 10 → **15**（M18-02 货摊定价）；作物表「推荐土地」列改为带反引号的土地 id（`field`/`medicinal`），使文档可被机器解析；`player/inventory.lua` 里"一茬灵芝收 2"的注释按真实值（1）改写，并把种子预算的推导写清 | 设计案 §4.1/§5.2 的修改说明段 + ①②两节断言 |
| 已知未落地项（显式登记，不再静默） | 商店价格表里的 草药/清水/石材 尚无对应物品（`SHOP_ROWS_WITHOUT_DATA` 白名单）；设计案 §4.2 配方表（米饭/小型内力丹/药粉）与 MVP 配方（`pill_refine`）口径不同 | 白名单在样例里显式存在；§4.2 未纳入本轮检查范围（列入 P1 内容对齐） |

#### G2 最小战斗闭环（受击反馈）：已完成

| 项 | 落地 | 证据 |
|---|---|---|
| 招式数据补全 | `data/skills.lua` 每招新增 **必填** `hitstun`（固定步）/`knockback`（世界单位）：普攻 10/8、千风掌 20/64；缺字段在 `validate_all` 报错（不允许隐式默认，同 `crop.yield` 的教训） | `jhyt_m25_combat_feedback` ⑦：抹掉 `strike.hitstun` → 校验报 `strike.hitstun` |
| 新增全局调参 | `scripts/data/combat.lua`：`player.iframes = 48`（0.8 秒无敌帧）+ 敌人默认抵抗 + `player_iframes()`/`enemy_knockback_resist()` 访问器；已登记进 `validate_data.lua`（校验器 18→19） | `jhyt_m23_data_validation` 的登记覆盖断言自动变成 19/19 |
| 敌人个体抵抗 | `data/enemies.lua` 支持 `knockback_resist`/`hitstun_resist`（0..1，校验期范围检查）；训练木人 = 1/1 | `jhyt_m25` ③：木人挨打 cell 不变、`last_knockback = 0`；⑤：临时把抵抗改成 0 → 千风掌推开 **64 单位 = 2 格**、硬直 19 步，恢复 1 后又打不动 |
| 玩家受击反应 | `systems/combat.lua`：无敌帧吸收整次命中（`dodge` 日志 + `stats.ignored`）、硬直（`player_hurt`）、击退（沿攻击者→目标方向位移，撞墙不穿墙 + `blocked` 标记）、`player_can_act()`/`player_hurt_frames()`/`player_iframes()`/`last_knockback()` 查询口、`hits_taken` 计数 | `jhyt_m25` ④：首次被咬读到硬直=10、无敌帧=48、击退=8；把狼的攻击间隔压到 12 步连咬 200 步，`ignored` 0→3，真实受击 **3 次 ≤ 上限 6**（无敌帧封顶掉血速度 = 防死亡螺旋的可验证形式） |
| 操作打断 | `ui/input.lua` 在硬直期间拦住移动/交互/出手并写 `last_action = "hurt"`（面板键不受影响，不会被锁死）；`combat.attack` 也返回 `false,"stunned"` 兜底 | `jhyt_m25` ④：硬直期间移动输入零位移、`player_attack` 返回 `stunned`、HUD 出「受击僵直」 |
| 敌人硬直与打断 | `systems/enemy.lua`：`hit()` 按抵抗应用硬直/击退（不穿墙）、**撤销当前寻路**；`tick_actor` 硬直期间只递减计数（不追击/不出手）；`query()` 增 `stun_frames/last_knockback/knockback_blocked/path_active` | `jhyt_m25` ⑥：实战打中灰狼当步 `path_active=false`、`state=stunned`、击退 8 = 数据，硬直后恢复追击 |
| 攻击失败反馈（`docs/37` 4-12 的落地部分） | `input.lua` 接收 `combat.player_attack` 的 `(ok, reason)` → `state.last_action = reason` + `audio.play_on("ui","ui_cancel")`；HUD 补 8 条中文文案（`hurt/stunned/dead/mode/player_dead/cooldown/inner_power/unknown_skill`） | `jhyt_m25` ④ 的 HUD 文案断言；`game --check` 校验 HUD 表无重复键/未知值 |
| 文档口径 | `docs/33` 新增 §6.4 受击反馈表（无敌帧/硬直/击退 + 规则：不穿墙、个体抵抗、硬直不进档） | `jhyt_m25` ①：解析 §6.4 六行与 `data/combat.lua`/`data/skills.lua` 逐格一致 |

**尚余（G6 范畴，下一轮）**：`last_action` 的其余缺失文案、`select_seed_`/`select_` 前缀误命中、
未知值不再裸露英文键（`docs/37` 4-13）；战败/复活时的文案与音量反馈统一。

#### G6 HUD 反馈闭环：已完成

| 项 | 落地 | 证据 |
|---|---|---|
| 缺失文案 | `ui/hud.lua` 的 `ACTION_TEXT` 补 15 条（`missing_map`/`map_goto_failed`/`gathered`/`gather_blocked`/`no_node`/`no_map`/`no_farm`/`unknown_recipe`/`no_station`/`station_level`/`busy`/`no_job`/`unknown_shop`/`closed`/`no_shop`），并把建造冲突键（`farm_plot`/`building`/`ground`/`solid`/`exit`/`road`/`unknown_building`）也纳入中文表 | `jhyt_m26_hud_feedback` ①③：扫描 27 个字面量 + 73 个 reason 词，逐个断言"反馈 ≠ 原值且含中文" |
| 前缀误命中与缺失前缀 | `ACTION_PREFIX` 新增 `select_seed_`（**排在** `select_` 前）、`equip_`、`tutorial_reward_`、`need_`、`gather_`、`exit_`；规则支持第三个字段（`"map"` → 地图短名 / `"plain"` → 原样），`exit_<map>` 取 `data/maps.lua` 的 `title` 短名 | ②：19 个动态前缀全部通过；④：`select_seed_lingzhi` →「已选种子：灵芝」、`equip_watering_can` →「已装备：水壶」、`exit_manor` →「前往：山庄」（此前显示 `manor`） |
| 未知值兜底 | `hud.feedback` 兜底从"原样返回英文键"改为「未知反馈（<原值>）」；同步更新 `jhyt_m19_ui` 里编码旧契约的断言 | ⑤：`feedback('totally_unknown_action')` →「未知反馈（totally_unknown_action）」；`feedback_line()` 同样带标签 |
| 顺带修正 | `hud.status_line()` 建造模式里把 `solid`/`farm_plot` 直接拼给玩家看 → 改走 `BUILD_REASON` 中文 | ⑥ 及 `game --check` |
| 防回归 | 新增 `jhyt_m26_hud_feedback`：**静态扫描 `scripts/**` 的 last_action 来源**（字面量 + 动态前缀 + reason 来源模块的返回键，带内部状态名白名单），任何新取值没有中文映射即失败 | ①②③ 共 119 个取值全部覆盖；⑥ 走真实玩法路径（翻土/播种/浇水/体力不足）核对 HUD 文案 |

### G1（7 日目标/里程碑与日结算联动）：已完成

脚本侧此前**完全没有**"日目标/里程碑"概念（`里程碑|七日|day_goal|milestone` 在 `scripts/**` 只命中 README
的历史说明），只有 `docs/46` 的 MVP 剧本与任务系统，因此 G1 是**新增**系统：目标表 + 每日结算 +
里程碑 + HUD 行 + 存档/新档隔离，全部由真实玩法路径驱动验收。

| 项 | 落地 | 证据 |
|---|---|---|
| 目标数据表 | 新增 `scripts/data/goals.lua`（`version=1`）：7 天 × 每天 3–4 条目标 + 当日奖励 + 七日里程碑；每条目标只有 `{ id, text, count?, match = { kind, value } }` 一个匹配口径，`M.validate_all()` 检查 7 天、日号连续、标题/文案非空、目标 id 全局唯一、`count` 为正整数、`match.kind` 合法、`match.value` 必填、**每天最后一条必须是 `day_start`**（日结算挂在跨天事件上）、奖励只能给 `item`/`money`/`inner_power` 之一且物品必须存在于 `data.items`/`data.tools`、里程碑奖励 > 0 | `jhyt_m27_goals` ①：表结构断言 + 4 个**负例**（删掉 `day_start` → 报 `day_start`；id 重复 → 报「重复」；`match.kind='teleport'` → 报 kind；奖励同时给 item+money → 报「只能给出」）；`jhyt_m23_data_validation` 的登记覆盖断言自动变成 20/20（`data.goals` 的 `validate_all` 也已登记进 `validate_data.lua`） |
| 目标运行时 | 新增 `scripts/systems/goals.lua`：进度只写 `state.vars` 标量（`goals_day` / `goals_p_<id>` / `goals_day_<n>` / `goals_day_<n>_reward` / `goals_milestone` / `goals_milestone_reward`），因此**天然进出存档**；`on_action` 每帧按「动作值变化」边沿计数，`on_enter` 记录 `enter_map`（当日粘性：去过就算，回山庄采集也不会把当天判成 missed），`on_day_start` 结算当天（全清→发奖，未清→`missed`），7 天全清→里程碑奖励，均带奖励锚点保证幂等 | `jhyt_m27_goals` ②–⑧：**全部用真实玩法**推进 7 天（E 键翻土/播种/浇水、E 键采集、T 键睡觉、与顾长风/沈青萝对话、`shop.buy`、`building.place`、`crafting.start/collect`、J 键打死木人桩），逐条断言目标变绿 + 当日 `completed` + 奖励入账金额/物品数量；重复 `on_day_start()` 不再发奖；第 7 天结束读到 `completed_days=7`、`milestone_complete=true`、`arc_done=true` |
| HUD 呈现 | `ui/hud.lua` 新增 `goals_line()`（「目标 第 N 天 · 今日 2/3 …」/ 达标后「… 七日之约已达成」），在 `draw` 里以 `ui.hud:goals` 画在左下（对话模式下让位给对话框） | ①②③⑧⑨ 断言 `goals_line()` 的天号与收尾文案；`game --check` 覆盖 HUD 表 |
| 联动接线 | `main.lua`：`new_game()` → `goals.reset()`；`enter_playable_map()` → `goals.on_enter(map)`；`on_day_start()` → `goals.on_day_start()`。`ui/input.lua`：每帧 `goals.on_action(state.last_action)`（在 `hud.draw` 前） | `jhyt_m27_goals` ③（进村那一刻目标即可读）、⑨（新档 `goals_day=1`、进度清零；读档后第 8 天/7 天全清/里程碑锚点全部回来） |
| 稳健性 | 漏做目标的那天记 `missed` 且**不发奖**，第二天目标照常给出（不会因为漏一天让整条目标线失效）；奖励发放走引擎全局 `inv`（与 `tutorial.lua` 同一条通路），失败时 `log.warn` 而不是静默 | ⑨：新档后什么都不做直接睡 → `goals_day_1="missed"`、`missed_days()=1`、种子数不变、第 2 天提示仍在 |

**已知口径与限制**（写进代码注释，不再当缺陷）：目标日的 `day()` 在 7 天走完后封顶为 8（"目标线结束"），
游戏日继续走；动作类目标按**动作值边沿**计数（同一次动作连续重复只算一次，需要中间插入别的动作或移动），
`enter_map` 是当日粘性事件，任务/击杀/物品/金钱类目标按实时状态判定。

### 阶段五 P1（资源接线/下架 + 天气与季节可见变化）：已完成

阶段五起点做了一次资产审计（`assets/**` 共 609 个文件）：**147 个没有任何引用点**
（46 个生成器开发产物 + 102 个内容缺口），其中最要命的是"系统实现了、画面上看不见"的那部分——
季节只影响作物与 HUD 文字（8 张四季地表从未被引用），天气只有晴（`farm` 雨天免浇水、NPC
`rain_place` 日程在真实玩法里走不到）。本轮接线 16 个、登记计划 102 个、下架 2 项。

| 项 | 落地 | 证据 |
|---|---|---|
| 引擎 API（表现可验收） | `tilemap.set_texture(id, texture)`：运行期换地表贴图（渲染器每帧重读 `Tilemap::texture`，组件变化→重建该层网格，格子/碰撞/meta 不动）；`tilemap.get` 补 `texture` 字段；`ui.image_get(id)`（`ui.panel_get` 的图片版，缺 id 返回 nil）。空/纯空格纹理、无 Tilemap 组件都直接 `api_error` | 引擎单测 `grid_tilemap_tag_and_interact_lua_api`（换图读回 + 两个负例）、`ui_panels_images_grids_and_drag_lua_api`（图片字段读回 + 缺失 id 为 nil + `ui.clear` 生效）；样例 ④⑥ 直接读 `ui.image_get` 断言语义 |
| 引擎打包规则 | `assetc::is_dev_artifact`：`_` 前缀 / 名字含 `preview` 的 PNG / `manifest.json` 不进发行包（生成器输出留在美术目录旁边，但无运行期读取点） | 单测 `dev_artifacts_are_not_packaged`（含 `title_bg_pixel.png` 不能误伤的边界）；样例 ⑦ 逐个核对 46 个开发产物 |
| 四季地表数据 | 新增 `scripts/data/season_art.lua`（`version=1`，`ENTRY_KEYS={entity,pattern,seasons}`）：山庄/云泽村 × 4 季，`pattern` 必须恰好一个 `%s` 且以 `assets/` 开头、季节必须在闭集内且不重复、未知键报错 | 样例 ①：8 张纹理存在性 + **6 个负例**（两个占位符 / 缺 `assets/` 前缀 / 空串 / 未知季节 / 重复季节 / 空季节表 / 未知键）；校验器 21→22（`jhyt_m23_data_validation` 的登记覆盖断言自动跟着变） |
| 季节运行时 | 新增 `scripts/world/season_visual.lua`：`apply/refresh/current_texture/swap_count/reset`，只在**进图**与**跨天**两个点换图（第 29/57/85 天跳变即可见），未登记地图保持基础图，失败只记日志不中断流程；换图次数写 `state.vars.season_swaps` | 样例 ②：春→夏（`clock.set(29,…)` + 真实 T 键跨天）换图、山道保持 `mountain_ground.png`、读档后季节/贴图一致且计数只增不减 |
| 按日天气 | 重写 `scripts/world/weather.lua`：`sunny|rainy` 由 `(seed, day, season)` 的确定性 LCG 签名决定（`roll_for` 纯函数、**不用 `math.random`**，否则读档后序列会跳）；季节概率 `RAIN_CHANCE`（春 .34/夏 .22/秋 .30/冬 .16）；`set()` 钉住当天、跨天重抽 | 样例 ③：同参数复现、不同种子序列不同、**400 天分布落在概率 ±0.10 内**、同一天重复 `sync` 不重抽、读档后第 32 天仍是同一结果 |
| 天气可见变化 | `ui/hud.lua`：雨天叠全屏冷色薄雾面板（`hud:rain`，1280×720，α=0.22）+ 右上角天气图标（晴/雨两张；取值闭集只有这两档，`ui_weather_cloud.png` 不接线、进 `docs/48` 计划行）；晴天 `ui.clear` 收干净 | 样例 ④：`ui.panel_get('hud:rain')` 的可见性/尺寸/α、`ui.image_get('hud:weather_icon').texture` 在雨/晴两态分别是 rain/sun，且晴天遮罩必须不存在 |
| 雨天真实影响 | 雨天按 E 走 `rain_watered`：免浇水、**不花体力也不加劳作分钟**；晴天没水壶→`need_watering_can` 且不误扣体力，装水壶后扣 4 点（`farm.ENERGY_COST.water`） | 样例 ⑤：同一块地雨天/晴天三段真实 E 键路径 + 体力差值断言 |
| HUD 资源条 | 状态面板正下方三条 9-slice 条（生命/体力/内力），填充宽度 = 真实比例（`combat.player_hp()/player_max_hp()`、`state.energy/max_energy()`、`state.inner_power/max_inner_power()`） | 样例 ⑥：满血 120px → 半血 60px，比例 0.500；`hud.bar_ratios()` 与 `ui.image_get('hud:bar_hp').width` 双向核对 |
| 资源决定表 | 新增 `examples/game_jhyt/docs/48-江湖有田-资源接线与下架清单.md`：口径（已引用/开发产物/内容缺口，**Lua 注释不算引用**）、102 个缺口的逐行归类（计划 102）、下架动作、开发产物为什么不进包 | 样例 ⑦：重扫 `assets/**` 得 **609 = 461 + 46 + 102**，解析文档表逐行核对数量，`接线` 行必须是事实（文件存在且真的被引用），未在表内的文件直接失败 |
| 下架动作 | 未被任何场景/脚本引用的第三方包 `art/tilesets/mushroom_village/**`（21 个文件）移到 `<repo>/third_party/MushroomVillage_16px/`；`art/title/title_bg.png`（生成器输入）改名 `_title_bg_source.png`；`docs/29` §4-8、`docs/40` §1、`tools/pixelate_title_bg.py` 同步 | 样例 ⑦：`assets/art/tilesets/mushroom_village` 必须不存在、`third_party/MushroomVillage_16px` ≥21 文件且含 `0_license_CC0.txt`、旧标题图必须不存在 |

**本轮修掉的两个真实缺陷**（都由新样例先复现，属于"零报错的坏行为"）：

1. `ui/hud.lua` 的两个绘制函数写成 `local function` 却定义在 `M.draw` 之后——Lua 局部变量只在其声明
   之后可见，`M.draw` 里的调用落到 nil，而 `pcall` 把错误吞掉：**画面上什么都不出现、日志里一行
   错误都没有**。改为显式前向声明（`local draw_bars` / `local draw_weather_visual`），样例直接断言元素存在。
2. 继续游戏路径是"先进图、后读档"，进图时 VM 时钟还是第 1 天 ⇒ 读档恢复到第 30 天后天气与季节地表
   停在第 1 天（此前季节只影响 HUD 文字所以看不出来）。现在读档成功后补 `weather.sync()` +
   `season_visual.refresh(map_id)`，样例 ②③ 都断言"读档后仍是第 30/32 天的季节与天气"。

**顺带修正的口径 bug**：审计最初把 `manifest.json` 里的出现当成"已引用"，于是
`ui_weather_rain.png` 这类只出现在清单里的文件被误判成已接线。`manifest.json` 是生成器清单，
引擎与 Lua 运行期都没有读取点，现已排除在引用语料外。

### 阶段五 P2（中文排版 / 缺字 / 溢出检查）：已完成

起点：运行时排版**从来没有被检查过**。HUD 用「CJK 记 2 单位、其余记 1 单位」的启发式截断，
`金钱 99999 文`、`WASD 移动 · E 交互…`（宽拉丁字母 + 中文混排）都会算错宽度；对话正文既没换行
也没裁剪（长台词直接画到面板外）；背包/任务/商店/暂停的行同样只按字符数截断。更隐蔽的一条：
正式包的字体是 `char_collect` 收的字符子集裁出来的，运行时文案里只要少一个字符，开发机上正常、
正式包里就是豆腐块。本轮把这三件事全部变成机器验收。

| 项 | 落地 | 证据 |
|---|---|---|
| 度量 API | `ui.measure_text(text, size)` → `{width,height,lines,glyphs,missing,missing_count}`：同字体、同 advance、同字间距（`spacing=1.0`），只度量不光栅化（headless 便宜）；`missing` 报字体没有字形的字符；`size<=0` 报 Lua 错误 | 引擎单测 `measure_matches_layout_size`（4 档字号 × 4 条文案，度量 == `TextLayout::build` 的 size/glyphs/lines）、`measure_reports_missing_glyphs`（emoji + 换行）、`probe_covers_chars_and_matches_atlas_advance`（`FontProbe` 与 `FontAtlas` 逐字 advance 一致）；绑定单测 `measure_text_and_text_get_lua_api`；样例 ① |
| 读回 API | `ui.text_get(id)` → `{text,x,y,scale,size,visible}`，`size` 按渲染口径折算（给了 `font_size` 就是它，否则 `scale × 12`） | 绑定单测 `measure_text_and_text_get_lua_api`（含未给 `font_size` 回落 24 的断言）；样例 ① |
| 引擎缺陷修复 | `TextLayout::build` 的高度此前是 `ascent + descent`（0.8em），正确值是 `ascent + |descent|`（1.2em 字身框）——否则行高与面板高度都偏小 | 由 `measure_matches_layout_size` 先复现（`"江湖有田"@13px`：measured 15.6 / mesh 10.4）再修，修复后 8/8 `core::text` 单测通过 |
| 排版层（纯数学） | 新增 `scripts/ui/text_fit.lua`：`width/char_width/fit/wrap`，按真实字宽裁剪与断行；CJK 逐字可断、连续 ASCII 视作一个词（不拆单词）、**标点禁则**用"与前一字符成组"实现（标点不落行首且不因此超宽）；`max_lines` 截断时最后一行以省略号收尾；容差 `EPSILON=0.05px` 解释 f32/f64 差异 | 样例 ①（两条路径 7 文案 × 4 字号逐条比对）、④（最坏行形态）、⑥（禁则/行数上限/省略号/空串/零预算边界） |
| 排版层（唯一切口） | 新增 `scripts/ui/ui_text.lua`：`begin(owner)/draw/draw_exact/draw_wrapped/hide/hide_all/reset/audit`。绘制同时登记 `(文字, 字号, 可用宽度)`，`audit()` 逐条量宽报出 缺字 / 字号 < 11px / 超宽；`owner` 隔离保证 HUD 每帧重绘不清空面板登记 | 样例 ③（逐面板遍历登记表量宽，`bad` 为空）、⑥（塞入超宽/emoji/10px 各报一条 ⇒ 审计有牙齿） |
| 接线 | HUD 状态行/键位/反馈/目标/引导、对话正文（两行换行）与选项、背包行（两列各自列宽）/容量/详情/提示、商店标题/金额/数量/行/提示、任务行/状态/发布者/目标行、暂停与设置页行/消息/提示、标题界面提示行——**44 处绘制调用**全部按各自真实可用宽度裁剪；删除启发式的 `hud.clip` | 样例 ⑤ 静态扫描 44 处字号的第 5 实参（解析失败即失败）+ 最小 13px；③ 量宽条数 hud=12 bp=21 qst=26 pse=22 shp=23 dlg=18 |
| 架构规则 | `scripts/**` 禁止直接调 `ui.text_at`（唯一出口 `ui/ui_text.lua`），否则"画了但没登记"的文字会从检查里漏掉 | 样例 ⑤ 的 `raw_text_at_sites()`：非 `ui_text.lua` 文件里出现 `ui.text_at(` 即失败（当前 0 处） |
| 缺字与首包覆盖 | 新增 `scripts/tools/text_audit.lua`：从**运行时**收集全部可能画出的文案（数据表 + 运行时行 + 全部对话节点/选项 + `last_action` 前缀族 + 最坏数字形态）= **371 条 / 601 字符**，逐条查缺字；并导出字符集供首包比对 | 样例 ②：语料规模下限（≥350 条 / ≥550 字符）、`missing == 0`、运行时字符 ⊆ `char_collect::collect_project_chars` 集合（2649 字符） |
| 字号下限 | 场景 `Text` 组件最小 14px（21 个）、Lua 绘制最小 13px（44 处）、运行时审计再兜一遍 `< 11px` | 样例 ⑤（Rust 侧解析 `scenes/*.scene` 的 `font_size`；Lua 侧解析绘制调用第 5 实参，含 `local LINE_FONT = 14` 这类常量解析；`skipped == 0` 是自检项） |
| 契约文档 | 新增 `examples/game_jhyt/docs/49-江湖有田-运行时中文排版与缺字契约.md`：度量口径、唯一出口规则、缺字与首包契约、**容器预算表**、验收段、改文案/改面板的流程 | 样例 ③④ 的数字与文档 §5/§6 一致；`docs/42` §1 补运行时字号下限指引 |
| 验收 | 新增 `engine/crates/game/examples/jhyt_m30_cjk_text.rs`，登记进 `scripts/verify.ps1`（MVP 样例 22→23），六节如左 | 最终行：`corpus=371 chars=601 collected=2649 missing=0 elements=12 hud=12/12/12 bp=21 qst=26 pse=22 shp=23 dlg=18 scene_text=21@14px lua_draw=44@13px` |

**过程中发现并修掉的三个真实问题**（都属于"看起来在工作、其实不对"）：

1. **引擎高度算错**：`TextLayout::build` 用 `ascent + descent`（`descent` 是负数 ⇒ 0.8em）算高度，
   正确是字身框 `ascent + |descent|`（1.2em）。由新单测先复现，属真缺陷（面板高度、行数判断都受影响）。
2. **`wrap` 的截断没有痕迹**：因为 `max_lines` 而丢掉后半句时，最后一行不加省略号 ⇒ 被截掉的半句话
   看起来就是完整文案。改为"因行数上限截断则最后一行必然以省略号收尾"，样例 ⑥ 直接断言。
3. **浮点口径**：引擎整串度量是 f32 累加、Lua 逐字符求和是 f64，几像素量级上差 ~1e-5
   （592px 处 f32 精度约 6e-5）。最初用 `1e-6` 判定"放得下"会误报，现统一为 `EPSILON = 0.05px`
   （1/20 像素：远高于噪声、远低于可见），并在 `text_fit`/`ui_text` 里写明原因。

### 阶段五 P3（PC 干净发布 / 用户数据目录 / 真机与浏览器验证）：已完成

起点：发布包把**玩家数据写进程序目录**。发布包形态（`<exe>/pack` + 同级 `release.toml`）下
工程根被解析成 `<安装目录>/pack`，于是 `save.sav` 落到 `pack/.cache/`、改键落到 `pack/settings.toml`：
装到 `Program Files` 时不可写（Lua 侧返回 `false` 但玩家只看到"存档失败"），可写时**重装即丢档**。
同时 `project.toml`/`release.toml` 的键读取在引擎里散了三份实现，发布名与工程名容易读错。

| 项 | 落地 | 证据 |
|---|---|---|
| 数据根规则 | 新增 `core::user_dir`：**发布包 → 用户数据目录**（Windows `%APPDATA%\<app>`，`RUSTLUA_USER_DIR` 可覆盖），**开发工程 → 工程内**（设置 `<project>/settings.toml`、存档 `<project>/.cache/save.sav`）。判定 = 目录名 `pack` + 同级 `release.toml`，或显式 `--pack`；应用名做安全化（非法字符、保留名、超长） | 引擎单测 6 条：`source_project_keeps_data_inside_the_project` / `release_pack_writes_into_the_user_data_directory` / `pack_detection_requires_release_marker_or_explicit_flag` / `first_run_seeds_settings_and_never_overwrites_user_edits` / `seeding_is_skipped_without_defaults_and_save_dir_is_created` / `user_dir_env_var_overrides_the_platform_directory`（用临时安装包 + `RUSTLUA_USER_DIR`，不污染真实 `%APPDATA%`） |
| 播种与升级 | 首次运行把包内 `pack/settings.toml` 播种到用户目录；**已存在则永不覆盖**——升级包不会重置玩家的改键、音量、手柄死区 | 单测 `first_run_seeds_settings_and_never_overwrites_user_edits`；样例 ④（改写用户目录里的 `stick_deadzone` 后再播种，值保持） |
| 设置根与工程根分离 | `LuaRuntime::set_settings_root(root)`：`input.settings_path()` / `audio.settings_path()` / `audio.load_settings` / `audio.save_settings` 全部经同一条解析（显式根优先，否则工程根）；**手柄死区（Rust 设备后端）与 Lua 设置面板读同一个根** | 绑定单测 `settings_root_redirects_player_settings_out_of_the_release_package`（发布形态 → 重定向后写用户目录、包内字节不变、相对路径也落在用户目录）；样例 ④（用户目录 `0.33/0.11` 生效，包内 `0.15/0.05` 不生效，缺失回落默认） |
| 配置读取唯一入口 | 新增 `core::project_config`：`read_key/read_key_in/parse_key/startup_scene/project_name/publish_name`，`project.toml` 与 `release.toml` 的键读取只此一份（`project_font::read_key` 也改为委托） | 单测 2 条；样例 ②（`publish_name = "jianghu_youtian"` 决定用户目录名，而非 `project.toml` 的 `name`） |
| 宿主接线 | `RuntimeScene::new_with_layout(project_root, packaged)` / `GameApp::new(..., packaged)`；`game --check` 多打一行 `player_data_roots packaged=<bool> save_dir=<> settings_dir=<>`，发布形态日志写明 `[save] release layout: user data directory in use` | 样例 ②（同一工程两种形态打印两套根）；`game --check` → `game_check_ok` |
| 发布卫生 | 发布目录不得有 `.cache`/`.data`/`*.sav`/`*.log`/`*.tmp`/`.staging-*` 残留，`pack/` 内不得有玩家数据 | 样例 ① 扫真实发布目录，据此清掉了 P2 遗留的一处 `.staging-*` |
| 验收 | 新增 `engine/crates/game/examples/jhyt_m31_pc_release.rs`，登记进 `scripts/verify.ps1`（MVP 样例 23→24），六节：① 发布形态与卫生 ② 判定与两套根 ③ 播种/升级 + 真实 `LuaRuntime` 落盘 ④ 设备后端与面板同源 ⑤ 真机手柄与浏览器证据 ⑥ 中文文本管线随包发布 | 最终行：`jhyt_m31_pc_release_ok`（`release_entries=13 stray=0`、`seeded_then_preserved`、`user_deadzone=0.33/0.11 pack_deadzone=0.15/0.05`、`gamepad: connected=false …`、`webgl: status=ok wasm_target=true`、`manifest_ok chars=947 cjk=812`） |
| 浏览器产物层验证 | 新增 `scripts/verify-webgl-bundle.ps1`：校验产物完整 + wasm 魔数，起静态服务器 + 无头 Edge 打开真实 `index.html`，读页面机器可读标记 `data-p2-boot`；`package.ps1` 生成的 `web-runtime.js`/`index.html` 加 fail-fast 诊断（启动失败时列出**缺失的引擎宿主 API 全局**） | `package.ps1 -Platform webgl` → `webgl_package_ok files=6`（wasm 39.6 KB、包 673 文件）；`verify-webgl-bundle.ps1` → **boot 失败并给出 17 个缺失全局**（见下） |
| 契约文档 | 新增 `docs/50-发布包与用户数据目录.md`（规则表、用户目录解析、播种/升级语义、"一根两读者"、验收命令、六节断言、FAQ）；`docs/32` §2/§3/§4 重写（wasm 已能在本机构建 + 产物层验收 + 新发现的平台缺口）；`docs/35` §6 的"发布包设置路径"限制标为已修复 | `docs/34`（质量门清单）P2 行与 §5 增补 `player_data_roots`、发布包无玩家数据、字符集检查；`docs/README.md` 收录 `docs/50` |
| 发布脚本与本轮实测修掉的空跑 | `run.txt` 增玩家数据目录/`pack/` 只读/`RUSTLUA_USER_DIR` 三条，`build-info.txt` 增 `player_data:` 行；`-SelfTest` 断言 `player_data_roots packaged=true` 且 `save_dir` 在临时 `RUSTLUA_USER_DIR` 下、隔离目录内无玩家数据。**过程中发现"发布包自检"一直是空跑**：发布版 `game.exe` 是 Windows GUI 子系统程序，`& game.exe --check` 既不等待也不捕获 stdout、`$LASTEXITCODE` 保留上一条命令的值 ⇒ 自检永远"通过"；改为 `Invoke-PackagedCheck`（`Start-Process -Wait -PassThru` + 重定向，含空格路径自行加引号）后立刻抓出第二个真问题——`-ArgumentList` 不代引号，带空格的隔离路径被拆成多参数（`unknown argument: release`） | 真实输出：打包期 `game_check_ok … packaged=true save_dir=%APPDATA%\jianghu_youtian`；隔离自测 `save_dir=<temp>\p2 user data test …\jianghu_youtian` + `Directory-isolation self test passed (spaces path + no-source check + user data dir OK)`，`PKG_EXIT=0`；`docs/50` §7 与 `docs/34` §5 记录了这条坑 |
| 日志也移出安装目录 | 实测在发布目录里直接跑一次 `game.exe --pack pack` 会在 `<release>/artifacts/logs/game.log` 留下运行期文件（装到 `Program Files` 时写不进去、日志静默丢失）。新增 `DataRoots::log_dir()`，发布形态把 `game.log`/`crash.log` 写到 `<用户数据目录>/logs/`；`run.txt` 第 4 条改为"日志/崩溃报告在玩家数据目录" | 单测 `release_pack_writes_into_the_user_data_directory` 增 `log_dir()==<user>/logs` 断言；`jhyt_m31_pc_release` ① 节的"发布目录不得有 `.log`/`.sav`/`.cache` 残留"随之成为回归门（旧行为会红灯） |

**本轮实测发现（未解决的平台缺口，已入 backlog 4-24）**：WebGL 发布包能在浏览器里**加载 wasm
并进入 Lua 执行**，但 `game_jhyt` 立刻在未实现的宿主 API 上停住——
`[string "scripts/main.lua"]:6545: attempt to index a nil value (global 'item')`，
缺失清单为 `app, asset, calendar, clock, container, data, day, event, grid, interact, inv, item,
light, map, shop, sprite, tag`（17 个）。`web-runtime.js` 目前是演示级子集（`log/time/input/scene/
entity/transform/transform3d/tilemap/health/physics/physics3d/animation/animation3d/audio/ui/save/
particles/camera/mesh`），与桌面引擎注册的 36 个顶层全局不等价。**PC 发行不受影响**；
要"浏览器内可玩"需把宿主 API 编进 wasm（wgpu WebGL2 后端 + Lua 桥）或把子集定为平台契约，
两条路见 `docs/32` §4。

**本机环境已可构建 WebGL**（此前记录的"离线装不上 wasm32"已不成立）：
`rustup target list --installed` 含 `wasm32-unknown-unknown`，`wasm-bindgen` 在
`C:\Users\Administrator\.cargo\bin\wasm-bindgen.exe`，测试浏览器 Edge 153.0.4234.32。

**真机手柄**：本机无 XInput 设备，探针输出 `gamepad: connected=false active=false slots=[]
axes=(0.000,0.000)`；逻辑路径由 `jhyt_m20_gamepad` 的 FakeBackend 覆盖（含死区、径向死区、
轴边沿、时间制连发），发布版读用户目录死区这一环由样例 ④ 用真实 `settings.toml` 覆盖。

### 阶段五 P4（文档路径/编号/状态、未跟踪文件与死代码收口）：已完成

做法：把"文档收口"从一次性人工校对变成**可重复的门**，机器能查的就不靠人眼。

| 项 | 落地 | 证据 |
|---|---|---|
| 文档一致性门 | 新增 `scripts/check-docs.ps1`（UTF-8 BOM）：① 每个目录内编号唯一 ② 每篇恰好一个 H1 且为首个非空行 ③ `docs/` 全部文档在 `docs/README.md` 被索引 ④ 无历史路径 ⑤ 相对链接可解析 ⑥ 无 U+FFFD（编码事故哨兵）；接进 `scripts/verify.ps1` 作为第一步（`-MvpOnly` 也跑，约 1 秒） | `check_docs_ok docs=38 game_docs=22 h1=62 links=204`，exit 0；首次运行即抓出 17 处真问题（见下各行） |
| 历史路径清零 | `C:\GitHub\p2`、`E:\Codex\p2` → `E:\Codex\RustLua`，覆盖 5 份活动文档 + 3 个 Rust 源文件注释/测试夹具（`editor/main.rs`、`editor/publish.rs`、`core/paths.rs`），共 **39 处**；`tasks/done/` 归档不动（历史记录），换算规则写在章程 §0 | 文档门 ④ 项通过（仅章程换算说明与 `docs/37` §5 失真引文在白名单内） |
| 编号收口 | 未被任何文档引用的 `game_jhyt/docs/42-江湖有田-演武场场景设计.md` → `50-...`（`git mv` 保留历史），消除同一目录内两份 42 号；**编号命名空间**裁定为"按目录独立"，约定写入 `docs/README.md`「文档编号约定」 | 文档门 ① 项通过；`docs/` 与 `game_jhyt/docs/` 各自编号唯一 |
| 索引补全 | `docs/README.md` 补 `docs/38`（统一 2D/3D 架构）、`docs/39`（能力等级复核）、`docs/40`（全面重构计划）三条索引（此前 38–40 无人引用、无处可查） | 文档门 ③ 项通过 |
| 相对链接修复 | `docs/24-3d-rendering.md`（7 条）、`docs/26-assetc-and-publish.md`（3 条）里的 `](engine/crates/...)` 是**相对仓库根**写的，从 `docs/` 出发解析不到 → 改为 `](../engine/crates/...)` | 文档门 ⑤ 项通过（204 条链接全部可解析） |
| 文档失真清零 | 章程 §0/§8/§13、`docs/README` 里程碑段、`tasks/README`、`engine/README:50`、`game_jhyt/docs/29` §1/§4/§9 全部按实况重写：`tasks/done/` 实为 **133 份、M0–M20 与 M22 输入层专题全部归档**（不是"M19/M20 收尾中"），`plan/` 只剩执行计划 | `docs/37` §5 失真清单 7 项中 6 项标 ✅（仅 `docs/25` 编辑器指南数值校准留给 5-7） |
| `docs/40` 编码事故 | 该文档部分段落是 GBK 字节（历史写入事故），其中 §8/§9 已不可恢复；按可读的 §0–§7 与 `docs/39` 结论**重建**（含 2D 专项工作表与技术选择原则），并在文件头写明重建说明 | 文档门 ⑥ 项通过（无 U+FFFD）；`docs/40` 同时补上 `docs/README` 索引 |
| 死代码收口 | ① `lua_ms` 死指标接线：`LuaRuntime::last_update_micros()`（Lua 体内测量）→ 内置系统 `lua_update` 累加 → `RuntimeScene::take_script_micros()`（取走即清零）→ `PerformanceStats::record_lua_millis()`，删掉只能测"两次调用之间墙钟"的 `begin_lua/end_lua` ② `plugin_status` 只写不显示 → 在"帮助 > 版本说明"列出 + `tracing` 记录 ③ `RecoveryManager::clear_all` 无入口 → 接到"⚠ 恢复"菜单的"全部丢弃（N 项）"，状态栏回报清理条数 ④ `AutoSaveController::on_edit` 已不存在 | 新增单测 `script_update_time_is_measured_and_taken_once`（断言 >0 且第二次取为 0）+ `performance` 两条改为累加语义；`cargo clippy -D warnings` 零告警；真实发布包日志从 `lua_ms=0.000` 变为真实值 |
| 未跟踪文件收口 | 39 个未跟踪项逐个归类：全部是应入库交付物；生成物全部被 `.gitignore` 覆盖；无"该忽略未忽略/该入库被忽略" | 结论写入本文件"环境限制与说明"；不代提交（未获提交指令） |
| 过程教训 | ① 就地编辑 `.ps1` 会丢 BOM → PowerShell 5.1 按 ANSI 读中文致 `Missing ')' in method call`（`check-docs.ps1` 当场复现）② 对非 UTF-8 文本用 read 工具触发了宿主侧有损重写（`docs/40` 事故） | 两条都写进"环境限制与说明"，并给 `.ps1` 定了"改完补 BOM + `ParseFile` 自检"的规矩 |

### 阶段五 P5（输入层收口 M22 批 3：物理键身份）：已完成

> 输入侧三项专题（W1/W5、W3/W4、W2）的实际落地统一记在
> [tasks/done/M22-输入层收口.md](M22-输入层收口.md)（§10 批 1、§11 批 2、§12 批 3；该专题已完结并归档到
> `tasks/done/`，W6 为已裁定不做的 backlog，见该文 §9），本执行计划只登记
> 它对本工程（`examples/game_jhyt` 与全部 `jhyt_*` 验收示例）的影响与证据。

| 项 | 落地 | 证据 |
|---|---|---|
| 键身份改为物理键 | `KeyCode` 重写为与 winit 0.30 逐项对齐的 **194 项物理键枚举**（文本 = 变体名小写：`key:keye`/`key:digit1`/`key:arrowup`/`key:shiftleft`）；宿主转换表收敛为 `engine::host::keymap`（winit，只读 `PhysicalKey::Code`）与 `editor::panels::keymap`（egui）两张，**都从引擎表派生并各有对齐测试**；原先 `game_app.rs`/`game_view.rs`/`input_bindings.rs` 三份手写表（含手写 60 项 `PREVIEW_KEYS`）全部删除 | `jhyt_m22_input` ① 章：键域 194 项、文本两两不同且逐项往返、AZERTY 下 `KeyW` 驱动 `move_up` 而 `KeyZ` 不误触发 |
| 设置文件读 v1 写 v2 | `[input].map` 版本头升为 `rustlua-actions-v2`；`rustlua-actions-v1` 仍可读入并按 US-QWERTY 换算成物理键（`key:f`→`key:keyf`、`key:1`→`key:digit1`、`key:up`→`key:arrowup`、`key:+`→`key:equal`）；带 Shift 字符的**有损**换算逐条进 `input.migration_notes()`/`input.diagnostics()`；未知版本头明确失败 + 回退默认，且**不吃掉其它段** | `jhyt_m22_input` ③ 章：写盘与内存逐字相等、`key:+` → `key:equal` 诊断可见、`[other]` 段原样保留 |
| 未知名报错 | Lua `input.is_down/just_pressed/just_released/pressed/released` 的键名解析失败改为**报错并回显原值**（旧实现静默落 `InputKey::Other(0)`）；键名同时接受 v2 物理键名与 v1 可读名，老脚本零改动可用 | `jhyt_m22_input` ② 章：`input.is_down('keye')` 为真、未知名报错且错误里带原值；全部 `jhyt_*` 示例（含 `jhyt_m20_gamepad` 14 章）原样通过 |
| 示例批量迁移 | 31 个文件从 `InputKey::` 迁到 `KeyCode`（`ARROW_*`→`Arrow*`、`Other(1\|2\|4)`→`F1/F2/F4`、`W/A/S/D`→`KeyW/KeyA/KeyS/KeyD`、`from_char(..)` 补 `.expect(..)`）；示例里「按字符喂键」的 helper 现返回错误而不是静默落其他键 | `cargo test --workspace --all-features` → **850 passed / 0 failed**（改造前 783）；`jhyt_m19_ui`/`jhyt_m26_hud_feedback` 的字符行号 helper 改为显式 `.expect("数字行号必须有对应主键区数字键")` |
| 端到端验收登记 | 新增 `engine/crates/game/examples/jhyt_m22_input.rs`（三章）并加进 `scripts/verify.ps1` 的 MVP 示例清单（该清单是唯一事实来源，缺项/多项都会 `throw`） | 收尾时补加 ④ 章，见下行 |
| 顺带修掉 | ① `editor/src/preview_session.rs` 的节流单测按**调用前**起算 `Instant`，而上一次 `update` 的离屏渲染可能吃掉大半个节流周期 → 随负载随机变红（改为按真实间隔判定）② `docs/35` 在 §7 后残留一整份**旧版本 §2~§6 重复段落**（含已被取代的 `InputKey`/`key:up` 说法）→ 删除并重写 ③ 工程 `settings.toml` 的导航手感仍是 M20-09 的帧数键（`nav_delay_frames`/`nav_repeat_frames`），批 1 起毫秒键优先、帧键只读 → 这份工程设置**自那时起就没生效过**；升 v2 时一并改为 `nav_delay_ms = 267`/`nav_repeat_ms = 133`（即原 16/8 帧 @60Hz 的手感） | 三条都留痕在 `tasks/done/M22-输入层收口.md` §12.7；`cargo fmt --check` / `clippy -D warnings` 退出 0 |
| 端到端验收登记 | 新增 `engine/crates/game/examples/jhyt_m22_input.rs`（四章）并加进 `scripts/verify.ps1` 的 MVP 示例清单（该清单是唯一事实来源，缺项/多项都会 `throw`） | `cargo run -p game --example jhyt_m22_input` → exit 0、`jhyt_m22_input_ok keys=194 actions=default settings=v1→v2` |
| 文档同步 | `docs/18`（新增「物理键身份」一节 + `migration_notes`）、`docs/22`（改键面板说明与 `settings.toml` 示例改 v2）、`docs/35`（§1/§2/§3/§4/§6 重写 + **新增 §8 W1 增量表**）、`docs/02`（ADR-0021 后续修订注记）、`docs/33`（双人「已裁定不做」）、`docs/37`（§10 两行标注已完成、P2 缺陷划掉）、`tasks/README`、`docs/README`、章程 §13、`CHANGELOG` | 文档一致性门 `check-docs.ps1` → `check_docs_ok docs=39 game_docs=22 h1=63 links=206`（exit 0）。原先拦门的未跟踪文档 `docs/wechat-minigame.md` 已按「补编号 + 索引」裁定改为 `docs/51-平台插件适配.md`（内容未改），见「环境限制与说明」 |
| 脚本侧键名显示防漂移 | `examples/game_jhyt/scripts/ui/hints.lua` 的键名表改按**物理键名**索引（原实现把 `key:` 后内容直接 `upper()`，W1 后顶部提示会变成 `KEYW/KEYA/KEYS/KEYD 移动`）；`jhyt_m22_input` **新增 ④ 章**把全部 194 个物理键喂给真实 Lua 运行时，与引擎 `Binding::display_label()` 逐项对照（51 项查表逐字相等、86 项按同一推导规则、57 项白名单内仍显示机器名且条目数必须与白名单**逐项相等**） | `jhyt_m20_gamepad` 第 ⑨ 章的操作提示文案断言（精确到「W/A/S/D 移动 · E 交互」）在 W1 后先红后绿；`jhyt_m22_input` ④ 章另加三条硬断言：v1 老写法映射同一套标签、手柄绑定不受影响、**默认表 36 个键盘绑定无一落到机器名分支** |

### 环境限制与说明

- 会话前段沙箱不允许写 `E:\Codex\RustLua\engine\target`（`cargo` 报 `os error 5`），全部构建/测试因此改用
  `CARGO_TARGET_DIR=E:\Codex\RustLua\.cargo-target`；会话后段该限制解除，最终质量门已按 CI 原始配置
  （不覆盖 target 目录）复跑。
- 本机只有 Windows PowerShell 5.1（无 `pwsh`），`scripts/verify.ps1` 用 `powershell -NoProfile -ExecutionPolicy Bypass -File` 调用；
  CI 用 `pwsh`，脚本本身两者兼容。
- 无 GPU 的 headless 环境下，渲染相关的真实帧验证依赖 `jhyt_m19_soak`/编辑器 Play 的 GPU 路径。
- **新增脚本必须带 UTF-8 BOM**：PowerShell 5.1 在 CP936 环境下按 ANSI 读 `.ps1`，无 BOM 的中文注释
  会被误解码并导致参数绑定异常（`verify-webgl-bundle.ps1` 首版即因此报
  `Cannot bind argument to parameter 'LiteralPath' because it is null`）；写入后请核对首三字节为 `239,187,191`。
- 实机手柄与移动端浏览器仍无法在本机验收（无设备）；WebGL 的"浏览器内可玩"按 backlog 4-24 处理。
- **编辑已存在的 `.ps1` 后会丢 BOM**：本轮工具链的文本编辑（就地替换）写回的是无 BOM UTF-8，
  于是 PowerShell 5.1 又把中文读成 ANSI → `check-docs.ps1` 当场报 `Missing ')' in method call`。
  规则：**每次改完 `.ps1` 都要补 BOM 并做语法自检**，两步都要留痕（首三字节 `239,187,191` +
  `[Parser]::ParseFile` 的 `parseErrors=0`），`scripts/verify.ps1` 与 `scripts/check-docs.ps1` 本轮均照此复核。
- **不要用 read 工具打开可能非 UTF-8 的文本文件**：`docs/40-引擎全面重构计划.md` 的部分段落是 GBK 字节
  （历史写入事故），直接读会失败；本轮对该文件的一次失败读取触发了宿主侧的**有损重写**，把原本可读的
  §0–§7 也一并变成替换字符（U+FFFD），§8/§9 彻底不可恢复。现在 `docs/40` 已按可读内容重建，并新增
  `scripts/check-docs.ps1` 的 U+FFFD 检查防止同类事故复发；遇到可疑文件先用
  `[System.IO.File]::ReadAllBytes` + 严格 UTF-8 解码探测，再决定怎么读。
- 未跟踪文件收口结论（P4）：`git status` 的 39 个未跟踪项全部是**应入库的交付物**（10 个 `jhyt_m22`–`m31`
  验收样例、`core/user_dir.rs`、`core/project_config.rs`、游戏侧新增 `data/*.lua`/`systems/*.lua`/`ui/text_fit.lua`、
  `docs/38`–`40`/`48`–`50`、计划文档、`scripts/verify-webgl-bundle.ps1`、`third_party/`），
  生成物（`engine/target/`、`artifacts/`、`**/.cache/`、`**/.data/`、`examples/**/builds/`）均已被 `.gitignore` 覆盖，
  无"该忽略却没忽略"或"该入库却在忽略"的项；本轮不代提交（未获提交指令）。
- **`docs/wechat-minigame.md` 已按「补编号 + 索引」裁定处理（2026-09-15，收尾时）**：该文件与本轮无关
  （未跟踪、428 字节，写于 2026-09-15 10:44），内容是「平台插件适配统一放在 `plugins/`」的说明
  （引用 `plugins/wechat-minigame/`、`plugins/webgl/` 与 `scripts/package.ps1 -Platform ...`）。它原先
  文件名**没有 `NN-` 编号前缀**，因此 `scripts/check-docs.ps1` 报 2 项（① 文件缺编号 ② 未被
  `docs/README.md` 索引），并让 `verify.ps1 -Mvp` 在第一步就 exit 1。现已**只改名与加索引、内容一字未改**：
  → `docs/51-平台插件适配.md`（`docs/` 当前最大号 `50` + 1，符合 `docs/README.md`「新增文档取当前最大号 +1」
  的约定），并在 `docs/README.md`「质量、平台与输入」段补上索引行。
  复核：`scripts/check-docs.ps1` → `check_docs_ok docs=39 game_docs=22 h1=63 links=206`（exit 0）；
  `scripts/verify.ps1 -Mvp` 因此得以跑过文档门并继续到示例清单。**未删除、未纳入版本控制**
  （本会话未获提交指令）。
- **`scripts/package.ps1` 尾部多出 5 个空行，曾让 `verify.ps1 -Mvp` 的「Git diff check」步 exit 2**
  （`git diff --check` 报 `scripts/package.ps1:875: new blank line at EOF.`）。该文件会话前就已带未提交改动
  （mtime 2026-09-15 10:48:44、`git diff --stat` 205 insertions / 36 deletions），**不是本轮引入的**。
  因为它卡住总门，本轮只做了**行尾规范化**：去掉文件末尾多余空行、以恰好一个 CRLF 收尾，并回补 UTF-8 BOM
  （首三字节 `239,187,191`）+ `[Parser]::ParseFile` 自检（`parseErrors=0`）+ U+FFFD 扫描（无）。
  **正文一个字符都没动**，`git diff --check` 现为 exit 0。若这份文件的未提交改动属于另一条工作线，请以
  `git diff scripts/package.ps1` 复核后再决定是否提交。
- **`jhyt_m29_season_weather` 需要的第三方素材包不在本机**：该示例第 ⑦ 章按 `docs/48 §3` 校验
  「第三方包已移出 `assets/`、但仍完整保存在工程根 `third_party/MushroomVillage_16px`（≥21 个文件 +
  `0_license_CC0.txt`）」，而本机**根本没有 `third_party/` 目录**（`Test-Path third_party` 为 false、
  `git ls-files third_party` 为空），于是报
  `third_party/MushroomVillage_16px 只有 0 个文件`。同时工作区里 23 个 `D`（含
  `assets/art/tilesets/mushroom_village/**` 与 `assets/art/title/title_bg.png`）说明**素材搬迁动作
  还没做完/第三方包被清掉了**。这不是 M22 输入层带来的回归（M22 不动素材），也不在本轮范围内，
  因此**不伪造通过、不删断言**，只如实记录。它会让 `verify.ps1 -Mvp` 在示例清单中段 exit 1；
  该示例之后的 `jhyt_m19_soak`、`jhyt_m30_cjk_text` 已单独补跑通过。

