//! 瓦片映射的顶点构建与 chunk 级脏重建（WP12 阶段 2 从 `runtime_scene.rs` 原样搬出）。
//!
//! 可见性依据（`tasks/done/WP12-host拆分.md` §3.1.4）：这里只有被 `RuntimeScene` 的每帧路径调用的
//! 入口标 `pub(super)`（私有域 = `runtime_scene`，父与全部兄弟子模块可用）；`build_chunk` 只被
//! 本文件的分块构建调用，保持私有。类型 `CachedTilemapLayer` / `CachedTileSprite` / `TileChunk`
//! 仍定义在 `mod.rs`（它们被 `RuntimeScene` 的私有字段持有），本文件靠 `use super::{...}` 取用。

use std::{collections::HashMap, path::PathBuf};

use crate::core::{
    camera::{CameraBounds, OrthoTerms, Viewport},
    entity::EntityId,
    hierarchy::GlobalTransform,
    sprite_batch::{OverlayVertexSink, SpriteDrawBatch, WorldSpriteVertexSink, ndc_from_screen},
    tilemap::Tilemap,
    transform::{Transform, Vec2},
};
use crate::host::frame::{OverlayVertex, WorldSpriteVertex};

use super::{CachedTileSprite, CachedTilemapLayer, RuntimeScene, TileChunk, texture_id_of};

/// WP4 ④ chunk 级脏重建：瓦片块边长（格数）。块行主序拼接 == 原整层行主序。
pub(super) const TILE_CHUNK_SIZE: usize = 32;

/// 构建一个块（`ox/oy` = 块左上角格坐标，`w/h` = 实际块尺寸，边缘块可能不满）。
/// 与整层平铺构建的唯一区别是范围限定在块内；`center_offset` 语义不变
/// （相对层原点、预乘 `scale`——因此 `scale` 变化必须整层重建，见 [`rebuild_dirty_chunks`]）。
///
/// M72-01/02：格值 → 图集条目走 `resolve_atlas_index`（自动瓦片/变体在里层解算，
/// 关闭时是恒等映射，旧图逐格等价），UV 走 `atlas_uv`（多行多列），顶点色走 `tint_at`。
fn build_chunk(
    tilemap: &Tilemap,
    ox: usize,
    oy: usize,
    w: usize,
    h: usize,
    scale: Vec2,
) -> TileChunk {
    let mut sprites = Vec::new();
    for y in oy..(oy + h) {
        for x in ox..(ox + w) {
            let entry = tilemap.resolve_atlas_index(x, y);
            if entry == 0 {
                continue;
            }
            let (uv_min, uv_max) = tilemap.atlas_uv(entry);
            let center_offset = Vec2::new(
                (x as f32 + 0.5) * tilemap.tile_size.x * scale.x,
                (y as f32 + 0.5) * tilemap.tile_size.y * scale.y,
            );
            sprites.push(CachedTileSprite {
                center_offset,
                uv_min,
                uv_max,
                tint: tilemap.tint_at(x, y),
            });
        }
    }
    TileChunk {
        ox,
        oy,
        w,
        h,
        sprites,
    }
}

/// 全新建层：按 32×32 块行主序切分并逐块构建，累计 `total`。
pub(super) fn build_all_chunks(
    transform: Transform,
    tilemap: Tilemap,
) -> anyhow::Result<CachedTilemapLayer> {
    let scale = transform.scale;
    let mut chunks = Vec::new();
    let mut total = 0usize;
    let mut oy = 0usize;
    while oy < tilemap.height {
        let h = (tilemap.height - oy).min(TILE_CHUNK_SIZE);
        let mut ox = 0usize;
        while ox < tilemap.width {
            let w = (tilemap.width - ox).min(TILE_CHUNK_SIZE);
            let chunk = build_chunk(&tilemap, ox, oy, w, h, scale);
            total += chunk.sprites.len();
            chunks.push(chunk);
            ox += w;
        }
        oy += h;
    }
    Ok(CachedTilemapLayer {
        transform,
        tilemap,
        chunks,
        total,
    })
}

