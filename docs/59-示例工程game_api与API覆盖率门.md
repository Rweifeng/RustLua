# 示例工程 game_api 与 API 覆盖率门

> **已被 [docs/74 game_api 2.0](74-game_api演示工程2.0.md) 取代（2026-10-24）。**
> 本文记录的是 1.0 形态（40/270 函数、21 个组件、一个模块一组按键），现状已改为分步 Lab
> （42/301、23 个组件）。**仍然有效**的部分：覆盖率门的判定语义、fixture 场景约束、
> 共享取景框与渲染顺序约束——2.0 原样保留并继续引用。

本文件记录 `examples/game_api` **按当前 API 完全推倒重建**的结果：它演示什么、怎么组织、
哪些场景是固定夹具、覆盖到什么程度、以及这些结论各自由哪条自动化门钉住。

前置阅读：[docs/18 Lua API](18-lua-api.md)（API 语义）、
[docs/16 场景格式](16-scene-format.md)（组件串与场景文件）、
[`examples/game_api/README.md`](../examples/game_api/README.md)（演示模块的写法与硬规则）。

## 1. 一句话状态

`examples/game_api` 从"一个能力一个场景的展示工程"改成 **"每个 Lua API 都有一处可运行演示"的
活文档**：42 个模块表 / 313 个 Lua 函数、30 个运行时组件，全部有演示；演示能否跑起来由
`host::sample_project` 的 12 条门在 `cargo test -p engine` 里自动验证。

旧的玩法型场景（`arena` / `production` / `menu` / `game3d` / `ui_layout` / `tilemap` /
`effects` / `stress` / `resources` / `persistence` / `data_inventory` / `gameplay_api` /
`ui_dynamic` / `physics3d`）已删除：**玩法样板继续由 `examples/game_jhyt` 承担**，
`game_api` 只做 API 演示，两个工程的定位因此不再重叠。

## 2. 场景 ↔ 模块映射

入口菜单 `scenes/index.scene` 的每张卡片就是一个演示；卡片实体名 = 目标场景名，
因此点击回调可以直接 `scene.load(名称)`。清单真源是 `scripts/demos.lua`。

| 场景 | 模块 | 覆盖的 API 模块 |
|---|---|---|
| `main` | `demo_main` | `transform` / `sprite` / `quad` / `animation` / `particles` / `world_text` / `visibility` / `physics`（2D 全部） |
| `unified` | `demo_unified` | `camera` / `mesh` / `light`：一份场景同时表达 Sprite + Quad + Mesh + 两种投影相机 + 粒子 + 世界文字 + 屏幕 UI，脚本在两种投影间切换 |
| `api_core` | `demo_core` | `log` / `time` / `entity` / `event` / `tag` / `health` / `asset` / `scene` / `save`（全部 9 个）/ `app` / `data` |
| `api_gameplay` | `demo_gameplay` | `clock` / `calendar` / `day` / `interact` / `item` / `inv`（15 个）/ `container` / `shop` / `map`（含 `map["goto"]`） |
| `api_input` | `demo_input` | `input` 全部 35 个（物理键 / 命名动作 / 手柄 / 绑定持久化 / 导航手感） |
| `api_audio` | `demo_audio` | `audio` 全部 14 个（总线 / 播放 / 设置文件） |
| `api_ui` | `demo_ui` | `ui` 全部 43 个（面板 / 图片 / 网格 / tooltip / modal / 拖拽 / 焦点环 / 滚动 / 度量 / 读回） |
| `ui_test` | `demo_ui_events` | 场景侧 UI 组件与事件：`Text`/`Image`（含 `nine_slice`）/`UiLayout`/`Container`（五种 kind）/`UiEvents`（九种事件）/`ButtonStates` |
| `api_camera` | `demo_camera` | `camera2d`（10）/ `camera3d`（5） |
| `api_3d` | `demo_3d` | `transform3d` / `animation3d`（骨骼动画，运行期列出 95 条剪辑）/ `physics3d`（9） |
| `api_tilemap` | `demo_tilemap` | `tilemap`（10，含运行期 `create` 与 A* `find_path`）/ `grid`（4） |
| `api_script` | `demo_script` | 每实体 `Script` 组件的 `on_spawn` / `on_update` + `entity` 稳定 ID（`persist_id` / `find_id`） |
| `api_prefab` | `demo_prefab` | 预制体实例在**运行期就是普通实体**（含两个 Prefab 源的真实实例） |
| `demo3d` / `mixed` | `demo_fixture` | **渲染 golden 的固定夹具**，不是演示（见 §4） |

