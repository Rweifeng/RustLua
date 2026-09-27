//! 工具：OGG Vorbis → 16-bit PCM WAV。
//!
//! 用途：微信小游戏的 InnerAudioContext 在 iOS 上不支持 ogg（官方格式矩阵
//! ogg: iOS ✗ / Android ✓），因此微信发布包需要一份 wav 备用格式。本工具用
//! rodio 解码 ogg（与引擎 audio.rs 同源），写出 16-bit PCM WAV。
//!
//! 运行：cargo run -p engine --example ogg2wav -- <in.ogg> <out.wav> [--rate <hz>]
//!   --rate 22050 时按整数因子平均降采样（默认保持原始采样率）。
//!   输出单声道（多声道输入取左声道，与引擎 wav2ogg.rs 的处理一致）。

use rodio::Source;
use std::io::Cursor;
use std::process::exit;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: ogg2wav <in.ogg> <out.wav> [--rate <hz>]");
        exit(1);
    }
    let input = &args[1];
    let output = &args[2];
    let mut target_rate = 0u32;
    let mut i = 3;
    while i < args.len() {
        if args[i] == "--rate" && i + 1 < args.len() {
            target_rate = args[i + 1]
                .parse::<u32>()
                .map_err(|error| format!("--rate 不是整数：{error}"))?;
            i += 2;
        } else {
            eprintln!("未知参数：{}", args[i]);
            exit(1);
        }
    }

    let bytes =
        std::fs::read(input).map_err(|error| format!("读取输入失败（{input}）：{error}"))?;
    let decoder = rodio::Decoder::new(Cursor::new(bytes))
        .map_err(|error| format!("OGG 解码失败（{input}）：{error}"))?;
    let sample_rate = decoder.sample_rate();
    let channels = decoder.channels();
    let samples: Vec<i16> = decoder.collect();

    if samples.is_empty() {
        return Err(format!("OGG 中没有音频样本：{input}").into());
    }
    let frames = samples.len() / usize::from(channels);
    // 取左声道（与 wav2ogg.rs 的"双声道取左"一致）。
    let mut mono = Vec::with_capacity(frames);
    for frame in 0..frames {
        mono.push(samples[frame * usize::from(channels)]);
    }

    // 整数因子平均降采样：factor = ceil(rate / target)，输出率 = rate / factor。
    let (out_rate, out_samples) = if target_rate > 0 && target_rate < sample_rate {
        let factor = sample_rate.div_ceil(target_rate) as usize;
        let mut out = Vec::with_capacity(mono.len() / factor + 1);
        let mut idx = 0usize;
        while idx < mono.len() {
            let end = (idx + factor).min(mono.len());
            let mut sum = 0i64;
            for s in &mono[idx..end] {
                sum += i64::from(*s);
            }
            out.push(
                (sum / (end - idx) as i64).clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16,
            );
            idx = end;
        }
        (sample_rate / factor as u32, out)
    } else {
        (sample_rate, mono)
    };

    write_pcm16_wav(output, out_rate, &out_samples)?;
    let seconds = out_samples.len() as f64 / f64::from(out_rate);
    let out_bytes = std::fs::metadata(output)?.len();
    println!(
        "ogg2wav_ok in={input} rate={sample_rate} ch={channels} frames={frames} -> out={output} rate={out_rate} samples={} duration={seconds:.2}s bytes={out_bytes}",
        out_samples.len()
    );
    Ok(())
}

/// 写出 16-bit PCM 单声道 WAV（RIFF/WAVE）。
fn write_pcm16_wav(
    path: &str,
    sample_rate: u32,
    samples: &[i16],
) -> Result<(), Box<dyn std::error::Error>> {
    const BYTES_PER_SAMPLE: u32 = 2;
    let data_bytes = (samples.len() as u32)
        .checked_mul(BYTES_PER_SAMPLE)
        .ok_or("样本数溢出")?;
    let byte_rate = sample_rate
        .checked_mul(BYTES_PER_SAMPLE)
        .ok_or("采样率溢出")?;
    let riff_size = 4 + (8 + 16) + (8 + data_bytes);

    let mut out = Vec::with_capacity(44 + data_bytes as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // 单声道
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&(2u16).to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, out).map_err(|error| format!("写出失败（{path}）：{error}"))?;
    Ok(())
}
