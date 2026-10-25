[CmdletBinding()]
param(
    [switch]$SkipClippy,
    [switch]$SkipTests,
    [switch]$SkipBuild,
    [switch]$Release,
    [switch]$KeepLog,
    # MVP 退出门（M19-04）：在常规质量门之后追加 headless 游戏回归、浸泡/性能
    # 基线与干净发行目录自测（package.ps1 -SelfTest）。
    [switch]$Mvp,
    # 仅跑 MVP 追加部分（跳过 fmt/clippy/test/build 常规门）。
    [switch]$MvpOnly,
    # WP0：运行固定 ECS/2D/3D 阶梯基准并写入本地 baselines/（RUSTLUA_BASELINES_DIR 可改位置）。
    [switch]$Benchmarks,
    # docs/54 §8-⑨：跑 Web 平台证据（浏览器契约层 + 已发布产物的启动层）。
    # 环境受限（无浏览器/无 node/无 wasm32 目标）时**显式**跳过并打印原因，不伪造通过。
    [switch]$Web,
    # wasm Lua 运行期验证门（scripts/verify/verify-wasm-lua.ps1）：no-modules 桥（game_web Lua 桥）
    # + gpu2d 宿主画布探针的 Node 验证。退出码 0=通过/1=判红/3=环境受限跳过，
    # 与 -Web 同一哲学：缺 node/wasm32/zig/wasm-bindgen 时显式跳过，绝不把"跳过"当"通过"。
    [switch]$WasmLua,
    # 供应链安全门（P2-01）：cargo-audit / cargo-deny / fork 同步 / SBOM 一致性。
    # 默认关：它需要固定版本的工具（scripts/release/install-security-tools.ps1）与
    # advisory DB；CI 的 security job 直接跑 check-security.ps1，不经过总门。
    [switch]$Security,
    # 快速读数（M66+）：只跑「零成本 + 数据级」的门，跳过编译重活与编辑器实机步骤。
    # 它与 -Mvp/-MvpOnly/-Benchmarks/-Web/-WasmLua 互斥（混用会让「跳过了什么」说不清），
    # 归一化与覆盖面见下面 `if ($Quick)` 那段长注释。
    [switch]$Quick,
    # P4-01：只跑覆盖率采集 + 棘轮门（重活，需 cargo-llvm-cov；缺工具退 3，不是通过）。
    [switch]$Coverage,
    # P4-04/06/07：只跑「已有产物的尺寸采集 + cfg 门 + 趋势检查」，不重跑 benchmark/构建。
    [switch]$Metrics,
    # 限制 cargo 编译并行度与 benchmark rayon 线程池上限，防止完整回归把 CPU/内存打满导致系统崩溃。
    # 未指定时自动取 CPU 核数的一半（最少 2），已足够在 8 核 / 16 GB 机器上把峰值内存与 CPU 占用
    # 控制在安全范围。CI 不受影响（runner 资源固定，可显式传 -MaxJobs 覆盖）。
    [int]$MaxJobs = 0
)

# ── -Quick：回答「这棵树现在是不是红的」的便宜读数 ─────────────────────────────
# 为什么需要它：pre-commit hook 只镜像 CI 的 lint job（`git diff --cached --check` + fmt + clippy），
# 而真正咬人的**数据级工程校验**（`game --check` / `editor --check`：场景引用的资源、脚本语法、
# `validate_data.lua`、组件 schema）此前只存在于总门里。总门在 8 核 / 16 GB 的开发机上要几十分钟
# 到几小时，于是「想拿一次读数」的成本高到没人愿意付——代价已经真实发生过：`scenes/main.scene`
# 的一句中文注释让 `game --check` 判红，这条红在本地 HEAD 里活了 5 个提交才被发现
# （见 `docs/changelog/发布-CI与质量门.md`「场景注释不再冒充依赖」）。`-Quick` 把这一类红的发现成本降到几十秒（热缓存）。
#
# 跑什么：文档门、vendor 补丁门、cargo metadata、fmt、Lua API 文档桩、Web API 覆盖清单、
#         git diff --check、两个判据自测、`game --check`、`editor --check`（两个工程）。
# 不跑什么（每一步都会在结尾逐个打印出来）：clippy、rustdoc、对外 Rust 符号面门（依赖 rustdoc
#         产物）、pre-commit hook 回归、全量测试、构建、空间迁移报告、编辑器性能基线、GUI smoke。
#
# 本仓库口径是「跳过绝不等于通过」：`-Quick` 的绿灯只能读作「数据级与格式门通过」，
# 不能读作总门通过——所以它不打印 Quality gate passed，只打印 Quick gate passed (PARTIAL)。
if ($Quick -and ($Mvp -or $MvpOnly -or $Benchmarks -or $Web -or $WasmLua -or $Security -or $Coverage -or $Metrics)) {
    throw "-Quick 与 -Mvp/-MvpOnly/-Benchmarks/-Web/-WasmLua/-Security/-Coverage/-Metrics 不能同时使用：-Quick 的定义就是「只跑便宜的那一档」，混用会让「跳过了什么」说不清。"
}
$quick = [bool]$Quick
# `-Quick` 本次跳过的步骤名：只由 guard 的调用点与下面这三行登记，结尾统一打印。
# 本仓库不接受「静默跳过」——跳过的每一步都必须在结尾的清单里露面。
$skippedByQuick = [System.Collections.Generic.List[string]]::new()
# 清单只在脚本**走到底**时才是完整的（guard 是走到才登记的）。判红时脚本停在半路，
# 清单必须自曝这一点，否则读者会把"登记了 3 条"误读成"只跳了 3 步"。
$quickCompleted = $false
# -Security 的三态：false=未跑/通过，true=工具或 advisory DB 受限（exit 3，不是通过）。
$securityBlocked = $false

# `-Quick` 的跳过判定：返回 $true 表示「这一步本次跳过」，**副作用**是把步骤名登记进
# `$skippedByQuick`。步骤名写在调用点（也就是 guard 那一行）上，所以清单与 guard 不可能漂移。
function Test-QuickSkip([string]$Step) {
    if (-not $script:quick) { return $false }
    [void]$skippedByQuick.Add($Step)
    return $true
}

if ($quick) {
    # 这三步是编译重活（clippy --all-targets --all-features / 全量测试 / debug+release 构建）。
    # 复用既有开关跳过，而不是并列写第二套判断逻辑——skip 的语义只留一处定义。
    $SkipClippy = $true
    $SkipTests = $true
    $SkipBuild = $true
    [void]$skippedByQuick.Add("Clippy (--all-targets --all-features)")
    [void]$skippedByQuick.Add("Workspace tests (--all-features)")
    [void]$skippedByQuick.Add("Release build + Debug build")
    # P4：覆盖率采集与趋势检查都是重活/依赖工具，-Quick 不跑；登记进跳过清单，不静默。
    [void]$skippedByQuick.Add("Coverage (P4-01, needs cargo-llvm-cov)")
    [void]$skippedByQuick.Add("Metrics (P4-04/06/07, size collection + cfg gate + trend check)")
}

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$engineRoot = Join-Path $projectRoot "engine"
# 基线目录：默认 `<仓库根>\baselines`（整机本地，不入库），可用 RUSTLUA_BASELINES_DIR 指到别处。
# 缺目录时由用到它的步骤现场建，不假设上一台机器留下了什么。
$baselinesRoot = if ($env:RUSTLUA_BASELINES_DIR) { $env:RUSTLUA_BASELINES_DIR } else { Join-Path $projectRoot "baselines" }
New-Item -ItemType Directory -Force -Path $baselinesRoot | Out-Null
$gameRoot = Join-Path $projectRoot "examples"
# 工程校验与干净目录自测的默认目标工程：examples 重组后样板工程在子目录里
# （`examples/game_jhyt`），根目录已没有 project.toml——此前写死 `examples` 会让
# 「Game project check」必然失败。
$defaultGameProject = Join-Path $gameRoot "game_jhyt"
$logRoot = Join-Path $projectRoot "artifacts\logs\quality"
$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$logPath = Join-Path $logRoot "verify-$timestamp.log"

# ── Resource limits ──────────────────────────────────────────────────────────
# 完整回归（fmt + clippy + test + build + MVP examples + WP0 benchmarks）默认会把所有
# CPU 核心与尽可能多的内存吃满，在 8 核 / 16 GB 的开发机上足以让系统卡死甚至崩溃重启。
# 在第一条 cargo 命令之前就把并行度上限写进环境变量，后续所有 cargo / benchmark 子进程
# 都会继承：
#   CARGO_BUILD_JOBS  → cargo 编译（rustc）并行度，直接决定 CPU 与链接期内存峰值
#   RAYON_NUM_THREADS → benchmark 示例运行时线程池（sprite/tilemap/3D stress 等）
if ($MaxJobs -le 0) {
    $procCount = if ($IsWindows -or $env:OS -like '*Windows*') {
        (Get-CimInstance Win32_ComputerSystem).NumberOfLogicalProcessors
    } else {
        [Environment]::ProcessorCount
    }
    $cpuBased = [math]::Max(2, [math]::Floor($procCount / 2))
    # 内存感知：每个 rustc 进程峰值约 3-4 GB，留一半给系统与其他进程。
    # 16 GB 机器 → max 4 jobs，32 GB → max 8 jobs，避免大核数机器把内存吃满。
    $maxJobs = $cpuBased
    try {
        if ($IsWindows -or $env:OS -like '*Windows*') {
            $freeRamGb = [math]::Round((Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory / 1MB, 1)
        } else {
            $freeRamGb = [math]::Round((Get-Content /proc/meminfo | Select-String 'MemAvailable' | ForEach-Object { ($_ -split '\s+')[1] }) / 1MB, 1)
        }
        $memBased = [math]::Max(2, [math]::Floor($freeRamGb / 4))
        $maxJobs = [math]::Min($cpuBased, $memBased)
        Write-Host "  memory check: free=${freeRamGb}GB → mem-based max=$memBased"
    } catch {
        Write-Host "  memory check unavailable ($_), using CPU-based max=$cpuBased" -ForegroundColor Yellow
    }
    $MaxJobs = $maxJobs
}
$env:CARGO_BUILD_JOBS = "$MaxJobs"
$env:RAYON_NUM_THREADS = "$MaxJobs"
# 2026-10-21：`engine --lib` 在 CI 上走到 runtime_scene/schedule 测试时 0xC0000005。
# libtest 在 `--test-threads=1` 时把测试跑在主线程，`RUST_MIN_STACK` 影响不到它；
# 固定 2 个测试线程让用例跑在带 64MB 栈的派生线程上，同时保留最基本的并发隔离。
$env:RUST_TEST_THREADS = "2"

New-Item -ItemType Directory -Force -Path $logRoot | Out-Null

function Write-Step([string]$Message) {
    Write-Host "`n==> $Message" -ForegroundColor Cyan
}

function Invoke-Checked([string]$File, [string[]]$Arguments, [switch]$QuietOutput) {
    $command = "$File $($Arguments -join ' ')"
    Write-Host "> $command" -ForegroundColor DarkGray

    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $output = & $File @Arguments 2>&1
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = $previousPreference

    # `-QuietOutput`：成功时**不**进 stdout/日志的步骤（目前只有 `cargo metadata`——它往 stdout
    # 倒的是整份 workspace JSON，几十 KB 机器可读噪音，人读质量日志时只会把真正的红淹掉）。
    # 判红时照样把捕获到的输出原样打出来：不能出现"失败了却看不到原因"。
    if ($QuietOutput -and $exitCode -eq 0) {
        Add-Content -Path $logPath -Value "> $command (ok; $($output.Count) 行输出按 -QuietOutput 抑制)" -Encoding UTF8
        return
    }

    $output | ForEach-Object {
        $line = $_.ToString()
        Write-Host $line
        Add-Content -Path $logPath -Value $line -Encoding UTF8
    }

    if ($exitCode -ne 0) {
        throw "Command failed (exit code $exitCode): $command"
    }
}

# 捕获输出但**不**让原生命令的 stderr 变成终止错误。
# PS 5.1 在 `$ErrorActionPreference = "Stop"` 下会把 `cargo` 的 linker warning 当成
# NativeCommandError，命令还没跑完就抛——`editor --check` 曾因此在“构建成功”时被误判红。
function Invoke-Captured([string]$File, [string[]]$Arguments) {
    Write-Host "> $File $($Arguments -join ' ')" -ForegroundColor DarkGray
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $output = & $File @Arguments 2>&1
        $exitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousPreference
    }
    [pscustomobject]@{ Output = $output; ExitCode = $exitCode }
}

Write-Step "Resource limits"
Write-Host "  cargo build jobs = $MaxJobs (env:CARGO_BUILD_JOBS)"
Write-Host "  rayon  threads   = $MaxJobs (env:RAYON_NUM_THREADS)"
Write-Host "  override with -MaxJobs <N>"

# package.ps1 必须用**具名参数**调用：PowerShell 的数组 splat（`@array`）是按位置传参，
# 数组里的 `-SelfTest` 会被当作 `-Platform` 的值而触发 ValidateSet 失败
# （`Cannot validate argument on parameter 'Platform'`）。
function Invoke-PackageSelfTest([string]$ProjectRootPath) {
    $packageScript = Join-Path $PSScriptRoot "..\build\package.ps1"
    Write-Host "> powershell -NoProfile -ExecutionPolicy Bypass -File $packageScript -ProjectRoot $ProjectRootPath -SelfTest" -ForegroundColor DarkGray

    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    # 子进程执行：package.ps1 大量使用 Write-Host（不经过管道，管道里捕获不到），
    # 只有子进程的 stdout 才能完整写进质量日志。
    $output = & powershell -NoProfile -ExecutionPolicy Bypass -File $packageScript -ProjectRoot $ProjectRootPath -SelfTest 2>&1
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = $previousPreference

    $output | ForEach-Object {
        $line = $_.ToString()
        Write-Host $line
        Add-Content -Path $logPath -Value $line -Encoding UTF8
    }

    if ($exitCode -ne 0) {
        throw "Package self test failed (exit code $exitCode): $packageScript -ProjectRoot $ProjectRootPath -SelfTest"
    }
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo was not found. Install Rust stable and ensure cargo is on PATH."
}

# ── Stale artifact cleanup ────────────────────────────────────────────────────
# 回归测试会反复跑，磁盘上容易堆积：① package.ps1 -SelfTest 在 Windows 上因 AV/索引器锁文件
# 偶尔清理失败，%TEMP% 里留下完整的发行包副本（每个 ~50-100 MB）；② 编辑器/引擎测试每个用例
# 都会往 %TEMP% 建 `rustlua-{module}-{label}-{token}` 目录，一次 interaction-state 用例 5 MB，
# 100 次 MVP 就 500 MB 起步；③ quality 日志每次运行新增一个带时间戳的文件，从不清理；
# ④ 手动 GUI 证据运行留下的 artifacts/tmp-* 目录；⑤ web/wasm 日志同样堆积。
# 这里在正式测试前统一清理，避免磁盘被悄悄吃满。
Write-Step "Stale artifact cleanup"
$cleanedBytes = 0

# ① + ② %TEMP% 下过期的 rustlua / p2 前缀目录（超过 1 小时的）
# 覆盖两类：package.ps1 的 "rustlua release test {ts}" / "rustlua user data test {ts}"（空格
# 分隔，故意带空格测路径），和 Rust 测试建的 rustlua-{module}-{label}-{token} 目录（连字符
# 分隔）。p2-* 是重命名前遗留，一并清掉。
$staleCutoff = (Get-Date).AddHours(-1)
foreach ($pattern in @("rustlua-*", "rustlua *", "p2-*", "p2 *")) {
    Get-ChildItem -Path $env:TEMP -Directory -Filter $pattern -ErrorAction SilentlyContinue |
        Where-Object { $_.CreationTime -lt $staleCutoff } |
        ForEach-Object {
            try {
                $size = (Get-ChildItem $_.FullName -Recurse -File -ErrorAction SilentlyContinue |
                    Measure-Object -Property Length -Sum).Sum
                Remove-Item -LiteralPath $_.FullName -Recurse -Force -ErrorAction Stop
                $cleanedBytes += $size
            } catch {
                Write-Host "  failed to remove $($_.Name): $_" -ForegroundColor Yellow
            }
        }
}

# ② quality 日志轮转：保留最近 10 个，删除更旧的
$logFiles = Get-ChildItem -LiteralPath $logRoot -File -Filter "verify-*.log" -ErrorAction SilentlyContinue |
    Sort-Object CreationTime -Descending
if ($logFiles.Count -gt 10) {
    $logFiles | Select-Object -Skip 10 | ForEach-Object {
        $cleanedBytes += $_.Length
        Remove-Item -LiteralPath $_.FullName -Force -ErrorAction SilentlyContinue
    }
    Write-Host "  rotated quality logs: kept 10, removed $($logFiles.Count - 10)" -ForegroundColor DarkGray
}

# ③ artifacts/tmp-* 目录（手动 GUI 证据运行的残留，超过 1 天的）
$evidenceCutoff = (Get-Date).AddDays(-1)
Get-ChildItem -LiteralPath $projectRoot -Directory -Filter "tmp-*" -ErrorAction SilentlyContinue |
    Where-Object { $_.CreationTime -lt $evidenceCutoff } |
    ForEach-Object {
        try {
            $size = (Get-ChildItem $_.FullName -Recurse -File -ErrorAction SilentlyContinue |
                Measure-Object -Property Length -Sum).Sum
            Remove-Item -LiteralPath $_.FullName -Recurse -Force -ErrorAction Stop
            $cleanedBytes += $size
            Write-Host "  removed stale evidence: $($_.Name)" -ForegroundColor DarkGray
        } catch {
            Write-Host "  failed to remove $($_.Name): $_" -ForegroundColor Yellow
        }
    }
# artifacts/ 下的 tmp-* 子目录
$artifactsDir = Join-Path $projectRoot "artifacts"
if (Test-Path -LiteralPath $artifactsDir) {
    Get-ChildItem -LiteralPath $artifactsDir -Directory -Filter "tmp-*" -ErrorAction SilentlyContinue |
        Where-Object { $_.CreationTime -lt $evidenceCutoff } |
        ForEach-Object {
            try {
                $size = (Get-ChildItem $_.FullName -Recurse -File -ErrorAction SilentlyContinue |
                    Measure-Object -Property Length -Sum).Sum
                Remove-Item -LiteralPath $_.FullName -Recurse -Force -ErrorAction Stop
                $cleanedBytes += $size
                Write-Host "  removed stale evidence: artifacts/$($_.Name)" -ForegroundColor DarkGray
            } catch {
                Write-Host "  failed to remove artifacts/$($_.Name): $_" -ForegroundColor Yellow
            }
        }
}

# ④ web/wasm 日志轮转：各保留最近 10 个
foreach ($subDir in @("webgl", "wasm-lua")) {
    $subPath = Join-Path $projectRoot "artifacts\logs\$subDir"
    if (Test-Path -LiteralPath $subPath) {
        $subLogs = Get-ChildItem -LiteralPath $subPath -File -Filter "*.log" -ErrorAction SilentlyContinue |
            Sort-Object CreationTime -Descending
        if ($subLogs.Count -gt 10) {
            $subLogs | Select-Object -Skip 10 | ForEach-Object {
                $cleanedBytes += $_.Length
                Remove-Item -LiteralPath $_.FullName -Force -ErrorAction SilentlyContinue
                # 也清理配套的 .status 和 .err 文件
                Remove-Item -LiteralPath "$($_.FullName).status" -Force -ErrorAction SilentlyContinue
                Remove-Item -LiteralPath "$($_.FullName).err" -Force -ErrorAction SilentlyContinue
            }
            Write-Host "  rotated $subDir logs: kept 10, removed $($subLogs.Count - 10)" -ForegroundColor DarkGray
        }
    }
}

$cleanedMB = [math]::Round($cleanedBytes / 1MB, 1)
if ($cleanedBytes -gt 0) {
    Write-Host "  cleaned up $cleanedMB MB of stale artifacts" -ForegroundColor Green
} else {
    Write-Host "  no stale artifacts found" -ForegroundColor DarkGray
}

$runGates = -not $MvpOnly

# 文档一致性门（P4）：编号唯一、H1、索引可达、相对链接、历史路径、UTF-8 可解码。
# 放在最前面：它只读文本、约 1 秒，且 `-MvpOnly` 也跑（文档失真不该被快速路径跳过）。
Write-Step "Documentation consistency"
$docsScript = Join-Path $projectRoot "scripts/verify/check-docs.ps1"
if (Test-Path $docsScript) {
    $docsOutput = & powershell -NoProfile -ExecutionPolicy Bypass -File $docsScript
    $docsExit = $LASTEXITCODE
    $docsOutput | ForEach-Object { Write-Host $_ }
    if ($docsExit -ne 0) {
        throw "check-docs.ps1 failed (exit code $docsExit)"
    }
}
# 静默留痕门：把"已经决定要留痕的点"钉住。测试门守不住"只缺留痕"这类缺陷——见 docs/60 §10.7 的
# 反面对照（把已修的静默点合并回旧写法，全量测试照样全绿）。纯文件检查、毫秒级，与文档门同段：
# 放在最前面，`-MvpOnly` 也跑，留痕约定不该被快速路径跳过。
Write-Step "Silent-failure traces"
$tracesScript = Join-Path $projectRoot "scripts/verify/check-silent-traces.ps1"
if (Test-Path $tracesScript) {
    $tracesOutput = & powershell -NoProfile -ExecutionPolicy Bypass -File $tracesScript
    $tracesExit = $LASTEXITCODE
    $tracesOutput | ForEach-Object { Write-Host $_ }
    if ($tracesExit -ne 0) {
        throw "check-silent-traces.ps1 failed (exit code $tracesExit)"
    }
}

# vendor 补丁基线门：`[patch.crates-io]` 的 5 个 fork 携带 wasm32 必需补丁，升级上游后
# 极易被覆盖丢失（extern "C-unwind" 回归会令 wasm 上宿主函数返回 Err 的 pcall 崩溃，
# 而 boot 依旧全绿）。纯文件检查、约 1 秒，与文档门同段：`-MvpOnly` 也跑，
# 补丁丢失不该被快速路径跳过。明细见 engine/vendor/README.md。
Write-Step "Vendor patch baseline"
$vendorGateScript = Join-Path $projectRoot "scripts/verify/check-vendor-patches.ps1"
if (Test-Path $vendorGateScript) {
    $vendorOutput = & powershell -NoProfile -ExecutionPolicy Bypass -File $vendorGateScript
    $vendorExit = $LASTEXITCODE
    $vendorOutput | ForEach-Object { Write-Host $_ }
    if ($vendorExit -ne 0) {
        throw "check-vendor-patches.ps1 failed (exit code $vendorExit)"
    }
}

# 工具链锁定门：`stable` 漂移会让"本地能过、CI 不能过"变成不可复现的口水仗。
# 纯文件检查、约 1 秒，`-MvpOnly` 也跑；安全工具/网络相关的门不放这里。
Write-Step "Toolchain lock"
$toolchainLockScript = Join-Path $projectRoot "scripts/verify/check-toolchain-lock.ps1"
if (Test-Path -LiteralPath $toolchainLockScript) {
    $lockOutput = & powershell -NoProfile -ExecutionPolicy Bypass -File $toolchainLockScript
    $lockExit = $LASTEXITCODE
    $lockOutput | ForEach-Object { Write-Host $_ }
    if ($lockExit -ne 0) {
        throw "check-toolchain-lock.ps1 failed (exit code $lockExit)"
    }
}
else {
    throw "check-toolchain-lock.ps1 missing: $toolchainLockScript"
}

# 版本一致性门（P0-02）：引擎 workspace 版本与两个示例工程的 project.toml 版本必须一致，
# 且编辑器新建工程模板不再写死字面量版本（历史漂移：header.rs 写死 0.1.0 而引擎已 0.1.1）。
# 纯文件检查、约 1 秒，与文档门同段：`-MvpOnly` 也跑，版本漂移不该被快速路径跳过。
Write-Step "Project version consistency"
$versionGateScript = Join-Path $projectRoot "scripts/verify/check-project-version.ps1"
if (Test-Path -LiteralPath $versionGateScript) {
    $versionOutput = & powershell -NoProfile -ExecutionPolicy Bypass -File $versionGateScript
    $versionExit = $LASTEXITCODE
    $versionOutput | ForEach-Object { Write-Host $_ }
    if ($versionExit -ne 0) {
        throw "check-project-version.ps1 failed (exit code $versionExit)"
    }
}
else {
    throw "check-project-version.ps1 missing: $versionGateScript"
}
# 降耦合门（`docs/62` P1 ③④⑤）：wasm 图里的音频后端、`rustlua-scene` 图里的渲染/UI 层、
# 契约层 `rustlua-lua-api` 的本仓依赖，三条都必须为 0。跑三次 `cargo tree`（已编译时约 1-2 秒），
# 放在编译之前——它只看依赖图，不改产物；退化时能直接指到"哪一层又被拖回来了"。
Write-Step "Decoupling gate"
$decouplingScript = Join-Path $projectRoot "scripts/verify/check-decoupling.ps1"
if (Test-Path -LiteralPath $decouplingScript) {
    $decoupleOutput = & powershell -NoProfile -ExecutionPolicy Bypass -File $decouplingScript
    $decoupleExit = $LASTEXITCODE
    $decoupleOutput | ForEach-Object { Write-Host $_ }
    if ($decoupleExit -ne 0) {
        throw "check-decoupling.ps1 failed (exit code $decoupleExit)"
    }
}
else {
    throw "check-decoupling.ps1 missing: $decouplingScript"
}


