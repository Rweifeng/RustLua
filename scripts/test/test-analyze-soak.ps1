# soak 判据脚本（scripts/lib/analyze-soak.ps1）的自测。
#
# 来历：M27-03 验收时第一次有人运行它，发现它在 HEAD 上就是坏的——首行 `[CmdletBinding()]`
# 丢了前括号（PowerShell 解析阶段直接退出 1），修好之后又倒在第二步（`Get-Content -Raw` 没写
# `-Encoding UTF8`，soak.json 里的中文面板标题被按 ANSI 解码，中文尾字节落成 0x5C 就成了非法
# JSON 转义）。docs/55 阶段 0 立下的 soak 判据因此**从来没有可执行过**。
# 判据工具坏着可以一直"看起来存在"，正因为它的调用方是人在验收当天手点，不在质量门里。
# 这里把它的四种判读结果固化成门：稳态放行、持续增长判红、锯齿不误报、坏输入不静默。
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$analyzer = Join-Path $PSScriptRoot "..\lib\analyze-soak.ps1"
$temp = Join-Path $root "artifacts\tests\soak-analyze"
New-Item -ItemType Directory -Force -Path $temp | Out-Null

# 写成**不带 BOM** 的 UTF-8：编辑器侧 soak.json 就是这个字节布局（Rust 直写），
# 带 BOM 反而测不到"中文被按 ANSI 解码"这一类伤。
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)

function New-SoakJson {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][scriptblock]$Memory,
        [int]$Samples = 120
    )
    $items = @()
    for ($index = 0; $index -lt $Samples; $index++) {
        $mib = & $Memory $index
        $items += ('{{"seconds":{0},"working_set_mib":{1:N3},"egui_texture_bytes":1048576,"entities":22,"churn_rounds":{2}}}' -f `
                (($index + 1) * 10.0), $mib, $index)
    }
    $json = ('{{"report_kind":"docs55-phase0-soak","requested_seconds":{0},"complete":true,"frames":43478,' -f ($Samples * 10))
    $json += '"play_cycles":120,"growth_mib":18.7,'
    # 中文面板标题：编码回归就埋在这里。
    $json += '"churn":{"rounds":40,"dock_churns":40,"drop_imports":40,"drop_cleanups":39,"skipped":0,'
    $json += '"recent_notes":["属性并入Scene标签页","层级拆成独立区域","导入 hero.png：已创建 1 个实体"]},'
    $json += '"samples":[' + ($items -join ",") + "]}"
    [System.IO.File]::WriteAllText($Path, $json, $utf8NoBom)
}

function Invoke-Analyzer {
    param([Parameter(Mandatory = $true)][string]$Path)
    $outFile = Join-Path $temp "analyzer-out.txt"
    $errFile = Join-Path $temp "analyzer-err.txt"
    Remove-Item -LiteralPath $outFile, $errFile -Force -ErrorAction SilentlyContinue
    $process = Start-Process -FilePath "powershell" `
        -ArgumentList @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $analyzer, "-Path", $Path, "-Windows", "8") `
        -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput $outFile -RedirectStandardError $errFile
    $text = ""
    if (Test-Path -LiteralPath $outFile) { $text += (Get-Content -LiteralPath $outFile -Raw -Encoding UTF8) }
    if (Test-Path -LiteralPath $errFile) { $text += (Get-Content -LiteralPath $errFile -Raw -Encoding UTF8) }
    return @{ Code = $process.ExitCode; Text = $text }
}

# ① 稳态（谷值不动 + 小幅锯齿 + 中文负载说明）必须放行，且判定行可读。
#    谷值取 460 + 8*sin(i)：窗口极小值由 sin 的采样点决定，但整段的**后半段谷值不随时间抬升**，
#    这正是 soak 判据只看谷值的原因（预热台阶与缓存涨落都不该算增长）。
$plateauPath = Join-Path $temp "plateau.json"
New-SoakJson -Path $plateauPath -Memory { param($i) 460.0 + 8.0 * [Math]::Sin($i) }
$plateau = Invoke-Analyzer -Path $plateauPath
if ($plateau.Code -ne 0) { throw "plateau soak must pass, got exit $($plateau.Code):`n$($plateau.Text)" }
if ($plateau.Text -notmatch "soak_verdict=no_sustained_growth") {
    throw "plateau soak must yield no_sustained_growth, got:`n$($plateau.Text)"
}
if ($plateau.Text -notmatch "samples=120") {
    throw "analyze-soak.ps1 must decode the Chinese-bearing payload (samples=120), got:`n$($plateau.Text)"
}

# ② 真泄漏（每个采样点比上一个高 4 MiB）必须判红并退出 2——否则这条门是空的。
$leakPath = Join-Path $temp "leak.json"
New-SoakJson -Path $leakPath -Memory { param($i) 300.0 + $i * 4.0 }
$leak = Invoke-Analyzer -Path $leakPath
if ($leak.Code -ne 2) { throw "monotonic leak must exit 2, got $($leak.Code):`n$($leak.Text)" }
if ($leak.Text -notmatch "soak_verdict=sustained_growth") {
    throw "monotonic leak must say sustained_growth, got:`n$($leak.Text)"
}

# ③ 大幅锯齿不算增长：谷值钉在 460，峰值一路到 496。判据若看峰值/均值就会误报，
#    而"看错了要判什么"恰恰是第一版 soak 判定误报过的那一格（docs/55 阶段 0 的教训）。
$sawPath = Join-Path $temp "sawtooth.json"
New-SoakJson -Path $sawPath -Memory { param($i) 460.0 + ($i % 7) * 6.0 }
$saw = Invoke-Analyzer -Path $sawPath
if ($saw.Code -ne 0) { throw "constant trough with large swings must pass, got $($saw.Code):`n$($saw.Text)" }
if ($saw.Text -notmatch "soak_verdict=no_sustained_growth") {
    throw "sawtooth must not be read as growth, got:`n$($saw.Text)"
}

# ④ 坏输入不静默放行：采样点不足与文件不存在都必须非零退出。
$tinyPath = Join-Path $temp "too-few.json"
New-SoakJson -Path $tinyPath -Memory { param($i) 460.0 } -Samples 1
$tiny = Invoke-Analyzer -Path $tinyPath
if ($tiny.Code -eq 0) { throw "a soak with one sample must not pass" }
$missing = Invoke-Analyzer -Path (Join-Path $temp "does-not-exist.json")
if ($missing.Code -eq 0) { throw "a missing soak json must not pass" }

Write-Host "soak_analyze_tests_ok cases=5" -ForegroundColor Green
