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
/// 四级 CSM 的级联接缝必须在着色器里真的做了边界混合，而不是只改了 CPU 层数。
#[test]
fn shadow_cascades_blend_at_their_edges() {
    for shader in [MESH3D, MESH3D_PBR] {
        assert!(
            shader.source.contains("fn cascade_edge"),
            "{} 缺少级联边界函数",
            shader.path
        );
        assert!(
            shader.source.contains("smoothstep(0.86, 1.0, edge)"),
            "{} 必须在每级最后 14% 边界带做 smoothstep 混合",
            shader.path
        );
        assert!(
            shader.source.matches("return mix(").count() >= 3,
            "{} 的 near→mid / mid→far / far→ultra 三处接缝都要混合",
            shader.path
        );
        for layer in 0..4 {
            assert!(
                shader
                    .source
                    .contains(&format!("sample_shadow(mid.xy, mid.z - 0.002, {layer})"))
                    || shader
                        .source
                        .contains(&format!("sample_shadow(far.xy, far.z - 0.002, {layer})"))
                    || shader.source.contains(&format!(
                        "sample_shadow(ultra.xy, ultra.z - 0.002, {layer})"
                    ))
                    || shader
                        .source
                        .contains(&format!("sample_shadow(near.xy, near.z - 0.002, {layer})")),
                "{} 缺少第 {layer} 层采样",
                shader.path
            );
        }
    }
}

/// 水面：世界空间波法线 + 菲涅尔；有 bed 的水面不再乘 UV 边缘淡出（否则 10 单位
/// 一块的水面会画出方格）。这两条是连续河道的着色器契约。
#[test]
fn terrain_cloud_shadows_use_world_space_and_camera_time() {
    let shader = &MESH3D_PBR;
    assert!(shader.source.contains("fn cloud_shadow"));
    assert!(shader.source.contains("camera.position.w"));
    assert!(shader.source.contains("let cloud_factor = 1.0 - cloud * camera.cloud.w"));
    assert!(
        shader.source.contains("* cloud_factor"),
        "云影必须在最终颜色上统一执行，而不是只压地形/水面"
    );
}

#[test]
fn water_shader_bends_normals_and_skips_tile_edges_for_bed() {
    let shader = MESH3D_PBR;
    assert!(
        shader.source.contains("var slope = vec2<f32>("),
        "{} 缺少世界空间水面斜率",
        shader.path
    );
    assert!(
        shader.source.contains("fn water_wave_slope"),
        "{} 缺少水面波斜率函数",
        shader.path
    );
    assert!(
        shader
            .source
            .contains("let distorted_uv = clamp(uv + slope * 0.004"),
        "{} 的河床深度必须随波折射",
        shader.path
    );
    assert!(
        shader.source.contains(".xz * ring"),
        "{} 缺少雨点环扰动",
        shader.path
    );
    assert!(
        shader.source.contains("-slope.x * 0.16"),
        "{} 必须把波斜率传给法线",
        shader.path
    );
    assert!(
        shader
            .source
            .contains("let fresnel = pow(1.0 - clamp(dot(n, view_dir)"),
        "{} 缺少水面菲涅尔",
        shader.path
    );
    assert!(
        shader
            .source
            .contains("select(smoothstep(0.0, 0.10, edge), 1.0, has_bed)"),
        "{} 的有 bed 水面不得再乘 UV 边缘淡出",
        shader.path
    );
    assert!(
        shader
            .source
            .contains("textureSampleLevel(normal_map, albedo_sampler, distorted_uv, 0.0)"),
        "{} 的水面必须能读逐像素河床深度",
        shader.path
    );
    assert!(
        shader.source.contains("material.surface.w < -0.5"),
        "{} 必须用 detail_scale=-1 识别水面深度图",
        shader.path
    );
    // P3-R49：水面天空反射（地平线/天顶渐变 + 太阳盘）。
    assert!(
        shader
            .source
            .contains("let reflected = safe_normalize(reflect(-view_dir, n))"),
        "{} 缺少水面反射方向",
        shader.path
    );
    assert!(
        shader
            .source
            .contains("sky_with_clouds + vec3<f32>(1.0, 0.82, 0.58) * sun_reflection"),
        "{} 缺少水面天空/太阳反射",
        shader.path
    );
}

