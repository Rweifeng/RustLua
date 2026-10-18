[CmdletBinding()]
param(
    # 目录（按文件名配对）或单个报告文件。
    [Parameter(Mandatory = $true)][string]$Current,
    [Parameter(Mandatory = $true)][string]$Previous,
    # 允许不同的**叶子键**（正则，匹配完整路径）。默认只放行"计时与运行噪声"：
    # 时间是机器/负载相关的量（baselines/README.md 已裁定跨机不可比），分配**计数**在同一份
    # 二进制上重跑也会抖（相机拆分批 6 实测 24570/24572/24575）；分配**峰值字节**不在此列，
    # 它是 docs/54 §7 的红线，必须逐值相同，由 compare-baseline.ps1 -GeometryOnly 另判。
    [string[]]$IgnoreKeyPattern = @(
        '\.(count|min|max|average|p50|p95|p99|min_us|max_us|average_us|p50_us|p95_us|p99_us|min_ns|max_ns|average_ns|p50_ns|p95_ns|p99_ns)$',
        '_us$', '_ns$', '_ms$',
        'alloc_count', 'alloc_bytes',
        # 时间占比（分子分母都是时间：`spatial_share_pct` 一类）——同机重跑的抖动就在这几个点。
        'share_pct$',
        'fps$'
    ),
    # 打印全部被放行的差异（默认只打印前 10 条）。
    [switch]$ShowIgnored
)

# 语义/像素判据：两份报告必须**逐值相同**，差异只允许落在上面那份噪声白名单里。
# 为什么要有这个脚本（而不是每次临时写一段）：相机拆分批 6 的"721 项字段全部相同"结论当时
# 没有任何可复跑的实现，谁都没法复核"721"是怎么数出来的。这条把它变成可执行的一行命令：
#
#   powershell -NoProfile -File scripts/verify/compare-golden.ps1 `
#       -Current artifacts/camera-viewport-after -Previous artifacts/camera-viewport-before
#
# 内存**字节**计数字段（`cpu_live_bytes` / `cpu_peak_bytes`）**刻意不在白名单里**：它们是 docs/54 §7
# 的红线，必须看见而不是被静默放行。但它们是"进程级 live 字节在某窗口内的读数"，窗口里任何线程
# （含 wgpu 驱动线程）的分配都会落进去，因此同机同二进制重跑也会抖（相机拆分批 7 实测 n=100 档
# 在 517696/551386/534849 之间跳，±3%）。判定这件事归 `compare-baseline.ps1 -GeometryOnly`
# 的 ±10% 阈值；本脚本只保证"这条差异被打印出来、必须由人给出解释"。
$ErrorActionPreference = "Stop"
$invariant = [System.Globalization.CultureInfo]::InvariantCulture

function Get-LeafMap {
    param($Node, [string]$Prefix, [hashtable]$Out)
    if ($null -eq $Node) {
        $Out[$Prefix] = "<null>"
        return
    }
    if ($Node -is [System.Management.Automation.PSCustomObject]) {
        $props = @($Node.PSObject.Properties)
        if ($props.Count -eq 0) {
            $Out[$Prefix] = "{}"
            return
        }
        foreach ($property in $props) {
            Get-LeafMap -Node $property.Value -Prefix "$Prefix.$($property.Name)" -Out $Out
        }
        return
    }
    if ($Node -is [System.Collections.IEnumerable] -and -not ($Node -is [string])) {
        $items = @($Node)
        if ($items.Count -eq 0) {
            $Out[$Prefix] = "[]"
            return
        }
        for ($index = 0; $index -lt $items.Count; $index++) {
            Get-LeafMap -Node $items[$index] -Prefix "$Prefix[$index]" -Out $Out
        }
        return
    }
    if ($Node -is [double] -or $Node -is [single] -or $Node -is [decimal]) {
        # 逐位比较：格式化成字符串会把 0.1 与 0.10000000000000001 混为一谈，而那正是
        # "矩阵按哪份比例算的"这类回归会留下的痕迹。
        $Out[$Prefix] = ([double]$Node).ToString("R", $invariant)
        return
    }
    $Out[$Prefix] = [string]$Node
}

