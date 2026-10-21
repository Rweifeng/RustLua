//! `item` / `inv` / `container` / `shop` 四张全局表的绑定（`docs/62` P1 ⑤-6：
//! 从 VM 宿主下沉到物品域）。
//!
//! ## 为什么在这里
//!
//! 物品定义、背包内容与商店目录的类型本来就在本 crate（[`crate::gameplay`]），
//! 数据保管方（[`crate::inventory_state::InventoryState`]）也在本 crate 了；剩下这一层
//! 只做"Lua 参数 → 领域规则"的翻译：读表、校验参数、把 [`InventoryState`] 的错误包成
//! `api_error`。槽位互换/合并这些规则住在状态单元里（见该模块头注释）。
//!
//! ## Lua 面
//!
//! 与原 `script/api/inventory.rs` **逐字一致**：`item.define/def`、`inv.create/hotbar/`
//! `capacity/exists/add/remove/count/get/slot/set/swap/move/equip/equip_get/equip_list`、
//! `container.open/get/close/move`、`shop.set_catalog/price`，共 **24 个**函数；
//! 报错文本、缺省值（容量 24/hotbar 9/容器 12、堆叠上限 99）与返回值都不变
//! （`api_surface_snapshot` 零 diff）。

use mlua::Result as LuaResult;
use mlua::{Lua, Table, Value};
use rustlua_lua_api::Handle;
use rustlua_lua_api::api_error;
use rustlua_lua_api::values::lua_value_to_json;

use crate::gameplay::{ItemDefinition, ItemStack};
use crate::inventory_state::InventoryState;

/// 装 `item` / `inv` / `container` / `shop` 四张全局表。
pub fn install(lua: &Lua, state: &Handle<InventoryState>) -> LuaResult<()> {
    install_item(lua, state)?;
    install_inv(lua, state)?;
    install_container(lua, state)?;
    install_shop(lua, state)?;
    Ok(())
}

fn install_item(lua: &Lua, state: &Handle<InventoryState>) -> LuaResult<()> {
    let item = lua.create_table()?;
    let s = state.clone();
    item.set(
        "define",
        lua.create_function(move |_, (id, definition): (String, Table)| {
            if id.is_empty() {
                return Err(api_error("item.define id must not be empty"));
            }
            let name = definition
                .get::<Option<String>>("name")?
                .unwrap_or_else(|| id.clone());
            let description = definition
                .get::<Option<String>>("description")?
                .unwrap_or_default();
            let kind = definition
                .get::<Option<String>>("kind")?
                .unwrap_or_default();
            let texture = definition
                .get::<Option<String>>("texture")?
                .unwrap_or_default();
            let stack = definition.get::<Option<i64>>("stack")?.unwrap_or(99).max(1) as u32;
            let buy_price = definition.get::<Option<i64>>("buy")?.unwrap_or(0);
            let sell_price = definition.get::<Option<i64>>("sell")?.unwrap_or(0);
            s.borrow_mut()?.define(ItemDefinition {
                id,
                name,
                description,
                stack,
                buy_price,
                sell_price,
                texture,
                kind,
            });
            Ok(true)
        })?,
    )?;
    let s = state.clone();
    item.set(
        "def",
        lua.create_function(move |lua, id: String| {
            let state = s.borrow()?;
            let Some(definition) = state.item_def(&id) else {
                return Ok(Value::Nil);
            };
            let result = lua.create_table()?;
            result.set("id", definition.id.clone())?;
            result.set("name", definition.name.clone())?;
            result.set("description", definition.description.clone())?;
            result.set("stack", definition.stack as i64)?;
            result.set("buy", definition.buy_price)?;
            result.set("sell", definition.sell_price)?;
            result.set("texture", definition.texture.clone())?;
            result.set("kind", definition.kind.clone())?;
            Ok(Value::Table(result))
        })?,
    )?;
    lua.globals().set("item", item)
}

