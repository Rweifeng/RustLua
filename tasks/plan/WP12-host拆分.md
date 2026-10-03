# 任务：WP12 `host/` 模块拆分（两个上帝文件 → 子目录）

- **状态**：待认领（BACKLOG）
- **优先级**：P1（`docs/62` §6 已登记两项，本文件把它们合成一个工作包）
- **前置**：无。可独立开工，不依赖 P0 的 PBR/HDR（但 PBR/HDR **应当**在本工作包之后启动，理由见 §1）
- **关联文档**：`docs/62`（成熟引擎差距分析）§1.1/§6、`docs/60`（全面复盘与优化项清单）§3/§4、`docs/54`（2D/3D 统一）
- **范本**：`tasks/done/WP11-script-rs拆分.md`（同类工作，12k 行单文件 → 子目录，已验收，本文件大量结论直接复用它的裁定）

---

## 1. 目标

`engine/crates/engine/src/host/` 共 **20,570 行 / 9 个文件**，其中两个文件占掉 80%：

| 文件 | 实测行数 |
|---|---|
| `host/runtime_scene.rs` | **10,458** |
| `host/renderer.rs` | **6,089** |

目标是把它们拆成**单文件不超过 1,500 行**的子目录形态，且：

- **行为零变化**：不修 bug、不做性能优化、不合并重复、不改任何算法。
- **对外符号零变化**：`pub` 面一个字节不动（编辑器 / `game` / `game_web` / `assetc` 零改动）。
- **Lua API 面零变化**：`lua_api_overrides.json` 与两个平台适配层不出现 diff。

**为什么必须排在 PBR/HDR 之前**：`docs/62` §6 列出的四项渲染差距（PBR + IBL、HDR + tonemap、阴影、shader 变体）**全部要写进 `renderer.rs` 这一个文件**。以现状（6,089 行、其中 `impl Renderer` 一块从 574 行铺到 4,005 行、`render_passes` 单函数 446 行）承接 4–6 周的新管线，等于继续往同一个文件里加代码——评审与回滚都会失去边界。

---

## 2. 现状解剖（2026-10-03 实测，认领后仍需复核）

口径：行数用 `[System.IO.File]::ReadAllLines(...).Count`（`Get-Content | Measure-Object -Line` 在中文文件上会少算约 13%，**不要用它**）。

### 2.1 `host/` 全貌

| 文件 | 行数 | `#[test]` |
|---|---|---|
| `runtime_scene.rs` | 10,458 | 100 |
| `renderer.rs` | 6,089 | 23 |
| `gamepad.rs` | 1,599 | 22 |
| `sample_project.rs` | 672 | 9 |
| `keymap.rs` | 578 | 4 |
| `runtime_scene/text.rs` | 569 | 0 |
| `frame.rs` | 302 | 0 |
| `shader.rs` | 272 | 0 |
| `mod.rs` | 31 | 0 |
| **合计** | **20,570** | **158** |

`runtime_scene.rs` 的成分：**4,915 行生产代码 + 5,543 行测试**（`//` 注释 1,437 行、空行 412 行）。

### 2.2 `runtime_scene.rs` 的实测行段

**生产代码 1–5,063**：

| 行段 | 行数 | 内容 |
|---|---|---|
| 1–93 | 93 | 模块文档 + `use`（含 `mod text;`）+ `FrameView3d` |
| 94–375 | 282 | `pub struct RuntimeScene` 字段（**私有**）+ `PhysicsStageOrder` / `FrameScheduleReport` / `CachedTileSprite` / `TileChunk` / `TextRenderMode` |
| 376–1,047 | 672 | `impl RuntimeScene`：构造 4 个入口 + 变换/空间/物理/可见性/并行度访问器 |
| 1,048–1,294 | 247 | `impl`：存档预览、碰撞线框调试绘制 |
| 1,295–1,778 | 484 | `impl`：`init`（165）+ `register_builtin_frame_systems`（309） |
| 1,779–1,889 | 111 | `impl`：相机同步 / 2D 相机步进 |
| 1,890–2,185 | 296 | `impl`：`run_script_steps` / `run_pipelines` |
| 2,186–2,271 | 86 | `impl`：UI 点击目标收集 |
| 2,272–2,583 | 312 | `impl`：`scene_3d` / `scene_3d_with_camera` / 生效相机 / 生效光源 |
| 2,584–3,183 | 600 | `impl`：**`vertices()`**（每帧顶点构建总入口） |
| 3,184–3,278 | 95 | `impl`：`snapshot_custom_fonts` / `push_particles` |
| 3,279–3,893 | 615 | `impl`：场景 UI 布局 + `push_ui_panels/images/grids/bars/text` + `push_scene_ui` |
| 3,894–4,081 | 188 | `impl`：`texture_id` / `texture_dimensions` / `append_tilemap_vertices` |
| 4,082–4,240 | 159 | 自由函数：`push_screen_line` / `texture_id_of` / `build_ui_scene_nodes` |
| 4,241–4,593 | 353 | 自由函数：tilemap 分块（`TILE_CHUNK_SIZE` / `build_chunk` / `rebuild_dirty_chunks` / 两条 `append_tilemap_layer*` / `sort_world_ranges`） |
| 4,594–4,607 | 14 | 自由函数：`startup_scene_path` / `project_text_render_mode` |
| 4,608–4,756 | 149 | 自由函数：`load_project_vector_atlas` / `load_project_font_atlas` / `prewarm_font_atlas` / `load_custom_*` |
| 4,757–4,822 | 66 | 自由函数：`declared_camera_2d_in` / `first_enabled_ortho_camera_in` / `selected_camera_2d_in` |
| 4,823–5,063 | 241 | 自由函数：`sprite_screen_rect` / `load_runtime_scene` / `merge_adjacent_draws` / `text_to_ndc_with_offset` / `nine_slice_quad` / `screen_rect_ndc` |

**测试 5,064–10,458（4 个模块，100 项）**：

| 行段 | 行数 | 模块 | 项数 |
|---|---|---|---|
| 5,064–5,347 | 284 | `tilemap_chunk_tests` | 7 |
| 5,348–10,024 | **4,677** | `sdf_tests` | 83 |
| 10,025–10,409 | 385 | `frame_schedule_tests` | 9 |
| 10,410–10,458 | 49 | `ui_circle_tests` | 1 |

> 测试真实读盘：`sdf_tests` / `frame_schedule_tests` 用 `crate::core::games_dir()`（即仓库根）读 `examples/game_api`、`examples/game_jhyt`，个别用例在工程缺失时自行 `return`。**搬动目录不影响它们**（路径不依赖源文件位置），这是本工作包可以放心搬测试的前提。

### 2.3 `renderer.rs` 的实测行段

**生产代码 1–4,005**：

| 行段 | 行数 | 内容 |
|---|---|---|
| 1–165 | 165 | `use` + `DepthPass` + `WorldSpriteView` + `depth_stencil_state` + `resolve_world_sprite_view` + `hash_bytes` + 2D 容量增长 |
| 166–419 | 254 | GPU 缓存类型（`LoadedTexture` / `CustomFontUpload` / `GpuGeometry` / `CachedAlbedoTexture` / `GpuMaterial` / `CachedMesh` / `CachedPrimitive` / `FrameDraw` / `StaticGroup` / `ReadbackSlot` / `TimestampSlot` / `GpuTimestampState`）+ `RenderFrameMetrics` + `record_*` / `clamp_viewport` / `apply_declared_viewport` / `ReadbackMetrics` / `ReadbackStatus` |
| 420–573 | 154 | **`pub struct Renderer` 字段（私有，约 90 个）** |
| 574–1,679 | **1,106** | `new` / `new_offscreen` / **`new_async`（管线与 GPU 资源创建的主体）** |
| 1,680–1,725 | 46 | `create_depth_view` / `create_shadow_view` |
| 1,726–1,869 | 144 | 网格缓存：`cached_mesh` / `cached_from_meshes` / `request_mesh_load` / `upload_decoded` |
| 1,870–1,993 | 124 | `upload_albedo` / `ground_plane_cached_mesh` / `upload_geometry` |
| 1,994–2,117 | 124 | 帧级环形缓冲 arena：`push_frame_uniform` / `push_frame_storage` / `material_bind_group` |
| 2,118–2,281 | 164 | 3D uniform / 光源 uniform / 世界 Sprite 准备与绘制 |
| 2,282–2,357 | 76 | `frame_3d_draws` |
| 2,358–2,480 | 123 | `resize` / `ensure_depth_view` / `ensure_post_process_scene_view` / `draw_post_process` |
| 2,481–2,544 | 64 | WP5/WP6 setter/getter + `ensure_world_sprite_capacity` |
| 2,545–2,719 | 175 | `render` / `render_into_texture` / `copy_texture_to_cpu` |
| 2,720–2,977 | 258 | 非阻塞回读：`readback_frame` … `submit_readback` |
| 2,978–3,423 | **446** | **`render_passes()`**（单函数，命令编码主体） |
| 3,424–3,450 | 27 | `ensure_2d_vertex_capacity` |
| 3,451–3,594 | 144 | GPU timestamp 五件：`acquire_timestamp_slot` / `finish_gpu_timestamp` / `arm_timestamp_slot` / `poll_gpu_timestamp` / `wait_for_gpu_timestamp` + 支持性查询 |
| 3,595–3,656 | 62 | metrics / 账本 getter 段 |
| 3,657–3,713 | 57 | `ensure_vector_buffer_capacity` |
| 3,714–4,005 | 292 | `sdf_bind_for` / `bind_group_for` / `ensure_white` / `ensure_atlas` / `ensure_font_atlas` / `ensure_custom_font` |
| 4,006–6,089 | **2,084** | `mod tests`（23 项；含两条**需要真实 GPU**的离屏回读用例，无适配器时自行 `return`，CI 上不会假红） |

