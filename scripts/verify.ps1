[CmdletBinding()]
param(
    [switch]$SkipClippy,
    [switch]$SkipTests,
    [switch]$SkipBuild,
    [switch]$Release,
    [switch]$KeepLog
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent $PSScriptRoot
$engineRoot = Join-Path $projectRoot "engine"
$gameRoot = Join-Path $projectRoot "game_jianghu_youtian"
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

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo was not found. Install Rust stable and ensure cargo is on PATH."
}

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
        Invoke-Checked cargo @("run", "--quiet", "--manifest-path", "engine/Cargo.toml", "-p", "game", "--", "--project", $gameRoot, "--check")
    }
    finally {
        Pop-Location
    }

    Write-Step "Editor project check"
    Push-Location $projectRoot
    try {
        Invoke-Checked cargo @("run", "--quiet", "--manifest-path", "engine/Cargo.toml", "-p", "editor", "--", "--project", $gameRoot, "--check")
    }
    finally {
        Pop-Location
    }

    Write-Host "`nQuality gate passed. Log: $logPath" -ForegroundColor Green
}
finally {
    Pop-Location
}

if (-not $KeepLog) {
    Write-Host "Quality log retained for review and regression." -ForegroundColor DarkGray
}

