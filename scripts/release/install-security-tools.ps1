# install-security-tools.ps1 —— 按 tools.json 安装固定版本的安全工具（P2-01）
#
# 为什么不用 `cargo install cargo-audit`（不带版本）：门禁工具的判据会随版本变化，
# 今天绿的结论明天可能因为工具升级而变红/变绿。版本唯一事实源是 scripts/release/tools.json。
#
# 用法：
#   pwsh -NoProfile -File scripts/release/install-security-tools.ps1
#   pwsh -NoProfile -File scripts/release/install-security-tools.ps1 -Check
#   pwsh -NoProfile -File scripts/release/install-security-tools.ps1 -CargoConfig artifacts/tmp/cargo-mirror.toml
#
# 安装位置默认 `artifacts/tools/security`（已被 .gitignore 忽略）。CI 用预编译 action
# 安装同名同版本工具，不调用本脚本；本地/离线环境用本脚本保证版本一致。
#
# 退出码：0 = 工具齐且版本一致；1 = 缺失/版本不符/安装失败。

[CmdletBinding()]
param(
    [switch]$Check,
    [string]$Root = "",
    # 可选：cargo 镜像配置（crates.io CDN 不可达时用）。只透传给 cargo install。
    [string]$CargoConfig = ""
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if ([string]::IsNullOrWhiteSpace($Root)) {
    $Root = Join-Path $projectRoot "artifacts\tools\security"
}
$toolsPath = Join-Path $projectRoot "scripts\release\tools.json"
$tools = Get-Content -LiteralPath $toolsPath -Raw | ConvertFrom-Json

$specs = @(
    @{ Name = "cargo-audit"; Crate = "cargo-audit"; Version = [string]$tools.cargoAudit },
    @{ Name = "cargo-deny"; Crate = "cargo-deny"; Version = [string]$tools.cargoDeny },
    @{ Name = "cargo-cyclonedx"; Crate = "cargo-cyclonedx"; Version = [string]$tools.cargoCyclonedx }
)

function Find-Tool([string]$Name, [string]$ExpectedVersion = "") {
    $candidates = @()
    $local = Join-Path $Root ("bin\{0}.exe" -f $Name)
    if (Test-Path -LiteralPath $local) { $candidates += $local }
    $onPath = Get-Command $Name -ErrorAction SilentlyContinue
    if ($onPath) { $candidates += $onPath.Source }
    foreach ($candidate in $candidates) {
        if ([string]::IsNullOrWhiteSpace($ExpectedVersion)) { return $candidate }
        if ((Get-ToolVersion $candidate) -eq $ExpectedVersion) { return $candidate }
    }
    if ($candidates.Count -gt 0) { return $candidates[0] }
    return $null
}

function Get-ToolVersion([string]$Exe) {
    # cargo 子命令插件不一定实现顶层 --version（cargo-cyclonedx 只认
    # `cyclonedx --version`），两种形式都试，取第一个能解析出版本的结果。
    foreach ($argv in @(@("--version"), @("cyclonedx", "--version"))) {
        try {
            $raw = & $Exe @argv 2>$null
            if ($LASTEXITCODE -ne 0 -or -not $raw) { continue }
            $m = [regex]::Match(("$raw" -join " "), '(\d+\.\d+\.\d+)')
            if ($m.Success) { return $m.Groups[1].Value }
        } catch { }
    }
    return ""
}

$failed = @()
$installed = @()
foreach ($spec in $specs) {
    $exe = Find-Tool $spec.Name $spec.Version
    $found = if ($exe) { Get-ToolVersion $exe } else { "" }
    if ($found -eq $spec.Version) {
        Write-Host ("[PASS] {0} {1}" -f $spec.Name, $found) -ForegroundColor Green
        $installed += @{ Name = $spec.Name; Version = $found; Path = $exe }
        continue
    }

    if ($Check) {
        $reason = if (-not $exe) { "not found (PATH or $Root\bin)" } else { "version $found != $($spec.Version)" }
        Write-Host ("[FAIL] {0}: {1}" -f $spec.Name, $reason) -ForegroundColor Red
        $failed += $spec.Name
        continue
    }

    if ($exe -and $found -and $found -ne $spec.Version) {
        Write-Host ("[WARN] {0} {1} on PATH differs from pinned {2}; installing pinned copy to {3}" -f $spec.Name, $found, $spec.Version, $Root) -ForegroundColor Yellow
    }
    Write-Host ("==> installing {0} {1} -> {2}" -f $spec.Crate, $spec.Version, $Root) -ForegroundColor Cyan
    $cargoArgs = @()
    if (-not [string]::IsNullOrWhiteSpace($CargoConfig)) {
        $cargoArgs += @("--config", (Resolve-Path -LiteralPath $CargoConfig).Path)
    }
    $cargoArgs += @("install", "--locked", "--root", $Root, "--version", $spec.Version, $spec.Crate)
    & cargo @cargoArgs
    $code = $LASTEXITCODE
    if ($code -ne 0) {
        Write-Host ("[FAIL] cargo install {0} {1} failed (exit {2})" -f $spec.Crate, $spec.Version, $code) -ForegroundColor Red
        $failed += $spec.Name
        continue
    }
    $exe = Find-Tool $spec.Name $spec.Version
    $found = if ($exe) { Get-ToolVersion $exe } else { "" }
    if ($found -ne $spec.Version) {
        Write-Host ("[FAIL] {0}: installed but version read as '{1}' (expect {2})" -f $spec.Name, $found, $spec.Version) -ForegroundColor Red
        $failed += $spec.Name
    } else {
        Write-Host ("[PASS] {0} {1}" -f $spec.Name, $found) -ForegroundColor Green
        $installed += @{ Name = $spec.Name; Version = $found; Path = $exe }
    }
}

Write-Host ""
if ($failed.Count -gt 0) {
    Write-Host ("security_tools_failed missing_or_mismatch=" + ($failed -join ",")) -ForegroundColor Red
    Write-Host "hint: run without -Check to install; offline/mirror users may pass -CargoConfig <file>" -ForegroundColor Yellow
    exit 1
}

$rootBin = Join-Path $Root "bin"
Write-Host ("security_tools_ok audit={0} deny={1} cyclonedx={2} root={3}" -f `
        $tools.cargoAudit, $tools.cargoDeny, $tools.cargoCyclonedx, $rootBin) -ForegroundColor Green
exit 0
