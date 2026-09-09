# M4-08：场景脚本 Demo

- 状态：PASS
- 完成日期：2026-08-20

## 实施内容

- 增加 `scene_script_demo` Release 示例；
- 创建包含玩家、敌人、拾取物、门和触发器的版本化场景；
- 使用 Lua 实现对象初始化、移动、拾取、敌人失败、开门和胜利规则；
- 验证 `on_create/on_destroy/update` 生命周期；
- 验证重新加载场景脚本后结果一致；
- 修改敌人位置后通过热重载将结果从胜利切换为失败；
- 增加场景脚本开发教程。

## 验收结果

- [x] Rust 未实现具体游戏规则；
- [x] Lua 驱动玩家、敌人、拾取物、门和触发器流程；
- [x] 场景重新加载结果一致；
- [x] Lua 文件修改后行为可热更新；
- [x] `cargo fmt --all` 通过；
- [x] `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过；
- [x] `cargo test --workspace --all-features` 通过；
- [x] Release Demo 通过。

## Demo 输出

```text
scene_script_demo_ok entities=5 rules=lua victory=true reload_consistent=true hot_reload=failure
```