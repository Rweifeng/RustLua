# CHANGELOG 归档 — 发布与 CI 质量门

> 从 [CHANGELOG.md](../../CHANGELOG.md) 按主题拆出（2026-10-25），段落逐字保留、新的在前；原先按仓库根书写的相对链接已改为相对本目录。返回[归档索引](README.md)。

## Unreleased — MVP 红门收口（第二批）：最后 3 门转绿，MVP 清单全绿

- **背景**：第一批（下方 “MVP 红门收口（第一批）”）把 14 个红门收到 3 个，红因都已查清但要决定：
  `jhyt_m63_title_prefab` 的黄金表与新门面差 28 行、`jhyt_m72_pixel_font` 的红点后移到
  “1:1 档 0 个”、`jhyt_m17_shop` 的 nil 索引只定位到段落。本批把三门逐个收口，并重跑
  `verify.ps1 -MvpOnly` 全量。
- **m63 标题门面**：新门面是横版三键（160×60 @ 380/560/740）并撤掉四角/竹分隔/印章，
  26 行是设计变更；另发现**真正的回归**——预设三行标签还是竖版坐标（`580,397 / 592,477 /
  592,557`），动态层按真实字宽居中在 `398/590/770, y=435`，② 实测差 182/42/178px。
  改预设后 ② 差 0/0/0。两处内容决定：`FooterText` 空 → 恢复为 `v0.1.1`（旧表 `v0.1.0`
  已过期，颜色回到可读的 `0.659,0.624,0.541`）；`hint` → **确认不要**（`jhyt_m20_gamepad`
  §M20-09c 断言“旧操作说明不应再显示”，不是丢失）。黄金表重导 68 行，④ 期望序列
  16 → 10 段（少掉的正是撤掉的 6 张装饰）。`--capture` 改成打印完即返回。
- **m72 像素字体**：② 要求真帧至少一个 1:1 档，标题全是 13/14/18/22/24px 非整数倍字号。
  不动字号（归 M72-10），在**第一次 `vertices()` 之前**补一个 12px 探针，首帧整张图集上传
  随探针一起发生；直方图与 1:1 计数读 `first`，稳态帧 `scene` 仍断言 `pixels` 为空。
  这里修掉一个“时绿时红”：探针放在 `frame()` 之后、靠增量上传取首帧，在
  `RAYON_NUM_THREADS=2` 下会红在“首帧 pixels 为空”（图集预热集合随线程数变）。
  读数：`75 个 UI 字形四边形 … 1:1 档 9 个，非整数倍字号 66 个`。
- **m17 商店**：商店 UI 已迁进 `shop.prefab`，反馈文案由 `prefab.set_text(handle, "ShopMsg", …)`
  写进 `Text` 组件，旧断言 `ui.get('shp:msg').text` 读到 nil。改为 `entity.find` + `World::get<UiText>`
  读 `ShopMsg`，断言“购买成功”；4a–4i 全过，临时 trace 删除。
- **发布门（顺带收口）**：重打 `builds/pc` 后，`jhyt_m31_pc_release` 连续暴露两处 **grouped 容器**
  的过期口径：① `chars.txt` 被收进 `core` 容器，但 `runtime_scene/fonts.rs` 按真实路径读它
  ⇒ `package.ps1` 补 `--keep chars.txt`；② `scripts/main.lua` / `ui_text.lua` / `text_fit.lua`
  在容器里，门却要求真实文件，而 assetc 明确不允许 keep 启动入口（Web 上 require 同步）
  ⇒ 门改按 `bundle_plan.json` 的逻辑路径验，`project.toml` / `manifest.txt` / `chars.txt` /
  字体仍必须是真实文件。重打后 `release_entries=11 stray=0`、`chars=1612 cjk=1473`、
  `jhyt_m31_pc_release_ok`。
- **实测**：`verify.ps1 -MvpOnly` → `MVP headless regression: executed=78 expected=78`
  （78/78 全绿），release 段 `jhyt_m31_pc_release_ok`、Clean release directory self test 通过，
  `MVP gate passed`（2026-10-21 重跑，主提交 `c913fc7f`）。
- **回滚档**：只回滚 `.rs` 不回滚黄金表 `.tsv` 会让 m63 红在 ①；m72 的探针与首帧顺序必须
  一起回滚，否则回到“1:1 档 0 个”。`builds/pc` 是产物（gitignore），回滚代码后需重打。
  文档读数在 `examples/game_jhyt/docs/66` §10。



## Unreleased — P2 发布与安全工程：供应链门、SBOM、tag 发布自动化、Authenticode 签名

- **问题**：发布会话只回答"包能不能跑"，不回答"依赖有没有已知漏洞/许可证越界"、"发布包里到底有什么"、
  "发布是不是只有某台机器会做"、"Windows 用户怎么确认 exe 没被换过"。`rust-toolchain.toml` 还是
  浮动的 `stable`，CI 与本机可能不是同一个 rustc；7 条 vendored fork（5 条带补丁、1 条仅 patch 映射、
  1 条 tool-only）只有"补丁在位"门，没有上游同步报告。
- **做法**：
  - 工具链锁定到 `1.97.1`，新增 `check-toolchain-lock.ps1`，并接进 `verify.ps1` 默认段；
  - 固定 `cargo-audit 0.22.2` / `cargo-deny 0.20.2` / `cargo-cyclonedx 0.5.9`（`scripts/release/tools.json`），
    新增 `check-security.ps1`：audit / deny / registry wildcard / fork-sync / SBOM 五项，三态退出
    （0 通过、1 判红、3 环境受限）；`engine/deny.toml` 落许可证/来源/bans 配置；
  - 新增 `gen-sbom.mjs`：CycloneDX 1.5、全 workspace、SHA-256、fork 标记；`--check` 可离线复核；
  - 新增 `fork-sync-report.mjs` + `engine/vendor/FORKS.json`：7 条 fork 的补丁足迹与上游版本漂移；
  - 新增 `release.yml` + `tag-release.ps1` + `release-notes.ps1`：tag 触发、PC/引擎包、SBOM、`SHA256SUMS.txt`、
    notes、Actions artifact 与 GitHub Release；`workflow_dispatch` 默认 dry-run；
  - 新增 `sign-windows.ps1`：signtool 优先、`Set-AuthenticodeSignature` 回退、RFC3161 时间戳、`-SelfTest`
    自签 + 篡改检测；无证书退 3，`-RequireSignature` 退 1；
  - 新增 `docs/69`，并更新 `docs/34`/`docs/README`/根 README/`scripts/README`/`tasks/README`。
- **证据（2026-10-20 本机/CI）**：`toolchain_lock_ok rust=1.97.1 node=20 tools=3`；
  `cargo-audit 0.22.2 vulns=0 warnings=5`（`v0.1.0` 发布时；后续清理见下）；`cargo-deny 0.20.2`
  advisories/bans/licenses/sources 全绿；`sbom_ok` 565 components / 535 hashed / 566 deps / forks=6；
  `fork_sync_ok forks=6 patched=4`；
  `signing_selftest_ok signer=authenticode tamper=HashMismatch`；`security_gates_selftest_ok`。
  真实 tag 发布已跑通：`v0.1.0` → Actions run `39378803837` `success`，4 个资产，
  `SHA256SUMS.txt` 逐文件复核 3/3 通过；真实证书签名仍未跑（本机无证书、无 `signtool`），
  本次资产为 `NotSigned`，见 `docs/69` §10/§11。
- **unmaintained 清理（2026-10-20，`v0.1.0` 之后）**：audit `warnings` 从 5 降到 1。
  `paste` 用 `engine/vendor/paste`（pastey 0.2.3 代码，兼容包名 `paste 1.0.15`）替换；
  `zbus_names`/`zvariant`/`zvariant_derive` 随 eframe 关闭 accesskit 默认特性移除；
  仅 `ttf-parser` 保留 `deny.toml` 例外（复核截止 2027-10-20）。SBOM 重算为
  505 components / 474 hashed / 506 deps / forks=7，fork 报告 `forks=7 patched=5`。
  `v0.1.0` 发布资产仍是清理前构建，未重发。
- **顺带修复**：audit 发现 `ruzstd 0.8.3` 的 RUSTSEC-2026-0344（解压炸弹），升级到 `0.9.1`；
  `ttf-parser` 的 unmaintained 例外登记在 `deny.toml`（复核截止 2027-10-20）。
- **回滚档**：删 `release.yml`/`security` job 与新增脚本即可回到纯手工发布；`rust-toolchain.toml` 改回
  `stable` 会同时让 `check-toolchain-lock` 判红——本任务不接受用 `stable` 掩盖漂移。

## Unreleased — P4 性能与测试度量：覆盖率棘轮、wasm 条件编译覆盖、平台矩阵、趋势库、编译/二进制/包体基线

- **问题**：覆盖率没有棘轮（没人拦"这次变慢/覆盖掉 0.5pp"）；wasm 条件编译只有"`game_web` check 绿了"
  的推断，没有"每条 cfg 都有编译单元、关键条目有运行证据"的表；Linux/macOS 从未编译过；
  性能比较是"这次 vs 上次"而非"vs 同环境最近 N 次中位数"；编译时间、二进制/包体大小没有历史。
- **做法**：
  - `metrics/` 底座：`policy.json`（阈值）、`allowlist.json`（到期制豁免）、`README.md`；
    `install-metrics-tools.ps1` 固定 `cargo-llvm-cov 0.9.1` + `llvm-tools-preview`，
    `tools.json` 追加 `python=3.12`；
  - 覆盖率：`coverage-summary.mjs`（llvm-cov → `coverage-v1`，口径含测试代码）+ `check-coverage.ps1`
    棘轮（总行 −0.5pp / 单 crate −2.0pp 判 2；缺工具/基线退 3；测试失败退 1）+ 5 条自测；
  - wasm 覆盖：`cfg-inventory.mjs` 扫描（剥离注释/字符串/字符字面量，稳定 key 不含行号）→
    `cfg-sites.json` 清单 + `wasm-cells.json`/`wasm-evidence.json` + `check-cfg-coverage.ps1`
    （每个 wasm 站点所在 crate ∈ wasm cell 直接包/闭包；`gpu-web-probe` 必须显式 check）+ 基座+4 反例自测；
  - 平台矩阵：`matrix.yml` 7 cell（Windows ×3 blocking；Linux/macOS ×4 advisory 观察期）+
    `promotion.json`（档位/到期日）+ `check-matrix-policy.ps1`（到期仍黄/红判 2）；
  - 趋势库：`trend_db.py`（Python stdlib sqlite3，`rebuild/ingest/report/check/query/prune`，
    6 类适配器，`run_id` 幂等，`git_dirty` 不能更新基线）+ 6 条自测；
  - 编译时间：`measure-build.ps1`（冷/热/增量，专用 target 目录）+ `parse-cargo-timings.mjs`
    （UNIT_DATA → wall/critical/per-crate）+ fixture 自测；
  - 二进制/包体：`measure-artifacts.mjs`（binary：raw/gzip9/brotli11/sha256/分节；
    package：PC/引擎/WebGL/微信主包+分包/`.rlb`，配额余量）+ 自测；
  - `verify.ps1` 新增 `-Coverage`/`-Metrics`（与 `-Quick` 及全部 `-Mvp*`/`-Benchmarks`/`-Web`/`-WasmLua`/`-Security`
    互斥；`-Quick` 跳过清单登记这两项）；`quality.yml` gates 段补 `setup-python 3.12`；
  - 新增 `docs/70`，更新 `docs/34`/`docs/README`/根 README/`scripts/README`/`tasks/README`。
