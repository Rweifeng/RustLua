# 任务：per-tile tint、确定性变体与 4-bit autotile

- 任务 ID：M72-02
- 状态：待办
- 关联里程碑：M72（P0 引擎地基）
- 创建日期：2026-10-08
- 依赖：M72-01
- 阻塞：M72-04 → M72-05 → M72-06

## 目标

补掉"地面没有变体、地形交界是直线、没有 per-tile 颜色"这三条，让地表能画出
星露谷那种"相邻两块永不完全相同 + 不规则犬牙边"的质感。今天这三条**一条都没有**。

## 现状（实测）

- **变体 / autotile / per-cell UV 偏移：完全没有。** `tile_uv` 是存储值的纯函数，
  每个非空格恰好映射一个图集位置。
- `meta: BTreeMap` 存在（`tilemap.rs:38`）但**不序列化**（`:27-28`）且渲染器从不读 → 不能拿它承载。
- **per-tile tint 结构上已经免费**：顶点格式带 `color:[f32;4]`
  （`engine/crates/engine/src/host/frame.rs:22`（OverlayVertex）、`:56`（WorldSpriteVertex）），
  但 tilemap 侧把 tint 硬编码成不透明白：`runtime_scene/tilemap.rs:202`（overlay）、`:297`（world）。
- 合批：一层一个 `SpriteDrawBatch`，同贴图连续段合并（`:221-236` overlay、`:319-335` world），
  每 tile 6 顶点。
- 容量：`tilemap_stress.rs:37` 默认 100×100=10,000 瓦 / 30 帧；编辑器网格维度 clamp 1..=256
  （`inspector.rs:716,720`）。`manor` 是 40×23=920 格 → **加变体和 autotile 不碰性能红线**。

## 影响文件

- `engine/crates/rustlua-ecs/src/tilemap.rs`（逻辑瓦 → 图集索引的解析、变体哈希、邻居掩码）
- `engine/crates/engine/src/host/runtime_scene/tilemap.rs:202,297`（接 per-tile tint）
- `engine/crates/editor/src/panels/`（笔刷要能刷"逻辑瓦"而不是图集索引，否则美术没法画）
- `examples/game_jhyt/tools/pixel_kit.py` / 新增 `terrain_kit.py`（生成器侧要能出同构的 autotile 表，
  否则预览图与实机不一致）

## 设计要点

1. **两层语义分开**：格子里存「逻辑瓦 id」（草/土/石/水…），渲染时再由
   「邻居掩码 + 确定性哈希」解析成「图集索引」。直接把图集索引写进格子 = 美术要手工摆 16 个变体，
   等于没有 autotile。
2. **autotile 首版只做 4-bit 掩码 = 16 变体**（上/下/左/右四个邻居在不在）。
   **明确做不到**星露谷那种 47 变体全表与不规则犬牙岸线——那需要 8-bit 掩码或手工装饰层。
   岸线的不规则感由 M72-05 的 sprite 装饰层压边凑，不假装 autotile 能解决。
3. **变体哈希必须确定性**：`hash(cell_x, cell_y, layer_seed) % variant_count`。
   用 `std::collections::hash_map::DefaultHasher` 或任何带随机种子的实现都是错的——
   每帧抖一次，所有画面类门必红。种子要进组件串并序列化，编辑器与运行时读同一个值。
4. per-tile tint 首版只接两个来源：**季节/湿润的整体色调** 与 **阴影压暗**。
   不做逐格手涂（没有编辑器 UI 支撑，做了也没人能用）。

## 风险

- autotile 让"格子值"的语义变了 → M72-01 的 `.scene` 迁移与 M72-02 的解析层要划清边界，
  别把两件事写进同一个函数。
- 确定性哈希一旦进黄金表，改哈希函数 = 全图重排 = 画面类门全红。**哈希函数与种子要钉进门里的常量**。
- 编辑器笔刷若仍刷图集索引，美术会绕过 autotile 手工摆位，规范立刻失效。这条要在验收里显式测。

## 验收方式

- `jhyt_m72_tile_variant_determinism`：同一种子重跑 100 帧，逐帧取 tile→图集索引序列，断言**全等**。
- `jhyt_m72_autotile_mask`：造一块 8×8 草地嵌 4×4 土，断言 12 个交界格取到转角/T 形瓦、
  内部格不出现压边瓦。
- **反面对照**：把邻居掩码里的"上"一位去掉，门必须红在顶部那一排交界格上。
- per-tile tint：断言 `tint != [1,1,1,1]` 的格子在顶点色里真的带下去了（读 mesh 顶点，不看像素）。
- **反面对照**：把 `:202`/`:297` 的 tint 改回硬编码白，tint 断言必须红。
- 登记进 `verify.ps1` MVP 清单。

## 实现记录

（待回填）

## 验收证据

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] 变体确定性门绿 + 反面对照跑红
- [ ] autotile 掩码门绿 + 反面对照跑红
- [ ] per-tile tint 门绿 + 反面对照跑红
- [ ] 已登记进 `verify.ps1` MVP 清单

## 未完成项 / 已知限制

- 4-bit 掩码做不到不规则岸线（设计内，见 §设计要点 2）。
- 逐格手涂 tint 无编辑器 UI（设计内）。
