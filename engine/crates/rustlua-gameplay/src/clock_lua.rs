//! `clock` / `calendar` / `day` 三张全局表的绑定（`docs/62` P1 ⑤-5：从 VM 宿主下沉到游戏时钟域）。
//!
//! ## 为什么在这里
//!
//! 时钟的**类型**（[`GameClock`]）与它的推进规则（`step` / `add_minutes` / 季节换算）一直在本
//! crate，但"Lua 怎么读写这台时钟"却住在 `rustlua-scripting` 的 `globals.rs` 里——于是 VM 宿主
//! 既要懂虚拟机，又要懂"一天的分钟数、季节怎么从绝对日推、睡一觉是哪一天"。这几件事现在跟着
//! 时钟回家：本模块只做"文本 → 时钟操作"的翻译，`ScriptState` 只留一个
//! [`Handle<GameClock>`](rustlua_lua_api::Handle)。
//!
//! ## 与旧实现的差别（只有借用面，Lua 面逐字未变）
//!
//! 1. 借用变窄：以前每个函数借整份 `ScriptState`（重入时报"脚本状态被重入"），现在只借时钟
//!    这一个能力单元，报错直接点名 `game_clock`。
//! 2. `day.sleep` 的**顺序**原样保留：先 `on_day_end`、再睡、再 `on_day_start`。两个钩子都在
//!    借还时钟**之外**调用——钩子里可以再调 `clock.*`，不会撞上本函数的借用。
//!
//! Lua 面的函数名、参数、返回值、报错文本**逐字未变**（`api_surface_snapshot` 零 diff）。

use mlua::{Function, Lua, Result as LuaResult};
use rustlua_lua_api::Handle;
use rustlua_lua_api::api_error;

use crate::gameplay::{
    DAYS_PER_SEASON, DAYS_PER_YEAR, GameClock, MINUTES_PER_DAY, SEASONS_PER_YEAR,
};

/// 游戏时钟的能力单元（`what` = `game_clock`，与 Lua 侧的读法一致）。
#[must_use]
pub fn handle() -> Handle<GameClock> {
    Handle::new(GameClock::default(), "game_clock")
}

/// 装 `clock` / `calendar` / `day` 三张全局表。
///
/// `clock` 与宿主共享同一份状态（[`Handle`] 是能力单元，不是拷贝）：宿主每帧
/// `LuaRuntime::step_clock` 推进的就是这里读到的那台时钟。
pub fn install(lua: &Lua, clock: &Handle<GameClock>) -> LuaResult<()> {
    install_clock(lua, clock)?;
    install_calendar(lua, clock)?;
    install_day(lua, clock)?;
    Ok(())
}

fn install_clock(lua: &Lua, clock: &Handle<GameClock>) -> LuaResult<()> {
    let table = lua.create_table()?;

    let s = clock.clone();
    table.set(
        "now",
        lua.create_function(move |lua, ()| {
            let clock = s.borrow()?;
            let now = lua.create_table()?;
            now.set("day", clock.day as i64)?;
            now.set("minute", clock.minute as i64)?;
            now.set("hour", (clock.minute / 60) as i64)?;
            now.set("minute_of_day", clock.minute as i64)?;
            now.set("rate", clock.rate)?;
            now.set("paused", clock.paused)?;
            Ok(now)
        })?,
    )?;
    let s = clock.clone();
    table.set(
        "set_rate",
        lua.create_function(move |_, rate: f32| {
            if !rate.is_finite() || rate < 0.0 {
                return Err(api_error("clock.set_rate must be finite and non-negative"));
            }
            s.borrow_mut()?.rate = rate;
            Ok(())
        })?,
    )?;
    let s = clock.clone();
    table.set(
        "pause",
        lua.create_function(move |_, paused: bool| {
            s.borrow_mut()?.paused = paused;
            Ok(())
        })?,
    )?;
    let s = clock.clone();
    table.set(
        "add",
        lua.create_function(move |_, minutes: f32| {
            if !minutes.is_finite() {
                return Err(api_error("clock.add minutes must be finite"));
            }
            s.borrow_mut()?.add_minutes(minutes);
            Ok(())
        })?,
    )?;
    let s = clock.clone();
    table.set(
        "set",
        lua.create_function(move |_, (day, minute): (i64, i64)| {
            if day < 1 || minute < 0 {
                return Err(api_error("clock.set day must be >= 1 and minute >= 0"));
            }
            s.borrow_mut()?
                .set(day as u32, minute.min((MINUTES_PER_DAY - 1) as i64) as u32);
            Ok(())
        })?,
    )?;
    let s = clock.clone();
    table.set(
        "sleep_to_morning",
        lua.create_function(move |_, ()| {
            let day = s.borrow_mut()?.sleep_to_morning();
            Ok(day as i64)
        })?,
    )?;

    lua.globals().set("clock", table)
}