- **证据（2026-10-20 本机）**：`toolchain_lock_ok rust=1.97.1 node=20 python=3.12 tools=5`（36 项）；
  `cfg_inventory_ok files=539 sites=398 wasm_attrs=48 wasm_macros=4 not_wasm_attrs=37`；
  `cfg_coverage_ok wasm_attrs=52 covered=52 wasm_macros=4 covered=4 native_attrs=37 covered=37 unknown=0`；
  `cfg_coverage_selftest_ok cases=5`、`coverage_gate_selftest_ok cases=5`、`trend_db_selftest_ok cases=6`、
  `cargo_timings_selftest_ok cases=5`、`package_metrics_selftest_ok cases=5`；
  `matrix_cells=7 blocking=3 advisory=4`；
  `binaries_ok game.exe=14495232 editor.exe=22673920 assetc.exe=4178432 game_web.wasm=9074177`；
  `packages_ok pc=20065008 webgl=12532043 wechat_main=493011 wechat_sub=5175946 quota_ok=1`；
  `baseline_compare_ok checks=12 regressions=0 mode=geometry-only`（`metrics/baselines/wp0-3d-stress.json`
  从干净 HEAD `b7958560` 恢复，三档几何量/分配峰值人工核对通过）。
  首份覆盖率基线：待 llvm-cov 首轮采集完成、人工审阅后 `--update`（见 `docs/70` §4）。
- **未完成项 / 待裁定**（登记，不另开包）：覆盖率绝对下限未定；wasm 行覆盖率未做（`wasm-bindgen-test`
  spike 未执行）；history 写入方式（bot commit vs artifact + 人工 merge）待仓库管理员定；
  CI 分钟预算（nightly perf + weekly package + coverage）待确认；Linux/macOS 是否产品支持不在 P4。
- **回滚档**：删 `verify.ps1` 的 `-Coverage`/`-Metrics` 分支与 `matrix.yml`/`metrics.yml`、
  `scripts/metrics/`、`metrics/` 即可回到"只有 m19-soak 单份基线"；各门是加法，不影响现有 Windows 门。

## Unreleased — 真实瘦首包（`-AssetPack remote`）跑通验收，顺手修掉 4 处"写死 `pack/`"的假设

- **为什么做**：remote / thin 形态此前只有"文档说它是坏的"（`cache=0` + `failed to load /project/scripts/main.lua`），
  从没有一份真实产物跑过容器闭包。而这一形态的物理形状和其它形态**完全不同**：`pack/` 被裁成骨架，
  **所有**组的容器（不只是 `cdn` 组）都随资源包导出到 `<发布根>/assets/pack-sub/bundles/…` 再整目录传 CDN。
- **真实产物**（`-AssetPack remote -RemotePackBaseUrl … -BundleMode grouped -WasmInSubpackage`）：
  `launcher/` 479 KB（骨架在 `pack-sub/`、内核在 `pack-wasm/`）+ `assets/pack-sub/` 5,493 KB。
  静态层全绿：`wechat_minigame_static_ok main=479KB … asset_pack=subpackage`、
  `bundle_mounts_ok roots=3 paths=1363 containers=5 remote=5 missing=0`、
  `boot_closure_ok roots=pack,pack-wasm`、
  `assetc_closure_ok roots=0 groups=5 entries=1357 cdn_skipped=0 cdn_checked=1 remote_checked=5 keep=17
  keep_remote=17 keep_contract=true`。
- **`assetc closure` 的 `--cdn` 语义升级**（`closure.rs`）：不再是"只核 `cdn` 组"，而是
  "**装容器的远端目录**"——本地找不到的组一律在那里核（`remote_checked=`），`keep` 条目也允许
  只存在于该目录（`keep_remote=`）。不给它时逐组判红（`组 core 的容器不在 pack/ 里 …`），不静默跳过。
  新增单测 `thin_release_checks_every_container_against_the_asset_export`（正例 + 远端缺件反例，
  `cargo test -p assetc --lib closure` 15 通过）。
- **修掉的四处硬编码假设**（都只在 remote 形态暴露，合成夹具永远碰不到）：
  1. `package.ps1` 的 `$cdnPackRoot` 停在 `assets/` 一层 → 收尾断言去找 `assets/pack`，
     **每一份 remote 产物**都会在原子交换之后抛 exit 1（产物已经换进去了，工具却报失败）；
  2. `verify-wechat-minigame.ps1` 的资源包发现/校验同样写死 `pack`（真实产物是 `assets/pack-sub`），
     于是每份瘦首包都被判"资源包不存在"；
  3. `check-bundle-mounts.js` / `check-boot-closure.js` 只认发布根里的容器 → 新增 `--remote <资源包根>`
     （远端那份**保持逻辑名**，与代码包白名单改名是两套），自测各加一例（6 例 / 2 例）；
  4. 运行时 harness 的 `--remote-root` 只认 `<root>/pack/`，改成按 `manifest.txt` 判资源包根（两种形状都认）。

  另在 `package.ps1` 原子交换**之前**对最终瘦首包跑一遍 `assetc closure --cdn <staging>/assets/pack-sub`：
  不在这里核，瘦首包的容器内容就从来没被任何门看过。
- **门**：`scripts/test/test-release-closure.ps1` 增第 10 例（瘦首包：本地缺件判红 / `--cdn` 逐组核对
  `remote_checked` / 资源包缺件判红），`release_closure_test_ok cases=10`。
- **运行时这一层也在真实开发者工具里跑通了**（同日补）：静态门只能证明"文件在、索引对"，
  所以另加了 `scripts/dev/wechat-thin-devtools-probe.js`——真实微信开发者工具 + 本地 CDN 替身
  （`pack-server.js --prefix /pack`，`/pack/<逻辑名>` → 资源包根），临时工程副本，注入探针把
  `__RUSTLUA_DIAG()` 的读数 POST 回来。连续两次绿：
  `verdict=thin-remote-ok-devtools boot=ok host=running presented=1037/1064 first_frame_at=3252ms
  remote="http://127.0.0.1:8787/pack/ remote=8 failed=0" containers_served=5 bytes=3,113,209`
  —— 5 个 `.rlb`（3.1 MB）**真的走 HTTP**、挂进引擎、世界千帧在跑。
  这一层一开始是红的，抓到两个只在远端形态存在的 bug（都在 `wechat-adapter.js`）：
  1. **容器从来没被下载过**：同步 reader（`wechatReadFile`）不会 await，远端回落只挂在 `fetchShim`
     那条异步文字路径；挂载走同步 reader ⇒ 每个容器"读不到"，日志却只有"包内未命中，转远端"。
     现象 `remote=0 failed=0` + `cannot read bundle …` + boot 落到 `lua bridge: runtime not booted`。
     修法：新增 `prefetchPlanBundles()`，挂载前把缺的容器按 `packBaseUrl` 先取回缓存
     （boot 前取齐计划表全部根，具名根 ready 后按根补取）。顺带修坐标错位：计划表里 `group.bundle`
     是包内相对路径（`bundles/x.rlb`），而 `remoteUrlFor()` 会 `slice('pack/'.length)`，
     不补前缀就砍成 `es/x.rlb`（实测 5/5 404）。
  2. **`boot` 标记抢跑**：`hostDirectBoot()` 挂在 `wasmReady`，而 Lua 运行时由宿主 `begin` 建、
     `begin` 现在要等容器预取 ⇒ 标记先报 `lua bridge: runtime not booted`（1.9s），画面其实千帧照跑。
     修法：`bundleReadyPromise` 让启动链在 `lua_execute_main` 前等"容器挂好 + reader 预装"。
  行为变更：`begin` 现在真的等各根容器就位（原来那个 `rootsReady` 算了却没 await，是死变量），
  瘦首包冷启动首帧 ~3.2s（3.1 MB 走本地 CDN；真机按带宽）。容器在代码包里的形态不受影响。

## Unreleased — 真实 grouped WebGL 产物端到端验收：抓到并修掉"cdn 组一律要 `--cdn`"的假红

- **为什么做**：WebGL 此前只有 `test-release-closure.ps1` 里的**单根合成夹具**被 `assetc closure` 查过；
  真实 WebGL 产物（`package.ps1 -Platform webgl`，默认 `-BundleMode grouped`）从没跑过这条门。
  合成夹具的组都是 `main`/`sub`，天然躲开了 `cdn` 与 `keep` 这两条分支——正是"夹具比现实干净"。
- **真跑一次就抓到红**：真实产物的 `brand` 组是 `unit=cdn`（品牌/splash 图；WebGL 没有远端那一跳，
  容器就打在自己的 `pack/bundles/` 里），而上一节刚写的规则是"计划里有 `cdn` 组、没给 `--cdn`
  就判红"。于是**一份完全正确的 WebGL 产物会被判成坏包**。
- **改法**（`engine/crates/assetc/src/closure.rs`）：`cdn` 组按**字节在哪**核对，而不是按组名：
  - 容器在发布根里 → 按普通组同一套断言就地核，计入 `cdn_local=`；
  - 容器只在镜像目录里 → 必须 `--cdn <目录>`，计入 `cdn_checked=`；
  - 两处都没有 → `cdn_skipped=N` 且**判红**（除非显式 `--allow-cdn-skip`）。

  给了 `--cdn` 时所有 `cdn` 组都会在镜像里**再核一遍**（镜像才是要上传的那份）。摘要行因此多一个字段：
  `… cdn_skipped=0 cdn_local=1 cdn_checked=0 …`。
- **实测（本机，真实产物）**：
  - `package.ps1 -Platform webgl` → `webgl_package_ok files=8`；`builds/webgl` 12,238 KB / 21 文件 / 7 组；
  - `assetc closure builds/webgl` → `assetc_closure_ok roots=1 groups=6 entries=1373 cdn_skipped=0
    cdn_local=1 cdn_checked=0 keep=1 keep_contract=true`（`keep=1` = 字体按真实路径留在 `pack/`）；
  - 反例（把 `brand` 容器搬到 `cdn/bundles/`）：不给 `--cdn` → exit 1「组 brand … 都没核对到它」；
    给 `--cdn` → `cdn_checked=1` 绿。**两条臂都在真实产物上验过**，不是只验正例；
  - 浏览器层（`verify-webgl-bundle.ps1`，无头 Chrome）：`boot=ok missing_host_api=0`、
    `webgl_bundle_engine_state_ok … draws=10 tris=200`、`webgl_bundle_ok wasm=5420.8KB`，exit 0。
    场景切换门现在是**显式跳过**（`data-rustlua-scene-verify` 的写者随 B8 旧 JS 世界删除）——不是本批引入的，
    但"boot ok + 引擎真画了 10 次、200 个三角形"是当前的产物层证据。
- **门**：`cargo test -p assetc --lib closure` 14 通过（新增
  `cdn_groups_inside_the_package_are_checked_in_place`，正例 + 改坏魔数反例）；`cargo test -p assetc` 53 通过。
  `scripts/test/test-release-closure.ps1` 的 cdn 用例随之扩成**双形态**（包内就地核 `cdn_local=1` /
  搬到镜像后不给 `--cdn` 判红 / 给了 `cdn_checked=1` / 远端缺件点名 `<cdn>` + 瘦首包形态），现在 `cases=10`。
  WebGL 这侧其实**早就有门**：`verify.ps1 -Web` 在没有产物时会就地发布一份（`package.ps1 -Platform webgl`，
  默认 grouped）再跑产物层，CI 的 `web-evidence` job 走的就是它——问题只是从没在本机真实产物上跑过。
- **边界**：闭包只解析索引、**不读条目内容**（条目字节对不对由运行期 `read()` 的逐条 FNV 兜）；
  WebGL 是单根形态，`-MultiSub` 只对微信 subpackage 开放，与本批无关。

## Unreleased — `check-bundle-mounts.js` 不再持有"第二套解析/改名假设"

