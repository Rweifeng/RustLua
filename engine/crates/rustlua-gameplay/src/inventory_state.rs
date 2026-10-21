//! 物品/背包/商店域的**能力单元**（`docs/62` P1 ⑤-6：从 `ScriptState` 搬进物品域）。
//!
//! ## 为什么这个类型住在这里
//!
//! 三样数据的**类型**本来就在本 crate（[`ItemDefinition`] / [`Inventory`] / [`ItemStack`] 都在
//! [`crate::gameplay`]），但"谁保管当前这一局的背包与商店目录"的答案一直在 VM 宿主里：
//! `ScriptState.inventory` 是 `rustlua-scripting` 的一个内联字段。于是脚本层既管虚拟机、
//! 又管物品规则（堆叠上限、槽位越界、换位与合并的 nine 种边界）。
//!
//! 现在它是 [`Handle<InventoryState>`](rustlua_lua_api::Handle)：绑定（[`crate::inventory_lua`]）
//! 与宿主（存档面板、编辑器数据面板）共享同一份状态，类型上互不认识——与 `SaveStore` /
//! `WorldDataState` / `GameClock` 一个形状。
//!
//! ## 规则为什么也搬过来
//!
//! `swap` / `move` 不只是"取两个背包出来"，它们是**物品域的业务规则**：
//! 同名物品向目标堆叠至上限、目标为空槽则整段放入、目标装别的物品则一件不动、
//! 放不下的部分退回源背包。这些规则原先散在 `script/mod.rs` 的七个自由函数里，
//! 只有 Lua 绑定会调它们；它们读写的东西全在本类型里，所以跟着数据一起走。
//! 报错文本逐字保留（[`Result<_, String>`]，绑定侧用 `api_error` 包成 Lua 错误）。

use std::collections::HashMap;

use rustlua_lua_api::Handle;

use crate::gameplay::{Inventory, ItemDefinition, ItemStack};

/// 物品定义 + 各背包内容 + 商店目录。
#[derive(Debug, Default)]
pub struct InventoryState {
    item_defs: HashMap<String, ItemDefinition>,
    inventories: HashMap<String, Inventory>,
    /// Shop catalogs: name -> `{ item_id: { buy, sell } }` (M16-10).
    shops: HashMap<String, serde_json::Value>,
}

impl InventoryState {
    /// 建一份能力单元。`what` 是重入报错里的短名（与 Lua 模块同名）。
    #[must_use]
    pub fn handle() -> Handle<Self> {
        Handle::new(Self::default(), "inventory")
    }

    /// 场景切换只丢弃商店目录；物品定义与背包内容随存档/持久实体存活（既有语义）。
    pub fn clear_for_scene_transition(&mut self) {
        self.shops.clear();
    }

    // ---- 物品定义 ----

    /// 登记一个物品定义（同名覆盖，与改造前 `HashMap::insert` 一致）。
    pub fn define(&mut self, definition: ItemDefinition) {
        self.item_defs.insert(definition.id.clone(), definition);
    }

    /// 按 id 取物品定义。
    #[must_use]
    pub fn item_def(&self, id: &str) -> Option<&ItemDefinition> {
        self.item_defs.get(id)
    }

    /// 物品堆叠上限——未定义的物品按 99（与历史行为一致）。
    #[must_use]
    pub fn item_stack_limit(&self, item_id: &str) -> u32 {
        self.item_defs
            .get(item_id)
            .map_or(99, |definition| definition.stack)
    }

    /// 已登记的物品定义数（宿主/诊断用）。
    #[must_use]
    pub fn item_def_count(&self) -> usize {
        self.item_defs.len()
    }

    // ---- 背包 ----

    /// 建/覆盖一个背包。
    pub fn insert_inventory(&mut self, id: impl Into<String>, inventory: Inventory) {
        self.inventories.insert(id.into(), inventory);
    }

    /// 少一层嵌套的建包入口（`inv.create` / `inv.hotbar` / `container.open`）。
    pub fn new_inventory(&mut self, id: impl Into<String>, capacity: usize) {
        self.insert_inventory(id, Inventory::new(capacity));
    }

