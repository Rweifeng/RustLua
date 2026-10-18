[CmdletBinding()]
param(
    # 已发布的 webgl 产物目录；留空则用 bundle-discovery 自动发现（要求唯一）。
    [string]$BundleRoot = "",
    [int]$Port = 8150,
    # 单个浏览器从启动到回传结果的等待上限（秒）。
    [int]$TimeoutSeconds = 60,
    # 调用方（verify.ps1 / CI）用它拿退出码：Start-Process -PassThru 在重定向下 ExitCode 是 $null。
    [string]$StatusFile = "",
    # 证据输出目录；留空用 artifacts\logs\webgl。
    [string]$OutDir = "",
    # 只测指定引擎（edge / chrome / firefox）；留空测全部已安装的。
    [string[]]$Only = @()
)

# wasm 新 EH（try_table/exnref）跨浏览器验证：把 eh-probe.html 送进本机每个可用浏览器，
# 让它 fetch + WebAssembly.compile 真实 webgl 产物的 game_web_bg.wasm，回传“能否编译”。
# 编译成功 == 该引擎支持本产物所用的新 EH。退出码：0=已测浏览器全 PASS；1=有 FAIL；
# 3=环境受限（无浏览器 / 无产物 / 无 node）——显式跳过，绝不把“跳过”当“通过”。
#
# 各引擎新 EH 门槛（MDN browser-compat-data，2026-09 核对，低于此连编译都过不去）：
#   Chrome / Edge 137   Firefox 131   Safari / iOS Safari 18.4
# Safari 无法在 Windows 上自动化：本脚本会在结尾打印手动验证 URL（在 Mac 上跑同一个 server 即可）。

$ErrorActionPreference = "Stop"

function Exit-With([int]$Code) {
    if (-not [string]::IsNullOrWhiteSpace($StatusFile)) {
        "exit=$Code" | Set-Content -LiteralPath $StatusFile -Encoding ASCII
    }
    exit $Code
}

trap {
    Write-Host "[RED] webgl EH cross-browser check aborted: $($_.Exception.Message) @line $($_.InvocationInfo.ScriptLineNumber)"
    Exit-With 1
}

$workspaceRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$probeDir = Join-Path $PSScriptRoot "..\lib\wasm-lua"
$probeHtml = Join-Path $probeDir "eh-probe.html"
$probeServer = Join-Path $probeDir "eh-probe-server.js"
foreach ($required in @($probeHtml, $probeServer)) {
    if (-not (Test-Path -LiteralPath $required)) { throw "missing probe asset: $required" }
}

# ---------- 产物 ----------
. (Join-Path $PSScriptRoot "..\lib\bundle-discovery.ps1")
if ([string]::IsNullOrWhiteSpace($BundleRoot)) {
    $BundleRoot = Resolve-GameBundle -Platform "webgl" -WorkspaceRoot $workspaceRoot
    if (-not $BundleRoot) { Exit-With 3 }
}
if (-not (Test-Path -LiteralPath $BundleRoot)) {
    Write-Host "[YELLOW] webgl bundle not found: $BundleRoot (先跑 scripts\build\package.ps1 -Platform webgl)"
    Exit-With 3
}
$bundle = (Resolve-Path -LiteralPath $BundleRoot).Path
$wasmRel = "pkg/game_web_bg.wasm"
$wasmPath = Join-Path $bundle $wasmRel
if (-not (Test-Path -LiteralPath $wasmPath)) {
    Write-Host "[YELLOW] webgl bundle has no $wasmRel : $bundle"
    Exit-With 3
}

# ---------- node ----------
$node = Get-Command node -ErrorAction SilentlyContinue
if (-not $node) {
    Write-Host "[YELLOW] node.js not found; cannot host the EH probe server"
    Exit-With 3
}

# ---------- 浏览器发现 ----------
$pf86 = ${env:ProgramFiles(x86)}
$candidates = @(
    [pscustomobject]@{ name = "edge"; kind = "chromium"; exe = (Join-Path $pf86 "Microsoft\Edge\Application\msedge.exe") },
    [pscustomobject]@{ name = "edge"; kind = "chromium"; exe = (Join-Path $env:ProgramFiles "Microsoft\Edge\Application\msedge.exe") },
    [pscustomobject]@{ name = "chrome"; kind = "chromium"; exe = (Join-Path $env:ProgramFiles "Google\Chrome\Application\chrome.exe") },
    [pscustomobject]@{ name = "chrome"; kind = "chromium"; exe = (Join-Path $pf86 "Google\Chrome\Application\chrome.exe") },
    [pscustomobject]@{ name = "firefox"; kind = "firefox"; exe = (Join-Path $env:ProgramFiles "Mozilla Firefox\firefox.exe") },
    [pscustomobject]@{ name = "firefox"; kind = "firefox"; exe = (Join-Path $pf86 "Mozilla Firefox\firefox.exe") }
)
$browsers = @()
$seen = @{}
foreach ($c in $candidates) {
    if (-not ($c.exe -and (Test-Path -LiteralPath $c.exe))) { continue }
    if ($seen.ContainsKey($c.name)) { continue }
    if ($Only.Count -gt 0 -and ($Only -notcontains $c.name)) { continue }
    $seen[$c.name] = $true
    $browsers += $c
}
if ($browsers.Count -eq 0) {
    Write-Host "[YELLOW] no supported browser found (Edge/Chrome/Firefox); EH cross-browser check cannot run here"
    Exit-With 3
}

