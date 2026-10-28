//! P3-R04-3：shader 变体矩阵——预处理结果与管线缓存。
//!
//! 离线部分断言 `mesh3d.wgsl` / `shadow.wgsl` 的 `//#if SKIN` 真的在编译期分叉；
//! GPU 部分在真实设备上构造 `Renderer`，证明四个基础变体（网格/阴影 × 静态/蒙皮）
//! 都能编译且每条只编译一次。

use super::*;
use crate::renderer::construct::{create_mesh_pipeline, variant_shader_module};
use crate::renderer::pipeline_cache::{
    ColorTarget, InstanceMode, PipelineCache, ShaderVariantKey, Shading, SkinMode,
};
use crate::shader::{MESH3D, MESH3D_PBR, SHADOW, ShaderSource};
use crate::shader_preprocess::{PreprocessOptions, preprocess};
use rustlua_assets::env_asset::{EnvironmentAsset, EnvironmentMip};
use rustlua_render::render_settings::{
    EnvironmentLighting, EnvironmentSettings, IblMode, RenderSettings, ToggleMode,
};

const KNOWN_FEATURES: &[&str] = &["SKIN", "IBL_DIFFUSE", "IBL_SPECULAR", "STORAGE_INSTANCES"];

fn expand_mode(
    shader: &ShaderSource,
    skin: SkinMode,
    ibl: IblMode,
    instance_mode: InstanceMode,
) -> crate::shader_preprocess::PreprocessedShader {
    let mut features = Vec::with_capacity(
        skin.features().len() + ibl.features().len() + instance_mode.features().len(),
    );
    features.extend_from_slice(skin.features());
    features.extend_from_slice(ibl.features());
    features.extend_from_slice(instance_mode.features());
    preprocess(
        shader.path,
        shader.source,
        PreprocessOptions {
            features: &features,
            known_features: KNOWN_FEATURES,
            ..Default::default()
        },
    )
    .unwrap_or_else(|error| panic!("{} 预处理失败：{error}", shader.path))
}

/// 有存储缓冲（原生 / WebGPU）时的展开结果；离线断言默认走这条。
fn expand(
    shader: &ShaderSource,
    skin: SkinMode,
    ibl: IblMode,
) -> crate::shader_preprocess::PreprocessedShader {
    expand_mode(shader, skin, ibl, InstanceMode::Storage)
}

fn expand_skin(
    shader: &ShaderSource,
    skin: SkinMode,
) -> crate::shader_preprocess::PreprocessedShader {
    expand(shader, skin, IblMode::Off)
}

/// 网格：静态变体不再读关节调色板，蒙皮变体保留完整权重求和；两者都保留实例矩阵与光照。
#[test]
fn mesh_variants_differ_only_in_skinning() {
    let static_source = expand_skin(&MESH3D, SkinMode::Off);
    let skinned_source = expand_skin(&MESH3D, SkinMode::On);
    for source in [static_source.source(), skinned_source.source()] {
        assert!(!source.contains("//#"), "指令行必须被预处理掉");
        assert!(source.contains("instance_models[instance_index]"));
        assert!(source.contains("fn fs_main"));
        assert!(
            !source.contains("material.surface.z"),
            "蒙皮开关已移到变体轴，运行时分支不应残留"
        );
    }
    assert!(
        !static_source.source().contains("joint_palette["),
        "静态变体不应索引关节调色板"
    );
    for joint in ["x", "y", "z", "w"] {
        assert!(
            skinned_source
                .source()
                .contains(&format!("joint_palette[joint_indices.{joint}]")),
            "蒙皮变体缺少 joint_indices.{joint}"
        );
    }
    // 输出行仍能指回原文件：第一行是 `struct Camera`。
    assert_eq!(
        static_source
            .origin_of(1)
            .map(|line| (line.file.as_str(), line.line)),
        Some((MESH3D.path, 1))
    );
}