    /// 按 id 取背包（只读）。
    #[must_use]
    pub fn inventory(&self, id: &str) -> Option<&Inventory> {
        self.inventories.get(id)
    }

    /// 按 id 取背包（可写）。
    pub fn inventory_mut(&mut self, id: &str) -> Option<&mut Inventory> {
        self.inventories.get_mut(id)
    }

    /// 背包是否存在。
    #[must_use]
    pub fn contains_inventory(&self, id: &str) -> bool {
        self.inventories.contains_key(id)
    }

    /// 删掉一个背包，返回它是否存在（`container.close`）。
    pub fn remove_inventory(&mut self, id: &str) -> bool {
        self.inventories.remove(id).is_some()
    }

    /// 现有背包数（宿主/诊断用）。
    #[must_use]
    pub fn inventory_count(&self) -> usize {
        self.inventories.len()
    }

    // ---- 商店目录 ----

    /// 登记一份商店目录（`shop.set_catalog`）。
    pub fn set_shop_catalog(&mut self, name: impl Into<String>, catalog: serde_json::Value) {
        self.shops.insert(name.into(), catalog);
    }

    /// 按名取商店目录。
    #[must_use]
    pub fn shop_catalog(&self, name: &str) -> Option<&serde_json::Value> {
        self.shops.get(name)
    }

    /// 商店目录数（宿主/诊断用）。
    #[must_use]
    pub fn shop_count(&self) -> usize {
        self.shops.len()
    }

    // ---- 槽位规则（原 `script/mod.rs` 的七个自由函数） ----

    fn clone_inventory(&self, id: &str) -> Result<Inventory, String> {
        self.inventory(id)
            .cloned()
            .ok_or_else(|| format!("inventory {id} not found"))
    }

    /// `inv.swap(from_id, from_index, to_id, to_index)`：两个槽位互换。
    ///
    /// 同背包走一条路径、跨背包走另一条，但槽位检查与报错措辞共用。
    pub fn swap(
        &mut self,
        from_id: &str,
        from_index: i64,
        to_id: &str,
        to_index: i64,
    ) -> Result<bool, String> {
        let from_index = inventory_index(from_index)?;
        let to_index = inventory_index(to_index)?;
        if from_id == to_id {
            let mut inventory = self.clone_inventory(from_id)?;
            check_inventory_slot(&inventory, from_index, from_id)?;
            check_inventory_slot(&inventory, to_index, to_id)?;
            inventory.slots.swap(from_index, to_index);
            self.insert_inventory(from_id.to_owned(), inventory);
            return Ok(true);
        }
        let mut from = self.clone_inventory(from_id)?;
        let mut to = self.clone_inventory(to_id)?;
        check_inventory_slot(&from, from_index, from_id)?;
        check_inventory_slot(&to, to_index, to_id)?;
        let from_slot = from.slots[from_index].clone();
        let to_slot = to.slots[to_index].clone();
        from.slots[from_index] = to_slot;
        to.slots[to_index] = from_slot;
        self.insert_inventory(from_id.to_owned(), from);
        self.insert_inventory(to_id.to_owned(), to);
        Ok(true)
    }