### 2.4 对外可见面（实测，跨 crate 引用）

跨 crate 实际用到的只有这些，**拆分后必须逐项仍在 `engine::host::` 下可用**：

- **`RuntimeScene`**：`vertices` / `run_script_steps` / `scene_3d` / `scene_3d_with_camera` / `set_frame_views`（无）/ `set_camera_2d` / `camera_center_2d` / `render_view_2d` / `sprite_view_projection` / `world_transform_2d_of` / `world_transform_3d_of` / `set_scene_entity_transform_preview` / `scene_entity_ui_layout_rects` / `set_ui_reference` / `set_render_ui_overlay` / `set_font_atlas` / `set_vector_atlas` / `set_script_error_policy` / `set_gamepad_deadzone` / `take_script_micros` / `quit_requested` / `measure_scene_ui_text` / `with_script`（编辑器 `scene_view.rs` / `game_view.rs` / `preview_session.rs` / `game_app.rs`）。
- **`Renderer`**：`new` / `new_offscreen` / `render` / `render_into_texture` / `resize` / `readback_frame` / `readback_pending` / `readback_in_flight` / `readback_metrics` / `set_frame_views` / `set_post_process` / `set_world_sprite_camera` / `last_frame_plan` / `free_texture`（编辑器）与 `vertex_buffer_*` 等诊断量（`game`）。
- **自由函数**：`engine::host::load_project_font_atlas`（编辑器 `panels/game_view.rs:14`）。
- **`host::mod.rs` 的 `pub use`**：`frame::{BoundsSource, OverlayVertex, Scene3d, Scene3dBounds, Scene3dDrawable, SceneDraw, SceneMesh}`、`gamepad::*`、`renderer::{ReadbackMetrics, ReadbackStatus, RenderFrameMetrics, RenderOutcome, Renderer}`、`runtime_scene::{RuntimeScene, load_project_font_atlas, load_project_vector_atlas}`、`ScriptErrorPolicy`。

**结论**：只要 `host/mod.rs` 的 `pub use` 逐行保持，`editor`/`game`/`game_web`/`assetc` 零改动——这是本工作包最硬的一条验收不变量。

---

## 3. 关键工程事实（决定目录形态）

### 3.1 Rust 可见性：私有项对定义它的模块及其所有后代可见

这是 WP11 的核心裁定，本工作包逐条复用：

1. **`RuntimeScene`（282 行字段）与 `Renderer`（154 行字段）留在各自的 `mod.rs`**。字段私有域 = `runtime_scene` / `renderer`，所有子模块是它的后代 ⇒ **字段属性零改动**。若把结构体搬进 `state.rs`，字段私有域变成 `runtime_scene::state`，兄弟子模块访问不到 ⇒ 约 90 + 40 个字段要逐个加 `pub(super)`，纯属自找。
2. **固有 `impl` 可以整体搬进子模块**：结构体与 `impl` 不需要同模块（同 crate 即可），`impl` 里的 `self.私有字段` 一直合法。
3. **入口方法要跟着它的 helper 走**：父模块（`mod.rs`）**访问不到子模块的私有项**（私有域只向下可见）。所以 `init`（165 行）应与它调用的 `build_ui_scene_nodes` / `project_text_render_mode` / `load_runtime_scene` / `declared_camera_2d_in` 放进**同一个**子模块，`register_builtin_frame_systems`（309 行）同理。
4. **兄弟子模块互相访问不了私有项** ⇒ 跨文件共用的 helper 用 `pub(super)`（私有域 = 父模块，父与全部兄弟可用）。**这是本工作包唯一允许的可见性改动**，且每次都是编译器逼出来的、不会静默漏掉。
5. **`#[cfg(test)]` 模块放在父模块的子目录里**：`host/runtime_scene/tests/*.rs` 作为 `runtime_scene` 的后代，`use super::*;` 的 glob 会连父模块的**私有项**一并带下来（glob 对后代可见且传递）——这是 WP11 §13.1 已实测的形态，测试零改写。

### 3.2 现成的成功先例

- **WP11**（`tasks/done/WP11-script-rs拆分.md`）：`core/script.rs` 单文件拆 20 个子模块，纯搬运，Lua API 面与 Rust 符号零变化。
- **相机拆分批 1–5**（commit `315274a`）：`core/camera.rs` 2,565 行拆成 9 个模块 + `core/geom3d.rs` 外迁，**26 个调用点零改动**。
- **本文件自身的第一次尝试**（commit `f8a747a`）：`host/runtime_scene/` 子目录 + `text.rs`（569 行），把字体/文本/字形预热搬出主文件。**形态已验证**，本次是把它做彻底。

### 3.3 不引入 lint 变更

workspace **没有 `[lints]` 段**，CI 只跑 `clippy --workspace --all-targets --all-features -- -D warnings`。搬运会制造两类必须当场清掉的告警：

- `unused_qualifications`：搬完后 `crate::host::runtime_scene::RuntimeScene` 这类全路径会变冗余（`renderer.rs` 的测试里就有 3 处，行 5198 / 5286 / 5511）。
- 未使用的 `use`：搬走的代码把某些 import 带走后，留在原文件的 import 会变孤儿。

两者都由编译器指出，**每阶段必须把 clippy 跑到零告警再提交**。

---

## 4. 目标结构与行数预算

```
host/
  mod.rs                        ~35   ← 只保留 pub mod / pub use（现状 31 行，尽量不动）
  frame.rs                        302  ← 不动
  shader.rs                       272  ← 不动
  keymap.rs                       578  ← 不动
  gamepad.rs                    1,599  ← 不动（已达标）
  sample_project.rs               672  ← 不动（测试专用工程门）

  runtime_scene/
    mod.rs                      ~900  RuntimeScene 结构体 + 子模块声明 + 跨模块共享访问器 + pub use
    scene.rs                    ~500  init / 构造 4 入口 / 存档与碰撞调试访问器
    schedule.rs                 ~420  register_builtin_frame_systems / run_script_steps / run_pipelines / 系统注册
    script.rs                   ~330  脚本桥（with_script / 热重载 / 错误策略 / 诊断）
    spatial.rs                  ~700  空间传播、空间索引、物理权威、步进、可见性掩码
    frame_data.rs               ~760  vertices() + 帧级 3D 视图 + 生效相机/光源 + particles
    mesh_ui.rs                  ~820  场景 UI 布局 + push_ui_* / push_scene_ui / push_image_quad
    tilemap.rs                  ~560  分块缓存 + append_tilemap_layer* + sort_world_ranges + texture_id/dimensions
    camera.rs                   ~180  declared/first_enabled/selected camera + sprite_screen_rect
    fonts.rs                    ~260  load_project_*_atlas / load_custom_* / prewarm_font_atlas
    text/
      mod.rs                     569  ← 现 text.rs 平移
    tests/
      mod.rs                     ~40  共享脚手架与 use
      tilemap.rs                ~290  ← tilemap_chunk_tests
      text.rs                  ~4,700  ← sdf_tests（内部按 SDF/vector/UI 文本分文件，见 §5 阶段 1）
      schedule.rs               ~390  ← frame_schedule_tests
      ui.rs                      ~60  ← ui_circle_tests

  renderer/
    mod.rs                    ~1,050  Renderer 结构体 + 缓存类型 + metrics/账本 getter + 子模块声明
    construct.rs              ~1,300  new / new_offscreen / new_async（管线与 GPU 资源创建）
    gpu_mesh.rs                 ~330  cached_mesh / upload_geometry / upload_albedo / ground_plane / upload_decoded / request_mesh_load
    gpu_texture.rs              ~390  upload_texture / bind_group_for / sdf_bind_for / ensure_white / atlas / font atlas / custom font
    arena.rs                    ~200  push_frame_uniform / push_frame_storage / material_bind_group / ensure_*_buffer_capacity
    world_sprite.rs             ~300  WorldSpriteView / resolve / prepare / draw / ensure_capacity
    passes.rs                   ~700  render / render_into_texture / render_passes / draw_post_process
    readback.rs                 ~420  Readback* 类型 / readback_frame / poll_readback / submit_readback / copy_texture_to_cpu
    timestamp.rs                ~260  GpuTimestampState / 五件 / wait / 支持性查询
    targets.rs                  ~200  create_depth_view / create_shadow_view / ensure_depth_view / ensure_post_process_scene_view / resize
    tests.rs                  ~2,100  ← mod tests
```

