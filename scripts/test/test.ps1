[CmdletBinding()]
param([switch]$NoClippy)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$verify = Join-Path $PSScriptRoot "..\verify\verify.ps1"

$params = @{
    SkipBuild = $true
}
if ($NoClippy) { $params.SkipClippy = $true }

& $verify @params
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}