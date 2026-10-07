//! Play 期间把预览世界的真值回灌进编辑器的**运行态镜像文档**（`PlaySession::runtime_document`）。
//!
//! 为什么要这一层：进入 Play 时编辑器把文档复制成"编辑快照 + 运行镜像"两份，面板在 Play
//! 期间读镜像、Stop 时丢弃镜像回到快照（`E07/E08` 的运行/编辑隔离）。但镜像过去**只在
//! `PlaySession::start` 那一刻被填过一次**，之后没人再写它——于是层级面板列不出脚本
//! `entity.spawn` 出来的实体，`entity.destroy` 掉的行还留在树上，位置也一直是进 Play 前的旧值。
//! Unity 的层级在 Play 期间跟着场景走，靠的就是这条回灌通道。
//!
//! 这里只放**纯函数**（吃数据、不吃运行时、不碰 GPU），因此可以脱离编辑器实例测试：
//! 输入是 [`RuntimeWorldSnapshot`]（宿主每帧从预览运行时打包的三份读数），输出是改过的
//! `Scene` + 一份 [`RuntimeSyncReport`]，调用方按 `structure_changed` 决定要不要 bump 修订号。
//!
//! 镜像文档**永不落盘**：保存/自动快照走的都是 `document.current_scene`（编辑那份），
//! Stop 用 `editor_snapshot` 复原，所以这里可以放心增删行——但**不能** `mark_dirty`，
//! 否则层级标题会一直挂着"● 未保存"，而那份"未保存"永远存不了。

use engine::core::{
    math3d::{Quat, Vec3},
    scene::{Scene, SceneEntity, SceneEntityId},
    spatial_transform::{LocalTransform, TRANSFORM_COMPONENT},
};
use engine::host::RuntimeSpawnedEntity;
use std::collections::{BTreeMap, BTreeSet};

use crate::document_space::encode_transform;

/// 镜像里一行"运行期生成实体"的身份键（存在 `SceneEntity::metadata` 里，值是句柄编码）。
///
/// 层级面板、选择模型都只认 `SceneEntityId`，而运行期实体的身份是 ECS 句柄（槽位+代际）。
/// 每帧重新分配文档 id 会让选中状态跟着漂，所以第一次见到某个句柄时就钉一行、之后按行认。
pub const RUNTIME_HANDLE_META: &str = "editor.runtime.handle";

/// 运行期行的第二个身份位：这个实体带 UI 组件（版面在组件里，不在姿态里）。
///
/// 为什么编辑器要单独记一位：镜像回灌**只写结构与姿态**（不做组件反向序列化），带
/// `UiLayout`/`Image`/`Text` 的运行期实体因此没有可解析的矩形——Scene 视图按姿态给它画框
/// 只会把整屏 UI 全叠在世界原点，那是**假框**。这一位让视图明确知道"这一行认不出位置"，
/// 从而不画框、把"它在哪儿"交给设计分辨率框里那张真游戏画面（见 docs/56 §3.53）。
/// 判据来自活世界的组件类型（`RuntimeSpawnedEntity::ui`），不是名字/路径的启发式。
pub const RUNTIME_UI_META: &str = "editor.runtime.ui";

/// 一帧的运行态读数（由宿主从预览运行时打包，见 `EditorApp::sync_runtime_document`）。
#[derive(Clone, Debug, Default)]
pub struct RuntimeWorldSnapshot<'a> {
    /// 运行期由脚本生成的实体（声明实体不在其中）。
    pub spawned: &'a [RuntimeSpawnedEntity],
    /// 「文档声明实体 → 运行句柄编码」登记表：用来区分"已销毁"与"从未实例化"。
    pub declared: &'a [(SceneEntityId, u64)],
    /// 声明实体此刻的局部变换；登记过句柄却不在这里 ⇒ 已被脚本销毁。
    pub authored: &'a [(SceneEntityId, LocalTransform)],
}

/// 一次回灌改了什么。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RuntimeSyncReport {
    pub rows_added: usize,
    pub rows_removed: usize,
    pub transforms_written: usize,
}

impl RuntimeSyncReport {
    /// 层级树本身是否变了（加了行/删了行）。
    ///
    /// 只有结构变了才需要 bump 文档修订号：行模型缓存、Scene 视图运行时都按修订号失效，
    /// 而"实体跑了一步"这种纯移动不该付整棵运行时重建的代价。
    pub fn structure_changed(&self) -> bool {
        self.rows_added != 0 || self.rows_removed != 0
    }
}

