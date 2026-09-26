# 任务：WP3 ECS 热存储（dense/sparse 原型与 A/B 证据）

- 任务 ID：WP3（工作包级）
- 状态：**完成**——原型与证据（2026-09-17）+ 按负责人裁定整体替换生产 `World` 存储（2026-09-18），
  结论与前后证据见下
- 关联里程碑：[docs/40](../docs/40-引擎全面重构计划.md) §3 WP3
- 前置依赖：WP0（已关闭）+ WP0 增量（`wp0-v4`/`wp0-v5`）

## 负责人裁定（本轮）

**整体替换生产 `World` 存储为 dense**（不采用"只热点组件 + 内部开关"的保守方案）。执行要求：
设计声明 → 旧新对拍（engine/editor 全部测试与 2D/3D golden 原样通过）→ 前后性能证据
（`wp0_ecs_storage` + `wp0_scene_frame` 同一把量尺）→ 3D 不回退 → 保留可回滚档。落实方式：
`World` 的组件存储换成 `DenseStorage`（dense 值数组 + sparse 槽位），**对外契约不变**——
遍历仍是排序实体序、`get`/`get_mut`/`insert`/`remove` 语义与原实现逐条对齐；回滚档是保留
原 `TypedStorage` 实现直到替换验证完成，替换后由提交边界（一次独立提交）承担回滚。

## 目标

把"是否把热组件从 `HashMap<EntityId, T>` + 排序 Vec 换成 dense/sparse 存储"从意见变成证据；在裁定前**不改动**现有 `World`（不破坏其"有序遍历"契约）。

## 已实施（2026-09-18，按裁定整体替换）

`core/world.rs` 的 `TypedStorage<T>` 内部换成 `DenseStorage<T>` + **惰性重建的有序 id 索引**：

- `dense`：dense 值数组 + `sparse` 槽位（`EntityId::index()` 直接定位），增删查 O(1)、generation 校验拒陈旧句柄；
- `order: RefCell<Vec<EntityId>>` + `order_dirty: Cell<bool>`：插入/删除只标脏，下一次有序访问整体重排一次（O(n log n)）；**稳态帧零分配、零哈希**。惰性而不是"插入时二分插入"，因为后者每次插入要 O(n) memmove（dense 数组本身仍保持 O(1) 增删）；
- `RefCell` 是必需的：`World::for_each(&self)` 只拿得到 `&self`，而有序索引需要在脏时重建；`for_each_mut` 走 `&mut` 路径用 `RefCell::get_mut` + `mem::take`（移动而非克隆）。

**对外契约逐条不变**：`for_each` / `for_each_mut` / `for_each2` 仍按 `EntityId` 升序；
`entities_with` / `entities_with2` 仍返回升序 `Vec`；`get` / `get_mut` / `insert` / `remove` /
`has` 语义与原实现一致（含"替换返回旧值""陈旧句柄报 `InvalidEntity`"）。

**旧新对拍**：engine lib **639 项**（含 `queries_component_combinations_in_sorted_entity_order`、
`stale_handles_are_rejected_after_slot_reuse`、新增的
`iteration_stays_sorted_after_interleaved_insert_and_remove`）、editor **109 + 165 项**、
2D overlay golden、3D semantic golden 与 `verify.ps1 -Benchmarks` 全绿。

**前后性能证据（同一把量尺：`wp0_scene_frame` 帧阶梯，Release 30 cycles，1280×720）**：

| 精灵 | 替换前 p50 | 替换后 p50（两次运行） | 变化 |
|---:|---:|---:|---|
| 1,000 | 208 µs | 195 / 188 µs | ≈ −9% |
| 10,000 | 1,612 µs | 933 / 953 µs | **−42%** |
| 50,000 | 9,688 µs | 4,706 / 4,285 µs | **−51%** |

跨越两次独立运行的差值与改动幅度一致（远大于该量尺 ±15–25% 的跨运行噪声）。
`wp0-v5`（`wp0_ecs_storage`）仍保留为"**替换前** hash 存储 vs 现生产 dense 存储"的对拍，
其契约门（dense 在全量可变遍历与隔一删除上必须占优）继续有效。

