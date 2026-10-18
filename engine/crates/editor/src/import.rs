//! 游戏项目导入与资源导入：后台线程任务、进度回传、结果应用。
//!
//! M27-01 起这里还是**系统拖放入库**的提交层（`import_external_files`）：
//! 复制进 `assets/` + 请求后台刷新住在面板外。

use crate::EditorApp;
use crate::panels::progress::{ProgressEvent, ProgressKind};
use crate::util::{
    display_path, format_project_error, save_recent_projects, strip_verbatim_prefix,
};
use editor::{
    ProjectDocument, ProjectInfo, RecoveryManager, RecoveryScan, ResourceBrowserState, open_project,
};
use rustlua_assets::import_cache::ImportCache;
use rustlua_assets::importer::{ImportSummary, run_import_batch_with_progress};
use rustlua_assets::resource_db::{AssetId, ResourceDatabase};
use rustlua_assets::resource_kind::ResourceKind;
use rustlua_scene::scene::SceneEntityId;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::AtomicBool,
        mpsc::{Sender, channel},
    },
    thread,
};

impl EditorApp {
    /// 启动导入游戏项目（后台执行，非阻塞）。
    pub(crate) fn start_import(&mut self) {
        let path = self.import_path.trim().to_owned();
        if path.is_empty() {
            self.status = "Enter a game project directory first".to_owned();
            return;
        }
        if self.progress.running {
            return;
        }
        let (tx, rx) = channel();
        spawn_import(PathBuf::from(path), tx);
        self.progress
            .start(ProgressKind::ImportProject, "导入游戏项目", rx, None, None);
    }

    /// 每帧轮询通用进度面板；任务完成时应用结果（导入）并落盘发布日志。
    pub(crate) fn poll_progress(&mut self) {
        let finished = self.progress.poll();
        if !finished {
            return;
        }
        self.apply_progress_result();
        // 发布：把输出落盘到 <workspace>/artifacts/builds/release/publish-last.log
        if let Some(dir) = &self.progress.output_dir
            && let Some(parent) = dir.parent()
        {
            let log = parent.join("publish-last.log");
            let _ = std::fs::create_dir_all(parent);
            let _ = std::fs::write(&log, &self.progress.output);
        }
    }

    /// 启动资源后台导入（资源面板刷新后的待导入项），非阻塞。
    pub(crate) fn start_resource_import(&mut self) {
        if self.progress.running || !self.project_loaded {
            return;
        }
        // 后台刷新在途时暂停新导入：`pending_imports` 来自上一次落地的刷新，
        // 在途刷新落地后会整体替换状态（含待导入项）。此刻起导入只会重复上一轮
        // 队列（导入完成后同帧弹窗立刻重开，进度条反复停在 100% 进行中）。
        if self.resource_refresh_job.is_some() {
            return;
        }
        let pending = self.resources.pending_imports().to_vec();
        if pending.is_empty() {
            return;
        }
        let root = self.project.root.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = cancel.clone();
        let (tx, rx) = channel();
        spawn_resource_import(root, pending.clone(), thread_cancel, tx);
        self.resources.record_import_attempts(&pending);
        self.progress.start(
            ProgressKind::ResourceImport,
            "导入资源",
            rx,
            None,
            Some(cancel),
        );
    }

