# https-cert.ps1：给本地试玩服务出一张**自签名**证书（PFX），供 `serve-static.js` 的 HTTPS 模式用。
#
# 为什么需要它：Chromium 只在**安全上下文**（HTTPS 或 localhost）才在请求头里宣告
# `Accept-Encoding: br`。局域网 HTTP 地址实测只报 `gzip, deflate` ⇒ 浏览器永远拿不到 brotli，
# 5.16 MB 的 wasm 只能压到 2.07 MB 而不是 1.76 MB。这条脚本就是解锁那 15% 的钥匙。
#
# 产出（都在 `.temp` 下，仓库已忽略）：`<repo>\.temp\https\rustlua-dev.pfx` + 同目录的 `pass.txt`。
# 幂等：PFX 存在且 30 天内不过期就复用，不重复建。
#
# 用法：
#   powershell -File scripts\dev\https-cert.ps1              # 建/复用并打印路径
#   powershell -File scripts\dev\https-cert.ps1 -PrintEnv    # 输出设置环境变量的两行（给脚本 eval）
#
# 手机侧：自签名证书会拦一次，点「高级 → 继续前往」即可；此后该地址按 HTTPS 走，
# 浏览器就会宣告 brotli。想彻底免提示可以把这张证书装进手机受信任根（不推荐，dev 用）。

[CmdletBinding()]
param(
    # 只打印 `$env:RUSTLUA_TLS_PFX=...` / `$env:RUSTLUA_TLS_PASS=...` 两行，供调用方拼接。
    [switch]$PrintEnv,
    # 被 test-bundle.ps1 调用时只要一行进度，别把 pfx/pass 与用法全打出来。
    [switch]$Quiet
)

$ErrorActionPreference = "Stop"
$workspaceRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$dir = Join-Path $workspaceRoot ".temp\https"
$pfx = Join-Path $dir "rustlua-dev.pfx"
$passFile = Join-Path $dir "pass.txt"
$subject = "CN=RustLua Local Dev"

function New-DevCert {
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    # 证书口令只用于保护 PFX 文件本身（本地 dev，无保密价值）。
    $password = -join ((48..57) + (97..122) | Get-Random -Count 24 | ForEach-Object { [char]$_ })
    Set-Content -LiteralPath $passFile -Value $password -Encoding ASCII -NoNewline

    # SAN 必须同时含 IP 与 localhost：手机按 IP 访问、桌面按 127.0.0.1/localhost 访问，
    # 缺一样就是 "证书对该地址无效" 的拦截（而不是"未知颁发者"那次可继续的拦截）。
    $ips = @("127.0.0.1") + @(
        Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue |
            Where-Object { $_.IPAddress -notlike "127.*" -and $_.IPAddress -notlike "169.254.*" } |
            Select-Object -ExpandProperty IPAddress
    ) | Select-Object -Unique
    $san = (@("DNS=localhost", "DNS=127.0.0.1") + ($ips | ForEach-Object { "IPAddress=$_" })) -join "&"

    $cert = New-SelfSignedCertificate `
        -Subject $subject `
        -TextExtension @("2.5.29.17={text}$san") `
        -CertStoreLocation "Cert:\CurrentUser\My" `
        -NotAfter (Get-Date).AddDays(365) `
        -KeyAlgorithm RSA -KeyLength 2048 -HashAlgorithm SHA256

    try {
        $secure = ConvertTo-SecureString -String $password -Force -AsPlainText
        Export-PfxCertificate -Cert $cert -FilePath $pfx -Password $secure | Out-Null
    }
    finally {
        # 私钥留在用户证书库里对 dev 没用，清掉避免越攒越多。
        Remove-Item -LiteralPath ("Cert:\CurrentUser\My\" + $cert.Thumbprint) -Force -ErrorAction SilentlyContinue
    }
    Write-Host "==> 已生成自签名证书：$pfx（SAN: $san）" -ForegroundColor DarkGray
}

$reuse = $false
if ((Test-Path -LiteralPath $pfx) -and (Test-Path -LiteralPath $passFile)) {
    try {
        $probe = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new(
            $pfx, (Get-Content -LiteralPath $passFile -Raw))
        # 30 天余量：别在联调中途过期。
        $reuse = $probe.NotAfter -gt (Get-Date).AddDays(30)
    }
    catch { $reuse = $false }
}
if (-not $reuse) { New-DevCert }

$password = Get-Content -LiteralPath $passFile -Raw
if ($PrintEnv) {
    Write-Output ("`$env:RUSTLUA_TLS_PFX=" + "'" + $pfx + "'")
    Write-Output ("`$env:RUSTLUA_TLS_PASS=" + "'" + $password + "'")
}
elseif ($Quiet) {
    # 被 test-bundle.ps1 调用：只报一句"证书就绪"，细节由调用方打。
    Write-Host "==> dev TLS certificate ready（自签名，SAN 含 localhost 与本机 IPv4）" -ForegroundColor DarkGray
}
else {
    Write-Host "pfx  : $pfx"
    Write-Host "pass : $password"
    Write-Host "用法 : `$env:RUSTLUA_TLS_PFX='$pfx'; `$env:RUSTLUA_TLS_PASS='$password'; node scripts/dev/serve-static.js <root> <port> 0.0.0.0"
}
