//! `Mask`：矩形裁剪容器（对齐 Unity `RectMask2D`）。
//!
//! 与 `ScrollView` 的区别只有**内容是否可滚动**：两者都按自身矩形裁剪子孙，
//! 绘制与命中共用同一份裁剪矩形。`Mask` 没有滚动范围，偏移恒为 0。
//!
//! 组件串：`padding=left,top,right,bottom`（缺省 0）。负 padding 表示向外扩张，
//! 与 Unity 的 RectMask2D 一致；非有限值直接拒绝。

use crate::ui_interaction::UiRect;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mask {
    /// 内边距（左、上、右、下）：正值缩小裁剪区。
    pub padding: [f32; 4],
}

impl Mask {
    pub fn from_component_str(raw: &str) -> Result<Self, String> {
        let mut mask = Self::default();
        for field in raw.split(';') {
            let field = field.trim();
            if field.is_empty() {
                continue;
            }
            let Some((key, value)) = field.split_once('=') else {
                return Err(format!("expected key=value, got `{field}`"));
            };
            match key {
                "padding" => {
                    let parsed: Vec<f32> = value
                        .split(',')
                        .map(|part| {
                            part.trim()
                                .parse::<f32>()
                                .map_err(|_| format!("invalid padding value `{part}`"))
                        })
                        .collect::<Result<_, _>>()?;
                    if parsed.len() != 4 {
                        return Err(format!(
                            "padding needs 4 values (left,top,right,bottom), got {}",
                            parsed.len()
                        ));
                    }
                    if parsed.iter().any(|value| !value.is_finite()) {
                        return Err("padding values must be finite".to_owned());
                    }
                    mask.padding = [parsed[0], parsed[1], parsed[2], parsed[3]];
                }
                other => return Err(format!("unknown Mask field `{other}`")),
            }
        }
        Ok(mask)
    }

    #[must_use]
    pub fn to_component_str(&self) -> String {
        format!(
            "padding={},{},{},{}",
            self.padding[0], self.padding[1], self.padding[2], self.padding[3]
        )
    }

    /// 把容器矩形收缩成裁剪矩形；padding 过大时退化为 `UiRect::ZERO`。
    ///
    /// 归一到 `ZERO` 而不是"宽 0 但带偏移"的空矩形：调用方会把它存进裁剪表，
    /// 零宽带偏移的矩形在下一次求交时看不出区别，但读数（日志/调试器）会误以为
    /// 遮罩还在某个位置生效。
    #[must_use]
    pub fn clip_rect(&self, rect: UiRect) -> UiRect {
        let width = rect.w - self.padding[0] - self.padding[2];
        let height = rect.h - self.padding[1] - self.padding[3];
        if width <= 0.0 || height <= 0.0 {
            return UiRect::ZERO;
        }
        UiRect::new(
            rect.x + self.padding[0],
            rect.y + self.padding[1],
            width,
            height,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_padding_and_round_trips() {
        assert_eq!(Mask::from_component_str("").unwrap(), Mask::default());
        let mask = Mask::from_component_str("padding=4,8,12,16").unwrap();
        assert_eq!(mask.padding, [4.0, 8.0, 12.0, 16.0]);
        assert_eq!(
            Mask::from_component_str(&mask.to_component_str()).unwrap(),
            mask
        );
        for bad in [
            "padding=1,2,3",
            "padding=1,2,3,4,5",
            "padding=a,b,c,d",
            "padding=NaN,0,0,0",
            "inset=1",
        ] {
            assert!(Mask::from_component_str(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn clip_rect_shrinks_and_allows_negative_padding() {
        let rect = UiRect::new(10.0, 20.0, 100.0, 50.0);
        assert_eq!(
            Mask {
                padding: [2.0, 3.0, 4.0, 5.0]
            }
            .clip_rect(rect),
            UiRect::new(12.0, 23.0, 94.0, 42.0)
        );
        assert_eq!(
            Mask {
                padding: [-2.0, -3.0, -4.0, -5.0]
            }
            .clip_rect(rect),
            UiRect::new(8.0, 17.0, 106.0, 58.0)
        );
        assert_eq!(
            Mask {
                padding: [100.0, 0.0, 100.0, 0.0]
            }
            .clip_rect(rect),
            UiRect::ZERO
        );
    }
}
