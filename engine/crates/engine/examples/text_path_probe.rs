//! 走**真实渲染器**的文字探针：同一串文本、同一设计分辨率，按 `vector` / `sdf` 两档出图。
//!
//!     cargo run -q -p engine --example text_path_probe -- <font.ttf> <font_px> <vector|sdf|hinted> <out.png> [designW designH outW outH]
//!
//! 与 `royale_text_probe` 的区别：不加载游戏场景，文本由环境变量 `TEXT` 指定，因此可以拿任意
//! 字体、任意字号、同一串字做 A/B。渲染路径与运行时完全一致——矢量档写 `vector_vertices`
//! （与 `push_vector_text_run_styled` 同样的快照结构），SDF 档写 `font_atlas` + 一条 `sdf` draw，
//! 视口 = 设计盒 contain 到真实附件（与宿主同一套 `contain_rect` 行为）。

use std::path::PathBuf;

use rustlua_gpu::{OffscreenRequest, OffscreenTarget, RenderOutcome, Renderer};
use rustlua_render::font::{FontAtlas, FontVectorAtlas};
use rustlua_render::frame::{
    CustomFontAtlasPixels, FONT_ATLAS_TEXTURE_PATH, FontAtlasPixels, OverlayVertex, SceneDraw,
    SceneMesh, VectorGlyphSnapshot, VectorSegmentSnapshot, VectorTextVertex,
};
use rustlua_render::hinted::HintedAtlas;
use rustlua_render::layer::RenderLayer;
use rustlua_render::text::{TextLayout, TextStyle};

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 4 {
        return Err(
            "用法: text_path_probe <font.ttf> <font_px> <vector|sdf|hinted> <out.png> [designW designH outW outH]"
                .to_owned(),
        );
    }
    let font_path = PathBuf::from(&args[0]);
    let font_px: f32 = args[1].parse().map_err(|_| "font_px 不是数字".to_owned())?;
    let mode = args[2].as_str();
    let output = PathBuf::from(&args[3]);
    let design_w: u32 = args.get(4).and_then(|v| v.parse().ok()).unwrap_or(720);
    let design_h: u32 = args.get(5).and_then(|v| v.parse().ok()).unwrap_or(1280);
    let out_w: u32 = args.get(6).and_then(|v| v.parse().ok()).unwrap_or(541);
    let out_h: u32 = args.get(7).and_then(|v| v.parse().ok()).unwrap_or(961);
    let text = std::env::var("TEXT").unwrap_or_else(|_| "骑士 野猪骑士 圣水".to_owned());

    let bytes = std::fs::read(&font_path).map_err(|error| format!("读不到字体：{error}"))?;
    // 字距取 0：A/B 只比光栅，不让排版差异混进宽度与墨量。
    let style = TextStyle {
        scale: font_px / 12.0,
        spacing: 0.0,
        color: [1.0, 1.0, 1.0, 1.0],
        ..Default::default()
    };
    // 设计空间里的笔位：左上留 8px，基线由 TextLayout 按字体度量算。
    let origin = [8.0_f32, 8.0_f32];

    let mut mesh = SceneMesh {
        vertices: Vec::new(),
        draws: Vec::new(),
        world_sprites: Vec::new(),
        world_sprite_draws: Vec::new(),
        paths: Vec::new(),
        path_handles: Vec::new(),
        font_atlas: None,
        custom_font_atlases: Vec::new(),
        vector_vertices: Vec::new(),
        vector_glyphs: Vec::new(),
        vector_segments: Vec::new(),
    };

    match mode {
        "vector" => {
            let mut atlas = FontVectorAtlas::load(&bytes)?;
            let layout = TextLayout::build_vector(&text, origin, style, &mut atlas)
                .map_err(|error| format!("矢量排版：{error}"))?;
            let mut remap = std::collections::HashMap::new();
            for vertex in layout {
                let mapped = if let Some(mapped) = remap.get(&vertex.glyph_id) {
                    *mapped
                } else {
                    let glyph = atlas
                        .glyph_by_id(vertex.glyph_id)
                        .ok_or_else(|| "字形不在图集里".to_owned())?;
                    let mapped = mesh.vector_glyphs.len() as u32;
                    let segment_start = mesh.vector_segments.len() as u32;
                    mesh.vector_segments
                        .extend(glyph.segments.iter().map(|segment| VectorSegmentSnapshot {
                            start: segment.start,
                            end: segment.end,
                            bounds: [
                                segment.start[0].min(segment.end[0]),
                                segment.start[1].min(segment.end[1]),
                                segment.start[0].max(segment.end[0]),
                                segment.start[1].max(segment.end[1]),
                            ],
                        }));
                    mesh.vector_glyphs.push(VectorGlyphSnapshot {
                        segment_start,
                        segment_count: mesh.vector_segments.len() as u32 - segment_start,
                        _padding: [0; 2],
                        bounds: glyph.bounds,
                    });
                    remap.insert(vertex.glyph_id, mapped);
                    mapped
                };
                mesh.vector_vertices.push(VectorTextVertex {
                    position: [
                        vertex.position[0] / design_w as f32 * 2.0 - 1.0,
                        1.0 - vertex.position[1] / design_h as f32 * 2.0,
                    ],
                    local: vertex.local,
                    glyph_id: mapped,
                    color: vertex.color,
                });
            }
        }
        "sdf" => {
            let mut atlas = FontAtlas::load(&bytes)?;
            let layout = TextLayout::build(&text, origin, style, &mut atlas)
                .map_err(|error| format!("SDF 排版：{error}"))?;
            let bitmap = layout.bitmap;
            let sdf_value: u32 = if bitmap { 2 } else { 1 };
            let start = mesh.vertices.len() as u32;
            mesh.vertices
                .extend(layout.vertices.into_iter().map(|vertex| OverlayVertex {
                    position: [
                        vertex.position[0] / design_w as f32 * 2.0 - 1.0,
                        1.0 - vertex.position[1] / design_h as f32 * 2.0,
                    ],
                    uv: vertex.uv,
                    color: vertex.color,
                    sdf: sdf_value,
                }));
            mesh.paths.push(FONT_ATLAS_TEXTURE_PATH.to_owned());
            mesh.draws.push(SceneDraw {
                texture: 1,
                start,
                count: mesh.vertices.len() as u32 - start,
                sdf: true,
                bitmap,
                vector: false,
                render_layer: RenderLayer::ScreenUI,
            });
            mesh.font_atlas = Some(FontAtlasPixels {
                width: atlas.width(),
                height: atlas.height(),
                pixels: atlas.pixels().to_vec(),
                version: atlas.version(),
            });
        }
        "hinted" => {
            // 物理像素排版：ppem 就是 font_px 取整，NDC 按附件尺寸换算（1:1）。
            let ppem = font_px.round().clamp(1.0, 255.0) as u16;
            let mut atlas = HintedAtlas::load(&bytes, ppem)?;
            let layout = TextLayout::build_hinted(&text, origin, style, &mut atlas, 1.0)
                .map_err(|error| format!("hinting 排版：{error}"))?;
            let key = "font://text-path-probe-hinted";
            let start = mesh.vertices.len() as u32;
            mesh.vertices
                .extend(layout.vertices.into_iter().map(|vertex| OverlayVertex {
                    position: [
                        vertex.position[0] / out_w as f32 * 2.0 - 1.0,
                        1.0 - vertex.position[1] / out_h as f32 * 2.0,
                    ],
                    uv: vertex.uv,
                    color: vertex.color,
                    sdf: 3,
                }));
            mesh.paths.push(key.to_owned());
            mesh.draws.push(SceneDraw {
                texture: 1,
                start,
                count: mesh.vertices.len() as u32 - start,
                sdf: true,
                bitmap: true,
                vector: false,
                render_layer: RenderLayer::ScreenUI,
            });
            mesh.custom_font_atlases.push(CustomFontAtlasPixels {
                key: key.to_owned(),
                pixels: FontAtlasPixels {
                    width: atlas.width(),
                    height: atlas.height(),
                    pixels: atlas.pixels().to_vec(),
                    version: atlas.version(),
                },
            });
            println!(
                "hinted_ppem={} is_hinted={}",
                atlas.ppem(),
                atlas.is_hinted()
            );
        }
        other => return Err(format!("未知模式：{other}（vector|sdf|hinted）")),
    }

    let fixture = OffscreenTarget::acquire(&OffscreenRequest::new("text-path-probe", out_w, out_h))
        .map_err(|error| format!("离屏设备：{error}"))?;
    let mut renderer = Renderer::new_offscreen(
        fixture.device().clone(),
        fixture.queue().clone(),
        None,
        false,
        true,
    )
    .map_err(|error| format!("renderer：{error}"))?;
    if !matches!(
        renderer.render_into_texture(&mesh, None, fixture.view(), out_w, out_h),
        RenderOutcome::Presented
    ) {
        return Err("离屏渲染没有出帧".to_owned());
    }
    let pixels = renderer
        .copy_texture_to_cpu(fixture.texture(), out_w, out_h)
        .map_err(|error| format!("读回像素：{error}"))?;
    let mut image = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_pixel(
        out_w,
        out_h,
        image::Rgba([0; 4]),
    );
    for (index, pixel) in image.pixels_mut().enumerate() {
        let src = index * 4;
        *pixel = image::Rgba([
            pixels[src],
            pixels[src + 1],
            pixels[src + 2],
            pixels[src + 3],
        ]);
    }
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("mkdir：{error}"))?;
    }
    image
        .save(&output)
        .map_err(|error| format!("保存 PNG：{error}"))?;
    println!(
        "text_path_probe_ok png={} mode={mode} font_px={font_px} design={design_w}x{design_h} out={out_w}x{out_h}",
        output.display()
    );
    Ok(())
}
