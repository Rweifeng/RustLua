//! M19-03 Tilemap 生产工具的纯逻辑：矩形填充与区域复制/粘贴。
//!
//! 本模块**不依赖 egui**，只负责矩形归一化、越界裁剪与数据生成，便于单元
//! 测试；真正的写回由 `scene_view` 走 `CommandHistory`（一次操作 = 单条 Undo）。
//!
//! 坐标约定与 `Tilemap`/农田一致：0-index、`y` 朝上（row 0 在地图最底），
//! 索引 = `y * width + x`。

use engine::core::tilemap::Tilemap;

/// 归一化的格矩形（**含**边界；`min <= max` 恒成立）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CellRect {
    pub min_x: usize,
    pub min_y: usize,
    pub max_x: usize,
    pub max_y: usize,
}

impl CellRect {
    /// 由任意两个角点构造（顺序无关：反向拖拽得到同一矩形）。
    pub(crate) fn normalize(a: (usize, usize), b: (usize, usize)) -> Self {
        Self {
            min_x: a.0.min(b.0),
            min_y: a.1.min(b.1),
            max_x: a.0.max(b.0),
            max_y: a.1.max(b.1),
        }
    }

    pub(crate) fn width(self) -> usize {
        self.max_x - self.min_x + 1
    }

    pub(crate) fn height(self) -> usize {
        self.max_y - self.min_y + 1
    }

    pub(crate) fn cell_count(self) -> usize {
        self.width() * self.height()
    }

    /// 裁剪到 `width × height` 的网格；完全越界时返回 `None`。
    pub(crate) fn clipped(self, width: usize, height: usize) -> Option<Self> {
        if width == 0 || height == 0 || self.min_x >= width || self.min_y >= height {
            return None;
        }
        Some(Self {
            min_x: self.min_x,
            min_y: self.min_y,
            max_x: self.max_x.min(width - 1),
            max_y: self.max_y.min(height - 1),
        })
    }
}

/// 一次矩形填充的写入目标：`None` 表示不改该项。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TileFill {
    /// 写入的 tile 值（`0` = 空；画笔值必须先过 [`clamp_brush`]）。
    pub value: Option<u8>,
    /// 写入的 solid 位。
    pub solid: Option<bool>,
}

/// 画笔值的上界由**地图的图集布局**决定（M72-01/02）：
///
/// * 直接寻址（没开 autotile、`variant_count == 1`）→ 格子里存的就是图集条目号，
///   上界 = `atlas_cols × atlas_rows`；
/// * 逻辑瓦模式（开了 autotile 或变体）→ 格子里存的是「逻辑瓦 id」（草/土/石…），
///   上界 = `tile_kinds`。
///
/// 旧实现把上界写死成 `6`，那是 `TILE_PALETTE` 充当列数真源时代的产物；现在它是
/// 「美术只能刷前 6 列第一行」的隐性牢笼，而且**会把 autotile 的 kind id 悄悄截掉**
/// （刷第 7 号逻辑瓦变成 6 号 = 错误的地形）。
#[must_use]
pub(crate) fn max_brush(tilemap: &Tilemap) -> u8 {
    tilemap.max_tile_id().min(u8::MAX as usize) as u8
}

/// 把画笔值收敛到当前布局允许的范围。
#[must_use]
pub(crate) fn clamp_brush(tilemap: &Tilemap, value: u8) -> u8 {
    value.min(max_brush(tilemap))
}

/// 槽位给美术看的标签。逻辑瓦模式必须说清「这是 kind，不是图集格子」——否则美术会
/// 以为自己在选第 N 个子图，转而手工摆变体（M72-02 §风险 3）。
#[must_use]
pub(crate) fn brush_label(tilemap: &Tilemap, value: u8) -> String {
    if value == 0 {
        return "×".to_owned();
    }
    if tilemap.autotile || tilemap.variant_count > 1 {
        format!("瓦 {value}")
    } else {
        // 直接寻址：告诉美术这一格落在图集的第几列第几行（0-index，与图集绘制一致）。
        let entry = value as usize - 1;
        format!(
            "{value}·{}:{}",
            entry % tilemap.atlas_cols,
            entry / tilemap.atlas_cols
        )
    }
}

/// 没有 tileset 可显示时的调试色（槽位 > 6 走确定性色相，不再退回旧代码的「一律 6 号色」）。
#[must_use]
pub(crate) fn debug_swatch(value: u8) -> [f32; 4] {
    let index = value as usize;
    if index >= 1 && index <= engine::core::tilemap::TILE_PALETTE.len() {
        return engine::core::tilemap::TILE_PALETTE[index - 1];
    }
    // 黄金角色相轮：相邻槽位色差最大，肉眼可分；alpha 恒 1（贴图缺失时才用）。
    let hue = (index as f32 * 0.618_034) % 1.0;
    let (r, g, b) = hsv_to_rgb(hue, 0.55, 0.85);
    [r, g, b, 1.0]
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let i = (h * 6.0) as usize;
    let f = h * 6.0 - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    match i % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    }
}

