//! 编辑器快捷键绑定：**键位文本**的唯一解析/序列化口径。
//!
//! 动作清单（哪些动作、默认键、中文名）属于编辑器 crate —— 那里才同时知道菜单与分发；
//! 本模块只负责与 egui 无关的部分：把 `"Ctrl+Shift+P"` 解析成结构、规范化写回、
//! 发现两条动作占用同一个键。这样用户配置（`editor.json` 的 `shortcuts` 段）能在
//! 没有 egui 的测试/工具里往返，而键名到底存不存在由编辑器用 `egui::Key::from_name`
//! 在**改键时**校验（磁盘上被改坏的键名只会让这条动作回落默认，不会 panic）。
//!
//! **未知动作 id 原样保留**：`ShortcutBindings` 不认识的 id 也留在表里并在下次保存时
//! 写回，旧版本编辑器打开新版本配置不会把新动作的键位抹掉（与 `user_config` 的
//! 未知键策略一致）。

use std::collections::BTreeMap;

/// 一条快捷键：修饰键 + 一个主键。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutSpec {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// 主键名（大小写不敏感地比较；显示时保留原始写法）。
    pub key: String,
}

impl ShortcutSpec {
    /// 解析 `"Ctrl+Shift+P"` / `"Delete"` / `"alt+F5"`。
    ///
    /// 只做**结构**校验：修饰键最多各一次、必须恰好一个主键、主键是纯字母数字。
    /// 主键是不是 egui 认识的键由编辑器负责（见模块注释）。
    pub fn parse(text: &str) -> Result<Self, String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err("快捷键不能为空".to_owned());
        }
        let mut spec = Self {
            ctrl: false,
            shift: false,
            alt: false,
            key: String::new(),
        };
        for part in trimmed.split('+') {
            let token = part.trim();
            if token.is_empty() {
                return Err(format!("快捷键「{trimmed}」里有空的段"));
            }
            match token.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => {
                    if spec.ctrl {
                        return Err(format!("快捷键「{trimmed}」重复写了 Ctrl"));
                    }
                    spec.ctrl = true;
                }
                "shift" => {
                    if spec.shift {
                        return Err(format!("快捷键「{trimmed}」重复写了 Shift"));
                    }
                    spec.shift = true;
                }
                "alt" | "option" => {
                    if spec.alt {
                        return Err(format!("快捷键「{trimmed}」重复写了 Alt"));
                    }
                    spec.alt = true;
                }
                _ => {
                    if !spec.key.is_empty() {
                        return Err(format!("快捷键「{trimmed}」只能有一个主键"));
                    }
                    if !token.chars().all(|ch| ch.is_ascii_alphanumeric()) {
                        return Err(format!("无法识别的主键「{token}」"));
                    }
                    // 单字母统一大写：`ctrl+s` 与 `Ctrl+S` 落盘成同一个写法。
                    spec.key = if token.len() == 1 {
                        token.to_ascii_uppercase()
                    } else {
                        token.to_owned()
                    };
                }
            }
        }
        if spec.key.is_empty() {
            return Err(format!("快捷键「{trimmed}」缺少主键"));
        }
        Ok(spec)
    }

    /// 规范写法：`Ctrl+Alt+Shift+Key`（固定顺序，方便比较与显示）。
    #[must_use]
    pub fn display(&self) -> String {
        let mut text = String::new();
        if self.ctrl {
            text.push_str("Ctrl+");
        }
        if self.alt {
            text.push_str("Alt+");
        }
        if self.shift {
            text.push_str("Shift+");
        }
        text.push_str(&self.key);
        text
    }

    /// 是否带修饰键。
    #[must_use]
    pub fn has_modifier(&self) -> bool {
        self.ctrl || self.shift || self.alt
    }

    /// 是否就是同一个组合（主键大小写不敏感）。
    #[must_use]
    pub fn same_as(&self, other: &Self) -> bool {
        self.ctrl == other.ctrl
            && self.shift == other.shift
            && self.alt == other.alt
            && self.key.eq_ignore_ascii_case(&other.key)
    }
}

/// 动作 id → 键位文本（**只存用户改过的与改过的默认值**，缺省即动作默认键）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShortcutBindings {
    bindings: BTreeMap<String, String>,
}

impl ShortcutBindings {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&str> {
        self.bindings.get(id).map(String::as_str)
    }