- **症状/风险**：这个发布门自己写死了 `bundle + '.txt'`（微信白名单改名），并在发布根缺计划表时
  回落到 `pack/bundle_plan.json`。两处都会**静默变绿**：`RENAME_SUFFIX` 一改、或 `.rlb` 进了白名单，
  检查就去找一个真机上不存在（或存在但名字不同）的文件；而真实产物里 `pack/`、`pack-sub/`
  **各有一份**计划表副本，回落到哪份全凭顺序——陈的那份照样能让门通过。
- **改法**（`scripts/build/check-bundle-mounts.js`）：
  - 计划表只认**一个**权威位置：发布根的 `bundle_plan.json`（= 适配层启动时读的那一份）；
    缺了就响亮失败，并在错误里说明"为什么不用 `pack/` 副本"。
  - 容器宿主名只认**产物自带**的 `code-package.json` 的 `renamed` 表（发布链真实写下的结果）；
    没有这张表时才用 `scripts/lib/code-package-rules.js` 的 `hostNameFor` 兜底（原名/改名都算候选），
    不再拼后缀。
  - 新增 `--self-test`（5 条：正例、缺容器、缺 `game.js`、改名表与产物不符、缺计划表不许拿副本冒充），
    已进 CI `gates` job；`bundle_mounts_ok` 摘要行多了 `containers=` 与 `renamed=` 两个计数。
- **实测**：真实产物 `roots=3 paths=1363 containers=5 renamed=5 missing=0`——5 个容器全部由产物自己的
  改名表解析出来（不是我猜的 `.txt`）；无 `code-package.json` 的多根夹具（`test-release-closure.ps1`
  的平台形态）仍绿：`release_closure_multiroot_ok roots=2 renamed=.rlb.txt`。
- **边界**：仍然**不打开容器**（内容闭包是 `assetc closure` 的事），也不校验 `pack/`→`pack-sub/` 副本
  是否与发布根那份一致（`check-boot-closure.js` 读 boot 元数据、`assetc closure` 读组表，两者各自对账）。

## Unreleased — MVP 红门收口（第一批）：10 个示例转绿，两处"红因"被查实只是门面改版的后遗症

- **背景**：上一节把清单补齐后逐个跑 80 个示例，65 绿 / 14 红。这批按"低成本 → 需要判断"
  顺序处理，原则是**先分清红因再动手**：同一个红可能意味着①判据过期、②被测物没了、
  ③真回归。三条的修法完全不同，混为一谈就会把"改名后的更准文档"当漂移修回去。
- **① 判据过期（改判据本身）**：
  - `jhyt_m23_data_validation`：`ui/prefab.lua` 的 `validate_all` 遍历的是 M71 清空的
    `NAMES`，是一个**永远返回绿**的校验器。既不登记进 `validate_data.lua`（登记 = 让
    "校验跑过了"变假），也不留入口（留着就会被 m23 的"带 validate_all 必须登记"判红）——
    直接删掉入口，文件里写明重新启用的前提是"先把校验器写成能真的失败"。现读数
    `registered=52 modules_with_validators=52`。
  - `jhyt_m30_cjk_text`：预设搬进 `assets/prefabs/ui/` 后多了一层，而扫描是**平铺**的，
    8 份预设共 154 个 `Text` 一个都没被统计（读数 14 < 21）。改成递归收集后 168 个。
    同门第二处：`MIN_REGISTERED_TEXT_SITES=28` 与去注释实测 `27` 差一处，逐提交复算
    （`fd3b4cb3`=28 → 当前=27）定位到 `a4675c53` 把 `title:hint` 搬进预设，按实测降为 27
    并把"为什么不是简单改数"写进注释。
  - `jhyt_m24_numeric_alignment`：§3 打坐单元格被改写成「恢复5点内力，钳经脉动态上限」
    （去空格 + 补上钳制那一半），旧断言按带空格的旧文案逐字比对，把一次**更准确**的文档
    修订判成了漂移。改成语义比对（去空白）**并新增代码侧断言**：`ui/input.lua` 的 practice
    分支必须真的用 `meridian.max_inner_power()` 钳制 +5。
  - `jhyt_m34_status`：§10 状态效果表的「治疗」改名「体力恢复」并补上钳制口径，归属表补该行
    （写明 40 体力出自 `data/items.lua`、钳制出自 `player/inventory.lua` 的 `meridian.max_energy()`）。
  - `jhyt_m66_pause_prefab`：黄金表里五处音量读数还是 100%，而 `project.toml [audio]` 是
    `master=0.5 / bgm=0.25`。用 `--capture` 回写，diff 只有 5 行（音量 50%、主音量 50%、
    BGM 25%、两根滑杆 x 由 840 移到 790/765），没有任何布局漂移。
- **② 被测物没了（整段退役，不改成跳过）**：
  - `jhyt_m72_tile_variant_determinism` ⑥ / `jhyt_m72_autotile_mask` ⑧ 原本把
    `plugins/webgl/web-runtime.js` 内嵌的 Lua 解算镜像喂进真 VM，与引擎逐格对账。
    M73 删掉旧 JS 世界后仓库里只剩引擎这一份实现（`plugins/` 下再无瓦片解算代码，
    `scripts/check-tile-hash-parity.mjs` 也删了），对账没有第二个边可比 —— 读文件只会
    `os error 2`。整段删除 + 写明"重新立门的前提是先有第二份实现"，①..⑤/⑦ 的确定性、
    掩码、UV 断言原样保留（读数里 `web_mirror=retired(M73)`）。
- **③ 判据写死了旧坐标（改成从数据现算）**：
  - `jhyt_m61_prefab_runtime` / `jhyt_m62_prefab_write`：门面 `BtnStart` 的命中点写死
    `(640,412)`（旧矩形 460,380,820,444 的中心）。M73 改版后按钮是 160×60 @ (380,420)，
    写死的点落到按钮上方 8px 空白里，读数变成"点一下没有派发"——看着像派发通道坏了。
    改成从 `assets/prefabs/ui/title_menu.prefab` 的 `UiLayout.offsets` 现算中心（读不到就红）。
  - `jhyt_m22_save_isolation`：地块键 `PLOT_CELL="19,4"` 与坐标常量 `PLOT_X/Y=16,4` 各说各话，
    断言读的是隔壁那块空地。改成"键是唯一真源"（坐标从键解析），失败时再打印地块账本与
    FarmGrid 上真正非零的格子 —— 这两种红长得一样（`0, want 1`）但修法相反。
- **④ 门被放错了清单**：`jhyt_m31_pc_release` 验收的是**打好的** PC 发布包（`builds/pc`），
  干净树上必然没有 → 它在 MVP 清单里只会恒红，红因是"没打包"。改登记到新的
  `$releaseExamples`：产物在就真跑、失败即红；产物不在就明打
  `[SKIP] …（不是通过）` 并指路 `scripts\build\package.ps1 -Platform pc`——与本文件里
  微信小游戏发布包那段同一口径。
- **实测**：全量重扫（79 个 `jhyt_*` 示例，`jhyt_m31_pc_release` 走发布清单不在其中）
  **75 绿 / 3 红**（上一节是 65 绿 / 14 红），红的三个就是下面"仍未收口"里的 m17_shop、
  m63_title_prefab、m72_pixel_font；退役的 `jhyt_m59_ui_prefab` 按退役口径仍留在红。
  逐门读数：`jhyt_m23_data_validation_ok registered=52 modules_with_validators=52`、
  `jhyt_m30_cjk_text` 通过（场景/预设 Text 168）、`jhyt_m66_pause_prefab_ok`、
  `jhyt_m24_numeric_alignment_ok`、`jhyt_m34_status_ok`、`jhyt_m22_save_isolation_ok
  providers=16 rows=8 day=2`、`jhyt_m61_prefab_runtime_ok`、`jhyt_m62_prefab_write_ok`、
  两个 m72 门 `web_mirror=retired(M73)`、`jhyt_m72_pixel_font` 走通 §② 的图集断言
  （红点后移到"1:1 档 0 个"，见"仍未收口"）。
- **同一批踩到的坑（不在代码里，值得记一笔）**：编辑工具改写 `.ps1` 会**丢掉 UTF-8 BOM**，
  而 PowerShell 5.1 按 ANSI 解码中文即报一堆假语法错。本次 `verify.ps1` 补 BOM 后复绿，
  另发现并发会话刚改过的 `apply-bundle-layout.ps1` / `package.ps1` /
  `verify-wechat-minigame.ps1` 三个文件同样掉了 BOM（`check-docs` 红 6 条），一并补回：
  `check_docs_ok … ps1_bom_ok=41 ps1_parse_ok=41`。
- **仍未收口（红因已查清，需要决定，不在本批）**：
  - `jhyt_m63_title_prefab`：黄金表与 `873f05f3` 之后的**新门面**差 28 行。其中 26 行是设计变更
    （三键横排、logo/lantern 重排、装饰与提示行在 Lua 里被注释掉），**另 2 行是内容丢失**：
    版本页脚 `FooterText` 的 `text=` 是空的（全工程再无 `v0.1.0`），键位提示 `hint` 被隐藏。
    回写黄金表之前要先决定这两处是"恢复文案"还是"确认不要"——否则等于把丢失当成设计签收。
  - `jhyt_m72_pixel_font`：§② 的下一步判据是"48 个字形四边形里至少一个落在 1:1 档"，
    而当前标题门面全是 13/14/18/22/24px 这类非整数倍字号 → 1:1 档 0 个。这是字号口径
    （M72-10）的事，不是点阵管线坏了。
  - `jhyt_m17_shop`：`<console>` 里 `attempt to index a nil value`（`[C]: in metamethod 'index'`，
    即被索引的是宿主 userdata 而不是普通表），失败段已定位到 `M17-04d 商店 UI` 那一段
    （E 开面板 → 键盘选择/数量/确认/切页/关闭），尚未定位到具体语句；该文件与
    `registry_with_handles` 的 `input`/`audio` 迁出内置由另一会话在改，本批没碰它。
- **回滚档**：`git checkout HEAD -- <文件>`。判据类改动（m30 的 27、m24 的语义比对、
  m61/m62 的现算中心）单独回滚会把对应的红带回来；退役类改动（两个 m72 段的删除）
  回滚后需同时恢复 `plugins/webgl/web-runtime.js` 才能跑。

## Unreleased — 远端组与 keep 文件也进闭包：跳过不再等于通过

- **症状/风险**：`assetc closure` 有两条"不在本地容器正常位置"的分支是被**静默跳过**的：
  ① `unit = "cdn"` 的组（`-AssetPack remote` / thin：容器发布在 CDN 镜像里）只被数进
  `cdn_skipped=N`，容器缺失或内容与计划不一致时日志与"通过"一模一样；② `--keep` 命中的条目
  （微信字体 `wx.loadFont`、音频 `InnerAudioContext.src` 只吃真实路径）根本没进计划表，于是
  "某个字体/音频既不在容器里、也不在目录里"（真机表现：字体回退、进游戏没声音）无人拦。
- **改法**：`assetc closure` 新增 `--cdn <目录>`（远端组按**同一套**断言逐组核对：条目数 /
  `block_size` / 原始字节 / 容器大小 / FNV / 每条计划路径都在索引里，根名报 `<cdn>`）；
  不给 `--cdn` 而计划里有 cdn 组 → **判红**，要显式 `--allow-cdn-skip` 才放行（摘要仍打印
  `cdn_skipped=N`，跳过数永远可见）。打包期把 `--keep` 命中的**具体路径**写进计划表 `keep` 字段，
  发布期断言"在发布根里是真实文件"且"不在任何容器索引里"。`keep` 是 `Option`：缺失/`null` =
  旧版本计划（无契约可查，摘要 `keep_contract=false`），`[]` = 声明零条。
