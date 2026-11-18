//! 逐纹理采样器表（拆分批 2 / 9）。
//!
//! ## 这一段原本在 `construct.rs` 的哪里
//!
//! `new_async` 里 `M79 P1-3c` 那段（51 行、1 处 `device.create_sampler`，在循环里）。
//!
//! ## 为什么单独拆它
//!
//! `new_async` 按**行号**切不出干净的批次：`texture_bind_group_layout`(216) →
//! `pipeline_layout`(238) → `shader`(344) → `pipeline`(348) 是一整条依赖链，
//! 而 `camera_bind_group_layout`(243–343) 夹在中间，且被 997/1052/1195/1203 行用到
//! ——它现在不能动。所以"文本段落"整体搬迁会打断依赖，或者被迫连 incompatible 的一起搬。
//!
//! 这一段是**自包含**的：只吃 `device` 和两个静态方法（`Renderer::wgpu_filter_mode` /
//! `wgpu_address_mode`），产出一个 `Vec`，不前依赖后文、后文也不改它。
//! 它是按依赖关系切分而非按行号切分的第一个样板。
//!
//! ## 语义零变化的等价点
//!
//! 1. **第一格必须与全局 `sampler` 同参数**（Nearest + ClampToEdge）——那是"没在
//!    `resources.meta` 里登记过的贴图"走的路径，要逐字节维持原样。这条由
//!    `the_undocumented_texture_path_keeps_todays_pixels` 钉住，搬迁不能碰它。
//! 2. **循环顺序**：`Nearest → Linear` 外层、`ClampToEdge → Repeat → MirroredRepeat`
//!    内层。下标 = `filter * Wrap::COUNT + wrap` 依赖这个顺序，改了就错位。
//! 3. `debug_assert_eq!` 的数量校验**跟着搬过来**：它在 debug 构建里是一道真门，
//!    留在原处会失去意义（那时 `texture_samplers` 已经离开作用域）。
//! 4. 资源创建序列里这一项仍是 `create_sampler`（在循环体内），
//!    `tests/construct_creation_order.rs` 会验证它还在原来的位置上。

use wgpu::{Device, Sampler};

use crate::renderer::Renderer;

/// 建"滤镜 × 寻址"的全部组合（`TextureFilter::COUNT × TextureWrap::COUNT` 个）。
///
/// 下标口径：`filter * TextureWrap::COUNT + wrap`，与 `gpu_texture` 侧取表的口径一致。
pub(crate) fn texture_samplers(device: &Device) -> Vec<Sampler> {
    // M79 P1-3c：逐纹理采样器表（滤镜 × 寻址 的全组合，下标 = `filter * Wrap::COUNT + wrap`）。
    // **第一格必须与全局 `sampler` 同参数**（Nearest + ClampToEdge）——那是"没在
    // `resources.meta` 里登记过的贴图"走的路径，它要逐字节维持今天的样子，这条由
    // `the_undocumented_texture_path_keeps_todays_pixels` 钉住。
    let mut texture_samplers = Vec::new();
    for filter in [
        rustlua_assets::import_settings::TextureFilter::Nearest,
        rustlua_assets::import_settings::TextureFilter::Linear,
    ] {
        use rustlua_assets::import_settings::TextureWrap;
        for wrap in [
            TextureWrap::ClampToEdge,
            TextureWrap::Repeat,
            TextureWrap::MirroredRepeat,
        ] {
            texture_samplers.push(device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(&format!(
                    "rustlua-texture-sampler-{}-{}",
                    filter.token(),
                    wrap.token()
                )),
                mag_filter: Renderer::wgpu_filter_mode(filter),
                min_filter: Renderer::wgpu_filter_mode(filter),
                // P3-R96：线性材质采样开 8× 各向异性，地形/水面在掠射角更清晰；
                // wgpu 要求 anisotropy>1 时三档过滤都必须是 Linear。
                mipmap_filter: if filter == rustlua_assets::import_settings::TextureFilter::Linear {
                    wgpu::FilterMode::Linear
                } else {
                    wgpu::FilterMode::Nearest
                },
                anisotropy_clamp: if filter
                    == rustlua_assets::import_settings::TextureFilter::Linear
                {
                    8
                } else {
                    1
                },
                address_mode_u: Renderer::wgpu_address_mode(wrap),
                address_mode_v: Renderer::wgpu_address_mode(wrap),
                address_mode_w: Renderer::wgpu_address_mode(wrap),
                ..Default::default()
            }));
        }
    }
    debug_assert_eq!(
        texture_samplers.len(),
        rustlua_assets::import_settings::TextureFilter::COUNT
            * rustlua_assets::import_settings::TextureWrap::COUNT
    );
    texture_samplers
}
