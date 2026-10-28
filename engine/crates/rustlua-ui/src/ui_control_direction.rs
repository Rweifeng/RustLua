//! 控件方向（对齐 Unity `Slider.Direction` / `Scrollbar.Direction`）。
//!
//! 四个方向只决定两件事：指针位置 → 0..=1 的比例，以及填充/滑块从哪条边开始长。
//! 控件本身不存像素，这套换算是纯函数——布局、命中与 Lua 取值共用同一份定义，
//! "点在哪 = 值是多少 = 填充画多长"因此不会分叉。

use crate::ui_layout::UiRect;

/// The travel axis and its sign for a value control.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ControlDirection {
    #[default]
    LeftToRight,
    RightToLeft,
    BottomToTop,
    TopToBottom,
}

impl ControlDirection {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LeftToRight => "left_to_right",
            Self::RightToLeft => "right_to_left",
            Self::BottomToTop => "bottom_to_top",
            Self::TopToBottom => "top_to_bottom",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "left_to_right" => Some(Self::LeftToRight),
            "right_to_left" => Some(Self::RightToLeft),
            "bottom_to_top" => Some(Self::BottomToTop),
            "top_to_bottom" => Some(Self::TopToBottom),
            _ => None,
        }
    }

    #[must_use]
    pub const fn is_vertical(self) -> bool {
        matches!(self, Self::BottomToTop | Self::TopToBottom)
    }

    /// 指针沿移动轴的位置，夹到 `0..=1`；零长度轴恒为 0。
    #[must_use]
    pub fn fraction(self, rect: UiRect, point: [f32; 2]) -> f32 {
        let (start, span, coordinate) = if self.is_vertical() {
            (rect.y, rect.h, point[1])
        } else {
            (rect.x, rect.w, point[0])
        };
        if span <= 0.0 || !span.is_finite() {
            return 0.0;
        }
        let raw = ((coordinate - start) / span).clamp(0.0, 1.0);
        if matches!(self, Self::RightToLeft | Self::BottomToTop) {
            1.0 - raw
        } else {
            raw
        }
    }

    /// 从最小端开始的填充段 `(起点, 长度)`：长度 = 轴长 × fraction，
    /// 反向方向从另一端对齐（填充贴着的仍是"值小"的那条边）。
    #[must_use]
    pub fn fill_span(self, rect: UiRect, fraction: f32) -> (f32, f32) {
        let fraction = finite_fraction(fraction);
        let (start, span) = self.axis(rect);
        let length = span * fraction;
        let start = match self {
            Self::RightToLeft | Self::BottomToTop => start + span - length,
            _ => start,
        };
        (start, length)
    }

    /// 滑块起点：可用行程 = 轴长 - 滑块长（滑块比轨道长时就地贴边）。
    #[must_use]
    pub fn handle_start(self, rect: UiRect, handle_len: f32, fraction: f32) -> f32 {
        let fraction = finite_fraction(fraction);
        let (start, span) = self.axis(rect);
        let len = handle_len.max(0.0).min(span.max(0.0));
        let travel = (span - len).max(0.0);
        match self {
            Self::RightToLeft | Self::BottomToTop => start + span - len - travel * fraction,
            _ => start + travel * fraction,
        }
    }

    fn axis(self, rect: UiRect) -> (f32, f32) {
        if self.is_vertical() {
            (rect.y, rect.h)
        } else {
            (rect.x, rect.w)
        }
    }
}

fn finite_fraction(fraction: f32) -> f32 {
    if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> UiRect {
        UiRect::new(10.0, 20.0, 100.0, 40.0)
    }

    #[test]
    fn pointer_fraction_follows_direction_and_clamps() {
        let rect = rect();
        for direction in [ControlDirection::LeftToRight, ControlDirection::RightToLeft] {
            assert!((direction.fraction(rect, [60.0, 40.0]) - 0.5).abs() < 1e-6);
        }
        assert_eq!(
            ControlDirection::LeftToRight.fraction(rect, [0.0, 0.0]),
            0.0
        );
        assert_eq!(
            ControlDirection::RightToLeft.fraction(rect, [10.0, 20.0]),
            1.0
        );
        assert_eq!(
            ControlDirection::RightToLeft.fraction(rect, [110.0, 20.0]),
            0.0
        );
        assert_eq!(
            ControlDirection::TopToBottom.fraction(rect, [0.0, 30.0]),
            0.25
        );
        assert_eq!(
            ControlDirection::BottomToTop.fraction(rect, [0.0, 30.0]),
            0.75
        );
        // 轴外的点夹到端点，而不是返回负数/大于 1。
        assert_eq!(
            ControlDirection::LeftToRight.fraction(rect, [-50.0, 0.0]),
            0.0
        );
        assert_eq!(
            ControlDirection::LeftToRight.fraction(rect, [500.0, 0.0]),
            1.0
        );
    }

    #[test]
    fn zero_length_axis_is_zero_not_nan() {
        let flat = UiRect::new(10.0, 20.0, 0.0, 0.0);
        assert_eq!(
            ControlDirection::LeftToRight.fraction(flat, [10.0, 20.0]),
            0.0
        );
        assert_eq!(
            ControlDirection::TopToBottom.fill_span(flat, 0.5),
            (20.0, 0.0)
        );
        assert_eq!(
            ControlDirection::BottomToTop.handle_start(flat, 8.0, 0.5),
            20.0
        );
    }

    #[test]
    fn fill_spans_anchor_to_the_minimum_edge() {
        let rect = rect();
        assert_eq!(
            ControlDirection::LeftToRight.fill_span(rect, 0.25),
            (10.0, 25.0)
        );
        assert_eq!(
            ControlDirection::RightToLeft.fill_span(rect, 0.25),
            (85.0, 25.0)
        );
        assert_eq!(
            ControlDirection::TopToBottom.fill_span(rect, 0.25),
            (20.0, 10.0)
        );
        assert_eq!(
            ControlDirection::BottomToTop.fill_span(rect, 0.25),
            (50.0, 10.0)
        );
        // 越界比例按端点处理。
        assert_eq!(
            ControlDirection::LeftToRight.fill_span(rect, 2.0),
            (10.0, 100.0)
        );
        assert_eq!(
            ControlDirection::LeftToRight.fill_span(rect, f32::NAN),
            (10.0, 0.0)
        );
    }

    #[test]
    fn handle_slides_inside_the_remaining_travel() {
        let rect = rect();
        // 水平轨道 100，滑块 20 ⇒ 行程 80。
        assert_eq!(
            ControlDirection::LeftToRight.handle_start(rect, 20.0, 0.0),
            10.0
        );
        assert_eq!(
            ControlDirection::LeftToRight.handle_start(rect, 20.0, 0.5),
            50.0
        );
        assert_eq!(
            ControlDirection::LeftToRight.handle_start(rect, 20.0, 1.0),
            90.0
        );
        assert_eq!(
            ControlDirection::RightToLeft.handle_start(rect, 20.0, 0.0),
            90.0
        );
        assert_eq!(
            ControlDirection::RightToLeft.handle_start(rect, 20.0, 1.0),
            10.0
        );
        // 垂直轨道 40，滑块 8 ⇒ 行程 32。
        assert_eq!(
            ControlDirection::TopToBottom.handle_start(rect, 8.0, 0.5),
            36.0
        );
        assert_eq!(
            ControlDirection::BottomToTop.handle_start(rect, 8.0, 0.5),
            36.0
        );
        // 滑块比轨道长：贴住起点，不产生负行程。
        assert_eq!(
            ControlDirection::LeftToRight.handle_start(rect, 200.0, 0.5),
            10.0
        );
    }
}
