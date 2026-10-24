//! 工程字体图集的加载与首包预热（WP12 阶段 2 从 `runtime_scene.rs` 原样搬出）。
//!
//! 四个 `pub fn` 是 `host` 对外符号面的一部分：`mod.rs` 用 `pub use fonts::{...}` 把它们留在
//! `engine::host::runtime_scene::` 这条路径上，因此 `host/mod.rs` 的 `pub use` 与
//! `engine::host::load_project_font_atlas` 的解析结果都没变（守卫见
//! `engine/crates/engine/tests/host_pub_surface.rs` 与 `baselines/engine-pub-surface.json`）。
//! `prewarm_font_atlas` 只被 `load_project_font_atlas` 调用，保持私有。

use rustlua_render::font::{FontAtlas, FontVectorAtlas, project_font};
use rustlua_render::frame::FontHandle;
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

/// 加载项目字体并包装为可跨运行时共享的矢量轮廓缓存。
pub fn load_project_vector_atlas(
    project_root: Option<&Path>,
) -> Option<Arc<Mutex<FontVectorAtlas>>> {
    let root = project_root?;
    let primary = read_project_bytes(&root.join(project_font::primary_path(root)))?;
    let fallback =
        project_font::fallback_path(root).and_then(|path| read_project_bytes(&root.join(path)));
    FontVectorAtlas::load_with_fallback(&primary, fallback.as_deref())
        .ok()
        .map(|atlas| Arc::new(Mutex::new(atlas)))
}

/// 工程文件是否存在：走 `vfs::is_file`（provider 优先；桌面无 provider 时就是 `Path::is_file`）。
fn project_file_exists(path: &Path) -> bool {
    rustlua_infra::vfs::is_file(path)
}

/// 读工程文件并解码：走 `vfs::read_resource`（**唯一资源加载通道**）。
///
/// 与 `rustlua-infra::project_config::read_text`、`rustlua-render::font::project_font::read_bytes`
/// 共用同一条通道：provider 优先、桌面回落 `std::fs`，再统一解码。Web/微信上工程随宿主资源表走，
/// `std::fs` 恒失败——没有这条通道，字体与配置都读不到。
fn read_project_bytes(path: &Path) -> Option<Vec<u8>> {
    rustlua_infra::vfs::read_resource(path).ok()
}

/// 预热分批粒度：够小到每批耗时/内存读数有分辨力，够大到日志条数可控（1558 字 ⇒ 7 条）。
const PREWARM_CHUNK_CHARS: usize = 256;

