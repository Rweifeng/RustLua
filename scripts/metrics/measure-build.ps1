# measure-build.ps1 —— 编译时间基线（P4-05）
#
# 把"编译多久"变成可比较的数字：冷/热/增量与 per-crate 关键路径。
# 专用 CARGO_TARGET_DIR=artifacts/metrics/target-compile，不动 engine/target；
# -Cold 只清这个专用目录。至少 3 次，记录 wall time 的 min/median/max。
#
# 用法：
#   pwsh -NoProfile -File scripts/metrics/measure-build.ps1 -Mode check -Cold -Yes -Out artifacts/metrics/compile-check.json
#   pwsh -NoProfile -File scripts/metrics/measure-build.ps1 -Mode check -Iterations 1 -Out artifacts/metrics/compile-warm.json
#   pwsh -NoProfile -File scripts/metrics/measure-build.ps1 -Incremental rustlua-infra -Out artifacts/metrics/compile-incremental.json
#
# 参数：
#   -Mode check|test|release
#   -Cold          冷构建（先 cargo fetch --locked，再清专用 target 目录）
#   -Yes           冷构建确认开关（防止误清）
#   -Incremental <crate>  增量模式：cargo clean -p <crate> 后 cargo build -p game
#   -Iterations N  重复次数（默认 3）
#   -Jobs N        覆盖 CARGO_BUILD_JOBS（默认取环境或 4）
#   -Out <path>    compile-v1 JSON 输出路径
#   -SkipRun       只生成 schema 骨架不执行 cargo（自测/参数校验用）

[CmdletBinding()]
param(
    [ValidateSet("check", "test", "release")]
    [string]$Mode = "check",
    [switch]$Cold,
    [switch]$Yes,
    [string]$Incremental = "",
    [int]$Iterations = 0,
    [int]$Jobs = 0,
    [string]$Out = "artifacts/metrics/compile.json",
    [switch]$SkipRun
)

# cargo 会把 `Compiling ...` 等进度写到 stderr；PS5.1 在 Stop 下会把它当终止错误，
# 但命令成败由 $LASTEXITCODE 判定。这里保持 Continue，失败仍由 Invoke-Timed 退 1。
$ErrorActionPreference = "Continue"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$engineRoot = Join-Path $projectRoot "engine"
$targetDir = Join-Path $projectRoot "artifacts\metrics\target-compile"

if (-not (Test-Path -LiteralPath (Join-Path $engineRoot "Cargo.toml"))) {
    Write-Host "[FAIL] 找不到 engine/Cargo.toml（$engineRoot）" -ForegroundColor Red
    exit 1
}
if ($Iterations -le 0) { $Iterations = 3 }
if ($Jobs -le 0) {
    $envJobs = [int]([Environment]::GetEnvironmentVariable("CARGO_BUILD_JOBS"))
    if ($envJobs -gt 0) { $Jobs = $envJobs } else { $Jobs = 4 }
}
if ($Cold -and -not $Yes -and -not $SkipRun) {
    Write-Host "[FAIL] -Cold 必须带 -Yes（会清专用 target 目录 artifacts/metrics/target-compile）" -ForegroundColor Red
    exit 1
}

$env:CARGO_BUILD_JOBS = "$Jobs"
$env:CARGO_INCREMENTAL = "0"
$env:RUSTFLAGS = ""
$env:CARGO_TARGET_DIR = $targetDir
$env:CARGO_PROFILE_RELEASE_DEBUG = "false"

function Invoke-Timed([string]$Label, [scriptblock]$Body, [int]$N) {
    $times = @()
    for ($i = 0; $i -lt $N; $i++) {
        if ($i -gt 0) { Write-Host ("  repeat {0}/{1} ..." -f ($i + 1), $N) }
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        & $Body
        if ($LASTEXITCODE -ne 0) {
            Write-Host ("[FAIL] {0} 执行失败（exit {1}）" -f $Label, $LASTEXITCODE) -ForegroundColor Red
            exit 1
        }
        $sw.Stop()
        $times += $sw.ElapsedMilliseconds
    }
    $sorted = @($times | Sort-Object)
    $mean = ($times | Measure-Object -Average).Average
    $var = ($times | ForEach-Object { ($_ - $mean) * ($_ - $mean) } | Measure-Object -Average).Average
    return @{
        n = $N
        min_ms = $sorted[0]
        median_ms = $sorted[[int]($N / 2)]
        max_ms = $sorted[-1]
        variance_ms2 = $var
    }
}

