$ErrorActionPreference = "Stop"
$scriptPath = "E:\Codex\RustLua\scripts\verify\verify.ps1"
$text = Get-Content -LiteralPath $scriptPath -Raw
$startMarker = '    if (-not $gpuParticles50kSkipped) {'
$endMarker = '    Write-Host "WP0 benchmark baselines'
$start = $text.IndexOf($startMarker)
$end = $text.IndexOf($endMarker)
if ($start -lt 0 -or $end -lt 0 -or $end -le $start) { throw "gate block markers not found start=$start end=$end" }
$block = $text.Substring($start, $end - $start)
Write-Host "HARNESS extracted $($block.Length) chars"

$gpuParticles50kSkipped = $false
$gpuParticles50kPath = "E:\Codex\RustLua\baselines\wp0-gpu-particles-50k.json"
Invoke-Expression $block
Write-Host "GATE_BLOCK_50K_OK"

$bad = Get-Content -LiteralPath $gpuParticles50kPath -Raw -Encoding UTF8 | ConvertFrom-Json
$bad.semantic.state_bytes = 480000
$badPath = Join-Path $env:TEMP "wp0-gpu-particles-50k-bad.json"
$bad | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $badPath -Encoding UTF8
$gpuParticles50kPath = $badPath
$caught = $false
try {
    Invoke-Expression $block
}
catch {
    $caught = $true
    Write-Host "NEGATIVE_OK state_bytes tamper caught: $($_.Exception.Message)"
}
if (-not $caught) { throw "gate did not catch tampered state_bytes" }
Remove-Item -LiteralPath $badPath -Force