/// 矩形填充：把裁剪后的矩形内所有格写成目标值。
///
/// 返回是否产生**净改动**（`false` = 越界/空矩形/全部已是目标值），调用方据此
/// 决定是否提交历史（避免空 Undo）。
pub(crate) fn fill_rect(tilemap: &mut Tilemap, rect: CellRect, fill: TileFill) -> bool {
    let Some(rect) = rect.clipped(tilemap.width, tilemap.height) else {
        return false;
    };
    let mut changed = false;
    for y in rect.min_y..=rect.max_y {
        for x in rect.min_x..=rect.max_x {
            let index = y * tilemap.width + x;
            if let Some(value) = fill.value
                && tilemap.tiles[index] != value
            {
                tilemap.tiles[index] = value;
                changed = true;
            }
            if let Some(solid) = fill.solid
                && tilemap.solid[index] != solid
            {
                tilemap.solid[index] = solid;
                changed = true;
            }
        }
    }
    changed
}

/// 复制出的区域内容（行优先，索引 = `y * width + x`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TileRegion {
    pub width: usize,
    pub height: usize,
    pub tiles: Vec<u8>,
    pub solid: Vec<bool>,
}

impl TileRegion {
    pub(crate) fn label(&self) -> String {
        format!("{}×{}", self.width, self.height)
    }
}

/// 复制矩形内的 tile 值与 solid 位（矩形按网格裁剪）。空矩形返回 `None`。
pub(crate) fn copy_region(tilemap: &Tilemap, rect: CellRect) -> Option<TileRegion> {
    let rect = rect.clipped(tilemap.width, tilemap.height)?;
    let width = rect.width();
    let height = rect.height();
    let mut tiles = Vec::with_capacity(width * height);
    let mut solid = Vec::with_capacity(width * height);
    for y in rect.min_y..=rect.max_y {
        for x in rect.min_x..=rect.max_x {
            let index = y * tilemap.width + x;
            tiles.push(tilemap.tiles[index]);
            solid.push(tilemap.solid[index]);
        }
    }
    Some(TileRegion {
        width,
        height,
        tiles,
        solid,
    })
}

/// 粘贴区域：`origin` 为区域左上角（允许负值，越界部分按网格裁剪）。
///
/// 返回是否产生净改动（全部越界/内容相同 → `false`）。
pub(crate) fn paste_region(
    tilemap: &mut Tilemap,
    region: &TileRegion,
    origin_x: isize,
    origin_y: isize,
) -> bool {
    let mut changed = false;
    for y in 0..region.height {
        let Some(target_y) = offset_index(origin_y, y, tilemap.height) else {
            continue;
        };
        for x in 0..region.width {
            let Some(target_x) = offset_index(origin_x, x, tilemap.width) else {
                continue;
            };
            let source = y * region.width + x;
            let target = target_y * tilemap.width + target_x;
            if tilemap.tiles[target] != region.tiles[source] {
                tilemap.tiles[target] = region.tiles[source];
                changed = true;
            }
            if tilemap.solid[target] != region.solid[source] {
                tilemap.solid[target] = region.solid[source];
                changed = true;
            }
        }
    }
    changed
}

