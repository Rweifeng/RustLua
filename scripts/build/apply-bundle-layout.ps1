# 分发布局应用器（docs/67 §5.4）：assetc layout --json → bundle-layout.js --apply → --verify →
# check-bundle-mounts → check-boot-closure → assetc closure。
#
# 输入是**发布根**（含 pack/：RLB 容器 + bundle_plan.json + game.json + pack/game.js）。
# 这是把「单元 → 根」真正落到目录上的唯一入口；package.ps1 接线时只调这一个脚本。
#
# 用法：
#   powershell -File scripts/build/apply-bundle-layout.ps1 -Launcher <发布根> [-Assetc <assetc.exe>]
#            [-CdnDir <远端镜像目录>] [-SkipVerify]
param(
    [Parameter(Mandatory = $true)][string]$Launcher,
    [string]$Assetc = "",
    [string]$CdnDir = "",
    [switch]$SkipVerify
)
$ErrorActionPreference = "Stop"
$launcherRoot = (Resolve-Path -LiteralPath $Launcher).Path
$packRoot = Join-Path $launcherRoot "pack"
$planSource = Join-Path $packRoot "bundle_plan.json"
if (-not (Test-Path -LiteralPath $planSource)) {
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
$mountCheck = Join-Path $PSScriptRoot "check-bundle-mounts.js"
if (-not (Test-Path -LiteralPath $mountCheck)) { throw "找不到 $mountCheck" }
$bootCheck = Join-Path $PSScriptRoot "check-boot-closure.js"
if (-not (Test-Path -LiteralPath $bootCheck)) { throw "找不到 $bootCheck" }

# 布局文件写到系统临时目录：**不能落在 launcher 根**（那会随包上传，且留下脏文件）。
$layoutFile = Join-Path $env:TEMP ("rustlua-bundle-layout-" + [guid]::NewGuid().ToString("N") + ".json")
try {
    $layoutJson = & $Assetc layout $packRoot --json
    if ($LASTEXITCODE -ne 0) {
        throw (("assetc layout 失败（exit {0}）：本机 assetc 可能太旧（需要含 layout 子命令的构建）；" +
            "重新跑 package.ps1（不要 -SkipBuild）或显式 -Assetc 指向新构建。") -f $LASTEXITCODE)
    }
    # 用 .NET 写，避免 PowerShell 5.1 的重定向编码/BOM 差异。
    [System.IO.File]::WriteAllText($layoutFile, ($layoutJson -join "`n"), (New-Object System.Text.UTF8Encoding($false)))

    & $node.Source $tool --file $layoutFile --apply $launcherRoot
    if ($LASTEXITCODE -ne 0) { throw "bundle-layout apply 失败（exit $LASTEXITCODE）" }
    # 计划表由 bundle-layout --apply 复制到 launcher 根（主包可读）：适配层挂载任何分包之前就要读它。
    if (-not $SkipVerify) {
        & $node.Source $tool --verify $launcherRoot
        if ($LASTEXITCODE -ne 0) { throw "bundle-layout verify 失败（exit $LASTEXITCODE）" }
    }
    # 以**计划表**为准再查一遍：每个根有目录、分包根有 game.js、声明的容器齐全。
    # `bundle-layout --verify` 只看 game.json 里写了的根，查不出"计划有、产物无"。
    & $node.Source $mountCheck --launcher $launcherRoot
    if ($LASTEXITCODE -ne 0) { throw "check-bundle-mounts 失败（exit $LASTEXITCODE）" }
    # 计划表的 boot 元数据必须与拆根后的真实目录一致；否则 lazy 只是"大概率安全"。
    & $node.Source $bootCheck --launcher $launcherRoot
    if ($LASTEXITCODE -ne 0) { throw "boot 闭包检查失败（exit $LASTEXITCODE）" }
    # 拆根之后容器换了目录、微信还会把 `.rlb` 改名成 `.rlb.txt`——**只有在这一刻**才能对账
    # "组里每条逻辑路径都还在它那个根的容器索引里"。`assetc closure` 是这条断言的唯一实现
    # （索引解析在 Rust 侧，不在这里再造一份）。
    #
    # `-CdnDir`（remote 形态）：`unit = "cdn"` 的组不在发布根里发布，而在 CDN 镜像目录里。
    # 不给它时 `assetc closure` 会把那些组判成"未检查"——打包步骤宁可判红也不静默跳过。
    $closureArgs = @("closure", $launcherRoot)
    if (-not [string]::IsNullOrWhiteSpace($CdnDir)) {
        $cdnRoot = (Resolve-Path -LiteralPath $CdnDir).Path
        $closureArgs += @("--cdn", $cdnRoot)
    }
    & $Assetc @closureArgs
    if ($LASTEXITCODE -ne 0) { throw "容器内容闭包检查失败（assetc closure，exit $LASTEXITCODE）" }
    Write-Host "apply_bundle_layout_ok launcher=$launcherRoot"
}
finally {
    if (Test-Path -LiteralPath $layoutFile) {
        Remove-Item -LiteralPath $layoutFile -Force -ErrorAction SilentlyContinue
    }
}
