[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent $PSScriptRoot
$outputPath = Join-Path $projectRoot "game\assets\models\m8-test-triangle.glb"
$outputDirectory = Split-Path -Parent $outputPath
New-Item -ItemType Directory -Force -Path $outputDirectory | Out-Null

# Self-authored unit triangle: three positions and one indexed primitive.
$positions = [Single[]](
    -0.5, -0.5, 0.0,
     0.5, -0.5, 0.0,
     0.0,  0.5, 0.0
)
$indices = [UInt16[]](0, 1, 2)
$bin = New-Object System.IO.MemoryStream
$writer = New-Object System.IO.BinaryWriter($bin)
foreach ($value in $positions) { $writer.Write($value) }
foreach ($value in $indices) { $writer.Write($value) }
while (($bin.Length % 4) -ne 0) { $writer.Write([Byte]0) }
$binBytes = $bin.ToArray()
$writer.Dispose()
$bin.Dispose()

$json = @"
{
  "asset": {"version": "2.0", "generator": "p2 self-authored M8 test asset"},
  "scene": 0,
  "scenes": [{"nodes": [0]}],
  "nodes": [{"mesh": 0, "name": "M8TestTriangle"}],
  "meshes": [{"name": "M8TestTriangle", "primitives": [{"attributes": {"POSITION": 0}, "indices": 1, "mode": 4}]}],
  "bufferViews": [
    {"buffer": 0, "byteOffset": 0, "byteLength": 36, "target": 34962},
    {"buffer": 0, "byteOffset": 36, "byteLength": 6, "target": 34963}
  ],
  "accessors": [
    {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [-0.5, -0.5, 0.0], "max": [0.5, 0.5, 0.0]},
    {"bufferView": 1, "componentType": 5123, "count": 3, "type": "SCALAR"}
  ],
  "buffers": [{"byteLength": 44}]
}
"@.Trim()
$jsonBytes = [Text.Encoding]::UTF8.GetBytes($json)
while (($jsonBytes.Length % 4) -ne 0) { $jsonBytes += [Byte]0x20 }

$file = New-Object System.IO.MemoryStream
$fileWriter = New-Object System.IO.BinaryWriter($file)
$fileWriter.Write([UInt32]0x46546C67)
$fileWriter.Write([UInt32]2)
$fileWriter.Write([UInt32](12 + 8 + $jsonBytes.Length + 8 + $binBytes.Length))
$fileWriter.Write([UInt32]$jsonBytes.Length)
$fileWriter.Write([UInt32]0x4E4F534A)
$fileWriter.Write($jsonBytes)
$fileWriter.Write([UInt32]$binBytes.Length)
$fileWriter.Write([UInt32]0x004E4942)
$fileWriter.Write($binBytes)
[IO.File]::WriteAllBytes($outputPath, $file.ToArray())
$fileWriter.Dispose()
$file.Dispose()

Write-Host "Generated self-authored GLB: $outputPath"
Write-Host ("Size: {0} bytes" -f (Get-Item -LiteralPath $outputPath).Length)