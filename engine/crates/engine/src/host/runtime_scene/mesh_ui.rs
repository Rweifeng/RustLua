//! 屏幕 UI 与场景 UI：点击目标收集、UI 布局解析与 `push_ui_*` / `push_scene_ui` 顶点构建
//! （WP12 阶段 3b 从 `mod.rs` 原样搬出）。
//!
//! `compute_click_targets` 与 `text_hit_rect` 跟着进来：点击矩形与绘制矩形必须同源，
//! 分开两个模块就成了"同一事实两处算"（计划 §2.2 的那类病）。

use crate::core::{
    camera::{OrthoTerms, Viewport},
    entity::EntityId,
    layer::RenderLayer,
    sprite::Sprite,
    transform::Transform,
    ui::{
        DynamicUiImage, UiImage, UiText, nine_slice_cells, resolve_ui_rect, ui_text_effective_scale,
    },
    ui_events::{UiClickTarget, UiEvents},
    ui_layout::UiRect,
    ui_scene_layout::resolve_scene_ui_layout,
};
use crate::host::frame::{
    ATLAS_TEXTURE_PATH, FONT_ATLAS_TEXTURE_PATH, OverlayVertex, SceneDraw, VectorGlyphSnapshot,
    VectorSegmentSnapshot, VectorTextVertex as FrameVectorTextVertex,
};

use super::{RuntimeScene, nine_slice_quad, screen_rect_ndc, text, text_to_ndc};
use crate::host::runtime_scene::camera::sprite_screen_rect;

impl RuntimeScene {
    /// 计算当前所有 `UiEvents` 实体的命中目标（屏幕像素）。
    ///
    /// Image/Text 用 UI 解析（viewport=实际渲染尺寸、reference=设计分辨率），
    /// Sprite 用相机投影到屏幕；供 `process_ui_clicks` 点选分发。
    pub(super) fn compute_click_targets(&mut self) -> Vec<UiClickTarget> {
        let viewport = Viewport {
            width: self.viewport_width.max(1) as f32,
            height: self.viewport_height.max(1) as f32,
        };
        let viewport_arr = [viewport.width, viewport.height];
        self.ensure_scene_ui_layout(viewport_arr);
        // 先提取 UiEvents 实体及其视觉组件（world 借用结束后再计算，避免借用冲突）。
        type UiEventComponents = (
            EntityId,
            Option<UiImage>,
            Option<UiText>,
            Option<(Sprite, Transform)>,
            Option<crate::core::ui_events::UiEvents>,
        );
        let entities: Vec<UiEventComponents> = self.script.with_world(|world| {
            let mut out = Vec::new();
            for id in world.entities_with::<crate::core::ui_events::UiEvents>() {
                let image = world.get::<UiImage>(id).ok().flatten().cloned();
                let text = world.get::<UiText>(id).ok().flatten().cloned();
                let sprite = world.get::<Sprite>(id).ok().flatten().cloned();
                let transform = world.get::<Transform>(id).ok().flatten().cloned();
                let events = world
                    .get::<crate::core::ui_events::UiEvents>(id)
                    .ok()
                    .flatten()
                    .cloned();
                out.push((id, image, text, sprite.zip(transform), events));
            }
            out
        });
        let camera = OrthoTerms::new(self.camera, viewport).ok();
        entities
            .into_iter()
            .filter_map(|(id, image, text, sprite, events)| {
                let name = self.script.entity_name(id)?;
                let rect: ([f32; 2], [f32; 2]) = if image.is_some() {
                    let layout_rect = self.scene_ui_layout_rect(id, viewport_arr)?;
                    (
                        [layout_rect.x, layout_rect.y],
                        [layout_rect.w, layout_rect.h],
                    )
                } else if let Some(text) = text {
                    let layout_rect = self.scene_ui_layout_rect(id, viewport_arr)?;
                    self.text_hit_rect(&layout_rect, &text)?
                } else if let Some((sprite, transform)) = sprite {
                    sprite_screen_rect(&sprite, &transform, camera?)?
                } else {
                    return None;
                };
                let functions =
                    events.map_or_else(Vec::new, |events| UiEvents::click_functions(&events));
                Some(UiClickTarget {
                    name,
                    origin: rect.0,
                    size: rect.1,
                    functions,
                })
            })
            .collect()
    }

