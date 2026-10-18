//! Virtual file system for project resources.
//!
//! On `wasm32-unknown-unknown` there is no `std::fs`; the engine's Lua bridge
//! loads project resources (.lua scripts, `require`d modules, `data.load`
//! JSON files) from disk on desktop. A host may install a read-only **resource
//! provider** (`set_provider`) at startup; when installed, every read goes
//! through it first. On desktop the provider is optional — without it, reads
//! hit the real filesystem exactly as before (zero behavior change); with it,
//! it takes priority (used by tests and by the Web host). On wasm the provider
//! is required (there is no filesystem).
//!
//! Storage differs by platform: desktop uses a `OnceLock` (the provider must
//! be `Send + Sync`, since any thread may read); wasm is single-threaded, so
//! the provider lives in a `thread_local!` and may capture non-`Send` handles
//! such as `js_sys::Function`.
//!
//! The provider returns **undecoded** bytes (identical to what `std::fs::read`
//! would return): compression/encryption is still handled by
//! `rustlua_infra::pack::decode`.

use std::io;
use std::path::{Path, PathBuf};

/// Filesystem-like metadata exposed to the Lua bridge (hot-reload uses it).
pub struct Meta {
    pub modified: Option<std::time::SystemTime>,
    pub len: u64,
}

/// 桌面端资源提供者的类型别名（签名较长，收敛重复，见 clippy type_complexity）。
#[cfg(not(target_arch = "wasm32"))]
type DesktopProvider = dyn Fn(&Path) -> Option<Vec<u8>> + Send + Sync;

/// 桌面前缀列举器：容器要能"列条目"，否则发布形态下 `scripts/**` 在磁盘上不存在，
/// 枚举类校验会静默跳过整棵树（`docs/67` §8 第 2 项）。
#[cfg(not(target_arch = "wasm32"))]
type DesktopLister = dyn Fn(&Path) -> Option<Vec<(PathBuf, bool)>> + Send + Sync;

/// Install the host resource reader. Must be called once before any Lua
/// script load; a second call fails.
///
/// Desktop signature requires `Send + Sync` (any thread may read); wasm is
/// single-threaded, so the provider may capture non-`Send` handles such as
/// `js_sys::Function`.
#[cfg(not(target_arch = "wasm32"))]
pub fn set_provider(provider: Box<DesktopProvider>) -> Result<(), &'static str> {
    backend::set(provider)
}

/// Install the host resource reader. Must be called once before any Lua
/// script load; a second call fails.
///
/// Desktop signature requires `Send + Sync` (any thread may read); wasm is
/// single-threaded, so the provider may capture non-`Send` handles such as
/// `js_sys::Function`.
#[cfg(target_arch = "wasm32")]
pub fn set_provider(
    provider: impl Fn(&Path) -> Option<Vec<u8>> + 'static,
) -> Result<(), &'static str> {
    backend::set(Box::new(provider))
}

/// Install the host directory lister（资源包的"列条目"能力）。
///
/// 与 provider 同一条纪律：进程级一次安装，第二次报错。桌面用 `Send + Sync`（可能与
/// provider 在不同线程被调用），wasm 单线程允许捕获非 `Send` 句柄。
#[cfg(not(target_arch = "wasm32"))]
pub fn set_lister(lister: Box<DesktopLister>) -> Result<(), &'static str> {
    backend::set_lister(lister)
}

/// Install the host directory lister（wasm 形态）。
#[cfg(target_arch = "wasm32")]
pub fn set_lister(
    lister: impl Fn(&Path) -> Option<Vec<(PathBuf, bool)>> + 'static,
) -> Result<(), &'static str> {
    backend::set_lister(Box::new(lister))
}

#[cfg(not(target_arch = "wasm32"))]
mod backend {
    use super::*;
    use std::sync::OnceLock;

    static PROVIDER: OnceLock<Box<DesktopProvider>> = OnceLock::new();
    static LISTER: OnceLock<Box<DesktopLister>> = OnceLock::new();

