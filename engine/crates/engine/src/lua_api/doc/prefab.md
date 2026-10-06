# Lua 模块 `prefab`

> 运行期 API 面基线里本模块有 7 个函数。本页由 `scripts/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

把 `.prefab` 实体树在运行期实例化进活世界，是 UI 预设的运行时载体：版面（层级、锚点、
矩形、图片九宫格、字号）写在预设文件里，取值（文案、显隐、按数据改的四边、点击后的行为）留在
脚本里。`spawn` 返回的 handle 是这一批 API 的句柄，只在本次运行内有效、**不进存档**，切图也不会
把实例当场景实体重生；实例里的节点可以用 `prefab.node` 按预设中写的名字取到实体 id
（与 `entity.*`、组件读写那套用的是同一个 id），也可以直接 `entity.find(name)`——spawn
的名字同样登记进名字表。

这条分工有一条硬边界：**每帧变化的东西不能进预设**。场景 `Image`/`Text` 组件只有
`color=[f32;3]`，没有 alpha，所以高亮框的呼吸、淡入淡出、跟着手指走的提示框这些"每帧改透明度
或改几何"的表现仍然属于 `ui.panel`/`ui.image`/`ui.text_at` 动态层；预设负责的是"这一刻有什么、
摆在哪、写着什么字"。

两条作者规则要记住：带 UI 组件却没有 `UiLayout` 的实体**既不会被画也不会被点中**（布局
解算只收有 `UiLayout` 的节点）；`UiEvents` 绑的是**全局函数名**，函数必须在 `spawn` 之前
已经存在，派发时找不到就静默什么都不做。

## `prefab.despawn(handle: i64)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/prefab.rs:52`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

销毁一次 spawn 的整棵实例，并把它并进宿主布局快照的节点一起撤掉。撤干净的标准是：实体不
再存活、那个矩形不再出现在 UI 读数里、`UiEvents` 也不再被点击派发——面板关掉之后不该留
下任何"看不见但点得中"的命中框。

- 参数：`handle` — `spawn` 返回的句柄。
- 返回：`true` 本句柄存在且已拆除；`false` 句柄未知或已经拆过（重复调用是安全的 no-op）。
- 失败：活世界写入失败时报 Lua 错误（句柄此时已从登记表移除，不会再被派发）。
- 示例：

```lua
if prefab.despawn(menu) then menu = nil end
```

## `prefab.node(handle: i64, name: String)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/prefab.rs:79`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

按预设里写的实体名取实例内的某个节点 id。同一个预设 spawn 两次会得到两套不同的实体，所以
**必须**经由 handle 查名，不能拿上一次的 id。

- 参数：`handle` — 实例句柄；`name` — 预设 `entity` 行的名字。
- 返回：`i64` 实体 id。
- 失败：`prefab.node: 没有 handle <n>`、`prefab.node: 实例 <n> 里没有节点 "<name>"`。
- 示例：`local start_btn = prefab.node(menu, 'BtnStart')`

## `prefab.root(handle: i64)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/prefab.rs:66`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

取实例根实体的 id，用于整棵树的显隐、位移或挂组件——预设文件里 `root` 那一行写的是谁，
这里就返回谁。

- 参数：`handle` — `spawn` 返回的句柄。
- 返回：`i64` 实体 id（`index << 32 | generation`，可直接喂给 `entity.*` 与组件接口）。
- 失败：`prefab.root: 没有 handle <n>`。
- 示例：`local panel = prefab.root(menu)`

## `prefab.set_rect(handle: i64, name: String, left: f32, top: f32, right: f32, bottom: f32)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/prefab.rs:155`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

运行期改一个节点的**四边**——列表行按数据排位、进度条按数值改宽度就靠它，于是"版面在预设里、
尺寸由数据决定"这条分工成立；没有它，运行期只能用 `ui.panel`/`ui.image` 动态层把同一块面板
再画一遍。

口径是引擎的布局解算本身：`四边 = 父矩形 × anchors + offset`，这里写的四个数就是 `offsets`，
**`anchors` 沿用预设里 authored 的值**（要改锚点应该改预设文件）。所以默认锚点
`(0,0,0,0)` 下这四个数就是父坐标里的像素边；stretch 锚点下它们是相对锚点的边距。改动会沿父链
传下去：改一个容器/行根，它的全部子节点跟着位移，不必逐个子节点通知。写入同时落到重建布局
节点的那份数据与世界的 `UiRectLayout` 组件，画面与 `entity` 侧查到的因此是同一个数。

- 参数：`handle` — 实例句柄；`name` — 节点名；`left/top/right/bottom` — 新四边（设计像素，
  写入按预设的文本口径保留两位小数）。
- 返回：无。宿主在下一步解算布局时自动采用新矩形（绘制与命中同源，没有"重算"这一步要做）。
- 失败：`prefab.set_rect: 没有 handle <n>`、`... 实例 <n> 里没有节点 "<name>"`、
  `... 节点 "<name>" 没有 UiLayout 组件（预设里它不会被画，也没有矩形可写）`。
- 示例：

```lua
-- 背包第 3 行放到行槽位上（版面给了 10 行的行高，位置由数据定）
prefab.set_rect(bag, 'Row3', 24, 24 + 3 * 52, 696, 24 + 3 * 52 + 48)
```

## `prefab.set_text(handle: i64, name: String, text: String)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/prefab.rs:97`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

改实例内某节点 `Text` 组件的文字——写的是渲染真正读的那一份，所以这一条补上之后，"版面进
预设、取值留脚本"才成立：标签不必再被场景层隐藏、由脚本另画一遍。字体、字号、颜色刻意不
碰，那些属于版面，要改应该改预设文件。

- 参数：`handle` — 实例句柄；`name` — 节点名；`text` — 新的文字。
- 返回：无。
- 失败：`prefab.set_text: 没有 handle <n>`、`... 实例 <n> 里没有节点 "<name>"`、
  `... 节点 "<name>" 没有 Text 组件（预设里它是图片/面板/容器）`。
- 示例：

```lua
prefab.set_text(menu, 'LblSave', save.exists('slot1') and '继续冒险' or '新的冒险')
```

## `prefab.set_visible(handle: i64, name: String, visible: bool)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/prefab.rs:133`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

隐藏/显示实例里的一个节点**及其整棵子树**。一屏里"这一刻不该出现的那些东西"靠它收起来：
列表只画前 N 行、未解锁的页签、悬停才出现的装饰——不必为它们各写一份 `ui.hide`，也不必像
现在这样把场景文字逐名 `ui.scene_text_visible(name, false)` 再让脚本重画一遍。

语义两条，都是刻意的：

* **子树**：藏行根 ⇒ 行里的图与字一起没了，不需要逐个藏。显隐是**相对父**的（与 Unity
  `GameObject.SetActive` 同语义）——父还藏着时单独显示某个子节点不生效，显示父之后整棵子树
  一起回来。
* **画与命中同一次失效**：这条路是"该节点不建布局项"，宿主拿不到矩形就既不产顶点也不产点击
  目标，所以不存在"看不见的按钮仍能被点中"。代价是它只管走 `UiLayout` 的场景 UI：带 `Sprite`
  的实体按相机投影绘制与命中，不受它影响（那种实体该用 `entity.destroy` 或视图遮罩）。

实体与组件一概不动：`set_text` 写过的文字、`set_rect` 写过的矩形都留着，显示回来不需要重设。

- 参数：`handle` — 实例句柄；`name` — 节点名；`visible` — `true` 显示、`false` 隐藏。
- 返回：无。
- 失败：`prefab.set_visible: 没有 handle <n>`、`... 实例 <n> 里没有节点 "<name>"`。
- 示例：

```lua
-- 只有 7 件行李：把第 8..10 行整行收起来（行里的图标与数量一起不见、也点不到）
for row = items + 1, 10 do
  prefab.set_visible(bag, 'Row' .. row, false)
