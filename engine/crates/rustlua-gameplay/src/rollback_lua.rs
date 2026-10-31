//! `rollback` 表的 Lua 绑定：把 [`rustlua_net::Rollback`] 的预测/回滚账本交给脚本。
//!
//! 账本不碰模拟：快照是脚本自己序列化的字符串（整张场景表、世界状态都行），
//! 引擎只保管字节并回答"从哪一帧起重模拟"。典型每帧用法：
//!
//! ```lua
//! local frame = rollback.next_frame()
//! rollback.save_snapshot(frame, world.snapshot())
//! rollback.record_local(frame, encode_input(local_input))
//! local inputs = rollback.inputs(frame)      -- 远端缺失处沿用上一帧预测
//! world.step(frame, inputs)
//! rollback.commit(frame)
//! -- 收包：
//! local kind, from = rollback.accept(player, frame, payload)
//! if kind == "resimulate" then
//!   world.restore(rollback.rollback_to(from))
//! end
//! ```
//!
//! 返回值口径：
//! - `accept` 返回 `"accepted"` / `"resimulate"` / `"toolate"`，后两种附带帧号；
//! - `inputs` 返回按玩家号升序的 `{ {player=, payload=}, ... }`；
//! - `rollback_to` 返回该帧模拟前的快照字节。
//!
//! 配置越界、提交乱序、快照不在窗口内都直接抛 Lua 错误：它们是脚本的调用顺序问题，
//! 静默返回 false 会把"不同步"伪装成"网络慢"。

use mlua::{Lua, Result as LuaResult};
use rustlua_lua_api::Handle;
use rustlua_net::{Rollback, RollbackConfig, RollbackStep};

/// 账本随注册表同寿命：热重载重建 VM 时回滚窗口与已确认输入都还在。
#[derive(Default)]
pub struct RollbackState {
    ledger: Option<Rollback>,
}

fn runtime(message: impl Into<String>) -> mlua::Error {
    mlua::Error::RuntimeError(message.into())
}

fn with_ledger<R>(
    state: &Handle<RollbackState>,
    apply: impl FnOnce(&mut Rollback) -> LuaResult<R>,
) -> LuaResult<R> {
    let mut guard = state.borrow_mut()?;
    let ledger = guard
        .ledger
        .as_mut()
        .ok_or_else(|| runtime("rollback.reset(players, local, window) has not been called"))?;
    apply(ledger)
}

/// 建一份回滚账本能力单元（注册表捕获同一份）。
#[must_use]
pub fn state() -> Handle<RollbackState> {
    Handle::new(RollbackState::default(), "rollback")
}

