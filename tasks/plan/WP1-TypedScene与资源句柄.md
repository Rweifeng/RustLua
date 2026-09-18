# 任务：WP1 Typed Scene 与资源句柄（原型 → 可启用门）

- 任务 ID：WP1（工作包级；子任务 WP1-1 ~ WP1-5 可各自认领）
- 状态：**WP1-1 ~ WP1-5 全部完成（2026-09-17）**，达到可启用门；WP1-5 依赖的 WP0-6 增量基准同轮完成（见 [docs/53](../docs/53-WP0增量场景帧基准与每帧分配.md)）
- 关联里程碑：引擎全面重构 [docs/40](../docs/40-引擎全面重构计划.md) §3 WP1；设计原型 [docs/45](../docs/45-WP1-WP2设计与原型.md)
- 创建日期：2026-09-17
- 前置依赖：WP0（已关闭，[docs/46](../docs/46-WP0关闭报告.md)）

## 目标

把 [docs/40](../docs/40-引擎全面重构计划.md) §3 WP1 的 typed compile 从"未接线的原型"推进到可启用门：源格式到运行时 typed 数据的单一边界成立、可对拍、可回退、有前后证据。

## 0. 现状核实（2026-09-17，以源码为准）

| 事实 | 依据 |
|---|---|
| 原型是**未被引用的死代码**：全仓符号命中只在两个文件自身与 `core/mod.rs` 声明内 | `engine/crates/engine/src/core/mod.rs:92,109` |
| 原型只覆盖 5 个组件 | `core/typed_scene.rs:16-22`（Transform2D/Transform3D/Camera3D/Light/MeshRenderer） |
| **AssetId 口径与资源库不一致**：原型用 `from_seed(path)`，资源库用 `from_seed("{path}:{importer_version}")` | `core/typed_scene.rs:100` vs `core/resource_db.rs:274` |
| **计划前提需修正**：运行帧 `from_component_str` 已为 0，不是每帧解析 | 唯一命中 `host/runtime_scene.rs:2229,2233`，位于只在 init 调用一次的 `build_ui_scene_nodes`（`host/runtime_scene.rs:585`） |
| 运行时装载在加载边界已 typed | `core/script.rs:593` `spawn_entity_components` |
| 字符串真实残留三处：编辑器每帧重绘、Lua 写 API、加载边界 | `editor/src/panels/scene_view.rs:785,1721`；`core/script.rs:2451-2453,2692-2696`；`core/script.rs:593` |

### 真实场景组件普查（26 个源场景，已排除 `builds/` 产物）

| 组件 | 使用文件数 | 原型是否覆盖 |
|---|---:|---|
| Transform | 24 | 是 |
| UiLayout | 23 | 否 |
| Text | 23 | 否 |
| Sprite | 15 | 否 |
| Tilemap | 12 | 否 |
| Collider | 11 | 否 |
| Animation | 10 | 否 |
| UiEvents / Image | 6 / 6 | 否 |
| Light / Camera3D / Transform3D / MeshRenderer | 各 3 | 是 |
| Collider3d / Animation3D | 各 2 | 否 |
| ButtonStates / Container | 各 1 | 否 |

结论：原型恰好覆盖了使用最少的三个组件，**覆盖优先级必须按上表用量排序**，而不是按 3D 先行。

## 子任务清单

### WP1-1 扩展 typed 组件覆盖（约 2 人日）

**状态：已完成（2026-09-17）**——覆盖面 5/17 → 17/17，验收项全部满足。

按上表用量顺序补齐：Transform → UiLayout / Text → Sprite → Tilemap → Collider → Animation → UiEvents / Image → 3D 组 + Collider3d / Animation3D / ButtonStates / Container。

复用既有解析器，不新写解析逻辑：`core/sprite.rs:146`、`core/tilemap.rs:393`、`core/ui.rs:417,487`、`core/ui_layout.rs:144,388`、`core/physics.rs:54`、`core/physics3d.rs:45`、`core/animation.rs:191`、`core/animation3d.rs:94`、`core/ui_states.rs:51`。

验收：17 个组件名全部可编译；未知组件仍留在源场景并显式跳过（不静默解释）。

### WP1-2 接真实 AssetId（约 0.5 人日）

**状态：已完成（2026-09-17）**。

实际做法比原计划更保守：新增 `AssetResolver` trait，并让 `ResourceDatabase` 实现它（先 `find_by_path` 精确命中，再退化为 `find_by_path_ci`，与 `core/dependency.rs` 的解析口径一致）；`TypedComponent::MeshRenderer.asset` 由 `AssetId` 改为 **`Option<AssetId>`**，`None` 明确表示"未解析"。