- **接线**：`package.ps1` 在 remote 形态把镜像 staging 传给 `apply-bundle-layout.ps1 -CdnDir`；
  `verify-wechat-minigame.ps1` 新增 `-CdnDir`（不给则显式 `--allow-cdn-skip`）。
  `test-release-closure.ps1` 从 7 条扩到 9 条（keep 正例 + 删文件反例；cdn 三种情形）。
- **实测**：`game_jhyt` 重打一份 grouped：`assetc_closure_ok roots=1 groups=5 entries=1357
  cdn_skipped=0 cdn_checked=0 keep=17 keep_contract=true`；删掉 `assets/audio/bgm_manor.mp3` 后
  判红并点名该文件；旧产物（无 keep 元数据）仍绿但打印 `keep_contract=false`。
  回归脚本：`release_closure_keep_ok keep=1 …`、`release_closure_cdn_ok cdn_groups=1 …`、
  `release_closure_test_ok cases=10`。
- **代价**：计划表多一个可选字段（JSON 只增不减，旧读取端忽略它）；远端组现在要求调用方给出镜像
  目录，否则必须显式声明跳过——这是**有意**的：这类"我忘了给参数"以前会伪装成通过。
- **边界**：`--cdn` 只核对"镜像里有这个容器且内容与计划一致"，不负责把容器**传上去**（上传仍是
  `package.ps1 -CdnPackDir` / rsync 的事）；keep 断言只覆盖计划表声明过的路径，旧计划无此元数据。

## Unreleased — CI 里的闭包门第一次站在**平台坐标**上：真拆根、真改名、真多根

- **症状/风险**：`assetc closure` 此前在 CI 上只对 assetc 的**原始输出**（单根、`.rlb` 未改名）跑过；
  而平台发布会做两件改变容器物理坐标的事：按布局把容器搬进 `pack/`、`pack-sub/`、`pack-<name>/`，
  以及按上传白名单把 `.rlb` 改名成 `.rlb.txt`。逻辑键不变、物理位置全变——"检查器在自家门口能跑"
  和"门在平台目录下仍然成立"是两件事，CI 一直只证了前者。
- **改法**：`scripts/test/test-release-closure.ps1` 从 5 条扩到 7 条，新增一节用**真工具**造平台形态：
  `assetc layout` 出布局 → `bundle-layout.js --apply` 真搬容器、真改名 → `check-bundle-mounts.js` →
  多根 `assetc closure`（断言摘要必须报 `roots=2`，否则说明它退回了单根回退、这条用例等于白跑），
  再加反例：删掉分包容器必须点名 `pack-sub/`。CI 的 `release-closure` job 与 `verify.ps1` 都跑，
  零 wasm、零浏览器、零 GPU，秒级。
- **实测**：`release_closure_multiroot_ok roots=2 sub_containers=1 renamed=.rlb.txt`；
  `assetc_closure_ok roots=2 groups=2 entries=6`；`bundle_mounts_ok roots=2 paths=8 missing=0`；
  反例 `release_closure_multiroot_missing_ok`（exit 1）；整脚本 `release_closure_test_ok cases=7`。
- **代价**：用例多约 60 行；多根夹具依赖 `bundle-layout.js`/`check-bundle-mounts.js`（已有，无新依赖）。
- **边界**：这是**形态**夹具，不是真微信包——`verify-wechat-minigame.ps1`（`pkg/`、字体、ECMA 语法、
  主包配额）仍只在有真产物时跑，两者不互相替代。
- **回滚档**：删掉第 6/7 条这两节即可回到"只测单根原始输出"的旧覆盖。

## Unreleased — 跨端路径黄金表：三个读者读同一份文件，映射收敛成一份实现

- **症状/风险**："逻辑路径 → 组 → 包根"有三个读者：Rust（`root_of_unit` + `distribution_layout`）、
  JS（`bundle-roots.js::rootForPath`）、发布产物（`assetc closure`）。此前是三份**手抄的表**，
  各自单测全绿——但它们对同一条路径给出不同答案时不会报错，症状是**真机白图**（容器里有字节，
  读取端拿另一个键去查）。这是原 P0 清单里唯一一条一直没做的"跨端对拍"。
- **改法**：
  ① 抽出**一张表** `scripts/test/fixtures/asset-routing-golden.json`（单元→根、每条路径的组/单元/根、
  容器路径、可接受写法与必须拒绝的写法），三个读者读同一个文件；
  ② Rust 侧新增集成测试 `engine/crates/rustlua-infra/tests/asset_routing_golden.rs`：黄金表必须先通过
  真正的 `BundlePlan::parse`，再断言"组归属唯一""unit→根""容器路径归组""写法规范化""拒绝绝对路径/`..`/空/NUL"；
  ③ JS 侧新增 `scripts/test/asset-routing-golden-test.js`：`rootOfUnit`/`rootForPath`/`rootsForPaths`
  与非法 unit 抛错，期望值同样从黄金表读；
  ④ `test-release-closure.ps1` 加第 5 条：把黄金表里的路径**真的打成容器**并过 `assetc closure`，
  否则"Rust/JS 认为的键"与"打包器实际写进容器的键"仍可各对一半；
  ⑤ 映射收敛成一份实现：`bundle_plan::unit_root_and_rank`，`distribution_layout` 与 `assetc closure`
  都调它（`assetc closure` 里那份手抄副本删掉）。
- **实测**：`cargo test -p rustlua-infra --test asset_routing_golden` 6 条；
  `node scripts/test/asset-routing-golden-test.js` → `asset_routing_golden_ok units=5 paths=6 containers=3 checks=24`；
  `test-release-closure.ps1` → `cases=5`（新增 `release_closure_golden_ok paths=6`）；`rustlua-infra` 全量 70 条、
  `assetc` 47 条、clippy `-D warnings` 0。
- **代价**：多一份约 100 行的 JSON 表与两个测试驱动；改路由判据时必须同步改表（表内 `note` 已写明）。
- **回滚档**：删掉表与两个测试驱动，把 `unit_root_and_rank` 内联回 `distribution_layout` 即恢复旧形态。

## Unreleased — MVP 示例清单：补登记 `jhyt_m93`、把 `jhyt_m59` 归入退役档

- **先纠正一条我自己的错报**：此前说"`verify.ps1` 的 MVP 清单停在 `jhyt_m60_tutorial`，
  `m61..m72` 都没登记"——**错的**。清单里早就有 `m61..m72`（只是排在 `m60` 之前，看结尾会误判）。
  逐个核对后的真实缺口只有两个：
  `jhyt_m93_spatial_audio`（本包新增，确实漏登记）与
  `jhyt_m59_ui_prefab`（**故意**不在清单里：文件头写着「M71 退役声明……已从 `verify.ps1` 移除」）。
- **两个缺口是两类问题，不能混**：前者是漏登记（补上即可），后者是"已知跑不了的历史门"。
  旧代码没有区分手段——`jhyt_*.rs` 在盘上但不在 `$examples` 里就 `throw`，
  于是 `-Mvp` 永远倒在第一步，而真正的"少跑了一个样例"也被同一个红掩盖。
- **改法（`scripts/verify/verify.ps1`）**：`$examples` 78 → **79**（补 `jhyt_m93_spatial_audio`，
  带"这道门守什么"的四行注释）；新增 `$retiredExamples = @("jhyt_m59_ui_prefab")`，并加两条守卫
  ——退役档里的文件**必须还在盘上**（否则这条豁免只是在掩盖删除），且**不得同时出现在 `$examples`**
  （"退役"与"要跑"是互斥的）。`$unlisted` 判定改为两边都排除。
  复核读数：清单 79 + 退役 1 = 磁盘 80，缺失 0、漏登记 0、无重复。
- **逐个跑过的读数（80 个全跑，2026-10-20）**：**65 绿 / 14 列红 / 1 退役红**。
  14 条红逐条查过原因，**没有一条与 M74-01 有关**（证据：① 临时探针证明
  `input_lua::registry_with_handles()` 装出来的运行时里 `clock/audio/input/time/log/ui/entity`
  全在、`clock.set` 可调用——所以 `jhyt_m17_shop` 的 `index a nil value` 不是"绑定没装"；
  ② 其余红的断言文本分别是 Text 组件数量、资产路径、`FarmGrid` 格位、打包目录缺失、
  已删除的 `plugins/webgl/web-runtime.js`，都不在音频/听者这条路径上）。
  归因分布：黄金表/资产路径/文本口径过期 5（m24/m30/m34_status/m63/m66）、
  预制体命中面与注册漂移 3（m23 的 `ui.prefab` 未登记、m61/m62 门口按钮派发名单为空）、
  游戏逻辑漂移 1（m22 `FarmGrid(16,4)=0 want 1`）、缺产物 3（m31 未打包、m72_tile_variant
  与 m72_autotile 都在读已删除的 `plugins/webgl/web-runtime.js`）、渲染侧 1（m72_pixel_font
  字体图集 `pixels` 为空）、未定位 1（m17_shop 的 console `index a nil value`）。
- **结论**：`-MvpOnly` 在修掉那 14 条之前仍是红的，且这次的红**不再是"清单不准"**——
  它是内容侧/资产侧/环境侧的真回归，需要单独排期逐条收（多数是更新黄金表或改路径）。
  回滚档：删掉 `$retiredExamples` 与那条 `jhyt_m93` 行即可（`$unlisted` 的那一行判定要一起回退）。

## Unreleased — `jhyt_m29_season_weather` 转绿：三条 HUD 断言改读预设节点，资源审计基准重钉 1065/23/143

- **症状/风险**：`docs/48` §1 的注释里留着一条悬案——"主角头像的 +6 立绘把已引用从 978 推到 984，
  但**这条增量从没跑过实测**，因为样例在走到资源审计之前就先红在雨天遮罩那一腿"。也就是说
  M29 这条 master 级验收自 M70/B7 起一共红了三处，全都源于**同一件事**：HUD 从脚本动态层
  （`ui.panel` / `ui.image`）搬进了 `assets/prefabs/ui/hud.prefab`，而断言还在读 `hud:rain` /
  `hud:weather_icon` / `hud:bar_hp` 这三个已经不存在的动态对象。后面那段资源审计因此两年没被执行过。
- **改法**：
  ① ④ 雨天：改成读**实体层**——`entity.find("HudRain")` → `EntityId::from_raw` →
     `runtime.prefab_ui_nodes().contains_key(&slot)` 判"这一帧画没画"（`prefab.set_visible(false)`
     是子树语义、走"不建布局项"，所以"画"与"能点中"同源），再读 `Image.color[3] ∈ (0, 0.6)`
     与 `UiLayout.offsets` 换算的 1280×720 覆盖范围；天气图标读 `HudWeatherRain` /
     `HudWeatherSun` 的 `Image.texture` 与互斥可见性。晴天那一腿同时断言雨罩节点**不在**绘制表里。
  ② ⑥ 资源条：改成读 `HudBarFill1` 的 `UiLayout` 宽度（脚本每帧 `prefab.set_rect` 写它，
     与 `ui.hud.bar_ratios()` 同源）：满血 160、半血 ≈80 且严格变短，贴图仍是
     `ui_bar_fill_hp.png`。
  ③ ⑦ 资源审计：在**干净树**（`examples/game_jhyt` 无未提交改动）上实测
     `assets/` 共 1231 = 已引用 **1065** + 开发产物 **23** + 内容缺口 **143**，按实测重钉
     （旧值 984 / 47 / 2）。三条断言合并成一条诊断：失败时一次给出两个基准、实测值、
     143 的构成与两条收口路线（不再只报第一条）；另加 `RUSTLUA_M29_DUMP_GAPS=1` 取数入口，
     跑一次就打出内容缺口的按目录分组，重钉不再需要临时改代码。
  ④ `docs/48` 同步：§1 追加 2026-10-19 重钉块（差额构成与复现命令）；§2 按目录补 36 行
     决定（29 个作物的 `stage_*.png` 中间帧 + 拆层整图/阴影/改色备份），`dialogue.ui.json`
     那行随 `.prefab` 迁移清零；§3 的"标题原图改名"改成"原图不再入库"。
  ⑤ `scripts/ui/hud.lua` 里随迁移失活的三个常量表（`RAIN_OVERLAY` / `RAIN_ICON` /
     `WEATHER_ICONS`）删掉——几何与配色全在预设，留着只会让下一个人以为雨罩还由脚本算；
     两张天气图标改由 `hud.prefab` 引用（审计口径里预设整目录进语料，引用不断）。
