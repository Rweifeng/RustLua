//! 预制体结构差异窗（只读）：变体 ↔ 基础、嵌套实例 ↔ 内层源。
//!
//! 口径：
//! 1. **只看不写**：没有"接受/合并/同步"按钮；改结构仍然走既有路径（或手工重建）。
//!    窗口存在的意义是让"为什么同步被拒绝"在动手之前就可查。
//! 2. **条目可定位**：候选侧实体就在当前文档里时，点一行只改选区（层级高亮），
//!    不改文档、不进历史、不写盘。
//! 3. **结果是快照**：在打开窗口 / 点「重新计算」的那一刻算一次，不每帧读盘；
//!    文档改过之后按钮就在手边。

use crate::EditorApp;
use crate::actions::prefab_structure_diff::{StructureDiffSection, StructureDiffTarget};
use eframe::egui;

/// 窗口状态（挂在 `EditorApp` 上；`target` 决定「重新计算」回到哪个对象）。
#[derive(Default)]
pub(crate) struct PrefabStructureViewState {
    pub show: bool,
    pub target: Option<StructureDiffTarget>,
    pub sections: Vec<StructureDiffSection>,
    /// 整体说明（没有打开文档 / 没有任何谱系 / 读文件失败）。
    pub message: String,
}

impl PrefabStructureViewState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 切换工程/文档：上一次的结果不再属于这里。
    pub fn clear(&mut self) {
        *self = Self::new();
    }
}

impl EditorApp {
    pub(crate) fn draw_prefab_structure_panel(&mut self, context: &egui::Context) {
        if !self.prefab_structure.show {
            return;
        }
        let mut open = true;
        egui::Window::new("预制体结构差异")
            .id(egui::Id::new("prefab_structure_window"))
            .open(&mut open)
            .default_width(720.0)
            .default_height(420.0)
            .resizable(true)
            .show(context, |ui| {
                self.draw_prefab_structure_body(ui);
            });
        self.prefab_structure.show = open && self.prefab_structure.show;
    }

    fn draw_prefab_structure_body(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui
                .button("重新计算")
                .on_hover_text("按当前文档（或资源面板里点开的那份文件）重新比较一次")
                .clicked()
            {
                self.refresh_prefab_structure_diff();
            }
            ui.label(
                egui::RichText::new("只读：这里只列差异，不合并、不写盘")
                    .small()
                    .weak(),
            );
        });
        if !self.prefab_structure.message.is_empty() {
            ui.label(
                egui::RichText::new(self.prefab_structure.message.as_str())
                    .small()
                    .weak(),
            );
        }
        if self.prefab_structure.sections.is_empty() {
            return;
        }
        ui.separator();
        let mut locate: Option<rustlua_scene::scene::SceneEntityId> = None;
        egui::ScrollArea::vertical()
            .id_salt("prefab_structure_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for section in &self.prefab_structure.sections {
                    draw_section(ui, section, &mut locate);
                    ui.separator();
                }
            });
        if let Some(entity) = locate {
            self.locate_structural_change(entity);
        }
    }
}

fn draw_section(
    ui: &mut egui::Ui,
    section: &StructureDiffSection,
    locate: &mut Option<rustlua_scene::scene::SceneEntityId>,
) {
    ui.label(egui::RichText::new(&section.title).strong());
    if let Some(note) = &section.note {
        ui.colored_label(egui::Color32::from_rgb(220, 150, 60), note);
        return;
    }
    if section.changes.is_empty() {
        ui.label(
            egui::RichText::new("结构一致（字段级差异不在这个视图里）")
                .small()
                .weak(),
        );
        return;
    }
    ui.label(
        egui::RichText::new(format!("{} 项结构差异", section.changes.len()))
            .small()
            .weak(),
    );
    for change in &section.changes {
        ui.horizontal_wrapped(|ui| {
            ui.label(&change.detail);
            match change.candidate {
                Some(entity) if section.locatable && ui.small_button("定位").clicked() => {
                    *locate = Some(entity);
                }
                _ => {}
            }
        });
    }
}
