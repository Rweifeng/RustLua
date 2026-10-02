//! Minimal sound-effect playback backed by rodio: WAV clips loaded from the
//! project assets, a master volume, and a lazily-opened output device so games
//! run fine on machines without audio hardware (play degrades to a warning).
//!
//! M20-02 追加**音量分组**（总线）：BGM / SFX / UI 各自独立调节，实际音量 =
//! 主音量 × 分组音量；设置可经 [`crate::audio::AudioSystem::to_settings_text`] 持久化。

use std::{collections::HashMap, fmt, io::Cursor, path::PathBuf};

use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};

/// 音量总线（M20-02）。`Master` 是总音量，其余为分组音量。
///
/// `Ord` 用于设置文件的有序容器（`core/settings.rs` 的 `BTreeMap<AudioBus, f32>`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AudioBus {
    Master,
    Bgm,
    Sfx,
    Ui,
}

impl AudioBus {
    /// 全部总线（设置面板遍历用）。
    pub const ALL: [Self; 4] = [Self::Master, Self::Bgm, Self::Sfx, Self::Ui];

    /// 稳定文本键（设置持久化用）。
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Master => "master",
            Self::Bgm => "bgm",
            Self::Sfx => "sfx",
            Self::Ui => "ui",
        }
    }

    /// 从文本键解析。
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "master" => Some(Self::Master),
            "bgm" => Some(Self::Bgm),
            "sfx" => Some(Self::Sfx),
            "ui" => Some(Self::Ui),
            _ => None,
        }
    }
}

/// 音频设置文本的版本头。
pub const AUDIO_SETTINGS_VERSION: &str = "rustlua-audio-v1";

/// Errors produced while configuring or playing audio.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("audio I/O failed: {0}")]
    Io(String),
    #[error("audio device failed: {0}")]
    Device(String),
    #[error("audio decode failed: {0}")]
    Decode(String),
    /// 音量设置文本版本不符（M20-02）。
    #[error("audio settings rejected: {0}")]
    Settings(String),
}

/// Plays named WAV effects from an asset directory. The output device is
/// opened on first play and kept alive; finished sinks are reaped so the
/// underlying mixer slots are recycled.
pub struct AudioSystem {
    asset_dir: Option<PathBuf>,
    sounds: HashMap<String, Vec<u8>>,
    volume: f32,
    /// 分组音量（M20-02）：主音量之外的 BGM/SFX/UI 各自可调。
    bus_volumes: HashMap<AudioBus, f32>,
    stream: Option<OutputStream>,
    handle: Option<OutputStreamHandle>,
    sinks: Vec<Sink>,
    music_sink: Option<Sink>,
    /// 当前 BGM 的解码字节（供曲尾续播重挂；换曲/停止时更新）。
    music_bytes: Option<Vec<u8>>,
}

impl fmt::Debug for AudioSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioSystem")
            .field("cached_sounds", &self.sounds.len())
            .field("active_sinks", &self.sinks.len())
            .field("volume", &self.volume)
            .field("bgm_volume", &self.bus_volume(AudioBus::Bgm))
            .field("sfx_volume", &self.bus_volume(AudioBus::Sfx))
            .field("ui_volume", &self.bus_volume(AudioBus::Ui))
            .field("device_open", &self.stream.is_some())
            .finish()
    }
}

impl Default for AudioSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioSystem {
    /// Creates an audio system with no device open and no asset directory.
    #[must_use]
    pub fn new() -> Self {
        Self {
            asset_dir: None,
            sounds: HashMap::new(),
            volume: 1.0,
            bus_volumes: HashMap::new(),
            stream: None,
            handle: None,
            sinks: Vec::new(),
            music_sink: None,
            music_bytes: None,
        }
    }

    /// Sets the directory WAV clips are loaded from (usually `<project>/assets`).
    pub fn set_asset_dir(&mut self, dir: PathBuf) {
        self.asset_dir = Some(dir);
    }

