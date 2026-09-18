# 任务：WP6 提取路径去分配（第一刀：2D 精灵收集）

- 任务 ID：WP6（工作包级；提取路径去分配已完成并实测；World2D/RenderSnapshot 的范围负责人本轮已裁定）
- 状态：提取路径去分配**已完成并实测**（2026-09-17）；世界 Sprite 深度通道**进行中**——
  地基已落地（Sprite 世界 z、`Camera2D::view_projection`），剩余的通道本体见下（2026-09-18）

## 世界 Sprite 深度通道：地基（本轮完成）

裁定要求"世界 Sprite 移入带深度的 pass"。落地需要两块地基，本轮先补上：

1. **Sprite 的世界 z**（`Sprite`/`SpriteComponent` 新增 `z`，组件串可选 `z=`，默认 0）：
   深度通道必须有一个可比较的深度来源，而 2D 之前只有 sorting layer/order（docs/54 明确
   "排序层不等于深度"）。默认 0 且**为 0 时不写回组件串**，既有场景字符串逐字节不变；
   `z=nan`/`z=inf`/拼错一律**报错**（NaN 会让整帧深度测试失效，属"必须拒绝"而不是静默当 0）。
2. **2D 相机的矩阵形式**：`Camera2D::view_projection(viewport, near, far) -> Mat4`——顶点着色器
   需要一份矩阵，而此前只有逐点 `world_to_screen`。它与逐点换算**同源同约定**（NDC 的 x/y 就是
   屏幕坐标线性映射到 [-1,1]，y 向上；世界 z 线性映射到 NDC z ∈ [0,1]，与 3D 一致），
   由 `two_d_view_projection_matches_the_pointwise_projection` 在
   `rotation × zoom × 采样点` 网格上逐点对拍，并钉住 near→0 / far→1 与非法深度范围报错。

**为什么先补地基而不是直接上通道**：通道本体需要新的顶点格式（世界 xyz）＋独立 quad pipeline
＋深度附件 + 提取侧产出世界空间四边形；没有这两块，写出来的通道只能靠"逐顶点在 CPU 上再算一遍
屏幕坐标"或"每个四边形一个近似深度"，后者正是本项目明确禁止的静默近似（docs/54 §1.2）。

## 层描述显式化（本轮完成）

裁定执行要求 ① 是"层描述显式化（World2D / Overlay2D / ScreenUI）"。此前"这段顶点属于世界内容
还是屏幕 UI"只体现在**谁调用了哪个 push 函数**里；世界 Sprite 深度通道落地后这个区分第一次有了
行为后果，因此写成类型：

- `core/layer.rs`：`RenderLayer::{World2D, Overlay2D, ScreenUI}` + `is_world()` /
  `is_screen_space()` / `as_str()`；文档写明 **`layer`（同一层内先后）≠ `render_layer`
  （属于哪个渲染层）**，与 docs/54"排序层不等于深度"一致。
- `SpriteDrawBatch.render_layer`（精灵批 = 世界内容）与 `SceneDraw.render_layer`；`SceneDraw`
  增加 `world(...)` / `screen_ui(...)` / `overlay(...)` / `with_sdf(...)` 构造器，让每个产出点
  **显式声明**自己的层，而不是靠字段顺序猜。
- 产出点标注：瓦片/精灵/世界文字 = `World2D`；屏幕 UI 面板/网格/血条/图片/UI 文本 = `ScreenUI`
  （`push_text_run` 因此新增 `render_layer` 参数，世界文字与屏幕文本各传各的）；
  collider 调试绘制与性能浮层 = `Overlay2D`。

**证据**：`draw_ranges_carry_the_render_layer_of_their_producer` —— 只有世界文字的场景里全部分段是
`World2D`；只有屏幕 UI 文本的场景里全部分段是 `ScreenUI`。这把"哪段几何属于哪一层"从"看代码"
变成可断言事实，也是世界 Sprite 深度通道"只吃世界层"的前提。

