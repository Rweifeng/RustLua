# verify-webgl-bundle.ps1（M31/P3）：验证**真实发布出来的 WebGL 包**能否在浏览器里启动
# 并跑通一次场景切换（防回归门）。
#
# 与 verify-webgl.ps1 的分工：
#   * verify-webgl.ps1  —— 平台契约层（资源获取/解析、WebGL2 能力、存储降级、缺失资源诊断），
#                          用一张契约检查页跑，不依赖 wasm 模块；
#   * 本脚本            —— 产物层：直接起静态服务器 + 无头浏览器打开 `builds/webgl/index.html`，
#                          断言 wasm 模块加载、页面进入 Lua 执行（`data-rustlua-boot` 标记），
#                          并且默认让首帧重新走一遍 applySceneSwitch（`?rustlua-scene-verify=1`），
#                          断言 `data-rustlua-scene-verify=done`。这条**场景切换门**是上次
#                          wasm panic=abort 与宿主闭包 raise 交叉崩溃（boot=ok 却一切就死）
#                          的回归哨兵；`-NoSceneVerify` 会把它关掉（只验 boot）。
#
# 用法：
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\verify-webgl-bundle.ps1 `
#       [-BundleRoot examples\game_jhyt\builds\webgl] [-Port 8137] [-TimeoutSeconds 90]
#   # 关掉场景切换门（boot-only，快但不防切换崩溃回归）：
#   powershell ... -File scripts\verify-webgl-bundle.ps1 -NoSceneVerify
#   # L3 行为契约对拍（PC transcript vs 活体页面跑同一份 lua_api_contract.lua，多开一次浏览器）：
#   powershell ... -File scripts\verify-webgl-bundle.ps1 -ApiContract
#
# 省略 -BundleRoot 时自动发现（本机唯一的合格 webgl 产物，见 scripts\bundle-discovery.ps1）：
# 默认值不再写死某个游戏工程，任何工程发布后都能直接跑。
#
# 退出码：0 = 启动成功且场景切换门通过（`data-rustlua-boot=ok` + `data-rustlua-scene-verify=done`）；
#         1 = 启动或场景切换失败（含缺失宿主 API 与 scene-verify 明细）；
#         2 = 环境受限（无 node/浏览器）；3 = 未找到产物（需 -BundleRoot 显式指定）。

[CmdletBinding()]
param(
    [string]$BundleRoot = "",
    [int]$Port = 8137,
    [int]$TimeoutSeconds = 90,
    # P0-①：URL 驱动"完整剧本"（`?rustlua-drive=auto`）：标题按确认 → 进玩法地图 → 按住方向键
    # 移动。断言 `data-rustlua-drive=done` 且位移非零（主角真的在浏览器里被驱动着走了）。
    # 与 -NoSceneVerify 独立：剧本是"跑游戏内容"，scene-verify 是"跑运行时切换机制"。
    [switch]$Drive,
    # P0-②：URL 驱动"完整玩法剧本"（`?rustlua-drive=jhyt`）：标题确认 → 新游戏(manor) →
    # 移动 → 背包/任务面板开合 → 山庄→云泽村真实场景切换 → 回山庄，里程碑逐段落定
    # （M1 新游戏 / M2 移动 / M3 背包 / M4 任务 / M5 场景切换 / M6 返回）。
    # 断言 `data-rustlua-drive=done` 且 detail 含全部里程碑。
    # 与 -Drive 互斥（两者都写 data-rustlua-drive）；带本开关时自动关闭 scene-verify
    # （首帧重载当前场景会干扰剧本的标题确认阶段）。
    [switch]$DriveJhyt,
    # 关掉场景切换防回归门（默认 ON）：仅当产物里没有可 reload 的启动场景时才关。
    [switch]$NoSceneVerify,
    # L3 行为契约对拍（默认 OFF，`verify.ps1 -Web` 打开它）：boot 落定后把仓库里的
    # `lua_api_contract.lua` 经 CDP 注入**这个活体页面**（补丁层已生效的 Lua VM）跑一遍，
    # 与桌面 transcript 基线逐行对拍。契约只碰纯状态类 API（clock/calendar/item/event/…），
    # 代价是这次无头实例里的游戏时间/背包定义会被改——那是**一次性浏览器**，不影响任何产物。
    # 契约文本不进发布包：它是验证资产，一帧都不参与游戏，却要占微信 4 MB 主包配额。
    [switch]$ApiContract,
    # 调用方（`verify.ps1 -Web`）用它拿退出码：`Start-Process -PassThru -RedirectStandardOutput`
    # 返回的进程对象上 `ExitCode` 是 **$null**（本轮实测），而 `[int]$null` 会静默变成 0——
    # "包起不来"于是被当成"通过"。因此由子进程自己把结果写进这个文件。
    [string]$StatusFile = ""
)

$ErrorActionPreference = "Stop"
# 所有退出点都走它：写状态文件（若有）再退出，退出码对调用方始终可见。
function Exit-With([int]$Code) {
    if (-not [string]::IsNullOrWhiteSpace($StatusFile)) {
        "exit=$Code" | Set-Content -LiteralPath $StatusFile -Encoding ASCII
    }
    exit $Code
}

# 未捕获的终止错误（本脚本里大量 `throw`）也必须是"失败且**可见**"：调用方只看状态文件，
# 没有 trap 时它只剩"没写状态文件"这条模糊线索（真实原因还压在 stderr 里，不该靠人肉去捞）。
trap {
    Write-Host "[RED] webgl bundle verification aborted: $($_.Exception.Message) @line $($_.InvocationInfo.ScriptLineNumber)"
    Exit-With 1
}
$workspaceRoot = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot "bundle-discovery.ps1")
if ([string]::IsNullOrWhiteSpace($BundleRoot)) {
    # 不写死游戏工程：自动发现本机唯一的合格 webgl 产物（诊断由 bundle-discovery 打印）。
    $BundleRoot = Resolve-GameBundle -Platform "webgl"
    if (-not $BundleRoot) { exit 3 }
}
if (-not (Test-Path -LiteralPath $BundleRoot)) {
    throw "WebGL bundle was not found: $BundleRoot (先跑 scripts\package.ps1 -Platform webgl)"
}
$bundle = (Resolve-Path -LiteralPath $BundleRoot).Path

$logRoot = Join-Path $workspaceRoot "artifacts\logs\webgl"
New-Item -ItemType Directory -Force -Path $logRoot | Out-Null
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$logPath = Join-Path $logRoot "bundle-$stamp.log"

function Write-Log([string]$Message, [string]$Color = "Gray") {
    # 本仓部分运行环境的 `Write-Host` 包装器不支持 `-Color`（会把参数并进文本），
    # 因此这里只把 RED/YELLOW 作为前缀写进纯文本：日志与证据必须是可比对的无装饰文本。
    if ($Color -eq "Yellow" -or $Color -eq "Red") {
        Write-Host ("[{0}] {1}" -f $Color.ToUpperInvariant(), $Message)
    } else {
        Write-Host $Message
    }
    Add-Content -Path $logPath -Value $Message -Encoding UTF8
}

Write-Log "==> WebGL bundle verification (bundle=$bundle)" Cyan

# ---------- 1. 产物完整性 ----------
$required = @(
    "index.html",
    "web-runtime.js",
    "pkg\game_web.js",
    "pkg\game_web_bg.wasm",
    "pack\manifest.txt",
    "pack\project.toml"
)
foreach ($item in $required) {
    $path = Join-Path $bundle $item
    if (-not (Test-Path -LiteralPath $path)) { throw "WebGL bundle is missing: $item" }
    if ((Get-Item -LiteralPath $path).Length -le 0) { throw "WebGL bundle entry is empty: $item" }
}
# WP10 之后 webgl 不随包带 fonts/（`__FONT_PATH__` 为空，走浏览器系统字体渲染中文）。
# 若哪天恢复随包子集字体，再在这里加回校验。

# wasm 魔数（\0asm）：产物必须是真 wasm，不是占位文件。
$wasmPath = Join-Path $bundle "pkg\game_web_bg.wasm"
if (-not (Test-Path -LiteralPath $wasmPath)) { throw "wasm module missing: $wasmPath" }
$wasmBytes = [System.IO.File]::ReadAllBytes($wasmPath)
$magic = -join ($wasmBytes[0..3] | ForEach-Object { [char]$_ })
if ($magic -ne "`0asm") { throw "pkg/game_web_bg.wasm is not a wasm module (magic=$([int[]]$wasmBytes[0..3] -join ','))" }
Write-Log "wasm module ok: $([math]::Round($wasmBytes.Length / 1024, 1)) KB, magic=$magic"

