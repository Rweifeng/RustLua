//! Headless acceptance drive for M72-03：**像素中文字模渲染（Fusion Pixel 接入）**。
//!
//! 需求（`tasks/done/M72-03-像素中文字模渲染.md`）：中文不再是压在像素美术上的平滑矢量字。
//! 这条门钉的是"点阵到底有没有真落到整数格、有没有真走硬边档"，而不是"看着挺清楚"。
//!
//! 为什么必须这样测——plan §风险 的原始怀疑是「12px 设计网格 vs `RASTER_SCALE=96` 是 8 倍关系，
//! 理论上每个像素单元正好落在整数设备像素上，**但这是推算，不是实测**」。实测结论：
//!   * 只有 **12 px/em**（1 texel = 1 设计像素）档给出**纯二值**覆盖；同一份字体在 96 px/em 档
//!     栅格化，单个'山'字就有上千个中间值 texel（本门 §②c 用被替换掉的 Noto 做同口径对照，
//!     "同字体两档"的对照钉在 `font.rs` 单测 `same_pixel_font_is_not_binary_at_msdf_scale`）；
//!   * "轮廓坐标 GCD"这条看似更省的判据**实测失效**：470 个字形里 429 个 GCD=100，另有 35 个
//!     带非整数坐标、6 个落在 50/25/10/2 的更细网格上 → 整体 GCD 被拉到 1。所以字体身份按
//!     "栅格化覆盖率是否恰好二值"判定（`font.rs::detect_pixel_size`）。
//!
//! 章节：
//!   ① 字体身份：项目 `font =` 指向的那份**子集**仍判为 12px 点阵（子集化重建 glyf/cmap 后网格
//!      没被破坏），全量上游字体同判；Noto 判不出来（反面对照：判据不是"什么字体都像点阵"）；
//!      矢量缓存共用这条判定（渲染层据此放弃矢量档）。
//!   ② 阈值档接线：真跑一帧——文字绘制段必须 `sdf` 通道 + `bitmap=true`（`passes.rs:518` 由此选
//!      Nearest 绑定）、顶点 `sdf` 必须恒为 2、矢量通道必须为空；本帧文字用到的图集 texel 只能
//!      是 0 或 255，且至少一段落在 1:1 档上。反面对照：Noto/MSDF 同口径灰阶成片。
//!   ③ 1:1 texel 覆盖：12px（`tex_scale=1`）下每个 texel **恰好被一个设备像素采到**。整数定位是
//!      这条成立的前提，所以把 0..1 的全部分数偏移扫一遍：只有 0 能全 1，其余偏移一律出现
//!      "某 texel 采两次 + 某 texel 一次都没采到"（= 关掉 `text.rs` 整数吸附的等价后果）。
//!   ④ 缺字契约：工程实际用到的字符全集（`char_collect` + `chars.txt`）逐字断言"字体覆盖它"
//!      且"点阵有亮 texel"。反面对照：真缺的字必须被 `covers()` 判出来，且仍画得出可见方框。
//!   ⑤ 预算：全部工程字一次性预热，报告图集尺寸与耗时（M72-10「全 UI 换像素字」的前提）。
//!
//! Usage:
//!   cargo run -p game --example jhyt_m72_pixel_font [-- <game_jhyt-root>]
//!
//! 前置：`engine/crates/editor/fonts/FusionPixel-12px-Monospaced-zhHans.ttf`（7 MB 上游全量，
//! **不入库**）必须就位；缺文件时本门**报错并打印取回命令**，不静默跳过（见 ① 那段）。

use engine::host::{RuntimeScene, SceneMesh, frame::FONT_ATLAS_TEXTURE_PATH};
use rustlua_assets::pack;
use rustlua_input::input::InputSnapshot;
use rustlua_render::font::{FontAtlas, FontVectorAtlas, RASTER_SCALE, project_font};
use rustlua_render::text::{TextLayout, TextMesh, TextStyle};
use rustlua_scene::char_collect::collect_project_chars;
use std::{
    env, fs,
    path::{Path, PathBuf},
    time::Instant,
};

const VIEW_W: u32 = 1280;
const VIEW_H: u32 = 720;
const DT: f32 = 1.0 / 60.0;
/// 探针串：plan 验收方式点名的四字（笔画密集，最容易暴露糊边）。
const PROBE_TEXT: &str = "山庄炼药";
/// 点阵字体应被判出的设计像素高度（= Fusion Pixel 的字模高度）。
const WANT_DESIGN_PX: f32 = 12.0;
/// 上游全量字体（子集化的源，即 `subset_project_font --source` 那份），相对 `crates/game`。
const FULL_FONT: &str = "../editor/fonts/FusionPixel-12px-Monospaced-zhHans.ttf";
/// 对照用的平滑矢量字体（本包之前的项目字体，仍在 dev 树里）。
const SMOOTH_FONT: &str = "assets/fonts/NotoSansSC-VF.ttf";

