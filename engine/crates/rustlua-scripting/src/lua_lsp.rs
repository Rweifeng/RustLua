//! 把 Lua 引擎 API 的类型提示**装进具体工程**（LuaLS `---@meta`）。
//!
//! 事实源不在这里：类型定义文件 `api_surface_snapshot/rustlua_api.d.lua` 由
//! `node scripts/docgen/gen-lua-api-docs.js` 从运行期 API 面基线生成，与 `doc/*.md` 文档桩同源、
//! 同一次运行（见该脚本头注与 docs/61）。本模块只做一件事——把那份编译进二进制的文本落到工程里
//! 正确的位置，并保证 LuaLS 真的会读它。
//!
//! 为什么需要这一层：脚本作者的外部编辑器（VS Code / Neovim / Helix + lua-language-server）只在
//! **工作区**里看得见定义文件。工程根放一份 `.luarc.json` 把 `.cache/` 挂进 `workspace.library`，
//! `audio.plat` 这种打错的名字才会在敲的时候亮出来、真实名字才会出现在补全里。
//!
//! 为什么落点是 `.cache/`：那份文件是**派生产物**（引擎版本一变就该重生成），不该进工程的版本库；
//! `.cache/` 已经在忽略列表里，跟存档同处一地。
//!
//! `.luarc.json` 的处理刻意保守：只在它是**合法 JSON 对象**时才动手（缺 `workspace.library` 就补，
//! 已经有了就不碰），坏文件一律报错返回而不是"顺手修好"——那是用户自己写的配置，覆盖它等于丢配置。

use std::path::{Path, PathBuf};

/// 生成好的 LuaLS 类型定义（`---@meta`）。随源码提交，因此二进制里带的那份与仓库里的基线同源。
pub const LUA_API_META: &str = include_str!("api_surface_snapshot/rustlua_api.d.lua");

/// 类型定义在工程里的相对落点（`.cache/` 被版本库忽略，派生产物不该污染工程）。
pub const META_DIR: &str = ".cache";

/// 类型定义文件名。
pub const META_FILE_NAME: &str = "rustlua_api.d.lua";

/// `.luarc.json` 里 `workspace.library` 用来认领类型定义所在目录的那一项（相对配置文件目录）。
const LIBRARY_ENTRY: &str = ".cache";

/// 一次安装的结果，宿主（编辑器状态栏 / `--check`）直接拿去显示。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetaInstall {
    /// 写出的类型定义文件绝对路径。
    pub meta_path: PathBuf,
    /// `.luarc.json` 路径（工程根下）。
    pub luarc_path: PathBuf,
    /// 类型定义里有多少个函数声明（从文本数出来，不另存一份计数）。
    pub functions: usize,
    /// 本次对 `.luarc.json` 做了什么。
    pub luarc: LuarcState,
}

/// `.luarc.json` 的处理结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LuarcState {
    /// 原先没有，写了一份只挂 `.cache` 的最小配置。
    Created,
    /// 原先有配置但没挂类型目录：补上了 `workspace.library`，其余键原样保留。
    LibraryAdded,
    /// 已经挂好了：只刷新类型定义文件。
    AlreadyConfigured,
}

/// 数一下类型定义里的函数声明（两种形状都算：`function 模块.名(...) end`，以及名字撞 Lua
/// 保留字时的 `模块["名"] = function(...) end`——实测只有 `map.goto` 走后者）。
#[must_use]
pub fn count_functions(meta: &str) -> usize {
    meta.lines()
        .filter(|line| {
            line.ends_with(" end")
                && (line.starts_with("function ") || line.contains("= function("))
        })
        .count()
}

/// 把类型定义装进 `project_root`：写 `.cache/rustlua_api.d.lua`，并保证 `.luarc.json` 挂着它。
///
/// 失败返回**给用户看的那句话**（含原因）；此时 `.luarc.json` 一定没被改动（顺序是先写定义文件、
/// 再动配置，配置解析失败就原样留着）。
pub fn install_lua_lsp_meta(project_root: &Path) -> Result<MetaInstall, String> {
    let cache_dir = project_root.join(META_DIR);
    std::fs::create_dir_all(&cache_dir)
        .map_err(|error| format!("创建 {} 失败：{error}", cache_dir.display()))?;
    let meta_path = cache_dir.join(META_FILE_NAME);
    std::fs::write(&meta_path, LUA_API_META)
        .map_err(|error| format!("写 {} 失败：{error}", meta_path.display()))?;

    let luarc_path = project_root.join(".luarc.json");
    let luarc = configure_luarc(&luarc_path)?;
    Ok(MetaInstall {
        functions: count_functions(LUA_API_META),
        meta_path,
        luarc_path,
        luarc,
    })
}

