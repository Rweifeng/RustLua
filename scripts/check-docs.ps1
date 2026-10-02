# 文档一致性门（P4 收口）
#
# 把"文档收口"从一次性人工校对变成可重复的门：编号、标题、索引、路径、链接五件事，
# 机器能查的就不靠人眼。用法：
#   powershell -File scripts\check-docs.ps1
# 退出码 0 = 全过；1 = 有失真（逐条打印 文件:行 与原因）。
#
# 检查项：
#   ① 编号唯一：docs/NN-*.md 与 examples/<project>/docs/NN-*.md 各自目录内编号不重复
#      （两个目录是**独立命名空间**，编号重叠是有意的，见 docs/README.md「文档编号约定」）；
#   ② 每个文档恰好一个 H1，且在文件首个非空行；
#   ③ 索引可达：docs/README.md 里列出的 docs/*.md 都存在，且 docs/ 下每个文档都被索引提到；
#   ④ 无历史路径：活动文档与源码注释里不得再出现 `C:\GitHub\p2` / `E:\Codex\p2`
#      （`tasks/done/` 归档、`docs/37` 失真清单、charter 的路径换算说明除外）；
#   ⑤ 相对链接可解析：](xxx.md) 指向的文件必须存在（跳过 http/https/mailto/#锚点）；
#   ⑥ UTF-8 可解码：出现替换字符 U+FFFD 视为编码事故（历史 CP936 误读的典型症状）。

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$issues = New-Object System.Collections.Generic.List[string]
$linkCount = 0
$h1Count = 0

function Get-Utf8Text([string]$path) {
    return [System.IO.File]::ReadAllText($path, [System.Text.Encoding]::UTF8)
}

# ---------------------------------------------------------------- 编号与 H1
$docRoots = @(
    (Join-Path $repoRoot "docs")
)
foreach ($projectDocs in Get-ChildItem (Join-Path $repoRoot "examples") -Directory -ErrorAction SilentlyContinue) {
    $candidate = Join-Path $projectDocs.FullName "docs"
    if (Test-Path $candidate) { $docRoots += $candidate }
}

