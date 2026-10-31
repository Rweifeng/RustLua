//! 用户级编辑器设置：`<用户主目录>/.rustlua/`。
//!
//! 与 `<project>/.cache/editor/layout.json` 的分工：
//!
//! * **布局**（面板宽度、dock 树、缩放、工作台）属于**工程**，跟着工程走；
//! * **服务商设置**（地址 / 模型 / max_tokens）与 **API Key** 属于**这台机器的这个用户**，
//!   换工程、换启动方式（带不带 `--project`）都不该重置。
//!
//! 目录位置：`RUSTLUA_HOME`（便携版/测试覆盖）→ Windows `%USERPROFILE%\.rustlua`
//! → 其他平台 `$HOME/.rustlua`；没有可用主目录时退回 `None`（设置只在本次进程内有效）。
//! 目录在第一次成功写盘时才创建。
//!
//! 文件：
//!
//! * `editor.json` —— 非密钥设置。顶层是对象，按段分放 `version`、`assistant`、`appearance`
//!   （主题）、`shortcuts`（改键）、`plugins`（插件开关）、`panels`（插件面板可见性），
//!   未知顶层键（以及各段内的未知键）原样保留，新版本写的设置不会被旧版本抹掉；
//! * `assistant.key` —— API Key 密文。Windows 用 DPAPI（`CryptProtectData`）绑定当前
//!   用户账户：拷到别的账户/机器解不开。非 Windows 平台退回 0600 权限的明文文件。
//!
//! **密钥不变量**：`editor.json` 永远不含 Key；Key 只进 `assistant.key`。解密失败
//! （密文损坏、换了 Windows 账户）返回原因给调用方提示，而不是静默当作"没填"。

use crate::{AppearanceSettings, AssistantLayout, ShortcutBindings};
use std::{
    collections::BTreeMap,
    env, fs, io,
    path::{Path, PathBuf},
};

/// 覆盖用户主目录的环境变量（便携版与测试）。
pub const HOME_ENV: &str = "RUSTLUA_HOME";
/// `editor.json` 的版本号；只有磁盘形状需要迁移时才升。
pub const CONFIG_VERSION: u32 = 1;
const CONFIG_FILE: &str = "editor.json";
const KEY_FILE: &str = "assistant.key";

/// `<用户主目录>/.rustlua` 的读写入口。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserConfigStore {
    root: PathBuf,
}

impl UserConfigStore {
    #[must_use]
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 默认位置；没有可用的用户主目录时返回 `None`。
    #[must_use]
    pub fn default_root() -> Option<PathBuf> {
        if let Some(value) = env::var_os(HOME_ENV).filter(|value| !value.is_empty()) {
            return Some(PathBuf::from(value));
        }
        let home = if cfg!(windows) {
            env::var_os("USERPROFILE")
        } else {
            env::var_os("HOME")
        };
        home.filter(|value| !value.is_empty())
            .map(|home| PathBuf::from(home).join(".rustlua"))
    }

    #[must_use]
    pub fn default_store() -> Option<Self> {
        Self::default_root().map(Self::at)
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        self.root.join(CONFIG_FILE)
    }

    #[must_use]
    pub fn api_key_path(&self) -> PathBuf {
        self.root.join(KEY_FILE)
    }

    /// 读非密钥设置。文件缺失、无法解析、没有 `assistant` 段都返回 `None`
    /// （调用方保留环境变量/旧工程布局的初值）。
    #[must_use]
    pub fn load_assistant(&self) -> Option<AssistantLayout> {
        let text = fs::read_to_string(self.config_path()).ok()?;
        let value: serde_json::Value = serde_json::from_str(&text).ok()?;
        parse_assistant(value.get("assistant")?)
    }

    /// 写非密钥设置（tmp + rename 原子写，保留未知键）。
    pub fn save_assistant(&self, settings: &AssistantLayout) -> io::Result<()> {
        self.save_section(
            "assistant",
            &[
                ("provider", serde_json::json!(settings.provider)),
                ("base_url", serde_json::json!(settings.base_url)),
                ("model", serde_json::json!(settings.model)),
                ("max_tokens", serde_json::json!(settings.max_tokens)),
            ],
        )
    }

    /// 读外观偏好（`appearance.theme`）。文件缺失/损坏/没有该段都返回 `None`，
    /// 调用方保留默认深色。
    #[must_use]
    pub fn load_appearance(&self) -> Option<AppearanceSettings> {
        let text = fs::read_to_string(self.config_path()).ok()?;
        let value: serde_json::Value = serde_json::from_str(&text).ok()?;
        parse_appearance(value.get("appearance")?)
    }