/// 把一帧运行态读数写进镜像文档：摘掉已经不在世界里的行、补上脚本新生成的行、
/// 把活姿态逐行写回 `Transform` 组件串。返回改了什么。
pub fn apply_runtime_snapshot(
    scene: &mut Scene,
    snapshot: &RuntimeWorldSnapshot,
) -> RuntimeSyncReport {
    let mut report = RuntimeSyncReport::default();
    let mut handle_to_row: BTreeMap<u64, SceneEntityId> = BTreeMap::new();
    for entity in scene.entities.values() {
        if let Some(handle) = runtime_handle(entity) {
            handle_to_row.insert(handle, entity.id);
        }
    }

    remove_dead_rows(scene, snapshot, &mut handle_to_row, &mut report);
    upsert_spawned_rows(scene, snapshot, &mut handle_to_row, &mut report);
    write_live_transforms(scene, snapshot, &handle_to_row, &mut report);
    report
}

/// 回灌之后「文档行 → 此刻的活局部变换」表，Scene 视图用它把编辑运行时推到当前姿态。
///
/// 单独一个读数而不是让视图去解析文档的 `Transform` 组件串：那份串是刚写进去的，
/// 再解析一遍等于同一件事做两次，10k 行的场景每帧多一次全表字符串解析。
pub fn live_transforms(
    scene: &Scene,
    snapshot: &RuntimeWorldSnapshot,
) -> Vec<(SceneEntityId, LocalTransform)> {
    let mut handle_to_row: BTreeMap<u64, SceneEntityId> = BTreeMap::new();
    for entity in scene.entities.values() {
        if let Some(handle) = runtime_handle(entity) {
            handle_to_row.insert(handle, entity.id);
        }
    }
    let mut out: Vec<(SceneEntityId, LocalTransform)> = snapshot.authored.to_vec();
    out.extend(snapshot.spawned.iter().filter_map(|row| {
        handle_to_row
            .get(&row.handle)
            .map(|id| (*id, local_transform(row)))
    }));
    out
}

/// 摘掉两种行：脚本销毁的声明实体、上一帧还在但这一帧没了的运行期行（连同子树）。
fn remove_dead_rows(
    scene: &mut Scene,
    snapshot: &RuntimeWorldSnapshot,
    handle_to_row: &mut BTreeMap<u64, SceneEntityId>,
    report: &mut RuntimeSyncReport,
) {
    let alive_authored: BTreeSet<SceneEntityId> =
        snapshot.authored.iter().map(|(id, _)| *id).collect();
    let mut doomed: Vec<SceneEntityId> = Vec::new();
    // 只判"登记过句柄却没了"的行；从未登记过的文档行（编辑期新增、还没实例化）
    // 不在 `declared` 里，因此不会被误摘。
    doomed.extend(
        snapshot
            .declared
            .iter()
            .filter(|(id, _)| !alive_authored.contains(id))
            .map(|(id, _)| *id),
    );
    for entity in scene.entities.values() {
        let Some(handle) = runtime_handle(entity) else {
            continue;
        };
        if !snapshot.spawned.iter().any(|row| row.handle == handle) {
            doomed.push(entity.id);
        }
    }
    for id in doomed {
        if !scene.entities.contains_key(&id) {
            continue;
        }
        for subtree_id in subtree_ids(scene, id) {
            if let Some(entity) = scene.entities.get(&subtree_id)
                && let Some(handle) = runtime_handle(entity)
            {
                handle_to_row.remove(&handle);
            }
            detach_from_parent(scene, subtree_id);
            scene.entities.remove(&subtree_id);
            report.rows_removed += 1;
        }
    }
}