- **实测**：`jhyt_m29_season_weather` **7 段全过**（此前卡在 ④）；`docs/48` 63 行决定
  覆盖 143 个内容缺口、接线行全部为真；`cargo test -p editor` **392 通过 / 0 失败 / 2 忽略**；
  `cargo fmt --all -- --check` 干净；`cargo check --workspace --all-targets` 0 错误；
  `check-docs`（links=440）与 `check-silent-traces`（assertions=8）绿。
- **代价与未验**：那 122 个 `assets/art/crops/<crop>/stage_*.png` 是 `tools/gen_crop_art.py` 的
  出图中间帧（运行期走图集 `manor_crops.png` + `CropGrid`），**是否进发行包属 P2 打包口径**，
  本次只在 §2 按"计划"登记、不改 `assetc::is_dev_artifact`；一旦认下它们，基准应改成
  1065 / 145 / 21（§1 与样例常量都写了这条）。未验：`clippy -p game --all-targets -D warnings`
  仍红在别的示例（`jhyt_m20_audio` / `jhyt_m34_skills` 的 `&host` needless borrow，属并发在制的
  `input_lua::registry_with_handles` 迁移），本次只对改动目标 `--example jhyt_m29_season_weather`
  跑 clippy `-D warnings` 并干净。
- **回滚档**：`git revert` 本提交即回到读动态面板的三条断言与 984/47/2 基准（只动验收代码与
  `docs/48`，不影响引擎与游戏运行期行为）。

## Unreleased — 修两条过期断言（预设路径、标题按钮宽度）并修掉 `host` 遮蔽

- **症状/风险**：
  ① `editor` 的 `play_sync::tests::hierarchy_panel_draws_a_runtime_spawned_prefab_screen` 长期红：
     它从 `examples/game_jhyt/assets/prefabs/title_menu.prefab` 复制门面预设，而该预设随 UI 预制体化
     搬到了 `prefabs/ui/` 下——测试拿一个不存在的路径，报 `复制门面预设: Os { code: 2, kind: NotFound }`；
  ② `game` 的 `jhyt_m20_gamepad` M20-09c 卡在 `width > 380`（注释写着"基准 372px"）：标题按钮布局后来
     改成 160×60，这条断言就只剩"数字过期"；而它一红，**后面整段**都没再执行过；
  ③ ②挡住的那段里还有真缺陷：示例用 `let (mut title, host) = …` 把 `host` 遮蔽了，于是 M22-01
     槽位/热插拔那段把 **Title 运行时的 host** 喂给主运行时——Lua 侧读到的自然是另一份输入
     （`input.pad_button(0)` 恒为 false，看起来像"手柄 A 不生效"）。
- **改法**：
  ① 预设路径按新位置改成 `assets/prefabs/ui/title_menu.prefab`（源路径与临时工程里的落点同改）；
  ② 高亮宽度不再写死像素，基准统一从 `ui.layout.PANEL.title_btn.w` 取：换项当帧断言
     `宽 > 基准 且 ≤ 基准 + 2×16`（`grow_x = 6 + 10×ease` 的上限），收敛后断言 `宽 == 基准 + 2×6`；
     顶部操作说明那条改成两件事——该面板**不再显示**（门面皮肤改版后每帧 `ui.hide("title:hint")`、
     绘制那行也已注释掉），以及 `M.hint_line()` 在手柄态仍提到 DPad（"按设备换文案"的能力没随面板丢掉）；
  ③ 每个运行时的 host 各自具名（`booted_host` / `title_host`，23 行），主运行时的 `&host` 不再被遮蔽。
- **实测**：`jhyt_m20_gamepad` **11 段全过**（此前第 11 段就红）；`editor` 测试 **392 通过 / 0 失败 /
  2 忽略**（此前 391 通过 + 那条红线）；clippy `-D warnings` 干净。
- **代价与未验**：断言从"绝对值"改成"相对布局基准"，改版面不会再让它变红，但也因此不再锁死具体像素
  值（要锁死就该在布局层加断言）；`jhyt_m29_season_weather` 仍红在 `ui.panel_get('hud:rain')`
  （M70/B7 把雨罩搬进预设后动态面板已不存在，示例里自带注释说明），与本改动无关，本次未修。
- **回滚档**：`git revert` 本提交即回到写死像素的断言与遮蔽的 `host`（只动测试代码，不影响引擎行为）。

## Unreleased — 两道闭包门各归其位：boot 门认「容器认领」，容器识别按**格式**定界

- **症状/风险**：两处"规则抄了两份"：
  ① `check-boot-closure.js` 要求每条 boot entry 都能在磁盘上找到文件；一旦首屏清单里出现
  **容器内**的条目（计划认领、内容在 `.rlb` 里），这道门就会假红——而它假红的修法通常是
  "把条目从清单里删掉"，那正好删掉首屏资源；
  ② `assetc closure` 里硬编码"`.rlb` 可能被改名成 `.rlb.txt`"，改名规则的真相却在
  `scripts/lib/code-package-rules.js`；换一种传输后缀（或白名单变一次），这道门就找不到容器、
  把合法产物判成缺件。
- **改法**：
  ① boot 门的判据改成两条通路——**散文件**（骨架/清单，允许 `.txt` 改名）或**被 main 单元的组认领**
  （计划认领即算，内容是否存在由 `assetc closure` 证明）。分工的理由写在脚本里：
  boot 门不摊开 RLB 索引，否则等于给容器格式加第二份 JS 实现，与 Rust 侧漂移的代价是启动期静默读错块；
  摘要行增加 `loose=` / `container=` 两个计数，`--self-test` 用临时目录钉住两条通路与
  "未被认领必须判红"（真产物的 boot 条目恰好全是散文件 `loose=7 container=0`，没有自测这条分支就没人走）；
  ② `assetc closure` 不再枚举改名：物理文件名 → 逻辑名一律**从 `.rlb` 截断**，后面当传输后缀，
  于是 `x.rlb` / `x.rlb.txt` / `x.rlb.bin` 归到同一个逻辑名，`x.rlb2` 不算。缺容器时的报错
  还带上"该根下现有容器"清单（搬错根时不用人肉 diff 两份目录）。
- **实测**：`boot_closure_selftest_ok cases=2`；真实产物 `boot_closure_ok … entries=7 loose=7
  container=0`；`cargo test -p assetc --lib closure` 8 条（新增 `container_names_are_recognized_across_transport_renames`、
  `a_renamed_container_still_closes`——把整个 `bundles/` 改名成 `.rlb.txt` 后闭包仍通过）；
  `cargo test -p assetc --all-features` 47 条全过。
- **代价**：`check-boot-closure.js` 多一条"计划认领"的判据（`--self-test` 与总门都跑，秒级）；
  容器识别改成扫目录，单次闭包检查多几次 `readdir`（可忽略）。
- **回滚档**：boot 门恢复"必须存在"只需删掉 `plannedPaths` 那条分支与自测；容器识别恢复白名单
  只需把 `logical_bundle_name` 换回固定两候选。

## Unreleased — 发布闭包门接进**默认流程**：总门与 CI 现在真的跑 `assetc closure`