    /// 写外观偏好（与助手设置同一个 `editor.json`，未知键原样保留）。
    pub fn save_appearance(&self, settings: &AppearanceSettings) -> io::Result<()> {
        self.save_section(
            "appearance",
            &[("theme", serde_json::json!(settings.theme))],
        )
    }

    /// 读编辑器快捷键改键（`shortcuts` 段：动作 id → 键位文本）。
    ///
    /// 没有该段返回 `None`（全部动作用默认键）；坏文本整条丢弃（该动作回落默认），
    /// 本版本不认识的 id 保留在表里，下次保存原样写回。
    #[must_use]
    pub fn load_shortcuts(&self) -> Option<ShortcutBindings> {
        let text = fs::read_to_string(self.config_path()).ok()?;
        let value: serde_json::Value = serde_json::from_str(&text).ok()?;
        let object = value.get("shortcuts")?.as_object()?;
        let entries = object
            .iter()
            .filter_map(|(id, text)| Some((id.clone(), text.as_str()?.to_owned())))
            .collect::<BTreeMap<_, _>>();
        Some(ShortcutBindings::from_entries(entries))
    }

    /// 写编辑器快捷键改键（与助手/外观同一个 `editor.json`，段内外的未知键都保留）。
    ///
    /// 写的是**整张表**（含本版本不认识的 id）：调用方加载后再保存，未来版本的动作
    /// 键位不会因为旧版本只写自己认识的 id 而丢失。
    pub fn save_shortcuts(&self, bindings: &ShortcutBindings) -> io::Result<()> {
        let entries = bindings
            .entries()
            .iter()
            .map(|(id, text)| (id.as_str(), serde_json::json!(text)))
            .collect::<Vec<_>>();
        self.save_section("shortcuts", &entries)
    }

    /// 读改写 `editor.json` 的一个对象段：保留段内外所有未知键与未知顶层键。
    fn save_section(&self, section: &str, entries: &[(&str, serde_json::Value)]) -> io::Result<()> {
        let path = self.config_path();
        let mut value = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .filter(serde_json::Value::is_object)
            .unwrap_or_else(|| serde_json::json!({}));
        let object = value
            .as_object_mut()
            .expect("value was normalized to an object");
        object.insert("version".to_owned(), serde_json::json!(CONFIG_VERSION));
        let mut stored = object
            .get(section)
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default();
        for (key, incoming) in entries {
            stored.insert((*key).to_owned(), incoming.clone());
        }
        object.insert(section.to_owned(), serde_json::Value::Object(stored));
        let text = serde_json::to_string_pretty(&value)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        write_atomic(&path, text.as_bytes(), false)
    }

    /// 读插件开关（`plugins` 段：插件 id → 是否启用）。
    ///
    /// 没有该段返回 `None`（全部按内置默认启用）；不是布尔值的条目整条丢弃（该插件回落
    /// 默认）；本版本不认识的 id 留着不动——插件改名/下线后这里只会多一条无用记录，
    /// 不会让读它的版本丢设置。
    #[must_use]
    pub fn load_plugin_states(&self) -> Option<BTreeMap<String, bool>> {
        let text = fs::read_to_string(self.config_path()).ok()?;
        let value: serde_json::Value = serde_json::from_str(&text).ok()?;
        let object = value.get("plugins")?.as_object()?;
        let entries = object
            .iter()
            .filter_map(|(id, state)| Some((id.clone(), state.as_bool()?)))
            .collect::<BTreeMap<_, _>>();
        Some(entries)
    }

    /// 写插件开关（与助手/外观/快捷键同一个 `editor.json`，段内外未知键都保留）。
    ///
    /// 写的是**整张表**：启停一个插件会把注册表里所有插件的当前状态一起落盘，
    /// 这样"这个机器上用户关掉过哪些"不会因为只写变化那一条而和内存脱节。
    pub fn save_plugin_states(&self, states: &BTreeMap<String, bool>) -> io::Result<()> {
        let entries = states
            .iter()
            .map(|(id, state)| (id.as_str(), serde_json::json!(state)))
            .collect::<Vec<_>>();
        self.save_section("plugins", &entries)
    }

