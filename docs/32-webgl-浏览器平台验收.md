# WebGL / 浏览器平台验收（M20-04）

本文记录 `game_web`（WASM 入口）与 `game_jhyt` 在浏览器上的构建方式、验收流程、
已实测证据与已知限制。

## 1. 组成

| 部件 | 位置 | 说明 |
|---|---|---|
| WASM 入口 | `engine/crates/game_web/src/lib.rs` | `start(canvas_id, title)`：设置画布与标题，按固定路径 `fetch` 资源 |
| 资源包 | `<release>/pack/` | 由 `assetc package` 产出；含 `manifest.txt`、`project.toml`、`scenes/`、`scripts/`、`assets/`、`chars.txt` |
| 发布页 | `scripts/package.ps1 -Platform webgl` | 生成 `index.html` + `web-runtime.js` + `pkg/`（wasm-bindgen 输出） |
| 浏览器契约检查 | `examples/game_jhyt/web/{index.html,web-check.js}` | 在真实浏览器中验证 WebGL 集成依赖的平台契约 |
| 验收脚本 | `scripts/verify-webgl.ps1` | 组装 web 包 → 本地静态服务器 → 无头浏览器断言 → 记录日志 |

WASM 入口读取的资源路径（契约，改动需同步 `web-check.js`）：

- `pack/manifest.txt`（首行 `assetc_manifest=1<TAB>assetc-package-1`，其后每行 `相对路径<TAB>字节数<TAB>指纹`）
- `pack/project.toml`（`startup_scene` 决定首个场景）
- `pack/<startup_scene>`

## 2. 运行方式

```powershell
# 完整 WebGL 发布。外部工具链由脚本自己解析/按需安装，**不需要手工装**：
#   * zig   —— 编译 vendored Lua 的 C 源（`zig cc --target=wasm32-freestanding`）；
#              解析顺序 $env:ZIG → engine\.tools\zig → PATH，全落空时下载固定 0.16.0
#              到 gitignore 的 engine\.tools\zig\（scripts/zig-toolchain.ps1）
#   * wasm32 目标 —— 装到真正编 wasm 的那个 toolchain（1.88.0）上，见下方"工具链口径"
#   * wasm-bindgen CLI —— 版本取自 Cargo.lock 的 wasm-bindgen 条目，不匹配就
#              cargo install 精确版本到 engine\.tools\wasm-bindgen（scripts/wasm-bindgen.ps1）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/package.ps1 -ProjectRoot examples/game_jhyt -Platform webgl

# 浏览器集成回归（日志与 DOM 快照写到 artifacts/logs/webgl/）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify-webgl.ps1

# 产物层启动验收：真实发布包在无头浏览器里能否启动到 Lua（M31/P3 新增）
# 省略 -BundleRoot 即自动发现本机唯一的合格 webgl 产物（不再写死某个游戏工程）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify-webgl-bundle.ps1

# 显式指定产物（本机有多个合格产物，或想验证某个特定包）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify-webgl-bundle.ps1 `
    -BundleRoot examples/game_jhyt/builds/webgl

# 一条命令试玩：定位产物 → 产物层验收 → 起本地服务（Ctrl-C 退出）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/test-bundle.ps1

# 本地静态服务（发布目录必须经 HTTP 提供，不能 file:// 打开：浏览器会拦 WASM/资源请求）
node scripts/serve-static.js examples/game_jhyt/builds/webgl/<name> 8000
```

## 2.1 工具链口径（三条都踩过坑，写在明面上）

* **wasm 必须用 rustc 1.88（LLVM 18）编**：1.97 对 vendored Lua C 的 setjmp 生成 new EH
  （`try_table`）、对 Rust 侧 `catch_unwind` 生成 legacy `try`(0x06)，同一模块里新旧混用，
  V8 直接拒绝编译（详见 `package.ps1` 里 WP10 那段）。所以 wasm32 目标要装到**这个 toolchain**
  上：`rustup target add wasm32-unknown-unknown --toolchain 1.88.0`。只跑不带 `--toolchain`
  的 `rustup target add` 只给默认 stable 装上，1.88.0 依旧报 `can't find crate for 'core'`
  （实测 2026-09-30）。
* **wasm-bindgen CLI 的版本事实源是 `engine/Cargo.lock` 里的 `wasm-bindgen`**，不是文档里的
  某个魔数：CLI 与 wasm 元数据 schema 必须精确一致，偏旧直接拒绝生成胶水。以前这里写死过
  `0.2.127`，而 crate 已是 `0.2.129`，照着文档装就把 webgl 与微信两条发布一起挂住。
  现在由 `scripts/wasm-bindgen.ps1` 读 lock 解析，必要时 `cargo install --root engine\.tools\wasm-bindgen`
  （不污染全局 `~\.cargo\bin`）。
