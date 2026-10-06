# install-git-hooks.ps1（优化项 11）：安装/卸载/体检本地 pre-commit 门。
#
# 它只做一件真正的事：`git config core.hooksPath git-hooks`，让 git 从仓库里那份
# **被跟踪、能 review、能改**的 hook 目录取钩子，而不是各人 `.git/hooks/` 下互不可见的副本。
# 选原生 hook 而不是 cargo-husky / pre-commit 框架的理由记在 [docs/34](../docs/34-质量门与发布清单.md) §3：
# 零新增依赖（本机 Python 有但 pre-commit 没装；cargo-husky 要把第三方 build script
# 塞进 workspace 成员），跨平台，且与仓库里"门都是脚本 + 退出码"的既有口径一致。
# 代价也很明确：装了才算有——所以本脚本自带 -Status 体检，且 CI 用
# scripts/test-git-hooks.ps1 守 hook 自身的判定逻辑。
#
# 用法：
#   powershell -NoProfile -File scripts/install-git-hooks.ps1              # 安装
#   powershell -NoProfile -File scripts/install-git-hooks.ps1 -Clippy full  # 安装 + clippy 与 CI 对齐
#   powershell -NoProfile -File scripts/install-git-hooks.ps1 -Status       # 只体检，不改任何东西
#   powershell -NoProfile -File scripts/install-git-hooks.ps1 -Uninstall    # 取消 core.hooksPath
#
# 退出码：0=成功；1=失败（缺 git / hook 文件不在 / 配置没落地）。

[CmdletBinding()]
param(
    [switch]$Uninstall,
    # 体检模式：只报告现状（hooksPath / 行尾 / 执行位 / clippy 档位 / cargo 可用性），
    # 不写配置、不改文件，可以随手跑。
    [switch]$Status,
    # 顺手把 clippy 档位写进 git config（off|fast|full），省得再记一条命令。
    [ValidateSet('off', 'fast', 'full')]
    [string]$Clippy
)

$ErrorActionPreference = 'Stop'
# 结论行要经 `powershell -File`（5.1，控制台默认 ANSI）显示，中文按 UTF8 无 BOM 输出才不会乱码。
try { [Console]::OutputEncoding = New-Object Text.UTF8Encoding($false) } catch { }

function Get-GitText([string[]]$Arguments) {
    # 统一的 git 调用：吞掉 stderr（git 用 --get 读不到值时既报错也返回非零），只留 stdout。
    & git @Arguments 2>$null | Select-Object -First 1
}

function Invoke-GitQuiet([string[]]$Arguments) {
    # 只看退出码的 git 调用。为什么非要单独一个：PowerShell 5.1 在
    # $ErrorActionPreference='Stop' 下会把原生命令写到 stderr 的行为升级成**终止错误**，
    # 而 `ls-files --error-unmatch`（文件没跟踪）和 `config --unset`（键不存在）都是
    # 正常路径——实测第一次安装就因此 exit 1，hook 明明已经装上了却报失败。
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $null = & git @Arguments 2>$null
        return ($LASTEXITCODE -eq 0)
    }
    finally {
        $ErrorActionPreference = $previous
    }
}

function Invoke-ExternalFile([string]$File, [string[]]$Arguments, [string]$WorkingDirectory) {
    # 自检要读 hook 的判定文字（走 stderr），而 PS 的 2>&1 管道在 5.1 里要么刷一屏红字
    # 要么整条丢掉，所以按进程级文件重定向取回，行为与版本无关。
    $outFile = [IO.Path]::GetTempFileName()
    $errFile = [IO.Path]::GetTempFileName()
    try {
        $quoted = @($Arguments | ForEach-Object { if ($_.ToString() -match '\s') { '"' + $_ + '"' } else { "$_" } })
        $proc = Start-Process -FilePath $File -ArgumentList $quoted -NoNewWindow -Wait -PassThru `
            -WorkingDirectory $WorkingDirectory `
            -RedirectStandardOutput $outFile -RedirectStandardError $errFile
        return @{
            Code = $proc.ExitCode
            Out  = ([IO.File]::ReadAllText($outFile, [Text.Encoding]::UTF8) + "`n" +
                    [IO.File]::ReadAllText($errFile, [Text.Encoding]::UTF8))
        }
    }
    finally {
        Remove-Item -LiteralPath $outFile, $errFile -ErrorAction SilentlyContinue
    }
}

