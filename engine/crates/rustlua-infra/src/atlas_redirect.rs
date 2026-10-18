//! `atlas_redirect.json`（`docs/67` §6.4）：构建期图集 cook 写、运行期装配时读。
//!
//! 为什么不改工程源：图集把 `assets/art/ui/hud/ui_icon_hp.png` 换成
//! `assets/atlases/ui_hud-0.rtex` + 区域 `ui_icon_hp`，但场景/Lua 里写的仍是原路径。
//! 重写工程源会污染所有场景与脚本，而且让"关掉图集"变成一次大手术。所以做成一张
//! **构建期生成的重定向表**：装配期查一次，产物与源工程解耦。
//!
//! 放在 infra 的理由与 `bundle::plan` 相同：构建期写、运行期读，两边必须是同一份定义，
//! 而 `rustlua-render` 不（也不该）依赖 `assetc`。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// 重定向表文件名（放包根，与 `manifest.txt` / `bundle_plan.json` 同级）。
pub const REDIRECT_FILE: &str = "atlas_redirect.json";
/// 格式标识（改动字段含义时自增）。
pub const REDIRECT_FORMAT: &str = "assetc-atlas-redirect-1";

/// 一条重定向：原图路径 → （图集路径，区域名）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedirectEntry {
    /// `.atlas` 的工程相对路径。
    pub atlas: String,
    pub region: String,
}

/// 重定向表。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtlasRedirect {
    pub schema: u32,
    pub format: String,
    /// 键是**工程相对的原图路径**（`assets/art/ui/hud/ui_icon_hp.png`）。
    pub entries: BTreeMap<String, RedirectEntry>,
}

impl AtlasRedirect {
    /// 空表（没有 cook 时也安全：查不到 → 原样透传）。
    #[must_use]
    pub fn empty() -> Self {
        Self {
            schema: 1,
            format: REDIRECT_FORMAT.to_owned(),
            entries: BTreeMap::new(),
        }
    }

    /// 解析；格式标识不认就报错（不猜）。
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let redirect: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("atlas_redirect 解析失败：{error}"))?;
        if redirect.format != REDIRECT_FORMAT {
            return Err(format!(
                "atlas_redirect 格式不认：{}（期望 {REDIRECT_FORMAT}）",
                redirect.format
            ));
        }
        Ok(redirect)
    }

    /// 查一条重定向。
    pub fn lookup(&self, original: &str) -> Option<&RedirectEntry> {
        self.entries.get(original)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 所有源图路径（`assetc` 删源图前用它自检"删了必须有重定向"）。
    pub fn sources(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rejects_unknown_format() {
        let good = br#"{"schema":1,"format":"assetc-atlas-redirect-1","entries":{}}"#;
        let redirect = AtlasRedirect::parse(good).expect("合法表");
        assert!(redirect.is_empty());

        let bad = br#"{"schema":1,"format":"other","entries":{}}"#;
        assert!(AtlasRedirect::parse(bad).is_err());
    }

    #[test]
    fn lookup_and_sources_roundtrip() {
        let text = br#"{
            "schema": 1,
            "format": "assetc-atlas-redirect-1",
            "entries": {
                "assets/art/ui/hud/a.png": { "atlas": "assets/atlases/ui-0.atlas", "region": "a" }
            }
        }"#;
        let redirect = AtlasRedirect::parse(text).unwrap();
        assert_eq!(redirect.len(), 1);
        let entry = redirect.lookup("assets/art/ui/hud/a.png").unwrap();
        assert_eq!(entry.region, "a");
        assert_eq!(entry.atlas, "assets/atlases/ui-0.atlas");
        assert_eq!(
            redirect.sources().collect::<Vec<_>>(),
            vec!["assets/art/ui/hud/a.png"]
        );
        assert!(redirect.lookup("assets/art/ui/hud/b.png").is_none());
    }
}
