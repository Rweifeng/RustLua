# 兼容性设计清单

本文记录当前仍然保留的兼容边界，以及已经从实现中删除的旧设计。原则是：
**旧数据只在版本化读取边界转换一次；运行时、编辑器主逻辑和保存路径只消费/
输出最新格式。** 任何新增兼容层都必须登记到本表，不允许散落在渲染或玩法
系统中。

## 当前兼容边界

| 领域 | 旧数据 / 旧设计 | 最新处理 | 状态 |
|---|---|---|---|
| 场景格式 | `scene_version=1`，无 3D 组件 | `Scene::from_text` 只提升到 v2 并记录 `migrated_from=1`，组件负载不改写 | 保留只读迁移 |
| 资源数据库 | TSV v1 状态和列集 | `load()` 迁移到 TSV v2；未支持版本拒绝；损坏库隔离恢复 | 保留只读迁移 |
| 存档格式 | 无版本头视为 v0；`CURRENT_SAVE_VERSION` 之后升级 | 加载时交给 Lua `on_migrate(from, to)`；保存总是带当前版本头 | 保留只读迁移 |
| Sprite | 旧短格式 `<texture>;size=WxH` | `from_component_str` 识别旧短格式；结构化字段总是序列化为 `texture=...` | 保留只读迁移 |
| Text 字号 | 旧 `scale` 倍率 | `from_component_str` 接受 `scale` 并换算为 `font_size`；`to_component_str` 只写 `font_size` | 已删除写入端 |
| UI 事件 | 空的 `clickEvent` 标记和固定 `on_click` 回调 | 场景实例化时转换为 `UiEvents::click("on_click")`；运行时命中目标携带 `UiEvents` 的回调列表，不再有 `ClickEvent` ECS 标记 | 已删除运行时旧路径 |
| UI 布局 | `Text` / `Image` 内联 `x/y/anchor/size` | 新路径使用 `UiRectLayout`；旧内联数据通过 `UiRectLayout::from_point` 映射。Text 的实测尺寸仍依赖视觉组件，内联布局读取暂不能删除 | 保留只读迁移 |
| Button 组件 | 旧 `Button` 场景组件字符串 | 运行时已移除 Button 渲染组件；编辑器用 `Image + Text + UiEvents` 组合表达按钮，视觉状态用 `ButtonStates` | 已删除编辑入口 |

## 已删除的旧写入路径

1. 编辑器和 Widget Palette 不再新增 `clickEvent` / `Button` 组件；示例场景已迁移
   到 `UiEvents`。
2. `UiEvents` 是唯一运行时事件组件；点击分发按绑定的函数名调用，不再隐式只调用
   `on_click`。
3. `Text` 保存不再输出 `scale`；编辑器读取旧倍率后只编辑和写回 `font_size`。

## 不允许保留第二套实现的位置

渲染、点击命中、Inspector、History、序列化和 Lua 桥都不应直接判断旧字段或旧
组件名。旧数据必须先在 `from_component_str`、`Scene` / 资源库 / 存档的加载迁移
处转换。唯一例外是当前还不能无损迁移的 `Text` / `Image` 内联布局；它们只能由
共享布局解析读取，不能在渲染层再复制一套坐标计算。

## 后续迁移

- 场景 v3：把 Text / Image 的布局字段完全迁入 `UiLayout`，把 `UiEvents`、
  `UiBindings`、`ButtonStates` 提升为类型化组件负载。
- 组件 Schema v2：删除 Text 的旧 `scale` 字段读取入口。
- 运行时路径：迁移完成后删除 `UiRectLayout::from_point` 对旧内联数据的兜底。
