//! WP2/WP9：场景空间层级**迁移命令**的数据层实现（dry-run / 写回 / 备份 / 一键回滚）。
//!
//! 负责人裁定"所有场景的 `parent` 一律按局部空间解释"。既有场景里子实体的坐标是**世界**坐标，
//! 直接按新语义解释会让整棵子树位移，因此必须先迁移：把"父世界矩阵的逆"烘进子实体的局部变换，
//! 世界位置保持不变（见 [`crate::core::spatial_transform::migrate_scene_to_local_space`]）。
//!
//! 写回是**危险操作**，所以：默认 dry-run（只报告不写盘）；写回前先备份成 `<file>.bak`
//! （已存在则不覆盖，保证备份永远是"最原始"的那份）；`restore` 把 `.bak` 拷回去并删掉它，
//! 即一键回滚。无法无损建立继承的实体会被拒绝、**清掉 `parent`** 并逐条报告。

use std::path::{Path, PathBuf};

use crate::core::{scene::Scene, spatial_transform::migrate_scene_to_local_space};

/// 迁移选项。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MigrationOptions {
    /// 真正写盘（默认 `false` = dry-run）。
    pub write: bool,
    /// 写盘前备份 `<file>.bak`（默认 `true`；`write` 为假时无意义）。
    pub backup: bool,
}

/// 一个场景文件的迁移结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneMigrationOutcome {
    pub path: PathBuf,
    /// 场景里的实体总数。
    pub entities: usize,
    /// 成功迁移为"相对父"的实体数。
    pub converted: usize,
    /// 被拒绝的实体数（无法无损表达）：它们的 `parent` 会被清掉，世界位置不变。
    pub refused: usize,
    /// 无变换 / 父无变换而跳过的实体数。
    pub skipped: usize,
    /// 是否真的写了盘。
    pub written: bool,
    /// 备份文件路径（写了备份才有）。
    pub backup: Option<PathBuf>,
}

impl SceneMigrationOutcome {
    /// 是否无损（没有实体被拒绝）。
    #[must_use]
    pub const fn is_lossless(&self) -> bool {
        self.refused == 0
    }
}

/// 迁移一个场景文件。`options.write` 为假时只读盘、只报告（dry-run）。
pub fn migrate_scene_file(
    path: &Path,
    options: MigrationOptions,
) -> Result<SceneMigrationOutcome, String> {
    // 迁移**不可重复施加**（数据在迁移后已经是局部坐标，再按"局部=世界"解释一次会把父变换
    // 又乘一遍）。因此写回时留下标记文件，`write` 模式下看到标记就跳过——命令因此可以安全地
    // 反复运行，不需要用户记住"这个场景迁过没有"。
    if options.write && marker_path(path).is_file() {
        let scene = Scene::load(path).map_err(|error| format!("{}: {error}", path.display()))?;
        return Ok(SceneMigrationOutcome {
            path: path.to_path_buf(),
            entities: scene.entities.len(),
            converted: 0,
            refused: 0,
            skipped: 0,
            written: false,
            backup: None,
        });
    }
    let scene = Scene::load(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let (migration, report) = migrate_scene_to_local_space(&scene);
    let mut outcome = SceneMigrationOutcome {
        path: path.to_path_buf(),
        entities: scene.entities.len(),
        converted: report.converted.len(),
        refused: report.refused.len(),
        skipped: report.skipped.len(),
        written: false,
        backup: None,
    };
    if !options.write {
        return Ok(outcome);
    }
    if migration.locals.is_empty() {
        // 没有可写的实体：不碰文件（避免"写一遍再写一遍"的无谓差异）。
        return Ok(outcome);
    }
    // 逐条应用：局部变换 + 被拒实体的 `parent` 清除。
    let mut updated = scene.clone();
    for (id, value) in &migration.locals {
        if let Some(entity) = updated.entities.get_mut(id) {
            entity.components.insert("Transform".into(), value.clone());
        }
    }
    for id in &migration.detached {
        if let Some(entity) = updated.entities.get_mut(id) {
            entity.parent = None;
        }
    }
    if options.backup {
        let backup = backup_path(path);
        if !backup.exists() {
            std::fs::copy(path, &backup)
                .map_err(|error| format!("备份失败 {}: {error}", backup.display()))?;
        }
        outcome.backup = Some(backup);
    }
    updated
        .save(path)
        .map_err(|error| format!("写回失败 {}: {error}", path.display()))?;
    // 标记文件 = "这个场景已经按局部空间写过了"。内容写一行说明，便于用户直接看懂。
    std::fs::write(
        marker_path(path),
        "spatial=local\n# 本场景的 parent 按局部空间解释，已由迁移命令写回（见 docs/40 WP2）。\n\
         # 删除本文件即可让迁移命令重新处理该场景；`--restore` 会连同内容一起回滚。\n",
    )
    .map_err(|error| format!("写入迁移标记失败 {}: {error}", marker_path(path).display()))?;
    outcome.written = true;
    Ok(outcome)
}

/// 迁移标记文件（`<scene>.spatial-local`）。存在即表示该场景已按局部空间写回。
#[must_use]
pub fn marker_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".spatial-local");
    PathBuf::from(name)
}

