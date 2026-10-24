# 静默失败的"留痕"回归门。
#
# 为什么需要它（本轮反面对照得出的结论）：把已修的静默点**合并回旧写法**，全量测试照样全绿——
# 也就是说**测试门守不住"只缺留痕"这类缺陷**。所以这里用最小代价把"我们已经决定要留痕的点"钉住：
# 谁把 warn!/记账删了，这道门立刻红。
#
# 只钉**已决定要留痕**的点；定性为"合法安静"的那些（相机退化、无实例化、并发去重等）**故意不断言**。

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$script:fail = 0
$script:checks = 0

function Assert-Contains([string]$rel, [string]$needle, [int]$min = 1) {
    $path = Join-Path $root $rel
    $script:checks++
    if (-not (Test-Path -LiteralPath $path)) {
        Write-Host "FAIL 缺文件: $rel" -ForegroundColor Red
        $script:fail++
        return
    }
    $text = [System.IO.File]::ReadAllText($path)
    $count = ([regex]::Matches($text, [regex]::Escape($needle))).Count
    if ($count -lt $min) {
        Write-Host ("FAIL {0}: 找不到 `"{1}`"（出现 {2} 次，要求 >= {3}）——留痕被删了？" -f $rel, $needle, $count, $min) -ForegroundColor Red
        $script:fail++
    }
}

# 1) draw 级丢弃记账：两条 2D 通道 + 世界精灵（渲染器）
Assert-Contains 'engine/crates/rustlua-gpu/src/renderer/passes.rs' 'record_dropped_draw(&mut metrics);' 2
Assert-Contains 'engine/crates/rustlua-gpu/src/renderer/world_sprite.rs' 'record_dropped_draw(metrics);' 1
Assert-Contains 'engine/crates/rustlua-gpu/src/renderer/mod.rs' 'pub dropped_draws: u64,' 1
# 2) 三条静默链的根因告警：字体图集上传失败 / 矢量文字字形缺失 / 自定义字体快照锁中毒
Assert-Contains 'engine/crates/rustlua-gpu/src/renderer/passes.rs' 'font atlas upload failed' 1
Assert-Contains 'engine/crates/engine/src/host/runtime_scene/text.rs' 'glyph missing from atlas' 1
Assert-Contains 'engine/crates/engine/src/host/runtime_scene/frame_data.rs' 'custom font atlas lock poisoned' 1
# 3) GPU 用例的"跳过"必须留痕（默认静默跳过 = 假绿）
Assert-Contains 'engine/crates/rustlua-gpu/src/renderer/tests/mod.rs' 'fn gpu_skipped(' 1
Assert-Contains 'engine/crates/rustlua-gpu/src/renderer/tests/mod.rs' 'RUSTLUA_REQUIRE_GPU' 1

if ($script:fail -gt 0) {
    Write-Host "silent_traces FAILED: $($script:fail) 项 / $($script:checks) 条断言" -ForegroundColor Red
    exit 1
}
Write-Host "silent_traces_ok assertions=$($script:checks)"
# 必须显式 exit 0：不写的话绿的时候脚本不设 $LASTEXITCODE，调用方会读到**上一次**的残留值
# （`verify.ps1` 的 Invoke-Checked 正是按退出码判定的）。
exit 0