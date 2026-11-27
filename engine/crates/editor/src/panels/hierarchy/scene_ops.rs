//! 场景切换/新建与 Prefab 创建入口（M79 结构切分，零行为变化）。
use super::*;

impl EditorApp {
    /// 从场景元数据同步 hidden/locked 到编辑态集合。
    /// 仅当场景标识（path, revision）变化时执行，避免每帧全量遍历实体。
    pub(crate) fn sync_hierarchy_metadata(&mut self) {
        let Some(scene) = self.document.current_scene.as_ref() else {
            self.last_metadata_key = None;
            return;
        };
        let key = (scene.path.clone(), scene.revision);
        if self.last_metadata_key.as_ref() == Some(&key) {
            return;
        }
        self.last_metadata_key = Some(key);
        for id in scene.scene.entities.keys().copied().collect::<Vec<_>>() {
            let hidden = scene
                .scene
                .metadata
                .get(&format!("editor.hidden.{}", id.0))
                .is_some_and(|v| v == "true");
            let locked = scene
                .scene
                .metadata
                .get(&format!("editor.locked.{}", id.0))
                .is_some_and(|v| v == "true");
            if hidden {
                self.scene_state.hidden.insert(id);
            } else {
                self.scene_state.hidden.remove(&id);
            }
            if locked {
                self.scene_state.locked.insert(id);
            } else {
                self.scene_state.locked.remove(&id);
            }
        }
    }

    // `persist_hierarchy_flag` **已搬到 shell**（`crate::hierarchy_ops`，第 28 轮）：
    // 隐藏/锁定标记是场景 metadata，属于文档写入。

    /// 场景切换下拉框的选中文本：场景名 + 未保存标记。
    pub(super) fn scene_selector_label(&self) -> String {
        let name = self.scene_name();
        if self.current_scene().is_some_and(|document| document.dirty) {
            format!("{name} ●未保存")
        } else {
            name
        }
    }

    /// 项目 `scenes/` 目录下的全部场景文件（相对路径，按名称排序），用于场景切换下拉。
    /// 至少 2 秒缓存一次，避免每帧 read_dir 扫 scenes 目录。
    pub(super) fn scene_files(&mut self) -> Vec<PathBuf> {
        let root = strip_verbatim_prefix(&self.project.root);
        let now = Instant::now();
        if self
            .scene_files_cache_time
            .is_some_and(|last| now.duration_since(last) < Duration::from_secs(2))
        {
            return self.scene_files_cache.clone();
        }
        self.scene_files_cache_time = Some(now);
        self.scene_files_cache = scan_scene_files(&root);
        self.scene_files_cache.clone()
    }

    /// 新建场景（未保存；保存时写入 scenes/<名称>.scene）。
    ///
    /// E06：先做项目已加载 / Play 锁 / 名称校验（这些失败不弹确认），随后经
    /// 「未保存切换保护」网关请求——dirty 时弹确认窗，clean 时直接 `perform_create_scene`。
    /// 新建场景（菜单「文件 ▸ 新建场景」）。
    ///
    /// 菜单里**没有场景名输入框**（与 Unity 的 File ▸ New Scene 一致）：名字由这里决定，
    /// 并在冲突时自动去重（见 [`Self::next_new_scene_name`]），因此不会默默覆盖已有场景文件。
    pub(crate) fn create_scene(&mut self) {
        let name = self.next_new_scene_name();
        let _ = self.request_create_scene(name);
    }

