# -----------------------------------------------------------------------------
# 打包阶段：工程配置打包（project.toml、release 标记）
# 自 scripts/build/package.ps1 原样迁出（原 L1466-L1509，2026-11-02 按功能拆分）。
# 执行语义：本文件被入口 dot-source，跑在入口的脚本作用域里——阶段之间共享变量、
#   ErrorActionPreference 与 throw 行为都跟拆分前一致。它不是函数，别当函数调。
#   PSScriptRoot 已统一换成入口设置的 packageScriptRoot（本文件不在 scripts/build/ 下）。
# -----------------------------------------------------------------------------

    $settingsSource = Join-Path $gameRoot "project.toml"
    $packSettings = Join-Path $packRoot "project.toml"
    $settingsPackaged = $false
    if (Test-Path -LiteralPath $settingsSource) {
        Copy-Item -Path $settingsSource -Destination $packSettings -Force
        $settingsPackaged = $true
        Write-Host "==> Packaged project.toml (input bindings / deadzone / audio)" -ForegroundColor DarkGray
    }

    # 发布名：默认 = project.toml 的 name；`-PublishName` 自定义（正式发行名/渠道名）时
    # 把 `publish_name` 键写进包内 project.toml（引擎的 user_dir/app_name 读它）。
    # 旧 `release.toml` 已退役——发布包判定由包内 project.toml 的 `release = true` 承担。
    $projectName = $null
    if ($settingsPackaged) {
        $nameLine = Get-Content -LiteralPath $settingsSource | Where-Object { $_ -match '^\s*name\s*=\s*"(.+)"\s*$' } | Select-Object -First 1
        if ($nameLine -and $nameLine -match '^\s*name\s*=\s*"(.+)"\s*$') { $projectName = $Matches[1] }
    }
    if ($null -ne $projectName -and $PublishName -ne $projectName) {
        Add-Content -LiteralPath $packSettings -Value "publish_name = `"$PublishName`"" -Encoding UTF8
        Write-Host "==> Injected publish_name = $PublishName into pack/project.toml (differs from project name)" -ForegroundColor DarkGray
    }

    # 发布标记要写进**包内**那份 project.toml（不是工程根那份）：双击 game.exe 时
    # `game::cli::find_project_root` 取 `<exe>/pack`，`user_dir::is_release_pack` 读的就是这个
    # 文件里的 `release = true`。缺这一个键整包退化成开发工程：存档/日志写进安装目录
    # （装在 `C:\Program Files` 时不可写＝静默丢档），`%APPDATA%\<app>\project.toml` 的玩家设置
    # 与首次播种整条不走，重装即覆盖玩家改键。2026-10-07 退役 release.toml 时这条注入漏了。
    # 追加而不是重写整份：引擎侧读 project.toml 全是按行取键、不看表段
    # （`project_config::parse_key`、`settings::from_toml`），键落在 `[input]` 之后照样读得到。
    # 只在 pc 形态注入：webgl/微信没有文件系统（`platform_data_dir` 在 wasm 上是虚拟根
    # `/rustlua-user`），翻开 packaged 形态等于换掉它们 settings 的落点，要单独验。
    $releaseMarked = $false
    if ($settingsPackaged -and ($Platform -eq "pc")) {
        if (-not (Select-String -LiteralPath $packSettings -Pattern '^\s*release\s*=\s*true\s*$' -Quiet)) {
            Add-Content -LiteralPath $packSettings -Value "release = true" -Encoding UTF8
        }
        # 反查真实产物：写不进就说写不进，别让 build-info 空头承诺 release=true。
        if (-not (Select-String -LiteralPath $packSettings -Pattern '^\s*release\s*=\s*true\s*$' -Quiet)) {
            throw "pack/project.toml 缺 `release = true` 标记，这个包会被当成开发工程运行（存档写进安装目录、玩家设置不生效）: $packSettings"
        }
        $releaseMarked = $true
        Write-Host "==> Marked pack/project.toml with release = true (player data -> $PlayerDataDir)" -ForegroundColor DarkGray
    }

