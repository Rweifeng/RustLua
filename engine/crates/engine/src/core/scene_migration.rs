//! WP2/WP9：场景空间层级**迁移命令**的数据层实现（dry-run / 写回 / 备份 / 一键回滚）。
//!
//! 负责人裁定"所有场景的 `parent` 一律按局部空间解释"。既有场景里子实体的坐标是**世界**坐标，
//! 直接按新语义解释会让整棵子树位移，因此必须先迁移：把"父世界矩阵的逆"烘进子实体的局部变换，
//! 世界位置保持不变（见 [`crate::core::spatial_transform::migrate_scene_to_local_space`]）。
//!
//! 写回是**危险操作**，所以：默认 dry-run（只报告不写盘）；写回前先备份成 `<file>.bak`
//! （已存在则不覆盖，保证备份永远是"最原始"的那份）；`restore` 把 `.bak` 拷回去并删掉它，
//! 即一键回滚。无法无损建立继承的实体会被拒绝、**清掉 `parent`** 并逐条报告。
//!
//! ## 幂等标记与"崩溃安全"（docs/54 §8-⑧）
//!
//! 迁移**不可重复施加**（数据在迁移后已经是局部坐标，再按"局部=世界"解释一次会把父变换又乘
//! 一遍），因此写回必须留下"我迁过了"的痕迹，而且这个痕迹不能是**第二步**写盘：先写场景、
//! 再写标记，中间崩溃就会留下"已迁移但没标记"的场景，下次运行会二次施加（数据被悄悄挪走）。
//!
//! 现在标记就是场景文件自己的 `meta` 行（`meta\tspatial\tlocal`）：
//!
//! - 它和迁移后的组件数据**在同一次原子写里**落盘，只有一个提交点，不存在"写了一半"；
//! - 回滚把 `<file>.bak` 拷回去，标记随之消失（不需要额外清理）；
//! - 旧版旁路标记 `<file>.spatial-local` 仍然被认（已在旧命令下迁过的工程不会二次施加），
//!   并在回滚时清理。
//!
//! 写盘本身用 [`write_scene_atomically`]：唯一临时名 → 写 → `sync_all` → rename → **读回校验**。
//! 读回的字节与期望不一致即报错，绝不留下"看起来写成功、内容却是别的"的场景。

use std::path::{Path, PathBuf};

use crate::core::{
    scene::{Scene, SceneEntityId},
    spatial_transform::migrate_scene_to_local_space,
};

/// 场景内嵌迁移标记的键（`meta\tspatial\tlocal`）。见模块头"幂等标记"。
pub const MIGRATION_META_KEY: &str = "spatial";
/// 场景内嵌迁移标记的值。
pub const MIGRATION_META_VALUE: &str = "local";

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
    /// 该场景**本来就**带迁移标记（本次跳过，没有改动文件）。
    pub already_migrated: bool,
    /// 写盘后的读回校验是否通过（没写盘时为 `false`）。
    pub verified: bool,
    /// 备份文件路径（写了备份才有）。
    pub backup: Option<PathBuf>,
    /// 命中"已删除的旧设计"的用法（docs/30 的弃用告警，见 [`scene_removed_designs`]）。
    pub removed_designs: Vec<RemovedDesign>,
}

impl SceneMigrationOutcome {
    /// 是否无损（没有实体被拒绝）。
    #[must_use]
    pub fn is_lossless(&self) -> bool {
        self.refused == 0
    }
}

/// 一条"旧设计已被删除"的诊断（docs/30《兼容性设计清单》的"已删除的旧设计"表）。
///
/// 为什么不只是报 `malformed field`：旧场景里的 `Text = text=hi;font_size=12;x=5` 会以
/// "malformed field: x=5" 失败，作者看不出**该把 x 搬到哪**。这里把"删了什么、现在写在哪"
/// 一起报出来，是迁移命令能给出的最小可用弃用告警。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovedDesign {
    pub entity: SceneEntityId,
    /// 组件名（`Text` / `Image` / `Sprite` / `Button` …）。
    pub component: String,
    /// 命中的旧写法（字段名或旧格式串）。
    pub removed: String,
    /// 现在应该写在哪（引 docs/30 的现行规则）。
    pub replacement: &'static str,
}

