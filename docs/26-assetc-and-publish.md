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

`scan` 覆盖 `project.toml`、`scenes/`、`scripts/`、`assets/`；跳过本地工作目录 `.cache`、`.temp`、`.git`、`target`。资源族统一为 `ResourceKind`（[resource_kind.rs](engine/crates/engine/src/core/resource_kind.rs)）。

### 打包内容（`package`）

`assetc package <项目> -o <out>` 输出自包含数据包 `out/`：

```
out/
├── project.toml
├── scenes/        # .scene，encrypt 时编码
├── scripts/       # .lua，encrypt 时编码
├── assets/        # compress 时压缩
├── fonts/…        # 发布子集化后的主字体（替换完整字体）
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

## 3. 发布资源编码（P2PK1）

编码文件布局（[pack.rs](engine/crates/engine/src/core/pack.rs)，ADR-0015）：

```
P2PK1 <flags> [12B nonce] <payload>
```

- `flags`：`0x01` 已 gzip 压缩、`0x02` 已加密（ChaCha20Poly1305）。
- 先压缩再加密；加密时生成随机 nonce。
- `decode` 自动检测：非 `P2PK1` 文件（开发明文）原样返回，因此同一套运行时代码既能加载开发明文也能加载发布加密包。
- 加密密钥固定内嵌于可执行文件，属于防随意读取的混淆，非强加密。

## 4. 发布流程（`scripts/package.ps1`）

编辑器「发布」动作后台启动 [scripts/package.ps1](../scripts/package.ps1)（不弹终端窗，进度解析 `==>` 阶段标记）。脚本执行：

1. Release 构建（`cargo build --workspace --release`）。
2. `assetc package` 生成数据包 `pack/`（按平台）。
3. 复制运行器到发布目录：
   - **pc**：`game.exe` + `pack/` + `fonts/NotoSansSC-VF.ttf` + `release.toml` + `run.txt` + `deps.txt` + `build-info.txt`。
   - **webgl**：`index.html` + `web-runtime.js` + `pkg/game_web.js` + `pkg/game_web_bg.wasm` + `pack/`（wasm-bindgen 生成）。
   - **wechat-minigame**：预留平台。
4. 用 `game.exe --pack <pack> --check` 做无窗口完整性校验（退出码 0 = 通过）；`-SelfTest` 把包复制到带空格的临时目录再校验，验证目录隔离（无源码/无 target 依赖）。
5. 生成 `release.toml`（`publish_name`）、`run.txt`（运行说明）、`build-info.txt`（引擎版本、toolchain、内容清单）。

PC 发布包是自包含目录，运行方式：`game.exe --pack pack --title "名称"`；无需源码或 Rust 环境。

## 5. 存档格式

运行时存档（[save.rs](engine/crates/engine/src/core/save.rs)）：

```
save_version=1
<key>\t<value>
...
```

- 文件命名 `save_<slot>.sav`（`save.slot(name)` 指定槽位）。
- 头部 `save_version=<n>`，当前 `CURRENT_SAVE_VERSION = 1`。加载只接受当前
  版本；无版本头或版本不符的存档会明确失败，没有自动迁移。
- Lua 任意值经 `save.blob_*` 序列化后按键值写入同一存档。
