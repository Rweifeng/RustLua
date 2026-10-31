//! 资源**移动/重命名**时的**引用保持**（docs/55 P1 的 Project Browser 一条）。
//!
//! 为什么单独成模块：这是"改一个文件、动一堆文档"的操作，安全性全靠两条纪律，
//! 而它们都必须可单测——
//! 1. **只替换整段取值**：`texture=assets/a.png` 里的 `assets/a.png` 会被替换，
//!    但 `assets/a.png` 出现在更长路径里（`assets/a.png.backup`）或只作为子串出现时**不动**。
//!    子串替换是这类功能最经典的静默数据损坏。
//! 2. **解析成功才写回**：加载不了的场景/Prefab 文件一律不覆盖（docs/55 §8："数据损坏直接阻断"）。
//!
//! 引用有三个承载位置，缺一个就会留下悬空谱系：组件串、实体/文档元数据（`prefab.source`、
//! `prefab.variant_base`），以及把路径写进**键名**的嵌套签名（`prefab.nested_stamp.<内层路径>`）。
//! `.prefab` 文件走 `Prefab::load`/`save`（场景解析器读不了 `prefab_version=`）。

use rustlua_scene::prefab::{PREFAB_NESTED_STAMP_META_PREFIX, Prefab};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 组件串里的路径写法（正斜杠）；Windows 上手工写的反斜杠先归一化再比较。
fn normalize_path(raw: &str) -> String {
    raw.trim().replace('\\', "/")
}

/// 改写**单个组件串**里的资源引用，返回 `Some(新串)` 表示确实改动了。
///
/// 覆盖两种承载形式：
/// - `key=value`（`texture=…`/`mesh=…`/`file=…`）：整段取值相等才替换；
/// - 位置式段（`Animation` 的 `clip:<名>:<sheet>:<cols>:<fps>:<looped>`）：冒号分隔的参数
///   逐个比较，相等才替换（`sheet` 是资源路径）。
pub fn rewrite_component_reference(payload: &str, from: &str, to: &str) -> Option<String> {
    let from = normalize_path(from);
    if from.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(payload.len());
    let mut changed = false;
    let mut segment_start = 0usize;
    for (index, ch) in payload.char_indices() {
        if ch == ';' || ch == '|' {
            push_segment(
                &mut out,
                &payload[segment_start..index],
                &from,
                to,
                &mut changed,
            );
            out.push(ch);
            segment_start = index + ch.len_utf8();
        }
    }
    push_segment(&mut out, &payload[segment_start..], &from, to, &mut changed);
    changed.then_some(out)
}

fn push_segment(out: &mut String, segment: &str, from: &str, to: &str, changed: &mut bool) {
    match rewrite_segment(segment, from, to) {
        Some(next) => {
            out.push_str(&next);
            *changed = true;
        }
        None => out.push_str(segment),
    }
}

fn rewrite_segment(segment: &str, from: &str, to: &str) -> Option<String> {
    if let Some((key, value)) = segment.split_once('=') {
        // `key=value`：整段取值相等才换（不做子串替换）。
        return (normalize_path(value) == from).then(|| format!("{key}={to}"));
    }
    if segment.contains(':') {
        let mut changed = false;
        let parts: Vec<String> = segment
            .split(':')
            .map(|part| {
                if normalize_path(part) == from {
                    changed = true;
                    to.to_owned()
                } else {
                    part.to_owned()
                }
            })
            .collect();
        return changed.then(|| parts.join(":"));
    }
    None
}

/// 元数据**值**里的引用：既可能是裸路径（`prefab.source`），也可能是整条覆盖快照
/// （`component:Sprite` 级覆盖存的是组件串）。两种口径都试，仍然只认整段相等。
fn rewrite_metadata_value(value: &str, from: &str, to: &str) -> Option<String> {
    if normalize_path(value) == from {
        return Some(to.to_owned());
    }
    rewrite_component_reference(value, from, to)
}

