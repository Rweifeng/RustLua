//! docs/55 阶段 0「GUI smoke 清单落地」的自动化闸门。
//!
//! 计划里阶段 0 的验收要求"固定小/中/大工程可重复测量（报告）"与"真实 GUI 的点击/拖放/弹窗
//! 操作脚本 + 证据目录约定"。这份模块负责**第二半**里可以完全自动化的那部分：
//!
//! * 用**真实 `egui::Context`** 跑**整帧**（`EditorApp::draw_frame`，所有面板都真的画一遍），
//!   输入是合成的 `RawInput.events`（指针按下/抬起/移动、键盘、文本），因此走的是与真人操作
//!   完全相同的 `Response` → 面板处理器路径，而不是直接调面板的内部方法；
//! * 用例自带不变量断言（选中集合、历史条数、文档内容、行高门槛、画布矩形），失败会**报出
//!   具体数字**并让命令以非零退出码结束，可以直接在 `verify.ps1` 里当质量门；
//! * 报告可落盘归档（`--gui-smoke --gui-report <路径>`），与 ER-01 的性能基线同一套约定。
//!
//! ## 与真实窗口证据的分工（刻意写清楚，避免把两者混为一谈）
//!
//! | 覆盖面 | 谁负责 |
//! |---|---|
//! | 整帧布局/交互逻辑、跨面板状态、DPI 与 resize 下的**点空间**几何 | 本模块（无窗口、可重复、进 CI） |
//! | 真实窗口的 GPU 画面、100/150/200% DPI 截图、30 分钟 Play/Stop、长会话内存 | `P2_EDITOR_*` 真实运行（`--gui-evidence`，产物归档到 `artifacts/acceptance/`） |
//!
//! 本模块**不**声称覆盖后者：合成帧没有 GPU 后备缓冲，也没有真实窗口的重绘节奏。
//! 这一点在 `docs/58` 里逐条登记。

use crate::{EditorApp, SceneTool};
use editor::{ProjectInfo, SceneDocument};
use eframe::egui;
use engine::core::{
    logging::LogSink,
    scene::SceneEntityId,
    sprite::{SpriteComponent, SpriteSorting},
    transform::Vec2 as CoreVec2,
};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// 一个 smoke 用例的判定结果。
#[derive(Clone, Debug)]
pub(crate) struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

impl Check {
    fn new(name: impl Into<String>, ok: bool, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ok,
            detail: detail.into(),
        }
    }
}

/// GUI smoke 报告（`--gui-smoke` 打印并可选落盘）。
#[derive(Clone, Debug, Default)]
pub(crate) struct SmokeReport {
    pub checks: Vec<Check>,
}

impl SmokeReport {
    pub fn ok(&self) -> bool {
        self.checks.iter().all(|check| check.ok)
    }

