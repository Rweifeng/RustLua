# 微信小游戏平台插件

微信小游戏发布所需的平台侧文件都在这个目录，`scripts/package.ps1 -Platform wechat-minigame`
会把它们复制进发布包并改写成对应工程。

| 文件 | 作用 |
|---|---|
| `game.js` | 小游戏入口：加载适配层 → Lua VM → 共享运行时 → 启动主循环 |
| `wechat-adapter.js` | wx 宿主适配层：把小游戏 API 映射成运行时所需的浏览器风格宿主 |
| `game.json` | 小游戏配置：横屏、网络超时。`subpackages` **由发布脚本按形态注入**，模板里保持不含（见下） |
| `project.config.json` | 微信开发者工具工程配置（`compileType: game`、忽略说明文件） |
| `remote-assets.json` | **可选**（`-AssetPack remote` 时才生成，见「远端资源回落」）：包内读不到 `pack/` 文件时回落到该地址下载，按清单校验后缓存 |

`pack/` 与 `mainpack/` 都不在这个目录里：它们由 assetc 与发布脚本产出到发布目录。
**资源形态三选一**（`-AssetPack`，微信平台默认 `subpackage`）：

* **`subpackage`（默认）**：`pack/` 声明为 `game.json` 的 `subpackages[0]`（`root = "pack"`），
  首屏资源被切到兄弟目录 `mainpack/`（属**主包**，启动即用），其余留在 `pack/` 分包里按需挂载。
  root 下必须有入口 `pack/game.js`——微信的硬要求，缺了开发者工具直接报
  「未找到 `["subpackages"][0]["root"]` 对应的 `/pack/game.js` 文件」并拒绝打开工程。
  现在的入口是**真的**入口（挂载后由宿主执行，发布脚本生成、harness 会 `require` 一次）。
* **`remote`**：瘦首包，重资源在 CDN。
* **`full`**：`pack/` 全量随主包。本工程 4.4 MB 必然超限。

三种形态互斥，细节与体积账见 [docs/52 微信小游戏分包发布](../../docs/52-微信小游戏分包发布.md)。

**代码包后缀白名单（真机硬约束，实测踩到）**：微信只上传后缀在白名单内的文件（69 个后缀，见
[scripts/code-package-rules.js](../../scripts/code-package-rules.js)）；`lua` / `toml` / `md`
**不在白名单里** —— 传不上去，真机上读它们是 `permission denied`。这个游戏的全部 Lua 逻辑都靠
逐文件读取，所以真机启动会直接失败（`pack/project.toml HTTP 404`）。构建期把这类文件改名成
`<原名>.txt`（`scripts/core/audio.lua` → `audio.lua.txt`），映射落在产物根的 `code-package.json`，
**运行时只查表、不重算规则**；分包 `pack/` 里的后缀本来都合规，所以改名不动分包、也不动 CDN 导出
（CDN 不受白名单约束，远端 URL / 缓存 / 校验和全按逻辑路径算）。
另：`*.js` 与 `game.json` 会被**编译**、上传后不能直接读，因此适配层**不得**拿 `game.json`
当运行期判据（分包判据改为首屏清单的 `layout.subpackage_root`，回归守卫是 harness 的
`--expect-no-read game.json`）。

## 发布与验证

```powershell
# 发布（默认形态：主包 + 单分包，零服务器/备案/域名白名单），-SkipBuild 可复用已有 assetc 产物
powershell -NoProfile -File scripts\package.ps1 -Platform wechat-minigame -SkipBuild -SelfTest
# 产物：<发布根>\launcher\（代码包，整目录上传微信）内含 mainpack.json + mainpack\ + pack\ + pack\game.js

# 可选：瘦首包（launcher/ 只留骨架 + assets/ 资源包）
powershell -NoProfile -File scripts\package.ps1 -Platform wechat-minigame -SkipBuild `
  -AssetPack remote -RemotePackBaseUrl https://cdn.example.com/games/jianghu -SelfTest

# 验证已有发布包：静态布局/配额 + 无头运行时实跑
# （-BundleRoot 可指 launcher/ 或整个发布根（会自动下沉）；资源包按 <发布根>\assets 自动探测）
powershell -NoProfile -File scripts\verify-wechat-minigame.ps1 `
  -BundleRoot examples\game_jhyt\builds\wechat-minigame\jianghu_youtian\launcher

# 只要运行时那一层（Node 伪造 wx API 真实执行 game.js）
node scripts\wechat-minigame-harness.js examples\game_jhyt\builds\wechat-minigame\jianghu_youtian\launcher --frames 12

# 分包形态用例（--subpackage 与 --wx-path-style 是正交的两轴）：
#   ok        → 声明有且仅有 1 个 root=pack、入口 pack/game.js 可执行
#   off       → 关掉挂载门（把"方言"这一轴单独隔离出来）
#   never     → 永不挂载：首屏照常启动 + 失败三重可见（属性/控制台/屏幕红条）
#   delay --subpackage-delay-frames 6
#             → 第 6 帧才挂载：证明"首屏不等分包"，也是进度条唯一可观察的窗口
#   fail-once → 首次失败、冷却后重试成功（冷却经存储注入压到 0 才能在用例里发生）
node scripts\wechat-minigame-harness.js <发布根>\launcher --frames 120 ^
  --subpackage delay --subpackage-delay-frames 6 --drive-new-game --expect-runtime-spawns

# 远端回落用例（仅 remote 形态；harness 自己起本地服务，不需要外部进程）：
#   thin    → 瘦首包：骨架走包内，重资源必须真的下载+校验+缓存
#   auto    → 包内完全读不到 pack/，一切都从远端取
#   corrupt → 服务端只翻转 1 字节（长度不变），必须被校验和拦下且启动失败
#   down    → 服务不可达，必须快速失败
node scripts\wechat-minigame-harness.js <发布根>\launcher --frames 12 --remote auto

# 宿主形态用例（都以"必须自报证据"为准，不看有没有报错）：
#   --image-src-prototype    宿主把 src 定义在原型上（基础库真实形态）→ 必须自报 prototype-accessor 且贴图真加载
#   --image-callback-bridge  宿主的 onload/onerror 也是原型访问器、回调存私有字段（基础库 k$ 包装类）
#                            → 必须自报 host-setter（用 own accessor 盖住 = 拆掉宿主的回调桥）
#   --image-src-silent --expect-boot-stall --boot-timeout-ms 1000 --boot-deadline-ms 6000
#                            宿主收下 src 却永不回调 → 看门狗必须报警**并画到屏幕上**
#   --image-src-frozen       宿主完全不让改写 src → **需要改写 src 的形态**（remote / subpackage）必须响亮失败；
#                            full 形态不需要改写，必须照常启动
#   --host-document stub|locked|frozen / --host-window separate   宿主自带残缺全局
#   --host-canvas offscreen / --debug-overlay / --remote png-fail --expect-image-band   画面自证
node scripts\wechat-minigame-harness.js <发布根>\launcher --frames 12 ^
  --image-src-silent --expect-boot-stall --boot-timeout-ms 1000 --boot-deadline-ms 6000
```

