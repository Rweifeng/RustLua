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

# 本地静态服务（发布目录必须经 HTTP 提供，不能 file:// 打开：浏览器会拦 WASM/资源请求）
node scripts/serve-static.js examples/game_jhyt/builds/webgl/<name> 8000
```

`verify-webgl.ps1` 的行为：

1. 检测 `wasm32-unknown-unknown` 是否安装；
2. 组装 web 包（`assetc package` + 契约检查页）；
3. 用 `scripts/serve-static.js`（零依赖 Node 静态服务器）在本地端口提供该目录；
4. 用 **无头 Edge/Chrome**（`--headless=new --dump-dom`，带 60s 超时兜底）执行契约检查并断言 DOM 结果；
5. 把浏览器版本、构建产物、DOM 快照、结果 JSON 与日志写入 `artifacts/logs/webgl/`。

## 3. 已实测证据（本机 Edge 153.0.4234.32，无头）

`powershell -File scripts/verify-webgl.ps1` → `webgl_browser_ok`：

- WebGL2 可用：`webgl2: true`，渲染器 `ANGLE (Microsoft, Microsoft Basic Render Driver …, D3D11)`
- 资源可获取：`manifestEntries: 696`、`startupScene` 解析正确并成功取回、manifest 首个资源 `assets/art/buildings/_buildable_preview.png`（16101 字节）
- 存储可用性探测：`storage.available: true`（不可用时会降级到内存并标记 `fallback: true`，不抛错）
- 缺失资源可诊断：请求不存在的资源返回 `404`（不静默成功）
- 产物：`artifacts/logs/webgl/{verify-webgl-*.log,dom-*.html,summary-*.json,wasm-target-*.txt}`

## 4. 已知限制

- **WASM 模块构建未在本环境执行**：本机为离线环境（`static.rust-lang.org` 不可达），
  `rustup target add wasm32-unknown-unknown` 报 `component download failed … os error 2`，
  因此 `cargo build -p game_web --release --target wasm32-unknown-unknown` 与
  `wasm-bindgen` 输出无法产出；`package.ps1 -Platform webgl` 会在该步明确失败（不会产出
  半成品包，M20-03 的 staging 机制保证发布目录不被破坏）。联网后按第 2 节命令即可补齐。
- 浏览器回归验证的是**平台契约**（资源获取/解析、WebGL2 能力、存储降级、缺失资源诊断），
  不是「WASM 游戏跑通完整剧本」；后者需要先完成 WASM 构建。
- 不承诺移动端浏览器与触摸设备体验；桌面无头浏览器结果不等同于桌面发行验收结果。
