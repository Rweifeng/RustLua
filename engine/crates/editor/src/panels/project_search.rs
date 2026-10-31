//! 全工程搜索窗（③）：一次输入，四类来源一起查，双击一行落到具体位置。
//!
//! 索引来源（与 `crate::actions::project_index` 一致）：
//! **文档组件字段** + **实体/文档元数据** + **资源路径** + **Lua 文本**。
//!
//! 三条口径：
//! 1. **每行都带"文档 ▸ 实体 ▸ 字段"**：跨文档的命中只说"第 3 处"没有意义，
//!    作者要的是"这一处在哪、跳过去是不是它"。
//! 2. **面板只发动作**：建索引、跑查询、定位一行都在 shell
//!    （`crate::actions::project_search`）；本文件不读盘、不写文档。
//! 3. **命中封顶但计数照实**：只画前 [`crate::actions::project_index::MAX_SEARCH_HITS`]
//!    行，摘要仍报真实总数——不制造"只有这么几条"的假象。
//!
//! 与断链检查窗共用同一份路径索引（`ProjectIndex`）：两边看到的文档集合与字段判据必须一样，
//! 否则会出现"搜索说这个资源没人用、断链检查却列出它"这类自相矛盾。

use crate::EditorApp;
use crate::actions::project_index::SearchHit;
use eframe::egui;

/// 搜索窗状态（挂在 `EditorApp` 上；索引是"某一次重建"的快照，不是实时视图）。
#[derive(Default)]
pub(crate) struct ProjectSearchState {
    pub show: bool,
    pub query: String,
    /// 已建立的索引（`None` = 还没扫过，此时搜索不给结果而不是给空结果）。
    pub index: Option<crate::actions::project_index::ProjectIndex>,
    pub hits: Vec<SearchHit>,
    /// 封顶前的真实命中数。
    pub total_hits: usize,
    /// 当前高亮行（Enter 定位它）。
    pub selected: usize,
    /// 命中是否被封顶（决定要不要显示"还有 N 条没列出"）。
    pub truncated: bool,
    pub message: String,
    /// 因"要先换文档（当前文档脏）"而挂起的定位目标。
    pub pending_jump: Option<crate::actions::project_index::SearchTarget>,
}

impl ProjectSearchState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 切换工程：旧索引的路径与内容都不属于这里，整份作废。
    pub fn clear(&mut self) {
        *self = Self::new();
    }
}

impl EditorApp {
    pub(crate) fn draw_project_search_panel(&mut self, context: &egui::Context) {
        if !self.project_search.show {
            return;
        }
        let mut open = true;
        egui::Window::new("全工程搜索")
            .id(egui::Id::new("project_search_window"))
            .open(&mut open)
            .default_width(940.0)
            .default_height(560.0)
            .resizable(true)
            .show(context, |ui| {
                self.draw_project_search_body(ui);
            });
        self.project_search.show = open && self.project_search.show;
    }

