//! Command-line argument parsing, project discovery and resource-integrity
//! verification, used by both interactive and `--check` modes.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// P3-R201/R204：画质预设（`--quality default|medium|low`）。
///
/// 预设是一次打开若干**已实测**档位的组合；逐项开关仍在，预设只覆盖它包含的字段
/// （构建链里预设排在逐项开关之后）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Quality {
    /// 逐项开关的旧行为（默认；GPU 路径逐字节不变）。
    #[default]
    Default,
    /// planar 1/4（R196）+ 水面廉价光照（R197）；阴影档与地形细节档都不动
    /// （地形廉价档的视觉代价未定档，R203 §21.489 决定不并入预设）。
    Medium,
    /// medium + 点光 cube 阴影 512（R200）。
    Low,
}

impl Quality {
    /// 稳定短名：启动日志与帧剖面 JSON 的自描述字段共用（P3-R205），两处不许各写一份。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

#[derive(Debug, Default)]
pub struct Arguments {
    pub project: Option<PathBuf>,
    pub log_filter: Option<String>,
    pub title: Option<String>,
    /// 发布包模式：目录为 assetc package 生成的发布资源目录，启动期严格校验资源。
    pub pack: bool,
    /// 仅校验资源完整性后退出（不弹窗口），供 CI/打包自测使用。
    pub check: bool,
    /// 关闭 CPU 视锥剔除（M8-08 基线回退）。
    pub no_cull: bool,
    /// 关闭实例化（M8-08 基线回退）。
    pub no_instance: bool,
    /// P3-R184：开发期归因开关——跳过网格路径含该子串的 3D drawable（A/B 量某类网格的 GPU 成本）。
    pub skip_mesh: Option<String>,
    /// P3-R185：关闭不透明前到后排序（early-Z 优化的 A/B 回滚档）。
    pub no_depth_sort: bool,
    /// P3-R270：关闭 GPU 位姿烘焙的持久输入缓冲 + 共享 compute 编码器（A/B 回滚档）。
    pub no_bake_cache: bool,
    /// P3-R191：关闭平面反射的镜像视锥剔除（A/B 回滚档）。
    pub no_planar_cull: bool,
    /// P3-R194：关闭点光阴影逐面方向锥剔除（A/B 回滚档）。
    pub no_point_face_cull: bool,
    /// P3-R196：平面反射目标分辨率除数（2 = 默认半分辨率；4 = 四分之一档 A/B）。
    pub planar_divisor: u32,
    /// P3-R197：全部水面强制廉价光照档（A/B 用；默认只按 spec 的 `cheap=1`）。
    pub water_cheap: bool,
    /// P3-R203：全部地形强制廉价细节档（`--terrain-cheap`，A/B 用；默认只按 spec 的 `cheap=1`）。
    pub terrain_cheap: bool,
    /// P3-R277：全部普通网格强制廉价着色档（`--crowd-cheap`，A/B 用；默认全价 PBR）。
    pub crowd_cheap: bool,
    /// P3-R206：3D 最大绘制距离（米，0 = 关闭）。超出者并入剔除计数，是植被 LOD 的第一级
    /// 硬截断档（低端机用；默认关闭 = 像素逐字节不变）。
    pub draw_distance: u32,
    /// P3-R206 分类档：只对网格路径含该子串的 drawable 生效（`--draw-distance-filter`）。
    /// 植被 LOD 用它只掐草，不连带蚁/地形；`None` = 全体（默认）。
    pub draw_distance_filter: Option<String>,
    /// P3-R248：方向光 CSM 链的分类排除档（`--shadow-caster-skip`）：样本名或网格路径含
    /// 该子串的 drawable 不投影（静态合批组只按路径判），主视图照画。`None` = 不排除
    /// （默认 = 像素逐字节不变）。
    pub shadow_caster_skip: Option<String>,
    /// P3-R265：点光链专用分类排除档（`--point-shadow-caster-skip`）：样本名或网格路径含
    /// 该子串的 drawable 不进点光 6 面链，方向光 CSM 与主视图照画。`None` = 不排除
    /// （默认 = 像素逐字节不变）。
    pub point_shadow_caster_skip: Option<String>,
    /// P3-R256：阴影投射者的距离预算（`--shadow-caster-distance`，米，0 = 关闭）。
    /// 超过该距离的 drawable 不进方向光与点光阴影链，主视图照画（观感取舍档；
    /// 默认关闭 = 像素逐字节不变）。
    pub shadow_caster_distance: u32,
    /// P3-R207：深度加权水面折射增益（`--water-refraction`，单位 uv / 米光程 / 掠射角）。
    /// `0` = 关闭 = 旧的固定偏移（像素逐字节不变）；真实量级约 `0.0008`。
    pub water_refraction: f32,
    /// P3-R208：水下视角的水-气界面强度（`--underwater-view`，0..=1）。`0` = 关闭 =
    /// 只有 P3-R173 的整帧吸收（像素逐字节不变）；1 = 界面像素完全换成 Snell 窗口/全反射镜像。
    pub underwater_view: f32,
    /// P3-R241：水面折射的屏幕空间 Snell 步进次数（`--water-refract-steps`，0..=32）。
    /// `0` = 关闭 = R207 的偏移近似（像素逐字节不变）；> 0 = 沿折射方向在水下找几何交点。
    pub water_refract_steps: u32,
    /// P3-R243：启动期预热 GPU 粒子管线（`--prewarm-particles`）。默认关 = 第一次画粒子
    /// 时才编译（P3-R02 的启动预算）；开 = 启动时把 LDR/HDR 两条都编好，第一股连续流
    /// 不再付管线编译的尖峰。像素与帧序都不变，只挪了编译时刻。
    pub prewarm_particles: bool,
    /// P3-R246：实例位姿交给渲染器现场求值（`--gpu-motion`）。默认关 = 宿主每帧求
    /// `matrices_at` 并把 N 份矩阵交给渲染器（旧行为，像素/帧序逐字节不变）；开 = 带运动
    /// 的批次**跳过** CPU 求值，批次登记给渲染器 compute 烘焙。
    pub gpu_motion: bool,
    /// P3-R250：GPU 位姿烘焙的逐实例距离剔除半径（`--gpu-motion-cull`，米）。`0` = 关闭
    /// （默认，逐矩阵不变）；> 0 = 离相机超过该距离的实例写零矩阵（只对 GPU 烘焙批次生效）。
    /// 观感取舍档：贴近切面的实例可能整只消失，不进 `--quality`。
    pub gpu_motion_cull: f32,
    /// P3-R255：GPU 位姿烘焙的逐实例视锥剔除（`--gpu-motion-frustum-cull`）。默认关 = 不判、
    /// 不传视锥参数（逐矩阵不变）；开 = 主视图看不见的实例连实例槽位都不领（与距离剔除
    /// 同一条取舍，阴影/planar 共享矩阵 ⇒ 全链一致消失）。只对 GPU 烘焙批次生效。
    pub gpu_motion_frustum_cull: bool,
    /// P3-R266：GPU 位姿烘焙的远程密度档（`--gpu-motion-thin <米>:<k>`）。`None` = 关闭
    /// （默认，逐矩阵不变）；`Some((米, k))` = 离相机超过该距离的实例只保留批次内
    /// `index % k == 0` 的那些。观感取舍档：远区变稀，近区一只不动；只对 GPU 烘焙批次生效。
    pub gpu_motion_thin: Option<(f32, u32)>,
    /// P3-R267：第二远带 `(米, k)`（`--gpu-motion-thin <近>:<k>,<远>:<k>` 的第二段）。
    /// 只在第一带已开、且半径严格更大时生效；与第一带取**交集**。`None` = 关（默认）。
    pub gpu_motion_thin_far: Option<(f32, u32)>,
    /// P3-R209：剖切口的实体封口深度（`--clip-cap`，米）。`0` = 关闭 = 只在切面贴 0.08 m
    /// 薄带（P3-R54 的旧行为，像素逐字节不变）；> 0 = 切得再深也一路画实体截面。
    pub clip_cap: f32,
    /// P3-R211：封口也吃到单面网格（`--clip-cap-both-sides`）。默认关 = 只有双面网格（水面、
    /// 带裙边的地形、`double_sided` 材质）的切口能画实体截面；开 = 挂了会话裁剪平面的帧里，
    /// 主网格通道对全部网格改用双面管线，单面 glb 壳（巢丘、道具）也被切成实心剖面。
    pub clip_cap_both_sides: bool,
    /// P3-R198：方向光 CSM 阴影图边长（512/1024/2048/4096，默认 2048 = 旧行为）。
    pub shadow_resolution: u32,
    /// P3-R199：方向光 CSM 实际渲染的级联层数（1..=4，默认 4 = 旧行为）。
    pub shadow_cascades: u32,
    /// P3-R200：点光 cube 阴影面的边长（512/1024，默认 1024 = 旧行为）。
    pub point_shadow_resolution: u32,
    /// P3-R202：点光阴影盏数预算（1..=8，默认 8 = 旧行为）——只给相机焦点最近的 N 盏录阴影。
    pub point_shadow_lights: u32,
    /// P3-R201/R204：画质预设（`--quality default|medium|low`）；默认 = 逐项开关的旧行为。
    pub quality: Quality,
    /// 加载集观测：把本次运行的资源读取按场景上下文写成 JSON（`docs/67` §5）。
    pub trace_loads: Option<PathBuf>,
    /// 帧调度分档（M74-02 P0 度量闸门）：退出时把每系统 p50/p99、每 stage 合计与
    /// "最慢 stage 占帧比"写成 JSON。`None` = 不开（零成本）。
    pub frame_profile: Option<PathBuf>,
    /// P3-R05-4：开发期 shader 源覆盖目录（`--shader-dir` 或 `RUSTLUA_SHADER_DIR`）。
    /// `None` = **不监听**、用内置源——这是发布默认形态，运行时不该有任何 shader 文件 IO。
    pub shader_dir: Option<PathBuf>,
    /// P3-R236：确定性帧档——每帧恰好推进一个固定步，不读墙钟 ⇒ 帧号与模拟步一一对应，
    /// 两次运行到同一帧号的世界状态相同（默认关 = 墙钟档，行为不变）。
    pub fixed_step: bool,
    /// P3-R236：把着色器时间钉在给定秒数（`--freeze-clock`）。`None` = 渲染钟（默认不变）；
    /// 钉住后水波/云/后期颗粒的相位在两次运行里一致 ⇒ 逐像素对账成立。
    pub freeze_clock: Option<f32>,
}

pub fn parse_arguments() -> Result<Arguments, String> {
    parse_arguments_from(env::args().skip(1))
}

/// 与 [`parse_arguments`] 同源，参数从迭代器取（单元测试用；`env::args` 进程级不可注入）。
pub fn parse_arguments_from(values: impl IntoIterator<Item = String>) -> Result<Arguments, String> {
    // P3-R196：默认半分辨率（默认值的 0 是占位，不能直接当除数）。
    let mut args = Arguments {
        planar_divisor: 2,
        shadow_resolution: 2048,
        shadow_cascades: 4,
        point_shadow_resolution: 1024,
        point_shadow_lights: 8,
        ..Arguments::default()
    };
    let mut values = values.into_iter();
    while let Some(value) = values.next() {
        match value.as_str() {
            "--project" => {
                args.project = Some(PathBuf::from(
                    values.next().ok_or("--project requires a path")?,
                ))
            }
            "--pack" => {
                args.project = Some(PathBuf::from(
                    values.next().ok_or("--pack requires a path")?,
                ));
                args.pack = true;
            }
            "--check" => args.check = true,
            "--no-cull" => args.no_cull = true,
            "--no-instance" => args.no_instance = true,
            "--no-depth-sort" => args.no_depth_sort = true,
            "--no-bake-cache" => args.no_bake_cache = true,
            "--no-planar-cull" => args.no_planar_cull = true,
            "--no-point-face-cull" => args.no_point_face_cull = true,
            "--water-cheap" => args.water_cheap = true,
            "--crowd-cheap" => args.crowd_cheap = true,
            "--clip-cap-both-sides" => args.clip_cap_both_sides = true,
            "--prewarm-particles" => args.prewarm_particles = true,
            "--gpu-motion" => args.gpu_motion = true,
            "--gpu-motion-frustum-cull" => args.gpu_motion_frustum_cull = true,
            "--gpu-motion-cull" => {
                let value = values
                    .next()
                    .ok_or("--gpu-motion-cull requires meters")?
                    .parse::<f32>()
                    .map_err(|_| "--gpu-motion-cull must be a number (0 = off)".to_owned())?;
                args.gpu_motion_cull = if value.is_finite() && value > 0.0 {
                    value
                } else {
                    0.0
                };
            }
            "--gpu-motion-thin" => {
                let value = values
                    .next()
                    .ok_or("--gpu-motion-thin requires meters:stride (e.g. 60:4 or 20:4,80:16)")?;
                let bands = value.split(',').collect::<Vec<_>>();
                if bands.len() > 2 {
                    return Err(
                        "--gpu-motion-thin takes at most two bands (near:stride,far:stride)"
                            .to_owned(),
                    );
                }
                let parse_band = |band: &str| -> Result<Option<(f32, u32)>, String> {
                    let (meters, stride) = band
                        .split_once(':')
                        .ok_or("--gpu-motion-thin must be meters:stride (e.g. 60:4)".to_owned())?;
                    let meters = meters.parse::<f32>().map_err(|_| {
                        "--gpu-motion-thin meters must be a number (0 = off)".to_owned()
                    })?;
                    let stride = stride.parse::<u32>().map_err(|_| {
                        "--gpu-motion-thin stride must be a non-negative integer (1 = off)"
                            .to_owned()
                    })?;
                    Ok(if meters.is_finite() && meters > 0.0 && stride >= 2 {
                        Some((meters, stride))
                    } else {
                        None
                    })
                };
                let near = parse_band(bands[0])?;
                let far = match bands.get(1) {
                    Some(band) => parse_band(band)?,
                    None => None,
                };
                // 第二带必须严格更远才有意义；写反就归一为关（第一带照旧）。
                let far = match (near, far) {
                    (Some((near_meters, _)), Some((far_meters, stride)))
                        if far_meters > near_meters =>
                    {
                        Some((far_meters, stride))
                    }
                    _ => None,
                };
                args.gpu_motion_thin = near;
                args.gpu_motion_thin_far = far;
            }
            "--terrain-cheap" => args.terrain_cheap = true,
            "--draw-distance" => {
                let value = values.next().ok_or("--draw-distance requires meters")?;
                args.draw_distance = value.parse().map_err(|_| {
                    "--draw-distance must be a non-negative integer (meters)".to_owned()
                })?;
            }
            "--draw-distance-filter" => {
                let value = values
                    .next()
                    .ok_or("--draw-distance-filter requires a mesh-path substring")?;
                args.draw_distance_filter = Some(value);
            }
            "--shadow-caster-skip" => {
                let value = values
                    .next()
                    .ok_or("--shadow-caster-skip requires a name-or-mesh-path substring")?;
                args.shadow_caster_skip = Some(value);
            }
            "--point-shadow-caster-skip" => {
                let value = values
                    .next()
                    .ok_or("--point-shadow-caster-skip requires a name-or-mesh-path substring")?;
                args.point_shadow_caster_skip = Some(value);
            }
            "--shadow-caster-distance" => {
                let value = values
                    .next()
                    .ok_or("--shadow-caster-distance requires meters")?;
                args.shadow_caster_distance = value.parse().map_err(|_| {
                    "--shadow-caster-distance must be a non-negative integer (meters)".to_owned()
                })?;
            }
            "--water-refraction" => {
                let value = values
                    .next()
                    .ok_or("--water-refraction requires a gain")?
                    .parse::<f32>()
                    .map_err(|_| "--water-refraction must be a number (0 = off)".to_owned())?;
                args.water_refraction = if value.is_finite() && value > 0.0 {
                    value
                } else {
                    0.0
                };
            }
            "--underwater-view" => {
                let value = values
                    .next()
                    .ok_or("--underwater-view requires a 0..=1 strength")?
                    .parse::<f32>()
                    .map_err(|_| "--underwater-view must be a number (0 = off)".to_owned())?;
                args.underwater_view = if value.is_finite() && value > 0.0 {
                    value.min(1.0)
                } else {
                    0.0
                };
            }
            "--water-refract-steps" => {
                let value = values
                    .next()
                    .ok_or("--water-refract-steps requires a step count")?;
                args.water_refract_steps = value
                    .parse::<u32>()
                    .map_err(|_| {
                        "--water-refract-steps must be a non-negative integer (0 = off)".to_owned()
                    })?
                    .min(32);
            }
            "--clip-cap" => {
                let value = values
                    .next()
                    .ok_or("--clip-cap requires a depth in meters")?
                    .parse::<f32>()
                    .map_err(|_| "--clip-cap must be a number (0 = off)".to_owned())?;
                args.clip_cap = if value.is_finite() && value > 0.0 {
                    value
                } else {
                    0.0
                };
            }
            "--planar-divisor" => {
                let value = values
                    .next()
                    .ok_or("--planar-divisor requires an integer divisor")?;
                let divisor: u32 = value
                    .parse()
                    .map_err(|_| "--planar-divisor must be an integer".to_owned())?;
                if !(1..=8).contains(&divisor) {
                    return Err("--planar-divisor must be within 1..=8".to_owned());
                }
                args.planar_divisor = divisor;
            }
            "--shadow-resolution" => {
                let value = values
                    .next()
                    .ok_or("--shadow-resolution requires an integer edge")?;
                let resolution: u32 = value
                    .parse()
                    .map_err(|_| "--shadow-resolution must be an integer".to_owned())?;
                if ![512, 1024, 2048, 4096].contains(&resolution) {
                    return Err(
                        "--shadow-resolution must be one of 512, 1024, 2048, 4096".to_owned()
                    );
                }
                args.shadow_resolution = resolution;
            }
            "--shadow-cascades" => {
                let value = values
                    .next()
                    .ok_or("--shadow-cascades requires an integer")?;
                let cascades: u32 = value
                    .parse()
                    .map_err(|_| "--shadow-cascades must be an integer".to_owned())?;
                if !(1..=4).contains(&cascades) {
                    return Err("--shadow-cascades must be within 1..=4".to_owned());
                }
                args.shadow_cascades = cascades;
            }
            "--point-shadow-resolution" => {
                let value = values
                    .next()
                    .ok_or("--point-shadow-resolution requires an integer edge")?;
                let resolution: u32 = value
                    .parse()
                    .map_err(|_| "--point-shadow-resolution must be an integer".to_owned())?;
                if ![512, 1024].contains(&resolution) {
                    return Err("--point-shadow-resolution must be one of 512, 1024".to_owned());
                }
                args.point_shadow_resolution = resolution;
            }
            "--point-shadow-lights" => {
                let value = values
                    .next()
                    .ok_or("--point-shadow-lights requires an integer count")?;
                let lights: u32 = value
                    .parse()
                    .map_err(|_| "--point-shadow-lights must be an integer".to_owned())?;
                if !(1..=8).contains(&lights) {
                    return Err("--point-shadow-lights must be within 1..=8".to_owned());
                }
                args.point_shadow_lights = lights;
            }
            "--quality" => {
                let value = values
                    .next()
                    .ok_or("--quality requires default, medium or low")?;
                args.quality = match value.as_str() {
                    "low" => Quality::Low,
                    "medium" => Quality::Medium,
                    "default" => Quality::Default,
                    _ => return Err("--quality must be 'default', 'medium' or 'low'".to_owned()),
                };
            }
            "--skip-mesh" => {
                let filter = values
                    .next()
                    .ok_or("--skip-mesh requires a mesh path substring")?;
                if filter.is_empty() {
                    return Err("--skip-mesh substring must not be empty".to_owned());
                }
                args.skip_mesh = Some(filter);
            }
            "--log-filter" => {
                args.log_filter = Some(
                    values
                        .next()
                        .ok_or("--log-filter requires a tracing filter")?,
                )
            }
            "--title" => args.title = Some(values.next().ok_or("--title requires a title")?),
            "--trace-loads" => {
                args.trace_loads = Some(PathBuf::from(
                    values.next().ok_or("--trace-loads requires a path")?,
                ));
            }
            "--frame-profile" => {
                args.frame_profile = Some(PathBuf::from(
                    values.next().ok_or("--frame-profile requires a path")?,
                ));
            }
            "--shader-dir" => {
                args.shader_dir = Some(PathBuf::from(
                    values.next().ok_or("--shader-dir requires a path")?,
                ));
            }
            "--fixed-step" => args.fixed_step = true,
            "--freeze-clock" => {
                let value = values.next().ok_or("--freeze-clock requires seconds")?;
                let secs = value
                    .parse::<f32>()
                    .map_err(|_| "--freeze-clock must be a number (seconds)".to_owned())?;
                if !secs.is_finite() || secs < 0.0 {
                    return Err(
                        "--freeze-clock must be a finite, non-negative number of seconds"
                            .to_owned(),
                    );
                }
                args.freeze_clock = Some(secs);
            }
            "--help" | "-h" => {
                println!(
                    "Usage: game [--project <game-project-root>] [--pack <release-pack>] [--title <window-title>] [--check] [--log-filter <tracing-filter>]\n\
                     \n\
                     --project <root>  加载开发游戏工程（含 .cache 等本地工作目录）\n\
                     --pack <dir>      加载 assetc package 生成的发布资源目录（严格校验资源完整性）\n\
                     --title <title>   设置窗口标题；未指定时优先读取发布包 release.toml\n\
                     --check           仅校验资源完整性后退出（exit 0/1，不弹窗口）\n\
                     --skip-mesh <子串>  开发期归因开关：跳过网格路径含该子串的 3D drawable\n\
                                       （A/B 量某类网格的 GPU 成本；不给 = 不跳过）\n\
                     --no-depth-sort   关闭不透明前到后排序（P3-R185 的 A/B 回滚档）\n\
                     --no-bake-cache    关闭烘焙输入缓冲持久化与共享 compute 编码器（P3-R270 回滚档）\n\
                     --no-planar-cull  关闭平面反射的镜像视锥剔除（P3-R191 的 A/B 回滚档）\n\
                     --no-point-face-cull  关闭点光阴影逐面方向锥剔除（P3-R194 的 A/B 回滚档）\n\
                     --planar-divisor <n>  平面反射目标分辨率除数（默认 2 = 半分辨率；4 = 1/4 档）\n\
                     --water-cheap     全部水面强制廉价光照档（P3-R197 的 A/B 开关）\n\
                     --terrain-cheap   全部地形强制廉价细节档（P3-R203 的 A/B 开关）\n\
                     --draw-distance <米>  3D 最大绘制距离（P3-R206；0 = 关闭，默认不变；\n\
                                       超出者并入剔除计数，低端机植被 LOD 的硬截断档）\n\
                     --draw-distance-filter <子串>  距离档只作用于网格路径含该子串的实例\n\
                                       （P3-R206；默认全体；植被只掐草用 grass）\n\
                     --shadow-caster-skip <子串>  样本名或网格路径含该子串的 drawable 不进\n\
                                       方向光阴影链（P3-R248；静态组只按路径；主视图\n\
                                       照画；默认不排除）\n\
                     --shadow-caster-distance <米>  离相机超过该距离的 drawable 不进阴影链\n\
                                       （P3-R256；方向光与点光都掐，主视图照画；\n\
                                       0 = 关闭，默认不变）\n\
                     --point-shadow-caster-skip <子串>  只掐点光 6 面链：样本名或网格路径\n\
                                       含该子串的 drawable 不投点光阴影（P3-R265；\n\
                                       方向光 CSM 与主视图照画；默认不排除）\n\
                     --water-refraction <增益>  水面折射偏移按「光程 × 掠射角」加权\n\
                                       （P3-R207；0 = 关闭 = 旧的固定偏移，默认不变；\n\
                                       真实量级约 0.0008 uv/米）\n\
                     --underwater-view <0..=1>  水下视角把水-气界面换成 Snell 窗口 + 全反射镜像\n\
                                       （P3-R208；0 = 关闭，默认只有 P3-R173 的整帧吸收，\n\
                                       像素逐字节不变；只在相机入水时生效）\n\
                     --water-refract-steps <n>  水面折射沿 Snell 方向做屏幕空间步进\n\
                                       （P3-R241；0 = 关闭 = R207 的偏移近似，默认不变；\n\
                                       n = 1..32 次深度取样，未命中退回偏移落点）\n\
                     --clip-cap <米>   剖切口的实体封口深度（P3-R209；0 = 关闭 = 旧的 0.08 薄带，\n\
                                       默认不变；> 0 = 从切面向内这么深都一路画实体截面，\n\
                                       只对双面网格生效）\n\
                     --clip-cap-both-sides  封口也吃到单面网格（P3-R211；默认关）\n\
                                       开了以后，挂了会话裁剪平面的帧里主网格通道对全部网格改用\n\
                                       双面管线，单面 glb 壳也被切成实心剖面；没有裁剪面的帧零变化\n\
                     --prewarm-particles  启动期预热 GPU 粒子管线（P3-R243；默认关 = 第一次画\n\
                                       粒子时才编译；开 = LDR/HDR 两条都在启动时编好，\n\
                                       第一股连续流不再付编译尖峰，像素不变）\n\
                     --gpu-motion     实例位姿交给渲染器现场求值（P3-R246；默认关 = 宿主每帧
                                      求 CPU 矩阵；开 = 带运动的批次跳过 matrices_at，由 compute
                                      烘焙进实例缓冲；像素同 CPU 档，CPU 提取省掉 N 份矩阵）
                     --gpu-motion-cull <米>  逐实例距离剔除（P3-R250；默认关 = 0；只对
                                      --gpu-motion 烘焙的批次生效：离相机超过该距离的实例
                                      写零矩阵，主视图/阴影/planar 全链一致地消失）
                     --gpu-motion-frustum-cull  逐实例视锥剔除（P3-R255；默认关；只对
                                      --gpu-motion 烘焙的批次生效：主视图看不见的实例连
                                      实例槽位都不领，阴影/planar 里也一起消失）
                     --gpu-motion-thin <近>:<k>[,<远>:<k>]  远程密度档（P3-R266/R267；默认关；
                                      只对 --gpu-motion 烘焙的批次生效：离相机超过该米数的
                                      实例只留批次内 index%k==0 的那些，近区一只不动；
                                      第二带必须更远、且与第一带取交集；观感取舍档）
                     --quality <档>    画质预设：default（逐项开关）/ medium（planar 1/4 + 水面\n\
                                       廉价光照）/ low（medium + 点光 cube 阴影 512）\n\
                     --trace-loads <file>  采集资源加载集（按场景上下文）并写 JSON\n\
                     --frame-profile <file> 退出时把帧调度分档（每系统 p50/p99、每 stage 合计、\n\
                                       最慢 stage 占帧比）写成 JSON（M74-02 P0）\n\
                     --shader-dir <dir>    开发期 shader 源覆盖目录（同名 .wgsl 覆盖内置源，\n\
                                       运行中按 150ms 去抖热重载）；等价于环境变量 RUSTLUA_SHADER_DIR，\n\
                                       两者都不给时不监听、只用内置源\n\
                     --fixed-step      确定性帧档：每帧恰好推进一个固定步，不读墙钟\n\
                                       （P3-R236；帧号 ⇔ 模拟步一一对应，两次运行到同一帧号\n\
                                       的世界状态相同；默认关 = 墙钟档）\n\
                     --freeze-clock <秒>  把着色器时间钉在给定秒数（P3-R236；不给 = 渲染钟）；\n\
                                       钉住后水波/云/后期颗粒相位在两次运行里一致 ⇒ 逐像素对账成立"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    // 环境变量是参数的同义开关（脚本/CI 里比拼参数好注入）；两者都没给就是 `None`，
    // 运行时不监听覆盖目录——发布路径不该出现任何 shader 文件 IO。
    args.shader_dir = rustlua_gpu::shader_library::shader_override_dir(
        args.shader_dir,
        env::var_os(rustlua_gpu::shader_library::SHADER_DIR_ENV).as_deref(),
        None,
    );
    Ok(args)
}

/// When no `--project` is given, walks up from the executable to find a game
/// project: either a directory with its own `project.toml` (packaged layout) or
/// a sibling `game/project.toml` (workspace layout). Lets double-clicking the
/// exe launch the game instead of the diagnostic scene.
///
/// 顺序即优先级：显式 `--project` > `pack/`（发布包）> 检出目录里的示例工程。
/// 示例工程按 `examples/game_api`（API 演示工程）优先，其次 `examples`/`game`
/// （历史布局与外部游戏工程）。
pub fn find_project_root() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)?;
    let packaged = exe_dir.join("pack").join("project.toml");
    if packaged.is_file() {
        return packaged.parent().map(Path::to_path_buf);
    }
    for dir in exe_dir.ancestors() {
        let direct = dir.join("project.toml");
        if direct.is_file() {
            return Some(dir.to_path_buf());
        }
        for project_name in ["examples/game_api", "examples", "game"] {
            let project = dir.join(project_name).join("project.toml");
            if project.is_file() {
                return Some(dir.join(project_name));
            }
        }
    }
    None
}

pub fn resolve_window_title(project: Option<&Path>, explicit: Option<String>) -> String {
    if let Some(title) = explicit.filter(|title| !title.trim().is_empty()) {
        return title;
    }
    if let Some(title) = release_title() {
        return title;
    }
    if let Some(root) = project
        && let Ok(text) = fs::read_to_string(root.join("project.toml"))
        && let Some(name) = extract_key(&text, "name")
    {
        return name;
    }
    "RustLua Engine".to_owned()
}

fn release_title() -> Option<String> {
    let exe_dir = env::current_exe().ok()?.parent().map(Path::to_path_buf)?;
    let text = fs::read_to_string(exe_dir.join("release.toml")).ok()?;
    extract_key(&text, "publish_name")
}

/// 校验项目/发布包资源完整性：项目清单、主脚本、启动场景及场景引用的资源。
/// 返回缺失项列表（空表示完整）。
pub fn verify_project(root: &Path) -> Vec<String> {
    let mut missing = verify_project_inner(root);
    if let Err(error) = crate::splash::load(root) {
        missing.push(error);
    }
    if let Err(error) = crate::splash::load_icon(root) {
        missing.push(error);
    }
    missing
}

fn verify_project_inner(root: &Path) -> Vec<String> {
    let mut missing = Vec::new();
    let project_toml = root.join("project.toml");
    // 全走 `vfs`：发布包形态下 `project.toml` 是**骨架**（磁盘上有），但 `scripts/`、`scenes/`、
    // `assets/` 只在 RLB 容器里——用 `path.is_file()` / `fs::read` 会把它们全判成"缺失"
    // （批 A 落地时实测过：`game --pack <单包> --check` 当场红）。见 `docs/67` §8 第 2 项。
    if !rustlua_infra::vfs::is_file(&project_toml) {
        missing.push(format!("缺少项目清单 {}", project_toml.display()));
        return missing;
    }
    let Some(text) = read_text(&project_toml) else {
        missing.push(format!("无法读取项目清单 {}", project_toml.display()));
        return missing;
    };
    if let Some(script) = extract_key(&text, "main_script") {
        let path = root.join(script);
        if !rustlua_infra::vfs::is_file(&path) {
            missing.push(format!("缺少主脚本 {}", path.display()));
        }
    }
    if let Some(scene) = extract_key(&text, "startup_scene") {
        let path = root.join(scene);
        if !rustlua_infra::vfs::is_file(&path) {
            missing.push(format!("缺少启动场景 {}", path.display()));
        } else if let Some(scene_text) = read_text(&path) {
            for asset in extract_asset_refs(&scene_text) {
                let asset_path = root.join(&asset);
                if !rustlua_infra::vfs::is_file(&asset_path) {
                    let message = format!("场景引用的资源缺失 {}", asset);
                    if !missing.contains(&message) {
                        missing.push(message);
                    }
                }
            }
        }
    }
    // M20-01：脚本语法校验（导入期同一校验器）+ 领域数据校验（复用工程
    // 自身 validate_data.lua），让发布前检查覆盖启动期会失败的问题。
    missing.extend(verify_script_syntax(root));
    missing.extend(verify_domain_data(root));
    missing
}

// An optional image must not prevent startup; --check still reports it.
pub fn verify_startup(root: &Path, packaged: bool) -> Result<(), String> {
    let issues = verify_project_inner(root);
    for item in &issues {
        tracing::error!(target: "game", "资源校验失败: {item}");
    }
    if packaged && !issues.is_empty() {
        return Err(format!(
            "发布资源包校验失败（{} 项缺失）：\n  {}",
            issues.len(),
            issues.join("\n  ")
        ));
    }
    Ok(())
}

/// M20-01：校验 `scripts/**/*.lua` 全部脚本语法（与导入期共用 Importer 校验器，
/// 错误含文件路径与 Lua 行列定位）。
fn verify_script_syntax(root: &Path) -> Vec<String> {
    use rustlua_assets_import::builtin;
    let mut issues = Vec::new();
    let scripts_root = root.join("scripts");
    // 目录形态下这里是真实目录；发布包形态下 `scripts/` 只在容器里，磁盘上不存在。
    // 因此**不能**用 `scripts_root.is_dir()` 提前返回，枚举走 `vfs::list_files_recursive`
    // （容器条目 ∪ 磁盘目录）。
    let mut files = rustlua_infra::vfs::list_files_recursive(&scripts_root);
    files.sort();
    let registry = builtin();
    for file in files {
        let is_lua = file
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("lua"));
        if !is_lua {
            continue;
        }
        let Some(importer) = registry.from_path(&file) else {
            continue;
        };
        let Ok(bytes) = rustlua_infra::vfs::read(&file) else {
            issues.push(format!("无法读取脚本 {}", file.display()));
            continue;
        };
        if let Err(error) = importer.validate(&file, &bytes) {
            issues.push(error);
        }
    }
    issues
}

/// M20-01：领域数据校验。约定入口 `scripts/validate_data.lua`（若存在）返回
/// `check()` 字符串：`""` 通过，否则换行分隔的问题列表。规则全部来自工程
/// 自身，不在此硬编码任何 NPC/任务语义；未提供该模块的工程跳过此项。
fn verify_domain_data(root: &Path) -> Vec<String> {
    let entry = root.join("scripts").join("validate_data.lua");
    if !rustlua_infra::vfs::is_file(&entry) {
        return Vec::new();
    }
    let runtime = match rustlua_scripting::script::LuaRuntime::with_installers(
        rustlua_gameplay::input_lua::registry(),
    ) {
        Ok(runtime) => runtime,
        Err(error) => return vec![format!("Lua 运行时初始化失败: {error}")],
    };
    let mut runtime = runtime;
    runtime.set_project_root(root.to_path_buf());
    // 以校验入口自身作为工程脚本执行：既建立 require 搜索路径，又不启动游戏。
    if let Err(error) = runtime.execute_project_script(&entry) {
        return vec![format!(
            "领域数据校验入口加载失败 {}: {error}",
            entry.display()
        )];
    }
    match runtime.execute_console("return require('validate_data').check()") {
        Ok(report) => report
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect(),
        Err(error) => vec![format!("领域数据校验执行失败: {error}")],
    }
}

/// 读一份工程文本：走 `vfs`（provider 优先，桌面无 provider 时回落文件系统）。
///
/// 发布包形态下 `project.toml` 在骨架里、`scenes/*.scene` 在容器里，两条通道必须同源——
/// 这也是把 `fs::read_to_string` 换成它的原因。
fn read_text(path: &Path) -> Option<String> {
    let bytes = rustlua_infra::vfs::read(path).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// 递归收集目录下的全部文件（诊断扫描用；符号链接不跟随）。
///
/// 已由 [`rustlua_infra::vfs::list_files_recursive`] 取代（容器条目 ∪ 磁盘目录）。
/// 保留这个名字只为让调用点读起来仍然是"收集文件"的语义。
#[allow(dead_code)]
fn collect_files(root: &Path, out: &mut Vec<PathBuf>) {
    out.extend(rustlua_infra::vfs::list_files_recursive(root));
}

/// 从 `key = "value"` 样式的项目清单文本中提取字段值。
fn extract_key(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim().trim_start_matches('\u{feff}');
        line.strip_prefix(&format!("{key} ="))
            .map(|rest| rest.trim().trim_matches('"').to_owned())
    })
}

/// 提取场景文本中以 `assets/` 开头、到 `;`/`:`/换行为止的资源引用。
/// 共享 `rustlua_assets::dependency` 提取器，保证运行时校验与依赖图同源。
fn extract_asset_refs(text: &str) -> Vec<String> {
    use rustlua_assets::dependency::DependencyRef;
    use rustlua_assets::resource_kind::ResourceKind;
    rustlua_assets::dependency::extract_refs(text, ResourceKind::Scene)
        .into_iter()
        .filter_map(|reference| match reference {
            DependencyRef::Asset(path) => Some(path.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::parse_arguments_from;
    use super::{Quality, extract_asset_refs, extract_key, resolve_window_title, verify_project};

    /// P3-R204：`--quality` 三档解析、末次出现生效、非法值给出三选一提示。
    #[test]
    fn quality_preset_parses_three_levels_and_last_wins() {
        let parse =
            |values: &[&str]| parse_arguments_from(values.iter().map(|value| (*value).to_owned()));
        assert_eq!(parse(&[]).unwrap().quality, Quality::Default);
        assert_eq!(
            parse(&["--quality", "medium"]).unwrap().quality,
            Quality::Medium
        );
        assert_eq!(parse(&["--quality", "low"]).unwrap().quality, Quality::Low);
        assert_eq!(
            parse(&["--quality", "low", "--quality", "default"])
                .unwrap()
                .quality,
            Quality::Default,
            "末次出现的档位生效"
        );
        let error = parse(&["--quality", "ultra"]).unwrap_err();
        assert!(
            error.contains("default") && error.contains("medium") && error.contains("low"),
            "错误提示要列出三档: {error}"
        );
    }

    /// P3-R236：确定性帧档的两个开关。默认全关（= 旧行为），非法值必须报错而不是
    /// 悄悄归一——归一了就会把"对账跑歪了"伪装成"跑过了"。
    #[test]
    fn deterministic_frame_flags_default_off_and_reject_garbage() {
        let parse =
            |values: &[&str]| parse_arguments_from(values.iter().map(|value| (*value).to_owned()));
        assert!(!parse(&[]).unwrap().fixed_step, "默认不启用确定性档");
        assert_eq!(parse(&[]).unwrap().freeze_clock, None, "默认用渲染钟");
        assert!(parse(&["--fixed-step"]).unwrap().fixed_step);
        assert_eq!(
            parse(&["--freeze-clock", "12.5"]).unwrap().freeze_clock,
            Some(12.5)
        );
        assert_eq!(
            parse(&["--freeze-clock", "0"]).unwrap().freeze_clock,
            Some(0.0),
            "0 是合法秒数（不是缺省）"
        );
        assert!(parse(&["--freeze-clock"]).is_err(), "缺值必须报错");
        assert!(parse(&["--freeze-clock", "abc"]).is_err(), "非数字必须报错");
        assert!(parse(&["--freeze-clock", "-1"]).is_err(), "负数必须报错");
        assert!(parse(&["--freeze-clock", "nan"]).is_err(), "NaN 必须报错");
        assert!(parse(&["--freeze-clock", "inf"]).is_err(), "无穷必须报错");
    }

    /// P3-R206：`--draw-distance` 默认关闭（0）、米数解析、末次生效、非法值给提示。
    #[test]
    fn draw_distance_parses_meters_and_defaults_to_off() {
        let parse =
            |values: &[&str]| parse_arguments_from(values.iter().map(|value| (*value).to_owned()));
        assert_eq!(parse(&[]).unwrap().draw_distance, 0, "默认关闭");
        assert_eq!(parse(&["--draw-distance", "35"]).unwrap().draw_distance, 35);
        assert_eq!(
            parse(&["--draw-distance", "35", "--draw-distance", "0"])
                .unwrap()
                .draw_distance,
            0,
            "末次出现的值生效"
        );
        let error = parse(&["--draw-distance", "far"]).unwrap_err();
        assert!(error.contains("--draw-distance"), "{error}");
        assert!(parse(&["--draw-distance"]).is_err(), "缺值必须报错");

        // P3-R206 分类档：默认全体（None）、子串原样保留、缺值报错。
        assert_eq!(parse(&[]).unwrap().draw_distance_filter, None);
        assert_eq!(
            parse(&["--draw-distance-filter", "grass"])
                .unwrap()
                .draw_distance_filter
                .as_deref(),
            Some("grass")
        );
        assert!(
            parse(&["--draw-distance-filter"]).is_err(),
            "缺子串必须报错"
        );
    }

    /// P3-R248：`--shadow-caster-skip` 默认不排除、子串原样保留、末次生效、缺值报错。
    #[test]
    fn shadow_caster_skip_parses_substring_and_defaults_to_none() {
        let parse =
            |values: &[&str]| parse_arguments_from(values.iter().map(|value| (*value).to_owned()));
        assert_eq!(parse(&[]).unwrap().shadow_caster_skip, None, "默认不排除");
        assert_eq!(
            parse(&["--shadow-caster-skip", "ant_carpenter"])
                .unwrap()
                .shadow_caster_skip
                .as_deref(),
            Some("ant_carpenter")
        );
        assert_eq!(
            parse(&[
                "--shadow-caster-skip",
                "ant",
                "--shadow-caster-skip",
                "grass"
            ])
            .unwrap()
            .shadow_caster_skip
            .as_deref(),
            Some("grass"),
            "末次出现的值生效"
        );
        assert!(parse(&["--shadow-caster-skip"]).is_err(), "缺子串必须报错");
    }

    /// P3-R265：`--point-shadow-caster-skip` 默认不排除、子串原样保留、末次生效、缺值报错。
    #[test]
    fn point_shadow_caster_skip_parses_substring_and_defaults_to_none() {
        let parse =
            |values: &[&str]| parse_arguments_from(values.iter().map(|value| (*value).to_owned()));
        assert_eq!(
            parse(&[]).unwrap().point_shadow_caster_skip,
            None,
            "默认不排除"
        );
        assert_eq!(
            parse(&["--point-shadow-caster-skip", "ant_carpenter"])
                .unwrap()
                .point_shadow_caster_skip
                .as_deref(),
            Some("ant_carpenter")
        );
        assert_eq!(
            parse(&[
                "--point-shadow-caster-skip",
                "ant",
                "--point-shadow-caster-skip",
                "grass"
            ])
            .unwrap()
            .point_shadow_caster_skip
            .as_deref(),
            Some("grass"),
            "末次出现的值生效"
        );
        assert!(
            parse(&["--point-shadow-caster-skip"]).is_err(),
            "缺子串必须报错"
        );
    }

    /// P3-R256：`--shadow-caster-distance` 默认 0（关闭）、整数米、末次生效、缺值/非法报错。
    #[test]
    fn shadow_caster_distance_parses_meters_and_defaults_to_off() {
        let parse =
            |values: &[&str]| parse_arguments_from(values.iter().map(|value| (*value).to_owned()));
        assert_eq!(parse(&[]).unwrap().shadow_caster_distance, 0, "默认关闭");
        assert_eq!(
            parse(&["--shadow-caster-distance", "150"])
                .unwrap()
                .shadow_caster_distance,
            150
        );
        assert_eq!(
            parse(&[
                "--shadow-caster-distance",
                "150",
                "--shadow-caster-distance",
                "80",
            ])
            .unwrap()
            .shadow_caster_distance,
            80,
            "末次出现的值生效"
        );
        assert!(
            parse(&["--shadow-caster-distance"]).is_err(),
            "缺米数必须报错"
        );
        assert!(
            parse(&["--shadow-caster-distance", "-5"]).is_err(),
            "负数必须报错"
        );
    }

    /// P3-R207：`--water-refraction` 默认关闭（0）、增益解析、非法/负值归一为关闭、缺值报错。
    #[test]
    fn water_refraction_parses_gain_and_rejects_garbage() {
        let parse =
            |values: &[&str]| parse_arguments_from(values.iter().map(|value| (*value).to_owned()));
        assert_eq!(
            parse(&[]).unwrap().water_refraction,
            0.0,
            "默认关闭 = 旧的固定偏移"
        );
        assert_eq!(
            parse(&["--water-refraction", "0.0008"])
                .unwrap()
                .water_refraction,
            0.0008,
        );
        assert_eq!(
            parse(&["--water-refraction", "0.0008", "--water-refraction", "0"])
                .unwrap()
                .water_refraction,
            0.0,
            "末次出现的值生效"
        );
        assert_eq!(
            parse(&["--water-refraction", "-1"])
                .unwrap()
                .water_refraction,
            0.0,
            "负增益归一为关闭"
        );
        let error = parse(&["--water-refraction", "deep"]).unwrap_err();
        assert!(error.contains("--water-refraction"), "{error}");
        assert!(parse(&["--water-refraction"]).is_err(), "缺值必须报错");
    }

    /// P3-R241：`--water-refract-steps` 默认关闭（0）、截到 32、非整数报错、缺值报错。
    #[test]
    fn water_refract_steps_parse_and_clamp() {
        let parse =
            |values: &[&str]| parse_arguments_from(values.iter().map(|value| (*value).to_owned()));
        assert_eq!(
            parse(&[]).unwrap().water_refract_steps,
            0,
            "默认关闭 = R207 的偏移近似"
        );
        assert_eq!(
            parse(&["--water-refract-steps", "16"])
                .unwrap()
                .water_refract_steps,
            16
        );
        assert_eq!(
            parse(&["--water-refract-steps", "64"])
                .unwrap()
                .water_refract_steps,
            32,
            "超过 32 截到 32"
        );
        assert_eq!(
            parse(&["--water-refract-steps", "8", "--water-refract-steps", "0"])
                .unwrap()
                .water_refract_steps,
            0,
            "末次出现的值生效"
        );
        let error = parse(&["--water-refract-steps", "-1"]).unwrap_err();
        assert!(error.contains("--water-refract-steps"), "{error}");
        let error = parse(&["--water-refract-steps", "deep"]).unwrap_err();
        assert!(error.contains("--water-refract-steps"), "{error}");
        assert!(parse(&["--water-refract-steps"]).is_err(), "缺值必须报错");
    }

    /// P3-R243：`--prewarm-particles` 默认关、出现即开。
    #[test]
    fn prewarm_particles_flag_parses() {
        let parse =
            |values: &[&str]| parse_arguments_from(values.iter().map(|value| (*value).to_owned()));
        assert!(!parse(&[]).unwrap().prewarm_particles, "默认不预热");
        assert!(parse(&["--prewarm-particles"]).unwrap().prewarm_particles);
    }

    /// P3-R270：`--no-bake-cache` 默认关（缓存开）、出现即关。
    #[test]
    fn no_bake_cache_flag_parses() {
        let parse =
            |values: &[&str]| parse_arguments_from(values.iter().map(|value| (*value).to_owned()));
        assert!(!parse(&[]).unwrap().no_bake_cache, "默认开烘焙缓存");
        assert!(parse(&["--no-bake-cache"]).unwrap().no_bake_cache);
    }

    /// P3-R246：`--gpu-motion` 默认关、出现即开。
    #[test]
    fn gpu_motion_flag_parses() {
        let parse =
            |values: &[&str]| parse_arguments_from(values.iter().map(|value| (*value).to_owned()));
        assert!(!parse(&[]).unwrap().gpu_motion, "默认走 CPU 求值");
        assert!(parse(&["--gpu-motion"]).unwrap().gpu_motion);
    }

    /// P3-R255：`--gpu-motion-frustum-cull` 默认关、出现即开、不吞后一个参数。
    #[test]
    fn gpu_motion_frustum_cull_parses_flag() {
        let parse =
            |values: &[&str]| parse_arguments_from(values.iter().map(|value| (*value).to_owned()));
        assert!(
            !parse(&[]).unwrap().gpu_motion_frustum_cull,
            "默认关闭（逐矩阵不变）"
        );
        assert!(
            parse(&["--gpu-motion-frustum-cull"])
                .unwrap()
                .gpu_motion_frustum_cull
        );
        assert!(
            parse(&["--gpu-motion-frustum-cull", "--gpu-motion"])
                .unwrap()
                .gpu_motion,
            "布尔旗标不许吞掉后面的旗标"
        );
    }

    /// P3-R266：`--gpu-motion-thin <米>:<k>` 默认关、解析出 (米, k)、k < 2 归一为关、
    /// 缺冒号/缺值/非数字报错。
    #[test]
    fn gpu_motion_thin_parses_meters_and_stride() {
        let parse =
            |values: &[&str]| parse_arguments_from(values.iter().map(|value| (*value).to_owned()));
        assert_eq!(parse(&[]).unwrap().gpu_motion_thin, None, "默认关闭");
        assert_eq!(
            parse(&["--gpu-motion-thin", "60:4"])
                .unwrap()
                .gpu_motion_thin,
            Some((60.0, 4))
        );
        assert_eq!(
            parse(&["--gpu-motion-thin", "30:1"])
                .unwrap()
                .gpu_motion_thin,
            None,
            "步长 1 = 关闭"
        );
        assert_eq!(
            parse(&["--gpu-motion-thin", "0:4"])
                .unwrap()
                .gpu_motion_thin,
            None,
            "半径 0 = 关闭"
        );
        assert!(
            parse(&["--gpu-motion-thin", "60"]).is_err(),
            "缺冒号必须报错"
        );
        assert!(parse(&["--gpu-motion-thin"]).is_err(), "缺值必须报错");
        assert!(
            parse(&["--gpu-motion-thin", "abc:4"]).is_err(),
            "米数非数字必须报错"
        );
        assert!(
            parse(&["--gpu-motion-thin", "60:x"]).is_err(),
            "步长非整数必须报错"
        );
        // P3-R267：第二远带必须严格更远，否则归一为关（第一带照旧）。
        let two = parse(&["--gpu-motion-thin", "30:4,90:16"]).unwrap();
        assert_eq!(two.gpu_motion_thin, Some((30.0, 4)));
        assert_eq!(two.gpu_motion_thin_far, Some((90.0, 16)));
        let single = parse(&["--gpu-motion-thin", "30:4"]).unwrap();
        assert_eq!(single.gpu_motion_thin, Some((30.0, 4)));
        assert_eq!(single.gpu_motion_thin_far, None, "单带 = 第二带关");
        let reversed = parse(&["--gpu-motion-thin", "90:4,30:16"]).unwrap();
        assert_eq!(reversed.gpu_motion_thin, Some((90.0, 4)));
        assert_eq!(reversed.gpu_motion_thin_far, None, "第二带更近 = 归一为关");
        assert!(
            parse(&["--gpu-motion-thin", "10:2,20:4,30:8"]).is_err(),
            "超过两带必须报错"
        );
        assert!(
            parse(&["--gpu-motion-thin", "30:4,90"]).is_err(),
            "第二带缺冒号必须报错"
        );
    }

    /// P3-R250：`--gpu-motion-cull` 默认 0（关）、非正值归一为 0、缺值/非数字报错。
    #[test]
    fn gpu_motion_cull_parses_meters_and_rejects_garbage() {
        let parse =
            |values: &[&str]| parse_arguments_from(values.iter().map(|value| (*value).to_owned()));
        assert_eq!(parse(&[]).unwrap().gpu_motion_cull, 0.0, "默认关闭");
        assert_eq!(
            parse(&["--gpu-motion-cull", "12.5"])
                .unwrap()
                .gpu_motion_cull,
            12.5
        );
        assert_eq!(
            parse(&["--gpu-motion-cull", "0"]).unwrap().gpu_motion_cull,
            0.0,
            "0 = 关闭"
        );
        assert_eq!(
            parse(&["--gpu-motion-cull", "-3"]).unwrap().gpu_motion_cull,
            0.0,
            "负值归一为关闭"
        );
        assert!(parse(&["--gpu-motion-cull"]).is_err(), "缺值必须报错");
        assert!(
            parse(&["--gpu-motion-cull", "abc"]).is_err(),
            "非数字必须报错"
        );
    }

    /// P3-R208：`--underwater-view` 默认关闭（0）、强度截到 1、非法/负值归一为关闭、缺值报错。
    #[test]
    fn underwater_view_parses_strength_and_rejects_garbage() {
        let parse =
            |values: &[&str]| parse_arguments_from(values.iter().map(|value| (*value).to_owned()));
        assert_eq!(
            parse(&[]).unwrap().underwater_view,
            0.0,
            "默认关闭 = 只有 P3-R173 的整帧吸收"
        );
        assert_eq!(
            parse(&["--underwater-view", "0.6"])
                .unwrap()
                .underwater_view,
            0.6
        );
        assert_eq!(
            parse(&["--underwater-view", "4"]).unwrap().underwater_view,
            1.0,
            "超过 1 的强度截到全替换档"
        );
        assert_eq!(
            parse(&["--underwater-view", "0.6", "--underwater-view", "0"])
                .unwrap()
                .underwater_view,
            0.0,
            "末次出现的值生效"
        );
        assert_eq!(
            parse(&["--underwater-view", "-1"]).unwrap().underwater_view,
            0.0,
            "负强度归一为关闭"
        );
        let error = parse(&["--underwater-view", "clear"]).unwrap_err();
        assert!(error.contains("--underwater-view"), "{error}");
        assert!(parse(&["--underwater-view"]).is_err(), "缺值必须报错");
    }

    /// P3-R209：`--clip-cap` 默认关闭（0）、是米制深度**不截断**、非法/负值归一为关闭、缺值报错。
    #[test]
    fn clip_cap_parses_depth_and_rejects_garbage() {
        let parse =
            |values: &[&str]| parse_arguments_from(values.iter().map(|value| (*value).to_owned()));
        assert_eq!(
            parse(&[]).unwrap().clip_cap,
            0.0,
            "默认关闭 = P3-R54 的 0.08 薄带，像素逐字节不变"
        );
        assert_eq!(parse(&["--clip-cap", "0.5"]).unwrap().clip_cap, 0.5);
        assert_eq!(
            parse(&["--clip-cap", "40"]).unwrap().clip_cap,
            40.0,
            "封口深度是米制长度，不是混合权重，不许截到 1"
        );
        assert_eq!(
            parse(&["--clip-cap", "0.5", "--clip-cap", "0"])
                .unwrap()
                .clip_cap,
            0.0,
            "末次出现的值生效"
        );
        assert_eq!(
            parse(&["--clip-cap", "-1"]).unwrap().clip_cap,
            0.0,
            "负深度归一为关闭"
        );
        let error = parse(&["--clip-cap", "deep"]).unwrap_err();
        assert!(error.contains("--clip-cap"), "{error}");
        assert!(parse(&["--clip-cap"]).is_err(), "缺值必须报错");
    }

    /// P3-R211：`--clip-cap-both-sides` 是无值开关，默认关、给了就开，且不吃后面那个 token。
    #[test]
    fn clip_cap_both_sides_is_a_bare_flag() {
        let parse =
            |values: &[&str]| parse_arguments_from(values.iter().map(|value| (*value).to_owned()));
        assert!(
            !parse(&[]).unwrap().clip_cap_both_sides,
            "默认关闭 = 单面网格的背面仍被剔除"
        );
        assert!(
            parse(&["--clip-cap-both-sides"])
                .unwrap()
                .clip_cap_both_sides,
            "开关一给就开"
        );
        let with_depth = parse(&["--clip-cap", "2", "--clip-cap-both-sides"]).unwrap();
        assert!(with_depth.clip_cap_both_sides && with_depth.clip_cap > 0.0);
        assert!(
            parse(&["--clip-cap-both-sides", "--clip-cap", "2"])
                .unwrap()
                .clip_cap
                > 0.0,
            "开关不许吞掉后面那条旗标的值"
        );
    }

    #[test]
    fn splash_check_missing_corrupt_plain_and_packed_images() {
        let root =
            std::env::temp_dir().join(format!("rustlua-splash-check-{}", std::process::id()));
        std::fs::create_dir_all(root.join("assets")).unwrap();
        let config = root.join("project.toml");
        std::fs::write(&config, "name = \"SplashTest\"\n").unwrap();
        assert!(
            verify_project(&root).is_empty(),
            "no configuration preserves old check"
        );
        std::fs::write(&config, "splash_image = \"assets/start.png\"\n").unwrap();
        let issues = verify_project(&root);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].contains("splash_image 读取失败"));
        assert!(
            super::verify_startup(&root, true).is_ok(),
            "optional image must not prevent startup"
        );
        std::fs::write(root.join("assets/start.png"), b"broken").unwrap();
        assert!(verify_project(&root)[0].contains("splash_image 解码失败"));
        for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg] {
            let image = image::RgbImage::from_pixel(4, 2, image::Rgb([20, 40, 80]));
            let mut bytes = std::io::Cursor::new(Vec::new());
            image.write_to(&mut bytes, format).unwrap();
            for packed in [false, true] {
                let data = rustlua_infra::pack::encode(bytes.get_ref(), packed, packed);
                std::fs::write(root.join("assets/start.png"), data).unwrap();
                assert!(verify_project(&root).is_empty());
                assert_eq!(
                    crate::splash::load(&root).unwrap().unwrap().dimensions(),
                    (4, 2)
                );
            }
        }
        std::fs::write(&config, "splash_image = \"../escape.png\"\n").unwrap();
        assert!(verify_project(&root)[0].contains("相对路径"));
        std::fs::write(&config, "main_script = \"missing.lua\"\n").unwrap();
        assert!(super::verify_startup(&root, false).is_ok());
        assert!(super::verify_startup(&root, true).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn icon_check_and_startup_fallback_plain_and_packed_images() {
        let root = std::env::temp_dir().join(format!("rustlua-icon-check-{}", std::process::id()));
        std::fs::create_dir_all(root.join("assets")).unwrap();
        let config = root.join("project.toml");
        std::fs::write(&config, "name = \"IconTest\"\n").unwrap();
        let default_splash = crate::splash::startup_image(None, (1280, 720)).unwrap();
        let default_icon = crate::splash::icon_image(None);
        assert_eq!(
            crate::splash::startup_image(Some(&root), (1280, 720)).unwrap(),
            default_splash
        );
        assert_eq!(crate::splash::icon_image(Some(&root)), default_icon);
        std::fs::write(
            &config,
            "icon_image = \"assets/icon.png\"\nsplash_image = \"assets/start.png\"\n",
        )
        .unwrap();
        let issues = verify_project(&root);
        assert_eq!(issues.len(), 2);
        assert!(
            issues
                .iter()
                .any(|issue| issue.contains("icon_image 读取失败"))
        );
        assert!(super::verify_startup(&root, true).is_ok());
        std::fs::write(root.join("assets/icon.png"), b"broken").unwrap();
        std::fs::write(root.join("assets/start.png"), b"broken").unwrap();
        assert!(
            verify_project(&root)
                .iter()
                .any(|issue| issue.contains("icon_image 解码失败"))
        );
        assert_eq!(
            crate::splash::startup_image(Some(&root), (1280, 720)).unwrap(),
            default_splash
        );
        assert_eq!(crate::splash::icon_image(Some(&root)), default_icon);
        for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg] {
            let image = image::RgbImage::from_pixel(4, 2, image::Rgb([20, 40, 80]));
            let mut bytes = std::io::Cursor::new(Vec::new());
            image.write_to(&mut bytes, format).unwrap();
            let expected = image::load_from_memory(bytes.get_ref()).unwrap().to_rgba8();
            for packed in [false, true] {
                let data = rustlua_infra::pack::encode(bytes.get_ref(), packed, packed);
                std::fs::write(root.join("assets/icon.png"), &data).unwrap();
                std::fs::write(root.join("assets/start.png"), &data).unwrap();
                assert!(verify_project(&root).is_empty());
                assert_eq!(crate::splash::icon_image(Some(&root)), expected);
                assert_eq!(
                    crate::splash::startup_image(Some(&root), (1280, 720)).unwrap(),
                    expected
                );
            }
        }
        for path in ["../escape.png", "/absolute.png", "C:\\absolute.png"] {
            std::fs::write(&config, format!("icon_image = \"{path}\"\n")).unwrap();
            assert!(verify_project(&root)[0].contains("icon_image 路径必须"));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extract_key_parses_quoted_project_fields() {
        let text = "[project]\nname = \"game\"\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n";
        assert_eq!(
            extract_key(text, "main_script"),
            Some("scripts/main.lua".to_owned())
        );
        assert_eq!(
            extract_key(text, "startup_scene"),
            Some("scenes/main.scene".to_owned())
        );
        assert_eq!(extract_key(text, "missing"), None);
        assert_eq!(
            extract_key("\u{feff}publish_name = \"Demo\"", "publish_name"),
            Some("Demo".to_owned())
        );
    }

    #[test]
    fn resolve_window_title_prefers_explicit_title_then_project_name() {
        assert_eq!(
            resolve_window_title(None, Some("Published".to_owned())),
            "Published"
        );
        let root = std::env::temp_dir().join(format!("rustlua-title-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "[project]\nname = \"DemoGame\"\n",
        )
        .unwrap();
        assert_eq!(resolve_window_title(Some(&root), None), "DemoGame");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn extract_asset_refs_scans_component_strings() {
        let scene = "component 1 Sprite texture=assets/textures/player.png;size=32\n\
                     component 1 Animation default=idle;clip:idle:assets/textures/hero-idle.png:4:4:true\n\
                     component 2 Tilemap texture=assets/textures/tileset.png;width=2\n";
        let refs = extract_asset_refs(scene);
        assert!(refs.contains(&"assets/textures/player.png".to_owned()));
        assert!(refs.contains(&"assets/textures/hero-idle.png".to_owned()));
        assert!(refs.contains(&"assets/textures/tileset.png".to_owned()));
        assert!(!refs.contains(&"assets/missing.png".to_owned()));
    }

    #[test]
    fn verify_project_reports_missing_resources_once() {
        let root = std::env::temp_dir().join(format!("rustlua-verify-{}", std::process::id()));
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        std::fs::create_dir_all(root.join("assets/textures")).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "[project]\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("scenes/main.scene"),
            "component 1 Sprite texture=assets/textures/player.png\ncomponent 2 Sprite texture=assets/textures/player.png\n",
        )
        .unwrap();
        // 主脚本缺失 + player.png 缺失（引用两次，应去重为一条）
        let missing = verify_project(&root);
        assert_eq!(
            missing.len(),
            2,
            "missing script + missing asset: {missing:?}"
        );
        assert!(missing.iter().any(|m| m.contains("main.lua")));
        assert_eq!(
            missing.iter().filter(|m| m.contains("player.png")).count(),
            1,
            "duplicate asset reference deduplicated"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn verify_project_passes_for_complete_project() {
        let root = std::env::temp_dir().join(format!("rustlua-verify-ok-{}", std::process::id()));
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "[project]\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("scripts/main.lua"),
            "function update() return 'ok' end\n",
        )
        .unwrap();
        std::fs::write(
            root.join("scenes/main.scene"),
            "scene_version=2\nname=main\n",
        )
        .unwrap();
        assert!(verify_project(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// M20-01：发布前检查同时覆盖脚本语法与工程自定义领域数据校验，
    /// 诊断含文件路径（以及 Lua/数据的行或字段定位）。
    #[test]
    fn verify_project_reports_script_syntax_and_domain_data_issues() {
        let root = std::env::temp_dir().join(format!("rustlua-verify-diag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("scripts/data")).unwrap();
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "[project]\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("scripts/main.lua"),
            "function update() return 'ok' end\n",
        )
        .unwrap();
        std::fs::write(
            root.join("scenes/main.scene"),
            "scene_version=2\nname=main\n",
        )
        .unwrap();
        // 故意写坏一个脚本：缺少 end（第 3 行 EOF 报错）。
        std::fs::write(
            root.join("scripts/broken.lua"),
            "function update()\n  return 1\n",
        )
        .unwrap();
        // 工程自定义领域校验入口：报告一条含文件/对象/字段路径的问题。
        std::fs::write(
            root.join("scripts/validate_data.lua"),
            "local M = {}\n\
             function M.check()\n\
               return \"[data.demo] data/demo.lua:items[1].name: required non-empty string\"\n\
             end\n\
             return M\n",
        )
        .unwrap();

        let issues = verify_project(&root);
        let syntax = issues
            .iter()
            .find(|issue| issue.contains("broken.lua"))
            .unwrap_or_else(|| panic!("缺少脚本语法诊断: {issues:?}"));
        assert!(
            syntax.contains("语法错误") && syntax.contains(":3:"),
            "语法诊断需含类型与行号: {syntax}"
        );
        let domain = issues
            .iter()
            .find(|issue| issue.contains("data/demo.lua:items[1].name"))
            .unwrap_or_else(|| panic!("缺少领域数据诊断: {issues:?}"));
        assert!(
            domain.contains("[data.demo]"),
            "领域诊断需含模块归属: {domain}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// M20-01：合法工程（含校验入口返回空）不应产生任何诊断。
    #[test]
    fn verify_project_accepts_valid_scripts_and_domain_data() {
        let root =
            std::env::temp_dir().join(format!("rustlua-verify-clean-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("scripts/data")).unwrap();
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "[project]\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("scripts/main.lua"),
            "function update() return 'ok' end\n",
        )
        .unwrap();
        std::fs::write(
            root.join("scripts/data/demo.lua"),
            "return { version = 1 }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("scripts/validate_data.lua"),
            "local M = {}\nfunction M.check() return \"\" end\nreturn M\n",
        )
        .unwrap();
        std::fs::write(
            root.join("scenes/main.scene"),
            "scene_version=2\nname=main\n",
        )
        .unwrap();
        assert!(
            verify_project(&root).is_empty(),
            "clean project must pass: {:?}",
            verify_project(&root)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn extract_asset_refs_scans_3d_components() {
        let scene = "component 1 MeshRenderer mesh=assets/models/hero.glb\n\
                     component 1 Camera3D position=0,0,5;target=0,0,0;fov=90;near=0.1;far=100\n\
                     component 2 MeshRenderer mesh=__ground_plane__\n";
        let refs = extract_asset_refs(scene);
        assert!(refs.contains(&"assets/models/hero.glb".to_owned()));
        assert!(!refs.contains(&"__ground_plane__".to_owned()));
        assert!(!refs.contains(&"assets/missing.glb".to_owned()));
    }

    #[test]
    fn verify_project_reports_missing_3d_mesh() {
        let root = std::env::temp_dir().join(format!("rustlua-verify-3d-{}", std::process::id()));
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::create_dir_all(root.join("assets/models")).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "[project]\nmain_script = \"scripts/main.lua\"\nstartup_scene = \"scenes/main.scene\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("scripts/main.lua"),
            "function update() return 'ok' end\n",
        )
        .unwrap();
        // v2 scene referencing an existing and a missing 3D mesh.
        std::fs::write(root.join("assets/models/cube.glb"), b"fake glb bytes").unwrap();
        std::fs::write(
            root.join("scenes/main.scene"),
            "scene_version=2\nname=3d\n\
             component 1 MeshRenderer mesh=assets/models/cube.glb\n\
             component 2 MeshRenderer mesh=assets/models/missing.glb\n",
        )
        .unwrap();
        let missing = verify_project(&root);
        assert_eq!(
            missing.iter().filter(|m| m.contains("missing.glb")).count(),
            1,
            "missing 3D mesh reported: {missing:?}"
        );
        assert!(
            missing.iter().all(|m| !m.contains("cube.glb")),
            "existing 3D mesh must not be reported: {missing:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
