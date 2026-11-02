[CmdletBinding()]
param(
    # Game project root to package.
    [string]$ProjectRoot = "",
    # Custom release name. Defaults to [project].name in project.toml.
    [string]$PublishName = "",
    # Target publish platform: pc, webgl or wechat-minigame.
    [ValidateSet("pc", "webgl", "wechat-minigame")]
    [string]$Platform = "pc",
    # Run the directory-isolation self test (copy to a temp dir with spaces,
    # launch the packaged game --pack without any source/target/.cache).
    [switch]$SelfTest,
    # Compress supported assets in the release pack (present = on).
    [switch]$Compress,
    # Encrypt Lua/scene resources in the release pack (present = on).
    [switch]$Encrypt,
    # M20-03: reuse existing release binaries (skip cargo builds) so repeated
    # publishes are fast and the staging/swap/rollback flow can be verified.
    [switch]$SkipBuild,
    # M73-00 WP5 第五片：web 产物带上 GPU 渲染路径（`game_web --features gpu2d`，
    # 含 wgpu 的 WebGL2 后端）。**默认关**：它会让每个 web 产物都背上这段代码
    # （体积账见 engine/Cargo.toml 的 webgl 段），而运行时还要页面上带
    # `?rustlua-gpu=1` 才会真的走 GPU——默认仍是既有 Canvas2D 路径。
    [switch]$Gpu2d,
    # 微信小游戏：资源远端回落基址（包内读不到时才用）。留空 = 纯包内，不落配置文件。
    # 例：-RemotePackBaseUrl http://127.0.0.1:8787/pack/（配 scripts/dev/serve-pack.ps1 本地测试）
    [string]$RemotePackBaseUrl = "",
    # 微信小游戏：正式 appid，写进 launcher/project.config.json 的 appid 字段。
    # 编辑器的「发布项目」面板填了就会带过来（值随工程记在 <工程>/project.toml 的 wechat_appid）。
    # 留空 = 保留模板默认 touristappid，只能本地预览，**上传会失败**（真机/上传都要求正式 appid）。
    [string]$WechatAppId = "",
    # 可选：`game --trace-loads` 采到的加载集。给了就在发布期校验 boot 上下文读到的
    # 每条路径都落在 main 单元，而不是只信计划表声明。
    [string]$BootTrace = "",
    # 微信小游戏：资源包形态（三选一，只对 -Platform wechat-minigame 有意义）。
    #   subpackage = **微信默认**。主包 + 单分包：首屏资源切到兄弟目录 mainpack/（随主包上传），
    #                其余资源留在 pack/（声明成 game.json 的 subpackages），运行时按需
    #                wx.loadSubpackage 挂载后本地沙箱读取。零服务器、零备案、零域名白名单。
    #   full       = pack/ 随主包一起分发全部资源（整个包都计入 4MB 主包上限）
    #   remote     = 瘦首包：pack/ 只留骨架（project.toml.txt/manifest.txt/chars.txt），
    #                重资源全部从远端下载；必须同时给 -RemotePackBaseUrl
    [ValidateSet("full", "subpackage", "remote")]
    [string]$AssetPack = $(if ($Platform -eq "wechat-minigame") { "subpackage" } else { "full" }),
    # 微信小游戏：资源包的**镜像**导出目录（形如 <目录>/pack/**），用于直接同步到 CDN。
    # 权威位置始终是 <发布根>/assets（与 launcher/ 同级、同一次原子交换落地）；
    # 给了这个参数就在 assets/ 之外**再**导出一份到该目录。
    [string]$CdnPackDir = "",
    # 微信小游戏 remote 形态：把 **wasm 内核**也搬进 pack/ 分包（默认关，其它形态行为不变）。
    # 动机（第 52 轮实测）：主包硬门 4 MB，而**内核一个人就超过它**——remote 形态下内核留在
    # pkg/，主包 = 内核 + 胶水 + 运行时 + 骨架 + 子集字体，怎么瘦都过不去；把内核交给分包是
    # 唯一不动引擎本体的出路（subpackage 形态本来就是这么做的，见下面的搬运段）。
    # **代价**：标题画面与"下载分包"画面排在 boot 之前（适配层已有 `ensurePackForBoot` +
    # pre-boot 进度界面）。前提是这份包要有 `pack.json`（`layout.subpackage_root=pack`、
    # entries 为空）——适配层就是靠它判断"内核在分包"，缺了它会去主包找内核并启动失败。
    [switch]$WasmInSubpackage,
    # 资源容器形态（`docs/67` 批 C2 之后）：**默认 grouped**。
    #   grouped = 按 `bundles.json`（缺省用内容域默认规则）分组，产出 `bundles/*.rlb` + `bundle_plan.json`
    #   single  = 单容器（整棵树一个包，不分组）
    #   none    = 目录形态（散文件），旧行为；要逐文件压缩/加密，或需要 JS 侧按散文件后处理时用它
    #
    # 为什么现在可以默认开（2026-10-19，三端都有实测证据）：
    #   · PC：`game --check` 走容器 1,371 条目；`assetc verify` 与目录形态逐字节相等
    #   · WebGL：`test-webgl.bat` → 请求 1,371 → 6，预载 1,596 ms → 0，传输 5.6 MB → 3.6 MB
    #   · 微信：`verify-wechat-minigame.ps1` → 主包 91.2% → **52.4%**（3,736 → 2,147 KB）
    # 容器形态下三段 JS 后处理各自换了活法：
    #   · Lua 注释剥离**不做**（工具依赖 manifest.txt；进 zstd 块后落盘只省一二百 KB）
    #   · 首屏切分改**读计划表**（不再推导闭包，assetc 是唯一分析者）
    #   · 代码包后缀改名照旧（按文件名生成映射，`.rlb` → `.rlb.txt`）
    # 想回旧行为：`-BundleMode none`（逐字节等于 C2 之前的产物）。
    [ValidateSet("none", "single", "grouped")]
    [string]$BundleMode = "grouped",
    # 容器块大小（KB）。0 = 用 assetc 默认 256 KB。块大小的实验开关
    # （`docs/67` §9）：越大压缩率略好、启动要解的块更少，但惰性读粒度更粗。
    [int]$BlockSizeKb = 0,
    # 多分包根（opt-in，`docs/67` §5.4）：按 `bundle_plan.json` 把 `sub:<name>` 拆成
    # 独立的 `pack-<name>/` 根，并写 `game.json` 的多个 `subpackages`。
    # 默认关闭——适配层的多根路由尚未接完，打开只用于验收产物形状。
    [switch]$MultiSub
)