原计划只是"删除 `from_seed` 兜底"，但那样在无资源库时没有合法取值可填。改为 `Option` 后，"没有库"和"库里没有这条记录"都不再需要用编造的 ID 顶替——这直接消除了"把未解析误当真实身份"的可能。新增入口 `TypedScene::compile_v2_with_assets(scene, Option<&dyn AssetResolver>)`，原 `compile_v2` 委托给它并传 `None`。

验收：新增回归保护断言 `registered != AssetId::from_seed(path)`——锁定种子口径差异，防止 seed fallback 被重新引入；同时断言有库时得到库内 ID、无库时为 `None`。

### WP1-3 全场景对拍（约 1 人日）

**状态：已完成（2026-09-17）**。

落地为单测门 `compiles_every_project_scene_and_matches_serialized_extraction`，遍历两个工程的全部真实源场景：`examples/game_api/scenes`（18 个）与 `examples/game_jhyt/scenes`（8 个），共 **26 个**。

三类断言：① 每个场景都能**无诊断**编译；② 源组件总数 == typed 组件总数——若某个组件名未被覆盖，这里会差出来，覆盖缺口无处可藏；③ 3D drawable 与旧 `extract_serialized_scene_3d` 的数量、名称、模型矩阵（1e-5 容差）、mesh 路径一致，且相机/光源实体集合逐一相等。

场景目录缺失时**不静默通过**（`read_dir` 失败即 panic，末尾断言 `scene_count > 0`）——本门的意义就是真的跑过全部工程场景，静默跳过等于把门变成摆设。

验收：26 个场景全绿，未发现任何覆盖缺口或提取差异。

### WP1-4 编辑器接线（范围已按证据修正，约 1 人日）

**状态：已完成（2026-09-17）**。

落地内容：

- `engine/crates/editor/src/main.rs`：新增 `SceneView3dTyped`（按 revision 缓存的 typed 提取：`revision` + `entities` + `diagnostics`）与 `SceneView3dTypedEntity`；`EditorApp` 加字段 `scene_view_3d_typed`。
- `engine/crates/editor/src/panels/scene_view.rs`：新增 `ensure_scene_view_3d_typed()`（revision 未变直接返回，变了才重新编译），并把 `draw_scene_view_3d` 的实体标记列表与 3D 渲染回退路径都改为消费该缓存——`extract_serialized_scene_3d` 在编辑器里已无调用点。
- `engine/crates/editor/src/actions.rs`：`invalidate_scene_view_state` 一并重置 typed 缓存（切文档时两个文档的 revision 可能都是 0，不清会沿用上一份文档的实体列表）。
- `engine/crates/engine/src/core/typed_scene.rs`：新增 `compile_v2_lenient`。编辑器不能因为一个坏组件就让整个 3D 视图空白，因此与 `compile_v2` 的全有或全无并存：宽容模式丢弃非法组件、记录诊断，父链错误只记录不中断；好实体照常可用。

验收：新增编辑器测试 `scene_view_3d_typed_cache_follows_document_and_resets_on_switch`，覆盖"首次提取填充 / 同 revision 命中缓存（清空当探针）/ 切文档整体失效 / revision 自增后重新提取"四条。`typed_scene` 侧新增 `lenient_compile_keeps_good_entities_and_reports_bad_ones`。

**范围修正（2026-09-17，重要）**：原计划的"`RuntimeScene` 加 feature flag 双路径"经核实**不应实施**，理由三条：

1. 运行时装载**本来就在加载边界只解析一次**：`core/script.rs:593` 的 `spawn_entity_components` 是全场景组件唯一的解析点，之后世界内只有 typed 组件；运行帧不解析（唯一命中在只在 init 调用一次的 `build_ui_scene_nodes`）。
2. 若把 typed 编译**并列**挂到装载路径，每个组件会在装载时被解析两次（typed 一次 + spawn 一次），净增工作量——违反 docs/40 §9.2"新增字符串解析量不小于被替换量时，优先选实现更简单的一侧"。
3. 若改为**替换** spawn 的解析（真正的一次解析），要动 `instantiate_scene`、持久实体重生（`script.rs:940`，其快照存的是字符串）、物理刚体创建与名字注册；而帧内收益为 0，风险却与 WP2/WP9 的改动面重叠。

**遗留（记入 TODO，需 WP0-6 基准裁量）**：若 WP0-6 的 typed compile / 场景加载基准证明装载期解析确为瓶颈，再按上面第 3 条做真正的替换式改造。

### WP1-5 证据与口径修正（约 0.5 人日）

**状态：已完成（2026-09-17，依赖 WP0-6，后者同轮完成）**。

口径按证据修正为三份可复核的材料，而不是"证明 typed 更快"（它并不更快）：

