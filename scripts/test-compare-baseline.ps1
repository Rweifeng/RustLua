$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$script = Join-Path $PSScriptRoot "compare-baseline.ps1"
$temp = Join-Path $root "artifacts\tests\baseline-compare"
New-Item -ItemType Directory -Force -Path $temp | Out-Null
$base = '{"report_version":"wp0-v3","stairs":[{"mesh_count":100,"samples":{"p95_us":100,"p99_us":100},"render":{"draw_calls":2,"instances":100,"triangles":100}}]}'
$warning = '{"report_version":"wp0-v3","stairs":[{"mesh_count":100,"samples":{"p95_us":106,"p99_us":111},"render":{"draw_calls":2,"instances":100,"triangles":100}}]}'
$regression = '{"report_version":"wp0-v3","stairs":[{"mesh_count":100,"samples":{"p95_us":100,"p99_us":100},"render":{"draw_calls":3,"instances":100,"triangles":100}}]}'
$basePath = Join-Path $temp "base.json"
$warningPath = Join-Path $temp "warning.json"
$errorPath = Join-Path $temp "error.json"
Set-Content -LiteralPath $basePath -Value $base -Encoding UTF8
Set-Content -LiteralPath $warningPath -Value $warning -Encoding UTF8
Set-Content -LiteralPath $errorPath -Value $regression -Encoding UTF8
& powershell -NoProfile -ExecutionPolicy Bypass -File $script -Previous $basePath -Current $warningPath
if ($LASTEXITCODE -ne 0) { throw "warning-only comparison must pass" }
& powershell -NoProfile -ExecutionPolicy Bypass -File $script -Previous $basePath -Current $errorPath
if ($LASTEXITCODE -ne 2) { throw "geometry regression must exit 2, got $LASTEXITCODE" }
Write-Host "baseline_compare_tests_ok cases=2" -ForegroundColor Green
