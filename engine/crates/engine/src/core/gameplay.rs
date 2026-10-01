//! Small gameplay data types shared by the Lua farm API: a game clock,
//! item definitions, inventories, and item stacks.

use std::collections::BTreeMap;

/// Minutes in one in-game day. `sleep_to_morning` and `now()` use this as the
/// day boundary.
pub const MINUTES_PER_DAY: u32 = 24 * 60;
/// The default wake-up time used by a fresh game clock (06:00).
pub const DEFAULT_WAKE_MINUTE: u32 = 6 * 60;
/// Days per season; the farm calendar uses a 28-day season × 4 seasons year.
pub const DAYS_PER_SEASON: u32 = 28;
/// Seasons per year.
pub const SEASONS_PER_YEAR: u32 = 4;
/// Total days per year (`28 × 4`).
pub const DAYS_PER_YEAR: u32 = DAYS_PER_SEASON * SEASONS_PER_YEAR;

/// Independent game clock decoupled from wall-clock `time.delta()`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GameClock {
    /// 1-based in-game day.
    pub day: u32,
    /// Minute of day in `0..MINUTES_PER_DAY`.
    pub minute: u32,
    /// Game minutes advanced per real second. `0.0` disables auto-advance.
    pub rate: f32,
    pub paused: bool,
}

impl Default for GameClock {
    fn default() -> Self {
        Self {
            day: 1,
            minute: DEFAULT_WAKE_MINUTE,
            rate: 0.0,
            paused: false,
        }
    }
}

impl GameClock {
    /// Advances the clock by `delta_seconds` using the configured rate.
    pub fn step(&mut self, delta_seconds: f32) {
        if self.paused || self.rate <= 0.0 || !delta_seconds.is_finite() {
            return;
        }
        let minutes = (self.rate * delta_seconds).max(0.0);
        self.add_minutes(minutes);
    }

    /// Advances the clock by fractional minutes, rolling over into new days.
    pub fn add_minutes(&mut self, minutes: f32) {
        if !minutes.is_finite() {
            return;
        }
        let total = self.minute as f32 + minutes;
        if total < 0.0 {
            self.minute = 0;
            return;
        }
        let total_minutes = total as u32;
        self.day += total_minutes / MINUTES_PER_DAY;
        self.minute = total_minutes % MINUTES_PER_DAY;
    }

    /// Sets an absolute day and minute-of-day.
    pub fn set(&mut self, day: u32, minute: u32) {
        self.day = day.max(1);
        self.minute = minute.min(MINUTES_PER_DAY - 1);
    }

    /// Sleeps until the next morning and returns the new day.
    pub fn sleep_to_morning(&mut self) -> u32 {
        self.day += 1;
        self.minute = DEFAULT_WAKE_MINUTE;
        self.day
    }

    /// 1-based season (`1..=4`) derived from the absolute day.
    #[must_use]
    pub fn season(&self) -> u32 {
        ((self.day - 1) / DAYS_PER_SEASON) % SEASONS_PER_YEAR + 1
    }

    /// 1-based year derived from the absolute day.
    #[must_use]
    pub fn year(&self) -> u32 {
        (self.day - 1) / DAYS_PER_YEAR + 1
    }

    /// 1-based day within the current season (`1..=DAYS_PER_SEASON`).
    #[must_use]
    pub fn day_of_season(&self) -> u32 {
        (self.day - 1) % DAYS_PER_SEASON + 1
    }

    /// Sets an absolute day (`>= 1`) and minute-of-day.
    pub fn set_absolute(&mut self, day: u32, minute: u32) {
        self.day = day.max(1);
        self.minute = minute.min(MINUTES_PER_DAY - 1);
    }
}

/// Errors produced by inventory operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum InventoryError {
    #[error("inventory slot {index} is out of bounds (capacity {capacity})")]
    IndexOutOfBounds { index: usize, capacity: usize },
}

/// One item quantity in an inventory slot.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ItemStack {
    pub item_id: String,
    pub count: u32,
}

impl ItemStack {
    pub fn new(item_id: impl Into<String>, count: u32) -> Self {
        Self {
            item_id: item_id.into(),
            count,
        }
    }
}

/// A fixed-capacity list of stacks. Empty slots use an empty `item_id`.
/// `equipment` is the M16-07 skeleton: named slots holding an item id, no
/// runtime effect yet (stats land in M18-00).
#[derive(Clone, Debug)]
pub struct Inventory {
    pub slots: Vec<ItemStack>,
    pub capacity: usize,
    pub equipment: BTreeMap<String, String>,
}

impl Inventory {
    pub fn new(capacity: usize) -> Self {
        Self {
            slots: vec![ItemStack::default(); capacity],
            capacity,
            equipment: BTreeMap::new(),
        }
    }

    /// Sets an equipment slot (empty `item_id` clears it). Empty slot names are
    /// rejected by the caller; this skeleton only stores the mapping.
    pub fn equip(&mut self, slot: &str, item_id: &str) {
        if item_id.is_empty() {
            self.equipment.remove(slot);
        } else {
            self.equipment.insert(slot.to_owned(), item_id.to_owned());
        }
    }

    /// Returns the item id equipped in `slot`, or `None`.
    pub fn equip_get(&self, slot: &str) -> Option<&str> {
        self.equipment.get(slot).map(String::as_str)
    }

    fn slot_mut(&mut self, index: usize) -> Result<&mut ItemStack, InventoryError> {
        self.slots
            .get_mut(index)
            .ok_or(InventoryError::IndexOutOfBounds {
                index,
                capacity: self.capacity,
            })
    }

