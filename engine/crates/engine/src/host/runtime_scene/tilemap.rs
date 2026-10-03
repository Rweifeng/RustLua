//! 瓦片映射的顶点构建与 chunk 级脏重建（WP12 阶段 2 从 `runtime_scene.rs` 原样搬出）。
//!
//! 可见性依据（`tasks/plan/WP12-host拆分.md` §3.1.4）：这里只有被 `RuntimeScene` 的每帧路径调用的
//! 入口标 `pub(super)`（私有域 = `runtime_scene`，父与全部兄弟子模块可用）；`build_chunk` 只被
//! 本文件的分块构建调用，保持私有。类型 `CachedTilemapLayer` / `CachedTileSprite` / `TileChunk`
//! 仍定义在 `mod.rs`（它们被 `RuntimeScene` 的私有字段持有），本文件靠 `use super::{...}` 取用。

use crate::core::{
    camera::{CameraBounds, OrthoTerms},
    sprite_batch::{OverlayVertexSink, SpriteDrawBatch, WorldSpriteVertexSink, ndc_from_screen},
    tilemap::Tilemap,
    transform::{Transform, Vec2},
};
use crate::host::frame::WorldSpriteVertex;

use super::{CachedTileSprite, CachedTilemapLayer, TileChunk};

/// WP4 ④ chunk 级脏重建：瓦片块边长（格数）。块行主序拼接 == 原整层行主序。
pub(super) const TILE_CHUNK_SIZE: usize = 32;

/// 构建一个块（`ox/oy` = 块左上角格坐标，`w/h` = 实际块尺寸，边缘块可能不满）。
/// 与整层平铺构建的唯一区别是范围限定在块内；`center_offset` 语义不变
/// （相对层原点、预乘 `scale`——因此 `scale` 变化必须整层重建，见 [`rebuild_dirty_chunks`]）。
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
            let cell = tilemap.tiles[y * tilemap.width + x];
            if cell == 0 {
                continue;
            }
            let (uv_min, uv_max) = Tilemap::tile_uv(cell);
            let center_offset = Vec2::new(
                (x as f32 + 0.5) * tilemap.tile_size.x * scale.x,
                (y as f32 + 0.5) * tilemap.tile_size.y * scale.y,
            );
            sprites.push(CachedTileSprite {
                center_offset,
                uv_min,
                uv_max,
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

/// 逐块脏重建：只重建 `tiles` 切片变化的块；`scale` 变化时所有块都要重建
/// （`center_offset` 预乘了 scale）。返回重建的块数。`solid`/`meta` 变化不触发
/// 重建——它们不参与 sprite 生成，像素结果不变（由 2D golden 像素等价兜底）。
pub(super) fn rebuild_dirty_chunks(
    layer: &mut CachedTilemapLayer,
    tilemap: &Tilemap,
    scale: Vec2,
) -> usize {
    let old = &layer.tilemap;
    let mut rebuilt = 0usize;
    let mut total = 0usize;
    for chunk in &mut layer.chunks {
        let scale_changed = layer.transform.scale != scale;
        // 逐行段比较：row-major 的 	iles 里块区域**不是连续切片**（块宽 < 层宽时
        // 跨行），按块内每行 [y*width+ox, y*width+ox+w) 分段比对才覆盖真实区域。
        let mut tiles_changed = false;
        for y in chunk.oy..(chunk.oy + chunk.h) {
            if old.tiles[y * old.width + chunk.ox..y * old.width + chunk.ox + chunk.w]
                != tilemap.tiles
                    [y * tilemap.width + chunk.ox..y * tilemap.width + chunk.ox + chunk.w]
            {
                tiles_changed = true;
                break;
            }
        }
        if scale_changed || tiles_changed {
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
            let tint = [1.0; 4];
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
            let tint = [1.0; 4];
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
