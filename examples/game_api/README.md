# game_api —— RustLua API 演示工程

这个示例工程的唯一目的：**让引擎当前的每一个 Lua API 都有一处可运行、可交互、可读的演示**。
它不是玩法样本（玩法样本是 `examples/game_jhyt`），而是一份"活文档"。

- 一个场景 = 一个 API 模块组：`scenes/api_*.scene` + `scripts/demo_*.lua`
- 入口菜单 `scenes/index.scene` 列出全部演示，点击卡片或按 Enter 进入，ESC 返回
- 演示清单的唯一真源是 `scripts/demos.lua`（场景名 ↔ 模块名 ↔ 菜单文案）
- 工程入口 `scripts/main.lua` 只转发生命周期，真正的分发在 `scripts/router.lua`

## 1. 目录约定

```text
examples/game_api/
├── project.toml            # main_script=scripts/main.lua, startup_scene=scenes/index.scene
├── scenes/
│   ├── index.scene         # 入口菜单（焦点环 + 滚动 + UiEvents 点击）
│   ├── main.scene          # 2D 综合：Sprite/Quad/Animation/Particles/Tilemap/物理/文字
│   ├── unified.scene       # 一份场景表达全部渲染类别（2D + 3D + UI + 粒子 + 世界文字）
│   ├── api_*.scene         # 每个 API 模块组一个场景
│   ├── demo3d.scene        # 3D 渲染 fixture（被 wp0_3d_golden 逐项断言，**不要改内容**）
│   └── mixed.scene         # 混合 2D/3D fixture（被 wp0_mixed_golden 逐项断言，**不要改内容**）
├── scripts/
│   ├── main.lua            # 工程级回调 → router
│   ├── router.lua          # 场景 → 演示模块分发 + ESC 返回 + 状态行渲染
│   ├── demos.lua           # 演示清单（真源）
│   ├── api_demo.lua        # 共用工具：标题/状态行/按键提示、宿主帧依赖调用的安全包装
│   ├── demo_*.lua          # 每个场景一个演示模块
│   └── entity/*.lua        # 每实体 `Script` 组件的钩子演示（on_spawn / on_update）
├── data/*.json             # `data.load` / `data.schema` 的演示数据
└── assets/                 # 纹理 / 音频 / 模型 / 预制体 / 字体
```

## 2. 演示模块的写法

```lua
-- api_xxx：演示 <模块名> 的 <函数清单>。
local api = require("api_demo")

local M = {}

--- 只在进入本场景时调用一次：注册动态 UI、初始化演示状态。
function M.on_create()
    api.title("api_xxx - <一句话说明>")
    api.hint("<按键> ... - " .. api.KEYS_BACK)
end

--- 每个固定步一次。返回的字符串会进引擎日志（`run_script_steps` / 编辑器 Play 都能看到）。
function M.update()
    api.status("<本帧想让人看到的事实：读回的数值、计数、状态>")
    return "api_xxx_ok <关键值>"
end

--- 可选：场景里 `UiEvents` 组件的点击回调（实体名就是参数）。
function M.on_click(name) end

--- 可选：`ui.drag_end` 触发的全局拖拽回调。
function M.on_drag(from_id, from_index, to_id, to_index) end

--- 可选：`day.sleep()` / 时钟跨午夜触发的日循环钩子。
function M.on_day_start() end
function M.on_day_end() end

--- 可选：离开场景时释放（`ui.focus_close` / `ui.scroll_close` / `ui.clear`）。
function M.on_destroy() end

return M
```

### 硬规则（会被自动化门检查）

1. **顶层只定义函数与常量。** 模块文件会被单独编译（`lua_check`）与加载，顶层不得调用引擎
   API：进入场景前的状态（实体还没生成、宿主还没回填相机）会让"只是加载一个模块"报错。
2. **`update()` 必须能在 headless 下跑通。** 引擎测试会实例化每个场景并步进若干帧：
   - 不要在 `update()` 里写盘（`save.commit` / `save.write` / `audio.save_settings` /
     `input.save_settings` / `settings.toml`），除非按键触发；
   - 不要在 `update()` 里无条件切场景（`scene.load` / `map.go` / `map.enter` / `app.quit`），
     除非按键触发；
   - 宿主帧依赖的调用必须用 `api.try` 包一层并写明原因：
     `camera2d.get/screen_to_world/world_to_screen/mouse_world`、
     `camera3d.ray/mouse_ray/world_to_screen/get/pick_plane`、
     `physics.debug_draw` / `physics3d.debug_draw`（渲染期消费）。
