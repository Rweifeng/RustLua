[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$InstallRoot,
    [switch]$AcceptAndroidLicenses
)

$ErrorActionPreference = "Stop"
$CommandLineToolsUrl = "https://dl.google.com/android/repository/commandlinetools-win-11076708_latest.zip"
$CommandLineToolsSha1 = "3d2917302740f476999a091bc5558837c7a863c5"
$JdkUrl = "https://github.com/adoptium/temurin17-binaries/releases/download/jdk-17.0.21%2B9/OpenJDK17U-jdk_x64_windows_hotspot_17.0.21_9.zip"
$JdkSha256 = "98518ecf554465057ee5509462631deeee374bc368c12a411d225cd29381f84b"
$GradleUrl = "https://services.gradle.org/distributions/gradle-8.9-bin.zip"
$GradleSha256 = "d725d707bfabd4dfdc958c624003b3c80accc03f7037b5122c4b1d0ef15cecab"
$NdkVersion = "27.2.12479018"

function Get-Sha256 {
    param([string]$Path)
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-Sha1 {
    param([string]$Path)
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA1).Hash.ToLowerInvariant()
}

function Download-Verified {
    param([string]$Url, [string]$Destination, [string]$ExpectedHash, [ValidateSet("SHA1", "SHA256")][string]$Algorithm)

    if (Test-Path -LiteralPath $Destination -PathType Leaf) {
        $existingHash = if ($Algorithm -eq "SHA1") { Get-Sha1 $Destination } else { Get-Sha256 $Destination }
        if ($existingHash -eq $ExpectedHash) {
            Write-Host "==> Reusing verified download: $(Split-Path -Leaf $Destination)"
            return
        }
    }
    if (Test-Path -LiteralPath $Destination) {
        Remove-Item -LiteralPath $Destination -Force
    }
    Write-Host "==> Downloading $Url"
    & curl.exe --fail --location --retry 3 --retry-delay 2 --output $Destination $Url
    if ($LASTEXITCODE -ne 0) { throw "Download failed: $Url" }
    $actual = if ($Algorithm -eq "SHA1") { Get-Sha1 $Destination } else { Get-Sha256 $Destination }
    if ($actual -ne $ExpectedHash) {
        Remove-Item -LiteralPath $Destination -Force -ErrorAction SilentlyContinue
        throw "$Algorithm verification failed for $(Split-Path -Leaf $Destination): expected $ExpectedHash, got $actual"
    }
}

if (-not $AcceptAndroidLicenses) {
    throw "Android SDK licenses have not been accepted. Review the licenses in the publishing panel and enable 'I accept the Android SDK licenses' before installing."
}
if (-not (Get-Command curl.exe -ErrorAction SilentlyContinue)) {
    throw "curl.exe is required to download the Android build tools."
}
if (-not (Get-Command rustup -ErrorAction SilentlyContinue) -or
    -not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "Rustup and Cargo must be available on PATH."
}

$InstallRoot = [IO.Path]::GetFullPath($InstallRoot)
$sdkRoot = Join-Path $InstallRoot "sdk"
$downloads = Join-Path $InstallRoot "downloads"
$jdkHome = Join-Path $InstallRoot "jdk-17"
$gradleHome = Join-Path $InstallRoot "gradle-8.9"
[IO.Directory]::CreateDirectory($downloads) | Out-Null
[IO.Directory]::CreateDirectory($InstallRoot) | Out-Null

$jdkArchive = Join-Path $downloads "temurin-17.zip"
$gradleArchive = Join-Path $downloads "gradle-8.9-bin.zip"
$toolsArchive = Join-Path $downloads "android-commandlinetools.zip"
Download-Verified $JdkUrl $jdkArchive $JdkSha256 "SHA256"
Download-Verified $GradleUrl $gradleArchive $GradleSha256 "SHA256"
Download-Verified $CommandLineToolsUrl $toolsArchive $CommandLineToolsSha1 "SHA1"

if (-not (Test-Path -LiteralPath (Join-Path $jdkHome "bin\java.exe") -PathType Leaf)) {
    Write-Host "==> Extracting Temurin JDK 17"
    $jdkStage = Join-Path $InstallRoot (".jdk-stage-" + [Guid]::NewGuid().ToString("N"))
    try {
        Expand-Archive -LiteralPath $jdkArchive -DestinationPath $jdkStage -Force
        $jdkTop = Get-ChildItem -LiteralPath $jdkStage -Directory | Select-Object -First 1
        if (-not $jdkTop -or -not (Test-Path -LiteralPath (Join-Path $jdkTop.FullName "bin\java.exe"))) {
            throw "Downloaded JDK archive has an unexpected layout."
        }
        if (Test-Path -LiteralPath $jdkHome) { Remove-Item -LiteralPath $jdkHome -Recurse -Force }
        Move-Item -LiteralPath $jdkTop.FullName -Destination $jdkHome
    } finally {
        if (Test-Path -LiteralPath $jdkStage) { Remove-Item -LiteralPath $jdkStage -Recurse -Force }
    }
}

