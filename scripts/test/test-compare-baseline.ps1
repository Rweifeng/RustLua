$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$script = Join-Path $PSScriptRoot "..\verify\compare-baseline.ps1"
$temp = Join-Path $root "artifacts\tests\baseline-compare"
New-Item -ItemType Directory -Force -Path $temp | Out-Null
# docs/54 §7 的红线必须**阻断**：阈值内的慢化放行，超出 p95 +5% / p99 +10% 或内存 +10% 一律判红。
# 这条测试曾经把"+6% / +11% 必须通过"当成契约——那正好把红线定义成了成功，已按文档口径改正。
$base = '{"report_version":"wp0-v3","stairs":[{"mesh_count":100,"cpu_peak_bytes":1000,"samples":{"p95_us":100,"p99_us":100},"render":{"draw_calls":2,"instances":100,"triangles":100}}]}'
$withinLimits = '{"report_version":"wp0-v3","stairs":[{"mesh_count":100,"cpu_peak_bytes":1050,"samples":{"p95_us":104,"p99_us":109},"render":{"draw_calls":2,"instances":100,"triangles":100}}]}'
$latencyRegression = '{"report_version":"wp0-v3","stairs":[{"mesh_count":100,"cpu_peak_bytes":1050,"samples":{"p95_us":106,"p99_us":111},"render":{"draw_calls":2,"instances":100,"triangles":100}}]}'
$memoryRegression = '{"report_version":"wp0-v3","stairs":[{"mesh_count":100,"cpu_peak_bytes":1200,"samples":{"p95_us":100,"p99_us":100},"render":{"draw_calls":2,"instances":100,"triangles":100}}]}'
$geometryRegression = '{"report_version":"wp0-v3","stairs":[{"mesh_count":100,"cpu_peak_bytes":1000,"samples":{"p95_us":100,"p99_us":100},"render":{"draw_calls":3,"instances":100,"triangles":100}}]}'
$basePath = Join-Path $temp "base.json"
$withinPath = Join-Path $temp "within.json"
$latencyPath = Join-Path $temp "latency.json"
$memoryPath = Join-Path $temp "memory.json"
$errorPath = Join-Path $temp "error.json"
Set-Content -LiteralPath $basePath -Value $base -Encoding UTF8
Set-Content -LiteralPath $withinPath -Value $withinLimits -Encoding UTF8
Set-Content -LiteralPath $latencyPath -Value $latencyRegression -Encoding UTF8
Set-Content -LiteralPath $memoryPath -Value $memoryRegression -Encoding UTF8
Set-Content -LiteralPath $errorPath -Value $geometryRegression -Encoding UTF8
& powershell -NoProfile -ExecutionPolicy Bypass -File $script -Previous $basePath -Current $withinPath
if ($LASTEXITCODE -ne 0) { throw "within-limits comparison must pass, got $LASTEXITCODE" }
& powershell -NoProfile -ExecutionPolicy Bypass -File $script -Previous $basePath -Current $latencyPath
if ($LASTEXITCODE -ne 2) { throw "p95/p99 over-limit must exit 2, got $LASTEXITCODE" }
& powershell -NoProfile -ExecutionPolicy Bypass -File $script -Previous $basePath -Current $memoryPath
if ($LASTEXITCODE -ne 2) { throw "memory over-limit must exit 2, got $LASTEXITCODE" }
& powershell -NoProfile -ExecutionPolicy Bypass -File $script -Previous $basePath -Current $errorPath
if ($LASTEXITCODE -ne 2) { throw "geometry regression must exit 2, got $LASTEXITCODE" }
# docs/54 §8-⑩：`-GeometryOnly`（进门时对比**提交进版本库**的基线）跳过机器相关的时间，
# 但几何量与分配峰值仍然判红——否则这一档就成了"什么都不查"。
& powershell -NoProfile -ExecutionPolicy Bypass -File $script -Previous $basePath -Current $latencyPath -GeometryOnly
if ($LASTEXITCODE -ne 0) { throw "geometry-only mode must ignore latency regressions, got $LASTEXITCODE" }
& powershell -NoProfile -ExecutionPolicy Bypass -File $script -Previous $basePath -Current $memoryPath -GeometryOnly
if ($LASTEXITCODE -ne 2) { throw "geometry-only mode must still block memory regressions, got $LASTEXITCODE" }
& powershell -NoProfile -ExecutionPolicy Bypass -File $script -Previous $basePath -Current $errorPath -GeometryOnly
if ($LASTEXITCODE -ne 2) { throw "geometry-only mode must still block geometry regressions, got $LASTEXITCODE" }
Write-Host "baseline_compare_tests_ok cases=7" -ForegroundColor Green