`verify-wechat-minigame.ps1` 会按**产物形态**自动分组跑完用例，全绿时输出（分包形态）：

```
code_package_whitelist_ok dirs=2 (rules_verified_on=2026-09-16/2.32.3)
wechat_minigame_static_ok main=1,534KB subpackage=3,011KB total=4,545KB subpackages=pack asset_pack=subpackage upload_rules=ok main_of_limit=37.5% subpackage_of_limit=73.5% total_of_limit=22.2%
wechat_minigame_package_ok files=12 runtime=ok path_styles=root,slash,sub-root,no-utf8 asset_pack=subpackage remote_fallback=n/a(分包形态不带远端配置) negatives=image-src-frozen,image-src-prototype,image-callback-bridge,boot-stall,host-document-frozen host_globals=ok,locked render_selfcheck=ok play_path=ok subpackage_case=ok,delay,delay-move,never,fail-once,no-game-json
```

瘦首包形态下同一行是 `asset_pack=thin remote_fallback=ok negatives=corrupt,down,… thin_case=ok`。

### 双击入口（不用记参数）

瘦首包在开发者工具里预览时，**必须有一个静态服务提供 `<CDN 基址>/pack/**`**（重资源不在包里），
所以仓库提供了两个 `.bat`（都会自动在 `examples\*\builds\wechat-minigame\` 下找到产物，
端口默认沿用 `remote-assets.json` 里烧的那个——端口不一致时工具会去拉一个没人监听的地址）：

| 双击 | 行为 |
|---|---|
| `scripts\serve-wechat-pack.bat` | 只起**资源包**静态服务（不跑验收），参数透传给 `test-bundle.ps1 -Platform wechat-minigame -NoVerify` |
| `scripts\test-wechat-minigame.bat` | 先跑完整验收（`verify-wechat-minigame.ps1`），再起同一个资源包服务 |

服务起来后：开发者工具「导入项目」选**发布目录**，并勾选
「详情 → 本地设置 → 不校验合法域名、web-view（业务域名）、TLS 版本以及 HTTPS 证书」。
等价的原始命令就是：

```powershell
powershell -NoProfile -File scripts\serve-pack.ps1 `
  -Root examples\game_jhyt\builds\wechat-minigame\jianghu_youtian-assets -Port 8787
```

发布之后用微信开发者工具「导入项目」选中发布目录即可；默认 appid 为 `touristappid`（本地预览），
上架前需要在 `project.config.json` 换成自己的 appid。

## 远端资源回落（`-AssetPack remote`：可选形态）

`-AssetPack remote` 会把资源包导出到 CDN 目录、包内只留 44 KB 骨架，并写入 `remote-assets.json`；
`-AssetPack full` 则把 `pack/` 全量随包分发（计入 4 MB 主包上限，本工程会超限）；
**默认形态 `subpackage` 既不需要 CDN 也不需要 `remote-assets.json`**（三形态互斥，发布脚本会拒绝混用）。
语义是 **包内优先 + 远端兜底**——先在代码包里读，读不到才下载、按清单校验、写入缓存，
下次直接命中缓存，因此不会因为配了远端就每次联网（`--remote auto` 用例里 `cache_hits` 覆盖了这一点）。

```powershell
# 1) 起一个本地静态服务托管**资源包目录**（瘦首包时发布目录里已经没有资源了）
powershell -NoProfile -File scripts\serve-pack.ps1 `
  -Root examples\game_jhyt\builds\wechat-minigame\jianghu_youtian-assets -Port 8787

# 2) 发布：瘦首包 + 导出资源包 + 写入远端地址
powershell -NoProfile -File scripts\package.ps1 -Platform wechat-minigame `
  -SkipBuild -AssetPack remote -RemotePackBaseUrl http://127.0.0.1:8787
# 想额外镜像一份到别处（方便 rsync/s3 sync；权威位置仍是 <发布根>\assets）：加 -CdnPackDir D:\cdn\jianghu
```

`remote-assets.json`（主包根目录，BOM 无关的纯 JSON）：

```json
{ "packBaseUrl": "http://127.0.0.1:8787/pack/", "verifyChecksum": true, "timeoutMs": 15000 }
```

- `packBaseUrl` 必须以 `http://` 或 `https://` 开头、以 `/` 结尾；不合法时发布脚本直接报错，
  **不会**写出半成品配置。发布脚本会把 `https://cdn.example.com/games/jianghu` 归一化成
  `https://cdn.example.com/games/jianghu/pack/`，因此 CDN 上要保证 `<目录>/pack/**` 这层结构。
- `verifyChecksum: true`：下载后比对 `pack/manifest.txt` 里的体积与 FNV-1a 校验和，
  不符就丢弃并记为 `remoteFailed`。
- 再次**不带** `-RemotePackBaseUrl` 发布时，陈旧配置会被删除——否则真机会去拉一个已经不存在的地址。
- `-SelfTest` 会把发布根（`launcher/` + `assets/`）整份复制到带空格的临时目录，用 `assets/` 当"CDN"跑验证。
- `-CdnPackDir <目录>` 是**镜像**导出：权威位置始终是 `<发布根>\assets`，给了这个参数就额外再导出一份
  （方便 `rsync` / `s3 sync`），同样走 staging + 原子交换。