    /// 文本 hit 矩形：与编辑器 UI 包围框同款——resolve 出网格原点，度量字形
    /// bbox，返回 `(glyph_origin, glyph_size)`（屏幕像素）。
    fn text_hit_rect(
        &mut self,
        layout_rect: &UiRect,
        text: &UiText,
    ) -> Option<([f32; 2], [f32; 2])> {
        let (_, glyph_bounds) = self.measure_scene_ui_text(text);
        let origin = [layout_rect.x, layout_rect.y];
        let glyph_origin = [origin[0] + glyph_bounds[0], origin[1] + glyph_bounds[1]];
        let glyph_size = [
            (glyph_bounds[2] - glyph_bounds[0]).max(0.0),
            (glyph_bounds[3] - glyph_bounds[1]).max(0.0),
        ];
        Some((glyph_origin, glyph_size))
    }

    /// E16：实体带 `UiLayout` 组件时，其四边 Anchor+Offset 布局优先于
    /// Text/Image 的内联位置数据。把解析矩形回写为 Screen/[0,0] 内联值即可
    /// 复用既有渲染路径——视觉属性（纹理/颜色/文本）仍来自原组件。
    fn scene_ui_layout_rect(&self, id: EntityId, _viewport: [f32; 2]) -> Option<UiRect> {
        self.ui_layout_rects
            .get(&(id.index() as u64))
            .map(|node| node.rect)
    }

    pub(super) fn ensure_scene_ui_layout(&mut self, viewport: [f32; 2]) {
        let (mut resolved, _) = resolve_scene_ui_layout(&self.ui_scene_nodes, viewport);
        // The resolver also gives non-layout roots a preferred-size rect at the
        // origin. That default is for hierarchy traversal; only authored layouts
        // or container placements may override it.
        resolved.retain(|id, resolved| {
            resolved.container_controlled
                || self
                    .ui_scene_nodes
                    .get(id)
                    .is_some_and(|node| node.layout.is_some())
        });
        self.ui_layout_rects = resolved;
    }