/// 逐块脏重建：只重建 `tiles`/`tint_ids` 切片变化的块；`scale` 变化时所有块都要重建
/// （`center_offset` 预乘了 scale）。返回重建的块数。`solid`/`meta` 变化不触发
/// 重建——它们不参与 sprite 生成，像素结果不变（由 2D golden 像素等价兜底）。
///
/// M72-02 自动瓦片的脏判定要**外扩一格**：一格的图集条目由它的四个邻格决定，
/// 改 A 格会连带改变相邻块里贴着边界的那一格的解算结果。`tint_ids` 只影响顶点色、
/// 不影响 UV，因此按同一区域一起比对（它没有邻格依赖，不需要外扩）。
pub(super) fn rebuild_dirty_chunks(
    layer: &mut CachedTilemapLayer,
    tilemap: &Tilemap,
    scale: Vec2,
) -> usize {
    let old = &layer.tilemap;
    let halo = usize::from(tilemap.autotile);
    let mut rebuilt = 0usize;
    let mut total = 0usize;
    for chunk in &mut layer.chunks {
        let scale_changed = layer.transform.scale != scale;
        // 逐行段比较：row-major 的 tiles 里块区域**不是连续切片**（块宽 < 层宽时
        // 跨行），按块内每行 [y*width+ox, y*width+ox+w) 分段比对才覆盖真实区域。
        let mut dirty = false;
        for y in chunk.oy.saturating_sub(halo)..=(chunk.oy + chunk.h).min(tilemap.height - 1) {
            let row_start = chunk.ox.saturating_sub(halo);
            let row_end = (chunk.ox + chunk.w).min(tilemap.width - 1);
            let stride = tilemap.width;
            if old.tiles[y * stride + row_start..=y * stride + row_end]
                != tilemap.tiles[y * stride + row_start..=y * stride + row_end]
                || old.tint_ids[y * stride + chunk.ox..y * stride + chunk.ox + chunk.w]
                    != tilemap.tint_ids[y * stride + chunk.ox..y * stride + chunk.ox + chunk.w]
            {
                dirty = true;
                break;
            }
        }
        if scale_changed || dirty {
            let rebuilt_chunk = build_chunk(tilemap, chunk.ox, chunk.oy, chunk.w, chunk.h, scale);
            total += rebuilt_chunk.sprites.len();
            *chunk = rebuilt_chunk;
            rebuilt += 1;
        } else {
            total += chunk.sprites.len();
        }
    }
    layer.total = total;
    rebuilt
}