fn banner(title: &str) {
    println!("\n== {title} ==");
}

/// 一个字形四边形：设备像素矩形（左闭上开）+ 它采到的图集 texel 矩形（同样左闭上开）。
#[derive(Clone, Copy, Debug)]
struct Quad {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    tx0: f32,
    ty0: f32,
    tx1: f32,
    ty1: f32,
}

impl Quad {
    /// 由文字 quad 的三个角构造。`text.rs:486` 的发序是 `[tl, tr, br, tl, br, bl]`
    /// （两个三角形共享对角线），所以左上=`c0`、右上=`c1`、左下是**第 6 个**顶点。
    fn from_corners(
        c0: [f32; 2],
        c1: [f32; 2],
        c3: [f32; 2],
        uv0: [f32; 2],
        uv1: [f32; 2],
        uv3: [f32; 2],
        atlas: (u32, u32),
    ) -> Self {
        let (aw, ah) = (atlas.0 as f32, atlas.1 as f32);
        Self {
            x0: c0[0],
            y0: c0[1],
            x1: c1[0],
            y1: c3[1],
            tx0: uv0[0] * aw,
            ty0: uv0[1] * ah,
            tx1: uv1[0] * aw,
            ty1: uv3[1] * ah,
        }
    }

    fn device_w(&self) -> f32 {
        self.x1 - self.x0
    }

    fn device_h(&self) -> f32 {
        self.y1 - self.y0
    }

    fn texel_w(&self) -> f32 {
        self.tx1 - self.tx0
    }

    fn texel_h(&self) -> f32 {
        self.ty1 - self.ty0
    }

    /// 四个边角是否都落在整数设备像素上（`1e-3` 容差是给 NDC 往返留的）。
    fn corners_integral(&self, tolerance: f32) -> bool {
        [self.x0, self.y0, self.x1, self.y1]
            .iter()
            .all(|value| value.rem_euclid(1.0).min(1.0 - value.rem_euclid(1.0)) <= tolerance)
    }
}

/// Nearest 采样模型（与 wgpu `FilterMode::Nearest` 同口径）：片元取设备像素中心，
/// `floor(uv × 图集尺寸)` 决定落到哪一格。返回逐 texel 被采到的设备像素数。
///
/// `offset` 把整块 quad 平移（§③ 用它扫分数偏移），不改 quad 自身几何。
fn sample_counts(quad: &Quad, offset: f32) -> Vec<usize> {
    let columns = quad.texel_w().round().max(0.0) as usize;
    let rows = quad.texel_h().round().max(0.0) as usize;
    let mut counts = vec![0usize; columns * rows];
    let x0 = quad.x0 + offset;
    let y0 = quad.y0 + offset;
    // 被这个 quad 覆盖的设备像素 = 中心落在 [x0, x1) × [y0, y1) 内的整数格。
    let first_x = (x0 - 0.5).ceil() as i64;
    let last_x = (x0 + quad.device_w() - 0.5).floor() as i64;
    let first_y = (y0 - 0.5).ceil() as i64;
    let last_y = (y0 + quad.device_h() - 0.5).floor() as i64;
    for py in first_y..=last_y {
        for px in first_x..=last_x {
            let u = (px as f32 + 0.5 - x0) / quad.device_w();
            let v = (py as f32 + 0.5 - y0) / quad.device_h();
            let tx = (quad.tx0 + u * quad.texel_w()).floor() - quad.tx0.floor();
            let ty = (quad.ty0 + v * quad.texel_h()).floor() - quad.ty0.floor();
            if tx < 0.0 || ty < 0.0 {
                continue;
            }
            let (tx, ty) = (tx as usize, ty as usize);
            if tx >= columns || ty >= rows {
                continue;
            }
            counts[ty * columns + tx] += 1;
        }
    }
    counts
}

/// 取一个字形在图集里的单元矩形字节（`cell_rect` 覆盖的范围）。
fn glyph_cell(atlas: &mut FontAtlas, character: char) -> Result<Vec<u8>, String> {
    let metrics = atlas
        .try_glyph_metrics(character)
        .map_err(|error| format!("{character:?} 字形：{error:?}"))?;
    let width = atlas.width();
    let left = metrics.cell_rect[0].floor().max(0.0) as u32;
    let top = metrics.cell_rect[1].floor().max(0.0) as u32;
    let right = metrics.cell_rect[2].ceil().min(atlas.width() as f32) as u32;
    let bottom = metrics.cell_rect[3].ceil().min(atlas.height() as f32) as u32;
    let pixels = atlas.pixels().to_vec();
    let mut out = Vec::new();
    for y in top..bottom {
        for x in left..right {
            let offset = ((y * width + x) * 4) as usize;
            out.extend_from_slice(&pixels[offset..offset + 4]);
        }
    }
    Ok(out)
}

