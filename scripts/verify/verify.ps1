[CmdletBinding()]
param(
    [switch]$SkipClippy,
    [switch]$SkipTests,
    [switch]$SkipBuild,
    [switch]$Release,
    [switch]$KeepLog,
    # MVP 退出门：历史上是《江湖有田》的 headless 回归 + 干净发行目录自测。
    # jhyt 工程已整体迁出仓库（E:\Qoder\game_jhyt），81 个 jhyt_* 示例与清单已删除；
    # 现在这一段只跑默认样板工程（examples/game_api）的 package.ps1 -SelfTest。
    [switch]$Mvp,
    # 仅跑 MVP 这一段（跳过 fmt/clippy/test/build 常规门）。
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
    [int]$MaxJobs = 0,
    # 起跑前的「树是否在动」探针阈值（秒）：源文件在这个窗口内被写过就退 3（blocked，不是红）。
    # 完整门要 25–45 分钟，边写边跑会把半保存态编译成假红（2026-10-27 实测三次）。
    # 0 = 关闭探针；`-Quick` 不受影响（它的语义就是「刚存完，现在到底红不红」）。
    [int]$TreeQuietSeconds = 60
)

# 证据模式（-Web / -Benchmarks / -WasmLua / -Coverage / -Metrics）的语义：
# 只跑对应证据段，不串行跑预检与主门；完整认证仍然是不带这些开关跑一次。
# 这样 Web/基准不会因为无关的文档/lint 在途红而“一行都没执行”（M78 §26.3 第 24 条）。
# 预检跳过时会显式打印未验证的步骤——跳过绝不写成通过。

# ── -Quick：回答「这棵树现在是不是红的」的便宜读数 ─────────────────────────────
# 为什么需要它：pre-commit hook 只镜像 CI 的 lint job（`git diff --cached --check` + fmt + clippy），
# 而真正咬人的**数据级工程校验**（`game --check` / `editor --check`：场景引用的资源、脚本语法、
# `validate_data.lua`、组件 schema）此前只存在于总门里。总门在 8 核 / 16 GB 的开发机上要几十分钟
# 到几小时，于是「想拿一次读数」的成本高到没人愿意付——代价已经真实发生过：`scenes/main.scene`
# 的一句中文注释让 `game --check` 判红，这条红在本地 HEAD 里活了 5 个提交才被发现
# （见 `docs/changelog/发布-CI与质量门.md`「场景注释不再冒充依赖」）。`-Quick` 把这一类红的发现成本降到几十秒（热缓存）。
#
# 跑什么：文档门、vendor 补丁门、cargo metadata、fmt、**wasm32 平台编译（game_web）**、
#         Lua API 文档桩、Web API 覆盖清单、git diff --check、两个判据自测、
#         `game --check`、`editor --check`（两个工程）。
# 不跑什么（每一步都会在结尾逐个打印出来）：clippy、rustdoc、对外 Rust 符号面门（依赖 rustdoc
#         产物）、pre-commit hook 回归、全量测试、原生构建、空间迁移报告、编辑器性能基线、GUI smoke。
# 为什么 wasm 编译进 -Quick：它是唯一能回答「WebGL/微信还编不编得过」的读数，热缓存下约 10 s
#         （2026-11-02 实测 10.5 s）。此前它只存在于 opt-in 的 -WasmLua/-Web 与 CI matrix 里，
#         结果是 `rustlua-gameplay/src/net_lua.rs` 的 E0282 在 HEAD 上活了两天没人发现——
#         本机 -Quick 全绿、平台链却是断的。跳过清单里没有它，因为它不是"重活"。
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
# 工程校验与干净目录自测的默认目标工程：仓库里唯一的样板工程是 `examples/game_api`
# （《江湖有田》已迁出仓库）。根目录没有 project.toml——写死 `examples` 会让
# 「Game project check」必然失败。
$defaultGameProject = Join-Path $gameRoot "game_api"
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

# 证据模式只跑自己的段：`-Mvp` 仍含全量门，`-MvpOnly` 仍跑预检（文档/版本等廉价门），
# `-Security` 也保留预检（它要工具链锁定），其余五个证据开关不跑预检/主门。
$evidenceOnly = $Web -or $Benchmarks -or $WasmLua -or $Coverage -or $Metrics
$runPreflight = (-not $evidenceOnly) -or $MvpOnly -or $Security
$runGates = -not ($MvpOnly -or $evidenceOnly)