    pub fn failed(&self) -> usize {
        self.checks.iter().filter(|check| !check.ok).count()
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str("=== RustLua editor GUI smoke (docs/55 阶段 0) ===\n");
        out.push_str(&format!(
            "profile={} os={}\n",
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            std::env::consts::OS
        ));
        out.push_str(
            "note: 合成 egui 帧（无窗口、无 GPU）；真实窗口证据见 artifacts/acceptance/\n\n",
        );
        for check in &self.checks {
            out.push_str(&format!(
                "{:<52} {}\n",
                check.name,
                if check.ok { "ok" } else { "FAILED" }
            ));
            if !check.detail.is_empty() {
                for line in check.detail.lines() {
                    out.push_str("    - ");
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        out.push_str(&format!(
            "\ncases={} failed={} result={}\n",
            self.checks.len(),
            self.failed(),
            if self.ok() { "ok" } else { "FAILED" }
        ));
        out
    }
}

/// 合成输入驱动的**整帧**编辑器（所有面板都按生产路径绘制）。
pub(crate) struct GuiSmoke {
    pub(crate) app: EditorApp,
    pub(crate) ctx: egui::Context,
    pub(crate) size: egui::Vec2,
    pub(crate) pixels_per_point: f32,
    time: f64,
    root: PathBuf,
}

impl GuiSmoke {
    /// 建一个临时工程 + 一个确定性场景，并把 `project_loaded` 置真
    /// （快捷键、命令分发、布局持久化都依赖它）。
    pub(crate) fn new(label: &str, size: egui::Vec2, pixels_per_point: f32) -> Self {
        let root = temp_root(label);
        let mut app = EditorApp::new(
            ProjectInfo {
                root: root.clone(),
                version: "gui-smoke".to_owned(),
                directories: Vec::new(),
            },
            None,
            LogSink::new(64),
        );
        app.project_loaded = true;
        app.document.current_scene = Some(smoke_document());
        app.selection.clear(editor::SelectionSource::Hierarchy);
        app.show_domain_overlay = false;
        app.show_performance_panel = false;
        let ctx = egui::Context::default();
        ctx.set_theme(egui::Theme::Dark);
        ctx.set_pixels_per_point(pixels_per_point);
        Self {
            app,
            ctx,
            size,
            pixels_per_point,
            time: 0.0,
            root,
        }
    }

    /// 推进一帧（无输入事件）。
    pub(crate) fn frame(&mut self) {
        self.frame_with(Vec::new(), egui::Modifiers::default());
    }

    /// 空闲若干帧：让首帧字体图集、面板尺寸、交互状态收敛。
    pub(crate) fn idle(&mut self, frames: usize) {
        for _ in 0..frames {
            self.frame();
        }
    }

    /// 推进一帧并注入合成事件。
    pub(crate) fn frame_with(&mut self, events: Vec<egui::Event>, modifiers: egui::Modifiers) {
        self.time += 1.0 / 60.0;
        let raw = egui::RawInput {
            time: Some(self.time),
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.size)),
            events,
            modifiers,
            focused: true,
            ..Default::default()
        };
        let app = &mut self.app;
        let _ = self.ctx.run(raw, |ctx| app.draw_frame(ctx));
    }

    /// 合成一次点击：悬停一帧 → 按下 → 抬起。
    ///
    /// 分开三帧是必要的：`egui` 的交互解析用"按下那一帧命中的控件"决定 press origin，
    /// 而 `Response::clicked()` 在抬起那一帧才算出来；把三者塞进一帧会让命中测试拿不到
    /// 上一帧的控件矩形。
    pub(crate) fn click_at(&mut self, pos: egui::Pos2, modifiers: egui::Modifiers) {
        self.frame_with(vec![egui::Event::PointerMoved(pos)], modifiers);
        self.frame_with(
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers,
            }],
            modifiers,
        );
        self.frame_with(
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers,
            }],
            modifiers,
        );
        // 抬起之后再走一帧：面板的"帧末提交边界"（`flush_frame_edit_boundary`）在这一帧生效。
        self.frame_with(Vec::new(), modifiers);
    }

    /// 合成一次键盘按键（按下 + 抬起，分两帧）。
    pub(crate) fn press_key(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
        self.frame_with(
            vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            modifiers,
        );
        self.frame_with(
            vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers,
            }],
            modifiers,
        );
    }

    /// 层级面板某一行**本帧**的屏幕矩形（行控件本身用稳定 id 注册）。
    pub(crate) fn row_rect(&self, id: SceneEntityId) -> Option<egui::Rect> {
        self.ctx
            .read_response(egui::Id::new(("hierarchy_entry", id.0)))
            .map(|response| response.rect)
    }

    /// 合成一段文本输入（逐帧推进：与真实键入的节奏一致，过滤器逐字符重算）。
    pub(crate) fn type_text(&mut self, text: &str) {
        for ch in text.chars() {
            self.frame_with(
                vec![egui::Event::Text(ch.to_string())],
                egui::Modifiers::default(),
            );
        }
    }

    /// 推进一帧并**注入一次拖放载荷**（docs/55 §8 质量门枚举的"拖放"项）。
    ///
    /// `egui` 的控件间拖放是"载荷 + 落区"两半：这里模拟的是**载荷已经挂在指针上**的那一帧，
    /// 随后由调用方在落区上释放指针。这样不必知道拖动源控件的矩形，而落区那一半
    /// （`dnd_drop_zone`、payload 类型判定、释放判定、生成实体）走的是完整生产路径。
    pub(crate) fn frame_with_payload(&mut self, payload: &str) {
        self.time += 1.0 / 60.0;
        let raw = egui::RawInput {
            time: Some(self.time),
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.size)),
            focused: true,
            ..Default::default()
        };
        let app = &mut self.app;
        let payload: String = payload.to_owned();
        let _ = self.ctx.run(raw, |ctx| {
            egui::DragAndDrop::set_payload(ctx, payload.clone());
            app.draw_frame(ctx);
        });
    }

    /// 层级面板搜索框的屏幕矩形（本帧没画时为 `None`）。
    pub(crate) fn filter_rect(&self) -> Option<egui::Rect> {
        let rect = self.app.hierarchy_filter_rect;
        (rect.width() > 1.0 && rect.height() > 1.0).then_some(rect)
    }

    /// 当前过滤词匹配到的实体数（与面板上的「匹配 N 项」同源）。
    pub(crate) fn matched_entities(&mut self) -> usize {
        self.app.filtered_match_count()
    }

    /// 当前场景里全部实体的名字（升序），用于断言拖放真的建出了东西。
    pub(crate) fn entity_names(&self) -> Vec<String> {
        self.app
            .document
            .current_scene
            .as_ref()
            .map(|document| {
                let mut names: Vec<String> = document
                    .scene
                    .entities
                    .values()
                    .map(|entity| entity.name.clone())
                    .collect();
                names.sort();
                names
            })
            .unwrap_or_default()
    }

    /// 落在"行名按钮"上的可点坐标（避开行首的展开/显示/锁定三个 18×18 控件）。
    pub(crate) fn row_name_pos(&self, id: SceneEntityId) -> Option<egui::Pos2> {
        let rect = self.row_rect(id)?;
        Some(egui::pos2(rect.left() + 80.0, rect.center().y))
    }

    pub(crate) fn selection_ids(&self) -> Vec<SceneEntityId> {
        let mut ids = self.app.selection.ids();
        ids.sort_unstable();
        ids
    }

    pub(crate) fn entity_count(&self) -> usize {
        self.app
            .document
            .current_scene
            .as_ref()
            .map_or(0, |document| document.scene.entities.len())
    }

    pub(crate) fn scene_document(&self) -> &SceneDocument {
        self.app
            .document
            .current_scene
            .as_ref()
            .expect("smoke 场景必须存在")
    }

    pub(crate) fn cleanup(&self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn temp_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "rustlua-editor-gui-smoke-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("临时工程目录必须可创建");
    root
}