fn install_inv(lua: &Lua, state: &Handle<InventoryState>) -> LuaResult<()> {
    let inv = lua.create_table()?;
    let s = state.clone();
    inv.set(
        "create",
        lua.create_function(move |_, (id, capacity): (String, Option<i64>)| {
            if id.is_empty() {
                return Err(api_error("inv.create id must not be empty"));
            }
            let capacity = capacity.unwrap_or(24).max(1) as usize;
            s.borrow_mut()?.new_inventory(id.clone(), capacity);
            Ok(id)
        })?,
    )?;
    let s = state.clone();
    inv.set(
        "hotbar",
        lua.create_function(move |_, (id, size): (String, Option<i64>)| {
            if id.is_empty() {
                return Err(api_error("inv.hotbar id must not be empty"));
            }
            let capacity = size.unwrap_or(9).max(1) as usize;
            s.borrow_mut()?.new_inventory(id.clone(), capacity);
            Ok(id)
        })?,
    )?;
    let s = state.clone();
    inv.set(
        "capacity",
        lua.create_function(move |_, id: String| {
            let state = s.borrow()?;
            state
                .inventory(&id)
                .map(|inventory| inventory.capacity as i64)
                .ok_or_else(|| api_error(format!("inventory {id} not found")))
        })?,
    )?;
    let s = state.clone();
    inv.set(
        "exists",
        lua.create_function(move |_, id: String| Ok(s.borrow()?.contains_inventory(&id)))?,
    )?;
    let s = state.clone();
    inv.set(
        "add",
        lua.create_function(move |_, (id, item_id, count): (String, String, i64)| {
            if item_id.is_empty() {
                return Err(api_error("inv.add item id must not be empty"));
            }
            if count <= 0 {
                return Err(api_error("inv.add count must be positive"));
            }
            let mut state = s.borrow_mut()?;
            if !state.contains_inventory(&id) {
                state.new_inventory(id.clone(), 24);
            }
            let max_stack = state.item_stack_limit(&item_id);
            let inventory = state.inventory_mut(&id).expect("inventory just inserted");
            Ok(inventory.add(&item_id, count as u32, max_stack) as i64)
        })?,
    )?;
    let s = state.clone();
    inv.set(
        "remove",
        lua.create_function(move |_, (id, item_id, count): (String, String, i64)| {
            if count <= 0 {
                return Err(api_error("inv.remove count must be positive"));
            }
            let mut state = s.borrow_mut()?;
            let Some(inventory) = state.inventory_mut(&id) else {
                return Ok(0);
            };
            Ok(inventory.remove(&item_id, count as u32) as i64)
        })?,
    )?;
    let s = state.clone();
    inv.set(
        "count",
        lua.create_function(move |_, (id, item_id): (String, String)| {
            let state = s.borrow()?;
            Ok(state
                .inventory(&id)
                .map_or(0, |inventory| inventory.count(&item_id)) as i64)
        })?,
    )?;
    let s = state.clone();
    inv.set(
        "get",
        lua.create_function(move |lua, id: String| {
            let state = s.borrow()?;
            let Some(inventory) = state.inventory(&id) else {
                return Err(api_error(format!("inventory {id} not found")));
            };
            let result = lua.create_table()?;
            result.set("id", id.clone())?;
            result.set("capacity", inventory.capacity as i64)?;
            let slots = lua.create_table()?;
            let mut occupied = 0;
            for (index, stack) in inventory.slots.iter().enumerate() {
                if stack.item_id.is_empty() {
                    continue;
                }
                let slot = lua.create_table()?;
                slot.set("item", stack.item_id.clone())?;
                slot.set("count", stack.count as i64)?;
                slots.set(index + 1, slot)?;
                occupied += 1;
            }
            result.set("slots", slots)?;
            result.set("occupied", occupied)?;
            Ok(result)
        })?,
    )?;
    let s = state.clone();
    inv.set(
        "slot",
        lua.create_function(move |lua, (id, index): (String, i64)| {
            if index <= 0 {
                return Err(api_error("inv.slot index must be positive"));
            }
            let state = s.borrow()?;
            let Some(inventory) = state.inventory(&id) else {
                return Err(api_error(format!("inventory {id} not found")));
            };
            let Some(stack) = inventory.get((index - 1) as usize) else {
                return Err(api_error(format!(
                    "inventory {id} slot {index} out of bounds"
                )));
            };
            if stack.item_id.is_empty() {
                return Ok(Value::Nil);
            }
            let slot = lua.create_table()?;
            slot.set("item", stack.item_id.clone())?;
            slot.set("count", stack.count as i64)?;
            Ok(Value::Table(slot))
        })?,
    )?;
    let s = state.clone();
    inv.set(
        "set",
        lua.create_function(
            move |_, (id, index, item_id, count): (String, i64, Option<String>, Option<i64>)| {
                if index <= 0 {
                    return Err(api_error("inv.set index must be positive"));
                }
                let stack = match (item_id, count) {
                    (Some(item_id), Some(count)) if count > 0 => {
                        ItemStack::new(item_id, count as u32)
                    }
                    _ => ItemStack::default(),
                };
                let mut state = s.borrow_mut()?;
                let Some(inventory) = state.inventory_mut(&id) else {
                    return Err(api_error(format!("inventory {id} not found")));
                };
                inventory
                    .set((index - 1) as usize, stack)
                    .map_err(|error| api_error(error.to_string()))?;
                Ok(())
            },
        )?,
    )?;
    let s = state.clone();
    inv.set(
        "swap",
        lua.create_function(
            move |_, (from_id, from_index, to_id, to_index): (String, i64, String, i64)| {
                s.borrow_mut()?
                    .swap(&from_id, from_index, &to_id, to_index)
                    .map_err(api_error)
            },
        )?,
    )?;
    let s = state.clone();
    inv.set(
        "move",
        lua.create_function(
            move |_,
                  (from_id, from_index, to_id, to_index, count): (
                String,
                i64,
                String,
                Option<i64>,
                Option<i64>,
            )| {
                s.borrow_mut()?
                    .move_between(&from_id, from_index, &to_id, to_index, count)
                    .map_err(api_error)
            },
        )?,
    )?;
    // Equipment skeleton (M16-07): named slots holding an item id. No runtime
    // effect yet — stat/gear systems consume this in M18-00.
    let s = state.clone();
    inv.set(
        "equip",
        lua.create_function(
            move |_, (id, slot, item_id): (String, String, Option<String>)| {
                if id.is_empty() || slot.is_empty() {
                    return Err(api_error(
                        "inv.equip needs an inventory id and a non-empty slot",
                    ));
                }
                let mut state = s.borrow_mut()?;
                let Some(inventory) = state.inventory_mut(&id) else {
                    return Err(api_error(format!("inventory {id} not found")));
                };
                inventory.equip(&slot, item_id.as_deref().unwrap_or(""));
                Ok(())
            },
        )?,
    )?;
    let s = state.clone();
    inv.set(
        "equip_get",
        lua.create_function(move |_, (id, slot): (String, String)| {
            let state = s.borrow()?;
            let Some(inventory) = state.inventory(&id) else {
                return Err(api_error(format!("inventory {id} not found")));
            };
            Ok(inventory.equip_get(&slot).map(str::to_owned))
        })?,
    )?;
    let s = state.clone();
    inv.set(
        "equip_list",
        lua.create_function(move |lua, id: String| {
            let state = s.borrow()?;
            let Some(inventory) = state.inventory(&id) else {
                return Err(api_error(format!("inventory {id} not found")));
            };
            let result = lua.create_table()?;
            for (slot, item) in &inventory.equipment {
                result.set(slot.clone(), item.clone())?;
            }
            Ok(result)
        })?,
    )?;
    lua.globals().set("inv", inv)
}

