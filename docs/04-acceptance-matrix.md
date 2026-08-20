# 验收矩阵

## 使用说明

本矩阵记录每个任务或里程碑的可验证结果。`PASS` 表示已完成验收，`BLOCKED` 表示实现已准备但受外部条件阻塞，`PENDING` 表示尚未开始，`FAIL` 表示验收失败。

| ID | 功能 | 前置条件 | 操作/命令 | 预期结果 | 自动化测试 | 手工证据 | 状态 | 版本 |
|---|---|---|---|---|---|---|---|---|
| M0-A01 | 项目结构 | M0-01 | 检查目录 | 所有基线目录存在 | 目录检查脚本 | `artifacts/screenshots/M0/` | PASS | M0 |
| M0-A02 | 项目章程 | M0-01 | 阅读章程 | 目标、范围、里程碑和验收门完整 | 文档检查 | 章程文件 | PASS | M0 |
| M0-A03 | 任务模板 | M0-01 | 创建示例任务 | 任务含目标、范围、测试和验收 | 模板检查 | 示例任务文件 | PASS | M0 |
| M0-A04 | 可继续开发 | M0-01 | 新 AI 会话读取文档 | 能理解当前状态和下一步 | 文档检查 | 会话验收记录 | PASS | M0 |
| M0-A05 | 质量门脚本 | M0-03 | `./scripts/verify.ps1` | 执行 metadata、fmt、clippy、test、build | PowerShell 脚本 | `artifacts/logs/quality/verify-20260820-113948.log` | PASS | M0 |
| M0-A06 | 测试入口 | M0-03 | `./scripts/test.ps1` | 执行测试质量门 | PowerShell 脚本 | `artifacts/logs/quality/verify-20260820-113952.log` | PASS | M0 |
| M0-A07 | Release 构建入口 | M0-03 | `./scripts/build-release.ps1` | 生成 Release 运行器 | PowerShell 脚本 | `artifacts/builds/release/game_runner.exe` | PASS | M0 |
| M0-A08 | CI 质量门 | M0-03 | GitHub Actions Windows runner | format、clippy、test、Debug/Release build 通过 | CI workflow | Actions 记录 | PENDING | M0 |
| M0-A09 | M0-04 文件完整性 | M0-04 | 检查验收矩阵和风险登记 | 两份文档 UTF-8 正常、字段完整、可直接使用 | 文档检查 | 文件内容 | PASS | M0 |
| M0-A10 | 风险登记 | M0-04 | 检查风险表 | 风险有概率、影响、应对策略、负责人和状态 | 文档检查 | 风险登记表 | PASS | M0 |
| M0-A11 | M0 里程碑验收 | M0-01 至 M0-04 | 执行完整回归验收 | 本地质量门、构建、Demo、文档和风险检查通过 | 回归命令集 | `tasks/done/M0-05-M0里程碑验收.md` | PASS | M0 |
| M0-A12 | M0-06 基线构建和标签 | M0-01 至 M0-05 | 执行质量门并检查 Git 标签 | 基线提交可构建且存在 `v0.0.1-baseline` | 质量门和 Git 检查 | `tasks/done/M0-06-首次基线构建和版本标签.md` | PASS | M0 |

## M0-04/M0-05 验收结论

- 验收矩阵已覆盖 M0-01、M0-03、M0-04 和 M0-05 的当前状态；
- 已区分通过、阻塞、待处理三种状态；
- M0-02/M0-03 的 Rust 命令级验证已完成，结果已回填到任务记录和本矩阵；
- M0-04 文件完整性和风险登记验收通过；
- M0-05 本地里程碑回归验收通过；
- M0-06 基线构建和 `v0.0.1-baseline` 标签验收通过，项目放行进入 M1；
- M0-A08 GitHub Actions 远程执行仍为 `PENDING`，不影响本地 M0 放行。
| M1-A01 | 窗口与应用生命周期 | M0 | `./scripts/verify.ps1`、运行 `game_runner` | 窗口创建、关闭、缩放、最小化/恢复不崩溃，生命周期日志顺序正确 | 状态机单元测试 | `artifacts/logs/M1-01-window-session.stdout.log`、`artifacts/logs/M1-01-window-interaction.stdout.log` | PASS | M1 |