    /// Adds `count` of `item_id`, respecting `max_stack`, and returns leftover.
    pub fn add(&mut self, item_id: &str, count: u32, max_stack: u32) -> u32 {
        let mut remaining = count;
        let max_stack = max_stack.max(1);
        for slot in &mut self.slots {
            if remaining == 0 {
                break;
            }
            if slot.item_id == item_id {
                let room = max_stack.saturating_sub(slot.count);
                let moved = remaining.min(room);
                slot.count += moved;
                remaining -= moved;
            }
        }
        if remaining > 0 {
            for slot in &mut self.slots {
                if remaining == 0 {
                    break;
                }
                if slot.item_id.is_empty() {
                    let moved = remaining.min(max_stack);
                    *slot = ItemStack::new(item_id, moved);
                    remaining -= moved;
                }
            }
        }
        remaining
    }

    /// Removes up to `count` of `item_id` and returns the amount removed.
    pub fn remove(&mut self, item_id: &str, count: u32) -> u32 {
        let mut remaining = count;
        for slot in self.slots.iter_mut().rev() {
            if remaining == 0 {
                break;
            }
            if slot.item_id != item_id {
                continue;
            }
            let removed = remaining.min(slot.count);
            slot.count -= removed;
            remaining -= removed;
            if slot.count == 0 {
                *slot = ItemStack::default();
            }
        }
        count - remaining
    }

    pub fn count(&self, item_id: &str) -> u32 {
        self.slots
            .iter()
            .filter(|slot| slot.item_id == item_id)
            .map(|slot| slot.count)
            .sum()
    }

    pub fn get(&self, index: usize) -> Option<&ItemStack> {
        self.slots.get(index)
    }

    /// Sets a slot to `stack` (empty `item_id` clears the slot).
    pub fn set(&mut self, index: usize, stack: ItemStack) -> Result<(), InventoryError> {
        let slot = self.slot_mut(index)?;
        if stack.count == 0 || stack.item_id.is_empty() {
            *slot = ItemStack::default();
        } else {
            *slot = stack;
        }
        Ok(())
    }

    pub fn swap(&mut self, a: usize, b: usize) -> Result<(), InventoryError> {
        let (left, right) = if a <= b { (a, b) } else { (b, a) };
        self.slot_mut(left)?;
        self.slot_mut(right)?;
        self.slots.swap(left, right);
        Ok(())
    }
}

/// Static definition for an item, registered by `item.define`.
#[derive(Clone, Debug)]
pub struct ItemDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub stack: u32,
    pub buy_price: i64,
    pub sell_price: i64,
    pub texture: String,
    pub kind: String,
}

impl ItemDefinition {
    pub fn default_for(id: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            id: id.clone(),
            name: id,
            description: String::new(),
            stack: 99,
            buy_price: 0,
            sell_price: 0,
            texture: String::new(),
            kind: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_rolls_over_and_sleeps() {
        let mut clock = GameClock::default();
        assert_eq!((clock.day, clock.minute), (1, DEFAULT_WAKE_MINUTE));
        clock.rate = 60.0;
        clock.paused = false;
        clock.step(1.0);
        assert_eq!((clock.day, clock.minute), (1, 7 * 60));
        clock.minute = MINUTES_PER_DAY - 1;
        clock.add_minutes(2.0);
        assert_eq!((clock.day, clock.minute), (2, 1));
        assert_eq!(clock.sleep_to_morning(), 3);
        assert_eq!(clock.minute, DEFAULT_WAKE_MINUTE);
    }

    #[test]
    fn calendar_derives_season_year_and_day_of_season() {
        use super::{DAYS_PER_SEASON, DAYS_PER_YEAR};
        let mut clock = GameClock::default();
        assert_eq!(
            (
                clock.day,
                clock.season(),
                clock.year(),
                clock.day_of_season()
            ),
            (1, 1, 1, 1)
        );
        clock.day = DAYS_PER_SEASON;
        assert_eq!(
            (clock.season(), clock.day_of_season()),
            (1, DAYS_PER_SEASON)
        );
        clock.day = DAYS_PER_SEASON + 1;
        assert_eq!((clock.season(), clock.day_of_season()), (2, 1));
        clock.day = DAYS_PER_YEAR;
        assert_eq!((clock.season(), clock.year()), (4, 1));
        clock.day = DAYS_PER_YEAR + 1;
        assert_eq!((clock.season(), clock.year()), (1, 2));
    }

    #[test]
    fn inventory_adds_stacks_and_removes() {
        let mut inv = Inventory::new(3);
        assert_eq!(inv.add("tomato", 10, 4), 0);
        assert_eq!(inv.count("tomato"), 10);
        assert_eq!(inv.remove("tomato", 5), 5);
        assert_eq!(inv.count("tomato"), 5);
        assert_eq!(inv.remove("tomato", 9), 5);
        assert!(inv.get(0).unwrap().item_id.is_empty());
        inv.set(1, ItemStack::new("potato", 2)).unwrap();
        inv.swap(0, 1).unwrap();
        assert_eq!(inv.get(0).unwrap().item_id, "potato");
        assert!(inv.set(9, ItemStack::new("x", 1)).is_err());
    }

    #[test]
    fn equipment_skeleton_stores_and_clears_slots() {
        let mut inv = Inventory::new(3);
        inv.equip("tool", "hoe");
        inv.equip("hat", "straw_hat");
        assert_eq!(inv.equip_get("tool"), Some("hoe"));
        assert_eq!(inv.equip_get("hat"), Some("straw_hat"));
        assert_eq!(inv.equip_get("missing"), None);
        // Empty item id clears the slot.
        inv.equip("tool", "");
        assert_eq!(inv.equip_get("tool"), None);
        assert_eq!(inv.equipment.len(), 1);
    }
}