**预算**：`host/` 总行数不变（约 20,570 → 约 21,000，新增的行只有子模块声明、`use` 与文件头注释）；**最大单文件 1,500 行以内**（现在是 10,458）；两个主 `mod.rs` 分别 ≤ 950 / ≤ 1,100 行。

---

## 5. 阶段划分与验收

每阶段一个独立 commit（**不含逻辑改动**，评审用 `git diff -w --find-copies-harder` 看整块移动）。任一阶段可停，不依赖后续阶段。

| 阶段 | 内容 | 风险 | 验收 |
|---|---|---|---|
| **0** | 先造守卫（§6） | 无 | 新门绿 + 基线可复现 |
| **1** | `runtime_scene.rs` → `runtime_scene/mod.rs`；**测试 5,543 行搬进 `runtime_scene/tests/`**（4 个域文件） | 极低，纯搬运 | 测试项数不减（≥1,480）+ clippy 零告警 |
| **2** | 自由函数分家：`tilemap.rs` / `camera.rs` / `fonts.rs` / `text/`（已是目录） | 低 | §7 快速组 |
| **3a** | `impl RuntimeScene` 第一刀：`scene.rs`（init + 构造 + 访问器）、`spatial.rs` | 中 | §7 快速组 |
| **3b** | `impl` 第二刀：`frame_data.rs`（`vertices()` 600 行）、`mesh_ui.rs`（615 行） | **中高**（每帧主路径） | §7 全量 + 2D golden |
| **3c** | `impl` 第三刀：`schedule.rs` / `script.rs` / `camera.rs` | 中 | §7 全量 |
| **4a** | `renderer.rs` → `renderer/mod.rs`；`mod tests` 搬 `renderer/tests.rs`；`targets.rs` / `timestamp.rs` | 低 | §7 快速组 |
| **4b** | `renderer/`：`gpu_mesh.rs` / `gpu_texture.rs` / `arena.rs` / `readout.rs` | 中 | §7 全量 + 3D golden |
| **4c** | `renderer/passes.rs`（`render_passes` 446 行**原样搬**，不拆函数）、`world_sprite.rs` | 中 | §7 全量 + 全部 golden |
| **4d** | `renderer/construct.rs`（`new_async` 1,106 行原样搬） | 低（纯搬） | §7 全量 |

**顺序上的两条硬规矩**：

1. **先搬测试，再搬代码**（阶段 1 先于 2/3）。测试是守卫，把它们搬到位能让后续每一刀都立刻有覆盖；且测试搬迁本身零风险（不碰生产代码）。
2. **`render_passes` 与 `new_async` 只搬位置，不在本工作包内拆函数体**。把 446 行函数拆成 `passes/clear.rs` / `passes/world2d.rs` / … 属于**改变代码结构**，会引入真实的回归风险，必须另开工作包（PBR/HDR 落地时顺势做，见 §9）。

---

## 6. 先造守卫，再动手（阶段 0）

拆 1 万行不难，难的是**证明拆完没变**。`renderer.rs` 有过真实事故：egui-wgpu 在 `Queue::write_buffer_with` 返回 `None` 时 panic（旧行号 `renderer.rs:958`，见 CHANGELOG），说明这个文件的失败模式是**静默的渲染差异**，人眼 diff 抓不住。四个守卫：

1. **对外符号面基线（新增）**：把 `engine` crate 的 `pub` 面（模块路径 + 项名 + 函数签名形态）导成基线，落 `artifacts/baselines/engine-pub-surface.json`，并加一条测试逐字节比对。做法与 `core/api_surface_snapshot` 同构，但**面向 Rust 侧**：用 `cargo doc --no-deps -p engine` 的 JSON 输出（`RUSTDOCFLAGS` 已有一条零告警门，复用它的构建产物即可）提取，不引入新依赖。断言消息里写明「拆分不得改变对外符号；有意新增才更新基线」。
2. **Lua API 面快照门（已有，必须绿）**：`core/api_surface_snapshot/lua_api_surface.json` + `lua_api_contract.lua`。`lua_api.rs` 是 `host` 的下游，这条门能抓到"搬动时不小心改了注册顺序"。
3. **Golden 帧（已有，必须绿）**：`wp0-2d-golden` / `wp0-3d-golden` / `wp0-mixed-golden` / `wp0-hud-golden` / `wp0-lighting-golden` / `wp0-lod-golden` / `wp0-particle-parallel-golden` / `wp0-spatial-index-golden` / `wp0-editor-preview-golden`（`crates/editor/examples/editor_preview_golden.rs`）。这是"像素等价"的唯一硬证据，且**离线确定性**（离屏渲染，`new_offscreen` 走 Sync 加载）。
4. **测试项数不变量**：基线 **1,480 passed / 0 failed / 2 ignored（43 个二进制）**。`host/` 内 158 项。每阶段跑完必须**不减**。

---

## 7. 每阶段统一验收命令

```powershell
# 快速组（阶段 1/2/3a/4a）
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p engine --all-features

# 全量（阶段 3b/3c/4b/4c/4d 必须）
cargo test --workspace --all-features            # 基线 1480 通过、0 失败，不得减少
cargo run -p game   -- --project examples/game_jhyt --check
cargo run -p editor -- --project examples/game_jhyt --check
cargo run -p engine --example wp0_2d_golden
cargo run -p engine --example wp0_3d_golden
cargo run -p engine --example wp0_mixed_golden
cargo run -p editor --example editor_preview_golden
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/compare-golden.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-docs.ps1

# 收口（阶段 4d 之后一次）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify.ps1 -Mvp
```

**每次提交前额外一条人工核对**（脚本化建议见 §10）：

```powershell
git diff --name-only <阶段起点>..HEAD -- engine/crates/editor engine/crates/game engine/crates/assetc engine/crates/game_web
# 必须为空
```

---

## 8. 范围控制（本工作包刻意不做）

- **不改任何算法、不合并重复、不修可疑实现**。发现的问题逐条记进 §12，另开任务。
- **不做子结构体分解**（不把 `Renderer` 的 90 个字段切成 `GpuCache` / `ReadbackRing` / `TimestampRing`）。收益是把"上帝对象"变成几个小对象，代价是**每一处 `self.x` 都要改**（数千个访问点），diff 不再能证明"纯移动"。要做另开工作包，且必须在本次拆分之后。
- **不拆函数体**：`render_passes`（446）/ `new_async`（1,106）/ `vertices`（600）/ `register_builtin_frame_systems`（309）**原样搬**。
- **不动 `gamepad.rs` / `frame.rs` / `shader.rs` / `keymap.rs` / `sample_project.rs`**（后两者已在合理规模；`gamepad.rs` 1,599 行可另议）。
- **不改 `docs/`**：本工作包不新增编号文档，只在完成后把结论追加进 `CHANGELOG.md` 与相关任务的实现记录。

---

## 9. 坑清单

