# M1 里程碑验收：v0.1-foundation

## 状态

- 状态：PASS
- 验收日期：2026-08-20
- 目标标签：`v0.1-foundation`

## 验收范围

M1 基础运行时包含以下任务：

- M1-01 窗口与应用生命周期；
- M1-02 wgpu 初始化与清屏；
- M1-03 Lua VM 桥接；
- M1-04 日志与错误系统；
- M1-05 固定时间步；
- M1-06 输入快照；
- M1-07 性能 overlay。

## 验收结果

| 检查项 | 结果 | 证据 |
|---|---|---|
| M1-01 至 M1-07 任务记录 | PASS | `tasks/done/M1-01-窗口与应用生命周期.md` 至 `tasks/done/M1-07-performance-overlay.md` |
| Debug 质量门 | PASS | `artifacts/logs/quality/verify-20260820-170201.log` |
| Release 质量门 | PASS | `artifacts/logs/quality/verify-20260820-170204.log` |
| workspace 测试 | PASS，19 项通过 | Debug/Release 质量门日志 |
| Clippy | PASS，`-D warnings` | Debug/Release 质量门日志 |
| Debug Demo | PASS，窗口、GPU、Lua、性能日志正常，运行 3 秒后安全结束 | `artifacts/logs/M1-foundation-session-debug.stdout.log`；`artifacts/logs/M1-foundation-session-debug.stderr.log` |
| Release Demo | PASS，窗口、GPU、Lua、性能日志正常，运行 3 秒后安全结束 | `artifacts/logs/M1-foundation-session-release.stdout.log`；`artifacts/logs/M1-foundation-session-release.stderr.log` |
| Demo stderr | PASS，两个 stderr 文件为空 | Debug/Release Demo stderr 日志 |
| Git 提交链 | PASS，M1-01 至 M1-07 连续提交可追溯 | `3417966` 至 `ddaaa61` |

## 结论

M1 基础运行时达到放行标准。允许创建并核验 Git 标签：

```text
v0.1-foundation
```

该标签对应 M1 验收提交，不代表跨设备性能基准；当前 FPS 仅用于验证性能采样和 overlay 链路。

## 已知环境信息

本机真实 Demo 使用 NVIDIA GeForce GT 1030 和 wgpu Mailbox present mode。Vulkan loader 输出的外部 Steam overlay manifest 诊断属于主机图形环境信息，不影响 GPU 初始化；Demo stderr 仍为空。