/// 扫描一个场景，逐条列出命中"已删除的旧设计"的用法（docs/30）。
///
/// 纯只读、幂等，顺序**确定**：实体与组件都走 `BTreeMap`（按 id / 名字），同一组件内按字段在
/// 组件串里的出现顺序——因此可以直接进迁移报告与质量门，逐字节可比。
#[must_use]
pub fn scene_removed_designs(scene: &Scene) -> Vec<RemovedDesign> {
    /// `Text` / `Image` 里已迁移到 `UiLayout` 的内联几何字段。
    const MOVED_TO_UI_LAYOUT: [&str; 7] = ["x", "y", "width", "height", "space", "anchor", "scale"];
    let mut found = Vec::new();
    for entity in scene.entities.values() {
        for (name, raw) in &entity.components {
            // ① `Text` / `Image` 的内联几何与 `scale`：几何现在是 `UiLayout` 的事。
            if name == "Text" || name == "Image" {
                for field in raw.split(';') {
                    let key = field.split('=').next().unwrap_or_default().trim();
                    if MOVED_TO_UI_LAYOUT.contains(&key) {
                        found.push(RemovedDesign {
                            entity: entity.id,
                            component: name.clone(),
                            removed: key.to_owned(),
                            replacement: "UiLayout 的四边 anchors/offsets（docs/30 §5/§6/§7）",
                        });
                    }
                }
                if raw.contains("clickEvent") {
                    found.push(RemovedDesign {
                        entity: entity.id,
                        component: name.clone(),
                        removed: "clickEvent".to_owned(),
                        replacement: "UiEvents 的结构化绑定（docs/30 §8）",
                    });
                }
            }
            // ② `Button` 事件标记：按钮现在是 `Image + Text + UiEvents`。
            if name == "Button" {
                found.push(RemovedDesign {
                    entity: entity.id,
                    component: name.clone(),
                    removed: "Button 事件标记".to_owned(),
                    replacement: "Image + Text + UiEvents（视觉状态用 ButtonStates）",
                });
            }
            // ③ `Sprite` 旧短格式：`<texture>;size=WxH`（首字段必须是 `texture=`）。
            if name == "Sprite" {
                let first = raw.split(';').next().unwrap_or_default().trim();
                let is_structured = first
                    .split_once('=')
                    .is_some_and(|(key, _)| key.trim() == "texture");
                if !is_structured {
                    found.push(RemovedDesign {
                        entity: entity.id,
                        component: name.clone(),
                        removed: first.to_owned(),
                        replacement: "texture=<路径>;size=<w>x<h>（docs/30 §4）",
                    });
                }
            }
        }
    }
    found
}

/// 该场景是否已按局部空间写回：认"文件内嵌标记"与"旧版旁路标记"两者之一。
#[must_use]
pub fn is_spatially_migrated(scene: &Scene, path: &Path) -> bool {
    scene.metadata.get(MIGRATION_META_KEY).map(String::as_str) == Some(MIGRATION_META_VALUE)
        || marker_path(path).is_file()
}

