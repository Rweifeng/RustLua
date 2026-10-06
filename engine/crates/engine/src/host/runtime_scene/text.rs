//! Text rendering methods extracted from `runtime_scene.rs` (P0).
//!
//! Keeps the `impl RuntimeScene` block in the parent module small: all font
//! loading / caching, text measurement, layout, glyph prewarm and draw-call
//! emission live here.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::core::{
    camera::{CameraView, OrthoTerms, Viewport},
    entity::EntityId,
    font::FontVectorAtlas,
    hierarchy::GlobalTransform,
    layer::RenderLayer,
    math3d::Vec3,
    sprite_batch::{SpriteDrawBatch, ndc_from_screen},
    text::{TextAtlas, TextLayout, TextMesh, TextStyle},
    transform::Transform,
    ui::UiText,
    world_text::WorldText,
};

use super::{
    FONT_ATLAS_TEXTURE_PATH, FrameVectorTextVertex, OverlayVertex, SceneDraw, TextRenderMode,
    VectorGlyphSnapshot, VectorSegmentSnapshot, WorldSpriteVertex, font_texture_path,
    load_custom_font_atlas, load_custom_vector_atlas, load_project_font_atlas,
    load_project_vector_atlas, text_to_ndc,
};
use crate::host::frame::{ATLAS_TEXTURE_PATH, FontAtlasMap, FontHandle, VectorAtlasMap};

/// 一段文字 run 的渲染种类：决定绑哪张图集、走 SDF 还是 bitmap 分支。
/// SDF run 携带自己的 font key（自定义字体是 `font://...`，否则是默认图集哨兵）；
/// bitmap run 永远绑内置 5x7 图集。
pub(crate) struct TextRunKind<'a> {
    pub sdf: bool,
    pub bitmap: bool,
    pub font_key: &'a str,
}

impl super::RuntimeScene {
    pub(crate) fn ensure_vector_atlas(&mut self) {
        if self.vector_atlas.is_none()
            && let Some(root) = &self.project_root
        {
            self.vector_atlas = load_project_vector_atlas(Some(root));
        }
    }

    /// 获取默认或指定字体的矢量轮廓缓存；自定义字体失败返回 `None`，调用方
    /// 仍可选择回退 SDF。
    pub(crate) fn ensure_vector_font(
        &mut self,
        font_asset: &str,
    ) -> Option<Arc<Mutex<FontVectorAtlas>>> {
        if font_asset.is_empty() {
            self.ensure_vector_atlas();
            return self.vector_atlas.clone();
        }
        let key = font_texture_path(font_asset);
        if self.missing_vector_fonts.contains(&key) {
            return None;
        }
        if let Ok(map) = self.custom_vector_atlases.lock()
            && let Some(handle) = map.get(&key).cloned()
        {
            return Some(handle);
        }
        if let Some(atlas) = load_custom_vector_atlas(self.project_root.as_deref(), font_asset) {
            if let Ok(mut map) = self.custom_vector_atlases.lock() {
                map.entry(key.clone()).or_insert_with(|| atlas.clone());
            }
            return Some(atlas);
        } else {
            self.missing_vector_fonts.insert(key.clone());
        }
        None
    }

    /// 惰性加载项目字体图集：未注入共享图集时才按需加载（含 chars.txt 预热）。
    /// 编辑器场景视图/预览会先 `set_font_atlas` 注入已预热图集，故这里通常不会触发；
    /// 独立游戏/测试首次排版文本时加载一次，此后复用。
    pub(crate) fn ensure_font_atlas(&mut self) {
        if self.font_atlas.is_none()
            && let Some(root) = &self.project_root
        {
            self.font_atlas = load_project_font_atlas(Some(root));
        }
    }

    /// 注入跨运行时重建共享的自定义字体缓存（编辑器场景视图/预览用；独立运行
    /// 时运行时各自持有新鲜空表）。
    pub fn set_custom_font_atlases(&mut self, custom_font_atlases: FontAtlasMap) {
        self.custom_font_atlases = custom_font_atlases;
    }

    /// 注入跨运行时重建共享的自定义矢量轮廓缓存。
    pub fn set_custom_vector_atlases(&mut self, custom_vector_atlases: VectorAtlasMap) {
        self.custom_vector_atlases = custom_vector_atlases;
    }

