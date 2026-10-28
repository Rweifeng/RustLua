//! 脚本层的**领域门面**：`script/` 下的生产代码只从这里认领域 crate。
//!
//! 为什么需要它：拆分后绑定散在 `script/api/*`、`scene_spawn`、`transition` 等十几处，
//! 每处各自 `use rustlua_ecs::…` / `use rustlua_ui::…`。依赖边本身没变（这些 crate 仍是
//! `rustlua-scripting` 的依赖），但**引用面**从 12 个 crate 名收敛成一个 `domain`：换名、
//! 合并领域、或把某个绑定搬回归属 crate 时，只改这张表，而不是全目录搜索替换。
//!
//! 边界规矩（由 `tests/domain_facade.rs` 的棘轮守住）：
//! - `script/` 下除本文件外，生产代码不得再出现 `rustlua_*` 直接引用；
//! - 新增领域 crate 先在这里登记别名，再在调用点用 `domain::<别名>`；
//! - 测试模块不受限（测试直接构造领域类型是合理的），但生产代码不享受这个豁免。
//!
//! 注意：这是**逻辑收敛**，不是依赖图合并。真正减少编译边要等绑定继续下沉到归属 crate
//! （`docs/62` P1 的方向）；在那之前，门面保证"谁依赖了哪些领域"只有一处可读。

pub(crate) use rustlua_animation as animation;
pub(crate) use rustlua_camera as camera;
pub(crate) use rustlua_ecs as ecs;
pub(crate) use rustlua_gameplay as gameplay;
pub(crate) use rustlua_infra as infra;
pub(crate) use rustlua_lua_api as lua_api;
pub(crate) use rustlua_math as math;
pub(crate) use rustlua_physics as physics;
pub(crate) use rustlua_render as render;
pub(crate) use rustlua_scene as scene;
pub(crate) use rustlua_ui as ui;
pub(crate) use rustlua_visual_script as visual_script;

/// 脚本层对宿主公开的运行期 UI 元素类型（路径 `script::UiBar` 等保持不变）。
pub use rustlua_ui::ui_state::{UiBar, UiDrag, UiText};
