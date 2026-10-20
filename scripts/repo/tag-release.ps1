# tag-release.ps1 —— 打 tag 并（可选）推送，触发 .github/workflows/release.yml（P2-01 ⑤）
#
# 发布只认 annotated tag `v<version>`：tag 是"这一份字节从哪来"的唯一锚点。
# 本脚本不构建、不上传，只做三件事：校验版本、校验工作树、打/推 tag。
#
# 用法：
#   pwsh -NoProfile -File scripts/repo/tag-release.ps1                 # 默认 dry-run，只打印命令
#   pwsh -NoProfile -File scripts/repo/tag-release.ps1 -Apply          # 只打本地 annotated tag
#   pwsh -NoProfile -File scripts/repo/tag-release.ps1 -Push           # 打 tag 并 push（触发 release.yml）
#   pwsh -NoProfile -File scripts/repo/tag-release.ps1 -AllowDirty     # 紧急发布，允许工作树脏
#
# 安全默认：不带 -Apply / -Push 时**不会**改仓库，只打印将执行的命令。
# 退出码：0 = dry-run/已创建/已推送；1 = 校验失败。

[CmdletBinding()]
param(
    [string]$Version = "",
    [switch]$Push,
    [switch]$Apply,
    [switch]$DryRun,
    [switch]$AllowDirty
)

if (-not $Push -and -not $Apply) { $DryRun = $true }

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$engineToml = Join-Path $projectRoot "engine\Cargo.toml"

$m = Select-String -Path $engineToml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $m) { throw "cannot read workspace version from $engineToml" }
$workspaceVersion = $m.Matches[0].Groups[1].Value
if ([string]::IsNullOrWhiteSpace($Version)) { $Version = $workspaceVersion }
if ($Version -ne $workspaceVersion) {
    throw "version mismatch: -Version $Version != engine/Cargo.toml $workspaceVersion"
}
if ($Version -notmatch '^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$') {
    throw "version must be semver (got '$Version')"
}

$tag = "v$Version"
$dirty = @(& git -C $projectRoot status --porcelain 2>$null)
if ($LASTEXITCODE -ne 0) { throw "git status failed" }
if ($dirty.Count -gt 0 -and -not $AllowDirty) {
    Write-Host "working tree is dirty; release tag must point at a reproducible commit." -ForegroundColor Red
    $dirty | Select-Object -First 20 | ForEach-Object { Write-Host ("  " + $_) }
    Write-Host "hint: commit/stash first, or pass -AllowDirty for an explicitly non-reproducible build." -ForegroundColor Yellow
    exit 1
}

$existing = (& git -C $projectRoot rev-parse -q --verify ("refs/tags/" + $tag) 2>$null)
if ($LASTEXITCODE -eq 0 -and $existing) {
    throw "tag $tag already exists locally ($existing)"
}

$changelog = Join-Path $projectRoot "CHANGELOG.md"
if (Test-Path -LiteralPath $changelog) {
    $hasSection = Select-String -Path $changelog -Pattern ('^##\s+\[?v?' + [regex]::Escape($Version) + '\]?(?:\s|$)') -Quiet
    if (-not $hasSection) {
        Write-Host "warning: CHANGELOG.md has no section for v$Version; release notes will fall back to Unreleased." -ForegroundColor Yellow
    }
}

$commit = (& git -C $projectRoot rev-parse --short HEAD 2>$null)
if ($LASTEXITCODE -ne 0 -or -not $commit) { throw "git rev-parse HEAD failed" }

$msg = "RustLua $tag"
Write-Host ("==> tag {0} at {1}" -f $tag, $commit) -ForegroundColor Cyan
if ($DryRun) {
    Write-Host ("[dry-run] git -C ""{0}"" tag -a {1} -m ""{2}""" -f $projectRoot, $tag, $msg)
    if ($Push) { Write-Host ("[dry-run] git -C ""{0}"" push origin {1}" -f $projectRoot, $tag) }
    Write-Host "tag_release_dry_run_ok" -ForegroundColor Green
    exit 0
}

& git -C $projectRoot tag -a $tag -m $msg
if ($LASTEXITCODE -ne 0) { throw "git tag failed (exit $LASTEXITCODE)" }
Write-Host ("[PASS] created annotated tag {0}" -f $tag) -ForegroundColor Green

if ($Push) {
    & git -C $projectRoot push origin $tag
    if ($LASTEXITCODE -ne 0) { throw "git push origin $tag failed (exit $LASTEXITCODE)" }
    Write-Host ("[PASS] pushed {0}; release.yml should now run" -f $tag) -ForegroundColor Green
} else {
    Write-Host ("next: git push origin {0}" -f $tag) -ForegroundColor Yellow
}
Write-Host ("tag_release_ok tag={0} commit={1}" -f $tag, $commit) -ForegroundColor Green
