# check-security.ps1 —— 发布/供应链安全门（P2-01 ②③⑦）
#
# 一道入口串起五件事：
#   1. cargo-audit：Cargo.lock 的已知漏洞与 yanked 包（engine/.cargo/audit.toml）
#   2. cargo-deny ：advisories / licenses / bans / sources（engine/deny.toml）
#   3. registry wildcard：workspace 依赖里不许出现 `"*"`
#   4. fork 同步 ：vendored fork 相对同版本上游的改动足迹（scripts/repo/fork-sync-report.mjs --check）
#   5. SBOM      ：可选，校验已生成的 CycloneDX 是否与 Cargo.lock 一致
#
# 三态退出（与仓库既有门一致）：
#   0 = 全绿
#   1 = 判红（有漏洞/许可证越界/补丁足迹丢失/结构漂移）
#   3 = 环境受限（工具缺失、advisory DB 拉取失败、上游源码缓存缺失）——**不是通过**
#
# 用法：
#   pwsh -NoProfile -File scripts/verify/check-security.ps1
#   pwsh -NoProfile -File scripts/verify/check-security.ps1 -RequireTools
#   pwsh -NoProfile -File scripts/verify/check-security.ps1 -SbomPath artifacts/reports/rustlua.cdx.json
#   pwsh -NoProfile -File scripts/verify/check-security.ps1 -FetchForkUpstream   # 干净 CARGO_HOME

#
# 工具定位：PATH 优先，其次 artifacts/tools/security/bin（install-security-tools.ps1 的落点）。

