# Lua 脚本性能分析与优化

> **结论先行**：RustLua 当前跑的是 **PUC-Rio Lua 5.4.8**（vendored，无 JIT），
> 不是 LuaJIT。纯 Lua 的加法循环约 **7.2 ns/次**，Lua 调 Rust 空函数约 **33.7 ns/次**，
> 跨语言绑定本身只占约 **1–2 ns**；真正贵的是**返回 table 的 API**：
> `transform.get` 优化前 p50 **1024.7 ns/次**，优化后 **705.7 ns/次**（−31.1%），
> 其中约 **651 ns 是 6 字段表的构造**，Rust 侧的查找与借用只占约 **55 ns**（噪声量级）。
> 因此最有效的优化不是换 LuaJIT，而是**让每帧每个实体的热路径少返回 table**：
> 已新增 `transform.position` / `transform.world_position`（**76.7 / 87.2 ns**），
> 比 `get` 便宜约 **9 倍**；批量写 API 实测**每实体反而更慢**，已放弃（§4.2）。
> 按 1000 次/帧估算，`transform.get` 约 **706 µs/帧**，已经接近 `jhyt_manor` 整段 Script 的 1224 µs。

本文只测 **RustLua 自身**。没有把 tolua#、xLua、zLua 放进同一张表：
它们绑定的宿主语言、对象模型和调用约定不同，脱离具体场景拼数字没有可比性；
需要横向对比时，应各跑同一份游戏脚本与同一组 API 调用。

## 一、测量口径

### 1.1 环境

| 项 | 值 |
|---|---|
| Lua | PUC-Rio Lua 5.4.8，vendored，**无 JIT** |
| 绑定 | `mlua 0.11`，features `["lua54", "vendored"]` |
| 构建 | `--release`（`opt-level` 按 workspace release profile） |
| 系统 | Windows x86_64 |
| 引擎 | `0.1.1` |
| 夹具 | 2 个实体：`bench`（2D `Transform`）+ `bench3d`（3D `Transform3D`），名字均已登记 |

### 1.2 基准程序

基准在 [`engine/crates/engine/examples/lua_perf.rs`](../engine/crates/engine/examples/lua_perf.rs)，
与其它 WP0 基准一样用 `--release` 运行，输出 JSON 到 [`metrics/baselines/lua-perf.json`](../metrics/baselines/lua-perf.json)。

它测三类路径：

1. **纯 Lua**：`for` 循环里的加法、函数调用、表读写/建表、方法调用、字符串拼接；
2. **Lua → Rust**：空函数、两个数相加、读时间、实体存活/按名查找、`transform.*`、
   `event.emit`，以及一个把 `get` + `set` 串起来的 `gameplay_mix`；
3. **Rust → Lua**：直接调一个 Lua 函数（`bench_tick`），以及引擎每帧走一遍
   `LuaRuntime::call_update()`（含全局查找、预算/守卫、错误快照桥与返回值转换）。

每个场景的做法是：

- 先编译一个 `N` 次的 chunk：`local N = ...; <setup>; local acc = 0; for i = 1, N do <body> end; return acc`；
- 用一个 2000 次的探针标定，让单个样本约 **15 ms**；
- 预热 2 轮，`gc_collect`，然后**纯 Lua / Lua→Rust 场景停 GC**；
  Rust→Lua 场景不停 GC（要把 `call_update` 的每帧分配算进去），采 **31 个样本**；
- 每个样本只调一次 chunk，`per_op_ns` = 样本耗时 / 该 chunk 的 `N`；
- 同时记录 p50/p95/p99、Lua `used_memory` 的净增量、Rust 全局分配器的分配次数/字节。

这样计时里不包含“Rust 循环调用 chunk”的开销；`noop` 场景可以作为跨语言调用的基线。

### 1.3 数字的含义与边界

- `lua_bytes_per_op` 是**样本前后 Lua 内存的净增量 / 操作数**，不是“每次分配了多少”。
  纯 Lua / Lua→Rust 的采样期间停 GC，短期垃圾会留在堆里；它适合看“每次调用是否留下长期对象”，
  不适合当分配速率。
- `rust_alloc_*` 只统计 **Rust 全局分配器**；Lua 自己的 C 堆分配不计入。
- 采样期间停 GC 是为了把 GC 停顿从单次操作里拿掉，**不代表真实帧里没有 GC 成本**；
  真实帧的 GC 压力要用帧级 `FrameProfile` 和长跑分配量尺看。
