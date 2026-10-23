//! 玩法域 crate 的**域账本**（P5-01；P5-02 后 2026-10-21 复核）。
//!
//! 名字叫 gameplay，但这里只有 **1,968 行**（非空，全 crate 4,875 行的 40.4%）是玩法数据模型；
//! 具体玩法规则（种植、战斗、商店、任务、对话、经济）全部在工程 Lua 里
//! （`examples/game_jhyt/scripts`：155 个文件 / 37,489 非空行），Rust 侧不实现规则。
//!
//! 口径：非空行，含测试与文档注释（与 `tasks/done/P5-玩法层边界与Lua下沉.md` §2.1 同口径）。
//!
//! | 域 | 文件（非空行） | 是不是玩法 |
//! |---|---|---|
//! | 玩法数据模型 | `gameplay.rs` 425、`clock_lua.rs` 403、`grid.rs` 91、`grid_lua.rs` 69、`inventory_state.rs` 391、`inventory_lua.rs` 589 = **1,968** | ✅ 数据模型与绑定 |
//! | 设置 | `settings.rs` **1,295** | ❌ 工程 `settings.toml`；编辑器/输入/音频共用 |
//! | 存档 | `save.rs` 301、`save_lua.rs` 317、`save_store.rs` 143 = **761** | ❌ 格式版本、原子写、平台槽 |
//! | 世界数据 | `world_data_lua.rs` 486、`world_data_state.rs` 82 = **568** | ❌ `data.load` 缓存/schema、`map.state` 持久键 |
//! | 绑定胶水 | `audio_lua.rs` 88、`input_lua.rs` 149 = **237** | ❌ 别的领域的注册入口 |
//! | crate 级 | `lib.rs` **46** | —（本账本） |
//!
//! ## 分层裁定
//!
//! - **留 Rust（服务/持久化）**：`save` / `data` / `map.state`——格式、原子写、平台槽与
//!   文件系统只有 Rust 有；Lua 只决定"存什么"。
//! - **留 Rust（ECS 身份）**：`health` / `tag` / `interact`——状态在 `EntityState`，
//!   `entity.destroy` 与切场景由引擎清理；Lua 镜像必然产生 staleness。
//! - **留 Rust（跨 VM 生命周期）**：`clock` / `calendar` / `item` / `inv` / `container`——
//!   热重载会替换 VM（docs/19），固定步进在 Lua `update` 之后推进；搬走需要跨 VM 状态桥。
//! - **参数化**：`calendar` 的 28 天/季 × 4 季/年是农场体裁默认值，不是引擎语义；
//!   `calendar.configure{days_per_season, seasons_per_year}` 改派生口径（P5-02）。
//! - **通用换算**：`grid` 是坐标约定的唯一来源（docs/06），与 tilemap/camera 同维护。
//! - **废弃候选**：`container` / `interact` / `tag` 在出货工程 0 静态调用点（2026-10-20 审计），
//!   保留给 `game_api` 与行为契约；删除需 v0.2 兼容窗口。
//!
//! 判定口径与触发条件见 `tasks/done/P5-玩法层边界与Lua下沉.md`；Lua 面分层见 `docs/18-lua-api.md`。

pub mod audio_lua;
pub mod clock_lua;
pub mod gameplay;
pub mod grid;
pub mod grid_lua;
pub mod input_lua;
pub mod inventory_lua;
pub mod inventory_state;
pub mod save;
pub mod save_lua;
pub mod save_store;
pub mod settings;
pub mod world_data_lua;
pub mod world_data_state;
