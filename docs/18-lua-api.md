# M4-05 Lua 游戏 API

- 状态：PASS
- 完成日期：2026-08-20

## 交付内容

- `engine/crates/engine_core/src/script.rs`
- `engine/crates/game_runner/examples/lua_api_demo.rs`
- `docs/18-lua-api.md`

## M8-06 新增 3D API

| 模块 | API | 说明 |
|---|---|---|
| `transform3d` | `attach/get/set/move` | 挂接/查询/设置/移动 3D 变换；`set(id,x,y,z[,rx,ry,rz,rw[,sx,sy,sz]])` |
| `mesh` | `set/get/clear` | 设置/读取/清除 `MeshRenderer` 网格路径 |
| `camera` | `set_active/get_active/get` | 切换/查询活动相机；`get(id)` 返回 `x,y,z,tx,ty,tz,fov,near,far` |
| `light` | `set_active/get/set` | 设置/读取方向光；`set(id,dx,dy,dz,r,g,b,intensity)` |

3D 组件在 `instantiate_scene` 时从场景字符串解析为 World 组件；`camera.set_active` 与 `light.set_active` 由宿主渲染器每帧读取，未选择时回退到首个 `Camera3D` / `Light` 实体。

## M8-07 新增骨骼动画 API

| 模块 | API | 说明 |
|---|---|---|
| `animation3d` | `get/set/list/paused/set_paused/time` | 查询/切换当前剪辑；`get(id)` 返回当前剪辑名，`set(id,name)` 切换并重播，`list(id)` 列出全部剪辑；宿主每帧推进时间 |

`Animation3D` 场景组件格式：`mesh=<path>;clip=<name>;looped=true|false`，剪辑来自同一 glTF 资产的 animations。

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
| `ui` | `text/text_at/show/hide/get` | 最小文本 UI 状态接口；`text_at(id,x,y,text[,scale])` 可选缩放 |
| `audio` | `play/volume/music/stop_music` | 音效播放与音量；`music(name)` 循环 BGM，`stop_music()` 停止 |
| `save` | `read/write/load/commit` | 键值存档接口（load 从磁盘载入，commit 原子写入） |
| `app` | `quit` | 请求退出应用 |

## 安全与错误策略

- Entity ID 使用 `index/generation` 编码，销毁后旧 ID 不会指向新实体；
- 非法、负数、代际为零或已销毁的 Entity ID 返回 Lua 运行时错误；
- Transform、生命值、物理查询拒绝非法浮点数和负数参数；
- API 错误转换为 Lua 错误，不通过 panic 终止宿主；
- API 运行在主线程共享状态中，后续可替换为 ECS Command 队列以支持安全跨系统修改；
- 当前 `scene/asset/ui/save` 是可测试的内存状态适配层，真实持久化和渲染绑定留给后续 M4/M5 任务。

## 脚本组织

- 项目脚本（`scripts/main.lua`）支持 `require` 加载同目录模块（`package.path` 已包含脚本目录，如 `require("player")` → `scripts/player.lua`）；
- 样板游戏将玩法按职责拆分：`game.lua` 编排、`state.lua` 共享状态、`player/enemies/coins/save_manager/hud` 各司其职，`main.lua` 仅作入口转发生命周期回调；
- 热重载会重建 Lua VM，模块随脚本一并重新加载（闭包/协程/全局状态不跨重载迁移，见 [19-lua-hot-reload.md](19-lua-hot-reload.md)）。

## 验收

```text
cargo clippy -p engine_core --all-targets --all-features -- -D warnings
通过

cargo test -p engine_core --all-features
94 passed; 0 failed

cargo run -p game_runner --example lua_api_demo --release
lua_api_demo_ok update=lua-api-ok quit=false
```
