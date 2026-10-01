//! wasm Lua 桥（WP10 迁移：Lua 执行层从 fengari 迁移到 mlua-wasm）。
//!
//! 职责：把 `web-runtime.js` 与 Lua 的交互面收窄成 5 个 wasm 导出函数，
//! JS 侧不再持有 Lua VM（fengari），全部 Lua 交互改经本桥：
//!
//! | 导出 | 语义 | 对应 web-runtime.js 旧调用 |
//! |---|---|---|
//! | `lua_boot` / `lua_boot_with_reader` | 建 LuaRuntime + vfs provider | `luaL_newstate` + `luaL_openlibs` |
//! | `lua_do_string` | 执行补丁/脚本代码串 | `doString(L, src, name)` |
//! | `lua_call_global` | 调 Lua 全局函数，返回字符串 | `callGlobal(L, name, args)` |
//! | `lua_register_js_callback` | JS 回调注册成 Lua 全局 | `installLuaAudioBridge` 的 `registerLuaFunction` |
//! | `lua_missing_globals` | 查宿主全局缺失清单 | `missingHostGlobals(L)` |
//!
//! 值协议（与 fengari 版逐项对齐）：
//! - 参数：JSON 数组（string/number/boolean/null），Rust 侧转 Lua 值；
//! - 返回值：`callGlobal` 的语义是"只取字符串"——`Value::String/Number` → `Some`，
//!   其余（含 nil / boolean / table）→ `None`（fengari `lua_isstring` 同款判定）；
//! - JS 回调：Lua 参数逐个转 JS 基本值；JS 返回 **数组 = 多返回值**（音频桥
//!   `[ok, reason]` 的 fengari 多 push 语义），单值 = 单返回值。

use std::cell::RefCell;

use wasm_bindgen::prelude::*;

/// 浏览器（webgl）入口预加载的项目资源表（`start()` 填充，`lua_boot()` 消费）。
///
/// 与微信端"打包资源表"语义一致：key 为项目相对路径（`/` 分隔，如
/// `scripts/main.lua`），值为**打包原始字节**（`pack::decode` 由引擎侧
/// `read_project_script` 自动解开）。
thread_local! {
    static LUA_RESOURCES: RefCell<Option<std::collections::HashMap<String, Vec<u8>>>> =
        const { RefCell::new(None) };
}

/// 当前 Lua 运行时（单线程 wasm：一个槽位足够；`lua_boot` 建，Lua 桥函数用）。
///
/// 借用约定：`LuaRuntime::lua()` 只需 `&self`，因此所有只读入口（do_string / execute_main /
/// call_global / register_js_callback / missing_globals）一律用共享 `borrow()`；唯一的
/// `borrow_mut()` 在 `boot_runtime`（一次性写入，此时尚无 Lua 在跑）。这样 Lua 执行期间经
/// JS 回调重入本桥时，嵌套的共享借用可以并存，不会触发 `already mutably borrowed: BorrowError`
/// （此前 do_string 持 `borrow_mut` 跨 `.exec()`，重入的 call_global 在 `borrow()` 处 panic →
/// wasm 上是无法 unwind 的 `unreachable` trap）。
thread_local! {
    static LUA_RUNTIME: RefCell<Option<engine::core::script::LuaRuntime>> =
        const { RefCell::new(None) };
}

/// JS 回调注册表（`lua_register_js_callback` 存，mlua 包装闭包按索引取用）。
/// JS 函数与 wasm 桥同生命周期；Lua 状态销毁前不应清空。
thread_local! {
    static JS_CALLBACKS: RefCell<Vec<js_sys::Function>> = const { RefCell::new(Vec::new()) };
}

/// 把 JS 预加载资源表写进全局（`start()` 在 fetch 预加载后调用）。
pub fn set_resources(resources: std::collections::HashMap<String, Vec<u8>>) {
    LUA_RESOURCES.with(|slot| *slot.borrow_mut() = Some(resources));
}

fn js_error_text(e: &JsValue) -> String {
    e.as_string().unwrap_or_else(|| format!("{e:?}"))
}

fn provider_from_resources() -> Box<dyn Fn(&std::path::Path) -> Option<Vec<u8>> + 'static> {
    Box::new(|path: &std::path::Path| {
        let key = path
            .strip_prefix(super::VIRTUAL_PROJECT_ROOT)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_string_lossy().into_owned())
            .replace('\\', "/");
        LUA_RESOURCES.with(|slot| {
            slot.borrow()
                .as_ref()
                .and_then(|table| table.get(&key).cloned())
        })
    })
}

