# WebGL 与微信小游戏统一 WASM 现状分析

> 口径日期：2026-10-12。本文记录当前仓库事实，不把 Node 模拟、无头浏览器或待接线代码写成真机能力。

## 1. 结论

RustLua 的**统一核心**已经基本完成：WebGL 与微信共用同一个 `game_web` Rust crate，以及同一套 Lua VM/桥、固定步与 RLFI 输入实现；两端是不同的**平台发布变体**，而由同一个 Rust engine host 完整接管模拟、输入消费和渲染的**统一宿主目标**尚未完成。

当前完成度可概括为：

- **统一 Lua 执行层：基本完成**；
- **统一输入 ABI 与固定步时钟：基本完成**；
- **统一 Rust 场景模拟与 GPU 渲染：WebGL 已可用，微信尚未成为生产路径**；
- **统一资源加载、音频、存储与生命周期：仍有平台适配分叉**；
- **统一构建源码：已完成；统一最终 WASM 二进制：未实现，也不应作为硬目标**；
- **离线自动化验证：覆盖较完整**；
- **微信开发者工具、iOS/Android 真机闭环：尚未完成**。

因此，当前架构更准确的表述是：

> WebGL 正在从 JS/Canvas2D 宿主迁往 Rust `RuntimeScene` + wgpu；微信仍以共享 `web-runtime.js` + Canvas2D 为默认生产宿主，只把 Lua VM、固定步策略、输入协议和部分引擎能力统一到了 WASM 中。

### 1.1 三层统一术语

为避免把“核心统一”误读成“最终文件相同”，本文统一使用以下三层术语：

- **统一核心**：同一个 `game_web` Rust crate，以及唯一一份 Lua VM/桥、固定步与 RLFI 输入实现。这里的“统一”指源码与语义事实源统一，不要求两个平台最终发行的 `_bg.wasm` 字节一致。
- **平台发布变体**：同一份原始 WASM 构建按宿主生成 Web ESM（`--target web`）与微信 no-modules（`--target no-modules`）胶水；微信最终 WASM 另做剥 Global、剥 Tag、noextern 降级等平台专属兼容后处理。
- **统一宿主目标**：最终由同一个 Rust engine host 同时负责 Web 与微信的场景模拟、输入消费和渲染；平台层只保留帧源、画布/surface、资源取得、音频、存储与生命周期接线。

按这套术语，当前状态是：**统一核心基本完成，平台发布变体已经形成且属于有意差异，统一宿主目标尚未完成。**

## 2. 已统一的核心层

### 2.1 同一个 `game_web` 源核心与原始 WASM 构建

WebGL 与微信小游戏都构建 `engine/crates/game_web`：

- 目标均为 `wasm32-unknown-unknown`；
- 均固定使用 Rust 1.88.0；
- 均由 Zig 编译 vendored Lua C 源；
- `wasm-bindgen` CLI 版本从 `Cargo.lock` 精确解析；
- WASM 构建统一使用 `opt-level=z`、fat LTO、单 codegen unit，并经 `wasm-opt -Oz` 优化；
- `gpu2d` 是同一 crate 上的可选 feature。

发布脚本只构建一个 `game_web.wasm`，再按平台生成不同胶水与后处理产物，见 `scripts/package.ps1`。

因此以下能力已经只有一套 Rust 实现：

- mlua 与 vendored Lua 5.4；
- Lua 桥接 API；
- `FrameClock` 固定步策略；
- RLFI 输入帧协议；
- Rust 宿主 API；
- 可选的 wgpu/WebGL2 GPU 宿主；
- `RuntimeScene` 场景模拟路径；
- WASM 平台存档槽与音频出口。

### 2.2 两个平台共用 `web-runtime.js`

浏览器与微信均使用 `plugins/webgl/web-runtime.js`：

- WebGL 页面直接加载该文件；
- 微信入口先加载 `wechat-adapter.js`，再加载同一份 `web-runtime.js`；
- 两端均通过 `window.__RUSTLUA_LUA_BRIDGE` 调用 WASM 内的 mlua；
- 两端共享场景兼容层、Canvas2D 绘制、宿主 API 补丁、玩法驱动和诊断字段。

微信启动顺序是：

1. `wechat-adapter.js` 安装平台适配；
2. `web-runtime.js` 导出 `rustluaRunLuaGame`；
3. `adapter.start()` 挂载资源与 WASM；
4. 实例化 WASM；
5. 挂载 `__RUSTLUA_LUA_BRIDGE`；
6. 启动 Lua 主循环。

这消除了 WebGL 与微信各维护一套 Lua 游戏逻辑的主要漂移来源。

### 2.3 Lua 已真实运行在 WASM 中

当前两端均已移除 fengari 执行回退。Lua VM 是编译进 `game_web_bg.wasm` 的 mlua + Lua 5.4，主要桥接口包括：

- `lua_boot`；
- `lua_do_string`；
- `lua_set_number` / `lua_set_numbers`；
- `lua_input_frame` / `input_frame_layout_json`；
- `lua_call_global` / `lua_call_global_void`；
- `lua_register_js_callback`；
- `lua_missing_globals`；
- `lua_execute_main`；
- `lua_fixed_frame` / `lua_fixed_run_steps`；
- `lua_entity_handle`。

WebGL 在 `plugins/webgl/index.html` 中挂载显式白名单；微信在 `plugins/wechat-minigame/wechat-adapter.js` 中挂载对应白名单。两处名单必须同步，否则会表现为某个平台启动后拿到 `undefined` 导出。

这一层是当前“统一 WASM”最完整的部分：两个平台执行同一套 Lua VM、桥协议、输入布局与固定步实现。

## 3. 为什么最终发行物不是同一个 WASM 文件

虽然源内核相同，但两个平台的最终发行物不会逐字节相同，这是宿主能力差异造成的。

### 3.1 WebGL 发行形态

WebGL 使用：

```text
wasm-bindgen --target web
```

生成浏览器 ESM 胶水与 `game_web_bg.wasm`，由 `<script type="module">` 加载。浏览器分支保留标准浏览器支持的 Global、EH 等能力。

### 3.2 微信发行形态

微信使用：

```text
wasm-bindgen --target no-modules
```

微信运行时按 CommonJS/普通脚本解析代码包，浏览器 ESM 顶层 `export` 会直接启动失败。发布阶段还要进行微信专有处理：

- 修补 `document.currentScript` / `location` 依赖；
- 增加 `module.exports = wasm_bindgen`；
- 将上传语法限制在 ES2019；
- 剥离微信 iOS 不接受的 Global 导出；
- 去除 Tag/Exception 胶水依赖并配套处理 WASM；
- 做 noextern/heap type 兼容降级；
- 通过 `WXWebAssembly.instantiate(path, imports)` 按代码包路径异步实例化。

因此当前正确的统一边界是：

> 同一内核源码、同一 Rust crate、同一 Lua/ABI 语义，按平台生成两套胶水和两份经过不同兼容处理的 WASM 发行物。

强求最终二进制逐字节一致会迫使浏览器放弃能力，或让微信无法加载，不应作为架构目标。

## 4. WebGL 当前架构

WebGL 目前保留两套宿主。

### 4.1 Rust GPU 宿主

`package.ps1 -Gpu2d` 会启用 `game_web` 的 `gpu2d` feature，把 wgpu WebGL2 后端与 Rust GPU 宿主编入 WASM。

这条路径使用：

- Rust `RuntimeScene`；
- 引擎场景实例化；
- Rust/Lua 工程脚本模拟；
- wgpu WebGL2 后端；
- 引擎 `Renderer`；
- Rust `FrameClock`；
- Rust 输入快照；
- Rust 网格生成与 GPU present。

当前已经能够：

- 加载工程真场景；
- 执行工程脚本；
- 按固定步推进场景；
- 生成 `SceneMesh`；
- 上传纹理；
- 绘制 Tilemap、Sprite 和文本 UI；
- 输出 draw call、triangle、帧耗时和 WASM 内存遥测。

`engine/crates/game_web/src/rust_scene.rs` 复用了桌面运行器使用的 `RuntimeScene`，因此这条路是向“桌面与 Web 共用模拟核心”收敛，而不是另写一套 Web 游戏逻辑。

### 4.2 旧 JS/Canvas2D 宿主

旧宿主仍保留在 `web-runtime.js` 中，负责：

- JS 侧场景兼容模型；
- Canvas2D 绘制；
- 宿主 API 映射；
- 完整玩法兼容；
- WebGL 与微信的共用生产兼容层。

WebGL 页面当前按能力选择帧所有者：