    /// 新建场景的**唯一请求入口**：前置校验 + 未保存切换闸门，返回命令结果。
    ///
    /// 菜单、快捷键、typed `EditorCommand::NewScene` 与 AI 的 `new_scene` 工具都走这里，
    /// 保证任何入口都不会绕过「有未保存改动先确认」的保护、直接替换当前文档。
    pub(crate) fn request_create_scene(&mut self, name: String) -> editor::CommandResult {
        if !self.project_loaded {
            self.status = "请先导入游戏工程再编辑".to_owned();
            return editor::CommandResult::Rejected(self.status.clone());
        }
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
            return editor::CommandResult::Rejected(self.status.clone());
        }
        self.request_document_change(crate::actions::PendingChange::CreateScene { name });
        self.command_result_from_status("新建场景被挂起或失败")
    }

    /// 新建场景的默认名：`main`；若 `scenes/main.scene` 已存在（或当前文档已叫这个名字）
    /// 则依次尝试 `main2`、`main3`…，避免"新建两次都在写同一个文件"。
    pub(crate) fn next_new_scene_name(&self) -> String {
        let scenes = strip_verbatim_prefix(&self.project.root).join("scenes");
        let current = self
            .current_scene()
            .map(|document| document.scene.name.clone());
        let taken = |candidate: &str| {
            current.as_deref() == Some(candidate)
                || scenes.join(format!("{candidate}.scene")).exists()
        };
        if !taken("main") {
            return "main".to_owned();
        }
        for index in 2..1000 {
            let candidate = format!("main{index}");
            if !taken(&candidate) {
                return candidate;
            }
        }
        // 极端情况下（1000 个同名场景）退回时间戳，保证一定拿到一个可用名字。
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or_default();
        format!("main{stamp}")
    }

    // `perform_create_scene` / `add_3d_entity` **已搬到 shell**
    // （`crate::hierarchy_ops`，第 28 轮）：它们都会替换/写入当前文档。

    /// 把选中实体保存为预制体（assets/prefabs/<name>.prefab）。
    pub(crate) fn create_prefab_from_selected(&mut self) {
        if self.is_playing() {
            self.status = "播放模式下不能编辑场景，请先点击停止".to_owned();
            return;
        }
        let Some(selected) = self.selection.primary() else {
            self.status = "请先在层级面板中选中实体".to_owned();
            return;
        };
        let Some(scene) = self.current_scene() else {
            return;
        };
        // **嵌套 Prefab 支持**（docs/55 P1）：从"已经是 Prefab 实例"的子树创建预制体 会写出一份
        // 嵌套源——内层节点在文件里带着自己的 `prefab.source` / `prefab.source_id` /
        // `prefab.root` 与一份取值快照，实例化与写回都按这套标记对号（见
        // `actions::prefab_rebuild`）。要挡的只剩两种真坏数据：**自指/环**（新预制体的路径
        // 出现在它自己引用的闭包里）与**超深链**。
        let name = scene
            .scene
            .entities
            .get(&selected)
            .map(|entity| entity.name.clone())
            .unwrap_or_else(|| "Prefab".to_owned());
        let root = strip_verbatim_prefix(&self.project.root);
        let dir = root.join("assets").join("prefabs");
        // **不静默覆盖**：同名预制体已存在时按 Unity 的语义取 `Name_1`、`Name_2`…，
        // 而不是把别人的预制体按当前选中实体整份重写（那是 Apply 的职责，且有专门的确认路径）。
        // 旧实现直接以实体名落盘，选错一次实体再点"创建预制体"就会毁掉原文件。
        let stem = unique_prefab_stem(&dir, &name);
        let relative = format!("assets/prefabs/{stem}.prefab");
        if let Some(problem) =
            nesting_problem_at_creation(&self.project.root, &scene.scene, selected, &relative)
        {
            self.status = problem;
            return;
        }
        let prefab = Prefab::from_scene(
            &scene.scene,
            selected,
            AssetId::from_seed(&stem),
            stem.clone(),
        );
        match prefab {
            Ok(mut prefab) => {
                // 新建预制体也是一次"外层写回"：子树里已挂的内层实例在此建立同步基线，
                // 之后再同步内层时才有记录可依。
                if let Err(error) =
                    crate::actions::prepare_nested_lineage(&mut prefab, &self.project.root)
                {
                    self.status = error;
                    return;
                }
                if std::fs::create_dir_all(&dir).is_err() {
                    self.status = "无法创建 prefabs 目录".to_owned();
                    return;
                }
                if let Err(error) = prefab.save(dir.join(format!("{stem}.prefab"))) {
                    self.status = format!("预制体保存失败：{error}");
                    return;
                }
                // 状态里报**最终路径**：名字被去重时用户必须知道写到哪去了。
                // 同时写一条 Console 记录：`refresh_resources()` 会覆盖状态栏，而"写到了哪个
                // 文件"是事后还要能查的事实（去重后缀尤其需要）。
                let message = if stem == name {
                    format!("已创建预制体 {relative}")
                } else {
                    format!("已创建预制体 {relative}（同名已存在，改用了 {stem}）")
                };
                self.push_console_notice(format!("[prefab] {message}"));
                self.status = message;
                self.refresh_resources();
            }
            Err(error) => self.status = format!("创建预制体失败：{error}"),
        }
    }
}

