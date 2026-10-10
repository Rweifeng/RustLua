//! `.atlas` 源文件格式：一张图集纹理与其中命名的像素区域。
//!
//! 行式文本，与 scene/prefab 手写格式同一约定：`#` 起始为注释、空行跳过、
//! `key=value` 头部、记录字段之间以制表符分隔（下面示例中的间隔即制表符）：
//!
//! ```text
//! atlas_version=1
//! texture=sprites/hero_sheet.png
//! region    idle_0    0    0    64    64
//! ```
//!
//! 区域坐标单位是**像素**，原点为纹理左上角，半开区间 `[x, x+width) × [y, y+height)`
//! （与 `docs/11-texture-atlas.md` 一致）。越界要拿纹理实际尺寸才能判定，因此留到
//! 渲染层 `SpriteAtlas::from_definition` 构造时由 `AtlasError` 拒绝；本层只拦语法、
//! 零尺寸、空名与缺字段。同名 region 后写覆盖先写（对齐 `SpriteAtlas::insert`）。

use std::{collections::BTreeMap, path::Path};

/// 当前唯一的 `.atlas` 文本格式版本。
pub const ATLAS_FORMAT_VERSION: u32 = 1;

/// 图集内的一个像素矩形（左上原点、半开区间）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtlasRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// 解析后的 `.atlas` 源：切哪张纹理、切出哪些命名区域。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AtlasDefinition {
    /// 纹理源文件相对工程的路径。
    pub texture: String,
    /// 命名区域；`BTreeMap` 保证写出顺序确定。
    pub regions: BTreeMap<String, AtlasRect>,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AtlasFormatError {
    #[error("{path}: 读取图集文件失败: {reason}")]
    Io { path: String, reason: String },
    #[error("第 {line} 行: {reason}")]
    Parse { line: usize, reason: String },
    #[error("缺少 atlas_version 头")]
    MissingVersion,
    #[error("缺少 texture 引用")]
    MissingTexture,
    #[error("至少需要一个 region")]
    MissingRegions,
}

fn parse_number(text: &str, line: usize, field: &str) -> Result<u32, AtlasFormatError> {
    text.parse::<u32>().map_err(|_| AtlasFormatError::Parse {
        line,
        reason: format!("{field} 不是非负整数: {text:?}"),
    })
}

