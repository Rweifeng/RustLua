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
    /// 写入的 tile 值（`0` = 空；画笔值应已按调色板上界截断）。
    pub value: Option<u8>,
    /// 写入的 solid 位。
    pub solid: Option<bool>,
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
}
