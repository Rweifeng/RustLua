# game_api 3.0：能力图谱与渲染/UI 能力展廊

> 状态：实施完成（2026-11-17）。取代 [docs/74](74-game_api演示工程2.0.md) 的"分步 Lab"叙事
> **作为当前形态的说明**，但 2.0 的框架（`lab.lua`）、步骤模型、行为自检与覆盖率底线**原样保留**；
> 3.0 在它上面补三件事：**能力图谱**（引擎全部能力 ↔ 演示步骤的单一真源）、
> **渲染展廊**（地形/水面/PBR/光照/后处理/特效叠加层）与 **UI 能力收口**（排版/状态过渡）。
>
> 基线读数（2026-11-17 工作区）：
> - 运行期 Lua API 面 **51 个模块表 / 432 个函数**（`rustlua-scripting/src/api_surface_snapshot/lua_api_surface.json`）；
> - 运行时组件 **31 个**（`ComponentId::ALL`）；
> - 能力图谱 **64 条能力 / 8 个分组**（`examples/game_api/scripts/capabilities.lua`，由门 `--check` 计数）；
> - `cargo test -p engine --lib sample_project` **16 条工程门全绿**（12 条 2.0 门 + 4 条 3.0 门）。
>
> 前置阅读：[docs/74](74-game_api演示工程2.0.md)（Lab 框架与步骤模型）、
> [docs/59](59-示例工程game_api与API覆盖率门.md)（覆盖率门语义与夹具约束）、
> [`examples/game_api/README.md`](../examples/game_api/README.md)（演示模块写法与硬规则）。

## 1. 为什么是 3.0

2.0 解决的是"**每个 Lua 函数都被调用过一次**"，并用 `lab.self_test` 钉住读回语义。
它追不上近两个月的引擎进化：

| 引擎变化 | 落点 | 2.0 的缺口 |
|---|---|---|
| Shader Graph 运行时叠加层（M88） | `project.toml [render] effect_overlay` + `render.set_effect_params/param/time`（M94/M95） | 3 个函数没有真实调用点（覆盖率门红）；`.sgraph` 资产、命名参数、场景深度采样在演示工程里完全缺席 |
| 项目级渲染配置（M98 等） | `[render]` 的 `particle_softness` 等键 | 只在引擎单测里；演示工程没有一份"项目级渲染键"的样例 |
| 3D 渲染管线跨档 | 地形 splat、连续水面、点光阴影、SSAO/SSR/接触阴影/雾/神光/景深 | `api_3d` 只有 `__ground_plane__` 与三角形；地形/水面/体素只在脚本采样里出现，**没有一张能看的画** |
| UI 子系统收口（M80） | 文本排版（align/valign/overflow/rich_text）、ButtonStates 的 tint/transition、Mask/CanvasGroup、值控件 | 组件覆盖到了，但能力层面没有"这一条对应哪个演示步骤"的索引 |

3.0 的目标是把演示工程变成**可被机器核对的能力图谱**，同时把新渲染能力画出来。

## 2. 五条标准（每条都有门或可复现命令）

| # | 标准 | 落地方式 | 怎么证明 |
|---|---|---|---|
| 1 | **能力可指认** | `scripts/capabilities.lua`：64 条能力，每条写 `scene` / `step` / `setting` / `components` / `api` / 证据 | `sample_project_capability_catalog_is_complete` |
| 2 | **能力不回退** | 每条能力的场景必须登记在 `demos.lua`、步骤必须真实存在于演示模块 | 同上（逐个 `require` 模块读 `M.steps`） |
| 3 | **渲染能力有单一真源** | `RenderSettings` 的每个布尔开档必须在图谱里有 `setting` 条目；`[render]` 项目键与 `.sgraph` 真解析 | `sample_project_render_capabilities_are_demonstrated` |
| 4 | **UI 能力分两层核对** | 运行时 UI 组件（`ComponentId::ALL`）+ `ui.*` 控件/状态 API 都必须被条目认领 | `sample_project_ui_capabilities_are_demonstrated` |
| 5 | **画面优先** | `render_landscape` / `render_pipeline` / `effect_overlay` / `voxel_cells` 四步把新能力画进舞台；`typography_states` 把排版与状态过渡放进场景 UI | 步骤自检 + 人工目验；画面本身不进像素 golden |

