//! 标题栏 + 菜单栏 + 崩溃恢复提示（编辑器顶部区域）。

use crate::EditorApp;
use crate::util::{choose_project_directory, icon_toggle, open_in_explorer, strip_verbatim_prefix};
use crate::workspace_root;
use eframe::egui;
use std::path::Path;

/// 新建项目的模板（M79-05 首启闭环）。两者都带相机与背景，差别是有没有可控角色。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProjectTemplate {
    /// 空白工程：一台正交相机 + 一块背景，Play 就有画面，之后全自己摆。
    Blank,
    /// 2D 起步：空白工程的内容 + 一块 WASD/方向键可移动的方块（`scripts/entity/player.lua`）。
    Starter2d,
}

impl EditorApp {
    /// 菜单栏（标题栏已移除，项目路径显示在窗口标题）。
    pub(crate) fn draw_header_panel(&mut self, context: &egui::Context) {
        // 把「RustLua 编辑器 - <项目路径>」同步到窗口标题（去除 \\?\ 前缀）。
        self.sync_window_title(context);
        egui::TopBottomPanel::top("menu_bar").show(context, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                // 菜单结构是数据（`crate::menu::MENU_BAR`，按 Unity 的七个顶级菜单组织）：
                // 这里只负责渲染 + 把动作交回 `run_menu_action`。
                self.draw_menu_bar(ui);
                // 旧的内联菜单（项目/编辑/视图/帮助/恢复）已迁移到 `crate::menu`：
                // 结构与动作都在那边的声明表里，这里不再逐个按钮地拼。

                // 右侧：状态 + 资源/控制台面板开关。
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_toggle(ui, self.show_lua_console, "⌨", "显示或隐藏控制台").clicked()
                    {
                        self.show_lua_console = !self.show_lua_console;
                    }
                    if icon_toggle(ui, self.show_resources_panel, "▤", "显示或隐藏资源面板")
                        .clicked()
                    {
                        self.show_resources_panel = !self.show_resources_panel;
                    }
                    ui.separator();
                    ui.label(format!("状态：{}", self.status));
                });
            });
        });
    }

    /// 把「RustLua 编辑器 - <项目路径>」同步到窗口标题（路径去除 \\?\ 前缀）。
    fn sync_window_title(&mut self, context: &egui::Context) {
        let title = self.window_title();
        if self.last_window_title.as_deref() != Some(title.as_str()) {
            self.last_window_title = Some(title.clone());
            context.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
    }

    fn window_title(&self) -> String {
        let root = strip_verbatim_prefix(&self.project.root);
        format!("RustLua 编辑器 - {}", root.display())
    }

    /// 恢复一条崩溃快照（E06：dirty 时先经确认网关，确认后才替换文档）。
    pub(crate) fn recover_candidate(&mut self, candidate: editor::RecoveryCandidate) {
        self.request_document_change(crate::actions::PendingChange::Recover { candidate });
    }

    // `perform_recover_candidate` **已搬到 shell**（`crate::actions`，第 28 轮）：它会替换当前文档，
    // 属于提交边界。面板只发确认/typed action。

    /// 重新扫描崩溃恢复目录。
    ///
    /// 失败时**保留上一份扫描结果并把原因写进状态栏**，而不是
    /// `unwrap_or_default()`：后者会把"恢复目录读不了"伪装成"没有任何待恢复内容"，
    /// 用户会以为崩溃前的编辑已经丢了，也看不到环境本身出了问题。
    pub(crate) fn refresh_recovery_scan(&mut self) {
        match self.recovery.scan() {
            Ok(scan) => {
                if !scan.invalid.is_empty() {
                    tracing::warn!(
                        target: "editor",
                        invalid = scan.invalid.len(),
                        "[editor] recovery scan found {} invalid snapshot(s)",
                        scan.invalid.len()
                    );
                }
                self.recovery_scan = scan;
            }
            Err(error) => {
                tracing::warn!(target: "editor", error = %error, "[editor] recovery scan failed; keeping the previous result");
                self.status = format!("扫描崩溃恢复快照失败：{error}");
            }
        }
    }

    /// 菜单：新建项目 —— 选目录，补骨架（project.toml/scripts/scenes/assets），
    /// 再按 `editor --check` 的同一套口径跑一次尾门校验，通过才切换项目。
    /// E06：目录选定并补齐骨架后，经确认网关切换项目（dirty 时弹确认窗）。
    pub(crate) fn create_new_project(&mut self, template: ProjectTemplate) {
        let Some(dir) = choose_project_directory() else {
            return;
        };
        if let Err(error) = Self::scaffold_project(&dir, template) {
            self.status = format!("创建项目骨架失败：{error}");
            return;
        }
        // 尾门必须真的跑：骨架写歪了却报"项目已创建"，用户下一步遇到的是"打开一个连
        // 引擎自己都读不了的项目"，比这里直接红更难查。失败时文件留在磁盘上（让用户
        // 能改），但不切项目。
        let summary = match Self::verify_scaffold(&dir) {
            Ok(summary) => summary,
            Err(report) => {
                tracing::warn!(target: "editor", report = %report, "[editor] new project scaffold check failed");
                self.status = format!("骨架已写入但校验未通过：{report}");
                return;
            }
        };
        self.request_project_switch(dir.clone());
        self.status = format!(
            "已创建{}：{}（{summary}）",
            match template {
                ProjectTemplate::Blank => "空白项目",
                ProjectTemplate::Starter2d => "2D 起步项目",
            },
            dir.display()
        );
    }

    /// 骨架的尾门校验：结构（`editor --check` 同源）+ 启动场景可读且带相机 +
    /// `scripts/**/*.lua` 全部语法可编译（`game --check` 同源的那段）。
    fn verify_scaffold(dir: &Path) -> Result<String, String> {
        let report = rustlua_editor_core::check_project(dir).map_err(|error| error.to_string())?;
        let summary = report
            .lines()
            .next()
            .unwrap_or("editor_project_ok")
            .trim()
            .to_owned();
        let scene_path = dir.join("scenes/main.scene");
        let scene = rustlua_scene::scene::Scene::load(&scene_path)
            .map_err(|error| format!("启动场景 {} 读不了：{error}", scene_path.display()))?;
        let has_camera = scene.entities.values().any(|entity| {
            entity
                .components
                .contains_key(rustlua_camera::camera::CAMERA_COMPONENT)
        });
        if !has_camera {
            return Err("启动场景没有相机，Game 视图会是黑的".to_owned());
        }
        let registry = rustlua_assets_import::builtin();
        let mut scripts = 0;
        for file in rustlua_infra::vfs::list_files_recursive(&dir.join("scripts")) {
            if !file
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("lua"))
            {
                continue;
            }
            let Some(importer) = registry.from_path(&file) else {
                continue;
            };
            let bytes = std::fs::read(&file).map_err(|error| error.to_string())?;
            importer
                .validate(&file, &bytes)
                .map_err(|error| format!("脚本语法不过：{error}"))?;
            scripts += 1;
        }
        Ok(format!(
            "{summary} scene_entities={} scripts={}",
            scene.entities.len(),
            scripts
        ))
    }

    /// 菜单：打开项目 —— 选目录并走后台导入流程（E06：dirty 时先确认）。
    pub(crate) fn open_project_menu(&mut self) {
        let Some(dir) = choose_project_directory() else {
            return;
        };
        self.request_project_switch(dir);
    }

    /// 把「新建/打开/最近项目」统一为经未保存确认网关注入后台导入路径。
    fn request_project_switch(&mut self, dir: std::path::PathBuf) {
        self.request_document_change(crate::actions::PendingChange::SwitchProject {
            path: dir.to_string_lossy().into_owned(),
        });
    }

    /// 帮助 > 版本说明窗口。
    pub(crate) fn draw_about_window(&mut self, context: &egui::Context) {
        if !self.show_about {
            return;
        }
        egui::Window::new("版本说明")
            .open(&mut self.show_about)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(context, |ui| {
                ui.label("RustLua 引擎编辑器");
                ui.label(format!("编辑器版本：{}", env!("CARGO_PKG_VERSION")));
                ui.label(format!("项目基线：{}", self.project.version));
                ui.label(format!(
                    "项目路径：{}",
                    crate::util::display_path(&self.project.root)
                ));
                // 插件加载结果（P4）：`plugin_status` 此前只写不显示，注册/加载失败
                // 在编辑器里完全静默——现在在版本说明里列出来，失败一眼可见。
                if !self.plugin_status.is_empty() {
                    ui.separator();
                    ui.label("插件：");
                    for line in &self.plugin_status {
                        ui.label(format!("· {line}"));
                    }
                }
            });
    }

    /// 菜单：使用手册 —— 在资源管理器中打开 docs 目录（docs 位于工作区根，不在 engine/ 下）。
    pub(crate) fn open_help(&mut self) {
        let docs = workspace_root().join("docs");
        if docs.is_dir() {
            open_in_explorer(&docs);
        } else {
            self.status = "未找到 docs 目录".to_owned();
        }
    }

    /// 若目录缺少游戏项目结构，补一个最小骨架（project.toml + main.lua + main.scene + assets/）。
    ///
    /// M79-05：模板从"两个空文件"升级为"建完就有画面、按 Play 就能跑"：
    /// 场景自带一台正交相机和一块背景（否则 Game 视图是一片纯黑，新手会以为引擎没起来），
    /// 脚本的回调名逐字对齐运行时真正查找的那三个（旧的 `on_update(dt)` 是**实体**钩子的名字，
    /// 写在工程级永远不会被调用）。
    fn scaffold_project(dir: &Path, template: ProjectTemplate) -> Result<(), String> {
        use std::io::Write;
        let fs_err = |error: std::io::Error| error.to_string();
        std::fs::create_dir_all(dir.join("assets/fonts")).map_err(fs_err)?;
        let font_path = dir.join("assets/fonts/NotoSansSC-VF.ttf");
        if !font_path.exists() {
            // 编译期 CARGO_MANIFEST_DIR 对门构建的 exe 指向已删除的临时 worktree。
            let source = crate::util::resolve_cjk_font().ok_or_else(|| {
                format!(
                    "未找到引擎内嵌中文字体 {}；请设置 RUSTLUA_FONT 指向一份 .ttf",
                    crate::util::CJK_FONT_FILE_NAME
                )
            })?;
            std::fs::copy(&source, &font_path).map_err(fs_err)?;
        }
        std::fs::create_dir_all(dir.join("scripts")).map_err(fs_err)?;
        std::fs::create_dir_all(dir.join("scenes")).map_err(fs_err)?;
        if !dir.join("project.toml").is_file() {
            let name = dir
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("new_project");
            let mut file = std::fs::File::create(dir.join("project.toml")).map_err(fs_err)?;
            writeln!(file, "[project]").map_err(fs_err)?;
            writeln!(file, "name = \"{name}\"").map_err(fs_err)?;
            writeln!(file, "version = \"{}\"", rustlua_infra::version()).map_err(fs_err)?;
            writeln!(file, "engine = \"RustLua\"").map_err(fs_err)?;
            writeln!(file, "main_script = \"scripts/main.lua\"").map_err(fs_err)?;
            writeln!(file, "startup_scene = \"scenes/main.scene\"").map_err(fs_err)?;
            writeln!(file, "font = \"assets/fonts/NotoSansSC-VF.ttf\"").map_err(fs_err)?;
            writeln!(file, "font_weight = 400.0").map_err(fs_err)?;
            writeln!(file, "text_render_mode = \"auto\"").map_err(fs_err)?;
        }
        if !dir.join("scripts/main.lua").is_file() {
            std::fs::write(dir.join("scripts/main.lua"), PROJECT_SCRIPT).map_err(fs_err)?;
        }
        if template == ProjectTemplate::Starter2d {
            std::fs::create_dir_all(dir.join("scripts/entity")).map_err(fs_err)?;
            if !dir.join("scripts/entity/player.lua").is_file() {
                std::fs::write(dir.join("scripts/entity/player.lua"), PLAYER_SCRIPT)
                    .map_err(fs_err)?;
            }
        }
        if !dir.join("scenes/main.scene").is_file() {
            std::fs::write(dir.join("scenes/main.scene"), template_scene_text(template))
                .map_err(fs_err)?;
        }
        Ok(())
    }
}

