//! `Slider`：连续/整数取值控件（对齐 Unity `Slider`）。
//!
//! 组件串：
//! `min=0;max=1;value=0.5;direction=left_to_right;fill=Fill;handle=Handle`。
//! 必填三件套是 `min/max/value`（`value` 缺省 = `min`）；`fill` / `handle` 是子实体名，
//! 宿主按当前值改写它们的布局矩形——填充条贴最小端，滑块沿剩余行程滑动。
//!
//! Unity 的 `wholeNumbers` 在这里同名：`value` 按整数吸附，方向键/拖拽都只落在整数上。
//! `direction` 同时决定指针位置到值的映射和填充从哪条边开始长，两条路径共用
//! [`ControlDirection`]，不会出现"拖到右边值变小、填充却从左边长"的分叉。

use crate::ui::{escape_text, split_fields, unescape_text};
use crate::ui_control_direction::ControlDirection;
use crate::ui_layout::UiRect;

#[derive(Clone, Debug, PartialEq)]
pub struct Slider {
    pub min: f32,
    pub max: f32,
    pub value: f32,
    pub whole_numbers: bool,
    pub direction: ControlDirection,
    pub fill: String,
    pub handle: String,
}

impl Default for Slider {
    fn default() -> Self {
        Self {
            min: 0.0,
            max: 1.0,
            value: 0.0,
            whole_numbers: false,
            direction: ControlDirection::LeftToRight,
            fill: String::new(),
            handle: String::new(),
        }
    }
}

impl Slider {
    pub fn from_component_str(raw: &str) -> Result<Self, String> {
        let mut slider = Self::default();
        let mut seen = [false; 7];
        for field in split_fields(raw) {
            let field = field.trim();
            if field.is_empty() {
                continue;
            }
            let Some((key, value)) = field.split_once('=') else {
                return Err(format!("expected key=value, got `{field}`"));
            };
            let value = value.trim();
            let index = match key {
                "min" => 0,
                "max" => 1,
                "value" => 2,
                "whole_numbers" => 3,
                "direction" => 4,
                "fill" => 5,
                "handle" => 6,
                other => return Err(format!("unknown Slider field `{other}`")),
            };
            if seen[index] {
                return Err(format!("duplicate `{key}=`"));
            }
            seen[index] = true;
            match key {
                "min" => slider.min = parse_finite("min", value)?,
                "max" => slider.max = parse_finite("max", value)?,
                "value" => slider.value = parse_finite("value", value)?,
                "whole_numbers" => {
                    slider.whole_numbers = match value {
                        "true" => true,
                        "false" => false,
                        other => {
                            return Err(format!(
                                "`whole_numbers` must be true or false, got `{other}`"
                            ));
                        }
                    };
                }
                "direction" => {
                    slider.direction = ControlDirection::parse(value)
                        .ok_or_else(|| format!("invalid direction `{value}`"))?;
                }
                "fill" => slider.fill = unescape_text(value),
                "handle" => slider.handle = unescape_text(value),
                _ => unreachable!("key was matched above"),
            }
        }
        if slider.max <= slider.min {
            return Err(format!(
                "max ({}) must be greater than min ({})",
                slider.max, slider.min
            ));
        }
        if slider.value < slider.min || slider.value > slider.max {
            return Err(format!(
                "value ({}) must be within {}..={}",
                slider.value, slider.min, slider.max
            ));
        }
        if slider.whole_numbers {
            slider.value = slider.value.round().clamp(slider.min, slider.max);
        }
        Ok(slider)
    }

    #[must_use]
    pub fn to_component_str(&self) -> String {
        let mut encoded = format!("min={};max={};value={}", self.min, self.max, self.value);
        if self.whole_numbers {
            encoded.push_str(";whole_numbers=true");
        }
        if self.direction != ControlDirection::LeftToRight {
            encoded.push_str(&format!(";direction={}", self.direction.as_str()));
        }
        if !self.fill.is_empty() {
            encoded.push_str(&format!(";fill={}", escape_text(&self.fill)));
        }
        if !self.handle.is_empty() {
            encoded.push_str(&format!(";handle={}", escape_text(&self.handle)));
        }
        encoded
    }

    #[must_use]
    pub fn normalized(&self) -> f32 {
        let span = self.max - self.min;
        if span <= 0.0 || !span.is_finite() {
            return 0.0;
        }
        ((self.value - self.min) / span).clamp(0.0, 1.0)
    }

    /// 按比例写值（拖拽/键盘都走这里）。返回是否真的变了。
    pub fn set_normalized(&mut self, fraction: f32) -> bool {
        if !fraction.is_finite() {
            return false;
        }
        let fraction = fraction.clamp(0.0, 1.0);
        self.set_value(self.min + (self.max - self.min) * fraction)
    }

    /// 按值写值：先夹到 `min..=max`，整数模式再吸附到最近整数。
    pub fn set_value(&mut self, value: f32) -> bool {
        if !value.is_finite() {
            return false;
        }
        let mut value = value.clamp(self.min, self.max);
        if self.whole_numbers {
            value = value.round().clamp(self.min, self.max);
        }
        let changed = value != self.value;
        self.value = value;
        changed
    }

    /// 填充条矩形：移动轴按比例，交叉轴铺满控件。
    #[must_use]
    pub fn fill_rect(&self, control: UiRect) -> UiRect {
        let (start, length) = self.direction.fill_span(control, self.normalized());
        if self.direction.is_vertical() {
            UiRect::new(control.x, start, control.w, length)
        } else {
            UiRect::new(start, control.y, length, control.h)
        }
    }

