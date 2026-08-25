# M13 里程碑验收证据（2D 内容生产工作流）

日期：2026-08-25
范围：本次归档覆盖 `M13-00` ~ `M13-06` 的当前最小闭环实现，即 2D 场景无需手改 scene/prefab 文件即可完成
Sprite / Tilemap / Animation / 排序 / Prefab / UI 的创建、编辑、保存、重开与运行。

## 总状态

**代码 / 自动化侧：PASS。**
**编辑器工作流侧：PASS（以 `editor` / `game` check 和当前自动化测试为证据）。**
**未纳入本次归档：** 原始规划中的高级增强项，例如图集区域可视选择、Tilemap 矩形/吸管/填充、动画事件标记、
视图书签/快照、UI 样式资源系统。这些不阻断当前 M13 里程碑的最小生产闭环，转后续增强。

## M13-00 / M13-01：2D 数据模型收敛 + Sprite 编辑与批量创建

- `SpriteComponent` 已统一 `texture / size / pivot / tint / sorting_layer / order`，并兼容旧格式：
  `engine/crates/engine/src/core/sprite.rs`
- 运行时、Inspector、Lua/场景实例化已使用同一套 Sprite 语义：
  `engine/crates/engine/src/core/script.rs`
  `engine/crates/editor/src/inspector_state.rs`
  `engine/crates/editor/src/panels/inspector.rs`
- 编辑器已支持：
  - 菜单快速创建 Sprite；
  - Inspector 编辑 Sprite 结构化字段；
  - 多选水平/垂直等间距；
  - Sprite 排序层级增减与按 Y 重排。

自动化证据：

- `cargo test -p engine sprite -- --nocapture`
  - `sprite_component_round_trips_structured_fields`
  - `sprite_component_reads_legacy_format`
  - `sprite_component_rejects_invalid_structured_payload`
- `cargo test -p editor`
  - `actions::tests::distribute_targets_preserves_endpoints_and_even_spacing`
  - `actions::tests::sorted_sprite_order_updates_keep_min_order_and_follow_y`

## M13-02：Tilemap 编辑器

- Inspector 已支持 Tilemap 纹理、尺寸、tiles/solid 数据编辑：
  `engine/crates/editor/src/panels/inspector.rs`
- Scene View 已支持 `TilePaint` 工具、网格叠加、左键绘制 / 右键擦除，并写回场景组件：
  `engine/crates/editor/src/panels/scene_view.rs`
- 运行时继续使用 Tilemap 渲染和碰撞导出路径，无需手写场景数据。

自动化证据：

- `cargo test -p editor`
  - `panels::scene_view::tests::tilemap_pointer_mapping_round_trips_visible_cells`
  - `panels::scene_view::tests::tilemap_pointer_mapping_rejects_outside_and_rotated_maps`

## M13-03：2D 动画编辑器

- `AnimationStateMachine` 组件序列化、状态推进和 Lua 切换已统一：
  `engine/crates/engine/src/core/animation.rs`
  `engine/crates/engine/src/core/script.rs`
- Inspector 已支持默认状态、Clip 列表、sheet/cols/frame_rate/looped 编辑：
  `engine/crates/editor/src/panels/inspector.rs`

自动化证据：

- `cargo test -p engine animation -- --nocapture`
  - `core::animation::tests::round_trips_through_component_string`
  - `core::animation::tests::advances_looped_clip_frames_and_wraps`
  - `core::animation::tests::non_looping_clip_clamps_and_fires_end_event_once`
  - `core::script::tests::instantiated_animation_is_switchable_from_lua`

## M13-04：2D 场景工具和排序

- Scene View 已有 Select / Move / Rotate / Scale / TilePaint 工具：
  `engine/crates/editor/src/panels/scene_view.rs`
- 工具栏已支持：
  - 对齐到网格；
  - 水平/垂直对齐；
  - 水平/垂直等间距；
  - Sprite `order +/-1`；
  - 按 Y 排序。
  `engine/crates/editor/src/panels/toolbar.rs`
  `engine/crates/editor/src/actions.rs`

自动化证据：