/// 一键回滚：把 `<file>.bak` 拷回去、删掉备份与迁移标记。返回是否真的回滚了。
pub fn restore_scene_backup(path: &Path) -> Result<bool, String> {
    let backup = backup_path(path);
    if !backup.is_file() {
        return Ok(false);
    }
    std::fs::copy(&backup, path)
        .map_err(|error| format!("回滚失败 {}: {error}", path.display()))?;
    std::fs::remove_file(&backup)
        .map_err(|error| format!("删除备份失败 {}: {error}", backup.display()))?;
    // 标记一并删掉：回滚后该场景又回到"未迁移"状态，迁移命令可以重新处理它。
    let _ = std::fs::remove_file(marker_path(path));
    Ok(true)
}

/// 迁移命令的备份命名约定（与编辑器其它原子写的 `.bak` 约定一致）。
#[must_use]
pub fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".bak");
    PathBuf::from(name)
}

/// 收集项目里所有场景文件（`scenes/**/*.scene`，排序保证报告可复现）。
#[must_use]
pub fn project_scene_files(project_root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_scene_files(&project_root.join("scenes"), &mut files);
    files.sort();
    files
}

fn collect_scene_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_scene_files(&path, files);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "scene")
        {
            files.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::scene::{Scene, SceneEntityId};

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "p2-scene-migration-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// 父缩放的场景：子按世界坐标写。
    fn hierarchy_scene() -> Scene {
        let mut scene = Scene::new("hierarchy");
        let parent = SceneEntityId(1);
        let child = SceneEntityId(2);
        scene.add_entity(parent, "Parent", None).unwrap();
        scene.add_entity(child, "Child", Some(parent)).unwrap();
        scene
            .entities
            .get_mut(&parent)
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "x=10.000000000;y=0.000000000;rotation=0.000000000;scale_x=2.000000000;scale_y=2.000000000".into(),
            );
        scene
            .entities
            .get_mut(&child)
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "x=3.000000000;y=0.000000000;rotation=0.000000000;scale_x=1.000000000;scale_y=1.000000000".into(),
            );
        scene
    }

    fn child_position(scene: &Scene) -> crate::core::transform::Vec2 {
        crate::core::transform::Transform::from_component_str(
            &scene.entities[&SceneEntityId(2)].components["Transform"],
        )
        .expect("child transform")
        .position
    }

    /// dry-run 不写盘；写回把子实体改成局部坐标（世界位置靠继承保持）。
    #[test]
    fn dry_run_reports_then_write_back_converts_child_to_local() {
        let dir = temp_dir("dry-run");
        let path = dir.join("main.scene");
        hierarchy_scene().save(&path).expect("save scene");
        let original = std::fs::read_to_string(&path).expect("read original");

        let dry = migrate_scene_file(&path, MigrationOptions::default()).expect("dry run");
        assert_eq!(dry.converted, 1);
        assert_eq!(dry.refused, 0);
        assert!(!dry.written);
        assert_eq!(
            std::fs::read_to_string(&path).expect("read after dry run"),
            original,
            "dry-run 不得改动文件"
        );

        let written = migrate_scene_file(
            &path,
            MigrationOptions {
                write: true,
                backup: true,
            },
        )
        .expect("write");
        assert!(written.written);
        assert_eq!(
            written.backup.as_deref(),
            Some(backup_path(&path).as_path()),
            "写回必须留下备份"
        );
        let migrated = Scene::load(&path).expect("load migrated");
        assert_eq!(
            child_position(&migrated),
            crate::core::transform::Vec2::new(-3.5, 0.0),
            "子实体应变成相对父的局部坐标"
        );

        // 重复运行安全：标记文件让第二次调用直接跳过（迁移**不可重复施加**——数据已经是局部
        // 坐标，再乘一次父变换会把它挪走）。
        let before_repeat = std::fs::read_to_string(&path).expect("read migrated");
        let again = migrate_scene_file(
            &path,
            MigrationOptions {
                write: true,
                backup: true,
            },
        )
        .expect("second write");
        assert!(!again.written, "已迁移的场景必须跳过");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read after repeat"),
            before_repeat,
            "重复运行不得改动文件"
        );
        assert_eq!(
            child_position(&Scene::load(&path).expect("reload")),
            crate::core::transform::Vec2::new(-3.5, 0.0)
        );
        assert!(marker_path(&path).is_file(), "写回后应留下迁移标记");

        // 一键回滚：回到最原始的内容，备份被删掉。
        assert!(restore_scene_backup(&path).expect("restore"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read restored"),
            original,
            "回滚必须还原出原始内容"
        );
        assert!(!backup_path(&path).exists());
        assert!(!marker_path(&path).exists(), "回滚后标记也要清掉");
        assert!(!restore_scene_backup(&path).expect("no-op restore"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 备份只留"最原始"的一份：连续两次写回不会把已迁移的内容覆盖成备份。
    #[test]
    fn backup_keeps_the_original_after_repeated_writes() {
        let dir = temp_dir("backup");
        let path = dir.join("main.scene");
        hierarchy_scene().save(&path).expect("save scene");
        let original = std::fs::read_to_string(&path).expect("read original");
        let options = MigrationOptions {
            write: true,
            backup: true,
        };
        migrate_scene_file(&path, options).expect("first write");
        migrate_scene_file(&path, options).expect("second write");
        assert_eq!(
            std::fs::read_to_string(backup_path(&path)).expect("read backup"),
            original,
            "备份必须仍是最原始那一份"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 无法无损迁移（含剪切）的实体被拒绝：`parent` 被清掉，世界位置不变。
    #[test]
    fn refused_entities_are_detached_and_keep_their_world_position() {
        let dir = temp_dir("refused");
        let path = dir.join("shear.scene");
        let mut scene = Scene::new("shear");
        let parent = SceneEntityId(1);
        let child = SceneEntityId(2);
        scene.add_entity(parent, "Parent", None).unwrap();
        scene.add_entity(child, "Child", Some(parent)).unwrap();
        scene.entities.get_mut(&parent).unwrap().components.insert(
            "Transform".into(),
            "x=0;y=0;rotation=0;scale_x=2;scale_y=1".into(),
        );
        scene.entities.get_mut(&child).unwrap().components.insert(
            "Transform".into(),
            "x=4;y=0;rotation=45;scale_x=1;scale_y=1".into(),
        );
        scene.save(&path).expect("save scene");
        let outcome = migrate_scene_file(
            &path,
            MigrationOptions {
                write: true,
                backup: false,
            },
        )
        .expect("migrate");
        assert_eq!(outcome.refused, 1);
        assert!(!outcome.is_lossless());
        let migrated = Scene::load(&path).expect("load");
        assert_eq!(
            migrated.entities[&child].parent, None,
            "被拒实体必须清掉 parent（否则新语义下世界位置会变）"
        );
        assert_eq!(
            child_position(&migrated),
            crate::core::transform::Vec2::new(4.0, 0.0),
            "被拒实体保持自己的世界位置"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 默认语义切换的仓库级对拍：**所有随仓库发布的示例场景**在"`parent` 只是组织关系"与
    /// "`parent` 是局部空间"两种解释下，每个实体的世界位置必须相同。这条把"翻默认值不会
    /// 改变现有内容"变成可验证的事实，而不是靠人肉检查 40 个场景文件。
    ///
    /// 任何新场景只要引入"子按世界坐标写"的父子关系，这条测试立刻判红，从而强迫作者先跑
    /// 迁移命令（`editor --project <p> --migrate-spatial-write`）。
    #[test]
    fn shipped_example_scenes_render_the_same_under_both_parent_semantics() {
        let mut checked_scenes = 0usize;
        let mut checked_entities = 0usize;
        for project in ["game_api", "game_jhyt"] {
            let root = crate::core::paths::games_dir()
                .join("examples")
                .join(project);
            for file in project_scene_files(&root) {
                let Ok(scene) = Scene::load(&file) else {
                    continue;
                };
                let mut locals = std::collections::BTreeMap::new();
                let mut parents = std::collections::BTreeMap::new();
                for entity in scene.entities.values() {
                    let Some((_, local)) =
                        crate::core::spatial_transform::scene_entity_local(entity)
                    else {
                        continue;
                    };
                    locals.insert(entity.id, local);
                    if let Some(parent) = entity.parent {
                        parents.insert(entity.id, parent);
                    }
                }
                if locals.is_empty() {
                    continue;
                }
                checked_scenes += 1;
                let globals = crate::core::spatial_transform::propagate_all(&locals, &parents)
                    .expect("scene hierarchy propagates");
                for (id, local) in &locals {
                    let world = globals[id].matrix();
                    let world_translation = crate::core::math3d::Vec3::new(
                        world.columns[3][0],
                        world.columns[3][1],
                        world.columns[3][2],
                    );
                    assert!(
                        (world_translation - local.translation).length() < 1e-3,
                        "{}: 实体 {} 在两种语义下世界位置不同（{:?} vs {:?}）——该场景必须先跑迁移命令",
                        file.display(),
                        id.0,
                        local.translation,
                        world_translation
                    );
                    checked_entities += 1;
                }
            }
        }
        assert!(
            checked_scenes >= 2 && checked_entities >= 10,
            "对拍应覆盖至少两个工程与若干实体，实际 scenes={checked_scenes} entities={checked_entities}"
        );
    }

    #[test]
    fn project_scene_files_are_sorted_and_recursive() {
        let dir = temp_dir("collect");
        std::fs::create_dir_all(dir.join("scenes/nested")).unwrap();
        for name in ["scenes/b.scene", "scenes/a.scene", "scenes/nested/c.scene"] {
            std::fs::write(dir.join(name), "scene_version=2\nname=x\n").unwrap();
        }
        let files = project_scene_files(&dir);
        let names = files
            .iter()
            .map(|path| {
                path.strip_prefix(&dir)
                    .expect("under root")
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            ["scenes/a.scene", "scenes/b.scene", "scenes/nested/c.scene"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
