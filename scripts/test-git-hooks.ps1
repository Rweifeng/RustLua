# test-git-hooks.ps1（优化项 11）：本地 pre-commit 门自身的回归测试。
#
# 为什么需要它：hook 只能靠每人记得跑一次 install-git-hooks.ps1，装没装从结果上看不出来；
# 更糟的是 hook **内部判定逻辑**回归了也没有信号——比如 CI 换了 clippy 参数而 hook 没跟上，
# 或者暂存区过滤规则写错导致门变成"永远跳过"（那是"跑过了但其实没测"的最坏形态）。
# 所以这里在临时 git 仓库里**真的执行提交**，用退出码和 HEAD 是否移动来断言拦与放，
# 而不是只 grep 脚本文本。
#
# 覆盖：
#   A. 选路逻辑（dry-run：只断言 hook 决定跑什么，不编译，零成本）
#   B. 真实拦截：行尾空白 / 格式不合格 / 修好后放行 / 逃生门确实绕过 / 缺 cargo 判红
#   C. 与 CI lint job 的命令口径不漂移（从 quality.yml 反查真实命令串比对）
#   D. 仓库自身的 hook 文件形态（LF、无 BOM、shebang）与安装器 -Status 体检
#
# 用法：powershell -NoProfile -File scripts/test-git-hooks.ps1
# 退出码：0=全绿；1=判红；3=环境受限（缺 git / cargo / sh，显式说明，不算通过）。

[CmdletBinding()]
param(
    # 保留临时仓库便于排查（默认跑完即删）。
    [switch]$KeepTemp
)

$ErrorActionPreference = 'Stop'
# 结论行要经 `powershell -File`（5.1，控制台默认 ANSI）显示，UTF8 无 BOM 才不乱码。
try { [Console]::OutputEncoding = New-Object Text.UTF8Encoding($false) } catch { }

$failures = @()
$caseCount = 0

function Assert-True([bool]$Condition, [string]$Message) {
    $script:caseCount++
    if ($Condition) {
        Write-Host "  ok - $Message" -ForegroundColor DarkGray
    }
    else {
        Write-Host "  FAIL - $Message" -ForegroundColor Red
        $script:failures += $Message
    }
}

