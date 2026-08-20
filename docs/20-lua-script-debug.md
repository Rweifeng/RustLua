# M4-07 Lua 脚本调试

- 状态：PASS
- 完成日期：2026-08-20

## 实现内容

- `ScriptDiagnostic` 结构化脚本诊断记录；
- 记录脚本路径、错误阶段、错误消息、行号、调用栈文本和出现次数；
- `call_update_debug()` 记录运行时错误；
- `ScriptDebugger` 对相同路径、阶段和消息进行去重；
- 支持按文件路径或错误文本过滤；
- 支持清空诊断记录；
- Lua 语法错误继续保留文件路径和行号信息；
- Debug API 只通过显式诊断方法暴露，发布运行路径不会自动开启调试控制台。

## 使用示例

```rust
runtime.execute_project_script(path)?;
if let Err(error) = runtime.call_update_debug() {
    eprintln!("{error}");
}

runtime.set_debug_filter(Some("player.lua".to_string()));
for diagnostic in runtime.diagnostics() {
    println!("{}:{} {}", diagnostic.path.display(), diagnostic.line.unwrap_or(0), diagnostic.message);
}
```

## 边界

- 当前调用栈使用 mlua 返回的错误文本保存；
- 错误去重按 `path + phase + message` 判定；
- `occurrences` 统计重复错误出现次数，避免每帧无限新增记录；
- 未来编辑器 Console 可直接消费 `diagnostics()`。

## 验收

```text
cargo clippy --workspace --all-targets --all-features -- -D warnings
通过

cargo test --workspace --all-features
98 passed; 0 failed

cargo run -p game_runner --example lua_script_debug_demo --release
lua_script_debug_demo_ok diagnostics=1 deduplicated=true filtered=true stack=true
```