    fn draw_project_search_body(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.project_search.query)
                    .id(ui.make_persistent_id("project_search_box"))
                    .hint_text("搜字段 / 元数据 / 实体名 / 资源路径 / Lua 文本（多词 AND）")
                    .desired_width(f32::INFINITY),
            );
            if !self.project_search.query.trim().is_empty()
                && crate::util::draw_icon_button(ui, crate::util::DrawIcon::Cross, "清空搜索")
                    .clicked()
            {
                self.project_search.query.clear();
                self.run_project_search();
            }
            if response.changed() {
                self.run_project_search();
            }
            if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                let selected = self.project_search.selected;
                self.activate_search_hit(selected);
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui
                .button("重建索引")
                .on_hover_text(
                    "重新读一遍所有场景/Prefab 与脚本（当前文档用内存版，含未保存编辑）。\n\
                     代价是逐份解析，工程越大越慢，所以只在显式点击时做。",
                )
                .clicked()
            {
                self.rebuild_project_search_index();
            }
            self.draw_project_search_summary(ui);
        });
        if !self.project_search.message.is_empty() {
            ui.label(
                egui::RichText::new(self.project_search.message.as_str())
                    .small()
                    .weak(),
            );
        }
        if self.project_search.index.is_none() {
            ui.weak("还没有索引：点「重建索引」（或按 Enter 前先建一次）。");
            return;
        }
        if self.project_search.query.trim().is_empty() {
            ui.weak("输入关键词开始搜索：搜索只在已建立的索引上做，不读盘。");
            return;
        }
        if self.project_search.hits.is_empty() {
            ui.label("没有命中。");
            return;
        }
        ui.separator();
        self.draw_project_search_hits(ui);
    }

    /// 摘要行：命中数（封顶前）+ 索引扫过的份数。
    fn draw_project_search_summary(&mut self, ui: &mut egui::Ui) {
        let Some(index) = self.project_search.index.as_ref() else {
            ui.label(egui::RichText::new("未建立索引").small().weak());
            return;
        };
        let mut text = format!(
            "命中 {} 条 · 索引 {} 份文档 / {} 个脚本",
            self.project_search.total_hits, index.scanned_documents, index.scanned_lua_files
        );
        if self.project_search.truncated {
            text.push_str(&format!(
                "（只列出前 {} 条，请细化搜索词）",
                self.project_search.hits.len()
            ));
        }
        if !index.unreadable.is_empty() {
            text.push_str(&format!(" · {} 份文档读不动", index.unreadable.len()));
        }
        if !index.unreadable_scripts.is_empty() {
            text.push_str(&format!(
                " · {} 个脚本读不动",
                index.unreadable_scripts.len()
            ));
        }
        ui.label(egui::RichText::new(text).small().weak());
    }

    /// 结果列表：一行一处命中，双击定位（键盘用 ↑/↓ + Enter）。
    fn draw_project_search_hits(&mut self, ui: &mut egui::Ui) {
        // 方向键改高亮行（不消费 Enter：Enter 由上面的搜索输入框负责）。
        let movement = ui.input(|input| {
            if input.key_pressed(egui::Key::ArrowDown) {
                Some(1_isize)
            } else if input.key_pressed(egui::Key::ArrowUp) {
                Some(-1)
            } else {
                None
            }
        });
        if let Some(step) = movement {
            let count = self.project_search.hits.len();
            let next = (self.project_search.selected as isize + step).clamp(0, count as isize - 1);
            self.project_search.selected = next as usize;
        }
        // 先读后写：循环里只借用 `hits`，选中态与定位动作在列表画完后统一发
        // （面板不直接改文档，见模块头）。
        let mut selected = self.project_search.selected;
        let mut activate: Option<usize> = None;
        egui::ScrollArea::vertical()
            .id_salt("project_search_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (index, hit) in self.project_search.hits.iter().enumerate() {
                    let row = draw_project_search_row(ui, hit, selected == index);
                    if row.clicked {
                        selected = index;
                    }
                    if row.double_clicked {
                        activate = Some(index);
                    }
                }
            });
        self.project_search.selected = selected;
        if let Some(index) = activate {
            self.activate_search_hit(index);
        }
    }
}

/// 一行的点击结果。
struct RowClick {
    clicked: bool,
    double_clicked: bool,
}

/// 画一行：类别前缀 + "文档 ▸ 实体 ▸ 字段" + 取值片段。
fn draw_project_search_row(ui: &mut egui::Ui, hit: &SearchHit, selected: bool) -> RowClick {
    let location = hit.location_text();
    let mut clicked = false;
    let mut double_clicked = false;
    ui.horizontal(|ui| {
        ui.add(
            egui::Label::new(
                egui::RichText::new(hit.kind.label())
                    .small()
                    .color(kind_color(hit)),
            )
            .selectable(false),
        );
        let response =
            ui.selectable_label(selected, egui::RichText::new(location.as_str()).monospace());
        let response = response.on_hover_text("双击定位到这一处（脚本行会在只读查看器里打开）");
        clicked = response.clicked();
        double_clicked = response.double_clicked();
        if !hit.detail.is_empty() {
            ui.label(
                egui::RichText::new(hit.detail.as_str())
                    .small()
                    .weak()
                    .monospace(),
            );
        }
    });
    RowClick {
        clicked,
        double_clicked,
    }
}

/// 一行的类别色：让"字段/元数据/实体/文档/资源/脚本"在长列表里一眼分得开。
fn kind_color(hit: &SearchHit) -> egui::Color32 {
    use crate::actions::project_index::SearchKind;
    match hit.kind {
        SearchKind::Field => egui::Color32::from_rgb(120, 180, 240),
        SearchKind::Metadata => egui::Color32::from_rgb(170, 150, 220),
        SearchKind::Entity => egui::Color32::from_rgb(140, 200, 150),
        SearchKind::Document => egui::Color32::from_rgb(210, 200, 140),
        SearchKind::Resource => egui::Color32::from_rgb(230, 170, 120),
        SearchKind::Script => egui::Color32::from_rgb(200, 140, 160),
    }
}
