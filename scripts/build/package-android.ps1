[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$ProjectRoot,
    [string]$PublishName = "",
    [string]$ApplicationId = "com.rustlua.game",
    [string]$SdkRoot = "",
    [string]$NdkRoot = "",
    [string]$JdkHome = "",
    [string]$GradleExecutable = "",
    [switch]$Compress,
    [switch]$Encrypt
)

$ErrorActionPreference = "Stop"

function Set-TomlProjectKey {
    param([string]$Path, [string]$Key, [string]$Value)

    $text = [IO.File]::ReadAllText($Path, [Text.Encoding]::UTF8)
    $tomlValue = if ($Key -eq "release" -and $Value -eq "true") {
        "true"
    } else {
        ConvertTo-Json -InputObject $Value -Compress
    }
    $lines = [Collections.Generic.List[string]]::new()
    $inProject = $false
    $insertAt = -1
    $firstSectionStart = -1
    foreach ($line in ($text -split "`n")) {
        $trimmed = $line.Trim().TrimStart([char]0xFEFF)
        if ($trimmed -match '^\[') {
            if ($inProject) { $insertAt = $lines.Count; $inProject = $false }
            if ($firstSectionStart -lt 0) { $firstSectionStart = $lines.Count }
            if ($trimmed -eq '[project]') { $inProject = $true }
        }
        if ($trimmed -notmatch ('^' + [regex]::Escape($Key) + '\s*=')) {
            $lines.Add($line.TrimEnd("`r"))
        }
    }
    if ($insertAt -lt 0) {
        $insertAt = if ($firstSectionStart -ge 0) { $firstSectionStart } else { $lines.Count }
    }
    $lines.Insert($insertAt, "$Key = $tomlValue")
    $updated = [string]::Join("`n", $lines)
    if ($text.EndsWith("`n")) { $updated += "`n" }
    [IO.File]::WriteAllText($Path, $updated, [Text.UTF8Encoding]::new($false))
}

function Get-ApplicationIdError {
    param([string]$Value)
    $parts = $Value.Split('.')
    if ($parts.Length -lt 2) { return "applicationId needs at least two dot-separated segments." }
    foreach ($part in $parts) {
        if ($part -notmatch '^[A-Za-z][A-Za-z0-9_]*$') {
            return "Invalid applicationId segment '$part'; use letters, digits, and underscores, starting with a letter."
        }
    }
    return $null
}

$ProjectRoot = (Resolve-Path -LiteralPath $ProjectRoot).Path
$projectToml = Join-Path $ProjectRoot "project.toml"
if (-not (Test-Path -LiteralPath $projectToml -PathType Leaf)) {
    throw "Project is missing project.toml: $ProjectRoot"
}
$appIdError = Get-ApplicationIdError $ApplicationId
if ($appIdError) { throw $appIdError }

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$engineRoot = Join-Path $repoRoot "engine"
$pluginRoot = Join-Path $repoRoot "plugins\android"
$SdkRoot = if (-not [string]::IsNullOrWhiteSpace($SdkRoot)) {
    $SdkRoot
} elseif ($env:ANDROID_SDK_ROOT) {
    $env:ANDROID_SDK_ROOT
} else {
    $env:ANDROID_HOME
}
if ([string]::IsNullOrWhiteSpace($SdkRoot) -or -not (Test-Path -LiteralPath $SdkRoot -PathType Container)) {
    throw "Android SDK directory not found. Configure SdkRoot in the editor publishing panel."
}
if (-not [string]::IsNullOrWhiteSpace($JdkHome)) {
    if (-not (Test-Path -LiteralPath (Join-Path $JdkHome "bin\java.exe") -PathType Leaf)) {
        throw "JDK 17 java.exe was not found under JdkHome: $JdkHome"
    }
    $env:JAVA_HOME = $JdkHome
    $env:PATH = (Join-Path $JdkHome "bin") + ";" + $env:PATH
} elseif (-not $env:JAVA_HOME -and -not (Get-Command java -ErrorAction SilentlyContinue)) {
    throw "JDK 17 was not found. Configure JdkHome in the editor publishing panel."
}
$javaCommand = if ($env:JAVA_HOME) {
    Join-Path $env:JAVA_HOME "bin\java.exe"
} else {
    (Get-Command java -ErrorAction Stop).Source
}
$previousErrorActionPreference = $ErrorActionPreference
try {
    $ErrorActionPreference = "Continue"
    $javaVersionOutput = & $javaCommand -version 2>&1
    $javaExitCode = $LASTEXITCODE
} finally {
    $ErrorActionPreference = $previousErrorActionPreference
}
if ($javaExitCode -ne 0 -or ($javaVersionOutput -join "`n") -notmatch 'version "17\.') {
    throw "JDK 17 is required. Configure JdkHome in the editor publishing panel."
}
$env:ANDROID_HOME = $SdkRoot
$env:ANDROID_SDK_ROOT = $SdkRoot
if (-not [string]::IsNullOrWhiteSpace($NdkRoot)) {
    if (-not (Test-Path -LiteralPath (Join-Path $NdkRoot "source.properties") -PathType Leaf)) {
        throw "Android NDK source.properties was not found under NdkRoot: $NdkRoot"
    }
    $env:ANDROID_NDK_HOME = $NdkRoot
    $env:ANDROID_NDK_ROOT = $NdkRoot
} elseif (-not (Get-ChildItem -LiteralPath (Join-Path $SdkRoot "ndk") -Directory -ErrorAction SilentlyContinue)) {
    throw "Android NDK was not found. Configure NdkRoot in the editor publishing panel."
}

