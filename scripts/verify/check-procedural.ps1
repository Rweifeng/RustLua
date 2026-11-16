# check-procedural.ps1 —— 程序化素材 API 面门（M82-A primitive + M82-B2 贴图 + M82-C 合成音效）
#
# 十三段（A 批 5 + B2 6 + C 2：primitive/texture/synth 单元·注册表·Lua 面 + 离屏像素·确定性·3D 材质）：
#   1. 单元（rustlua-render primitives）：顶点/索引/法线/AABB/确定性/逐字段拒绝；
#   2. 离屏像素 + 确定性（rustlua-gpu primitives_mesh）：六种基本体进 GPU、同规格两帧逐字节相同；
#      RUSTLUA_REQUIRE_GPU=1 把「无适配器悄悄跳过」升成硬失败（防 gpu_skipped 假绿）；
#   3. 演示自检（engine sample_project_demos_self_test）：api_3d 的 primitive 步骤真调 mesh.set/get/set_batch；
#   4. 全量 API 面契约（rustlua-scripting api_surface）：运行期面 == 全量基线；
#   5. 2D API 面契约（--no-default-features）：运行期面 == 2D 基线。
# A 批不增 Lua 函数 ⇒ 4/5 两段绿即「两份基线逐字节不变」的运行期凭证（基线文件本身未被 --write 触碰）。
#
# 刻意**不塞进 verify.ps1 默认串行段**：离屏 GPU 门与全量/2D 两套 feature 缓存交替会互相重编
# （照 check-api-2d.ps1 先例，M82 计划「门生态」节）；CI 单列一步。
#
# 用法：
#   powershell -NoProfile -File scripts/verify/check-procedural.ps1
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
        Invoke-Gate "primitive 单元（rustlua-render primitives）" {
            cargo test -q -p rustlua-render --lib primitives
        }
        Invoke-Gate "texture 单元（rustlua-render texture_gen）" {
            cargo test -q -p rustlua-render --lib texture_gen
        }
        Invoke-Gate "texture 注册表（rustlua-render texture_lua）" {
            cargo test -q -p rustlua-render --lib texture_lua
        }
        Invoke-Gate "texture 文字（rustlua-render text_texture）" {
            cargo test -q -p rustlua-render --lib text_texture
        }
        Invoke-Gate "texture Lua 面（rustlua-scripting texture_gen）" {
            cargo test -q -p rustlua-scripting --lib texture_gen
        }
        Invoke-Gate "synth 单元（rustlua-audio synth）" {
            cargo test -q -p rustlua-audio --lib synth
        }
        Invoke-Gate "synth Lua 面（rustlua-scripting synth_play）" {
            cargo test -q -p rustlua-scripting --lib synth_play
        }
        Invoke-Gate "primitive 离屏像素 + 确定性（rustlua-gpu primitives_mesh）" {
            $env:RUSTLUA_REQUIRE_GPU = "1"
            cargo test -q -p rustlua-gpu --lib primitives_mesh -- --nocapture
            Remove-Item Env:\RUSTLUA_REQUIRE_GPU -ErrorAction SilentlyContinue
        }
        Invoke-Gate "texture 离屏像素 + 确定性（rustlua-gpu texture_gen_gpu）" {
            $env:RUSTLUA_REQUIRE_GPU = "1"
            cargo test -q -p rustlua-gpu --lib texture_gen_gpu -- --nocapture --test-threads=1
            Remove-Item Env:\RUSTLUA_REQUIRE_GPU -ErrorAction SilentlyContinue
        }
        Invoke-Gate "texture 材质（rustlua-gpu texture_material3d）" {
            $env:RUSTLUA_REQUIRE_GPU = "1"
            cargo test -q -p rustlua-gpu --lib texture_material3d -- --nocapture --test-threads=1
            Remove-Item Env:\RUSTLUA_REQUIRE_GPU -ErrorAction SilentlyContinue
        }
        Invoke-Gate "演示自检（engine sample_project_demos_self_test）" {
            cargo test -q -p engine --lib sample_project_demos_self_test
        }
        Invoke-Gate "全量 API 面契约（rustlua-scripting api_surface）" {
            cargo test -q -p rustlua-scripting api_surface
        }
        Invoke-Gate "2D API 面契约（--no-default-features）" {
            cargo test -q -p rustlua-scripting api_surface --no-default-features
        }
    }
    finally {
        $ErrorActionPreference = $savedEap
        Remove-Item Env:\RUSTLUA_REQUIRE_GPU -ErrorAction SilentlyContinue
    }
}
finally {
    Pop-Location
}

Write-Host "check_procedural_ok" -ForegroundColor Green
exit 0