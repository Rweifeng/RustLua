//! **一次性数据迁移工具**：把旧工程的场景/Prefab 改写成统一组件的样子。
//!
//! 引擎里只保留"统一表示"这一套核心代码（`Transform` / `Camera`），旧名（`Transform3D` /
//! `Camera2D` / `Camera3D`）与"打开文档时自动迁移"都已经删除。外部老工程因此**不会**再被
//! 自动迁移，需要先用本工具跑一次；仓库内的两个示例工程已经迁完（见 docs/56 §3.52）。
//!
//! 做三件事（只改名/换形态，**不动数值**）：
//!
//! 1. `Transform3D position=..;rotation=..;scale=..` → `Transform`（载荷逐字节不变）；
//! 2. 2D 旧形态的 `Transform x=..;y=..;rotation=..;scale_x=..;scale_y=..` → 统一形态
//!    （走 `LocalTransform` 的 2D 视图：z=0、绕 Z 四元数、scale_z=1）；
//! 3. `Camera2D` / `Camera3D` → `Camera`，并补上最前面的投影判别字段
//!    （`projection=orthographic|perspective`，字段体仍由各自既有的编码器写出）。
//!
//! **改名之前的空间自检**（把"实体属于 2D 还是 3D"变成可验证的事实，而不是猜测）：
//! 统一之后磁盘上不再有 2D/3D 组件名可依据，运行时由 `spatial_transform::entity_space`
//! 按"实体带的其它组件"推断。因此工具先把改名结果放进一份**候选组件表**，再用**引擎那个
//! 同一个函数**问一遍："它还是 3D 吗？"——不是就报告并拒绝迁移该实体。默默改名会让它在
//! 运行时落进 2D 空间而画面变样，那正是最难查的一类问题。
//!
//! 用法（从 `engine/` 目录跑）：
//!
//! ```text
//! cargo run -q -p editor --example migrate_project -- <项目根> [<项目根>...]
//! cargo run -q -p editor --example migrate_project -- --write <项目根>
//! ```
//!
//! 默认 **dry-run**（只报告、不写盘）；加 `--write` 才写回，并逐字节读回校验。
//! 有跳过项即以非零码退出（"没迁干净"不能当成功）。

use std::{
    collections::BTreeMap,
    env, fmt, fs,
    path::{Path, PathBuf},
};

use rustlua_camera::camera::{CAMERA_COMPONENT, Camera, Camera2D, Camera3D};
use rustlua_ecs::spatial_transform::{LocalTransform, SpatialKind, THREE_D_MARKERS, entity_space};
use rustlua_ecs::transform::{Transform, Transform3D};
use rustlua_scene::prefab::Prefab;
use rustlua_scene::scene::{Scene, SceneEntity, SceneEntityId};

/// 旧组件名（本次要抹掉的三个）。
const LEGACY_TRANSFORM_3D: &str = "Transform3D";
const LEGACY_CAMERA_2D: &str = "Camera2D";
const LEGACY_CAMERA_3D: &str = "Camera3D";
/// 统一后的变换组件名。
const TRANSFORM: &str = "Transform";

/// 迁移过程中**不能**被静默忽略的问题。
#[derive(Debug)]
enum Skip {
    /// 载荷解析不了：原样保留（改写它等于毁数据）。
    BrokenPayload { component: String, reason: String },
    /// 改名之后空间判不出来：3D 旧名的实体却没有 3D 专属组件。
    SpaceNotInferable,
    /// 同一实体两个旧相机名并存：不猜，报告出来让人决定。
    ConflictingCameras,
}

impl fmt::Display for Skip {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BrokenPayload { component, reason } => {
                write!(formatter, "{component} 载荷无法解析（原样保留）：{reason}")
            }
            Self::SpaceNotInferable => write!(
                formatter,
                "{LEGACY_TRANSFORM_3D} 实体没有任何 3D 专属组件（{}），统一后空间判不出来（保持原样）",
                THREE_D_MARKERS.join(" / ")
            ),
            Self::ConflictingCameras => write!(
                formatter,
                "{LEGACY_CAMERA_2D} 与 {LEGACY_CAMERA_3D} 并存，不猜（保持原样）"
            ),
        }
    }
}

#[derive(Default, Debug)]
struct Report {
    files_seen: usize,
    files_changed: usize,
    files_written: usize,
    transforms_renamed: usize,
    transforms_reshaped: usize,
    cameras_renamed: usize,
    skipped: Vec<(PathBuf, u64, String, Skip)>,
}

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let write = args.iter().any(|arg| arg == "--write");
    let roots = args
        .iter()
        .filter(|arg| !arg.starts_with("--"))
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if roots.is_empty() {
        eprintln!("用法：migrate_project [--write] <项目根> [<项目根>...]");
        std::process::exit(2);
    }
    let mut report = Report::default();
    for root in &roots {
        migrate_root(root, write, &mut report);
    }
    print_report(&report, write);
    if !report.skipped.is_empty() {
        std::process::exit(1);
    }
}

