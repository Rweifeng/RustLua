use crate::SceneDocument;
use engine_core::scene::{Scene, SceneError};
use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const RECOVERY_VERSION: u32 = 1;
const EXTENSION: &str = "recovery";

/// Durable recovery-file manager for editor scene documents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryManager {
    root: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoSaveController {
    interval: Duration,
    last_autosave: Option<SystemTime>,
}

impl AutoSaveController {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_autosave: None,
        }
    }
    pub fn interval(&self) -> Duration {
        self.interval
    }
    pub fn set_interval(&mut self, interval: Duration) {
        self.interval = interval;
    }
    pub fn reset(&mut self) {
        self.last_autosave = None;
    }
    pub fn on_edit(
        &mut self,
        manager: &RecoveryManager,
        document: &SceneDocument,
        now: SystemTime,
    ) -> Result<Option<PathBuf>, RecoveryError> {
        if !document.is_dirty() {
            return Ok(None);
        }
        let path = manager.autosave(document)?;
        self.last_autosave = Some(now);
        Ok(Some(path))
    }
    pub fn tick(
        &mut self,
        manager: &RecoveryManager,
        document: &SceneDocument,
        now: SystemTime,
    ) -> Result<Option<PathBuf>, RecoveryError> {
        if !document.is_dirty()
            || self
                .last_autosave
                .is_some_and(|last| now.duration_since(last).unwrap_or_default() < self.interval)
        {
            return Ok(None);
        }
        let path = manager.autosave(document)?;
        self.last_autosave = Some(now);
        Ok(Some(path))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryCandidate {
    pub path: PathBuf,
    pub original_path: Option<PathBuf>,
    pub timestamp_ns: u128,
    pub scene: Scene,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecoveryScan {
    pub candidates: Vec<RecoveryCandidate>,
    pub invalid: Vec<RecoveryIssue>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryIssue {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug)]
pub enum RecoveryError {
    Io { path: PathBuf, message: String },
    Scene(SceneError),
    Invalid { path: PathBuf, message: String },
}

impl fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => {
                write!(f, "recovery I/O failed: {}: {message}", path.display())
            }
            Self::Scene(error) => write!(f, "recovery scene is invalid: {error}"),
            Self::Invalid { path, message } => {
                write!(f, "invalid recovery file {}: {message}", path.display())
            }
        }
    }
}
impl Error for RecoveryError {}

impl RecoveryManager {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn save_document(&self, document: &mut SceneDocument) -> Result<(), RecoveryError> {
        let previous = document.clone();
        let path = document.path.clone().unwrap_or_default();
        document.save().map_err(|error| RecoveryError::Invalid {
            path,
            message: error.to_string(),
        })?;
        let _ = self.clear_for(&previous)?;
        let _ = self.clear_for(document)?;
        Ok(())
    }

    /// Writes a validated snapshot. The formal scene file is never touched.
    pub fn autosave(&self, document: &SceneDocument) -> Result<PathBuf, RecoveryError> {
        document.scene.validate().map_err(RecoveryError::Scene)?;
        let timestamp_ns = now_ns();
        let scene_text = document.scene.to_text();
        let key = document
            .path
            .as_deref()
            .map(path_key)
            .unwrap_or_else(|| format!("unsaved:{}", document.scene.name));
        let path = self.root.join(format!(
            "recovery-{:016x}.{EXTENSION}",
            fnv1a(key.as_bytes())
        ));
        let original = document
            .path
            .as_deref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let content = format!(
            "recovery_version={RECOVERY_VERSION}\ntimestamp_ns={timestamp_ns}\noriginal_path_hex={}\nchecksum={:016x}\npayload_hex={}\n",
            hex_encode(original.as_bytes()),
            fnv1a(scene_text.as_bytes()),
            hex_encode(scene_text.as_bytes())
        );
        atomic_write(&path, content.as_bytes())?;
        Ok(path)
    }

    pub fn scan(&self) -> Result<RecoveryScan, RecoveryError> {
        let mut scan = RecoveryScan::default();
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(scan),
            Err(error) => return Err(io_error(&self.root, error)),
        };
        for entry in entries {
            let entry = entry.map_err(|error| io_error(&self.root, error))?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some(EXTENSION) {
                continue;
            }
            match parse_candidate(&path) {
                Ok(candidate) => scan.candidates.push(candidate),
                Err(error) => scan.invalid.push(RecoveryIssue {
                    path,
                    message: error.to_string(),
                }),
            }
        }
        scan.candidates
            .sort_by_key(|candidate| std::cmp::Reverse(candidate.timestamp_ns));
        Ok(scan)
    }

    pub fn recover(&self, candidate: &RecoveryCandidate) -> SceneDocument {
        SceneDocument {
            path: candidate.original_path.clone(),
            scene: candidate.scene.clone(),
            dirty: true,
            revision: 0,
        }
    }

    pub fn discard(&self, candidate: &RecoveryCandidate) -> Result<(), RecoveryError> {
        fs::remove_file(&candidate.path).map_err(|error| io_error(&candidate.path, error))
    }

    /// Removes the recovery snapshot belonging to a successfully saved document.
    pub fn clear_for(&self, document: &SceneDocument) -> Result<bool, RecoveryError> {
        let key = document
            .path
            .as_deref()
            .map(path_key)
            .unwrap_or_else(|| format!("unsaved:{}", document.scene.name));
        let path = self.root.join(format!(
            "recovery-{:016x}.{EXTENSION}",
            fnv1a(key.as_bytes())
        ));
        match fs::remove_file(path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(io_error(&self.root, error)),
        }
    }

    pub fn clear_all(&self) -> Result<usize, RecoveryError> {
        let mut count = 0;
        for candidate in self.scan()?.candidates {
            self.discard(&candidate)?;
            count += 1;
        }
        Ok(count)
    }
}

