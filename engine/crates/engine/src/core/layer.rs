//! WP6：渲染层的显式说法（World2D / Overlay2D / ScreenUI）。
//!
//! 裁定要求"层描述显式化"。此前"这一段顶点属于世界内容还是屏幕 UI"只体现在**谁调用了
//! 哪个 push 函数**里：世界文字、瓦片、精灵与屏幕 UI 都往同一份 overlay 顶点缓冲里追加，
//! 顺序靠调用点排列。世界 Sprite 深度通道落地后，这个区分第一次有了**行为后果**（世界内容
//! 需要世界坐标与深度，屏幕 UI 必须留在屏幕空间、不参与深度与相机），因此把它写成类型。
//!
//! 层次与 docs/54 的原则一致：**排序层 ≠ 深度**。`SpriteSorting` 决定同一层内的先后，
//! `RenderLayer` 决定"这段几何属于哪个渲染层"，两者的职责不重叠。

use std::fmt;

/// 一段几何属于哪个渲染层。
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum RenderLayer {
    /// 世界内容：精灵、瓦片、世界文字。跟随世界相机、可参与深度测试。
    #[default]
    World2D,
    /// 世界之上的叠加内容：HUD、调试绘制、性能浮层。屏幕空间、不参与深度。
    Overlay2D,
    /// 屏幕空间 UI：Text/Image/Button 等按设计分辨率布局的界面元素。**不受世界相机影响**。
    ScreenUI,
}

impl RenderLayer {
    /// 是否属于世界内容（需要世界坐标 + 世界相机 + 可参与深度）。
    #[must_use]
    pub const fn is_world(self) -> bool {
        matches!(self, Self::World2D)
    }

    /// 是否屏幕空间（不参与深度，也不受世界相机 pan/zoom 影响）。
    #[must_use]
    pub const fn is_screen_space(self) -> bool {
        !self.is_world()
    }

    /// 稳定短名（日志与诊断用）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::World2D => "world2d",
            Self::Overlay2D => "overlay2d",
            Self::ScreenUI => "screen_ui",
        }
    }
}

impl fmt::Display for RenderLayer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 2D 叠加通道的**绘制顺序**：世界内容 → 叠加内容 → 屏幕 UI。
///
/// 为什么要有这个常量：`RenderLayer` 只是类型，如果渲染器照顶点缓冲的先后画，层的次序仍然
/// 由**调用点排版**决定——某次不小心把 ScreenUI 推到 Overlay2D 之前，HUD 就会被调试线盖住，
/// 而类型检查不会报错。渲染器现在按这个顺序分桶（[`layered_indices`]），层因此是契约而不是注释。
///
/// 宿主本来就按这个顺序产出顶点（世界通道 → 调试绘制/世界文字 → 屏幕 UI），因此分桶不改变任何
/// 现有画面的像素；它挡住的是"以后悄悄错序"这一类改动。
pub const OVERLAY_DRAW_ORDER: [RenderLayer; 3] = [
    RenderLayer::World2D,
    RenderLayer::Overlay2D,
    RenderLayer::ScreenUI,
];

/// 只取**一个层**的 draw 索引（桶内保持原序）。
///
/// `RenderPlan` 里 `Legacy2DOverlay` 与 `ScreenUI` 是两条独立通道，渲染器因此按通道取桶；
/// 这条函数与 [`layered_indices`] 共用同一套"层怎么比"的规则，不会各自演化。
#[must_use]
pub fn layer_indices<T>(
    items: &[T],
    layer_of: impl Fn(&T) -> RenderLayer,
    layer: RenderLayer,
) -> Vec<usize> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| layer_of(item) == layer)
        .map(|(index, _)| index)
        .collect()
}

/// 按 [`OVERLAY_DRAW_ORDER`] 把一段几何的 draw 顺序重排成**桶内保持原序**的索引列表。
///
/// 返回索引而不是克隆元素：`SceneDraw` 不大，但每帧每 draw 克隆一次没有必要。同一层的 draw
/// 之间顺序不变（半透明叠加靠的就是这个），因此"分层"与"层内排序"是两件事、不会互相干扰。
#[must_use]
pub fn layered_indices<T>(items: &[T], layer_of: impl Fn(&T) -> RenderLayer) -> Vec<usize> {
    let mut ordered = Vec::with_capacity(items.len());
    for layer in OVERLAY_DRAW_ORDER {
        ordered.extend(layer_indices(items, &layer_of, layer));
    }
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 层次划分的两条硬性质：世界层只有一个，屏幕空间包含其余两者。
    #[test]
    fn world_and_screen_space_partition_the_layers() {
        assert!(RenderLayer::World2D.is_world());
        assert!(!RenderLayer::World2D.is_screen_space());
        for layer in [RenderLayer::Overlay2D, RenderLayer::ScreenUI] {
            assert!(layer.is_screen_space(), "{layer} 应属于屏幕空间");
            assert!(!layer.is_world(), "{layer} 不应被当作世界内容");
        }
        assert_eq!(RenderLayer::default(), RenderLayer::World2D);
        assert_eq!(RenderLayer::ScreenUI.as_str(), "screen_ui");
    }

    /// 分桶的两条硬性质：**桶内原序** + **桶间按既定层次**。
    #[test]
    fn layered_indices_keep_within_layer_order_and_apply_canonical_layer_order() {
        use RenderLayer::{Overlay2D, ScreenUI, World2D};
        // 已经排好的输入 → 恒等（宿主当前就是这样产出的，因此像素不该变）。
        let canonical = [World2D, World2D, Overlay2D, ScreenUI, ScreenUI];
        assert_eq!(
            layered_indices(&canonical, |layer| *layer),
            vec![0, 1, 2, 3, 4]
        );

        // 错序输入 → 按层次重排，桶内先后不变（同层两个元素的相对顺序保留）。
        let scrambled = [ScreenUI, Overlay2D, ScreenUI, World2D, Overlay2D];
        assert_eq!(
            layered_indices(&scrambled, |layer| *layer),
            vec![3, 1, 4, 0, 2]
        );

        // 空输入不产生 draw。
        assert!(layered_indices(&[], |layer: &RenderLayer| *layer).is_empty());
    }

    /// 每个层都必须出现在顺序常量里——漏一层就意味着那层的 draw 会被静默丢掉。
    #[test]
    fn overlay_draw_order_covers_every_layer_once() {
        let mut seen = OVERLAY_DRAW_ORDER;
        seen.sort_by_key(|layer| layer.as_str());
        let mut expected = [
            RenderLayer::World2D,
            RenderLayer::Overlay2D,
            RenderLayer::ScreenUI,
        ];
        expected.sort_by_key(|layer| layer.as_str());
        assert_eq!(seen, expected);
        assert_eq!(OVERLAY_DRAW_ORDER.len(), 3, "层数变化必须同步改渲染器分桶");
    }
}