/// 把一层的瓦片顶点**直接以 NDC**追加进最终 overlay 缓冲（WP6 单趟化：不再先产出
/// `SpriteVertex` 再转换一遍）。屏幕像素 → NDC 走 `ndc_from_screen` 这唯一一份实现。
///
/// `cull` 为真时跳过**完全落在视口外**的瓦片（WP4 ④"可见 chunk 提取"）。判定用的是
/// "瓦片中心的屏幕坐标 vs 视口矩形外扩一个瓦片包围盒"：外扩量由 4 个角点偏移的 AABB 得出
/// （`screen_offsets` 对整层是同一组常量），因此旋转、缩放、负缩放都自动算进去，且**跨在
/// 边缘上的瓦片一律保留**。视口外的瓦片本来就画不出来，所以像素结果必须完全不变——这一点
/// 由 2D overlay golden 用"开/关两种设置的像素逐字节相同"来证明。
pub(super) fn append_tilemap_layer<V: OverlayVertexSink>(
    vertices: &mut Vec<V>,
    draws: &mut Vec<SpriteDrawBatch>,
    texture_id: u32,
    camera: OrthoTerms,
    layer: &CachedTilemapLayer,
    cull: bool,
) {
    if layer.total == 0 {
        return;
    }
    let start = vertices.len();
    let half = Vec2::new(
        0.5 * layer.tilemap.tile_size.x * layer.transform.scale.x,
        0.5 * layer.tilemap.tile_size.y * layer.transform.scale.y,
    );
    let (sin, cos) = layer.transform.rotation.sin_cos();
    let corners = [
        Vec2::new(-half.x, half.y),
        Vec2::new(half.x, half.y),
        Vec2::new(half.x, -half.y),
        Vec2::new(-half.x, -half.y),
    ];
    let screen_offsets = corners.map(|corner| {
        let rotated = Vec2::new(
            corner.x * cos - corner.y * sin,
            corner.x * sin + corner.y * cos,
        );
        camera.world_delta_to_screen(rotated)
    });
    // 整层共用的一组保守外扩量：角点屏幕偏移的 AABB，再加 1px 抹掉亚像素与采样误差。
    let (mut extent_x, mut extent_y) = (0.0f32, 0.0f32);
    for offset in &screen_offsets {
        extent_x = extent_x.max(offset.x.abs());
        extent_y = extent_y.max(offset.y.abs());
    }
    extent_x += 1.0;
    extent_y += 1.0;
    let viewport = camera.viewport();
    let (viewport_w, viewport_h) = (viewport.width, viewport.height);
    // WP4 ④：块行主序 + 块内行主序 == 原整层行主序 → 顶点顺序逐字节不变。
    for chunk in &layer.chunks {
        for tile in &chunk.sprites {
            let center_world = Vec2::new(
                layer.transform.position.x + tile.center_offset.x,
                layer.transform.position.y + tile.center_offset.y,
            );
            let center = camera.world_to_screen(center_world);
            if cull
                && (center.x + extent_x < 0.0
                    || center.x - extent_x > viewport_w
                    || center.y + extent_y < 0.0
                    || center.y - extent_y > viewport_h)
            {
                continue;
            }
            let positions = screen_offsets.map(|offset| [center.x + offset.x, center.y + offset.y]);
            let uv = [
                [tile.uv_min.x, tile.uv_min.y],
                [tile.uv_max.x, tile.uv_min.y],
                [tile.uv_max.x, tile.uv_max.y],
                [tile.uv_min.x, tile.uv_max.y],
            ];
            let tint = tile.tint;
            let vertex = |corner: usize| {
                V::from_ndc(
                    ndc_from_screen(positions[corner], viewport),
                    uv[corner],
                    tint,
                )
            };
            vertices.extend_from_slice(&[
                vertex(0),
                vertex(1),
                vertex(2),
                vertex(0),
                vertex(2),
                vertex(3),
            ]);
        }
    }
    let count = vertices.len() - start;
    if let Some(previous) = draws.last_mut()
        && previous.texture_id == texture_id
        && previous.layer == 0
        && previous.vertex_start + previous.vertex_count == start
    {
        previous.vertex_count += count;
    } else {
        // overlay 版瓦片（NDC 顶点）不进世界深度通道，深度策略恒为"不写"。
        draws.push(SpriteDrawBatch::translucent(
            texture_id,
            0,
            crate::core::layer::RenderLayer::World2D,
            start,
            count,
        ));
    }
}