# 打包入口（2026-11-02 按功能拆分）：本文件只保留参数与阶段编排。
# 各阶段在 scripts/build/package/*.ps1 里，按顺序 dot-source —— 与拆分前是同一个作用域，
# 所以共享变量、ErrorActionPreference、throw/return 语义都不变（拆开不改行为）。
# 为什么不是函数文件：主体共享约 200 个跨阶段变量，改成函数要么传巨型参数表、要么全改
# script 前缀，两者都会让「逐字节等价」失去可验证性。验收方式见 docs/changelog：
# 三平台各打一次包，产物逐文件 SHA256 与拆分前一致（build-info.txt 的 build_time 除外）。
$packageScriptRoot = $PSScriptRoot
$packageModuleRoot = Join-Path $packageScriptRoot "package"

# ---- 准备：上下文、工具函数、发布目录 ----
. (Join-Path $packageModuleRoot "00-context.ps1")
. (Join-Path $packageModuleRoot "10-helpers.ps1")
. (Join-Path $packageModuleRoot "20-staging.ps1")

Push-Location $engineRoot
try {
    # ---- 构建与打包阶段（顺序即语义） ----
    . (Join-Path $packageModuleRoot "30-build.ps1")
    . (Join-Path $packageModuleRoot "40-assets.ps1")
    . (Join-Path $packageModuleRoot "50-runner.ps1")
    . (Join-Path $packageModuleRoot "60-settings.ps1")
    . (Join-Path $packageModuleRoot "70-first-screen.ps1")
    . (Join-Path $packageModuleRoot "72-subpackage.ps1")
    . (Join-Path $packageModuleRoot "74-pack-export.ps1")
    . (Join-Path $packageModuleRoot "76-code-rename.ps1")
    . (Join-Path $packageModuleRoot "80-validate.ps1")
    . (Join-Path $packageModuleRoot "82-manifest.ps1")
    . (Join-Path $packageModuleRoot "84-swap.ps1")
}
catch {
    # 交换之前失败：最终目录从未被改动，直接丢弃 staging，并明确告知旧版本仍在。
    if (-not $script:swapAttempted -and (Test-Path -LiteralPath $stageRoot)) {
        Write-Warning "==> Release failed before swap; existing release kept at $finalRoot"
        Write-Warning "==> Discarding staging: $stageRoot"
        Undo-ReleaseStaging -StageRoot $stageRoot | Out-Null
    }
    elseif ($script:swapAttempted -and (Test-Path -LiteralPath $stageRoot)) {
        Write-Warning "==> Swap failed; old release restored. Staging kept for diagnosis: $stageRoot"
    }
    # -CdnPackDir 镜像的 staging 建在发布根之外，不清理就会在镜像目录里留下 .staging-*。
    if ($script:cdnMirrorStage -and (Test-Path -LiteralPath $script:cdnMirrorStage)) {
        Write-Warning "==> Discarding asset-pack mirror staging: $($script:cdnMirrorStage)"
        Undo-ReleaseStaging -StageRoot $script:cdnMirrorStage | Out-Null
    }
    throw
}
finally {
    Pop-Location
}

# ---- 自测与收尾（-SelfTest） ----
. (Join-Path $packageModuleRoot "90-selftest.ps1")