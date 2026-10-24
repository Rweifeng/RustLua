//! M76-05: 切图与九宫格区域。
//!
//! 全部是纯几何：输入尺寸/RGBA，输出区域清单。产物名字稳定、顺序稳定（按 y 再按 x），
//! 因此同一张图重跑得到同样的区域清单与文件名，回放/golden 可以直接比对。

use std::collections::BTreeSet;
use std::fmt;

/// 一个切图区域：`[x, y, width, height]`，像素、左上原点。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SliceRegion {
    pub name: String,
    pub rect: [u32; 4],
}

impl SliceRegion {
    #[must_use]
    pub fn new(name: impl Into<String>, rect: [u32; 4]) -> Self {
        Self {
            name: name.into(),
            rect,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SliceError {
    EmptyImage,
    ZeroCells,
    EmptyName,
    DuplicateName(String),
    OutOfBounds {
        name: String,
        rect: [u32; 4],
        width: u32,
        height: u32,
    },
    SizeMismatch {
        expected: usize,
        actual: usize,
    },
}

impl fmt::Display for SliceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyImage => write!(formatter, "图片尺寸不能为 0"),
            Self::ZeroCells => write!(formatter, "切图行列数至少为 1"),
            Self::EmptyName => write!(formatter, "切图区域必须有名字（它是引用键）"),
            Self::DuplicateName(name) => write!(formatter, "切图区域名字重复：{name}"),
            Self::OutOfBounds {
                name,
                rect,
                width,
                height,
            } => write!(formatter, "区域 {name} {:?} 超出 {width}x{height}", rect),
            Self::SizeMismatch { expected, actual } => write!(
                formatter,
                "RGBA 缓冲长度不符：期望 {expected} 字节，实际 {actual}"
            ),
        }
    }
}

impl std::error::Error for SliceError {}

/// 等分/网格切图。剩下的像素归最后一列/行，避免整除丢边。
pub fn slice_grid(
    width: u32,
    height: u32,
    columns: u32,
    rows: u32,
    prefix: &str,
) -> Result<Vec<SliceRegion>, SliceError> {
    if width == 0 || height == 0 {
        return Err(SliceError::EmptyImage);
    }
    if columns == 0 || rows == 0 {
        return Err(SliceError::ZeroCells);
    }
    if prefix.trim().is_empty() {
        return Err(SliceError::EmptyName);
    }
    let columns = columns.min(width);
    let rows = rows.min(height);
    let mut regions = Vec::with_capacity((columns * rows) as usize);
    for row in 0..rows {
        let y0 = row * height / rows;
        let y1 = (row + 1) * height / rows;
        for column in 0..columns {
            let x0 = column * width / columns;
            let x1 = (column + 1) * width / columns;
            regions.push(SliceRegion::new(
                format!("{prefix}_{column}_{row}"),
                [x0, y0, x1 - x0, y1 - y0],
            ));
        }
    }
    Ok(regions)
}

/// 手框切图：名字与矩形都由调用方给，只做边界和重名校验。
pub fn slice_manual(
    width: u32,
    height: u32,
    regions: Vec<SliceRegion>,
) -> Result<Vec<SliceRegion>, SliceError> {
    if width == 0 || height == 0 {
        return Err(SliceError::EmptyImage);
    }
    let mut seen = BTreeSet::new();
    for region in &regions {
        if region.name.trim().is_empty() {
            return Err(SliceError::EmptyName);
        }
        if !seen.insert(region.name.clone()) {
            return Err(SliceError::DuplicateName(region.name.clone()));
        }
        let [x, y, w, h] = region.rect;
        if w == 0 || h == 0 || x + w > width || y + h > height {
            return Err(SliceError::OutOfBounds {
                name: region.name.clone(),
                rect: region.rect,
                width,
                height,
            });
        }
    }
    Ok(regions)
}

