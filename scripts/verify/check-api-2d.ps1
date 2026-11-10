# check-api-2d.ps1 —— 2D 可插拔 API 面门（M81 批 3）
#
# 三段（全部 --no-default-features，即 2D 构建）：
#   1. rustlua-scripting 的 api_surface 快照/契约测试（对 lua_api_surface_2d.json 基线）；
#   2. game 叶子 `cargo check --no-default-features`（feature 链路边漏改的编译面证据）；
#   3. engine 的 sample_project 工程门 2D 段（面外演示按静态扫描跳过并打印原因；
#      覆盖率门仍要求 2D 面 100%——面内缺演示不豁免）。
# 另加一段 Node：2D LuaLS 产物（--surface）再生成一致性。
#
# 刻意**不塞进 verify.ps1 默认串行段**：2D 构建与全量构建是两套 cargo feature 缓存，
# 串行段里交替切换会让每个门都重编（M81 计划「门生态」节）；CI 在 quality.yml 单列一步。
#
# 用法：
#   powershell -NoProfile -File scripts/verify/check-api-2d.ps1
# 退出码：0 = 全绿；1 = 任一段红（输出里带段名）。

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$engineRoot = Join-Path $projectRoot "engine"

function Invoke-Gate([string]$Label, [scriptblock]$Body) {
    Write-Host "==> $Label" -ForegroundColor Cyan
    & $Body
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[FAIL] $Label (exit=$LASTEXITCODE)" -ForegroundColor Red
        exit 1
    }
    Write-Host "[OK] $Label" -ForegroundColor Green
}

Push-Location $engineRoot
try {
    # PS 5.1：cargo 的正常进度走 stderr，Stop 偏好下会变成终止性错误——逐段降为 Continue。
    $savedEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        Invoke-Gate "2D API 面快照/契约（rustlua-scripting api_surface）" {
            cargo test -q -p rustlua-scripting api_surface --no-default-features
        }
        Invoke-Gate "game 叶子 2D check（--no-default-features）" {
            cargo check -q -p game --no-default-features
        }
        Invoke-Gate "engine sample_project 2D 段（面外演示跳过）" {
            cargo test -q -p engine --lib sample_project --no-default-features
        }
    }
    finally {
        $ErrorActionPreference = $savedEap
    }
}
finally {
    Pop-Location
}

Invoke-Gate "2D LuaLS 产物再生成一致（--surface --check）" {
    node (Join-Path $projectRoot "scripts\docgen\gen-lua-api-docs.js") `
        --surface engine/crates/rustlua-scripting/src/api_surface_snapshot/lua_api_surface_2d.json --check
}

Write-Host "check_api_2d_ok" -ForegroundColor Green
exit 0