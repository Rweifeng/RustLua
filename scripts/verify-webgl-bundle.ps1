# verify-webgl-bundle.ps1（M31/P3）：验证**真实发布出来的 WebGL 包**能否在浏览器里启动
# 并跑通一次场景切换（防回归门）。
#
# 与 verify-webgl.ps1 的分工：
#   * verify-webgl.ps1  —— 平台契约层（资源获取/解析、WebGL2 能力、存储降级、缺失资源诊断），
#                          用一张契约检查页跑，不依赖 wasm 模块；
#   * 本脚本            —— 产物层：直接起静态服务器 + 无头浏览器打开 `builds/webgl/index.html`，
#                          断言 wasm 模块加载、页面进入 Lua 执行（`data-rustlua-boot` 标记），
#                          并且默认让首帧重新走一遍 applySceneSwitch（`?rustlua-scene-verify=1`），
#                          断言 `data-rustlua-scene-verify=done`。这条**场景切换门**是上次
#                          wasm panic=abort 与宿主闭包 raise 交叉崩溃（boot=ok 却一切就死）
#                          的回归哨兵；`-NoSceneVerify` 会把它关掉（只验 boot）。
#
# 用法：
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\verify-webgl-bundle.ps1 `
#       [-BundleRoot examples\game_jhyt\builds\webgl] [-Port 8137] [-TimeoutSeconds 90]
#   # 关掉场景切换门（boot-only，快但不防切换崩溃回归）：
#   powershell ... -File scripts\verify-webgl-bundle.ps1 -NoSceneVerify
#   # L3 行为契约对拍（PC transcript vs 活体页面跑同一份 lua_api_contract.lua，多开一次浏览器）：
#   powershell ... -File scripts\verify-webgl-bundle.ps1 -ApiContract
#
# 省略 -BundleRoot 时自动发现（本机唯一的合格 webgl 产物，见 scripts\bundle-discovery.ps1）：
# 默认值不再写死某个游戏工程，任何工程发布后都能直接跑。
#
# 退出码：0 = 启动成功且场景切换门通过（`data-rustlua-boot=ok` + `data-rustlua-scene-verify=done`）；
#         1 = 启动或场景切换失败（含缺失宿主 API 与 scene-verify 明细）；
#         2 = 环境受限（无 node/浏览器）；3 = 未找到产物（需 -BundleRoot 显式指定）。

[CmdletBinding()]
param(
    [string]$BundleRoot = "",
    [int]$Port = 8137,
    [int]$TimeoutSeconds = 90,
    # P0-①：URL 驱动"完整剧本"（`?rustlua-drive=auto`）：标题按确认 → 进玩法地图 → 按住方向键
    # 移动。断言 `data-rustlua-drive=done` 且位移非零（主角真的在浏览器里被驱动着走了）。
    # 与 -NoSceneVerify 独立：剧本是"跑游戏内容"，scene-verify 是"跑运行时切换机制"。
    [switch]$Drive,
    # P0-②：URL 驱动"完整玩法剧本"（`?rustlua-drive=jhyt`）：标题确认 → 新游戏(manor) →
    # 移动 → 背包/任务面板开合 → 山庄→云泽村真实场景切换 → 回山庄，里程碑逐段落定
    # （M1 新游戏 / M2 移动 / M3 背包 / M4 任务 / M5 场景切换 / M6 返回）。
    # 断言 `data-rustlua-drive=done` 且 detail 含全部里程碑。
    # 与 -Drive 互斥（两者都写 data-rustlua-drive）；带本开关时自动关闭 scene-verify
    # （首帧重载当前场景会干扰剧本的标题确认阶段）。
    [switch]$DriveJhyt,
    # 关掉场景切换防回归门（默认 ON）：仅当产物里没有可 reload 的启动场景时才关。
    [switch]$NoSceneVerify,
    # L3 行为契约对拍（默认 OFF，`verify.ps1 -Web` 打开它）：boot 落定后把仓库里的
    # `lua_api_contract.lua` 经 CDP 注入**这个活体页面**（补丁层已生效的 Lua VM）跑一遍，
    # 与桌面 transcript 基线逐行对拍。契约只碰纯状态类 API（clock/calendar/item/event/…），
    # 代价是这次无头实例里的游戏时间/背包定义会被改——那是**一次性浏览器**，不影响任何产物。
    # 契约文本不进发布包：它是验证资产，一帧都不参与游戏，却要占微信 4 MB 主包配额。
    [switch]$ApiContract,
    # M73-00 WP5 第五片：要求页面走 GPU 渲染路径（URL 带 `?rustlua-gpu=1`），并断言
    # `data-rustlua-gpu` 落到 `presented`。前提是产物用 `package.ps1 -Gpu2d` 构建过：
    # 没编进 GPU 宿主时这个属性根本不会被写，门会明确说明（而不是含糊地"没等到"）。
    [switch]$Gpu2d,
    # M73-00 WP6-02：落定后把**页面截图**写到这个路径（PNG）。抓的是合成后的画面，
    # 所以 GPU 画布的内容也在里面（WebGL 画布默认不可读，JS 侧 `toDataURL` 是空白）。
    # 用途：把"GPU 层画的"与"只有 Canvas2D 层时画的"两张图对起来看——两层画面是否一致的唯一客观证据。
    [string]$Shot = "",
    # M73-00 WP6-02：把 GPU 画布临时藏掉后再拍一张（同一时刻的"只有 Canvas2D 层"）。
    # 两张同帧对比才能回答"两层画面是否一致"——分两次运行取图会因动画相位不同而不可比。
    [string]$ShotUnder = "",
    # M73-00 WP6-03：把产物里的 `web-runtime.js` 换成替身 ⇒ **整个 JS 世界不启动**，
    # 只剩 Rust 那条路（`index.html`/wasm/pack 一行不改）。用于证明"Rust 侧自足"，
    # 也用于量"JS 世界到底还贡献了什么"。产物会被拷到临时目录，不污染构建输出。
    [switch]$StubJsRuntime,
    # M73-00 WP6-04：落定后真的按几下键，读"宿主收到的 DOM 事件数"与"落到模拟的边沿数"。
    # 两个数分开看才知道失败在"没收到事件"还是"收到了没应用"——Rust-only 宿主里没有页内剧本，
    # 只能用页内派发；CDP 的 `Input.dispatchKeyEvent` 在这个 headless 配置里已知进不来。
    [switch]$KeyProbe,
    # M73-00 WP6-04：GPU 帧循环的帧预算（URL 位 `rustlua-gpu-frames=`）。0 = 不设上限。
    # 需要它的原因：默认 240 帧跑完循环就停、状态属性随之冻结，于是"按键后读读数"永远看不到变化。
    [int]$GpuFrames = -1,
    # M73-00 WP6-05：确定性输入剧本（URL 位 `rustlua-input-script=`），如 `w:30-150`。
    # 给了它就额外开一道"位移非零"门：输入进去之后，Rust 侧从引擎世界读出的主角位置必须变过。
    [string]$InputScript = "",
    # 配合 `-InputScript` 的**自定义期望**：整条状态读数必须匹配这条正则（留空＝走默认的位移门）。
    [string]$ExpectWorld = "",
    # M73-00 WP6-06：随状态属性一起报出来的**只读** Lua 表达式（URL 位 `rustlua-world-expr=`），
    # 如 `require('core.state').get_mode()`。为什么要它：`scene_delta=` 这类输出差值证明不了
    # "是哪个逻辑动了"（待机动画也会让它动），工程自己的状态才是干净判据，而它只有工程知道。
    [string]$WorldExpr = "",
    # M73-00 WP6-07：**只跑一次**的、允许有副作用的 Lua 探针（URL 位 `rustlua-world-do=`）。
    # 存档这类验证必须有写能力（`save.write` → `commit` → `clear` → `load`），只读查询做不到。
    # 与 `-WorldExpr` 分开命名是刻意的：读数只读，动作显式。
    [string]$WorldDo = "",
    # M73-00 WP6-07：落定后从**浏览器侧**独立读一次 `localStorage`（键 + 长度）。
    # 这是"Rust 真的把存档写进了平台槽"的外部证据——只信 wasm 自己回报的 `commit=true` 不够。
    [switch]$StorageProbe,
    # M73-00 WP6-08：音频探针（`any` / `none` / 空 = 关）。从**浏览器侧**数 Rust 建出来的
    # `<audio>` 元素与错误数组：wasm 自报的 `audio=N` 是"我以为我请求了"，页面里的元素才是
    # "浏览器真的收到了"。自动播放策略的拒绝也记在那个错误数组里（最常见的"没声音"）。
    [string]$AudioProbe = "",
    # M73-00 WP3 第三片：Rust 侧运行时场景改用这个场景起（URL 位 `rustlua-sim-scene=`）。
    # 空 = 用工程启动场景。传 `village` 之类**有实体**的场景，才能验"引擎画得出工程真场景"。
    [string]$SimScene = "",
    # 模拟来源（URL 位 `rustlua-sim=`）：`script` = 引擎自己加载启动场景并跑脚本；
    # `scene-script` = 宿主给场景字节但走跑脚本那条分支；空 = 宿主给场景字节、不跑脚本。
    [string]$SimRoute = "",
    # 把工程脚本截断成前 N 行再跑（URL 位 `rustlua-sim-lines=`）：二分"脚本卡在哪一段"用。
    [int]$SimScriptLines = 0,
    # 调用方（`verify.ps1 -Web`）用它拿退出码：`Start-Process -PassThru -RedirectStandardOutput`
    # 返回的进程对象上 `ExitCode` 是 **$null**（本轮实测），而 `[int]$null` 会静默变成 0——
    # "包起不来"于是被当成"通过"。因此由子进程自己把结果写进这个文件。
    [string]$StatusFile = ""
)

$ErrorActionPreference = "Stop"
# 所有退出点都走它：写状态文件（若有）再退出，退出码对调用方始终可见。
function Exit-With([int]$Code) {
    if (-not [string]::IsNullOrWhiteSpace($StatusFile)) {
        "exit=$Code" | Set-Content -LiteralPath $StatusFile -Encoding ASCII
    }
    exit $Code
}

# 未捕获的终止错误（本脚本里大量 `throw`）也必须是"失败且**可见**"：调用方只看状态文件，
# 没有 trap 时它只剩"没写状态文件"这条模糊线索（真实原因还压在 stderr 里，不该靠人肉去捞）。
trap {
    Write-Host "[RED] webgl bundle verification aborted: $($_.Exception.Message) @line $($_.InvocationInfo.ScriptLineNumber)"
    Exit-With 1
}
$workspaceRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot "bundle-discovery.ps1")
if ([string]::IsNullOrWhiteSpace($BundleRoot)) {
    # 不写死游戏工程：自动发现本机唯一的合格 webgl 产物（诊断由 bundle-discovery 打印）。
    $BundleRoot = Resolve-GameBundle -Platform "webgl"
    if (-not $BundleRoot) { exit 3 }
}
if (-not (Test-Path -LiteralPath $BundleRoot)) {
    throw "WebGL bundle was not found: $BundleRoot (先跑 scripts\package.ps1 -Platform webgl)"
}
$bundle = (Resolve-Path -LiteralPath $BundleRoot).Path

$logRoot = Join-Path $workspaceRoot "artifacts\logs\webgl"
New-Item -ItemType Directory -Force -Path $logRoot | Out-Null
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$logPath = Join-Path $logRoot "bundle-$stamp.log"

function Write-Log([string]$Message, [string]$Color = "Gray") {
    # 本仓部分运行环境的 `Write-Host` 包装器不支持 `-Color`（会把参数并进文本），
    # 因此这里只把 RED/YELLOW 作为前缀写进纯文本：日志与证据必须是可比对的无装饰文本。
    if ($Color -eq "Yellow" -or $Color -eq "Red") {
        Write-Host ("[{0}] {1}" -f $Color.ToUpperInvariant(), $Message)
    } else {
        Write-Host $Message
    }
    Add-Content -Path $logPath -Value $Message -Encoding UTF8
}

# M73-00 WP6-03：`-StubJsRuntime` = 把产物里的 `web-runtime.js` 换成替身，**整个 JS 世界不启动**。
#
# 为什么能这么做而不动用户文件：`index.html` 启动 Rust 侧后是**显式**调一次
# `window.rustluaRunLuaGame(status)` 来拉起 JS 世界的，而 GPU 宿主是 Rust 的 `start()` 自己
# 读 `?rustlua-gpu=1` 拉起的（`game_web/src/lib.rs`）。所以只要把那个入口换成空实现，
# 跑起来的就是"只有 Rust"的那条路：`index.html`、wasm、pack 全部一行不改。
#
# 拷贝产物到临时目录（而不是就地改）：就地改会把用户下次打包的产物弄脏，且并行验证会互相踩。
# 位置必须在 `Write-Log` 定义之后——PS 顺序执行，放前面会报"无法识别的命令"（第一次就踩了）。
$stubBundle = ""
if ($StubJsRuntime) {
    $stubBundle = Join-Path $logRoot "rust-only-$stamp"
    New-Item -ItemType Directory -Force -Path $stubBundle | Out-Null
    Copy-Item -Path (Join-Path $bundle "*") -Destination $stubBundle -Recurse -Force
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot "webgl-rust-only-stub.js") `
        -Destination (Join-Path $stubBundle "web-runtime.js") -Force
    $bundle = $stubBundle
    Write-Log "rust-only host: web-runtime.js 已换成替身（JS 世界不启动），产物副本=$stubBundle" Cyan
}

