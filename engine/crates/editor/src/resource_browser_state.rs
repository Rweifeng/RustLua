use engine::core::{
    resource_db::{AssetId, DiskAsset, ResourceDatabase, ResourceDbError, ResourceState},
    resource_kind::ResourceKind,
};
use std::{
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

            let importer_version = format!("editor-m5-05-{}", kind.label());
            match database.register(&relative, kind, &importer_version) {
                Ok(asset_id) => {
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

        if let Err(error) = database.save() {
            self.last_error = Some(error.to_string());
            return Err(error.into());
        }
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

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rustlua-m5-05-{label}-{}", std::process::id()))
    }

    fn file(root: &Path, relative: &str, contents: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::File::create(path).unwrap().write_all(contents).unwrap();
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
    fn missing_assets_directory_is_visible_as_error() {
        let root = temp_root("missing");
        let mut browser = ResourceBrowserState::new();
        assert!(browser.refresh(&root).is_err());
        assert!(browser.last_error().unwrap().contains("资源目录不存在"));
        let _ = fs::remove_dir_all(root);
    }
}
