<#
.SYNOPSIS
    《江湖有田》阶段 5 全量 QA headless 脚本（docs/54 §阶段5 「全量 QA」独立落地件）。

.DESCRIPTION
    只读、独立运行：不修改 verify.ps1 / main.lua / validate_data.lua / 任何 Lua 数据与系统文件。
    覆盖三块：
      1) 存档 QA —— 从 scripts/main.lua 的 register_save_providers() 真实枚举 provider，
         再对 verify.ps1 $examples 清单（只读镜像快照）逐样例静态分类：
         磁盘级 save→continue 往返 / 进程内 capture·restore 单元级 / 无存档断言。
      2) 性能 QA —— 按同一清单逐项 `cargo run --quiet -p game --example <name>` 计时，
         输出最慢 Top N、总量、单样例/总量阈值（通过/警告）。
      3) 多分辨率 QA —— 静态断言 design resolution 三方一致（ui/layout.lua 的
         DESIGN_W/H、project.toml 的 window_width/height、scenes/*.scene 的
         ui_reference_*）。引擎 headless 不支持分辨率参数（已查证：jhyt_* 样例只读
         env::args().nth(1)=工程路径；--width/--height 仅存在于引擎基准样例
         tilemap_stress.rs），故 headless 无法跑真实分辨率循环，运行时适配由
         ui/layout_check.lua 在 `game --check` 守门，本脚本只守常量漂移。

.PARAMETER SkipPerf
    跳过 cargo 计时（只跑存档/多分辨率静态 QA）。冷全量编译较慢，静态自检用。

.PARAMETER PerfTopN
    性能报告最慢样例展示条数（默认 10）。

.PARAMETER ExampleWarnSec
    单样例耗时警告阈值（秒，默认 60）。

.PARAMETER TotalWarnSec
    全量样例总耗时警告阈值（秒，默认 600）。

.NOTES
    存档 provider 枚举基线（2026-10-06 实测 main.lua）：
      30 个 save_manager.register + 1 个 audio.register_save() = 31 个 provider；
      jhyt_m55_solarterm（节气历法）为设计上纯派生、显式不入档（capture/restore/reset
      空实现），本脚本把它列为「有意无存档断言」而非缺口。

    【未落地的新档冒烟补丁（按任务边界不新建 .rs 样例——新建会触发 verify.ps1
    $unlisted 清单门变红）】「新档→睡觉自动存→继续→读档一致」链路已由既有样例
    jhyt_m19_slice §8-9 覆盖（睡觉 day.sleep 自动存档 → save_manager.last_summary()
    断言 → 第二个 runtime → BtnContinue → 跨 runtime 一致）。如需独立最小冒烟，
    可新增 engine/crates/game/examples/jhyt_qa_new_game_smoke.rs 并登记进
    verify.ps1 $examples（本任务不触碰这两处）。
#>
[CmdletBinding()]
param(
    [switch]$SkipPerf,
    [int]$PerfTopN = 10,
    [double]$ExampleWarnSec = 60.0,
    [double]$TotalWarnSec = 600.0
)

$ErrorActionPreference = "Stop"

$repoRoot   = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$engineRoot = Join-Path $repoRoot "engine"
$gameRoot   = Join-Path $repoRoot "examples\game_jhyt"
$exampleDir = Join-Path $engineRoot "crates\game\examples"

# ── 清单（verify.ps1 $examples 的只读镜像快照。唯一事实来源仍是 verify.ps1；此处漂移 = 静态告警。
#    定位用锚点而不是行号——2026-10-06 那份注释写的是「行 797–915」，2026-10-27 实测清单已在
#    `$examples = @(`（现 1441 行）之后：verify.ps1 只长不减，行号必然腐烂，锚点不会。）──────
$examples = @(
    "jhyt_smoke",
    "jhyt_m17",
    "jhyt_m17_npc",
    "jhyt_m17_dialogue",
    "jhyt_m17_shop",
    "jhyt_m18_combat",
    "jhyt_m18_stats",
    "jhyt_m18_build",
    "jhyt_m19_slice",
    "jhyt_m19_soak",
    "jhyt_m19_ui",
    "jhyt_m20_audio",
    "jhyt_m20_gamepad",
    "jhyt_m21_appearance",
    "jhyt_m22_input",
    "jhyt_m22_save_isolation",
    "jhyt_m23_data_validation",
    "jhyt_m24_numeric_alignment",
    # 阶段1（改造计划 M36–M43）：四季作物矩阵与生产闭环门（verify.ps1 行 816–826 补登）。
    "jhyt_m36_regrow",
    "jhyt_m36_economy_sim",
    "jhyt_m37_tool_upgrade",
    "jhyt_m38_fertilizer_quality",
    "jhyt_m39_sprinkler",
    "jhyt_m40_fruit_tree",
    "jhyt_m41_greenhouse",
    "jhyt_m42_shipping_box",
    "jhyt_m43_mastery",
    "jhyt_m25_combat_feedback",
    "jhyt_m26_hud_feedback",
    "jhyt_m27_goals",
    "jhyt_m28_loop",
    "jhyt_m29_season_weather",
    "jhyt_m30_cjk_text",
    "jhyt_m31_pc_release",
    "jhyt_m32_camera",
    "jhyt_m34_save_gates",
    "jhyt_m34_relationship_tier",
    "jhyt_m34_festival",
    "jhyt_m34_status",
    "jhyt_m34_skills",
    "jhyt_m34_teahouse",
    "jhyt_m35_depth_sort",
    "jhyt_m59_ui_prefab",
    "jhyt_m44_animal",
    "jhyt_m45_fishing",
    "jhyt_m46_mine",
    "jhyt_m47_cooking",
    "jhyt_m48_npc",
    "jhyt_m49_festival",
    "jhyt_m50_questboard",
    "jhyt_m51_codex",
    "jhyt_m52_achievement",
    "jhyt_m53_meridian",
    "jhyt_m54_clinic",
    "jhyt_m55_solarterm",
    "jhyt_m56_blackmarket",
    "jhyt_m57_teaparty",
    "jhyt_m58_ending"
)

# 基线（2026-10-06 实测）：provider 总数 / 磁盘往返样例数 / 进程内 capture·restore 样例数。
# 低于基线 = 能力回退，判红；新增样例后把基线上调。
$baselineProviders      = 31
$baselineDiskRoundtrip  = 22
$baselineCaptureRestore = 16

$script:hardFails  = @()
$script:warnings   = @()

function Write-Section([string]$title) {
    Write-Host "`n===== $title =====" -ForegroundColor Cyan
}
function Fail([string]$msg)  { $script:hardFails += $msg; Write-Host "  [FAIL] $msg" -ForegroundColor Red }
function Warn2([string]$msg) { $script:warnings  += $msg; Write-Host "  [WARN] $msg" -ForegroundColor Yellow }

# ══ 1. 存档 QA ══════════════════════════════════════════════════════════════
Write-Section "1. 存档 QA（provider 枚举 + 样例往返覆盖汇总）"

$mainLua = Join-Path $gameRoot "scripts\main.lua"
if (-not (Test-Path -LiteralPath $mainLua)) { Fail "找不到 $mainLua"; }
else {
    $mainText = Get-Content -Raw -Encoding UTF8 $mainLua
    $registered = [regex]::Matches($mainText, 'save_manager\.register\("([^"]+)"') |
        ForEach-Object { $_.Groups[1].Value }
    $hasAudio = [bool]($mainText -match 'audio\.register_save\(\)')
    $providerCount = $registered.Count + ($(if ($hasAudio) { 1 } else { 0 }))

    Write-Host ("  已注册 provider（save_manager.register）: " + $registered.Count + " 个")
    Write-Host ("    " + ($registered -join ", "))
    Write-Host ("  audio.register_save()  : " + $(if ($hasAudio) { "是（第 " + ($registered.Count + 1) + " 个 provider）" } else { "否" }))
    Write-Host ("  provider 合计           : $providerCount")

    if ($providerCount -lt $baselineProviders) {
        Fail "provider 数 $providerCount < 基线 $baselineProviders（main.lua 注册中心回退？）"
    }
    if ($registered -contains "solarterm") {
        Fail "solarterm 被注册进存档——它是 day 纯派生系统，设计上 capture/restore/reset 空实现、不入档（docs/33 §2.1）"
    }
}

# 样例清单存在性 + 磁盘镜像一致性
$missingFiles = @()
foreach ($e in $examples) {
    if (-not (Test-Path -LiteralPath (Join-Path $exampleDir ($e + ".rs")))) { $missingFiles += $e }
}
if ($missingFiles.Count -gt 0) { Fail "清单样例缺源文件: $($missingFiles -join ', ')" }
$onDisk = @(Get-ChildItem -LiteralPath $exampleDir -Filter "jhyt_*.rs" -File | ForEach-Object { $_.BaseName })
$unlisted = @($onDisk | Where-Object { $examples -notcontains $_ })
if ($unlisted.Count -gt 0) {
    Warn2 ("磁盘上存在未登记进本镜像的 jhyt_* 样例（verify.ps1 才是严格清单门）: " + ($unlisted -join ', '))
}

# 逐样例静态分类（真实信号，不臆测）：
#   磁盘级 = 出现 BtnContinue（标题→继续档）或 save_manager').load()（读盘）——
#            样例能继续档必然自己先落了盘（含睡觉 day.sleep 自动存档，如 m19_slice）；
#   进程内 = 直接调 .capture() 与 .restore( 做内存表往返、不落盘；
#   其余   = 无存档断言。
$disk = @(); $cap = @(); $none = @()
foreach ($e in $examples) {
    $f = Join-Path $exampleDir ($e + ".rs")
    if (-not (Test-Path -LiteralPath $f)) { continue }
    $t = Get-Content -Raw -Encoding UTF8 $f
    $hasContinue = ($t -match 'BtnContinue') -or ($t -match "save_manager'\)\.load\(\)")
    $hasCap      = $t -match '\.capture\(\)'
    $hasRestore  = $t -match '\.restore\('
    if     ($hasContinue)            { $disk += $e }
    elseif ($hasCap -and $hasRestore) { $cap  += $e }
    else                              { $none += $e }
}

Write-Host "`n  [磁盘级 save→continue 往返] $($disk.Count) 个："
Write-Host ("    " + ($disk -join ", "))
Write-Host "  [进程内 capture·restore 单元级] $($cap.Count) 个："
Write-Host ("    " + ($cap -join ", "))
Write-Host "  [无存档断言] $($none.Count) 个："
Write-Host ("    " + ($none -join ", "))

if ($disk.Count -lt $baselineDiskRoundtrip) {
    Fail "磁盘往返样例 $($disk.Count) < 基线 $baselineDiskRoundtrip（存档链路覆盖回退？）"
}
if ($cap.Count -lt $baselineCaptureRestore) {
    Fail "进程内 capture·restore 样例 $($cap.Count) < 基线 $baselineCaptureRestore（provider 单元覆盖回退？）"
}
if ($none -notcontains "jhyt_m55_solarterm") {
    Warn2 "jhyt_m55_solarterm 不再位于「无存档断言」组——它是设计上纯派生不入档系统，确认这是有意变更。"
}

# ══ 2. 性能 QA ══════════════════════════════════════════════════════════════
if ($SkipPerf) {
    Write-Section "2. 性能 QA（-SkipPerf 跳过）"
}
else {
    Write-Section "2. 性能 QA（逐项 cargo run 计时）"
    $results = @()
    Push-Location $engineRoot
    try {
        foreach ($e in $examples) {
            $sw = [System.Diagnostics.Stopwatch]::StartNew()
            & cargo run --quiet -p game --example $e 2>&1 | Out-Null
            $code = $LASTEXITCODE
            $sw.Stop()
            $sec = [math]::Round($sw.Elapsed.TotalSeconds, 2)
            $results += [pscustomobject]@{ Example = $e; Sec = $sec; Exit = $code }
            $tag = if ($code -ne 0) { "FAIL" } elseif ($sec -gt $ExampleWarnSec) { "slow" } else { "ok" }
            Write-Host ("  {0,-28} {1,8:N2}s  exit={2}  [{3}]" -f $e, $sec, $code, $tag)
            if ($code -ne 0) { Fail "样例 $e 运行失败（exit=$code）" }
        }
    }
    finally { Pop-Location }

    $totalSec = [math]::Round(($results | Measure-Object -Property Sec -Sum).Sum, 2)
    Write-Host "`n  -- 最慢 Top $PerfTopN --"
    $results | Sort-Object Sec -Descending | Select-Object -First $PerfTopN | ForEach-Object {
        Write-Host ("    {0,-28} {1,8:N2}s" -f $_.Example, $_.Sec)
    }
    Write-Host ("  全量样例合计: $totalSec s（共 $($results.Count) 个）")
    $slowOnes = @($results | Where-Object { $_.Sec -gt $ExampleWarnSec -and $_.Exit -eq 0 })
    if ($slowOnes.Count -gt 0) {
        Warn2 ("超过单样例阈值 ${ExampleWarnSec}s 的样例 " + $slowOnes.Count + " 个: " + (($slowOnes | ForEach-Object { $_.Example }) -join ', '))
    }
    if ($totalSec -gt $TotalWarnSec) {
        Warn2 ("全量耗时 $totalSec s 超过总量阈值 ${TotalWarnSec}s")
    }
}

# ══ 3. 多分辨率 QA（静态常量断言；headless 无分辨率参数）════════════════════
Write-Section "3. 多分辨率 QA（design resolution 三方一致性静态断言）"
Write-Host "  说明：引擎 headless 样例只接收工程路径参数（env::args().nth(1)），"
Write-Host "  无 --width/--height（该参数仅存在于引擎基准样例 tilemap_stress.rs）；"
Write-Host "  故 headless 无法跑 1280x720 / 1920x1080 / 2560x1440 真实分辨率循环。"
Write-Host "  运行时版面适配由 ui/layout_check.lua 在 game --check 守门；本脚本守常量漂移。"

# 3a. ui/layout.lua DESIGN_W / DESIGN_H
$layoutLua = Join-Path $gameRoot "scripts\ui\layout.lua"
$wantW = 1280; $wantH = 720
if (-not (Test-Path -LiteralPath $layoutLua)) {
    Fail "找不到 $layoutLua"
} else {
    $lt = Get-Content -Raw -Encoding UTF8 $layoutLua
    $mw = [regex]::Match($lt, 'M\.DESIGN_W\s*=\s*(\d+)')
    $mh = [regex]::Match($lt, 'M\.DESIGN_H\s*=\s*(\d+)')
    $lw = if ($mw.Success) { [int]$mw.Groups[1].Value } else { -1 }
    $lh = if ($mh.Success) { [int]$mh.Groups[1].Value } else { -1 }
    Write-Host "  ui/layout.lua  DESIGN_W=$lw  DESIGN_H=$lh（期望 $wantW x $wantH）"
    if ($lw -ne $wantW) { Fail "ui/layout.lua DESIGN_W=$lw != $wantW" }
    if ($lh -ne $wantH) { Fail "ui/layout.lua DESIGN_H=$lh != $wantH" }
}

# 3b. project.toml window_width / window_height
$projToml = Join-Path $gameRoot "project.toml"
if (-not (Test-Path -LiteralPath $projToml)) {
    Fail "找不到 $projToml"
} else {
    $pt = Get-Content -Raw -Encoding UTF8 $projToml
    $tw = [regex]::Match($pt, 'window_width\s*=\s*(\d+)')
    $th = [regex]::Match($pt, 'window_height\s*=\s*(\d+)')
    $pw = if ($tw.Success) { [int]$tw.Groups[1].Value } else { -1 }
    $ph = if ($th.Success) { [int]$th.Groups[1].Value } else { -1 }
    Write-Host "  project.toml   window_width=$pw  window_height=$ph（期望 $wantW x $wantH）"
    if ($pw -ne $wantW) { Fail "project.toml window_width=$pw != $wantW" }
    if ($ph -ne $wantH) { Fail "project.toml window_height=$ph != $wantH" }
}

# 3c. scenes/*.scene 的 meta ui_reference_width/height
$sceneDir = Join-Path $gameRoot "scenes"
$badScenes = @(); $sceneCount = 0
if (-not (Test-Path -LiteralPath $sceneDir)) {
    Fail "找不到场景目录 $sceneDir"
} else {
    foreach ($scn in (Get-ChildItem -LiteralPath $sceneDir -Filter "*.scene" -File)) {
        $sceneCount++
        $st = Get-Content -Raw -Encoding UTF8 $scn.FullName
        $rw = [regex]::Match($st, 'ui_reference_width\s+(\d+)')
        $rh = [regex]::Match($st, 'ui_reference_height\s+(\d+)')
        $vw = if ($rw.Success) { [int]$rw.Groups[1].Value } else { -1 }
        $vh = if ($rh.Success) { [int]$rh.Groups[1].Value } else { -1 }
        if ($vw -ne $wantW -or $vh -ne $wantH) { $badScenes += "$($scn.Name)(${vw}x${vh})" }
    }
    Write-Host "  scenes/*.scene 共 $sceneCount 个，全部 ui_reference = ${wantW}x${wantH}"
    if ($badScenes.Count -gt 0) { Fail "场景分辨率元数据漂移: $($badScenes -join ', ')" }
}

# ══ 汇总 ════════════════════════════════════════════════════════════════════
Write-Section "汇总"
Write-Host ("  硬失败: " + $script:hardFails.Count)
foreach ($m in $script:hardFails) { Write-Host "    - $m" -ForegroundColor Red }
Write-Host ("  警告  : " + $script:warnings.Count)
foreach ($m in $script:warnings)  { Write-Host "    - $m" -ForegroundColor Yellow }

if ($script:hardFails.Count -gt 0) {
    Write-Host "QA FAILED（硬失败 $($script:hardFails.Count) 项）" -ForegroundColor Red
    exit 1
}
Write-Host "QA PASSED（警告 $($script:warnings.Count) 项，非阻断）" -ForegroundColor Green
exit 0
