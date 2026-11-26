# Web/微信 wasm 打包工具链的唯一事实源。
#
# 为什么单独抽一个文件：wasm 侧必须用 **rustc 1.88.0**（LLVM18）构建——只有它产出**纯 legacy EH**
# 的模块；≥1.95 的 rustc 会把 vendored Lua C 的 setjmp 编成 `try_table`（new EH）、把 Rust
# `catch_unwind` 编成 legacy `try`，同一个模块里混用两种 EH，V8/Edge 直接拒绝：
# "module uses a mix of legacy and new exception handling instructions"（2026-11-26 在 Edge 158 实测，
# 见 docs/84）。打包脚本（30-build.ps1）与一致性门（check-wasm-release-recipe.ps1）都必须读这里，
# 不许各写一份——两处写死不同版本正是 M86-01 的根因之一。

function Get-RustLuaWasmToolchain {
    return "+1.88.0"
}
