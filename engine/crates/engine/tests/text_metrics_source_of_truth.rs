//! 字号基准（`scale == 1.0` 的 em 像素高）只有一个事实源。
//!
//! 这条对拍原本住在 `rustlua-render` 自己的单测里，为了看见 ui 那一侧，render 的
//! `[dev-dependencies]` 得声明 `rustlua-ui`——而渲染的文本度量与 UI 组件之间没有任何依赖关系。
//! 字面量收进 `rustlua_math::text_metrics::TEXT_BASE_PX` 之后，两个 crate 的公开别名各自指过去，
//! 门挪到这里：engine 同时依赖三个 crate 且只有一份类型，不需要再拉一条反向边。

/// render 与 ui 的公开别名都必须等于 math 里那一个基准值。
///
/// 任何一侧被重新写死成别的数（历史上的双写就是这个形态）都会在这里判红。
#[test]
fn text_base_px_aliases_point_at_one_source() {
    let base = rustlua_math::text_metrics::TEXT_BASE_PX;
    assert_eq!(
        rustlua_render::text::EM_PX,
        base,
        "render 的 `EM_PX` 必须指回 `text_metrics::TEXT_BASE_PX`"
    );
    assert_eq!(
        rustlua_ui::ui::UI_TEXT_BASE_PX,
        base,
        "ui 的 `UI_TEXT_BASE_PX` 必须指回 `text_metrics::TEXT_BASE_PX`"
    );
}

/// ui 的 `font_size → scale` 换算必须按同一个基准做除法。
///
/// 只测别名相等不够：换算函数抄走一份常量时，值仍然对得上但口径会随基准变化脱钩。
#[test]
fn ui_scale_conversion_uses_the_same_base() {
    let base = rustlua_math::text_metrics::TEXT_BASE_PX;
    assert_eq!(rustlua_ui::ui::ui_text_effective_scale(base), 1.0);
    assert_eq!(rustlua_ui::ui::ui_text_effective_scale(base * 2.0), 2.0);
}
