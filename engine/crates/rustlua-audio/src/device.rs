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
    use crate::tone::{ToneControl, ToneFilter};
    use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source, SpatialSink};
    use std::io::Cursor;
    use std::time::Duration;

    /// 把解码后的 f32 源包上单声道降混与低/高频衰减。
    ///
    /// 只有 [`ToneControl::is_neutral`] 为假时才构造本类型；默认路径不改变旧声音。
    struct ToneSource<I> {
        inner: I,
        control: ToneControl,
        filter: ToneFilter,
        channels: u16,
        frame_pos: usize,
    }

    impl<I> ToneSource<I>
    where
        I: Source<Item = f32>,
    {
        fn new(inner: I, control: ToneControl) -> Self {
            let channels = inner.channels().max(1);
            let filter = ToneFilter::new(inner.sample_rate(), channels, control);
            Self {
                inner,
                control,
                filter,
                channels,
                frame_pos: 0,
            }
        }
    }

    impl<I> Iterator for ToneSource<I>
    where
        I: Source<Item = f32>,
    {
        type Item = f32;

        fn next(&mut self) -> Option<f32> {
            if self.control.is_neutral() {
                return self.inner.next();
            }
            if self.control.mono() {
                let mut sum = 0.0;
                for _ in 0..self.channels {
                    sum += self.inner.next()?;
                }
                return Some(self.filter.process(0, sum / f32::from(self.channels)));
            }
            let channel = self.frame_pos % usize::from(self.channels);
            let sample = self.inner.next()?;
            self.frame_pos += 1;
            Some(self.filter.process(channel, sample))
        }

        fn size_hint(&self) -> (usize, Option<usize>) {
            let (lo, hi) = self.inner.size_hint();
            if self.control.mono() {
                let n = usize::from(self.channels);
                (lo / n, hi.map(|v| v / n))
            } else {
                (lo, hi)
            }
        }
    }

    impl<I> Source for ToneSource<I>
    where
        I: Source<Item = f32>,
    {
        fn current_frame_len(&self) -> Option<usize> {
            self.inner.current_frame_len().map(|samples| {
                if self.control.mono() {
                    samples / usize::from(self.channels)
                } else {
                    samples
                }
            })
        }

        fn channels(&self) -> u16 {
            if self.control.mono() {
                1
            } else {
                self.channels
            }
        }

        fn sample_rate(&self) -> u32 {
            self.inner.sample_rate()
        }

        fn total_duration(&self) -> Option<Duration> {
            self.inner.total_duration()
        }

        fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
            self.inner.try_seek(pos)
        }
    }

    /// 距离空气吸收：逐声道一阶低通（P3-R61）。只在 cutoff < 19 kHz 时套在 ToneSource 外。
    struct AirAbsorb<I> {
        inner: I,
        state: [f32; 8],
        alpha: f32,
        channels: u16,
        frame_pos: usize,
    }

    impl<I> AirAbsorb<I>
    where
        I: Source<Item = f32>,
    {
        fn new(inner: I, cutoff_hz: f32) -> Self {
            let channels = inner.channels().clamp(1, 8);
            let sample_rate = inner.sample_rate().max(1) as f32;
            let cutoff = cutoff_hz.clamp(200.0, 20_000.0);
            let alpha = (1.0 - (-std::f32::consts::TAU * cutoff / sample_rate).exp())
                .clamp(0.0, 1.0);
            Self {
                inner,
                state: [0.0; 8],
                alpha,
                channels,
                frame_pos: 0,
            }
        }
    }

    impl<I> Iterator for AirAbsorb<I>
    where
        I: Source<Item = f32>,
    {
        type Item = f32;

        fn next(&mut self) -> Option<f32> {
            let sample = self.inner.next()?;
            let channel = (self.frame_pos % usize::from(self.channels)).min(7);
            self.frame_pos += 1;
            let y = self.state[channel] + self.alpha * (sample - self.state[channel]);
            self.state[channel] = y;
            Some(y.clamp(-1.0, 1.0))
        }
    }

    impl<I> Source for AirAbsorb<I>
    where
        I: Source<Item = f32>,
    {
        fn current_frame_len(&self) -> Option<usize> {
            self.inner.current_frame_len()
        }

        fn channels(&self) -> u16 {
            self.channels
        }

        fn sample_rate(&self) -> u32 {
            self.inner.sample_rate()
        }

        fn total_duration(&self) -> Option<Duration> {
            self.inner.total_duration()
        }

        fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
            self.inner.try_seek(pos)
        }
    }

    /// 双耳时间差（ITD-lite）：把较远那侧耳道延迟最多 0.65 ms（P3-R73）。
    /// 只在双声道源上生效；单声道/多声道原样透传。
    struct InterauralDelay<I> {
        inner: I,
        ring: Vec<[f32; 2]>,
        write: usize,
        delay_left: usize,
        delay_right: usize,
        channels: u16,
        frame_pos: usize,
        pending: [f32; 2],
        shadow: [f32; 2],
        shadow_alpha: f32,
        far_ear: usize,
        near: [f32; 2],
        near_alpha: f32,
        near_ear: usize,
    }

    impl<I> InterauralDelay<I>
    where
        I: Source<Item = f32>,
    {
        fn new(inner: I, pan: f32) -> Self {
            let channels = inner.channels();
            let sample_rate = inner.sample_rate().max(1) as f32;
            let max_delay = ((sample_rate * 0.00065).round() as usize).clamp(1, 64);
            let amount = pan.clamp(-1.0, 1.0).abs();
            let delay = (amount * max_delay as f32).round() as usize;
            // pan > 0 = 声源在右 ⇒ 左耳更远，左耳多延迟。
            let (delay_left, delay_right) = if pan >= 0.0 {
                (delay, 0)
            } else {
                (0, delay)
            };
            // 头部遮蔽：远耳 4 kHz 一阶低通（与 ITD 同一侧）。
            let shadow_alpha =
                (1.0 - (-std::f32::consts::TAU * 4000.0 / sample_rate).exp()).clamp(0.0, 1.0);
            let near_alpha = shadow_alpha;
            Self {
                inner,
                ring: vec![[0.0; 2]; max_delay + 1],
                write: 0,
                delay_left,
                delay_right,
                channels,
                frame_pos: 0,
                pending: [0.0; 2],
                shadow: [0.0; 2],
                shadow_alpha,
                far_ear: if pan >= 0.0 { 0 } else { 1 },
                near: [0.0; 2],
                near_alpha,
                near_ear: if pan >= 0.0 { 1 } else { 0 },
            }
        }
    }

    impl<I> Iterator for InterauralDelay<I>
    where
        I: Source<Item = f32>,
    {
        type Item = f32;

        fn next(&mut self) -> Option<f32> {
            if self.channels != 2 {
                return self.inner.next();
            }
            if self.frame_pos % 2 == 1 {
                self.frame_pos += 1;
                return Some(self.pending[1]);
            }
            let left = self.inner.next()?;
            let right = self.inner.next()?;
            let cap = self.ring.len();
            self.ring[self.write] = [left, right];
            self.pending = [
                self.ring[(self.write + cap - self.delay_left) % cap][0],
                self.ring[(self.write + cap - self.delay_right) % cap][1],
            ];
            let far = self.far_ear;
            self.shadow[far] += self.shadow_alpha * (self.pending[far] - self.shadow[far]);
            self.pending[far] = self.shadow[far];
            // P3-R95：近耳耳道共振（约 4 kHz 高频抬升）。
            let near = self.near_ear;
            let low = self.near[near] + self.near_alpha * (self.pending[near] - self.near[near]);
            self.near[near] = low;
            self.pending[near] += (self.pending[near] - low) * 0.22;
            self.frame_pos += 1;
            self.write = (self.write + 1) % cap;
            Some(self.pending[0])
        }
    }

    impl<I> Source for InterauralDelay<I>
    where
        I: Source<Item = f32>,
    {
        fn current_frame_len(&self) -> Option<usize> {
            self.inner.current_frame_len()
        }

        fn channels(&self) -> u16 {
            self.channels
        }

        fn sample_rate(&self) -> u32 {
            self.inner.sample_rate()
        }

        fn total_duration(&self) -> Option<Duration> {
            self.inner.total_duration()
        }

        fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
            self.inner.try_seek(pos)
        }
    }

    /// 桌面短混响：4 条反馈延迟（29.7/37.1/41.1/43.7 ms），末段继续输出尾巴（P3-R78）。
    struct Reverb<I> {
        inner: I,
        combs: [Vec<[f32; 2]>; 4],
        idx: [usize; 4],
        early: [Vec<[f32; 2]>; 4],
        early_idx: [usize; 4],
        damp: [[f32; 2]; 4],
        damp_alpha: f32,
        low: [[f32; 2]; 4],
        low_alpha: f32,
        channels: u16,
        phase: usize,
        pending: [f32; 2],
        tail_frames: usize,
        input_done: bool,
    }

    impl<I> Reverb<I>
    where
        I: Source<Item = f32>,
    {
        fn new(inner: I) -> Self {
            let channels = inner.channels();
            let sample_rate = inner.sample_rate().max(1) as f32;
            let delays = [0.0297, 0.0371, 0.0411, 0.0437];
            let combs = delays.map(|seconds| {
                let len = ((sample_rate * seconds).round() as usize).clamp(8, 8192);
                vec![[0.0_f32; 2]; len]
            });
            // P3-R110：4 条早期反射（7/11/13/17 ms），给混响一点房间方向感。
            let early = [0.007, 0.011, 0.013, 0.017].map(|seconds| {
                let len = ((sample_rate * seconds).round() as usize).clamp(4, 4096);
                vec![[0.0_f32; 2]; len]
            });
            // P3-R109：反馈环里的一阶低通，尾音随反射次数变暗（3.5 kHz）。
            let damp_alpha =
                (1.0 - (-std::f32::consts::TAU * 3500.0 / sample_rate).exp()).clamp(0.0, 1.0);
            let low_alpha =
                (1.0 - (-std::f32::consts::TAU * 2000.0 / sample_rate).exp()).clamp(0.0, 1.0);
            Self {
                inner,
                combs,
                idx: [0; 4],
                early,
                early_idx: [0; 4],
                damp: [[0.0; 2]; 4],
                damp_alpha,
                low: [[0.0; 2]; 4],
                low_alpha,
                channels,
                phase: 0,
                pending: [0.0; 2],
                tail_frames: 0,
                input_done: false,
            }
        }

        fn max_delay(&self) -> usize {
            self.combs.iter().map(Vec::len).max().unwrap_or(8)
        }

        fn process(&mut self, input: [f32; 2]) -> [f32; 2] {
            let mut sum = [0.0_f32; 2];
            let mut early_sum = [0.0_f32; 2];
            for k in 0..4 {
                let e_idx = self.early_idx[k];
                let early = self.early[k][e_idx];
                self.early[k][e_idx] = input;
                self.early_idx[k] = (e_idx + 1) % self.early[k].len();
                early_sum[0] += early[0] * 0.10;
                early_sum[1] += early[1] * 0.10;
            }
            let mut delayed = [[0.0_f32; 2]; 4];
            let mut write_idx = [0usize; 4];
            for k in 0..4 {
                let index = self.idx[k];
                delayed[k] = self.combs[k][index];
                write_idx[k] = index;
                self.idx[k] = (index + 1) % self.combs[k].len();
                self.damp[k][0] += self.damp_alpha * (delayed[k][0] - self.damp[k][0]);
                self.damp[k][1] += self.damp_alpha * (delayed[k][1] - self.damp[k][1]);
                sum[0] += self.damp[k][0];
                sum[1] += self.damp[k][1];
            }
            // P3-R113：4×4 Householder 反馈矩阵，让四条延迟互相混合（FDN）而不是各自独立。
            let mut feedback = [[0.0_f32; 2]; 4];
            for k in 0..4 {
                self.low[k][0] += self.low_alpha * (self.damp[k][0] - self.low[k][0]);
                self.low[k][1] += self.low_alpha * (self.damp[k][1] - self.low[k][1]);
                // P3-R118：低频反馈保留更多能量，高频衰减更快（双频带）。
                feedback[k] = [
                    self.low[k][0] * 0.82 + (self.damp[k][0] - self.low[k][0]) * 0.55,
                    self.low[k][1] * 0.82 + (self.damp[k][1] - self.low[k][1]) * 0.55,
                ];
            }
            let mean = [
                (feedback[0][0] + feedback[1][0] + feedback[2][0] + feedback[3][0]) * 0.25,
                (feedback[0][1] + feedback[1][1] + feedback[2][1] + feedback[3][1]) * 0.25,
            ];
            for k in 0..4 {
                let fed = [
                    feedback[k][0] - mean[0] * 0.5,
                    feedback[k][1] - mean[1] * 0.5,
                ];
                self.combs[k][write_idx[k]] = [
                    input[0] + fed[0] * 0.72,
                    input[1] + fed[1] * 0.72,
                ];
            }
            [
                input[0] * 0.88 + sum[0] * 0.12 + early_sum[0],
                input[1] * 0.88 + sum[1] * 0.12 + early_sum[1],
            ]
        }
    }

    impl<I> Iterator for Reverb<I>
    where
        I: Source<Item = f32>,
    {
        type Item = f32;

        fn next(&mut self) -> Option<f32> {
            if self.channels != 2 {
                return self.inner.next();
            }
            if self.phase == 1 {
                self.phase = 0;
                return Some(self.pending[1]);
            }
            let input = if self.input_done {
                if self.tail_frames == 0 {
                    return None;
                }
                self.tail_frames -= 1;
                [0.0; 2]
            } else {
                match (self.inner.next(), self.inner.next()) {
                    (Some(left), Some(right)) => [left, right],
                    _ => {
                        self.input_done = true;
                        self.tail_frames = self.max_delay() * 3;
                        if self.tail_frames == 0 {
                            return None;
                        }
                        self.tail_frames -= 1;
                        [0.0; 2]
                    }
                }
            };
            self.pending = self.process(input);
            self.phase = 1;
            Some(self.pending[0])
        }
    }

    impl<I> Source for Reverb<I>
    where
        I: Source<Item = f32>,
    {
        fn current_frame_len(&self) -> Option<usize> {
            self.inner.current_frame_len()
        }

        fn channels(&self) -> u16 {
            self.channels
        }

        fn sample_rate(&self) -> u32 {
            self.inner.sample_rate()
        }

        fn total_duration(&self) -> Option<Duration> {
            self.inner.total_duration()
        }

        fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
            self.inner.try_seek(pos)
        }
    }

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
        tone: ToneControl,
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

        /// 设置播放期音色；之后新建的每个 sink（含 BGM 续播）都使用它。
        pub(crate) fn set_tone(&mut self, tone: ToneControl) {
            self.tone = tone;
        }

        /// 播放一次性音效。设备没开时是 no-op（调用方已在外面判过 `is_open`，这里只是兜底）。
        pub(crate) fn play(&mut self, bytes: Vec<u8>, volume: f32) -> Result<(), AudioError> {
            let Some(handle) = self.handle.as_ref() else {
                return Ok(());
            };
            let sink =
                Sink::try_new(handle).map_err(|error| AudioError::Device(error.to_string()))?;
            let source = Decoder::new(Cursor::new(bytes))
                .map_err(|error| AudioError::Decode(error.to_string()))?
                .convert_samples::<f32>();
            sink.set_volume(volume);
            sink.append(ToneSource::new(source, self.tone));
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
            pan: f32,
            cutoff_hz: f32,
        ) -> Result<(), AudioError> {
            // 单声道无障碍模式必须真的只有一个声道；`SpatialSink` 会把它重新铺成
            // 立体声，所以这里退回普通 Sink（音量已由上层按距离算好）。
            if self.tone.mono() {
                return self.play(bytes, volume);
            }
            let Some(handle) = self.handle.as_ref() else {
                return Ok(());
            };
            let sink = SpatialSink::try_new(handle, folded, left, right)
                .map_err(|error| AudioError::Device(error.to_string()))?;
            let source = Decoder::new(Cursor::new(bytes))
                .map_err(|error| AudioError::Decode(error.to_string()))?
                .convert_samples::<f32>();
            sink.set_volume(volume);
            let tone = ToneSource::new(source, self.tone);
            let reverb = Reverb::new(tone);
            if cutoff_hz < 19_000.0 {
                let air = AirAbsorb::new(reverb, cutoff_hz);
                if pan.abs() > 0.02 {
                    sink.append(InterauralDelay::new(air, pan));
                } else {
                    sink.append(air);
                }
            } else if pan.abs() > 0.02 {
                sink.append(InterauralDelay::new(reverb, pan));
            } else {
                sink.append(reverb);
            }
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
                .map_err(|error| AudioError::Decode(error.to_string()))?
                .convert_samples::<f32>();
            sink.set_volume(volume);
            sink.append(ToneSource::new(source, self.tone));
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
                .map_err(|error| AudioError::Decode(error.to_string()))?
                .convert_samples::<f32>();
            sink.append(ToneSource::new(source, self.tone));
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

    #[cfg(test)]
    mod tests {
        use super::*;
        use rodio::buffer::SamplesBuffer;

        #[test]
        fn interaural_delay_delays_the_far_ear_only() {
            let mut samples = vec![0.0_f32; 400];
            samples[0] = 1.0;
            samples[1] = 1.0;
            let input = SamplesBuffer::new(2, 48_000, samples);
            let mut source = InterauralDelay::new(input, 1.0);
            let mut left_first = None;
            let mut right_first = None;
            for frame in 0..100 {
                let left = source.next().expect("left sample");
                let right = source.next().expect("right sample");
                if left.abs() > 0.2 && left_first.is_none() {
                    left_first = Some(frame);
                }
                if right.abs() > 0.5 && right_first.is_none() {
                    right_first = Some(frame);
                }
            }
            let (left, right) = (left_first.expect("left impulse"), right_first.expect("right impulse"));
            assert!(
                left > right,
                "pan=1 must delay the left ear: left={left} right={right}"
            );
            assert!(
                left > right,
                "pan=1 must delay the left ear: left={left} right={right}"
            );
        }

        #[test]
        fn far_ear_is_low_passed_by_head_shadow() {
            let mut samples = Vec::new();
            for i in 0..4000 {
                let value = if i % 2 == 0 { 1.0 } else { -1.0 };
                samples.push(value);
                samples.push(value);
            }
            let input = SamplesBuffer::new(2, 48_000, samples);
            let mut source = InterauralDelay::new(input, 1.0);
            let mut left_energy = 0.0_f32;
            let mut right_energy = 0.0_f32;
            for _ in 0..500 {
                let left = source.next().expect("left sample");
                let right = source.next().expect("right sample");
                left_energy += left * left;
                right_energy += right * right;
            }
            assert!(
                left_energy < right_energy * 0.5,
                "far ear must be shadowed: left={left_energy} right={right_energy}"
            );
        }

        #[test]
        fn reverb_keeps_a_tail_after_the_source_ends() {
            let mut samples = vec![0.0_f32; 64];
            samples[0] = 1.0;
            samples[1] = 1.0;
            let input = SamplesBuffer::new(2, 48_000, samples);
            let mut source = Reverb::new(input);
            let mut tail_energy = 0.0_f32;
            let mut frames = 0;
            while let Some(left) = source.next() {
                let right = source.next().unwrap_or(0.0);
                if frames > 64 {
                    tail_energy += left * left + right * right;
                }
                frames += 1;
                if frames > 20_000 {
                    break;
                }
            }
            assert!(
                tail_energy > 1e-6,
                "reverb must keep ringing after the source: {tail_energy}"
            );
        }

        #[test]
        fn mono_averages_stereo_and_reports_one_channel() {
            let input = SamplesBuffer::new(2, 48_000, vec![0.5_f32, -0.5, 0.25, 0.25]);
            let mut source = ToneSource::new(input, ToneControl::new(true, 0.0, 0.0).unwrap());
            assert_eq!(source.channels(), 1);
            assert!(source.next().unwrap().abs() < 1e-6);
            assert!((source.next().unwrap() - 0.25).abs() < 1e-6);
        }

        #[test]
        fn neutral_tone_keeps_stereo_samples_exact() {
            let input = SamplesBuffer::new(2, 48_000, vec![0.5_f32, -0.5, 0.25, -0.25]);
            let mut source = ToneSource::new(input, ToneControl::OFF);
            assert_eq!(source.channels(), 2);
            assert_eq!(source.next().unwrap(), 0.5);
            assert_eq!(source.next().unwrap(), -0.5);
            assert_eq!(source.next().unwrap(), 0.25);
            assert_eq!(source.next().unwrap(), -0.25);
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

        pub(crate) fn set_tone(&mut self, _tone: crate::tone::ToneControl) {}

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
            _pan: f32,
            _cutoff_hz: f32,
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
