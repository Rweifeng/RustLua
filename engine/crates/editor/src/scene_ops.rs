//! Scene View 的**提交层（之二）**：Tile 绘制落盘与拖放生成实体
//! （docs/55 §4.1 规则 1：View 只发 typed action）。
//!
//! 与 `scene_drag.rs` 同一分工：面板负责绘制、命中测试、资源拖放识别与参数收集；
//! "写进文档 + 一条 Undo"的提交边界住在面板外。搬出前后行为一字未改：
//! Play 中一律拒绝、无净变化不产生历史、失败只返回错误字符串（由面板写状态栏）。
//! M27-01 起拖放生成走 `spawn_dropped_batch`：一次拖放无论几个文件都只有**一条** Undo。

use crate::EditorApp;
use crate::util::strip_verbatim_prefix;
use editor::{CommandKind, SceneTreeState};
use rustlua_assets::resource_kind::ResourceKind;
use rustlua_ecs::transform::{Transform3D, Vec2};
use rustlua_render::texture::{TextureAsset, TextureColorSpace};
use rustlua_scene::prefab::Prefab;
use rustlua_scene::scene::SceneEntityId;
use std::path::Path;

impl EditorApp {
    /// 把面板 `TilemapEditorState` 写回实体组件（一次调用 = 单条 Undo；无净变化
    /// 不产生历史）。
    pub(crate) fn commit_tilemap_paint_labelled(&mut self, entity_id: SceneEntityId, label: &str) {
        if self.is_playing() {
            return;
        }
        let tilemap = self.tilemap.map.clone();
        if let Some(scene) = self.document.current_scene.as_mut() {
            let _ = self
                .history
                .execute(scene, label, CommandKind::Property, move |scene| {
                    if let Some(entity) = scene.scene.entities.get_mut(&entity_id) {
                        entity
                            .components
                            .insert("Tilemap".to_owned(), tilemap.to_component_str());
                        scene.mark_dirty();
                    }
                    Ok::<(), ()>(())
                });
        }
    }

    /// 从拖放的资源路径**批量**建实体：一次拖放 = 恰好一条 Undo。
    ///
    /// 三种语义与迁移前的 `spawn_prefab_from_drop` / `spawn_model_from_drop` /
    /// `spawn_sprite_from_drop` 一一对应（预制体→实例化并打 prefab.* 标记、模型→
    /// Transform3D+MeshRenderer、其余→Transform+Sprite）。差别有两点：
    /// ① 整批只走一次 `history.execute`，撤销一步回到拖放前，不会"撤一半"；
    /// ② 需要读盘的（预制体解析、源纹理尺寸）在动手之前全部读完，任一文件读不动
    ///    就整批拒绝，文档零改动——不存在"前三个建好了第四个失败"的中间态。
    /// `parent` 只影响新建实体的挂点（层级面板按落点带传父节点，画布传 `None`）。
    /// 返回状态栏文案（本层不写 `self.status`）。
    pub(crate) fn spawn_dropped_batch(
        &mut self,
        paths: &[String],
        parent: Option<SceneEntityId>,
    ) -> String {
        if self.is_playing() {
            return "播放模式下不能编辑场景，请先点击停止".to_owned();
        }
        if paths.is_empty() {
            return "没有可创建的资产".to_owned();
        }
        if self.document.current_scene.is_none() {
            return "请先创建场景".to_owned();
        }
        let root = strip_verbatim_prefix(&self.project.root);
        let mut specs = Vec::with_capacity(paths.len());
        for path in paths {
            match SpawnSpec::prepare(&root, path) {
                Ok(spec) => specs.push(spec),
                Err(error) => return error,
            }
        }
        let label = if specs.len() == 1 {
            specs[0].label().to_owned()
        } else {
            "放置资源".to_owned()
        };
        let tree = self.tree.clone();
        let created = std::cell::RefCell::new(Vec::new());
        let Some(scene) = self.document.current_scene.as_mut() else {
            return "请先创建场景".to_owned();
        };
        let result = self
            .history
            .execute(scene, label, CommandKind::Create, |scene| {
                for spec in &specs {
                    let id = spec.apply(&tree, scene, parent)?;
                    created.borrow_mut().push(id);
                }
                Ok::<(), String>(())
            });
        let created = created.into_inner();
        match result {
            Err(error) => error,
            Ok(false) => "没有变化".to_owned(),
            Ok(true) => {
                let Some(&first) = created.first() else {
                    return "没有可创建的资产".to_owned();
                };
                // 整批都选中，Inspector 停在第一个：拖完 20 张图能立刻框住全批改属性。
                if created.len() == 1 {
                    self.selection
                        .set_single(first, editor::SelectionSource::SceneView2D);
                } else {
                    self.selection.set_many(
                        created.iter().copied(),
                        Some(first),
                        editor::SelectionSource::SceneView2D,
                    );
                }
                self.sync_inspector(first);
                format!("已创建 {} 个实体", created.len())
            }
        }
    }
}