    /// Appends Lua-created `ui.panel` quads using the white texture.
    pub(super) fn push_ui_panels(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<()> {
        let panels = self.script.with_dynamic_ui(|panels, _, _| panels.clone());
        for panel in panels.values().filter(|panel| panel.visible) {
            let (origin, size) = resolve_ui_rect(
                panel.position,
                panel.size,
                panel.space,
                panel.anchor,
                [width.max(1) as f32, height.max(1) as f32],
                self.ui_reference,
            );
            let start = vertices.len() as u32;
            if panel.shape == 1 {
                // 圆形（`ui.circle`）：2D 管线拓扑是 triangle list（矩形 6 顶点），圆必须同样
                // 展开成三角列表——每段写 (圆心, 环[i], 环[i+1])。三角扇顶点序（圆心 + 一圈
                // 环）在 triangle list 下会被切成碎三角形：一个楔形 + 沿圆周的一串细片，
                // 画面上就是"轮廓圆 + 跨圆交叉细三角"的伪影（虚拟摇杆/按钮实测踩中）。
                let sw = width.max(1) as f32;
                let sh = height.max(1) as f32;
                let cx = origin[0] + size[0] / 2.0;
                let cy = origin[1] + size[1] / 2.0;
                let r = size[0].min(size[1]) / 2.0;
                let to_ndc =
                    |x: f32, y: f32| -> [f32; 2] { [x / sw * 2.0 - 1.0, 1.0 - y / sh * 2.0] };
                let v = |x: f32, y: f32| OverlayVertex {
                    position: to_ndc(x, y),
                    uv: [0.0, 0.0],
                    color: panel.color,
                    sdf: 0,
                };
                const SEGMENTS: usize = 32;
                let center = v(cx, cy);
                for i in 0..SEGMENTS {
                    let a0 = i as f32 / SEGMENTS as f32 * std::f32::consts::PI * 2.0;
                    let a1 = (i + 1) as f32 / SEGMENTS as f32 * std::f32::consts::PI * 2.0;
                    vertices.push(center);
                    vertices.push(v(cx + a0.cos() * r, cy + a0.sin() * r));
                    vertices.push(v(cx + a1.cos() * r, cy + a1.sin() * r));
                }
            } else {
                vertices.extend(screen_rect_ndc(
                    origin[0],
                    origin[1],
                    size[0],
                    size[1],
                    panel.color,
                    width,
                    height,
                ));
            }
            draws.push(SceneDraw {
                texture: 0, // white
                start,
                count: vertices.len() as u32 - start,
                sdf: false,
                bitmap: false,
                vector: false,
                render_layer: RenderLayer::ScreenUI,
            });
        }
        Ok(())
    }

    /// Appends Lua-created `ui.image` quads above panels.
    pub(super) fn push_ui_images(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<()> {
        let images = self.script.with_dynamic_ui(|_, images, _| images.clone());
        for image in images.values() {
            self.push_image_quad(vertices, draws, width, height, image)?;
        }
        Ok(())
    }

    /// Appends Lua-created `ui.grid` cells and their item icon textures.
    pub(super) fn push_ui_grids(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<()> {
        let grids = self.script.with_dynamic_ui(|_, _, grids| grids.clone());
        for grid in grids.values() {
            let (origin, total) = resolve_ui_rect(
                grid.position,
                grid.total_size(),
                grid.space,
                grid.anchor,
                [width.max(1) as f32, height.max(1) as f32],
                self.ui_reference,
            );
            let start = vertices.len() as u32;
            vertices.extend(screen_rect_ndc(
                origin[0],
                origin[1],
                total[0],
                total[1],
                grid.background,
                width,
                height,
            ));
            draws.push(SceneDraw {
                texture: 0,
                start,
                count: vertices.len() as u32 - start,
                sdf: false,
                bitmap: false,
                vector: false,
                render_layer: RenderLayer::ScreenUI,
            });
            for index in 1..=grid.columns * grid.rows {
                let Some((cell_origin, cell_size)) = grid.cell_rect(
                    index,
                    [width.max(1) as f32, height.max(1) as f32],
                    self.ui_reference,
                ) else {
                    continue;
                };
                let cell = grid.cells.get(index - 1).cloned().unwrap_or_default();
                let start = vertices.len() as u32;
                vertices.extend(screen_rect_ndc(
                    cell_origin[0],
                    cell_origin[1],
                    cell_size[0],
                    cell_size[1],
                    cell.color,
                    width,
                    height,
                ));
                draws.push(SceneDraw {
                    texture: 0,
                    start,
                    count: vertices.len() as u32 - start,
                    sdf: false,
                    bitmap: false,
                    vector: false,
                    render_layer: RenderLayer::ScreenUI,
                });
                if let Some(texture) = cell.texture {
                    let image = DynamicUiImage {
                        texture,
                        position: cell_origin,
                        size: cell_size,
                        color: [cell.color[0], cell.color[1], cell.color[2]],
                        space: crate::core::ui::UiLayoutSpace::Screen,
                        anchor: [0.0, 0.0],
                        nine_slice: None,
                    };
                    self.push_image_quad(vertices, draws, width, height, &image)?;
                }
            }
        }
        Ok(())
    }

    /// Appends screen-space health bars (background + fill) as NDC quads using
    /// the white texture.
    pub(super) fn push_ui_bars(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<()> {
        let bars = self.script.with_ui(|_, bars| bars.clone());
        for bar in bars.values() {
            let start = vertices.len() as u32;
            let background = [0.12, 0.12, 0.15, 0.75];
            let fill_width = (bar.size[0] * bar.ratio).max(0.0);
            vertices.extend_from_slice(&screen_rect_ndc(
                bar.position[0],
                bar.position[1],
                bar.size[0],
                bar.size[1],
                background,
                width,
                height,
            ));
            vertices.extend_from_slice(&screen_rect_ndc(
                bar.position[0],
                bar.position[1],
                fill_width,
                bar.size[1],
                bar.color,
                width,
                height,
            ));
            let count = vertices.len() as u32 - start;
            draws.push(SceneDraw {
                texture: 0, // white
                start,
                count,
                sdf: false,
                bitmap: false,
                vector: false,
                render_layer: RenderLayer::ScreenUI,
            });
        }
        Ok(())
    }

    /// Appends screen-space text glyphs (SDF font atlas) as NDC quads. Shares
    /// `layout_text` with the scene-node UI Text component.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_ui_text(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        vector_vertices: &mut Vec<FrameVectorTextVertex>,
        vector_glyphs: &mut Vec<VectorGlyphSnapshot>,
        vector_segments: &mut Vec<VectorSegmentSnapshot>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<()> {
        let texts = self.script.with_ui(|texts, _| texts.clone());
        let visible: Vec<_> = texts
            .values()
            .filter(|text| text.visible)
            .map(|text| {
                (
                    text.position,
                    text.text.clone(),
                    text.font_size.map_or(text.scale, ui_text_effective_scale),
                )
            })
            .collect();
        if visible.is_empty() {
            return Ok(());
        }
        for (position, text, scale) in visible {
            let use_vector = self.should_render_vector(None, scale, text.chars().count());
            if !use_vector
                || !self.push_vector_text_run(
                    vector_vertices,
                    vector_glyphs,
                    vector_segments,
                    &text,
                    position,
                    scale,
                    [1.0, 1.0, 1.0],
                    None,
                    width,
                    height,
                )?
            {
                let (mesh, is_sdf) = self.layout_text(&text, position, scale, [1.0, 1.0, 1.0])?;
                let is_bitmap = mesh.bitmap;
                let text_vertices = text_to_ndc(mesh, is_sdf, width, height);
                let font_key = if is_sdf {
                    FONT_ATLAS_TEXTURE_PATH.to_owned()
                } else {
                    ATLAS_TEXTURE_PATH.to_owned()
                };
                self.push_text_run(
                    vertices,
                    draws,
                    text_vertices,
                    text::TextRunKind {
                        sdf: is_sdf,
                        bitmap: is_bitmap,
                        font_key: &font_key,
                    },
                    RenderLayer::ScreenUI,
                );
            }
        }
        Ok(())
    }

    /// Pushes scene-node UI components (`Text` / `Image` / `Button`) as NDC
    /// overlay quads on top of the Lua UI. Order: images, buttons, then text.
    /// Buttons render by composing the same image/text helpers used elsewhere.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_scene_ui(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        vector_vertices: &mut Vec<FrameVectorTextVertex>,
        vector_glyphs: &mut Vec<VectorGlyphSnapshot>,
        vector_segments: &mut Vec<VectorSegmentSnapshot>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<()> {
        let viewport = [width.max(1) as f32, height.max(1) as f32];
        self.ensure_scene_ui_layout(viewport);
        // docs/54 §4.2：实体身份的 UI 吃 **overlay 视图**的可见遮罩；按名字注册的画布 UI
        // （面板/血条/网格）不是实体，因此不受遮罩影响。
        let overlay_mask = self.view_overlay_mask;
        let mut filtered = 0usize;
        let images: Vec<(EntityId, UiImage)> = self.script.with_world(|world| {
            let mut images = Vec::new();
            world.for_each::<UiImage>(|id, image| {
                if crate::core::visibility::entity_visible_in(world, id, overlay_mask) {
                    images.push((id, image.clone()));
                } else {
                    filtered += 1;
                }
            });
            images
        });
        for (id, image) in images {
            if let Some(rect) = self.scene_ui_layout_rect(id, viewport) {
                let image = DynamicUiImage {
                    texture: image.texture,
                    position: [rect.x, rect.y],
                    size: [rect.w, rect.h],
                    color: image.color,
                    space: crate::core::ui::UiLayoutSpace::Screen,
                    anchor: [0.0, 0.0],
                    nine_slice: image.nine_slice,
                };
                self.push_image_quad(vertices, draws, width, height, &image)?;
            }
        }
        let texts: Vec<(EntityId, UiText)> = self.script.with_world(|world| {
            let mut texts = Vec::new();
            world.for_each::<UiText>(|id, text| {
                // `ui.scene_text_visible(name, false)` 按实体名隐藏场景文字：
                // 实体仍在场景与 UiLayout 布局里，仅跳过绘制（语义与 web 兼容层一致）。
                // P3-②：换成免克隆判定（旧实现每文字每帧 clone 整份隐藏集）。
                let name_hidden = self.script.is_entity_scene_text_hidden(id);
                if crate::core::visibility::entity_visible_in(world, id, overlay_mask)
                    && !name_hidden
                {
                    texts.push((id, text.clone()));
                } else {
                    filtered += 1;
                }
            });
            texts
        });
        self.view_filtered.set(self.view_filtered.get() + filtered);
        // 关键：先把本帧全部文本的字形预热进图集，再逐条构建网格。若在排版过程
        // 中才光栅化，图集会增长（1024→2048…）；`FontAtlas::grow` 只重缩放已缓存
        // 字形的归一化 UV，不会修正已经写进顶点缓冲的 UV，于是同一帧内先构建的
        // 网格会用旧尺寸的 UV 在更大的图集里采样到 2 倍坐标处的错误字形，表现为
        // 白块 / 缺笔画。预热后图集尺寸在本帧保持稳定。
        self.prewarm_frame_glyphs(&texts);
        for (id, text) in texts {
            let rect = self.scene_ui_layout_rect(id, viewport);
            if let Some(rect) = rect {
                // 点锚点（w=h=0）的 Text：按 pivot 在该点居中文字。
                // 否则文字从 pivot 点开始左对齐向下排，偏出按钮中心。
                let pivot = self
                    .ui_scene_nodes
                    .get(&(id.index() as u64))
                    .and_then(|n| n.layout.as_ref())
                    .map(|l| l.pivot)
                    .unwrap_or([0.5, 0.5]);
                let mut position = [rect.x, rect.y];
                // 先判断渲染路径，measure 必须与渲染走同一路径，否则字体度量不一致。
                let use_vector = self.should_render_vector(
                    Some(text.font.as_str()),
                    text.effective_scale(),
                    text.text.chars().count(),
                );
                if rect.w < 1.0 && rect.h < 1.0 {
                    // 收集实际渲染顶点的包围盒（origin=[0,0] 时）。
                    let mut bbox: Option<(f32, f32, f32, f32)> = None;
                    if use_vector
                        && let Some(handle) = self.ensure_vector_font(if text.font.is_empty() {
                            ""
                        } else {
                            text.font.as_str()
                        })
                        && let Ok(mut atlas) = handle.lock()
                    {
                        let style = crate::core::text::TextStyle {
                            scale: text.effective_scale(),
                            color: [1.0, 1.0, 1.0, 1.0],
                            ..Default::default()
                        };
                        if let Ok(verts) = crate::core::text::TextLayout::build_vector(
                            &text.text,
                            [0.0, 0.0],
                            style,
                            &mut atlas,
                        ) {
                            bbox = Some(verts.iter().fold(
                                (f32::MAX, f32::MIN, f32::MAX, f32::MIN),
                                |(a, b, c, d), v| {
                                    (
                                        a.min(v.position[0]),
                                        b.max(v.position[0]),
                                        c.min(v.position[1]),
                                        d.max(v.position[1]),
                                    )
                                },
                            ));
                        }
                    }
                    if bbox.is_none()
                        && let Ok((measure, _)) = self.layout_text(
                            &text.text,
                            [0.0, 0.0],
                            text.effective_scale(),
                            text.color,
                        )
                    {
                        bbox = Some(measure.vertices.iter().fold(
                            (f32::MAX, f32::MIN, f32::MAX, f32::MIN),
                            |(a, b, c, d), v| {
                                (
                                    a.min(v.position[0]),
                                    b.max(v.position[0]),
                                    c.min(v.position[1]),
                                    d.max(v.position[1]),
                                )
                            },
                        ));
                    }
                    if let Some((min_x, max_x, min_y, max_y)) = bbox {
                        position[0] = rect.x - min_x * (1.0 - pivot[0]) - max_x * pivot[0];
                        position[1] = rect.y - min_y * (1.0 - pivot[1]) - max_y * pivot[1];
                    }
                }
                // `layout_text` already emits absolute screen-space vertices;
                // adding the layout origin again would displace scene text by
                // its full top-left position (the visible "text ran away" bug).
                if use_vector
                    && self.push_vector_text_run(
                        vector_vertices,
                        vector_glyphs,
                        vector_segments,
                        &text.text,
                        position,
                        text.effective_scale(),
                        text.color,
                        Some(&text.font),
                        width,
                        height,
                    )?
                {
                    continue;
                }
                let (mesh, is_sdf, font_key) = self.layout_scene_text(&text, position)?;
                let is_bitmap = mesh.bitmap;
                let text_vertices = text_to_ndc(mesh, is_sdf, width, height);
                self.push_text_run(
                    vertices,
                    draws,
                    text_vertices,
                    text::TextRunKind {
                        sdf: is_sdf,
                        bitmap: is_bitmap,
                        font_key: &font_key,
                    },
                    RenderLayer::ScreenUI,
                );
            }
        }
        Ok(())
    }

    /// Pushes a screen-space textured quad for the `Image` component.
    fn push_image_quad(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        width: u32,
        height: u32,
        image: &DynamicUiImage,
    ) -> anyhow::Result<()> {
        let sw = width.max(1) as f32;
        let sh = height.max(1) as f32;
        let (origin, size) = resolve_ui_rect(
            image.position,
            image.size,
            image.space,
            image.anchor,
            [sw, sh],
            self.ui_reference,
        );
        let color = [image.color[0], image.color[1], image.color[2], 1.0];
        // 九宫格路径：把目标矩形拆成 3×3 格。无切片 / 纹理尺寸取不到时回退单四边形。
        if let Some(slice) = image
            .nine_slice
            .filter(|s| s.iter().all(|v| v.is_finite() && *v >= 0.0) && s.iter().any(|v| *v > 0.0))
            && let Some((tw, th)) = self.texture_dimensions(&image.texture)
        {
            // 把作者空间边框映射到解析后的目标空间，使角部在 Canvas 缩放下
            // 保持纹理像素密度（Screen / scale=1 时比值为 1）。
            let scale_x = if image.size[0] > f32::EPSILON {
                size[0] / image.size[0]
            } else {
                1.0
            };
            let scale_y = if image.size[1] > f32::EPSILON {
                size[1] / image.size[1]
            } else {
                1.0
            };
            let slice = [
                slice[0] * scale_x,
                slice[1] * scale_x,
                slice[2] * scale_y,
                slice[3] * scale_y,
            ];
            let cells = nine_slice_cells(size[0], size[1], tw as f32, th as f32, slice);
            let start = vertices.len() as u32;
            for cell in cells {
                let cw = cell.dest[2] - cell.dest[0];
                let ch = cell.dest[3] - cell.dest[1];
                if cw <= 0.0 || ch <= 0.0 {
                    continue;
                }
                let x0 = (origin[0] + cell.dest[0]) / sw * 2.0 - 1.0;
                let y0 = 1.0 - (origin[1] + cell.dest[1]) / sh * 2.0;
                let x1 = (origin[0] + cell.dest[2]) / sw * 2.0 - 1.0;
                let y1 = 1.0 - (origin[1] + cell.dest[3]) / sh * 2.0;
                vertices.extend(nine_slice_quad(x0, y0, x1, y1, cell.src, color));
            }
            if vertices.len() as u32 > start {
                draws.push(SceneDraw {
                    texture: self.texture_id(&image.texture) + 1,
                    start,
                    count: vertices.len() as u32 - start,
                    sdf: false,
                    bitmap: false,
                    vector: false,
                    render_layer: RenderLayer::ScreenUI,
                });
            }
            return Ok(());
        }
        let x0 = origin[0] / sw * 2.0 - 1.0;
        let y0 = 1.0 - origin[1] / sh * 2.0;
        let x1 = (origin[0] + size[0]) / sw * 2.0 - 1.0;
        let y1 = 1.0 - (origin[1] + size[1]) / sh * 2.0;
        let quad = [
            OverlayVertex {
                position: [x0, y0],
                uv: [0.0, 0.0],
                color,
                sdf: 0,
            },
            OverlayVertex {
                position: [x1, y0],
                uv: [1.0, 0.0],
                color,
                sdf: 0,
            },
            OverlayVertex {
                position: [x1, y1],
                uv: [1.0, 1.0],
                color,
                sdf: 0,
            },
            OverlayVertex {
                position: [x0, y0],
                uv: [0.0, 0.0],
                color,
                sdf: 0,
            },
            OverlayVertex {
                position: [x1, y1],
                uv: [1.0, 1.0],
                color,
                sdf: 0,
            },
            OverlayVertex {
                position: [x0, y1],
                uv: [0.0, 1.0],
                color,
                sdf: 0,
            },
        ];
        let start = vertices.len() as u32;
        vertices.extend(quad);
        draws.push(SceneDraw {
            texture: self.texture_id(&image.texture) + 1,
            start,
            count: vertices.len() as u32 - start,
            sdf: false,
            bitmap: false,
            vector: false,
            render_layer: RenderLayer::ScreenUI,
        });
        Ok(())
    }
}
