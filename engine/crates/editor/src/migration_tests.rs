//! docs/54 §8-⑧：迁移命令的**报告 / 退出码门 / 弃用告警 / 失败路径**。
//!
//! 这些是不需要 egui 的纯数据层测试，因此单独一个模块：命令的输出就是质量门要断言的契约
//! （`spatial_migration_ok` / `refused=` / `removed=`），而不是"跑一下看看"。
//! 从 `main.rs` 原样搬出（P1 结构收口），测试名不变。

use super::*;
use rustlua_scene::scene::Scene;

fn temp_project(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rustlua-editor-migration-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(dir.join("scenes")).expect("create project");
    dir
}

/// 父缩放 + 子按世界坐标写：迁移应当把子转成局部（`converted=1`）。
/// 另外塞一个带旧内联几何的 `Text`：迁移报告必须点名它（docs/30 的弃用告警）。
fn project_scene(with_legacy_text: bool) -> Scene {
    let mut scene = Scene::new("hierarchy");
    let parent = SceneEntityId(1);
    let child = SceneEntityId(2);
    scene.add_entity(parent, "Parent", None).unwrap();
    scene.add_entity(child, "Child", Some(parent)).unwrap();
    scene.entities.get_mut(&parent).unwrap().components.insert(
        "Transform".into(),
        "position=10,0,0;rotation=0,0,0,1;scale=2,2,1".into(),
    );
    scene.entities.get_mut(&child).unwrap().components.insert(
        "Transform".into(),
        "position=3,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    if with_legacy_text {
        let hud = SceneEntityId(3);
        scene.add_entity(hud, "Hud", None).unwrap();
        scene.entities.get_mut(&hud).unwrap().components.insert(
            "Text".into(),
            "text=hi;font_size=12;color=1,1,1;x=8;y=8".into(),
        );
    }
    scene
}

#[test]
fn dry_run_report_is_lossless_and_names_removed_designs() {
    let root = temp_project("dry-run");
    project_scene(true)
        .save(root.join("scenes/main.scene"))
        .expect("save scene");
    let (report, blocked) = run_spatial_migration(&root, false, false);
    assert!(!blocked, "没有拒绝时不得阻断：{report}");
    assert!(report.contains("spatial_migration_ok"), "{report}");
    assert!(report.contains("converted=1"), "{report}");
    assert!(report.contains("refused=0"), "{report}");
    assert!(report.contains("removed=2"), "两条旧内联几何：{report}");
    assert!(
        report.contains("removed\tscenes/main.scene\tentity=3\tText.x\t→ UiLayout"),
        "弃用告警必须点名组件、字段与去处：{report}"
    );
    assert!(report.contains("mode=dry-run"), "{report}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn write_mode_reports_verified_backups_and_is_idempotent() {
    let root = temp_project("write");
    let scene_path = root.join("scenes/main.scene");
    project_scene(false).save(&scene_path).expect("save scene");
    let (report, blocked) = run_spatial_migration(&root, true, false);
    assert!(!blocked, "{report}");
    assert!(report.contains("written\tscenes/main.scene"), "{report}");
    assert!(report.contains("verified=true"), "{report}");
    assert!(report.contains("mode=write"), "{report}");
    assert!(scene_path.with_extension("scene.bak").is_file());
    // 幂等标记在场景自身里（与组件数据同一次原子写）。
    let written = std::fs::read_to_string(&scene_path).expect("read written scene");
    assert!(written.contains("meta\tspatial\tlocal"), "{written}");

    let (again, blocked) = run_spatial_migration(&root, true, false);
    assert!(!blocked, "{again}");
    assert!(again.contains("already-migrated"), "{again}");
    assert!(again.contains("already_migrated=1"), "{again}");
    assert_eq!(
        std::fs::read_to_string(&scene_path).expect("read again"),
        written,
        "重复运行不得改动文件"
    );

    // 回滚：内容与标记一起回到原样。
    let (restore, blocked) = run_spatial_migration(&root, false, true);
    assert!(!blocked, "{restore}");
    assert!(restore.contains("restored\tscenes/main.scene"), "{restore}");
    assert!(restore.contains("mode=restore"), "{restore}");
    assert!(
        !std::fs::read_to_string(&scene_path)
            .expect("read restored")
            .contains("meta\tspatial\tlocal")
    );
    assert!(!scene_path.with_extension("scene.bak").exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// 有实体无法无损迁移时：报告行变成 `spatial_migration_blocked` 且调用方拿到 `blocked = true`
/// （`main` 据此 `exit(2)`，让脚本能把它当质量门）。报告**仍然完整**，不是静默失败。
#[test]
fn refused_entities_block_the_gate_and_are_reported() {
    let root = temp_project("refused");
    let mut scene = Scene::new("shear");
    let parent = SceneEntityId(1);
    let child = SceneEntityId(2);
    scene.add_entity(parent, "Parent", None).unwrap();
    scene.add_entity(child, "Child", Some(parent)).unwrap();
    scene.entities.get_mut(&parent).unwrap().components.insert(
        "Transform".into(),
        "position=0,0,0;rotation=0,0,0,1;scale=2,1,1".into(),
    );
    scene.entities.get_mut(&child).unwrap().components.insert(
        "Transform".into(),
        "position=4,0,0;rotation=0,0,0.38268343,0.92387953;scale=1,1,1".into(),
    );
    scene
        .save(root.join("scenes/shear.scene"))
        .expect("save scene");
    let (report, blocked) = run_spatial_migration(&root, false, false);
    assert!(blocked, "被拒实体必须阻断：{report}");
    assert!(report.contains("spatial_migration_blocked"), "{report}");
    assert!(report.contains("refused=1"), "{report}");
    let _ = std::fs::remove_dir_all(&root);
}

/// 报告 artifact：给定路径就落盘（含自动建目录），内容与终端输出逐字节一致。
#[test]
fn migration_report_is_written_to_the_requested_path() {
    let root = temp_project("report");
    project_scene(false)
        .save(root.join("scenes/main.scene"))
        .expect("save scene");
    let (report, _) = run_spatial_migration(&root, false, false);
    let report_path = root.join("artifacts/baselines/spatial-migration.txt");
    write_migration_report(&report_path, &report).expect("write report");
    assert_eq!(
        std::fs::read_to_string(&report_path).expect("read report"),
        report,
        "落盘的报告必须与终端输出一致（否则归档的证据不是判定用的那一份）"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// 空工程也不能静默通过：没有场景可处理时报 `blocked`，否则质量门会把"扫了个寂寞"当成功。
#[test]
fn empty_project_is_blocked() {
    let root = temp_project("empty");
    let (report, blocked) = run_spatial_migration(&root, false, false);
    assert!(blocked, "没有场景必须阻断：{report}");
    assert!(report.contains("没有找到"), "{report}");
    let _ = std::fs::remove_dir_all(&root);
}
