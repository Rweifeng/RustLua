# test-security-gates.ps1 —— 安全门的反例自测（P2-01）
#
# 只证明"门会拦"，不重复跑全量审计：
#   1. 篡改 SBOM 的一个哈希，gen-sbom --check 必须退 1；
#   2. 原 SBOM 必须退 0（证明上一条不是"永远红"）；
#   3. 给 check-security 一个不存在的 advisory DB，必须退 1（配置错误不能伪装成通过）；
#   4. 有未签名样本 exe 时，sign-windows -SelfTest 必须退 0 且能检出篡改；没有样本则显式 SKIP。
#
# 用法：
#   pwsh -NoProfile -File scripts/verify/test-security-gates.ps1
#
# 退出码：0 = 反例全部按预期；1 = 有一条没有按预期拦/放。

[CmdletBinding()]
param(
    [string]$CargoConfig = ""
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$tmp = Join-Path $projectRoot ("artifacts\tmp\security-selftest-" + $stamp)
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

$failed = @()
function Invoke-AllowFailure([scriptblock]$Block) {
    # 预期失败的命令会往 stderr 写 [FAIL]；PS 5.1 在 $ErrorActionPreference=Stop
    # 下会把原生 stderr 升级成 NativeCommandError，必须临时降级，否则反例自测自己先炸。
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try { & $Block } finally { $ErrorActionPreference = $prev }
}
function Assert-Exit([string]$Name, [int]$Expected, [int]$Actual) {
    if ($Expected -eq $Actual) {
        Write-Host ("[PASS] {0} exit={1}" -f $Name, $Actual) -ForegroundColor Green
    } else {
        Write-Host ("[FAIL] {0} exit={1}, expect {2}" -f $Name, $Actual, $Expected) -ForegroundColor Red
        $script:failed += $Name
    }
}

$node = Get-Command node -ErrorAction SilentlyContinue
if (-not $node) {
    Write-Host "[FAIL] node not found; cannot test SBOM gate" -ForegroundColor Red
    exit 1
}

$sbom = Join-Path $projectRoot "artifacts\reports\rustlua.cdx.json"
if (-not (Test-Path -LiteralPath $sbom)) {
    Write-Host ("[FAIL] SBOM missing: {0} (generate it first)" -f $sbom) -ForegroundColor Red
    exit 1
}

$metaArgs = @()
if (-not [string]::IsNullOrWhiteSpace($CargoConfig)) {
    $metaArgs += @("--cargo-config", $CargoConfig)
}

$good = Join-Path $tmp "good.cdx.json"
Copy-Item -LiteralPath $sbom -Destination $good -Force
& node (Join-Path $projectRoot "scripts\release\gen-sbom.mjs") --check --output $good @metaArgs 2>&1 | Out-Null
Assert-Exit "sbom_original" 0 $LASTEXITCODE

$bad = Join-Path $tmp "tampered.cdx.json"
Copy-Item -LiteralPath $sbom -Destination $bad -Force
$tamper = @'
const fs = require("fs");
const p = process.argv[2];
const j = JSON.parse(fs.readFileSync(p, "utf8"));
const c = j.components.find((x) => Array.isArray(x.hashes) && x.hashes.length > 0);
c.hashes[0].content = "0".repeat(64);
fs.writeFileSync(p, JSON.stringify(j, null, 2));
'@
$tamperFile = Join-Path $tmp "tamper.js"
[System.IO.File]::WriteAllText($tamperFile, $tamper, [System.Text.UTF8Encoding]::new($false))
& node $tamperFile $bad
Invoke-AllowFailure { & node (Join-Path $projectRoot "scripts\release\gen-sbom.mjs") --check --output $bad @metaArgs 2>&1 | Out-Null }
Assert-Exit "sbom_tampered" 1 $LASTEXITCODE

# 不存在的 advisory DB 必须在 audit 启动前判红（否则"DB 路径写错"会被误读成"没漏洞"）。
$security = Join-Path $projectRoot "scripts\verify\check-security.ps1"
Invoke-AllowFailure { & powershell -NoProfile -ExecutionPolicy Bypass -File $security -AdvisoryDb (Join-Path $tmp "no-such-db") -SkipDeny -SkipForkSync 2>&1 | Out-Null }
Assert-Exit "check_security_bad_db" 1 $LASTEXITCODE

$sample = Join-Path $projectRoot "engine\target\debug\game.exe"
if (-not (Test-Path -LiteralPath $sample)) { $sample = Join-Path $projectRoot "engine\target\release\game.exe" }
if (Test-Path -LiteralPath $sample) {
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $projectRoot "scripts\release\sign-windows.ps1") -SelfTest 2>&1 | Out-Null
    Assert-Exit "signing_selftest" 0 $LASTEXITCODE
} else {
    Write-Host "[SKIP] signing_selftest: no unsigned engine/target/{debug,release}/game.exe (build engine to cover this)" -ForegroundColor Yellow
}

Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
if ($failed.Count -gt 0) {
    Write-Host ("security_gates_selftest_failed: " + ($failed -join ", ")) -ForegroundColor Red
    exit 1
}
Write-Host "security_gates_selftest_ok" -ForegroundColor Green
exit 0
