#requires -Version 5.1
<#
.SYNOPSIS
    起一个本地静态资源服务，作为远程 CDN 的测试替身（开发者工具验证远端回落用）。

.DESCRIPTION
    服务 `-Root` 目录（默认微信小游戏发布根下的 `assets\`，即 CDN 资源包）。配合发布期参数使用：

        # 1) 发布时把远端基址写进首包的 remote-assets.json（不改包内 pack/ 的优先级）
        powershell -File scripts/package.ps1 -Platform wechat-minigame -SkipBuild `
            -RemotePackBaseUrl http://127.0.0.1:8787/pack/

        # 2) 起本地服务（前台运行，可直接作为后台任务管理）
        powershell -File scripts/serve-pack.ps1 -Port 8787

    边界：开发者工具需要勾选「不校验合法域名、TLS 版本以及 HTTPS 证书」；
    真机访问不到本机 127.0.0.1，且必须 HTTPS + mp 后台 downloadFile 合法域名。
    发布根布局是 builds/<平台>/{launcher,assets}，服务的是 assets/，开发者工具导入的是 launcher/。
#>
param(
    [string]$Root = "examples\game_jhyt\builds\wechat-minigame\assets",
    [int]$Port = 8787,
    # 参数不能叫 $Host：那是 PowerShell 的只读自动变量，赋值会直接报
    # "Cannot overwrite variable Host because it is read-only or constant."（实测启动即失败）。
    [string]$BindAddress = "127.0.0.1",
    [string]$Log = ""
)

$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$node = Get-Command node -ErrorAction SilentlyContinue
if (-not $node) { throw "未找到 node，无法启动本地资源服务" }

$serverScript = Join-Path $PSScriptRoot "pack-server.js"
if (-not (Test-Path -LiteralPath $serverScript)) { throw "缺少服务脚本: $serverScript" }

$resolvedRoot = (Resolve-Path -LiteralPath $Root).Path
if (-not (Test-Path -LiteralPath (Join-Path $resolvedRoot "pack"))) {
    throw "资源根里没有 pack/（应为资源包目录 <发布根>\assets，或 pack/ 完整的发布目录）: $resolvedRoot"
}

Write-Host "==> 本地资源服务 root=$resolvedRoot port=$Port" -ForegroundColor Cyan
Write-Host "    远端基址（填给 -RemotePackBaseUrl）: http://$BindAddress`:$Port/pack/" -ForegroundColor DarkGray
Write-Host "    开发者工具需勾选「不校验合法域名、TLS 版本以及 HTTPS 证书」" -ForegroundColor DarkGray

$arguments = @($serverScript, "--root", $resolvedRoot, "--port", $Port, "--host", $BindAddress)
if ($Log) { $arguments += @("--log", $Log) }

# 前台运行：由调用方决定是否放到后台任务里；日志直接透传，便于实时观察请求。
& $node.Source @arguments
exit $LASTEXITCODE
