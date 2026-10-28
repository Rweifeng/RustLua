//! `input` 绑定的**组合根接线**（`docs/62` P1 阶段 3）。
//!
//! 与 `audio_lua` 同构：绑定与端口在 `rustlua-input`，这里只负责"设置文件在哪、怎么读、
//! 写完保留别的段"——那些规则归 [`ProjectSettings`]（本 crate 拥有 `settings.toml` 的结构），
//! 而本 crate 已依赖 `rustlua-input`，所以"实现端口"不会成环。
//!
//! 设置根是共享单元（注册表持有）：宿主先建 VM、再定工程根，端口每次调用都读最新值。

use crate::settings::ProjectSettings;
use rustlua_audio::audio::AudioSystem;
use rustlua_input::host::HostInput;
use rustlua_input::input::ActionMap;
use rustlua_input::port::{LoadedSettings, SettingsLoadError, SettingsPort, SettingsSaveError};
use rustlua_lua_api::{Handle, Registry};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// `[input]` 段设置文件的读写端口。
pub struct FileInputSettings {
    root: Rc<RefCell<Option<PathBuf>>>,
}

impl FileInputSettings {
    #[must_use]
    pub fn new(root: Rc<RefCell<Option<PathBuf>>>) -> Self {
        Self { root }
    }

    /// 显式路径优先（绝对路径原样、相对路径相对设置根），否则用工程默认位置。
    ///
    /// 与旧实现逐字一致：无根时返回 `None`（调用方翻译成 `no_project_root` / `nil`）。
    fn resolve(&self, path: Option<&str>) -> Option<PathBuf> {
        let root = self.root.borrow().clone();
        match path.map(str::trim).filter(|value| !value.is_empty()) {
            Some(value) => {
                let candidate = PathBuf::from(value);
                if candidate.is_absolute() {
                    Some(candidate)
                } else {
                    root.map(|root| root.join(candidate))
                }
            }
            None => root.map(|root| ProjectSettings::path_in(&root)),
        }
    }
}

impl SettingsPort for FileInputSettings {
    fn path(&self) -> Option<PathBuf> {
        self.root
            .borrow()
            .as_ref()
            .map(|root| ProjectSettings::path_in(root))
    }

    fn load(&self, path: Option<&str>) -> Result<LoadedSettings, SettingsLoadError> {
        let Some(path) = self.resolve(path) else {
            return Err(SettingsLoadError::NoRoot);
        };
        // 经 `vfs` 读（不是裸 `std::fs`）：wasm 上没有文件系统，裸读恒 `ENOSYS`，
        // 项目自定义动作绑定会整表丢失（只剩默认绑定），键位映射在小游戏里静默失效。
        let bytes = match rustlua_infra::vfs::read_resource(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(SettingsLoadError::Missing);
            }
            Err(error) => {
                return Err(SettingsLoadError::ReadFailed {
                    path,
                    message: error.to_string(),
                });
            }
        };
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let parsed = crate::settings::parse_input_section(&text);
        Ok(LoadedSettings {
            path,
            map: parsed.map,
            // 同一个 `[input]` 段的导航手感：`load_settings` 一次调用就把阈值与节奏带上。
            nav: crate::settings::parse_nav_section(&text),
            notes: parsed.migration_notes,
            reason: parsed.reason,
        })
    }

    fn save(&self, path: Option<&str>, map: &ActionMap) -> Result<(), SettingsSaveError> {
        let Some(path) = self.resolve(path) else {
            return Err(SettingsSaveError::NoRoot);
        };
        // 保序读改写：只动 `[input]` 段，`[audio]` 段与注释原样保留。
        // 这里刻意用裸 `std::fs`（与旧实现一致）：wasm 上没有可写的设置根，这条分支走不到。
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        if let Some(parent) = path.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            return Err(SettingsSaveError::Directory {
                message: error.to_string(),
            });
        }
        let text = crate::settings::upsert_input_section(&existing, map);
        match atomic_write(&path, &text) {
            Ok(()) => Ok(()),
            Err(error) => Err(SettingsSaveError::Write {
                path,
                message: error.to_string(),
            }),
        }
    }
}

/// 原子写（临时文件 + 改名），与 `rustlua_scripting::script::require` 里那份同语义。
fn atomic_write(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, text)?;
    std::fs::rename(temp, path)?;
    Ok(())
}

/// 组合根拿到的那对宿主句柄（与 Lua 侧是同一份状态，不是拷贝）。
pub struct Handles {
    /// 音频系统（`set_audio_asset_dir` / `pause_music` / `maintain_music`）。
    pub audio: Handle<AudioSystem>,
    /// 输入门面（`set_input` / `action_map` / `nav_settings` / `take_rumble_request`）。
    pub input: HostInput,
}

/// 装 `audio` + `input` 两个模块（示例、测试、以及"不要句柄"的宿主用）。
#[must_use]
pub fn registry() -> Rc<Registry> {
    registry_with_handles().0
}

/// 同上，并把宿主句柄一并交出。
///
/// `audio` 与 `input` 的模块**一起装**：两者都靠注册表的共享单元（工程根 / 步长 / 指针），
/// 拆成两个注册表会让"同一帧里 audio 读到的根与 input 读到的根"变成两次查询。
#[must_use]
pub fn registry_with_handles() -> (Rc<Registry>, Handles) {
    let mut registry = Registry::new();
    let audio = Handle::new(AudioSystem::default(), "audio");
    let input = HostInput::new(&registry);

    let audio_settings: Rc<dyn rustlua_audio::lua::SettingsPort> = Rc::new(
        crate::audio_lua::FileAudioSettings::new(registry.root_cell()),
    );
    let audio_installer = audio.clone();
    registry.add("audio", move |lua| {
        rustlua_audio::lua::install(lua, &audio_installer, &audio_settings)
    });

    let input_settings: Rc<dyn SettingsPort> =
        Rc::new(FileInputSettings::new(registry.root_cell()));
    let input_installer = input.clone();
    registry.add("input", move |lua| {
        rustlua_input::lua::install(lua, &input_installer, &input_settings)
    });

    // `nav` 网格状态跟注册表同寿命：热重载重建 VM 时同一份状态被重新装回去，
    // 脚本按名字 `nav.create_grid` 会替换旧网格而不是每次 reload 漏一张。
    let nav = crate::nav_lua::state();
    registry.add("nav", move |lua| crate::nav_lua::install(lua, &nav));

    (Rc::new(registry), Handles { audio, input })
}
