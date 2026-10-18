[CmdletBinding()]
param(
    # 输出根目录（默认 artifacts\packages，已被 .gitignore 忽略）。
    [string]$OutputRoot = "",
    # 包名后缀标签（默认 win64）。
    [string]$Label = "win64",
    # 是否连同样例工程一起打包（默认带上：跨机验收需要"打开就能跑"的东西）。
    [switch]$SkipSamples,
    # 是否复用已有的 release 二进制（不重新 cargo build）。
    [switch]$SkipBuild,
    # 打完包后是否再压一个 .zip（便于拷到其它 Windows 主机）。
    [switch]$Zip
)

$ErrorActionPreference = "Stop"
$workspaceRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$engineRoot = Join-Path $workspaceRoot "engine"
if ([string]::IsNullOrWhiteSpace($OutputRoot)) {
    $OutputRoot = Join-Path $workspaceRoot "artifacts\packages"
}

# 把文本的行尾统一成 CRLF。
#
# 本脚本自身是 LF 换行，here-string 会原样带出 LF。Windows 上的 .bat 必须是 CRLF：
# cmd.exe 按 CR 找行尾，LF-only 的 bat 在遇到多字节（GBK）注释行时会把行切碎，
# 用户实际看到的是 `'_api: 2D' 不是内部或外部命令`——`game_api：2D` 的残片被当命令执行了。
# .txt/.manifest 也一并转成 CRLF：记事本等老工具对 LF-only 的显示仍然不一致。
function ConvertTo-Crlf {
    param([string]$Text)
    return ($Text -replace "`r`n", "`n") -replace "`n", "`r`n"
}

# ---------------------------------------------------------------------------
# 版本与构建信息
# ---------------------------------------------------------------------------
$cargoToml = Join-Path $engineRoot "Cargo.toml"
$version = (Select-String -Path $cargoToml -Pattern '^version\s*=\s*"(.+)"' | Select-Object -First 1)
$version = if ($version) { $version.Matches[0].Groups[1].Value } else { "0.0.0" }
$commit = (& git -C $workspaceRoot rev-parse --short HEAD 2>$null)
$commit = if ($LASTEXITCODE -eq 0 -and $commit) { "$commit".Trim() } else { "unknown" }
$dirty = (& git -C $workspaceRoot status --porcelain 2>$null | Measure-Object).Count
$toolchain = (& rustc --version 2>$null)
$buildTime = (Get-Date -Format "yyyy-MM-dd HH:mm:ss")
$packageName = "RustLua-Engine-$version-$Label"
$packageRoot = Join-Path $OutputRoot $packageName

Write-Host "==> 绿色版引擎包：$packageName" -ForegroundColor Cyan
Write-Host "    version=$version commit=$commit dirty_files=$dirty"
Write-Host "    toolchain=$toolchain"
Write-Host "    输出：$packageRoot"

# ---------------------------------------------------------------------------
# 1) 构建 release 二进制（编辑器 / 运行时 / 资源编译器）
# ---------------------------------------------------------------------------
if (-not $SkipBuild) {
    Write-Host "`n==> cargo build --release（editor / game / assetc）" -ForegroundColor Cyan
    Push-Location $engineRoot
    try {
        & cargo build --release -p editor -p game -p assetc
        if ($LASTEXITCODE -ne 0) { throw "cargo build --release failed (exit $LASTEXITCODE)" }
    }
    finally {
        Pop-Location
    }
}

$binaries = @("editor.exe", "game.exe", "assetc.exe")
foreach ($binary in $binaries) {
    $path = Join-Path $engineRoot "target\release\$binary"
    if (-not (Test-Path -LiteralPath $path)) {
        throw "release 二进制缺失：$path（先跑 scripts\build\package-engine.ps1，不要加 -SkipBuild）"
    }
}

# ---------------------------------------------------------------------------
# 2) 组装包（staging → 原子替换：失败不留半份包）
# ---------------------------------------------------------------------------
$staging = Join-Path $OutputRoot ("_staging-" + [guid]::NewGuid().ToString("N"))
if (Test-Path -LiteralPath $staging) { Remove-Item -Recurse -Force -LiteralPath $staging }
New-Item -ItemType Directory -Force -Path $staging | Out-Null

