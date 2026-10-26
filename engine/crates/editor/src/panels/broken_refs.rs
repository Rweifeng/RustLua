//! 断链检查窗（P1）：列出工程里指向不存在资源的引用字段，并把"同名文件在工程里另有
//! **唯一**命中"的那一类一键改指。
//!
//! 三条口径：
//! 1. **审计单位是"一条断掉的路径"，不是"一个字段"**：同一条路径通常被几十处引用，
//!    修复动作也天然是路径级的（`from → to` 全工程改写）。所以列表按路径聚合，
//!    消费者只作证据展示。
//! 2. **只做同名字符匹配**：文件被挪了目录/改了大小写时命中唯一，可以一键改指；
//!    同名多份或根本找不到时**不给按钮**——猜一个填进去，比留着断链更难发现
//!    （和"看起来生效了其实没动"是同一类坑）。
//! 3. **面板只发动作**：扫描与改写都在 shell（`crate::actions::broken_refs`），
//!    改写走 `history.execute` 一条 Undo，与资源重命名同一条链路。

use crate::EditorApp;
use editor::EditorCommand;
use eframe::egui;

/// 一条断掉的引用（按**路径**聚合）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BrokenReference {
    /// 当前指向的相对路径（工程内，正斜杠）。
    pub value: String,
    /// 工程里的同名文件（0 = 无从推断；1 = 可一键改指；>1 = 需要人工判断）。
    pub candidates: Vec<String>,
    /// 消费者证据：`文档 ▸ 实体(#id) ▸ 组件.字段`，最多列 `MAX_CONSUMERS` 条。
    pub consumers: Vec<String>,
    /// 消费者总数（可能大于 `consumers.len()`）。
    pub consumer_count: usize,
}

/// 列表里最多列几条消费者（剩下的报数量）。
pub(crate) const MAX_CONSUMERS: usize = 6;

/// 断链检查窗状态（挂在 `EditorApp` 上；行是"某一次扫描"的结果，不是实时视图）。
#[derive(Default)]
pub(crate) struct BrokenRefsPanelState {
    pub show: bool,
    pub rows: Vec<BrokenReference>,
    /// 扫描时读过的文档数（报告口径：`0 份`说明扫描根本没跑起来）。
    pub scanned_documents: usize,
    /// 读不动的文档（解析失败），在窗口里点名——否则"没有断链"可能是"没看得见"。
    pub unreadable: Vec<String>,
    pub message: String,
    /// 是否已经扫过（没扫过时窗口给引导而不是空表）。
    pub scanned: bool,
}

impl BrokenRefsPanelState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 切换/关闭工程：上一次扫描的结果不再属于这里。
    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// 可一键改指的条数（判定口径见模块头第 2 条：同名且工程里唯一命中）。
    ///
    /// 摘要行与"按钮该不该出现"必须共用这一个判据，否则标题写"3 处可改"、
    /// 点进去只按得动 2 个。
    pub fn fixable_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| row.candidates.len() == 1)
            .count()
    }
}

impl EditorApp {
    /// 菜单入口：打开窗口。不自动扫描——扫描要读全部场景/Prefab，代价该由用户点。
    pub(crate) fn open_broken_refs_panel(&mut self) {
        if !self.project_loaded {
            return;
        }
        self.broken_refs.show = true;
    }

    pub(crate) fn draw_broken_refs_panel(&mut self, context: &egui::Context) {
        if !self.broken_refs.show {
            return;
        }
        let mut open = true;
        egui::Window::new("断链检查")
            .open(&mut open)
            .default_width(820.0)
            .default_height(520.0)
            .resizable(true)
            .show(context, |ui| {
                self.draw_broken_refs_body(ui);
            });
        self.broken_refs.show = open && self.broken_refs.show;
    }

