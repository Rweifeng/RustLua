<#
.SYNOPSIS
工程图（branding）键的唯一解析口径：`project.toml` 里的 `icon_image` / `splash_image`。

.DESCRIPTION
`scripts/build/package-branding.ps1`（web/微信的启动品牌图）与 `scripts/build/package-icon.ps1`（PC 的
exe 图标）**必须**选到同一张图，所以扫描逻辑只留这一份（旧实现是
`package-branding.py` 里的 `configured_image()`，被 `package-icon.py` 用 importlib 复用）。

口径与引擎一致（`rustlua_infra::project_config::parse_key`）：逐行扫、第一处 `<key> =`
生效、**不认表作用域**、引号剥掉、空值当缺省。真 TOML 解析器在反斜杠 / 行内注释 /
作用域上会给出不同答案，而打包必须与运行期选到同一张图——所以这里刻意不用 TOML 库。

为什么不再用 Python：打包链上多一个解释器就多一类环境故障（PATH 上同时存在
Microsoft Store 的 `python.exe` 应用执行别名与真实安装时，`Get-Command python` 返回的是
**数组**，`& $cmd.Source` 会拼出一个不存在的命令名；Pillow 缺失又会把 web/微信两条发布
一起拦下）。纯 PowerShell 实现只依赖 .NET，Windows 上恒在。
#>

# 注意：这里**不要**开 `Set-StrictMode`——本文件是被 dot-source 进调用方作用域的，
# 严格模式会连带约束 `package.ps1` 自己（它的历史写法依赖宽松模式）。

# 解析 `project.toml` 里某个 `<key> = "<相对路径>"`：返回绝对路径，缺省/空值返回 $null。
# 非法形态（绝对路径、`..`、控制字符、Windows 保留名、结尾点/空格）直接抛——静默回落到
# 默认图会让"工程声明的图没进包"变成一个查不出来的现象。
function Get-ProjectConfiguredImage {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Key
    )

    $rootPath = (Resolve-Path -LiteralPath $Root).Path
    $toml = Join-Path $rootPath "project.toml"
    if (-not (Test-Path -LiteralPath $toml -PathType Leaf)) { return $null }

    # Windows 保留设备名（DOS 别名）：`COM1` 之类的相对路径会被 Win32 解释成设备而不是文件。
    $reserved = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
    foreach ($name in @("CON", "PRN", "AUX", "NUL")) { [void]$reserved.Add($name) }
    foreach ($prefix in @("COM", "LPT")) {
        foreach ($digit in @("1", "2", "3", "4", "5", "6", "7", "8", "9", [char]0x00B9, [char]0x00B2, [char]0x00B3)) {
            [void]$reserved.Add("$prefix$digit")
        }
    }

    foreach ($raw in [IO.File]::ReadAllLines($toml, [Text.Encoding]::UTF8)) {
        $line = $raw.Trim().TrimStart([char]0xFEFF)
        if (-not $line.StartsWith($Key + " =", [StringComparison]::Ordinal)) { continue }
        $value = $line.Substring($Key.Length + 2).Trim().Trim('"')
        if ([string]::IsNullOrWhiteSpace($value)) { continue }

        $normalized = $value.Replace("\", "/")
        $parts = $normalized.Split("/")
        $hasRealPart = $false
        foreach ($part in $parts) { if ($part -ne "" -and $part -ne ".") { $hasRealPart = $true; break } }
        $bad = $normalized.StartsWith("/", [StringComparison]::Ordinal) -or -not $hasRealPart
        if (-not $bad) {
            foreach ($ch in $normalized.ToCharArray()) {
                if ([int][char]$ch -lt 32 -or ':<>"|?*'.IndexOf($ch) -ge 0) { $bad = $true; break }
            }
        }
        if (-not $bad) {
            foreach ($part in $parts) {
                if ($part -eq "..") { $bad = $true; break }
                if ($part -eq "" -or $part -eq ".") { continue }
                if ($part.TrimEnd([char[]]@(" ", ".")) -ne $part) { $bad = $true; break }
                if ($reserved.Contains($part.Split(".")[0])) { $bad = $true; break }
            }
        }
        if ($bad) {
            throw "$Key must be a project-relative file without '..': '$value'"
        }

        $image = [IO.Path]::GetFullPath((Join-Path $rootPath ($parts -join [IO.Path]::DirectorySeparatorChar)))
        # 越出工程根（含软链/`..` 绕行）一律拒绝：打包只认工程内的文件。
        $rootPrefix = $rootPath.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
        if (-not $image.StartsWith($rootPrefix, [StringComparison]::OrdinalIgnoreCase)) {
            throw "$Key must resolve to a file inside the project: '$value'"
        }
        if (-not (Test-Path -LiteralPath $image -PathType Leaf)) {
            throw "$Key does not exist as a file: '$value'"
        }
        return $image
    }
    return $null
}
