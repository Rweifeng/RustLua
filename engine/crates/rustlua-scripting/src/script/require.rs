//! 脚本源码读取与 require 装配：项目脚本读取（含发布包压缩/加密解码）、
//! 文件指纹（热重载比对）、原子写盘、web 侧 require 安装。
//!
//! 桌面与 wasm 的分歧只在这里收敛：读路径一律经 `rustlua_infra::vfs`（wasm 没有文件系统，
//! 由宿主注入 provider），写路径仍在真实文件系统（存档、设置、原子写）。

use std::path::{Path, PathBuf};

use mlua::{Lua, Table, Value};

use super::error::{LuaBridgeError, ScriptFingerprint};

pub(super) fn file_fingerprint(
    path: &Path,
    source: &str,
) -> Result<ScriptFingerprint, LuaBridgeError> {
    use std::hash::{Hash, Hasher};
    let meta = rustlua_infra::vfs::metadata(path).map_err(|error| LuaBridgeError::Load {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    Ok(ScriptFingerprint {
        modified: meta.modified,
        length: meta.len,
        hash: hasher.finish(),
    })
}

/// 读取项目脚本源码，支持发布包的压缩/加密（明文原样返回）。
/// 桌面走 `std::fs`；wasm32-unknown-unknown 走宿主注入的虚拟文件系统（`core::vfs`）。
/// `pub`：wasm Lua 桥（`game_web::lua_bridge::lua_execute_main`）需要它在**既有状态**
/// 上读取+执行 main.lua（`execute_project_script` 的 `load_candidate` 会新建 Lua
/// 状态、丢弃已注入的宿主补丁，web 流程不可用）。
pub fn read_project_script(path: &Path) -> Result<String, LuaBridgeError> {
    let bytes = rustlua_infra::vfs::read(path).map_err(|error| LuaBridgeError::Load {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    let decoded = rustlua_infra::pack::decode(&bytes).map_err(|error| LuaBridgeError::Load {
        path: path.to_path_buf(),
        reason: error,
    })?;
    Ok(String::from_utf8_lossy(&decoded).into_owned())
}

/// 在 Lua VM 上安装支持压缩/加密的 require 搜索器：
/// 读取模块文件字节 → 解码 → 编译，供 `require` 加载发布包中的模块。
pub(super) fn install_encoded_require_searcher(lua: &Lua, script_dir: PathBuf) -> mlua::Result<()> {
    let searcher = lua.create_function(move |lua, module: String| {
        let relative = module.replace('.', std::path::MAIN_SEPARATOR_STR);
        let file_path = script_dir.join(format!("{relative}.lua"));
        if !rustlua_infra::vfs::is_file(&file_path) {
            return Ok(Value::Nil);
        }
        let bytes = rustlua_infra::vfs::read(&file_path).map_err(mlua::Error::external)?;
        let decoded = rustlua_infra::pack::decode(&bytes).map_err(mlua::Error::external)?;
        let source = String::from_utf8_lossy(&decoded).into_owned();
        let chunk_name = format!("scripts/{module}.lua");
        let chunk = lua.load(&source).set_name(chunk_name).into_function()?;
        Ok(Value::Function(chunk))
    })?;
    let package: Table = lua.globals().get("package")?;
    let searchers: Table = package.get("searchers")?;
    searchers.raw_insert(1, searcher)?;
    Ok(())
}

/// 在**既有** Lua 状态上配置 require（web 侧 `lua_execute_main` 用）：
/// mlua 默认 require 搜索器走 C 文件 API，wasm 下没有文件系统，必须装
/// vfs provider 支持的编码搜索器。`execute_project_script` 的 `load_candidate`
/// 会新建 Lua 状态（丢弃已注入补丁），因此 web 流程不能复用，只能这样显式补装。
pub fn install_web_require(lua: &Lua, script_dir: &Path) -> mlua::Result<()> {
    install_encoded_require_searcher(lua, script_dir.to_path_buf())
}

/// Writes `text` to `path` atomically (temp file + rename), like scene saves.
pub(super) fn atomic_write(path: &Path, text: &str) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(temp, path)?;
    Ok(())
}