**排序契约改为"测深度但不写深度 + 按 z 排序"（按负责人要求）**：宿主把世界内容整体排成
**画家顺序**——`SpriteBatch::build_world_into` 按（`layer` → z 远到近 → 纹理 → 原始下标）排序，
`sort_world_ranges` 再把瓦片层/精灵/世界文字三组分段整体重排（它们各自有序，跨组必须统一排）。
分段保证**深度一致**（跨 z 不合并），因此"一个分段一个 z"成立，排序键可直接读分段首顶点的 z，
不必给分段加字段。渲染器侧 `DepthPass::WorldSprite` 不写深度、深度附件保留（为将来与 3D 共享
深度缓冲留位）。**代价写清楚**：顺序成为宿主的责任——两条像素测试钉住这份契约
（画家顺序 ⇒ 近处胜出；顺序反过来 ⇒ 远处胜出，说明深度不兜底）。

**顺带修掉一个此前的真实回退**：三条管线（3D 网格 / 阴影 / 世界 Sprite）的
`depth_write_enabled` 曾是散落字段，一次脚本化编辑把**网格与阴影**误改成不写深度 ⇒ 3D 前后遮挡
静默失效，而 golden 只断言可绘制对象/实例/三角形/非背景像素、**断言不了深度顺序**，所以全门
绿灯。现在策略收进 `DepthPass` 类型 + `depth_policies_are_pinned_per_pass` 逐条锁死。
**瓦片层已进入世界通道（本轮）**：`append_tilemap_layer_world`（与 `append_tilemap_layer` 共用
同一份几何——中心偏移 + 旋转缩放后的四角 + 瓦片 UV，区别只有"写世界坐标"与"用世界 AABB 剔除"）
让瓦片层在开启世界深度通道时也产出世界坐标顶点（`z = 0`，瓦片层就是世界平面）。证据
`world_sprite_depth_moves_tilemaps_into_the_world_channel`：2×2 地图在关闭时产出 24 个 overlay
顶点、无世界顶点；打开后 24+ 个**世界坐标**顶点（`max_x > 100`，不是 NDC）、overlay 顶点数下降、
分段全部是 `World2D`。此前的缺口是"世界内容一半有深度、一半没有"，遮挡必然错。

**世界文字也进了世界通道（本轮）**：转换走 `view_projection` 的**逆映射**——字形先在像素
空间排版（与 overlay 路径同一套字体管线），再把每个像素顶点映回世界坐标，因此屏幕观感与 overlay
路径逐个像素一致，却获得了世界坐标与 `z`。证据 `world_sprite_depth_moves_world_text_into_the_world_channel`：
关闭时产出 overlay 顶点、无世界顶点；打开后世界顶点落在**实体世界坐标 (0,0) 附近**（不是 NDC）、
`z` 全为 0、分段登记在**世界**绘制列表里。至此精灵、瓦片层与世界文字三类世界内容全部参与深度。
## 世界 Sprite 深度通道：渲染侧已完成（本轮）

- `WORLD_SPRITE_SHADER`：group0 = 2D 相机 `view_projection` uniform，group1 = 既有
  texture/sampler 布局；顶点是**世界坐标**，片元与 `TEXTURED_SHADER` 的普通纹理分支同源
  （采样 × 顶点色），因此世界路径与 overlay 路径在颜色上不分叉——差异只有"投影在哪算"
  与"是否参与深度"。
- 渲染器：`world_sprite_pipeline`（顶点布局 Float32x3/Float32x2/Float32x4，ALPHA_BLENDING，
  深度 `LessEqual` **并写深度**）、`set_world_sprite_camera(Mat4)`、按需扩容的世界顶点缓冲、
  独立深度附件；pass 插在 **3D mesh pass 之后、2D overlay 之前**，只在
  `mesh.world_sprites` 非空时执行（不喂数据时零成本）。
- `RuntimeScene::set_world_sprite_depth(bool)`（默认关闭 = 现状 overlay）＋
  `sprite_view_projection(w, h)`：深度范围取 **(-1, 1)**，精灵默认 `z = 0` 落在深度中间，
  向 -z 靠近相机、向 +z 远离；宿主（`game_app`）每帧把矩阵交给渲染器。
- **取舍写进代码**：写深度让 `z` 成为权威遮挡依据，代价是"半透明精灵与更远精灵重叠时，
  远处那个不再透出来"——这是从"2D 完全没有深度"到"有深度"的必要取舍，回滚档是整个通道的开关。

