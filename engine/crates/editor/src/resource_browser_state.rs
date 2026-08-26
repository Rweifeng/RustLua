use engine::core::{
    dependency::recompute_edges,
    importer::ImporterRegistry,
    resource_db::{AssetId, DiskAsset, ResourceDatabase, ResourceDbError, ResourceState},
    resource_kind::ResourceKind,
};
use std::{
    collections::HashSet,
    error::Error,
    fmt, fs,
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

#[derive(Debug)]
pub enum ResourceBrowserError {
    MissingAssetsDirectory(PathBuf),
    Io { path: PathBuf, message: String },
    Database(ResourceDbError),
}

impl fmt::Display for ResourceBrowserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingAssetsDirectory(path) => {
                write!(f, "资源目录不存在：{}", path.display())
            }
            Self::Io { path, message } => write!(f, "扫描资源失败：{}：{message}", path.display()),
            Self::Database(error) => write!(f, "资源数据库错误：{error}"),
        }
    }
}

impl Error for ResourceBrowserError {}

impl From<ResourceDbError> for ResourceBrowserError {
    fn from(error: ResourceDbError) -> Self {
        Self::Database(error)
    }
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
    /// 已启动导入的路径；`ImportFailed` 的资源不再自动重试，直到内容再次
    /// 变化（reconcile 转 `Stale`）或用户重新导入，避免失败项无限循环重试。
    import_attempted: HashSet<PathBuf>,
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

    /// 记录本次已启动导入的路径；被标记的 `ImportFailed` 资源不再自动重试，
    /// 直到内容再次变化触发 reconcile 转 `Stale`。
    pub fn record_import_attempts(&mut self, items: &[(AssetId, PathBuf)]) {
        self.import_attempted
            .extend(items.iter().map(|(_, path)| path.clone()));
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
        for (path, modified) in found {
            let relative = path
                .strip_prefix(project_root)
                .expect("asset path must be inside project root")
                .to_path_buf();
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
            let importer_version = ImporterRegistry::builtin()
                .get(kind)
                .map(engine::core::importer::Importer::version)
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

        // 产出待导入资源：未导入/变更/丢失的总是加入；导入失败的仅当尚未
        // 尝试过（避免失败项在每次刷新后无限重试）。Invalid（依赖断链）不
        // 加入——重导消费者自己无法修复，等依赖恢复后传播自动撤销。
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
                    ResourceState::Discovered | ResourceState::Stale | ResourceState::Missing
                ) || (entry.status == ResourceState::ImportFailed
                    && !self.import_attempted.contains(&entry.path));
                needs_import.then_some((id, entry.path.clone()))
            })
            .collect();
        self.last_refresh = Some(SystemTime::now());
        Ok(())
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
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../game_test_2d_production")
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

    fn copy_sample_project(label: &str) -> PathBuf {
        let source = sample_project_root();
        assert!(
            source.join("project.toml").is_file(),
            "sample project missing: {}",
            source.display()
        );
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
        root
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
    fn refresh_registers_audio_ogg_and_wav() {
        let root = temp_root("audio");
        file(&root, "assets/audio/attack.ogg", b"ogg");
        file(&root, "assets/audio/bgm.wav", b"wav");
        file(&root, "assets/readme.txt", b"text");
        let mut browser = ResourceBrowserState::new();
        browser.refresh(&root).unwrap();
        let audio: Vec<_> = browser
            .entries()
            .iter()
            .filter(|entry| entry.kind == ResourceKind::Audio)
            .map(|entry| entry.path_string())
            .collect();
        assert_eq!(audio.len(), 2, "ogg and wav recognized as audio: {audio:?}");
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
            b"[scene]\nSprite assets/player.png;size=32\n",
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

    #[test]
    fn pending_imports_exclude_invalid() {
        let root = temp_root("pending-invalid");
        file(&root, "assets/player.png", b"png");
        file(
            &root,
            "scenes/main.scene",
            b"[scene]\nSprite assets/player.png;size=32\n",
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
            b"[scene]\nSprite assets/player.png;size=32\n",
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
        let root = copy_sample_project("rename-fix");
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
            invalid_paths.contains(&"scenes/production.scene".to_owned()),
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
            &root.join("scenes/production.scene"),
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
                && !invalid_paths.contains(&"scenes/production.scene".to_owned()),
            "updating consumers should clear invalid state: {invalid_paths:?}"
        );

        let _ = fs::remove_dir_all(root);
    }
}
