# -----------------------------------------------------------------------------
# 打包阶段：上下文归一化（工作区/输出根/发布名/平台开关）
# 自 scripts/build/package.ps1 原样迁出（原 L81-L230，2026-11-02 按功能拆分）。
# 执行语义：本文件被入口 dot-source，跑在入口的脚本作用域里——阶段之间共享变量、
#   ErrorActionPreference 与 throw 行为都跟拆分前一致。它不是函数，别当函数调。
#   PSScriptRoot 已统一换成入口设置的 packageScriptRoot（本文件不在 scripts/build/ 下）。
# -----------------------------------------------------------------------------

$ErrorActionPreference = "Stop"
$workspaceRoot = Split-Path -Parent (Split-Path -Parent $packageScriptRoot)
$engineRoot = Join-Path $workspaceRoot "engine"
# 构建产物目录：默认是 `engine\target`（cargo 与本仓 `.cargo/config.toml` 的默认），
# 但只要是 `cargo` 在跑，`CARGO_TARGET_DIR` 就是 cargo 自己认的权威开关。
# **必须跟着它走**：本仓的开发/CI 环境里 `engine\target` 可能是只读的（沙箱、只读挂载、
# 共享缓存），此时构建只能落在别处，而脚本再去固定路径取 `assetc.exe` / `game_web.wasm`
# 就会报"缺产物"——那会把"构建成功但取错路径"伪装成"构建失败"。
$cargoTargetRoot = if ([string]::IsNullOrWhiteSpace($env:CARGO_TARGET_DIR)) {
    Join-Path $engineRoot "target"
} else {
    # 相对路径按 cargo 的口径解析：相对于**执行 cargo 的目录**（脚本里是 $engineRoot）。
    if ([IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) {
        $env:CARGO_TARGET_DIR
    } else {
        Join-Path $engineRoot $env:CARGO_TARGET_DIR
    }
}
$nativeTargetRoot = Join-Path $cargoTargetRoot "release"
$wasmTargetRoot = Join-Path $cargoTargetRoot "wasm32-unknown-unknown\release"
if ([string]::IsNullOrWhiteSpace($ProjectRoot)) {
    # 仓库里唯一的样板工程（jhyt 已迁出仓库，见 E:\Qoder\game_jhyt）。
    $ProjectRoot = Join-Path $workspaceRoot "examples\game_api"
}
$gameRoot = (Resolve-Path -LiteralPath $ProjectRoot).Path
$projectToml = Join-Path $gameRoot "project.toml"
if (-not (Test-Path $projectToml)) { throw "Game project.toml was not found: $projectToml" }
if ([string]::IsNullOrWhiteSpace($PublishName)) {
    # 必须显式 -Encoding UTF8：Windows PowerShell 5.1 默认按 ANSI 解码，project.toml 里的
    # 中文注释会让整行匹配失配——`font` 行（纯 ASCII）也一样读不到，于是静默回退到默认
    # 字体名，发布包与工程声明不一致。
    $nameLine = Get-Content -LiteralPath $projectToml -Encoding UTF8 | Where-Object { $_ -match '^\s*name\s*=\s*"(.+)"\s*$' } | Select-Object -First 1
    if ($nameLine -and $nameLine -match '^\s*name\s*=\s*"(.+)"\s*$') { $PublishName = $Matches[1] }
    if ([string]::IsNullOrWhiteSpace($PublishName)) { $PublishName = Split-Path -Leaf $gameRoot }
}
$invalidNameChars = [IO.Path]::GetInvalidFileNameChars()
foreach ($ch in $invalidNameChars) { $PublishName = $PublishName.Replace([string]$ch, "_") }
$PublishName = $PublishName.Trim()
if ([string]::IsNullOrWhiteSpace($PublishName)) { $PublishName = "game" }
# 微信 appid 归一化：只有微信小游戏有 project.config.json 可落，其它平台给了就明确忽略
# （而不是悄悄写进一个不存在的地方）；空值回落到模板默认的 touristappid（仅本地预览）。
$WechatAppId = $WechatAppId.Trim()
if ($Platform -ne "wechat-minigame") {
    if ($WechatAppId) { Write-Warning "==> -WechatAppId 只对 -Platform wechat-minigame 有意义，本次发布（$Platform）忽略该值" }
    $WechatAppId = ""
}
if ([string]::IsNullOrWhiteSpace($WechatAppId)) { $WechatAppId = "touristappid" }
# M31/P3：玩家数据目录（存档 + 玩家设置）。与引擎 `rustlua-infra` 的 user_dir 规则一致
# （engine/crates/rustlua-infra/src/user_dir.rs）：
# Windows 为 `%APPDATA%\<publish_name>`，其它平台取各自的数据目录；`RUSTLUA_USER_DIR` 可覆盖。
# 这里只用于**文档与自检断言**（发布包目录里不写任何玩家数据）。
$PlayerDataDir = if ($env:RUSTLUA_USER_DIR) {
    $env:RUSTLUA_USER_DIR
} elseif ($env:APPDATA) {
    Join-Path $env:APPDATA $PublishName
} else {
    Join-Path $env:USERPROFILE (".local\share\" + $PublishName)
}
# 发布根布局（2026-09-28 起）：产物直接落在 `builds\<平台>\`，不再按 publish_name 建子目录；
# $PublishName 仍用于玩家数据目录、发布名与各配置文件的标识字段（见下）。
$distRoot = Join-Path $gameRoot ("builds\" + $Platform)
# 微信分包：主包首屏目录叫 pack/，默认分包根叫 pack-sub/（wasm 另有 pack-wasm/）。
# Web/桌面没有微信的"分包根"概念，资源仍统一落在 pack/；主包首屏切分只用于微信。
$packDirName = if ($Platform -eq "wechat-minigame") { "pack-sub" } else { "pack" }
$mainPackDirName = if ($Platform -eq "wechat-minigame") { "pack" } else { "mainpack" }
$packWasmDirName = "pack-wasm"
$packRoot = Join-Path $distRoot $packDirName
$mainPackRoot = Join-Path $distRoot $mainPackDirName
$packWasmRoot = Join-Path $distRoot $packWasmDirName
# M20-03：原子发布。所有内容先写进 staging，全部成功后再原子替换最终目录；
# 失败时最终目录保持旧版本不变（不会出现半更新状态）。
. (Join-Path $packageScriptRoot "release-atomic.ps1")
# web 平台的两个外部工具链：zig（编译 vendored Lua 的 C 源）与 wasm-bindgen CLI
# （生成 JS 胶水，版本必须与 Cargo.lock 的 crate 精确一致）。两者的解析顺序、按需安装
# 与"为什么不能写死本机路径"的说明都在各自的脚本里，这里只消费。
. (Join-Path $packageScriptRoot "zig-toolchain.ps1")
. (Join-Path $packageScriptRoot "wasm-bindgen.ps1")
. (Join-Path $packageScriptRoot "minify-toolchain.ps1")
# 工程图（`icon_image` / `splash_image`）的唯一键扫描口径：品牌图与 exe 图标两条路共用一份。
. (Join-Path $packageScriptRoot "..\lib\project-image-key.ps1")

# 打包期的图标/品牌图**不再依赖 Python**（2026-10-17 改）：旧实现是
# `package-branding.py` + `package-icon.py`，需要宿主上有 python 与 Pillow，而 PATH 上同时
# 存在「Microsoft Store 应用执行别名」(%LOCALAPPDATA%\Microsoft\WindowsApps\python.exe) 与
# 真实安装时 `Get-Command python -CommandType Application` 返回的是**数组**，`& $cmd.Source`
# 会拼成 `& "<store 别名>" "<真 python>" -B …`，PS 5.1 把第二个元素当参数 ⇒
# "The term 'C:\…\WindowsApps\python.exe C:\…\python.exe' is not recognized"（实测，
# PC 与 webgl 两条发布全被这一句拦下，而错误离真正原因隔了 500 行）；Pillow 缺失还会在
# 工程声明了 `splash_image`/`icon_image` 时把两条 web 发布一起拦下。现在两条路都是纯
# PowerShell（只依赖 .NET 与 kernel32），调用契约不变：成功 0 + 一行 `branding_ok`/`project_icon_*`，
# 失败 1 + 一行原因。

# webgl 与微信小游戏共用同一份 game_web 原始 WASM 构建与静态产物管线；差别在入口文件、资源
# 配额、平台配置，以及平台专属的发行后处理：胶水按平台各生成一份（--target web / no-modules），
# 微信侧最终 WASM 另经剥 Global / 剥 Tag / noextern 降级——两平台发行的 _bg.wasm 不是逐字节
# 相同的文件，也不应要求相同（口径见 docs/65 §3）。
$isWebPlatform = $Platform -in @("webgl", "wechat-minigame")

# 三种形态都只对微信小游戏有意义（只有微信有"主包 / 分包 / 远端资源"这套划分），
# 且互斥：subpackage 资源全在包内，remote 资源全在远端，full 谁都不用。
# 混着给参数不会"更保险"，只会让两套资源来源互相掩盖失败（本地读到了就永远发现不了远端是坏的）。
if ($Platform -ne "wechat-minigame" -and $AssetPack -ne "full") {
    throw "-AssetPack $AssetPack 只支持 -Platform wechat-minigame（当前: $Platform）"
}
if ($AssetPack -eq "subpackage") {
    if (-not [string]::IsNullOrWhiteSpace($RemotePackBaseUrl)) {
        throw "-AssetPack subpackage 与 -RemotePackBaseUrl 互斥：分包形态的资源全部随包分发，配上远端回落会让'包内读到了'掩盖'分包装载失败'。"
    }
    if (-not [string]::IsNullOrWhiteSpace($CdnPackDir)) {
        throw "-AssetPack subpackage 与 -CdnPackDir 互斥：分包形态没有独立资源包可导出（资源在代码包里随包上传）。"
    }
}
# 瘦首包必须配远端基址：否则被裁掉的资源没有任何来源，产出的包是"能发布但跑不起来"。
if ($AssetPack -eq "remote" -and [string]::IsNullOrWhiteSpace($RemotePackBaseUrl)) {
    throw "-AssetPack remote 必须同时给 -RemotePackBaseUrl：pack/ 只留骨架，重资源只能从远端取。"
}
# -WasmInSubpackage 只对 remote 有多余意义：subpackage 形态本来就搬内核，full 形态搬了等于没搬
# （pack/ 与主包同在 launcher 下，主包字节数一个都不少）。
if ($WasmInSubpackage -and -not ($Platform -eq "wechat-minigame" -and $AssetPack -eq "remote")) {
    throw ("-WasmInSubpackage 只支持 -Platform wechat-minigame -AssetPack remote（当前: {0}/{1}）" -f $Platform, $AssetPack)
}
if (-not [string]::IsNullOrWhiteSpace($CdnPackDir) -and $Platform -ne "wechat-minigame") {
    throw "-CdnPackDir 只支持 -Platform wechat-minigame（当前: $Platform）"
}
# 资源包导出目录在 $finalRoot 确定后再算（见下），这里只做合法性检查。

# 字体解析必须早于 index.html 生成与运行器复制：index.html 需要真实字体路径，
# 且小游戏平台只随包分发 pack/ 里的子集字体（不复制完整字体）。
$fontLine = Get-Content -LiteralPath $projectToml -Encoding UTF8 | Where-Object { $_ -match '^\s*font\s*=\s*"(.+)"\s*$' } | Select-Object -First 1
$fontRelative = if ($fontLine -and $fontLine -match '^\s*font\s*=\s*"(.+)"\s*$') { $Matches[1] } else { "assets/fonts/NotoSansSC-VF.ttf" }
$fontSource = Join-Path $gameRoot ($fontRelative -replace '/', '\\')
if (-not (Test-Path $fontSource)) { throw "Game font resource was not found: $fontSource" }
if ((Get-Item $fontSource).Length -le 0) { throw "Game font resource is empty: $fontSource" }
$fontName = Split-Path -Leaf $fontSource
$publishNameJson = $PublishName | ConvertTo-Json -Compress

# 容器形态下这两条限制都松开了（docs/67 批 F）：压缩与解密都在 wasm/Rust 侧做，
# JS 运行时不需要读明文。所以只有**目录形态**才拒绝——那时 JS 确实读不了 gzip/AEAD 的文件。
if ($isWebPlatform -and $BundleMode -eq "none" -and ($Encrypt -or $Compress)) {
    throw "Platform '$Platform' cannot use -Encrypt/-Compress with -BundleMode none: the web runtime reads plaintext pack/ only. Use the default -BundleMode grouped (block-level AEAD + zstd are handled in wasm)."
}
# -MultiSub 只对微信 subpackage 形态有意义：其它形态没有 `subpackages` 概念，
# 也没有 `mainpack/` 兄弟目录可拆。
if ($MultiSub -and -not ($Platform -eq "wechat-minigame" -and $AssetPack -eq "subpackage")) {
    throw "-MultiSub 只支持 -Platform wechat-minigame -AssetPack subpackage（当前: $Platform/$AssetPack）"
}

# M31/P3：发布版运行器是 Windows GUI 子系统程序，`& exe args` 不等待也不捕获输出
# （`$LASTEXITCODE` 会保留前一条命令的值 ⇒ 自检空跑）。统一用 Start-Process -Wait +
# 重定向取回真实退出码与输出。返回 `[pscustomobject]@{ ExitCode; Output }`。
