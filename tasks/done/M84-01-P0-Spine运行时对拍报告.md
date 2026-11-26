# M84-01 P0：Spine 运行时对拍报告

> 阶段：P0 无侵入技术验证（已完成；2026-11-26 归档）。日期：2026-11-19。
> 对应方案：`M84-00-Spine2D接入-现状分析与实施方案.md` §3 / §4.1。
> spike 位置：`tools/spine-spike/`（临时，验收后归档或删除；官方示例导出不入库）。

## 1. 环境三元组（升级回归基线）

| 项 | 值 |
|---|---|
| Rust | `1.97.1-x86_64-pc-windows-msvc`（spike 独立 `rust-toolchain.toml` 锁定） |
| 目标 | native + `wasm32-unknown-unknown` |
| spine2d | `0.5.0`，MIT OR Apache-2.0，MSRV 1.95，只用 core（不引 wgpu 后端） |
| chine（备选） | `0.2.0`，MIT，默认 features（json + binary） |
| 上游示例 | `EsotericSoftware/spine-runtimes` 分支 `4.3`，commit `d913caab6ead0a4c9165548bb9dfa4bd35cdd331` |
| Spine Editor 导出版本 | 4.3.x（上游 4.3 分支自带导出，未另开 Editor 重导） |
| 骨骼 SHA256-16 | `spineboy-pro.json` `8bfcf88077364594` / `spineboy-pro.skel` `e10de3f2473a3713` / `spineboy.atlas` `779d1b252362187b` / `spineboy-pma.atlas` `6f927e25d0350e20` / `raptor-pro.json` `503a5d7ba4b08bf8` / `raptor-pro.skel` `796a673fc1d0246b` |

## 2. P0 门槛逐项结果

| 门槛 | 结果 | 证据 |
|---|---|---|
| 1.97.1 `cargo check` | ✅ | 两个 crate 与 spike 全部通过 |
| wasm32 `cargo check` | ✅ | spine2d 与 chine 均通过（spike 的 `std::fs` 仅编译期，不运行） |
| 加载 4.3 示例 `.json`/`.skel`/`.atlas` | ✅ | spineboy / raptor 的 json、skel、normal 与 PMA atlas 全部解析成功 |
| 逐帧输出 draw list | ✅ | 顶点/索引/颜色/dark/blend/PMA 均可读到；见 §3 |
| 与官方同源导出对拍 | ✅ | JSON 与 SKEL 逐帧拓扑零差异，几何误差亚像素；见 §4 |
| 与另一运行时对拍 | ⚠️→✅ **已裁决（P1 前置门）** | 顶点/三角形计数 100% 一致；骨骼偏差经官方 golden 裁定为 chine 侧问题，spine2d 无误；见 §5.2 与 `M84-02` |
| `cargo deny` 许可证 | ✅ | spine2d MIT/Apache、chine MIT、glam/serde 均在 `deny.toml` 白名单内，无需新增例外 |

## 3. 官方样例渲染覆盖

| 样例 | atlas | 动画 | draws | 顶点 | 三角形 | blend |
|---|---|---|---:|---:|---:|---|
| spineboy | spineboy.atlas | idle / run / walk / jump / death | 1 | 276 | 343 | normal |
| spineboy | spineboy.atlas | shoot | 2 | 280 | 345 | normal + additive |
| spineboy | spineboy-pma.atlas | run | 1 | 276 | 343 | normal，PMA=1 |
| spineboy | spineboy-pma.atlas | shoot | 2 | 280 | 345 | normal + additive，PMA=1 |
| raptor | raptor.atlas | walk | 4 | 667 | 776 | normal |
| raptor | raptor.atlas | roar | 3 | 642 | 753 | normal |
| raptor | raptor-pma.atlas | walk | 4 | 667 | 776 | normal，PMA=1 |

结论：atlas 的 `pma: true` 能正确传播到 draw；additive blend、多 draw、多页纹理路径均可表达。

## 4. 官方同源对拍（spine2d 解析 JSON vs SKEL）

同一份官方导出同时提供 `.json` 与二进制 `.skel`，用 spine2d 分别加载、同样推进 60 帧：

| 指标 | spineboy run | raptor walk |
|---|---:|---:|
| topology_mismatches（draw/顶点/索引/blend/PMA/颜色） | **0** | **0** |
| max_position_err | 0.00606 px | 0.00684 px |
| max_uv_err | 5.96e-8 | 5.96e-8 |
| max_color_lsb / max_dark_lsb | 0 / 0 | 0 / 0 |
| max_index_err | 0 | 0 |

结论：差异来自 f32 十进制文本与二进制存储精度，**可解释、亚像素级**。这一项通过。

## 5. 跨实现对拍（spine2d 0.5 vs chine 0.2）

### 5.1 计数与 draw 粒度

9 组组合（spineboy × 5 动画 + PMA × 2 + raptor × 2）逐帧比较，**顶点数与三角形数 100% 一致**。
draw 数不同但可解释：spine2d 把连续同页同 blend 的 slot 合并（spineboy run 为 1 draw），
chine 逐 slot 输出（同为 18 个 command）。引擎侧按页 + blend 合批即可统一。

### 5.2 骨骼世界坐标（风险发现）

| 动画 | IK 轨道 | 最大骨骼世界坐标偏差 | 偏差集中骨骼 |
|---|---|---:|---|
| shoot | 无 | 6.1e-5 | —（一致） |
| death | 4 个 | 6.1e-5 | —（一致） |
| idle | 无 | 1.79 px | back-shoulder / rear-upper-arm |
| jump | 4 个 | 12.68 px | back-shoulder / rear-upper-arm |
| walk | 2 个 | 13.04 px | gun-tip / back-shoulder |
| run | 2 个 | 24.02 px | muzzle 链 / back-shoulder / rear-upper-arm |