    /// 任务完成：按任务类型应用后台结果。
    fn apply_progress_result(&mut self) {
        if !self.progress.done {
            return;
        }
        match self.progress.kind {
            // 资源导入完成：重新扫描读取最终状态（Ready/ImportFailed）。
            ProgressKind::ResourceImport => {
                if self.progress.success {
                    self.refresh_resources();
                    self.status = format!("资源导入完成：{}", display_path(&self.project.root));
                    // 资源导入成功：自动关闭进度面板（失败保留窗口查看原因），
                    // 与项目导入成功行为一致。
                    self.progress.show = false;
                }
            }
            ProgressKind::ImportProject => {
                let Some(result) = self.progress.result.take() else {
                    return;
                };
                if let Ok(outcome) = result.downcast::<ImportOutcome>() {
                    let outcome = *outcome;
                    self.project = outcome.project;
                    self.document = outcome.document;
                    self.recovery = outcome.recovery;
                    self.recovery_scan = outcome.recovery_scan;
                    self.resources = outcome.resources;
                    self.resource_tree_cache = None;
                    // 项目切换：清空旧项目缩略图缓存并让后台解码结果失效。
                    self.thumbnails.clear();
                    self.thumbnail_worker.clear();
                    // 源 Prefab 缓存按项目根解析出的绝对路径缓存，跨项目清空避免残留。
                    self.prefab_source_cache.clear();
                    // 字体图集按项目缓存（project.toml `font` 键）：切换项目后重建。
                    self.scene_font_atlas = None;
                    self.scene_vector_atlas = None;
                    // Scene View renderer 内部缓存项目 root；跨项目必须重建，
                    // 否则相对纹理路径会解析到上一个项目。
                    self.reset_scene_render_resources();
                    // 自定义字体缓存按项目路径缓存，同样随项目切换清空。
                    self.custom_font_atlases.lock().unwrap().clear();
                    self.custom_vector_atlases.lock().unwrap().clear();
                    // 项目设置缓冲按项目读取，切换后重置为默认（下次打开再读取）。
                    self.project_settings = Default::default();
                    // 发布面板的「发布名 / 微信 appid」同样按项目读取：不重新播种的话，
                    // 面板里还是上一个工程的值，点发布或关面板就会把它写进新工程的 project.toml。
                    self.publish.reseed(&self.project.root);
                    // 图集面板的配置同样按工程读取：不重播的话会拿上一个工程的
                    // `atlas.json` 去扫新工程，甚至把它保存进新工程。
                    self.atlas
                        .reseed(&crate::util::strip_verbatim_prefix(&self.project.root));
                    // 分包配置同样按工程读取：否则会把上一个工程的 bundles.json 存进新工程。
                    self.bundles
                        .reseed(&crate::util::strip_verbatim_prefix(&self.project.root));
                    self.history.clear();
                    self.autosave.reset();
                    self.play_session = None;
                    // E03 项目切换：整个文档被替换，清空统一选择模型。
                    self.selection.clear(editor::SelectionSource::Playback);
                    // H06：项目状态文件随新工程根切换；若新工程暂无场景则清空。
                    self.load_hierarchy_collapsed();
                    self.delete_target = crate::DeleteTarget::None;
                    self.project_loaded = true;
                    self.status = format!(
                        "Imported game project: {}",
                        display_path(&self.project.root)
                    );
                    let root = self.project.root.clone();
                    self.record_recent_project(&root);
                    // 导入成功：自动关闭进度面板（失败保留窗口查看原因）。
                    self.progress.show = false;
                }
            }
            ProgressKind::Publish => {}
        }
    }

    /// 记录最近打开的项目（去重、最新在前、最多 5 个），并落盘。
    pub(crate) fn record_recent_project(&mut self, path: &Path) {
        let clean = strip_verbatim_prefix(path).to_string_lossy().into_owned();
        self.recent_projects.retain(|p| p != &clean);
        self.recent_projects.insert(0, clean);
        self.recent_projects.truncate(5);
        save_recent_projects(&self.recent_projects);
    }

    /// 打开最近项目（项目菜单用）：E06 先经未保存确认网关，确认后走后台导入流程。
    pub(crate) fn open_recent_project(&mut self, path: String) {
        self.request_document_change(crate::actions::PendingChange::SwitchProject { path });
    }
}

