# 任务：Compute Shader 支持（能力接缝 + 第一个真实消费者）

- 任务 ID：M74-03
- 状态：已排期（未开工，**排在 PBR/HDR 之后**；无消费者就不开工）。**2026-10-20 批次归属：
  由 [`P3-渲染与编辑器能力`](P3-渲染与编辑器能力.md) 的 `P3-R06` 认领，首个消费者固定为
  `P3-R07` GPU 粒子；本任务书仍是 compute 的详细阶段表，P3 不复制它。**
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

（未开工）