    /// Sets the master volume in `0..=1`.
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        self.apply_music_volume();
    }

    /// Returns the master volume.
    #[must_use]
    pub fn volume(&self) -> f32 {
        self.volume
    }

    /// 设置某个总线的音量（`0..=1`）；`Master` 等价于 [`Self::set_volume`]。
    pub fn set_bus_volume(&mut self, bus: AudioBus, volume: f32) {
        let clamped = volume.clamp(0.0, 1.0);
        if bus == AudioBus::Master {
            self.volume = clamped;
        } else {
            self.bus_volumes.insert(bus, clamped);
        }
        // BGM 是常驻 sink，改音量要立刻生效（SFX/UI 在播放时取快照）。
        self.apply_music_volume();
    }

    /// 返回某个总线的独立音量（不含主音量；`Master` 即主音量）。
    #[must_use]
    pub fn bus_volume(&self, bus: AudioBus) -> f32 {
        if bus == AudioBus::Master {
            return self.volume;
        }
        self.bus_volumes.get(&bus).copied().unwrap_or(1.0)
    }

    /// 实际生效音量 = 主音量 × 分组音量。
    #[must_use]
    pub fn effective_volume(&self, bus: AudioBus) -> f32 {
        if bus == AudioBus::Master {
            return self.volume;
        }
        self.volume * self.bus_volume(bus)
    }

    /// 音量设置持久化文本（含版本头）。
    #[must_use]
    pub fn to_settings_text(&self) -> String {
        let mut text = String::from(AUDIO_SETTINGS_VERSION);
        for bus in AudioBus::ALL {
            text.push('\n');
            text.push_str(bus.key());
            text.push('=');
            text.push_str(&format!("{:.3}", self.bus_volume(bus)));
        }
        text
    }

    /// 应用音量设置文本；版本头不符返回错误，非法行忽略（保持可用）。
    pub fn apply_settings_text(&mut self, text: &str) -> Result<(), AudioError> {
        let mut lines = text.lines();
        let header = lines.next().unwrap_or_default().trim();
        if header != AUDIO_SETTINGS_VERSION {
            return Err(AudioError::Settings(format!(
                "unsupported audio settings version: {header}"
            )));
        }
        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let Some(bus) = AudioBus::from_key(key.trim()) else {
                continue;
            };
            if let Ok(volume) = value.trim().parse::<f32>() {
                self.set_bus_volume(bus, volume);
            }
        }
        Ok(())
    }

    /// 播放音效（SFX 总线）。
    pub fn play(&mut self, name: &str) -> Result<(), AudioError> {
        self.play_on(AudioBus::Sfx, name)
    }

    /// 在指定总线上播放一次音效（UI 点击音等用 [`AudioBus::Ui`]）。
    ///
    /// 返回 `Ok(())` 表示**播放请求已被接受**：无输出设备时静默降级（M19-02
    /// 策略，只记日志），不视为失败；只有缺文件/解码失败等才返回错误。
    pub fn play_on(&mut self, bus: AudioBus, name: &str) -> Result<(), AudioError> {
        let bytes = self.sound_bytes(name)?;
        let volume = self.effective_volume(bus);
        self.ensure_device()?;
        let Some(handle) = self.handle.as_ref() else {
            tracing::debug!(target: "audio", name, bus = bus.key(), "[audio] no device; skipped");
            return Ok(());
        };
        let sink = Sink::try_new(handle).map_err(|error| AudioError::Device(error.to_string()))?;
        let source = Decoder::new(Cursor::new(bytes))
            .map_err(|error| AudioError::Decode(error.to_string()))?;
        sink.set_volume(volume);
        sink.append(source);
        self.sinks.push(sink);
        self.sinks.retain(|sink| !sink.empty());
        tracing::info!(target: "audio", name, bus = bus.key(), "[audio] played");
        Ok(())
    }

    /// 循环播放 `name` 作为 BGM，替换当前曲目。
    ///
    /// 循环方式是"**单次 append + 宿主曲尾续播**"（见 [`Self::maintain_music`]），
    /// 刻意不用 `Source::repeat_infinite()`：rodio 0.19 的循环源套在 `Buffered` 里，
    /// 被 sink 的消费线程判定为"已播完"（实测起播 ~3s 后 `Sink::empty()` 即为真），
    /// 表现为日志 `music started` 但完全无声；而普通 decoder 源（与音效同一路径）
    /// 实测正常。曲尾续播的间隙只有一个宿主帧周期（毫秒级），听感上不可辨。
    ///
    /// 与 [`Self::play_on`] 一致：无输出设备时静默降级（不报错），缺文件才失败。
    pub fn play_music(&mut self, name: &str) -> Result<(), AudioError> {
        let bytes = self.sound_bytes(name)?;
        self.ensure_device()?;
        // 停掉当前音乐再起新曲目。
        if let Some(music) = self.music_sink.take() {
            music.stop();
        }
        self.music_bytes = None;
        let Some(handle) = self.handle.as_ref() else {
            tracing::debug!(target: "audio", name, "[audio] no device; music skipped");
            return Ok(());
        };
        let sink = Sink::try_new(handle).map_err(|error| AudioError::Device(error.to_string()))?;
        let source = Decoder::new(Cursor::new(bytes.clone()))
            .map_err(|error| AudioError::Decode(error.to_string()))?;
        sink.set_volume(self.effective_volume(AudioBus::Bgm));
        sink.append(source);
        self.music_bytes = Some(bytes);
        self.music_sink = Some(sink);
        tracing::info!(target: "audio", name, "[audio] music started (refill loop)");
        Ok(())
    }

    /// 曲尾续播钩子：宿主**每帧**调用一次。当前 BGM 自然播完（`Sink::empty`）后
    /// 把缓存的解码字节重新包一个 decoder 挂进同一个 sink，实现无限循环。
    ///
    /// 无曲目/无设备时是纯 no-op；续播解码失败（曲本已被替换等）只 warn 一次并
    /// 放弃循环，避免每帧刷屏。暂停中的 sink 不会走到 empty（`Pausable` 暂停时
    /// 产静音采样、不结束），因此暂停→恢复不会触发续播、也不会丢曲目。
    pub fn maintain_music(&mut self) {
        let (Some(sink), Some(bytes)) = (self.music_sink.as_ref(), self.music_bytes.as_ref())
        else {
            return;
        };
        if !sink.empty() {
            return;
        }
        match Decoder::new(Cursor::new(bytes.clone())) {
            Ok(source) => sink.append(source),
            Err(error) => {
                tracing::warn!(target: "audio", error = %error, "[audio] music refill failed; loop ended");
                self.music_bytes = None;
            }
        }
    }

    /// 把当前 BGM 总线音量应用到常驻音乐 sink（分组/主音量变更时调用）。
    fn apply_music_volume(&mut self) {
        if let Some(music) = self.music_sink.as_ref() {
            music.set_volume(self.effective_volume(AudioBus::Bgm));
        }
    }

    /// Stops any looping background music.
    pub fn stop_music(&mut self) {
        self.music_bytes = None;
        if let Some(music) = self.music_sink.take() {
            music.stop();
            tracing::info!(target: "audio", "[audio] music stopped");
        }
    }
    /// Pauses the looping background music（编辑器暂停时调用；不释放 sink，恢复时继续）。
    pub fn pause_music(&self) {
        if let Some(music) = self.music_sink.as_ref() {
            music.pause();
            tracing::debug!(target: "audio", "[audio] music paused");
        }
    }

    /// Resumes a previously paused looping background music sink。
    pub fn resume_music(&self) {
        if let Some(music) = self.music_sink.as_ref() {
            music.play();
            tracing::debug!(target: "audio", "[audio] music resumed");
        }
    }

    /// Opens the output device on first use; degrades to a warning without one.
    fn ensure_device(&mut self) -> Result<(), AudioError> {
        if self.stream.is_some() {
            return Ok(());
        }
        match OutputStream::try_default() {
            Ok((stream, handle)) => {
                self.stream = Some(stream);
                self.handle = Some(handle);
                Ok(())
            }
            Err(error) => {
                tracing::warn!(target: "audio", error = %error, "[audio] no output device; audio disabled");
                Ok(())
            }
        }
    }

    fn sound_bytes(&mut self, name: &str) -> Result<Vec<u8>, AudioError> {
        if let Some(bytes) = self.sounds.get(name) {
            return Ok(bytes.clone());
        }
        let Some(dir) = &self.asset_dir else {
            return Err(AudioError::Io("no asset directory configured".to_owned()));
        };
        // Try `.mp3` first, then `.wav` / `.ogg` (rodio decodes all three from bytes).
        let mut loaded = None;
        for file in [
            format!("{name}.mp3"),
            format!("{name}.wav"),
            format!("{name}.ogg"),
        ] {
            let path = dir.join(&file);
            if path.is_file() {
                let bytes = std::fs::read(&path)
                    .map_err(|error| AudioError::Io(format!("{}: {error}", path.display())))?;
                let bytes = rustlua_infra::pack::decode(&bytes)
                    .map_err(|error| AudioError::Decode(format!("{}: {error}", path.display())))?;
                loaded = Some(bytes);
                break;
            }
        }
        let bytes = loaded
            .ok_or_else(|| AudioError::Io(format!("no .mp3/.wav/.ogg clip found for {name}")))?;
        self.sounds.insert(name.to_owned(), bytes.clone());
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_is_clamped_and_reported() {
        let mut audio = AudioSystem::new();
        audio.set_volume(1.7);
        assert_eq!(audio.volume(), 1.0);
        audio.set_volume(-0.2);
        assert_eq!(audio.volume(), 0.0);
    }

    #[test]
    fn missing_clip_returns_io_error_without_opening_a_device() {
        let mut audio = AudioSystem::new();
        audio.set_asset_dir(std::env::temp_dir());
        // loading fails before any device is opened
        assert!(audio.play("rustlua-audio-definitely-missing").is_err());
        assert!(
            audio.stream.is_none(),
            "device must not open on a failed load"
        );
    }

    #[test]
    fn bus_volumes_are_independent_and_multiply_with_master() {
        let mut audio = AudioSystem::new();
        // 默认全 1.0。
        for bus in AudioBus::ALL {
            assert_eq!(audio.bus_volume(bus), 1.0, "bus {bus:?} default");
        }
        // 分组独立：调 BGM 不影响 SFX/UI。
        audio.set_bus_volume(AudioBus::Bgm, 0.4);
        assert_eq!(audio.bus_volume(AudioBus::Bgm), 0.4);
        assert_eq!(audio.bus_volume(AudioBus::Sfx), 1.0);
        assert_eq!(audio.bus_volume(AudioBus::Ui), 1.0);
        // 实际音量 = 主 × 分组。
        audio.set_volume(0.5);
        assert!((audio.effective_volume(AudioBus::Bgm) - 0.2).abs() < 1e-6);
        assert!((audio.effective_volume(AudioBus::Sfx) - 0.5).abs() < 1e-6);
        // 钳制与 Master 语义。
        audio.set_bus_volume(AudioBus::Ui, 2.0);
        assert_eq!(audio.bus_volume(AudioBus::Ui), 1.0);
        audio.set_bus_volume(AudioBus::Master, 0.25);
        assert_eq!(audio.volume(), 0.25);
        assert!((audio.effective_volume(AudioBus::Bgm) - 0.1).abs() < 1e-6);
    }

    #[test]
    fn audio_settings_round_trip_and_reject_unknown_version() {
        let mut audio = AudioSystem::new();
        audio.set_volume(0.8);
        audio.set_bus_volume(AudioBus::Bgm, 0.3);
        audio.set_bus_volume(AudioBus::Sfx, 0.9);
        audio.set_bus_volume(AudioBus::Ui, 0.6);
        let text = audio.to_settings_text();
        assert!(text.starts_with(AUDIO_SETTINGS_VERSION));

        let mut restored = AudioSystem::new();
        restored.apply_settings_text(&text).unwrap();
        assert!((restored.volume() - 0.8).abs() < 1e-3);
        assert!((restored.bus_volume(AudioBus::Bgm) - 0.3).abs() < 1e-3);
        assert!((restored.bus_volume(AudioBus::Sfx) - 0.9).abs() < 1e-3);
        assert!((restored.bus_volume(AudioBus::Ui) - 0.6).abs() < 1e-3);

        // 版本不符 → 明确错误；非法行被忽略而非panic。
        assert!(matches!(
            restored.apply_settings_text("rustlua-audio-v0\nbgm=0.5\n"),
            Err(AudioError::Settings(_))
        ));
        let mut tolerant = AudioSystem::new();
        tolerant
            .apply_settings_text("rustlua-audio-v1\nunknown=0.5\nbgm=not-a-number\n")
            .unwrap();
        assert_eq!(tolerant.bus_volume(AudioBus::Bgm), 1.0);
    }
}