## 3. 能力图谱（`scripts/capabilities.lua`）

结构与字段：

```lua
return {
    version = 3,
    groups = { { id = "render", title = "Rendering", zh = "渲染" }, ... },  -- 8 组
    capabilities = {
        {
            id = "render.effect_overlay",      -- 全局唯一，文档/门按 id 引用
            group = "render",
            title = "Shader Graph 运行时叠加层",
            scene = "api_3d",                  -- 必须是 demos.lua 登记的场景
            step = "effect_overlay",           -- 必须是该模块 M.steps 里的 id
            setting = "effect_overlay",        -- RenderSettings/RenderConfig 键名（渲染条）
            components = { "MeshRenderer" },   -- 运行时组件（UI/渲染条）
            api = { "render.set_effect_params", ... },
            note = "证据/边界一句话",
        },
    },
}
```

- `kind = "platform"` 的条目可以没有 `scene`（平台能力不属于单个演示场景）；
- `kind = "golden"` 的条目没有 `step`（渲染夹具由 golden 钉住，没有步骤）；
- 其余条目（默认 `feature`）**必须**同时写 `scene` 与 `step`。

门 `sample_project_capability_catalog_is_complete` 逐条核对：

1. `id` 唯一、`group` 在 `groups` 里、场景已登记、步骤真实存在（`require(module).steps`）；
2. 除 golden / platform 外，每个演示场景至少一条能力（"这个场景演示什么"必须说得清）；
3. `demos.lua` 每个场景的 `modules = { ... }` 并集**等于**运行期 API 面（新增 Lua 模块没人认领会红）；
4. 能力条数下限 40、分组下限 5（防"图谱被删空后门仍绿"）。

## 4. 渲染能力展廊（`api_3d` 新增四步）

| 步骤 | 演示的能力 | 关键调用 | 读者能看到什么 |
|---|---|---|---|
| `render_landscape` | 参数化地形（高度场 + 四层 splat + 内嵌贴图）、连续水面（波法线/菲涅尔/天空反射）、5 种 PBR 材质分支、运行期相机 | `mesh.set("__terrain__?…")`、`mesh.set("__water__?…")`、`mesh.set_material`、`camera.set` | 场景外 (40,-1,40) 的 48m 地形 + 20m 水面 + 缓慢自转的金属/粗糙/自发光/半透明物体 |
| `render_pipeline` | 雾/云/水色/曝光/闪光/积雪/后处理/画质档 + `render.stats` | `render.set_fog`、`set_cloud_*`、`set_exposure`、`set_post_fx`、`set_surface_cover`、`set_quality`、`stats` | 雾与云随时间缓慢变化，右侧卡片读上一帧 draw/tri/set_pass |
| `effect_overlay` | `.sgraph` 运行时叠加层：槽位参数、命名参数、时间 | `render.set_effect_params`、`set_effect_param`、`set_effect_time` | 金色深度描边叠在 3D 画面上；改槽位改宽度、改时间改相位 |
| `voxel_cells` | 逐格体素网格（外露面剔除） | `cells3d.build/dig/count`、`mesh.set` | 挖空中心的 3×3×3 体素：满格 54 面、挖中心 60 面 |

项目级渲染键写在 `examples/game_api/project.toml`：

```toml
[render]
# M88/M91/M92/M94/M95：全屏深度描边叠加层（`.sgraph` 是唯一源）。
effect_overlay = "assets/fx/showcase_outline.sgraph"
# M98：GPU 粒子软边（0 = 硬边；默认 0.02）。
particle_softness = 0.04
```

`.sgraph` 是 JSON 图（`rustlua_visual_script::shader_graph::ShaderGraph`）：
`depth_edge` → `output.alpha`，`constant` → `output.tint`。命名参数为 `edge_width` 与
`color_r/g/b`；运行时叠加层采样场景深度（`@binding(6)`），因此走"离屏 + 回拷"路径——
这正是 M91/M92 给叠加层加的能力。