Write-Log "==> WebGL bundle verification (bundle=$bundle)" Cyan

# ---------- 1. 产物完整性 ----------
$required = @(
    "index.html",
    "web-runtime.js",
    "pkg\game_web.js",
    "pkg\game_web_bg.wasm",
    "pack\manifest.txt",
    "pack\project.toml"
)
foreach ($item in $required) {
    $path = Join-Path $bundle $item
    if (-not (Test-Path -LiteralPath $path)) { throw "WebGL bundle is missing: $item" }
    if ((Get-Item -LiteralPath $path).Length -le 0) { throw "WebGL bundle entry is empty: $item" }
}
# WP10 之后 webgl 不随包带 fonts/（`__FONT_PATH__` 为空，走浏览器系统字体渲染中文）。
# 若哪天恢复随包子集字体，再在这里加回校验。

# wasm 魔数（\0asm）：产物必须是真 wasm，不是占位文件。
$wasmPath = Join-Path $bundle "pkg\game_web_bg.wasm"
if (-not (Test-Path -LiteralPath $wasmPath)) { throw "wasm module missing: $wasmPath" }
$wasmBytes = [System.IO.File]::ReadAllBytes($wasmPath)
$magic = -join ($wasmBytes[0..3] | ForEach-Object { [char]$_ })
if ($magic -ne "`0asm") { throw "pkg/game_web_bg.wasm is not a wasm module (magic=$([int[]]$wasmBytes[0..3] -join ','))" }
Write-Log "wasm module ok: $([math]::Round($wasmBytes.Length / 1024, 1)) KB, magic=$magic"