Push-Location $engineRoot
try {
    $installedTargets = & rustup target list --installed
    if ($LASTEXITCODE -ne 0) { throw "Unable to query installed Rust targets." }
} finally {
    Pop-Location
}
if ($installedTargets -notcontains "aarch64-linux-android") {
    throw "Rust target aarch64-linux-android is not installed. Run: rustup target add aarch64-linux-android"
}
if (-not (Get-Command cargo-ndk -ErrorAction SilentlyContinue)) {
    throw "cargo-ndk is unavailable. Install it with: cargo install cargo-ndk"
}
$gradleCommand = if (-not [string]::IsNullOrWhiteSpace($GradleExecutable)) {
    if (-not (Test-Path -LiteralPath $GradleExecutable -PathType Leaf)) {
        throw "Gradle executable was not found: $GradleExecutable"
    }
    $GradleExecutable
} else {
    $command = Get-Command gradle -ErrorAction SilentlyContinue
    if (-not $command) { throw "Gradle 8.9+ was not found. Configure Gradle in the editor publishing panel." }
    $command.Source
}
$gradleVersionOutput = & $gradleCommand --version
$versionMatch = [regex]::Match(($gradleVersionOutput -join "`n"), 'Gradle\s+(\d+)\.(\d+)')
if ($LASTEXITCODE -ne 0 -or -not $versionMatch.Success -or
    [version]::new([int]$versionMatch.Groups[1].Value, [int]$versionMatch.Groups[2].Value) -lt [version]"8.9") {
    throw "Gradle 8.9 or newer is required. Executable: $gradleCommand"
}

$buildRoot = Join-Path $ProjectRoot "builds\android"
[IO.Directory]::CreateDirectory($buildRoot) | Out-Null
$publishToken = [Guid]::NewGuid().ToString("N")
$stageRoot = Join-Path $buildRoot (".stage-" + $publishToken)
$packRoot = Join-Path $stageRoot "app\src\main\assets\pack"
$outputName = if ([string]::IsNullOrWhiteSpace($PublishName)) {
    [IO.Path]::GetFileName($ProjectRoot)
} else {
    $PublishName.Trim()
}
foreach ($invalid in [IO.Path]::GetInvalidFileNameChars()) {
    $outputName = $outputName.Replace([string]$invalid, "_")
}
if ([string]::IsNullOrWhiteSpace($outputName) -or $outputName -in @(".", "..") -or
    $outputName -match '^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])$') {
    $outputName = "game"
}
$finalApk = Join-Path $buildRoot "$outputName.apk"
$tempApk = Join-Path $buildRoot ".$outputName.$publishToken.tmp.apk"
$backupApk = Join-Path $buildRoot ".$outputName.$publishToken.previous.apk"