fn install_calendar(lua: &Lua, clock: &Handle<GameClock>) -> LuaResult<()> {
    // `calendar` derives season/year from the absolute in-game day (M16-08).
    let table = lua.create_table()?;

    let s = clock.clone();
    table.set(
        "now",
        lua.create_function(move |lua, ()| {
            let clock = s.borrow()?;
            let now = lua.create_table()?;
            now.set("absolute_day", clock.day as i64)?;
            now.set("day", clock.day_of_season() as i64)?;
            now.set("season", clock.season() as i64)?;
            now.set("year", clock.year() as i64)?;
            now.set("minute", clock.minute as i64)?;
            Ok(now)
        })?,
    )?;
    let s = clock.clone();
    table.set(
        "set",
        lua.create_function(
            move |_, (day, season, year, minute): (i64, i64, i64, i64)| {
                let days = DAYS_PER_SEASON as i64;
                if day < 1 || day > days {
                    return Err(api_error(format!("calendar.set day must be in 1..={days}")));
                }
                if !(1..=SEASONS_PER_YEAR as i64).contains(&season) {
                    return Err(api_error("calendar.set season must be in 1..=4"));
                }
                if year < 1 {
                    return Err(api_error("calendar.set year must be >= 1"));
                }
                let absolute = (year - 1) * DAYS_PER_YEAR as i64 + (season - 1) * days + day;
                s.borrow_mut()?
                    .set_absolute(absolute as u32, minute.max(0) as u32);
                Ok(())
            },
        )?,
    )?;

    lua.globals().set("calendar", table)
}

fn install_day(lua: &Lua, clock: &Handle<GameClock>) -> LuaResult<()> {
    // `day` lifecycle: sleep to the next morning, firing `on_day_end` then
    // `on_day_start` (M16-08). Gameplay registers those globals for settlement.
    let table = lua.create_table()?;

    let s = clock.clone();
    table.set(
        "sleep",
        lua.create_function(move |lua, ()| {
            call_optional_day_hook(lua, "on_day_end")?;
            let new_day = {
                let mut clock = s.borrow_mut()?;
                clock.sleep_to_morning()
            };
            call_optional_day_hook(lua, "on_day_start")?;
            Ok(new_day as i64)
        })?,
    )?;
    let s = clock.clone();
    table.set(
        "now",
        lua.create_function(move |lua, ()| {
            let clock = s.borrow()?;
            let now = lua.create_table()?;
            now.set("day", clock.day as i64)?;
            now.set("minute", clock.minute as i64)?;
            Ok(now)
        })?,
    )?;

    lua.globals().set("day", table)
}