    pub fn set(provider: Box<DesktopProvider>) -> Result<(), &'static str> {
        PROVIDER
            .set(provider)
            .map_err(|_| "vfs: provider already installed")
    }

    pub fn set_lister(lister: Box<DesktopLister>) -> Result<(), &'static str> {
        LISTER
            .set(lister)
            .map_err(|_| "vfs: lister already installed")
    }

    pub fn lister() -> Option<&'static DesktopLister> {
        LISTER.get().map(|b| b.as_ref())
    }

    fn provider() -> Option<&'static DesktopProvider> {
        PROVIDER.get().map(|b| b.as_ref())
    }

    pub fn read(path: &Path) -> io::Result<Vec<u8>> {
        if let Some(provider) = provider()
            && let Some(bytes) = provider(path)
        {
            return Ok(bytes);
        }
        std::fs::read(path)
    }

    pub fn is_file(path: &Path) -> bool {
        if let Some(provider) = provider()
            && provider(path).is_some()
        {
            return true;
        }
        path.is_file()
    }

    pub fn metadata(path: &Path) -> io::Result<Meta> {
        if let Some(provider) = provider()
            && let Some(bytes) = provider(path)
        {
            return Ok(Meta {
                modified: None,
                len: bytes.len() as u64,
            });
        }
        let meta = std::fs::metadata(path)?;
        Ok(Meta {
            modified: meta.modified().ok(),
            len: meta.len(),
        })
    }

    /// 目录直接子项：先问 lister（资源包条目），再并上真实文件系统。
    ///
    /// 两者都返回才算完整：发布包形态下 `scripts/` 只在容器里，开发形态下只在磁盘上；
    /// 混合形态（骨架 + 容器）两种都可能有。
    pub fn list(dir: &Path) -> Vec<(PathBuf, bool)> {
        let mut out = lister().and_then(|lister| lister(dir)).unwrap_or_default();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let is_dir = path.is_dir();
                if !out.iter().any(|(existing, _)| existing == &path) {
                    out.push((path, is_dir));
                }
            }
        }
        out.sort();
        out
    }
}

#[cfg(target_arch = "wasm32")]
mod backend {
    use super::*;
    use std::cell::RefCell;

    type WasmLister = dyn Fn(&Path) -> Option<Vec<(PathBuf, bool)>>;

    thread_local! {
        static PROVIDER: RefCell<Option<Box<dyn Fn(&Path) -> Option<Vec<u8>>>>> =
            const { RefCell::new(None) };
        static LISTER: RefCell<Option<Box<WasmLister>>> = const { RefCell::new(None) };
    }

    pub fn set(provider: Box<dyn Fn(&Path) -> Option<Vec<u8>>>) -> Result<(), &'static str> {
        PROVIDER.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_some() {
                return Err("vfs: provider already installed");
            }
            *slot = Some(provider);
            Ok(())
        })
    }

    pub fn set_lister(lister: Box<WasmLister>) -> Result<(), &'static str> {
        LISTER.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_some() {
                return Err("vfs: lister already installed");
            }
            *slot = Some(lister);
            Ok(())
        })
    }

    /// wasm 上没有文件系统：目录项只能来自 lister（资源包条目）。
    pub fn list(dir: &Path) -> Vec<(PathBuf, bool)> {
        let mut out = LISTER.with(|slot| {
            slot.borrow()
                .as_deref()
                .and_then(|lister| lister(dir))
                .unwrap_or_default()
        });
        out.sort();
        out.dedup();
        out
    }

    pub fn read(path: &Path) -> io::Result<Vec<u8>> {
        PROVIDER.with(|slot| {
            let slot = slot.borrow();
            let provider = slot.as_deref().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "vfs: no resource provider installed (call vfs::set_provider at startup)",
                )
            })?;
            provider(path).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("vfs: resource not found: {}", path.display()),
                )
            })
        })
    }

    pub fn is_file(path: &Path) -> bool {
        PROVIDER.with(|slot| {
            slot.borrow()
                .as_deref()
                .is_some_and(|provider| provider(path).is_some())
        })
    }

    pub fn metadata(path: &Path) -> io::Result<Meta> {
        let bytes = read(path)?;
        Ok(Meta {
            modified: None,
            len: bytes.len() as u64,
        })
    }
}

pub use backend::{is_file, list, metadata, read};

/// 递归列出一个目录下的全部**文件**（`vfs::list` 的便捷封装）。
///
/// 用途：发布校验要枚举 `scripts/**`，而发布包形态下这些文件只在容器条目里。
/// 符号链接不跟随（与 `assetc` 的扫描口径一致）。
pub fn list_files_recursive(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for (path, is_dir) in list(&current) {
            if is_dir {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// 读一个工程资源并解码——**引擎里读工程文件的唯一入口**。
///
/// 为什么要有这一层（而不是各处自己 `fs::read` + `pack::decode`）：
/// 1. **优先级只有一份判据**：provider 优先、桌面无 provider 时回落 `std::fs`（就是 [`read`]
///    自己的语义）。各处若自己"先 `fs::read` 再 vfs"，桌面上一旦装了 provider 就会被文件系统
///    抢先，与"provider 优先"相反——Web 上恰好看不出来（`std::fs` 恒失败），是一处只在
///    宿主环境下才现形的分叉。
/// 2. **解码不漏**：provider 返回的是**未解码**字节（与 `std::fs::read` 一致，见模块头），
///    压缩/加密由 [`crate::pack::decode`] 统一处理；明文原样返回，所以重复解码安全。
///
/// 读不到与解码失败都是 `Err`（不再让调用方各自 `.ok()` 吞掉原因）。
pub fn read_resource(path: &Path) -> io::Result<Vec<u8>> {
    let bytes = read(path)?;
    crate::pack::decode(&bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
