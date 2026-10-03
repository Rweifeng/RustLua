//! 原 `mod tilemap_chunk_tests`（WP12 阶段 1 纯搬运；源行段 5066–5345）。

use super::*;

/// 整层平铺参考实现：与 [`build_chunk`] 同公式，顺序 = 行主序（y 外层 x 内层）。
/// 它是 WP4 ④ 的"像素等价"证明的单元级替身——chunk 化后的展平序列必须逐项相等。
fn flat_sprites(tilemap: &Tilemap, scale: Vec2) -> Vec<CachedTileSprite> {
    let mut sprites = Vec::new();
    for y in 0..tilemap.height {
        for x in 0..tilemap.width {
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
    sprites
}

/// 块行主序展平（append 实际消费的顺序）。
fn flatten(layer: &CachedTilemapLayer) -> Vec<CachedTileSprite> {
    let mut out = Vec::new();
    for chunk in &layer.chunks {
        out.extend_from_slice(&chunk.sprites);
    }
    out
}

/// 覆盖完备性比较：按格坐标排序后逐项相等。块行主序 ≠ 整层行主序
/// （层宽 > 块宽时顺序不同），瓦片互不重叠 → 像素不变，由 2D golden 证明；
/// 单元级只验证"每个非空格恰好出现一次、内容正确"。
fn sorted(mut v: Vec<CachedTileSprite>) -> Vec<CachedTileSprite> {
    v.sort_by(|a, b| {
        (a.center_offset.y, a.center_offset.x, a.uv_min.x, a.uv_min.y)
            .partial_cmp(&(b.center_offset.y, b.center_offset.x, b.uv_min.x, b.uv_min.y))
            .unwrap()
    });
    v
}

/// 块内行主序属性：块内相邻 sprite 的格索引（y*W+x）严格递增。
fn sprite_index(s: &CachedTileSprite, tile_size: f32, scale: f32, map_width: usize) -> i64 {
    let x = (s.center_offset.x / (tile_size * scale) - 0.5).round() as i64;
    let y = (s.center_offset.y / (tile_size * scale) - 0.5).round() as i64;
    y * map_width as i64 + x
}
fn assert_chunk_rows_major(chunk: &TileChunk, map_width: usize) {
    if chunk.sprites.len() < 2 {
        return; // 空块/单格块无需窗口比较
    }
    let mut prev = sprite_index(&chunk.sprites[0], 16.0, 1.0, map_width);
    for s in &chunk.sprites[1..] {
        let cur = sprite_index(s, 16.0, 1.0, map_width);
        assert!(cur > prev, "chunk 内顺序必须行主序 (y 外层 x 内层)");
        prev = cur;
    }
}

/// 块行主序属性：chunks 整体按 (oy, ox) 字典序排列（append 消费顺序稳定）。
fn assert_chunks_block_major(layer: &CachedTilemapLayer) {
    for pair in layer.chunks.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        assert!((a.oy, a.ox) <= (b.oy, b.ox), "chunks 必须按块行主序排列");
    }
}

fn transform(position: Vec2, scale: Vec2) -> Transform {
    Transform {
        position,
        rotation: 0.0,
        scale,
    }
}

/// 确定性伪随机填充（1..=6，约 2/3 非空），避免测试依赖手写小地图。
fn fill(map: &mut Tilemap, seed: u32) {
    let mut state = seed;
    for y in 0..map.height {
        for x in 0..map.width {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            let v = ((state >> 16) % 9) as u8; // 0..=8
            if v > 0 && v <= 6 {
                map.set_cell(x, y, v).unwrap();
            }
        }
    }
}

fn make_map(width: usize, height: usize, seed: u32) -> Tilemap {
    let mut map = Tilemap::new(Vec2::new(16.0, 16.0), width, height).unwrap();
    fill(&mut map, seed);
    map
}

/// 覆盖块边界两侧的尺寸：小于一块、恰好一块、一块半、非 32 倍数、整块倍数。
#[test]
fn chunked_flat_order_matches_row_major_reference() {
    for (w, h) in [
        (1usize, 1usize),
        (3, 3),
        (32, 32),
        (33, 65),
        (40, 22),
        (100, 100),
    ] {
        let map = make_map(w, h, (w * 31 + h) as u32);
        let layer = build_all_chunks(
            transform(Vec2::new(7.0, -3.0), Vec2::new(1.0, 1.0)),
            map.clone(),
        )
        .unwrap();
        assert_eq!(
            sorted(flatten(&layer)),
            sorted(flat_sprites(&map, Vec2::new(1.0, 1.0))),
            "w={w} h={h}: 覆盖不完备或内容错误"
        );
        assert_eq!(
            layer.total,
            flatten(&layer).len(),
            "total mismatch w={w} h={h}"
        );
        for chunk in &layer.chunks {
            assert_chunk_rows_major(chunk, map.width);
        }
        assert_chunks_block_major(&layer);
    }
}

/// 非 32 倍数尺寸的块布局：33×65 → 2 列 × 3 行，边缘块不满。
#[test]
fn non_multiple_32_chunk_layout() {
    let map = make_map(33, 65, 7);
    let layer = build_all_chunks(Transform::identity(), map).unwrap();
    assert_eq!(layer.chunks.len(), 6);
    let got: Vec<(usize, usize, usize, usize)> = layer
        .chunks
        .iter()
        .map(|c| (c.ox, c.oy, c.w, c.h))
        .collect();
    assert_eq!(
        got,
        vec![
            (0, 0, 32, 32),
            (32, 0, 1, 32),
            (0, 32, 32, 32),
            (32, 32, 1, 32),
            (0, 64, 32, 1),
            (32, 64, 1, 1),
        ]
    );
}

/// 编辑一格 → 只重建它所在的 1 块；同块多格 → 仍 1 块；跨块两格 → 2 块；零改动 → 0 块。
#[test]
fn edit_rebuilds_only_dirty_chunk() {
    let mut map = make_map(40, 22, 9);
    let mut layer = build_all_chunks(
        transform(Vec2::new(0.0, 0.0), Vec2::new(1.0, 1.0)),
        map.clone(),
    )
    .unwrap();

    // 零改动。
    assert_eq!(
        rebuild_dirty_chunks(&mut layer, &map, Vec2::new(1.0, 1.0)),
        0
    );

    // 改一格（块 (0,0) 内）。
    map.set_cell(3, 5, 4).unwrap();
    assert_eq!(
        rebuild_dirty_chunks(&mut layer, &map, Vec2::new(1.0, 1.0)),
        1
    );

    // 同块再改一格 → 仍 1 块。
    map.set_cell(7, 9, 2).unwrap();
    assert_eq!(
        rebuild_dirty_chunks(&mut layer, &map, Vec2::new(1.0, 1.0)),
        1
    );

    // 跨块两格（(3,3) 与 (35,5) 分属两列两块）→ 2 块。
    map.set_cell(3, 3, 5).unwrap();
    map.set_cell(35, 5, 6).unwrap();
    assert_eq!(
        rebuild_dirty_chunks(&mut layer, &map, Vec2::new(1.0, 1.0)),
        2
    );

    // 覆盖仍然完备。
    assert_eq!(
        sorted(flatten(&layer)),
        sorted(flat_sprites(&map, Vec2::new(1.0, 1.0)))
    );
}

/// position/rotation 变化不触发任何块重建（center_offset 相对层原点）。
#[test]
fn position_rotation_change_keeps_chunks() {
    let map = make_map(40, 22, 11);
    let mut layer = build_all_chunks(
        transform(Vec2::new(1.0, 2.0), Vec2::new(1.0, 1.0)),
        map.clone(),
    )
    .unwrap();
    let before = flatten(&layer);
    layer.transform = transform(Vec2::new(-9.0, 42.0), Vec2::new(1.0, 1.0));
    layer.transform.rotation = 1.25;
    assert_eq!(
        rebuild_dirty_chunks(&mut layer, &map, Vec2::new(1.0, 1.0)),
        0
    );
    assert_eq!(flatten(&layer), before); // 零重建 → 序列逐项原样
}

/// scale 变化 → 所有块重建（center_offset 预乘 scale）。
#[test]
fn scale_change_rebuilds_all_chunks() {
    let map = make_map(40, 22, 13);
    let mut layer = build_all_chunks(
        transform(Vec2::new(0.0, 0.0), Vec2::new(1.0, 1.0)),
        map.clone(),
    )
    .unwrap();
    let chunk_count = layer.chunks.len();
    let rebuilt = rebuild_dirty_chunks(&mut layer, &map, Vec2::new(2.0, 2.0));
    assert_eq!(rebuilt, chunk_count);
    assert_eq!(
        sorted(flatten(&layer)),
        sorted(flat_sprites(&map, Vec2::new(2.0, 2.0)))
    );
}

/// solid/meta 变化不触发重建：它们不参与 sprite 生成，像素不变。
#[test]
fn solid_and_meta_change_do_not_rebuild() {
    let mut map = make_map(40, 22, 17);
    let mut layer = build_all_chunks(Transform::identity(), map.clone()).unwrap();
    map.set_solid(3, 5, true).unwrap();
    map.meta.insert(
        (1, 1),
        BTreeMap::from([("name".to_owned(), "door".to_owned())]),
    );
    assert_eq!(
        rebuild_dirty_chunks(&mut layer, &map, Vec2::new(1.0, 1.0)),
        0
    );
}

/// 1000×1000 量级：改一格只重建 1 块（O(块) 而非 O(100 万格)）——
/// 这是 WP4 ④ 触发条件（1000×1000 真实地图）未出现时，结构性证明编辑代价受块界约束。
#[test]
fn million_cell_edit_is_chunk_bounded() {
    let mut map = make_map(1000, 1000, 23);
    let layer = build_all_chunks(Transform::identity(), map.clone()).unwrap();
    // 1000×1000 → 32×32 块满铺：ceil(1000/32)=32 列 × 32 行（边缘 1000=32*31+8）。
    assert_eq!(layer.chunks.len(), 32 * 32);
    assert_eq!(layer.total, flat_sprites(&map, Vec2::new(1.0, 1.0)).len());
    let mut dirty = layer;
    // 选一个与现值不同的值，保证确实发生编辑。
    let original = map.cell(999, 999).unwrap();
    let target = if original == 5 { 6 } else { 5 };
    map.set_cell(999, 999, target).unwrap();
    let rebuilt = rebuild_dirty_chunks(&mut dirty, &map, Vec2::new(1.0, 1.0));
    assert_eq!(rebuilt, 1, "编辑代价必须受块界约束：只重建 1 块而非全层");
    assert_eq!(
        sorted(flatten(&dirty)),
        sorted(flat_sprites(&map, Vec2::new(1.0, 1.0))),
        "覆盖不完备"
    );
}
