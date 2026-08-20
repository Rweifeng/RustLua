# M1-07 性能 overlay

## 状态

- 状态：PASS
- 完成日期：2026-08-20
- Git 提交：待提交

## 实施内容

- `engine_core::performance` 提供帧、固定更新、Lua 调用耗时和 FPS 采样。
- `game_runner` 接入每帧采样，并按固定更新步骤统计 Lua 调用耗时。
- wgpu 绘制左上角半透明绿色性能面板和四条性能条：FPS、Frame ms、Update ms、Lua ms。
- overlay 默认开启，F3 切换，并输出 `[perf] overlay enabled/disabled` 日志。

## 验收结果

- 格式检查：PASS，`cargo fmt --all -- --check`。
- Clippy：PASS，`cargo clippy --workspace --all-targets --all-features -- -D warnings`。
- 单元测试：PASS，19 项通过。
- Debug/Release 质量门：PASS，见 `artifacts/logs/quality/verify-20260820-164230.log` 和 `verify-20260820-164231.log`。
- Debug/Release Demo：PASS，真实窗口和 NVIDIA GeForce GT 1030 GPU 初始化成功，overlay 性能日志持续输出，运行 5 秒后安全结束。
- Demo stderr：PASS，两个 stderr 证据文件为空。

## 设计边界

本里程碑不引入字体渲染系统。overlay 先提供低成本的面板和趋势条，数值通过结构化日志输出；后续 UI/字体系统建立后再增加文字绘制。

## 风险与后续

- 当前窗口使用 Mailbox present mode，FPS 结果用于验证采样链路，不作为跨机器性能基准。
- Vulkan loader 的外部图形层诊断未进入 Demo stderr，也未阻止 GPU 初始化。
- F3 切换路径已接入 `NamedKey::F3` 和日志；交互录制可在后续自动化窗口测试中补充。
