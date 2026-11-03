# wasm-bindgen CLI 的定位与按需安装（被 package.ps1 / verify-wasm-lua.ps1 共用）。
#
# 为什么必须管版本：CLI 与 wasm 模块里的 wasm-bindgen 元数据 schema **必须精确一致**。
# 本机 PATH 上的 CLI 版本与 crate 版本错位是最常见的失败模式：
#   - CLI 偏旧：报 "rust Wasm file schema version: 0.2.129 / this binary schema version: 0.2.127"
#     并直接拒绝生成胶水；
#   - 同族但不匹配 catch 处理：报 "externref table required for catch wrappers"。
# 原实现只判断"PATH 上有没有 wasm-bindgen"，版本不对也照用，于是一台机器上的旧 CLI
# 会把 webgl 与微信两条发布路径一起弄挂（实测 2026-09-30：PATH 0.2.127 vs crate 0.2.129）。
#
# 版本的**唯一事实源**是 engine/Cargo.lock 里 `wasm-bindgen` 那条 —— 升级 crate 时
# 不用回来改这里的魔数。
#
# 解析顺序（先命中且**版本精确匹配**才返回）：
#   1. $env:WASM_BINDGEN                              —— 显式覆盖
#   2. engine\.tools\wasm-bindgen\bin\...             —— 本脚本装进仓外缓存的那份
#   3. engine\vendor\wasm-bindgen-cli 已构建出的那份  —— 版本对得上就用（离线机器友好）
#   4. PATH 上的 wasm-bindgen
#   5. 全不匹配且允许安装时：cargo install 精确版本到仓外缓存
#      `--root engine\.tools\wasm-bindgen`，**不碰全局 `~\.cargo\bin`**：既不顶掉别人
#      装的全局工具，也不留下"只有这台机器能打包"的隐式依赖。
#
# 命令行用法：
#   powershell -File scripts\build\wasm-bindgen.ps1              # 解析（必要时安装）并打印
#   powershell -File scripts\build\wasm-bindgen.ps1 -PrintPath   # 只打印 exe 路径
#   powershell -File scripts\build\wasm-bindgen.ps1 -NoInstall   # 只解析，绝不安装（环境探测）
[CmdletBinding()]
param(
    [switch]$NoInstall,
    [switch]$PrintPath
)

$workspaceRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$engineRoot = Join-Path $workspaceRoot "engine"
$cargoLock = Join-Path $engineRoot "Cargo.lock"
$toolsRoot = Join-Path $engineRoot ".tools\wasm-bindgen"
$toolsExe = Join-Path $toolsRoot "bin\wasm-bindgen.exe"
$vendorCliExe = Join-Path $engineRoot "vendor\wasm-bindgen-cli\cli\target\release\wasm-bindgen.exe"

function Get-WasmBindgenCrateVersion {
    if (-not (Test-Path -LiteralPath $cargoLock -PathType Leaf)) { return $null }
    $lines = @(Get-Content -LiteralPath $cargoLock)
    for ($i = 0; $i -lt $lines.Count; $i += 1) {
        if ($lines[$i] -match '^name\s*=\s*"wasm-bindgen"\s*$') {
            # Cargo.lock 的格式：name 行之后紧跟 version 行。
            if ($i + 1 -lt $lines.Count -and $lines[$i + 1] -match '^version\s*=\s*"(.+?)"\s*$') {
                return $Matches[1]
            }
        }
    }
    return $null
}

