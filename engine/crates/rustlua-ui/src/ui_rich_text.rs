//! M76-07：受限富文本。只认 `<b>` / `<color=#rrggbb>` / `<icon=name>` / `<br/>`，
//! 其余尖括号内容按字面量渲染——外部文本不能借标签改变排版或执行指令。

/// 一段同一样式的文本。`icon` 非空时 `text` 是图标名而非可排版文字。
#[derive(Debug, Clone, PartialEq)]
pub struct RichSpan {
    pub text: String,
    pub bold: bool,
    pub color: Option<[u8; 3]>,
    pub icon: Option<String>,
    pub line_break: bool,
}

impl RichSpan {
    fn new(text: String, bold: bool, color: Option<[u8; 3]>) -> Self {
        Self {
            text,
            bold,
            color,
            icon: None,
            line_break: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RichText {
    pub spans: Vec<RichSpan>,
    /// 去掉标签后的纯文本，供无障碍朗读与宽度估算。
    pub plain: String,
    /// 不认识的标签名（已按字面量保留），供编辑器提示。
    pub unsupported: Vec<String>,
}

impl RichText {
    pub fn line_count(&self) -> usize {
        1 + self.spans.iter().filter(|span| span.line_break).count()
    }
}

/// 解析有限标签集。未闭合标签不报错：标签文本原样保留，避免半截输入吞掉内容。
#[must_use]
pub fn parse_rich_text(source: &str) -> RichText {
    let mut spans = Vec::new();
    let mut plain = String::new();
    let mut unsupported = Vec::new();
    let mut bold = false;
    let mut color: Option<[u8; 3]> = None;
    let mut color_stack: Vec<Option<[u8; 3]>> = Vec::new();
    let mut buffer = String::new();

    fn flush(
        buffer: &mut String,
        spans: &mut Vec<RichSpan>,
        plain: &mut String,
        bold: bool,
        color: Option<[u8; 3]>,
    ) {
        if !buffer.is_empty() {
            plain.push_str(buffer);
            spans.push(RichSpan::new(std::mem::take(buffer), bold, color));
        }
    }

    let mut rest = source;
    while let Some(open) = rest.find('<') {
        buffer.push_str(&rest[..open]);
        rest = &rest[open..];
        let Some(close) = rest.find('>') else {
            buffer.push_str(rest);
            rest = "";
            break;
        };
        let tag = &rest[1..close];
        let lowered = tag.trim().to_ascii_lowercase();
        let consumed = close + 1;
        if lowered == "b" {
            flush(&mut buffer, &mut spans, &mut plain, bold, color);
            bold = true;
            rest = &rest[consumed..];
        } else if lowered == "/b" {
            flush(&mut buffer, &mut spans, &mut plain, bold, color);
            bold = false;
            rest = &rest[consumed..];
        } else if lowered == "br" || lowered == "br/" {
            flush(&mut buffer, &mut spans, &mut plain, bold, color);
            plain.push('\n');
            spans.push(RichSpan {
                text: String::new(),
                bold,
                color,
                icon: None,
                line_break: true,
            });
            rest = &rest[consumed..];
        } else if let Some(raw) = lowered.strip_prefix("color=") {
            flush(&mut buffer, &mut spans, &mut plain, bold, color);
            color_stack.push(color);
            color = parse_color(raw.trim());
            rest = &rest[consumed..];
        } else if lowered == "/color" {
            flush(&mut buffer, &mut spans, &mut plain, bold, color);
            color = color_stack.pop().unwrap_or(None);
            rest = &rest[consumed..];
        } else if let Some(raw) = lowered.strip_prefix("icon=") {
            flush(&mut buffer, &mut spans, &mut plain, bold, color);
            let name = raw.trim();
            if name.is_empty() {
                spans.push(RichSpan::new(rest[..consumed].to_owned(), bold, color));
                plain.push_str(&rest[..consumed]);
            } else {
                spans.push(RichSpan {
                    text: String::new(),
                    bold,
                    color,
                    icon: Some(name.to_owned()),
                    line_break: false,
                });
            }
            rest = &rest[consumed..];
        } else {
            unsupported.push(tag.trim().to_owned());
            buffer.push_str(&rest[..consumed]);
            rest = &rest[consumed..];
        }
    }
    buffer.push_str(rest);
    flush(&mut buffer, &mut spans, &mut plain, bold, color);

    RichText {
        spans,
        plain,
        unsupported,
    }
}

fn parse_color(raw: &str) -> Option<[u8; 3]> {
    let hex = raw.strip_prefix('#')?;
    if hex.len() != 6 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some([
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        (value & 0xff) as u8,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bold_color_icon_and_break_are_parsed() {
        let rich = parse_rich_text("Hi <b>bold</b> <color=#FF8000>warm</color><br/><icon=coin>");
        assert_eq!(rich.plain, "Hi bold warm\n");
        assert_eq!(rich.line_count(), 2);
        let warm = rich.spans.iter().find(|span| span.text == "warm").unwrap();
        assert_eq!(warm.color, Some([0xff, 0x80, 0x00]));
        assert!(
            rich.spans
                .iter()
                .any(|span| span.text == "bold" && span.bold)
        );
        assert!(
            rich.spans
                .iter()
                .any(|span| span.icon.as_deref() == Some("coin"))
        );
        assert!(rich.unsupported.is_empty());
    }

    #[test]
    fn unknown_tags_stay_literal_and_never_execute() {
        let rich = parse_rich_text("<script>alert(1)</script> <img src=x>");
        assert_eq!(rich.plain, "<script>alert(1)</script> <img src=x>");
        assert!(rich.unsupported.iter().any(|tag| tag.starts_with("script")));
        assert!(rich.spans.iter().all(|span| span.icon.is_none()));
    }

    #[test]
    fn unclosed_tags_keep_text_and_invalid_color_is_ignored() {
        let rich = parse_rich_text("<b>open <color=red>tint");
        assert_eq!(rich.plain, "open tint");
        assert!(rich.spans.iter().all(|span| span.bold));
        assert!(rich.spans.iter().all(|span| span.color.is_none()));
        let truncated = parse_rich_text("tail <b");
        assert_eq!(truncated.plain, "tail <b");
    }
}
