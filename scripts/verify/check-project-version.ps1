# check-project-version.ps1 — 引擎与示例工程版本一致性门（P0-02 修复的机器契约）
#
# 背景（M78 §24 / §26.3-29）：版本号曾三处各说各话——`engine/Cargo.toml` = 0.1.1、
# `examples/game_jhyt/project.toml` = 0.1.1、`examples/game_api/project.toml` = 0.1.0，
# 且再生源 `editor/src/panels/header.rs` 把 `version = "0.1.0"` 写死（编辑器新建项目
# 生成 project.toml 时版本永远停在旧值）。本门把"三处一致 + 模板不写死"变成机器契约：
# 纯文件检查、秒级，`-MvpOnly` 也跑（版本漂移不该被快速路径跳过）。
#
# 退出码：0 = 一致；1 = 判红（并逐处打印读数）；-SelfTest 用于反例自测。
#
# 本脚本由 verify.ps1 以 Windows PowerShell 5.1 调用，勿使用 pwsh 7 专属语法。

[CmdletBinding()]
param(
    [string]$EngineToml,
    [string]$JhytToml,
    [string]$ApiToml,
    [string]$HeaderRs,
    [switch]$SelfTest
)

$ErrorActionPreference = 'Stop'

# scripts/verify -> 仓库根
$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)

if (-not $EngineToml) { $EngineToml = Join-Path $repoRoot "engine/Cargo.toml" }
if (-not $JhytToml)   { $JhytToml   = Join-Path $repoRoot "examples/game_jhyt/project.toml" }
if (-not $ApiToml)    { $ApiToml    = Join-Path $repoRoot "examples/game_api/project.toml" }
if (-not $HeaderRs)   { $HeaderRs   = Join-Path $repoRoot "engine/crates/editor/src/panels/header.rs" }

# 读取指定 [section] 段的 version = "x.y.z"。文件均为 UTF-8（无 BOM 或带 BOM），
# [IO.File]::ReadAllText 默认按 UTF-8 解码，BOM 字符单独剥掉防止干扰行首匹配。
function Get-SectionVersion([string]$Path, [string]$Section) {
    if (-not (Test-Path -LiteralPath $Path)) { return $null }
    $text = [IO.File]::ReadAllText($Path)
    $text = $text.TrimStart([char]0xFEFF)
    $lines = $text -split "`r?`n"
    $inside = $false
    foreach ($line in $lines) {
        if ($line -match ("^\[" + [regex]::Escape($Section) + "\]")) {
            $inside = $true
            continue
        }
        if ($inside -and $line -match '^\[[^\]]+\]') { break }
        if ($inside -and $line -match '^version\s*=\s*"([^"]+)"') { return $Matches[1] }
    }
    return $null
}

function Test-VersionConsistency([string]$EngineToml, [string]$JhytToml, [string]$ApiToml, [string]$HeaderRs) {
    $engineV = Get-SectionVersion $EngineToml "workspace.package"
    $jhytV   = Get-SectionVersion $JhytToml "project"
    $apiV    = Get-SectionVersion $ApiToml "project"

    if (-not $engineV -or -not $jhytV -or -not $apiV) {
        Write-Host "[RED] project version gate: 版本解析失败（engine=$engineV jhyt=$jhytV api=$apiV）" -ForegroundColor Red
        Write-Host "       engine=$EngineToml" -ForegroundColor Red
        Write-Host "       jhyt=$JhytToml" -ForegroundColor Red
        Write-Host "       api=$ApiToml" -ForegroundColor Red
        return 1
    }
    if ($engineV -ne $jhytV -or $engineV -ne $apiV) {
        Write-Host "[RED] project version mismatch: engine=$engineV jhyt=$jhytV api=$apiV" -ForegroundColor Red
        Write-Host "       engine 事实源: $EngineToml ([workspace.package])" -ForegroundColor Red
        Write-Host "       jhyt   工程  : $JhytToml ([project])" -ForegroundColor Red
        Write-Host "       api    工程  : $ApiToml ([project])" -ForegroundColor Red
        return 1
    }
    $headerText = [IO.File]::ReadAllText($HeaderRs)
    $headerText = $headerText.TrimStart([char]0xFEFF)
    if ([regex]::Match($headerText, 'version\s*=\s*\\?"\d+\.\d+\.\d+\\?"').Success) {
        Write-Host "[RED] header.rs 仍写死字面量版本；新建项目模板必须取 rustlua_infra::version()（当前引擎版本）" -ForegroundColor Red
        Write-Host "       file: $HeaderRs" -ForegroundColor Red
        return 1
    }
    Write-Host "project_version_ok engine=$engineV jhyt=$jhytV api=$apiV header_dynamic=true" -ForegroundColor Green
    return 0
}

if ($SelfTest) {
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("rl-version-gate-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $e = Join-Path $tmp "engine.toml"
        $j = Join-Path $tmp "jhyt.toml"
        $a = Join-Path $tmp "api.toml"
        $h = Join-Path $tmp "header.rs"
        [IO.File]::WriteAllText($e, "[workspace.package]`nversion = ""0.1.1""`n")
        [IO.File]::WriteAllText($j, "[project]`nversion = ""0.1.1""`n")
        [IO.File]::WriteAllText($a, "[project]`nversion = ""0.1.1""`n")
        [IO.File]::WriteAllText($h, "// scaffold: version = rustlua_infra::version()`n")

        $ok = Test-VersionConsistency $e $j $a $h
        if ($ok -ne 0) { throw "正例应当通过（三处一致 + header 动态）" }

        [IO.File]::WriteAllText($j, "[project]`nversion = ""0.1.0""`n")
        $mismatch = Test-VersionConsistency $e $j $a $h
        if ($mismatch -eq 0) { throw "反例 A 应当判红（jhyt 版本不一致）" }

        [IO.File]::WriteAllText($j, "[project]`nversion = ""0.1.1""`n")
        [IO.File]::WriteAllText($h, "writeln!(file, ""version = \""0.1.0\"""")`n")
        $literal = Test-VersionConsistency $e $j $a $h
        if ($literal -eq 0) { throw "反例 B 应当判红（header 写死字面量）" }

        Write-Host "project_version_gate_selftest_ok cases=3 (1 green + 2 red)" -ForegroundColor Green
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
    exit 0
}

exit (Test-VersionConsistency $EngineToml $JhytToml $ApiToml $HeaderRs)