**证据**：

| 项 | 结果 |
|---|---|
| 深度决定遮挡（GPU 回读） | `world_sprite_pass_resolves_overlap_by_depth_not_draw_order`：先画近处绿色 `z=-0.5`、后画远处红色 `z=+0.5`，中心像素为**绿色**——纯按绘制顺序会得到红色，因此这条只有真的做了深度测试才会通过 |
| 开关两态 | `world_sprite_depth_flag_switches_between_overlay_and_world_vertices`：关闭 ⇒ NDC 顶点 + 无世界顶点；打开 ⇒ 世界坐标（`640-8=632`、`z=-0.25` 来自 `Sprite::z`）+ 无 overlay 精灵顶点；关回去逐字节回到 overlay |
| 提取侧契约 | `world_sprite_builder_emits_world_coordinates_with_z`（世界坐标/pivot 偏移/z/分段/并行==串行/剔除） |
| 像素不回退 | 开关默认关闭 ⇒ 2D overlay golden 与 3D semantic golden 逐字节不变（`verify.ps1 -Benchmarks` 全绿） |
| 门 | engine lib **661 项**、editor 109 + 165、clippy/fmt/build/check-docs 全绿 |



**提取侧已就绪（本轮）**：

- `Sprite::quad_world(transform)`：世界空间四角，与 `Sprite::quad` **共用同一份局部角点与变换
  数学**（区别只是不做投影），因此不会出现"CPU 投影"与"GPU 投影"两套约定；
- `SpriteBatch::WorldSpriteVertexSink` + `SpriteBatch::build_world_into(...)`：单趟产出世界
  `(x, y, z)` + UV + 颜色，排序/分段/错误语义与 overlay 路径一致，支持并行（与串行逐字节相同）
  与世界矩形剔除；`z` 来自 `Sprite::z`；
- `host::frame::SceneMesh` 新增 `world_sprites` / `world_sprite_draws`（宿主侧顶点类型
  `WorldSpriteVertex` 实现该 sink），全部既有 `SceneMesh` 字面量已同步（编译期强制）。

测试 `world_sprite_builder_emits_world_coordinates_with_z`：世界坐标角点（含 pivot 偏移）、
`z` 透传、分段与 overlay 一致、并行 == 串行、矩形剔除生效。

**渲染侧（下一步，工作量集中在这里）**：

1. `WORLD_SPRITE_SHADER`（group0 = 2D 相机 `view_projection` uniform，group1 = 既有
   texture/sampler 布局）＋ `world_sprite_pipeline`（顶点布局 Float32x3/Float32x2/Float32x4，
   ALPHA_BLENDING，深度 `Less`、**不写深度**）；
2. 渲染器新增：`sprite_camera_buffer`+bind group（由宿主每帧写入
   `Camera2D::view_projection(viewport, near, far)`）、`world_sprite_buffer`（按需扩容）、
   独立深度附件 `world_sprite_depth_view`；
3. `render_passes` 在 mesh pass 之后、2D overlay 之前插入该 pass（仅当
   `mesh.world_sprites` 非空），按 `world_sprite_draws` 逐段绑定纹理；
4. `RuntimeScene::set_world_sprite_depth(bool)`（默认关闭 = 现状 overlay）＋
   `sprite_view_projection(width, height)` 供宿主喂给渲染器；
5. 像素证据：关闭 ⇒ 现有 golden 逐字节不变；打开 ⇒ 两个重叠精灵在"z 说后面那个后画"时
   仍正确遮挡（纯绘制顺序做不到），并解释与 overlay 路径的像素差异。

- 关联里程碑：[docs/40](../docs/40-引擎全面重构计划.md) §3 WP6
- 前置：WP0 增量基准（`wp0-v4`，每帧分配量尺）；WP0 关闭报告授权增量扩展

## 负责人裁定（本轮）