| 构建和 URL | 实际宿主 |
|---|---|
| `-Gpu2d` 产物，裸 URL | Rust GPU 宿主 |
| 非 `-Gpu2d` 产物，裸 URL | 能力探针发现无 `gpu_host_run`，自动退回旧 JS/Canvas2D |
| `?rustlua-js-host=1` | 强制旧 JS 世界 |
| `?rustlua-gpu=0` | 关闭 GPU 宿主 |
| `?rustlua-js-host=0&rustlua-gpu=0` | 显式无人绘制 |

`verify-webgl-bundle.ps1` 已增加帧所有者断言：若 GPU 宿主未运行、旧 JS 世界也未运行，且 URL 没有显式要求无人绘制，则以 `webgl_bundle_no_frame_owner` 判红。

### 4.3 构建默认与运行默认不同

需要区分两层默认值：

1. `package.ps1` 的 `-Gpu2d` 默认关闭，普通 WebGL 包不含 wgpu；
2. 如果产物已经带 `gpu2d`，裸 URL 默认运行 Rust GPU 宿主。

所以：

```text
package.ps1 -Platform webgl
```

仍生成较小的非 GPU 内核，页面自动使用 JS/Canvas2D；只有：

```text
package.ps1 -Platform webgl -Gpu2d
```

生成的产物才会在裸 URL 下默认使用 Rust GPU 宿主。

这说明 Rust GPU 路径已可用，但尚未成为构建层面的唯一生产形态。

## 5. 微信小游戏当前架构

### 5.1 默认生产宿主仍是 JS/Canvas2D

微信默认入口为：

```text
game.js
  -> wechat-adapter.js
  -> web-runtime.js
  -> WASM Lua bridge
  -> JS/Canvas2D 主循环
```

默认微信路径中：

- Lua VM 在 WASM；
- 固定步权威策略和执行顺序在 WASM；
- 输入批量进入 WASM；
- 场景兼容、帧驱动与绘制仍主要由 JS/Canvas2D 承担。

因此不能把“Lua 在 WASM 中运行”写成“微信已经由 Rust 引擎完整接管”。

### 5.2 微信已有 Rust 引擎宿主骨架

`game_web` 已提供无 DOM 的宿主接口，包括：

- `engine_host_begin`；
- `engine_host_frame_into`；
- `engine_host_frame`；
- `engine_host_state`；
- `engine_host_end`；
- `probe_canvas_host`。

该接口由调用方直接传入 canvas，不依赖浏览器的 `document`、`window` 或 `fetch`。微信适配层已经接线，并提供显式开关：

```javascript
wx.setStorageSync('rustlua_engine_host', 1)
```

启用引擎宿主；另加：

```javascript
wx.setStorageSync('rustlua_engine_host_tick', 1)
```

允许宿主接管 Lua 固定步。

适配层还具备：

- 专用 offscreen WebGL canvas；
- 固定步单一所有者标记；
- 定长 `Float64Array` 帧 ABI；
- 双步冲突检测；
- 宿主失败后把 tick 归还旧运行时；
- 诊断属性与屏幕覆盖层输出。

### 5.3 该宿主尚未接入完整真场景渲染

当前 `host_runtime.rs` 已完成：

- begin/frame/end；
- wgpu surface/device 建立；
- `FrameClock`；
- 可选 Lua 固定步推进；
- 清屏与 present；
- 定长帧状态 ABI。

但它尚未像 WebGL 的 `gpu_host.rs` 那样接入完整的：

- `RuntimeScene`；
- 场景 step；
- `SceneMesh`；
- 工程纹理上传；
- 引擎 `Renderer` 真场景绘制。

因此，微信 Rust GPU 宿主当前更接近“可建立 surface、推进固定步并提交帧的宿主骨架和诊断通道”，还不是替代 `web-runtime.js` 绘制整个游戏的生产宿主。

## 6. 输入与固定步统一程度

### 6.1 输入统一为 RLFI 批量帧

JS 不再逐帧拼 Lua 源码，而是把输入状态编码成 RLFI 定长二进制帧，通过 `lua_input_frame(bytes)` 一次跨 WASM 边界注入。

布局由 WASM 的 `input_frame_layout_json()` 提供，JS 不自行维护偏移。WebGL 与微信因此共用：

- 键位表；
- 按住、按下、释放语义；
- 轴输入；
- 虚拟摇杆；
- 一次性边沿处理。

### 6.2 固定步策略与执行均已进入 WASM

共享运行时每帧调用：

- `lua_fixed_frame(elapsedSeconds)`：由 WASM `FrameClock` 规划步数；
- `lua_fixed_run_steps(steps, delta)`：在 WASM 中执行虚拟输入、写入 `dt`、调用 `update()` 与清理边沿。

这消除了 WebGL 与微信各维护一套累加器、追帧上限和 dropped-step 规则的漂移。

### 6.3 仍存在两类世界所有者

虽然固定步实现只有一份，但当前仍有两种运行形态：

- 旧 JS 世界通过 WASM Lua 桥推进其游戏状态；
- Rust `RuntimeScene` 持有自己的 LuaRuntime 并推进引擎世界。

代码已避免默认同时运行，并有双步冲突检测；但旧世界尚未退休，因此还不能说整个 Web 平台只有唯一模拟实例。

## 7. 资源管线现状

### 7.1 WebGL Rust 路径

WebGL 的 Rust `start()` 会：

1. 拉取 `manifest.txt`；
2. 读取 `project.toml`；
3. 确定启动场景；
4. 读取 `dependencies.json`；
5. 选择依赖闭包；
6. 将资源预载进 WASM 资源表；
7. 交给 VFS 与 `RuntimeScene`。

当前 `game_jhyt` 因 Lua 动态或 opaque 引用仍退回全量预载，已见读数：

```text
selected=1370 manifest=1370 full_fallback=true
```

因此依赖索引已经接通，但当前工程还没有取得实际按需加载收益。

### 7.2 微信路径

微信仍由 `wechat-adapter.js` 单独负责：

- `mainpack/` 首屏资源；
- `pack/` 分包；
- CDN remote 回落；
- 路径方言探测；
- 微信代码包后缀改名；
- 下载校验和；
- `USER_DATA_PATH` 缓存；
- 分包挂载、失败显示与重试。

WASM Lua 桥当前通过同步文本读取器，把 `/project/...` 转换为 `pack/...` 后交给微信适配层路由。

### 7.3 尚未统一的资源边界

两端最终都进入 Rust VFS，但上游加载服务仍不同：

- WebGL：浏览器 fetch + Rust 预载依赖闭包；
- 微信：JS 适配层 + mainpack/subpackage/CDN 路由。

微信 Rust `RuntimeScene` 若要成为生产路径，还需要补齐：

- 二进制资源读取；
- 贴图与字体字节进入 WASM；
- 分包加载与 Rust 场景加载协同；
- 动态资源按需供应。

## 8. 存档、音频与生命周期

### 8.1 存档

两端共享“宿主字符串槽”语义：

- 浏览器映射到 `localStorage`；
- 微信映射到 `wx.getStorageSync` / `wx.setStorageSync`；
- Lua 侧负责序列化；
- WASM 桥只搬字符串。

旧 JS 生产路径已有存档持久化验证；微信 Rust 场景宿主完全接管后的存档与设置行为尚未经过真机闭环。

### 8.2 音频

当前存在两类出口：

- JS 兼容运行时：浏览器 `HTMLAudioElement`、微信 `wx.createInnerAudioContext`；
- Rust GPU 宿主：`audio_sink.rs` 把播放请求交给平台宿主。

WebGL 已有浏览器播放请求证据，微信默认 JS 路径也有 harness 的 `audio_ctx/audio_played` 证据；微信 Rust 宿主路径的实际出声尚未真机验证。

### 8.3 生命周期

- WebGL Rust GPU 宿主已接 `visibilitychange`，隐藏时释放输入并暂停音频，恢复时重置时钟；
- 微信 JS 宿主已接 `wx.onShow` / `wx.onHide`；
- 两个平台尚未把生命周期完全收进同一个 Rust 宿主抽象。

因此这一层属于语义趋同、实现仍分平台。

## 9. 发布链与验证覆盖

### 9.1 WebGL 已有证据

WebGL 当前具备：

- 真实 Chromium/Edge 页面加载；
- WASM 真编译与执行；
- 新 EH 浏览器版本门槛与低版本提示；
- 无头浏览器启动门；
- 黑屏像素门；
- 帧所有者门；
- Rust-only GPU 240 帧门；
- Rust 真场景与输入位移验证；
- JS 宿主 API contract；
- JS 宿主完整玩法剧本；
- 音频、存档和世界状态诊断；
- LAN 静态服务入口。