**这解决了什么**：`pack/` 曾是贴着 4 MB 上限的 99.7%；把资源移到 CDN、包内只留骨架后，
代码包只有 479 KB。但这条路的代价是**必须有公网 HTTPS、必须备案、必须在公众平台配
`downloadFile` 域名白名单**——这也是默认形态改成 `subpackage` 的原因：后者零服务器、零备案、
零域名白名单，只靠微信自带的分发。详见 [docs/52](../../docs/52-微信小游戏分包发布.md)。

**本地服务只在开发者工具这一侧有用**（工具默认不校验域名，需要勾上
「详情 → 本地设置 → 不校验合法域名、web-view（业务域名）、TLS 版本以及 HTTPS 证书」）。
真机边界：

- 手机连不上 `127.0.0.1` / 局域网 IP，必须换成公网 **HTTPS** 地址；
- 该域名要加进微信公众平台后台的 `downloadFile` 合法域名白名单；
- 本地服务不能验证 CDN 行为、证书链、超时与限流——这些只能在真机/体验版上验。

**为什么必须有校验和**：远端内容被篡改、被截断、或者 CDN 回了一份旧文件时，
体积检查不一定能发现问题（改一个字节体积不变）。无头 harness 的 `--remote corrupt`
用例就是服务端只翻转 1 个字节、长度保持不变，必须被 `checksumMismatch` 拦下并让启动失败；
`--remote down` 则验证服务不可达时**快速失败**（实测 4~80 ms，不会挂在启动路径上）。

## 产物布局

**默认形态（`subpackage`）**：发布根下只有一个 `launcher\`，整目录一次上传微信。

```
examples\<game>\builds\wechat-minigame\<PublishName>\      # 发布根
└── launcher\                                   # 代码包（上传微信/导入开发者工具的目录），1,644 KB
    ├── game.js, game.json, project.config.json, wechat-adapter.js
    ├── web-runtime.js        # 与 webgl 共用的 Lua 运行时（同一份文件，逻辑不会漂移）
    ├── pkg/                  # 随包内核：fengari-web.js（Lua VM）+ game_web.js/_bg.wasm（wasm 核心，M32）
    ├── mainpack.json         # 首屏清单：构建/运行时/harness 三方共用的路由依据
    ├── code-package.json     # 改名映射：逻辑路径 → 会随代码包上传的宿主文件名（见下）
    ├── mainpack\             # 首屏资源（在**主包**里，启动即用，不等分包）
    │   └── manifest.txt, chars.txt, project.toml.txt, settings.toml.txt,
    │       scripts\**（逐个改名为 *.lua.txt）, assets\fonts\**, …
    ├── pack\                 # 分包 root（3,011 KB），目录下所有文件都进分包
    │   └── game.js           # 分包入口（微信硬要求；挂载后由宿主执行）
    └── release.toml, run.txt, deps.txt, build-info.txt
```

**可选形态（`remote`）**：发布根下是 `launcher\` + `assets\` 两个兄弟目录——`assets\` 是资源包
（整目录上传 CDN）。两者在**同一次 staging 里生成、一次原子交换落地**，
所以不存在"新首包配旧资源包"的半更新状态。

```
└── assets\                                     # 资源包（整目录上传 CDN，形如 <目录>/pack/**）
    └── pack\             # 与 launcher\pack\manifest.txt 逐一对应的完整资源树
