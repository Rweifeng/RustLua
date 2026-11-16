//! 折叠状态的按场景持久化（M79 结构切分，零行为变化）。
use super::*;

impl EditorApp {
    /// 以项目根相对路径作为场景标识（统一 `/` 分隔），保证跨会话稳定。
    pub(super) fn hierarchy_scene_key(&self) -> Option<String> {
        let path = self.current_scene()?.path.as_deref()?;
        let clean_path = crate::util::strip_verbatim_prefix(path);
        let project_root = crate::util::strip_verbatim_prefix(&self.project.root);
        let relative = clean_path
            .strip_prefix(&project_root)
            .unwrap_or(&clean_path);
        let key = relative.to_string_lossy().replace('\\', "/");
        if key.is_empty() { None } else { Some(key) }
    }

    /// 保存当前场景折叠状态；Play 模式保持原快照机制，不落盘运行期变化。
    pub(crate) fn save_hierarchy_collapsed(&mut self) {
        if self.is_playing() {
            return;
        }
        let Some(key) = self.hierarchy_scene_key() else {
            return;
        };
        let collapsed: std::collections::BTreeSet<u64> =
            self.scene_state.collapsed.iter().map(|id| id.0).collect();
        crate::util::save_hierarchy_collapsed(&self.project.root, &key, &collapsed);
    }

    /// 打开/恢复文档后读取配置；新建或未保存文档没有稳定场景标识，保持全展开。
    pub(crate) fn load_hierarchy_collapsed(&mut self) {
        let Some(document) = self.current_scene() else {
            self.scene_state.collapsed.clear();
            return;
        };
        let Some(key) = self.hierarchy_scene_key() else {
            self.scene_state.collapsed.clear();
            return;
        };
        let collapsed: std::collections::BTreeSet<u64> =
            crate::util::load_hierarchy_collapsed(&self.project.root, &key);
        self.scene_state.collapsed = collapsed
            .into_iter()
            .map(SceneEntityId)
            .filter(|id| document.scene.entities.contains_key(id))
            .collect();
    }

    /// 文档切换时加载/清空；同文档编辑后剔除悬空 ID 并即时落盘。
    pub(crate) fn sync_hierarchy_collapsed(&mut self, change: crate::actions::DocumentChange) {
        match change {
            crate::actions::DocumentChange::Opened => self.load_hierarchy_collapsed(),
            crate::actions::DocumentChange::Created => self.scene_state.collapsed.clear(),
            crate::actions::DocumentChange::PlaybackStopped => {}
            _ => {
                let valid: std::collections::BTreeSet<SceneEntityId> = self
                    .current_scene()
                    .map(|document| document.scene.entities.keys().copied().collect())
                    .unwrap_or_default();
                let had_stale_id = self
                    .scene_state
                    .collapsed
                    .iter()
                    .any(|id| !valid.contains(id));
                self.scene_state.collapsed.retain(|id| valid.contains(id));
                if had_stale_id {
                    self.save_hierarchy_collapsed();
                }
            }
        }
    }
}
