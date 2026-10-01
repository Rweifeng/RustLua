# rustdoc 站点构建（优化项 10）
#
# 做什么：跑 `cargo doc` 生成全 workspace 的 rustdoc，然后把它组装成一个**可以直接发布**
# 的静态站点目录——cargo 对多 crate workspace **不写根 `index.html`**（产物里只有
# `doc/<crate>/index.html`），直接把 `target/doc` 传到静态托管会得到一个 404 的站点根。
# 所以这里补三件事：根导航页、`.nojekyll`（GitHub Pages 默认走 Jekyll，`_downloads`/
# 下划线目录会被吞掉）、以及一份可核对的产物统计。
#
# 用法：
#   powershell -File scripts/gen-rustdoc.ps1                    # 构建并打印站点目录
#   powershell -File scripts/gen-rustdoc.ps1 -SiteDir D:\docs   # 指定输出目录
#   powershell -File scripts/gen-rustdoc.ps1 -Open             # 构建完用默认浏览器打开
#   powershell -File scripts/gen-rustdoc.ps1 -AllowWarnings     # 不把 rustdoc 告警当错误
#
# 退出码沿用本仓库口径：0=通过；1=判红（文档没生成、告警非零、站点缺根页）；
# 3=环境受限（没有 cargo）——显式打印原因，**绝不把"跳过"说成"通过"**。
#
# 默认 `-D warnings`：断链在站点上就是点不开的按钮，而 fmt/clippy/test 全绿时文档可以同时烂掉，
# 只有把告警升级为错误才能拦住这类增量。存量断链已清零，见 docs/61。