# 把"这条路径到底有没有那个可执行文件"收敛成一处，并且**不只靠一条判据**：
#   1) `Get-Item -Force`（拿 FileSystemInfo，判 PSIsContainer）；
#   2) 还拿不到就枚举父目录按名字精确匹配（`Get-ChildItem -File`）。
# 为什么要两条：CI（windows-latest）2026-11-02 实测过"目录里明明有 wasm-bindgen.exe
# （6,119,936 字节，Get-ChildItem 列得出来），`Test-Path -PathType Leaf` 却返回 False"，
# 于是版本检查永远拿不到 exe，web-evidence 连红十几轮。枚举父目录这条路在同样的现场是好的，
# 所以它当兜底；两条都拿不到才承认"真的没有"。
function Resolve-WasmBindgenExePath([string]$Exe) {
    if ([string]::IsNullOrWhiteSpace($Exe)) { return $null }
    try {
        $item = Get-Item -LiteralPath $Exe -Force -ErrorAction Stop
        if ($null -ne $item -and -not $item.PSIsContainer) { return $item.FullName }
    }
    catch { }
    $dir = Split-Path -Parent $Exe
    $leaf = Split-Path -Leaf $Exe
    if ($dir -and $leaf) {
        try {
            $hit = @(Get-ChildItem -LiteralPath $dir -File -Force -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -eq $leaf })
            if ($hit.Count -ge 1) { return $hit[0].FullName }
        }
        catch { }
    }
    # 第三条：直接在 $toolsRoot 下递归找同名文件。**不走 Split-Path**——CI 上出现过
    # "Get-ChildItem $toolsRoot\bin 列得出文件，但按同一个 $toolsRoot 拼出来的 $Exe 直接
    # 查不到"的现场（$toolsExe 字符串本身与预期的差异迄今未定位），这条路只依赖
    # $toolsRoot 这一个字符串，而它是**已经证明能列出文件**的那个。
    if ($leaf -and $toolsRoot -and (Test-Path -LiteralPath $toolsRoot)) {
        try {
            $deep = @(Get-ChildItem -LiteralPath $toolsRoot -Recurse -File -Force -Filter $leaf -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -eq $leaf })
            if ($deep.Count -ge 1) { return $deep[0].FullName }
        }
        catch { }
    }
    return $null
}

function Get-WasmBindgenCliVersion([string]$Exe, [switch]$Exact) {
    if ([string]::IsNullOrWhiteSpace($Exe)) { return $null }
    # $Exact：只认**这条路径本身**（不走"枚举父目录/在 .tools 下递归找"的兜底）。
    # 兜底路径的存在是为了兼容"Test-Path 说没有、目录里其实有"的现场，但它有个副作用——
    # 对一个**不存在**的路径也可能返回别的文件，于是"这个路径能不能跑"这个问题就被答错了。
    # 任何"我要确认这条路径真实可用"的场合都必须用 -Exact（2026-11-03 本机踩到：
    # 临时副本根本没落地，却因为兜底找到了 .tools 里那份而被判为通过）。
    if ($Exact) {
        $item = $null
        try { $item = Get-Item -LiteralPath $Exe -Force -ErrorAction Stop } catch { return $null }
        if ($null -eq $item -or $item.PSIsContainer) { return $null }
    }
    else {
        $resolvedExe = Resolve-WasmBindgenExePath $Exe
        if (-not $resolvedExe) { return $null }
        $Exe = $resolvedExe
    }
    # 原生工具的正常输出也可能落 stderr；PS 5.1 在 EAP=Stop 下会把它变成终止性错误，
    # 所以逐次调用都临时降级（同 package.ps1 调 cargo 的既有模式）。
    $savedEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $out = & $Exe --version 2>$null
        if ($LASTEXITCODE -ne 0 -or -not $out) { return $null }
        $text = ($out | Select-Object -First 1).ToString()
        if ($text -match '(\d+\.\d+\.\d+)') { return $Matches[1] }
        return $null
    }
    catch { return $null }
    finally { $ErrorActionPreference = $savedEap }
}

function Test-WasmBindgenCli([string]$Exe, [string]$WantVersion, [switch]$Exact) {
    if ([string]::IsNullOrWhiteSpace($Exe)) { return $false }
    if ($Exact) { return (Get-WasmBindgenCliVersion $Exe -Exact) -eq $WantVersion }
    return (Get-WasmBindgenCliVersion $Exe) -eq $WantVersion
}

# 把"候选路径"归一成**真实存在的那个路径**：候选可能是拼出来的、可能在 CI 上是个
# Test-Path 认不出的怪路径，而兜底解析拿到的 FullName 才是能 `&` 的那个。归一不了就返回 $null。
function Get-RealWasmBindgenPath([string]$Candidate, [string]$WantVersion) {
    if ([string]::IsNullOrWhiteSpace($Candidate)) { return $null }
    if (-not (Test-WasmBindgenCli $Candidate $WantVersion)) { return $null }
    $real = Resolve-WasmBindgenExePath $Candidate
    if (-not $real) { return $null }
    if ((Test-WasmBindgenCli $real $WantVersion -Exact)) { return $real }
    # 归一后的路径仍可能"Test-Path 认不出"（Windows 上文件名含不可见字符之类），
    # 只要它能跑出正确版本就仍然采用——但此时**不再**做临时副本（副本会复制到一个
    # 同样认不出的名字下，等于自找麻烦）。
    if ((Test-WasmBindgenCli $real $WantVersion)) { return $real }
    return $null
}