    /// 读插件面板的可见性（`panels` 段：面板 id → 是否可见）。
    ///
    /// 没有该段返回 `None`（全部按默认关闭）；不是布尔值的条目整条丢弃；本版本不认识的
    /// id 留着不动（插件下线后这里只会多一条无用记录）。
    #[must_use]
    pub fn load_panel_states(&self) -> Option<BTreeMap<String, bool>> {
        let text = fs::read_to_string(self.config_path()).ok()?;
        let value: serde_json::Value = serde_json::from_str(&text).ok()?;
        let object = value.get("panels")?.as_object()?;
        let entries = object
            .iter()
            .filter_map(|(id, state)| Some((id.clone(), state.as_bool()?)))
            .collect::<BTreeMap<_, _>>();
        Some(entries)
    }

    /// 写插件面板的可见性（与其它段同一个 `editor.json`，未知键都保留）。
    ///
    /// 写的是**整张已知表**（含 `false`）：只写"可见的那些"会让用户关掉的面板在文件里
    /// 仍是 `true`，下次启动又自己弹出来。
    pub fn save_panel_states(&self, states: &BTreeMap<String, bool>) -> io::Result<()> {
        let entries = states
            .iter()
            .map(|(id, state)| (id.as_str(), serde_json::json!(state)))
            .collect::<Vec<_>>();
        self.save_section("panels", &entries)
    }

    /// 读并解密 API Key。文件不存在 → `Ok(None)`；密文解不开 → `Err(原因)`。
    pub fn load_api_key(&self) -> Result<Option<String>, String> {
        let path = self.api_key_path();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("读取 {} 失败：{error}", path.display())),
        };
        if bytes.is_empty() {
            return Ok(None);
        }
        let plain = secrets::unprotect(&bytes).map_err(|error| {
            format!(
                "{} 里的 API Key 无法解密（{error}）；可能来自其他 Windows 用户或别的机器，\
                 重新填写即可覆盖",
                path.display()
            )
        })?;
        String::from_utf8(plain)
            .map(Some)
            .map_err(|_| format!("{} 解密后不是 UTF-8 文本", path.display()))
    }

    /// 写/清除 API Key（空串 = 删除密钥文件）。
    pub fn save_api_key(&self, key: &str) -> io::Result<()> {
        let path = self.api_key_path();
        let key = key.trim();
        if key.is_empty() {
            return match fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            };
        }
        let protected = secrets::protect(key.as_bytes())?;
        write_atomic(&path, &protected, true)
    }
}

/// 宽松解析 `assistant` 段：单个字段类型不符只丢那个字段。
fn parse_assistant(value: &serde_json::Value) -> Option<AssistantLayout> {
    let object = value.as_object()?;
    let mut stored = AssistantLayout::default();
    if let Some(provider) = object.get("provider").and_then(|v| v.as_str()) {
        stored.provider = provider.to_owned();
    }
    if let Some(base_url) = object.get("base_url").and_then(|v| v.as_str()) {
        stored.base_url = base_url.to_owned();
    }
    if let Some(model) = object.get("model").and_then(|v| v.as_str()) {
        stored.model = model.to_owned();
    }
    if let Some(max_tokens) = object.get("max_tokens").and_then(serde_json::Value::as_u64) {
        stored.max_tokens = u32::try_from(max_tokens).unwrap_or(4096);
    }
    Some(stored)
}

fn parse_appearance(value: &serde_json::Value) -> Option<AppearanceSettings> {
    let object = value.as_object()?;
    let mut stored = AppearanceSettings::default();
    if let Some(theme) = object.get("theme").and_then(|v| v.as_str()) {
        // 未知主题名按 `set_theme` 的规则回落深色，不把坏值带进进程。
        stored.set_theme(theme);
    }
    Some(stored)
}

fn write_atomic(path: &Path, bytes: &[u8], private: bool) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension(if private { "key.tmp" } else { "json.tmp" });
    write_file(&temporary, bytes, private)?;
    fs::rename(&temporary, path)
}

