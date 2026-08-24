//! Screen-space UI scene components: `Text`, `Image`, `Button`.
//!
//! These are ordinary scene-entity components (opaque `k=v;...` strings) that
//! render as NDC overlay quads on top of the scene. Coordinates are
//! viewport-pixel positions (top-left origin), matching the Lua `ui.text_at`
//! convention.

use std::{error::Error, fmt};

/// Errors while parsing a UI component string.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiComponentError {
    /// A field was not `key=value` as the scene format requires.
    MalformedField(String),
    /// A required field was missing.
    MissingField(&'static str),
    /// A numeric or color field could not be parsed / was invalid.
    InvalidValue(String),
}

impl fmt::Display for UiComponentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedField(field) => write!(f, "malformed UI field: {field}"),
            Self::MissingField(field) => write!(f, "missing UI field: {field}"),
            Self::InvalidValue(value) => write!(f, "invalid UI value: {value}"),
        }
    }
}
impl Error for UiComponentError {}

/// Screen-space text label (`Text` component).
#[derive(Clone, Debug, PartialEq)]
pub struct UiText {
    pub text: String,
    /// Pixel position of the label top-left.
    pub position: [f32; 2],
    /// Screen-space glyph scale (1.0 = 5x7 px glyphs).
    pub scale: f32,
    /// RGB tint; default white.
    pub color: [f32; 3],
}

/// Screen-space textured quad (`Image` component).
#[derive(Clone, Debug, PartialEq)]
pub struct UiImage {
    /// Project-relative texture path.
    pub texture: String,
    /// Pixel position of the quad top-left.
    pub position: [f32; 2],
    /// Quad size in pixels.
    pub size: [f32; 2],
    /// RGB tint; default white.
    pub color: [f32; 3],
}

/// Screen-space clickable button (`Button` component).
#[derive(Clone, Debug, PartialEq)]
pub struct UiButton {
    /// Optional label text drawn centered on the button.
    pub label: String,
    /// Pixel position of the rect top-left.
    pub position: [f32; 2],
    /// Rect size in pixels.
    pub size: [f32; 2],
    /// Background fill color.
    pub background: [f32; 3],
    /// Label text color.
    pub text_color: [f32; 3],
}

fn parse_number(raw: &str) -> Result<f32, UiComponentError> {
    let value = raw
        .parse::<f32>()
        .map_err(|_| UiComponentError::InvalidValue(raw.to_owned()))?;
    if !value.is_finite() {
        return Err(UiComponentError::InvalidValue(raw.to_owned()));
    }
    Ok(value)
}

/// Parses an `r,g,b` color triple (each in `0..=1`).
fn parse_rgb(raw: &str) -> Result<[f32; 3], UiComponentError> {
    let parts: Vec<&str> = raw.split(',').collect();
    if parts.len() != 3 {
        return Err(UiComponentError::InvalidValue(raw.to_owned()));
    }
    Ok([
        parse_number(parts[0])?,
        parse_number(parts[1])?,
        parse_number(parts[2])?,
    ])
}

/// Splits a component string on `;` only when NOT preceded by a backslash
/// escape (so escaped `\;` inside text fields survives intact).
fn split_fields(value: &str) -> Vec<&str> {
    let mut fields = Vec::new();
    let bytes = value.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b';' {
            let mut backslashes = 0;
            let mut j = i;
            while j > start && bytes[j - 1] == b'\\' {
                backslashes += 1;
                j -= 1;
            }
            if backslashes % 2 == 0 {
                fields.push(&value[start..i]);
                start = i + 1;
            }
        }
        i += 1;
    }
    fields.push(&value[start..]);
    fields
}

/// Escapes `\`, `;`, `=` inside a free-form text field so it survives the
/// `;`-separated component encoding.
fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            '=' => out.push_str("\\="),
            _ => out.push(ch),
        }
    }
    out
}

/// Inverse of [`escape_text`]; unknown `\X` sequences pass through unchanged.
fn unescape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some(';') => out.push(';'),
                Some('=') => out.push('='),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