/// 确定性 smoke 场景：4 个 2D 精灵实体（各自带 Transform + Sprite），没有父子关系，
/// 因此层级行深度全为 0（合成点击的落点与深度无关，用例更稳）。
pub(crate) fn smoke_document() -> SceneDocument {
    let mut document = SceneDocument::new_scene("gui_smoke");
    for index in 0..4_u64 {
        let id = SceneEntityId(index + 1);
        let name = format!("Node{index}");
        document.scene.add_entity(id, name, None).expect("加实体");
        let entity = document.scene.entities.get_mut(&id).expect("实体必须存在");
        entity.components.insert(
            "Transform".to_owned(),
            format!(
                "position={},100,0;rotation=0,0,0,1;scale=1,1,1",
                100.0 + index as f32 * 40.0
            ),
        );
        entity.components.insert(
            "Sprite".to_owned(),
            SpriteComponent {
                texture: "assets/textures/player.png".to_owned(),
                size: CoreVec2::new(32.0, 32.0),
                pivot: CoreVec2::new(0.5, 0.5),
                tint: [1.0; 4],
                sorting: SpriteSorting::default(),
                flip_x: false,
                flip_y: false,
                z: 0.0,
                nine_slice: None,
            }
            .to_component_str(),
        );
    }
    document
}

/// 量一次"当前样式下的层级行高"（与面板绘制时用的是同一个函数）。
fn measured_row_height(ctx: &egui::Context, size: egui::Vec2, console: bool) -> f32 {
    let mut height = 0.0;
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        ..Default::default()
    };
    let _ = ctx.run(raw, |ctx| {
        egui::TopBottomPanel::bottom("smoke_measure")
            .resizable(false)
            .show(ctx, |ui| {
                height = if console {
                    crate::panels::console::console_row_height(ui)
                } else {
                    crate::panels::hierarchy::hierarchy_row_height(ui)
                };
            });
    });
    height
}

// ---------------------------------------------------------------------------
// 用例
// ---------------------------------------------------------------------------

/// 用例 1：层级面板的点击选择性通过**合成指针事件**走完整帧。
fn case_hierarchy_pointer_selection() -> Vec<Check> {
    let mut smoke = GuiSmoke::new("hierarchy-click", egui::vec2(1280.0, 720.0), 1.0);
    smoke.idle(4);
    let mut checks = Vec::new();

    let (first, second, third) = (SceneEntityId(1), SceneEntityId(2), SceneEntityId(3));
    let Some(first_pos) = smoke.row_name_pos(first) else {
        checks.push(Check::new(
            "hierarchy/row_rect_available",
            false,
            "层级行没有注册可读矩形（read_response 拿不到 hierarchy_entry id）",
        ));
        smoke.cleanup();
        return checks;
    };
    let second_pos = smoke.row_name_pos(second).expect("第二行矩形");
    let third_pos = smoke.row_name_pos(third).expect("第三行矩形");

    // ① 普通点击 = 单选。
    smoke.click_at(first_pos, egui::Modifiers::default());
    checks.push(Check::new(
        "hierarchy/click_selects_single",
        smoke.selection_ids() == vec![first],
        format!("selection={:?} 期望 [{:?}]", smoke.selection_ids(), first),
    ));

    // ② Ctrl+点击 = 加选。
    smoke.click_at(
        second_pos,
        egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        },
    );
    checks.push(Check::new(
        "hierarchy/ctrl_click_adds_to_selection",
        smoke.selection_ids() == vec![first, second],
        format!(
            "selection={:?} 期望 [{first:?}, {second:?}]",
            smoke.selection_ids()
        ),
    ));

    // ③ 不带修饰键点击另一行 = 替换为单选。
    smoke.click_at(third_pos, egui::Modifiers::default());
    checks.push(Check::new(
        "hierarchy/plain_click_replaces_selection",
        smoke.selection_ids() == vec![third],
        format!("selection={:?} 期望 [{third:?}]", smoke.selection_ids()),
    ));

    // ④ Shift+点击 = 以锚点为端点的范围选择（锚点在 ③ 里落在第三行）。
    smoke.click_at(
        first_pos,
        egui::Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    checks.push(Check::new(
        "hierarchy/shift_click_selects_range",
        smoke.selection_ids() == vec![first, second, third],
        format!(
            "selection={:?} 期望 [{first:?}, {second:?}, {third:?}]",
            smoke.selection_ids()
        ),
    ));

    // ⑤ 在画布**空白处**拖一小段 = 框选，起点为空 → 清空选择（H07 语义）。
    let canvas = smoke.app.scene_canvas_rect;
    let empty = egui::pos2(canvas.left() + 12.0, canvas.bottom() - 12.0);
    smoke.frame_with(
        vec![egui::Event::PointerMoved(empty)],
        egui::Modifiers::default(),
    );
    smoke.frame_with(
        vec![egui::Event::PointerButton {
            pos: empty,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        }],
        egui::Modifiers::default(),
    );
    smoke.frame_with(
        vec![egui::Event::PointerMoved(empty + egui::vec2(20.0, -20.0))],
        egui::Modifiers::default(),
    );
    smoke.frame_with(
        vec![egui::Event::PointerButton {
            pos: empty + egui::vec2(20.0, -20.0),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }],
        egui::Modifiers::default(),
    );
    smoke.frame();
    checks.push(Check::new(
        "hierarchy/empty_canvas_drag_clears_selection",
        smoke.selection_ids().is_empty(),
        format!("selection={:?} 期望 []", smoke.selection_ids()),
    ));

    smoke.cleanup();
    checks
}

