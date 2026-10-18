# 分发布局应用器（docs/67 §5.4）：assetc layout --json → bundle-layout.js --apply → --verify。
#
# 输入是**发布根**（含 pack/：RLB 容器 + bundle_plan.json + game.json + pack/game.js）。
# 这是把「单元 → 根」真正落到目录上的唯一入口；package.ps1 接线时只调这一个脚本。
#
# 用法：
#   powershell -File scripts/build/apply-bundle-layout.ps1 -Launcher <发布根> [-Assetc <assetc.exe>] [-SkipVerify]
param(
    [Parameter(Mandatory = $true)][string]$Launcher,
    [string]$Assetc = "",
    [switch]$SkipVerify
)
$ErrorActionPreference = "Stop"
$launcherRoot = (Resolve-Path -LiteralPath $Launcher).Path
$packRoot = Join-Path $launcherRoot "pack"
if (-not (Test-Path -LiteralPath (Join-Path $packRoot "bundle_plan.json"))) {
    throw "缺少 bundle_plan.json：$packRoot（先 assetc package --bundle grouped）"
}
if ([string]::IsNullOrWhiteSpace($Assetc)) {
    $repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
    $Assetc = Join-Path $repo "engine\target\release\assetc.exe"
}
if (-not (Test-Path -LiteralPath $Assetc)) { throw "找不到 assetc：$Assetc" }
$node = (Get-Command node -ErrorAction SilentlyContinue)
if (-not $node) { throw "需要 node 运行 bundle-layout.js" }
$tool = Join-Path $PSScriptRoot "bundle-layout.js"
if (-not (Test-Path -LiteralPath $tool)) { throw "找不到 $tool" }

$layoutFile = Join-Path $launcherRoot ".bundle-layout.json"
$layoutJson = & $Assetc layout $packRoot --json
if ($LASTEXITCODE -ne 0) { throw "assetc layout 失败（exit $LASTEXITCODE）" }
# 用 .NET 写，避免 PowerShell 5.1 的重定向编码/BOM 差异。
[System.IO.File]::WriteAllText($layoutFile, ($layoutJson -join "`n"), (New-Object System.Text.UTF8Encoding($false)))

& $node.Source $tool --file $layoutFile --apply $launcherRoot
if ($LASTEXITCODE -ne 0) { throw "bundle-layout apply 失败（exit $LASTEXITCODE）" }
if (-not $SkipVerify) {
    & $node.Source $tool --verify $launcherRoot
    if ($LASTEXITCODE -ne 0) { throw "bundle-layout verify 失败（exit $LASTEXITCODE）" }
}
Write-Host "apply_bundle_layout_ok launcher=$launcherRoot"