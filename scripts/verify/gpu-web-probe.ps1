# GPU 后端能力探针（WP2 的"GPU 后端技术验证"）与**渲染器**探针（WP5 第一片）。
#
# 为什么必须真的在浏览器里跑一遍：wgpu 能不能用 WebGL2 后端，取决于**运行时**才成立的东西——
# 浏览器给的是 WebGL2 还是 WebGPU、适配器报什么 limits/features、`Limits::default()` 会不会
# 直接建不出设备、无存储缓冲的绑定能不能建。这些在编译期一个都看不出来：
# `cargo check --target wasm32-unknown-unknown` 只能证明**类型**对得上。
#
# 两个阶段问的不是同一件事，成败原因也完全不同，所以分成 `-Stage`：
#   backend（默认）——**wgpu 的 GL 后端能不能用**：适配器/限制/清屏+读回/存储缓冲会不会被拒。
#   render         ——**本仓那台按 WebGPU 规格写的渲染器能不能在 WebGL2 上画对一帧**：
#                    真 `engine::host::Renderer` 离屏渲染 + 像素与帧计数断言。能力探针过了
#                    不代表渲染器过了（几十条管线的翻译、绑定组、深度附件、构造期的阻塞调用，
#                    坑都在后者），所以这一段不做玩具渲染器。
#
# 这一步的结论直接决定 WP5（GPU 2D 渲染器）怎么做：如果 WebGL2 上 `Limits::default()` 建不出
# 设备、或者只读存储缓冲绑定必失败，那么现有的 `engine/crates/rustlua-gpu/src/renderer` 就不能原样搬上 web，
# 必须先给一条"无存储缓冲 + downlevel 限制"的路径。
#
# 产物布局（都在 artifacts 下，不进发布包）：
#   artifacts/gpu-web-probe/pkg/gpu_web_probe.js|_bg.wasm   wasm-bindgen 的 web 目标胶水
#   artifacts/gpu-web-probe/index.html                      能力探针页面（scripts/ 下同名文件拷过来）
#   artifacts/gpu-web-probe/render.html                     渲染探针页面（同上）
#   artifacts/logs/gpu-web-probe/*                           日志、DOM dump、JSON 报告
#
# 退出码：0 = 探针跑完（细节看 JSON）；1 = 探针页面失败/超时/断言不成立；3 = 环境受限（缺
# node/浏览器/wasm-bindgen/wasm32 目标，或这台机器给不出 WebGL2 画布）——与
# scripts/verify/verify-wasm-lua.ps1 同一套语义，绝不把"跳过"当"通过"。
param(
    [switch]$SkipBuild,
    [int]$TimeoutSeconds = 60,
    [string]$ExtraBrowserFlags = "",
    # `backend` = WP2 的能力探针（wgpu 的 WebGL2 后端能不能用）；
    # `render`  = WP5 第一片（**真正的 engine 渲染器**在 WebGL2 上能不能把一帧画对）。
    # 两者共用同一份 wasm 产物与同一套浏览器/服务器管线，只有页面与判据不同。
    [ValidateSet("backend", "render")]
    [string]$Stage = "backend",
    # `scripts/verify/verify.ps1 -Web` 段要求子脚本把退出码写进这个文件（见 Set-Status 的注释）。
    [string]$StatusFile = ""
)

$ErrorActionPreference = "Stop"
# `$PSScriptRoot` 是自动变量，不要赋值（本仓脚本一律直接用）。
$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$engineRoot = Join-Path $repoRoot "engine"
$logRoot = Join-Path $repoRoot "artifacts\logs\gpu-web-probe"
$probeRoot = Join-Path $repoRoot "artifacts\gpu-web-probe"
$pkgDir = Join-Path $probeRoot "pkg"
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"

New-Item -ItemType Directory -Force -Path $logRoot, $probeRoot, $pkgDir | Out-Null