### 9.2 微信已有离线证据

微信发布与 Node wx 模拟宿主当前覆盖：

- `WXWebAssembly` 路径式异步实例化；
- 分包挂载成功、延迟、失败与重试；
- 路径方言；
- 代码包后缀白名单；
- ES2019 上传语法；
- 240 帧玩法；
- 触摸摇杆；
- 存档持久化；
- 音频播放请求；
- 单一固定步所有者；
- 单一输入注入所有者；
- EH 探针正对照。

2026-10-11 的 `game_jhyt` 分包产物读数为：

- 主包：3,030 KB / 4,096 KB；
- 分包：8,188 KB；
- 总计：11,218 KB / 20,480 KB；
- `wasm=ok:ok:path-instantiate`；
- 240 帧玩法、触摸摇杆、存档与音频请求通过。

### 9.3 仍不能宣称通过的范围

现有离线门不能替代微信开发者工具与真机。当前尚无权威证据确认：

- 真机 `WXWebAssembly` 是否接受当前全部 EH 形态；
- iOS 与 Android 行为是否一致；
- 微信是否向 wgpu 提供可用 WebGL2 canvas；
- Rust 引擎宿主能否在设备上长期 present；
- 实际音频是否出声；
- 前后台切换是否正确；
- 十分钟以上内存趋势；
- 微信工具显示的真实代码包大小；
- 真机分包与 CDN 行为；
- GPU 性能、发热与稳定性。

微信 GPU 宿主在离线 harness 中目前得到：

```text
host: canvas_surface: ... webgl2 not available
```

该读数只证明“开关 -> 适配层 -> WASM 导出 -> wgpu 建 surface”接线执行到了，不证明真机 GPU 可用。

## 10. 主要风险

### 10.1 WebGL Rust GPU 首帧过慢

现有设备验收包记录到 Rust GPU 路径字体预热约耗时 36–46 秒，几乎占据整个首帧时间。该问题足以阻止 Rust GPU 路径成为默认产品路径。

优先排查方向：

- 构建期生成字体 atlas；
- 分帧预热；
- 只预热首屏字符；
- 按字形惰性上传；
- 缓存 atlas；
- 将完整项目字符集预热改为分层预热。

> **2026-10-12 P0-② 归因更正（旧读数→新实测）**：这条 36–46 s 属于 **MSDF CJK 串行预热**，
> 且整段落在首帧的 mesh 趟（同内核对照实测 `first_frame_mesh_ms=27,510.7`）。工程现声明
> FusionPixel 12px 点阵（M72-03），**产品路径首帧实测 58.3 ms / startup≈147 ms**，该风险只在
> "工程未标点阵字体、落回 MSDF 缺省"时存在；对照还发现 MSDF 路在 jhyt 字集上图集顶格缺字
> （`字体图集已满…容量 1558`）。逐阶段读数、复现命令与判据见 `tasks/plan/M73-00` 的 docs65-P0② 一节。

### 10.2 WASM 内存峰值异常

同批记录出现约 300 MB 的 WASM 线性内存峰值，而旧记录约为 17 MB；不同场景又得到相同峰值，尚未归因。

上设备前必须在同一源码、同一内容与同一参数下定位：

- 字体 atlas；
- PNG 解码临时缓冲；
- 资源全量预载；
- 网格临时向量；
- WASM memory growth；
- Rust 分配器未向线性内存归还页面。

> **2026-10-12 P0-② 归因更正（旧读数→新实测）**：同一份遥测内核、同一条 240 帧命令、只换工程
> 字体的对拍结果是——峰值主体在 **MSDF 路**（图集涨到 4096² 顶格 64 MB + grow 双缓冲 + 整表
> 字形光栅化临时驻留），"不同场景峰值逐字节相同"正因为预热与场景无关；预热改为 256 字/批后
> 同一条对照峰值 **243 MB**（旧 300 MB 记录），点阵产品路实测峰值 **46 MB**（资源全量预载本体
> 22.3 MB、贴图批 +2.4 MB，都不是主体）。逐阶段读数见 `tasks/plan/M73-00` 的 docs65-P0② 一节。

### 10.3 微信 GPU 路径尚未证明可落地

当前最大未知不是代码是否接线，而是微信真机 canvas 与 `WXWebAssembly` 环境是否足以支持 wgpu WebGL2 后端。

如果答案是否定的，目标应调整为“统一模拟、平台渲染”：继续统一 Lua、输入、固定步、场景状态和网格生成，但允许微信保留平台专用绘制适配层。

### 10.4 旧 JS 世界仍是生产依赖

当前不能删除 `web-runtime.js` 的旧世界，因为它仍承担：

- 非 `-Gpu2d` WebGL 的默认兜底；
- 微信默认生产渲染；
- 完整玩法剧本；
- 大量宿主 API 兼容；
- 分包、触摸、音频与存档等成熟链路。

因此“退休旧生产热路径”尚未真正完成。

### 10.5 Rust GPU 与旧 JS 路径尚未严格对拍

现有 Rust GPU 验收能证明真场景可绘制、输入可移动、脚本可运行并连续 present，但尚未证明与旧 JS 世界在相同输入下的以下结果完全一致：

- 世界状态；
- 场景切换；
- UI 行为；
- 碰撞；
- 存档；
- 画面层级；
- 完整玩法 M1–M6。

完成确定性对拍之前，不应删除旧路径。

## 11. 能力分级

| 能力 | 当前状态 |
|---|---|
| 同一 Rust WASM crate | 已完成 |
| 同一 mlua/Lua 5.4 VM | 已完成 |
| 同一 Lua 桥接口 | 已完成 |
| 同一 `web-runtime.js` | 已完成 |
| 同一输入二进制 ABI | 已完成 |
| 同一固定步算法与执行顺序 | 基本完成 |
| 同一宿主 API 表面 | 基本完成，显式白名单仍有同步风险 |
| WebGL Rust `RuntimeScene` 模拟 | 已可运行 |
| WebGL Rust GPU 真场景渲染 | 已可运行 |
| WebGL 默认构建启用 GPU | 未完成，`-Gpu2d` 仍是显式开关 |
| 微信 WASM Lua 执行 | 已完成 |
| 微信 Rust GPU surface/帧宿主 | 已接线，默认关闭 |
| 微信 Rust 真场景渲染 | 未完成 |
| 两平台统一资源服务 | 未完成 |
| 两平台统一音频、存储与生命周期 | 部分完成 |
| 旧 JS 模拟/Canvas2D 退休 | 未完成 |
| 微信开发者工具与真机矩阵 | 未完成 |
| 性能达到设备发布水平 | 尚不能确认 |

## 12. 后续优先级

### P0：先验证微信 GPU 路线是否成立

> **2026-10-12 状态**：微信开发者工具已在本机可用（`D:\Program Files (x86)\Tencent\微信web开发者工具`，
> CLI 在位），Android/iOS 真机仍无。**模拟器侧 ①②③⑤ 已答且为绿**（`node scripts/wechat-devtools-probe.js`
> 终行 `verdict=gpu-route-ok-simulator`：探针 ok、宿主 begin 在 2532x1170 离屏画布建立 surface、
> 322/300 连续 present 0 失败、EH `accepted`）；途中修掉三条宿主缺陷（缺 `queueMicrotask`、GPU 入口缺
> panic hook、缺 `WebGL2RenderingContext` 全局——详见 `tasks/plan/M73-00` 的 docs65-P0① 一节）。
> **④⑥ 同日第二批已答（模拟器口径）**——旧说法"只能在真机回答"已不成立：**④ 前后台恢复=recovered**
> （微信 onHide/onShow → 适配层 visibilitychange 派发 → 引擎宿主出帧门控：hidden 期间 present 冻结、
> `hidden_skips=135`、onShow 后 323→457 恢复，run23 终行原文见 `tasks/plan/M73-00` 的 docs65-P0①补充
> 一节；这**不是** OS 级 suspend，回调链外的 OS 级切换仍 device-pending）；**⑥ 内存读数双侧口径已开表**
> （JS 侧 `__RUSTLUA_WASM_MEMORY_BYTES` 与 Rust 侧 begin summary 的 `wasm_mem_bytes`：线性内存
> 2,228,224 → 9,240,576 B，`onMemoryWarning` 告警 0 条；**设备预算**——真机 RSS 与微信告警阈值——仍
> device-pending）。执行进展登记在 `tasks/plan/M73-00`。

应在微信开发者工具、至少一台 Android 和一台 iOS 设备上回答：

