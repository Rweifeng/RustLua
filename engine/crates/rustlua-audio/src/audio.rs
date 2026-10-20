//! Minimal sound-effect playback backed by rodio（桌面）/ 平台出口（web、微信）：
//! WAV 片段从工程资产里读，主音量与分组音量各一套，输出设备**懒打开**，
//! 没有声卡的机器上照常跑（播放退化成一条 warning）。
//!
//! M20-02 追加**音量分组**（总线）：BGM / SFX / UI 各自独立调节，实际音量 =
//! 主音量 × 分组音量；设置可经 [`crate::audio::AudioSystem::to_settings_text`] 持久化。
//!
//! M74-01 追加空间化：听者位姿 → 增益/声像的唯一来源在 [`crate::spatial`]（纯函数）。
//! 桌面把 `pan` 折回固定距离喂 `rodio::SpatialSink`；平台出口只应用增益。
//!
//! 平台设备本身住在 [`crate::device`]：桌面是 rodio，wasm 是 no-op stub。这个文件因此
//! 一条 `cfg` 都没有，`rodio` 也不是本 crate 的无条件依赖（docs/62 P1 第 4 条）。

use std::{collections::HashMap, fmt, path::PathBuf};

use crate::device::Device;
use crate::sink::SpatialEmission;
use crate::spatial::{Listener, SpatialError, SpatialModel, SpatialParams, evaluate, fold_to_ears};

/// 音量总线（M20-02）。`Master` 是总音量，其余为分组音量。
///
/// `Ord` 用于设置文件的有序容器（`rustlua-gameplay` 的 settings 里的 `BTreeMap<AudioBus, f32>`）。
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
    /// 空间参数被拒（M74-01）：非有限值、模型不合法、听者朝向退化、或宿主还没接线。
    ///
    /// 为什么不静默降级：`play_at` 的语义是"按位置播"，听者缺失时按原点瞎算衰减，
    /// 症状是"声音莫名很小/全在中间"，且不指向任何一处代码。
    #[error("audio spatial rejected: {0}")]
    Spatial(#[from] SpatialError),
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
    /// 平台输出设备（[`crate::device`]）：桌面是 rodio，wasm 是 no-op stub。
    /// 设备的细节（stream/sink）全在那一层，这里只按"开没开、播什么"调用。
    device: Device,
    /// 听者位姿（M74-01）：宿主每帧从相机灌入；`None` = 还没接线，`play_at` 会拒绝。
    listener: Option<Listener>,
    /// 距离衰减模型（M74-01），默认 [`SpatialModel::DEFAULT`]。
    spatial_model: SpatialModel,
    /// 当前 BGM 的解码字节（供曲尾续播重挂；换曲/停止时更新）。
    music_bytes: Option<Vec<u8>>,
}