    /// 取场景 Text 指定字体的共享图集句柄；未加载则按需从项目根解码（缺文件/
    /// 不可读返回 None，调用方回退默认字体）。加载失败仅对同一路径 warn 一次。
    pub(crate) fn ensure_custom_font(&mut self, asset: &str) -> Option<FontHandle> {
        let key = font_texture_path(asset);
        if let Ok(map) = self.custom_font_atlases.lock()
            && let Some(handle) = map.get(&key).cloned()
        {
            return Some(handle);
        }
        // 已知缺失：直接回退默认，不再每帧尝试读盘。
        if self.missing_fonts.contains(asset) {
            return None;
        }
        let root = self.project_root.clone();
        let handle = match load_custom_font_atlas(root.as_deref(), asset) {
            Some(handle) => handle,
            None => {
                if self.missing_fonts.insert(asset.to_owned()) {
                    tracing::warn!(
                        target: "render",
                        path = %asset,
                        "[render] custom font load failed; text falls back to default font"
                    );
                }
                return None;
            }
        };
        if let Ok(mut map) = self.custom_font_atlases.lock() {
            let handle = map.entry(key).or_insert_with(|| handle.clone()).clone();
            Some(handle)
        } else {
            None
        }
    }

    /// 度量一段场景文本在给定 scale 下的网格尺寸与字形像素范围
    /// `(size, pixel_bounds)`（编辑器 UI 包围框用）。`size` 含
    /// ascender/descender 空白（锚点对齐用），`pixel_bounds` 是可见字形的
    /// 紧致范围（包围框贴合文字用）。与渲染同路径：SDF 字体存在时用真实
    /// 字体，否则用内置 5x7 位图。
    pub fn measure_ui_text(&mut self, text: &str, scale: f32) -> ([f32; 2], [f32; 4]) {
        self.ensure_font_atlas();
        let style = TextStyle {
            scale,
            ..Default::default()
        };
        if let Some(font) = &self.font_atlas {
            let mut font = font.lock().expect("font atlas lock");
            TextLayout::build(text, [0.0, 0.0], style, &mut font)
                .map(|mesh| (mesh.size, mesh.pixel_bounds()))
                .unwrap_or(([0.0, 0.0], [0.0; 4]))
        } else {
            let atlas = TextAtlas::built_in();
            TextLayout::build_bitmap(text, [0.0, 0.0], style, &atlas)
                .map(|mesh| (mesh.size, mesh.pixel_bounds()))
                .unwrap_or(([0.0, 0.0], [0.0; 4]))
        }
    }

    /// 决定一段文本的首选渲染路径；矢量图集不可用时渲染层仍会回退 SDF。
    pub(crate) fn should_render_vector(
        &self,
        font_asset: Option<&str>,
        scale: f32,
        chars: usize,
    ) -> bool {
        match self.text_render_mode {
            TextRenderMode::Vector => true,
            TextRenderMode::Sdf => false,
            TextRenderMode::Auto => {
                font_asset.is_none_or(str::is_empty) && scale >= 2.0 && chars <= 32
            }
        }
    }

    /// Measures scene-node UI text with the same backend choice as rendering.
    /// `auto` keeps the default-font heuristic; forced modes apply to every text.
    pub fn measure_scene_ui_text(&mut self, text: &UiText) -> ([f32; 2], [f32; 4]) {
        let scale = text.effective_scale();
        let use_vector =
            self.should_render_vector(Some(text.font.as_str()), scale, text.text.chars().count());
        if use_vector
            && let Some(measured) = self.measure_vector_text(&text.text, scale, &text.font)
        {
            return measured;
        }
        self.measure_ui_text(&text.text, scale)
    }

    pub(crate) fn measure_vector_text(
        &mut self,
        text: &str,
        scale: f32,
        font_asset: &str,
    ) -> Option<([f32; 2], [f32; 4])> {
        let atlas = self.ensure_vector_font(font_asset)?;
        let style = TextStyle {
            scale,
            color: [1.0, 1.0, 1.0, 1.0],
            ..Default::default()
        };
        let mut atlas = atlas.lock().expect("vector atlas lock");
        let layout = TextLayout::build_vector(text, [0.0, 0.0], style, &mut atlas).ok()?;
        if layout.is_empty() {
            return None;
        }
        let mut min = [f32::INFINITY; 2];
        let mut max = [f32::NEG_INFINITY; 2];
        for vertex in &layout {
            min[0] = min[0].min(vertex.position[0]);
            min[1] = min[1].min(vertex.position[1]);
            max[0] = max[0].max(vertex.position[0]);
            max[1] = max[1].max(vertex.position[1]);
        }
        if !min.iter().all(|value| value.is_finite()) {
            return None;
        }
        Some((
            [max[0] - min[0], max[1] - min[1]],
            [min[0], min[1], max[0], max[1]],
        ))
    }