/// 一次外部文件入库的结果明细（状态栏文案要如实说出"几个成功、几个失败"）。
pub(crate) struct IngestReport {
    /// 成功进入工程的**相对路径**（`assets/textures/x.png`）。
    pub(crate) imported: Vec<String>,
    /// 逐条失败原因（复制报错、目标目录建不出来）。
    failures: Vec<String>,
    /// 跳过的条目数（不是文件：设备路径、已消失的临时文件）。
    skipped: usize,
}

impl IngestReport {
    pub(crate) fn status_text(&self) -> String {
        let mut text = if self.imported.is_empty() {
            "未导入任何文件".to_owned()
        } else {
            format!("已导入 {} 个文件", self.imported.len())
        };
        if self.skipped > 0 {
            text.push_str(&format!("，跳过 {} 个非文件条目", self.skipped));
        }
        if !self.failures.is_empty() {
            text.push_str(&format!(
                "；{} 个失败：{}",
                self.failures.len(),
                self.failures[0]
            ));
        }
        text
    }
}

impl EditorApp {
    /// M27-01：把系统拖进来的文件复制进 `assets/<家族目录>`，随后请求一次后台资源刷新。
    ///
    /// 三条不变量：① **永不覆盖**——重名由 `dnd::target_path` 让路（`x.png` → `x-1.png`）；
    /// ② **工程内拖入不产生副本**（从资源浏览器拖自己的资产时保留原路径）；
    /// ③ 部分失败不影响成功项，逐条列进状态栏文案。
    pub(crate) fn import_external_files(&mut self, files: &[PathBuf]) -> IngestReport {
        let mut report = IngestReport {
            imported: Vec::new(),
            failures: Vec::new(),
            skipped: 0,
        };
        if !self.project_loaded {
            report.failures.push("请先打开游戏项目".to_owned());
            return report;
        }
        let root = strip_verbatim_prefix(&self.project.root);
        for source in files {
            if !source.is_file() {
                report.skipped += 1;
                continue;
            }
            let within_project = source.starts_with(&root);
            let target = if within_project {
                source.clone()
            } else {
                crate::dnd::target_path(&root, source)
            };
            let name = target
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| target.display().to_string());
            let result = (|| -> Result<(), String> {
                let dir = target.parent().ok_or("目标路径没有父目录")?;
                std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
                if !within_project {
                    std::fs::copy(source, &target).map_err(|error| error.to_string())?;
                }
                Ok(())
            })();
            match result {
                Err(error) => report.failures.push(format!("{name}：{error}")),
                Ok(()) => {
                    if let Some(relative) = crate::dnd::relative_asset(&root, &target) {
                        report.imported.push(relative);
                    }
                }
            }
        }
        if !report.imported.is_empty() {
            // 复制完交给既有的**后台**扫描：新文件进 pending_imports 后由
            // `start_resource_import` 转成导入任务（同步扫描会卡住 UI 线程）。
            self.refresh_resources();
        }
        report
    }

    /// 只入库（资源浏览器落点）。
    pub(crate) fn ingest_external_files(&mut self, files: &[PathBuf]) -> String {
        self.import_external_files(files).status_text()
    }

    /// 入库 + 为可生成的那批建实体（画布/层级落点）。
    ///
    /// Play 锁定在复制**之前**就拒绝：拖进来的文件不该在只读预览期间落盘。
    pub(crate) fn ingest_and_spawn(
        &mut self,
        files: &[PathBuf],
        parent: Option<SceneEntityId>,
    ) -> String {
        if self.is_playing() {
            return "Play mode locks scene editing; click Stop first".to_owned();
        }
        let report = self.import_external_files(files);
        let spawns: Vec<String> = report
            .imported
            .iter()
            .filter(|relative| {
                crate::dnd::referenceable(ResourceKind::from_path(Path::new(relative)))
            })
            .cloned()
            .collect();
        let ingest = report.status_text();
        if spawns.is_empty() {
            return if ingest == "未导入任何文件" {
                "该类型不能直接建实体（音频/字体/Lua 只能进资源库）".to_owned()
            } else {
                format!("{ingest}；该类型不能直接建实体")
            };
        }
        // 导入本身没有 Undo（它是文件系统动作），只有建实体那一步进历史。
        format!("{}；{}", self.spawn_dropped_batch(&spawns, parent), ingest)
    }
}

