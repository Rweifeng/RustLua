# release-atomic.ps1（M20-03）：发布目录的临时构建、原子替换与失败回滚。
#
# 设计：发布内容先在 `<release>/.staging-<时间戳>` 里完整构建与自检，全部成功
# 后才交换到最终目录；交换期间旧版本整体移到 `<release>/.backup-<时间戳>`，
# 任一步失败即回滚（新内容退回 staging、旧内容放回原位），因此发布目录**不会**
# 处于半更新状态。交换是同卷目录重命名（原子），不改变既有发布目录布局。
#
# 该模块是纯文件系统逻辑，可独立测试（见 scripts/test/test-release-atomic.ps1）。
# 注意：本文件**不**启用 Set-StrictMode —— 它会被 package.ps1 点源，不应改变
# 调用方的变量解析语义。

$script:StagingPrefix = ".staging-"
$script:BackupPrefix = ".backup-"

function Get-ReleaseStagingRoot {
    <#
    .SYNOPSIS
    为一个发布根目录生成 staging 路径（同卷、同级，确保重命名原子）。
    #>
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$FinalRoot)
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $suffix = [guid]::NewGuid().ToString("N").Substring(0, 6)
    return (Join-Path $FinalRoot ("{0}{1}-{2}" -f $script:StagingPrefix, $stamp, $suffix))
}

function New-ReleaseStaging {
    <#
    .SYNOPSIS
    创建 staging 目录（并确保最终目录存在）。
    #>
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$FinalRoot)
    New-Item -ItemType Directory -Force -Path $FinalRoot | Out-Null
    $stage = Get-ReleaseStagingRoot -FinalRoot $FinalRoot
    New-Item -ItemType Directory -Force -Path $stage | Out-Null
    return $stage
}

function Clear-StaleReleaseStaging {
    <#
    .SYNOPSIS
    清理遗留的 staging/backup 目录（中断留下的），只按前缀匹配，绝不触碰发布内容。
    .PARAMETER MaxAgeHours
    超过该时长才清理（默认 24 小时），避免误删正在进行的发布。
    #>
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$FinalRoot,
        [double]$MaxAgeHours = 24
    )
    $removed = @()
    if (-not (Test-Path -LiteralPath $FinalRoot)) {
        return $removed
    }
    $cutoff = (Get-Date).AddHours(-$MaxAgeHours)
    foreach ($entry in Get-ChildItem -LiteralPath $FinalRoot -Force -Directory) {
        $isStaging = $entry.Name.StartsWith($script:StagingPrefix)
        $isBackup = $entry.Name.StartsWith($script:BackupPrefix)
        if (-not ($isStaging -or $isBackup)) { continue }
        if ($entry.LastWriteTime -gt $cutoff) { continue }
        Remove-Item -LiteralPath $entry.FullName -Recurse -Force -ErrorAction SilentlyContinue
        if (-not (Test-Path -LiteralPath $entry.FullName)) {
            $removed += $entry.FullName
        }
    }
    return $removed
}

function Undo-ReleaseStaging {
    <#
    .SYNOPSIS
    发布失败时丢弃 staging（最终目录从未被改动，因此无需回滚）。
    #>
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$StageRoot
    )
    if (Test-Path -LiteralPath $StageRoot) {
        Remove-Item -LiteralPath $StageRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
    return -not (Test-Path -LiteralPath $StageRoot)
}

