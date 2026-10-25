# release-notes.ps1 —— 生成 GitHub Release 说明（P2-01 ⑤）
#
# 说明只由三样东西组成，避免"手写一段和产物对不上"：
#   1. 变更记录里对应版本的段落（`docs/changelog/版本历史.md`；找不到就退到
#      `docs/changelog/最新条目.md` 并**聚合全部 Unreleased 批次**，批次标题去掉
#      "Unreleased" 前缀、按出现顺序保留，明确写出来源与条数）；
#   2. 构建证据（commit / 工具链 / SBOM 文件哈希）；
#   3. 每个发布资产的字节数与 SHA-256。
#
# 破坏性变更约定（§26.3-28）：批次内写 `### 破坏性变更` 子标题即为机器可见的
# breaking 通道——聚合时会检测该子标题并在「变更」区顶部给出醒目提示。
#
# 用法：
#   pwsh -NoProfile -File scripts/release/release-notes.ps1 -Version 0.1.0 -Assets a.zip,b.zip
#   pwsh -NoProfile -File scripts/release/release-notes.ps1 -Version 0.1.0 -Assets <dir> -Output notes.md

[CmdletBinding()]
param(
    [string]$Version = "",
    [string[]]$Assets = @(),
    [string]$Output = "",
    [string]$Sbom = "artifacts/reports/rustlua.cdx.json"
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)

function Get-WorkspaceVersion {
    $toml = Join-Path $projectRoot "engine\Cargo.toml"
    $m = Select-String -Path $toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if ($m) { return $m.Matches[0].Groups[1].Value }
    return "0.0.0"
}

if ([string]::IsNullOrWhiteSpace($Version)) { $Version = Get-WorkspaceVersion }
if ([string]::IsNullOrWhiteSpace($Output)) {
    $Output = Join-Path $projectRoot ("artifacts\release-notes-v" + $Version + ".md")
}
# `pwsh -File` 不会把 PowerShell 数组绑定到 [string[]] 参数（只传第一个元素，其余变成
# positional）。允许逗号/分号分隔的单个字符串，CI 用 `-Assets ($assets -join ',')`。
$Assets = @($Assets | ForEach-Object { $_ -split '[,;]' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })

# 从变更记录文本里找 `## vX.Y.Z` / `## X.Y.Z` / `## [X.Y.Z]`；找不到则用第一条 Unreleased。
function Get-ChangelogSection([string]$Text, [string]$Ver) {
    $lines = $Text -split "`r?`n"
    $escaped = [regex]::Escape($Ver)
    $patterns = @(
        ('^##\s+\[?v?' + $escaped + '\]?(?:\s|$)'),
        '^##\s+Unreleased(?:\s|$)'
    )
    foreach ($pattern in $patterns) {
        $start = -1
        for ($i = 0; $i -lt $lines.Count; $i++) {
            if ($lines[$i] -match $pattern) { $start = $i; break }
        }
        if ($start -lt 0) { continue }
        $end = $lines.Count
        for ($j = $start + 1; $j -lt $lines.Count; $j++) {
            if ($lines[$j] -match '^##\s+') { $end = $j; break }
        }
        $body = ($lines[$start..($end - 1)] -join "`n").Trim()
        $kind = if ($pattern -match 'Unreleased') { "unreleased" } else { "version" }
        return [pscustomobject]@{ Kind = $kind; Body = $body }
    }
    return $null
}

# 聚合 `## Unreleased — …` 的全部批次（§26.3-28 兜底）：每条保留原批次内容，
# 但标题去掉 "Unreleased — " 前缀、降为 `### `，避免把 Unreleased 字样带到发布页，
# 且不再只取第一条——9+ 条批次一条都不能丢。
function Get-UnreleasedBatches([string]$Text) {
    $lines = $Text -split "`r?`n"
    $batches = @()
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^##\s+Unreleased(?:\s|$)') {
            $j = $i + 1
            while ($j -lt $lines.Count -and $lines[$j] -notmatch '^##\s+') { $j++ }
            $batchBody = ($lines[$i..($j - 1)] -join "`n").Trim()
            $batchBody = $batchBody -replace '^##\s+Unreleased\s*—\s*', '### '
            $batches += [pscustomobject]@{ Body = $batchBody }
            $i = $j - 1
        }
    }
    return $batches
}

$section = $null
$sectionSource = ""
# 1) 版本历史.md 里找版本段（那是"真正的版本段落"）。
$historyPath = Join-Path $projectRoot "docs\changelog\版本历史.md"
if (Test-Path -LiteralPath $historyPath) {
    $candidate = Get-ChangelogSection ([System.IO.File]::ReadAllText($historyPath, [System.Text.UTF8Encoding]::new($false))) $Version
    if ($candidate) {
        $section = $candidate
        $sectionSource = "docs\changelog\版本历史.md"
    }
}
# 2) 没有版本段 → 聚合最新条目.md 的**全部** Unreleased 批次（不是只取第一条）。
if (-not $section) {
    $latestPath = Join-Path $projectRoot "docs\changelog\最新条目.md"
    if (Test-Path -LiteralPath $latestPath) {
        $batches = @(Get-UnreleasedBatches ([System.IO.File]::ReadAllText($latestPath, [System.Text.UTF8Encoding]::new($false))))
        if ($batches.Count -gt 0) {
            $joined = ($batches | ForEach-Object { $_.Body }) -join "`n`n"
            $section = [pscustomobject]@{ Kind = "unreleased"; Body = $joined; BatchCount = $batches.Count }
            $sectionSource = "docs\changelog\最新条目.md"
        }
    }
}