/// 把 `prefab.nested_stamp.<内层路径>` 键名里的路径跟着改；返回改动的键数。
fn rewrite_stamp_keys(metadata: &mut BTreeMap<String, String>, from: &str, to: &str) -> usize {
    let renames: Vec<String> = metadata
        .keys()
        .filter(|key| {
            key.strip_prefix(PREFAB_NESTED_STAMP_META_PREFIX)
                .is_some_and(|source| normalize_path(source) == from)
        })
        .cloned()
        .collect();
    for old in &renames {
        if let Some(value) = metadata.remove(old) {
            metadata.insert(format!("{PREFAB_NESTED_STAMP_META_PREFIX}{to}"), value);
        }
    }
    renames.len()
}

/// 改一份元数据表里的引用（值 + 写进键名的嵌套签名）；返回改动数。
fn rewrite_metadata_references(
    metadata: &mut BTreeMap<String, String>,
    from: &str,
    to: &str,
) -> usize {
    let mut changed = rewrite_stamp_keys(metadata, from, to);
    for value in metadata.values_mut() {
        if let Some(next) = rewrite_metadata_value(value, from, to) {
            *value = next;
            changed += 1;
        }
    }
    changed
}

/// 改写一份场景（含以场景文档打开的 Prefab）里的全部引用；返回**被改动的引用数**。
///
/// 三处都要改：组件串、实体元数据（嵌套实例的内层来源）、文档元数据（变体基础 +
/// 嵌套签名键）。只改组件串会让移动内层/基础预制体后谱系指向旧路径。
pub fn rewrite_scene_references(
    scene: &mut rustlua_scene::scene::Scene,
    from: &str,
    to: &str,
) -> usize {
    let from = normalize_path(from);
    if from.is_empty() {
        return 0;
    }
    let mut changed = 0;
    for entity in scene.entities.values_mut() {
        for payload in entity.components.values_mut() {
            if let Some(next) = rewrite_component_reference(payload, &from, to) {
                *payload = next;
                changed += 1;
            }
        }
        changed += rewrite_metadata_references(&mut entity.metadata, &from, to);
    }
    changed += rewrite_metadata_references(&mut scene.metadata, &from, to);
    changed
}

/// 改写一份 `Prefab` 文件里的全部引用（组件 + 实体元数据 + 文件级元数据）；返回改动数。
///
/// 文件级元数据里有 `prefab.variant_base`（值）与 `prefab.nested_stamp.<内层路径>`（键），
/// 两者都得跟着移动后的路径走，否则变体/嵌套同步会拿旧路径去找签名。
pub fn rewrite_prefab_references(prefab: &mut Prefab, from: &str, to: &str) -> usize {
    let from = normalize_path(from);
    if from.is_empty() {
        return 0;
    }
    let mut changed = 0;
    for entity in prefab.entities.values_mut() {
        for payload in entity.components.values_mut() {
            if let Some(next) = rewrite_component_reference(payload, &from, to) {
                *payload = next;
                changed += 1;
            }
        }
        changed += rewrite_metadata_references(&mut entity.metadata, &from, to);
    }
    changed += rewrite_metadata_references(&mut prefab.metadata, &from, to);
    changed
}

/// 一条"谁引用了这条路径"的证据（安全删除用）。
///
/// 定位串是人读的：`assets/scenes/main.scene ▸ E#3 ▸ Sprite`。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferenceHit {
    pub location: String,
}

/// 组件串里是否**整段**指向 `target`（判据与改写同一条，避免"扫描说没引用、改指却会改"）。
fn component_references(payload: &str, target: &str) -> bool {
    rewrite_component_reference(payload, target, target).is_some()
}

/// 元数据值是否指向 `target`（裸路径或组件串两种形态都试）。
fn metadata_value_references(value: &str, target: &str) -> bool {
    rewrite_metadata_value(value, target, target).is_some()
}

/// 键名里的嵌套签名是否指向 `target`。
fn stamp_key_references(key: &str, target: &str) -> bool {
    key.strip_prefix(PREFAB_NESTED_STAMP_META_PREFIX)
        .is_some_and(|source| normalize_path(source) == target)
}

