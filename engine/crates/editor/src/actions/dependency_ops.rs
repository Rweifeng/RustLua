//! 资源依赖查看（只读）：一个资源**引用谁**（出边）与**谁引用它**（入边）。
//!
//! 与断链审计、资源重命名改写引用共用同一台扫描器
//! （`rustlua_assets::dependency::extract_refs`）：三处口径一致，才不会出现
//! "改名说改了两处、依赖视图只认一处"这类无法自证的差别。
//!
//! 资产 ID 不需要 `.meta`/GUID 来支撑这件事：id 由**路径**派生，引用就是路径文本，
//! 因此出边/入边都能从文本直接扫出来，搬文件时改写路径即可保持引用。

use crate::EditorApp;
use std::path::{Path, PathBuf};

/// 依赖查看结果（只读，落进资源面板的依赖区）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ResourceDependencyReport {
    /// 被查看的资源（工程内相对路径，`/` 分隔）。
    pub target: String,
    /// 它引用的资源（去重、排序后的展示文本）。
    pub outgoing: Vec<String>,
    /// 引用它的工程文档。
    pub incoming: Vec<String>,
}

impl EditorApp {
    /// 计算 `relative` 的依赖两边并写进 `resource_dependency`（面板只负责呈现）。
    ///
    /// 代价：入边要读全工程的文本型文档（场景/预制体/材质/脚本），所以只由右键
    /// 显式触发，不在每帧计算。当前打开的文档用**内存版**参与（未保存的编辑也算）。
    pub(crate) fn show_resource_dependencies(&mut self, relative: &Path) {
        use rustlua_assets::dependency::{DependencyRef, extract_refs};
        use rustlua_assets::resource_kind::ResourceKind;

        let root = self.project.root.clone();
        let target = relative.to_string_lossy().replace('\\', "/");
        let mut outgoing = Vec::new();
        match std::fs::read_to_string(root.join(relative)) {
            Ok(text) => {
                let kind = ResourceKind::from_path(relative);
                for reference in extract_refs(&text, kind) {
                    let text = match reference {
                        DependencyRef::Asset(path) => path.to_string_lossy().replace('\\', "/"),
                        DependencyRef::LuaModule(name) => format!("require(\"{name}\")"),
                        DependencyRef::SceneName(name) => format!("scene.load(\"{name}\")"),
                        DependencyRef::Dynamic(text) => format!("动态：{text}"),
                    };
                    if !outgoing.contains(&text) {
                        outgoing.push(text);
                    }
                }
            }
            Err(error) => {
                self.status = format!("读取资源失败：{}：{error}", relative.display());
            }
        }

        let mut incoming = Vec::new();
        let open_document = self
            .document
            .current_scene
            .as_ref()
            .and_then(|document| document.path.clone());
        // 磁盘上的文档；当前打开的那一份跳过（下面用内存版，避免同一份数两遍）。
        for path in editor::project_scene_documents(&root) {
            if open_document
                .as_ref()
                .is_some_and(|open| editor::paths_match(open, &path))
            {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let kind = ResourceKind::from_path(&path);
            if references_target(&text, kind, &target) {
                incoming.push(relative_label(&root, &path));
            }
        }
        if let Some(scene) = self.document.current_scene.as_ref() {
            let label = open_document
                .as_ref()
                .map(|path| relative_label(&root, path))
                .unwrap_or_else(|| "未命名场景".to_owned());
            let kind = open_document
                .as_ref()
                .map_or(ResourceKind::Scene, ResourceKind::from_path);
            if references_target(&scene.scene.to_text(), kind, &target) {
                incoming.push(label);
            }
        }
        // 脚本：`require` 与资源配置里的路径都可能指向它，用同一台扫描器。
        for entry in self.resources.entries() {
            let extension = entry
                .path
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if extension != "lua" || entry.path == relative {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(root.join(&entry.path)) else {
                continue;
            };
            if references_target(&text, ResourceKind::LuaScript, &target) {
                incoming.push(entry.path.to_string_lossy().replace('\\', "/"));
            }
        }
        incoming.sort();
        incoming.dedup();
        outgoing.sort();
        self.resource_dependency = Some(ResourceDependencyReport {
            target,
            outgoing,
            incoming,
        });
    }
}

/// 文本里是否有一处引用正好指向 `target`（工程内相对路径）。
fn references_target(
    text: &str,
    kind: rustlua_assets::resource_kind::ResourceKind,
    target: &str,
) -> bool {
    use rustlua_assets::dependency::{DependencyRef, extract_refs};
    extract_refs(text, kind)
        .iter()
        .any(|reference| match reference {
            DependencyRef::Asset(path) => path.to_string_lossy().replace('\\', "/") == target,
            DependencyRef::LuaModule(name) => target == format!("scripts/{name}.lua"),
            DependencyRef::SceneName(name) => target == format!("scenes/{name}.scene"),
            DependencyRef::Dynamic(_) => false,
        })
}

fn relative_label(root: &Path, path: &Path) -> String {
    let path: PathBuf = path
        .strip_prefix(root)
        .map_or_else(|_| path.to_path_buf(), Path::to_path_buf);
    path.to_string_lossy().replace('\\', "/")
}