function Install-WasmBindgenCli([string]$WantVersion) {
    New-Item -ItemType Directory -Force -Path $toolsRoot | Out-Null
    Write-Host "==> Installing wasm-bindgen-cli $WantVersion into engine\.tools (cargo install --root)" -ForegroundColor Cyan
    $savedEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $exit = 1
    try {
        Push-Location $engineRoot
        try {
            cargo install wasm-bindgen-cli --version $WantVersion --root $toolsRoot --locked 2>&1 |
                ForEach-Object { Write-Host $_ }
            $exit = $LASTEXITCODE
        }
        finally { Pop-Location }
    }
    finally { $ErrorActionPreference = $savedEap }
    if ($exit -ne 0) { throw "wasm-bindgen-cli $WantVersion install failed (exit $exit)." }
    if (-not (Test-WasmBindgenCli $toolsExe $WantVersion)) {
        # 判红时把"为什么判不出来"一起带走：CI 上这一步 2026-11-02 只留下"version check
        # failed"，日志要认证才能读、annotation 还被截断，等于没有信息。这里把 cargo/rustc
        # 版本、可执行文件本身、`--version` 的原始输出与退出码、bin 目录清单全部塞进消息里，
        # 下一次红就能一眼定位（是装成了别的版本、exe 根本跑不起来、还是根本没落地）。
        $detail = New-Object System.Collections.Generic.List[string]
        $savedEapDetail = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            $cargoVersion = (& cargo --version 2>&1 | Out-String).Trim()
            $rustcVersion = (& rustc --version 2>&1 | Out-String).Trim()
            $detail.Add("want=$WantVersion")
            $detail.Add("cargo=$cargoVersion")
            $detail.Add("rustc=$rustcVersion")
            $detail.Add("exe=$toolsExe exists=$(Test-Path -LiteralPath $toolsExe -PathType Leaf)")
            $itemProbe = $null
            try { $itemProbe = Get-Item -LiteralPath $toolsExe -Force -ErrorAction Stop } catch { }
            $detail.Add("item_ok=$($null -ne $itemProbe)")
            $binDirProbe = Join-Path $toolsRoot "bin"
            $detail.Add("exe_len=$($toolsExe.Length)")
            $detail.Add("parent_repr=[$(Split-Path -Parent $toolsExe)]")
            $detail.Add("bin_dir_repr=[$binDirProbe]")
            $detail.Add("dir_same=$((Split-Path -Parent $toolsExe) -eq $binDirProbe)")
            try {
                $parentEntries = @(Get-ChildItem -LiteralPath (Split-Path -Parent $toolsExe) -File -Force -ErrorAction SilentlyContinue |
                    ForEach-Object { "len=$($_.Length)" })
                $detail.Add("parent_entries=$($parentEntries.Count)")
            }
            catch { $detail.Add("parent_entries=err") }
            try {
                $deepHit = @(Get-ChildItem -LiteralPath $toolsRoot -Recurse -File -Force -Filter "wasm-bindgen.exe" -ErrorAction SilentlyContinue)
                $detail.Add("deep_hits=$($deepHit.Count)")
            }
            catch { $detail.Add("deep_hits=err") }
            if (Test-Path -LiteralPath $toolsExe -PathType Leaf) {
                $info = Get-Item -LiteralPath $toolsExe
                $detail.Add("exe_size=$($info.Length)")
                $detail.Add("exe_mtime=$($info.LastWriteTime.ToString('s'))")
                $rawVersion = (& $toolsExe --version 2>&1 | Out-String).Trim()
                $detail.Add("exe_version_exit=$LASTEXITCODE")
                $detail.Add("exe_version_raw='$rawVersion'")
            }
            $binDir = Join-Path $toolsRoot "bin"
            if (Test-Path -LiteralPath $binDir) {
                $binList = @(Get-ChildItem -LiteralPath $binDir | ForEach-Object { "$($_.Name)=$($_.Length)" }) -join ','
                $detail.Add("bin=[$binList]")
            }
            $detail.Add("parsed_version='$(Get-WasmBindgenCliVersion $toolsExe)'")
        }
        catch { $detail.Add("diagnostics_failed=$($_.Exception.Message)") }
        finally { $ErrorActionPreference = $savedEapDetail }
        # 先补一次 --force：`cargo install` 遇到"同一版本已登记"会直接 Ignored 并退 0；若那份
        # 登记旁边的 exe 是坏的或根本没落地，就会长期停在"装了但版本不对"这一句上（CI 上就是
        # 这个签名）。--force 会真的重装一遍，把这种半成品状态修好。
        $forceOut = @()
        $forceExit = 0
        # cargo 的正常进度/thin 警告都走 stderr；PS 5.1 在 EAP=Stop 下会把 stderr 变成终止性
        # 错误（本文件第 80 行的 install 段为此显式降级），这里同样必须降级。
        $savedEapForce = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        Push-Location $engineRoot
        try {
            $forceOut = @(cargo install wasm-bindgen-cli --version $WantVersion --root $toolsRoot --locked --force 2>&1 |
                ForEach-Object { "$_" })
            $forceExit = $LASTEXITCODE
            $forceOut | ForEach-Object { Write-Host $_ }
        }
        finally { Pop-Location; $ErrorActionPreference = $savedEapForce }
        $detail.Add("force_exit=$forceExit")
        if ($forceExit -ne 0) {
            $forceTail = (@($forceOut | Select-Object -Last 5) -join ' | ') -replace "[\r\n]+", ' '
            $detail.Add("force_tail='$forceTail'")
        }
        if (Test-WasmBindgenCli $toolsExe $WantVersion) {
            Write-Host "wasm-bindgen ready: $toolsExe ($WantVersion, reinstalled with --force)" -ForegroundColor Yellow
            return $toolsExe
        }

        # 版本检查失败**之后**的诊断要包含"能不能跑"这一项：force 装完仍然对不上，最可能是
        # exe 根本起不来（缺 DLL / 被杀软截断），而不是版本号错。这里先探一次，把退出码与
        # 原始输出带进 detail。
        $probeExit = $null
        $probeOut = ''
        $savedEapProbe = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            if (Test-Path -LiteralPath $toolsExe -PathType Leaf) {
                $probeOut = (& $toolsExe --version 2>&1 | Out-String).Trim()
                $probeExit = $LASTEXITCODE
            }
        }
        catch { $probeOut = "threw=$($_.Exception.Message)" }
        finally { $ErrorActionPreference = $savedEapProbe }
        $detail.Add("probe_exit=$probeExit")
        if ($probeOut.Length -gt 120) { $probeOut = $probeOut.Substring(0, 120) + '...' }
        $detail.Add("probe_out='$probeOut'")


        # 兜底：cargo 有时把可执行文件落到别处（`CARGO_INSTALL_ROOT`/`CARGO_HOME` 环境、
        # 或 `--root` 被上层配置顶掉）。只要**某个候选**的版本对得上，就搬到 `.tools` 里继续，
        # 不影响"版本必须精确匹配 crate 版本"这条不变量（错的版本照样判红）。
        $retryCandidates = @(
            (Join-Path $env:USERPROFILE ".cargo\bin\wasm-bindgen.exe"),
            (Join-Path $env:CARGO_HOME "bin\wasm-bindgen.exe")
        ) | Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Leaf) }
        foreach ($candidate in $retryCandidates) {
            if (Test-WasmBindgenCli $candidate $WantVersion) {
                Write-Host "==> wasm-bindgen 被 cargo 装到了别处，搬进 .tools：$candidate" -ForegroundColor Yellow
                New-Item -ItemType Directory -Force -Path (Split-Path $toolsExe) | Out-Null
                Copy-Item -LiteralPath $candidate -Destination $toolsExe -Force
                $detail.Add("rescued_from=$candidate")
                break
            }
        }
        if (Test-WasmBindgenCli $toolsExe $WantVersion) {
            Write-Host "wasm-bindgen ready: $toolsExe ($WantVersion, rescued)" -ForegroundColor Green
            return $toolsExe
        }
        # PowerShell 把 ErrorRecord 显示成 "FullyQualifiedErrorId : 前20字...尾20字"，中间全被
        # 吃掉——CI annotation 上就只剩 "wasm-bindgen-cl...'; force_exit=0"，等于白写。改用
        # workflow 命令 `::error::` 发一行：GitHub 把这一行**原样**收进 annotation，不受
        # PowerShell 的错误格式化截断影响。本地跑时它只是一行普通输出。
        $flat = (($detail -join '; ') -replace "[\r\n]+", ' ')
        if ($flat.Length -gt 1500) { $flat = $flat.Substring(0, 1500) + '...' }
        Write-Host "::error::wasm_bindgen_version_check_failed $flat"
        throw "wasm-bindgen-cli installed but version check failed: $toolsExe $($detail -join '; ')"
    }
    Write-Host "wasm-bindgen ready: $toolsExe ($WantVersion)" -ForegroundColor Green
    return $toolsExe
}

