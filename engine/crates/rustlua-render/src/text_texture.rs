//! 文字栅格化贴图（M82-B2 `texture.from_text`）：把一行/多行文本画成独立 RGBA8 贴图。
//!
//! 与 `text.rs` 的 GPU 排版管线不同——那里产出覆在覆盖率图集上的 `TextMesh`，这里是**
//! 直接合成一张独立图像**（"贴一张就走"的标签/编号）。用 ab_glyph 的高层 `outline_glyph().draw()`
//! 逐字形取覆盖率，按 source-over 合成到背景上。
//!
//! 确定性：同字体字节 + 同 spec 同像素（本机逐字节一致）。**跨平台字节一致不作整数级承诺**——
//! 覆盖率来自 ab_glyph 光栅器的浮点运算，与 `texture_gen` 的整数噪声不同；同一台机器/同一字体
//! 版本内确定，换平台可能有个别边界像素翻转。

use crate::texture::{TextureAsset, TextureColorSpace, TextureError};
use crate::texture_gen::TEXTURE_MAX_SIZE;
use ab_glyph::{Font, PxScale, ScaleFont, point};

pub use ab_glyph::FontArc;

/// 从字体字节解析可复用的 [`FontArc`]（编辑器等宿主加载工程/内置字体用，不必自己依赖 ab_glyph）。
#[must_use]
pub fn load_font(bytes: Vec<u8>) -> Option<FontArc> {
    FontArc::try_from_vec(bytes).ok()
}

/// 字号下限（px）。
pub const TEXT_MIN_FONT_PX: u32 = 8;
/// 字号上限（px）。
pub const TEXT_MAX_FONT_PX: u32 = 256;
/// 内边距上限（px）。
pub const TEXT_MAX_PADDING: u32 = 64;

/// 一张文字贴图的规格。
#[derive(Clone, Debug, PartialEq)]
pub struct TextSpec {
    /// 字号（px）。
    pub font_px: u32,
    /// 四周内边距（px）。
    pub padding: u32,
    /// 文字颜色（RGBA）。
    pub color: [u8; 4],
    /// 背景色（RGBA；alpha=0 = 透明）。
    pub background: [u8; 4],
    /// 最大**贴图**宽度（含内边距）。给定则按字符换行；`None` = 只按 `\n` 分行、过宽报错。
    pub max_width: Option<u32>,
}

impl Default for TextSpec {
    fn default() -> Self {
        Self {
            font_px: 24,
            padding: 4,
            color: [255, 255, 255, 255],
            background: [0, 0, 0, 0],
            max_width: None,
        }
    }
}

/// 文字贴图生成错误。
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum TextRenderError {
    #[error("text must not be empty")]
    Empty,
    #[error("font size {px} out of range {TEXT_MIN_FONT_PX}..={TEXT_MAX_FONT_PX}")]
    FontOutOfRange { px: u32 },
    #[error("padding {padding} out of range 0..={TEXT_MAX_PADDING}")]
    PaddingOutOfRange { padding: u32 },
    #[error(
        "text texture {width}x{height} exceeds max {TEXTURE_MAX_SIZE} (give `max_width` to wrap)"
    )]
    TooLarge { width: u32, height: u32 },
    #[error("texture asset error: {0}")]
    Asset(#[from] TextureError),
}

/// 把文本画成一张方形无关（按内容定尺寸）的 RGBA8 贴图。
pub fn render_text(
    font: &FontArc,
    text: &str,
    spec: &TextSpec,
) -> Result<TextureAsset, TextRenderError> {
    if text.is_empty() {
        return Err(TextRenderError::Empty);
    }
    if !(TEXT_MIN_FONT_PX..=TEXT_MAX_FONT_PX).contains(&spec.font_px) {
        return Err(TextRenderError::FontOutOfRange { px: spec.font_px });
    }
    if spec.padding > TEXT_MAX_PADDING {
        return Err(TextRenderError::PaddingOutOfRange {
            padding: spec.padding,
        });
    }

    let scale = PxScale::from(spec.font_px as f32);
    let sf = font.as_scaled(scale);
    let ascent = sf.ascent();
    let descent = sf.descent();
    let line_height = (ascent - descent + sf.line_gap()).max(1.0);
    let advance = |ch: char| sf.h_advance(font.glyph_id(ch));
    let content_width = |line: &str| -> f32 { line.chars().map(advance).sum() };

    // 分行：显式 `\n` 优先；给了 `max_width` 再按字符折行（CJK 按字断无不自然）。
    let max_content = spec
        .max_width
        .map(|w| w.saturating_sub(spec.padding.saturating_mul(2)) as f32)
        .filter(|w| *w >= 1.0);
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        if let Some(maxw) = max_content {
            let mut current = String::new();
            let mut width = 0.0f32;
            for ch in raw.chars() {
                let step = advance(ch);
                if width + step > maxw && !current.is_empty() {
                    lines.push(std::mem::take(&mut current));
                    width = 0.0;
                }
                current.push(ch);
                width += step;
            }
            lines.push(current);
        } else {
            lines.push(raw.to_string());
        }
    }

    let widest = lines
        .iter()
        .map(|line| content_width(line))
        .fold(0.0f32, f32::max);
    let width = (widest.ceil() as u32)
        .saturating_add(spec.padding.saturating_mul(2))
        .max(1);
    let height = ((lines.len() as f32 * line_height).ceil() as u32)
        .saturating_add(spec.padding.saturating_mul(2))
        .max(1);
    if width > TEXTURE_MAX_SIZE || height > TEXTURE_MAX_SIZE {
        return Err(TextRenderError::TooLarge { width, height });
    }

    let mut rgba = vec![0u8; (width as usize) * (height as usize) * 4];
    if spec.background[3] != 0 {
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.copy_from_slice(&spec.background);
        }
    }

    let first_baseline = spec.padding as f32 + ascent;
    for (line_index, line) in lines.iter().enumerate() {
        let baseline = (first_baseline + line_index as f32 * line_height).round();
        let mut pen_x = spec.padding as f32;
        for ch in line.chars() {
            let glyph_id = font.glyph_id(ch);
            let glyph = glyph_id.with_scale_and_position(scale, point(pen_x.round(), baseline));
            if let Some(outlined) = font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                outlined.draw(|gx, gy, coverage| {
                    let px = bounds.min.x as i32 + gx as i32;
                    let py = bounds.min.y as i32 + gy as i32;
                    if px < 0 || py < 0 {
                        return;
                    }
                    let (px, py) = (px as u32, py as u32);
                    if px >= width || py >= height {
                        return;
                    }
                    let index = ((py * width + px) * 4) as usize;
                    composite(&mut rgba[index..index + 4], spec.color, coverage);
                });
            }
            pen_x += advance(ch);
        }
    }

    Ok(TextureAsset::from_rgba8(
        width,
        height,
        rgba,
        TextureColorSpace::Srgb,
    )?)
}

