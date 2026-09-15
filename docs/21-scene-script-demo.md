# M4-08 场景脚本 Demo

- 状态：PASS
- 完成日期：2026-08-20

## Demo 目标

用一个最小可运行场景验证场景格式、Lua API 和热重载能够联通。场景包含玩家、敌人、拾取物、门和触发器；Rust 只负责准备场景文件和启动 Lua，具体游戏规则全部写在 Lua 脚本中。

## Lua 游戏流程

1. `on_create()` 加载 `arena` 场景并创建五类游戏对象；
2. 玩家每次 `update()` 向右移动一个单位；
3. 玩家到达拾取物位置后销毁拾取物并更新目标文本；
4. 玩家碰到敌人时扣血并返回 `failure`；
5. 玩家拿到拾取物并进入触发器后打开门；
6. 玩家到达门的位置后返回 `victory`；
7. `on_destroy()` 清理本脚本创建的实体，支持安全热重载。

## 运行

在 `E:\Codex\RustLua\engine` 目录执行：

```powershell
cargo run -p game_runner --example scene_script_demo --release
```

预期输出：

```text
scene_script_demo_ok entities=5 rules=lua victory=true reload_consistent=true hot_reload=failure
```

## 验收边界

- Rust 没有写入玩家移动、拾取、敌人失败、开门和胜利条件；
- 首次运行与重新加载同一场景脚本的结果均为 `victory`；
- 将 Lua 中敌人位置从安全位置修改到玩家路径后，热重载结果变为 `failure`；
- 当前 Demo 使用 M4-05 的内存场景适配层，尚未接入真实渲染实体和编辑器 Inspector；
- `on_destroy()` 负责脚本实体清理，Lua 全局状态不跨 VM 迁移，符合 M4-06 热重载边界。

## 脚本开发教程

### 1. 初始化对象

在 `on_create()` 中使用 `entity.spawn()` 创建对象，再通过 `transform.set()` 和 `health.set()` 设置初始状态：

```lua
player = entity.spawn("player")
transform.set(player, 0, 0)
health.set(player, 3)
```

### 2. 编写逐帧规则

`update()` 中读取 Transform，调用物理查询，并根据条件返回游戏状态：

```lua
function update()
    transform.move(player, 1, 0)
    local position = transform.get(player)
    if position.x >= 3 then
        return "victory"
    end
    return "running"
end
```

### 3. 编写失败条件

使用 `physics.overlap()` 查询附近实体，并通过 `health.damage()` 修改玩家状态：

```lua
for _, id in ipairs(physics.overlap(position.x, position.y, 0.6)) do
    if id == enemy then
        health.damage(player, 1)
        return "failure"
    end
end
```

### 4. 支持热重载

将初始化放到 `on_create()`，清理放到 `on_destroy()`。修改 Lua 文件后调用 `reload_if_changed()`，候选脚本加载失败时旧 VM 保留，不会把运行中的脚本替换成半成品。