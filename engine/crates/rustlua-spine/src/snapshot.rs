//! 骨骼姿态快照（M85 G03）：把 `SpineRenderOutput` 软栅格化成一张 PNG。
//!
//! ## 为什么需要
//!
//! 引擎把动态 UI 整层画在 `WorldSprite` 之上（`render_plan` 的规范顺序），
//! 于是"角色站在界面里"这件事只有两条路：UI 分区留白，或者**把角色烤成贴图**。
//! 后者还顺带解决两个痛点：运行期换不了骨骼（G10）、角色没法当头像/卡面用。
//!
//! ## 为什么不走 GPU
//!
//! GPU 路径要新建 render target、离屏 pass、回读，动 `is_canonical` 的断言，
//! 而且每帧都可能要一张，成本与复杂度都不成比例。这里的数据量很小
//! （一个角色约 1~3k 顶点、一张 512² 图集），CPU 软栅格化一次几毫秒，
//! 且**完全离线**：不碰渲染状态，不影响任何现有通道。
//!
//! ## 算法
//!
//! 1. 取输出顶点（世界坐标，Y 向上）、索引与 draw 列表；
//! 2. 用包围盒 + 目标尺寸算出"世界 → 图像"的等比变换（保持宽高比、居中）；
//! 3. 逐三角形做重心坐标光栅化，逐像素对图集页做双线性采样，
//!    按 `src*a + dst*(1-a)` 混合（图集若是预乘 alpha，先反预乘到直通）；
//! 4. 写出 RGBA PNG。
//!
//! 三角形按 draw 列表顺序、draw 内部按索引顺序绘制——这正是 Spine 的画家算法，
//! 早画的被晚画的覆盖，所以不需要深度缓冲。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::model::SpineRenderOutput;

/// 一张已解码的图集页（RGBA8）。
struct Page {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Page {
    /// 双线性采样，返回直通（非预乘）alpha 的 RGBA。
    ///
    /// UV 原点在左上（Spine 图集导出惯例），与 `SpineVertex::uv` 一致。
    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        if self.width == 0 || self.height == 0 {
            return [0.0, 0.0, 0.0, 0.0];
        }
        let x = (u * self.width as f32 - 0.5).clamp(0.0, (self.width - 1) as f32);
        let y = (v * self.height as f32 - 0.5).clamp(0.0, (self.height - 1) as f32);
        let x0 = x.floor() as u32;
        let y0 = y.floor() as u32;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let fx = x - x0 as f32;
        let fy = y - y0 as f32;

        let texel = |ix: u32, iy: u32| -> [f32; 4] {
            let off = ((iy * self.width + ix) * 4) as usize;
            [
                f32::from(self.pixels[off]),
                f32::from(self.pixels[off + 1]),
                f32::from(self.pixels[off + 2]),
                f32::from(self.pixels[off + 3]),
            ]
        };
        let a = texel(x0, y0);
        let b = texel(x1, y0);
        let c = texel(x0, y1);
        let d = texel(x1, y1);
        let mut out = [0.0f32; 4];
        for k in 0..4 {
            let top = a[k] * (1.0 - fx) + b[k] * fx;
            let bot = c[k] * (1.0 - fx) + d[k] * fx;
            out[k] = top * (1.0 - fy) + bot * fy;
        }
        out
    }
}

/// 快照失败的分类。调用方（Lua 绑定）把它转成脚本错误文本。
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("no vertices sampled yet: the skeleton has never been advanced")]
    EmptyOutput,
    #[error("project root is unknown; cannot resolve atlas page \"{0}\"")]
    NoProjectRoot(String),
    #[error("atlas page not found: {0}")]
    PageMissing(String),
    #[error("failed to decode atlas page {path}: {reason}")]
    PageDecode { path: String, reason: String },
    #[error("failed to encode snapshot PNG {path}: {reason}")]
    Encode { path: String, reason: String },
}

