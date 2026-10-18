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
    use std::sync::{Mutex, OnceLock};

    static PROVIDER: OnceLock<Box<DesktopProvider>> = OnceLock::new();
    static LISTER: OnceLock<Box<DesktopLister>> = OnceLock::new();

    /// 采集状态：工程根 + 当前上下文 + 去重计数（BTreeMap 保证输出确定性）。
    struct TraceState {
        root: PathBuf,
        context: String,
        loads: std::collections::BTreeMap<(String, String), u64>,
    }

    static TRACE: OnceLock<Mutex<Option<TraceState>>> = OnceLock::new();

    fn trace_slot() -> &'static Mutex<Option<TraceState>> {
        TRACE.get_or_init(|| Mutex::new(None))
    }

    pub fn start_trace(root: &Path) {
        if let Ok(mut slot) = trace_slot().lock() {
            *slot = Some(TraceState {
                root: root.to_path_buf(),
                context: super::LOAD_CONTEXT_BOOT.to_owned(),
                loads: std::collections::BTreeMap::new(),
            });
        }
    }

    pub fn trace_active() -> bool {
        trace_slot().lock().is_ok_and(|slot| slot.is_some())
    }

    pub fn stop_trace() -> Option<LoadTrace> {
        let state = trace_slot().lock().ok()?.take()?;
        let entries = state
            .loads
            .into_iter()
            .map(|((context, path), count)| LoadEntry {
                context,
                path,
                count,
            })
            .collect();
        Some(LoadTrace {
            schema: 1,
            root: state.root.to_string_lossy().replace('\\', "/"),
            entries,
        })
    }

    pub fn note_context(context: &str) {
        if let Ok(mut slot) = trace_slot().lock()
            && let Some(state) = slot.as_mut()
        {
            state.context = context.to_owned();
        }
    }

    /// 只有**真正读字节**的 `read` 会走到这里；工程根之外的路径不记。
    fn record(path: &Path) {
        let Ok(mut slot) = trace_slot().lock() else {
            return;
        };
        let Some(state) = slot.as_mut() else {
            return;
        };
        let Ok(relative) = path.strip_prefix(&state.root) else {
            return;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        if relative.is_empty() {
            return;
        }
        *state
            .loads
            .entry((state.context.clone(), relative))
            .or_insert(0) += 1;
    }

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
            record(path);
            return Ok(bytes);
        }
        let bytes = std::fs::read(path)?;
        record(path);
        Ok(bytes)
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

// ---------------------------------------------------------------------------------------------
// 加载集观测（`docs/67` §5 的"观测而不是推断"）
// ---------------------------------------------------------------------------------------------
//
// 为什么放在 vfs：它是**所有工程资源读取的唯一入口**（provider 优先、磁盘回落），
// 在这里记一笔就不需要每个读者各自埋点；漏点会直接表现为"某个资源没进观测集"。
//
// 记录的是 `(上下文, 工程相对路径, 次数)`：
// - 上下文由宿主在场景切换时用 [`note_load_context`] 打点（`scenes/manor.scene`）；
// - 只有**真正读字节**的 [`read`] 会记录（`is_file`/`metadata` 是探测，不算加载）；
// - 工程根之外（用户目录、临时文件）的读取不记——它们不是包内资源。

/// 一次观测的上下文（未打点时）。
pub const LOAD_CONTEXT_BOOT: &str = "boot";

/// 一条加载记录。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LoadEntry {
    /// 场景/界面上下文（`scenes/manor.scene` 或 `boot`）。
    pub context: String,
    /// 工程相对路径（`/` 分隔）。
    pub path: String,
    /// 同一上下文里被读了几次（同一帧重复读也算，便于看"热"资源）。
    pub count: u64,
}

/// 一次观测的结果。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LoadTrace {
    pub schema: u32,
    /// 观测时使用的工程根（诊断用；运行期不依赖它）。
    pub root: String,
    pub entries: Vec<LoadEntry>,
}

