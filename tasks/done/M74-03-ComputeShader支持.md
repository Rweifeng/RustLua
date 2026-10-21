# 任务：Compute Shader 支持（能力接缝 + 第一个真实消费者）

- 任务 ID：M74-03
- 状态：**已完成（2026-10-21）**。P0–P2 与 P4 文档/门已落地；消费者是渲染器级 GPU 粒子
  （`configure_gpu_particles` + `RenderPass::Particle`）。场景/Lua 的 `mode=cpu|gpu` 接线、
  连续发射与 50k 基准仍归 [`P3-渲染与编辑器能力`](../plan/P3-渲染与编辑器能力.md) 的 `P3-R07`。
  **批次归属：`P3-R06` 认领；本任务书仍是 compute 的详细阶段表，P3 不复制它。**
- 关联里程碑：M74（能力补齐批，见[差距分析](../../docs/62-成熟引擎差距分析.md) §6 P1）
- 创建日期：2026-10-19

## 目标

给渲染器加一条 compute 通道路径（管线缓存 + 存储缓冲 + 提交点 + 能力探测 + 无能力时
**显式降级**），并用**一个真实消费者**证明它值得存在。反过来说：只把 `ComputePipeline`
建起来、没有消费者，不算完成——那是"能力表上多了一行"。

## 现状（实测）

| 事实 | 位置 / 读数 |
|---|---|
| 全仓 **0 处** `ComputePipeline` / `@compute` / `dispatch_workgroups`（grep 实证） | `engine/crates/**` |
| 渲染器只请求时间戳查询特性，`required_features` 里没有 compute 相关位 | `engine/src/host/renderer/construct.rs:129-139` |
| wasm 产物走 `webgl` 特性 ⇒ **WebGL2 后端**（微信小游戏只有 WebGL2，没有 WebGPU）；这一点由 GPU 探针的立项动机写明 | `crates/gpu-web-probe/src/lib.rs` 模块头、`game_web` 的 `wgpu` features |
| 能力位可查：`wgpu::DownlevelFlags::COMPUTE_SHADERS = 1 << 0` ⇒ 启动期就能判定"这台机器/这个后端有没有 compute" | `wgpu-types-27.0.1/src/lib.rs:1211` |
| 后处理**不需要** compute：它已经是一条全屏 fragment pass | `rustlua-render/src/post_process.rs`、`host/renderer/passes.rs::draw_post_process` |
| 粒子是 CPU 实现（`step_with_parallelism` 走 `infra::par`），精灵是 CPU 顶点 + 合批（单次 draw 覆盖连续顶点） | `rustlua-ecs/src/particle.rs`、`host/renderer/passes.rs::flush_overlay_batch` |

**结论**：今天 compute 没有消费者。所以本任务的前两天不是写代码，而是**选消费者**并留下
"值得做"的证据；选不出来就该停在探针阶段。

## 平台矩阵（决定设计形态）

| 平台 | compute | 含义 |
|---|---|---|
| 桌面（DX12 / Vulkan / Metal） | ✅ | 可做真实加速 |
| Web + WebGPU 浏览器 | ✅ | 但目前 web 产物开的是 `webgl` feature，**实际落不到** |
| Web + WebGL2（今天的 web 产物） | ❌ | `DownlevelFlags::COMPUTE_SHADERS` 不置位 |
| 微信小游戏（只有 WebGL2） | ❌ | 同上 |

⇒ 任何依赖 compute 的东西**必须是"可选加速"而不是"功能开关"**：没有 compute 时画面/玩法
仍然正确，只是慢一点。这条是设计约束，不是建议。

## 阶段拆分