/// 工程级脚本模板：三个回调名是运行时真的在查的那三个（`script/runtime.rs` 的
/// `on_create` / 全局 `update()` / `on_destroy`）。
const PROJECT_SCRIPT: &str = r#"-- 工程级入口。回调名由运行时决定，只有这三个：
--   on_create()   场景建好后一次
--   update()      每个逻辑步一次，**不收参数**（要 dt 就用 time.delta()）
--   on_destroy()  场景销毁前一次
-- 每实体脚本（场景里 Script 组件指向的文件）用的是另一套名字：on_spawn(id) / on_update(id, dt)，
-- 两组不重名。

function on_create()
    log.info("scene created")
end

function update()
end

function on_destroy()
end
"#;

/// 2D 起步模板的角色脚本：方向键/WASD 移动一块方块，让"建完就能动"看得见。
const PLAYER_SCRIPT: &str = r#"-- Player 实体的每实体脚本（场景里 `Script file=scripts/entity/player.lua`）。
-- 这里的钩子收 (id, dt)：`id` 是引擎给的实体编号，`dt` 是本步秒数。
-- WASD 或方向键都能走；世界 +y 朝上，所以 w 是往上。

local SPEED = 240.0 -- 像素/秒

function on_update(id, dt)
    local dx = 0.0
    local dy = 0.0
    if input.is_down("a") or input.is_down("left") then
        dx = dx - 1.0
    end
    if input.is_down("d") or input.is_down("right") then
        dx = dx + 1.0
    end
    if input.is_down("w") or input.is_down("up") then
        dy = dy + 1.0
    end
    if input.is_down("s") or input.is_down("down") then
        dy = dy - 1.0
    end
    if dx ~= 0.0 or dy ~= 0.0 then
        transform.move(id, dx * SPEED * dt, dy * SPEED * dt)
    end
