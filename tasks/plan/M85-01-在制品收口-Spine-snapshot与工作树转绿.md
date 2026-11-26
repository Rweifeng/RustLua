# 任务：在制品收口 —— Spine snapshot 与工作树转绿

- 任务 ID：M85-01
- 状态：已验收（2026-11-26；批次归档见 M85-00 §归档记录）。4 道红门全部转绿，`-Quick` 通过；
  在制品开工前已备份到 `artifacts/wip-backup-20261126/`（10 文件 + SHA256SUMS.txt）
- 关联里程碑：M85
- 创建日期：2026-11-26

## 目标

把当前未提交的 Spine `snapshot` 能力收口成**可提交、门全绿**的状态：4 道失败门全部转绿，
新 API 的文档/示例/基线/测试/计数同步，`tools/spine-spike/` 的 891 MB 临时探针有明确去处，
CHANGELOG 留痕。**本包不新增能力**，只把已有在制品补齐到仓库契约。

## 背景与证据（2026-11-26 实测）

工作树：9 个已修改文件 + 未跟踪 [snapshot.rs](../../engine/crates/rustlua-spine/src/snapshot.rs)（300 行软光栅化）
与 `tools/spine-spike/`（1,879 文件 / 891 MB，其中 887 MB 是已被 `.gitignore` 的 `target/`）。

| 门 | 命令 | 实测结果 |
|---|---|---|
| clippy | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | 红：`snapshot.rs:321` `field_reassign_with_default`；`spine_lua.rs:287` `type_complexity` |
| 运行期 API 面基线 | `cargo test -p rustlua-scripting --lib api_surface_snapshot` | 红：基线 442 / 当前 444（`spine.snapshot`、`spine.snapshot_frames`） |
| 示例覆盖率门 | `cargo test -p engine --lib sample_project_demos_cover_every_lua_api` | 红：`示例工程缺少 2 个 API 的演示（共 444 个）` |
| 文档桩门 | `node scripts/docgen/gen-lua-api-docs.js --check` | 红：`dead_bindings=2: spine.snapshot, spine.snapshot_frames`；[spine.md](../../engine/crates/engine/src/lua_api/doc/spine.md) 头仍写"8 个函数" |

已核对的事实：
- 新绑定在 [spine_lua.rs](../../engine/crates/rustlua-spine/src/spine_lua.rs) 注册，软栅格化在 `snapshot.rs`
  （按 draw 顺序画家算法、图集页按需解码缓存、PMA 先反预乘，写出 RGBA PNG）；