**把世界 Sprite 移入带深度的 pass**（不是"只声明层元数据、像素不变"的保守方案）：Sprite 与
Mesh 在同一视图内按材质/深度/pass 协作，2D 世界不再是 3D 之后的无深度 overlay。执行要求：
① 层描述显式化（World2D / Overlay2D / ScreenUI）；② 世界 Sprite 走独立 quad pipeline 并参与
深度；③ **2D overlay golden 与 3D semantic golden 是门槛**——像素差异必须逐条解释（排序、
透明、深度），不能默默变化；④ 逐场景/逐视图回滚档（`set_world_sprite_depth(bool)`），关掉即
回到"3D 之后画 2D overlay"。

## 为什么先做这条

做完各包证据后对比实测规模：WP2 空间继承对当前内容收益为 0；WP3 存储在 1k 规模不足帧预算的 0.05%。而 `wp0-v4` 量到 **10k Sprite 仅 CPU 提取就占 60fps 预算的七成（11.5 ms/帧）、每帧分配 36,739 次 / 13.25 MB**——这是唯一已被证明的、当下就感知得到的瓶颈。

而且这条**不需要 World2D 范围裁定**：它只消除重复克隆与中间表，不改变 2D/3D 分层语义。

## 改动

`engine/crates/engine/src/host/runtime_scene.rs`（`vertices()`）：

- 删掉每帧构造的中间表
  `Vec<(Transform, Sprite, Option<AnimationStateMachine>, Option<String>, Option<SpriteSorting>)>`——它每帧要搬上兆字节，还要为**每个精灵**克隆资源路径（`SpriteAsset::path().to_owned()`）与排序层（`SpriteSorting::cloned()`）。
- 改为在 `world` 借用内部**直接写进批处理**：用 `let RuntimeScene { script, sprite_batch, texture_paths, texture_lookup, .. } = self;` 做字段级拆分借用（同 `run_steps` 里 `mem::take(scheduler)` 的既有思路）。
- 纹理注册抽为自由函数 `texture_id_of(path, paths, lookup)`，使闭包内无需 `&mut self`；`texture_id` 保留并委托，行为不变。
- **行为保持不变**：遍历顺序、纹理注册点与"缺资源精灵仍占一个序号"的层号规则都按旧实现照抄；`with_uv` 的错误改为经 `failure` 槽上报（`for_each` 的闭包签名返回 `()`，不能直接 `?`）。

## 实测（Release，30 cycles，1280×720，`wp0-v4`）

| Sprite | 分配次数/帧 | 分配字节/帧 | 帧 p50 |
|---:|---|---|---|
| 1,000 | 3,736 → **1,727**（−54%） | 1.11 MB → 0.65 MB（−41%） | 1,013 → 892 µs（−12%） |
| 10,000 | 36,739 → **16,726**（−54%） | 13.25 MB → **6.10 MB**（−54%） | 11,534 → **8,943 µs（−22%）** |
| 50,000 | 183,407 → **83,392**（−55%） | 59.24 MB → 30.33 MB（−49%） | 65,330 → 54,559 µs（−17%） |

新增归属拆分字段（`script_alloc_count_p50` / `extract_alloc_count_p50`）：10k 档剩余 16,726 次中，**步骤管线 3,376（20%）+ 提取 13,350（80%）**。

## 第二刀：定位并消除"每精灵 1 次分配"（2026-09-17）

用定向探针把"每精灵"与"每实体"成本分开：同实体数但只有 `Transform` 的场景，10k 实体下提取侧分配为 3,336（0.33/实体，与精灵无关）；含精灵时为 13,350 → **差 10,014 ≈ 1.0/精灵**。

定位结果：`core/sprite.rs` 的 `sorting_layer_rank` 用了 `layer.trim().to_ascii_lowercase()`——`to_ascii_lowercase()` 在 `&str` 上会**返回新的 `String`**，而 `effective_layer()` 每个精灵每帧都要调一次，于是 10k 精灵 = 1 万次分配/帧。

改法：用 `eq_ignore_ascii_case` 做大小写不敏感比较，零分配；`default` 与未知值仍归 0，行为不变。

### 累计实测（Release，30 cycles）

| Sprite | 分配次数/帧（原始 → 第6轮 → 本轮） | 帧 p50（原始 → 本轮中位数） |
|---:|---|---|
| 1,000 | 3,736 → 1,727 → **727（−80.5%）** | 1,013 → 870 µs |
| 10,000 | 36,739 → 16,726 → **6,726（−81.7%）** | 11,534 → 9,643 µs（−16%） |
| 50,000 | 183,407 → 83,392 → **33,392（−81.8%）** | 65,330 → 55,479 µs（−15%） |