1. `probe_canvas_host` 是否成功；
2. `engine_host_begin` 是否能建立 surface；
3. 是否能连续 present 300 帧；
4. 前后台切换后是否能恢复；
5. WASM EH 探针是否为 `accepted`；
6. 内存是否在设备预算内。

如果这一层不成立，应尽早停止扩建微信 wgpu 宿主，改按“统一模拟、平台渲染”推进。

### P0：定位字体首帧与 300 MB 内存峰值 —— **2026-10-12 已完成归因**

应补充阶段遥测，至少覆盖：

- 资源预载前后；
- 字体文件读入后；
- 字体解析后；
- atlas 分配后；
- 每批字形预热后；
- PNG 解码前后；
- 首帧网格生成前后。

上述七段已全部进门（`[resources] preload_ms/mem_*`、`[render] font atlas ready/prewarm chunk`、
`[gpu] first_frame_* / textures_decoded / wasm_mem_peak_bytes`），归因与对拍读数见
`tasks/plan/M73-00` 的 docs65-P0② 一节；本节余账：MSDF 兜底路本身的提速与图集容量。

### P1：将微信 `host_runtime` 接入真正的 `RuntimeScene`

> **2026-10-12 前置通道已验（模拟器口径，方案 A 最小接管）**：`__RUSTLUA_ENGINE_HOST_VISIBLE`
> 开启时 2d 改道离屏、宿主直接绑**主画布**——`engine_host_begin` 成功（`source=main-canvas`，
> present 44/44 失败 0），每帧 present 后同任务的引擎侧 `readPixels` 五点回读全部等于 clear color
> （`[69,75,85,255]`）⇒ **此刻可见屏幕的像素所有者是引擎宿主**；老 JS 世界经对照组证明仍在画
> （改道 2d 结构读数 luma=98.7 与接管前同形）。本片的画面只有 clear-color——下面这条共享核心
> 就是"接管的不只是一块纯色屏"的下一片。真机主画布给不给 webgl2 未验（模拟器不外推）；
> 读数与注入复现步骤见 `tasks/plan/M73-00` 的"可见画面接管"一节与 CHANGELOG 同批条目。

> **2026-10-13 共享核心第一片落地：微信模拟器第一次在离屏画出真场景（浏览器侧同片行为等价）**。
> 下面"抽成共享核心"那句的改法已做：`gpu_host` 的平台无关渲染链抽进 `game_web/src/engine_core.rs`
> （新文件），`gpu_host`（浏览器）与 `host_runtime`（微信引擎宿主）两个调用者共用；宿主新增
> `scene` 档 options，未给 scene 时 clear 分支与 P0-① 形态逐字节同形。实测（模拟器口径）：
> `verdict=scene-real presented=61 failed=0 sim=rust(boot ok)`，`textures=14/15` 与浏览器基线同形
> （缺项 `__font_atlas__` 白兜底是分包配额登记的既有决定），`step_errors=0`，boot_ms=1435；
> 浏览器侧 240 帧门绿。途中钉死并修掉三条宿主侧缺陷：帧泵不再依赖 JS 世界 rAF（scene 档自泵）、
> vfs 一次性 provider 槽竞态（scene 档 begin 前预装 reader provider）、reader 文本通道损坏二进制
> （改 ArrayBuffer 字节通道）。**上一块里"本片的画面只有 clear-color"这句到此收窄**：离屏 scene 档
> 已画真场景；可见接管 + scene 的组合（判据从清屏像素换成场景像素）是下一片，真机仍未验。
> 逐条读数与复现见 `tasks/plan/M73-00` 的 docs65-P1 第一片一节与 CHANGELOG 同批条目。

> **2026-10-13 组合片（P1 第二片）首验：此刻可见的主画布画的是真场景（模拟器；产品码零改动）**。
> 两开关同开（`__RUSTLUA_ENGINE_HOST_VISIBLE` + `__RUSTLUA_ENGINE_HOST_SCENE`）即达——三路接线
> （target=主画布 / options.scene / scene 自泵）在前面两批里已正交铺好。run8 终行
> `verdict=combo-scene-visible presented=68 failed=0 source=main-canvas(visible-takeover) mode=scene
> sim=rust(boot ok)`；像素判据从"五点=清屏色 [69,75,85,255]"换成含场景内容
> （中心=[10,13,20,255]、左上=[11,16,29,255]；四角=[0,0,0,0]，清除形态差异登记未归因）。
> 上一块"真机仍未验"照旧成立；逐条读数见 `tasks/plan/M73-00` 的 P1 第二片一节与 CHANGELOG 同批条目。
>
> **2026-10-13 run9 收口四角落**：上面组合块"四角=[0,0,0,0]，清除形态差异登记未归因"已归因并修掉——
> scene 档**两平台同形地从没显式清屏**（`RustScene::frame_views()` 只声明 overlay 视图，其恒带
> `ClearPolicy::None` 的"叠在已有内容上"语义 + 没有主视图垫底 ⇒ 渲染计划里连清屏通道都不存在），
> 修法是给 scene 档唯一视图声明 `ClearPolicy::Color(DEFAULT_3D_CLEAR)`（`game_web/src/rust_scene.rs`
> 产品码一行）。run9（模拟器）：`verdict=combo-scene-visible presented=69 failed=0`，四角变
> `[56,75,97,255]`（=清屏色线性→sRGB，不透明），左上仍是场景内容；两侧发布产物已带新核重建、
> 两平台门同批绿。逐条读数与复现见 `tasks/plan/M73-00` 的任务#17 一节与 CHANGELOG 同批条目。
>
> **2026-10-13 回退路径判别（任务#18，模拟器，产品码零改动）**：接管失败时分两种形状——
> **A 形**（options 期具名拒绝，run2 现场：`tick="lua" 与 scene 互斥` 把 begin 挡在拿上下文之前）
> 主画布没被锁（`getContext('webgl2')` 现场给得出；被 2d 绑过按 WebGL 规范必为 null），runtime 回退
> 物理可行，但适配层 2d 通道焊死在改道件上（`same_as_bound=true`）——**缺接线不是缺能力**；
> **B 形**（begin 成功、拿过 webgl2 之后崩/停）主画布永远回不出 2d，**回退=关开关冷启动**。
> 生产推进的回退预算按 B 形口径设计。读数与量具自纠见 `tasks/plan/M73-00` 的任务#18 一节。

> **2026-10-13 事后像素回读口径收口（任务#19，模拟器，产品行为零变化）**：接管形态下 JS **事后**
> `toDataURL` 是竞态——同一形态两次冷启动，一次给整幅 `RGBA(0,0,0,0)`（合成清空后的缓冲）、一次给到
> 活画面，而拿到时内容是真的（与引擎侧 readPixels 四点逐值一致，含 #13 那张 54,721 B 原图重测＝满幅
> `[69,75,85,255]` clear 色，那条读数成立不更正）。所以"画面归属"的证据只认 present 后同任务的引擎侧
> 回读，事后 JS 截图不进判据。run8/#17 在账的 `no-toDataURL` 本笔判明是探针读错把手（`G.canvas` 在接管
> 形态下不存在，正确把手是 `__RustLuaWechat.canvas`），不是宿主缺陷；"present 循环在场"这个候选同时被
> 否证（两组都有 present 循环而行为不同）。本笔另钉出一条产品缺陷并登记不修：scene 形态的活画面只占
> 表面 31.1%（内容包围盒 1380×720 / 表面 2532×1170）⇒ **未按表面缩放**，可见画面是"一角场景 + 其余
> 清屏色"。判别变量（`__RUSTLUA_ENGINE_HOST_PIXEL_SAMPLE` 透传开关，默认不偏离）与逐条读数见
> `tasks/plan/M73-00` 的任务#19 一节。

> **2026-10-13 场景按表面缩放（任务#20，模拟器验收，上面 #19 那条新缺陷已还）**：归属先判清——浏览器
> 同形态**免疫**（`fit_now` 把后备缓冲缩到等比适配盒 ⇒ 声明盒与表面同比例 ⇒ 新算子在 1 px 容差内 snap
> 成整目标、渲染器不发 `set_viewport`，而旧 `clamp_viewport` 给的是同一个矩形 ⇒ 命令流逐字节不变，
> run14 浏览器门与改前同读数），所以这条是**微信接管形态特有**：那里的表面必须是整屏设备像素，留边
> 只能发生在表面内部。根因两段：`host_runtime` 从没把逻辑视口收到设计分辨率（1 世界单位 = 1 设备像素），
> 渲染器又只能表达"夹到左上角"。修法是一份 `engine::host::fit::contain_rect`（等比 contain + 居中 +
> "1 px 内贴满即整目标"），渲染器 / 浏览器 fit / Canvas2D 层 / 微信宿主几何读数四处共用，矩形同时进
> `engine_host_state().fit`。run2 实测：`fit=declared=1280x720 rect=2080x1170+226+0`，内容包围盒
> `226,0..2305,1169` **恰好等于该矩形**，非底色像素覆盖率 `31.1% → 77.92%`，四角 `[56,75,97,255]`
> （清屏色留边条，此处是期望值）、中心 `[58,72,104,255]` 是场景内容。逐条读数与复现见
> `tasks/plan/M73-00` 的任务#20 一节与 CHANGELOG 同批条目。

