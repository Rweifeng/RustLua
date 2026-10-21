//! `asset` / `data` / `map`（数据面）的 Lua 绑定（`docs/62` P1 ⑤-4c）。
//!
//! ## 为什么在这里
//!
//! 这三张表读写的都是**工程数据**：`asset.*` 登记资源句柄，`data.*` 读数据表并按 schema
//! 校验，`map.state/schema/spawn/set_spawn` 把分地图持久状态写进
//! [`SaveStore`]——存档的格式与数据本来就在本 crate。⑤-4a/4b 已把状态收进
//! [`WorldDataState`] 并换成 `Handle`，这一步把绑定一起搬过来，`rustlua-scripting` 只交出
//! 三个能力单元（世界数据、存档、工程根），不再知道数据表怎么读、schema 怎么校验。
//!
//! ## 边界的划法（与旧实现的唯一差别）
//!
//! `scene.*` 与 `map.goto/go/enter/current` 留在脚本层：它们碰的是**场景切换**
//! （`scene_pending_load` / 当前场景名），不是数据；`map` 表由脚本层建好、本模块往里填数据面
//! 函数（与 `ui` 分三段填表同一个手法）。Lua 面的函数名、参数、返回值逐字未变
//! （`api_surface_snapshot` 零 diff）。
//!
//! ## 借用口径
//!
//! 每次调用只借 [`WorldDataState`] / [`SaveStore`] 之一，且不跨 Lua 调用持有借用——
//! 重入时报的是具体能力单元（`world_data` / `save`），而不是"脚本状态被重入"。

use std::path::{Path, PathBuf};

use mlua::{Lua, Result as LuaResult, Table, Value};
use rustlua_lua_api::Handle;
use rustlua_lua_api::ProjectRoot;
use rustlua_lua_api::api_error;
use rustlua_lua_api::values::{json_to_lua_value, lua_value_to_json};

use crate::save_store::SaveStore;
use crate::world_data_state::WorldDataState;

/// 装 `asset` / `data` 两张全局表，并把数据面的函数填进调用方建好的 `map` 表。
///
/// `map` 由宿主（`rustlua-scripting`）创建：它还要在同一张表上装 `goto/go/enter/current`
/// 这些**场景切换**函数。Lua 表是引用语义，克隆只是多一个句柄。
pub fn install(
    lua: &Lua,
    map: &Table,
    world_data: &Handle<WorldDataState>,
    save: &Handle<SaveStore>,
    project_root: &Handle<ProjectRoot>,
) -> LuaResult<()> {
    install_map_data(lua, map, world_data, save)?;

    let asset = lua.create_table()?;
    let wd = world_data.clone();
    asset.set(
        "load",
        lua.create_function(move |_, path: String| {
            let token = format!("asset:{}", path);
            wd.borrow_mut()?.assets.insert(path, token.clone());
            Ok(token)
        })?,
    )?;
    let wd = world_data.clone();
    asset.set(
        "release",
        lua.create_function(move |_, token: String| {
            wd.borrow_mut()?.assets.retain(|_, value| value != &token);
            Ok(())
        })?,
    )?;
    lua.globals().set("asset", asset)?;

    install_data(lua, world_data, project_root)?;
    Ok(())
}