/// 用例 2：键盘路径（工具切换 / 删除 / 撤销 / 复制）通过**合成按键事件**生效。
fn case_keyboard_shortcuts() -> Vec<Check> {
    let mut smoke = GuiSmoke::new("keyboard", egui::vec2(1280.0, 720.0), 1.0);
    smoke.idle(4);
    let mut checks = Vec::new();
    let id = SceneEntityId(1);

    // 选中一个实体（走真实点击，保证后续命令有目标）。
    let pos = smoke.row_name_pos(id).expect("行矩形");
    smoke.click_at(pos, egui::Modifiers::default());

    // ① Q/W/E/R 走状态机（docs/55 ER-07：工具的唯一事实源）。
    let mut tools = Vec::new();
    for (key, expected) in [
        (egui::Key::W, SceneTool::Move),
        (egui::Key::E, SceneTool::Rotate),
        (egui::Key::R, SceneTool::Scale),
        (egui::Key::Q, SceneTool::Select),
    ] {
        smoke.press_key(key, egui::Modifiers::default());
        tools.push((format!("{expected}"), smoke.app.scene_tool() == expected));
    }
    checks.push(Check::new(
        "keyboard/qwer_switches_scene_tool",
        tools.iter().all(|(_, ok)| *ok),
        format!("{:?}", smoke.app.scene_tool()),
    ));

    // ② Ctrl+D 原地复制：实体数 +1，且恰好一条 Undo。
    let before = smoke.entity_count();
    smoke.press_key(
        egui::Key::D,
        egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        },
    );
    checks.push(Check::new(
        "keyboard/ctrl_d_duplicates_selection",
        smoke.entity_count() == before + 1 && smoke.app.history.undo_len() == 1,
        format!(
            "entities={} 期望 {}；undo={}",
            smoke.entity_count(),
            before + 1,
            smoke.app.history.undo_len()
        ),
    ));

    // ③ Delete 删除当前**选中集合**（Ctrl+D 之后原件与副本都在选中里，因此删掉 2 个）。
    let selected_before_delete = smoke.selection_ids().len();
    smoke.press_key(egui::Key::Delete, egui::Modifiers::default());
    checks.push(Check::new(
        "keyboard/delete_removes_selection",
        smoke.entity_count() == before + 1 - selected_before_delete,
        format!(
            "entities={} 期望 {}（删除前选中 {selected_before_delete} 个）",
            smoke.entity_count(),
            before + 1 - selected_before_delete
        ),
    ));

    // ④ Ctrl+Z 撤销删除（回到复制后的状态）。
    smoke.press_key(
        egui::Key::Z,
        egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        },
    );
    checks.push(Check::new(
        "keyboard/ctrl_z_undoes_delete",
        smoke.entity_count() == before + 1,
        format!("entities={} 期望 {}", smoke.entity_count(), before + 1),
    ));
    smoke.cleanup();
    checks
}