function Get-CargoCmd {
    param([string]$M)
    if ($M -eq "check") { return "cargo", "check", "--workspace", "--all-targets", "--locked" }
    if ($M -eq "test") { return "cargo", "test", "--workspace", "--all-features", "--locked", "--no-fail-fast" }
    return "cargo", "build", "--workspace", "--release", "--locked"
}

$result = [ordered]@{
    schema = "compile-v1"
    generated_at = (Get-Date -Format "o")
    profile = $Mode
    jobs = $Jobs
    incremental = 0
    cold = [bool]$Cold
    target_dir = $targetDir
    rustc = (& rustc --version 2>$null) -join ""
}

Write-Host ("measure-build: mode={0} cold={1} jobs={2} iterations={3} target={4}" -f $Mode, $Cold, $Jobs, $Iterations, $targetDir)

if (-not $SkipRun) {
    if ($Cold) {
        Write-Host "  cargo fetch --locked ..."
        Push-Location $engineRoot
        & cargo fetch --locked
        if ($LASTEXITCODE -ne 0) { Pop-Location; Write-Host "[FAIL] cargo fetch" -ForegroundColor Red; exit 1 }
        Pop-Location
        Write-Host "  cleaning dedicated target dir $targetDir ..."
        if (Test-Path -LiteralPath $targetDir) { Remove-Item -LiteralPath $targetDir -Recurse -Force }
    }
}

if ($Incremental) {
    # 增量模式：清指定 crate 后构建 game（模拟"改一行"的重建半径）
    $result.incremental = 1
    $result.incremental_crate = $Incremental
    Push-Location $engineRoot
    & cargo clean -p $Incremental 2>$null | Out-Null
    $t = Invoke-Timed "incremental build -p game" {
        & cargo build -p game --locked 2>$null | Out-Null
    } $Iterations
    Pop-Location
    $result.incremental_after_clean_package = $t.median_ms
    $result.units = [ordered]@{ incremental_rebuild = 1 }
} else {
    $key = switch ($Mode) {
        "check" { if ($Cold) { "check_cold" } else { "check_warm" } }
        "test" { "test_build_cold" }
        "release" { "release_build_cold" }
    }
    if (-not $SkipRun) {
        $cmd = @(Get-CargoCmd -M $Mode)
        $cargoArgs = @()
        if ($cmd.Count -gt 1) { $cargoArgs = @($cmd[1..($cmd.Count - 1)]) }
        $t = Invoke-Timed ("cargo {0} ({1})" -f $cmd[0], $Mode) {
            Push-Location $engineRoot
            & $cmd[0] @cargoArgs 2>$null | Out-Null
            Pop-Location
        } $Iterations
        $result[$key] = $t.median_ms
        $result["${key}_min_ms"] = $t.min_ms
        $result["${key}_max_ms"] = $t.max_ms
        $result["${key}_variance_ms2"] = $t.variance_ms2
    } else {
        $result[$key] = 0
        $result["${key}_min_ms"] = 0
        $result["${key}_max_ms"] = 0
        $result["${key}_variance_ms2"] = 0
    }
}

$outPath = Join-Path $projectRoot $Out
New-Item -ItemType Directory -Force -Path (Split-Path $outPath) | Out-Null
$json = $result | ConvertTo-Json -Depth 8
[IO.File]::WriteAllText($outPath, $json, (New-Object Text.UTF8Encoding $false))
Write-Host ("compile_ok mode={0} cold={1} iterations={2} out={3}" -f $Mode, $Cold, $Iterations, $outPath) -ForegroundColor Green
exit 0
