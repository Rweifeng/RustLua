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
    [switch]$Benchmarks
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

    if (-not $SkipClippy) {
        Write-Step "Clippy"
        Invoke-Checked cargo @("clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings")
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

    Write-Step "Editor project check"
    Push-Location $projectRoot
    try {
        Invoke-Checked cargo @("run", "--quiet", "--manifest-path", "engine/Cargo.toml", "-p", "editor", "--", "--project", $defaultGameProject, "--check")
    }
    finally {
        Pop-Location
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
        "jhyt_m31_pc_release"
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
        )        # WP3 存储 A/B（wp0-v5）：现状 HashMap+排序 Vec 对 dense/sparse。用来把"是否替换
        # ECS 热存储"从意见变成证据。
        Invoke-Checked cargo @(
            "run", "--quiet", "--release", "-p", "engine", "--example", "wp0_ecs_storage", "--",
            "--cycles", "30", "--output", $ecsStoragePath
        )
    }
    finally {
        Pop-Location
    }
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
    # wp0-v4：场景/帧报告契约。场景数下界防止"扫不到场景却报绿"；帧阶梯必须三档且
    # 每档都有非零时间、非零分配——分配为 0 只可能意味着计数没生效，而不是"优化到位"。
    $sceneFrameReport = Get-Content -LiteralPath $sceneFramePath -Raw | ConvertFrom-Json
    if ($sceneFrameReport.report_version -ne "wp0-v4" -or
        $sceneFrameReport.benchmark -ne "scene_frame" -or
        $sceneFrameReport.scene_load.scenes -lt 26 -or
        $sceneFrameReport.frame_stairs.Count -ne 3) {
        throw "wp0-v4 scene/frame report schema invalid"
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

if (-not $KeepLog) {
    Write-Host "Quality log retained for review and regression." -ForegroundColor DarkGray
}