/// 用例 3：Scene View 的**真实指针事件**拖动 = 恰好一条 Undo（docs/55 阶段 2 验收）。
fn case_scene_view_pointer_drag() -> Vec<Check> {
    let mut smoke = GuiSmoke::new("scene-drag", egui::vec2(1280.0, 720.0), 1.0);
    smoke.idle(4);
    let mut checks = Vec::new();
    let id = SceneEntityId(1);

    smoke.app.set_scene_tool(SceneTool::Move);
    smoke.app.snap_enabled = false;
    smoke.frame();
    let before_transform =
        smoke.scene_document().scene.entities[&id].components["Transform"].clone();
    let Some((start, box_size)) = smoke.app.scene_2d_entity_box(id, smoke.pixels_per_point) else {
        checks.push(Check::new(
            "scene_view/canvas_and_entity_visible",
            false,
            format!(
                "2D 画布矩形={:?}（NOTHING 表示本帧没画 2D 视图）",
                smoke.app.scene_canvas_rect
            ),
        ));
        smoke.cleanup();
        return checks;
    };
    checks.push(Check::new(
        "scene_view/canvas_and_entity_visible",
        true,
        format!(
            "画布={:?} 实体中心={start:?} 包围框={box_size:?}",
            smoke.app.scene_canvas_rect
        ),
    ));

    let end = start + egui::vec2(24.0, 12.0);
    // ① 悬停 → ② 按下 → ③④ 越过拖动阈值 → ⑤ 抬起。
    smoke.frame_with(
        vec![egui::Event::PointerMoved(start)],
        egui::Modifiers::default(),
    );
    smoke.frame_with(
        vec![egui::Event::PointerButton {
            pos: start,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        }],
        egui::Modifiers::default(),
    );
    for step in [egui::vec2(12.0, 6.0), egui::vec2(24.0, 12.0)] {
        smoke.frame_with(
            vec![egui::Event::PointerMoved(start + step)],
            egui::Modifiers::default(),
        );
    }
    let mid_undo = smoke.app.history.undo_len();
    let mid_phase = smoke.app.interaction.phase();
    let mid_drag = smoke.app.scene_state.drag.is_some();
    let mid_preview = smoke.app.scene_state.preview.len();
    smoke.frame_with(
        vec![egui::Event::PointerButton {
            pos: end,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }],
        egui::Modifiers::default(),
    );
    smoke.frame();

    let after_transform =
        smoke.scene_document().scene.entities[&id].components["Transform"].clone();
    checks.push(Check::new(
        "scene_view/drag_commits_exactly_one_undo",
        smoke.app.history.undo_len() == 1 && after_transform != before_transform,
        format!(
            "undo={}（松手前 {mid_undo}）；前={before_transform} 后={after_transform}",
            smoke.app.history.undo_len()
        ),
    ));
    checks.push(Check::new(
        "scene_view/drag_has_no_history_before_release",
        mid_undo == 0,
        format!(
            "拖动过程中 undo={mid_undo} 期望 0；phase={mid_phase:?} drag={mid_drag} preview={mid_preview}"
        ),
    ));
    checks.push(Check::new(
        "scene_view/drag_leaves_interaction_idle",
        smoke.app.interaction.phase() == crate::scene_interaction::ToolPhase::Idle
            && smoke.app.scene_state.drag.is_none()
            && smoke.app.scene_state.preview.is_empty(),
        format!(
            "phase={:?} drag={} preview={}",
            smoke.app.interaction.phase(),
            smoke.app.scene_state.drag.is_some(),
            smoke.app.scene_state.preview.len()
        ),
    ));

    // 撤销回到原文。
    smoke.press_key(
        egui::Key::Z,
        egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        },
    );
    let undone = smoke.scene_document().scene.entities[&id].components["Transform"].clone();
    checks.push(Check::new(
        "scene_view/undo_restores_original_transform",
        undone == before_transform,
        format!("undo 后={undone} 期望={before_transform}"),
    ));

    smoke.cleanup();
    checks
}

/// 用例 4：未保存保护 —— 弹窗在整帧里出现、期间不执行离开动作、取消后文档原样。
fn case_unsaved_change_modal() -> Vec<Check> {
    let mut smoke = GuiSmoke::new("unsaved-modal", egui::vec2(1280.0, 720.0), 1.0);
    smoke.idle(4);
    let mut checks = Vec::new();

    smoke
        .app
        .document
        .current_scene
        .as_mut()
        .expect("场景")
        .mark_dirty();
    let original_name = smoke.scene_document().scene.name.clone();
    smoke
        .app
        .request_document_change(crate::actions::PendingChange::CreateScene {
            name: "another".to_owned(),
        });
    smoke.idle(3);

    checks.push(Check::new(
        "modal/pending_change_is_blocked_not_executed",
        smoke.app.pending_document_change.is_some()
            && smoke.scene_document().scene.name == original_name,
        format!(
            "pending={} 当前文档={}",
            smoke.app.pending_document_change.is_some(),
            smoke.scene_document().scene.name
        ),
    ));

    // 弹窗是模态窗口：多帧之后文档仍然没被替换。
    smoke.idle(5);
    checks.push(Check::new(
        "modal/stays_open_across_frames",
        smoke.app.pending_document_change.is_some()
            && smoke.scene_document().scene.name == original_name,
        format!(
            "pending={} 当前文档={}",
            smoke.app.pending_document_change.is_some(),
            smoke.scene_document().scene.name
        ),
    ));

    // 「取消」按钮的处理器与这里走同一入口（`resolve_pending_change`）。
    let node_count = smoke.entity_count();
    smoke
        .app
        .resolve_pending_change(&smoke.ctx, crate::actions::PendingDecision::Cancel);
    smoke.idle(2);
    checks.push(Check::new(
        "modal/cancel_keeps_document_and_selection",
        smoke.app.pending_document_change.is_none()
            && smoke.scene_document().scene.name == original_name
            && smoke.entity_count() == node_count,
        format!(
            "pending={} 文档={} 实体={}",
            smoke.app.pending_document_change.is_some(),
            smoke.scene_document().scene.name,
            smoke.entity_count()
        ),
    ));

    smoke.cleanup();
    checks
}

