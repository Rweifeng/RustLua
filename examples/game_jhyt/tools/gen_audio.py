# -*- coding: utf-8 -*-
"""《江湖有田》音频资源生成（自研合成，无第三方素材）。

License / 来源声明
------------------
本文件生成的全部 .wav 均为 **本仓库自行合成的程序化音频（self-authored
synthesis）**：只使用 Python 标准库（wave / math / array / os / sys / hashlib）
现场计算波形，**不包含任何第三方素材、采样、音源库或网络下载**，许可证随仓库，
详见 `docs/THIRD_PARTY.md`（engine 与游戏源码为 MIT OR Apache-2.0）。

生成命令（Windows / 仓库根目录任意位置均可，脚本用自身路径解析项目根）::

    py -3 examples/game_jhyt/tools/gen_audio.py

输出
----
`examples/game_jhyt/assets/audio/*.wav`，共 15 个文件：
4 个 BGM（title / manor / village / wilds）+ 11 个音效。

格式契约
--------
16-bit PCM、**单声道**、**22050 Hz**，与引擎 `engine/crates/engine/src/core/audio.rs`
的 `<project>/assets/audio/<name>.wav` 读取约定一致（引擎优先 .wav，其次 .ogg）。

确定性
------
全脚本 **不使用 random**。若需要噪声，使用下方固定种子的手写 LCG
（`LCG`，a=1664525 / c=1013904223 / m=2**32，Numerical Recipes 参数），
所有振荡器走整数相位查表（相位步长是整数），因此同一脚本重复运行输出
**字节完全一致**，可用 `hashlib.sha256` 复现校验（本脚本自检时也会报告）。

无缝循环设计（BGM）
-------------------
每个 BGM 时长 T 固定；所有音符频率被量化到 k/T（k 为整数），即整段恰好容纳
整数个周期，故 `s[0] == s[L]`（含尾部余响回卷后仍成立）；包络起落都从 0 出发、
回到 0，循环点无爆音。
"""

import array
import hashlib
import math
import os
import sys
import wave

# ---------------------------------------------------------------------------
# 全局契约
# ---------------------------------------------------------------------------
SR = 22050                     # 采样率（硬性要求）
CHANNELS = 1                   # 单声道
SAMPLE_WIDTH = 2               # 16-bit PCM
TWO_PI = 2.0 * math.pi
TABLE_BITS = 13                # 波表长度 8192（2 的幂，整数相位回绕）
TABLE_SIZE = 1 << TABLE_BITS
TABLE_MASK = TABLE_SIZE - 1
GRID_HZ = 1.0 / 128.0          # BGM 频率量化栅格（1/128 Hz，听感无偏差）
SFX_MAX_SECONDS = 0.6          # 音效时长上限
BGM_MAX_BYTES = 2 * 1024 * 1024  # 单个 BGM 体积上限 2 MB
DUR_TOLERANCE_S = 0.010        # 时长误差容忍 10 ms
LOOP_EDGE_TOLERANCE = 0.02     # 循环点首尾 |sample| 上限（归一化后）
ACTIVE_RATIO_MIN = 0.01        # 至少 1% 样本 |v| > 100
ACTIVE_ABS = 100
FULL_SCALE = 32767

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT_DIR = os.path.join(PROJECT_ROOT, "assets", "audio")


# ---------------------------------------------------------------------------
# 确定性噪声：手写线性同余发生器（固定种子，绝不使用 random）
# ---------------------------------------------------------------------------
class LCG(object):
    """手写 32-bit LCG：x = (1664525 * x + 1013904223) mod 2**32。

    固定种子 => 噪声序列完全可复现（跨进程、跨运行一致）。
    """

    A = 1664525
    C = 1013904223
    M = 1 << 32

    def __init__(self, seed):
        self.state = seed & 0xFFFFFFFF

    def next_u32(self):
        self.state = (self.A * self.state + self.C) % self.M
        return self.state

    def signed(self):
        """[-1, 1) 的白噪声。"""
        return (self.next_u32() >> 8) / 8388608.0 - 1.0

    def unit(self):
        """[0, 1) 均匀分布。"""
        return (self.next_u32() >> 8) / 8388608.0


def one_pole(samples, cutoff_hz, sr=SR):
    """一阶低通滤波（in-place，返回同一 array）。"""
    if cutoff_hz >= sr * 0.5:
        return samples
    alpha = 1.0 - math.exp(-TWO_PI * cutoff_hz / sr)
    state = 0.0
    for i in range(len(samples)):
        state += alpha * (samples[i] - state)
        samples[i] = state
    return samples


def high_pass(samples, cutoff_hz, sr=SR):
    """一阶高通（用于去直流/去隆隆声）。"""
    alpha = 1.0 - math.exp(-TWO_PI * cutoff_hz / sr)
    state = 0.0
    for i in range(len(samples)):
        state += alpha * (samples[i] - state)
        samples[i] = samples[i] - state
    return samples


def soft_limit(value):
    """三次软限幅：>0.7 后逐渐压缩，绝不产生硬削波方波感。"""
    limit = 0.7
    if value > limit:
        return limit + (1.0 - limit) * math.tanh((value - limit) / (1.0 - limit))
    if value < -limit:
        return -limit - (1.0 - limit) * math.tanh((-value - limit) / (1.0 - limit))
    return value


def scale_to_peak(mix, target):
    """把缓冲缩放到目标峰值；全零缓冲原样返回。"""
    peak = 0.0
    for value in mix:
        mag = value if value >= 0.0 else -value
        if mag > peak:
            peak = mag
    if peak <= 1e-12:
        return mix, 0.0
    gain = target / peak
    for i in range(len(mix)):
        mix[i] = soft_limit(mix[i] * gain)
    return mix, peak