/// 让 `.luarc.json` 的 `workspace.library` 里出现 [LIBRARY_ENTRY]；返回本次做了什么。
fn configure_luarc(path: &Path) -> Result<LuarcState, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write_luarc(path, &default_luarc()).map(|_| LuarcState::Created)?;
            return Ok(LuarcState::Created);
        }
        Err(error) => return Err(format!("读 {} 失败：{error}", path.display())),
    };
    let mut root: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
        format!(
            "{} 不是合法 JSON（{error}），已保留原文件；请手工把 \".cache\" 加进 workspace.library",
            path.display()
        )
    })?;
    let object = root
        .as_object_mut()
        .ok_or_else(|| format!("{} 不是 JSON 对象，已保留原文件", path.display()))?;
    let library = object
        .entry("workspace")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            format!(
                "{} 的 `workspace` 不是对象，已保留原文件；请手工把 \".cache\" 加进 workspace.library",
                path.display()
            )
        })?
        .entry("library")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .ok_or_else(|| {
            format!(
                "{} 的 `workspace.library` 不是数组，已保留原文件；请手工把 \".cache\" 加进去",
                path.display()
            )
        })?;
    if library
        .iter()
        .any(|entry| entry.as_str().is_some_and(|value| value == LIBRARY_ENTRY))
    {
        return Ok(LuarcState::AlreadyConfigured);
    }
    library.push(serde_json::json!(LIBRARY_ENTRY));
    write_luarc(path, &root.to_string()).map(|_| LuarcState::LibraryAdded)
}

/// 最小可用配置：只挂类型定义目录。其余（运行时版本、诊断开关）留给用户自己写。
fn default_luarc() -> String {
    serde_json::json!({
        "//": "由 RustLua 编辑器生成：workspace.library 指向引擎 API 类型定义所在目录。删除本文件不影响运行。",
        "workspace": { "library": [LIBRARY_ENTRY] }
    })
    .to_string()
}

