# scripts —— 仓库自动化脚本索引

本目录按功能分层。CI、根 README、`docs/` 与各 `*.bat` 双击入口使用的路径均已同步到新位置；
`CHANGELOG.md` 与 `tasks/done/` 是历史归档，保留旧路径不改，以本页为新口径。

| 目录 | 职责 |
|---|---|
| `build/` | 构建与打包 |
| `verify/` | 质量门与验收 |
| `test/` | 回归测试与自测 |
| `dev/` | 本地运行、静态服务与探针 |
| `docgen/` | 文档与代码生成 |
| `repo/` | 仓库维护 |
| `lib/` | 跨类别公共件（被上面几类复用） |

## build/ —— 构建与打包

| 脚本 | 作用 |
|---|---|
| `package.ps1` | 组装自包含发布包（PC / WebGL / 微信小游戏），`-SelfTest` 做目录隔离自测 |
| `package-engine.ps1` | 引擎绿色版/发布包组装 |
| `release-atomic.ps1` | 发布目录的临时 staging、原子替换与失败回滚（被 `package.ps1`、`test-release-atomic.ps1` 点源） |
| `package-branding.ps1` | 把启动品牌图（icon/splash）落到发布根的 `branding/` |
| `package-icon.ps1` | 把工程 `icon_image` 注入发布暂存目录的 `game.exe` |
| `first-screen-pack.js` | 首包/代码包清单、后缀白名单与分包体积检查 |
| `strip-lua-comments.mjs` | 发布产物 Lua 注释剥离 + manifest 重写 |
| `minify-toolchain.ps1` | binaryen（wasm-opt）与 terser 的定位/按需安装 |
| `zig-toolchain.ps1` | zig 的定位/按需下载 |
| `wasm-bindgen.ps1` | wasm-bindgen CLI 的版本解析与按需安装（版本必须与 `Cargo.lock` 一致） |

## verify/ —— 质量门与验收

| 脚本 | 作用 |
|---|---|
| `verify.ps1` | **唯一总门**：文档门、cargo 三件套、`game --check`/`editor --check`、headless 回归、Web/微信/基准门 |
| `verify-webgl.ps1` | WebGL 浏览器集成回归 |
| `verify-webgl-bundle.ps1` | 验证真实发布出来的 WebGL 包能启动并跑通场景切换 |
| `verify-webgl-eh.ps1` | wasm 异常处理（EH）探针门 |
| `verify-wechat-minigame.ps1` | 微信小游戏发布包静态布局/配额 + 无头运行验证 |
| `verify-wasm-lua.ps1` | wasm Lua 运行期验证（spike + no-modules 桥） |
| `check-docs.ps1` | 文档一致性门（编号/H1/索引/链接/历史路径/BOM/解析） |
| `check-silent-traces.ps1` | 静默失败"留痕"回归门 |
| `check-vendor-patches.ps1` | vendor 补丁基线自动校验 |
| `check-input-abi-parity.mjs` | 输入线格式（`RLFI`）键表静态一致性门 |
| `check-upload-ecma.mjs` | 微信上传器代码包 JS 解析器兼容性门 |
| `engine-pub-surface.mjs` | engine crate 对外 Rust 符号面基线比对 |
| `compare-baseline.ps1` | 性能基线对账（p95/p99/内存红线） |
| `compare-golden.ps1` | 黄金报告/黄金表对账 |
| `compare-shots.ps1` | 两张页面截图的差异读数（不判红绿，供人判读） |
| `wechat-minigame-harness.js` | 微信发布包无头验证器（伪 `wx` 宿主，跑真实帧） |
| `gpu-web-probe.ps1` | GPU 后端能力探针与渲染器探针 |
| `gpu-web-probe/` | 探针页面（`index.html` / `render.html`） |
| `webgl-contract/` | WebGL 行为契约页面（`index.html` / `web-check.js`） |

## test/ —— 回归测试与自测

