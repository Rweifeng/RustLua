//! P3-E05-4：28 个 UI 源场景/预制体的布局回归。
//!
//! 报告 = 每个文件 × 3 个分辨率的 UI 实体矩形（三位小数）+ 诊断。基线文件
//! `tests/fixtures/ui_source_layout.golden.txt` 是 2026-10-23（E05-1..3 之后）
//! 生成的；仓库此前没有 UI 布局基线，因此无法重建 E05 之前的逐值 diff（E05-3 的
//! 文本固有尺寸属于有意变化），从本轮起用哈希锁住每一行的全部数值。
//!
//! 更新基线：`UPDATE_UI_LAYOUT_GOLDEN=1 cargo test -p editor ui_source_scene_layout_matches_golden`。

use super::*;
use rustlua_scene::prefab::Prefab;
use rustlua_ui::ui_scene_layout::resolve_scene_ui_layout;
use std::collections::BTreeMap;
use std::fs;

const RESOLUTIONS: [[f32; 2]; 3] = [[1280.0, 720.0], [1920.0, 1080.0], [1080.0, 1920.0]];

/// 参与布局报告的组件：屏幕空间 UI 与容器；`WorldText` 是 3D 世界文字，不在此列。
const UI_COMPONENTS: [&str; 9] = [
    "UiLayout",
    "Container",
    "LayoutItem",
    "Text",
    "Image",
    "UiEvents",
    "ButtonStates",
    "UiBindings",
    "UiAnimation",
];

fn examples_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../examples")
}

fn load_source(path: &Path) -> rustlua_scene::scene::Scene {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("scene") => rustlua_scene::scene::Scene::load(path)
            .unwrap_or_else(|error| panic!("{path:?}: {error}")),
        Some("prefab") => {
            let prefab = Prefab::load(path).unwrap_or_else(|error| panic!("{path:?}: {error}"));
            let mut scene = rustlua_scene::scene::Scene::new(prefab.name.clone());
            prefab
                .instantiate(&mut scene, None, &BTreeMap::new())
                .unwrap_or_else(|error| panic!("{path:?}: {error}"));
            scene
        }
        _ => unreachable!("only .scene/.prefab reach here"),
    }
}

fn is_ui_source(scene: &rustlua_scene::scene::Scene) -> bool {
    scene.entities.values().any(|entity| {
        UI_COMPONENTS
            .iter()
            .any(|key| entity.components.contains_key(*key))
    })
}

fn ui_source_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory).expect("examples/ must be readable") {
            let path = entry.expect("readable dir entry").path();
            if path.is_dir() {
                let name = path.file_name().and_then(|name| name.to_str());
                // 发布包与临时目录不是源场景（规划 §6.12 的 28 个源文件口径）。
                if matches!(name, Some("builds" | ".temp")) {
                    continue;
                }
                directories.push(path);
            } else if matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("scene" | "prefab")
            ) && is_ui_source(&load_source(&path))
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn layout_reports(root: &Path) -> Vec<String> {
    ui_source_files(root)
        .into_iter()
        .flat_map(|path| {
            let relative = path
                .strip_prefix(root)
                .expect("source file lives under examples/")
                .to_string_lossy()
                .replace('\\', "/");
            let scene = load_source(&path);
            let ui_ids: Vec<u64> = scene
                .entities
                .values()
                .filter(|entity| {
                    UI_COMPONENTS
                        .iter()
                        .any(|key| entity.components.contains_key(*key))
                })
                .map(|entity| entity.id.0)
                .collect();
            let nodes = scene_ui_nodes(&scene);
            RESOLUTIONS
                .iter()
                .map(|resolution| {
                    let (resolved, diagnostics) = resolve_scene_ui_layout(&nodes, *resolution);
                    let rects = ui_ids
                        .iter()
                        .map(|id| {
                            let node = resolved.get(id).unwrap_or_else(|| {
                                panic!("{relative}: missing rect for entity {id}")
                            });
                            format!(
                                "{id}:{:.3},{:.3},{:.3},{:.3},{}",
                                node.rect.x,
                                node.rect.y,
                                node.rect.w,
                                node.rect.h,
                                u8::from(node.container_controlled)
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(";");
                    let mut diagnostic_lines = diagnostics
                        .iter()
                        .map(|diagnostic| format!("{:?}", diagnostic))
                        .collect::<Vec<_>>();
                    diagnostic_lines.sort();
                    format!(
                        "{relative}|{}x{}|nodes={}|ui={}|diags={}|rects={:016x}|diag={:016x}",
                        resolution[0] as u32,
                        resolution[1] as u32,
                        nodes.len(),
                        ui_ids.len(),
                        diagnostics.len(),
                        fnv1a(rects.as_bytes()),
                        fnv1a(diagnostic_lines.join(";").as_bytes()),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn ui_source_scene_layout_matches_golden() {
    let root = examples_root();
    let reports = layout_reports(&root);
    assert_eq!(
        reports.len(),
        29 * RESOLUTIONS.len(),
        "UI 源场景/预制体数量变了（规划口径 29 个）：{}",
        reports
            .iter()
            .map(|report| report.split('|').next().unwrap())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    );

    let golden_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ui_source_layout.golden.txt");
    if std::env::var_os("UPDATE_UI_LAYOUT_GOLDEN").is_some() {
        fs::write(&golden_path, format!("{}\n", reports.join("\n")))
            .expect("golden file must be writable");
        return;
    }
    let golden = fs::read_to_string(&golden_path)
        .unwrap_or_else(|error| panic!("{}: {error}", golden_path.display()));
    let expected = golden.lines().collect::<Vec<_>>();
    assert_eq!(
        expected,
        reports.iter().map(String::as_str).collect::<Vec<_>>(),
        "UI 布局与基线不一致。若是有意改动，跑 UPDATE_UI_LAYOUT_GOLDEN=1 重新生成并解释 diff"
    );
}
