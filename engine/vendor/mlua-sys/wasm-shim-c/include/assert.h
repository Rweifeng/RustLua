#ifndef WASM_SHIM_ASSERT_H
#define WASM_SHIM_ASSERT_H

/*
 * Minimal <assert.h> for wasm32-unknown-unknown. `abort` is provided by the
 * shim runtime (csrc/sjlj.c).
 */

#ifdef NDEBUG
#define assert(x) ((void)0)
#else
extern void abort(void) __attribute__((noreturn));
#define assert(x) ((void)((x) ? 0 : (abort(), 0)))
#endif

#endif