/// 把当前姿态渲染成 RGBA 缓冲。
///
/// `width/height` 是目标像素尺寸；骨骼按包围盒**等比**缩放到这个框里并居中，
/// 因此宽高比与骨骼本身不一致时上下或左右会留透明边（不拉伸，避免变形）。
pub fn render_snapshot(
    output: &SpineRenderOutput,
    page_paths: &HashMap<String, std::sync::Arc<str>>,
    project_root: Option<&Path>,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, SnapshotError> {
    if output.vertices.is_empty() || output.indices.len() < 3 {
        return Err(SnapshotError::EmptyOutput);
    }
    let (w, h) = (width.max(1), height.max(1));

    // 1) 世界包围盒
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for v in &output.vertices {
        min_x = min_x.min(v.position[0]);
        min_y = min_y.min(v.position[1]);
        max_x = max_x.max(v.position[0]);
        max_y = max_y.max(v.position[1]);
    }
    if !min_x.is_finite() || !min_y.is_finite() {
        return Err(SnapshotError::EmptyOutput);
    }
    let span_x = (max_x - min_x).max(1e-4);
    let span_y = (max_y - min_y).max(1e-4);

    // 2) 等比映射：世界 → 图像（图像 Y 向下，世界 Y 向上，所以 y 要翻转）
    let scale = (w as f32 / span_x).min(h as f32 / span_y);
    let draw_w = span_x * scale;
    let draw_h = span_y * scale;
    let offset_x = (w as f32 - draw_w) * 0.5;
    let offset_y = (h as f32 - draw_h) * 0.5;
    let to_image = |p: [f32; 3]| -> (f32, f32) {
        (
            offset_x + (p[0] - min_x) * scale,
            // 世界 max_y 映射到图像顶部
            offset_y + (max_y - p[1]) * scale,
        )
    };

    // 3) 图集页按需解码（独立函数，避免闭包把 &Page 借出捕获的 map）
    let mut pages: HashMap<String, Page> = HashMap::new();
    fn ensure_page<'a>(
        pages: &'a mut HashMap<String, Page>,
        page_paths: &HashMap<String, std::sync::Arc<str>>,
        project_root: Option<&Path>,
        key: &str,
    ) -> Result<&'a Page, SnapshotError> {
        if !pages.contains_key(key) {
            let rel = page_paths
                .get(key)
                .map(std::string::ToString::to_string)
                .unwrap_or_else(|| key.to_owned());
            let root = project_root.ok_or_else(|| SnapshotError::NoProjectRoot(rel.clone()))?;
            let path: PathBuf = root.join(rel.replace('\\', "/"));
            if !path.is_file() {
                return Err(SnapshotError::PageMissing(path.display().to_string()));
            }
            let img = image::open(&path).map_err(|error| SnapshotError::PageDecode {
                path: path.display().to_string(),
                reason: error.to_string(),
            })?;
            let rgba = img.to_rgba8();
            pages.insert(
                key.to_owned(),
                Page {
                    width: rgba.width(),
                    height: rgba.height(),
                    pixels: rgba.into_raw(),
                },
            );
        }
        Ok(&pages[key])
    }

    // 4) 目标缓冲（透明底）+ alpha 混合
    let mut dst = vec![0f32; (w * h * 4) as usize];
    // 逐像素做「src over dst」，存成 0..255 的浮点，最后一次性量化

    let mut index_cursor = 0usize;
    for draw in &output.draws {
        let page = ensure_page(&mut pages, page_paths, project_root, &draw.texture)?;
        let start = draw.first_index as usize;
        let count = draw.index_count as usize;
        index_cursor = start.max(index_cursor);

        let mut i = start;
        while i + 2 < start + count && i + 2 < output.indices.len() {
            let ia = output.indices[i] as usize;
            let ib = output.indices[i + 1] as usize;
            let ic = output.indices[i + 2] as usize;
            i += 3;
            let (Some(va), Some(vb), Some(vc)) = (
                output.vertices.get(ia),
                output.vertices.get(ib),
                output.vertices.get(ic),
            ) else {
                continue;
            };

            let (ax, ay) = to_image(va.position);
            let (bx, by) = to_image(vb.position);
            let (cx, cy) = to_image(vc.position);

            let area = (bx - ax) * (cy - ay) - (cx - ax) * (by - ay);
            if area.abs() < 1e-6 {
                continue; // 退化三角形
            }

            let min_px = ax.min(bx).min(cx).floor().max(0.0) as i32;
            let max_px = ax.max(bx).max(cx).ceil().min(w as f32 - 1.0) as i32;
            let min_py = ay.min(by).min(cy).floor().max(0.0) as i32;
            let max_py = ay.max(by).max(cy).ceil().min(h as f32 - 1.0) as i32;
            if max_px < min_px || max_py < min_py {
                continue;
            }

            let inv_area = 1.0 / area;
            for py in min_py..=max_py {
                let fy = py as f32 + 0.5;
                for px in min_px..=max_px {
                    let fx = px as f32 + 0.5;
                    // 重心坐标
                    let w0 = ((bx - fx) * (cy - fy) - (cx - fx) * (by - fy)) * inv_area;
                    let w1 = ((cx - fx) * (ay - fy) - (ax - fx) * (cy - fy)) * inv_area;
                    let w2 = 1.0 - w0 - w1;
                    // 容一点误差，避免共享边出现裂缝
                    if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                        continue;
                    }

                    let u = w0 * va.uv[0] + w1 * vb.uv[0] + w2 * vc.uv[0];
                    let v = w0 * va.uv[1] + w1 * vb.uv[1] + w2 * vc.uv[1];
                    let texel = page.sample(u, v);

                    // 顶点色（light color）调制
                    let cr = w0 * va.color[0] + w1 * vb.color[0] + w2 * vc.color[0];
                    let cg = w0 * va.color[1] + w1 * vb.color[1] + w2 * vc.color[1];
                    let cb = w0 * va.color[2] + w1 * vb.color[2] + w2 * vc.color[2];
                    let ca = w0 * va.color[3] + w1 * vb.color[3] + w2 * vc.color[3];

                    let sa = (texel[3] / 255.0) * ca;
                    if sa <= 0.0 {
                        continue;
                    }
                    // 图集若是预乘 alpha，先反预乘再按直通混合
                    let unpremul = |c: f32| -> f32 {
                        if draw.premultiplied_alpha && texel[3] > 0.5 {
                            (c / texel[3] * 255.0).clamp(0.0, 255.0)
                        } else {
                            c
                        }
                    };
                    let sr = unpremul(texel[0]) * cr;
                    let sg = unpremul(texel[1]) * cg;
                    let sb = unpremul(texel[2]) * cb;

                    let off = ((py as u32 * w + px as u32) * 4) as usize;
                    let inv = 1.0 - sa;
                    dst[off] = sr * sa + dst[off] * inv;
                    dst[off + 1] = sg * sa + dst[off + 1] * inv;
                    dst[off + 2] = sb * sa + dst[off + 2] * inv;
                    dst[off + 3] = sa * 255.0 + dst[off + 3] * inv;
                }
            }
        }
    }

    let mut out = vec![0u8; (w * h * 4) as usize];
    for (i, value) in dst.iter().enumerate() {
        out[i] = value.round().clamp(0.0, 255.0) as u8;
    }
    Ok(out)
}