/// 阶段遥测的线性内存读数（docs/65 P0：300MB 峰值归因）。wasm 上取 64KB 页数×页大小；
/// 桌面返回 `None`——桌面的内存本来就能用调试器看，不给它的日志加伪读数。
fn stage_memory_bytes() -> Option<usize> {
    #[cfg(target_arch = "wasm32")]
    {
        Some(std::arch::wasm32::memory_size(0) * 65_536)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

/// 加载项目字体并包装为可跨运行时共享的图集（MSDF 字形按需生成，共享后只生成
/// 缺省路径是 NotoSansSC-VF.ttf，项目可通过 `font` 键切换字体；
/// 缺省字重是 Regular（400）。
pub fn load_project_font_atlas(project_root: Option<&Path>) -> Option<Arc<Mutex<FontAtlas>>> {
    let root = project_root?;
    // 首帧文字的全部成本都在这一个函数里，所以按阶段计时（读数进 `tracing`，wasm 侧经
    // `wasm_log` 落到控制台）：读盘/解包 → 建图集（解析字体）→ 预热（光栅化字符）。
    // 三段的相对大小决定了下一步该优化谁——而不是猜。
    let started = web_time::Instant::now();
    let primary_path = {
        let path = root.join(project_font::primary_path(root));
        project_file_exists(&path).then_some(path)
    }?;
    let primary_bytes = read_project_bytes(&primary_path)?;
    // 可选后备字体（project.toml `font_fallback` 键），用于主字体缺失的字形。
    let fallback_bytes = project_font::fallback_path(root)
        .and_then(|font| {
            let path = root.join(font);
            project_file_exists(&path).then_some(path)
        })
        .and_then(|path| read_project_bytes(&path));
    let font_weight = project_font::weight(root);
    let decode_ms = started.elapsed().as_secs_f64() * 1000.0;
    let build_started = web_time::Instant::now();
    let mut atlas = match FontAtlas::load_with_fallback_and_weight(
        &primary_bytes,
        fallback_bytes.as_deref(),
        font_weight,
    ) {
        Ok(atlas) => atlas,
        Err(error) => {
            tracing::warn!(target: "render", error = %error, "[render] font atlas load failed; text uses placeholders");
            return None;
        }
    };
    let build_ms = build_started.elapsed().as_secs_f64() * 1000.0;
    tracing::info!(
        target: "render",
        path = %primary_path.display(),
        weight = font_weight,
        decode_ms,
        build_ms,
        primary_bytes = primary_bytes.len(),
        fallback_bytes = fallback_bytes.as_ref().map_or(0, Vec::len),
        atlas_px = atlas.width() * atlas.height(),
        atlas_bytes = atlas.pixels().len(),
        mem_bytes = stage_memory_bytes().unwrap_or(0),
        "[render] font atlas ready"
    );
    // 首包预热：把打包时收集的字符清单（chars.txt）预生成进图集，首帧文字不卡。
    prewarm_font_atlas(root, &mut atlas);
    Some(Arc::new(Mutex::new(atlas)))
}

/// 从项目根 `chars.txt`（打包时由 assetc 生成）读取字符并预热字形。
/// MSDF 光栅化跨线程并行，debug 构建下显著加速（ui_demo 135 字从 ~2.3s 降到 ~0.3s）。
///
/// 分批（256 字/批）调用 `prewarm` 而不是整表一次：Phase A 的光栅化结果在批内驻留，
/// 分批只改变**驻留时长**与日志粒度，不改变字形内容与顺序（`prewarm` 按 chars 顺序
/// 去重、跳过已缓存、打包布局逐批接续）。docs/65 P0 的峰值归因要靠每批落一条
/// 耗时+线性内存读数，而不是事后猜"300MB 是不是字体"。
fn prewarm_font_atlas(root: &Path, atlas: &mut FontAtlas) {
    let Some(bytes) = read_project_bytes(&root.join("chars.txt")) else {
        return;
    };
    let text = String::from_utf8_lossy(&bytes);
    let chars: Vec<char> = text.chars().collect();
    let started = web_time::Instant::now();
    let mut count = 0usize;
    let mut chunks = 0usize;
    let mut prev_total_ms = 0.0f64;
    for chunk in chars.chunks(PREWARM_CHUNK_CHARS) {
        count += atlas.prewarm(chunk.iter().copied());
        chunks += 1;
        let total_ms = started.elapsed().as_secs_f64() * 1000.0;
        tracing::info!(
            target: "render",
            chunk = chunks,
            chunk_chars = chunk.len(),
            count,
            chunk_ms = total_ms - prev_total_ms,
            total_ms,
            atlas_px = atlas.width() * atlas.height(),
            atlas_bytes = atlas.pixels().len(),
            mem_bytes = stage_memory_bytes().unwrap_or(0),
            "[render] font prewarm chunk"
        );
        prev_total_ms = total_ms;
    }
    if count > 0 {
        tracing::info!(
            target: "render",
            count,
            chunks,
            elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
            "[render] font prewarmed {count} chars"
        );
    }
}

pub fn load_custom_vector_atlas(
    project_root: Option<&Path>,
    font_asset: &str,
) -> Option<Arc<Mutex<FontVectorAtlas>>> {
    let root = project_root?;
    if font_asset.trim().is_empty() {
        return None;
    }
    let bytes = read_project_bytes(&root.join(font_asset))?;
    let fallback =
        project_font::fallback_path(root).and_then(|path| read_project_bytes(&root.join(path)));
    FontVectorAtlas::load_with_fallback(&bytes, fallback.as_deref())
        .ok()
        .map(|atlas| Arc::new(Mutex::new(atlas)))
}

/// 读取 + pack 解码 + 项目 fallback 字体 + 项目字重。**不做** chars.txt 预热
/// （字形按需光栅化）。任何失败返回 `None` → 调用方回退默认字体，不中断渲染
/// （wasm / 发布包缺该字体时自动降级）。
pub fn load_custom_font_atlas(project_root: Option<&Path>, font_asset: &str) -> Option<FontHandle> {
    let root = project_root?;
    if font_asset.trim().is_empty() {
        return None;
    }
    let path = root.join(font_asset);
    if !project_file_exists(&path) {
        return None;
    }
    let bytes = read_project_bytes(&path)?;
    // 与默认字体一致地附上项目后备字体，缺字时回退（如拉丁字体不含 CJK）。
    let fallback_bytes = project_font::fallback_path(root)
        .and_then(|font| {
            let path = root.join(font);
            project_file_exists(&path).then_some(path)
        })
        .and_then(|path| read_project_bytes(&path));
    let font_weight = project_font::weight(root);
    let atlas = match FontAtlas::load_with_fallback_and_weight(
        &bytes,
        fallback_bytes.as_deref(),
        font_weight,
    ) {
        Ok(atlas) => atlas,
        Err(error) => {
            tracing::warn!(
                target: "render",
                error = %error,
                path = %font_asset,
                "[render] custom font atlas load failed; text falls back to default"
            );
            return None;
        }
    };
    tracing::info!(target: "render", path = %font_asset, "[render] custom font atlas ready");
    Some(Arc::new(Mutex::new(atlas)))
}