fn install_container(lua: &Lua, state: &Handle<InventoryState>) -> LuaResult<()> {
    let container = lua.create_table()?;
    let s = state.clone();
    container.set(
        "open",
        lua.create_function(move |_, (entity_id, capacity): (i64, Option<i64>)| {
            let key = format!("container:{entity_id}");
            let capacity = capacity.unwrap_or(12).max(1) as usize;
            s.borrow_mut()?.new_inventory(key.clone(), capacity);
            Ok(key)
        })?,
    )?;
    let s = state.clone();
    container.set(
        "get",
        lua.create_function(move |_, entity_id: i64| {
            let key = format!("container:{entity_id}");
            let exists = s.borrow()?.contains_inventory(&key);
            if exists { Ok(Some(key)) } else { Ok(None) }
        })?,
    )?;
    let s = state.clone();
    container.set(
        "close",
        lua.create_function(move |_, entity_id: i64| {
            let key = format!("container:{entity_id}");
            Ok(s.borrow_mut()?.remove_inventory(&key))
        })?,
    )?;
    let s = state.clone();
    container.set(
        "move",
        lua.create_function(
            move |_, (entity_id, from_index, to_id, to_index): (i64, i64, String, i64)| {
                let from_id = format!("container:{entity_id}");
                s.borrow_mut()?
                    .move_between(&from_id, from_index, &to_id, Some(to_index), None)
                    .map_err(api_error)
            },
        )?,
    )?;
    lua.globals().set("container", container)
}