提取侧分配已降到"无精灵场景"的地板（10k 档 3,350 ≈ 0.33/实体），说明精灵路径现在基本零分配。

**必须如实说明**：分配次数在三次重复运行中完全稳定（727 / 6,726 / 33,392），但**帧时间的改善落在测量的 ±5% 噪声带内**——第 7 轮三次运行 10k 档为 9,562 / 9,643 / 10,421 µs，与第 6 轮单次 8,943 µs 无法区分（单次对照不足以判定）。因此本轮**不主张提速**：可靠结论是"分配次数减少约 82%"，这对分配器压力与帧时间抖动有价值，但不能宣称帧时间因此变快。若要判定 >5% 的帧时间变化，需要按 docs/40 §9.2 做固定硬件下的 30 次重复对照。

## 第三刀：UI 布局解算不再遍历全部实体（2026-09-17，最大一刀）

前两刀后仍剩 6,726 次分配/帧，其中"步骤管线"3,376、"提取"3,350。分段探针（在 `vertices()` 内按阶段早退）把提取侧定位到 **UI 合批段**：`A/B/D/E` 各阶段只有 2–11 次分配，到 `G`（UI 段之后）跳到 **3,350**。

根因不在 `vertices()` 本身，而在 `build_ui_scene_nodes`：它为**场景里每一个实体**都建了一个布局节点（纯精灵实体也建），于是 `ensure_scene_ui_layout` 每帧对全部节点跑一次 `resolve_scene_ui_layout`（构造整张 BTreeMap 再 `retain`）。而它每帧被调用**两次**——一次经 `compute_click_targets`（Script 阶段，即上面那 3,376），一次经 `push_scene_ui`（3,350）。两处都是 O(总实体数)。

改法：只为**参与场景 UI 的实体**建节点——带 UI 组件的实体，加上它们的**祖先**（解析父链用）与**后代**（容器的子节点自身可能没有 UI 组件却被容器摆放）。键的编号方式保持不变（仍按场景遍历序 = ECS 槽位序），因此 `ui_layout_rects` 的键与 ECS 实体索引的对应关系不变；`ensure_scene_ui_layout` 的 `retain` 本来就把无 layout/container 的节点剔除，所以行为不变。

### 累计实测（Release，30 cycles）

| Sprite | 分配次数/帧（原始 → 第2刀 → 本轮） | 帧 p50（原始 → 本轮，三次运行） |
|---:|---|---|
| 1,000 | 3,736 → 727 → **57（−98.5%）** | 1,013 → 364 / 374 / 368 µs（**−64%**） |
| 10,000 | 36,739 → 6,726 → **60（−99.8%）** | 11,534 → 3,285 / 3,246 / 3,410 µs（**−71%**） |
| 50,000 | 183,407 → 33,392 → **62（−99.97%）** | 65,330 → 24,554 / 25,293 / 26,632 µs（**−61%**） |

**分配次数现在与精灵规模无关**（57 / 60 / 62），说明 2D 帧内已基本零分配。拆分：10k 档 60 = 步骤管线 43 + `vertices()` 17 + `scene_3d()` 0。

**这次的时间收益是可信的**：与第 2 刀不同，三次重复运行的帧时间落在 ±5% 内，且相对基线的降幅达 61–71%，远超测量噪声。

意义：10k Sprite 的 CPU 提取从**占 60fps 预算的 69%（11.5 ms）降到 20%（3.3 ms）**。

## 第五刀：粒子批次不再整数组拷贝（2026-09-17）

`push_particles` 先把整条粒子数组 `to_vec()` 拷一份再遍历：`with_particles(|particles| particles.to_vec())`，10k 粒子每帧白拷 840 KB（`Particle` 84 B）。改为字段级拆分借用后直接在 `with_particles` 内部把四边形写进 `sprite_batch`（与第一刀的精灵、第四刀的 Tilemap 同一手法），纹理注册复用同一条 `texture_id_of`。

