# test-bundle.ps1：通用的「本地试玩一个 web 平台发布产物」入口。
#
# 一个游戏工程的 web 平台产物落在 `examples\<工程>\builds\<平台>\`（2026-09-28 起不再按
# publish_name 建子目录），但试玩这件事与工程名、平台名、目录深度都无关：**定位产物 → 交给该平台既有的验收脚本
# 断言 → 起本地服务**。这里就是这个与平台无关的入口，webgl 与微信小游戏共用。
#
# 分工（严格）：本脚本**不重复实现任何产物断言**。断言属于
#   * webgl            -> scripts\verify\verify-webgl-bundle.ps1（产物完整 + wasm 魔数 + 无头浏览器读 data-rustlua-boot/画面采样）
#   * wechat-minigame  -> scripts\verify\verify-wechat-minigame.ps1（静态布局/配额 + Node 伪造 wx 宿主跑帧）
# 两边的验收标准只会有一处，不会出现「试玩脚本说通过、验收费劲却说不通过」的分叉。
#
# 服务根按形态决定：
#   * webgl              -> 发布目录本身（浏览器直接打开 index.html）
#   * 微信小游戏 全量包   -> 不需要本地服务，用开发者工具导入发布目录即可（资源都在包内）
#   * 微信小游戏 瘦首包   -> 服务**资源包目录**（约定位置 <发布根>/assets，与 launcher/ 同级），
#                           端口默认沿用 remote-assets.json 里烧的那个；否则包内骨架能跑、重资源全读不到
#
# 用法：
#   powershell -NoProfile -File scripts\test\test-bundle.ps1
#   powershell -NoProfile -File scripts\test\test-bundle.ps1 -Platform webgl -BundleRoot examples\game_jhyt\builds\webgl
#   powershell -NoProfile -File scripts\test\test-bundle.ps1 -Platform wechat-minigame -NoVerify
#   powershell -NoProfile -File scripts\test\test-bundle.ps1 -Platform wechat-minigame -AssetPackRoot D:\cdn\jianghu -Port 8899
#   powershell -NoProfile -File scripts\test\test-bundle.ps1 -BindHost 0.0.0.0      # 让同一 WiFi 的手机也能打开
#
# 退出码（与两个验收脚本统一）：
#   0 = 验证通过并正常退出服务
#   1 = 验证失败（产物坏了）
#   2 = 环境受限（无 node / 无浏览器 / 无静态服务）——**不是通过**，日志里会显式标出
#   3 = 未找到产物，或本机有多个产物需要 -BundleRoot 显式指定，或瘦首包找不到资源包目录
#   4 = 找不到可用端口

[CmdletBinding()]
param(
    # Target platform whose release bundle to test.
    [ValidateSet("webgl", "wechat-minigame")]
    [string]$Platform = "webgl",
    # Release bundle directory. Empty = auto-discover the single valid bundle under examples\*\builds\<Platform>\.
    [string]$BundleRoot = "",
    # Local static server port. webgl defaults to 8137; a WeChat thin first package defaults to the
    # port baked into its remote-assets.json (serving it on another port = DevTools fetches a dead address).
    [int]$Port = 0,
    # Listen address for the local static server. Default 127.0.0.1 = loopback only (a phone on the
    # same LAN cannot reach it). Pass 0.0.0.0 to let another device open the same page.
    # 名字刻意不叫 -Host：`$Host` 是 PowerShell 的自动变量（宿主信息），覆盖它有副作用。
    [string]$BindHost = "127.0.0.1",
    # WeChat asset pack directory (thin first package only). Empty = <BundleRoot>/../assets.
    [string]$AssetPackRoot = "",
    # Serve over HTTPS with a self-signed dev certificate (scripts\dev\https-cert.ps1).
    # 为什么值得开：Chromium 只在**安全上下文**（HTTPS / localhost）才在请求头里宣告 `br`。
    # 同一个 bundle，局域网 HTTP 只能 gzip 到 2.07 MB，HTTPS 走 brotli 是 1.76 MB（wasm 一份）。
    # 代价：手机首次会拦一次证书，点「高级 → 继续前往」。
    [switch]$Tls,
    # Skip the platform verifier and only start the local server (for machines without a browser).
    [switch]$NoVerify
)