> **2026-10-13 接管+scene 的输入归属（任务#21，模拟器）**：#20 在账的那句"接管形态今天拿不到输入"被 run16 否证——
> `web-runtime.js` 是输入注入的唯一所有者，接管+scene 每帧照样把 `RLFI` 送进 wasm 的 `LATEST_FRAME`，宿主 scene
> 档消费它；本笔给这条通道补了现场读数（`engine_host_state().scene_input` = 本帧消费的 `frame/axis/cursor/held/press/mouse`）。
> 合成事件从**适配层触摸/键盘桥之上**注入（包住 `wx.onTouch*/onKey*` 抓 handler 再调用），实测键盘按下→`axis=1.00,held=1`、
> 跨帧仍 `held=1`、松开→归零，触摸按下→`mouse=1`、cursor 落**设计坐标**且与 #20 那套 contain 矩形反算**逐值互逆**
> （328,447 与 889,201 两点全等）。分工钉清：`input_frames` 数的是宿主帧入口被调用次数，`input_applied=0` 只说明
> "没随调用附带批量"，**不等于没有输入**。逐条读数与量具自纠（采样必须等指纹前进，定长 sleep 会采到陈旧帧）见
> `tasks/plan/M73-00` 的任务#21 一节。〔**#22 改判这最后两句**〕当时"接管形态还没有只读世界查询"是
> #22 的题面，现已接上：`engine_host_state().scene.world` = `名字@x,y moved=dx,dy`，本笔那句"没证世界位移"
> 由 run5/6/7 的"对照零 / 按住给方向正确位移 / 松开归零"三段判掉。刻意**没有**搬浏览器那条
> `rustlua-world-expr=<任意 Lua>`（无护栏，§WP6-06 未完成① 记着"微信侧若复用需补"），用的是引擎自己的
> `entity.find`/`transform.get`，构造上只读。逐条读数见下面的 #22 块与 `tasks/plan/M73-00` 的任务#22 一节。

> **2026-10-13 接管+scene 的世界位移读数（任务#22，模拟器）**：#21 只证到"输入进到宿主消费的那一帧"，本笔把最后
> 那半步补上——`engine_host_state().scene.world` 给出 `名字@x,y moved=dx,dy`，读数**在读态那一刻**由引擎自己的
> `entity.find`/`transform.get` 现刷（不挂每帧：WP6-05 记过热路径多一次 Lua 求值会挤掉同机剧本的帧预算；而接管形态
> 没有状态属性可写、`engine_host_state` 是诊断口，产品帧循环与微信门都不调它）。判据三段：**对照**（什么都不按、跨
> 10 宿主帧）`528,112 → 528,112` ⇒ 零；**处理**（按住 `keyd`=move_right）⇒ `@532,112 moved=4,0`，指纹全程
> `held=1 axis=1,0`；**松开** 6 帧 ⇒ 停；**反方向**（按住 `keya`）⇒ `@524,112 moved=-4,0`（Δ=`-8,0`）。三次冷启动
> （run5/6/7）逐值相同，终行 `walk_cycle verdict=world-moves-on-input`；`moved=` 与"相对第一次可读坐标的差"逐值相等
> 是自证项（绝对坐标不说明任何事，位移才说明）。两个不是缺陷的读数：接管形态走 `SimRoute::EngineStartup`，这一档
> **不解析场景文档**，故本笔同时补了活世界登记名表兜底（新 `RuntimeScene::named_entity_names()`；第一版误用面向编辑器
> 镜像、**排除声明实体**的 `runtime_spawned_entities()`，靠 `no-player(named=… samples=…)` 的样本名定位到）；而 `main.scene`
> 出生点 `528,112` 四面被顶住（向右被边界挡在 532：同一次按下从 528 给 `+4`、从 524 给 `+8`、在 532 给 `0`；竖直按住
> 52 帧 `Δy` 恒 0）⇒ **"按住是否连续走"在这张图上没有判据可用**，登记为需要开阔地取点的后续。**（两条各定各的案，别并成一条：
> 竖直那个 `Δy` 恒 0 按 #25+#26 定案**不是几何**而是 web 键盘纵向抵消（合成轴按"屏幕向下为正"写，与 `padaxis:1+` 反号 ⇒
> `action_axis` 归零）——本条自己的读数就是第一手凭据：按住 `keys` 给 `axis=0,1`，而按旧号那正是"上推"，与键位声明的方向
> 逐值相反；横向 `+4` 才归 #24 的门柱。"四面被顶住"这条按 #24 作废：出生点正下方实测有 340 px 余量，卡住的只有左右各 4 px。）**
> 微信接管的**正向**确认仍待开发者工具恢复（今天工具起不来，判因改由浏览器通道隔离 + 已有接管读数定案，见下面 #26 那块。）**浏览器侧同一条兜底也判了：
> `-SimRoute script -InputScript 'space:10-15,axis0+:100-160'`（§WP6 那台历史命令逐字重跑）**EXIT=0** 且
> `world=JianghuPlayer@532,112 moved=4,0`——这条档以前只会印 `no-player`。**顺带一条要还的账**：同一台命令当年记录的是
> `528→≈668`（140 世界单位），今天两条平台、两条路由都只走到 532，规模差 35 倍；候选是 #20 的视口改动或"轴窗口落在转场
> 前/后"，都没测，而那条位移门只断言 `moved≠0`，看不见规模。**这条账 #24 已结：两条候选都否证，真因是 `35155c81` 的脚底球
> `offset=0,-16` 让出生球上半截伸进南门门洞那一档（横向只剩 ±4 px），详见下面那块。**另：同一把 space 挪到晚窗（`space:30-33`/`space:30-120`）
> 会停在标题（`spawned=18 last=bgm_title`、`input_edges=4` 照旧）⇒ 当时判成"早窗/晚窗的差别没归因"，而它决定这根线进门时窗口怎么取点。**这条已由 #23 判因并修掉：与窗口早晚无关**，见下面那块。

