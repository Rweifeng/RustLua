# 任务：RuntimeScene 职责与数据流拆分设计

- 任务 ID：M83-03
- 状态：已归档（2026-11-26）。候选 1 三步（`45059416` / `ad94743a` / `c9f29d5d`）、候选 2
  （`20b626a5`）、候选 3 三阶段（`3a9646e5` / `3359a79f` / `44c0fd7d`）全部落地；候选 4 延后，
  EditorApp 侧按"先收敛访问"登记
- 关联里程碑：M83
- 创建日期：2026-11-16

## 目标

降低 `RuntimeScene` 对 Lua、ECS、物理、相机、UI、资源缓存和帧提取的集中耦合；先以真实消费者、编译/性能测量和可逆迁移设计确定边界，再实施小步拆分。

## 影响文件

- 预期涉及 `engine/crates/engine/src/host/runtime_scene/`
- 可能涉及 `rustlua-scripting`、`rustlua-scene`、`rustlua-render` 及架构边界测试

## 风险

- 大规模搬迁会扩大回归面、破坏帧调度顺序或形成新的循环依赖。
- 不以文件行数下降作为唯一收益；没有两个以上真实消费者的抽象应判为不做。

## 验收方式

1. 记录当前字段/调用/消费者和调度边界。
2. 提交按数据流划分的候选接口及依赖方向检查。
3. 每一迁移步保持调度顺序、API 快照、相关单测和性能基线不回退。
4. 没有足够消费者的候选模块明确标注为不做或延后。

## 未完成项 / 已知限制

- ~~尚未开始实施；本任务不授权无证据的大重写。~~
  （2026-11-26 归档时修订：候选 1/2/3 已按"单候选单提交"落地，逐条见 §3；仍然有效的纪律是
  本任务不授权无证据的大重写，§4 已把五条"明确不做"逐条裁定。）

## 证据盘点（2026-11-25，验收①②）

> 只读分析，口径：字段"消费者"= 除构造点外对 `self.<field>` / destructuring 取用的
> 源文件数（不含 tests）。行号为该日工作树快照。

### 1. RuntimeScene 规模与字段分布

- 结构体在 `engine/crates/engine/src/host/runtime_scene/mod.rs:111`（字段区间 `:115-454`）：
  **130 个字段，全部 `pub(crate)`**；唯一构造点 `scene.rs:816-953`。
- 没有 ECS `World` 字段：世界由 `rustlua-scripting/src/script/mod.rs:253` 的
  `ScriptState.world: Handle<World>` 持有；宿主只有 `with_world` / `with_world_mut` 两个入口
  （运行期内这两类入口共 107 处）。
- 按数据流分 15 组：UI 31、渲染帧提取 18、文本字体 16、ECS/空间传播 15、粒子 12、
  资源身份 7、相机 6、物理 6、调度器 7、环境 3、Lua 2、动画/Spine 3、输入 2、音频 1、
  渲染覆盖 1。
- 消费者分布：**57/130 字段只有 1 个消费者文件**，53 个 2 个，11 个 3 个，仅 8 个 ≥4
  （`script` 17、`camera` 8、`input` 6、`ui_scene_nodes` 5、`ui_layout_rects` 5、
  `assets` 4、`spatial_active` 4、`world_sprite_depth` 4）。按本任务判据，57 个单消费者
  字段**不产生新抽象**。

### 2. 调度与 API 边界（迁移必须保持不变）

- 两条固定管线：step 管线与 frame 管线，系统顺序由
  `runtime_scene/tests/schedule.rs:176-210` 逐名钉死；帧提取顺序由
  `frame_data.rs:700`（`vertices`）与 `frame_data.rs:67`（`scene_3d`）决定。
- 跨 crate 消费者全部走 public 方法（字段出不了 crate）；对外符号面由
  `engine/tests/host_pub_surface.rs:52-128` 的 77 条编译期 pin 守护，另有
  `scripts/verify/engine-pub-surface.mjs`（需本地基线，缺基线时门 SKIP 而非通过）。