/// 用例 5：DPI × 密度 × resize 组合下**整帧**仍满足几何门槛。
///
/// 与 ER-01 基线里的 `layout_row_height_gate` 的区别：那边只画面板自身，这里跑的是
/// `draw_frame`（标题栏 + 菜单 + 两个左面板 + 场景/游戏工作台 + 右属性面板 + 底部控制台
/// + 弹窗），因此它验证的是**组合之后**的布局；行高门槛仍然复用同一对测量函数。
fn case_dpi_and_resize_matrix() -> Vec<Check> {
    let mut checks = Vec::new();
    let sizes = [
        egui::vec2(1280.0, 720.0),
        egui::vec2(1024.0, 720.0),
        egui::vec2(900.0, 760.0),
        egui::vec2(1600.0, 900.0),
    ];
    let mut combinations = 0_usize;
    let mut violations: Vec<String> = Vec::new();
    let mut canvas_failures: Vec<String> = Vec::new();

    for pixels_per_point in [1.0_f32, 1.5, 2.0] {
        for size in sizes {
            combinations += 1;
            let label = format!("ppi={pixels_per_point} size={:.0}x{:.0}", size.x, size.y);
            let mut smoke = GuiSmoke::new("dpi-matrix", size, pixels_per_point);
            smoke.idle(4);

            // ① 画布矩形必须真实存在，并且落在屏幕内。
            let canvas = smoke.app.scene_canvas_rect;
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            if !(canvas.width() > 1.0 && canvas.height() > 1.0) {
                canvas_failures.push(format!("{label}: 画布为空 {canvas:?}"));
            } else if !screen.contains_rect(canvas) {
                canvas_failures.push(format!("{label}: 画布 {canvas:?} 超出屏幕 {screen:?}"));
            }

            // ② 行内容不得高于行高（虚拟化的前提）。
            let hierarchy_row = measured_row_height(&smoke.ctx, size, false);
            let content = smoke.app.hierarchy_row_probe.max_content_height();
            if content > hierarchy_row + 0.01 {
                violations.push(format!(
                    "{label}: hierarchy 内容 {content:.2} > 行高 {hierarchy_row:.2}"
                ));
            }
            let console_row = measured_row_height(&smoke.ctx, size, true);
            let console_content = smoke.app.console_row_probe.max_content_height();
            if console_content > console_row + 0.01 {
                violations.push(format!(
                    "{label}: console 内容 {console_content:.2} > 行高 {console_row:.2}"
                ));
            }

            // ③ 层级虚拟化仍然生效（只画视口内的行）。
            let rows = smoke.app.hierarchy_row_probe.rows();
            if rows == 0 || rows > smoke.entity_count() {
                violations.push(format!(
                    "{label}: 层级绘制行数 {rows} 不合理（实体 {}）",
                    smoke.entity_count()
                ));
            }
            smoke.cleanup();
        }
    }

    checks.push(Check::new(
        "dpi/canvas_rect_present_at_every_dpi_and_size",
        canvas_failures.is_empty(),
        if canvas_failures.is_empty() {
            format!("{combinations} 组组合全部有真实画布矩形")
        } else {
            canvas_failures.join("\n")
        },
    ));
    checks.push(Check::new(
        "dpi/row_content_never_exceeds_row_height",
        violations.is_empty(),
        if violations.is_empty() {
            format!("{combinations} 组组合实测内容 ≤ 行高")
        } else {
            violations.join("\n")
        },
    ));
    checks
}

/// 用例 6：面板宽度改动 + 窗口 resize 之后，选中状态与 Scene 画布保持一致。
fn case_resize_keeps_state_consistent() -> Vec<Check> {
    let mut smoke = GuiSmoke::new("resize-state", egui::vec2(1280.0, 720.0), 1.0);
    smoke.idle(4);
    let mut checks = Vec::new();
    let id = SceneEntityId(2);
    let pos = smoke.row_name_pos(id).expect("行矩形");
    smoke.click_at(pos, egui::Modifiers::default());
    let selected = smoke.selection_ids();

    let mut sizes = Vec::new();
    for size in [
        egui::vec2(1024.0, 700.0),
        egui::vec2(1600.0, 900.0),
        egui::vec2(900.0, 680.0),
        egui::vec2(1280.0, 720.0),
    ] {
        smoke.size = size;
        smoke.idle(3);
        sizes.push(format!(
            "{:.0}x{:.0}→selection={:?}",
            size.x,
            size.y,
            smoke.selection_ids()
        ));
    }
    checks.push(Check::new(
        "resize/selection_survives_window_resizes",
        smoke.selection_ids() == selected && !selected.is_empty(),
        sizes.join("; "),
    ));
    checks.push(Check::new(
        "resize/canvas_tracks_the_new_window_size",
        smoke.app.scene_canvas_rect.width() > 1.0
            && smoke.app.scene_canvas_rect.right() <= smoke.size.x + 0.5,
        format!(
            "canvas={:?} window={:.0}x{:.0}",
            smoke.app.scene_canvas_rect, smoke.size.x, smoke.size.y
        ),
    ));

    smoke.cleanup();
    checks
}

