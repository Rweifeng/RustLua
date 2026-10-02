//! Versioned save-data serialization: key/value pairs persisted as a
//! `save_version=<n>` header plus tab-separated rows. Corrupt or unsupported
//! saves are rejected without panicking.
//!
//! # 行编码与版本
//!
//! * **v1（历史格式）**：`key<TAB>value` 原样写入。任何含 `\t`/`\n`/`\r` 的值
//!   都会写出**伪行**，随后 `decode` 整档失败——一次多行文本就能让玩家的
//!   整个存档不可读（已复现）。
//! * **v2（当前写入格式）**：key/value 逐字符转义（`\\` `\n` `\r` `\t` 与
//!   其它控制字符的 `\xNN`），因此任意 Lua 字符串都能无损往返，且一行数据
//!   永远只占一行。
//!
//! v1 **仍然可以读**（旧存档零丢失），v2 会被旧引擎明确拒绝
//! （`UnsupportedVersion`，不静默错读、不覆盖原文件）。

use std::collections::BTreeMap;

/// The save file format version written by this engine.
pub const CURRENT_SAVE_VERSION: u32 = 2;

/// 最老的、仍可读取的存档版本（v1 的裸行格式）。
pub const MIN_READABLE_SAVE_VERSION: u32 = 1;

/// The `save_version=` header prefix.
const VERSION_PREFIX: &str = "save_version=";

/// Errors produced while encoding or decoding a save file.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SaveError {
    #[error("unsupported save version: {0}")]
    UnsupportedVersion(u32),
    #[error("save file is missing its version header")]
    MissingVersion,
    #[error("malformed save file at line {line}: {reason}")]
    Malformed { line: usize, reason: String },
}

/// Escapes one key or value so that a data row never contains a raw TAB, CR or
/// LF. All other characters (including CJK) pass through unchanged, so plain
/// ASCII/CJK data is byte-identical to the v1 encoding.
fn escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 || ch as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", ch as u32));
            }
            ch => out.push(ch),
        }
    }
    out
}

/// Reverses [`escape`]. Unknown escapes and truncated hex are hard errors:
/// silently treating them as literal text would corrupt data that the writer
/// believed was escaped.
fn unescape(encoded: &str, line: usize) -> Result<String, SaveError> {
    let mut out = String::with_capacity(encoded.len());
    let mut chars = encoded.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        let Some(next) = chars.next() else {
            return Err(SaveError::Malformed {
                line,
                reason: "trailing backslash in escaped field".to_owned(),
            });
        };
        match next {
            '\\' => out.push('\\'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'x' => {
                let high = chars.next();
                let low = chars.next();
                let (Some(high), Some(low)) = (high, low) else {
                    return Err(SaveError::Malformed {
                        line,
                        reason: "truncated \\xNN escape in field".to_owned(),
                    });
                };
                let digits: String = [high, low].iter().collect();
                let code = u32::from_str_radix(&digits, 16).map_err(|_| SaveError::Malformed {
                    line,
                    reason: format!("invalid \\x escape: \\x{digits}"),
                })?;
                let Some(decoded) = char::from_u32(code) else {
                    return Err(SaveError::Malformed {
                        line,
                        reason: format!("\\x{digits} is not a valid character"),
                    });
                };
                out.push(decoded);
            }
            other => {
                return Err(SaveError::Malformed {
                    line,
                    reason: format!("unknown escape sequence: \\{other}"),
                });
            }
        }
    }
    Ok(out)
}

/// Serializes save data into the versioned text format.
pub fn encode(data: &BTreeMap<String, String>) -> String {
    let mut out = format!("{VERSION_PREFIX}{CURRENT_SAVE_VERSION}\n");
    for (key, value) in data {
        out.push_str(&escape(key));
        out.push('\t');
        out.push_str(&escape(value));
        out.push('\n');
    }
    out
}