1. **可见性**（§3.1）：唯一会让拆分卡在编译上的点。裁定是"结构体与共享 helper 留 `mod.rs`、入口方法跟着 helper 走"，把 `pub(super)` 收敛到个位数。
2. **`mod.rs` 只放共享访问器，不要放需要子模块私有 helper 的方法**：父模块访问不到子模块私有项，反向依赖会让 `mod.rs` 被迫加一堆 `pub(super)`，与 WP11 的裁定相反。
3. **`use super::*;` 的传播方向**：子模块能拿父模块的私有项，**兄弟之间拿不到**。跨文件共用的纯函数（`texture_id_of` / `merge_adjacent_draws` / `screen_rect_ndc` 等）要么放 `mod.rs`，要么标 `pub(super)`。
4. **`crate::host::runtime_scene::RuntimeScene` 这类全路径**：搬完会触发 `unused_qualifications`（`renderer.rs` 行 5198 / 5286 / 5511 已知 3 处）。统一改用 `RuntimeScene`（`mod.rs` 的 `pub use` 已在作用域内）。
5. **孤儿 `use`**：`renderer.rs` 顶部 30 行的 `wgpu::*` / `winit::*` import 会随管线创建代码搬走，留在 `mod.rs` 的会变孤儿；`runtime_scene.rs` 顶部 55 行的 `crate::core::*` 同理。每阶段用 clippy 清零，**不要靠 `#[allow]`**。
6. **测试里的相对路径**：全部走 `crate::core::games_dir()`（`CARGO_MANIFEST_DIR` 向上三级 = 仓库根），**与源文件位置无关**。搬迁安全；但**不要**顺手改成相对路径。
7. **GPU 依赖的测试**：`renderer.rs` 的两条离屏回读用例在无适配器时自行 `return`（软跳过）。搬迁后必须保持这个形态——改成 `panic!` 会让无 GPU 的 CI 假红。
8. **`host/sample_project.rs` 是 `#[cfg(test)]` 且被 `mod.rs` 声明**：它持 `pub(crate)` 的 `RuntimeScene::script` 字段做 API/组件覆盖率门。搬动 `RuntimeScene` 时**不要动这个字段的可见性**（`pub(crate)` 是它存在的唯一理由）。
9. **`mod text;` 的声明位置**：`text` 是 `runtime_scene` 的子模块，它 `use super::{...}` 拿了父模块的私有项（`FONT_ATLAS_TEXTURE_PATH` / `load_custom_font_atlas` / `text_to_ndc` …）。把 `text.rs` 平移到 `runtime_scene/text/mod.rs` 后**这些路径全部不变**；但若把某个被它引用的自由函数搬到**别的**子模块（如 `fonts.rs`），必须同步改 `text/mod.rs` 的 `use super::...` 为 `use super::fonts::...`（跨兄弟 ⇒ 那些项要 `pub(super)`）。
10. **`verifier` 的路径假设**：`scripts/compare-golden.ps1` 与 `verify.ps1` 依赖 `artifacts/baselines/*.json` 的固定文件名，搬迁不得改名或移动这些基线文件。
11. **提交粒度**：一个阶段一个 commit，commit message 里写清"纯搬运，无逻辑改动"与行数前后；**不要**把两个阶段合成一个 commit（回滚档会失去意义）。

---

## 10. 验收证据清单

完成后本文件需补一节 §13「实现记录」，至少包含：

1. **行数前后对照表**（每个新文件的实测行数 + `host/` 总行数）。
2. **四道门的读数**：`cargo test --workspace --all-features` 的通过/失败/忽略数；金样例各 example 的 compare 结果；`check-docs.ps1`；`verify.ps1 -Mvp` 的结论（含它已知的第一条红是否与本工作包相关）。
3. **对外符号面无变化**的证据：`engine-pub-surface.json` 比对通过；`git diff --name-only` 证明 `editor`/`game`/`assetc`/`game_web` 零改动。
4. **Lua API 面快照门**读数（两个平台 `cases` 数不变）。
5. **性能不回退**的证据：`wp0-scene-frame` 或 `-Benchmarks` 的"这一次 vs 上一次"对比（结构搬迁不应改变帧时间；若读数有偏移，必须给出机器噪声之外的解释或按噪声处理）。
6. **偏差记录**：与 §4 预算不一致的文件要逐条写明理由。
7. **回滚档**：每阶段的起点 commit。

---

## 11. 未完成项 / 已知限制（预期在本工作包之后仍存在）

- `render_passes` 与 `new_async` **仍是单个大函数**，只是换了文件。真正拆开它们属于 PBR/HDR 落地时的工作。
- `Renderer` 的 90 个字段与 `RuntimeScene` 的 40 个字段**仍是扁平结构**。"上帝对象"这一半的债本工作包不解。
- `engine` crate 里 `host/` 仍占绝对多数（拆分只改变文件粒度，不改变"引擎代码近半在宿主侧"这个分层事实，见 `docs/62` §1.1）。真正的分层债是 P1 的另一项：削 `rustlua-scripting` 的 13 条正向依赖。
- `gamepad.rs`（1,599 行）与 `rustlua-ecs/src/component_schema.rs`（2,654 行）、`editor/src/panels/scene_view.rs`（6,038 行）、`editor/src/actions.rs`（4,630 行）仍在 `docs/62` §1.1 的上帝文件表里，**本工作包不覆盖**。

---

## 12. 搬运过程中发现的问题（认领后逐条追加，不在本工作包内修）

按 §8 的规矩，这些**只登记、不修**。每条都写清"在哪读到的"，便于下一个人复核。

1. **`world_sprite_capacity` 与 `world_sprite_buffer.size()` 是同一事实的两份记录**。
   `ensure_world_sprite_capacity`（现 `renderer/world_sprite.rs`）扩容时同时写这两处，任何第三条
   路径重建那张缓冲都会让字段与真实容量静默脱节；2D overlay 那一侧只读 `vertex_buffer.size()`，
   没有冗余字段。读码时看到，未改。
2. **"2 的幂扩容"这件事写了两遍**：2D 侧抽了自由函数 `next_2d_vertex_capacity`（留在
   `renderer/mod.rs`，`arena.rs` 按短名取用），世界 Sprite 侧把 `while capacity < required { capacity *= 2 }`
   内联在方法里。§8 明确禁止合并重复 ⇒ 两份原样保留（搬完分别落在 `arena.rs` 与 `world_sprite.rs`）。
3. **测试目录靠 `use super::*` 的 glob 同时拿"父模块私有项"和"父模块的 use 绑定"**——这是本次
   拆分最大的隐性成本。搬一段代码时，同一个名字可能有三个归属（父模块仍需要 / 只有测试需要 /
   跟着代码搬走），只能逐个问编译器。四刀累计为此补了 7 条显式 `use`
   （`GROUND_PLANE_SENTINEL`、`WHITE_TEXTURE_PATH`、`ClearPolicy`、`DEFAULT_3D_CLEAR`、
   `WorldSpriteVertex`、`DeviceDescriptor`、`resolve_world_sprite_view`）。**下一批搬 `host/` 的人
   会踩同一颗雷**；能治的做法是把测试的依赖面显式化（每个 `tests/*.rs` 自带 use），代价是那次
   diff 不再是纯搬运。
4. **模块重名会遮蔽路径**：`renderer::tests::world_sprite` 与 `renderer::world_sprite` 同名，
   在测试文件里写 `use super::world_sprite::X` 解析到的是**测试自己**，只能写
   `crate::host::renderer::world_sprite::X` 全路径。`runtime_scene/tests/` 与 `runtime_scene/`
   的子模块也有同样的同名对（`tests/tilemap.rs` vs `tilemap.rs`、`tests/spatial.rs` vs `spatial.rs`、
   `tests/schedule.rs` vs `schedule.rs`、`tests/camera_*.rs` vs `camera.rs`）。新环境里第一次撞上时
   报错信息是"找不到 X"，不会提示是遮蔽问题。
5. **`new_async` 的 1,070 行里约 90 条字段初始化与结构体定义是"改一处要改两处"的耦合**
   （现 `renderer/construct.rs`）。§8 把子结构体分解排除在外，所以这条只能等 PBR/HDR 那批。
6. **四个大函数仍是单个大函数**，只是换了文件：`render_passes` 446 行、`new_async` 1,070 行、
   `vertices` 600 行、`register_builtin_frame_systems` 309 行（§11 已预告，这里是实测值）。
7. **工具口径的两处坑，都会让"证明纯搬运"假红**：
   - rustfmt 会因为 `pub(super)` 把签名折成竖排并**补尾逗号**，所以"逐行 diff 一致"这种证法在本包
     必然失败。实现记录里的证明脚本按"去注释 + 去空白 + 抹掉 `pub(super)` 与尾逗号"后取**按段包含**。
   - `grep -c '#\[test\]'` 会把**文档注释里的属性字面量**算进测试数（本包实测虚高 1，已在
     `renderer/tests/mod.rs` 的头注释里换掉措辞）。要数测试得 `grep -A1 '#\[test\]'` 再取 `fn` 名。