fn print_report(report: &Report, write: bool) {
    println!(
        "扫描 {} 个文件，改动 {} 个{}",
        report.files_seen,
        report.files_changed,
        if write {
            format!("（已写回 {}）", report.files_written)
        } else {
            "（dry-run，未写盘）".to_owned()
        }
    );
    println!(
        "  {LEGACY_TRANSFORM_3D} → {TRANSFORM}：{} 处；2D 旧形态 → 统一形态：{} 处；相机改名：{} 处",
        report.transforms_renamed, report.transforms_reshaped, report.cameras_renamed
    );
    for (path, entity, component, skip) in &report.skipped {
        println!(
            "  ! {} 实体 {entity} 组件 {component}：{skip}",
            path.display()
        );
    }
}

fn migrate_root(root: &Path, write: bool, report: &mut Report) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("");
                // 构建产物与缓存不是源数据：迁移它们只会制造无谓差异（发布包由构建重新生成）。
                if matches!(name, "builds" | ".cache" | "target" | ".git") {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if !matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("scene" | "prefab")
            ) {
                continue;
            }
            report.files_seen += 1;
            migrate_file(&path, write, report);
        }
    }
}

/// 一个实体上各类改动的条数（分开报，便于与迁移前的普查数字对账）。
#[derive(Clone, Copy, Default, Debug)]
struct Changes {
    renamed: usize,
    reshaped: usize,
    cameras: usize,
}

impl Changes {
    const fn total(self) -> usize {
        self.renamed + self.reshaped + self.cameras
    }
}

/// 把一个实体的组件表改写成统一形态。
///
/// 返回 `Ok((新组件表, 改动条数))` 或 `Err(跳过原因)`；**失败时不做任何改动**（调用方保留原表）。
fn migrate_entity(
    components: &BTreeMap<String, String>,
) -> Result<(BTreeMap<String, String>, Changes), Skip> {
    let mut next = components.clone();
    let mut changes = Changes::default();

    // ① `Transform3D` → `Transform`（载荷逐字节不变，数字格式仍由原编码器决定）。
    let was_three_d = if let Some(payload) = next.remove(LEGACY_TRANSFORM_3D) {
        Transform3D::from_component_str(&payload).map_err(|error| Skip::BrokenPayload {
            component: LEGACY_TRANSFORM_3D.to_owned(),
            reason: error.to_string(),
        })?;
        next.insert(TRANSFORM.to_owned(), payload);
        changes.renamed += 1;
        true
    } else {
        false
    };

    // ② 2D 旧形态 → 统一形态（键集合不相交，所以"不是 3D 形态"就等于"是 2D 旧形态"）。
    if let Some(payload) = next.get(TRANSFORM).cloned() {
        if Transform3D::from_component_str(&payload).is_err() {
            let flat =
                Transform::from_component_str(&payload).map_err(|error| Skip::BrokenPayload {
                    component: TRANSFORM.to_owned(),
                    reason: error.to_string(),
                })?;
            let local = LocalTransform::from_2d(flat).map_err(|error| Skip::BrokenPayload {
                component: TRANSFORM.to_owned(),
                reason: error.to_string(),
            })?;
            next.insert(TRANSFORM.to_owned(), local.to_unified_component_str());
            changes.reshaped += 1;
        } else {
            // 已经是统一形态：仍要能被自己读回来（畸形串不许悄悄过）。
            LocalTransform::from_unified_component_str(&payload).map_err(|reason| {
                Skip::BrokenPayload {
                    component: TRANSFORM.to_owned(),
                    reason,
                }
            })?;
        }
    }

    // ③ 相机改名 + 补投影判别字段。
    let legacy_2d = next.get(LEGACY_CAMERA_2D).cloned();
    let legacy_3d = next.get(LEGACY_CAMERA_3D).cloned();
    if legacy_2d.is_some() && legacy_3d.is_some() {
        return Err(Skip::ConflictingCameras);
    }
    for (legacy, is_3d) in [(LEGACY_CAMERA_2D, false), (LEGACY_CAMERA_3D, true)] {
        let Some(payload) = next.get(legacy).cloned() else {
            continue;
        };
        next.remove(legacy);
        // 两条分支的错误类型不同（各自解析器的错误），因此统一成字符串再收敛。
        let camera = if is_3d {
            Camera3D::from_component_str(&payload)
                .map(Camera::Perspective)
                .map_err(|error| error.to_string())
        } else {
            Camera2D::from_component_str(&payload)
                .map(Camera::Orthographic)
                .map_err(|error| error.to_string())
        }
        .map_err(|reason| Skip::BrokenPayload {
            component: legacy.to_owned(),
            reason,
        })?;
        next.insert(CAMERA_COMPONENT.to_owned(), camera.to_component_str());
        changes.cameras += 1;
    }
    // 统一名但缺判别字段的相机：报出来（不猜投影）。
    if let Some(raw) = next.get(CAMERA_COMPONENT)
        && Camera::from_component_str(raw).is_err()
    {
        return Err(Skip::BrokenPayload {
            component: CAMERA_COMPONENT.to_owned(),
            reason: "统一 Camera 载荷不合法（或缺 projection 判别字段）".to_owned(),
        });
    }

    // ④ **空间自检**：旧 3D 名改完之后，运行时必须仍然判它是 3D。
    if was_three_d && entity_space(&next) != SpatialKind::ThreeD {
        return Err(Skip::SpaceNotInferable);
    }
    Ok((next, changes))
}

