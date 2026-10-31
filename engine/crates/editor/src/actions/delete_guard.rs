//! 安全删除：删资源前先扫**反向依赖**，被引用就停下来并把消费者报出来。
//!
//! 为什么需要它：资源删除是编辑器里唯一**不可撤销**的动作（没有 Undo，文件真没了），
//! 而引用它的地方散在三类载体里——场景/Prefab 的组件串与谱系元数据、脚本与配置里的
//! 字符串常量、工程根下的 `resources.meta` 设置行。删掉一个还在被引用的资源，症状是
//! 运行期"白块/没声音"，而不是任何一处报错。
//!
//! 三条口径：
//! 1. **判据与资源移动/重命名同源**：改指会改写的地方，这里一条都不能漏——
//!    扫描用 [`editor::collect_document_references_on_disk`]，与改写走同一套
//!    "整段取值相等"的匹配规则（见 `rustlua_editor_core::resource_ops`）。
//! 2. **扫得到才算数**：脚本文本与配置里的字符串常量也扫（引号整段匹配），
//!    音效还额外按**文件名**扫（`audio.play("hit")` 这种没有路径的写法）。
//! 3. **不猜、不代替用户决定**：命中就报出位置与总数，删不删由用户点第二次确认；
//!    没有引用就一次删掉（最常见的路径不该被多一步操作挡住）；读不动的文档不拦删除，
//!    但要在状态栏**点名**——"扫不动"不等于"没有引用"，不能悄悄当无事发生。

use crate::EditorApp;
use std::path::{Path, PathBuf};

/// 一次删除前的引用扫描结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DeleteGuard {
    /// 被删资源的工程相对路径（正斜杠）。
    pub path: String,
    /// 引用位置（人读：`assets/scenes/main.scene ▸ E#3 ▸ Sprite`）。
    pub references: Vec<String>,
    /// 读不动的文档/文件（解析失败）。它们可能藏着引用，所以要报出来而不是当"没有引用"。
    pub unreadable: Vec<String>,
    /// 扫过的文档数（场景 + Prefab）。
    pub scanned_documents: usize,
    /// 扫过的文本文件数（脚本 + 配置）。
    pub scanned_texts: usize,
}

impl DeleteGuard {
    /// 引用条数（消费者条数，不是文件数）。
    pub(crate) fn reference_count(&self) -> usize {
        self.references.len()
    }

    /// 一句人话说明"为什么这次没直接删"。
    pub(crate) fn block_reason(&self) -> String {
        let mut text = format!("{} 处引用", self.reference_count());
        if !self.unreadable.is_empty() {
            text.push_str(&format!(
                "，另有 {} 份文档读不动（可能藏着引用）",
                self.unreadable.len()
            ));
        }
        text
    }
}

/// 扫描整个工程里指向 `target` 的引用。
pub(crate) fn scan_references(root: &Path, target: &str) -> DeleteGuard {
    let mut guard = DeleteGuard {
        path: target.to_owned(),
        ..DeleteGuard::default()
    };
    // 1) 场景/Prefab 文档（与改指走同一批文件、同一套判据）。
    for path in editor::project_scene_documents(root) {
        let label = relative_text(root, &path);
        match editor::collect_document_references_on_disk(&path, target, &label) {
            Ok(hits) => {
                guard.scanned_documents += 1;
                guard
                    .references
                    .extend(hits.into_iter().map(|hit| hit.location));
            }
            Err(error) => guard.unreadable.push(format!("{label}（{error}）")),
        }
    }
    // 2) 脚本与配置：引号整段匹配（`"assets/art/a.png"`），不做子串猜测。
    let needles = text_needles(target);
    let stems = audio_stems(target);
    for path in project_text_files(root) {
        let label = relative_text(root, &path);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        guard.scanned_texts += 1;
        let mut hits = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line_number = index + 1;
            for needle in &needles {
                if line.contains(needle) {
                    hits.push(format!("{label} ▸ 第 {line_number} 行 ▸ {needle}"));
                }
            }
            for stem in &stems {
                for quote in ['"', '\''] {
                    let quoted = format!("{quote}{stem}{quote}");
                    if line.contains(&quoted) {
                        hits.push(format!("{label} ▸ 第 {line_number} 行 ▸ 音效名 \"{stem}\""));
                    }
                }
            }
        }
        if !hits.is_empty() {
            guard.references.extend(hits);
        }
    }
    guard
}

/// 工程内相对路径（正斜杠）。
fn relative_text(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// 要在文本里找的**引号整段**形态：双引号、单引号，以及 Windows 手写反斜杠。
fn text_needles(target: &str) -> Vec<String> {
    let forward = target.replace('\\', "/");
    let mut needles = vec![format!("\"{forward}\""), format!("'{forward}'")];
    if forward.contains('/') {
        needles.push(format!("\"{}\"", forward.replace('/', "\\")));
    }
    needles
}

/// 音效的**文件名**（不带扩展）：`audio.play("hit")` 这种写法没有路径，只按文件名认。
///
/// 只认 `assets/audio/` 下的顶层音效——别处的同名短串（按钮文案之类）不是引用。
fn audio_stems(target: &str) -> Vec<String> {
    let forward = target.replace('\\', "/");
    let Some(rest) = forward.strip_prefix("assets/audio/") else {
        return Vec::new();
    };
    if rest.contains('/') {
        // 带子目录的音效按文件名认会把同名兄弟也算进来，这里只认顶层音效。
        return Vec::new();
    }
    let stem = Path::new(rest)
        .file_stem()
        .map(|stem| stem.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    if stem.is_empty() {
        return Vec::new();
    }
    vec![stem]
}

/// 工程里会被文本扫描的文件（脚本 + 配置 + 资源描述 JSON）。
fn project_text_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_text_files(root, &mut files, 0);
    files.sort();
    files
}

fn collect_text_files(directory: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 16 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if path.is_dir() {
            if matches!(
                name.as_str(),
                "builds" | "target" | ".git" | ".vs" | "node_modules" | "artifacts" | ".cache"
            ) {
                continue;
            }
            collect_text_files(&path, out, depth + 1);
            continue;
        }
        let is_text = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "lua" | "toml" | "json"
                )
            });
        if is_text {
            out.push(path);
        }
    }
}