8. **`check-docs.ps1` 只校验 markdown 链接目标是否存在，不校验散文里的行号引用**。本包把
   `docs/24` 的失效链接改成了目录形态，但 `docs/37` §P1-7/P1-8、`docs/38` §2、`docs/53` §1 里那些
   `renderer.rs:2403-2405`、`runtime_scene.rs:734-768` 之类的**行号引用在拆分后全部失效**——它们是
   当时的历史审计证据，改写会伪造证据链，因此本包**不动**，只在此登记：读这些行号要按
   `42a565c`（拆分前最后一次提交）解释。
9. **共享工作树的提交窗口风险（流程事实）**：阶段 4a 的文件在 `git add` 之后、`git commit` 之前被
   同仓另一路的提交 `eb38a46` 整体扫走；对方随后 `reset --soft` 重commit（`786cfa5` 只含它自己的
   4 个脚本），4a 的内容于是由下一刀的 `git add` 重新带进 `2e451f3` ⇒ **4a 与 4b 落在同一个 commit**，
   违反 §9 坑 11"一阶段一 commit"。没有改写历史（共享工作树里改写会影响别人）。4a 的完整账（前后
   行数、可见性清单、证明口径）记在实现记录 §14.6，回滚档按 `ef30c52 → 2e451f3` 这一对读。
10. **`verify.ps1 -Mvp` 的 MVP 段本来就红，红在示例数据与用例期望脱节**（收口时实测，**先于本包存在**）：
    `jhyt_m17_shop` 的 M17-04a 断言 `#(require('systems.shop').ids()) = 1`
    （`engine/crates/game/examples/jhyt_m17_shop.rs:60`，该文件最后一次改动是 `6bcdabd`，2026-09-27），
    而 `examples/game_jhyt/scripts/data/shops.lua` 在 **`1cd624b`（2026-10-01，"v0.2 剧情与数据扩展"）**
    加了 `han_forge` 与 `a_qi_provisions` 两家店 ⇒ 现在 `ids()` 返回 3。`1cd624b` 是本包起点
    `42a565c` 的**祖先**，所以这条与本包无关（本包没碰 `examples/`，也没碰 `crates/game/examples/`）。
    同一段里前面三条是绿的（`jhyt_smoke_ok`、`jhyt_m17_ok`、`jhyt_m17_dialogue_ok`）。
    **最硬的一条证明**是本包起点上两边就已经不一致，且这两个文件在区间里一个字都没改：

    ```powershell
    git diff --name-only 42a565c..HEAD -- examples/game_jhyt/scripts/data/shops.lua engine/crates/game/examples/jhyt_m17_shop.rs   # 空
    git show 42a565c:examples/game_jhyt/scripts/data/shops.lua | grep -cE "^  [a-z_0-9]+ = \{"   # 3 家
    git show 42a565c:engine/crates/game/examples/jhyt_m17_shop.rs | sed -n 60p                   # 断言仍是 1.0
    ```

    这条**不能**在本包里顺手改：§7 的人工核对要求
    `git diff --name-only <起点>..HEAD -- engine/crates/game` 为空，而那条断言正好住在这个不许动的
    crate 里。修法是把按数量的断言改成按 id 取（或把期望值更新成 3），留给示例内容的下一批。

---

## 13. 工作量

| 阶段 | 预估 |
|---|---|
| 0 守卫（符号面基线 + 跑通四门） | 0.5 天 |
| 1 测试搬迁（5,543 行，零风险） | 0.5 天 |
| 2 自由函数分家 | 0.5 天 |
| 3a/3b/3c `impl RuntimeScene` 三刀 | 1.5 天 |
| 4a–4d `renderer.rs` 四刀 | 1.5 天 |
| 收口（`verify.ps1 -Mvp` + 证据） | 0.5 天 |
| **合计** | **约 5 天**（`docs/62` §6 给 `renderer.rs` 1 周、`runtime_scene.rs` 1–2 周；本预算更紧是因为**只搬不改**，且已排除函数体拆分与子结构体分解） |

**推荐执行顺序**：阶段 0 → 1 → 2 → 3a →（可停）3b → 3c → 4a → 4b →（可停）4c → 4d。

阶段 1 结束即可让 `runtime_scene.rs` 从 10,458 行降到 5,063 行——**这是投入产出比最高的一步，且零风险**。若只能做一件事，做阶段 1。

---

## 14. 实现记录（阶段 0–4d + 收口，2026-10-03 回填）

> §10 要求把这一节写成"§13"，但本文件成文时 §13 已被"工作量"占用，故落编号为 §14。

### 14.1 行数前后对照（全部实测，`wc -l`）

两个上帝文件**都不存在了**：

| 拆分前 | 行数 | 拆分后 | 行数 |
|---|---|---|---|
| `host/runtime_scene.rs` | 10,458 | `host/runtime_scene/`（11 个源文件 + `tests/` 18 个，都含 `mod.rs`） | 11,438 |
| `host/renderer.rs` | 6,089 | `host/renderer/`（10 个源文件 + `tests/` 8 个，都含 `mod.rs`） | 6,303 |
| `host/` 合计 | 20,570 | `host/` 合计 | **21,195** |

`host/runtime_scene/`：`mod.rs` 776、`frame_data.rs` 1,048、`mesh_ui.rs` 728、`spatial.rs` 591、
`text.rs` 569、`tilemap.rs` 571、`scene.rs` 560、`schedule.rs` 549、`camera.rs` 244、
`fonts.rs` 155、`script.rs` 67；`tests/` 最大 `camera_physics.rs` 498，`mod.rs` 105。

`host/renderer/`：`construct.rs` 1,143、`mod.rs` 701、`passes.rs` 631、`gpu_texture.rs` 505、
`readback.rs` 371、`world_sprite.rs` 322、`gpu_mesh.rs` 296、`timestamp.rs` 173、`arena.rs` 164、
`targets.rs` 111；`tests/` 最大 `world_sprite.rs` 410，`mod.rs` 27。

**目标达成**：本包两个目标的最大产物 1,143 行，全部 ≤ 1,500（§2）。`host/` 里唯一超过 1,500 的
文件是**没动过**的 `gamepad.rs` 1,599（§8 明确排除，§11 已登记）。

### 14.2 每阶段的起点与回滚档

| 阶段 | 起点 commit | 落地 commit | 该刀父文件 |
|---|---|---|---|
| 0 守卫 | `42a565c` | `378e7ad` | — |
| 1 测试搬迁 | `378e7ad` | `faa943b` | 10,458 → 5,063 |
| 2 自由函数 | `faa943b` | `2ede51c` | 5,063 → 4,438 |
| 3a impl 第一刀 | `2ede51c` | `ad130aa` | 4,438 → 3,180 |
| 3b 第二刀 | `ad130aa` | `69323e8` | 3,180 → 1,748 |
| 3c 第三刀 | `69323e8` | `ef30c52` | 1,748 → 772 |
| 4a `renderer.rs`→目录 + targets/timestamp | `ef30c52` | **内容在 `2e451f3`**（见 §14.6） | 6,089 → 3,990 |
| 4b impl 第一刀 | `ef30c52`（与 4a 同树） | `2e451f3` | 3,990 → 2,753 |
| 4c 第二刀 | `2e451f3` | `2b707a2` | 2,753 → 1,818 |
| 4d 第三刀 | `2b707a2` | `2057aeb` | 1,818 → 701 |

任一刀可单独 `git revert`。4c/4d 的父文件行数由证明脚本一并输出（"源 2,754 行覆盖 927 行"、
"源 1,819 行覆盖 1,102 行"），与上表一致。

一处对不上是**故意的**：3c 那行写 `1,748 → 772`，而 §14.1 的表里 `runtime_scene/mod.rs` 现在是
**776**。差的 4 行是收口时修 rustdoc 短链接那次改动（两个链接改成全路径 + 一条说明为什么必须写
全路径的注释，§14.9 第 2 项），不属于任何一刀。`renderer/tests/mod.rs` 同理：4a 落地时是 **26 行**，
收口换掉那句措辞（多加一行说明）后是 §14.1 里的 **27 行**。

### 14.3 可见性放宽的总账（§3.1.4 允许的唯一一类改动）

拆分前两个文件里 `pub(super)` 出现 **0 次**；全套共 **61 处**（逐条可在源码里按
`grep -n '^\s*pub(super) '` 复现）：

- `renderer/` 30 处：4a 10（targets 4 + timestamp 6）/ 4b 9（gpu_mesh 2、gpu_texture 6、arena 1）/
  4c 11（world_sprite 10 + arena 1）/ **4d 0**（`construct.rs` 是 `renderer` 的后代，按私有字段
  构造 `Self { … }` 字面量本就合法，那一刀没有任何放宽）。