# ---------------------------------------------------------------------------
# 波表：整数相位 -> 天然得到"整数周期 = 无缝循环"
# ---------------------------------------------------------------------------
def _build_tables():
    tables = {}
    sin_tab = [math.sin(TWO_PI * i / TABLE_SIZE) for i in range(TABLE_SIZE)]
    tables["sine"] = sin_tab
    # 三角：asinh(sin)/asinh(1) 归一化，谐波比正弦更"木质"
    norm = math.asinh(1.0)
    tables["triangle"] = [math.asinh(math.sin(TWO_PI * i / TABLE_SIZE)) / norm
                          for i in range(TABLE_SIZE)]
    # 方波：只取奇次谐波（加法合成，避免 aliasing 尖锐感）
    sq = [0.0] * TABLE_SIZE
    for h in range(1, 20, 2):
        amp = 1.0 / h
        for i in range(TABLE_SIZE):
            sq[i] += amp * math.sin(TWO_PI * h * i / TABLE_SIZE)
    peak = max(abs(v) for v in sq)
    tables["square"] = [v / peak for v in sq]
    # 锯齿：全部谐波（限制 20 次，减少高频锯齿味）
    saw = [0.0] * TABLE_SIZE
    for h in range(1, 21):
        amp = 1.0 / h
        for i in range(TABLE_SIZE):
            saw[i] += amp * math.sin(TWO_PI * h * i / TABLE_SIZE)
    peak = max(abs(v) for v in saw)
    tables["saw"] = [v / peak for v in saw]
    # 钟/铃：基音 + 少量 2 次谐波，空灵
    tables["bell"] = [sin_tab[i] + 0.35 * sin_tab[(2 * i) & TABLE_MASK]
                      for i in range(TABLE_SIZE)]
    # 木/簧：基音 + 3 次谐波，温和的木质感
    tables["reed"] = [sin_tab[i] + 0.22 * sin_tab[(3 * i) & TABLE_MASK]
                      for i in range(TABLE_SIZE)]
    return tables


WAVETABLES = _build_tables()


# ---------------------------------------------------------------------------
# 合成核心
# ---------------------------------------------------------------------------
def envelope(position, duration, attack, decay, sustain, release):
    """ADSR 包络：attack 线性升 -> decay 降到 sustain -> 保持 -> release 线性归零。

    `position`/`duration` 单位秒；起点与终点都恰为 0，保证循环点无样本跳变。
    """
    if position < 0.0 or position > duration:
        return 0.0
    if duration <= 1e-9:
        return 0.0
    release = min(release, duration * 0.49)
    attack = min(attack, max(duration - release, 1e-9))
    decay = min(decay, max(duration - attack - release, 0.0))
    head = attack + decay
    if position < attack:
        value = position / attack
    elif position < head:
        span = max(decay, 1e-9)
        value = 1.0 + (sustain - 1.0) * ((position - attack) / span)
    elif position <= duration - release:
        value = sustain
    else:
        span = max(release, 1e-9)
        value = sustain * (1.0 - (position - (duration - release)) / span)
    if value < 0.0:
        return 0.0
    return value


class Voice(object):
    """一个音符事件（支持 1~3 层泛音堆叠，如八度 / 五度）。"""

    __slots__ = ("t0", "freq", "duration", "wave", "gain",
                 "attack", "decay", "sustain", "release", "layers")

    def __init__(self, t0, freq, duration, wave="sine", gain=0.3,
                 attack=0.01, decay=0.15, sustain=0.7, release=0.2,
                 layers=None):
        self.t0 = float(t0)
        self.freq = float(freq)
        self.duration = float(duration)
        self.wave = wave
        self.gain = float(gain)
        self.attack = float(attack)
        self.decay = float(decay)
        self.sustain = float(sustain)
        self.release = float(release)
        self.layers = layers if layers else ((1.0, 1.0),)

    def stop_time(self):
        return self.t0 + self.duration


class Play(object):
    """一次合成的总账：采样率、循环长度、频率量化栅格、事件表。"""

    def __init__(self, loop_samples=0, loop_seconds=0.0, quantize=False):
        self.loop_samples = loop_samples
        self.loop_seconds = loop_seconds
        self.quantize = quantize
        self.voices = []
        self.max_tail = 0.0

    # -- 事件登记 ----------------------------------------------------------
    def note(self, t0, freq, duration, **kwargs):
        if self.quantize:
            freq = self.snap(freq)
        voice = Voice(t0, freq, duration, **kwargs)
        self._place(voice)
        return voice

    def snap(self, freq):
        """把频率吸附到 k/loop_seconds，令整段容纳整数个周期。"""
        grid = GRID_HZ
        k = int(round(freq / grid))
        if k < 1:
            k = 1
        return k * grid

    def _place(self, voice):
        """事件起点落在 [0, loop) 内；跨过循环点的部分靠渲染回卷自然接回。"""
        if self.loop_seconds > 0.0:
            while voice.t0 >= self.loop_seconds:
                voice.t0 -= self.loop_seconds
        self.voices.append(voice)
        if voice.stop_time() > self.max_tail:
            self.max_tail = voice.stop_time()

    # -- 渲染 --------------------------------------------------------------
    def render(self, total_samples):
        """把全部事件叠加进长度为 total_samples 的缓冲。"""
        mix = array.array("d", bytes(8 * total_samples))
        for voice in self.voices:
            self._add_voice(mix, voice)
        return mix

    def _add_voice(self, mix, voice):
        start = int(round(voice.t0 * SR))
        if start >= len(mix):
            return
        duration = voice.duration
        # 事件长度 = round(duration*SR) 个样本；配合下方"末尾归零"的包络，
        # 事件在其自身起止点振幅都为 0，叠加后不会产生样本跳变。
        count = int(round(duration * SR))
        if count <= 0:
            return
        length = min(count, len(mix) - start)
        inv_sr = 1.0 / SR
        layers = []
        for ratio, amp in voice.layers:
            freq = voice.freq * ratio
            step = int(round(freq * TABLE_SIZE / SR))
            if step <= 0:
                continue
            table = WAVETABLES[voice.wave]
            layers.append((0, step, table, amp))
        if not layers:
            return
        gain = voice.gain
        attack, decay = voice.attack, voice.decay
        sustain, release = voice.sustain, voice.release
        for i in range(length):
            amp = envelope(i * inv_sr, duration, attack, decay, sustain, release)
            if amp <= 0.0:
                continue
            # 各层同速推进：相位为整数，波表回绕天然精确
            value = 0.0
            for idx in range(len(layers)):
                phase, step, table, layer_amp = layers[idx]
                value += layer_amp * table[phase & TABLE_MASK]
                layers[idx] = ((phase + step) & TABLE_MASK, step, table, layer_amp)
            mix[start + i] += gain * amp * value