[CmdletBinding()]
param(
    [string]$SiteDir = "",
    [switch]$Open,
    # 关掉 `-D warnings`（临时排障用；CI 与 verify.ps1 都不该带这个开关）。
    [switch]$AllowWarnings
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$engineRoot = Join-Path $repoRoot "engine"
$docRoot = Join-Path $engineRoot "target\doc"
if ([string]::IsNullOrWhiteSpace($SiteDir)) {
    $SiteDir = Join-Path $repoRoot "artifacts\doc-site"
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "rustdoc_skipped reason=cargo_not_found exit=3（没有 cargo 就没法生成文档；这不是通过）" -ForegroundColor Yellow
    exit 3
}

# ---------------------------------------------------------------- 生成 rustdoc
Write-Host "==> cargo doc --no-deps --workspace --lib" -ForegroundColor Cyan
Push-Location $engineRoot
try {
    $previousDocFlags = $env:RUSTDOCFLAGS
    # cargo 的进度输出走 stderr；PS 5.1 在 `$ErrorActionPreference="Stop"` 下会把原生
    # stderr 当异常抛（`NativeCommandError`），看起来像"文档构建崩了"其实是脚本口径问题。
    # 与 verify.ps1 的 Invoke-Checked 同一处理：调用期间降到 Continue，用退出码判定。
    $previousPreference = $ErrorActionPreference
    try {
        if (-not $AllowWarnings) { $env:RUSTDOCFLAGS = "-D warnings" }
        $ErrorActionPreference = "Continue"
        # `--lib` 而非 `--all-targets`：文档站点的读者要的是 API 面（lib），examples/benches
        # 的私有胶水进目录只会把导航淹没；examples 的公开入口本来就在各 crate 的文档里说明。
        & cargo doc --no-deps --workspace --lib 2>&1 | ForEach-Object { Write-Host "$_" }
        $docExit = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousPreference
        $env:RUSTDOCFLAGS = $previousDocFlags
    }
} finally {
    Pop-Location
}
if ($docExit -ne 0) {
    Write-Host "rustdoc_failed exit=$docExit（cargo doc 非零退出；告警按错误处理时先跑 -AllowWarnings 看清单）" -ForegroundColor Red
    exit 1
}
if (-not (Test-Path (Join-Path $docRoot "index.html")) -and -not (Test-Path $docRoot)) {
    Write-Host "rustdoc_failed reason=target\doc 不存在" -ForegroundColor Red
    exit 1
}

# ---------------------------------------------------------------- 组装站点
Write-Host "==> 组装站点 $SiteDir" -ForegroundColor Cyan
if (Test-Path $SiteDir) { Remove-Item $SiteDir -Recurse -Force }
New-Item -ItemType Directory -Force -Path $SiteDir | Out-Null
Copy-Item -Path (Join-Path $docRoot "*") -Destination $SiteDir -Recurse -Force

# crate 目录 = 有 index.html 的一级子目录；`src`（源码浏览器）与 `static.files` 不是 crate。
$crates = Get-ChildItem $SiteDir -Directory | Where-Object {
    $_.Name -notin @("src", "static.files") -and (Test-Path (Join-Path $_.FullName "index.html"))
} | Sort-Object Name
if ($crates.Count -eq 0) {
    Write-Host "rustdoc_failed reason=站点里没有任何 crate 文档目录" -ForegroundColor Red
    exit 1
}

# 每个 crate 一句话：优先取 Cargo.toml 的 description，没有就用下表（仓库当前布局）。
$fallback = @{
    engine    = "纯逻辑核心（ECS / 场景 / Lua / 渲染数据 / 资源 / 物理）与 wgpu+winit 宿主"
    editor    = "编辑器：文档态、撤销历史、面板与场景校验"
    game      = "桌面宿主可执行与工程校验（--check / --pack / headless）"
    game_web  = "Web/wasm 宿主：浏览器与微信小游戏里的运行期入口"
    assetc    = "资源命令行工具：导入、烘焙、打包"
}
$links = New-Object System.Collections.Generic.List[string]
foreach ($crate in $crates) {
    $name = $crate.Name
    $description = $null
    $manifest = Join-Path $engineRoot "crates\$name\Cargo.toml"
    if (Test-Path $manifest) {
        $line = Select-String -Path $manifest -Pattern '^\s*description\s*=\s*"(.*)"' | Select-Object -First 1
        if ($line) { $description = $line.Matches[0].Groups[1].Value }
    }
    if ([string]::IsNullOrWhiteSpace($description)) {
        $description = if ($fallback.ContainsKey($name)) { $fallback[$name] } else { "（Cargo.toml 未写 description）" }
    }
    $links.Add("      <li><a href=`"./$name/index.html`"><code>$name</code></a><span>$description</span></li>")
}

# Lua API 参考页是这次发布的**主要读者入口**（41 个模块表 / 275 个绑定函数），
# 单独置顶，不让它埋在 engine → lua_api 的三级导航里。
$luaApiHref = if ($crates.Name -contains "engine") { "./engine/lua_api/index.html" } else { $null }
$luaNavLink = if ($luaApiHref) {
    "      <p class=`"lead`"><a class=`"primary`" href=`"$luaApiHref`">Lua API 参考</a> —— 脚本侧能调用的每个模块与函数（由运行期 API 面基线自动生成，见 <code>scripts/gen-lua-api-docs.js</code>）。</p>"
} else { "" }

$index = @"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>RustLua — API 文档</title>
<style>
  :root { color-scheme: light dark; }
  body { font: 16px/1.65 system-ui, "Segoe UI", "Microsoft YaHei", sans-serif;
         max-width: 52rem; margin: 3rem auto 4rem; padding: 0 1.25rem; color: #222; }
  h1 { font-size: 1.6rem; margin-bottom: .25rem; }
  .lead { color: #555; }
  a.primary { font-weight: 600; }
  ul.crates { list-style: none; padding: 0; margin: 1.5rem 0 0; }
  ul.crates li { padding: .55rem 0; border-bottom: 1px solid #eee; display: grid;
                 grid-template-columns: 11rem 1fr; gap: .75rem; }
  ul.crates span { color: #555; font-size: .92rem; }
  code { font-family: ui-monospace, Consolas, monospace; background: #f4f4f5; padding: .1em .35em; border-radius: 4px; }
  footer { margin-top: 2.5rem; color: #777; font-size: .88rem; }
  @media (prefers-color-scheme: dark) {
    body { color: #ddd; } ul.crates li { border-color: #333; } code { background: #26262a; }
    .lead, ul.crates span { color: #9aa; }
  }
</style>
</head>
<body>
  <h1>RustLua — API 文档</h1>
  <p class="lead">由 <code>cargo doc</code> 生成（<code>powershell -File scripts/gen-rustdoc.ps1</code>），
     共 $($crates.Count) 个 crate。</p>
$luaNavLink
  <ul class="crates">
$($links -join "`n")
  </ul>
  <footer>生成时间：$(Get-Date -Format "yyyy-MM-dd HH:mm") ·
     源码：<a href="https://github.com/Rweifeng/RustLua">Rweifeng/RustLua</a></footer>
</body>
</html>
"@
[System.IO.File]::WriteAllText((Join-Path $SiteDir "index.html"), $index, (New-Object System.Text.UTF8Encoding($false)))
Set-Content -Path (Join-Path $SiteDir ".nojekyll") -Value "" -Encoding Ascii

# ---------------------------------------------------------------- 核对与汇报
$rootPage = Join-Path $SiteDir "index.html"
if (-not (Test-Path $rootPage)) {
    Write-Host "rustdoc_failed reason=站点根页缺失" -ForegroundColor Red
    exit 1
}
foreach ($crate in $crates) {
    if (-not (Test-Path (Join-Path $SiteDir "$($crate.Name)\index.html"))) {
        Write-Host "rustdoc_failed reason=crate $($crate.Name) 的 index.html 缺失" -ForegroundColor Red
        exit 1
    }
}
$fileCount = (Get-ChildItem $SiteDir -Recurse -File).Count
$sizeMb = [math]::Round(((Get-ChildItem $SiteDir -Recurse -File | Measure-Object -Property Length -Sum).Sum / 1MB), 1)
Write-Host "rustdoc_ok crates=$($crates.Count) files=$fileCount size=${sizeMb}MB site=$SiteDir" -ForegroundColor Green

if ($Open) { Start-Process $rootPage }