* **zig 只服务 web 平台**（`zig cc` 编 vendored Lua 的 C 源），pc 构建完全不碰。以前写死在
  `engine/.cargo/config.toml` 的 `[env] ZIG` 里是某台机器的绝对路径，换机器就 panic
  `run zig cc: NotFound`，还被 `cargo ... | Out-Null` 吞成"WASM build failed"。现在由
  `scripts/zig-toolchain.ps1` 解析（`$env:ZIG` → `engine\.tools\zig` → PATH → 按需下载固定
  0.16.0），下载目录已 gitignore：仓库里只留脚本，不留二进制。

产物目录由 `scripts/bundle-discovery.ps1` 定位，规则与 `package.ps1` 的产物校验清单逐项一致；
`test-bundle.ps1` 是**与平台无关**的试玩入口（`-Platform webgl|wechat-minigame`），它自身不做任何
产物断言，只把产物交给对应平台的验收脚本，因此两边的通过标准不会分叉。
产物目录里还带一个 `test-webgl.bat`（平台插件模板 `plugins/webgl/test-webgl.bat` 的副本，
发布时由 `package.ps1` 复制）：双击即验证并起服务，靠向上查找仓库根定位脚本，不依赖目录深度。

`verify-webgl.ps1` 的行为：

1. 检测 `wasm32-unknown-unknown` 是否安装；
2. 组装 web 包（`assetc package` + 契约检查页）；
3. 用 `scripts/serve-static.js`（零依赖 Node 静态服务器）在本地端口提供该目录；
4. 用 **无头 Edge/Chrome**（`--headless=new --dump-dom`，带 60s 超时兜底）执行契约检查并断言 DOM 结果；
5. 把浏览器版本、构建产物、DOM 快照、结果 JSON 与日志写入 `artifacts/logs/webgl/`。

`verify-webgl-bundle.ps1`（产物层，M31/P3）的行为：

1. 断言发布产物完整（`index.html`/`web-runtime.js`/`pkg/game_web.js`/`pkg/game_web_bg.wasm`/
   `pack/manifest.txt`/`fonts/*.ttf`），并校验 wasm 魔数（`\0asm`，不是占位文件）；
2. 起静态服务器 + 无头浏览器打开 `index.html`，读页面上的机器可读启动标记
   `data-p2-boot`（`pending`/`booted`/`running`/`ok`/`failed`）与 `data-p2-boot-detail`；
3. `boot=ok|booted|running` → 退出 0 并写 `bundle-summary-*.json`；`failed` → 退出 1 并把
   **缺失的引擎宿主 API 全局**列出来；无 node/浏览器 → 退出 2（环境受限，不伪造通过）；
   省略 `-BundleRoot` 又没能唯一定位产物 → 退出 3（打印候选与被排除项及原因）。
4. 顺带把运行时的**世界状态自报**（`data-p2-world`：`scene=/map=/alive=/player=@x,y/clip`）
   写进 summary 与日志。它在这里只是**证据**、不是判定：无头浏览器这一次是"打开页面等 15s
   再 dump DOM"，停在启动场景是正常的（玩家要按键才进玩法地图）。有牙的那一组断言在
   `scripts/wechat-minigame-harness.js --drive-new-game --expect-runtime-spawns --drive-move`
   里——两个 web 平台共用同一份 `web-runtime.js`，字段含义一致，可以直接对照。

同一套退出码由 `test-bundle.ps1` 原样透传（它另外用 4 表示端口不可用）。

## 3. 已实测证据（本机 Edge 153.0.4234.32，无头）

`powershell -File scripts/verify-webgl.ps1` → `webgl_browser_ok`：

- WebGL2 可用：`webgl2: true`，渲染器 `ANGLE (Microsoft, Microsoft Basic Render Driver …, D3D11)`
- 资源可获取：`manifestEntries: 662`、`startupScene` 解析正确并成功取回、manifest 首个资源可取（811 字节）
- 存储可用性探测：`storage.available: true`（不可用时会降级到内存并标记 `fallback: true`，不抛错）
- 缺失资源可诊断：请求不存在的资源返回 `404`（不静默成功）
- 产物：`artifacts/logs/webgl/{verify-webgl-*.log,dom-*.html,summary-*.json,wasm-target-*.txt}`

`package.ps1 -Platform webgl`（M31/P3，本机已具备 `wasm32-unknown-unknown` + `wasm-bindgen`）：