/// 区域坐标 + 偏移 → 网格内索引；越界返回 `None`。
fn offset_index(origin: isize, offset: usize, limit: usize) -> Option<usize> {
    let value = origin.checked_add(offset as isize)?;
    if value < 0 || value as usize >= limit {
        return None;
    }
    Some(value as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::core::transform::Vec2;

    fn map(width: usize, height: usize) -> Tilemap {
        Tilemap::new(Vec2::new(32.0, 32.0), width, height).expect("tilemap")
    }

    #[test]
    fn normalize_handles_reverse_drag_and_single_cell() {
        let forward = CellRect::normalize((1, 2), (4, 5));
        let reverse = CellRect::normalize((4, 5), (1, 2));
        assert_eq!(forward, reverse, "反向拖拽必须归一化到同一矩形");
        assert_eq!((forward.width(), forward.height()), (4, 4));
        assert_eq!(CellRect::normalize((3, 3), (3, 3)).cell_count(), 1);
        assert_eq!(CellRect::normalize((7, 7), (7, 7)).cell_count(), 1);
    }

    #[test]
    fn fill_rect_clips_out_of_bounds_and_reports_noop() {
        let mut tilemap = map(4, 3);
        // 矩形从 (2,1) 伸到 (9,9)：应裁剪到网格内 (2..3, 1..2)。
        let rect = CellRect::normalize((2, 1), (9, 9));
        assert!(fill_rect(
            &mut tilemap,
            rect,
            TileFill {
                value: Some(5),
                solid: Some(true),
            }
        ));
        let width = tilemap.width;
        let index = |x: usize, y: usize| y * width + x;
        assert_eq!(tilemap.tiles[index(2, 1)], 5);
        assert_eq!(tilemap.tiles[index(3, 2)], 5);
        assert!(tilemap.solid[index(3, 2)]);
        // 未覆盖的格子保持不变。
        assert_eq!(tilemap.tiles[0], 0);
        assert!(!tilemap.solid[0]);
        // 再次写入相同值 → 无净改动。
        assert!(!fill_rect(
            &mut tilemap,
            rect,
            TileFill {
                value: Some(5),
                solid: Some(true),
            }
        ));
        // 完全越界 → 无改动。
        assert!(!fill_rect(
            &mut tilemap,
            CellRect::normalize((8, 8), (9, 9)),
            TileFill {
                value: Some(1),
                solid: None,
            }
        ));
    }

    #[test]
    fn fill_rect_can_target_tiles_solid_or_both_and_erase() {
        let mut tilemap = map(2, 2);
        fill_rect(
            &mut tilemap,
            CellRect::normalize((0, 0), (1, 1)),
            TileFill {
                value: Some(3),
                solid: None,
            },
        );
        assert!(tilemap.tiles.iter().all(|value| *value == 3));
        assert!(
            tilemap.solid.iter().all(|value| !*value),
            "solid 位未被要求"
        );
        fill_rect(
            &mut tilemap,
            CellRect::normalize((0, 0), (1, 1)),
            TileFill {
                value: None,
                solid: Some(true),
            },
        );
        assert!(tilemap.solid.iter().all(|value| *value));
        // 擦除：tile 值 0 + solid false。
        fill_rect(
            &mut tilemap,
            CellRect::normalize((0, 0), (1, 1)),
            TileFill {
                value: Some(0),
                solid: Some(false),
            },
        );
        assert!(tilemap.tiles.iter().all(|value| *value == 0));
        assert!(tilemap.solid.iter().all(|value| !*value));
    }

    #[test]
    fn copy_region_reads_values_and_solid_bits() {
        let mut tilemap = map(4, 4);
        fill_rect(
            &mut tilemap,
            CellRect::normalize((1, 1), (2, 2)),
            TileFill {
                value: Some(4),
                solid: Some(true),
            },
        );
        let region = copy_region(&tilemap, CellRect::normalize((1, 1), (2, 2))).expect("region");
        assert_eq!((region.width, region.height), (2, 2));
        assert_eq!(region.label(), "2×2");
        assert!(region.tiles.iter().all(|value| *value == 4));
        assert!(region.solid.iter().all(|value| *value));
        // 越界复制按网格裁剪，完全越界返回 None。
        let clipped = copy_region(&tilemap, CellRect::normalize((3, 3), (9, 9))).expect("clipped");
        assert_eq!((clipped.width, clipped.height), (1, 1));
        assert!(copy_region(&tilemap, CellRect::normalize((8, 8), (9, 9))).is_none());
    }

    #[test]
    fn paste_region_clips_overflow_and_is_a_noop_when_identical() {
        let mut source = map(2, 2);
        fill_rect(
            &mut source,
            CellRect::normalize((0, 0), (1, 1)),
            TileFill {
                value: Some(6),
                solid: Some(true),
            },
        );
        let region = copy_region(&source, CellRect::normalize((0, 0), (1, 1))).expect("region");

        let mut target = map(4, 4);
        // 粘贴到 (3,3)：仅原区左上角落在网格内，其余溢出被裁剪。
        assert!(paste_region(&mut target, &region, 3, 3));
        assert_eq!(target.tiles[3 * 4 + 3], 6);
        assert!(target.solid[3 * 4 + 3]);
        assert_eq!(target.tiles[0], 0, "越界部分不得写入");
        // 相同内容再贴 → 无净改动。
        assert!(!paste_region(&mut target, &region, 3, 3));
        // 负原点同样按裁剪处理（只写入落在网格内的行/列）。
        let mut negative = map(4, 4);
        assert!(paste_region(&mut negative, &region, -1, -1));
        let written: Vec<usize> = negative
            .tiles
            .iter()
            .enumerate()
            .filter(|(_, value)| **value == 6)
            .map(|(index, _)| index)
            .collect();
        assert_eq!(written, vec![0], "只有区域右下角落在 (0,0)");
        // 完全越界 → 无改动。
        assert!(!paste_region(&mut negative, &region, 9, 9));
        assert!(!paste_region(&mut negative, &region, -9, -9));
    }

    #[test]
    fn copy_then_paste_round_trips_a_region() {
        let mut tilemap = map(5, 5);
        fill_rect(
            &mut tilemap,
            CellRect::normalize((0, 0), (1, 2)),
            TileFill {
                value: Some(2),
                solid: Some(false),
            },
        );
        fill_rect(
            &mut tilemap,
            CellRect::normalize((1, 2), (1, 2)),
            TileFill {
                value: Some(5),
                solid: Some(true),
            },
        );
        let before = copy_region(&tilemap, CellRect::normalize((0, 0), (1, 2))).expect("region");
        assert!(paste_region(&mut tilemap, &before, 3, 2));
        let after = copy_region(&tilemap, CellRect::normalize((3, 2), (4, 4))).expect("pasted");
        assert_eq!(before, after, "粘贴结果应与复制内容逐格一致");
    }

    #[test]
    fn brush_bounds_follow_the_atlas_layout_not_the_six_color_palette() {
        // 旧实现把上界写死 6（= TILE_PALETTE 的色数）。M72-01 后 12×8 图集能寻址 96 格，
        // 再 clamp 到 6 等于把新图集的第 2 行以后全锁死在编辑器里。
        let mut tilemap = map(4, 4);
        tilemap.atlas_cols = 12;
        tilemap.atlas_rows = 8;
        assert_eq!(max_brush(&tilemap), 96);
        assert_eq!(max_brush(&tilemap) as usize + 1, 97);
        assert_eq!(clamp_brush(&tilemap, 200), 96);
        // 越界请求按布局收敛，矩形填充真的能把第 2 行以后的条目刷进去。
        let painted = clamp_brush(&tilemap, 97);
        assert!(fill_rect(
            &mut tilemap,
            CellRect::normalize((0, 0), (0, 0)),
            TileFill {
                value: Some(painted),
                solid: None,
            }
        ));
        assert_eq!(tilemap.tiles[0], 96);
    }

    #[test]
    fn brush_is_bounded_by_kind_count_in_logical_tile_mode() {
        // autotile/变体模式下格子存 kind，画笔必须只到 kind 数：刷到图集条目号会
        // 让美术绕过 autotile 手工摆变体（M72-02 §风险 3），这里把牢笼换成护栏。
        let mut tilemap = map(4, 4);
        tilemap.atlas_cols = 16;
        tilemap.atlas_rows = 8;
        tilemap.tile_kinds = 5;
        tilemap.variant_count = 2;
        tilemap.autotile = true;
        tilemap.validate_layout().expect("layout");
        assert_eq!(max_brush(&tilemap), 5);
        assert_eq!(clamp_brush(&tilemap, 12), 5);
        // 越界 kind 写不进地图（引擎侧同一道上界）。
        assert!(tilemap.set_cell(0, 0, 6).is_err());
    }

    #[test]
    fn brush_labels_distinguish_kind_from_atlas_entry() {
        let mut direct = map(2, 2);
        direct.atlas_cols = 12;
        direct.atlas_rows = 8;
        // 直接寻址：标签带上图集列/行，美术能对回贴图里的位置（0-index）。
        assert_eq!(brush_label(&direct, 1), "1·0:0");
        assert_eq!(brush_label(&direct, 13), "13·0:1");
        assert_eq!(brush_label(&direct, 0), "×");

        let mut logical = map(2, 2);
        logical.atlas_cols = 16;
        logical.atlas_rows = 4;
        logical.tile_kinds = 3;
        logical.autotile = true;
        logical.validate_layout().expect("layout");
        assert_eq!(brush_label(&logical, 2), "瓦 2");
    }

    #[test]
    fn debug_swatch_covers_slots_beyond_the_legacy_palette() {
        // 前 6 槽仍是 TILE_PALETTE 的旧色（迁移后的老图在编辑器里颜色不变）。
        assert_eq!(debug_swatch(1), engine::core::tilemap::TILE_PALETTE[0]);
        assert_eq!(debug_swatch(6), engine::core::tilemap::TILE_PALETTE[5]);
        // 第 7 槽起不再是"一律 6 号色"：相邻槽位必须给出可区分的颜色。
        assert_ne!(debug_swatch(7), debug_swatch(8));
        for slot in [0_u8, 7, 42, 96, 255] {
            let color = debug_swatch(slot);
            assert!(
                color.iter().all(|value| (0.0..=1.0).contains(value)),
                "槽位 {slot} 的颜色分量越界：{color:?}"
            );
        }
    }
}