/// 地形贴图 splat：世界坐标采样内嵌 2x2 图集、按顶点权重混合、走 mip，
/// 并保留原来的纯色路径（`surface.w == 0`）作为逐字节不变的回退。
#[test]
fn terrain_detail_atlas_is_sampled_in_world_space() {
    let shader = MESH3D_PBR;
    assert!(
        shader.source.contains("fn terrain_atlas_sample"),
        "{} 缺少地形图集采样函数",
        shader.path
    );
    assert!(
        shader.source.contains("textureSampleLevel(albedo"),
        "{} 的地形图集必须显式选 mip",
        shader.path
    );
    assert!(
        shader.source.contains("material.surface.w > 0.001"),
        "{} 必须用 surface.w 作为贴图开关/瓦片米数",
        shader.path
    );
    assert!(
        shader.source.contains("world_position.xz / tile_scale"),
        "{} 必须按世界坐标而不是高度/坡度 UV 采样",
        shader.path
    );
    for tile in [
        "vec2<f32>(0.0, 0.0)",
        "vec2<f32>(1.0, 0.0)",
        "vec2<f32>(0.0, 1.0)",
        "vec2<f32>(1.0, 1.0)",
    ] {
        assert!(
            shader.source.contains(tile),
            "{} 缺少图集瓦片 {tile}",
            shader.path
        );
    }
    assert!(
        shader
            .source
            .contains("let grass = vec3<f32>(0.20, 0.31, 0.15);"),
        "{} 的无贴图回退色不能丢",
        shader.path
    );
    // 地形法线图集：世界空间混合；通用切线基法线路径必须跳过地形。
    assert!(
        shader.source.contains("fn terrain_normal_atlas"),
        "{} 缺少地形法线图集采样",
        shader.path
    );
    assert!(
        shader.source.contains("terrain_normal.x * 2.0"),
        "{} 必须把地形法线混进 PBR n",
        shader.path
    );
    assert!(
        shader
            .source
            .contains("material.animation.x < 0.5 && material.params.z > 0.5"),
        "{} 的通用法线贴图路径必须跳过地形与水",
        shader.path
    );
    // P3-R48：逐像素 splat 走 occlusion 槽位，地形不再走通用 AO。
    assert!(
        shader
            .source
            .contains("textureSampleLevel(occlusion_map, albedo_sampler, world_uv, 0.0)"),
        "{} 缺少逐像素 splat 采样",
        shader.path
    );
    assert!(
        shader.source.contains("material.animation.y > 0.5"),
        "{} 必须用 animation.y 识别地形 splat 尺寸",
        shader.path
    );
    // P3-R60：近处才跑多步视差/crevice AO。
    assert!(
        shader.source.contains("let near_detail = lod < 1.0"),
        "{} 缺少地形细节 LOD 门",
        shader.path
    );
    assert!(
        shader.source.contains("if (near_detail)"),
        "{} 必须按 LOD 跳过视差/AO",
        shader.path
    );
    assert!(
        shader.source.contains("let curvature = hc * 4.0 - hl - hr - hu - hd"),
        "{} 缺少地形高度曲率",
        shader.path
    );
    // P3-R71：大尺度地形色斑。
    assert!(
        shader.source.contains("let macro_a = value_noise(world_position.xz * 0.008)"),
        "{} 缺少地形 macro 色斑",
        shader.path
    );
    assert!(
        shader.source.contains("bare * vec3<f32>(macro_tint * 1.02"),
        "{} 缺少地形 macro 混色",
        shader.path
    );
    assert!(
        shader.source.contains("camera.surface.x * 0.75"),
        "{} 缺少地形积雪覆盖",
        shader.path
    );
    assert!(
        shader.source.contains("camera.surface.x * 0.7"),
        "{} 缺少积雪法线压平",
        shader.path
    );
    assert!(
        shader.source.contains("cover_wet * 0.35"),
        "{} 缺少湿润粗糙度",
        shader.path
    );
    assert!(shader.source.contains("material.flags.w > 1.5"), "{} 缺少冰面标记", shader.path);
    assert!(shader.source.contains("P3-R125"), "{} 缺少冰裂纹", shader.path);
    assert!(shader.source.contains("P3-R135"), "{} 缺少水面涟漪", shader.path);
    assert!(shader.source.contains("P3-R136"), "{} 缺少涟漪法线扰动", shader.path);
    assert!(shader.source.contains("camera.ripple2"), "{} 缺少多涟漪槽", shader.path);
    // P3-R70：水面反射云层。
    assert!(
        shader.source.contains("let cloud_p = camera.position.xz + reflected.xz * cloud_t"),
        "{} 缺少水面云反射平面",
        shader.path
    );
    assert!(
        shader.source.contains("let sky_with_clouds = mix(sky, cloud_color"),
        "{} 缺少水面云反射混合",
        shader.path
    );
    // P3-R68：太阳方向的地形微阴影。
    assert!(
        shader.source.contains("var terrain_shadow = 0.0"),
        "{} 缺少地形微阴影变量",
        shader.path
    );
    assert!(
        shader.source.contains("for (var i: i32 = 1; i <= 5; i = i + 1)"),
        "{} 缺少 5 步地形阴影",
        shader.path
    );
    assert!(
        shader.source.contains("let ray_slope = clamp(0.55 * sun_height / horizontal"),
        "{} 缺少太阳高度阴影斜率",
        shader.path
    );
    assert!(
        shader.source.contains("(1.0 - terrain_shadow * 0.55)"),
        "{} 缺少微阴影压直接光",
        shader.path
    );
}

