# 叠加层帧耗时 A/B 基准（M97）：临时给工程配 .sgraph → 跑三档 → 还原 → 输出对比表。
#
# 为什么要临时改工程：叠加层是工程级配置（`[render] effect_overlay`），而 A/B 的
# 三档（无叠加层 / 纯程化快路径 / 采样场景色离屏回拷）只差这一行配置。脚本保证
# 无论成功失败都把 project.toml 还原、临时资产删掉，不把工作区留在半改状态。
#
# 用法：
#   pwsh scripts/verify/bench_overlay_ab.ps1
#   pwsh scripts/verify/bench_overlay_ab.ps1 -Frames 600 -Project examples/game_api
#
# 判据（宽松，只为拦住"没量到"与"读坏"）：
#   - 三档都跑满 Frames 帧且生成了 profile JSON；
#   - 每档 frame.p50/p95 为正、p95 >= p50；
#   - 打印 p50/p95/submit 对比与相对基准的差值，不做跨硬件阈值断言。
param(
    [string]$Project = "examples/game_api",
    [int]$Frames = 240,
    [string]$OutDir = "artifacts/logs"
)

$ErrorActionPreference = "Stop"
$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$project_root = (Resolve-Path (Join-Path $repo $Project)).Path
$project_toml = Join-Path $project_root "project.toml"
$fx_dir = Join-Path $project_root "assets\fx"
$out_dir = (Join-Path $repo $OutDir)
New-Item -ItemType Directory -Force -Path $out_dir | Out-Null

$game = Join-Path $repo "engine\target\release\game.exe"
if (-not (Test-Path $game)) {
    throw "缺少 release 版 game.exe；先跑: cargo build --release --manifest-path engine/Cargo.toml -p game"
}

$backup_toml = [System.IO.File]::ReadAllText($project_toml, [System.Text.Encoding]::UTF8)
if ($backup_toml -match "(?m)^\[render\]") {
    throw "工程已带 [render] 段，拒绝覆盖：请先在干净工程上跑本脚本"
}

$fire_graph = @'
{
  "version": 1,
  "name": "bench_fire",
  "nodes": [
    {"id": "uv", "kind": "uv"},
    {"id": "tex", "kind": "texture"},
    {"id": "flame", "kind": "constant", "params": {"r": 1.0, "g": 0.45, "b": 0.1}},
    {"id": "tint", "kind": "multiply"},
    {"id": "time", "kind": "time"},
    {"id": "flicker", "kind": "sine", "params": {"frequency": 16.0}},
    {"id": "out", "kind": "output"}
  ],
  "edges": [
    {"from": {"node": "uv", "port": "value"}, "to": {"node": "tex", "port": "uv"}},
    {"from": {"node": "tex", "port": "value"}, "to": {"node": "tint", "port": "a"}},
    {"from": {"node": "flame", "port": "value"}, "to": {"node": "tint", "port": "b"}},
    {"from": {"node": "tint", "port": "value"}, "to": {"node": "out", "port": "tint"}},
    {"from": {"node": "time", "port": "value"}, "to": {"node": "flicker", "port": "time"}},
    {"from": {"node": "flicker", "port": "value"}, "to": {"node": "out", "port": "alpha"}}
  ]
}
'@

$scene_graph = @'
{
  "version": 1,
  "name": "bench_scene",
  "nodes": [
    {"id": "uv", "kind": "uv"},
    {"id": "spin", "kind": "rotate_uv", "params": {"angle0": 0.0, "spin_speed": 1.5}},
    {"id": "scene", "kind": "scene_color"},
    {"id": "time", "kind": "time"},
    {"id": "swirl", "kind": "sine", "params": {"frequency": 2.0}},
    {"id": "out", "kind": "output"}
  ],
  "edges": [
    {"from": {"node": "uv", "port": "value"}, "to": {"node": "spin", "port": "uv"}},
    {"from": {"node": "spin", "port": "value"}, "to": {"node": "scene", "port": "uv"}},
    {"from": {"node": "scene", "port": "value"}, "to": {"node": "out", "port": "tint"}},
    {"from": {"node": "time", "port": "value"}, "to": {"node": "swirl", "port": "time"}},
    {"from": {"node": "swirl", "port": "value"}, "to": {"node": "out", "port": "alpha"}}
  ]
}
'@