def assemble(voice_list, loop_samples, loop_seconds, quantize, fade_seconds,
             cutoff_hz, target_peak):
    """渲染 + 循环回卷 + 滤波 + 循环点包络 + 归一化。"""
    play = Play(loop_samples=loop_samples, loop_seconds=loop_seconds,
                quantize=quantize)
    for spec in voice_list:
        play.note(**spec)
    tail_samples = int(math.ceil(play.max_tail * SR)) - loop_samples
    if tail_samples < 0:
        tail_samples = 0
    mix = play.render(loop_samples + tail_samples)
    # 尾部余响回卷到开头：使 s[L] 与 s[0] 连续（整数周期 + 端点归零 => 无缝）
    for i in range(loop_samples, len(mix)):
        mix[i - loop_samples] += mix[i]
    del mix[loop_samples:]
    if cutoff_hz:
        one_pole(mix, cutoff_hz)
    if fade_seconds > 0.0:
        fade = int(round(fade_seconds * SR))
        if fade > 0:
            # rise 与 fall 之和恒为 1（余弦窗），loop 周期整体增益守恒
            for i in range(fade):
                t = i / fade
                gain = 0.5 - 0.5 * math.cos(math.pi * t)
                mix[i] *= gain
                mix[len(mix) - 1 - i] *= gain
    mix, _peak = scale_to_peak(mix, target_peak)
    return mix


# ---------------------------------------------------------------------------
# 后台音乐：4 首，风格明确区分
# ---------------------------------------------------------------------------
def _deg(base, degree, mode="gong"):
    """五声音阶取音：degree 为级数（0=宫），mode 为调式名称。"""
    ratios = {
        "gong": (1.0, 9.0 / 8.0, 5.0 / 4.0, 3.0 / 2.0, 5.0 / 3.0),
        "shang": (1.0, 9.0 / 8.0, 4.0 / 3.0, 3.0 / 2.0, 5.0 / 3.0),
        "jue": (1.0, 6.0 / 5.0, 4.0 / 3.0, 3.0 / 2.0, 9.0 / 5.0),
        "zhi": (1.0, 9.0 / 8.0, 5.0 / 4.0, 3.0 / 2.0, 5.0 / 3.0),
        "yu": (1.0, 6.0 / 5.0, 4.0 / 3.0, 3.0 / 2.0, 9.0 / 5.0),
    }
    table = ratios[mode]
    octave, idx = divmod(degree, len(table))
    return base * table[idx] * (2.0 ** octave)


def _drone_events(events, length_s, base, gain, wave="sine", breathe=4.0):
    """低频长音（每 breathe 秒换一次泵浦点），整段铺满。"""
    cursor = 0.0
    while cursor < length_s - 1e-6:
        span = min(breathe, length_s - cursor)
        events.append(dict(t0=cursor, freq=base, duration=span, wave=wave,
                           gain=gain, attack=1.1, decay=0.6, sustain=0.72,
                           release=1.0))
        events.append(dict(t0=cursor, freq=base * 1.5, duration=span,
                           wave="sine", gain=gain * 0.34, attack=1.5,
                           decay=0.8, sustain=0.6, release=1.2))
        cursor += span


def build_bgm_title():
    """曲一 标题：慢速空灵，宫调式，低音长音 + 稀疏琶音，约 16s。"""
    length = 16.0          # 慢速：1 拍 ≈ 1.0s（60 BPM），16s ≈ 6.4 小节
    bar = 2.5
    events = []
    _drone_events(events, length, 130.813, 0.30, wave="sine", breathe=8.0)
    # 呼吸式高频衬底：五度泛音，极弱
    events.append(dict(t0=0.0, freq=392.438, duration=16.0, wave="sine",
                       gain=0.055, attack=3.0, decay=1.5, sustain=0.55,
                       release=3.0))
    melody = [                      # (拍长, 级数, 时长)
        (1.0, 9, 1.5), (0.5, None, 0.0), (1.0, 7, 1.5), (0.5, None, 0.0),
        (1.0, 5, 1.8), (0.5, None, 0.0), (0.5, 7, 1.2), (0.5, None, 0.0),
        (1.0, 4, 1.8), (1.0, None, 0.0),
        (1.0, 5, 1.5), (0.5, None, 0.0), (1.0, 9, 1.6), (0.5, None, 0.0),
        (1.0, 12, 2.0), (0.5, None, 0.0), (0.5, 9, 1.2), (0.5, None, 0.0),
        (1.0, 7, 1.8), (1.0, None, 0.0),
        (0.5, 12, 1.4), (0.5, 11, 1.4), (0.5, 9, 1.6), (0.5, 7, 1.4),
        (0.5, 5, 1.8), (0.5, 4, 1.4), (0.5, 2, 1.8), (0.5, None, 0.0),
        (1.0, 0, 2.0), (1.0, None, 0.0),
    ]
    cursor = 0.0
    for beats, degree, duration in melody:
        if degree is not None:
            events.append(dict(t0=cursor, degree=degree, duration=duration,
                               wave="triangle", gain=0.22, attack=0.42,
                               decay=0.5, sustain=0.75, release=0.85,
                               base=261.626, mode="gong"))
        cursor += beats
        if cursor >= length:
            break
    # 稀疏钟声琶音：每 2 小节落一颗
    for index, beat in enumerate((0.0, 3.75, 7.5, 11.25, 13.75)):
        degree = (4, 7, 9, 11, 7)[index]
        events.append(dict(t0=beat, degree=degree, duration=2.4, wave="bell",
                           gain=0.13, attack=0.02, decay=1.1, sustain=0.30,
                           release=1.2, base=261.626, mode="gong"))
    return _bgm_spec(length, events, cutoff_hz=4200.0, fade=0.9, peak=0.88)