实测（Release 30 cycles，`wp0-v4` 粒子档）：每帧分配字节 **1k 571 → 476 KB、10k 5.29 → 4.24 MB**（差额恰好 84 B × 粒子数），分配次数 59 → 58；帧 p50 1,782 → 1,878 µs 落在噪声内，**不记作提速**。

## 未完成项 / 已知限制

- **提取侧"每精灵分配"已定位并消除**（`sorting_layer_rank` 的 `to_ascii_lowercase`）；**UI 布局解算的全量遍历也已消除**（`build_ui_scene_nodes`），这是最大的一处。
- 步骤管线侧现为 43 次分配/帧（与实体数无关）——不再是可优化项。
- 第 2 刀的帧时间收益未被证明（落在噪声带内）；第 3 刀的收益经三次重复验证，可信。
- **World2D / Legacy2DOverlay / ScreenUI 分层、`RenderSnapshot` 尚未开始**，仍等 WP4/WP6 范围裁定。
- 3D 路径未改动；`wp0-3d-golden` 与 3D 阶梯在 `-Benchmarks` 门中通过（无 3D 回退）。
- `scene_3d()` 每帧克隆 mesh 路径（3D 侧同类问题）尚未处理。
- 粒子路径已去拷贝（第五刀），但粒子每帧仍物化 `SpriteVertex` + `OverlayVertex` 两份顶点，属下面这条结构性问题的受益方。

## 第六刀：2D 顶点单趟化（已完成）

**做法**：核心新增 `SpriteBatch::build_overlay_into`，对输出顶点类型泛型化——核心只算
NDC 位置/UV/颜色，布局由宿主的 `OverlayVertex` 通过 `OverlayVertexSink` trait 决定，因此
`core` 仍不知道宿主顶点长什么样（分层架构测试继续通过）。瓦片与精灵的顶点直接追加进同一个
最终缓冲，draw range 由各段按当前缓冲长度自行偏移，`merge_sprite_meshes` 与 `sprite_vertex`
删除。屏幕像素 → NDC 只留 `core::sprite_batch::ndc_from_screen` 一份实现。

**实测（`wp0-v4`，每帧分配字节逐字节稳定）：**

| 阶梯 | 改前 | 改后 | 降幅 |
|---|---:|---:|---:|
| Sprite 1k / 10k / 50k | 495,743 / 4,529,663 / 22,456,831 B | 303,743 / 2,609,663 / 12,858,975 B | −39% / −42% / −43% |
| Tilemap 32×32 / 100×100 | 466,238 / 4,128,446 B | 269,630 / 2,208,446 B | −42% / −47% |
| 粒子 1k / 10k | 487,609 / 4,247,609 B | 295,609 / 2,527,609 B | −39% / −40% |

**踩坑记录**：第一版没预留容量，追加式写入的 `Vec` 翻倍增长把省下的又还了回去（10k 精灵
4.53 → 7.53 MB）。加 `vertices.reserve(sprite_batch.len() * 6)` 后落到 2.61 MB。**凡是从空
`Vec` 开始追加，先 `reserve` 再写**——这与 Tilemap 那次的增长 churn 是同一个坑。

**不主张帧时间收益**：本入口跨运行噪声 ±25%，50k 档 24,771 → 19,971 µs 中约 4.3 ms 来自 WP8
并行合批（同二进制 `--sprite-parallelism off` 24,283 µs vs `on` 19,971 µs，分配字节相同）。

**行为不变的证据**：2D overlay 语义 golden 改动前后逐项相同（顶点 60、draw 3、泄漏 0、
红块中心 `(255,0,0)`、区域 4096/1024/8192、瓦片行序签名全等）。

## 剩余项

- 10k 精灵的 2.6 MB 已是"一份顶点（60,000 × 36 B ≈ 2.16 MB）+ 脚本/UI 固定开销"，要再降
  得改顶点格式本身（例如压 UV/颜色精度，或改成实例化 quad + 每实例数据）。
- `scene_3d()` 每帧克隆 mesh 路径（3D 侧同类问题）仍未处理；3D 数量级 ≤13 mesh，优先级低。

## 第七刀：视口外精灵剔除（CPU 可见性）