# 供应链安全门：放编译之前。缺工具/DB 时 check-security 退 3（环境受限），
# 这里保留该读数并在结尾把总门降级为 PARTIAL，而不是 throw 成"代码红了"。
if ($Security) {
    Write-Step "Supply-chain security"
    $securityScript = Join-Path $projectRoot "scripts/verify/check-security.ps1"
    if (-not (Test-Path -LiteralPath $securityScript)) {
        throw "check-security.ps1 missing: $securityScript"
    }
    $securityArgs = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $securityScript, "-FetchForkUpstream")
    $sbomCandidate = Join-Path $projectRoot "artifacts\reports\rustlua.cdx.json"
    if (Test-Path -LiteralPath $sbomCandidate) {
        $securityArgs += @("-SbomPath", $sbomCandidate)
    }
    $securityOutput = & powershell @securityArgs 2>&1
    $securityExit = $LASTEXITCODE
    $securityOutput | ForEach-Object { Write-Host $_ }
    if ($securityExit -eq 1) {
        throw "check-security.ps1 failed (exit code 1); see $securityOutput"
    }
    if ($securityExit -eq 3) {
        Write-Host "security_blocked: tools or advisory database unavailable (not a pass)" -ForegroundColor Yellow
        $securityBlocked = $true
    }
}

