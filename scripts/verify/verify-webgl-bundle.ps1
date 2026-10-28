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
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\verify\verify-webgl-bundle.ps1 `
#       [-BundleRoot examples\game_jhyt\builds\webgl] [-Port 8137] [-TimeoutSeconds 90]
#   # 关掉场景切换门（boot-only，快但不防切换崩溃回归）：
#   powershell ... -File scripts\verify\verify-webgl-bundle.ps1 -NoSceneVerify
#   # L3 行为契约对拍（PC transcript vs 活体页面跑同一份 lua_api_contract.lua，多开一次浏览器）：
#   powershell ... -File scripts\verify\verify-webgl-bundle.ps1 -ApiContract
#
# 省略 -BundleRoot 时自动发现（本机唯一的合格 webgl 产物，见 scripts\lib\bundle-discovery.ps1）：
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
    #
    # **本开关驱动的是旧 JS 世界**（`web-runtime.js` 的 `stepDriveJhyt`，`data-rustlua-drive`
    # 只有它写）。**该被试物已随 B8 删除**：今天带不带本开关都不会有写者，`data-rustlua-drive`
    # 恒 absent ⇒ 本门只能走"跳过"分支（下面 826 行附近），加 `-UrlQuery 'rustlua-js-host=1'`
    # 也救不回来（旧世界没有了）。保留开关是为了不动既有调用面与历史命令。
    # 引擎侧的等价验收通道是 `-Gpu2d -SimRoute script -InputScript …` + `-WorldExpr`（不依赖 JS 世界）。
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
    # M73-00 WP6-04：落定后真的按几下键，读"宿主收到的 DOM 事件数"与"落到模拟的边沿数"。
    # 两个数分开看才知道失败在"没收到事件"还是"收到了没应用"——Rust-only 宿主里没有页内剧本，
    # 只能用页内派发；CDP 的 `Input.dispatchKeyEvent` 在这个 headless 配置里已知进不来。
    [switch]$KeyProbe,
    # 真事件探针：页内派发**真实**的两指 `pointerdown`，验"真实输入源真的在产多指数据"。
    [switch]$PointerProbe,
    # M73-00 WP6-04：GPU 帧循环的帧预算（URL 位 `rustlua-gpu-frames=`）。0 = 不设上限。
    # 需要它的原因：默认 240 帧跑完循环就停、状态属性随之冻结，于是"按键后读读数"永远看不到变化。
    [int]$GpuFrames = -1,
    # M73-00 WP6-05：确定性输入剧本（URL 位 `rustlua-input-script=`），如 `w:30-150` 或 `axis1-:30-150`。
    # 键名驱动键盘，axis0/axis1 的 +/- 驱动手柄摇杆；给了它就额外开一道"位移非零"门。
    # 输入进去之后，Rust 侧从引擎世界读出的主角位置必须变过。
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
    [string]$StatusFile = "",
    # 追加到页面 URL 的**原始**查询串（例如 `rustlua-js-host=1`）。迁移期验收旧 JS 世界
    # （`web-runtime.js`）是否被显式叫回来时用——它默认已不再作为第二宿主启动。
    [string]$UrlQuery = "",
    # assetc 可执行文件（容器内容闭包检查用）。留空 = 按 engine\target\{release,debug}\assetc.exe 自动找。
    [string]$Assetc = ""
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
$workspaceRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
# **前端语法自检已退休**（B8）：它检的是 `plugins/webgl/web-runtime.js` —— 那个文件是**一大段嵌在
# JS 模板里的 Lua**，模板被提前闭合（Lua 注释里写反引号）或写出八进制转义时，`node --check` 会报错
# 而打包链路不查这一步（本项目重复踩过 5 次，见旧注释）。**删掉旧世界之后，仓里不再有"JS 模板里嵌
# Lua"这种形态**（补丁层就是那个模板），所以这类检查没有对象了。
. (Join-Path $PSScriptRoot "..\lib\bundle-discovery.ps1")
if ([string]::IsNullOrWhiteSpace($BundleRoot)) {
    # 不写死游戏工程：自动发现本机唯一的合格 webgl 产物（诊断由 bundle-discovery 打印）。
    $BundleRoot = Resolve-GameBundle -Platform "webgl"
    if (-not $BundleRoot) {
        Write-Host "webgl_bundle_blocked reason=no_bundle（本机没有合格 webgl 产物；先跑 scripts\build\package.ps1 -Platform webgl。exit 3 = 环境缺失，不是代码红）" -ForegroundColor Yellow
        exit 3
    }
}
if (-not (Test-Path -LiteralPath $BundleRoot)) {
    throw "WebGL bundle was not found: $BundleRoot (先跑 scripts\build\package.ps1 -Platform webgl)"
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

# **`-StubJsRuntime` 已退休**（B8）：它曾把**产物里**的 `web-runtime.js` 换成替身来证明"只有 Rust
# 也跑得起来"。现在产物里**根本没有**这个文件了（`package.ps1` 不再复制它，四处完整性清单也已去掉），
# 所以"JS 世界不启动"不再是需要构造的实验状态，而是**默认事实**；替身文件 `webgl-rust-only-stub.js`
# 一并删除。$stubBundle 保留为空串，只是因为下面还有引用它的读数分支。
$stubBundle = ""

Write-Log "==> WebGL bundle verification (bundle=$bundle)" Cyan

# ---------- 1. 产物完整性 ----------
$required = @(
    "index.html",
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

# ---------- 1b. 容器内容闭包（grouped 产物才跑） ----------
#
# 浏览器启动只证明"首屏那几条资源读得到"。grouped 产物的按需组（`unit = sub` / `cdn`）里
# 少一条资源，页面照样 boot=ok、场景门照样绿——真机玩到那一步才白图。这里按 `bundle_plan.json`
# 逐组打开 RLB 索引对账（实现只有一份：`assetc closure`，解析在 Rust 侧）。
# 放在起浏览器**之前**：这类缺陷不需要浏览器就能判，而且没有浏览器的机器上也要能拦住。
$webglPlan = @(
    (Join-Path $bundle "bundle_plan.json"),
    (Join-Path $bundle "pack\bundle_plan.json")
) | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if ($webglPlan) {
    $assetcExe = $Assetc
    if ([string]::IsNullOrWhiteSpace($assetcExe)) {
        foreach ($candidate in @(
                (Join-Path $workspaceRoot "engine\target\release\assetc.exe"),
                (Join-Path $workspaceRoot "engine\target\debug\assetc.exe")
            )) {
            if (Test-Path -LiteralPath $candidate) { $assetcExe = $candidate; break }
        }
    }
    if ([string]::IsNullOrWhiteSpace($assetcExe) -or -not (Test-Path -LiteralPath $assetcExe)) {
        throw ("容器内容闭包检查需要 assetc（找不到：先 `cargo build -p assetc --release`，或传 -Assetc）；" +
            "这一门不允许静默跳过——它正是「容器存在但内容缺件」的唯一拦截点。计划表：$webglPlan")
    }
    $savedEapClosure = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $closureOutput = & $assetcExe closure $bundle 2>&1
        $closureExit = $LASTEXITCODE
    }
    finally { $ErrorActionPreference = $savedEapClosure }
    if ($closureExit -ne 0) {
        throw ("容器内容闭包检查失败（assetc closure exit {0}）: {1}" -f $closureExit, (($closureOutput | ForEach-Object { "$_" }) -join "`n"))
    }
    Write-Log (($closureOutput | Where-Object { $_ -match '^assetc_closure_ok' } | Select-Object -First 1))
}

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

