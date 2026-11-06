//! Procedural ground detail atlas for the built-in terrain.
//!
//! The terrain sentinel deliberately stays a **numeric mesh path** (see
//! [`crate::terrain::TerrainSpec`]): no scene-side texture reference, no asset
//! file, identical result on every platform and inside a packed build. The
//! visual detail therefore rides in a generated 2x2 atlas (512², four 256²
//! tiles) that is embedded into the terrain material as a
//! [`rustlua_assets::gltf_asset::GltfTextureImage`]:
//!
//! ```text
//! +---------+---------+
//! |  grass  |  moss   |
//! +---------+---------+
//! | litter  |  soil   |
//! +---------+---------+
//! ```
//!
//! The PBR terrain branch samples the atlas with **world-space** UVs, one
//! sample per layer, weighted by the per-vertex splat weights. That is a
//! textured splat, not a texture-array splatmap: the four layers share one
//! sampler, stay tileable, and receive a generated mip chain so the god view's
//! far field fades to a stable average instead of shimmering.
//!
//! Tiles are built from a periodic value-noise fBm. Periodicity is what makes
//! the `fract(world / scale)` lookup seamless; the generator never draws shapes
//! that would need extra wrapping code.

use rustlua_assets::gltf_asset::GltfTextureImage;

/// Edge length of one atlas tile in texels.
pub const DETAIL_TILE_SIZE: u32 = 256;
/// Edge length of the whole 2x2 atlas in texels.
pub const DETAIL_ATLAS_SIZE: u32 = DETAIL_TILE_SIZE * 2;

/// Builds the deterministic 2x2 ground detail atlas.
///
/// Same `seed` -> identical pixels. The seed is the terrain seed, so two
/// terrains with different seeds also get different ground variation while
/// staying visually consistent.
#[must_use]
pub fn ground_atlas(seed: u32) -> GltfTextureImage {
    let size = DETAIL_ATLAS_SIZE;
    let mut rgba = vec![255u8; (size * size * 4) as usize];
    let blobs = tile_blobs(seed);
    for tile in 0..4u32 {
        let origin_x = (tile % 2) * DETAIL_TILE_SIZE;
        let origin_y = (tile / 2) * DETAIL_TILE_SIZE;
        for y in 0..DETAIL_TILE_SIZE {
            for x in 0..DETAIL_TILE_SIZE {
                // Sample at texel centres so the periodic domain is symmetric.
                let u = (x as f32 + 0.5) / DETAIL_TILE_SIZE as f32;
                let v = (y as f32 + 0.5) / DETAIL_TILE_SIZE as f32;
                let rgb = tile_color(tile, u, v, seed, &blobs);
                let index = (((origin_y + y) * size + origin_x + x) * 4) as usize;
                rgba[index] = encode_srgb(rgb[0]);
                rgba[index + 1] = encode_srgb(rgb[1]);
                rgba[index + 2] = encode_srgb(rgb[2]);
                rgba[index + 3] = 255;
            }
        }
    }
    GltfTextureImage {
        width: size,
        height: size,
        rgba,
    }
}

