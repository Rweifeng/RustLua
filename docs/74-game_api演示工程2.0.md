# game_api 演示工程 2.0：分步 Lab、行为自检与生成式目录

> 状态：方案定稿并实施（2026-10-24）。取代 [docs/59](59-示例工程game_api与API覆盖率门.md)
> 中"一个模块一组按键、状态行刷读数"的 1.0 形态；覆盖率底线、fixture 场景与既有门全部保留。
>
> 基线读数（2026-10-24，本机）：
> - 运行期 Lua API 面 **42 个模块表 / 313 个函数**（`engine/crates/rustlua-scripting/src/api_surface_snapshot/lua_api_surface.json`）；
> - 运行时组件 **23 个**（`ComponentId::ALL`，P3 后新增 `Environment` / `LayoutItem`）；
> - `examples/game_api` 改造前 **15 个场景 / 103 个文件 / 6.6 MB**；
> - `cargo test -p engine --lib sample_project` **11 条门全绿（0.85s）**——这是改造前必须守住的底线。

## 1. 为什么要 2.0

1.0 工程解决的是"**有没有**"：每个 API 都被真实调用一次，覆盖率由运行期 `_G` 探针钉住。
它已经追不上当前引擎的三处变化：

| 变化 | 读数 | 1.0 的问题 |
|---|---|---|
| API 面继续增长 | 40/270 → **42/301**（P5 `calendar.configure`、M74 空间音频、M72 tilemap atlas/autotile/tint、P3 `.anim` 播放器、prefab 节点操作等） | 演示靠"模块组 + 状态行"平铺，新增函数只补一行调用，读者看不出它解决什么问题 |
| 渲染能力跨档 | P3 交付 PBR / HDR / IBL / `.material` / Environment / GPU 粒子 / compute 接缝 | 3D 场景仍以三角形、战士、物理球为主，**新能力没有一张能看的画面** |
| 演示标准提高 | 编辑器、AI 面板、UI 体系已按"可操作 + 可回归"验收 | 演示仍是"按 T/Y/U/I/O/P/V/J/K/L/C 看左下角一串数字"，没有步骤、没有断言、没有失败反馈 |

1.0 还有三处可被门直接证明的欠账：

- 入口菜单里的 API 表清单是**手写 41 项**，而运行期是 42 个模块（漏了 `prefab`）；
- `api_ui.lua` 文件头写着"44 个函数"，实际已是 45 个；
- 文档、清单、状态行里的数字与运行期事实没有单向真源，只能靠人记。

2.0 的目标不是"再补几个场景"，而是把演示工程变成**可读、可操作、可自证**的引擎活文档。

## 2. 五条标准（每条都有门或可复现命令）

| # | 标准 | 落地方式 | 怎么证明 |
|---|---|---|---|
| 1 | **一步一个可验证主张** | 每个演示由有序步骤组成；每步声明 `claim`（人话）、`api`/`code`（调了什么）、`read`（读回什么）、`run`（真实调用）、`check`（断言） | `__api_demo_self_test()` 逐场景跑完所有步骤的 `check` |
| 2 | **画面优先，读数克制** | 左侧步骤轨、中间舞台、右侧 API 卡；一次只展示当前步骤的 3–5 个关键读数 | 新 shell 几何由固定布局给出；场景文案缺字由既有字形门兜底 |
| 3 | **可操作、可复位** | `Space/→` 下一步、`←/Backspace` 上一步、`R` 复位当前步、`F` 聚焦模式、`Esc` 回目录；键盘/手柄走同一套命名动作 | 菜单卡片命中门 + 自检门 |
| 4 | **覆盖率不回退** | 每个函数仍要求在演示脚本里有真实调用点 | 既有 `sample_project_demos_cover_every_lua_api`（42/313） |
| 5 | **目录、文档、数字都有单一真源** | 目录从 `demos.lua` 生成；API 索引从运行期 `_G` 枚举；README 规则写死"改哪里、跑哪条门" | 入口菜单三方一致门 + 本文复现命令 |

## 3. 结构与交互

```text
examples/game_api/
├── scenes/                 # 每个领域一个 Lab 场景 + 2 个渲染 fixture
├── scripts/
│   ├── lab.lua             # 2.0 框架：主题 / 分栏 / 步骤引擎 / 断言 / API 卡
│   ├── demos.lua           # 目录真源：场景 / 分类 / 摘要 / 覆盖模块 / 步骤数
│   ├── router.lua          # 生命周期分发 + 自检入口
│   ├── main.lua            # 工程回调 → router；暴露 __api_demo_self_test
│   ├── demo_*.lua          # 每个场景一个模块（顶层只定义函数与常量）
│   └── entity/*.lua        # 每实体 Script 钩子
├── assets/                 # 纹理 / 音频 / 模型 / 材质 / 环境 / 字体
└── data/                   # data.load / schema 的演示数据
```