/// 渲染并写出 PNG。返回实际写出的路径。
pub fn write_snapshot(
    output: &SpineRenderOutput,
    page_paths: &HashMap<String, std::sync::Arc<str>>,
    project_root: Option<&Path>,
    width: u32,
    height: u32,
    out_path: &Path,
) -> Result<PathBuf, SnapshotError> {
    let rgba = render_snapshot(output, page_paths, project_root, width, height)?;
    if let Some(parent) = out_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let buffer =
        image::RgbaImage::from_raw(width.max(1), height.max(1), rgba).ok_or_else(|| {
            SnapshotError::Encode {
                path: out_path.display().to_string(),
                reason: "buffer size does not match dimensions".to_owned(),
            }
        })?;
    buffer
        .save(out_path)
        .map_err(|error| SnapshotError::Encode {
            path: out_path.display().to_string(),
            reason: error.to_string(),
        })?;
    Ok(out_path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SpineBlend, SpineDraw, SpineVertex};
    use std::sync::Arc;

    /// 唯一临时目录（与仓库既有测试隔离约定一致：进程 id + 纳秒后缀）。
    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "rustlua-spine-snapshot-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        dir
    }

    /// 写一张 `size×size` 的单色图集页，返回 `page_paths` 里用的相对名。
    fn write_page(dir: &Path, name: &str, size: u32, rgba: [u8; 4]) -> Arc<str> {
        let page = image::RgbaImage::from_pixel(size, size, image::Rgba(rgba));
        page.save(dir.join(name)).expect("写图集页失败");
        Arc::from(name)
    }

    /// 覆盖整张图的一个四边形（uv 覆盖 0..1），颜色全白。
    fn full_quad(premultiplied_alpha: bool) -> SpineRenderOutput {
        let color = [1.0, 1.0, 1.0, 1.0];
        SpineRenderOutput {
            vertices: vec![
                SpineVertex {
                    position: [0.0, 0.0, 0.0],
                    uv: [0.0, 1.0],
                    color,
                },
                SpineVertex {
                    position: [10.0, 0.0, 0.0],
                    uv: [1.0, 1.0],
                    color,
                },
                SpineVertex {
                    position: [10.0, 10.0, 0.0],
                    uv: [1.0, 0.0],
                    color,
                },
                SpineVertex {
                    position: [0.0, 10.0, 0.0],
                    uv: [0.0, 0.0],
                    color,
                },
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            draws: vec![SpineDraw {
                texture: "page.png".into(),
                first_index: 0,
                index_count: 6,
                blend: SpineBlend::Normal,
                premultiplied_alpha,
            }],
        }
    }

    fn center_pixel(rgba: &[u8], width: u32, height: u32) -> [u8; 4] {
        let offset = (((height / 2) * width + width / 2) * 4) as usize;
        [
            rgba[offset],
            rgba[offset + 1],
            rgba[offset + 2],
            rgba[offset + 3],
        ]
    }

    #[test]
    fn empty_output_is_rejected() {
        let output = SpineRenderOutput::default();
        let err = render_snapshot(&output, &HashMap::new(), None, 64, 64).unwrap_err();
        assert!(matches!(err, SnapshotError::EmptyOutput));
    }

    #[test]
    fn missing_project_root_is_reported() {
        let err = render_snapshot(&full_quad(false), &HashMap::new(), None, 32, 32).unwrap_err();
        assert!(matches!(err, SnapshotError::NoProjectRoot(_)));
    }

    #[test]
    fn degenerate_triangle_is_skipped_without_panic() {
        let dir = temp_dir("degenerate");
        let mut output = full_quad(false);
        // 三个顶点重合 ⇒ 面积为 0，必须跳过而不是 panic。
        output.vertices.truncate(3);
        output.vertices.iter_mut().for_each(|vertex| {
            vertex.position = [1.0, 1.0, 0.0];
        });
        output.indices = vec![0, 1, 2];
        output.draws[0].index_count = 3;
        let page_paths = HashMap::from([(
            "page.png".to_owned(),
            write_page(&dir, "page.png", 2, [255, 255, 255, 255]),
        )]);
        let rgba =
            render_snapshot(&output, &page_paths, Some(&dir), 8, 8).expect("退化三角形应可跳过");
        assert_eq!(rgba.len(), 8 * 8 * 4);
        assert!(
            rgba.iter().all(|byte| *byte == 0),
            "退化三角形不应画出任何像素"
        );
    }

    #[test]
    fn opaque_quad_fills_the_frame() {
        let dir = temp_dir("opaque");
        let page_paths = HashMap::from([(
            "page.png".to_owned(),
            write_page(&dir, "page.png", 2, [255, 255, 255, 255]),
        )]);
        let rgba = render_snapshot(&full_quad(false), &page_paths, Some(&dir), 8, 8)
            .expect("四边形应能光栅化");
        assert_eq!(rgba.len(), 8 * 8 * 4);
        assert_eq!(center_pixel(&rgba, 8, 8), [255, 255, 255, 255]);
        assert!(
            rgba.chunks_exact(4).all(|pixel| pixel[3] == 255),
            "不透明页应铺满整帧"
        );
    }

    #[test]
    fn premultiplied_alpha_is_unpremultiplied_before_blending() {
        // 页像素 = 预乘后的 50% 白（rgb=128, a=128）。直通混合会把它当成暗灰，
        // 反预乘后才是白 ✕ 50%。两次输出必须明显不同。
        let dir = temp_dir("pma");
        let page_paths = HashMap::from([(
            "page.png".to_owned(),
            write_page(&dir, "page.png", 2, [128, 128, 128, 128]),
        )]);
        let straight =
            render_snapshot(&full_quad(false), &page_paths, Some(&dir), 8, 8).expect("直通 alpha");
        let premultiplied =
            render_snapshot(&full_quad(true), &page_paths, Some(&dir), 8, 8).expect("预乘 alpha");
        let straight_pixel = center_pixel(&straight, 8, 8);
        let premultiplied_pixel = center_pixel(&premultiplied, 8, 8);
        assert_eq!(
            straight_pixel[3], premultiplied_pixel[3],
            "alpha 通道不受反预乘影响"
        );
        assert!(
            premultiplied_pixel[0] > straight_pixel[0] + 40,
            "预乘页必须先反预乘：straight={straight_pixel:?} premultiplied={premultiplied_pixel:?}"
        );
    }

    #[test]
    fn zero_size_is_clamped_to_one_pixel() {
        let dir = temp_dir("zero");
        let page_paths = HashMap::from([(
            "page.png".to_owned(),
            write_page(&dir, "page.png", 2, [255, 255, 255, 255]),
        )]);
        let rgba = render_snapshot(&full_quad(false), &page_paths, Some(&dir), 0, 0)
            .expect("零尺寸应被钳到 1×1 而不是 panic");
        assert_eq!(rgba.len(), 4, "1×1 RGBA");
    }

    #[test]
    fn write_snapshot_creates_a_decodable_png() {
        let dir = temp_dir("write");
        let page_paths = HashMap::from([(
            "page.png".to_owned(),
            write_page(&dir, "page.png", 2, [255, 255, 255, 255]),
        )]);
        let out_path = dir.join("nested").join("pose.png");
        let written = write_snapshot(&full_quad(false), &page_paths, Some(&dir), 4, 4, &out_path)
            .expect("写 PNG 失败");
        assert_eq!(written, out_path);
        let decoded = image::open(&out_path).expect("写出的 PNG 应可解码");
        assert_eq!((decoded.width(), decoded.height()), (4, 4));
    }
}
