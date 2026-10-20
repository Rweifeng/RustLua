//! 输出设备：桌面走 rodio；其它目标没有桌面设备，由宿主装平台出口。
//!
//! 这一层是 **docs/62 P1 第 4 条（"`rustlua-audio` 的桌面/浏览器适配"）** 的落点：
//!
//! - `rodio` 从 `[dependencies]` 挪进了
//!   `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`，所以 **wasm 产物里根本没有
//!   cpal/rodio**（此前它是无条件依赖，web 构建照编，白背一份桌面音频栈）；
//! - 两份实现共用同一组方法，于是 `audio.rs` 里一条 `cfg` 都不用写，读代码时不用在两套
//!   后端之间来回跳。
//!
//! 为什么不用 feature 开关：feature 在**同一次构建内所有 target 合并**（`cargo build
//! --workspace` 时 `game_web` 打开的 feature 会连带让别的 target 也拿到桌面后端），
//! target 条件依赖不会 —— 这正是"平台适配不该靠 feature 猜"的原因。
//!
//! 平台出口优先：`crate::sink::is_installed()` 为真时 `AudioSystem` **不会**走到这里
//! （浏览器 `HTMLAudioElement` / 微信 `InnerAudioContext` 自己知道怎么用 URL 加载）。
//! 所以 wasm 上的 [stub](self) 只在"宿主忘了装出口"时被碰到，行为是**记一条 warn 并跳过**，
//! 与桌面"没有输出设备"时同一条降级路径。

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use crate::audio::AudioError;
    use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, SpatialSink};
    use std::io::Cursor;

    /// 桌面实现：rodio 的输出流 + 若干个 sink。
    #[derive(Default)]
    pub(crate) struct Device {
        stream: Option<OutputStream>,
        handle: Option<OutputStreamHandle>,
        sinks: Vec<Sink>,
        /// 空间化的 sink（M74-01）。与 `sinks` 分开存：`SpatialSink` 是另一种类型，
        /// 回收判据一样（`empty()`），但位置参数在播放期间靠它自己持有。
        spatial_sinks: Vec<SpatialSink>,
        music_sink: Option<Sink>,
    }

    impl Device {
        pub(crate) fn new() -> Self {
            Self::default()
        }

        /// 打开默认输出设备（幂等；失败只记 warning —— 没声音不该让游戏起不来）。
        pub(crate) fn open(&mut self) {
            if self.stream.is_some() {
                return;
            }
            match OutputStream::try_default() {
                Ok((stream, handle)) => {
                    self.stream = Some(stream);
                    self.handle = Some(handle);
                }
                Err(error) => {
                    tracing::warn!(target: "audio", error = %error, "[audio] no output device; audio disabled");
                }
            }
        }

        pub(crate) fn is_open(&self) -> bool {
            self.stream.is_some()
        }

        pub(crate) fn sink_count(&self) -> usize {
            self.sinks.len()
        }

        pub(crate) fn spatial_count(&self) -> usize {
            self.spatial_sinks.len()
        }

        /// 播放一次性音效。设备没开时是 no-op（调用方已在外面判过 `is_open`，这里只是兜底）。
        pub(crate) fn play(&mut self, bytes: Vec<u8>, volume: f32) -> Result<(), AudioError> {
            let Some(handle) = self.handle.as_ref() else {
                return Ok(());
            };
            let sink =
                Sink::try_new(handle).map_err(|error| AudioError::Device(error.to_string()))?;
            let source = Decoder::new(Cursor::new(bytes))
                .map_err(|error| AudioError::Decode(error.to_string()))?;
            sink.set_volume(volume);
            sink.append(source);
            self.sinks.push(sink);
            // 播完的 sink 回收掉，否则底层混音槽位会一直涨。
            self.sinks.retain(|sink| !sink.empty());
            Ok(())
        }

        /// 按位置播放（M74-01 桌面路径）：`folded`/`left`/`right` 已由
        /// [`crate::spatial::fold_to_ears`] 从 `pan` 折算好，这里只负责建 `SpatialSink`。
        pub(crate) fn play_spatial(
            &mut self,
            bytes: Vec<u8>,
            volume: f32,
            folded: [f32; 3],
            left: [f32; 3],
            right: [f32; 3],
        ) -> Result<(), AudioError> {
            let Some(handle) = self.handle.as_ref() else {
                return Ok(());
            };
            let sink = SpatialSink::try_new(handle, folded, left, right)
                .map_err(|error| AudioError::Device(error.to_string()))?;
            let source = Decoder::new(Cursor::new(bytes))
                .map_err(|error| AudioError::Decode(error.to_string()))?;
            sink.set_volume(volume);
            sink.append(source);
            self.spatial_sinks.push(sink);
            self.spatial_sinks.retain(|sink| !sink.empty());
            Ok(())
        }

        /// 起一首常驻 BGM（先停掉当前曲目）。循环靠"单次 append + 宿主曲尾续播"，
        /// 刻意不用 `Source::repeat_infinite()`：rodio 0.19 的循环源套在 `Buffered` 里
        /// 会被 sink 判成"已播完"（实测起播 ~3s 后 `empty()` 即为真，表现为完全无声）。
        pub(crate) fn play_music(&mut self, bytes: &[u8], volume: f32) -> Result<(), AudioError> {
            if let Some(music) = self.music_sink.take() {
                music.stop();
            }
            let Some(handle) = self.handle.as_ref() else {
                return Ok(());
            };
            let sink =
                Sink::try_new(handle).map_err(|error| AudioError::Device(error.to_string()))?;
            let source = Decoder::new(Cursor::new(bytes.to_vec()))
                .map_err(|error| AudioError::Decode(error.to_string()))?;
            sink.set_volume(volume);
            sink.append(source);
            self.music_sink = Some(sink);
            Ok(())
        }

        /// 常驻 BGM 是否已经播完（曲尾续播的判据）。没有曲目时为 `false`。
        pub(crate) fn music_finished(&self) -> bool {
            self.music_sink.as_ref().is_some_and(|sink| sink.empty())
        }

        pub(crate) fn refill_music(&mut self, bytes: &[u8]) -> Result<(), AudioError> {
            let Some(sink) = self.music_sink.as_ref() else {
                return Ok(());
            };
            let source = Decoder::new(Cursor::new(bytes.to_vec()))
                .map_err(|error| AudioError::Decode(error.to_string()))?;
            sink.append(source);
            Ok(())
        }

        pub(crate) fn set_music_volume(&self, volume: f32) {
            if let Some(music) = self.music_sink.as_ref() {
                music.set_volume(volume);
            }
        }

        pub(crate) fn stop_music(&mut self) {
            if let Some(music) = self.music_sink.take() {
                music.stop();
                tracing::info!(target: "audio", "[audio] music stopped");
            }
        }

        pub(crate) fn pause_music(&self) {
            if let Some(music) = self.music_sink.as_ref() {
                music.pause();
                tracing::debug!(target: "audio", "[audio] music paused");
            }
        }

        pub(crate) fn resume_music(&self) {
            if let Some(music) = self.music_sink.as_ref() {
                music.play();
                tracing::debug!(target: "audio", "[audio] music resumed");
            }
        }
    }
}

