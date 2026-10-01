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
    # WP0：运行固定 ECS/2D/3D 阶梯基准并写入本地 artifacts/baselines。
    [switch]$Benchmarks,
    # docs/54 §8-⑨：跑 Web 平台证据（浏览器契约层 + 已发布产物的启动层）。
    # 环境受限（无浏览器/无 node/无 wasm32 目标）时**显式**跳过并打印原因，不伪造通过。
    [switch]$Web,
    # wasm Lua 运行期验证门（scripts/verify-wasm-lua.ps1）：spike（vendored mlua 移植）
    # + no-modules 桥（game_web Lua 桥）的 Node 验证。退出码 0=通过/1=判红/3=环境受限跳过，
    # 与 -Web 同一哲学：缺 node/wasm32/zig/wasm-bindgen 时显式跳过，绝不把"跳过"当"通过"。
    [switch]$WasmLua
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent $PSScriptRoot
$engineRoot = Join-Path $projectRoot "engine"
$gameRoot = Join-Path $projectRoot "examples"
# 工程校验与干净目录自测的默认目标工程：examples 重组后样板工程在子目录里
# （`examples/game_jhyt`），根目录已没有 project.toml——此前写死 `examples` 会让
# 「Game project check」必然失败。
$defaultGameProject = Join-Path $gameRoot "game_jhyt"
$logRoot = Join-Path $projectRoot "artifacts\logs\quality"
$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$logPath = Join-Path $logRoot "verify-$timestamp.log"

New-Item -ItemType Directory -Force -Path $logRoot | Out-Null

function Write-Step([string]$Message) {
    Write-Host "`n==> $Message" -ForegroundColor Cyan
}

function Invoke-Checked([string]$File, [string[]]$Arguments) {
    $command = "$File $($Arguments -join ' ')"
    Write-Host "> $command" -ForegroundColor DarkGray

    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $output = & $File @Arguments 2>&1
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = $previousPreference

    $output | ForEach-Object {
        $line = $_.ToString()
        Write-Host $line
        Add-Content -Path $logPath -Value $line -Encoding UTF8
    }

    if ($exitCode -ne 0) {
        throw "Command failed (exit code $exitCode): $command"
    }
}

