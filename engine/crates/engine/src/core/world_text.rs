//! WP4 增补：世界空间文字。
//!
//! 与屏幕空间 `Text`（由 `UiLayout` 定位在设计分辨率上）的区别：`WorldText` 锚在实体的
//! **世界位置**上，用 `Camera2D` 投影后走**完全相同**的字形管线。相机平移/缩放时文字跟随
//! 世界移动，而屏幕 UI 不动（docs/40 §8「屏幕 UI 与世界文字分开」）。
//!
//! 绘制层：精灵之上、屏幕 UI 之下，不参与世界深度；排序与透明规则同覆盖层。

use crate::core::ui::{UiComponentError, UiText};

/// 世界空间文字组件。文字字段（`text` / `font_size` / `color` / `font`）复用 [`UiText`]
/// 的解析与校验，避免两套字号/颜色规则分叉。
#[derive(Clone, Debug, PartialEq)]
pub struct WorldText {
    pub text: UiText,
    /// 投影到屏幕后的像素偏移（例如"头顶 12px"）。
    pub offset: [f32; 2],
    /// 以锚点水平居中（宽度按字形实测）。
    pub centered: bool,
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum WorldTextError {
    /// 组件串里出现未知或格式错误的字段。
    #[error("malformed WorldText field: {0}")]
    InvalidField(String),
    /// `offset` 不是两个有限的数。
    #[error("invalid WorldText offset: {0}")]
    InvalidOffset(String),
    /// `centered` 不是布尔值。
    #[error("invalid WorldText flag: {0}")]
    InvalidFlag(String),
    /// 文字字段本身不合法（转发 `UiText` 的错误）。
    #[error(transparent)]
    Text(#[from] UiComponentError),
}

impl WorldText {
    /// 解析 `text=..;font_size=..;color=r,g,b[;font=..][;offset=x,y][;centered=true]`。
    ///
    /// 文字字段原样转交 `UiText`，因此字号/颜色/字体的校验与转义处理只有一份；
    /// 切分也必须用 `UiText` 那套**转义感知**的切分，否则文字里带分号会被切坏。
    pub fn from_component_str(value: &str) -> Result<Self, WorldTextError> {
        let mut text_fields: Vec<&str> = Vec::new();
        let mut offset = [0.0_f32, 0.0_f32];
        let mut centered = false;
        for field in crate::core::ui::split_fields(value) {
            let Some((key, raw)) = field.split_once('=') else {
                return Err(WorldTextError::InvalidField(field.to_owned()));
            };
            match key {
                "offset" => offset = parse_offset(raw)?,
                "centered" => centered = parse_flag(raw)?,
                "text" | "font_size" | "color" | "font" => text_fields.push(field),
                _ => return Err(WorldTextError::InvalidField(field.to_owned())),
            }
        }
        let text = UiText::from_component_str(&text_fields.join(";"))?;
        Ok(Self {
            text,
            offset,
            centered,
        })
    }

    #[must_use]
    pub fn to_component_str(&self) -> String {
        // 复用 `UiText` 的编码（含 text/font 的转义），再附加世界侧字段。
        let mut out = self.text.to_component_str();
        out.push_str(&format!(
            ";offset={:.2},{:.2}",
            self.offset[0], self.offset[1]
        ));
        // 与 `Sprite.flip_*` 同一约定：只在为 true 时输出，避免无意义的存档改动。
        if self.centered {
            out.push_str(";centered=true");
        }
        out
    }
}

fn parse_offset(raw: &str) -> Result<[f32; 2], WorldTextError> {
    let parts = raw.split(',').collect::<Vec<_>>();
    if parts.len() != 2 {
        return Err(WorldTextError::InvalidOffset(raw.to_owned()));
    }
    let x = parts[0]
        .trim()
        .parse::<f32>()
        .map_err(|_| WorldTextError::InvalidOffset(raw.to_owned()))?;
    let y = parts[1]
        .trim()
        .parse::<f32>()
        .map_err(|_| WorldTextError::InvalidOffset(raw.to_owned()))?;
    if !x.is_finite() || !y.is_finite() {
        return Err(WorldTextError::InvalidOffset(raw.to_owned()));
    }
    Ok([x, y])
}

/// 只接受明确的 true/false（含 1/0）；拼错必须报错，不能静默当成 false。
fn parse_flag(raw: &str) -> Result<bool, WorldTextError> {
    match raw.trim() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        other => Err(WorldTextError::InvalidFlag(other.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_round_trips() {
        let text = WorldText::from_component_str(
            "text=受伤-12;font_size=18;color=1.000,0.200,0.200;offset=0,-24;centered=true",
        )
        .unwrap();
        assert_eq!(text.text.text, "受伤-12");
        assert_eq!(text.text.font_size, 18.0);
        assert_eq!(text.offset, [0.0, -24.0]);
        assert!(text.centered);

        let encoded = text.to_component_str();
        assert_eq!(WorldText::from_component_str(&encoded).unwrap(), text);

        // 未开启 centered 时不得输出该字段。
        let plain = WorldText::from_component_str("text=名字;font_size=16;color=1,1,1").unwrap();
        assert!(!plain.centered);
        assert!(!plain.to_component_str().contains("centered"));
        assert!(!plain.to_component_str().contains("font="));
    }

    #[test]
    fn rejects_bad_fields() {
        assert!(WorldText::from_component_str("text=x;offset=1").is_err());
        assert!(WorldText::from_component_str("text=x;offset=nan,0").is_err());
        assert!(WorldText::from_component_str("text=x;centered=maybe").is_err());
        assert!(WorldText::from_component_str("text=x;bogus=1").is_err());
        // 文字字段的校验由 UiText 负责：字号为 0 必须报错。
        assert!(WorldText::from_component_str("text=x;font_size=0").is_err());
        // 缺 text 字段也要失败。
        assert!(WorldText::from_component_str("font_size=12").is_err());
    }
}
