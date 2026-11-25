# 任务：RuntimeScene 职责与数据流拆分设计

- 任务 ID：M83-03
- 状态：验收①②证据盘点已完成（2026-11-25）；候选 2 已落地（`20b626a5`）、
  候选 1 第一/二步已落地（`45059416` / `ad94743a`），候选 1 第三步（文本提取）与候选 3 未开始
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

- 尚未开始实施；本任务不授权无证据的大重写。

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
   - **第三步（未做）**：`text.rs` 的文本提取仍读 `self`（`text_render_mode` /
     `render_scale` 读点）；迁移时把 `render_scale` 放进 input。
     先例：`FrameView3d`（`mod.rs:96-101`）已经是"只读视图 struct + 同帧多消费方"。
   验证：`tests/frame_view.rs` 4 条、`tests/frame_ledger.rs` 2 条、
   `tests/frame_extraction.rs`、`tests/world_depth.rs` 2 条、`ui*` 四文件、
   `wp0_*_golden` 像素门。
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
   3 个模块消费者（`text.rs`/`frame_data.rs`/`scene.rs`）+ 2 个跨 crate 注入方
   （editor `preview_session.rs:215-223`/`scene_view/render.rs:463-466`、`game_web`）。
   注意：`frame_data.rs:1355-1361` 的"空网格帧不推进字体版本"只有注释**没有测试**，
   动之前先补一条断言。
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

> 现状结论：本任务验收①②的**证据已齐**（本节），③④的结论是"只有候选 1-3 值得立项"。
> 实施仍按"先补缺口测试 → 单候选单提交 → 逐条跑 API/像素/性能门"推进。