/// 世界坐标版瓦片层（WP6 世界 Sprite 深度通道用）：与 [`append_tilemap_layer`] 共用同一份
/// 几何（中心偏移 + 旋转缩放后的四角 + 瓦片 UV），区别只有两点：
/// ① 顶点写**世界坐标** `(x, y, 0)`，投影交给顶点着色器的 `Camera2D::view_projection`；
/// ② 剔除用相机可见域的**世界 AABB**（CPU 侧不需要再算屏幕坐标）。
///
/// `z` 固定 0：瓦片层就是世界平面。将来若要"压在瓦片下面/上面"的层，把 `z` 参数化即可。
pub(super) fn append_tilemap_layer_world<V: WorldSpriteVertexSink>(
    vertices: &mut Vec<V>,
    draws: &mut Vec<SpriteDrawBatch>,
    texture_id: u32,
    layer: &CachedTilemapLayer,
    cull_bounds: Option<CameraBounds>,
) {
    if layer.total == 0 {
        return;
    }
    let start = vertices.len();
    let half = Vec2::new(
        0.5 * layer.tilemap.tile_size.x * layer.transform.scale.x,
        0.5 * layer.tilemap.tile_size.y * layer.transform.scale.y,
    );
    let (sin, cos) = layer.transform.rotation.sin_cos();
    let corners = [
        Vec2::new(-half.x, half.y),
        Vec2::new(half.x, half.y),
        Vec2::new(half.x, -half.y),
        Vec2::new(-half.x, -half.y),
    ];
    let offsets = corners.map(|corner| {
        Vec2::new(
            corner.x * cos - corner.y * sin,
            corner.x * sin + corner.y * cos,
        )
    });
    // 保守剔除半径：外接圆半径（与旋转无关），因此旋转/负缩放都不会误剔。
    let radius = (half.x * half.x + half.y * half.y).sqrt() + 1.0;
    // WP4 ④：块行主序 + 块内行主序 == 原整层行主序 → 顶点顺序逐字节不变。
    for chunk in &layer.chunks {
        for tile in &chunk.sprites {
            let center = Vec2::new(
                layer.transform.position.x + tile.center_offset.x,
                layer.transform.position.y + tile.center_offset.y,
            );
            if let Some(bounds) = cull_bounds
                && (center.x + radius < bounds.min.x
                    || center.x - radius > bounds.max.x
                    || center.y + radius < bounds.min.y
                    || center.y - radius > bounds.max.y)
            {
                continue;
            }
            let positions = offsets.map(|offset| [center.x + offset.x, center.y + offset.y]);
            let uv = [
                [tile.uv_min.x, tile.uv_min.y],
                [tile.uv_max.x, tile.uv_min.y],
                [tile.uv_max.x, tile.uv_max.y],
                [tile.uv_min.x, tile.uv_max.y],
            ];
            let tint = tile.tint;
            let vertex = |corner: usize| {
                V::from_world(
                    [positions[corner][0], positions[corner][1], 0.0],
                    uv[corner],
                    tint,
                )
            };
            vertices.extend_from_slice(&[
                vertex(0),
                vertex(1),
                vertex(2),
                vertex(0),
                vertex(2),
                vertex(3),
            ]);
        }
    }
    let count = vertices.len() - start;
    if count == 0 {
        return;
    }
    if let Some(previous) = draws.last_mut()
        && previous.texture_id == texture_id
        && previous.layer == 0
        && previous.vertex_start + previous.vertex_count == start
    {
        previous.vertex_count += count;
    } else {
        // 瓦片是**完全不透明**的世界内容：写深度，因此近处的瓦片能挡住远处的网格，
        // 远处的网格也不会被瓦片层错误地穿出来（docs/54 §8-④）。
        draws.push(SpriteDrawBatch::opaque(
            texture_id,
            0,
            crate::core::layer::RenderLayer::World2D,
            start,
            count,
        ));
    }
}

/// 把世界通道的绘制分段整体重排成**画家顺序**（远 → 近），并同步搬移顶点。
///
/// 不写深度时顺序是唯一的遮挡依据，而世界内容来自三个各自有序的产出者（瓦片层、精灵、
/// 世界文字），因此必须在交给渲染器之前统一重排。排序键与 `SpriteBatch::build_world_into`
/// 保持一致：**先 `layer`（作者声明的排序层必须压过深度）、再 z 从远到近、最后原有先后**
/// （稳定，因此同一输入每次输出相同）。分段的 z 直接读它第一个顶点的 z——分段由产出者保证
/// **深度一致**（跨 z 不合并），所以这是准确的，不需要给分段再加一个字段。
pub(super) fn sort_world_ranges(
    vertices: &mut Vec<WorldSpriteVertex>,
    draws: &mut Vec<SpriteDrawBatch>,
) {
    if draws.len() < 2 {
        return;
    }
    let mut order: Vec<usize> = (0..draws.len()).collect();
    order.sort_by(|left, right| {
        let (left_draw, right_draw) = (&draws[*left], &draws[*right]);
        left_draw
            .layer
            .cmp(&right_draw.layer)
            .then_with(|| {
                let left_z = vertices[left_draw.vertex_start].position[2];
                let right_z = vertices[right_draw.vertex_start].position[2];
                // 大 z = 更远 ⇒ 先画。
                right_z.total_cmp(&left_z)
            })
            .then_with(|| left.cmp(right))
    });
    if order.iter().copied().eq(0..draws.len()) {
        return; // 已经是有序的：不做任何搬移，也不分配。
    }
    let mut rebuilt: Vec<WorldSpriteVertex> = Vec::with_capacity(vertices.len());
    let mut rebuilt_draws: Vec<SpriteDrawBatch> = Vec::with_capacity(draws.len());
    for index in order {
        let draw = draws[index];
        let start = rebuilt.len();
        let end = draw.vertex_start + draw.vertex_count;
        rebuilt.extend_from_slice(&vertices[draw.vertex_start..end]);
        rebuilt_draws.push(SpriteDrawBatch {
            vertex_start: start,
            vertex_count: draw.vertex_count,
            ..draw
        });
    }
    *vertices = rebuilt;
    *draws = rebuilt_draws;
}

