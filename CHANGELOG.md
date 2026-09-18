# Changelog

版本采用 git 标签（`git tag`）；本文只记录**能力增量**——做了什么、为什么重要、回滚档是什么。

度量数字（µs、分配字节、测试计数）、逐条证据、golden 断言与实施细节留在能力文档与任务文件里，不在本文重复：
重构过程见 [docs/53](docs/53-WP0增量场景帧基准与每帧分配.md) 与 [docs/40](docs/40-引擎全面重构计划.md)，已完成任务的验收记录见 `tasks/done/`。

## Unreleased — 2D/3D 统一最终收口（docs/54 §8 逐条达成）

主题：按 [docs/54](docs/54-2D3D统一优化方案与现状对比.md) §8「最终完成定义」的 10 条判据逐条收口，
每条都带可执行证据、可回滚开关与不变量测试。进度对照表见 docs/54 §10。

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