- `engine` 是组合根（`rustlua-infra/tests/architecture_edges.rs:271`）：任何留在 `engine`
  内的拆分单元零依赖变更；下沉到领域 crate 属另一层决策，本任务不做。

### 3. 候选（按收益/风险排序；均要求 API 快照与调度顺序不变）

1. **帧提取只读输入视图 `FrameExtractInput`（推荐先做）**
   - **第一步已落地（2026-11-25，`45059416`）**：`FrameExtractInput` 收
     `camera` / `frame_view_3d` / `viewport` / `view_world_mask` / `sprite_culling` /
     `tilemap_culling` / `sprite_parallelism` / `world_sprite_depth` / `text_render_mode` /
     `render_ui_overlay` / `spatial_index` / `spatial_index_cell_size`（全部 `Copy`）。
     `vertices()` 的精灵/Quad/粒子/世界通道/文字开关读点 + `append_tilemap_vertices`
     （签名改收 `&FrameExtractInput`）+ `scene_3d_with_camera` 的视图/遮罩读点已改读 input。
     验证：engine 270 lib + 全部集成测试绿、`cargo check --workspace` 通过。
   - **第二步已落地（2026-11-25，`ad94743a`）**：UI 提取并入同一输入——
     `push_ui_panels` / `push_ui_images` / `push_ui_grids` / `push_scene_ui_images` /
     `push_scene_ui_text` 改收 `&FrameExtractInput`；`scene_ui_image_candidates` /
     `scene_ui_text_candidates` 改收 `overlay_mask`；`push_image_quad` 收
     `ui_reference`；测试侧新增 `extract_input(scene, w, h)`。
   - **第三步已落地（2026-11-25）**：文本提取并入同一输入——新增 `TextExtractConfig`
     （`text_render_mode`/`render_scale`/`world_sprite_depth`/`render_ui_overlay`），
     公开度量 API 用 `TextExtractConfig::capture(scene)` 自取、提取路径用
     `FrameExtractInput::text_config()`；`should_render_vector`/`should_render_hinted`/
     `hinted_ppem`/`prewarm_frame_(hinted_)glyphs`/`layout_(scene_)hinted_text`/
     `push_world_text`/`push_ui_text` 改收配置；`prewarm_frame_hinted_glyphs` 显式收
     `overlay_mask`。度量与渲染共用同一份模式配置。
     验证：engine 270 lib + 全部集成测试绿、`cargo check --workspace` 通过。
   - 先例：`FrameView3d`（`mod.rs:96-101`）已经是"只读视图 struct + 同帧多消费方"。
   验证：`tests/frame_view.rs` 4 条、`tests/frame_ledger.rs` 2 条、
   `tests/frame_extraction.rs`、`tests/world_depth.rs` 2 条、`ui*` 四文件、
   `custom_fonts.rs` 7 条、`wp0_*_golden` 像素门。
2. **帧内纹理身份表 `TextureIdentities`——✅ 已落地（2026-11-25，`20b626a5`）**
   收 `assets` / `texture_paths` / `texture_lookup` / `texture_handles` 为一个
   `pub(crate)` 单元（`runtime_scene/mod.rs`），`texture_id_of` 变 `id_of` 方法；
   `begin_frame()` 每帧清本帧三表、`assets` 不清空；`intern_asset()` 幂等；
   `handle_of`/`entries`/`count` 同时服务 3D 网格、动画片段与性能面板。
   调用点：`frame_data.rs`（3 处 destructuring→1 字段、4 处 id 注册）、`tilemap.rs`（2）、
   `schedule.rs`（2）、`scene.rs`（init + asset_entries/count/handle）、两处测试改用
   `runtime.textures.paths()`。公开方法签名与数量不变；像素/排序/每帧重建顺序不变。
   验证：`cargo test -p engine --tests -- --skip
   sample_project_demos_cover_every_lua_api`（lib 270 + 集成全绿）、`cargo check
   --workspace`、`cargo fmt`。跳过项与 clippy 红来自并发会话在制品，与候选 2 无关。
   下一步在它之上做候选 1：`vertices()`/`scene_3d()` 已只剩一个身份表字段要传。