## 3. 工程结构

```text
examples/game_api/
├── project.toml          # main_script=scripts/main.lua, startup_scene=scenes/index.scene
├── README.md             # 演示模块的写法与硬规则（改演示前先读它）
├── scenes/               # 见 §2 的表；index.scene 由生成器产出
├── scripts/
│   ├── main.lua          # 工程级回调 → router（只转发，不含逻辑）
│   ├── router.lua        # 场景 → 模块分发、ESC 返回、共享取景框渲染
│   ├── demos.lua         # 演示清单（场景 ↔ 模块 ↔ 菜单文案）
│   ├── api_demo.lua      # 取景框 + `api.try`（宿主帧依赖调用的安全包装）
│   ├── demo_*.lua        # 每个场景一个演示模块
│   └── entity/patrol.lua # 每实体脚本钩子演示
├── assets/ data/         # 纹理 / 音频 / 模型 / 预制体 / 字体 / JSON 数据
└── (生成物) scenes/index.scene、assets/fonts/*.ttf 子集
```

两条**生成物**规则（改动后要重跑工具，否则门会红）：

| 生成物 | 生成器 | 说明 |
|---|---|---|
| `scenes/index.scene` | `scripts/docgen/gen-api-showcase-index.ps1` | 从 `demos.lua` 读清单，铺成 2 列卡片网格；手改该文件会在下次生成时丢失 |
| `assets/fonts/NotoSansSC-VF.ttf` | `cargo run -q -p engine --example subset_project_font -- examples/game_api` | 按**工程实际用字**子集化（场景文本 + Lua 字符串字面量）。文案里新增了汉字就要重跑，否则屏幕上出现方框（有门钉住） |

## 4. 固定的渲染夹具：`demo3d` 与 `mixed`

这两个场景**内容不变**（连坐标都不改），因为它们是两条渲染 golden 的判据来源：

| 场景 | 钉住它的证据 |
|---|---|
| `demo3d.scene` | `engine --example wp0_3d_golden`：3 个 drawable（Ground / ShadowTriangle / SkeletonWarrior）、相机位姿、`draw_calls` / `instances` / `triangles` / 非背景像素比例 |
| `mixed.scene` | `engine --example wp0_mixed_golden`：在**同一个世界视图**里网格墙与三个精灵按深度互相遮挡（前精灵可见、后精灵被墙完全遮住、半透明面纱叠出第三种像素） |

它们的价值恰恰在于"内容一像素不变"，所以没有拆掉重建——`scripts/demo_fixture.lua`
只负责把"这是夹具、不是演示"显示出来。要改这两个场景，必须同时更新对应 golden 的断言与
基线报告（`scripts/verify/verify.ps1` 会校验报告字段）。

## 5. 自动化门：`host::sample_project`

全部在 `cargo test -p engine` 里跑（`engine/crates/engine/src/host/sample_project.rs`）。