- `cargo test -p editor`
  - `actions::tests::distribute_targets_preserves_endpoints_and_even_spacing`
  - `actions::tests::sorted_sprite_order_updates_keep_min_order_and_follow_y`

## M13-05：Prefab 与 2D 内容复用

- 已支持从选中实体创建 Prefab、资源拖入实例化：
  `engine/crates/editor/src/panels/hierarchy.rs`
  `engine/crates/editor/src/panels/scene_view.rs`
- Inspector 已支持：
  - Prefab 来源展示；
  - override 列表；
  - 断链诊断；
  - 重新应用源 Prefab；
  - 写回源并更新同源实例；
  - 打开源 Prefab。
  `engine/crates/editor/src/panels/inspector.rs`
- 文档模型已支持直接打开 / 保存 `.prefab`，形成最小 Prefab 编辑模式：
  `engine/crates/editor/src/document.rs`
  `engine/crates/editor/src/panels/resource_browser.rs`

自动化证据：

- `cargo test -p editor`
  - `panels::inspector::tests::collect_prefab_rebuild_state_preserves_detected_overrides`
  - `panels::inspector::tests::collect_prefab_rebuild_state_rejects_mixed_subtree`
  - `panels::inspector::tests::build_prefab_from_instance_root_strips_prefab_metadata`
  - `document::tests::open_and_save_prefab_document_roundtrip`
  - `document::tests::prefab_document_requires_single_root`

## M13-06：2D UI 生产最小闭环

- `UiText / UiImage / UiButton` 已支持 `Canvas / Screen` 空间与 anchor 布局：
  `engine/crates/engine/src/core/ui.rs`
- 运行时渲染与点击命中已按 anchor / viewport 一致计算：
  `engine/crates/engine/src/host/runtime_scene.rs`
  `engine/crates/engine/src/core/script.rs`
- Inspector 已支持 Text / Image / Button 的 layout space 与九宫格 anchor 编辑：
  `engine/crates/editor/src/inspector_state.rs`
  `engine/crates/editor/src/panels/inspector.rs`
- 编辑器菜单已支持快速创建 UI Text / UI Image / UI Button。

自动化证据：

- `cargo test -p engine core::ui -- --nocapture`
  - `canvas_rect_and_text_origin_follow_anchor_and_viewport`
  - `button_round_trip_and_click_rect`
  - `text_round_trip_preserves_escaped_specials`
- `cargo run -q -p editor -- --check --project "e:\\Codex\\p2\\game_test"`
  - `editor_project_ok ... baseline=v0.10-editor`
- `cargo run -q -p game -- --check --project "e:\\Codex\\p2\\game_test"`
  - `game_check_ok project=e:\\Codex\\p2\\game_test`

## 本轮统一验证

```powershell
cargo test -p engine sprite -- --nocapture
cargo test -p engine animation -- --nocapture
cargo test -p engine core::ui -- --nocapture
cargo test -p editor
cargo run -q -p editor -- --check --project "e:\Codex\p2\game_test"
cargo run -q -p game -- --check --project "e:\Codex\p2\game_test"
```

结果：

- `engine` Sprite 相关测试：12 通过；
- `engine` Animation 相关测试：16 通过；
- `engine` UI 相关测试：7 通过；
- `editor`：72 + 31 通过；
- `editor --check`：通过；
- `game --check`：通过。

## 归档结论

M13 当前已经形成可归档的 2D 内容生产最小闭环：

1. Sprite 结构化编辑与批量排序；
2. Tilemap 直接绘制与保存；
3. 动画状态机编辑与运行时一致；
4. 2D 场景工具、对齐、等间距和排序；
5. Prefab 创建、实例化、override、源编辑与传播；
6. UI Canvas/Anchor/Text/Image/Button 与点击绑定。

这意味着编辑器已经可以在不手改 scene/prefab 文件的前提下，完成基础 2D 内容生产。

## 未纳入本次归档

- 图集区域可视化选择；
- Tilemap 矩形/吸管/填充/图层显隐锁定；
- 动画事件标记、完整时间线预览；
- 2D 视图书签 / 快照；
- UI 样式资源系统。

以上条目保留为后续增强，不在本次 M13 归档范围内。