| 脚本 | 作用 |
|---|---|
| `test.ps1` | 质量门快捷入口（`-NoClippy`） |
| `test-bundle.ps1` | 本地试玩一个 web 平台发布产物（验证 + 静态服务） |
| `test-webgl.bat` | 双击入口：验证 WebGL 包并起本地服务 |
| `test-wechat-minigame.bat` | 双击入口：验证微信包并起资源服务 |
| `test-analyze-soak.ps1` | `lib/analyze-soak.ps1` 的自测 |
| `test-compare-baseline.ps1` | `verify/compare-baseline.ps1` 的红线回归 |
| `test-release-atomic.ps1` | 原子发布交换与回滚的独立回归 |
| `test-git-hooks.ps1` | pre-commit 门自身的回归（临时 git 仓库真实提交） |
| `qa_full.ps1` | 《江湖有田》阶段 5 全量 QA（headless） |
| `web-fixed-step-test.js` | Web 运行时固定时间步回归 |
| `wasm-bridge-fallback-test.js` | wasm-bridge 通道组合的定点回归 |
| `wechat-asset-routing-test.js` | 微信适配层资源路由单元测试 |
| `wechat-startup-branding-test.js` | 启动品牌图确定性测试 |
| `test-wechat-multisub.ps1` | 微信多分包根端到端（audio → `pack-audio`，boot 前预挂 + 容器读取） |

## dev/ —— 本地运行、静态服务与探针

| 脚本 | 作用 |
|---|---|
| `run-demo.ps1` | 运行默认 `examples` 工程桌面 Demo |
| `serve-static.js` | 零依赖静态文件服务器（WebGL 浏览器集成用） |
| `serve-pack.ps1` | 服务微信小游戏的资源包（供 DevTools 预览薄首包） |
| `serve-wechat-pack.bat` | 双击入口：`test-bundle.ps1 -NoVerify` 的薄封装 |
| `pack-server.js` | 本地静态资源服务器，作为远程 CDN 的测试替身 |
| `https-cert.ps1` | 给本地试玩服务出自签名证书（PFX） |
| `wechat-devtools-probe.js` | 微信开发者工具模拟器侧探针 |

## docgen/ —— 文档与代码生成

| 脚本 | 作用 |
|---|---|
| `gen-rustdoc.ps1` | rustdoc 站点构建与导航页生成 |
| `gen-lua-api-docs.js` | Lua API 文档桩生成/一致性校验（`--check`） |
| `gen-api-showcase-index.ps1` | 从 `demos.lua` 重生成示例工程入口菜单场景 |

## repo/ —— 仓库维护

| 脚本 | 作用 |
|---|---|
| `install-git-hooks.ps1` | 安装/体检/卸载本地 pre-commit 门（`-Status`、`-Clippy`、`-Uninstall`） |

## lib/ —— 跨类别公共件

| 脚本 | 作用 |
|---|---|
| `bundle-discovery.ps1` | 定位某个工程/平台的发布产物目录（被 verify/test 复用） |
| `project-image-key.ps1` | `project.toml` 的 `icon_image` / `splash_image` 唯一解析口径 |
| `code-package-rules.js` | 微信代码包上传规则（构建期与验收侧共用同一份） |
| `web-api-contract.js` | Web 侧行为契约执行器共用件（页面表达式 + 对拍） |
| `webgl-cdp-wait.js` | 通过 Chrome DevTools Protocol 轮询页面状态并 dump DOM |
| `analyze-soak.ps1` | soak 归档判据分析（窗口谷值/中位/峰值） |
| `wasm-lua/` | wasm Lua 探针与后处理：`downgrade-noextern`、`strip-wasm-globals`、`strip-wasm-tags`、`verify-wasm-host`、`measure-lua-boundary`、`run-spike`、`verify-no-modules-bridge`、`verify-canvas-host-probe`、`eh-probe-*` |

## 调用关系

- `verify/verify.ps1` 是唯一总门，CI 的 `quality.yml`、`test/test.ps1` 都从它进入。
- `build/package.ps1` 点源 `build/release-atomic.ps1`、`build/zig-toolchain.ps1`、`build/wasm-bindgen.ps1`、
  `build/minify-toolchain.ps1` 与 `lib/project-image-key.ps1`。
- `test/test-bundle.ps1` 点源 `lib/bundle-discovery.ps1`，并调用 `verify/verify-*.ps1` 与 `dev/` 下的服务器。
- `lib/` 下的公共件不直接作为门入口；改它们等于同时改构建与验收两侧的口径。

## 常用命令

```powershell
# 总门（完整）
powershell -NoProfile -File scripts/verify/verify.ps1

# 发布（PC / WebGL / 微信小游戏）
powershell -NoProfile -File scripts/build/package.ps1 -Project examples/game_api -Platform pc

# 本地试玩 WebGL 发布包
powershell -NoProfile -File scripts/test/test-bundle.ps1 -Platform webgl

# 只跑文档门
powershell -NoProfile -File scripts/verify/check-docs.ps1

# 安装本地 pre-commit 门
powershell -NoProfile -File scripts/repo/install-git-hooks.ps1
```
