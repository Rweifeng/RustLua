# One-off dev tool: generate short 16-bit PCM WAV sound effects.
function Write-Wav([string]$path, [int]$frequency, [double]$duration) {
    $sampleRate = 22050
    $samples = [int]($sampleRate * $duration)
    $dataSize = $samples * 2
    $bytes = New-Object byte[] (44 + $dataSize)
    $stream = New-Object System.IO.MemoryStream
    $writer = New-Object System.IO.BinaryWriter $stream
    $writer.Write([System.Text.Encoding]::ASCII.GetBytes("RIFF"))
    $writer.Write([int](36 + $dataSize))
    $writer.Write([System.Text.Encoding]::ASCII.GetBytes("WAVE"))
    $writer.Write([System.Text.Encoding]::ASCII.GetBytes("fmt "))
    $writer.Write([int]16)
    $writer.Write([int16]1)          # PCM
    $writer.Write([int16]1)          # mono
    $writer.Write([int]$sampleRate)
    $writer.Write([int]($sampleRate * 2))
    $writer.Write([int16]2)          # block align
    $writer.Write([int16]16)         # bits per sample
    $writer.Write([System.Text.Encoding]::ASCII.GetBytes("data"))
    $writer.Write([int]$dataSize)
    $phase = 0.0
    for ($i = 0; $i -lt $samples; $i++) {
        $v = [math]::Sin(2 * [math]::PI * $frequency * $i / $sampleRate)
        # envelope: quick attack, exponential-ish decay
        $env = 1.0 - ($i / $samples)
        $sample = [int16](($v * 0.8 * $env) * 32767)
        $writer.Write($sample)
    }
    $writer.Flush()
    $out = $stream.ToArray()
    $writer.Dispose()
    $stream.Dispose()
    [System.IO.File]::WriteAllBytes($path, $out)
    Write-Output "wav -> $path"
}

$root = Split-Path -Parent $PSScriptRoot
$assets = Join-Path $root "game\assets\audio"
Write-Wav (Join-Path $assets "attack.wav") 440 0.15
Write-Wav (Join-Path $assets "hit.wav") 160 0.25
