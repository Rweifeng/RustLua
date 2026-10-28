//! 编辑器各面板：每个面板一个文件，`impl EditorApp` 中定义绘制与支撑方法。

#[cfg(feature = "ai-bridge")]
pub(crate) mod assistant;
pub(crate) mod atlas;
/// P1-3b：断链检查窗（全工程引用审计 + 同名唯一命中的一键改指）。
pub(crate) mod broken_refs;
pub(crate) mod bundles;
pub(crate) mod console;
pub(crate) mod domain_data;
pub(crate) mod game_view;
/// P3-E03-2：可视化脚本（`.vgraph`）画布与编译诊断。
pub(crate) mod graph;
pub(crate) mod header;
pub(crate) mod hierarchy;
pub(crate) mod input_bindings;
pub(crate) mod inspector;
/// egui 键 ⇄ 引擎物理键的唯一转换层（M22/W1）。
pub(crate) mod keymap;
/// P3-E02：Inspector 里的 `.material` 编辑段（只发 typed action）。
pub(crate) mod material;
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
/// P1：贴图双击预览窗（全尺寸解码 + 逐像素开关，只读）。
pub(crate) mod texture_preview;
pub(crate) mod tilemap_tools;
/// P3-E01-3：`.anim` 时间线面板（只读预览）。
pub(crate) mod timeline;
pub(crate) mod toolbar;
/// 与模型无关的树控件原语（缩进/行标签/拖放落点），场景层级与 UI 工作室共用。
pub(crate) mod tree;
/// M76-04：UI Studio（画布/令牌/资产/提案箱；节点树与检查器内嵌在通用面板里）。
pub(crate) mod ui_studio;
/// M81：UI 节点树，内嵌在通用层级面板里（UI 工作室在前台时接管层级面板）。
pub(crate) mod ui_studio_hierarchy;
/// M80：UI 节点检查器，内嵌在通用属性面板里（UI 工作室在前台时接管属性面板）。
pub(crate) mod ui_studio_inspector;

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