    fn draw_broken_refs_body(&mut self, ui: &mut egui::Ui) {
        let fixable = self.broken_refs.fixable_count();
        ui.horizontal_wrapped(|ui| {
            if ui
                .button("扫描全工程")
                .on_hover_text(
                    "读一遍所有场景/Prefab 与当前文档（内存版，含未保存编辑）：\n\
                     代价是逐份解析，工程越大越慢，所以只在显式点击时做。",
                )
                .clicked()
            {
                self.dispatch(EditorCommand::ScanBrokenReferences);
            }
            if self.broken_refs.scanned {
                ui.label(
                    egui::RichText::new(format!(
                        "{} 条断链 · {} 处可一键改指 · 读过 {} 份文档",
                        self.broken_refs.rows.len(),
                        fixable,
                        self.broken_refs.scanned_documents
                    ))
                    .small()
                    .weak(),
                );
            }
        });
        if !self.broken_refs.message.is_empty() {
            ui.label(
                egui::RichText::new(self.broken_refs.message.as_str())
                    .small()
                    .weak(),
            );
        }
        if !self.broken_refs.unreadable.is_empty() {
            ui.colored_label(
                egui::Color32::from_rgb(220, 150, 60),
                format!(
                    "读不动的文档 {} 份：{}（这些里面的断链不在下面清单里）",
                    self.broken_refs.unreadable.len(),
                    self.broken_refs.unreadable.join("、")
                ),
            );
        }
        if !self.broken_refs.scanned {
            ui.weak("还没扫描过：点「扫描全工程」。");
            return;
        }
        if self.broken_refs.rows.is_empty() {
            ui.label("没有断链：所有引用字段指向的路径都在资源索引里。");
            return;
        }
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("broken_refs_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut repair: Option<(String, String)> = None;
                for row in &self.broken_refs.rows {
                    draw_broken_row(ui, row, &mut repair);
                    ui.separator();
                }
                if let Some((from, to)) = repair {
                    self.dispatch(EditorCommand::RepairBrokenReference { from, to });
                }
            });
    }
}

/// 一行断链 + 它的改指按钮。
///
/// 按钮只在**唯一命中**时出现；其余情况把原因写在行里，不留一个"点了也许对"的按钮。
fn draw_broken_row(
    ui: &mut egui::Ui,
    row: &BrokenReference,
    repair: &mut Option<(String, String)>,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(&row.value).monospace());
        ui.label(
            egui::RichText::new(format!("· {} 处引用", row.consumer_count))
                .small()
                .weak(),
        );
        if row.candidates.len() == 1 {
            let candidate = row.candidates[0].clone();
            if ui
                .small_button(format!("改指到 {candidate}"))
                .on_hover_text(
                    "全工程改写这条路径（当前文档进一条撤销；其它文档直接写盘）。\n\
                     判定依据只有文件名相同且工程里唯一命中——不做相似度猜测。",
                )
                .clicked()
            {
                *repair = Some((row.value.clone(), candidate));
            }
        } else if row.candidates.is_empty() {
            ui.colored_label(
                egui::Color32::from_rgb(220, 150, 60),
                "· 工程里没有同名文件：请在 Inspector 里重选资源",
            );
        } else {
            ui.colored_label(
                egui::Color32::from_rgb(220, 150, 60),
                format!(
                    "· 工程里有 {} 份同名文件，需要人工判断：{}",
                    row.candidates.len(),
                    row.candidates.join("、")
                ),
            );
        }
    });
    for consumer in row.consumers.iter().take(MAX_CONSUMERS) {
        ui.label(
            egui::RichText::new(format!("    ↳ {consumer}"))
                .small()
                .weak(),
        );
    }
    if row.consumer_count > row.consumers.len().min(MAX_CONSUMERS) {
        ui.label(
            egui::RichText::new(format!(
                "    ↳ 其余 {} 处省略",
                row.consumer_count - row.consumers.len().min(MAX_CONSUMERS)
            ))
            .small()
            .weak(),
        );
    }
}

/// 一条断链的收集器（扫描时按路径聚合）。
#[derive(Default)]
pub(crate) struct BrokenHit {
    pub(crate) consumers: Vec<String>,
    pub(crate) count: usize,
}

impl BrokenHit {
    /// 记一个消费者；超过上限只留样本，总数照记。
    pub fn push(&mut self, consumer: String) {
        self.count += 1;
        if self.consumers.len() < MAX_CONSUMERS {
            self.consumers.push(consumer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BrokenHit, BrokenReference, BrokenRefsPanelState, MAX_CONSUMERS};

    #[test]
    fn consumers_are_capped_but_the_count_is_not() {
        let mut hit = BrokenHit::default();
        for index in 0..(MAX_CONSUMERS + 5) {
            hit.push(format!("main.scene ▸ E#{index} ▸ Sprite.texture"));
        }
        assert_eq!(hit.count, MAX_CONSUMERS + 5, "总数要照实");
        assert_eq!(hit.consumers.len(), MAX_CONSUMERS, "样本封顶");
    }

    #[test]
    fn only_a_single_candidate_is_a_repair_offer() {
        let row = |candidates: Vec<String>| BrokenReference {
            value: "assets/textures/player.png".to_owned(),
            candidates,
            consumers: Vec::new(),
            consumer_count: 1,
        };
        let state = BrokenRefsPanelState {
            rows: vec![
                row(vec![]),
                row(vec!["assets/other/player.png".to_owned()]),
                row(vec!["a/player.png".to_owned(), "b/player.png".to_owned()]),
            ],
            ..BrokenRefsPanelState::new()
        };
        // 判据只有一条：同名唯一命中。0 份与 2 份都不许出现「改指」按钮（宁可不给）。
        assert_eq!(state.fixable_count(), 1);
    }
}
