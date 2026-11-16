# 任务：GPU 基准证据的 BLOCKED 语义扩展（无适配器不再 panic 判红）

- 任务 ID：M83-08
- 状态：待定案（等 CI 注解确认 gates 红因）
- 关联里程碑：M83
- 创建日期：2026-11-16

## 目标

把 `verify.ps1 -Benchmarks` 里"拿不到离屏适配器"的处置，从**当前两种口径并存**
（部分示例 `exit 3` = BLOCKED、部分示例 `.expect` panic = 红）统一成一种：
**环境缺口显式 BLOCKED(3) + 有到期日的登记**，同时不放过任何真实渲染回归。

## 一、现状盘点（2026-11-16 静态核对）

`verify.ps1` 的基准段跑了 17 个示例，按"拿不到适配器时怎么死"分三类：

| 类别 | 示例 | 当前行为 | verify.ps1 侧 |
|---|---|---|---|
| BLOCKED-aware | `wp0_anim_golden`、`wp0_material_golden`、`wp0_gpu_particles_golden`、`wp0_gpu_particles_50k` | `exit 3` + 打印 `*_skipped reason=no_adapter` | 已按 3 处理（`RUSTLUA_REQUIRE_GPU=1` 时升级为红） |
| **panic** | `wp0_3d_stress`、`wp0_3d_golden`、`wp0_2d_golden`、`wp0_mixed_golden`、`wp0_hud_golden`、`wp0_lighting_golden`、`wp0_point_light_golden` | `OffscreenTarget::acquire(...).expect("offscreen fixture …")` ⇒ panic ⇒ exit 101 | `Invoke-Checked` ⇒ 抛错判红 |
| CPU-only | `ecs_stress`、`sprite_stress`、`tilemap_stress`、`wp0_scene_frame`、`wp0_ecs_storage` | 不需要适配器 | 与 GPU 无关 |

`quality.yml:375-378` 的注释是当时的决定：**"适配器缺失会让示例报错退出而不是静默跳过——宁可判红，
也不要一份'跑过了但其实没测'的绿灯"**。这个立场本身没错，但它把"环境缺失"和"代码回归"
混成了同一种红——与 M83-04 在 Web 证据上已经解决的那类问题一模一样。

## 二、为什么现在要动

`1864ce63` 的 CI 读数（2026-11-16）：

- `build-test` ✅、`lint` ✅、`security` ✅、`release-closure` ✅；
- `web-evidence` 已收敛成 BLOCKED ✅（M83-04）；
- 但 `gates` 仍红在 `WP0 benchmarks and semantic goldens`，且**连续多个提交同一步红**
  （`44a27bf3`、`1864ce63`，修复前更早的提交也红在这里或更早）。

两种互斥假说，`4b68a60e` 起 gates 步骤会把失败行发成匿名可读的 `::error::` annotation：

- **假说 A（适配器缺失）**：注解里出现 `offscreen fixture` panic / `wp0_3d_stress` 之类的
  示例名 ⇒ 走本任务的"BLOCKED 语义扩展"。
- **假说 B（跨机内存基线漂移）**：注解里出现 `REGRESSION … cpu_peak_bytes … > 10%` ⇒
  走 M83-07 的口径修正（内存峰值是否跨机可比），不是本任务。

## 三、方案（仅假说 A 成立时执行）

1. **示例侧统一出口**：把上表 7 个 panic 示例改成与 `wp0_anim_golden` 同款——
   `match OffscreenTarget::acquire(..) { Ok(f) => f, Err(_) => { eprintln!("..._skipped reason=no_adapter"); std::process::exit(3) } }`；
   抽一个共用小助手（`engine/examples` 内不能共享 crate，就各文件内保持 3 行引导注释 + 同款代码）。
2. **verify.ps1 侧**：把这 7 条从 `Invoke-Checked` 改成"捕获退出码 + 3 视为跳过"，
   与 anim/material/gpu_particles 完全同款；`RUSTLUA_REQUIRE_GPU=1` 时 3 升级为红。
3. **登记与到期**：复用 M83-04 的到期模式，新增 `metrics/gpu-evidence.json`
   （`status`/`since`/`review_by`/`unblock_conditions`）与对应 `check-*.mjs --check`，
   接进 `verify.ps1` node 门组与 CI lint。**BLOCKED 不是通过**，但"永久 BLOCKED"到期判红。
4. **反面对照（必须有）**：
   - 把某个示例的像素 golden 故意改坏 ⇒ 仍判红（证明 BLOCKED 化没有把真回归一起放走）；
   - `RUSTLUA_REQUIRE_GPU=1` 下无适配器 ⇒ 判红（证明发布门不受影响）。

## 四、风险

- 发布/验收门若也在无 GPU 机器上跑，BLOCKED 可能掩盖真实回归。缓解：`RUSTLUA_REQUIRE_GPU=1`
  在发布链上保持"必须真跑"，登记文件只对 CI 诊断轮次生效。
- 7 个示例各改一处 `match`，是机械改动但**会动 diff 面**；必须在对方会话收口后单独一轮做
  （当前它们正在改 `rustlua-gpu` 与 `editor`，避免踩踏）。

## 五、验收方式

```powershell
# 有适配器：仍然真跑（本机）
cargo run -p engine --example wp0_3d_golden -- --output artifacts/tmp/g.json
# 无适配器模拟：BLOCKED 而非红（实现后提供环境开关，如 WGPU_BACKEND=gl 且禁用 WARP）
# 反面对照：改坏 golden 后仍判红
```

## 六、未完成项 / 已知限制

- 尚未实现；先等 `4b68a60e` 的 CI 注解给 gates 红定性（假说 A/B 二选一）。
- 本任务只处理"离屏适配器缺失"，不改变任何像素 golden 的判据。