def build_bgm_manor():
    """曲二 山庄：田园温和，徵调式，木质感（三角+方波），约 14s。"""
    length = 14.0          # 中速：1 拍 = 1.0s（60 BPM），14 小节，温和不赶
    events = []
    _drone_events(events, length, 146.832, 0.24, wave="triangle", breathe=5.0)
    # 低音行进（方波 + 慢包络 = 木质感）
    bass = [0, 3, 2, 4, 0, 3, 4, 2, 0, 3, 2, 4, 0, 3]
    for i, degree in enumerate(bass):
        events.append(dict(t0=i * 1.0, degree=degree - 5, duration=1.15,
                           wave="square", gain=0.155, attack=0.06,
                           decay=0.35, sustain=0.60, release=0.45,
                           base=146.832, mode="zhi"))
    # 主旋律：2 小节主题 + 变奏，落在整拍网格
    melody_a = [(0.0, 7, 1.0), (1.0, 9, 0.5), (1.5, 7, 1.0), (2.5, 5, 1.0),
                (3.5, 4, 0.5)]
    melody_b = [(0.0, 5, 1.0), (1.0, 7, 0.5), (1.5, 9, 1.5),
                (3.0, 7, 1.0)]
    melody_c = [(0.0, 9, 1.0), (1.0, 11, 0.5), (1.5, 12, 1.25),
                (2.75, 9, 1.0), (3.75, 7, 0.25)]
    melody_d = [(0.0, 7, 1.5), (1.5, 5, 0.75), (2.25, 4, 0.75),
                (3.0, 2, 1.0)]
    for bar_index in range(len(bass)):
        phrase = (melody_a, melody_b, melody_c, melody_d,
                  melody_a, melody_b)[bar_index % 6]
        for offset, degree, duration in phrase:
            events.append(dict(t0=bar_index * 1.0 + offset, degree=degree,
                               duration=duration, wave="triangle",
                               gain=0.235, attack=0.05, decay=0.30,
                               sustain=0.66, release=0.40,
                               base=293.665, mode="zhi"))
    # 木铃点缀：每小节一个重拍
    for bar_index in range(len(bass)):
        events.append(dict(t0=bar_index * 1.0, degree=12, duration=0.6,
                           wave="bell", gain=0.075, attack=0.005,
                           decay=0.22, sustain=0.10, release=0.35,
                           base=293.665, mode="zhi"))
    return _bgm_spec(length, events, cutoff_hz=3200.0, fade=0.5, peak=0.89)


def build_bgm_village():
    """曲三 市集：活泼明亮，羽调式，简单节奏型，约 14s。"""
    length = 14.0          # 120 BPM，4/4 => 0.5s/拍、2.0s/小节，共 7 小节
    events = []
    _drone_events(events, length, 146.832, 0.20, wave="square", breathe=3.5)
    # 节奏型底鼓/木鱼：整数周期事件，循环点干净
    for bar_index in range(7):
        bar = bar_index * 2.0
        events.append(dict(t0=bar, freq=100.0, duration=0.30, wave="sine",
                           gain=0.30, attack=0.004, decay=0.10,
                           sustain=0.20, release=0.16))
        events.append(dict(t0=bar + 1.0, freq=96.0, duration=0.24, wave="sine",
                           gain=0.20, attack=0.004, decay=0.08,
                           sustain=0.18, release=0.13))
        for offset in (0.5, 1.5):
            events.append(dict(t0=bar + offset, freq=760.0, duration=0.09,
                               wave="triangle", gain=0.085, attack=0.003,
                               decay=0.04, sustain=0.10, release=0.05))
    # 活泼旋律：八分音符为主，明亮三角波
    melody = [
        (0, 0, 7, 0.5), (0, 0.5, 9, 0.5), (0, 1.0, 12, 0.5),
        (0, 1.5, 9, 0.5), (0, 2.0, 7, 0.5), (0, 2.5, 9, 0.5),
        (0, 3.0, 11, 0.5), (0, 3.5, 9, 0.5),
        (1, 0, 9, 0.5), (1, 0.5, 11, 0.5), (1, 1.0, 12, 0.75),
        (1, 2.0, 9, 0.5), (1, 2.5, 7, 0.5), (1, 3.0, 5, 0.5),
        (1, 3.5, 7, 0.5),
        (2, 0, 12, 0.5), (2, 0.5, 11, 0.5), (2, 1.0, 9, 0.5),
        (2, 1.5, 7, 0.5), (2, 2.0, 9, 1.0), (2, 3.0, 12, 1.0),
        (3, 0, 11, 0.5), (3, 0.5, 9, 0.5), (3, 1.0, 7, 0.5),
        (3, 1.5, 5, 0.5), (3, 2.0, 4, 1.5), (3, 3.5, 2, 0.5),
        (4, 0, 7, 0.5), (4, 0.5, 9, 0.5), (4, 1.0, 12, 0.5),
        (4, 1.5, 14, 0.5), (4, 2.0, 12, 1.0), (4, 3.0, 9, 1.0),
        (5, 0, 9, 0.5), (5, 0.5, 11, 0.5), (5, 1.0, 12, 0.5),
        (5, 1.5, 9, 0.5), (5, 2.0, 7, 1.5),
        (6, 0, 5, 0.5), (6, 0.5, 7, 0.5), (6, 1.0, 9, 0.5),
        (6, 1.5, 7, 0.5), (6, 2.0, 5, 1.0), (6, 3.0, 0, 1.0),
    ]
    for bar_index, offset, degree, duration in melody:
        events.append(dict(t0=bar_index * 2.0 + offset, degree=degree,
                           duration=duration, wave="triangle", gain=0.215,
                           attack=0.012, decay=0.12, sustain=0.62,
                           release=0.18, base=293.665, mode="yu"))
    # 低音拨弦：每拍一根
    bass = [0, 3, 2, 4, 2, 0, 3, 1, 0, 3, 2, 4, 0, 3]
    for i, degree in enumerate(bass):
        events.append(dict(t0=i * 1.0, degree=degree - 5, duration=0.42,
                           wave="square", gain=0.15, attack=0.005,
                           decay=0.14, sustain=0.42, release=0.24,
                           base=146.832, mode="yu"))
    return _bgm_spec(length, events, cutoff_hz=5200.0, fade=0.4, peak=0.90)


