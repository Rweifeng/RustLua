# M0-02 Cargo workspace 基线

## 状态

REVIEW（等待 Rust 工具链可用后执行命令验收）

## 目标

建立最小、可扩展、无外部依赖的 Rust workspace，为后续引擎模块和运行器提供稳定基线。

## 实施内容

- 创建 workspace 根清单；
- 创建 `engine_core` 库 crate；
- 创建 `game_runner` 二进制 crate；
- 配置 Rust stable、rustfmt 和 clippy；
- 编写 workspace README；
- 添加最小库测试和运行器输出。

## 不做什么

- 不接入 wgpu、Lua、ECS、物理、音频或编辑器；
- 不引入外部依赖；
- 不创建质量门脚本；
- 不修改两个总计划文件和 M0-01 文档。

## 修改范围

```text
engine/
tasks/done/
```

## 验收标准

- [x] `engine/Cargo.toml` 定义 workspace 和两个成员；
- [x] workspace 使用 resolver 2；
- [x] `rust-toolchain.toml` 声明 stable、rustfmt、clippy；
- [x] `engine_core` 可以提供最小库 API；
- [x] `game_runner` 可以调用 `engine_core` 并输出版本；
- [x] 提供标准验证命令和预期输出；
- [ ] `cargo metadata --no-deps` 通过；
- [ ] `cargo fmt --all -- --check` 通过；
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过；
- [ ] `cargo test --workspace --all-features` 通过；
- [ ] Debug/Release 构建通过。

## 当前阻塞

当前环境找不到 `cargo`、`rustc` 和 `rustup`，因此命令级验收尚未执行。代码和清单已准备完成；待安装或配置 Rust stable 后继续执行验证，不将未执行的命令标记为通过。

## 验收命令

```powershell
Set-Location E:\Codex\p2\engine
cargo metadata --no-deps
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace
cargo build --workspace --release
cargo run -p game_runner
```

## 下一步

1. 安装 Rust stable 工具链；
2. 执行上面的命令并保存日志；
3. 通过后将状态改为 `ACCEPTED`；
4. 进入 M0-03 质量门和自动化检查。