$projectRoot = Split-Path -Parent $PSScriptRoot
$hooksDirName = 'git-hooks'
$hooksDir = Join-Path $projectRoot $hooksDirName

Write-Host '== 安装本地 pre-commit 门 ==' -ForegroundColor Cyan

# --- 前置：git 与仓库根 ---------------------------------------------------
$gitVersion = $null
try { $gitVersion = (& git --version) 2>$null } catch { }
if (-not $gitVersion) {
    Write-Host 'FAIL - 找不到 git，无法安装 hook。' -ForegroundColor Red
    exit 1
}
$repoRoot = (& git -C $projectRoot rev-parse --show-toplevel 2>$null)
if (-not $repoRoot) {
    Write-Host "FAIL - $projectRoot 不是 git 仓库。" -ForegroundColor Red
    exit 1
}
$repoRoot = $repoRoot.Trim()
Write-Host "  repo: $repoRoot"

# --- 卸载 ----------------------------------------------------------------
if ($Uninstall) {
    if (Invoke-GitQuiet @('-C', $repoRoot, 'config', '--unset', 'core.hooksPath')) {
        Write-Host '  core.hooksPath 已取消。'
    }
    else {
        # 没装过也要能安静退出：卸载失败与否不该把脚本炸成红字终止错误。
        Write-Host '  core.hooksPath 本来就没设置（未安装状态）。' -ForegroundColor Yellow
    }
    Write-Host '  注意：取消后 .git/hooks/ 下的历史遗留脚本会重新生效，那些文件不在版本库里、没人 review。' -ForegroundColor Yellow
    Write-Host 'git_hooks_uninstalled' -ForegroundColor Green
    exit 0
}

# --- 体检 / 安装共用的现状采集 --------------------------------------------
$declaredHooksPath = (Get-GitText @('-C', $repoRoot, 'config', '--get', 'core.hooksPath'))
if (-not $declaredHooksPath) { $declaredHooksPath = '' }
$hooksOff = (Get-GitText @('-C', $repoRoot, 'config', '--bool', '--get', 'rustlua.hooks'))
$clippyMode = (Get-GitText @('-C', $repoRoot, 'config', '--get', 'rustlua.clippy'))
if (-not $clippyMode) { $clippyMode = 'fast（默认）' }
# 作用域：默认 commit = 只对**本次提交**负责（与 CI 看的东西一致）。见 docs/34 §3.5。
# 体检必须把它报出来——没人知道存在的开关等于不存在。
$scopeMode = (Get-GitText @('-C', $repoRoot, 'config', '--get', 'rustlua.scope'))
if (-not $scopeMode) { $scopeMode = 'commit（默认：只对本次提交负责）' }
$cargoCmd = Get-Command cargo -ErrorAction SilentlyContinue

$hookFiles = @()
if (Test-Path -LiteralPath $hooksDir) {
    $hookFiles = @(Get-ChildItem -LiteralPath $hooksDir -File | ForEach-Object { $_.Name })
}
# .git/hooks 里的老脚本：core.hooksPath 一旦生效它们就不再执行，但会误导下一个人，
# 所以体检时列出来。
$legacyHooks = @()
$legacyDir = Join-Path $repoRoot '.git\hooks'
if (Test-Path -LiteralPath $legacyDir) {
    $legacyHooks = @(Get-ChildItem -LiteralPath $legacyDir -File | Where-Object { $_.Name -notlike '*.sample' } | ForEach-Object { $_.Name })
}