/// 迁移一个场景文件。`options.write` 为假时只读盘、只报告（dry-run）。
pub fn migrate_scene_file(
    path: &Path,
    options: MigrationOptions,
) -> Result<SceneMigrationOutcome, String> {
    let scene = Scene::load(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let removed_designs = scene_removed_designs(&scene);
    // 迁移**不可重复施加**：标记存在就直接跳过（见模块头"幂等标记"）。
    if options.write && is_spatially_migrated(&scene, path) {
        return Ok(SceneMigrationOutcome {
            path: path.to_path_buf(),
            entities: scene.entities.len(),
            converted: 0,
            refused: 0,
            skipped: 0,
            written: false,
            already_migrated: true,
            verified: false,
            backup: None,
            removed_designs,
        });
    }
    let (migration, report) = migrate_scene_to_local_space(&scene);
    let mut outcome = SceneMigrationOutcome {
        path: path.to_path_buf(),
        entities: scene.entities.len(),
        converted: report.converted.len(),
        refused: report.refused.len(),
        skipped: report.skipped.len(),
        written: false,
        already_migrated: false,
        verified: false,
        backup: None,
        removed_designs,
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
    // 幂等标记写进场景自身的 meta：与组件数据同一次原子写（没有"第二步"）。
    updated
        .metadata
        .insert(MIGRATION_META_KEY.into(), MIGRATION_META_VALUE.into());
    // 写盘前先验证内容能被自己解析回来、且再序列化逐字节一致（"写出去的东西一定读得回来"）。
    let contents = updated.to_text();
    let round_trip = Scene::from_text(&contents)
        .map_err(|error| format!("迁移结果自检失败 {}: {error}", path.display()))?;
    if round_trip.to_text() != contents {
        return Err(format!(
            "迁移结果自检失败 {}：重新序列化与写入内容不一致（已放弃写回，源文件未改动）",
            path.display()
        ));
    }
    if options.backup {
        let backup = backup_path(path);
        if backup.exists() {
            // 已存在 ⇒ 保留**最原始**的那一份（不覆盖），迁移可以反复运行而备份不被冲掉。
            // 但它必须是个文件：路径上蹲着一个目录/其它东西属于坏状态，静默当成"已有备份"
            // 会让用户以为有回滚点，其实没有。
            if !backup.is_file() {
                return Err(format!(
                    "备份失败 {}：路径已存在且不是文件（请先清理它，源文件未改动）",
                    backup.display()
                ));
            }
        } else {
            std::fs::copy(path, &backup)
                .map_err(|error| format!("备份失败 {}: {error}", backup.display()))?;
        }
        outcome.backup = Some(backup);
    }
    // 原子写 + 读回校验：失败时源文件仍是**原样**（临时文件写坏不会碰到目标文件）。
    write_scene_atomically(path, &contents)?;
    outcome.verified = true;
    outcome.written = true;
    Ok(outcome)
}

/// 原子写回一个场景文件：唯一临时名 → 写 + `sync_all` → rename → **读回校验**。
///
/// `Scene::save` 用的是 `<path>.tmp` 这种**共享**临时名（两个进程同时迁移会互相覆盖），
/// 而且写完不校验。迁移是"改用户源码"的操作，因此这里两件事都做：
/// 临时名带进程 id，写完把文件读回来逐字节比对，不一致就报错（此时目标文件要么是旧的、
/// 要么是新的完整内容，绝不会是拼起来的）。
pub fn write_scene_atomically(path: &Path, contents: &str) -> Result<(), String> {
    use std::io::Write;
    let temp = temporary_path(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建目录失败 {}: {error}", parent.display()))?;
    }
    let write = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        Ok(())
    };
    if let Err(error) = write() {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("写回失败 {}: {error}", temp.display()));
    }
    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("替换失败 {}: {error}", path.display()));
    }
    let read_back = std::fs::read_to_string(path)
        .map_err(|error| format!("写回校验失败（读回）{}: {error}", path.display()))?;
    if read_back != contents {
        return Err(format!(
            "写回校验失败 {}：读回的字节与写入内容不一致（{} vs {} 字节）",
            path.display(),
            read_back.len(),
            contents.len()
        ));
    }
    Ok(())
}

/// 原子写的临时文件路径（带进程 id，避免两个迁移进程互相覆盖）。
#[must_use]
pub fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".new-{}", std::process::id()));
    PathBuf::from(name)
}

/// 迁移标记文件（`<scene>.spatial-local`）——**旧版**旁路标记，现在只用于兼容读取。
/// 新写回把标记内嵌进场景（见模块头），因此这个文件不再由迁移命令创建。
#[must_use]
pub fn marker_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".spatial-local");
    PathBuf::from(name)
}