1. **每帧分配量尺**（`wp0-v4`，`artifacts/baselines/wp0-scene-frame.json`）：1k/10k/50k Sprite 的帧 p50 为 1.01/11.5/65.3 ms，每帧分配 3,736/36,739/183,407 次、1.11/13.25/59.24 MB，折算约 **3.67 次分配与 1,325 字节 / 精灵 / 帧**。这是 WP1-4/WP3/WP6 的共同对照基线。
2. **typed 编译 vs 旧提取的 A/B**（同一 `Scene` 上直接计时）：含 3D 的场景里 typed 编译是旧 `extract_serialized_scene_3d` 的 **1.33–2.00 倍**（demo3d 4 vs 3 µs、game3d 11 vs 7 µs、physics3d 4 vs 2 µs）。原因是旧提取只解析 4 类组件、typed 覆盖 17 类。**编辑器的收益来自缓存（每帧一次 → 每 revision 一次），不是来自编译速度。**
3. **装载期成本的上界**：单场景最大 typed 编译 85 µs（`game_api/production`，23 实体 / 81 组件），约占 60 fps 帧预算的 0.5%。这构成 WP1-4 范围修正的量化依据。

验收：报告落 `artifacts/baselines/`，沿用 WP0 既有 schema 与 `verify.ps1 -Benchmarks` 的 `wp0-v4` 契约校验。

**如实登记的限制**：编辑器重绘耗时**没有仪器化量测**——缓存收益目前只由"调用频次从每帧一次降到每 revision 一次"加上失效语义测试推出，**没有前后性能数字**。若要把它变成实测结论，需要给编辑器加采样（属 WP9 范围）。不得把 §1 的帧分配数据记作 WP1 的运行时收益。

## 影响文件

- `engine/crates/engine/src/core/typed_scene.rs`（主要改动）
- `engine/crates/engine/src/core/mod.rs`
- `engine/crates/engine/src/host/runtime_scene.rs`（feature flag 与双路径）
- `engine/crates/editor/src/panels/scene_view.rs`（消费 typed 数据）
- `engine/crates/engine/src/core/resource_db.rs`（只读复用其 ID 口径）

## 风险

- **Lua 动态组件 / 热重载 / 自定义组件兼容**：按场景启用 typed 编译，失败回退解释器，旧 v2 只读保留。
- **AssetId 变更影响依赖图与缓存**：走资源库口径是修正而非改写，但需确认 `assetc` 与编辑器依赖图同步（`core/dependency.rs`）。
- 编辑器接线会触及每帧 UI 路径，需保证 Play/Stop 与预览不污染保存（对齐 WP9 的验证范围）。
- 若把"运行帧解析=0"当作成果上报，会产出**假绿灯**——明确禁止。

## 验收方式

```powershell
cd engine
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace
```

外加：26 个源场景对拍脚本输出、编辑器重绘前后报告、`scripts/verify.ps1 -Benchmarks` 通过。

## 实现记录

### WP1-1（2026-09-17）

改动文件：`engine/crates/engine/src/core/typed_scene.rs`（唯一实现文件）。

设计：把原来的 17 路 `match` 内联在 `compile_v2` 里改为独立的 `compile_component(name, value) -> Option<Result<(ComponentId, TypedComponent), String>>`——`None` 表示未知组件（留在源场景、不解释），`Some(Err)` 表示已知组件载荷非法（必须上报诊断）。这样"未知识别"和"非法诊断"两条语义各有唯一出口，新增组件只改一处。

`ComponentId` 保持 2D/3D 变体分开（`Transform2D`/`Transform3D`、`Collider`/`Collider3d`），不合并，符合 docs/40 §5 禁止把两个组件当作一个兼容槽。

`UiEvents` 的解析入口是 `UiEvents::parse`（不是 `from_component_str`），单独接线。

`MeshRenderer` 的 `AssetId` 仍是 `from_seed(path)`，已在代码里留 `TODO(WP1-2)` 注明它**不等于**资源库 ID，防止被误当资源身份使用。

新增 3 个测试：`covers_every_component_name_used_in_source_scenes`（表驱动锁定 17 个名字 → ComponentId 映射，并抽样断言载荷真被解析）、`unknown_components_are_left_in_the_source_scene`（未知组件既不入 typed 也不报诊断）、`invalid_payload_of_any_known_component_is_diagnosed`（11 个新增组件族的非法载荷逐一必须报诊断且带组件名）。

### WP1-2（2026-09-17）

改动文件：`engine/crates/engine/src/core/typed_scene.rs`。

新增 `AssetResolver` trait + `impl AssetResolver for ResourceDatabase`（`find_by_path` → `find_by_path_ci` 兜底）；`TypedComponent::MeshRenderer.asset` 改为 `Option<AssetId>`；新增 `compile_v2_with_assets`，`compile_v2` 委托传 `None`。新增测试 `mesh_asset_resolves_through_the_resource_database`。

### WP1-3（2026-09-17）

改动文件：`engine/crates/engine/src/core/typed_scene.rs`。