/// P3-R02：PBR 变体同样只按蒙皮分叉；法线矩阵、GGX 与相机位置都在。
#[test]
fn pbr_variants_differ_only_in_skinning() {
    let static_source = expand_skin(&MESH3D_PBR, SkinMode::Off);
    let skinned_source = expand_skin(&MESH3D_PBR, SkinMode::On);
    for source in [static_source.source(), skinned_source.source()] {
        assert!(!source.contains("//#"), "指令行必须被预处理掉");
        assert!(
            source.contains("@location(6) normal_m0")
                && source.contains("mat4x4<f32>(normal_m0, normal_m1, normal_m2, normal_m3)"),
            "PBR 顶点阶段必须从顶点缓冲槽 1 读逐实例法线矩阵"
        );
        assert!(source.contains("camera.position.xyz"));
        assert!(source.contains("distribution_ggx"));
        assert!(source.contains("visibility_smith_correlated"));
        assert!(source.contains("fresnel_schlick"));
    }
    assert!(!static_source.source().contains("joint_palette["));
    assert!(
        skinned_source
            .source()
            .contains("joint_palette[joint_indices.w]")
    );
    // 与旧着色器的关键区别：法线不再原样输出本地值。
    assert!(!static_source.source().contains("out.normal = normal;"));
}

/// P3-R03-2：`IBL_DIFFUSE` 变体用 SH9 替换常数环境光；Off 变体保留 R02 的逐字节公式。
#[test]
fn pbr_diffuse_ibl_replaces_constant_ambient() {
    let off = expand_skin(&MESH3D_PBR, SkinMode::Off);
    let diffuse = expand(&MESH3D_PBR, SkinMode::Off, IblMode::Diffuse);
    for source in [off.source(), diffuse.source()] {
        assert!(!source.contains("//#"), "指令行必须被预处理掉");
        assert!(
            source.contains("var<uniform> environment: Environment"),
            "PBR 布局含环境组，两份变体都必须声明它"
        );
    }
    assert!(
        !off.source().contains("fn sh_irradiance"),
        "Off 变体不应带上 SH 求值代码"
    );
    assert!(off.source().contains("AMBIENT_FACTOR * radiance"));
    assert!(diffuse.source().contains("fn sh_irradiance"));
    assert!(diffuse.source().contains("environment.sh[8]"));
    for constant in ["0.282095", "0.488603", "1.092548", "0.315392", "0.546274"] {
        assert!(
            diffuse.source().contains(constant),
            "SH 变体缺少基函数常数 {constant}"
        );
    }
    assert!(
        diffuse
            .source()
            .contains("rotate_y(n, -environment.params.y)")
    );
    assert!(
        diffuse.source().contains("(1.0 - metallic)"),
        "金属不能拿 SH 漫反射"
    );
    assert!(
        !diffuse.source().contains("AMBIENT_FACTOR * radiance"),
        "IBL 变体不应再叠常数环境光"
    );
}

/// P3-R03-3：`IBL_SPECULAR` 变体采样预过滤 cubemap；`Specular` 不叠 SH 或常数环境，
/// `Full` 同时含两段；`Off` 不含镜面代码。
#[test]
fn pbr_specular_ibl_samples_prefiltered_cube() {
    let off = expand_skin(&MESH3D_PBR, SkinMode::Off);
    let specular = expand(&MESH3D_PBR, SkinMode::Off, IblMode::Specular);
    let full = expand(&MESH3D_PBR, SkinMode::Off, IblMode::Full);
    for source in [specular.source(), full.source()] {
        assert!(source.contains("texture_cube<f32>"));
        assert!(source.contains("fn env_brdf_approx"));
        assert!(source.contains("-0.0275"), "EnvBRDFApprox 常数缺失");
        assert!(
            source.contains("textureSampleLevel(environment_cube, environment_sampler"),
            "镜面变体必须按显式 LOD 采样预过滤 cubemap"
        );
        assert!(
            source.contains("environment.params.w"),
            "mip 选择必须读取 uniform 里的最大 LOD"
        );
        assert!(source.contains("reflect(-view, normal)"));
        assert!(
            !source.contains("AMBIENT_FACTOR * radiance"),
            "开启 IBL 的变体不应再叠常数环境光"
        );
    }
    assert!(
        !off.source().contains("fn env_brdf_approx"),
        "Off 变体不应带镜面 IBL 代码"
    );
    assert!(
        !specular.source().contains("fn sh_irradiance"),
        "Specular-only 不应带 SH 漫反射"
    );
    assert!(full.source().contains("fn sh_irradiance"));
    assert!(full.source().contains("fn env_brdf_approx"));
    // 镜面-only 档没有漫反射项，间接光从 0 起算。
    assert!(specular.source().contains("var indirect = vec3<f32>(0.0);"));
}