function Set-Mode([string]$mode) {
    switch ($mode) {
        "off" {
            [System.IO.File]::WriteAllText($project_toml, $backup_toml, (New-Object System.Text.UTF8Encoding($false)))
        }
        "fire" {
            New-Item -ItemType Directory -Force -Path $fx_dir | Out-Null
            [System.IO.File]::WriteAllText((Join-Path $fx_dir "bench_fire.sgraph"), $fire_graph, (New-Object System.Text.UTF8Encoding($false)))
            [System.IO.File]::WriteAllText($project_toml, $backup_toml.TrimEnd() + "`n`n[render]`neffect_overlay = `"assets/fx/bench_fire.sgraph`"`n", (New-Object System.Text.UTF8Encoding($false)))
        }
        "scene" {
            New-Item -ItemType Directory -Force -Path $fx_dir | Out-Null
            [System.IO.File]::WriteAllText((Join-Path $fx_dir "bench_scene.sgraph"), $scene_graph, (New-Object System.Text.UTF8Encoding($false)))
            [System.IO.File]::WriteAllText($project_toml, $backup_toml.TrimEnd() + "`n`n[render]`neffect_overlay = `"assets/fx/bench_scene.sgraph`"`n", (New-Object System.Text.UTF8Encoding($false)))
        }
        default { throw "未知档位 $mode" }
    }
}

$rows = @()
try {
    foreach ($mode in @("off", "fire", "scene")) {
        Set-Mode $mode
        $profile = Join-Path $out_dir "perf-$mode.json"
        & $game --project $project_root --exit-after-frames $Frames --fixed-step --frame-profile $profile 2>&1 |
            Select-String -Pattern "m74_frame_profile frames" | Select-Object -Last 1
        if (-not (Test-Path $profile)) { throw "档位 $mode 没生成 frame profile：$profile" }
        $data = Get-Content $profile -Raw -Encoding UTF8 | ConvertFrom-Json
        $frame = $data.frame
        if ($frame.count -ne $Frames) { throw "档位 $mode 只采到 $($frame.count)/$Frames 帧" }
        if ($frame.p50_us -le 0 -or $frame.p95_us -le 0) { throw "档位 $mode 的 p50/p95 不是正数" }
        if ($frame.p95_us -lt $frame.p50_us) { throw "档位 $mode 的 p95 < p50，读数可疑" }
        $rows += [pscustomobject]@{
            mode      = $mode
            frame_p50 = [double]$frame.p50_us
            frame_p95 = [double]$frame.p95_us
            submit_p50 = [double]$data.host.submit.p50_us
        }
    }
    $base = $rows[0]
    Write-Output ""
    Write-Output ("{0,-8}{1,14}{2,14}{3,14}" -f "mode", "frame_p50_us", "frame_p95_us", "submit_p50_us")
    foreach ($row in $rows) {
        Write-Output ("{0,-8}{1,14:F1}{2,14:F1}{3,14:F1}" -f $row.mode, $row.frame_p50, $row.frame_p95, $row.submit_p50)
    }
    foreach ($row in $rows[1..($rows.Count - 1)]) {
        $d50 = $row.frame_p50 - $base.frame_p50
        $d95 = $row.frame_p95 - $base.frame_p95
        $pct = (($row.frame_p50 / $base.frame_p50) - 1) * 100
        Write-Output ("delta {0}: frame_p50 {1:+0.0;-0.0} us ({2:+0.00;-0.00}%), frame_p95 {3:+0.0;-0.0} us" -f $row.mode, $d50, $pct, $d95)
    }
    Write-Output "bench_overlay_ab_ok frames=$Frames out=$out_dir"
}
finally {
    [System.IO.File]::WriteAllText($project_toml, $backup_toml, (New-Object System.Text.UTF8Encoding($false)))
    if (Test-Path $fx_dir) { Remove-Item -Recurse -Force $fx_dir }
}
