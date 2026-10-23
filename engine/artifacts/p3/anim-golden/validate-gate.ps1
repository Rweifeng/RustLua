$ErrorActionPreference = "Stop"
$path = "E:\Codex\RustLua\scripts\verify\verify.ps1"

$errors = $null
[System.Management.Automation.Language.Parser]::ParseFile($path, [ref]$null, [ref]$errors) | Out-Null
if ($errors -and $errors.Count -gt 0) {
    $errors | ForEach-Object { Write-Host $_.Message }
    throw "verify.ps1 has parse errors: $($errors.Count)"
}
Write-Host "PARSE_OK"

$verify = Get-Content -LiteralPath $path -Raw
$start = $verify.LastIndexOf('# P3-E01-4a')
if ($start -lt 0) { throw "gate block start not found" }
$end = $verify.IndexOf('# docs/54', $start)
if ($end -lt 0) { throw "gate block end not found" }
$block = $verify.Substring($start, $end - $start)

$reportPath = "E:\Codex\RustLua\engine\artifacts\p3\anim-golden\wp0-anim-golden.json"
$animGoldenSkipped = $false
$animGoldenPath = $reportPath
Invoke-Expression $block
Write-Host "GATE_BLOCK_OK"

function Test-Mutation([string]$name, [scriptblock]$mutate, [string]$expect) {
    $report = Get-Content -LiteralPath $reportPath -Raw | ConvertFrom-Json
    & $mutate $report
    $mutated = Join-Path (Split-Path $reportPath) "mutated-$name.json"
    $report | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $mutated -Encoding UTF8
    $script:animGoldenPath = $mutated
    $failed = $false
    $message = ""
    try {
        Invoke-Expression $block | Out-Null
    } catch {
        $failed = $true
        $message = $_.Exception.Message
    } finally {
        Remove-Item -LiteralPath $mutated -ErrorAction SilentlyContinue
    }
    if (-not $failed) { throw "gate accepted mutated report: $name" }
    if ($message -notlike $expect) { throw "gate failed for the wrong reason ($name): $message" }
    Write-Host "MUTATION_CAUGHT $name"
}

Test-Mutation "sprite-world-x" { param($r) $r.frames[1].sprite_world_x = 650.0 } "*anim golden frame 1*"
Test-Mutation "clamp" { param($r) $r.clamped_after_duration.unchanged = $false } "*anim golden*"
Test-Mutation "check-flag" { param($r) $r.checks.pixels_track_the_transform = $false } "*pixels_track_the_transform*"
Write-Host "NEGATIVE_GATES_OK"
