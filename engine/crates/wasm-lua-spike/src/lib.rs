//! Minimal wasm32-unknown-unknown verification harness for the vendored mlua
//! fork. Each export exercises one facet of the wasm port:
//!
//! - `spike_add`: Lua::new() + eval (VM bootstrap, allocation via realloc)
//! - `spike_arith`: float arithmetic + tostring (%g via shim snprintf)
//! - `spike_error`: pcall of a failing chunk (setjmp/longjmp via wasm EH)
//! - `spike_format`: string.format float formats (%f/%e/%g)
//! - `spike_sort`: table.sort (qsort from the shim)
//! - `spike_math`: libm exports (sqrt/sin/floor/...)
//! - `spike_rust_fn`: Rust closure registered as a Lua function (FFI callback)
//! - `spike_tostring`: number -> Lua string, copied out to a caller buffer

use mlua::{Function, Lua, Result as LuaResult};

#[no_mangle]
pub extern "C" fn spike_add(a: i64, b: i64) -> i64 {
    let lua = Lua::new();
    lua.load(format!("return {} + {}", a, b))
        .eval::<i64>()
        .unwrap_or(-999)
}

#[no_mangle]
pub extern "C" fn spike_arith(a: f64, b: f64) -> f64 {
    let lua = Lua::new();
    lua.load(format!("return {} * {} + 1.5", a, b))
        .eval::<f64>()
        .unwrap_or(f64::NAN)
}

#[no_mangle]
pub extern "C" fn spike_error() -> i32 {
    // If longjmp works, the RuntimeError is caught and we return 1.
    let lua = Lua::new();
    match lua.load("error('boom')").exec() {
        Ok(_) => 0, // expected to fail
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("boom") {
                1
            } else {
                2
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn spike_format(a: f64, buf: *mut u8, cap: usize) -> i32 {
    let lua = Lua::new();
    let script = format!("return string.format('%10.3f %e %g', {}, {}, {})", a, a, a);
    match lua.load(&script).eval::<String>() {
        Ok(s) => write_out(&s, buf, cap),
        Err(_) => -2,
    }
}

#[no_mangle]
pub extern "C" fn spike_sort(buf: *mut u8, cap: usize) -> i32 {
    let lua = Lua::new();
    match lua
        .load("local t = {5,3,9,1,7}; table.sort(t); return table.concat(t, ',')")
        .eval::<String>()
    {
        Ok(s) => write_out(&s, buf, cap),
        Err(_) => -2,
    }
}

#[no_mangle]
pub extern "C" fn spike_math() -> f64 {
    let lua = Lua::new();
    lua.load("return math.sqrt(2) + math.sin(1) + math.floor(2.9)")
        .eval::<f64>()
        .unwrap_or(f64::NAN)
}

#[no_mangle]
pub extern "C" fn spike_rust_fn() -> i64 {
    let lua = Lua::new();
    let mul: Function = match lua
        .create_function(|_, (x, y): (i64, i64)| LuaResult::Ok(x * y + 3))
        .and_then(|f| {
            lua.globals().set("rust_mul", f)?;
            lua.globals().get::<Function>("rust_mul")
        }) {
        Ok(f) => f,
        Err(_) => return -2,
    };
    match mul.call::<i64>((6, 7)) {
        Ok(v) => v,
        Err(_) => -3,
    }
}

#[no_mangle]
pub extern "C" fn spike_tostring(x: f64, buf: *mut u8, cap: usize) -> i32 {
    let lua = Lua::new();
    match lua
        .load(format!("return tostring({})", x))
        .eval::<String>()
    {
        Ok(s) => write_out(&s, buf, cap),
        Err(_) => -2,
    }
}

#[no_mangle]
pub extern "C" fn spike_type() -> i32 {
    let lua = Lua::new();
    match lua.load("return type({1,2})").eval::<String>() {
        Ok(s) if s == "table" => 1,
        Ok(_) => 2,
        Err(_) => -2,
    }
}

fn write_out(s: &str, buf: *mut u8, cap: usize) -> i32 {
    if buf.is_null() || cap == 0 {
        return s.len() as i32;
    }
    let bytes = s.as_bytes();
    let n = bytes.len().min(cap - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, n);
        *buf.add(n) = 0;
    }
    bytes.len() as i32
}