if ($runPreflight) {

# 树稳定性探针（放在所有门之前，毫秒级）：源文件最近被写过就说明有人在边写边跑，
# 后续 25–45 分钟的编译/测试读数全部不可信——这里退 **3 = blocked**（与 GPU/wasm 的
# exit 3 同一语义），而不是跑完再交一份"看起来像回归"的日志。探针自己扫不到文件时判红
# （配错的守卫不能放行）。`-Quick` 跳过：它的定义就是"刚存完，现在红不红"。
# CI 上 checkout 把整棵树写在同一秒，wrapper 自己按 env:CI 显式跳过并打印原因。
if (-not $Quick) {
    Write-Step "Tree stability probe"
    $treeQuietScript = Join-Path $projectRoot "scripts/verify/check-tree-quiet.ps1"
    if (-not (Test-Path -LiteralPath $treeQuietScript)) {
        throw "check-tree-quiet.ps1 missing: $treeQuietScript"
    }
    $treeOutput = & powershell -NoProfile -ExecutionPolicy Bypass -File $treeQuietScript -QuietSeconds $TreeQuietSeconds
    $treeExit = $LASTEXITCODE
    $treeOutput | ForEach-Object { Write-Host $_ }
    if ($treeExit -eq 3) {
        Write-Host "quality_gate_blocked reason=tree_busy window_s=$TreeQuietSeconds（读数不可信，不是代码红；等保存停手后重跑）" -ForegroundColor Yellow
        exit 3
    }
    if ($treeExit -ne 0) {
        throw "check-tree-quiet.ps1 failed (exit code $treeExit)"
    }
}

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

# 文档声明数字门：README/docs 里人写的「42 个模块表 / 313 个 Lua 函数」「29 个运行时组件」
# 必须等于机器事实（API 快照 + `ComponentId::ALL`）。`check-docs.ps1` 只查链接/编号/H1，
# 查不出摘要数字过期；`gen-lua-api-docs.js --check` 只查文档桩清单，同样不覆盖摘要。
# 纯文本 + 两个源文件，毫秒级，`-MvpOnly` 也跑。缺 node 时显式跳过并注明未验证，不写成通过。
Write-Step "Documentation claims"
$docClaimsScript = Join-Path $projectRoot "scripts/verify/check-doc-claims.mjs"
if (Test-Path -LiteralPath $docClaimsScript) {
    $nodeCommand = Get-Command node -ErrorAction SilentlyContinue
    if (-not $nodeCommand) {
        Write-Host "doc_claims_skipped reason=node_missing (NOT verified)" -ForegroundColor Yellow
    }
    else {
        $claimsOutput = & node $docClaimsScript 2>&1
        $claimsExit = $LASTEXITCODE
        $claimsOutput | ForEach-Object { Write-Host $_ }
        if ($claimsExit -ne 0) {
            throw "check-doc-claims.mjs failed (exit code $claimsExit)"
        }
        $claimsSelfTest = & node $docClaimsScript "--self-test" 2>&1
        $claimsSelfExit = $LASTEXITCODE
        $claimsSelfTest | ForEach-Object { Write-Host $_ }
        if ($claimsSelfExit -ne 0) {
            throw "check-doc-claims.mjs --self-test failed (exit code $claimsSelfExit)"
        }
    }
}
else {
    throw "check-doc-claims.mjs missing: $docClaimsScript"
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

# 版本一致性门（P0-02）：引擎 workspace 版本与样板工程 `examples/game_api` 的 project.toml 版本必须一致，
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
    # M78 §26.3-30：缺 SBOM 不再静默跳过——先生成再比。gen-sbom.mjs 离线可跑
    # （输入只有 cargo metadata + engine/Cargo.lock，不联网、不重新解析依赖）；
    # 生成失败判红，杜绝"缺文件就把跳过态读成通过"。
    if (-not (Test-Path -LiteralPath $sbomCandidate)) {
        Write-Host "==> Generating SBOM (missing): $sbomCandidate" -ForegroundColor Cyan
        $genOutput = & node (Join-Path $projectRoot "scripts/release/gen-sbom.mjs") "--output" $sbomCandidate 2>&1
        $genExit = $LASTEXITCODE
        $genOutput | ForEach-Object { Write-Host $_ }
        if ($genExit -ne 0 -or -not (Test-Path -LiteralPath $sbomCandidate)) {
            throw "SBOM generation failed (exit code $genExit): $sbomCandidate"
        }
    }
    $securityArgs += @("-SbomPath", $sbomCandidate)
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
}
else {
    Write-Host "==> Preflight skipped: evidence-only mode (-Web/-Benchmarks/-WasmLua/-Coverage/-Metrics)." -ForegroundColor Yellow
    Write-Host "    NOT verified in this run: documentation consistency, documentation claims, silent-failure traces," -ForegroundColor Yellow
    Write-Host "    vendor patches, toolchain lock, project version consistency, decoupling, supply-chain security." -ForegroundColor Yellow
    Write-Host "    This run is evidence for the selected mode only; it is NOT a full quality-gate pass." -ForegroundColor Yellow
}

# `-Web` 只跑 Web 证据段：进入下面这个大块是为了复用它的日志/收尾结构，但主门那部分
# 由块内的 `if ($runGates)` 守；其余证据开关（-Benchmarks/-WasmLua/-Coverage/-Metrics）
# 的证据段在各自顶层，连这个块都不进。
if ($runGates -or $Web) {
Push-Location $engineRoot
try {
    if ($runGates) {
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
        # `engine/crates/rustlua-ecs/src/tilemap.rs`（crate 内测试）钉着（jhyt 迁移示例已随工程迁出删除）。
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
        Write-Step "Flaky CI skip-ledger (static parity)"
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

    if (-not $SkipClippy) {
        Write-Step "Clippy"
        Invoke-Checked cargo @("clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings")
    }

    # wasm 平台编译（P0 回归门，2026-11-02）：`game_web` 是 WebGL 与微信小游戏两条发布链的
    # 共同入口，而这条命令此前只出现在 opt-in 的 `-WasmLua`/`-Web` 与 CI matrix 里。代价是
    # `rustlua-gameplay/src/net_lua.rs` 的 `#[cfg(target_arch = "wasm32")] let stats = None;`
    # 推不出类型（E0282），HEAD 上 WebGL/微信产物**根本编不出来**，本机 -Quick 却全绿。
    # 热缓存实测（2026-11-02）：两条命令 1.3 s + 0.5 s（无改动时近乎空跑），改过
    # game_web/gameplay 之后约 10 s；整个 `-Quick` 端到端 233 s。因此不登记进 -Quick 跳过
    # 清单：它不是重活，而"这棵树红不红"必须包含平台分支。
    # 缺 wasm32 目标时打印原因跳过（跳过不等于通过），不判红。
    # 口径说明：这里用的是仓库默认工具链（与 CI matrix 的 wasm cell 同一口径，1.97.1）；
    # 发布链另有自己的 wasm 工具链（package.ps1 固定 1.88.0，见其 WP10 的 EH 口径），
    # 两者不互相替代——这一步只回答"代码还编不编得过"，产物由 package.ps1/verify-webgl-bundle 证明。
    Write-Step "WASM platform compile (game_web -> wasm32-unknown-unknown)"
    $installedRustTargets = @()
    if (Get-Command rustup -ErrorAction SilentlyContinue) {
        $installedRustTargets = @(& rustup target list --installed 2>$null)
    }
    if ($installedRustTargets -contains "wasm32-unknown-unknown") {
        # 包集与 CI 的 wasm cell **逐字对齐**（2026-11-02）：此前这里只查 `-p game_web`，
        # 而 matrix.yml 的 blocking cell 查的是 `-p game_web -p gpu-web-probe`，另有
        # `-p game_web --no-default-features` 一格。两条命令的漂移由
        # `check-platform-parity.ps1` 当场判红，所以这里改包集必须同步改 matrix.yml。
        Invoke-Checked cargo @("check", "-p", "game_web", "-p", "gpu-web-probe", "--target", "wasm32-unknown-unknown", "--locked")
        Invoke-Checked cargo @("check", "-p", "game_web", "--no-default-features", "--target", "wasm32-unknown-unknown", "--locked")
    } else {
        Write-Host "  [SKIP] 未安装 wasm32-unknown-unknown 目标：WebGL/微信平台编译未运行（不是通过；先 rustup target add wasm32-unknown-unknown）" -ForegroundColor Yellow
    }

    # 本地门与 CI cell 的命令集漂移检查（P4-03）：HEAD `7d951c84` 上 CI 的 4 个 wasm cell 全红、
    # 而本地门只查了 `-p game_web` 的那一段——"两边各写一条命令"才是根因。这份脚本只比命令文本
    # （忽略空白与 `--locked`），毫秒级，因此放在 -Quick 里也不占成本。
    Write-Step "Platform cell parity (matrix.yml vs verify.ps1)"
    & (Join-Path $PSScriptRoot "check-platform-parity.ps1")
    if ($LASTEXITCODE -ne 0) {
        throw "Platform cell parity failed (exit $LASTEXITCODE)：本地门与 CI 的 wasm 命令集不一致"
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
            # 上面的检查只证明场景自报的不变量通过，**不约束耗时**。这里把每个场景的 p95 与
            # `metrics/editor-perf-budgets.json` 的硬上限比较：超预算、报告里出现没有预算的场景、
            # 预算里的场景从报告消失，三者都判红（新增/删除慢场景不能悄悄绕过门）。
            # 同机同比仍由趋势库（`trend_db.py check`）负责，两层互补。
            $editorPerfGate = Join-Path $projectRoot "scripts/verify/check-editor-perf.ps1"
            $editorPerfBudget = Join-Path $projectRoot "metrics/editor-perf-budgets.json"
            if (-not (Test-Path -LiteralPath $editorPerfGate)) {
                throw "editor performance budget gate missing: $editorPerfGate"
            }
            $budgetSelfTest = & powershell -NoProfile -ExecutionPolicy Bypass -File $editorPerfGate -SelfTest
            $budgetSelfExit = $LASTEXITCODE
            $budgetSelfTest | ForEach-Object { Write-Host $_ }
            if ($budgetSelfExit -ne 0) {
                throw "editor performance budget gate self-test failed (exit code $budgetSelfExit)"
            }
            $budgetOutput = & powershell -NoProfile -ExecutionPolicy Bypass -File $editorPerfGate -Report $editorBaselinePath -Budget $editorPerfBudget
            $budgetExit = $LASTEXITCODE
            $budgetOutput | ForEach-Object { Write-Host $_ }
            if ($budgetExit -ne 0) {
                throw "editor performance budget gate failed (exit code $budgetExit); report: $editorBaselinePath"
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
    } # end of mainstream gate steps (skipped in evidence-only mode)

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
        # 进度标记：CI 上发成 `::warning::` annotation（job 日志要认证才能读，annotation 不要），
        # 本机只打一行普通输出。用途是把"卡住"和"在编译"区分开——2026-11-03 实测这一步在 CI 上
        # 一个多小时没有任何输出，只能干等。
        # **阶段流水账**：每一步都记一行到文件。CI 的 annotation 只在**步骤结束时**才刷新，
        # 于是"步骤被 job 的 timeout 掐掉"这种收场**一条注解都读不到**（2026-11-03 实测：
        # web-evidence 跑满 150 分钟被 cancel，annotations=0）。所以留一份文件，失败时把它
        # 的尾巴一起倒出来——它不受 annotation 刷新时机影响。
        $webPhaseLog = Join-Path $webEvidenceRoot "phases-$timestamp.log"
        $emitProgress = {
            param([string]$Message)
            $line = "web evidence: $Message"
            Write-Host $line
            try { Add-Content -LiteralPath $webPhaseLog -Value ("{0} {1}" -f (Get-Date -Format "HH:mm:ss"), $Message) -Encoding UTF8 } catch { }
            if ($env:GITHUB_ACTIONS -eq 'true') { Write-Host "::warning::$line" }
        }
        $publishLimitMin = 75
        if ($env:RUSTLUA_PUBLISH_TIMEOUT_MIN -match '^\d+$') { $publishLimitMin = [int]$env:RUSTLUA_PUBLISH_TIMEOUT_MIN }
        # **整段证据的总预算**：单个阶段有界不代表整段有界——6 个阶段各烧满 10 分钟 + 发布 75 分钟
        # 正好能把 150 分钟的 job 撑爆，而"被掐掉"比"判红"更糟：连注解都没有。默认 100 分钟，
        # $env:RUSTLUA_WEB_EVIDENCE_TIMEOUT_MIN 可覆盖；超预算时**带着流水账判红**。
        $webEvidenceLimitMin = 100
        if ($env:RUSTLUA_WEB_EVIDENCE_TIMEOUT_MIN -match '^\d+$') { $webEvidenceLimitMin = [int]$env:RUSTLUA_WEB_EVIDENCE_TIMEOUT_MIN }
        $webEvidenceStart = Get-Date
        # 阶段子进程用**和本进程同一个宿主**：CI 的步骤跑在 pwsh 7 上，若这里固定写 powershell(5.1)，
        # 子进程会继承 7 的 `PSModulePath` 却按 5.1 的 `$PSHOME` 找模块 ⇒ 用到才自动加载的命令
        # 直接 CommandNotFoundException（2026-11-03 在发布子进程里实测过同一条）。本机只有 5.1，
        # 父与子同宿主，故退回 powershell。
        $childHostExe = (Get-Command pwsh -ErrorAction SilentlyContinue | Select-Object -First 1).Source
        if (-not $childHostExe) { $childHostExe = (Get-Command pwsh -ErrorAction SilentlyContinue | Select-Object -First 1).Path }
        if (-not $childHostExe) { $childHostExe = "powershell" }

        $runWebEvidence = {
            param([string]$Script, [string]$Label, [string[]]$ExtraArgs = @())
            $logFile = Join-Path $webEvidenceRoot ("gate-" + $Label + "-$timestamp.log")
            $statusFile = "$logFile.status"
            # **有界等待**，不用 `-Wait`：`-Wait` 会等到整个进程树结束，而无头浏览器偶尔不自行退出，
            # 于是质量门会一直挂在那里（本轮实际踩到）。这里自己轮询 + 超时即杀，并把它当失败
            # ——宁可判红，也不要"卡住"。
            # 单个阶段的上限：900 秒（15 分钟）。**不要往下调**——这些阶段里有 `gpu-probe` /
            # `render-probe` 那种要在 CI 上冷编 wasm32 probe 的，本机秒级不代表 CI 秒级；
            # 真正的"整段有界"由下面的总预算（默认 100 分钟）负责，它到点会带着阶段流水账判红，
            # 既能给出文件名，也不会把"慢"误判成"红"。2026-11-03 的教训是"6×600 秒 + 发布 75 分钟
            # 正好撑爆 150 分钟的 job 并被无声掐掉"，解法是总预算 + 流水账，不是把单阶段掐到很短。
            $timeoutSeconds = 900
            # 总预算到点就判红（见上面 $webEvidenceLimitMin 的说明），并带上流水账尾巴。
            $spentMin = [int]((Get-Date) - $webEvidenceStart).TotalMinutes
            if ($spentMin -ge $webEvidenceLimitMin) {
                $phaseTail = "(no phases)"
                if (Test-Path -LiteralPath $webPhaseLog) { $phaseTail = ((Get-Content -LiteralPath $webPhaseLog -Tail 20) -join "`n") }
                throw "web evidence 总预算 ${webEvidenceLimitMin} 分钟用尽（停在阶段 '$Label' 之前）。阶段流水账：`n$phaseTail"
            }
            $startedAt = Get-Date
            $argumentList = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $Script, "-StatusFile", $statusFile) + $ExtraArgs
            & $emitProgress ("{0}_start timeout={1}s" -f $Label, $timeoutSeconds)
            $process = Start-Process -FilePath $childHostExe `
                -ArgumentList $argumentList `
                -PassThru -RedirectStandardOutput $logFile -RedirectStandardError "$logFile.err"
            if (-not $process.WaitForExit($timeoutSeconds * 1000)) {
                Write-Host "web evidence '$Label' timed out after ${timeoutSeconds}s; killing it" -ForegroundColor Yellow
                & $emitProgress ("{0}_timeout after={1}s" -f $Label, $timeoutSeconds)
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
            # 失败时把子脚本日志的尾 40 行**逐行发成注解**：CI 上 job 日志要认证才能读，
            # 而"失败原因在子脚本的 stdout 里、只抄了 12 行"已经害过好几轮（2026-11-04：
            # bundle 阶段 0 个候选，否决理由就在被截掉的那几行里）。
            if ($env:GITHUB_ACTIONS -eq 'true') {
                $childExit = 1
                if (Test-Path -LiteralPath $statusFile) {
                    $raw2 = (Get-Content -LiteralPath $statusFile -Raw).Trim() -replace '^exit=', ''
                    [int]$p2 = 1
                    if ([int]::TryParse($raw2, [ref]$p2)) { $childExit = $p2 }
                }
                if ($childExit -ne 0) {
                    foreach ($row in ($lines | Select-Object -Last 40)) {
                        $line2 = ("{0}" -f $row).TrimEnd()
                        if ($line2) { Write-Host "::error::web evidence $Label log: $line2" }
                    }
                }
            }
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
            & $emitProgress ("{0}_done exit={1} elapsed={2}s" -f $Label, $exitCode, [int]((Get-Date) - $startedAt).TotalSeconds)
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
            # CI 上"卡住"和"在编译"必须分得出来（2026-11-03 实测：这一步在 CI 上跑了一个多小时，
            # 中间一行输出都没有，而 job 日志要认证才能读）。$emitProgress 在上一段定义好了：
            # 每 10 分钟发一条进度，CI 上是 `::warning::` annotation（不需要认证）。
            if (-not $existingBundle -and $webglBuilds.Count -eq 0) {
                Write-Step "Publish WebGL bundle (no published artifact found)"
                $publishLog = Join-Path $webEvidenceRoot ("publish-webgl-$timestamp.log")
                $publishStatus = "$publishLog.status"
                $bootstrap = Join-Path $webEvidenceRoot ("publish-webgl-$timestamp.ps1")
                # 退出码**只认子进程自己写的状态文件**：PS 5.1 上 `Start-Process -PassThru
                # -RedirectStandardOutput` 的 `.ExitCode` 是 $null，`[int]$null` 静默变 0，
                # "失败"会被读成"通过"（`$runWebEvidence` 的注释里记着同一条）。`package.ps1`
                # 自己不写状态文件，所以外面套一层 bootstrap 代写。
                $bootstrapBody = @(
                    'param([string]$PackageScript, [string]$ProjectRoot, [string]$StatusFile)',
                    '$ErrorActionPreference = "Continue"',
                    '# 用**和父进程同一个宿主**：CI 的步骤跑在 pwsh 7 上。以前这里固定写 powershell(5.1)，',
                    '# 子进程会继承 7 的 PSModulePath、却按 5.1 的 $PSHOME 找模块 ⇒ 模块自动加载失败，',
                    '# `Get-FileHash` 这类"用到才加载"的命令直接 CommandNotFoundException（2026-11-03 实测），',
                    '# 发布就死在一个与打包逻辑无关的地方。优先 pwsh；没有就退回 powershell，并把',
                    '# $PSHOME\Modules 拼回 PSModulePath——两条都要，缺一条都可能再踩回去。',
                    '$hostExe = (Get-Command pwsh -ErrorAction SilentlyContinue | Select-Object -First 1).Source',
                    'if (-not $hostExe) { $hostExe = (Get-Command pwsh -ErrorAction SilentlyContinue | Select-Object -First 1).Path }',
                    'if (-not $hostExe) { $hostExe = "powershell" }',
                    '$env:PSModulePath = (Join-Path $PSHOME "Modules") + ";" + $env:PSModulePath',
                    '& $hostExe -NoProfile -ExecutionPolicy Bypass -File $PackageScript -ProjectRoot $ProjectRoot -Platform webgl',
                    '$code = $LASTEXITCODE',
                    'Set-Content -LiteralPath $StatusFile -Value ("exit=" + $code) -Encoding ASCII',
                    'exit $code'
                ) -join "`n"
                [IO.File]::WriteAllText($bootstrap, $bootstrapBody, [Text.UTF8Encoding]::new($true))
                & $emitProgress "publish_start platform=webgl timeout=${publishLimitMin}m log=$publishLog"
                $publishStart = Get-Date
                $publishProc = Start-Process powershell -PassThru -NoNewWindow -ArgumentList @(
                    "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $bootstrap,
                    "-PackageScript", (Join-Path $projectRoot "scripts\build\package.ps1"),
                    "-ProjectRoot", $defaultGameProject, "-StatusFile", $publishStatus
                ) -RedirectStandardOutput $publishLog -RedirectStandardError "$publishLog.err"
                $lastMark = 0
                while (-not $publishProc.HasExited) {
                    Start-Sleep -Seconds 10
                    $elapsedMin = [int]((Get-Date) - $publishStart).TotalMinutes
                    if ($elapsedMin -ge ($lastMark + 10)) {
                        $lastMark = $elapsedMin
                        $tailLine = ""
                        if (Test-Path -LiteralPath $publishLog) {
                            $tailLine = ((Get-Content -LiteralPath $publishLog -Tail 1) -join " ").Trim()
                        }
                        & $emitProgress "publish_running elapsed=${elapsedMin}m tail=$tailLine"
                    }
                    if ($elapsedMin -ge $publishLimitMin) {
                        # 杀**进程树**：publish 会起 cargo/rustc/zig，只杀父进程会留下孙进程继续烧 CPU。
                        & taskkill /T /F /PID $publishProc.Id 2>&1 | Out-Null
                        throw "WebGL publish 超过 ${publishLimitMin} 分钟未完成（判为挂死，已杀进程树）。日志：$publishLog"
                    }
                }
                $publishExit = 1
                if (Test-Path -LiteralPath $publishStatus) {
                    $rawStatus = (Get-Content -LiteralPath $publishStatus -Raw).Trim()
                    $parsedStatus = 0
                    if ([int]::TryParse(($rawStatus -replace '^exit=', ''), [ref]$parsedStatus)) {
                        $publishExit = $parsedStatus
                    }
                }
                $publishElapsed = [int]((Get-Date) - $publishStart).TotalMinutes
                & $emitProgress "publish_done exit=$publishExit elapsed=${publishElapsed}m"
                # 发布说自己成功之后，**当场**把产物再发现一次并报出候选/否决原因。
                # 2026-11-04 实测：CI 上 `publish_done exit=0` 紧接着 bundle 阶段报"先发布一个"
                # （0 个候选），而同一份代码在本机干净 worktree 里发布 7 分钟就产出齐备产物、候选=1。
                # 差异只能在 CI 的产物布局上，光看退出码看不出来——所以把 `Find-GameBundle` 的
                # 扫描根与否决理由直接发成注解，下一次跑就能指名是哪个文件缺了。
                if ($publishExit -eq 0) {
                    try {
                        $postReport = Find-GameBundle -Platform "webgl" -WorkspaceRoot $projectRoot
                        & $emitProgress ("publish_bundle_candidates=" + @($postReport.Candidates).Count)
                        foreach ($root2 in @($postReport.ScanRoots)) { & $emitProgress ("publish_bundle_scan_root=" + $root2) }
                        foreach ($rej in @($postReport.Rejected)) { & $emitProgress ("publish_bundle_rejected: " + $rej) }
                        foreach ($cand in @($postReport.Candidates)) { & $emitProgress ("publish_bundle_candidate=" + $cand.Path) }
                    }
                    catch {
                        & $emitProgress ("publish_bundle_report_failed: " + $_.Exception.Message)
                    }
                    # 胶水指纹：2026-11-04 CI 上 bundle 阶段报 `Failed to resolve module specifier "env"`
                    # （页面 boot=pending，随后 timeout waiting for scene），而同一份代码在本机干净
                    # worktree 里 bundle 阶段是绿的。`--target web` 的 ESM 胶水**不该**出现裸 `"env"`
                    # 说明符（那是 bundler/nodejs 目标的形状），所以先把胶水的身份钉下来：
                    # 大小、sha256、wasm-bindgen 版本、以及胶水里是否真的出现 `"env"`。
                    try {
                        $gluePath = Join-Path (Join-Path $projectRoot "examples/game_api/builds/webgl/pkg") "game_web.js"
                        if (Test-Path -LiteralPath $gluePath) {
                            $glueItem = Get-Item -LiteralPath $gluePath
                            $glueHash = (Get-FileHash -LiteralPath $gluePath -Algorithm SHA256).Hash
                            & $emitProgress ("publish_glue size=" + $glueItem.Length + " sha256=" + $glueHash.Substring(0, 16))
                            $glueText = [IO.File]::ReadAllText($gluePath)
                            $envHits = @([regex]::Matches($glueText, '["'']env["'']')).Count
                            & $emitProgress ("publish_glue_env_specifier_hits=" + $envHits)
                            $moduleHits = @([regex]::Matches($glueText, '(?m)^\s*import\s')).Count
                            & $emitProgress ("publish_glue_import_lines=" + $moduleHits)
                            $firstLine = ([string](($glueText -split "`n")[0])).Trim()
                            & $emitProgress ("publish_glue_first_line=" + $firstLine.Substring(0, [Math]::Min(120, $firstLine.Length)))
                        }
                        else {
                            & $emitProgress "publish_glue_missing: $gluePath"
                        }
                        # 产物目录清单 + index.html 指纹：胶水一样而页面不一样，说明问题在页面/加载方式；
                        # 胶水本身不一样，说明 CI 的发布产出与本地不同源（工具链/目标/版本）。
                        $pkgDir = Join-Path (Join-Path $projectRoot "examples/game_api/builds/webgl") "pkg"
                        if (Test-Path -LiteralPath $pkgDir) {
                            $listing = (Get-ChildItem -LiteralPath $pkgDir -File | Sort-Object Name | ForEach-Object { "{0}={1}" -f $_.Name, $_.Length }) -join ","
                            & $emitProgress ("publish_pkg_listing=" + $listing)
                        }
                        $indexPath = Join-Path (Join-Path $projectRoot "examples/game_api/builds/webgl") "index.html"
                        if (Test-Path -LiteralPath $indexPath) {
                            $idxHash = (Get-FileHash -LiteralPath $indexPath -Algorithm SHA256).Hash
                            & $emitProgress ("publish_index size=" + (Get-Item -LiteralPath $indexPath).Length + " sha256=" + $idxHash.Substring(0, 16))
                            $idxText = [IO.File]::ReadAllText($indexPath)
                            $scriptTags = ([regex]::Matches($idxText, '<script[^>]*src="([^"]+)"') | ForEach-Object { $_.Groups[1].Value }) -join ","
                            & $emitProgress ("publish_index_scripts=" + $scriptTags)
                        }
                        $bindgen = Get-Command wasm-bindgen -ErrorAction SilentlyContinue
                        if ($bindgen) { & $emitProgress ("publish_wasm_bindgen=" + (& $bindgen.Source --version 2>&1 | Select-Object -First 1)) }
                        $toolsBindgen = Join-Path (Join-Path $projectRoot "engine/.tools/wasm-bindgen/bin") "wasm-bindgen.exe"
                        if (Test-Path -LiteralPath $toolsBindgen) {
                            & $emitProgress ("publish_wasm_bindgen_tool=" + (& $toolsBindgen --version 2>&1 | Select-Object -First 1))
                        }
                    }
                    catch {
                        & $emitProgress ("publish_glue_report_failed: " + $_.Exception.Message)
                    }
                }
                if ($publishExit -ne 0) {
                    $tail = "(no log)"
                    $tailRows = @()
                    # package.ps1 的 `throw` 落在 **stderr**：只倒 stdout 会看到"最后一个正常步骤"，
                    # 2026-11-03 就这样连着两轮只看到 "Installing wasm-bindgen.exe" 而看不到原因。
                    $errRows = @()
                    if (Test-Path -LiteralPath "$publishLog.err") {
                        $errRows = @(Get-Content -LiteralPath "$publishLog.err" -Tail 25)
                    }
                    if (Test-Path -LiteralPath $publishLog) {
                        $tailRows = @(Get-Content -LiteralPath $publishLog -Tail 25)
                        $tail = $tailRows -join "`n"
                    }
                    # 失败时把发布日志的尾 25 行**逐行发成 annotation**：CI 的 ::error:: 截断只保留很短一段，
                    # 而真正的原因（例如 binaryen 下载失败的原句）常常正好被截掉。这里一行一条，保证读得到。
                    if ($env:GITHUB_ACTIONS -eq 'true') {
                        foreach ($row in $errRows) {
                            $line = ("{0}" -f $row).TrimEnd()
                            if ($line) { Write-Host "::error::web evidence: publish stderr: $line" }
                        }
                        foreach ($row in $tailRows) {
                            $line = ("{0}" -f $row).TrimEnd()
                            if ($line) { Write-Host "::error::web evidence: publish log: $line" }
                        }
                    }
                    $errTail = $errRows -join "`n"
                    throw "WebGL publish failed (exit $publishExit, ${publishElapsed}m)。stdout 尾部：`n$tail`nstderr 尾部：`n$errTail"
                }
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
    elseif ($evidenceOnly) {
        # 证据模式的绿灯只证明这一段证据，不证明主门/预检；措辞上必须与"门全绿"区分。
        Write-Host "`nEvidence run completed (selected mode only; NOT a full quality-gate pass). Log: $logPath" -ForegroundColor Yellow
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
    # 《江湖有田》整套 headless 回归随工程迁出仓库（现在在 E:\Qoder\game_jhyt）而退役：
    # 81 个 jhyt_* 示例、发布产物门与旧 MVP 清单都已从本仓库删除，样板工程只剩 examples/game_api。
    # 开关保留是为了不破坏既有调用方；这一段现在只跑 game_api 的干净目录发布自测。
    Write-Step "Clean release directory self test (game_api)"
    Invoke-PackageSelfTest -ProjectRootPath $defaultGameProject

    Write-Host "`nMVP gate passed (jhyt regression retired). Log: $logPath" -ForegroundColor Green
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
        $pointLightGoldenPath = Join-Path $baselineRoot "wp0-point-light-golden.json"
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
        # 多点光源：两盏异色点光 + range 截断，PBR 与默认着色路径各一次。方向光强度为 0，
        # 所以读回的颜色只可能来自点光——打包偏移、count 字段、数组槽位任何一处错位都会红。
        Invoke-Checked cargo @(
            "run", "--quiet", "--release", "-p", "engine", "--example", "wp0_point_light_golden", "--",
            "--output", $pointLightGoldenPath
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
    # wp0-v4：场景/帧报告契约。场景数**必须等于样板工程 scenes/ 里的场景文件数**
    # （此前写死一个下界，示例工程重建后它既挡不住"扫漏了场景"，也会在场景增减时误报）；
    # 帧阶梯必须三档且每档都有非零时间、非零分配——分配为 0 只可能意味着计数没生效，
    # 而不是"优化到位"。
    $sceneFrameReport = Get-Content -LiteralPath $sceneFramePath -Raw | ConvertFrom-Json
    $sceneFileCount = @(
        Get-ChildItem -Path (Join-Path $gameRoot "game_api/scenes") `
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
        # 空门面场景（UI 全走 prefab 实例化）是合法的：所以"每条 entry 都要有 typed 组件"
        # 在它上面必然为假。判据改成从场景文本自己推：
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
    # 判据用**配对基线**：动画阶梯减去"同实体数、同布局、只少一个 `Animation` 组件"的
    # 控制组阶梯。理由是本条 2026-11-02 的实测——5,000 实体时动画档 11,069 次/帧、控制组
    # 也 11,069 次/帧，**差值为 0**：整帧分配里那 2.2 次/实体是实体数的固定成本（脚本侧
    # 每实体一份登记 + 提取侧批次项），静态精灵阶梯一样付（1,000 精灵 = 2,341 次/帧，其中
    # 脚本步 1,225 次），跟动画路径无关。此前拿整帧绝对数去比 200，等于把这笔固定成本记在
    # 动画头上，判据从落地那天起就永远不可能通过（门红了两个月没人发现，正是因为它红得
    # 没有信息量）。现在判的是 docs/54 §4.4 真正主张的那件事：**动画路径自己多花多少**。
    if ($sceneFrameReport.animation_control_stairs.Count -ne 2) {
        throw "wp0-v4 animation control stairs missing: expected two entity counts (paired baseline)"
    }
    $animationAlloc = @($sceneFrameReport.animation_stairs | ForEach-Object { [int]$_.alloc_count_p50 })
    $animationControlAlloc = @($sceneFrameReport.animation_control_stairs | ForEach-Object { [int]$_.alloc_count_p50 })
    $animationEntities = @($sceneFrameReport.animation_stairs | ForEach-Object { [int]$_.entities })
    $controlEntities = @($sceneFrameReport.animation_control_stairs | ForEach-Object { [int]$_.entities })
    if ($animationEntities[0] -ge $animationEntities[1]) {
        throw "animation stairs must increase in entity count: $($animationEntities -join ',')"
    }
    if (($controlEntities -join ',') -ne ($animationEntities -join ',')) {
        throw "animation control stairs must pair with animation stairs: control=$($controlEntities -join ',') animation=$($animationEntities -join ',')"
    }
    if (($animationAlloc | Measure-Object -Minimum).Minimum -le 0) {
        throw "animation measurement is dead: alloc_count_p50 must be positive"
    }
    $animationExtra = @(0..1 | ForEach-Object {
        [Math]::Max(0, $animationAlloc[$_] - $animationControlAlloc[$_])
    })
    $maxAnimationExtra = ($animationExtra | Measure-Object -Maximum).Maximum
    if ($maxAnimationExtra -gt 200) {
        throw ("animation path allocations too high: extra={0} per frame " +
            "(animated={1} control={2})") -f $maxAnimationExtra, ($animationAlloc -join ','), ($animationControlAlloc -join ',')
    }
    if (($animationExtra[1] - $animationExtra[0]) -gt 20) {
        throw ("animation path allocations scale with entities: extra {0}->{1} vs {2}->{3}" -f `
            $animationEntities[0], $animationExtra[0], $animationEntities[1], $animationExtra[1])
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
    # 判据同样用**相位内配对**：静态相位当作基线（顶层字段整棵不动 ⇒ 传播应零分配，批 R 的
    # "没变就跳过"）；两个运动相位减去基线，剩下的才是传播路径自己的账。
    #
    # 2026-11-02 实测（5,000 实体）：静态 22,005 / 单叶 22,021 / 全动 22,021——三个相位都被
    # 同一笔"每实体固定成本"（约 2.2 次/帧，脚本侧登记 + 提取侧批次项，静态精灵阶梯同样付）
    # 抬到两万二；**相位差只有 12~16 次，与实体数无关**，也就是说本条要判的不变量（传播分配
    # 不随实体数增长）其实是成立的。旧判据拿绝对数比 400，量的却是实体数成本，于是从批 S 落地
    # 之后一路红到今天。现在判差值，绝对值仍写进报告供人看。
    $hierarchyAlloc = @($sceneFrameReport.hierarchy_stairs | ForEach-Object { [int]$_.alloc_count_p50 })
    $hierarchyEntities = @($sceneFrameReport.hierarchy_stairs | ForEach-Object { [int]$_.entities })
    $hierarchyPhaseAlloc = @($sceneFrameReport.hierarchy_stairs | ForEach-Object {
            $_.alloc_count_p50
            $_.moving_one.alloc_count_p50
            $_.moving.alloc_count_p50
        })
    if (($hierarchyPhaseAlloc | Measure-Object -Minimum).Minimum -le 0) {
        throw ("hierarchy measurement is dead: alloc_count_p50 must be positive ({0})" -f ($hierarchyPhaseAlloc -join ','))
    }
    $hierarchyExtra = @($sceneFrameReport.hierarchy_stairs | ForEach-Object {
            [int]$_.moving_one.alloc_count_p50 - [int]$_.alloc_count_p50
            [int]$_.moving.alloc_count_p50 - [int]$_.alloc_count_p50
        })
    if (($hierarchyExtra | Measure-Object -Maximum).Maximum -gt 400) {
        throw ("hierarchy propagation allocations too high: extra={0} (three phases={1})" -f `
            ($hierarchyExtra -join ','), ($hierarchyPhaseAlloc -join ','))
    }
    $hierarchyMovingOneExtra = @($sceneFrameReport.hierarchy_stairs | ForEach-Object {
            ([int]$_.moving_one.alloc_count_p50 - [int]$_.alloc_count_p50) })
    if (($hierarchyMovingOneExtra[1] - $hierarchyMovingOneExtra[0]) -gt 100) {
        throw ("hierarchy propagation allocations scale with entities: extra {0}->{1} vs {2}->{3}" -f `
            $hierarchyEntities[0], $hierarchyMovingOneExtra[0], $hierarchyEntities[1], $hierarchyMovingOneExtra[1])
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
    # 多点光源：两盏异色点光分别照亮各自正下方的地面，range 截断后同一点必须变暗；
    # 默认（非 PBR）着色路径也必须同样上色。四条都断在像素上。
    $pointLight = Get-Content -LiteralPath $pointLightGoldenPath -Raw | ConvertFrom-Json
    if ($pointLight.report_version -ne "wp0-point-light-golden-v1") {
        throw "point light golden report version invalid: $($pointLight.report_version)"
    }
    if ($pointLight.luminance.red_lit -le ($pointLight.luminance.red_dark + 15)) {
        throw "point light golden: 红灯正下方没有变亮（count/偏移/衰减）：$($pointLight.luminance.red_lit) vs $($pointLight.luminance.red_dark)"
    }
    if ($pointLight.luminance.green_lit -le ($pointLight.luminance.green_dark + 15)) {
        throw "point light golden: 绿灯正下方没有变亮（数组第 1 槽没读到）：$($pointLight.luminance.green_lit) vs $($pointLight.luminance.green_dark)"
    }
    if ($pointLight.samples.red_lit[0] -le ($pointLight.samples.red_lit[1] + 20)) {
        throw "point light golden: 红灯位置没有偏红：[$($pointLight.samples.red_lit -join ',')]"
    }
    if ($pointLight.samples.green_lit[1] -le ($pointLight.samples.green_lit[0] + 20)) {
        throw "point light golden: 绿灯位置没有偏绿：[$($pointLight.samples.green_lit -join ',')]"
    }
    if ($pointLight.samples.legacy_red_lit[0] -le ($pointLight.samples.legacy_red_lit[1] + 20)) {
        throw "point light golden: 默认着色路径（mesh3d.wgsl）没吃到点光：[$($pointLight.samples.legacy_red_lit -join ',')]"
    }
    Write-Host ("point_light_golden_ok red={0} green={1} dark={2} legacy_red={3}" -f `
        $pointLight.luminance.red_lit, $pointLight.luminance.green_lit,
        $pointLight.luminance.red_dark, $pointLight.samples.legacy_red_lit) -ForegroundColor Green
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