$ErrorActionPreference = "Stop"
$workspaceRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
. (Join-Path $PSScriptRoot "..\lib\bundle-discovery.ps1")

# 本机局域网 IPv4（同一 WiFi 下手机该开哪个地址由它给，别让人从 ipconfig 里猜）。
# 只在监听地址不是回环时才打印——绑回环时那些地址**连不上**，打出来只会误导。
function Get-LanUrls([string]$ListenHost, [int]$ListenPort) {
    if ($ListenHost -in @("127.0.0.1", "localhost", "::1")) { return @() }
    $addresses = @()
    try {
        $addresses = Get-NetIPAddress -AddressFamily IPv4 -ErrorAction Stop |
            Where-Object { $_.IPAddress -notlike "127.*" -and $_.IPAddress -notlike "169.254.*" } |
            Select-Object -ExpandProperty IPAddress
    }
    catch {
        # 老系统/受限环境没有 Get-NetIPAddress：退回 ipconfig 解析（口径同上，够用）。
        $addresses = (ipconfig | Select-String -Pattern "IPv4.*:\s*([0-9.]+)" -AllMatches) |
            ForEach-Object { $_.Matches } | ForEach-Object { $_.Groups[1].Value } |
            Where-Object { $_ -notlike "127.*" -and $_ -notlike "169.254.*" }
    }
    return @($addresses | Sort-Object -Unique | ForEach-Object { "http://${_}:$ListenPort/" })
}

# 端口 0 = "用平台默认值"：webgl 8137；微信瘦首包用 remote-assets.json 里烧进去的那个端口。
$portGiven = $PSBoundParameters.ContainsKey("Port") -and $Port -gt 0
if ($Port -le 0) { $Port = 8137 }

function Test-PortFree([int]$Candidate) {
    $client = New-Object System.Net.Sockets.TcpClient
    try {
        $client.Connect("127.0.0.1", $Candidate)
        return $false   # 连得上 = 已有进程在监听
    }
    catch {
        return $true
    }
    finally {
        $client.Close()
    }
}

# ---------------------------------------------------------------- 1. 定位产物
$bundle = ""
if (-not [string]::IsNullOrWhiteSpace($BundleRoot)) {
    if (-not (Test-Path -LiteralPath $BundleRoot)) {
        Write-Host "[RED] 产物目录不存在: $BundleRoot"
        exit 3
    }
    $bundle = (Resolve-Path -LiteralPath $BundleRoot).Path
    $reason = Get-BundleRejectReason -Path $bundle -Platform $Platform
    if ($reason) {
        Write-Host "[RED] $bundle 不是合格的 $Platform 产物：$reason"
        exit 3
    }
}
else {
    $bundle = Resolve-GameBundle -Platform $Platform
    if (-not $bundle) { exit 3 }
}

# ---------------------------------------------------------------- 2. 端口预检（webgl）
# 验收脚本自己也要在 $Port 上起一次服务，端口被占时它按设计**直接判红**（静默复用会让断言
# 读到另一个产物）。但试玩入口不该因为上一次运行/后台任务残留的静态服务就当场退出——双击时
# 窗口一关，连原因都看不见。所以验收前先在 $Port..$Port+10 里挑一个空闲端口，验收与服务都用
# 它；下面打印的 URL 就是实际端口。整段都被占才按既有契约退 4。
#
# 判据用"监听表"而不是连一下 127.0.0.1：验收脚本的闸门就是按监听判定的，两处口径必须一致，
# 否则会挑到一个"连不上回环、但它确实在监听"的端口，验收照样判红。
function Test-PortInUse([int]$Candidate) {
    $listener = Get-NetTCPConnection -LocalPort $Candidate -State Listen -ErrorAction SilentlyContinue |
        Select-Object -First 1
    return [bool]$listener
}

if ($Platform -eq "webgl" -and (Test-PortInUse $Port)) {
    $requestedPort = $Port
    for ($offset = 1; $offset -le 10; $offset++) {
        if (-not (Test-PortInUse $Port)) { break }
        $Port = $requestedPort + $offset
    }
    if (Test-PortInUse $Port) {
        Write-Host ("[RED] 端口 {0}-{1} 都被占用；请用 -Port 指定一个空闲端口。" -f $requestedPort, ($requestedPort + 10))
        exit 4
    }
    Write-Host ("[YELLOW] 端口 {0} 已被占用，改用 {1}（验收与服务都用 {1}）。" -f $requestedPort, $Port)
}