/// source-over 合成一个带覆盖率的文字像素（straight alpha）。
fn composite(dst: &mut [u8], color: [u8; 4], coverage: f32) {
    let coverage = coverage.clamp(0.0, 1.0);
    let foreground = coverage * (f32::from(color[3]) / 255.0);
    if foreground <= 0.0 {
        return;
    }
    let background = f32::from(dst[3]) / 255.0;
    let out_alpha = foreground + background * (1.0 - foreground);
    if out_alpha <= 0.0 {
        return;
    }
    for channel in 0..3 {
        let f = f32::from(color[channel]);
        let b = f32::from(dst[channel]);
        dst[channel] = ((f * foreground + b * background * (1.0 - foreground)) / out_alpha)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
    dst[3] = (out_alpha * 255.0).round().clamp(0.0, 255.0) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_font() -> FontArc {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/editor/fonts/NotoSansSC-VF.ttf");
        let bytes = std::fs::read(&path).expect("editor NotoSansSC font available");
        FontArc::try_from_vec(bytes).expect("parse font")
    }

    #[test]
    fn renders_and_is_deterministic() {
        let font = test_font();
        let spec = TextSpec {
            font_px: 18,
            padding: 4,
            color: [255, 240, 200, 255],
            background: [20, 20, 30, 255],
            max_width: None,
        };
        let a = render_text(&font, "ABC 123", &spec).expect("render");
        let b = render_text(&font, "ABC 123", &spec).expect("render");
        assert_eq!(a, b, "同字体 + 同 spec 必须逐字节相同");
        assert!(a.width() > 4 && a.height() > 4);
        // 有文字落在不透明背景上：必出现既非纯背景、也非透明的像素。
        let has_ink = a
            .pixels()
            .chunks_exact(4)
            .any(|p| p != &spec.background[..]);
        assert!(has_ink, "画布上必须有字形像素");
    }

    #[test]
    fn transparent_background_keeps_alpha() {
        let font = test_font();
        let spec = TextSpec {
            font_px: 32,
            padding: 2,
            color: [255, 255, 255, 255],
            background: [0, 0, 0, 0],
            max_width: None,
        };
        let asset = render_text(&font, "X", &spec).expect("render");
        // 透明底：四角应是全 0（没有字落到角上），字形区 alpha > 0。
        let corner = &asset.pixels()[0..4];
        assert_eq!(corner, [0, 0, 0, 0], "透明底的角落像素保持全 0");
        assert!(
            asset.pixels().chunks_exact(4).any(|p| p[3] > 0),
            "字形区 alpha 必须非 0"
        );
    }

    #[test]
    fn wraps_when_max_width_given() {
        let font = test_font();
        let single = render_text(
            &font,
            "0123456789012345678901234567890",
            &TextSpec {
                font_px: 24,
                padding: 2,
                max_width: None,
                ..TextSpec::default()
            },
        )
        .expect("single line");
        let wrapped = render_text(
            &font,
            "0123456789012345678901234567890",
            &TextSpec {
                font_px: 24,
                padding: 2,
                max_width: Some(80),
                ..TextSpec::default()
            },
        )
        .expect("wrapped");
        assert!(
            wrapped.height() > single.height(),
            "折行后高度必须增加：{} vs {}",
            wrapped.height(),
            single.height()
        );
        assert!(wrapped.width() <= 80, "折行后宽度不超过 max_width");
    }

    #[test]
    fn rejects_empty_and_out_of_range() {
        let font = test_font();
        assert!(matches!(
            render_text(&font, "", &TextSpec::default()),
            Err(TextRenderError::Empty)
        ));
        assert!(matches!(
            render_text(
                &font,
                "x",
                &TextSpec {
                    font_px: 4,
                    ..TextSpec::default()
                }
            ),
            Err(TextRenderError::FontOutOfRange { px: 4 })
        ));
        assert!(matches!(
            render_text(
                &font,
                "x",
                &TextSpec {
                    padding: 99,
                    ..TextSpec::default()
                }
            ),
            Err(TextRenderError::PaddingOutOfRange { padding: 99 })
        ));
    }
}