impl fmt::Debug for AudioSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioSystem")
            .field("cached_sounds", &self.sounds.len())
            .field("active_sinks", &self.device.sink_count())
            .field("spatial_sinks", &self.device.spatial_count())
            .field("listener", &self.listener.is_some())
            .field("volume", &self.volume)
            .field("bgm_volume", &self.bus_volume(AudioBus::Bgm))
            .field("sfx_volume", &self.bus_volume(AudioBus::Sfx))
            .field("ui_volume", &self.bus_volume(AudioBus::Ui))
            .field("device_open", &self.device.is_open())
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
            device: Device::new(),
            listener: None,
            spatial_model: SpatialModel::DEFAULT,
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
        // 装了平台音频出口就**整段交给它**：wasm 上既没有可读文件系统也没有 cpal 设备，
        // 下面那条"读字节 → 解码 → 送设备"的路在那里必然失败（见 `sink` 模块文档）。
        // 平台自己知道怎么用 URL 加载（浏览器 `HTMLAudioElement` / 微信 `InnerAudioContext`）。
        if crate::sink::is_installed() {
            return self.play_via_sink(bus, name, false, None);
        }
        let bytes = self.sound_bytes(name)?;
        let volume = self.effective_volume(bus);
        self.ensure_device();
        if !self.device.is_open() {
            tracing::debug!(target: "audio", name, bus = bus.key(), "[audio] no device; skipped");
            return Ok(());
        }
        self.device.play(bytes, volume)?;
        tracing::info!(target: "audio", name, bus = bus.key(), "[audio] played");
        Ok(())
    }

    /// 设置听者位姿（M74-01）：宿主**每帧**调用一次（通常直接来自活动相机）。
    ///
    /// 为什么不做成"每次 `play_at` 传听者"：那会把同一份位姿抄到 120 个示例与所有调用点，
    /// 而且漏传一处的症状是"这个音效的位置怪怪的"，非常难查。
    pub fn set_listener(&mut self, listener: Listener) {
        self.listener = Some(listener);
    }

    /// 当前听者位姿；`None` = 宿主还没接线。
    #[must_use]
    pub fn listener(&self) -> Option<Listener> {
        self.listener
    }

    /// 设置距离衰减模型（M74-01）。非法模型当场拒绝（不夹紧）。
    pub fn set_spatial_model(&mut self, model: SpatialModel) -> Result<(), AudioError> {
        // 用一次"与听者同点"的求值来校验模型本身（其余字段都合法，只有模型可能不合法）。
        evaluate([0.0; 3], &Listener::at_origin(), model)?;
        self.spatial_model = model;
        Ok(())
    }

    /// 当前距离衰减模型。
    #[must_use]
    pub fn spatial_model(&self) -> SpatialModel {
        self.spatial_model
    }

    /// 在 SFX 总线上按世界坐标播放（M74-01），返回本次的增益与声像。
    pub fn play_at(&mut self, name: &str, emitter: [f32; 3]) -> Result<SpatialParams, AudioError> {
        self.play_at_on(AudioBus::Sfx, name, emitter)
    }

    /// 在指定总线上按世界坐标播放（M74-01），返回本次的增益与声像。
    ///
    /// 两条平台路径：
    /// - 桌面（无平台出口）：`rodio::SpatialSink`，位置由 [`fold_to_ears`] 从 `pan` 折回
    ///   （距离恒定 ⇒ rodio 自己的 `1/dist²` 不参与，衰减只由我们算的 `gain` 决定）。
    /// - web/微信：把 `gain` 乘进音量交给平台，`pan` 原样放进 [`SpatialEmission`] 供有能力
    ///   的平台使用（当前两个 provider 只有音量，会显式记一条"声像未应用"）。
    pub fn play_at_on(
        &mut self,
        bus: AudioBus,
        name: &str,
        emitter: [f32; 3],
    ) -> Result<SpatialParams, AudioError> {
        let listener = self.listener.ok_or(SpatialError::NoListener)?;
        let params = evaluate(emitter, &listener, self.spatial_model)?;
        let spatial = SpatialEmission {
            gain: params.gain,
            pan: params.pan,
        };
        if crate::sink::is_installed() {
            self.play_via_sink(bus, name, false, Some(spatial))?;
            return Ok(params);
        }
        let bytes = self.sound_bytes(name)?;
        self.ensure_device();
        if !self.device.is_open() {
            tracing::debug!(target: "audio", name, bus = bus.key(), "[audio] no device; skipped (spatial)");
            return Ok(params);
        }
        let (left_ear, right_ear, folded) = fold_to_ears(&listener, params.pan);
        self.device.play_spatial(
            bytes,
            self.effective_volume(bus) * params.gain,
            folded,
            left_ear,
            right_ear,
        )?;
        tracing::info!(
            target: "audio",
            name,
            bus = bus.key(),
            gain = params.gain,
            pan = params.pan,
            "[audio] played (spatial)"
        );
        Ok(params)
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
        // 与 `play_on` 同一条分叉：装了平台出口就交给平台（它自己会循环）。
        if crate::sink::is_installed() {
            return self.play_via_sink(AudioBus::Bgm, name, true, None);
        }
        let bytes = self.sound_bytes(name)?;
        self.ensure_device();
        self.music_bytes = None;
        if !self.device.is_open() {
            tracing::debug!(target: "audio", name, "[audio] no device; music skipped");
            return Ok(());
        }
        self.device
            .play_music(&bytes, self.effective_volume(AudioBus::Bgm))?;
        self.music_bytes = Some(bytes);
        tracing::info!(target: "audio", name, "[audio] music started (refill loop)");
        Ok(())
    }

    /// 把一次播放交给平台出口（[`crate::sink`]）。
    ///
    /// 失败一律记成 [`AudioError::Device`]：`play_on` 的既有契约里"设备/平台这类问题"就是它
    /// （缺文件才是 `Io`）。这里刻意**不新增错误变体**——`AudioError` 是公开枚举，
    /// 加变体等于改公共面，而这一层目前没有调用方需要区分"平台失败"与"设备失败"。
    fn play_via_sink(
        &mut self,
        bus: AudioBus,
        name: &str,
        looping: bool,
        spatial: Option<SpatialEmission>,
    ) -> Result<(), AudioError> {
        // 空间增益在这里乘进音量：`AudioRequest.volume` 的契约就是"已算好的有效音量"，
        // 平台不需要知道空间模型（它也无法知道）。
        let gain = spatial.map_or(1.0, |emission| emission.gain);
        let volume = self.effective_volume(bus) * gain;
        let request = crate::sink::AudioRequest {
            name,
            bus,
            volume,
            asset_dir: self.asset_dir.as_deref(),
            looping,
            spatial,
        };
        crate::sink::play(request).map_err(|error| {
            tracing::warn!(target: "audio", name, bus = bus.key(), error = %error, "[audio] platform sink failed");
            AudioError::Device(format!("platform audio sink: {error}"))
        })
    }

    /// 曲尾续播钩子：宿主**每帧**调用一次。当前 BGM 自然播完（`Sink::empty`）后
    /// 把缓存的解码字节重新包一个 decoder 挂进同一个 sink，实现无限循环。
    ///
    /// 无曲目/无设备时是纯 no-op；续播解码失败（曲本已被替换等）只 warn 一次并
    /// 放弃循环，避免每帧刷屏。暂停中的 sink 不会走到 empty（`Pausable` 暂停时
    /// 产静音采样、不结束），因此暂停→恢复不会触发续播、也不会丢曲目。
    pub fn maintain_music(&mut self) {
        let Some(bytes) = self.music_bytes.clone() else {
            return;
        };
        if !self.device.music_finished() {
            return;
        }
        if let Err(error) = self.device.refill_music(&bytes) {
            tracing::warn!(target: "audio", error = %error, "[audio] music refill failed; loop ended");
            self.music_bytes = None;
        }
    }

    /// 把当前 BGM 总线音量应用到常驻音乐 sink（分组/主音量变更时调用）。
    fn apply_music_volume(&mut self) {
        self.device
            .set_music_volume(self.effective_volume(AudioBus::Bgm));
    }

    /// Stops any looping background music.
    pub fn stop_music(&mut self) {
        // 装了平台出口就交给它：桌面设备在 wasm 上根本不存在（`device` 是 no-op stub），
        // 于是切图时的 `audio.stop_music()` 会退化成 no-op —— 旧 BGM 继续响，比"没声音"更难查。
        if crate::sink::is_installed() {
            crate::sink::stop_music();
            self.music_bytes = None;
            return;
        }
        self.music_bytes = None;
        self.device.stop_music();
    }
    /// Pauses the looping background music（编辑器暂停时调用；不释放 sink，恢复时继续）。
    pub fn pause_music(&self) {
        if crate::sink::is_installed() {
            crate::sink::pause_music();
            return;
        }
        self.device.pause_music();
    }

    /// Resumes a previously paused looping background music sink。
    pub fn resume_music(&self) {
        if crate::sink::is_installed() {
            crate::sink::resume_music();
            return;
        }
        self.device.resume_music();
    }

    /// Opens the output device on first use; degrades to a warning without one.
    ///
    /// 不再返回 `Result`：这条路径从来没有失败过（打不开设备就降级成"静默跳过"），
    /// 之前那个 `Ok(())` 只是让每个调用点多写一个 `?`。设备层的失败处理在
    /// [`crate::device`] 里（桌面记 warning，wasm 记一条"没有桌面设备"）。
    fn ensure_device(&mut self) {
        self.device.open();
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
            // 走 `vfs::read_resource`（provider 优先 + 解包）——发布包形态下音频在 RLB 容器里，
            // `path.is_file()` + `fs::read` 会把它判成"没有这个音效"并且**不报错**，
            // 症状是"所有音效静默不播"（`docs/67` §8）。明文/loose 形态行为不变。
            if !rustlua_infra::vfs::is_file(&path) {
                continue;
            }
            let bytes = rustlua_infra::vfs::read_resource(&path)
                .map_err(|error| AudioError::Io(format!("{}: {error}", path.display())))?;
            loaded = Some(bytes);
            break;
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
            !audio.device.is_open(),
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

    /// M74-01：没接线就拒绝（而不是按原点瞎衰减）。
    #[test]
    fn play_at_without_a_listener_is_rejected() {
        let mut audio = AudioSystem::new();
        audio.set_asset_dir(std::env::temp_dir());
        let error = audio.play_at("rustlua-audio-definitely-missing", [1.0, 0.0, 0.0]);
        assert!(
            matches!(error, Err(AudioError::Spatial(SpatialError::NoListener))),
            "得到 {error:?}"
        );
        // 仍然没有开设备：拒绝发生在读文件之前。
        assert!(!audio.device.is_open());
    }

    /// M74-01：接线之后走的是"按位置播放"，缺文件仍然是 `Io`（不是静默成功）。
    #[test]
    fn play_at_with_listener_still_reports_missing_clip() {
        let mut audio = AudioSystem::new();
        audio.set_asset_dir(std::env::temp_dir());
        audio.set_listener(Listener::at_origin());
        let error = audio.play_at("rustlua-audio-definitely-missing", [0.0, 0.0, -5.0]);
        assert!(matches!(error, Err(AudioError::Io(_))), "得到 {error:?}");
    }

    /// M74-01：模型非法当场拒绝；合法模型可读回。
    #[test]
    fn spatial_model_is_validated_and_read_back() {
        let mut audio = AudioSystem::new();
        assert_eq!(audio.spatial_model(), SpatialModel::DEFAULT);
        let error = audio.set_spatial_model(SpatialModel {
            min_distance: 30.0,
            max_distance: 10.0,
        });
        assert!(
            matches!(error, Err(AudioError::Spatial(_))),
            "得到 {error:?}"
        );
        assert_eq!(
            audio.spatial_model(),
            SpatialModel::DEFAULT,
            "拒绝后保持原值"
        );
        audio
            .set_spatial_model(SpatialModel {
                min_distance: 2.0,
                max_distance: 50.0,
            })
            .unwrap();
        assert_eq!(audio.spatial_model().max_distance, 50.0);
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
