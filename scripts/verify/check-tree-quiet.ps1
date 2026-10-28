# check-tree-quiet.ps1 — 「读数是否可信」前置探针（不是代码红，是测量无效）
#
# 背景（2026-10-27 实测）：完整门单次要跑 25–45 分钟，而作者常在门跑的过程中继续保存。
# 结果是把**半保存态**编译成红：第 10 次尝试在 `draw.rs` 保存中途报
# `unexpected closing delimiter`，第 11/12 次在 clippy/资源门上死掉——日志读起来像回归，
# 实际是编辑器刚写完一半的文件。这类失败最贵的地方是它长得像真失败。
#
# 本探针把「树在动」与「代码坏」分开：起跑前若最近 N 秒内有源文件被写入，就退 **3 = blocked**
# （与 GPU/wasm 的 `exit 3` 同一语义：环境没准备好，绝不写成通过），而不是跑 40 分钟再假红。
# 它只测时间戳、毫秒级，所以放在所有门之前。
#
# CI 上整棵树是 `actions/checkout` 同一秒写出来的，必然"刚刚被改过"——那不是有人在编辑，
# 所以 `env:CI` 为真时显式跳过并打印原因（跳过 ≠ 通过，措辞里写清）。
#
# 退出码：0 = 静默（可以起跑）；1 = 探针自身失效（扫不到文件 = 路径/扩展名集合配错，
# 绝不能放行）；3 = 树在动，本次读数不可信。
#
# 本脚本由 verify.ps1 以 Windows PowerShell 5.1 调用，勿使用 pwsh 7 专属语法。

[CmdletBinding()]
param(
    [string]$RepoRoot,
    # 最近多少秒内有源文件写入就算"树在动"。
    [int]$QuietSeconds = 60,
    # 自测：用临时目录造三种反面对照（动 / 静 / 探针失效），断言退出码与输出。
    [switch]$SelfTest
)

$ErrorActionPreference = 'Stop'

# scripts/verify -> 仓库根
if (-not $RepoRoot) {
    $RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
}

# 只扫人写的源文件：target/ 与 artifacts/ 由构建自身写入，vendor/ 是解包出来的第三方源。
$scanRoots = @('engine\crates', 'examples', 'scripts', 'docs', '.github')
$extensions = @('*.rs', '*.toml', '*.json', '*.md', '*.ps1', '*.mjs', '*.js', '*.py', '*.yml',
    '*.lua', '*.scene', '*.prefab', '*.ui.json', '*.wgsl', '*.html', '*.css')

