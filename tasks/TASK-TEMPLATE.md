# AI 开发任务模板

## 任务编号和名称

例如：M1-01 窗口与应用生命周期

## 状态

BACKLOG / READY / IN_PROGRESS / REVIEW / ACCEPTED / REWORK / BLOCKED

## 目标

描述用户可观察的结果。

## 背景

说明为什么需要此任务，以及它与当前里程碑的关系。

## 输入

- 相关计划或 ADR；
- 当前代码；
- 当前资源；
- 依赖版本文档。

## 不做什么

明确本任务排除的功能，防止 AI 扩大范围。

## 技术方案

由 AI 在实施前先填写，包含模块、接口、生命周期和错误处理。

## 修改范围

列出允许修改的目录和文件。

## 验收标准

使用可观察、可测试、可复现的条目。

## 测试命令

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace
cargo build --workspace --release
```

## 验收证据

记录测试日志、截图、录屏、构建产物和性能数据路径。

## 风险

列出兼容性、性能、生命周期和回滚风险。

## AI 输出

- 已完成内容；
- 修改文件；
- 验证结果；
- 已知问题；
- 下一步建议。

## 已知限制

不得将未完成内容描述为完成。