3. **每个函数都要真的被调用一次。** 覆盖率门按 `模块.函数(` 的文本出现判定，
   因此不要用变量间接调用（`local f = ui.panel` 这种写法在门里等于没演示）。
4. **场景必须过 schema。** `editor --project examples/game_api --check` 要求
   `schema_errors=0`、`unknown_components=0`；组件串字段名/类型见第 3 节。
5. **注释写明"看什么"**：演示的价值在于读代码的人知道该按哪个键、该看哪个数值。

## 3. 场景文件格式

```text
scene_version=2
name=<场景名>
meta	spatial	local|organizational        # parent 边的语义；不写 = organizational
meta	editor.root_order	<根实体 id 列表>
entity	<id>	<名字>	[父实体 id]
component	<id>	<组件名>	<组件串>
```

组件串一律 `键=值;键=值`。常用组件的字段（完整定义见
`engine/crates/engine/src/core/component_schema.rs` 的 `builtin_registry`）：

| 组件 | 字段 |
|---|---|
| `Transform` | `position=x,y,z`（必填）`;rotation=rx,ry,rz,rw`（必填）`;scale=sx,sy,sz`（必填）——2D/3D 同一个组件 |
| `Camera` | `projection=orthographic`（必填判别字段）+ `center=x,y;zoom=z`；或 `projection=perspective` + `position=x,y,z;target=x,y,z;fov=度;near=n;far=f`。可选 `pose=transform`、`enabled=false` |
| `Light` | `direction=x,y,z;color=r,g,b;intensity=i`（`direction` 是**光传播方向**，太阳应写 y 为负） |
| `MeshRenderer` | `mesh=assets/models/x.glb` 或内置哨兵 `mesh=__ground_plane__` |
| `Sprite` | `texture=..;size=宽x高;tint=r,g,b,a;sorting_layer=default;pivot=x,y;order=i;z=f;flip_x/flip_y=true` |
| `Quad` | `size=w,h;color=r,g,b,a;pivot=x,y;texture=..;z=f;layer=名;order=i;flip_x/flip_y=true`（不写 texture = 内置白纹理） |
| `Animation` | `default=<状态名>;clip:<状态>:<图集>:<列数>:<fps>:<looped>[:events=<帧>:<名>]`（可多个 `clip:` 段） |
| `Animation3D` | `mesh=assets/models/x.glb;clip=<剪辑名>;looped=true` |
| `Tilemap` | `tile_size=宽x高;width=n;height=n;tiles=<逐格字符>;solid=<逐格 0/1>;texture=..` |
| `Collider` | `shape=ball;radius=r`（或 `shape=cuboid;half_width=..;half_height=..`）`;sensor=true;dynamic=false;offset=x,y;layer=N;mask=N;ccd=true` |
| `Collider3d` | `shape=ball;radius=r` / `shape=cuboid;half_width=..;half_height=..;half_z=..` / `shape=capsule;half_height=..;radius=..`，可选字段与 2D 同名 |
| `ParticleEmitter` | `count=n;speed=f;life=f;size=f;color=r,g,b,a;rate=每秒`（`rate>0` 为连续发射）`;size_end;color_end;gravity=x,y;drag` |
| `WorldText` | `text=..;font_size=px;color=r,g,b;offset=x,y;centered=true;font=..` |
| `Visibility` | `layers=<u32 位遮罩，可用 0x 十六进制>` —— **运行时不消费场景里的这个组件**（`typed_scene` 没有它的分支，写了会让"源组件数 == typed 组件数"那条门判红）：只在 Lua 里用 `visibility.set` 运行期挂 |
| `RenderMode` | `mode=world\|overlay\|screen` |
| `Script` | `file=scripts/entity/x.lua;enabled=false` |
| `Text` | `text=..;font_size=px;color=r,g,b;font=assets/fonts/x.ttf` |
| `Image` | `texture=..;color=r,g,b;nine_slice=l,r,t,b` |
| `UiLayout` | `anchors=l,t,r,b;offsets=l,t,r,b;min_size=w,h;max_size=w,h;pivot=x,y`（屏幕 1280×720，锚点 0.5 = 屏幕中心，偏移是相对锚点的像素） |
| `Container` | `kind=vbox\|hbox\|grid\|margin\|center;padding=l,t,r,b;spacing=f;columns=n;stretch=true;fill=true;align=start\|center\|end` |
| `UiEvents` | `event=click\|pointer_enter\|pointer_exit\|pointer_down\|pointer_up\|focus\|submit\|value_changed\|drag;function=<全局函数名>;target=<实体名>;arg.<名>=值`，多条用 `\|` 分隔。**运行时只分发 `click`**，其余事件类型是编辑器/解析侧的绑定 |
| `ButtonStates` | `normal=..;hover=..;pressed=..;disabled=..;focused=..;disabled_state=true`。**运行时零消费方**（只有编辑器读它），因此写进场景是"给编辑器看的五态配置"，按下去不会换贴图 |
| `UiBindings` | `property=text.text\|image.visible\|progress.value;target=<实体名>;source=<数据源>;default=<值>`，多条用 `\|` 分隔。**只注册了 schema**（编辑器 UI Designer 侧），`typed_scene` 没有它的分支 → 写进场景会判红"源组件数 == typed 组件数" |
| `UiAnimation` | `track.<属性名>=<时间>:<值>:<缓动>\|...`，多条用 `\|` 分隔。同上：schema-only，运行时不消费 |

