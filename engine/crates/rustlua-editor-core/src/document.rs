use crate::{ProjectError, ProjectInfo};
use rustlua_assets::resource_db::AssetId;
use rustlua_scene::prefab::{Prefab, PrefabError};
use rustlua_scene::scene::{Scene, SceneEntityId, SceneError};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const PREFAB_DOCUMENT_KIND: &str = "prefab";
const DOC_KIND_META_KEY: &str = "editor.document.kind";
const PREFAB_ASSET_ID_META_KEY: &str = "editor.prefab.asset_id";

/// 编辑器中的可编辑场景及其文件状态。
#[derive(Clone, Debug)]
pub struct SceneDocument {
    pub path: Option<PathBuf>,
    pub scene: Scene,
    pub dirty: bool,
    /// 单调递增的编辑修订号：每次 `mark_dirty` 自增，供编辑器检测场景内容是否变化。
    /// 不参与相等比较（纯修订计数变化不算内容变化），避免干扰 history 的 no-op 检测。
    pub revision: u64,
    /// 加载/上次保存时该文件的签名（长度 + mtime）。用于判断**外部改动**，
    /// 不参与相等比较（与 `revision` 同理：磁盘状态不是文档内容）。
    pub disk_signature: Option<crate::file_watcher::FileSignature>,
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
            disk_signature: None,
        }
    }

    pub fn new_scene(name: impl Into<String>) -> Self {
        Self::new(name)
    }

    /// 从磁盘打开场景；只有成功解析和校验后才创建文档。
    ///
    /// **第 55 轮起不再有"打开时自动迁移"**：旧的组件名/旧写法兼容已从引擎删除
    /// （用户裁定"只保留最核心的代码"）。仓库内的示例数据已经迁到统一写法，
    /// 外部老工程需要自己先跑一次迁移（迁移工具的用法见 docs/56 §3.50 与 docs/57 §4）。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DocumentError> {
        let path = path.as_ref().to_path_buf();
        let extension = path.extension().and_then(|value| value.to_str());
        let scene = if extension == Some("prefab") {
            let prefab = Prefab::load(&path).map_err(DocumentError::Prefab)?;
            scene_from_prefab(&prefab)
        } else {
            Scene::load(&path).map_err(DocumentError::Scene)?
        };
        // 场景里的元数据键是编辑器自己的约定，打开时补齐（不是内容迁移）。
        Ok(Self {
            path: Some(path.clone()),
            scene,
            dirty: false,
            revision: 0,
            // 记下加载时的磁盘签名：之后保存前就能判断"这份文件是不是被别人改过"。
            disk_signature: crate::file_watcher::FileSignature::of(&path),
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

    pub fn is_prefab_document(&self) -> bool {
        is_prefab_document(&self.scene, self.path.as_deref())
    }

    /// 把本文档转换成**将要落盘的那份 Prefab 资产**（与 `Self::save_to` 走同一条转换）。
    ///
    /// 存在的理由：in-context 编辑（editor bin 的 `actions::prefab_stage`）返回场景时，
    /// 需要先拿到「新源」再用它重建场景里的全部实例。这一步必须与 `save` 共用
    /// `prefab_from_scene`，否则「保存并返回」与 Ctrl+S 会产出两份不同形状的资产
    /// （asset_id 变了、或 `editor.document.kind` 这类文档元数据被写进预制体里）。
    ///
    /// 未命名文档返回 [`DocumentError::MissingPath`]：`asset_id` 的回退派生取自文件路径，
    /// 没有路径就没有稳定身份，此时必须让用户先保存而不是造一个随机 id。
    pub fn prefab_asset(&self) -> Result<Prefab, DocumentError> {
        if !self.is_prefab_document() {
            return Err(DocumentError::InvalidPrefab(
                "当前文档不是预制体文档".to_owned(),
            ));
        }
        let path = self.path.clone().ok_or(DocumentError::MissingPath)?;
        prefab_from_scene(&self.scene, &path)
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

    /// 本文件在磁盘上是否**已被外部改动**（生成器、git checkout、别的编辑器…）。
    ///
    /// docs/55 §8 把"数据损坏"列为直接阻断项，而"保存时静默覆盖外部改动"正是最典型的
    /// 数据损坏路径之一：`scenes/` 与 `scripts/` 本来就是外部工具会碰的目录
    /// （`file_watcher.rs` 的模块注释里写明了这一点）。这里给出判定，
    /// 由调用方（`EditorApp::save_scene`）决定是拒绝还是强制覆盖。
    ///
    /// 未命名文档（`path` 为 `None`）恒为 `false`——没有磁盘文件就无从冲突。
    /// 记录签名缺失（例如文档是内存里造出来的）也返回 `false`：宁可保守地允许保存，
    /// 也不要在没有基准的情况下把用户挡在门外。
    pub fn disk_status(&self) -> DiskStatus {
        let Some(path) = self.path.as_deref() else {
            return DiskStatus::Untracked;
        };
        let Some(recorded) = self.disk_signature else {
            return DiskStatus::Untracked;
        };
        match crate::file_watcher::FileSignature::of(path) {
            None => DiskStatus::Removed,
            Some(current) if current == recorded => DiskStatus::Unchanged,
            Some(_) => DiskStatus::Modified,
        }
    }

    /// 记录"当前磁盘状态就是我保存/加载时的状态"。
    fn record_disk_signature(&mut self, path: &Path) {
        self.disk_signature = crate::file_watcher::FileSignature::of(path);
    }

    fn save_to(&mut self, path: &Path) -> Result<(), DocumentError> {
        self.scene.validate().map_err(DocumentError::Scene)?;
        if is_prefab_document(&self.scene, Some(path)) {
            let prefab = prefab_from_scene(&self.scene, path)?;
            prefab.save(path).map_err(DocumentError::Prefab)?;
        } else {
            atomic_write_scene(path, &self.scene)?;
        }
        self.dirty = false;
        // 落盘成功 → 磁盘状态就是当前状态（否则下一次保存会把自己的写入当成"外部改动"）。
        self.record_disk_signature(path);
        Ok(())
    }
}

/// 文档与磁盘文件的关系（`docs/55` 阶段 3 的"外部变化"数据安全半边）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskStatus {
    /// 未命名文档，或没有可比较的基准签名。
    Untracked,
    /// 磁盘内容与记录一致。
    Unchanged,
    /// 磁盘文件被外部改动过。
    Modified,
    /// 磁盘文件被外部删除。
    Removed,
}

impl DiskStatus {
    /// 是否应当**阻止**直接覆盖。
    pub const fn blocks_overwrite(self) -> bool {
        matches!(self, Self::Modified | Self::Removed)
    }

    /// 状态栏/诊断用的一句话。
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Untracked => "未跟踪磁盘状态",
            Self::Unchanged => "磁盘内容与编辑器一致",
            Self::Modified => "文件已在外部被修改",
            Self::Removed => "文件已在外部被删除",
        }
    }
}

