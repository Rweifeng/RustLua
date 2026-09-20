//! 资源**移动/重命名**时的**引用保持**（docs/55 P1 的 Project Browser 一条）。
//!
//! 为什么单独成模块：这是"改一个文件、动一堆文档"的操作，安全性全靠两条纪律，
//! 而它们都必须可单测——
//! 1. **只替换整段取值**：`texture=assets/a.png` 里的 `assets/a.png` 会被替换，
//!    但 `assets/a.png` 出现在更长路径里（`assets/a.png.backup`）或只作为子串出现时**不动**。
//!    子串替换是这类功能最经典的静默数据损坏。
//! 2. **解析成功才写回**：加载不了的场景文件一律不覆盖（docs/55 §8："数据损坏直接阻断"）。

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
            push_segment(&mut out, &payload[segment_start..index], &from, to, &mut changed);
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

/// 改写一份场景里所有组件的引用；返回**被改动的组件数**。
pub fn rewrite_scene_references(
    scene: &mut engine::core::scene::Scene,
    from: &str,
    to: &str,
) -> usize {
    let mut changed = 0;
    for entity in scene.entities.values_mut() {
        for payload in entity.components.values_mut() {
            if let Some(next) = rewrite_component_reference(payload, from, to) {
                *payload = next;
                changed += 1;
            }
        }
    }
    changed
}

/// 一次工程级改写的报告。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RewriteReport {
    /// 被改写并写回的文件（项目相对路径，正斜杠）。
    pub rewritten_files: Vec<String>,
    /// 被改写的组件总数。
    pub rewritten_components: usize,
    /// 解析失败/写回失败的文件（**原文件未被覆盖**）。
    pub failures: Vec<String>,
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
    let mut files = Vec::new();
    collect_documents(root, &mut files, 0);
    files.sort();
    for path in files {
        if skip.is_some_and(|skip| paths_match(skip, &path)) {
            continue;
        }
        let Ok(mut scene) = engine::core::scene::Scene::load(&path) else {
            report
                .failures
                .push(relative_display(root, &path) + "（解析失败，未改动）");
            continue;
        };
        let changed = rewrite_scene_references(&mut scene, from, to);
        if changed == 0 {
            continue;
        }
        if let Err(error) = scene.save(&path) {
            report
                .failures
                .push(format!("{}（写回失败：{error}）", relative_display(root, &path)));
            continue;
        }
        report.rewritten_components += changed;
        report.rewritten_files.push(relative_display(root, &path));
    }
    report
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
    use engine::core::scene::Scene;

    /// 只替换**整段取值**：同名前缀/后缀的路径不许被顺手改掉（子串替换是典型的数据损坏源）。
    #[test]
    fn rewrites_only_whole_reference_values() {
        let payload =
            "texture=assets/textures/a.png;size=1x1;tint=1,1,1,1;sorting_layer=default";
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
        assert_eq!(rewrite_component_reference(payload, "", "assets/x.png"), None);
    }

    /// 位置式段（`Animation` 的 `clip:<名>:<sheet>:…`）里的资源路径也要跟着改。
    #[test]
    fn rewrites_positional_clip_sheet_and_multiple_records() {
        let payload = "default=squish;clip:squish:assets/textures/slime.png:2:3:true";
        assert_eq!(
            rewrite_component_reference(payload, "assets/textures/slime.png", "assets/textures/red.png")
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
            "p2-resource-ops-{}-{}",
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
            .add_entity(engine::core::scene::SceneEntityId(1), "Hero", None)
            .unwrap();
        scene.entities.get_mut(&engine::core::scene::SceneEntityId(1)).unwrap().components.insert(
            "Sprite".to_owned(),
            "texture=assets/textures/old.png;size=1x1;tint=1,1,1,1;sorting_layer=default".to_owned(),
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
        assert_eq!(report.rewritten_components, 1);
        assert_eq!(
            std::fs::read(root.join("scenes/broken.scene")).unwrap(),
            broken_before,
            "解析失败的文件必须原样保留"
        );
        let reloaded = Scene::load(root.join("scenes/main.scene")).unwrap();
        assert!(
            reloaded.entities[&engine::core::scene::SceneEntityId(1)].components["Sprite"]
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
                .entities[&engine::core::scene::SceneEntityId(1)]
                .components["Sprite"]
                .contains("assets/textures/new.png")
        );
        let _ = std::fs::remove_dir_all(root);
    }
}