- 偏差**不是简单的“有无 IK”**：death 有 4 个 IK 仍一致，idle 无 IK 却偏 1.79 px。
  共同点是 `back-shoulder → rear-upper-arm → gun/muzzle` 这条链在不同动画下被求解。
- 唯一顶点集比较中，`shoot` 的最大位置误差仅 1.5e-4（两实现一致），
  其余动画的大误差是骨骼偏差向下游传播的结果，不是顶点格式问题。
- P0 时两边都是实验性实现，互相对拍无法裁决；**该问题已在 P1 前置门解决**：
  官方 spine-ts 4.3.13 golden 对拍显示 spine2d 误差 ≤0.0012 px（这组动画），
  chine 偏差 4.4–25.9 px，**偏差方是 chine**。详见 `M84-02-P1前置门-官方Golden对拍报告.md`。
- P0 唯一未完全通过的门槛至此关闭。

## 6. 定案

- **默认采用 `spine2d 0.5`**：依赖、wasm32、官方样例加载、draw list、官方同源对拍全部通过，
  许可证无例外；其 renderer-agnostic `Vertex`/`DrawList` 与方案 §4 的 `SpineRenderOutput` 对接成本最低。
- **`chine 0.2` 保留为备选**：编译、wasm32、许可证同样通过，计数对拍完全一致；
  其 `RenderScratch` 稳态零分配的设计值得在 P4 性能阶段参考。
- 不切换运行时的前提**已满足**：P1 前置门用官方 golden data 证明 spine2d 的骨骼求解
  （尤其上述骨骼链）与官方一致，偏差来自 chine。默认运行时不再变动；若未来升级 4.4/5.x，
  按 §4.1 只替换 `backend/`，上层不动。

## 7. 后续动作（移交 P1）

1. ~~引入官方 runtime 的 golden 输出作为第三方裁判~~ **已完成（P1 前置门）**：
   官方 spine-ts 4.3.13 三方对拍结果见 `M84-02`；8/8 样例裁定 spine2d 一致。
2. ~~把 §1 的三元组与本报告的对拍命令固化为回归脚本~~ **已完成**：
   `tools/spine-spike/golden/run_golden_gate.py`，P1 起每次 backend 改动都跑。
3. P1 实现 `backend/` 时以 `build_draw_list_with_atlas` / `append_draw_list_with_atlas` 为入口，
   后者可复用 buffer，契合稳态零分配目标。
4. 记录 clipping：spine2d 内部有 `SkeletonClipper`，能输出官方 raptor 的 clip 结果；
   但按已定决策，引擎 P2 仍在导入期拒绝 clip attachment，stencil 留后续增量。

## 8. 复跑命令（在 `tools/spine-spike/` 下）

```bash
cargo check --features json,binary
cargo check --target wasm32-unknown-unknown --features json,binary
cargo build --features json,binary
# 官方同源对拍
./target/debug/compare.exe assets/upstream-repo/examples/spineboy/export/spineboy.atlas \
  assets/upstream-repo/examples/spineboy/export/spineboy-pro.json \
  assets/upstream-repo/examples/spineboy/export/spineboy-pro.skel run 60 0.0166667
# 跨实现计数对拍
./target/debug/chine_dump.exe assets/upstream-repo/examples/spineboy/export/spineboy.atlas \
  assets/upstream-repo/examples/spineboy/export/spineboy-pro.json run 30 0.0166667
# 骨骼世界坐标对拍（两实验实现）
./target/debug/bone_compare.exe assets/upstream-repo/examples/spineboy/export/spineboy-pro.json run 30 0.0166667
# 官方 golden 三方对拍（P1 前置门，全量回归）
python golden/run_golden_gate.py
```

## 9. 归档记录（2026-11-26）

- **归档依据**：P0 六项门槛全部通过（§2）；§5.2 遗留的跨实现骨骼偏差已由 `M84-02` 的官方 golden
  裁决（spine2d 一致、chine 为偏差方）。§7 的四条移交项全部落地：①第三方裁判 = `M84-02`；
  ②回归脚本 `tools/spine-spike/golden/run_golden_gate.py`；③P1 按建议以
  `append_draw_list_with_atlas` 为入口（`backend/spine2d_backend.rs`）；④clipping 按既定决策在 P2
  改为导入期拒绝。本文件无剩余可执行条目。
- **本次复验（提交态 `a5dc4b44` 的干净 worktree，2026-11-26）**：
  `python golden/run_golden_gate.py` = **PASS**，8 行读数与 `M84-02` §3 逐行一致
  （`spineboy/run` spine2d 0.0011373 / chine 25.878；`raptor/roar` 0.035151）；
  `cargo test -p rustlua-spine` = 15 passed / 0 failed。
- **只登记、不开新包**：
  1. §8 的 `compare.exe` / `chine_dump.exe` / `bone_compare.exe` 依赖 `tools/spine-spike/target/debug/`
     的本地产物与 `assets/upstream-repo`（官方导出不入库）。新克隆上要先按 §8 重建；
     golden 门本身只需 `bone_dump`（命令见 `M84-02` §6）。
  2. 未来升 Spine 4.4/5.x 或换 backend 时，仍须先跑 golden 门再改上层（`M84-00` §4.1 的隔离层约定）。
- **归档动作**：移入 `tasks/done/`，保留原名；`tasks/README.md` 与 `.gitignore` 的路径引用同步。