fn parse_candidate(path: &Path) -> Result<RecoveryCandidate, RecoveryError> {
    let text = fs::read_to_string(path).map_err(|error| io_error(path, error))?;
    let mut version = None;
    let mut timestamp = None;
    let mut original = None;
    let mut checksum = None;
    let mut payload = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("recovery_version=") {
            version = value.parse().ok();
        } else if let Some(value) = line.strip_prefix("timestamp_ns=") {
            timestamp = value.parse().ok();
        } else if let Some(value) = line.strip_prefix("original_path_hex=") {
            original = Some(hex_decode(value).map_err(|message| invalid(path, message))?);
        } else if let Some(value) = line.strip_prefix("checksum=") {
            checksum = u64::from_str_radix(value, 16).ok();
        } else if let Some(value) = line.strip_prefix("payload_hex=") {
            payload = Some(hex_decode(value).map_err(|message| invalid(path, message))?);
        }
    }
    if version != Some(RECOVERY_VERSION) {
        return Err(invalid(path, "unsupported recovery version"));
    }
    let timestamp_ns = timestamp.ok_or_else(|| invalid(path, "missing timestamp"))?;
    let original = original.ok_or_else(|| invalid(path, "missing original path"))?;
    let payload = payload.ok_or_else(|| invalid(path, "missing payload"))?;
    let checksum = checksum.ok_or_else(|| invalid(path, "missing checksum"))?;
    if fnv1a(&payload) != checksum {
        return Err(invalid(path, "checksum mismatch"));
    }
    let scene_text =
        String::from_utf8(payload).map_err(|_| invalid(path, "payload is not UTF-8"))?;
    let scene = Scene::from_text(&scene_text).map_err(RecoveryError::Scene)?;
    let original_path = if original.is_empty() {
        None
    } else {
        Some(PathBuf::from(
            String::from_utf8(original).map_err(|_| invalid(path, "original path is not UTF-8"))?,
        ))
    };
    Ok(RecoveryCandidate {
        path: path.to_path_buf(),
        original_path,
        timestamp_ns,
        scene,
    })
}

fn invalid(path: &Path, message: impl Into<String>) -> RecoveryError {
    RecoveryError::Invalid {
        path: path.to_path_buf(),
        message: message.into(),
    }
}
fn io_error(path: &Path, error: std::io::Error) -> RecoveryError {
    RecoveryError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}
fn path_key(path: &Path) -> String {
    path.to_string_lossy().to_string()
}
fn now_ns() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0)
}
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn hex_decode(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) {
        return Err("hex payload has odd length".into());
    }
    (0..text.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&text[index..index + 2], 16)
                .map_err(|_| "invalid hex payload".to_owned())
        })
        .collect()
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), RecoveryError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("recovery"),
        now_ns()
    ));
    fs::write(&temp, bytes).map_err(|error| io_error(&temp, error))?;
    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(io_error(path, error));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::scene::SceneEntityId;

    fn root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("m5-09-{label}-{}", now_ns()))
    }
    fn document(path: Option<PathBuf>) -> SceneDocument {
        let mut document = SceneDocument::new("main");
        document.path = path;
        document
            .scene
            .add_entity(SceneEntityId(1), "Player", None)
            .unwrap();
        document
    }

    #[test]
    fn autosave_scan_and_recover_preserves_original_path() {
        let root = root("roundtrip");
        let original = root.join("main.scene");
        let manager = RecoveryManager::new(root.join("recovery"));
        let document = document(Some(original.clone()));
        manager.autosave(&document).unwrap();
        let scan = manager.scan().unwrap();
        assert_eq!(scan.candidates.len(), 1);
        assert!(scan.invalid.is_empty());
        let recovered = manager.recover(&scan.candidates[0]);
        assert_eq!(recovered.path, Some(original));
        assert!(recovered.is_dirty());
        assert_eq!(recovered.scene, document.scene);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn checksum_or_scene_corruption_is_reported_without_touching_main_file() {
        let root = root("corrupt");
        let main = root.join("main.scene");
        fs::create_dir_all(&root).unwrap();
        fs::write(&main, "main-file").unwrap();
        let manager = RecoveryManager::new(root.join("recovery"));
        let recovery = manager.autosave(&document(Some(main.clone()))).unwrap();
        let mut bytes = fs::read(&recovery).unwrap();
        let index = bytes.iter().position(|byte| *byte == b'0').unwrap();
        bytes[index] = b'f';
        fs::write(&recovery, bytes).unwrap();
        let scan = manager.scan().unwrap();
        assert!(scan.candidates.is_empty());
        assert_eq!(fs::read_to_string(main).unwrap(), "main-file");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn version_mismatch_is_rejected_and_clear_for_removes_valid_snapshot() {
        let root = root("cleanup");
        let manager = RecoveryManager::new(root.join("recovery"));
        let mut document = document(None);
        let recovery = manager.autosave(&document).unwrap();
        let mut text = fs::read_to_string(&recovery).unwrap();
        text = text.replace("recovery_version=1", "recovery_version=99");
        fs::write(&recovery, text).unwrap();
        assert_eq!(manager.scan().unwrap().invalid.len(), 1);
        fs::remove_file(&recovery).unwrap();
        manager.autosave(&document).unwrap();
        document.mark_clean();
        assert!(manager.clear_for(&document).unwrap());
        assert!(manager.scan().unwrap().candidates.is_empty());
        let _ = fs::remove_dir_all(root);
    }
}