function Complete-ReleaseSwap {
    <#
    .SYNOPSIS
    原子替换：旧内容移入 backup → staging 内容移入最终目录 → 清理 backup/staging。
    任一步失败则回滚（新内容退回 staging、旧内容放回最终目录）并以明确原因抛出。
    .OUTPUTS
    交换摘要（Hashtable：MovedIn / MovedAside / BackupRoot）。
    #>
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$FinalRoot,
        [Parameter(Mandatory)][string]$StageRoot
    )
    if (-not (Test-Path -LiteralPath $StageRoot)) {
        throw "release swap failed: staging directory is missing: $StageRoot"
    }
    New-Item -ItemType Directory -Force -Path $FinalRoot | Out-Null
    # **发布目录被占用预检**（2026-10-15 实测踩到）：微信开发者工具开着本工程时会**抱住**发布目录，
    # 于是下面的原子交换报 "The process cannot access the file because it is being used by another
    # process"，然后**静默回滚**并留下 .staging-*。表面现象是"appid 没写进去 / 产物没更新"，
    # 真正的原因在几屏之外 —— 那次为它花了半小时。这里在动任何文件之前就把它变成一条具名错误。
    #
    # 只报**命令行里真的提到这个发布目录**的进程：否则"开发者工具开着别的工程"会被误判成占用。
    # 预检必须在创建 backup 之前：目录被占用时直接以统一错误契约抛出，
    # 不能留下空的 .backup-*（test-release-atomic.ps1 的目标 2 钉住这一点）。
    try {
        Assert-ReleaseDirNotLocked -FinalRoot $FinalRoot
    } catch {
        throw "release swap failed: $($_.Exception.Message)"
    }
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $backupRoot = Join-Path $FinalRoot ("{0}{1}-{2}" -f $script:BackupPrefix, $stamp, [guid]::NewGuid().ToString("N").Substring(0, 6))
    New-Item -ItemType Directory -Force -Path $backupRoot | Out-Null

    $movedAside = New-Object System.Collections.ArrayList
    $movedIn = New-Object System.Collections.ArrayList
    try {
        # 旧内容整体移到 backup（跳过 staging/backup 自身）。
        foreach ($entry in Get-ChildItem -LiteralPath $FinalRoot -Force) {
            if ($entry.Name.StartsWith($script:StagingPrefix) -or $entry.Name.StartsWith($script:BackupPrefix)) {
                continue
            }
            Move-Item -LiteralPath $entry.FullName -Destination (Join-Path $backupRoot $entry.Name) -Force -ErrorAction Stop
            [void]$movedAside.Add($entry.Name)
        }
        # 新内容从 staging 移入最终目录。
        foreach ($entry in Get-ChildItem -LiteralPath $StageRoot -Force) {
            Move-Item -LiteralPath $entry.FullName -Destination (Join-Path $FinalRoot $entry.Name) -Force -ErrorAction Stop
            [void]$movedIn.Add($entry.Name)
        }
    }
    catch {
        $reason = $_.Exception.Message
        Write-Warning "==> 发布交换失败，正在回滚：$reason"
        foreach ($name in $movedIn) {
            $from = Join-Path $FinalRoot $name
            if (Test-Path -LiteralPath $from) {
                Move-Item -LiteralPath $from -Destination (Join-Path $StageRoot $name) -Force -ErrorAction SilentlyContinue
            }
        }
        foreach ($name in $movedAside) {
            $from = Join-Path $backupRoot $name
            if (Test-Path -LiteralPath $from) {
                Move-Item -LiteralPath $from -Destination (Join-Path $FinalRoot $name) -Force -ErrorAction SilentlyContinue
            }
        }
        Remove-Item -LiteralPath $backupRoot -Recurse -Force -ErrorAction SilentlyContinue
        # 交换失败时补一条**可能持有者**：预检靠命令行匹配，匹配不到时（IDE 主进程命令行常常不含工程路径）
        # 这条兜底至少把"先看看是不是开发者工具开着"这句话送到操作者面前。
        try {
            $suspects = @(Get-Process -Name 'wechatdevtools', '微信开发者工具', 'wxfilewatcher', 'wxfilewatcher_x64' -ErrorAction SilentlyContinue |
                ForEach-Object { "{0}(pid={1})" -f $_.ProcessName, $_.Id })
            if ($suspects.Count -gt 0) {
                $reason = "$reason（检测到可能持有目录的进程：$($suspects -join ', ') —— 开发者工具开着本工程时先关掉再打包）"
            }
        } catch { }        throw "release swap failed (old release restored at $FinalRoot): $reason"
    }

    Remove-Item -LiteralPath $backupRoot -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $StageRoot -Recurse -Force -ErrorAction SilentlyContinue
    return @{
        MovedIn     = @($movedIn)
        MovedAside  = @($movedAside)
        BackupRoot  = $backupRoot
    }
}
# **发布目录锁探测**（2026-10-15 实测）：微信开发者工具开着本工程时会抱住发布目录，原子交换于是报
# "The process cannot access the file …" 然后**静默回滚**并留下 .staging-* —— 表面现象是"appid 没
# 写进去 / 产物没更新"，真因在几屏之外（那次为它花了半小时）。
#
# 判据刻意**不猜持有者**：试一次"把第一个条目挪走再挪回来"——正是交换要做的那个操作。
# 按进程名或命令行匹配都靠不住：实测 `wxfilewatcher_x64.exe` 的命令行里**不含**发布目录路径
# （按命令行匹配会漏，第一版就是这么漏的），而只按进程名匹配又会把"开发者工具开着别的工程"误判。
function Assert-ReleaseDirNotLocked {
    param([Parameter(Mandatory)][string]$FinalRoot)
    if (-not (Test-Path -LiteralPath $FinalRoot)) { return }
    # **逐个**探测：交换会把发布根下**每个**条目都挪走，只试第一个会在"被锁的是第二个"时漏判
    # （发布根下常见 launcher/ 与 assets/ 两个兄弟目录）。
    $entries = @(Get-ChildItem -LiteralPath $FinalRoot -Force -ErrorAction SilentlyContinue |
        Where-Object { -not $_.Name.StartsWith($script:StagingPrefix) -and -not $_.Name.StartsWith($script:BackupPrefix) })
    foreach ($probeEntry in $entries) {
        $probeAside = Join-Path $FinalRoot (".lockprobe-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
        $locked = $null
        try {
            Move-Item -LiteralPath $probeEntry.FullName -Destination $probeAside -Force -ErrorAction Stop
            Move-Item -LiteralPath $probeAside -Destination $probeEntry.FullName -Force -ErrorAction Stop
        } catch {
            $locked = $_.Exception.Message
            # 挪出去成功、挪回来失败：一定要放回去，别把发布目录弄成半态。
            if (Test-Path -LiteralPath $probeAside) {
                Move-Item -LiteralPath $probeAside -Destination $probeEntry.FullName -Force -ErrorAction SilentlyContinue
            }
        }
        if ($locked) {
            throw ("release dir is locked (probe '$($probeEntry.Name)' 挪不动): $locked —— 先关掉持有它的程序再打包（微信开发者工具开着本工程时：cli.bat quit，或直接关闭 IDE）。")
        }
    }
}