# 把"验证过的那份 exe"复制到进程外的临时目录再交出去。为什么：CI（windows-latest）2026-11-03
# 实测到"解析时能跑、几十分钟后真正 `& $cli` 时却报 `The term '...wasm-bindgen.exe' is not
# recognized`"——文件在运行中途消失了（仓库内 .tools 会被清、被扫描器拿走都可能）。临时目录里的
# 副本与仓库生命周期无关，最坏情况也只是"复制过去的那一秒被拦"，而且复制后会**再验一次版本**，
# 验不过就退回原路径（不改变既有语义）。
function Get-WasmBindgenTempCopy([string]$Source, [string]$WantVersion) {
    if ([string]::IsNullOrWhiteSpace($Source)) { return $null }
    try {
        $dir = Join-Path ([IO.Path]::GetTempPath()) ("rustlua-wasm-bindgen\" + $WantVersion)
        $dst = Join-Path $dir "wasm-bindgen.exe"
        # **一律 -Exact**：这里问的是"这份副本本身在不在、能不能跑"，兜底会骗人。
        if (Test-WasmBindgenCli $dst $WantVersion -Exact) { return $dst }
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        Copy-Item -LiteralPath $Source -Destination $dst -Force -ErrorAction Stop
        if (Test-WasmBindgenCli $dst $WantVersion -Exact) { return $dst }
    }
    catch { }
    return $null
}
function Resolve-WasmBindgenCli {
    param([switch]$Provision)

    $want = Get-WasmBindgenCrateVersion
    if (-not $want) {
        throw "cannot read wasm-bindgen version from $cargoLock（crate 版本是 CLI 版本的事实源）"
    }

    $candidates = @()
    if ($env:WASM_BINDGEN) { $candidates += $env:WASM_BINDGEN }
    $candidates += $toolsExe
    $candidates += $vendorCliExe
    $onPath = Get-Command wasm-bindgen -ErrorAction SilentlyContinue
    if ($onPath) { $candidates += $onPath.Source }

    foreach ($cand in $candidates) {
        # 先归一成真实路径，再决定要不要复制：直接返回候选字符串会在 CI 上返回一个
        # Test-Path / `&` 都不认的路径（2026-11-03 的 CommandNotFoundException）。
        $real = Get-RealWasmBindgenPath $cand $want
        if (-not $real) { continue }
        $copy = Get-WasmBindgenTempCopy $real $want
        if ($copy) { return $copy }
        return $real
    }

    # 把"为什么全被否掉"说出来：版本错位是唯一常见原因，沉默会让人以为只是没装。
    $seen = @()
    foreach ($cand in $candidates) {
        $v = Get-WasmBindgenCliVersion $cand
        if ($v) { $seen += ("{0} = {1}" -f $cand, $v) }
    }
    $seenText = 'none'
    if ($seen.Count -gt 0) { $seenText = $seen -join ', ' }
    if (-not $Provision) {
        if ($seen.Count) {
            Write-Host "==> no wasm-bindgen matches crate version $want (saw: $seenText)" -ForegroundColor Yellow
        }
        return $null
    }
    Write-Host "==> no wasm-bindgen matches crate version $want (saw: $seenText); installing the pinned version" -ForegroundColor Yellow
    $installed = Install-WasmBindgenCli $want
    $real = Get-RealWasmBindgenPath $installed $want
    if ($real) {
        $copy = Get-WasmBindgenTempCopy $real $want
        if ($copy) { return $copy }
        return $real
    }
    return $installed
}

# 只有作为脚本直接运行时才走 CLI；被 `. scripts\build\wasm-bindgen.ps1` 点源时只导出函数，
# 不改调用者的输出与退出码。
if ($MyInvocation.InvocationName -ne '.' -and $MyInvocation.InvocationName -ne 'source') {
    $ErrorActionPreference = "Stop"
    $resolved = Resolve-WasmBindgenCli -Provision:(-not $NoInstall)
    if (-not $resolved) {
        if ($PrintPath) { exit 2 }
        throw "wasm-bindgen CLI 解析失败（版本必须与 Cargo.lock 里的 wasm-bindgen 一致）。自动安装：powershell -File scripts\build\wasm-bindgen.ps1"
    }
    if ($PrintPath) { Write-Output $resolved }
    else { Write-Host ("wasm-bindgen = {0} ({1})" -f $resolved, (Get-WasmBindgenCliVersion $resolved)) -ForegroundColor Green }
    exit 0
}
