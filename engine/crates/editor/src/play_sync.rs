//! Play 期间的**运行态同步**：把预览活世界的真值回灌进编辑器的运行态镜像文档。
//!
//! 分工：`editor::runtime_sync` 是纯函数（吃读数、改 `Scene`、可脱机测试），这里只做宿主接线
//! ——每帧从预览运行时取三份读数、按节流写镜像、结构变了就 bump 镜像修订号并清理悬空选择。
//!
//! 为什么 bump 修订号而不 `mark_dirty`：层级行模型与 Scene 视图运行时都按文档修订号失效，
//! 结构（加行/删行）必须让它们重算；而"实体跑了一步"只是姿态变了，属性面板逐帧读文档，
//! 不需要为此重建整棵编辑运行时。`mark_dirty` 反而会把镜像标成"● 未保存"——那份改动
//! Stop 时必然丢弃，标了也存不了。

use std::time::{Duration, Instant};

use crate::EditorApp;
use editor::runtime_sync::{RuntimeWorldSnapshot, apply_runtime_snapshot, live_transforms};
use editor::{PlaySession, ProjectDocument, SceneDocument};
use rustlua_input::input::InputSnapshot;
use rustlua_scene::scene::SceneEntityId;

/// 运行态回灌的节流：层级面板是给人看的，30Hz 足够，也避免每帧克隆整份世界读数。
const RUNTIME_SYNC_INTERVAL: Duration = Duration::from_millis(33);

/// 面板（Scene 视图 + 层级）要跟随的那份场景文档：**Play 期间是运行态镜像**（会被回灌真值、
/// 修订号随结构变化 bump），编辑期间就是编辑文档本身。
///
/// 为什么是个收两个字段引用的自由函数、而不是 `EditorApp` 上的方法：方法返回的引用借的是
/// **整个 `self`**，调用方接着就没法再改 `self.scene_runtime`/`self.scene_font_atlas` 这些
/// 别的字段（Scene 视图每帧都要）。按字段收引用只锁住这两个，其余字段照旧可动。
///
/// 视图侧的 revision 判断、运行时重建、覆盖层与命中几何一律走这里，不要在原地写
/// `app.document.current_scene`——否则游戏已经跑起来了，Scene 视图还在画进 Play 前的
/// 那份场景，两个视图各说一套，正是这次要修的毛病。
///
/// **踩过的坑（第 58 轮用户报障）**：面板里"读文档"往往不止一处——层级面板的**行模型**
/// （`ensure_hierarchy_rows`）接了镜像，而**行数据**（`draw_entity_row` 取名字/组件名）
/// 还在原地读编辑文档，运行期生成的行于是在绘制时被静默跳过：行模型里有、屏幕上没有。
/// 所以接线时要按"行模型 / 行数据 / 覆盖层 / 命中几何 / 修订号"逐条数，不能只改入口那一处。
pub(crate) fn panel_scene_document<'a>(
    play_session: &'a Option<PlaySession>,
    document: &'a ProjectDocument,
) -> Option<&'a SceneDocument> {
    play_session
        .as_ref()
        .map(PlaySession::runtime_document)
        .or(document.current_scene.as_ref())
}

/// Scene 视图设计分辨率框里那格"游戏画面"该显示哪张纹理。
///
/// Play 期间答案是 **Game 视图那一帧**（`live`），不是编辑运行时按镜像重画的那张
/// （`mirror_rendered`）：镜像只回灌结构与姿态，运行期生成的行没有 `Image`/`Text`/`UiLayout`
/// 载荷，重画出来是空白——那正是"两个视图各说一套"的另一种形态。`live` 还没就绪时返回
/// `None`（先空着），**不**退回 `mirror_rendered`：宁可空一帧，也不拿一份无脚本的重画冒充
/// 游戏画面（否则作者会以为游戏真的没画面）。
pub(crate) fn game_screen_texture(
    playing: bool,
    live: Option<eframe::egui::TextureId>,
    mirror_rendered: Option<eframe::egui::TextureId>,
) -> Option<eframe::egui::TextureId> {
    if playing { live } else { mirror_rendered }
}