if ($runGates) {
Push-Location $engineRoot
try {
    Write-Step "Cargo metadata"
    Invoke-Checked cargo @("metadata", "--no-deps", "--format-version", "1") -QuietOutput

    Write-Step "Rust formatting"
    Invoke-Checked cargo @("fmt", "--all", "--", "--check")

    # 优化项 10：Lua API 文档桩的再生成一致性。事实源是**运行期 API 面基线**
    # （`engine/crates/rustlua-scripting/src/api_surface_snapshot/lua_api_surface.json`），所以这一步查的是
    # 「文档桩与真正暴露给脚本的那 275 个函数是否还是同一份清单」，外加「待补桩数没有变多」。
    # 缺 node 时打印原因后跳过——沿用本仓库口径，跳过绝不写成通过。
    Write-Step "Lua API doc stubs"
    if (Get-Command node -ErrorAction SilentlyContinue) {
        Invoke-Checked node @((Join-Path $projectRoot "scripts/docgen/gen-lua-api-docs.js"), "--check")
        # **L2「覆盖清单」门已退休**（B8）：它量的是"`web-runtime.js` 里那批 Lua 宿主补丁
        # 盖掉了引擎多少实现"（56 个函数式覆盖点 / 23 处整表替换 / 275 个函数里 77 个因此在 web
        # 上是 nil）。**补丁层随旧世界一起消失**，于是"谁盖了谁"这个问题整体不存在了 ——
        # 名字面与行为面的分叉源（`clock.now()` 少三个字段那类）也一并消失。
        # 门脚本 `scripts/check-web-api-overrides.mjs` 已删除；它依赖的那本账
        # `plugins/webgl/lua_api_overrides.json（已随 B8 删除；逐表分类见 tasks/plan/M73-02）` **降级为历史记录**（第二个消费者
        # `scripts/lua-api-overrides-report.mjs` 仍能把它导出成工单，读的是历史，不读产物）。
        # **M72-02 的跨宿主哈希契约已退休**（B8）：它从 `web-runtime.js` 里抽**两份抄本**
        # （JS 解算 + 内嵌 Lua 垫片）互相对拍并与金标比。那两份抄本随旧世界一起消失，而它守的
        # 那份关切（"地表别换个样子"）**已经在 Rust 侧**：同一组金标数字由
        # `engine/crates/rustlua-ecs/src/tilemap.rs`（crate 内测试）与
        # `engine/crates/game/examples/jhyt_m72_tile_variant_determinism.rs` 钉着（已核对到位）。
        # 门脚本 `scripts/check-tile-hash-parity.mjs` 一并删除。
        # M73-00：两条**不需要 wasm 产物**的运行时回归门。它们是"改坏了看不出来、而且要
        # 等一次完整微信打包才能发现"的那类规则的唯一快速判据，因此和上面几条同层：每次质量门都跑。
        #   ① 资源路由：主体 mainpack/pack、远端 URL、远端缓存的发布标识与旧版清理。
        #      抓的是"首屏音频指向不存在的 pack/ 路径"与"换版本后旧缓存永久遮蔽新内容"。
        #   ② wasm 兜底通道：首选通道失败/抛错时必须真的用兜底通道的结果，而不是一律判失败。
        Write-Step "WeChat asset routing (adapter unit)"
        Invoke-Checked node @((Join-Path $projectRoot "scripts/test/wechat-asset-routing-test.js"))
        #   ①′ boot 闭包自测：`check-boot-closure.js` 有两条通路（散文件 / 主包容器认领），
        #      而真产物的 boot 条目恰好全是散文件（loose=7/container=0）——没有这条自测，
        #      "容器认领"那条分支只有等到换产物形态才会被走到。
        Write-Step "WeChat boot closure (self-test)"
        Invoke-Checked node @((Join-Path $projectRoot "scripts/build/check-boot-closure.js"), "--self-test")
        #   ①″ 跨端路径黄金对拍（JS 侧）：与 `cargo test -p rustlua-infra --test asset_routing_golden`
        #      读**同一份** `scripts/test/fixtures/asset-routing-golden.json`。两侧都绿才算
        #      "键的口径一致"；各绿各的只证明两份测试各说各话（症状是真机白图）。
        Write-Step "Cross-host path routing golden table (js)"
        Invoke-Checked node @((Join-Path $projectRoot "scripts/test/asset-routing-golden-test.js"))
        Write-Step "WeChat wasm bridge fallback (unit)"
        Invoke-Checked node @((Join-Path $projectRoot "scripts/test/wasm-bridge-fallback-test.js"))
        #   ③ 固定时间步：把 `advanceFixedSteps` 从 web-runtime.js **抽出源码**直接跑，
        #      判据对齐 Rust `FixedTimeStep::advance`（追帧上限/整数步丢弃/长跑不漂移/高刷与低刷）。
        #      "逻辑速度与刷新率解耦"全靠它，写错不会崩、只会慢半拍或快半拍。
        Write-Step "Web fixed timestep (unit)"
        Invoke-Checked node @((Join-Path $projectRoot "scripts/test/web-fixed-step-test.js"))
        #   **④ 已退休**（B8）：原"64 位变体哈希 oracle 对拍"的被测物是 `web-runtime.js` 里那版
        #   "两个 32 位字"的 JS 实现，它随旧世界消失；oracle（旧 BigInt 实现）本来就在测试内联。
        #   同一份算法的**生产侧是 Rust**，金标由上面那两条 Rust 门钉着；测试 `scripts/web-u64-hash-test.js`
        #   与被测物一起删除。
        #   ⑤ 输入线格式键表：批量 ABI 把键名从"每帧上线"变成"位图 + 固定键表"，于是多了一个
        #      静默失效面——游戏绑定了某个键而键表里没有它，那个键就永远收不到。静态对三份来源
        #      （Rust 键表 / 预置层 bindings / 宿主 DOM 键名归一），构建前即可判红。
        Write-Step "Input frame ABI key table (static parity)"
        Invoke-Checked node @((Join-Path $projectRoot "scripts/verify/check-input-abi-parity.mjs"))
        #   ⑥ flaky skip 账本：CI 用 RUSTLUA_SKIP_CI_FLAKY=1 少跑的用例，必须逐条有原因/读者/
        #      恢复条件/复审日期（scripts/verify/flaky-skips.json）。门扫所有读该变量的代码，
        #      拦三种静默失效：新加 skip 不登记、登记的读者/发放点被删、skip 代码只跳名字不打。
        Write-Step "Flaky CI skip ledger (static parity)"
        Invoke-Checked node @((Join-Path $projectRoot "scripts/verify/check-flaky-skips.mjs"))
    } else {
        Write-Host "  [SKIP] 未找到 node：Lua API 文档桩门、微信资源路由门、wasm 兜底通道门、固定时间步门、输入键表门与 flaky skip 账本门未运行（不是通过）" -ForegroundColor Yellow
    }

    # ---------- 发布闭包（容器内容）：此前没有任何自动化入口 ----------
    # `assetc closure` 是"容器文件都在、但计划里某条资源没进容器"的唯一拦截点：启动场景刚好不用
    # 那条资源时，缺件不会掀翻启动，只会在玩到一半白图。它此前只在发布机上真有产物时才跑
    # （`apply-bundle-layout.ps1` / `verify-wechat-minigame.ps1`），总门与 CI 都覆盖不到。这里补两条：
    #   ① 合成一份 grouped 产物 → 正例 + 三个反例（改坏容器 / 删容器 / 多容器），零依赖、秒级；
    #   ② 微信产物存在时，把整份发布包静态验收拉进总门（此前总门**完全不碰**微信产物）。
    Write-Step "Release closure (grouped containers, regress)"
    $assetcExe = $null
    foreach ($closureProfile in @("release", "debug")) {
        $closureCandidate = Join-Path $projectRoot "engine\target\$closureProfile\assetc.exe"
        if (-not $assetcExe -and (Test-Path -LiteralPath $closureCandidate -PathType Leaf)) {
            $assetcExe = $closureCandidate
        }
    }
    $closureTestArgs = @(
        "-NoProfile", "-ExecutionPolicy", "Bypass",
        "-File", (Join-Path $projectRoot "scripts\test\test-release-closure.ps1")
    )
    if ($assetcExe) { $closureTestArgs += @("-Assetc", $assetcExe) }
    Invoke-Checked powershell $closureTestArgs

    # 微信发布包：总门此前从不验收它（只有 `package.ps1` 那一次 layout 检查）。产物在就跑静态验收
    # （`assetc closure` + boot 闭包 + 主包配额 + 上传白名单），产物不在就明说"没跑"——
    # 跳过写成通过是这个项目反复踩过的坑。
    if (-not (Get-Command Resolve-GameBundle -ErrorAction SilentlyContinue)) {
        . (Join-Path $projectRoot "scripts/lib/bundle-discovery.ps1")
    }
    $wechatBundle = Resolve-GameBundle -Platform "wechat-minigame" -WorkspaceRoot $projectRoot
    if ($wechatBundle) {
        Write-Step "WeChat mini-game package (static release closure)"
        Invoke-Checked powershell @(
            "-NoProfile", "-ExecutionPolicy", "Bypass",
            "-File", (Join-Path $projectRoot "scripts\verify\verify-wechat-minigame.ps1"),
            "-BundleRoot", $wechatBundle, "-SkipRuntime"
        )
    }
    else {
        Write-Host "  [SKIP] 没有微信小游戏产物：发布包验收未运行（不是通过；先跑 scripts\build\package.ps1 -Platform wechat-minigame）" -ForegroundColor Yellow
    }

    # M72-05 装饰散布的**确定性**门。事实源在 Python（`tools/decor_kit.py` 的 `scatter()`），所以这条
    # 只能在 Python 侧跑——Rust 侧的 `jhyt_m72_scatter_decor` 钉的是数量、层带、渲染与物理，读的是
    # 盘上结果，"换种子会不会跟着变"它管不着。三段各判一件事：同种子两次逐字节相同（真确定）、
    # 换种子内容必须变（否则散布根本没读 seed，上面那个"相同"是空的）、盘上装饰段 == 现算段
    # （有人手改了装饰、或改了地表却没重撒，红在这里而不是等目验发现）。
    # 路径一律给绝对值：这份工具的输入按 `__file__` 定位，在哪个 cwd 下跑都是同一份结果（曾经不是——
    # places.lua 走的是相对 cwd 的默认路径，换目录跑就静默少一份留白输入，见 gen_manor_scene 的注释）。
    # 缺 python 时按本仓库口径打印原因后跳过，绝不把"跳过"写成通过。
    Write-Step "Decor scatter determinism (python)"
    # `Get-Command python` 会被 WindowsApps 占位符骗过：找得到，执行却返回 9009。
    # 探一次真能跑的解释器，优先 python，回退本机惯例的 py。
    $pythonExe = $null
    foreach ($candidate in @("python", "py")) {
        if (-not (Get-Command $candidate -ErrorAction SilentlyContinue)) { continue }
        & $candidate -c "import sys" *> $null
        if ($LASTEXITCODE -eq 0) { $pythonExe = $candidate; break }
    }
    if ($pythonExe) {
        # Windows 控制台默认代码页是 GBK，工具输出的中文在编码不匹配时抛 UnicodeEncodeError——
        # 那是门自己的环境问题，不是散布判红。
        $env:PYTHONIOENCODING = "utf-8"
        # 碎屑库自己的门：26 张 PNG（13 形状 × 本体+镜像；旧说法 24 张是 M72-05 那一档）条目齐全、尺寸 == 目录、每个不透明像素都在 `terrain_ramp.json`
        # 覆盖的色族里、alpha 只有 0/255。Rust 那条只比尺寸与明度档，"手改过一张图的像素"只有这里看得见。
        Invoke-Checked $pythonExe @(
            (Join-Path $projectRoot "examples/game_jhyt/tools/gen_decor_art.py"), "--check"
        )
        # 逐图三条门，一张图一轮跑齐：散布确定性 → 建筑占地 solid 对拍 → 逐图量化读数。
        #
        # ① `set_scatter_decor.py --check`：盘上装饰段 == 按当前地表现算的装饰段（M72-05 只有
        #    manor 一条（204 件），village 326 件、mountain_path 245 件、combat_arena 340 件、
        #    cold_spring_cave 57 件、interior_forge 43 件、interior_herb 41 件；洞穴那张件数少是它的可走地面只有
        #    144 格，两张室内图更少是 `decor_kit.SCENE_DECOR` 把八件户外条目摘掉后可撒的地表本来就窄
        #    （散布按可走格数 × 档位概率出），而 `decor_kit.CATALOG` 是所有图共用的形状库——
        #    改了库没重撒，红在这里而不是等目验发现）。
        # ② `building_kit.py --check`：M72-11 铺量——"每格 solid 都要有可视依据"这条规则的**实现**，
        #    而 `jhyt_m72_building_footprint` 是它的第二份独立实现（Rust 不 import Python）。
        #    现算的应实心格集必须 == 盘上 `solid` 串（缺 0 悬空 0）。为什么每张图都要——`solid`
        #    同时是物理（每个实心格一个静态 cuboid），手改一格、或改了建筑贴图却没重跑
        #    `--apply`，都只有这条能在目验之前抓住。
        # ③ `scene_readings.py --gate`：plan §验收方式 要求**逐图**量化（同屏可见瓦种类 ≥24、
        #    每屏装饰 median ≥40），而 `jhyt_m72_manor_terrain` 按设计只管 manor 那张图集、
        #    `jhyt_m72_scatter_decor` 只管 manor 那份散布。这条是"任意一张图都能判"的那一份，
        #    判据常量与那两扇门同名同值。分档表 `MIN_DECOR_OBJECTS` 里没有的场景直接红
        #    （室内/洞穴要按 plan §风险 另定分档，工具不替它猜；寒泉洞、铁匠铺与药庐那三条都是
        #    **按实测登记 = 40**（洞穴 57/57/57、铁匠铺 43/43/43、药庐 41/41/41），没有为它们降过
        #    门槛；`main` 仍是旧 6×1 档、还没接进这张循环）。
        foreach ($decorScene in @("manor", "village", "mountain_path", "combat_arena", "cold_spring_cave", "interior_forge", "interior_herb")) {
            Invoke-Checked $pythonExe @(
                (Join-Path $projectRoot "examples/game_jhyt/tools/set_scatter_decor.py"),
                (Join-Path $projectRoot "examples/game_jhyt/scenes/$decorScene.scene"),
                "--check"
            )
            Invoke-Checked $pythonExe @(
                (Join-Path $projectRoot "examples/game_jhyt/tools/building_kit.py"),
                (Join-Path $projectRoot "examples/game_jhyt/scenes/$decorScene.scene"),
                "--check"
            )
            Invoke-Checked $pythonExe @(
                (Join-Path $projectRoot "examples/game_jhyt/tools/scene_readings.py"),
                (Join-Path $projectRoot "examples/game_jhyt/scenes/$decorScene.scene"),
                "--gate"
            )
        }
    } else {
        Write-Host "  [SKIP] 未找到可用的 python/py：装饰库色表门、装饰散布确定性门、建筑占地 solid 对拍门与逐图量化读数门未运行（不是通过）" -ForegroundColor Yellow
    }

    # M75-01：AI 能力层的 tool 面快照门。tool 名/参数/风险档一改，外部 MCP 客户端拿到的
    # 契约就漂移；口径与 `lua_api_surface.json` 相同：先红，显式
    # `AI_TOOL_SURFACE_BLESS=1 cargo test -p rustlua-editor-ai surface::tests` 刷基线再提交。
    Write-Step "AI tool surface snapshot"
    Invoke-Checked cargo @("test", "-p", "rustlua-editor-ai", "--lib", "surface::tests::tool_surface_matches_baseline")

    # M75-01：feature gate 隔离。`ai-bridge` 只在原生编辑器默认开启；关掉后 editor 的
    # 依赖图里必须**完全没有** AI crate（它带协议线程与文件写入 tool，绝不能漏进 wasm/游戏包）。
    Write-Step "AI bridge feature gate"
    $editorTree = & cargo tree -p editor --no-default-features -e normal --prefix none 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "cargo tree -p editor --no-default-features failed: $editorTree"
    }
    if ($editorTree | Select-String -SimpleMatch "rustlua-editor-ai") {
        throw "feature gate broken: rustlua-editor-ai 出现在 --no-default-features 的 editor 依赖图里"
    }

    # M76-05/06：UI 发布门。扫工程内全部 `.ui.json` + provenance 边车：坏 JSON、
    # 生成计划缺来源、未审阅被引用、单件/总包超体积都是 error。三宿主未测只打印读数，
    # 不写成通过（报告里固定是 untested）。反面对照在该测试文件的单元测试里。
    Write-Step "UI asset provenance & review gate"
    Invoke-Checked cargo @(
        "test", "-p", "editor", "--test", "ui_asset_gate", "--", "--nocapture"
    )

    # M76-①：上面那条门只有**消费端**——`ui/hosts.json` 里没条目时它永远报 untested，绿得毫无意义。
    # 这一步是生产端：把 `ui/matrix.ui.json` 走编辑器侧确定性导出桥（`ui_scene_export`）喂进真实的
    # 运行时布局解算与网格绘制，逐键对拍预览器矩形（流向子树键此刻只对"运行时解出正尺寸"，
    # 因为入库这版的预览器不推进它们）、逐图块对拍绘制次序；两项检查全绿才写产物。
    # 产物只写 %TEMP% 的固定文件名：入库那份是 bless 后提交的，这一步证明"当前代码在这台机器上跑得
    # 出同样绿"，不改动工作区（10-25 实测：现跑产物与入库那份 13583 字节逐位相同，且在 HEAD 干净检出
    # 与主工作区两棵树上跑出的字节也相同——产物里不记预览数字，所以与预览器哪一版无关）。它**不与**入库
    # 那份对账——那是上面 ui_asset_gate 用 revision 做的事；这里只买"生产端没有烂掉"。
    Write-Step "M76-① PC UI host evidence"
    $pcUiEvidencePath = Join-Path $env:TEMP "rustlua-m76-pc-ui-evidence.json"
    Invoke-Checked cargo @(
        "run", "--quiet", "-p", "editor", "--example", "m76_ui_matrix", "--",
        "--document", "ui/matrix.ui.json", "--output", $pcUiEvidencePath
    )

    # M72-10b：UI 资源契约（Python 只读量具）。九宫格半句查 8 份 prefab 的 54 行
    # `_9s`：必须带 nine_slice、等于 manifest、且 UiLayout 宽高装得下边距；
    # 图标半句查 93 张 32×32 的硬边/6bit/非纯色/非噪点。审美精修仍是外部委托，
    # 这里只挡机器能判的退化；`--self-test` 带四种反面对照。
    Write-Step "UI asset contract (python)"
    if (Get-Command python -ErrorAction SilentlyContinue) {
        $env:PYTHONIOENCODING = "utf-8"
        Invoke-Checked python @(
            (Join-Path $projectRoot "examples/game_jhyt/tools/ui_asset_census.py"),
            "--self-test"
        )
        Invoke-Checked python @(
            (Join-Path $projectRoot "examples/game_jhyt/tools/ui_asset_census.py"),
            "--gate"
        )
    } else {
        Write-Host "  [SKIP] 未找到 python：九宫格契约门与图标技术门未运行（不是通过）" -ForegroundColor Yellow
    }
    if (-not $SkipClippy) {
        Write-Step "Clippy"
        Invoke-Checked cargo @("clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings")
    }

    # -Quick 跳过：`cargo doc --workspace` 是编译重活，而紧跟的对外符号面门**依赖它刚生成的产物**
    # （见上面的 WP12 注释）。跳过 doc 却照跑符号面门，等于拿上一次的旧产物当证据——陈旧证据不算证据。
    if (-not (Test-QuickSkip "Rustdoc (workspace, warnings are errors) + 对外 Rust 符号面门")) {
        # 优化项 10：rustdoc 必须是**零告警**的——失效的 intra-doc 链接是文档站点里点不开的按钮，
        # 而 `-D warnings` 让"新增一条断链"当场判红（存量已在本批清零）。
        # `$env.RUSTDOCFLAGS` 只在这一步生效，用 try/finally 复原，避免污染后面的 build/test。
        Write-Step "Rustdoc (workspace, warnings are errors)"
        $previousDocFlags = $env:RUSTDOCFLAGS
        try {
            $env:RUSTDOCFLAGS = "-D warnings"
            Invoke-Checked cargo @("doc", "--no-deps", "--workspace", "--lib")
        } finally {
            $env:RUSTDOCFLAGS = $previousDocFlags
        }

        # WP12 阶段 0：crate 对外 Rust 符号面的基线门。它读的就是上面 `cargo doc` 刚生成的产物
        # （`engine/target/doc/<crate>`），所以必须紧跟在那一步之后；判据是"路径 + 项的种类 + 方法还挂在
        # 哪个类型上"逐项与本地基线相同。
        # 抓的是模块拆分这一类事故：`impl` 块搬进 `pub mod` 子模块、`pub use` 漏了一项，都会多/少一条路径。
        # 缺 node 时按本仓库口径打印原因后跳过，绝不把"跳过"写成通过。
        # `baselines/` 在 .gitignore 里（本地基线不入库），干净 CI 必然没有这份 JSON；
        # 没有基线就显式跳过并说明，而不是让每个 push 都红在"baseline_missing"。
        # M77-01：渲染器搬进 `rustlua-gpu` 之后这条门盯**两份**基线（`engine` + `rustlua-gpu`），
        # 少任何一份都算"未运行"——只比对 engine 会漏掉整套 GPU 符号面。
        Write-Step "Crate public Rust surface"
        $baselinesDir = if ($env:RUSTLUA_BASELINES_DIR) { $env:RUSTLUA_BASELINES_DIR } else { Join-Path $projectRoot "baselines" }
        $pubSurfaceBaselines = @(
            (Join-Path $baselinesDir "engine-pub-surface.json"),
            (Join-Path $baselinesDir "rustlua-gpu-pub-surface.json")
        )
        $missingBaselines = @($pubSurfaceBaselines | Where-Object { -not (Test-Path -LiteralPath $_) })
        if (Get-Command node -ErrorAction SilentlyContinue) {
            if ($missingBaselines.Count -eq 0) {
                Invoke-Checked node @((Join-Path $projectRoot "scripts/verify/engine-pub-surface.mjs"), "--check")
            } else {
                Write-Host "  [SKIP] 缺少本地基线 $($missingBaselines -join '、')（baselines/ 不入库；先跑 node scripts/verify/engine-pub-surface.mjs --update，或设 RUSTLUA_BASELINES_DIR）：对外 Rust 符号面基线门未运行（不是通过）" -ForegroundColor Yellow
            }
        } else {
            Write-Host "  [SKIP] 未找到 node：对外 Rust 符号面基线门未运行（不是通过）" -ForegroundColor Yellow
        }
    }

    # -Quick 跳过：这一步量的是 pre-commit hook 自己的选路逻辑，不是这棵树的状态；它会另起临时
    # git 仓库跑 cargo fmt，与「树是不是红的」无关，故不进快速读数。
    if (-not (Test-QuickSkip "Local pre-commit hook regression")) {
        # 优化项 11：本地 pre-commit 门自身的回归。fmt/clippy 步查的是代码，这一步查的是
        # "提交前到底会不会有人被挡住"——hook 的选路、拦截与逃生门逻辑回归了，前面所有步都发现不了。
        # 它只用临时 git 仓库 + cargo fmt（零依赖、秒级）。退出码沿用本仓库口径：
        # 0=通过；3=环境受限（缺 git/cargo/sh）打印原因后跳过，绝不把"跳过"说成"通过"；其余=判红。
        Write-Step "Local pre-commit hook regression"
        $hookGateScript = Join-Path $projectRoot "scripts/test/test-git-hooks.ps1"
        $previousPreference = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $hookGateOutput = & powershell -NoProfile -ExecutionPolicy Bypass -File $hookGateScript 2>&1
        $hookGateCode = $LASTEXITCODE
        $ErrorActionPreference = $previousPreference
        $hookGateOutput | ForEach-Object {
            $line = $_.ToString()
            Write-Host $line
            Add-Content -Path $logPath -Value $line -Encoding UTF8
        }
        if ($hookGateCode -eq 3) {
            Write-Host "git_hooks_skipped reason=environment_limited exit=3（缺 git/cargo/sh；见上面一行原因）" -ForegroundColor Yellow
        }
        elseif ($hookGateCode -ne 0) {
            throw "Command failed (exit code $hookGateCode): powershell -File $hookGateScript"
        }
    }

    if (-not $SkipTests) {
        # M75-02：确定性回放门。fixture 里的 tool 序列（建实体/加组件/写字段/撤销）
        # 在 CI 上无 LLM 重放，两次独立会话必须得到逐字节同一场景——把"AI 动作"里
        # 唯一非确定的 LLM 留在环外，进门的只是它产出的 tool 调用。
        Write-Step "AI deterministic replay"
        Invoke-Checked cargo @("test", "-p", "editor", "--bin", "editor", "--all-features", "ai_host::tests::replay_fixture_is_deterministic_across_fresh_sessions")

        Write-Step "Workspace tests"
        Invoke-Checked cargo @("test", "--workspace", "--all-features")

        # M74-03：compute 接缝 + GPU 粒子两档对拍（10k × 120 步）。
        # 0=通过；3=本机没有 compute（例如 CI runner / 只有 WebGL2 的环境）——**显式跳过，
        # 不是通过**；设 RUSTLUA_REQUIRE_GPU=1 时把跳过升级为红（发布门用）。
        Write-Step "Compute / GPU particles"
        Push-Location $engineRoot
        try {
            & cargo run --quiet -p game --example jhyt_m95_gpu_particles
            $computeExit = $LASTEXITCODE
        }
        finally {
            Pop-Location
        }
        if ($computeExit -eq 3) {
            if ($env:RUSTLUA_REQUIRE_GPU -eq "1") {
                throw "compute gate skipped (exit 3) 但 RUSTLUA_REQUIRE_GPU=1：这台机器必须提供 compute"
            }
            Write-Host "compute_gate_skipped reason=no_compute exit=3（CPU 粒子仍是默认与回滚档；不是通过）" -ForegroundColor Yellow
        }
        elseif ($computeExit -ne 0) {
            throw "compute gate FAILED (jhyt_m95_gpu_particles exit $computeExit)"
        }
        else {
            Write-Host "compute_gate_ok (capacity=10000 steps=120 max_position_error<1e-4)" -ForegroundColor Green
        }
    }

    if (-not $SkipBuild) {
        if ($Release) {
            Write-Step "Release build"
            Invoke-Checked cargo @("build", "--workspace", "--release")
        }
        else {
            Write-Step "Debug build"
            Invoke-Checked cargo @("build", "--workspace")
        }
    }

    Write-Step "Git diff check"
    Push-Location $projectRoot
    try {
        Invoke-Checked git @("diff", "--check")
    }
    finally {
        Pop-Location
    }

    # docs/54 §7：性能红线的**判定逻辑**本身要有测试。这段自测证明"超出 p95 +5% / p99 +10% /
    # 内存 +10% / 几何量变化"都会让比较脚本以非零退出（此前它把超阈定义为成功）。
    Write-Step "Baseline comparison thresholds"
    Push-Location $projectRoot
    try {
        Invoke-Checked powershell @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", (Join-Path $projectRoot "scripts/test/test-compare-baseline.ps1"))
    }
    finally {
        Pop-Location
    }

    # M27-03：docs/55 阶段 0 立下的 soak **判据脚本**自己坏了五天没人发现（`6bcdabd` 起首行少了
    # 前括号；读 JSON 又没写 `-Encoding UTF8`），因为调用方是"验收当天手点的人"，不在任何门里。
    # 这段自测把它的四种判读钉住：稳态放行、持续增长退 2、大锯齿不误报、坏输入不静默退 0。
    Write-Step "Soak analyzer self test"
    Push-Location $projectRoot
    try {
        Invoke-Checked powershell @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", (Join-Path $projectRoot "scripts/test/test-analyze-soak.ps1"))
    }
    finally {
        Pop-Location
    }

    Write-Step "Game project check"
    Push-Location $projectRoot
    try {
        Invoke-Checked cargo @("run", "--quiet", "--manifest-path", "engine/Cargo.toml", "-p", "game", "--", "--project", $defaultGameProject, "--check")
    }
    finally {
        Pop-Location
    }

    # docs/55 §1 第 3 条：组件 schema 审计。`editor --check` 现在会逐个校验真实场景/Prefab 里的
    # 组件负载；已注册组件的字段级漂移（schema_errors）必须为 0，未注册组件按 component_no_schema
    # 单独计数（扩展组件本来就可以没有 schema，不算失败）。
    Write-Step "Editor project check + component schema audit"
    Push-Location $projectRoot
    try {
        $editorCheck = Invoke-Captured cargo @("run", "--quiet", "--manifest-path", "engine/Cargo.toml", "-p", "editor", "--", "--project", $defaultGameProject, "--check")
        $editorCheckOutput = $editorCheck.Output
        $editorCheckExit = $editorCheck.ExitCode
        $editorCheckOutput | ForEach-Object { Write-Host $_ }
        if ($editorCheckExit -ne 0) {
            throw "editor project check failed (exit code $editorCheckExit)"
        }
        $editorCheckText = $editorCheckOutput -join "`n"
        if ($editorCheckText -notmatch "editor_project_ok") {
            throw "editor project check output has no editor_project_ok line"
        }
        if ($editorCheckText -notmatch "schema_errors=0") {
            throw "editor project check reports component schema drift (schema_errors != 0)"
        }
        # Second real project (2D + 3D + UI + prefabs): auditing ONE project is how "required
        # field is wrong" drift stayed invisible. example game_api is the 3D-heavy one.
        $secondProject = Join-Path $gameRoot "game_api"
        if (Test-Path (Join-Path $secondProject "project.toml")) {
            $secondCheck = Invoke-Captured cargo @("run", "--quiet", "--manifest-path", "engine/Cargo.toml", "-p", "editor", "--", "--project", $secondProject, "--check")
            $secondOutput = $secondCheck.Output
            $secondExit = $secondCheck.ExitCode
            $secondOutput | ForEach-Object { Write-Host $_ }
            if ($secondExit -ne 0) {
                throw "editor project check failed for game_api (exit code $secondExit)"
            }
            $secondText = $secondOutput -join "`n"
            if ($secondText -notmatch "schema_errors=0") {
                throw "editor project check reports component schema drift on game_api (schema_errors != 0)"
            }
        }
    }
    finally {
        Pop-Location
    }

    # -Quick 跳过：以下三步都是「跑起编辑器进程」的重活（空间迁移报告 / 性能基线 / GUI smoke），
    # 都要完整构建编辑器。它们量的是能力与性能，不是这棵树当下红不红。
    if (-not (Test-QuickSkip "Spatial migration report + Editor performance baseline + Editor GUI smoke")) {
        # docs/54 §8-⑧：空间迁移命令必须**每次**都给出可归档的报告 artifact，并且示例工程里
        # 一个实体都不该被拒（refused=0）、一处旧设计都不该命中（removed=0）。把报告落盘后逐条断言，
        # 因此"迁移干净"是每次 verify 重新证明的事实，而不是一次性的手工检查。
        Write-Step "Spatial migration report"
        Push-Location $projectRoot
        try {
            $migrationReportPath = Join-Path $baselinesRoot "spatial-migration-report.txt"
            $migration = Invoke-Captured cargo @("run", "--quiet", "--manifest-path", "engine/Cargo.toml", "-p", "editor", "--", "--project", $defaultGameProject, "--migrate-spatial", "--migrate-report", $migrationReportPath)
            $migrationOutput = $migration.Output
            $migrationExit = $migration.ExitCode
            $migrationOutput | ForEach-Object { Write-Host $_ }
            if ($migrationExit -ne 0) {
                throw "spatial migration dry-run failed (exit code $migrationExit)"
            }
            if (-not (Test-Path -LiteralPath $migrationReportPath)) {
                throw "spatial migration report was not written: $migrationReportPath"
            }
            $migrationText = Get-Content -Raw -LiteralPath $migrationReportPath
            if ($migrationText -notmatch "spatial_migration_ok") {
                throw "spatial migration report has no spatial_migration_ok summary line"
            }
            if ($migrationText -notmatch "refused=0") {
                throw "spatial migration report lists refused entities (not losslessly migratable)"
            }
            if ($migrationText -notmatch "removed=0") {
                throw "spatial migration report lists removed legacy designs (docs/30 compatibility table)"
            }
        }
        finally {
            Pop-Location
        }

        # docs/55 阶段 0 / ER-01：编辑器自身的性能基线必须**每次**给出可归档报告，并且每个场景的
        # 不变量（10k 实体扁平化、100k 日志有界、历史预算、后台扫描、保存/加载往返一致）都要通过。
        # 命令在有场景不达标时以退出码 2 结束，因此这里既拿到数字也拿到判定。
        Write-Step "Editor performance baseline"
        Push-Location $projectRoot
        try {
            $editorBaselinePath = Join-Path $baselinesRoot "editor-baseline.txt"
            $editorBaseline = Invoke-Captured cargo @("run", "--quiet", "--manifest-path", "engine/Cargo.toml", "-p", "editor", "--", "--perf-baseline", "--perf-report", $editorBaselinePath)
            $editorBaselineOutput = $editorBaseline.Output
            $editorBaselineExit = $editorBaseline.ExitCode
            $editorBaselineOutput | ForEach-Object { Write-Host $_ }
            if ($editorBaselineExit -ne 0) {
                throw "editor performance baseline failed (exit code $editorBaselineExit); report: $editorBaselinePath"
            }
            if (-not (Test-Path -LiteralPath $editorBaselinePath)) {
                throw "editor performance baseline report was not written: $editorBaselinePath"
            }
            $editorBaselineText = Get-Content -Raw -LiteralPath $editorBaselinePath
            if ($editorBaselineText -notmatch "result=ok") {
                throw "editor performance baseline report has no result=ok summary line"
            }
            if ($editorBaselineText -notmatch "failed=0") {
                throw "editor performance baseline report lists failed scenarios"
            }
        }
        finally {
            Pop-Location
        }

        # docs/55 阶段 0「GUI smoke 清单落地」：无窗口的**合成输入整帧**用例（层级点击/多选/范围选、
        # 键盘快捷键、Scene View 真实指针拖动、未保存保护弹窗、DPI×resize 组合下的行高与画布几何）。
        # 它覆盖的是"整帧 + 真实 egui 事件路径"，与下面的基线（`--perf-baseline`）互补：
        # 基线量性能与几何，这里量**交互语义**。有用例失败时退出码为 2。
        Write-Step "Editor GUI smoke (synthetic full frames)"
        Push-Location $projectRoot
        try {
            $guiSmokePath = Join-Path $baselinesRoot "editor-gui-smoke.txt"
            $guiSmoke = Invoke-Captured cargo @("run", "--quiet", "--manifest-path", "engine/Cargo.toml", "-p", "editor", "--", "--gui-smoke", "--gui-report", $guiSmokePath)
            $guiSmokeOutput = $guiSmoke.Output
            $guiSmokeExit = $guiSmoke.ExitCode
            $guiSmokeOutput | ForEach-Object { Write-Host $_ }
            if ($guiSmokeExit -ne 0) {
                throw "editor GUI smoke failed (exit code $guiSmokeExit); report: $guiSmokePath"
            }
            if (-not (Test-Path -LiteralPath $guiSmokePath)) {
                throw "editor GUI smoke report was not written: $guiSmokePath"
            }
            $guiSmokeText = Get-Content -Raw -LiteralPath $guiSmokePath
            if ($guiSmokeText -notmatch "result=ok") {
                throw "editor GUI smoke report has no result=ok summary line"
            }
            if ($guiSmokeText -notmatch "failed=0") {
                throw "editor GUI smoke report lists failed cases"
            }
        }
        finally {
            Pop-Location
        }
    }

    # docs/54 §8-⑨：Web 平台的可重复证据。两层，退出码语义严格区分：
    #   ① 浏览器契约层（verify-webgl.ps1）：真实浏览器 + Node 静态服务器跑资源获取/解析、
    #      WebGL2 能力、存储降级与缺失资源诊断。不需要 wasm 目标，本机可跑。
    #   ② 产物层（verify-webgl-bundle.ps1）：真实发布包在浏览器里启动（wasm 模块 + Lua 引导）。
    #      需要 wasm32 目标与已发布产物。
    # 0 = 通过；1 = **真失败**（判红）；2/3 = 环境受限（无浏览器/node/产物）——打印原因后跳过，
    # 绝不把"跳过"说成"通过"。
    if ($Web) {
        Write-Step "Web platform evidence"
        $webEvidenceRoot = Join-Path $projectRoot "artifacts\logs\webgl"
        New-Item -ItemType Directory -Force -Path $webEvidenceRoot | Out-Null
        # 子进程 + **独立日志文件**：这两个脚本内部会起 node 静态服务器与无头浏览器（长命孙进程），
        # 把它们的 stdout 接进本进程的管道会让质量门永远等不到 EOF（本轮实际踩到）。因此让子进程
        # 自己写文件、只取退出码，再把要点抄进质量日志。
        $runWebEvidence = {
            param([string]$Script, [string]$Label, [string[]]$ExtraArgs = @())
            $logFile = Join-Path $webEvidenceRoot ("gate-" + $Label + "-$timestamp.log")
            $statusFile = "$logFile.status"
            # **有界等待**，不用 `-Wait`：`-Wait` 会等到整个进程树结束，而无头浏览器偶尔不自行退出，
            # 于是质量门会一直挂在那里（本轮实际踩到）。这里自己轮询 + 超时即杀，并把它当失败
            # ——宁可判红，也不要"卡住"。
            $timeoutSeconds = 600
            $startedAt = Get-Date
            $argumentList = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $Script, "-StatusFile", $statusFile) + $ExtraArgs
            $process = Start-Process -FilePath "powershell" `
                -ArgumentList $argumentList `
                -PassThru -RedirectStandardOutput $logFile -RedirectStandardError "$logFile.err"
            if (-not $process.WaitForExit($timeoutSeconds * 1000)) {
                Write-Host "web evidence '$Label' timed out after ${timeoutSeconds}s; killing it" -ForegroundColor Yellow
                Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
                # 无头浏览器/静态服务器可能是**孙进程**：父进程被杀后仍会占着端口与文件句柄。
                # 只杀**本脚本那一类**进程（本轮的浏览器 + 只跑 `serve-static.js` 的 node），绝不按
                # 进程名一网打尽——开发机上还有别的 node（曾把 agent 自己的 node 也杀掉过）。
                Get-Process msedge, chrome -ErrorAction SilentlyContinue |
                    Where-Object { $_.StartTime -gt (Get-Date).AddMinutes(-15) } |
                    Stop-Process -Force -ErrorAction SilentlyContinue
                Get-CimInstance Win32_Process -Filter "Name = 'node.exe'" -ErrorAction SilentlyContinue |
                    Where-Object { $_.CommandLine -and $_.CommandLine -like "*serve-static.js*" } |
                    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
                return [pscustomobject]@{ ExitCode = 1; StartedAt = $startedAt }
            }
            $lines = if (Test-Path -LiteralPath $logFile) { Get-Content -LiteralPath $logFile } else { @() }
            $lines | Select-Object -Last 12 | ForEach-Object { Write-Host $_ }
            $lines | ForEach-Object { Add-Content -Path $logPath -Value $_ -Encoding UTF8 }
            if (Test-Path -LiteralPath "$logFile.err") {
                $errLines = Get-Content -LiteralPath "$logFile.err" -ErrorAction SilentlyContinue
                if ($errLines) {
                    # 子脚本的错误写在 stderr：**必须**抄进门日志。本轮实际踩到："子脚本自己没 BOM、
                    # WinPS 按 GBK 解码后把字符串终止符吃掉"→ 解析失败、stdout 空、若只看退出码与
                    # 目录里最新摘要，就会读到上一轮的陈旧证据。
                    Write-Host "web evidence '$Label' stderr:" -ForegroundColor Yellow
                    $errLines | Select-Object -First 8 | ForEach-Object {
                        Write-Host "  $_"
                        Add-Content -Path $logPath -Value "  $_" -Encoding UTF8
                    }
                }
            }
            # 退出码**只**认子进程自己写的状态文件：`Start-Process -PassThru -RedirectStandardOutput`
            # 返回的进程对象上 `ExitCode` 是 `$null`（本轮实测），`[int]$null` 静默变 0，于是"子进程
            # 失败"会被当成"通过"——本轮的陈旧摘要事件就是这么发生的。缺失状态文件即判失败。
            $exitCode = 1
            if (Test-Path -LiteralPath $statusFile) {
                $raw = (Get-Content -LiteralPath $statusFile -Raw).Trim()
                $parsed = 0
                if ([int]::TryParse(($raw -replace '^exit=', ''), [ref]$parsed)) {
                    $exitCode = $parsed
                }
            }
            else {
                Write-Host "web evidence '$Label' wrote no status file (treated as failure)" -ForegroundColor Yellow
            }
            return [pscustomobject]@{ ExitCode = $exitCode; StartedAt = $startedAt }
        }
        Push-Location $projectRoot
        try {
            $webScript = Join-Path $projectRoot "scripts/verify/verify-webgl.ps1"
            $webRun = & $runWebEvidence $webScript "browser"
            $webExit = $webRun.ExitCode
            if ($webExit -eq 1) {
                throw "browser contract check FAILED (verify-webgl.ps1 exit 1)"
            }
            if ($webExit -ne 0) {
                Write-Host "webgl_browser_skipped reason=environment_limited exit=$webExit" -ForegroundColor Yellow
            }
            else {
                # 证据必须**这一次**真的落盘：退出码 0 不够，摘要 JSON 的 status 必须是 ok，而且这份
                # 摘要必须是本轮子进程写的（`LastWriteTime >= 子进程启动时刻`）——否则读到的可能是上
                # 一轮遗留的摘要，"通过"就变成了陈旧证据的复读（本轮实际踩到过）。
                $summary = Get-ChildItem -LiteralPath $webEvidenceRoot -Filter "summary-*.json" -ErrorAction SilentlyContinue |
                    Where-Object { $_.LastWriteTime -ge $webRun.StartedAt } |
                    Sort-Object LastWriteTime -Descending | Select-Object -First 1
                if (-not $summary) {
                    throw "browser contract check reported success but wrote no FRESH summary JSON (stale evidence is not evidence)"
                }
                $payload = Get-Content -LiteralPath $summary.FullName -Raw | ConvertFrom-Json
                if ($payload.status -ne "ok") {
                    throw "browser contract summary status is '$($payload.status)' (expected ok): $($summary.Name)"
                }
                Write-Host "webgl_browser_ok status=$($payload.status) wasm_target=$($payload.wasmTarget) browser=$($payload.browserVersion)" -ForegroundColor Green
            }

            $bundleScript = Join-Path $projectRoot "scripts/verify/verify-webgl-bundle.ps1"
            # 产物层的证据要求"真的有产物"：本机**一个 webgl 产物都没有**时就地发布一份
            # （wasm 构建 + wasm-bindgen 胶水）。否则 CI 上这一步永远是 skip，"Web 有可重复质量
            # 证据"就只剩契约层。已存在产物（哪怕有多个候选）时不动它——"拿 A 的包验证 B 并报出
            # 通过"比跳过更糟（与 bundle-discovery 的既有口径一致）。
            . (Join-Path $projectRoot "scripts/lib/bundle-discovery.ps1")
            $existingBundle = Resolve-GameBundle -Platform "webgl" -WorkspaceRoot $projectRoot
            $webglBuilds = @(
                Get-ChildItem -Path (Join-Path $projectRoot "examples") -Recurse -Directory -ErrorAction SilentlyContinue |
                    Where-Object { $_.Parent -and $_.Parent.Name -eq "webgl" }
            )
            if (-not $existingBundle -and $webglBuilds.Count -eq 0) {
                Write-Step "Publish WebGL bundle (no published artifact found)"
                Invoke-Checked powershell @(
                    "-NoProfile", "-ExecutionPolicy", "Bypass",
                    "-File", (Join-Path $projectRoot "scripts\build\package.ps1"),
                    "-ProjectRoot", $defaultGameProject, "-Platform", "webgl"
                )
            }
            $bundleRun = & $runWebEvidence $bundleScript "bundle"
            $bundleExit = $bundleRun.ExitCode
            if ($bundleExit -eq 1) {
                throw "published webgl bundle FAILED verification (verify-webgl-bundle.ps1 exit 1; boot / scene-verify / blank check - see bundle-summary-*.json)"
            }
            if ($bundleExit -ne 0) {
                Write-Host "webgl_bundle_skipped reason=environment_limited_or_no_bundle exit=$bundleExit (先跑 scripts\build\package.ps1 -Platform webgl)" -ForegroundColor Yellow
            }
            else {
                # 产物层同样要**新鲜**的摘要：`boot` 必须是可接受状态，且画面采样必须 > 0
                # （`drawnSamples == 0` 就是"页面能跑却黑屏"）。
                $bundleSummary = Get-ChildItem -LiteralPath $webEvidenceRoot -Filter "bundle-summary-*.json" -ErrorAction SilentlyContinue |
                    Where-Object { $_.LastWriteTime -ge $bundleRun.StartedAt } |
                    Sort-Object LastWriteTime -Descending | Select-Object -First 1
                if (-not $bundleSummary) {
                    throw "webgl bundle check reported success but wrote no FRESH bundle-summary JSON"
                }
                $bundlePayload = Get-Content -LiteralPath $bundleSummary.FullName -Raw | ConvertFrom-Json
                if ($bundlePayload.boot -notin @("ok", "booted", "running")) {
                    throw "webgl bundle boot state is '$($bundlePayload.boot)' (expected ok/booted/running): $($bundleSummary.Name)"
                }
                # 非黑屏判据有**两个写者**，按页面自报的形态选：
                #   · 旧 JS 世界出帧 ⇒ `drawnSamples`（它自己的像素采样）＋ `sceneVerify`；
                #   · 引擎宿主出帧（`jsHostSkipped`）⇒ `gpuState` 的 `presented…`（present 后的引擎侧
                #     像素回读），而旧世界那两个读数**必然缺席**——这是设计，不是回归。
                # **两者都缺席才是黑屏**：所以这里不是"跳过断言"，而是"换一个有写者的断言"。
                # （R2 纪律：不能把"缺席"当"正常"。）
                $jsHostSkipped = ($bundlePayload.PSObject.Properties.Name -contains 'jsHostSkipped') -and [bool]$bundlePayload.jsHostSkipped
                $gpuStateNow = if ($bundlePayload.PSObject.Properties.Name -contains 'gpuState') { [string]$bundlePayload.gpuState } else { '' }
                if ($jsHostSkipped) {
                    if ($gpuStateNow -notlike 'presented*') {
                        throw "webgl bundle: 旧 JS 世界未参与，且引擎宿主没有给出 presented 证据（gpu=$gpuStateNow）⇒ 有绘制者但画面全空：$($bundleSummary.Name)"
                    }
                    Write-Host "webgl_bundle_engine_host_ok boot=$($bundlePayload.boot) wasm_bytes=$($bundlePayload.wasmBytes) gpu=$gpuStateNow ui=$($bundlePayload.uiElements) missing_host_api=$($bundlePayload.missingCount)（旧世界未参与，非黑屏判据改用引擎侧像素证据）" -ForegroundColor Green
                } else {
                    if ($null -eq $bundlePayload.drawnSamples -or $bundlePayload.drawnSamples -le 0) {
                        throw "webgl bundle reports no painted canvas samples (blank frame): drawn=$($bundlePayload.drawnSamples)"
                    }
                    # 场景切换防回归门：verify-webgl-bundle 默认在 URL 里带 rustlua-scene-verify=1，
                    # 断言首帧把当前场景重新走一遍 applySceneSwitch 全流程（fetch → parse →
                    # 重绑 Lua → 重跑 on_create）。这里独立于子脚本 exit code 再兜一层，防止
                    # 未来 URL/断言解耦时被无意跳过；旧产物若没升级 web-runtime.js 会是 absent。
                    if ($bundlePayload.PSObject.Properties.Name -contains 'sceneVerify') {
                        if ($bundlePayload.sceneVerify -ne 'done') {
                            throw "webgl scene switch gate FAILED: scene_verify=$($bundlePayload.sceneVerify) detail=$($bundlePayload.sceneVerifyDetail)"
                        }
                    }
                    Write-Host "webgl_bundle_ok boot=$($bundlePayload.boot) wasm_bytes=$($bundlePayload.wasmBytes) drawn=$($bundlePayload.drawnSamples) ui=$($bundlePayload.uiElements) missing_host_api=$($bundlePayload.missingCount) scene_verify=$($bundlePayload.sceneVerify)" -ForegroundColor Green
                }
            }

            # ②′ wgpu 的 WebGL2 后端能力门（WP2 的 GPU 后端技术验证，已进门）：
            # 上面 ①（verify-webgl.ps1）只证明**页面**能拿到 `getContext('webgl2')`；这一步证明
            # **wgpu 自己**能在这条后端上建 surface、拿适配器、建设备、清屏并读回像素。
            # 两者是不同的东西：wgpu 27 在 wasm 上默认**不带** GL 后端（default 只给 webgpu），
            # `Instance::new(&Default::default())` 在浏览器里可能一个适配器都拿不到。
            # 这门直接量 WP5（GPU 2D 渲染器）的地基，地基塌了不该等到渲染器写完才发现。
            # 0=通过；1=真回归；3=这台机器给不出 WebGL2 画布（打印原因后跳过）。
            $gpuProbeScript = Join-Path $projectRoot "scripts/verify/gpu-web-probe.ps1"
            $gpuRun = & $runWebEvidence $gpuProbeScript "gpu-probe"
            $gpuExit = $gpuRun.ExitCode
            if ($gpuExit -eq 1) {
                throw "wgpu WebGL2 backend check FAILED (gpu-web-probe.ps1 exit 1; 见 gate-gpu-probe-*.log)"
            }
            if ($gpuExit -ne 0) {
                Write-Host "gpu_probe_skipped reason=environment_limited exit=$gpuExit (无头实例给不出 WebGL2 画布)" -ForegroundColor Yellow
            }
            else {
                Write-Host "gpu_probe_ok (wgpu 在 WebGL2 后端上建 surface/适配器/设备并可读回)" -ForegroundColor Green
            }

            # ②′ 渲染器门（WP5 第一片）：上面 ② 证明"wgpu 的 GL 后端能用"，这一步证明
            # **本仓那台按 WebGPU 规格写的渲染器**在 WebGL2 上能把一帧画对——真
            # `engine::host::Renderer` 离屏渲染 + 像素按解析期望值精确对拍 + 帧计数对账
            # （draw_calls/triangles/dropped_draws）。能力探针过了不代表渲染器过了：
            # 管线翻译、绑定组布局、深度附件、构造期的存储缓冲都是后者才有的坑。
            # 0=通过；1=真回归（含"构造期被设备拒绝"这种已知未完成态）；3=给不出 WebGL2 画布。
            $renderProbeRun = & $runWebEvidence $gpuProbeScript "render-probe" -ExtraArgs @("-Stage", "render")
            $renderProbeExit = $renderProbeRun.ExitCode
            if ($renderProbeExit -eq 1) {
                throw "engine renderer on WebGL2 FAILED (gpu-web-probe.ps1 -Stage render exit 1; 见 gate-render-probe-*.log)"
            }
            if ($renderProbeExit -ne 0) {
                Write-Host "render_probe_skipped reason=environment_limited exit=$renderProbeExit (无头实例给不出 WebGL2 画布)" -ForegroundColor Yellow
            }
            else {
                Write-Host "render_probe_ok (真渲染器在 WebGL2 上离屏出帧且像素精确)" -ForegroundColor Green
            }

            # ③′ 完整玩法剧本门（P0-②）：verify-webgl-bundle -DriveJhyt 驱动 URL
            # `?rustlua-drive=jhyt`，在真实浏览器里跑完"标题确认 → 新游戏(manor) →
            # 移动 → 背包/任务面板开合 → 山庄↔云泽村真实场景切换 → 回山庄"的完整玩法
            # 链路，M1-M6 里程碑全落定才写 data-rustlua-drive=done（bundle-summary 的
            # driveJhyt 字段）。这步把"产物层验收 ≠ 完整剧本"的诚实标注升级成机器门：
            # 玩法内容动不了、动线不对、UI 开合失效、场景切换丢失实体，都会在这里判红。
            # 与常规 bundle 步互补（那步带 scene-verify，这一步自动关掉它走完整玩法）。
            $jhytRun = & $runWebEvidence $bundleScript "bundle-jhyt" -ExtraArgs @("-DriveJhyt")
            $jhytExit = $jhytRun.ExitCode
            if ($jhytExit -eq 1) {
                throw "webgl complete playthrough FAILED (verify-webgl-bundle.ps1 -DriveJhyt exit 1; playthrough milestones did not all land - see bundle-summary-*.json driveJhyt=)"
            }
            if ($jhytExit -ne 0) {
                Write-Host "webgl_playthrough_skipped reason=environment_limited_or_no_bundle exit=$jhytExit" -ForegroundColor Yellow
            }
            else {
                $jhytSummary = Get-ChildItem -LiteralPath $webEvidenceRoot -Filter "bundle-summary-*.json" -ErrorAction SilentlyContinue |
                    Where-Object { $_.LastWriteTime -ge $jhytRun.StartedAt } |
                    Sort-Object LastWriteTime -Descending | Select-Object -First 1
                if (-not $jhytSummary) {
                    throw "webgl complete playthrough reported success but wrote no FRESH bundle-summary JSON"
                }
                $jhytPayload = Get-Content -LiteralPath $jhytSummary.FullName -Raw | ConvertFrom-Json
                if ($jhytPayload.PSObject.Properties.Name -contains 'driveJhyt') {
                    if ($jhytPayload.driveJhyt -eq 'skipped') {
                        # B8：剧本写者（旧 JS 世界）已删除。子脚本在页面自报 `legacy JS world skipped`
                        # 时判黄跳过并写 `skipped`；`absent`/`failed` 仍然是真红（请求了剧本却连自报都没有）。
                        Write-Host "webgl_playthrough_skipped reason=writer_retired (旧 JS 世界写者已随 B8 删除；引擎侧玩法证据见 tasks/plan/M73-00 WP3)" -ForegroundColor Yellow
                    }
                    elseif ($jhytPayload.driveJhyt -ne 'done') {
                        throw "webgl complete playthrough gate FAILED: driveJhyt=$($jhytPayload.driveJhyt) detail=$($jhytPayload.driveJhytDetail)"
                    }
                    else {
                        Write-Host "webgl_playthrough_ok driveJhyt=$($jhytPayload.driveJhyt) milestones=$($jhytPayload.driveJhytDetail)" -ForegroundColor Green
                    }
                }
            }

            # ③″ L3 行为契约层（docs/51）：把桌面那份 `lua_api_contract.lua` 注进**这个已打完
            # 宿主补丁的活体 Lua 状态**再跑一遍，与 PC 的 transcript 逐行对拍。它是三层门里唯一
            # 能抓"名字两边都在、行为不一样"的那层——L1 比的是注册表（wasm 与桌面同源，空转），
            # L2 比的是名字清单（整表替换那 77 个 nil 它只知道"没了"）。
            # 判红口径沿用子脚本：`unavailable`（产物没接 Lua 桥、报告首行三选一都对不上）同样
            # 算红——"没数据所以放行"等于给门开洞。无浏览器/无产物才是环境受限（exit 2/3）。
            $contractRun = & $runWebEvidence $bundleScript "bundle-api-contract" -ExtraArgs @("-ApiContract")
            $contractExit = $contractRun.ExitCode
            if ($contractExit -eq 1) {
                throw "webgl API contract FAILED (verify-webgl-bundle.ps1 -ApiContract exit 1; PC 与打完补丁的 web 活体行为不一致，或契约报告首行读不出来 - 见 bundle-summary-*.json 的 apiContract 与 artifacts/logs/webgl/api-contract-*.txt)"
            }
            if ($contractExit -ne 0) {
                Write-Host "webgl_api_contract_skipped reason=environment_limited_or_no_bundle exit=$contractExit" -ForegroundColor Yellow
            }
            else {
                $contractSummary = Get-ChildItem -LiteralPath $webEvidenceRoot -Filter "bundle-summary-*.json" -ErrorAction SilentlyContinue |
                    Where-Object { $_.LastWriteTime -ge $contractRun.StartedAt } |
                    Sort-Object LastWriteTime -Descending | Select-Object -First 1
                if (-not $contractSummary) {
                    throw "webgl API contract reported success but wrote no FRESH bundle-summary JSON"
                }
                $contractPayload = Get-Content -LiteralPath $contractSummary.FullName -Raw | ConvertFrom-Json
                if ($contractPayload.apiContract -eq 'skipped') {
                    # B8：L3 契约注入的是旧 JS 世界的活体 Lua 状态，写者已删除 ⇒ 没有被测物。
                    # 等价覆盖在微信 harness（同一份契约文件、经 wasm 桥，l3_api_contract=ok）。
                    Write-Host "webgl_api_contract_skipped reason=writer_retired (L3 契约写者已随 B8 删除；等价覆盖见微信 harness l3_api_contract)" -ForegroundColor Yellow
                }
                elseif ($contractPayload.apiContract -ne 'ok') {
                    throw "webgl API contract gate FAILED: api_contract=$($contractPayload.apiContract) cases=$($contractPayload.apiContractCases)"
                }
                elseif ($contractPayload.apiContractCases -le 0) {
                    throw "webgl API contract reports ok with cases=$($contractPayload.apiContractCases) (0 条用例的绿不算证据)"
                }
                else {
                    Write-Host "webgl_api_contract_ok cases=$($contractPayload.apiContractCases) report=$($contractPayload.apiContractReport)" -ForegroundColor Green
                }
            }

            # ③ 新 EH（try_table/exnref）跨浏览器层：本产物是纯新 EH，低于门槛的引擎连编译都
            # 过不去（iOS Safari 18.4 / Chrome·Edge 137 / Firefox 131）。这一层只**编译**真实
            # wasm（不跑游戏），因此与启动层正交：启动层可能因运行期 bug 判红，但只要模块能编译，
            # EH 覆盖就是绿的。runner 逐个验本机所有浏览器；无浏览器/无产物时以 3 显式跳过。
            # Safari 无法在 Windows 自动化——runner 会打印 Mac 上的手动验证流程。
            $ehScript = Join-Path $projectRoot "scripts/verify/verify-webgl-eh.ps1"
            if (Test-Path -LiteralPath $ehScript) {
                $ehRun = & $runWebEvidence $ehScript "eh"
                $ehExit = $ehRun.ExitCode
                if ($ehExit -eq 1) {
                    throw "webgl new-EH cross-browser check FAILED (verify-webgl-eh.ps1 exit 1)"
                }
                if ($ehExit -ne 0) {
                    Write-Host "webgl_eh_skipped reason=environment_limited_or_no_browser exit=$ehExit" -ForegroundColor Yellow
                }
                else {
                    $ehSummary = Get-ChildItem -LiteralPath $webEvidenceRoot -Filter "eh-summary-*.json" -ErrorAction SilentlyContinue |
                        Where-Object { $_.LastWriteTime -ge $ehRun.StartedAt } |
                        Sort-Object LastWriteTime -Descending | Select-Object -First 1
                    if (-not $ehSummary) {
                        throw "webgl new-EH check reported success but wrote no FRESH eh-summary JSON"
                    }
                    $ehPayload = Get-Content -LiteralPath $ehSummary.FullName -Raw | ConvertFrom-Json
                    if ($ehPayload.fail -gt 0 -or $ehPayload.timeout -gt 0 -or $ehPayload.pass -le 0) {
                        throw "webgl new-EH summary not clean: pass=$($ehPayload.pass) fail=$($ehPayload.fail) timeout=$($ehPayload.timeout)"
                    }
                    Write-Host "webgl_eh_ok tested=$($ehPayload.testedCount) pass=$($ehPayload.pass) (new-EH wasm compiles in all discovered browsers)" -ForegroundColor Green
                }
            }
            else {
                Write-Host "webgl_eh_skipped reason=script_missing ($ehScript)" -ForegroundColor Yellow
            }
        }
        finally {
            Pop-Location
        }
    }

    if ($quick) {
        # 「跳过绝不等于通过」：`-Quick` 的绿灯必须与「跑过全部」区分开——措辞与清单两处一起区分。
        $quickCompleted = $true
        Write-Host "`nQuick gate passed (PARTIAL). Log: $logPath" -ForegroundColor Green
    }
    elseif ($securityBlocked) {
        # 环境受限不是通过：措辞与退出码都要与"全绿"区分。
        Write-Host "`nQuality gate PARTIAL: supply-chain security was BLOCKED (tools/advisory DB unavailable). Log: $logPath" -ForegroundColor Yellow
        exit 3
    }
    else {
        Write-Host "`nQuality gate passed. Log: $logPath" -ForegroundColor Green
    }
}
finally {
    Pop-Location
    # 清单放在 finally：红的读数要说清「哪一步红了」，同样也要说清「还有哪些根本没跑」。
    # 清单由 Test-QuickSkip 的调用点登记，不在这里手写。
    if ($quick -and $skippedByQuick.Count -gt 0) {
        Write-Host "  本轮 -Quick 未运行的步骤（跳过不等于通过）：" -ForegroundColor Yellow
        foreach ($skippedStep in $skippedByQuick) {
            Write-Host "    [SKIP] $skippedStep" -ForegroundColor Yellow
        }
        if (-not $quickCompleted) {
            Write-Host "  （上面只登记到脚本走到的位置：本轮中途判红，后面的 guard 还没走到，清单并不完整）" -ForegroundColor Yellow
        }
        Write-Host "  要拿到可号称「门全绿」的读数：不带 -Quick 跑一次完整 verify.ps1。" -ForegroundColor Yellow
    }
}
}

