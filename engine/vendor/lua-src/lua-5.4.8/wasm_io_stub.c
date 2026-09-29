/*
 * wasm32-unknown-unknown: `io` library stub.
 *
 * The `io` library is dropped on bare WebAssembly (no filesystem), so
 * linit.c no longer registers it and liolib.c is excluded from the build.
 * mlua's StdLib machinery still references `luaopen_io` (via the
 * extern-function table), so provide a stub that pushes an empty module
 * table. This file only compiles into the wasm build (-DLUA_NO_IO_LIB);
 * the native build defines the real `luaopen_io` in liolib.c.
 */
#if defined(LUA_NO_IO_LIB)

#include "lua.h"

int luaopen_io(lua_State *L) {
  lua_createtable(L, 0, 0);
  return 1;
}

#endif /* LUA_NO_IO_LIB */
