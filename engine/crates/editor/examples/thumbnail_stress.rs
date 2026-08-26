use engine::core::texture::{TextureAsset, TextureColorSpace};
use std::{
    fs,
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const THUMBNAILS: usize = 1000;
const MAX_SIZE: usize = 64;

fn unique_token() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be valid")
        .as_nanos()
}

fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("rustlua-m15-thumbnail-{label}-{}", unique_token()))
}

fn sample_project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../game_test_2d_production")
}

fn downscale_rgba(
    pixels: &[u8],
    width: usize,
    height: usize,
    max: usize,
) -> (Vec<u8>, usize, usize) {
    if (width <= max && height <= max) || width == 0 || height == 0 {
        return (pixels.to_vec(), width, height);
    }
    let scale = max as f32 / width.max(height) as f32;
    let new_w = ((width as f32 * scale).round() as usize).max(1);
    let new_h = ((height as f32 * scale).round() as usize).max(1);
    let mut out = vec![0u8; new_w * new_h * 4];
    for y in 0..new_h {
        let sy = ((y as f32 + 0.5) / new_h as f32 * height as f32) as usize;
        let sy = sy.min(height - 1);
        for x in 0..new_w {
            let sx = ((x as f32 + 0.5) / new_w as f32 * width as f32) as usize;
            let sx = sx.min(width - 1);
            let src = (sy * width + sx) * 4;
            let dst = (y * new_w + x) * 4;
            out[dst..dst + 4].copy_from_slice(&pixels[src..src + 4]);
        }
    }
    (out, new_w, new_h)
}

fn main() {
    let source = sample_project_root().join("assets/textures/slime.png");
    assert!(
        source.is_file(),
        "sample texture missing: {}",
        source.display()
    );

    let root = temp_root("stress");
    fs::create_dir_all(&root).expect("create temp root");
    let mut files = Vec::with_capacity(THUMBNAILS);
    for index in 0..THUMBNAILS {
        let path = root.join(format!("thumb-{index:04}.png"));
        fs::copy(&source, &path).expect("copy sample texture");
        files.push(path);
    }

    let started = Instant::now();
    let mut total_rgba_bytes = 0usize;
    let mut total_thumb_bytes = 0usize;
    let mut total_pixels = 0usize;
    for path in &files {
        let asset = TextureAsset::load_png(path, TextureColorSpace::Srgb).expect("load png");
        total_rgba_bytes += asset.pixels().len();
        let (thumb, width, height) = downscale_rgba(
            asset.pixels(),
            asset.width() as usize,
            asset.height() as usize,
            MAX_SIZE,
        );
        total_thumb_bytes += thumb.len();
        total_pixels += width * height;
    }
    let total_ms = started.elapsed().as_secs_f64() * 1000.0;
    println!(
        "thumbnail_stress_ok thumbnails={THUMBNAILS} total_ms={total_ms:.3} avg_ms={:.3} rgba_bytes={} thumb_bytes={} thumb_pixels={}",
        total_ms / THUMBNAILS as f64,
        total_rgba_bytes,
        total_thumb_bytes,
        total_pixels
    );

    let _ = fs::remove_dir_all(root);
}
