//! 编辑器各面板：每个面板一个文件，`impl EditorApp` 中定义绘制与支撑方法。

#[cfg(feature = "ai-bridge")]
pub(crate) mod assistant;
pub(crate) mod atlas;
pub(crate) mod bundles;
pub(crate) mod console;
pub(crate) mod domain_data;
pub(crate) mod game_view;
pub(crate) mod header;
pub(crate) mod hierarchy;
pub(crate) mod input_bindings;
pub(crate) mod inspector;
/// egui 键 ⇄ 引擎物理键的唯一转换层（M22/W1）。
pub(crate) mod keymap;
/// 性能分析面板（独立弹窗）：Play 运行时帧率/draw/setpass/内存/资源缓存 + 编辑器帧/面板耗时。
pub(crate) mod performance;
pub(crate) mod progress;
pub(crate) mod project_settings;
pub(crate) mod publish;
pub(crate) mod resource_browser;
pub(crate) mod runtime_debug;
pub(crate) mod scene_view;
/// schema 驱动的通用组件字段编辑器（docs/55 阶段 2：新增组件不改巨型 match）。
pub(crate) mod schema_inspector;
pub(crate) mod tilemap_tools;
pub(crate) mod toolbar;

/// 架构守卫：面板只发 typed action，不直接写文档（棘轮测试）。
#[cfg(test)]
mod architecture_tests;

/// `--no-default-features` 构建没有 AI 依赖图：助手标签仍存在（布局/菜单一致），
/// 但只显示一条说明而不是把整块工作区编译掉。
#[cfg(not(feature = "ai-bridge"))]
impl crate::EditorApp {
    pub(crate) fn draw_assistant_panel(&mut self, ui: &mut eframe::egui::Ui) {
        ui.weak("内置 AI 助手需要 `ai-bridge` 特性（默认构建已开启）。");
    }
}
