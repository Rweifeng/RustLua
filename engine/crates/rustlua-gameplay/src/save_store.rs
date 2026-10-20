//! 存档域的**能力单元**（`docs/62` P1 ⑤ 第一刀：`ScriptState` 的存档字段归位）。
//!
//! ## 为什么这个类型住在这里
//!
//! 存档的**格式**本来就在本 crate（[`crate::save::encode`] / [`crate::save::decode`]），
//! 键值数据的读写方也几乎全在存档域：`save.write/read/blob_*`、`map.persist` 的
//! `__map*` 键、开局 `save.clear`。它却一直作为 `ScriptState` 的一个内联字段存在，
//! 于是"谁拥有存档数据"这个问题的答案落在 VM 宿主里——`rustlua-scripting` 既解析 Lua
//! 又保管玩家的存档，两边都改不动。
//!
//! 现在它是一个 [`Handle`]（`rustlua-lua-api` 的能力单元）：
//!
//! - **绑定**（无论暂时还留在 `rustlua-scripting`，还是下一步整体搬到本 crate）借它读写；
//! - **宿主**（`RuntimeScene` 的 `set_save_path`、编辑器的存档面板）借它配置路径；
//! - 双方共享同一份状态，类型上互不认识——和 `audio`/`input` 的 `Handle` 一个形状。
//!
//! 语义上刻意保持"笨"：没有校验、没有 IO。**落盘**留在绑定侧（它要按平台分叉：
//! 装了平台槽走 `save_slot`，否则走 `std::fs`），**格式**留在 [`crate::save`]。
//! 这个单元只回答"现在存了哪些键、写到哪个文件、哪个槽位"。
//!
//! ## 为什么 `insert` 拒绝空键
//!
//! `crate::save::decode` 遇到空键会把**整份**存档判为损坏。空键一旦写进去，玩家下次
//! 读档就是"存档没了"而不是"写错了"——所以拒绝点必须在这里（唯一入口），而不是散落在
//! 每个绑定里各判一次。

use rustlua_lua_api::Handle;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 存档域的键值数据 + 落点。
#[derive(Debug, Default)]
pub struct SaveStore {
    data: BTreeMap<String, String>,
    /// 基础存档文件路径（不含槽位后缀）。`None` = 宿主还没定（发布包早期、纯脚本测试）。
    path: Option<PathBuf>,
    /// 当前槽位（`None` = 默认槽）。空串按 `None` 处理，见 [`SaveStore::set_slot`]。
    slot: Option<String>,
}

impl SaveStore {
    /// 建一份能力单元。`what` 是重入报错里的短名（与 Lua 模块同名）。
    #[must_use]
    pub fn handle() -> Handle<Self> {
        Handle::new(Self::default(), "save")
    }

    /// 全部键值（只读）。
    #[must_use]
    pub fn data(&self) -> &BTreeMap<String, String> {
        &self.data
    }

    /// 全部键值（可写）。批量操作（`clear`、整表替换）用它，单键操作用下面的方法。
    pub fn data_mut(&mut self) -> &mut BTreeMap<String, String> {
        &mut self.data
    }

    /// 写入一个键。空键返回 `false` 且**不写入**（空键会让整份存档在下次读档时报损坏）。
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) -> bool {
        let key = key.into();
        if key.is_empty() {
            return false;
        }
        self.data.insert(key, value.into());
        true
    }

    /// 读取一个键。
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&String> {
        self.data.get(key)
    }

    /// 删除一个键，返回是否删除成功。
    pub fn remove(&mut self, key: &str) -> bool {
        self.data.remove(key).is_some()
    }

    /// 键数量。
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// 清空全部键，返回清掉的条数。
    pub fn clear(&mut self) -> usize {
        let removed = self.data.len();
        self.data.clear();
        removed
    }

    /// 整表替换（读档成功时用；失败时**不动**现有数据，避免半份存档）。
    pub fn replace_data(&mut self, data: BTreeMap<String, String>) -> usize {
        let previous = self.data.len();
        self.data = data;
        previous
    }

    /// 基础存档路径。
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// 设置基础存档路径（宿主在启动时定一次）。
    pub fn set_path(&mut self, path: Option<PathBuf>) {
        self.path = path;
    }

    /// 当前槽位。
    #[must_use]
    pub fn slot(&self) -> Option<&str> {
        self.slot.as_deref()
    }

    /// 设置当前槽位。空串等同"回到默认槽"——Lua 侧 `save.slot("")` 与 `save.slot(nil)`
    /// 必须是同一件事，否则存档会写到一个空后缀的文件里，玩家看不到也读不回来。
    pub fn set_slot(&mut self, slot: Option<String>) {
        self.slot = slot.filter(|value| !value.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_keys_and_normalizes_slots() {
        let mut store = SaveStore::default();
        assert!(!store.insert("", "x"), "空键必须被拒绝");
        assert!(store.is_empty());
        assert!(store.insert("hp", "10"));
        assert_eq!(store.get("hp").map(String::as_str), Some("10"));
        assert_eq!(store.len(), 1);

        store.set_slot(Some(String::new()));
        assert_eq!(store.slot(), None, "空串槽位等同默认槽");
        store.set_slot(Some("slot2".to_owned()));
        assert_eq!(store.slot(), Some("slot2"));
    }

    #[test]
    fn clear_and_replace_report_previous_size() {
        let mut store = SaveStore::default();
        store.insert("a", "1");
        store.insert("b", "2");
        assert_eq!(store.clear(), 2);
        assert!(store.is_empty());

        let mut next = BTreeMap::new();
        next.insert("c".to_owned(), "3".to_owned());
        assert_eq!(store.replace_data(next), 0);
        assert_eq!(store.get("c").map(String::as_str), Some("3"));
        assert_eq!(store.path(), None);
        store.set_path(Some(PathBuf::from("save.json")));
        assert_eq!(store.path(), Some(Path::new("save.json")));
    }
}