$commit = (& git -C $projectRoot rev-parse HEAD 2>$null)
$commit = if ($LASTEXITCODE -eq 0 -and $commit) { "$commit".Trim() } else { "unknown" }
$short = if ($commit -ne "unknown") { $commit.Substring(0, [Math]::Min(12, $commit.Length)) } else { "unknown" }
$rustc = (& rustc --version 2>$null)
$rustc = if ($LASTEXITCODE -eq 0 -and $rustc) { "$rustc".Trim() } else { "unknown" }
$builtAt = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")

$files = @()
foreach ($a in $Assets) {
    $full = if ([System.IO.Path]::IsPathRooted($a)) { $a } else { Join-Path $projectRoot $a }
    if (-not (Test-Path -LiteralPath $full)) { throw "asset not found: $full" }
    $item = Get-Item -LiteralPath $full
    if ($item.PSIsContainer) {
        $files += @(Get-ChildItem -LiteralPath $item.FullName -Recurse -File | Sort-Object FullName)
    } else {
        $files += $item
    }
}

$sb = New-Object System.Text.StringBuilder
[void]$sb.AppendLine("# RustLua v$Version")
[void]$sb.AppendLine()
[void]$sb.AppendLine("构建时间（UTC）：``$builtAt``  ")
[void]$sb.AppendLine("提交：``$short``  ")
[void]$sb.AppendLine("工具链：``$rustc``")
[void]$sb.AppendLine()
if ($section) {
    if ($section.Kind -eq "unreleased") {
        [void]$sb.AppendLine("> 说明：``docs\changelog\版本历史.md`` 中没有 ``v$Version`` 的段落，下面聚合 ``$sectionSource`` 的全部 $($section.BatchCount) 条 Unreleased 批次。")
        [void]$sb.AppendLine()
    }
    # 版本段以 `## vX ...` 开头：发布页统一成中性的「变更」，剥掉段标题；
    # 聚合的 Unreleased 批次已把标题转成 `### `（保留来源批次），**不能**再用
    # `\A##` 剥——那会把第一条批次的 `### ` 前两个 `#` 误当段标题剥掉。
    if ($section.Kind -eq "version") {
        $body = ($section.Body -replace '\A##[^\r\n]*(\r?\n)?', '').Trim()
    } else {
        $body = $section.Body.Trim()
    }
    # 破坏性变更约定（§26.3-28）：`### 破坏性变更` 子标题是机器可见的 breaking 通道。
    if ($body -match '(?m)^###\s+破坏性变更\s*$') {
        [void]$sb.AppendLine("> **注意：本版本含破坏性变更（批次内写明「破坏性变更」子标题），升级前请先读迁移说明。**")
        [void]$sb.AppendLine()
    }
    [void]$sb.AppendLine("## 变更")
    [void]$sb.AppendLine()
    [void]$sb.AppendLine($body)
} else {
    [void]$sb.AppendLine("> 说明：``docs\changelog\`` 里没有 ``v$Version`` 版本段，也没有 Unreleased 段落，本页只列构建证据与资产。")
}
[void]$sb.AppendLine()
[void]$sb.AppendLine("## 发布资产")
[void]$sb.AppendLine()
[void]$sb.AppendLine("| 文件 | 字节 | SHA-256 |")
[void]$sb.AppendLine("| --- | ---: | --- |")
foreach ($f in $files) {
    $hash = (Get-FileHash -LiteralPath $f.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    [void]$sb.AppendLine(("| {0} | {1} | ``{2}`` |" -f $f.Name, $f.Length, $hash))
}
[void]$sb.AppendLine()
[void]$sb.AppendLine("## 安全与可追溯证据")
[void]$sb.AppendLine()
$sbomFull = if ([System.IO.Path]::IsPathRooted($Sbom)) { $Sbom } else { Join-Path $projectRoot $Sbom }
if (Test-Path -LiteralPath $sbomFull) {
    $sbomHash = (Get-FileHash -LiteralPath $sbomFull -Algorithm SHA256).Hash.ToLowerInvariant()
    [void]$sb.AppendLine(("- CycloneDX SBOM：``{0}``（SHA-256 ``{1}``）" -f (Split-Path -Leaf $sbomFull), $sbomHash))
} else {
    [void]$sb.AppendLine("- CycloneDX SBOM：**缺失**（发布流程不应走到这里）")
}
[void]$sb.AppendLine("- 安全门：``cargo-audit 0.22.2`` + ``cargo-deny 0.20.2``；版本锁定见 ``scripts/release/tools.json``")
[void]$sb.AppendLine("- 代码签名：见本次 release 的签名步骤日志；未配置证书时资产为**未签名**，不伪称已签")
[void]$sb.AppendLine()
[void]$sb.AppendLine("---")
[void]$sb.AppendLine("由 ``scripts/release/release-notes.ps1`` 生成；资产哈希以本页为准，下载后请自行复核。")

$outFull = if ([System.IO.Path]::IsPathRooted($Output)) { $Output } else { Join-Path $projectRoot $Output }
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $outFull) | Out-Null
[System.IO.File]::WriteAllText($outFull, $sb.ToString(), [System.Text.UTF8Encoding]::new($false))
Write-Host ("release_notes_ok version={0} assets={1} path={2}" -f $Version, $files.Count, $outFull) -ForegroundColor Green