[CmdletBinding()]
param(
    [switch]$RequireTools,
    [string]$ToolsRoot = "",
    [string]$SbomPath = "",
    [string]$CargoConfig = "",
    # 离线/镜像环境：指向一个本地 advisory-db git 仓库（RustSec 快照）。
    # 设置后 audit 用 --db/--no-fetch，deny 用临时配置从 file:// 克隆，均不走 GitHub。
    # CI 不传，走官方 db-urls。
    [string]$AdvisoryDb = $env:RUSTLUA_ADVISORY_DB,
    [switch]$SkipAudit,
    [switch]$SkipDeny,
    [switch]$SkipForkSync,
    # CI 的干净 CARGO_HOME 里没有 [patch] 掉的注册表源码；显式允许从 crates.io
    # 下载 .crate 并解到 registry/src/fork-upstream，避免 fork 门永远 BLOCKED。
    [switch]$FetchForkUpstream
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$engineRoot = Join-Path $projectRoot "engine"
if ([string]::IsNullOrWhiteSpace($ToolsRoot)) {
    $ToolsRoot = Join-Path $projectRoot "artifacts\tools\security"
}
$toolsPath = Join-Path $projectRoot "scripts\release\tools.json"
$tools = Get-Content -LiteralPath $toolsPath -Raw | ConvertFrom-Json

$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$logDir = Join-Path $projectRoot "artifacts\logs\security"
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
$summaryLog = Join-Path $logDir ("security-" + $stamp + ".log")

$failed = @()
$blocked = @()
$passed = 0
$results = [ordered]@{}

function Add-Pass([string]$Name) {
    $script:passed++
    # 结果表的 key 只取第一段：`cargo-audit 0.22.2 vulns=0` -> `cargo-audit`。
    $script:results[($Name -split '\s+')[0]] = "ok"
    Write-Host ("[PASS] " + $Name) -ForegroundColor Green
}
function Add-Fail([string]$Name, [string]$Reason) {
    $script:results[($Name -split '\s+')[0]] = "failed"
    $script:failed += ("[FAIL] " + $Name + " -- " + $Reason)
    Write-Host ("[FAIL] " + $Name + " -- " + $Reason) -ForegroundColor Red
}
function Add-Blocked([string]$Name, [string]$Reason) {
    $script:results[($Name -split '\s+')[0]] = "blocked"
    $script:blocked += ("[BLOCKED] " + $Name + " -- " + $Reason)
    Write-Host ("[BLOCKED] " + $Name + " -- " + $Reason) -ForegroundColor Yellow
}
function Write-Log([string]$Text) {
    Add-Content -LiteralPath $summaryLog -Value $Text -Encoding UTF8
}
function Find-Tool([string]$Name) {
    $onPath = Get-Command $Name -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    $local = Join-Path $ToolsRoot ("bin\{0}.exe" -f $Name)
    if (Test-Path -LiteralPath $local) { return $local }
    return $null
}
function Get-ToolVersion([string]$Exe) {
    try {
        $raw = & $Exe --version 2>$null
        if ($LASTEXITCODE -ne 0 -or -not $raw) { return "" }
        $m = [regex]::Match(("$raw" -join " "), '(\d+\.\d+\.\d+)')
        if ($m.Success) { return $m.Groups[1].Value }
    } catch { }
    return ""
}

# 运行外部命令，stdout/stderr 分开落盘；返回 exit code。
# 用 Start-Process 而不是 `&`，因为 audit 的 JSON 必须与 stderr 警告分开。
function Invoke-Captured([string]$Exe, [string[]]$ArgList, [string]$WorkDir, [string]$OutFile, [string]$ErrFile) {
    $p = Start-Process -FilePath $Exe -ArgumentList $ArgList -WorkingDirectory $WorkDir -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput $OutFile -RedirectStandardError $ErrFile
    return $p.ExitCode
}
function Looks-LikeNetworkFailure([string]$Text) {
    return ($Text -match '(?i)(failed to (fetch|update|download|clone)|could not|couldn''t|network|dns|timed? ?out|timeout|connection refused|tls|git operation failed|error sending request)')
}

# registry 依赖上的 `*` 是供应链风险（绕过 lockfile)，但 workspace 内部的 path 依赖
# 本来就不写版本。cargo-deny 0.20 对"公开 crate"的 path 依赖仍报 wildcard，本仓没有
# 逐个标 publish=false，所以这里做一次只针对 registry 的精确扫描。
function Find-RegistryWildcards([string]$Root) {
    $hits = @()
    $manifests = @()
    $rootManifest = Join-Path $Root "Cargo.toml"
    if (Test-Path -LiteralPath $rootManifest) { $manifests += $rootManifest }
    $cratesDir = Join-Path $Root "crates"
    if (Test-Path -LiteralPath $cratesDir) {
        $manifests += @(Get-ChildItem -LiteralPath $cratesDir -Recurse -Filter Cargo.toml -File -ErrorAction SilentlyContinue |
                Select-Object -ExpandProperty FullName)
    }
    foreach ($manifest in $manifests) {
        $lines = @(Get-Content -LiteralPath $manifest -ErrorAction SilentlyContinue)
        for ($i = 0; $i -lt $lines.Count; $i++) {
            $line = [string]$lines[$i]
            if ($line -match '^\s*#') { continue }
            if ($line -match '\bpath\s*=' -or $line -match '\bgit\s*=') { continue }
            $isShorthand = $line -match '^\s*[A-Za-z0-9_-]+\s*=\s*"\*"\s*(#.*)?$'
            $isVersion = $line -match '\bversion\s*=\s*"\*"'
            if ($isShorthand -or $isVersion) {
                $hits += ("{0}:{1}: {2}" -f $manifest, ($i + 1), $line.Trim())
            }
        }
    }
    return $hits
}

# 本地 advisory DB 可以是"根目录"（内含 advisory-db-<hash> 子目录），也可以直接是
# 一个 RustSec 仓库。cargo-audit 要后者，cargo-deny 要前者作为 db-path。
# RustSec 用 crates/ 与 rust/ 两个目录存放 advisory，不是 advisories/。
function Test-AdvisoryDb([string]$Path) {
    return ((Test-Path -LiteralPath (Join-Path $Path "crates")) -and (Test-Path -LiteralPath (Join-Path $Path "rust")))
}
function Resolve-AdvisoryRepo([string]$InputPath) {
    $root = if ([System.IO.Path]::IsPathRooted($InputPath)) { $InputPath } else { Join-Path $projectRoot $InputPath }
    if (-not (Test-Path -LiteralPath $root)) { return $null }
    if (Test-AdvisoryDb $root) {
        return [pscustomobject]@{ Root = (Split-Path -Parent $root); Repo = $root }
    }
    $child = Get-ChildItem -LiteralPath $root -Directory -ErrorAction SilentlyContinue |
        Where-Object { Test-AdvisoryDb $_.FullName } |
        Select-Object -First 1
    if ($child) { return [pscustomobject]@{ Root = $root; Repo = $child.FullName } }
    return $null
}

Write-Log ("security gate " + $stamp)
Write-Log ("tools.json: audit={0} deny={1} cyclonedx={2}" -f $tools.cargoAudit, $tools.cargoDeny, $tools.cargoCyclonedx)

# ── 1. cargo-audit ──────────────────────────────────────────────────────────
if (-not $SkipAudit) {
    $audit = Find-Tool "cargo-audit"
    if (-not $audit) {
        if ($RequireTools) { Add-Fail "cargo-audit" "not found; run scripts/release/install-security-tools.ps1" }
        else { Add-Blocked "cargo-audit" "not found (install with scripts/release/install-security-tools.ps1)" }
    } else {
        $ver = Get-ToolVersion $audit
        Write-Host ("==> cargo-audit {0} ({1})" -f $ver, $audit) -ForegroundColor Cyan
        $outJson = Join-Path $logDir ("audit-" + $stamp + ".json")
        $errTxt = Join-Path $logDir ("audit-" + $stamp + ".stderr.txt")
        $auditArgs = @("audit", "--file", "Cargo.lock", "--json")
        if (-not [string]::IsNullOrWhiteSpace($AdvisoryDb)) {
            $adv = Resolve-AdvisoryRepo $AdvisoryDb
            if (-not $adv) {
                Add-Fail "cargo-audit" ("AdvisoryDb not found or has no advisories/: " + $AdvisoryDb)
                $audit = $null
            } else {
                $auditArgs += @("--db", $adv.Repo, "--no-fetch")
            }
        }
        if ($audit) {
        $code = Invoke-Captured $audit $auditArgs $engineRoot $outJson $errTxt
        $err = if (Test-Path $errTxt) { Get-Content -LiteralPath $errTxt -Raw } else { "" }
        $json = $null
        if (Test-Path $outJson) {
            try {
                $raw = Get-Content -LiteralPath $outJson -Raw
                if ($raw -and $raw.TrimStart().StartsWith("{")) { $json = $raw | ConvertFrom-Json }
            } catch { $json = $null }
        }
        $vulnCount = -1
        $warnCount = -1
        if ($null -ne $json -and $null -ne $json.vulnerabilities) {
            $vulnCount = [int]$json.vulnerabilities.count
            # cargo-audit 的 warnings 不是一个带 count 的对象，而是 unmaintained/unsound/... 数组。
            $warnCount = 0
            if ($null -ne $json.warnings) {
                foreach ($prop in $json.warnings.PSObject.Properties) {
                    if ($prop.Value -is [System.Array]) { $warnCount += $prop.Value.Count }
                }
            }
        }
        Write-Log ("cargo-audit exit={0} vulns={1} warnings={2} json={3}" -f $code, $vulnCount, $warnCount, $outJson)
        if ($code -eq 0) {
            Add-Pass ("cargo-audit {0} vulns=0 warnings={1}" -f $ver, [Math]::Max($warnCount, 0))
        } elseif ($vulnCount -gt 0) {
            Add-Fail "cargo-audit" ("vulns=" + $vulnCount + "; see " + $outJson)
        } elseif (Looks-LikeNetworkFailure $err) {
            Add-Blocked "cargo-audit" "advisory database unavailable (network); see $errTxt"
        } else {
            Add-Fail "cargo-audit" ("exit=" + $code + "; see " + $outJson + " / " + $errTxt)
        }
        }
    }
}

# ── 1b. registry wildcard scan ─────────────────────────────────────────────
$wildcards = @(Find-RegistryWildcards $engineRoot)
if ($wildcards.Count -gt 0) {
    Add-Fail "registry-wildcards" (($wildcards -join " | ") + " (use a real version range; path deps are exempt)")
} else {
    Add-Pass "registry-wildcards"
}

# ── 2. cargo-deny ───────────────────────────────────────────────────────────
if (-not $SkipDeny) {
    $deny = Find-Tool "cargo-deny"
    if (-not $deny) {
        if ($RequireTools) { Add-Fail "cargo-deny" "not found; run scripts/release/install-security-tools.ps1" }
        else { Add-Blocked "cargo-deny" "not found (install with scripts/release/install-security-tools.ps1)" }
    } else {
        $ver = Get-ToolVersion $deny
        Write-Host ("==> cargo-deny {0} ({1})" -f $ver, $deny) -ForegroundColor Cyan
        $outTxt = Join-Path $logDir ("deny-" + $stamp + ".log")
        $errTxt = Join-Path $logDir ("deny-" + $stamp + ".stderr.txt")
        $denyArgs = @("--locked")
        $denyReady = $true
        if (-not [string]::IsNullOrWhiteSpace($AdvisoryDb)) {
            $adv = Resolve-AdvisoryRepo $AdvisoryDb
            if (-not $adv) {
                Add-Fail "cargo-deny" ("AdvisoryDb not found or has no advisories/: " + $AdvisoryDb)
                $denyReady = $false
            } else {
                $cfg = Join-Path $projectRoot ("artifacts\tmp\deny-local-" + $stamp + ".toml")
                # PS 5.1 的 Get-Content 会把无 BOM 的 UTF-8 当 ANSI，中文注释会变乱码。
                $base = [System.IO.File]::ReadAllText((Join-Path $engineRoot "deny.toml"), [System.Text.UTF8Encoding]::new($false))
                $dbRootToml = $adv.Root -replace '\\', '/'
                # 不写 db-urls：保留默认 RustSec URL，cargo-deny 用该 URL 的 hash 找
                # $db-path/advisory-db-<hash>；--offline 禁止 fetch，只读现有 git 仓库。
                $inject = "[advisories]`ndb-path = `"$dbRootToml`""
                $base = $base.Replace("[advisories]", $inject)
                [System.IO.File]::WriteAllText($cfg, $base, [System.Text.UTF8Encoding]::new($false))
                $denyArgs += @("--offline", "--config", $cfg)
            }
        }
        $denyArgs += @("check")
        if ($denyReady) {
            $oldNoColor = $env:NO_COLOR
            $env:NO_COLOR = "1"
            try {
                $code = Invoke-Captured $deny $denyArgs $engineRoot $outTxt $errTxt
            } finally {
                if ($null -eq $oldNoColor) { Remove-Item Env:NO_COLOR -ErrorAction SilentlyContinue } else { $env:NO_COLOR = $oldNoColor }
            }
            $err = if (Test-Path $errTxt) { Get-Content -LiteralPath $errTxt -Raw } else { "" }
            $body = if (Test-Path $outTxt) { Get-Content -LiteralPath $outTxt -Raw } else { "" }
            Write-Log ("cargo-deny exit={0} log={1}" -f $code, $outTxt)
            if ($code -eq 0) {
                Add-Pass ("cargo-deny {0}" -f $ver)
            } elseif (Looks-LikeNetworkFailure ($err + "`n" + $body)) {
                Add-Blocked "cargo-deny" "advisory/index unavailable (network); see $errTxt"
            } else {
                Add-Fail "cargo-deny" ("exit=" + $code + "; see " + $outTxt)
            }
        }
    }
}

# ── 3. fork 同步（离线结构检查）──────────────────────────────────────────────
if (-not $SkipForkSync) {
    $node = Get-Command node -ErrorAction SilentlyContinue
    if (-not $node) {
        if ($RequireTools) { Add-Fail "fork-sync" "node not found" }
        else { Add-Blocked "fork-sync" "node not found" }
    } else {
        $report = Join-Path $logDir ("fork-sync-" + $stamp + ".md")
        $prev = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $forkArgs = @("--check", "--write", $report)
        if ($FetchForkUpstream) { $forkArgs = @("--check", "--fetch-missing", "--write", $report) }
        & node (Join-Path $projectRoot "scripts\repo\fork-sync-report.mjs") @forkArgs 2>&1 | Tee-Object -FilePath $summaryLog -Append | Out-Null
        $code = $LASTEXITCODE
        $ErrorActionPreference = $prev
        if ($code -eq 0) { Add-Pass "fork-sync" }
        elseif ($code -eq 3) { Add-Blocked "fork-sync" "upstream source missing; see $report" }
        else { Add-Fail "fork-sync" ("exit=" + $code + "; see " + $report) }
    }
}

# ── 4. SBOM（可选）──────────────────────────────────────────────────────────
if (-not [string]::IsNullOrWhiteSpace($SbomPath)) {
    $sbomFull = if ([System.IO.Path]::IsPathRooted($SbomPath)) { $SbomPath } else { Join-Path $projectRoot $SbomPath }
    if (-not (Test-Path -LiteralPath $sbomFull)) {
        Add-Fail "sbom" ("missing " + $sbomFull + "; run scripts/release/gen-sbom.mjs first")
    } else {
        $prev = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        & node (Join-Path $projectRoot "scripts\release\gen-sbom.mjs") --check --output $sbomFull 2>&1 | Tee-Object -FilePath $summaryLog -Append | Out-Null
        $code = $LASTEXITCODE
        $ErrorActionPreference = $prev
        if ($code -eq 0) { Add-Pass "sbom" } else { Add-Fail "sbom" ("exit=" + $code + "; see " + $summaryLog) }
    }
}

Write-Host ""
$summary = ("security: " + $passed + " passed, " + $failed.Count + " failed, " + $blocked.Count + " blocked")
Write-Host $summary
Write-Log $summary
if ($failed.Count -gt 0) {
    $failed | ForEach-Object { Write-Host $_ -ForegroundColor Red; Write-Log $_ }
    Write-Host "security_failed" -ForegroundColor Red
    Write-Log "security_failed"
    exit 1
}
if ($blocked.Count -gt 0) {
    $blocked | ForEach-Object { Write-Host $_ -ForegroundColor Yellow; Write-Log $_ }
    Write-Host "security_blocked" -ForegroundColor Yellow
    Write-Log "security_blocked"
    exit 3
}
Write-Host ("security_ok audit={0} deny={1} fork={2} log={3}" -f $results["cargo-audit"], $results["cargo-deny"], $results["fork-sync"], $summaryLog) -ForegroundColor Green
Write-Log ("security_ok audit={0} deny={1} fork={2}" -f $results["cargo-audit"], $results["cargo-deny"], $results["fork-sync"])
exit 0