impl RuntimeScene {
    /// Deduplicates a texture asset path and returns its id for batch items.
    pub(super) fn texture_id(&mut self, path: &str) -> u32 {
        texture_id_of(
            path,
            &self.assets,
            &mut self.texture_paths,
            &mut self.texture_lookup,
            &mut self.texture_handles,
        )
    }

    /// 纹理像素尺寸（惰性、按 path 缓存），仅供九宫格 UV 计算。只读 PNG 头，
    /// 不做全量解码。路径解析与渲染器一致（`project_root.join(path)`，无根则用
    /// 原始路径），并走与纹理加载相同的 pack/decode 解包。读取失败 → `None` →
    /// 回退单四边形。
    pub(super) fn texture_dimensions(&mut self, path: &str) -> Option<(u32, u32)> {
        if let Some(dims) = self.texture_dims.get(path) {
            return Some(*dims);
        }
        let resolved = self
            .project_root
            .as_ref()
            .map(|root| root.join(path))
            .unwrap_or_else(|| PathBuf::from(path));
        // 与贴图加载同一条通道：wasm 上没有文件系统，裸 `fs::read` 恒 `ENOSYS`，
        // 拿不到尺寸就会退化成单四边形（第 45 轮：manor 的地表 tileset 正在失败清单里）。
        let dims = crate::core::vfs::read(&resolved)
            .ok()
            .and_then(|bytes| crate::core::pack::decode(&bytes).ok())
            .and_then(|bytes| {
                image::ImageReader::new(std::io::Cursor::new(bytes))
                    .with_guessed_format()
                    .ok()?
                    .into_dimensions()
                    .ok()
            });
        if let Some(dims) = dims {
            self.texture_dims.insert(path.to_owned(), dims);
        }
        dims
    }