| 阶段 | 产出 | 估时 | 验收门 |
|---|---|---|---|
| P0 能力探针 | `Renderer::capabilities()` 暴露 `compute: bool`（`DownlevelFlags::COMPUTE_SHADERS` + `max_compute_workgroup_size_*` 非零）；桌面与 `scripts/verify/gpu-web-probe.ps1 -Stage backend` 各留一条读数 | 1 天 | **硬门**：WebGL2 上必须读到 `false`。若读到 `true`，说明我对 WebGL2 的判断错了，整条排期要重写——先钉这一条再谈后面 |
| P1 compute 接缝 | `host/renderer/compute.rs`：pipeline 缓存（WGSL `@compute`）、storage buffer 池、bind group 缓存、提交点（放在 `render_passes` 之前）、可选 readback（仅调试）；无 compute 时所有入口返回 `Unsupported`（**不 panic、不静默空转**） | 1 周 | 单元测试：无能力档下 `dispatch` 返回 `Unsupported`；有能力的桌面档下"dispatch 一个写满常量的 shader → readback 校验"通过 |
| P2 第一个消费者 | 二选一，按 P0 的读数定：①GPU 粒子（10k+ 粒子的位置更新，CPU 档保留为默认与回滚）；②2D 光照/tilemap 剔除 | 1–1.5 周 | 同一初始状态两档对拍：最终状态差异 < 1e-4；帧时间对比（10k/50k）；无 compute 的机器上结果**逐字节等于 CPU 档** |
| P3 第二个消费者（可选） | 按 P2 的收益读数决定，不预先承诺 | 0.5–1 周 | 同上 |
| P4 收口 | 渲染文档回写（哪条通道用了 compute、降级路径怎么走）、CHANGELOG、`verify.ps1` | 2 天 | 全套门 + wasm 编译 + WebGL2 降级路径绿 |

合计 **3–4 周**（含 P0 的否决权），与[差距分析](../../docs/62-成熟引擎差距分析.md) §6 一致。

## 风险与回滚

- **WebGL2 无 compute ⇒ 每条 compute 通道都要养一份回滚实现**，成本直接翻倍。这是本项目
  必须接受的结构性成本（微信是主发布目标之一）。
- **桌面 GL 后端（GLES）能力参差**：`DownlevelFlags` 是按适配器查的，别写死"桌面就有"。
- **WGSL 变体膨胀**：管线缓存与启动时间要一起量；compute 的 shader 不要进 2D 主路径的热启动。
- **与 PBR/HDR 抢同一个渲染器**：两者都改 `renderer/`，顺序上 compute 排后面（PBR 是玩家可见的，
  compute 是潜在能力），否则两个在制品会在同一批文件里打架。
- **回滚档**：`capabilities().compute == false` 或 `set_compute_enabled(false)` 时全走 CPU 路径；
  compute 通道不得成为任何画面的唯一实现。

## 明确不做

- 把后处理迁到 compute（今天一条全屏 pass 就够，迁了只是换写法）。
- 通用"GPU 任务图/跨帧异步 compute"：超出本仓当前规模。
- 为 WebGL2 写 compute 仿真（transform feedback 之类）：收益与维护成本不成比例。

## 验收方式（命令级）