- **症状/风险**：上一笔把 `assetc closure` 写好了，但它只在"发布机上真有产物"时跑
  （`apply-bundle-layout.ps1` 拆根后、`verify-wechat-minigame.ps1` 手工调用），于是：
  ① `verify.ps1` **从不验收微信产物**（全文 3 处 wechat，且只调路由单测）——它连"有没有产物"都不知道；
  ② CI 不打任何 grouped 包，`assetc closure` 与它覆盖的那一类缺陷（容器在、内容缺件）在 CI 上是零覆盖；
  ③ 顺带发现**产物发现器本身是坏的**：`bundle-discovery.ps1` 的微信判据写死 `pack\game_web_bg.wasm`
  + 骨架根 `mainpack\`，而 `-WasmInSubpackage` 形态把内核放 `pack-wasm/`、骨架放 `pack/`，
  于是 2026-10-19 那份**完全合法**的产物被 `Resolve-GameBundle` 判成"缺 pkg\game_web_bg.wasm"——
  总门就算想调微信验收也调不动。
- **改法**：
  ① 新增 `scripts/test/test-release-closure.ps1`：合成一个小工程 → `assetc package --bundle grouped`
  → 正例（`assetc_closure_ok` 必须出现）+ **三个反例**（改坏容器字节 / 删容器 / 塞计划外容器，
  必须分别判红并点明原因）。零 wasm、零浏览器、秒级；`-RequireAssetc` 供 CI 用（找不到就红，
  不把"门没跑"伪装成"门通过"）；
  ② `assetc closure` 支持**"发布根即 pack 根"**：`assetc package -o <dir> --bundle grouped` 的原始输出
  没有 `pack/` 那一层，此前会被判成"容器不在 pack/ 里"（实测复现）；现在单根形态下 `pack/` 不存在
  就退回发布根本身，摘要显示 `roots=<发布根>`；
  ③ `verify.ps1` 加两步：**发布闭包回归**（有 assetc 就跑，没有则明说跳过）与**微信产物静态验收**
  （解析到产物就调 `verify-wechat-minigame.ps1 -SkipRuntime`，含 `assetc closure` + boot 闭包 +
  主包配额 + 上传白名单；没有产物才打印"未运行"）；
  ④ CI 新增 `release-closure` job（`needs: lint`，构建 `assetc --release` 后跑回归）；
  ⑤ `bundle-discovery.ps1` 修正微信判据：内核在哪个 `pack*` 根、骨架在 `mainpack/` 还是 `pack/`
  一律**以磁盘为准探测**，不再在发现器里重算发布器的命名规则。
- **实测**：`release_closure_test_ok cases=4 containers=2`（正例 `assetc_closure_ok roots=1 groups=2
  entries=6`；改坏/删/多容器三条各自 exit 1）；`Resolve-GameBundle -Platform wechat-minigame`
  现在能定位 `examples\game_jhyt\builds\wechat-minigame\launcher`（此前 `[RED] 未定位到可用的产物`）；
  按总门的调用方式跑 `verify-wechat-minigame.ps1 -SkipRuntime` → `wechat_minigame_package_ok files=14`
  （`assetc_closure_ok roots=2 groups=5 entries=1357`、主包 2,934KB / 71.6%）。
- **代价**：总门在有微信产物时多约 15 秒（静态验收）；CI 多一个 job（`assetc` release 构建约 1–2 分钟，
  命中缓存后更短）。另修了两处**既有** rustdoc 断链（`assetc` 的 `package_grouped_keep` 文档链到私有项），
  它们会让 `cargo doc -D warnings` 直接判红、把总门挡在新步骤之前。
- **回滚档**：删 `verify.ps1` 的两段调用与 CI 的 `release-closure` job 即回到"只在发布机跑"；
  `test-release-closure.ps1`、`assetc closure` 的裸布局支持、发现器修正都可单独保留。

## Unreleased — 资源路径单一口径 + **容器内容**发布闭包门（`assetc closure`）

- **症状/风险**：三处各说各话，且都是"静默绿"：
  ① 构建端 `BundleBuilder::add` 从不校验路径，能建出 `assets\art\a.png` / `./x` 这类容器里有字节、
  读取端 `BundleSet::key_of` 永远查不到的包（症状是白图，不是报错）；
  ② `check-bundle-mounts.js` 只查"根目录/分包入口/容器文件在"，`check-boot-closure.js` 只查"boot 元数据与首屏清单一致"，
  两者**都不打开容器** ⇒ 某个按需资源没进容器可以一路绿到真机（启动场景不用它）；
  ③ `check_plan` 把已经实现的 `sub:<name>` 具名分包判成"发布器不支持"，计划永远到不了多根发布器；
  ④ WebGL 验收（`verify-webgl-bundle.ps1`）完全不看 `bundle_plan.json`，微信验收只在 `$multiRoot` 时才查 mount/boot
  ⇒ grouped full/thin 形态整类绕过。
- **改法**：
  ① `rustlua-infra/src/paths.rs` 新增 `normalize_project_path` / `join_project_path`——**唯一**的工程相对路径口径
  （`/` 分隔、丢 `.`、空段、拒绝绝对路径与 `..`），构建端、读取端、计划校验三处共用；
  ② `BundleBuilder::add`/`add_ranked` 先规范化再入表，非法路径报 `InvalidPath`，重复按**规范化后**判；
  `BundleSet::key_of` 改用同一函数（此前是手工 `strip_prefix` + `to_string_lossy`，`..` 会逃出根）；
  ③ `check_plan` 新增第 ⑤ 块：计划里的每条路径必须是规范形式且升序（否则运行期必然查不到），
  并删掉"具名分包没拆分"的假错误（`distribution_layout` 早已产出 `pack-<name>` 根）；
  ④ 新增 `assetc closure <发布根> [--plan …] [--json]`：逐组打开 RLB 索引，核对
  容器存在（接受微信 `.rlb.txt` 改名）、索引可解析、`entries`/`block_size`/`raw_bytes`/`bundle_bytes`/FNV 与计划一致、
  **每条 `group.paths` 都在索引里**，并查根下有无计划外容器；单根（WebGL/PC）与多根（微信）按
  "有没有 `pack-*` 兄弟目录"自适配，`cdn` 组跳过；
  ⑤ 接线：`apply-bundle-layout.ps1`（`-MultiSub` 拆根后）、`verify-webgl-bundle.ps1`（起浏览器**之前**）、
  `verify-wechat-minigame.ps1`（**无条件**，只要产物有计划表）都跑它；`-Assetc` 可显式指定二进制；
  ⑥ `plugins/wechat-minigame/bundle-roots.js::parsePlan` 增加结构校验（schema/`block_size`/unit 语法/
  `bundle` 必须 `bundles/` 前缀/路径规范与升序/跨组重复/同名组），并补 9 条负向自测；
  ⑦ `scripts/test/test-wechat-multisub.ps1` 增加**正反两向**断言：拆根后闭包必须过；改坏一个容器字节必须判红。
- **实测**：`assetc closure examples\game_jhyt\builds\wechat-minigame\launcher` →
  `assetc_closure_ok roots=2 groups=5 entries=1357`（真实产物，多根 + `.txt` 改名）；
  `cargo test -p assetc --lib closure` 4 条（正常产物、缺条路径、容器字节被改、unit 与所在根不一致）；
  `cargo test -p rustlua-infra --features bundle-write bundle` 31 条（含新增
  `builder_normalizes_or_rejects_non_canonical_paths`、`key_of_uses_canonical_project_relative_paths`、
  `non_canonical_or_unsorted_paths_are_rejected`、`named_sub_units_are_accepted_by_multi_root_layout`）；
  `node plugins/wechat-minigame/bundle-roots.js --self-test`、`node scripts/test/wechat-asset-routing-test.js` 7 组全绿。
- **代价与未验**：`checksum` **不在** JS 侧重算（两份哈希实现漂移过一次，代价是真机启动失败）——
  JS 只查无歧义的结构，校验和由 Rust 侧（`BundlePlan::parse` + `assetc closure`）在发布/验收期断言；
  CI 仍不打包微信产物（闭包门跑在发布机与本地验收，CI 只跑单测与两个 Node 门）。
- **回滚档**：三个脚本里的 `assetc closure` 调用可单独移除（Rust 侧与路径口径改动独立保留）；
  具名分包重新判错只需恢复 `check_plan` ⑤ 旧块与对应测试。

## Unreleased — 任务：`baselines/` 从 `artifacts/` 挪到仓库根并整体脱管（目录搬迁 + 三份原跟踪文件 `git rm --cached` + 三个读取方接环境变量；零逻辑改动）：题面是用户口径"E:\Codex\RustLua\artifacts\baselines 把这个目录挪出来并修改git忽略"，追问两句定的落法="..\RustLua\baselines"（=仓库根 `baselines/`，仍在树内、只是不再跟踪）与"环境变量覆盖"，提交口径="拆两笔"。旧白名单是 `.gitignore` 里那 8 行 `!/artifacts/baselines/...`（README.md、wp0-3d-stress.committed.json、engine-pub-surface.json 三份放行），本笔换成一条 `/baselines/` 整目录排除，读取方三处统一成 `RUSTLUA_BASELINES_DIR` 优先、缺省 `<仓库根>/baselines`：`scripts/verify.ps1:80-83`（`$baselinesRoot` + 现场建目录，5 处硬路径改走它）、`scripts/engine-pub-surface.mjs:32`、`engine/crates/game/examples/jhyt_m19_soak.rs:378`；注释面另清 6 处（fonts.rs、api_surface_snapshot/mod.rs、两个 wp0 示例、compare-*.ps1）。判别读数：`node scripts/engine-pub-surface.mjs --check` 从新路径读到基线并 `pub_surface_ok items=1562`（这条同时证明路径接线是对的）；`cargo check -p game --example jhyt_m19_soak` Finished exit 0；`cargo fmt --check -p game -p engine -p rustlua-scripting` EXIT=0；三个 ps1 AST errors=0、`node --check` 通过；`scripts/check-docs.ps1` DOCS_EXIT=0 六项 = docs=60 game_docs=46 h1=108 links=420 ps1_bom_ok=33 ps1_parse_ok=33；12 个改动文件行尾 crlf 全数 bare_lf=0/lone_cr=0。新增的一条账（不是缺陷，是脱管的代价）：这目录不入库 ⇒ 新克隆或换机器时它是空的 ⇒ `verify.ps1 -Benchmarks` 的 committed-3D 步与 pub-surface 门都成了**缺文件判红的预期红**，throw 文案已改成可执行的"把上一次确认没回退的 wp0-3d-stress.json 复制成这个文件名"；而 `baselines/README.md` 自己也不入库了 ⇒ 那套可比性裁定在仓库里查不到，本笔把它改写成"本地基线"口径并留在本机；`wp0-3d-stress.committed.json` 文件名里的 committed 成了历史称谓，没重命名。文档面按用户口径"只改活句与命令"清 21 处（根 README.md 目录表新增 `baselines/` 一行、docs/34/36/37/39/43/53/54/56、game_jhyt/docs/54），日期型验收记录按仓库惯例不回填（docs/34 §252、docs/54 §584 与 §717、docs/58 §124、game_jhyt/docs/66 §196、CHANGELOG 与 tasks/done/* 里的旧路径引文）。现场一条并发事实：本笔工作期间 `scripts/verify.ps1`、`docs/34`、`.gitignore`、`CHANGELOG.md` 同时压着 B8 那包的在途 hunk——`.gitignore` 的 `artifacts/` 与 `**/artifacts/logs/` 两行在我改完之后被并发改成 `/artifacts/`，工作树 CHANGELOG 里 `grep -c 接管形态纵向按键` = 0 而 `git show HEAD:CHANGELOG.md` = 1 ⇒ 这三处（含 CHANGELOG）一律按"HEAD 字节 + 只施加自己的 hunk"局部暂存（量具在 `.temp/stage-verify-mine.py`、`.temp/stage-gitignore-docs34.py`），B8 的内容留在下一笔，不由本笔替他们提交或替他们抹回。没测与本笔不碰：① `verify.ps1` 全量门与 `-Benchmarks` 实跑没跑（本笔只改路径取值，不碰判据）；② `cargo clippy`/工作区 test 没跑；③ 空目录那两条"预期红"只按代码路径推断，没在真·干净克隆上验；④ `engine/crates/editor/src/main.rs:3068` 里的 `artifacts/baselines/spatial-migration.txt` 没动（它在临时工程根下拼路径，不指仓库，改它零功能收益）。回滚档：`mv baselines artifacts/baselines` + `.gitignore` 退回那 8 行白名单 + 三处读者退回原硬路径 + 文档 21 处按本条点名的行段撤 + 摘掉本条；`git revert` 本笔即可把三份跟踪状态与目录位置一起复原。

## Unreleased — 两条护栏：发布目录**锁探测**（持有者无关、交换之前就报）+ `-PointerProbe` 与 `-GpuFrames` 互斥

上一条把"打包与开发者工具互斥"的**根因**记下来了，但只做到了"报错具名"。这一段把两条护栏补成**真管用**的，并各自双向验证。

### 1) 发布目录锁探测（`release-atomic.ps1` + `package.ps1`）

- **判据换成"做一次交换要做的操作"**：把发布根下**每个**条目逐个 `Move-Item` 挪走再挪回来；失败即抛具名错误。**不猜持有者**。
- 为什么不用进程匹配：第一版按"命令行里提到发布目录"筛，**实测漏判** —— `wxfilewatcher_x64.exe` 的命令行里**不含**路径；而只按进程名筛又会把"开发者工具开着别的工程"误判成占用。锁探测两种都绕开。
- 为什么**逐个**：交换会把发布根下每个条目都挪走，只试第一个会在"被锁的是第二个"时漏判（发布根下常见 `launcher/` 与 `assets/` 兄弟目录）。挪出去成功、挪回来失败时**务必放回去**，别把发布目录弄成半态。
- 调用点两处：交换处（`release-atomic.ps1`）+ **`package.ps1` 里一确定发布根就探**（`$finalRoot` 出现处，`Clear-StaleReleaseStaging` 之前）—— 否则要等几分钟打包完才报错。

**双向验证**：

```text
A) 占住 launcher/game.js（文件句柄）⇒
   release dir is locked (probe 'launcher' 挪不动): The process cannot access the file because it is being used by another process.
   Atomic release swap 出现次数 = 0        ← 交换根本没走到（不再回滚、不留 staging）
B) 无占用 ⇒ 打包成功并换入：launcher mtime 23:36:29（最新），build-info.txt 同步；
   残留 .lockprobe-* = 0                    ← 探针自己收拾干净
```

顺手清掉了此前失败交换留下的 3 个 `.staging-*`（诊断残留）。

### 2) `-PointerProbe` 与 `-GpuFrames` 互斥

`-GpuFrames N` 会让宿主跑满 N 帧就**停**，而真事件探针在之后才派发 ⇒ 读数必然是 0。这不是"真实输入源没接通"，是**假阴性**（本会话实测 `frames 240->240` 配 `multipoint=0`，我一度把它汇报成功能不通）。现在在**启动浏览器之前**直接拒绝：

```text
[RED] webgl bundle verification aborted: -PointerProbe 不能与 -GpuFrames 同用：
      -GpuFrames 会让宿主在 N 帧后停帧，探针读数必然为 0（假阴性）。请去掉 -GpuFrames（或设成 0）。