# ---------- 输出目录与日志 ----------
$logRoot = if ([string]::IsNullOrWhiteSpace($OutDir)) { (Join-Path $workspaceRoot "artifacts\logs\webgl") } else { $OutDir }
New-Item -ItemType Directory -Force -Path $logRoot | Out-Null
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$serverLog = Join-Path $logRoot "eh-server-$stamp.log"
$resultsJson = Join-Path $logRoot "eh-results-$stamp.json"
$summaryJson = Join-Path $logRoot "eh-summary-$stamp.json"

function Write-Log([string]$Message) {
    Write-Host $Message
    Add-Content -Path (Join-Path $logRoot "eh-check-$stamp.log") -Value $Message -Encoding UTF8
}

Write-Log "==> WebGL new-EH cross-browser check (bundle=$bundle)"
Write-Log ("    wasm={0} ({1} KB)" -f $wasmRel, [math]::Round((Get-Item -LiteralPath $wasmPath).Length / 1KB, 1))
Write-Log ("    browsers: {0}" -f (($browsers | ForEach-Object { $_.name }) -join ", "))

# ---------- 启动探针服务器 ----------
$serverProc = Start-Process -FilePath $node.Source `
    -ArgumentList @($probeServer, $bundle, "$Port", $probeHtml, $resultsJson) `
    -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput $serverLog -RedirectStandardError "$serverLog.err"

function Wait-ServerReady {
    for ($i = 0; $i -lt 40; $i++) {
        Start-Sleep -Milliseconds 250
        if (Test-Path -LiteralPath $serverLog) {
            $txt = Get-Content -LiteralPath $serverLog -Raw -ErrorAction SilentlyContinue
            if ($txt -and $txt -match "serving http") { return $true }
        }
        if ($serverProc.HasExited) { return $false }
    }
    return $false
}
if (-not (Wait-ServerReady)) {
    $errTxt = if (Test-Path -LiteralPath "$serverLog.err") { Get-Content -LiteralPath "$serverLog.err" -Raw } else { "" }
    Write-Host "[RED] EH probe server did not become ready. stderr: $errTxt"
    try { Stop-Process -Id $serverProc.Id -Force -ErrorAction SilentlyContinue } catch { }
    Exit-With 1
}

function Read-Result([string]$Tag) {
    if (-not (Test-Path -LiteralPath $resultsJson)) { return $null }
    try {
        $arr = Get-Content -LiteralPath $resultsJson -Raw | ConvertFrom-Json
    } catch { return $null }
    foreach ($r in $arr) { if ($r.tag -eq $Tag) { return $r } }
    return $null
}

function Kill-Tree($Proc) {
    if ($null -eq $Proc) { return }
    try { & taskkill /PID $Proc.Id /T /F 2>&1 | Out-Null } catch { }
    try { Stop-Process -Id $Proc.Id -Force -ErrorAction SilentlyContinue } catch { }
}