function Show-State {
    Write-Host '-- 现状 --' -ForegroundColor Cyan
    if ($declaredHooksPath) {
        Write-Host "  core.hooksPath = $declaredHooksPath"
    }
    else {
        Write-Host '  core.hooksPath = (未设置)  ← pre-commit 门当前不生效' -ForegroundColor Yellow
    }
    Write-Host "  hooks 目录 = $hooksDirName/，脚本：$(if ($hookFiles.Count) { $hookFiles -join ', ' } else { '(空)' })"
    Write-Host "  clippy 档位 = $clippyMode"
    Write-Host "  scope 档位 = $scopeMode"
    Write-Host "  rustlua.hooks = $(if ($hooksOff) { $hooksOff } else { '(未设置，即开启)' })"
    Write-Host "  cargo 可用 = $(if ($cargoCmd) { $cargoCmd.Source } else { '否 —— 装了 hook 后碰 Rust 的提交会被判红' })"
    if ($legacyHooks.Count) {
        Write-Host "  .git/hooks 下的遗留脚本（hooksPath 生效后不再执行，建议清掉以免误导）：$($legacyHooks -join ', ')" -ForegroundColor Yellow
    }
}

Show-State

# 行尾体检/修正：CRLF 的 sh 脚本会被解释器报 "bad interpreter: /bin/sh^M"，
# 而 Windows 上 core.autocrlf=true 的克隆很容易踩到（.gitattributes 已钉 eol=lf，
# 但属性只管得住之后的检出，存量工作区仍可能是 CRLF）。
function Sync-HookLineEndings([switch]$Fix) {
    $bad = @()
    foreach ($name in $hookFiles) {
        $path = Join-Path $hooksDir $name
        $bytes = [IO.File]::ReadAllBytes($path)
        if ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF) {
            $bad += "$name（UTF-8 BOM：会让 '#!' 不是文件头，解释器直接选错）"
            continue
        }
        if ($bytes -contains 13) {
            if ($Fix) {
                $fixed = New-Object byte[] $bytes.Length
                $n = 0
                for ($i = 0; $i -lt $bytes.Length; $i++) {
                    # 只在 CR 紧跟 LF 时丢弃 CR；孤立 CR 原样保留，不做猜测性改写。
                    if ($bytes[$i] -eq 13 -and $i + 1 -lt $bytes.Length -and $bytes[$i + 1] -eq 10) { continue }
                    $fixed[$n] = $bytes[$i]
                    $n++
                }
                [IO.File]::WriteAllBytes($path, $fixed[0..($n - 1)])
                Write-Host "  已把 $name 的行尾从 CRLF 修回 LF（sh 解释器不认 CRLF 的 #! 行）。" -ForegroundColor Yellow
            }
            else {
                $bad += "$name（CRLF）"
            }
        }
    }
    return $bad
}

if ($Status) {
    $badOnDisk = Sync-HookLineEndings
    if ($badOnDisk.Count) { Write-Host "  行尾问题：$($badOnDisk -join '; ')" -ForegroundColor Yellow }
    if (-not $cargoCmd) { Write-Host '  提示：PATH 上没有 cargo，碰 Rust 的提交会被 hook 判红。' -ForegroundColor Yellow }
    Write-Host 'git_hooks_status_ok' -ForegroundColor Green
    exit 0
}

# --- 安装 ----------------------------------------------------------------
if (-not (Test-Path -LiteralPath $hooksDir)) {
    Write-Host "FAIL - 缺少 $hooksDirName/ 目录：仓库内容不完整（该目录是 git 跟踪的）。" -ForegroundColor Red
    exit 1
}
if ($hookFiles -notcontains 'pre-commit') {
    Write-Host "FAIL - $hooksDirName/pre-commit 不存在：仓库内容不完整。" -ForegroundColor Red
    exit 1
}

$badOnDisk = Sync-HookLineEndings -Fix:$true
if ($badOnDisk.Count) {
    Write-Host "FAIL - 仍有无法自动修正的 hook 文件：$($badOnDisk -join '; ')" -ForegroundColor Red
    exit 1
}

