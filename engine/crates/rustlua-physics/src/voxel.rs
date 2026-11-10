//! 逐格体素 → 轴对齐长方体集合（P3-R215）。
//!
//! 渲染侧的 `__cells__`（`rustlua-render::cellmesh`）已经把「位图只出外露面」这件事做完了，
//! 但物理侧拿到的仍然只是一张网格：Rapier 不会因为有一堆三角形就挡住一个动态体，
//! 而逐格一个 `Cuboid` 又会让碰撞体数量等于实心格数（Myrmex 的巢是 3.5 k 个）。
//! 这里做的是把同一张位图**贪心并块**：先在地表格子里拼出尽量大的矩形，再把上下层里
//!  footprint 完全相同的矩形竖着接起来。碰撞体的形状因此与渲染网格同源，但数量只看
//! 表面的「块状程度」，不看格数。

/// 一个并出来的长方体：局部格坐标的 `min`（含）与各轴格数（≥1）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoxelBox {
    pub min: [usize; 3],
    pub extent: [usize; 3],
}

/// 单轴最多多少格、总格数上限、格边长范围——与渲染侧 `CellMeshSpec` 的承诺同数，
/// 两侧不是同一份常数（物理不依赖渲染），改任何一边都要同步改另一边。
pub const MAX_AXIS: usize = 64;
pub const MAX_CELLS: usize = 32_768;
pub const MIN_CELL: f32 = 0.05;
pub const MAX_CELL: f32 = 64.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoxelError {
    Axis,
    Total,
    IndexOutOfRange(usize),
    Duplicate(usize),
}

/// 位图合法性：下标必须是非负、区域内、不重复的格。出界与重复都报错，不夹取——
/// 夹取等于挖到邻居（与 `cells3d.dig` 同一口径）。
pub fn validate(dims: [usize; 3], solids: &[usize]) -> Result<(), VoxelError> {
    if dims.iter().any(|axis| *axis == 0 || *axis > MAX_AXIS) {
        return Err(VoxelError::Axis);
    }
    let total = dims[0] * dims[1] * dims[2];
    if total > MAX_CELLS {
        return Err(VoxelError::Total);
    }
    let mut seen = vec![false; total];
    for &index in solids {
        if index >= total {
            return Err(VoxelError::IndexOutOfRange(index));
        }
        if seen[index] {
            return Err(VoxelError::Duplicate(index));
        }
        seen[index] = true;
    }
    Ok(())
}

/// 0 基平铺下标：`x + w * (z + d * y)`，与渲染侧 `CellMeshSpec::index` 同式
/// （`d` 是**深度** `dims[2]`，不是层数 `dims[1]`——写错时立方区域照样自洽，
/// 只有非立方区域才暴露，所以这条由 `flat_index_uses_depth_not_layer_count` 钉住）。
#[must_use]
pub fn flat_index(dims: [usize; 3], x: usize, y: usize, z: usize) -> usize {
    x + dims[0] * (z + dims[2] * y)
}

/// 把 `solids` 收成 `occupied` 位图后并块。要求 `validate` 已过。
#[must_use]
pub fn merge(dims: [usize; 3], solids: &[usize]) -> Vec<VoxelBox> {
    let total = dims[0] * dims[1] * dims[2];
    let mut occupied = vec![false; total];
    for &index in solids {
        occupied[index] = true;
    }
    let [w, h, d] = dims;
    let mut footprints: Vec<(usize, usize, usize, usize, usize)> = Vec::new();
    for y in 0..h {
        let mut used = vec![false; w * d];
        for z in 0..d {
            for x in 0..w {
                let here = flat_index(dims, x, y, z);
                if !occupied[here] || used[x + z * w] {
                    continue;
                }
                let mut xe = x;
                while xe + 1 < w
                    && occupied[flat_index(dims, xe + 1, y, z)]
                    && !used[xe + 1 + z * w]
                {
                    xe += 1;
                }
                let mut ze = z;
                'grow: while ze + 1 < d {
                    for xx in x..=xe {
                        let next = flat_index(dims, xx, y, ze + 1);
                        if !occupied[next] || used[xx + (ze + 1) * w] {
                            break 'grow;
                        }
                    }
                    ze += 1;
                }
                for zz in z..=ze {
                    for xx in x..=xe {
                        used[xx + zz * w] = true;
                    }
                }
                footprints.push((x, z, xe - x + 1, ze - z + 1, y));
            }
        }
    }
    // 竖接：footprint 相同且 y 连续的矩形合成一个更高的长方体。
    footprints.sort_by_key(|a| (a.0, a.1, a.2, a.3, a.4));
    let mut boxes: Vec<VoxelBox> = Vec::with_capacity(footprints.len());
    let mut open: Vec<(usize, usize)> = Vec::new();
    for (x, z, sx, sz, y) in footprints {
        let mut merged = false;
        if y > 0 {
            for slot in &mut open {
                let previous = boxes[slot.0];
                if previous.min[0] == x
                    && previous.min[2] == z
                    && previous.extent[0] == sx
                    && previous.extent[2] == sz
                    && slot.1 == y - 1
                {
                    boxes[slot.0] = VoxelBox {
                        min: previous.min,
                        extent: [sx, previous.extent[1] + 1, sz],
                    };
                    slot.1 = y;
                    merged = true;
                    break;
                }
            }
        }
        if !merged {
            boxes.push(VoxelBox {
                min: [x, y, z],
                extent: [sx, 1, sz],
            });
            open.push((boxes.len() - 1, y));
        }
    }
    boxes
}