impl LoadTrace {
    /// 按上下文分组的路径集（编辑器"按场景看读了什么"直接用这个）。
    pub fn by_context(&self) -> std::collections::BTreeMap<&str, Vec<&str>> {
        let mut out: std::collections::BTreeMap<&str, Vec<&str>> =
            std::collections::BTreeMap::new();
        for entry in &self.entries {
            out.entry(entry.context.as_str())
                .or_default()
                .push(entry.path.as_str());
        }
        out
    }

    /// 确定性 JSON（编辑器 / 诊断工具读同一份格式）。
    pub fn to_json(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec_pretty(self).map_err(|error| format!("序列化加载集失败：{error}"))
    }

    /// 从 JSON 还原。
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(bytes).map_err(|error| format!("解析加载集失败：{error}"))
    }

    /// 把观测写入文件（父目录自动创建）。
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = self
            .to_json()
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        std::fs::write(path, bytes)
    }
}

/// 当前是否在采集（宿主/面板用来决定是否提示）。
#[must_use]
pub fn load_trace_active() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        backend::trace_active()
    }
    #[cfg(target_arch = "wasm32")]
    {
        false
    }
}

/// 开始采集。`root` 用来把绝对路径折算成工程相对路径。
#[cfg(not(target_arch = "wasm32"))]
pub fn start_load_trace(root: &Path) {
    backend::start_trace(root);
}

/// wasm：采集不启用（真机诊断另走宿主通道）。
#[cfg(target_arch = "wasm32")]
pub fn start_load_trace(_root: &Path) {}

/// 结束采集并取回结果（没在采集时返回 `None`）。
#[cfg(not(target_arch = "wasm32"))]
pub fn stop_load_trace() -> Option<LoadTrace> {
    backend::stop_trace()
}

/// wasm：永远没有采集结果。
#[cfg(target_arch = "wasm32")]
pub fn stop_load_trace() -> Option<LoadTrace> {
    None
}

/// 打一个上下文点（场景切换时调用）。
#[cfg(not(target_arch = "wasm32"))]
pub fn note_load_context(context: &str) {
    backend::note_context(context);
}

/// wasm：无操作。
#[cfg(target_arch = "wasm32")]
pub fn note_load_context(_context: &str) {}

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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod trace_tests {
    use super::*;

    #[test]
    fn load_trace_records_reads_by_context() {
        let root = std::env::temp_dir().join(format!(
            "rustlua-vfs-trace-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("assets/a.txt"), b"a").unwrap();
        std::fs::write(root.join("assets/b.txt"), b"b").unwrap();

        start_load_trace(&root);
        read(&root.join("assets/a.txt")).unwrap();
        read(&root.join("assets/a.txt")).unwrap();
        note_load_context("scenes/manor.scene");
        read(&root.join("assets/b.txt")).unwrap();
        // 工程根之外不记（用户目录/临时文件不是包内资源）。
        let outside = root.parent().unwrap().join("outside-trace.txt");
        std::fs::write(&outside, b"x").unwrap();
        read(&outside).unwrap();
        let trace = stop_load_trace().expect("采集必须返回结果");
        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(trace.schema, 1);
        assert_eq!(trace.entries.len(), 2, "{:?}", trace.entries);
        let boot = trace
            .entries
            .iter()
            .find(|entry| entry.path == "assets/a.txt")
            .unwrap();
        assert_eq!(boot.context, LOAD_CONTEXT_BOOT);
        assert_eq!(boot.count, 2);
        let scene = trace
            .entries
            .iter()
            .find(|entry| entry.path == "assets/b.txt")
            .unwrap();
        assert_eq!(scene.context, "scenes/manor.scene");
        assert_eq!(scene.count, 1);

        // 往返：编辑器读同一份 JSON。
        let json = trace.to_json().unwrap();
        assert_eq!(LoadTrace::parse(&json).unwrap(), trace);
        assert!(!load_trace_active());
    }
}