# package.ps1 必须用**具名参数**调用：PowerShell 的数组 splat（`@array`）是按位置传参，
# 数组里的 `-SelfTest` 会被当作 `-Platform` 的值而触发 ValidateSet 失败
# （`Cannot validate argument on parameter 'Platform'`）。
function Invoke-PackageSelfTest([string]$ProjectRootPath) {
    $packageScript = Join-Path $PSScriptRoot "package.ps1"
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

$runGates = -not $MvpOnly

# 文档一致性门（P4）：编号唯一、H1、索引可达、相对链接、历史路径、UTF-8 可解码。
# 放在最前面：它只读文本、约 1 秒，且 `-MvpOnly` 也跑（文档失真不该被快速路径跳过）。
Write-Step "Documentation consistency"
$docsScript = Join-Path $projectRoot "scripts/check-docs.ps1"
if (Test-Path $docsScript) {
    $docsOutput = & powershell -NoProfile -ExecutionPolicy Bypass -File $docsScript
    $docsExit = $LASTEXITCODE
    $docsOutput | ForEach-Object { Write-Host $_ }
    if ($docsExit -ne 0) {
        throw "check-docs.ps1 failed (exit code $docsExit)"
    }
}

if ($runGates) {
Push-Location $engineRoot
try {
    Write-Step "Cargo metadata"
    Invoke-Checked cargo @("metadata", "--no-deps", "--format-version", "1")

    Write-Step "Rust formatting"
    Invoke-Checked cargo @("fmt", "--all", "--", "--check")

    # 优化项 10：Lua API 文档桩的再生成一致性。事实源是**运行期 API 面基线**
    # （`core::api_surface_snapshot/lua_api_surface.json`），所以这一步查的是
    # 「文档桩与真正暴露给脚本的那 275 个函数是否还是同一份清单」，外加「待补桩数没有变多」。
    # 缺 node 时打印原因后跳过——沿用本仓库口径，跳过绝不写成通过。
    Write-Step "Lua API doc stubs"
    if (Get-Command node -ErrorAction SilentlyContinue) {
        Invoke-Checked node @((Join-Path $projectRoot "scripts/gen-lua-api-docs.js"), "--check")
    } else {
        Write-Host "  [SKIP] 未找到 node：Lua API 文档桩门未运行（不是通过）" -ForegroundColor Yellow
    }

    if (-not $SkipClippy) {
        Write-Step "Clippy"
        Invoke-Checked cargo @("clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings")
    }

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

    # 优化项 11：本地 pre-commit 门自身的回归。fmt/clippy 步查的是代码，这一步查的是
    # "提交前到底会不会有人被挡住"——hook 的选路、拦截与逃生门逻辑回归了，前面所有步都发现不了。
    # 它只用临时 git 仓库 + cargo fmt（零依赖、秒级）。退出码沿用本仓库口径：
    # 0=通过；3=环境受限（缺 git/cargo/sh）打印原因后跳过，绝不把"跳过"说成"通过"；其余=判红。
    Write-Step "Local pre-commit hook regression"
    $hookGateScript = Join-Path $projectRoot "scripts/test-git-hooks.ps1"
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

    if (-not $SkipTests) {
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
        Invoke-Checked powershell @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", (Join-Path $projectRoot "scripts/test-compare-baseline.ps1"))
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
        $editorCheckOutput = & cargo run --quiet --manifest-path "engine/Cargo.toml" -p editor -- --project $defaultGameProject --check 2>&1
        $editorCheckExit = $LASTEXITCODE
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
            $secondOutput = & cargo run --quiet --manifest-path "engine/Cargo.toml" -p editor -- --project $secondProject --check 2>&1
            $secondExit = $LASTEXITCODE
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

    # docs/54 §8-⑧：空间迁移命令必须**每次**都给出可归档的报告 artifact，并且示例工程里
    # 一个实体都不该被拒（refused=0）、一处旧设计都不该命中（removed=0）。把报告落盘后逐条断言，
    # 因此"迁移干净"是每次 verify 重新证明的事实，而不是一次性的手工检查。
    Write-Step "Spatial migration report"
    Push-Location $projectRoot
    try {
        $migrationReportPath = Join-Path $projectRoot "artifacts\baselines\spatial-migration-report.txt"
        $migrationOutput = & cargo run --quiet --manifest-path "engine/Cargo.toml" -p editor -- --project $defaultGameProject --migrate-spatial --migrate-report $migrationReportPath 2>&1
        $migrationExit = $LASTEXITCODE
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
        $editorBaselinePath = Join-Path $projectRoot "artifacts\baselines\editor-baseline.txt"
        $editorBaselineOutput = & cargo run --quiet --manifest-path "engine/Cargo.toml" -p editor -- --perf-baseline --perf-report $editorBaselinePath 2>&1
        $editorBaselineExit = $LASTEXITCODE
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
        $guiSmokePath = Join-Path $projectRoot "artifacts\baselines\editor-gui-smoke.txt"
        $guiSmokeOutput = & cargo run --quiet --manifest-path "engine/Cargo.toml" -p editor -- --gui-smoke --gui-report $guiSmokePath 2>&1
        $guiSmokeExit = $LASTEXITCODE
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
            param([string]$Script, [string]$Label)
            $logFile = Join-Path $webEvidenceRoot ("gate-" + $Label + "-$timestamp.log")
            $statusFile = "$logFile.status"
            # **有界等待**，不用 `-Wait`：`-Wait` 会等到整个进程树结束，而无头浏览器偶尔不自行退出，
            # 于是质量门会一直挂在那里（本轮实际踩到）。这里自己轮询 + 超时即杀，并把它当失败
            # ——宁可判红，也不要"卡住"。
            $timeoutSeconds = 600
            $startedAt = Get-Date
            $process = Start-Process -FilePath "powershell" `
                -ArgumentList @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $Script, "-StatusFile", $statusFile) `
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
            $webScript = Join-Path $projectRoot "scripts/verify-webgl.ps1"
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

            $bundleScript = Join-Path $projectRoot "scripts/verify-webgl-bundle.ps1"
            # 产物层的证据要求"真的有产物"：本机**一个 webgl 产物都没有**时就地发布一份
            # （wasm 构建 + wasm-bindgen 胶水）。否则 CI 上这一步永远是 skip，"Web 有可重复质量
            # 证据"就只剩契约层。已存在产物（哪怕有多个候选）时不动它——"拿 A 的包验证 B 并报出
            # 通过"比跳过更糟（与 bundle-discovery 的既有口径一致）。
            . (Join-Path $projectRoot "scripts/bundle-discovery.ps1")
            $existingBundle = Resolve-GameBundle -Platform "webgl" -WorkspaceRoot $projectRoot
            $webglBuilds = @(
                Get-ChildItem -Path (Join-Path $projectRoot "examples") -Recurse -Directory -ErrorAction SilentlyContinue |
                    Where-Object { $_.Parent -and $_.Parent.Name -eq "webgl" }
            )
            if (-not $existingBundle -and $webglBuilds.Count -eq 0) {
                Write-Step "Publish WebGL bundle (no published artifact found)"
                Invoke-Checked powershell @(
                    "-NoProfile", "-ExecutionPolicy", "Bypass",
                    "-File", (Join-Path $projectRoot "scripts\package.ps1"),
                    "-ProjectRoot", $defaultGameProject, "-Platform", "webgl"
                )
            }
            $bundleRun = & $runWebEvidence $bundleScript "bundle"
            $bundleExit = $bundleRun.ExitCode
            if ($bundleExit -eq 1) {
                throw "published webgl bundle FAILED verification (verify-webgl-bundle.ps1 exit 1; boot / scene-verify / blank check - see bundle-summary-*.json)"
            }
            if ($bundleExit -ne 0) {
                Write-Host "webgl_bundle_skipped reason=environment_limited_or_no_bundle exit=$bundleExit (先跑 scripts\package.ps1 -Platform webgl)" -ForegroundColor Yellow
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
                if ($null -eq $bundlePayload.drawnSamples -or $bundlePayload.drawnSamples -le 0) {
                    throw "webgl bundle reports no painted canvas samples (blank frame): drawn=$($bundlePayload.drawnSamples)"
                }
                # 场景切换防回归门：verify-webgl-bundle 默认在 URL 里带 p2-scene-verify=1，
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

            # ③ 新 EH（try_table/exnref）跨浏览器层：本产物是纯新 EH，低于门槛的引擎连编译都
            # 过不去（iOS Safari 18.4 / Chrome·Edge 137 / Firefox 131）。这一层只**编译**真实
            # wasm（不跑游戏），因此与启动层正交：启动层可能因运行期 bug 判红，但只要模块能编译，
            # EH 覆盖就是绿的。runner 逐个验本机所有浏览器；无浏览器/无产物时以 3 显式跳过。
            # Safari 无法在 Windows 自动化——runner 会打印 Mac 上的手动验证流程。
            $ehScript = Join-Path $projectRoot "scripts/verify-webgl-eh.ps1"
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

    Write-Host "`nQuality gate passed. Log: $logPath" -ForegroundColor Green
}
finally {
    Pop-Location
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
        "jhyt_m25_combat_feedback",
        "jhyt_m26_hud_feedback",
        "jhyt_m27_goals",
        "jhyt_m28_loop",
        "jhyt_m29_season_weather",
        "jhyt_m30_cjk_text",
        "jhyt_m31_pc_release",
        # M32 相机收口：相机跟随玩家 + 取景由场景 bounds 锁在地图内（含换图后重绑 bounds）。
        "jhyt_m32_camera"
    )
    $exampleDir = Join-Path $engineRoot "crates\game\examples"

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
    $unlisted = @($onDisk | Where-Object { $examples -notcontains $_ })
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

    Write-Step "Clean release directory self test"
    Invoke-PackageSelfTest -ProjectRootPath $defaultGameProject

    Write-Host "`nMVP gate passed. Log: $logPath" -ForegroundColor Green
}

if ($Benchmarks) {
    Write-Step "WP0 performance baselines"
    Push-Location $engineRoot
    try {
        $baselineRoot = Join-Path $projectRoot "artifacts\baselines"
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
        $lightingGoldenPath = Join-Path $baselineRoot "wp0-lighting-golden.json"
        $editorPreviewPath = Join-Path $baselineRoot "wp0-editor-preview-golden.json"
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
    }
    finally {
        Pop-Location
    }
    # docs/54 §8-⑩：把"这一次的 3D 报告"和**提交进版本库**的结构性基线比一次——几何量
    # （draw/instances/triangles）与分配峰值跨机可比，因此可以进门判红；时间（p95/p99）是机器
    # 相关的量，这一档只打印不判定（同机 A/B 用完整模式，见 artifacts/baselines/README.md）。
    Write-Step "3D baseline comparison (geometry + allocation)"
    $committedBaseline = Join-Path $projectRoot "artifacts\baselines\wp0-3d-stress.committed.json"
    if (-not (Test-Path -LiteralPath $committedBaseline)) {
        throw "committed 3D baseline is missing: $committedBaseline（见 artifacts/baselines/README.md）"
    }
    Invoke-Checked powershell @(
        "-NoProfile", "-ExecutionPolicy", "Bypass",
        "-File", (Join-Path $projectRoot "scripts\compare-baseline.ps1"),
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
        if ($entry.diagnostics -ne 0 -or $entry.typed_components -le 0 -or
            $entry.load_us.p50_us -le 0 -or $entry.typed_compile_us.p50_us -le 0) {
            throw "wp0-v4 typed compile entry invalid: $($entry.project)/$($entry.scene)"
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
    Write-Host "WP0 benchmark baselines and semantic golden written and schema-checked: $baselineRoot" -ForegroundColor Green
}

# wasm Lua 运行期验证门：把 scripts/wasm-lua 下的两个 Node 用例（spike + no-modules 桥）
# 纳入可重复门。与 -Web 同一子进程约定——Start-Process -PassThru 在重定向下 ExitCode 是
# $null，退出码**只**认子进程写的 StatusFile；缺失即判失败。0=通过 / 1=判红 / 3=环境受限跳过。
if ($WasmLua) {
    Write-Step "wasm Lua runtime evidence"
    $wasmEvidenceRoot = Join-Path $projectRoot "artifacts\logs\wasm-lua"
    New-Item -ItemType Directory -Force -Path $wasmEvidenceRoot | Out-Null
    $wasmScript = Join-Path $projectRoot "scripts\verify-wasm-lua.ps1"
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
        Write-Host "wasm_lua_ok (spike + no-modules bridge passed)" -ForegroundColor Green
    }
    else {
        Write-Host "wasm_lua_skipped reason=environment_limited exit=$exitCode (缺 node/wasm32 target/zig/wasm-bindgen)" -ForegroundColor Yellow
    }
}

if (-not $KeepLog) {
    Write-Host "Quality log retained for review and regression." -ForegroundColor DarkGray
}

