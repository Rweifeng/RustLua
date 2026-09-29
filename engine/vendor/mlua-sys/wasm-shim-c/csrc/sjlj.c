/*
 * wasm32-unknown-unknown setjmp/longjmp runtime support.
 *
 * Implements the LLVM "new wasm setjmp/longjmp" ABI (llvm-project PR #84137):
 * clang 20 (zig 0.16) lowers setjmp/longjmp calls, when compiling with
 * `-mllvm -wasm-enable-sjlj`, into calls to __wasm_setjmp / __wasm_setjmp_test
 * / __wasm_longjmp, plus try/catch on the __c_longjmp tag.  This is the same
 * contract wasi-libc implements (libc-top-half/musl/src/setjmp/wasm32/rt.c).
 *
 * jmp_buf must be large enough to hold struct jmp_buf_impl below; see
 * include/setjmp.h.
 */
#include <stddef.h>
#include <stdint.h>

struct jmp_buf_impl {
  void *func_invocation_id;
  uint32_t label;

  /* Temporary storage used to pass the longjmp payload from
   * __wasm_longjmp to the compiler-generated catch handler. */
  struct arg {
    void *env;
    int val;
  } arg;
};

void __wasm_setjmp(void *env, uint32_t label, void *func_invocation_id) {
  struct jmp_buf_impl *jb = env;
  if (label == 0) /* ABI contract */
    __builtin_trap();
  if (func_invocation_id == NULL) /* sanity check */
    __builtin_trap();
  jb->func_invocation_id = func_invocation_id;
  jb->label = label;
}

uint32_t __wasm_setjmp_test(void *env, void *func_invocation_id) {
  struct jmp_buf_impl *jb = env;
  if (jb->label == 0) /* ABI contract */
    __builtin_trap();
  if (func_invocation_id == NULL) /* sanity check */
    __builtin_trap();
  if (jb->func_invocation_id == func_invocation_id)
    return jb->label;
  return 0;
}

void __wasm_longjmp(void *env, int val) {
  struct jmp_buf_impl *jb = env;
  struct arg *arg = &jb->arg;
  /* C standard: longjmp must not cause setjmp to return 0. */
  if (val == 0)
    val = 1;
  arg->env = env;
  arg->val = val;
  __builtin_wasm_throw(1, arg); /* 1 == C_LONGJMP */
}

/* abort/exit: no process to exit on bare wasm — trap. */
void abort(void) { __builtin_trap(); }
void exit(int status) {
  (void)status;
  __builtin_trap();
}
