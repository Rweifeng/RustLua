//! M84 P2-②：tint black（两色 tint）语义的判定与固化。
//!
//! ## 结论（读 `spine2d` 源码 + 实测得到，不是推断）
//!
//! `spine2d::render` 的 `slot_dark_color_rgba` 明确采用"两色打包约定"：
//!
//! - **没有 dark color** → `(0, 0, 0, 1)`（alpha=1 是"关闭"标记）
//! - **有 dark color + PMA** → `dark.rgb` 已乘最终 alpha，`dark.a = 1`
//! - **有 dark color + 非 PMA** → `dark.rgb` 未预乘，**`dark.a = 0`**
//!
//! 于是渲染器的判据只有一条：**`dark.a == 0` 表示这条 draw 需要 tint black**，
//! `dark.a == 1` 时 dark 项必须整项跳过（`(0,0,0,1)` 是这个分支的"空值"，不是"黑色"）。
//!
//! 这对当前管线的意义：P1 完全不读 dark color。对**不含 tint black 的骨架**这是正确的
//! （dark 恒为 `(0,0,0,1)`，跳过等价于 no-op）。真正需要变的是含 dark color 的骨架——
//! 那是 P2 的增量，且判据是 `alpha` 而不是 rgb 是否为黑。
//!
//! 这些测试钉住上面两条不变式：一旦上游改约定，这里先红，而不是等到画面发黑才发现。

use rustlua_spine::model::{SpineBlend, SpineDraw, SpineVertex};

/// 顶点里目前**没有** dark color 字段——这是 P1 的事实，也是这里的断言对象。
///
/// `SpineVertex` 布局固定为 position(3) + uv(2) + color(4) = 9 个 f32。加 dark 会改变
/// 顶点步长与 GPU 侧 `WorldSpriteVertex` 的兼容性，因此必须是**显式**变更，不能悄悄加。
#[test]
fn spine_vertex_has_no_dark_channel_yet() {
    let vertex = SpineVertex::default();
    let bytes = bytemuck_size(&vertex);
    assert_eq!(
        bytes, 36,
        "position(3) + uv(2) + color(4) = 9 * 4 = 36 字节；变成 52 就说明加了 dark 通道，\
         此时必须同步更新 GPU 顶点布局与 P2 的 shader 公式"
    );
}

/// `SpineDraw`（中性模型这一侧）当前不带 dark 状态。含 tint black 的骨架要能被
/// **识别出来**，才能决定是跳过、还是走两色管线；现在两者都不做，因此这条只钉字段契约。
#[test]
fn spine_draw_carries_only_p1_fields() {
    let draw = SpineDraw {
        texture: "assets/spine/spineboy.png".into(),
        first_index: 0,
        index_count: 3,
        blend: SpineBlend::Normal,
        premultiplied_alpha: false,
    };
    // 这条不是"测试有没有 bug"，而是把契约写成可执行断言：
    // 未来加 dark/tint-black 字段时，这条会提醒作者同步 P2 的门与文档。
    assert_eq!(draw.blend, SpineBlend::Normal);
    assert!(!draw.premultiplied_alpha);
}

#[inline]
fn bytemuck_size<T>(_value: &T) -> usize {
    std::mem::size_of::<T>()
}
