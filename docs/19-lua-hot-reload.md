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

## 每实体脚本（docs/54 §4.1）

`Script` 组件声明的脚本（`file=scripts/ai/patrol.lua`）也能热重载，但**不在** `reload_if_changed()`
的范围内——那个入口看的是 `scripts/main.lua` 一个文件。多了一个入口：

- `RuntimeScene::reload_scripts()`：一次返回 `(工程级脚本状态, 被重载的实体脚本文件数)`；
  工程级脚本没变而实体脚本改了时，前者是 `Unchanged`、后者是 `> 0`（两个数字分开报，避免
  "状态未变"被误读成"什么都没重载"）；
- `LuaRuntime::reload_entity_scripts()`：只重载文件指纹变化过的每实体脚本，返回文件数；
- `LuaRuntime::entity_script_files()`：当前已加载的脚本文件（诊断/验收用）。

边界与工程级一致：重载会**清掉该文件 Lua 状态里的一切**（顶层局部变量、模块缓存、闭包），因此
每实体状态请放在实体组件里，或放在脚本内以实体 `id` 为键的表里，并接受重载时重建。已经调用过
`on_spawn(id)` 的实体不会因为重载而重复触发生成。

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