瓦片剔除（见 [WP4 任务文件](WP4-2D核心能力重构.md) 与 docs/53 §3.9）之后发现精灵路径同样的问题：
10k 精灵场景中大部分精灵在视口外，每帧照样算四边形。

判定 = 屏幕中心 + **外接圆半径**（世界半对角线 × zoom ⇒ 与旋转无关，旋转/负缩放/任意 pivot 都被
保守覆盖），中心走 `Camera2D::world_to_screen`（与 `Sprite::quad` 同一个投影）。过滤在**生成顶点
之前**完成，因此并行分块"每项固定 6 顶点"的前提与 draw range 的按项计数都不受影响。

实测（`wp0-v4` 的 `sprite_culling_ab`，同进程逐轮交替）：1 万精灵 **3,044 → 1,927 µs（−36.7%）**、
顶点 60,000 → 3,036（−95%）。帧阶梯：10k 3,288 → 1,946 µs、50k 19,971 → 13,009 µs。

像素等价证明扩成两个开关的四种组合（972 / 204 / 204 / 198 顶点），四个像素缓冲逐字节相同；场景里
特意放了一个**跨在右边缘**的精灵——它必须被保留，否则边缘像素变化会立刻判红。

## 下一步瓶颈：**收集阶段**（已量出，未修）

剔除之后 5 万精灵档的顶点只剩 3,036，帧仍是 13,009 µs。余下时间与视口内容无关，花在
**逐实体收集精灵并 push 进批处理**：`world.for_each::<Sprite>` 走全部实体 + 5 万次 `SpriteBatchItem`
push（每项 80 字节 ⇒ 每帧约 4 MB 写入）。

下一刀的方向（按 docs/40 §3 WP6"层级 bounds 与轻量空间索引"）：

1. 让 `SpriteBatch` 直接接收**世界 AABB**，在收集阶段就按视口矩形跳过（省下 push 与后续排序）；
2. 更彻底：按空间索引（或场景构建期分块）只遍历可能可见的实体——但这需要一个插入/删除时维护的
   索引结构，属于 WP6 的可见性系统范围，与 WP3 的 ECS 存储形态也有耦合。

先做 1（改动局限于批处理与收集循环，收益可直接用同一把量尺对比）。

## 第八刀：收集阶段剔除 + 消掉每个精灵一次 `get`

第七刀之后量到"5 万精灵档顶点只剩 3,036 却仍要 13,009 µs"，于是转向收集阶段：

1. `SpriteBatch::set_cull_bounds`：收集阶段就按相机可见域（世界 AABB，
   `Camera2D::visible_world_bounds`）丢掉不可能可见的精灵——省下每项 80 字节写入与随后对全部项的排序。
2. `for_each::<Sprite>` + `get::<Transform>(id)` → `for_each2::<Transform, Sprite>`：少一次
   `World::get` = 少**两层哈希查询**（`TypeId` 表 + 实体表）。语义不变（同为排序实体序；
   无 `Transform` 的精灵本来就被跳过且不推进默认层序号）。

实测：1k 267 → 208 µs、10k 1,946 → **1,612 µs**、50k 13,009 → **9,688 µs**（同轮 21,407 → 9,688，−55%）。

**这把热点归因到了 ECS 组件查询**：下一刀不再是渲染管线的事，而是 `World` 的存储形态
（WP3 决策，dense 乱序 `get` 实测快 3.6–4.3×）。证据已补进 `tasks/plan/WP3-ECS热存储.md`。

## 一条需要与本节一起裁定的 2D 空间约定

2D 世界是 **Y 向上**，而瓦片 `tiles` 数组按 `(y+0.5)*tile_size` 排布 ⇒ **第 0 行画在地图最下方**。
此约定此前无文档，现已被 `wp0-2d-golden-v1` 用像素固定（交换两行后颜色整体对调）。
**尚未核对编辑器 Tile 画笔的网格预览方向是否与运行时一致**——若不一致，就是"编辑器里看到的样子
和跑起来不一样"的真实缺陷，属于 WP4/WP6 的 2D 空间统一范围（docs/40 §3 WP4 第 ① 项
"明确 pivot/anchor/z/rotation/scale 语义"）。