    /// 世界空间文字（WP4）：把实体的世界位置投影到屏幕后，走与屏幕文字**完全相同**的
    /// 字形管线（含 vector / SDF 两种模式与自定义字体）。
    ///
    /// 绘制层：精灵之上、屏幕 UI 之下，不参与世界深度。字形预热必须在构建任何字形网格
    /// **之前**完成，理由与 `push_scene_ui` 中的说明相同（图集增长会让先构建的网格 UV 失效）。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_world_text(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        world_vertices: &mut Vec<WorldSpriteVertex>,
        draws: &mut Vec<SceneDraw>,
        world_draws: &mut Vec<SpriteDrawBatch>,
        vector_vertices: &mut Vec<FrameVectorTextVertex>,
        vector_glyphs: &mut Vec<VectorGlyphSnapshot>,
        vector_segments: &mut Vec<VectorSegmentSnapshot>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<()> {
        // WP2：世界文字锚在**世界**位置上——继承开启时读传播结果，关闭时读局部值。
        // 两条路径都是 `for_each2`，因此不额外多做一次 `get`。
        let inherit = self.spatial_active;
        let entries = self.script.with_world(|world| {
            let mut entries = Vec::new();
            if inherit {
                world.for_each2::<GlobalTransform, WorldText>(|id, global, text| {
                    if let Some(plane) = global.plane() {
                        entries.push((id, plane, text.clone()));
                    }
                });
            } else {
                world.for_each2::<Transform, WorldText>(|id, transform, text| {
                    entries.push((id, *transform, text.clone()));
                });
            }
            entries
        });
        if entries.is_empty() {
            return Ok(());
        }
        let prewarm = entries
            .iter()
            .map(|(id, _, text)| (*id, text.text.clone()))
            .collect::<Vec<_>>();
        self.prewarm_frame_glyphs(&prewarm);

        let viewport = Viewport::new(width.max(1) as f32, height.max(1) as f32)?;
        // 预算项与世界通道要用的**逆矩阵**都在循环外算一次：此前每个世界文字实体各算一次
        // `view_projection` + `Mat4::inverse()`，实体一多就是每帧成百上千次 4×4 求逆。
        let Ok(camera) = OrthoTerms::new(self.camera, viewport) else {
            return Ok(());
        };
        let world_inverse = if self.world_sprite_depth {
            CameraView::from_camera2d(self.camera, viewport)
                .ok()
                .and_then(CameraView::inverse)
        } else {
            None
        };
        for (_, transform, text) in entries {
            let screen = camera.world_to_screen(transform.position);
            let mut origin = [screen.x + text.offset[0], screen.y + text.offset[1]];
            if text.centered {
                let (size, _) = self.measure_scene_ui_text(&text.text);
                origin[0] -= size[0] * 0.5;
            }
            let use_vector = self.should_render_vector(
                Some(text.text.font.as_str()),
                text.text.effective_scale(),
                text.text.text.chars().count(),
            );
            if use_vector
                && self.push_vector_text_run(
                    vector_vertices,
                    vector_glyphs,
                    vector_segments,
                    &text.text.text,
                    origin,
                    text.text.effective_scale(),
                    text.text.color,
                    Some(&text.text.font),
                    width,
                    height,
                )?
            {
                continue;
            }
            let (mesh, is_sdf, font_key) = self.layout_scene_text(&text.text, origin)?;
            if self.world_sprite_depth {
                // WP6：世界文字也进世界坐标深度通道。做法是把**像素空间**的字形顶点经
                // `view_projection` 的逆映射回世界坐标——因此屏幕上的观感与 overlay 路径
                // 逐个像素一致（字号仍是像素大小、随 zoom 换算成世界尺寸），但它有了世界
                // 坐标与 z，可以和精灵/瓦片一起参与深度测试。
                let Some(inverse) = world_inverse else {
                    continue;
                };
                let converted = mesh
                    .vertices
                    .iter()
                    .map(|vertex| {
                        let ndc = ndc_from_screen(vertex.position, viewport);
                        let Ok(world) = inverse.transform_point(Vec3::new(ndc[0], ndc[1], 0.0))
                        else {
                            return None;
                        };
                        Some(WorldSpriteVertex {
                            position: [world.x, world.y, 0.0],
                            uv: vertex.uv,
                            color: vertex.color,
                        })
                    })
                    .collect::<Option<Vec<_>>>();
                if let Some(converted) = converted {
                    let start = world_vertices.len();
                    world_vertices.extend(converted);
                    // 文字是**带 alpha 的字形边缘**：测深度但不写深度（写深度会在字形边缘
                    // 留下硬边并遮住后面的内容）。
                    world_draws.push(SpriteDrawBatch::translucent(
                        self.texture_id(&font_key),
                        0,
                        RenderLayer::World2D,
                        start,
                        world_vertices.len() - start,
                    ));
                }
                continue;
            }
            let is_bitmap = mesh.bitmap;
            let text_vertices = text_to_ndc(mesh, is_sdf, width, height);
            self.push_text_run(
                vertices,
                draws,
                text_vertices,
                TextRunKind {
                    sdf: is_sdf,
                    bitmap: is_bitmap,
                    font_key: &font_key,
                },
                RenderLayer::World2D,
            );
        }
        Ok(())
    }

