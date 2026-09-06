# 兼容性设计清单

P2 的当前格式就是唯一运行时格式。加载边界只接受当前版本和当前字段；
旧版本、旧字段、旧组件名和旧 API 都会明确失败，而不是在渲染、Inspector、
玩法脚本或保存路径里悄悄转换。

## 当前规则

1. `Scene` 只读写 `scene_version=2`。更低或更高版本由
   `SceneError::UnsupportedVersion` 拒绝，没有自动迁移和 `migrated_from` 元数据。
2. 资源数据库只读取 `# rustlua-resource-db-v2`；v1 或未知版本必须重建或隔离。
3. 存档必须有 `save_version=1`；缺失、更高或更低版本都加载失败。
4. `Sprite` 只接受结构化字段，且首字段必须是 `texture=...`；旧短格式
   `<texture>;size=WxH` 不再识别。
5. `Text` 只接受 `text`、`font_size`、`color`；旧 `scale`、`x`、`y` 不再读取。
6. `Image` 只携带 `texture`、`color`、`nine_slice`；位置、尺寸、空间和锚点
   一律由 `UiLayout` 提供。
7. `UiLayout` 是场景 UI 的唯一几何来源，字段为 `anchors`、`offsets`、
   `min_size`、`max_size`、`pivot`。
8. `UiEvents` 是唯一 UI 事件组件。`clickEvent` 和 `Button` 事件标记已删除；
   按钮用 `Image + Text + UiEvents` 表达，视觉状态用 `ButtonStates`。
9. 默认示例与验收工程是仓库内 `examples`。旧的 `game_showcase`、
   `game_test` 系列不是运行时兼容目标。
10. Prefab 只读写 `prefab_version=1`；导入缓存只使用当前 `v1` 缓存头。

## 保留的错误边界

以下不是兼容层；它们的作用是让坏数据尽早失败并给出可定位的错误：

- 版本头解析和版本拒绝；
- 组件字符串的 `from_component_str` 严格校验；
- 导入缓存 schema 头校验；
- 资源库损坏库隔离、导入器版本失效和依赖状态传播；
- 存档槽位命名、损坏文件拒绝和事务提交。

## 新增格式的流程

需要演进格式时，必须同步完成以下工作：

1. 提升版本号并更新当前解析器、保存器、Inspector 和测试；
2. 迁移仓库内示例、文档和打包检查；
3. 删除旧解析分支；
4. 在本文件记录“已删除的旧设计”，而不是登记一个长期兼容层。

## 已删除的旧设计

| 旧设计 | 当前处理 |
|---|---|
| `scene_version=1` 自动提升 | 只接受 `scene_version=2` |
| 资源数据库 v1 | 只接受 v2；旧库明确失败 |
| 无版本头存档 / Lua `on_migrate` | 必须有 `save_version=1` |
| Sprite 短格式 | 只接受 `texture=...;size=...` |
| Text 内联几何和 `scale` | `Text` 只存视觉属性，`font_size` 是像素字号 |
| Text / Image 内联 `x/y/width/height/space/anchor` | `UiLayout` 四边 Anchor + Offset |
| `UiRectLayout::from_point` | 删除；`UiLayout` 不再有单点布局输入 |
| `clickEvent` / `Button` ECS 标记 | `UiEvents` 结构化绑定 |
