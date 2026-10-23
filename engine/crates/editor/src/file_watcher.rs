//! Polling file watcher for author-time changes.
//!
//! docs/55 §3.2 requires "外部文件变化反映到 Project：2 秒内最终一致", and stage 3 asks for a
//! FileWatcher feeding an incremental asset graph. This module provides the detection half:
//! a bounded metadata walk of the project's asset/script/scene/shader directories, diffed
//! against the previous snapshot.
//!
//! Design notes:
//! * **No new dependencies** — the workspace stays offline/self-contained, so this is a polling
//!   watcher rather than an OS-notification binding. Polling also makes the "2 s eventual
//!   consistency" budget directly measurable.
//! * **Cheap by construction**: one `metadata()` per candidate file, no file contents read, no
//!   recursive allocation per entry beyond the path. `scan_cost` is measured by the baseline
//!   suite so the cost stays visible.
//! * **Diff-based, so bursts coalesce**: a snapshot comparison naturally turns a burst of writes
//!   into one event, which is what keeps a "save all" from triggering N refreshes.
//! * **Cooperative cancellation**: a cancelled watcher stops mid-walk and reports
//!   [`WatchError::Cancelled`] instead of returning a half-scanned snapshot.

use crate::CancellationToken;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

/// Lightweight identity of one file: size + mtime. Contents are never read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileSignature {
    pub len: u64,
    pub modified: Option<SystemTime>,
}

impl FileSignature {
    /// 单个文件的签名；文件不存在/无法读取时返回 `None`。
    ///
    /// 与 `scan_tree` 用同一套判据（长度 + mtime，**不读内容**），
    /// 因此"保存前检查外部改动"的成本就是一次 `metadata()`（docs/55 阶段 3 的数据安全半边）。
    #[must_use]
    pub fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        Some(Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

/// Files whose signature changed since the previous snapshot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WatchDiff {
    pub added: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
}

impl WatchDiff {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.modified.is_empty() && self.removed.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.added.len() + self.modified.len() + self.removed.len()
    }

    /// 全部变化路径（新增 + 修改 + 删除），按 added/modified/removed 顺序。
    pub fn paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.added.iter().chain(&self.modified).chain(&self.removed)
    }

    /// 供状态栏/日志用的一行摘要。
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "新增 {} / 修改 {} / 删除 {}",
            self.added.len(),
            self.modified.len(),
            self.removed.len()
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchError {
    Cancelled,
}

/// Which project subtrees are watched, and how large a scan may get.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchScope {
    /// Project-relative directory names to walk.
    pub roots: Vec<String>,
    /// Hard cap on scanned entries; protects against pathological trees.
    pub max_entries: usize,
}

impl Default for WatchScope {
    fn default() -> Self {
        Self {
            roots: vec![
                "assets".to_owned(),
                "scripts".to_owned(),
                "scenes".to_owned(),
                "data".to_owned(),
                // P3-R05-4：`shaders/`（内置 shader 的开发期覆盖目录）与资源同权监听。
                // 没有这个根，改 `shaders/mesh3d.wgsl` 就永远不会产生事件，热重载只能靠重启编辑器。
                engine::host::shader_library::SHADER_SOURCE_DIR.to_owned(),
            ],
            max_entries: 200_000,
        }
    }
}

/// Snapshot of one project subtree: relative path → signature.
pub type Snapshot = BTreeMap<PathBuf, FileSignature>;