**回滚档**：本次替换落在独立改动区间内——`TypedStorage` 的三个字段（`dense` / `order` /
`order_dirty`）与其方法就是全部改动面，恢复旧的 HashMap + 排序表实现即可回到替换前行为
（对外 API 未变，因此不需要改任何调用点）。

## 已完成（原型与证据）

- `engine/crates/engine/src/core/dense_storage.rs`：`DenseStorage<T>`（dense 值数组 + `sparse: Vec<u32>` 槽位索引）。增删查 O(1)、遍历零分配、generation 校验拒绝陈旧句柄；`sorted()` 供确定性边界使用。6 个测试，含 swap_remove 错位回归。
- `engine/crates/engine/examples/wp0_ecs_storage.rs`（报告 `wp0-v5`）：同一套固定工作负载跑两侧——现状口径照抄 `core/world.rs` 的 `TypedStorage`（HashMap + 排序实体表，可变遍历每次克隆实体表）。
- `scripts/verify.ps1 -Benchmarks` 新增 `wp0-v5` 契约校验（含"dense 在全量可变遍历与隔一删除上必须占优"的不变量门）。

## 证据（Release，30 cycles，p50）

| 操作 | 规模 | 现状 hash | dense | 倍数 | 分配（hash → dense） |
|---|---:|---:|---:|---:|---|
| insert | 1k / 10k / 100k | 51µs / 565µs / 5.79ms | 4.8µs / 106µs / 1.03ms | 10.7× / 5.4× / 5.6× | 19→27 / 26→39 / 32→48 |
| get（乱序按 id） | 1k / 10k / 100k | 7.4µs / 79µs / 1.08ms | 1.8µs / 18.6µs / 302µs | 4.1× / 4.3× / 3.6× | 0 / 0 / 0 |
| **全量可变遍历** | 1k / 10k / 100k | 7.6µs / 81.9µs / 1.26ms | 0.2µs / 2.0µs / 20.3µs | **38× / 41× / 62×** | **1→0 / 1→0 / 1→0** |
| **隔一个删一个** | 1k / 10k / 100k | 84.7µs / 2.91ms / **282ms** | 6.0µs / 87.9µs / 1.16ms | 14× / 33× / **243×** | 19→27 / 26→39 / 32→48 |

两条关键结论：

1. **现状的可变遍历每次调用分配 1 次**（克隆整份实体表），dense 是 0 次。这是**每帧**都会发生的分配——与 `wp0-v4` 量到的"每帧分配"同源，属于真实收益。
2. **删除是数量级问题**：100k 实体隔一删除 282 ms（排序 Vec 的 O(n) memmove），dense 1.16 ms。任何高频 spawn/despawn 的玩法（子弹、实体化粒子）在规模上升时会直接撞上它。

## 实施设计（本轮已定，下一步执行）

`World` 的组件存储从 `TypedStorage<T> { HashMap<EntityId, T>, entities: Vec<EntityId> }` 换成
`DenseStorage<T>`（dense 值数组 + `sparse: Vec<u32>` 槽位索引，`core/dense_storage.rs` 已存在），
**对外 API 与语义逐条保持不变**：

| 契约 | 现实现 | dense 实现 |
|---|---|---|
| `insert`/`remove`/`get`/`get_mut`/`has` | HashMap 查表 | dense 槽位 O(1)（`sparse` 直接用 `EntityId.index()` 定位） |
| `entity_ids` / `entities_with` | 克隆排序 Vec（**每次 1 次分配**） | 排序 id 索引的**借用**（`as_slice`），调用方需要 Vec 时才克隆 |
| `for_each` / `for_each2` 的**排序实体序** | 遍历排序 Vec + 查表 | 遍历排序 id 索引 + dense 直取（**零分配、无哈希**） |
| `for_each_mut` | 克隆排序 Vec（**每帧 1 次分配**，`wp0-v5` 量到） | 同上，零分配 |
| `despawn` | 排序 Vec 的 O(n) memmove（100k 隔一删 282 ms） | `swap_remove` + 槽位修复（100k 1.16 ms） |

