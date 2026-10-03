//! 工程字体图集的加载与首包预热（WP12 阶段 2 从 `runtime_scene.rs` 原样搬出）。
//!
//! 四个 `pub fn` 是 `host` 对外符号面的一部分：`mod.rs` 用 `pub use fonts::{...}` 把它们留在
//! `engine::host::runtime_scene::` 这条路径上，因此 `host/mod.rs` 的 `pub use` 与
//! `engine::host::load_project_font_atlas` 的解析结果都没变（守卫见
//! `engine/crates/engine/tests/host_pub_surface.rs` 与 `artifacts/baselines/engine-pub-surface.json`）。
//! `prewarm_font_atlas` 只被 `load_project_font_atlas` 调用，保持私有。

use crate::core::font::{FontAtlas, FontVectorAtlas, project_font};
use crate::host::frame::FontHandle;
use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
};

/// 加载项目字体并包装为可跨运行时共享的矢量轮廓缓存。
pub fn load_project_vector_atlas(
    project_root: Option<&Path>,
) -> Option<Arc<Mutex<FontVectorAtlas>>> {
    let root = project_root?;
    let primary = fs::read(root.join(project_font::primary_path(root))).ok()?;
    let primary = crate::core::pack::decode(&primary).ok()?;
    let fallback = project_font::fallback_path(root).and_then(|path| {
        let bytes = fs::read(root.join(path)).ok()?;
        crate::core::pack::decode(&bytes).ok()
    });
    FontVectorAtlas::load_with_fallback(&primary, fallback.as_deref())
        .ok()
        .map(|atlas| Arc::new(Mutex::new(atlas)))
}

/// 加载项目字体并包装为可跨运行时共享的图集（MSDF 字形按需生成，共享后只生成
/// 缺省路径是 NotoSansSC-VF.ttf，项目可通过 `font` 键切换字体；
/// 缺省字重是 Regular（400）。
pub fn load_project_font_atlas(project_root: Option<&Path>) -> Option<Arc<Mutex<FontAtlas>>> {
    let root = project_root?;
    let primary_path = {
        let path = root.join(project_font::primary_path(root));
        path.is_file().then_some(path)
    }?;
    let primary_bytes = fs::read(&primary_path).ok()?;
    let primary_bytes = crate::core::pack::decode(&primary_bytes).ok()?;
    // 可选后备字体（project.toml `font_fallback` 键），用于主字体缺失的字形。
    let fallback_bytes = project_font::fallback_path(root)
        .and_then(|font| {
            let path = root.join(font);
            path.is_file().then_some(path)
        })
        .and_then(|path| {
            let bytes = fs::read(&path).ok()?;
            crate::core::pack::decode(&bytes).ok()
        });
    let font_weight = project_font::weight(root);
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
    tracing::info!(
        target: "render",
        path = %primary_path.display(),
        weight = font_weight,
        "[render] font atlas ready"
    );
    // 首包预热：把打包时收集的字符清单（chars.txt）预生成进图集，首帧文字不卡。
    prewarm_font_atlas(root, &mut atlas);
    Some(Arc::new(Mutex::new(atlas)))
}

/// 从项目根 `chars.txt`（打包时由 assetc 生成）读取字符并预热字形。
/// MSDF 光栅化跨线程并行，debug 构建下显著加速（ui_demo 135 字从 ~2.3s 降到 ~0.3s）。
fn prewarm_font_atlas(root: &Path, atlas: &mut FontAtlas) {
    let Ok(bytes) = fs::read(root.join("chars.txt")) else {
        return;
    };
    let Ok(bytes) = crate::core::pack::decode(&bytes) else {
        return;
    };
    let text = String::from_utf8_lossy(&bytes);
    let count = atlas.prewarm(text.chars());
    if count > 0 {
        tracing::info!(target: "render", count, "[render] font prewarmed {count} chars");
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
    let bytes = fs::read(root.join(font_asset)).ok()?;
    let bytes = crate::core::pack::decode(&bytes).ok()?;
    let fallback = project_font::fallback_path(root).and_then(|path| {
        let bytes = fs::read(root.join(path)).ok()?;
        crate::core::pack::decode(&bytes).ok()
    });
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
    if !path.is_file() {
        return None;
    }
    let bytes = fs::read(&path).ok()?;
    let bytes = crate::core::pack::decode(&bytes).ok()?;
    // 与默认字体一致地附上项目后备字体，缺字时回退（如拉丁字体不含 CJK）。
    let fallback_bytes = project_font::fallback_path(root)
        .and_then(|font| {
            let path = root.join(font);
            path.is_file().then_some(path)
        })
        .and_then(|path| {
            let bytes = fs::read(&path).ok()?;
            crate::core::pack::decode(&bytes).ok()
        });
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