def build_bgm_wilds():
    """曲四 山野：苍凉低沉，宫调式，五度/八度堆叠，约 16s。"""
    length = 16.0          # 105 BPM，4/4 => 2.285714s/小节，共 7 小节
    bar = 16.0 / 7.0
    events = []
    _drone_events(events, length, 98.999, 0.34, wave="triangle", breathe=8.0)
    # 低沉长旋律 + 五度/八度堆叠（layers）
    melody = [
        (0, 0, 0, 3.0), (0, 1.0, 4, 3.0), (0, 3.0, 2, 3.0), (0, 5.0, 0, 3.5),
        (0, 6.5, 4, 3.0),
        (1, 0, 2, 3.0), (1, 1.5, 4, 3.5), (1, 4.0, 5, 3.0), (1, 6.0, 2, 3.5),
        (2, 0, 7, 3.0), (2, 1.5, 5, 3.0), (2, 3.5, 4, 3.0), (2, 5.5, 2, 3.5),
        (3, 0, 0, 3.0), (3, 1.5, 2, 3.0), (3, 3.5, 4, 4.0), (3, 6.0, 0, 3.5),
    ]
    for bar_index, offset, degree, duration in melody:
        events.append(dict(t0=bar_index * bar + offset, degree=degree,
                           duration=duration, wave="saw", gain=0.145,
                           attack=0.55, decay=0.7, sustain=0.62,
                           release=1.1, base=146.832, mode="gong",
                           layers=((0.5, 0.45), (1.0, 1.0), (1.5, 0.30),
                                   (2.0, 0.18))))
    # 低沉节拍：每小节一次闷响
    for bar_index in range(7):
        events.append(dict(t0=bar_index * bar, freq=73.416, duration=0.55,
                           wave="sine", gain=0.26, attack=0.006,
                           decay=0.20, sustain=0.22, release=0.30))
        events.append(dict(t0=bar_index * bar + bar * 0.5, freq=110.0,
                           duration=0.30, wave="sine", gain=0.10,
                           attack=0.004, decay=0.10, sustain=0.15,
                           release=0.18))
    return _bgm_spec(length, events, cutoff_hz=2400.0, fade=0.6, peak=0.89)


def _bgm_spec(length, events, cutoff_hz, fade, peak):
    """把 (base, mode, degree) 形式的事件解析成绝对 Hz。"""
    resolved = []
    for event in events:
        item = dict(event)
        base = item.pop("base", None)
        mode = item.pop("mode", "gong")
        if base is None:
            # 绝对频率事件：不做五声取音
            resolved.append(item)
            continue
        item["freq"] = _deg(base, item.pop("degree"), mode)
        resolved.append(item)
    return {
        "length": length,
        "events": resolved,
        "cutoff_hz": cutoff_hz,
        "fade": fade,
        "peak": peak,
    }


# ---------------------------------------------------------------------------
# 音效：11 个，全部 <= 0.6s
# ---------------------------------------------------------------------------
def _sfx_note(play, t0, freq, duration, wave="sine", gain=0.5,
              attack=0.005, decay=0.05, sustain=0.5, release=0.06,
              layers=None):
    play.note(t0=t0, freq=freq, duration=duration, wave=wave, gain=gain,
              attack=attack, decay=decay, sustain=sustain, release=release,
              layers=layers)


def _render_sfx(play, seconds, cutoff_hz=None, highpass_hz=None, peak=0.9):
    samples = int(round(seconds * SR))
    mix = play.render(samples)
    if cutoff_hz:
        one_pole(mix, cutoff_hz)
    if highpass_hz:
        high_pass(mix, highpass_hz)
    # 末尾 3ms 强制归零，杜绝尾部爆音
    tail = min(int(0.003 * SR), samples)
    for i in range(tail):
        mix[samples - 1 - i] *= (i / float(tail)) if tail else 0.0
    mix, _peak = scale_to_peak(mix, peak)
    return mix


def sfx_ui_click():
    """ui_click 短促点击：0.06s 高频短脉冲。"""
    play = Play()
    _sfx_note(play, 0.0, 1760.0, 0.06, wave="sine", gain=0.8,
              attack=0.0015, decay=0.014, sustain=0.16, release=0.03,
              layers=((1.0, 1.0), (2.0, 0.30)))
    return _render_sfx(play, 0.06, highpass_hz=420.0, peak=0.86)


