#ifndef WASM_SHIM_SIGNAL_H
#define WASM_SHIM_SIGNAL_H

/*
 * Minimal <signal.h> for wasm32-unknown-unknown. Lua only needs
 * `sig_atomic_t` (lstate.h defines l_signalT from it); signal handlers make
 * no sense on bare wasm.
 */

typedef int sig_atomic_t;

#endif