/// Walk `scope.roots` under `project_root` and collect signatures.
pub fn scan_tree(
    project_root: &Path,
    scope: &WatchScope,
    cancel: &CancellationToken,
) -> Result<Snapshot, WatchError> {
    let mut out = Snapshot::new();
    let mut stack: Vec<PathBuf> = scope
        .roots
        .iter()
        .map(|root| project_root.join(root))
        .collect();
    while let Some(directory) = stack.pop() {
        if cancel.is_cancelled() {
            return Err(WatchError::Cancelled);
        }
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                // 隐藏目录与构建产物不进快照（与资源扫描/审计保持一致）。
                if name.starts_with('.') || name == "target" || name == "node_modules" {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            if out.len() >= scope.max_entries {
                return Ok(out);
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let relative = path
                .strip_prefix(project_root)
                .map_or_else(|_| path.clone(), PathBuf::from);
            out.insert(
                relative,
                FileSignature {
                    len: metadata.len(),
                    modified: metadata.modified().ok(),
                },
            );
        }
    }
    Ok(out)
}

/// Polling watcher over one project root.
#[derive(Debug)]
pub struct FileWatcher {
    project_root: PathBuf,
    scope: WatchScope,
    interval: Duration,
    snapshot: Snapshot,
    primed: bool,
    cancel: CancellationToken,
    last_poll: Option<Instant>,
    polls: u64,
    events: u64,
    changed_files: u64,
    last_scan_cost: Duration,
}

impl FileWatcher {
    #[must_use]
    pub fn new(project_root: impl Into<PathBuf>, scope: WatchScope, interval: Duration) -> Self {
        Self {
            project_root: project_root.into(),
            scope,
            interval: interval.max(Duration::from_millis(50)),
            snapshot: Snapshot::new(),
            primed: false,
            cancel: CancellationToken::new(),
            last_poll: None,
            polls: 0,
            events: 0,
            changed_files: 0,
            last_scan_cost: Duration::ZERO,
        }
    }

    #[must_use]
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    #[must_use]
    pub fn interval(&self) -> Duration {
        self.interval
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// 建立基线快照（不产生事件）。
    pub fn prime(&mut self) -> Result<usize, WatchError> {
        let started = Instant::now();
        let snapshot = scan_tree(&self.project_root, &self.scope, &self.cancel)?;
        self.last_scan_cost = started.elapsed();
        let len = snapshot.len();
        self.snapshot = snapshot;
        self.primed = true;
        Ok(len)
    }

    #[must_use]
    pub fn snapshot_len(&self) -> usize {
        self.snapshot.len()
    }

    /// `(轮询次数, 事件数, 变化文件数, 最近一次扫描耗时)`
    #[must_use]
    pub fn stats(&self) -> (u64, u64, u64, Duration) {
        (
            self.polls,
            self.events,
            self.changed_files,
            self.last_scan_cost,
        )
    }

    /// 最近一次扫描耗时（只读探测，供后台线程上报统计）。
    #[must_use]
    pub fn last_scan_cost(&self) -> Duration {
        self.last_scan_cost
    }

    /// 扫描一次并与快照做 diff（立即执行，不看间隔）。
    pub fn poll_now(&mut self) -> Result<WatchDiff, WatchError> {
        let started = Instant::now();
        let current = scan_tree(&self.project_root, &self.scope, &self.cancel)?;
        self.last_scan_cost = started.elapsed();
        self.polls += 1;
        if !self.primed {
            // 未建立基线时把首次扫描当作基线，避免把"项目初次打开"报成上万条新增。
            self.snapshot = current;
            self.primed = true;
            return Ok(WatchDiff::default());
        }
        let diff = diff_snapshots(&self.snapshot, &current);
        self.snapshot = current;
        if !diff.is_empty() {
            self.events += 1;
            self.changed_files += diff.len() as u64;
        }
        Ok(diff)
    }

    /// 到达轮询间隔才扫描；未到间隔返回 `None`（UI 每帧调用它，因此开销≈一次比较）。
    pub fn poll_if_due(&mut self, now: Instant) -> Result<Option<WatchDiff>, WatchError> {
        let due = self
            .last_poll
            .is_none_or(|last| now.saturating_duration_since(last) >= self.interval);
        if !due {
            return Ok(None);
        }
        self.last_poll = Some(now);
        self.poll_now().map(Some)
    }

    /// 阻塞轮询直到出现变化或超时（后台任务/测试用）。
    pub fn wait_for_change(&mut self, timeout: Duration) -> Result<Option<WatchDiff>, WatchError> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.cancel.is_cancelled() {
                return Err(WatchError::Cancelled);
            }
            let diff = self.poll_now()?;
            if !diff.is_empty() {
                return Ok(Some(diff));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(self.interval);
        }
    }
}

/// 后台监听线程的统计（原子计数，供 profiler 读取）。
#[derive(Debug, Default)]
pub struct WatchStats {
    polls: std::sync::atomic::AtomicU64,
    events: std::sync::atomic::AtomicU64,
    changed_files: std::sync::atomic::AtomicU64,
    last_scan_micros: std::sync::atomic::AtomicU64,
    entries: std::sync::atomic::AtomicU64,
}

impl WatchStats {
    #[must_use]
    pub fn polls(&self) -> u64 {
        self.polls.load(std::sync::atomic::Ordering::Relaxed)
    }
    #[must_use]
    pub fn events(&self) -> u64 {
        self.events.load(std::sync::atomic::Ordering::Relaxed)
    }
    #[must_use]
    pub fn changed_files(&self) -> u64 {
        self.changed_files
            .load(std::sync::atomic::Ordering::Relaxed)
    }
    #[must_use]
    pub fn last_scan_cost(&self) -> Duration {
        Duration::from_micros(
            self.last_scan_micros
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }
    #[must_use]
    pub fn entries(&self) -> u64 {
        self.entries.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// 后台文件监听句柄：worker 线程负责扫描/diff，UI 线程每帧只做一次 `try_recv`。
///
/// docs/55 §4.1 规则 5（后台任务只返回不可变结果，主线程在明确提交点合并）：
/// 2k 文件的一次元数据扫描在 Debug 下约 10 ms，放在 UI 线程会吃掉半帧预算，
/// 因此扫描在 worker 上完成，主线程只检查通道。
#[derive(Debug)]
pub struct WatchHandle {
    receiver: std::sync::mpsc::Receiver<WatchDiff>,
    cancel: CancellationToken,
    stats: std::sync::Arc<WatchStats>,
    interval: Duration,
    project_root: PathBuf,
}

impl WatchHandle {
    /// 取走一条待处理的变化（无变化时返回 `None`，不阻塞）。
    #[must_use]
    pub fn try_take(&self) -> Option<WatchDiff> {
        self.receiver.try_recv().ok()
    }

    /// 监听的项目根（用于检测项目切换后是否需要重建监听）。
    #[must_use]
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    #[must_use]
    pub fn stats(&self) -> &std::sync::Arc<WatchStats> {
        &self.stats
    }

    #[must_use]
    pub fn interval(&self) -> Duration {
        self.interval
    }
}

/// 启动后台监听线程；句柄被 drop 或 `cancel()` 后线程退出。
pub fn spawn_watcher(
    project_root: impl Into<PathBuf>,
    scope: WatchScope,
    interval: Duration,
) -> WatchHandle {
    let root = project_root.into();
    let root_for_handle = root.clone();
    let cancel = CancellationToken::new();
    let worker_cancel = cancel.clone();
    let stats = std::sync::Arc::new(WatchStats::default());
    let worker_stats = stats.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    let poll_interval = interval.max(Duration::from_millis(50));
    std::thread::spawn(move || {
        let mut watcher = FileWatcher::new(root, scope, poll_interval);
        if watcher.prime().is_err() {
            return;
        }
        worker_stats.entries.store(
            watcher.snapshot_len() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        while !worker_cancel.is_cancelled() {
            std::thread::sleep(poll_interval);
            match watcher.poll_now() {
                Ok(diff) => {
                    let (polls, events, changed, scan_cost) = watcher.stats();
                    worker_stats
                        .polls
                        .store(polls, std::sync::atomic::Ordering::Relaxed);
                    worker_stats
                        .events
                        .store(events, std::sync::atomic::Ordering::Relaxed);
                    worker_stats
                        .changed_files
                        .store(changed, std::sync::atomic::Ordering::Relaxed);
                    worker_stats.last_scan_micros.store(
                        scan_cost.as_micros() as u64,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    if !diff.is_empty() && sender.send(diff).is_err() {
                        return; // 接收端已 drop（编辑器关闭/切换项目）。
                    }
                }
                Err(WatchError::Cancelled) => return,
            }
        }
    });
    WatchHandle {
        receiver,
        cancel,
        stats,
        interval: poll_interval,
        project_root: root_for_handle,
    }
}

/// Pure diff: what changed between two snapshots.
#[must_use]
pub fn diff_snapshots(previous: &Snapshot, current: &Snapshot) -> WatchDiff {
    let mut diff = WatchDiff::default();
    for (path, signature) in current {
        match previous.get(path) {
            None => diff.added.push(path.clone()),
            Some(previous_signature) if previous_signature != signature => {
                diff.modified.push(path.clone());
            }
            Some(_) => {}
        }
    }
    for path in previous.keys() {
        if !current.contains_key(path) {
            diff.removed.push(path.clone());
        }
    }
    diff
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_root(label: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "rustlua-watch-{label}-{}-{counter}-{nanos}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("assets")).expect("assets dir");
        fs::create_dir_all(root.join("scenes")).expect("scenes dir");
        root
    }

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent dir");
        }
        fs::write(path, bytes).expect("write file");
    }

    fn watcher(root: &Path) -> FileWatcher {
        let mut watcher = FileWatcher::new(root, WatchScope::default(), Duration::from_millis(50));
        watcher.prime().expect("prime");
        watcher
    }

    #[test]
    fn detects_added_modified_and_removed_files() {
        let root = temp_root("diff");
        write(&root.join("assets/keep.png"), b"a");
        write(&root.join("assets/edit.png"), b"bb");
        write(&root.join("assets/gone.png"), b"ccc");
        let mut watcher = watcher(&root);
        assert_eq!(watcher.snapshot_len(), 3);

        // 无变化：空 diff（幂等轮询）。
        let quiet = watcher.poll_now().expect("poll");
        assert!(quiet.is_empty(), "{quiet:?}");

        write(&root.join("assets/new.png"), b"d");
        write(&root.join("assets/edit.png"), b"bbbb");
        fs::remove_file(root.join("assets/gone.png")).expect("remove");

        let diff = watcher.poll_now().expect("poll");
        assert_eq!(diff.added, vec![PathBuf::from("assets/new.png")]);
        assert_eq!(diff.modified, vec![PathBuf::from("assets/edit.png")]);
        assert_eq!(diff.removed, vec![PathBuf::from("assets/gone.png")]);
        assert_eq!(diff.len(), 3);
        assert!(diff.summary().contains("修改 1"));

        // 再次轮询：稳定下来后不再重复报告。
        assert!(watcher.poll_now().expect("poll").is_empty());
        let (polls, events, files, _) = watcher.stats();
        assert_eq!(polls, 3);
        assert_eq!(events, 1, "只有一次真实变化事件");
        assert_eq!(files, 3);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn modification_with_same_length_is_detected() {
        // 长度相同、仅内容/mtime 变化也必须被发现（保存覆盖写是常见情形）。
        let root = temp_root("same-len");
        let path = root.join("scripts/main.lua");
        write(&path, b"return 1\n");
        let mut watcher = watcher(&root);
        std::thread::sleep(Duration::from_millis(20));
        write(&path, b"return 2\n");
        let diff = watcher.poll_now().expect("poll");
        assert_eq!(diff.modified, vec![PathBuf::from("scripts/main.lua")]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ignores_hidden_and_build_directories() {
        let root = temp_root("ignored");
        write(&root.join("assets/kept.png"), b"a");
        write(&root.join("assets/.cache/tmp.png"), b"b");
        write(&root.join("assets/target/out.png"), b"c");
        write(&root.join(".hidden/secret.png"), b"d");
        let watcher = watcher(&root);
        assert_eq!(watcher.snapshot_len(), 1, "只应看到 assets/kept.png");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn first_poll_without_prime_is_treated_as_baseline() {
        let root = temp_root("baseline");
        write(&root.join("assets/a.png"), b"a");
        let mut watcher = FileWatcher::new(&root, WatchScope::default(), Duration::from_millis(50));
        let diff = watcher.poll_now().expect("poll");
        assert!(diff.is_empty(), "首次扫描不应报成上万条新增");
        assert_eq!(watcher.snapshot_len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn change_is_detected_within_the_two_second_budget() {
        // docs/55 §3.2：外部变化 2 秒内最终一致。这里测的是**检测**延迟。
        let root = temp_root("latency");
        write(&root.join("assets/start.png"), b"a");
        let mut watcher = watcher(&root);
        let started = Instant::now();
        std::thread::spawn({
            let path = root.join("assets/later.png");
            move || {
                std::thread::sleep(Duration::from_millis(150));
                write(&path, b"hello");
            }
        });
        let diff = watcher
            .wait_for_change(Duration::from_secs(3))
            .expect("no cancellation")
            .expect("必须在超时前检测到变化");
        let latency = started.elapsed();
        assert_eq!(diff.added, vec![PathBuf::from("assets/later.png")]);
        assert!(
            latency < Duration::from_secs(2),
            "检测延迟必须在 2 秒预算内：{latency:?}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn poll_if_due_respects_the_interval() {
        let root = temp_root("interval");
        write(&root.join("assets/a.png"), b"a");
        let mut watcher =
            FileWatcher::new(&root, WatchScope::default(), Duration::from_millis(200));
        watcher.prime().expect("prime");
        let now = Instant::now();
        // 第一次调用即到期。
        assert!(watcher.poll_if_due(now).expect("poll").is_some());
        // 间隔内不再扫描。
        assert!(
            watcher
                .poll_if_due(now + Duration::from_millis(50))
                .expect("poll")
                .is_none()
        );
        assert!(
            watcher
                .poll_if_due(now + Duration::from_millis(250))
                .expect("poll")
                .is_some()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cancellation_stops_scanning() {
        let root = temp_root("cancel");
        for index in 0..20 {
            write(&root.join(format!("assets/f{index}.png")), b"x");
        }
        let mut watcher = watcher(&root);
        watcher.cancel();
        assert!(watcher.is_cancelled());
        assert_eq!(watcher.poll_now(), Err(WatchError::Cancelled));
        assert_eq!(
            watcher.wait_for_change(Duration::from_millis(10)),
            Err(WatchError::Cancelled)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scan_respects_the_entry_cap() {
        let root = temp_root("cap");
        for index in 0..10 {
            write(&root.join(format!("assets/f{index}.png")), b"x");
        }
        let scope = WatchScope {
            roots: vec!["assets".to_owned()],
            max_entries: 4,
        };
        let snapshot = scan_tree(&root, &scope, &CancellationToken::new()).expect("scan");
        assert_eq!(snapshot.len(), 4);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_roots_are_tolerated() {
        let root = temp_root("missing");
        let scope = WatchScope {
            roots: vec!["assets".to_owned(), "does-not-exist".to_owned()],
            max_entries: 100,
        };
        let snapshot = scan_tree(&root, &scope, &CancellationToken::new()).expect("scan");
        assert!(snapshot.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    /// P3-R05-4：默认监听根必须包含 `shaders/`——少了这个根，改覆盖文件不会产生任何事件，
    /// "热重载"就只能靠重启编辑器，而那是这条验收门要排除掉的行为。
    #[test]
    fn default_scope_watches_the_shader_override_directory() {
        let scope = WatchScope::default();
        let shaders = engine::host::shader_library::SHADER_SOURCE_DIR;
        assert!(
            scope.roots.iter().any(|root| root == shaders),
            "默认监听根缺少 {shaders}：{:?}",
            scope.roots
        );
        let root = temp_root("shaders");
        fs::create_dir_all(root.join(shaders)).expect("shaders dir");
        fs::write(root.join(shaders).join("mesh3d.wgsl"), "// override\n").expect("write");
        let snapshot = scan_tree(&root, &scope, &CancellationToken::new()).expect("scan");
        assert!(
            snapshot.contains_key(Path::new("shaders/mesh3d.wgsl")),
            "覆盖目录里的文件要进快照：{:?}",
            snapshot.keys().collect::<Vec<_>>()
        );
        let _ = fs::remove_dir_all(root);
    }
}
