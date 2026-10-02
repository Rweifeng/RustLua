# check-vendor-patches.ps1 —— vendor 补丁基线自动校验门
#
# 守护对象：`engine/Cargo.toml` 的 `[patch.crates-io]` 指向的 5 个 vendored fork
# （mlua / mlua-sys / lua-src / wasm-bindgen-macro(+support) / wasm-bindgen-cli）。
# 这些 fork 携带 wasm32-unknown-unknown 上 Lua 与 wasm-bindgen 的必需补丁，**升级上游
# 后补丁极易被覆盖丢失**（实测教训：extern "C-unwind" 回归会让宿主函数返回 Err 的
# pcall 在 wasm 上崩溃，而 boot 依旧全绿）。本门逐条断言每个补丁点仍在，不依赖人肉 diff。
#
# 每个断言与 `engine/vendor/README.md` 的「逐文件补丁清单」一一对应；改补丁必须两边同步。
#
# 用法：
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\check-vendor-patches.ps1
#
# 退出码：0 = 全部补丁点在位；1 = 至少一处丢失/漂移（打印 [FAIL] 明细）。
# 已接入 `scripts/verify.ps1` 总门（与文档一致性门同段，-MvpOnly 也跑：补丁丢失不该被快速路径跳过）。

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent $PSScriptRoot
$vendorRoot = Join-Path $projectRoot "engine\vendor"
$engineToml = Join-Path $projectRoot "engine\Cargo.toml"
$lockFile = Join-Path $projectRoot "engine\Cargo.lock"

$failed = @()
$passed = 0

function Add-Check([string]$Name, [bool]$Ok, [string]$FailReason) {
    if ($Ok) {
        $script:passed++
        Write-Host ("[PASS] " + $Name)
    } else {
        $script:failed += ("[FAIL] " + $Name + " -- " + $FailReason)
        Write-Host ("[FAIL] " + $Name + " -- " + $FailReason) -ForegroundColor Red
    }
}

function Test-Contains([string]$Path, [string[]]$Needles, [string]$What) {
    if (-not (Test-Path -LiteralPath $Path)) {
        Add-Check "${What}: 文件存在" $false "missing $Path"
        return
    }
    $raw = Get-Content -LiteralPath $Path -Raw -ErrorAction SilentlyContinue
    if ($null -eq $raw) {
        Add-Check "${What}: 可读" $false "cannot read $Path"
        return
    }
    $missing = @($Needles | Where-Object { $raw.IndexOf($_, [System.StringComparison]::OrdinalIgnoreCase) -lt 0 })
    if ($missing.Count -eq 0) {
        Add-Check "${What}: 含 $($Needles -join ' / ')" $true
    } else {
        Add-Check "${What}: 含 $($Needles -join ' / ')" $false "missing needle(s): $($missing -join '; ')"
    }
}

if (-not (Test-Path -LiteralPath $vendorRoot)) {
    Write-Host ("[FAIL] vendor root missing: " + $vendorRoot) -ForegroundColor Red
    exit 1
}
if (-not (Test-Path -LiteralPath $engineToml)) {
    Write-Host ("[FAIL] engine/Cargo.toml missing: " + $engineToml) -ForegroundColor Red
    exit 1
}

