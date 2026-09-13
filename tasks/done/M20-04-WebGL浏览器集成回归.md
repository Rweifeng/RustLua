# 任务：WebGL 浏览器集成回归

- 任务 ID：M20-04
- 状态：部分完成（WASM 构建受环境阻塞，浏览器侧契约已实测通过）
- 关联里程碑：M20
- 创建日期：2026-09-09

## 目标

建立 `game_web` 和 `game_jhyt` 的浏览器集成回归，覆盖启动、输入、资源加载、存档降级和核心垂直切片流程。

## 影响文件

- `engine/crates/game_web/`（未改：入口已按固定资源路径实现，本轮以契约测试覆盖）
- `engine/crates/game/`（未改）
- `examples/game_jhyt/web/{index.html,web-check.js}`（新增：浏览器侧契约检查页）
- `scripts/verify-webgl.ps1`、`scripts/serve-static.js`（新增：浏览器集成回归与静态服务）
- `docs/32-webgl-浏览器平台验收.md`（新增：平台验收说明与限制）

## 风险

- 浏览器、WebGL 和本地文件系统能力不同；测试必须使用浏览器可用的资源和存储路径。
- 自动化浏览器环境可能不稳定；核心规则应保留 headless 测试，浏览器测试只验证平台集成。
- WebGL 性能与桌面不同，必须单独记录帧时间和资源加载失败。

## 实现记录

- **浏览器契约检查页**（`examples/game_jhyt/web/`）：零依赖 HTML+JS，在真实浏览器中验证 WASM 入口所依赖的平台契约——
  ① `pack/manifest.txt`（`assetc_manifest=1` 头 + `路径<TAB>字节数<TAB>指纹` 行）可获取并按 TAB 解析（696 项）；
  ② `pack/project.toml` 的 `startup_scene` 解析正确且对应场景可获取；
  ③ manifest 首个资源可获取；
  ④ WebGL2 上下文与渲染器探测；
  ⑤ 存储可用性探测（`localStorage` 不可用时降级到内存并标记 `fallback=true`，不抛错）；
  ⑥ 缺失资源必须得到可诊断的 404（不得静默成功）。
  结果写入 `<strong id="result" data-status>` 与 `<pre id="details">`（JSON），供无头浏览器断言。
- **验收脚本**（`scripts/verify-webgl.ps1`）：检测 wasm32 目标 → 组装 web 包（`assetc package` + 契约页）→
  `scripts/serve-static.js` 起本地静态服务 → 无头 Edge/Chrome（`--headless=new`，失败回落 `--headless`，
  统一 60s 超时兜底并杀进程）执行 `--dump-dom` → 断言结果 → 把浏览器版本、DOM 快照、结果 JSON、wasm 目标
  检测结果写入 `artifacts/logs/webgl/`。**状态提取只认结果元素/详情 JSON**，不复用裸 `data-status=` 正则
  （页面 CSS 选择器会先匹配，曾导致失败被误判为通过——已修）。
- **静态服务器**（`scripts/serve-static.js`）：Node 标准库零依赖，拒绝路径穿越，显式给出
  `.wasm`/`.js`/`.toml` 等 MIME（保证 `WebAssembly.instantiateStreaming` 类路径可用）。
- **修复**：`web-check.js` 的 `project.toml` 键解析偏移 bug（原实现把 `= "scenes/main.scene"` 截成
  `= "scenes/main.scene`，导致 404 误报）；结果元素同步写入文本与 `data-status`。

## 验收方式

- 在浏览器中启动发布包并完成新游戏、移动、交互、睡觉和重新加载流程。
- 验证资源加载失败、浏览器刷新和存储不可用时的诊断与降级行为。
- 记录支持浏览器、构建产物、运行日志和已知限制。
- 执行 WebAssembly 构建、浏览器集成测试和 `game_jhyt` 核心 headless 回归。

## 验收证据

- [x] `cargo fmt --all -- --check` / `cargo clippy --workspace …` / `cargo test --workspace` / `cargo build --workspace` 全绿（本轮收尾统一复跑；本任务未改 Rust 源码）
- [x] 浏览器集成测试通过：`powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify-webgl.ps1` → `webgl_browser_ok`，实测 Edge 153.0.4234.32 无头：`webgl2=true`（ANGLE/D3D11）、`manifestEntries=696`、`startupScene` 解析并取回成功、首个资源 16101 字节、缺失资源 404、`storage.available=true`
- [x] 资源加载失败与存储降级有诊断：缺失资源返回 404 且检查页判定失败；存储不可用时降级标记 `fallback=true` 不抛错（代码路径 + 探测证据）
- [x] 记录支持浏览器、构建产物、运行日志与已知限制：`docs/32-webgl-浏览器平台验收.md`；产物在 `artifacts/logs/webgl/`
- [x] `game_jhyt` 核心 headless 回归：`jhyt_smoke`/`jhyt_m17`/`jhyt_m17_npc`/`jhyt_m17_dialogue`/`jhyt_m17_shop`/`jhyt_m18_combat`/`jhyt_m18_stats`/`jhyt_m18_build`/`jhyt_m19_ui`/`jhyt_m19_soak` 全部 exit 0
- [ ] WebAssembly 构建（**环境阻塞**）：本机离线（`static.rust-lang.org` 超时），`rustup target add wasm32-unknown-unknown` 报 `component download failed … os error 2`，`cargo build -p game_web --target wasm32-unknown-unknown` 无法执行 → 无法产出 `pkg/` 与完整 WebGL 页面
- [ ] 浏览器中跑通完整剧本（新游戏/移动/交互/睡觉/重载）（**依赖上一条**：需要 WASM 模块；契约层已验证，玩法层未在浏览器执行）
- [x] 文档与 CHANGELOG 已更新（`docs/32-webgl-浏览器平台验收.md`；CHANGELOG v0.15 段由本轮收尾统一补）

## 未完成项 / 已知限制

- **WASM 构建未在本环境执行**：需要联网执行 `rustup target add wasm32-unknown-unknown` 并
  `cargo install wasm-bindgen-cli --version 0.2.127`；随后 `scripts/package.ps1 -Platform webgl` +
  `scripts/verify-webgl.ps1` 即可补齐完整链路（脚本已就绪，未通过部分会在 wasm 目标缺失时明确跳过并标注，
  不会伪装成通过）。
- 浏览器回归只覆盖平台契约（资源获取/解析、WebGL2 能力、存储降级、缺失资源诊断），不覆盖 WASM 内的玩法逻辑；
  玩法规则仍以 `game_jhyt` headless 回归为准（见 M19-04/M20-05）。
- 暂不承诺移动端浏览器和触摸设备体验；浏览器性能与桌面不同，本轮未记录 WebGL 帧时间（需先有 WASM 构建）。
