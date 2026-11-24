# GPU 测试怎么跑（M84 诊断结论，2026-11-19）

回答一个问题：为什么 `cargo test -p rustlua-gpu` 在这台机器上"跑不完"，以及正确的跑法。
结论来自实测，不是推断。

## 事实

本机（RYJ-WORK，i5-12400F + GT 1030 2GB）**有可用的独显**：

```
cargo run -p rustlua-gpu --example gpu_probe
-> [0] NVIDIA GeForce GT 1030        Vulkan  DiscreteGpu
   [2] Microsoft Basic Render Driver Dx12    Cpu        <- 软件光栅化
```

## 两个独立原因

### 原因一：测试被写死用软件渲染器

10 处 `test_device()` 原来都硬编码 `force_fallback_adapter: true`，于是**每个** GPU 测试
都跑在 `Microsoft Basic Render Driver` 上，GT 1030 全程闲置。

| 条件 | depth_sort（2 个测试） |
|---|---|
| 软件（旧行为） | 18.2 s |
| 硬件 | 2.7 s |

**6.6 倍差距。** 已改为共享策略 `rustlua_gpu::test_adapter::request_test_adapter()`，
由 `RUSTLUA_TEST_GPU` 环境变量控制，**默认仍是 software**（不让 CI 结果漂移）。

### 原因二：GT 1030 只有 2GB 显存，高并发会互相拖死

默认并行度（= 逻辑核数）下，几十个测试同时建 GPU 设备与纹理，显存与驱动队列争用，
单个测试从 27 s 涨到"60 s 还没完"。

实测 `water` 模块（9 个测试）：

| `--test-threads` | 结果 |
|---|---|
| 8（接近默认） | 超时、跑不完（部分测试 >60 s 未结束） |
| 2 | **101 s 全部通过** |

这不是崩溃（没有 FAILED），是资源争用导致的总时长爆炸。同一现象也解释了
`exit code 2173` —— 那是 cargo 的内建超时，不是测试失败。

## 正确跑法

```powershell
# 全量（推荐：硬件 + 低并发）
$env:RUSTLUA_TEST_GPU = "hardware"
cargo test -p rustlua-gpu --lib -- --test-threads 2

# 单模块（最快，日常改动用这个）
$env:RUSTLUA_TEST_GPU = "hardware"
cargo test -p rustlua-gpu --lib renderer::tests::water_

# CI / 无显卡机器（默认行为，不用设环境变量）
cargo test -p rustlua-gpu --lib
```

`RUSTLUA_TEST_GPU` 三档：

| 值 | 行为 |
|---|---|
| `software`（默认） | 强制软件回退，行为与改动前**完全一致** |
| `hardware` | 优先真实 GPU，拿不到回退软件并打印一行 |
| `strict` | 必须真实 GPU，拿不到 panic（确认"这台机器真能跑硬件路径"） |

## 顺带修掉一个假门

`renderer::tests::hdr::ssr_misses_fall_back_to_the_procedural_sky` 断言 SSR 参数缓冲在
`construct/mod.rs` 里，但拆分批之后它已搬到 `construct/post_process.rs`。

这条断言**在软件档和硬件档都恒定失败**——一个早已失效、却被"整体跑不完"掩盖住的门。
现在它检查正确的文件，并且真的在起判据作用。

**教训**：一个长期跑不完的测试套件，会让里面所有失效的门都失去意义。修好"怎么跑"之前，
不能相信"这个套件是绿的"。
