//! Lua API 参考（**本文件与 `src/lua_api/doc/*.md` 由脚本生成，勿手改**）。
//!
//! - 事实源：运行期 `_G` 探针导出的 API 面基线（`rustlua-scripting` 的 api_surface_snapshot 快照门，
//!   `lua_api_surface.json`，42 个模块表 / 292 个函数）；
//! - 再生成：`node scripts/docgen/gen-lua-api-docs.js`；说明文字写在各模块的 markdown 里
//!   （`src/lua_api/doc/<模块>.md`），Lua 侧签名从绑定源码推断；
//! - 门：`node scripts/docgen/gen-lua-api-docs.js --check`——再生成必须是 no-op、待补桩数不得
//!   超标、不许出现死绑定与重复注册；
//! - 还没补的函数在小节里标着 `TODO(文档桩)`（计数见 `doc/baseline.json`）。
//!
//! 这些模块只带文档、不带任何条目：Lua 侧的名字才是脚本契约，Rust 侧不放同名函数，
//! 免得两边各活一份、互相说不清谁是真的。

pub mod animation {
    #![doc = include_str!("lua_api/doc/animation.md")]
}

pub mod animation3d {
    #![doc = include_str!("lua_api/doc/animation3d.md")]
}

pub mod app {
    #![doc = include_str!("lua_api/doc/app.md")]
}

pub mod asset {
    #![doc = include_str!("lua_api/doc/asset.md")]
}

pub mod audio {
    #![doc = include_str!("lua_api/doc/audio.md")]
}

pub mod calendar {
    #![doc = include_str!("lua_api/doc/calendar.md")]
}

pub mod camera {
    #![doc = include_str!("lua_api/doc/camera.md")]
}

pub mod camera2d {
    #![doc = include_str!("lua_api/doc/camera2d.md")]
}

pub mod camera3d {
    #![doc = include_str!("lua_api/doc/camera3d.md")]
}

pub mod clock {
    #![doc = include_str!("lua_api/doc/clock.md")]
}

pub mod container {
    #![doc = include_str!("lua_api/doc/container.md")]
}

pub mod data {
    #![doc = include_str!("lua_api/doc/data.md")]
}

pub mod day {
    #![doc = include_str!("lua_api/doc/day.md")]
}

pub mod entity {
    #![doc = include_str!("lua_api/doc/entity.md")]
}

pub mod event {
    #![doc = include_str!("lua_api/doc/event.md")]
}

pub mod grid {
    #![doc = include_str!("lua_api/doc/grid.md")]
}

pub mod health {
    #![doc = include_str!("lua_api/doc/health.md")]
}

pub mod input {
    #![doc = include_str!("lua_api/doc/input.md")]
}

pub mod interact {
    #![doc = include_str!("lua_api/doc/interact.md")]
}

pub mod inv {
    #![doc = include_str!("lua_api/doc/inv.md")]
}

pub mod item {
    #![doc = include_str!("lua_api/doc/item.md")]
}

pub mod light {
    #![doc = include_str!("lua_api/doc/light.md")]
}

pub mod log {
    #![doc = include_str!("lua_api/doc/log.md")]
}

pub mod map {
    #![doc = include_str!("lua_api/doc/map.md")]
}

pub mod mesh {
    #![doc = include_str!("lua_api/doc/mesh.md")]
}

pub mod particles {
    #![doc = include_str!("lua_api/doc/particles.md")]
}

pub mod physics {
    #![doc = include_str!("lua_api/doc/physics.md")]
}

pub mod physics3d {
    #![doc = include_str!("lua_api/doc/physics3d.md")]
}

pub mod prefab {
    #![doc = include_str!("lua_api/doc/prefab.md")]
}

pub mod quad {
    #![doc = include_str!("lua_api/doc/quad.md")]
}

pub mod save {
    #![doc = include_str!("lua_api/doc/save.md")]
}

pub mod scene {
    #![doc = include_str!("lua_api/doc/scene.md")]
}

pub mod shop {
    #![doc = include_str!("lua_api/doc/shop.md")]
}

pub mod sprite {
    #![doc = include_str!("lua_api/doc/sprite.md")]
}

pub mod tag {
    #![doc = include_str!("lua_api/doc/tag.md")]
}

pub mod tilemap {
    #![doc = include_str!("lua_api/doc/tilemap.md")]
}

pub mod time {
    #![doc = include_str!("lua_api/doc/time.md")]
}

pub mod transform {
    #![doc = include_str!("lua_api/doc/transform.md")]
}

pub mod transform3d {
    #![doc = include_str!("lua_api/doc/transform3d.md")]
}

pub mod ui {
    #![doc = include_str!("lua_api/doc/ui.md")]
}

pub mod visibility {
    #![doc = include_str!("lua_api/doc/visibility.md")]
}

pub mod world_text {
    #![doc = include_str!("lua_api/doc/world_text.md")]
}