fn write_luarc(path: &Path, payload: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建 {} 失败：{error}", parent.display()))?;
    }
    // 配置文件很小，且紧接着一次成功写入就完成；这里不做临时文件 + rename 的原子换，
    // 因为**读侧只在编辑器启动时读一次**，写坏了也由上层的"解析失败就不动"兜住。
    std::fs::write(path, format!("{payload}\n"))
        .map_err(|error| format!("写 {} 失败：{error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{LIBRARY_ENTRY, LUA_API_META, LuarcState, count_functions, install_lua_lsp_meta};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// 每个用例一个临时工程根（与仓库里那些 `rustlua-m*` 临时目录同一命名法）。
    fn project_dir(tag: &str) -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("rustlua-m79-lsp-{tag}-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp project dir");
        dir
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).expect("file written by the installer")
    }

    /// 类型定义必须是**能解析的 Lua**，且每个函数都声明了返回类型：
    /// 少一条注解，`local h = audio.play(x)` 就会被 LuaLS 判成拿到 nil。
    #[test]
    fn shipped_meta_is_valid_lua_and_types_every_return() {
        let runtime = mlua::Lua::new();
        runtime
            .load(LUA_API_META)
            .into_function()
            .expect("rustlua_api.d.lua 必须能被 Lua 5.4 解析");
        let declarations = count_functions(LUA_API_META);
        assert!(declarations > 200, "类型定义只剩 {declarations} 个函数？");
        let annotated = LUA_API_META
            .lines()
            .filter(|line| *line == "---@return any" || line.starts_with("---@type fun("))
            .count();
        assert_eq!(
            annotated, declarations,
            "每个函数声明都要带返回类型（`---@return any`，或关键字名那条的 `---@type fun(...)`）"
        );
        assert!(
            LUA_API_META.starts_with("---@meta"),
            "第一行必须是 `---@meta`，否则 LuaLS 会把它当普通脚本进运行时"
        );
    }

    /// **零参数**与**参数未知**是两种声明，不许合并：前者发 `x()`（LuaLS 能对多写的实参报
    /// "不该有实参"），后者发 `x(...)` + `---@param ... any`（不检查实参）。合成一个形状时，
    /// `physics.events(1)` 与 `physics.set_velocity(id, comp)` 会双双变成假诊断。
    #[test]
    fn zero_arity_and_unknown_arity_stay_distinct() {
        let lines: Vec<&str> = LUA_API_META.lines().collect();
        let mut zero = 0;
        let mut unknown = 0;
        for (index, line) in lines.iter().enumerate() {
            if !line.starts_with("function ") {
                continue;
            }
            let before = lines.get(index.saturating_sub(1)).copied().unwrap_or("");
            if line.ends_with("() end") {
                zero += 1;
                assert_eq!(before, "---@return any", "{line} 的上一条不该是参数注解");
            } else if line.ends_with("(...) end") {
                unknown += 1;
                assert_eq!(before, "---@return any", "{line} 的上一条应是返回注解");
                assert_eq!(
                    lines.get(index.saturating_sub(2)).copied().unwrap_or(""),
                    "---@param ... any",
                    "{line} 应带未知参数注解"
                );
            }
        }
        assert!(zero > 20, "零参数声明只剩 {zero} 个？");
        assert!(
            unknown > 0 && unknown < 20,
            "未知参数声明 {unknown} 个：突然变多说明签名解析器坏了，别把具名参数降级成 `...`"
        );
    }

    /// 类型定义覆盖 API 面基线里的**每一张表与每一个函数**（漏一个 = 那个名字的补全不存在）。
    #[test]
    fn meta_covers_every_table_in_the_surface_baseline() {
        let baseline: serde_json::Value =
            serde_json::from_str(include_str!("api_surface_snapshot/lua_api_surface.json"))
                .expect("基线 JSON 解析");
        let tables = baseline["tables"].as_array().expect("tables 应为数组");
        assert!(!tables.is_empty());
        for table in tables {
            let name = table["name"].as_str().expect("模块名");
            assert!(
                LUA_API_META.contains(&format!("{name} = {{}}")),
                "类型定义里没有 `{name}` 这张表"
            );
            for func in table["funcs"].as_array().expect("funcs 应为数组") {
                let func = func.as_str().expect("函数名");
                let dotted = format!("function {name}.{func}(");
                let bracketed = format!("{name}[\"{func}\"] = function(");
                assert!(
                    LUA_API_META.contains(&dotted) || LUA_API_META.contains(&bracketed),
                    "类型定义里缺 `{name}.{func}`"
                );
            }
        }
    }

    #[test]
    fn install_writes_meta_and_creates_a_minimal_luarc() {
        let root = project_dir("fresh");
        let install = install_lua_lsp_meta(&root).expect("安装成功");
        assert_eq!(install.meta_path, root.join(".cache/rustlua_api.d.lua"));
        assert_eq!(read(&install.meta_path), LUA_API_META);
        assert_eq!(install.luarc, LuarcState::Created);
        let config: serde_json::Value = serde_json::from_str(&read(&install.luarc_path)).unwrap();
        assert_eq!(
            config["workspace"]["library"].as_array().unwrap().len(),
            1,
            "最小配置只该挂一个目录"
        );
        assert_eq!(
            config["workspace"]["library"][0].as_str(),
            Some(LIBRARY_ENTRY)
        );
        assert!(
            install.functions > 200,
            "状态栏要报的函数数：{}",
            install.functions
        );
    }

    /// 已有 `.luarc.json` 时**只补自己那一项**：用户写着的键一个都不许丢。
    #[test]
    fn install_keeps_user_settings_and_only_adds_the_library_entry() {
        let root = project_dir("merge");
        let user = r#"{ "runtime": { "version": "Lua 5.4" }, "workspace": { "library": ["lib"] }, "diagnostics": { "globals": ["vec2"] } }"#;
        std::fs::write(root.join(".luarc.json"), user).expect("seed luarc");
        let install = install_lua_lsp_meta(&root).expect("安装成功");
        assert_eq!(install.luarc, LuarcState::LibraryAdded);
        let config: serde_json::Value = serde_json::from_str(&read(&install.luarc_path)).unwrap();
        assert_eq!(config["runtime"]["version"].as_str(), Some("Lua 5.4"));
        assert_eq!(config["diagnostics"]["globals"][0].as_str(), Some("vec2"));
        let library = config["workspace"]["library"].as_array().unwrap();
        assert_eq!(library.len(), 2, "{library:?}");
        assert_eq!(library[0].as_str(), Some("lib"), "用户原有的项要保持在前面");
        assert_eq!(library[1].as_str(), Some(LIBRARY_ENTRY));
    }

    /// 已经挂好了就不要再动配置文件（第二次安装 = 只刷新类型定义）。
    #[test]
    fn second_install_is_a_no_op_for_the_config() {
        let root = project_dir("idempotent");
        let first = install_lua_lsp_meta(&root).expect("first");
        let before = read(&first.luarc_path);
        let second = install_lua_lsp_meta(&root).expect("second");
        assert_eq!(second.luarc, LuarcState::AlreadyConfigured);
        assert_eq!(read(&second.luarc_path), before, "配置文件不该被改写");
    }

    /// 坏配置：报错、原文件不动、但类型定义照样落盘（下次手改完就能用）。
    #[test]
    fn a_broken_config_is_reported_and_left_untouched() {
        let root = project_dir("broken");
        std::fs::write(root.join(".luarc.json"), "{ not json").expect("seed broken");
        let error = install_lua_lsp_meta(&root).expect_err("坏配置必须报错");
        assert!(error.contains(".luarc.json"), "{error}");
        assert!(
            error.contains("已保留原文件"),
            "必须说清没动用户的文件：{error}"
        );
        assert_eq!(read(&root.join(".luarc.json")), "{ not json");
        assert!(root.join(".cache/rustlua_api.d.lua").is_file());
    }
}