# ---------------------------------------------------------------- 3. 平台验收
# 子进程调用：两个验收脚本都用 `exit N` 收尾，点源或在同进程调用会把本脚本一起结束掉。
$verifyCode = 0
if (-not $NoVerify) {
    $verifier = if ($Platform -eq "webgl") {
        Join-Path $PSScriptRoot "..\verify\verify-webgl-bundle.ps1"
    }
    else {
        Join-Path $PSScriptRoot "..\verify\verify-wechat-minigame.ps1"
    }
    if (-not (Test-Path -LiteralPath $verifier)) { throw "缺少平台验收脚本: $verifier" }
    $verifierArgs = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $verifier, "-BundleRoot", $bundle)
    if ($Platform -eq "webgl") { $verifierArgs += @("-Port", "$Port") }
    & powershell @verifierArgs
    $verifyCode = $LASTEXITCODE

    if ($verifyCode -eq 1) {
        Write-Host "[RED] 产物验证失败（exit 1）：$bundle"
        Write-Host "     证据（DOM 快照与 summary JSON）见 artifacts\logs\"
        exit 1
    }
    if ($verifyCode -eq 2) {
        # 环境受限：本机没有 node/浏览器，断言根本没跑起来。这时**仍然**起服务让人肉眼看，
        # 但绝不能把结果报成通过——那是仓库一直在拒绝的假绿。
        Write-Host "[YELLOW] 环境受限（exit 2）：缺少 node 或浏览器，产物断言未执行完。"
        Write-Host "[YELLOW] 下面起的本地服务仅供肉眼看，结果不算通过。"
    }
}
else {
    Write-Host "[YELLOW] -NoVerify：已跳过平台验收，只起本地服务。"
}