impl UiText {
    /// Parses `text=<esc>;x=..;y=..;scale=..;color=r,g,b` (scale/color optional).
    pub fn from_component_str(value: &str) -> Result<Self, UiComponentError> {
        let mut text = String::new();
        let mut position = [0.0; 2];
        let mut scale = 1.0;
        let mut color = [1.0, 1.0, 1.0];
        let mut seen = [false; 4]; // text, x, y
        for field in split_fields(value) {
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| UiComponentError::MalformedField(field.to_owned()))?;
            match key {
                "text" => {
                    text = unescape_text(raw);
                    seen[0] = true;
                }
                "x" => {
                    position[0] = parse_number(raw)?;
                    seen[1] = true;
                }
                "y" => {
                    position[1] = parse_number(raw)?;
                    seen[2] = true;
                }
                "scale" => {
                    scale = parse_number(raw)?;
                    if scale <= 0.0 {
                        return Err(UiComponentError::InvalidValue(raw.to_owned()));
                    }
                    seen[3] = true;
                }
                "color" => color = parse_rgb(raw)?,
                _ => return Err(UiComponentError::MalformedField(field.to_owned())),
            }
        }
        if !seen[0] {
            return Err(UiComponentError::MissingField("text"));
        }
        if !seen[1] || !seen[2] {
            return Err(UiComponentError::MissingField("x"));
        }
        let _ = seen[3];
        Ok(Self {
            text,
            position,
            scale,
            color,
        })
    }

    /// Encodes back to the component string.
    #[must_use]
    pub fn to_component_str(&self) -> String {
        format!(
            "text={};x={:.2};y={:.2};scale={:.2};color={:.3},{:.3},{:.3}",
            escape_text(&self.text),
            self.position[0],
            self.position[1],
            self.scale,
            self.color[0],
            self.color[1],
            self.color[2]
        )
    }
}

impl UiImage {
    /// Parses `texture=..;x=..;y=..;width=..;height=..;color=r,g,b` (color optional).
    pub fn from_component_str(value: &str) -> Result<Self, UiComponentError> {
        let mut texture = String::new();
        let mut position = [0.0; 2];
        let mut size = [0.0; 2];
        let mut color = [1.0, 1.0, 1.0];
        let mut seen = [false; 6];
        for field in split_fields(value) {
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| UiComponentError::MalformedField(field.to_owned()))?;
            match key {
                "texture" => {
                    texture = raw.to_owned();
                    seen[0] = true;
                }
                "x" => {
                    position[0] = parse_number(raw)?;
                    seen[1] = true;
                }
                "y" => {
                    position[1] = parse_number(raw)?;
                    seen[2] = true;
                }
                "width" => {
                    size[0] = parse_number(raw)?;
                    seen[3] = true;
                }
                "height" => {
                    size[1] = parse_number(raw)?;
                    seen[4] = true;
                }
                "color" => color = parse_rgb(raw)?,
                _ => return Err(UiComponentError::MalformedField(field.to_owned())),
            }
        }
        for (field, present) in [
            ("texture", seen[0]),
            ("x", seen[1]),
            ("y", seen[2]),
            ("width", seen[3]),
            ("height", seen[4]),
        ] {
            if !present {
                return Err(UiComponentError::MissingField(field));
            }
        }
        Ok(Self {
            texture,
            position,
            size,
            color,
        })
    }

    /// Encodes back to the component string.
    #[must_use]
    pub fn to_component_str(&self) -> String {
        format!(
            "texture={};x={:.2};y={:.2};width={:.2};height={:.2};color={:.3},{:.3},{:.3}",
            self.texture,
            self.position[0],
            self.position[1],
            self.size[0],
            self.size[1],
            self.color[0],
            self.color[1],
            self.color[2]
        )
    }
}

impl UiButton {
    /// Parses `label=<esc>;x=..;y=..;width=..;height=..;bg=r,g,b;text_color=r,g,b`.
    pub fn from_component_str(value: &str) -> Result<Self, UiComponentError> {
        let mut label = String::new();
        let mut position = [0.0; 2];
        let mut size = [0.0; 2];
        let mut background = [0.2, 0.45, 0.8];
        let mut text_color = [1.0, 1.0, 1.0];
        let mut seen = [false; 5];
        for field in split_fields(value) {
            let (key, raw) = field
                .split_once('=')
                .ok_or_else(|| UiComponentError::MalformedField(field.to_owned()))?;
            match key {
                "label" => {
                    label = unescape_text(raw);
                    seen[0] = true;
                }
                "x" => {
                    position[0] = parse_number(raw)?;
                    seen[1] = true;
                }
                "y" => {
                    position[1] = parse_number(raw)?;
                    seen[2] = true;
                }
                "width" => {
                    size[0] = parse_number(raw)?;
                    seen[3] = true;
                }
                "height" => {
                    size[1] = parse_number(raw)?;
                    seen[4] = true;
                }
                "bg" => background = parse_rgb(raw)?,
                "text_color" => text_color = parse_rgb(raw)?,
                _ => return Err(UiComponentError::MalformedField(field.to_owned())),
            }
        }
        for (field, present) in [
            ("x", seen[1]),
            ("y", seen[2]),
            ("width", seen[3]),
            ("height", seen[4]),
        ] {
            if !present {
                return Err(UiComponentError::MissingField(field));
            }
        }
        let _ = seen[0];
        Ok(Self {
            label,
            position,
            size,
            background,
            text_color,
        })
    }