end
```

## `prefab.spawn(path: String)`

> 绑定：`engine/crates/rustlua-scripting/src/script/api/prefab.rs:22`（直写）；签名由 `scripts/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

读一个 `.prefab` 并把它整棵实体树实例化进活世界，返回实例 handle。路径相对工程根，读取走
VFS（打包后的包内读取只有这一条路，直连 `fs` 在发布包里就是"开发机能跑、真机白屏"）。
解析末尾会做完整性校验（版本行、root 存在、`parent` 边成树），所以坏预设是**加载即报错**
而不是静默少一层。实例化成功后，带 `UiLayout` 的节点会登记进宿主的 UI 布局快照，于是
"画得出"与"点得中"对 spawn 出来的实体与场景里摆的一律成立。

- 参数：`path` — 预设文件路径，如 `assets/prefabs/title_menu.prefab`。
- 返回：`i64` handle，交给 `root` / `node` / `set_text` / `set_rect` / `set_visible` / `despawn`。
- 失败：工程未加载（`project_root` 为空）、文件读不到、非 UTF-8、预设不合法（如 `parent`
  指向不存在的实体，错误串里会点名那条边）——均为 Lua 错误，不返回 nil。
- 示例：

```lua
local menu = prefab.spawn('assets/prefabs/title_menu.prefab')
prefab.set_text(menu, 'LblVersion', 'v1.0.0')
```
