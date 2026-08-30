//! Versioned save-data serialization: key/value pairs persisted as a
//! `save_version=<n>` header plus tab-separated rows. Corrupt or unsupported
//! saves are rejected without panicking.

use std::{collections::BTreeMap, error::Error, fmt};

/// The current save file format version.
pub const CURRENT_SAVE_VERSION: u32 = 1;

/// Errors produced while encoding or decoding a save file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SaveError {
    UnsupportedVersion(u32),
    MissingVersion,
    Malformed { line: usize, reason: String },
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported save version: {version}")
            }
            Self::MissingVersion => write!(f, "save file is missing its version header"),
            Self::Malformed { line, reason } => {
                write!(f, "malformed save file at line {line}: {reason}")
            }
        }
    }
}

impl Error for SaveError {}

/// Serializes save data into the versioned text format.
pub fn encode(data: &BTreeMap<String, String>) -> String {
    let mut out = format!("save_version={CURRENT_SAVE_VERSION}\n");
    for (key, value) in data {
        out.push_str(&format!("{key}\t{value}\n"));
    }
    out
}

/// Parses save data and returns it together with the file's version, accepting
/// any version number so Lua can run `on_migrate` on older saves (M16-09).
/// A save with no `save_version=` header is treated as version 0 (pre-versioning
/// saves), so the migration hook can upgrade them. Malformed rows still error.
pub fn decode_with_version(text: &str) -> Result<(BTreeMap<String, String>, u32), SaveError> {
    let mut data = BTreeMap::new();
    let mut version = 0;
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(value) = line.strip_prefix("save_version=") {
            version = value.parse().map_err(|_| SaveError::Malformed {
                line: index + 1,
                reason: "version is not an integer".to_owned(),
            })?;
            continue;
        }
        let Some((key, value)) = line.split_once('\t') else {
            return Err(SaveError::Malformed {
                line: index + 1,
                reason: "row is not `key<TAB>value`".to_owned(),
            });
        };
        if key.is_empty() {
            return Err(SaveError::Malformed {
                line: index + 1,
                reason: "empty key".to_owned(),
            });
        }
        data.insert(key.to_owned(), value.to_owned());
    }
    Ok((data, version))
}

/// Parses save data back from the versioned text format. Returns a
/// [`SaveError`] for a missing/unsupported version or a malformed row.
pub fn decode(text: &str) -> Result<BTreeMap<String, String>, SaveError> {
    let mut data = BTreeMap::new();
    let mut version = None;
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(value) = line.strip_prefix("save_version=") {
            version = Some(value.parse().map_err(|_| SaveError::Malformed {
                line: index + 1,
                reason: "version is not an integer".to_owned(),
            })?);
            continue;
        }
        let Some((key, value)) = line.split_once('\t') else {
            return Err(SaveError::Malformed {
                line: index + 1,
                reason: "row is not `key<TAB>value`".to_owned(),
            });
        };
        if key.is_empty() {
            return Err(SaveError::Malformed {
                line: index + 1,
                reason: "empty key".to_owned(),
            });
        }
        data.insert(key.to_owned(), value.to_owned());
    }
    match version {
        Some(version) if version == CURRENT_SAVE_VERSION => Ok(data),
        Some(version) => Err(SaveError::UnsupportedVersion(version)),
        None => Err(SaveError::MissingVersion),
    }
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
    fn decode_with_version_accepts_older_and_headerless_saves() {
        use super::{decode_with_version, encode};
        let old = "save_version=0\nhp\t10\n";
        let (data, version) = decode_with_version(old).unwrap();
        assert_eq!(version, 0);
        assert_eq!(data.get("hp").map(String::as_str), Some("10"));
        // 无版本头的旧存档视为 v0，便于 on_migrate 升级。
        let (data, version) = decode_with_version("hp\t10\n").unwrap();
        assert_eq!(version, 0);
        assert_eq!(data.get("hp").map(String::as_str), Some("10"));
        // 当前版本 round-trip 仍成立。
        let (data, version) = decode_with_version(&encode(&sample())).unwrap();
        assert_eq!(version, CURRENT_SAVE_VERSION);
        assert_eq!(data.get("hp").map(String::as_str), Some("75"));
        // 畸形行仍是错误。
        assert!(decode_with_version("broken\n").is_err());
    }

    #[test]
    fn round_trips_with_version_header() {
        let text = encode(&sample());
        assert!(text.starts_with("save_version=1\n"));
        assert_eq!(decode(&text).unwrap(), sample());
    }

    #[test]
    fn rejects_unsupported_version_and_missing_header() {
        assert_eq!(
            decode("save_version=2\nhp\t10"),
            Err(SaveError::UnsupportedVersion(2))
        );
        assert_eq!(decode("hp\t10\n"), Err(SaveError::MissingVersion));
    }

    #[test]
    fn rejects_malformed_rows_with_line_number() {
        let error = decode("save_version=1\nhp\t75\nbroken").unwrap_err();
        assert_eq!(
            error,
            SaveError::Malformed {
                line: 3,
                reason: "row is not `key<TAB>value`".to_owned(),
            }
        );
        let error = decode("save_version=1\n\t75").unwrap_err();
        assert_eq!(
            error,
            SaveError::Malformed {
                line: 2,
                reason: "empty key".to_owned(),
            }
        );
    }
}
