# sign-local.ps1 —— 本机真实证书签名入口（P2-01 ⑥ 的本地封装）
#
# 目的：把“PFX 导入 + 证书校验 + 调 signtool + 验签”收敛成一条命令，
#       密码只从参数/环境变量/交互输入读取，绝不打印、不写进仓库。
#
# 用法：
#   # 1) 已有 PFX（推荐导入 CurrentUser\My，后续免密码）
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/release/sign-local.ps1 `
#       -PfxPath C:\certs\rustlua-codesign.pfx
#
#   # 2) 证书已在存储（USB Token / 已导入）
#   ... -Thumbprint <SHA1>
#
#   # 3) 只验签（不签名）
#   ... -VerifyOnly
#
# 可选：-Path 指定文件/目录（默认 engine\target\release 下三个 exe）；
#       -NoTimestamp 跳过 RFC3161 时间戳（离线排障用，正式发布不要用）；
#       -WriteBase64To 把 PFX 的 Base64 写到指定文件，供 GitHub Secret 上传。

[CmdletBinding(DefaultParameterSetName = "Pfx")]
param(
    [Parameter(ParameterSetName = "Pfx", Mandatory = $true)]
    [string]$PfxPath,
    [Parameter(ParameterSetName = "Thumb", Mandatory = $true)]
    [string]$Thumbprint,
    [string]$PfxPassword = $env:RUSTLUA_SIGN_PFX_PASSWORD,
    [string[]]$Path = @(),
    [switch]$VerifyOnly,
    [switch]$NoTimestamp,
    [string]$WriteBase64To = ""
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$signScript = Join-Path $PSScriptRoot "sign-windows.ps1"

function Find-LocalSignTool {
    $roots = @(
        (Join-Path $env:LOCALAPPDATA "RustLua\signtool"),
        (Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"),
        (Join-Path $env:ProgramFiles "Windows Kits\10\bin")
    )
    foreach ($root in $roots) {
        if ([string]::IsNullOrWhiteSpace($root) -or -not (Test-Path -LiteralPath $root)) { continue }
        $candidate = Get-ChildItem -LiteralPath $root -Recurse -Filter signtool.exe -File -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match '\\x64\\signtool\.exe$' } |
            Sort-Object FullName -Descending |
            Select-Object -First 1
        if ($candidate) { return $candidate.FullName }
    }
    $cmd = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    return ""
}

function Assert-CodeSigningCert($Cert) {
    if (-not $Cert.HasPrivateKey) { throw "certificate has no private key" }
    if ($Cert.NotAfter -lt (Get-Date)) { throw "certificate expired at $($Cert.NotAfter)" }
    $eku = @($Cert.EnhancedKeyUsageList | ForEach-Object { $_.ObjectId.ToString() })
    if ($eku -notcontains "1.3.6.1.5.5.7.3.3") {
        throw "certificate is not a Code Signing cert (EKU 1.3.6.1.5.5.7.3.3 missing)"
    }
    $chain = New-Object System.Security.Cryptography.X509Certificates.X509Chain
    $chain.ChainPolicy.RevocationMode = [System.Security.Cryptography.X509Certificates.X509RevocationMode]::NoCheck
    $ok = $chain.Build($Cert)
    if (-not $ok) {
        $status = ($chain.ChainStatus | ForEach-Object { $_.Status }) -join ","
        Write-Host "[WARN] certificate chain not trusted: $status（自签测试会这样；正式证书应为 Valid）" -ForegroundColor Yellow
    }
}

if (-not (Test-Path -LiteralPath $signScript)) { throw "missing $signScript" }

$targets = @()
if ($Path.Count -gt 0) {
    $targets = @($Path | ForEach-Object { $_ -split '[,;]' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })
} else {
    foreach ($name in @("game.exe", "editor.exe", "assetc.exe")) {
        $candidate = Join-Path $projectRoot "engine\target\release\$name"
        if (Test-Path -LiteralPath $candidate) { $targets += $candidate }
    }
    if ($targets.Count -eq 0) {
        throw "no release binaries found; build first (cargo build --release -p game -p editor -p assetc) or pass -Path"
    }
}

$cert = $null
if ($PfxPath) {
    if (-not (Test-Path -LiteralPath $PfxPath)) { throw "PFX not found: $PfxPath" }
    if ([string]::IsNullOrWhiteSpace($PfxPassword)) {
        $secureInput = Read-Host -AsSecureString "PFX password"
        $PfxPassword = [System.Net.NetworkCredential]::new("", $secureInput).Password
    }
    $secure = ConvertTo-SecureString -String $PfxPassword -AsPlainText -Force
    $cert = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2(
        (Resolve-Path -LiteralPath $PfxPath).Path, $secure,
        [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]::Exportable)
    Assert-CodeSigningCert $cert
    Write-Host ("[INFO] PFX cert subject=" + $cert.Subject + " thumbprint=" + $cert.Thumbprint + " expires=" + $cert.NotAfter)

    if (-not $VerifyOnly) {
        # 导入 CurrentUser\My：后续签名走 -Thumbprint，不再需要 PFX 密码。
        $imported = Import-PfxCertificate -FilePath (Resolve-Path -LiteralPath $PfxPath).Path `
            -CertStoreLocation "Cert:\CurrentUser\My" -Password $secure
        if ($imported) { $cert = @($imported)[0] }
        $Thumbprint = $cert.Thumbprint
        Write-Host ("[INFO] imported to Cert:\CurrentUser\My thumbprint=" + $Thumbprint)
    }

    if (-not [string]::IsNullOrWhiteSpace($WriteBase64To)) {
        $b64 = [Convert]::ToBase64String([System.IO.File]::ReadAllBytes((Resolve-Path -LiteralPath $PfxPath).Path))
        $dir = Split-Path -Parent $WriteBase64To
        if ($dir) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
        Set-Content -LiteralPath $WriteBase64To -Value $b64 -Encoding ASCII -NoNewline
        Write-Host ("[INFO] base64 written to " + $WriteBase64To + " (upload as WINDOWS_CERT_PFX_BASE64; do not commit)")
    }
}

if ($VerifyOnly -and -not $Thumbprint -and -not $cert) {
    throw "-VerifyOnly needs -PfxPath or -Thumbprint to know which signer to expect"
}

$signTool = Find-LocalSignTool
$signArgs = @{
    Path             = $targets
    RequireSignature = $true
}
if ($VerifyOnly) { $signArgs.VerifyOnly = $true }
if ($Thumbprint) { $signArgs.Thumbprint = $Thumbprint }
if ($signTool) { $signArgs.SignTool = $signTool }
if ($NoTimestamp) { $signArgs.TimestampUrl = "" }

$toolLabel = if ($signTool) { $signTool } else { "authenticode-fallback" }
Write-Host ("==> sign-local targets=" + ($targets -join ";") + " tool=" + $toolLabel)
& $signScript @signArgs
exit $LASTEXITCODE