function Invoke-Native([string]$File, [string[]]$Arguments, [hashtable]$Environment) {
    # 进程级文件重定向，而不是 `& file 2>&1`：PowerShell 5.1 会把原生命令的 stderr 变成
    # ErrorRecord —— 偏好设 Continue 时它既进管道又刷一屏红字，设 SilentlyContinue 时干脆
    # 整条丢掉（hook 的判定文字全在 stderr，实测 16 项断言一起落空）。hook 的输出是这里的
    # 被测对象，必须完整、无歧义地拿到，所以走 Start-Process 的 stdout/stderr 落盘。
    $outFile = [IO.Path]::GetTempFileName()
    $errFile = [IO.Path]::GetTempFileName()
    $restore = @{}
    if ($Environment) {
        foreach ($key in $Environment.Keys) {
            $existing = Get-Item -LiteralPath "Env:$key" -ErrorAction SilentlyContinue
            $restore[$key] = if ($existing) { $existing.Value } else { $null }
            if ($null -eq $Environment[$key]) { Remove-Item -LiteralPath "Env:$key" -ErrorAction SilentlyContinue }
            else { Set-Item -LiteralPath "Env:$key" -Value $Environment[$key] }
        }
    }
    try {
        # Start-Process 不替带空格的参数加引号（本机与 CI 都可能落在带空格路径下），自己钉。
        $quoted = @($Arguments | ForEach-Object {
            if ($_ -match '\s') { '"' + $_ + '"' }
            else { $_ }
        })
        $proc = Start-Process -FilePath $File -ArgumentList $quoted -NoNewWindow -Wait -PassThru `
            -RedirectStandardOutput $outFile -RedirectStandardError $errFile
        $code = $proc.ExitCode
        # hook 用 printf 输出 UTF-8；按 UTF8 读才不会把中文断言目标读成乱码。
        $stdout = [IO.File]::ReadAllText($outFile, [Text.Encoding]::UTF8)
        $stderr = [IO.File]::ReadAllText($errFile, [Text.Encoding]::UTF8)
    }
    finally {
        Remove-Item -LiteralPath $outFile, $errFile -ErrorAction SilentlyContinue
        foreach ($key in $restore.Keys) {
            if ($null -ne $restore[$key]) { Set-Item -LiteralPath "Env:$key" -Value $restore[$key] -ErrorAction SilentlyContinue }
            else { Remove-Item -LiteralPath "Env:$key" -ErrorAction SilentlyContinue }
        }
    }
    return @{ Code = $code; Out = ($stdout + "`n" + $stderr) }
}

# --- 前置探测：三项缺一，这套测试就什么都测不到，按环境受限显式跳过 ---------
$gitCmd = Get-Command git -ErrorAction SilentlyContinue
$cargoCmd = Get-Command cargo -ErrorAction SilentlyContinue
$shCmd = Get-Command sh -ErrorAction SilentlyContinue
if (-not $gitCmd -or -not $cargoCmd -or -not $shCmd) {
    $missing = @()
    if (-not $gitCmd) { $missing += 'git' }
    if (-not $cargoCmd) { $missing += 'cargo' }
    if (-not $shCmd) { $missing += 'sh' }
    Write-Host "git_hooks_skipped reason=missing_$(($missing -join '+')) exit=3" -ForegroundColor Yellow
    Write-Host '  缺工具时说"通过"就是自欺；口径见 docs/34 §3。'
    exit 3
}

$projectRoot = Split-Path -Parent $PSScriptRoot
$repoHook = Join-Path $projectRoot 'git-hooks\pre-commit'
$fullClippy = 'cargo clippy --workspace --all-targets --all-features -- -D warnings'
$fastClippy = 'cargo clippy --workspace --all-targets -- -D warnings'
# 全部用绝对路径调用：B6 会临时改写 PATH，用命令名会连命令本身都解析不到（PowerShell 的
# 命令解析也走 PATH），那会让"缺 cargo"的用例假红在别处。
$git = $gitCmd.Source
$sh = $shCmd.Source
$powershellCmd = Get-Command powershell -ErrorAction SilentlyContinue
$powershell = if ($powershellCmd) { $powershellCmd.Source } else { 'powershell' }

$fakeManifest = @'
[package]
name = "fake"
version = "0.1.0"
edition = "2021"
'@
# 故意不合格的行文（rustfmt 稳定判红，不依赖具体版式偏好）。
$uglyRs = "pub   fn   value( ) ->i32{42}`n"
$cleanRs = "pub fn value() -> i32 {`n    42`n}`n"

function New-HookRepo {
    # 一次性 git 仓库：带一个假 workspace（engine/ + 成员 crate `fake`）和一份 hook 脚本，
    # 目录布局与真实仓库同构，所以 hook 里的路径推导是被真实执行的而不是摆设。
    $root = Join-Path $env:TEMP ('git-hooks-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Force -Path (Join-Path $root 'git-hooks') | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $root 'engine\crates\fake\src') | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $root 'docs') | Out-Null
    Copy-Item -LiteralPath $repoHook -Destination (Join-Path $root 'git-hooks\pre-commit') -Force
    Set-Content -LiteralPath (Join-Path $root 'engine\Cargo.toml') -Encoding UTF8 -Value @(
        '[workspace]', 'resolver = "2"', 'members = ["crates/fake"]')
    Set-Content -LiteralPath (Join-Path $root 'engine\crates\fake\Cargo.toml') -Encoding UTF8 -Value $fakeManifest
    Set-Content -LiteralPath (Join-Path $root 'engine\crates\fake\src\lib.rs') -Encoding UTF8 -Value $cleanRs
    Set-Content -LiteralPath (Join-Path $root 'docs\note.md') -Encoding UTF8 -Value '# note'
    $null = Invoke-Native $git @('-C', $root, 'init', '--quiet')
    $null = Invoke-Native $git @('-C', $root, 'config', 'user.email', 'hook-test@example.com')
    $null = Invoke-Native $git @('-C', $root, 'config', 'user.name', 'hook-test')
    $null = Invoke-Native $git @('-C', $root, 'config', 'core.hooksPath', 'git-hooks')
    $null = Invoke-Native $git @('-C', $root, 'add', '--all')
    # 基线提交走 --no-verify：此时"提交能成功"是前置条件，不是被测结论。
    $null = Invoke-Native $git @('-C', $root, 'commit', '--quiet', '--no-verify', '-m', 'base')
    return $root
}

function Set-RepoFile([string]$Root, [string]$Relative, [string]$Content) {
    $path = Join-Path $Root $Relative
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $path) | Out-Null
    # 固定 LF + UTF8 无 BOM：不让本机 autocrlf 混进"行尾空白"这种行尾相关的用例。
    [IO.File]::WriteAllText($path, $Content, (New-Object Text.UTF8Encoding($false)))
}

function Set-ClippyMode([string]$Root, [string]$Mode) {
    $null = Invoke-Native $git @('-C', $Root, 'config', 'rustlua.clippy', $Mode)
}

function Add-All([string]$Root) {
    $null = Invoke-Native $git @('-C', $Root, 'add', '--all')
}

function Commit-Hook([string]$Root, [hashtable]$Environment, [string]$Message = 'wip') {
    return Invoke-Native $git @('-C', $Root, 'commit', '--quiet', '-m', $Message) -Environment $Environment
}

function Commit-Staged([string]$Root, [hashtable]$Environment) {
    Add-All $Root
    return Commit-Hook $Root $Environment
}

function Get-Head([string]$Root) {
    (Invoke-Native $git @('-C', $Root, 'rev-parse', '--short', 'HEAD')).Out.Trim()
}

Write-Host '== A. 选路逻辑（dry-run：只断言 hook 决定跑什么，不真的编译）==' -ForegroundColor Cyan
$repo = New-HookRepo

# A1 只碰文档 → 整段 fmt/clippy 跳过，一条 cargo 都不该出现。
Set-RepoFile $repo 'docs\note.md' "# note`n`n改文档。`n"
$r = Commit-Staged $repo @{ 'RUSTLUA_HOOKS_DRY_RUN' = '1' }
Assert-True ($r.Code -eq 0) '只碰 docs 的提交放行'
Assert-True ($r.Out -match 'fmt\+clippy 跳过') 'docs-only 明确打印跳过原因'
Assert-True ($r.Out -notmatch 'hook:run cargo') 'docs-only 一条 cargo 都不跑'

# A2 碰 .rs → fmt 全量 + clippy fast（不带 --all-features）。
Set-RepoFile $repo 'engine\crates\fake\src\lib.rs' ($cleanRs + "// A2`n")
$r = Commit-Staged $repo @{ 'RUSTLUA_HOOKS_DRY_RUN' = '1' }
Assert-True ($r.Out -match 'hook:run cargo fmt --all -- --check') 'Rust 改动触发 cargo fmt --all -- --check'
Assert-True ($r.Out -match [regex]::Escape("hook:run $fastClippy")) "默认 fast 档：$fastClippy"
Assert-True ($r.Out -notmatch 'hook:run .*--all-features') 'fast 档不带 --all-features（本机实测冷缓存 50.9s vs 热 9.2s）'

# A3 full 档与 CI 逐字对齐；off 档只跑 fmt 且结论按 PARTIAL 报。
Set-ClippyMode $repo 'full'
Set-RepoFile $repo 'engine\crates\fake\src\lib.rs' ($cleanRs + "// A3`n")
$r = Commit-Staged $repo @{ 'RUSTLUA_HOOKS_DRY_RUN' = '1' }
Assert-True ($r.Out -match [regex]::Escape("hook:run $fullClippy")) 'rustlua.clippy=full 时与 CI 命令逐字一致'
Set-ClippyMode $repo 'off'
Set-RepoFile $repo 'engine\crates\fake\src\lib.rs' ($cleanRs + "// A3b`n")
$r = Commit-Staged $repo @{ 'RUSTLUA_HOOKS_DRY_RUN' = '1' }
Assert-True ($r.Out -notmatch 'hook:run cargo clippy') 'rustlua.clippy=off 时不跑 clippy'
Assert-True ($r.Out -match 'PARTIAL') 'clippy=off 的结论是 PARTIAL，不冒充 ok'

# A4 非法档位回落到 fast 并报警，而不是静默降级。
Set-ClippyMode $repo 'ful'
Set-RepoFile $repo 'engine\crates\fake\src\lib.rs' ($cleanRs + "// A4`n")
$r = Commit-Staged $repo @{ 'RUSTLUA_HOOKS_DRY_RUN' = '1' }
Assert-True ($r.Out -match '不是合法值') '非法 clippy 档位显式报警'
Assert-True ($r.Out -match [regex]::Escape("hook:run $fastClippy")) '非法档位回落 fast'
Set-ClippyMode $repo 'off'

# A5 只改清单（无 .rs）也算碰 Rust：feature 变化会改 clippy 的结论集。
Set-RepoFile $repo 'engine\crates\fake\Cargo.toml' ($fakeManifest + "`n[dependencies]`n")
$r = Commit-Staged $repo @{ 'RUSTLUA_HOOKS_DRY_RUN' = '1' }
Assert-True ($r.Out -match 'hook:run cargo fmt') '清单改动（无 .rs）同样触发门'
$null = Invoke-Native $git @('-C', $repo, 'checkout', '--quiet', '--', 'engine/crates/fake/Cargo.toml')

# A6 暂存内容与工作区不一致时，必须把"cargo 看的是工作区"说在前面。
Set-RepoFile $repo 'engine\crates\fake\src\lib.rs' ($cleanRs + "// A6 staged`n")
Add-All $repo
Set-RepoFile $repo 'engine\crates\fake\src\lib.rs' ($cleanRs + "// A6 staged + 工作区又改了一行，没 add`n")
$r = Commit-Hook $repo @{ 'RUSTLUA_HOOKS_DRY_RUN' = '1' }
Assert-True ($r.Out -match 'note：下列文件的工作区内容与暂存区不同') '暂存/工作区漂移时打印提示'
Assert-True ($r.Out -match 'engine/crates/fake/src/lib\.rs') '漂移提示点到具体文件'

# A7 非法的 scope 值回落 commit 并报警，而不是静默降级（与 A4 的 clippy 档位同口径）。
$null = Invoke-Native $git @('-C', $repo, 'config', 'rustlua.scope', 'bogus')
Set-RepoFile $repo 'engine\crates\fake\src\lib.rs' ($cleanRs + "// A7`n")
$r = Commit-Staged $repo @{ 'RUSTLUA_HOOKS_DRY_RUN' = '1' }
Assert-True ($r.Out -match 'rustlua\.scope=bogus 不是合法值') '非法 scope 值显式报警'
$null = Invoke-Native $git @('-C', $repo, 'config', '--unset', 'rustlua.scope')

Remove-Item -LiteralPath $repo -Recurse -Force

Write-Host '== B. 真拦（不 dry-run：退出码与 HEAD 是否移动才算数）==' -ForegroundColor Cyan
# 这一段只跑 fmt（clippy=off）：临时 crate 零依赖，秒级完成，不产生本工程编译压力。
$repo = New-HookRepo
Set-ClippyMode $repo 'off'
$base = Get-Head $repo

# B1 行尾空白 → 拦下，且不留半提交。
Set-RepoFile $repo 'docs\note.md' "# note   `n"
$r = Commit-Staged $repo
Assert-True ($r.Code -ne 0) '行尾空白让提交失败'
Assert-True ($r.Out -match '行尾空白或冲突标记') '行尾空白给出可读原因'
Assert-True ((Get-Head $repo) -eq $base) '被拦下后 HEAD 未移动（没有半提交状态）'

# B2 Rust 格式不合格 → 拦下，并把修复命令写进输出。
Set-RepoFile $repo 'docs\note.md' "# note`n"
Set-RepoFile $repo 'engine\crates\fake\src\lib.rs' $uglyRs
$r = Commit-Staged $repo
Assert-True ($r.Code -ne 0) '格式不合格让提交失败'
Assert-True ($r.Out -match '格式检查不通过') '格式失败给出定位与修复命令'
Assert-True ($r.Out -match 'cargo fmt --all') '修复命令原样可复制'
Assert-True ((Get-Head $repo) -eq $base) '格式失败后 HEAD 未移动'

# B3 照提示修好 → 同一份改动放行（证明门不会把人焊死在里面）。
Set-RepoFile $repo 'engine\crates\fake\src\lib.rs' $cleanRs
$r = Commit-Staged $repo
Assert-True ($r.Code -eq 0) '修好格式后同一份改动放行'
Assert-True ((Get-Head $repo) -ne $base) '放行后 HEAD 前进'

# B4 逃生门确实绕过：暂存一份**不合格**的 Rust 改动，加 skip 变量后放行（措辞按"跳过"报，
# 与"通过"严格区分——绕过不是绿灯）。
Set-RepoFile $repo 'engine\crates\fake\src\lib.rs' $uglyRs
$null = Invoke-Native $git @('-C', $repo, 'add', 'engine/crates/fake/src/lib.rs')
$base = Get-Head $repo
$r = Commit-Hook $repo @{ 'RUSTLUA_SKIP_PRECOMMIT' = '1' }
Assert-True ($r.Code -eq 0) 'RUSTLUA_SKIP_PRECOMMIT=1 放行'
Assert-True ($r.Out -match 'SKIPPED') '跳过时打印 SKIPPED 而不是 ok'
Assert-True ((Get-Head $repo) -ne $base) '跳过确实完成了提交（不是假通过）'

# B5 rustlua.hooks=false 同样绕过（GUI 客户端 PATH 里没有 cargo 时的正解）。
Set-RepoFile $repo 'engine\crates\fake\src\lib.rs' ($uglyRs + "// 又一次`n")
$null = Invoke-Native $git @('-C', $repo, 'add', 'engine/crates/fake/src/lib.rs')
$null = Invoke-Native $git @('-C', $repo, 'config', 'rustlua.hooks', 'false')
$r = Commit-Hook $repo
Assert-True ($r.Code -eq 0) 'rustlua.hooks=false 放行'
Assert-True ($r.Out -match 'SKIPPED') '永久关闭也按"跳过"措辞报告'
$null = Invoke-Native $git @('-C', $repo, 'config', 'rustlua.hooks', 'true')

# B6 PATH 里没有 cargo → 碰 Rust 的提交判红，而不是静默放行（跑不了≠过了）。
# 构造办法：从当前 PATH 里剔掉所有含 cargo.exe 的目录，其余原样保留——这样 git/sh/date/grep
# 都还在（hook 自己也要用它们），只有 cargo 消失。前提不成立就明确跳过，不猜。
$keptDirs = @()
$droppedDirs = @()
foreach ($dir in ($env:PATH -split ';' | Where-Object { $_ })) {
    if (Test-Path -LiteralPath (Join-Path $dir 'cargo.exe')) { $droppedDirs += $dir }
    else { $keptDirs += $dir }
}
$barePath = ($keptDirs -join ';')
$probe = Invoke-Native $sh @('-c', 'command -v git >/dev/null && command -v grep >/dev/null && command -v date >/dev/null && ! command -v cargo') -Environment @{ 'PATH' = $barePath }
if ($probe.Code -ne 0 -or $droppedDirs.Count -eq 0) {
    Write-Host '  note - 无法构造"有 git/sh 但无 cargo"的 PATH，B6 跳过：不算通过也不算失败' -ForegroundColor Yellow
    $script:caseCount++
}
else {
    Write-Host "  note - B6 从 PATH 暂时移除：$($droppedDirs -join ', ')" -ForegroundColor DarkGray
    # 先把 B5 留下的脏工作区收拾干净，再只暂存一份合格改动。
    $null = Invoke-Native $git @('-C', $repo, 'checkout', '--quiet', '--', 'engine/crates/fake/src/lib.rs')
    Set-RepoFile $repo 'engine\crates\fake\src\lib.rs' ($cleanRs + "// B6`n")
    Add-All $repo
    $r = Commit-Hook $repo @{ 'PATH' = $barePath }
    Assert-True ($r.Code -ne 0) '缺 cargo 时碰 Rust 的提交判红'
    Assert-True ($r.Out -match '找不到 cargo') '缺 cargo 的原因写在输出里'
}

# B7 假红消除：**提交之外**的未跟踪未格式化 .rs（= 另一个会话的在途活）不得挡住我的提交。
# 用 examples/ 下的文件而不是 src/scratch.rs：`cargo fmt --all` 只看**目标的**文件，
# 摆在 src/ 下但没被 lib.rs 引用的 .rs 根本不会被检查——那会让这条用例空转。
$null = Invoke-Native $git @('-C', $repo, 'reset', '--quiet', '--hard', 'HEAD')
Set-RepoFile $repo 'engine\crates\fake\examples\scratch.rs' $uglyRs
Set-RepoFile $repo 'engine\crates\fake\src\lib.rs' ($cleanRs + "// B7`n")
$null = Invoke-Native $git @('-C', $repo, 'add', 'engine/crates/fake/src/lib.rs')
$base = Get-Head $repo
$r = Commit-Hook $repo
Assert-True ($r.Code -eq 0) '提交之外的未跟踪未格式化 .rs 不再挡住提交（假红已消除）'
Assert-True ($r.Out -match '不在本次提交里') '非阻塞情形点名说明失败落在提交之外'
Assert-True ($r.Out -match 'PARTIAL') '非阻塞的结论按 PARTIAL 措辞，不冒充 ok'
Assert-True ((Get-Head $repo) -ne $base) '放行确实完成了提交（不是假通过）'

# B8 旧口径可恢复：同一场景下 scope=worktree 必须重新拦住（证明开关真的起作用）。
Set-RepoFile $repo 'engine\crates\fake\src\lib.rs' ($cleanRs + "// B8`n")
$null = Invoke-Native $git @('-C', $repo, 'add', 'engine/crates/fake/src/lib.rs')
$null = Invoke-Native $git @('-C', $repo, 'config', 'rustlua.scope', 'worktree')
$base = Get-Head $repo
$r = Commit-Hook $repo
Assert-True ($r.Code -ne 0) 'rustlua.scope=worktree 时工作树里的失败重新拦提交'
Assert-True ((Get-Head $repo) -eq $base) '被拦下后 HEAD 未移动'
$null = Invoke-Native $git @('-C', $repo, 'config', '--unset', 'rustlua.scope')

# B9 反向保护：**暂存区自己**的未格式化 .rs 必须判红，且不许被归到"提交之外"混过去。
# 场景同时摆一份提交之外的失败，用来验证分类器真的在分辨，而不是"一有失败就放行"。
Set-RepoFile $repo 'engine\crates\fake\src\lib.rs' ($cleanRs + "// B9 clean`n")
Set-RepoFile $repo 'engine\crates\fake\examples\ugly_staged.rs' $uglyRs
$null = Invoke-Native $git @('-C', $repo, 'add', 'engine/crates/fake/src/lib.rs', 'engine/crates/fake/examples/ugly_staged.rs')
$base = Get-Head $repo
$r = Commit-Hook $repo
Assert-True ($r.Code -ne 0) '暂存区自己的未格式化 .rs 仍然判红'
Assert-True ($r.Out -match '失败文件在本次提交里') '判红时说明失败文件就在本次提交里'
Assert-True ($r.Out -match 'ugly_staged\.rs') '判红时点到具体文件'
Assert-True ((Get-Head $repo) -eq $base) '判红后 HEAD 未移动'
$null = Invoke-Native $git @('-C', $repo, 'rm', '--cached', '--quiet', 'engine/crates/fake/examples/ugly_staged.rs')

# B10 工具前置的**依据**（把前提钉住，而不是假定它）。
# 实测：git 执行 hook 时会把它自己的 usr/bin 放进 PATH 头部（探针读到 `/usr/bin/grep`，
# PATH 里出现 `:/usr/bin:`），所以"缺 grep/sed/tr"这种情形**在真实提交里构造不出来**——
# 先前那版 B10 想构造它，结果红在了别处（`git add --all` 把 B7/B9 残留的未跟踪脏文件一起暂存了）。
# 于是改成钉住前提本身：哪天 git 不再自带这些工具，这条会先红并指向真正的原因。
# hook 里的工具前置（0a 段）作为纵深防御留着——手工 `sh git-hooks/pre-commit` 时它真的会红。
$keptDirs = @()
$droppedDirs = @()
foreach ($dir in ($env:PATH -split ';' | Where-Object { $_ })) {
    $unixHere = @('grep.exe', 'sed.exe', 'tr.exe') | Where-Object { Test-Path -LiteralPath (Join-Path $dir $_) }
    if ($unixHere) { $droppedDirs += $dir } else { $keptDirs += $dir }
}
$noUnixPath = ($keptDirs -join ';')
$probeRepo = Join-Path $env:TEMP ('git-hooks-path-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force -Path (Join-Path $probeRepo '.githooks') | Out-Null
[IO.File]::WriteAllText((Join-Path $probeRepo '.githooks\pre-commit'),
    "#!/bin/sh`nprintf 'tools: %s %s %s`n' `"`$(command -v grep)`" `"`$(command -v sed)`" `"`$(command -v tr)`"`n",
    (New-Object Text.UTF8Encoding($false)))
$null = Invoke-Native $git @('-C', $probeRepo, 'init', '--quiet')
$null = Invoke-Native $git @('-C', $probeRepo, 'config', 'user.email', 'hook-test@example.com')
$null = Invoke-Native $git @('-C', $probeRepo, 'config', 'user.name', 'hook-test')
$null = Invoke-Native $git @('-C', $probeRepo, 'config', 'core.hooksPath', '.githooks')
Set-Content -LiteralPath (Join-Path $probeRepo 'x.txt') -Value 'x' -Encoding UTF8
$null = Invoke-Native $git @('-C', $probeRepo, 'add', '--all')
$r = Invoke-Native $git @('-C', $probeRepo, 'commit', '--quiet', '-m', 'probe') -Environment @{ 'PATH' = $noUnixPath }
Assert-True ($r.Out -match 'tools: .*grep.*sed.*tr') 'git 执行 hook 时自带 grep/sed/tr（hook 工具前置的依据；这条红了说明该前提变了）'
Remove-Item -LiteralPath $probeRepo -Recurse -Force

# B11 反向保护：**本次提交自己**的 clippy 报错必须判红。
# 这一条来自实测的假阴性：`captured_names_staged_file` 拿 git 的正斜杠暂存路径去比 cargo 在 Windows 上
# 输出的**反斜杠**诊断路径，永远比不上 → clippy 的失败一律被归成"提交之外"，clippy 这一半等于没拦过。
# 造法：先清干净残留的未跟踪文件（`reset --hard` 不动它们；B7/B9/B10 留下的脏 .rs 会被 `add --all`
# 一起暂存，于是红在 fmt 而不是 clippy），再暂存一份带告警的 lib.rs，最后把 clippy 档位切到 fast。
Set-ClippyMode $repo 'fast'
$null = Invoke-Native $git @('-C', $repo, 'reset', '--quiet', '--hard', 'HEAD')
$null = Invoke-Native $git @('-C', $repo, 'clean', '--quiet', '-fd')
Set-RepoFile $repo 'engine\crates\fake\src\lib.rs' "pub fn value() -> i32 {`n    let unused = 1;`n    42`n}`n"
Add-All $repo
$base = Get-Head $repo
$r = Commit-Hook $repo
Assert-True ($r.Code -ne 0) '本次提交自己的 clippy 报错必须判红'
Assert-True ($r.Out -match 'clippy 判红') '判红原因指向 clippy 而不是别处'
Assert-True ((Get-Head $repo) -eq $base) '判红后 HEAD 未移动'
Set-ClippyMode $repo 'off'

Remove-Item -LiteralPath $repo -Recurse -Force

Write-Host '== C. 与 CI lint job 的命令口径不漂移 ==' -ForegroundColor Cyan
$workflow = Get-Content -LiteralPath (Join-Path $projectRoot '.github\workflows\quality.yml') -Raw
$fmtMatches = [regex]::Matches($workflow, '(?m)^\s*run:\s*(cargo fmt[^\r\n]*)')
$clippyMatches = [regex]::Matches($workflow, '(?m)^\s*run:\s*(cargo clippy[^\r\n]*)')
Assert-True ($fmtMatches.Count -ge 1) '能从 quality.yml 读到 CI 的 fmt 命令'
Assert-True ($clippyMatches.Count -ge 1) '能从 quality.yml 读到 CI 的 clippy 命令'
$clippyDrift = @($clippyMatches | ForEach-Object { $_.Groups[1].Value.Trim() } | Where-Object { $_ -ne $fullClippy })
Assert-True ($clippyDrift.Count -eq 0) "CI 的 clippy 命令仍全是 '$fullClippy'（变了要同步 hook 的 full 档并更新本断言）"
$fmtDrift = @($fmtMatches | ForEach-Object { $_.Groups[1].Value.Trim() } | Where-Object { $_ -ne 'cargo fmt --all -- --check' })
Assert-True ($fmtDrift.Count -eq 0) 'CI 的 fmt 命令仍是 cargo fmt --all -- --check'

$hookText = Get-Content -LiteralPath $repoHook -Raw
Assert-True ($hookText -match [regex]::Escape('clippy --workspace --all-targets --all-features -- -D warnings')) 'hook 的 full 档与 CI 逐字一致'
Assert-True ($hookText -match [regex]::Escape('fmt --all -- --check')) 'hook 的 fmt 命令与 CI 逐字一致'

Write-Host '== D. 真实仓库里的 hook 文件形态与安装器体检 ==' -ForegroundColor Cyan
$bytes = [IO.File]::ReadAllBytes($repoHook)
Assert-True ($bytes.Length -gt 100) 'git-hooks/pre-commit 非空'
Assert-True ($bytes[0] -ne 0xEF) 'hook 无 UTF-8 BOM（BOM 会让 "#!" 不再是文件头）'
$hasCR = $false
foreach ($b in $bytes) { if ($b -eq 13) { $hasCR = $true; break } }
Assert-True (-not $hasCR) 'hook 是 LF 行尾（CRLF 会让 sh 报 bad interpreter: /bin/sh^M）'
Assert-True ($hookText.StartsWith('#!/bin/sh')) 'hook 有 #!/bin/sh 头'
$gitattributes = Join-Path $projectRoot '.gitattributes'
$attrs = if (Test-Path -LiteralPath $gitattributes) { Get-Content -LiteralPath $gitattributes -Raw } else { '' }
Assert-True ($attrs -match 'git-hooks/\*\s+text\s+eol=lf') '.gitattributes 把 git-hooks/* 钉成 LF'
# 先记下本机现状：-Status 只读，跑完必须一字不变。
$hooksPathBefore = (Invoke-Native $git @('-C', $projectRoot, 'config', '--get', 'core.hooksPath')).Out.Trim()
$clippyBefore = (Invoke-Native $git @('-C', $projectRoot, 'config', '--get', 'rustlua.clippy')).Out.Trim()
$doctor = Invoke-Native $powershell @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path $projectRoot 'scripts\install-git-hooks.ps1'), '-Status')
Assert-True ($doctor.Code -eq 0) "install-git-hooks.ps1 -Status 体检可跑（code=$($doctor.Code)）"
Assert-True ($doctor.Out -match 'git_hooks_status_ok') '体检产出结论行'
Assert-True ($doctor.Out -match 'core\.hooksPath') '体检报告 hooksPath 现状'
# 体检必须是只读的：跑完 core.hooksPath 与 rustlua.clippy 都不该被它自己改掉。
$hooksPathNow = (Invoke-Native $git @('-C', $projectRoot, 'config', '--get', 'core.hooksPath')).Out.Trim()
$clippyNow = (Invoke-Native $git @('-C', $projectRoot, 'config', '--get', 'rustlua.clippy')).Out.Trim()
Assert-True ($hooksPathNow -eq $hooksPathBefore) '体检不改 core.hooksPath（只读性）'
Assert-True ($clippyNow -eq $clippyBefore) '体检不改 rustlua.clippy（只读性）'

if (-not $KeepTemp) {
    Get-ChildItem -LiteralPath $env:TEMP -Directory -Filter 'git-hooks-*' -ErrorAction SilentlyContinue |
        Where-Object { $_.LastWriteTime -lt (Get-Date).AddHours(-2) } |
        ForEach-Object { Remove-Item -LiteralPath $_.FullName -Recurse -Force -ErrorAction SilentlyContinue }
}

if ($failures.Count -gt 0) {
    Write-Host "`ngit_hooks test FAILED: $($failures.Count) 项 / cases=$caseCount" -ForegroundColor Red
    foreach ($f in $failures) { Write-Host "  - $f" -ForegroundColor Red }
    exit 1
}
Write-Host "`ngit_hooks_ok cases=$caseCount" -ForegroundColor Green
exit 0