```

**验收**：`check-docs` 绿（`ps1_parse_ok=33`，含新改的三个 ps1）；三个 ps1 均 parse ok。

**回滚档**：删 `Assert-ReleaseDirNotLocked` 与两处调用、去掉互斥断言。

本条有 CHANGELOG 条目。

## Unreleased — wasm-only 探针不再拖红全量 clippy：`gpu-web-probe` 补目标位闸门、`Navigator` 自己声明

- **症状**：`cargo clippy --workspace --all-targets`（本地 pre-commit 与 CI 的 Clippy 步同口径）在
  `crates/gpu-web-probe` 上固定红：`E0433` 找不到 `engine::host::web_surface`、`E0277` ×2
  `map_async` 的回调不满足 `WasmNotSend`、`E0599` `Window::navigator` 不存在。此前它被当作
  "别人的在建工作"绕开——`tasks/done/M72-04` §8 明记 "`--exclude gpu-web-probe` 后 clippy 零告警"。
- **根因不在 feature 表**：这个 crate 通篇是浏览器里的异步问答——`web_surface` 在 engine 侧就是
  `#[cfg(target_arch = "wasm32")]`，而 `map_async` 的回调必须捕获 `Rc<RefCell<…>>`，非 wasm 目标上
  `WasmNotSend` 退化成 `Send` 就直接不成立。也就是说**宿主目标上它本来就无法编译**，补 feature、
  换 `Arc` 都治不到根。
- **改法**：① `src/lib.rs` 顶部加 crate 级 `#![cfg(target_arch = "wasm32")]`，注释里写清三条实测错误码、
  代价（宿主侧不再检查本 crate）与"仍留在 workspace members"的理由（`cargo tree -p game_web`
  那条"发布包不含本探针"的门要靠 members 才查得到）；② `Cargo.toml` 的 web-sys 特性补 `"Navigator"`
  ——`window.navigator()` 是探针自己调的，以前编得过是靠同一次构建里 engine 的 web 依赖把这项
  特性**合并**进来，属于借来的绿灯。
- **实测**：`cargo clippy -j 2 -p gpu-web-probe --all-targets -- -D warnings` 与带 `--all-features`
  同一条都 exit 0；正向对照（证明闸门没把探针变成空壳）`cargo build -j 2 -p gpu-web-probe --target
  wasm32-unknown-unknown` exit 0、debug 产物 324,920,099 字节，`gpu_probe` / `gpu_render_probe` /
  `storage_buffer` 仍在符号表里。只加 `Navigator`、不加闸门那一步的读数是 E0599 消失、E0433+E0277 剩
  3 条——这条差量就是"特性合并掩盖了缺声明"的证据。`cargo fmt -p gpu-web-probe -- --check` exit 0。
- **代价与未做**：宿主目标上这个 crate 从此不被任何 lint 检查，它的编译门只剩 wasm 那条构建路径
  （`scripts/gpu-web-probe.ps1` 走的就是它）；没有改 CI 的 `--workspace` 命令（改命令去迁就一个红，
  等于给所有 crate 开逃生门）。**同一轮回归里 `game_web` 仍红 15+2 条 dead_code**（`frame_input.rs`
  的 `apply_frame`/`FrameInputProducer`/`InputScript`/`ScriptDriver` 等、`fit.rs` 的 `contain_box`/
  `css_to_design`），它们只被 `gpu_host.rs`/`rust_scene.rs` 这两个
  `#[cfg(all(target_arch = "wasm32", feature = "gpu2d"))]` 模块使用，属同一类"宿主目标看不见消费者"
  的红——但那份工作正在被另一路改（`game_web/src/lib.rs` 有未提交在途内容），本批不碰。

## Unreleased — 搬门关掉写死绝对路径的调试残留：跑一次门不再脏仓库根

- **症状**：`git status` 在跑完 `jhyt_m70_hud_prefab` 后总挂一条 `?? .tmp_m70_got_dump.txt`
  （7,111 B），下一条来自 m71 的同族。它不在 `.gitignore` 里（`/.tmp_*/` 只挡目录），于是每跑一次
  门就多一个未跟踪文件，容易被读成"上一批没提交干净"。
- **归因**：`68ffcde` 给两个对账门的非 capture 分支各留了一行
  `std::fs::write("C:\\GitHub\\RustLua\\.tmp_m7?_got_dump.txt", &dump).unwrap_or_default()`
  ——写死的单机绝对路径（Linux/CI 上静默失败），且表体本来就有 `--capture` 这条正通道，
  重刷黄金表也从没读过这个文件。是脚手架，不是门的一部分。
- **改法**：删掉这两行（`jhyt_m70_hud_prefab.rs:445`、`jhyt_m71_dialogue_prefab.rs:244`），
  `compare_golden(&dump)?` 原位不动；顺手改掉两处会跟着画面漂移的行数注释
  （`scripts/verify.ps1:1037`、`hud.prefab:6` 的"黄金表 124 行"→ 136）。
- **读数**：两门删后各跑一次仍绿——`golden_lines=136` + `jhyt_m70_hud_prefab_ok`、
  `jhyt_m71_dialogue_prefab_ok`；跑完 `ls .tmp_m7*.txt` 报无此文件、`git status` 只剩本批四个改动文件。
- **买不到的部分**：这次没给"红的时候打印整张实得表"补通道（`compare_golden` 仍只报第一条不一致的
  排序行），要看全表得自己带 `--capture` 跑一遍——够用，因为它本来就是重刷通道。同族的
  `probe_title_render.rs:157` 也写死绝对路径（`engine\title_offscreen.png`），但那是给人眼看的离屏
  截图、被 `engine/*.png` 忽略、不脏 status，本批有意不动。
- 回滚档：`engine/crates/game/examples/jhyt_m7{0,1}_*_prefab.rs` + `scripts/verify.ps1` +
  `examples/game_jhyt/assets/prefabs/hud.prefab` + 本文件。

## Unreleased — GPU 用例的"跳过"不再冒充通过：`gpu_skipped` 标记 + `RUSTLUA_REQUIRE_GPU=1` 硬失败

主题：渲染器的 GPU 用例在没有适配器时是裸 `return; // no GPU available; skip`，而 **`cargo test` 对早退的
用例照样报 `ok`**——于是"engine --lib 160 通过"**可能**包含十几个一行没跑的用例，正是本仓最忌讳的
"跳过当通过"。更麻烦的是：**CI runner 没有 GPU**，这类用例在 CI 上**恒为跳过**，所以渲染器的 GPU 路径
只有本地门能覆盖；而本地门此前同样是静默跳过。

- **修法**：13 处裸跳过（7 个文件）统一走 `gpu_skipped(reason)`（`renderer/tests/mod.rs`）。它做两件事：
  ① 打印 `gpu_skipped reason=...`（配 `--nocapture` 可见、可被门统计）；② **`RUSTLUA_REQUIRE_GPU=1`
  时跳过即 panic**——在"应该有 GPU"的机器与发布门上把"悄悄没跑"变成硬失败。
- **两个探针（当场复现问题本身）**：给一个 GPU 用例插一行无条件跳过 →
  ① 默认跑：`test result: ok. 1 passed` + 打出 `gpu_skipped reason=probe`（**这就是假绿**）；
  ② 同一个用例加 `RUSTLUA_REQUIRE_GPU=1`：**exit 101 panic** ✓。探针已还原。
- **顺带得到一个真读数**：在本机用 `--nocapture` 数标记 = **0**——即这 13 个 GPU 用例在**本机真的都跑了**，
  所以本轮以及先前那些"engine --lib 通过"的读数在本机是**有 GPU 覆盖撑着的**（此前只是假设，现在可验）。
- 读数：`engine --lib` **160 通过 0 失败**；fmt / clippy 干净。
- 回滚档：7 个测试文件（`renderer/tests/*.rs`）+ `docs/34` + 本文件。
## Unreleased — 修掉 pre-commit 里 clippy 归属判断的假阴性：暂存文件自己 clippy 红过不了门

主题：**上一批自己引入的缺陷**——`captured_names_staged_file` 拿 git 的**正斜杠**暂存路径
（`crates/a/b.rs`）去比 cargo 在 Windows 上输出的**反斜杠**诊断路径（`crates\a\b.rs`），**永远比不上**。
后果不是"偶尔漏判"，而是 **clippy 这一半从没拦过任何东西**：任何 clippy 失败都被归成"提交之外"，
门打印一句"clippy 判红，但诊断里没有出现本次提交的文件"然后放行。上一批改的正是这个归属判断，
而**修错了对象比不修更危险**——它会让人以为 clippy 在守门。

- **修法**：比对前先 `tr '\\' '/'` 归一化诊断输出。一行，但它决定"clippy 算不算我的"。
- **反面对照（本条的价值在这里）**：新用例 **B11** 先暂存一份自己带告警的 `lib.rs`、再断言提交
  **必须**判红。修之前跑：**红**（`本次提交自己的 clippy 报错必须判红` 与 `判红后 HEAD 未移动`
  两条失败——即那次提交真的被放行了）；修之后跑：**绿**。用例里还特意 `git clean -fd` 清掉前面几例
  留下的未跟踪 `.rs`——否则它们会被 `add --all` 一起暂存，红在 fmt 而不是 clippy，**用例会空转**
  （这个坑本次踩过一次）。
- **读数**：`scripts/test-git-hooks.ps1` **61 例全绿**（58 → 61）；`sh -n git-hooks/pre-commit` 通过；
  hook 仍是 LF 无 BOM。
- **回滚档**：`git-hooks/pre-commit`（一处归一化）+ `scripts/test-git-hooks.ps1`（B11）+ `docs/34` 计数。
- **诚实边界**：B11 用 `let unused = 1;` 触发的是 rustc 的 `unused_variables`（`-D warnings` 下升级为
  错误），**没有单独构造 clippy 专有 lint**——所以这条用例证明的是"诊断路径能被认领"，不是"clippy 的
  每条 lint 都能被认领"。两者共用同一套路径输出，结论可迁移，但口径要说清。

## Unreleased — pre-commit 量错了对象：从「整棵工作树」改成「本次提交」

主题：**本地门禁在检查工作树，CI 在检查提交**。CI 的 lint job 先 checkout 那个 commit 再看内容；而
`git-hooks/pre-commit` 跑的是 `cargo fmt --all -- --check` / `cargo clippy --workspace`——看的是**整棵
工作树**。单人独占一棵树时两者几乎等价，所以这个错位藏了很久；但本仓的实际工作方式是**多会话共享一棵
树**，于是它变成了假红工厂：另一个会话的在途/未跟踪文件（`engine/crates/game/examples/jhyt_m6x_*.rs`）
格式不合格，**我的**提交就被挡住——而 CI 对我这个提交是绿的。假红的代价不是"多等一会儿"，而是训练所有人
去按 `RUSTLUA_SKIP_PRECOMMIT=1`；门禁一旦被习惯性绕过就名存实亡，而"没人跑得起门"正是缺陷能在 HEAD 里
活过多个提交的土壤（docs/34 §6 记过一次同款事故「门禁波动（并行会话）」，当时的处置只是记录在案）。

- **判据改成"失败的文件在不在本次提交里"**：fmt 用 `capture()` 捕获输出，把 `Diff in <路径>:行:` 归一化成
  仓库相对路径（`tr '\\' '/'` + 去掉 UNC 前缀 + 去仓库根前缀），再与 `git diff --cached --name-only` 的
  暂存集比对，同一个文件的多行 hunk 去重后点名；clippy 因为编译天生是整仓的、没法只看暂存内容，改成
  "诊断里有没有出现本次提交的文件"。命中 → 照旧判红；只在提交之外 → 逐条点名 + `PARTIAL` 措辞，
  **不冒充"通过"，也不越权替别人的在途活儿判红**。
- **开关 `rustlua.scope`（默认 `commit`）**：`worktree` 恢复旧口径（工作树里任何一处失败都拦提交）。
  非法值显式报警并回落 `commit`，不静默降级——与既有 `rustlua.clippy` 档位同口径。`install-git-hooks.ps1
  -Status` 一并报出该档位：没人知道存在的开关等于不存在，体检仍严格只读。