**反面对照**（自检里真跑）：`render.set_effect_params(16, …)` 拒非法槽位、NaN 值拒、
`render.set_effect_time(-1)` 拒、未知命名参数拒；`render.set_fog(2.0)` 等越界值拒并带原因。

## 5. UI 能力收口

`ui_test.scene` 新增 7 个实体（id 46–52），把 2.0 只"写过组件"的能力变成可看的能力：

| 实体 | 能力 | 组件字段 |
|---|---|---|
| `TypographyPanel` + `TypographyCenter/Ellipsis/Rich` | 文本排版 | `align` / `valign` / `overflow` / `voverflow` / `rich_text`（`<b>` / `<color=#…>`） |
| `TransitionButton` + `TransitionLabel` | 状态过渡 | `ButtonStates` 的 `tint.normal/hover/pressed` + `transition=0.15:smoothstep` |

演示模块 `demo_ui_events.lua` 新增第 6 步 `typography_states`：
`ui.measure_text` 断言工程字体覆盖富文本样例、实体都在、排版样例可度量；
悬停过渡在真实运行里由宿主交互树驱动（自检不猜像素）。

门 `sample_project_ui_capabilities_are_demonstrated` 核对：

- 15 个运行时 UI 组件（`Text/Image/UiLayout/Container/LayoutItem/UiEvents/ButtonStates/Mask/CanvasGroup/Toggle/Slider/Progress/Scrollbar/ScrollView/UiVirtualList`）都被能力条目认领，且仍在 `ComponentId::ALL` 里；
- 19 个 `ui.*` 控件/状态 API（`panel/text_at/image/measure_text/grid/drag_*/tooltip/modal/focus_*/nav_stack/scroll*/virtual_list/rich_text/text_input/view_model_set/toggle/slider/progress`）都被条目认领；
- UI 能力条目下限 12 条。

### 5.1 重制过程发现并修掉的真问题

| 问题 | 症状 | 修法 |
|---|---|---|
| `ButtonStates` 的 schema 没登记 `tint.<state>` / `transition` | `editor --project examples/game_api --check` 对 `ui_test.scene` 的 `TransitionButton` 报 `schema_error … unknown field "tint.normal"`（运行时 M80-07 早就支持，schema 停在 M76） | `rustlua-ecs/src/component_schema.rs` 的 `BUTTON_STATES_FIELDS` 补 6 个字段，并在 `component_schema_parity` 的真实工程样本里加一条 `tint.* + transition` 串，钉住"schema 接受 = 引擎接受" |
| 3 个 M94/M95 特效函数只有名字没有调用 | `sample_project_demos_cover_every_lua_api` 红：全量 432 个函数缺 3 个 | `demo_3d.lua` 的 `effect_overlay` 步真实调用槽位/命名参数/时间三条路径，并带反面对照 |
| 按 A 的能力图谱与菜单"两层文字重叠" | 实机：覆盖层面板盖住了卡片图，但卡片三行文字与页眉/页脚仍画在动态 UI **之上** | `demo_index.lua` 打开时用 `ui.scene_text_visible(name, false)` 隐藏全部菜单场景文字、收起时按清单恢复；新增门 `sample_project_index_atlas_hides_menu_layers` 钉住"藏/恢复"两条路径 |

## 6. 入口菜单：能力图谱浏览器

`scenes/index.scene` 仍由 `scripts/docgen/gen-api-showcase-index.ps1` 从 `demos.lua` 生成，
副标题现在多一项**能力条数**（从 `capabilities.lua` 现算，不手写）。
`scripts/demo_index.lua` 新增 `A` 键：打开"能力图谱覆盖层"，按分组显示条数与三条样例标题；
收起时清理动态元素（`ui.hide` + `ui.clear`），普通菜单照常重绘。

这是"图谱"的**运行时入口**：文档、门、菜单读的是同一份 `scripts/capabilities.lua`。

## 7. 门矩阵（16 条）