# ── A. [patch.crates-io] 指向（engine/Cargo.toml）──────────────────────────────
$engineTomlRaw = Get-Content -LiteralPath $engineToml -Raw
$patchChecks = @{
    "mlua"                     = "vendor/mlua";
    "mlua-sys"                 = "vendor/mlua-sys";
    "lua-src"                  = "vendor/lua-src";
    "wasm-bindgen-macro"       = "vendor/wasm-bindgen-macro/macro";
    "wasm-bindgen-macro-support" = "vendor/wasm-bindgen-macro/macro-support";
}
foreach ($k in $patchChecks.Keys) {
    $needle = "$k = { path = `"$($patchChecks[$k])`" }"
    Add-Check "patch: $k -> $($patchChecks[$k])" `
        ($engineTomlRaw.IndexOf($needle, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) `
        "missing needle: $needle"
}

# ── B. mlua-sys：wasm libc shim（C 半 wasm-shim-c + Rust 半 wasm_shim.rs）──────
$mluaSys = Join-Path $vendorRoot "mlua-sys"
$shimInclude = Join-Path $mluaSys "wasm-shim-c\include"
$shimCsrc = Join-Path $mluaSys "wasm-shim-c\csrc"
if (Test-Path -LiteralPath $shimInclude) {
    $hCount = @(Get-ChildItem -LiteralPath $shimInclude -Filter *.h -File -ErrorAction SilentlyContinue).Count
    Add-Check "mlua-sys: wasm-shim-c/include 头文件数" ($hCount -ge 10) "found $hCount .h (expect >= 10)"
} else {
    Add-Check "mlua-sys: wasm-shim-c/include 存在" $false "missing $shimInclude"
}
foreach ($c in @("mlua_trampoline.c", "sjlj.c", "stdio.c")) {
    Add-Check "mlua-sys: wasm-shim-c/csrc/$c" (Test-Path -LiteralPath (Join-Path $shimCsrc $c)) "missing $shimCsrc\$c"
}
$wasmShimRs = Join-Path $mluaSys "src\wasm_shim.rs"
if (Test-Path -LiteralPath $wasmShimRs) {
    $sz = (Get-Item -LiteralPath $wasmShimRs).Length
    Add-Check "mlua-sys: src/wasm_shim.rs 非空(Rust 半)" ($sz -gt 10000) "size=$sz (expect > 10KB)"
} else {
    Add-Check "mlua-sys: src/wasm_shim.rs 存在" $false "missing $wasmShimRs"
}
Test-Contains (Join-Path $mluaSys "src\lib.rs") @("#[path = `"wasm_shim.rs`"]", "mod wasm_shim") "mlua-sys: src/lib.rs 引入 wasm_shim"
Test-Contains (Join-Path $mluaSys "build\main_inner.rs") @("build_wasm_shim", "DEP_WASM_LUA_SHIM_INCLUDE_DIR", "rerun-if-changed=wasm-shim-c") "mlua-sys: build 脚本集成 shim"
Add-Check "mlua-sys: Cargo.toml.orig 存在" (Test-Path -LiteralPath (Join-Path $mluaSys "Cargo.toml.orig")) "missing mlua-sys\Cargo.toml.orig"

# ── C. lua-src：zig cc 编译 Lua C（wasm32 分支）────────────────────────────────
$luaSrc = Join-Path $vendorRoot "lua-src"
$luaLibRs = Join-Path $luaSrc "src\lib.rs"
Test-Contains $luaLibRs @("wasm32-unknown-unknown") "lua-src: src/lib.rs wasm32 分支"
Test-Contains $luaLibRs @("-fwasm-exceptions", "-wasm-enable-sjlj") "lua-src: src/lib.rs wasm EH 编译参数"
Test-Contains $luaLibRs @("liolib.c") "lua-src: src/lib.rs 排除 liolib(独立工具不编入)"
Add-Check "lua-src: 现用版本目录 lua-5.5.0" (Test-Path -LiteralPath (Join-Path $luaSrc "lua-5.5.0")) "missing lua-src\lua-5.5.0"
Add-Check "lua-src: Cargo.toml.orig 存在" (Test-Path -LiteralPath (Join-Path $luaSrc "Cargo.toml.orig")) "missing lua-src\Cargo.toml.orig"

# ── D. mlua：wasm EH 错误链路（Err 不在 Rust 里 raise）────────────────────────
$mlua = Join-Path $vendorRoot "mlua"
Test-Contains (Join-Path $mlua "src\state\util.rs")  @("#[cfg(target_arch = `"wasm32`")]") "mlua: src/state/util.rs wasm32 分支"
Test-Contains (Join-Path $mlua "src\state\raw.rs")    @("target_arch = `"wasm32`"") "mlua: src/state/raw.rs wasm32 分支"
Test-Contains (Join-Path $mlua "src\util\error.rs")   @("target_arch = `"wasm32`"") "mlua: src/util/error.rs wasm 跳过 catch_unwind"
Add-Check "mlua: Cargo.toml.orig 存在" (Test-Path -LiteralPath (Join-Path $mlua "Cargo.toml.orig")) "missing mlua\Cargo.toml.orig"

# ── E. wasm-bindgen-macro：extern "C-unwind" -> extern "C"（无 C-unwind 残留）──
$wbm = Join-Path $vendorRoot "wasm-bindgen-macro"
$unwindHits = 0
foreach ($dir in @("macro\src", "macro-support\src")) {
    $dirPath = Join-Path $wbm $dir
    if (Test-Path -LiteralPath $dirPath) {
        $unwindHits += @(Get-ChildItem -LiteralPath $dirPath -Recurse -Filter *.rs -File -ErrorAction SilentlyContinue |
            Select-String -Pattern "C-unwind" -SimpleMatch -ErrorAction SilentlyContinue).Count
    }
}
Add-Check "wasm-bindgen-macro: 无 C-unwind 残留(已改 extern C)" ($unwindHits -eq 0) "found $unwindHits C-unwind occurrence(s) in macro/macro-support"
Test-Contains (Join-Path $wbm "macro-support\src\codegen.rs") @('extern "C"') "wasm-bindgen-macro: codegen.rs 导出模板为 extern C"

# ── F. 版本耦合：vendor 与 Cargo.lock 一致（升级时必须三处同改）───────────────
function Get-LockVersion([string]$Name) {
    $lines = Get-Content -LiteralPath $lockFile
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -eq "name = `"$Name`"") {
            for ($j = $i + 1; $j -lt [math]::Min($i + 4, $lines.Count); $j++) {
                if ($lines[$j] -match '^version = "([^"]+)"') { return $Matches[1] }
            }
        }
    }
    return ""
}
function Get-VendorVersion([string]$RelativeToml) {
    $toml = Join-Path $vendorRoot $RelativeToml
    if (-not (Test-Path -LiteralPath $toml)) { return "" }
    $m = Select-String -LiteralPath $toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if ($m -and $m.Matches.Count -gt 0) { return $m.Matches[0].Groups[1].Value }
    return ""
}
$lockVersions = @{
    "mlua"    = (Get-LockVersion "mlua");
    "mlua-sys" = (Get-LockVersion "mlua-sys");
    "lua-src" = (Get-LockVersion "lua-src");
    "wasm-bindgen" = (Get-LockVersion "wasm-bindgen");
}
foreach ($name in @("mlua", "mlua-sys", "lua-src")) {
    $vV = Get-VendorVersion "$name\Cargo.toml"
    $lV = $lockVersions[$name]
    Add-Check "版本耦合: $name vendor=$vV lock=$lV" ($vV -eq $lV -and $vV -ne "") "vendor($vV) != lock($lV)"
}
$wbmV = Get-VendorVersion "wasm-bindgen-macro\macro\Cargo.toml"
Add-Check "版本耦合: wasm-bindgen 三处一致(macro=$wbmV support=$wbmV lock=$($lockVersions['wasm-bindgen']))" `
    ($wbmV -ne "" -and $wbmV -eq $lockVersions["wasm-bindgen"]) `
    "macro($wbmV) != lock wasm-bindgen($($lockVersions['wasm-bindgen']))"

# ── G. wasm-bindgen-cli：随仓源码（离线构建 CLI 的第三回退，未打 patch）───────
Add-Check "wasm-bindgen-cli: 随仓源码存在(未打 patch, 供离线构建)" `
    (Test-Path -LiteralPath (Join-Path $vendorRoot "wasm-bindgen-cli\cli")) "missing vendor\wasm-bindgen-cli\cli"

# ── 汇总 ─────────────────────────────────────────────────────────────────────
Write-Host ""
Write-Host ("vendor patches: " + $passed + " passed, " + $failed.Count + " failed")
if ($failed.Count -gt 0) {
    Write-Host "vendor_patches_failed" -ForegroundColor Red
    exit 1
}
Write-Host "vendor_patches_ok" -ForegroundColor Green
exit 0