> **2026-10-14 标题确认"只吃早窗"判因（任务#23）**：`RustScene::step` 原来**无条件** `finish_frame()`，
> 而 `apply_frame` 的 `just_pressed` 只在"松开→按下"的跳变上产生（按住集合是权威，仍按住的键不补发）
> ⇒ 边沿落在**不跑固定步的 rAF 帧**里就被当场抹掉，那一下确认永久丢。浏览器里 0 步帧是常态：实测
> `frames=240` 对 `sim_steps=152`（`sim_dropped=7 sim_clamped=1`）⇒ ≥88 帧一步不跑，而 boot 段每帧都在
> 追帧、必然跑步——这就是 `space:10-15` 行而 `space:30-33`/`space:30-120` 不行的全部原因（90 帧长窗里
> 下沿仍只有一次，所以"窗口太短"本来就不是候选）。同一个守卫本仓已有两处：桌面 `game_app.rs:203-208`、
> 编辑器 `game_view.rs:384-395`（M22/W4 补漏），`run_script_steps` 的注释（`schedule.rs:356-359`）还把这条
> 写成契约；web 宿主是这条契约的第三个消费者，漏的是第三个。改法 `if steps > 0 { finish_frame() }` 加回归
> 测试 `a_press_edge_survives_a_frame_without_fixed_steps`；`rust_scene.rs` 两平台共用 ⇒ 微信宿主同一条
> `FrameClock.plan`（`host_runtime.rs:541`）一并覆盖。实测（clean bundle，撤净插桩后重打）：三条窗口
> `space:30-33,axis0+:100-160` / `space:30-120,axis0+:140-200` / `space:10-15,axis0+:100-160` **全部 EXIT=0**
> 且同一读数 `world=JianghuPlayer@532,112 moved=4,0`；文档档（`-SimScene manor -SimRoute scene-script
> axis0+:30-150`）逐字同基线；带插桩那一跑给出正分支凭据 `c=29:c1U0D0u0d0X0Y0S1ABtnStart`（第 29 个 Lua
> 步看到确认、`activated=BtnStart`）。
>
> **进门口径随之一变**：微信生产门接这根线时**不需要为窗口取点做特殊设计**（原话"它决定这根线进门时
> 窗口怎么取点"作废），但需要把"按下那一次落在跑步的帧"当成**门自己的读数**——今天只能由
> `frames − sim_steps` 反算，门里没有"0 步帧数"这一项。
>
> **不在本笔射程的一条**：#22 的位移规模账 `528→≈668` vs 今天 `528→532` 一字未动，本笔三条门仍是
> `moved=4,0`——守卫修的是"确认这一帧有没有被看见"，不是"单位时间走多少世界单位"，两件事不同因，
> 别并成一条。**（这条分立由 #24 证实：规模那条与守卫无关，见下面那块。）**
>
> **2026-10-14 位移规模 `140 → 4` 判因（任务#24，判因片，产品码零改动）**：`manor` 出生格是 `(16,3)`
> （`gen_manor_scene.py` 的 `SPAWN`，与 `data/maps.lua` 的 `main.spawn` 同源），而 rows 1-2 的实心列是 13/15/17、
> 空列是 14/16/18 ⇒ **(16,1)(16,2) 就是 `fe7f0510` 挖的南门门洞**，两侧 col 15/17 是门柱。脚底球球心
> `(528,96)`、半径 12 ⇒ 盒 `x∈[516,540] y∈[84,108]`，上半截伸进 row 2 那一档，向右极限 = `17*32 − 12 = 532.0`
> （向左对称 524）。旧口径球心 y=112 ⇒ 整截落在全空的 row 3，横走无挡，才有当年那个 140。**盘面上把它换掉的只有一行**：
> `git diff 35155c81^ 35155c81 -- scenes/manor.scene` = 1/1，`Collider` 加上 `;offset=0,-16`，`solid` 位串一字未动。
> 判别靠七拍同图对照（日志 `.temp/24-gate-*.log`）：文档档（根本不过转场）同样 `532.0` ⇒ 转场假说否证；轴窗拉到 140 帧
> 仍 `532.2` ⇒ 窗口长度假说否证；**先向南挪 21 px 再走同一条 80 帧窗 ⇒ `640.0`（Δ112）**，先向北挪半格 ⇒ 更卡
> （`532.2`），向南挪后走满 120 帧 ⇒ `696.0`（Δ168）⇒ 走廊开着、位移能力完好。**（这三个方向名按 #26 作废：`axis1+` 给的是
> `y=112→133`，而本图口径是"y 朝上"（`gen_manor_scene.py:19`）、harness 的运行时读数也报按住 `w` 到 `@528,205` 且姿态
> `idle_n` ⇒ `axis1+` 是向**北**，"更卡"那一挪是向**南**；数值与机理一字不改，只换方向名。）**速度也没回归：`MOVE_SPEED=140` 单位/秒、
> 固定步 1/60 ⇒ 每步 2.33 单位，脱困后实测每步 ≈2.0，与当年同量级。**剩下的 35% 差不是代码而是量具口径**：剧本窗口按
> **宿主帧**索引，模拟按 1/60 固定步走，今天这台 `frames=240 / sim_steps=166 / fps=88.2` ⇒ 同一个"60 帧窗"只值 0.69 秒。
> 于是那条在册命令的 `-ExpectWorld 'expr=.*manor:6[0-9]{2}'` **今天直接 `[RED]`**（当年绿是踩在 60 Hz 上）——台账已把它拆成
> "历史原样（注明为什么必红）+ 一条跑绿的形态"，绿的那条只断"进了 manor 且朝所按方向动了"；默认位移门不动。
> **静态外推（`.temp/24-probe-all-spawns.py`，在 `manor` 上与浏览器实测 4/4 逐值相同）**：七张图里 `manor`/`village`/
> `mountain_path` 三张的出生点都卡在门洞门柱上，四向余量 4/4 px ⇒ 按左右只动 4 px 就停；`village`/`mountain_path`
> 那两条今天只有静态读数。修复属场景线（出生点向南挪一格，或门洞挖宽一格；都必须走生成器），本片不动产品码。**（"向南"这个方向名
> 按 #26 作废：脱困那一挪实测是 `y` 增（`112→133`），本图口径 y 朝上 ⇒ 应为向**北**挪一格；"门洞挖宽一格"那条不受影响。）**
>
> **2026-10-14 web 键盘纵向抵消判因并翻号（任务#25+#26，1 路径产品码 `plugins/webgl/web-runtime.js` +24/−1）**：题面是
> #22 那条"竖直按住 52 宿主帧 `Δy` 恒 0"——#24 把它连同横向一起记成了门柱几何。#25 本要在微信接管里复跑位移探针定案，
> **开发者工具今天起不来**（用户裁：先不查，继续推进）⇒ 改由"已有读数 + 源码算式 + 浏览器通道隔离"三件定案。机制是一行符号：
> 工程的绑定 `move_up=key:keyw,padaxis:1+` / `move_down=key:keys,padaxis:1-`（`examples/game_jhyt/project.toml` 的 `map`）
> 把同一次按键喂进两条通道，而 `web-runtime.js` 的键盘→轴合成按"屏幕向下为正"写 ⇒ 两通道**反号**，
> `input.action_axis("move_down","move_up")` 把它们相加归零；横向 `d` 与 `padaxis:0+` 同号所以照常能动 ⇒ "只有纵向坏"
> 就是这个不对称暴露出来的。引擎那侧的约定另有两处钉着：`input.axis("vertical")` 是 `KeyW − KeyS`
> （`engine/crates/rustlua-input/src/input.rs:1535-1547`）、editor 面板的断言"上推 = padaxis:1+（上为正）"
> （`engine/crates/editor/src/panels/input_bindings.rs:636-640`）。判别靠浏览器五拍（`.temp/25-browser-*.log`，全
> `-BundleRoot examples/game_jhyt/builds/webgl -Gpu2d -NoSceneVerify -SimRoute script` + `-WorldExpr` 只读
> `require('player.player').position()`）：只键 `w:60-90` ⇒ `moved=0,+47`；只轴 `axis1+:60-90` ⇒ `moved=0,+47`
> （同向同量 ⇒ 这两条通道本应同号）；只键 `s:60-75` ⇒ `moved=0,−23`；把两通道摆成反号
> （`w:60-90,axis1-:60-90` 与 `s:60-90,axis1+:60-90`）⇒ **`moved=0,0` 且那条位移门直接 `[RED]`
> （`webgl_bundle_rust_input_failed`）**——抵消的姿态默认门本来就抓得住。射程按实测划（**#25 起初报成"两个 web 平台都坏"
> 是过宽，本笔更正**）：翻号只在"JS 快照真的送进引擎"的形态生效 = **微信接管**（适配层置
> `__RUSTLUA_ENGINE_HOST_OWNS_TICK`，`wechat-adapter.js:1753` ⇒ `applyInputFrame` 每帧跑；#21/#22 的
> `scene_input.axis=1,0`/`axis=0,1` 就是这份轴的读数）。**浏览器引擎宿主路由惰性等值**：那条路由不跑 `web-runtime.js` 的
> `frame()`（修前修后两拍的 DOM 快照里 `data-rustlua-timing`/`-input`/`-export` 出现次数都是 0），输入只来自 Rust 侧 DOM
> 采集，按住真 `w` 前后同形（`act=1.00 pad1=0.00`；`moved=0,0→+124→+142` vs 修后 `0,0→+126→+138`，两条 EXIT=0）。
> **老 JS 世界（微信生产默认形态，`engine_host=off`）逐值不变**：它的 Lua `input` 垫片不看 padaxis，修前修后三跑的
> `--drive-move` 读数完全相同（12 帧窗 `d=0,47`、40 帧窗 `d=0,93`；摇杆用例同为 `d=0,93`）。
> **顺带更正 #24 的方位措辞**（数字与机理一字不改，只换方向名）：`examples/game_jhyt/tools/gen_manor_scene.py:19` 写
> "单格 32 世界单位，y 朝上"，harness 的运行时读数也对上（按住 `w` ⇒ `@528,112 → 528,205` 且姿态 `idle_n`）⇒ #24 的
> "先向**南**挪 21 px（`axis1+`）脱困"与"先向**北**挪半格（`axis1-`）更卡"方向名**写反了**；连带那两条修法里的选项 ①
> "出生点向**南**挪一格"按此口径应为向**北**（把 `SPAWN` 的 y 抬高，实测 +21 px 即脱困）。这条与代码那处同号同源，
> 都是"屏幕向下为正"的直觉——同一张账上它只误导了措辞，进了产品码就是本片这处缺陷。
> 量具两条账：`-KeyProbe` 原来只数 `dom_events`/`input_edges`**不数位移**（本笔在 `scripts/webgl-cdp-wait.js` 的打印里
> 补上 `world=`，print-only +11/−1）；`-GpuFrames` 是**帧预算**（`gpu_host.rs:57-65`）而不是采样参数，带着它跑 `-KeyProbe`
> 读到的是冻住的计数器（现场 `frames 240 -> 240 -> 240`），量活的按键必须不带它。
> **登记未测（旧说法）→ #27 新实测（本条闭合）**：原句"微信接管的**正向**确认（纵向按键现在能否连续走）等开发者工具恢复"
> 已在 2026-10-14 开发者工具恢复后跑通：接管宿主起来了（`host_status=running`、`mode=scene`、`size=2532x1170`、
> `fit=declared=1280x720`），纵向按键**按声明方向连续走**，两跑逐向复现（`s` ⇒ `Δy=-44`/`axis=0,-1`、`w` ⇒ `+44→+89`/
> `(0,+1)`、对照段 `Δ=0`）。同一条读数把 #25 那三次"工具起不来"归了因：**包没带真实 appid**（`touristappid` 的包在
> 开发者工具里编译不过 ⇒ 宿主从未 boot，症状是 `host_status=off` 而不是输入缺陷）。仍**未测**：真机（不外推）；
> 原句里"方向键纵向反向是两拍实测拼接的推导，反向本身没单独跑过"这条也一并闭合——接管形态直接给了方向键读数
> （`ArrowDown ⇒ Δy=-46/-49`、`ArrowUp ⇒ +45/+51`，与 `w`/`s` 同号），拼接推导不再需要。
>
> **#27 接管形态的正向确认（2026-10-14，零产品码，只登记）**：判据是**同一次跑里逐方向对拍**，不是"能走"这一个布尔。
> 探针注入在 `launcher/game.js` 的 `require('./wechat-adapter.js')` **之前**置
> `__RUSTLUA_ENGINE_HOST/VISIBLE/SCENE`（开关置晚了就掉进老 JS 世界，那条路的 `input` 垫片不看 padaxis，测不到本片改的东西），
> 读数走 `__RustLuaWechat.engineHostState()` 的 `scene.world` + `scene_input`，采样一律等宿主帧前进（不用定长 sleep，
> 转场期主线程 ≈2 fps 会读到陈旧值）。两跑逐向一致（run2 / run3，单位=世界 px）：对照段（不按键，跨 ≥10 宿主帧）`Δ=0,0`
> 两跑同形；`s` `-44,-8 / -51`；`w` `+44,+89 / +49,+89`；`d` `+47,+51 / +49`；`ArrowDown` `-46 / -49`；`ArrowUp` `+45 / +51`
> ——修前 #22 那条"按住 `keys` 共 52 宿主帧 `Δy` 恒 0"从此有了反向读数，且轴指纹也对上（`s ⇒ axis=0,-1`、`w ⇒ 0,+1`，
> 即"声明的 `move_down` 与送到的向下同号"）。日志 `.temp/27-wechat-takeover-run{2,3}.log`、`.temp/v27-report.json`。
> **一条顺带更正（防 #24 被误读）**：同一张 `manor` 出生点这次按 `d` 走了 `+47/+49`，而 #24 量的是 `moved=4,0`——
> 两者不矛盾：#24 那拍**先按纵向**，纵向被当时那处抵消吃掉 ⇒ 主角没脱开南门门洞（左右各 4 px 余量）；本次纵向先走通了，
> 横向自然走满。"4 px"是**卡在门洞里**的读数，不是横向能力。
> **一条不算缺陷但必须登记的残余**：松手后世界还有 2–5 单位惯性（`w-stop +2`、`d-stop +5`、`arrow-down-stop −5`，
> 而 `held=0 axis=0,0` 已归零）。**成因本笔没判**（最像是固定步与帧边沿的既有差，但没证据），也没进门——默认位移门断的是
> 同号与 `moved≠0`，这个量级不触发它。
> **包的身份这一档记进命令口径**：`package.ps1 -Platform wechat-minigame -Gpu2d` 不带 `-WechatAppId` ⇒ 写进
> `project.config.json` 的是 `touristappid`，模拟器能跑 harness、开发者工具却**编译不过**（#25 那三次 `host_status=off` 全是
> 这个形状）。appid 的唯一出处是 `examples/game_jhyt/project.toml:31`，**不写进命令行**（凭据形态会被分类器拦，也避免进 shell 历史）。
> 手工改产物里的 `project.config.json` **不等于**带 `-WechatAppId` 重打包：run1 就是只改了 JSON 没重打，`no-report(timeout)`、
> 且没有任何开发者工具进程起来。**手改不算数，重打才算**。
> **体积一条（本笔的第二次红，归美术线）**：带 appid 首次重打包对总包硬门 `20,480 KB` 报红（实测 `21,331 KB`，
> `assetc files=2487` vs 上一档 `2108`），成因只读地定在**打包把未跟踪的备份目录一并扫进资源**：`assets/art/` 下 11 个
> `*_backup_20261014/` = 1,169 个文件 / 2,528 KB，而 14:48 那次成功的 `pack/` 里已经压着 740 个备份文件 / 737 KB
> （ ⇒ 门红不是本笔带来的，是本包第一次撞上它）。用户把备份迁出 `assets/`（`examples/game_jhyt/art_backup/`）后重打包绿：
> `assetc files=1368`、`Package size: 20,237.6 KB, 1384 files`、`main_of_limit=61.0% / subpackage_of_limit=86.6% /
> total_of_limit=98.7%` ⇒ **总包余量只剩 ≈243 KB**，美术下一次重存要按这个数读，不能按 4,096/20,480 的的名义上限读。
> **门禁（最终字节那一版）**：`verify-wechat-minigame.ps1 -ExpectGpuKernel` VERIFY_EXIT=0（注入撤净后才跑，
> `wechat_minigame_package_ok`、生产口径 `engine_host=off`、harness 240 帧、五条负面用例全绿）；
> 本笔零 Rust、零 `web-runtime.js` 改动 ⇒ `cargo fmt/clippy/test` 与 `wasm32` check、webgl 那条打包门没重跑（`5ab416b9`
> 的基线仍在位）；`powershell -NoProfile -File scripts/check-docs.ps1` **DOCS_EXIT=0**、六项读数逐字同基线
> （`docs=59 game_docs=46 h1=107 links=414 ps1_bom_ok=33 ps1_parse_ok=33`）——#26 那条挂在别包 `docs/58` 里的
> `xxx.md` 反例引用红今天已由他们自己清掉（那份文件 `grep -c -F 'xxx.md'` = 0 且与 HEAD 一致）。**一条现场踩到的坑**：
> `CHANGELOG.md` 在这个门的扫描面里（`scripts/check-docs.ps1:135`），所以条目里**不能**把"方括号紧跟圆括号"那个链接形态
> 原样拼出来当引文——那会被判成一条真链接（与 M72-10 归档那条同一条坑）。
> **一次性工具留 `.temp/`**（`p1-v27-{probe,collector}.js`、`27-package-wechat-appid{,-run2}.log`、
> `27-wechat-takeover-run{1,2,3}.log`、`27-verify-wechat-appid.log`、`v27-report.json`），不进 verify 入口；
> `builds/wechat-minigame/` 是重打包产物，**不是提交路径**。
>