# ---- 服务闸门：**必须证明服务的就是待验产物**（M73-00 第 57 轮实测过一次静默错验） ----
#
# 实测经过：第 31 轮遗留的一个后台任务还在跑，它的 `serve-static.js` 占着**默认端口 8137**
# 服务的是 `examples\game_api\builds\webgl`；这次为正确产物起的服务**绑定失败**，而失败既没
# 被检查也没进日志判据，浏览器于是被那个残留服务应答 —— 于是"验证"的读数（`project bytes=329`
# 与 `demo_main.lua` 的报错）说的是**另一个产物**，眼看就要得出"webgl 侧红了"的错结论。
# 下面三道闸把它变成响亮的失败，而不是靠"端口号应该没别人用"这种假设：
#   ① 起服务前端口必须空闲（占用则报出占用者的命令行，连 PID 一起）；
#   ② 起服务后等它就绪；进程已退出（典型就是 EADDRINUSE）或始终不应答 ⇒ 抛，并附服务日志尾部；
#   ③ 就绪后核对一个**金丝雀文件**：HTTP 取回的字节与本地待验产物逐字节同 SHA256
#      ⇒ "服务根 == 待验产物根"由证据自证。三道闸只加断言，不放宽任何既有判据。
function Get-PortListener([int]$PortNumber) {
    $conn = Get-NetTCPConnection -LocalPort $PortNumber -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $conn) { return $null }
    $proc = Get-CimInstance Win32_Process -Filter "ProcessId=$($conn.OwningProcess)" -ErrorAction SilentlyContinue
    $cmd = if ($proc -and $proc.CommandLine) { $proc.CommandLine } else { "(命令行不可读)" }
    return [pscustomobject]@{ Pid = $conn.OwningProcess; CommandLine = $cmd }
}

$portOwner = Get-PortListener $Port
if ($portOwner) {
    throw ("端口 $Port 已被占用（PID=$($portOwner.Pid)）：$($portOwner.CommandLine)" +
        " —— 很可能是上一次运行/后台任务残留的静态服务。静默复用它会让验证读到**另一个产物**，" +
        "因此这里直接失败：先结束它，或用 -Port 指定一个空闲端口。")
}