try {
    foreach ($binary in $binaries) {
        Copy-Item -LiteralPath (Join-Path $engineRoot "target\release\$binary") -Destination $staging -Force
    }

    # 中文字体：编辑器按「exe 同目录 /fonts/」查找（见 crates/editor/src/util.rs 的候选表），
    # 绿色版里这份字体就是中文能正常显示的前提。
    $fontSource = Join-Path $engineRoot "crates\editor\fonts\NotoSansSC-VF.ttf"
    if (-not (Test-Path -LiteralPath $fontSource)) { throw "编辑器字体缺失：$fontSource" }
    if ((Get-Item -LiteralPath $fontSource).Length -le 0) { throw "编辑器字体为空：$fontSource" }
    $fontDir = Join-Path $staging "fonts"
    New-Item -ItemType Directory -Force -Path $fontDir | Out-Null
    Copy-Item -LiteralPath $fontSource -Destination $fontDir -Force

    # 样例工程：跨机验收需要"打开就能跑"的东西。排除构建产物与本地缓存
    # （builds/ 是发布产物、.cache/ 是资源库与导入缓存、.temp/ 是崩溃恢复快照）。
    if (-not $SkipSamples) {
        $samples = @("game_api", "game_jhyt")
        $excluded = @("builds", ".cache", ".temp", "target")
        foreach ($sample in $samples) {
            $source = Join-Path $workspaceRoot "examples\$sample"
            if (-not (Test-Path -LiteralPath $source)) { continue }
            $target = Join-Path $staging "samples\$sample"
            New-Item -ItemType Directory -Force -Path $target | Out-Null
            Get-ChildItem -LiteralPath $source -Force | Where-Object { $excluded -notcontains $_.Name } |
                ForEach-Object {
                    if ($_.PSIsContainer) {
                        Copy-Item -LiteralPath $_.FullName -Destination $target -Recurse -Force
                    }
                    else {
                        Copy-Item -LiteralPath $_.FullName -Destination $target -Force
                    }
                }
        }
    }

    # -----------------------------------------------------------------------
    # 3) 启动脚本（双击即用）
    #
    # **批处理文件的两条硬约束**（都是真实踩过的坑，改动前请先读）：
    #   ① **行尾必须是 CRLF**。本脚本自身是 LF 换行，here-string 会原样带出 LF；
    #      而 cmd.exe 按 CR 找行尾，LF-only 的 bat 遇到 GBK 多字节内容会把注释行切碎，
    #      用户看到的是 `'_api: 2D' 不是内部或外部命令`（`game_api：2D` 的残片被当命令执行）。
    #      → 一律过 `ConvertTo-Crlf`。
    #   ② **内容保持纯 ASCII**：注释用英文。bat 的编码随控制台代码页而变（GBK/UTF-8 都可能），
    #      写中文就有乱码与误切的风险；中文说明放在 `运行说明.txt`（UTF-8 with BOM，记事本可直接看）。
    #      UTF-8 BOM 更是绝不能加：cmd 会把 BOM 当命令执行。
    # 脚本里的路径一律加引号——包可能被放在带空格的目录里。
    # -----------------------------------------------------------------------
    $asciiBat = [System.Text.ASCIIEncoding]::new()
    $editorBat = @'
@echo off
rem Portable launcher: opens the bundled sample project (game_api: 2D + 3D + UI + Prefab).
rem The Chinese UI needs fonts\NotoSansSC-VF.ttf shipped next to this exe - do not move it away.
rem See 运行说明.txt for the full (Chinese) guide.
setlocal
cd /d "%~dp0"
start "" "%~dp0editor.exe" --project "%~dp0samples\game_api"
endlocal
'@
    # 真正便携：把玩家数据/日志写进包内 userdata\，不落 %APPDATA%。
    $portableBat = @'
@echo off
rem Fully portable mode: player data, logs and crash reports go to .\userdata
rem instead of %APPDATA%. Copy the whole folder to a USB stick and go.
rem See 运行说明.txt for the full (Chinese) guide.
setlocal
cd /d "%~dp0"
set RUSTLUA_USER_DIR=%~dp0userdata
if not exist "%~dp0userdata" mkdir "%~dp0userdata"
start "" "%~dp0editor.exe" --project "%~dp0samples\game_api"
endlocal
'@
    $gameBat = @'
@echo off
rem Runs the bundled game_jhyt sample with game.exe (development layout: reads
rem scenes\scripts\assets live, so a .cache\ folder appears in the project - expected).
rem For a real release package (game.exe + read-only pack\) use scripts\build\package.ps1 from the repo.
rem See 运行说明.txt for the full (Chinese) guide.
setlocal
cd /d "%~dp0"
start "" "%~dp0game.exe" --project "%~dp0samples\game_jhyt"
endlocal
'@
    [System.IO.File]::WriteAllText((Join-Path $staging "启动编辑器.bat"), (ConvertTo-Crlf $editorBat), $asciiBat)
    [System.IO.File]::WriteAllText((Join-Path $staging "便携模式启动.bat"), (ConvertTo-Crlf $portableBat), $asciiBat)
    [System.IO.File]::WriteAllText((Join-Path $staging "运行样板游戏.bat"), (ConvertTo-Crlf $gameBat), $asciiBat)

    # -----------------------------------------------------------------------
    # 4) 文件清单（含 SHA256）——跨机核对"拷过去的是不是同一份"
    # -----------------------------------------------------------------------
    $files = Get-ChildItem -LiteralPath $staging -Recurse -File | Sort-Object FullName
    $manifest = New-Object System.Collections.Generic.List[string]
    $manifest.Add("# $packageName 文件清单（相对路径`t字节`tSHA256）")
    $totalBytes = 0
    foreach ($file in $files) {
        $relative = $file.FullName.Substring($staging.Length).TrimStart('\') -replace '\\', '/'
        $hash = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        $manifest.Add(("{0}`t{1}`t{2}" -f $relative, $file.Length, $hash))
        $totalBytes += $file.Length
    }
    [System.IO.File]::WriteAllText((Join-Path $staging "文件清单.manifest.txt"), (ConvertTo-Crlf ($manifest -join "`n")), [System.Text.UTF8Encoding]::new($false))

    $totalMiB = [math]::Round($totalBytes / 1MB, 1)
    $sampleBytes = 0
    if (Test-Path (Join-Path $staging "samples")) {
        $sampleBytes = (Get-ChildItem (Join-Path $staging "samples") -Recurse -File | Measure-Object -Property Length -Sum).Sum
    }
    $sampleMiB = [math]::Round($sampleBytes / 1MB, 1)
    $fontMiB = [math]::Round((Get-Item -LiteralPath $fontSource).Length / 1MB, 1)

    $buildInfo = @"
RustLua 引擎编辑器 —— 免安装（绿色）版
package_name: $packageName
version: $version
git_commit: $commit (工作区未提交文件数: $dirty)
build_time: $buildTime
toolchain: $toolchain
platform: Windows x64
contents: editor.exe, game.exe, assetc.exe, fonts/NotoSansSC-VF.ttf, samples/, *.bat, 运行说明.txt, 文件清单.manifest.txt
sizes: total=${totalMiB}MiB font=${fontMiB}MiB samples=${sampleMiB}MiB
player_data_default: %APPDATA%\examples （用 便携模式启动.bat 或 RUSTLUA_USER_DIR 可改为包内 userdata\）
cjk_font_lookup: <exe 同目录>/fonts/NotoSansSC-VF.ttf （其次 exe 同目录，再其次构建机源码路径；RUSTLUA_FONT 可覆盖）
"@
    [System.IO.File]::WriteAllText((Join-Path $staging "build-info.txt"), (ConvertTo-Crlf $buildInfo), [System.Text.UTF8Encoding]::new($true))

    $readme = @"
RustLua 引擎编辑器 · 绿色版运行说明
=====================================

一、这是什么
  免安装的 Windows x64 包：解开压缩包、双击 bat 即可运行，不需要 Rust 工具链、
  不需要 IDE、不写注册表。三个可执行文件：
    editor.exe   编辑器（场景/层级/属性/资源/Play 预览/发布）
    game.exe     游戏运行时（配合已发布的 pack\ 目录）
    assetc.exe   资源编译器（导入/校验/字体子集化）

二、怎么跑
  1) 双击「启动编辑器.bat」——打开包内样例工程 samples\game_api。
     也可以自己指定：editor.exe --project <你的工程目录>
  2) 双击「便携模式启动.bat」——同上，但把玩家数据/日志写进包内 userdata\，
     整个文件夹拷走即走（真正的"绿色"；默认形态玩家数据在 %APPDATA%\examples）。
  3) 双击「运行样板游戏.bat」——用包内 game.exe 跑 samples\game_jhyt。
     注意这是**开发态**运行（实时读 scenes\scripts\assets，会在工程目录下建 .cache\），
     目的是验证运行时能起来；samples\game_api 是交互式 API 演示，按 Q 退出。
  4) 命令行自检（不需要显卡、不需要开窗口）：
       editor.exe --gui-smoke            合成输入 GUI 用例（29 个，全过退出码 0）
       editor.exe --perf-baseline        性能基线与不变量（31 个场景）
       editor.exe --project samples\game_api --check    工程审计（schema_errors 应为 0）