3. **文本/字体缓存单元 `TextFontCache`（16 字段）**
   - **第一阶段已落地（2026-11-25）**：14 个缓存/账字段（四类图集句柄、
     `last_font_version` 与各版本账、`frame_*_keys`、`missing_*`、`hinted_font_bytes`）
     收成 `TextFontCache`；`render_scale`/`text_render_mode` 留在 `RuntimeScene` 当配置；
     编辑器注入 setter 签名不变。引用搬家涉及 `text.rs` / `frame_data.rs` / `scene.rs`。
     验证：engine 270 lib + 全部集成测试绿、`cargo check --workspace` 通过。
   - **第二/三阶段（第三阶段已完成 2026-11-25）**：
     `snapshot_custom_fonts` / `snapshot_hinted_fonts` 已从 `frame_data.rs` 移入 `text.rs`；
     新增不依赖 `examples/sourcehan_test` 资产的回归
     `empty_frames_never_consume_the_default_font_version`；默认图集的版本握手判断也收成
     `TextFontCache::snapshot_default_atlas(will_draw)` 方法（`frame_data` 只留调用点，
     原"为什么空网格帧不能推进版本"注释保留）。
     注入接口评估：**不加整体 `replace`**——编辑器只在 4 处按字段注入，没有整单元替换的消费者。
   注意：`frame_data.rs` 的"空网格帧不推进字体版本"注释必须逐字保留。
4. **延后：相机定帧单元 `CameraFrame`**：字段消费者最多（8 文件）但账本门
   （`tests/frame_ledger.rs:51-154`）精确钉住 `camera3d_resolutions=1/view_3d=1`，
   先做完候选 1 再评估。

### 4. 明确不做（验收④）

- 为 57 个单消费者字段建接口；
- 给 `spine_state`/`animation_state` 再包一层（已是 `Handle` 能力单元，单站点）；
- `system_parallelism` 未找到真实消费者（仅声明+setter+getter+API pin）→ 不改行为，
  另登记核对；
- `render_overrides` 已是能力单元，字段只是透传；
- 把任何单元下沉到领域 crate（需改架构预算与 Rustdoc 面，另开决策）。

### 5. EditorApp 侧读数（与 RuntimeScene 同题但不同批）

- `engine/crates/editor/src/main.rs:256-795`：**248 个字段**，其中
  "面板本地会话状态" 57、"文档/工程生命周期" 48、"预览/渲染桥接" 43、
  "dock/布局/矩形缓存" 39、"场景编辑交互/事务" 28、资源浏览器缓存 21、性能证据 12。
- 文档生命周期类型已下沉 `rustlua-editor-core`，但**状态所有权与编排仍在 EditorApp**
  （`document/history/play/play_session/recovery/autosave/pending_document_change/
  prefab_stage` 等），面板私有字段（`command_palette_*`、`console_filter` 等）也被
  面板模块直接读写（`editor/src/ai_host.rs:110`、`command_router.rs:673` 等）。
- 若后续做同类拆分，按"≥2 消费者"判据：`document+history+pending_*+autosave/recovery`
  具备抽 `DocumentSession` 的资格；`command_palette_*` 只有 1 个面板消费者，不抽。

#### 5.1 实测读数（2026-11-25，`editor/src` 全量 grep `app.<字段>`/`self.<字段>`）