- 所有数字都是本机、单进程、release 的 p50；跨机器、跨电源模式、跨编译选项不可直接比。
  同一台机器上重复跑，纯 Lua 基线也可能有 **±5–10%** 的漂移。

## 二、纯 Lua 结果

p50，单位 ns/次：

| 场景 | p50 | p95 | Lua 净增量 B/次 | Rust 分配次/次 |
|---|---:|---:|---:|---:|
| `loop_add`（`acc = acc + i`） | **7.16** | 9.88 | 0 | 0.000 |
| `function_call`（调空 Lua 函数） | **31.38** | 35.35 | 0 | 0.000 |
| `table_get`（读固定键） | **18.11** | 35.53 | 0 | 0.000 |
| `table_set`（写固定键） | **18.81** | 22.54 | 0 | 0.000 |
| `table_new`（建 2 字段表） | **155.50** | 228.06 | 104 | 2.000 |
| `method_call`（`obj:m()`） | **51.10** | 77.17 | 0 | 0.001 |
| `string_concat`（`acc = acc .. i`） | **118.84** | 181.09 | 2.53 | 0.065 |

读法：

- 解释器主循环的加法约 **7 ns**；这是 PUC Lua 5.4 的正常量级，不能拿 LuaJIT 的
  1–2 ns 来要求它。
- 一次普通 Lua 函数调用约 **31 ns**；带 `self` 的方法调用约 **51 ns**。
- 表读/写约 **18–19 ns**；建一张 2 字段表约 **156 ns**，并留下约 **104 B** 的 Lua 对象。
- 字符串拼接每次约 **119 ns**；它的净增量看起来小，是因为只保留最后结果，
  不能据此认为“拼接不产生垃圾”。

## 三、Lua → Rust 结果

p50，单位 ns/次：

| 场景 | p50 | p95 | Lua 净增量 B/次 | Rust 分配次/次 |
|---|---:|---:|---:|---:|
| `noop`（空 Rust 函数） | **33.69** | 35.66 | 0 | 0.000 |
| `add_numbers`（两个 f64 相加） | **56.40** | 80.27 | 0 | 0.000 |
| `time_delta`（读时间） | **37.91** | 56.06 | 0 | 0.000 |
| `entity_is_alive` | **46.41** | 51.54 | 0 | 0.000 |
| `entity_find`（按名字查） | **103.96** | 109.95 | 0 | **1.000** |
| `entity_find_global`（全局表查找 + 查） | **113.18** | 144.39 | 0 | **1.000** |
| `transform_set` | **92.03** | 113.39 | 0 | 0.000 |
| `transform_move` | **80.17** | 114.22 | 0 | 0.000 |
| `transform_space` | **72.36** | 109.45 | 0 | 0.000 |
| `transform_get` | **705.73** | 814.81 | **248** | **3.000** |
| `transform_get_global` | **719.93** | 940.76 | **248** | **3.000** |
| `transform_position`（只读局部 XY） | **76.72** | 78.18 | 0 | 0.000 |
| `transform_world_position`（只读世界 XY） | **87.15** | 119.43 | 0 | 0.000 |
| `transform3d_position`（只读局部 XYZ） | **87.11** | 131.16 | 0 | 0.000 |
| `transform3d_world_position`（只读世界 XYZ） | **110.34** | 151.10 | 0 | 0.000 |
| `gameplay_mix`（`get` + `set`） | **823.61** | 1074.03 | **248** | **3.001** |
| `event_emit`（发一个事件） | **215.40** | 330.16 | 0 | **1.001** |
| `bench_tick`（Rust 直接调 Lua 函数） | **47.65** | 50.82 | 0 | 0.000 |
| `update_callback`（`call_update` 一整圈） | **813.27** | 1164.93 | 0 | **8.000** |

### 3.1 绑定开销不是瓶颈

纯 Lua 空函数调用 **31.38 ns**，Lua 调 Rust 空函数 **33.69 ns**，差 **2.3 ns**，
仍在本次运行的噪声范围内。也就是说，`mlua` 的参数/返回值转换和调用栈切换，
相对一次普通 Lua 调用几乎可以忽略；真正决定成本的是 **Rust 函数里做了什么**。