fn provider_from_reader(
    read_file: js_sys::Function,
) -> Box<dyn Fn(&std::path::Path) -> Option<Vec<u8>> + 'static> {
    Box::new(move |path: &std::path::Path| {
        let key = path
            .strip_prefix(super::VIRTUAL_PROJECT_ROOT)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_string_lossy().into_owned())
            .replace('\\', "/");
        let value = read_file
            .call1(&JsValue::NULL, &JsValue::from_str(&key))
            .ok()?;
        value.as_string().map(|s| s.into_bytes())
    })
}

fn boot_runtime(
    provider: Box<dyn Fn(&std::path::Path) -> Option<Vec<u8>> + 'static>,
) -> Result<(), JsValue> {
    engine::core::vfs::set_provider(provider).map_err(|e| JsValue::from_str(&e.to_string()))?;
    let mut runtime =
        engine::core::script::LuaRuntime::new().map_err(|e| JsValue::from_str(&e.to_string()))?;
    runtime.set_project_root(std::path::PathBuf::from(super::VIRTUAL_PROJECT_ROOT));
    LUA_RUNTIME.with(|slot| *slot.borrow_mut() = Some(runtime));
    Ok(())
}

/// 建 Lua 运行时（webgl：资源 provider 用 `start()` 预加载表）。
///
/// 注意：**不执行 `scripts/main.lua`**——web 侧流程是"先注入全部宿主补丁，
/// 再执行 main.lua"（与 fengari 版 `rustluaRunLuaGame` 同序）。`execute_project_script`
/// 内部的 `load_candidate` 会新建 Lua 状态（补丁会丢），因此 main.lua 由 JS 侧
/// 以 `lua_do_string(main, "scripts/main.lua")` 直接在既有状态上执行。
#[wasm_bindgen]
pub fn lua_boot() -> Result<(), JsValue> {
    super::install_panic_hook();
    boot_runtime(provider_from_resources())
}

/// 建 Lua 运行时（微信：资源 provider 用适配层同步读取器 `read_file`）。
#[wasm_bindgen]
pub fn lua_boot_with_reader(read_file: &js_sys::Function) -> Result<(), JsValue> {
    super::install_panic_hook();
    boot_runtime(provider_from_reader(read_file.clone()))
}

/// 在既有状态上执行 `scripts/main.lua`（web 侧在全部宿主补丁注入之后调用；
/// 读 vfs 预加载表 + `pack::decode` 自动解开压缩/加密，与桌面语义一致）。
///
/// **不能走 `execute_project_script`**：其内部 `load_candidate` 会 `Lua::new()` 新建
/// 状态并重跑 `register_api`，刚注入的 web 宿主补丁（`input`/`audio`/`__rustlua_*` 等）
/// 全部丢失。这里直接在运行时既有状态上 load + exec，保持补丁在位。
#[wasm_bindgen]
pub fn lua_execute_main() -> Result<(), JsValue> {
    LUA_RUNTIME.with(|slot| {
        let guard = slot.borrow();
        let runtime = guard
            .as_ref()
            .ok_or_else(|| JsValue::from_str("lua bridge: runtime not booted"))?;
        let path = std::path::PathBuf::from(super::VIRTUAL_PROJECT_ROOT).join("scripts/main.lua");
        let source = engine::core::script::read_project_script(&path)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        // 装 require 搜索器：mlua 默认搜索器走 C 文件 API，wasm 下无文件系统；
        // 与桌面 load_candidate 语义一致（vfs provider → pack::decode）。
        if let Some(script_dir) = path.parent() {
            engine::core::script::install_web_require(runtime.lua(), script_dir)
                .map_err(|e| JsValue::from_str(&format!("lua install require: {e}")))?;
        }
        runtime
            .lua()
            .load(&source)
            .set_name("scripts/main.lua")
            .exec()
            .map_err(|e| JsValue::from_str(&format!("lua scripts/main.lua: {e}")))
    })
}

/// 在既有状态上执行一段 Lua 代码（补丁/脚本；chunkname 用于报错定位）。
#[wasm_bindgen]
pub fn lua_do_string(source: &str, chunkname: &str) -> Result<(), JsValue> {
    LUA_RUNTIME.with(|slot| {
        let guard = slot.borrow();
        let runtime = guard
            .as_ref()
            .ok_or_else(|| JsValue::from_str("lua bridge: runtime not booted"))?;
        let lua = runtime.lua();
        lua.load(source)
            .set_name(chunkname)
            .exec()
            .map_err(|e| JsValue::from_str(&format!("lua {chunkname}: {e}")))
    })
}