新增对拍门 `compiles_every_project_scene_and_matches_serialized_extraction`：遍历 `game_api`（18）与 `game_jhyt`（8）共 26 个真实源场景，断言无诊断编译、源/typed 组件计数相等、3D drawable 与相机/光源集合与旧提取一致。实测 26 个场景全部通过。

### WP1-4（2026-09-17）

改动文件：`engine/crates/editor/src/main.rs`、`engine/crates/editor/src/panels/scene_view.rs`、`engine/crates/editor/src/actions.rs`、`engine/crates/engine/src/core/typed_scene.rs`。

- `main.rs`：新增 `SceneView3dTyped`（`revision` + `entities` + `diagnostics`）与 `SceneView3dTypedEntity`；`EditorApp` 加字段 `scene_view_3d_typed`。
- `scene_view.rs`：新增 `pub(crate) fn ensure_scene_view_3d_typed()`（revision 未变直接返回）；`draw_scene_view_3d` 的实体标记列表与 3D 渲染回退路径改为消费该缓存。**`extract_serialized_scene_3d` 在编辑器里已无调用点**（import 一并删除）。
- `actions.rs`：`invalidate_scene_view_state` 一并重置 typed 缓存。
- `typed_scene.rs`：新增 `compile_v2_lenient` + 抽出 `compile_entities` / `parent_first_order`。父链出错实体在回退时标记为已完成，避免同一错误被外层循环重复上报。严格模式行为保持：先报组件诊断，再报父链诊断，`errors[0]` 不变。
- 3D 渲染回退路径不再提前 `current_scene()?`：那会让不可变借用跨过后面的可变调用，改为交给既有的 `drawables.is_empty()` 统一返回 None（行为等价，且消除 clippy `question_mark`）。

新增测试：`typed_scene::tests::lenient_compile_keeps_good_entities_and_reports_bad_ones`；`editor::actions::tests::scene_view_3d_typed_cache_follows_document_and_resets_on_switch`（覆盖首次填充 / 同 revision 命中缓存 / 切文档失效 / revision 自增后重算）。

测试期间发现：切文档前若文档 dirty，会被 E06 未保存保护拦下而**实际不切文档**，因此该用例先验证切文档失效、再验证 revision 自增重算。

### 本轮验收证据（2026-09-17）

```powershell
cd engine
cargo fmt --all -- --check          # exit 0
cargo clippy --workspace --all-targets --all-features -- -D warnings   # exit 0
cargo test -p engine --lib          # 566 passed / 0 failed
cargo test -p editor --bin editor   # 161 passed / 1 failed（环境性，见下）
cargo build --workspace             # exit 0
```

`scripts/check-docs.ps1` → `check_docs_ok docs=46 links=236`。

**已知环境性失败（非本次改动引入）**：`editor::publish::publish_tests::publish_spawn_end_to_end` 需要已存在的 release 产物，实测报 `SkipBuild requires existing release artifact: engine\target\release\assetc.exe`。章程 §10 规定 release 构建由负责人手动执行，因此该用例在未做 release 构建的检出上必然失败；已在本轮补做 `cargo build --workspace --release` 以消除该环境前置。

## 验收证据

- [x] `cargo fmt --all -- --check` 通过
- [x] `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过
- [x] 相关测试通过（17 组件覆盖、AssetId 真实身份、26 场景全量对拍、宽容编译、编辑器 revision 缓存）
- [x] Debug 构建通过
- [ ] Demo / 手工步骤可复现（编辑器打开 `examples/game_jhyt` 并重绘场景视图）——**未执行**：本轮无 GUI 交互验收，仅以用例覆盖缓存语义
- [x] 注释符合 [章程 §9.7 代码与注释规范](../docs/00-project-charter.md#97-代码与注释规范)（精简中文）
- [x] 文档与 CHANGELOG 已更新

## 未完成项 / 已知限制

- **WP1-5（证据）未完成**：需先做 WP0-6 的 typed compile / 场景加载 / 首帧 / 每帧分配基准；缺该指标时 WP1-5 只能出编辑器耗时证据。因此本包尚未达到"可启用门"的完整证据要求。
- 编辑器缓存的性能收益**尚无前后实测数据**（缺 WP0-6 的每帧分配与重绘耗时指标）；当前只证明了正确的失效语义，不得据此宣称性能已改善。
- 本工作包**不影响运行帧解析**（已为 0）；运行帧收益由 WP3/WP6 兑现，此处不得重复计入。
- Lua 组件写 API 仍是字符串编码（`core/script.rs:2451-2453,2692-2696`），改 typed 属 WP9 Lua facade 范围，不在本包。
- 编辑器 GUI 未做人工验收；3D 视图标记位置依赖 `TypedScene` 与旧 `extract_serialized_scene_3d` 同集合（两者在 26 个真实场景上已逐项对拍一致）。