    /// Builds the 2D mesh for all Tilemap components using a tilemap-specific
    /// hot path that precomputes shared camera/rotation terms once per frame.
    ///
    /// 缓存命中时**不克隆地图**：把上一帧的缓存整体取出，命中的条目直接移动进新缓存，
    /// 只有真正变化的地图才克隆重建。旧实现每帧为每个 Tilemap 克隆整张地图，100×100
    /// 实测每帧 8.5 MB 分配（`wp0-v4` 的 tilemap 阶梯）。
    pub(super) fn append_tilemap_vertices(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        world_vertices: &mut Vec<WorldSpriteVertex>,
        overlay_draws: &mut Vec<SpriteDrawBatch>,
        world_draws: &mut Vec<SpriteDrawBatch>,
        viewport: Viewport,
    ) -> anyhow::Result<()> {
        let Ok(camera) = OrthoTerms::new(self.camera, viewport) else {
            // 相机退化（zoom 非正等）时不产瓦片顶点：与精灵路径一致，等宿主修好相机再画。
            return Ok(());
        };
        // WP4 ④：视口外瓦片只在 `vertices()` 之外还被用到时不剔除（不存在这种调用），
        // 因此这里直接取运行时开关；关掉即像素对拍用的"整层全画"档。
        let cull = self.tilemap_culling;
        // WP6：世界 Sprite 深度通道打开时，瓦片层也产出**世界坐标**顶点（与精灵一致），
        // 因此它同样参与深度测试——此前只有精灵有世界顶点，瓦片会被挡在通道外面。
        let world_pass = self.world_sprite_depth;
        // 与世界 Sprite 同一条规矩：世界通道的可见域由宿主声明的世界视图决定，有 3D 内容时
        // 一律不按 2D 相机裁剪（宁可多画，绝不误丢）。
        let cull_bounds = if cull && world_pass && !self.world_view_has_3d_content() {
            self.camera.visible_world_bounds(viewport).ok()
        } else {
            None
        };
        {
            let RuntimeScene {
                script,
                tilemap_cache,
                texture_paths,
                texture_lookup,
                assets,
                texture_handles,
                ..
            } = self;
            let mut previous = std::mem::take(tilemap_cache);
            let mut next_cache = HashMap::with_capacity(previous.len());
            let mut failure: Option<anyhow::Error> = None;
            let inherit = self.spatial_active;
            script.with_world(|world| {
                // WP2：瓦片层挂在**世界**位置上（继承开启时读传播结果，关闭时读局部值）。
                // 缓存比较用的也是同一个值，因此开关切换会让缓存自然失效、不会画在旧位置。
                let mut collect = |entity: EntityId, transform: &Transform, tilemap: &Tilemap| {
                    if failure.is_some() {
                        return;
                    }
                    let texture = texture_id_of(
                        &tilemap.texture,
                        assets,
                        texture_paths,
                        texture_lookup,
                        texture_handles,
                    );
                    let mut layer = match previous.remove(&entity) {
                        Some(layer)
                            if layer.tilemap == *tilemap
                                && layer.transform.scale == transform.scale =>
                        {
                            // 命中：瓦片与缩放都没变。`center_offset` 相对层原点，
                            // 与 position/rotation 无关——只同步 transform，不重建任何块。
                            layer
                        }
                        Some(mut layer)
                            if layer.tilemap.width == tilemap.width
                                && layer.tilemap.height == tilemap.height =>
                        {
                            // 未命中但**尺寸相同**：逐块比对 tiles 切片，只重建变化的块
                            // （scale 变化时脏重建会把每一块都判为变化，等于整层重建）。
                            let rebuilt =
                                rebuild_dirty_chunks(&mut layer, tilemap, transform.scale);
                            if rebuilt > 0 {
                                tracing::debug!(
                                    target: "scene",
                                    "tilemap chunk rebuild: {} chunk(s)",
                                    rebuilt
                                );
                            }
                            layer.tilemap = tilemap.clone();
                            layer
                        }
                        // 尺寸不同（室外 40×23 → 室内 20×12 换图）或首次出现：整层重建。
                        // 不能走 `rebuild_dirty_chunks`——它按旧层的 chunk 网格
                        // （`oy..oy+h`、`ox..ox+w` 可达旧图的 40×23）去索引**新图**的
                        // `tiles`（宽只有 20、len 240），会越界 panic。
                        _ => match build_all_chunks(*transform, tilemap.clone()) {
                            Ok(layer) => layer,
                            Err(error) => {
                                failure = Some(error);
                                return;
                            }
                        },
                    };
                    layer.transform = *transform;
                    // 预留本层顶点容量：`Vec::new()` 一路翻倍到 6 万顶点会额外搬上兆字节
                    // （`wp0-v4` tilemap 阶梯实测的每帧分配里，增长churn 占约 1.9 MB）。
                    if world_pass {
                        world_vertices.reserve(layer.total * 6);
                        world_draws.reserve(1);
                        append_tilemap_layer_world(
                            world_vertices,
                            world_draws,
                            texture,
                            &layer,
                            cull_bounds,
                        );
                    } else {
                        vertices.reserve(layer.total * 6);
                        overlay_draws.reserve(1);
                        append_tilemap_layer(
                            vertices,
                            overlay_draws,
                            texture,
                            camera,
                            &layer,
                            cull,
                        );
                    }
                    next_cache.insert(entity, layer);
                };
                if inherit {
                    world.for_each2::<GlobalTransform, Tilemap>(|entity, global, tilemap| {
                        if let Some(plane) = global.plane() {
                            collect(entity, &plane, tilemap);
                        }
                    });
                } else {
                    world.for_each2::<Transform, Tilemap>(|entity, transform, tilemap| {
                        collect(entity, transform, tilemap);
                    });
                }
            }); // 先提交已处理部分，再报错：出错帧不清空整张缓存，下一帧能继续复用。
            *tilemap_cache = next_cache;
            if let Some(message) = failure {
                return Err(message);
            }
        }
        Ok(())
    }
}
