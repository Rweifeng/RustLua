use crate::{ProjectError, ProjectInfo};
use engine_core::scene::{Scene, SceneError};
use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// 编辑器中的可编辑场景及其文件状态。
#[derive(Clone, Debug)]
pub struct SceneDocument {
    pub path: Option<PathBuf>,
    pub scene: Scene,
    pub dirty: bool,
    /// 单调递增的编辑修订号：每次 `mark_dirty` 自增，供编辑器检测场景内容是否变化。
    /// 不参与相等比较（纯修订计数变化不算内容变化），避免干扰 history 的 no-op 检测。
    pub revision: u64,
}

impl PartialEq for SceneDocument {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path && self.scene == other.scene && self.dirty == other.dirty
    }
}

impl Eq for SceneDocument {}

impl SceneDocument {
    /// 创建尚未保存的新场景。新文档必须显示为 dirty，避免用户误以为已有文件。
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            path: None,
            scene: Scene::new(name),
            dirty: true,
            revision: 0,
        }
    }

    pub fn new_scene(name: impl Into<String>) -> Self {
        Self::new(name)
    }

    /// 从磁盘打开场景；只有成功解析和校验后才创建文档。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DocumentError> {
        let path = path.as_ref().to_path_buf();
        let scene = Scene::load(&path).map_err(DocumentError::Scene)?;
        Ok(Self {
            path: Some(path),
            scene,
            dirty: false,
            revision: 0,
        })
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
        // 每次标记 dirty 都自增修订号，让编辑器能够检测到场景内容变化（即使 dirty 早已为真）。
        self.revision = self.revision.wrapping_add(1);
    }
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// 保存到当前路径；未命名的新场景必须使用 `save_as`。
    pub fn save(&mut self) -> Result<(), DocumentError> {
        let path = self.path.clone().ok_or(DocumentError::MissingPath)?;
        self.save_to(&path)
    }

    /// 保存到新路径。成功后该路径成为文档当前路径。
    pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<(), DocumentError> {
        let path = path.as_ref().to_path_buf();
        self.save_to(&path)?;
        self.path = Some(path);
        Ok(())
    }

    fn save_to(&mut self, path: &Path) -> Result<(), DocumentError> {
        self.scene.validate().map_err(DocumentError::Scene)?;
        atomic_write_scene(path, &self.scene)?;
        self.dirty = false;
        Ok(())
    }
}

/// Play/Stop runtime snapshot used by the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaySession {
    editor_snapshot: SceneDocument,
    runtime_document: SceneDocument,
}

impl PlaySession {
    pub fn start(document: &SceneDocument) -> Self {
        Self {
            editor_snapshot: document.clone(),
            runtime_document: document.clone(),
        }
    }

    pub fn runtime_document(&self) -> &SceneDocument {
        &self.runtime_document
    }

    pub fn runtime_document_mut(&mut self) -> &mut SceneDocument {
        &mut self.runtime_document
    }

    pub fn editor_snapshot(&self) -> &SceneDocument {
        &self.editor_snapshot
    }

    /// Stop discards the runtime copy and returns the editor snapshot.
    pub fn stop(self) -> SceneDocument {
        self.editor_snapshot
    }
}

/// Open project editor document context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectDocument {
    pub project: ProjectInfo,
    pub current_scene: Option<SceneDocument>,
}

impl ProjectDocument {
    pub fn open(project: ProjectInfo) -> Self {
        Self {
            project,
            current_scene: None,
        }
    }

    pub fn new_scene(&mut self, name: impl Into<String>) -> &mut SceneDocument {
        self.current_scene = Some(SceneDocument::new(name));
        self.current_scene.as_mut().expect("scene was just created")
    }

    pub fn open_scene(&mut self, path: impl AsRef<Path>) -> Result<(), DocumentError> {
        self.current_scene = Some(SceneDocument::open(path)?);
        Ok(())
    }

    pub fn save_scene(&mut self) -> Result<(), DocumentError> {
        self.current_scene
            .as_mut()
            .ok_or(DocumentError::NoCurrentScene)?
            .save()
    }

    pub fn save_scene_as(&mut self, path: impl AsRef<Path>) -> Result<(), DocumentError> {
        self.current_scene
            .as_mut()
            .ok_or(DocumentError::NoCurrentScene)?
            .save_as(path)
    }
}

fn atomic_write_scene(path: &Path, scene: &Scene) -> Result<(), DocumentError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| DocumentError::Io {
        path: parent.to_path_buf(),
        message: error.to_string(),
    })?;
    let token = unique_token();
    let temp = parent.join(format!(".{}.{}.tmp", file_name(path), token));
    let backup = parent.join(format!(".{}.{}.bak", file_name(path), token));

    fs::write(&temp, scene.to_text().as_bytes()).map_err(|error| DocumentError::Io {
        path: temp.clone(),
        message: error.to_string(),
    })?;
    // 替换正式文件前重新加载临时文件，确保落盘内容可被引擎解析。
    if let Err(error) = Scene::load(&temp) {
        let _ = fs::remove_file(&temp);
        return Err(DocumentError::Scene(error));
    }

    let had_original = path.exists();
    if had_original {
        fs::rename(path, &backup).map_err(|error| {
            let _ = fs::remove_file(&temp);
            DocumentError::Io {
                path: path.to_path_buf(),
                message: error.to_string(),
            }
        })?;
    }
    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        if had_original {
            let _ = fs::rename(&backup, path);
        }
        return Err(DocumentError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        });
    }
    if had_original {
        fs::remove_file(&backup).map_err(|error| DocumentError::Io {
            path: backup,
            message: error.to_string(),
        })?;
    }
    Ok(())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("scene")
        .replace(['\\', '/', ':'], "_")
}