`UiLayout` 的锚点+偏移是**屏幕空间**：`anchors=0.5,0.5,0.5,0.5;offsets=-100,-50,100,50`
= 以屏幕中心为锚点、宽 200 高 100 的矩形。零尺寸（四条偏移相等）是"点锚"，文字从该点开始画。

### 两条会被门拦住的场景约束

1. **`parent` 边只在父链零平移时可用。** `scene_migration` 的门会对示例工程的每个场景逐实体断言
   "父当作组织关系"与"父当作局部空间"两种解释下世界位置相同（它**不读 `meta spatial`**），
   所以示例场景里的父子对都把父放在 (0,0,0)；要演示"子跟着父走"，请在运行期用
   `transform.move` 移动父。
2. **场景 `Image`/`Text` 永远画在 Lua 动态 UI 之上**（`runtime_scene.rs` 的绘制顺序）。
   铺满屏幕的背景**不能**写成场景 `Image` 实体——它会把脚本的状态行整片盖住；背景请用
   `Quad`（世界通道）或 `ui.panel`（动态面板）。`sample_project_shared_overlay_is_not_covered`
   会抓这类覆盖。

### 屏幕版式

`api_demo` 的三行取景框：标题在左上 (24,16)；状态行与按键提示**从屏幕底部往上排**
（提示贴 y≈688，状态行压在它上方，两段都按自己的行数算高度，因此多行状态不会压住提示）。
演示自己画的说明行请留在 **y ≤ 600**，把 600..720 留给共享取景框。

## 4. 自检命令

```powershell
# ① 语法：所有演示脚本编译得过（只编译不执行，秒级）
cd engine
cargo run -q -p engine --example lua_check -- ../examples/game_api (Get-ChildItem ..\examples\game_api\scripts\*.lua -Recurse | ForEach-Object FullName)

# ② 场景 schema / 组件漂移（要求 schema_errors=0 unknown_components=0）
cargo run -q -p editor -- --project ../examples/game_api --check

# ③ 全部工程门：逐场景 headless 实例化+步进、脚本编译、API 覆盖率、组件覆盖率、
#    入口菜单接线、卡片命中、共享取景框、字体缺字
cargo test -p engine --lib sample_project

# ④ 改文案后重新子集化工程字体（缺字会让 ③ 里的字形门判红）
cargo run -q -p engine --example subset_project_font -- examples/game_api
```

## 5. 两个生成物（改完要重跑工具）

| 生成物 | 生成器 | 说明 |
|---|---|---|
| `scenes/index.scene` | `scripts/docgen/gen-api-showcase-index.ps1` | 从 `scripts/demos.lua` 读清单，铺成 2 列卡片网格；手改会在下次生成时丢失。一致性由 `sample_project_index_menu_lists_every_scene` 兜底 |
| `assets/fonts/NotoSansSC-VF.ttf` | `cargo run -q -p engine --example subset_project_font -- examples/game_api` | 按**工程实际用字**（场景文本 + Lua 字符串字面量）子集化。文案里新增汉字就要重跑，否则屏幕上出现方框（`sample_project_scene_text_has_every_glyph` 会判红） |

新增一个演示的完整步骤：① 写 `scenes/api_xxx.scene` + `scripts/demo_xxx.lua`；
② 在 `scripts/demos.lua` 里登记（场景名 / 模块名 / 菜单文案）；
③ 重跑索引生成器；④ 跑 §4 的 ③。清单、菜单、门三者会互相校验，漏一步就会判红。
