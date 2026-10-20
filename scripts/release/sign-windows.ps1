# sign-windows.ps1 —— Windows Authenticode 代码签名（P2-01 ⑥）
#
# 设计要点：
#   - 证书只从参数/环境变量/证书存储读，**永不打印密码或 PFX 内容**；
#   - 没有证书时不是"成功"：默认 exit 3（signing_skipped），-RequireSignature 时 exit 1；
#   - 优先 signtool（支持 RFC3161 时间戳与 PFX）；没有 Windows SDK 时回退
#     PowerShell 的 Set-AuthenticodeSignature（本机可验证）；
#   - -SelfTest 用 CurrentUser 自签证书签一个临时 exe，并做**篡改检测**，不需要真实证书。
#
# 用法：
#   pwsh -NoProfile -File scripts/release/sign-windows.ps1 -Path artifacts/packages/foo -PfxPath cert.pfx
#   pwsh -NoProfile -File scripts/release/sign-windows.ps1 -Path foo.exe -Thumbprint <sha1>
#   pwsh -NoProfile -File scripts/release/sign-windows.ps1 -SelfTest
#   pwsh -NoProfile -File scripts/release/sign-windows.ps1 -Path foo.exe -VerifyOnly
#
# 退出码：0 = 已签名/验证通过；1 = 要求签名但失败；3 = 无证书/无 signtool 的显式跳过。