function Test-FilePair {
    param([string]$CurrentPath, [string]$PreviousPath)
    if (-not (Test-Path -LiteralPath $CurrentPath)) { throw "Current report missing: $CurrentPath" }
    if (-not (Test-Path -LiteralPath $PreviousPath)) { throw "Previous report missing: $PreviousPath" }
    $currentLeaves = @{}
    $previousLeaves = @{}
    Get-LeafMap -Node (Get-Content -LiteralPath $CurrentPath -Raw | ConvertFrom-Json) -Prefix "root" -Out $currentLeaves
    Get-LeafMap -Node (Get-Content -LiteralPath $PreviousPath -Raw | ConvertFrom-Json) -Prefix "root" -Out $previousLeaves

    $differences = New-Object System.Collections.ArrayList
    $ignored = New-Object System.Collections.ArrayList
    $keys = @($currentLeaves.Keys + $previousLeaves.Keys | Sort-Object -Unique)
    foreach ($key in $keys) {
        $currentValue = if ($currentLeaves.ContainsKey($key)) { $currentLeaves[$key] } else { "<missing>" }
        $previousValue = if ($previousLeaves.ContainsKey($key)) { $previousLeaves[$key] } else { "<missing>" }
        if ($currentValue -ceq $previousValue) { continue }
        $isIgnored = $false
        foreach ($pattern in $IgnoreKeyPattern) {
            if ($key -match $pattern) { $isIgnored = $true; break }
        }
        $entry = "{0}: {1} -> {2}" -f $key, $previousValue, $currentValue
        if ($isIgnored) { [void]$ignored.Add($entry) } else { [void]$differences.Add($entry) }
    }
    return [pscustomobject]@{
        Compared  = $keys.Count
        Diffs     = $differences
        Ignored   = $ignored
    }
}

$pairs = New-Object System.Collections.ArrayList
if ((Test-Path -LiteralPath $Current -PathType Container) -or (Test-Path -LiteralPath $Previous -PathType Container)) {
    if (-not ((Test-Path -LiteralPath $Current -PathType Container) -and (Test-Path -LiteralPath $Previous -PathType Container))) {
        throw "Both paths must be directories when either one is a directory"
    }
    foreach ($file in Get-ChildItem -LiteralPath $Current -Filter *.json | Sort-Object Name) {
        [void]$pairs.Add(@{
            Name     = $file.Name
            Current  = $file.FullName
            Previous = Join-Path $Previous $file.Name
        })
    }
    if ($pairs.Count -eq 0) { throw "No *.json reports found in $Current" }
}
else {
    [void]$pairs.Add(@{
        Name     = (Split-Path -Leaf $Current)
        Current  = (Resolve-Path -LiteralPath $Current).Path
        Previous = (Resolve-Path -LiteralPath $Previous).Path
    })
}

$totalCompared = 0
$totalIgnored = 0
$totalDiffs = 0
foreach ($pair in $pairs) {
    $result = Test-FilePair -CurrentPath $pair.Current -PreviousPath $pair.Previous
    $totalCompared += $result.Compared
    $totalIgnored += $result.Ignored.Count
    $totalDiffs += $result.Diffs.Count
    $status = if ($result.Diffs.Count -eq 0) { "ok" } else { "DIFF" }
    Write-Host ("{0,-32} compared={1,-6} semantic_diffs={2,-4} ignored_noise={3,-4} {4}" -f `
        $pair.Name, $result.Compared, $result.Diffs.Count, $result.Ignored.Count, $status)
    foreach ($line in $result.Diffs) { Write-Host "    DIFF  $line" }
    if ($ShowIgnored) {
        foreach ($line in $result.Ignored) { Write-Host "    noise $line" }
    }
}
Write-Host ("golden_compare_ok files={0} compared={1} semantic_diffs={2} ignored_noise={3}" -f `
    $pairs.Count, $totalCompared, $totalDiffs, $totalIgnored)

if ($totalDiffs -gt 0) {
    throw "golden semantic differences found: $totalDiffs (see DIFF lines above)"
}
