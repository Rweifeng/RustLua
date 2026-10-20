use rustlua_assets::dependency::recompute_edges;
use rustlua_assets::resource_db::{
    AssetId, DiskAsset, ResourceDatabase, ResourceDbError, ResourceState,
};
use rustlua_assets::resource_kind::ResourceKind;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// 一个资源浏览器条目。路径始终是相对项目根目录的正斜杠路径。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceEntry {
    pub path: PathBuf,
    pub kind: ResourceKind,
    pub status: ResourceState,
    pub asset_id: Option<AssetId>,
    pub error: Option<String>,
}

impl ResourceEntry {
    pub fn path_string(&self) -> String {
        self.path.to_string_lossy().replace('\\', "/")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ResourceBrowserError {
    #[error("资源目录不存在：{}", .0.display())]
    MissingAssetsDirectory(PathBuf),
    #[error("扫描资源失败：{}：{message}", path.display())]
    Io { path: PathBuf, message: String },
    #[error("资源数据库错误：{0}")]
    Database(#[from] ResourceDbError),
}

/// 编辑器资源浏览器状态：扫描、筛选、刷新和错误状态均集中在这里。
#[derive(Debug, Default)]
pub struct ResourceBrowserState {
    filter: String,
    entries: Vec<ResourceEntry>,
    last_error: Option<String>,
    last_refresh: Option<SystemTime>,
    refresh_count: u64,
    /// 需要后台导入的资源（asset id + 项目相对路径），由 `refresh` 产出。
    pending_imports: Vec<(AssetId, PathBuf)>,
    /// 已启动导入的路径 → **尝试时**该文件的签名（mtime；文件不存在记 0）。
    ///
    /// 规则：**同一份内容只自动尝试一次**。文件内容再次变化后签名不同，才重新排队
    /// （`reconcile` 会把变更的文件转 `Stale`）。这条守卫对**所有**未完成状态生效，不只是
    /// `ImportFailed`——否则一个永远到不了 `Ready` 的资源（类型没有真正注册、文件已缺失等）
    /// 会让 `pending_imports` 每次刷新都非空，而 `start_resource_import` 是**每帧**调用的，
    /// 于是"导入资源"弹窗会不停重开、每轮还起一个后台线程。
    import_attempted: HashMap<PathBuf, u64>,
    /// 最近一次 `refresh` 看到的文件签名（相对路径 → mtime；不在盘上的记 0）。
    current_signatures: HashMap<PathBuf, u64>,
}

impl ResourceBrowserState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn set_filter(&mut self, filter: impl Into<String>) {
        self.filter = filter.into();
    }

    pub fn entries(&self) -> &[ResourceEntry] {
        &self.entries
    }

    pub fn visible_entries(&self) -> Vec<&ResourceEntry> {
        let filter = self.filter.trim().to_ascii_lowercase();
        self.entries
            .iter()
            .filter(|entry| {
                filter.is_empty() || entry.path_string().to_ascii_lowercase().contains(&filter)
            })
            .collect()
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn last_refresh(&self) -> Option<SystemTime> {
        self.last_refresh
    }

    pub fn refresh_count(&self) -> u64 {
        self.refresh_count
    }

    /// 需要后台导入的资源（相对项目根路径）。
    pub fn pending_imports(&self) -> &[(AssetId, PathBuf)] {
        &self.pending_imports
    }

    /// 记录本次已启动导入的路径**及其当时的文件签名**；同一签名的资源不再自动重试，
    /// 直到内容变化（签名不同）或用户显式刷新（`clear_import_attempts`）。
    pub fn record_import_attempts(&mut self, items: &[(AssetId, PathBuf)]) {
        for (_, path) in items {
            let signature = self.signature_of(path);
            self.import_attempted.insert(path.clone(), signature);
        }
    }

    /// 用户显式刷新（资源面板「刷新」/菜单「刷新资源」）时清空尝试记录，强制重新排队。
    pub fn clear_import_attempts(&mut self) {
        self.import_attempted.clear();
    }

    /// 跨刷新保留"已尝试"签名表，并按保留后的表**重算待导入队列**。
    ///
    /// 后台刷新（`RefreshHandle`）产出的是**全新**的 `ResourceBrowserState`
    /// （`import_attempted` 为空），其 `pending_imports` 是在空表下算出来的——未就绪
    /// 资源会全部重新排队。主线程应用前调用本方法：把旧表的签名搬过来（`import_attempted`
    /// 记的是**尝试时签名**，`current_signatures` 仍取新扫描的结果），再重算队列，
    /// 去重守卫才真正生效——否则未就绪资源每次刷新都重新导入，"导入资源"弹窗
    /// 无限重开、进度条反复停在 100% 进行中（第 49 轮报障的形态）。
    pub fn preserve_attempted_from(&mut self, previous: &Self) {
        self.import_attempted = previous.import_attempted.clone();
        self.recompute_pending();
    }
    /// 该路径在**最近一次刷新**里的签名（不在盘上 = 0）。
    fn signature_of(&self, path: &Path) -> u64 {
        self.current_signatures.get(path).copied().unwrap_or(0)
    }

    /// 扫描项目 assets 目录，并同步已识别资源到 ResourceDatabase。
    pub fn refresh(&mut self, project_root: impl AsRef<Path>) -> Result<(), ResourceBrowserError> {
        let project_root = project_root.as_ref();
        let assets_root = project_root.join("assets");
        self.entries.clear();
        self.last_error = None;
        self.refresh_count = self.refresh_count.saturating_add(1);

        if !assets_root.is_dir() {
            let error = ResourceBrowserError::MissingAssetsDirectory(assets_root);
            self.last_error = Some(error.to_string());
            return Err(error);
        }

        let mut found = Vec::new();
        collect_files(&assets_root, &mut found).inspect_err(|error| {
            self.last_error = Some(error.to_string());
        })?;
        // 场景文件目录（scenes/）。
        let scenes_root = project_root.join("scenes");
        if scenes_root.is_dir() {
            collect_files(&scenes_root, &mut found).inspect_err(|error| {
                self.last_error = Some(error.to_string());
            })?;
        }
        // Lua 脚本目录（scripts/）：让脚本也出现在资源浏览器中。
        let scripts_root = project_root.join("scripts");
        if scripts_root.is_dir() {
            collect_files(&scripts_root, &mut found).inspect_err(|error| {
                self.last_error = Some(error.to_string());
            })?;
        }
        found.sort_by(|a, b| a.0.cmp(&b.0));

        let mut database = ResourceDatabase::open(project_root)?;
        let mut disk_assets = Vec::with_capacity(found.len());
        // 已注册到数据库的条目延迟到 reconcile+传播后再重建，确保 UI 展示的
        // 状态反映最新数据库（依赖断链标 Invalid、文件删除标 Missing 等）。
        let mut registered: Vec<(PathBuf, ResourceKind, AssetId)> = Vec::new();
        // 本轮看到的文件签名：判断"已尝试过的资源内容是否又变了"。
        self.current_signatures.clear();
        for (path, modified) in found {
            let relative = path
                .strip_prefix(project_root)
                .expect("asset path must be inside project root")
                .to_path_buf();
            self.current_signatures.insert(relative.clone(), modified);
            disk_assets.push(DiskAsset {
                path: relative.clone(),
                modified,
            });
            let kind = ResourceKind::from_path(&path);
            if kind == ResourceKind::Other {
                self.entries.push(ResourceEntry {
                    path: relative,
                    kind,
                    status: ResourceState::ImportFailed,
                    asset_id: None,
                    error: Some("不支持的资源类型".to_owned()),
                });
                continue;
            }

            // 统一从 ImporterRegistry 取版本：assetc 与编辑器同源，避免版本漂移。
            // core 拆分后 builtin() 不含 Scene/Prefab 导入器，必须用完整注册表。
            let importer_version = rustlua_assets_import::full_registry()
                .get(kind)
                .map(rustlua_assets::importer::Importer::version)
                .unwrap_or(kind.token());
            match database.register(&relative, kind, importer_version) {
                Ok(asset_id) => registered.push((relative, kind, asset_id)),
                Err(error) => {
                    self.entries.push(ResourceEntry {
                        path: relative,
                        kind,
                        status: ResourceState::ImportFailed,
                        asset_id: None,
                        error: Some(error.to_string()),
                    });
                }
            }
        }

        // 用本次扫描结果协调状态（变更/删除/移出扫描根）。
        let mut scan_roots = Vec::new();
        for dir in ["assets", "scenes", "scripts"] {
            if project_root.join(dir).is_dir() {
                scan_roots.push(PathBuf::from(dir));
            }
        }
        database.reconcile(&disk_assets, &scan_roots);

        // 重算依赖边并传播失效：场景/预制体/脚本引用的资源被删除、导入失败
        // 或已变更时，消费者标 Invalid/Stale；依赖恢复后撤销回可消费状态。
        recompute_edges(&mut database, &|path| {
            fs::read_to_string(project_root.join(path)).ok()
        });
        database.propagate_invalidation();

        if let Err(error) = database.save() {
            self.last_error = Some(error.to_string());
            return Err(error.into());
        }

        // 用 reconcile+传播后的最新状态重建条目，并恢复按路径排序的展示顺序。
        for (relative, kind, asset_id) in registered {
            let record = database
                .get(asset_id)
                .expect("registered asset must be present in database");
            self.entries.push(ResourceEntry {
                path: relative,
                kind,
                status: record.status,
                asset_id: Some(asset_id),
                error: record.error.clone(),
            });
        }
        self.entries.sort_by(|a, b| a.path.cmp(&b.path));

        // 产出待导入资源：未导入/变更/丢失的都算，但**同一内容只自动尝试一次**
        // （`import_attempted` 存的是尝试时的签名）——否则一个永远到不了 `Ready` 的资源会让
        // `start_resource_import` 每帧重开"导入资源"弹窗（见字段注释）。内容变了签名就变，
        // 自然重新排队。Invalid（依赖断链）不加入——重导消费者自己无法修复，等依赖恢复后
        // 传播自动撤销。
        self.recompute_pending();
        self.last_refresh = Some(SystemTime::now());
        Ok(())
    }

    /// 按当前条目与"已尝试"签名表重算待导入队列（`refresh` 与
    /// `preserve_attempted_from` 共用）。
    fn recompute_pending(&mut self) {
        let attempted = &self.import_attempted;
        self.pending_imports = self
            .entries
            .iter()
            .filter_map(|entry| {
                let id = entry.asset_id?;
                if entry.kind == ResourceKind::Other || entry.status == ResourceState::Invalid {
                    return None;
                }
                let needs_import = matches!(
                    entry.status,
                    ResourceState::Discovered
                        | ResourceState::Stale
                        | ResourceState::Missing
                        | ResourceState::ImportFailed
                ) && attempted.get(&entry.path)
                    != Some(&self.signature_of(&entry.path));
                needs_import.then_some((id, entry.path.clone()))
            })
            .collect();
    }
}

fn collect_files(
    directory: &Path,
    output: &mut Vec<(PathBuf, u64)>,
) -> Result<(), ResourceBrowserError> {
    let mut children = fs::read_dir(directory)
        .map_err(|error| ResourceBrowserError::Io {
            path: directory.to_path_buf(),
            message: error.to_string(),
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ResourceBrowserError::Io {
            path: directory.to_path_buf(),
            message: error.to_string(),
        })?;
    children.sort_by_key(|entry| entry.path());
    for child in children {
        let path = child.path();
        let file_type = child
            .file_type()
            .map_err(|error| ResourceBrowserError::Io {
                path: path.clone(),
                message: error.to_string(),
            })?;
        if file_type.is_dir() {
            collect_files(&path, output)?;
        } else if file_type.is_file() {
            let modified = modified_nanos(&path)?;
            output.push((path, modified));
        }
    }
    Ok(())
}

fn modified_nanos(path: &Path) -> Result<u64, ResourceBrowserError> {
    let modified = fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| ResourceBrowserError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    Ok(modified
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rustlua-m5-05-{label}-{}", std::process::id()))
    }

    fn file(root: &Path, relative: &str, contents: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::File::create(path).unwrap().write_all(contents).unwrap();
    }

    fn unique_suffix() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be valid")
            .as_nanos()
    }

    fn sample_project_root() -> PathBuf {
        rustlua_infra::games_dir().join("examples").join("game_api")
    }

    fn copy_file(from: &Path, to: &Path) -> std::io::Result<()> {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(from, to)?;
        Ok(())
    }

    fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let src = entry.path();
            let dst = to.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy_dir(&src, &dst)?;
            } else {
                copy_file(&src, &dst)?;
            }
        }
        Ok(())
    }

