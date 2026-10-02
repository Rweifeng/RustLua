# 资源管线与发布（assetc）

- 适用范围：项目数据模型（`project.toml`）、资源数据库/导入缓存、`assetc` CLI、发布包格式与编码、存档格式。
- 配套文档：[资源数据库](14-resource-database.md)、[导入缓存](15-import-cache.md)、[场景格式](16-scene-format.md)、[技术决策 ADR-0014/0015](02-technical-decisions.md)。

## 1. 工程配置 `project.toml`

游戏工程（数据型目录，非 Rust crate）在根目录用 `project.toml` 声明：

```toml
[project]
name = "game"                  # 工程名，发布包名默认取此值
version = "0.1.0"
engine = "RustLua"
main_script = "scripts/main.lua"
startup_scene = "scenes/main.scene"   # 可为空（headless 无场景）
font = "assets/fonts/NotoSansSC-VF.ttf"
font_fallback = "assets/fonts/NotoSansSC-Fallback.ttf"
font_weight = 400.0
```

`startup_scene` 为空时运行器/编辑器只加载脚本不实例化场景。
`font` 缺省为 `assets/fonts/NotoSansSC-VF.ttf`；可变字体默认使用 `font_weight`
实例化，缺省 400（Regular），避免 CJK 字体默认 Thin 在小字号下发虚。

## 2. assetc CLI

`assetc` 是资源构建工具（`engine/crates/assetc`），命令见其 `--help`：

| 命令 | 作用 |
|---|---|
| `scan <项目>` | 扫描并注册资源到 ResourceDatabase |
| `import <项目>` | 增量导入资源到 ImportCache |
| `clean <项目>` | 清空导入缓存 |
| `list <项目>` | 列出已注册资源 |
| `benchmark <项目>` | 输出各类资产压缩/解压基准 |
| `package <项目> -o <输出>` | 打包发布资源目录并生成清单（可选 `--compress` / `--encrypt`） |
| `--check-deps <项目>` | 检查依赖链路，列出 `Invalid` 断链 |
| `--check` | 冒烟检查（临时项目闭环验证） |

退出码：`0` 成功、`1` 失败。

### 扫描范围

`scan` 覆盖 `project.toml`、`scenes/`、`scripts/`、`assets/`；跳过本地工作目录 `.cache`、`.temp`、`.git`、`target`。资源族统一为 `ResourceKind`（[resource_kind.rs](../engine/crates/rustlua-assets/src/resource_kind.rs)）。

### 打包内容（`package`）

`assetc package <项目> -o <out>` 输出自包含数据包 `out/`：

```
out/
├── project.toml
├── scenes/        # .scene，encrypt 时编码
├── scripts/       # .lua，encrypt 时编码
├── assets/        # compress 时压缩；子集化后的主字体落在 assets/fonts/…（工程里的 font 路径被原样保留）
├── chars.txt      # 收集到的项目用字字符清单
└── manifest.txt   # 路径、大小、校验和清单
```

- `--compress`：仅对 `assets/` 下支持压缩的资产生效。
- `--encrypt`：仅对 Lua 与场景文件生效。
- **字体子集化**：`package` 用 `char_collect` 收集场景 `Text` + Lua 字符串字符，`font_subset` 把主字体裁剪到用到的字符（如 19.7MB 完整字体 → 约 33KB）；子集化失败（如 CFF 字体）时保留完整字体。

### 清单 `manifest.txt`

```
assetc_manifest=1\tassetc-package-1
{相对路径}\t{大小}\t{fnv1a 校验和(16 hex)}
```

版本号 `PACKAGE_VERSION = "assetc-package-1"` 变更会触发发布脚本重新打包。

## 3. 发布资源编码（RustLua）

编码文件布局（[pack.rs](../engine/crates/rustlua-infra/src/pack.rs)，ADR-0015）：

```
RustLua <flags> [12B nonce] <payload>
```

- `flags`：`0x01` 已 gzip 压缩、`0x02` 已加密（ChaCha20Poly1305）。
- 先压缩再加密；加密时生成随机 nonce。
- `decode` 自动检测：非 `RustLua` 文件（开发明文）原样返回，因此同一套运行时代码既能加载开发明文也能加载发布加密包。
- 加密密钥固定内嵌于可执行文件，属于防随意读取的混淆，非强加密。

## 4. 发布流程（`scripts/package.ps1`）