```

> `launcher\pack\manifest.txt` 与 `assets\pack\manifest.txt` 必须**逐字节相同**：
> 远端校验用的是包内那份清单，两侧不一致时真机会把下载文件整批丢弃
> （`verify-wechat-minigame.ps1` 与 `-SelfTest` 都会断言这一点）。

体积（`jianghu_youtian` 实测）：

| 形态 | 主包 | 分包 | 总计 |
|---|---|---|---|
| **`subpackage`（默认）** | 1,644 KB（40.1%） | 3,019 KB（73.7%） | 4,663 KB（22.8%） |
| `remote` | 479 KB（11.7%） | — | 479 KB + CDN 上 4,083 KB |
| `full` | 4,507 KB（110%）→ **必然超限** | — | 4,507 KB |

> `wechat_minigame_size_precheck …`（写 run.txt/deps.txt/build-info.txt **之前**的初步值）
> 与收尾的 `wechat_minigame_package_ok …`（权威值）会差几 KB：微信上传时说明文件也计入代码包，
> 守卫盯的是后者。两个数字不一致属于预期，不是回归。

> **`-AssetPack full` 会把 `pack/` 整个算进主包**：本工程 4.4 MB 必然超限——发布脚本会在超限时
> **明确报错**并提示改用 `-AssetPack subpackage`（首选）或 `-AssetPack remote`，
> 而不是发出一个传不上去的包。小体量工程仍可用 full 自包含发布。

**不随包分发 17 MB 的完整字体**：assetc 已按项目用字子集化（实测 226 KB），
运行时用 `wx.loadFont` 注册该子集字体。它在包内的位置随形态变：分包形态下字体属首屏资源，
在 `mainpack/assets/fonts/`（构建期随 `mainpack.json` 一起切过去）。

**wasm 核心随包分发（M32 起）**：`pkg/game_web.js` + `pkg/game_web_bg.wasm`（实测 22 KB + 42 KB）。
微信分支**必须**用 `wasm-bindgen --target no-modules` 生成胶水——`--target web` 那份是浏览器
ES Module（顶层 `export { … }`），小游戏按 CommonJS 解析，加载即报
`WAGame.js:1 SyntaxError: MiniProgramError Unexpected token 'export'`。
no-modules 产物是纯脚本（无 import/export，整体包在一个 IIFE 里），由 `wasm-bridge.js` 包成
CommonJS 后、用 `initSync(bytes)` **同步实例化**（小游戏没有 `instantiateStreaming`、也不认
`Response`，走字节路径最少猜测）。见「wasm 核心」一节。

> **构建期有两处必需的补丁**（都在 `scripts/package.ps1`，改胶水写法时会响亮失败而不是静默放过）：
> ① no-modules 胶水的第一句是 `script_src = new URL(document.currentScript.src, location.href)...`，
> 小游戏的 `document` shim 没有 `currentScript`/`location`，**这句必然抛错**（实测
> `Cannot read properties of undefined (reading 'src')`）→ 包一层 try/catch；
> ② 胶水只有顶层 `let wasm_bindgen = (…)()`，**不是** `module.exports`、也不挂全局，
> `require()` 拿到的是空对象 → 追加一行 `module.exports = wasm_bindgen;`。

## wasm 核心（M32）

代码包里随包发行 `pkg/game_web.js`（22 KB，`--target no-modules` 胶水）+ `pkg/game_web_bg.wasm`
（42 KB）。这两样都在**主包**（`pkg/` 不在分包 root 下），因此启动时就能读到，不需要等分包。

**谁在什么时候加载它**：`wechat-adapter.js` 的 `loadWasmCoreOnce()`，在 `start()` 里
`probePackReadability()` 之后**同步**执行一次。它读字节（`readBytes('pkg/game_web_bg.wasm')`，
走适配层既有的路径方言与改名表）→ 调 `wasm-bridge.loadWasmCore()` → `initSync(bytes)` 实例化
→ 把导出清单与字节数记进证据。

**失败不掀翻启动**：wasm 只是"包里多带的一份核心"，游戏逻辑由 Fengari + `pack/` 承载。
失败时 `console.warn` 一行 + `data-p2-wasm` 记状态，Lua 那条路照常跑完。
**但不会被静默忽略**——状态有四个读取通道：

| 通道 | 怎么读 | 取值 |
|---|---|---|
| DOM 标记 | `__RustLuaWechat.getAttribute('data-p2-wasm')` | 同下表 `status` |
| 诊断快照 | `__P2_DIAG().wasm` / `wx.__p2Diag().wasm` | `{status, source, bytes, exports, detail}` |
| 启动日志 | 控制台搜 `wasm-bridge:` | 成功一行（含导出清单）或 `wasm 核心未就绪（<reason>）` |
| API | `__RustLuaWechat.loadWasm()` / `.wasmInfo()` | 手动触发 / 只读状态 |

`status` 的取值与含义（每一种都能指出是哪一层不对）：

| status | 含义 | 通常原因 |
|---|---|---|
| `ok` | 实例化成功，`exports` 是真实导出名 | 正常 |
| `not-attempted` | 还没触发（`start()` 之前） | 正常 |
| `no-webassembly` | 宿主没有 `WXWebAssembly` / `WebAssembly` | 无头 harness（未补该全局）；真机基础库过旧 |
| `webassembly-api-incomplete` | 只有 `instantiate`、没有 `Module`/`Instance` 构造器 | 宿主 wasm 实现残缺（`initSync` 走不通） |
| `wasm-bytes-unreadable` | 包内读不到 wasm 字节 | 发布形态没把它打进去（检查 `pkg/game_web_bg.wasm` 是否在产物里） |
| `instantiate-failed` | 胶水抛错（`detail` 带原始信息） | 胶水被改动后与宿主不兼容（如 M32 之前那句 `document.currentScript.src`） |
| `bridge-missing` | `require('./wasm-bridge.js')` 失败 | 平台文件没被复制进产物 |
| `glue-shape-unexpected` | `require('./pkg/game_web.js')` 不是函数或缺 `initSync` | **构建期没补 CommonJS 导出**（`--target no-modules` 只有顶层 `let`） |

**为什么用 `initSync` 而不是 async init**：`WXWebAssembly` 只有 `instantiate(path, imports)`，
而字节已经在我们手里，同步路径少一层"把字节包装成宿主认的形态"的猜测，也不需要
`fetch` / `Response` / `instantiateStreaming`（小游戏三者都没有）。实例化期间临时把
`globalThis.WebAssembly` 指向 `WXWebAssembly`（胶水的 `initSync` 用 `new WebAssembly.Module/Instance`），
调用完立即还原，不污染宿主全局。

**能力边界**：wasm 到"能读、能实例化、导出清单可核对"为止。`game_web::start()`（web-sys 形态）
在小游戏里跑不了，适配层不调用；`start_wechat()`（去 DOM 入口）已写在 Rust 侧但**未接线、未验证**
——详见 `engine/crates/game_web/src/lib.rs` 的状态段。

### 怎么验证 wasm 那一步

官方验收（`verify-wechat-minigame.ps1`）会跑十几个 harness 用例，但 **harness 伪造的 API 面里没有
`WXWebAssembly`**，所以它只能看到 `wasm.status=no-webassembly`。要把 wasm 那一步真正验证到，
给 harness 补上这个全局再跑：

```powershell
# 只验证"发布产物里的 wasm 桥接"（读字节 → 实例化 → 导出清单），最快
node artifacts\probes\verify-wasm-bridge.js examples\game_jhyt\builds\wechat-minigame\jianghu_youtian\launcher