/// `map.state` / `map.state_clear` / `map.schema` / `map.spawn` / `map.set_spawn`。
///
/// 这些函数只碰存档与地图状态 schema，不碰场景切换——切场景的 `goto/go/enter` 在脚本层。
fn install_map_data(
    lua: &Lua,
    map: &Table,
    world_data: &Handle<WorldDataState>,
    save: &Handle<SaveStore>,
) -> LuaResult<()> {
    let wd = world_data.clone();
    let store = save.clone();
    map.set(
        "state",
        lua.create_function(move |lua, (name, value): (String, Option<Value>)| {
            let key = format!("__mapstate:{name}");
            if let Some(value) = value {
                let json = lua_value_to_json(value)?;
                // B-3：注册 `map.schema(name, spec)` 后，写入会先校验，
                // 让错误在发生处暴露，而不是延迟到读档时才发现。
                let schema = wd.borrow()?.map_state_schemas.get(&name).cloned();
                if let Some(schema) = schema
                    && let Some(error) = rustlua_ecs::data_schema::validate(
                        &json,
                        &schema,
                        &format!("map.state {name}"),
                    )
                {
                    return Err(api_error(error));
                }
                let encoded =
                    serde_json::to_string(&json).map_err(|error| api_error(error.to_string()))?;
                store.borrow_mut()?.insert(key, encoded);
                return Ok(Value::Boolean(true));
            }
            let Some(encoded) = store.borrow()?.get(&key).cloned() else {
                return Ok(Value::Nil);
            };
            let json = serde_json::from_str(&encoded)
                .map_err(|error| api_error(format!("map.state {name} is corrupt: {error}")))?;
            json_to_lua_value(lua, &json)
        })?,
    )?;

    let store = save.clone();
    map.set(
        "state_clear",
        lua.create_function(move |_, name: Option<String>| {
            let before = store.borrow()?.len();
            match &name {
                Some(name) => {
                    let key = format!("__mapstate:{name}");
                    store.borrow_mut()?.remove(&key);
                }
                None => store
                    .borrow_mut()?
                    .data_mut()
                    .retain(|key, _| !key.starts_with("__mapstate:")),
            }
            Ok(before - store.borrow()?.len())
        })?,
    )?;

    let wd = world_data.clone();
    map.set(
        "schema",
        lua.create_function(move |_, (name, spec): (String, Option<Value>)| {
            if let Some(spec) = spec {
                let json = lua_value_to_json(spec)?;
                wd.borrow_mut()?.map_state_schemas.insert(name, json);
            } else {
                wd.borrow_mut()?.map_state_schemas.remove(&name);
            }
            Ok(())
        })?,
    )?;

    let store = save.clone();
    map.set(
        "spawn",
        lua.create_function(move |lua, ()| {
            // 一次窄借用同时取两个键：`Handle::borrow()` 的 guard 不能跨 `let...else`
            // 存活（那是临时值），所以先 clone 出字符串再解析。
            let (x, y) = {
                let store = store.borrow()?;
                (
                    store.get("__map:spawn:x").cloned(),
                    store.get("__map:spawn:y").cloned(),
                )
            };
            let (Some(x), Some(y)) = (x, y) else {
                return Ok(Value::Nil);
            };
            let Ok(x) = x.parse::<f32>() else {
                return Ok(Value::Nil);
            };
            let Ok(y) = y.parse::<f32>() else {
                return Ok(Value::Nil);
            };
            let result = lua.create_table()?;
            result.set("x", x)?;
            result.set("y", y)?;
            Ok(Value::Table(result))
        })?,
    )?;

    let store = save.clone();
    map.set(
        "set_spawn",
        lua.create_function(move |_, (x, y): (f32, f32)| {
            if !x.is_finite() || !y.is_finite() {
                return Err(api_error("map.set_spawn must be finite"));
            }
            store
                .borrow_mut()?
                .insert("__map:spawn:x".to_owned(), x.to_string());
            store
                .borrow_mut()?
                .insert("__map:spawn:y".to_owned(), y.to_string());
            Ok(())
        })?,
    )?;
    Ok(())
}

/// `data.load` / `data.reload` / `data.cache_clear` / `data.schema`。
fn install_data(
    lua: &Lua,
    world_data: &Handle<WorldDataState>,
    project_root: &Handle<ProjectRoot>,
) -> LuaResult<()> {
    let data = lua.create_table()?;

    let wd = world_data.clone();
    let root = project_root.clone();
    data.set(
        "load",
        lua.create_function(move |lua, path: String| {
            // 与下沉前同序：先确认有工程根，再看缓存。少了第一步会让"没有工程根"
            // 在缓存命中时被静默吞掉（宿主配置错误更难发现）。
            let root_path = project_root_path(&root, "load")?;
            if let Some(cached) = wd.borrow()?.data_cache.get(&path).cloned() {
                return json_to_lua_value(lua, &cached);
            }
            let json = read_data_table(&root_path, &path, "load")?;
            validate_data_schema(&wd, &path, &json, "load")?;
            wd.borrow_mut()?.data_cache.insert(path, json.clone());
            json_to_lua_value(lua, &json)
        })?,
    )?;

    let wd = world_data.clone();
    let root = project_root.clone();
    data.set(
        "reload",
        lua.create_function(move |lua, path: String| {
            wd.borrow_mut()?.data_cache.remove(&path);
            let root_path = project_root_path(&root, "reload")?;
            let json = read_data_table(&root_path, &path, "reload")?;
            validate_data_schema(&wd, &path, &json, "reload")?;
            wd.borrow_mut()?.data_cache.insert(path, json.clone());
            json_to_lua_value(lua, &json)
        })?,
    )?;

    let wd = world_data.clone();
    data.set(
        "cache_clear",
        lua.create_function(move |_, ()| {
            wd.borrow_mut()?.data_cache.clear();
            Ok(())
        })?,
    )?;

    let wd = world_data.clone();
    data.set(
        "schema",
        lua.create_function(move |_, (path, spec): (String, Option<Value>)| {
            if let Some(spec) = spec {
                let json = lua_value_to_json(spec)?;
                wd.borrow_mut()?.schema_defs.insert(path.clone(), json);
                // A schema change must not be masked by a stale cached table.
                wd.borrow_mut()?.data_cache.remove(&path);
            } else {
                wd.borrow_mut()?.schema_defs.remove(&path);
            }
            Ok(())
        })?,
    )?;

    lua.globals().set("data", data)?;
    Ok(())
}

