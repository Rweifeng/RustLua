//! 物理域：2D/3D 模拟世界、权威/同步契约、拾取-碰撞查询与 Lua 绑定（P1 ⑤-9）。
//!
//! | 模块 | 内容 |
//! |---|---|
//! | [`physics`] | 2D 模拟世界（Rapier 2D）：刚体/碰撞体、步进、查询、调试形状 |
//! | [`physics3d`] | 3D 模拟世界（Rapier 3D），与 2D 同形的推进与查询契约 |
//! | [`physics_state`] | 会话态：两个世界 + 分阶段回写队列 + 每帧调试请求 |
//! | [`physics_lua`] | `physics` / `physics3d` 两张表的绑定（19 个函数） |

pub mod physics;
pub mod physics3d;
pub mod physics_lua;
pub mod physics_state;
pub mod tilemap_lua;

#[cfg(test)]
mod physics_parity;
