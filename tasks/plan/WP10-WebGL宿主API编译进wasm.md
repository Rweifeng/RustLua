# WP10 — WebGL 宿主 API 编译进 wasm（docs/32 §4 方案①）

> 状态：规划中（Phase 1 = 批次 A 实施中）｜计划日期：2026-09-27

## 1. 目标与背景

docs/32 §4 backlog 4-24：**17 个引擎宿主全局**在 WebGL / 微信小游戏两条路径上都缺失——

`app, asset, calendar, clock, container, data, day, event, grid, interact, inv, item, light, map, shop, sprite, tag`

当前它们在浏览器端由 `__p2_missing_host_global` 自报家门代理占位（`[lua missing-host-api] 全局.字段`），
游戏"能启动、能进 Lua、能画画面"，但**一碰玩法就静默缺 API**——即"能看不能玩"。
用户决策（2026-09-27）：按 docs/32 §4 方案①，**把 17 个宿主 API 编译进 wasm**（能力与桌面一致）。

## 2. 关键工程事实（决定架构）

- **mlua 编不进 `wasm32-unknown-unknown`**：引擎桌面 Lua 桥是 `mlua 0.11`（vendored Lua 5.4），
  `mlua-sys 0.10.0` 的 `build/main_inner.rs` 用 `cc` 编译 Lua C 源码；`wasm32-unknown-unknown`
  没有 C 编译器 → 构建失败（已读缓存源码核实）。`wasm32-wasi` / `emscripten` 可编 C，
  但与 wgpu/winit 的目标集不兼容。→ **"整个引擎（script.rs 的 mlua 桥）编译进 wasm"在工具链层面不可行**。
- 因此方案①落地为：**17 个宿主 API 以纯 Rust 实现进 `game_web` wasm 核心（wasm-bindgen 导出），
  fengari Lua 运行时经 `web-runtime.js` 桥接调用**。API 的逻辑与状态**编译进 wasm 二进制**，
  Lua 侧只做全局注册与参数编解码——满足"编译进 wasm、能力与桌面一致"，又不依赖 C 工具链。
- 现状盘点：
  - 桌面 36 全局：`engine/crates/engine/src/core/script.rs`（11490 行），Rust 注册。
  - web 已实现 19 个（`log/time/input/scene/entity/transform/transform3d/tilemap/health/physics/
    physics3d/animation/animation3d/audio/ui/save/particles/camera/mesh`）：`plugins/webgl/web-runtime.js`。
  - `game_web` wasm 核心（39.6 KB）：目前只做启动自检（画布/清单/场景读取），**不含游戏状态**；
    不依赖 `engine` crate（`Cargo.toml` 只有 wasm-bindgen 系），天然可加纯 Rust 模块。
  - 两条 web 路径共用同一份 `web-runtime.js`（docs/32 §3），改动两边同时生效。

## 3. 架构

- **Rust 侧**：新增 `engine/crates/game_web/src/host_api.rs`。每批 API 一个模块、状态自持
  （如时钟语义与 `engine::core::gameplay::GameClock` 对齐：`MINUTES_PER_DAY=1440`、
  `DEFAULT_WAKE_MINUTE=360`、28 天/季 × 4 季/年）。wasm-bindgen 导出 `HostApi` 结构（方法级导出），
  由 `lib.rs` 挂到 cdylib。
- **桥接**：`index.html` 在 `await init()` 后实例化导出挂到 `window.__P2_HOST_API`；
  `web-runtime.js` 在 `p2RunLuaGame` 里 `installRustHostApiBridge(L)`：把对应全局注册成**真实表**
  （方法调用 wasm 导出）；**导出缺失时回退旧代理**（旧包不炸，向后兼容）。
- **回调类 API**（`event.on/emit`、`day.sleep` 的 `on_day_end`/`on_day_start` 钩子）：回调是
  fengari Lua 函数，**由 JS 桥持有**（JS 调 fengari 钩子）；wasm 侧只提供状态与判定，
  钩子时序由桥编排——避免 fengari 函数对象进 Rust 再回跨的编解码黑洞。
- **微信侧**：与 webgl 共用 `web-runtime.js`；wasm 导出经适配层既有 `wasm-bridge.js`
  实例化暴露（微信批另算）。

## 4. 批次划分（按数据依赖）

| 批次 | 全局 | 状态/数据依赖 | 回调依赖 | 说明 |
|---|---|---|---|---|
| **A** | `app, clock, calendar, day` | 引擎时钟状态（wasm 自持） | `day.sleep` 钩子走 JS 桥 | 纯状态、无 IO、无游戏数据 → **Phase 1** |
| **B** | `event, data, asset, tag, sprite, light, grid, interact` | 事件表/持久化存储/实体组件/资源注册表 | `event.on/emit` 走 JS 桥；`interact` 队列 | 需存储后端（localStorage/IndexedDB）与实体状态桥 |
| **C** | `item, inv, container, shop, map` | 游戏数据（items/crops/shops.lua、.scene） | 少 | 数据由 fengari 加载后**单向喂入** wasm 数据层（避免双写漂移） |

（`save` 已在 web-runtime 实现，仅做一致性核对；其余 19 个已实现全局不动。）

## 5. Phase 1（本工作包第一步）= 批次 A

- **Rust**：`host_api.rs` ——
  - `HostClock`：day/minute/rate/paused + `now/set/set_rate/pause/add/sleep_to_morning` +
    季节/年推导（`day_of_season`/`season`/`year`，与 gameplay.rs 同公式）；
  - `app`：name/platform/version/root 等只读字段；
  - `clock`/`calendar`/`day` 表语义：clock.now 返回 day/minute/hour/minute_of_day/rate/paused；
    calendar.now 返回 absolute_day/day/season/year/minute；day.now 返回 day/minute。
- **JS**：`web-runtime.js` 新增 `installRustHostApiBridge(L)`（注册 app/clock/calendar/day，
  导出缺失回退代理）；`index.html` 传导出。
- **验收**：harness / `missingHostGlobals()` 清单不再含 app/clock/calendar/day；
  `clock.now()` 数值正确；`calendar.now()` 第 1 天 = season 1 / year 1；`clock.add(60)` 跨天进位正确。

## 6. 整包验收标准（17 全局全部落地时）

- docs/32 §4 的 17 全局全部真实注册；`bundle-summary-*.json` 的 `missingHostApi` 为空。
- 桌面对拍：关键数值（时钟换算、日历、库存增减、商店买卖、地图格/交互）与桌面 harness 结论一致。
- 两条路径（webgl 无头浏览器 + 微信 harness）全绿；`verify.ps1 -Mvp` 不回归；docs/34 同步。

## 7. 风险与对策

- **跨桥编解码开销**：每调用 Lua→JS→wasm 三层跳；对策：Lua 侧批量/缓存（如 now() 一次取整表）。
- **回调桥语义对齐**：event/day 钩子的触发时序、快照（emit 期间订阅变更不炸）在 JS 桥复刻。
- **数据双写一致性**（批次 C）：设计成"Lua 加载数据 → 喂入 wasm"单向流，wasm 侧只读/只改运行态。
- **并行会话**正在改 `web-runtime.js`/`wechat-adapter.js`（面板关闭按钮功能）——本工作包桥接代码
  做**增量**（新增函数 + 注册点插入），不覆盖其 hunk；提交时只提交本工作包文件。
