# check-project-version.ps1 — 引擎与示例工程版本一致性门（P0-02 修复的机器契约）
#
# 背景（M78 §24 / §26.3-29）：版本号曾三处各说各话——`engine/Cargo.toml` = 0.1.1、
# `examples/game_jhyt/project.toml` = 0.1.1、`examples/game_api/project.toml` = 0.1.0，
# 且再生源 `editor/src/panels/header.rs` 把 `version = "0.1.0"` 写死（编辑器新建项目
# 生成 project.toml 时版本永远停在旧值）。本门把"三处一致 + 模板不写死 + 发布产物
# 指纹（build-info.txt）一致"变成机器契约：纯文件检查、秒级，`-MvpOnly` 也跑
# （版本漂移不该被快速路径跳过）。
#
# build-info.txt（`examples/**/builds/*/build-info.txt`，第 5 行 `engine version: x.y.z`）
# 是本地产物指纹：它在就比对，缺失是常态（builds/ 不入库）——显式 SKIP 不算红，
# 但"存在却版本不符/解析不出"判红。
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
    [string[]]$BuildInfoFiles,
    [switch]$SelfTest
)

$ErrorActionPreference = 'Stop'

# scripts/verify -> 仓库根
$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)

if (-not $EngineToml) { $EngineToml = Join-Path $repoRoot "engine/Cargo.toml" }
if (-not $JhytToml)   { $JhytToml   = Join-Path $repoRoot "examples/game_jhyt/project.toml" }
if (-not $ApiToml)    { $ApiToml    = Join-Path $repoRoot "examples/game_api/project.toml" }
if (-not $HeaderRs)   { $HeaderRs   = Join-Path $repoRoot "engine/crates/editor/src/panels/header.rs" }
if (-not $BuildInfoFiles -or $BuildInfoFiles.Count -eq 0) {
    # 默认扫描 examples/ 下所有构建产物的 build-info.txt；builds/ 不入库，缺文件是常态。
    $BuildInfoFiles = @(
        Get-ChildItem -LiteralPath (Join-Path $repoRoot "examples") -Recurse -Filter "build-info.txt" -File -ErrorAction SilentlyContinue |
            ForEach-Object { $_.FullName }
    )
}

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

function Get-BuildInfoVersions([string[]]$Files) {
    $out = @()
    foreach ($f in $Files) {
        $rec = [pscustomobject]@{ Path = $f; Version = $null; Missing = $false; ParseFail = $false }
        if (-not (Test-Path -LiteralPath $f)) { $rec.Missing = $true; $out += $rec; continue }
        $t = [IO.File]::ReadAllText($f)
        $t = $t.TrimStart([char]0xFEFF)
        if ($t -match '(?m)^engine version:\s*([0-9]+\.[0-9]+\.[0-9]+)') { $rec.Version = $Matches[1] }
        else { $rec.ParseFail = $true }
        $out += $rec
    }
    return $out
}

function Test-VersionConsistency([string]$EngineToml, [string]$JhytToml, [string]$ApiToml, [string]$HeaderRs, [string[]]$BuildInfoFiles) {
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

    # build-info.txt 产物指纹：存在则必须 == engineV；缺失是常态，显式 SKIP 不判红。
    $bi = @(Get-BuildInfoVersions $BuildInfoFiles)
    $biBad = @($bi | Where-Object { (-not $_.Missing) -and ($_.ParseFail -or ($_.Version -ne $engineV)) })
    $biMissing = @($bi | Where-Object { $_.Missing })
    if ($biBad.Count -gt 0) {
        Write-Host "[RED] build-info.txt 的 engine version 与 workspace 不一致或解析失败：" -ForegroundColor Red
        foreach ($b in $biBad) {
            $shown = if ($b.Version) { $b.Version } else { "PARSE_FAIL" }
            Write-Host "       $($b.Path) -> $shown（期望 $engineV）" -ForegroundColor Red
        }
        return 1
    }
    foreach ($b in $biMissing) {
        Write-Host "[SKIP] build-info.txt 不存在（本地产物未构建，跳过比对）: $($b.Path)" -ForegroundColor Yellow
    }
    Write-Host "build_info_ok files=$($bi.Count) ok=$($bi.Count - $biMissing.Count) missing=$($biMissing.Count)" -ForegroundColor Green
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
        $b = Join-Path $tmp "build-info.txt"
        [IO.File]::WriteAllText($e, "[workspace.package]`nversion = ""0.1.1""`n")
        [IO.File]::WriteAllText($j, "[project]`nversion = ""0.1.1""`n")
        [IO.File]::WriteAllText($a, "[project]`nversion = ""0.1.1""`n")
        [IO.File]::WriteAllText($h, "// scaffold: version = rustlua_infra::version()`n")
        [IO.File]::WriteAllText($b, "RustLua game engine release package`nengine version: 0.1.1`n")

        $ok = Test-VersionConsistency $e $j $a $h @($b)
        if ($ok -ne 0) { throw "正例应当通过（三处一致 + header 动态 + build-info 一致）" }

        [IO.File]::WriteAllText($j, "[project]`nversion = ""0.1.0""`n")
        $mismatch = Test-VersionConsistency $e $j $a $h @($b)
        if ($mismatch -eq 0) { throw "反例 A 应当判红（jhyt 版本不一致）" }

        [IO.File]::WriteAllText($j, "[project]`nversion = ""0.1.1""`n")
        [IO.File]::WriteAllText($h, "writeln!(file, ""version = \""0.1.0\"""")`n")
        $literal = Test-VersionConsistency $e $j $a $h @($b)
        if ($literal -eq 0) { throw "反例 B 应当判红（header 写死字面量）" }

        # 恢复干净的 header（动态取版本），后续反例只针对 build-info，避免先因 header 红而误判。
        [IO.File]::WriteAllText($h, "// scaffold: version = rustlua_infra::version()`n")
        [IO.File]::WriteAllText($b, "RustLua game engine release package`nengine version: 0.1.0`n")
        $biMismatch = Test-VersionConsistency $e $j $a $h @($b)
        if ($biMismatch -eq 0) { throw "反例 C 应当判红（build-info 版本不一致）" }

        [IO.File]::WriteAllText($b, "RustLua game engine release package`nplatform: pc`n")
        $biParse = Test-VersionConsistency $e $j $a $h @($b)
        if ($biParse -eq 0) { throw "反例 D 应当判红（build-info 无 engine version 行）" }

        [IO.File]::WriteAllText($b, "RustLua game engine release package`nengine version: 0.1.1`n")
        $missingOk = Test-VersionConsistency $e $j $a $h @((Join-Path $tmp "no-build-info.txt"))
        if ($missingOk -ne 0) { throw "缺失 build-info 应当 SKIP 不红" }

        Write-Host "project_version_gate_selftest_ok cases=4 (1 green + 3 red) build_info_missing_is_skip ok" -ForegroundColor Green
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
    exit 0
}

exit (Test-VersionConsistency $EngineToml $JhytToml $ApiToml $HeaderRs $BuildInfoFiles)
