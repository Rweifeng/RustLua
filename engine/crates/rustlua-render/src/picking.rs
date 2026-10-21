//! 拾取的共享投影/查询链——实现随相机数学搬进了 `rustlua-camera`（P1 ⑤-8）。
//!
//! 保留本模块与全部函数路径，是因为编辑器 3D 点选与宿主调试线框都从这里取（`rustlua_render::picking::*`）；
//! 搬家的动因见 [`rustlua_camera::picking`] 的模块文档：`camera3d.*` 绑定现在住在相机 crate 里，
//! 让相机 crate 反向依赖 render 才是错的依赖方向。

pub use rustlua_camera::picking::*;
