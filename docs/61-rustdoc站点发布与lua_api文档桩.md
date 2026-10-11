# rustdoc 站点发布与 Lua API 文档桩

优化项 10（见 [60-全面复盘与优化项清单.md](60-全面复盘与优化项清单.md)）落地：**rustdoc 能一键生成、能发布、且不新增断链；Lua 侧每个绑定函数都有文档条目，条目数与运行期 API 面基线锁死**。三件事各自的"事实源"不同，凑在一起做是因为它们共用同一批产物（`cargo doc`）与同一条门（零告警）。

## 一、三条命令

```powershell
node scripts/gen-lua-api-docs.js              # 生成/增量更新 Lua API 文档桩
node scripts/gen-lua-api-docs.js --check      # 门：再生成必须是 no-op、桩数不得变多
powershell -File scripts/gen-rustdoc.ps1      # cargo doc → 可发布的静态站点目录
```

`gen-rustdoc.ps1` 默认带 `RUSTDOCFLAGS=-D warnings`，产物落在 `artifacts/doc-site`（已在 `.gitignore` 的 `/artifacts/*` 里）。加 `-Open` 用默认浏览器打开，加 `-SiteDir <路径>` 换输出位置，加 `-AllowWarnings` 只用于排障（CI 与门都不带）。

## 二、rustdoc 站点

`cargo doc --no-deps --workspace --lib` 对多 crate workspace **不写根 `index.html`**，产物只有 `doc/<crate>/index.html`——直接把 `target/doc` 推上静态托管会得到 404 的站点根。所以脚本负责补三件事：根导航页（列出实际生成的 crate，并把 Lua API 参考置顶）、`.nojekyll`（GitHub Pages 默认走 Jekyll，会吞掉下划线开头的目录）、以及"根页与每个 crate 的 index 都在"的自检。

发布走 `.github/workflows/rustdoc.yml`：push 到 `main`（或手动 dispatch）时生成站点 → `actions/upload-pages-artifact` → `actions/deploy-pages`，站点地址是 `https://Rweifeng.github.io/RustLua/`。

**一次性人工前提**：仓库 Settings → Pages → Source 必须改成 **GitHub Actions**。本机没有 `gh` CLI、也没有仓库凭据，这一步没做也不能由脚本代做；没改时 `deploy-pages` 会显式报错而不是静默不发——这是刻意的，宁可红也不要"看起来成功了其实没发布"。

本地实测规模：5 个 crate 文档目录、1872 个文件、49.8MB（含源码浏览器），在 Pages 的 100MB 站点建议线内。

## 三、告警清零（78 → 0）

存量 78 条 rustdoc 告警（`engine` 77 + `editor` 1）全部是失效的 intra-doc 链接。根因不是笔误，而是 rustdoc 的两条解析规则：

1. **从私有子模块 `pub use` 上来的名字不进链接解析作用域**。`rustlua-camera/src/camera/mod.rs` 里 `mod component;` / `mod view;` 是私有的，条目页写在 `camera/component/struct.Camera2D.html`（定义位置），`camera/` 下那份是 re-export。于是文档里写 `[`Camera2D`]` 解析不到，必须写公开路径。
2. **文档链接要使用定义 crate 的真实公开路径**。例如相机 API 位于 `rustlua_camera::camera`，Lua 运行时位于 `rustlua_scripting::script`；编辑器、运行器和宿主均直接依赖对应的 `rustlua-*` crate。

判"哪些路径真的是公开的"没有靠人眼：`target/doc/**` 的目录结构本身就是权威答案——**含 `index.html` 的目录才是公开模块**，只含条目页的目录是私有模块（rustdoc 给私有模块写条目页但不写 index）。据此索引把 120 处裸链接机械限定成公开路径，剩下 10 处按性质逐条处理：

| 情形 | 处理 |
|---|---|
| `[`component`]` 等 6 个私有子模块名 | 降级成代码块（`component`），没有公开条目可链 |
| `[`WorldTransform`] / [`WorldTransform3D`]` | 指向已合并的 [`GlobalTransform`](../engine/crates/rustlua-ecs/src/spatial_transform.rs)（docs/60 那批的改名遗留） |
| `[`rustlua_scripting::script::LuaRuntime::with_script`]` | 该方法不存在；反向解码其实是私有 `entity_from_lua`，降级成代码块 |
| `[`Camera2D::visible_extent`]` | 私有方法，"public documentation links to private item" 告警，降级 |

改动全部是注释行，`git diff` 可按 `[`crate::` 复核。**没有为了消警而把私有项改成 `pub`**——那会让 API 面变大，属于用错工具解决问题。

## 四、Lua API 文档桩

形态：**rustdoc 页 + 再生成一致性门**。`scripts/gen-lua-api-docs.js` 从基线生成 `engine/crates/engine/src/lua_api/doc/<模块>.md`（41 个模块），`lua_api.rs` 里每个模块是一个只带 `#![doc = include_str!(…)]` 的空 `pub mod`，于是 `cargo doc` 产物里出现 `engine::lua_api::camera` 这样的参考页，函数名与签名各成一节、进侧边栏目录。