### 3.1 屏幕分栏（1280×720 设计基准）

```text
┌──────────────────────────────────────────────────────────────┐
│ 标题 / 分类 / 进度                                    F 聚焦 │  0..64
├────────────┬────────────────────────────┬────────────────────┤
│ 步骤轨     │  舞台：世界 / 场景 UI       │ API 卡             │  64..656
│ 01 ... 12  │  （当前步骤的可视结果）     │ 调用名 / 代码 / 读数│
├────────────┴────────────────────────────┴────────────────────┤
│ 键位提示 / 当前步骤结论 / 自检状态                            │  656..720
└──────────────────────────────────────────────────────────────┘
```

- 舞台区保留给世界内容；左右栏是半透明深色面板，不占世界坐标。
- `F` 进入聚焦模式：隐藏左右栏与页眉页脚，只留一行浮动说明，用于看画面。
- 场景里原有的屏幕 `Text` 一律移进舞台或删除：场景文字画在动态 UI **之上**，
  留在页眉位置会盖住 shell（1.0 已经踩过同类问题，见 [docs/59](59-示例工程game_api与API覆盖率门.md) §6）。

### 3.2 步骤模型

```lua
local step = {
    id = "physics_raycast",
    claim = "raycast 返回最近的碰撞体，而不是沿途所有",
    api = { "physics.raycast", "physics.overlap" },
    code = "physics.raycast(x, y, dx, dy, max)",
    enter = function() ... end,      -- 进入步骤时布景
    run = function(dt) ... end,      -- 每帧（或按键）推进
    read = function() ... end,       -- 右侧 API 卡的读数
    check = function(t) ... end,     -- 自检断言：t:eq / t:near / t:ok
}
```

- `lab.tick` 负责导航、绘制、把 `read()` 的键值对排进 API 卡；
- `check(t)` 用 `t:eq / t:near / t:ok` 收集断言，一处失败不会中断同一步的其余断言；
- 步骤的 `run` 必须能在 headless 下跑：不写盘、不无条件切场景、宿主帧依赖调用用 `lab.try`。

### 3.3 自检契约

`main.lua` 暴露全局 `__api_demo_self_test()`；`router.lua` 把它转给当前演示模块的
`M.self_test()`。框架对所有步骤依次执行 `enter` → `run` × 3 → `check`，最后恢复原步骤并重跑它的
`enter`，返回：

```text
ok steps=<N> checks=<M> failed=0
FAIL steps=<N> checks=<M> failed=<K> | <step id>: <message> | ...
```

新增门 `sample_project_demos_self_test` 在 `cargo test -p engine` 里对每个场景：

1. 实例化并步进 1 帧（确保 `on_create` 已跑）；
2. 调 `__api_demo_self_test()`；
3. 要求结果以 `ok ` 开头、`failed=0`，且非 fixture 场景 `steps>=3`、`checks>=3`。

它钉住的漂移是 1.0 完全抓不到的一类：API 还能被调用、场景还能跑，但**读回语义已经错了**
（例如 `transform.set` 没生效、`inv.add` 不计数、`calendar.configure` 没改上界）。

## 4. 场景与领域

目录是 `scripts/demos.lua` 的唯一真源。分类只影响首页分组，不影响覆盖率。

| 分类 | 场景 | 覆盖模块 | 本轮重点 |
|---|---|---|---|
| 起点 | `index` | `ui` / `input` / `scene` / `entity` | 3 列卡片、运行期 API 索引、焦点与滚动 |
| 2D | `main` | `transform` / `sprite` / `quad` / `animation` / `particles` / `world_text` / `visibility` / `physics` / `camera2d` | 分步演示局部/世界、换装、动画事件、物理查询与调试绘制 |
| 2D | `api_tilemap` | `tilemap` / `grid` | atlas / autotile / tint / meta / A* 可视化 |
| 2D | `api_camera` | `camera2d` / `camera3d` / `camera` | 跟随时序、缩放、抖动、拾取与射线命中 |
| 2D | `unified` | `camera` / `mesh` / `light` | 同一世界里 2D + 3D + UI 的类别对照 |
| 3D | `api_3d` | `transform3d` / `animation3d` / `physics3d` / `mesh` / `light` / `camera` | PBR 网格与 IBL 环境、骨骼剪辑浏览、CCD 对照 |
| 玩法 | `api_gameplay` | `clock` / `calendar` / `day` / `item` / `inv` / `container` / `shop` / `map` / `interact` / `tag` | 一天的循环、背包/装备、商店、分地图状态 |
| 输入 | `api_input` | `input`（39） | 键鼠/手柄仪表、动作映射、重绑定与诊断 |
| 音频 | `api_audio` | `audio`（17） | 四路混音、空间声源、设置文本往返 |
| UI | `api_ui` | `ui`（45） | 面板/文本/图片/网格/模态/拖拽/焦点/滚动 |
| UI | `ui_test` | 场景 UI 组件 | Image/Text/UiLayout/Container/UiEvents/ButtonStates |
| 脚本 | `api_script` | 每实体 `Script` | `on_spawn` / `on_update` / 稳定 ID |
| 脚本 | `api_prefab` | `prefab`（7） | spawn/despawn/node/set_rect/set_text/set_visible |
| 核心 | `api_core` | `log` / `time` / `entity` / `event` / `tag` / `health` / `asset` / `scene` / `save` / `app` / `data` | 生命周期、事件总线、存档、数据 schema |
| fixture | `demo3d` / `mixed` | — | 内容一像素不变，只做最小说明；由两条渲染 golden 钉住 |

