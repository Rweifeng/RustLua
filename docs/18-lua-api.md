# M4-05 Lua 游戏 API

- 状态：PASS
- 完成日期：2026-08-20

## 交付内容

- `engine/crates/engine_core/src/script.rs`
- `engine/crates/game_runner/examples/lua_api_demo.rs`
- `docs/18-lua-api.md`

## 第一版 API

| 模块 | API | 说明 |
|---|---|---|
| `log` | `info/warn/error` | 通过 tracing 输出脚本日志 |
| `time` | `delta` | 获取当前帧 Delta Time |
| `input` | `action/axis/pressed/released` | 键盘与鼠标输入快照 |
| `entity` | `find/spawn/is_alive/destroy` | 创建、查找、检查和销毁实体 |
| `transform` | `get/set/move` | 读取和修改位置、旋转、缩放 |
| `health` | `get/set/damage` | Demo 用生命值与受伤 API |
| `physics` | `overlap/raycast` | 最小圆形重叠和射线查询 |
| `scene` | `load/reload` | 场景状态切换接口 |
| `asset` | `load/release` | 资源句柄生命周期接口 |
| `ui` | `text/show/hide/get` | 最小文本 UI 状态接口 |
| `save` | `read/write` | 键值存档接口 |
| `app` | `quit` | 请求退出应用 |

## 安全与错误策略

- Entity ID 使用 `index/generation` 编码，销毁后旧 ID 不会指向新实体；
- 非法、负数、代际为零或已销毁的 Entity ID 返回 Lua 运行时错误；
- Transform、生命值、物理查询拒绝非法浮点数和负数参数；
- API 错误转换为 Lua 错误，不通过 panic 终止宿主；
- API 运行在主线程共享状态中，后续可替换为 ECS Command 队列以支持安全跨系统修改；
- 当前 `scene/asset/ui/save` 是可测试的内存状态适配层，真实持久化和渲染绑定留给后续 M4/M5 任务。

## 验收

```text
cargo clippy -p engine_core --all-targets --all-features -- -D warnings
通过

cargo test -p engine_core --all-features
94 passed; 0 failed

cargo run -p game_runner --example lua_api_demo --release
lua_api_demo_ok update=lua-api-ok quit=false
```
