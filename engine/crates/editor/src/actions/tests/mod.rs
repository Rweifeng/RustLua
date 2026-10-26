//! `actions` 的回归测试：文档切换闸门（E06）、编辑事务与撤销（E07）、
//! Inspector 脏编辑（E08）、统一刷新路由，以及布局/预设辅助函数。
//!
//! 按主题分家：`document_gate` / `transactions` / `layout` / `refresh` / `prefab_stage`
//! （P1-4 in-context 预制体编辑）/ `prefab_overrides`（P2 整实例覆盖视图与逐项还原）/
//! `broken_refs`（P1-3b 断链审计）；本文件只放
//! 各主题共用的导入与辅助函数（临时工程、样例工程拷贝、Scene 视图缓存探针）。

use super::*;
use crate::{EditorApp, EditorCamera, SceneViewCache2d, SceneViewCache3d};
use editor::open_project;
use rustlua_ecs::transform::Transform3D;
use rustlua_ecs::transform::Vec2;
use rustlua_infra::logging::LogSink;
use std::{
    fs,
    path::{Path, PathBuf},
};
fn unique_token() -> u128 {
    // 进程内唯一：避免并行测试临时目录互撞（详见 editor::test_token）。
    crate::util::test_token()
}

fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("m15-action-{label}-{}", unique_token()))
}

fn sample_project_root() -> PathBuf {
    rustlua_infra::games_dir().join("examples").join("game_api")
}

fn copy_dir(source: &Path, destination: &Path) {
    if !source.is_dir() {
        return;
    }
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let source_path = entry.path();
        let target_path = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            fs::create_dir_all(&target_path).unwrap();
            copy_dir(&source_path, &target_path);
        } else {
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::copy(&source_path, &target_path).unwrap();
        }
    }
}

fn copy_sample_project(label: &str) -> PathBuf {
    let source = sample_project_root();
    assert!(
        source.join("project.toml").is_file(),
        "sample project missing: {}",
        source.display()
    );
    let root = temp_root(label);
    fs::create_dir_all(&root).unwrap();
    fs::copy(source.join("project.toml"), root.join("project.toml")).unwrap();
    copy_dir(&source.join("scripts"), &root.join("scripts"));
    copy_dir(&source.join("scenes"), &root.join("scenes"));
    copy_dir(&source.join("assets"), &root.join("assets"));
    root
}

// E02/E07 回归测试：使用仓库内 examples 工程（project.toml +
// scripts/scenes；不复制巨大的 assets，也绝不调用 refresh_resources）。
fn copy_examples_project(label: &str) -> PathBuf {
    let source = sample_project_root();
    assert!(
        source.join("project.toml").is_file(),
        "examples 工程缺失: {}",
        source.display()
    );
    assert!(
        source.join("scenes/main.scene").is_file(),
        "examples 缺少 scenes/main.scene"
    );
    let root = temp_root(label);
    fs::create_dir_all(&root).unwrap();
    fs::copy(source.join("project.toml"), root.join("project.toml")).unwrap();
    copy_dir(&source.join("scripts"), &root.join("scripts"));
    copy_dir(&source.join("scenes"), &root.join("scenes"));
    root
}

/// 打开示例工程副本并打开 **Prefab 实例场景** `scenes/api_prefab.scene`。
///
/// 与 inspector tests 的 `sample_app_with_production_scene` 指向同一个场景：
/// 它带两个 Prefab 源（slime_green / slime_red）的真实实例，Apply / Revert / 逐字段
/// Override 这些路径都需要"实例实体上真的有 `prefab.source` 元数据"才测得动。
/// （拷贝 assets/scenes 后不依赖资源索引，Prefab 按相对路径加载。）
fn open_sample_production(label: &str) -> (EditorApp, PathBuf) {
    let root = copy_sample_project(label);
    let project = open_project(&root).expect("production 副本应可打开");
    let mut app = EditorApp::new(project, None, LogSink::new(64));
    app.open_scene_resource(PathBuf::from("scenes/api_prefab.scene"));
    (app, root)
}

/// 打开示例工程副本并打开 2D 综合场景 `scenes/main.scene`。
///
/// 场景名与实体结构见 `examples/game_api/README.md`；断言**不写死实体数量**，
/// 而是在运行期从文档里取（示例工程的场景会随演示增删而变，写死数量只会制造假失败）。
fn open_examples_main(label: &str) -> (EditorApp, PathBuf) {
    let root = copy_examples_project(label);
    let project = open_project(&root).expect("examples 副本应可打开");
    let mut app = EditorApp::new(project, None, LogSink::new(64));
    app.open_scene_resource(PathBuf::from("scenes/main.scene"));
    (app, root)
}

/// 文档里第一个**没有父实体**的实体 id（删除/撤销这类测试需要一个真有意义的根实体）。
fn first_root_entity(app: &EditorApp) -> SceneEntityId {
    app.document
        .current_scene
        .as_ref()
        .expect("已打开场景")
        .scene
        .entities
        .values()
        .find(|entity| entity.parent.is_none())
        .expect("示例场景应当有根实体")
        .id
}

/// 制造一份“上次文档”遗留的陈旧状态：运行时 revision、2D/3D 缓存、
/// 预览变换、画面纹理 key、Hierarchy 元数据 key。
fn poison_scene_view_caches(app: &mut EditorApp) {
    app.scene_runtime_revision = Some(0);
    app.scene_view_cache_2d = SceneViewCache2d {
        revision: Some(0),
        size: (320, 180),
        camera: (eframe::egui::vec2(12.0, -8.0), 1.5),
    };
    app.scene_view_cache_3d = SceneViewCache3d {
        revision: Some(0),
        size: (640, 360),
        camera: EditorCamera::default(),
    };
    app.scene_state
        .preview
        .insert(SceneEntityId(1), TransformFields::default());
    app.scene_state
        .preview3d_transform
        .insert(SceneEntityId(1), Transform3D::default());
    app.preview_diagnostic = Some("stale frame".to_owned());
    app.scene_screen_cache_key = Some((0, (1280, 720)));
    app.scene_screen_tex_id = Some(eframe::egui::TextureId::User(0));
    app.last_metadata_key = Some((Some(PathBuf::from("stale/path")), 5));
}

fn assert_scene_view_and_metadata_invalidated(app: &EditorApp) {
    assert_eq!(app.scene_runtime_revision, None);
    assert_eq!(app.scene_view_cache_2d.revision, None);
    assert_eq!(app.scene_view_cache_2d.size, (0, 0));
    assert_eq!(app.scene_view_cache_3d.revision, None);
    assert_eq!(app.scene_view_cache_3d.size, (0, 0));
    assert!(app.scene_state.preview.is_empty());
    assert!(app.scene_state.preview3d_transform.is_empty());
    assert_eq!(app.preview_diagnostic, None);
    assert_eq!(app.scene_screen_cache_key, None);
    assert_eq!(app.scene_screen_tex_id, None);
    assert!(app.scene_view_repaint_pending);
    assert_eq!(app.last_metadata_key, None);
}

/// 弹窗决策仅在 QuitEditor 路径会向原生窗口发送 ViewportCommand；
/// OpenResource 等路径不依赖真实窗口，测试用裸 Context 即可驱动。
fn test_context() -> egui::Context {
    egui::Context::default()
}

mod broken_refs;
mod document_gate;
mod layout;
mod prefab_overrides;
mod prefab_stage;
mod refresh;
mod transactions;