$docCounts = @{}
$allDocs = New-Object System.Collections.Generic.List[string]
foreach ($root in $docRoots) {
    $relRoot = $root.Substring($repoRoot.Length).TrimStart("\")
    $numbers = @{}
    foreach ($file in Get-ChildItem $root -Filter *.md -File) {
        $allDocs.Add($file.FullName)
        $text = Get-Utf8Text $file.FullName
        if ($text.Contains([char]0xFFFD)) {
            $issues.Add("$relRoot/$($file.Name): 含替换字符 U+FFFD（非 UTF-8 或写入时编码错）")
        }
        if ($file.Name -eq "README.md") { continue }
        # 顶层无编号文档（第三方许可说明）不需要 NN- 前缀。
        if ($file.Name -eq "THIRD_PARTY.md") { continue }
        $match = [regex]::Match($file.Name, '^(\d{2,3})-')
        if (-not $match.Success) {
            $issues.Add("$relRoot/$($file.Name): 文件名缺少 NN- 编号前缀")
            continue
        }
        $number = $match.Groups[1].Value
        if ($numbers.ContainsKey($number)) {
            $issues.Add("$relRoot/$($file.Name): 编号 $number 与 $($numbers[$number]) 重复（同一目录内编号必须唯一）")
        } else {
            $numbers[$number] = $file.Name
        }
    }
    $docCounts[$relRoot] = $numbers.Count

    # H1：首个非空行必须是 `# `，且全文（代码块外）只有一个一级标题
    foreach ($file in Get-ChildItem $root -Filter *.md -File) {
        $lines = [System.IO.File]::ReadAllLines($file.FullName, [System.Text.Encoding]::UTF8)
        $firstNonEmpty = $null
        $h1 = 0
        $inFence = $false
        foreach ($line in $lines) {
            if ($line.TrimStart().StartsWith('```')) { $inFence = -not $inFence; continue }
            if ($inFence) { continue }
            if ($null -eq $firstNonEmpty -and $line.Trim().Length -gt 0) { $firstNonEmpty = $line }
            if ($line.StartsWith('# ')) { $h1++ }
        }
        if ($null -eq $firstNonEmpty -or -not $firstNonEmpty.StartsWith('# ')) {
            $issues.Add("$relRoot/$($file.Name): 首个非空行不是 H1（# 标题）")
        }
        if ($h1 -ne 1) {
            $issues.Add("$relRoot/$($file.Name): H1 数量为 $h1（必须恰好 1 个）")
        } else {
            $h1Count++
        }
    }
}

# ---------------------------------------------------------------- 相对链接
foreach ($doc in $allDocs) {
    $dir = Split-Path -Parent $doc
    $text = Get-Utf8Text $doc
    foreach ($m in [regex]::Matches($text, '\]\(([^)\s]+)\)')) {
        $target = $m.Groups[1].Value
        if ($target -match '^(https?:|mailto:|#)') { continue }
        $clean = ($target -split '#')[0]
        if ($clean.Length -eq 0) { continue }
        $linkCount++
        $resolved = Join-Path $dir $clean
        if (-not (Test-Path $resolved)) {
            $rel = $doc.Substring($repoRoot.Length).TrimStart("\")
            $issues.Add("${rel}: 相对链接指向不存在的路径 -> $target")
        }
    }
}

# ---------------------------------------------------------------- 索引可达
$indexPath = Join-Path $repoRoot "docs\README.md"
if (Test-Path $indexPath) {
    $indexText = Get-Utf8Text $indexPath
    foreach ($doc in Get-ChildItem (Join-Path $repoRoot "docs") -Filter *.md -File) {
        if ($doc.Name -eq "README.md") { continue }
        if (-not $indexText.Contains($doc.Name)) {
            $issues.Add("docs/README.md: 未索引 docs/$($doc.Name)")
        }
    }
}

# ---------------------------------------------------------------- 历史路径
$legacyPattern = 'C:\\GitHub\\p2|E:\\Codex\\p2'
$legacyAllow = @(
    "docs\00-project-charter.md",                 # 路径换算说明（历史名 -> 当前检出）
    "docs\37-引擎与游戏工程复核及优化完善计划.md"  # §5 引用失真原文
)
$scanTargets = New-Object System.Collections.Generic.List[string]
foreach ($doc in $allDocs) { $scanTargets.Add($doc) }
foreach ($extra in @("engine\README.md", "tasks\README.md", "CHANGELOG.md")) {
    $p = Join-Path $repoRoot $extra
    if (Test-Path $p) { $scanTargets.Add($p) }
}
foreach ($suffix in @("*.rs", "*.lua")) {
    foreach ($f in Get-ChildItem (Join-Path $repoRoot "engine\crates") -Recurse -Filter $suffix -File) {
        if ($f.FullName -match '\\target\\') { continue }
        $scanTargets.Add($f.FullName)
    }
    foreach ($f in Get-ChildItem (Join-Path $repoRoot "examples\game_jhyt\scripts") -Recurse -Filter $suffix -File -ErrorAction SilentlyContinue) {
        $scanTargets.Add($f.FullName)
    }
}
foreach ($file in $scanTargets) {
    $rel = $file.Substring($repoRoot.Length).TrimStart("\")
    $allowed = $false
    foreach ($a in $legacyAllow) { if ($rel -eq $a) { $allowed = $true } }
    if ($allowed) { continue }
    $text = Get-Utf8Text $file
    if ($text -notmatch $legacyPattern) { continue }
    $lineNo = 0
    foreach ($line in [System.IO.File]::ReadAllLines($file, [System.Text.Encoding]::UTF8)) {
        $lineNo++
        if ($line -match $legacyPattern) {
            $issues.Add("${rel}:$lineNo`: 出现历史路径（应为 E:\Codex\RustLua）")
        }
    }
}

# ---------------------------------------------------------------- PowerShell 脚本编码
# ⑦ 仓库里的 .ps1 必须带 UTF-8 BOM：Windows PowerShell 5.1 在中文区域设置下按 ANSI 读
#    **没有 BOM** 的脚本，里面的中文注释/字符串会变成乱码并直接解析失败——整条质量门失效。
#    这条检查是有来历的：编辑器重构期间用文件工具改写 scripts/verify.ps1 时 BOM 被静默剥掉，
#    脚本随即无法解析（详见 docs/56 §3.14）。
$ps1BomMissing = New-Object System.Collections.Generic.List[string]
foreach ($file in Get-ChildItem (Join-Path $repoRoot "scripts") -Filter "*.ps1" -File -ErrorAction SilentlyContinue) {
    $bytes = [System.IO.File]::ReadAllBytes($file.FullName)
    $hasBom = $bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF
    if (-not $hasBom) {
        $rel = $file.FullName.Substring($repoRoot.Length).TrimStart("\")
        $ps1BomMissing.Add("$rel")
        $issues.Add("${rel}: 缺少 UTF-8 BOM（PowerShell 5.1 会按 ANSI 读，中文脚本会解析失败）")
    }
}

# ---------------------------------------------------------------- PowerShell 脚本可解析性
# ⑧ BOM 只保证"字节层没坏"，保证不了"首行没被工具吃掉一个字符"。来历：
#    scripts/analyze-soak.ps1 的首行 `[CmdletBinding()]` 丢了前括号，HEAD 上就带着这个伤，
#    而它是 docs/55 阶段 0 的 soak 判据工具——直到 M27-03 验收当天第一次运行才暴露
#    （PowerShell 报"应为表达式"，ConvertFrom-Json 都没跑到）。这类脚本几个月没人碰一次，
#    全靠验收当天手气是不可接受的，所以这里用解析器把每个 .ps1 读一遍：只解析、不执行。
$ps1ParseFailed = New-Object System.Collections.Generic.List[string]
foreach ($file in Get-ChildItem (Join-Path $repoRoot "scripts") -Filter "*.ps1" -File -ErrorAction SilentlyContinue) {
    $parseTokens = $null
    $parseErrors = $null
    [void][System.Management.Automation.Language.Parser]::ParseFile($file.FullName, [ref]$parseTokens, [ref]$parseErrors)
    if ($parseErrors.Count -gt 0) {
        $rel = $file.FullName.Substring($repoRoot.Length).TrimStart("\")
        $first = $parseErrors[0]
        $ps1ParseFailed.Add("$rel")
        $issues.Add("${rel}: PowerShell 解析失败（$($first.Extent.StartLineNumber) 行：$($first.Message)）")
    }
}

# ---------------------------------------------------------------- 汇总
$docTotal = 0
foreach ($key in $docCounts.Keys) { $docTotal += $docCounts[$key] }
if ($issues.Count -gt 0) {
    Write-Host "[RED] check_docs_failed issues=$($issues.Count)"
    foreach ($issue in $issues) { Write-Host "  - $issue" }
    exit 1
}
Write-Host "check_docs_ok docs=$($docCounts['docs']) game_docs=$($docCounts['examples\game_jhyt\docs']) h1=$h1Count links=$linkCount ps1_bom_ok=$((Get-ChildItem (Join-Path $repoRoot 'scripts') -Filter '*.ps1' -File).Count) ps1_parse_ok=$((Get-ChildItem (Join-Path $repoRoot 'scripts') -Filter '*.ps1' -File).Count)"
exit 0
