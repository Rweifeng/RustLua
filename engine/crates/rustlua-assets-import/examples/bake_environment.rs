//! 烘焙环境资产：`studio.hdr` 夹具生成 + `.hdr` → `.rlenv`。
//!
//! ```text
//! cargo run -p rustlua-assets-import --example bake_environment -- --fixture
//! cargo run -p rustlua-assets-import --example bake_environment -- in.hdr out.rlenv [face_size] [mip_count]
//! ```

use std::path::{Path, PathBuf};

use rustlua_assets_import::environment::{
    BakeOptions, HdrImage, bake_hdr_bytes, decode_hdr, encode_radiance,
};
use rustlua_math::math3d::Vec3;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--fixture") {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let directory = root.join("examples/game_api/assets/env");
        std::fs::create_dir_all(&directory)?;
        let image = studio_equirect(64, 32);
        let hdr_path = directory.join("studio.hdr");
        std::fs::write(&hdr_path, encode_radiance(&image))?;
        println!(
            "wrote {} ({}x{}, {} bytes)",
            hdr_path.display(),
            image.width(),
            image.height(),
            std::fs::metadata(&hdr_path)?.len()
        );
        bake_csv(
            &hdr_path,
            &directory.join("studio.rlenv"),
            BakeOptions::default(),
        )?;
        return Ok(());
    }
    let input = args
        .first()
        .ok_or("用法：bake_environment <in.hdr> <out.rlenv> [face] [mip] 或 --fixture")?;
    let output = args.get(1).ok_or("缺少输出路径")?;
    let face_size = args
        .get(2)
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(BakeOptions::default().face_size);
    let mip_count = args
        .get(3)
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(BakeOptions::default().mip_count);
    bake_csv(
        Path::new(input),
        Path::new(output),
        BakeOptions {
            face_size,
            mip_count,
        },
    )
}

fn bake_csv(
    input: &Path,
    output: &Path,
    options: BakeOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let source = std::fs::read(input)?;
    let image = decode_hdr(&source)?;
    let report = bake_hdr_bytes(&source, options)?;
    let bytes = report.encode()?;
    std::fs::write(output, &bytes)?;
    println!(
        "source={}x{} face={} mips={} bytes={} bake_ms={:.1} samples={}",
        report.source_width,
        report.source_height,
        options.face_size,
        options.mip_count,
        bytes.len(),
        report.bake_ms,
        report.samples_per_texel
    );
    println!("wrote {}", output.display());
    println!(
        "sh0={:.4},{:.4},{:.4}",
        report.asset.sh[0], report.asset.sh[1], report.asset.sh[2]
    );
    let sh = rustlua_assets::env_asset::sh_irradiance(&report.asset.sh, Vec3::Y);
    println!("irradiance(+Y)={:.4},{:.4},{:.4}", sh[0], sh[1], sh[2]);
    let _ = image;
    Ok(())
}

/// 合成的 studio 环境：天顶蓝 → 地平线灰白、地面暖褐，加一个高亮太阳。
///
/// 尺寸小（64×32）却有 HDR 峰值（太阳 ~40），足以验证线性解码与镜面预过滤。
fn studio_equirect(width: u32, height: u32) -> HdrImage {
    let sun = Vec3::new(0.35, 0.55, -0.76)
        .normalized()
        .expect("sun direction");
    let zenith = [0.08_f32, 0.16, 0.42];
    let horizon = [0.65_f32, 0.72, 0.85];
    let ground = [0.16_f32, 0.13, 0.10];
    let sun_color = [1.0_f32, 0.82, 0.55];
    let pixels = (0..height)
        .flat_map(|y| {
            (0..width).map(move |x| {
                let u = (x as f32 + 0.5) / width as f32;
                let v = (y as f32 + 0.5) / height as f32;
                let phi = (u - 0.5) * std::f32::consts::TAU;
                let theta = v * std::f32::consts::PI;
                let direction = Vec3::new(
                    phi.cos() * theta.sin(),
                    theta.cos(),
                    phi.sin() * theta.sin(),
                );
                let base = if direction.y >= 0.0 {
                    lerp3(horizon, zenith, direction.y.powf(0.45))
                } else {
                    lerp3(
                        [horizon[0] * 0.75, horizon[1] * 0.75, horizon[2] * 0.75],
                        ground,
                        (-direction.y).powf(0.35),
                    )
                };
                let cos_angle = direction.dot(sun).max(0.0);
                let glow = cos_angle.powf(96.0) * 40.0 + cos_angle.powf(8.0) * 0.6;
                [
                    base[0] + sun_color[0] * glow,
                    base[1] + sun_color[1] * glow,
                    base[2] + sun_color[2] * glow,
                ]
            })
        })
        .collect();
    HdrImage::new(width, height, pixels).expect("studio image")
}

fn lerp3(from: [f32; 3], to: [f32; 3], t: f32) -> [f32; 3] {
    [
        from[0] + (to[0] - from[0]) * t,
        from[1] + (to[1] - from[1]) * t,
        from[2] + (to[2] - from[2]) * t,
    ]
}