/// wasm/微信没有桌面设备：留一组同形状的 no-op，让 `audio.rs` 不必写 cfg。
///
/// 正常路径上它根本不会被用到 —— 平台出口（`crate::sink` 的 `AudioSinkProvider`）
/// 装了的话，`AudioSystem` 在更早的分叉就把请求交出去了。真走到这里说明**宿主忘了装出口**，
/// 那就明确记一条 warn 再跳过，而不是假装播过了。
#[cfg(target_arch = "wasm32")]
mod imp {
    use crate::audio::AudioError;

    #[derive(Default)]
    pub(crate) struct Device;

    impl Device {
        pub(crate) fn new() -> Self {
            Self
        }

        pub(crate) fn open(&mut self) {
            tracing::warn!(
                target: "audio",
                "[audio] 本目标没有桌面音频设备（wasm/微信请由宿主装 AudioSinkProvider）；请求被跳过"
            );
        }

        pub(crate) fn is_open(&self) -> bool {
            false
        }

        pub(crate) fn sink_count(&self) -> usize {
            0
        }

        pub(crate) fn spatial_count(&self) -> usize {
            0
        }

        pub(crate) fn play(&mut self, _bytes: Vec<u8>, _volume: f32) -> Result<(), AudioError> {
            Ok(())
        }

        pub(crate) fn play_spatial(
            &mut self,
            _bytes: Vec<u8>,
            _volume: f32,
            _folded: [f32; 3],
            _left: [f32; 3],
            _right: [f32; 3],
        ) -> Result<(), AudioError> {
            Ok(())
        }

        pub(crate) fn play_music(&mut self, _bytes: &[u8], _volume: f32) -> Result<(), AudioError> {
            Ok(())
        }

        pub(crate) fn music_finished(&self) -> bool {
            false
        }

        pub(crate) fn refill_music(&mut self, _bytes: &[u8]) -> Result<(), AudioError> {
            Ok(())
        }

        pub(crate) fn set_music_volume(&self, _volume: f32) {}

        pub(crate) fn stop_music(&mut self) {}

        pub(crate) fn pause_music(&self) {}

        pub(crate) fn resume_music(&self) {}
    }
}

pub(crate) use imp::Device;