fn install_shop(lua: &Lua, state: &Handle<InventoryState>) -> LuaResult<()> {
    // `shop` catalogs and price queries (M16-10). Catalogs are data-driven from
    // data/shop.json; buy/sell transactions and the money balance stay in Lua
    // (game logic), with `shop.price` as the engine's price/catalog service.
    let shop = lua.create_table()?;
    let s = state.clone();
    shop.set(
        "set_catalog",
        lua.create_function(move |_, (name, spec): (String, Value)| {
            let json = lua_value_to_json(spec)?;
            s.borrow_mut()?.set_shop_catalog(name, json);
            Ok(true)
        })?,
    )?;
    let s = state.clone();
    shop.set(
        "price",
        lua.create_function(move |lua, (name, item_id): (String, String)| {
            let state = s.borrow()?;
            let Some(catalog) = state.shop_catalog(&name) else {
                return Ok(Value::Nil);
            };
            let Some(item) = catalog.get(item_id.as_str()) else {
                return Ok(Value::Nil);
            };
            let buy = item
                .get("buy")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            let sell = item
                .get("sell")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            let result = lua.create_table()?;
            result.set("buy", buy)?;
            result.set("sell", sell)?;
            Ok(Value::Table(result))
        })?,
    )?;
    lua.globals().set("shop", shop)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed() -> (Lua, Handle<InventoryState>) {
        let lua = Lua::new();
        let state = InventoryState::handle();
        install(&lua, &state).unwrap();
        (lua, state)
    }

    /// 四张表的 Lua 面就这 24 个函数：搬家之后一个都不能少
    /// （`api_surface_snapshot` 盯全量，这条盯"这一步搬全了"）。
    const NAMES: [(&str, &[&str]); 4] = [
        ("item", &["define", "def"]),
        (
            "inv",
            &[
                "create",
                "hotbar",
                "capacity",
                "exists",
                "add",
                "remove",
                "count",
                "get",
                "slot",
                "set",
                "swap",
                "move",
                "equip",
                "equip_get",
                "equip_list",
            ],
        ),
        ("container", &["open", "get", "close", "move"]),
        ("shop", &["set_catalog", "price"]),
    ];

    #[test]
    fn every_inventory_function_is_installed() {
        let (lua, _state) = installed();
        let mut total = 0;
        for (table, names) in NAMES {
            let module: Table = lua.globals().get(table).unwrap();
            for name in names {
                total += 1;
                let value: Value = module.get(*name).unwrap();
                assert!(
                    matches!(value, Value::Function(_)),
                    "`{table}.{name}` 缺失或不是函数：{value:?}"
                );
            }
        }
        assert_eq!(total, 23, "函数总数（快照门盯名字，这条盯个数）");
    }

    #[test]
    fn item_and_inventory_state_live_in_the_handle() {
        let (lua, state) = installed();
        lua.load(
            r#"
            assert(item.define("seed", { name = "Seed", stack = 5, buy = 3, sell = 1 }))
            assert(item.def("seed").name == "Seed" and item.def("seed").stack == 5)
            assert(item.def("nope") == nil)
            assert(inv.create("bag", 4) == "bag")
            assert(inv.capacity("bag") == 4 and inv.exists("bag"))
            -- `inv.add` 返回**放不下的剩余件数**（原语义），不是放入件数。
            assert(inv.add("bag", "seed", 3) == 0)
            assert(inv.count("bag", "seed") == 3)
            assert(inv.slot("bag", 1).item == "seed")
            local got = inv.get("bag")
            assert(got.id == "bag" and got.occupied == 1 and got.slots[1].count == 3)
            "#,
        )
        .exec()
        .unwrap();
        // 状态确实在能力单元里（宿主/存档面板读同一份）。
        let state = state.borrow().unwrap();
        assert_eq!(state.item_def("seed").map(|def| def.stack), Some(5));
        assert_eq!(state.inventory("bag").map(|inv| inv.capacity), Some(4));
        assert_eq!(state.inventory("bag").unwrap().count("seed"), 3);
    }

    #[test]
    fn swap_move_and_container_go_through_the_handle() {
        let (lua, state) = installed();
        lua.load(
            r#"
            inv.create("bag", 4)
            inv.create("box", 2)
            inv.set("bag", 1, "seed", 3)
            assert(inv.swap("bag", 1, "box", 1))
            assert(inv.count("box", "seed") == 3 and inv.count("bag", "seed") == 0)
            assert(inv.move("box", 1, "bag", 2) == 3)
            assert(inv.count("bag", "seed") == 3)
            local key = container.open(7, 2)
            assert(key == "container:7" and container.get(7) == key)
            assert(container.close(7) == true and container.get(7) == nil)
            "#,
        )
        .exec()
        .unwrap();
        let state = state.borrow().unwrap();
        assert_eq!(state.inventory("bag").unwrap().count("seed"), 3);
        assert!(state.inventory("box").is_some());
    }

    #[test]
    fn shop_catalogs_round_trip_and_scene_transition_drops_them() {
        let (lua, state) = installed();
        lua.load(
            r#"
            shop.set_catalog("village", { seed = { buy = 3, sell = 1 } })
            local price = shop.price("village", "seed")
            assert(price.buy == 3 and price.sell == 1)
            assert(shop.price("village", "nope") == nil)
            assert(shop.price("nope", "seed") == nil)
            "#,
        )
        .exec()
        .unwrap();
        assert_eq!(state.borrow().unwrap().shop_count(), 1);
        state.borrow_mut().unwrap().clear_for_scene_transition();
        assert_eq!(
            state.borrow().unwrap().shop_count(),
            0,
            "商店目录随场景丢弃"
        );
    }

    #[test]
    fn invalid_arguments_are_rejected_with_the_original_wording() {
        let (lua, _state) = installed();
        // 越界那条要先有一个真实背包，否则先撞"not found"（措辞也是既有语义的一部分）。
        lua.load("inv.create('bag', 4)").exec().unwrap();
        for (source, needle) in [
            ("item.define('', {})", "item.define id must not be empty"),
            ("inv.create('', 4)", "inv.create id must not be empty"),
            ("inv.add('bag', '', 1)", "inv.add item id must not be empty"),
            (
                "inv.add('bag', 'seed', 0)",
                "inv.add count must be positive",
            ),
            (
                "inv.remove('bag', 'seed', 0)",
                "inv.remove count must be positive",
            ),
            ("inv.slot('bag', 0)", "inv.slot index must be positive"),
            (
                "inv.set('bag', 0, 'seed', 1)",
                "inv.set index must be positive",
            ),
            ("inv.equip('', 'head', 'hat')", "needs an inventory id"),
            (
                "inv.swap('bag', 0, 'bag', 1)",
                "slot index must be positive",
            ),
            (
                "inv.swap('bag', 9, 'bag', 1)",
                "inventory bag slot out of bounds",
            ),
            ("inv.count('nope', 'seed')", ""),
        ] {
            let outcome = lua.load(source).exec();
            if needle.is_empty() {
                // `inv.count` 对不存在的背包返回 0 而不是报错（既有语义）。
                outcome.expect("缺失的背包不报错");
                let count: i64 = lua.load("return inv.count('nope', 'seed')").eval().unwrap();
                assert_eq!(count, 0);
                continue;
            }
            let error = outcome.expect_err(source).to_string();
            assert!(
                error.contains(needle),
                "`{source}` 应报 `{needle}`：{error}"
            );
        }
    }
}