def sfx_ui_move():
    """ui_move 选择移动：0.05s 轻短音（比 ui_click 更低更轻，按住连发时不刺耳）。"""
    play = Play()
    _sfx_note(play, 0.0, 1318.51, 0.05, wave="sine", gain=0.44,
              attack=0.002, decay=0.016, sustain=0.18, release=0.028,
              layers=((1.0, 1.0), (0.5, 0.22)))
    return _render_sfx(play, 0.05, highpass_hz=520.0, peak=0.62)


def sfx_ui_confirm():
    """ui_confirm 确认：两音上行 0.18s。"""
    play = Play()
    _sfx_note(play, 0.0, 987.767, 0.085, wave="triangle", gain=0.62,
              attack=0.003, decay=0.03, sustain=0.44, release=0.05)
    _sfx_note(play, 0.07, 1318.51, 0.11, wave="triangle", gain=0.70,
              attack=0.003, decay=0.04, sustain=0.50, release=0.07,
              layers=((1.0, 1.0), (2.0, 0.22)))
    return _render_sfx(play, 0.18, highpass_hz=300.0, peak=0.9)


def sfx_ui_cancel():
    """ui_cancel 取消：两音下行 0.18s。"""
    play = Play()
    _sfx_note(play, 0.0, 1046.50, 0.085, wave="triangle", gain=0.60,
              attack=0.003, decay=0.03, sustain=0.42, release=0.05)
    _sfx_note(play, 0.07, 783.991, 0.11, wave="triangle", gain=0.68,
              attack=0.003, decay=0.04, sustain=0.46, release=0.07,
              layers=((1.0, 1.0), (0.5, 0.30)))
    return _render_sfx(play, 0.18, highpass_hz=260.0, peak=0.88)


def sfx_hoe():
    """sfx_hoe 锄头翻土：低频噪声短促 + 快速衰减 0.25s。"""
    play = Play()
    _sfx_note(play, 0.0, 150.0, 0.25, wave="sine", gain=0.55,
              attack=0.002, decay=0.07, sustain=0.10, release=0.16,
              layers=((1.0, 1.0), (0.5, 0.35)))
    samples = int(round(0.25 * SR))
    mix = play.render(samples)
    rng = LCG(20240617)                     # 固定种子 => 完全确定性
    noise = array.array("d", bytes(8 * samples))
    state = 0.0
    alpha = 1.0 - math.exp(-TWO_PI * 320.0 / SR)
    for i in range(samples):
        state += alpha * (rng.signed() - state)
        thump = math.exp(-i / (0.055 * SR))
        noise[i] = state * thump * 1.35
    for i in range(samples):
        mix[i] += noise[i]
    return _render_sfx(play, 0.25, cutoff_hz=1400.0, peak=0.9)


def sfx_plant():
    """sfx_plant 播种：柔和短音 0.15s，带轻微下滑。"""
    play = Play()
    _sfx_note(play, 0.0, 523.251, 0.15, wave="sine", gain=0.70,
              attack=0.012, decay=0.045, sustain=0.40, release=0.09)
    _sfx_note(play, 0.028, 659.255, 0.115, wave="sine", gain=0.42,
              attack=0.014, decay=0.04, sustain=0.34, release=0.07)
    return _render_sfx(play, 0.15, highpass_hz=220.0, peak=0.82)


def sfx_water():
    """sfx_water 浇水：滤波噪声带轻微下滑 0.35s。"""
    play = Play()
    _sfx_note(play, 0.02, 440.0, 0.28, wave="sine", gain=0.14,
              attack=0.02, decay=0.08, sustain=0.35, release=0.16)
    samples = int(round(0.35 * SR))
    rng = LCG(12072024)                     # 固定种子
    raw = array.array("d", bytes(8 * samples))
    for i in range(samples):
        raw[i] = rng.signed()
    one_pole(raw, 2300.0)
    high_pass(raw, 380.0)
    mix = play.render(samples)
    for i in range(samples):
        t = i / float(samples)
        # 噪声"水量"包络：快起、先强后弱，叠加每样本微调制模拟水滴
        env = math.exp(-t * 2.1) * (1.0 - math.exp(-i / (0.012 * SR)))
        mix[i] += 0.55 * raw[i] * env
        # 下滑的共鸣分量（湿感）
        mix[i] += 0.20 * math.sin(TWO_PI * (880.0 - 300.0 * t) * (i / SR)) * env
    return _render_sfx(play, 0.35, cutoff_hz=5600.0, highpass_hz=180.0,
                       peak=0.88)


def sfx_harvest():
    """sfx_harvest 收获：明亮三音上行琶音 0.4s。"""
    play = Play()
    notes = (1046.50, 1318.51, 1567.98)
    for index, freq in enumerate(notes):
        _sfx_note(play, index * 0.10, freq, 0.26, wave="sine", gain=0.55,
                  attack=0.004, decay=0.07, sustain=0.34, release=0.17,
                  layers=((1.0, 1.0), (2.0, 0.34), (3.0, 0.12)))
    return _render_sfx(play, 0.40, highpass_hz=330.0, peak=0.9)


def sfx_hit():
    """sfx_hit 战斗命中：短促冲击 + 噪声 0.15s。"""
    play = Play()
    _sfx_note(play, 0.0, 190.0, 0.15, wave="sine", gain=0.72,
              attack=0.0015, decay=0.045, sustain=0.12, release=0.10,
              layers=((1.0, 1.0), (0.5, 0.30)))
    samples = int(round(0.15 * SR))
    rng = LCG(771113)                      # 固定种子
    raw = array.array("d", bytes(8 * samples))
    for i in range(samples):
        raw[i] = rng.signed()
    one_pole(raw, 2600.0)
    high_pass(raw, 500.0)
    mix = play.render(samples)
    for i in range(samples):
        env = math.exp(-i / (0.022 * SR))
        mix[i] += 0.75 * raw[i] * env
    return _render_sfx(play, 0.15, cutoff_hz=7000.0, highpass_hz=140.0,
                       peak=0.93)