    /// Layouts a string into a pixel-space text mesh using real font metrics.
    /// Uses the SDF font atlas when available, else the built-in bitmap atlas.
    /// Returns `(mesh, is_sdf)`.
    pub(crate) fn layout_text(
        &mut self,
        text: &str,
        position: [f32; 2],
        scale: f32,
        color: [f32; 3],
    ) -> anyhow::Result<(TextMesh, bool)> {
        self.ensure_font_atlas();
        let style = TextStyle {
            scale,
            color: [color[0], color[1], color[2], 1.0],
            ..Default::default()
        };
        if let Some(font) = &self.font_atlas {
            let mut font = font.lock().expect("font atlas lock");
            Ok(TextLayout::build(text, position, style, &mut font).map(|mesh| (mesh, true))?)
        } else {
            let atlas = TextAtlas::built_in();
            Ok(
                TextLayout::build_bitmap(text, position, style, &atlas)
                    .map(|mesh| (mesh, false))?,
            )
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_vector_text_run(
        &mut self,
        vertices: &mut Vec<FrameVectorTextVertex>,
        glyphs: &mut Vec<VectorGlyphSnapshot>,
        segments: &mut Vec<VectorSegmentSnapshot>,
        text: &str,
        position: [f32; 2],
        scale: f32,
        color: [f32; 3],
        font_asset: Option<&str>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<bool> {
        let atlas = self.ensure_vector_font(font_asset.unwrap_or_default());
        let Some(atlas) = atlas else {
            return Ok(false);
        };
        let style = TextStyle {
            scale,
            color: [color[0], color[1], color[2], 1.0],
            ..Default::default()
        };
        let mut atlas = atlas.lock().expect("vector atlas lock");
        let layout = TextLayout::build_vector(text, position, style, &mut atlas)?;
        if layout.is_empty() {
            return Ok(false);
        }
        let base_glyph = glyphs.len() as u32;
        let mut remap = HashMap::new();
        for vertex in layout {
            let mapped = if let Some(mapped) = remap.get(&vertex.glyph_id) {
                *mapped
            } else {
                let Some(glyph) = atlas.glyph_by_id(vertex.glyph_id) else {
                    // docs/60 §10.7 定性：**整段矢量文字被丢弃且不留痕**——症状与「文字没画出来、
                    // 日志一个字都没有」完全一样。同一类在 2D 叠加通道那一侧已改成「记账 + 告警」
                    // （`dropped_draws`），但本函数在上游、拿不到 `RenderFrameMetrics`，
                    // 所以这里只到 warn 这一层。
                    tracing::warn!(
                        target: "render",
                        glyph_id = vertex.glyph_id,
                        "[runtime] vector text run dropped: glyph missing from atlas"
                    );
                    return Ok(false);
                };
                let mapped = base_glyph + remap.len() as u32;
                let segment_start = segments.len() as u32;
                segments.extend(glyph.segments.iter().map(|segment| VectorSegmentSnapshot {
                    start: segment.start,
                    end: segment.end,
                    bounds: [
                        segment.start[0].min(segment.end[0]),
                        segment.start[1].min(segment.end[1]),
                        segment.start[0].max(segment.end[0]),
                        segment.start[1].max(segment.end[1]),
                    ],
                }));
                glyphs.push(VectorGlyphSnapshot {
                    segment_start,
                    segment_count: segments.len() as u32 - segment_start,
                    _padding: [0; 2],
                    bounds: glyph.bounds,
                });
                remap.insert(vertex.glyph_id, mapped);
                mapped
            };
            vertices.push(FrameVectorTextVertex {
                position: [
                    vertex.position[0] / width.max(1) as f32 * 2.0 - 1.0,
                    1.0 - vertex.position[1] / height.max(1) as f32 * 2.0,
                ],
                local: vertex.local,
                glyph_id: mapped,
                color: vertex.color,
            });
        }
        Ok(true)
    }

    /// 在构建任何文本网格之前，把本帧全部文本所需字形光栅化进各自字体图集，
    /// 使图集在本帧增长到最终尺寸。见 `push_scene_ui` 调用处注释。
    pub(crate) fn prewarm_frame_glyphs(&mut self, texts: &[(EntityId, UiText)]) {
        let mut default_chars: Vec<char> = Vec::new();
        let mut custom_chars: HashMap<&str, Vec<char>> = HashMap::new();
        for (_, text) in texts {
            if self.should_render_vector(
                Some(text.font.as_str()),
                text.effective_scale(),
                text.text.chars().count(),
            ) {
                continue;
            }
            if text.font.is_empty() {
                default_chars.extend(text.text.chars());
            } else {
                custom_chars
                    .entry(text.font.as_str())
                    .or_default()
                    .extend(text.text.chars());
            }
        }
        if !default_chars.is_empty() {
            self.ensure_font_atlas();
            if let Some(font) = &self.font_atlas {
                let mut font = font.lock().expect("font atlas lock");
                font.prewarm(default_chars.into_iter());
            }
        }
        for (asset, chars) in custom_chars {
            if let Some(handle) = self.ensure_custom_font(asset) {
                let mut atlas = handle.lock().expect("font atlas lock");
                atlas.prewarm(chars.into_iter());
            }
        }
    }

    /// 记录入本帧待上报列表；无自定义字体 / 字体缺失不可读时回退默认（默认 SDF 或
    /// 内置位图）。返回 `(mesh, is_sdf, font_key)`。
    pub(crate) fn layout_scene_text(
        &mut self,
        text: &UiText,
        position: [f32; 2],
    ) -> anyhow::Result<(TextMesh, bool, String)> {
        let effective_scale = text.effective_scale();
        if !text.font.is_empty()
            && let Some(handle) = self.ensure_custom_font(&text.font)
        {
            let mesh = {
                let mut atlas = handle.lock().expect("font atlas lock");
                let style = TextStyle {
                    scale: effective_scale,
                    color: [text.color[0], text.color[1], text.color[2], 1.0],
                    ..Default::default()
                };
                TextLayout::build(&text.text, position, style, &mut atlas)?
            };
            let key = font_texture_path(&text.font);
            self.frame_font_keys.push(key.clone());
            return Ok((mesh, true, key));
        }
        // 无自定义字体，或自定义字体缺失/不可读：回退默认字体路径。
        let (mesh, is_sdf) = self.layout_text(&text.text, position, effective_scale, text.color)?;
        let key = if is_sdf {
            FONT_ATLAS_TEXTURE_PATH.to_owned()
        } else {
            ATLAS_TEXTURE_PATH.to_owned()
        };
        Ok((mesh, is_sdf, key))
    }

    /// Appends a text run as a single SDF or bitmap draw.
    pub(crate) fn push_text_run(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        text_vertices: Vec<OverlayVertex>,
        kind: TextRunKind<'_>,
        render_layer: RenderLayer,
    ) {
        if text_vertices.is_empty() {
            return;
        }
        let start = vertices.len() as u32;
        let texture = if kind.sdf {
            self.texture_id(kind.font_key) + 1
        } else {
            self.texture_id(ATLAS_TEXTURE_PATH) + 1
        };
        vertices.extend(text_vertices);
        draws.push(SceneDraw {
            texture,
            start,
            count: vertices.len() as u32 - start,
            sdf: kind.sdf,
            bitmap: kind.bitmap,
            vector: false,
            render_layer,
        });
    }
}