- `runtime_scene/` 31 处：`tilemap.rs` 9、`mesh_ui.rs` 8、`camera.rs` 6、`scene.rs` 3、
  `frame_data.rs` 3、`schedule.rs` 2。

方向只有"子模块 ↔ 父模块 / 兄弟模块"，**没有一处放宽到 `pub`**：这一条不是口头保证，而是由
符号面门兜住（§14.4 的 `pub_surface_ok`——`cargo doc` 的项清单按路径+种类逐项与基线比，
私有项一旦变 `pub` 就多一条锚点页，门必红）。

一处**账目更正**：4c 的 commit message 写"编译器点名的 8 处"，它下面列的清单实际是 11 处
（把 `WorldSpriteView` 的 3 个字段与结构体本身混算成了"1 处"）。以本节实测为准。

### 14.4 四道门与证据读数

收口时**一次**跑全（用户裁定：逐阶段只跑 `cargo fmt` + `cargo check`，测试留到最后）。
每条都给复现命令，读数按命令原样抄，不做四舍五入。

**① 全量测试**

```powershell
cargo test --workspace --all-features          # 在 engine/ 下，或 --manifest-path engine/Cargo.toml
```

实测 **1,481 passed / 0 failed / 2 ignored，44 个测试目标**（17 个 doc-test 目标全 0）。
对照阶段 0 记下的搬前基线 **1,480 / 0 / 2 / 43 套件**（`378e7ad` 的 message）：+1 目标 +1 用例
就是阶段 0 自己新增的 `engine/crates/engine/tests/host_pub_surface.rs`，其余一格未动。
**本包最硬的一条自证**在这里：`engine` 的 unittests（`src/lib.rs`）**158 passed**，而拆分前
`host/` 的 `#[test]` 实测 100 + 23 + 22 + 4 + 9 = **158**（`42a565c` 逐文件 `grep -c` 可复现，
`engine/crates/engine/src` 里的单测 100% 落在 `host/`）。搬了 5,395 + 1,847 行测试，
一条不多一条不少。

**② 宿主与两个工程的 `--check`**（要在仓库根跑，`examples/game_jhyt` 在根上）

```powershell
cargo run --manifest-path engine/Cargo.toml -p game   -- --project examples/game_jhyt --check
cargo run --manifest-path engine/Cargo.toml -p editor -- --project examples/game_jhyt --check
```

`game_check_ok project=examples/game_jhyt`（exit 0）、
`editor_project_ok root=…\examples\game_jhyt baseline=v0.21` +
`schema_audit documents=8 components=422 schema_errors=0 unknown_components=0 renamed_components=0`（exit 0）。
`verify.ps1` 里还有第二个工程：`editor_project_ok root=…\examples\game_api baseline=v0.21`。

**③ 六个像素/语义 golden + scene_frame**（`--release`，全部 `--output` 要放在 `--` 之后）

```powershell
cargo run --quiet --release --manifest-path engine/Cargo.toml -p engine --example wp0_3d_golden -- --output <tmp>/wp0_3d_golden.json
# 同理 wp0_2d_golden / wp0_mixed_golden / wp0_hud_golden / wp0_lighting_golden / -p editor --example editor_preview_golden
cargo run --quiet --release --manifest-path engine/Cargo.toml -p engine --example wp0_scene_frame -- --output … --cycles …
```

六个都 exit 0，自带断言原样打印：`wp0_hud_golden_ok … overlay_pixel_without_ui=[137, 225, 255]
overlay_pixel_with_ui=[243, 89, 243]`、`wp0_lighting_golden_ok shadow_darkens=true
diffuse_responds=true lit_lum=153.7 shadow_lum=59.4`、
`wp0_editor_preview_golden_ok painted=65536 vertices=6 draws=1`。逐字段对照见 §14.7。

**④ 对外符号面 / Lua API 面 / 文档面**

```powershell
node scripts/engine-pub-surface.mjs --check
node scripts/gen-lua-api-docs.js --check
node scripts/check-web-api-overrides.mjs
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-docs.ps1
cargo doc --no-deps --workspace --lib        # warnings are errors（verify 里带 -D warnings）
```

- `pub_surface_ok items=2131 moved_into_submodules=6`——与阶段 0 基线（2,125 项）逐项同路径同种类，
  多出的 6 条是**定义搬进子模块后新增的 rustdoc 页面**，原路径仍可解析（脚本对这一类有专门计数，
  私有项若被放宽成 `pub` 会再多一条锚点页 ⇒ 本包 61 处 `pub(super)` 就是被这道门兜住的，见 §14.3）。
- `lua_api_docs_ok tables=41 funcs=275 stubs=275 documented=0`（再生成是 no-op）。
- `web_api_overrides_ok overrides=56 modules=11 wholesale=23 dropped=76 invented=3 referenced=8`。
- `check_docs_ok docs=58 game_docs=24 h1=84 links=299 ps1_bom_ok=29 ps1_parse_ok=29`。
- `cargo doc` **0 警告**（中途出过 2 条，见 §14.9）。

**⑤ 未改动证明**（§7 要求的人工核对，扩成"我的 commit 集"而不是区间，因为区间里混着同仓另一路）

```powershell
# 8 个搬运 commit 碰过的文件，去掉 host/ 前缀后必须只剩符号面门那一个工具文件
for c in faa943b 2ede51c ad130aa 69323e8 ef30c52 2e451f3 2b707a2 2057aeb; do git show --name-only --format= $c; done \
  | grep -v '^engine/crates/engine/src/host/'
```

实测只剩 `scripts/engine-pub-surface.mjs`（门的"子模块定义处"计数，非引擎代码）。
`git diff --name-only 42a565c..HEAD -- engine/crates/editor engine/crates/game engine/crates/assetc engine/crates/game_web`
= **0 行**；`git diff --name-only 42a565c..HEAD -- plugins/` 的 3 个文件全部来自同仓另一路的
微信提交（`git show --name-only --format= <本包 9 个 commit> -- plugins/` 逐个为 0），
`lua_api_overrides.json` 同理不在本包 diff 里。

**⑥ `verify.ps1 -Mvp` 全量门**

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify.ps1 -Mvp
```

- **Quality gate 段全绿**（`Quality gate passed`，日志
  `artifacts/logs/quality/verify-20261003-194147.log`）：文档一致性、vendor 补丁基线 29/0、
  fmt 零 diff、Lua API 桩、Web API override、`clippy --workspace --all-targets --all-features -D warnings`
  零告警、rustdoc 零警告、符号面门、pre-commit hook 门 `git_hooks_ok cases=46`、
  全量测试、debug build、`git diff --check`、compare-baseline 自检 `cases=7`、
  soak 分析器自检 `cases=5`、两个工程的 check、空间迁移报告、编辑器性能基线 `scenarios=32 failed=0`、
  编辑器 GUI 冒烟 `cases=41 failed=0`。
- **MVP headless 段红了一条，整条门因此 `exit=1`**：第 4 条 `jhyt_m17_shop` 报
  `#(require('systems.shop').ids()) = 3, want 1`。**先于本包存在**——期望值写在
  `engine/crates/game/examples/jhyt_m17_shop.rs:60`（最后一次改动 `6bcdabd`，2026-09-27），
  而商店数据在 `1cd624b`（2026-10-01）从 1 家扩到 3 家，`1cd624b` 是本包起点 `42a565c` 的祖先。
  本包没碰 `examples/`，也没碰 `crates/game/examples/`（§14.4 ⑤ 的命令可复现）。前面三条
  （`jhyt_smoke_ok`、`jhyt_m17_ok`、`jhyt_m17_dialogue_ok`）全绿。逐条登记在 §12 第 10 项，**本包不修**。

### 14.5 与 §4 预算的偏差（逐条理由）

`runtime_scene/`：