/// 导入游戏项目的后台结果。
struct ImportOutcome {
    project: ProjectInfo,
    document: ProjectDocument,
    recovery: RecoveryManager,
    recovery_scan: RecoveryScan,
    resources: ResourceBrowserState,
}

/// 从 panic payload 里提取可读消息。
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "未知原因".to_owned()
    }
}

/// 后台导入游戏项目：打开项目 + 加载文档 + 扫描资源，结果回传主线程。
///
/// worker 用 `catch_unwind` 兜底：线程一旦 panic（此前只能静默死亡），也要回传
/// `Finished{success:false}`，否则"导入游戏项目"进度面板会永远停在"进行中"。
fn spawn_import(path: PathBuf, tx: Sender<ProgressEvent>) {
    thread::spawn(move || {
        let _ = tx.send(ProgressEvent::Line(format!(
            "打开项目 {} ...\n",
            display_path(&path)
        )));
        let _ = tx.send(ProgressEvent::Progress(0.1));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || -> Result<ImportOutcome, String> {
                let project =
                    open_project(&path).map_err(|error| format_project_error(&path, error))?;
                if !project.root.join("project.toml").is_file() {
                    return Err("不是独立游戏项目：缺少 project.toml".to_owned());
                }
                let _ = tx.send(ProgressEvent::Progress(0.3));
                let document = ProjectDocument::open(project.clone());
                let _ = tx.send(ProgressEvent::Progress(0.55));
                let recovery = RecoveryManager::new(project.root.join(".temp"));
                // 恢复快照扫描失败要**说出来**：静默当作"没有快照"会让用户以为编辑内容
                // 已经没了，也让"恢复目录不可读"这类环境问题查不出来。
                let recovery_scan = match recovery.scan() {
                    Ok(scan) => scan,
                    Err(error) => {
                        let _ = tx.send(ProgressEvent::Line(format!(
                            "警告：崩溃恢复快照扫描失败（{error}）；本次导入不会列出待恢复内容。\n"
                        )));
                        RecoveryScan::default()
                    }
                };
                let _ = tx.send(ProgressEvent::Progress(0.7));
                let mut resources = ResourceBrowserState::new();
                if let Err(error) = resources.refresh(&project.root) {
                    return Err(format!("资源扫描失败：{error}"));
                }
                // 资源扫描是最重的一步，完成后接近收尾。
                let _ = tx.send(ProgressEvent::Progress(0.9));
                Ok(ImportOutcome {
                    project,
                    document,
                    recovery,
                    recovery_scan,
                    resources,
                })
            },
        ));
        match result {
            Ok(Ok(outcome)) => {
                let _ = tx.send(ProgressEvent::Line("导入完成。\n".to_owned()));
                let _ = tx.send(ProgressEvent::Finished {
                    success: true,
                    result: Some(Box::new(outcome)),
                });
            }
            Ok(Err(error)) => {
                let _ = tx.send(ProgressEvent::Line(format!("导入失败：{error}\n")));
                let _ = tx.send(ProgressEvent::Finished {
                    success: false,
                    result: None,
                });
            }
            Err(panic) => {
                let message = panic_message(&*panic);
                let _ = tx.send(ProgressEvent::Line(format!(
                    "导入线程异常退出：{message}\n"
                )));
                let _ = tx.send(ProgressEvent::Finished {
                    success: false,
                    result: None,
                });
            }
        }
    });
}

