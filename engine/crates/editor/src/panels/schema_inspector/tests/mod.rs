use super::{FieldWidget, field_widget};
use crate::EditorApp;
use editor::{CommandResult, EditorCommand, PlaySession, ProjectInfo, SceneDocument};
use rustlua_ecs::component_schema::FieldKind;
use rustlua_infra::logging::LogSink;
use rustlua_scene::scene::SceneEntityId;

pub(super) fn app_with_quad() -> EditorApp {
    let mut app = EditorApp::new(
        ProjectInfo {
            root: std::env::temp_dir().join(format!(
                "rustlua-schema-inspector-{}",
                crate::util::test_token()
            )),
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
    document
        .scene
        .entities
        .get_mut(&SceneEntityId(1))
        .unwrap()
        .components
        .insert("Quad".to_owned(), "size=64,32;color=1,1,1,1".to_owned());
    app.document.current_scene = Some(document);
    app.project_loaded = true;
    app.selection
        .set_single(SceneEntityId(1), editor::SelectionSource::Hierarchy);
    // 真实 UI 每帧都会把 Inspector 缓冲与文档对齐；测试也要对齐，否则
    // `Undo` 的帧末收敛边界会把"从未同步过的 Inspector 缓冲"先提交成一条历史，
    // 于是单次 Undo 撤销的是那条而不是本用例的字段编辑。
    app.sync_inspector(SceneEntityId(1));
    app
}

mod migrated;
mod references;
mod widgets;
