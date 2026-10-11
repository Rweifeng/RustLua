//! 屏幕 UI 与场景 UI：点击目标收集、UI 布局解析与 `push_ui_*` / `push_scene_ui` 顶点构建
//! （WP12 阶段 3b 从 `mod.rs` 原样搬出）。
//!
//! `compute_click_targets` 与 `text_hit_rect` 跟着进来：点击矩形与绘制矩形必须同源，
//! 分开两个模块就成了"同一事实两处算"（计划 §2.2 的那类病）。

use crate::host::frame::{
    ATLAS_TEXTURE_PATH, FONT_ATLAS_TEXTURE_PATH, OverlayVertex, SceneDraw, VectorGlyphSnapshot,
    VectorSegmentSnapshot, VectorTextVertex as FrameVectorTextVertex,
};
use rustlua_camera::camera::{OrthoTerms, Viewport};
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::transform::Transform;
use rustlua_render::layer::RenderLayer;
use rustlua_render::sprite::Sprite;
use rustlua_ui::ui::{
    DynamicUiImage, UiImage, UiText, nine_slice_cells, resolve_ui_rect, ui_text_effective_scale,
};
use rustlua_ui::ui_events::{UiClickTarget, UiEvents};
use rustlua_ui::ui_layout::UiRect;
use rustlua_ui::ui_scene_layout::resolve_scene_ui_layout;

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
            Option<rustlua_ui::ui_events::UiEvents>,
        );
        let entities: Vec<UiEventComponents> = self.script.with_world(|world| {
            let mut out = Vec::new();
            for id in world.entities_with::<rustlua_ui::ui_events::UiEvents>() {
                let image = world.get::<UiImage>(id).ok().flatten().cloned();
                let text = world.get::<UiText>(id).ok().flatten().cloned();
                let sprite = world.get::<Sprite>(id).ok().flatten().cloned();
                let transform = world.get::<Transform>(id).ok().flatten().cloned();
                let events = world
                    .get::<rustlua_ui::ui_events::UiEvents>(id)
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
        self.sync_prefab_ui_nodes();
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

    /// 把脚本侧运行期 spawn 的 UI 预设节点并进 `ui_scene_nodes`（键 = 活实体槽位号）。
    ///
    /// 并表发生在**解算之前**：矩形、文本 pivot、点击命中框三条读路径都查这张表，
    /// 分两处维护就等于"画的位置和点中的位置可以不一样"。
    ///
    /// **先比修订号，再取表**：原写法是每帧先 `let spawned = self.script.prefab_ui_nodes();`
    /// 再 `if spawned == self.applied_prefab_ui_nodes`——克隆发生在比较**之前**，于是
    /// "没变就早退"只在分配已经付过之后才成立；UI 预设化之后这张表是非空的（每个面板的布局节点
    /// 都在里头），那份拷贝每帧都付（`docs/60` §10.3 第 2 条）。现在未变时**一次 clone 都不做**。
    ///
    /// 槽位被复用（场景 UI 实体先被 `entity.destroy`、随后 spawn 落进同一个槽）时 spawn 的节点
    /// 覆盖旧的：**旧节点本来就是死实体的**，despawn 时按登记过的键移除，该槽此后没有活实体，
    /// 移除不会带走仍在世的东西。
    pub(super) fn sync_prefab_ui_nodes(&mut self) {
        let revision = self.script.prefab_ui_nodes_revision();
        if revision == self.applied_prefab_ui_revision {
            return;
        }
        let spawned = self.script.prefab_ui_nodes();
        for key in self.applied_prefab_ui_nodes.keys() {
            self.ui_scene_nodes.remove(key);
        }
        for (key, node) in &spawned {
            self.ui_scene_nodes.insert(*key, node.clone());
        }
        self.ui_layout_rects.clear();
        self.applied_prefab_ui_nodes = spawned;
        self.applied_prefab_ui_revision = revision;
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
                        // 格子贴图这一层**一直**是不透明发射（`cell.color[3]` 只作用于同一格
                        // 先推的那块底色四边形）。这批只给实体层 `Image` 补 alpha，这里保持旧
                        // 画面，免得顺手改动背包格子的观感。
                        alpha: 1.0,
                        space: rustlua_ui::ui::UiLayoutSpace::Screen,
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
            // 路径判定为 false 时**不能**调用 push_vector_text_run：顶点只要进了
            // vector_vertices 就会被矢量通道画出来，于是像素字体在同一帧里被点阵档
            // 与轮廓档各画一遍（双层字形互相错位），而日志一个字都没有。
            let vector_ok = use_vector
                && match self.push_vector_text_run(
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
                ) {
                    Ok(ok) => ok,
                    Err(error) => {
                        tracing::warn!(
                            target: "render",
                            text = %text,
                            error = %error,
                            "[runtime] overlay text vector run error, falling back"
                        );
                        false
                    }
                };
            if !use_vector || !vector_ok {
                let (mesh, is_sdf) = match self.layout_text(&text, position, scale, [1.0, 1.0, 1.0])
                {
                    Ok(result) => result,
                    Err(error) => {
                        tracing::warn!(
                            target: "render",
                            text = %text,
                            error = %error,
                            "[runtime] overlay text layout failed, skipping"
                        );
                        continue;
                    }
                };
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

    /// 预设/场景实体 `Image` 趟（静态骨架）。**必须在动态层 `push_ui_*` 之前提交**：
    /// M66–M71 六屏迁移的前提就是"骨架在下、脚本每帧写的元素在上"，顺序反了卡底
    /// （`ui_skill_card_9s.png` 全不透明）会整块埋掉压在它上面的 24×24 招式图标。
    /// 布局由调用方每帧解算一次（`ensure_scene_ui_layout`），这里不再重复解算。
    pub(super) fn push_scene_ui_images(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<()> {
        let viewport = [width.max(1) as f32, height.max(1) as f32];
        // docs/54 §4.2：实体身份的 UI 吃 **overlay 视图**的可见遮罩；按名字注册的画布 UI
        // （面板/血条/网格）不是实体，因此不受遮罩影响。
        let overlay_mask = self.view_overlay_mask;
        let mut filtered = 0usize;
        let images: Vec<(EntityId, UiImage)> = self.script.with_world(|world| {
            let mut images = Vec::new();
            world.for_each::<UiImage>(|id, image| {
                if rustlua_render::visibility::entity_visible_in(world, id, overlay_mask) {
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
                    color: [image.color[0], image.color[1], image.color[2]],
                    alpha: image.color[3],
                    space: rustlua_ui::ui::UiLayoutSpace::Screen,
                    anchor: [0.0, 0.0],
                    nine_slice: image.nine_slice,
                };
                self.push_image_quad(vertices, draws, width, height, &image)?;
            }
        }
        self.view_filtered.set(self.view_filtered.get() + filtered);
        Ok(())
    }

    /// 预设/场景实体 `Text` 趟：仍然**最后**提交。`prewarm_frame_glyphs` 必须是本帧最后一个
    /// 会撑大字体图集的人——图集增长只重缩放已缓存字形的 UV、不回改已写进顶点缓冲的 UV，
    /// 所以这一趟不能挪到动态层文字（`push_ui_text`）之前。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_scene_ui_text(
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
        let overlay_mask = self.view_overlay_mask;
        let mut filtered = 0usize;
        let texts: Vec<(EntityId, UiText)> = self.script.with_world(|world| {
            let mut texts = Vec::new();
            world.for_each::<UiText>(|id, text| {
                // `ui.scene_text_visible(name, false)` 按实体名隐藏场景文字：
                // 实体仍在场景与 UiLayout 布局里，仅跳过绘制（语义与 web 兼容层一致）。
                // P3-②：换成免克隆判定（旧实现每文字每帧 clone 整份隐藏集）。
                let name_hidden = self.script.is_entity_scene_text_hidden(id);
                if rustlua_render::visibility::entity_visible_in(world, id, overlay_mask)
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
                        let style = rustlua_render::text::TextStyle {
                            scale: text.effective_scale(),
                            color: [1.0, 1.0, 1.0, 1.0],
                            ..Default::default()
                        };
                        if let Ok(verts) = rustlua_render::text::TextLayout::build_vector(
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
                // P3：单条文字排版失败（缺字形/字体图集未就绪）不得中断整帧场景 UI——
                // 否则面板里第一个缺字的 Text 会让后续所有文字连同按钮一起消失，
                // 症状与「字体没加载」一模一样但日志一个字都没有。
                // 与 `push_ui_text`/世界文字同一牙齿：use_vector=false 时不能生成矢量档
                // 顶点，否则像素字体会被点阵与轮廓两条通道各画一遍。
                let vector_ok = use_vector
                    && match self.push_vector_text_run(
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
                    ) {
                        Ok(ok) => ok,
                        Err(error) => {
                            tracing::warn!(
                                target: "render",
                                text = %text.text,
                                error = %error,
                                "[runtime] vector text run error, falling back to SDF"
                            );
                            false
                        }
                    };
                if vector_ok {
                    continue;
                }
                let (mesh, is_sdf, font_key) = match self.layout_scene_text(&text, position) {
                    Ok(result) => result,
                    Err(error) => {
                        tracing::warn!(
                            target: "render",
                            text = %text.text,
                            entity = ?id,
                            error = %error,
                            "[runtime] scene UI text layout failed, skipping"
                        );
                        continue;
                    }
                };
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
        let color = [image.color[0], image.color[1], image.color[2], image.alpha];
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