/// 一份待迁移的数据文件：场景与 Prefab 是两种容器，实体表示完全相同（`SceneEntity`）。
///
/// 必须分开处理：`.prefab` 用 `Scene::load` 是读不出来的（头部是 `prefab_version=1`），
/// 而 `Prefab` 自己也有 `to_text`/`from_text` 用于写盘自检。
enum Document {
    Scene(Scene),
    Prefab(Prefab),
}

impl Document {
    fn load(path: &Path) -> Result<Self, String> {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("prefab") => Prefab::load(path)
                .map(Self::Prefab)
                .map_err(|error| error.to_string()),
            _ => Scene::load(path)
                .map(Self::Scene)
                .map_err(|error| error.to_string()),
        }
    }

    fn entities_mut(&mut self) -> &mut BTreeMap<SceneEntityId, SceneEntity> {
        match self {
            Self::Scene(scene) => &mut scene.entities,
            Self::Prefab(prefab) => &mut prefab.entities,
        }
    }

    fn to_text(&self) -> String {
        match self {
            Self::Scene(scene) => scene.to_text(),
            Self::Prefab(prefab) => prefab.to_text(),
        }
    }

    /// 写盘自检：写出去的内容必须能被自己读回来，且与将要写入的文本逐字节一致。
    fn round_trips(&self, text: &str) -> bool {
        match self {
            Self::Scene(_) => Scene::from_text(text).is_ok_and(|again| again.to_text() == text),
            Self::Prefab(_) => Prefab::from_text(text).is_ok_and(|again| again.to_text() == text),
        }
    }
}

fn migrate_file(path: &Path, write: bool, report: &mut Report) {
    let Ok(mut updated) = Document::load(path) else {
        report.skipped.push((
            path.to_path_buf(),
            0,
            "<file>".to_owned(),
            Skip::BrokenPayload {
                component: "<scene>".to_owned(),
                reason: "场景/Prefab 文件读不出来".to_owned(),
            },
        ));
        return;
    };
    let mut changes = Changes::default();
    for entity in updated.entities_mut().values_mut() {
        match migrate_entity(&entity.components) {
            Ok((next, entity_changes)) => {
                if entity_changes.total() > 0 {
                    entity.components = next;
                    changes.renamed += entity_changes.renamed;
                    changes.reshaped += entity_changes.reshaped;
                    changes.cameras += entity_changes.cameras;
                }
            }
            Err(skip) => {
                let component = skip_component(&skip);
                report
                    .skipped
                    .push((path.to_path_buf(), entity.id.0, component, skip));
            }
        }
    }
    if changes.total() == 0 {
        return;
    }
    report.files_changed += 1;
    report.transforms_renamed += changes.renamed;
    report.transforms_reshaped += changes.reshaped;
    report.cameras_renamed += changes.cameras;
    if !write {
        return;
    }
    let text = updated.to_text();
    if !updated.round_trips(&text) {
        eprintln!("{} 迁移结果自检失败：已放弃写回", path.display());
        return;
    }
    if let Err(error) = fs::write(path, &text) {
        eprintln!("{} 写回失败：{error}", path.display());
        return;
    }
    let read_back = fs::read_to_string(path).unwrap_or_default();
    if read_back != text {
        eprintln!(
            "{} 写回校验失败：读回的字节与写入内容不一致",
            path.display()
        );
        return;
    }
    report.files_written += 1;
}

/// 报告里点名的组件名。
fn skip_component(skip: &Skip) -> String {
    match skip {
        Skip::BrokenPayload { component, .. } => component.clone(),
        Skip::SpaceNotInferable => LEGACY_TRANSFORM_3D.to_owned(),
        Skip::ConflictingCameras => format!("{LEGACY_CAMERA_2D}+{LEGACY_CAMERA_3D}"),
    }
}