**关键取舍**：排序 id 索引用一个"脏标记 + 惰性重建"的 `Vec<EntityId>`——插入按 `binary_search`
位置插入（O(n) 但只在插入时发生），删除只标脏；下一次需要有序遍历时若标脏则整体重排一次
（`sort_unstable`，O(n log n)），因此**稳态帧零分配、零哈希、无 O(n) memmove**。这与 `wp0-v5`
的 dense 侧口径一致，可用同一把量尺对拍。

**分步执行（每步都必须全绿再进下一步）**：

1. `DenseStorage<T>` 补 `as_slice_of_ids()` / 惰性排序索引（若原型里没有），单测覆盖
   插入乱序、删除后重排、generation 校验拒陈旧句柄；
2. `TypedStorage<T>` 换成 dense 实现，跑 `cargo test -p engine --lib`（含
   `queries_component_combinations_in_sorted_entity_order`）与 editor 测试；
3. 跑 2D overlay golden / 3D semantic golden / `verify.ps1 -Benchmarks`（`wp0_ecs_storage`
   与 `wp0_scene_frame` 同尺对拍，记录前后数字）；
4. 更新 docs/53 与 CHANGELOG，删除旧实现（回滚档 = 本次替换的独立提交边界）。

**建议：现在替换"热四件套"的存储实现，但保留 `World` 的对外 API 与有序语义。**

理由：

- dense 在**全部四个操作**上占优（3.6×–243×），且消除每帧 1 次实体表克隆；这是 WP3 里唯一有实测收益的部分。
- **但收益的绝对量级取决于规模**：`game_jhyt` 单场景约 30 个实体，1k 实体下可变遍历 7.6µs → 0.2µs，占 60fps 预算不足 0.05%——**当前内容感知不到**。收益要到 10k–100k 实体才决定性。
- 因此可分两步：先只换 **Transform / Sprite / Velocity / PhysicsProxy**（docs/40 §3 WP3 已限定范围），并保留一条 `for_each*` 的**有序**入口（内部用 `sorted()`），使现有"有序遍历"契约与既有测试不被破坏；大范围 archetype 改造仍不做。

**若裁定暂不替换**：`DenseStorage` 与 `wp0-v5` 基准保留为待用件与量尺，`World` 不动；触发条件为出现 10k+ 实体场景或高频 spawn/despawn 的玩法。

### 决策材料补充（2026-09-17，来自 WP6 的收集阶段剖析）

原先的判断是"收益要到 10k–100k 实体才决定性，当前内容感知不到"。**现在这条更硬了**：
WP6 在 5 万精灵档把帧成本逐项拆开后量到——顶点生成剔除干净之后，剩下的时间**几乎全在
逐实体组件查询**上（`SpriteBatch` 收集循环）。把 `for_each::<Sprite>` + `get::<Transform>(id)`
换成 `for_each2::<Transform, Sprite>`（少一次 `get`，即少**两层哈希查询**：`TypeId` 表 + 实体表），
同一轮内帧时间从 **21,407 → 9,688 µs（−55%）**。

也就是说："每个精灵一次 `get` = 两层哈希查询"这件事在 5 万规模上就是**每帧几毫秒**的实测热点，
而不是理论推断。WP3 若采纳 dense 存储（乱序 `get` 实测快 3.6–4.3×、且遍历零分配），受益的正是
这条路径。**这使 WP3 从"等规模到了再说"变成"已有一个实测热点可直接受益"**——请连同 docs/53 §3.10
一起裁定。

## 未完成项 / 已知限制

- **尚未替换 `World`**：`DenseStorage` 目前无生产调用点。任何"ECS 已 dense 化"的说法都不成立。
- 现有 `World::for_each`/`for_each2` 的**有序遍历是文档化并被测试保护的契约**（`core/world.rs` 注释 + `queries_component_combinations_in_sorted_entity_order`）；替换时必须以有序入口保留它，或作为显式语义变更连同测试一起改——不得顺手改掉。
- 基准两侧共用同一套实体 id（0..N、generation 1），未覆盖槽位复用后的 generation 抖动；`dense_storage` 单测覆盖了陈旧句柄与槽位复用，但基准没量这部分成本。
- 100k 档 hash 侧 `remove_half` 单次 282 ms，基准本身耗时集中在这一点；若后续加 1M 档需先降低 cycles。