/// 调用 Lua 全局函数。`args_json` 为 JSON 数组（string/number/boolean/null）。
/// 返回该全局的**字符串化结果**；全局不存在或返回非字符串（nil/boolean/table）
/// 返回 `None`（fengari `callGlobal` 同款协议）。
#[wasm_bindgen]
pub fn lua_call_global(name: &str, args_json: &str) -> Result<Option<String>, JsValue> {
    LUA_RUNTIME.with(|slot| {
        let guard = slot.borrow();
        let runtime = guard
            .as_ref()
            .ok_or_else(|| JsValue::from_str("lua bridge: runtime not booted"))?;
        let lua = runtime.lua();
        let value: mlua::Value = lua
            .globals()
            .get(name)
            .map_err(|e| JsValue::from_str(&format!("lua get global {name}: {e}")))?;
        let mlua::Value::Function(function) = value else {
            return Ok(None);
        };
        let args = parse_args_json(lua, args_json)?;
        let result: mlua::Value = function
            .call(args)
            .map_err(|e| JsValue::from_str(&format!("lua call {name}: {e}")))?;
        Ok(mlua_value_to_string(result))
    })
}

/// 注册 JS 回调为 Lua 全局（音频桥等 JS 宿主能力）。
///
/// JS 回调收 Lua 参数（string/number/boolean/nil → JS 基本值；其余 tostring），
/// 返回 **数组 = 多返回值**（音频桥 `[ok, reason]`），单值 = 单返回值。
#[wasm_bindgen]
pub fn lua_register_js_callback(name: &str, callback: &js_sys::Function) -> Result<(), JsValue> {
    let name_owned = name.to_owned();
    let index = JS_CALLBACKS.with(|slot| {
        let mut callbacks = slot.borrow_mut();
        callbacks.push(callback.clone());
        callbacks.len() - 1
    });
    LUA_RUNTIME.with(|slot| {
        let guard = slot.borrow();
        let runtime = guard
            .as_ref()
            .ok_or_else(|| JsValue::from_str("lua bridge: runtime not booted"))?;
        let lua = runtime.lua();
        let wrapped = lua
            .create_function(move |lua, args: mlua::MultiValue| {
                let js_fn = JS_CALLBACKS
                    .with(|slot| slot.borrow().get(index).cloned())
                    .ok_or_else(|| mlua::Error::RuntimeError("js callback slot lost".into()))?;
                let js_args = js_sys::Array::new();
                for arg in args.into_iter() {
                    js_args.push(&lua_value_to_js(lua, arg)?);
                }
                let result = js_fn.apply(&JsValue::NULL, &js_args).map_err(|e| {
                    mlua::Error::RuntimeError(format!("js callback {name_owned}: {e:?}"))
                })?;
                js_result_to_lua(lua, result)
            })
            .map_err(|e| JsValue::from_str(&format!("create_function {name}: {e}")))?;
        lua.globals()
            .set(name, wrapped)
            .map_err(|e| JsValue::from_str(&format!("set global {name}: {e}")))
    })
}

/// 返回宿主全局清单里**缺失**的名字（JSON 数组字符串）——`missingHostGlobals` 的替代。
#[wasm_bindgen]
pub fn lua_missing_globals(names_json: &str) -> Result<String, JsValue> {
    let names: Vec<String> = serde_json::from_str(names_json)
        .map_err(|e| JsValue::from_str(&format!("bad names json: {e}")))?;
    LUA_RUNTIME.with(|slot| {
        let guard = slot.borrow();
        let runtime = guard
            .as_ref()
            .ok_or_else(|| JsValue::from_str("lua bridge: runtime not booted"))?;
        let lua = runtime.lua();
        let mut missing = Vec::new();
        for name in names {
            let value: mlua::Value = lua
                .globals()
                .get(name.as_str())
                .map_err(|e| JsValue::from_str(&format!("lua get global {name}: {e}")))?;
            if matches!(value, mlua::Value::Nil) {
                missing.push(name);
            }
        }
        serde_json::to_string(&missing)
            .map_err(|e| JsValue::from_str(&format!("serialize missing globals: {e}")))
    })
}

// ---- 值转换 ----

fn parse_args_json(lua: &mlua::Lua, json: &str) -> Result<Vec<mlua::Value>, JsValue> {
    let parsed: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| JsValue::from_str(&format!("bad args json: {e}")))?;
    let serde_json::Value::Array(list) = parsed else {
        return Err(JsValue::from_str("lua bridge: args must be a JSON array"));
    };
    let mut values = Vec::with_capacity(list.len());
    for item in list {
        values.push(json_to_lua_value(lua, &item)?);
    }
    Ok(values)
}