/// 后台资源导入：对待导入项跑 importer batch，回写 DB，进度逐行回传。
/// 不依赖前台状态：自行打开 DB/缓存实例，原子写盘防撕裂。
/// worker 用 `catch_unwind` 兜底：线程一旦 panic（此前只能静默死亡），也要回传
/// `Finished{success:false}`，否则"导入资源"进度面板会永远停在"进行中"。
fn spawn_resource_import(
    root: PathBuf,
    items: Vec<(AssetId, PathBuf)>,
    cancel: Arc<AtomicBool>,
    tx: Sender<ProgressEvent>,
) {
    thread::spawn(move || {
        let _ = tx.send(ProgressEvent::Line(format!(
            "导入 {} 个资源...\n",
            items.len()
        )));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || -> Result<ImportSummary, String> {
                let mut database = ResourceDatabase::open(&root)
                    .map_err(|error| format!("资源库打开失败：{error}"))?;
                let mut cache = ImportCache::open(&root)
                    .map_err(|error| format!("导入缓存打开失败：{error}"))?;
                // core 拆分后 builtin() 不含 Scene/Prefab，编辑器导入必须用完整注册表。
                let registry = rustlua_scene::asset_import::full_registry();
                let items: Vec<(AssetId, PathBuf)> = items
                    .into_iter()
                    .map(|(id, relative)| (id, root.join(relative)))
                    .collect();
                // 按实际导入数量回报进度，驱动确定进度条。
                let tx_progress = tx.clone();
                let summary = run_import_batch_with_progress(
                    &root,
                    &mut database,
                    &mut cache,
                    &registry,
                    &items,
                    &cancel,
                    |done, total| {
                        let _ =
                            tx_progress.send(ProgressEvent::Progress(done as f32 / total as f32));
                    },
                );
                // 导入可能修复/恢复了依赖（如失败的纹理重新就绪）：重新传播失效，
                // 把消费者从 Invalid/Stale 撤回到可消费状态，再写盘。进度已到 100%，
                // 先告知用户正在收尾，避免"100% 却一直进行中"被误认作卡死。
                let _ = tx.send(ProgressEvent::Line("正在保存资源库…\n".to_owned()));
                database.propagate_invalidation();
                database
                    .save()
                    .map_err(|error| format!("保存资源数据库失败：{error}"))?;
                Ok(summary)
            },
        ));
        match result {
            Ok(Ok(summary)) => {
                let _ = tx.send(ProgressEvent::Line(format!(
                    "导入完成：{} 个（导入 {}/复用 {}/失败 {}/跳过 {}），耗时 {}ms\n",
                    summary.total,
                    summary.imported,
                    summary.reused,
                    summary.failed,
                    summary.skipped,
                    summary.elapsed_ms
                )));
                let _ = tx.send(ProgressEvent::Finished {
                    success: true,
                    result: None,
                });
            }
            Ok(Err(error)) => {
                let _ = tx.send(ProgressEvent::Line(format!("资源导入失败：{error}\n")));
                let _ = tx.send(ProgressEvent::Finished {
                    success: false,
                    result: None,
                });
            }
            Err(panic) => {
                let message = panic_message(&*panic);
                let _ = tx.send(ProgressEvent::Line(format!(
                    "资源导入线程异常退出：{message}\n"
                )));
                let _ = tx.send(ProgressEvent::Finished {
                    success: false,
                    result: None,
                });
            }
        }
    });
}

#[cfg(test)]
mod import_tests {
    use super::*;
    use rustlua_infra::logging::LogSink;