# 在**完整启动路径**里验证：给 harness 预载 WXWebAssembly（能力面只有 Module/Instance/instantiate）
$env:NODE_OPTIONS="--require $PWD\artifacts\wx-preload.js"
node scripts\wechat-minigame-harness.js examples\game_jhyt\builds\wechat-minigame\jianghu_youtian\launcher --frames 12
Remove-Item Env:\NODE_OPTIONS
# 期望在日志里看到：wasm-bridge: 核心实例化成功（WXWebAssembly，42641 字节，导出 3 项：memory,start,start_wechat）
```

`artifacts/wx-preload.js` 是那个预载补丁（真机由宿主注入 `WXWebAssembly`，harness 没有，
所以它属于**验证环境**的缺口而不是适配层的问题）；`artifacts/probes/` 下是配套的一次性探针
（桥接直测 / 适配层直测 / 画布-版面一致性 / 改名表核对）。

### 画布尺寸 = 竖版设计分辨率（三处必须一起改）

M32 起游戏是竖版，适配层的画布也跟着是 **720×1280**（此前 1280×720）。这三处必须同时改，
否则症状是"竖版面板横向溢出画布、底部状态栏看不见"，而**无头验证看不出来**（假 ctx 不裁剪、
超界坐标照样计数）：

| # | 位置 | 值 |
|---|---|---|
| ① | `wechat-adapter.js` 的 `VIEW_WIDTH`/`VIEW_HEIGHT` | 720 / 1280 |
| ② | `plugins/wechat-minigame/game.json` 的 `deviceOrientation` | `portrait` |
| ③ | `engine/crates/game_web/src/lib.rs` 的 `canvas.set_width/height`（webgl 侧） | 720 / 1280 |

适配层启动时会做一次**一致性自检**（`reportViewAgreement`）：读 `project.toml` 的
`window_width/height`（= 工程声明的设计分辨率，与 `scripts/ui/layout.lua` 同源）与画布比对，
结果进 `data-p2-view` 与 `__P2_DIAG().view_agreement`。三档口径：
`ok`（读到声明且一致）/ `mismatch`（读到但不一致 → `warn`）/ `unknown`（读不到声明，跳过判断，
**不算错**）。
注意方向**不参与**这个比对：`game.json` 在真机上读不到（上传后被编译），适配层有一条硬约束
"一次都不许读它"（回归守卫 `--expect-no-read game.json`），所以方向只报编译期常量。

## 运行原理

适配层把运行时需要的宿主 API 映射到小游戏 API，因此 `plugins/webgl/web-runtime.js`
一行不改即可运行：

| 运行时需要 | 小游戏实现 |
|---|---|
| `fetch('pack/…')` | `wx.getFileSystemManager().readFileSync`，先查 `USER_DATA_PATH` 再查代码包。**首屏清单内的路径直接读主包 `mainpack/`（不等分包）**；清单外的才先 `await wx.loadSubpackage` 再读 `pack/` |
| `document` / `window` | 轻量 shim（`getElementById('p2-canvas')`、`body.setAttribute('data-p2-*')` 启动标记） |
| `new Image()` | `wx.createImage()`；**包内没有这张贴图时先下载到 `USER_DATA_PATH`，再把 `src` 指向本地文件**（`Image.src` 是宿主自己加载的，给一个不存在的路径只会静默 `onerror`，画面上少东西而日志里看不出来） |
| `FontFace` + `document.fonts` | `wx.loadFont` / `canvas.loadFont`，并把 `ctx.font` 里的 `RustLuaGameFont` 替换成真实字体族 |
| `requestAnimationFrame` | `wx.requestAnimationFrame`（无则 `setTimeout` 兜底） |
| `localStorage` | `wx.getStorageSync` / `setStorageSync` |
| `keydown` / `mouse*` | `wx.onKeyDown/onKeyUp`（微信 PC 端）；触摸映射为指针事件 + 左半屏虚拟摇杆（WASD）、右半屏动作键（space） |
| 生命周期 | `wx.onShow` / `wx.onHide` → `focus` / `blur` |

启动失败时适配层会把原因直接画到画布上（小游戏没有 DOM 控制台），便于真机定位。

## 已知边界（不要夸大）

- **wasm 核心：能装进来、能实例化、导出清单可核对（M32 起），但游戏逻辑不由它承载**。
  小游戏侧的游戏逻辑仍由 Fengari（Lua in JS）+ `pack/` 承载——这也是 webgl 发布当前的执行路径。
  M32 做的事是：微信分支**也**构建 `-p game_web` 的 wasm、以 `--target no-modules` 生成
  CommonJS 胶水随包发行，并由适配层在启动时同步实例化（结果进 `data-p2-wasm` /
  `__P2_DIAG().wasm` / 一行启动日志）。实测（`--target no-modules` + `WXWebAssembly`，
  42,641 字节，导出 `memory,start`）：
  `wasm-bridge: 核心实例化成功（WXWebAssembly，42641 字节，导出 3 项：memory,start,start_wechat）`。
  **边界（不要夸大）**：`start()` 是 web-sys 形态（`document`/`window`/`fetch`/`Response`），
  小游戏里跑不了，因此适配层**不调用**它；`start_wechat()`（去 DOM 入口）已写在 Rust 侧但
  **未接线、未验证**（见 `engine/crates/game_web/src/lib.rs` 的状态段）。
  所以"wasm 核心在小游戏里跑起来了"这句话，目前只对"加载 + 实例化 + 导出可读"成立，
  **不对**"引擎在 wasm 里跑 Lua/渲染"成立。
- **真机会没有 `WXWebAssembly` 吗**：不会（基础库 2.13+ 有），但 harness **默认没有**——
  它伪造的 API 面里没有这个成员。所以无头环境下适配层会报 `no-webassembly`，那不是缺陷复现。
  要验证 wasm 那一步，用 `NODE_OPTIONS=--require artifacts/wx-preload.js` 给它补上
  （能力面刻意只有 `Module`/`Instance`/`instantiate`，没有 `instantiateStreaming`）。
- **包里所有 `.js` 必须是 CommonJS**：发布脚本、验证脚本、无头 harness 三处都会按
  **脚本模式**（`vm.Script`，不是 `node --check`——Node 会自动识别 ESM 反而漏检）逐个解析，
  发现顶层 `export`/`import` 直接失败。
- **代码包路径形式由适配层自己探明**：开发者工具/真机/不同基础库对「根相对 / 前导斜杠 /
  `./` 前缀 / 分包根相对」的接受程度不一致，而 `FileSystemManager` 会把原因吞成异常，
  上层只能看到 `HTTP 404`（实测 `pack/project.toml HTTP 404` 就是这样来的）。
  适配层按候选策略逐个尝试、记住可用的那种，并在全部失败时打印**宿主原始错误**。
  启动时会输出一行自检，例如：

  ```
  wechat-adapter: 资源读取自检 manifest.txt=ok@mainpack project.toml=ok@mainpack settings.toml=ok@mainpack（路径形式: main=root-relative pack=root-relative，分包状态: ready）
  ```

  - 三个 =ok → 路径与挂载都正常（`@mainpack` 表示这一项按首屏清单被路由到了**主包** `mainpack/`，
    分包形态下这是预期：这三个探针验证的正是路由层本身）；
  - 全 FAIL → pack/ 里的文件读不到（看上一行的 `读取失败 …` 里宿主的 errMsg，以及末尾的
    `分包状态:`——`loading`/`failed` 时读不到 `pack/` 是**预期**的，首屏本就不该依赖它）；
  - 某项 =WRONG-CONTENT → 读到了内容不对的同名文件（路径形式被宿主错误解析）；
  - 末尾出现「远端回落已启用 …」→ 本包配了远端兜底，包内 FAIL 属于预期路径。
- **宿主自带的全局对象要抢得回来（开发者工具实测）**：微信开发者工具的壳里**已经有一个
  `document`**（半套 DOM，且**不可写**），于是适配层的 shim 在非严格模式下"赋值成功"其实
  是**静默失败**，全局上留下的仍是宿主那个残缺对象——直到运行时调用
  `document.getElementById(...)` 才炸出 `TypeError: document.getElementById is not a function`，
  报错点离原因很远。现在 `installShimGlobal` 一律**回读校验**：
  能接替就接替，接替不了就把 shim 的成员**就地补齐**到宿主对象上（`document`/`window` 都走这条路），
  并把结果（接替/已补齐/补不动/仍缺）记进 `__P2_DIAG()`。启动时会输出一行自检，例如：

  ```
  wechat-adapter: 宿主全局自检 window=globalThis document=宿主 document（已补齐） canvas=原对象 getElementById=ok body=ok
  wechat-adapter: 宿主全局自检 window=宿主 window（已补齐） document=适配层 shim canvas=转发包装 getElementById=ok body=ok
  ```

  **绝不在模块加载期抛错**（这一条是实测换来的）：开发者工具的宿主 `document` 是
  **不可扩展**的——`Object.keys(document)` 只有 `location`，只有已存在的自有属性（实测
  `documentElement`/`querySelector`/`getElementById` 三个）还能重定义，`body`/`createElement`/
  `fonts` 一律加不上去。早期版本"补不齐就在加载期 throw"，结果是 `game.js` 整体中止、
  连 `__RustLuaWechat` 都不存在（控制台只报一句 `__RustLuaWechat is not defined`），
  屏幕上**一片黑且没有任何线索**——比"跑起来再报错"难诊断得多。现在的分工：

  - 硬要求只有运行时会**直接调用且没有前置保护**的成员（`document.getElementById`）；
  - 补不动的成员用 `console.error` 明确喊出来并进诊断快照，不阻止启动；
  - 真正用到而不可用时，由 `start()` 的错误路径把原因**画到屏幕上**。

  这条契约由 `--host-document stub|locked|frozen`、`--host-window separate` 四个用例覆盖：
  残缺可补齐 → 照常启动跑满帧；**不可扩展（`locked`，开发者工具真实形态）→ 照样跑起来，
  `blocked` 成员被明确记录**；冻结到连 `getElementById` 都补不上 → 启动失败且 `console.error`
  给出"被宿主占用且补不齐"的诊断（一帧都不许跑）。

  另外：代码包会被开发者工具缓存，"有没有报错"分不清旧包新包，所以适配层会在加载末尾打一行
  `wechat-adapter: 适配层已就绪 v0.6.1+diag-world（__RustLuaWechat / __P2_DIAG 已注册；wx 通道=ok）`，
  版本号同时进 `__P2_DIAG().version`；控制台搜 `适配层已就绪` 就能确认现场跑的是哪一版。

  **控制台未必看得见游戏全局**（实测：`适配层已就绪` 打印了、`__P2_DIAG()` 却报
  `is not defined`——开发者工具控制台的"上下文"与游戏全局不是同一个对象）。所以诊断入口做成
  四条互不依赖的通道，任何一条可用就能拿到现场：

  | 通道 | 用法 | 前提 |
  | --- | --- | --- |
  | 全局 | `__P2_DIAG()` / `__RustLuaWechat.canvasInfo()` | 控制台上下文就是游戏全局 |
  | `wx` | `wx.__p2Diag()` / `wx.__p2CanvasInfo()` | 控制台里能看到 `wx` |
  | 存储 | `wx.getStorageSync('p2_diag')`，或直接在**存储面板**里看 | 开发者工具的存储面板能打开 |
  | 屏幕 | 覆盖层（发布目录放 `debug-overlay.txt`，或 `wx.setStorageSync('p2_debug_overlay','1')`） | **不需要控制台**，真机上唯一可用 |

  覆盖层的像素回读与四角测试图案、贴图失败警示条是"画面到底有没有真的上屏"的最终证据；
  `p2_diag` 快照在覆盖层开启后每 60 帧刷新一次。

  同样的道理适用于 canvas：宿主 canvas 若不允许挂监听（`canvas.addEventListener` 赋值静默失败），
  适配层会换成只做事件转发的包装对象（`getContext`/`width`/`height` 仍转发到真 canvas）。
- **屏幕自证（渲染出问题时唯一的证据来源）**：小游戏宿主没有 DOM 控制台，而最坏的一种故障是
  **"流程完全正常、屏幕全黑"**——实测有两条路会走到这里，且日志里都看不出来：
  ① `wx.createCanvas()` 只有**首次**调用返回上屏画布，之后返回离屏画布（引导代码先调过一次
  就会这样），画得再卖力也看不见；② 贴图**全部**加载失败时游戏照样 `boot=running`、帧循环照跑，
  只是画面上没有素材。适配层因此内置三件工具：

  - **上屏画布优先级**：优先用平台给的全局上屏画布（`GameGlobal.canvas` / `canvas`），拿不到才
    调 `wx.createCanvas()`；`__RustLuaWechat.canvasInfo()` 报告 `source` / `isMain`。
  - **常驻失败警示条**：只要贴图真的加载失败（宿主 `onerror`）或明知包内与远端都拿不到，屏幕顶部
    就出现红色警示条；启动失败画面也会带上它。
  - **覆盖层 + 像素回读**：把 `debug-overlay.txt` 放进发布目录，或调用
    `wx.setStorageSync('p2_debug_overlay', '1')`，屏幕上会叠一层四角色块 + 对角线 + 诊断文字，
    并在首帧做一次**真像素回读**（画 `#ff00ff` 再 `getImageData` 读回来）。控制台里
    `__P2_DIAG()`（以及每 60 帧的 `渲染自证 {...}` 日志）给出可复制的现场快照：主画布来源与尺寸、
    屏幕尺寸、`ctx` 能力、像素回读、帧数、`drawn`（运行时采样的非背景像素数）、贴图
    `ok/fail/未供应` 与失败样本、读取形式、远端状态、以及**世界状态**（`world`，
    见下一条）。
  - **世界状态行**：运行时每帧把 `scene=/map=/spawn=/alive=/player=@x,y/clip` 写进
    `data-p2-world`，适配层读出来翻成覆盖层的两行短句（`世界 地图=… 实体=…/… 落点=…`、
    `主角 …@x,y 动画=… 渲染侧=x,y`）。为什么需要它："帧循环在跑、UI 在画"和
    "世界里真的有人、主角真的在动"完全是两件事——前者全绿而后者全空时，画面就是一张
    背景图，而日志里一句错都没有。`渲染侧=` 是**渲染器自己**记住的坐标，与 Lua 世界的坐标
    并列显示，两者不一致就是回写通道（Lua → 渲染器）断了。

  这三条契约由 `--host-canvas offscreen`（离屏画布陷阱：上屏 168 次绘制、离屏 0 次）、
  `--debug-overlay`（像素回读命中 + 四角图案留在像素上）、
  `--remote png-fail --expect-image-band`（文本正常、PNG 全 500 → 必须出现警示条）三个用例覆盖，
  且都已进 `verify-wechat-minigame.ps1` 的常规质量门。