/// 脚本新生成的实体进表：先按句柄找到已有行，找不到就新开一行，然后统一接父边。
fn upsert_spawned_rows(
    scene: &mut Scene,
    snapshot: &RuntimeWorldSnapshot,
    handle_to_row: &mut BTreeMap<u64, SceneEntityId>,
    report: &mut RuntimeSyncReport,
) {
    let authored_by_handle: BTreeMap<u64, SceneEntityId> = snapshot
        .declared
        .iter()
        .map(|(id, handle)| (*handle, *id))
        .collect();
    for row in snapshot.spawned {
        // 已有行就沿用它的行号，没有才新开一行——行号是选择状态的键，绝不能每帧重算。
        let id = match handle_to_row.get(&row.handle) {
            Some(&id) => id,
            None => {
                let id = next_scene_entity_id(scene);
                scene.entities.insert(
                    id,
                    SceneEntity {
                        id,
                        name: row.name.clone(),
                        parent: None,
                        children: Vec::new(),
                        components: BTreeMap::new(),
                        resources: BTreeMap::new(),
                        metadata: BTreeMap::new(),
                    },
                );
                handle_to_row.insert(row.handle, id);
                report.rows_added += 1;
                id
            }
        };
        let Some(entity) = scene.entities.get_mut(&id) else {
            continue;
        };
        entity.name = row.name.clone();
        entity
            .metadata
            .insert(RUNTIME_HANDLE_META.to_owned(), row.handle.to_string());
        // UI 行标一位：版面载荷不在镜像里，视图据此不画姿态位置的假框（见常量文档）。
        // 非 UI 行不留这一位（而不是写 "false"）：判据只查"有没有"，键越少每帧解析越省。
        if row.ui {
            entity
                .metadata
                .insert(RUNTIME_UI_META.to_owned(), "true".to_owned());
        }
    }
    // 父边第二遍接：`spawned` 按句柄升序，父可能在子之后才出现。
    for row in snapshot.spawned {
        let Some(&id) = handle_to_row.get(&row.handle) else {
            continue;
        };
        let parent = row
            .parent
            .as_ref()
            .and_then(|handle| {
                handle_to_row
                    .get(handle)
                    .or_else(|| authored_by_handle.get(handle))
            })
            .copied()
            .filter(|parent| *parent != id);
        set_parent(scene, id, parent);
    }
}

/// 活姿态逐行写回：声明实体用宿主给的局部变换，运行期行用快照里的分量。
fn write_live_transforms(
    scene: &mut Scene,
    snapshot: &RuntimeWorldSnapshot,
    handle_to_row: &BTreeMap<u64, SceneEntityId>,
    report: &mut RuntimeSyncReport,
) {
    for (id, local) in snapshot.authored {
        let Some(entity) = scene.entities.get_mut(id) else {
            continue;
        };
        if write_transform(entity, local) {
            report.transforms_written += 1;
        }
    }
    for row in snapshot.spawned {
        let Some(&id) = handle_to_row.get(&row.handle) else {
            continue;
        };
        let local = local_transform(row);
        let Some(entity) = scene.entities.get_mut(&id) else {
            continue;
        };
        if write_transform(entity, &local) {
            report.transforms_written += 1;
        }
    }
}

fn write_transform(entity: &mut SceneEntity, local: &LocalTransform) -> bool {
    let encoded = encode_transform(*local);
    if entity.components.get(TRANSFORM_COMPONENT) == Some(&encoded) {
        return false;
    }
    entity
        .components
        .insert(TRANSFORM_COMPONENT.to_owned(), encoded);
    true
}

fn local_transform(row: &RuntimeSpawnedEntity) -> LocalTransform {
    LocalTransform {
        translation: Vec3::new(row.position[0], row.position[1], row.position[2]),
        rotation: Quat {
            x: row.rotation[0],
            y: row.rotation[1],
            z: row.rotation[2],
            w: row.rotation[3],
        },
        scale: Vec3::new(row.scale[0], row.scale[1], row.scale[2]),
    }
}

fn runtime_handle(entity: &SceneEntity) -> Option<u64> {
    entity
        .metadata
        .get(RUNTIME_HANDLE_META)
        .and_then(|value| value.parse::<u64>().ok())
}

/// `id` 及其全部后代（含环保护：正常层级深度不会超过实体数）。
fn subtree_ids(scene: &Scene, root: SceneEntityId) -> Vec<SceneEntityId> {
    let mut out = Vec::new();
    let mut visiting = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !visiting.insert(id) {
            continue;
        }
        out.push(id);
        if let Some(entity) = scene.entities.get(&id) {
            stack.extend(entity.children.iter().copied());
        }
    }
    out
}

