pub mod asset_import;
/// 表现层组件的载荷契约注册表（P1 ④）。
///
/// 实现住在叶子 crate `rustlua-component-codec`（本 crate 只做再导出，方便调用方写
/// `rustlua_scene::component_codec::install(...)`）。为什么需要那张表、未装表为什么是
/// 硬错误，都写在那个 crate 的模块文档里。
pub use rustlua_component_codec as component_codec;
pub mod prefab;
pub mod scene;
pub mod scene_migration;
pub mod scene_space;
pub mod typed_scene;