impl EditorApp {
    /// 每帧调用（Play 期间）：把预览世界回灌进镜像文档，并留下「文档行 → 活变换」表
    /// 供 Scene 视图推进运行时（见 [`Self::push_play_transforms_to_scene_runtime`]）。
    ///
    /// 非 Play / 没有预览会话 / 未到节流周期时什么都不做。
    pub(crate) fn sync_runtime_document_from_preview(&mut self) {
        if !self.is_playing() {
            self.play_sync_at = None;
            self.play_scene_transforms.clear();
            return;
        }
        if self
            .play_sync_at
            .is_some_and(|last| last.elapsed() < RUNTIME_SYNC_INTERVAL)
        {
            return;
        }
        self.play_sync_at = Some(Instant::now());

        let Some(preview) = &self.preview else {
            return;
        };
        let runtime = preview.runtime();
        let spawned = runtime.runtime_spawned_entities();
        let declared = runtime.scene_entity_handles();
        let authored = runtime.scene_entity_local_transforms();
        let snapshot = RuntimeWorldSnapshot {
            spawned: &spawned,
            declared: &declared,
            authored: &authored,
        };
        let Some(session) = self.play_session.as_mut() else {
            return;
        };
        let document = session.runtime_document_mut();
        let report = apply_runtime_snapshot(&mut document.scene, &snapshot);
        self.play_scene_transforms = live_transforms(&document.scene, &snapshot);
        if !report.structure_changed() {
            return;
        }
        // 结构变了才让缓存失效：只加 1，不置 dirty（见模块头）。
        document.revision = document.revision.wrapping_add(1);
        self.drop_selection_for_missing_entities();
    }

    /// Play：把刚回灌的活姿态推进 Scene 视图的编辑运行时。
    ///
    /// 结构没变时镜像修订号不动 ⇒ `ensure_scene_runtime` 不重建运行时，它因此停在上一帧
    /// 的姿态。这里逐行比对、**只写真的变了的那些**，Scene 视图本帧就画在活位置上。
    /// 编辑期间不做任何事（拖动预览另有它自己的一条线：`crate::scene_drag`）。
    pub(crate) fn push_play_transforms_to_scene_runtime(&mut self) {
        if !self.is_playing() || self.play_scene_transforms.is_empty() {
            return;
        }
        // 把表暂时取出来：`self.play_scene_transforms` 与 `self.scene_runtime` 是两个字段，
        // 直接同时借用会撞所有权——取出来就不再借 `self`，也省掉每帧克隆一份表。
        let mut table = std::mem::take(&mut self.play_scene_transforms);
        if let Some(runtime) = self.scene_runtime.as_mut() {
            for (id, local) in table.iter_mut() {
                if runtime.local_transform_of(*id) == Some(*local) {
                    continue;
                }
                let _ = runtime.set_local_transform(*id, *local);
            }
        }
        self.play_scene_transforms = table;
    }

    /// 本帧 Game 视图没被绘制（切到 Scene 页签、面板被挡住）时仍然步进预览。
    ///
    /// Unity 的 Scene 页签不会让游戏停摆，靠的就是"推进与绘制分家"。这里在帧末补一次
    /// 空输入的 `update`：`PreviewSession::update` 自带 ~60fps 节流与 repaint 续约，所以
    /// 看不见 Game 视图时游戏照跑，而一次性输入边沿不会被这条路径消费（传的是默认快照，
    /// `consumes_input_edges` 因此为假，`preview_input_state` 里的边沿留给下一次真实桥接）。
    pub(crate) fn keep_preview_running_without_game_view(
        &mut self,
        context: &eframe::egui::Context,
    ) {
        if !self.is_playing() || self.preview_stepped_this_frame {
            return;
        }
        if let Some(preview) = self.preview.as_mut() {
            preview.update(context, &InputSnapshot::default());
            self.preview_stepped_this_frame = true;
        }
    }

