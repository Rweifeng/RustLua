#ifndef WASM_SHIM_SETJMP_H
#define WASM_SHIM_SETJMP_H

/*
 * Minimal setjmp/longjmp for wasm32-unknown-unknown, implemented on top of the
 * WebAssembly exception-handling proposal (see csrc/sjlj.c).
 *
 * The new-wasm-sjlj ABI (LLVM PR #84137) stores a func_invocation_id, a label
 * and the longjmp payload inside the jmp_buf; the leading fields must exactly
 * match struct jmp_buf_impl in csrc/sjlj.c.
 */

typedef struct {
  void *func_invocation_id;
  unsigned int label;
  struct {
    void *env;
    int val;
  } arg;
  unsigned char __reserved[48];
} jmp_buf[1];

int setjmp(jmp_buf env) __attribute__((returns_twice));
void longjmp(jmp_buf env, int val) __attribute__((noreturn));

#define _setjmp  setjmp
#define _longjmp longjmp

#endif