- `add_numbers` 比 `noop` 多 **22.7 ns**：两个 `f64` 的 Lua↔Rust 转换、加法、返回；
- `entity_is_alive` 比 `noop` 多 **12.7 ns**：`i64` 句柄转换、一次世界借用、存活判断；
- `transform_set` 比 `noop` 多 **58.3 ns**：句柄转换、一次 `world_mut` 借用、组件查找、写两个 `f32`；
- `transform_move` 比 `noop` 多 **46.5 ns**；`transform_space` 多 **38.7 ns**。

这些都在“每帧几百到几千次”的规模下完全可接受。例如 1000 次 `transform_set`/帧约
**92 µs**，1000 次 `entity_find`/帧约 **104 µs**；标量位置读取 1000 次只有 **77–110 µs**。

### 3.2 `transform.get` 的成本几乎全在 table

`transform.get` 比 `noop` 多 **672.0 ns**，其中：

- **Rust 侧**：一次世界借用、一次组件查找、6 次 `table.set`；
- **Lua 侧**：新建一张 6 字段表（约 **248 B**），6 个字符串键 + 6 个数值。

下一节用同一次运行里的合成表基线把这两部分拆开。

### 3.3 `call_update` 的每帧成本是固定的，不需要先优化

`update_callback` 每次 **813 ns**，其中 Lua `update` 函数体是空的（返回 `""`）。
这 828 ns 包括：

- `call_update` 的全局 `update` 查找；
- 指令预算的 `arm` 与守卫检查；
- `crash::call_guarded` 的 `xpcall`/错误快照桥；
- 返回值的 `String::from_lua`；
- 8 次 Rust 分配（包括路径、错误上下文与返回值转换等）。

按 60 FPS 算，**0.813 µs/帧 = 48.8 µs/秒**，约占 1 秒的 **0.005%**。
除非未来把 `update` 提高到每秒几万次，否则不值得为它做路径分配之类的微优化。

## 四、瓶颈拆解：table 的形状

同一次运行里加了三张合成表作为对照（都是返回后立即读一个字段）：

| 场景 | 字段数 | 建表方式 | p50 | Lua B/次 | Rust 分配次/次 |
|---|---:|---|---:|---:|---:|
| `return_table` | 2 | `create_table_with_capacity(0, 2)` | **352.54** | 104 | 3.000 |
| `return_table6_capacity` | 6 | `create_table_with_capacity(0, 6)` | **651.04** | 248 | 3.000 |
| `return_table6_default` | 6 | `create_table()`（默认 0 槽） | **969.54** | 248 | 6.000 |
| `transform_get` | 6 | 容量 6（优化后） | **705.73** | 248 | 3.000 |

结论很直接：

- 6 字段表从默认建表改成**按字段数预分配哈希槽**，p50 从 **969.54 → 651.04 ns**
  （−32.9%，少 **318.5 ns**），Rust 分配从 6 次降到 3 次；
- `transform_get` 优化后 **705.73 ns**，与“6 字段 + 容量 6”的合成表 **651.04 ns**
  只差约 **54.7 ns**；也就是说，**Rust 侧的查找/借用已经不是可优化的大头**；
- 2 字段表 352.54 ns，6 字段表 651.04 ns，多 4 个字段约多 **298.5 ns**，
  折合每字段约 **74.6 ns**；这就是“少返回字段”能省下的量级。
- 直接返回标量比 2 字段表还便宜一个量级：`transform.position` **76.72 ns**、
  `transform3d.position` **87.11 ns**，且 Lua 净增量与 Rust 分配都是 **0**。

### 4.1 一个被证伪的优化：复用 table

试过给 `transform.get(id, scratch)` 增加可选的输出表，内部 `table.clear()` 后复用。
当次运行的 p50 是 **742.86 ns**，同一次运行的新建表是 **721.72 ns**：
复用反而略慢约 **2.9%**，基本持平——`clear` + 重填 6 个字段，并不比
“分配 + 填 6 个字段”便宜。因此**没有保留这个 API**。这条负结果值得记下来：
在这个形状下，优化方向不是“省掉那次分配”，而是**减少字段数或减少调用次数**。

### 4.2 一个被证伪的优化：批量写位置