def sfx_quest():
    """sfx_quest 任务完成：四音上行小旋律 0.5s。"""
    play = Play()
    melody = ((523.251, 0.0), (659.255, 0.11), (783.991, 0.22),
              (1046.50, 0.33))
    for freq, t0 in melody:
        _sfx_note(play, t0, freq, 0.34, wave="triangle", gain=0.52,
                  attack=0.006, decay=0.09, sustain=0.40, release=0.22,
                  layers=((1.0, 1.0), (2.0, 0.25)))
    return _render_sfx(play, 0.50, highpass_hz=240.0, peak=0.9)


def sfx_sleep():
    """sfx_sleep 睡觉：柔和下滑长音 0.5s。"""
    play = Play()
    _sfx_note(play, 0.0, 392.0, 0.36, wave="sine", gain=0.50,
              attack=0.02, decay=0.10, sustain=0.55, release=0.22)
    samples = int(round(0.5 * SR))
    mix = play.render(samples)
    # 手写相位累加实现连续下滑：660Hz -> 220Hz，三角波更柔
    phase = 0.0
    table = WAVETABLES["triangle"]
    for i in range(samples):
        freq = 660.0 * math.exp(-math.log(3.0) * (i / float(samples)))
        phase = (phase + freq * TABLE_SIZE / SR) % TABLE_SIZE
        env = math.exp(-(i / float(samples)) * 1.25) * (1.0 - math.exp(-i / (0.03 * SR)))
        mix[i] += 0.55 * table[int(phase) & TABLE_MASK] * env
    return _render_sfx(play, 0.50, cutoff_hz=3600.0, highpass_hz=150.0,
                       peak=0.85)


def sfx_craft():
    """sfx_craft 制作：两声木质敲击 0.3s。"""
    play = Play()
    for t0, freq in ((0.0, 620.0), (0.155, 480.0)):
        _sfx_note(play, t0, freq, 0.13, wave="triangle", gain=0.55,
                  attack=0.002, decay=0.035, sustain=0.12, release=0.09,
                  layers=((1.0, 1.0), (2.0, 0.30), (3.0, 0.12)))
    samples = int(round(0.30 * SR))
    rng = LCG(90210)                       # 固定种子
    raw = array.array("d", bytes(8 * samples))
    for i in range(samples):
        raw[i] = rng.signed()
    one_pole(raw, 2100.0)
    high_pass(raw, 600.0)
    mix = play.render(samples)
    for strike in (0.0, 0.155):
        start = int(strike * SR)
        for i in range(samples - start):
            if i > int(0.06 * SR):
                break
            mix[start + i] += 0.45 * raw[start + i] * math.exp(-i / (0.014 * SR))
    return _render_sfx(play, 0.30, cutoff_hz=5200.0, highpass_hz=160.0,
                       peak=0.88)


# ---------------------------------------------------------------------------
# 写出 + 自检
# ---------------------------------------------------------------------------
def seam_jump(samples):
    """循环点诊断：把缓冲当作圆环，量最大的样本跳变 / 峰值。

    seam 处各音符合成事件的包络都从 0 出发、回到 0，且相位按整数个周期推进，
    因此该比值应与"段内相邻样本的典型跳变"同量级（不会出现满幅爆音）。
    """
    if len(samples) < 3:
        return 0.0, 0.0
    peak = 0
    for value in samples:
        mag = value if value >= 0 else -value
        if mag > peak:
            peak = mag
    if peak == 0:
        return 0.0, 0.0
    seam = abs(int(samples[0]) - int(samples[-1]))
    inner = abs(int(samples[1]) - int(samples[0]))
    if inner > seam:
        seam = inner
    return seam / float(peak), seam


def write_wav(path, samples):
    """16-bit 单声道 PCM 写盘（小端由 wave 模块保证）。"""
    pcm = array.array("h")
    pcm.extend(int(max(-1.0, min(1.0, value)) * FULL_SCALE) for value in samples)
    if sys.byteorder == "big":
        pcm.byteswap()
    with wave.open(path, "wb") as handle:
        handle.setnchannels(CHANNELS)
        handle.setsampwidth(SAMPLE_WIDTH)
        handle.setframerate(SR)
        handle.writeframes(pcm.tobytes())


def active_ratio(samples):
    """|v| > 100 的样本占比（以 16-bit 计）。"""
    if not samples:
        return 0.0
    loud = 0
    for value in samples:
        scaled = int(value * FULL_SCALE)
        if scaled > ACTIVE_ABS or scaled < -ACTIVE_ABS:
            loud += 1
    return loud / float(len(samples))


def fail(name, reason):
    """自检失败：抛出异常，由 main 打印细节并 sys.exit(1)。"""
    raise GenAudioError("%s: %s" % (name, reason))


class GenAudioError(Exception):
    """资源自检失败。"""


