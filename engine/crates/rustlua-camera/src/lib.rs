//! 统一相机：2D/3D 投影、2D 控制器、拾取共享链，以及脚本侧的相机绑定（P1 ⑤-8）。
//!
//! | 模块 | 内容 |
//! |---|---|
//! | [`camera`] | 相机类型与数学、控制器、每帧渲染契约（既有主体） |
//! | [`picking`] | 屏幕 → 射线 → 命中的共享链（从 `rustlua-render` 搬来，render 保留再导出） |
//! | [`camera_state`] | 相机会话态：显式选中 + 2D 控制器 + 每帧回填快照 |
//! | [`camera_lua`] | `camera` / `camera3d` / `camera2d` 三张表的绑定（19 个函数） |

pub mod camera;
pub mod camera_lua;
pub mod camera_state;
pub mod picking;