[CmdletBinding()]
param(
    [string[]]$Path = @(),
    [string]$PfxPath = "",
    [string]$PfxPassword = $env:RUSTLUA_SIGN_PFX_PASSWORD,
    [string]$Thumbprint = "",
    [string]$TimestampUrl = "http://timestamp.digicert.com",
    [switch]$VerifyOnly,
    [switch]$RequireSignature,
    [switch]$SelfTest,
    [switch]$DryRun,
    [string]$SignTool = ""
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
# `pwsh -File` 的数组参数同样只绑定第一个元素；允许 `-Path "a,b"` 的单个字符串。
$Path = @($Path | ForEach-Object { $_ -split '[,;]' } | ForEach-Object { $_.Trim() } | Where-Object { $_ })

function Find-SignTool {
    if (-not [string]::IsNullOrWhiteSpace($SignTool)) {
        if (Test-Path -LiteralPath $SignTool) { return (Resolve-Path -LiteralPath $SignTool).Path }
        throw "SignTool not found: $SignTool"
    }
    $cmd = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $roots = @(
        (Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"),
        (Join-Path $env:ProgramFiles "Windows Kits\10\bin"),
        (Join-Path ${env:ProgramFiles(x86)} "Windows Kits\8.1\bin")
    )
    foreach ($root in $roots) {
        if ([string]::IsNullOrWhiteSpace($root) -or -not (Test-Path -LiteralPath $root)) { continue }
        $candidates = @()
        $candidates += @(Get-ChildItem -LiteralPath $root -Directory -ErrorAction SilentlyContinue |
                Sort-Object Name -Descending |
                ForEach-Object { Join-Path $_.FullName "x64\signtool.exe" })
        $candidates += @(Get-ChildItem -LiteralPath $root -Recurse -Filter signtool.exe -File -ErrorAction SilentlyContinue |
                Select-Object -ExpandProperty FullName)
        foreach ($candidate in $candidates) {
            if (Test-Path -LiteralPath $candidate) { return $candidate }
        }
    }
    return $null
}

function Resolve-SignFiles([string[]]$Paths) {
    $files = @()
    foreach ($p in $Paths) {
        $full = if ([System.IO.Path]::IsPathRooted($p)) { $p } else { Join-Path $projectRoot $p }
        if (-not (Test-Path -LiteralPath $full)) { throw "sign path not found: $full" }
        $item = Get-Item -LiteralPath $full
        if ($item.PSIsContainer) {
            $files += @(Get-ChildItem -LiteralPath $item.FullName -Recurse -File -ErrorAction Stop |
                    Where-Object { $_.Extension -in @(".exe", ".dll", ".msi", ".sys") } |
                    Select-Object -ExpandProperty FullName)
        } else {
            $files += $item.FullName
        }
    }
    return @($files | Sort-Object -Unique)
}

function Get-CertificateFromInputs {
    if (-not [string]::IsNullOrWhiteSpace($PfxPath)) {
        if (-not (Test-Path -LiteralPath $PfxPath)) { throw "PFX not found: $PfxPath" }
        if ([string]::IsNullOrWhiteSpace($PfxPassword)) {
            throw "PFX password missing: pass -PfxPassword or set RUSTLUA_SIGN_PFX_PASSWORD"
        }
        $secure = ConvertTo-SecureString -String $PfxPassword -AsPlainText -Force
        $cert = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2(
            (Resolve-Path -LiteralPath $PfxPath).Path, $secure,
            [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]::Exportable)
        if (-not $cert.HasPrivateKey) { throw "PFX has no private key: $PfxPath" }
        return $cert
    }
    if (-not [string]::IsNullOrWhiteSpace($Thumbprint)) {
        $normalized = ($Thumbprint -replace '\s', '').ToUpperInvariant()
        $cert = Get-ChildItem Cert:\CurrentUser\My, Cert:\LocalMachine\My -ErrorAction SilentlyContinue |
            Where-Object { $_.Thumbprint -eq $normalized } | Select-Object -First 1
        if (-not $cert) { throw "certificate not found in CurrentUser\My or LocalMachine\My: $Thumbprint" }
        if (-not $cert.HasPrivateKey) { throw "certificate has no private key: $Thumbprint" }
        return $cert
    }
    return $null
}

function Invoke-SignToolSign([string]$Tool, [string]$File, [string]$Pfx, [string]$Sha1) {
    $display = @("sign", "/fd", "SHA256")
    $real = @("sign", "/fd", "SHA256")
    if (-not [string]::IsNullOrWhiteSpace($TimestampUrl)) {
        $display += @("/tr", $TimestampUrl, "/td", "SHA256")
        $real += @("/tr", $TimestampUrl, "/td", "SHA256")
    }
    if (-not [string]::IsNullOrWhiteSpace($Pfx)) {
        $display += @("/f", $Pfx, "/p", "***", $File)
        $real += @("/f", $Pfx, "/p", $PfxPassword, $File)
    } else {
        $display += @("/sha1", $Sha1, $File)
        $real += @("/sha1", $Sha1, $File)
    }
    if ($DryRun) {
        Write-Host ("[dry-run] " + $Tool + " " + (($display -join " ")))
        return 0
    }
    & $Tool @real | Out-Host
    return $LASTEXITCODE
}

function Invoke-SignToolVerify([string]$Tool, [string]$File) {
    if ($DryRun) {
        Write-Host ("[dry-run] " + $Tool + " verify /pa /v " + $File)
        return 0
    }
    & $Tool verify /pa /v $File | Out-Host
    return $LASTEXITCODE
}

function Get-SignatureStatus([string]$File) {
    $sig = Get-AuthenticodeSignature -LiteralPath $File
    return [pscustomobject]@{
        Status = [string]$sig.Status
        Subject = if ($sig.SignerCertificate) { $sig.SignerCertificate.Subject } else { "" }
        Thumbprint = if ($sig.SignerCertificate) { $sig.SignerCertificate.Thumbprint } else { "" }
        IsSigned = ($null -ne $sig.SignerCertificate)
    }
}

function Sign-WithPowerShell([System.Security.Cryptography.X509Certificates.X509Certificate2]$Cert, [string]$File) {
    if ($DryRun) {
        Write-Host ("[dry-run] Set-AuthenticodeSignature -FilePath " + $File + " -Certificate " + $Cert.Thumbprint)
        return
    }
    $params = @{
        FilePath        = $File
        Certificate     = $Cert
        HashAlgorithm   = "SHA256"
    }
    if (-not [string]::IsNullOrWhiteSpace($TimestampUrl)) { $params.TimestampServer = $TimestampUrl }
    Set-AuthenticodeSignature @params | Out-Null
}

# ── SelfTest ────────────────────────────────────────────────────────────────
if ($SelfTest) {
    $temp = Join-Path $env:TEMP ("rustlua-sign-selftest-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $temp | Out-Null
    $cert = $null
    $savedTimestamp = $TimestampUrl
    $TimestampUrl = ""
    try {
        # 必须用**未签名**的样本：对已带 Microsoft 签名的系统 exe 再签会形成双签名，
        # Get-AuthenticodeSignature 仍读到第一签名，自测会误报 thumbprint mismatch。
        $sourceExe = Join-Path $projectRoot "engine\target\release\game.exe"
        if (-not (Test-Path -LiteralPath $sourceExe)) { $sourceExe = Join-Path $projectRoot "engine\target\debug\game.exe" }
        if (-not (Test-Path -LiteralPath $sourceExe)) {
            throw "no unsigned sample exe for signing self-test (build engine first: cargo build -p game)"
        }
        $sample = Join-Path $temp "sample.exe"
        Copy-Item -LiteralPath $sourceExe -Destination $sample -Force

        $cert = New-SelfSignedCertificate -Type CodeSigningCert `
            -Subject ("CN=RustLua SelfTest " + [guid]::NewGuid().ToString("N")) `
            -CertStoreLocation "Cert:\CurrentUser\My" `
            -KeyUsage DigitalSignature `
            -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3") `
            -NotAfter (Get-Date).AddDays(1)
        Write-Host ("[INFO] self-signed cert thumbprint=" + $cert.Thumbprint)

        # 1) 证书对象直接签
        Sign-WithPowerShell $cert $sample
        $sig1 = Get-SignatureStatus $sample
        if (-not $sig1.IsSigned) { throw "self-test: file is not signed (status=$($sig1.Status))" }
        if ($sig1.Thumbprint -ne $cert.Thumbprint) { throw "self-test: signer thumbprint mismatch" }
        Write-Host ("[PASS] authenticode signature status=" + $sig1.Status + " signer=" + $sig1.Subject)

        # 2) PFX 路径（导出/再导入）也走一遍
        $pfx = Join-Path $temp "selftest.pfx"
        $pwd = ConvertTo-SecureString -String "rustlua-selftest" -AsPlainText -Force
        Export-PfxCertificate -Cert $cert -FilePath $pfx -Password $pwd | Out-Null
        $imported = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2(
            $pfx, $pwd, [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]::Exportable)
        $sample2 = Join-Path $temp "sample2.exe"
        Copy-Item -LiteralPath $sample -Destination $sample2 -Force
        Sign-WithPowerShell $imported $sample2
        $sig2 = Get-SignatureStatus $sample2
        if (-not $sig2.IsSigned) { throw "self-test: PFX signing failed" }
        Write-Host ("[PASS] PFX signature status=" + $sig2.Status)

        # 3) 篡改检测：改 PE **内容区**的字节（文件中部）。不要改尾部证书表——
        # 改证书表会让签名直接读不出来（NotSigned），那是另一种失败，不能证明哈希校验生效。
        $bytes = [System.IO.File]::ReadAllBytes($sample2)
        $offset = [int]($bytes.Length / 2)
        $bytes[$offset] = [byte]($bytes[$offset] -bxor 0xFF)
        [System.IO.File]::WriteAllBytes($sample2, $bytes)
        $tampered = Get-SignatureStatus $sample2
        if ($tampered.Status -ne "HashMismatch") {
            throw "self-test: tampered file status=$($tampered.Status), expect HashMismatch"
        }
        Write-Host ("[PASS] tamper detected status=" + $tampered.Status)

        $tool = Find-SignTool
        if ($tool) {
            Write-Host ("[INFO] signtool available: " + $tool + " (trusted-chain verification needs a real cert; not run in self-test)")
        } else {
            Write-Host "[INFO] signtool not found; self-test exercised the Set-AuthenticodeSignature fallback"
        }

        Write-Host "signing_selftest_ok signer=authenticode tamper=HashMismatch" -ForegroundColor Green
        exit 0
    } finally {
        $TimestampUrl = $savedTimestamp
        if ($cert) {
            Remove-Item -LiteralPath ("Cert:\CurrentUser\My\" + $cert.Thumbprint) -Force -ErrorAction SilentlyContinue
        }
        Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue
    }
}

if ($Path.Count -eq 0) {
    throw "nothing to sign: pass -Path <file|dir> (or -SelfTest)"
}
$files = Resolve-SignFiles $Path
if ($files.Count -eq 0) {
    throw "no signable files (*.exe/*.dll/*.msi/*.sys) under: $($Path -join ', ')"
}

$tool = Find-SignTool
$cert = $null
if (-not $VerifyOnly) {
    $cert = Get-CertificateFromInputs
    if (-not $cert) {
        $msg = "signing_skipped reason=no_certificate files=$($files.Count)"
        if ($RequireSignature) {
            Write-Host ("[FAIL] " + $msg) -ForegroundColor Red
            exit 1
        }
        Write-Host ("[BLOCKED] " + $msg + " (pass -PfxPath/-Thumbprint, or set RUSTLUA_REQUIRE_SIGNING=1 to make this fail)") -ForegroundColor Yellow
        exit 3
    }
}

$failed = 0
foreach ($file in $files) {
    Write-Host ("==> " + $file) -ForegroundColor Cyan
    if ($VerifyOnly) {
        if ($tool) {
            $code = Invoke-SignToolVerify $tool $file
            if ($code -ne 0) { Write-Host ("[FAIL] signtool verify /pa: " + $file) -ForegroundColor Red; $failed++; continue }
        }
        $sig = Get-SignatureStatus $file
        if (-not $sig.IsSigned) { Write-Host ("[FAIL] not signed: " + $file) -ForegroundColor Red; $failed++; continue }
        Write-Host ("[PASS] verify status=" + $sig.Status + " signer=" + $sig.Subject)
        continue
    }

    if ($tool) {
        $pfxFull = if (-not [string]::IsNullOrWhiteSpace($PfxPath)) { (Resolve-Path -LiteralPath $PfxPath).Path } else { "" }
        $code = Invoke-SignToolSign $tool $file $pfxFull $Thumbprint
        if ($code -ne 0) { Write-Host ("[FAIL] signtool sign exit=" + $code + ": " + $file) -ForegroundColor Red; $failed++; continue }
    } else {
        Sign-WithPowerShell $cert $file
    }

    $sig = Get-SignatureStatus $file
    if (-not $sig.IsSigned) {
        Write-Host ("[FAIL] signature missing after sign: " + $file) -ForegroundColor Red
        $failed++
        continue
    }
    Write-Host ("[PASS] signed status=" + $sig.Status + " signer=" + $sig.Subject)
}

if ($failed -gt 0) {
    Write-Host ("signing_failed files=" + $failed) -ForegroundColor Red
    exit 1
}
Write-Host ("signing_ok files=" + $files.Count + " tool=" + $(if ($tool) { "signtool" } else { "authenticode" })) -ForegroundColor Green
exit 0