编辑器「发布」动作后台启动 [scripts/package.ps1](../scripts/package.ps1)（不弹终端窗，进度解析 `==>` 阶段标记）。脚本执行：

1. Release 构建（`cargo build --workspace --release`；web 平台改为 `cargo build -p game_web --release --target wasm32-unknown-unknown` + `wasm-bindgen`）。
2. `assetc package` 生成数据包 `pack/`（按平台）。
3. 复制运行器到发布目录：
   - **pc**：`game.exe` + `pack/` + `release.toml` + `run.txt` + `deps.txt` + `build-info.txt`。
     字体**在包内**（`pack/assets/fonts/NotoSansSC-VF.ttf`）：PC 不把 17 MB 完整字体复制到发布根，
     运行时以 `pack/` 为工程根、按 `settings.toml` 的 `font` 路径读取（`package.ps1` 会在打包时
     真查这一条；发布根下的 `fonts/` 只有 webgl 的 `index.html` 需要）。
   - **webgl**：`index.html`（由 `plugins/webgl/index.html` 模板替换工程名/字体路径生成）+ `web-runtime.js` + `pkg/game_web.js` + `pkg/game_web_bg.wasm` + `pkg/fengari-web.js` + `fonts/` + `pack/`。
   - **wechat-minigame**：`game.js` + `wechat-adapter.js` + `game.json` + `project.config.json` + `web-runtime.js` + `pkg/fengari-web.js` + `pack/` + `release.toml` + `run.txt` + `deps.txt` + `build-info.txt`；
     默认形态（`-AssetPack subpackage`）下另有 `mainpack.json` + `mainpack/`（主包侧首屏资源）+ `pack/game.js`（分包入口）。
     平台文件来自 `plugins/wechat-minigame/`；发布脚本会检查入口/配置/资源齐全、拒绝混入 `game.exe`/`index.html`、**拒绝任何非 CommonJS 脚本**（小游戏不支持 ES Module）、并强制**主包 ≤ 4 MB / 单分包 ≤ 4 MB / 总计 ≤ 20 MB**。
     不发行 wasm-bindgen 产物（浏览器 ESM 胶水，小游戏用不到也解析不了）；字体只分发 assetc 子集化产物。
     发布脚本还会做**代码包后缀合规**：微信只上传后缀在白名单内的文件（69 个，见
     `scripts/code-package-rules.js`），`.lua`/`.toml`/`.md` 传不上去（真机上读它们是
     `permission denied`），因此构建期把这些文件名追加 `.txt` 并把映射写进产物根的
     `code-package.json`（运行时只查表）；分包 `pack/` 的后缀本来都合规，改名不动分包与 CDN 导出。
     资源形态三选一，详见 [微信小游戏分包发布](52-微信小游戏分包发布.md) 与
     [plugins/wechat-minigame/README.md](../plugins/wechat-minigame/README.md)。
4. 用 `game.exe --pack <pack> --check` 做无窗口完整性校验（退出码 0 = 通过）；`-SelfTest` 把包复制到带空格的临时目录再校验，验证目录隔离（无源码/无 target 依赖）。web 平台的 `-SelfTest` 改为断言静态产物齐全，并（小游戏）在隔离目录里跑一次无头运行时验证。
5. 生成 `release.toml`（`publish_name`）、`run.txt`（运行说明）、`build-info.txt`（引擎版本、toolchain、内容清单）。

**平台差异**：web 平台共用 `plugins/webgl/web-runtime.js`，该 JS 运行时只读明文 `pack/`，因此 `-Encrypt`/`-Compress` 仅对 PC 有效；对 web 平台传这两个开关会被脚本直接拒绝（避免产出跑不起来的包）。

**主包 + 单分包（小游戏默认形态）**。`-AssetPack subpackage`（微信平台不传时的默认值）先把首屏资源
从 `pack/` **搬**到兄弟目录 `mainpack/`（搬运依据是 `scripts/first-screen-pack.js` 生成的 `mainpack.json`，
清单项在 `pack/` 下随即消失——两侧互斥，切分不全过不了字节守恒断言），再把 `pack/`
写进 `game.json` 的 `subpackages` 并生成分包入口 `pack/game.js`。运行时首屏读主包 `mainpack/`、
其余读分包 `pack/`（`wx.loadSubpackage` 按需挂载）。产物形态：

