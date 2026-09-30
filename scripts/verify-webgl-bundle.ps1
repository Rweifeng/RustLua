# verify-webgl-bundle.ps1（M31/P3）：验证**真实发布出来的 WebGL 包**能否在浏览器里启动
# 并跑通一次场景切换（防回归门）。
#
# 与 verify-webgl.ps1 的分工：
#   * verify-webgl.ps1  —— 平台契约层（资源获取/解析、WebGL2 能力、存储降级、缺失资源诊断），
#                          用一张契约检查页跑，不依赖 wasm 模块；
#   * 本脚本            —— 产物层：直接起静态服务器 + 无头浏览器打开 `builds/webgl/index.html`，
#                          断言 wasm 模块加载、页面进入 Lua 执行（`data-p2-boot` 标记），
#                          并且默认让首帧重新走一遍 applySceneSwitch（`?p2-scene-verify=1`），
#                          断言 `data-p2-scene-verify=done`。这条**场景切换门**是上次
#                          wasm panic=abort 与宿主闭包 raise 交叉崩溃（boot=ok 却一切就死）
#                          的回归哨兵；`-NoSceneVerify` 会把它关掉（只验 boot）。
#
# 用法：
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\verify-webgl-bundle.ps1 `
#       [-BundleRoot examples\game_jhyt\builds\webgl] [-Port 8137] [-TimeoutSeconds 90]
#   # 关掉场景切换门（boot-only，快但不防切换崩溃回归）：
#   powershell ... -File scripts\verify-webgl-bundle.ps1 -NoSceneVerify
#
# 省略 -BundleRoot 时自动发现（本机唯一的合格 webgl 产物，见 scripts\bundle-discovery.ps1）：
# 默认值不再写死某个游戏工程，任何工程发布后都能直接跑。
#
# 退出码：0 = 启动成功且场景切换门通过（`data-p2-boot=ok` + `data-p2-scene-verify=done`）；
#         1 = 启动或场景切换失败（含缺失宿主 API 与 scene-verify 明细）；
#         2 = 环境受限（无 node/浏览器）；3 = 未找到产物（需 -BundleRoot 显式指定）。

[CmdletBinding()]
param(
    [string]$BundleRoot = "",
    [int]$Port = 8137,
    [int]$TimeoutSeconds = 90,
    # P0-①：URL 驱动"完整剧本"（`?p2-drive=auto`）：标题按确认 → 进玩法地图 → 按住方向键
    # 移动。断言 `data-p2-drive=done` 且位移非零（主角真的在浏览器里被驱动着走了）。
    # 与 -NoSceneVerify 独立：剧本是"跑游戏内容"，scene-verify 是"跑运行时切换机制"。
    [switch]$Drive,
    # 关掉场景切换防回归门（默认 ON）：仅当产物里没有可 reload 的启动场景时才关。
    [switch]$NoSceneVerify,
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
if ($Drive) { $queryBits += "p2-drive=auto" }
if (-not $NoSceneVerify) { $queryBits += "p2-scene-verify=1" }
$url = "http://127.0.0.1:$Port/index.html"
if ($queryBits.Count -gt 0) { $url += "?" + ($queryBits -join "&") }
$domPath = Join-Path $logRoot "bundle-dom-$stamp.html"
$errPath = Join-Path $logRoot "bundle-dom-$stamp.err"

function Invoke-HeadlessDom([string]$HeadlessFlag) {
    $arguments = @(
        $HeadlessFlag,
        "--disable-gpu-sandbox",
        "--no-sandbox",
        "--disable-extensions",
        # 转发 console.*（含 wasm panic hook 的 console.error）到 stderr（CONSOLE 前缀），
        # 排查 wasm trap（如场景切换 unreachable）时能从 bundle-dom-*.err 拿到 Rust panic 详情。
        "--enable-logging=stderr",
        "--v=0",
        "--virtual-time-budget=120000",
        "--dump-dom",
        $url
    )
    $process = Start-Process -FilePath $browser -ArgumentList $arguments -PassThru -NoNewWindow `
        -RedirectStandardOutput $domPath -RedirectStandardError $errPath
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
        try { $process.Kill() } catch { }
        Write-Log "headless browser timed out after ${TimeoutSeconds}s ($HeadlessFlag)" Yellow
    }
    if (Test-Path -LiteralPath $domPath) { return Get-Content -LiteralPath $domPath -Raw } else { return "" }
}

$dom = Invoke-HeadlessDom "--headless"
if (-not $dom -or $dom -notmatch "data-p2-boot") {
    $dom = Invoke-HeadlessDom "--headless=new"
}
Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue

if (-not $dom) {
    Write-Log "headless browser produced no DOM dump (see $errPath)" Red
    Exit-With 2
}