/// 阴影：同一组轴，蒙皮分叉；P3-R02-4 起片元阶段只做 mask alpha 剔除（opaque 不采样）。
#[test]
fn shadow_variants_differ_only_in_skinning() {
    let static_source = expand_skin(&SHADOW, SkinMode::Off);
    let skinned_source = expand_skin(&SHADOW, SkinMode::On);
    assert!(!static_source.source().contains("joint_palette["));
    assert!(
        skinned_source
            .source()
            .contains("joint_palette[joint_indices.w]")
    );
    for source in [static_source.source(), skinned_source.source()] {
        assert!(source.contains("@vertex"));
        assert!(source.contains("@fragment fn fs_main"));
        assert!(source.contains("flags.z"));
        assert!(source.contains("instance_models[instance_index]"));
    }
}

/// WebGL2 路径：`InstanceMode::Uniform` 把逐实例模型矩阵换成 `@binding(2)` 的 64 字节 uniform，
/// 三份 3D 源都不再声明存储缓冲；`Storage` 展开则相反。两条分支互斥，指令行必须被吃掉。
#[test]
fn uniform_instance_mode_replaces_storage_buffer() {
    for shader in [&MESH3D, &MESH3D_PBR, &SHADOW] {
        let uniform = expand_mode(shader, SkinMode::Off, IblMode::Off, InstanceMode::Uniform);
        let source = uniform.source();
        assert!(
            !source.contains("//#"),
            "{}：指令行必须被预处理掉",
            shader.path
        );
        assert!(
            source.contains("@group(1) @binding(2) var<uniform> instance_model: mat4x4<f32>;"),
            "{}：无存储档必须声明 uniform 模型矩阵",
            shader.path
        );
        assert!(
            source.contains("let model = instance_model;"),
            "{}：顶点阶段必须读 uniform",
            shader.path
        );
        assert!(
            !source.contains("instance_models"),
            "{}：无存储档不得残留存储缓冲标识符",
            shader.path
        );
        assert!(
            !source.contains("var<storage"),
            "{}：WebGL2 没有存储缓冲，任何 var<storage> 都会让管线创建失败",
            shader.path
        );

        let storage = expand_mode(shader, SkinMode::Off, IblMode::Off, InstanceMode::Storage);
        assert!(
            storage.source().contains(
                "@group(1) @binding(2) var<storage, read> instance_models: array<mat4x4<f32>>;"
            ),
            "{}：有存储档必须保留存储缓冲声明",
            shader.path
        );
        assert!(
            !storage.source().contains("var<uniform> instance_model"),
            "{}：有存储档不得同时声明 uniform 模型矩阵",
            shader.path
        );
    }
}

/// 无存储档的展开结果必须通过 naga 的**完整校验**（语法 + 类型/语义）。
///
/// 原生测试设备都有存储缓冲，`uniform` 分支在 GPU 上跑不到；这条是它的离线证据：
/// 绑定声明与顶点阶段的读取类型对得上，WebGL2 建管线时不会因为源本身不合法而失败。
#[test]
fn uniform_instance_variant_passes_naga_validation() {
    for shader in [&MESH3D, &MESH3D_PBR, &SHADOW] {
        for skin in [SkinMode::Off, SkinMode::On] {
            let preprocessed = expand_mode(shader, skin, IblMode::Off, InstanceMode::Uniform);
            crate::shader_reload::validate_module(shader.path, preprocessed.source())
                .unwrap_or_else(|error| panic!("{}（{skin:?}）：{error}", shader.path));
        }
    }
}

/// 变体键与缓存的行为（不依赖 GPU）：同键复用、不同键分开、超限报错不淘汰。
#[test]
fn variant_keys_and_cache_are_consistent() {
    let keys = [
        ShaderVariantKey::mesh(Shading::Legacy, SkinMode::Off, ColorTarget::Ldr),
        ShaderVariantKey::mesh(Shading::Legacy, SkinMode::On, ColorTarget::Ldr),
        ShaderVariantKey::mesh(Shading::Legacy, SkinMode::Off, ColorTarget::Hdr),
        ShaderVariantKey::mesh(Shading::Pbr, SkinMode::Off, ColorTarget::Ldr),
        ShaderVariantKey::mesh(Shading::Pbr, SkinMode::On, ColorTarget::Hdr),
        ShaderVariantKey::mesh(Shading::Pbr, SkinMode::Off, ColorTarget::Ldr)
            .with_ibl(IblMode::Diffuse),
        ShaderVariantKey::mesh(Shading::Pbr, SkinMode::Off, ColorTarget::Ldr)
            .with_ibl(IblMode::Full),
        ShaderVariantKey::shadow(SkinMode::Off),
        ShaderVariantKey::shadow(SkinMode::On),
    ];
    for (index, key) in keys.iter().enumerate() {
        for other in &keys[index + 1..] {
            assert_ne!(key, other);
        }
    }
    let mut cache: PipelineCache<u8> = PipelineCache::with_limits(2, 3);
    for (index, key) in keys.iter().take(2).enumerate() {
        cache
            .get_or_create(*key, || Ok::<_, ()>(index as u8))
            .expect("family 上限 2 内");
    }
    assert!(
        cache.get_or_create(keys[2], || Ok::<_, ()>(9)).is_err(),
        "第 3 个网格变体必须超限"
    );
    assert_eq!(cache.len(), 2, "超限不淘汰既有变体");
}

