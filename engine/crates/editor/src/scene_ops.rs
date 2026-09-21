//! Scene View 的**提交层（之二）**：Tile 绘制落盘与拖放生成实体
//! （docs/55 §4.1 规则 1：View 只发 typed action）。
//!
//! 与 `scene_drag.rs` 同一分工：面板负责绘制、命中测试、资源拖放识别与参数收集；
//! "写进文档 + 一条 Undo"的提交边界住在面板外。搬出前后行为一字未改：
//! Play 中一律拒绝、无净变化不产生历史、失败只返回错误字符串（由面板写状态栏）。

use crate::EditorApp;
use crate::util::strip_verbatim_prefix;
use editor::CommandKind;
use engine::core::{
    prefab::Prefab,
    scene::SceneEntityId,
    texture::{TextureAsset, TextureColorSpace},
    transform::{Transform3D, Vec2},
};
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

    /// 从拖放的预制体路径实例化到场景。
    pub(crate) fn spawn_prefab_from_drop(&mut self, path: &str) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        let root = strip_verbatim_prefix(&self.project.root);
        let prefab = Prefab::load(root.join(path)).map_err(|error| error.to_string())?;
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let overrides = std::collections::BTreeMap::new();
        let result = self.history.execute(
            scene,
            "Instantiate Prefab",
            CommandKind::Create,
            move |scene| {
                let instance = prefab
                    .instantiate(&mut scene.scene, None, &overrides)
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
                Ok::<(), String>(())
            },
        );
        match result {
            Ok(_) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// 从拖放的模型路径创建 3D 网格实体（Transform3D + MeshRenderer）。
    pub(crate) fn spawn_model_from_drop(&mut self, path: &str) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        if self.document.current_scene.is_none() {
            return Err("请先创建场景".to_owned());
        }
        let name = Path::new(path)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Model".to_owned());
        let tree = self.tree.clone();
        let path = path.to_owned();
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        let result =
            self.history
                .execute(scene, "Create Model", CommandKind::Create, move |scene| {
                    let id = tree
                        .create_entity(scene, name.clone(), None)
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
                    Ok::<(), String>(())
                });
        match result {
            Ok(_) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// 从拖放的资源路径创建 2D 精灵实体（Transform + Sprite）。
    pub(crate) fn spawn_sprite_from_drop(&mut self, path: &str) -> Result<(), String> {
        if self.is_playing() {
            return Err("Play mode locks scene editing; click Stop first".to_owned());
        }
        if self.document.current_scene.is_none() {
            return Err("请先创建场景".to_owned());
        }
        let name = Path::new(path)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Sprite".to_owned());
        let tree = self.tree.clone();
        let path = path.to_owned();
        let root = strip_verbatim_prefix(&self.project.root);
        let Some(scene) = self.document.current_scene.as_mut() else {
            return Ok(());
        };
        // 记下新建实体的 id：拖放之后要把它**选中**（docs/55 阶段 0 的 GUI smoke 用例
        // `dnd/dropped_entity_is_named_and_selected` 抓到的缺口）。
        //
        // 其余创建入口（层级面板的「+」/`Ctrl+D`/Inspector 添加组件）都把新实体设为选中，
        // 只有拖放这条路径漏了——用户拖完资源，Inspector 还停在旧对象上，得再去层级面板点一次。
        let mut created: Option<SceneEntityId> = None;
        let result = self
            .history
            .execute(scene, "Create Sprite", CommandKind::Create, |scene| {
                let id = tree
                    .create_entity(scene, name.clone(), None)
                    .map_err(|error| error.to_string())?;
                if let Some(entity) = scene.scene.entities.get_mut(&id) {
                    entity.components.insert(
                        "Transform".into(),
                        "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
                    );
                    // 新建精灵按源纹理实际像素尺寸生成，编辑器包围框与实际显示一致；
                    // 纹理读取失败时回退到默认 32×32。
                    let size = TextureAsset::load_png(root.join(&path), TextureColorSpace::Srgb)
                        .map(|asset| Vec2::new(asset.width() as f32, asset.height() as f32))
                        .unwrap_or_else(|_| Vec2::new(32.0, 32.0));
                    let sprite = engine::core::sprite::SpriteComponent {
                        texture: path.clone(),
                        size,
                        pivot: engine::core::transform::Vec2::new(0.5, 0.5),
                        tint: [1.0; 4],
                        sorting: engine::core::sprite::SpriteSorting::default(),

                        flip_x: false,
                        flip_y: false,
                        z: 0.0,
                    };
                    entity
                        .components
                        .insert("Sprite".into(), sprite.to_component_str());
                }
                created = Some(id);
                Ok::<(), String>(())
            });
        match result {
            Ok(_) => {
                if let Some(id) = created {
                    self.selection
                        .set_single(id, editor::SelectionSource::SceneView2D);
                    self.sync_inspector(id);
                    self.status = format!("已从资源创建精灵：{id:?}");
                }
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
}