    /// `inv.move(...)` / `container.move(...)`：把一个槽位搬到另一个槽位。
    ///
    /// `to_index` 为空表示**自动落位**（放进第一个放得下的槽位）；`count` 为空表示整堆。
    /// 返回值是实际搬动的件数。
    pub fn move_between(
        &mut self,
        from_id: &str,
        from_index: i64,
        to_id: &str,
        to_index: Option<i64>,
        count: Option<i64>,
    ) -> Result<i64, String> {
        let from_index = inventory_index(from_index)?;
        let to_index = to_index.map(inventory_index).transpose()?;
        if from_id == to_id {
            let mut inventory = self.clone_inventory(from_id)?;
            check_inventory_slot(&inventory, from_index, from_id)?;
            let source = inventory.slots[from_index].clone();
            if source.item_id.is_empty() {
                return Ok(0);
            }
            let max_stack = self.item_stack_limit(&source.item_id);
            if let Some(to_index) = to_index {
                check_inventory_slot(&inventory, to_index, to_id)?;
                let (from_slot, to_slot, moved) =
                    slot_merge_result(&source, &inventory.slots[to_index], max_stack, count);
                inventory.slots[from_index] = from_slot;
                inventory.slots[to_index] = to_slot;
                self.insert_inventory(from_id.to_owned(), inventory);
                return Ok(moved as i64);
            }
            // 未指定目标槽：自动落位（先按物品整体移除，再放回到放得下的槽位）。
            let requested = count.unwrap_or(i64::from(source.count)).max(1) as u32;
            let removed = inventory.remove(&source.item_id, requested);
            let leftover = inventory.add(&source.item_id, removed, max_stack);
            self.insert_inventory(from_id.to_owned(), inventory);
            return Ok(removed as i64 - leftover as i64);
        }
        let mut from = self.clone_inventory(from_id)?;
        let mut to = self.clone_inventory(to_id)?;
        check_inventory_slot(&from, from_index, from_id)?;
        let source = from.slots[from_index].clone();
        if source.item_id.is_empty() {
            return Ok(0);
        }
        let max_stack = self.item_stack_limit(&source.item_id);
        if let Some(to_index) = to_index {
            check_inventory_slot(&to, to_index, to_id)?;
            let (from_slot, to_slot, moved) =
                slot_merge_result(&source, &to.slots[to_index], max_stack, count);
            from.slots[from_index] = from_slot;
            to.slots[to_index] = to_slot;
            self.insert_inventory(from_id.to_owned(), from);
            self.insert_inventory(to_id.to_owned(), to);
            return Ok(moved as i64);
        }
        // 跨背包自动落位：目标放不下的部分退回源背包（语义与历史一致）。
        let requested = count.unwrap_or(i64::from(source.count)).max(1) as u32;
        let removed = from.remove(&source.item_id, requested);
        let leftover = to.add(&source.item_id, removed, max_stack);
        if leftover > 0 {
            let _ = from.add(&source.item_id, leftover, max_stack);
        }
        self.insert_inventory(from_id.to_owned(), from);
        self.insert_inventory(to_id.to_owned(), to);
        Ok(removed as i64 - leftover as i64)
    }
}

fn inventory_index(index: i64) -> Result<usize, String> {
    if index <= 0 {
        Err("inventory slot index must be positive".to_owned())
    } else {
        Ok((index - 1) as usize)
    }
}

/// P2-①：槽位越界检查（同背包与跨背包共用同一措辞的报错）。
fn check_inventory_slot(inventory: &Inventory, index: usize, id: &str) -> Result<(), String> {
    if index >= inventory.capacity {
        Err(format!("inventory {id} slot out of bounds"))
    } else {
        Ok(())
    }
}

