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
# 完整 WebGL 发布（需要 wasm32 目标与 wasm-bindgen-cli）
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.127
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/package.ps1 -ProjectRoot examples/game_jhyt -Platform webgl

# 浏览器集成回归（日志与 DOM 快照写到 artifacts/logs/webgl/）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify-webgl.ps1

# 产物层启动验收：真实发布包在无头浏览器里能否启动到 Lua（M31/P3 新增）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify-webgl-bundle.ps1 `
    -BundleRoot examples/game_jhyt/builds/webgl/jianghu_youtian

# 本地静态服务（发布目录必须经 HTTP 提供，不能 file:// 打开：浏览器会拦 WASM/资源请求）
node scripts/serve-static.js examples/game_jhyt/builds/webgl/<name> 8000
```

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
   **缺失的引擎宿主 API 全局**列出来；无 node/浏览器 → 退出 2（环境受限，不伪造通过）。

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

- **WebGL 运行时没有实现桌面引擎的宿主 API（M31/P3 实测，未解决）**：浏览器里能加载 wasm、
  能进 Lua，但 `game_jhyt` 会在第一个未实现的全局上停下（见第 3 节清单）。要真正"浏览器内可玩"，
  需要二选一：
  1. 把宿主 API 编译进 wasm（引擎侧 `core/script.rs` 的 Lua 桥 + `user_dir`/`save`/`input` 的
     wasm 实现，渲染走 wgpu 的 WebGL2 后端）——工作量最大，但能力与桌面一致；
  2. 明确"WebGL 只支持演示级子集"，把子集写成契约（允许的全局清单），让游戏工程按平台分叉，
     并在 `package.ps1 -Platform webgl` 时对不满足子集的工程**构建期报警**（而不是运行期报错）。
  当前默认按 (1) 规划、(2) 兜底（缺失清单已可机器读取）；本机离线环境下也验证过构建链路可复现。
- **浏览器回归验证的是平台契约 + 产物启动**，不是「WASM 游戏跑通完整剧本」。
- WASM 模块依赖 CDN 上的 `fengari-web`（`index.html` 里 `<script src="https://cdn.jsdelivr.net/...">`）：
  离线或纯内网部署需要把该脚本随包发布（已记入待办）。
- 不承诺移动端浏览器与触摸设备体验；桌面无头浏览器结果不等同于桌面发行验收结果。
