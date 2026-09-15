[CmdletBinding()]
param(
    [switch]$SkipClippy,
    [switch]$SkipTests,
    [switch]$SkipBuild,
    [switch]$Release,
    [switch]$KeepLog,
    # MVP 退出门（M19-04）：在常规质量门之后追加 headless 游戏回归、浸泡/性能
    # 基线与干净发行目录自测（package.ps1 -SelfTest）。
    [switch]$Mvp,
    # 仅跑 MVP 追加部分（跳过 fmt/clippy/test/build 常规门）。
    [switch]$MvpOnly
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent $PSScriptRoot
$engineRoot = Join-Path $projectRoot "engine"
$gameRoot = Join-Path $projectRoot "examples"
# 工程校验与干净目录自测的默认目标工程：examples 重组后样板工程在子目录里
# （`examples/game_jhyt`），根目录已没有 project.toml——此前写死 `examples` 会让
# 「Game project check」必然失败。
$defaultGameProject = Join-Path $gameRoot "game_jhyt"
$logRoot = Join-Path $projectRoot "artifacts\logs\quality"
$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$logPath = Join-Path $logRoot "verify-$timestamp.log"

New-Item -ItemType Directory -Force -Path $logRoot | Out-Null

function Write-Step([string]$Message) {
    Write-Host "`n==> $Message" -ForegroundColor Cyan
}

function Invoke-Checked([string]$File, [string[]]$Arguments) {
    $command = "$File $($Arguments -join ' ')"
    Write-Host "> $command" -ForegroundColor DarkGray

    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $output = & $File @Arguments 2>&1
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = $previousPreference

    $output | ForEach-Object {
        $line = $_.ToString()
        Write-Host $line
        Add-Content -Path $logPath -Value $line -Encoding UTF8
    }

    if ($exitCode -ne 0) {
        throw "Command failed (exit code $exitCode): $command"
    }
}

# package.ps1 必须用**具名参数**调用：PowerShell 的数组 splat（`@array`）是按位置传参，
# 数组里的 `-SelfTest` 会被当作 `-Platform` 的值而触发 ValidateSet 失败
# （`Cannot validate argument on parameter 'Platform'`）。
function Invoke-PackageSelfTest([string]$ProjectRootPath) {
    $packageScript = Join-Path $PSScriptRoot "package.ps1"
    Write-Host "> powershell -NoProfile -ExecutionPolicy Bypass -File $packageScript -ProjectRoot $ProjectRootPath -SelfTest" -ForegroundColor DarkGray

    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    # 子进程执行：package.ps1 大量使用 Write-Host（不经过管道，管道里捕获不到），
    # 只有子进程的 stdout 才能完整写进质量日志。
    $output = & powershell -NoProfile -ExecutionPolicy Bypass -File $packageScript -ProjectRoot $ProjectRootPath -SelfTest 2>&1
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = $previousPreference

    $output | ForEach-Object {
        $line = $_.ToString()
        Write-Host $line
        Add-Content -Path $logPath -Value $line -Encoding UTF8
    }

    if ($exitCode -ne 0) {
        throw "Package self test failed (exit code $exitCode): $packageScript -ProjectRoot $ProjectRootPath -SelfTest"
    }
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo was not found. Install Rust stable and ensure cargo is on PATH."
}

$runGates = -not $MvpOnly

# 文档一致性门（P4）：编号唯一、H1、索引可达、相对链接、历史路径、UTF-8 可解码。
# 放在最前面：它只读文本、约 1 秒，且 `-MvpOnly` 也跑（文档失真不该被快速路径跳过）。
Write-Step "Documentation consistency"
$docsScript = Join-Path $projectRoot "scripts/check-docs.ps1"
if (Test-Path $docsScript) {
    $docsOutput = & powershell -NoProfile -ExecutionPolicy Bypass -File $docsScript
    $docsExit = $LASTEXITCODE
    $docsOutput | ForEach-Object { Write-Host $_ }
    if ($docsExit -ne 0) {
        throw "check-docs.ps1 failed (exit code $docsExit)"
    }
}

if ($runGates) {
Push-Location $engineRoot
try {
    Write-Step "Cargo metadata"
    Invoke-Checked cargo @("metadata", "--no-deps", "--format-version", "1")

    Write-Step "Rust formatting"
    Invoke-Checked cargo @("fmt", "--all", "--", "--check")

    if (-not $SkipClippy) {
        Write-Step "Clippy"
        Invoke-Checked cargo @("clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings")
    }

    if (-not $SkipTests) {
        Write-Step "Workspace tests"
        Invoke-Checked cargo @("test", "--workspace", "--all-features")
    }

    if (-not $SkipBuild) {
        if ($Release) {
            Write-Step "Release build"
            Invoke-Checked cargo @("build", "--workspace", "--release")
        }
        else {
            Write-Step "Debug build"
            Invoke-Checked cargo @("build", "--workspace")
        }
    }

    Write-Step "Git diff check"
    Push-Location $projectRoot
    try {
        Invoke-Checked git @("diff", "--check")
    }
    finally {
        Pop-Location
    }

    Write-Step "Game project check"
    Push-Location $projectRoot
    try {
        Invoke-Checked cargo @("run", "--quiet", "--manifest-path", "engine/Cargo.toml", "-p", "game", "--", "--project", $defaultGameProject, "--check")
    }
    finally {
        Pop-Location
    }

    Write-Step "Editor project check"
    Push-Location $projectRoot
    try {
        Invoke-Checked cargo @("run", "--quiet", "--manifest-path", "engine/Cargo.toml", "-p", "editor", "--", "--project", $defaultGameProject, "--check")
    }
    finally {
        Pop-Location
    }

    Write-Host "`nQuality gate passed. Log: $logPath" -ForegroundColor Green
}
finally {
    Pop-Location
}
}

if ($Mvp -or $MvpOnly) {
    # MVP 退出门（M19-04）：核心剧本/浸泡 headless 回归 + 干净发行目录自测。
    # 清单是**唯一事实来源**：清单里的样例缺失即失败（此前是打印 skip 后 continue，
    # 会让"少跑几个样例"看起来仍然绿灯，等于把质量门变成摆设）。
    Write-Step "MVP headless regression suite"
    $examples = @(
        "jhyt_smoke",
        "jhyt_m17",
        "jhyt_m17_npc",
        "jhyt_m17_dialogue",
        "jhyt_m17_shop",
        "jhyt_m18_combat",
        "jhyt_m18_stats",
        "jhyt_m18_build",
        "jhyt_m19_slice",
        "jhyt_m19_soak",
        "jhyt_m19_ui",
        "jhyt_m20_audio",
        "jhyt_m20_gamepad",
        "jhyt_m21_appearance",
        "jhyt_m22_input",
        "jhyt_m22_save_isolation",
        "jhyt_m23_data_validation",
        "jhyt_m24_numeric_alignment",
        "jhyt_m25_combat_feedback",
        "jhyt_m26_hud_feedback",
        "jhyt_m27_goals",
        "jhyt_m28_loop",
        "jhyt_m29_season_weather",
        "jhyt_m30_cjk_text",
        "jhyt_m31_pc_release"
    )
    $exampleDir = Join-Path $engineRoot "crates\game\examples"

    # 1) 先全量校验清单，缺失/多余都直接失败（缺失 = 回归消失，多余 = 清单漏登记）。
    $missing = @()
    foreach ($example in $examples) {
        $source = Join-Path $exampleDir ($example + ".rs")
        if (-not (Test-Path -LiteralPath $source)) {
            $missing += $example
        }
    }
    if ($missing.Count -gt 0) {
        throw ("MVP example list is missing $($missing.Count) file(s): " + ($missing -join ", ") + " (expected under $exampleDir)")
    }
    $onDisk = @(Get-ChildItem -LiteralPath $exampleDir -Filter "jhyt_*.rs" -File | ForEach-Object { $_.BaseName })
    $unlisted = @($onDisk | Where-Object { $examples -notcontains $_ })
    if ($unlisted.Count -gt 0) {
        throw ("jhyt_* example(s) exist but are not in the MVP list: " + ($unlisted -join ", ") + " (add them to verify.ps1 or delete them)")
    }

    # 2) 逐项执行，并断言"执行数量 == 清单数量"。
    $executed = 0
    Push-Location $engineRoot
    try {
        foreach ($example in $examples) {
            Invoke-Checked cargo @("run", "--quiet", "-p", "game", "--example", $example)
            $executed++
        }
    }
    finally {
        Pop-Location
    }
    if ($executed -ne $examples.Count) {
        throw "MVP gate ran $executed example(s) but the manifest lists $($examples.Count); a silent skip is never acceptable."
    }
    Write-Host "MVP headless regression: executed=$executed expected=$($examples.Count)" -ForegroundColor Green

    Write-Step "Clean release directory self test"
    Invoke-PackageSelfTest -ProjectRootPath $defaultGameProject

    Write-Host "`nMVP gate passed. Log: $logPath" -ForegroundColor Green
}

if (-not $KeepLog) {
    Write-Host "Quality log retained for review and regression." -ForegroundColor DarkGray
}