## 5. 当前引擎能力在演示里的落点

| 能力 | 演示位置 | 读者能看到什么 |
|---|---|---|
| PBR 网格 + 光照 | `api_3d` 的 PBR / IBL 步骤 | 场景里的 PBR 网格共用方向光与 `StudioEnvironment`；环境贴图强度、太阳方向与网格材质由场景数据给出 |
| HDR + Tonemap | `api_3d` / fixture | 环境贴图与 PBR 网格同框；曝光链走引擎默认，fixture 仍由既有 golden 钉住 |
| IBL / Environment | `api_3d` 的 `StudioEnvironment` 步骤 | 场景加载即生效的 `Environment`（map / intensity / rotation）；步骤用 `entity.find` 读回环境实体 |
| 3D 空间音频（M74-01） | `api_audio` 的空间步骤 | 声源移动时读回 `gain/pan`，听者位姿同时显示 |
| 图集 / autotile / tint（M72） | `api_tilemap` | 同一格切换图集索引、autotile mask、逐格 tint |
| 动画时间线播放器（P3-E01） | `main` / `api_3d` | 播放/暂停/seek 与 `player_events` 的读回 |
| Prefab 节点操作 | `api_prefab` | UI prefab 的 `set_rect/set_text/set_visible` 实时生效 |
| 输入重绑定与诊断 | `api_input` | `bindings` 表、`migration_notes`、保存/载入往返 |
| 日历体裁参数化（P5-02） | `api_gameplay` | `calendar.configure` 改 7×3 后第 8 天被拒，再恢复 28×4 |

## 6. 门矩阵

| 门 | 位置 | 覆盖 |
|---|---|---|
| 逐场景 headless 实例化 + 步进 | `sample_project_runs_every_scene_headless` | 场景/脚本在无输入下能跑 |
| 脚本全量编译 | `sample_project_scripts_all_compile` | 语法与 `require` |
| Lua API 覆盖 | `sample_project_demos_cover_every_lua_api` | 42/313 每个函数都有真实调用点 |
| 组件覆盖 | `sample_project_scenes_cover_every_runtime_component` | 30 个运行时组件 |
| 目录三方一致 | `sample_project_index_menu_lists_every_scene` | `demos.lua` ↔ `scenes/*.scene` ↔ 卡片 |
| 卡片可点 | `sample_project_index_cards_are_clickable` | 解算矩形 + 真实点击切场景 |
| 点击真的切场景 | `sample_project_index_click_opens_the_demo` | 点卡片后 `scene.reload()` 读到目标场景 |
| 共享区不被遮挡 | `sample_project_shared_overlay_is_not_covered` | 场景 UI 不压住 shell 读数带 |
| 文案不缺字 | `sample_project_scene_text_has_every_glyph` | 工程字体子集 |
| **行为自检** | `sample_project_demos_self_test`（新增） | 每个非 fixture 场景 `steps>=3`、`checks>=3`、`failed=0` |
| 卡片文字在父矩形内 | `sample_project_index_labels_are_inside_their_cards` | 三行文字的锚点落在卡片里、自上而下、左对齐 |
| 卡片文字实测宽度 | `sample_project_index_labels_fit_their_cards` | `ui.measure_text` 量出的宽度不顶出卡片右缘（留 8 px） |
| 渲染 golden | `wp0_3d_golden` / `wp0_mixed_golden` | fixture 内容不变 |
| schema / 组件漂移 | `editor --project examples/game_api --check` | `schema_errors=0` / `unknown_components=0` |

### 6.1 为了让菜单"看得清"顺手修掉的两个引擎缺陷

`project.toml` 里 `text_render_mode = "vector"` 让全部文字走矢量轮廓档，两个只有这条通道
才有的缺陷因此暴露出来（MSDF 档一直是好的）：

1. **空格零宽**：空格没有轮廓，`FontVectorAtlas::glyph` 把它记成 missing，排版于是只加
   `style.spacing`。整句空格被压掉——屏幕上 `RustLua API Showcase` 画成 `RustLuaAPIShowcase`。
   修法是给矢量图集补一条只读 `advance_em`（读 hmtx，不光栅化），排版按真实字距推进。
