# 任务：编辑器 Unity 对标彻底优化（第 1 批）

- 任务 ID：M87-00
- 状态：已验收（2026-11-27 归档至 `done/`）
- 关联里程碑：M87（编辑器 Unity 对标）
- 创建日期：2026-11-27

## 目标

按 Unity 编辑器的日常工作流复核 RustLua 编辑器，先出一份**带 file:line 证据**的缺口台账，
再把其中"确定性高、可无头验证"的一批真正落地：控制台不再冻结主线程、视口内 F 聚焦、
多选批量复制/移动、Inspector 搜索与混合值可读。

## 影响文件

- `engine/crates/editor/src/panels/console.rs`：sink 快照按代数缓存 + `show_rows` 真虚拟化 + 跟随尾部
- `engine/crates/editor/src/main.rs`：profiler gauge 不再每帧 clone 日志缓冲
- `engine/crates/editor/src/panels/scene_view/frame.rs`（新增）：聚焦的纯几何 + 聚合入口
- `engine/crates/editor/src/panels/scene_view/{mod,draw,view_3d,view_state}.rs`：接线、共用口径
- `engine/crates/editor/src/panels/hierarchy/navigation.rs`：F 改走统一入口
- `engine/crates/editor/src/hierarchy_ops.rs`：`ReparentTarget` + `commit_reparent_batch` + 批量复制
- `engine/crates/editor/src/actions/components.rs`：`reparent_selected` 走批量内核
- `engine/crates/editor/src/schema_fields.rs`：`SchemaFieldRow.mixed_values`
- `engine/crates/editor/src/panels/schema_inspector/panel.rs`：搜索框常驻 + 混合值提示
- `engine/crates/editor/src/perf_baseline.rs`：100k 控制台场景真断言"只画视口行"
- 文档：`docs/85-编辑器Unity对标优化第2轮.md`、`docs/README.md`、`docs/changelog/最新条目.md`

## 风险

- 控制台改 `show_rows` 会改变"每帧行数"读数：`perf_baseline` 的密度门与 `framegate` 的
  行高门都读同一批探针，所以**同时**补了一条"绘制行数 < 200"的真断言（以前那条只是注释）。
- 聚焦统一了 2D pan 的符号约定（旧实现 Y 写成 `+center.y*zoom`）：`fit_scene_view_to_design`
  与聚焦现在共用 `pan_centered_on`，用"投影回面板中心"的往返测试钉住。
- 批量换父抽了提交内核：拖放与右键菜单会走同一份 keep-world 校验，回滚语义由事务保证。
- 新面板文件会触发 `panels_only_emit_actions` 的登记要求（已登记 `scene_view/frame.rs`，0 处写入）。

## 验收方式

```powershell
cd engine
cargo check -p editor --all-targets
cargo test -p editor --bin editor panels::scene_view::frame
cargo test -p editor --bin editor panels::hierarchy::tests
cargo test -p editor --bin editor panels::schema_inspector
cargo test -p editor --bin editor panels::architecture_tests
cargo test -p editor          # 全量（795+）
cargo run -q -p editor -- --perf-baseline --perf-report ..\artifacts\metrics\m87-perf-after.json
cargo run -q -p editor -- --gui-smoke
cargo fmt --all -- --check
cargo clippy -p editor --all-targets -- -D warnings
```

对比读数：`artifacts/metrics/m87-perf-before.json` 与 `m87-perf-after.json` 的
`console_panel_frame_100k` 段。

## 实现记录

见 [docs/85](../../docs/85-编辑器Unity对标优化第2轮.md) §4。要点：

1. **Console 不是虚拟滚动**：注释写着"只画视口行"，实现是 `for index in 0..row_count`
   逐行建 Label。100k 推送 / 10k 保留时 p95 = **120.96 ms**，每帧画 10000 行。
   改成 `ScrollArea::show_rows` + `stick_to_bottom(true)`，并把 sink 文本快照按
   `LogSink::generation` 缓存（稳态帧不再 clone 上万条字符串）：
   p95 = **0.529 ms**、每帧绘制 **9** 行（同一台机器、同一 debug 档）。
2. **profiler gauge** `snapshot().len()` → `len()`：每帧省掉一次整条日志缓冲的 clone。
3. **视口内 F 聚焦**：抽出 `FocusBounds` / `framed_distance` / `framed_zoom_2d` /
   `pan_centered_on`；层级面板 F 与 2D/3D 视口 F 共用一个入口，按**选中集合并集包围盒**适配。
   旧实现 3D 只改 target 不调距离、2D 只改 pan 不调 zoom，且 Y 符号写反。
4. **多选批量**：`duplicate_selected_entities` 复制整个选中集合的顶层根（一条 Undo、复制体成为新选择）；
   `reparent_selected` 与拖放共用 `commit_reparent_batch`（整批校验 / keep-world / 整体回滚）。
5. **Inspector**：搜索框在 0 命中时不再消失（旧实现先 `rows.is_empty()` 早退、连清除按钮一起藏），
   0 命中给空状态引导；混合值从"混合"两字改为"——"占位 + 悬停列出全部不同取值。

## 验收证据

- [x] `cargo fmt --all -- --check` 通过（见 §6 读数；`view_3d.rs` 历史 CRLF 已在同笔转 LF）
- [x] `cargo clippy -p editor --all-targets -- -D warnings` 通过
- [x] 相关测试通过（frame 5/5、hierarchy 21/21、schema_inspector 29/29、architecture 3/3）
- [x] 全量 `cargo test -p editor` 通过（796 passed / 0 failed / 2 ignored）
- [x] Debug 构建通过（`cargo build -p editor`）
- [x] `--perf-baseline` 37 场景 `failed=0`；`--gui-smoke` **cases=43 failed=0 result=ok**
- [x] `scripts\verify\check-docs.ps1` 通过（`docs=79 h1=81 links=509`）
- [x] 注释符合章程 §9.7（精简中文，写"为什么"与边界）
- [x] 文档与 changelog 已更新

## 未完成项 / 已知限制

本批**没有**做（按"只登记，不开新包"记在 [docs/85](../../docs/85-编辑器Unity对标优化第2轮.md) §5）：

1. **Lua 断点调试 UI**（断点列/设删、步入步过、Locals/Watch）：内核缺 `PauseReason::Pause`、
   同步 handler 与暂停期求值，属 L 成本，需先扩内核协议。
2. **层级面板"揭示选中"**（从场景视图选中后自动展开祖先 + 滚动到行）：需要面板滚动状态跨帧，
   本批只登记。
3. **控制台折叠/去重 + ×N 计数**、级别多选、崩溃栈帧点击跳转。
4. **资源：改名迁移 `resources.meta` 键**（当前改名会丢导入设置）、删除守卫补扫 `.material`/`.atlas`/`.spine`、
   浏览器类型筛选/多选。
5. **场景视图**：3D 网格与原点轴、选中轮廓跟真实包围盒、Center 轴心真正生效、2D 手柄、正交/透视切换。
6. **预制体重名静默覆盖**（`scene_ops.rs` 直接以实体名落盘）：数据丢失风险，下一批优先。
7. 真实 GPU 会话下的人工观感（暂停图标、DPI 截图）与 8 小时 soak 仍不在本批。