/// 一个待创建实体的**预备**结果：所有需要读盘的信息都在这里，创建过程本身纯内存。
enum SpawnSpec {
    Prefab { path: String, prefab: Prefab },
    Model { path: String },
    Sprite { path: String, size: Vec2 },
}

impl SpawnSpec {
    /// 资源类型按扩展名判定（`ResourceKind::from_path` 是工程内唯一口径）。
    fn prepare(root: &Path, path: &str) -> Result<Self, String> {
        Ok(match ResourceKind::from_path(Path::new(path)) {
            ResourceKind::Prefab => Self::Prefab {
                path: path.to_owned(),
                prefab: Prefab::load(root.join(path)).map_err(|error| error.to_string())?,
            },
            ResourceKind::Model => Self::Model {
                path: path.to_owned(),
            },
            _ => {
                // 精灵尺寸按源纹理实际像素生成，编辑器包围框与显示一致；读不到回退 32×32。
                let size = TextureAsset::load_png(root.join(path), TextureColorSpace::Srgb)
                    .map(|asset| Vec2::new(asset.width() as f32, asset.height() as f32))
                    .unwrap_or(Vec2::new(32.0, 32.0));
                Self::Sprite {
                    path: path.to_owned(),
                    size,
                }
            }
        })
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Prefab { .. } => "实例化预制体",
            Self::Model { .. } => "创建模型",
            Self::Sprite { .. } => "创建精灵",
        }
    }

    fn name(&self) -> String {
        let fallback = match self {
            Self::Prefab { .. } => "预制体",
            Self::Model { .. } => "模型",
            Self::Sprite { .. } => "精灵",
        };
        Path::new(self.path())
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| fallback.to_owned())
    }

    fn path(&self) -> &str {
        match self {
            Self::Prefab { path, .. } | Self::Model { path } | Self::Sprite { path, .. } => path,
        }
    }

    /// 在文档里真正建出实体，返回**新建的根**实体 id（预制体实例化出的整棵子树算一个）。
    fn apply(
        &self,
        tree: &SceneTreeState,
        scene: &mut editor::SceneDocument,
        parent: Option<SceneEntityId>,
    ) -> Result<SceneEntityId, String> {
        match self {
            Self::Prefab { prefab, path } => {
                let overrides = std::collections::BTreeMap::new();
                let instance = prefab
                    .instantiate(&mut scene.scene, parent, &overrides)
                    .map_err(|error| error.to_string())?;
                for (source_id, entity_id) in &instance.source_to_instance {
                    if let Some(entity) = scene.scene.entities.get_mut(entity_id) {
                        entity
                            .metadata
                            .insert("prefab.source".to_owned(), path.to_owned());
                        entity
                            .metadata
                            .insert("prefab.source_id".to_owned(), source_id.0.to_string());
                        entity.metadata.insert(
                            "prefab.root".to_owned(),
                            (entity.id == instance.instance_root).to_string(),
                        );
                    }
                }
                scene.mark_dirty();
                Ok(instance.instance_root)
            }
            Self::Model { path } => {
                let id = tree
                    .create_entity(scene, self.name(), parent)
                    .map_err(|error| error.to_string())?;
                if let Some(entity) = scene.scene.entities.get_mut(&id) {
                    entity.components.insert(
                        "Transform".into(),
                        Transform3D::default().to_component_str(),
                    );
                    entity
                        .components
                        .insert("MeshRenderer".into(), format!("mesh={path}"));
                }
                Ok(id)
            }
            Self::Sprite { path, size } => {
                let id = tree
                    .create_entity(scene, self.name(), parent)
                    .map_err(|error| error.to_string())?;
                if let Some(entity) = scene.scene.entities.get_mut(&id) {
                    entity.components.insert(
                        "Transform".into(),
                        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
                    );
                    let sprite = rustlua_render::sprite::SpriteComponent {
                        texture: path.to_owned(),
                        size: *size,
                        pivot: rustlua_ecs::transform::Vec2::new(0.5, 0.5),
                        tint: [1.0; 4],
                        sorting: rustlua_render::sprite::SpriteSorting::default(),

                        flip_x: false,
                        flip_y: false,
                        z: 0.0,
                        nine_slice: None,
                    };
                    entity
                        .components
                        .insert("Sprite".into(), sprite.to_component_str());
                }
                Ok(id)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::EditorApp;
    use crate::util::test_token;
    use editor::{CommandResult, EditorCommand, ProjectInfo, SceneDocument};
    use rustlua_infra::logging::LogSink;
    use rustlua_scene::scene::SceneEntityId;

    /// 一个只带根实体的临时工程：精灵拖放不需要磁盘上的文件（读不到尺寸时回退 32×32），
    /// 所以这些用例能在纯内存条件下成立。
    fn app_with_scene() -> (EditorApp, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("rustlua-scene-ops-{}", test_token()));
        let mut app = EditorApp::new(
            ProjectInfo {
                root: root.clone(),
                version: "test".to_owned(),
                directories: Vec::new(),
            },
            None,
            LogSink::new(64),
        );
        let mut document = SceneDocument::new_scene("main");
        document
            .scene
            .add_entity(SceneEntityId(1), "Root", None)
            .unwrap();
        app.document.current_scene = Some(document);
        app.project_loaded = true;
        (app, root)
    }

    fn entity_count(app: &EditorApp) -> usize {
        app.document
            .current_scene
            .as_ref()
            .unwrap()
            .scene
            .entities
            .len()
    }

    fn children_of(app: &EditorApp, parent: SceneEntityId) -> Vec<SceneEntityId> {
        let document = app.document.current_scene.as_ref().unwrap();
        let mut ids: Vec<SceneEntityId> = document
            .scene
            .entities
            .iter()
            .filter(|(_, entity)| entity.parent == Some(parent))
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids
    }

    /// 一次拖放两个资产 = **恰好一条** Undo，两个实体都挂到落点的父节点下且都被选中；
    /// 撤销一步整批消失（不存在"撤一半"的中间态）。
    #[test]
    fn a_two_asset_drop_is_one_undo_that_reverts_as_a_whole() {
        let (mut app, root) = app_with_scene();
        let before = entity_count(&app);
        let text = app.spawn_dropped_batch(
            &["assets/a.png".to_owned(), "assets/b.png".to_owned()],
            Some(SceneEntityId(1)),
        );
        assert_eq!(text, "已创建 2 个实体");
        assert_eq!(entity_count(&app), before + 2);
        assert_eq!(app.history.undo_len(), 1, "整批只能有一条历史");
        assert_eq!(children_of(&app, SceneEntityId(1)).len(), 2);
        assert_eq!(app.selection.ids().len(), 2, "拖完要能立刻全批改属性");
        assert_eq!(app.dispatch(EditorCommand::Undo), CommandResult::Applied);
        assert_eq!(entity_count(&app), before);
        assert!(children_of(&app, SceneEntityId(1)).is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    /// 批次里有一个预制体读不动 ⇒ **整批**在动手之前就被拒绝：文档零改动、历史零增长。
    #[test]
    fn one_unreadable_asset_refuses_the_whole_batch() {
        let (mut app, root) = app_with_scene();
        let before = entity_count(&app);
        let text = app.spawn_dropped_batch(
            &[
                "assets/a.png".to_owned(),
                "assets/does-not-exist.prefab".to_owned(),
            ],
            None,
        );
        assert!(!text.starts_with("已创建"), "必须报告失败：{text}");
        assert_eq!(entity_count(&app), before, "拒绝时不能留下半个批次");
        assert_eq!(app.history.undo_len(), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    /// Play 模式是只读预览：拖放必须在**碰文档之前**就被拒绝。
    #[test]
    fn play_mode_refuses_a_drop_without_touching_the_document() {
        let (mut app, root) = app_with_scene();
        app.play.set_state_for_test(editor::PlayState::Playing);
        let before = entity_count(&app);
        let text = app.spawn_dropped_batch(&["assets/a.png".to_owned()], None);
        assert!(text.contains("播放模式下"), "文案要说清为什么不行：{text}");
        assert_eq!(entity_count(&app), before);
        assert_eq!(app.history.undo_len(), 0);
        let _ = std::fs::remove_dir_all(root);
    }
}