function Write-Log([string]$Message, [string]$Color = "Gray") {
    Write-Host "  $Message" -ForegroundColor $Color
}

function Exit-With([int]$Code) { exit $Code }

# ---------- 依赖解析（与 package.ps1 / verify-wasm-lua.ps1 共用同一份逻辑） ----------
. (Join-Path $PSScriptRoot "..\build\wasm-bindgen.ps1")
# vendored Lua 的 wasm C 源要 `zig cc` 才能编（见 vendor/mlua-sys/build）；不先解析，
# 探针会以 "mlua-sys: cannot locate zig" 失败，看起来像探针坏了。与 verify-wasm-lua 同一口径：
# $env:ZIG → engine\.tools\zig → PATH，缺了就按需装。
. (Join-Path $PSScriptRoot "..\build\zig-toolchain.ps1")
$zig = Resolve-ZigToolchain
if (-not $zig) {
    Write-Log "gpu_probe_skipped reason=no_zig (vendored Lua 的 wasm C 编译需要 zig)" Yellow
    Exit-With 3
}
$env:ZIG = $zig

$node = Get-Command node -ErrorAction SilentlyContinue
if (-not $node) {
    Write-Log "gpu_probe_skipped reason=no_node" Yellow
    Exit-With 3
}

$browser = @(
    (Join-Path $env:ProgramFiles "Microsoft\Edge\Application\msedge.exe"),
    (Join-Path ${env:ProgramFiles(x86)} "Microsoft\Edge\Application\msedge.exe"),
    (Join-Path $env:ProgramFiles "Google\Chrome\Application\chrome.exe"),
    (Join-Path ${env:ProgramFiles(x86)} "Google\Chrome\Application\chrome.exe")
) | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if (-not $browser) {
    Write-Log "gpu_probe_skipped reason=no_browser (需要 Edge 或 Chrome)" Yellow
    Exit-With 3
}

# ---------- 1. 构建探针（wasm32 + 胶水） ----------
if (-not $SkipBuild) {
    $wasmBindgen = Resolve-WasmBindgenCli -Provision
    if (-not $wasmBindgen) {
        Write-Log "gpu_probe_skipped reason=no_wasm_bindgen（跑 scripts\build\wasm-bindgen.ps1 装）" Yellow
        Exit-With 3
    }
    # 保留 target_features 自定义段（含 reference-types），否则 wasm-bindgen 报
    # "externref table required for catch wrappers"（见 verify-wasm-lua.ps1 同款处理）。
    $savedStrip = $env:CARGO_PROFILE_RELEASE_STRIP
    $env:CARGO_PROFILE_RELEASE_STRIP = "none"
    Push-Location $engineRoot
    try {
        Write-Log "==> cargo build -p gpu-web-probe (release, wasm32)" Cyan
        # PS 5.1 陷阱：`$ErrorActionPreference='Stop'` 下原生程序写到 stderr 会被当成**终止性
        # 错误**（cargo 的进度全在 stderr），所以调用期间必须切回 Continue，退出码自己判。
        $savedEap = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            & cargo build -p gpu-web-probe --release --target wasm32-unknown-unknown
            $cargoExit = $LASTEXITCODE
        }
        finally { $ErrorActionPreference = $savedEap }
        if ($cargoExit -ne 0) { throw "gpu-web-probe wasm 构建失败 (exit $cargoExit)" }
    }
    finally {
        Pop-Location
        if ($null -eq $savedStrip) { Remove-Item Env:\CARGO_PROFILE_RELEASE_STRIP -ErrorAction SilentlyContinue }
        else { $env:CARGO_PROFILE_RELEASE_STRIP = $savedStrip }
    }

    $probeWasm = Join-Path $engineRoot "target\wasm32-unknown-unknown\release\gpu_web_probe.wasm"
    if (-not (Test-Path -LiteralPath $probeWasm)) { throw "找不到探针 wasm: $probeWasm" }
    Write-Log "==> wasm-bindgen --target web → $pkgDir" Cyan
    $savedEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & $wasmBindgen $probeWasm "--out-dir" $pkgDir "--target" "web" "--no-typescript"
        $bindgenExit = $LASTEXITCODE
    }
    finally { $ErrorActionPreference = $savedEap }
    if ($bindgenExit -ne 0) { throw "wasm-bindgen 失败 (exit $bindgenExit)" }
}