# ---------- 3. 断言 ----------
$boot = if ($dom -match 'data-p2-boot="([^"]*)"') { $Matches[1] } else { "absent" }
$detail = if ($dom -match 'data-p2-boot-detail="([^"]*)"') { $Matches[1] } else { "" }
$statusText = if ($dom -match '<div id="status">([^<]*)</div>') { $Matches[1] } else { "" }
# 缺失宿主 API 明细：`detail` 里是 `… [missing engine host API: a, b, c]`。单独导出一个
# **数值**字段给验收样例读（JSON 里逗号与转义引号都会打乱"按字段切割"的朴素解析）。
$missingNames = @()
if ($detail -match 'missing engine host API:\s*([^\]]*)') {
    $missingNames = @($Matches[1].Split(',') | ForEach-Object { $_.Trim() } | Where-Object { $_ })
}
# 画面自检：`data-p2-drawn` 是运行时对 canvas 采样得到的“非背景像素采样点”数量。
# `-1` = 像素不可读（跨域贴图污染），`0` = 启动了但真的什么都没画出来。
$drawnRaw = if ($dom -match 'data-p2-drawn="(-?\d+)"') { [int]$Matches[1] } else { $null }
$uiElements = if ($dom -match 'data-p2-ui-elements="(\d+)"') { [int]$Matches[1] } else { $null }
# 世界状态自报：`scene=/map=/alive=/player=@x,y/clip`（+ 渲染侧 `drawn=`）。
# 这里只作为**证据**记录，不新增判定：无头浏览器这一次是"打开页面等 15s 后 dump DOM"，
# 停在启动场景是正常的（玩家要按键才进玩法地图）。真正有牙的那一组断言在
# `scripts/wechat-minigame-harness.js --drive-new-game ...` 里——两个 web 平台共用同一份
# `web-runtime.js`，字段含义完全一致，所以这里读到的世界状态可以直接对照它。
$world = if ($dom -match 'data-p2-world="([^"]*)"') { $Matches[1] } else { "" }
# P0-①：`?p2-drive=auto` 的剧本结果。`done` = 确认 → 进玩法地图 → 位移非零全部发生；
# `failed` 时 detail 给出卡在哪一步（场景没切换 / 没有玩家位置 / 位移为 0）。
$driveState = if ($dom -match 'data-p2-drive="([^"]*)"') { $Matches[1] } else { "absent" }
$driveStateDetail = if ($dom -match 'data-p2-drive-detail="([^"]*)"') { $Matches[1] } else { "" }
# 场景切换防回归门：`?p2-scene-verify=1` 让首帧把当前场景重新走一遍 applySceneSwitch
# 全流程（fetch → parse → 重绑 Lua → 重跑 on_create）。done=通过、failed=细节给出错误、
# in-flight=在途未落定（通常说明 headless 虚拟时间预算在切换 promise resolve 前烧完）、
# absent=URL 门没接上或 runtime 未升级。后两种都算失败。
$sceneVerifyState = if ($dom -match 'data-p2-scene-verify="([^"]*)"') { $Matches[1] } else { "absent" }
$sceneVerifyDetail = if ($dom -match 'data-p2-scene-verify-detail="([^"]*)"') { $Matches[1] } else { "" }

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
    drive          = $driveState
    driveDetail    = $driveStateDetail
    sceneVerify    = $sceneVerifyState
    sceneVerifyDetail = $sceneVerifyDetail
    domDump        = $domPath
    log            = $logPath
}
$summary | ConvertTo-Json | Set-Content -Path (Join-Path $logRoot "bundle-summary-$stamp.json") -Encoding UTF8

Write-Log "boot=$boot missing_host_api=$($missingNames.Count) detail=$detail" $(if ($boot -eq "ok") { "Green" } else { "Yellow" })
Write-Log "status text: $statusText"
Write-Log "drawn samples: $(if ($null -eq $drawnRaw) { 'unknown' } else { $drawnRaw }) ui elements: $(if ($null -eq $uiElements) { 'unknown' } else { $uiElements })"
Write-Log "world: $(if ([string]::IsNullOrWhiteSpace($world)) { '(未自报)' } else { $world })"
if ($Drive) {
    Write-Log "drive: $driveState detail: $driveStateDetail" $(if ($driveState -eq "done") { "Green" } else { "Yellow" })
}
if (-not $NoSceneVerify) {
    Write-Log "scene_verify: $sceneVerifyState detail: $sceneVerifyDetail" $(if ($sceneVerifyState -eq "done") { "Green" } else { "Yellow" })
}
Write-Log "dom: $domPath"

if ($drawnRaw -eq 0) {
    # 启动成功但没有画面：这正是"页面能跑却黑屏"的典型症状，必须显式失败。
    Write-Log "webgl_bundle_blank boot=$boot detail=canvas has no painted pixels" Red
    Exit-With 1
}

if ($Drive -and $driveState -ne "done") {
    # P0-①：请求了"完整剧本"却没跑完——无论 boot 是否 ok，都是验收失败。
    Write-Log "webgl_bundle_drive_failed boot=$boot drive=$driveState detail=$driveStateDetail world=$world" Red
    Exit-With 1
}

if (-not $NoSceneVerify -and $sceneVerifyState -ne "done") {
    # 场景切换防回归门：boot 能过、场景一切就死（wasm 宿主闭包 raise 撞 panic=abort 的现场
    # 症状）就是这类问题。absent/in-flight/failed 三种都算失败，因为都意味着
    # applySceneSwitch 的 fetch → parse → on_create 全流程没有干净跑完。
    Write-Log "webgl_bundle_scene_verify_failed boot=$boot scene_verify=$sceneVerifyState detail=$sceneVerifyDetail world=$world" Red
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