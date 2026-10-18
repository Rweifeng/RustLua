//! 发布包（RLB 容器）在 wasm 宿主上的挂载（`docs/67` §7.3，批 B）。
//!
//! 与桌面的差别只有两点，都是被运行时形态逼出来的：
//! 1. **没有文件系统**：容器字节只能来自 `fetch`（浏览器）或适配层同步读取器（微信）。
//!    所以这里只接受"已经拿到的字节"，取字节的方式留给调用方。
//! 2. **provider 已被占用**：浏览器是预载表（`lua_bridge::LUA_RESOURCES`）、微信是适配层读取器，
//!    两者都已经装进了 vfs 的唯一 provider 槽。所以这里**只装 lister**，把查包这件事交给
//!    [`lookup`]，由那两个 provider 自己在第一步里调用（见 `lua_bridge::provider_from_*`）。
//!
//! 形态判定与桌面一致：`manifest.txt` 里出现 `bundles/*.rlb` 就是容器形态。没有这一条时
//! [`mount`] 返回 0，宿主行为与本批之前**逐字节一致**（这也是批 C 翻默认值之前的保险）。

use std::cell::RefCell;
use std::rc::Rc;

use rustlua_infra::bundle::BundleSet;

thread_local! {
    /// 已挂载的容器集合。wasm 单线程，用 `Rc<RefCell<..>>` 即可。
    static MOUNTED: RefCell<Option<Rc<RefCell<BundleSet>>>> = const { RefCell::new(None) };
    /// 还在后台下载的容器数（`docs/67` 批 C2 第 2 步：只先挂 `unit = main` 的组）。
    ///
    /// 它只服务一件事：**命中失败时能说清"是缺件还是还没下完"**。
    /// 没有这个计数，延迟挂载会把"资源不在包里"和"资源还在路上"变成同一条错误，
    /// 而那两种情况的处置完全不同（前者改配置，后者等一会儿）。
    static PENDING: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// 已挂载的**容器名**（`bundles/x.rlb` 逻辑路径）。补挂时用它去重——
    /// `BundleSet::paths()` 返回的是包内条目，不是容器名，不能拿来判重。
    static MOUNTED_NAMES: RefCell<std::collections::HashSet<String>> =
        RefCell::new(std::collections::HashSet::new());
}

/// 登记"还有 N 个容器在后台下载"。
pub fn set_pending(count: usize) {
    PENDING.with(|slot| slot.set(count));
}

/// 当前还在后台下载的容器数（0 = 全部就位）。
pub fn pending() -> usize {
    PENDING.with(std::cell::Cell::get)
}

/// 从 `manifest.txt` 里挑出容器条目（实现见 `resource_index::bundle_files`，桌面也有门）。
pub use super::resource_index::bundle_files;

/// 是否已经挂载过容器。
pub fn is_mounted() -> bool {
    MOUNTED.with(|slot| slot.borrow().is_some())
}

/// 挂载若干容器（`entries` = `(manifest 里的相对路径, 容器字节)`）。
///
/// 只装 vfs 的 **lister**：`verify_script_syntax` 之类的"枚举工程文件"要能从容器里列出条目，
/// 而读字节的通道仍然由宿主自己的 provider 负责（它先调 [`lookup`]）。
pub fn mount(entries: Vec<(String, Vec<u8>)>) -> Result<usize, String> {
    if entries.is_empty() || is_mounted() {
        return Ok(0);
    }
    let mut set = BundleSet::new(super::VIRTUAL_PROJECT_ROOT);
    let mut total = 0usize;
    // **按值接收**（不是 `&[...]` + `bytes.clone()`）：克隆会把 3.6 MB 的容器再复制一份，
    // 启动期的峰值内存直接翻倍——实测这是那 80 ms 里真正花得冤的一块。
    for (path, bytes) in entries {
        total += set
            .mount_bytes(&path, bytes)
            .map_err(|error| format!("挂载 {path} 失败：{error}"))?;
        MOUNTED_NAMES.with(|names| {
            names.borrow_mut().insert(path);
        });
    }
    let set = Rc::new(RefCell::new(set));

    // 只装 lister（provider 留给宿主）。
    let for_list = Rc::clone(&set);
    rustlua_infra::vfs::set_lister(move |dir: &std::path::Path| {
        let guard = for_list.borrow();
        let key = guard.key_of(dir)?;
        let list: Vec<(std::path::PathBuf, bool)> = guard
            .list_dir(&key)
            .into_iter()
            .map(|(name, is_dir)| (dir.join(name), is_dir))
            .collect();
        Some(list)
    })
    .map_err(|error| format!("安装 bundle lister 失败：{error}"))?;

    MOUNTED.with(|slot| *slot.borrow_mut() = Some(set));
    Ok(total)
}

