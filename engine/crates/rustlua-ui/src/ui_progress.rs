//! `Progress`：只读进度条（Unity 里等价于 `interactable=false` 的 `Slider`）。
//!
//! 组件串：`min=0;max=1;value=0;direction=left_to_right;fill=Fill`。
//! 与 `Slider` 的差别只有两点：没有滑块，也**不响应输入**——指针和键盘都不会改值，
//! 脚本/绑定写值后布局下一帧重新解算填充条。
//!
//! 单独成一个组件而不是给 `Slider` 加开关，是因为"能不能拖"是作者在场景里一眼要看到
//! 的属性；把它藏在一个布尔字段里会让 Inspector 上两个完全不同的控件长得一样。

use crate::ui::{escape_text, split_fields, unescape_text};
use crate::ui_control_direction::ControlDirection;
use crate::ui_layout::UiRect;

#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    pub min: f32,
    pub max: f32,
    pub value: f32,
    pub direction: ControlDirection,
    pub fill: String,
}

impl Default for Progress {
    fn default() -> Self {
        Self {
            min: 0.0,
            max: 1.0,
            value: 0.0,
            direction: ControlDirection::LeftToRight,
            fill: String::new(),
        }
    }
}

impl Progress {
    pub fn from_component_str(raw: &str) -> Result<Self, String> {
        let mut progress = Self::default();
        let mut seen = [false; 5];
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
                "direction" => 3,
                "fill" => 4,
                other => return Err(format!("unknown Progress field `{other}`")),
            };
            if seen[index] {
                return Err(format!("duplicate `{key}=`"));
            }
            seen[index] = true;
            match key {
                "min" => progress.min = parse_finite("min", value)?,
                "max" => progress.max = parse_finite("max", value)?,
                "value" => progress.value = parse_finite("value", value)?,
                "direction" => {
                    progress.direction = ControlDirection::parse(value)
                        .ok_or_else(|| format!("invalid direction `{value}`"))?;
                }
                "fill" => progress.fill = unescape_text(value),
                _ => unreachable!("key was matched above"),
            }
        }
        if progress.max <= progress.min {
            return Err(format!(
                "max ({}) must be greater than min ({})",
                progress.max, progress.min
            ));
        }
        if progress.value < progress.min || progress.value > progress.max {
            return Err(format!(
                "value ({}) must be within {}..={}",
                progress.value, progress.min, progress.max
            ));
        }
        Ok(progress)
    }

    #[must_use]
    pub fn to_component_str(&self) -> String {
        let mut encoded = format!("min={};max={};value={}", self.min, self.max, self.value);
        if self.direction != ControlDirection::LeftToRight {
            encoded.push_str(&format!(";direction={}", self.direction.as_str()));
        }
        if !self.fill.is_empty() {
            encoded.push_str(&format!(";fill={}", escape_text(&self.fill)));
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

    /// 写入进度值（夹到 `min..=max`）。返回是否真的变了。
    pub fn set_value(&mut self, value: f32) -> bool {
        if !value.is_finite() {
            return false;
        }
        let value = value.clamp(self.min, self.max);
        let changed = value != self.value;
        self.value = value;
        changed
    }

    pub fn set_normalized(&mut self, fraction: f32) -> bool {
        if !fraction.is_finite() {
            return false;
        }
        let fraction = fraction.clamp(0.0, 1.0);
        self.set_value(self.min + (self.max - self.min) * fraction)
    }

    #[must_use]
    pub fn fill_rect(&self, control: UiRect) -> UiRect {
        let (start, length) = self.direction.fill_span(control, self.normalized());
        if self.direction.is_vertical() {
            UiRect::new(control.x, start, control.w, length)
        } else {
            UiRect::new(start, control.y, length, control.h)
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

    #[test]
    fn parses_defaults_and_round_trips() {
        assert_eq!(
            Progress::from_component_str("").unwrap(),
            Progress::default()
        );
        let progress = Progress {
            min: 0.0,
            max: 100.0,
            value: 42.0,
            direction: ControlDirection::BottomToTop,
            fill: "Health;Fill".to_owned(),
        };
        let encoded = progress.to_component_str();
        assert!(encoded.contains("direction=bottom_to_top"), "{encoded}");
        assert!(encoded.contains("fill=Health\\;Fill"), "{encoded}");
        assert_eq!(Progress::from_component_str(&encoded).unwrap(), progress);
    }

    #[test]
    fn rejects_bad_ranges_and_values() {
        for bad in [
            "min=1;max=1;value=1",
            "min=10;max=0;value=5",
            "min=0;max=1;value=1.5",
            "min=0;max=1;value=inf",
            "direction=up",
            "value=1;value=0",
            "handle=Knob",
        ] {
            assert!(Progress::from_component_str(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn fill_rect_and_clamped_updates() {
        let control = UiRect::new(0.0, 10.0, 80.0, 8.0);
        let mut progress = Progress {
            value: 0.5,
            ..Progress::default()
        };
        assert_eq!(
            progress.fill_rect(control),
            UiRect::new(0.0, 10.0, 40.0, 8.0)
        );
        assert!(progress.set_normalized(1.5));
        assert_eq!(
            progress.fill_rect(control),
            UiRect::new(0.0, 10.0, 80.0, 8.0)
        );
        assert!(progress.set_value(-5.0));
        assert_eq!(progress.value, 0.0);
        assert!(!progress.set_value(f32::NAN));
    }
}