# ---------- 2. 浏览器与静态服务器 ----------
$browser = $null
foreach ($candidate in @(
        (Join-Path ${env:ProgramFiles(x86)} "Microsoft\Edge\Application\msedge.exe"),
        (Join-Path $env:ProgramFiles "Microsoft\Edge\Application\msedge.exe"),
        (Join-Path $env:ProgramFiles "Google\Chrome\Application\chrome.exe"),
        (Join-Path ${env:ProgramFiles(x86)} "Google\Chrome\Application\chrome.exe")
    )) {
    if ($candidate -and (Test-Path -LiteralPath $candidate)) { $browser = $candidate; break }
}
if (-not $browser) {
    Write-Log "no supported browser found (Edge/Chrome); bundle boot check cannot run here" Yellow
    Exit-With 2
}
$node = Get-Command node -ErrorAction SilentlyContinue
if (-not $node) {
    Write-Log "node.js not found; static server for the boot check is unavailable" Yellow
    Exit-With 2
}

$server = Start-Process -FilePath $node.Source `
    -ArgumentList @((Join-Path $PSScriptRoot "serve-static.js"), $bundle, "$Port") `
    -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput (Join-Path $logRoot "bundle-serve-$stamp.log") `
    -RedirectStandardError (Join-Path $logRoot "bundle-serve-$stamp.log.err")
Start-Sleep -Milliseconds 1200

$queryBits = @()
if ($Drive -and $DriveJhyt) {
    throw "参数互斥：-Drive 与 -DriveJhyt 只能二选一（两者都写 data-rustlua-drive）"
}
if ($Drive) { $queryBits += "rustlua-drive=auto" }
if ($DriveJhyt) {
    $queryBits += "rustlua-drive=jhyt"
    # 剧本驱动走标题确认 → 真实场景切换全流程，首帧重载当前场景（scene-verify）会干扰它。
    $NoSceneVerify = $true
}
if (-not $NoSceneVerify) { $queryBits += "rustlua-scene-verify=1" }
$url = "http://127.0.0.1:$Port/index.html"
if ($queryBits.Count -gt 0) { $url += "?" + ($queryBits -join "&") }
$domPath = Join-Path $logRoot "bundle-dom-$stamp.html"
$errPath = Join-Path $logRoot "bundle-dom-$stamp.err"

function Invoke-HeadlessDom([string]$HeadlessFlag, [string]$ModeOverride = "", [string]$OutFile = "") {
    # CDP 轮询替代 `--dump-dom`：dump-dom 在页面 load 事件后**立即** dump，而本游戏在
    # module 顶层 await 里加载 wasm + boot（`rustluaRunLuaGame`），dump 常常早于 boot
    # （body 还是 data-rustlua-boot="pending"）。用 `--remote-debugging-port` + Node 轮询
    # 真实时钟，等状态真正落定再 dump——常规 bundle 等 boot，scene-verify 等 scene 落定，
    # 剧本门等 drive 落定。时长由 $TimeoutSeconds 控制，超时判红（与 dump-dom 同语义）。
    $cdpScript = Join-Path $PSScriptRoot "webgl-cdp-wait.js"
    if (-not (Test-Path -LiteralPath $cdpScript)) {
        throw "CDP wait script missing: $cdpScript"
    }
    $cdpPort = 9200 + (Get-Random -Maximum 700)
    $profileDir = Join-Path $logRoot "cdp-profile-$stamp-$([guid]::NewGuid().ToString('N').Substring(0, 8))"
    $waitMode = if ($ModeOverride) { $ModeOverride } elseif ($DriveJhyt) { "drive" } elseif (-not $NoSceneVerify) { "scene" } else { "boot" }
    # contract 模式写的是**对拍报告**而不是 DOM，所以输出目标可由调用方指定。
    $outTarget = if ([string]::IsNullOrWhiteSpace($OutFile)) { $domPath } else { $OutFile }
    $arguments = @(
        $HeadlessFlag,
        "--disable-gpu-sandbox",
        "--no-sandbox",
        "--disable-extensions",
        # 转发 console.*（含 wasm panic hook 的 console.error）到 stderr（CONSOLE 前缀），
        # 排查 wasm trap（如场景切换 unreachable）时能从 bundle-dom-*.err 拿到 Rust panic 详情。
        "--enable-logging=stderr",
        "--v=0",
        # 关键：无头下**不要**加 `--virtual-time-budget`——虚拟时间预算让 rAF 在页面稳定后
        # 停止调度（实测 90s 只跑 63 帧、剧本永远停在某个阶段）。用真实时钟跑，配合下面
        # 这个标志强制每帧走完整合成管线，rAF 保持 ~55fps——完整玩法剧本（drive=jhyt）
        # 才能跑完并落定 done（实测 30s 推进 1635 帧、3.6 秒剧本一次全通）。
        "--run-all-compositor-stages-before-draw",
        # 音频后端的验收读数要能显示"真的在放"，而不是"被浏览器拦住"。无头实例没有用户
        # 手势，Chrome 默认自动播放策略会把 HTMLAudioElement.play() 直接 reject（实测不加
        # 这条时 `audio: kind=browser created=1 played=0 gated=1`——那不是回归，是策略）。
        # 这里放行策略，门验的是**我们这侧**的链路（src 解析 → 下载 → 解码 → currentTime
        # 前进）；策略本身的行为由 runtime 的手势补播路径负责，浏览器真机人工试听。
        "--autoplay-policy=no-user-gesture-required",
        "--remote-debugging-port=$cdpPort",
        "--user-data-dir=$profileDir",
        $url
    )
    $process = Start-Process -FilePath $browser -ArgumentList $arguments -PassThru -NoNewWindow `
        -RedirectStandardOutput $domPath -RedirectStandardError $errPath
    # **必须接住** node 的 stdout：PS 里函数内未被捕获的输出会并进返回值，于是
    # Invoke-HeadlessDom 返回的是「[cdp] 那几行 + 文件正文」的**数组**。而 `-match` 作用在
    # 数组上返回的是"匹配到的元素数组"、**不填 $Matches**（判空时却照样真值），于是
    # `$contractOutput -match 'mismatches=(\d+)'` 之后 `$Matches[1]` 还是上一条无关 -match
    # 的捕获组——实测红字里出现过 `verdict=failed(scene=main entities=17) cases=0`：
    # 红绿方向侥幸没错，但 mismatches 与 cases 两个数都是假的（$dom 那条同理，只是它被
    # 下面那句 `[string]$dom` 顺手压平了才没出事）。node 那几行本来就只写给控制台，
    # 真正的结论在 $outTarget 报告文件里，所以这里接住不转印。
    $cdpEcho = @(& node $cdpScript $cdpPort $outTarget $waitMode $TimeoutSeconds)
    $cdpExit = $LASTEXITCODE
    foreach ($cdpLine in $cdpEcho) { Write-Log "cdp: $cdpLine" }
    # 收尾：杀本脚本启动的浏览器**全部进程** + 清理 CDP profile（无头实例不会自己退出）。
    # 按唯一 user-data-dir 路径匹配命令行，精确杀本实例的进程树——绝不按进程名一网打尽
    # （开发机上还有用户自己的浏览器）；taskkill /T 对 Edge 的启动器/子进程不可靠。
    Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -in @("msedge.exe", "chrome.exe") -and $_.CommandLine -like "*$profileDir*" } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    Remove-Item -Recurse -Force $profileDir -ErrorAction SilentlyContinue
    if ($cdpExit -eq 2) {
        Write-Log "CDP unavailable (chrome did not expose debug endpoint); no DOM dump" Yellow
    }
    if (Test-Path -LiteralPath $outTarget) { return Get-Content -LiteralPath $outTarget -Raw } else { return "" }
}

