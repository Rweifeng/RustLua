/*
 * wasm32: raise Lua errors from C, never from Rust.
 *
 * On wasm32-unknown-unknown the engine builds with panic=abort, so Rust frames
 * carry no unwind tables and any foreign wasm-EH throw (Lua's setjmp/longjmp,
 * see sjlj.c) that crosses one is turned into `panic in a function that cannot
 * unwind` -> unreachable trap, whether the raising callee is declared
 * `extern "C"` or `extern "C-unwind"`. A host callback returning Err therefore
 * must NOT call lua_error from Rust: the throw would have to cross the Rust
 * trampoline frame to reach the Lua VM's pcall catch and would abort instead.
 *
 * Contract: mlua pushes this function as the lua_CFunction (upvalue 1 = the
 * CallbackUpvalue userdata, upvalue 2 = light userdata holding the Rust
 * callback pointer). The Rust callback returns the usual nargs, or
 * MLUA_CB_RAISE with the error object left on top of the stack; this function
 * then raises from C so the throw stays entirely within C frames and is caught
 * by the pcall/setjmp handler below.
 */
#include <stddef.h>

typedef struct lua_State lua_State;

extern int lua_error(lua_State *L);
extern void *lua_touserdata(lua_State *L, int idx);

/* Lua 5.4: LUA_REGISTRYINDEX = -LUAI_MAXSTACK - 1000 = -1001000. */
#define MLUA_REGISTRYINDEX (-1001000)
#define MLUA_UPVALUE_2 (MLUA_REGISTRYINDEX - 2)

/* Must match mlua::state::util::MLUA_CB_RAISE. */
#define MLUA_CB_RAISE (-100)

typedef int (*mlua_rust_callback)(lua_State *);

int mlua_c_trampoline(lua_State *L) {
  mlua_rust_callback cb = (mlua_rust_callback)lua_touserdata(L, MLUA_UPVALUE_2);
  int r = cb(L);
  if (r == MLUA_CB_RAISE) {
    return lua_error(L);
  }
  return r;
}
