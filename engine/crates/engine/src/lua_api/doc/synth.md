# Lua 模块 `synth`

> 运行期 API 面基线里本模块有 1 个函数。本页由 `scripts/docgen/gen-lua-api-docs.js` 生成：
> **函数清单与标题里的签名**不要手改；在每个小节里把
> `TODO(文档桩)` 换成真实说明即可，再生成会保留已补的内容。

<!-- 以上为自动生成头，以下第一段散文是模块说明（再生成时保留） -->

合成音效原语（M82 C 批）：按规格表在 CPU 侧合成 PCM 采样（振荡器 + 噪声 + ADSR 包络 + biquad 滤波），
桌面包成 rodio 采样缓冲挂上 Sink。这是**维度无关基底**（2D/3D 构建都登记）。只交付合成原语，
不含"调得像枪声/引擎"的预设——那是人耳定标，不进机器门。web/微信的平台出口本批无合成通道（返回 `false` + 告警）。

## `synth.play(spec: Table)`

> 绑定：`engine/crates/rustlua-audio/src/lua.rs:330`（直写）；签名由 `scripts/docgen/gen-lua-api-docs.js` 从绑定源码推断，需人工核对。

按规格表合成并播放一次性音效。桌面走音频设备（与 `audio.play` 同一条 Sink 通道）；规格非法 / 无设备 /
平台出口不支持合成时返回 `false` 并告警（不静默成功）。

- 参数：`spec` 表——
  - `wave`：`sine` | `square` | `saw` | `triangle` | `noise`，缺省 `sine`。
  - `freq`：基频 Hz，`20..=20000`（噪声忽略），缺省 `440`。
  - `dur`：按键持续秒，`0.01..=10`，缺省 `0.3`（总长 = `dur + release`）。
  - `attack` / `decay` / `release`：包络段秒（`0..=10`，缺省 `0.01` / `0.05` / `0.1`）。
  - `sustain`：持续电平 `0..=1`，缺省 `0.7`。
  - `gain`：峰值增益 `0..=1`，缺省 `0.5`。
  - `filter`：可选 `{ type = "lowpass"|"highpass", cutoff = Hz }`（缺省无滤波）。
- 返回：`boolean`——是否真的发出了声音（`false` = 非法 spec / 无设备 / 平台不支持，已告警）。
- 失败：不抛错；非法 spec / 无设备 / 平台不支持合成时返回 `false`。
- 示例：`local ok = synth.play{wave='square', freq=220, dur=0.1, filter={type='lowpass', cutoff=1200}}`
