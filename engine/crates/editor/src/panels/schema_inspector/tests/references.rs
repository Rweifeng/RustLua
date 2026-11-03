use super::app_with_quad;

// ---- M27-01：资源引用字段的落点结算 ----

/// 三种结算各管各的：自家资产**只交出一个值**（写文档仍走 `pending` 那条 dispatch）；
/// 工程外的文件**先入库**再交出相对路径；拒绝没有值、只把原因写进状态栏，磁盘与文档都不动。
#[test]
fn a_reference_field_drop_yields_a_value_or_a_reason() {
    use crate::dnd::DropAction;
    let mut app = app_with_quad();
    assert_eq!(
        app.commit_field_ref_drop(
            &DropAction::SetAssetRef("assets/textures/a.png".to_owned()),
            String::new(),
        )
        .as_deref(),
        Some("assets/textures/a.png")
    );

    let name = format!("rustlua-inspector-drop-{}.png", crate::util::test_token());
    let outside = std::env::temp_dir().join(&name);
    std::fs::write(&outside, b"png").expect("写外部文件");
    let value =
        app.commit_field_ref_drop(&DropAction::ImportAndSetRef(outside.clone()), String::new());
    assert_eq!(
        value.as_deref(),
        Some(format!("assets/textures/{name}").as_str()),
        "外部文件要先复制进家族目录，字段写的是工程相对路径：{}",
        app.status
    );
    assert!(
        app.project
            .root
            .join("assets/textures")
            .join(&name)
            .is_file(),
        "文件必须真的入库"
    );
    assert!(
        app.status.starts_with("已导入 1 个文件"),
        "状态={}",
        app.status
    );

    let entities = app
        .document
        .current_scene
        .as_ref()
        .unwrap()
        .scene
        .entities
        .len();
    assert_eq!(
        app.commit_field_ref_drop(
            &DropAction::Reject("这个字段要的是纹理/模型/预制体".to_owned()),
            "（跳过 1 个链接）".to_owned(),
        ),
        None
    );
    assert_eq!(
        app.status, "这个字段要的是纹理/模型/预制体（跳过 1 个链接）",
        "拒绝必须带上损耗说明"
    );
    assert_eq!(
        app.document
            .current_scene
            .as_ref()
            .unwrap()
            .scene
            .entities
            .len(),
        entities
    );
    assert_eq!(app.history.undo_len(), 0, "落点结算本身不写历史");
    let _ = std::fs::remove_file(&outside);
    let _ = std::fs::remove_dir_all(&app.project.root);
}
