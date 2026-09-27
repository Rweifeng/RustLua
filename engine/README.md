# RustLua Engine Workspace

这是 RustLua（原 p2）Rust + Lua 游戏引擎的 Cargo workspace 基线。

## 当前内容

- `crates/engine`：引擎 crate（`core/` 纯逻辑模块 + `host/` GPU 渲染模块）；
- `crates/game`：最小可执行运行器；
- `rust-toolchain.toml`：固定 stable 工具链并声明 rustfmt/clippy；
- `engine/.cargo/config.toml`：workspace 构建配置；
- workspace 使用 Cargo resolver 2。

## 质量门

在项目根目录执行：

```powershell
./scripts/verify.ps1
```

该脚本依次执行：

1. Cargo metadata；
2. rustfmt 检查；
3. Clippy 严格检查；
4. workspace 测试；
5. Debug 构建。

Release 质量门：

```powershell
./scripts/verify.ps1 -Release
```

仅运行测试：

```powershell
./scripts/test.ps1
```

运行最小 Demo：

```powershell
./scripts/run-demo.ps1
./scripts/run-demo.ps1 -Release
```

## 本地验证

也可以在 `E:\Codex\RustLua\engine` 目录执行：

```powershell
cargo metadata --no-deps --format-version 1
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace
cargo build --workspace --release
cargo run -p game
```

预期运行器输出：

```text
RustLua Engine 0.1.0
```

## CI

`.github/workflows/quality.yml` 在 Windows runner 上执行 metadata、format、clippy、test、Debug/Release build 与两个工程的 `--check`，再跑 `scripts/verify.ps1` 的 `-MvpOnly`／`-Benchmarks`／`-Web` 三步，以及 `scripts/test-compare-baseline.ps1`、`scripts/test-release-atomic.ps1`。

## 范围限制

M0-03 只建立质量门和自动化检查，不接入 wgpu、Lua、ECS 或其他运行时依赖。