    /// 设置一条键位；非法文本返回 `Err`，与现值相同返回 `Ok(false)`（不算变化、不写盘）。
    pub fn set(&mut self, id: &str, spec: &str) -> Result<bool, String> {
        let parsed = ShortcutSpec::parse(spec)?;
        let canonical = parsed.display();
        if self.get(id) == Some(canonical.as_str()) {
            return Ok(false);
        }
        self.bindings.insert(id.to_owned(), canonical);
        Ok(true)
    }

    /// 删除一条显式键位（调用方随之回到动作默认键）。
    pub fn remove(&mut self, id: &str) -> bool {
        self.bindings.remove(id).is_some()
    }

    pub fn clear(&mut self) {
        self.bindings.clear();
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// 全部条目（含本版本不认识的 id；保存时原样写回）。
    #[must_use]
    pub fn entries(&self) -> &BTreeMap<String, String> {
        &self.bindings
    }

    /// 从磁盘读到的 `id → 文本` 建表；非法文本整条丢弃（动作回落默认键）。
    #[must_use]
    pub fn from_entries(entries: BTreeMap<String, String>) -> Self {
        let mut bindings = Self::new();
        for (id, text) in entries {
            if let Ok(parsed) = ShortcutSpec::parse(&text) {
                bindings.bindings.insert(id, parsed.display());
            }
        }
        bindings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_round_trip_in_canonical_order() {
        let spec = ShortcutSpec::parse("shift+ctrl+p").unwrap();
        assert_eq!(spec.display(), "Ctrl+Shift+P", "顺序固定，大小写规范化");
        let spec = ShortcutSpec::parse(" Delete ").unwrap();
        assert_eq!(spec.display(), "Delete");
        assert!(!spec.has_modifier());
        assert!(ShortcutSpec::parse("Ctrl+P").unwrap().has_modifier());
    }

    #[test]
    fn malformed_specs_are_rejected_with_a_reason() {
        for text in [
            "",
            "  ",
            "Ctrl+",
            "+P",
            "Ctrl+Ctrl+P",
            "Ctrl+P+Q",
            "Ctrl+Space Key",
        ] {
            let error = ShortcutSpec::parse(text).expect_err(&format!("「{text}」应当被拒绝"));
            assert!(!error.is_empty(), "拒绝时必须给出原因");
        }
    }

    #[test]
    fn same_specs_compare_case_insensitively_on_the_key() {
        let lower = ShortcutSpec::parse("ctrl+p").unwrap();
        let upper = ShortcutSpec::parse("Ctrl+P").unwrap();
        assert!(lower.same_as(&upper));
        assert!(!ShortcutSpec::parse("Ctrl+Shift+P").unwrap().same_as(&upper));
    }

    #[test]
    fn bindings_normalize_on_write_and_report_changes() {
        let mut bindings = ShortcutBindings::new();
        assert_eq!(bindings.get("undo"), None);
        assert!(bindings.set("undo", "ctrl+z").unwrap());
        assert_eq!(bindings.get("undo"), Some("Ctrl+Z"), "落表时是规范写法");
        assert!(!bindings.set("undo", "Ctrl+Z").unwrap(), "重复设置不算变化");
        assert!(bindings.set("undo", "Ctrl+U").unwrap());
        assert_eq!(bindings.get("undo"), Some("Ctrl+U"));
        assert!(bindings.remove("undo"));
        assert!(!bindings.remove("undo"));
        assert!(bindings.set("undo", "not a shortcut").is_err());
        assert_eq!(bindings.get("undo"), None, "非法值不能写进表");
    }

    /// 磁盘上的坏值整条丢弃；不认识的 id 保留（下次保存原样写回）。
    #[test]
    fn from_entries_drops_bad_specs_and_keeps_unknown_ids() {
        let mut entries = BTreeMap::new();
        entries.insert("undo".to_owned(), "ctrl+z".to_owned());
        entries.insert("future_action".to_owned(), "Alt+F9".to_owned());
        entries.insert("broken".to_owned(), "Ctrl+".to_owned());
        let bindings = ShortcutBindings::from_entries(entries);
        assert_eq!(bindings.get("undo"), Some("Ctrl+Z"));
        assert_eq!(bindings.get("future_action"), Some("Alt+F9"));
        assert_eq!(bindings.get("broken"), None);
        assert!(bindings.entries().contains_key("future_action"));
    }
}