impl EditorApp {
    /// 删掉选中的资源：先扫反向依赖，被引用就停下来报出消费者。
    ///
    /// 第一次点「删除」只**扫描**（不动磁盘）；用户看到引用清单后点「强制删除」才真删。
    /// 没有引用时一次点就删掉（最常见的路径不该被多一步操作挡住）。
    pub(crate) fn delete_selected_resource(&mut self) {
        if self.is_playing() {
            self.status = "播放模式下不能删除资源，请先点击停止".to_owned();
            return;
        }
        let Some(relative_path) = self.selected_resource.clone() else {
            self.status = "请先在资源面板选中资源".to_owned();
            return;
        };
        let absolute_path = self.project.root.join(&relative_path);
        if self
            .document
            .current_scene
            .as_ref()
            .and_then(|document| document.path.as_ref())
            .is_some_and(|path| {
                crate::util::strip_verbatim_prefix(path)
                    == crate::util::strip_verbatim_prefix(&absolute_path)
            })
        {
            self.status = format!("当前打开的文档不能直接删除：{}", relative_path.display());
            return;
        }
        let target = relative_text(&self.project.root, &relative_path);
        let mut guard = scan_references(&self.project.root, &target);
        // 当前打开的文档：磁盘上那一份是旧的，用内存版（含未保存编辑）参与统计。
        self.collect_open_document_references(&target, &mut guard);
        if guard.reference_count() == 0 {
            // 没有引用 → 一次删掉（最常见的路径不该被多一步操作挡住）。
            // 读不动的文档不拦删除（那份文档本来就不参与运行），但**必须说出来**：
            // 扫描没覆盖到它，就不能谎报"确认没人引用"。
            if self.perform_delete(&relative_path) && !guard.unreadable.is_empty() {
                self.status = format!(
                    "已删除资源：{}（另有 {} 份文档读不动，没能确认其中是否引用它）",
                    relative_path.display(),
                    guard.unreadable.len()
                );
            }
            return;
        }
        let reason = guard.block_reason();
        self.pending_delete = Some(guard);
        self.status =
            format!("{target} 还有 {reason}，没删。改完引用或确认要留断链，再点一次「强制删除」。");
    }

    /// 第二次确认：仍然删除（会留下断链，由断链检查窗收尾）。
    pub(crate) fn force_delete_pending_resource(&mut self) {
        let Some(guard) = self.pending_delete.take() else {
            self.status = "没有待确认的删除".to_owned();
            return;
        };
        if self.is_playing() {
            self.status = "播放模式下不能删除资源，请先点击停止".to_owned();
            return;
        }
        let relative_path = PathBuf::from(&guard.path);
        let reference_count = guard.reference_count();
        let unreadable = guard.unreadable.len();
        if !self.perform_delete(&relative_path) {
            return;
        }
        self.status = format!(
            "已删除 {}（留下了 {} 处断链，用断链检查窗收尾）",
            guard.path, reference_count
        );
        if unreadable > 0 {
            self.status
                .push_str(&format!("；另有 {unreadable} 份文档读不动"));
        }
    }

    /// 取消待确认的删除。
    pub(crate) fn cancel_pending_delete(&mut self) {
        if self.pending_delete.take().is_some() {
            self.status = "已取消删除".to_owned();
        }
    }

    /// 当前打开文档（内存版）里指向 `target` 的引用。
    fn collect_open_document_references(&mut self, target: &str, guard: &mut DeleteGuard) {
        let Some(scene) = self.document.current_scene.as_ref() else {
            return;
        };
        guard.scanned_documents += 1;
        let label = scene
            .path
            .as_ref()
            .map(|path| relative_text(&self.project.root, path))
            .unwrap_or_else(|| "未命名场景".to_owned());
        let hits = editor::collect_scene_references(&scene.scene, target, &label);
        guard
            .references
            .extend(hits.into_iter().map(|hit| hit.location));
    }

    /// 真正动磁盘的那一步。返回是否真的删掉了。
    ///
    /// 状态栏在 `refresh_resources()` **之后**写：刷新会把状态栏改成"正在后台刷新资源…"，
    /// 先写后刷的话，"已删除资源"这条提示会被当场吃掉（用户只看到刷新在转）。
    fn perform_delete(&mut self, relative_path: &Path) -> bool {
        let absolute_path = self.project.root.join(relative_path);
        match std::fs::remove_file(&absolute_path) {
            Ok(()) => {
                self.thumbnails
                    .remove(&relative_path.to_string_lossy().replace('\\', "/"));
                self.clear_resource_selection();
                self.refresh_resources();
                self.status = format!("已删除资源：{}", relative_path.display());
                true
            }
            Err(error) => {
                self.status = format!("删除资源失败：{}：{error}", relative_path.display());
                false
            }
        }
    }
}