- **远端回落的信任边界**：校验和只能证明"下载到的字节与清单一致"，不能证明
  这个 CDN 是你的、也不能防住同时能改清单的攻击者（清单自身随包分发）。
  它是**防传输损坏/半截文件/缓存陈旧**的最小防线，不是内容签名。
- **缓存不会自动清理**：`USER_DATA_PATH` 下的副本会一直留着（小游戏对用户目录有容量上限，
  超限时写入失败只是本轮重新下载，不会崩）。需要精确控制容量时应自行加清理策略。
- **不支持 `-Encrypt` / `-Compress`**：共享 JS 运行时只读明文 `pack/`（不做 P2PK1 解包与
  gzip 解压）。发布脚本对 web 平台直接拒绝这两个开关，避免产出跑不起来的包。
- **触摸端是虚拟摇杆近似**，不是原生多指手势；手柄/键鼠映射仍以 PC 端为准。
- **远端模式下 `Image.src` 必须能被改写**：适配层把"包内没有的贴图"换成本地缓存路径，
  而宿主怎么写 `src` 由适配层启动时**自己探明**，并打一行自检
  `wechat-adapter: Image.src 接管方式 = prototype-accessor`（同时进 `__P2_DIAG().image_src_takeover`
  与覆盖层/`贴图` 诊断行）。三种形态**都必须能用**：

  | 接管方式 | 形态 | 写法 |
  | --- | --- | --- |
  | `own-accessor` | 实例上有 own 的 `src` 访问器 | 直接定义 own property 包一层 |
  | `prototype-accessor` | **微信基础库的真实形态**：`src` 是原型上的 class 访问器（实测 2.25.3 的 `Image` 类），实例上**没有** own 描述符 | 在原型链上找到 `set` 并 `descriptor.set.call(image, …)` |
  | `setSrc-method` | 代理/包装对象取不到任何描述符，但类自带 `setSrc()` | 调 `setSrc()` |

  三者都没有时：远端模式**响亮失败**（启动即报「无法把 Image.src 交给宿主」，而不是静默丢图），
  包内完整则告警后按原样交给宿主。契约由 `--image-src-frozen`（远端 → 必须响亮失败；
  包内完整 → 必须照常启动）与 `--image-src-prototype`（必须自报 `prototype-accessor`
  且 `imagesLoaded > 0`）两个用例覆盖。

  **`onload`/`onerror` 是同一条契约的另一半**，也必须自报
  `wechat-adapter: Image.onload/onerror 接管方式 = host-setter|own-accessor`
  （`__P2_DIAG().image_callback_takeover`）。宿主把回调做成**原型访问器**并存在自己的私有字段里
  （基础库 2.32.3 的 `k$` 包装类：`set onload(v){ this.__onload = v }`，而真实 image 的回调是
  **构造时**就接成 `this.__onload` 的）时，用自己的 accessor 盖住 `onload` ＝ 把宿主的桥拆掉：
  **贴图加载成功也传不回运行时**，`loadImage` 的 Promise 永不 settle。症状与"src 没交到宿主"
  一模一样（卡死 + 0 错误 + 全黑 + `ok=0 fail=0 未供应=0`），只有 `回调接管=` 这一项能区分。
  所以宿主有 setter 就**转发**给它，只在宿主是普通数据属性时才用自己的 accessor。
  契约由 `--image-callback-bridge` 覆盖（必须自报 `host-setter` 且贴图真的加载成功）。