/// 追加挂载（启动时只挂 `unit = main` 的组，其余后台补挂）。
///
/// 不需要重装 lister：装上去的那个闭包持有 `Rc::clone(&set)`，它每次调用都读最新的集合，
/// 所以后加的容器立刻对"枚举工程文件"可见。
pub fn mount_more(entries: Vec<(String, Vec<u8>)>) -> Result<usize, String> {
    if entries.is_empty() {
        return Ok(0);
    }
    let Some(set) = MOUNTED.with(|slot| slot.borrow().as_ref().map(Rc::clone)) else {
        return mount(entries);
    };
    let mut total = 0usize;
    for (path, bytes) in entries {
        let added = set
            .borrow_mut()
            .mount_bytes(&path, bytes)
            .map_err(|error| format!("追加挂载 {path} 失败：{error}"))?;
        total += added;
        MOUNTED_NAMES.with(|names| {
            names.borrow_mut().insert(path);
        });
    }
    Ok(total)
}

/// 查一个工程相对路径（宿主 provider 的第一步）。
pub fn lookup(path: &std::path::Path) -> Option<Vec<u8>> {
    let set = MOUNTED.with(|slot| slot.borrow().as_ref().map(Rc::clone))?;
    rustlua_infra::bundle::lookup(&mut set.borrow_mut(), path)
}

/// 只看有没有（不解码块、不拷字节）。
pub fn contains(path: &std::path::Path) -> bool {
    let Some(set) = MOUNTED.with(|slot| slot.borrow().as_ref().map(Rc::clone)) else {
        return false;
    };
    rustlua_infra::bundle::contains_path(&set.borrow(), path)
}

/// 已挂载条目数（启动诊断用）。
pub fn entry_count() -> usize {
    MOUNTED.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|set| set.borrow().entry_count())
            .unwrap_or(0)
    })
}

/// 已挂载条目名（诊断/门用）。
pub fn paths() -> Vec<String> {
    MOUNTED.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|set| set.borrow().paths().map(str::to_owned).collect())
            .unwrap_or_default()
    })
}

/// 微信侧：用适配层的同步读取器把**当前已挂载根**里的容器读进来并挂载。
///
/// 与 [`mount_from_reader`] 的区别：读不到的容器**不报错**，只记 `pending`。多根布局 boot 时
/// 具名根还没 `loadSubpackage`，这是预期状态；等该根挂上后由 [`mount_bundles_from_reader`]
/// 补挂。**一个容器都读不到**仍报错——那说明主包/内核根都坏了，继续启动只会得到假死。
pub fn mount_from_reader_partial(
    manifest: &str,
    read_file: &js_sys::Function,
) -> Result<usize, wasm_bindgen::JsValue> {
    let files = bundle_files(manifest);
    if files.is_empty() || is_mounted() {
        return Ok(0);
    }
    let read = |key: &str| -> Option<Vec<u8>> {
        let value = read_file
            .call1(
                &wasm_bindgen::JsValue::NULL,
                &wasm_bindgen::JsValue::from_str(key),
            )
            .ok()?;
        js_bytes(&value)
    };
    let mut entries = Vec::with_capacity(files.len());
    let mut missing = 0usize;
    for relative in files {
        match read(&format!("pack/{relative}")).or_else(|| read(&relative)) {
            Some(bytes) => entries.push((relative, bytes)),
            None => missing += 1,
        }
    }
    if entries.is_empty() {
        return Err(wasm_bindgen::JsValue::from_str(
            "wechat wasm: cannot read any bundle (main/wasm roots not mounted?)",
        ));
    }
    let mounted = mount(entries).map_err(|error| wasm_bindgen::JsValue::from_str(&error))?;
    set_pending(missing);
    Ok(mounted)
}