# ---------- 2. 浏览器与静态服务器 ----------
$browser = $null
foreach ($candidate in @(
        (Join-Path ${env:ProgramFiles(x86)} "Microsoft\Edge\Application\msedge.exe"),
        (Join-Path $env:ProgramFiles "Microsoft\Edge\Application\msedge.exe"),
        (Join-Path $env:ProgramFiles "Google\Chrome\Application\chrome.exe"),
        (Join-Path ${env:ProgramFiles(x86)} "Google\Chrome\Application\chrome.exe")
    )) {
    if ($candidate -and (Test-Path -LiteralPath $candidate)) { $browser = $candidate; break }
}
if (-not $browser) {
    Write-Log "no supported browser found (Edge/Chrome); bundle boot check cannot run here" Yellow
    Exit-With 2
}
$node = Get-Command node -ErrorAction SilentlyContinue
if (-not $node) {
    Write-Log "node.js not found; static server for the boot check is unavailable" Yellow
    Exit-With 2
}

$server = Start-Process -FilePath $node.Source `
    -ArgumentList @((Join-Path $PSScriptRoot "serve-static.js"), $bundle, "$Port") `
    -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $logRoot "bundle-serve-$stamp.log") `
    -RedirectStandardError (Join-Path $logRoot "bundle-serve-$stamp.log.err")
Start-Sleep -Milliseconds 1200

$queryBits = @()
if ($Drive -and $DriveJhyt) {
    throw "参数互斥：-Drive 与 -DriveJhyt 只能二选一（两者都写 data-rustlua-drive）"
}
if ($Drive) { $queryBits += "rustlua-drive=auto" }
if ($DriveJhyt) {
    $queryBits += "rustlua-drive=jhyt"
    # 剧本驱动走标题确认 → 真实场景切换全流程，首帧重载当前场景（scene-verify）会干扰它。
    $NoSceneVerify = $true
}
if (-not $NoSceneVerify) { $queryBits += "rustlua-scene-verify=1" }
# M73-00 WP5 第五片：GPU 渲染路径（引擎渲染器直接画到画布）**只在显式要求时**开启。
# Rust 侧读这个参数（`game_web` 的 `gpu_requested`），成败写进 `data-rustlua-gpu`——
# 这样不必去改 `plugins/webgl/` 下的既有脚本，默认路径也一行不动。
if ($Gpu2d) { $queryBits += "rustlua-gpu=1" }
# 帧预算：只有显式给了才拼进 URL（`-1` = 不给，保持既有默认路径一行不变）。
if ($GpuFrames -ge 0) { $queryBits += "rustlua-gpu-frames=$GpuFrames" }
# 输入剧本里的 `,`/`:` 在 URL 里必须编码，否则会被当成 query 分隔符截断。
if ($InputScript -ne "") { $queryBits += "rustlua-input-script=$([uri]::EscapeDataString($InputScript))" }
if ($WorldExpr -ne "") { $queryBits += "rustlua-world-expr=$([uri]::EscapeDataString($WorldExpr))" }
if ($WorldDo -ne "") { $queryBits += "rustlua-world-do=$([uri]::EscapeDataString($WorldDo))" }
if ($SimScriptLines -gt 0) { $queryBits += "rustlua-sim-lines=$SimScriptLines" }
if ($SimRoute) {
    if ($SimRoute -notmatch '^(script|scene-script|scene-script-noroot)$') { throw "-SimRoute 只接受 script / scene-script / scene-script-noroot：$SimRoute" }
    $queryBits += "rustlua-sim=$SimRoute"
}
# M73-00 WP3 第三片：让 Rust 侧运行时场景**换一个场景**来起（默认用工程的启动场景）。
# 为什么要它：这个工程的 `scenes/main.scene` 是"主入口门面"——只有 meta、**零实体**，
# 画面内容全由 Lua 现造。所以"引擎能不能画出工程的真场景"必须拿**有实体的**场景去问，
# 否则零实体是一个正确但什么也证明不了的读数。
if ($SimScene) {
    if ($SimScene -notmatch '^[A-Za-z0-9_/-]+$') { throw "-SimScene 只接受场景名（字母/数字/下划线/斜杠/短横线）：$SimScene" }
    $queryBits += "rustlua-sim-scene=$SimScene"
}
# 画面自检（data-rustlua-drawn）走全画布像素回读，发布默认关闭（见 web-runtime.js 的
# DIAGNOSTICS_ON）：验收要读这个数，就必须显式打开。这对**产品行为零影响**——
# 它只决定"要不要为了给验收看而每 30 帧读一次像素"。
$queryBits += "rustlua-diagnostics=1"
$url = "http://127.0.0.1:$Port/index.html"
if ($queryBits.Count -gt 0) { $url += "?" + ($queryBits -join "&") }
$domPath = Join-Path $logRoot "bundle-dom-$stamp.html"
$errPath = Join-Path $logRoot "bundle-dom-$stamp.err"

function Invoke-HeadlessDom([string]$HeadlessFlag, [string]$ModeOverride = "", [string]$OutFile = "") {
    # CDP 轮询替代 `--dump-dom`：dump-dom 在页面 load 事件后**立即** dump，而本游戏在
    # module 顶层 await 里加载 wasm + boot（`rustluaRunLuaGame`），dump 常常早于 boot
    # （body 还是 data-rustlua-boot="pending"）。用 `--remote-debugging-port` + Node 轮询
    # 真实时钟，等状态真正落定再 dump——常规 bundle 等 boot，scene-verify 等 scene 落定，
    # 剧本门等 drive 落定。时长由 $TimeoutSeconds 控制，超时判红（与 dump-dom 同语义）。
    $cdpScript = Join-Path $PSScriptRoot "webgl-cdp-wait.js"
    if (-not (Test-Path -LiteralPath $cdpScript)) {
        throw "CDP wait script missing: $cdpScript"
    }
    $cdpPort = 9200 + (Get-Random -Maximum 700)
    $profileDir = Join-Path $logRoot "cdp-profile-$stamp-$([guid]::NewGuid().ToString('N').Substring(0, 8))"
    $waitMode = if ($ModeOverride) { $ModeOverride } elseif ($DriveJhyt) { "drive" } elseif (-not $NoSceneVerify) { "scene" } else { "boot" }
    # contract 模式写的是**对拍报告**而不是 DOM，所以输出目标可由调用方指定。
    $outTarget = if ([string]::IsNullOrWhiteSpace($OutFile)) { $domPath } else { $OutFile }
    $arguments = @(
        $HeadlessFlag,
        "--disable-gpu-sandbox",
        "--no-sandbox",
        "--disable-extensions",
        # 转发 console.*（含 wasm panic hook 的 console.error）到 stderr（CONSOLE 前缀），
        # 排查 wasm trap（如场景切换 unreachable）时能从 bundle-dom-*.err 拿到 Rust panic 详情。
        "--enable-logging=stderr",
        "--v=0",
        # 关键：无头下**不要**加 `--virtual-time-budget`——虚拟时间预算让 rAF 在页面稳定后
        # 停止调度（实测 90s 只跑 63 帧、剧本永远停在某个阶段）。用真实时钟跑，配合下面
        # 这个标志强制每帧走完整合成管线，rAF 保持 ~55fps——完整玩法剧本（drive=jhyt）
        # 才能跑完并落定 done（实测 30s 推进 1635 帧、3.6 秒剧本一次全通）。
        "--run-all-compositor-stages-before-draw",
        # 音频后端的验收读数要能显示"真的在放"，而不是"被浏览器拦住"。无头实例没有用户
        # 手势，Chrome 默认自动播放策略会把 HTMLAudioElement.play() 直接 reject（实测不加
        # 这条时 `audio: kind=browser created=1 played=0 gated=1`——那不是回归，是策略）。
        # 这里放行策略，门验的是**我们这侧**的链路（src 解析 → 下载 → 解码 → currentTime
        # 前进）；策略本身的行为由 runtime 的手势补播路径负责，浏览器真机人工试听。
        "--autoplay-policy=no-user-gesture-required",
        "--remote-debugging-port=$cdpPort",
        "--user-data-dir=$profileDir",
        $url
    )
    $process = Start-Process -FilePath $browser -ArgumentList $arguments -PassThru -NoNewWindow `
        -RedirectStandardOutput $domPath -RedirectStandardError $errPath
    # **必须接住** node 的 stdout：PS 里函数内未被捕获的输出会并进返回值，于是
    # Invoke-HeadlessDom 返回的是「[cdp] 那几行 + 文件正文」的**数组**。而 `-match` 作用在
    # 数组上返回的是"匹配到的元素数组"、**不填 $Matches**（判空时却照样真值），于是
    # `$contractOutput -match 'mismatches=(\d+)'` 之后 `$Matches[1]` 还是上一条无关 -match
    # 的捕获组——实测红字里出现过 `verdict=failed(scene=main entities=17) cases=0`：
    # 红绿方向侥幸没错，但 mismatches 与 cases 两个数都是假的（$dom 那条同理，只是它被
    # 下面那句 `[string]$dom` 顺手压平了才没出事）。node 那几行本来就只写给控制台，
    # 真正的结论在 $outTarget 报告文件里，所以这里接住不转印。
    # 可选参数用 `-` 占位，不能传空串：PowerShell 向原生程序传参时会**丢掉空字符串**，
    # 后面所有参数就会错位一格（实测踩到：`-KeyProbe` 的 `1` 落到了 shot 的位置，探针静默不跑）。
    $shotArg = if ($Shot) { $Shot } else { "-" }
    $underArg = if ($ShotUnder) { $ShotUnder } else { "-" }
    $probeArg = if ($KeyProbe) { "1" } else { "-" }
    # M73-00 WP6-06：确定性剧本/世界查询要读到**落定之后**的读数。
    # 没有它，"模式何时落定"取决于浏览器调度 ⇒ 读到哪一帧是碰运气（本轮实测同一条命令两次分别读到
    # frames=30 与 frames=240，前者正好在按 `i` 的那一刻，于是门把"还没生效"误判成"没生效"）。
    # 只在开了剧本/查询时才等，其余路径一个字节的行为都不变。
    $settleArg = if ($InputScript -ne "" -or $WorldExpr -ne "" -or $WorldDo -ne "" -or $StorageProbe -or $AudioProbe -ne "") { "200" } else { "-" }
    $probeStorageArg = if ($StorageProbe) { "1" } else { "-" }
    $probeAudioArg = if ($AudioProbe -ne "") { $AudioProbe } else { "-" }
    $cdpEcho = @(& node $cdpScript $cdpPort $outTarget $waitMode $TimeoutSeconds $shotArg $underArg $probeArg $settleArg $probeStorageArg $probeAudioArg)
    $cdpExit = $LASTEXITCODE
    foreach ($cdpLine in $cdpEcho) { Write-Log "cdp: $cdpLine" }
    # 收尾：杀本脚本启动的浏览器**全部进程** + 清理 CDP profile（无头实例不会自己退出）。
    # 按唯一 user-data-dir 路径匹配命令行，精确杀本实例的进程树——绝不按进程名一网打尽
    # （开发机上还有用户自己的浏览器）；taskkill /T 对 Edge 的启动器/子进程不可靠。
    Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -in @("msedge.exe", "chrome.exe") -and $_.CommandLine -like "*$profileDir*" } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    Remove-Item -Recurse -Force $profileDir -ErrorAction SilentlyContinue
    if ($cdpExit -eq 2) {
        Write-Log "CDP unavailable (chrome did not expose debug endpoint); no DOM dump" Yellow
    }
    # M73-00 WP6-07/08：槽探针与音频探针的失败走各自退出码（等待器里查的
    # localStorage / 页面里的音频元素），在这里翻成明确红字——不然它们会被当成
    # "页面没起来"混进别的失败里。
    if ($cdpExit -eq 4) {
        Write-Log "webgl_bundle_audio_probe_failed 浏览器侧音频元素数与期望不符（见上面 audio probe 行）" Red
        Exit-With 1
    }
    if ($cdpExit -eq 3) {
        Write-Log "webgl_bundle_save_slot_failed 平台存档槽里没有可读的存档（见上面 storage probe 行）" Red
        Exit-With 1
    }
    if (Test-Path -LiteralPath $outTarget) { return Get-Content -LiteralPath $outTarget -Raw } else { return "" }
}

