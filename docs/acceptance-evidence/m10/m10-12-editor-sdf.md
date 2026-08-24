# M10-12 编辑器优化与 SDF 字体验收证据

日期：2026-08-24
实现提交：

- `63ebff4`（`feat(M10-12): 编辑器优化`）；
- `cdf7448`（`feat(M10-12): 字体渲染优化（TMP 风格 SDF）`）。

基线标签：`v0.10-editor` → M10-10 提交 `86095ed`

## 自动化验证范围

M10-12 涉及编辑器面板和输入逻辑，以及 engine_core/engine_host 的动态字体、SDF 布局和渲染路径。主要文件包括：

- `engine/crates/editor/src/panels/`
- `engine/crates/editor/src/inspector_state.rs`
- `engine/crates/editor/src/resource_browser_state.rs`
- `engine/crates/engine_core/src/font.rs`
- `engine/crates/engine_core/src/text.rs`
- `engine/crates/engine_core/src/ui.rs`
- `engine/crates/engine_host/src/renderer.rs`
- `engine/crates/engine_host/src/runtime_scene.rs`
- `engine/crates/engine_host/src/shader.rs`
- `game/scenes/ui_test.scene`

当前 workspace 测试通过，包含：

- engine_core：字体 atlas、SDF glyph、ASCII/CJK 混合布局测试；
- engine_host：offscreen renderer/readback 测试；
- engine_host：`ui_test.scene` 产生 SDF draw 和 SDF 顶点标记测试；
- editor：文档、Inspector、资源浏览器、恢复和布局相关状态测试；
- 全 workspace：299 项测试执行通过，1 项 ignored，退出码 0。

## 已证明内容

- 动态字体可读取并生成固定尺寸 glyph atlas；
- 文本布局支持 ASCII/CJK 混合 mesh；
- runtime scene 能产生带 SDF 标记的 draw；
- offscreen renderer/readback 测试能产生有效结果；
- editor、engine_host 和 game_runner 可通过 workspace 编译和测试。

## 未证明内容

当前没有新增独立的中文 SDF 截图、像素基准、GPU 型号记录、DPI/resize 记录或 clean-package 字体日志。因此以下内容仍待专项验收：

- 真实窗口中中文 glyph 的可见性、抗锯齿、缩放和字重；
- 不同 GPU/驱动上的 SDF shader 结果；
- 发布包中 `game/assets/fonts/NotoSansSC-VF.ttf` 的路径契约；
- 长时间编辑和重复 Play/Stop 的字体内存增长；
- glyph atlas 达到 256 slot 上限后的行为；
- Scene View 半分辨率缓存恢复和 readback 失败重试；
- 内嵌预览输入 `just_pressed` 边沿语义。

因此 M10-A12 在验收矩阵中保持 `PENDING`，直到真实 GUI/GPU 和发布包证据归档。

## 版本关系

`v0.10-editor` 只指向 M10-10 验收提交 `86095ed`。`63ebff4` 和 `cdf7448` 是其后的 M10-12 增量提交，本证据不表示这些增量已经被该标签覆盖。