if (-not (Test-Path -LiteralPath (Join-Path $pkgDir "gpu_web_probe.js"))) {
    Write-Log "gpu_probe_skipped reason=probe_artifact_missing（先不带 -SkipBuild 跑一次）" Yellow
    Exit-With 3
}
# 两个阶段各一个页面、各一组 data-* 属性名（CDP 驱动按属性名判落定，见 webgl-cdp-wait.js）。
$pageName = if ($Stage -eq "render") { "render.html" } else { "index.html" }
$stateAttr = if ($Stage -eq "render") { "data-render-probe" } else { "data-gpu-probe" }
$jsonAttr = "$stateAttr-json"
$cdpMode = if ($Stage -eq "render") { "render-probe" } else { "gpu-probe" }
$logPrefix = if ($Stage -eq "render") { "render-probe" } else { "gpu-probe" }
Copy-Item -LiteralPath (Join-Path $PSScriptRoot "gpu-web-probe\$pageName") -Destination (Join-Path $probeRoot $pageName) -Force

# ---------- 2. 起静态服务器 ----------
$port = 8300 + (Get-Random -Maximum 600)
$serveLog = Join-Path $logRoot "serve-$stamp.log"
$server = Start-Process -FilePath $node.Source `
    -ArgumentList @((Join-Path $PSScriptRoot "..\dev\serve-static.js"), $probeRoot, "$port") `
    -PassThru -NoNewWindow `
    -RedirectStandardOutput $serveLog -RedirectStandardError "$serveLog.err"
Start-Sleep -Milliseconds 800
$url = "http://127.0.0.1:$port/$pageName"

$domPath = Join-Path $logRoot "dom-$stamp.html"
$errPath = Join-Path $logRoot "dom-$stamp.err"
$cdpPort = 9900 + (Get-Random -Maximum 90)
$profileDir = Join-Path $logRoot "cdp-profile-$stamp"

# 无头实例默认**没有 GPU**，WebGL 要走软件光栅（SwiftShader）；Chrome/Edge 122 起软件 WebGL
# 需要显式放行，否则 `getContext('webgl2')` 直接返回 null，探针会得到"没有 WebGL2"这个
# **假结论**。所以这几条标志不是可选优化，是"能不能测"的前提。
$arguments = @(
    "--headless=new",
    "--disable-gpu-sandbox",
    "--no-sandbox",
    "--disable-extensions",
    "--enable-unsafe-swiftshader",
    "--use-angle=swiftshader",
    "--enable-logging=stderr",
    "--v=1",
    "--remote-debugging-port=$cdpPort",
    "--user-data-dir=$profileDir",
    $url
)
if ($ExtraBrowserFlags) { $arguments = @($arguments[0..($arguments.Count - 2)]) + $ExtraBrowserFlags.Split(" ") + @($url) }

Write-Log "==> 探针页面 $url（CDP $cdpPort，超时 ${TimeoutSeconds}s）" Cyan
$process = Start-Process -FilePath $browser -ArgumentList $arguments -PassThru -NoNewWindow `
    -RedirectStandardOutput $domPath -RedirectStandardError $errPath

$cdpEcho = @()
$savedEap = $ErrorActionPreference
$ErrorActionPreference = "Continue"
try {
    $cdpEcho = @(& node (Join-Path $PSScriptRoot "..\lib\webgl-cdp-wait.js") $cdpPort $domPath $cdpMode $TimeoutSeconds)
    $cdpExit = $LASTEXITCODE
}
finally { $ErrorActionPreference = $savedEap }
foreach ($line in $cdpEcho) { Write-Log "cdp: $line" }

