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
//! `crate::core::pack::decode`.

use std::io;
use std::path::Path;

/// Filesystem-like metadata exposed to the Lua bridge (hot-reload uses it).
pub struct Meta {
    pub modified: Option<std::time::SystemTime>,
    pub len: u64,
}

/// 桌面端资源提供者的类型别名（签名较长，收敛重复，见 clippy type_complexity）。
#[cfg(not(target_arch = "wasm32"))]
type DesktopProvider = dyn Fn(&Path) -> Option<Vec<u8>> + Send + Sync;

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

#[cfg(not(target_arch = "wasm32"))]
mod backend {
    use super::*;
    use std::sync::OnceLock;

    static PROVIDER: OnceLock<Box<DesktopProvider>> = OnceLock::new();

    pub fn set(provider: Box<DesktopProvider>) -> Result<(), &'static str> {
        PROVIDER
            .set(provider)
            .map_err(|_| "vfs: provider already installed")
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
}

#[cfg(target_arch = "wasm32")]
mod backend {
    use super::*;
    use std::cell::RefCell;

    thread_local! {
        static PROVIDER: RefCell<Option<Box<dyn Fn(&Path) -> Option<Vec<u8>>>>> =
            const { RefCell::new(None) };
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

pub use backend::{is_file, metadata, read};
