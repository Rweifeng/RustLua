//! 跨端路径黄金对拍（Rust 侧）。
//!
//! 同一份表 `scripts/test/fixtures/asset-routing-golden.json` 由三个读者分别解析：
//! - Rust（本文件）：`normalize_project_path` / `PlannedGroup::contains` / `distribution_layout`；
//! - JS（`scripts/test/asset-routing-golden-test.js`）：`rootOfUnit` / `planRootIndex` / `rootForPath`；
//! - 发布产物（`assetc closure`，见 `scripts/test/test-release-closure.ps1`）。
//!
//! 为什么必须"同一份表"：三个读者各自的单测都能绿，而它们对同一条路径给出不同答案时，
//! 症状是**真机白图**——容器里有字节、读取端用另一个键去查。把期望值写死在两份测试里就只能
//! 证明"两处各说各话"，证明不了"两处说的是同一件事"。

use std::path::PathBuf;

use rustlua_infra::bundle::BundleSet;
use rustlua_infra::bundle::plan::BundlePlan;
use rustlua_infra::bundle_plan::{distribution_layout, root_of_unit};
use rustlua_infra::paths::normalize_project_path;
use serde::Deserialize;

#[derive(Deserialize)]
struct Golden {
    schema: u32,
    plan: BundlePlan,
    units: Vec<UnitCase>,
    paths: Vec<PathCase>,
    containers: Vec<ContainerCase>,
    spellings: Vec<Spelling>,
    rejected: Vec<String>,
}

#[derive(Deserialize)]
struct UnitCase {
    unit: String,
    root: Option<String>,
}

#[derive(Deserialize)]
struct PathCase {
    path: String,
    group: String,
    unit: String,
    root: Option<String>,
}

#[derive(Deserialize)]
struct ContainerCase {
    path: String,
    root: Option<String>,
}

#[derive(Deserialize)]
struct Spelling {
    input: String,
    canonical: String,
}

fn load() -> Golden {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../scripts/test/fixtures/asset-routing-golden.json");
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("读不到黄金表 {}：{error}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("黄金表不是合法 JSON {}：{error}", path.display()))
}

#[test]
fn golden_table_is_a_legal_plan() {
    let golden = load();
    assert_eq!(golden.schema, 1, "黄金表 schema 不认识");
    // 走一次真正的解析器：这张表本身必须是"运行时愿意接受的计划表"，
    // 否则 JS 侧 `parsePlan` 与 Rust 侧 `BundlePlan::parse` 的判据已经分叉了。
    let bytes = serde_json::to_vec(&golden.plan).expect("计划表序列化");
    let plan = BundlePlan::parse(&bytes).expect("黄金计划表必须能通过 BundlePlan::parse");
    assert_eq!(plan.groups.len(), golden.plan.groups.len());
}

#[test]
fn units_map_to_the_same_roots_as_the_javascript_router() {
    let golden = load();
    // 纯映射是**全函数**（任何合法 unit 都有答案，包括计划里暂时没用到具名分包）；
    // 布局是**计划派生**的（只有真出现的单元才有一个根）。两者都要对得上黄金表，
    // 否则"JS 认识 sub:level_2 而 Rust 说它不存在"这种分叉会在加分包时才炸。
    let layout = distribution_layout(&golden.plan).expect("布局折算");
    for case in &golden.units {
        let mapped = root_of_unit(&case.unit).expect("unit 合法");
        assert_eq!(mapped, case.root, "单元 {} 的根不一致", case.unit);
        let in_plan = golden
            .plan
            .groups
            .iter()
            .any(|group| group.unit == case.unit);
        let found = layout.iter().find(|root| root.unit == case.unit);
        // `cdn` 可以有组，但永远不进包（布局里没有它的根）。
        if in_plan && case.root.is_some() {
            let root = found.unwrap_or_else(|| panic!("单元 {} 在计划里却没有布局根", case.unit));
            assert_eq!(
                Some(&root.root),
                case.root.as_ref(),
                "单元 {} 的布局根不一致",
                case.unit
            );
        } else {
            assert!(found.is_none(), "单元 {} 不该出现在布局里", case.unit);
        }
    }
}

#[test]
fn every_golden_path_is_claimed_by_exactly_one_group_in_the_declared_root() {
    let golden = load();
    let plan = BundlePlan::parse(&serde_json::to_vec(&golden.plan).unwrap()).expect("解析");
    let layout = distribution_layout(&plan).expect("布局折算");
    for case in &golden.paths {
        let owners: Vec<&str> = plan
            .groups
            .iter()
            .filter(|group| group.contains(&case.path))
            .map(|group| group.name.as_str())
            .collect();
        assert_eq!(
            owners,
            vec![case.group.as_str()],
            "路径 {} 的归属组不一致",
            case.path
        );
        let group = plan.group(&case.group).expect("组存在");
        assert_eq!(group.unit, case.unit, "路径 {} 的 unit 不一致", case.path);
        let root = layout.iter().find(|root| root.unit == case.unit);
        match &case.root {
            Some(expected) => {
                let root = root.unwrap_or_else(|| {
                    panic!("路径 {} 的单元 {} 不在布局里", case.path, case.unit)
                });
                assert_eq!(&root.root, expected, "路径 {} 的根不一致", case.path);
            }
            None => assert!(
                root.is_none(),
                "路径 {} 属于 cdn，不应出现在任何包根里",
                case.path
            ),
        }
    }
}

/// 容器路径本身也是路由表的一条（引擎按 `pack/bundles/x.rlb` 去挂载），
/// JS `planRootIndex` 会把它塞进 `pathRoot`。Rust 侧对应的真相是"容器所属组的根"。
#[test]
fn container_paths_route_to_their_groups_root() {
    let golden = load();
    let plan = BundlePlan::parse(&serde_json::to_vec(&golden.plan).unwrap()).expect("解析");
    let layout = distribution_layout(&plan).expect("布局折算");
    for case in &golden.containers {
        let group = plan
            .groups
            .iter()
            .find(|group| group.bundle == case.path)
            .unwrap_or_else(|| panic!("容器 {} 不被任何组声明", case.path));
        let root = layout.iter().find(|root| root.unit == group.unit);
        match &case.root {
            Some(expected) => {
                let root = root.unwrap_or_else(|| panic!("容器 {} 的单元不在布局里", case.path));
                assert_eq!(&root.root, expected, "容器 {} 的根不一致", case.path);
            }
            None => assert!(root.is_none(), "容器 {} 属于 cdn，不应进包", case.path),
        }
    }
}

#[test]
fn spellings_normalize_to_the_golden_canonical_form() {
    let golden = load();
    let set = BundleSet::new("/project");
    for case in &golden.spellings {
        assert_eq!(
            normalize_project_path(&case.input),
            Some(case.canonical.clone()),
            "写法 {:?} 的规范化结果不一致",
            case.input
        );
        // 读取端的键必须与规范化结果逐字相同（这正是容器条目的键）。
        let joined = PathBuf::from("/project").join(&case.canonical);
        assert_eq!(
            set.key_of(&joined),
            Some(case.canonical.clone()),
            "写法 {:?} 在 BundleSet 里的键不一致",
            case.input
        );
    }
}

#[test]
fn rejected_spellings_stay_rejected() {
    let golden = load();
    for raw in &golden.rejected {
        assert_eq!(
            normalize_project_path(raw),
            None,
            "{raw:?} 必须被拒绝（绝对路径/.. /空/NUL 都不能进容器键）"
        );
    }
}
