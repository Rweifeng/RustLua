//! M84 P3-1：Spine 资产经 `pack` 压缩后必须仍能被运行时读出来。
//!
//! 打包改了 `.skel` / `.spineatlas` / `.spine` 的压缩白名单，收益是体积（实测 -32% / -60%），
//! 但**压缩本身是危险动作**：如果解压路径有一处没接上，现象是"进游戏黑屏"而不是"体积小了"。
//! 所以这里不看体积，只看**往返**：`pack::encode` → `pack::decode` → 真实解析 → 出 draw。

use rustlua_spine::{SpineData, SpineSkeletonComponent};

const TWO_PAGE_JSON: &str = include_str!("fixtures/two_page.json");
const TWO_PAGE_ATLAS: &str = include_str!("fixtures/two_page.atlas");

/// 压缩 → 解压往返后，骨架与图集都还能解析并产出几何。
#[test]
fn spine_assets_survive_a_pack_round_trip() {
    // `pack` 的编码入口只在压缩档生效；用与 assetc 相同的那一对函数。
    let skeleton_packed = rustlua_infra::pack::encode(TWO_PAGE_JSON.as_bytes(), true, false);
    let atlas_packed = rustlua_infra::pack::encode(TWO_PAGE_ATLAS.as_bytes(), true, false);

    // 真的压小了，否则这条测试就没在验压缩路径。
    assert!(
        skeleton_packed.len() < TWO_PAGE_JSON.len(),
        "夹具太小导致没压小，测试失去意义：{} vs {}",
        skeleton_packed.len(),
        TWO_PAGE_JSON.len()
    );

    let skeleton = rustlua_infra::pack::decode(&skeleton_packed).expect("解压骨骼");
    let atlas = rustlua_infra::pack::decode(&atlas_packed).expect("解压图集");
    assert_eq!(skeleton, TWO_PAGE_JSON.as_bytes(), "解压结果必须逐字节还原");
    assert_eq!(atlas, TWO_PAGE_ATLAS.as_bytes(), "解压结果必须逐字节还原");

    let atlas_text = String::from_utf8(atlas).expect("图集是 UTF-8");
    let data = SpineData::from_bytes(&skeleton, false, &atlas_text, "4.3.75-beta", "")
        .expect("解压后的骨架必须能解析");
    let component = SpineSkeletonComponent {
        data: "fixture".to_owned(),
        ..SpineSkeletonComponent::default()
    };
    let mut player = data.instantiate(&component).expect("实例化成功");
    player.advance(1.0 / 60.0).expect("推进成功");
    assert!(
        !player.output().draws.is_empty(),
        "往返之后必须仍能产出 draw——压缩链断在这里的症状是进游戏黑屏"
    );
}