三、跨机验收建议按这个顺序看
  1) 体积：先看 文件清单.manifest.txt 与 build-info.txt 里的 total=??MiB；
     再和拷贝前后逐文件核对 SHA256（Get-FileHash -Algorithm SHA256）。
  2) 能不能起来：双击「启动编辑器.bat」。首次运行 Windows 可能弹 SmartScreen
     （可执行文件未签名），选「更多信息 → 仍要运行」。
  3) 中文是否正常：界面若无豆腐块，说明包内 fonts\NotoSansSC-VF.ttf 被正确找到；
     编辑器启动时会在 editor.log 里打印一行 "CJK fallback font = ..."（见下）。
  4) 场景与 Play：层级面板点 main 场景里的实体 → 右侧属性改一个字段 → Ctrl+S →
     按 Play 看内嵌预览是否出画面（Play 需要可用的 GPU）。
  5) 日志位置：默认 %APPDATA%\examples\logs；便携模式在 包根\userdata\logs。
     崩溃报告写在 editor.exe 同目录的 crash.log。

四、已知限制（请先读，避免把预期行为当成故障）
  * 仅 Windows x64。可执行文件未签名，SmartScreen 会拦一次。
  * 需要支持 Vulkan 或 DX12 的 GPU（编辑器用 wgpu）。无独显/驱动过旧时编辑器可能
    起不来或 Play 失败；--gui-smoke / --perf-baseline 不需要 GPU，可用来分离"包坏了"
    还是"这台机器的图形栈不行"。
  * samples\ 是开发态工程（可以随时改），不是发布包；发布游戏请用仓库里的
    scripts\build\package.ps1（产出 game.exe + pack\，且发布包不会往自己目录里写玩家数据）。
  * 本包不含源码与测试工具链；编辑器要读的中文字体是包内 fonts\ 那一份，
    请不要单独挪走它（挪走会变豆腐块，日志里会有 "no CJK fallback font found"）。
  * 这台机器上构建的包没有做代码签名与安装器；分发前请自行确认公司安全策略。