    /// 滑块矩形：保留子实体原有尺寸（沿轴不超过控件轴长），沿轴滑动、交叉轴居中。
    #[must_use]
    pub fn handle_rect(&self, control: UiRect, handle_size: [f32; 2]) -> UiRect {
        if self.direction.is_vertical() {
            let len = handle_size[1].max(0.0).min(control.h.max(0.0));
            let cross = handle_size[0].max(0.0).min(control.w.max(0.0));
            let start = self.direction.handle_start(control, len, self.normalized());
            let x = control.x + (control.w - cross) * 0.5;
            UiRect::new(x, start, cross, len)
        } else {
            let len = handle_size[0].max(0.0).min(control.w.max(0.0));
            let cross = handle_size[1].max(0.0).min(control.h.max(0.0));
            let start = self.direction.handle_start(control, len, self.normalized());
            let y = control.y + (control.h - cross) * 0.5;
            UiRect::new(start, y, len, cross)
        }
    }
}

fn parse_finite(key: &str, raw: &str) -> Result<f32, String> {
    let value = raw
        .parse::<f32>()
        .map_err(|_| format!("invalid `{key}` value `{raw}`"))?;
    if !value.is_finite() {
        return Err(format!("invalid `{key}` value `{raw}`"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-6,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn parses_defaults_and_round_trips() {
        let slider = Slider::from_component_str("").unwrap();
        assert_eq!(slider, Slider::default());
        assert_eq!(slider.to_component_str(), "min=0;max=1;value=0");

        let slider = Slider {
            min: -10.0,
            max: 10.0,
            value: 2.5,
            whole_numbers: false,
            direction: ControlDirection::RightToLeft,
            fill: "Fill;Bar".to_owned(),
            handle: "Knob".to_owned(),
        };
        let encoded = slider.to_component_str();
        assert!(encoded.contains("direction=right_to_left"), "{encoded}");
        assert!(encoded.contains("fill=Fill\\;Bar"), "{encoded}");
        assert_eq!(
            Slider::from_component_str(&encoded).unwrap(),
            slider,
            "{encoded}"
        );
    }

    #[test]
    fn rejects_bad_ranges_values_and_duplicates() {
        for bad in [
            "min=0;max=0;value=0",
            "min=2;max=1;value=1.5",
            "min=0;max=1;value=2",
            "min=0;max=1;value=NaN",
            "min=0;min=1;value=0.5",
            "whole_numbers=maybe",
            "direction=diagonal",
            "slider=1",
        ] {
            assert!(Slider::from_component_str(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn whole_numbers_snap_on_parse_and_set() {
        let mut slider =
            Slider::from_component_str("min=0;max=10;value=3.6;whole_numbers=true").unwrap();
        assert_eq!(slider.value, 4.0);
        assert!(slider.set_normalized(0.52));
        assert_eq!(slider.value, 5.0);
        assert!(!slider.set_normalized(0.52));
        assert!(slider.set_value(-3.0));
        assert_eq!(slider.value, 0.0);
    }

    #[test]
    fn fill_rect_grows_from_the_minimum_edge() {
        let control = UiRect::new(100.0, 50.0, 200.0, 20.0);
        let mut slider = Slider {
            min: 0.0,
            max: 1.0,
            value: 0.25,
            ..Slider::default()
        };
        assert_eq!(
            slider.fill_rect(control),
            UiRect::new(100.0, 50.0, 50.0, 20.0)
        );
        slider.direction = ControlDirection::RightToLeft;
        assert_eq!(
            slider.fill_rect(control),
            UiRect::new(250.0, 50.0, 50.0, 20.0)
        );
        slider.direction = ControlDirection::TopToBottom;
        assert_eq!(
            slider.fill_rect(control),
            UiRect::new(100.0, 50.0, 200.0, 5.0)
        );
        slider.direction = ControlDirection::BottomToTop;
        assert_eq!(
            slider.fill_rect(control),
            UiRect::new(100.0, 65.0, 200.0, 5.0)
        );
    }

    #[test]
    fn handle_rect_keeps_authored_size_and_centers_cross_axis() {
        let control = UiRect::new(100.0, 50.0, 200.0, 20.0);
        let mut slider = Slider {
            value: 0.5,
            ..Slider::default()
        };
        assert_eq!(
            slider.handle_rect(control, [20.0, 10.0]),
            UiRect::new(190.0, 55.0, 20.0, 10.0)
        );
        slider.value = 0.0;
        assert_eq!(
            slider.handle_rect(control, [20.0, 10.0]),
            UiRect::new(100.0, 55.0, 20.0, 10.0)
        );
        slider.direction = ControlDirection::BottomToTop;
        assert_eq!(
            slider.handle_rect(control, [20.0, 10.0]),
            UiRect::new(190.0, 60.0, 20.0, 10.0)
        );
        // 滑块比轨道还长时贴住轨道，不越界。
        slider.direction = ControlDirection::LeftToRight;
        slider.value = 1.0;
        assert_eq!(
            slider.handle_rect(control, [500.0, 10.0]),
            UiRect::new(100.0, 55.0, 200.0, 10.0)
        );
    }

    #[test]
    fn normalized_clamps_and_zero_span_is_zero() {
        let slider = Slider {
            min: 5.0,
            max: 5.0001,
            value: 5.0,
            ..Slider::default()
        };
        assert_close(slider.normalized(), 0.0);
        let mut slider = Slider::default();
        assert!(slider.set_normalized(2.0));
        assert_close(slider.value, 1.0);
        assert!(!slider.set_normalized(f32::NAN));
        assert_close(slider.value, 1.0);
    }
}