    #[test]
    fn project_import_resets_scene_render_resources() {
        // 旧项目样例：sourcehan_test 已随示例重组删除，改用 game_api 样例工程。
        let old_project =
            open_project(rustlua_infra::games_dir().join("examples").join("game_api"))
                .expect("open old project");
        let new_project = open_project(rustlua_infra::games_dir().join("examples/game_jhyt"))
            .expect("open new project");
        let mut app = EditorApp::new(old_project, None, LogSink::new(64));
        app.scene_screen_cache_key = Some((12, (1280, 720)));

        let document = ProjectDocument::open(new_project.clone());
        let recovery = RecoveryManager::new(new_project.root.join(".temp"));
        let recovery_scan = recovery.scan().unwrap_or_default();
        let resources = ResourceBrowserState::new();
        app.progress.kind = ProgressKind::ImportProject;
        app.progress.done = true;
        app.progress.success = true;
        app.progress.show = true;
        app.progress.result = Some(Box::new(ImportOutcome {
            project: new_project,
            document,
            recovery,
            recovery_scan,
            resources,
        }));

        app.apply_progress_result();
        assert!(
            app.scene_renderer.is_none(),
            "renderer root must be rebuilt"
        );
        assert!(app.scene_texture.is_none());
        assert!(app.scene_screen_texture.is_none());
        assert_eq!(app.scene_screen_cache_key, None);
    }

    /// 资源导入成功应自动关闭进度面板（与项目导入成功行为一致）；失败保留窗口。
    #[test]
    fn resource_import_success_auto_closes_progress_panel() {
        let project =
            open_project(rustlua_infra::games_dir().join("examples").join("game_api")).unwrap();
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        // 模拟一个已完成且成功的资源导入。
        app.progress.kind = ProgressKind::ResourceImport;
        app.progress.done = true;
        app.progress.success = true;
        app.progress.show = true;
        app.apply_progress_result();
        assert!(!app.progress.show, "资源导入成功应自动关闭进度面板");
    }

    /// 资源导入失败应保留进度面板，便于查看失败原因。
    #[test]
    fn resource_import_failure_keeps_panel_open() {
        let project =
            open_project(rustlua_infra::games_dir().join("examples").join("game_api")).unwrap();
        let mut app = EditorApp::new(project, None, LogSink::new(64));
        app.progress.kind = ProgressKind::ResourceImport;
        app.progress.done = true;
        app.progress.success = false;
        app.progress.show = true;
        app.apply_progress_result();
        assert!(app.progress.show, "资源导入失败应保留进度面板查看原因");
    }

    #[test]
    fn import_spawn_end_to_end() {
        // 复现编辑器「导入游戏项目」后台路径：打开 game 项目并扫描资源。
        let game = rustlua_infra::games_dir().join("examples").join("game_api");
        let (tx, rx) = channel();
        spawn_import(game, tx);
        let mut output = String::new();
        let mut success = false;
        let mut max_progress: f32 = 0.0;
        while let Ok(event) = rx.recv() {
            match event {
                ProgressEvent::Line(text) => output.push_str(&text),
                ProgressEvent::Finished { success: s, result } => {
                    success = s;
                    assert!(
                        result.is_some(),
                        "导入应带回 ImportOutcome 结果；输出：\n{output}"
                    );
                    break;
                }
                ProgressEvent::Progress(value) => {
                    assert!((0.0..=1.0).contains(&value), "进度应在 0..=1：{value}");
                    max_progress = max_progress.max(value);
                }
            }
        }
        assert!(success, "导入应成功；输出：\n{output}");
        assert!(
            max_progress >= 0.9,
            "导入应按步骤回报真实进度；max={max_progress}"
        );
    }

    // ---- M27-01：系统拖放入库的三条不变量 ----

    /// 一个临时工程 + 一个"资源管理器里的外部目录"。
    struct DropFixture {
        project: PathBuf,
        staging: PathBuf,
    }

    impl DropFixture {
        fn new(label: &str) -> Self {
            let token = crate::util::test_token();
            let root = std::env::temp_dir().join(format!("rustlua-import-{label}-{token}"));
            // 外部目录必须在工程**外面**，否则它就不是"从资源管理器拖进来"了。
            let staging = std::env::temp_dir().join(format!("rustlua-import-{label}-{token}-out"));
            let _ = std::fs::remove_dir_all(&root);
            let _ = std::fs::remove_dir_all(&staging);
            std::fs::create_dir_all(&staging).expect("临时目录");
            std::fs::create_dir_all(&root).expect("临时工程目录");
            Self {
                project: root,
                staging,
            }
        }

