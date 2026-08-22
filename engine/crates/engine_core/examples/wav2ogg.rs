//! 工具：16-bit PCM WAV → OGG Vorbis（用于把 bgm.wav 压缩为 bgm.ogg）。
//! 运行：cargo run -p engine_core --example wav2ogg -- <in.wav> <out.ogg>

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: wav2ogg <in.wav> <out.ogg>");
        std::process::exit(1);
    }
    let bytes = std::fs::read(&args[1]).map_err(|error| format!("读取输入失败：{error}"))?;
    let (sample_rate, samples) = parse_pcm16_wav(&bytes)?;

    let mut encoder = vorbis::Encoder::new(
        1,
        u64::from(sample_rate),
        vorbis::VorbisQuality::HighQuality,
    )
    .map_err(|error| format!("创建编码器失败：{error}"))?;
    let mut out = Vec::new();
    out.extend(
        encoder
            .encode(&samples)
            .map_err(|error| format!("编码失败：{error}"))?,
    );
    out.extend(
        encoder
            .flush()
            .map_err(|error| format!("冲刷失败：{error}"))?,
    );
    std::fs::write(&args[2], out).map_err(|error| format!("写出失败：{error}"))?;
    println!(
        "wav2ogg_ok rate={sample_rate} samples={} -> {}",
        samples.len(),
        args[2]
    );
    Ok(())
}

/// 解析 16-bit PCM（单声道或双声道）WAV，返回 (采样率, i16 样本)。
fn parse_pcm16_wav(bytes: &[u8]) -> Result<(u32, Vec<i16>), String> {
    // 最短合法 RIFF/WAVE 头为 12 字节；先做长度守卫避免切片越界 panic。
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("不是 RIFF/WAVE 文件".to_owned());
    }
    let mut pos = 12;
    let mut channels = 1u16;
    let mut sample_rate = 0u32;
    let mut bits = 16u16;
    let mut data = Vec::new();
    while pos + 8 <= bytes.len() {
        let chunk_id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = pos + 8;
        if chunk_id == b"fmt " {
            // fmt 块至少 16 字节：format/channels/sample_rate/…/bits。
            if size >= 16 && body + 16 <= bytes.len() {
                channels = u16::from_le_bytes(bytes[body + 2..body + 4].try_into().unwrap());
                sample_rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into().unwrap());
                bits = u16::from_le_bytes(bytes[body + 14..body + 16].try_into().unwrap());
            }
        } else if chunk_id == b"data" {
            let end = (body + size).min(bytes.len());
            data.extend_from_slice(&bytes[body..end]);
        }
        pos = body + size + (size % 2);
    }
    if sample_rate == 0 || bits != 16 {
        return Err("仅支持 16-bit PCM WAV".to_owned());
    }
    let frame_size = 2 * usize::from(channels);
    let mut samples = Vec::with_capacity(data.len() / 2);
    for frame in data.chunks_exact(frame_size) {
        // 单声道取前 2 字节；双声道取左声道。
        samples.push(i16::from_le_bytes([frame[0], frame[1]]));
    }
    if samples.is_empty() {
        return Err("WAV 中没有音频数据".to_owned());
    }
    Ok((sample_rate, samples))
}