/// 收集一份文档里所有指向 `target` 的引用（组件串 / 实体元数据 / 文档元数据 / 嵌套签名键）。
///
/// **判据必须与改写同源**：[`rewrite_scene_references`] 会改的地方，这里一条都不能漏——
/// 否则会出现"删除守卫说没人引用，资源一删就断链"的自相矛盾。
fn collect_document_references(
    entities: &std::collections::BTreeMap<
        rustlua_scene::scene::SceneEntityId,
        rustlua_scene::scene::SceneEntity,
    >,
    document_metadata: &BTreeMap<String, String>,
    target: &str,
    document_label: &str,
) -> Vec<ReferenceHit> {
    let target = normalize_path(target);
    if target.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for entity in entities.values() {
        for (key, payload) in &entity.components {
            if component_references(payload, &target) {
                hits.push(ReferenceHit {
                    location: format!("{document_label} ▸ E#{} ▸ {key}", entity.id.0),
                });
            }
        }
        for (key, value) in &entity.metadata {
            if metadata_value_references(value, &target) {
                hits.push(ReferenceHit {
                    location: format!("{document_label} ▸ E#{} ▸ {key}", entity.id.0),
                });
            }
        }
    }
    for (key, value) in document_metadata {
        if metadata_value_references(value, &target) {
            hits.push(ReferenceHit {
                location: format!("{document_label} ▸ 文档元数据 ▸ {key}"),
            });
        }
        if stamp_key_references(key, &target) {
            hits.push(ReferenceHit {
                location: format!("{document_label} ▸ 嵌套签名 ▸ {key}"),
            });
        }
    }
    hits
}

/// 收集一份场景里指向 `target` 的引用。
#[must_use]
pub fn collect_scene_references(
    scene: &rustlua_scene::scene::Scene,
    target: &str,
    document_label: &str,
) -> Vec<ReferenceHit> {
    collect_document_references(&scene.entities, &scene.metadata, target, document_label)
}

/// 收集一份 `Prefab` 里指向 `target` 的引用。
#[must_use]
pub fn collect_prefab_references(
    prefab: &Prefab,
    target: &str,
    document_label: &str,
) -> Vec<ReferenceHit> {
    collect_document_references(&prefab.entities, &prefab.metadata, target, document_label)
}

/// 收集一份磁盘文档（`.scene`/`.prefab`）里指向 `target` 的引用。
///
/// 解析器按扩展名选（`.prefab` 走 [`Prefab::load`]）：用错解析器会让每份预制体都落进
/// "读不动"，里面的引用一条也看不见。
pub fn collect_document_references_on_disk(
    path: &Path,
    target: &str,
    document_label: &str,
) -> Result<Vec<ReferenceHit>, String> {
    let is_prefab = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("prefab"));
    if is_prefab {
        let prefab = Prefab::load(path).map_err(|error| error.to_string())?;
        Ok(collect_prefab_references(&prefab, target, document_label))
    } else {
        let scene = rustlua_scene::scene::Scene::load(path).map_err(|error| error.to_string())?;
        Ok(collect_scene_references(&scene, target, document_label))
    }
}

/// 一次工程级改写的报告。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RewriteReport {
    /// 被改写并写回的文件（项目相对路径，正斜杠）。
    pub rewritten_files: Vec<String>,
    /// 被改写的引用总数（组件串 / 元数据值 / 嵌套签名键）。
    pub rewritten_references: usize,
    /// 解析失败/写回失败的文件（**原文件未被覆盖**）。
    pub failures: Vec<String>,
}

/// 单份文档的改写结果：把"解析失败/写回失败"分开，调用方才能给出准确原因。
enum FileRewrite {
    Unchanged,
    Rewritten(usize),
    ParseFailed,
    WriteFailed(String),
}

fn rewrite_document_file(path: &Path, from: &str, to: &str) -> FileRewrite {
    let is_prefab = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("prefab"));
    if is_prefab {
        let Ok(mut prefab) = Prefab::load(path) else {
            return FileRewrite::ParseFailed;
        };
        let changed = rewrite_prefab_references(&mut prefab, from, to);
        if changed == 0 {
            return FileRewrite::Unchanged;
        }
        return match prefab.save(path) {
            Ok(()) => FileRewrite::Rewritten(changed),
            Err(error) => FileRewrite::WriteFailed(error.to_string()),
        };
    }
    let Ok(mut scene) = rustlua_scene::scene::Scene::load(path) else {
        return FileRewrite::ParseFailed;
    };
    let changed = rewrite_scene_references(&mut scene, from, to);
    if changed == 0 {
        return FileRewrite::Unchanged;
    }
    match scene.save(path) {
        Ok(()) => FileRewrite::Rewritten(changed),
        Err(error) => FileRewrite::WriteFailed(error.to_string()),
    }
}

