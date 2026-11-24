# M84-02 P1 前置门：官方 Golden 三方对拍报告

> 阶段：P1 前置门（已完成，**放行 P1 上层实现**）。日期：2026-11-19。
> 承接：`M84-01-P0-Spine运行时对拍报告.md` §5.2 / §7 遗留风险。
> 对应方案：`M84-00-Spine2D接入-现状分析与实施方案.md` §3 / §5 / §7。
> 工具位置：`tools/spine-spike/golden/`（随 spike 临时归档，不进引擎主干）。

## 1. 结论（先看这里）

**P0 遗留的骨骼求解争议已裁决：`spine2d 0.5` 与官方运行时一致，`chine 0.2` 是偏差方。**

- 8 组官方 4.3 样例动画、每组 60 帧、逐帧逐骨骼比较世界变换 `[worldX, worldY, a, b, c, d]`：
  `spine2d` **8/8 全部一致**（最大误差 0.0352 px，绝大多数 < 0.002 px）；
  `chine` **4/8 偏差 4.4–25.9 px**（idle/run/walk/jump），另外 4 组一致。
- 偏差骨骼链与 P0 发现完全吻合：`muzzle* → gun → gun-tip → rear-upper-arm → back-shoulder`。
- 因此 **不切换默认运行时**，P1 按 `spine2d` 铺开；P0 §5.2 的风险项关闭。
- `chine` 仍保留为备选，但其骨骼求解存在实证缺陷，**在修复并对拍通过前不得替换 backend**。

## 2. 裁判与口径

| 项 | 值 |
|---|---|
| 第三方裁判 | 官方 `spine-ts/spine-core` **4.3.13**（上游 4.3 分支 commit `d913caab6ead0a4c9165548bb9dfa4bd35cdd331`，与 P0 同源） |
| 运行方式 | `esbuild 0.28.2` 将官方 TS 源码 bundle 成 ESM，Node `22.22.2` 直接执行；不依赖 canvas/DOM（`FakeTexture` + `AtlasAttachmentLoader`） |
| 比较单位 | 每帧每骨骼 6 个分量：世界位置 `worldX/worldY` 与矩阵 `a/b/c/d` |
| 推进口径 | `state.update(dt) → skeleton.setupPose() → state.apply() → updateWorldTransform(Physics.none)`；Rust 侧 spine2d/chine 用同口径 |
| 一致性阈值 | 位置误差 ≤ 0.05 px 且矩阵元素误差 ≤ 1e-3（给 f32 量化留余量） |
| 样例导出 | 官方仓库 `examples/spineboy/export`、`examples/raptor/export`（哈希见 P0 报告 §1） |

为什么可以当裁判：官方 TS runtime 是 Spine 作者维护的参考实现；它既不是 spine2d 也不是 chine，
且加载的是同一份官方导出数据，因此能区分“哪个实验实现算错了”。

## 3. 全量对拍结果

命令：`python golden/run_golden_gate.py`（60 帧 / 动画，`dt = 0.0166667`）。

| 样例 / 动画 | spine2d vs 官方 | 最大误差 | chine vs 官方 | 最大误差 |
|---|---|---:|---|---:|
| spineboy / idle | MATCH | 0.000145 px | **MISMATCH** | **4.4233 px** |
| spineboy / run | MATCH | 0.001137 px | **MISMATCH** | **25.878 px** |
| spineboy / walk | MATCH | 0.000725 px | **MISMATCH** | **13.956 px** |
| spineboy / jump | MATCH | 0.000707 px | **MISMATCH** | **15.204 px** |
| spineboy / shoot | MATCH | 0.000767 px | MATCH | 0.000771 px |
| spineboy / death | MATCH | 0.000656 px | MATCH | 0.000661 px |
| raptor / walk | MATCH | 0.000755 px | MATCH | 0.000720 px |
| raptor / roar | MATCH | 0.035151 px | MATCH | 0.035111 px |

- 最大偏差骨骼：`muzzle-ring4@run f58`（25.88 px）、`gun-tip@idle f49`（4.42 px）、
  `rear-upper-arm@walk f59`（13.96 px）、`gun@jump f5`（15.20 px）。矩阵分量误差均在 1e-5 量级，
  说明差的是骨骼**平移求解**，不是旋转/缩放，也不是顶点格式。
- `raptor/roar` 的 0.035 px 是单帧瞬时值（同为两实现共有，均值 1.4e-4），源于约束求解的数值抖动，
  在阈值内，不构成风险。
- 追加验证 `plain` 口径（不逐帧 `setupPose`）：spine2d/chine 的数值与 `reset` 口径**完全相同**，
  说明裁决不受该口径选择影响。

## 4. 对方案的影响

1. **P0 风险关闭**：`M84-00` §7 中“两实验实现互相对拍不能裁决”的表述改为已裁决；
   `back-shoulder → rear-upper-arm → gun/muzzle` 链经官方验证无误。
2. **P1 放行**：`M84-00` §5 的前置门已过，可以开始 `crates/rustlua-spine` 的 `backend/` 与
   `player/` 实现；不必再等任何运行时选型验证。
3. **回归门固化**：P1 起每次修改 backend（含未来升 4.4/5.x、换 crate）必须跑
   `python golden/run_golden_gate.py`，全绿才算过；单案例可用 `compare_three.py` 定位到骨骼级。
4. **备选降级**：`chine` 的 `RenderScratch` 稳态零分配设计仍值得 P4 借鉴，但其 4.3 骨骼求解
   有缺陷；若未来考虑切换，须先修复并通过同一 golden 门，而不是仅凭计数一致。

## 5. 边界与未完成项

- 本次 golden 覆盖**骨骼世界变换**，未做官方 TS 的逐顶点 draw list 对拍：官方 TS 的 draw 输出
  依赖 canvas 后端（`spine-canvas`），本 spike 只 bundle 了 `spine-core`。不过 draw list 的顶点
  由骨骼矩阵驱动，且 P0 已证明 spine2d 的 JSON/SKEL 拓扑零差异、颜色/索引/UV 一致，
  因此骨骼裁决足以解除“建立在错误骨骼求解上”的风险。若 P2 发现网格级异常，再补 canvas 后端对拍。
- golden 工具依赖 `esbuild`（npx 按需获取）与 Node 22；二者只在 spike 内使用，**不进引擎依赖图、
  不影响 deny.toml 白名单**，也不要求 CI 引入 Node。

## 6. 复跑命令（在 `tools/spine-spike/` 下）

```bash
# 一次性：重建官方 runtime bundle（已生成则跳过）
npx --yes esbuild@0.28.2 assets/upstream-repo/spine-ts/spine-core/src/index.ts \
  --bundle --format=esm --platform=node --outfile=golden/spine-core.mjs

# Rust 侧骨骼导出器
cargo build --features json,binary --bin bone_dump

# 全量回归门（P1 起每次 backend 改动都跑）
python golden/run_golden_gate.py

# 单案例定位（可选 reset|plain）
python golden/compare_three.py \
  assets/upstream-repo/examples/spineboy/export/spineboy-pro.json \
  assets/upstream-repo/examples/spineboy/export/spineboy.atlas run 60 0.0166667
```
