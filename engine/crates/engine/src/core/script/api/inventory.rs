//! 物品与背包：`item`（定义）、`inv`（格子与增删）、`container`、`shop`。

// WP11-USE:BEGIN
use crate::core::gameplay::{Inventory, ItemDefinition, ItemStack};
use crate::core::script::LuaStateBorrow;
use crate::core::script::error::api_error;
use crate::core::script::values::lua_value_to_json;
use crate::core::script::{ScriptState, move_inventories, swap_inventories};
use mlua::Result as LuaResult;
use mlua::{Lua, Table, Value};
use std::cell::RefCell;
use std::rc::Rc;
// WP11-USE:END

pub(super) fn register(lua: &Lua, state: Rc<RefCell<ScriptState>>) -> LuaResult<()> {
    let item = lua.create_table()?;
    let s = Rc::clone(&state);
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
            s.borrow_mut().inventory.item_defs.insert(
                id.clone(),
                ItemDefinition {
                    id,
                    name,
                    description,
                    stack,
                    buy_price,
                    sell_price,
                    texture,
                    kind,
                },
            );
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    item.set(
        "def",
        lua.create_function(move |lua, id: String| {
            let state = s.lua_borrow()?;
            let Some(definition) = state.inventory.item_defs.get(&id) else {
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
    lua.globals().set("item", item)?;

    let inv = lua.create_table()?;
    let s = Rc::clone(&state);
    inv.set(
        "create",
        lua.create_function(move |_, (id, capacity): (String, Option<i64>)| {
            if id.is_empty() {
                return Err(api_error("inv.create id must not be empty"));
            }
            let capacity = capacity.unwrap_or(24).max(1) as usize;
            s.borrow_mut()
                .inventory
                .inventories
                .insert(id.clone(), Inventory::new(capacity));
            Ok(id)
        })?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "hotbar",
        lua.create_function(move |_, (id, size): (String, Option<i64>)| {
            if id.is_empty() {
                return Err(api_error("inv.hotbar id must not be empty"));
            }
            let capacity = size.unwrap_or(9).max(1) as usize;
            s.borrow_mut()
                .inventory
                .inventories
                .insert(id.clone(), Inventory::new(capacity));
            Ok(id)
        })?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "capacity",
        lua.create_function(move |_, id: String| {
            let state = s.lua_borrow()?;
            state
                .inventory
                .inventories
                .get(&id)
                .map(|inventory| inventory.capacity as i64)
                .ok_or_else(|| api_error(format!("inventory {id} not found")))
        })?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "exists",
        lua.create_function(move |_, id: String| {
            Ok(s.borrow().inventory.inventories.contains_key(&id))
        })?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "add",
        lua.create_function(move |_, (id, item_id, count): (String, String, i64)| {
            if item_id.is_empty() {
                return Err(api_error("inv.add item id must not be empty"));
            }
            if count <= 0 {
                return Err(api_error("inv.add count must be positive"));
            }
            let mut state = s.lua_borrow_mut()?;
            if !state.inventory.inventories.contains_key(&id) {
                state
                    .inventory
                    .inventories
                    .insert(id.clone(), Inventory::new(24));
            }
            let max_stack = state
                .inventory
                .item_defs
                .get(&item_id)
                .map_or(99, |definition| definition.stack);
            let inventory = state
                .inventory
                .inventories
                .get_mut(&id)
                .expect("inventory just inserted");
            Ok(inventory.add(&item_id, count as u32, max_stack) as i64)
        })?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "remove",
        lua.create_function(move |_, (id, item_id, count): (String, String, i64)| {
            if count <= 0 {
                return Err(api_error("inv.remove count must be positive"));
            }
            let mut state = s.lua_borrow_mut()?;
            let Some(inventory) = state.inventory.inventories.get_mut(&id) else {
                return Ok(0);
            };
            Ok(inventory.remove(&item_id, count as u32) as i64)
        })?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "count",
        lua.create_function(move |_, (id, item_id): (String, String)| {
            let state = s.lua_borrow()?;
            Ok(state
                .inventory
                .inventories
                .get(&id)
                .map_or(0, |inventory| inventory.count(&item_id)) as i64)
        })?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "get",
        lua.create_function(move |lua, id: String| {
            let state = s.lua_borrow()?;
            let Some(inventory) = state.inventory.inventories.get(&id) else {
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
    let s = Rc::clone(&state);
    inv.set(
        "slot",
        lua.create_function(move |lua, (id, index): (String, i64)| {
            if index <= 0 {
                return Err(api_error("inv.slot index must be positive"));
            }
            let state = s.lua_borrow()?;
            let Some(inventory) = state.inventory.inventories.get(&id) else {
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
    let s = Rc::clone(&state);
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
                let mut state = s.lua_borrow_mut()?;
                let Some(inventory) = state.inventory.inventories.get_mut(&id) else {
                    return Err(api_error(format!("inventory {id} not found")));
                };
                inventory
                    .set((index - 1) as usize, stack)
                    .map_err(|error| api_error(error.to_string()))?;
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "swap",
        lua.create_function(
            move |_, (from_id, from_index, to_id, to_index): (String, i64, String, i64)| {
                swap_inventories(&mut s.borrow_mut(), &from_id, from_index, &to_id, to_index)
            },
        )?,
    )?;
    let s = Rc::clone(&state);
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
                move_inventories(
                    &mut s.borrow_mut(),
                    &from_id,
                    from_index,
                    &to_id,
                    to_index,
                    count,
                )
            },
        )?,
    )?;
    // Equipment skeleton (M16-07): named slots holding an item id. No runtime
    // effect yet — stat/gear systems consume this in M18-00.
    let s = Rc::clone(&state);
    inv.set(
        "equip",
        lua.create_function(
            move |_, (id, slot, item_id): (String, String, Option<String>)| {
                if id.is_empty() || slot.is_empty() {
                    return Err(api_error(
                        "inv.equip needs an inventory id and a non-empty slot",
                    ));
                }
                let mut state = s.lua_borrow_mut()?;
                let Some(inventory) = state.inventory.inventories.get_mut(&id) else {
                    return Err(api_error(format!("inventory {id} not found")));
                };
                inventory.equip(&slot, item_id.as_deref().unwrap_or(""));
                Ok(())
            },
        )?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "equip_get",
        lua.create_function(move |_, (id, slot): (String, String)| {
            let state = s.lua_borrow()?;
            let Some(inventory) = state.inventory.inventories.get(&id) else {
                return Err(api_error(format!("inventory {id} not found")));
            };
            Ok(inventory.equip_get(&slot).map(str::to_owned))
        })?,
    )?;
    let s = Rc::clone(&state);
    inv.set(
        "equip_list",
        lua.create_function(move |lua, id: String| {
            let state = s.lua_borrow()?;
            let Some(inventory) = state.inventory.inventories.get(&id) else {
                return Err(api_error(format!("inventory {id} not found")));
            };
            let result = lua.create_table()?;
            for (slot, item) in &inventory.equipment {
                result.set(slot.clone(), item.clone())?;
            }
            Ok(result)
        })?,
    )?;
    lua.globals().set("inv", inv)?;

    let container = lua.create_table()?;
    let s = Rc::clone(&state);
    container.set(
        "open",
        lua.create_function(move |_, (entity_id, capacity): (i64, Option<i64>)| {
            let key = format!("container:{entity_id}");
            let capacity = capacity.unwrap_or(12).max(1) as usize;
            s.borrow_mut()
                .inventory
                .inventories
                .insert(key.clone(), Inventory::new(capacity));
            Ok(key)
        })?,
    )?;
    let s = Rc::clone(&state);
    container.set(
        "get",
        lua.create_function(move |_, entity_id: i64| {
            let key = format!("container:{entity_id}");
            let exists = s.borrow().inventory.inventories.contains_key(&key);
            if exists { Ok(Some(key)) } else { Ok(None) }
        })?,
    )?;
    let s = Rc::clone(&state);
    container.set(
        "close",
        lua.create_function(move |_, entity_id: i64| {
            let key = format!("container:{entity_id}");
            Ok(s.borrow_mut().inventory.inventories.remove(&key).is_some())
        })?,
    )?;
    let s = Rc::clone(&state);
    container.set(
        "move",
        lua.create_function(
            move |_, (entity_id, from_index, to_id, to_index): (i64, i64, String, i64)| {
                let from_id = format!("container:{entity_id}");
                move_inventories(
                    &mut s.borrow_mut(),
                    &from_id,
                    from_index,
                    &to_id,
                    Some(to_index),
                    None,
                )
            },
        )?,
    )?;
    lua.globals().set("container", container)?;

    // `shop` catalogs and price queries (M16-10). Catalogs are data-driven from
    // data/shop.json; buy/sell transactions and the money balance stay in Lua
    // (game logic), with `shop.price` as the engine's price/catalog service.

    let shop = lua.create_table()?;
    let s = Rc::clone(&state);
    shop.set(
        "set_catalog",
        lua.create_function(move |_, (name, spec): (String, Value)| {
            let json = lua_value_to_json(spec)?;
            s.borrow_mut().inventory.shops.insert(name, json);
            Ok(true)
        })?,
    )?;
    let s = Rc::clone(&state);
    shop.set(
        "price",
        lua.create_function(move |lua, (name, item_id): (String, String)| {
            let state = s.lua_borrow()?;
            let Some(catalog) = state.inventory.shops.get(&name) else {
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
    lua.globals().set("shop", shop)?;

    Ok(())
}