fn json_to_lua_value(lua: &mlua::Lua, value: &serde_json::Value) -> Result<mlua::Value, JsValue> {
    match value {
        serde_json::Value::Null => Ok(mlua::Value::Nil),
        serde_json::Value::Bool(b) => Ok(mlua::Value::Boolean(*b)),
        serde_json::Value::Number(n) => {
            // 整数走 Value::Integer：Lua 5.4 区分 integer/float，luaL_checkinteger
            // 只认 Integer，float 42.0 会报错（fengari 是 5.3 不区分，迁移后必须显式分）。
            if let Some(i) = n.as_i64() {
                Ok(mlua::Value::Integer(i))
            } else if let Some(u) = n.as_u64() {
                i64::try_from(u)
                    .map(mlua::Value::Integer)
                    .map_err(|_| JsValue::from_str("lua bridge: integer overflow in args"))
            } else {
                n.as_f64()
                    .map(mlua::Value::Number)
                    .ok_or_else(|| JsValue::from_str("lua bridge: non-finite number in args"))
            }
        }
        serde_json::Value::String(s) => lua
            .create_string(s.as_bytes())
            .map(mlua::Value::String)
            .map_err(|e| JsValue::from_str(&format!("create_string: {e}"))),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => Err(JsValue::from_str(
            "lua bridge: nested JSON values are not supported in call args",
        )),
    }
}

/// fengari `callGlobal` 协议：只取字符串化结果（String/Number/Integer），其余 → None。
fn mlua_value_to_string(value: mlua::Value) -> Option<String> {
    match value {
        mlua::Value::String(s) => Some(s.to_string_lossy()),
        mlua::Value::Number(n) => Some(n.to_string()),
        mlua::Value::Integer(i) => Some(i.to_string()),
        _ => None,
    }
}

/// Lua 值 → JS 基本值（音频桥参数是 string；其余类型 tostring 兜底）。
fn lua_value_to_js(lua: &mlua::Lua, value: mlua::Value) -> Result<JsValue, mlua::Error> {
    match value {
        mlua::Value::Nil => Ok(JsValue::NULL),
        mlua::Value::Boolean(b) => Ok(JsValue::from_bool(b)),
        // Lua 5.4 的 42 是 Integer 不是 Number；JS 侧统一按 f64 数值给。
        mlua::Value::Integer(i) => Ok(JsValue::from_f64(i as f64)),
        mlua::Value::Number(n) => Ok(JsValue::from_f64(n)),
        mlua::Value::String(s) => Ok(JsValue::from_str(&s.to_string_lossy())),
        other => Ok(JsValue::from_str(&value_tostring(lua, other)?)),
    }
}

/// JS 回调结果 → Lua 值：**数组 = 多返回值**；单值 = 单返回。
fn js_result_to_lua(lua: &mlua::Lua, result: JsValue) -> Result<mlua::MultiValue, mlua::Error> {
    if let Some(array) = result.dyn_ref::<js_sys::Array>() {
        let mut values = Vec::with_capacity(array.length() as usize);
        for item in array.iter() {
            values.push(js_value_to_lua(lua, item)?);
        }
        Ok(mlua::MultiValue::from_vec(values))
    } else {
        Ok(mlua::MultiValue::from_vec(vec![js_value_to_lua(
            lua, result,
        )?]))
    }
}

fn js_value_to_lua(lua: &mlua::Lua, value: JsValue) -> Result<mlua::Value, mlua::Error> {
    if value.is_null() || value.is_undefined() {
        return Ok(mlua::Value::Nil);
    }
    if let Some(b) = value.as_bool() {
        return Ok(mlua::Value::Boolean(b));
    }
    if let Some(n) = value.as_f64() {
        // JS 数值都是 f64：整数值转 Integer（Lua 5.4 区分，整数运算/checkinteger 需要）。
        if n.fract() == 0.0 && n >= -(2f64.powi(63)) && n < 2f64.powi(63) {
            return Ok(mlua::Value::Integer(n as i64));
        }
        return Ok(mlua::Value::Number(n));
    }
    if let Some(s) = value.as_string() {
        return lua.create_string(s.as_bytes()).map(mlua::Value::String);
    }
    // 对象/函数等：tostring 兜底（音频桥只返回 boolean/string/数组）。
    let text = js_error_text(&value);
    lua.create_string(text.as_bytes()).map(mlua::Value::String)
}

fn value_tostring(lua: &mlua::Lua, value: mlua::Value) -> Result<String, mlua::Error> {
    match value {
        mlua::Value::String(s) => Ok(s.to_string_lossy()),
        mlua::Value::Number(n) => Ok(n.to_string()),
        mlua::Value::Boolean(b) => Ok(b.to_string()),
        mlua::Value::Nil => Ok("nil".to_owned()),
        other => Ok(lua
            .coerce_string(other)?
            .map(|s| s.to_string_lossy())
            .unwrap_or_else(|| "<unprintable>".to_owned())),
    }
}
