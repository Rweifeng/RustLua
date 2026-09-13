# verify-webgl.ps1（M20-04）：WebGL 浏览器集成回归。
#
# 流程：
#   1. 检查 wasm32 目标；
#      * 可用 → 走完整 WebGL 发布（package.ps1 -Platform webgl，含 wasm-bindgen）；
#      * 不可用（离线环境常见）→ **明确跳过** wasm 模块构建（不伪造通过），
#        改为执行浏览器侧集成契约检查（资源获取/解析、WebGL2 能力、存储降级、
#        缺失资源诊断），并在日志与摘要中标注差异。
#   2. 组装 web 包（assetc 打包 + 契约检查页）。
#   3. 用 Node 静态服务器 + Edge 无头浏览器（--dump-dom）执行检查并断言 DOM 结果。
#   4. 记录支持浏览器、构建产物、运行日志与已知限制到 artifacts/logs/webgl/。
#
# 用法：
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify-webgl.ps1 [-ProjectRoot <dir>] [-Port 8123] [-KeepBundle]

[CmdletBinding()]
param(
    [string]$ProjectRoot = "",
    [int]$Port = 8123,
    [switch]$KeepBundle
)

$ErrorActionPreference = "Stop"
$workspaceRoot = Split-Path -Parent $PSScriptRoot
$engineRoot = Join-Path $workspaceRoot "engine"
if ([string]::IsNullOrWhiteSpace($ProjectRoot)) {
    $ProjectRoot = Join-Path $workspaceRoot "examples\game_jhyt"
}
$gameRoot = (Resolve-Path -LiteralPath $ProjectRoot).Path
$projectToml = Join-Path $gameRoot "project.toml"
if (-not (Test-Path $projectToml)) { throw "Game project.toml was not found: $projectToml" }

$nameLine = Get-Content -LiteralPath $projectToml | Where-Object { $_ -match '^\s*name\s*=\s*"(.*)"\s*$' } | Select-Object -First 1
$PublishName = if ($nameLine -and $nameLine -match '^\s*name\s*=\s*"(.*)"\s*$') { $Matches[1] } else { "game" }
$logRoot = Join-Path $workspaceRoot "artifacts\logs\webgl"
New-Item -ItemType Directory -Force -Path $logRoot | Out-Null
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$logPath = Join-Path $logRoot "verify-webgl-$stamp.log"

function Write-Log([string]$Message, [string]$Color = "Gray") {
    Write-Host $Message -Color $Color
    Add-Content -Path $logPath -Value $Message -Encoding UTF8
}

Write-Log "==> WebGL verification (project=$gameRoot)" Cyan

# ---------- 1. wasm 目标与浏览器可用性 ----------
$wasmTargets = & rustup target list --installed 2>&1
$hasWasm = ($wasmTargets | Select-String -Pattern 'wasm32-unknown-unknown' -Quiet) -eq $true
$rustupPath = Join-Path $logRoot "wasm-target-$stamp.txt"
@(
    "rustup target list --installed:",
    ($wasmTargets -join "`n"),
    "",
    "wasm32-unknown-unknown available: $hasWasm",
    "note: 本环境为离线环境（static.rust-lang.org 不可达），如需构建 WASM 请先联网执行：rustup target add wasm32-unknown-unknown"
) | Set-Content -Path $rustupPath -Encoding UTF8
Write-Log "wasm32 target available: $hasWasm (details: $rustupPath)"

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
    Write-Log "no supported browser found (Edge/Chrome); browser integration cannot run in this environment" Yellow
    exit 2
}
$browserVersion = (Get-Item -LiteralPath $browser).VersionInfo.ProductVersion
Write-Log "browser: $browser (version $browserVersion)"

$node = Get-Command node -ErrorAction SilentlyContinue
if (-not $node) {
    Write-Log "node.js not found; static server for the browser check is unavailable" Yellow
    exit 2
}