/// 按区域从整张 RGBA 缓冲裁出像素，顺序与 `regions` 一致。
///
/// 这是切图落盘的纯数据半边：宿主只负责读/写 PNG，裁像素的逻辑留在这里，
/// 保证「屏幕上量到的区域」与「写出去的文件」用同一套边界规则。
pub fn slice_rgba(
    width: u32,
    height: u32,
    rgba: &[u8],
    regions: &[SliceRegion],
) -> Result<Vec<Vec<u8>>, SliceError> {
    if width == 0 || height == 0 {
        return Err(SliceError::EmptyImage);
    }
    let expected = width as usize * height as usize * 4;
    if rgba.len() != expected {
        return Err(SliceError::SizeMismatch {
            expected,
            actual: rgba.len(),
        });
    }
    let regions = slice_manual(width, height, regions.to_vec())?;
    let width = width as usize;
    Ok(regions
        .iter()
        .map(|region| {
            let [x, y, w, h] = region.rect;
            let row_bytes = w as usize * 4;
            let mut out = Vec::with_capacity(row_bytes * h as usize);
            for row in 0..h as usize {
                let start = (((y as usize + row) * width) + x as usize) * 4;
                out.extend_from_slice(&rgba[start..start + row_bytes]);
            }
            out
        })
        .collect())
}

/// 透明边界切图：按 alpha>0 的 4 邻接连通块切，`padding` 向外扩并夹在图片内。
///
/// 非透明像素一律算内容（不做颜色容差），因此结果与图片内容一一对应；输入抖动不会
/// 改变区域数量。块顺序固定为「最小 y、再最小 x」。
pub fn slice_alpha(
    rgba: &[u8],
    width: u32,
    height: u32,
    padding: u32,
) -> Result<Vec<SliceRegion>, SliceError> {
    if width == 0 || height == 0 {
        return Err(SliceError::EmptyImage);
    }
    let expected = width as usize * height as usize * 4;
    if rgba.len() != expected {
        return Err(SliceError::SizeMismatch {
            expected,
            actual: rgba.len(),
        });
    }
    let w = width as usize;
    let h = height as usize;
    let opaque = |x: usize, y: usize| rgba[(y * w + x) * 4 + 3] > 0;
    let mut visited = vec![false; w * h];
    let mut regions = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if visited[y * w + x] || !opaque(x, y) {
                continue;
            }
            let mut stack = vec![(x, y)];
            visited[y * w + x] = true;
            let (mut min_x, mut min_y, mut max_x, mut max_y) = (x, y, x, y);
            while let Some((cx, cy)) = stack.pop() {
                min_x = min_x.min(cx);
                min_y = min_y.min(cy);
                max_x = max_x.max(cx);
                max_y = max_y.max(cy);
                let neighbours = [
                    (cx.wrapping_sub(1), cy),
                    (cx + 1, cy),
                    (cx, cy.wrapping_sub(1)),
                    (cx, cy + 1),
                ];
                for (nx, ny) in neighbours {
                    if nx < w && ny < h && !visited[ny * w + nx] && opaque(nx, ny) {
                        visited[ny * w + nx] = true;
                        stack.push((nx, ny));
                    }
                }
            }
            let pad = padding as usize;
            let x0 = min_x.saturating_sub(pad);
            let y0 = min_y.saturating_sub(pad);
            let x1 = (max_x + 1 + pad).min(w);
            let y1 = (max_y + 1 + pad).min(h);
            regions.push(SliceRegion::new(
                format!("region_{}", regions.len()),
                [x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32],
            ));
        }
    }
    regions.sort_by_key(|region| (region.rect[1], region.rect[0]));
    Ok(regions)
}