| 文件 | 预算 | 实测 | 理由 |
|---|---|---|---|
| `mod.rs` | ~900 | 776 | 预算按"跨模块共享访问器都留父模块"宽估；实测多数访问器只服务自己那一族方法，跟着方法搬走了（父模块留结构体 + 声明 + 真正共用的类型）。 |
| `schedule.rs` | ~420 | 549 | `register_builtin_frame_systems`（309 行）按 §8 **整段原样搬**，预算给的是"拆掉它之后"的错觉值。 |
| `script.rs` | ~330 | **67** | 最大偏差。3c 现场裁定：脚本桥的方法与调度/帧数据在同一条调用链上，独立成文件会逼出一批兄弟间 `pub(super)`；把能并的都并回 `schedule`/`camera`，`script.rs` 只剩真正独立的那一小撮（见 `ef30c52` 的 message）。 |
| `frame_data.rs` | ~760 | **1,048** | 同上：`vertices()` 600 行按 §8 不拆函数体，整段落在这里。它仍是本目录最大文件，但 ≤ 1,500。 |
| `spatial.rs` / `mesh_ui.rs` | ~700 / ~820 | 591 / 728 | 条目归属与预算略有出入（`push_*` 一族里几条按调用链落到了 `frame_data.rs`）。 |
| `text.rs` | `text/mod.rs` 569 | 569（**单文件**） | 预算假设 `text` 要建目录；它本来就 ≤1,500，建目录只多一层 ⇒ 保持 `text.rs` 平移，§9 坑 9 里说的"路径不变"这条依然成立。 |
| `tests/` | 单文件 `text.rs` ~4,700 | 18 个文件，最大 498 | 预算里那个 4,700 行单文件**与 §2 的 1,500 上限自相矛盾**；按 §5 阶段 1 的括注（"内部按 SDF/vector/UI 文本分文件"）落成多文件。`tests/mod.rs` 预算 ~40 → 105（18 条 `mod` 声明 + 分域注释）。 |

`renderer/`：

| 文件 | 预算 | 实测 | 理由 |
|---|---|---|---|
| `mod.rs` | ~1,050 | **701** | 预算假设"缓存类型 + metrics getter"留父模块；实测它们各自跟自己的域走（`ReadbackMetrics`/`ReadbackStatus` 跟 `readback.rs`，`mod.rs` 用 `pub use` 接回原路径）。父模块留：`Renderer` 结构体与全部字段、`ReadbackSlot`、`DepthPass`/`GpuMaterial`/`StaticGroup`/`FrameDraw` 等共享类型与自由函数。 |
| `gpu_texture.rs` | ~390 | **505** | `ensure_vector_buffer_capacity` 按计划该进 `arena.rs`，但它与 `sdf_bind_for`/字体图集共用同一批字段和 helper；放 `arena.rs` 会额外逼出 3 条放宽 ⇒ 按 §9 坑 2 的裁定（"不要让父模块反向依赖子模块私有项"）留在 `gpu_texture.rs`。这是本包**唯一一处与预算文件的归属不一致**，在 `2e451f3` 的 message 里也登记了。 |
| `readback.rs` | ~420 | 371 | `struct ReadbackSlot` 按 §3.1.2 **现场裁定不搬**（`tests/readback.rs` 里有它的结构体字面量，搬走要放宽 10 个字段）⇒ 留 `mod.rs`，`readback.rs` 只带一条 `use super::{ReadbackSlot, Renderer};`。 |
| `arena.rs` / `gpu_mesh.rs` / `world_sprite.rs` / `passes.rs` / `timestamp.rs` / `targets.rs` / `construct.rs` | 200/330/300/700/260/200/1,300 | 164/296/322/631/173/111/1,143 | 都是同一批条目的自然尺寸，没有额外内容。 |
| `tests.rs` | ~2,100 单文件 | **不成立** → `tests/` 7 文件 | 那一段实测 1,847 行，**超过 §2 的 1,500 上限**，按域落成 `tests/`（最大 `world_sprite.rs` 410）。这是本包对预算的第二处结构性偏离，理由与 `runtime_scene/tests/` 同一句话：预算写的时候没实测这一段。 |

总量：预算"约 20,570 → 约 21,000"，实测 **20,570 → 21,195（+625）**。多出的 ~200 行是每个新文件
3–8 行的头部 doc（写明"从哪搬来、可见性依据是什么"）与同一条 `use` 在多个文件里的重复计数。

**工期对照（不要拿去换算下一批）**：§13 的预算是约 5 天，实际 0→4d 的九个 commit 落在
`10-03 15:57 → 19:21`（**3.5 小时**墙钟），收口再加约 40 分钟。这个数**不能**当人类工期用：
执行体是 AI、每一刀都由脚本代跑（`gen.mjs`/`proof*.mjs` 生成 + 校验），中间还夹着同仓另一路的
三次并发提交与等待。有意义的对照只有两条——验证策略按用户裁定改成"逐阶段只 fmt+check、
测试全留到最后"（省下 8 次全量编译），以及真正的耗时都花在**编译器点名的可见性**上（4c 一刀
11 处、每处一轮 `cargo check`）。

### 14.6 阶段 4a 的完整账（它的文件与 4b 落在同一个 commit）

§9 坑 11 要求一阶段一 commit。4a 实际执行到位（fmt/check/证明脚本都跑过），但**内容落在了
`2e451f3`**：4a 的文件在 `git add` 之后、`git commit` 之前被同仓另一路的 `eb38a46` 整体扫走，
对方随后 `reset --soft` 重commit 成 `786cfa5`（只含它自己的 4 个脚本），4a 的内容于是由下一刀的
`git add` 重新带走。共享工作树里不改写历史，所以这条只能记账：

- 4a 做了什么：`renderer.rs` → `renderer/mod.rs`（`git mv`，`pub mod renderer;` 与全部
  `engine::host::renderer::…` 路径不变）；`#[cfg(test)] mod tests`（实测 1,847 行）按域搬成
  `renderer/tests/`（7 个域文件 + `mod.rs` 27 行）；`targets.rs` 111（`create_depth_view` /
  `create_shadow_view` / `ensure_depth_view` / `ensure_post_process_scene_view`）；`timestamp.rs`
  173（`TimestampSlot` / `GpuTimestampState` + 5 条时间戳方法）。父文件 6,089 → 3,990。
- 可见性放宽：10 处（`targets.rs` 4 + `timestamp.rs` 6；两个类型的定义留在能看见它们的地方，
  只有被 `mod.rs` 调用的方法放宽）。
- 证明：`node %TEMP%/wp12/proof4a.mjs`（按段去空白包含 + 父文件按原序 + `headExempt` 处理被重排的
  `use` 块；`pub(super)` 与 rustfmt 补的尾逗号一并归一）。
- 回滚档：`ef30c52`（4a 的起点）→ 与 4b 一起在 `2e451f3`；要单独回退 4a 只能反向重放，
  实际处置是**整刀回退到 `ef30c52`**（把 4a+4b 一起退回）。

### 14.7 性能与画面没有回退（读数 + 每条差异的归属）

**A. 六个像素/语义 golden 的逐字段对照**（本机同适配器：两份都是 `Dx12 / Microsoft Basic Render Driver`）

`compare-golden.ps1` 按**文件名**配对，而示例写报告用下划线（`wp0_2d_golden.json`）、
`artifacts/baselines/` 里的留档用连字符（`wp0-2d-golden.json`），所以要先把留档换成下划线名：

```powershell
# <prev> = artifacts\baselines 里那六份 2026-09-20 报告的下划线改名副本（-Scene-Frame 那份不参与，见 B）
mkdir <cur> <prev>; cd <cur>; cargo run --quiet --release --manifest-path ../engine/Cargo.toml -p engine `
      --example wp0_3d_golden -- --output $PWD/wp0_3d_golden.json      # 其余五份同理（editor_preview 用 -p editor）
cd <prev>; Copy-Item ..\artifacts\baselines\wp0-3d-golden.json .\wp0_3d_golden.json   # 六份照此改名
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/compare-golden.ps1 -Current <cur> -Previous <prev>
```

实测 `golden_compare_ok files=6 compared=209 semantic_diffs=1 ignored_noise=3`，唯一一条：

```
DIFF  root.semantic.content_bounds[3]: 179 -> 205      (wp0_3d_golden)
```

归属是**可 mechanistic 复现的**，不是猜：`wp0_3d_golden` 加载 `examples/game_api/scenes/demo3d.scene`
（`engine/crates/engine/examples/wp0_3d_golden.rs:15-17`），该文件在 `0de935f`（2026-09-21，
**在 09-20 那份留档之后、在本包起点之前**）把 `ShadowTriangle` 的 `Transform` 从
`position=0.0,0.70,0.0` 改成 `position=0.0,0.00,0.0`——物体下移，内容的下边界从 179 扩到 205，
方向和量级都对得上。其余五份（2d / mixed / hud / lighting / editor_preview）**逐值相同**，
`ignored_noise=3` 全在 `editor_preview` 的 `p50_us/p95_us/…`（脚本的计时白名单）。

**B. `wp0_scene_frame` 与 2026-09-26 那份报告对照**（比 09-20 那份更近，且两边场景数都是 24）

```
golden_compare_ok files=1 compared=1,057 semantic_diffs=6 ignored_noise=576
  entries[11] game_api/index:            entities 33->34, typed 114->115
  entries[16] game_jhyt/cold_spring_cave: entities 12->13, typed  28->30
  entries[22] game_jhyt/mountain_path:    entities 27->32, typed  58->68