    /// Encodes back to the component string.
    #[must_use]
    pub fn to_component_str(&self) -> String {
        format!(
            "label={};x={:.2};y={:.2};width={:.2};height={:.2};bg={:.3},{:.3},{:.3};text_color={:.3},{:.3},{:.3}",
            escape_text(&self.label),
            self.position[0],
            self.position[1],
            self.size[0],
            self.size[1],
            self.background[0],
            self.background[1],
            self.background[2],
            self.text_color[0],
            self.text_color[1],
            self.text_color[2]
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trip_preserves_escaped_specials() {
        let ui = UiText {
            text: "Score;1=0 \\ 高".to_owned(),
            position: [12.0, 30.0],
            scale: 2.0,
            color: [1.0, 0.5, 0.0],
        };
        let encoded = ui.to_component_str();
        let decoded = UiText::from_component_str(&encoded).expect("valid text");
        assert_eq!(decoded.text, ui.text);
        assert_eq!(decoded.position, ui.position);
        assert_eq!(decoded.scale, ui.scale);
        assert_eq!(decoded.color, ui.color);
    }

    #[test]
    fn text_defaults_scale_and_color() {
        let ui = UiText::from_component_str("text=Hello;x=10;y=20").expect("valid");
        assert_eq!(ui.text, "Hello");
        assert_eq!(ui.position, [10.0, 20.0]);
        assert_eq!(ui.scale, 1.0);
        assert_eq!(ui.color, [1.0, 1.0, 1.0]);
    }

    #[test]
    fn text_rejects_missing_required_field_and_bad_number() {
        assert_eq!(
            UiText::from_component_str("text=Hi;y=1").unwrap_err(),
            UiComponentError::MissingField("x")
        );
        assert_eq!(
            UiText::from_component_str("text=Hi;x=abc;y=1").unwrap_err(),
            UiComponentError::InvalidValue("abc".to_owned())
        );
        assert_eq!(
            UiText::from_component_str("text=Hi;x=1;y=1;scale=0").unwrap_err(),
            UiComponentError::InvalidValue("0".to_owned())
        );
    }

    #[test]
    fn image_round_trip() {
        let img = UiImage {
            texture: "assets/ui/panel.png".to_owned(),
            position: [100.0, 50.0],
            size: [64.0, 64.0],
            color: [0.5, 0.5, 1.0],
        };
        let decoded = UiImage::from_component_str(&img.to_component_str()).expect("valid");
        assert_eq!(decoded, img);
    }

    #[test]
    fn button_round_trip_and_click_rect() {
        let button = UiButton {
            label: "Start;Game".to_owned(),
            position: [200.0, 300.0],
            size: [120.0, 40.0],
            background: [0.1, 0.6, 0.2],
            text_color: [1.0, 1.0, 1.0],
        };
        let decoded = UiButton::from_component_str(&button.to_component_str()).expect("valid");
        assert_eq!(decoded, button);
        // Click rect: contains a point inside, rejects outside.
        let inside = decoded.position[0]..(decoded.position[0] + decoded.size[0]);
        let inside_y = decoded.position[1]..(decoded.position[1] + decoded.size[1]);
        assert!(
            inside.contains(&(decoded.position[0] + 5.0))
                && inside_y.contains(&(decoded.position[1] + 5.0))
        );
    }

    #[test]
    fn text_survives_scene_file_round_trip() {
        // Multi-line / special text passes through Scene to_text/from_text.
        let ui = UiText {
            text: "L1\nL2=大;x".to_owned(),
            position: [1.0, 2.0],
            scale: 1.0,
            color: [1.0, 1.0, 1.0],
        };
        let mut scene = crate::scene::Scene::new("ui");
        scene
            .add_entity(crate::scene::SceneEntityId(1), "Label", None)
            .unwrap();
        scene
            .entities
            .get_mut(&crate::scene::SceneEntityId(1))
            .unwrap()
            .components
            .insert("Text".to_owned(), ui.to_component_str());
        let reloaded = crate::scene::Scene::from_text(&scene.to_text()).expect("reload");
        let value = reloaded.entities[&crate::scene::SceneEntityId(1)]
            .components
            .get("Text")
            .expect("Text component");
        let decoded = UiText::from_component_str(value).expect("valid text");
        assert_eq!(decoded.text, ui.text);
    }
}