if ($Mvp -or $MvpOnly) {
    # MVP 退出门（M19-04）：核心剧本/浸泡 headless 回归 + 干净发行目录自测。
    # 清单是**唯一事实来源**：清单里的样例缺失即失败（此前是打印 skip 后 continue，
    # 会让"少跑几个样例"看起来仍然绿灯，等于把质量门变成摆设）。
    Write-Step "MVP headless regression suite"
    $examples = @(
        "jhyt_smoke",
        "jhyt_m17",
        "jhyt_m17_npc",
        "jhyt_m17_dialogue",
        "jhyt_m17_shop",
        "jhyt_m18_combat",
        "jhyt_m18_stats",
        "jhyt_m18_build",
        "jhyt_m19_slice",
        "jhyt_m19_soak",
        "jhyt_m19_ui",
        "jhyt_m20_audio",
        "jhyt_m20_gamepad",
        "jhyt_m21_appearance",
        "jhyt_m22_input",
        "jhyt_m22_save_isolation",
        "jhyt_m23_data_validation",
        "jhyt_m24_numeric_alignment",
        # 阶段1（改造计划 M36–M43）：四季作物矩阵与生产闭环门。此前以单样例方式回归，
        # 1.0 集成时补登进 MVP 清单（与磁盘 jhyt_*.rs 一一对应，缺一即门红）。
        "jhyt_m36_regrow",
        "jhyt_m36_economy_sim",
        "jhyt_m37_tool_upgrade",
        "jhyt_m38_fertilizer_quality",
        "jhyt_m39_sprinkler",
        "jhyt_m40_fruit_tree",
        "jhyt_m41_greenhouse",
        "jhyt_m42_shipping_box",
        "jhyt_m43_mastery",
        "jhyt_m25_combat_feedback",
        "jhyt_m26_hud_feedback",
        "jhyt_m27_goals",
        "jhyt_m28_loop",
        "jhyt_m29_season_weather",
        "jhyt_m30_cjk_text",
        # `jhyt_m31_pc_release` 不在这里：它验收的是**打好的 PC 发布包**（`builds/pc`），
        # 干净树上必然没有那个目录，留在 MVP 清单里等于让这道门恒红（红因是"没打包"，
        # 不是"发布包坏了"）。它改登记在下面的 `$releaseExamples`，由"产物在就跑、不在就
        # 明说 [SKIP] 不是通过"的那一段执行——与本文件里微信小游戏发布包那段同一口径。
        # M32 相机收口：相机跟随玩家 + 取景由场景 bounds 锁在地图内（含换图后重绑 bounds）。
        "jhyt_m32_camera",
        # M34-00 存档负向门：map.state/快照版本门会红、provider 报错整批回滚、
        # 100 次磁盘往返逐字段不漂移（soak 的成功路径门只断言"没炸"，踩不到失败分支）。
        "jhyt_m34_save_gates",
        # M34-01 档位接线门：五档关系/五档声望的结构负例、店主阵营声望逐档折扣
        # （含 19/20/39/40/59/60/79/80/100 与负声望）、结局交付的声望门槛、
        # 对话的关系档位分支（真实送礼到档才出现）、档位随存档往返派生。
        "jhyt_m34_relationship_tier",
        # M34-02 节气门：112 天逐日扫描（21 个事件日/91 个空日/循环不错位）、四类消费方
        # 各走真实玩法路径（会期内才听得见的茶叶单子、清明采集 ×2、端午行情 +20%、
        # 夏至门派声望、中秋好感与莲子、重阳一次性点按日再刷、冬至只封数据点名的出口
        # 且回村方向可用 + HUD 中文），节气不入存档（读档后随 day 一起回来）+ 十条负例。
        "jhyt_m34_festival",
        # M34-03 中毒门：docs/33 §10 状态表与 data/combat.lua 的 status 表逐格比对（按 60Hz
        # 固定步换算）、毒蝎真实追击咬中（账本 + hit 日志带状态 + HUD 图标第一次被驱动）、
        # 精确到步（第 120 步第一口/600 步正好 5 口 15 点/到期图标收掉/换图不清/读档必清/
        # DOT 不给硬直与无敌帧/再咬只刷新不重置跳表）、DOT 致死走同一套战败流程、
        # 分片与快挂时钟同 5 跳 + 源码扫真实时间、十条数据负例。
        "jhyt_m34_status",
        # M34-04 招式扩充门：docs/33 §6.1 追风步/守心诀两行与 data/skills.lua 逐格比对（秒×60=固定步）、
        # 真实按键 H 走满 3 格 + 扣 15 内力 + 15 步无敌吃掉一整刀、贴墙夹到可走格（一格不通不收钱不进冷却）、
        # Z 格挡正/背/不格挡三档对照（10→3 / 10→10 / 10→10，日志 damage/absorbed）、每 60 步扣一笔与
        # 内力不足自动落防、倒地即落防且 capture 不含 guard、30/120Hz 同一节奏、底座分派负例、
        # 十一条数据负例、HUD 两个新原因的中文词条与动作表 dash/guard 两行。
        "jhyt_m34_skills",
        # M34-05 茶棚门：膳房拒收茶饮配方（no_station/no_building/station_level 三种理由各钉一格，
        # 含 HUD 中文词条）、23:50 开工 20 分钟跨到次日 00:10（未到点收不到、到了点一次收走，
        # 茶叶 2 → 茶汤 1 实数）、社交乘子 ×1 → ×1.1（茶棚 2 级）→ ×1.2（当日饮茶）与
        # 8→8/6/10 三档具体分数、第二杯 tea_today 不扣物、同日礼物限次不被乘子绕开、
        # 存读档后茶汤库存/tea_day 锚点/茶棚历史最高等级三者一致、拆了茶棚门不重新锁上、
        # 七条数据负例走 validate_data（与 game --check 同一批校验器）。
        "jhyt_m34_teahouse",
        # M35 遮挡门：运行期 sprite.set 真的写排序层（漏写 = 运行期声明全失效）、
        # 只把三个 actor 探针的 Y 倒过来绘制序必须整体反转（证明排的是 Y 不是 id/纹理巧合）、
        # 方向钉死为"y 越小越靠画面下方 = 越近 = 最后画"（世界 +y 朝上，写反了两边门都会绿）、
        # 场上主角与每个 NPC 的 order 恒等于 -floor(y/2)、夹在 -999 不溢进相邻层、
        # 特效在 foreground 层永远画在所有角色之后。
        "jhyt_m35_depth_sort",
        # M59 已退役（M71）：JSON 预设层整体退役（`assets/ui/prefabs/dialogue.ui.json` 删除、
        # `ui.prefab` 的 JSON render/clear 不再被任何玩法调用），对话面板对账由
        # `jhyt_m71_dialogue_prefab`（黄金表 34 行，A 态 = 开场对话 gu_changfeng_teach）接管；
        # 锚点表与引擎 UiLayoutPreset::ALL 的对账由 m71 门 submission_order 实测背书。
        # M61 引擎桥（`.prefab` 作 UI 预设的运行时载体）：`prefab.spawn` 走 VFS 读预设并把
        # `Text`/`Image`/`UiEvents`/`UiLayout` 落进**活世界**，宿主本帧真正用于绘制的那份矩形
        # 逐值等于预设（含父链偏移）、无 `UiLayout` 的 UI 实体不参与解算也不会被画、命中框与
        # 绘制框同源（矩形内派发一次、矩形外零次）、`prefab.set_text` 写的就是渲染读的那份组件
        # （三条误用各自报错点名）、`prefab.despawn` 只摘**自己那一批**（别批实例与矩形逐值不动，
        # 本批矩形归零），以及换场景后运行期实例的登记表必须是空的（`scene.load` 重建整个 ECS
        # 世界，旧 handle 留着会让 `prefab.despawn` 拿死句柄去新世界删实体而报错，M20-09c 实测），
        # 且门面场景自己的按钮在 spawn/despawn 前后都照常派发（并表/摘表不许污染场景那份快照）。
        "jhyt_m61_prefab_runtime",
        # M62 预设实例的运行期写入面：`prefab.set_rect` 写四边之后，宿主本帧真正用于绘制的那份
        # 矩形跟着变（子节点整棵子树位移、没参与本次写入的节点一动不如一静）、预设里 authored 的
        # anchors 一个字不动（stretch 通栏不许被降级成左上角定宽）、世界的 UiRectLayout 与画面同一个
        # 数、命中框就是新矩形（新位置点得到、旧位置点不到）；`prefab.set_visible` 是子树语义且画与
        # 命中同一次失效（藏行根 ⇒ 行里的图与字一起消失且点不到），显隐相对父（藏父时单独显示子不
        # 生效），显示回来不必重设矩形、不必重写字；重建只动本实例的键（同屏第二个预设实例不受牵连）；
        # 两个函数各自的误用（无 UiLayout / 无节点 / 无句柄）全部点名报错，不许静默 no-op。
        "jhyt_m62_prefab_write",
        # M63 门面（title_menu）版面搬进一份 `.prefab` 的搬运门：一张表同时钉两层——`shown` 层读的是
        # 宿主本帧真正用于绘制的矩形（`ui_rects_by_name`：名字换自那张全局实体名表，场景实体与 spawn
        # 实例因此同源）加上 `ui.*_get` 的动态层读数，迁移前后逐字节相等才谈得上“一像素没动”，而同一块
        # 东西在实体层与动态层**各画一遍**（M22 修过的重影老毛病）当场红；`authored` 层钉的是“预设里写的
        # 就是 `.scene` 里原来写的”（anchors/offsets 串、字与字号与色、贴图与九宫格、绑的回调名），矩形
        # 对上了而九宫格边距丢了也会在这里红；另守标签两层的位置账（场景那份不可见、动态那份才是玩家看到
        # 的，x 实测差 2px，超 4px 判为两边不是同一坐标口径）与载体归属账（每块只允许一层把它画出来）；
        # ④ 另出一笔层序账：段序表把本帧**全部**提交段按先后钉死（实体层内按预设行序 = 槽位升序，
        # 层与层之间再按"骨架在下、动态在上"分档），所以搬运除了
        # "每块矩形对不对"还要钉"谁画在谁上面"。2026-10-07 的绘制序修正把这张表整张重刷了一次
        # （实体图段移到最前，脚本画的高亮/四角/分隔/印章从此浮在背景之上）——这是本批**有意**改画面
        # 的那一笔，八张 `support/*_golden.tsv` 一个字节没动；同世界重 spawn 的 LIFO 回收槽
        # 形状也一并写出（这是 M63b 实测到的、脚本侧必须"一个世界只 spawn 一次"的根因）。
        "jhyt_m63_title_prefab"
        # M65 商店面板（systems/shop.lua）版面搬进一份 `.prefab` 的搬运门，面板五屏的第一屏。
        # ① 逐 id 黄金表两个状态（A=刚打开、B=42 行库存且选中末行 ⇒ 窗口在末段），每块一条
        # `shown`/`hidden` 行，读数与载体无关（矩形/贴图/九宫格/字/字号/颜色），迁移前后逐字节
        # 相等；同一块在实体层与动态层各画一遍（重影）当场红。② 整屏同层：一屏里的块只许全在
        # 动态层或全在实体层，半搬是最坏形状（实体层的图整体画在动态层**之下**，搬进来的那块
        # 会垫到别人底下，而 ① 的矩形读数对层序全瞎）。③ 覆盖账：谁盖住谁，
        # 每块一行 `cover` 也进同一份黄金表（只打印的账改红了不拦人，而层序正是搬运最容易改掉的一维）；
        # 排序按"层档 → 档内序"，层档是引擎不变式（push_scene_ui_images 在 push_ui_* 之前），
        # submission_order 在这类面板屏只报段序读数、不当背书（整屏同层时"白图在贴图之前"只是槽位序
        # 的产物，拿它当规则就会因别人挪一行预设而假红），
        # 本次实测 A 态有 8 块被整屏不透明的 skin_bg 盖住（含选中行高亮），B 态高亮落在 y=1534
        # （面板只到 680），两条既有画面事实照抄进预设、改不改由用户另行裁定。④ 文字着色：动态层
        # 写死白，所以预设里每条 Text 必须是白（① 的表管不到颜色这一维）。⑤ 跨屏相交必须为空集
        # 才允许整屏搬（实测 0 个 ⇒ 搬进预设不会把别人的块压到底下）。
        "jhyt_m65_shop_prefab"
        # M66 暂停菜单（ui/pause_menu.lua）三页共用骨架 → 一份 `.prefab` 三页容器互斥的搬运门。
        # 首跑门红教训：动态层 BTreeMap 按 id 字典序（skin_bg 排在全部同级 image 之后、slot 之前），
        # 预设槽位须复刻该字典序；cover 段行序跨载体不同源（同为层档+档内序，档内动态层=id 字典序、
        # 实体层=槽位号）→ compare_golden 一律排序后逐行按内容比对。
        "jhyt_m66_pause_prefab"
        # M67 背包面板（ui/inventory_ui.lua）全迁一份 `.prefab` 的搬运门。capture 三坑：shown_rows
        # 按 body≠"hidden" 数行（state 对 hidden 也插条目）；补 query_num 编译 E0425；PowerShell `*>`
        # 重定向 stdout 炸 NativeCommandError。关键字典序修正：bp:sel_slot 第 5 字符是 'e' → 在 skin_bg
        # 前被盖；bp:slot1..10 的 'l'>'k' → 在 skin_bg 后不被盖。
        "jhyt_m67_inventory_prefab"
        # M68 任务日志（ui/quest_ui.lua）全迁一份 `.prefab` 的搬运门（提速后第一屏，一次通过）。
        # bullet 二选一：同一矩形叠 Todo/Done 两张 12×12 图 + set_visible 互斥；门 PROBES 用双实体名。
        "jhyt_m68_quest_prefab"
        # M69 出货箱（systems/shipping.lua）全迁一份 `.prefab` 的搬运门。顺手修两个既有 bug：
        # ① inventory_items() 用 ipairs(data) 遍历恒空（slots 稀疏表）→ pairs(data.slots or {})；
        # ② row 循环缺 else 隐藏分支（行数变少留上一帧字）→ 预设化后行不足即 set_visible false。
        # 入口是 open_ui 不是 open；无 key_hint 面板、有 close 按钮与条件滚动箭头。
        "jhyt_m69_shipping_prefab"
        # M70 常驻 HUD（ui/hud.lua）静态骨架全量搬进 `hud.prefab`（45 实体、部分预设混合载体）的
        # 搬运门：黄金表 138 行（此前这里写的 136 是抄来的、没人量过）；compare_golden 排序集合比对；
        # 坑 = 黄金表首行 UTF-8 BOM 沉底错位。
        # M72-09 换九宫格切图后本门加了两处口径：① 的 Panel 读数多 `texture,nine_slice` 两列
        # （否则"画面真的换了贴图"在黄金表里不可见），③ 的不透明判定改成"贴图在表里 **且**
        # Image 染色 alpha ≥ 0.99"（色与图相乘），另加一档实测「导轨档」= 面板底必须提交在金轨之前
        # （slot2/3 对调是黄金表量不到的行序变化，背书只在这档上）。
        "jhyt_m70_hud_prefab"
        # M71 对话面板（world/dialogue.lua）全迁一份 `.prefab` 的搬运门（JSON 预设层退役的
        # 接管门）：黄金表 34 行（A 态 = 开场对话 gu_changfeng_teach，顾长风 4 选项、marker1
        # 选中、立绘 calm）；capture 一次过；verify 一次绿；部分预设混合载体（立绘本体留
        # 动态层，贴图随 NPC/mood 变化、引擎无运行期换贴图 API）② 豁免。
        "jhyt_m71_dialogue_prefab"
        # M72 对标星露谷第 2 轮（P0 引擎地基，`tasks/done/M72-00-*.md`）。
        # M72-01 图集寻址与 `.scene` 编码迁移门：12×8 图集逐格 UV 落在正确的 (col,row) 子矩形、
        # 索引 0 不产出 quad（quad 数被钉死 = 非空格数）；反面对照内建两处——纯数据侧把
        # `atlas_rows` 改回 1 后条目 13 的 V 必须越出 0..1（证明 UV 断言对"多行"不是瞎的），
        # 运行时侧 `tilemap.set_atlas(id,12,1)` 必须报错点名越界的那一格；旧编码（fmt=0）仍解出
        # 6×1 与整幅拉伸 V 且只认 1..=6，重存后带 `tiles_format=1` 与逗号编码；发布数据侧
        # 关掉视口剔除后"渲染格数 == 逐格数出的非空格数"（920）。
        "jhyt_m72_tileset_atlas"
        # M72-02 确定性变体门：8×4/kind1/variant_count=4/seed=0x12345678 的 32 格变体序号逐格等于
        # 金标（与 `rustlua-ecs` 的单测、`preview_scene.py`、web 的 JS+Lua 镜像同一串位）；100 帧
        # 条目序列与 UV 序列逐元素全等；恒等档 id 就是条目、切变体后落进自己那块；反面对照内建
        # 两处——门自带一份**不做雪崩收尾**的 FNV 参考实现并断言它的第一行正是 `3,2,1,0,3,2,1,0`
        # 的周期点阵（少了收尾就是在"确定地画一张马赛克"），以及 §⑥ 把 web 宿主的 Lua 解算镜像
        # 原样喂进真 VM 重解 32 格（镜像里种子 +1 必须换表，否则同值是空的）。
        "jhyt_m72_tile_variant_determinism"
        # M72-02 4-bit autotile 门：8×8 草嵌 4×4 土的 64 格邻居掩码逐格对手写位序表（bit0=y-1、
        # bit1=x+1、bit2=y+1、bit3=x-1，按 index 命名）；内芯 0b1111 必须取块内最后一格（满块瓦），
        # 条目布局 `1+kinds*span+(kind-1)*16+mask` 两块互不越界；地图四角当暴露边界；挖一格空洞
        # 只让两个邻居各少一位、填回逐格复原；一路到渲染的 64 格 UV 等于算术结果。反面对照：
        # 抹掉 bit0 后土块顶行必须 4/4 全变且底行 0 格（plan 点名的"红在顶排交界格"），抹掉 bit2
        # 顶行必须 0 格，互换两个列位顶行必须恰好 2 格错位；§⑧ 再用 web 的 Lua 垫片重解整张图
        # （全草图的 36 个内部格必须读到 0b1111+条目 18，证明它真在读邻居）。
        "jhyt_m72_autotile_mask"
        # M72-02 per-tile tint 门：tint 表幂等（同色复用一个 id）、未染色格 `tint_at` 读回白、
        # 稀疏载荷 `tints=<cellIndex>:<tintId>` + `tints_rgb` 序列化往返逐格同色；overlay 与
        # 世界深度**两条通道**各 6 个顶点都带上那格的颜色；alpha 原样进第 4 分量；`clear_tint`
        # 当帧回白且其他格颜色不动、组件侧 `tilemap.tint` 与顶点色同一份。反面对照：把渲染侧
        # 的 tint 改回硬编码白，"非白格数 = 染色格数"这条正好红（实测 0）；只改世界通道那一处时
        # §② 仍绿、§③ 红 —— 两个赋值点各自被看着。
        "jhyt_m72_tile_tint"
        # M72-03 像素中文字模门：① 工程子集与全量上游字体同判 12px 点阵（`raster_scale=12`），
        # Noto 判不出来，且图集与矢量缓存两处判定一致（`should_render_vector` 读的就是后者）；
        # ② 真跑一帧 jhyt 场景：UI 字形四边形全落 sdf=2 阈值档、矢量通道顶点数必须 0（这条就是
        # 抓住"无条件 push_vector_text_run"那次的牙齿——修前实测漏出 510 个轮廓顶点，同一句字
        # 会被点阵与轮廓各画一遍）、所用图集 texel 逐像素直方图灰阶=0 且 alpha 恒 255；
        # 反面对照同口径跑 Noto/MSDF：四字各 3600+ 灰阶 texel、全亮 texel 只有 1；
        # ③ 12px 排版逐 texel 采样次数恒为 1、四边角全整数；反面对照按实测重定为 14px 档
        # （tex_scale=7/6）——那里分数偏移才真会让边角脱离整数、160 个 texel 被采两次；
        # ④ 工程用到的 1605 字逐字非空点阵，缺字按"全量也没有=改文案 / 全量有=重跑子集"分开
        # 点名，反面对照 🙂 判为缺字且占位方框仍有 39 个亮 texel；
        # ⑤ 1605 字一次预热仍留在 1024² 内，点阵汉字单元 11×11 texel vs 同字 MSDF 75×87（54 倍面积）。
        "jhyt_m72_pixel_font"
        # M72-04 manor 地表图集与色阶门：① 地表层声明 12×9/tile_kinds=6/autotile 开、格子值只有
        # 1..6（`max_tile_id` 在自动瓦片档就是 kind 种数 6，图集容量 108 只在布局里起作用），
        # 关掉视口剔除后 mesh 的条目多重集 == 逐格 `tilemap.atlas_index` 的多重集，去重 5→59 种；
        # ② 一屏可见瓦种类 ≥24 按**外观等价类（图集条目 × 顶点色档）**两个口径读——实机相机 64 种
        # （纯条目 31）、数据侧所有 20×12 窗口最小 25，逐窗口扫而不是只信开机那一帧；
        # ③ 交界 0 格取到满块瓦，天然地貌（草/土/水）之间同外观连排直边 ≤5 格；
        # ④ 色阶门：每个在用料族实测 ≥3 档明度且深→浅单调，四季图先做同位置色置换还原再同判
        # （顺带证明四季变体是"换色不重画"）；⑤ 每个不透明像素等于色阶表、尾部 6 个空槽全透明；
        # ⑥ 白/shaded/sunlit 三档都真的落在满块格上（§② 的多样性确实靠染色打散重复）。
        # 反面对照内建三处——合成"草地嵌 8×8 规则土块"必须报出 >5 格直边（旧图几何读法），
        # 草族整族压成单档必须让色阶门红，从门内色表删掉 #6F8F52 必须点名它。
        "jhyt_m72_manor_terrain"
        # M72-05 装饰散布门：manor 从前只有 15 个非幽灵 sprite（"画面几乎没有东西"）。① 结构契约：
        # 175 件 `role=decor` 各恰好 Sprite+Transform、**无 Collider**、层名 world、order 只取碎屑 -30
        # /中景 12 两档，声明尺寸 == 门内手抄目录 == PNG 实测（三方），12 种形状一种都没白画；
        # ② 每屏对象：640×360 世界单位窗口以 8px 滑过全图取中位数 ≥40（判 median 不判 min——药田
        # 200 格按玩法契约必须留白，最坏那一屏 89% 是空的）；③ 关掉视口剔除后 mesh 的装饰 quad
        # == 文件件数，开剔除后 == 门按 `sprite_may_be_visible` 同判据数出的件数，且两数必须不同
        # （相同即"剔除没生效、第二条断言是空的"）；④ 层带：碎屑压在所有 world sprite 之前、中景
        # 压在树/竹之下；⑤ 每件装饰在**运行期刚体位置**查一次 physics.overlap，取到装饰贴图 = 0。
        # 反面对照四处——目录副本改 1px 即红、装饰改进 actor 带即红（走同一个 band_violations）、
        # 散布换种子必须变（Python 侧 `set_scatter_decor.py --check`）、⑤ 的正向对照拿带 collider 的
        # 实体真取到命中且含其自身（否则那个 0 可能只是查询在空转）。
        "jhyt_m72_scatter_decor"
        # M72-06 前景遮挡与全局光投影门：manor 以前"画面是平的"——没有一层能压住玩家，也没有一个
        # 光向。① 光向事实源 `data/lighting.lua` 自身干净、登记在册且它的校验真的会跑；② 四条负向
        # 对照注入后各自点名（`dir_y=1` 朝天投影 / `tint.a=1` 地上一个洞 / `occluder=10` 刀光被树冠
        # 埋掉 / `min_cover_ratio=0.9` 前景层白加），每条进入前先断言基线干净，免得"还原没还原上"
        # 被下一条的假绿掩盖；③ 拆层结构契约：11 件遮断物（M72-07 把屋顶拆法推广到柴房与五座
        # 可建造建筑后；一茎多株的树按前缀配对，所以件数不等于 `LAYER_SPLITS` 的 8 行）与表
        # 一一配对、无 Collider、
        # 层与 order 同源于 lighting、接缝等式成立、两张分块按行拼回去**逐字节 == 整图**（生成器之外
        # 的第二份独立实现，带"上下对调必须不等"的对照）；④ 覆盖率按**可达**口径扫——可行走位图取自
        # 运行期 `tilemap.is_solid`，920 格逐格 r=4 探针证明 `solid` 真是按格生成的静态碰撞体
        # （359/359 实心格恰好命中 GroundMap、561 空格 0 命中；这两个数从 M72-06 的 360/560 变成
        # 现在这样，是 M72-07 重算占地 + 挖开南门门洞 + 池塘改不规则岸线的净结果），
        # 最深 0.604 必须落在 lighting 的
        # [0.05,0.65] 内，且最深点 teleport 之后 8 帧仍待得住（门按几何判的可达，物理也必须允许）；
        # ⑤ 用运行期 `sprite.set` 把 canopy 改回 world 层，绘制下标必须翻到玩家之前；⑥ 16 件投影贴片
        # 逐件独立复算落点（接触点 + 半偏移，容差 0.001），尺寸 `2r+1` 让"角度等于 sun"成为恒等式
        # ⇒ 最大偏离恰好 0.000000°；"够格而未挂影"必须 0 件；抬层对照证明先后是层带决定的；
        # ⑦ 8 朝向 × 4 帧 = 32 帧的脚下椭圆质心恒在身体中心同一侧（先烘进帧再镜像会把 w/sw/nw
        # 翻到左边，正是 C-1 修掉的那处）；⑧ Y 排序容差 = `depth.lua` 的 `STEP`：Δy=1 不许翻桶、
        # Δy=2 必须恰好翻一桶。跨宿主那条读的是假 ctx 的 `drawImage` 序列
        # （`--check-foreground-order`，复现命令与三段读数在 `tasks/done/M72-06` D 段）。
        "jhyt_m72_foreground_occlusion"
        # M72-07 多格建筑占地与碰撞门（M72-11 起逐图跑，五张图在 `SCENES` 里）：manor 以前"撞空气"
        # ——solid 是按 sprite 的矩形画框标出来的，而建筑贴图四角全透明。① 门**独立复算**一遍
        # `building_kit` 的判据（故意不共享代码）再和盘上 solid 串逐格比：地表天然实心（墙/水）
        # + 有可视依据的 sprite 格 ⇒ 应实心格 == 盘上格，五张图逐格相同、缺 0 悬空 0——
        # 山庄 187+179⇒359、村里 198+142⇒328、山道 343+71⇒400、演武场 180+62⇒239、
        # 寒泉洞 83+13⇒96（洞穴那张 sprite 依据只有 13 格：洞壁与暗河都由瓦自己画成实心）。
        # 顺手量出"按 quad 框与按不透明像素会算出不同占地的建筑"= 山庄 2 栋（BuildTeaShed /
        # ManorHouse）、村里 2 栋（VillageShrine / VillageTeaStall）、山道 1 栋（BrokenBridge）、
        # 寒泉洞 1 栋（CaveShrine）、演武场 0 栋——这条读数从 M72-07 的 0 栋变来，是因为本包给
        # "压进一格"加了 1/4 格门槛（`MIN_CELL_OVERLAP`，摊位/山石那类贴图描边 1px 压进相邻格
        # 不算实心），差的是那一小条而不是"贴图没画满"；它只用来挑反面对照 A 的落点，不参与判据。
        # ② 拆层不变量：山庄/村里各 7 栋、山道 4 栋、演武场 5 栋、寒泉洞 1 栋（石龛）拆层件在场、
        # 每栋本体∪屋顶 ≥4 格，且"整图算出来的格集 == 本体∪屋顶"逐格相同（拆层只许换画面）。
        # ③ 门洞：山庄南门 (16,1)(16,2)、村里南门 (19,1)(19,2) 与山门 (10,20)、山道山门
        # (19,1)(19,2)、演武场南门 (9,1)(9,2) 必须**因为规则**被挖掉（在视觉框内 + 盘上确实空）、
        # 每列最低那格贴图实测不透明占比 0.16 ≤ 0.35、4 邻域 BFS 从出生点走得到
        # （可达 550 / 589 / 520 / 681 格）。反面对照的起点会**往洞外挪一格**：村的出生点
        # (19,2) 本身就站在南门洞里，从它起算会让"填洞"变成"把出生点砌进墙里"（可达集缩到 0 的
        # 假信号）——从 (19,3) 起算，填洞后少的正好是那 3 个洞格本身，证明门是叶子不是桥。
        # 寒泉洞 `openings` 是空表（本图唯一的建筑是石龛，洞口那一格落在厅口地面上、不是任何
        # sprite 的通道）：这一节在它身上只剩一条可达读数（144 格），门为此点名"三条判据与填洞
        # 对照在本图没有主体"，**不把空跑打印成通过**。
        # ④ 够格的标准不是名单而是 `lighting.lua` 的两个数（总高 × reach ≥ ¼ 瓦）：山庄 9 栋、
        # 村里 5 栋、山道 3 栋、演武场 2 栋各有且仅有一件 `<Name>Shadow`，平地基的 BuildFoundation、
        # 村里三栋 64px 矮屋（铁匠铺/茶棚/土地庙）与山道那座断桥必须没有；寒泉洞**够格 0 件**
        # （洞内最高一件就是石龛，整图 64px ⇒ reach 7.68 < 8），走同一条判据报"CaveShrine 无投影"
        # 后以 0 件通过——不是为暗场景另开的豁免。
        # ⑤ 运行期对拍：引擎读回的 `tilemap.is_solid` 位图逐格 == 盘上 solid 串（四张 40×23、
        # 寒泉洞 20×12；每格一个静态 cuboid），每张图都从门面 `scene.load` 真切进去再对拍。
        # 反面对照三条且都走同一个函数——道具按 quad 框标实心 ⇒ 悬空 5 / 4 / 3 / 5 / 1 格
        # （山庄东南竹梢、村里石桥是唯一的过河通道、山道松冠、演武场竹丛、寒泉洞火把的火焰那一格：
        # 洞里没有前景遮断道具，所以这条落在"画在人身高的东西不算占地"）、
        # 摘掉拆层屋顶件 ⇒ 缺碰撞 74 / 42 / 4 / 10 / 2 格（屋檐会被人走进屋里）、
        # 把洞填实 ⇒ 出口当场断。
        "jhyt_m72_building_footprint"
        # M72-08 作物阶段图与农田三态门：以前 29 种作物**共用一张三列阶段图**（灵芝和稻谷
        # 长得一模一样，还只有 3 档）。① 资产：`assets/art/crops/` 阶段图 122 张（判据 ≥116），
        # 图集实测 8×29 == 场景 CropGrid 声明，122 个阶段槽按**字节摘要**两两不同且无一空槽，
        # 29×3 抖动槽也都必须有内容；反面对照把稻谷第 2 档配成灵芝第 2 档，同一个函数必须报红。
        # ② 三态分两半。**贴图侧**（后加的，因为只判瓦值时贴图库被并发写回旧版 6 列而门全绿）：
        # FarmGrid/FarmMoist 的场景 atlas_cols == `farm_soil.lua` 的列数 == 盘上 PNG 宽÷32，
        # 两列土面**字节不同**（把干涸配成犁地必须红），湿痕那一列有内容但不许满不透明（≥10%
        # 即判"第三张全幅土面会抹掉垄沟"）。**运行期**：未垦三层全空 → 翻土 soil=1 且无湿痕/tint 白
        # → 浇水 moist=1 **且 tint 非白**（湿润是叠痕×顶点色，不是第三列）→ 日结算 soil=2 干涸
        # + 湿痕回 0 + **tint 必须回白**（不撤色下一轮串色）→ 档位按已浇满天数走：第二天仍第 1 档、
        # 第三天降到第 2 档。
        # ③ 阶段推进：稻谷 4 档/4 天、灵芝 5 档/7 天各自走全程，序列单调不减、终点恰为最后一档，
        # 且成熟日 `watered_days == growth_days`（数值零改动锚点）；同档序号两种作物瓦值必须不同；
        # 29 种寻址三方对拍（门内手抄表 == crop_art.lua == crops.lua 的 stages）。
        # ④ 收成动画：收一格稻谷后逐帧采样 CropGrid，读到 抖1→抖2→抖3→落回真值 共 4 次换格，
        # 相邻间隔 5–7 帧（=100ms ±1 帧），同期 `harvest_sparkle` 在场；净收获个数 == crops.lua yield。
        "jhyt_m72_crop_stages"
        # 阶段2（docs/33 §4.3–4.8、§7.3、§8.3，1.0–1.8）：九个内容系统门。
        # M44 畜牧：6 动物/三类畜舍、未建舍拒养、喂抚好感、吃饱按周期产蛋毛奶肉、
        # 好感≥8 出灵蛋、入箱结算、存档往返。
        "jhyt_m44_animal",
        # M45 钓鱼：4 水域/23 鱼种/3 阶竿、抛竿体力时间消耗、竿阶门槛、季节/时段/天气闸、
        # 鱼入镖局货箱、存档往返。
        "jhyt_m45_fishing",
        # M46 矿脉：30 层/电梯层 5,10,15,20,25,30、逐层下潜解锁+体力扣减、挖矿入包+见底拒绝、
        # 电梯直达、矿石入箱、存档往返。
        "jhyt_m46_mine",
        # M47 料理：膳房建/升级、真实 crafting 链路做菜（扣料→计时→收取）、吃菜体力/内力/Buff
        # 按日过期、菜入箱结算、存档往返。
        "jhyt_m47_cooking",
        # M48 NPC 扩编与好感事件：16 NPC/全员 birthday、送礼三档 +40/+20/-10、生日 ×3、
        # 43 条门槛 2/5/8 好感事件（once 发奖）、存档往返。
        "jhyt_m48_npc",
        # M49 节日补足 8 个（每季 2）：112 天逐日 24 事件日、清明 gather×2、端午 sell_bonus+20
        # 真实卖出溢价、冬至 exit_closed 回归、除夕纯标签、跨天派生不入档。
        "jhyt_m49_festival",
        # M50 任务板 3 层：day1 日常 3 单/季初单、接单→备齐→交单发奖、跨天换榜旧单不残留、
        # weekly@day7 / quarterly@day29 锚点、存档往返。
        "jhyt_m50_questboard",
        # M51 图鉴 6 类：crop24/fish23/ore12/herb11/dish16/enemy9、显式 notify 解锁、
        # 50%/100% 里程碑奖励各一次、未知 id 拒绝、存档往返。
        "jhyt_m51_codex",
        # M52 成就 30：30 条注册/初始无解锁、出货/金钱/熟练度/好感/图鉴/时间/节日/战斗
        # 八类谓词触发解锁+发奖、隐藏成就未解锁显示 ???、幂等不重复发奖、存档往返。
        "jhyt_m52_achievement",
        # 阶段3（docs/33 §2.1/§4.10/§5.5/§7.4/§8.4，1.9–2.3）：五个创新系统门。
        # M53 经脉修炼：12 经脉/前置链合法、冲穴耗内力+材料按成功率掷骰（失败耗内不涨进度）、
        # attack/defense 经 stats 常驻、体力/内力上限经 state、on_day_start 内力恢复+重挂、存档往返。
        "jhyt_m53_meridian",
        # M54 医馆经营与疫情：建馆/升级/接诊（处方药材→诊费+声望）、药材不足拒绝不写 last_action、
        # 夏疫触发与诊费乘子上浮、治疫/囤药/闭馆三决策、存档往返。
        "jhyt_m54_clinic",
        # M55 节气历法：24 节气（每季 6）逐日命中/无撞日、季内日与 8 节日窗口对照、修饰查询面、
        # 纯派生（capture/restore/reset 空实现、不入档）。
        "jhyt_m55_solarterm",
        # M56 黑市抉择循环：深夜窗口营业、买卖违禁/稀有货、风险掷骰东窗事发、三选抉择、
        # 黑市声望档位解锁/跌落、存档往返。
        "jhyt_m56_blackmarket",
        # M57 茶会社交：2 心门槛邀人、备茶消耗、双话题按阵营/NPC 系数结算好感与声望、
        # 7 日冷却、存档往返。
        "jhyt_m57_teaparty",
        # M58 多结局：5 条结局（归田/入仕/神医/武林/隐世）查询面判定。
        # 各结局门槛边界（19/20、59/60、549/550XP、111/112天）+ 幂等 + 存档往返 + strict 门绿。
        "jhyt_m58_ending",
        # M60 新手引导重做：16 步序/条件合法、新档第 0 步仍为 till（不抢占）、
        # 真实 farm.act+day.sleep 推进首次种植五步闭环到萝卜成熟收获、strict 门绿。
        "jhyt_m60_tutorial",
        # M74-01 3D 空间音频：距离衰减四档（min 内不衰减/线性/边界归零）、四类非法输入被拒
        # （非有限值/模型反了/零朝向/朝向退化）、声像折回两耳坐标（rodio 自带 1/dist² 不外溢）、
        # 宿主契约（没听者时 play_at 拒绝、模型非法保持原值、sink 路径把 gain 乘进音量）。
        # 纯 `rustlua_audio` 驱动：headless 无音频设备也全程可跑（只算数值，不出声）。
        "jhyt_m93_spatial_audio"
    )
    $exampleDir = Join-Path $engineRoot "crates\game\examples"

    # 0) 「已知跑不了、留作历史记录」的门——和「漏登记」是两件事，必须分开登记。
    #    加进这张表的前提：文件头写清了「为什么跑不了」与「谁接管了这份覆盖」，
    #    否则正确的动作是修好它或者删掉它，而不是登记进来（登记 = 永久失去这份回归）。
    #    `jhyt_m59_ui_prefab`：M71 把 JSON 预设层整体退役（`assets/ui/prefabs/*.ui.json` 删除、
    #    `ui/prefab.lua` 的 NAMES 清空），本门依赖的 `ui.prefab` render/audit 已不存在；
    #    对话面板的对账由 `jhyt_m71_dialogue_prefab`（运行期 `.prefab`，黄金表 34 行）接管。
    #    文件头有退役声明，文件保留以便追溯 M59 当时的口径。它仍在盘上、仍参与编译门，
    #    只是不进 MVP 执行清单。
    $retiredExamples = @(
        "jhyt_m59_ui_prefab"
    )
    # 「验收的是打包产物」的门——和「退役」也是两件事：它们**仍然有效、仍然必须过**，
    # 只是前提是磁盘上有发布包。放这里而不是 `$examples`，是因为 MVP 清单的隐含契约是
    # 「干净树上全部可跑」；把需要产物的门混进去，红因会永远是"没打包"，久了就没人看它。
    # 执行口径见下面第 3 段：产物在 → 真跑并判红；产物不在 → 明打 [SKIP]（不是通过），
    # 并指路 `scripts\build\package.ps1 -Platform pc`。与本文件微信发布包那段同一口径。
    $releaseExamples = @(
        # PC 发布包四类契约：存档/玩家设置写用户目录（`%APPDATA%\<app>`）、升级不覆盖玩家设置、
        # 设备后端与 Lua 面板读同一个根、发布目录卫生（无 .cache/存档/.staging-* 残留）。
        @{ Name = "jhyt_m31_pc_release"; Artifact = "builds\pc"; Hint = "scripts\build\package.ps1 -Platform pc" }
    )
    foreach ($example in $retiredExamples) {
        if (-not (Test-Path -LiteralPath (Join-Path $exampleDir ($example + ".rs")))) {
            throw ("retired example list names $example but the file is gone; drop the entry (it no longer records anything)")
        }
        if ($examples -contains $example) {
            throw ("$example is both retired and in the MVP list; pick one (a retired gate must not run)")
        }
    }
    foreach ($entry in $releaseExamples) {
        $example = $entry.Name
        if (-not (Test-Path -LiteralPath (Join-Path $exampleDir ($example + ".rs")))) {
            throw ("release example list names $example but the file is gone; drop the entry (a missing gate is a lost gate)")
        }
        if (($examples -contains $example) -or ($retiredExamples -contains $example)) {
            throw ("$example is in the MVP/retired list as well as the release list; pick one")
        }
    }

    # 「由总门其它段执行」的门——第四件事：它们**真的在跑、仍然必须过**，只是不在 MVP 这一段。
    # 登记进来的唯一理由是退出码语义与 `Invoke-Checked`（任何非零即红）冲突：
    # `jhyt_m95_gpu_particles` 在没有 compute 的机器上按仓内约定退 **3**（环境受限，不是通过），
    # 由本文件的 "Compute / GPU particles" 步骤带 exit-3 分支执行，`RUSTLUA_REQUIRE_GPU=1`
    # 时把跳过升级为红。登记进 `$examples` 会让所有无 GPU 机器与 CI runner 恒红，
    # 而恒红的门等于没有门。
    # 防漂移：这张表里的每一项必须在脚本里有一处**真实调用点**（`--example <名字>`）。
    # 只登记不调用 = 把回归藏进第四张表，那正是本段要防的「静默少跑」。
    $gatedElsewhereExamples = @(
        "jhyt_m95_gpu_particles"
    )
    $verifySource = Get-Content -LiteralPath $PSCommandPath -Raw
    foreach ($example in $gatedElsewhereExamples) {
        if (-not (Test-Path -LiteralPath (Join-Path $exampleDir ($example + ".rs")))) {
            throw ("elsewhere-gated example list names $example but the file is gone; drop the entry (a missing gate is a lost gate)")
        }
        if (($examples -contains $example) -or ($retiredExamples -contains $example) -or ($releaseExamples.Name -contains $example)) {
            throw ("$example appears in more than one example list; pick one")
        }
        if ($verifySource -notmatch [regex]::Escape("--example $example")) {
            throw ("$example 登记为「其它段执行」，但 verify.ps1 里没有 --example $example 的调用点：没有任何门真的在跑它")
        }
    }

    # 1) 先全量校验清单，缺失/多余都直接失败（缺失 = 回归消失，多余 = 清单漏登记）。
    $missing = @()
    foreach ($example in $examples) {
        $source = Join-Path $exampleDir ($example + ".rs")
        if (-not (Test-Path -LiteralPath $source)) {
            $missing += $example
        }
    }
    if ($missing.Count -gt 0) {
        throw ("MVP example list is missing $($missing.Count) file(s): " + ($missing -join ", ") + " (expected under $exampleDir)")
    }
    $onDisk = @(Get-ChildItem -LiteralPath $exampleDir -Filter "jhyt_*.rs" -File | ForEach-Object { $_.BaseName })
    $unlisted = @($onDisk | Where-Object { $examples -notcontains $_ -and $retiredExamples -notcontains $_ -and $gatedElsewhereExamples -notcontains $_ -and @($releaseExamples | ForEach-Object { $_.Name }) -notcontains $_ })
    if ($unlisted.Count -gt 0) {
        throw ("jhyt_* example(s) exist but are not in the MVP list: " + ($unlisted -join ", ") + " (add them to verify.ps1 or delete them)")
    }

    # 2) 逐项执行，并断言"执行数量 == 清单数量"。
    $executed = 0
    Push-Location $engineRoot
    try {
        foreach ($example in $examples) {
            Invoke-Checked cargo @("run", "--quiet", "-p", "game", "--example", $example)
            $executed++
        }
    }
    finally {
        Pop-Location
    }
    if ($executed -ne $examples.Count) {
        throw "MVP gate ran $executed example(s) but the manifest lists $($examples.Count); a silent skip is never acceptable."
    }
    Write-Host "MVP headless regression: executed=$executed expected=$($examples.Count)" -ForegroundColor Green

    # 3) 打包产物门：产物在就真跑（失败即红），不在就明说 [SKIP]——不是通过。
    Push-Location $engineRoot
    try {
        foreach ($entry in $releaseExamples) {
            $artifactPath = Join-Path $defaultGameProject $entry.Artifact
            if (-not (Test-Path -LiteralPath $artifactPath)) {
                $message = "[SKIP] $($entry.Name): 没有发布产物 $($entry.Artifact)（不是通过；先跑 $($entry.Hint)）"
                Write-Host "  $message" -ForegroundColor Yellow
                Add-Content -Path $logPath -Value $message -Encoding UTF8
                continue
            }
            Invoke-Checked cargo @("run", "--quiet", "-p", "game", "--example", $entry.Name)
        }
    }
    finally {
        Pop-Location
    }

    Write-Step "Clean release directory self test"
    Invoke-PackageSelfTest -ProjectRootPath $defaultGameProject

    Write-Host "`nMVP gate passed. Log: $logPath" -ForegroundColor Green
}