| 门 | 钉住的漂移 |
|---|---|
| `sample_project_runs_every_scene_headless` | 场景文件从 `scenes/` **目录读**（不再手抄名单）：逐个加载 + 实例化 + 步进 3 步，任何 Lua 报错直接判红 |
| `sample_project_scripts_all_compile` | `scripts/**/*.lua` 全部能编译（只编译不执行，秒级） |
| `sample_project_demos_cover_every_lua_api` | **API 覆盖率**：运行期导出 `_G` 里的模块表与函数名（40 表 / 270 个），逐个要求在演示脚本里找到真实调用点 |
| `sample_project_scenes_cover_every_runtime_component` | **组件覆盖率**：`ComponentId::ALL`（30 个运行时组件）每个都要在某个场景里出现 |
| `sample_project_index_menu_lists_every_scene` | `demos.lua` ↔ `scenes/*.scene` ↔ 入口菜单卡片三方一致 |
| `sample_project_index_click_opens_the_demo` | 逐个走"点击卡片 → `scene.load` → 过渡 → 新场景的演示模块 `on_create` + 一帧 `update`" |
| `sample_project_index_cards_are_clickable` | 卡片矩形的真实解算结果（`scene_entity_ui_layout_rects`，与渲染同源）+ 在中心点按左键真的能切场景 |
| `sample_project_shared_overlay_is_not_covered` | 场景 UI 不得大面积压住共享取景框（见 §6） |
| `sample_project_scene_text_has_every_glyph` | 场景里的 `Text`/`WorldText` 在工程字体下**没有缺字**（`ui.measure_text` 与渲染同口径） |
| `sample_project_gameplay_scene_instantiates_gameplay_entities` | 玩法场景真的带出地块/演员/世界文字并渲染 |
| `sample_project_ui_scene_instantiates_ui` | 场景 UI 场景真的产出 UI draw |
| `core::script::tests::sample_project_*`（3 条） | 入口菜单跑得起来、点卡片能切场景、按 S 存档真的落盘、按 ESC 真的回菜单 |

覆盖率的**事实源是运行期**，不是任何文档或手抄清单：引擎新注册一个 Lua 函数，
下次跑测试就会要求示例工程补上演示。三种"算不算被演示"的写法都有约定：

- `ui.panel(...)` 直接调用 → 算；
- `api.try(camera2d.get, ...)` / `pcall(light.set, ...)`（把函数本身交给受控调用）→ 算；
- `map["goto"](...)`（`goto` 是 Lua 保留字，`map.goto(` 语法上写不出来）→ 算；
- 只出现在注释或屏显文案里的名字 → **不算**（门会剥掉注释与字符串再比对）。

## 6. 共享取景框与"场景 UI 在上面"这条硬约束

演示模块共用 `scripts/api_demo.lua` 的三行取景框：标题（左上）、状态行、按键提示。
渲染顺序是 `runtime_scene.rs` 里定死的：

```text
世界精灵 / 瓦片 / 网格 → collider 调试 → 世界文字
  → Lua 动态 UI：面板 → 图片 → 网格 → 条 → 文本
    → **场景实体的 Text / Image**（永远在最上面）
```

由此得到两条必须在写场景时遵守的结论：

1. **铺满屏幕的背景不能是场景 `Image` 实体**：它会把脚本的读数整片盖住。背景要用
   `Quad`（世界通道）或脚本里的 `ui.panel`（动态面板在动态文本之下、世界内容之上）。
   这条由 `sample_project_shared_overlay_is_not_covered` 兜底（只抓大面积覆盖，
   小控件贴边不会被误伤）。
2. **状态行与提示行按行数从屏幕底部往上排**（`api_demo.render`），因此多行状态不会压住提示行；
   演示自己画的说明行请留在 y ≤ 600。

## 7. 两条示例工程专属的引擎约束（写场景时会被门拦住）

1. **父子关系只在"父链零平移"时可用**。`scene_migration` 的
   `shipped_example_scenes_render_the_same_under_both_parent_semantics` 会对两个示例工程的
   每个场景逐实体断言"`parent` 作组织关系"与"`parent` 作局部空间"两种解释下世界位置相同，
   它**不读 `meta spatial`**。因此示例场景里的父子对都把父放在 (0,0,0)（父子同位置），
   要演示"子跟着父走"就在运行期用 `transform.move` 移动父。