- **真机端到端（不是只在临时仓库里过）**：在**真实仓库**上用临时索引 + `GIT_CONFIG_*` 覆盖跑真 hook
  （真索引与仓库配置一个字节没动）。工作树此刻有 6 个属于并发会话的未跟踪未格式化文件、暂存的是我自己
  格式合格的改动：`commit` 口径 **exit 0** 并把那 6 个文件逐条点名进"不在本次提交里"；同一次暂存换
  `worktree` 口径 **exit 1** 拦住。假红消除与旧口径可恢复，都是实测读数。
- **反面对照四条（`test-git-hooks.ps1` §B/§A，46 → 58 例）**：B7 提交之外的未格式化 `.rs` **不拦**且断言
  提交真的完成；B8 同一场景 `scope=worktree` **重新拦住**；B9 同时摆一份提交之外的失败，暂存区自己的未
  格式化 `.rs` **仍然判红并点名文件**（证明分类器在分辨，而不是"一有失败就放行"）；A7 非法 `scope` 值
  报警并回落。B7 的失败样本特意放在 `examples/` 而不是 `src/scratch.rs`——`cargo fmt --all` 只看**目标
  的**文件，摆在 `src/` 下但没被 `lib.rs` 引用的 `.rs` 根本不会被检查，那会让用例空转。
- **顺带两处**：删掉因此变成死代码的 `step()`（fmt/clippy 都改走 `capture()`）；新增工具前置（0a 段）——
  缺 `grep/sed/tr` 时 hook 会**静默退化**成"没碰 Rust 就跳过"（实测：直接 `sh git-hooks/pre-commit` 时
  grep 不在，两次运行都以"跳过"通过），所以判红而不是放行。
- **B10 的诚实边界**：想构造"缺 grep"的用例时发现**构造不出来**——实测 git 执行 hook 时会把自带
  `/usr/bin` 放进 PATH 头部（探针读到 `/usr/bin/grep`）。所以 0a 那段是**纵深防御**（只在手工调 `sh` 时
  可达），B10 改成钉住这条前提本身：哪天 git 不再自带这些工具，B10 会先红并指向真正的原因。
- **回滚档**：`git-hooks/pre-commit`、`scripts/test-git-hooks.ps1`、`scripts/install-git-hooks.ps1`、
  `docs/34` §3.5。`git config --unset rustlua.scope` 回默认，`= worktree` 回旧口径，无需改代码。
- **验证**：`test-git-hooks.ps1` **58 例全绿**；`sh -n git-hooks/pre-commit` 通过；hook 仍 LF 无 BOM；
  `check-docs.ps1` 的 `scripts/*.ps1` BOM 与解析门通过（30/30）。

## Unreleased — `verify.ps1 -Quick`：把「这棵树现在红不红」压到 6.6 秒

主题：**门的覆盖面与门的成本错配**。pre-commit hook 只镜像 CI 的 lint job（`git diff --cached --check`
+ fmt + clippy），而真正咬人的**数据级工程校验**（`game --check` / `editor --check`：场景引用的资源、
脚本语法、`validate_data.lua`、组件 schema）此前只存在于总门里；总门要几十分钟到几小时，于是没人愿意
为「看一眼树是不是红的」付这个成本。代价已经记过两次：`scenes/main.scene` 的一句中文注释让
`game --check` 判红并在本地 HEAD 里活了 **5 个提交**（见下一条），以及 docs/34 §6 的
「门禁波动（并行会话）」——并行会话改写示例时总门因别人的半成品判红，读的人分不清是回归还是在途。

- **跑什么**：文档门 / vendor 补丁门 / `cargo metadata` / fmt / Lua API 文档桩 / Web API 覆盖账 /
  `git diff --check` / 两个判据自测 / `game --check` / `editor --check`（两工程）。
- **跳过什么（每一步都在结尾逐个打印）**：clippy、rustdoc、对外 Rust 符号面门（**与 rustdoc 一起跳**——
  它读的正是 `cargo doc` 刚生成的产物，只跳 doc 会拿上一次的旧产物当证据）、pre-commit hook 回归、
  全量测试、构建、空间迁移报告、编辑器性能基线、GUI smoke。本仓库口径是「跳过绝不等于通过」，
  所以它不打印 `Quality gate passed`，只打印 `Quick gate passed (PARTIAL)`。
- **清单不可能与 guard 漂移**：跳过判定走 `Test-QuickSkip "<步骤名>"`，步骤名写在 guard 那一行上、
  登记是它的副作用；脚本中途判红时清单会**自曝**「只登记到脚本走到的位置，清单并不完整」。
- **顺带修正**：`cargo metadata` 往 stdout 倒的整份 workspace JSON（几十 KB 机器可读噪音）改为成功时
  抑制、判红时照原样打出（`Invoke-Checked -QuietOutput`）——它此前会把质量日志里真正的红淹掉。
- **回滚档**：单文件（`scripts/verify.ps1`）、纯增量开关；不带 `-Quick` 时行为不变（唯一例外是上面那条
  metadata 输出抑制）。与 `-Mvp`/`-MvpOnly`/`-Benchmarks`/`-Web`/`-WasmLua` 互斥，混用当场判错。
- **验证**：`-Quick -Mvp` 混用立即判错且不跑任何 cargo；在本机工作树上实测 **6.6 秒**判红并退出 1，
  红在 `cargo fmt --all -- --check`——元凶是并行会话的 3 个未跟踪示例文件（`jhyt_m66_pause_prefab.rs`
  / `jhyt_m67_inventory_prefab.rs` / `probe_title_render.rs`），不是本次改动；quick 段其余每一步
  （文档门 / vendor 门 29 条 / metadata / Lua doc stubs / Web API 覆盖账 / `git diff --check` /
  两个自测 / `game --check` / `editor --check` 两工程）已逐条单独跑过，全绿。

## Unreleased — 场景注释不再冒充依赖：`game --check` 的幻影资源引用

主题：**门面场景的一句中文说明让整条质量门判红**。`scenes/main.scene:8`（M63b 的搬迁说明）
把 `assets/prefabs/title_menu.prefab` 写进了反引号、以全角逗号收尾；而场景/预制的依赖提取是
"任意位置扫全文"，终止符只有 `;` `:` 换行三种，于是截出的"引用"自带反引号与全角逗号，
`verify_project` 的 `is_file()` 必然为假——`cargo run -p game -- --project examples/game_jhyt --check`
报"场景引用的资源缺失"并以 1 退出，`verify.ps1` 与 CI 的 Game project check 同步判红。
为什么值得单独记一笔：这条红**不在任何代码改动里**，它是"注释里提了一句文件名"造成的，
从 diff 上完全看不出来。

- **注释不是数据**：`extract_scene_assets` 改为逐行扫描并跳过 `#` 行首注释，判定与两个真正的
  解析器逐字同源（`Scene::from_text` 与 prefab 解析器都跳过 `line.starts_with('#')`）。图集那条
  路径早就按同一原则处理（注释里的 `texture=assets/x.png` 不算边），本次只是把场景/预制补齐。
- **反面对照**：新增 `scene_comment_lines_do_not_produce_refs`（注释行整行跳过 + 同文件里的真实
  引用一个不能少 + prefab 同款）与 `comment_only_when_hash_leads_the_line`（行内 `#` 是图集区域
  后缀、不是注释）。把修复临时改回旧行为后该用例实测红，红出来的 `left` 逐字节就是门报的那条
  幻影路径（`assets/prefabs/title_menu.prefab` + 反引号 + 全角逗号）。
- **回滚档**：单文件单函数（`engine/crates/rustlua-assets/src/dependency.rs`），连同两条用例一起
  revert 即可；无格式迁移、无数据迁移。
- **同类隐患**：`examples/game_api` 的 `api_tilemap.scene` 与 `ui_test.scene` 注释里也有裸
  `assets/…` 路径，同一刀一并覆盖。

## Unreleased — 优化项 11：本地 pre-commit 门（提交前跑 CI 的 fmt/clippy），门自身进 CI

主题：CI 的 `lint` job 是 `build-test` / `gates` / `web-evidence` 三个 job 的 `needs` 前置，它一红整条
流水线什么都不跑。而 lint 失败（格式、`clippy -D warnings`、行尾空白）在本地零成本就能判定——推到
CI 才发现的代价是"提交—等—红—改—再推"一整轮。本次把这三条搬到 `git commit` 之前。

- **`git-hooks/pre-commit`**：版本库内的原生 git 钩子，用 `core.hooksPath` 指过去，因此它**被跟踪、能
  review、改了所有人跟着生效**，而不是各人 `.git/hooks/` 下互不可见的副本。跑 CI lint job 的同一批命令；
  暂存内容没碰 `engine/**` 的 `.rs/.toml/.lock` 时整段跳过（改文档不该等编译门）。
- **没有选 cargo-husky / pre-commit 框架**，是当场比过本机事实后否掉的：cargo-husky 要把第三方
  build script 塞进 workspace 成员，而 `engine/Cargo.toml` 目前是虚拟清单；pre-commit 框架要求队友先
  `pip install`（本机 Python 在、`pre-commit` 不在），且仓库所有门禁的既有口径是 `scripts/*.ps1` + 退出码。
  代价说清楚：原生 hook 要每人跑一次安装脚本，装没装从结果上看不出来——所以配了 `-Status` 只读体检。
- **档位与逃生门都留了口子，但措辞按"跳过"而不是"通过"来写**：clippy 默认 `fast`（不带 `--all-features`，
  冷缓存时那一档要重编整个 feature 图），`git config rustlua.clippy full` 与 CI 逐字对齐；
  `RUSTLUA_SKIP_PRECOMMIT=1` / `rustlua.hooks off` / `--no-verify` 都会打印 `SKIPPED` 并说明失败留给 CI。
  PATH 上找不到 cargo 时**判红而不是放行**——跑不了的门禁不能算测过。
- **`scripts/test-git-hooks.ps1`**：门自身的回归，进 CI 的 lint job 与 `verify.ps1` 常规门。它在临时 git
  仓库里**真的执行提交**，用退出码和 HEAD 是否移动断言拦与放（含"修好格式后同一份改动必须放行"，
  否则门会把人焊死在里面），另外反查 `quality.yml` 的命令原文比对——CI 改了 clippy 参数而 hook 没跟上
  会直接判红。为什么非要真提交：只 grep 脚本文本的话，"暂存区过滤规则写错导致门永远跳过"这种最坏形态
  恰好测不出来。
- **`.gitattributes`（新增）**：`git-hooks/* text eol=lf`。Git for Windows 装在
  `C:\Program Files\Git\etc\gitconfig` 的 `core.autocrlf=true`（本机实测就在这）会把 sh 钩子检出成 CRLF，
  表现为"hook 装了却每次提交都报 `bad interpreter: /bin/sh^M`"。刻意不加 `* text=auto`：那会把
  `scripts/*.ps1` 既有的 UTF-8 BOM + CRLF 口径一次性改成满屏脏 diff。安装脚本也会顺手把工作区里的
  CRLF 修回 LF。
- 装完第一次真跑就抓到东西：`engine/crates/engine/src/core/lod.rs` 当前未提交的改动里有一条超行宽的
  `#[error("…")]`，`cargo fmt --all -- --check` 判红——正是本项要消掉的那类循环。

细节与已知边界（cargo 读工作区而非暂存区、CI 的 `git diff --check` 在干净检出上恒真所以本地改用
`--cached`、GUI 客户端 PATH 里没有 cargo 怎么办）记在 [docs/34](../34-质量门与发布清单.md) §3.5，
目录职责见 [docs/23](../23-工程目录功能说明.md)。

回滚档：本提交之前。回滚只影响提交前的本地等待与 CI 里那一 step，不影响任何运行行为
（`git config --unset core.hooksPath` 或跑 `install-git-hooks.ps1 -Uninstall` 即可停用钩子）。