/// Play 期间一次运行态改动的身份：(实体, 组件, 字段)。
pub type PlayEditKey = (SceneEntityId, String, String);

/// Play/Stop runtime snapshot used by the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaySession {
    editor_snapshot: SceneDocument,
    runtime_document: SceneDocument,
    /// Play 期间用户**亲手填过**的取值：键见 [`PlayEditKey`]，值是那次编辑输入的字符串。
    ///
    /// 为什么存"用户输入的值"而不是"镜像里现在这串"：游戏跑起来后同一个字段很可能已经被
    /// 脚本/物理改走（位置每帧被覆盖）。「应用到场景」要带走的是**我填的那个数**，
    /// 不是游戏此刻的读数——否则会把手忙脚乱的运行时状态写进场景文件。
    /// 这份表属于会话，Stop 随会话一起丢弃，所以"默认回滚"不需要任何撤销操作。
    play_edits: BTreeMap<PlayEditKey, String>,
}

impl PlaySession {
    pub fn start(document: &SceneDocument) -> Self {
        Self {
            editor_snapshot: document.clone(),
            runtime_document: document.clone(),
            play_edits: BTreeMap::new(),
        }
    }

    /// 本次 Play 里用户填过的取值（按 (实体, 组件, 字段) 去重，同一字段以最后一次为准）。
    pub fn play_edits(&self) -> &BTreeMap<PlayEditKey, String> {
        &self.play_edits
    }