try {
    [IO.Directory]::CreateDirectory($stageRoot) | Out-Null
    Copy-Item -Path (Join-Path $pluginRoot "*") -Destination $stageRoot -Recurse -Force
    [IO.Directory]::CreateDirectory((Split-Path -Parent $packRoot)) | Out-Null

    Write-Host "==> Android asset package"
    Push-Location $engineRoot
    try {
        & cargo build --release -p assetc
        if ($LASTEXITCODE -ne 0) { throw "Failed to build assetc." }
    } finally {
        Pop-Location
    }
    $assetc = Join-Path $engineRoot "target\release\assetc.exe"
    if (-not (Test-Path -LiteralPath $assetc)) { throw "assetc binary was not produced: $assetc" }
    $assetArgs = @("package", $ProjectRoot, "-o", $packRoot, "--bundle", "grouped", "--keep", "assets/fonts/", "--keep", "chars.txt")
    if ($Encrypt) { $assetArgs += "--encrypt" }
    if ($Compress) {
        Write-Host "Android grouped assets use built-in block compression; the per-file compression switch is ignored."
    }
    & $assetc @assetArgs
    if ($LASTEXITCODE -ne 0) { throw "assetc failed to package Android resources." }

    $effectiveName = if ([string]::IsNullOrWhiteSpace($PublishName)) {
        $ProjectRoot | Split-Path -Leaf
    } else {
        $PublishName.Trim()
    }
    Set-TomlProjectKey -Path (Join-Path $packRoot "project.toml") -Key "publish_name" -Value $effectiveName
    Set-TomlProjectKey -Path (Join-Path $packRoot "project.toml") -Key "release" -Value "true"

    $assetFiles = @(Get-ChildItem -LiteralPath $packRoot -File -Recurse | Sort-Object FullName)
    if (-not ($assetFiles | Where-Object { $_.Name -eq "project.toml" })) {
        throw "Packaged resources are missing project.toml."
    }
    $assetEntries = [Collections.Generic.List[string]]::new()
    $hashInput = [Text.StringBuilder]::new()
    foreach ($file in $assetFiles) {
        $relative = "pack/" + $file.FullName.Substring($packRoot.Length + 1).Replace("\", "/")
        $fileHash = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        $assetEntries.Add($relative)
        [void]$hashInput.Append($relative).Append(" ").Append($fileHash).Append("`n")
    }
    $versionBytes = [Text.Encoding]::UTF8.GetBytes($hashInput.ToString())
    $sha256 = [Security.Cryptography.SHA256]::Create()
    try {
        $version = [BitConverter]::ToString($sha256.ComputeHash($versionBytes)).Replace("-", "").ToLowerInvariant()
    } finally {
        $sha256.Dispose()
    }
    $assetIndex = @("version=$version") + $assetEntries.ToArray()
    $indexPath = Join-Path (Split-Path -Parent $packRoot) "rustlua-asset-index.txt"
    [IO.File]::WriteAllLines($indexPath, $assetIndex, [Text.UTF8Encoding]::new($false))

    $stringsPath = Join-Path $stageRoot "app\src\main\res\values\strings.xml"
    $displayName = [Security.SecurityElement]::Escape($effectiveName)
    [IO.File]::WriteAllText($stringsPath, "<resources>`n    <string name=`"app_name`">$displayName</string>`n</resources>`n", [Text.UTF8Encoding]::new($false))

    Write-Host "==> Android release build (arm64-v8a)"
    $jniLibs = Join-Path $stageRoot "app\src\main\jniLibs"
    Push-Location $engineRoot
    try {
        & cargo ndk -t arm64-v8a -o $jniLibs build --manifest-path (Join-Path $engineRoot "Cargo.toml") -p game --lib --release
        if ($LASTEXITCODE -ne 0) { throw "Android Rust runtime build failed." }
    } finally {
        Pop-Location
    }

    Push-Location $stageRoot
    try {
        & $gradleCommand --no-daemon -p $stageRoot assembleRelease "-PapplicationId=$ApplicationId"
        if ($LASTEXITCODE -ne 0) { throw "Gradle failed to build the Android APK." }
    } finally {
        Pop-Location
    }
    $builtApk = Join-Path $stageRoot "app\build\outputs\apk\release\app-release.apk"
    if (-not (Test-Path -LiteralPath $builtApk -PathType Leaf)) {
        throw "Gradle completed without producing the expected APK: $builtApk"
    }

    Copy-Item -LiteralPath $builtApk -Destination $tempApk -Force
    if (Test-Path -LiteralPath $finalApk) {
        [IO.File]::Replace($tempApk, $finalApk, $backupApk, $true)
    } else {
        [IO.File]::Move($tempApk, $finalApk)
    }
    if (Test-Path -LiteralPath $backupApk) {
        try {
            Remove-Item -LiteralPath $backupApk -Force
        } catch {
            Write-Warning "APK was published, but the previous APK backup could not be removed: $backupApk"
        }
    }
    Write-Host "Android APK ready: $finalApk"
} finally {
    if (Test-Path -LiteralPath $tempApk) { Remove-Item -LiteralPath $tempApk -Force }
    if (Test-Path -LiteralPath $stageRoot) { Remove-Item -LiteralPath $stageRoot -Recurse -Force }
}