    fn copy_sample_project(label: &str) -> Option<PathBuf> {
        let source = sample_project_root();
        if !source.join("project.toml").is_file() {
            return None;
        }
        let root = std::env::temp_dir().join(format!(
            "rustlua-m14-02-{label}-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        copy_file(&source.join("project.toml"), &root.join("project.toml")).unwrap();
        copy_dir(&source.join("scripts"), &root.join("scripts")).unwrap();
        copy_dir(&source.join("scenes"), &root.join("scenes")).unwrap();
        copy_dir(&source.join("assets"), &root.join("assets")).unwrap();
        Some(root)
    }

    fn replace_in_file(path: &Path, from: &str, to: &str) {
        let text = fs::read_to_string(path).unwrap();
        assert!(
            text.contains(from),
            "expected {} to contain {from}",
            path.display()
        );
        fs::write(path, text.replace(from, to)).unwrap();
    }

    #[test]
    fn refresh_scans_supported_and_unsupported_files() {
        let root = temp_root("scan");
        file(&root, "assets/player.png", b"png");
        file(&root, "assets/scripts/player.lua", b"return {}");
        file(&root, "assets/readme.txt", b"text");
        let mut browser = ResourceBrowserState::new();
        browser.refresh(&root).unwrap();
        assert_eq!(browser.entries().len(), 3);
        assert_eq!(browser.entries()[0].path_string(), "assets/player.png");
        assert_eq!(browser.entries()[0].status, ResourceState::Discovered);
        assert_eq!(
            browser
                .entries()
                .iter()
                .find(|entry| entry.path_string() == "assets/readme.txt")
                .and_then(|entry| entry.error.as_deref()),
            Some("不支持的资源类型")
        );
        assert_eq!(browser.refresh_count(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn refresh_registers_audio_ogg_wav_and_mp3() {
        let root = temp_root("audio");
        file(&root, "assets/audio/attack.ogg", b"ogg");
        file(&root, "assets/audio/bgm.wav", b"wav");
        file(&root, "assets/audio/theme.mp3", b"mp3");
        file(&root, "assets/readme.txt", b"text");
        let mut browser = ResourceBrowserState::new();
        browser.refresh(&root).unwrap();
        let audio: Vec<_> = browser
            .entries()
            .iter()
            .filter(|entry| entry.kind == ResourceKind::Audio)
            .map(|entry| entry.path_string())
            .collect();
        assert_eq!(
            audio.len(),
            3,
            "ogg, wav and mp3 recognized as audio: {audio:?}"
        );
        assert!(
            browser
                .entries()
                .iter()
                .filter(|entry| entry.kind == ResourceKind::Audio)
                .all(|entry| entry.status == ResourceState::Discovered),
            "audio files registered into the resource database"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn refresh_scans_lua_scripts_from_scripts_dir() {
        let root = temp_root("scripts");
        file(&root, "assets/player.png", b"png");
        file(&root, "scripts/main.lua", b"return {}");
        file(&root, "scripts/ui/hud.lua", b"return {}");
        let mut browser = ResourceBrowserState::new();
        browser.refresh(&root).unwrap();
        let lua: Vec<_> = browser
            .entries()
            .iter()
            .filter(|entry| entry.kind == ResourceKind::LuaScript)
            .map(|entry| entry.path_string())
            .collect();
        assert_eq!(lua, vec!["scripts/main.lua", "scripts/ui/hud.lua"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn filter_matches_case_insensitive_relative_path() {
        let root = temp_root("filter");
        file(&root, "assets/UI/Button.PNG", b"png");
        file(&root, "assets/player.lua", b"return {}");
        let mut browser = ResourceBrowserState::new();
        browser.refresh(&root).unwrap();
        browser.set_filter("button");
        let visible = browser.visible_entries();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].path_string(), "assets/UI/Button.PNG");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn refresh_picks_up_new_file_and_keeps_database_identity() {
        let root = temp_root("refresh");
        file(&root, "assets/player.png", b"png");
        let mut browser = ResourceBrowserState::new();
        browser.refresh(&root).unwrap();
        let id = browser.entries()[0].asset_id;
        file(&root, "assets/enemy.png", b"png");
        browser.refresh(&root).unwrap();
        assert_eq!(browser.entries().len(), 2);
        assert_eq!(
            browser
                .entries()
                .iter()
                .find(|e| e.path_string() == "assets/player.png")
                .unwrap()
                .asset_id,
            id
        );
        assert_eq!(browser.refresh_count(), 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn refresh_produces_pending_imports_for_discovered_files() {
        let root = temp_root("pending");
        file(&root, "assets/player.png", b"png");
        file(&root, "assets/readme.txt", b"text");
        let mut browser = ResourceBrowserState::new();
        browser.refresh(&root).unwrap();
        let pending = browser.pending_imports().to_vec();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].1, PathBuf::from("assets/player.png"));
        assert_eq!(pending[0].0, browser.entries()[0].asset_id.unwrap());
        let _ = fs::remove_dir_all(root);
    }

    /// **回归**（第 49 轮）：一个永远到不了 `Ready` 的资源不得让"导入资源"弹窗每帧重开。
    ///
    /// 触发链：`start_resource_import` 每帧检查 `pending_imports`；只要它非空就再起一次导入，
    /// 弹窗随之重开。旧实现只对 `ImportFailed` 记住"已尝试"，于是 `Discovered`/`Stale`/
    /// `Missing` 的资源每次刷新都重新排队 → 弹窗（和后台线程）无限重复。
    /// 现在按**文件签名**记：同一内容只自动尝试一次；内容变化或用户显式刷新才重排。
    #[test]
    fn unchanged_assets_are_not_retried_after_attempt() {
        let root = temp_root("no-loop");
        file(&root, "assets/player.png", b"png");
        let mut browser = ResourceBrowserState::new();
        browser.refresh(&root).unwrap();
        let pending = browser.pending_imports().to_vec();
        assert_eq!(pending.len(), 1);

        // 模拟"导入跑完了但状态没变"（该资源类型没真正注册 / 文件已缺失等）。
        browser.record_import_attempts(&pending);
        browser.refresh(&root).unwrap();
        assert!(
            browser.pending_imports().is_empty(),
            "同一内容不得自动重试，否则导入弹窗每帧重开"
        );
        // 再刷新几次也必须保持空（弹窗不会自己回来）。
        browser.refresh(&root).unwrap();
        assert!(browser.pending_imports().is_empty());

        // 用户显式刷新（资源面板「刷新」/菜单「刷新资源」）→ 强制重新排队。
        browser.clear_import_attempts();
        browser.refresh(&root).unwrap();
        assert_eq!(browser.pending_imports().len(), 1, "显式刷新必须能强制重试");

        // 文件内容变化 → 签名变化 → 自动重新排队（与 staled_files 那条门一致）。
        let pending = browser.pending_imports().to_vec();
        browser.record_import_attempts(&pending);
        file(&root, "assets/player.png", b"png3");
        browser.refresh(&root).unwrap();
        assert_eq!(
            browser.pending_imports().len(),
            1,
            "内容变了就该自动再试一次"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn failed_imports_are_not_automatically_retried_after_attempt() {
        let root = temp_root("attempted");
        file(&root, "assets/player.png", b"png");
        let mut browser = ResourceBrowserState::new();
        browser.refresh(&root).unwrap();
        let pending = browser.pending_imports().to_vec();
        assert_eq!(pending.len(), 1);
        // 模拟导入失败：DB 标 ImportFailed，并记录已尝试。
        {
            let mut db = ResourceDatabase::open(&root).unwrap();
            let record = db.find_by_path("assets/player.png").unwrap().unwrap();
            db.mark_failed(record.id, "模拟失败").unwrap();
            db.save().unwrap();
        }
        browser.record_import_attempts(&pending);
        browser.refresh(&root).unwrap();
        assert!(
            browser.pending_imports().is_empty(),
            "attempted+failed 资源不再自动重试"
        );
        let _ = fs::remove_dir_all(root);
    }

    /// **回归**（本轮报障）：去重守卫必须跨刷新存活。
    ///
    /// 生产路径里 `poll_resource_refresh` 用后台 worker 产出的**全新**状态整体替换
    /// `self.resources`（worker 的 `import_attempted` 是空的）。若不把旧状态的
    /// 已尝试签名搬过来，`unchanged_assets_are_not_retried_after_attempt` 的守卫只在
    /// "同一对象连续 refresh" 的测试里成立，真实编辑器里每次刷新都会把未就绪资源
    /// 重新排队 → "导入资源"弹窗无限重开、进度条反复停在 100% 进行中。
    #[test]
    fn attempted_dedup_survives_state_replacement() {
        let root = temp_root("state-replace");
        file(&root, "assets/player.png", b"png");
        // 第一轮刷新（模拟刷新 J1 落地后的状态）：产出待导入项。
        let mut applied = ResourceBrowserState::new();
        applied.refresh(&root).unwrap();
        let pending = applied.pending_imports().to_vec();
        assert_eq!(pending.len(), 1);
        // 导入启动：记录已尝试签名（模拟 start_resource_import 的 record_import_attempts）。
        applied.record_import_attempts(&pending);

        // 导入完成后再刷新：worker 产出全新状态（import_attempted 为空），
        // 主线程应用前必须 preserve_attempted_from 旧状态，否则去重失效。
        let mut worker_state = ResourceBrowserState::new();
        worker_state.refresh(&root).unwrap();
        assert!(
            worker_state.pending_imports().len() == 1,
            "worker 全新状态没有去重记录，待导入项重新出现"
        );
        worker_state.preserve_attempted_from(&applied);
        assert!(
            worker_state.pending_imports().is_empty(),
            "preserve 后同一内容不得再次排队"
        );
        // 再模拟几轮刷新替换：必须持续保持空（弹窗不会自己回来）。
        for _ in 0..3 {
            let mut next = ResourceBrowserState::new();
            next.refresh(&root).unwrap();
            next.preserve_attempted_from(&worker_state);
            assert!(next.pending_imports().is_empty());
            worker_state = next;
        }
        // 内容变化后签名不同 → 自动重新排队（语义不被破坏）。
        file(&root, "assets/player.png", b"png3");
        let mut changed = ResourceBrowserState::new();
        changed.refresh(&root).unwrap();
        changed.preserve_attempted_from(&worker_state);
        assert_eq!(
            changed.pending_imports().len(),
            1,
            "内容变了就该自动再试一次"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn staled_files_are_pending_even_after_attempt() {
        let root = temp_root("pending-stale");
        file(&root, "assets/player.png", b"png");
        let mut browser = ResourceBrowserState::new();
        browser.refresh(&root).unwrap();
        let pending = browser.pending_imports().to_vec();
        browser.record_import_attempts(&pending);
        // 修改文件内容 → mtime 变化 → reconcile 转 Stale → 应重新待导入。
        file(&root, "assets/player.png", b"png2");
        browser.refresh(&root).unwrap();
        assert_eq!(browser.pending_imports().len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_assets_directory_is_visible_as_error() {
        let root = temp_root("missing");
        let mut browser = ResourceBrowserState::new();
        assert!(browser.refresh(&root).is_err());
        assert!(browser.last_error().unwrap().contains("资源目录不存在"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn refresh_marks_scene_invalid_when_dependency_missing() {
        let root = temp_root("invalid-dep");
        file(&root, "assets/player.png", b"png");
        file(
            &root,
            "scenes/main.scene",
            b"[scene]\nSprite texture=assets/player.png;size=32\n",
        );
        let mut browser = ResourceBrowserState::new();
        browser.refresh(&root).unwrap();
        // 删除被场景引用的纹理 → 刷新后场景标 Invalid（断链）。
        fs::remove_file(root.join("assets/player.png")).unwrap();
        browser.refresh(&root).unwrap();
        let scene = browser
            .entries()
            .iter()
            .find(|entry| entry.path_string() == "scenes/main.scene")
            .expect("scene registered");
        assert_eq!(scene.status, ResourceState::Invalid);
        assert!(
            scene.error.as_deref().unwrap_or("").contains("缺失"),
            "error 应说明依赖缺失：{:?}",
            scene.error
        );
        let _ = fs::remove_dir_all(root);
    }

    /// 批 E（`docs/63`）：图集在资源浏览器里是一行**真资产**，并且
    /// `scene → atlas → texture` 两级边参与失效传导；`#区域名` 后缀不参与建边。
    #[test]
    fn atlas_rows_display_and_chain_texture_to_scene() {
        let root = temp_root("atlas-chain");
        file(&root, "assets/atlases/hero-0.png", b"png");
        file(
            &root,
            "assets/atlases/hero.atlas",
            b"atlas_version=1\ntexture=assets/atlases/hero-0.png\nregion\twalk\t2\t2\t32\t16\n",
        );
        file(
            &root,
            "scenes/main.scene",
            b"component 1 Sprite texture=assets/atlases/hero.atlas#walk;size=32x16\n",
        );
        let entry = |browser: &ResourceBrowserState, relative: &str| {
            browser
                .entries()
                .iter()
                .find(|entry| entry.path_string() == relative)
                .unwrap_or_else(|| panic!("{relative} 未登记"))
                .clone()
        };

        let mut browser = ResourceBrowserState::new();
        browser.refresh(&root).unwrap();
        assert_eq!(
            entry(&browser, "assets/atlases/hero.atlas").kind,
            ResourceKind::Atlas,
            "`.atlas` 必须归到图集家族而不是 Other"
        );
        assert!(
            browser
                .pending_imports()
                .iter()
                .any(|(_, path)| path == &PathBuf::from("assets/atlases/hero.atlas")),
            "未导入的图集要进待导入队列，编辑器才会跑真实校验"
        );

        // 把三者标成 Ready，再改源图内容：变更应沿 atlas→texture 边传导。
        {
            let mut db = ResourceDatabase::open(&root).unwrap();
            for (relative, hash) in [
                ("assets/atlases/hero-0.png", "h-tex"),
                ("assets/atlases/hero.atlas", "h-atlas"),
                ("scenes/main.scene", "h-scene"),
            ] {
                let record = db.find_by_path(relative).unwrap().unwrap();
                db.mark_ready(record.id, format!("cache/{relative}"), hash)
                    .unwrap();
            }
            db.save().unwrap();
        }
        file(&root, "assets/atlases/hero-0.png", b"png2");
        browser.refresh(&root).unwrap();
        assert_eq!(
            entry(&browser, "assets/atlases/hero-0.png").status,
            ResourceState::Stale,
            "源图自身变更"
        );
        assert_eq!(
            entry(&browser, "assets/atlases/hero.atlas").status,
            ResourceState::Stale,
            "源图变化沿 atlas→texture 边弄脏图集"
        );
        assert_eq!(
            entry(&browser, "scenes/main.scene").status,
            ResourceState::Stale,
            "再沿 scene→atlas 边传导"
        );
        assert!(
            browser
                .pending_imports()
                .iter()
                .any(|(_, path)| path == &PathBuf::from("assets/atlases/hero.atlas")),
            "变脏的图集重新排队重导入"
        );

        // 删掉图集文件：场景按图集路径断链（`#walk` 只在使用点解析）。
        fs::remove_file(root.join("assets/atlases/hero.atlas")).unwrap();
        browser.refresh(&root).unwrap();
        let scene = entry(&browser, "scenes/main.scene");
        assert_eq!(scene.status, ResourceState::Invalid);
        assert!(
            scene
                .error
                .as_deref()
                .unwrap_or("")
                .contains("assets/atlases/hero.atlas"),
            "断链文案要指出图集路径而非带 # 的原始引用：{:?}",
            scene.error
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pending_imports_exclude_invalid() {
        let root = temp_root("pending-invalid");
        file(&root, "assets/player.png", b"png");
        file(
            &root,
            "scenes/main.scene",
            b"[scene]\nSprite texture=assets/player.png;size=32\n",
        );
        let mut browser = ResourceBrowserState::new();
        browser.refresh(&root).unwrap();
        fs::remove_file(root.join("assets/player.png")).unwrap();
        browser.refresh(&root).unwrap();
        assert!(
            browser
                .pending_imports()
                .iter()
                .all(|(_, path)| path != &PathBuf::from("scenes/main.scene")),
            "Invalid 断链资源不进入待导入队列"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn refresh_restores_ready_when_dependency_returns() {
        let root = temp_root("restore");
        file(&root, "assets/player.png", b"png");
        file(
            &root,
            "scenes/main.scene",
            b"[scene]\nSprite texture=assets/player.png;size=32\n",
        );
        let mut browser = ResourceBrowserState::new();
        browser.refresh(&root).unwrap();
        // 导入依赖纹理 → Ready。
        {
            let mut db = ResourceDatabase::open(&root).unwrap();
            let record = db.find_by_path("assets/player.png").unwrap().unwrap();
            db.mark_ready(record.id, "cache/player.png", "h1").unwrap();
            db.save().unwrap();
        }
        // 删除依赖 → 场景 Invalid。
        fs::remove_file(root.join("assets/player.png")).unwrap();
        browser.refresh(&root).unwrap();
        let scene = browser
            .entries()
            .iter()
            .find(|entry| entry.path_string() == "scenes/main.scene")
            .expect("scene registered");
        assert_eq!(scene.status, ResourceState::Invalid);
        // 恢复依赖 → 撤销 Invalid，场景回到可消费状态（依赖重写后为 Stale）。
        file(&root, "assets/player.png", b"png");
        browser.refresh(&root).unwrap();
        let scene = browser
            .entries()
            .iter()
            .find(|entry| entry.path_string() == "scenes/main.scene")
            .expect("scene registered");
        assert_ne!(scene.status, ResourceState::Invalid);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sample_project_resource_rename_marks_consumers_invalid_and_recovers_after_fix() {
        let Some(root) = copy_sample_project("rename-fix") else {
            return;
        };
        let old_path = root.join("assets/textures/slime.png");
        let new_rel = "assets/textures/slime-renamed.png";
        let new_path = root.join(new_rel);
        let mut browser = ResourceBrowserState::new();

        browser.refresh(&root).unwrap();
        fs::rename(&old_path, &new_path).unwrap();
        browser.refresh(&root).unwrap();

        let invalid_paths: Vec<_> = browser
            .entries()
            .iter()
            .filter(|entry| entry.status == ResourceState::Invalid)
            .map(|entry| entry.path_string())
            .collect();
        assert!(
            invalid_paths.contains(&"assets/prefabs/slime_green.prefab".to_owned()),
            "green prefab should become invalid after texture rename: {invalid_paths:?}"
        );
        assert!(
            invalid_paths.contains(&"assets/prefabs/slime_red.prefab".to_owned()),
            "red prefab should become invalid after texture rename: {invalid_paths:?}"
        );
        assert!(
            invalid_paths.contains(&"scenes/api_gameplay.scene".to_owned()),
            "production scene should become invalid after texture rename: {invalid_paths:?}"
        );

        replace_in_file(
            &root.join("assets/prefabs/slime_green.prefab"),
            "assets/textures/slime.png",
            new_rel,
        );
        replace_in_file(
            &root.join("assets/prefabs/slime_red.prefab"),
            "assets/textures/slime.png",
            new_rel,
        );
        replace_in_file(
            &root.join("scenes/api_gameplay.scene"),
            "assets/textures/slime.png",
            new_rel,
        );

        browser.refresh(&root).unwrap();
        let invalid_paths: Vec<_> = browser
            .entries()
            .iter()
            .filter(|entry| entry.status == ResourceState::Invalid)
            .map(|entry| entry.path_string())
            .collect();
        assert!(
            !invalid_paths.contains(&"assets/prefabs/slime_green.prefab".to_owned())
                && !invalid_paths.contains(&"assets/prefabs/slime_red.prefab".to_owned())
                && !invalid_paths.contains(&"scenes/api_gameplay.scene".to_owned()),
            "updating consumers should clear invalid state: {invalid_paths:?}"
        );

        let _ = fs::remove_dir_all(root);
    }
}
