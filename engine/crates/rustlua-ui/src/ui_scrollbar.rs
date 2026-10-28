//! `Scrollbar`：滚动条（对齐 Unity `Scrollbar`）。
//!
//! 组件串：`value=0;size=0.2;direction=left_to_right;handle=Handle`。
//!
//! * `value`：0..=1 的滚动位置；
//! * `size`：滑块占整条轨道的比例（Unity 的 `Scrollbar.size`，内容越多滑块越短）；
//! * `handle`：子实体名，宿主把它的矩形改写成"长度 = 轨道 × size、位置 = value × 剩余行程"。
//!
//! 与 `Slider` 的区别是**滑块长度由 size 决定**，而不是保留子实体自身尺寸——滚动条的
//! 滑块必须随内容长度变化，Unity 的 ScrollRect 也按同一公式驱动它。

use crate::ui::{escape_text, split_fields, unescape_text};
use crate::ui_control_direction::ControlDirection;
use crate::ui_layout::UiRect;

pub const DEFAULT_SCROLLBAR_SIZE: f32 = 0.2;

#[derive(Clone, Debug, PartialEq)]
pub struct ScrollBar {
    pub value: f32,
    pub size: f32,
    pub direction: ControlDirection,
    pub handle: String,
}

impl Default for ScrollBar {
    fn default() -> Self {
        Self {
            value: 0.0,
            size: DEFAULT_SCROLLBAR_SIZE,
            direction: ControlDirection::LeftToRight,
            handle: String::new(),
        }
    }
}

impl ScrollBar {
    pub fn from_component_str(raw: &str) -> Result<Self, String> {
        let mut scrollbar = Self::default();
        let mut seen = [false; 4];
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
                "value" => 0,
                "size" => 1,
                "direction" => 2,
                "handle" => 3,
                other => return Err(format!("unknown Scrollbar field `{other}`")),
            };
            if seen[index] {
                return Err(format!("duplicate `{key}=`"));
            }
            seen[index] = true;
            match key {
                "value" => scrollbar.value = parse_unit("value", value)?,
                "size" => scrollbar.size = parse_unit("size", value)?,
                "direction" => {
                    scrollbar.direction = ControlDirection::parse(value)
                        .ok_or_else(|| format!("invalid direction `{value}`"))?;
                }
                "handle" => scrollbar.handle = unescape_text(value),
                _ => unreachable!("key was matched above"),
            }
        }
        Ok(scrollbar)
    }

    #[must_use]
    pub fn to_component_str(&self) -> String {
        let mut encoded = format!("value={};size={}", self.value, self.size);
        if self.direction != ControlDirection::LeftToRight {
            encoded.push_str(&format!(";direction={}", self.direction.as_str()));
        }
        if !self.handle.is_empty() {
            encoded.push_str(&format!(";handle={}", escape_text(&self.handle)));
        }
        encoded
    }

    /// 写入 0..=1 的位置。返回是否真的变了。
    pub fn set_value(&mut self, value: f32) -> bool {
        if !value.is_finite() {
            return false;
        }
        let value = value.clamp(0.0, 1.0);
        let changed = value != self.value;
        self.value = value;
        changed
    }

    /// 滑块矩形：长度 = 轨道 × size，位置 = value × (轨道 - 滑块)。
    #[must_use]
    pub fn handle_rect(&self, control: UiRect) -> UiRect {
        let size = if self.size.is_finite() {
            self.size.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if self.direction.is_vertical() {
            let length = control.h * size;
            let start = self.direction.handle_start(control, length, self.value);
            UiRect::new(control.x, start, control.w, length)
        } else {
            let length = control.w * size;
            let start = self.direction.handle_start(control, length, self.value);
            UiRect::new(start, control.y, length, control.h)
        }
    }
}

fn parse_unit(key: &str, raw: &str) -> Result<f32, String> {
    let value = raw
        .parse::<f32>()
        .map_err(|_| format!("invalid `{key}` value `{raw}`"))?;
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(format!("`{key}` must be within 0..=1, got `{raw}`"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_defaults_and_round_trips() {
        assert_eq!(
            ScrollBar::from_component_str("").unwrap(),
            ScrollBar::default()
        );
        let scrollbar = ScrollBar {
            value: 0.4,
            size: 0.3,
            direction: ControlDirection::BottomToTop,
            handle: "Scroll;Handle".to_owned(),
        };
        let encoded = scrollbar.to_component_str();
        assert!(encoded.contains("direction=bottom_to_top"), "{encoded}");
        assert!(encoded.contains("handle=Scroll\\;Handle"), "{encoded}");
        assert_eq!(ScrollBar::from_component_str(&encoded).unwrap(), scrollbar);
    }

    #[test]
    fn rejects_out_of_range_and_duplicates() {
        for bad in [
            "value=1.5",
            "value=-0.1",
            "size=2",
            "value=0.5;value=0.6",
            "direction=up",
            "slider=1",
        ] {
            assert!(ScrollBar::from_component_str(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn handle_length_tracks_size_and_position_tracks_value() {
        let control = UiRect::new(10.0, 20.0, 100.0, 10.0);
        let mut scrollbar = ScrollBar {
            value: 0.0,
            size: 0.25,
            ..ScrollBar::default()
        };
        assert_eq!(
            scrollbar.handle_rect(control),
            UiRect::new(10.0, 20.0, 25.0, 10.0)
        );
        assert!(scrollbar.set_value(1.0));
        assert_eq!(
            scrollbar.handle_rect(control),
            UiRect::new(85.0, 20.0, 25.0, 10.0)
        );
        scrollbar.set_value(0.5);
        assert_eq!(
            scrollbar.handle_rect(control),
            UiRect::new(47.5, 20.0, 25.0, 10.0)
        );
    }

    #[test]
    fn vertical_and_reverse_directions_mirror() {
        let control = UiRect::new(10.0, 20.0, 10.0, 100.0);
        let mut scrollbar = ScrollBar {
            value: 0.0,
            size: 0.25,
            direction: ControlDirection::BottomToTop,
            ..ScrollBar::default()
        };
        assert_eq!(
            scrollbar.handle_rect(control),
            UiRect::new(10.0, 95.0, 10.0, 25.0)
        );
        scrollbar.set_value(1.0);
        assert_eq!(
            scrollbar.handle_rect(control),
            UiRect::new(10.0, 20.0, 10.0, 25.0)
        );
        scrollbar.direction = ControlDirection::TopToBottom;
        assert_eq!(
            scrollbar.handle_rect(control),
            UiRect::new(10.0, 95.0, 10.0, 25.0)
        );
    }

    #[test]
    fn value_clamps_and_nan_is_ignored() {
        let mut scrollbar = ScrollBar::default();
        assert!(scrollbar.set_value(2.0));
        assert_eq!(scrollbar.value, 1.0);
        assert!(!scrollbar.set_value(f32::NAN));
        assert_eq!(scrollbar.value, 1.0);
    }
}
