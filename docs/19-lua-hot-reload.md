# M4-06 Lua 热重载

- 状态：PASS
- 完成日期：2026-08-20

## 实现

`LuaRuntime` 增加：

- `reload_if_changed()`：按文件修改时间、长度和内容哈希检测变更；
- `reload()`：强制重新加载当前脚本；
- `ReloadStatus::{Unchanged, Reloaded}`；
- 新 Lua VM 候选加载和原子替换；
- `on_create`、`on_destroy` 生命周期回调；
- 新脚本编译或初始化失败时保留旧 VM，旧脚本继续可更新。

## 策略边界

- 热重载不迁移旧 VM 中的 Lua 全局变量、闭包和协程；
- Rust 侧共享的引擎状态继续保留；
- 重载提交顺序为：候选脚本加载 → 候选 `on_create` → 旧脚本 `on_destroy` → 替换 VM；
- 候选加载失败时，不调用旧脚本清理，也不改变当前 VM；
- 回调自身失败会报告错误，项目可选择停止脚本系统。

## 验收

```text
cargo clippy --workspace --all-targets --all-features -- -D warnings
通过

cargo test --workspace --all-features
96 passed; 0 failed

cargo run -p game_runner --example lua_hot_reload_demo --release
lua_hot_reload_demo_ok unchanged=true reloaded=true rollback=true update=v2
```