def verify(path, expected_seconds, is_bgm, loop_check):
    """自检：头参数、帧数、非空音频、时长/体积上限、循环点。

    任何一项失败都会抛 GenAudioError（由 main 转成 sys.exit(1)）。
    """
    name = os.path.basename(path)
    if not os.path.isfile(path):
        fail(name, "文件未生成")
    size = os.path.getsize(path)
    if size <= 44:
        fail(name, "文件为空（%d 字节）" % size)
    try:
        with wave.open(path, "rb") as handle:
            channels = handle.getnchannels()
            width = handle.getsampwidth()
            rate = handle.getframerate()
            frames = handle.getnframes()
            raw = handle.readframes(frames)
    except GenAudioError:
        raise
    except Exception as error:                     # noqa: BLE001
        fail(name, "无法用 wave 打开：%s" % error)
        return None
    if channels != CHANNELS:
        fail(name, "声道数 %d != %d" % (channels, CHANNELS))
    if rate != SR:
        fail(name, "采样率 %d != %d" % (rate, SR))
    if width != SAMPLE_WIDTH:
        fail(name, "位深 %d 字节 != 16-bit" % width)
    if frames <= 0:
        fail(name, "帧数为 0")
    samples = array.array("h")
    samples.frombytes(raw)
    if sys.byteorder == "big":
        samples.byteswap()
    if len(samples) != frames:
        fail(name, "样本数 %d != 帧数 %d" % (len(samples), frames))
    duration = frames / float(SR)
    if abs(duration - expected_seconds) > DUR_TOLERANCE_S:
        fail(name, "时长 %.6fs 与预期 %.6fs 误差 %.1fms 超过 %.0fms"
             % (duration, expected_seconds,
                abs(duration - expected_seconds) * 1000.0,
                DUR_TOLERANCE_S * 1000.0))
    if is_bgm:
        if size > BGM_MAX_BYTES:
            fail(name, "BGM 体积 %d 字节超过 2MB" % size)
    elif duration > SFX_MAX_SECONDS + 1e-9:
        fail(name, "音效时长 %.4fs 超过 %.2fs" % (duration, SFX_MAX_SECONDS))
    ratio = active_ratio(samples)
    if ratio < ACTIVE_RATIO_MIN:
        fail(name, "有效样本仅 %.3f%%（要求 >= %.1f%%，|样本| > %d）"
             % (ratio * 100.0, ACTIVE_RATIO_MIN * 100.0, ACTIVE_ABS))
    head = samples[0] / float(FULL_SCALE)
    tail = samples[-1] / float(FULL_SCALE)
    if loop_check and (abs(head) > LOOP_EDGE_TOLERANCE
                       or abs(tail) > LOOP_EDGE_TOLERANCE):
        fail(name, "循环点振幅过大：首 %+.4f 尾 %+.4f（上限 %.3f）"
             % (head, tail, LOOP_EDGE_TOLERANCE))
    seam_ratio, seam_abs = seam_jump(samples) if loop_check else (0.0, 0)
    with open(path, "rb") as handle:
        digest = hashlib.sha256(handle.read()).hexdigest()
    return {
        "name": name,
        "path": path,
        "duration": duration,
        "size": size,
        "frames": frames,
        "active": ratio,
        "head": head,
        "tail": tail,
        "seam_ratio": seam_ratio,
        "seam_abs": seam_abs,
        "sha256": digest,
        "is_bgm": is_bgm,
    }


# ---------------------------------------------------------------------------
# 主流程
# ---------------------------------------------------------------------------
def build_jobs():
    return [
        # (文件名, 生成器, 预期时长, 是否 BGM, 循环校验)
        ("bgm_title.wav", build_bgm_title, 16.0, True, True),
        ("bgm_manor.wav", build_bgm_manor, 14.0, True, True),
        ("bgm_village.wav", build_bgm_village, 14.0, True, True),
        ("bgm_wilds.wav", build_bgm_wilds, 16.0, True, True),
        ("ui_click.wav", sfx_ui_click, 0.06, False, False),
        ("ui_move.wav", sfx_ui_move, 0.05, False, False),
        ("ui_confirm.wav", sfx_ui_confirm, 0.18, False, False),
        ("ui_cancel.wav", sfx_ui_cancel, 0.18, False, False),
        ("sfx_hoe.wav", sfx_hoe, 0.25, False, False),
        ("sfx_plant.wav", sfx_plant, 0.15, False, False),
        ("sfx_water.wav", sfx_water, 0.35, False, False),
        ("sfx_harvest.wav", sfx_harvest, 0.40, False, False),
        ("sfx_hit.wav", sfx_hit, 0.15, False, False),
        ("sfx_quest.wav", sfx_quest, 0.50, False, False),
        ("sfx_sleep.wav", sfx_sleep, 0.50, False, False),
        ("sfx_craft.wav", sfx_craft, 0.30, False, False),
    ]


def main():
    if not os.path.isdir(OUT_DIR):
        os.makedirs(OUT_DIR)
    reports = []
    for name, builder, seconds, is_bgm, loop_check in build_jobs():
        try:
            spec = builder()
            if is_bgm:
                length = spec["length"]
                if abs(length - seconds) > 1e-9:
                    fail(name, "BGM 长度 %.4fs 与声明 %.4fs 不一致"
                         % (length, seconds))
                if not (12.0 <= length <= 20.0):
                    fail(name, "BGM 时长 %.4fs 不在 12~20s" % length)
                samples = assemble(spec["events"], int(round(length * SR)),
                                   length, True, spec["fade"],
                                   spec["cutoff_hz"], spec["peak"])
            else:
                samples = spec
            if not len(samples):
                fail(name, "合成器未产出任何样本")
            path = os.path.join(OUT_DIR, name)
            write_wav(path, samples)
            del samples
            reports.append(verify(path, seconds, is_bgm, loop_check))
        except GenAudioError as error:
            print("gen_audio_FAIL %s" % error)
            return 1
    for report in reports:
        loop_note = ""
        if report["is_bgm"]:
            loop_note = "  seam=%5.2f%% peak (%d LSB)" % (
                report["seam_ratio"] * 100.0, report["seam_abs"])
        print("%-16s %7.3fs %8d bytes  active=%5.1f%%  loop_head=%+.4f "
              "loop_tail=%+.4f%s  sha256=%s"
              % (report["name"], report["duration"], report["size"],
                 report["active"] * 100.0, report["head"], report["tail"],
                 loop_note, report["sha256"][:16]))
    print("gen_audio_ok files=%d" % len(reports))
    return 0


if __name__ == "__main__":
    sys.exit(main())