function Get-ScannedFiles([string]$Root) {
    $found = @()
    foreach ($relative in $scanRoots) {
        $path = Join-Path $Root $relative
        if (-not (Test-Path -LiteralPath $path)) { continue }
        $leaf = Split-Path -Leaf $relative
        $targetPrefix = (Join-Path $path 'target') + '\'
        $found += Get-ChildItem -LiteralPath $path -Recurse -File -Include $extensions -ErrorAction SilentlyContinue |
            Where-Object {
                -not $_.FullName.StartsWith($targetPrefix, [StringComparison]::OrdinalIgnoreCase) -and
                -not $_.FullName.Contains('\artifacts\')
            }
    }
    return @($found)
}

# 自测要以**子进程**跑本脚本：Write-Host 写的是 host 流，同进程 `&` 调用拿不到文本，
# 断言会假红（10-27 实测：三例退出码全对，文本断言全空）。
function Invoke-Probe([string]$Root) {
    $text = (& powershell -NoProfile -ExecutionPolicy Bypass -File $PSCommandPath -RepoRoot $Root -QuietSeconds 60 2>&1 |
        ForEach-Object { $_.ToString() }) -join "`n"
    return [pscustomobject]@{ ExitCode = $LASTEXITCODE; Text = $text }
}

# ── 自测：反面对照必须真的让探针变红，否则探针只是个装饰 ──────────────────────
if ($SelfTest) {
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("rustlua-tree-quiet-selftest-" + [guid]::NewGuid().ToString('N'))
    $cases = 0
    $failures = @()
    try {
        # 1) 探针失效：根目录里一个源文件都没有 → 必须退 1（不能放行）。
        $emptyRoot = Join-Path $tmp 'empty'
        New-Item -ItemType Directory -Path (Join-Path $emptyRoot 'engine\crates') -Force | Out-Null
        $emptyOut = Invoke-Probe -Root $emptyRoot
        $code = $emptyOut.ExitCode
        if ($code -ne 1) { $failures += "探针失效（空树）应退 1，实际 $code" }
        elseif ($emptyOut.Text -notmatch 'tree_quiet_probe_failed') { $failures += '探针失效却没打印 tree_quiet_probe_failed' }
        else { $cases++ }

        # 2) 树在动：刚写一个 .rs → 必须退 3，且打印 busy。
        $busyRoot = Join-Path $tmp 'busy'
        New-Item -ItemType Directory -Path (Join-Path $busyRoot 'engine\crates') -Force | Out-Null
        [IO.File]::WriteAllText((Join-Path $busyRoot 'engine\crates\hot.rs'), "// written now`n")
        $busyOut = Invoke-Probe -Root $busyRoot
        $code = $busyOut.ExitCode
        if ($code -ne 3) { $failures += "树在动应退 3，实际 $code（输出：$($busyOut.Text -join ' | ')）" }
        elseif ($busyOut.Text -notmatch 'tree_busy') { $failures += '树在动却没打印 tree_busy' }
        else { $cases++ }

        # 3) 树静默：把同一个文件的时间戳拨回 10 分钟前 → 必须退 0。
        $quietRoot = Join-Path $tmp 'quiet'
        New-Item -ItemType Directory -Path (Join-Path $quietRoot 'engine\crates') -Force | Out-Null
        $cold = Join-Path $quietRoot 'engine\crates\cold.rs'
        [IO.File]::WriteAllText($cold, "// long untouched`n")
        (Get-Item -LiteralPath $cold).LastWriteTime = (Get-Date).AddMinutes(-10)
        $quietOut = Invoke-Probe -Root $quietRoot
        $code = $quietOut.ExitCode
        if ($code -ne 0) { $failures += "树静默应退 0，实际 $code（输出：$($quietOut.Text -join ' | ')）" }
        elseif ($quietOut.Text -notmatch 'tree_quiet_ok') { $failures += '静默却没打印 tree_quiet_ok' }
        else { $cases++ }
    }
    finally {
        if (Test-Path -LiteralPath $tmp) { Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue }
    }
    if ($failures.Count -gt 0) {
        foreach ($f in $failures) { Write-Host "[FAIL] $f" -ForegroundColor Red }
        Write-Host 'tree_quiet_selftest_failed' -ForegroundColor Red
        exit 1
    }
    Write-Host "tree_quiet_selftest_ok cases=$cases（空树=1 / 在动=3 / 静默=0 三种对照全过）"
    exit 0
}

if ($QuietSeconds -le 0) {
    Write-Host "tree_quiet_skipped reason=disabled (QuietSeconds=$QuietSeconds)" -ForegroundColor Yellow
    exit 0
}

# CI：checkout 把整棵树写在同一秒，"刚刚被改过"是常态而不是有人在编辑。
$ciEnv = $env:CI
if ($ciEnv -eq 'true' -or $ciEnv -eq '1') {
    Write-Host 'tree_quiet_skipped reason=ci_fresh_checkout (CI 的 mtime 全是 checkout 时刻，不是编辑)' -ForegroundColor Yellow
    exit 0
}

$files = @(Get-ScannedFiles $RepoRoot)
if ($files.Count -eq 0) {
    Write-Host "tree_quiet_probe_failed reason=no_files_rooted_at path=$RepoRoot" -ForegroundColor Red
    Write-Host '  扫不到任何源文件 = 扫描根/扩展名配错。探针失效不放行，判红。' -ForegroundColor Red
    exit 1
}

$now = Get-Date
$threshold = $now.AddSeconds(-$QuietSeconds)
# LastWriteTime 用本地时间，与 Get-Date 同基准；只比大小，不做时区换算。
$recent = @($files | Where-Object { $_.LastWriteTime -gt $threshold })
$newestAge = [Math]::Round((($now - (($files | Measure-Object -Property LastWriteTime -Maximum).Maximum)).TotalSeconds), 0)

if ($recent.Count -eq 0) {
    Write-Host ("tree_quiet_ok files={0} newest_age_s={1} threshold={2}" -f $files.Count, $newestAge, $QuietSeconds)
    exit 0
}

Write-Host ("tree_busy files_edited_last_{0}s={1} newest_age_s={2} scanned={3}" -f $QuietSeconds, $recent.Count, $newestAge, $files.Count) -ForegroundColor Yellow
foreach ($item in ($recent | Sort-Object LastWriteTime -Descending | Select-Object -First 10)) {
    Write-Host ("  {0}  ({1:HH:mm:ss})" -f $item.FullName, $item.LastWriteTime) -ForegroundColor Yellow
}
Write-Host '  这不是代码红：文件可能只写了一半，任何编译/测试读数都不可信。等保存停手后重跑。' -ForegroundColor Yellow
exit 3