$serveLogPath = Join-Path $logRoot "bundle-serve-$stamp.log"
$serveErrPath = Join-Path $logRoot "bundle-serve-$stamp.log.err"
$server = Start-Process -FilePath $node.Source `
    -ArgumentList @((Join-Path $PSScriptRoot "..\dev\serve-static.js"), $bundle, "$Port") `
    -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput $serveLogPath `
    -RedirectStandardError $serveErrPath

# ② 就绪等待（而不是"睡 1.2 秒然后假设它在跑"）：最多 15 秒，进程退出立即判红。
$canaryRel = $null
foreach ($candidate in @("pack/project.toml", "build-info.txt", "index.html")) {
    $localCandidate = Join-Path $bundle ($candidate -replace '/', '\')
    if (Test-Path -LiteralPath $localCandidate) { $canaryRel = $candidate; break }
}
if (-not $canaryRel) {
    throw "待验产物里找不到任何可用的金丝雀文件（pack/project.toml、build-info.txt、index.html 都不在）：$bundle"
}
$serveReady = $false
$readyDeadline = (Get-Date).AddSeconds(15)
while ((Get-Date) -lt $readyDeadline) {
    if ($server.HasExited) {
        $tail = @()
        foreach ($p in @($serveLogPath, $serveErrPath)) { if (Test-Path -LiteralPath $p) { $tail += (Get-Content -LiteralPath $p -Encoding UTF8 -ErrorAction SilentlyContinue | Select-Object -Last 6) } }
        throw ("静态服务启动后立即退出（exit=$($server.ExitCode)）——典型原因是端口被占。" +
            "服务日志尾部：`n" + ($tail -join "`n"))
    }
    try {
        $probe = Invoke-WebRequest -Uri "http://127.0.0.1:$Port/$canaryRel" -UseBasicParsing -TimeoutSec 3 -ErrorAction Stop
        if ($probe.StatusCode -eq 200) { $serveReady = $true; break }
    } catch {
        Start-Sleep -Milliseconds 250
    }
}
if (-not $serveReady) {
    $tail = @()
    foreach ($p in @($serveLogPath, $serveErrPath)) { if (Test-Path -LiteralPath $p) { $tail += (Get-Content -LiteralPath $p -Encoding UTF8 -ErrorAction SilentlyContinue | Select-Object -Last 6) } }
    throw ("静态服务在端口 $Port 上 15 秒内没有就绪（$canaryRel 取不到）。服务日志尾部：`n" + ($tail -join "`n"))
}

# ③ 身份核对：HTTP 字节 vs 本地字节，SHA256 必须相等。
$servedCanary = Join-Path $logRoot "served-canary-$stamp.bin"
Invoke-WebRequest -Uri "http://127.0.0.1:$Port/$canaryRel" -UseBasicParsing -OutFile $servedCanary -TimeoutSec 10 -ErrorAction Stop
$localHash = (Get-FileHash -LiteralPath (Join-Path $bundle ($canaryRel -replace '/', '\')) -Algorithm SHA256).Hash
$servedHash = (Get-FileHash -LiteralPath $servedCanary -Algorithm SHA256).Hash
if ($localHash -ne $servedHash) {
    throw ("服务根与待验产物**不是同一份**：$canaryRel 本地 SHA256=$localHash，HTTP 取回 SHA256=$servedHash" +
        "（服务根=$bundle，端口=$Port）——这正是「验证读到另一个产物」的形态，拒绝继续。")
}
Write-Log "serve identity ok: $canaryRel 与待验产物逐字节一致（SHA256 $($localHash.Substring(0,16))…，端口 $Port）"

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
    if ($SimRoute -notmatch '^(script|scene-only|scene-script|scene-script-noroot)$') { throw "-SimRoute 只接受 script / scene-only / scene-script / scene-script-noroot：$SimRoute" }
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
if ($UrlQuery -ne "") { $queryBits += $UrlQuery }
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
    $cdpScript = Join-Path $PSScriptRoot "..\lib\webgl-cdp-wait.js"
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
    # `-PointerProbe` 需要宿主**持续推进帧**：`-GpuFrames N` 会让宿主跑满 N 帧就**停**，探针随后派发时读数
# 必然是 0 —— 实测拿到 `frames 240->240` 配 `multipoint=0`，那是**假阴性**（宿主根本没在跑），
# 而不是"真实输入源没接通"。所以在跑之前就拒绝，别让它变成一条会骗人的红或绿。
if ($PointerProbe -and $GpuFrames -gt 0) {
    throw "-PointerProbe 不能与 -GpuFrames 同用：-GpuFrames 会让宿主在 N 帧后停帧，探针读数必然为 0（假阴性）。请去掉 -GpuFrames（或设成 0）。"
}
$probeArg = if ($KeyProbe) { "1" } else { "-" }
$probePointerArg = if ($PointerProbe) { "1" } else { "-" }
    # M73-00 WP6-06：确定性剧本/世界查询要读到**落定之后**的读数。
    # 没有它，"模式何时落定"取决于浏览器调度 ⇒ 读到哪一帧是碰运气（本轮实测同一条命令两次分别读到
    # frames=30 与 frames=240，前者正好在按 `i` 的那一刻，于是门把"还没生效"误判成"没生效"）。
    # 只在开了剧本/查询时才等，其余路径一个字节的行为都不变。
    $settleArg = if ($GpuFrames -gt 0) {
        [string]$GpuFrames
    } elseif ($InputScript -ne "" -or $WorldExpr -ne "" -or $WorldDo -ne "" -or $StorageProbe -or $AudioProbe -ne "") {
        "200"
    } else {
        "-"
    }
    $probeStorageArg = if ($StorageProbe) { "1" } else { "-" }
    $probeAudioArg = if ($AudioProbe -ne "") { $AudioProbe } else { "-" }
    $cdpEcho = @(& node $cdpScript $cdpPort $outTarget $waitMode $TimeoutSeconds $shotArg $underArg $probeArg $settleArg $probeStorageArg $probeAudioArg $probePointerArg)
    $cdpExit = $LASTEXITCODE
    foreach ($cdpLine in $cdpEcho) { Write-Log "cdp: $cdpLine" }
    # M73-00 第 58 轮：把页面**自报的宿主形态**与**自报的产物身份**留下来给门用。
    #
    # 为什么必须读它：webgl 页面**没有第二条绘制/剧本路**了（B8 删掉了旧 JS 世界），
    # 页面自己打 `[host] legacy JS world skipped (…)`，而场景门 / 剧本门 / 世界读数 /
    # 诊断像素全部**由已删的那个 JS 宿主写**。宿主不存在时这些门没有写者，把它们判红
    # 就是默认路径上一盏永远亮着的红灯——真正的回归反而会被这盏灯盖住。
    # **B8：原"请求了 JS 宿主却仍自报 skipped ⇒ 判红"那一条已退休**（见下面 `$jsHostSkipped` 处的说明）。
    $script:pageHostSkipped = [bool]($cdpEcho | Where-Object { $_ -match 'legacy JS world skipped' })
    $script:pageStartedLine = ($cdpEcho | Where-Object { $_ -match 'RustLua WebGL/WASM started' } | Select-Object -First 1)
# 真事件探针的回显。**必须显式送到 script 作用域**：`$cdpEcho` 是本函数的局部变量，
# 外面读到的是 `$null` —— 症状是"探针没跑起来"（本轮实测踩到，而探针其实跑了并报了 multipoint=186）。
$script:pointerProbeLine = ($cdpEcho | Where-Object { $_ -match 'pointer probe multipoint=' } | Select-Object -First 1)
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
    # **必须显式 -Encoding UTF8**：DOM dump 是 UTF-8，而 PS 5.1 的 `Get-Content` 默认按 ANSI
    # （本机 cp936）解码 ⇒ 来自 DOM 的**中文一律变乱码**（第 67 轮实测：`返回 nil：实体 2 没有
    # Sprite 组件` 被读成 `杩斿洖 nil锛氬疄浣?2 …`）。乱码不是"显示问题"：读数无法引用、无法 grep、
    # 也无法与脚本源码对照，等于把一条刚修好的证据通道又堵回去。
    if (Test-Path -LiteralPath $outTarget) { return Get-Content -LiteralPath $outTarget -Raw -Encoding UTF8 } else { return "" }
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
# 宿主**方法级**缺失清单：`data-rustlua-missing-api`（逗号分隔）。与上面那份"模块级缺失"
# 互补——模块表存在但方法缺失时（例如 `particles.count`）只有这一份说得清；而陷阱代理会把
# 缺失方法顶成"可调用且返回一张表"，症状因此离原因很远（实测：`%d` 收到表）。
$missingApiPaths = @()
if ($dom -match 'data-rustlua-missing-api="([^"]*)"' -and $Matches.Count -gt 0) {
    $missingApiPaths = @($Matches[1].Split(',') | ForEach-Object { $_.Trim() } | Where-Object { $_ })
}
# 宿主**降级**清单：`data-rustlua-degraded`（逗号分隔）。与上面那份"缺口"是两件事，分开列：
#   缺口 = 宿主**没有**这个方法（陷阱代理顶成"可调用且返回一张表"，症状离原因很远）⇒ 要补实现；
#   降级 = 我们**有**实现但语义弱于引擎（只记录开关、恒空表…）⇒ 要么补真实现，要么在设计上接受
#          并保持**可断言**（所以它走 DOM + console.log，而不是只有 console.warn——warn 不进日志）。
$degradedApiPaths = @()
if ($dom -match 'data-rustlua-degraded="([^"]*)"' -and $Matches.Count -gt 0) {
    $degradedApiPaths = @($Matches[1].Split(',') | ForEach-Object { $_.Trim() } | Where-Object { $_ })
}
# 脚本自己画的具名 UI 文本（`data-rustlua-ui-texts`，`名字=文本`，多行用字面 `\n` 连接）。
# **为什么读它**：剧本把「每个 API 模块这一帧干了什么」聚合成一行文本画在 canvas 上——
# DOM 里没有、宿主 API 也查不到，于是"剧本跑到第几节、哪一节报错"过去只能靠"崩在哪一行"反推
# （第 62~66 轮就是这么一轮一个缺口地往前挪的）。有了这条读数，一次运行就能看到全部小节的结果。
$uiTexts = if ($dom -match 'data-rustlua-ui-texts="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "" }
$uiTexts = $uiTexts -replace '&quot;', '"' -replace '&amp;', '&' -replace '\\n', ' | '
# 剧本 `update()` 的返回值（`data-rustlua-script-result`）：`demo_main` 的返回值就是"本帧每个 API
# 小节各干了什么"的聚合串。它比具名文本更直接——一句话读完整个剧本的推进结果。
$scriptResult = if ($dom -match 'data-rustlua-script-result="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "" }
$scriptResult = $scriptResult -replace '&quot;', '"' -replace '&amp;', '&' -replace '&lt;', '<' -replace '&gt;', '>'
# 键位权威（`data-rustlua-bindings`）：工程 project.toml 的 [input].map 与运行时硬编码键位表是
# **两处真相**，启动后由宿主写一行读数说明"这一局以谁为准、多少动作只能靠回退"。
# 为什么值得读：键位错了不会崩、不会报缺 API，只会"按了没反应"——最难自查的一类差异。
$bindingsInfo = if ($dom -match 'data-rustlua-bindings="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "" }
$drawnRaw = if ($dom -match 'data-rustlua-drawn="(-?\d+)"' -and $Matches.Count -gt 0) { [int]$Matches[1] } else { $null }
$uiElements = if ($dom -match 'data-rustlua-ui-elements="(\d+)"' -and $Matches.Count -gt 0) { [int]$Matches[1] } else { $null }
# 世界状态自报：`scene=/map=/alive=/player=@x,y/clip`（+ 渲染侧 `drawn=`）。
# 这里只作为**证据**记录，不新增判定：无头浏览器这一次是"打开页面等 15s 后 dump DOM"，
# 停在启动场景是正常的（玩家要按键才进玩法地图）。真正有牙的那一组断言在
# `scripts/verify/wechat-minigame-harness.js --drive-new-game ...` 里——两个 web 平台共用同一份
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
# P1 第三片（§9-A 第一刀）：引擎宿主的**结构化**状态 + 它本帧消费到的输入。
# 与 `data-rustlua-gpu` 的分工：那条是"画面有没有出来"，这条是"**世界有没有收到输入**"——
# 删掉旧 JS 世界之后风险最高的那一条（旧运行时不跑 ⇒ 不崩溃、不报错、`presented` 照涨，
# 只是人钉死），只有结构化读数能把它变成一条会红的断言。
$engineHostStateRaw = if ($dom -match 'data-rustlua-engine-host="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "" }
$sceneVerifyDetail = if ($dom -match 'data-rustlua-scene-verify-detail="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "" }
# 音频后端读数（`data-rustlua-audio`，由 web-runtime.js 的 syncAudioMarker 写）：**只记录、
# 不判定**，与微信 harness 的 audio_ctx/audio_played 同口径。absent = 产物还是旧版运行时
# （后端接缝没落地）；`kind=none` = 后端没装上；停在启动场景时 created 至少为 1（标题 BGM）。
$audioEvidence = if ($dom -match 'data-rustlua-audio="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "absent" }

# ---- M73-00 第 58 轮 / B8：宿主形态与产物身份（都由页面自报，用来决定"哪些门有写者"） ----
#
# `$jsHostSkipped`：页面自报"旧 JS 世界被跳过、引擎宿主独占这一帧"。
# **B8 之后这条恒为真**：旧世界（`web-runtime.js`）已删除，页面里没有可回退的第二条路，
# 于是 JS 宿主专属的门（场景门/剧本门/世界读数/像素诊断）**没有写者**——那不是回归，
# 而是被测物本身消失了，所以这里只能判"跳过"，引擎侧等价断言由本文件后半段与
# `-Gpu2d` 那几条用例负责。
#
# **已退休的断言**（B8）：原"请求了 JS 宿主（`rustlua-js-host=1`）却仍自报 skipped ⇒ 宿主开关坏了"
# 这条是**旧世界还在时的**一致性检查。今天请求旧世界不可能成功（没有可跑的路），照旧判红只会
# 让所有还带着 `-UrlQuery 'rustlua-js-host=1'` 的既有命令集体变红，而红的原因与被测内容无关。
# 页面侧对应地改成打一行"该开关已退休"的告警（`plugins/webgl/index.html`），留痕但不判红。
$jsHostSkipped = [bool]$script:pageHostSkipped
# `$jsHostRequested`：本次 URL 是否还带着**已退休**的旧世界开关（`rustlua-js-host=1`）。
# 迁移期的契约/剧本门还在带它（那些门本身已随之变成"跳过"），所以这里要能把它和"裸默认 URL"分开：
# 前者不等于"默认入口黑屏"，后者才是。见下面的绘制者判据。
$jsHostRequested = $url -match '[?&]rustlua-js-host=1(?:&|$)'
if ($jsHostSkipped) {
    Write-Log "[跳过] JS 宿主专属的门（场景切换/剧本/世界读数/像素诊断）：页面自报 legacy JS world skipped（B8 之后旧 JS 世界已删除、引擎宿主独占这一帧）⇒ 这些门没有写者，不是回归" Yellow
}

# ---- M73-00 WP8：**默认页面必须有绘制者**（否则 boot=ok 也是纯黑，而旧门不会红）----
#
# 来由（第 113 轮实测）：默认 URL 曾同时"旧 JS 世界不启动 + GPU 宿主没开/没编"，页面
# `boot=ok`、控制台零报错、纯黑一屏，而两个验收入口**全绿**——因为它们只走显式带开关的 URL
# （`-Gpu2d` 加 `rustlua-gpu=1`；契约/剧本加 `rustlua-js-host=1`），裸默认 URL 一条用例都没有。
#
# 判据（B8 之后收紧成一条）：**唯一的绘制者是引擎 GPU 宿主**（`data-rustlua-gpu` 在），
# 或本次 URL **显式**要求"谁都别画"（`rustlua-gpu=0` / `rustlua-js-host=0`）。
# 旧世界那条"没 skipped 就说明它在画"的分支已随 B8 删除：今天"没自报 skipped"只可能是
# **启动链根本没走到写诊断那一步**，那按没有绘制者判红，不再当成"旧世界在跑"。
$gpuHostStarted = [bool]($dom -match 'data-rustlua-gpu="')
$painterOptedOut = ($url -match '[?&]rustlua-gpu=0(?:&|$)') -or ($url -match '[?&]rustlua-js-host=0(?:&|$)')
if ($gpuHostStarted) {
    Write-Log "frame owner ok: engine GPU host (data-rustlua-gpu)" Green
}
elseif ($painterOptedOut) {
    # 合法但**必须显式**：这一次 URL 自己说了"谁都别画"（只验启动链时用）。
    Write-Log "frame owner: none — 本次 URL 显式要求不画（rustlua-gpu=0 / rustlua-js-host=0），画面为空是预期" Yellow
}
elseif ($jsHostRequested) {
    # 带的是**已退休**的开关：旧世界没了 ⇒ 它不带来任何绘制者；而本快照模式下也不等引擎宿主的
    # first-frame（字体预热在实机/headless 上都要几十秒），所以"没有 data-rustlua-gpu"在这里
    # 既不是黑屏、也不是回归。**默认 URL 的黑屏判据由不带该开关的用例负责**（默认用例会等到
    # `data-rustlua-gpu`，那条才是"没有写者的门不会红"的补丁）。
    Write-Log "frame owner: 本次 URL 要求了已退休的旧 JS 宿主（rustlua-js-host=1）：那条路已随 B8 删除，本快照模式下也不等引擎宿主的 first-frame ⇒ 本用例不判'没有绘制者'（默认 URL 的绘制者判据由不带该开关的用例负责）" Yellow
}
elseif ($jsHostSkipped) {
    Write-Log "webgl_bundle_no_frame_owner：boot 已落定但**没有任何绘制者**（GPU 宿主未起，旧 JS 世界也已在 B8 删除）⇒ 页面是黑屏。修法：产物用 package.ps1 -Gpu2d 构建（没有 GPU 宿主的产物今天一个像素都不画）。" Red
    Exit-With 1
}
else {
    Write-Log "webgl_bundle_no_frame_owner：页面连'谁在画'的诊断都没打出来（`legacy JS world skipped` 缺失）⇒ 启动链没走到那一步，不能认定有绘制者" Red
    Exit-With 1
}

# 产物身份自证（**引擎侧、更强的一条**）：页面自报的 `RustLua WebGL/WASM started: <名字>; …`
# 必须与待验产物 `build-info.txt` 里的 `publish_name` 一致。第 57 轮遇到过"验证读到另一个产物"，
# 那时靠 `pack/project.toml` 的 SHA256 抓到了服务根；这条把**页面那侧**也钉住：
# 浏览器真的在跑这份产物，而不是别的产物/别的标签页。名字取不到、对不上，都算红。
$pageIdentity = $null
$pageStartupScene = $null
if ($script:pageStartedLine -and $script:pageStartedLine -match 'started:\s*([^;]+);') { $pageIdentity = $Matches[1].Trim() }
if ($script:pageStartedLine -and $script:pageStartedLine -match 'startup_scene=([^;]+);') { $pageStartupScene = $Matches[1].Trim() }
$bundlePublishName = $null
$buildInfoPath = Join-Path $bundle "build-info.txt"
if (Test-Path -LiteralPath $buildInfoPath) {
    $publishLine = Get-Content -LiteralPath $buildInfoPath -Encoding UTF8 -ErrorAction SilentlyContinue |
        Where-Object { $_ -match '^\s*publish_name\s*:' } | Select-Object -First 1
    if ($publishLine -and $publishLine -match ':\s*(\S+)\s*$') { $bundlePublishName = $Matches[1] }
}
if (-not $pageIdentity) {
    Write-Log "webgl_bundle_page_identity_missing：页面没有自报产物身份（RustLua WebGL/WASM started: …）⇒ 无法证明浏览器跑的是待验产物" Red
    Exit-With 1
}
if ($bundlePublishName -and $pageIdentity -ne $bundlePublishName) {
    Write-Log "webgl_bundle_page_identity_mismatch：页面自报=$pageIdentity，待验产物 publish_name=$bundlePublishName" Red
    Exit-With 1
}
if (-not $pageStartupScene) {
    Write-Log "webgl_bundle_page_startup_scene_missing：页面自报里没有 startup_scene ⇒ 引擎没有报出启动场景" Red
    Exit-With 1
}
Write-Log "page identity ok: $pageIdentity（startup_scene=$pageStartupScene，与待验产物 build-info.txt 一致）"

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
    missingApiPathCount = $missingApiPaths.Count
    missingApiPaths = ($missingApiPaths -join ';')
    degradedApiPathCount = $degradedApiPaths.Count
    degradedApiPaths = ($degradedApiPaths -join ';')
    uiTexts = $uiTexts
    scriptResult = $scriptResult
    bindingsInfo = $bindingsInfo
    statusText     = $statusText
    drawnSamples   = $drawnRaw
    # P1 第二片续：**引擎侧**证据也进摘要。此前它只活在控制台与 DOM dump 里，于是
    # `verify.ps1 -Web` 只能拿旧 JS 世界的 `drawnSamples`/`sceneVerify` 当"没黑屏"的判据——
    # 而默认产物一旦由 Rust 宿主出帧（旧世界被跳过），那两个读数**必然缺席**，
    # 门就会把"设计如此"读成"黑屏"。落进摘要之后，`verify.ps1` 才有替代判据可用。
    gpuState       = $gpuState
    jsHostSkipped  = $jsHostSkipped
    engineHostState = $engineHostStateRaw
    uiElements     = $uiElements
    world          = $world
    tileLayers     = if ($null -eq $tileDrawn) { "absent" } else { "$tileDrawn/$tileDeclared" }
    drive          = $driveState
    driveDetail    = $driveStateDetail
    # `skipped` 与 `absent` 必须分开：前者 = 写者(旧 JS 世界)已随 B8 删除、本门没有被测物
    # （见 911 行的跳过分支），后者 = 请求了剧本却连自报都没有（真的坏了）。调用方按这个字段判。
    driveJhyt      = if (-not $DriveJhyt) { "not-requested" } elseif ($driveState -ne "done" -and $jsHostSkipped) { "skipped" } else { $driveState }
    driveJhytDetail = if ($DriveJhyt) { $driveStateDetail } else { "" }
    sceneVerify    = $sceneVerifyState
    sceneVerifyDetail = $sceneVerifyDetail
    audio          = $audioEvidence
    # 同上：`unavailable` 是因为旧 JS 世界的 Lua 状态随 B8 删除、没有可注的被测物，不是契约失败。
    apiContract    = if ($ApiContract -and $contractVerdict -ne "ok" -and $jsHostSkipped) { "skipped" } else { $contractVerdict }
    apiContractCases = $contractCases
    apiContractReport = $contractReportPath
    domDump        = $domPath
    log            = $logPath
}
$summary | ConvertTo-Json | Set-Content -Path (Join-Path $logRoot "bundle-summary-$stamp.json") -Encoding UTF8

Write-Log "missing_api_paths=$($missingApiPaths.Count)$(if ($missingApiPaths.Count -gt 0) { ': ' + ($missingApiPaths -join ', ') })" $(if ($missingApiPaths.Count -eq 0) { "Green" } else { "Yellow" })
# 降级读数用 Yellow（它本身不算失败，但必须是**看得见**的事实：不能只有 console.warn，
# 那条通道 harness 抓不到，于是"没触发"与"没抓到"分不开——第 65 轮的教训）。
Write-Log "degraded_api_paths=$($degradedApiPaths.Count)$(if ($degradedApiPaths.Count -gt 0) { ': ' + ($degradedApiPaths -join ', ') })" $(if ($degradedApiPaths.Count -eq 0) { "Green" } else { "Yellow" })
# 脚本自报的具名 UI 文本：这是"剧本这一帧到底说了什么"的直接读数（过去读不到，只能反推崩溃行号）。
Write-Log "script_ui_texts$(if ([string]::IsNullOrWhiteSpace($uiTexts)) { '(空)' } else { '' }): $uiTexts"
Write-Log "script_result$(if ([string]::IsNullOrWhiteSpace($scriptResult)) { '(空)' } else { '' }): $scriptResult"
Write-Log "key_bindings$(if ([string]::IsNullOrWhiteSpace($bindingsInfo)) { '(无读数)' } else { '' }): $bindingsInfo"
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
    # 判定配色要跟着"有没有被测物"走（B8）：写者已删时 `unavailable` 不是失败，别印成 RED 让人误读
    # （最终判决在下面 `webgl_api_contract_failed` 那一段，那里同样按 `$jsHostSkipped` 跳过）。
    Write-Log "api_contract: $contractVerdict cases=$contractCases report=$contractReportPath" `
        $(if ($contractVerdict -eq "ok") { "Green" } elseif ($jsHostSkipped) { "Yellow" } else { "Red" })
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
        #
        # **但先看清页面在哪张图上**：同一份产物连跑两次，实测一次红一次绿 —— 红的那次
        # `script_result: index_open scene=main`、`world=scene=34 map=index tilemaps=0/0`，
        # 即剧本**还停在启动场景（菜单）**、根本没走进玩法场景；绿的另一次是
        # `world=scene=22 map=main tilemaps=1/1`。两者产物字节相同 ⇒ 这是等待器落定时机造成的
        # 抖动，不是玩法场景缺层。所以：观测到的场景 == 页面启动场景时，判黄跳过并写明原因
        # （`startup_scene` 从页面自报身份里取；取不到就保持原来的红灯，不静默放行）。
        $observedScene = if ($world -match 'map=([^\s]+)') { $Matches[1] } else { "" }
        # 页面自报的启动场景形如 `scenes/index.scene`（见上面 page identity 那段）。
        $startupScene = if ($pageStartupScene -match 'scenes/([^\./]+)\.scene') { $Matches[1] } else { "" }
        if ($observedScene -ne "" -and $startupScene -ne "" -and $observedScene -eq $startupScene) {
            Write-Log ("[跳过] 瓦片层门：页面仍在启动场景 '$observedScene'（剧本尚未进入玩法场景）world=$world") Yellow
        } else {
            Write-Log "webgl_bundle_tile_layers_empty detail=gameplay scene has no tilemap layer world=$world" Red
            Exit-With 1
        }
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
    # P0-①：请求了"完整剧本"（标题确认 → 进玩法地图 → 位移非零）却没跑完 ⇒ 验收失败。
    # 唯一例外：页面自报 skipped ⇒ 剧本门**没有写者**。B8 之后这一条是恒真的：
    # 旧 JS 世界（剧本的唯一写者）已删除，`rustlua-js-host=1` 也没有可回退的路。
    #
    # **为什么不按工程名/工程类型跳过**（第 75 轮改动）：这里以前写着
    # `elseif ($pageIdentity -and $pageIdentity -notmatch 'jhyt')`，即"名字里没有 jhyt 就跳过"。
    # 它与"运行时到底做了什么"无关：多一个展示型工程就掉进假红，而玩法工程真的没响应 confirm
    # 时又会被当成"不是玩法工程"**静默跳过**（假绿，最坏的一种）。第 74 轮我把它换成
    # "页面自报没观察到场景切换就跳过"，**实测仍然不够**：game_api 的启动场景**确实**消费了
    # confirm（场景切到 main），卡在后面"按住的键不驱动玩家位移" ⇒ 仍会走进跳过分支。
    # 根因是运行时**原理上分不出**这两件事：「这个工程本来就没有可走动玩法」与「可走动玩法坏了」
    # 在观测上完全一样（都是"没有输入驱动的位移"）。既然观测分不出，就不许猜 ——
    # 门保持严格：任何失败一律 RED，并把**观测到的数据**与**该找谁**写进消息。
    if ($jsHostSkipped) {
        Write-Log "[跳过] 剧本门（-Drive）：写者（旧 JS 世界）已随 B8 删除，本门没有替代者。引擎侧的等价通道是 -Gpu2d -SimRoute script -InputScript … + -WorldExpr（见 tasks/plan/M73-00 的 WP3 各片读数）" Yellow
    } else {
        # 按页面自报的 detail 说明**卡在哪一步**（这是信息，不是判决）。三种形态：
        #   · 没观察到场景切换 ⇒ 启动场景不消费 confirm（展示型工程常见）
        #   · 没有玩家坐标     ⇒ 进入的场景里读不到 player
        #   · 位移为 0         ⇒ 有玩家，但按住的键不驱动它
        $where = "（按页面自报的数据未能定位到具体阶段）"
        if ($driveStateDetail -match 'scene switch did not happen') {
            $where = "（卡在『确认→进玩法地图』：按 2 帧空格并等待后未观察到任何场景切换 ⇒ 该工程的启动场景不消费 confirm）"
        } elseif ($driveStateDetail -match 'no player position observed') {
            $where = "（卡在『进玩法地图』：场景里读不到 player=…@x,y ⇒ 该场景没有可读的玩家实体）"
        } elseif ($driveStateDetail -match 'd=0,0') {
            $where = "（卡在『位移非零』：读到了玩家，但按住的键没有驱动它移动）"
        }
        Write-Log "webgl_bundle_drive_failed boot=$boot drive=$driveState detail=$driveStateDetail world=$world $where" Red
        Write-Log "[提示] -Drive 是**玩法工程**的门（要求确认键进得了玩法地图、且方向键能驱动玩家位移）。没有可走动玩法的展示型工程请改用不带 -Drive 的验收 + lua-do/WorldDo 断言来验输入链路；玩法动线用 -DriveJhyt" Yellow
        Exit-With 1
    }
}

if ($DriveJhyt -and $driveState -ne "done") {
    # P0-②：请求了"完整玩法剧本"却没跑完（卡在某个里程碑或超时）——无论 boot 是否 ok，都是验收失败。
    if ($jsHostSkipped) {
        Write-Log "[跳过] 玩法剧本门（-DriveJhyt）：写者（旧 JS 世界）已随 B8 删除，本门没有替代者；引擎侧玩法证据见 tasks/plan/M73-00 的 WP3 各片与微信侧探针记录" Yellow
    } else {
        Write-Log "webgl_bundle_drivejhyt_failed boot=$boot drive=$driveState detail=$driveStateDetail world=$world" Red
        Exit-With 1
    }
}

# M73-00 WP5 第五片：GPU 自检放**场景切换门之前**。理由是实测的：GPU 画布若抢走 2D 上下文，
# 先红的是场景切换门（`boot=failed detail=2D canvas context is not available`），而根因在 GPU
# 那一侧——两条读数必须同框出现，否则这份日志会把人指向错误的文件。
if ($Gpu2d -and $gpuState -notlike "presented*") {
    # `absent` 的头号原因是产物没用 `package.ps1 -Gpu2d` 构建（GPU 宿主没编进去）——这种情况
    # 必须说清楚，否则会被读成"引擎渲染器在浏览器里画不出来"。
    $hint = if ($gpuState -eq "absent") {
        "（产物里没有 GPU 宿主：需要用 scripts/build/package.ps1 -Gpu2d 构建）"
    } else { "" }
    Write-Log "webgl_bundle_gpu2d_failed boot=$boot gpu=$gpuState $hint" Red
    Exit-With 1
}

# P1 第三片（§9-A 第一刀）：引擎宿主必须把它本帧消费到的输入与场景读数报出来，而且报的东西
# 必须过断言。**为什么这必须是门而不是人工读**：旧 JS 世界一被跳过，`drawnSamples`/`sceneVerify`/
# `world` 这些旧世界的读数全都缺席（它们不缺才是怪事），于是"输入还有没有进到宿主"就没有写者了
# ——而那正是删旧世界时最危险的失效（不崩溃、不报错、presented 照涨）。
if ($jsHostSkipped -and -not $painterOptedOut -and -not $jsHostRequested) {
    if (-not $engineHostStateRaw) {
        Write-Log "webgl_bundle_engine_state_absent: 页面自报引擎宿主独占这一帧，但没有 data-rustlua-engine-host ⇒ 无法断言输入是否进了宿主" Red
        Exit-With 1
    }
    $engineHostParsed = $null
    try { $engineHostParsed = (($engineHostStateRaw -replace '&quot;', '"') -replace '&amp;', '&') | ConvertFrom-Json } catch { }
    if (-not $engineHostParsed) {
        Write-Log "webgl_bundle_engine_state_unparsable: $($engineHostStateRaw.Substring(0, [Math]::Min(160, $engineHostStateRaw.Length)))" Red
        Exit-With 1
    }
    # ⓪ 先认外壳：这一项证明"答这个导出的"是浏览器那条（与微信那条区分开）。
    if ($engineHostParsed.shell -ne "browser-gpu-host") {
        Write-Log "webgl_bundle_engine_shell_unexpected: 期望 shell=browser-gpu-host，实际 shell=$($engineHostParsed.shell)" Red
        Exit-With 1
    }
    # ① 画的必须是**工程真场景**：`builtin=true` 是"引擎 loader 失败、悄悄换内建兜底"，
    #    它同样能让 presented/draws 都好看。
    if ($engineHostParsed.scene.builtin -ne $false) {
        Write-Log "webgl_bundle_engine_builtin_scene: 引擎宿主画的是内建兜底场景（builtin=$($engineHostParsed.scene.builtin)、sim=$($engineHostParsed.scene.sim)）而不是工程真场景" Red
        Exit-With 1
    }
    # ② 请求了确定性输入剧本 ⇒ 宿主必须**真的收到过**输入。判据用**累计量** `seen_frames` 而不是
    #    瞬时 axis/held：剧本窗口跑完就回到 0，读瞬时值会把"输入进了宿主"读成"没进"（假阴性）。
    if ($InputScript -ne "" -and [int]$engineHostParsed.scene.input.seen_frames -le 0) {
        Write-Log "webgl_bundle_engine_input_missing: 请求了输入剧本 $InputScript，但宿主自报 seen_frames=$($engineHostParsed.scene.input.seen_frames) ⇒ 输入没有进到宿主（不崩溃、不报错的失效）" Red
        Exit-With 1
    }
    # ②′ 指针剧本（`at:` + `hold:`）请求了「按下后拖走」⇒ 宿主必须报出「按下点与光标**真的不同**」的帧数。
    # 判据刻意不是"字段存在"而是"两个值真的分开了"：字段一直在（刚加的那次就在），而"退化成一个值"
    # 才是这条链唯一会静默出错的方式——`press_cursor` 跟着光标走，命中测试就回到"跟着手指跑"。
    if ($InputScript -match 'hold:' -and [int]$engineHostParsed.scene.input.press_distinct_frames -le 0) {
        Write-Log "webgl_bundle_engine_press_cursor_missing: 请求了指针剧本 $InputScript，但宿主自报 press_distinct_frames=$($engineHostParsed.scene.input.press_distinct_frames) ⇒ 按下点没有与光标分开（press_cursor 退化成实时光标）" Red
        Exit-With 1
    }
    # ②″ 多指剧本（`at2:`/`hold2:`）请求了「两指同时按」⇒ 宿主必须报出「同时有多个指针」的帧数。
    # 判据仍然是**累计量**：单帧采样会抓到两指都抬起之后的 0（这一条已经被前两个读数教训过两次）。
    if ($InputScript -match 'at2:|hold2:' -and [int]$engineHostParsed.scene.input.multipoint_frames -le 0) {
        Write-Log "webgl_bundle_engine_multipoint_missing: 请求了多指剧本 $InputScript，但宿主自报 multipoint_frames=$($engineHostParsed.scene.input.multipoint_frames) ⇒ 第二根手指没进到宿主" Red
        Exit-With 1
    }
    # ③ 真事件通路（`-PointerProbe`）：页内派发**真实**的两指 `pointerdown` ⇒ 宿主必须报出多指帧数。
    # 与 ②″ 的分工：那条靠**剧本**证明数据链装得下两指，这条靠**真事件**证明输入源真的在产它——
    # 少了这条，"接了真实输入源"就只是我的一句声明。两项都是**累计**量，抬手不会抹回去。
    if ($PointerProbe) {
        # 探针跑在 **DOM 落盘之后**（与 `-KeyProbe` 同一位置），所以读数只从 cdp 回显里取：
        # dump 里那份是探针之前的那一刻，拿它断言等于断言了个寂寞（本轮实测踩到）。
        $probeLine = $script:pointerProbeLine
        if (-not $probeLine) {
            Write-Log "webgl_bundle_engine_pointer_probe_missing: 探针没有回显读数（--pointerProbe 没跑起来？）" Red
            Exit-With 1
        }
        $probeMulti = [int]([regex]::Match($probeLine, 'multipoint=(\d+)').Groups[1].Value)
        if ($probeMulti -le 0) {
            Write-Log "webgl_bundle_engine_pointer_probe_missing: 页内派发了两指 pointerdown，但宿主自报 multipoint=$probeMulti ⇒ 真实指针事件没有进到宿主（$($probeLine.Trim())）" Red
            Exit-With 1
        }
        Write-Log "webgl_bundle_engine_pointer_probe_ok $($probeLine.Trim())" Green
    }
    Write-Log "webgl_bundle_engine_state_ok builtin=$($engineHostParsed.scene.builtin) sim=$($engineHostParsed.scene.sim) draws=$($engineHostParsed.scene.draws) tris=$($engineHostParsed.scene.tris) input_frame=$($engineHostParsed.scene.input.frame) input_seen_frames=$($engineHostParsed.scene.input.seen_frames) input_script=$InputScript press_cursor=$($engineHostParsed.scene.input.press_cursor -join ',') press_distinct_frames=$($engineHostParsed.scene.input.press_distinct_frames) pointer_count=$($engineHostParsed.scene.input.pointer_count) multipoint_frames=$($engineHostParsed.scene.input.multipoint_frames)" Green
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
# 与 JS 世界无关；再配上按帧号触发的 `<键|axis0±|axis1±>:<起帧>-<止帧>` 剧本，这个门就是**确定性**的。
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
    #
    # 第 58 轮的唯一例外：这个门由 **JS 宿主**写（页面里的 `[scene-verify] …`）。本次**没要求**
    # JS 宿主、且页面自报 `legacy JS world skipped`（引擎宿主独占这一帧）时，这个门没有写者
    # ⇒ 不能判红，否则默认路径（玩家实际拿到的形态）上永远是一盏亮着的红灯。
    # 这时改由上面那条**更强**的引擎侧断言兜底：页面自报的 publish_name 必须与待验产物一致、
    # 且必须报出 startup_scene。
    if ($jsHostSkipped) {
        Write-Log "[跳过] 场景切换门：JS 宿主未参与（页面自报 legacy JS world skipped）⇒ 该门没有写者；引擎侧身份与启动场景断言已通过（page=$pageIdentity startup_scene=$pageStartupScene）" Yellow
    } else {
        Write-Log "webgl_bundle_scene_verify_failed boot=$boot scene_verify=$sceneVerifyState detail=$sceneVerifyDetail world=$world gpu=$gpuState" Red
        Exit-With 1
    }
}

if ($ApiContract -and $contractVerdict -ne "ok") {
    if ($jsHostSkipped) {
        # **B8：这条门的写者已删除**。契约是注入到页面里那个**打完宿主补丁的活体 Lua 状态**上跑的
        # （`lua_api_contract.lua` 经 CDP 送进 JS 世界）；旧 JS 世界随 B8 删除后，页面上没有那个
        # Lua 状态可注，`contractVerdict` 只会是 `unavailable` —— 那不是"契约失败"，是**没有被测物**。
        # 保留的等价覆盖：微信侧 harness 用**同一份契约文件**在 wasm 桥上跑（那里的 L3 契约仍然
        # `ok`，因为引擎自己的 Lua 状态还在），桌面 transcript 基线也仍在仓库里。
        Write-Log "[跳过] L3 行为契约门（-ApiContract）：写者（旧 JS 世界的 Lua 状态）已随 B8 删除 ⇒ 本门没有被测物；等价覆盖在微信 harness 的 l3_api_contract=ok（同一份契约、经 wasm 桥）" Yellow
    } else {
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
