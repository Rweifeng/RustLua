use std::os::raw::c_int;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::state::{ExtraData, RawLua};
use crate::util::{self, get_internal_metatable, WrappedFailure};

/// [DIAG] mlua wasm 宿主 Err 崩溃定位（2026-09-29，定位后移除）。
#[cfg(target_arch = "wasm32")]
static DIAG_WRAP_STEP: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
#[cfg(target_arch = "wasm32")]
pub(crate) fn diag_wrap_step(step: u8) {
    DIAG_WRAP_STEP.store(step, std::sync::atomic::Ordering::SeqCst);
}
#[cfg(target_arch = "wasm32")]
pub fn mlua_diag_wrap_read() -> u8 {
    DIAG_WRAP_STEP.load(std::sync::atomic::Ordering::SeqCst)
}

struct StateGuard<'a>(&'a RawLua, *mut ffi::lua_State);

impl<'a> StateGuard<'a> {
    fn new(inner: &'a RawLua, mut state: *mut ffi::lua_State) -> Self {
        state = inner.state.replace(state);
        Self(inner, state)
    }
}

impl Drop for StateGuard<'_> {
    fn drop(&mut self) {
        self.0.state.set(self.1);
    }
}

// An optimized version of `callback_error` that does not allocate `WrappedFailure` userdata
// and instead reuses unused values from previous calls (or allocates new).
pub(crate) unsafe fn callback_error_ext<F, R>(
    state: *mut ffi::lua_State,
    mut extra: *mut ExtraData,
    wrap_error: bool,
    f: F,
) -> R
where
    F: FnOnce(*mut ExtraData, c_int) -> Result<R>,
{
    if extra.is_null() {
        extra = ExtraData::get(state);
    }

    let nargs = ffi::lua_gettop(state);

    enum PreallocatedFailure {
        New(*mut WrappedFailure),
        Reserved,
    }

    impl PreallocatedFailure {
        unsafe fn reserve(state: *mut ffi::lua_State, extra: *mut ExtraData) -> Self {
            if (*extra).wrapped_failure_top > 0 {
                (*extra).wrapped_failure_top -= 1;
                return PreallocatedFailure::Reserved;
            }

            // We need to check stack for Luau in case when callback is called from interrupt
            // See https://github.com/luau-lang/luau/issues/446 and mlua #142 and #153
            #[cfg(feature = "luau")]
            ffi::lua_rawcheckstack(state, 2);
            // Place it to the beginning of the stack
            let ud = WrappedFailure::new_userdata(state);
            ffi::lua_insert(state, 1);
            PreallocatedFailure::New(ud)
        }

        #[cold]
        unsafe fn r#use(&self, state: *mut ffi::lua_State, extra: *mut ExtraData) -> *mut WrappedFailure {
            let ref_thread = (*extra).ref_thread;
            match *self {
                PreallocatedFailure::New(ud) => {
                    ffi::lua_settop(state, 1);
                    ud
                }
                PreallocatedFailure::Reserved => {
                    let index = (*extra).wrapped_failure_pool.pop().unwrap();
                    ffi::lua_settop(state, 0);
                    #[cfg(feature = "luau")]
                    ffi::lua_rawcheckstack(state, 2);
                    ffi::lua_xpush(ref_thread, state, index);
                    ffi::lua_pushnil(ref_thread);
                    ffi::lua_replace(ref_thread, index);
                    (*extra).ref_free.push(index);
                    ffi::lua_touserdata(state, -1) as *mut WrappedFailure
                }
            }
        }

        unsafe fn release(self, state: *mut ffi::lua_State, extra: *mut ExtraData) {
            let ref_thread = (*extra).ref_thread;
            match self {
                PreallocatedFailure::New(_) => {
                    ffi::lua_rotate(state, 1, -1);
                    ffi::lua_xmove(state, ref_thread, 1);
                    let index = (*extra).ref_stack_pop();
                    (*extra).wrapped_failure_pool.push(index);
                    (*extra).wrapped_failure_top += 1;
                }
                PreallocatedFailure::Reserved => (*extra).wrapped_failure_top += 1,
            }
        }
    }

    // We cannot shadow Rust errors with Lua ones, so we need to reserve pre-allocated memory
    // to store a wrapped failure (error or panic) *before* we proceed.
    let prealloc_failure = PreallocatedFailure::reserve(state, extra);

    // wasm32-unknown-unknown 特殊分支（P2 fork 2026-09-29）：
    // `catch_unwind` 的 Rust EH 边界会把 Lua `lua_error`（longjmp/throw）截获成 Rust
    // panic——panic=abort 下直接 `unreachable` trap（浏览器只报 "Error: unreachable"，
    // Lua 侧 `pcall(宿主函数)` 捕获不到宿主返回的 Err，场景切换一遇已死实体就崩）。
    // 这里在 wasm 上跳过 catch_unwind：宿主函数返回 Err 时 `lua_error` 的 throw 不再
    // 穿过 Rust 的 catch 边界，直达 Lua VM 的 pcall 被正常捕获；代价是 wasm 上宿主
    // panic 无法被 catch_unwind 兜底（本就 panic=abort，行为一致）。桌面保持原逻辑。
    #[cfg(target_arch = "wasm32")]
    {
        diag_wrap_step(1);
        match f(extra, nargs) {
            Ok(r) => {
                // Return unused `WrappedFailure` to the pool
                prealloc_failure.release(state, extra);
                diag_wrap_step(10);
                r
            }
            Err(err) => {
                diag_wrap_step(2);
                let wrapped_error = prealloc_failure.r#use(state, extra);
                diag_wrap_step(3);

                if !wrap_error {
                    ptr::write(wrapped_error, WrappedFailure::Error(err));
                    diag_wrap_step(4);
                    get_internal_metatable::<WrappedFailure>(state);
                    diag_wrap_step(5);
                    ffi::lua_setmetatable(state, -2);
                    diag_wrap_step(6);
                    ffi::lua_error(state)
                }

                // Build `CallbackError` with traceback
                diag_wrap_step(7);
                let traceback = if ffi::lua_checkstack(state, ffi::LUA_TRACEBACK_STACK) != 0 {
                    ffi::luaL_traceback(state, state, ptr::null(), 0);
                    let traceback = util::to_string(state, -1);
                    ffi::lua_pop(state, 1);
                    traceback
                } else {
                    "<not enough stack space for traceback>".to_string()
                };
                let cause = Arc::new(err);
                ptr::write(
                    wrapped_error,
                    WrappedFailure::Error(Error::CallbackError { traceback, cause }),
                );
                diag_wrap_step(8);
                get_internal_metatable::<WrappedFailure>(state);
                ffi::lua_setmetatable(state, -2);
                diag_wrap_step(9);
                ffi::lua_error(state)
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    match catch_unwind(AssertUnwindSafe(|| {
        let rawlua = (*extra).raw_lua();
        let _guard = StateGuard::new(rawlua, state);
        f(extra, nargs)
    })) {
        Ok(Ok(r)) => {
            // Return unused `WrappedFailure` to the pool
            prealloc_failure.release(state, extra);
            r
        }
        Ok(Err(err)) => {
            let wrapped_error = prealloc_failure.r#use(state, extra);

            if !wrap_error {
                ptr::write(wrapped_error, WrappedFailure::Error(err));
                get_internal_metatable::<WrappedFailure>(state);
                ffi::lua_setmetatable(state, -2);
                ffi::lua_error(state)
            }

            // Build `CallbackError` with traceback
            let traceback = if ffi::lua_checkstack(state, ffi::LUA_TRACEBACK_STACK) != 0 {
                ffi::luaL_traceback(state, state, ptr::null(), 0);
                let traceback = util::to_string(state, -1);
                ffi::lua_pop(state, 1);
                traceback
            } else {
                "<not enough stack space for traceback>".to_string()
            };
            let cause = Arc::new(err);
            ptr::write(
                wrapped_error,
                WrappedFailure::Error(Error::CallbackError { traceback, cause }),
            );
            get_internal_metatable::<WrappedFailure>(state);
            ffi::lua_setmetatable(state, -2);

            ffi::lua_error(state)
        }
        Err(p) => {
            let wrapped_panic = prealloc_failure.r#use(state, extra);
            ptr::write(wrapped_panic, WrappedFailure::Panic(Some(p)));
            get_internal_metatable::<WrappedFailure>(state);
            ffi::lua_setmetatable(state, -2);
            ffi::lua_error(state)
        }
    }
}