fn unique_token() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

#[derive(Debug)]
pub enum DocumentError {
    MissingPath,
    NoCurrentScene,
    Project(ProjectError),
    Scene(SceneError),
    Io { path: PathBuf, message: String },
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPath => write!(f, "场景尚未命名，请使用另存为选择文件路径"),
            Self::NoCurrentScene => write!(f, "当前项目没有打开的场景"),
            Self::Project(error) => write!(f, "项目文档错误：{error}"),
            Self::Scene(error) => write!(f, "场景文档错误：{error}"),
            Self::Io { path, message } => write!(f, "文档 I/O 失败：{}：{message}", path.display()),
        }
    }
}

impl Error for DocumentError {}
impl From<ProjectError> for DocumentError {
    fn from(error: ProjectError) -> Self {
        Self::Project(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::scene::SceneEntityId;

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rustlua-m5-02-{label}-{}", unique_token()))
    }

    fn scene() -> Scene {
        let mut scene = Scene::new("main");
        scene.add_entity(SceneEntityId(1), "Root", None).unwrap();
        scene.metadata.insert("author".into(), "ai".into());
        scene
    }

    #[test]
    fn new_document_is_dirty_and_has_no_path() {
        let document = SceneDocument::new_scene("main");
        assert!(document.is_dirty());
        assert!(document.path.is_none());
    }

    #[test]
    fn save_load_roundtrip_and_save_as_update_state() {
        let root = temp_root("roundtrip");
        let first = root.join("scenes/main.scene");
        let second = root.join("scenes/copy.scene");
        let mut document = SceneDocument::new_scene("main");
        document.scene = scene();
        document.save_as(&first).unwrap();
        assert!(!document.is_dirty());
        assert_eq!(SceneDocument::open(&first).unwrap().scene, document.scene);
        document.mark_dirty();
        document.save_as(&second).unwrap();
        assert_eq!(document.path, Some(second.clone()));
        assert!(!document.is_dirty());
        assert_eq!(Scene::load(second).unwrap(), document.scene);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn save_without_path_is_rejected_and_dirty_remains() {
        let mut document = SceneDocument::new("main");
        assert!(matches!(document.save(), Err(DocumentError::MissingPath)));
        assert!(document.is_dirty());
    }

    #[test]
    fn invalid_scene_does_not_modify_original_or_clear_dirty() {
        let root = temp_root("preserve");
        let path = root.join("main.scene");
        let mut document = SceneDocument::new_scene("main");
        document.scene = scene();
        document.save_as(&path).unwrap();
        let original = fs::read(&path).unwrap();
        document.scene.version = 999;
        document.mark_dirty();
        let error = document.save().unwrap_err();
        assert!(error.to_string().contains("unsupported scene version"));
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(document.is_dirty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn repeated_save_is_deterministic() {
        let root = temp_root("stable");
        let path = root.join("main.scene");
        let mut document = SceneDocument::new_scene("main");
        document.scene = scene();
        document.save_as(&path).unwrap();
        let first = fs::read(&path).unwrap();
        document.mark_dirty();
        document.save().unwrap();
        assert_eq!(fs::read(path).unwrap(), first);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn project_document_tracks_current_scene() {
        let root = temp_root("project");
        let project = ProjectInfo {
            root: root.clone(),
            version: "test".into(),
            directories: Vec::new(),
        };
        let mut document = ProjectDocument::open(project);
        assert!(document.current_scene.is_none());
        assert!(document.save_scene().is_err());
        document.new_scene("main");
        assert!(document.current_scene.as_ref().unwrap().is_dirty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn play_session_isolates_runtime_changes_and_restores_editor_snapshot() {
        let mut document = SceneDocument::new_scene("main");
        document
            .scene
            .add_entity(SceneEntityId(1), "Player", None)
            .unwrap();
        document.mark_clean();
        let original = document.clone();

        let mut session = PlaySession::start(&document);
        session
            .runtime_document_mut()
            .scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .name = "RuntimePlayer".into();
        session.runtime_document_mut().mark_dirty();

        assert_eq!(session.editor_snapshot(), &original);
        assert_eq!(
            session.runtime_document().scene.entities[&SceneEntityId(1)].name,
            "RuntimePlayer"
        );
        assert_eq!(document, original);
        assert_eq!(session.stop(), original);
    }

    #[test]
    fn play_stop_can_repeat_one_hundred_times_without_edit_state_drift() {
        let mut document = SceneDocument::new_scene("main");
        document
            .scene
            .add_entity(SceneEntityId(1), "Player", None)
            .unwrap();
        document.mark_clean();
        let original = document.clone();

        for iteration in 0..100 {
            let mut session = PlaySession::start(&document);
            session
                .runtime_document_mut()
                .scene
                .metadata
                .insert("frame".into(), iteration.to_string());
            session.runtime_document_mut().mark_dirty();
            let restored = session.stop();
            assert_eq!(restored, original);
            assert_eq!(document, original);
        }
    }

    #[test]
    fn mark_dirty_bumps_revision_but_equality_ignores_it() {
        let mut document = SceneDocument::new_scene("main");
        let original = document.clone();
        document.mark_dirty();
        // 纯修订计数变化不算内容变化：等值比较仍相等（history 的 no-op 检测依赖此语义）。
        assert_eq!(document, original);
        // revision 递增，供编辑器检测"是否发生过编辑"。
        assert_eq!(document.revision, original.revision + 1);
        // 实际内容变化后不再相等。
        document
            .scene
            .add_entity(SceneEntityId(1), "A", None)
            .unwrap();
        assert_ne!(document, original);
    }
}