/// Derives a 2x2 **terrain normal atlas** from the colour atlas' luminance.
///
/// The colour atlas is the visual source of truth: grass blades, leaf edges and
/// pebbles are exactly the places where the ground should catch light. Each tile
/// is differentiated independently and wraps **inside its own tile**, never
/// across the atlas border, so the four layers stay seamless after blending.
///
/// Encoding is `R = n.x * 0.5 + 0.5`, `G = n.z * 0.5 + 0.5`, `B = n.y * 0.5 + 0.5`
/// (linear RGBA8; the PBR terrain branch reconstructs the normal from RG only).
#[must_use]
pub fn normal_atlas(color: &GltfTextureImage, strength: f32) -> GltfTextureImage {
    let size = DETAIL_ATLAS_SIZE;
    let mut rgba = vec![255u8; (size * size * 4) as usize];
    let luma = |x: u32, y: u32, tile_x: u32, tile_y: u32| -> f32 {
        // Wrap inside the 256px tile: neighbouring tiles must not bleed their
        // luminance gradient into this tile's edges.
        let px = (x + DETAIL_TILE_SIZE) % DETAIL_TILE_SIZE;
        let py = (y + DETAIL_TILE_SIZE) % DETAIL_TILE_SIZE;
        let index = (((tile_y * DETAIL_TILE_SIZE + py) * size + (tile_x * DETAIL_TILE_SIZE + px))
            * 4) as usize;
        let r = f32::from(color.rgba[index]);
        let g = f32::from(color.rgba[index + 1]);
        let b = f32::from(color.rgba[index + 2]);
        (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
    };
    for tile in 0..4u32 {
        let tile_x = tile % 2;
        let tile_y = tile / 2;
        for y in 0..DETAIL_TILE_SIZE {
            for x in 0..DETAIL_TILE_SIZE {
                let left = luma(
                    (x + DETAIL_TILE_SIZE - 1) % DETAIL_TILE_SIZE,
                    y,
                    tile_x,
                    tile_y,
                );
                let right = luma((x + 1) % DETAIL_TILE_SIZE, y, tile_x, tile_y);
                let up = luma(
                    x,
                    (y + DETAIL_TILE_SIZE - 1) % DETAIL_TILE_SIZE,
                    tile_x,
                    tile_y,
                );
                let down = luma(x, (y + 1) % DETAIL_TILE_SIZE, tile_x, tile_y);
                // Height grows with luminance; the surface normal is (-dh/dx, 1, -dh/dy).
                let dx = (right - left) * strength;
                let dz = (down - up) * strength;
                let length = (dx * dx + 1.0 + dz * dz).sqrt();
                let nx = (-dx / length).clamp(-1.0, 1.0);
                let ny = (1.0 / length).clamp(0.0, 1.0);
                let nz = (-dz / length).clamp(-1.0, 1.0);
                let index = (((tile_y * DETAIL_TILE_SIZE + y) * size
                    + (tile_x * DETAIL_TILE_SIZE + x))
                    * 4) as usize;
                rgba[index] = encode_normal_component(nx);
                rgba[index + 1] = encode_normal_component(nz);
                rgba[index + 2] = encode_normal_component(ny);
                rgba[index + 3] = 255;
            }
        }
    }
    GltfTextureImage {
        width: size,
        height: size,
        rgba,
    }
}

/// Blob lists for the four tiles, generated once per atlas instead of once per
/// texel (the ellipses are static marks, only the per-pixel coverage changes).
struct TileBlobs {
    grass: Vec<Blob>,
    moss: Vec<Blob>,
    litter: Vec<Blob>,
    soil: Vec<Blob>,
}

fn tile_blobs(seed: u32) -> TileBlobs {
    TileBlobs {
        grass: make_blobs(seed ^ 0x6A09_E667, 54, 0.018, 0.045, 0.004, 0.010),
        moss: make_blobs(seed ^ 0x3C6E_F372, 26, 0.02, 0.07, 0.015, 0.05),
        litter: make_blobs(seed ^ 0x1234_5678, 42, 0.032, 0.080, 0.016, 0.040),
        soil: make_blobs(seed ^ 0xBEEF, 30, 0.006, 0.018, 0.005, 0.014),
    }
}

/// Linear-space target color for one tile at normalized tile coordinates.
///
/// The values are authored in linear space because the shader consumes the
/// decoded (linear) sRGB texel directly; [`encode_srgb`] handles the encoding.
fn tile_color(tile: u32, u: f32, v: f32, seed: u32, blobs: &TileBlobs) -> [f32; 3] {
    let seed = seed.wrapping_mul(2_654_435_761).wrapping_add(tile);
    match tile {
        0 => grass(u, v, seed, &blobs.grass),
        1 => moss(u, v, seed, &blobs.moss),
        2 => litter(u, v, seed, &blobs.litter),
        _ => soil(u, v, seed, &blobs.soil),
    }
}

fn grass(u: f32, v: f32, seed: u32, blades: &[Blob]) -> [f32; 3] {
    let patches = fbm(u, v, 3, 4, seed);
    let fine = fbm(u, v, 24, 2, seed ^ 0x9E37_79B9);
    // Two anisotropic noises give the fine streaky look of blades seen from above.
    let streaks = periodic_noise_xy(u, v, 96, 24, seed ^ 0x51_ed) * 0.55
        + periodic_noise_xy(u, v, 24, 96, seed ^ 0xA3_1F) * 0.45;
    let t = (0.34 * patches + 0.40 * fine + 0.26 * streaks).clamp(0.0, 1.0);
    // Wider dark/light range: the old near-uniform green read as a flat colour at
    // the god view, and the four-layer blend washed the remaining contrast out.
    let mut color = mix3([0.035, 0.075, 0.025], [0.360, 0.460, 0.180], t);
    // A few dozen long thin blades break up the noise-only look.
    let (coverage, tint) = blob_field(u, v, blades);
    let blade_color = mix3([0.070, 0.140, 0.040], [0.420, 0.500, 0.220], tint);
    color = mix3(color, blade_color, coverage * 0.80);
    // Broad dry-grass / shade patches: at the god view these survive the mip
    // chain and the four-layer blend, unlike the thin blades alone.
    let dry = fbm(u, v, 7, 3, seed ^ 0x51A3_9C2D);
    let dry_mix = smoothstep(0.52, 0.78, dry);
    color = mix3(
        color,
        mix3([0.300, 0.240, 0.090], [0.450, 0.370, 0.150], dry),
        dry_mix * 0.6,
    );
    let shade = fbm(u, v, 11, 3, seed ^ 0x2B7F_1E45);
    color = [
        (color[0] * (0.55 + shade * 0.90)).clamp(0.0, 1.0),
        (color[1] * (0.57 + shade * 0.86)).clamp(0.0, 1.0),
        (color[2] * (0.53 + shade * 0.94)).clamp(0.0, 1.0),
    ];
    // Small bare/earth gaps beneath the blades.
    let gap = fbm(u, v, 40, 2, seed ^ 0x1F3D_5B7A);
    if gap > 0.74 {
        color = mix3(color, [0.180, 0.140, 0.080], (gap - 0.74) * 2.6);
    }
    // Sparse pale blade tips keep the surface from reading as a single hue.
    if fine > 0.72 {
        color = mix3(color, [0.460, 0.520, 0.240], (fine - 0.72) * 2.4);
    }
    color
}

fn moss(u: f32, v: f32, seed: u32, cushions: &[Blob]) -> [f32; 3] {
    let clumps = fbm(u, v, 4, 4, seed);
    let tufts = fbm(u, v, 18, 2, seed ^ 0x0BADC0DE);
    let t = (0.62 * clumps + 0.38 * tufts).clamp(0.0, 1.0);
    let mut color = mix3([0.060, 0.112, 0.048], [0.178, 0.276, 0.106], t);
    // Round moss cushions rather than grass-like streaks.
    let (coverage, tint) = blob_field(u, v, cushions);
    let cushion = mix3([0.075, 0.150, 0.062], [0.215, 0.320, 0.135], tint);
    color = mix3(color, cushion, coverage * 0.6);
    // Sporophyte dots: tiny pale specks that survive the far-field mip as texture.
    let dots = fbm(u, v, 52, 2, seed ^ 0x44A1_2C3D);
    if dots > 0.84 {
        color = mix3(color, [0.345, 0.390, 0.170], (dots - 0.84) * 3.2);
    }
    if tufts > 0.70 {
        color = mix3(color, [0.246, 0.330, 0.158], (tufts - 0.70) * 2.2);
    }
    color
}

fn litter(u: f32, v: f32, seed: u32, leaves: &[Blob]) -> [f32; 3] {
    let patches = fbm(u, v, 5, 3, seed);
    let grain = fbm(u, v, 28, 2, seed ^ 0x77AA_3311);
    let background = mix3(
        [0.110, 0.074, 0.040],
        [0.170, 0.112, 0.056],
        (0.55 * patches + 0.45 * grain).clamp(0.0, 1.0),
    );
    // Fallen leaves: broad, rotated ellipses with soft edges. Wrapping is
    // handled by the periodic blob field, so leaves cross tile edges cleanly.
    let (coverage, tint) = blob_field(u, v, leaves);
    let leaf = mix3([0.240, 0.135, 0.055], [0.345, 0.205, 0.082], tint);
    let mut color = mix3(background, leaf, coverage * 0.82);
    // Fine fragments and dark gaps between leaves.
    let fine = (grain - 0.5) * 0.16;
    color = [
        (color[0] + fine).clamp(0.0, 1.0),
        (color[1] + fine).clamp(0.0, 1.0),
        (color[2] + fine).clamp(0.0, 1.0),
    ];
    color
}

fn soil(u: f32, v: f32, seed: u32, pebbles: &[Blob]) -> [f32; 3] {
    let grain = fbm(u, v, 32, 3, seed);
    let clods = fbm(u, v, 6, 3, seed ^ 0x3333_5555);
    let t = (0.55 * grain + 0.45 * clods).clamp(0.0, 1.0);
    let mut color = mix3([0.130, 0.105, 0.075], [0.245, 0.203, 0.140], t);
    // Small pebbles and clods: rotated ellipses, low contrast.
    let (coverage, tint) = blob_field(u, v, pebbles);
    let pebble = mix3([0.200, 0.182, 0.155], [0.312, 0.290, 0.248], tint);
    color = mix3(color, pebble, coverage * 0.7);
    color
}

/// One wrapping elliptical mark used by the detail tiles (leaf, pebble, blade).
#[derive(Clone, Copy)]
struct Blob {
    center_x: f32,
    center_y: f32,
    half_x: f32,
    half_y: f32,
    cos: f32,
    sin: f32,
    tint: f32,
}

fn make_blobs(
    seed: u32,
    count: u32,
    min_rx: f32,
    max_rx: f32,
    min_ry: f32,
    max_ry: f32,
) -> Vec<Blob> {
    (0..count)
        .map(|index| {
            let hash_seed = seed.wrapping_add(index.wrapping_mul(0x9E37_79B9));
            let angle = hash(index, 83, hash_seed) * std::f32::consts::TAU;
            let (sin, cos) = angle.sin_cos();
            Blob {
                center_x: hash(index, 11, hash_seed),
                center_y: hash(index, 29, hash_seed),
                half_x: min_rx + (max_rx - min_rx) * hash(index, 47, hash_seed),
                half_y: min_ry + (max_ry - min_ry) * hash(index, 61, hash_seed),
                cos,
                sin,
                tint: hash(index, 97, hash_seed),
            }
        })
        .collect()
}

/// Returns `(coverage, tint)` of the strongest blob at `(u, v)`.
///
/// Blobs are periodic: their centres wrap across the tile edge, and the
/// per-pixel delta is wrapped to `[-0.5, 0.5)`, so a leaf that crosses the edge
/// is drawn correctly on both sides without duplicating geometry.
fn blob_field(u: f32, v: f32, blobs: &[Blob]) -> (f32, f32) {
    let mut coverage = 0.0f32;
    let mut tint = 0.5f32;
    for blob in blobs {
        let delta_x = wrap_delta(u - blob.center_x);
        let delta_y = wrap_delta(v - blob.center_y);
        let local_x = delta_x * blob.cos + delta_y * blob.sin;
        let local_y = -delta_x * blob.sin + delta_y * blob.cos;
        let distance = (local_x / blob.half_x).powi(2) + (local_y / blob.half_y).powi(2);
        // Soft edge: fully inside below 0.82, faded out by 1.0.
        let blob_coverage = smoothstep(1.0, 0.82, distance);
        if blob_coverage > coverage {
            coverage = blob_coverage;
            tint = blob.tint;
        }
    }
    (coverage, tint)
}

fn wrap_delta(value: f32) -> f32 {
    value - value.round()
}

// ---------------------------------------------------------------- noise ----

fn hash(ix: u32, iy: u32, seed: u32) -> f32 {
    let mut h = ix
        .wrapping_mul(0x9E37_79B1)
        .wrapping_add(iy.wrapping_mul(0x85EB_CA6B))
        .wrapping_add(seed.wrapping_mul(0xC2B2_AE35));
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h & 0x00FF_FFFF) as f32 / 16_777_215.0
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Periodic value noise with `cells_x` x `cells_y` lattice cells; `u, v` in
/// `[0, 1)`. Indices wrap, which is exactly what keeps the generated tile
/// seamless.
fn periodic_noise_xy(u: f32, v: f32, cells_x: u32, cells_y: u32, seed: u32) -> f32 {
    let cells_x = cells_x.max(1);
    let cells_y = cells_y.max(1);
    let x = u * cells_x as f32;
    let y = v * cells_y as f32;
    let x0 = x.floor();
    let y0 = y.floor();
    let tx = smoothstep(0.0, 1.0, x - x0);
    let ty = smoothstep(0.0, 1.0, y - y0);
    let ix0 = (x0 as i64).rem_euclid(cells_x as i64) as u32;
    let iy0 = (y0 as i64).rem_euclid(cells_y as i64) as u32;
    let ix1 = (ix0 + 1) % cells_x;
    let iy1 = (iy0 + 1) % cells_y;
    let a = hash(ix0, iy0, seed);
    let b = hash(ix1, iy0, seed);
    let c = hash(ix0, iy1, seed);
    let d = hash(ix1, iy1, seed);
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    top + (bottom - top) * ty
}

fn periodic_noise(u: f32, v: f32, cells: u32, seed: u32) -> f32 {
    periodic_noise_xy(u, v, cells, cells, seed)
}

/// Fractal sum of periodic value noise, normalized to `0..=1`.
fn fbm(u: f32, v: f32, base_cells: u32, octaves: u32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut norm = 0.0;
    let mut amplitude = 0.5;
    let mut cells = base_cells.max(1);
    for octave in 0..octaves.max(1) {
        sum += amplitude * periodic_noise(u, v, cells, seed.wrapping_add(octave * 0x9E37));
        norm += amplitude;
        amplitude *= 0.5;
        cells = cells.saturating_mul(2);
    }
    if norm > 0.0 { sum / norm } else { 0.0 }
}

fn encode_normal_component(value: f32) -> u8 {
    ((value * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8
}

fn encode_srgb(linear: f32) -> u8 {
    let linear = linear.clamp(0.0, 1.0);
    let encoded = if linear <= 0.003_130_8 {
        12.92 * linear
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(image: &GltfTextureImage, x: u32, y: u32, channel: usize) -> u8 {
        image.rgba[((y * image.width + x) * 4) as usize + channel]
    }

    fn tile_average(image: &GltfTextureImage, tile: u32) -> [f32; 3] {
        let ox = (tile % 2) * DETAIL_TILE_SIZE;
        let oy = (tile / 2) * DETAIL_TILE_SIZE;
        let mut sum = [0.0f64; 3];
        for y in 0..DETAIL_TILE_SIZE {
            for x in 0..DETAIL_TILE_SIZE {
                for (c, total) in sum.iter_mut().enumerate() {
                    *total += f64::from(channel(image, ox + x, oy + y, c));
                }
            }
        }
        let n = f64::from(DETAIL_TILE_SIZE * DETAIL_TILE_SIZE);
        [
            (sum[0] / n) as f32,
            (sum[1] / n) as f32,
            (sum[2] / n) as f32,
        ]
    }

    #[test]
    fn atlas_is_a_deterministic_2x2_opaque_image() {
        let a = ground_atlas(7);
        let b = ground_atlas(7);
        assert_eq!(a, b);
        assert_eq!(a.width, DETAIL_ATLAS_SIZE);
        assert_eq!(a.height, DETAIL_ATLAS_SIZE);
        assert_eq!(
            a.rgba.len(),
            (DETAIL_ATLAS_SIZE * DETAIL_ATLAS_SIZE * 4) as usize
        );
        assert!(a.rgba.iter().skip(3).step_by(4).all(|alpha| *alpha == 255));
        let other = ground_atlas(8);
        assert_ne!(a.rgba, other.rgba, "different terrain seeds must differ");
    }

    #[test]
    fn each_tile_has_real_detail_and_they_are_visually_distinct() {
        let atlas = ground_atlas(7);
        let averages: Vec<[f32; 3]> = (0..4).map(|tile| tile_average(&atlas, tile)).collect();
        for (tile, average) in averages.iter().enumerate() {
            let ox = (tile as u32 % 2) * DETAIL_TILE_SIZE;
            let oy = (tile as u32 / 2) * DETAIL_TILE_SIZE;
            let mut min = 255u8;
            let mut max = 0u8;
            for y in 0..DETAIL_TILE_SIZE {
                for x in 0..DETAIL_TILE_SIZE {
                    let value = channel(&atlas, ox + x, oy + y, 1);
                    min = min.min(value);
                    max = max.max(value);
                }
            }
            assert!(
                max - min > 24,
                "tile {tile} is too flat (green range {min}..{max})"
            );
            assert!(
                average[1] >= average[2],
                "tile {tile} should stay in the green/brown family: {average:?}"
            );
            if tile < 2 {
                assert!(
                    average[1] >= average[0],
                    "grass/moss tile {tile} should be green-dominant: {average:?}"
                );
            } else {
                assert!(
                    average[0] >= average[1],
                    "litter/soil tile {tile} should be brown-dominant: {average:?}"
                );
            }
        }
        // Grass/moss/litter/soil must not collapse into one colour.
        let distance = |a: [f32; 3], b: [f32; 3]| {
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        assert!(distance(averages[0], averages[2]) > 16.0);
        assert!(distance(averages[1], averages[3]) > 10.0);
        assert!(distance(averages[0], averages[1]) > 6.0);
        assert!(distance(averages[2], averages[3]) > 6.0);
    }

    #[test]
    fn tiles_wrap_seamlessly() {
        let atlas = ground_atlas(7);
        let mut seam_diff = 0.0f64;
        let mut interior_diff = 0.0f64;
        let mut interior_samples = 0.0f64;
        for tile in 0..4u32 {
            let ox = (tile % 2) * DETAIL_TILE_SIZE;
            let oy = (tile / 2) * DETAIL_TILE_SIZE;
            for y in 0..DETAIL_TILE_SIZE {
                let last = channel(&atlas, ox + DETAIL_TILE_SIZE - 1, oy + y, 1) as f64;
                let first = channel(&atlas, ox, oy + y, 1) as f64;
                seam_diff += (last - first).abs();
                for x in 1..DETAIL_TILE_SIZE {
                    let a = channel(&atlas, ox + x - 1, oy + y, 1) as f64;
                    let b = channel(&atlas, ox + x, oy + y, 1) as f64;
                    interior_diff += (a - b).abs();
                    interior_samples += 1.0;
                }
            }
        }
        let seam_average = seam_diff / f64::from(4 * DETAIL_TILE_SIZE);
        let interior_average = interior_diff / interior_samples;
        assert!(
            seam_average <= interior_average * 3.0 + 2.0,
            "tile seam {seam_average:.2} is harsher than interior {interior_average:.2}"
        );
    }

    #[test]
    fn normal_atlas_is_derived_from_colour_luminance() {
        let color = ground_atlas(7);
        let normal = normal_atlas(&color, 2.5);
        assert_eq!(normal.width, DETAIL_ATLAS_SIZE);
        assert_eq!(normal.height, DETAIL_ATLAS_SIZE);
        assert!(normal.rgba.iter().skip(3).step_by(4).all(|a| *a == 255));

        // Not a flat normal map: blades/leaves produce real XZ slopes.
        let mut tilted = 0u32;
        let mut samples = 0u32;
        for y in 0..DETAIL_ATLAS_SIZE {
            for x in 0..DETAIL_ATLAS_SIZE {
                let r = channel(&normal, x, y, 0);
                let g = channel(&normal, x, y, 1);
                if (i32::from(r) - 128).abs() > 6 || (i32::from(g) - 128).abs() > 6 {
                    tilted += 1;
                }
                samples += 1;
            }
        }
        assert!(
            tilted > samples / 20,
            "only {tilted}/{samples} normal texels are tilted"
        );

        // A flat colour atlas must stay flat and point up (B = n.y = 1).
        let flat = GltfTextureImage {
            width: DETAIL_ATLAS_SIZE,
            height: DETAIL_ATLAS_SIZE,
            rgba: vec![128u8; (DETAIL_ATLAS_SIZE * DETAIL_ATLAS_SIZE * 4) as usize],
        };
        let flat_normal = normal_atlas(&flat, 2.5);
        for y in 0..DETAIL_ATLAS_SIZE {
            for x in 0..DETAIL_ATLAS_SIZE {
                assert!((i32::from(channel(&flat_normal, x, y, 0)) - 128).abs() <= 1);
                assert!((i32::from(channel(&flat_normal, x, y, 1)) - 128).abs() <= 1);
                assert_eq!(channel(&flat_normal, x, y, 2), 255);
            }
        }
        assert_eq!(normal_atlas(&color, 2.5), normal);
    }
}