    /// 镜像里没了的行（被脚本销毁，或随父级一起消失）不能继续留在选择模型里，
    /// 否则属性面板会显示一个游戏里已经不存在的实体。
    fn drop_selection_for_missing_entities(&mut self) {
        let Some(document) = self.current_scene() else {
            return;
        };
        let removed: std::collections::BTreeSet<SceneEntityId> = self
            .selection
            .ids()
            .into_iter()
            .filter(|id| !document.scene.entities.contains_key(id))
            .collect();
        if removed.is_empty() {
            return;
        }
        self.selection.on_entities_removed(&removed, None);
        // 还有别的行被选中就回读那一份，全部没了才清空缓冲。
        match self.selection.primary() {
            Some(id) => self.sync_inspector(id),
            None => self.clear_inspector(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor::ProjectInfo;
    use eframe::egui;
    use rustlua_ecs::spatial_transform::TRANSFORM_COMPONENT;
    use rustlua_infra::logging::LogSink;
    use rustlua_scene::scene::Scene;

    /// headless（CPU fallback）wgpu 设备；拿不到就跳过——这几条要真预览会话。
    fn test_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()?;
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("rustlua-play-sync-test"),
            required_features: wgpu::Features::empty(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .ok()
    }

    /// 一个已进 Play、场景里只有一个声明实体 `Hero` 的应用。
    ///
    /// 工程根是空的临时目录：预览运行时照样会注册全套 Lua API，测试要用的是
    /// `entity.spawn` 这条**运行期**通道，而不是工程脚本。
    fn playing_app(label: &str) -> Option<(EditorApp, std::path::PathBuf)> {
        let (device, queue) = test_device()?;
        let root = std::env::temp_dir().join(format!(
            "rustlua-play-sync-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock should be valid")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut app = EditorApp::new(
            ProjectInfo {
                root: root.clone(),
                version: "test".into(),
                directories: Vec::new(),
            },
            Some((device, queue)),
            LogSink::new(64),
        );
        app.document.new_scene("main");
        {
            let document = app.document.current_scene.as_mut().unwrap();
            document
                .scene
                .add_entity(SceneEntityId(1), "Hero", None)
                .unwrap();
            document
                .scene
                .entities
                .get_mut(&SceneEntityId(1))
                .unwrap()
                .components
                .insert(
                    TRANSFORM_COMPONENT.to_owned(),
                    "position=10,20,0;rotation=0,0,0,1;scale=1,1,1".to_owned(),
                );
        }
        app.play();
        assert!(
            app.play_session.is_some(),
            "有 GPU 上下文时 Play 应当建立运行态镜像"
        );
        Some((app, root))
    }

    /// 在预览世界里跑一段 Lua（等价于游戏脚本在某一帧干的事）。
    fn console(app: &mut EditorApp, source: &str) {
        let result = app
            .preview
            .as_mut()
            .expect("preview session")
            .with_script(|script| script.execute_console(source));
        result.unwrap_or_else(|error| panic!("console 执行失败：{error}"));
    }

    fn row_id(scene: &Scene, name: &str) -> Option<SceneEntityId> {
        scene
            .entities
            .values()
            .find(|entity| entity.name == name)
            .map(|entity| entity.id)
    }

    /// Play 期间脚本 `entity.spawn` 的实体必须出现在**镜像文档**里（层级面板读的就是它），
    /// 且带的是活位置——这是"Scene/Game 两视图同源"的地基。
    #[test]
    fn spawned_entity_shows_up_in_the_mirror_with_a_live_position() {
        let Some((mut app, root)) = playing_app("spawn") else {
            return;
        };
        console(
            &mut app,
            "bullet = entity.spawn('Bullet') transform.set(bullet, 300, 400)",
        );

        app.sync_runtime_document_from_preview();

        let (bullet, local) = {
            let mirror = app.play_session.as_ref().unwrap().runtime_document();
            let bullet = row_id(&mirror.scene, "Bullet").expect("镜像里没有脚本生成的实体行");
            let entity = &mirror.scene.entities[&bullet];
            // 身份键是句柄编码（槽位+代际），Stop 之后没人会拿它去查世界。
            assert!(
                entity.metadata[editor::runtime_sync::RUNTIME_HANDLE_META]
                    .parse::<u64>()
                    .is_ok()
            );
            // 声明实体由镜像自带的行表示，不该被当成"运行期生成"再列一遍。
            assert_eq!(mirror.scene.entities.len(), 2, "Hero + Bullet，不多不少");
            (
                bullet,
                rustlua_ecs::spatial_transform::scene_entity_local(&entity.components)
                    .expect("镜像行该有 Transform 分量串"),
            )
        };
        assert_eq!([local.translation.x, local.translation.y], [300.0, 400.0]);

        // 用户看得见的那一半：层级面板的行模型必须把这一行列出来（Unity 的 Play 层级就是这样）。
        assert!(
            app.hierarchy_rows().iter().any(|row| row.id == bullet),
            "层级面板没列出运行期生成的实体行"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 行模型列出了运行期行还不够——**绘制路径**也得画得出来。这里跑一遍真的层级面板帧
    /// （与 `perf_baseline` 的 10k 档同一套无窗口 egui 帧），数"实际画了几行"。
    ///
    /// 为什么必须有这道门：`spawned_entity_shows_up_in_the_mirror_with_a_live_position`
    /// 只断言行模型，而行模型与行数据源是**两处**读文档的地方——绘制时若仍只认编辑文档，
    /// 运行期行（编辑文档里根本没有的 id）会在 `draw_entity_row` 里被静默跳过：行模型里有、
    /// 屏幕上没有，正是"层级面板没同步 Game 视图"的用户可见形态。
    #[test]
    fn hierarchy_panel_actually_draws_runtime_rows() {
        let Some((mut app, root)) = playing_app("hierarchy-draw") else {
            return;
        };
        console(
            &mut app,
            "bullet = entity.spawn('Bullet') transform.set(bullet, 300, 400)",
        );
        app.sync_runtime_document_from_preview();
        let expected = app
            .play_session
            .as_ref()
            .unwrap()
            .runtime_document()
            .scene
            .entities
            .len();
        assert_eq!(expected, 2, "Hero + Bullet");

        let context = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(320.0, 720.0));
        let run_frame = |app: &mut EditorApp, context: &egui::Context| {
            let raw = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            let _ = context.run(raw, |context| {
                app.draw_hierarchy_panel(context);
            });
        };
        // 预热帧吃掉字体图集与首次布局；之后单帧的绘制计数就是本帧实际画出的行数。
        for _ in 0..3 {
            run_frame(&mut app, &context);
        }
        let before = app.profiler.counter("hierarchy_rows_drawn");
        run_frame(&mut app, &context);
        let drawn = app.profiler.counter("hierarchy_rows_drawn") - before;

        assert_eq!(
            drawn as usize, expected,
            "层级面板只画出了 {drawn} 行（镜像里有 {expected} 行）——运行期生成的行没被画出来"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 用户报障的**原形**：空场景 + 整屏运行期 UI（`examples/game_jhyt` 门面就是
    /// `main.scene` 里一个实体都没有、18 个节点全靠 `prefab.spawn`）。这里拿工程里那份
    /// **真实预设**跑一遍：镜像要有 18 行、每一行都要带上"UI 行"标记（Scene 视图据此不画
    /// 姿态假框），而且层级帧必须把它们**都画出来**。
    #[test]
    fn hierarchy_panel_draws_a_runtime_spawned_prefab_screen() {
        let Some((mut app, root)) = playing_app("hierarchy-prefab") else {
            return;
        };
        let source = rustlua_infra::games_dir()
            .join("examples/game_jhyt/assets/prefabs/ui/title_menu.prefab");
        let target = root.join("assets/prefabs/ui/title_menu.prefab");
        std::fs::create_dir_all(target.parent().expect("prefab dir")).unwrap();
        std::fs::copy(&source, &target).expect("复制门面预设");
        console(
            &mut app,
            "prefab.spawn('assets/prefabs/ui/title_menu.prefab')",
        );
        app.sync_runtime_document_from_preview();

        let (rows, ui_rows) = {
            let mirror = app.play_session.as_ref().unwrap().runtime_document();
            // Hero（编辑文档里那一个声明实体）+ 预设的 18 个节点。
            let rows = mirror.scene.entities.len();
            let ui_rows = mirror
                .scene
                .entities
                .values()
                .filter(|entity| {
                    entity
                        .metadata
                        .contains_key(editor::runtime_sync::RUNTIME_UI_META)
                })
                .count();
            (rows, ui_rows)
        };
        assert_eq!(rows, 19, "Hero + 预设的 18 个节点");
        assert_eq!(
            ui_rows, 18,
            "整屏运行期 UI 没被标成 UI 行——Scene 视图会把 18 个框全叠在世界原点"
        );

        let context = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(320.0, 900.0));
        let run_frame = |app: &mut EditorApp, context: &egui::Context| {
            let raw = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            let _ = context.run(raw, |context| {
                app.draw_hierarchy_panel(context);
            });
        };
        for _ in 0..3 {
            run_frame(&mut app, &context);
        }
        let before = app.profiler.counter("hierarchy_rows_drawn");
        run_frame(&mut app, &context);
        let drawn = app.profiler.counter("hierarchy_rows_drawn") - before;
        assert_eq!(
            drawn as usize, rows,
            "门面的运行期节点没被画出来（画出 {drawn} 行，镜像 {rows} 行）"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// Scene 视图设计分辨率框里那格"游戏画面"：Play 期间必须是 Game 视图那一帧，
    /// 而不是编辑运行时按镜像重画的那张（镜像没有运行期 UI 行的版面载荷，重画是空白）。
    /// 预览还没出帧时先空着——宁可空一帧，也不拿无脚本的重画冒充游戏画面。
    #[test]
    fn play_mode_game_screen_shows_the_live_preview_frame() {
        let live = egui::TextureId::User(7);
        let mirror_rendered = egui::TextureId::User(3);

        assert_eq!(
            game_screen_texture(true, Some(live), Some(mirror_rendered)),
            Some(live),
            "Play 期间这一格必须是 Game 视图那一帧"
        );
        assert_eq!(
            game_screen_texture(true, None, Some(mirror_rendered)),
            None,
            "预览还没出帧时不能退回无脚本的重画（那是错的数据，不是过渡态）"
        );
        assert_eq!(
            game_screen_texture(false, Some(live), Some(mirror_rendered)),
            Some(mirror_rendered),
            "编辑期间仍是编辑运行时渲染出来的那张"
        );
    }

    /// 结构没变、只有位置在变的那些帧：**不能**重建 Scene 视图的编辑运行时（那等于每帧
    /// 重新实例化一遍场景），但视图里的姿态必须跟上。
    #[test]
    fn position_follows_without_rebuilding_the_scene_runtime() {
        let Some((mut app, root)) = playing_app("follow") else {
            return;
        };
        console(
            &mut app,
            "bullet = entity.spawn('Bullet') transform.set(bullet, 300, 400)",
        );
        app.sync_runtime_document_from_preview();
        app.ensure_scene_runtime();
        let bullet = {
            let mirror = app.play_session.as_ref().unwrap().runtime_document();
            row_id(&mirror.scene, "Bullet").expect("镜像里没有 Bullet 行")
        };
        assert!(
            app.scene_runtime.is_some(),
            "镜像里已有场景，Scene 视图运行时该建起来"
        );
        let revision = app
            .play_session
            .as_ref()
            .unwrap()
            .runtime_document()
            .revision;

        // 下一帧：只挪位置，不改结构。回灌要绕开 30Hz 节流才能确定性地在同一测试里跑第二次。
        console(&mut app, "transform.set(bullet, 500, 600)");
        app.play_sync_at = None;
        app.sync_runtime_document_from_preview();
        app.push_play_transforms_to_scene_runtime();

        assert_eq!(
            app.play_session
                .as_ref()
                .unwrap()
                .runtime_document()
                .revision,
            revision,
            "只是走了一步，不该 bump 镜像修订号（那会重建编辑运行时）"
        );
        let view_local = app
            .scene_runtime
            .as_ref()
            .expect("Scene 视图运行时不该被重建掉")
            .local_transform_of(bullet)
            .expect("视图运行时里该有这一行");
        assert_eq!(
            [view_local.translation.x, view_local.translation.y],
            [500.0, 600.0],
            "Scene 视图停在旧姿态：活变换没推进运行时"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 切到 Scene 页签（Game 视图这一帧没被绘制）时游戏照跑，而 Game 视图已经步进过就
    /// **不该**再补一步（否则逻辑时间走两倍，游戏加速）。
    #[test]
    fn preview_keeps_running_when_the_game_view_is_not_drawn() {
        let Some((mut app, root)) = playing_app("tab") else {
            return;
        };
        let ctx = egui::Context::default();

        // 帧末补步进：步进过之后节流基准被推进。
        app.preview.as_mut().unwrap().force_tick();
        app.preview_stepped_this_frame = false;
        let before = app.preview.as_ref().unwrap().last_tick();
        app.keep_preview_running_without_game_view(&ctx);
        assert!(
            app.preview_stepped_this_frame,
            "Game 视图没画时，帧末必须补一次步进"
        );
        let after = app.preview.as_ref().unwrap().last_tick();
        assert_ne!(before, after, "补步进应该真的走到 PreviewSession::update");
        assert!(
            app.preview
                .as_ref()
                .unwrap()
                .runtime()
                .last_schedule_report()
                .is_some_and(|report| report.steps >= 1),
            "补的那一次必须跑逻辑步，否则切页签等于暂停"
        );

        // Game 视图本帧已经步进过：这条路径整段跳过，节流基准一个字都不动。
        app.preview_stepped_this_frame = true;
        let already = app.preview.as_ref().unwrap().last_tick();
        app.keep_preview_running_without_game_view(&ctx);
        assert_eq!(
            app.preview.as_ref().unwrap().last_tick(),
            already,
            "本帧已步进过还补一步，逻辑时间就走两倍"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