2. **细笔画整字消失**：矢量档的 AA 在片元里做，但光栅化是 1x 采样。子集字体里 `i` 的墨迹
   在 13 px 下只有 0.86 px 宽，四边形可以一个采样中心都不落在里面——`Input` 画成 `nput`、
   `audio` 画成 `aud o`。修法是墨迹不足 1 px 时按墨迹中心补到 1 px 见方（≥1 px 的字形
   逐字节不变，所以锚点与包围盒不受影响）。

两条各挂一条单测（`rustlua-render` 的 `text::tests`），并且都跑过反面对照：把修复行注掉，
`vector_layout_advances_spaces` 与 `vector_quads_never_fall_below_one_pixel` 分别变红。

## 7. 实施顺序（本轮）

1. 设计文档与目录真源（本文 + `demos.lua`）；
2. Lab 框架与视觉主题（`lab.lua` + 九宫格贴图）；
3. 入口菜单（3 列卡片 + 运行期 API 索引）；
4. 按"2D → 3D → 玩法 → 输入/音频 → UI/脚本/核心"重写演示；
5. 自检门、索引生成器、字体子集、全部门复跑。

## 8. 诚实边界

- **不是每个分支都有演示**：自检证明的是每步声明的主路径与关键读回，错误路径只挑有契约价值的
  几条（如 `measure_text` 非法字号、`calendar.configure` 非法值、`find_path` 不可达）。
- **像素级外观仍只有两条 fixture golden**：其余场景的门是"读数正确、文字不缺、布局不互相遮挡"，
  不是"像素与设计稿一致"。
- **夹具场景内容不动**：`demo3d.scene` / `mixed.scene` 继续由既有 golden 钉住；2.0 只改它们的说明层。
- **平台差异不在本工程门内**：Web/微信的输入、存档、音频降级由平台侧门负责；本工程只把
  "无音频设备/无手柄/无渲染帧"的降级路径写成可读的失败状态。

## 9. 复现

```powershell
cd engine

# 工程门（12 条）
cargo test -p engine --lib sample_project

# 只看 API 覆盖缺什么
cargo test -p engine --lib sample_project_demos_cover_every_lua_api -- --nocapture

# 脚本语法
cargo run -q -p engine --example lua_check -- ../examples/game_api (Get-ChildItem ..\examples\game_api\scripts\*.lua -Recurse | ForEach-Object FullName)

# 场景 schema
cargo run -q -p editor -- --project ../examples/game_api --check

# 改文案后重生成入口场景与字体子集
powershell -NoProfile -ExecutionPolicy Bypass -File ../scripts/docgen/gen-api-showcase-index.ps1
cargo run -q -p engine --example subset_project_font -- ../examples/game_api --source ../engine/crates/editor/fonts/NotoSansSC-VF.ttf

# 版面目视证据：离屏渲染成 PNG（无窗口，CI 里也能跑），并打印每条 draw 的排版通道
# 第二个参数相对**当前目录**，第一个相对 examples/game_api；这里从 engine/ 出发
cargo run -q -p engine --example scene_shot -- scenes/index.scene target/eyeballs/index-shot.png
```

## 10. 本轮（视觉修复后）复跑读数

| 门 | 命令 | 读数 |
|---|---|---|
| 工程门 12 条 + 实例化 2 条 | `cargo test -p engine --lib sample_project` | 14 passed / 0 failed |
| 渲染 crate 单测（含两条新门） | `cargo test -p rustlua-render --lib` | 148 passed / 0 failed |
| 脚本全量编译 | `lua_check`（绝对路径传参） | `all 22 file(s) compile` |
| renderer 组 | `cargo test -p engine --lib host::renderer::tests:: -- --test-threads=1` | 82 passed / 0 failed（91.8s） |
| 场景 schema | `cargo run -q -p editor -- --project ../examples/game_api --check` | **未跑**：`editor` 二进制编译红，来自在途的 UI Studio 改动（`layout.rs` 新增 `PANEL_UI_STUDIO`，`lib.rs` 的 re-export 未带上），与本工程无关 |

schema 门缺席期间的替代核对：29 个组件名逐一比对 `builtin_registry` + `Script` 的特例分支，
没有 scene 侧写了引擎不认识的名字；字段级校验由
`sample_project_runs_every_scene_headless`（逐场景真实反序列化每条 payload）覆盖。

renderer 组必须单线程跑：并发时每条门都自己 `request_device`，本机驱动会回收设备实例
（`Device(Lost)` / `DXGI_ERROR 0x887A0005`），一次丢失之后其余全是连锁的 Validation Error。
这条是调度红，不是内容红——判真红以前先加 `--test-threads=1` 复跑。