#[cfg(unix)]
fn write_file(path: &Path, bytes: &[u8], private: bool) -> io::Result<()> {
    use std::{
        io::Write as _,
        os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    };
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    if private {
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    if private {
        // 已存在的文件不会因为 OpenOptions::mode 改权限：显式收紧一次。
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn write_file(path: &Path, bytes: &[u8], _private: bool) -> io::Result<()> {
    fs::write(path, bytes)
}

/// API Key 的落盘保护：Windows 用 DPAPI 绑定当前用户；其他平台明文 + 0600。
mod secrets {
    use std::io;

    #[cfg(windows)]
    mod platform {
        use std::{io, ptr};
        use windows_sys::Win32::{
            Foundation::LocalFree,
            Security::Cryptography::{
                CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
            },
        };

        fn blob(bytes: &[u8]) -> io::Result<CRYPT_INTEGER_BLOB> {
            Ok(CRYPT_INTEGER_BLOB {
                cbData: u32::try_from(bytes.len())
                    .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "密钥过长"))?,
                pbData: bytes.as_ptr().cast_mut(),
            })
        }

        pub(super) fn protect(plain: &[u8]) -> io::Result<Vec<u8>> {
            let input = blob(plain)?;
            let mut output = CRYPT_INTEGER_BLOB::default();
            let ok = unsafe {
                CryptProtectData(
                    &input,
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(unsafe { take(output) })
        }

        pub(super) fn unprotect(cipher: &[u8]) -> io::Result<Vec<u8>> {
            let input = blob(cipher)?;
            let mut output = CRYPT_INTEGER_BLOB::default();
            let ok = unsafe {
                CryptUnprotectData(
                    &input,
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut output,
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(unsafe { take(output) })
        }

        /// 拷走 DPAPI 返回的缓冲区后 `LocalFree`。
        unsafe fn take(blob: CRYPT_INTEGER_BLOB) -> Vec<u8> {
            let bytes = if blob.pbData.is_null() || blob.cbData == 0 {
                Vec::new()
            } else {
                unsafe { std::slice::from_raw_parts(blob.pbData, blob.cbData as usize) }.to_vec()
            };
            if !blob.pbData.is_null() {
                unsafe {
                    LocalFree(blob.pbData.cast());
                }
            }
            bytes
        }
    }

    #[cfg(not(windows))]
    mod platform {
        use std::io;

        /// 非 Windows 没有 DPAPI：`write_file` 以 0600 落盘，这里只负责往返。
        pub(super) fn protect(plain: &[u8]) -> io::Result<Vec<u8>> {
            Ok(plain.to_vec())
        }

        pub(super) fn unprotect(cipher: &[u8]) -> io::Result<Vec<u8>> {
            Ok(cipher.to_vec())
        }
    }

    pub(super) fn protect(plain: &[u8]) -> io::Result<Vec<u8>> {
        platform::protect(plain)
    }

    pub(super) fn unprotect(cipher: &[u8]) -> io::Result<Vec<u8>> {
        platform::unprotect(cipher)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or_default();
        let root = env::temp_dir().join(format!(
            "rustlua-user-config-{tag}-{}-{nanos}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    /// 非密钥设置往返；未知顶层键与 `assistant` 内的未知键都要活下来，
    /// 且 `editor.json` 里不得出现 Key。
    #[test]
    fn assistant_round_trips_and_keeps_unknown_keys() {
        let root = temp_root("assistant");
        let store = UserConfigStore::at(&root);
        fs::write(
            store.config_path(),
            r#"{"future_pref":7,"assistant":{"provider":"anthropic","model":"old","custom":true}}"#,
        )
        .unwrap();

        let loaded = store.load_assistant().expect("应读到 assistant 段");
        assert_eq!(loaded.provider, "anthropic");
        assert_eq!(loaded.model, "old");

        store
            .save_assistant(&AssistantLayout {
                provider: "ollama".to_owned(),
                base_url: "http://127.0.0.1:11434/v1".to_owned(),
                model: "qwen".to_owned(),
                max_tokens: 2048,
            })
            .unwrap();

        let text = fs::read_to_string(store.config_path()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["version"], CONFIG_VERSION);
        assert_eq!(value["future_pref"], 7, "未知顶层键被抹掉了");
        assert_eq!(value["assistant"]["custom"], true, "未知子键被抹掉了");
        assert_eq!(value["assistant"]["provider"], "ollama");
        assert_eq!(value["assistant"]["max_tokens"], 2048);
        assert!(
            !text.contains("api_key"),
            "非密钥文件里出现了 api_key：{text}"
        );
        let _ = fs::remove_dir_all(root);
    }

    /// 改键与其它用户级设置同住 `editor.json`：写 shortcuts 不抹掉 assistant/appearance
    /// 与未知键；坏键位读回时整条丢弃，未来版本的动作 id 原样保留。
    #[test]
    fn shortcut_bindings_round_trip_alongside_other_sections() {
        let root = temp_root("shortcuts");
        let store = UserConfigStore::at(&root);
        fs::write(
            store.config_path(),
            r#"{"version":1,"appearance":{"theme":"light"},"shortcuts":{"future_action":"Alt+F9","broken":"Ctrl+"}}"#,
        )
        .unwrap();

        let mut bindings = store.load_shortcuts().expect("应读到 shortcuts 段");
        assert_eq!(bindings.get("future_action"), Some("Alt+F9"));
        assert_eq!(bindings.get("broken"), None, "坏键位整条丢弃");

        assert!(bindings.set("undo", "ctrl+shift+u").unwrap());
        store.save_shortcuts(&bindings).unwrap();

        let value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(store.config_path()).unwrap()).unwrap();
        assert_eq!(value["shortcuts"]["undo"], "Ctrl+Shift+U", "落盘是规范写法");
        assert_eq!(
            value["shortcuts"]["future_action"], "Alt+F9",
            "不认识的动作 id 必须原样写回"
        );
        assert_eq!(value["appearance"]["theme"], "light", "其它段不能被抹掉");

        let reloaded = store.load_shortcuts().unwrap();
        assert_eq!(reloaded.get("undo"), Some("Ctrl+Shift+U"));
        let _ = fs::remove_dir_all(root);
    }

    /// 插件开关往返：非布尔值整条丢弃、别的段不被抹掉、不认识的 id 原样留着。
    ///
    /// 这条之所以要紧：插件启停是**用户级**的（换工程也该保持禁用），而 `editor.json`
    /// 里同时住着主题、改键与服务商设置——任何一段把另一段抹掉都是丢用户设置。
    #[test]
    fn plugin_states_round_trip_alongside_other_sections() {
        let root = temp_root("plugin-states");
        let store = UserConfigStore::at(&root);
        fs::write(
            store.config_path(),
            r#"{"version":1,"appearance":{"theme":"light"},"plugins":{"gone_plugin":true,"broken":"yes"}}"#,
        )
        .unwrap();

        let states = store.load_plugin_states().expect("应读到 plugins 段");
        assert_eq!(states.get("gone_plugin"), Some(&true));
        assert_eq!(
            states.get("broken"),
            None,
            "不是布尔值的条目整条丢弃（该插件回落默认）"
        );

        let mut next = states.clone();
        next.insert("project_stats".to_owned(), false);
        next.insert("core".to_owned(), true);
        store.save_plugin_states(&next).unwrap();

        let value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(store.config_path()).unwrap()).unwrap();
        assert_eq!(value["plugins"]["project_stats"], false, "落盘是布尔");
        assert_eq!(value["plugins"]["core"], true);
        assert_eq!(
            value["plugins"]["gone_plugin"], true,
            "本版本不认识的插件 id 必须原样留着"
        );
        assert_eq!(value["appearance"]["theme"], "light", "其它段不能被抹掉");

        let reloaded = store.load_plugin_states().unwrap();
        assert_eq!(reloaded.get("project_stats"), Some(&false));
        let _ = fs::remove_dir_all(root);
    }

    /// 插件面板可见性往返：非布尔值整条丢弃、别的段不被抹掉、不认识的 id 原样留着。
    ///
    /// 与插件开关同一条纪律：这一段和主题/改键住在同一个文件里，任何一段把另一段抹掉
    /// 都是丢用户设置。
    #[test]
    fn panel_states_round_trip_alongside_other_sections() {
        let root = temp_root("panel-states");
        let store = UserConfigStore::at(&root);
        fs::write(
            store.config_path(),
            r#"{"version":1,"plugins":{"core":true},"panels":{"gone.panel":true,"broken":"yes"}}"#,
        )
        .unwrap();

        let states = store.load_panel_states().expect("应读到 panels 段");
        assert_eq!(states.get("gone.panel"), Some(&true));
        assert_eq!(
            states.get("broken"),
            None,
            "不是布尔值的条目整条丢弃（该面板回落默认关闭）"
        );

        let mut next = states.clone();
        next.insert("project_stats.panel".to_owned(), true);
        next.insert("closed.panel".to_owned(), false);
        store.save_panel_states(&next).unwrap();

        let value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(store.config_path()).unwrap()).unwrap();
        assert_eq!(value["panels"]["project_stats.panel"], true, "落盘是布尔");
        assert_eq!(
            value["panels"]["closed.panel"], false,
            "关掉的面板也要落盘：只写可见的那些会让它下次又弹出来"
        );
        assert_eq!(
            value["panels"]["gone.panel"], true,
            "本版本不认识的面板 id 必须原样留着"
        );
        assert_eq!(value["plugins"]["core"], true, "别的段不能被抹掉");

        let reloaded = store.load_panel_states().unwrap();
        assert_eq!(reloaded.get("project_stats.panel"), Some(&true));
        assert_eq!(reloaded.get("closed.panel"), Some(&false));
        let _ = fs::remove_dir_all(root);
    }

    /// 文件缺失/损坏/没有 assistant 段都返回 None，而不是 panic。
    #[test]
    fn missing_or_corrupt_config_falls_back_to_none() {
        let root = temp_root("corrupt");
        let store = UserConfigStore::at(&root);
        assert_eq!(store.load_assistant(), None);

        fs::write(store.config_path(), "{not json").unwrap();
        assert_eq!(store.load_assistant(), None);

        fs::write(store.config_path(), r#"{"version":1}"#).unwrap();
        assert_eq!(store.load_assistant(), None);

        // 单个字段类型不符：只丢那个字段。
        fs::write(
            store.config_path(),
            r#"{"assistant":{"provider":9,"model":"keep-me"}}"#,
        )
        .unwrap();
        let loaded = store.load_assistant().expect("仍应读到 assistant 段");
        assert_eq!(loaded.provider, "openai", "类型不符的字段回落到默认");
        assert_eq!(loaded.model, "keep-me");
        let _ = fs::remove_dir_all(root);
    }

    /// Key 往返：Windows 上磁盘里必须是密文（不含明文子串），空串删除文件。
    #[test]
    fn api_key_round_trips_and_is_not_stored_in_plaintext() {
        let root = temp_root("key");
        let store = UserConfigStore::at(&root);
        assert_eq!(store.load_api_key().unwrap(), None, "没文件就是没填");

        store.save_api_key("  sk-very-secret  ").unwrap();
        assert_eq!(
            store.load_api_key().unwrap(),
            Some("sk-very-secret".to_owned()),
            "首尾空白应被裁掉"
        );
        assert!(
            !store.config_path().exists(),
            "只存 Key 不该造出 editor.json"
        );

        #[cfg(windows)]
        {
            let raw = fs::read(store.api_key_path()).unwrap();
            assert!(
                !raw.windows(b"sk-very-secret".len())
                    .any(|window| window == b"sk-very-secret"),
                "DPAPI 密文里出现了明文密钥"
            );
        }

        store.save_api_key("").unwrap();
        assert!(!store.api_key_path().exists(), "清空 Key 应删除密文文件");
        assert_eq!(store.load_api_key().unwrap(), None);
        let _ = fs::remove_dir_all(root);
    }

    /// 密文损坏时给出可读原因（而不是静默当作没填）。
    #[test]
    fn corrupt_key_file_reports_a_reason() {
        let root = temp_root("badkey");
        let store = UserConfigStore::at(&root);
        fs::write(store.api_key_path(), b"not a dpapi blob").unwrap();
        let error = store.load_api_key().expect_err("损坏的密文必须报错");
        assert!(error.contains("API Key"), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    /// 外观偏好与助手设置同住 `editor.json`：写 appearance 不能抹掉 assistant 与未知键；
    /// 文件里的非法主题名按深色读回。
    #[test]
    fn appearance_round_trips_alongside_assistant() {
        let root = temp_root("appearance");
        let store = UserConfigStore::at(&root);
        fs::write(
            store.config_path(),
            r#"{"future_pref":7,"assistant":{"provider":"ollama"}}"#,
        )
        .unwrap();

        let mut settings = AppearanceSettings::default();
        assert_eq!(settings.normalized_theme(), "dark", "默认深色");
        assert!(settings.set_theme("light"));
        assert!(!settings.set_theme("light"), "重复设置不算变化");
        store.save_appearance(&settings).unwrap();

        assert_eq!(store.load_appearance().unwrap().theme, "light");
        let value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(store.config_path()).unwrap()).unwrap();
        assert_eq!(
            value["assistant"]["provider"], "ollama",
            "写外观不能抹掉助手设置"
        );
        assert_eq!(value["future_pref"], 7, "未知顶层键被抹掉了");

        fs::write(
            store.config_path(),
            r#"{"appearance":{"theme":"solarized","custom":1}}"#,
        )
        .unwrap();
        assert_eq!(
            store.load_appearance().unwrap().theme,
            "dark",
            "未知主题名必须回落到深色"
        );
        let _ = fs::remove_dir_all(root);
    }
}
