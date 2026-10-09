//! 存档的**平台字符串槽**：把"存档写到哪儿"从引擎里剥出来。
//!
//! ## 为什么需要它
//!
//! 存档在引擎里是"内存表 + `commit`/`load` 落盘"两段（`script/api/save.rs`）。落盘那一段本来
//! 直接走 `std::fs`（`atomic_write` + `read_to_string`）：桌面没问题，**wasm 上没有可写的文件系统**
//! ——那里 `commit` 恒返回 `false`、`load` 恒返回 `missing`，玩家进度永远存不下来。
//!
//! 平台各有自己的"字符串槽"：浏览器是 `localStorage`，微信小游戏是 `wx.setStorageSync`
//! （由 `wechat-adapter` 把 `localStorage` 映射过去）。所以这一层只做两件事：**读一个键、
//! 写一个键**，序列化仍然在 Lua 侧/引擎里完成。
//!
//! ## 键名与 JS 世界一致（这一点是硬要求）
//!
//! 键的格式不是随便定的：web 上的旧运行时（`plugins/webgl/web-runtime.js` 的 `save` 实现）
//! 用的是 `rustlua:save` / `rustlua:save_<sanitized slot>`。**同一个键**意味着切到 Rust 通道之后，
//! 玩家浏览器里已有的存档仍然读得到；键名分叉则等于把所有人的进度清零。
//!
//! ## 存储位置按平台分叉（与 `vfs` 同一套写法）
//!
//! 桌面用 `OnceLock`（任何线程都可能读，provider 必须 `Send + Sync`）；wasm 单线程，provider
//! 放 `thread_local!`，因而可以捕获 `web_sys::Storage` 这类非 `Send` 句柄。桌面**不装** provider
//! 时回落文件系统 ⇒ 零行为变化。

/// 平台槽：读一个键、写一个键。`read` 返回 `None` 表示"这个键还没有值"（不是错误）。
pub trait SaveSlotProvider {
    fn read(&self, key: &str) -> Option<String>;
    fn write(&self, key: &str, text: &str) -> Result<(), String>;
}

/// 存档键前缀。与 `plugins/webgl/web-runtime.js:2895` 的 `__SAVE_STORAGE_PREFIX` 必须逐字一致。
pub const STORAGE_PREFIX: &str = "rustlua:save";

/// 槽位 → 平台存储键。
///
/// 规则与 JS 世界（`web-runtime.js:3109-3118`）以及引擎的 `save_path_for_slot` 三方一致：
/// 默认档就是前缀本身；具名档把非字母数字/`-`/`_` 的字符换成 `_`，全被换掉则用 `default`。
/// 三方一致是硬要求——不一致就变成"换了通道读不到旧档"。
pub fn storage_key(slot: Option<&str>) -> String {
    let Some(slot) = slot else {
        return STORAGE_PREFIX.to_owned();
    };
    let safe: String = slot
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let safe = if safe.is_empty() {
        "default".to_owned()
    } else {
        safe
    };
    format!("{STORAGE_PREFIX}_{safe}")
}

/// 装了没有。调用方用它决定"走槽还是走文件系统"。
pub fn is_installed() -> bool {
    backend::is_installed()
}

/// 读一个键（`None` = 没装 provider 或该键不存在）。
pub fn read(key: &str) -> Option<String> {
    backend::read(key)
}

/// 写一个键。没装 provider 时返回 `Err`（调用方应当先问 [`is_installed`]）。
pub fn write(key: &str, text: &str) -> Result<(), String> {
    backend::write(key, text)
}

/// 装平台槽。只能装一次；第二次返回 `Err`（与 `vfs::set_provider` 同语义：静默覆盖会让
/// "谁最后装的"变成一个查不出来的问题）。
#[cfg(not(target_arch = "wasm32"))]
pub fn set_provider(provider: Box<dyn SaveSlotProvider + Send + Sync>) -> Result<(), &'static str> {
    backend::set(provider)
}

/// 装平台槽。只能装一次；第二次返回 `Err`。
///
/// wasm 单线程，所以这里的 provider **不要求** `Send + Sync`——它可以捕获 `web_sys::Storage`
/// 这类句柄。
#[cfg(target_arch = "wasm32")]
pub fn set_provider(provider: Box<dyn SaveSlotProvider>) -> Result<(), &'static str> {
    backend::set(provider)
}

#[cfg(not(target_arch = "wasm32"))]
mod backend {
    use super::SaveSlotProvider;
    use std::sync::OnceLock;

    type Slot = dyn SaveSlotProvider + Send + Sync;

    static PROVIDER: OnceLock<Box<Slot>> = OnceLock::new();

    pub fn set(provider: Box<Slot>) -> Result<(), &'static str> {
        PROVIDER
            .set(provider)
            .map_err(|_| "save_slot: provider already installed")
    }

    pub fn is_installed() -> bool {
        PROVIDER.get().is_some()
    }

    pub fn read(key: &str) -> Option<String> {
        PROVIDER.get().and_then(|provider| provider.read(key))
    }

    pub fn write(key: &str, text: &str) -> Result<(), String> {
        match PROVIDER.get() {
            Some(provider) => provider.write(key, text),
            None => Err("save_slot: no provider installed".to_owned()),
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod backend {
    use super::SaveSlotProvider;
    use std::cell::RefCell;

    type Slot = dyn SaveSlotProvider;

    thread_local! {
        static PROVIDER: RefCell<Option<Box<Slot>>> = const { RefCell::new(None) };
    }

    pub fn set(provider: Box<Slot>) -> Result<(), &'static str> {
        PROVIDER.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_some() {
                return Err("save_slot: provider already installed");
            }
            *slot = Some(provider);
            Ok(())
        })
    }

    pub fn is_installed() -> bool {
        PROVIDER.with(|slot| slot.borrow().is_some())
    }

    pub fn read(key: &str) -> Option<String> {
        PROVIDER.with(|slot| slot.borrow().as_deref().and_then(|p| p.read(key)))
    }

    pub fn write(key: &str, text: &str) -> Result<(), String> {
        PROVIDER.with(|slot| match slot.borrow().as_deref() {
            Some(provider) => provider.write(key, text),
            None => Err("save_slot: no provider installed".to_owned()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 键名规则必须与 JS 世界逐字一致——不一致就是"换通道读不到旧档"。
    #[test]
    fn storage_keys_match_the_web_runtime_rule() {
        assert_eq!(storage_key(None), "rustlua:save");
        assert_eq!(storage_key(Some("slot1")), "rustlua:save_slot1");
        assert_eq!(storage_key(Some("Slot 2/x")), "rustlua:save_Slot_2_x");
        // 注意：`///` 被换成 `___` 后**非空**，所以它不走 `default`——这是 JS 侧的实际规则
        // （`gsub` 之后才判空）。也别把分隔下划线数漏了：键是 `前缀 + "_" + 槽`，
        // 于是这里是 4 个下划线。这两处我第一版都写错了，测试当场抓住。
        assert_eq!(storage_key(Some("///")), "rustlua:save____");
        // 真正空串才落 `default`。Lua 侧的空槽会被过滤成 `None`，所以这条是防御性的。
        assert_eq!(storage_key(Some("")), "rustlua:save_default");
    }
}