- `webgl_package_ok files=6`，`Package size: 21,522.8 KB, 673 files`
- 产物：`index.html`、`web-runtime.js`、`pkg/game_web.js`（17.0 KB）、
  `pkg/game_web_bg.wasm`（39.6 KB，wasm 魔数 `\0asm` 校验通过）、`pack/`（662 项）、`fonts/`

`powershell -File scripts/verify-webgl-bundle.ps1` → **`webgl_bundle_boot_failed`（exit 1）**：

- 好消息：wasm 模块**真实加载**，页面**进入了 Lua 执行**（`scripts/main.lua` 已被加载执行）；
- 坏消息：`game_jhyt` 的脚本在一个 WebGL 运行时没有实现的宿主 API 上失败——
  `[string "scripts/main.lua"]:6545: attempt to index a nil value (global 'item')`，
  失败时页面标记 `data-p2-boot="failed"` 并给出缺失清单：

  `app, asset, calendar, clock, container, data, day, event, grid, interact, inv, item, light,
  map, shop, sprite, tag`（17 个）

即：`web-runtime.js` 目前实现的是**演示级子集**（`log/time/input/scene/entity/transform/
transform3d/tilemap/health/physics/physics3d/animation/animation3d/audio/ui/save/particles/
camera/mesh`），与桌面引擎注册的 36 个顶层全局并不等价。桌面发行（PC）不受影响。

## 4. 已知限制

- **宿主 API 子集**（原"M31/P3 实测未解决"一条已推进，2026-09-19 复测）：真发布包在无头 Edge 里
  `missing_host_api=0`、`boot=running`、canvas 有 15,987 个非背景采样点、世界自报
  `scene=17 map=main alive=17`，即**启动路径不再因缺失全局停下**。但这只证明"启动 + 进入 Lua +
  画出画面"，**不等于**"WASM 跑通完整剧本"（见下一条）。
  真正"浏览器内可玩"的两条路仍是：
  1. 把宿主 API 编译进 wasm（引擎侧 `core/script.rs` 的 Lua 桥 + `user_dir`/`save`/`input` 的
     wasm 实现，渲染走 wgpu 的 WebGL2 后端）——工作量最大，但能力与桌面一致；
  2. 明确"WebGL 只支持演示级子集"，把子集写成契约（允许的全局清单），让游戏工程按平台分叉，
     并在 `package.ps1 -Platform webgl` 时对不满足子集的工程**构建期报警**（而不是运行期报错）。
  当前默认按 (1) 规划、(2) 兜底（缺失清单已可机器读取：`bundle-summary-*.json` 的
  `missingHostApi`/`missingCount`）。

- **WP10（2026-09-27 立项）**：按 (1) 落地，设计见 `tasks/plan/WP10-WebGL宿主API编译进wasm.md`。
  工具链事实：mlua-sys 的 build 用 `cc` crate 编译 Lua C 源码，`wasm32-unknown-unknown` 无 C 编译器
  → 整引擎 script.rs 进 wasm 不可行；方案①落地 = 17 个宿主 API 以**纯 Rust** 实现进 `game_web` wasm
  （wasm-bindgen 导出），fengari Lua 经 `web-runtime.js` 桥接（两条 web 路径共用）。
  Phase 1（批次 A：`app/clock/calendar/day`）已实现：`engine/crates/game_web/src/host_api.rs` +
  桥 + index.html 接线；缺失清单 17 → 13（eager `missingHostGlobals()`，运行门禁 boot 正常）。
  批次 B/C 见工作包文档。
- **浏览器回归验证的是平台契约 + 产物启动**，不是「WASM 游戏跑通完整剧本」。
- **Lua VM 已随包分发**：`index.html` 引 `./pkg/fengari-web.js`（由 `package.ps1` 从
  `plugins/webgl/pkg/fengari-web.js` 复制），不再依赖 CDN，离线/纯内网可运行。
- **本机曾装不上 wasm32 目标**，原因不是网络不通：`rustup target add wasm32-unknown-unknown` 用的是
  **已装 toolchain 的 manifest 里烘死的绝对 URL**（`~/.rustup/toolchains/<tc>/lib/rustlib/
  multirust-channel-manifest.toml`），那份 manifest 指向的镜像对该文件返回 403，而
  `RUSTUP_DIST_SERVER` 对已装 toolchain 的组件下载无效。把 manifest 里的镜像前缀改成
  `https://static.rust-lang.org/dist/` 再 `rustup target add` 即成功（2026-09-19 实测；
  同样的提示写进了 `scripts/verify-webgl.ps1` 的诊断文件）。zig 与 wasm-bindgen CLI 现在
  都由 `package.ps1` 按需解析/安装（见 §2.1），不再需要手工 `cargo install`。
- 不承诺移动端浏览器与触摸设备体验；桌面无头浏览器结果不等同于桌面发行验收结果。