"@
    [System.IO.File]::WriteAllText((Join-Path $staging "运行说明.txt"), (ConvertTo-Crlf $readme), [System.Text.UTF8Encoding]::new($true))

    # -----------------------------------------------------------------------
    # 5) 原子替换最终目录
    # -----------------------------------------------------------------------
    if (Test-Path -LiteralPath $packageRoot) { Remove-Item -Recurse -Force -LiteralPath $packageRoot }
    Move-Item -LiteralPath $staging -Destination $packageRoot
}
catch {
    if (Test-Path -LiteralPath $staging) { Remove-Item -Recurse -Force -LiteralPath $staging }
    throw
}

# ---------------------------------------------------------------------------
# 6) 可选：打包成 zip
# ---------------------------------------------------------------------------
$zipPath = $null
if ($Zip) {
    $zipPath = Join-Path $OutputRoot "$packageName.zip"
    if (Test-Path -LiteralPath $zipPath) { Remove-Item -Force -LiteralPath $zipPath }
    Compress-Archive -Path (Join-Path $packageRoot "*") -DestinationPath $zipPath -CompressionLevel Optimal
    $zipMiB = [math]::Round((Get-Item -LiteralPath $zipPath).Length / 1MB, 1)
    Write-Host ("`n==> zip: {0} ({1} MiB)" -f $zipPath, $zipMiB) -ForegroundColor Cyan
}

# ---------------------------------------------------------------------------
# 7) 汇总
# ---------------------------------------------------------------------------
$summaryBytes = (Get-ChildItem -LiteralPath $packageRoot -Recurse -File | Measure-Object -Property Length -Sum).Sum
Write-Host "`n==> 完成" -ForegroundColor Green
Write-Host ("    目录：{0}" -f $packageRoot)
Write-Host ("    解包体积：{0} MiB（其中 samples {1} MiB / 字体 {2} MiB）" -f `
        [math]::Round($summaryBytes / 1MB, 1), $sampleMiB, $fontMiB)
Get-ChildItem -LiteralPath $packageRoot | Select-Object Mode, @{n = 'Name'; e = { $_.Name } }, @{n = 'MiB'; e = { if ($_.PSIsContainer) { [math]::Round((Get-ChildItem $_.FullName -Recurse -File | Measure-Object -Property Length -Sum).Sum / 1MB, 1) } else { [math]::Round($_.Length / 1MB, 2) } } } | Format-Table -AutoSize
