//! 编辑器各面板：每个面板一个文件，`impl EditorApp` 中定义绘制与支撑方法。

pub(crate) mod console;
pub(crate) mod domain_data;
pub(crate) mod game_view;
pub(crate) mod header;
pub(crate) mod hierarchy;
pub(crate) mod input_bindings;
pub(crate) mod inspector;
/// egui 键 ⇄ 引擎物理键的唯一转换层（M22/W1）。
pub(crate) mod keymap;
pub(crate) mod progress;
pub(crate) mod project_settings;
pub(crate) mod publish;
pub(crate) mod resource_browser;
pub(crate) mod runtime_debug;
pub(crate) mod scene_view;
pub(crate) mod tilemap_tools;
pub(crate) mod toolbar;
