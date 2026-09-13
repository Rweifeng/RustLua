# release-atomic.ps1（M20-03）：发布目录的临时构建、原子替换与失败回滚。
#
# 设计：发布内容先在 `<release>/.staging-<时间戳>` 里完整构建与自检，全部成功
# 后才交换到最终目录；交换期间旧版本整体移到 `<release>/.backup-<时间戳>`，
# 任一步失败即回滚（新内容退回 staging、旧内容放回原位），因此发布目录**不会**
# 处于半更新状态。交换是同卷目录重命名（原子），不改变既有发布目录布局。
#
# 该模块是纯文件系统逻辑，可独立测试（见 scripts/test-release-atomic.ps1）。
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
        throw "release swap failed (old release restored at $FinalRoot): $reason"
    }

    Remove-Item -LiteralPath $backupRoot -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $StageRoot -Recurse -Force -ErrorAction SilentlyContinue
    return @{
        MovedIn     = @($movedIn)
        MovedAside  = @($movedAside)
        BackupRoot  = $backupRoot
    }
}
