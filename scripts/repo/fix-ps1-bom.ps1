# 给仓库里所有缺 UTF-8 BOM 的 `.ps1` 补 BOM（幂等）。
#
# 为什么需要它：PowerShell 5.1 在没有 BOM 时按 ANSI 解码，中文注释会被打散、引号错位，
# 于是一份"看起来没问题"的脚本报出上百个解析错误。本轮实测：`scripts/build/package.ps1`
# 反复被外部工具重写成**不带 BOM**，每次都会让打包脚本静默走错分支（症状是微信包里
# 内核与胶水位置错位，运行时 `Cannot find module './game_web_bg.wasm'`）——
# 离真正原因非常远，所以做成一条随时能跑的命令。
#
# `scripts/verify/check-docs.ps1` 的 `ps1_bom_ok` 就是这条判据的门；本脚本只负责修。
#
# 用法：powershell -NoProfile -File scripts/repo/fix-ps1-bom.ps1 [仓库根]

param(
    [string]$RepoRoot = ""
)

$ErrorActionPreference = "Stop"
if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
}
$RepoRoot = (Resolve-Path -LiteralPath $RepoRoot).Path

$fixed = 0
$scanned = 0
Get-ChildItem -LiteralPath $RepoRoot -Recurse -Filter *.ps1 -File -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -notmatch '[\\/]\.git[\\/]' -and $_.FullName -notmatch '[\\/]node_modules[\\/]' -and $_.FullName -notmatch '[\\/]\.tools[\\/]' } |
    ForEach-Object {
        $scanned += 1
        $bytes = [System.IO.File]::ReadAllBytes($_.FullName)
        if ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF) {
            return
        }
        $text = [System.Text.Encoding]::UTF8.GetString($bytes)
        if ($text.Length -gt 0 -and $text[0] -eq [char]0xFEFF) { $text = $text.Substring(1) }
        [System.IO.File]::WriteAllText($_.FullName, $text, (New-Object System.Text.UTF8Encoding($true)))
        $fixed += 1
        Write-Output ("fix_ps1_bom_patched {0}" -f $_.FullName.Substring($RepoRoot.Length).TrimStart('\'))
    }

Write-Output ("fix_ps1_bom_ok scanned={0} patched={1}" -f $scanned, $fixed)