前提是微信真机 GPU 探针成功。应把 `gpu_host.rs` 中的平台无关部分抽成共享核心，而不是继续复制：

```text
平台帧源/canvas
        |
共享 EngineHost
  |- FrameClock
  |- RLFI InputSnapshot
  |- RuntimeScene
  |- SceneMesh
  |- Renderer
  `- audio/storage hooks
        |
平台 surface/present
```

应共享 scene boot、step、mesh、texture upload、render 和 diagnostics，只保留平台帧源与 canvas 获取差异。

**已落地两片**：

- **P1 第一片 `engine_core`**：`surface/device → Renderer → RustScene boot → step → mesh → 贴图 → 取景 → render → present → poll` 只剩一份；两个外壳各自调用它。
- **P1 第二片 `engine_host`**：帧的**驱动**也收成一份。此前两个外壳各持一个 `FrameClock`、各写一遍
  `plan → 取输入 → core.frame(plan.steps, plan.delta_seconds, input)`（两张表逐格同形，一处改了另一处不一定改）；
  现在这一段只有 `engine_host::EngineHost`（= 帧时钟 + `EngineCore` + 唯一的 `frame(raw_seconds, input)`），
  浏览器 `gpu_host` 与微信 `host_runtime` 的 scene 档都走它。时钟配置收成 `host_config::HostConfig`
  （默认 `(60, 8, 0.25)`，并用桌面单测锁住"喂 Lua 的步长必须 f64 现算"这条契约）。

- **前置（同批落地）**：`engine_host_begin` 现在接受**只给 `sim` 而不给 `scene`**——`sim=script`
  （`SimRoute::EngineStartup`）由**引擎自己**解析启动场景，它的 `scene_path` 只进读数，所以把微信默认翻到
  Rust 宿主时不必在适配层再抄一份"启动场景是哪个"；另外三条路线仍必须有 `scene`（仍然具名拒绝，不静默退成 clear 档）。
  这处 Rust 改动之后复跑浏览器那两条门：`webgl_bundle_ok wasm=5291.1KB`（`-Gpu2d -GpuFrames 240`，exit 0）与
  `webgl_bundle_rust_input_ok script=axis0+:30-150 world=JianghuPlayer@797,112 moved=173,0`（scene + 确定性输入，exit 0）
  ⇒ 统一宿主在 Web 这条路上"输入真的改变了世界"，改动没有回归。

- **微信默认已翻转（步骤 2）**：适配层用一个**原子模式**取代原来那三个开关，**默认开**——微信默认形态就是
  Rust 宿主的可见 scene 宿主（主画布归宿主、2d 改道到离屏、`sim=script` 由引擎自己解析启动场景、`tick=none`），
  帧源改成 `wx.requestAnimationFrame` 并进 `engine_host.frame_source` 读数；只认**显式**关才退回旧形态。
  门禁随之改成断言新默认（`engine_host_source` 含 `visible-takeover` + 结论永远具名），并新增一条显式关用例守逃生门。
  读数：`verify-wechat-minigame.ps1 -ExpectGpuKernel` **exit 0**。

- **构建默认也已翻转（同批）**：`game_web` 的 `gpu2d` 特性转**默认开**（`Cargo.toml` 的 `default = ["gpu2d"]`）。
  此前两个平台的**运行期**默认都已翻到 Rust 宿主（浏览器 `index.html` 的"Rust 侧默认开、`?rustlua-gpu=0` 关"；
  微信的原子模式默认开），但 `gpu2d` 默认关 + `package.ps1` 只在 `-Gpu2d` 时编它 ⇒ **默认产物里没有宿主**：
  浏览器退回旧 JS 世界，微信直接 `engine_host=unavailable reason=kernel-without-gpu2d`（按已裁定的"默认开、不回退"，那是黑屏）。
  改后整条链走默认档的读数：浏览器 `legacy JS world skipped (engine host owns the frame)`、
  `frame owner ok: engine GPU host (data-rustlua-gpu)`、`webgl_bundle_ok wasm=5292.2KB`（exit 0）；
  微信 `wasm_host_ok exports=146 globals=0`、`verify-wechat-minigame.ps1 -ExpectGpuKernel` **exit 0**。
  ⇒ **旧 JS 世界现在在默认产物上不跑**（仍可由 `?rustlua-jshost=1` 或 `--no-default-features` 产物进入）。

**本项仍未统一的两条 + 一条观测阻塞**（正是"统一宿主目标尚未完成"的具体内容）：

1. 微信输入仍由旧 `web-runtime.js` 经 `lua_input_frame` 注入 `LATEST_FRAME`，宿主只负责取用
   ⇒ **旧运行时停摆则设备输入不再更新**（浏览器那条路已有 Rust 侧 DOM 生产者，微信没有）。这一步**刻意留到删掉旧世界那一笔**：
   注入搬给宿主会让 `input frames=N bits=M` 这个守卫读数（恒 0 = 两边对"这一帧该变哪些位"理解不一致）退化成恒 0 的假信号；
2. `clear` 档（显式关掉原子模式、又没给 `scene` 时的诊断形态）还自己持钟，且仍有一份独立于
   `engine_core::build_gpu_stack` 的建栈代码；它与探针那条路合计三份建栈代码待去重；

**观测阻塞（它排在"把输入搬给宿主"前面）**：`engine_host_state()` 的 `scene` 块是 scene 档唯一的输入证据来源，
而门与 harness **都不调用它**（`wechat-minigame-harness.js` 里 `engine_host_state` 出现 0 次），且**即使调用也读不到**——
离线假宿主给不出 WebGL2 ⇒ `engine_host_begin` 必然失败 ⇒ `HOST` 是 `None`。于是"输入到底还有没有进到宿主"这条**最危险**的读数，
恰恰是**最没有门可依**的一条。两条解法见 `tasks/plan/M73-01-删除旧JS世界-删除面清单.md` §9：
**A（推荐）让浏览器也走 `host_runtime` 这台壳**（浏览器有 WebGL2，那几个导出同是 wasm 导出），在一个有 GL 的环境里给 N2/N3 造出可断言现场，
顺带把两个外壳收成一个；B 给离线 harness 一个真 GL 上下文。**在解开之前，"把输入所有权搬给宿主"只能靠真机验证**，与"每步都要有可复现读数"冲突。

**已补齐的一条（原第 3 条）**：`engine_host_resize` 已落地并**进了产物**（`exports=146`，实例化日志里可见该导出），
`wx.onWindowResize` 会在宿主 `running` 且接管形态下通知它，并留 `resize_calls` 读数；但它**还没有专属门**（harness 无以模拟窗口变化），记为 §5 的 N8。

### P1：完成 Rust GPU 与旧 JS 世界的确定性对拍

至少覆盖：

- 标题到新游戏；
- 移动；
- 背包；
- 任务；
- manor 与 village 往返；
- 存档重开；
- 相同输入脚本后的玩家坐标、地图与实体数；
- 关键画面截图或结构读数。

对拍通过后，才适合把 WebGL `-Gpu2d` 提升为构建默认，并逐步退休 JS 模拟。

### P2：统一资源读取接口

建议将 WebGL fetch 预载、微信 mainpack/subpackage/CDN、JS 字符串 reader 与 Rust VFS provider 收敛为统一平台资源服务，至少提供：

```text
read_text(path)
read_bytes(path)
ensure_available(path)
prefetch(paths)
```

尤其不能让微信 Rust 场景路径长期依赖只返回字符串的读取器。

## 13. 最终判断

按本文三层术语，最终判断是：

- **统一核心：基本完成。** WebGL 与微信已经使用同一个 `game_web` Rust crate，以及同一套 Lua VM/桥、固定步与 RLFI 输入实现。
- **平台发布变体：已经形成，且差异是有意的。** Web 使用 ESM 胶水；微信使用 no-modules 胶水和平台专属 WASM 后处理，最终 `_bg.wasm` 不同文件、不要求哈希一致。
- **统一宿主目标：浏览器已达成默认，微信差输入这一条。** 浏览器**默认产物**现在由 Rust 宿主出帧（`legacy JS world skipped`、
  `frame owner ok: engine GPU host`），微信默认也已是原子模式且内核在位，但它的**输入仍由旧 JS 运行时注入**（见 §12 的剩余第 1 条），
  而这一条的可断言性被观测阻塞压住（离线假宿主不给 GL ⇒ 宿主到不了 `running`）。

因此，RustLua 的 WebGL/微信统一 WASM 已越过概念验证阶段，但若“统一”指两个平台均由同一个 `RuntimeScene`、同一个 Renderer、同一个资源服务和同一个帧宿主运行，当前仍处于中后期迁移阶段，而不是完成态。决定后续路线的五个关键缺口是：

1. 微信 Rust GPU 宿主尚未真机证实；
2. 微信默认仍依赖 JS/Canvas2D 世界；
3. 资源、音频、存储和生命周期仍有平台分叉；
4. WebGL Rust GPU 存在严重首帧与内存风险；
5. 旧 JS 路径尚未完成行为与画面对拍，不能退休。

## 14. 关联资料

- [WebGL 浏览器平台验收](32-webgl-浏览器平台验收.md)
- [平台插件适配](51-平台插件适配.md)
- [微信小游戏分包发布](52-微信小游戏分包发布.md)
- `plugins/webgl/README.md`
- `plugins/wechat-minigame/README.md`
- `tasks/plan/M73-00-Web运行时性能重构.md`
- `tasks/plan/M73-01-删除旧JS世界-删除面清单.md`（删旧世界的删除面分区账、依赖顺序 B1–B8、待写门 N1–N8、体积收益与静默风险 R1–R9）