/// P2-①：槽位 → 槽位合并搬运的唯一口径（同背包 / 跨背包共用）。
///
/// 规则不变：同名物品向目标堆叠至上限；目标为空槽则整段放入；目标是**别的**物品则一件不动。
/// 返回（源槽新值, 目标槽新值, 实际搬动件数）。
fn slot_merge_result(
    source: &ItemStack,
    target: &ItemStack,
    max_stack: u32,
    count: Option<i64>,
) -> (ItemStack, ItemStack, u32) {
    let requested = count
        .unwrap_or(i64::from(source.count))
        .min(i64::from(source.count))
        .max(0) as u32;
    let moved = if target.item_id == source.item_id {
        max_stack.saturating_sub(target.count).min(requested)
    } else if target.item_id.is_empty() {
        requested
    } else {
        0
    };
    if moved == 0 {
        return (source.clone(), target.clone(), 0);
    }
    let mut from_slot = source.clone();
    from_slot.count -= moved;
    if from_slot.count == 0 {
        from_slot = ItemStack::default();
    }
    let to_slot = if target.item_id == source.item_id {
        let mut merged = target.clone();
        merged.count += moved;
        merged
    } else {
        ItemStack::new(source.item_id.clone(), moved)
    };
    (from_slot, to_slot, moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> InventoryState {
        let mut state = InventoryState::default();
        state.define(ItemDefinition {
            id: "seed".to_owned(),
            name: "Seed".to_owned(),
            description: String::new(),
            stack: 5,
            buy_price: 3,
            sell_price: 1,
            texture: String::new(),
            kind: String::new(),
        });
        state.new_inventory("bag", 4);
        state.new_inventory("box", 2);
        state
    }

    #[test]
    fn stack_limit_defaults_to_99_and_follows_definitions() {
        let state = state();
        assert_eq!(state.item_stack_limit("seed"), 5);
        assert_eq!(state.item_stack_limit("unknown"), 99);
        assert_eq!(state.item_def("seed").map(|def| def.buy_price), Some(3));
        assert!(state.item_def("nope").is_none());
        assert_eq!(state.item_def_count(), 1);
    }

    #[test]
    fn swap_works_inside_and_across_inventories() {
        let mut state = state();
        state
            .inventory_mut("bag")
            .unwrap()
            .set(0, ItemStack::new("seed", 2))
            .unwrap();
        state
            .inventory_mut("bag")
            .unwrap()
            .set(1, ItemStack::new("wood", 1))
            .unwrap();
        state.swap("bag", 1, "bag", 2).unwrap();
        let bag = state.inventory("bag").unwrap();
        assert_eq!(bag.get(0).unwrap().item_id, "wood");
        assert_eq!(bag.get(1).unwrap().item_id, "seed");

        state.swap("bag", 1, "box", 1).unwrap();
        assert_eq!(
            state.inventory("box").unwrap().get(0).unwrap().item_id,
            "wood"
        );
    }

    #[test]
    fn move_merges_up_to_the_stack_limit_and_keeps_the_rest_at_source() {
        let mut state = state();
        state
            .inventory_mut("bag")
            .unwrap()
            .set(0, ItemStack::new("seed", 4))
            .unwrap();
        state
            .inventory_mut("bag")
            .unwrap()
            .set(1, ItemStack::new("seed", 4))
            .unwrap();
        // 上限 5：目标已有 4，只剩 1 个位置。
        assert_eq!(
            state.move_between("bag", 1, "bag", Some(2), None).unwrap(),
            1
        );
        let bag = state.inventory("bag").unwrap();
        assert_eq!(bag.get(0).unwrap().count, 3);
        assert_eq!(bag.get(1).unwrap().count, 5);
    }

    #[test]
    fn move_auto_places_when_no_target_slot_is_given() {
        let mut state = state();
        state
            .inventory_mut("bag")
            .unwrap()
            .set(0, ItemStack::new("seed", 3))
            .unwrap();
        assert_eq!(state.move_between("bag", 1, "box", None, None).unwrap(), 3);
        assert_eq!(state.inventory("box").unwrap().count("seed"), 3);
        assert_eq!(state.inventory("bag").unwrap().count("seed"), 0);
    }

    #[test]
    fn out_of_bounds_and_missing_inventories_report_the_same_wording() {
        let mut state = state();
        assert_eq!(
            state.swap("bag", 0, "bag", 0).unwrap_err(),
            "inventory slot index must be positive"
        );
        assert_eq!(
            state.swap("bag", 5, "bag", 1).unwrap_err(),
            "inventory bag slot out of bounds"
        );
        assert_eq!(
            state.swap("nope", 1, "bag", 1).unwrap_err(),
            "inventory nope not found"
        );
        assert_eq!(
            state
                .move_between("bag", 1, "nope", Some(1), None)
                .unwrap_err(),
            "inventory nope not found"
        );
    }

    #[test]
    fn scene_transition_clears_only_shop_catalogs() {
        let mut state = state();
        state.set_shop_catalog("village", serde_json::json!({ "seed": { "buy": 3 } }));
        state
            .inventory_mut("bag")
            .unwrap()
            .set(0, ItemStack::new("seed", 1))
            .unwrap();
        state.clear_for_scene_transition();
        assert_eq!(state.shop_count(), 0, "商店目录随场景丢弃");
        assert_eq!(state.inventory("bag").unwrap().count("seed"), 1, "背包存活");
        assert_eq!(state.item_def_count(), 1, "物品定义存活");
    }
}