$dom = Invoke-HeadlessDom "--headless"
if (-not $dom -or $dom -notmatch "data-rustlua-boot") {
    $dom = Invoke-HeadlessDom "--headless=new"
}
# L3 行为契约（-ApiContract）：boot 那次验完了，这里**另开一次**无头实例专跑契约——
# 契约要 CDP 在同一个页面上下文里 `lua_do_string` + `lua_call_global`，浏览器句柄跨不过去。
# 必须在静态服务器关掉之前跑；boot 都没起来时不必浪费第二次 90s（那次红已经说明问题）。
$contractReportPath = Join-Path $logRoot "bundle-api-contract-$stamp.txt"
$contractOutput = ""
if ($ApiContract -and $dom) {
    $contractOutput = Invoke-HeadlessDom "--headless" "contract" $contractReportPath
}
Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue

if (-not $dom) {
    Write-Log "headless browser produced no DOM dump (see $errPath)" Red
    Exit-With 2
}

# ---------- 3. 断言 ----------
# CDP dump 文件为 UTF-8 无 BOM（Node 写入），PS 5.1 的 Get-Content 按 ANSI 读可能引入
# 乱码/空行；且 -Raw 对空文件返回 $null、对含空行文件返回非空串。这里统一强制成
# 单字符串再解析，避免"空数组/空串在 -match 下 $Matches 为 null 再取 [1]"这类抛错。
$dom = [string]$dom
$boot = if ($dom -match 'data-rustlua-boot="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "absent" }
$detail = if ($dom -match 'data-rustlua-boot-detail="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "" }
$statusText = if ($dom -match '<div id="status">([^<]*)</div>' -and $Matches.Count -gt 0) { $Matches[1] } else { "" }
# 缺失宿主 API 明细：`detail` 里是 `… [missing engine host API: a, b, c]`。单独导出一个
# **数值**字段给验收样例读（JSON 里逗号与转义引号都会打乱"按字段切割"的朴素解析）。
$missingNames = @()
if ($detail -match 'missing engine host API:\s*([^\]]*)' -and $Matches.Count -gt 0) {
    $missingNames = @($Matches[1].Split(',') | ForEach-Object { $_.Trim() } | Where-Object { $_ })
}
# 画面自检：`data-rustlua-drawn` 是运行时对 canvas 采样得到的“非背景像素采样点”数量。
# `-1` = 像素不可读（跨域贴图污染），`0` = 启动了但真的什么都没画出来。
$drawnRaw = if ($dom -match 'data-rustlua-drawn="(-?\d+)"' -and $Matches.Count -gt 0) { [int]$Matches[1] } else { $null }
$uiElements = if ($dom -match 'data-rustlua-ui-elements="(\d+)"' -and $Matches.Count -gt 0) { [int]$Matches[1] } else { $null }
# 世界状态自报：`scene=/map=/alive=/player=@x,y/clip`（+ 渲染侧 `drawn=`）。
# 这里只作为**证据**记录，不新增判定：无头浏览器这一次是"打开页面等 15s 后 dump DOM"，
# 停在启动场景是正常的（玩家要按键才进玩法地图）。真正有牙的那一组断言在
# `scripts/wechat-minigame-harness.js --drive-new-game ...` 里——两个 web 平台共用同一份
# `web-runtime.js`，字段含义完全一致，所以这里读到的世界状态可以直接对照它。
$world = if ($dom -match 'data-rustlua-world="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "" }
# 瓦片层自对账：`tilemaps=画/声明` 是运行时每帧写的两个**独立来源**读数（渲染循环实际遍历的
# 层数 / Lua 侧注册表里的层数）。它抓的是"overlay 层整层没进画面"这一类静默差异——山庄的
# FarmGrid 种田层曾被"只取第一张 Tilemap"的老写法丢掉，PC 有、web 无（实机 2026-10-04）。
# 字段缺失（产物是旧运行时、或这条自报被删）同样算红：世界自报都在，读数却少一项，
# 说明门与产物错位，不能降级成"没数据所以放行"。
$tileDrawn = $null
$tileDeclared = $null
if ($world -match 'tilemaps=(\d+)/(\d+)' -and $Matches.Count -gt 1) {
    $tileDrawn = [int]$Matches[1]
    $tileDeclared = [int]$Matches[2]
}
# P0-①：`?rustlua-drive=auto` 的剧本结果。`done` = 确认 → 进玩法地图 → 位移非零全部发生；
# `failed` 时 detail 给出卡在哪一步（场景没切换 / 没有玩家位置 / 位移为 0）。
$driveState = if ($dom -match 'data-rustlua-drive="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "absent" }
$driveStateDetail = if ($dom -match 'data-rustlua-drive-detail="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "" }
# 场景切换防回归门：`?rustlua-scene-verify=1` 让首帧把当前场景重新走一遍 applySceneSwitch
# 全流程（fetch → parse → 重绑 Lua → 重跑 on_create）。done=通过、failed=细节给出错误、
# in-flight=在途未落定（通常说明 headless 虚拟时间预算在切换 promise resolve 前烧完）、
# absent=URL 门没接上或 runtime 未升级。后两种都算失败。
$sceneVerifyState = if ($dom -match 'data-rustlua-scene-verify="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "absent" }
# M73-00 WP5 第五片：GPU 渲染路径的自报状态（`?rustlua-gpu=1` 时由 `game_web` 的 GPU 宿主写）。
# `presented frames=…` = 引擎渲染器真的把帧画到画布上并 present 了；`failed: …` 给出卡在哪一步；
# `absent` = 产物没编进 GPU 宿主（缺 `package.ps1 -Gpu2d`）或 URL 门没接上——两种情况都要能分辨。
$gpuState = if ($dom -match 'data-rustlua-gpu="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "absent" }
$sceneVerifyDetail = if ($dom -match 'data-rustlua-scene-verify-detail="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "" }
# 音频后端读数（`data-rustlua-audio`，由 web-runtime.js 的 syncAudioMarker 写）：**只记录、
# 不判定**，与微信 harness 的 audio_ctx/audio_played 同口径。absent = 产物还是旧版运行时
# （后端接缝没落地）；`kind=none` = 后端没装上；停在启动场景时 created 至少为 1（标题 BGM）。
$audioEvidence = if ($dom -match 'data-rustlua-audio="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "absent" }

# 预设文字自对账：`data-rustlua-prefab-text=画/应有` 由 web-runtime.js 每帧写，只统计
# **运行期 prefab 实例**里带非空 Text 的实体（画的那一半是实际走满 fillText 的）。
# 为什么单独要这一对读数：`ui-elements`/`ui-now` 数的是 Lua 每帧 `ui.*` 的动态行，屏幕 UI
# 实体一个都不计入，所以"面板只剩暗底、正文整批不画"在它上面是静默的（实测 2026-10-07
# 修复前后 drawn=5713 / ui elements=34 逐位相同）。字段缺失同样算红：产物与门错位。
$prefabTextDrawn = $null
$prefabTextTotal = $null
if ($dom -match 'data-rustlua-prefab-text="(\d+)/(\d+)"' -and $Matches.Count -gt 1) {
    $prefabTextDrawn = [int]$Matches[1]
    $prefabTextTotal = [int]$Matches[2]
}

# 预设九宫格自报（data-rustlua-prefab-slice=切片次数/该切实体数）。原生把 Image.nine_slice
# 拆成 3×3 格（mesh_ui.rs:727），web 侧曾解析完就没人读、整张拉伸：面板边框吹糊、卡片压扁
# （实机 2026-10-07）。这条断"切没切"：没切时 cells == 实体数（每实体一次 drawImage）。
$prefabSliceCells = $null
$prefabSliceEntities = $null
if ($dom -match 'data-rustlua-prefab-slice="(\d+)/(\d+)"' -and $Matches.Count -gt 1) {
    $prefabSliceCells = [int]$Matches[1]
    $prefabSliceEntities = [int]$Matches[2]
}

# L3 行为契约结论。报告首行三选一：`web_api_contract_ok cases=N` /
# `web_api_contract_failed mismatches=N cases=N` / `web_api_contract_unavailable <原因>`。
# **找不到这三行也必须算红**：那次 node 调用超时时把 DOM 写进了同一个文件，
# 把它当"没数据所以放行"就等于给门开了个洞。反过来，格式对不上（比如产物还是旧版
# cdp 脚本、失败行没有 cases）也一律红，不静默降级成"只报 mismatches"。
$contractVerdict = "not-requested"
$contractCases = 0
if ($ApiContract) {
    if ($contractOutput -match 'web_api_contract_ok cases=(\d+)' -and $Matches.Count -gt 0) {
        $contractVerdict = "ok"; $contractCases = [int]$Matches[1]
    } elseif ($contractOutput -match 'web_api_contract_failed mismatches=(\d+) cases=(\d+)' -and $Matches.Count -gt 1) {
        $contractVerdict = "failed(" + $Matches[1] + ")"; $contractCases = [int]$Matches[2]
    } else {
        $contractVerdict = "unavailable"
    }
}

$summary = [ordered]@{
    task           = "M31"
    bundle         = $bundle
    url            = $url
    browser        = $browser
    wasmBytes      = $wasmBytes.Length
    boot           = $boot
    detail         = $detail
    missingCount   = $missingNames.Count
    missingHostApi = ($missingNames -join ';')
    statusText     = $statusText
    drawnSamples   = $drawnRaw
    uiElements     = $uiElements
    world          = $world
    tileLayers     = if ($null -eq $tileDrawn) { "absent" } else { "$tileDrawn/$tileDeclared" }
    drive          = $driveState
    driveDetail    = $driveStateDetail
    driveJhyt      = if ($DriveJhyt) { $driveState } else { "not-requested" }
    driveJhytDetail = if ($DriveJhyt) { $driveStateDetail } else { "" }
    sceneVerify    = $sceneVerifyState
    sceneVerifyDetail = $sceneVerifyDetail
    audio          = $audioEvidence
    apiContract    = $contractVerdict
    apiContractCases = $contractCases
    apiContractReport = $contractReportPath
    domDump        = $domPath
    log            = $logPath
}
$summary | ConvertTo-Json | Set-Content -Path (Join-Path $logRoot "bundle-summary-$stamp.json") -Encoding UTF8

Write-Log "boot=$boot missing_host_api=$($missingNames.Count) detail=$detail" $(if ($boot -eq "ok") { "Green" } else { "Yellow" })
Write-Log "status text: $statusText"
Write-Log "drawn samples: $(if ($null -eq $drawnRaw) { 'unknown' } else { $drawnRaw }) ui elements: $(if ($null -eq $uiElements) { 'unknown' } else { $uiElements })"
Write-Log "world: $(if ([string]::IsNullOrWhiteSpace($world)) { '(未自报)' } else { $world })"
Write-Log "tile_layers: $(if ($null -eq $tileDrawn) { 'absent' } else { "$tileDrawn/$tileDeclared" })" `
    $(if ($null -eq $tileDrawn) { "Yellow" } elseif ($tileDrawn -ne $tileDeclared) { "Red" } else { "Green" })
if ($Drive) {
    Write-Log "drive: $driveState detail: $driveStateDetail" $(if ($driveState -eq "done") { "Green" } else { "Yellow" })
}
if ($DriveJhyt) {
    Write-Log "drive_jhyt: $driveState detail: $driveStateDetail" $(if ($driveState -eq "done") { "Green" } else { "Yellow" })
}
if (-not $NoSceneVerify) {
    Write-Log "scene_verify: $sceneVerifyState detail: $sceneVerifyDetail" $(if ($sceneVerifyState -eq "done") { "Green" } else { "Yellow" })
}
Write-Log "audio: $audioEvidence" $(if ($audioEvidence -eq "absent") { "Yellow" } else { "Green" })
Write-Log "prefab_text: $(if ($null -eq $prefabTextDrawn) { 'absent' } else { "$prefabTextDrawn/$prefabTextTotal" })" `
    $(if ($null -eq $prefabTextDrawn) { "Yellow" } elseif ($prefabTextDrawn -ne $prefabTextTotal) { "Red" } else { "Green" })
Write-Log "prefab_slice: $(if ($null -eq $prefabSliceCells) { 'absent' } else { "$prefabSliceCells/$prefabSliceEntities" })" `
    $(if ($null -eq $prefabSliceCells) { "Yellow" } elseif ($prefabSliceEntities -gt 0 -and $prefabSliceCells -lt ($prefabSliceEntities * 2)) { "Red" } else { "Green" })
if ($ApiContract) {
    Write-Log "api_contract: $contractVerdict cases=$contractCases report=$contractReportPath" `
        $(if ($contractVerdict -eq "ok") { "Green" } else { "Red" })
}
Write-Log "dom: $domPath"

if ($drawnRaw -eq 0) {
    # 启动成功但没有画面：这正是"页面能跑却黑屏"的典型症状，必须显式失败。
    Write-Log "webgl_bundle_blank boot=$boot detail=canvas has no painted pixels" Red
    Exit-With 1
}

if (-not [string]::IsNullOrWhiteSpace($world)) {
    if ($null -eq $tileDrawn) {
        # 世界自报在、读数缺一项 = 运行时与门版本错位（字段被删或产物是旧版），不静默放行。
        Write-Log "webgl_bundle_tile_layers_missing detail=no tilemaps= field in world report world=$world" Red
        Exit-With 1
    }
    if ($tileDrawn -ne $tileDeclared) {
        # 渲染循环遍历的层数 ≠ Lua 注册表里的层数 ⇒ 有一整层瓦片根本没进画面（种田/浇水
        # 这类 overlay 会表现为"数据在动、画面不变"）。
        Write-Log "webgl_bundle_tile_layers_mismatch drawn=$tileDrawn declared=$tileDeclared world=$world" Red
        Exit-With 1
    }
    if (($Drive -or $DriveJhyt) -and $tileDeclared -lt 1) {
        # 剧本已经进了玩法地图，那里至少该有一张地表瓦片层；0 层 = 场景没带瓦片或换图没落地。
        Write-Log "webgl_bundle_tile_layers_empty detail=gameplay scene has no tilemap layer world=$world" Red
        Exit-With 1
    }
}

# 预设文字门。放在 drive 判定**之前**：drive 现在本来就是红的（nav-exit），排在它后面这条
# 断言永远轮不到跑，就成了摆设。
if (-not [string]::IsNullOrWhiteSpace($world)) {
    if ($null -eq $prefabTextDrawn) {
        Write-Log "webgl_bundle_prefab_text_missing detail=no data-rustlua-prefab-text in dom world=$world" Red
        Exit-With 1
    }
    if ($prefabTextDrawn -ne $prefabTextTotal) {
        # 预设实例声明了正文却没画满 = 面板只剩暗底（换场景后 sceneTextSuppressed 误伤
        # prefab 实例就是这一类：set_text 返回 true，屏幕上一个字都没有）。
        Write-Log "webgl_bundle_prefab_text_suppressed drawn=$prefabTextDrawn expected=$prefabTextTotal world=$world" Red
        Exit-With 1
    }
    if ($DriveJhyt -and $prefabTextTotal -lt 1) {
        # 剧本已进玩法地图，HUD 预设每帧都在写 HudLine/目标/提示，应有数不可能是 0。
        Write-Log "webgl_bundle_prefab_text_empty detail=gameplay scene has no prefab text world=$world" Red
        Exit-With 1
    }
    if ($null -eq $prefabSliceCells) {
        Write-Log "webgl_bundle_prefab_slice_missing detail=no data-rustlua-prefab-slice in dom world=$world" Red
        Exit-With 1
    }
    if ($prefabSliceEntities -gt 0 -and $prefabSliceCells -lt ($prefabSliceEntities * 2)) {
        # 声明了 nine_slice 却每实体只发一次 drawImage = 整张拉伸，边框与原生不一致。
        Write-Log "webgl_bundle_prefab_slice_flat cells=$prefabSliceCells entities=$prefabSliceEntities world=$world" Red
        Exit-With 1
    }
    if ($DriveJhyt -and $prefabSliceEntities -lt 1) {
        # 玩法场景的 HudSkillCard1..8 声明了 nine_slice=8,8,8,8，走切片的实体数不可能是 0。
        Write-Log "webgl_bundle_prefab_slice_empty detail=gameplay scene draws no nine_slice world=$world" Red
        Exit-With 1
    }
}

if ($Drive -and $driveState -ne "done") {
    # P0-①：请求了"完整剧本"却没跑完——无论 boot 是否 ok，都是验收失败。
    Write-Log "webgl_bundle_drive_failed boot=$boot drive=$driveState detail=$driveStateDetail world=$world" Red
    Exit-With 1
}

if ($DriveJhyt -and $driveState -ne "done") {
    # P0-②：请求了"完整玩法剧本"却没跑完（卡在某个里程碑或超时）——无论 boot 是否 ok，都是验收失败。
    Write-Log "webgl_bundle_drivejhyt_failed boot=$boot drive=$driveState detail=$driveStateDetail world=$world" Red
    Exit-With 1
}

# M73-00 WP5 第五片：GPU 自检放**场景切换门之前**。理由是实测的：GPU 画布若抢走 2D 上下文，
# 先红的是场景切换门（`boot=failed detail=2D canvas context is not available`），而根因在 GPU
# 那一侧——两条读数必须同框出现，否则这份日志会把人指向错误的文件。
if ($Gpu2d -and $gpuState -notlike "presented*") {
    # `absent` 的头号原因是产物没用 `package.ps1 -Gpu2d` 构建（GPU 宿主没编进去）——这种情况
    # 必须说清楚，否则会被读成"引擎渲染器在浏览器里画不出来"。
    $hint = if ($gpuState -eq "absent") {
        "（产物里没有 GPU 宿主：需要用 scripts/package.ps1 -Gpu2d 构建）"
    } else { "" }
    Write-Log "webgl_bundle_gpu2d_failed boot=$boot gpu=$gpuState $hint" Red
    Exit-With 1
}

# M73-00 WP0 续五：GPU 层的几何必须**落在 Canvas2D 层的等比适配盒里**。
#
# 为什么值得一条门：错法本身完全不可见——固定 1280×720 + CSS 铺满视口时，
# 属性读数（`frames=`/`draws=`/`avg_*_ms=`）一个都不异常，画面却在非 16:9 窗口里被非等比拉伸。
# 实测窗口是 756×441（不是 16:9），旧行为下 GPU 内容被纵向拉伸约 7%，两层画的是两个不同的盒子。
# 这里只断言"盒子是 DesignWidth:DesignHeight 的比例、且不超出游戏画布"，不去钉死具体像素
# （无头窗口尺寸会变）；比例如不对、或超出了画布，就是回归。
if ($Gpu2d -and $gpuState -like "presented*") {
    $fitOk = $false
    $fitDetail = "missing"
    $fitMatch = [regex]::Match($gpuState, 'viewport=(\d+)x(\d+) fit=(\d+)x(\d+)')
    if ($fitMatch.Success) {
        $vw = [int]$fitMatch.Groups[1].Value
        $vh = [int]$fitMatch.Groups[2].Value
        $fw = [int]$fitMatch.Groups[3].Value
        $fh = [int]$fitMatch.Groups[4].Value
        $wantRatio = 1280.0 / 720.0
        $gotRatio = if ($fh -gt 0) { $fw / [double]$fh } else { 0.0 }
        $fitDetail = "viewport=${vw}x${vh} fit=${fw}x${fh} ratio=$([math]::Round($gotRatio, 4))"
        # 1% 容差 = 整数像素取整（756×425 相对 1.7778 差 0.0004；钉死具体像素反而会假红）。
        $fitOk = ($fw -gt 0) -and ($fh -gt 0) -and ($fw -le $vw) -and ($fh -le $vh) -and `
            ([math]::Abs($gotRatio - $wantRatio) -lt ($wantRatio * 0.01))
    }
    if (-not $fitOk) {
        Write-Log "webgl_bundle_gpu_fit_failed gpu=$gpuState fit=$fitDetail" Red
        Exit-With 1
    }
    Write-Log "webgl_bundle_gpu_fit_ok $fitDetail" Green
}

# M73-00 WP6-05/06：输入 → 世界改变的**门**（只在显式给了确定性输入剧本时开）。
#
# 为什么必须让驱动自己断言、而不是"人去 dump 里看一眼"：`input_edges` 只证明边沿进了快照，
# 证明不了游戏逻辑真的因此动了什么——这两件事之间隔着整个工程脚本。读数由 Rust 侧
# 从引擎自己的世界（`entity.find` + `transform.get`）与场景输出（`scene_delta=`）读出，
# 与 JS 世界无关；再配上按帧号触发的 `<键>:<起帧>-<止帧>` 剧本，这个门就是**确定性**的。
#
# 两种期望：
#   * 默认（不给 -ExpectWorld）＝**位移门**：按住的键必须让主角位移非零（`moved=`）。
#   * `-ExpectWorld <正则>`＝**自定义门**：断言整条读数匹配，用于"按一下改变场景输出"这类
#     非移动交互（如 `-InputScript i:30-33 -ExpectWorld 'scene_delta=\+[1-9]'`＝开背包面板）。
if (($InputScript -ne "" -or $WorldExpr -ne "" -or $WorldDo -ne "") -and $gpuState -like "presented*") {
    if ($ExpectWorld -ne "") {
        $hit = [regex]::Match($gpuState, $ExpectWorld)
        if (-not $hit.Success) {
            Write-Log "webgl_bundle_rust_input_failed script=$InputScript expect=<$ExpectWorld> state=$gpuState" Red
            Exit-With 1
        }
        Write-Log "webgl_bundle_rust_input_ok script=$InputScript expect=<$ExpectWorld> match=<$($hit.Value)>" Green
    } else {
        $movedOk = $false
        $movedDetail = "missing"
        $found = [regex]::Matches($gpuState, '([A-Za-z_]\w*)@(-?\d+),(-?\d+) moved=(-?\d+),(-?\d+)')
        foreach ($m in $found) {
            $dx = [int]$m.Groups[4].Value
            $dy = [int]$m.Groups[5].Value
            $movedDetail = "$($m.Groups[1].Value)@$($m.Groups[2].Value),$($m.Groups[3].Value) moved=${dx},${dy}"
            if ($dx -ne 0 -or $dy -ne 0) { $movedOk = $true; break }
        }
        if ($found.Count -eq 0) { $movedDetail = "world=no-player（场景里没有名字含 player 的实体？）" }
        if (-not $movedOk) {
            Write-Log "webgl_bundle_rust_input_failed script=$InputScript world=$movedDetail" Red
            Exit-With 1
        }
        Write-Log "webgl_bundle_rust_input_ok script=$InputScript world=$movedDetail" Green
    }
}

if (-not $NoSceneVerify -and $sceneVerifyState -ne "done") {
    # 场景切换防回归门：boot 能过、场景一切就死（wasm 宿主闭包 raise 撞 panic=abort 的现场
    # 症状）就是这类问题。absent/in-flight/failed 三种都算失败，因为都意味着
    # applySceneSwitch 的 fetch → parse → on_create 全流程没有干净跑完。
    # 带 `-Gpu2d` 时把 GPU 自报一起打出来：两者同框才判得清是谁把谁弄死的。
    Write-Log "webgl_bundle_scene_verify_failed boot=$boot scene_verify=$sceneVerifyState detail=$sceneVerifyDetail world=$world gpu=$gpuState" Red
    Exit-With 1
}

if ($ApiContract -and $contractVerdict -ne "ok") {
    # L3：桌面与 web（**打完宿主补丁**的活体运行时）行为不一致。这正是"第二套实现"最容易烂掉
    # 的地方，也是 docs/51 那句"按引擎源码逐条实现宿主 API"唯一能被机器检查的形式。
    # 处置顺序固定：先修实现（补丁层多半是历史遗留的死覆盖），再把确实该不同的行为从契约里
    # **删掉**并在 L2 覆盖清单说明理由；把期望改成 web 的现状 = 把门拆了。
    Write-Log "webgl_api_contract_failed verdict=$contractVerdict report=$contractReportPath" Red
    if (Test-Path -LiteralPath $contractReportPath) {
        Get-Content -LiteralPath $contractReportPath -Encoding UTF8 -TotalCount 26 |
            ForEach-Object { Write-Log "  $_" Red }
    }
    Exit-With 1
}

if ($boot -eq "ok") {
    Write-Log "webgl_bundle_ok wasm=$([math]::Round($wasmBytes.Length / 1024, 1))KB" Green
    Exit-With 0
}
if ($boot -in @("booted", "running")) {
    Write-Log "webgl_bundle_ok (booted; status text not final: $statusText)" Green
    Exit-With 0
}
Write-Log "webgl_bundle_boot_failed boot=$boot detail=$detail" Red
Exit-With 1