if (-not (Test-Path -LiteralPath (Join-Path $gradleHome "bin\gradle.bat") -PathType Leaf)) {
    Write-Host "==> Extracting Gradle 8.9"
    $gradleStage = Join-Path $InstallRoot (".gradle-stage-" + [Guid]::NewGuid().ToString("N"))
    try {
        Expand-Archive -LiteralPath $gradleArchive -DestinationPath $gradleStage -Force
        $gradleTop = Join-Path $gradleStage "gradle-8.9"
        if (-not (Test-Path -LiteralPath (Join-Path $gradleTop "bin\gradle.bat"))) {
            throw "Downloaded Gradle archive has an unexpected layout."
        }
        if (Test-Path -LiteralPath $gradleHome) { Remove-Item -LiteralPath $gradleHome -Recurse -Force }
        Move-Item -LiteralPath $gradleTop -Destination $gradleHome
    } finally {
        if (Test-Path -LiteralPath $gradleStage) { Remove-Item -LiteralPath $gradleStage -Recurse -Force }
    }
}

$sdkManager = Join-Path $sdkRoot "cmdline-tools\latest\bin\sdkmanager.bat"
if (-not (Test-Path -LiteralPath $sdkManager -PathType Leaf)) {
    Write-Host "==> Installing Android command-line tools"
    $sdkStage = Join-Path $InstallRoot (".sdk-tools-stage-" + [Guid]::NewGuid().ToString("N"))
    $cmdlineRoot = Join-Path $sdkRoot "cmdline-tools"
    try {
        Expand-Archive -LiteralPath $toolsArchive -DestinationPath $sdkStage -Force
        $unpacked = Join-Path $sdkStage "cmdline-tools"
        if (-not (Test-Path -LiteralPath (Join-Path $unpacked "bin\sdkmanager.bat"))) {
            throw "Downloaded Android command-line tools archive has an unexpected layout."
        }
        [IO.Directory]::CreateDirectory($cmdlineRoot) | Out-Null
        $latest = Join-Path $cmdlineRoot "latest"
        if (Test-Path -LiteralPath $latest) { Remove-Item -LiteralPath $latest -Recurse -Force }
        Move-Item -LiteralPath $unpacked -Destination $latest
    } finally {
        if (Test-Path -LiteralPath $sdkStage) { Remove-Item -LiteralPath $sdkStage -Recurse -Force }
    }
}

$env:JAVA_HOME = $jdkHome
$env:PATH = (Join-Path $jdkHome "bin") + ";" + (Join-Path $gradleHome "bin") + ";" + (Join-Path $env:USERPROFILE ".cargo\bin") + ";" + $env:PATH
$env:ANDROID_HOME = $sdkRoot
$env:ANDROID_SDK_ROOT = $sdkRoot
$java = Join-Path $jdkHome "bin\java.exe"
& $java -version
if ($LASTEXITCODE -ne 0) { throw "Temurin JDK 17 failed to start." }

Write-Host "==> Accepting Android SDK licenses (user consent was given in the editor)"
$licenseAnswers = (1..64 | ForEach-Object { "y" }) -join "`n"
$licenseAnswers | & $sdkManager "--sdk_root=$sdkRoot" --licenses
if ($LASTEXITCODE -ne 0) { throw "Android SDK license setup failed." }

Write-Host "==> Installing Android SDK platform, build tools, platform tools, and NDK $NdkVersion"
& $sdkManager "--sdk_root=$sdkRoot" "platform-tools" "platforms;android-35" "build-tools;35.0.0" "ndk;$NdkVersion"
if ($LASTEXITCODE -ne 0) { throw "Android SDK package installation failed." }

$ndkRoot = Join-Path $sdkRoot "ndk\$NdkVersion"
if (-not (Test-Path -LiteralPath (Join-Path $ndkRoot "source.properties") -PathType Leaf)) {
    throw "Android NDK $NdkVersion was not installed successfully."
}
$env:ANDROID_NDK_HOME = $ndkRoot
$env:ANDROID_NDK_ROOT = $ndkRoot

Write-Host "==> Installing Rust Android target"
$engineRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\engine")).Path
Push-Location $engineRoot
try {
    & rustup target add aarch64-linux-android
    if ($LASTEXITCODE -ne 0) { throw "Rust Android target installation failed." }
} finally {
    Pop-Location
}
if (-not (Get-Command cargo-ndk -ErrorAction SilentlyContinue)) {
    Write-Host "==> Installing cargo-ndk"
    & cargo install cargo-ndk --locked
    if ($LASTEXITCODE -ne 0) { throw "cargo-ndk installation failed." }
}

Write-Host "Android toolchain ready:"
Write-Host "  SDK=$sdkRoot"
Write-Host "  NDK=$ndkRoot"
Write-Host "  JDK=$jdkHome"
Write-Host "  Gradle=$(Join-Path $gradleHome 'bin\gradle.bat')"