/// 改写工程目录下所有 `.scene`/`.prefab` 里的引用。
///
/// `skip` 通常是"当前打开的文档"：它由编辑器在内存里改（保住用户未保存的编辑），
/// 这里不碰它的磁盘文件。
pub fn rewrite_project_references(
    root: &Path,
    from: &str,
    to: &str,
    skip: Option<&Path>,
) -> RewriteReport {
    let mut report = RewriteReport::default();
    for path in project_scene_documents(root) {
        if skip.is_some_and(|skip| paths_match(skip, &path)) {
            continue;
        }
        match rewrite_document_file(&path, from, to) {
            FileRewrite::Unchanged => {}
            FileRewrite::Rewritten(changed) => {
                report.rewritten_references += changed;
                report.rewritten_files.push(relative_display(root, &path));
            }
            FileRewrite::ParseFailed => report
                .failures
                .push(relative_display(root, &path) + "（解析失败，未改动）"),
            FileRewrite::WriteFailed(error) => report.failures.push(format!(
                "{}（写回失败：{error}）",
                relative_display(root, &path)
            )),
        }
    }
    report
}

/// 工程目录下全部 `.scene`/`.prefab` 文档（已排序，绝对路径）。
///
/// 这就是 [`rewrite_project_references`] 会改写的那一批。审计（断链检查）必须复用
/// 同一份遍历口径：两边看到的文件集合不同，就会出现"扫描说没断链、改指却漏改那份"
/// 这类无法自证的差别。
#[must_use]
pub fn project_scene_documents(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_documents(root, &mut files, 0);
    files.sort();
    files
}

/// 递归收集场景/Prefab 文档（跳过构建产物与 VCS 目录；深度上限防御异常目录结构）。
fn collect_documents(directory: &Path, out: &mut Vec<PathBuf>, depth: usize) {
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
                "builds" | "target" | ".git" | ".vs" | "node_modules" | "artifacts"
            ) {
                continue;
            }
            collect_documents(&path, out, depth + 1);
            continue;
        }
        let is_document = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                let extension = extension.to_ascii_lowercase();
                extension == "scene" || extension == "prefab"
            });
        if is_document {
            out.push(path);
        }
    }
}

/// 去掉 Windows 的 `\\?\` 前缀（canonicalize 会加上它，逐字符比较会因此误判不相等）。
#[must_use]
pub fn strip_verbatim_prefix(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => path.to_path_buf(),
    }
}