/// 取工程根；未设置时报 `data.<verb> requires a project root`（措辞与旧绑定逐字一致）。
fn project_root_path(project_root: &Handle<ProjectRoot>, verb: &str) -> LuaResult<PathBuf> {
    project_root
        .borrow()?
        .path()
        .map(Path::to_path_buf)
        .ok_or_else(|| api_error(format!("data.{verb} requires a project root")))
}

/// 读一张数据表：经 [`rustlua_infra::vfs`]（wasm 上 provider 才是真相）+ `pack::decode`
/// （发布包里同一份 .json 是压缩/加密过的），再解析 JSON。
fn read_data_table(project_root: &Path, path: &str, verb: &str) -> LuaResult<serde_json::Value> {
    let file_path = project_root.join(path);
    let bytes = rustlua_infra::vfs::read(&file_path)
        .map_err(|error| api_error(format!("data.{verb} {}: {error}", file_path.display())))?;
    let decoded = rustlua_infra::pack::decode(&bytes)
        .map_err(|error| api_error(format!("data.{verb} {}: {error}", file_path.display())))?;
    let text = String::from_utf8_lossy(&decoded);
    serde_json::from_str(&text)
        .map_err(|error| api_error(format!("data.{verb} {}: {error}", file_path.display())))
}

/// Validates a freshly loaded data table against its registered schema
/// (`data.schema`), rejecting bad content with a located error instead of
/// silently caching it.
fn validate_data_schema(
    world_data: &Handle<WorldDataState>,
    path: &str,
    json: &serde_json::Value,
    verb: &str,
) -> LuaResult<()> {
    let schema = world_data.borrow()?.schema_defs.get(path).cloned();
    if let Some(schema) = schema
        && let Some(error) = rustlua_ecs::data_schema::validate(json, &schema, path)
    {
        return Err(api_error(format!(
            "data.{verb} {path}: schema violation: {error}"
        )));
    }
    Ok(())
}