| 字段 | 读/写点 | 涉及文件 | 判读 |
|---|---:|---:|---|
| `document` | 451 | 67 | 面板/命令/AI/门全在用；**抽结构前必须先收敛访问** |
| `history` | 226 | 40 | 同上（与 document 强相关） |
| `play` | 52 | 13 | 预览编排，跨 actions/panels |
| `play_session` | 46 | 16 | 同上 |
| `pending_document_change` | 44 | 9 | 已有 `request_document_change` 网关；余下多为门/测试只读 |
| `prefab_stage` | 26 | 7 | 20 处在 prefab 自己的 actions/测试，单特性 owner |
| `edit_transaction` | 19 | 3 | 13/19 在 `actions/transaction.rs`，已是单 owner |
| `pending_save_failed` | 12 | 3 | 集中在 `document_gate.rs` |
| `recovery` / `autosave` / `recovery_scan` | 9/8/7 | 6/6/3 | 只是几个布尔 + 扫描时间戳，散布在菜单/头部/导入，抽单元收益弱 |
| `quit_approved` | 3 | 3 | 退出编排标记 |

**结论与下一步（2026-11-25）**

1. **暂不抽 `DocumentSession`**：`document`+`history` 共 677 个读写点、跨 70+ 文件，
   抽结构会把一次提交变成全 crate 机械改动，回归面远大于收益；"≥2 消费者"只是必要条件，
   这次被"单点改动爆炸半径"否决。
2. 若要做，先做**访问收敛**：给 `document`/`history` 加 `document()/document_mut()/
   history_mut()` 一层，把跨模块直接字段访问降下来并分批提交；收敛达标后再评估抽结构。
   这一步本身也按 `editor-core` → `actions` → `panels` 分批，不能一把梭。
3. 小单元（`RecoveryState` 之类）**不做**：字段太少（3 个布尔 + 1 个时间戳）、
   消费者分散在菜单/头部/导入，抽出来只是搬字段名。
4. `edit_transaction` 已是单 owner（`actions/transaction.rs` 13/19），不抽。
5. 下一批 EditorApp 工作若继续：先测量 `command_router`/`ai_host` 对面板私有字段的
   直读（已知各 1 处），给面板加只读查询 API，减少跨模块写面——这是比抽 Session
   更小、可独立验收的一步。

> 现状结论：本任务验收①②的**证据已齐**（本节），③④的结论是"只有候选 1-3 值得立项"。
> 实施仍按"先补缺口测试 → 单候选单提交 → 逐条跑 API/像素/性能门"推进。

## 归档记录（2026-11-26）

- **归档依据**：验收四项都有落点——①证据盘点（§1 字段/消费者分布、§2 调度与 API 面钉点）；
  ②候选接口与依赖方向（§3，`FrameExtractInput` / `TextExtractConfig` / `TextureIdentities` /
  `TextFontCache`）；③每一步迁移的记录（每步 engine 270 lib + 集成测试绿、`cargo check
  --workspace` 通过）；④"消费者不足的候选不做"（§4 五条）。三个候选全部落地，提交为
  `20b626a5`（候选 2）、`45059416` / `ad94743a` / `c9f29d5d`（候选 1 三步）、
  `3a9646e5` / `3359a79f` / `44c0fd7d`（候选 3 三阶段）。
- **未达成 / 只登记、不开新包**：
  1. 候选 4 `CameraFrame` 延后——账本门 `tests/frame_ledger.rs` 钉死
     `camera3d_resolutions=1/view_3d=1`，要先证明拆分不动这两条再评估；
  2. EditorApp 侧（§5/§5.1）：`document` + `history` 677 个读写点跨 70+ 文件，**暂不抽
     `DocumentSession`**；先做 `document()/document_mut()/history_mut()` 的访问收敛并按
     `editor-core` → `actions` → `panels` 分批提交，达标后再评估抽结构。面板私有字段加只读
     查询 API 是比抽 Session 更小的一步。
  两条都留在 §3/§5.1 的原文里，由后续批次按判据触发，不另立任务包。
- **口径**：本任务交付的是"有证据的小步拆分"，不是 RuntimeScene 大重写；行数下降不是收益指标，
  §4 的单消费者字段条款保持有效。