impl AtlasDefinition {
    pub fn from_text(text: &str) -> Result<Self, AtlasFormatError> {
        let mut has_version = false;
        let mut texture: Option<String> = None;
        let mut regions = BTreeMap::new();
        for (index, line) in text.lines().enumerate() {
            let line_no = index + 1;
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(value) = line.strip_prefix("atlas_version=") {
                let parsed = parse_number(value.trim(), line_no, "atlas_version")?;
                if parsed != ATLAS_FORMAT_VERSION {
                    return Err(AtlasFormatError::Parse {
                        line: line_no,
                        reason: format!(
                            "不支持的 atlas_version: {parsed}（当前仅 {ATLAS_FORMAT_VERSION}）"
                        ),
                    });
                }
                has_version = true;
                continue;
            }
            if let Some(value) = line.strip_prefix("texture=") {
                let value = value.trim();
                if value.is_empty() {
                    return Err(AtlasFormatError::Parse {
                        line: line_no,
                        reason: "texture 路径为空".to_owned(),
                    });
                }
                if texture.is_some() {
                    return Err(AtlasFormatError::Parse {
                        line: line_no,
                        reason: "texture 重复定义".to_owned(),
                    });
                }
                texture = Some(value.to_owned());
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            match fields.first().copied() {
                Some("region") => {
                    if fields.len() != 6 {
                        return Err(AtlasFormatError::Parse {
                            line: line_no,
                            reason: format!("region 记录需要 6 个字段，实到 {}", fields.len()),
                        });
                    }
                    let name = fields[1];
                    if name.trim().is_empty() {
                        return Err(AtlasFormatError::Parse {
                            line: line_no,
                            reason: "region 名为空".to_owned(),
                        });
                    }
                    if name.contains('#') {
                        return Err(AtlasFormatError::Parse {
                            line: line_no,
                            reason: format!(
                                "region 名不能含 '#'（与场景引用语法 图集路径#区域名 冲突）: {name:?}"
                            ),
                        });
                    }
                    let x = parse_number(fields[2], line_no, "x")?;
                    let y = parse_number(fields[3], line_no, "y")?;
                    let width = parse_number(fields[4], line_no, "width")?;
                    let height = parse_number(fields[5], line_no, "height")?;
                    if width == 0 || height == 0 {
                        return Err(AtlasFormatError::Parse {
                            line: line_no,
                            reason: format!("region 尺寸不能为 0: {width}x{height}"),
                        });
                    }
                    regions.insert(
                        name.to_owned(),
                        AtlasRect {
                            x,
                            y,
                            width,
                            height,
                        },
                    );
                }
                _ => {
                    return Err(AtlasFormatError::Parse {
                        line: line_no,
                        reason: format!("未知记录: {:?}", fields.first().copied().unwrap_or("")),
                    });
                }
            }
        }
        if !has_version {
            return Err(AtlasFormatError::MissingVersion);
        }
        let texture = texture.ok_or(AtlasFormatError::MissingTexture)?;
        if regions.is_empty() {
            return Err(AtlasFormatError::MissingRegions);
        }
        Ok(Self { texture, regions })
    }

    pub fn to_text(&self) -> String {
        let mut out = format!(
            "atlas_version={ATLAS_FORMAT_VERSION}\ntexture={}\n",
            self.texture
        );
        for (name, rect) in &self.regions {
            out.push_str(&format!(
                "region\t{name}\t{}\t{}\t{}\t{}\n",
                rect.x, rect.y, rect.width, rect.height
            ));
        }
        out
    }

    /// 从磁盘读一个 `.atlas`。
    ///
    /// 不做发布包解码：assetc 只压缩 png/jpg/wav/ogg/mp3/ttf/otf/glb（`is_compressible_asset`），
    /// `.atlas` 在包里保持明文；它引用的纹理才需要 [`rustlua_infra::pack::decode`]。
    pub fn load(path: impl AsRef<Path>) -> Result<Self, AtlasFormatError> {
        let path = path.as_ref();
        // 经 `vfs` 读（**不是** `fs::read_to_string`）：wasm 上没有文件系统，裸读恒
        // `ENOSYS`。`.atlas` 在包里保持明文（见上一条注释），所以这里只取原始字节、
        // 不做 `pack::decode`；语义与 `fs::read_to_string` 一致（非 UTF-8 报错）。
        let bytes = rustlua_infra::vfs::read(path).map_err(|error| AtlasFormatError::Io {
            path: path.display().to_string(),
            reason: error.to_string(),
        })?;
        let text = String::from_utf8(bytes).map_err(|error| AtlasFormatError::Io {
            path: path.display().to_string(),
            reason: error.to_string(),
        })?;
        Self::from_text(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> AtlasDefinition {
        let mut regions = BTreeMap::new();
        regions.insert(
            "idle_1".to_owned(),
            AtlasRect {
                x: 64,
                y: 0,
                width: 64,
                height: 64,
            },
        );
        regions.insert(
            "idle_0".to_owned(),
            AtlasRect {
                x: 0,
                y: 0,
                width: 64,
                height: 64,
            },
        );
        AtlasDefinition {
            texture: "sprites/hero_sheet.png".to_owned(),
            regions,
        }
    }

    #[test]
    fn roundtrip_is_deterministic_and_sorted() {
        let def = sample();
        let text = def.to_text();
        let reparsed = AtlasDefinition::from_text(&text).unwrap();
        assert_eq!(reparsed, def);
        // BTreeMap 顺序稳定：idle_0 行先于 idle_1。
        let idle_0 = text.find("idle_0").unwrap();
        let idle_1 = text.find("idle_1").unwrap();
        assert!(idle_0 < idle_1);
    }

    #[test]
    fn skips_comments_and_blank_lines() {
        let text = "# hero 图集\n\natlas_version=1\t\n\n# cut sheet\ntexture=a.png\nregion\ta\t0\t0\t8\t8\n";
        let def = AtlasDefinition::from_text(text).unwrap();
        assert_eq!(def.texture, "a.png");
        assert_eq!(def.regions.len(), 1);
    }

    #[test]
    fn duplicate_region_name_last_wins() {
        let text =
            "atlas_version=1\ntexture=a.png\nregion\tdupe\t0\t0\t8\t8\nregion\tdupe\t8\t8\t4\t4\n";
        let def = AtlasDefinition::from_text(text).unwrap();
        assert_eq!(
            def.regions.get("dupe"),
            Some(&AtlasRect {
                x: 8,
                y: 8,
                width: 4,
                height: 4
            })
        );
    }

    #[test]
    fn rejects_missing_and_malformed_records() {
        // 缺 atlas_version
        assert_eq!(
            AtlasDefinition::from_text("texture=a.png\nregion\ta\t0\t0\t1\t1\n"),
            Err(AtlasFormatError::MissingVersion)
        );
        // 缺 texture
        assert_eq!(
            AtlasDefinition::from_text("atlas_version=1\nregion\ta\t0\t0\t1\t1\n"),
            Err(AtlasFormatError::MissingTexture)
        );
        // 无 region
        assert_eq!(
            AtlasDefinition::from_text("atlas_version=1\ntexture=a.png\n"),
            Err(AtlasFormatError::MissingRegions)
        );
        // 未知版本号
        let error =
            AtlasDefinition::from_text("atlas_version=2\ntexture=a.png\nregion\ta\t0\t0\t1\t1\n")
                .unwrap_err();
        assert!(matches!(error, AtlasFormatError::Parse { line: 1, .. }));
        // 零尺寸
        let error =
            AtlasDefinition::from_text("atlas_version=1\ntexture=a.png\nregion\ta\t0\t0\t0\t8\n")
                .unwrap_err();
        assert!(matches!(error, AtlasFormatError::Parse { line: 3, .. }));
        // 负坐标
        let error =
            AtlasDefinition::from_text("atlas_version=1\ntexture=a.png\nregion\ta\t-1\t0\t8\t8\n")
                .unwrap_err();
        assert!(matches!(error, AtlasFormatError::Parse { line: 3, .. }));
        // 空名
        let error =
            AtlasDefinition::from_text("atlas_version=1\ntexture=a.png\nregion\t \t0\t0\t8\t8\n")
                .unwrap_err();
        assert!(matches!(error, AtlasFormatError::Parse { line: 3, .. }));
        // 字段数不足
        let error =
            AtlasDefinition::from_text("atlas_version=1\ntexture=a.png\nregion\ta\t0\t0\t8\n")
                .unwrap_err();
        assert!(matches!(error, AtlasFormatError::Parse { line: 3, .. }));
        // 名字含 '#'：与场景引用语法 图集路径#区域名 冲突，必须在源头拒绝
        let error =
            AtlasDefinition::from_text("atlas_version=1\ntexture=a.png\nregion\ta#b\t0\t0\t8\t8\n")
                .unwrap_err();
        assert!(matches!(error, AtlasFormatError::Parse { line: 3, .. }));
        // 未知记录
        let error = AtlasDefinition::from_text("atlas_version=1\ntexture=a.png\nframe\ta\t0\n")
            .unwrap_err();
        assert!(matches!(error, AtlasFormatError::Parse { line: 3, .. }));
    }
}