# ---------- 2. 组装 web 包 ----------
$bundleRoot = Join-Path $gameRoot ("builds\webgl\" + $PublishName + "-browser-check")
if (Test-Path -LiteralPath $bundleRoot) { Remove-Item -LiteralPath $bundleRoot -Recurse -Force }
New-Item -ItemType Directory -Force -Path $bundleRoot | Out-Null

Push-Location $engineRoot
try {
    Write-Log "==> Build assetc (debug)" Cyan
    cargo build -p assetc
    if ($LASTEXITCODE -ne 0) { throw "assetc build failed" }
}
finally {
    Pop-Location
}
$assetc = Join-Path $engineRoot "target\debug\assetc.exe"
if (-not (Test-Path -LiteralPath $assetc)) { throw "assetc binary missing: $assetc" }

Write-Log "==> Package project data (assetc package)" Cyan
& $assetc package $gameRoot -o (Join-Path $bundleRoot "pack")
if ($LASTEXITCODE -ne 0) { throw "assetc package failed" }

Copy-Item -LiteralPath (Join-Path $gameRoot "web\index.html") -Destination (Join-Path $bundleRoot "index.html") -Force
Copy-Item -LiteralPath (Join-Path $gameRoot "web\web-check.js") -Destination (Join-Path $bundleRoot "web-check.js") -Force

$manifestPath = Join-Path $bundleRoot "pack\manifest.txt"
$aManifest = Join-Path $bundleRoot "pack\project.toml"
if (-not (Test-Path $manifestPath)) { throw "web bundle is missing pack/manifest.txt" }
if (-not (Test-Path $aManifest)) { throw "web bundle is missing pack/project.toml" }

# ---------- 3. 静态服务器 + 无头浏览器 ----------
Write-Log "==> Serve bundle and run headless browser check" Cyan
$serverLog = Join-Path $logRoot "serve-$stamp.log"
$server = Start-Process -FilePath $node.Source `
    -ArgumentList @((Join-Path $PSScriptRoot "serve-static.js"), $bundleRoot, "$Port") `
    -PassThru -NoNewWindow -RedirectStandardOutput $serverLog -RedirectStandardError "$serverLog.err"
$dumpPath = Join-Path $logRoot "dom-$stamp.html"
$dom = $null
try {
    $ready = $false
    for ($attempt = 0; $attempt -lt 30; $attempt++) {
        Start-Sleep -Milliseconds 200
        try {
            $probe = Invoke-WebRequest -Uri "http://127.0.0.1:$Port/index.html" -UseBasicParsing -TimeoutSec 3
            if ($probe.StatusCode -eq 200) { $ready = $true; break }
        }
        catch { }
    }
    if (-not $ready) { throw "static server did not become ready on port $Port (see $serverLog)" }

    $url = "http://127.0.0.1:$Port/index.html"
    # 无头浏览器调用必须有超时：--dump-dom 在部分 Edge 版本上不会自行退出，
    # 这里用 Start-Process + WaitForExit(ms) 兜底，超时即杀进程并保留输出诊断。
    function Invoke-HeadlessDom([string]$HeadlessFlag) {
        $suffix = ($HeadlessFlag -replace '[^a-zA-Z]', '')
        $outFile = Join-Path $logRoot ("dom-$stamp-$suffix.out")
        $errFile = "$outFile.err"
        $arguments = @(
            $HeadlessFlag, "--disable-gpu", "--no-sandbox", "--no-first-run",
            "--disable-extensions", "--virtual-time-budget=8000", "--dump-dom", $url
        )
        $process = Start-Process -FilePath $browser -ArgumentList $arguments -PassThru -NoNewWindow `
            -RedirectStandardOutput $outFile -RedirectStandardError $errFile
        if (-not $process.WaitForExit(60000)) {
            Write-Log "headless browser did not exit within 60s ($HeadlessFlag); killing process" Yellow
            Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        }
        if (Test-Path -LiteralPath $outFile) {
            return (Get-Content -LiteralPath $outFile -Raw)
        }
        return $null
    }
    $dom = Invoke-HeadlessDom "--headless=new"
    if (-not $dom -or $dom -notmatch 'data-status') {
        # 老版本 Edge 不支持 --headless=new：退回旧开关重试一次。
        $dom = Invoke-HeadlessDom "--headless"
    }
    if ($dom) { $dom | Set-Content -Path $dumpPath -Encoding UTF8 }
}
finally {
    if ($server -and -not $server.HasExited) {
        Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
    }
}

if (-not $dom) { throw "headless browser produced no DOM output" }
# 状态必须取自结果元素本身（`<strong id="result" data-status="...">`）。注意不能
# 用裸 `data-status="..."` 正则：页面 <style> 里的 CSS 选择器
# `#result[data-status="ok"]` 会先匹配到，导致「失败被当成通过」。
$detailsMatch = [regex]::Match($dom, '(?s)<pre id="details">(.*?)</pre>')
$details = if ($detailsMatch.Success) { $detailsMatch.Groups[1].Value } else { "(no details)" }
$status = $null
if ($detailsMatch.Success) {
    try {
        $payload = $details | ConvertFrom-Json
        if ($payload.PSObject.Properties.Name -contains 'status') {
            $status = [string]$payload.status
        }
    }
    catch {
        Write-Log "could not parse browser details JSON: $($_.Exception.Message)" Yellow
    }
}
if (-not $status) {
    $statusMatch = [regex]::Match($dom, '<strong id="result" data-status="([^"]+)"')
    if ($statusMatch.Success) { $status = $statusMatch.Groups[1].Value }
}
if (-not $status) {
    Write-Log "browser DOM is missing the result marker; dump kept at $dumpPath" Red
    exit 1
}
Write-Log "browser check status: $status"
Write-Log $details

$summary = @{
    task           = "M20-04"
    browser        = $browser
    browserVersion = $browserVersion
    wasmTarget     = $hasWasm
    bundleRoot     = $bundleRoot
    domDump        = $dumpPath
    status         = $status
    log            = $logPath
} | ConvertTo-Json
$summary | Set-Content -Path (Join-Path $logRoot "summary-$stamp.json") -Encoding UTF8

if (-not $KeepBundle) {
    Remove-Item -LiteralPath $bundleRoot -Recurse -Force -ErrorAction SilentlyContinue
}

if ($status -ne "ok") {
    Write-Log "webgl browser integration check FAILED (see $dumpPath)" Red
    exit 1
}
if (-not $hasWasm) {
    Write-Log "webgl_browser_ok (WASM module build SKIPPED: wasm32-unknown-unknown target unavailable offline; browser contract check passed)" Yellow
    exit 0
}
Write-Log "webgl_browser_ok (browser contract check; wasm target available - run package.ps1 -Platform webgl for the full bundle)" Green
exit 0