- **启动看门狗：卡死也必须有人管**（`BOOT_TIMEOUT_MS`，默认 10s）。启动 Promise 既不 resolve
  也不 reject 的卡死（弱网、CDN 挂起、宿主收下 `src` 却永不回调）**不会产生任何异常**，
  日志里和"还在加载"长得一模一样。超时后适配层会 `console.error` 一段现场并
  **把"启动超时 …s"画到画布上**——真机/开发者工具里没有控制台，只打日志等于还是黑屏。
  超时可用存储覆盖（`wx.setStorageSync('p2_boot_timeout_ms', 300)`）：慢 CDN 上放大，
  自动化用例里缩短（否则验证看门狗本身就要等 10 秒，进不了质量门）。
  这条契约由 `--image-src-silent --expect-boot-stall` 覆盖：宿主收下 `src` 但永不回调 →
  断言看门狗确实报警**且"启动超时"真的落在画布像素上**（只报警不上屏 = 仍然黑屏）。
- **无头验证用 Node 伪造 `wx`**，覆盖"资源读取 + Lua 主循环 + 输入桥接 + 贴图加载 +
  玩家真实玩法路径"，但不能替代开发者工具与真机验收（渲染效果、性能、真机 API 差异仍需真机确认）。
  十条实测教训都已经被固化成断言：
  ① `pack/game.js` 缺失时本机全绿、开发者工具却拒绝启动 → 分包形态下这个入口现在是**真的**
  入口（发布脚本生成、挂载时由 harness `require` 一次证明可执行），非分包形态则断言
  `subpackage_loads=0` 且 `game.json` 不含 `pack` 分包（两个方向都断言，避免"悄悄退化成 full"）；
  ② harness 的假 `Image` 曾直接从磁盘读贴图，于是"包内没有的贴图"在无头测试里照样"成功"，
  掩盖了真机上贴图会全部静默消失的问题 → 现在假 `Image` 只走宿主的受限读取路径，
  并且启动 Promise 不 settle 时会带诊断**失败**（而不是无限等待）；
  ③ 开发者工具里 `document.getElementById is not a function`：壳自带的不可写 `document` 吞掉了
  适配层 shim（赋值静默失败）→ 现在回读校验 + 就地补齐，并用 `--host-document stub` 在本地
  1:1 复现这个宿主形态（含 `--host-window separate` 的独立 window）；
  ④ 开发者工具里 `__RustLuaWechat is not defined`：宿主 `document` **不可扩展**（
  `Object.keys` 只有 `location`，只有已存在的自有属性能重定义），早期"补不齐就在模块加载期
  抛错"直接把 `game.js` 打中止——黑屏 + 唯一一句控制台错误 → 现在改为"补能补的、报不能补的、
  照常启动"，并新增 `--host-document locked` 1:1 复现该形态（顺带加了适配层版本号日志，
  否则分不清开发者工具跑的是新包还是缓存里的旧包）；
  ⑤ "流程正常但画面全黑"：`boot=running` 与"真的画到屏幕上"是两件事 → 现在用
  `--host-canvas offscreen` 覆盖离屏画布陷阱，用 `--remote png-fail --expect-image-band`
  覆盖"贴图全丢却照常启动"（实测 35 张贴图全失败时 boot 仍是 running），
  并给出屏幕警示条 / 覆盖层 / `__P2_DIAG()` 三条自证通道；
  ⑥ 开发者工具（稳定版 2.02.2608060，基础库 2.25.3）里**全黑、0 帧、0 错误、日志全绿**：
  适配层只读 `Object.getOwnPropertyDescriptor(image, 'src')`，而基础库把 `src` 定义在
  **原型**上（`class Image { setSrc(v){this._src=v;…} getSrc(){return this._src} }`），
  于是拿到 `undefined` 后静默退化成写 `image._src` —— 而基础库是在 `setSrc` 里才发起加载的，
  宿主永远不开始加载 → 运行时的贴图 Promise 永不 settle → 启动卡死。现在按原型链探明
  own/原型访问器与 `setSrc` 三种接管方式（`--image-src-prototype` 1:1 复现该形态），
  并把这个"全黑但零错误"的故障类补上**启动看门狗**（`--image-src-silent --expect-boot-stall`）；
  ⑦ 修完 ⑥ 之后**开发者工具里仍然全黑**，只是现场换成了 `src接管=prototype-accessor` +
  `remote=10 failed=0` + `ok=0 fail=0`：这一半是 `onload`/`onerror`。基础库 2.32.3 的
  `k$` 包装类把回调也做成原型访问器并存在私有字段 `__onload` 上，而真实 image 的回调桥是
  **构造时**按 `this.__onload` 接好的——适配层用自己的 accessor 盖住 `onload` 就把桥拆了，
  贴图加载成功也传不回运行时。现在回调同样**转发给宿主的 setter**，并自报
  `Image.onload/onerror 接管方式`（`--image-callback-bridge` 1:1 复现该形态）。
  ⑧ 贴图全修好之后，**"点开始新游戏"仍然崩**（`attempt to call a nil value (field 'current')`）：
  引擎里 `scene.load` 是**排队的场景切换**（真正换数据发生在下一帧），而 web 运行时若不换
  场景数据，`update()` 会立刻按"已经在玩法地图里"的分支往下跑——于是没有场景数据的表被当成
  有数据用。现在换场景与"排队的场景切换 + `map` 表"一起落地，并新增 `--drive-new-game`
  走玩家的真实路径（标题按确认 → `on_click('BtnStart')` → `scene.load("manor")`），
  而不再只跑标题界面。
  ⑨ "帧循环全绿、世界却是空的"：宿主 API 缺失曾**静默降级**成空表（`entity.find_id` 返回真值 →
  `activate()` 永远走"已存在"分支 → NPC 根本不生成；`tilemap.find_path` 返回 nil → 寻路直接放弃）。
  现在缺失的宿主 API **逐条点名**（`[lua missing-host-api] X`）+ 白名单断言（只允许游戏自己用
  `type(x)=="function"` 探测过、会降级的那批），并且按引擎源码逐条补齐：`physics.set_position`、
  `transform.set`、`entity.spawn/find_id/persist_id/destroy`、`sprite.set`、
  `animation.attach/set/get/ended/frame`、`tilemap.set_cell/set_texture/find_path`（二叉堆 A*，
  与 `Map::find_path` 同序）、`save.clear`、`audio.play_on/set_bus_volume`、`log.debug`。
  ⑩ "世界在动"与"画面在动"是两件事：Lua 世界与渲染器各存一份实体数据，靠**回写通道**同步
  （结构变更走一次性 op 列表，每帧的 Transform/动画走 `T`/`A` 行）。只在 Lua 侧取样的话，
  "世界里主角在走、画面上钉死在出生点"完全看不见 → 运行时每帧自报 `data-p2-world`
  （含渲染侧坐标 `drawn=`），质量门新增玩法路径用例
  （`--drive-new-game --expect-runtime-spawns --drive-move`）：走到玩法地图后
  **按住运行时自己从触摸摇杆派发出来的键**若干帧，断言主角坐标真的变了、且渲染侧坐标与
  Lua 世界一致。按哪个键由运行时产出、用例不硬编码游戏键位——断的是宿主契约，不是某一款游戏的键位。
  三条反向用例（`T` 行关掉、速度积分关掉、`entity.spawn` 关掉）都必须让对应用例变红，
  否则断言就是摆设。
  ⑪ 分包形态（主包 + 单分包）另有三条**本机才能抓**的陷阱，都已固化成断言：
  **清单漏列**（首屏资源留在 `pack/` 里 → 真机上要等分包下载，表现为标题白屏）由
  `denied_before_boot>0` 与发布期的字节守恒断言双向拦截；
  **挂载前就把贴图交给宿主**（`Image.src` 是同步通道，先交一个读不到的路径会让宿主立刻 `onerror`，
  而运行时的 `loadImage` 以第一次 `onerror` 为准，之后补挂载也救不回来）由"未挂载时延迟到
  分包结算再写 `src`"+`--subpackage delay` 拦截；
  **进度条把帧数断言打红**（适配层自绘会让某帧之后每帧少画几笔 → "每帧绘制数严格递增"递减）
  由 `withAdapterDraw` 标记 + harness 侧排除适配层的笔数解决——口径是**排除适配层的笔**，
  不是把断言放宽成 `ops > 0`（那等于放弃这条证据）。