& git -C $repoRoot config core.hooksPath $hooksDirName
if ($LASTEXITCODE -ne 0) {
    Write-Host "FAIL - git config core.hooksPath 写入失败（exit=$LASTEXITCODE）。" -ForegroundColor Red
    exit 1
}
$after = (Get-GitText @('-C', $repoRoot, 'config', '--get', 'core.hooksPath'))
if ($after -ne $hooksDirName) {
    Write-Host "FAIL - core.hooksPath 期望 $hooksDirName，实际读回 '$after'。" -ForegroundColor Red
    exit 1
}

# 执行位：Windows 不需要，但 macOS/Linux 上 git 会跳过没有 +x 的 hook（只给一行警告，
# 很容易看漏）。索引里的 mode 一旦记成 100755，所有平台都受益；文件未跟踪时只能跳过。
if (Invoke-GitQuiet @('-C', $repoRoot, 'ls-files', '--error-unmatch', "$hooksDirName/pre-commit")) {
    if (Invoke-GitQuiet @('-C', $repoRoot, 'update-index', '--chmod=+x', "$hooksDirName/pre-commit")) {
        Write-Host "  已确保索引里的 $hooksDirName/pre-commit 带执行位（mode 100755，macOS/Linux 才认）。"
    }
}
else {
    Write-Host "  note - $hooksDirName/pre-commit 还没进版本库，执行位要等 git add 之后再跑一次本脚本。" -ForegroundColor Yellow
}

if ($Clippy) {
    & git -C $repoRoot config rustlua.clippy $Clippy
    Write-Host "  rustlua.clippy = $Clippy"
}

Write-Host '-- 自检：真跑一次 hook（cargo 步骤只打印不执行）--' -ForegroundColor Cyan
# 直接执行脚本本体，而不是绕 `git hook run`（2.36 才有，且各版本参数不一致，容易在体检里
# 报出"hook 坏了"的假红）。"git 到底会不会调到它"由 core.hooksPath 写入校验 +
# scripts/test-git-hooks.ps1 里的真实 commit 用例负责，两件事各测各的。
if (Get-Command sh -ErrorAction SilentlyContinue) {
    $env:RUSTLUA_HOOKS_DRY_RUN = '1'
    try {
        $dry = Invoke-ExternalFile 'sh' @("$hooksDirName/pre-commit") $repoRoot
    }
    finally {
        Remove-Item Env:\RUSTLUA_HOOKS_DRY_RUN -ErrorAction SilentlyContinue
    }
    $dryText = $dry.Out
    # 结论行是 hook 自己打的（ok / FAILED / PARTIAL / SKIPPED / note）。读不到就说明脚本
    # 根本没跑起来，最常见的是行尾不是 LF 或 PATH 上没有 sh。
    if ($dryText -notmatch 'pre-commit (ok|FAILED|SKIPPED|PARTIAL|note)|pre-commit ok') {
        Write-Host "FAIL - hook 自检没有产出任何结论行（exit=$($dry.Code)），说明它没能执行起来。原始输出：" -ForegroundColor Red
        Write-Host $dryText
        Write-Host "  常见原因：PATH 上没有 sh（装了 Git Bash 就有），或 hook 文件行尾不是 LF（本脚本会顺手修回 LF）。" -ForegroundColor Yellow
        exit 1
    }
    Write-Host ($dryText.Trim())
    # 自检跑成了 FAILED（例如暂存区里真有格式不合格的东西）不是安装失败：hook 正常工作才会红。
    if ($dry.Code -ne 0) {
        Write-Host '  note - 自检退出码非 0：这是 hook 在报告当前暂存内容没过门，属于它在干活。' -ForegroundColor Yellow
    }
}
else {
    Write-Host '  note - PATH 上没有 sh，跳过自检；commit 时 git 用的是它自带的 bash，hook 仍会执行。' -ForegroundColor Yellow
}
Write-Host 'git_hooks_installed' -ForegroundColor Green
exit 0