在标量 API 之后，又试了 `transform.set_positions(ids, xs, ys)` /
`transform3d.set_positions(ids, xs, ys, zs)`：一次跨语言调用写 N 个实体，
不建每实体 table。直觉上应该把 N 次调用压成 1 次，但实测（同一夹具，x16；
确认负结果后这组场景已从基准里移除，所以不在 `metrics/baselines/lua-perf.json`，
下表是移除前的当次读数）：

| 路径 | 每次调用 p50 | 每实体 p50 |
|---|---:|---:|
| `transform.position`（标量） | **76.72 ns** | **76.72 ns** |
| `transform3d.position`（标量） | **87.11 ns** | **87.11 ns** |
| 批量读 16 个 2D 位置 | 约 **1881 ns** | 约 **118 ns** |
| 批量读 16 个 3D 位置 | 约 **2390 ns** | 约 **149 ns** |
| 批量写 16 个 2D 位置 | 约 **2368 ns** | 约 **148 ns** |
| 批量写 16 个 3D 位置 | 约 **2467 ns** | 约 **154 ns** |

原因不是“批量”本身，而是为了批量必须**遍历 Lua table 取元素**（`raw_get` + 类型转换），
每次取值的成本与一次标量调用的参数转换同量级；再加上逐元素 `world.get_mut`，
最终每实体比标量调用更贵。标量 API 的参数直接走 mlua 的栈转换，没有 table 索引。
因此**批量 API 没有保留**：它会让“每帧 N 个实体”的代码看起来更短，但实测更慢，
不符合“只保留有证据的优化”这条底线。

批量遍历仍有价值的方向是**引擎内部一次遍历、一次返回紧凑数组**，而不是
“Lua 建三个数组再交给 Rust 逐个解包”。那需要新的数据形状与错误语义，
在真实工程的调用计数证明批量写是瓶颈之前不做。

## 五、已经落地的优化

### 5.1 `transform.*` 热路径去掉重复存活检查与重复借用

文件：[`engine/crates/rustlua-ecs/src/transform_lua.rs`](../engine/crates/rustlua-ecs/src/transform_lua.rs)。

优化前，`transform.get` 的典型路径是：

1. `require_alive(state, id)`：借一次世界，做 `is_alive`，失败时格式化错误；
2. `state.world()?.get::<Transform>(id)`：再借一次世界，`World::get` 内部又做一次 `ensure_alive`；
3. 如果不是 2D，再 `state.world()` 借一次，查 `Transform3D`。

`transform.set` / `move` 同样先 `require_alive`，再 `world_mut()`；`transform3d.*` 也复制了这套。
每次多一次 `RefCell` 借用和一次 `is_alive`，在 1000 次/帧的规模下就是白付的成本。

现在：

- 每个入口只借一次 `s.world()` / `s.world_mut()`；
- 直接用 `World::get` / `World::get_mut` 自带的 `ensure_alive`；
- 把 `WorldError::InvalidEntity` 映射回原来的 **`invalid entity id: {id}`** 文案，
  脚本侧的错误契约不变；
- 返回表改成 `create_table_with_capacity(0, 6)`（2D）或 `(0, 10)`（3D）。

覆盖 `transform.get/world/space/set/move` 与 `transform3d.get/set/move/world`。
`transform.attach` / `transform3d.attach` 仍保留显式 `require_alive`，因为它们还要先确认
“没有 transform”，错误语义不同。

### 5.2 基准需要显式的预算重置

`progress` 的指令预算是**按入口周期**计的：正常游戏里 `call_update` 每个回调开头
`arm()` 一次；一个基准进程在**一次宿主入口**里跑几十亿条指令，会被判成死循环
（第一次跑就在 `table_new` 触发 `脚本超过指令预算 1000000000`）。

因此给 `LuaRuntime` 加了：

```rust
pub fn reset_instruction_budget(&self)
```

它只清计数，**不关预算**；正常帧循环由 `call_update` 自己清，不需要宿主调用。
基准在每个场景开始前调用一次。这不是给 Lua 脚本的 API，脚本无法调用它绕过护栏。

### 5.3 前后对比

同一台机器，优化前 15 个样本、优化后 31 个样本，p50：