```
examples\<game>\builds\wechat-minigame\<PublishName>\
└── launcher\            # 代码包：整目录一次上传微信（就是它，没有第二个目录）
    ├── mainpack.json    # 首屏清单：构建/运行时/harness 三方共用的路由依据
    ├── mainpack\        # 首屏资源（在**主包**里，启动即用，不等分包）
    ├── pack\            # 分包 root（含入口 game.js；目录下所有文件都进分包）
    └── …
```

**瘦首包 / 资源包放 CDN（小游戏可选形态）**。`-AssetPack remote -RemotePackBaseUrl https://cdn.example.com/games/x`
会把 `pack/` 的重资源裁掉（包内只留 `project.toml`/`manifest.txt`/`settings.toml`/`chars.txt` 骨架，约 44 KB），
并把完整资源树导出到发布根下的 `assets/`（形如 `<发布根>/assets/pack/**`，整目录上传 CDN），
在首包根写入 `remote-assets.json`。微信的发布根此时是 `<PublishName>/{launcher,assets}`：

```
examples\<game>\builds\wechat-minigame\<PublishName>\
├── launcher\   # 首包：上传微信 / 导入开发者工具的就是它（含 pack/ 骨架、remote-assets.json）
└── assets\     # 资源包：整目录上传 CDN（形如 <目录>/pack/**）
```

两者在**同一个 staging 里生成、由一次原子交换一起落地**，因此不存在"新首包配旧资源包"的半更新状态；
`launcher\pack\manifest.txt` 与 `assets\pack\manifest.txt` 必须逐字节相同（远端校验用的是包内那份清单）。
`-CdnPackDir <目录>` 是镜像导出：权威位置仍是 `assets/`，给了它就额外再导出一份（方便 rsync/s3 sync）。

```json
{ "packBaseUrl": "https://cdn.example.com/games/x/pack/", "verifyChecksum": true, "timeoutMs": 15000 }
```

语义是**包内优先 + 远端兜底**，不配 `-RemotePackBaseUrl` 时行为完全不变（且会删除陈旧配置，避免真机去拉已经不存在的地址）。
下载后按 `pack/manifest.txt` 的体积 + FNV-1a 校验和比对，不符即丢弃（校验通过才写入缓存）。
发布脚本会校验导出目录与包内 manifest 的文件数/字节数逐一相等，不等就报错（避免发出一个"少资源"的包）。
本地联调：`scripts/serve-pack.ps1 -Root <发布根>\assets -Port 8787`（或直接双击 `scripts\serve-wechat-pack.bat`）
用 Node 起一个只读静态服务托管资源包（`scripts/pack-server.js`，防目录穿越、带请求日志），
并在开发者工具勾选「不校验合法域名…」。
瘦首包的运行时验证默认就能找到资源包（`<发布根>/assets`），也可显式指定
`scripts/verify-wechat-minigame.ps1 -BundleRoot <发布根>\launcher -AssetPackRoot <资源包目录>`。
注意这只是**开发者工具侧的自测替身**：真机需要公网 HTTPS + 公众平台 `downloadFile` 白名单，且手机访问不到 `127.0.0.1`。
平台细节与边界见 [plugins/wechat-minigame/README.md](../plugins/wechat-minigame/README.md)。

PC 发布包是自包含目录，运行方式：`game.exe --pack pack --title "名称"`；无需源码或 Rust 环境。

## 5. 存档格式

运行时存档（[save.rs](../engine/crates/rustlua-gameplay/src/save.rs)）：

```
save_version=2
<key>\t<value>
...
```

- 文件命名 `save_<slot>.sav`（`save.slot(name)` 指定槽位）。
- 头部 `save_version=<n>`，当前 `CURRENT_SAVE_VERSION = 2`；`MIN_READABLE_SAVE_VERSION = 1`。
  写入 v2（对 `\\`、`\n`、`\r`、`\t` 及其它控制字符做 `\xNN` 转义），因此**多行文本、
  制表符与 CJK 都能安全往返**，不会再出现"一条带换行的值破坏整档行结构"。
  读取接受 v1（原始行格式，原样读入、下次保存升级为 v2）与 v2；无版本头、`0`
  或高于当前版本的存档明确失败且**不覆盖原文件**。
- `save.load()` 返回 `(ok, reason)`，`reason ∈ {"no_save_path", "missing", "corrupt: <err>"}`，
  调用方可区分"没有存档"（正常首启动）与"存档损坏"（必须提示、保留文件）。
- `save.clear()` 丢弃内存中的全部键值；`map.state_clear(name?)` 清理 `__mapstate:*`。
  新游戏必须用它们（连同 `__map:*`/`__blob:*`）确保上一局状态不残留，见
  [18-lua-api.md](18-lua-api.md)。
