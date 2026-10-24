//! WGSL shader sources embedded at compile time (renderer-only).
//!
//! P3-R04-1：源码放在 `engine/crates/rustlua-gpu/shaders/*.wgsl`，`include_str!` 在**编译期**嵌回二进制——运行
//! 时不读文件，打包/部署也不需要额外路径。搬运逐字节保真（只去掉了字符串字面量开头的空行），
//! 管线、绑定与像素行为不变。
//!
//! [`ShaderSource`] 给每份源一个稳定身份（名字 + 仓库内路径 + 64 位 FNV-1a），供后续变体
//! 缓存 / 热重载判断"这份源有没有变"；旧的 `*_SHADER` 常量保留为别名，调用点不用改。

/// 一份内嵌 WGSL 源的身份。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShaderSource {
    /// 稳定短名（变体键 / 日志 / Profiler 共用）。
    pub name: &'static str,
    /// 仓库内路径，相对仓库根（报错与文档用）。
    pub path: &'static str,
    /// 源文本的 64 位 FNV-1a，构建期算好。
    pub hash: u64,
    /// 源文本（`include_str!` 嵌入二进制）。
    pub source: &'static str,
}

impl ShaderSource {
    const fn new(name: &'static str, path: &'static str, source: &'static str) -> Self {
        Self {
            name,
            path,
            hash: fnv1a64(source.as_bytes()),
            source,
        }
    }
}

/// FNV-1a 64：只用于"源变没变"的身份，不是密码学哈希。
///
/// `pub(crate)`：P3-R05 的覆盖目录源用同一个函数算哈希，内置/覆盖两条路径的哈希口径必须一致，
/// 否则"同一内容不重复编译"会在切换来源时失效。
pub(crate) const fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut index = 0;
    while index < bytes.len() {
        hash ^= bytes[index] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        index += 1;
    }
    hash
}

pub const VECTOR_TEXT: ShaderSource = ShaderSource::new(
    "vector_text",
    "engine/crates/rustlua-gpu/shaders/vector_text.wgsl",
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/shaders/vector_text.wgsl"
    )),
);

pub const TEXTURED: ShaderSource = ShaderSource::new(
    "textured",
    "engine/crates/rustlua-gpu/shaders/textured.wgsl",
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/shaders/textured.wgsl"
    )),
);

/// WP6 世界 Sprite 通道：顶点是**世界坐标** `(x, y, z)`，投影由顶点着色器用 2D 相机的
/// `view_projection` 完成（`Camera2D::view_projection`，与 CPU 侧 `world_to_screen` 同源）。
/// 片元与 [`TEXTURED`] 的普通纹理分支一致（采样 × 顶点色），因此世界路径与 overlay
/// 路径在颜色上不会分叉；两者的差异只有"投影在哪算"和"是否参与深度测试"。
pub const WORLD_SPRITE: ShaderSource = ShaderSource::new(
    "world_sprite",
    "engine/crates/rustlua-gpu/shaders/world_sprite.wgsl",
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/shaders/world_sprite.wgsl"
    )),
);

pub const MESH3D: ShaderSource = ShaderSource::new(
    "mesh3d",
    "engine/crates/rustlua-gpu/shaders/mesh3d.wgsl",
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/shaders/mesh3d.wgsl")),
);

/// P3-R02：Cook-Torrance PBR 网格着色（`RenderSettings.pbr=off` 时不编译、不使用）。
///
/// 与 [`MESH3D`] 共用顶点输入、实例矩阵与阴影采样；差异只在片元：法线走 CPU 算好的
/// 逆转置矩阵、视线取相机世界位置、光照换成 GGX/Smith/Schlick。保留独立文件而不是
/// `//#if PBR`，是为了让回滚档的源与管线**一个字节都不变**。
pub const MESH3D_PBR: ShaderSource = ShaderSource::new(
    "mesh3d_pbr",
    "engine/crates/rustlua-gpu/shaders/mesh3d_pbr.wgsl",
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/shaders/mesh3d_pbr.wgsl"
    )),
);

pub const SHADOW: ShaderSource = ShaderSource::new(
    "shadow",
    "engine/crates/rustlua-gpu/shaders/shadow.wgsl",
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/shaders/shadow.wgsl")),
);

/// docs/54 §4.3：后处理通道的全屏 pass。
///
/// 顶点从 `@builtin(vertex_index)` 生成一个覆盖全屏的三角形（不需要顶点缓冲），片元采样场景
/// 颜色纹理后施加曝光、色调与暗角。`uv` 的 y 需要翻转：纹理坐标原点在左上，而 NDC 的 +y 朝上。
///
/// P3-R01：曝光增益是 `exposure * 2^exposure_ev`——LDR 模式没有 Tonemap pass，EV 必须在这里
/// 生效；HDR 模式由渲染器把 `exposure_ev` 置 0 再上传（曝光已归 Tonemap）。
pub const POST_PROCESS: ShaderSource = ShaderSource::new(
    "post_process",
    "engine/crates/rustlua-gpu/shaders/post_process.wgsl",
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/shaders/post_process.wgsl"
    )),
);

