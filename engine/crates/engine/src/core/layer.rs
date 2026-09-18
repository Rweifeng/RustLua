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
}
