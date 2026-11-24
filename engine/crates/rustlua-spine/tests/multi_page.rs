//! M84 P2-④：多图集页的 draw 必须各自指向**自己的**页路径。
//!
//! 单页骨架掩盖不了问题：所有 draw 的 `texture` 都是同一个字符串，即使代码把页名
//! 硬编码成"第一页"也照样能过。双页夹具是唯一能证伪这件事的最小形状。
//!
//! 上游到下游的分工：本层只保证 `SpineDraw.texture` 是**该 draw 所属页**的路径；
//! 宿主 `collect_spine_mesh` 把它逐个登记进 `texture_paths`（同一套 `texture_id_of`，
//! 因此也拿到 `AssetHandle`），GPU 侧再按这个下标取绑定组。

use rustlua_spine::{SpineData, SpineSkeletonComponent};

const TWO_PAGE_JSON: &str = include_str!("fixtures/two_page.json");
const TWO_PAGE_ATLAS: &str = include_str!("fixtures/two_page.atlas");

#[test]
fn draws_on_different_pages_report_their_own_page_path() {
    let data = SpineData::from_bytes(
        TWO_PAGE_JSON.as_bytes(),
        false,
        TWO_PAGE_ATLAS,
        "4.3.75-beta",
        "",
    )
    .expect("双页夹具必须能加载");
    let component = SpineSkeletonComponent {
        data: "fixture".to_owned(),
        ..SpineSkeletonComponent::default()
    };
    let mut player = data.instantiate(&component).expect("实例化成功");
    player.advance(1.0 / 60.0).expect("推进成功");

    let pages: Vec<&str> = player
        .output()
        .draws
        .iter()
        .map(|draw| &*draw.texture)
        .collect();
    assert!(
        pages.iter().any(|page| page.contains("page_a")),
        "应有一条 draw 指向 page_a，实际：{pages:?}"
    );
    assert!(
        pages.iter().any(|page| page.contains("page_b")),
        "应有一条 draw 指向 page_b，实际：{pages:?}"
    );
    // 两条 draw 必须指向**不同**的页——都指向同一个页说明页名被写死了。
    let unique: std::collections::BTreeSet<&str> = pages.iter().copied().collect();
    assert_eq!(
        unique.len(),
        2,
        "两条 draw 应分别落在两页上，实际只有 {} 个不同页：{pages:?}",
        unique.len()
    );
}

/// atlas 页名是**相对 atlas 文件**的。宿主传了 `atlas_dir` 时，输出的 draw 路径必须是
/// 工程内路径，否则页会被当成工程根下的同名文件——后者恰好存在时静默贴错图。
#[test]
fn page_paths_are_resolved_against_the_atlas_directory() {
    let data = SpineData::from_bytes(
        TWO_PAGE_JSON.as_bytes(),
        false,
        TWO_PAGE_ATLAS,
        "4.3.75-beta",
        "assets/spine",
    )
    .expect("双页夹具必须能加载");
    let component = SpineSkeletonComponent {
        data: "fixture".to_owned(),
        ..SpineSkeletonComponent::default()
    };
    let mut player = data.instantiate(&component).expect("实例化成功");
    player.advance(1.0 / 60.0).expect("推进成功");

    for draw in &player.output().draws {
        assert!(
            draw.texture.starts_with("assets/spine/"),
            "页路径应解析成工程内路径，实际：{}",
            draw.texture
        );
    }
}

/// 页名自带目录时不重复拼接（有些导出工作流会把目录写进 atlas）。
#[test]
fn page_paths_that_already_carry_a_directory_are_left_alone() {
    // 骨架有两个 slot（region_a / region_b），所以 atlas 必须两页都在——
    // 少一页会让页表查询落空（`copy_to_output` 有 debug 断言直接炸），
    // 那样测的就不是"页名带目录"这件事了。
    let atlas = "sub/page_a.png\nsize: 64, 64\nfilter: Linear, Linear\nregion_a\n\tbounds: 0, 0, 32, 32\n\n\
                 sub/page_b.png\nsize: 64, 64\nfilter: Linear, Linear\nregion_b\n\tbounds: 0, 0, 32, 32\n";
    let data = SpineData::from_bytes(
        TWO_PAGE_JSON.as_bytes(),
        false,
        atlas,
        "4.3.75-beta",
        "assets/spine",
    )
    .expect("夹具必须能加载");
    let component = SpineSkeletonComponent {
        data: "fixture".to_owned(),
        ..SpineSkeletonComponent::default()
    };
    let mut player = data.instantiate(&component).expect("实例化成功");
    player.advance(1.0 / 60.0).expect("推进成功");
    let output = player.output();
    // 不写 `if let`：夹具里有附件，没有 draw 就说明夹具本身坏了，必须红。
    assert!(!output.draws.is_empty(), "夹具应产生至少一条 draw");
    for draw in &output.draws {
        assert!(
            draw.texture.starts_with("sub/"),
            "页名已带目录时不应再拼 atlas_dir，实际：{}",
            draw.texture
        );
    }
}