| 场景 | 优化前 | 优化后 | 变化 |
|---|---:|---:|---:|
| `transform_get` | 1024.70 | **705.73** | **−31.1%** |
| `transform_get_global` | 1050.42 | **719.93** | **−31.5%** |
| `gameplay_mix` | 1179.66 | **823.61** | **−30.2%** |
| `transform_set` | 96.51 | 92.03 | −4.6% |
| `transform_move` | 87.43 | 80.17 | −8.3% |
| `transform_space` | 78.81 | 72.36 | −8.2% |

注意：

- 这两次运行之间，纯 Lua 基线整体有 **+4%～+10%** 的漂移（CPU 频率/后台负载），
  所以跨运行的百分比只能看量级；**同一次运行**里的
  `return_table6_default` 969.54 → `return_table6_capacity` 651.04（−32.9%）
  才是干净的因果对照。
- `transform_get` 优化前 1024.70 与“6 字段默认表”1015.27 几乎重合，
  优化后 705.73 与“6 字段容量表”651.04 在同一量级；这正好说明改动命中了瓶颈。
- `transform_set/move` 的变化在噪声内；它们的收益主要是少一次借用，量级只有几 ns。
- `transform_get` 的 Lua 净增量仍是 **248 B/次**：表还在建，只是建得更快；
  要消掉这 248 B，必须改 API 形状，而不是继续抠建表。

### 5.4 新增标量位置 API

文件：[`engine/crates/rustlua-ecs/src/transform_lua.rs`](../engine/crates/rustlua-ecs/src/transform_lua.rs)。

新增四个函数，只返回数字、不建 table：

| API | 返回 | p50 | Lua B/次 | Rust 分配/次 |
|---|---|---:|---:|---:|
| `transform.position(id)` | `x, y`（局部；2D/3D 都可用） | **76.72** | 0 | 0.000 |
| `transform.world_position(id)` | `x, y`（世界） | **87.15** | 0 | 0.000 |
| `transform3d.position(id)` | `x, y, z`（局部，只认 `Transform3D`） | **87.11** | 0 | 0.000 |
| `transform3d.world_position(id)` | `x, y, z`（世界） | **110.34** | 0 | 0.000 |

`transform.position` 比 `transform.get` 便宜 **9.2 倍**，且不产生 Lua 垃圾。
3D 世界位置比局部位置多约 **23 ns**：它要先看传播好的 `GlobalTransform`；
无父节点时走局部快路径，只有父链确实未传播时才现场组合（见 `world_xyz`）。
这正是“热循环只读位置”的推荐写法；`get` 留给需要旋转/缩放/space 的调用点。

错误契约与表版本一致：死句柄仍是 `invalid entity id: …`，2D/3D 都没有时报
`entity has no transform (2d or 3d)`，`transform3d.position` 只认 3D 组件。

### 5.5 事件分发去掉每次 emit 的整表克隆

文件：[`engine/crates/rustlua-scripting/src/script/api/globals.rs`](../engine/crates/rustlua-scripting/src/script/api/globals.rs)。

`event.emit` 原先把订阅者 `Vec` 整个 `clone()` 后再遍历，保证回调里 `on/off`
不会让迭代器失效；代价是每次 emit 一次 Rust 分配、一次 `Vec` 拷贝。
现在订阅表是 `Rc<RefCell<BTreeMap<String, Rc<Vec<Function>>>>>`：

- 注册用 `Rc::make_mut(...).push(...)`；
- emit 只克隆 `Rc`（**不拷贝 Vec**），拿一个快照再遍历；
- 重入的 `on/off` 仍然修改“下一份”订阅表，语义不变；已有一次性监听器测试覆盖
  “回调里 off 自己，下一次 emit 不再触发”。

结果：`event_emit` 从 **268.52 → 215.40 ns**（−19.8%），Rust 分配从 **2.001 → 1.001 次/次**。
剩下那次分配是事件名/参数路径本身，不再是订阅者表的克隆。

### 5.6 示例工程的脚本侧缓存

[`examples/game_api/scripts/demo_script.lua`](../examples/game_api/scripts/demo_script.lua)
的巡逻实体原先每帧 `entity.find(name)` 三次；现在 `find_patrols()` 只在缓存为空
或实体死亡时重建，热路径只剩三次 `entity.is_alive`：

| 每帧做法 | 成本（p50，三次） |
|---|---:|
| 3 × `entity.find` | 约 **312 ns** + 3 次 Rust 分配 |
| 3 × `entity.is_alive` | 约 **139 ns** + 0 次 Rust 分配 |