/// 装 `rollback` 全局表。
pub fn install(lua: &Lua, state: &Handle<RollbackState>) -> LuaResult<()> {
    let table = lua.create_table()?;

    let s = state.clone();
    table.set(
        "reset",
        lua.create_function(move |_, (players, local, window): (u8, u8, u32)| {
            let config = RollbackConfig::new(players, local, window)
                .map_err(|error| runtime(error.to_string()))?;
            s.borrow_mut()?.ledger = Some(Rollback::new(config));
            Ok(())
        })?,
    )?;

    let s = state.clone();
    table.set(
        "next_frame",
        lua.create_function(move |_, ()| with_ledger(&s, |ledger| Ok(ledger.next_frame())))?,
    )?;

    let s = state.clone();
    table.set(
        "record_local",
        lua.create_function(move |_, (frame, payload): (u32, mlua::String)| {
            let bytes = payload.as_bytes().to_vec();
            with_ledger(&s, |ledger| {
                ledger
                    .record_local(frame, bytes)
                    .map_err(|error| runtime(error.to_string()))
            })
        })?,
    )?;

    let s = state.clone();
    table.set(
        "accept",
        lua.create_function(
            move |_, (player, frame, payload): (u8, u32, mlua::String)| {
                let bytes = payload.as_bytes().to_vec();
                let step = with_ledger(&s, |ledger| {
                    ledger
                        .accept_remote(player, frame, bytes)
                        .map_err(|error| runtime(error.to_string()))
                })?;
                Ok(match step {
                    RollbackStep::Accepted => ("accepted", None),
                    RollbackStep::Resimulate { from } => ("resimulate", Some(from)),
                    RollbackStep::TooLate { frame } => ("toolate", Some(frame)),
                })
            },
        )?,
    )?;

    let s = state.clone();
    table.set(
        "inputs",
        lua.create_function(move |lua, frame: u32| {
            let inputs = with_ledger(&s, |ledger| Ok(ledger.inputs_for(frame)))?;
            let table = lua.create_table()?;
            for (index, (player, payload)) in inputs.into_iter().enumerate() {
                let entry = lua.create_table()?;
                entry.set("player", player)?;
                entry.set("payload", lua.create_string(&payload)?)?;
                table.set(index + 1, entry)?;
            }
            Ok(table)
        })?,
    )?;

    let s = state.clone();
    table.set(
        "save_snapshot",
        lua.create_function(move |_, (frame, bytes): (u32, mlua::String)| {
            let snapshot = bytes.as_bytes().to_vec();
            with_ledger(&s, |ledger| {
                ledger.save_snapshot(frame, snapshot);
                Ok(())
            })
        })?,
    )?;

    let s = state.clone();
    table.set(
        "commit",
        lua.create_function(move |_, frame: u32| {
            with_ledger(&s, |ledger| {
                ledger
                    .commit_frame(frame)
                    .map_err(|error| runtime(error.to_string()))
            })
        })?,
    )?;

    let s = state.clone();
    table.set(
        "rollback_to",
        lua.create_function(move |lua, from: u32| {
            let snapshot = with_ledger(&s, |ledger| {
                ledger
                    .rollback_to(from)
                    .map_err(|error| runtime(error.to_string()))
            })?;
            lua.create_string(&snapshot)
        })?,
    )?;

    let s = state.clone();
    table.set(
        "stats",
        lua.create_function(move |lua, ()| {
            let stats = with_ledger(&s, |ledger| Ok(ledger.stats()))?;
            let table = lua.create_table()?;
            table.set("resimulations", stats.resimulations)?;
            table.set("resimulated_frames", stats.resimulated_frames)?;
            table.set("stale_inputs", stats.stale_inputs)?;
            table.set("predicted_inputs", stats.predicted_inputs)?;
            Ok(table)
        })?,
    )?;

    lua.globals().set("rollback", table)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lua_with_rollback() -> Lua {
        let lua = Lua::new();
        install(&lua, &state()).expect("install");
        lua
    }

    #[test]
    fn predicted_frame_is_replaced_by_resimulation() {
        let lua = lua_with_rollback();
        lua.load(
            r#"
            rollback.reset(2, 1, 4)

            local frame = rollback.next_frame()
            assert(frame == 0)
            rollback.save_snapshot(frame, "snap0")
            rollback.record_local(frame, "L0")
            local kind = rollback.accept(0, frame, "R0")
            assert(kind == "accepted")

            local inputs = rollback.inputs(frame)
            assert(#inputs == 2, "expected the full player set")
            assert(inputs[1].player == 0 and inputs[1].payload == "R0")
            assert(inputs[2].player == 1 and inputs[2].payload == "L0")
            rollback.commit(frame)

            -- 第 1 帧远端还没到：沿用上一帧的 R0 预测。
            rollback.save_snapshot(1, "snap1")
            rollback.record_local(1, "L1")
            assert(rollback.inputs(1)[1].payload == "R0")
            rollback.commit(1)
            assert(rollback.next_frame() == 2)

            -- 确认输入与预测不同：从第 1 帧起重模拟。
            local kind, from = rollback.accept(0, 1, "R1")
            assert(kind == "resimulate" and from == 1)
            assert(rollback.rollback_to(from) == "snap1")
            assert(rollback.next_frame() == 1)
            assert(rollback.inputs(1)[1].payload == "R1")

            local stats = rollback.stats()
            assert(stats.resimulations == 1)
            assert(stats.resimulated_frames == 1)
            assert(stats.predicted_inputs == 1)
        "#,
        )
        .exec()
        .expect("script");
    }

    #[test]
    fn misuse_raises_lua_errors() {
        let lua = lua_with_rollback();
        lua.load(
            r#"
            -- 没 reset 就调用：报错而不是悄悄返回 nil。
            assert(not pcall(rollback.next_frame))
            rollback.reset(2, 1, 2)
            assert(not pcall(rollback.commit, 5), "out-of-order commit must raise")
            for frame = 0, 2 do
                rollback.save_snapshot(frame, "s" .. frame)
                rollback.commit(frame)
            end
            -- window=2：窗口外（0）的快照已淘汰，回滚必须报错；窗口内（1）仍可回滚。
            assert(not pcall(rollback.rollback_to, 0))
            assert(rollback.rollback_to(1) == "s1")
            assert(rollback.next_frame() == 1)
        "#,
        )
        .exec()
        .expect("script");
    }
}