/// 调用一个可选的全局日钩子（`on_day_end` / `on_day_start`）。没有定义就是跳过。
///
/// 从 `rustlua-scripting` 的 `script/mod.rs` 逐字搬来：这两个钩子只服务于 `day.sleep`，
/// 跟着它走比留在宿主里再反向依赖更干净。钩子报错照常向上抛（脚本作者要看见）。
fn call_optional_day_hook(lua: &Lua, name: &str) -> LuaResult<()> {
    if let Ok(function) = lua.globals().get::<Function>(name) {
        function.call::<()>(())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed() -> (Lua, Handle<GameClock>) {
        let lua = Lua::new();
        let clock = handle();
        install(&lua, &clock).unwrap();
        (lua, clock)
    }

    /// 三张表的 Lua 面就这 10 个函数：搬家之后一个都不能少
    /// （`api_surface_snapshot` 盯全量，这条盯"这一步搬全了"）。
    const NAMES: [(&str, &[&str]); 3] = [
        (
            "clock",
            &["now", "set_rate", "pause", "add", "set", "sleep_to_morning"],
        ),
        ("calendar", &["now", "set"]),
        ("day", &["sleep", "now"]),
    ];

    #[test]
    fn every_clock_function_is_installed() {
        let (lua, _clock) = installed();
        for (table, names) in NAMES {
            let module: mlua::Table = lua.globals().get(table).unwrap();
            for name in names {
                let value: mlua::Value = module.get(*name).unwrap();
                assert!(
                    matches!(value, mlua::Value::Function(_)),
                    "`{table}.{name}` 缺失或不是函数：{value:?}"
                );
            }
        }
    }

    #[test]
    fn clock_state_lives_in_the_handle() {
        let (lua, clock) = installed();
        lua.load(
            r#"
            clock.set(3, 90)
            local now = clock.now()
            assert(now.day == 3 and now.minute == 90 and now.hour == 1, "clock.now")
            clock.set_rate(2.0)
            clock.pause(true)
            clock.add(30.5)
            local after = clock.now()
            assert(after.minute == 120 and after.rate == 2.0 and after.paused == true)
            assert(clock.sleep_to_morning() == 4)
            "#,
        )
        .exec()
        .unwrap();
        // 状态确实在能力单元里（宿主 `step_clock` 读同一份）。
        let clock = clock.borrow().unwrap();
        assert_eq!(clock.day, 4);
        assert_eq!(clock.minute, 6 * 60);
        assert!(
            clock.paused,
            "pause 是独立开关：`sleep_to_morning` 不解除暂停"
        );
        assert_eq!(clock.rate, 2.0, "rate 经句柄往返");
    }

    #[test]
    fn calendar_derives_season_and_year_from_the_absolute_day() {
        let (lua, clock) = installed();
        lua.load(
            r#"
            local now = calendar.now()
            assert(now.absolute_day == 1 and now.day == 1 and now.season == 1 and now.year == 1)
            calendar.set(1, 2, 1, 60)
            local spring2 = calendar.now()
            assert(spring2.season == 2 and spring2.day == 1 and spring2.absolute_day == 29)
            "#,
        )
        .exec()
        .unwrap();
        assert_eq!(clock.borrow().unwrap().day, 29);
    }

    #[test]
    fn day_sleep_fires_the_hooks_in_order_and_moves_the_handle() {
        let (lua, clock) = installed();
        lua.load(
            r#"
            trace = {}
            function on_day_end() trace[#trace + 1] = "end" end
            function on_day_start() trace[#trace + 1] = "start" end
            local next_day = day.sleep()
            assert(next_day == 2, "睡一觉到第二天")
            assert(#trace == 2 and trace[1] == "end" and trace[2] == "start", "钩子顺序")
            local now = day.now()
            assert(now.day == 2 and now.minute == 360)
            "#,
        )
        .exec()
        .unwrap();
        assert_eq!(clock.borrow().unwrap().day, 2);
    }

    #[test]
    fn invalid_arguments_are_rejected_without_moving_the_clock() {
        let (lua, clock) = installed();
        for (source, needle) in [
            ("clock.set_rate(-1.0)", "must be finite and non-negative"),
            ("clock.set(0, 0)", "day must be >= 1"),
            ("clock.add(0/0)", "minutes must be finite"),
            (
                "calendar.set(29, 1, 1, 0)",
                "calendar.set day must be in 1..=28",
            ),
            ("calendar.set(1, 5, 1, 0)", "season must be in 1..=4"),
            ("calendar.set(1, 1, 0, 0)", "year must be >= 1"),
        ] {
            let error = lua.load(source).exec().unwrap_err().to_string();
            assert!(
                error.contains(needle),
                "`{source}` 应报 `{needle}`：{error}"
            );
        }
        let clock = clock.borrow().unwrap();
        assert_eq!(*clock, GameClock::default(), "非法参数不得改动时钟");
    }
}