end
"#;

/// 模板场景文本（tab 分隔，与 `Scene::load` 的格式同源；`Transform` 的 12 位小数
/// 与 `Scene::to_text` 的写法一致，避免"新建完第一次保存就整行漂移"）。
fn template_scene_text(template: ProjectTemplate) -> String {
    let transform = |x: f32, y: f32, z: f32| {
        format!(
            "position={x:.9},{y:.9},{z:.9};rotation=0.000000000,0.000000000,0.000000000,1.000000000;scale=1.000000000,1.000000000,1.000000000"
        )
    };
    let mut text = format!(
        "scene_version={}\nname=main\nmeta\teditor.root_order\t1,2{}\n",
        rustlua_scene::scene::CURRENT_SCENE_VERSION,
        if template == ProjectTemplate::Starter2d {
            ",3"
        } else {
            ""
        }
    );
    text.push_str(
        "entity\t1\tCamera\n\
         component\t1\tCamera\tprojection=orthographic;center=640.0000,360.0000;zoom=1.000000\n",
    );
    text.push_str(&format!(
        "entity\t2\tBackdrop\n\
         component\t2\tQuad\tsize=1280,720;color=0.070,0.085,0.120,1.000;z=-10.000\n\
         component\t2\tTransform\t{}\n",
        transform(640.0, 360.0, 0.0)
    ));
    if template == ProjectTemplate::Starter2d {
        text.push_str(&format!(
            "entity\t3\tPlayer\n\
             component\t3\tScript\tfile=scripts/entity/player.lua\n\
             component\t3\tQuad\tsize=32,32;color=0.360,0.780,0.460,1.000;z=0.000\n\
             component\t3\tTransform\t{}\n",
            transform(640.0, 360.0, 0.0)
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor::{SceneDocument, open_project};
    use rustlua_infra::logging::LogSink;
    use rustlua_scene::scene::SceneEntityId;
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    fn unique_token() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos()
    }

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("m14-03-header-{label}-{}", unique_token()))
    }

    fn sample_project_root() -> PathBuf {
        rustlua_infra::games_dir().join("examples").join("game_api")
    }

    fn copy_dir(source: &Path, destination: &Path) {
        if !source.is_dir() {
            return;
        }
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let source_path = entry.path();
            let target_path = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                fs::create_dir_all(&target_path).unwrap();
                copy_dir(&source_path, &target_path);
            } else {
                if let Some(parent) = target_path.parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                fs::copy(&source_path, &target_path).unwrap();
            }
        }
    }

    fn copy_sample_project(label: &str) -> PathBuf {
        let source = sample_project_root();
        assert!(
            source.join("project.toml").is_file(),
            "sample project missing: {}",
            source.display()
        );
        let root = temp_root(label);
        fs::create_dir_all(&root).unwrap();
        fs::copy(source.join("project.toml"), root.join("project.toml")).unwrap();
        copy_dir(&source.join("scripts"), &root.join("scripts"));
        copy_dir(&source.join("scenes"), &root.join("scenes"));
        copy_dir(&source.join("assets"), &root.join("assets"));
        root
    }

    #[test]
    fn scaffold_project_creates_font_and_config() {
        let root = temp_root("scaffold");
        fs::create_dir_all(&root).unwrap();

        EditorApp::scaffold_project(&root, ProjectTemplate::Blank).unwrap();

        let font = root.join("assets/fonts/NotoSansSC-VF.ttf");
        assert!(font.is_file());
        let project = fs::read_to_string(root.join("project.toml")).unwrap();
        assert!(project.contains("font = \"assets/fonts/NotoSansSC-VF.ttf\""));
        assert!(project.contains("font_weight = 400.0"));
        assert!(project.contains("text_render_mode = \"auto\""));
        // P0-02：新建工程模板的版本必须取当前引擎版本（历史漂移：曾写死 0.1.0 而引擎已 0.1.1）。
        assert!(
            project.contains(&format!("version = \"{}\"", rustlua_infra::version())),
            "scaffold version must follow engine version"
        );

        let _ = fs::remove_dir_all(root);
    }

    /// M79-05 首启闭环：模板的脚本回调名必须是运行时真正查找的那三个，
    /// 场景必须自带相机（否则新建完 Play 是一片纯黑），且骨架自己得过尾门校验。
    #[test]
    fn blank_scaffold_runs_the_tail_check_and_has_a_camera() {
        let root = temp_root("scaffold-blank");
        fs::create_dir_all(&root).unwrap();
        EditorApp::scaffold_project(&root, ProjectTemplate::Blank).unwrap();

        let script = fs::read_to_string(root.join("scripts/main.lua")).unwrap();
        for name in [
            "function on_create()",
            "function update()",
            "function on_destroy()",
        ] {
            assert!(script.contains(name), "模板缺工程级回调 {name}");
        }
        assert!(
            !script.contains("function on_update"),
            "工程级模板不该**定义**实体钩子 on_update（它收 (id, dt)，写在 main.lua 永远不会被调用）"
        );

        let scene = fs::read_to_string(root.join("scenes/main.scene")).unwrap();
        assert!(scene.contains("component\t1\tCamera\tprojection=orthographic"));
        assert!(scene.contains("Backdrop"));

        let report = EditorApp::verify_scaffold(&root).unwrap_or_else(|error| {
            panic!("空白骨架应当过尾门校验：{error}");
        });
        assert!(report.contains("editor_project_ok"), "{report}");
        assert!(report.contains("scene_entities=2"), "{report}");
        assert!(report.contains("scripts=1"), "{report}");

        let _ = fs::remove_dir_all(root);
    }

    /// 2D 起步模板：多一块 Player 与它的实体脚本，同样必须过尾门（实体脚本的语法也在门内）。
    #[test]
    fn starter_2d_scaffold_ships_a_movable_player() {
        let root = temp_root("scaffold-2d");
        fs::create_dir_all(&root).unwrap();
        EditorApp::scaffold_project(&root, ProjectTemplate::Starter2d).unwrap();

        assert!(root.join("scripts/entity/player.lua").is_file());
        let player = fs::read_to_string(root.join("scripts/entity/player.lua")).unwrap();
        assert!(
            player.contains("function on_update(id, dt)"),
            "实体钩子收 (id, dt)"
        );
        let scene = fs::read_to_string(root.join("scenes/main.scene")).unwrap();
        assert!(
            scene.contains("component\t3\tScript\tfile=scripts/entity/player.lua"),
            "Player 必须挂上那份实体脚本"
        );

        let report = EditorApp::verify_scaffold(&root)
            .unwrap_or_else(|error| panic!("2D 起步骨架应当过尾门校验：{error}"));
        assert!(report.contains("scene_entities=3"), "{report}");
        assert!(report.contains("scripts=2"), "{report}");

        let _ = fs::remove_dir_all(root);
    }

    /// 反面对照：尾门不是摆设——把启动场景改成没有相机，校验必须红并说清原因。
    #[test]
    fn tail_check_rejects_a_scene_without_a_camera() {
        let root = temp_root("scaffold-no-camera");
        fs::create_dir_all(&root).unwrap();
        EditorApp::scaffold_project(&root, ProjectTemplate::Blank).unwrap();
        fs::write(
            root.join("scenes/main.scene"),
            format!(
                "scene_version={}\nname=main\n",
                rustlua_scene::scene::CURRENT_SCENE_VERSION
            ),
        )
        .unwrap();

        let error = EditorApp::verify_scaffold(&root).expect_err("空场景应当被尾门拦下");
        assert!(error.contains("相机"), "{error}");

        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn app_startup_scans_recovery_and_recover_candidate_restores_scene() {
        let root = copy_sample_project("recover-menu");
        let scene_path = root.join("scenes/api_gameplay.scene");
        let manager = editor::RecoveryManager::new(root.join(".temp"));
        let mut modified = SceneDocument::open(&scene_path).unwrap();
        let original = modified.clone();
        modified
            .scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .name = "RecoveredViaMenu".to_owned();
        modified.mark_dirty();
        manager.autosave(&modified).unwrap();

        let project = open_project(&root).unwrap();
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        assert_eq!(app.recovery_scan.candidates.len(), 1);

        app.document.current_scene = Some(SceneDocument::open(&scene_path).unwrap());
        app.selection
            .set_single(SceneEntityId(2), editor::SelectionSource::Hierarchy);

        let candidate = app.recovery_scan.candidates[0].clone();
        app.recover_candidate(candidate.clone());

        let recovered = app.document.current_scene.as_ref().unwrap();
        assert_eq!(recovered.path, Some(scene_path.clone()));
        assert!(recovered.is_dirty());
        assert_eq!(
            recovered.scene.entities[&SceneEntityId(2)].name,
            "RecoveredViaMenu"
        );
        assert!(app.selection.is_empty());
        assert!(
            app.status.contains("已恢复崩溃快照"),
            "status should report recovery result"
        );

        let reopened = SceneDocument::open(&scene_path).unwrap();
        assert_eq!(
            reopened.scene.entities[&SceneEntityId(2)].name,
            original.scene.entities[&SceneEntityId(2)].name,
            "recovering a candidate must not overwrite the formal scene file"
        );
        let _ = fs::remove_dir_all(root);
    }
}