```powershell
cargo run -p game --example jhyt_m95_gpu_particles     # 两档对拍（CPU / compute）
cargo check --workspace --all-targets
cargo check -p game_web --target wasm32-unknown-unknown
scripts\verify\gpu-web-probe.ps1 -Stage backend        # WebGL2 上 compute 必须读 false
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

## 实现记录

### 结论

compute 接缝与第一个真实消费者都在渲染器层落地，**默认行为不变**：没有调用
`configure_gpu_particles` 时，计划里没有 `Particle`、不建 compute 对象、像素与收敛前逐字节相同。
WebGL2 / 微信上 `capabilities().compute == false`，所有入口返回 `ComputeError::Unsupported`。

### 交付（按阶段）

| 阶段 | 落地 | 位置 |
|---|---|---|
| P0 | `RendererCapabilities` 构造期快照：`compute` 由 `DownlevelFlags::COMPUTE_SHADERS` **与** compute 限制值非零共同决定；同时报告 workgroup/invocations/storage 上限与 `compute_unavailable_reason()` | `engine/src/host/renderer/capabilities.rs`；`construct.rs` 构造期采一次 |
| P1 | `renderer/compute.rs`：`ComputeRequest`/`ComputeBinding`、WGSL `@compute` 管线缓存、绑定组缓存（调用方给稳定 key）、dispatch 提交、native 同步 readback；无能力/超限/用法不匹配分别返回 `Unsupported`/`WorkgroupsOutOfRange`/`BindingTooLarge`/`MissingBufferUsage` 等 | 同上；`ComputeStats` 记 dispatches/pipeline/bind-group 命中 |
| P2 | GPU 粒子：CPU 用同一 `ParticleEmitter` 生成初始 burst → storage buffer；WGSL 积分 gravity/drag/rotation/life 并直接写 `WorldSpriteVertex`；复用世界 Sprite 管线/白贴图；新增 `RenderPass::Particle`（`ALL` 9→10，排在世界 Sprite 之后、2D overlay/UI 之前） | `renderer/gpu_particles.rs`、`passes.rs`、`rustlua-render/src/render_plan.rs` |
| P3 | 未做（可选第二消费者不预先承诺） | — |
| P4 | 文档回写（docs/54 的 `Particle`/compute 段）、CHANGELOG、headless 门示例、wasm 编译检查、WebGL2 探针字段 | `docs/54`、`CHANGELOG.md`、`game/examples/jhyt_m95_gpu_particles.rs`、`gpu-web-probe` |

### 关键设计决策（与原因）

1. **随机数不进 shader**。初始状态由 CPU 的 `ParticleSystem::burst_with` 生成再上传；GPU 只做
   积分。否则"两档对拍"会退化成"两个 RNG 是否碰巧一致"，< 1e-4 的判据没有意义。
2. **compute 写裸 `f32` 数组，不写 `array<Vertex>`**。WGSL 的 `vec3` 对齐是 16，`Vertex` 的
   stride 会变成 48，而管线的 `WorldSpriteVertex` 是 36 字节；stride 不一致时顶点拉取错位、
   写入还会越过绑定范围被丢弃——症状正是"draw 调了、屏幕全黑"。裸数组让 9 个 f32/顶点的布局
   由索引算术唯一决定（单测钉住 `WorldSpriteVertex == 36`）。
3. **能力位与策略位分开**。`capabilities().compute` 是硬件/后端事实；`set_compute_enabled(false)`
   是可回滚的宿主策略。后者不会篡改能力读数，重新打开即可恢复。
4. **显式拒绝而不是静默近似**。`emitter.rate > 0`（连续发射）、`capacity < count`、workgroup
   或 storage 超限、非有限参数全部返回可读错误；无 compute 时返回 `Unsupported`，**不**假装
   dispatch 成功，也**不**把失败当成"这一帧没有粒子"。
5. **`Particle` 不是功能开关**。CPU 粒子仍是默认与回滚实现；GPU 档只影响"谁来积分"，
   不引入只有 GPU 才能表达的画面。

### 验收读数（2026-10-21，本机 NVIDIA GeForce GT 1030 / Vulkan）

- `cargo run -p game --example jhyt_m95_gpu_particles`：`compute=true`，workgroup
  `[256, 256, 64]`，10k 粒子 × 120 步；CPU/GPU 最终状态
  `max_position_error=9.155e-5`、`max_velocity_error=3.815e-5`（< 1e-4）；
  `Particle` 进计划，`draw_calls=1`、`instances=10000`、`vertices=60000`、
  `changed_pixels=32212`；关掉 compute 后 step/readback 均 `Unsupported`、计划无 `Particle`、
  像素回到纯背景。
- 同一条门另报 50k × 60 步：CPU 迭代 `103.8ms`，GPU 侧 command 提交 `10.5ms`
  （**仅提交，不含等待 GPU 完成**；本适配器 `timestamp=false`，没有独立 GPU 时间），
  50k 回读全有限，位置差 `4.578e-5`（1e-4 判据只对 10k 档承诺）。
- `cargo test -p engine --lib`：215 通过 / 0 失败（含 `renderer::tests::compute` 五条 GPU 门）。
- `cargo test -p rustlua-render --lib render_plan`：6 通过；`full_frame` 现在断言 10 条通道。
- `cargo check -p game_web --target wasm32-unknown-unknown`、
  `cargo check -p gpu-web-probe --target wasm32-unknown-unknown`：通过。
- **P0 硬门实测**：`scripts\verify\gpu-web-probe.ps1 -Stage backend` 退出 0，报告
  `backend=Gl device=Cpu`（ANGLE/SwiftShader），`downlevel_compute_shaders=false`、
  `compute_capable=false`、`max_compute_workgroup_size_*`/`max_compute_invocations_per_workgroup`/
  `max_compute_workgroups_per_dimension`/`max_storage_buffer_binding_size` **全为 0**；
  `Limits::default()` 请求被拒（`max_compute_workgroups_per_dimension 65535 > 0`），
  `downlevel_webgl2_defaults()` 设备可建。探针新增 `downlevel_compute_shaders` /
  `compute_capable` / `compute_unavailable_reason` 与 compute 限制字段；`gpu_render_probe`
  回传 `engine_capabilities`。这就是 WebGL2 降级路径的证据，不是推断。
  探针现在自己解析 zig（`$env:ZIG` → `engine/.tools/zig` → PATH），上面这条命令在干净环境
  可直接跑，不再先报 `mlua-sys: cannot locate zig`。
- 同一探针 `-Stage render` 也退出 0：真 `engine::host::Renderer` 在 WebGL2 上
  `renderer_created=true`、`draw_calls=2`、`pass_count=2`、`mismatches=0`，
  `engine_capabilities.compute=false`；新加的 compute 路径没有碰坏 2D 渲染。
  最终树复跑报告为 `artifacts/logs/gpu-web-probe/render-probe-20261021-194835.json`。
- **总门 `scripts\verify\verify.ps1`（2026-10-21 复跑）**：文档/ps1 解析、vendor patch、
  toolchain lock、rustdoc、clippy、workspace tests、GPU 门 `compute_gate_ok`、game check、
  editor check（`schema_errors=0`）均通过。当时停在 **editor performance baseline** 的
  `layout_row_height_gate`（`scenarios=31 failed=1`，console compact 行内容 30px > 行高 17.12px）；
  这是既有的 M72-10 UI 密度残项，不是 M74-03 引入。该门已由
  [M72-10a](M72-10a-UI行高门.md) 修复（Console `Wrap` → `Truncate` + 窄宽单测）；
  2026-10-21 全量复跑 `scripts\verify\verify.ps1` 退出 0，`scenarios=31 failed=0`，
  `cases=43 failed=0`，日志 `artifacts/logs/quality/verify-20261021-195452.log`。
- 总门第一次在 editor check 被 PS 5.1 的 `NativeCommandError` 误红：`cargo` 把 linker warning
  写到 stderr，`$ErrorActionPreference = "Stop"` 下 `2>&1` 直接抛。已把 `editor --check`、空间
  迁移、性能基线、GUI smoke 四处捕获统一为 `Invoke-Captured`（局部 `Continue` + `try/finally`
  恢复），复跑后 editor check 正常通过；改动在 `scripts/verify/verify.ps1`。

### 明确边界（留给 P3-R07，不在本任务内假装完成）

- 场景 schema / inspector / Lua 的 `ParticleEmitter.mode=cpu|gpu` 与自动回退标记
  `particle_fallback=cpu`：未接线；当前 GPU 档是宿主/渲染器 API。
- 连续发射（`rate > 0`）、spawn 环形队列、R07 的 1.5× 收益判据与独立 GPU 时间：未做。
  50k 只在示例里量了 CPU 迭代与 command 提交，不是 R07 的完整基准。
- 3D billboard、逐 emitter 排序、GPU→Lua 读回：按任务书"明确不做"。
- wasm 上 `read_storage_buffer` 返回 `ReadbackUnsupported`（浏览器主线程不阻塞映射）；渲染路径
  本身不读回，因此不影响 WebGL2 降级。

### 风险与回滚

- 回滚档一：不调用 `configure_gpu_particles` ⇒ 零 compute 对象、零新 pass。
- 回滚档二：`set_compute_enabled(false)` ⇒ 计划剔除 `Particle`，CPU 粒子照旧。
- 回滚档三：删除 `renderer/gpu_particles.rs` 与 `RenderPass::Particle` 即可回到 9 通道；
  能力快照与 compute 接缝可独立保留（无消费者时不会被调用）。
- 已知成本：WGSL 与 CPU 的浮点舍入不完全一致，10k 粒子 2 秒累积差 ~9e-5；判据是 1e-4，
  余量不大但确定（不随帧数指数发散，drag 把速度压住）。若未来放宽步数需重新量这条门。