/// 同名预制体已存在时给出不冲突的文件名主干：`Hero` → `Hero_1` → `Hero_2`…
///
/// 与"新建场景避重名"（`next_new_scene_name`）同一思路：编辑器不该因为用户手上恰好有
/// 两个同名实体就毁掉磁盘上的资产。上限之后退到毫秒时间戳，保证一定拿得到唯一名。
pub(crate) fn unique_prefab_stem(dir: &std::path::Path, stem: &str) -> String {
    let taken = |candidate: &str| dir.join(format!("{candidate}.prefab")).exists();
    if !taken(stem) {
        return stem.to_owned();
    }
    for index in 1..=9_999_u32 {
        let candidate = format!("{stem}_{index}");
        if !taken(&candidate) {
            return candidate;
        }
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    format!("{stem}_{stamp}")
}

/// 创建**嵌套**预制体时要拒绝的两类坏数据，返回给用户的一句话；`None` = 可以创建。
///
/// 事实源是磁盘上那些内层文件各自的引用闭包（`rustlua_editor_core::prefab_graph` 的扫描器）：
///
/// * 子树里直接挂着"正在创建的这张"（`assets/prefabs/名字.prefab`）→ 新文件一上来就引用自己；
/// * 内层链绕回这张新文件、或内层链本身成环 → 两份文件互相引用，谁是事实源没有答案；
/// * 内层链超过扫描深度上限 → 再往下没人验证过，写回时可能踩到未展开的层级。
fn nesting_problem_at_creation(
    project_root: &std::path::Path,
    scene: &rustlua_scene::scene::Scene,
    root: SceneEntityId,
    new_relative: &str,
) -> Option<String> {
    let sources = subtree_prefab_sources(scene, root);
    if sources.is_empty() {
        return None;
    }
    if sources.iter().any(|source| source == new_relative) {
        return Some(format!(
            "所选实体里已经挂着这张预制体自己的实例（{new_relative}）：嵌套会指向自己，请先解包再创建"
        ));
    }
    let root = strip_verbatim_prefix(project_root).to_path_buf();
    let links = |name: &str| {
        Prefab::load(root.join(name))
            .map(|prefab| prefab.nested_sources())
            .unwrap_or_default()
    };
    for source in &sources {
        let report = prefab_graph::scan_prefab_nesting(source, links);
        if let Some(cycle) = report.cycles.first() {
            return Some(format!(
                "内层预制体 {source} 的引用成环（{}），嵌套后无法确定谁是事实源，请先解开",
                cycle.join(" → ")
            ));
        }
        if report.transitive.iter().any(|item| item == new_relative) {
            return Some(format!(
                "内层预制体 {source} 已经引用了正在创建的这张（{new_relative}）：嵌套会成环，请先换个名字"
            ));
        }
        if !report.truncated_at.is_empty() {
            return Some(format!(
                "内层预制体 {source} 的嵌套链超过 {} 层（{}），请先减少层级",
                prefab_graph::MAX_NESTING_DEPTH,
                report.truncated_at.join("、")
            ));
        }
    }
    None
}

/// 子树里出现的**全部** `prefab.source`（含根自己；去重、排序）。
fn subtree_prefab_sources(scene: &rustlua_scene::scene::Scene, root: SceneEntityId) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        let Some(entity) = scene.entities.get(&id) else {
            continue;
        };
        if let Some(source) = entity
            .metadata
            .get("prefab.source")
            .filter(|source| !source.trim().is_empty())
        {
            out.push(source.clone());
        }
        stack.extend(entity.children.iter().copied());
    }
    out.sort();
    out.dedup();
    out
}

/// 扫描项目 `scenes/` 目录下的场景文件（相对路径，按名称排序）。
fn scan_scene_files(root: &std::path::Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root.join("scenes")) {
        let mut paths: Vec<_> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "scene"))
            .collect();
        paths.sort();
        for path in paths {
            if let Ok(relative) = path.strip_prefix(root) {
                files.push(relative.to_path_buf());
            }
        }
    }
    files
}