/// 九宫格的 9 个区域（四角、四边、中心），用于从一张大图切出可伸缩片段。
pub fn nine_slice_regions(
    width: u32,
    height: u32,
    margins: [u32; 4],
) -> Result<Vec<SliceRegion>, SliceError> {
    let [left, right, top, bottom] = margins;
    if width == 0 || height == 0 {
        return Err(SliceError::EmptyImage);
    }
    if left + right >= width || top + bottom >= height {
        return Err(SliceError::OutOfBounds {
            name: "nine_slice".to_owned(),
            rect: [left, top, width, height],
            width,
            height,
        });
    }
    let xs = [0, left, width - right];
    let ws = [left, width - left - right, right];
    let ys = [0, top, height - bottom];
    let hs = [top, height - top - bottom, bottom];
    let names = [
        ["top_left", "top", "top_right"],
        ["left", "center", "right"],
        ["bottom_left", "bottom", "bottom_right"],
    ];
    let mut regions = Vec::with_capacity(9);
    for row in 0..3 {
        for column in 0..3 {
            regions.push(SliceRegion::new(
                names[row][column],
                [xs[column], ys[row], ws[column], hs[row]],
            ));
        }
    }
    Ok(regions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_keeps_remainder_on_the_last_cell() {
        let regions = slice_grid(10, 7, 3, 2, "icon").unwrap();
        assert_eq!(regions.len(), 6);
        assert_eq!(regions[0].rect, [0, 0, 3, 3]);
        assert_eq!(regions[2].rect, [6, 0, 4, 3]);
        assert_eq!(regions[5].rect, [6, 3, 4, 4]);
    }

    #[test]
    fn manual_regions_must_fit_and_have_unique_names() {
        let ok = slice_manual(
            16,
            16,
            vec![
                SliceRegion::new("a", [0, 0, 8, 8]),
                SliceRegion::new("b", [8, 8, 8, 8]),
            ],
        )
        .unwrap();
        assert_eq!(ok.len(), 2);
        assert!(matches!(
            slice_manual(16, 16, vec![SliceRegion::new("a", [12, 0, 8, 8])]),
            Err(SliceError::OutOfBounds { .. })
        ));
        assert!(matches!(
            slice_manual(
                16,
                16,
                vec![
                    SliceRegion::new("a", [0, 0, 1, 1]),
                    SliceRegion::new("a", [1, 1, 1, 1])
                ]
            ),
            Err(SliceError::DuplicateName(_))
        ));
    }

    #[test]
    fn alpha_slicing_finds_separate_sprites_with_padding() {
        let mut rgba = vec![0u8; 8 * 4 * 4];
        let mut paint = |x: usize, y: usize| {
            rgba[(y * 8 + x) * 4 + 3] = 255;
        };
        // 两个不相连的 2x2 块。
        paint(1, 1);
        paint(2, 1);
        paint(1, 2);
        paint(2, 2);
        paint(5, 0);
        paint(6, 0);
        let regions = slice_alpha(&rgba, 8, 4, 1).unwrap();
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0].rect, [0, 0, 4, 4]);
        assert_eq!(regions[1].rect, [4, 0, 4, 2]);
    }

    #[test]
    fn alpha_slicing_rejects_wrong_buffer_size() {
        assert!(matches!(
            slice_alpha(&[0; 4], 2, 2, 0),
            Err(SliceError::SizeMismatch { .. })
        ));
    }

    #[test]
    fn rgba_slicing_copies_exact_rows() {
        let mut rgba = vec![0u8; 8 * 4 * 4];
        for (index, pixel) in rgba.chunks_exact_mut(4).enumerate() {
            pixel[0] = index as u8;
            pixel[3] = 255;
        }
        let crops = slice_rgba(8, 4, &rgba, &[SliceRegion::new("a", [1, 1, 2, 2])]).unwrap();
        assert_eq!(crops.len(), 1);
        assert_eq!(crops[0].len(), 2 * 2 * 4);
        // 行优先：(1,1) (2,1) (1,2) (2,2)，每像素的 R 通道就是线性下标。
        assert_eq!(crops[0][0], 9);
        assert_eq!(crops[0][4], 10);
        assert_eq!(crops[0][8], 17);
        assert_eq!(crops[0][12], 18);
        assert_eq!(crops[0][3], 255);

        assert!(matches!(
            slice_rgba(8, 4, &rgba[..4], &[]),
            Err(SliceError::SizeMismatch { .. })
        ));
        assert!(matches!(
            slice_rgba(8, 4, &rgba, &[SliceRegion::new("a", [7, 0, 2, 1])]),
            Err(SliceError::OutOfBounds { .. })
        ));
    }

    #[test]
    fn nine_slice_regions_cover_the_whole_image() {
        let regions = nine_slice_regions(20, 10, [4, 4, 2, 2]).unwrap();
        assert_eq!(regions.len(), 9);
        assert_eq!(regions[0].rect, [0, 0, 4, 2]);
        assert_eq!(regions[4].rect, [4, 2, 12, 6]);
        assert_eq!(regions[8].rect, [16, 8, 4, 2]);
        assert!(matches!(
            nine_slice_regions(20, 10, [10, 10, 2, 2]),
            Err(SliceError::OutOfBounds { .. })
        ));
    }
}