# 收尾：只杀本实例（按唯一 user-data-dir 匹配命令行），绝不按进程名一网打尽。
Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -in @("msedge.exe", "chrome.exe") -and $_.CommandLine -like "*$profileDir*" } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force $profileDir -ErrorAction SilentlyContinue

# ---------- 3. 取回 JSON ----------
# CDP 落盘的是**序列化 DOM**，属性值里的 `"` 是 `&quot;`、`&` 是 `&amp;`，必须先反转义。
function ConvertFrom-HtmlAttr([string]$Text) {
    return $Text.Replace('&quot;', '"').Replace('&#39;', "'").Replace('&lt;', '<').Replace('&gt;', '>').Replace('&amp;', '&')
}
$dom = if (Test-Path -LiteralPath $domPath) { [System.IO.File]::ReadAllText($domPath) } else { "" }
$state = if ($dom -match "$stateAttr=`"([^`"]*)`"") { $Matches[1] } else { "absent" }
$json = if ($dom -match "$jsonAttr=`"([^`"]*)`"") { ConvertFrom-HtmlAttr $Matches[1] } else { "" }
$reportPath = Join-Path $logRoot "$logPrefix-$stamp.json"
if ($json) { [System.IO.File]::WriteAllText($reportPath, $json, (New-Object System.Text.UTF8Encoding($false))) }

function Set-Status([int]$Code) {
    # `-Web` 段（scripts/verify/verify.ps1）**只认状态文件**：`Start-Process -PassThru` 的 ExitCode 是 $null，
    # `[int]$null` 会静默变 0，于是"子进程失败"会被当成"通过"。缺状态文件即判失败。
    if ($StatusFile) {
        [System.IO.File]::WriteAllText($StatusFile, "exit=$Code", (New-Object System.Text.UTF8Encoding($false)))
    }
}

if ($state -ne "done" -or -not $json) {
    Write-Log "gpu_probe_FAILED state=$state cdp_exit=$cdpExit" Red
    if ($json) { Write-Log $json Red }
    Write-Log "详情: $domPath / $errPath" Yellow
    Set-Status 1
    Exit-With 1
}

$probe = $null
try { $probe = $json | ConvertFrom-Json } catch { }

# ---------- WP5 第一片（`-Stage render`）：真渲染器在 WebGL2 上的判据 ----------
#
# 这一段的红/绿/受限三分与 backend 阶段同款，但"受限"只认**环境给不出 WebGL2**那几种
# （surface/adapter 建不出来）；渲染器自己起不来（管线/绑定组/深度附件/时钟）是**结论**，
# 必须判红——那正是本片要抓的东西，把它归成"环境受限"等于把发现丢掉。
if ($Stage -eq "render") {
    $renderFailures = @()
    $fatal = if ($probe) { $probe.fatal } else { $null }
    if ($fatal) {
        if ($fatal -match 'create_surface|request_adapter|no window|no document') {
            Write-Log "render_probe_skipped reason=environment_limited error=$fatal" Yellow
            Set-Status 3
            Exit-With 3
        }
        $renderFailures += "渲染探针致命失败: $fatal"
    }
    elseif ($probe.blocker) {
        # 构造期就被 WebGL2 拒绝：这不是环境问题，是**渲染器还没为 WebGL2 做完降级**，
        # 所以判红并把 wgpu 的原话原样打出来（判据与修法见 plan/M73-00 的 WP5 段）。
        $renderFailures += "渲染器构造期被拒: $($probe.blocker)"
        if ($probe.construction_validation_error) {
            $renderFailures += "wgpu 原话: $($probe.construction_validation_error)"
        }
        $renderFailures += "（存储缓冲在 WebGL2 上不可用是 WP2 已知事实；修法=按设备能力把矢量文字与 GPU 实例化两条路降级，3D 蒙皮用的是 uniform 骨骼数组、不受影响）"
    }
    else {
        if ($probe.backend -ne "Gl") {
            $renderFailures += "适配器后端是 $($probe.backend)，不是 Gl（这门测不到 WebGL2）"
        }
        if (-not $probe.renderer_created) { $renderFailures += "Renderer 没建起来" }
        if (-not $probe.outcome_presented) { $renderFailures += "render_into_texture 没返回 Presented" }
        if ($probe.mismatches -ne 0) {
            $renderFailures += "采样点有 $($probe.mismatches) 处像素不符（期望值与实际值见报告）"
        }
        # 内存上传的贴图（WP5 第二片前置）：四个纹素中心必须等于上传的字节。
        # 单独计数是为了失败时能一眼分清"贴图路径断了"还是"几何/视口错了"。
        if ($probe.texture_mismatches -ne 0) {
            $renderFailures += "内存上传贴图有 $($probe.texture_mismatches) 个纹素采样不符（web 上没有文件系统，这条断了图集就无从验证）"
        }
        if ($probe.loaded_texture_count -ne 1) {
            $renderFailures += "loaded_texture_count=$($probe.loaded_texture_count)，期望 1（上传没进缓存？）"
        }
        # 渲染器自报的帧计数必须与手工场景对得上：3 个 draw（每个 6 顶点 = 2 三角形）、0 丢弃。
        # 前两块**同贴图且顶点首尾相接**，因此 2D 保序合并应把它们合成**一次**提交 ⇒ draw_calls=2
        # （triangles 仍是 6）。这条断言同时是"合并在 WebGL2 上真的生效"的证据：合并若回归，
        # draw_calls 会回到 3，而像素断言照样绿——只有这条能抓住它。`draw_calls` 是设备无关的
        # 硬指标，所以软件光栅上量它不算"拿绘图性能当读数"。
        if ($probe.draw_calls -ne 2) {
            $renderFailures += "draw_calls=$($probe.draw_calls)，期望 2（3 个 draw 中前两块同贴图相邻，应合并成一次提交）"
        }
        if ($probe.triangles -ne 6) { $renderFailures += "triangles=$($probe.triangles)，期望 6" }
        if ($probe.dropped_draws -ne 0) { $renderFailures += "dropped_draws=$($probe.dropped_draws)，期望 0" }
        # WP2 硬约束 2 的回归门：请求值必须是"地板 + 抬分辨率"，设备拿到的 2D 上限不能被压回 2048。
        if ($probe.limits.requested_2d -ne $probe.limits.adapter_2d) {
            $renderFailures += "请求的 2D 上限 $($probe.limits.requested_2d) ≠ 适配器值 $($probe.limits.adapter_2d)（using_resolution 没生效？）"
        }
        if ($probe.limits.device_2d_after_request -lt $probe.limits.requested_2d) {
            $renderFailures += "设备实际 2D 上限 $($probe.limits.device_2d_after_request) < 请求值 $($probe.limits.requested_2d)"
        }
        $bytes = 256 * 256 * 4
        if ($probe.readback_bytes -ne $bytes) {
            $renderFailures += "读回字节数 $($probe.readback_bytes)，期望 $bytes"
        }
        # 结构检查：白色像素数≈两块四边形的解析面积（2% 容差只用来吸收光栅化的边界舍入）。
        $expected = [double]$probe.expected_white_pixels
        $actual = [double]$probe.white_pixels
        if ($expected -le 0 -or [math]::Abs($actual - $expected) / $expected -gt 0.02) {
            $renderFailures += "白色像素数 $actual，期望 ≈$expected（几何位置/视口不对会在这里露出来）"
        }
        # 画布表面（present）：web 宿主要走的那条路。判据是"按渲染器的格式配得起来、渲得进去、
        # present 出去且**没有任何校验错误**"——画布纹理内容读不回来（WebGL2 只给
        # RENDER_ATTACHMENT，见 `canvas_readback`），所以逐像素判据仍取自离屏那一帧。
        $sp = $probe.surface_present
        if (-not $sp) {
            $renderFailures += "报告里没有 surface_present（画布 present 那一段没跑？）"
        }
        else {
            if (-not $sp.format_offered) {
                $renderFailures += "画布不提供 Rgba8UnormSrgb（给的是 $($sp.capabilities_formats -join ','))，而渲染器的颜色附件格式是构造时烘死的——Web 宿主必须先把目标格式参数化"
            }
            if (-not $sp.configured) { $renderFailures += "画布 surface 配置失败: $($sp.configure_error)$($sp.blocker)" }
            if (-not $sp.outcome_presented) { $renderFailures += "渲进画布纹理没有返回 Presented" }
            if (-not $sp.presented) { $renderFailures += "画布 present 没成功: $($sp.get_current_texture_error)$($sp.render_present_error)" }
            if ($sp.render_present_error) { $renderFailures += "渲染/ present 画布时报了校验错误: $($sp.render_present_error)" }
            if ($sp.presented -and $sp.canvas_mismatches -and $sp.canvas_mismatches -ne 0) {
                $renderFailures += "画布纹理逐像素对拍有 $($sp.canvas_mismatches) 处不符"
            }
        }
    }

    if ($renderFailures.Count -gt 0) {
        Write-Log "render_probe_FAILED $($renderFailures.Count) 项断言不成立：" Red
        $renderFailures | ForEach-Object { Write-Log "  - $_" Red }
        Write-Log "完整 JSON: $reportPath" Yellow
        Set-Status 1
        Exit-With 1
    }

    Write-Log "render_probe_ok backend=$($probe.backend) device=$($probe.device_type) report=$reportPath" Green
    Write-Log "  帧计数：draw_calls=$($probe.draw_calls) triangles=$($probe.triangles) dropped=$($probe.dropped_draws) passes=$($probe.pass_count) cpu_us=$($probe.cpu_frame_us)"
    Write-Log "  2D 上限：适配器 $($probe.limits.adapter_2d) → 请求 $($probe.limits.requested_2d) → 设备 $($probe.limits.device_2d_after_request)（存储缓冲 $($probe.limits.device_storage_buffers)）"
    Write-Log "  像素：白 $($probe.white_pixels) / 期望 $($probe.expected_white_pixels)，采样不符 $($probe.mismatches) 处（其中贴图纹素 $($probe.texture_mismatches) 处）"
    Write-Log "  内存上传贴图：缓存 $($probe.loaded_texture_count) 张（键 probe://checker2x2，2×2 四色，逐纹素对拍）"
    Write-Log "  画布 surface：格式 $($probe.surface_present.capabilities_formats -join '/')（请求 Rgba8UnormSrgb 可用=$($probe.surface_present.format_offered)）、present_modes $($probe.surface_present.capabilities_present_modes -join '/')、usages $($probe.surface_present.capabilities_usages) → 配置=$($probe.surface_present.configured) present=$($probe.surface_present.presented) 校验错误=$([bool]$probe.surface_present.render_present_error)"
    Write-Log "  画布读回：$($probe.surface_present.canvas_readback)" Yellow
    Set-Status 0
    Exit-With 0
}

$gl = if ($probe) { $probe.configs.gl_only } else { $null }

# 判据（红/绿/受限三分，别把"环境给不出"当"通过"，也别把"环境给不出"当"回归"）：
#   ① `Backends::GL` 连 **surface** 都建不出来 → 这台机器给不出 WebGL2 画布（无头无软件光栅等）
#      → 环境受限（exit 3），打印原因后跳过。
#   ② surface 建得出来、却拿不到适配器 → **判红**：wgpu 的 WebGL2 后端在这套环境上坏了，
#      这正是本门要抓的回归（WG5/WP5 的渲染路径全靠它）。
#   ③ 适配器到手后逐项断言：必须是 `Gl` 后端、`downlevel_webgl2_defaults()` 能建出设备、
#      清屏 + 读回真的是纯绿、只读存储缓冲绑定**必须**被校验拒绝（WebGL2 上没有存储缓冲；
#      这条同时防止"探针其实跑在 WebGPU 上"这种自欺）。
$failures = @()
if (-not $gl) {
    $failures += "报告里没有 configs.gl_only（JSON 结构变了？）"
}
elseif (-not $gl.create_surface_ok) {
    Write-Log "gpu_probe_skipped reason=no_webgl2_surface phase=$($gl.phase) error=$($gl.error)" Yellow
    Write-Log "（这台机器给不出 WebGL2 画布：无头实例需要 --enable-unsafe-swiftshader 之类的软件光栅支持）" Yellow
    Set-Status 3
    Exit-With 3
}
else {
    if (-not $gl.request_adapter_ok) { $failures += "Backends::GL 拿不到适配器: $($gl.error)" }
    if ($gl.adapter_info.backend -ne "Gl") { $failures += "适配器后端是 $($gl.adapter_info.backend)，不是 Gl（这门测不到 WebGL2）" }
    if (-not $gl.request_device_downlevel_limits.ok) { $failures += "downlevel_webgl2_defaults() 建不出设备: $($gl.request_device_downlevel_limits.error)" }
    $px = $gl.render_readback.first_pixel_rgba
    if (-not $px -or $px.Count -lt 4 -or $px[0] -ne 0 -or $px[1] -ne 255 -or $px[2] -ne 0 -or $px[3] -ne 255) {
        $failures += "清屏绿读回不是 [0,255,0,255]，实际是 [$($px -join ',')]"
    }
    if (-not $gl.storage_buffer_bind_group_layout.validation_error) {
        $failures += "只读存储缓冲绑定竟然建成功了——WebGL2 上不该发生（适配器可能不是 WebGL2）"
    }
    if ($gl.request_device_default_limits.ok) {
        # 不是失败：将来 wgpu/浏览器把 default limits 变成可用是好事，但读数变了要留痕。
        Write-Log "注意：Limits::default() 这次**建出了**设备（WebGL2 上历史读数是被拒），读数已变，请更新台账" Yellow
    }
}

if ($failures.Count -gt 0) {
    Write-Log "gpu_probe_FAILED $($failures.Count) 项断言不成立：" Red
    $failures | ForEach-Object { Write-Log "  - $_" Red }
    Write-Log "完整 JSON: $reportPath" Yellow
    Write-Log $json Red
    Set-Status 1
    Exit-With 1
}

Write-Log "gpu_probe_ok backend=$($gl.adapter_info.backend) device=$($gl.adapter_info.device_type) report=$reportPath" Green
Write-Log "  limits：adapter 2d=$($gl.adapter_limits.max_texture_dimension_2d) 设备(降级后) 2d=$($gl.device_limits_after_request.max_texture_dimension_2d) storage_buffers=$($gl.device_limits_after_request.max_storage_buffers_per_shader_stage) uniform_binding=$($gl.device_limits_after_request.max_uniform_buffer_binding_size)"
Write-Log "  defaults 被拒原因：$($gl.request_device_default_limits.error)"
Write-Log "  读回像素：[$($gl.render_readback.first_pixel_rgba -join ',')]（期望 0,255,0,255）"
# **不**在成功路径 dump 整份 JSON：它是 4 KB 的单行，塞进 `verify.ps1 -Web` 的门日志会把有用读数
# 淹掉。完整 JSON 上面那句已经给了落盘路径，这里只给要点。
Set-Status 0
Exit-With 0
