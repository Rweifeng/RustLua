//! M84 P2-③：非 Normal blend 的 draw 必须**原样**出现在中性输出里。
//!
//! 这一层不判断"能不能画"（那是 GPU 的事），只保证 blend 信息不在链路里丢失。
//! P1 的教训是：如果输出层把 blend 归一成 Normal，GPU 侧再想分管线也无从判断——
//! 而症状是"光效画成了普通叠加"，截图看着只是"不太亮"。

use rustlua_spine::model::SpineBlend;
use rustlua_spine::{SpineData, SpineSkeletonComponent};

const ADDITIVE_SAMPLE: &str = include_str!("fixtures/additive_slot_sample.json");

/// 夹具骨架配套的真实 atlas：region 与页的映射必须是**真的**，否则每帧都会走到
/// "页表查不到"的兜底分支（`backend::copy_to_output` 里有 debug 断言会直接炸）。
const ATLAS: &str = include_str!("fixtures/additive_slot.atlas");

fn load() -> SpineData {
    SpineData::from_bytes(ADDITIVE_SAMPLE.as_bytes(), false, ATLAS, "4.3.75-beta", "")
        .expect("夹具骨架必须能加载")
}

#[test]
fn additive_slots_survive_into_the_neutral_draw_list() {
    let data = load();
    let component = SpineSkeletonComponent {
        data: "fixture".to_owned(),
        animation: String::new(),
        ..SpineSkeletonComponent::default()
    };
    let mut player = data.instantiate(&component).expect("实例化成功");
    // 推一帧让几何产出（没有动画时 setup_pose 就够，但 advance 才是真实路径）。
    player.advance(1.0 / 60.0).expect("推进成功");
    let output = player.output();
    let blends: Vec<SpineBlend> = output.draws.iter().map(|draw| draw.blend).collect();
    assert!(
        blends.contains(&SpineBlend::Additive),
        "夹具里有 additive slot，输出里必须出现 Additive draw，实际：{blends:?}"
    );
    assert!(
        blends.contains(&SpineBlend::Normal),
        "其余 slot 仍是 Normal，实际：{blends:?}"
    );
}

#[test]
fn blend_kinds_round_trip_through_as_str() {
    for (blend, expected) in [
        (SpineBlend::Normal, "normal"),
        (SpineBlend::Additive, "additive"),
        (SpineBlend::Multiply, "multiply"),
        (SpineBlend::Screen, "screen"),
    ] {
        assert_eq!(blend.as_str(), expected);
        assert_eq!(blend.to_string(), expected);
    }
}