2. **不是每个组件都能写进场景**。`typed_scene` 只认识 `ComponentId::ALL` 里的 21 个组件，
   而 `compiles_every_project_scene_and_matches_serialized_extraction` 会断言"源组件数 ==
   typed 组件数"，所以下列组件的差异会被判红：
   - `Visibility`：**运行时没有 typed 分支**，场景里写了会导致数量不一致 → 只能由 Lua
     `visibility.set` 在运行期挂上（`main.scene` 就是这么演示的）；
   - `UiBindings` / `UiAnimation`：只有 schema（编辑器 UI Designer 侧），运行时不消费；
   - `ButtonStates`：运行时零消费方（编辑器读），写进场景只是"给编辑器看的五态配置"。

## 8. 重建过程中发现并修掉的真问题

这些都不是"写完就算"，而是被门或实测抓出来的：

| 问题 | 症状 | 修法 |
|---|---|---|
| 入口菜单的 `Backdrop` 是铺满屏幕的场景 `Image` | 脚本的标题/状态/提示行与焦点框全被一张图盖住（脚本照跑、屏幕全黑） | 背景改由 `demo_index.lua` 的 `ui.panel` 画；新增门 `sample_project_shared_overlay_is_not_covered` 兜底 |
| 演示模块用 `api.try(light.set, …)` 传函数引用 | 文本覆盖率门把它算成"没演示" | 门同时接受"函数引用"与 `map["goto"]` 的索引写法；README 写明三种合法写法 |
| 工程字体是**旧文案**的子集（242KB） | 新的中文文案在屏幕上缺字（豆腐块） | 按实际用字重新子集化（76KB），并新增 `sample_project_scene_text_has_every_glyph` |
| 场景 `Text` 的 `text=` 里出现裸 `;` | 组件串被切坏 → 整场景实例化失败（`malformed UI field`） | 示例工程文案避开裸分号；运行期错误由第一条门直接抓到 |
| `world_text.attach` 传了"人话"而不是组件串 | 每实体脚本 `on_update` 抛 `malformed WorldText field` | 改成 `text=…;font_size=…;color=…` 的合法串（门当场抓到） |
| 若干测试把示例场景的**实体数量/名字/剪辑名**写死 | 场景一重建就假失败 | 改成运行期从文档里取（数量、首个带 `Animation` 的实体、第一个根实体…） |

## 9. 复现命令

```powershell
cd engine

# API 覆盖率 + 逐场景 headless + 菜单接线 + 组件覆盖率（12 条门一次跑完）
cargo test -p engine --lib sample_project

# 只看覆盖率缺什么
cargo test -p engine --lib sample_project_demos_cover_every_lua_api -- --nocapture

# 全部 Lua 脚本语法
cargo run -q -p engine --example lua_check -- ../examples/game_api (Get-ChildItem ..\examples\game_api\scripts\*.lua -Recurse | ForEach-Object FullName)

# 场景 schema 与组件漂移（要求 schema_errors=0 unknown_components=0）
cargo run -q -p editor -- --project ../examples/game_api --check

# 渲染夹具的两条 golden
cargo run --release -p engine --example wp0_3d_golden -- --output ..\artifacts\wp0-3d-golden.json
cargo run --release -p engine --example wp0_mixed_golden -- --output ..\artifacts\wp0-mixed-golden.json
```

## 10. 仍然存在的边界

- **覆盖面是"调用过一次"级别的**：门证明每个 API 都被真实调用，但不证明每个分支/错误路径
  都被演示（例如 `audio` 无设备时的降级、手柄震动需要有真实设备）。
- **像素级外观没有门**：只有 `demo3d` / `mixed` 两条 golden 有像素证据；其它场景的目标是
  "读数看得见"，不是"像素正确"。
- **`main.scene` 里没有真正的空间父子演示**（见 §7 第 1 条）：要演示就得放宽那条引擎门，
  本轮选择保留门、把"子跟父走"放进运行期用 `transform.move` 演示。
- **`camera2d.shake` 的抖动偏移是逐帧累加在相机中心上的**（衰减结束后相机停在被推开的位置，
  不回到抖动前的中心）。演示脚本里如实记录了这个现象，引擎侧未改动——它超出"重建示例工程"的范围。