$dom = Invoke-HeadlessDom "--headless"
if (-not $dom -or $dom -notmatch "data-rustlua-boot") {
    $dom = Invoke-HeadlessDom "--headless=new"
}
# L3 行为契约（-ApiContract）：boot 那次验完了，这里**另开一次**无头实例专跑契约——
# 契约要 CDP 在同一个页面上下文里 `lua_do_string` + `lua_call_global`，浏览器句柄跨不过去。
# 必须在静态服务器关掉之前跑；boot 都没起来时不必浪费第二次 90s（那次红已经说明问题）。
$contractReportPath = Join-Path $logRoot "bundle-api-contract-$stamp.txt"
$contractOutput = ""
if ($ApiContract -and $dom) {
    $contractOutput = Invoke-HeadlessDom "--headless" "contract" $contractReportPath
}
Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue

if (-not $dom) {
    Write-Log "headless browser produced no DOM dump (see $errPath)" Red
    Exit-With 2
}

# ---------- 3. 断言 ----------
# CDP dump 文件为 UTF-8 无 BOM（Node 写入），PS 5.1 的 Get-Content 按 ANSI 读可能引入
# 乱码/空行；且 -Raw 对空文件返回 $null、对含空行文件返回非空串。这里统一强制成
# 单字符串再解析，避免"空数组/空串在 -match 下 $Matches 为 null 再取 [1]"这类抛错。
$dom = [string]$dom
$boot = if ($dom -match 'data-rustlua-boot="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "absent" }
$detail = if ($dom -match 'data-rustlua-boot-detail="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "" }
$statusText = if ($dom -match '<div id="status">([^<]*)</div>' -and $Matches.Count -gt 0) { $Matches[1] } else { "" }
# 缺失宿主 API 明细：`detail` 里是 `… [missing engine host API: a, b, c]`。单独导出一个
# **数值**字段给验收样例读（JSON 里逗号与转义引号都会打乱"按字段切割"的朴素解析）。
$missingNames = @()
if ($detail -match 'missing engine host API:\s*([^\]]*)' -and $Matches.Count -gt 0) {
    $missingNames = @($Matches[1].Split(',') | ForEach-Object { $_.Trim() } | Where-Object { $_ })
}
# 画面自检：`data-rustlua-drawn` 是运行时对 canvas 采样得到的“非背景像素采样点”数量。
# `-1` = 像素不可读（跨域贴图污染），`0` = 启动了但真的什么都没画出来。
$drawnRaw = if ($dom -match 'data-rustlua-drawn="(-?\d+)"' -and $Matches.Count -gt 0) { [int]$Matches[1] } else { $null }
$uiElements = if ($dom -match 'data-rustlua-ui-elements="(\d+)"' -and $Matches.Count -gt 0) { [int]$Matches[1] } else { $null }
# 世界状态自报：`scene=/map=/alive=/player=@x,y/clip`（+ 渲染侧 `drawn=`）。
# 这里只作为**证据**记录，不新增判定：无头浏览器这一次是"打开页面等 15s 后 dump DOM"，
# 停在启动场景是正常的（玩家要按键才进玩法地图）。真正有牙的那一组断言在
# `scripts/wechat-minigame-harness.js --drive-new-game ...` 里——两个 web 平台共用同一份
# `web-runtime.js`，字段含义完全一致，所以这里读到的世界状态可以直接对照它。
$world = if ($dom -match 'data-rustlua-world="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "" }
# 瓦片层自对账：`tilemaps=画/声明` 是运行时每帧写的两个**独立来源**读数（渲染循环实际遍历的
# 层数 / Lua 侧注册表里的层数）。它抓的是"overlay 层整层没进画面"这一类静默差异——山庄的
# FarmGrid 种田层曾被"只取第一张 Tilemap"的老写法丢掉，PC 有、web 无（实机 2026-10-04）。
# 字段缺失（产物是旧运行时、或这条自报被删）同样算红：世界自报都在，读数却少一项，
# 说明门与产物错位，不能降级成"没数据所以放行"。
$tileDrawn = $null
$tileDeclared = $null
if ($world -match 'tilemaps=(\d+)/(\d+)' -and $Matches.Count -gt 1) {
    $tileDrawn = [int]$Matches[1]
    $tileDeclared = [int]$Matches[2]
}
# P0-①：`?rustlua-drive=auto` 的剧本结果。`done` = 确认 → 进玩法地图 → 位移非零全部发生；
# `failed` 时 detail 给出卡在哪一步（场景没切换 / 没有玩家位置 / 位移为 0）。
$driveState = if ($dom -match 'data-rustlua-drive="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "absent" }
$driveStateDetail = if ($dom -match 'data-rustlua-drive-detail="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "" }
# 场景切换防回归门：`?rustlua-scene-verify=1` 让首帧把当前场景重新走一遍 applySceneSwitch
# 全流程（fetch → parse → 重绑 Lua → 重跑 on_create）。done=通过、failed=细节给出错误、
# in-flight=在途未落定（通常说明 headless 虚拟时间预算在切换 promise resolve 前烧完）、
# absent=URL 门没接上或 runtime 未升级。后两种都算失败。
$sceneVerifyState = if ($dom -match 'data-rustlua-scene-verify="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "absent" }
$sceneVerifyDetail = if ($dom -match 'data-rustlua-scene-verify-detail="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "" }
# 音频后端读数（`data-rustlua-audio`，由 web-runtime.js 的 syncAudioMarker 写）：**只记录、
# 不判定**，与微信 harness 的 audio_ctx/audio_played 同口径。absent = 产物还是旧版运行时
# （后端接缝没落地）；`kind=none` = 后端没装上；停在启动场景时 created 至少为 1（标题 BGM）。
$audioEvidence = if ($dom -match 'data-rustlua-audio="([^"]*)"' -and $Matches.Count -gt 0) { $Matches[1] } else { "absent" }

# 预设文字自对账：`data-rustlua-prefab-text=画/应有` 由 web-runtime.js 每帧写，只统计
# **运行期 prefab 实例**里带非空 Text 的实体（画的那一半是实际走满 fillText 的）。
# 为什么单独要这一对读数：`ui-elements`/`ui-now` 数的是 Lua 每帧 `ui.*` 的动态行，屏幕 UI
# 实体一个都不计入，所以"面板只剩暗底、正文整批不画"在它上面是静默的（实测 2026-10-07
# 修复前后 drawn=5713 / ui elements=34 逐位相同）。字段缺失同样算红：产物与门错位。
$prefabTextDrawn = $null
$prefabTextTotal = $null
if ($dom -match 'data-rustlua-prefab-text="(\d+)/(\d+)"' -and $Matches.Count -gt 1) {
    $prefabTextDrawn = [int]$Matches[1]
    $prefabTextTotal = [int]$Matches[2]
}

# L3 行为契约结论。报告首行三选一：`web_api_contract_ok cases=N` /
# `web_api_contract_failed mismatches=N cases=N` / `web_api_contract_unavailable <原因>`。
# **找不到这三行也必须算红**：那次 node 调用超时时把 DOM 写进了同一个文件，
# 把它当"没数据所以放行"就等于给门开了个洞。反过来，格式对不上（比如产物还是旧版
# cdp 脚本、失败行没有 cases）也一律红，不静默降级成"只报 mismatches"。
$contractVerdict = "not-requested"
$contractCases = 0
if ($ApiContract) {
    if ($contractOutput -match 'web_api_contract_ok cases=(\d+)' -and $Matches.Count -gt 0) {
        $contractVerdict = "ok"; $contractCases = [int]$Matches[1]
    } elseif ($contractOutput -match 'web_api_contract_failed mismatches=(\d+) cases=(\d+)' -and $Matches.Count -gt 1) {
        $contractVerdict = "failed(" + $Matches[1] + ")"; $contractCases = [int]$Matches[2]
    } else {
        $contractVerdict = "unavailable"
    }
}

$summary = [ordered]@{
    task           = "M31"
    bundle         = $bundle
    url            = $url
    browser        = $browser
    wasmBytes      = $wasmBytes.Length
    boot           = $boot
    detail         = $detail
    missingCount   = $missingNames.Count
    missingHostApi = ($missingNames -join ';')
    statusText     = $statusText
    drawnSamples   = $drawnRaw
    uiElements     = $uiElements
    world          = $world
    tileLayers     = if ($null -eq $tileDrawn) { "absent" } else { "$tileDrawn/$tileDeclared" }
    drive          = $driveState
    driveDetail    = $driveStateDetail
    driveJhyt      = if ($DriveJhyt) { $driveState } else { "not-requested" }
    driveJhytDetail = if ($DriveJhyt) { $driveStateDetail } else { "" }
    sceneVerify    = $sceneVerifyState
    sceneVerifyDetail = $sceneVerifyDetail
    audio          = $audioEvidence
    apiContract    = $contractVerdict
    apiContractCases = $contractCases
    apiContractReport = $contractReportPath
    domDump        = $domPath
    log            = $logPath
}
$summary | ConvertTo-Json | Set-Content -Path (Join-Path $logRoot "bundle-summary-$stamp.json") -Encoding UTF8

Write-Log "boot=$boot missing_host_api=$($missingNames.Count) detail=$detail" $(if ($boot -eq "ok") { "Green" } else { "Yellow" })
Write-Log "status text: $statusText"
Write-Log "drawn samples: $(if ($null -eq $drawnRaw) { 'unknown' } else { $drawnRaw }) ui elements: $(if ($null -eq $uiElements) { 'unknown' } else { $uiElements })"
Write-Log "world: $(if ([string]::IsNullOrWhiteSpace($world)) { '(未自报)' } else { $world })"
Write-Log "tile_layers: $(if ($null -eq $tileDrawn) { 'absent' } else { "$tileDrawn/$tileDeclared" })" `
    $(if ($null -eq $tileDrawn) { "Yellow" } elseif ($tileDrawn -ne $tileDeclared) { "Red" } else { "Green" })
if ($Drive) {
    Write-Log "drive: $driveState detail: $driveStateDetail" $(if ($driveState -eq "done") { "Green" } else { "Yellow" })
}
if ($DriveJhyt) {
    Write-Log "drive_jhyt: $driveState detail: $driveStateDetail" $(if ($driveState -eq "done") { "Green" } else { "Yellow" })
}
if (-not $NoSceneVerify) {
    Write-Log "scene_verify: $sceneVerifyState detail: $sceneVerifyDetail" $(if ($sceneVerifyState -eq "done") { "Green" } else { "Yellow" })
}
Write-Log "audio: $audioEvidence" $(if ($audioEvidence -eq "absent") { "Yellow" } else { "Green" })
Write-Log "prefab_text: $(if ($null -eq $prefabTextDrawn) { 'absent' } else { "$prefabTextDrawn/$prefabTextTotal" })" `
    $(if ($null -eq $prefabTextDrawn) { "Yellow" } elseif ($prefabTextDrawn -ne $prefabTextTotal) { "Red" } else { "Green" })
if ($ApiContract) {
    Write-Log "api_contract: $contractVerdict cases=$contractCases report=$contractReportPath" `
        $(if ($contractVerdict -eq "ok") { "Green" } else { "Red" })
}
Write-Log "dom: $domPath"

if ($drawnRaw -eq 0) {
    # 启动成功但没有画面：这正是"页面能跑却黑屏"的典型症状，必须显式失败。
    Write-Log "webgl_bundle_blank boot=$boot detail=canvas has no painted pixels" Red
    Exit-With 1
}

if (-not [string]::IsNullOrWhiteSpace($world)) {
    if ($null -eq $tileDrawn) {
        # 世界自报在、读数缺一项 = 运行时与门版本错位（字段被删或产物是旧版），不静默放行。
        Write-Log "webgl_bundle_tile_layers_missing detail=no tilemaps= field in world report world=$world" Red
        Exit-With 1
    }
    if ($tileDrawn -ne $tileDeclared) {
        # 渲染循环遍历的层数 ≠ Lua 注册表里的层数 ⇒ 有一整层瓦片根本没进画面（种田/浇水
        # 这类 overlay 会表现为"数据在动、画面不变"）。
        Write-Log "webgl_bundle_tile_layers_mismatch drawn=$tileDrawn declared=$tileDeclared world=$world" Red
        Exit-With 1
    }
    if (($Drive -or $DriveJhyt) -and $tileDeclared -lt 1) {
        # 剧本已经进了玩法地图，那里至少该有一张地表瓦片层；0 层 = 场景没带瓦片或换图没落地。
        Write-Log "webgl_bundle_tile_layers_empty detail=gameplay scene has no tilemap layer world=$world" Red
        Exit-With 1
    }
}

# 预设文字门。放在 drive 判定**之前**：drive 现在本来就是红的（nav-exit），排在它后面这条
# 断言永远轮不到跑，就成了摆设。
if (-not [string]::IsNullOrWhiteSpace($world)) {
    if ($null -eq $prefabTextDrawn) {
        Write-Log "webgl_bundle_prefab_text_missing detail=no data-rustlua-prefab-text in dom world=$world" Red
        Exit-With 1
    }
    if ($prefabTextDrawn -ne $prefabTextTotal) {
        # 预设实例声明了正文却没画满 = 面板只剩暗底（换场景后 sceneTextSuppressed 误伤
        # prefab 实例就是这一类：set_text 返回 true，屏幕上一个字都没有）。
        Write-Log "webgl_bundle_prefab_text_suppressed drawn=$prefabTextDrawn expected=$prefabTextTotal world=$world" Red
        Exit-With 1
    }
    if ($DriveJhyt -and $prefabTextTotal -lt 1) {
        # 剧本已进玩法地图，HUD 预设每帧都在写 HudLine/目标/提示，应有数不可能是 0。
        Write-Log "webgl_bundle_prefab_text_empty detail=gameplay scene has no prefab text world=$world" Red
        Exit-With 1
    }
}

if ($Drive -and $driveState -ne "done") {
    # P0-①：请求了"完整剧本"却没跑完——无论 boot 是否 ok，都是验收失败。
    Write-Log "webgl_bundle_drive_failed boot=$boot drive=$driveState detail=$driveStateDetail world=$world" Red
    Exit-With 1
}

if ($DriveJhyt -and $driveState -ne "done") {
    # P0-②：请求了"完整玩法剧本"却没跑完（卡在某个里程碑或超时）——无论 boot 是否 ok，都是验收失败。
    Write-Log "webgl_bundle_drivejhyt_failed boot=$boot drive=$driveState detail=$driveStateDetail world=$world" Red
    Exit-With 1
}

if (-not $NoSceneVerify -and $sceneVerifyState -ne "done") {
    # 场景切换防回归门：boot 能过、场景一切就死（wasm 宿主闭包 raise 撞 panic=abort 的现场
    # 症状）就是这类问题。absent/in-flight/failed 三种都算失败，因为都意味着
    # applySceneSwitch 的 fetch → parse → on_create 全流程没有干净跑完。
    Write-Log "webgl_bundle_scene_verify_failed boot=$boot scene_verify=$sceneVerifyState detail=$sceneVerifyDetail world=$world" Red
    Exit-With 1
}

if ($ApiContract -and $contractVerdict -ne "ok") {
    # L3：桌面与 web（**打完宿主补丁**的活体运行时）行为不一致。这正是"第二套实现"最容易烂掉
    # 的地方，也是 docs/51 那句"按引擎源码逐条实现宿主 API"唯一能被机器检查的形式。
    # 处置顺序固定：先修实现（补丁层多半是历史遗留的死覆盖），再把确实该不同的行为从契约里
    # **删掉**并在 L2 覆盖清单说明理由；把期望改成 web 的现状 = 把门拆了。
    Write-Log "webgl_api_contract_failed verdict=$contractVerdict report=$contractReportPath" Red
    if (Test-Path -LiteralPath $contractReportPath) {
        Get-Content -LiteralPath $contractReportPath -Encoding UTF8 -TotalCount 26 |
            ForEach-Object { Write-Log "  $_" Red }
    }
    Exit-With 1
}

if ($boot -eq "ok") {
    Write-Log "webgl_bundle_ok wasm=$([math]::Round($wasmBytes.Length / 1024, 1))KB" Green
    Exit-With 0
}
if ($boot -in @("booted", "running")) {
    Write-Log "webgl_bundle_ok (booted; status text not final: $statusText)" Green
    Exit-With 0
}
Write-Log "webgl_bundle_boot_failed boot=$boot detail=$detail" Red
Exit-With 1