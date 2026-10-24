# 降耦合门：把 `docs/62` P1 ③④⑤ 与 M77-00 ⑥⑦ 的验收口径变成可执行检查（不需要人肉跑 cargo tree 再数行）。
#
# 五条硬约束（各有出处）：
#   ③ wasm 图里不得出现 rodio / cpal / symphonia —— 音频后端只能在桌面层，web 包一碰就炸体积。
#   ④ `rustlua-scene` 的 lib 图里不得出现 `rustlua-render` / `rustlua-ui` —— 组件编解码走注册表。
#   ⑤ 契约层 `rustlua-lua-api` 不得依赖任何 `rustlua-*` —— 它一旦依赖领域 crate，倒置就白做。
#   ⑥ 组合根 `engine` 的 normal 图里不得出现 wgpu / naga（M77-01：渲染器住在 rustlua-gpu）。
#   ⑦ `gpu-web-probe` 的 normal 图里不得出现 `engine`（M77-01：拿渲染器不必拖上组合根）。
#
# 用法：powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify/check-decoupling.ps1
# 退出码 0 = 全绿；1 = 有违规（会打印违规条目与取证命令）。

$ErrorActionPreference = 'Stop'

$engineDir = Join-Path $PSScriptRoot '..\..\engine'
if (-not (Test-Path -LiteralPath $engineDir)) {
    Write-Host "check_decoupling_fail reason=engine_missing path=$engineDir"
    exit 1
}

function Invoke-CargoTree {
    param([string[]] $Arguments)
    # PowerShell 5.1 在 EAP=Stop 下会把原生程序的 stderr 当成终止性错误：cargo 的
    # "Updating crates.io index" 一走 stderr，命令还没结束就被 PowerShell 打断，现场只剩
    # 这一行（2026-10-21 CI web-evidence 实测）。这里临时降级 EAP，并把 stderr 单独落文件，
    # 只把 stdout 的依赖图交给匹配逻辑——否则 "Downloaded rodio" 这类进度行会被误判成
    # wasm 图里真的出现了音频后端（同日第二轮 CI 实测）。
    $savedEap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    $errFile = [IO.Path]::GetTempFileName()
    try {
        $output = & cargo tree --locked @Arguments 2> $errFile
        $code = $LASTEXITCODE
        $errText = if (Test-Path -LiteralPath $errFile) { Get-Content -LiteralPath $errFile -Raw } else { '' }
    }
    finally {
        $ErrorActionPreference = $savedEap
        Remove-Item -LiteralPath $errFile -Force -ErrorAction SilentlyContinue
    }
    if ($code -ne 0) {
        $detail = (($errText -split "`r?`n") | Select-Object -First 8) -join ' | '
        throw "cargo tree --locked $($Arguments -join ' ') 失败（exit $code）：$detail"
    }
    return $output
}

$violations = New-Object System.Collections.Generic.List[string]
Push-Location $engineDir
try {
    # ③ wasm 图：音频后端必须归零。
    $wasm = Invoke-CargoTree @('-p', 'game_web', '--target', 'wasm32-unknown-unknown', '-e', 'normal')
    $audio = @($wasm | Select-String -Pattern 'rodio|cpal|symphonia')
    foreach ($hit in $audio) { $violations.Add("[③] wasm 图出现音频后端：$($hit.Line.Trim())") }

    # ④ 运行层不得看见渲染层与 UI 层。
    $scene = Invoke-CargoTree @('-p', 'rustlua-scene', '-e', 'normal')
    $layer = @($scene | Select-String -Pattern 'rustlua-render|rustlua-ui')
    foreach ($hit in $layer) { $violations.Add("[④] rustlua-scene 图出现渲染/UI 层：$($hit.Line.Trim())") }

    # ⑤ 契约层是叶子：只允许 mlua / serde_json 等非本仓 crate。
    $contract = Invoke-CargoTree @('-p', 'rustlua-lua-api', '-e', 'normal')
    # 首行是 crate 自己（`rustlua-lua-api v0.1.0 (path)`），从第二行起才是依赖。
    $peers = @($contract | Select-Object -Skip 1 | Select-String -Pattern 'rustlua-[a-z-]+ v')
    foreach ($hit in $peers) { $violations.Add("[⑤] rustlua-lua-api 依赖了本仓 crate：$($hit.Line.Trim())") }

    # ⑥ M77-01：组合根 `engine` 的 normal 图里不许出现 GPU 栈。
    # 这是 `crates/rustlua-infra/tests/architecture_edges.rs` 那条门的**另一半**：那条扫的是
    # 工作区清单的传递闭包（抓"把 rustlua-gpu 加回 engine 的 [dependencies]"这种形状），
    # 这一条用 cargo 自己算的图，因此连"某个核心 crate 经外部 crate 间接拉进 wgpu"也看得见。
    # `-e normal` 是口径的一部分：engine 的 `wp0_*_golden.rs` 基准例子还带着 wgpu/pollster 作为
    # **dev** 依赖（等 M77-02 的"两套 offscreen renderer 合并"把设备管子收进 rustlua-gpu），
    # dev 依赖不进产物依赖图，也不影响任何下游 crate 的增量编译半径——而那两件事正是这条门保护的。
    $engineTree = Invoke-CargoTree @('-p', 'engine', '-e', 'normal')
    $gpu = @($engineTree | Select-String -Pattern 'wgpu|naga')
    foreach ($hit in $gpu) { $violations.Add("[⑥] engine 的 normal 依赖图出现 GPU 栈：$($hit.Line.Trim())") }

    # ⑦ M77-01：`gpu-web-probe` 只要渲染器，不该为此拖上整个组合根。
    # 它此前依赖 `engine`（为了驱动真渲染器），现在直连 `rustlua-gpu` + `rustlua-render`。
    $probe = Invoke-CargoTree @('-p', 'gpu-web-probe', '-e', 'normal')
    $roots = @($probe | Select-String -Pattern '\bengine v')
    foreach ($hit in $roots) { $violations.Add("[⑦] gpu-web-probe 的 normal 依赖图出现组合根 engine：$($hit.Line.Trim())") }
}
catch {
    Pop-Location
    Write-Host "check_decoupling_fail reason=cargo_tree_error detail=$($_.Exception.Message)"
    exit 1
}
finally {
    if ((Get-Location).Path -eq $engineDir) { Pop-Location }
}

if ($violations.Count -gt 0) {
    foreach ($v in $violations) { Write-Host $v }
    Write-Host "check_decoupling_fail violations=$($violations.Count)"
    exit 1
}

Write-Host "check_decoupling_ok wasm_audio=0 scene_layer=0 lua_api_peers=0 engine_gpu_normal=0 probe_engine_normal=0"
exit 0