/// 统计一块字形字节：`(灰阶 texel, 全亮 texel, 全暗 texel, alpha 非 255 的 texel)`。
fn coverage_histogram(cell: &[u8]) -> (usize, usize, usize, usize) {
    let mut grey = 0;
    let mut lit = 0;
    let mut dark = 0;
    let mut alpha_off = 0;
    for pixel in cell.chunks_exact(4) {
        let (r, g, b) = (pixel[0], pixel[1], pixel[2]);
        if r == 0 && g == 0 && b == 0 {
            dark += 1;
        } else if r == 255 && g == 255 && b == 255 {
            lit += 1;
        } else {
            grey += 1;
        }
        if pixel[3] != 255 {
            alpha_off += 1;
        }
    }
    (grey, lit, dark, alpha_off)
}

fn quads_from_text_mesh(mesh: &TextMesh, atlas: (u32, u32)) -> Vec<Quad> {
    mesh.vertices
        .chunks_exact(6)
        .map(|hexagon| {
            Quad::from_corners(
                hexagon[0].position,
                hexagon[1].position,
                hexagon[5].position,
                hexagon[0].uv,
                hexagon[1].uv,
                hexagon[5].uv,
                atlas,
            )
        })
        .collect()
}

/// overlay 顶点是 NDC，按 `text_to_ndc` 的逆变换回到设备像素。
fn to_device_px(value: [f32; 2]) -> [f32; 2] {
    [
        (value[0] + 1.0) * 0.5 * VIEW_W as f32,
        (1.0 - value[1]) * 0.5 * VIEW_H as f32,
    ]
}

fn path_of(mesh: &SceneMesh, texture: u32) -> Option<&str> {
    if texture == 0 {
        return None;
    }
    mesh.paths.get((texture - 1) as usize).map(String::as_str)
}

fn frame(runtime: &mut RuntimeScene) -> Result<(), Box<dyn std::error::Error>> {
    runtime.vertices(VIEW_W, VIEW_H)?;
    runtime
        .run_script_steps(1, DT, &InputSnapshot::default())
        .map_err(|error| error.into())
}