fn detach_from_parent(scene: &mut Scene, id: SceneEntityId) {
    let Some(parent) = scene.entities.get(&id).and_then(|entity| entity.parent) else {
        return;
    };
    if let Some(entity) = scene.entities.get_mut(&parent) {
        entity.children.retain(|child| *child != id);
    }
}

/// 换父：同时维护 `parent` 与父的 `children` 两侧，顺序保持已有兄弟位置不变。
fn set_parent(scene: &mut Scene, id: SceneEntityId, parent: Option<SceneEntityId>) {
    let current = scene.entities.get(&id).and_then(|entity| entity.parent);
    if current == parent {
        return;
    }
    if let Some(old) = current
        && let Some(entity) = scene.entities.get_mut(&old)
    {
        entity.children.retain(|child| *child != id);
    }
    if let Some(new) = parent
        && let Some(entity) = scene.entities.get_mut(&new)
        && !entity.children.contains(&id)
    {
        entity.children.push(id);
    }
    if let Some(entity) = scene.entities.get_mut(&id) {
        entity.parent = parent;
    }
}

fn next_scene_entity_id(scene: &Scene) -> SceneEntityId {
    let mut id = scene
        .entities
        .keys()
        .map(|key| key.0)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    while scene.entities.contains_key(&SceneEntityId(id)) {
        id = id.saturating_add(1);
    }
    SceneEntityId(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::core::spatial_transform::scene_entity_local;

    const HERO: SceneEntityId = SceneEntityId(1);
    const HAND: SceneEntityId = SceneEntityId(2);
    /// 句柄在这里只是**字典键**：编辑器不解开它（编码是宿主侧的事），所以测试里给什么数
    /// 都行，只要两两不同、并且和 `declared`/`authored` 里的数一致。
    const HERO_HANDLE: u64 = 101;
    const HAND_HANDLE: u64 = 102;
    const BULLET_HANDLE: u64 = 201;
    const SPARK_HANDLE: u64 = 202;

    /// 一份最小镜像文档：`Hero`（1）+ 其子 `Hand`（2），都带 `Transform` 分量串。
    fn mirror() -> Scene {
        let mut scene = Scene::new("play_sync");
        scene.add_entity(HERO, "Hero", None).unwrap();
        scene.add_entity(HAND, "Hand", Some(HERO)).unwrap();
        // 初值串直接用 `encode_transform` 生成：写回判的是"字符串有没有变"，手写格式跟
        // 编码器的格式化口径一旦对不上，"没变"的行会被数成"变了"。
        for id in [HERO, HAND] {
            scene.entities.get_mut(&id).unwrap().components.insert(
                TRANSFORM_COMPONENT.to_owned(),
                encode_transform(local(0.0, 0.0)),
            );
        }
        scene
    }

    fn spawned(handle: u64, name: &str, parent: Option<u64>) -> RuntimeSpawnedEntity {
        RuntimeSpawnedEntity {
            handle,
            name: name.to_owned(),
            parent,
            ui: false,
            position: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        }
    }

    fn local(x: f32, y: f32) -> LocalTransform {
        LocalTransform {
            translation: Vec3::new(x, y, 0.0),
            rotation: Quat::identity(),
            scale: Vec3::new(1.0, 1.0, 1.0),
        }
    }

    /// 声明实体都还活着的读数源（`authored` 给满两份）。
    ///
    /// 三份读数存成 owned 的 `Vec`：`RuntimeWorldSnapshot` 只借不拥有，直接内联临时数组
    /// 出了语句就没了。
    struct RunningWorld {
        declared: Vec<(SceneEntityId, u64)>,
        authored: Vec<(SceneEntityId, LocalTransform)>,
    }

    impl RunningWorld {
        fn new() -> Self {
            Self {
                declared: vec![(HERO, HERO_HANDLE), (HAND, HAND_HANDLE)],
                authored: vec![(HERO, local(0.0, 0.0)), (HAND, local(0.0, 0.0))],
            }
        }

        fn snapshot<'a>(&'a self, spawned: &'a [RuntimeSpawnedEntity]) -> RuntimeWorldSnapshot<'a> {
            RuntimeWorldSnapshot {
                spawned,
                declared: &self.declared,
                authored: &self.authored,
            }
        }
    }

    fn transform_of(scene: &Scene, id: SceneEntityId) -> Option<LocalTransform> {
        scene_entity_local(&scene.entities.get(&id)?.components)
    }

    fn row<'a>(scene: &'a Scene, name: &str) -> &'a SceneEntity {
        scene
            .entities
            .values()
            .find(|entity| entity.name == name)
            .unwrap_or_else(|| panic!("层级里没有名为 {name} 的行"))
    }

    /// 脚本 `entity.spawn` 出来的实体必须成为层级里的一行：作者在游戏里打出的子弹看不见，
    /// 就谈不上"定位问题"。
    #[test]
    fn runtime_spawned_entity_becomes_a_row() {
        let mut scene = mirror();
        let world = RunningWorld::new();
        let rows = [spawned(BULLET_HANDLE, "Bullet", None)];

        let report = apply_runtime_snapshot(&mut scene, &world.snapshot(&rows));
        assert_eq!(report.rows_added, 1);
        assert!(report.structure_changed(), "加了行必须让行模型缓存失效");
        let bullet = row(&scene, "Bullet");
        assert_eq!(
            bullet.metadata[RUNTIME_HANDLE_META],
            BULLET_HANDLE.to_string()
        );
        // 回灌写的是局部变换的规范分量串，属性面板与编辑运行时读的都是它。
        assert_eq!(
            bullet.components[TRANSFORM_COMPONENT],
            encode_transform(local(0.0, 0.0))
        );
    }

    /// 运行期 **UI** 行要带上那一位置标（`RUNTIME_UI_META`），非 UI 行不能带：
    /// Scene 视图据此决定"这一行认不出版面，别按姿态画假框"。
    #[test]
    fn ui_rows_are_flagged_and_plain_rows_are_not() {
        let mut scene = mirror();
        let world = RunningWorld::new();
        let rows = [
            spawned(BULLET_HANDLE, "Bullet", None),
            RuntimeSpawnedEntity {
                ui: true,
                ..spawned(SPARK_HANDLE, "Panel", None)
            },
        ];

        apply_runtime_snapshot(&mut scene, &world.snapshot(&rows));

        assert_eq!(row(&scene, "Panel").metadata[RUNTIME_UI_META], "true");
        assert!(
            !row(&scene, "Bullet").metadata.contains_key(RUNTIME_UI_META),
            "非 UI 行不该有这一位：判据是「有没有」，写 false 会让每帧多解析一个键"
        );
    }

    /// 同一份读数连回灌两次不该再建行：文档行号是选择状态的键，每帧漂一次等于选中永远留不住。
    #[test]
    fn repeated_snapshot_is_idempotent_and_keeps_row_ids() {
        let mut scene = mirror();
        let world = RunningWorld::new();
        let rows = [spawned(BULLET_HANDLE, "Bullet", None)];
        apply_runtime_snapshot(&mut scene, &world.snapshot(&rows));
        let first = row(&scene, "Bullet").id;

        let report = apply_runtime_snapshot(&mut scene, &world.snapshot(&rows));
        assert_eq!(report.rows_added, 0);
        assert!(!report.structure_changed(), "没有净变化不该 bump 修订号");
        assert_eq!(row(&scene, "Bullet").id, first, "行号必须稳定");
    }

    /// 运行期实体被 `entity.destroy` 之后，镜像里那一行连同它的运行期子行一起消失，
    /// 否则层级会列出游戏里已经不存在的东西。
    #[test]
    fn destroyed_spawned_entity_drops_its_row_and_children() {
        let mut scene = mirror();
        let world = RunningWorld::new();
        let rows = [
            spawned(BULLET_HANDLE, "Bullet", None),
            spawned(SPARK_HANDLE, "Spark", Some(BULLET_HANDLE)),
        ];
        apply_runtime_snapshot(&mut scene, &world.snapshot(&rows));
        assert_eq!(scene.entities.len(), 4);

        let report = apply_runtime_snapshot(&mut scene, &world.snapshot(&[]));
        assert_eq!(report.rows_removed, 2, "父行连同子行一起摘掉");
        assert_eq!(scene.entities.len(), 2);
    }

    /// 声明实体被脚本销毁 ⇒ 摘行（含子树）；**从未实例化过的行**（没登记句柄）不能被当成
    /// 已销毁，否则编辑期刚加的一行会在第一次同步时凭空消失。
    #[test]
    fn destroyed_authored_entity_drops_row_but_unregistered_row_survives() {
        let mut scene = mirror();
        scene.add_entity(SceneEntityId(7), "Fresh", None).unwrap();

        let report = apply_runtime_snapshot(
            &mut scene,
            &RuntimeWorldSnapshot {
                spawned: &[],
                declared: &[(HERO, HERO_HANDLE), (HAND, HAND_HANDLE)],
                // Hero 登记过句柄却没有活变换读数 ⇒ 已被脚本销毁。
                authored: &[(HAND, local(0.0, 0.0))],
            },
        );
        assert_eq!(report.rows_removed, 2, "Hero 与它的子树一起摘掉");
        assert!(!scene.entities.contains_key(&HERO));
        assert!(!scene.entities.contains_key(&HAND));
        assert!(
            scene.entities.contains_key(&SceneEntityId(7)),
            "未登记的行被当成已销毁摘掉了"
        );
    }

    /// 位置必须是活值，而纯移动**不该** bump 修订号：属性面板逐帧读文档，重建整棵编辑
    /// 运行时换来的只是卡顿。
    #[test]
    fn live_position_is_written_without_bumping_the_revision() {
        let mut scene = mirror();

        let report = apply_runtime_snapshot(
            &mut scene,
            &RuntimeWorldSnapshot {
                spawned: &[],
                declared: &[(HERO, HERO_HANDLE), (HAND, HAND_HANDLE)],
                authored: &[(HERO, local(120.0, 40.0)), (HAND, local(0.0, 0.0))],
            },
        );
        assert_eq!(report.rows_added, 0);
        assert_eq!(report.rows_removed, 0);
        assert!(!report.structure_changed(), "只是走了一步，不该重算行模型");
        assert_eq!(report.transforms_written, 1, "Hero 变了、Hand 没变");
        let hero = transform_of(&scene, HERO).expect("Hero 的 Transform 分量串");
        assert_eq!([hero.translation.x, hero.translation.y], [120.0, 40.0]);
    }

    /// 父边用句柄表达，两侧都要能对上：父是文档里的声明实体、或父的读数排在子之后
    /// （第二遍再接），都不能默默把它当根节点挂。
    #[test]
    fn spawned_row_attaches_to_its_parent_row() {
        let mut scene = mirror();
        let world = RunningWorld::new();
        let rows = [
            // 故意让"子"先于"父"出现：父此时还没建行，第二遍接父边时两侧都已就位。
            spawned(SPARK_HANDLE, "Spark", Some(BULLET_HANDLE)),
            spawned(BULLET_HANDLE, "Bullet", Some(HERO_HANDLE)),
        ];

        apply_runtime_snapshot(&mut scene, &world.snapshot(&rows));
        let bullet = row(&scene, "Bullet");
        assert_eq!(bullet.parent, Some(HERO), "父是声明实体时该对回文档行");
        let spark = row(&scene, "Spark");
        assert_eq!(spark.parent, Some(bullet.id), "父排在子之后读数时也要接上");
        assert!(
            scene.entities[&HERO].children.contains(&bullet.id),
            "父侧的 children 必须同步维护，否则层级展开看不见它"
        );
    }

    /// Scene 视图推进编辑运行时用的是 `(行, 活局部变换)` 表；让视图自己再解析一遍文档分量串
    /// 等于同一件事做两次，所以这张表必须声明实体与运行期行都覆盖到。
    #[test]
    fn live_transforms_cover_both_authored_and_spawned_rows() {
        let mut scene = mirror();
        let world = RunningWorld::new();
        let rows = [RuntimeSpawnedEntity {
            position: [9.0, 8.0, 0.0],
            ..spawned(BULLET_HANDLE, "Bullet", None)
        }];
        apply_runtime_snapshot(&mut scene, &world.snapshot(&rows));

        let table = live_transforms(&scene, &world.snapshot(&rows));
        let bullet_id = row(&scene, "Bullet").id;
        assert!(
            table.iter().any(|(id, _)| *id == HERO),
            "声明实体的活变换也该在表里"
        );
        let bullet = table
            .iter()
            .find(|(id, _)| *id == bullet_id)
            .expect("运行期行也该在表里")
            .1;
        assert_eq!([bullet.translation.x, bullet.translation.y], [9.0, 8.0]);
    }
}