/// P3-R01：HDR 场景颜色 → LDR 的 Tonemap 全屏 pass。
///
/// 输入是**线性** `Rgba16Float`（可以 > 1），输出写 8 位 sRGB 目标（硬件负责线性→sRGB 编码）。
/// 曝光先于曲线：`linear * 2^exposure_ev`，再走 ACES（Narkowicz 拟合）或 Reinhard，最后夹到
/// `0..=1`。曲线公式必须与 [`rustlua_render::post_process::Tonemap::apply_rgb`] 逐行对应。
pub const TONEMAP: ShaderSource = ShaderSource::new(
    "tonemap",
    "engine/crates/rustlua-gpu/shaders/tonemap.wgsl",
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/shaders/tonemap.wgsl")),
);

/// 全部内嵌 shader（顺序稳定；新增 shader 必须同时进这里，测试会断言数量）。
pub const ALL_SHADERS: [&ShaderSource; 8] = [
    &VECTOR_TEXT,
    &TEXTURED,
    &WORLD_SPRITE,
    &MESH3D,
    &MESH3D_PBR,
    &SHADOW,
    &POST_PROCESS,
    &TONEMAP,
];

/// 按稳定短名查找。
#[must_use]
pub fn find(name: &str) -> Option<&'static ShaderSource> {
    ALL_SHADERS
        .iter()
        .copied()
        .find(|shader| shader.name == name)
}

pub const VECTOR_TEXT_SHADER: &str = VECTOR_TEXT.source;
pub const TEXTURED_SHADER: &str = TEXTURED.source;
pub const WORLD_SPRITE_SHADER: &str = WORLD_SPRITE.source;
pub const MESH3D_SHADER: &str = MESH3D.source;
pub const MESH3D_PBR_SHADER: &str = MESH3D_PBR.source;
pub const SHADOW_SHADER: &str = SHADOW.source;
pub const POST_PROCESS_SHADER: &str = POST_PROCESS.source;
pub const TONEMAP_SHADER: &str = TONEMAP.source;

#[cfg(test)]
mod tests {
    use super::*;

    /// 注册表必须覆盖全部内嵌 shader，名字/路径唯一，哈希与源一致。
    #[test]
    fn registry_is_unique_and_hashes_match_sources() {
        assert_eq!(ALL_SHADERS.len(), 8, "新增 shader 必须同步注册表与别名");
        for (index, shader) in ALL_SHADERS.iter().enumerate() {
            assert!(!shader.name.is_empty(), "{shader:?}");
            assert!(
                shader
                    .path
                    .starts_with("engine/crates/rustlua-gpu/shaders/")
                    && shader.path.ends_with(".wgsl"),
                "路径口径必须是仓库根相对：{}",
                shader.path
            );
            assert_eq!(
                shader.hash,
                fnv1a64(shader.source.as_bytes()),
                "{} 的哈希与源不一致",
                shader.name
            );
            assert!(
                shader.source.contains("@vertex"),
                "{} 缺少顶点入口",
                shader.name
            );
            for other in &ALL_SHADERS[index + 1..] {
                assert_ne!(shader.name, other.name, "稳定短名重复");
                assert_ne!(shader.path, other.path, "路径重复");
                assert_ne!(shader.hash, other.hash, "不同源的哈希不应相同");
            }
        }
    }

    /// 旧常量必须与注册表里的源逐字节相同（调用点继续用别名）。
    #[test]
    fn legacy_constants_match_the_registry() {
        for (constant, shader) in [
            (VECTOR_TEXT_SHADER, &VECTOR_TEXT),
            (TEXTURED_SHADER, &TEXTURED),
            (WORLD_SPRITE_SHADER, &WORLD_SPRITE),
            (MESH3D_SHADER, &MESH3D),
            (MESH3D_PBR_SHADER, &MESH3D_PBR),
            (SHADOW_SHADER, &SHADOW),
            (POST_PROCESS_SHADER, &POST_PROCESS),
            (TONEMAP_SHADER, &TONEMAP),
        ] {
            assert_eq!(constant, shader.source, "{} 的旧常量已漂移", shader.name);
        }
    }

    /// `find` 覆盖命中与未命中；未知名字返回 `None`（调用方据此报错，而不是静默用错源）。
    #[test]
    fn find_by_stable_name() {
        assert_eq!(
            find("mesh3d").map(|shader| shader.path),
            Some("engine/crates/rustlua-gpu/shaders/mesh3d.wgsl")
        );
        assert_eq!(
            find("tonemap").map(|shader| shader.hash),
            Some(TONEMAP.hash)
        );
        assert!(find("Mesh3D").is_none(), "短名区分大小写");
        assert!(find("").is_none());
    }

    /// 源文件真的被搬出 Rust 字符串：源里不应再有 WGSL 正文的返回类型声明之外的东西。
    /// 这里只钉住"文件化后内容非空且带入口"；像素行为由旧 golden 复核。
    #[test]
    fn sources_are_non_empty_and_end_with_newline() {
        for shader in ALL_SHADERS {
            assert!(
                shader.source.ends_with('\n'),
                "{} 应以换行结尾（diff 友好）",
                shader.path
            );
            assert!(
                !shader.source.starts_with('\n'),
                "{} 不应以空行开头（搬运时已去掉）",
                shader.path
            );
        }
    }
}