/// 某根挂载后补挂它的容器：`paths` 是 `bundle_plan.json` 里的 `bundles/x.rlb` 逻辑路径。
/// 已挂载过的路径会被过滤（`mount_more` 不负责去重）。
pub fn mount_bundles_from_reader(
    bundle_paths: &[String],
    read_file: &js_sys::Function,
) -> Result<usize, wasm_bindgen::JsValue> {
    let already: std::collections::HashSet<String> =
        MOUNTED_NAMES.with(|names| names.borrow().clone());
    let read = |key: &str| -> Option<Vec<u8>> {
        let value = read_file
            .call1(
                &wasm_bindgen::JsValue::NULL,
                &wasm_bindgen::JsValue::from_str(key),
            )
            .ok()?;
        js_bytes(&value)
    };
    let mut entries = Vec::with_capacity(bundle_paths.len());
    for relative in bundle_paths {
        if already.contains(relative) {
            continue;
        }
        let bytes = read(&format!("pack/{relative}"))
            .or_else(|| read(relative))
            .ok_or_else(|| {
                wasm_bindgen::JsValue::from_str(&format!(
                    "wechat wasm: cannot read bundle {relative}"
                ))
            })?;
        entries.push((relative.clone(), bytes));
    }
    if entries.is_empty() {
        return Ok(0);
    }
    let mounted_bundles = entries.len();
    let mounted = mount_more(entries).map_err(|error| wasm_bindgen::JsValue::from_str(&error))?;
    // partial 时按"缺几个容器"记的 pending；补挂成功后扣掉本次的容器数。
    set_pending(pending().saturating_sub(mounted_bundles));
    Ok(mounted)
}

/// 微信侧：用适配层的同步读取器把容器读进来并挂载。
///
/// `read_file` 的键域是**带 `pack/` 前缀的逻辑路径**（适配层按这个方言做 mainpack 路由与改名映射）。
///
/// 这点踩过坑：之前传的是裸 `bundles/x.rlb`，适配层认不出，`mount_from_reader` 静默拿到 0 条容器，
/// 于是 `scripts/main.lua` 回落成磁盘读取 → 真机报 `vfs: resource not found: /project/scripts/main.lua`。
/// 现在两种方言都试：先 `pack/…`（正解），失败再试裸路径（兼容老适配层）。
pub fn mount_from_reader(
    manifest: &str,
    read_file: &js_sys::Function,
) -> Result<usize, wasm_bindgen::JsValue> {
    let files = bundle_files(manifest);
    if files.is_empty() || is_mounted() {
        return Ok(0);
    }
    let read = |key: &str| -> Option<Vec<u8>> {
        let value = read_file
            .call1(
                &wasm_bindgen::JsValue::NULL,
                &wasm_bindgen::JsValue::from_str(key),
            )
            .ok()?;
        js_bytes(&value)
    };
    let mut entries = Vec::with_capacity(files.len());
    for relative in files {
        let bytes = read(&format!("pack/{relative}")).or_else(|| read(&relative));
        let bytes = bytes.ok_or_else(|| {
            wasm_bindgen::JsValue::from_str(&format!("wechat wasm: cannot read bundle {relative}"))
        })?;
        entries.push((relative, bytes));
    }
    mount(entries).map_err(|error| wasm_bindgen::JsValue::from_str(&error))
}

/// JS 值 → 字节。**必须按字节取**：走 `as_string` 会把二进制换成 U+FFFD
/// （0.7.11 之前那条通道的实测症状是字体 InvalidFont + 贴图全白，见 `provider_from_reader`）。
pub fn js_bytes(value: &wasm_bindgen::JsValue) -> Option<Vec<u8>> {
    use wasm_bindgen::JsCast;
    if let Some(buffer) = value.dyn_ref::<js_sys::ArrayBuffer>() {
        return Some(js_sys::Uint8Array::new(buffer).to_vec());
    }
    if let Some(bytes) = value.dyn_ref::<js_sys::Uint8Array>() {
        return Some(bytes.to_vec());
    }
    None
}