/// Parses save data back from the versioned text format. Returns a
/// [`SaveError`] for a missing/unsupported version or a malformed row.
///
/// v1 files are read as raw rows (unescaped), so saves written before the
/// escaping change stay loadable.
pub fn decode(text: &str) -> Result<BTreeMap<String, String>, SaveError> {
    let mut rows: Vec<(usize, &str, &str)> = Vec::new();
    let mut version = None;
    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        if line.trim().is_empty() {
            continue;
        }
        // 版本头只认「没有 TAB 的 save_version= 行」：键名恰好是
        // `save_version=1` 的数据行仍然带 TAB，不会被误判成版本头。
        let is_version_header = !line.contains('\t') && line.starts_with(VERSION_PREFIX);
        if is_version_header {
            let value = &line[VERSION_PREFIX.len()..];
            version = Some(value.parse().map_err(|_| SaveError::Malformed {
                line: line_number,
                reason: "version is not an integer".to_owned(),
            })?);
            continue;
        }
        let Some((key, value)) = line.split_once('\t') else {
            return Err(SaveError::Malformed {
                line: line_number,
                reason: "row is not `key<TAB>value`".to_owned(),
            });
        };
        rows.push((line_number, key, value));
    }

    let version = match version {
        Some(version) => version,
        None => return Err(SaveError::MissingVersion),
    };
    if version == 0 || version > CURRENT_SAVE_VERSION {
        return Err(SaveError::UnsupportedVersion(version));
    }
    let escaped = version >= 2;

    let mut data = BTreeMap::new();
    for (line_number, raw_key, raw_value) in rows {
        let (key, value) = if escaped {
            (
                unescape(raw_key, line_number)?,
                unescape(raw_value, line_number)?,
            )
        } else {
            (raw_key.to_owned(), raw_value.to_owned())
        };
        if key.is_empty() {
            return Err(SaveError::Malformed {
                line: line_number,
                reason: "empty key".to_owned(),
            });
        }
        data.insert(key, value);
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> BTreeMap<String, String> {
        let mut data = BTreeMap::new();
        data.insert("hp".into(), "75".into());
        data.insert("scene".into(), "arena".into());
        data
    }

    #[test]
    fn round_trips_with_version_header() {
        let text = encode(&sample());
        assert!(text.starts_with("save_version=2\n"));
        assert_eq!(decode(&text).unwrap(), sample());
        // 纯 ASCII 值与 v1 逐字节兼容（转义对无特殊字符是恒等的）。
        assert!(text.contains("hp\t75\n"));
    }

    #[test]
    fn rejects_unsupported_version_and_missing_header() {
        assert_eq!(
            decode("save_version=3\nhp\t10"),
            Err(SaveError::UnsupportedVersion(3))
        );
        assert_eq!(
            decode("save_version=0\nhp\t10"),
            Err(SaveError::UnsupportedVersion(0))
        );
        assert_eq!(decode("hp\t10\n"), Err(SaveError::MissingVersion));
    }

    #[test]
    fn rejects_malformed_rows_with_line_number() {
        let error = decode("save_version=2\nhp\t75\nbroken").unwrap_err();
        assert_eq!(
            error,
            SaveError::Malformed {
                line: 3,
                reason: "row is not `key<TAB>value`".to_owned(),
            }
        );
        let error = decode("save_version=2\n\t75").unwrap_err();
        assert_eq!(
            error,
            SaveError::Malformed {
                line: 2,
                reason: "empty key".to_owned(),
            }
        );
        // v1 仍报同样的行错误（旧格式诊断不得退化）。
        let error = decode("save_version=1\n\t75").unwrap_err();
        assert_eq!(
            error,
            SaveError::Malformed {
                line: 2,
                reason: "empty key".to_owned(),
            }
        );
    }

    /// 回归：多行文本曾经写出伪行，使**整个存档**读取失败（已复现的 P0）。
    #[test]
    fn multiline_values_no_longer_corrupt_the_whole_save() {
        let mut data = sample();
        data.insert("note".into(), "line one\nline two".into());
        data.insert("tabbed".into(), "a\tb".into());
        data.insert("carr".into(), "x\ry".into());
        data.insert("slash".into(), "C:\\saves\\slot1".into());
        let text = encode(&data);
        // 每个键值对只占一行：key 行数 == 数据条数 + 1 版本头。
        assert_eq!(text.lines().count(), data.len() + 1);
        assert_eq!(decode(&text).unwrap(), data);
    }

    /// 回归：CJK 与控制字符必须无损往返。
    #[test]
    fn cjk_and_control_characters_round_trip() {
        let mut data = BTreeMap::new();
        data.insert("草药".into(), "灵芝 x2".into());
        data.insert("控制".into(), "a\u{7}b\u{0}c".into());
        let text = encode(&data);
        assert_eq!(decode(&text).unwrap(), data);
    }

    /// 回归：v1 存档仍然可读（零数据丢失）。
    #[test]
    fn legacy_v1_saves_still_load() {
        let legacy = "save_version=1\nhp\t75\nscene\tarena\n";
        let mut expected = BTreeMap::new();
        expected.insert("hp".to_owned(), "75".to_owned());
        expected.insert("scene".to_owned(), "arena".to_owned());
        assert_eq!(decode(legacy).unwrap(), expected);
    }

    /// v2 行里的坏转义必须报错（而不是当作字面文本静默读错）。
    #[test]
    fn malformed_escapes_are_rejected_not_silently_kept() {
        for (row, reason) in [
            ("note\ttrailing\\", "trailing backslash in escaped field"),
            ("note\t\\q", "unknown escape sequence: \\q"),
            ("note\t\\xZZ", "invalid \\x escape: \\xZZ"),
        ] {
            let text = format!("save_version=2\n{row}\n");
            let error = decode(&text).unwrap_err();
            assert_eq!(
                error,
                SaveError::Malformed {
                    line: 2,
                    reason: reason.to_owned(),
                },
                "row: {row}"
            );
        }
    }

    /// 键名恰好是版本头样式时不能被误判成版本行（否则该键会毁掉整个存档）。
    #[test]
    fn key_mimicking_the_version_header_is_still_data() {
        let mut data = BTreeMap::new();
        data.insert("save_version=1".into(), "not a header".into());
        let text = encode(&data);
        assert_eq!(decode(&text).unwrap(), data);
    }

    /// 空键在解码期被拒绝（写入期另有 API 层校验，见 script.rs `save.write`）。
    #[test]
    fn empty_key_is_rejected_by_decode() {
        assert!(matches!(
            decode("save_version=2\n\tvalue\n"),
            Err(SaveError::Malformed { .. })
        ));
    }
}
