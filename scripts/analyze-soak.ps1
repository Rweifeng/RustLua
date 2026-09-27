CmdletBinding()]
param(
    # soak.json 路径（`RUSTLUA_EDITOR_SOAK` 运行产出的归档文件）。
    [Parameter(Mandatory = $true)][string]$Path,
    # 把整段按时长切成多少个窗口（每段取谷值/中位/峰值）。
    [int]$Windows = 8
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $Path)) {
    throw "soak json not found: $Path"
}
$payload = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
$samples = @($payload.samples)
if ($samples.Count -lt 2) {
    throw "soak json has too few samples: $($samples.Count)"
}

$start = [double]$samples[0].seconds
$end = [double]$samples[$samples.Count - 1].seconds
$span = [Math]::Max($end - $start, 1.0)
$windowCount = [Math]::Min($Windows, $samples.Count)

Write-Host "soak: $Path"
Write-Host ("requested_seconds={0} complete={1} samples={2} frames={3} play_cycles={4}" -f `
        $payload.requested_seconds, $payload.complete, $samples.Count, $payload.frames, $payload.play_cycles)
Write-Host ""
Write-Host "window`tmin_mib`tp50_mib`tmax_mib`ttex_min_MiB`ttex_max_MiB`tsamples"

$troughs = @()
$p50s = @()
$peaks = @()
$amplitudes = @()
for ($index = 0; $index -lt $windowCount; $index++) {
    $low = $start + $span * $index / $windowCount
    $high = $start + $span * ($index + 1) / $windowCount
    $memory = @()
    $texture = @()
    foreach ($sample in $samples) {
        $seconds = [double]$sample.seconds
        $inside = if ($index + 1 -eq $windowCount) {
            $seconds -ge $low -and $seconds -le $high
        }
        else {
            $seconds -ge $low -and $seconds -lt $high
        }
        if ($inside) {
            $memory += [double]$sample.working_set_mib
            if ($null -ne $sample.egui_texture_bytes) {
                $texture += [double]$sample.egui_texture_bytes / 1MB
            }
        }
    }
    if ($memory.Count -eq 0) { continue }
    $sorted = $memory | Sort-Object
    $min = $sorted[0]
    $max = $sorted[$sorted.Count - 1]
    $p50 = $sorted[[int][Math]::Floor($sorted.Count / 2)]
    $texMin = if ($texture.Count -gt 0) { ($texture | Sort-Object)[0] } else { 0 }
    $texMax = if ($texture.Count -gt 0) { ($texture | Sort-Object)[-1] } else { 0 }
    $troughs += $min
    $p50s += $p50
    $peaks += $max
    $amplitudes += ($max - $min)
    Write-Host ("{0}`t{1:N1}`t{2:N1}`t{3:N1}`t{4:N1}`t{5:N1}`t{6}" -f `
            ($index + 1), $min, $p50, $max, $texMin, $texMax, $memory.Count)
}

$troughGrowth = $troughs[$troughs.Count - 1] - $troughs[0]
$meanGrowth = $p50s[$p50s.Count - 1] - $p50s[0]
$maxAmplitude = ($amplitudes | Measure-Object -Maximum).Maximum

Write-Host ""
Write-Host ("trough_first_mib={0:N1} trough_last_mib={1:N1} trough_growth_mib={2:N1}" -f `
        $troughs[0], $troughs[$troughs.Count - 1], $troughGrowth)
Write-Host ("p50_first_mib={0:N1} p50_last_mib={1:N1} p50_growth_mib={2:N1}" -f `
        $p50s[0], $p50s[$p50s.Count - 1], $meanGrowth)
Write-Host ("max_amplitude_mib={0:N1}" -f $maxAmplitude)
Write-Host ""
# **判据只看后半段**：开头必然有一次预热（加载工程 / 资源导入 / 字体图集），
# 拿它当基准会把"启动成本"误读成"持续增长"（第一版就是这么误报的：把 30 分钟那一档
# 的第一窗口谷值 195.7 MiB 当基准，得出 +54.9 MiB 的假阳性）。
$tailStart = [int][Math]::Floor($troughs.Count / 2)
$tailTroughs = $troughs[$tailStart..($troughs.Count - 1)]
$tailFirst = $tailTroughs[0]
$tailLast = $tailTroughs[$tailTroughs.Count - 1]
$tailGrowth = $tailLast - $tailFirst
$tailNoise = ($tailTroughs | Measure-Object -Maximum).Maximum - ($tailTroughs | Measure-Object -Minimum).Minimum
$threshold = [Math]::Max(0.15 * $tailFirst, 10.0)

Write-Host ("post_warmup_windows={0} tail_first_mib={1:N1} tail_last_mib={2:N1} tail_growth_mib={3:N1} tail_noise_mib={4:N1}" -f `
        $tailTroughs.Count, $tailFirst, $tailLast, $tailGrowth, $tailNoise)
Write-Host ""
# 判据：**预热之后**谷值是否持续抬升。锯齿/缓存涨落只让峰值与均值上下跳、谷值不动；
# 持续泄漏会让谷值抬升。阈值取 max(后半段谷值首项的 15%, 10 MiB)。
if ($tailGrowth -le $threshold) {
    Write-Host ("soak_verdict=no_sustained_growth post_warmup_trough_growth={0:N1}MiB threshold={1:N1}MiB" -f $tailGrowth, $threshold)
}
else {
    Write-Host ("soak_verdict=sustained_growth post_warmup_trough_growth={0:N1}MiB threshold={1:N1}MiB" -f $tailGrowth, $threshold)
    exit 2
}