| 门 | 位置 | 3.0 变化 |
|---|---|---|
| 逐场景 headless 实例化 + 步进 | `sample_project_runs_every_scene_headless` | 场景新增实体/步骤仍能跑 |
| 行为自检 | `sample_project_demos_self_test` | api_3d 17 步、ui_test 6 步；新步骤各带断言 |
| 脚本全量编译 | `sample_project_scripts_all_compile` | 覆盖 `capabilities.lua` |
| Lua API 覆盖 | `sample_project_demos_cover_every_lua_api` | 51/432；修掉 3 个 render 特效函数 |
| 组件覆盖 | `sample_project_scenes_cover_every_runtime_component` | 不变（31） |
| 目录/菜单/卡片/命中/遮挡/字形 | 2.0 七条 | 重新生成 `index.scene` 与字体子集 |
| **能力图谱完整性** | `sample_project_capability_catalog_is_complete`（新） | id/分组/场景/步骤/modules 并集/下限 |
| **渲染能力** | `sample_project_render_capabilities_are_demonstrated`（新） | RenderSettings 开档、`[render]` 键、`.sgraph` 编译与命名参数、3 个特效函数真实调用 |
| **UI 能力** | `sample_project_ui_capabilities_are_demonstrated`（新） | 15 个组件 + 19 个 `ui.*` API 被条目认领 |
| **菜单覆盖层让位** | `sample_project_index_atlas_hides_menu_layers`（新） | 按 A 打开图谱时 `Title/Subtitle/Footer` 与 15×3 行卡片文字必须全部隐藏，收起后全部恢复 |

## 8. 实施顺序

1. 修覆盖率红：`render.set_effect_params/param/time` 在 `demo_3d.lua` 里真实调用；
2. 加 `.sgraph` 资产与 `[render]` 配置，演示叠加层参数面；
3. `api_3d` 四步渲染展廊（地形/水面/PBR、管线、叠加层、体素）；
4. `ui_test` 排版/过渡实体与第 6 步；
5. 能力图谱 + 三条新门 + 菜单 `A` 覆盖层；
6. 重生成 `index.scene` 与字体子集，跑全部门。

## 9. 复现

```powershell
cd engine

# 全部工程门（16 条）
cargo test -p engine --lib sample_project

# 只看能力图谱缺什么
cargo test -p engine --lib sample_project_capability_catalog_is_complete -- --nocapture
cargo test -p engine --lib sample_project_render_capabilities_are_demonstrated -- --nocapture
cargo test -p engine --lib sample_project_ui_capabilities_are_demonstrated -- --nocapture

# 脚本语法
cargo run -q -p engine --example lua_check -- ../examples/game_api (Get-ChildItem ..\examples\game_api\scripts\*.lua -Recurse | ForEach-Object FullName)

# 场景 schema（schema_errors=0 / unknown_components=0）
cargo run -q -p editor -- --project ../examples/game_api --check

# 重新生成入口菜单与字体子集
powershell -NoProfile -ExecutionPolicy Bypass -File ../scripts/docgen/gen-api-showcase-index.ps1
cargo run -q -p engine --example subset_project_font -- ../examples/game_api --source ../engine/crates/editor/fonts/NotoSansSC-VF.ttf
```

## 10. 诚实边界

- **能力图谱是"有演示"的索引，不是"每个分支都测过"**：门证明每条能力指向的场景/步骤真实存在、
  渲染开档与 UI 组件/API 被认领；它不证明每个能力的所有错误路径与平台降级都跑到。
- **`.sgraph` 的视觉只做目验**：门真编译它并检查命名参数与深度绑定，但"描边好不好看"没有像素门；
  `wp0_3d_golden` 等 golden 用的是不带叠加层的独立渲染器，项目级叠加层不改变它们的读数。
- **`particle_softness` 是项目级开关**：演示只把它打开并让粒子出现在画面里；像素级差异由
  `rustlua-gpu` 的软边单测负责，不在本工程门内。
- **平台能力不在本工程运行**：`platform.*` 条目只登记产物层证据的出处（PC/WebGL/微信/Android），
  三宿主真机验收仍由各自的门与文档负责。
- **fixture 场景内容不变**：`demo3d.scene` / `mixed.scene` 继续由既有金色门钉住；3.0 只给它们
  登记 `kind = "golden"` 的能力条目。