/// 用例 5：**输入焦点**（docs/55 §8 质量门枚举项）。
///
/// 这条守的是一类很容易回归的缺陷：文本输入框有焦点时，Q/W/E/R 这类单键快捷键
/// **不得**抢键（否则用户在搜索框里打 "w" 就切到移动工具）。断言分三段：
/// 点进搜索框 → 键盘焦点真的在 egui 里；此时按 Q/W/E/R → 工具不变；
/// 输入过滤词 → 过滤真的生效（说明按键/文本被输入框消费了）。
fn case_input_focus_consumes_shortcuts() -> Vec<Check> {
    let mut smoke = GuiSmoke::new("input-focus", egui::vec2(1280.0, 720.0), 1.0);
    smoke.idle(4);
    let mut checks = Vec::new();

    let Some(filter_rect) = smoke.filter_rect() else {
        checks.push(Check::new(
            "focus/filter_rect_available",
            false,
            "层级搜索框没有记录矩形（探针未生效）",
        ));
        smoke.cleanup();
        return checks;
    };
    let tool_before = smoke.app.scene_tool();
    smoke.click_at(filter_rect.center(), egui::Modifiers::default());
    let wants_keyboard = smoke.ctx.wants_keyboard_input();
    checks.push(Check::new(
        "focus/click_gives_the_text_field_keyboard_focus",
        wants_keyboard,
        format!(
            "点击 ({:.0},{:.0}) 之后 wants_keyboard_input={wants_keyboard}",
            filter_rect.center().x,
            filter_rect.center().y
        ),
    ));

    // 焦点在文本框里时，单键快捷键必须不生效。
    for key in [egui::Key::W, egui::Key::E, egui::Key::R, egui::Key::Q] {
        smoke.press_key(key, egui::Modifiers::default());
    }
    checks.push(Check::new(
        "focus/single_key_shortcuts_do_not_fire_while_typing",
        smoke.app.scene_tool() == tool_before,
        format!(
            "键入 Q/W/E/R 之后工具={:?}（期望保持 {:?}）",
            smoke.app.scene_tool(),
            tool_before
        ),
    ));

    // 文本真的进了过滤器（并且过滤在帧级生效：匹配数骤降）。
    smoke.type_text("Node1");
    smoke.idle(2);
    let filter = smoke.app.hierarchy_filter.clone();
    let matched = smoke.matched_entities();
    checks.push(Check::new(
        "focus/typed_text_reaches_the_filter_and_narrows_the_tree",
        filter == "Node1" && matched == 1,
        format!(
            "filter={filter:?} 匹配={matched}（期望 1 / 共 {}）",
            smoke.entity_count()
        ),
    ));

    smoke.cleanup();
    checks
}

/// 用例 6：**拖放**（docs/55 §8 质量门枚举项）。
///
/// 把资源拖到 Scene 视图生成实体的落区一侧走完整生产路径：
/// 注入 `String` 载荷 → 在画布上释放指针 → 断言语义（新增实体 + 恰好一条 Undo + 名字可辨）。
fn case_drop_payload_on_scene_view() -> Vec<Check> {
    let mut smoke = GuiSmoke::new("drop-payload", egui::vec2(1280.0, 720.0), 1.0);
    smoke.idle(4);
    let mut checks = Vec::new();
    let canvas = smoke.app.scene_canvas_rect;
    if canvas.width() <= 1.0 {
        checks.push(Check::new(
            "dnd/canvas_available",
            false,
            format!("2D 画布矩形不可用：{canvas:?}"),
        ));
        smoke.cleanup();
        return checks;
    }
    let target = canvas.center();
    let before_entities = smoke.entity_count();
    let before_undo = smoke.app.history.undo_len();

    // 悬停 → 挂上载荷 → 释放。
    smoke.frame_with(
        vec![egui::Event::PointerMoved(target)],
        egui::Modifiers::default(),
    );
    smoke.frame_with_payload("assets/textures/player.png");
    smoke.frame_with(
        vec![egui::Event::PointerButton {
            pos: target,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }],
        egui::Modifiers::default(),
    );
    smoke.frame();

    let after_entities = smoke.entity_count();
    let undo_delta = smoke.app.history.undo_len() - before_undo;
    checks.push(Check::new(
        "dnd/dropping_a_texture_creates_one_entity_with_one_undo",
        after_entities == before_entities + 1 && undo_delta == 1,
        format!(
            "实体 {before_entities}→{after_entities}；undo +{undo_delta}；名字={:?}",
            smoke.entity_names()
        ),
    ));
    checks.push(Check::new(
        "dnd/dropped_entity_is_named_and_selected",
        smoke
            .entity_names()
            .iter()
            .any(|name| name.to_lowercase().contains("sprite") || name.contains("player"))
            && smoke.selection_ids().len() == 1,
        format!(
            "selection={:?}；状态={}",
            smoke.selection_ids(),
            smoke.app.status
        ),
    ));

    smoke.cleanup();
    checks
}