这就是 P1 的示范：**名字→句柄在初始化时解析一次，热循环只拿 `i64`**。
同一脚本新增的 `scalar_reads` 步骤直接调用 `transform.position` / `world_position`，
把本文的标量 API 用在真实帧循环里。

## 六、优化优先级

### P0：热路径少返回 table

按“每帧每实体调用一次”估算，`transform.get` 的 706 ns 是当前最值得动的点。
已经落地的方案：

1. **标量 API（已落地）**：`transform.position` / `transform.world_position` /
   `transform3d.position` / `transform3d.world_position`，只返回 2–3 个数，不建表；
   实测 **76.7–110.3 ns**，比 `get` 便宜约一个数量级（§5.4）。
2. **批量 API（已试、已放弃）**：`transform.set_positions` 等实测每实体
   118–154 ns，**比标量调用更慢**（§4.2），已从运行时 API 面移除。
3. **脚本侧缓存（已示范）**：`demo_script.lua` 把巡逻实体的 `entity.find`
   移出每帧循环，热路径只保留 `is_alive`（§5.6）。
4. **保留 table 但只取需要的字段**：需要旋转/缩放/前向时才用 `transform.get`；
   只拿位置用标量 API，只拿空间类型用 `transform.space(id)`（72.4 ns）。

在没有新增 API 之前，**至少不要让 `transform.get` 出现在“每帧 × 每实体”的内层循环**；
现在有了标量 API，内层循环应优先读 `position` / `world_position`。

### P1：按名查找与事件

- `entity_find` **103.96 ns/次 + 1 次 Rust 分配**。名字→实体应在脚本初始化时缓存，
  不要在每帧按名字找人；如果必须动态查找，优先用已经拿到的 `i64` 句柄。
  示例工程已按此改造（§5.6）。
- `event_emit` **215.40 ns/次 + 1 次 Rust 分配**（订阅表克隆已去掉，§5.5）。
  高频事件（每帧每实体）仍应合并成一次批量事件，或直接调用同脚本内的函数；
  事件适合“低频、跨模块”的通知，不适合当每帧的逐实体管道。
- `transform_set` **92.03 ns**、`transform_move` **80.17 ns**：单次不贵，
  1000 次/帧约 92 µs / 80 µs。批量写实测更慢（§4.2），所以当前没有批量写 API；
  真要更新上千个实体，优先确认是否真的需要每帧全量写。

### P2：不要先动 `call_update`

`update_callback` 813 ns/帧，其中大部分是固定护栏成本；按 60 FPS 只占
**0.005%** 的 CPU 时间。先优化脚本里的热循环，收益高一个数量级以上。

### P3：LuaJIT 不是当前的第一选项

- 当前运行时是 **Lua 5.4.8**，LuaJIT 是 **5.1 语义**，不是 drop-in；
  整数除法、`//`、位运算、`goto`、`utf8` 等都要重新评估，绑定层也要换。
- 本次数据里，纯 Lua 计算（8 ns 加法、34 ns 调用）不是帧时间的大头；
  大头是 **table 形状的 API 返回**，换 JIT 不会让 6 字段表的构造免费。
- 只有当真实工程的 `FrameProfile` 证明“纯 Lua 计算”占 Script stage 的主导，
  且这部分无法用 API 形状优化时，才值得做 LuaJIT 评估（另立工作包，
  并保留 5.4 兼容路径）。

## 七、换算到帧预算

[`metrics/baselines/m74-frame-profile.json`](../metrics/baselines/m74-frame-profile.json) 里 `jhyt_manor` 的 p50：

- 整帧 **1609.8 µs**
- Script stage **1224.3 µs**（占整帧 **76.05%**）

用本次 p50 做线性换算（**只是量级估算**，真实脚本的调用次数没有埋点）：

| 每帧调用次数 | `transform.get` | 占 Script | 占整帧 |
|---:|---:|---:|---:|
| 100 | 70.6 µs | 5.8% | 4.4% |
| 500 | 352.9 µs | 28.8% | 21.9% |
| 1000 | 705.7 µs | 57.6% | 43.8% |

其它单次成本：

