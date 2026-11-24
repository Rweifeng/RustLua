//! M84 P2-①：clipping 附件的导入期拒绝。
//!
//! 用两份手写的最小骨架做正负样本——不依赖官方示例文件（那些不入库，许可原因）。
//! 正样本能被 `spine2d` 正常解析（它支持 clipping），所以这里验的是**我们的筛查**，
//! 不是解析器能力。

use rustlua_spine::SpineError;
use rustlua_spine::validate;

const CLIPPING: &str = include_str!("fixtures/clipping_sample.json");
const NO_CLIPPING: &str = include_str!("fixtures/no_clipping_sample.json");

#[test]
fn clipping_attachment_is_rejected_with_a_locating_diagnostic() {
    let error = validate::reject_unsupported_features(CLIPPING.as_bytes(), false)
        .expect_err("含 clipping 的骨架必须被拒绝");
    let message = error.to_string();
    // 三处定位必须都在：作者要能直接在 Spine 编辑器里找到那一条。
    assert!(
        message.contains("clipping_sample") || message.contains("clip_slot"),
        "{message}"
    );
    assert!(
        message.contains("clip_slot"),
        "诊断必须点名 slot：{message}"
    );
    assert!(message.contains("clip_att"), "诊断必须点名附件：{message}");
    assert!(
        message.contains("skin `default`"),
        "诊断必须点名 skin：{message}"
    );
    assert!(
        matches!(&error, SpineError::UnsupportedClipping { count, .. } if *count == 1),
        "应报出恰好 1 个 clipping 附件，实际：{error:?}"
    );
}

#[test]
fn skeleton_without_clipping_passes_the_check() {
    validate::reject_unsupported_features(NO_CLIPPING.as_bytes(), false)
        .expect("不含 clipping 的骨架必须通过");
}