    /// 记下一笔运行态改动，并把新组件串写进**运行态镜像**（不是编辑文档）。
    ///
    /// 返回 `false` = 镜像里没有这个实体（例如已 `entity.destroy`），调用方据此拒绝而不是
    /// 静默丢弃。这里**不** `mark_dirty`：镜像永不落盘，标了会在层级标题上挂一个存不掉的
    /// 「● 未保存」。
    pub fn apply_runtime_edit(
        &mut self,
        entity: SceneEntityId,
        component: &str,
        payload: String,
        fields: &[(String, String)],
    ) -> bool {
        let Some(instance) = self.runtime_document.scene.entities.get_mut(&entity) else {
            return false;
        };
        instance.components.insert(component.to_owned(), payload);
        for (field, value) in fields {
            self.play_edits
                .insert((entity, component.to_owned(), field.clone()), value.clone());
        }
        true
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

    /// 编辑器快照的可借副本。
    ///
    /// 只给一个调用方：Play 期间的「应用到场景」。Stop 会把 `editor_snapshot` 交还给编辑器
    /// 作为当前文档（`actions::document_gate::stop`），所以想让改动**活过 Stop**，
    /// 就必须同时写进这份快照——只写当前文档会在 Stop 那一刻被整体覆盖回去。
    pub fn editor_snapshot_mut(&mut self) -> &mut SceneDocument {
        &mut self.editor_snapshot
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

fn is_prefab_document(scene: &Scene, path: Option<&Path>) -> bool {
    scene.metadata.get(DOC_KIND_META_KEY).map(String::as_str) == Some(PREFAB_DOCUMENT_KIND)
        || path.and_then(|value| value.extension().and_then(|ext| ext.to_str())) == Some("prefab")
}

pub(crate) fn scene_from_prefab(prefab: &Prefab) -> Scene {
    let mut scene = Scene::new(prefab.name.clone());
    scene.entities = prefab.entities.clone();
    scene.metadata = prefab.metadata.clone();
    scene.metadata.insert(
        DOC_KIND_META_KEY.to_owned(),
        PREFAB_DOCUMENT_KIND.to_owned(),
    );
    scene.metadata.insert(
        PREFAB_ASSET_ID_META_KEY.to_owned(),
        prefab.asset_id.to_string(),
    );
    scene
}

fn prefab_from_scene(scene: &Scene, path: &Path) -> Result<Prefab, DocumentError> {
    let mut source = scene.clone();
    source
        .metadata
        .retain(|key, _| key != DOC_KIND_META_KEY && key != PREFAB_ASSET_ID_META_KEY);
    let roots: Vec<_> = source
        .entities
        .values()
        .filter(|entity| entity.parent.is_none())
        .map(|entity| entity.id)
        .collect();
    let [root] = roots.as_slice() else {
        return Err(DocumentError::InvalidPrefab(format!(
            "Prefab 文档必须恰好有一个根实体，当前为 {} 个",
            roots.len()
        )));
    };
    let asset_id = scene
        .metadata
        .get(PREFAB_ASSET_ID_META_KEY)
        .and_then(|value| value.parse::<AssetId>().ok())
        .unwrap_or_else(|| AssetId::from_seed(&path.to_string_lossy()));
    let mut prefab = Prefab::from_scene(&source, *root, asset_id, source.name.clone())
        .map_err(DocumentError::Prefab)?;
    prefab.metadata = source.metadata.clone();
    Ok(prefab)
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

#[derive(Debug, thiserror::Error)]
pub enum DocumentError {
    #[error("场景尚未命名，请使用另存为选择文件路径")]
    MissingPath,
    #[error("当前项目没有打开的场景")]
    NoCurrentScene,
    #[error("项目文档错误：{0}")]
    Project(#[from] ProjectError),
    #[error("场景文档错误：{0}")]
    Scene(#[source] SceneError),
    #[error("Prefab 文档错误：{0}")]
    Prefab(#[source] PrefabError),
    #[error("Prefab 文档错误：{0}")]
    InvalidPrefab(String),
    #[error("文档 I/O 失败：{}：{message}", path.display())]
    Io { path: PathBuf, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_ecs::tilemap::Tilemap;
    use rustlua_scene::prefab::Prefab;
    use rustlua_scene::scene::SceneEntityId;

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rustlua-m5-02-{label}-{}", unique_token()))
    }

    fn unique_token() -> u128 {
        // 测试专用：与 lib.rs 的令牌同源，避免并行测试临时目录互撞。
        crate::test_token()
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

    fn copy_sample_project(label: &str) -> Option<PathBuf> {
        let source = sample_project_root();
        if !source.join("project.toml").is_file() {
            return None;
        }
        let root = temp_root(&format!("sample-{label}-{}", unique_token()));
        fs::create_dir_all(&root).unwrap();
        fs::copy(source.join("project.toml"), root.join("project.toml")).unwrap();
        copy_dir(&source.join("scripts"), &root.join("scripts"));
        copy_dir(&source.join("scenes"), &root.join("scenes"));
        copy_dir(&source.join("assets"), &root.join("assets"));
        Some(root)
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

    #[test]
    fn open_and_save_prefab_document_roundtrip() {
        let root = temp_root("prefab-doc");
        let path = root.join("assets/enemy.prefab");
        let mut source = Scene::new("Enemy");
        source.add_entity(SceneEntityId(1), "Enemy", None).unwrap();
        let prefab = Prefab::from_scene(
            &source,
            SceneEntityId(1),
            AssetId::from_seed("enemy"),
            "Enemy",
        )
        .unwrap();
        prefab.save(&path).unwrap();

        let mut document = SceneDocument::open(&path).unwrap();
        assert!(document.is_prefab_document());
        document
            .scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .name = "Boss".into();
        document.mark_dirty();
        document.save().unwrap();

        let saved = Prefab::load(&path).unwrap();
        assert_eq!(saved.entities[&SceneEntityId(1)].name, "Boss");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn prefab_document_requires_single_root() {
        let root = temp_root("prefab-multi-root");
        let path = root.join("assets/enemy.prefab");
        let mut document = SceneDocument::new_scene("Enemy");
        document
            .scene
            .add_entity(SceneEntityId(1), "A", None)
            .unwrap();
        document
            .scene
            .add_entity(SceneEntityId(2), "B", None)
            .unwrap();
        document.scene.metadata.insert(
            DOC_KIND_META_KEY.to_owned(),
            PREFAB_DOCUMENT_KIND.to_owned(),
        );
        document.path = Some(path);

        let error = document.save().unwrap_err();
        assert!(error.to_string().contains("恰好有一个根实体"));
        let _ = fs::remove_dir_all(root);
    }

    /// `prefab_asset` 与 `save` 必须产出**同一份**资产：in-context 编辑靠它拿「新源」再重建
    /// 场景实例，两条路径形状不同就意味着 Ctrl+S 与「保存并返回」会写出两份不同的预制体
    /// （asset_id 被按路径重算、或 `editor.document.kind` 这类文档元数据混进资产里）。
    /// 反面对照：普通场景文档调用它必须报错，而不是凑出一份没有身份的资产。
    #[test]
    fn prefab_asset_matches_the_saved_asset_and_rejects_scene_documents() {
        let root = temp_root("prefab-asset");
        let path = root.join("assets/enemy.prefab");
        let mut source = Scene::new("Enemy");
        source.add_entity(SceneEntityId(1), "Enemy", None).unwrap();
        source
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert("Sprite".to_owned(), "texture=a.png".to_owned());
        let prefab = Prefab::from_scene(
            &source,
            SceneEntityId(1),
            AssetId::from_seed("enemy"),
            "Enemy",
        )
        .unwrap();
        prefab.save(&path).unwrap();

        let mut document = SceneDocument::open(&path).unwrap();
        document
            .scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .name = "Boss".into();
        document.mark_dirty();
        let asset = document.prefab_asset().expect("预制体文档应能导出为资产");
        assert_eq!(
            asset.asset_id, prefab.asset_id,
            "asset_id 必须沿用文件里的那一个，不能按路径重算"
        );
        assert!(
            !asset.metadata.contains_key(DOC_KIND_META_KEY),
            "文档元数据不得写进预制体资产：{:?}",
            asset.metadata
        );
        assert_eq!(asset.entities[&SceneEntityId(1)].name, "Boss");

        // 与真正落盘的那份逐字节一致（同一条转换、同一个形状）。
        document.save().unwrap();
        assert_eq!(
            Prefab::load(&path).unwrap().to_text(),
            asset.to_text(),
            "prefab_asset 与 save 必须产出同一份资产"
        );

        // 反面对照：普通场景文档没有预制体身份。
        let scene_document = SceneDocument::new_scene("Plain");
        let error = scene_document.prefab_asset().unwrap_err();
        assert!(error.to_string().contains("不是预制体文档"), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sample_project_scene_document_roundtrips_cleanly() {
        let Some(root) = copy_sample_project("scene") else {
            return;
        };
        let path = root.join("scenes/api_gameplay.scene");
        let mut document = SceneDocument::open(&path).unwrap();
        assert_eq!(document.scene.name, "api_gameplay");
        assert!(document.scene.entities.contains_key(&SceneEntityId(1)));
        assert!(document.scene.entities.contains_key(&SceneEntityId(2)));
        assert!(
            document.scene.entities.len() >= 6,
            "示例玩法场景应包含地块/玩家/箱子/商人与 HUD 文本"
        );
        // 地块是 Tilemap 组件：往返必须逐字节保住它的逐格数据。
        let tilemap = Tilemap::from_component_str(
            &document.scene.entities[&SceneEntityId(1)].components["Tilemap"],
        )
        .unwrap();
        assert_eq!(tilemap.tiles.len(), tilemap.solid.len());

        document.save().unwrap();
        let reopened = SceneDocument::open(&path).unwrap();
        assert_eq!(reopened.scene, document.scene);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sample_project_scene_edit_save_and_reopen_persists_changes() {
        let Some(root) = copy_sample_project("scene-edit") else {
            return;
        };
        let path = root.join("scenes/api_gameplay.scene");
        let mut document = SceneDocument::open(&path).unwrap();

        document
            .scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .name = "SavedPlayer".to_owned();
        document
            .scene
            .metadata
            .insert("m14.save_reopen".to_owned(), "ok".to_owned());
        document.mark_dirty();
        assert!(document.is_dirty());

        document.save().unwrap();
        assert!(!document.is_dirty());

        let reopened = SceneDocument::open(&path).unwrap();
        assert_eq!(
            reopened.scene.entities[&SceneEntityId(2)].name,
            "SavedPlayer"
        );
        assert_eq!(
            reopened
                .scene
                .metadata
                .get("m14.save_reopen")
                .map(String::as_str),
            Some("ok")
        );
        assert!(!reopened.is_dirty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sample_project_prefab_document_roundtrips_cleanly() {
        let Some(root) = copy_sample_project("prefab") else {
            return;
        };
        let path = root.join("assets/prefabs/slime_red.prefab");
        let mut document = SceneDocument::open(&path).unwrap();
        assert!(document.is_prefab_document());
        assert_eq!(document.scene.name, "SlimeRed");
        assert_eq!(document.scene.entities.len(), 1);
        assert_eq!(
            document.scene.entities[&SceneEntityId(1)].components["Sprite"],
            "texture=assets/textures/slime.png;size=32x32;pivot=0.5,0.5;tint=1.000,0.550,0.550,1.000;sorting_layer=actors;order=0"
        );

        document.save().unwrap();
        let saved = Prefab::load(&path).unwrap();
        assert_eq!(saved.name, "SlimeRed");
        assert_eq!(saved.entities.len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sample_project_play_session_discards_runtime_edits() {
        let Some(root) = copy_sample_project("play-session") else {
            return;
        };
        let path = root.join("scenes/api_gameplay.scene");
        let document = SceneDocument::open(&path).unwrap();
        let original = document.clone();
        let mut session = PlaySession::start(&document);

        session
            .runtime_document_mut()
            .scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .name = "RuntimePlayer".to_owned();
        session
            .runtime_document_mut()
            .scene
            .metadata
            .insert("runtime-only".into(), "true".into());
        session.runtime_document_mut().mark_dirty();

        assert_eq!(session.editor_snapshot(), &original);
        assert_eq!(
            session.runtime_document().scene.entities[&SceneEntityId(2)].name,
            "RuntimePlayer"
        );
        assert!(!original.scene.metadata.contains_key("runtime-only"));
        assert_eq!(session.stop(), original);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sample_project_recovery_roundtrip_preserves_path_and_keeps_main_scene_clean() {
        let Some(root) = copy_sample_project("recovery") else {
            return;
        };
        let path = root.join("scenes/api_gameplay.scene");
        let manager = crate::RecoveryManager::new(root.join(".temp"));
        let mut document = SceneDocument::open(&path).unwrap();
        let original = document.clone();

        document
            .scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .name = "RecoveredPlayer".to_owned();
        document.mark_dirty();

        manager.autosave(&document).unwrap();
        let scan = manager.scan().unwrap();
        assert_eq!(scan.candidates.len(), 1);
        assert!(scan.invalid.is_empty());

        let recovered = manager.recover(&scan.candidates[0]);
        assert_eq!(recovered.path, Some(path.clone()));
        assert!(recovered.is_dirty());
        assert_eq!(
            recovered.scene.entities[&SceneEntityId(2)].name,
            "RecoveredPlayer"
        );

        let reopened = SceneDocument::open(&path).unwrap();
        assert_eq!(
            reopened.scene.entities[&SceneEntityId(2)].name,
            original.scene.entities[&SceneEntityId(2)].name,
            "autosave must not overwrite the formal scene file"
        );
        assert_eq!(reopened, original);
        let _ = fs::remove_dir_all(root);
    }
}