/// 测试与调用方共用的临时工程根：`<tmp>/rustlua-world-data-<tag>-<pid>`。
///
/// 不用 `tempfile` 是为了不给 gameplay 加一个只为测试存在的依赖（与 `save_lua` 的
/// "零 dev-dependency"口径一致）。
#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn temp_project(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("rustlua-world-data-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(root.join("data")).unwrap();
        root
    }

    /// `asset` / `data` 两张表的 Lua 面 + `map` 的数据面函数就这些：搬家之后一个都不能少
    /// （`api_surface_snapshot` 盯全量，这条盯"这一步搬全了"）。
    const ASSET_NAMES: [&str; 2] = ["load", "release"];
    const DATA_NAMES: [&str; 4] = ["load", "reload", "cache_clear", "schema"];
    const MAP_NAMES: [&str; 5] = ["state", "state_clear", "schema", "spawn", "set_spawn"];

    struct Fixture {
        lua: Lua,
        map: Table,
        world_data: Handle<WorldDataState>,
        save: Handle<SaveStore>,
        root: Handle<ProjectRoot>,
    }

    fn installed() -> Fixture {
        let lua = Lua::new();
        let map = lua.create_table().unwrap();
        let world_data = WorldDataState::handle();
        let save = SaveStore::handle();
        let root = ProjectRoot::handle();
        install(&lua, &map, &world_data, &save, &root).unwrap();
        lua.globals().set("map", &map).unwrap();
        Fixture {
            lua,
            map,
            world_data,
            save,
            root,
        }
    }

    #[test]
    fn every_data_function_is_installed() {
        let fixture = installed();
        let asset: Table = fixture.lua.globals().get("asset").unwrap();
        let data: Table = fixture.lua.globals().get("data").unwrap();
        for (table, names) in [
            (&asset, ASSET_NAMES.as_slice()),
            (&data, DATA_NAMES.as_slice()),
            (&fixture.map, MAP_NAMES.as_slice()),
        ] {
            for name in names {
                let value: Value = table.get(*name).unwrap();
                assert!(
                    matches!(value, Value::Function(_)),
                    "`{name}` 缺失或不是函数：{value:?}"
                );
            }
        }
    }

    #[test]
    fn data_tables_validate_against_schema_and_cache_round_trips() {
        let fixture = installed();
        let root = temp_project("data");
        std::fs::write(
            root.join("data/items.json"),
            r#"{"tomato":{"name":"Tomato","stack":8}}"#,
        )
        .unwrap();
        fixture.root.borrow_mut().unwrap().set(root.clone());

        fixture
            .lua
            .load(
                r#"
                data.schema("data/items.json",
                    { ["$kind"] = "map", values = { name = "string", stack = "integer" } })
                local items = data.load("data/items.json")
                assert(items.tomato.name == "Tomato" and items.tomato.stack == 8)
                "#,
            )
            .exec()
            .unwrap();
        // 缓存确实落在能力单元里（不是 Lua 侧影子表）。
        assert_eq!(fixture.world_data.borrow().unwrap().data_cache.len(), 1);

        // 改盘上内容：缓存命中时读到的仍是旧值，reload 才看见新值。
        std::fs::write(
            root.join("data/items.json"),
            r#"{"tomato":{"name":"Cherry","stack":3}}"#,
        )
        .unwrap();
        fixture
            .lua
            .load(
                r#"
                assert(data.load("data/items.json").tomato.name == "Tomato", "缓存命中")
                assert(data.reload("data/items.json").tomato.name == "Cherry", "reload 绕过缓存")
                "#,
            )
            .exec()
            .unwrap();

        // schema 违规必须在 load 处报「data.reload <path>: schema violation」，而不是静默缓存。
        std::fs::write(
            root.join("data/items.json"),
            r#"{"tomato":{"stack":"big"}}"#,
        )
        .unwrap();
        let error = fixture
            .lua
            .load(r#"data.reload("data/items.json")"#)
            .exec()
            .unwrap_err()
            .to_string();
        assert!(error.contains("schema violation"), "得到：{error}");
        // 违规内容没有进缓存。
        fixture
            .lua
            .load(r#"assert(data.cache_clear() == nil)"#)
            .exec()
            .unwrap();
        assert!(fixture.world_data.borrow().unwrap().data_cache.is_empty());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn map_state_round_trips_validates_and_clears() {
        let fixture = installed();
        fixture
            .lua
            .load(
                r#"
                map.schema("farm", { ["$kind"] = "object",
                    fields = { version = "integer", tiles = { ["$kind"] = "array", items = "integer" } },
                    required = { "version", "tiles" } })
                assert(map.state("farm", { version = 1, tiles = { 0, 3 } }))
                local state = map.state("farm")
                assert(state.version == 1 and state.tiles[2] == 3)
                local ok, err = pcall(function() map.state("farm", { version = "x" }) end)
                assert(not ok and tostring(err):find("map.state farm"), tostring(err))
                map.set_spawn(120, 80)
                local spawn = map.spawn()
                assert(spawn.x == 120 and spawn.y == 80)
                "#,
            )
            .exec()
            .unwrap();
        // 状态确实写进了存档能力单元（`__mapstate:*` 与 `__map:spawn:*` 同一份）。
        {
            let store = fixture.save.borrow().unwrap();
            assert!(store.get("__mapstate:farm").is_some());
            assert_eq!(store.get("__map:spawn:x").map(String::as_str), Some("120"));
        }
        fixture
            .lua
            .load(
                r#"
                assert(map.state("other", { anything = true }))
                local removed = map.state_clear()
                assert(removed >= 2, "state_clear 应报告清掉的键数：" .. tostring(removed))
                assert(map.state("farm") == nil and map.state("other") == nil)
                map.state("single", { x = 1 })
                map.state_clear("single")
                assert(map.state("single") == nil)
                "#,
            )
            .exec()
            .unwrap();
    }

    #[test]
    fn asset_tokens_are_tracked_in_the_handle() {
        let fixture = installed();
        fixture
            .lua
            .load(
                r#"
                assert(asset.load("art/hero.png") == "asset:art/hero.png")
                asset.release("asset:art/hero.png")
                local again = asset.load("art/hero.png")
                asset.release(again)
                "#,
            )
            .exec()
            .unwrap();
        assert!(
            fixture.world_data.borrow().unwrap().assets.is_empty(),
            "release 之后句柄表应为空"
        );
    }

    #[test]
    fn data_load_without_a_project_root_names_the_requirement() {
        let fixture = installed();
        let error = fixture
            .lua
            .load(r#"data.load("data/items.json")"#)
            .exec()
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("requires a project root"),
            "报错应点名缺工程根：{error}"
        );
    }
}
