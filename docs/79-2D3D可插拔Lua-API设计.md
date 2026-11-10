# 2D/3D 可插拔 Lua API 设计

## 目标与边界

2D 是基础 API 面；3D 是可选的增量 API 面。两者共用场景、实体、资源和 Lua VM，不维护两套引擎。
API 扩展按模块包组合，核心 2D/3D 语义仍由引擎维护，领域插件只经受控安装器增加 Lua 模块。

本设计的“可插拔”指 **Rust 编译期链接、VM 创建时组合**。它不承诺 `.dll`/`.so` 动态加载、
Rust 稳定 ABI、运行中热卸载，也不把进程内 Rust 代码当作沙箱。若未来需要不可信或无需重编
引擎的插件，应另行设计进程外协议及权限模型。

## 分层

```text
game / game_web / engine 组合根
  ├─ 选择构建面：默认 3D；无默认 feature 构建为 2D
  ├─ 组合 API 包：共享/2D/3D 包及其依赖
  └─ 创建 LuaRuntime
       ├─ 2D 核心 API：所有构建都具备
       ├─ 3D 核心 API：仅启用 `3d` feature 时编译和登记
       └─ API 包注册表：按依赖顺序装配领域扩展模块

rustlua-lua-api（无领域依赖）
  └─ ApiPack / ApiDimension / Registry
```

- `ApiDimension` 声明包归属，`Registry` 的 `ApiSurface` 声明宿主选择的 2D/3D 面；2D 注册表会在
  执行安装器前拒绝 3D 包。它们不代替 Cargo feature 做代码裁剪。构建 2D 版本时，3D 实现仍须由
  依赖链上的 `#[cfg(feature = "3d")]` 排除；仓库现有 2D API 快照和无默认 feature 验收门继续负责
  证明 3D Lua 表没有泄漏。
- `ApiPack` 在一个稳定 `id` 下声明包版本、API 契约版本、维度、依赖包和模块安装器。
- `Registry::new()` 默认创建 2D 安全面；3D 宿主必须通过 `Registry::for_surface(ApiSurface::ThreeD)`
  明确选择，避免遗漏设置导致 3D 包意外进入 2D 构建。
- `Registry` 在安装任何模块前校验包版本、依赖存在性、依赖循环和 Lua 模块重名，再以稳定拓扑序
  安装模块。缺失依赖或冲突会显式失败，不采用“跳过并继续”。
- 模块安装器通过闭包显式捕获宿主创建的 `Handle`/端口；领域 API 契约层不依赖 ECS、GPU、场景或
  具体玩法 crate。宿主决定可授予哪些能力。
- `LuaRuntime` 在新 VM 和热重载 VM 上重复装配同一注册表。热重载保留包及捕获句柄；卸载扩展的
  v1 操作是重建 VM/宿主，不提供伪卸载。

## 2D/3D 组装规则

1. 2D 核心 API 始终存在，是玩法和资源接口的稳定基线。
2. 3D 核心 API 是 `3d` 构建 feature 的增量：包体/构建链选择维度，不能在运行时开启未编译的 3D
   实现。
3. 共享模块只能有一个 Lua 表所有者；按需扩展共享表时，必须由该表的所有者提供受控安装点，不能
   再注册同名全局表。
4. 3D 专属领域模块以 `ThreeD` 包声明，并在 2D 构建时完全不登记。导航 `nav` 是一个实例：其
   XZ 网格 API 随 `navigation-3d` 包登记，并依赖共享 `gameplay-base` 包。
5. 新增 Lua 模块必须更新 API surface 快照、LuaLS 文档、示例覆盖率和 2D/3D 构建门；不能靠包名
   或文档约定代替运行期契约测试。

## 当前实现

`rustlua-lua-api` 已提供：

- `ApiPack`：版本化包描述、共享/2D/3D 归属、依赖和模块安装器；
- `Registry::for_surface`：显式选择构建面，2D 注册表拒绝 3D 扩展包；
- `Registry::add_pack` 与 `Registry::packs`：组合包并读取已登记包的可诊断清单；
- 安装前校验及依赖拓扑排序；同名/空模块、重复包 id、API 版本错误、缺失依赖和循环依赖均失败；
- 旧 `Registry::add` 保持兼容，作为无依赖的基础模块入口。

`rustlua-gameplay::input_lua::registry_with_handles` 已将 `audio`、`input`、`net`、`rollback` 组合为
共享 `gameplay-base` 包；3D 构建额外加入依赖它的 `navigation-3d` 包。其它 3D 核心 API 仍由
`rustlua-scripting` 的 `3d` feature 门控，保持现有 API 面和构建产物行为。

## 新增扩展步骤

```rust
let mut pack = ApiPack::new("weather", env!("CARGO_PKG_VERSION"), ApiDimension::Shared);
pack.module("weather", move |lua| install_weather(lua, &weather_handle));
registry.add_pack(pack);
```

如依赖另一包，调用 `with_dependency("package-id")`。依赖表示装配先后，不代表 Rust crate 依赖；
Rust crate 仍应通过叶子契约 crate 解耦，禁止依赖环。新增 3D 包时还须在包的注册点使用 `3d`
feature 门控，不能仅将维度标成 `ThreeD`。

## 暂不纳入

- 动态库、脚本插件或第三方安全沙箱；
- 运行中卸载、跨包共享可变 ECS 世界、直接访问 GPU 设备/渲染器内部；
- 允许插件覆盖核心模块、依赖版本范围解析、包级热更新；
- 将全部 3D 内部 API 重构成外部插件。核心 3D API 是可选构建面，但仍属于引擎受版本控制的核心契约。

这些能力只有在出现具体插件需求后再扩展，避免先承诺无法稳定维护的 ABI 或扩大宿主权限。