if ($Benchmarks) {
    Write-Step "WP0 performance baselines"
    Push-Location $engineRoot
    try {
        $baselineRoot = $baselinesRoot
        $ecsBaselinePath = Join-Path $baselineRoot "ecs-stairs.json"
        $spriteBaselinePath = Join-Path $baselineRoot "sprite-10000.json"
        $tilemapBaselinePath = Join-Path $baselineRoot "tilemap-100x100.json"
        $threeDBaselinePath = Join-Path $baselineRoot "wp0-3d-stress.json"
        $threeDGoldenPath = Join-Path $baselineRoot "wp0-3d-golden.json"
        $sceneFramePath = Join-Path $baselineRoot "wp0-scene-frame.json"
        $ecsStoragePath = Join-Path $baselineRoot "wp0-ecs-storage.json"
        $twoDGoldenPath = Join-Path $baselineRoot "wp0-2d-golden.json"
        $mixedGoldenPath = Join-Path $baselineRoot "wp0-mixed-golden.json"
        $hudGoldenPath = Join-Path $baselineRoot "wp0-hud-golden.json"
        $animGoldenPath = Join-Path $baselineRoot "wp0-anim-golden.json"
        $materialGoldenPath = Join-Path $baselineRoot "wp0-material-golden.json"
        $lightingGoldenPath = Join-Path $baselineRoot "wp0-lighting-golden.json"
        $editorPreviewPath = Join-Path $baselineRoot "wp0-editor-preview-golden.json"
        $gpuParticlesGoldenPath = Join-Path $baselineRoot "wp0-gpu-particles-golden.json"
        $gpuParticles50kPath = Join-Path $baselineRoot "wp0-gpu-particles-50k.json"
        Invoke-Checked cargo @(
            "run", "--quiet", "--release", "-p", "engine", "--example", "ecs_stress", "--",
            "--stairs", "--seconds", "1", "--output", $ecsBaselinePath
        )
        Invoke-Checked cargo @(
            "run", "--quiet", "--release", "-p", "engine", "--example", "sprite_stress", "--",
            "--sprites", "10000", "--cycles", "30", "--output", $spriteBaselinePath
        )
        Invoke-Checked cargo @(
            "run", "--quiet", "--release", "-p", "engine", "--example", "tilemap_stress", "--",
            "--width", "100", "--height", "100", "--cycles", "30", "--output", $tilemapBaselinePath
        )
        Invoke-Checked cargo @(
            "run", "--quiet", "--release", "-p", "engine", "--example", "wp0_3d_stress", "--",
            "--cycles", "30", "--output", $threeDBaselinePath
        )
        Invoke-Checked cargo @(
            "run", "--quiet", "--release", "-p", "engine", "--example", "wp0_3d_golden", "--",
            "--output", $threeDGoldenPath
        )
        # WP0 增量（wp0-v4）：场景装载 / typed 编译 / 运行时帧分配。帧内分配是 WP1-5、
        # WP3、WP6 的前后对照量尺——运行帧本就不解析字符串，真正要替换的是分配与克隆。
        Invoke-Checked cargo @(
            "run", "--quiet", "--release", "-p", "engine", "--example", "wp0_scene_frame", "--",
            "--cycles", "30", "--output", $sceneFramePath
        )
        # WP0 增量（wp0-2d-golden-v1）：2D overlay 的语义 golden。WP6 要把 2D 顶点改成单趟
        # 生成，改动的风险是"画歪/少画/顺序错"——3D golden 与 CPU 顶点计数都发现不了。
        Invoke-Checked cargo @(
            "run", "--quiet", "--release", "-p", "engine", "--example", "wp0_2d_golden", "--",
            "--output", $twoDGoldenPath
        )
        # docs/54 §8-④：混合 2D/3D 的像素级遮挡证据。2D golden 只覆盖 overlay 顶点管线、
        # 3D golden 只覆盖网格 pass，两者都看不到"精灵与网格在同一份深度缓冲上互相遮挡"。
        Invoke-Checked cargo @(
            "run", "--quiet", "--release", "-p", "engine", "--example", "wp0_mixed_golden", "--",
            "--output", $mixedGoldenPath
        )
        # docs/54 §8-⑤：HUD 隔离的像素级证据。2D golden 只看单层内容，看不到"层与层谁盖谁"——
        # 这条用同一次场景的 A/B 渲染证明 ScreenUI 画在世界内容与 Overlay2D 之后。
        Invoke-Checked cargo @(
            "run", "--quiet", "--release", "-p", "engine", "--example", "wp0_hud_golden", "--",
            "--output", $hudGoldenPath
        )
        # P3-E01-4a：场景级动画 golden。E01 的播放器此前只有单元测试（采样值），"动画真的
        # 驱动了渲染"没有被任何 golden 证明过——这条用同一场里的 2D 精灵与 3D 灯各跑一遍
        # `.anim`，在三个时刻**同时**断言世界坐标、屏幕质心与像素数：采样对了但没写回
        # Transform、或写回了却没走到提取/投影，都会立刻在这里显形。exit 3 = 本机没有
        # offscreen adapter，显式跳过而不是通过；RUSTLUA_REQUIRE_GPU=1 时升级为红。
        $animGoldenSkipped = $false
        & cargo run --quiet --release -p engine --example wp0_anim_golden -- --output $animGoldenPath
        $animGoldenExit = $LASTEXITCODE
        if ($animGoldenExit -eq 3) {
            if ($env:RUSTLUA_REQUIRE_GPU -eq "1") {
                throw "anim golden skipped (exit 3) 但 RUSTLUA_REQUIRE_GPU=1：这台机器必须提供 offscreen adapter"
            }
            $animGoldenSkipped = $true
            Write-Host "anim_golden_skipped reason=no_adapter exit=3（不是通过）" -ForegroundColor Yellow
        }
        elseif ($animGoldenExit -ne 0) {
            throw "anim golden FAILED (wp0_anim_golden exit $animGoldenExit)"
        }
        # P3-E02-4：材质四组场景级 golden。四块地面只差 `.material` 的 metallic/roughness，
        # 镜面反射点必须落在采样网格内——否则 0.1 的窄高光会被整片漏采，看起来像参数没生效。
        # exit 3 与 anim golden 同一语义：无 offscreen adapter 显式跳过，不是通过。
        $materialGoldenSkipped = $false
        & cargo run --quiet --release -p engine --example wp0_material_golden -- --output $materialGoldenPath
        $materialGoldenExit = $LASTEXITCODE
        if ($materialGoldenExit -eq 3) {
            if ($env:RUSTLUA_REQUIRE_GPU -eq "1") {
                throw "material golden skipped (exit 3) 但 RUSTLUA_REQUIRE_GPU=1：这台机器必须提供 offscreen adapter"
            }
            $materialGoldenSkipped = $true
            Write-Host "material_golden_skipped reason=no_adapter exit=3（不是通过）" -ForegroundColor Yellow
        }
        elseif ($materialGoldenExit -ne 0) {
            throw "material golden FAILED (wp0_material_golden exit $materialGoldenExit)"
        }
        # docs/54 §8-⑩：**材质/光照与阴影**的像素级证据。3D golden 只断言"有内容、几何量对"，
        # 材质颜色错、`Light` 方向反了、阴影贴图没被采样都能蒙过去——这条门就是为它们立的，
        # 也是本轮量出 `Light::direction` 符号缺陷的那个探针。
        Invoke-Checked cargo @(
            "run", "--quiet", "--release", "-p", "engine", "--example", "wp0_lighting_golden", "--",
            "--output", $lightingGoldenPath
        )
        Invoke-Checked cargo @(
            "run", "--quiet", "--release", "-p", "engine", "--example", "wp0_ecs_storage", "--",
            "--cycles", "30", "--output", $ecsStoragePath
        )
        # docs/54 §8-⑨：**编辑器路径**的可重复证据。运行时的三种画面都有 golden，但"编辑器里改了
        # 东西之后预览画成什么样"此前只有人工目测；这条把文档 → 撤销/重做 → 空间语义 → 预览一帧
        # 整条链在无窗口条件下跑通并逐条断言。
        Invoke-Checked cargo @(
            "run", "--quiet", "--release", "-p", "editor", "--example", "editor_preview_golden", "--",
            "--output", $editorPreviewPath
        )
        # P3-R07-3：instanced GPU 粒子的场景级 golden（10k 粒子、固定 seed/dt；首帧逐位对拍 +
        # 0.5 s 积分对拍 + 9 s 状态在界 + instanced 记账 + 两帧像素校验和 + 回滚档空帧）。
        # 与上面的 Compute 门同一约定：exit 3 = 本机没有 compute，**显式跳过而不是通过**；
        # RUSTLUA_REQUIRE_GPU=1 时把跳过升级为红（发布门用）。
        $gpuParticlesGoldenSkipped = $false
        & cargo run --quiet --release -p engine --example wp0_gpu_particles_golden -- --output $gpuParticlesGoldenPath
        $gpuParticlesExit = $LASTEXITCODE
        if ($gpuParticlesExit -eq 3) {
            if ($env:RUSTLUA_REQUIRE_GPU -eq "1") {
                throw "gpu particles golden skipped (exit 3) 但 RUSTLUA_REQUIRE_GPU=1：这台机器必须提供 compute"
            }
            $gpuParticlesGoldenSkipped = $true
            Write-Host "gpu_particles_golden_skipped reason=no_compute exit=3（CPU 粒子仍是默认与回滚档；不是通过）" -ForegroundColor Yellow
        }
        elseif ($gpuParticlesExit -ne 0) {
            throw "gpu particles golden FAILED (wp0_gpu_particles_golden exit $gpuParticlesExit)"
        }
        # P3-R07-4：50k 粒子的 CPU/GPU 两档读数与默认档判定。与 golden 同一约定——exit 3 =
        # 本机没有 compute，显式跳过而不是通过；RUSTLUA_REQUIRE_GPU=1 时升级为红。
        # 注意这**不是**像素 golden：它刻意只比"两档读数 + instanced 记账 + 内存界"，因为
        # 默认档结论依赖机器（WARP 上总帧读数不代表性），像素契约由上面的 10k golden 负责。
        $gpuParticles50kSkipped = $false
        & cargo run --quiet --release -p engine --example wp0_gpu_particles_50k -- --output $gpuParticles50kPath
        $gpuParticles50kExit = $LASTEXITCODE
        if ($gpuParticles50kExit -eq 3) {
            if ($env:RUSTLUA_REQUIRE_GPU -eq "1") {
                throw "gpu particles 50k benchmark skipped (exit 3) 但 RUSTLUA_REQUIRE_GPU=1：这台机器必须提供 compute"
            }
            $gpuParticles50kSkipped = $true
            Write-Host "gpu_particles_50k_skipped reason=no_compute exit=3（默认档仍按 CPU 路径，不是通过）" -ForegroundColor Yellow
        }
        elseif ($gpuParticles50kExit -ne 0) {
            throw "gpu particles 50k benchmark FAILED (wp0_gpu_particles_50k exit $gpuParticles50kExit)"
        }
    }
    finally {
        Pop-Location
    }
    # docs/54 §8-⑩：把"这一次的 3D 报告"和**提交进版本库**的结构性基线比一次——几何量
    # （draw/instances/triangles）与分配峰值跨机可比，因此可以进门判红；时间（p95/p99）是机器
    # 相关的量，这一档只打印不判定（同机 A/B 用完整模式，见 metrics/README.md §7）。
    # P4-00：长期基线先读 `metrics/baselines/wp0-3d-stress.json`（被 git 跟踪），
    # 不存在再回退旧路径 `baselines/wp0-3d-stress.committed.json`（gitignore），避免"基线换位置"假红。
    Write-Step "3D baseline comparison (geometry + allocation)"
    $metricsBaseline = Join-Path $projectRoot "metrics\baselines\wp0-3d-stress.json"
    $committedBaseline = Join-Path $baselinesRoot "wp0-3d-stress.committed.json"
    if (Test-Path -LiteralPath $metricsBaseline) {
        $committedBaseline = $metricsBaseline
        Write-Host "  using tracked baseline: $metricsBaseline"
    }
    if (-not (Test-Path -LiteralPath $committedBaseline)) {
        throw "committed 3D baseline is missing: $committedBaseline（结构性基线放 metrics/baselines/wp0-3d-stress.json，本地 A/B 可放 baselines/；生成与更新流程见 metrics/README.md §5/§8。示例：在干净 HEAD 上跑 `cargo run --release -p engine --example wp0_3d_stress -- --cycles 30` 后人工核对几何量并写入 metrics/baselines/）"
    }
    Invoke-Checked powershell @(
        "-NoProfile", "-ExecutionPolicy", "Bypass",
        "-File", (Join-Path $projectRoot "scripts\verify\compare-baseline.ps1"),
        "-Current", $threeDBaselinePath,
        "-Previous", $committedBaseline,
        "-GeometryOnly"
    )
    $threeDReport = Get-Content -LiteralPath $threeDBaselinePath -Raw | ConvertFrom-Json
    if ($threeDReport.report_version -ne "wp0-v3" -or $threeDReport.stairs.Count -ne 3) {
        throw "3D WP0 report schema invalid: expected wp0-v3 with three stairs"
    }
    foreach ($stair in $threeDReport.stairs) {
        if ($stair.render.draw_calls -ne 2 -or
            $stair.render.instances -ne $stair.mesh_count -or
            $stair.render.triangles -ne $stair.mesh_count -or
            (($stair.render.gpu_timestamp_supported -and $null -eq $stair.render.gpu_frame_us -and $stair.render.gpu_timestamp_reason -notin @("pending", "resolved")) -or
             (-not $stair.render.gpu_timestamp_supported -and $null -ne $stair.render.gpu_frame_us)) -or
            -not $stair.render.gpu_timestamp_reason) {
            throw "3D WP0 report metrics invalid for mesh_count=$($stair.mesh_count)"
        }
        # docs/54 §7：内存峰值是红线之一（+10% 需评审）。这里先强制"必须报出真实数字"——
        # 没有测量就没有判定，缺字段比数字难看更危险（阈值对照由 compare-baseline.ps1 负责）。
        if ($null -eq $stair.cpu_peak_bytes -or $stair.cpu_peak_bytes -le 0) {
            throw "3D WP0 report is missing cpu_peak_bytes for mesh_count=$($stair.mesh_count)"
        }
    }
    $goldenReport = Get-Content -LiteralPath $threeDGoldenPath -Raw | ConvertFrom-Json
    if ($goldenReport.report_version -ne "wp0-golden-v1" -or
        $goldenReport.scene -ne "demo3d" -or
        $goldenReport.semantic.drawable_names.Count -ne 3 -or
        $goldenReport.semantic.draw_calls -lt 4 -or
        $goldenReport.semantic.instances -lt 3 -or
        $goldenReport.semantic.triangles -lt 3 -or
        $goldenReport.semantic.non_background_pixels -le 1000 -or
        $goldenReport.semantic.center_non_background_pixels -le 100) {
        throw "3D semantic golden invalid"
    }
    # wp0-v4：场景/帧报告契约。场景数**必须等于两个示例工程 scenes/ 里的场景文件数**
    # （此前写死一个下界，示例工程重建后它既挡不住"扫漏了场景"，也会在场景增减时误报）；
    # 帧阶梯必须三档且每档都有非零时间、非零分配——分配为 0 只可能意味着计数没生效，
    # 而不是"优化到位"。
    $sceneFrameReport = Get-Content -LiteralPath $sceneFramePath -Raw | ConvertFrom-Json
    $sceneFileCount = @(
        Get-ChildItem -Path (Join-Path $gameRoot "game_api/scenes"), (Join-Path $gameRoot "game_jhyt/scenes") `
            -Filter *.scene -File -ErrorAction SilentlyContinue
    ).Count
    if ($sceneFileCount -le 0) {
        throw "示例工程目录里找不到任何 .scene 文件（路径或布局变了）"
    }
    if ($sceneFrameReport.report_version -ne "wp0-v4" -or
        $sceneFrameReport.benchmark -ne "scene_frame" -or
        $sceneFrameReport.scene_load.scenes -ne $sceneFileCount -or
        $sceneFrameReport.frame_stairs.Count -ne 3) {
        throw "wp0-v4 scene/frame report schema invalid: scenes=$($sceneFrameReport.scene_load.scenes) expected=$sceneFileCount"
    }
    foreach ($entry in $sceneFrameReport.scene_load.entries) {
        # M63b 起 `game_jhyt/main.scene` 是**故意留空**的门面（17 个 UI 实体整批搬进
        # `assets/prefabs/title_menu.prefab`，由 `scripts/ui/title_menu.lua` 进门面时实例化），
        # 所以"每条 entry 都要有 typed 组件"在它是必然为假。判据改成从场景文本自己推：
        # **声明了实体的场景**必须有非零 typed 计数与非零 typed 编译耗时，空场景只查"载入没报错"。
        # 这样"场景有实体但 typed 编译没计数"这类真事故仍然 RED，而不是把整条门下界成 0。
        $scenePath = Join-Path $gameRoot (Join-Path $entry.project "scenes\$($entry.scene).scene")
        if (-not (Test-Path -LiteralPath $scenePath)) {
            throw "wp0-v4 scene file not found for entry $($entry.project)/$($entry.scene): $scenePath"
        }
        $entityLines = @(Get-Content -LiteralPath $scenePath -Encoding UTF8 |
            Where-Object { $_ -match '^entity\s' }).Count
        if ($entry.diagnostics -ne 0 -or $entry.load_us.p50_us -le 0) {
            throw "wp0-v4 scene load invalid: $($entry.project)/$($entry.scene) diagnostics=$($entry.diagnostics) load_p50=$($entry.load_us.p50_us)"
        }
        if ($entityLines -gt 0 -and ($entry.typed_components -le 0 -or $entry.typed_compile_us.p50_us -le 0)) {
            throw ("wp0-v4 typed compile entry invalid: {0}/{1} entities={2} typed_components={3} " +
                "typed_compile_p50={4}") -f $entry.project, $entry.scene, $entityLines, `
                $entry.typed_components, $entry.typed_compile_us.p50_us
        }
    }
    foreach ($stair in $sceneFrameReport.frame_stairs) {
        if ($stair.frame_us.p50_us -le 0 -or $stair.alloc_count_p50 -le 0 -or
            $stair.alloc_bytes_p50 -le 0 -or $stair.vertices -le 0) {
            throw "wp0-v4 frame metrics invalid for sprites=$($stair.sprites)"
        }
    }
    # docs/54 §4.4 批 N：**动画路径的每帧分配不许随实体数增长**。
    #
    # 背景：批 H 把"帧 → UV"与帧事件改成走轨道视图，但每次调用都现算一遍——对一群动画实体就是
    # 每帧每实体一次分配。**静态精灵阶梯完全看不见它**（WP0 的场景里没有动画实体），所以这条
    # 回归是读代码时发现的，不是门发现的。现在单列一档动画阶梯，并在这里判红两件事：
    # ① 两档实体数相差 10 倍时每帧分配次数几乎不变（差值上限 20 次）；② 绝对上限 200 次兜住
    # "两档一起退化"（5000 实体若每实体一次分配就是 5000，远超门槛）。
    if ($sceneFrameReport.animation_stairs.Count -ne 2) {
        throw "wp0-v4 animation stairs missing: expected two entity counts"
    }
    $animationAlloc = @($sceneFrameReport.animation_stairs | ForEach-Object { [int]$_.alloc_count_p50 })
    $animationEntities = @($sceneFrameReport.animation_stairs | ForEach-Object { [int]$_.entities })
    $maxAnimationAlloc = ($animationAlloc | Measure-Object -Maximum).Maximum
    if ($animationEntities[0] -ge $animationEntities[1]) {
        throw "animation stairs must increase in entity count: $($animationEntities -join ',')"
    }
    if ($maxAnimationAlloc -gt 200) {
        throw "animation frame allocations too high: max=$maxAnimationAlloc per frame"
    }
    if (($animationAlloc[1] - $animationAlloc[0]) -gt 20) {
        throw ("animation frame allocations scale with entities: {0}->{1} vs {2}->{3}" -f `
            $animationEntities[0], $animationAlloc[0], $animationEntities[1], $animationAlloc[1])
    }
    # docs/54 §11.1：`spatial=local` 场景里的**世界变换传播**必须有实测数字（此前那条"不做
    # dirty/version 增量"的理由只是判断；现在它由这份报告给出占比）。每档三个相位：
    #   静态（顶层字段，整棵层级不动，应走批 R 的"没变就跳过"）／`moving_one`（只挪一个叶子）／
    #   `moving`（挪动所有链根，最坏情况）。
    # 这里只判"测量是活的 + 跳过真的发生 + 有改动就不会被跳过"：阶梯恰为 2 档、静态
    # `spatial_propagate_us` 非零、静态 `spatial_skips_p50` 非零（跳过生效的证据）、占比落在
    # 0..100、三个相位里"有改动"的传播耗时**严格大于**静态档（指纹不能把真改动吞掉）。
    # **时间阈值不进门**（跨机不可比，与 `compare-baseline -GeometryOnly` 同一口径）；
    # 占比与省下的量记在 docs/53 §3.4c/§3.4e。
    if ($sceneFrameReport.hierarchy_stairs.Count -ne 2) {
        throw "wp0-v4 hierarchy stairs missing: expected two entity counts"
    }
    foreach ($stair in $sceneFrameReport.hierarchy_stairs) {
        $share = [double]$stair.spatial_share_pct
        if ([int]$stair.entities -le 0 -or [int]$stair.chains -le 0 -or
            [int]$stair.chain_depth -le 0 -or [int]$stair.spatial_propagate_us -le 0 -or
            [int]$stair.spatial_skips_p50 -le 0 -or $share -le 0 -or $share -gt 100) {
            throw ("hierarchy stair metrics invalid for entities={0}: propagate_us={1} skips={2} share={3}" -f `
                $stair.entities, $stair.spatial_propagate_us, $stair.spatial_skips_p50, `
                $stair.spatial_share_pct)
        }
        foreach ($phase in @('moving_one', 'moving')) {
            $moving = $stair.$phase
            if ($null -eq $moving) {
                throw ("hierarchy stair missing {0} phase for entities={1}" -f $phase, $stair.entities)
            }
            $movingShare = [double]$moving.spatial_share_pct
            if ([int]$moving.spatial_propagate_us -le 0 -or $movingShare -le 0 -or $movingShare -gt 100) {
                throw ("hierarchy {0} phase metrics invalid for entities={1}: propagate_us={2} share={3}" -f `
                    $phase, $stair.entities, $moving.spatial_propagate_us, $moving.spatial_share_pct)
            }
            if ([int]$moving.spatial_propagate_us -le [int]$stair.spatial_propagate_us) {
                throw ("hierarchy {0} phase does not cost more than the skipped static phase for entities={1}: " +
                    "{2} <= {3} (fingerprint may be swallowing a real change)" -f `
                    $phase, $stair.entities, $moving.spatial_propagate_us, $stair.spatial_propagate_us)
            }
        }
        # docs/54 §10 批 S：**逐实体增量**的判据（都是确定量，不受跨机速度影响）。
        # - 静态相位不许重算任何实体（整趟跳过 ⇒ 0）；
        # - 只挪一个叶子时重算数必须**恰好是 1**：这是"增量真的生效"的核心证据——批 R 时代它是
        #   实体总数（"单叶动与全动同价"就是这么量出来的）；
        # - 挪动所有链根时必须重算到**几乎每个实体**（父的世界值一变，整棵子树过期；少算就是错）。
        if ([int]$stair.spatial_recomputed_p50 -ne 0) {
            throw ("hierarchy static phase recomputed entities: expected 0, got {0} (entities={1})" -f `
                $stair.spatial_recomputed_p50, $stair.entities)
        }
        if ([int]$stair.moving_one.spatial_recomputed_p50 -ne 1) {
            throw ("hierarchy single-leaf phase must recompute exactly 1 entity (entities={0}, got {1}, " +
                "incremental_calls={2})" -f $stair.entities, $stair.moving_one.spatial_recomputed_p50, `
                $stair.moving_one.spatial_incremental_calls_p50)
        }
        if ([int]$stair.moving_one.spatial_incremental_calls_p50 -le 0) {
            throw ("hierarchy single-leaf phase never took the incremental path (entities={0})" -f `
                $stair.entities)
        }
        if ([int]$stair.moving.spatial_recomputed_p50 -lt ([int]$stair.entities / 2)) {
            throw ("hierarchy all-roots phase recomputed too few entities (entities={0}, got {1})" -f `
                $stair.entities, $stair.moving.spatial_recomputed_p50)
        }
        # 时间比值**同进程内**可比：单叶动至少要比全动省一半（批 S 实测 2.8×/3.0×，留约四成余量）。
        if ([int]$stair.moving_one.spatial_propagate_us * 2 -gt [int]$stair.moving.spatial_propagate_us) {
            throw ("hierarchy incremental path did not pay off for entities={0}: single-leaf {1} us vs " +
                "all-roots {2} us" -f $stair.entities, $stair.moving_one.spatial_propagate_us, `
                $stair.moving.spatial_propagate_us)
        }
    }
    # docs/54 §10 批 Q/R/S：传播路径的**每帧分配不许随实体数增长**，三个相位都算。修前 1 万实体是
    # 4.5 万次/帧（`BTreeSet`/`BTreeMap` 逐实体树节点），批 Q 修成 138/155，批 R 静态档 75/80，
    # 批 S 三档实测 75/87/87 与 80/96/96；门槛留约四倍余量。分配次数是确定量，不受跨机速度影响。
    $hierarchyAlloc = @($sceneFrameReport.hierarchy_stairs | ForEach-Object { [int]$_.alloc_count_p50 })
    $hierarchyEntities = @($sceneFrameReport.hierarchy_stairs | ForEach-Object { [int]$_.entities })
    $hierarchyPhaseAlloc = @($sceneFrameReport.hierarchy_stairs | ForEach-Object {
            [int]$_.alloc_count_p50
            [int]$_.moving_one.alloc_count_p50
            [int]$_.moving.alloc_count_p50
        })
    if (($hierarchyPhaseAlloc | Measure-Object -Maximum).Maximum -gt 400) {
        throw ("hierarchy frame allocations too high: {0}" -f ($hierarchyPhaseAlloc -join ','))
    }
    if (($hierarchyAlloc[1] - $hierarchyAlloc[0]) -gt 100) {
        throw ("hierarchy frame allocations scale with entities: {0}->{1} vs {2}->{3}" -f `
            $hierarchyEntities[0], $hierarchyAlloc[0], $hierarchyEntities[1], $hierarchyAlloc[1])
    }
    # 深度阶梯：增量传播的不变量必须成立——叶子变动只重算 1 个节点，根变动重算整条链。
    # 任一条不成立就说明脏标记或父先序求解已经错了。
    if ($sceneFrameReport.transform_depth.Count -ne 3) {
        throw "wp0-v4 transform depth stairs invalid: expected three depths"
    }
    foreach ($stair in $sceneFrameReport.transform_depth) {
        if ($stair.full_ns.p50_ns -le 0 -or $stair.incremental_leaf_ns.p50_ns -le 0 -or
            $stair.incremental_root_ns.p50_ns -le 0 -or
            $stair.full_recomputes -ne $stair.depth -or
            $stair.leaf_recomputes -ne 1 -or
            $stair.root_recomputes -ne $stair.depth -or
            $stair.leaf_alloc_p50 -le 0) {
            throw "wp0-v4 transform depth metrics invalid for depth=$($stair.depth)"
        }
    }
    # Tilemap 阶梯：WP4 ④ 起顶点数**不再**等于格数 × 6——视口外的瓦片会被剔除，这正是要证明
    # 的事。因此改断言"顶点数是 6 的倍数、非零、且确实少于整层"：最后一条证明剔除在生效，
    # 而"没有漏画任何可见瓦片"由 2D golden 的像素等价证明负责（关/开剔除像素逐字节相同）。
    if ($sceneFrameReport.tilemap_stairs.Count -ne 2) {
        throw "wp0-v4 tilemap stairs invalid: expected two sides"
    }
    foreach ($stair in $sceneFrameReport.tilemap_stairs) {
        if ($stair.frame_us.p50_us -le 0 -or $stair.alloc_bytes_p50 -le 0 -or
            $stair.tiles -ne ($stair.side * $stair.side) -or
            $stair.vertices -le 0 -or
            ($stair.vertices % 6) -ne 0 -or
            $stair.vertices -ge ($stair.tiles * 6)) {
            throw "wp0-v4 tilemap metrics invalid for side=$($stair.side): vertices=$($stair.vertices) tiles=$($stair.tiles)"
        }
    }
    # WP4 ④ 编辑阶梯：改一格之后再渲染一帧的成本必须可测（两侧非零），否则这把量尺失效。
    if ($sceneFrameReport.tilemap_edit_stairs.Count -ne 2) {
        throw "wp0-v4 tilemap edit stairs invalid: expected two sides"
    }
    foreach ($stair in $sceneFrameReport.tilemap_edit_stairs) {
        if ($stair.steady_frame_us.p50_us -le 0 -or $stair.edited_frame_us.p50_us -le 0 -or
            $stair.edit_cost_p50_us -lt 0 -or $stair.vertices -le 0) {
            throw "wp0-v4 tilemap edit metrics invalid for side=$($stair.side)"
        }
    }
    # 粒子阶梯：docs/40 §8 的目标是 1 万 CPU 粒子帧 < 4 ms，这里把该目标直接纳入门。活粒子数
    # 必须等于请求数（发射器没补满说明补发逻辑或统计已错），顶点数必须是粒子的 6 倍。
    if ($sceneFrameReport.particle_stairs.Count -ne 2) {
        throw "wp0-v4 particle stairs invalid: expected two scales"
    }
    foreach ($stair in $sceneFrameReport.particle_stairs) {
        if ($stair.particles -ne $stair.live -or
            $stair.frame_us.p50_us -le 0 -or $stair.alloc_count_p50 -le 0 -or
            $stair.alloc_bytes_p50 -le 0 -or
            $stair.vertices -ne ($stair.particles * 6)) {
            throw "wp0-v4 particle metrics invalid for particles=$($stair.particles)"
        }
        if ($stair.frame_us.p50_us -ge 4000) {
            throw "wp0-v4 particle budget exceeded: $($stair.frame_us.p50_us) us at particles=$($stair.particles)"
        }
    }
    # wp0-2d-golden-v1：2D overlay 语义契约。场景是基准自己合成的固定场景（2 精灵 + 8 瓦片），
    # 因此这些数字是**精确**契约而不是"下界"：顶点数、draw range、实例与三角形数一变就说明
    # 提取路径动了结构；leaked_pixels 必须为 0，它是"投影或合批顺序错了"最灵敏的一条。
    $twoDGoldenReport = Get-Content -LiteralPath $twoDGoldenPath -Raw | ConvertFrom-Json
    if ($twoDGoldenReport.report_version -ne "wp0-2d-golden-v1") {
        throw "2D golden report version invalid: $($twoDGoldenReport.report_version)"
    }
    $twoD = $twoDGoldenReport.semantic
    if ($twoD.vertices -ne 60 -or $twoD.draw_ranges -lt 3 -or $twoD.instances -ne 10 -or $twoD.triangles -ne 20) {
        throw "2D golden overlay structure changed: vertices=$($twoD.vertices) draws=$($twoD.draw_ranges) instances=$($twoD.instances) triangles=$($twoD.triangles)"
    }
    if ($twoD.leaked_pixels -ne 0) {
        throw "2D golden leaked $($twoD.leaked_pixels) pixels outside the expected rects: projection or batching order is wrong"
    }
    if ($twoD.center_pixel[0] -lt 200 -or $twoD.center_pixel[1] -gt 40 -or $twoD.center_pixel[2] -gt 40) {
        throw "2D golden solid sprite is not red: $($twoD.center_pixel -join ',')"
    }
    foreach ($region in $twoD.regions) {
        if ($region.non_background_pixels -le 0) {
            throw "2D golden region '$($region.name)' rendered nothing"
        }
    }
    # docs/54 §8-④：混合 2D/3D 的遮挡契约。三条必须**同时**成立——只断言"精灵可见"会在
    # 精灵永远画在最上层时也通过；只断言"被遮挡"会在精灵根本没被画出时也通过。
    $mixedGoldenReport = Get-Content -LiteralPath $mixedGoldenPath -Raw | ConvertFrom-Json
    if ($mixedGoldenReport.report_version -ne "wp0-mixed-golden-v1") {
        throw "mixed golden report version invalid: $($mixedGoldenReport.report_version)"
    }
    if ($mixedGoldenReport.semantic.meshes -lt 1 -or $mixedGoldenReport.semantic.world_sprites -lt 3) {
        throw "mixed golden is not mixed: meshes=$($mixedGoldenReport.semantic.meshes) world_sprites=$($mixedGoldenReport.semantic.world_sprites)"
    }
    if ($mixedGoldenReport.semantic.draw_calls -lt 4) {
        throw "mixed golden frame is missing passes: draw_calls=$($mixedGoldenReport.semantic.draw_calls)"
    }
    $occlusion = $mixedGoldenReport.occlusion
    if (-not $occlusion.front_sprite_visible) {
        throw "mixed golden: 网格前面的不透明精灵不可见（不透明精灵必须写深度）：$($occlusion.samples.front -join ',')"
    }
    if (-not $occlusion.behind_sprite_occluded) {
        throw "mixed golden: 网格后面的精灵没有被遮挡——Sprite 与 Mesh 没有共用同一份深度：$($occlusion.samples.behind -join ',') wall=$($occlusion.samples.wall -join ',')"
    }
    if (-not $occlusion.translucent_blended) {
        throw "mixed golden: 半透明精灵不是叠加结果（深度策略没有按不透明性下发）：$($occlusion.samples.veil -join ',')"
    }
    # docs/54 §8-⑤：HUD 隔离。ScreenUI 必须画在世界内容与 Overlay2D **之后**——这不是"看起来
    # 差不多"，而是同一像素在开关屏幕 UI 前后的两次采样：关闭时是调试线的青色（证明那段叠加
    # 内容确实画在这里），打开后必须变成 HUD 的品红（证明 HUD 盖住了它）。
    $hudGoldenReport = Get-Content -LiteralPath $hudGoldenPath -Raw | ConvertFrom-Json
    if ($hudGoldenReport.report_version -ne "wp0-hud-golden-v1") {
        throw "HUD golden report version invalid: $($hudGoldenReport.report_version)"
    }
    if (-not $hudGoldenReport.hud_isolated) {
        throw "HUD golden declares the HUD is not isolated"
    }
    if (($hudGoldenReport.layer_order -join ',') -ne "world2d,overlay2d,screen_ui") {
        throw "HUD golden layer order changed: $($hudGoldenReport.layer_order -join ',')"
    }
    if ($hudGoldenReport.layers.world2d -lt 1 -or $hudGoldenReport.layers.overlay2d -lt 1 -or $hudGoldenReport.layers.screen_ui -lt 1) {
        throw "HUD golden is missing a layer: world=$($hudGoldenReport.layers.world2d) overlay=$($hudGoldenReport.layers.overlay2d) screen_ui=$($hudGoldenReport.layers.screen_ui)"
    }
    $hud = $hudGoldenReport.samples
    if ($hud.overlay_pixel_without_ui[1] -le 150 -or $hud.overlay_pixel_without_ui[2] -le 180) {
        throw "HUD golden: 关闭屏幕 UI 时那个像素不是静态碰撞体轮廓的青色（叠加层没画到 HUD 面板的位置）：$($hud.overlay_pixel_without_ui -join ',')"
    }
    if ($hud.overlay_pixel_with_ui[0] -le 180 -or $hud.overlay_pixel_with_ui[2] -le 180 -or $hud.overlay_pixel_with_ui[1] -ge 90) {
        throw "HUD golden: 打开屏幕 UI 后那个像素不是 HUD 的颜色——ScreenUI 没有盖住 Overlay2D：$($hud.overlay_pixel_with_ui -join ',')"
    }
    if ($hud.hud_inside_without_ui[0] -le 180 -or $hud.hud_inside_without_ui[1] -ge 70) {
        throw "HUD golden: 关闭屏幕 UI 时 HUD 区域没有露出下面的世界精灵：$($hud.hud_inside_without_ui -join ',')"
    }
    if ($hud.world_only_with_ui[0] -le 180 -or $hud.world_only_with_ui[1] -ge 70 -or $hud.world_only_with_ui[2] -ge 70) {
        throw "HUD golden: HUD 矩形之外的世界内容被改了颜色（屏幕 UI 不该影响它矩形外的像素）：$($hud.world_only_with_ui -join ',')"
    }
    # P3-E01-4a：场景级动画契约。三个时刻的读数都是**精确**的：`.anim` 的线性键
    # 540→740（2D）与 0→4（3D）在 t=0/0.5/1.0 的世界坐标由插值唯一确定；屏幕质心把
    # "采样值 → 写回 Transform → 提取 → 投影 → 光栅"整条链钉在一起；像素数 >100 排除
    # "坐标对了但根本没画出来"；clamp 段证明非循环片段走完停在末键而不是继续推。
    if (-not $animGoldenSkipped) {
        $animGolden = Get-Content -LiteralPath $animGoldenPath -Raw | ConvertFrom-Json
        if ($animGolden.report_version -ne "wp0-anim-golden-v1") {
            throw "anim golden report version invalid: $($animGolden.report_version)"
        }
        if ($animGolden.frames.Count -ne 3) {
            throw "anim golden must sample three frames, got $($animGolden.frames.Count)"
        }
        $animExpected = @(
            @{ t = 0.0; sprite = 540.0; sun = 0.0; screen = 100.0 },
            @{ t = 0.5; sprite = 640.0; sun = 2.0; screen = 200.0 },
            @{ t = 1.0; sprite = 740.0; sun = 4.0; screen = 300.0 }
        )
        for ($i = 0; $i -lt 3; $i++) {
            $frame = $animGolden.frames[$i]
            $expected = $animExpected[$i]
            if ([math]::Abs($frame.t - $expected.t) -gt 1e-4 -or
                [math]::Abs($frame.sprite_world_x - $expected.sprite) -gt 0.05 -or
                [math]::Abs($frame.sun_world_x - $expected.sun) -gt 0.01) {
                throw "anim golden frame $i 的世界坐标不对：t=$($frame.t) sprite_x=$($frame.sprite_world_x) sun_x=$($frame.sun_world_x)"
            }
            if ([math]::Abs($frame.sprite_screen_center[0] - $expected.screen) -gt 3) {
                throw "anim golden frame $i 的精灵屏幕质心不对：$($frame.sprite_screen_center -join ',')（期望 x≈$($expected.screen)）"
            }
            if ($frame.sprite_pixels -le 100) {
                throw "anim golden frame $i 的精灵几乎没画出来：pixels=$($frame.sprite_pixels)"
            }
        }
        $clamped = $animGolden.clamped_after_duration
        if (-not $clamped.unchanged -or
            [math]::Abs($clamped.after[0] - 740.0) -gt 0.05 -or
            [math]::Abs($clamped.after[1] - 4.0) -gt 0.01) {
            throw "anim golden: 非循环片段播完后没有停在末键：$($clamped | ConvertTo-Json -Compress)"
        }
        foreach ($check in @("two_d_moves_100px_per_second", "three_d_moves_4units_per_second", "pixels_track_the_transform", "non_loop_stops_at_duration")) {
            if (-not $animGolden.checks.$check) {
                throw "anim golden check '$check' is false"
            }
        }
        Write-Host ("anim_golden_ok frames=3 sprite_x={0}/{1}/{2} screen_x={3}/{4}/{5} pixels={6}/{7}/{8}" -f `
            $animGolden.frames[0].sprite_world_x, $animGolden.frames[1].sprite_world_x, $animGolden.frames[2].sprite_world_x,
            $animGolden.frames[0].sprite_screen_center[0], $animGolden.frames[1].sprite_screen_center[0], $animGolden.frames[2].sprite_screen_center[0],
            $animGolden.frames[0].sprite_pixels, $animGolden.frames[1].sprite_pixels, $animGolden.frames[2].sprite_pixels) -ForegroundColor Green
    }
    # P3-E02-4：材质参数确实改变着色。四组只差 metallic/roughness，读数来自 640×360 读回：
    # 电介质平均亮度必须高于同粗糙度的金属（金属没有漫反射）；0.1 粗糙度在镜面采样点的
    # 峰值必须高于 0.9；任意两组至少在一个通道上差 ≥4（参数真的进了 uniform 与 BRDF）。
    if (-not $materialGoldenSkipped) {
        $materialGolden = Get-Content -LiteralPath $materialGoldenPath -Raw | ConvertFrom-Json
        if ($materialGolden.report_version -ne "wp0-material-golden-v1") {
            throw "material golden report version invalid: $($materialGolden.report_version)"
        }
        foreach ($check in @("groups_are_distinct", "metal_removes_diffuse", "smooth_highlight_peaks_dielectric", "smooth_highlight_peaks_metal")) {
            if (-not $materialGolden.checks.$check) {
                throw "material golden check '$check' is false"
            }
        }
        if ($materialGolden.min_pair_delta -lt 4) {
            throw "material golden: 四组材质几乎没有像素差：min_pair_delta=$($materialGolden.min_pair_delta)"
        }
        Write-Host ("material_golden_ok groups={0} min_pair_delta={1} dielectric_mean={2} metal_mean={3} peaks={4}/{5}/{6}/{7}" -f `
            $materialGolden.groups.Count, $materialGolden.min_pair_delta,
            $materialGolden.groups[1].mean_luma, $materialGolden.groups[3].mean_luma,
            $materialGolden.groups[0].max_luma, $materialGolden.groups[1].max_luma,
            $materialGolden.groups[2].max_luma, $materialGolden.groups[3].max_luma) -ForegroundColor Green
    }
    # docs/54 §8-⑩：材质/光照与阴影。三条都必须成立，任何一条不成立都说明 3D 着色链断了：
    # ① 漫反射随光照方向变化（光反向 → 地面只剩环境光）；② 阴影把地面压暗；③ 阴影足迹落在
    # "遮挡物足迹 + 光照偏移"处（落点算错/遮挡物尺寸写错都会在这里显形）。
    $lighting = Get-Content -LiteralPath $lightingGoldenPath -Raw | ConvertFrom-Json
    if ($lighting.report_version -ne "wp0-lighting-golden-v1") {
        throw "lighting golden report version invalid: $($lighting.report_version)"
    }
    if (-not $lighting.diffuse_responds_to_light_direction) {
        throw "lighting golden: 漫反射没有随光照方向变化（光反向时地面仍是 $($lighting.luminance.opposite_light)）——`Light::direction` 的符号或光照 uniform 有问题"
    }
    if (-not $lighting.shadow_darkens_ground) {
        throw "lighting golden: 阴影没有把地面压暗：lit=$($lighting.luminance.lit) shadow=$($lighting.luminance.shadow)"
    }
    if ($lighting.luminance.lit -le ($lighting.luminance.shadow + 8)) {
        throw "lighting golden: 受光与阴影的亮度差太小：$($lighting.luminance.lit) vs $($lighting.luminance.shadow)"
    }
    if ($lighting.luminance.lit -le ($lighting.luminance.opposite_light + 20)) {
        throw "lighting golden: 受光地面没比「只有环境光」亮多少：$($lighting.luminance.lit) vs $($lighting.luminance.opposite_light)"
    }
    if ($lighting.shadow_samples -lt 100) {
        throw "lighting golden: 被遮暗的地面采样点太少（$($lighting.shadow_samples)）——阴影几乎不存在"
    }
    $shadowBbox = $lighting.shadow_world_bbox
    if ([math]::Abs($shadowBbox[0] - -0.7) -gt 0.35 -or [math]::Abs($shadowBbox[2] - 1.3) -gt 0.35 -or
        [math]::Abs($shadowBbox[1] - -0.8) -gt 0.35 -or [math]::Abs($shadowBbox[3] - 1.2) -gt 0.35) {
        throw "lighting golden: 阴影世界足迹不对：[$(($shadowBbox) -join ',')]，期望≈(-0.7,-0.8,1.3,1.2)（遮挡物 2×2 + 光照偏移 (0.30,0.20)）"
    }
    # docs/54 §8-⑨：编辑器路径。文档 → 撤销/重做 → 空间语义 → 预览一帧，四段都要有牙：
    # 世界保持的编辑、撤销回原样、重做回同一结果、预览真的画出了像素、两次预览逐字节相同。
    $editorPreview = Get-Content -LiteralPath $editorPreviewPath -Raw | ConvertFrom-Json
    if ($editorPreview.report_version -ne "wp0-editor-preview-golden-v1") {
        throw "editor preview golden report version invalid: $($editorPreview.report_version)"
    }
    if ($editorPreview.entities -lt 3) {
        throw "editor preview golden scene is too small: entities=$($editorPreview.entities)"
    }
    if (-not $editorPreview.edit.preserved -or -not $editorPreview.edit.parent_unmoved) {
        throw "editor preview golden: 世界保持的编辑 / 父实体不动这两条没成立：$($editorPreview.edit | ConvertTo-Json -Compress)"
    }
    if (-not $editorPreview.history.executed -or -not $editorPreview.history.undo_restores_original -or -not $editorPreview.history.redo_reapplies) {
        throw "editor preview golden: 撤销/重做链断了：$($editorPreview.history | ConvertTo-Json -Compress)"
    }
    if ($editorPreview.preview.non_background_pixels -le 200) {
        throw "editor preview golden: 预览几乎什么都没画（$($editorPreview.preview.non_background_pixels) 个非背景像素）"
    }
    if (-not $editorPreview.preview.deterministic -or -not $editorPreview.preview.digest) {
        throw "editor preview golden: 同一份文档的两次预览不是逐字节相同"
    }
    if ($editorPreview.timing.p50_us -le 0 -or $editorPreview.timing.p95_us -lt $editorPreview.timing.p50_us) {
        throw "editor preview golden: 预览耗时采样异常：$($editorPreview.timing | ConvertTo-Json -Compress)"
    }

    # 瓦片行序：把 tiles 的两行交换后，上下两半的颜色必须整体对调。这条用像素证明了
    # "tiles 第 0 行画在地图最下方"（2D 世界 Y 向上），不依赖任何具体贴图颜色。
    $rows = $twoD.tile_rows
    if (-not $rows.row_zero_is_bottom) {
        throw "2D golden declares tile row 0 not at the bottom"
    }
    if (($rows.in_order_lower -join ',') -ne ($rows.swapped_upper -join ',') -or
        ($rows.in_order_upper -join ',') -ne ($rows.swapped_lower -join ',')) {
        throw "2D golden tile row order changed: swapping the tiles rows no longer swaps the rendered halves"
    }
    # WP7 ② collider 调试绘制：默认关闭，开启后**只多出轮廓**（box 4 段 × 6 顶点 = 24），
    # 且轮廓必须压在碰撞体的真实位置（绿占优）——这条把"世界→屏幕投影与碰撞体位置一致"
    # 变成了像素级契约。
    $debug = $twoD.collider_debug
    if ($null -eq $debug) {
        throw "2D golden report has no collider_debug block"
    }
    if ($debug.default_enabled) {
        throw "collider debug draw must default to off"
    }
    if ($debug.extra_vertices -ne 24) {
        throw "collider debug outline vertex count changed: extra=$($debug.extra_vertices) (expected 24)"
    }
    if ($debug.total_vertices -ne 84) {
        throw "collider debug total vertex count changed: $($debug.total_vertices) (expected 60 + 24)"
    }
    if ($debug.outline_pixel[1] -le ($debug.outline_pixel[0] + 60) -or
        $debug.outline_pixel[1] -le ($debug.outline_pixel[2] + 40)) {
        throw "collider debug outline pixel is not green: $($debug.outline_pixel -join ',')"
    }
    if ($debug.leaked_pixels -ne 0) {
        throw "collider debug outline drew $($debug.leaked_pixels) pixels outside the expected rects"
    }
    # WP4 ④ 视口外瓦片剔除：默认开启、确实剔掉了瓦片、且**像素逐字节不变**（这条是它正确性的
    # 全部依据：被剔掉的瓦片本就不该影响任何像素）。
    $culling = $twoD.tilemap_culling
    if ($null -eq $culling) {
        throw "2D golden report has no tilemap_culling block"
    }
    if (-not $culling.default_enabled) {
        throw "tilemap culling must default to on"
    }
    if (-not $culling.pixels_identical) {
        throw "tilemap culling changed the rendered pixels"
    }
    if (($culling.full_vertices % 6) -ne 0 -or
        ($culling.tiles_only_vertices % 6) -ne 0 -or
        ($culling.both_culled_vertices % 6) -ne 0) {
        throw "culling vertex counts are not whole quads: full=$($culling.full_vertices) tiles=$($culling.tiles_only_vertices) both=$($culling.both_culled_vertices)"
    }
    if ($culling.tiles_only_vertices -ge $culling.full_vertices -or
        $culling.both_culled_vertices -ge $culling.tiles_only_vertices) {
        throw "sprite culling removed nothing: full=$($culling.full_vertices) tiles=$($culling.tiles_only_vertices) both=$($culling.both_culled_vertices)"
    }
    # WP6 精灵剔除的同进程交替 A/B：必须确实减少顶点。帧时间刻意**不**断言——跨运行噪声
    # ±15–25%，同进程交替实测 -36.7%，但写成门会在负载波动时假红。
    $spriteCulling = $sceneFrameReport.sprite_culling_ab
    if ($spriteCulling.Count -ne 2) {
        throw "wp0-v4 sprite culling A/B invalid: expected two scales (10k vertex stage, 50k collection stage)"
    }
    foreach ($stair in $spriteCulling) {
        if ($stair.full_frame_us.p50_us -le 0 -or $stair.culled_frame_us.p50_us -le 0) {
            throw "sprite culling A/B frame timings invalid for sprites=$($stair.sprites)"
        }
        if ($stair.culled_vertices -le 0 -or
            $stair.culled_vertices -ge $stair.full_vertices -or
            ($stair.culled_vertices % 6) -ne 0) {
            throw "sprite culling removed nothing (or removed badly) for sprites=$($stair.sprites): full=$($stair.full_vertices) culled=$($stair.culled_vertices)"
        }
    }
    # WP8 并行合批契约（sprite_stress 的 parallel_ab）：**唯一硬要求是"并行 == 串行"**。
    # 这里刻意不断言"并行更快"——收益随规模变化（10k 档实测在噪声内），把它写成门会在负载
    # 波动时假红。判定提速请用同一进程内交替采样的 A/B 数据（见 docs/53 §3.6）。
    $spriteAbReport = Get-Content -LiteralPath $spriteBaselinePath -Raw | ConvertFrom-Json
    $ab = $spriteAbReport.parallel_ab
    if ($null -eq $ab) {
        throw "sprite report has no parallel_ab block: $spriteBaselinePath"
    }
    if (-not $ab.used) {
        throw "WP8 parallel A/B did not exercise the parallel path (used=false)"
    }
    if (-not $ab.identical) {
        throw "WP8 parallel build output differs from sequential byte-for-byte (digest=$($ab.digest))"
    }
    if ($ab.threads -lt 2) {
        throw "WP8 parallel A/B reports threads=$($ab.threads)"
    }
    $sequentialBuild = $ab.sequential_build_us.p50_us
    $parallelBuild = $ab.parallel_build_us.p50_us
    if ($sequentialBuild -le 0 -or $parallelBuild -le 0) {
        throw "WP8 parallel A/B build timings invalid: sequential=$sequentialBuild parallel=$parallelBuild"
    }
    # 只有"灾难性变慢"才判红：并行慢到 2 倍以上说明退化逻辑或线程使用已经错了。
    if ($parallelBuild -gt ($sequentialBuild * 2)) {
        throw "WP8 parallel build is catastrophically slower: parallel=${parallelBuild}us sequential=${sequentialBuild}us"
    }
    # wp0-v5：存储 A/B 契约。dense 侧必须在四个操作上都非零，且"全量可变遍历"必须快于
    # 现状——现状每次调用克隆整份实体表，若 dense 连这一点都不占优，说明前提判断错了。
    $ecsStorageReport = Get-Content -LiteralPath $ecsStoragePath -Raw | ConvertFrom-Json
    if ($ecsStorageReport.report_version -ne "wp0-v5" -or
        $ecsStorageReport.benchmark -ne "ecs_storage" -or
        $ecsStorageReport.stairs.Count -ne 3) {
        throw "wp0-v5 storage report schema invalid"
    }
    foreach ($stair in $ecsStorageReport.stairs) {
        foreach ($op in @("insert", "get", "for_each_mut", "remove_half")) {
            if ($stair.hash.$op.p50_ns -le 0 -or $stair.dense.$op.p50_ns -le 0) {
                throw "wp0-v5 $op metrics invalid for entities=$($stair.entities)"
            }
        }
        if ($stair.speedup.for_each_mut -le 1 -or $stair.speedup.remove_half -le 1) {
            throw "wp0-v5 storage speedup invalid for entities=$($stair.entities): dense 未占优"
        }
    }
    # P3-R07-3：instanced GPU 粒子的语义契约。只有本机真的跑了 compute 才判；跳过时上面已经
    # 打印过 gpu_particles_golden_skipped，这里不能把"没跑"读成"通过"。
    if (-not $gpuParticlesGoldenSkipped) {
        $gpuParticles = Get-Content -LiteralPath $gpuParticlesGoldenPath -Raw | ConvertFrom-Json
        if ($gpuParticles.report_version -ne "wp0-gpu-particles-golden-v1") {
            throw "gpu particles golden report version invalid: $($gpuParticles.report_version)"
        }
        $particles = $gpuParticles.semantic
        if ($particles.mode -ne "gpu" -or $particles.fallback -ne "none") {
            throw "gpu particles golden is not the GPU path: mode=$($particles.mode) fallback=$($particles.fallback)"
        }
        if ($particles.count -ne 10000 -or $particles.spawned -ne 10000) {
            throw "gpu particles golden scale changed: count=$($particles.count) spawned=$($particles.spawned)（R07-3 的验收规模是 10k）"
        }
        if ($particles.first_frame_mismatches -ne 0) {
            throw "gpu particles golden: 首帧 GPU 状态与 CPU burst 不同（mismatches=$($particles.first_frame_mismatches)）——这不是精度问题，是接线问题"
        }
        if ($particles.max_position_error -ge 1e-4 -or $particles.max_velocity_error -ge 1e-4) {
            throw "gpu particles golden: 0.5 s 对拍超容差 pos=$($particles.max_position_error) vel=$($particles.max_velocity_error)"
        }
        if ($particles.near_non_finite -ne 0 -or $particles.far_non_finite -ne 0) {
            throw "gpu particles golden: 状态里出现非有限值 near=$($particles.near_non_finite) far=$($particles.far_non_finite)"
        }
        if ($particles.far_dead_count -le 0) {
            throw "gpu particles golden: 9 s 时没有粒子死亡（$($particles.far_dead_count)）——寿命门失效，剔除/冻结无从验证"
        }
        if ($particles.draw_calls -ne 1 -or $particles.instances -ne 10000) {
            throw "gpu particles golden: instanced 记账变了 draws=$($particles.draw_calls) instances=$($particles.instances)（一次 draw、实例数 = capacity）"
        }
        if ($particles.vertices -ne 40000 -or $particles.triangles -ne 20000) {
            throw "gpu particles golden: 顶点/三角形不对 vertices=$($particles.vertices) triangles=$($particles.triangles)（4 顶点 triangle strip，期望 4×/2× capacity）"
        }
        if ($particles.dropped_draws -ne 0) {
            throw "gpu particles golden: 本帧丢弃了 $($particles.dropped_draws) 次 draw（管线或绑定组不可用）"
        }
        if ($particles.state_bytes -ne 480000) {
            throw "gpu particles golden: 状态缓冲不是 capacity×48 B（$($particles.state_bytes)）——GpuParticleState 布局漂移"
        }
        if ($particles.content_pixels_near -le 200 -or $particles.content_pixels_far -le 200) {
            throw "gpu particles golden: 粒子几乎没画进附件 near=$($particles.content_pixels_near) far=$($particles.content_pixels_far)"
        }
        if ($particles.pixels_checksum_near -eq $particles.pixels_checksum_far) {
            throw "gpu particles golden: 0.5 s 与 9 s 两帧像素校验和相同（粒子没在动，或读的是同一张图）"
        }
        if ($particles.rollback_particle_pass -or $particles.rollback_content_pixels -ne 0 -or -not $particles.rollback_unsupported) {
            throw "gpu particles golden: 关掉 compute 的回滚档不干净 particle_pass=$($particles.rollback_particle_pass) content=$($particles.rollback_content_pixels) unsupported=$($particles.rollback_unsupported)"
        }
        Write-Host ("gpu_particles_golden_ok count=10000 draw_calls=1 instances=10000 vertices=40000 " +
            "content_near={0} content_far={1} max_position_error={2}" -f `
            $particles.content_pixels_near, $particles.content_pixels_far, $particles.max_position_error) -ForegroundColor Green
    }
    # P3-R07-4：50k 基准的结构性契约。默认档是**结论**而不是前提——这里不判"必须选 gpu"，
    # 只判读数自洽（规模/内存/instanced 记账/两档门槛的一致性）；本机 WARP 上 total_ratio
    # 不可代表真实 GPU，所以默认档结论连同理由一起记进报告，由 docs/24 §3.15 解释。
    if (-not $gpuParticles50kSkipped) {
        # 报告里有中文的 default_mode_reason：Windows PowerShell 5.1 的 Get-Content 默认按
        # ANSI 读，会把 UTF-8 字节截断成非法 JSON——必须显式 -Encoding UTF8。
        $bench50k = Get-Content -LiteralPath $gpuParticles50kPath -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($bench50k.report_version -ne "wp0-gpu-particles-50k-v1") {
            throw "gpu particles 50k report version invalid: $($bench50k.report_version)"
        }
        $p50 = $bench50k.semantic
        if ($p50.mode -ne "gpu" -or $p50.fallback -ne "none") {
            throw "gpu particles 50k is not the GPU path: mode=$($p50.mode) fallback=$($p50.fallback)"
        }
        if ($p50.count -ne 50000 -or $p50.capacity -ne 50000 -or $p50.spawned -ne 50000) {
            throw "gpu particles 50k scale changed: count=$($p50.count) capacity=$($p50.capacity) spawned=$($p50.spawned)（R07-4 的验收规模是 50k）"
        }
        if ($p50.state_bytes -ne 2400000 -or $p50.particle_buffer_bytes -ne ($p50.state_bytes + 64)) {
            throw "gpu particles 50k buffer layout drifted: state=$($p50.state_bytes) total=$($p50.particle_buffer_bytes)（期望 50000×48 B，params 64 B）"
        }
        if (-not $p50.memory_ok -or $p50.particle_buffer_bytes -gt $p50.memory_budget_bytes) {
            throw "gpu particles 50k exceeds memory budget: $($p50.particle_buffer_bytes) > $($p50.memory_budget_bytes)"
        }
        if (-not $p50.buffer_stable) {
            throw "gpu particles 50k: 状态缓冲帧间增长（容量漂移/重复建缓冲）"
        }
        if ($p50.draw_calls -ne 1 -or $p50.instances -ne 50000) {
            throw "gpu particles 50k: instanced 记账变了 draws=$($p50.draw_calls) instances=$($p50.instances)（一次 draw、实例数 = capacity）"
        }
        if ($p50.vertices -ne 200000 -or $p50.triangles -ne 100000) {
            throw "gpu particles 50k: 顶点/三角形不对 vertices=$($p50.vertices) triangles=$($p50.triangles)（4×/2× capacity）"
        }
        if ($p50.dropped_draws -ne 0) {
            throw "gpu particles 50k: 本帧丢弃了 $($p50.dropped_draws) 次 draw（管线或绑定组不可用）"
        }
        if ($p50.non_finite -ne 0) {
            throw "gpu particles 50k: 状态里出现非有限值（$($p50.non_finite)）"
        }
        if ($p50.gpu_alive -le 0 -or $p50.gpu_alive -gt 50000 -or $p50.cpu_live -le 0) {
            throw "gpu particles 50k: 存活数异常 gpu=$($p50.gpu_alive) cpu=$($p50.cpu_live)（应落在 (0, 50000]）"
        }
        if ($p50.content_pixels -le 200) {
            throw "gpu particles 50k: 粒子几乎没画进附件（content=$($p50.content_pixels)）"
        }
        if ($p50.default_mode -ne "cpu" -and $p50.default_mode -ne "gpu") {
            throw "gpu particles 50k: 默认档取值非法（$($p50.default_mode)）"
        }
        if ($p50.default_mode -eq "gpu") {
            # 选 gpu 必须真的有收益：任一条门槛成立，且总帧不慢于 CPU 档。
            if (-not ($p50.cpu_side_gate -or $p50.total_gate) -or $p50.total_ratio -gt 1.0) {
                throw "gpu particles 50k: 默认档选了 gpu 但门槛不成立 cpu_side=$($p50.cpu_side_ratio) total=$($p50.total_ratio)"
            }
        }
        elseif ([string]::IsNullOrWhiteSpace($p50.default_mode_reason)) {
            throw "gpu particles 50k: 默认档为 cpu 但没记录理由（结论必须可复核）"
        }
        Write-Host ("gpu_particles_50k_ok count=50000 state_bytes=2400000 default_mode={0} cpu_side_ratio={1:N3} total_ratio={2:N3}" -f `
            $p50.default_mode, $p50.cpu_side_ratio, $p50.total_ratio) -ForegroundColor Green
    }
    Write-Host "WP0 benchmark baselines and semantic golden written and schema-checked: $baselineRoot" -ForegroundColor Green
}

# wasm Lua 运行期验证门：把 scripts/lib/wasm-lua 下的两个 Node 用例（no-modules 桥 + gpu2d 宿主画布探针）
# 纳入可重复门。与 -Web 同一子进程约定——Start-Process -PassThru 在重定向下 ExitCode 是
# $null，退出码**只**认子进程写的 StatusFile；缺失即判失败。0=通过 / 1=判红 / 3=环境受限跳过。
if ($WasmLua) {
    Write-Step "wasm Lua runtime evidence"
    $wasmEvidenceRoot = Join-Path $projectRoot "artifacts\logs\wasm-lua"
    New-Item -ItemType Directory -Force -Path $wasmEvidenceRoot | Out-Null
    $wasmScript = Join-Path $projectRoot "scripts\verify\verify-wasm-lua.ps1"
    if (-not (Test-Path -LiteralPath $wasmScript)) {
        throw "wasm-lua verification script not found: $wasmScript"
    }
    $logFile = Join-Path $wasmEvidenceRoot "gate-wasm-lua-$timestamp.log"
    $statusFile = "$logFile.status"
    $timeoutSeconds = 900
    $process = Start-Process -FilePath "powershell" `
        -ArgumentList @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $wasmScript, "-StatusFile", $statusFile) `
        -PassThru -RedirectStandardOutput $logFile -RedirectStandardError "$logFile.err"
    if (-not $process.WaitForExit($timeoutSeconds * 1000)) {
        Write-Host "wasm-lua evidence timed out after ${timeoutSeconds}s; killing it" -ForegroundColor Yellow
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        throw "wasm-lua verification timed out (treated as failure, not skip)"
    }
    $lines = if (Test-Path -LiteralPath $logFile) { Get-Content -LiteralPath $logFile } else { @() }
    $lines | Select-Object -Last 20 | ForEach-Object { Write-Host $_ }
    $lines | ForEach-Object { Add-Content -Path $logPath -Value $_ -Encoding UTF8 }
    if (Test-Path -LiteralPath "$logFile.err") {
        $errLines = Get-Content -LiteralPath "$logFile.err" -ErrorAction SilentlyContinue
        if ($errLines) {
            Write-Host "wasm-lua evidence stderr:" -ForegroundColor Yellow
            $errLines | Select-Object -First 12 | ForEach-Object {
                Write-Host "  $_"
                Add-Content -Path $logPath -Value "  $_" -Encoding UTF8
            }
        }
    }
    $exitCode = 1
    if (Test-Path -LiteralPath $statusFile) {
        $raw = (Get-Content -LiteralPath $statusFile -Raw).Trim()
        $parsed = 0
        if ([int]::TryParse(($raw -replace '^exit=', ''), [ref]$parsed)) {
            $exitCode = $parsed
        }
    }
    else {
        Write-Host "wasm-lua evidence wrote no status file (treated as failure)" -ForegroundColor Yellow
    }
    if ($exitCode -eq 1) {
        throw "wasm-lua verification FAILED (verify-wasm-lua.ps1 exit 1)"
    }
    if ($exitCode -eq 0) {
        # 成功必须是**本轮**的新鲜证据：wrapper 自己会在通过时打 wasm_lua_ok，日志里没有
        # 就说明读到的是陈旧/空输出——退出码 0 不足以证明"这一次真的跑过了"。
        $hasOk = $false
        foreach ($line in $lines) { if ($line -match 'wasm_lua_ok') { $hasOk = $true } }
        if (-not $hasOk) {
            throw "wasm-lua verification reported success but wrote no wasm_lua_ok line (stale evidence is not evidence)"
        }
        Write-Host "wasm_lua_ok (no-modules bridge + gpu2d host probe passed)" -ForegroundColor Green
    }
    else {
        Write-Host "wasm_lua_skipped reason=environment_limited exit=$exitCode (缺 node/wasm32 target/zig/wasm-bindgen)" -ForegroundColor Yellow
    }
}

# P4-01 覆盖率门：全 workspace 采集 + 棘轮。哲学与 -Web/-WasmLua 一致：
# 缺工具/缺基线退 3（BLOCKED，打印原因），测试失败退 1——覆盖率数字不能掩盖红测试。
if ($Coverage) {
    Write-Step "Coverage gate (P4-01)"
    $coverageScript = Join-Path $projectRoot "scripts/verify/check-coverage.ps1"
    if (-not (Test-Path -LiteralPath $coverageScript)) {
        Write-Host "coverage_blocked reason=missing_script path=$coverageScript（P4-01 未落地）" -ForegroundColor Yellow
        exit 3
    }
    & powershell -NoProfile -ExecutionPolicy Bypass -File $coverageScript -RequireTool
    if ($LASTEXITCODE -eq 3) {
        Write-Host "coverage_blocked reason=missing_tool_or_baseline（缺 cargo-llvm-cov / llvm-tools-preview / 基线；先跑 install-metrics-tools.ps1，见 metrics/README.md）" -ForegroundColor Yellow
        exit 3
    }
    if ($LASTEXITCODE -ne 0) {
        throw "coverage gate failed (check-coverage.ps1 exit $LASTEXITCODE)"
    }
}

# P4-04/06/07：对**已有产物**做尺寸采集 + cfg 门 + 趋势检查，不重跑 benchmark/构建。
# 依赖脚本未落地时显式退 3（BLOCKED），不静默跳过。
if ($Metrics) {
    Write-Step "Metrics gate (P4-04/06/07)"
    $cfgScript = Join-Path $projectRoot "scripts/verify/check-cfg-coverage.ps1"
    if (-not (Test-Path -LiteralPath $cfgScript)) {
        Write-Host "metrics_blocked reason=missing_script path=$cfgScript（P4-02 未落地）" -ForegroundColor Yellow
        exit 3
    }
    & powershell -NoProfile -ExecutionPolicy Bypass -File $cfgScript
    if ($LASTEXITCODE -ne 0) {
        throw "cfg coverage gate failed (check-cfg-coverage.ps1 exit $LASTEXITCODE)"
    }
    $measureScript = Join-Path $projectRoot "scripts/metrics/measure-artifacts.mjs"
    if (-not (Test-Path -LiteralPath $measureScript)) {
        Write-Host "metrics_blocked reason=missing_script path=$measureScript（P4-06/07 未落地）" -ForegroundColor Yellow
        exit 3
    }
    $metricsArtifacts = Join-Path $projectRoot "artifacts\metrics"
    New-Item -ItemType Directory -Force -Path $metricsArtifacts | Out-Null
    node $measureScript --kind binary --out (Join-Path $metricsArtifacts "binaries.json")
    if ($LASTEXITCODE -ne 0) {
        throw "binary size collection failed (exit $LASTEXITCODE)"
    }
    node $measureScript --kind package --project $defaultGameProject --out (Join-Path $metricsArtifacts "packages.json")
    if ($LASTEXITCODE -ne 0) {
        throw "package size collection failed (exit $LASTEXITCODE)"
    }
    $checkMetrics = Join-Path $projectRoot "scripts/verify/check-metrics.ps1"
    if (-not (Test-Path -LiteralPath $checkMetrics)) {
        Write-Host "metrics_blocked reason=missing_script path=$checkMetrics（P4-04 未落地）" -ForegroundColor Yellow
        exit 3
    }
    & powershell -NoProfile -ExecutionPolicy Bypass -File $checkMetrics
    if ($LASTEXITCODE -eq 3) {
        Write-Host "metrics_blocked reason=insufficient_history（缺历史样本；先入库，见 metrics/README.md §8）" -ForegroundColor Yellow
        exit 3
    }
    if ($LASTEXITCODE -ne 0) {
        throw "metrics trend check failed (check-metrics.ps1 exit $LASTEXITCODE)"
    }
    Write-Host "metrics_gate_passed" -ForegroundColor Green
}

if (-not $KeepLog) {
    Write-Host "Quality log retained for review and regression." -ForegroundColor DarkGray
}