- 新增依赖 `image 0.25`（`default-features=false, features=["png"]`，见 `rustlua-spine/Cargo.toml`）；
- `snapshot.rs` 目前只有 **2 个单测**（`#[test]` 位于 :310/:317），覆盖不到退化输入与像素断言；
- 文档缺口：[docs/18:337](../../docs/18-lua-api.md#L337) 的 spine 支持范围表没有这两个函数；
- 生产侧证据：该能力已在 `D:\GitHub\RustLuaSword` 实际用于"38 武将全量烘焙约 3 秒"（仓库外，引用带来源）。

## 动作拆分

> 开工前**先备份工作区**（避免误操作丢在制品）：把 `git status --porcelain` 列出的文件复制到
> `artifacts/wip-backup-<date>/`，逐文件哈希比对后再动手。仓库已有先例
> （`artifacts/foreign-wip-backup-20261125/`）。

1. **修 2 条 clippy**
   - `snapshot.rs:321`：把 `Default::default()` 后的字段赋值改成一次结构体字面量初始化
     （或 `..Default::default()` 合并），不改变功能。
   - `spine_lua.rs:287`：把 7 元组参数抽成 `struct SnapshotFramesArgs` / `type` 别名，消除 `type_complexity`。
   - 验收：`cargo clippy -p rustlua-spine --all-targets --all-features -- -D warnings` exit 0；
     `cargo clippy --workspace --all-targets --all-features -- -D warnings` exit 0。

2. **更新运行期 API 面基线 442 → 444**
   - 全量面：`cargo run -p engine --example dump_lua_api -- --write`
   - 2D 面：`cargo run -p engine --example dump_lua_api --no-default-features -- --write`
     （写入 `lua_api_surface_2d.json`；若 2D 面不含 `spine` 表，则该文件不变，需在实现记录里写明）
   - 验收：`cargo test -p rustlua-scripting --lib --all-features api_surface_snapshot` 全绿。

3. **补示例工程演示（2 个函数）**
   - 位置：[demo_spine.lua](../../examples/game_api/scripts/demo_spine.lua)（已有 spine Lab，每步必须**真的调用一次**，注释不算）。
   - `spine.snapshot(id, w, h, path)`：烘焙当前姿态到 `examples/game_api/.temp/` 下的 PNG，断言返回的绝对路径文件存在且 > 0 字节；
   - `spine.snapshot_frames(id, w, h, dir, n, dt, stem)`：烤 3 帧小尺寸序列，断言返回数组长度 = 3 且文件都在。
   - 注意 headless 规则：写盘只能写工程 `.temp/`，不写仓库内其它路径、不切场景。
   - 验收：`cargo test -p engine --lib --all-features sample_project_demos_cover_every_lua_api` 全绿
     （`示例工程缺少 0 个 API`）。

4. **补文档桩并重新生成**
   - `node scripts/docgen/gen-lua-api-docs.js`（生成/增量更新）→ 新增两个 `## \`spine.snapshot…\`` 小节；
   - 在两个新小节里把 `TODO(文档桩)` 换成真实说明（参数、返回、失败语义、示例），
     **不能只留桩**——桩数必须保持 282（否则 `--check` 判红）；
   - 同步 [spine.md](../../engine/crates/engine/src/lua_api/doc/spine.md) 头部的"8 个函数"→"10 个函数"
     （由生成器自动改写，人工核对）；
   - 验收：`node scripts/docgen/gen-lua-api-docs.js --check` 输出 `lua_api_docs_ok`、`stubs=282`、无 `dead_bindings`。

5. **回写文档与计数（所有受门约束的 442）**
   受门约束文件（`check-doc-claims.mjs` 的 CLAIMS）：`README.md`、`docs/59`、`docs/62`（两处格式）、
   `docs/74`（三处格式）、`docs/demos/README.md`、`docs/README.md`、`docs/60`。
   另需人工同步：`docs/81`（两处）、`docs/18` 支持范围表（spine 小节加 snapshot 两行 + 已知限制）。
   - 验收：`node scripts/verify/check-doc-claims.mjs` 输出 `doc_claims_ok ... funcs=444`。

6. **补 `snapshot.rs` 单测**（当前只有 2 个）
   - 退化输入：零尺寸 / 1×1 / 空 draws / 顶点全重合（退化三角形）不 panic，返回明确错误或空图；
   - 像素断言：小尺寸（如 4×4）纯色四边形，断言输出 PNG 尺寸与中心/边角像素；
   - PMA 往返：`premultiplied_alpha=true` 时先反预乘，输出不出现"越界色"；
   - 临时目录用 `std::env::temp_dir()` + 唯一 token（仓库既有隔离约定）。
   - 验收：`cargo test -p rustlua-spine` 全绿（当前 15 条基线之外新增用例）。

7. **处置 `tools/spine-spike/`**
   - 推荐（S）：把整目录加入 `.gitignore`（现在只忽略 `target/` 与 `out/`？实测只有 `target/`），
     并在 [M84-02](../../tasks/done/M84-02-P1前置门-官方Golden对拍报告.md) §6 的复现说明处补一句
     "该目录为本地探针，不入库；重建命令见本节"。理由：`assets/upstream-repo/` 是上游 spine-ts 源码
     （第三方，含 498 KB 生成物），入仓会违反本仓"第三方素材不进库"的口径。
   - 备选（M）：只入库 runner（`golden/run_golden_gate.py`、`golden/golden.mjs` 的摘要、`Cargo.toml`），
     明确排除 `assets/`、`out/`、`golden/spine-core.mjs`。
   - 验收：`git status --porcelain` 不再出现 `?? tools/spine-spike/`（或只剩明确入仓的少量文件）；
     `git add -A` 不会把 891 MB / 1,879 文件带进索引（`git status --porcelain | Measure-Object` 核对）。

8. **CHANGELOG 条目 + 提交**
   - 在 [docs/changelog/最新条目.md](../../docs/changelog/最新条目.md) 顶部加一条
     `Unreleased — Spine 姿态烘焙（`spine.snapshot` / `spine.snapshot_frames`）`，写清：
     能力、门（4 条读数）、边界（不支持 clipping、图集页需可读盘、不适合 60fps 实时）、回滚档（删绑定 + 还原基线）。
   - 提交信息带门读数（clippy 0 / 444 / demos 0 missing / docgen ok / -Quick 绿）。
   - 验收：提交后 `git status --porcelain` 为空；`git log -1 --stat` 无 `tools/spine-spike/assets`、无 `target/`。

## 影响文件

- `engine/crates/rustlua-spine/src/snapshot.rs`（clippy + 测试）
- `engine/crates/rustlua-spine/src/spine_lua.rs`（clippy：参数结构体）
- `engine/crates/rustlua-spine/Cargo.toml`（`image` 依赖已在在制品中；确认 `deny/license` 允许）
- `engine/crates/rustlua-scripting/src/api_surface_snapshot/lua_api_surface.json`（及 `_2d.json`）
- `engine/crates/rustlua-scripting/src/api_surface_snapshot/rustlua_api.d.lua`（由生成器改写）
- `engine/crates/engine/src/lua_api/doc/spine.md`、`engine/crates/engine/src/lua_api/doc/baseline.json`
- `examples/game_api/scripts/demo_spine.lua`
- `README.md`、`docs/README.md`、`docs/18-lua-api.md`、`docs/59`、`docs/60`、`docs/62`、`docs/74`、`docs/81`、`docs/demos/README.md`
- `.gitignore`（spike 处置）
- `docs/changelog/最新条目.md`
- `tasks/plan/M85-00-下一阶段优先级总览与依赖顺序.md`（状态列回写）

## 风险

- **在制品丢失**：工作区里是唯一副本，任何 `git checkout/restore/stash` 前必须先备份（见开工前置）。
- **`image` 依赖的供应链面**：新依赖要过 `cargo deny` 的 licenses/bans 与 SBOM；若许可证不在 allow-list，
  需按 docs/69 的例外格式登记**带到期日的理由**，不能直接放宽 allow-list。
- **wasm 面**：`rustlua-spine` 进 `game_web` 的依赖图，新增 `image`（png）可能影响 wasm 体积；
  验收必须包含 `cargo check -p game_web --target wasm32-unknown-unknown`（需 `$env:ZIG`）。
- **文档计数是门**：442→444 有 7 个文件受 `check-doc-claims` 约束，漏改一个就红；以门的输出为准，不靠记忆。
- **不要顺手改其它在制品**：本包只碰 Spine snapshot 相关文件；工作区若有其它会话改动，按"只登记不动"处理。

## 验收方式

```powershell
cd engine
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p rustlua-spine
cargo test -p rustlua-scripting --lib --all-features api_surface_snapshot
cargo test -p engine --lib --all-features sample_project_demos_cover_every_lua_api
cargo check -p game_web --target wasm32-unknown-unknown   # 需 ZIG 指向 engine/.tools/zig
cd ..
node scripts/docgen/gen-lua-api-docs.js --check
node scripts/verify/check-doc-claims.mjs
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/verify/verify.ps1 -Quick
git status --porcelain
git diff --check
```

全绿判据：clippy exit 0；spine 测试全绿；API 面 444/444 一致；示例缺 0；docgen `lua_api_docs_ok`；
`check_doc_claims_ok ... funcs=444`；`-Quick` 收尾 `Quick gate passed`；`git status` 为空。

## 实现记录

2026-11-26 收口（承接并行会话的在制品；开工前已备份到 `artifacts/wip-backup-20261126/`，
10 个文件 + `SHA256SUMS.txt`）：

1. **clippy 两条 + 一条被遮蔽的红**：`snapshot.rs` 的单测从 `Default::default()` 后逐字段赋值改为
   结构体字面量（`field_reassign_with_default`）；`spine_lua.rs` 的 7 元组**没有**抽 `type` 别名
   ——实测 `gen-lua-api-docs` 会把标题退化成 `(((id, width, …): SnapshotFramesArgs))`，
   改为函数级 `#[allow(clippy::type_complexity)]`（先例：`push_image_quad` 的
   `too_many_arguments`），注释写明原因。同时发现并修掉 **`script_packs.rs:503` 的
   collapsible if**（在制品引入，之前被 spine 两条错误遮蔽）→ 改成 let-chain。
2. **API 面基线**：`dump_lua_api --write` **不可用**（实测只装部分领域、写出 32 表/242 函数，
   会删 289 行；`.workbuddy/memory/2026-11-19.md` 记过这个坑）。改为按 `tables[].name`
   在两份基线（全量 + 2D）的 `spine.funcs` 末尾插入 `snapshot` / `snapshot_frames`。
   实测：全量与 2D 面都是 **444**，2D 面同样含 `spine` 表（`_2d.json` **需要**改）。
3. **示例演示**：`demo_spine.lua` 新增「姿态烘焙」步骤——`run` 只读姿态（遵守 headless 不写盘），
   写盘与断言放在 `check`（自检实跑）：单帧写 `.temp/spine-snapshot/pose.png`（9,585 B），
   2 帧序列写 `walk_000.png` / `walk_001.png`，三张 PNG 实测都生成。
4. **文档桩**：先修签名推断（见 1）再生成；`spine.md` 两节补真实说明（参数/返回/失败/示例），
   桩数 284 → **282**（与基线持平）、documented 160 → 162、无死绑定；`docs/18` 的函数表加两行 +
   支持范围表加「姿态烘焙」行（核对日期改 2026-11-26）。
5. **计数**：受 `check-doc-claims` 约束的 7 个文件 + `docs/81` 共 13 处 `442 → 444`。
6. **`tools/spine-spike/`**：选推荐方案 A——整目录进 `.gitignore`（原来只忽略 `target/`），
   注释写明 891 MB 含上游 spine-ts 源码与生成物、第三方素材不进仓，复跑命令指向
   M84-01 §8 / M84-02 §6；`git check-ignore` 已确认生效。
7. **CHANGELOG**：顶部新增一条（做了什么 / 为什么重要 / 门 / 边界 / 回滚档）。

**门读数（本机 2026-11-26）**：

- `cargo clippy --workspace --all-targets --all-features -- -D warnings` → exit 0
- `cargo test -p rustlua-spine` → 全绿（lib 10 条，含新增 5 条：退化三角形 / 不透明铺满 /
  预乘反预乘 / 零尺寸钳位 / PNG 往返）
- `cargo test -p rustlua-scripting --lib --all-features api_surface_snapshot` → 2 passed（444）
- `scripts/verify/check-api-2d.ps1` → exit 0（2D api_surface 7 passed、game 2D check、
  engine sample_project 2D 段 18 passed、2D LuaLS `--surface --check`）
- `cargo test -p engine --lib --all-features sample_project` → 18 passed
- `node scripts/docgen/gen-lua-api-docs.js --check` → `lua_api_docs_ok`（stubs=282、funcs=444）
- `node scripts/verify/check-doc-claims.mjs` → `doc_claims_ok … funcs=444`
- `scripts/verify/check-docs.ps1` → `check_docs_ok docs=75 links=468`
- `scripts/verify/verify.ps1 -Quick` → `Quick gate passed (PARTIAL)`，
  log `artifacts/logs/quality/verify-20261126-123049.log`。PARTIAL 是 `-Quick` 的固有口径：
  它明列跳过 clippy/测试/release/coverage/metrics，其中 **clippy 与相关测试本包已单独跑绿**
- `cargo fmt --all -- --check` → exit 0；`git diff --check` → ok

**提交**：`40cd1a71`（29 files，+838/−38）。

## 验收证据

- [x] `cargo clippy --workspace --all-targets --all-features -- -D warnings` 通过
- [x] `cargo test -p rustlua-spine` 通过（含新增退化/像素/PMA 用例）
- [x] `cargo test -p rustlua-scripting --lib --all-features api_surface_snapshot` 通过（444）
- [x] `cargo test -p engine --lib --all-features sample_project_demos_cover_every_lua_api` 通过（缺 0）
- [x] `cargo check -p game_web --target wasm32-unknown-unknown` 通过（含在 `-Quick` 的 wasm32 平台编译段）
- [x] `node scripts/docgen/gen-lua-api-docs.js --check` 通过（stubs=282、dead_bindings=0）
- [x] `node scripts/verify/check-doc-claims.mjs` 通过（funcs=444）
- [x] `verify.ps1 -Quick` 通过（PARTIAL；跳过的 clippy/测试已单独跑绿）
- [x] `check-api-2d.ps1` 通过（2D 面 444、2D LuaLS 一致）
- [x] `tools/spine-spike/` 已按推荐方案处置（整目录 `.gitignore`，`git check-ignore` 生效）
- [x] CHANGELOG 有本笔条目；提交信息带门读数
- [x] 注释符合 [章程 §9.7](../../docs/00-project-charter.md#97-代码与注释规范)（精简中文）

## 未完成项 / 已知限制

- **Spine 真机几何未验**：微信/WebGL2 的运行期几何仍只到 `wasm32` 编译通过（沿用 M84 登记，不在本包解决）。
- **clipping 仍不支持**：导入期硬拒绝（`reject_unsupported_features`），这是既有边界，本包不改。
- **PMA/Multiply/Screen 混合仍被拒**：`spine.snapshot` 的软栅格化对 PMA 做了反预乘，但**渲染路径**的 PMA
  仍在绘制时跳过——那属于 P1-01（见 [M85-00](M85-00-下一阶段优先级总览与依赖顺序.md) §四），不要在本包顺手改。
- **2D 基线口径（已实测）**：`--no-default-features` 的 API 面**含** `spine` 表，`_2d.json` 同样需要
  加两个函数（已加，2D 测试与 LuaLS 一致性均绿）。