/// 一键回滚：把 `<file>.bak` 拷回去、删掉备份与旧版迁移标记。返回是否真的回滚了。
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
        assert!(written.verified, "写回后必须读回校验通过");
        assert!(!written.already_migrated);
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
        // 幂等标记**内嵌在场景文件里**（与组件数据同一次原子写），不是第二步写的旁路文件。
        assert!(
            is_spatially_migrated(&migrated, &path),
            "写回后场景自身必须带迁移标记"
        );
        assert!(
            std::fs::read_to_string(&path)
                .expect("read migrated")
                .contains("meta\tspatial\tlocal"),
            "标记必须是场景文件里的一行 meta"
        );
        assert!(!marker_path(&path).exists(), "新写回不再创建旁路标记文件");
        assert!(
            !temporary_path(&path).exists(),
            "原子写的临时文件必须已被 rename 掉"
        );

        // 重复运行安全：标记让第二次调用直接跳过（迁移**不可重复施加**——数据已经是局部
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
        assert!(again.already_migrated, "跳过原因必须是「已带标记」");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read after repeat"),
            before_repeat,
            "重复运行不得改动文件"
        );
        assert_eq!(
            child_position(&Scene::load(&path).expect("reload")),
            crate::core::transform::Vec2::new(-3.5, 0.0)
        );
        assert!(is_spatially_migrated(&migrated, &path));

        // 一键回滚：回到最原始的内容（标记随备份一起消失），备份被删掉。
        assert!(restore_scene_backup(&path).expect("restore"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read restored"),
            original,
            "回滚必须还原出原始内容"
        );
        assert!(
            !is_spatially_migrated(&Scene::load(&path).expect("reload restored"), &path),
            "回滚后场景必须回到「未迁移」状态，否则迁移命令会永远跳过它"
        );
        assert!(!backup_path(&path).exists());
        assert!(!marker_path(&path).exists(), "回滚后旧版标记也要清掉");
        assert!(!restore_scene_backup(&path).expect("no-op restore"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 旧版旁路标记（`<scene>.spatial-local`）仍然被认：已在旧命令下迁过的工程不会被二次施加。
    #[test]
    fn legacy_sidecar_marker_still_skips_the_write() {
        let dir = temp_dir("legacy-marker");
        let path = dir.join("main.scene");
        hierarchy_scene().save(&path).expect("save scene");
        std::fs::write(marker_path(&path), "spatial=local\n").expect("legacy marker");
        let before = std::fs::read_to_string(&path).expect("read");
        let outcome = migrate_scene_file(
            &path,
            MigrationOptions {
                write: true,
                backup: true,
            },
        )
        .expect("migrate");
        assert!(outcome.already_migrated);
        assert!(!outcome.written);
        assert_eq!(std::fs::read_to_string(&path).expect("unchanged"), before);
        assert!(
            !backup_path(&path).exists(),
            "跳过时不该留下备份（没有改动就没有回滚点）"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// docs/54 §8-⑧「弃用告警」：命中 docs/30《兼容性设计清单》里"已删除的旧设计"的用法
    /// 必须被逐条点名，并给出"现在该写在哪"——只报 `malformed field: x=5` 作者看不出搬家方向。
    #[test]
    fn removed_designs_are_reported_from_the_compatibility_table() {
        let mut scene = Scene::new("legacy-designs");
        let text = SceneEntityId(1);
        let sprite = SceneEntityId(2);
        let button = SceneEntityId(3);
        for (id, name) in [(text, "Hud"), (sprite, "Icon"), (button, "Ok")] {
            scene.add_entity(id, name, None).unwrap();
        }
        scene.entities.get_mut(&text).unwrap().components.insert(
            "Text".into(),
            "text=hi;font_size=12;color=1,1,1;x=10;y=20".into(),
        );
        scene
            .entities
            .get_mut(&text)
            .unwrap()
            .components
            .insert("Image".into(), "texture=a.png;width=32;height=32".into());
        scene
            .entities
            .get_mut(&sprite)
            .unwrap()
            .components
            .insert("Sprite".into(), "assets/hero.png;size=16x16".into());
        scene
            .entities
            .get_mut(&button)
            .unwrap()
            .components
            .insert("Button".into(), "on_click=ok".into());

        let found = scene_removed_designs(&scene);
        let names: Vec<(u64, &str, &str)> = found
            .iter()
            .map(|entry| {
                (
                    entry.entity.0,
                    entry.component.as_str(),
                    entry.removed.as_str(),
                )
            })
            .collect();
        assert_eq!(
            names,
            [
                (1, "Image", "width"),
                (1, "Image", "height"),
                (1, "Text", "x"),
                (1, "Text", "y"),
                (2, "Sprite", "assets/hero.png"),
                (3, "Button", "Button 事件标记"),
            ],
            "顺序确定（按实体 + 组件 + 旧写法），且每条都要报出来"
        );
        assert!(found.iter().all(|entry| !entry.replacement.is_empty()));
        // 合规的场景一条都不报。
        assert!(scene_removed_designs(&hierarchy_scene()).is_empty());
        // dry-run 报告里也带这些诊断（迁移命令因此能对整仓场景给出弃用清单）。
        let dir = temp_dir("removed-designs");
        let path = dir.join("legacy.scene");
        scene.save(&path).expect("save scene");
        let outcome = migrate_scene_file(&path, MigrationOptions::default()).expect("dry run");
        assert_eq!(outcome.removed_designs.len(), found.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 失败路径：备份写不出来（目标是目录）时**源文件不得被改动**。
    #[test]
    fn backup_failure_leaves_the_scene_untouched() {
        let dir = temp_dir("backup-failure");
        let path = dir.join("main.scene");
        hierarchy_scene().save(&path).expect("save scene");
        let original = std::fs::read_to_string(&path).expect("read original");
        // `<file>.bak` 先占成一个**目录**：`fs::copy` 必然失败。
        std::fs::create_dir(backup_path(&path)).expect("block the backup path");
        let error = migrate_scene_file(
            &path,
            MigrationOptions {
                write: true,
                backup: true,
            },
        )
        .expect_err("备份失败必须报错");
        assert!(error.contains("备份失败"), "{error}");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read after failure"),
            original,
            "备份失败时源文件必须原样不动"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 失败路径：原子写的临时位置被占死时**源文件不得被改动**，且不留半写文件。
    #[test]
    fn write_failure_leaves_the_scene_untouched() {
        let dir = temp_dir("write-failure");
        let path = dir.join("main.scene");
        hierarchy_scene().save(&path).expect("save scene");
        let original = std::fs::read_to_string(&path).expect("read original");
        // 临时名先占成一个目录：`File::create` 必然失败。
        std::fs::create_dir(temporary_path(&path)).expect("block the temp path");
        let error = migrate_scene_file(
            &path,
            MigrationOptions {
                write: true,
                backup: false,
            },
        )
        .expect_err("写回失败必须报错");
        assert!(error.contains("写回失败"), "{error}");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read after failure"),
            original,
            "写回失败时源文件必须原样不动"
        );
        // 源文件仍是旧语义：这次失败**没有**留下标记，因此迁移命令仍会重试它。
        assert!(!is_spatially_migrated(
            &Scene::load(&path).expect("reload"),
            &path
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 失败路径：源文件不存在 / 不是场景时给出带路径的错误，不创建新文件。
    #[test]
    fn missing_source_reports_the_path_without_creating_files() {
        let dir = temp_dir("missing");
        let path = dir.join("nope.scene");
        let error = migrate_scene_file(&path, MigrationOptions::default()).expect_err("missing");
        assert!(error.contains("nope.scene"), "{error}");
        assert!(!path.exists(), "失败的迁移不得凭空创建文件");
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