#[test]
fn clip_plane_caps_backfaces_near_the_cut() {
    let shader = &MESH3D_PBR;
    assert!(shader.source.contains("let clip_distance = dot(world_position"));
    assert!(shader.source.contains("!front_facing && clip_distance > -0.08"));
    assert!(shader.source.contains("return vec4<f32>(section, 1.0)"));
}

/// 点光软阴影必须与方向光共用 `light.params.y`，并真的做 3×3 比较采样。
#[test]
fn point_shadow_pcf_reuses_the_soft_shadow_flag() {    for shader in [MESH3D, MESH3D_PBR] {
        assert!(
            shader.source.contains("fn sample_point_shadow"),
            "{} 缺少点光采样函数",
            shader.path
        );
        assert!(
            shader.source.contains("cameras: array<mat4x4<f32>, 48>")
                && shader.source.contains("params: array<vec4<f32>, 8>"),
            "{} 必须给前八盏点光各留 6 层（48 矩阵 + 8 组参数）",
            shader.path
        );
        assert!(
            shader.source.contains("fn point_shadow_slot"),
            "{} 必须按光源位置分配点光阴影槽位",
            shader.path
        );
        assert!(
            shader
                .source
                .contains("var point_shadow_cube: texture_depth_cube_array"),
            "{} 的点光阴影必须走 cube array，靠硬件跨面过滤",
            shader.path
        );
        assert!(
            shader.source.contains("sum / 25.0"),
            "{} 的点光 PCSS 必须是 5×5 平均",
            shader.path
        );
        assert!(
            shader.source.contains("light.params.y < 0.5"),
            "{} 的点光软阴影必须复用 Light.params.y 开关",
            shader.path
        );
        assert!(
            shader
                .source
                .contains("return sample_point_shadow(delta, i32(slot), depth_ref)"),
            "{} 的 cube 方向必须交给点光 PCF 采样",
            shader.path
        );
    }
}

/// P3-R18：方向光软阴影必须是 PCSS-lite（遮挡体搜索 + 接触硬化半径 + 5×5 PCF），
/// 硬阴影仍走一次硬件比较；点光的 3×3 PCF 不被改写。
#[test]
fn directional_pcss_searches_blockers_before_filtering() {
    for shader in [MESH3D, MESH3D_PBR] {
        assert!(
            shader.source.contains("fn sample_shadow"),
            "{} 缺少方向光采样函数",
            shader.path
        );
        assert!(
            shader.source.contains("average_blocker")
                && shader.source.contains("blocker_sum")
                && shader.source.contains("blocker_count"),
            "{} 的 PCSS 必须先求遮挡体平均深度",
            shader.path
        );
        assert!(
            shader
                .source
                .contains("clamp((shadow_depth - average_blocker) * 48.0, 1.0, 3.0)"),
            "{} 的 PCSS 半径必须随遮挡距离放大并夹在 1–3 纹素",
            shader.path
        );
        assert!(
            shader.source.contains("textureLoad(shadow_map"),
            "{} 的遮挡体搜索必须读原始深度而不是复用比较结果",
            shader.path
        );
        assert!(
            shader.source.contains("sum / 25.0"),
            "{} 的 PCSS 必须是 5×5 平均",
            shader.path
        );
        assert!(
            shader
                .source
                .contains("return textureSampleCompare(shadow_map, shadow_sampler, shadow_uv, layer, shadow_depth);"),
            "{} 的硬阴影仍必须是一次硬件比较",
            shader.path
        );
    }
}

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