- 1000 次 `transform_set` ≈ **92.0 µs**（Script 的 7.5%）；
- 1000 次 `transform.position` ≈ **76.7 µs**（Script 的 6.3%）；
- 1000 次 `transform3d_world_position` ≈ **110.3 µs**（Script 的 9.0%）；
- 1000 次 `entity_find` ≈ **104.0 µs**（Script 的 8.5%）；
- 1000 次 `event_emit` ≈ **215.4 µs**（Script 的 17.6%）；
- `update` 每帧一次 ≈ **0.813 µs**，60 FPS 下 **48.8 µs/秒**。

所以“Lua 慢”在这份工程里的具体含义，通常不是解释器算得慢，而是
**每帧每个实体都跨语言返回一张表**。先量调用次数，再决定用标量 API、
脚本侧缓存，还是把读取移出内层循环；批量 API 已试过，当前形状下不划算。

## 八、边界与未验证

- **没有 LuaJIT 数据**：本仓库没有启用 LuaJIT；不能用本文数字推断 LuaJIT 的表现。
- **没有 tolua#/xLua/zLua 数据**：它们是 Unity C#↔Lua 绑定，宿主、GC、对象模型都不同；
  本文不提供跨绑定排名。
- **夹具只有 2 个实体（2D + 3D）**，没有层级、父子链、销毁/复用、并发脚本；
  `transform_get` 的绝对数字会随组件存储布局和实体数量变化。
  `transform3d.world_position` 的 110 ns 是在**无父节点**夹具上测的；有父链且本帧尚未传播时，
  它要现场组合一次，成本会更高（仍只建 Rust 临时数据，不建 Lua table）。
- **没有统计 Lua C 堆的总分配速率**；`lua_bytes_per_op` 只是净增量。
- **纯 Lua / Lua→Rust 采样停 GC**，不代表真实帧的 GC 停顿；真实 GC 成本应看长跑 `FrameProfile` 与
  `wp0_alloc` 的每帧分配量尺。
- **单机单次**，p50/p95 会随 CPU 频率、后台进程、编译选项漂移；不要跨机器比较绝对值。
- 本次确实**新增了运行时 API**（4 个标量位置函数），并同步了 API 快照、`docs/18-lua-api.md`、
  文档桩与 `examples/game_api` 覆盖率演示；`transform.get` 本身仍是 `transform.get(id)`，返回 table。

## 九、复现

```powershell
cd engine
cargo run --release -p engine --example lua_perf -- --cycles 31 --output ..\metrics\baselines\lua-perf.json
```

- `--cycles N`：样本数，默认 11；正式记录用 31。
- `--output PATH`：写 JSON；不传则只打印表格。
- `--help`：参数说明。
- 成功时最后一行是 `lua_perf_ok report_version=lua-perf-v1 ...`，可直接作为 CI 断言。

报告里的关键字段：

| 字段 | 含义 |
|---|---|
| `p50_op_ns` / `p95_op_ns` / `p99_op_ns` | 单次操作的样本分位数 |
| `per_op_ns` | 所有样本的均值（分子是总耗时，分母是 `lua_ops_per_sample`） |
| `lua_ops_per_sample` | 一个样本里实际执行的 Lua 操作数 |
| `lua_bytes_per_op` | Lua 内存净增量 / 操作数（不是分配速率） |
| `rust_alloc_count_per_op` / `rust_alloc_bytes_per_op` | Rust 全局分配器的分配次数/字节 / 操作数 |
| `samples_ns` | 每个样本的原始耗时，便于复查异常值 |

只改文档或只改绑定时，可用 `--cycles 9` 做快速回归；正式基线用 31。

## 十、后续

1. **标量 API 已落地**（`transform.position` / `world_position`、`transform3d.position` /
   `world_position`），API 快照、`docs/18-lua-api.md`、文档桩与示例覆盖率演示均已同步；
   **批量写已试并放弃**（实测更慢，§4.2），不要再按“批量一定更快”的直觉加回来；
2. 给 `jhyt_manor` 的 Script stage 加**每个 API 的调用计数**（低成本计数器），
   把本文的“每次成本”乘上真实次数，替代线性估算；
3. 把 `lua_perf` 接入性能趋势库（见 [70](70-性能与测试度量.md)），
   以 `p50_op_ns` 做棘轮，防止 `transform.get` / `transform.position` 回退；
4. 只有在真实数据证明纯 Lua 计算占主导时，才评估 LuaJIT；评估前先读
   [02](02-technical-decisions.md) 的 Lua 运行时待决策项。