# ---------- 逐个浏览器验证 ----------
$records = @()
$profileRoot = Join-Path $env:TEMP ("eh-probe-$stamp")
New-Item -ItemType Directory -Force -Path $profileRoot | Out-Null
foreach ($b in $browsers) {
    $tag = $b.name
    $url = "http://127.0.0.1:$Port/__eh_probe__?tag=$tag&wasm=/$wasmRel"
    Write-Log "---- $tag : $url"
    if ($b.kind -eq "chromium") {
        $profile = Join-Path $profileRoot $tag
        New-Item -ItemType Directory -Force -Path $profile | Out-Null
        $args = @(
            "--headless=new", "--disable-gpu", "--no-sandbox", "--disable-extensions",
            "--no-first-run", "--no-default-browser-check",
            "--user-data-dir=$profile",
            $url
        )
    } else {
        # Firefox 无 --dump-dom；靠 POST 回传，headless 打开页面执行 JS 即可。
        $args = @("--headless", $url)
    }
    $browserLog = Join-Path $logRoot "eh-$tag-$stamp.log"
    $proc = Start-Process -FilePath $b.exe -ArgumentList $args -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput $browserLog -RedirectStandardError "$browserLog.err"

    $result = $null
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 400
        $result = Read-Result $tag
        if ($null -ne $result) { break }
        if ($proc.HasExited) {
            # 浏览器已退出但还没回传：再给服务器一点落盘时间。
            Start-Sleep -Milliseconds 600
            $result = Read-Result $tag
            break
        }
    }
    Kill-Tree $proc

    if ($null -eq $result) {
        Write-Log "[YELLOW] $tag : NO_RESULT within ${TimeoutSeconds}s (launched but never posted back)"
        $records += [pscustomobject]@{
            tag = $tag; exe = $b.exe; verdict = "TIMEOUT"; engine = ""; engineVersion = ""
            compile = ""; compileMs = ""; error = "no result posted within timeout"; newEhSupported = $null
        }
    } else {
        $line = "[{0}] {1} : engine={2} {3} compile={4} ({5}ms) verdict={6}" -f `
            $(if ($result.verdict -eq "PASS") { "GREEN" } else { "RED" }), $tag, $result.engine, $result.engineVersion, `
            $result.compile, $result.compileMs, $result.verdict
        Write-Log $line
        if ($result.verdict -ne "PASS" -and $result.error) { Write-Log "        error: $($result.error)" }
        $records += [pscustomobject]@{
            tag = $tag; exe = $b.exe; verdict = $result.verdict; engine = $result.engine
            engineVersion = $result.engineVersion; requiredFloor = $result.requiredFloor
            versionMeetsFloor = $result.versionMeetsFloor; compile = $result.compile
            compileMs = $result.compileMs; bytes = $result.bytes; exports = $result.exports
            newEhSupported = $result.newEhSupported; error = $result.error
        }
    }
}

# ---------- 收尾 ----------
try { Stop-Process -Id $serverProc.Id -Force -ErrorAction SilentlyContinue } catch { }
Remove-Item -Recurse -Force -Path $profileRoot -ErrorAction SilentlyContinue

$passCount = @($records | Where-Object { $_.verdict -eq "PASS" }).Count
$failCount = @($records | Where-Object { $_.verdict -eq "FAIL" -or $_.verdict -eq "NO_WASM" }).Count
$timeoutCount = @($records | Where-Object { $_.verdict -eq "TIMEOUT" }).Count

$summary = [ordered]@{
    task        = "webgl-new-eh-cross-browser"
    bundle      = $bundle
    wasm        = $wasmRel
    port        = $Port
    testedCount = $records.Count
    pass        = $passCount
    fail        = $failCount
    timeout     = $timeoutCount
    floors      = [ordered]@{ chrome = 137; edge = 137; firefox = 131; safari = "18.4"; safari_ios = "18.4" }
    records     = $records
    # Safari 无法在 Windows 上自动化：给出在 Mac 上手动验证的确切步骤。
    safariManualHint = "On macOS: node scripts/lib/wasm-lua/eh-probe-server.js <bundle> 8150 scripts/lib/wasm-lua/eh-probe.html <out.json> ; then: open -a Safari 'http://127.0.0.1:8150/__eh_probe__?tag=safari&wasm=/pkg/game_web_bg.wasm'"
}
# 证据 JSON 必须**无 BOM**：PS 5.1 的 `Set-Content -Encoding UTF8` 会加 BOM，node/CI 的
# JSON.parse 会因此报 "Unexpected token ﻿"。用 .NET UTF8Encoding($false) 显式写无 BOM。
$summaryText = ($summary | ConvertTo-Json -Depth 6)
[System.IO.File]::WriteAllText($summaryJson, $summaryText, (New-Object System.Text.UTF8Encoding($false)))
Write-Log "==> summary: $summaryJson"
Write-Log ("==> tested={0} pass={1} fail={2} timeout={3}" -f $records.Count, $passCount, $failCount, $timeoutCount)
Write-Log ("==> Safari/iOS 手动验证：{0}" -f $summary.safariManualHint)

# 判定：无任何浏览器可测已在前面 exit 3。到这里至少发现了一个浏览器。
# 有 FAIL/NO_WASM => 真红（该引擎编译不了本产物的新 EH）。
# 有 TIMEOUT（浏览器已安装却没回传）=> 也判红：本机装了浏览器却验不出结果，宁可红不可假绿。
if ($failCount -gt 0 -or $timeoutCount -gt 0) {
    Write-Host "[RED] webgl_new_eh_check_failed pass=$passCount fail=$failCount timeout=$timeoutCount"
    Exit-With 1
}
Write-Host "[GREEN] webgl_new_eh_ok tested=$($records.Count) pass=$passCount (all discovered browsers compile the new-EH wasm)"
Exit-With 0