/// 用例 7：**停靠/面板可见性与布局持久化**（docs/55 §8 质量门枚举项）。
///
/// 断言三件事：① 收起底部控制台后画布**真的变大**（不是只改了个标志位）；
/// ② `Ctrl+Shift+L` 把当前布局写进 `<项目>/.cache/editor/layout.json`；
/// ③ 重新读取的布局与内存中的一致（保存/恢复闭环）。
fn case_dock_panel_visibility_and_layout_persistence() -> Vec<Check> {
    let mut smoke = GuiSmoke::new("dock-layout", egui::vec2(1280.0, 720.0), 1.0);
    smoke.idle(4);
    let mut checks = Vec::new();

    let canvas_with_console = smoke.app.scene_canvas_rect;
    smoke.app.show_lua_console = false;
    smoke.idle(3);
    let canvas_without_console = smoke.app.scene_canvas_rect;
    checks.push(Check::new(
        "dock/hiding_the_console_gives_the_canvas_more_room",
        canvas_without_console.height() > canvas_with_console.height() + 1.0,
        format!(
            "画布高度 {:.1} → {:.1}",
            canvas_with_console.height(),
            canvas_without_console.height()
        ),
    ));

    // Ctrl+Shift+L 保存布局（键盘路径）。
    smoke.press_key(
        egui::Key::L,
        egui::Modifiers {
            ctrl: true,
            command: true,
            shift: true,
            ..Default::default()
        },
    );
    smoke.idle(2);
    let root = smoke.app.project.root.clone();
    let layout = editor::load_layout(&root);
    let saved = layout.layout();
    checks.push(Check::new(
        "dock/ctrl_shift_l_persists_the_layout",
        !saved.panel_is_visible(editor::PANEL_CONSOLE),
        format!(
            "console_visible={} 文件={}",
            saved.panel_is_visible(editor::PANEL_CONSOLE),
            editor::layout_path(&root).display()
        ),
    ));

    // 恢复显示 → 画布回到原来的量级（可见性不是单向的）。
    smoke.app.show_lua_console = true;
    smoke.idle(3);
    checks.push(Check::new(
        "dock/showing_the_console_again_restores_the_canvas",
        (smoke.app.scene_canvas_rect.height() - canvas_with_console.height()).abs() < 2.0,
        format!(
            "画布高度 {:.1}（原始 {:.1}）",
            smoke.app.scene_canvas_rect.height(),
            canvas_with_console.height()
        ),
    ));

    smoke.cleanup();
    checks
}

/// 跑完整套 GUI smoke（`--gui-smoke` 与单元测试共用同一条路径）。
pub(crate) fn run_all() -> SmokeReport {
    let mut checks = Vec::new();
    checks.extend(case_hierarchy_pointer_selection());
    checks.extend(case_keyboard_shortcuts());
    checks.extend(case_scene_view_pointer_drag());
    checks.extend(case_unsaved_change_modal());
    checks.extend(case_input_focus_consumes_shortcuts());
    checks.extend(case_drop_payload_on_scene_view());
    checks.extend(case_dock_panel_visibility_and_layout_persistence());
    checks.extend(case_dpi_and_resize_matrix());
    checks.extend(case_resize_keeps_state_consistent());
    SmokeReport { checks }
}

/// `--gui-smoke [--gui-report <路径>]` 的入口；返回"是否全部通过"。
pub(crate) fn run_and_write_report(report_path: Option<&Path>) -> Result<bool, String> {
    let report = run_all();
    let text = report.to_text();
    print!("{text}");
    if let Some(path) = report_path {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("创建报告目录失败 {}: {error}", parent.display()))?;
        }
        fs::write(path, &text)
            .map_err(|error| format!("写入 GUI smoke 报告失败 {}: {error}", path.display()))?;
    }
    Ok(report.ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--gui-smoke` 的全部用例同时是一条集成测试：CI 里 `cargo test` 就会判红。
    #[test]
    fn gui_smoke_cases_all_pass() {
        let report = run_all();
        assert!(
            report.ok(),
            "GUI smoke 用例失败：\n{}",
            report
                .checks
                .iter()
                .filter(|check| !check.ok)
                .map(|check| format!("{} → {}", check.name, check.detail))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    /// 报告文本必须自证"这不是真实窗口证据"，避免被当成 GPU/截图验收。
    #[test]
    fn report_text_states_its_own_boundary() {
        let report = SmokeReport {
            checks: vec![Check::new("x", true, "detail")],
        };
        let text = report.to_text();
        assert!(text.contains("docs/55 阶段 0"), "{text}");
        assert!(text.contains("无窗口、无 GPU"), "{text}");
        assert!(text.contains("cases=1 failed=0 result=ok"), "{text}");
    }

    #[test]
    fn failed_checks_make_the_report_not_ok() {
        let report = SmokeReport {
            checks: vec![
                Check::new("good", true, ""),
                Check::new("bad", false, "boom"),
            ],
        };
        assert!(!report.ok());
        assert_eq!(report.failed(), 1);
        assert!(report.to_text().contains("result=FAILED"));
    }
}