**事实源是运行期 API 面基线**（`core/api_surface_snapshot/lua_api_surface.json`，41 张表 / 275 个函数），不是绑定源码。理由与快照门 `lua_api_surface_matches_baseline` 一致：源码里 `create_function` 有 267 处，运行期真正暴露 275 个函数，只有以基线为主才能同时发现两类失真——

- **死绑定**：源码注册了、运行期 `_G` 里没有（docs/37 记过 `input.action` 被注册两次的同类坑）→ `--check` 判红；
- **解析器漏形状**：基线有、源码定位不到绑定点 → 打 `unmatched` 并把签名标成 `(...)`，提示解析器该修。

绑定源码里实测有**三种注册形状**，都覆盖了：直写（`camera.set("set_active", lua.create_function(…))`）、宏展开（`bind_events!(physics3d, …)` 一份实现进多张表，按调用点分别记账）、循环（`for (name, mode) in [("is_down", 0), …]`，名字取自元组字面量；必须取**最近的前置循环**，否则 `input.mouse_is_down` 会错认到第一个循环上）。另外字符串与 `create_function` 之间可能有注释行（`cargo fmt` 不移动注释），解析要容忍。签名从闭包参数表推断：`|lua, (x, y): (f32, f32)|` → `x: f32, y: f32`。

保留规则：人工在 `## \`模块.函数(…)\`` 小节里写下的说明会被保留（识别方式：正文不再含 `TODO(文档桩)`），模块标题与函数清单之间的散文当"模块说明"同样保留；再生成只补节、删节、重排签名行。

## 五、门落在哪

| 门 | 命令 | 位置 |
|---|---|---|
| 文档桩再生成一致性 | `node scripts/gen-lua-api-docs.js --check` | `verify.ps1`「Lua API doc stubs」+ `quality.yml` lint job |
| rustdoc 零告警 | `RUSTDOCFLAGS=-D warnings cargo doc --no-deps --workspace --lib` | `verify.ps1`「Rustdoc」+ `quality.yml` lint job |
| 站点发布 | `scripts/gen-rustdoc.ps1`（自带 `-D warnings` 与根页自检） | `rustdoc.yml` |

桩数基线 `src/lua_api/doc/baseline.json` 当前是 `{funcs: 275, stubs: 275, documented: 0}`。`--check` 有两档：`stubs > 基线` 判红（新增 API 却没补文档），`stubs ≠ 基线` 也判红并提示跑 `--update-baseline` 收紧——**补文档让桩数下降是好事，但必须显式承认新基线**，否则"下降"会变成没人核对的漂移。

`verify.ps1` 里 Lua API 那步在找不到 `node` 时打印 `[SKIP] …（不是通过）` 后继续，遵守本仓库"跳过绝不等于通过"的口径；CI 用 `actions/setup-node` 固定 node 20，不存在静默跳过。

## 六、边界（没做的事）

- **275 个函数目前全是桩**，一条人工说明都还没写；`documented=0` 是诚实的起点而不是完成态。签名是**从绑定源码推断**的，`Option<f32>` / `?` 这类标记需人工核对成 Lua 侧口径（Lua 里没有 `Option`，实际是"可省略参数"）。
- 文档桩只覆盖 `_G` 上的 41 张模块表；场景组件字符串、`project.toml` 字段、编辑器面板不在 `lua_api_surface.json` 里，各自有自己的 schema 门。
- `-D warnings` 只作用在 `--lib`；`--all-targets`（examples/benches/tests）没进门，因为 examples 的文档注释不是发布面，且会把一次性脚本的私有胶水也拉进断链统计。
- 本批**只动注释行**，因此不引入格式 diff：在提交树上 `cargo fmt --all -- --check` 实测退出码 0。写作过程中本机一度报 8 个文件的**存量**格式差异（`core/commands.rs`、`core/lod.rs`、`core/texture.rs`、`host/renderer.rs` 等在途文件），那属于并行进行中的另一批改动，没有顺手格式化——格式化别人的在途代码会把无关 diff 混进这次提交。
- Pages 的 Source 设置、以及"站点真的可访问"这两点未在本机验证（无 `gh`、无凭据），需要一次 push + Actions 观察。
- 本机 `cargo doc --no-deps --workspace --lib` 出现过**同一条命令一次报 9~19 个编译错、下一次全绿**的抖动（`?` 转换、`Self::init` 实参类型这类不该随机的错误）。`touch crates/engine/src/lib.rs` 强制全量重编后连续多次干净，怀疑是 Windows 上增量编译指纹的 mtime 抖动。CI 是干净 checkout，不受影响；本地若复现，`cargo clean -p engine` 再跑。
