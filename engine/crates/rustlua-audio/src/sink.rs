//! 音频的**平台出口**：把"声音交给谁放"从引擎里剥出来。
//!
//! ## 为什么需要它
//!
//! 引擎的播放入口是 [`crate::audio::AudioSystem::play_on`] / `play_music`，实现基于 rodio：
//! 它先 `std::fs::read` 把音频文件读成字节，再解码送进输出设备。桌面没问题；
//! **wasm 上两头都不成立**——没有可读的文件系统（`sound_bytes` 直接失败），也没有 cpal 设备。
//! 结果就是 `audio.play('x')` 在 web/微信上恒失败，游戏全程静音。
//!
//! 平台侧其实各有自己的"能用 URL 播放"的通道：浏览器是 `HTMLAudioElement`，
//! 微信小游戏是 `wx.createInnerAudioContext`（由 `wechat-adapter` 提供路径解析）。
//! 所以这一层的契约刻意做得**很小**：给名字、总线、音量与资源目录，**由平台自己解析路径并加载**
//! ——引擎这边不读字节、不碰文件系统。这与 web 旧运行时的分工一致（Lua 侧只算逻辑名，
//! `src` 交给宿主）。
//!
//! ## 存储位置按平台分叉（与 `rustlua-infra::vfs` / `save_slot` 同一套写法）
//!
//! 桌面用 `OnceLock`（provider 必须 `Send + Sync`）；wasm 单线程，provider 放 `thread_local!`，
//! 因而可以捕获 `web_sys` 句柄。桌面**不装** provider 时完全走原来的 rodio 路径 ⇒ 零行为变化。

use std::path::Path;

use crate::audio::AudioBus;

/// 一次播放请求。字段都是"引擎知道、平台需要"的东西，没有平台细节。
pub struct AudioRequest<'a> {
    /// 逻辑名（`audio.play` 里的那个名字，不带扩展名）。
    pub name: &'a str,
    /// 总线：决定用哪个音量（平台侧若要按总线分组调节，看这个）。
    pub bus: AudioBus,
    /// 已算好的有效音量（主音量 × 总线音量），范围 0..=1。
    pub volume: f32,
    /// 音频资源目录（通常 `<project>/assets/audio`）。平台据此算出可加载的 URL：
    /// 引擎不知道 web 的 `pack/…` 前缀，也不该知道。
    pub asset_dir: Option<&'a Path>,
    /// 是否循环（BGM）。
    pub looping: bool,
}

/// 平台播放出口。`play` 返回 `Err` 表示**这次播放没有发生**（平台会给出原因）。
///
/// 为什么不让平台回报"播完了"：引擎侧目前没有任何调用方需要播放结束事件；
/// 契约小一点，接两个平台就少两处猜。
pub trait AudioSinkProvider {
    fn play(&self, request: AudioRequest<'_>) -> Result<(), String>;

    /// 停掉当前 BGM。默认空实现：平台若只有"播一次"的能力，不实现它就退化成"停不掉"，
    /// 但那至少是**显式**的（调用方看得见这条默认值）。
    ///
    /// 为什么必须有它：工程在切图时会调 `audio.stop_music()`（`core/audio.lua`）。
    /// 平台出口如果只能播不能停，换场景之后旧 BGM 会一直响下去——比"没声音"更难查。
    fn stop_music(&self) {}

    /// 暂停 BGM（音效不受影响）。
    fn pause_music(&self) {}

    /// 继续 BGM。
    fn resume_music(&self) {}
}

/// 装了没有。调用方据此决定"走平台出口还是走 rodio"。
pub fn is_installed() -> bool {
    backend::is_installed()
}

/// 交给平台播放。没装 provider 时返回 `Err`（调用方应先问 [`is_installed`]）。
pub fn play(request: AudioRequest<'_>) -> Result<(), String> {
    backend::play(request)
}

/// 停掉 BGM（平台出口没装时是 no-op，与"没设备就静默降级"的既有语义一致）。
pub fn stop_music() {
    backend::stop_music();
}

/// 暂停 BGM。
pub fn pause_music() {
    backend::pause_music();
}

/// 继续 BGM。
pub fn resume_music() {
    backend::resume_music();
}

/// 装平台音频出口。只能装一次；第二次返回 `Err`（静默覆盖会让"谁最后装的"变成查不出来的问题）。
#[cfg(not(target_arch = "wasm32"))]
pub fn set_provider(
    provider: Box<dyn AudioSinkProvider + Send + Sync>,
) -> Result<(), &'static str> {
    backend::set(provider)
}

/// 装平台音频出口。只能装一次；第二次返回 `Err`。
///
/// wasm 单线程，provider **不要求** `Send + Sync`——它可以捕获 `web_sys` 句柄。
#[cfg(target_arch = "wasm32")]
pub fn set_provider(provider: Box<dyn AudioSinkProvider>) -> Result<(), &'static str> {
    backend::set(provider)
}

#[cfg(not(target_arch = "wasm32"))]
mod backend {
    use super::{AudioRequest, AudioSinkProvider};
    use std::sync::OnceLock;

    type Sink = dyn AudioSinkProvider + Send + Sync;

    static PROVIDER: OnceLock<Box<Sink>> = OnceLock::new();

    pub fn set(provider: Box<Sink>) -> Result<(), &'static str> {
        PROVIDER
            .set(provider)
            .map_err(|_| "audio sink: provider already installed")
    }

    pub fn is_installed() -> bool {
        PROVIDER.get().is_some()
    }

    pub fn play(request: AudioRequest<'_>) -> Result<(), String> {
        match PROVIDER.get() {
            Some(provider) => provider.play(request),
            None => Err("audio sink: no provider installed".to_owned()),
        }
    }

    pub fn stop_music() {
        if let Some(provider) = PROVIDER.get() {
            provider.stop_music();
        }
    }

    pub fn pause_music() {
        if let Some(provider) = PROVIDER.get() {
            provider.pause_music();
        }
    }

    pub fn resume_music() {
        if let Some(provider) = PROVIDER.get() {
            provider.resume_music();
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod backend {
    use super::{AudioRequest, AudioSinkProvider};
    use std::cell::RefCell;

    type Sink = dyn AudioSinkProvider;

    thread_local! {
        static PROVIDER: RefCell<Option<Box<Sink>>> = const { RefCell::new(None) };
    }

    pub fn set(provider: Box<Sink>) -> Result<(), &'static str> {
        PROVIDER.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_some() {
                return Err("audio sink: provider already installed");
            }
            *slot = Some(provider);
            Ok(())
        })
    }

    pub fn is_installed() -> bool {
        PROVIDER.with(|slot| slot.borrow().is_some())
    }

    pub fn play(request: AudioRequest<'_>) -> Result<(), String> {
        PROVIDER.with(|slot| match slot.borrow().as_deref() {
            Some(provider) => provider.play(request),
            None => Err("audio sink: no provider installed".to_owned()),
        })
    }

    pub fn stop_music() {
        PROVIDER.with(|slot| {
            if let Some(provider) = slot.borrow().as_deref() {
                provider.stop_music();
            }
        });
    }

    pub fn pause_music() {
        PROVIDER.with(|slot| {
            if let Some(provider) = slot.borrow().as_deref() {
                provider.pause_music();
            }
        });
    }

    pub fn resume_music() {
        PROVIDER.with(|slot| {
            if let Some(provider) = slot.borrow().as_deref() {
                provider.resume_music();
            }
        });
    }
}