```

6 条全部是 `.scene` 文件里的实体数，三条都能对上内容提交：
`git log --since=2026-09-25 -- examples/game_api/scenes/index.scene` → `f8a747a`(09-29)，
`…/cold_spring_cave.scene` 与 `…/mountain_path.scene` → `257b5ec`(10-03)、`61d0825`(10-02)，
**都是 `42a565c` 的祖先**。除这三个场景外的 1,051 个字段逐值相同（帧时间、每帧分配、
draw/instance/triangle 计数都在"相同"那一侧）。

**C. 几何量与分配峰值 vs 入库基线**（`-Mvp` 不含这一步，单独跑；这是跨机可比的那道门）

```powershell
cargo run --quiet --release --manifest-path engine/Cargo.toml -p engine --example wp0_3d_stress -- --cycles 30 --output <tmp>/wp0-3d-stress.json
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/compare-baseline.ps1 `
    -Current <tmp>/wp0-3d-stress.json -Previous artifacts\baselines\wp0-3d-stress.committed.json -GeometryOnly
```

`baseline_compare_ok checks=12 regressions=0 mode=geometry-only`：三档 mesh_count（100/1000/10000）
的 draw_calls / instances / triangles **逐值相同**，`cpu_peak_bytes` 相对 2026-09-19 入库基线
**-27.11% / -21.76% / +2.52%**（阈值 ±10%，全部不判红）。计时（p95/p99）在这一档只打印，
读数为 -8.49% / +30.30% / +26.90%——**这条不构成性能结论**：与入库基线差 14 天，中间隔着
`f8a747a` 那批优化与本机负载，只做"没有跌破红线"用。

**D. 编辑器路径**：`verify.ps1` 的 `==> Editor performance baseline` `scenarios=32 failed=0`，
`==> Editor GUI smoke` `cases=41 failed=0`，`wp0_editor_preview_golden_ok painted=65536 vertices=6 draws=1`。

**诚实边界（这条最重要）**：本包**没有**做"同一台机器上 `42a565c` 跑一份、HEAD 跑一份"的同机前后对照。
原因是那份"previous"报告在仓库里不存在（`artifacts/baselines/` 只入库
`wp0-3d-stress.committed.json`，其余是本机运行证据，最近的一批是 09-20/09-26），
补做要付一次 release 冷重建。所以上面 A/B 两组的论证方式是
**"逐条差异 → 归属到某个内容提交 → 用 `git log` 复现"**，而不是"总量相等"。
支撑这个论证成立的前提有两个，都已实测：本包 8 个搬运 commit 碰过的文件全部在 `host/` 里（§14.4 ⑤），
且 `engine` 的单测数与拆分前逐文件计数相等（§14.4 ①）。

### 14.8 证明"纯搬运"的脚本口径（为什么逐行 diff 一定不够）

每一刀都用同一个临时脚本（`%TEMP%/wp12/proof4{a,b,c,d}.mjs` + `*.spec.json`，本包收口后删除；
下面写的是口径，不是可复用工具——可复用性不在本包预算里）。判定分三层：

1. **搬出的每一段**：源文件该行的区间取出来，**去掉注释、去掉全部空白**后，必须原样出现在目标文件里。
2. **留在父文件的每一段**：按原顺序做**单调包含**（`monotone ordered containment`）——
   即"剩下的东西还是那个顺序那个内容"，只是中间少了被搬走的段。这一条挡住"搬走时顺手改了文字"。
3. **头部被删掉的名字**：每个被从 `use`/`mod` 区域剪掉的名字，断言它(a)出现在某个**指名**的目标文件里，
   且(b)不再出现在父文件的 use 区域。防止"名字消失了但代码没搬"。

归一化掉的三类"必然噪声"：`pub(super)` 前缀（§14.3 允许的放宽）、rustfmt 为竖排签名补的**尾逗号**、
以及被重排过的 `use` 块（`headExempt` 跳过该区）。**如果换成逐行 diff，这三类都会假红**：
rustfmt 在 `pub(super)` 存在时会把 `fn f(a: A, b: B)` 折成每参数一行并补尾逗号，
同一段代码的字节变了但语义没变——这正是 §9 坑 5 说"每阶段用 clippy 清零孤儿 use"的同一件事的另一面。

复核命令（任何一刀）：`node %TEMP%/wp12/proof4c.mjs --spec %TEMP%/wp12/proof4c.spec.json`；
`wc -l` 的行数对照见 §14.1，回滚档见 §14.2。

### 14.9 收口动作与两处自己造成的回归（都已修）

1. **`check-docs` 一开始是红的**：`docs/24-3d-rendering.md:84` 链到已被搬走的
   `host/renderer.rs`。`check-docs.ps1` 只校验链接目标存在性，所以这条必须改；改成
   `../engine/crates/engine/src/host/renderer/mod.rs`。**顺带把路径写错的账一次结清**：
   `docs/23-工程目录功能说明.md` 的 `host/` 目录树、`docs/54-2D3D统一优化方案与现状对比.md`
   里那两个"上帝文件"条目都改成目录形态（列子模块，不再列不存在的单文件）。
   复现：`powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-docs.ps1` → `check_docs_ok`。
   §8 说"本工作包不改 `docs/`"，这里改的是**因为拆分而失效的路径**，不是文档结论——
   散文里的行号引用按 §12 第 8 项**刻意不动**。
2. **`cargo doc` 报出 2 条我自己造的回归**：`runtime_scene/mod.rs` 的模块文档里
   `[`SceneMesh`]` / `[`Scene3d`]` 这两个短链接失效了——那两个 `use` 跟着建帧代码搬进了
   `frame_data.rs`，父模块只剩这段注释还需要它们。改成写全路径
   `` [`SceneMesh`](crate::host::frame::SceneMesh) ``，并在文档注释里注明为什么必须写全路径。
   复跑 `cargo doc --no-deps --workspace --lib` = **0 警告**。
3. **测试计数的一处假象**：`grep -c '#\[test\]'` 数出来 `host/` 是 159，比 158 多 1。
   多出来的那一行是 `renderer/tests/mod.rs` **头注释里引用了那个属性字面量**。改成
   "本目录内没有测试之外的 helper"的措辞后回到 158（既没有新增用例也没有丢用例：
   改动前后 `#[test]` 下的 `fn` 名集合逐条相同）。这就是 §12 第 7 项第二条坑的真身。
4. **临时物**：`%TEMP%/wp12/`（脚本、spec、before 快照、日志、golden/baseline 报告目录）
   在收口 commit 之后删除；`artifacts/baselines/` 里本次新跑的报告按该目录既有惯例留在本机
   （只有 `README.md`、`engine-pub-surface.json`、`wp0-3d-stress.committed.json` 入库）。
5. **归档这一步本包没做**：按 `tasks/README.md` 的生命周期，验收通过的任务要从 `plan/` 移进
   `done/`。这一步**刻意留到下一批**：`tasks/README.md` 当前有另一路未提交的在途改动
   （它的 `plan/` 条目正指向 `plan/WP12-host拆分.md`，还把本包写成"待认领"），移动文件会让
   那个链接变死链（`check-docs.ps1` 会扫 `tasks/README.md`，死链接必判红），而修它就得
   替别人提交它的改动。所以下一个人要做的是：`git mv tasks/plan/WP12-host拆分.md tasks/done/` +
   把 `tasks/README.md` 的 `plan/` 条目改成"已归档 `done/WP12-host拆分`" +
   把 `CHANGELOG.md` 本条里的路径一起改过去。
6. **§14.3 的 61 处逐文件实测**（收口时重跑，与本节上面那份账逐格相同）：
   `renderer/` = `arena.rs` 2 + `gpu_mesh.rs` 2 + `gpu_texture.rs` 6 + `targets.rs` 4 +
   `timestamp.rs` 6 + `world_sprite.rs` 10 = **30**（`construct.rs`/`passes.rs`/`readback.rs` 为 0）；
   `runtime_scene/` = `camera.rs` 6 + `frame_data.rs` 3 + `mesh_ui.rs` 8 + `scene.rs` 3 +
   `schedule.rs` 2 + `tilemap.rs` 9 = **31**。命令：
   `grep -r "^\s*pub(super) " engine/crates/engine/src/host/{renderer,runtime_scene}/*.rs | wc -l` → 61。