/// 把外部给出的路径收敛成**工程内相对路径**：绝对路径、`..`、盘符一律拒绝（返回 `None`）。
///
/// 资源移动必须走这条闸门：`..\\..\\windows\\system32` 这种输入不该有机会碰到工程外的文件。
#[must_use]
pub fn normalize_relative(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => out.push(part),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// 两个路径是否指向同一个文件（尽量比较规范化后的绝对路径，失败则退回原路径比较）。
#[must_use]
pub fn paths_match(left: &Path, right: &Path) -> bool {
    let canonical = |path: &Path| {
        strip_verbatim_prefix(&std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
    };
    canonical(left) == canonical(right)
}

/// 项目相对显示路径（正斜杠）；不在工程内时退回原样。
fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustlua_scene::scene::Scene;

    /// 只替换**整段取值**：同名前缀/后缀的路径不许被顺手改掉（子串替换是典型的数据损坏源）。
    #[test]
    fn rewrites_only_whole_reference_values() {
        let payload = "texture=assets/textures/a.png;size=1x1;tint=1,1,1,1;sorting_layer=default";
        assert_eq!(
            rewrite_component_reference(payload, "assets/textures/a.png", "assets/textures/b.png")
                .as_deref(),
            Some("texture=assets/textures/b.png;size=1x1;tint=1,1,1,1;sorting_layer=default")
        );
        // 没有引用 → None（调用方据此跳过整个文件）。
        assert_eq!(
            rewrite_component_reference(payload, "assets/textures/c.png", "assets/x.png"),
            None
        );
        // 只是**子串**（更长路径 / 更短路径）时不动。
        let longer = "texture=assets/textures/a.png.backup;size=1x1";
        assert_eq!(
            rewrite_component_reference(longer, "assets/textures/a.png", "assets/b.png"),
            None
        );
        let shorter = "texture=a.png;size=1x1";
        assert_eq!(
            rewrite_component_reference(shorter, "assets/textures/a.png", "assets/b.png"),
            None
        );
        // 反斜杠写法按组件串约定归一化后比较，替换结果用正斜杠。
        assert_eq!(
            rewrite_component_reference(
                "mesh=assets\\models\\hero.glb;clip=Idle",
                "assets/models/hero.glb",
                "assets/models/hero2.glb"
            )
            .as_deref(),
            Some("mesh=assets/models/hero2.glb;clip=Idle")
        );
        // 空 from 是无效输入（不许把空前缀到处替换）。
        assert_eq!(
            rewrite_component_reference(payload, "", "assets/x.png"),
            None
        );
    }

    /// 位置式段（`Animation` 的 `clip:<名>:<sheet>:…`）里的资源路径也要跟着改。
    #[test]
    fn rewrites_positional_clip_sheet_and_multiple_records() {
        let payload = "default=squish;clip:squish:assets/textures/slime.png:2:3:true";
        assert_eq!(
            rewrite_component_reference(
                payload,
                "assets/textures/slime.png",
                "assets/textures/red.png"
            )
            .as_deref(),
            Some("default=squish;clip:squish:assets/textures/red.png:2:3:true")
        );
        // 多条记录的 `|` 分隔与记录内 `;` 都要原样保留（不能把分隔符吃掉或改掉）。
        let multi = "track.a=0.000:0.000:linear|track.b=assets/x.png";
        assert_eq!(
            rewrite_component_reference(multi, "assets/x.png", "assets/y.png").as_deref(),
            Some("track.a=0.000:0.000:linear|track.b=assets/y.png")
        );
        assert_eq!(
            rewrite_component_reference(multi, "assets/none.png", "assets/x.png"),
            None
        );
        // 位置式段里没有"这是路径"的标记，因此**调用方必须传资源路径**：
        // 传一个裸数字会改到 `cols` 这种位置参数上。编辑器侧的入口
        // （`rewrite_scene_references` / `perform_resource_move`）只会拿到选中资源的路径。
        let clip = "default=x;clip:walk:assets/a.png:2:3:true";
        assert_eq!(
            rewrite_component_reference(clip, "2", "9").as_deref(),
            Some("default=x;clip:walk:assets/a.png:9:3:true"),
            "记录语义：位置参数相等即替换（所以 from 必须是资源路径，不能是数字）"
        );
    }

    /// 场景级改写 + 工程级改写：**解析失败的文件绝不被覆盖**。
    #[test]
    fn project_rewrite_updates_documents_and_skips_broken_files() {
        let root = std::env::temp_dir().join(format!(
            "rustlua-resource-ops-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        std::fs::create_dir_all(root.join("assets/textures")).unwrap();
        std::fs::create_dir_all(root.join("builds")).unwrap();
        std::fs::write(root.join("assets/textures/old.png"), b"png").unwrap();
        // 一个引用它的场景 + 一个不引用它的场景 + 一个坏文件 + 一个构建产物里的场景。
        let mut scene = Scene::new("main");
        scene
            .add_entity(rustlua_scene::scene::SceneEntityId(1), "Hero", None)
            .unwrap();
        scene
            .entities
            .get_mut(&rustlua_scene::scene::SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Sprite".to_owned(),
                "texture=assets/textures/old.png;size=1x1;tint=1,1,1,1;sorting_layer=default"
                    .to_owned(),
            );
        scene.save(root.join("scenes/main.scene")).unwrap();
        std::fs::write(root.join("scenes/other.scene"), b"name = \"other\"\n").unwrap();
        std::fs::write(root.join("scenes/broken.scene"), b"\0\0\0").unwrap();
        std::fs::write(root.join("builds/kept.scene"), b"name = \"kept\"\n").unwrap();
        let broken_before = std::fs::read(root.join("scenes/broken.scene")).unwrap();

        let report = rewrite_project_references(
            &root,
            "assets/textures/old.png",
            "assets/textures/new.png",
            None,
        );
        assert_eq!(report.rewritten_files, vec!["scenes/main.scene".to_owned()]);
        assert_eq!(report.rewritten_references, 1);
        assert_eq!(
            std::fs::read(root.join("scenes/broken.scene")).unwrap(),
            broken_before,
            "解析失败的文件必须原样保留"
        );
        let reloaded = Scene::load(root.join("scenes/main.scene")).unwrap();
        assert!(
            reloaded.entities[&rustlua_scene::scene::SceneEntityId(1)].components["Sprite"]
                .contains("assets/textures/new.png")
        );
        // `skip`（当前打开的文档）不碰磁盘文件——它由编辑器在内存里改。
        let report = rewrite_project_references(
            &root,
            "assets/textures/new.png",
            "assets/textures/third.png",
            Some(&root.join("scenes/main.scene")),
        );
        assert!(report.rewritten_files.is_empty());
        assert!(
            Scene::load(root.join("scenes/main.scene"))
                .unwrap()
                .entities[&rustlua_scene::scene::SceneEntityId(1)]
                .components["Sprite"]
                .contains("assets/textures/new.png")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// 以场景文档打开的 Prefab（变体/嵌套）在内存里改写时，元数据也要跟着改。
    ///
    /// 编辑器只对当前打开文档调用 `rewrite_scene_references`；漏掉元数据，用户保存后
    /// 文件里仍是旧路径，谱系与嵌套签名就指向一个不存在的文件。
    #[test]
    fn scene_rewrite_covers_entity_and_document_metadata() {
        let mut scene = Scene::new("variant");
        scene
            .add_entity(rustlua_scene::scene::SceneEntityId(1), "Root", None)
            .unwrap();
        let root = scene
            .entities
            .get_mut(&rustlua_scene::scene::SceneEntityId(1))
            .unwrap();
        root.metadata.insert(
            rustlua_scene::prefab::PREFAB_SOURCE_META_KEY.to_owned(),
            "assets/prefabs/old.prefab".to_owned(),
        );
        // 更长路径只是子串，不允许被顺手改掉（元数据值同样只认整段相等）。
        root.metadata.insert(
            "note".to_owned(),
            "assets/prefabs/old.prefab.backup".to_owned(),
        );
        scene.metadata.insert(
            rustlua_scene::prefab::PREFAB_VARIANT_BASE_META_KEY.to_owned(),
            "assets/prefabs/old.prefab".to_owned(),
        );
        scene.metadata.insert(
            format!("{PREFAB_NESTED_STAMP_META_PREFIX}assets/prefabs/old.prefab"),
            "1:2".to_owned(),
        );

        let changed = rewrite_scene_references(
            &mut scene,
            "assets/prefabs/old.prefab",
            "assets/prefabs/new.prefab",
        );
        assert_eq!(changed, 3, "实体元数据值 + 变体基础值 + 嵌套签名键");
        let root = &scene.entities[&rustlua_scene::scene::SceneEntityId(1)];
        assert_eq!(
            root.metadata[rustlua_scene::prefab::PREFAB_SOURCE_META_KEY],
            "assets/prefabs/new.prefab"
        );
        assert_eq!(
            root.metadata["note"], "assets/prefabs/old.prefab.backup",
            "子串不是引用"
        );
        assert_eq!(
            scene.metadata[rustlua_scene::prefab::PREFAB_VARIANT_BASE_META_KEY],
            "assets/prefabs/new.prefab"
        );
        assert_eq!(
            scene.metadata[&format!("{PREFAB_NESTED_STAMP_META_PREFIX}assets/prefabs/new.prefab")],
            "1:2"
        );
        assert!(!scene.metadata.contains_key(&format!(
            "{PREFAB_NESTED_STAMP_META_PREFIX}assets/prefabs/old.prefab"
        )));
    }

    /// `.prefab` 文件走 `Prefab::load`/`save`：组件串、实体/文件元数据**值**、
    /// 嵌套签名**键名**三处引用都要跟着改，坏文件原样保留。
    #[test]
    fn prefab_files_rewrite_components_metadata_values_and_stamp_keys() {
        use rustlua_assets::resource_db::AssetId;
        use rustlua_scene::prefab::{PREFAB_SOURCE_META_KEY, Prefab};
        use rustlua_scene::scene::SceneEntityId;

        let root = std::env::temp_dir().join(format!(
            "rustlua-resource-ops-prefab-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("assets/prefabs")).unwrap();
        std::fs::create_dir_all(root.join("assets/textures")).unwrap();
        std::fs::write(root.join("assets/textures/old.png"), b"png").unwrap();

        // 外层预制体：组件引用纹理；实体元数据指向内层；文件元数据带变体谱系与嵌套签名。
        let mut scene = Scene::new("Outer");
        scene.add_entity(SceneEntityId(1), "Outer", None).unwrap();
        scene
            .add_entity(SceneEntityId(2), "Inner", Some(SceneEntityId(1)))
            .unwrap();
        let inner = scene.entities.get_mut(&SceneEntityId(2)).unwrap();
        inner.components.insert(
            "Sprite".to_owned(),
            "texture=assets/textures/old.png;size=1x1;tint=1,1,1,1;sorting_layer=default"
                .to_owned(),
        );
        inner.metadata.insert(
            PREFAB_SOURCE_META_KEY.to_owned(),
            "assets/prefabs/old_inner.prefab".to_owned(),
        );
        let mut prefab = Prefab::from_scene(
            &scene,
            SceneEntityId(1),
            AssetId::from_seed("outer"),
            "Outer",
        )
        .unwrap();
        prefab.metadata.insert(
            rustlua_scene::prefab::PREFAB_VARIANT_BASE_META_KEY.to_owned(),
            "assets/prefabs/old_inner.prefab".to_owned(),
        );
        prefab.metadata.insert(
            rustlua_scene::prefab::PREFAB_VARIANT_BASE_STAMP_META_KEY.to_owned(),
            "9:9".to_owned(),
        );
        prefab.metadata.insert(
            format!("{PREFAB_NESTED_STAMP_META_PREFIX}assets/prefabs/old_inner.prefab"),
            "1:2".to_owned(),
        );
        prefab
            .save(root.join("assets/prefabs/outer.prefab"))
            .unwrap();
        std::fs::write(
            root.join("assets/prefabs/broken.prefab"),
            b"prefab_version=9\n",
        )
        .unwrap();
        let broken_before = std::fs::read(root.join("assets/prefabs/broken.prefab")).unwrap();

        // 内层来源移动：实体元数据值 + 变体基础值 + 嵌套签名键，三处各算一处引用。
        let report = rewrite_project_references(
            &root,
            "assets/prefabs/old_inner.prefab",
            "assets/prefabs/new_inner.prefab",
            None,
        );
        assert_eq!(
            report.rewritten_files,
            vec!["assets/prefabs/outer.prefab".to_owned()]
        );
        assert_eq!(report.rewritten_references, 3);
        assert_eq!(
            std::fs::read(root.join("assets/prefabs/broken.prefab")).unwrap(),
            broken_before,
            "解析失败的 Prefab 必须原样保留"
        );
        let mut reloaded = Prefab::load(root.join("assets/prefabs/outer.prefab")).unwrap();
        assert_eq!(
            reloaded.entities[&SceneEntityId(2)].metadata[PREFAB_SOURCE_META_KEY],
            "assets/prefabs/new_inner.prefab"
        );
        assert_eq!(
            reloaded.variant_base(),
            Some("assets/prefabs/new_inner.prefab")
        );
        assert_eq!(
            reloaded.nested_stamp("assets/prefabs/new_inner.prefab"),
            Some("1:2"),
            "签名值跟着键名一起搬"
        );
        assert!(
            reloaded
                .nested_stamp("assets/prefabs/old_inner.prefab")
                .is_none()
        );

        // 纹理移动：组件串里的引用单独走组件口径。
        let report = rewrite_project_references(
            &root,
            "assets/textures/old.png",
            "assets/textures/new.png",
            None,
        );
        assert_eq!(report.rewritten_references, 1);
        reloaded = Prefab::load(root.join("assets/prefabs/outer.prefab")).unwrap();
        assert!(
            reloaded.entities[&SceneEntityId(2)].components["Sprite"]
                .contains("assets/textures/new.png")
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