/// GPU：构造期编译整个基础变体矩阵；统计证明没有重复管线。
#[test]
fn variant_matrix_compiles_once_per_key() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let Some(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()
    else {
        return gpu_skipped("request_adapter");
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rustlua-variant-test-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let renderer =
        Renderer::new_offscreen(device, queue, None, false, false).expect("offscreen renderer");
    let stats = renderer.pipeline_cache_stats();
    assert!(
        stats.variants >= 4,
        "至少应有网格/阴影 × 静态/蒙皮四条基础变体，实际 {}",
        stats.variants
    );
    assert_eq!(
        stats.misses as usize, stats.variants,
        "每条变体只应编译一次（无重复管线）"
    );
    assert_eq!(stats.hits, 0, "构造期不应请求同一变体两次");
    for skin in [SkinMode::Off, SkinMode::On] {
        assert!(
            renderer
                .pipelines
                .get(&ShaderVariantKey::mesh(
                    Shading::Legacy,
                    skin,
                    ColorTarget::Ldr
                ))
                .is_some(),
            "缺少 LDR 网格变体：{skin:?}"
        );
        assert!(
            renderer
                .pipelines
                .get(&ShaderVariantKey::shadow(skin))
                .is_some(),
            "缺少阴影变体：{skin:?}"
        );
    }
}

/// P3-R02：PBR 变体**只在显式打开时**编译；重复设置不产生新编译。
#[test]
fn pbr_variants_compile_lazily_and_only_once() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let Some(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()
    else {
        return gpu_skipped("request_adapter");
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rustlua-pbr-variant-test-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let mut renderer =
        Renderer::new_offscreen(device, queue, None, false, false).expect("offscreen renderer");
    let baseline = renderer.pipeline_cache_stats();
    assert!(!renderer.pbr_pipelines_ready());
    // 默认（pbr=off）不得编译任何 PBR 变体：启动预算只花在旧路径上。
    for skin in [SkinMode::Off, SkinMode::On] {
        for target in [ColorTarget::Ldr, ColorTarget::Hdr] {
            assert!(
                renderer
                    .pipelines
                    .get(&ShaderVariantKey::mesh(Shading::Pbr, skin, target))
                    .is_none(),
                "PBR 变体不应在默认设置下编译：{skin:?}/{target:?}"
            );
        }
    }
    renderer.set_render_settings(RenderSettings::default().with_pbr(true));
    assert!(renderer.pbr_pipelines_ready());
    let enabled = renderer.pipeline_cache_stats();
    assert!(
        enabled.variants > baseline.variants,
        "打开 PBR 后变体数没有增加：before={} after={}",
        baseline.variants,
        enabled.variants
    );
    for skin in [SkinMode::Off, SkinMode::On] {
        assert!(
            renderer
                .pipelines
                .get(&ShaderVariantKey::mesh(
                    Shading::Pbr,
                    skin,
                    ColorTarget::Ldr
                ))
                .is_some(),
            "缺少 PBR LDR 网格变体：{skin:?}"
        );
    }
    // 再次设置同一份开关：命中既有变体，不再编译。
    renderer.set_render_settings(RenderSettings::default().with_pbr(true));
    assert_eq!(
        renderer.pipeline_cache_stats().misses,
        enabled.misses,
        "重复打开 PBR 不应重新编译变体"
    );
}

/// P3-R03-2 GPU：IBL 档位随环境出现而切到 Diffuse；带 group2 的 IBL 变体在真实设备上
/// 能编译——预处理、WGSL 声明与三组管线布局三者一致。
#[test]
fn ibl_diffuse_variant_compiles_with_environment_binding() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let Some(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()
    else {
        return gpu_skipped("request_adapter");
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rustlua-ibl-variant-test-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let mut renderer =
        Renderer::new_offscreen(device, queue, None, false, false).expect("offscreen renderer");
    renderer.set_render_settings(RenderSettings {
        pbr: true,
        ibl: ToggleMode::On,
        ..RenderSettings::default()
    });
    assert_eq!(
        renderer.ibl_mode(),
        IblMode::Off,
        "没有环境时不得凭空开 IBL"
    );
    let mut sh = [0.0_f32; 27];
    sh[0] = 1.0;
    renderer.set_environment(EnvironmentLighting::from_sh(sh));
    assert_eq!(renderer.ibl_mode(), IblMode::Diffuse);
    let module = variant_shader_module(
        &renderer.device,
        &MESH3D_PBR,
        SkinMode::Off,
        IblMode::Diffuse,
        renderer.instance_mode(),
        "rustlua-test-ibl-shader",
    );
    let pipeline = create_mesh_pipeline(
        &renderer.device,
        &renderer.pbr_pipeline_layout,
        &module,
        renderer.config.format,
        "rustlua-test-ibl-pipeline",
        true,
        rustlua_render::mesh3d::MaterialAlphaMode::Opaque,
        false,
    );
    drop(pipeline);
    renderer.clear_environment();
    assert_eq!(renderer.ibl_mode(), IblMode::Off);
}

/// 供测试用的常量 cubemap：每级 mip 的 6 个面都是同一个 `texel`。
fn constant_environment_asset(face_size: u32, mip_count: u32) -> EnvironmentAsset {
    let mips = (0..mip_count)
        .map(|level| {
            let size = face_size >> level;
            let texels = (size * size) as usize;
            EnvironmentMip::new(
                size,
                std::array::from_fn(|_| vec![[2.0, 1.0, 0.5, 1.0]; texels]),
            )
        })
        .collect();
    EnvironmentAsset {
        source_hash: 0x5eed,
        face_size,
        sh: [0.0; 27],
        mips,
    }
}

/// P3-R03-3 GPU：上传预过滤 cubemap 后档位切到 `Full`，带镜面采样的管线在真实设备上
/// 能编译；清除后回到 Off。坏资产必须返回错误且**不覆盖**当前环境。
#[test]
fn ibl_specular_variant_compiles_with_prefiltered_cube() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let Some(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
        }))
        .ok()
    else {
        return gpu_skipped("request_adapter");
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rustlua-ibl-specular-test-device"),
        required_features: wgpu::Features::empty(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request test device");
    let mut renderer =
        Renderer::new_offscreen(device, queue, None, false, false).expect("offscreen renderer");
    renderer.set_render_settings(RenderSettings {
        pbr: true,
        ibl: ToggleMode::On,
        ..RenderSettings::default()
    });
    let asset = constant_environment_asset(4, 3);
    renderer
        .set_environment_asset(
            &asset,
            EnvironmentSettings {
                intensity: 1.5,
                rotation: 0.3,
            },
        )
        .expect("upload environment cubemap");
    assert_eq!(renderer.environment_map_info(), Some((4, 3)));
    assert_eq!(
        renderer.ibl_mode(),
        IblMode::Full,
        "有 cubemap 且设备可绑定浮点纹理时必须解析到 Full"
    );
    let module = variant_shader_module(
        &renderer.device,
        &MESH3D_PBR,
        SkinMode::Off,
        IblMode::Full,
        renderer.instance_mode(),
        "rustlua-test-ibl-specular-shader",
    );
    let pipeline = create_mesh_pipeline(
        &renderer.device,
        &renderer.pbr_pipeline_layout,
        &module,
        renderer.config.format,
        "rustlua-test-ibl-specular-pipeline",
        true,
        rustlua_render::mesh3d::MaterialAlphaMode::Opaque,
        false,
    );
    drop(pipeline);

    // 坏资产（没有 mip）必须拒绝，且当前环境/档位原样保留。
    let invalid = EnvironmentAsset {
        mips: Vec::new(),
        ..asset.clone()
    };
    assert!(
        renderer
            .set_environment_asset(&invalid, EnvironmentSettings::default())
            .is_err()
    );
    assert_eq!(renderer.environment_map_info(), Some((4, 3)));
    assert_eq!(renderer.ibl_mode(), IblMode::Full);

    renderer.clear_environment();
    assert_eq!(renderer.environment_map_info(), None);
    assert_eq!(renderer.ibl_mode(), IblMode::Off);
}