        fn app(&self) -> EditorApp {
            let mut app = EditorApp::new(
                ProjectInfo {
                    root: self.project.clone(),
                    version: "test".to_owned(),
                    directories: Vec::new(),
                },
                None,
                LogSink::new(64),
            );
            app.project_loaded = true;
            app
        }

        fn write_outside(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.staging.join(name);
            std::fs::write(&path, bytes).expect("写外部文件");
            path
        }

        fn read_in_project(&self, relative: &str) -> Vec<u8> {
            std::fs::read(self.project.join(relative)).unwrap_or_default()
        }

        fn dispose(self) {
            let _ = std::fs::remove_dir_all(&self.project);
            let _ = std::fs::remove_dir_all(&self.staging);
        }
    }

    /// ① **永不覆盖**：第二次拖同名文件必须让路（`hero-1.png`），第一份内容不能被改。
    #[test]
    fn an_external_file_lands_in_its_family_folder_and_never_overwrites() {
        let fixture = DropFixture::new("no-overwrite");
        let mut app = fixture.app();
        let first = fixture.write_outside("hero.png", b"one");

        let report = app.import_external_files(std::slice::from_ref(&first));
        assert_eq!(report.imported, vec!["assets/textures/hero.png"]);
        assert_eq!(fixture.read_in_project("assets/textures/hero.png"), b"one");

        // 同名再来一次：外部文件内容已经变了，落点必须换名而不是覆盖。
        std::fs::write(&first, b"two").expect("改写外部文件");
        let second = app.import_external_files(&[first]);
        assert_eq!(second.imported, vec!["assets/textures/hero-1.png"]);
        assert_eq!(
            fixture.read_in_project("assets/textures/hero.png"),
            b"one",
            "已有资产不能被一次拖放覆盖"
        );
        assert_eq!(
            fixture.read_in_project("assets/textures/hero-1.png"),
            b"two"
        );
        drop(app);
        fixture.dispose();
    }

    /// ② **工程内拖入不产生副本**：从资源浏览器拖自家资产时保留原路径（不复制、不换名）。
    #[test]
    fn a_file_already_in_the_project_is_referenced_not_copied() {
        let fixture = DropFixture::new("in-project");
        let inside = fixture.project.join("assets/textures/inside.png");
        std::fs::create_dir_all(inside.parent().unwrap()).unwrap();
        std::fs::write(&inside, b"keep").unwrap();

        let report = fixture
            .app()
            .import_external_files(std::slice::from_ref(&inside));
        assert_eq!(report.imported, vec!["assets/textures/inside.png"]);
        assert!(
            !fixture
                .project
                .join("assets/textures/inside-1.png")
                .exists(),
            "工程内的文件拖回来不该留下第二份"
        );
        assert_eq!(std::fs::read(&inside).unwrap(), b"keep");
        fixture.dispose();
    }

    /// ③ 部分失败不影响成功项，而且要说出来：目录不是文件（跳过）、没开项目（整批拒绝）。
    #[test]
    fn partial_failures_are_skipped_or_reported_not_panicked() {
        let fixture = DropFixture::new("partial");
        let mut app = fixture.app();
        let good = fixture.write_outside("ok.png", b"png");
        let report = app.import_external_files(&[fixture.staging.clone(), good]);
        assert_eq!(report.imported, vec!["assets/textures/ok.png"]);
        assert_eq!(report.skipped, 1, "目录不是文件，只能跳过");
        assert!(report.status_text().contains("跳过 1 个非文件条目"));

        let mut closed = fixture.app();
        closed.project_loaded = false;
        let refused = closed.import_external_files(&[fixture.write_outside("x.png", b"png")]);
        assert!(refused.imported.is_empty());
        assert!(refused.status_text().contains("请先打开游戏项目"));
        drop(app);
        fixture.dispose();
    }
}
