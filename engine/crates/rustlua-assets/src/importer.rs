//! 导入契约的**运行时那一半**：[`Importer`] trait、[`ImporterRegistry`]、批量导入的结果类型，
//! 以及几个不依赖解码器的校验函数（JSON / RON / 空文件 / BOM）。
//!
//! 六个内置实现**不在**这里，它们住在 `rustlua-assets-import`（要 `image`/`rodio`/`ab_glyph`/
//! `mlua`，而本 crate 是浏览器也要编的运行时）。拆分动机见 `docs/62` §6 P1 与
//! `tasks/done/P1-降耦合收口.md` 阶段 ③。
//!
//! 因此本模块只做两件事：定义"导入器长什么样"（trait），以及"按 kind/扩展名查导入器"
//! （注册表）。谁得到哪些实现由调用方决定：构建工具用
//! `rustlua_assets_import::full_registry()`，只导入普通资源时用
//! `rustlua_assets_import::builtin()`。批量驱动（`run_import_batch*`）同样在导入层，
//! 因为它要写导入缓存。

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::resource_db::AssetId;
use crate::resource_kind::ResourceKind;
/// One artifact an importer wants written to the cache.
#[derive(Clone, Debug)]
pub struct ImportArtifact {
    /// Project-root-relative path (e.g. `.cache/cache/{id}-{hash}.bin`).
    pub path: PathBuf,
    /// File contents of this artifact.
    pub bytes: Vec<u8>,
}

/// Validates a source file and produces cache artifacts for one resource kind.
pub trait Importer: Send + Sync {
    /// Importer version for this kind; bumping it forces a re-import of every asset.
    fn version(&self) -> &'static str;
    /// The unified resource family this importer handles.
    fn kind(&self) -> ResourceKind;
    /// Lowercase extensions this importer accepts, without the leading dot.
    fn extensions(&self) -> &'static [&'static str];
    /// Rejects malformed or unsupported sources before they reach the cache.
    fn validate(&self, source: &Path, bytes: &[u8]) -> Result<(), String>;
    /// Produces the cached artifacts. The default copies the raw source bytes
    /// into a single artifact; formatters may override this to transform.
    fn produce(
        &self,
        bytes: &[u8],
        id: AssetId,
        hash: &str,
    ) -> Result<Vec<ImportArtifact>, String> {
        Ok(vec![ImportArtifact {
            path: PathBuf::from(rustlua_infra::PROJECT_WORK_DIR)
                .join("cache")
                .join(format!("{id}-{hash}.bin")),
            bytes: bytes.to_vec(),
        }])
    }
}

/// Rejects empty sources uniformly across builtin importers.
pub fn reject_empty(source: &Path, bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() {
        Err(format!("{}: 源文件为空", source.display()))
    } else {
        Ok(())
    }
}

/// M20-01：UTF-8 BOM 检查。
///
/// Lua 解析器不接受 BOM（报 `unexpected symbol near '<\239>'`），而文本编辑器
/// 「另存为 UTF-8」常带 BOM；这里给出可操作的诊断，而不是让错误在运行时以
/// 乱码形式出现（场景加载器已自带 BOM 容忍，脚本/数据文件按无 BOM 约定）。
pub fn reject_bom(source: &Path, bytes: &[u8], label: &str) -> Result<(), String> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Err(format!(
            "{}: {label}含 UTF-8 BOM，请另存为「UTF-8 无 BOM」",
            source.display()
        ));
    }
    Ok(())
}

/// M20-01：`.json` 数据表严格校验；serde_json 的错误自带行/列定位。
pub fn validate_json_data(source: &Path, bytes: &[u8]) -> Result<(), String> {
    reject_bom(source, bytes, "JSON 文件")?;
    let text = std::str::from_utf8(bytes)
        .map_err(|error| format!("{}: JSON 编码非法（需 UTF-8）: {error}", source.display()))?;
    serde_json::from_str::<serde_json::Value>(text)
        .map_err(|error| format!("{}: JSON 解析失败: {error}", source.display()))?;
    Ok(())
}

/// M20-01：`.ron` 结构化校验（分隔符/字符串配平 + 定位到行列）。
///
/// 说明：工作区未引入 `ron` 依赖（离线环境无法新增 crates），此处做结构级
/// 校验以拦截截断/损坏文件；完整 RON 语法解析留待引入依赖后替换。
pub fn validate_ron_structure(source: &Path, bytes: &[u8]) -> Result<(), String> {
    reject_bom(source, bytes, "RON 文件")?;
    let text = std::str::from_utf8(bytes)
        .map_err(|error| format!("{}: RON 编码非法（需 UTF-8）: {error}", source.display()))?;
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut line = 1usize;
    let mut column = 1usize;
    for (index, character) in text.char_indices() {
        if character == '\n' {
            line += 1;
            column = 1;
            continue;
        }
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            column += 1;
            continue;
        }
        match character {
            '"' => in_string = true,
            '(' | '[' | '{' => stack.push((character, index)),
            ')' | ']' | '}' => {
                let expected = match character {
                    ')' => '(',
                    ']' => '[',
                    _ => '{',
                };
                match stack.pop() {
                    Some((open, _)) if open == expected => {}
                    Some((open, open_index)) => {
                        let (open_line, open_column) = line_column(text, open_index);
                        return Err(format!(
                            "{}: RON 结构错误: 第 {line} 行第 {column} 列的 '{character}' 与第 {open_line} 行第 {open_column} 列的 '{open}' 不匹配",
                            source.display()
                        ));
                    }
                    None => {
                        return Err(format!(
                            "{}: RON 结构错误: 第 {line} 行第 {column} 列出现多余的 '{character}'",
                            source.display()
                        ));
                    }
                }
            }
            _ => {}
        }
        column += 1;
    }
    if in_string {
        return Err(format!(
            "{}: RON 结构错误: 字符串未闭合（起始于第 {line} 行）",
            source.display()
        ));
    }
    if let Some((open, open_index)) = stack.pop() {
        let (open_line, open_column) = line_column(text, open_index);
        return Err(format!(
            "{}: RON 结构错误: 第 {open_line} 行第 {open_column} 列的 '{open}' 未闭合",
            source.display()
        ));
    }
    Ok(())
}

/// 由字节偏移估算 (line, column)（仅用于 RON 结构诊断，允许 O(n) 回扫）。
fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut column = 1usize;
    for (index, character) in text.char_indices() {
        if index >= offset {
            break;
        }
        if character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

/// The set of builtin importers, keyed by resource kind.
pub struct ImporterRegistry {
    map: HashMap<ResourceKind, &'static dyn Importer>,
}

impl ImporterRegistry {
    /// Builds a registry from an explicit set of importers (used by upper layers to
    /// combine the assets-level core importers with crate-specific ones).
    #[must_use]
    pub fn from_importers(importers: &[&'static dyn Importer]) -> Self {
        let mut map = HashMap::with_capacity(importers.len());
        for importer in importers {
            map.insert(importer.kind(), *importer);
        }
        Self { map }
    }

    /// Returns the importer for a resource kind, if one is built in.
    pub fn get(&self, kind: ResourceKind) -> Option<&'static dyn Importer> {
        self.map.get(&kind).copied()
    }

    /// Returns the importer matching a file path's extension, or `None` for
    /// kinds that are not yet importable (`ResourceKind::Other`).
    pub fn from_path(&self, path: &Path) -> Option<&'static dyn Importer> {
        self.get(ResourceKind::from_path(path))
    }
}
