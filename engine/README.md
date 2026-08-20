# p2 Engine Workspace

这是 Rust + Lua 游戏引擎的 Cargo workspace 基线。

## 当前内容

- `crates/engine_core`：无第三方依赖的引擎核心库占位；
- `crates/game_runner`：最小可执行运行器；
- `rust-toolchain.toml`：固定 stable 工具链并声明 rustfmt/clippy；
- workspace 使用 Cargo resolver 2。

## 本地验证

在 `E:\Codex\p2\engine` 目录执行：

```powershell
cargo metadata --no-deps
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace
cargo build --workspace --release
cargo run -p game_runner
```

预期运行器输出：

```text
p2 game runner 0.1.0
```

## 范围限制

M0-02 不接入 wgpu、Lua、ECS 或其他运行时依赖；这些内容分别在后续里程碑实现。