# ---------------------------------------------------------------- 4. 本地服务
if ($Platform -eq "wechat-minigame") {
    # 入口永远是微信开发者工具（产物目录只读，不会写入玩家数据）。但**瘦首包**（-AssetPack remote）
    # 的重资源不在包里，而在导出的资源包目录里：工具预览时必须有一个静态服务提供 `<base>/pack/**`，
    # 否则包内骨架能跑起来、贴图与场景却全部读不到（真机同理，只是换成了公网 HTTPS CDN）。
    Write-Host "微信小游戏产物：$bundle"
    $packAssetDirs = @("assets", "scenes", "scripts") | Where-Object { Test-Path -LiteralPath (Join-Path $bundle ("pack\" + $_)) }
    $isThinPack = ($packAssetDirs.Count -eq 0) -and (Test-Path -LiteralPath (Join-Path $bundle "remote-assets.json"))
    if (-not $isThinPack) {
        # 这里同时覆盖 full 与 subpackage 两种形态：两者的资源都在**代码包**里
        # （分包形态下 `pack/` 是分包、由微信自己分发并挂载），都不需要自建静态服务。
        Write-Host "用微信开发者工具「导入项目」指向该目录即可运行（资源都在代码包内，不需要本地服务）。"
        Write-Host "  * 分包形态（默认）：首次导入后核对「详情 -> 代码包大小」的主包 / 分包两块数值。"
        exit $verifyCode
    }

    # 瘦首包必须让服务端口与 remote-assets.json 里的基址一致，否则工具去拉一个没人监听的地址。
    $configPath = Join-Path $bundle "remote-assets.json"
    $baseUrl = ""
    $configHost = ""
    $configPort = 8787
    try {
        $remoteConfig = Get-Content -LiteralPath $configPath -Raw -Encoding UTF8 | ConvertFrom-Json
        $baseUrl = [string]$remoteConfig.packBaseUrl
        $uri = [Uri]$baseUrl
        $configHost = $uri.Host
        if ($uri.Port -gt 0) { $configPort = $uri.Port }
    }
    catch {
        Write-Host "[RED] 读不出 remote-assets.json 的 packBaseUrl：$($_.Exception.Message)"
        exit 1
    }
    if (-not $portGiven) { $Port = $configPort }

    if ($configHost -and $configHost -notin @("127.0.0.1", "localhost", "::1")) {
        # 配置指向公网 CDN：本地起服务对它毫无意义（要验的是那份 CDN）。
        Write-Host "远端基址是 $baseUrl（主机 $configHost），不是本机地址——本地服务对它没有意义。"
        Write-Host "  * 开发者工具/真机会直接访问该 CDN：确认已上传资源包、且已勾选「不校验合法域名…」；"
        Write-Host "    真机还需要公网 HTTPS + 公众平台 downloadFile 白名单。"
        Write-Host "  * 想本地联调：用 -AssetPack remote -RemotePackBaseUrl http://127.0.0.1:8787 重新发布。"
        exit $verifyCode
    }

    $assetPack = $AssetPackRoot
    if ([string]::IsNullOrWhiteSpace($assetPack)) {
        # 约定形态：<发布根>/{launcher,assets}（$bundle 就是 launcher）；老形态 <发布目录>-assets 仍然认。
        $candidates = @(
            (Join-Path (Split-Path -Parent $bundle) "assets"),
            ($bundle + "-assets")
        )
        foreach ($candidate in $candidates) {
            if (Test-Path -LiteralPath (Join-Path $candidate "pack")) { $assetPack = $candidate; break }
        }
    }
    if ([string]::IsNullOrWhiteSpace($assetPack) -or -not (Test-Path -LiteralPath (Join-Path $assetPack "pack"))) {
        Write-Host "[RED] 这是瘦首包，但找不到资源包目录（约定位置：与 launcher/ 同级的 assets/pack/**）。"
        Write-Host "      用 -AssetPackRoot <资源包目录> 显式指定；它由 -AssetPack remote 发布时导出。"
        exit 3
    }
    $assetPack = (Resolve-Path -LiteralPath $assetPack).Path

    $node = Get-Command node -ErrorAction SilentlyContinue
    if (-not $node) {
        Write-Host "[YELLOW] 未找到 node.js，无法起本地静态服务；资源包目录：$assetPack"
        if ($verifyCode -eq 0) { exit 2 }
        exit $verifyCode
    }

    # 端口被占就换一个（与 webgl 同样的理由：连上别人的旧服务会让断言/预览作用在错误的产物上）。
    $listenPort = $Port
    for ($offset = 1; $offset -le 10 -and -not (Test-PortFree $listenPort); $offset++) {
        $listenPort = $Port + $offset
    }
    if (-not (Test-PortFree $listenPort)) {
        Write-Host "[RED] 端口 $Port-$($Port + 10) 都被占用；请用 -Port 指定一个空闲端口。"
        exit 4
    }
    if ($listenPort -ne $Port) {
        Write-Host ("[YELLOW] 端口 {0} 已被占用，改用 {1}；但 remote-assets.json 烧的是 {0}，" -f $Port, $listenPort)
        Write-Host ("          开发者工具会去拉 {0}。要么腾出该端口，要么用 -RemotePackBaseUrl http://127.0.0.1:{1} 重新发布。" -f $Port, $listenPort)
    }

    Write-Host ""
    Write-Host ("==> 资源包服务：{0}" -f $assetPack)
    Write-Host ("==> 实际地址：http://127.0.0.1:{0}/pack/（remote-assets.json 里配置的是 {1}）" -f $listenPort, $baseUrl)
    # 同网段访问：只有显式 -BindHost 0.0.0.0 时才有意义（绑回环别人连不上）。
    $packLanUrls = Get-LanUrls -ListenHost $BindHost -ListenPort $listenPort
    if ($packLanUrls.Count -gt 0) {
        Write-Host ("==> 同一局域网的手机可用：{0}（同一 WiFi；防火墙放行 TCP {1}）" -f ($packLanUrls -join "  "), $listenPort)
    }
    else {
        Write-Host "==> 手机访问：本次只绑回环，别的设备连不上。要真机就传监听地址重跑一次："
        Write-Host "      产物目录里的 test-webgl.bat：  test-webgl.bat <端口> 0.0.0.0"
        Write-Host "      直接用本脚本：                -BindHost 0.0.0.0"
    }
    Write-Host ("==> 开发者工具「导入项目」选择：{0}" -f $bundle)
    Write-Host "==> 并勾选「详情 → 本地设置 → 不校验合法域名、web-view（业务域名）、TLS 版本以及 HTTPS 证书」"
    Write-Host "==> 按 Ctrl-C 停止服务（服务停止后包内骨架仍能启动，但重资源会读不到）"
    & $node.Source (Join-Path $PSScriptRoot "..\dev\pack-server.js") --root $assetPack --port "$listenPort" --host $BindHost

    if ($verifyCode -ne 0) { exit $verifyCode }
    exit 0
}

$node = Get-Command node -ErrorAction SilentlyContinue
if (-not $node) {
    Write-Host "[YELLOW] 未找到 node.js，无法起本地静态服务；产物目录：$bundle"
    if ($verifyCode -eq 0) { exit 2 }
    exit $verifyCode
}

# 端口被占就换一个：产物验收脚本里也起过一次服务，上一次异常退出的 node 可能还占着端口；
# 若把新服务起失败（EADDRINUSE），无头浏览器会连上**旧的**服务器去访问另一个包，
# 断言就在错误的产物上通过了。
$listenPort = $Port
for ($offset = 1; $offset -le 10 -and -not (Test-PortFree $listenPort); $offset++) {
    $listenPort = $Port + $offset
}
if (-not (Test-PortFree $listenPort)) {
    Write-Host "[RED] 端口 $Port-$($Port + 10) 都被占用；请用 -Port 指定一个空闲端口。"
    exit 4
}
if ($listenPort -ne $Port) {
    Write-Host "[YELLOW] 端口 $Port 已被占用，改用 $listenPort。"
}

Write-Host ("==> serving {0}" -f $bundle)
$scheme = if ($Tls) { "https" } else { "http" }
if ($Tls) {
    # 出/复用自签名证书，并把 PFX 路径与口令交给 serve-static.js（它读这两个环境变量起 https）。
    $certScript = Join-Path $PSScriptRoot "..\dev\https-cert.ps1"
    & powershell -NoProfile -ExecutionPolicy Bypass -File $certScript -Quiet | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "https-cert.ps1 失败（exit $LASTEXITCODE）：没有证书就不该假装开了 HTTPS。" }
    $env:RUSTLUA_TLS_PFX = Join-Path $workspaceRoot ".temp\https\rustlua-dev.pfx"
    $env:RUSTLUA_TLS_PASS = (Get-Content -LiteralPath (Join-Path $workspaceRoot ".temp\https\pass.txt") -Raw)
}
Write-Host ("==> open {0}://127.0.0.1:{1}/ in a browser (Ctrl-C to stop)" -f $scheme, $listenPort)
if ($Tls) {
    Write-Host "==> HTTPS 自签名证书：浏览器/手机会拦一次，点「高级 → 继续前往」；此后 brotli 才生效（wasm 2.07 MB → 1.76 MB）。"
}
# 同一 WiFi 下的手机该开哪个地址：绑到 0.0.0.0 时打印本机局域网地址（此时手机真的连得上）。
# `serve-static.js` 自己也会打一行 `LAN:`；这里再打一行是为了让 **-BindHost 的两种取值**下
# 都有一句明确的"手机能不能用"，不必让人去比对端口与网卡。
$lanUrls = @(Get-LanUrls -ListenHost $BindHost -ListenPort $listenPort | ForEach-Object { $_ -replace "^http://", "${scheme}://" })
if ($lanUrls.Count -gt 0) {
    Write-Host ("==> 同一局域网的手机可用：{0}（同一 WiFi；防火墙放行 TCP {1}）" -f ($lanUrls -join "  "), $listenPort)
}
else {
    Write-Host "==> 手机访问：本次只绑回环，别的设备连不上。要真机就传监听地址重跑一次："
    Write-Host "      产物目录里的 test-webgl.bat：  test-webgl.bat <端口> 0.0.0.0"
    Write-Host "      直接用本脚本：                -BindHost 0.0.0.0"
}
& $node.Source (Join-Path $PSScriptRoot "..\dev\serve-static.js") $bundle "$listenPort" $BindHost

if ($verifyCode -ne 0) { exit $verifyCode }
exit 0
