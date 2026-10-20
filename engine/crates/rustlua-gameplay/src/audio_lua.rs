//! `audio` 绑定的**组合根接线**（`docs/62` P1 依赖倒置）。
//!
//! 为什么接线点在 `rustlua-gameplay`：
//!
//! - `audio.load_settings` / `save_settings` 读写 `settings.toml` 的 `[audio]` 段，而那份文件的
//!   结构由本 crate 的 [`ProjectSettings`] 定义；
//! - 本 crate 已经依赖 `rustlua-audio`，所以"实现端口 + 建注册表"放在这里不会成环；
//! - 凡是会建 Lua 运行时的地方（`engine`、`game`、`game_web`、`editor`、示例与测试）都已经
//!   依赖本 crate——它们只需要一行 `LuaRuntime::with_installers(rustlua_gameplay::input_lua::registry())`，
//!   不必各自复制一份设置端口。
//!
//! 绑定本体在 `rustlua_audio::lua`，这里只负责"文件在哪、怎么读、写完保留别的段"。

use crate::settings::ProjectSettings;
use rustlua_audio::audio::AudioSystem;
use rustlua_audio::lua::{self, SettingsLoadError, SettingsPort};
use rustlua_lua_api::{Handle, Registry};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// `settings.toml` 的读写端口：口径与绑定还在 `rustlua-scripting` 时逐字一致。
///
/// 根目录是**共享单元**而不是构造参数：宿主先建 VM、再定工程根（`set_project_root` /
/// `set_settings_root`），端口在每次调用时读取，因此拿到的一定是最新值。
pub struct FileAudioSettings {
    root: Rc<RefCell<Option<PathBuf>>>,
}

impl FileAudioSettings {
    #[must_use]
    pub fn new(root: Rc<RefCell<Option<PathBuf>>>) -> Self {
        Self { root }
    }
}

impl SettingsPort for FileAudioSettings {
    fn path(&self) -> Option<PathBuf> {
        self.root
            .borrow()
            .as_ref()
            .map(|root| ProjectSettings::path_in(root))
    }

    fn load(&self, audio: &mut AudioSystem) -> Result<(), SettingsLoadError> {
        let Some(root) = self.root.borrow().clone() else {
            return Err(SettingsLoadError::NoRoot);
        };
        let path = ProjectSettings::path_in(&root);
        // 设置文本经 `vfs` 读（**不是** `ProjectSettings::load_from` 里的 `std::fs`）：
        // wasm 上没有文件系统，裸读恒 `ENOSYS`，`audio.load_settings()` 会退化成
        // "operation not supported on this platform"。桌面无 provider 时
        // `vfs::read_resource` 自己回落 `std::fs`，语义不变。
        let bytes = match rustlua_infra::vfs::read_resource(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(SettingsLoadError::Missing);
            }
            Err(error) => return Err(SettingsLoadError::Failed(error.to_string())),
        };
        let text = String::from_utf8_lossy(&bytes).into_owned();
        ProjectSettings::from_toml(&text).apply_to(audio);
        Ok(())
    }

    fn save(&self, audio: &AudioSystem) -> Result<(), String> {
        let Some(root) = self.root.borrow().clone() else {
            // 文本原样给 Lua：`audio.save_settings()` 的返回值是 `(false, 文本)` 的契约
            // （已有工程脚本按 `no_project_root` 分支处理）。
            return Err("no_project_root".to_owned());
        };
        ProjectSettings::from_audio(audio)
            .save_to(&root)
            .map_err(|error| error.to_string())
    }
}

/// 只装了 `audio` 模块的注册表（示例、测试、以及"不要宿主句柄"的宿主用）。
#[must_use]
pub fn registry() -> Rc<Registry> {
    registry_with_audio().0
}

/// 同上，并把能力单元一并交出：宿主（`engine` 的运行时驱动）需要它来
/// `set_audio_asset_dir` / `pause_music` / `resume_music` / `maintain_music`。
///
/// 返回的句柄与装进 Lua 的是**同一份状态**（`Handle` 是共享单元），不是两份拷贝。
#[must_use]
pub fn registry_with_audio() -> (Rc<Registry>, Handle<AudioSystem>) {
    let audio = Handle::new(AudioSystem::default(), "audio");
    let mut registry = Registry::new();
    let settings: Rc<dyn SettingsPort> = Rc::new(FileAudioSettings::new(registry.root_cell()));
    let installer = audio.clone();
    registry.add("audio", move |lua| lua::install(lua, &installer, &settings));
    (Rc::new(registry), audio)
}