fn load_font(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let raw = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    // dev 树里既有打包编码的资源也有裸 TTF；`pack::decode` 对非 `RustLua` 头原样透传，
    // 与 `runtime_scene/fonts.rs` 的加载口径一致。
    pack::decode(&raw).map_err(|error| format!("decode {}: {error}", path.display()).into())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project = env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        rustlua_infra::games_dir()
            .join("examples")
            .join("game_jhyt")
    });
    let project = project.canonicalize()?;
    if !project.join("project.toml").is_file() {
        return Err(format!("not a game project: {}", project.display()).into());
    }
    // 全量上游字体在 `engine/crates/editor/fonts/`（`subset_project_font --source` 用的那份）。
    // **它不入库**（7 MB，仓里已有 17.7 MB 的 Noto 做对照，再加一份纯子集源不值），所以新克隆
    // 必须先按下面的命令取回。这里**故意不做“文件没了就跳过”**：本包 §① 的“子集与全量同判”
    // 那半条一旦能静默跳过，就成了“看起来跑过了”（跳过绝不等于通过）。
    let full_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(FULL_FONT)
        .canonicalize()
        .map_err(|_| {
            let want = Path::new(env!("CARGO_MANIFEST_DIR")).join(FULL_FONT);
            format!(
                "缺少全量上游字体：{}（本门 §① 要用它对照“子集与全量同判”）。\
                 取回（两条命令，第二条把解出的字节写进上面那个路径）：\n  \
                 curl -sL -o \"$TEMP/fpfont/fp12.zip\" https://github.com/TakWolf/fusion-pixel-font/releases/download/2026.09.25/fusion-pixel-font-12px-monospaced-ttf-v2026.09.25.zip\n  \
                 unzip -p \"$TEMP/fpfont/fp12.zip\" fusion-pixel-12px-monospaced-zh_hans.ttf > {}\n\
                 应为 7,040,712 字节 / SHA-256 339f3c71531a6b3caae1023c68873eea45554e87571e8fac5c8954abe27fbff1；\
                 授权与表结构核验记录见 examples/game_jhyt/assets/fonts/LICENSE-FusionPixel.txt",
                want.display(),
                want.display(),
            )
        })?;
    let font_rel = project_font::primary_path(&project);
    let subset = load_font(&project.join(&font_rel))?;
    let smooth = load_font(&project.join(SMOOTH_FONT))?;
    let full = load_font(&full_path)?;

    // ---------------- ① 字体身份 ----------------
    banner("M72-03① 字体身份：子集仍是 12px 点阵，平滑字体判不出来");
    let mut atlas = FontAtlas::load(&subset)?;
    let full_atlas = FontAtlas::load(&full)?;
    let mut smooth_atlas = FontAtlas::load(&smooth)?;
    let subset_vector = FontVectorAtlas::load(&subset)?;
    let smooth_vector = FontVectorAtlas::load(&smooth)?;
    println!(
        "  {font_rel}：子集 {}B → design_px={:?} raster_scale={}；全量 {}B → {:?}；Noto {}B → {:?}",
        subset.len(),
        atlas.pixel_design_px(),
        atlas.raster_scale(),
        full.len(),
        full_atlas.pixel_design_px(),
        smooth.len(),
        smooth_atlas.pixel_design_px(),
    );
    if atlas.pixel_design_px() != Some(WANT_DESIGN_PX) {
        return Err(format!(
            "项目字体没被判为 {WANT_DESIGN_PX}px 点阵（实测 {:?}）——子集化重建 glyf/cmap 后若点阵网格被动过，\
             整条硬边路就退化回糊字，这条门拦的就是这个",
            atlas.pixel_design_px()
        )
        .into());
    }
    if full_atlas.pixel_design_px() != Some(WANT_DESIGN_PX) {
        return Err(format!(
            "全量上游字体判为 {:?}，与子集不一致——开发形态与发布形态的字形必须同判，否则一边点阵一边平滑",
            full_atlas.pixel_design_px()
        )
        .into());
    }
    if atlas.raster_scale() != WANT_DESIGN_PX {
        return Err(format!(
            "raster_scale={} 不等于设计像素高度（布局的 tex_scale 由它算；写死 RASTER_SCALE 会把点阵显示成 8 倍大）",
            atlas.raster_scale()
        )
        .into());
    }
    // 反面对照：判据不许"什么字体都判成点阵"。
    if smooth_atlas.is_pixel_font() || smooth_atlas.raster_scale() != RASTER_SCALE {
        return Err(format!(
            "Noto 被误判为点阵（design_px={:?} raster_scale={}）——判据失效，全引擎的 MSDF 文字会一起切到硬边档",
            smooth_atlas.pixel_design_px(),
            smooth_atlas.raster_scale()
        )
        .into());
    }
    if !subset_vector.is_pixel_font() || smooth_vector.is_pixel_font() {
        return Err(format!(
            "矢量缓存的判定与图集不一致（subset={} smooth={}）——`should_render_vector` 读的就是这个值，\
             不一致等于像素字体仍会被送去矢量档做 AA",
            subset_vector.is_pixel_font(),
            smooth_vector.is_pixel_font()
        )
        .into());
    }
    println!("  矢量档挡板：subset.is_pixel_font=true，smooth.is_pixel_font=false");

    // ---------------- ③ 1:1 texel 覆盖（排版层，字号 = 设计字号） ----------------
    banner("M72-03③ 1:1 texel 覆盖：整数定位才成立，分数偏移必破");
    let mesh = TextLayout::build(
        PROBE_TEXT,
        [0.0, 0.0],
        TextStyle {
            scale: 1.0,
            ..Default::default()
        },
        &mut atlas,
    )
    .map_err(|error| format!("排版失败：{error:?}"))?;
    if !mesh.bitmap {
        return Err(
            "点阵字体的 mesh 没标 bitmap=true——渲染侧会按 MSDF（Linear + smoothstep）画它".into(),
        );
    }
    let atlas_size = (atlas.width(), atlas.height());
    let quads = quads_from_text_mesh(&mesh, atlas_size);
    if quads.len() != PROBE_TEXT.chars().count() {
        return Err(format!(
            "探针四边形 {} 个 != 字数 {}",
            quads.len(),
            PROBE_TEXT.chars().count()
        )
        .into());
    }
    let loose: Vec<String> = quads
        .iter()
        .filter(|quad| !quad.corners_integral(0.0))
        .map(|quad| format!("{quad:?}"))
        .collect();
    if !loose.is_empty() {
        return Err(format!(
            "{} 个字形四边形的边角不在整数设备像素上：{loose:?}",
            loose.len()
        )
        .into());
    }
    let warped: Vec<String> = quads
        .iter()
        .filter(|quad| quad.device_w() != quad.texel_w() || quad.device_h() != quad.texel_h())
        .map(|quad| format!("{quad:?}"))
        .collect();
    if !warped.is_empty() {
        return Err(format!(
            "{} 个字形的设备尺寸 != texel 数（12px 档 tex_scale 必须是 1，否则 1:1 覆盖无从谈起）：{warped:?}",
            warped.len()
        )
        .into());
    }
    for quad in &quads {
        let counts = sample_counts(quad, 0.0);
        let min = counts.iter().copied().min().unwrap_or(0);
        let max = counts.iter().copied().max().unwrap_or(0);
        if min != 1 || max != 1 {
            return Err(format!(
                "1:1 覆盖失败：{quad:?} 逐 texel 采样次数 min={min} max={max}（期望恒为 1）——\
                 采两次=字形抖一格，采不到=少一列笔画"
            )
            .into());
        }
    }
    println!(
        "  {} 个字形（{PROBE_TEXT}）在 12px 下逐 texel 采样次数恒为 1，四边角全整数",
        quads.len()
    );
    // 反面对照：吸附到底有没有在起作用，用**非整数倍字号**测。实测结论（推翻了 plan 里
    // "去掉整数吸附→12px 直方图出灰阶"的设想）：
    //   * 12px 档 `tex_scale=1`，bounds/advance 本来就是整数，吸附是个 no-op——把 `round()`
    //     删掉这条 §③a 也不会红；而且 1:1 的 Nearest 采样对**整块平移**是置换不变的（每个
    //     texel 仍恰好被采一次，只是内容整体挪一格），所以"分数偏移必破"在 1:1 档不成立。
    //   * 真正吃吸附的是 `tex_scale≠1` 的档（14px = 7/6）：不吸附时每个字形的 pen/宽高落在
    //     分数上，同一份点阵会被切成"这个字 14px、那个字 13.99px"的抖动宽度；吸附后边角
    //     全部回到整数设备像素。下面这条就是按这个口径钉的。
    let upscaled = TextLayout::build(
        PROBE_TEXT,
        [0.5, 0.25],
        TextStyle {
            scale: 14.0 / 12.0,
            ..Default::default()
        },
        &mut atlas,
    )
    .map_err(|error| format!("14px 排版失败：{error:?}"))?;
    let up_quads = quads_from_text_mesh(&upscaled, atlas_size);
    let up_loose: Vec<String> = up_quads
        .iter()
        .filter(|quad| !quad.corners_integral(0.0))
        .map(|quad| format!("{quad:?}"))
        .collect();
    if !up_loose.is_empty() {
        return Err(format!(
            "14px 档有 {} 个字形的边角不在整数设备像素上：{up_loose:?}——整数吸附没生效，\
             同一个点阵会被排出宽窄不一的笔画",
            up_loose.len()
        )
        .into());
    }
    // 同一档下 1:1 覆盖必然破（设备 14px 盖 12 texel → 必有 texel 被采两次）。这条不是
    // "门要放过的东西"，而是采样模型有牙齿的证据，也是 M72-10 要把字号统一到整数倍的原因。
    let mut doubled = 0usize;
    let mut starved = 0usize;
    for quad in &up_quads {
        let counts = sample_counts(quad, 0.0);
        doubled += counts.iter().filter(|count| **count > 1).count();
        starved += counts.iter().filter(|count| **count == 0).count();
    }
    if doubled == 0 {
        return Err(format!(
            "14px 档（设备 {}x 对 12 texel）一个 texel 都没被采两次——采样模型看不出非 1:1，\
             §③a 那条「恒为 1」就没有牙齿",
            up_quads[0].device_w()
        )
        .into());
    }
    println!(
        "  反面对照：14px 档（tex_scale=7/6）边角仍全整数（吸附生效），但逐 texel 出现\
         采两次={doubled}、采不到={starved}——非整数倍字号会重排点阵，字号口径归 M72-10",
    );

    // ---------------- ② 真运行时一帧：阈值档接线 + 二值覆盖 ----------------
    banner("M72-03② 真跑一帧：文字必须走 sdf=2 阈值档，用到的图集 texel 只能 0/255");
    let mut runtime = RuntimeScene::new(Some(project.as_path()))?;
    frame(&mut runtime)?;
    let scene = runtime.vertices(VIEW_W, VIEW_H)?;
    let font_draws: Vec<(usize, usize)> = scene
        .draws
        .iter()
        .filter(|draw| draw.sdf)
        .map(|draw| (draw.start as usize, draw.count as usize))
        .collect();
    if font_draws.is_empty() {
        return Err("本帧没有任何文字绘制段——门没有证据（场景没起来？还是文字全被裁掉了？)".into());
    }
    let ragged: Vec<usize> = font_draws
        .iter()
        .filter(|(_, count)| count % 6 != 0)
        .map(|(_, count)| *count)
        .collect();
    if !ragged.is_empty() {
        return Err(format!(
            "文字绘制段顶点数 {ragged:?} 不是 6 的倍数——四边形发序被打断，后面所有 UV 统计都不可信"
        )
        .into());
    }
    let mut draw_paths: Vec<&str> = scene
        .draws
        .iter()
        .filter(|draw| draw.sdf)
        .map(|draw| path_of(&scene, draw.texture).unwrap_or("<缺图>"))
        .collect();
    draw_paths.sort_unstable();
    draw_paths.dedup();
    if draw_paths.len() != 1 || draw_paths[0] != FONT_ATLAS_TEXTURE_PATH {
        return Err(format!(
            "文字绘制段的图集路径 = {draw_paths:?}，期望只有 {FONT_ATLAS_TEXTURE_PATH}（工程用默认字体）"
        )
        .into());
    }
    let not_bitmap = scene
        .draws
        .iter()
        .filter(|draw| draw.sdf && !draw.bitmap)
        .count();
    if not_bitmap != 0 {
        return Err(format!(
            "{not_bitmap} 个文字绘制段没标 bitmap=true → 渲染器会给它 Linear 采样（`passes.rs:518`），点阵会被糊掉"
        )
        .into());
    }
    if scene.draws.iter().any(|draw| draw.sdf && draw.vector) {
        return Err("文字绘制段同时被标成 vector——三条档互斥，混标说明 sdf 判定串了".into());
    }
    let text_sdf: Vec<u32> = font_draws
        .iter()
        .flat_map(|(start, count)| scene.vertices[*start..*start + *count].iter())
        .map(|vertex| vertex.sdf)
        .collect();
    let not_two = text_sdf.iter().filter(|value| **value != 2).count();
    if not_two != 0 {
        let mut distinct_values = text_sdf.clone();
        distinct_values.sort_unstable();
        distinct_values.dedup();
        return Err(format!(
            "{} 个文字顶点 sdf={distinct_values:?}（期望恒为 2）——这一档由 `text_to_ndc_with_offset` 按 \
             mesh.bitmap 决定，出现 1 说明仍有文字在走 MSDF smoothstep",
            not_two
        )
        .into());
    }
    if !scene.vector_vertices.is_empty() {
        return Err(format!(
            "矢量通道本帧有 {} 个顶点——像素字体必须被 `should_render_vector` 挡在矢量档之外",
            scene.vector_vertices.len()
        )
        .into());
    }
    let frame_atlas = scene
        .font_atlas
        .as_ref()
        .ok_or("本帧 SceneMesh 没带字体图集像素（直方图要用的就是它）")?;
    if frame_atlas.pixels.is_empty() {
        return Err("字体图集 pixels 为空（只在变化时带；这帧没上传过？)".into());
    }
    if !scene.custom_font_atlases.is_empty() {
        return Err(format!(
            "本帧另有 {} 个自定义字体图集，直方图口径会混进别的字体",
            scene.custom_font_atlases.len()
        )
        .into());
    }
    let text_quads: Vec<Quad> = font_draws
        .iter()
        .flat_map(|(start, count)| scene.vertices[*start..*start + *count].chunks_exact(6))
        .map(|hexagon| {
            Quad::from_corners(
                to_device_px(hexagon[0].position),
                to_device_px(hexagon[1].position),
                to_device_px(hexagon[5].position),
                hexagon[0].uv,
                hexagon[1].uv,
                hexagon[5].uv,
                (frame_atlas.width, frame_atlas.height),
            )
        })
        .collect();
    let fractional = text_quads
        .iter()
        .filter(|quad| !quad.corners_integral(1.0e-3))
        .count();
    if fractional != 0 {
        return Err(format!(
            "{fractional} 个真实 UI 四边形的边角落在次像素上（容差 1e-3）——整数吸附没生效，Nearest 采样会抖格"
        )
        .into());
    }
    // 逐 quad 只统计本帧真被用到的 texel，不数整张图集的空白。
    let mut grey = 0usize;
    let mut lit = 0usize;
    let mut dark = 0usize;
    let mut alpha_off = 0usize;
    let mut exact = 0usize;
    let mut scaled = 0usize;
    for quad in &text_quads {
        let left = quad.tx0.floor().max(0.0) as u32;
        let top = quad.ty0.floor().max(0.0) as u32;
        let right = quad.tx1.ceil().min(frame_atlas.width as f32) as u32;
        let bottom = quad.ty1.ceil().min(frame_atlas.height as f32) as u32;
        let mut cell = Vec::new();
        for y in top..bottom {
            for x in left..right {
                let offset = ((y * frame_atlas.width + x) * 4) as usize;
                cell.extend_from_slice(&frame_atlas.pixels[offset..offset + 4]);
            }
        }
        let (g, l, d, a) = coverage_histogram(&cell);
        grey += g;
        lit += l;
        dark += d;
        alpha_off += a;
        let one_to_one = (quad.device_w() - quad.texel_w()).abs() < 0.5
            && (quad.device_h() - quad.texel_h()).abs() < 0.5;
        if one_to_one {
            let counts = sample_counts(quad, 0.0);
            if counts.iter().all(|count| *count == 1) {
                exact += 1;
            }
        } else {
            scaled += 1;
        }
    }
    println!(
        "  {} 个 UI 字形四边形：亮 texel={lit} 全暗={dark} 灰阶={grey} alpha!=255={alpha_off}；\
         1:1 档 {exact} 个，非整数倍字号 {scaled} 个",
        text_quads.len()
    );
    if grey != 0 || alpha_off != 0 {
        return Err(format!(
            "点阵图集里出现 {grey} 个灰阶 texel、{alpha_off} 个 alpha!=255 的 texel——硬边档只能是 0 或 255"
        )
        .into());
    }
    if lit == 0 {
        return Err("本帧文字一个亮 texel 都没有（全是空字形？）".into());
    }
    if exact == 0 {
        return Err(format!(
            "本帧 {} 个四边形没有一个落在 1:1 档（全是非整数倍字号）——真帧上就没验到过\
             硬边覆盖，只验到二值；字号口径见 §未完成项（M72-10 统一按整数倍排）",
            text_quads.len()
        )
        .into());
    }
    // 反面对照：被替换掉的 Noto 在 MSDF 档、同一口径统计 → 灰阶必须成片。
    let smooth_rows: Vec<(char, usize, usize)> = PROBE_TEXT
        .chars()
        .map(|character| {
            let cell = glyph_cell(&mut smooth_atlas, character).expect("Noto 字形");
            let (grey, lit, _, _) = coverage_histogram(&cell);
            (character, grey, lit)
        })
        .collect();
    if smooth_rows.iter().any(|(_, greys, _)| *greys == 0) {
        return Err(format!(
            "MSDF 对照档没出灰阶：{smooth_rows:?}——那 §② 的二值判据就只是在测笔画多少，不是测档位"
        )
        .into());
    }
    let mut subset_for_hist = FontAtlas::load(&subset)?;
    let subset_rows: Vec<(char, usize, usize)> = PROBE_TEXT
        .chars()
        .map(|character| {
            let cell = glyph_cell(&mut subset_for_hist, character).expect("点阵字形");
            let (grey, lit, _, _) = coverage_histogram(&cell);
            (character, grey, lit)
        })
        .collect();
    if subset_rows
        .iter()
        .any(|(_, greys, lit)| *greys != 0 || *lit == 0)
    {
        return Err(format!("点阵档不干净：{subset_rows:?}").into());
    }
    println!("  反面对照：Noto/MSDF 档 (char, 灰阶, 全亮) = {smooth_rows:?}");
    println!("            点阵档 (char, 灰阶, 全亮) = {subset_rows:?}");

    // ---------------- ④ 缺字契约 ----------------
    banner("M72-03④ 缺字：工程用到的每个字都要有非空点阵");
    let collected = collect_project_chars(&project);
    let chars_txt = fs::read_to_string(project.join("chars.txt")).unwrap_or_default();
    let mut wanted: Vec<char> = collected.chars().chain(chars_txt.chars()).collect();
    wanted.retain(|character| !character.is_whitespace());
    wanted.sort_unstable();
    wanted.dedup();
    let mut warmed_atlas = FontAtlas::load(&subset)?;
    let mut missing = Vec::new();
    let mut empty = Vec::new();
    for character in &wanted {
        if !warmed_atlas.covers(*character) {
            missing.push(*character);
            continue;
        }
        let cell = glyph_cell(&mut warmed_atlas, *character)
            .map_err(|error| format!("{character:?}: {error}"))?;
        let (_, lit, _, _) = coverage_histogram(&cell);
        if lit == 0 {
            empty.push(*character);
        }
    }
    println!(
        "  用到的字符 {} 个（char_collect 贡献 {}）：缺字 {}、空字模 {}",
        wanted.len(),
        collected.chars().count(),
        missing.len(),
        empty.len()
    );
    if !missing.is_empty() || !empty.is_empty() {
        // 缺字的两种成因处置完全相反，混在一条提示里就会让人白跑子集：
        // 全量字体里有 → 忘了重跑 `subset_project_font`；全量字体里也没有 → 这个码位
        // 压根不在该点阵字体的收字范围内，只能改文案（实测踩过：`›` `−` `✓` `鳜` 四个
        // 全是全量就没有的，重跑多少遍子集都不会有）。
        let absent_from_source: String = missing
            .iter()
            .copied()
            .filter(|character| !full_atlas.covers(*character))
            .collect();
        let stale_subset: String = missing
            .iter()
            .copied()
            .filter(|character| full_atlas.covers(*character))
            .collect();
        return Err(format!(
            "缺字=\"{}\" 空字模=\"{}\"——像素字体不带后备字体（混排版面更露馅），超出子集的字符只能画方框。\
             全量字体也没有（只能改文案，见 docs/49 收字白名单）：\"{absent_from_source}\"；\
             全量有而子集没有（重跑 subset_project_font）：\"{stale_subset}\"",
            String::from_iter(missing.iter().take(20)),
            String::from_iter(empty.iter().take(20)),
        )
        .into());
    }
    // 反面对照：真缺的字必须被 `covers()` 判出来，且仍画得出可见方框（缺字不许静默消失）。
    let absent = '🙂';
    if warmed_atlas.covers(absent) {
        return Err(
            format!("反面对照失效：{absent} 竟被子集覆盖——`covers()` 这条判据没有牙齿").into(),
        );
    }
    let placeholder = glyph_cell(&mut warmed_atlas, absent).expect("占位字形");
    let (_, placeholder_lit, _, _) = coverage_histogram(&placeholder);
    if placeholder_lit == 0 {
        return Err(
            "缺字占位方框一个亮 texel 都没有——缺字会画成完全透明的空洞，屏幕上看不出少字".into(),
        );
    }
    println!("  反面对照：{absent} 判为缺字，占位方框有 {placeholder_lit} 个亮 texel（看得见）");

    // ---------------- ⑤ 图集预算 ----------------
    banner("M72-03⑤ 预算：全部工程字一次性预热");
    let started = Instant::now();
    // §④ 已经逐字取过 metrics，这里必须换一份全新图集，否则预热数与耗时都归零。
    let mut budget_atlas = FontAtlas::load(&subset)?;
    let warmed = budget_atlas.prewarm(wanted.iter().copied());
    let elapsed = started.elapsed().as_secs_f32();
    // 单元尺寸从两份图集**实测**（MSDF 那侧取对照字体，raster_scale=96），不按内部常量推算：
    // gutter 是多少由图集自己说，这条预算数才站得住。
    let probe_char = PROBE_TEXT.chars().next().unwrap_or('山');
    let pixel_metrics = budget_atlas
        .try_glyph_metrics(probe_char)
        .map_err(|error| format!("{probe_char:?} 点阵字形：{error:?}"))?;
    let msdf_metrics = smooth_atlas
        .try_glyph_metrics(probe_char)
        .map_err(|error| format!("{probe_char:?} MSDF 字形：{error:?}"))?;
    let cell_side = |rect: &[f32; 4]| (rect[2] - rect[0], rect[3] - rect[1]);
    let cell_area = |rect: &[f32; 4]| cell_side(rect).0 * cell_side(rect).1;
    let (pw, ph) = cell_side(&pixel_metrics.cell_rect);
    let (mw, mh) = cell_side(&msdf_metrics.cell_rect);
    println!(
        "  {} 字一次性预热（新建 {warmed}，耗时 {elapsed:.2}s）→ 图集 {}x{}；\
         点阵汉字单元 {pw:.0}x{ph:.0} texel，MSDF 档同字 {mw:.0}x{mh:.0}（{} 倍面积）",
        wanted.len(),
        budget_atlas.width(),
        budget_atlas.height(),
        (cell_area(&msdf_metrics.cell_rect) / cell_area(&pixel_metrics.cell_rect)).round() as u32,
    );
    if budget_atlas.width() * budget_atlas.height() > 1024 * 1024 {
        return Err(format!(
            "全量工程字把图集顶到 {}x{}（超过 1024²）——M72-10 换全 UI 之前要先看清这个余量",
            budget_atlas.width(),
            budget_atlas.height()
        )
        .into());
    }

    println!("\njhyt_m72_pixel_font_ok");
    Ok(())
}