- Lua 任意值经 `save.blob_*` 序列化后按键值写入同一存档。
- v1→v2 的读兼容是**读路径**：旧二进制读新档会以 `UnsupportedVersion` 明确拒绝，
  不会静默误读。

## 8. 原子发布与失败回滚（M20-03）

发布流程按「临时目录构建 → 原子替换 → 失败回滚」组织，发布目录不会处于半更新状态：

```
<release>/                     最终目录（构建期不被触碰）
  .staging-<时间戳>-<随机>/    本次发布全部内容在此生成与自检
  .backup-<时间戳>-<随机>/     交换期间暂存旧内容
```

- **构建阶段**：`scripts/package.ps1` 把所有产物写进 `.staging-*`（assetc 打包、运行器/字体/文档、
  `release.toml`、`deps.txt`、`build-info.txt`、`run.txt`、pack 自检）。此阶段失败 → 丢弃 staging，
  打印 `==> Release failed before swap; existing release kept at <dir>`，旧版本原样可用。
- **替换阶段**：旧内容整体重命名进 `.backup-*`（同卷重命名，原子），再把 staging 顶层项重命名进最终目录，
  最后删除 backup 与 staging；成功打印 `==> Release swapped in: N entries replaced, M entries moved aside`。
- **回滚**：替换中任一步失败（文件占用/磁盘错误等）→ 新内容退回 staging、旧内容从 backup 放回原位、
  删除 backup，并以 `release swap failed (old release restored at <dir>): <原因>` 抛错；staging 保留用于诊断。
- **遗留清理**：每次发布开始时按 `.staging-`/`.backup-` 前缀 + 24 小时阈值清理中断遗留目录
  （不触碰发布内容）。`-SelfTest` 的隔离临时目录清理为尽力而为（退避重试 + 告警路径），不阻断发布。
- **`-SkipBuild`**：复用 `target/release` 既有产物重复发布（缺少产物时明确报错）。
- **回归**：`scripts/test-release-atomic.ps1` 覆盖成功交换、文件占用回滚、交换前失败丢弃、遗留清理四个场景，
  输出 `release_atomic_ok cases=4`。

发布目录布局与 RustLua 资源包格式未变（pc：`game.exe + pack/ + release.toml + run.txt + deps.txt`，
字体在 `pack/assets/fonts/` 内；webgl 另加 `index.html + pkg/ + web-runtime.js + fonts/`——根下的
`fonts/` 只服务 webgl 的 `index.html`）。

### assetc 打包的输入/输出安全边界（M23）

`assetc package` 只负责"复制 + 编码 + 清单"，因此边界必须自己守住（`engine/crates/assetc/src/lib.rs`）：

- **输出目录与项目目录的关系**：`out == project`、`out` 位于 `scenes/`/`scripts/`/`assets/`
  之内、以及项目位于 `out` 之内三种情况**明确报错**。前两者会直接覆写项目文件或让
  `collect_files` 一边遍历一边写入（自我复制，且下次打包把产物当输入）。**允许**
  `out` 位于项目内的其它位置（如 `builds/`）——`scan`/`collect_data_files` 只遍历
  `project.toml` 与那三个子目录，`scripts/package.ps1` 的原子发布正是这样布局的
  （`<project>/builds/<platform>/.staging-*/pack`）。比较前先规范化路径
  （`canonical_or_absolute`：输出目录通常不存在，按"最近存在的祖先 + 剩余分量"规范化），
  因此 `assets/dist/../dist` 这类写法绕不过检查。
  > 回归记录：这条规则最初写成"输出目录位于项目内部一律拒绝"，`verify.ps1 -Mvp`
   > 的干净发行目录自测立刻以 `assetc error: 输出目录不能位于项目目录内部` 失败——
   > 规则必须按**真实发布布局**收窄，而不是按直觉扩大。
- **符号链接**：`collect_files` 遇到符号链接会先 `canonicalize` 再校验是否仍在被扫描的根目录内；
  指向项目外的链接**报错而不是跟随**——`fs::copy` 会跟随链接，把项目外文件悄悄塞进发布包。
  （本机无符号链接权限时对应回归用例自动跳过，纯路径判定 `path_escapes_root` 仍有独立单测。）
- **清单路径**：`strip_prefix(out)` 失败时返回可定位错误，不再 `expect` panic——打包中途 panic 会
  留下半成品发布目录。
