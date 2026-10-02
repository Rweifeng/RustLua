//! Runtime scene: instantiates the project scene/scripts and produces
//! per-frame [`SceneMesh`] (2D) and [`Scene3d`] draw data for the renderer.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::core::{
    animation::{AnimationFrameEvent, AnimationStateMachine},
    animation3d::Animation3D,
    asset_handle::{AssetHandle, AssetRecord, AssetStore},
    camera::Camera3D,
    camera::{
        Camera2D, CameraBounds, CameraSelection, CameraView, DEFAULT_3D_CLEAR, OrthoTerms,
        RenderView, Viewport,
    },
    entity::EntityId,
    font::{FontAtlas, FontVectorAtlas, project_font},
    hierarchy::{GlobalTransform, SpatialReport},
    input::InputSnapshot,
    layer::RenderLayer,
    light::Light,
    math3d::{Mat4, Vec3},
    mesh3d::MeshRenderer,
    par::Parallelism,
    particle::{ParticleEmitter, SceneParticleEmitter},
    physics::PhysicsAuthority,
    quad::Quad,
    resource_kind::ResourceKind,
    scene::{Scene, SceneEntityId},
    scheduler::{
        ScheduleReport, ScheduleStage, Scheduler, SchedulerError, System, SystemError, SystemTiming,
    },
    script::{LuaRuntime, ReloadStatus},
    sprite::{Sprite, SpriteAsset, SpriteSorting},
    sprite_batch::{
        OverlayVertexSink, SpriteBatch, SpriteBatchItem, SpriteDrawBatch, WorldSpriteVertexSink,
        ndc_from_screen,
    },
    text::TextMesh,
    tilemap::Tilemap,
    transform::Transform3D,
    transform::{Transform, Vec2, integrate_velocity},
    ui::{
        DynamicUiImage, UI_REFERENCE_SIZE, UiImage, UiText, nine_slice_cells, resolve_ui_rect,
        ui_text_effective_scale,
    },
    ui_events::{UiClickTarget, UiEvents},
    ui_layout::{UiRect, UiRectLayout},
    ui_scene_layout::{ResolvedUiNode, UiSceneNode, resolve_scene_ui_layout},
};

use crate::host::frame::{
    ATLAS_TEXTURE_PATH, BoundsSource, CustomFontAtlasPixels, FONT_ATLAS_TEXTURE_PATH, FontAtlasMap,
    FontAtlasPixels, FontHandle, OverlayVertex, Scene3d, Scene3dBounds, Scene3dDrawable, SceneDraw,
    SceneMesh, VectorAtlasMap, VectorFontHandle, VectorGlyphSnapshot, VectorSegmentSnapshot,
    VectorTextVertex as FrameVectorTextVertex, WHITE_TEXTURE_PATH, WorldSpriteVertex,
    font_texture_path,
};
use crate::host::gamepad::{GamepadBackend, GamepadDeadzone, GamepadPoller};

mod text;

/// 本帧的 3D 视图（相机拆分批 8）：`vertices()` **定帧**时解析一次、烘焙一次，同一帧的消费方
/// （回填给 Lua 的 `camera3d.*`、3D 渲染提取、3D 调试线框）共用它。
///
/// ## 为什么必须定帧
///
/// 此前这三个消费方各自调一次 `active_camera3d()`，两份结果一致靠的是"同帧两步之间没有代码
/// 改过世界"这个**隐含约定**——与批 7 治好的"同一事实两处算"是同一类病：宿主只要在两步之间
/// 插入一次步进/拖拽预览，画面与拾取就会按两台不同的相机各算一套，且没有任何报错。
///
/// ## 语义
///
/// - **谁定帧**：`vertices()`（宿主的每帧入口：它已经在做"传播世界变换 + 清每帧状态 + 回填 Lua
///   相机"这三件事，是天然的帧边界）。
/// - **失效**：任何可能改世界或改相机选择的入口（`run_script_steps`、编辑器拖拽预览）之后，
///   定帧值作废，下一次消费重新定帧——绝不拿步进前的相机去渲染步进后的世界。
/// - **视口参与缓存键**：宿主若用另一个视口调 `scene_3d`，比例仍按**传入视口**算（不复用），
///   因此"渲染视口"与"矩阵比例"仍然只有一个来源。
/// - 宿主没走 `vertices()` 就直接调 `scene_3d`（示例/golden 里就有这种顺序）时缓存为空，
///   按需解析——行为与定帧前逐字节相同。
#[derive(Clone, Copy, Debug)]
pub(crate) struct FrameView3d {
    camera: Camera3D,
    viewport: Viewport,
    view: RenderView,
}

pub struct RuntimeScene {
    /// Lua 桥。**crate 内可见**：工程级门（`host::sample_project`）要用它驱动
    /// `on_click` / `scene.load` / 控制台表达式，验证示例工程的菜单与演示真的能跑；
    /// 对外仍然只暴露 `RuntimeScene` 自己的方法。
    pub(crate) script: LuaRuntime,
    /// 自上次 `take_script_micros` 以来所有固定步里 Lua `update()` 的累计耗时
    /// （微秒）。宿主把它送进 `PerformanceStats::lua_ms`，性能面板因此显示的是
    /// "Lua 花了多少"而不是恒为 0（P4 接线）。
    pub(crate) script_micros: u64,
    pub(crate) sprite_batch: SpriteBatch,
    /// 每精灵顶点生成的并行度（WP8）。默认取检测到的核数；`Parallelism::SEQUENTIAL`
    /// 即完全回滚到单线程，输出逐字节相同（见 `core/par.rs` 与 `sprite_batch` 的测试）。
    pub(crate) sprite_parallelism: Parallelism,
    /// P1-③：系统级数据并行开关（粒子积分等系统内纯数据子任务）。默认取检测到的
    /// 核数；`Parallelism::SEQUENTIAL` 完全回滚到单线程——输出逐位相同（回滚档）。
    pub(crate) system_parallelism: Parallelism,
    /// P1-①：渲染级空间索引（视口候选预筛）开关。默认关闭；打开后精灵收集阶段
    /// 用 uniform grid 预筛视口候选，静态场景收集从 O(N) 降为 O(视口内)
    /// （docs/40 WP6 ⑧ / docs/53 §3.9）。关闭即逐字节回滚。
    pub(crate) spatial_index: bool,
    /// 空间索引网格（惰性创建）。跨帧复用：`hierarchy::local_fingerprint` 不变则不重建
    /// ——静态场景收集从 O(N) 降为 O(视口内)；任何实体一动（指纹变）就整体重建。
    pub(crate) spatial_grid: Option<crate::core::spatial_index::SpatialGrid2D>,
    /// 上次重建网格时的指纹（与 `hierarchy::local_fingerprint` 返回同型；
    /// 与层级传播的 `spatial_fingerprint` 是两套独立缓存）。
    pub(crate) spatial_index_fingerprint: Option<(u64, usize, usize)>,
    /// 网格 cell 边长（世界单位）。只影响性能不影响正确性；默认 256。
    pub(crate) spatial_index_cell_size: f32,
    /// collider 调试绘制开关（WP7 ②）。默认关闭。
    pub(crate) collider_debug_draw: bool,
    /// docs/54 §8-⑥：**3D** collider 调试绘制开关（`physics3d.debug_draw`）。默认关闭，
    /// 与 2D 开关独立——混合场景里只想看一侧是常态。
    pub(crate) collider_debug_draw_3d: bool,
    /// 视口外瓦片剔除（WP4 ④）。默认开启；关掉即回到整层全画，用于像素对拍与回滚。
    pub(crate) tilemap_culling: bool,
    /// 视口外精灵剔除（WP6 CPU 可见性）。默认开启，同样可关掉做像素对拍。
    pub(crate) sprite_culling: bool,
    pub(crate) camera: Camera2D,
    /// 本帧的 3D 视图（拆分批 8，见 [`FrameView3d`]）：`vertices()` 定帧，同帧消费方共用。
    /// `None` = 本帧还没定帧，或刚被一次世界变更作废。
    pub(crate) frame_view_3d: Option<FrameView3d>,
    /// 拆分批 9：**已经采用过载荷**的 2D 相机来源（`camera.set_active` 选中的那台正交相机）。
    /// 它让"选择变化"成为一次性事件：同一台不再重复采用（否则每帧把控制器的取景抹平），
    /// 而选择离开 2D 相机后再选回来会重新采用（"把 2D 取景复位回场景声明"由此表达）。
    pub(crate) adopted_camera_2d_source: Option<EntityId>,
    pub(crate) viewport_width: u32,
    pub(crate) viewport_height: u32,
    /// Canvas 空间 UI 参考分辨率（设计分辨率）。发布游戏默认 `UI_REFERENCE_SIZE`；
    /// 编辑器按设计分辨率显式设置，使 UI 适配 = viewport / reference。
    pub(crate) ui_reference: [f32; 2],
    /// 是否渲染屏幕空间 UI 覆盖层。编辑器场景视图置 false：UI 由 egui 在
    /// 设计分辨率包围框内绘制布局框，避免旧的面板适配覆盖层与之冲突。
    pub(crate) render_ui_overlay: bool,
    /// 项目根目录（纹理路径解析用，与渲染器 `project_root.join(path)` 一致）。
    pub(crate) project_root: Option<PathBuf>,
    /// 九宫格 UV 计算用的纹理像素尺寸缓存（惰性、按 path）。
    pub(crate) texture_dims: HashMap<String, (u32, u32)>,
    pub(crate) texture_paths: Vec<String>,
    /// 本帧的路径去重表（每帧清空）：把"同一路径只注册一次"保持成 O(1)，与身份表分开。
    pub(crate) texture_lookup: HashMap<String, u32>,
    /// docs/54 §4.4：**跨帧稳定**的运行时资源身份表（`路径 → 句柄`）。
    ///
    /// 与每帧重建的 `texture_paths` 不同，它**不清空**：同一路径因此在进程内恒同句柄，
    /// "这一帧的 draw 引用的是哪个资源"跨帧可比；资源释放后旧句柄失效，而不是悄悄指向别人
    /// 放在同一个下标上的贴图。
    pub(crate) assets: std::cell::RefCell<AssetStore<AssetRecord>>,
    /// 本帧与 `texture_paths` **同序**的句柄（交给渲染器做 O(1) 的贴图缓存查找）。
    pub(crate) texture_handles: Vec<AssetHandle>,
    /// docs/54 §4.3：声明了 `mode=overlay` 的实体（屏幕空间，落 `Overlay2D` 桶）。
    pub(crate) overlay_batch: SpriteBatch,
    /// docs/54 §4.3：声明了 `mode=screen` 的实体（屏幕空间，落 `ScreenUI` 桶）。
    pub(crate) screen_batch: SpriteBatch,
    /// 本帧三种显式模式各路由了几个实体 `(world, overlay, screen)`（诊断/验收用）。
    pub(crate) render_mode_routed: (usize, usize, usize),
    pub(crate) tilemap_cache: HashMap<EntityId, CachedTilemapLayer>,
    /// 共享字体图集：运行时重建（编辑器拖动/编辑场景导致 revision 变化时）复用
    /// 同一实例，避免每帧重新生成 MSDF 字形。生成字形需可变访问（内部 Mutex）。
    pub(crate) font_atlas: Option<Arc<Mutex<FontAtlas>>>,
    pub(crate) vector_atlas: Option<Arc<Mutex<FontVectorAtlas>>>,
    /// 场景 Text 自定义字体矢量轮廓缓存（key = `font://<asset>`）。编辑器注入
    /// 同一实例供场景视图/预览跨运行时重建复用；独立运行时用新鲜空表。
    pub(crate) custom_vector_atlases: VectorAtlasMap,
    pub(crate) last_font_version: u64,
    /// 场景 Text 自定义字体缓存（key = `font://<asset>` → 图集句柄）。编辑器注入
    /// 同一实例供场景视图/预览跨运行时重建复用；独立运行时用各自新鲜的空表。
    pub(crate) custom_font_atlases: FontAtlasMap,
    /// 每个自定义字体“上次上报版本”，用于判定本帧像素是否变化（见默认图集的
    /// `last_font_version` 语义，保证新渲染器首帧一定拿到非空像素）。
    pub(crate) custom_font_versions: HashMap<String, u64>,
    /// 本帧布局用到、待上报渲染器的自定义字体 key（`font://...`），`vertices`
    /// 末尾生成像素快照后清空。
    pub(crate) frame_font_keys: Vec<String>,
    /// 已 warn 过的加载失败字体路径（避免文本每帧 layout 时重复刷屏）。
    pub(crate) missing_fonts: HashSet<String>,
    pub(crate) missing_vector_fonts: HashSet<String>,
    pub(crate) text_render_mode: TextRenderMode,
    pub(crate) ui_scene_nodes: BTreeMap<u64, UiSceneNode>,
    pub(crate) ui_layout_rects: BTreeMap<u64, ResolvedUiNode>,
    /// 共享帧调度管线（M18-03）：每个固定步执行一次的系统。调度器实例**跨帧保留**
    /// （只在 `run_script_steps` 期间临时借出再放回），因此系统的拓扑执行序只算一次，
    /// 而不是每步重建调度器 + 重算拓扑序——固定步一帧可跑 8 步，这份开销纯属浪费。
    pub(crate) step_scheduler: Scheduler<RuntimeScene>,
    /// 共享帧调度管线：每次 `run_script_steps` 调用只执行一次的系统。
    pub(crate) frame_scheduler: Scheduler<RuntimeScene>,
    /// 系统注册用**名字**记录，供诊断与重名校验；系统本体归调度器所有。
    pub(crate) step_system_names: Vec<String>,
    pub(crate) frame_system_names: Vec<String>,
    /// 当前固定步的输入快照：由 `run_script_steps` 注入、`input_apply` 消费，
    /// `ui_click_dispatch` 复用，步结束即清空。
    pub(crate) pending_step_input: Option<InputSnapshot>,
    /// 最近一次 `run_script_steps` 的调度统计（M18-03 性能统计出口）。
    pub(crate) last_schedule: Option<FrameScheduleReport>,
    /// 手柄设备轮询（M20-06）：每帧采样后合并进本步输入快照。
    pub(crate) gamepad: GamepadPoller,
    /// WP2：是否把 `parent` 当**局部空间**解释（负责人裁定）。默认关闭 = 旧行为
    /// （`parent` 只是组织关系，实体位置就是世界位置）；打开后每帧传播世界变换，
    /// 消费者读 `WorldTransform`。关掉即回滚，且关掉时世界变换组件会被清掉。
    pub(crate) spatial_inheritance: bool,
    /// docs/54 §4.1：**场景声明的** `parent` 语义（与 `spatial_inheritance` 区分：后者是当前
    /// 生效的开关，宿主可覆盖；这一份是场景自己说的，用于诊断与验收）。
    pub(crate) spatial_mode: crate::core::scene_space::SpatialMode,
    /// docs/54 §10 批 R：上一帧传播用的**局部真源指纹**（`(hash, 组件数, Parent 数)`）。
    /// 与上一帧相同 ⇒ 世界值仍然是上一帧算出来的那一份，整趟跳过传播。
    pub(crate) spatial_fingerprint: Option<(u64, usize, usize)>,
    /// 跳过策略的开关（回滚档：`false` = 每帧全量传播，与收敛前逐字节一致）。
    pub(crate) spatial_skip_unchanged: bool,
    /// 累计"因为输入没变而跳过传播"的帧数（诊断/验收用）。
    pub(crate) spatial_skipped_frames: u64,
    /// docs/54 §10 批 S：**逐实体增量**传播的状态（上一帧的输入快照 + 父先序求解顺序）。
    pub(crate) spatial_incremental: crate::core::hierarchy::SpatialIncremental,
    /// 增量策略的开关（回滚档：`false` = 指纹变化后一律全量传播，与批 S 之前逐字节一致）。
    pub(crate) spatial_incremental_enabled: bool,
    /// 最近一次传播走的路径（`Skipped` / `Full` / `Incremental`）——门与验收读它。
    pub(crate) last_spatial_path: crate::core::hierarchy::SpatialPath,
    /// 最近一次传播**真正重算**的实体数（增量路径 = 脏子树大小）。这是"增量真的生效"的证据：
    /// 只挪一个叶子时它必须是 1，而不是实体总数。
    pub(crate) last_spatial_recomputed: usize,
    /// 累计的三条路径调用次数 `[跳过, 全量, 增量]`（取走即清零；一步里传播会被调用多次）。
    pub(crate) spatial_path_counts: [u64; 3],
    /// 累计"真正重算的实体数"（取走即清零）——一帧里只挪一个叶子时它应当是 1。
    pub(crate) spatial_recomputed_total: u64,
    /// 最近一次空间传播的诊断（环/无法无损表达），供宿主与测试查。
    pub(crate) last_spatial_report: SpatialReport,
    /// 本帧是否**真的**在传播世界变换（= 开关打开且场景里存在 `Parent`）。提取路径按它选
    /// 遍历的组件：为假时读局部值，与"没有层级"的语义完全一致，也不花任何传播成本。
    pub(crate) spatial_active: bool,
    /// WP7：谁有权写 `Transform`（默认 `Physics` = 模拟结果回写，与既有行为一致）。
    pub(crate) physics_authority: PhysicsAuthority,
    /// WP7：阶段顺序（`Unified` = 脚本→同步→物理→回写→世界变换；`Legacy` = 旧的单系统
    /// "步进即回写"顺序，作为回滚档）。
    pub(crate) physics_stage_order: PhysicsStageOrder,
    /// WP6：世界 Sprite 是否走**带深度**的通道（默认关闭 = 现状"3D 之后的 overlay"）。
    /// 打开后精灵以世界坐标产出（`SceneMesh::world_sprites`），由渲染器的世界 Sprite 通道投影
    /// 并参与深度测试；关掉即逐字节回到 overlay 路径——这就是回滚档。
    pub(crate) world_sprite_depth: bool,
    /// docs/54 §8-①：最近一步由**场景声明的发射器**发出的粒子个数（诊断与验收用）。
    pub(crate) last_scene_emission: usize,
    /// WP7：最近一步 `physics_sync` 同步的刚体数 `(2D, 3D)`。0 表示这一步没做步前同步
    /// （默认 authority=Physics，或回滚档 `Legacy`）——因此它同时也是"回滚档真的生效"的证据。
    pub(crate) last_physics_sync: (usize, usize),
    /// docs/54 §4.1：最近一步**每实体脚本**实际调用的实体数（诊断与验收用）。
    ///
    /// 0 既可能是"没有脚本实体"，也可能是"这一步没跑脚本格"——它与
    /// [`Self::last_schedule`] 一起看才能区分，因此诊断里两个都报。
    pub(crate) last_entity_script_calls: usize,
    /// docs/54 §4.2：宿主声明的**视图可见遮罩**（`world`, `overlay`），分别对应
    /// `FrameViews::{main, overlay}.visibility`。默认 `u32::MAX` = 全可见，因此不声明时行为不变。
    pub(crate) view_world_mask: u32,
    pub(crate) view_overlay_mask: u32,
    /// 最近一次提取里被视图遮罩拦掉的实体数（诊断用：区分"被遮罩拦住"与"根本没提取到"）。
    ///
    /// 用 `Cell` 是因为 `scene_3d` 是 `&self`（编辑器与宿主都只借用），而它同样要记这个数。
    /// 语义与 `take_script_micros` 一致：**取走即清空**。
    pub(crate) view_filtered: std::cell::Cell<usize>,
}

/// WP7 阶段顺序（回滚档）。
///
/// - `Unified`（默认）：脚本 → 静态/运动学同步 → 物理 → 动态回写 → 世界变换传播。
/// - `Legacy`：回到收敛前的单系统顺序（"物理步进时立刻回写"，2D 的回写夹在 3D 推进之前）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PhysicsStageOrder {
    #[default]
    Unified,
    Legacy,
}

/// Aggregated scheduler statistics of one `run_script_steps` call (M18-03).
///
/// Per-system `elapsed` values are accumulated across every fixed step of the
/// call, so a system that runs `steps` times reports its total time.
#[derive(Clone, Debug, Default)]
pub struct FrameScheduleReport {
    /// Number of fixed steps the per-step pipeline executed.
    pub steps: u32,
    /// Accumulated wall time of all scheduled pipeline executions.
    pub total_elapsed: Duration,
    /// Per-system timings in first-execution order.
    pub timings: Vec<SystemTiming>,
}

impl FrameScheduleReport {
    fn merge(&mut self, report: &ScheduleReport) {
        for timing in &report.timings {
            if let Some(existing) = self
                .timings
                .iter_mut()
                .find(|entry| entry.name == timing.name)
            {
                existing.elapsed += timing.elapsed;
            } else {
                self.timings.push(timing.clone());
            }
        }
        self.total_elapsed += report.total_elapsed;
    }
}

#[derive(Clone, Copy, Debug)]
struct CachedTileSprite {
    center_offset: Vec2,
    uv_min: Vec2,
    uv_max: Vec2,
}

/// 分量级相等：测试里用 ssert_eq! 对比 chunk 化展平与整层平铺参考
/// （Vec2 未实现 PartialEq，此处按分量逐一比较——数值由同一公式算出，精确相等）。
impl PartialEq for CachedTileSprite {
    fn eq(&self, other: &Self) -> bool {
        self.center_offset.x == other.center_offset.x
            && self.center_offset.y == other.center_offset.y
            && self.uv_min.x == other.uv_min.x
            && self.uv_min.y == other.uv_min.y
            && self.uv_max.x == other.uv_max.x
            && self.uv_max.y == other.uv_max.y
    }
}

/// WP4 ④ chunk 级脏重建：每层按固定块（32×32 格）切分，编辑只重建变化的块。
/// `ox/oy` 是块左上角的**格坐标**；边缘块宽/高可能不满 [`TILE_CHUNK_SIZE`]。
/// 块内 `sprites` 按块内行主序（与整层行主序拼接后的顺序一致——像素等价由此兜住）。
#[derive(Clone, Debug)]
struct TileChunk {
    ox: usize,
    oy: usize,
    w: usize,
    h: usize,
    sprites: Vec<CachedTileSprite>,
}

/// 文本渲染后端的项目级策略。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum TextRenderMode {
    /// 大号短文本用矢量轮廓，其余用 MSDF/位图。
    #[default]
    Auto,
    /// 全部文本优先用矢量轮廓；字体不可用时仍回退 SDF。
    Vector,
    /// 全部文本用 MSDF/内置位图。
    Sdf,
}

impl TextRenderMode {
    fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "vector" | "outline" => Self::Vector,
            "sdf" | "msdf" => Self::Sdf,
            _ => Self::Auto,
        }
    }
}

pub(crate) struct CachedTilemapLayer {
    transform: Transform,
    /// 比对基准：命中判断与逐块脏比对都拿它跟当前 world 里的 `Tilemap` 比较。
    tilemap: Tilemap,
    /// 行主序块排列的瓦片块缓存。
    chunks: Vec<TileChunk>,
    /// 全层非空格总数（append 前置判断与顶点容量预留用，避免每帧重算）。
    total: usize,
}

impl RuntimeScene {
    pub fn new(project_root: Option<&Path>) -> anyhow::Result<Self> {
        let scene = load_runtime_scene(project_root);
        Self::init(project_root, scene, true, false)
    }

    /// P3：显式声明"这是发布包"（`game --pack`）。决定存档与玩家设置写到哪里——
    /// 发布包 → 用户数据目录（`%APPDATA%\<app>`），开发工程 → 工程内。
    /// 不显式声明时 `user_dir::is_release_pack` 仍能凭 `<exe>/pack` + `release.toml`
    /// 认出双击启动的发布包。
    pub fn new_with_layout(project_root: Option<&Path>, packaged: bool) -> anyhow::Result<Self> {
        let scene = load_runtime_scene(project_root);
        Self::init(project_root, scene, true, packaged)
    }

    /// 实例化一个具体场景（编辑器"运行当前场景"用），而非工程的启动场景。
    /// 编辑器处理的永远是开发工程：玩家设置留在工程根（面板直接编辑那份）。
    pub fn from_scene(project_root: Option<&Path>, scene: Scene) -> anyhow::Result<Self> {
        Self::init(project_root, scene, true, false)
    }

    /// Instantiates a scene WITHOUT running the project script, for the editor's
    /// scene view (render the scene as edited, no game logic).
    pub fn from_scene_edit(project_root: Option<&Path>, scene: Scene) -> anyhow::Result<Self> {
        Self::init(project_root, scene, false, false)
    }

    /// Lua 侧 `app.quit()` 只置引擎的退出请求标记（见 `script.rs`），桌面运行器
    /// 借此轮询并在帧末真正退出，否则“退出”按钮在窗口环境无效。
    pub fn quit_requested(&self) -> bool {
        self.script.quit_requested()
    }

    /// 取走并清零"上次取走以来 Lua `update()` 的累计耗时"（微秒）。
    ///
    /// 宿主每次跑完固定步后调用一次，喂给 `PerformanceStats::lua_ms`。用"取走即清零"
    /// 而不是查询，是为了让 0 步帧（帧率高于逻辑步时）自然得到 0，不会把上一帧的
    /// Lua 耗时重复计入。
    pub fn take_script_micros(&mut self) -> u64 {
        std::mem::take(&mut self.script_micros)
    }

    /// docs/54 §4.1：按**场景实体 id** 读运行时局部变换，**用统一的 `LocalTransform` 表示**。
    ///
    /// 运行时 ECS 里存的是 `Transform`（2D）与 `Transform3D`（3D）——它们是**序列化边界格式**，也是
    /// 2D 快路径（平面 f32 弧度比四元数便宜，10k 精灵档实测过）。跨空间、跨系统要"一个局部变换"
    /// 时就该走这里：`LocalTransform` 是引擎内**唯一的规范表示**（组合、反解、换父、迁移都基于它，
    /// 见 docs/40 WP2 与 docs/54 §8-②），这个读法把它暴露出来，而不是让每个调用方各写一次转换。
    #[must_use]
    pub fn local_transform_of(
        &self,
        entity: SceneEntityId,
    ) -> Option<crate::core::spatial_transform::LocalTransform> {
        let runtime = self.script.scene_entity_handle(entity)?;
        self.script.with_world(|world| {
            if let Ok(Some(transform)) = world.get::<Transform3D>(runtime) {
                return Some(crate::core::spatial_transform::LocalTransform::from_3d(
                    *transform,
                ));
            }
            world
                .get::<Transform>(runtime)
                .ok()
                .flatten()
                .and_then(|transform| {
                    crate::core::spatial_transform::LocalTransform::from_2d(*transform).ok()
                })
        })
    }

    /// docs/54 §4.1：把统一的 `LocalTransform` 写回该实体**自己的空间**。
    ///
    /// 三条明确的出口（都不猜）：
    /// - 实体有 `Transform3D` ⇒ 原样写回；
    /// - 实体只有 2D `Transform` ⇒ 只在**真能无损压平**时才写（四元数只有 z/w 分量、z 位移为 0、
    ///   z 缩放为 1），否则**报错**——手抄一个 XY 会把旋转与缩放悄悄丢掉（docs/54 §1.2）；
    /// - 两种都没有 ⇒ `Ok(false)`（没有局部变换可更新，调用方自己决定要不要新建组件）。
    ///
    /// 拆分批 8：写成功即视为**相机相关的世界变更**（`pose=transform` 的相机可能就在这个实体或
    /// 它的父链上），因此作废本帧定下的 3D 视图。
    pub fn set_local_transform(
        &mut self,
        entity: SceneEntityId,
        value: crate::core::spatial_transform::LocalTransform,
    ) -> anyhow::Result<bool> {
        let Some(runtime) = self.script.scene_entity_handle(entity) else {
            return Ok(false);
        };
        let written = self
            .script
            .with_world_mut(|world| -> anyhow::Result<bool> {
                if world.get::<Transform3D>(runtime).ok().flatten().is_some() {
                    let transform = Transform3D {
                        position: value.translation,
                        rotation: value.rotation,
                        scale: value.scale,
                    };
                    world.insert(runtime, transform)?;
                    return Ok(true);
                }
                if world.get::<Transform>(runtime).ok().flatten().is_some() {
                    let flat =
                        crate::core::spatial_transform::plane_local(value).ok_or_else(|| {
                            anyhow::anyhow!(
                                "this entity only has a 2D Transform; the value is not planar \
                         (tilt, z offset or z scale) — flattening would drop it"
                            )
                        })?;
                    world.insert(runtime, flat)?;
                    return Ok(true);
                }
                Ok(false)
            })?;
        if written {
            self.invalidate_frame_view_3d();
        }
        Ok(written)
    }

    /// WP2 ④：按**场景实体 id** 读世界变换（2D）。
    ///
    /// 编辑器拾取包围框必须与渲染用同一份位置：场景视图渲染走 `vertices()`（父链组合后的
    /// 世界变换），若包围框仍读文档里的局部值，带父子关系的实体会**框在人不在的地方**。
    /// 继承未生效（没有 `Parent`）时返回的正是局部值，因此调用方无需分支。
    pub fn world_transform_2d_of(&self, entity: SceneEntityId) -> Option<Transform> {
        let runtime = self.script.scene_entity_handle(entity)?;
        self.script
            .with_world(|world| crate::core::hierarchy::world_transform_2d(world, runtime))
            .ok()
            .flatten()
    }

    /// WP2 ④：按**场景实体 id** 读世界变换（3D）。
    pub fn world_transform_3d_of(&self, entity: SceneEntityId) -> Option<Transform3D> {
        let runtime = self.script.scene_entity_handle(entity)?;
        self.script
            .with_world(|world| crate::core::hierarchy::world_transform_3d(world, runtime))
            .ok()
            .flatten()
    }

    /// 设置 2D 相机（编辑器场景视图用，匹配编辑器 pan/zoom）。
    pub fn set_camera_2d(&mut self, camera: Camera2D) {
        self.camera = camera;
    }

    /// WP2 ④ 回滚档：`parent` 是否按**局部空间**解释（默认关闭 = 旧行为）。打开后每帧把
    /// 局部变换沿父链组合成世界变换（`WorldTransform` / `WorldTransform3D`），渲染与 Lua
    /// 读到的位置因此是**世界**位置；关掉时这些组件会被清掉，消费者退回局部值。
    pub fn set_spatial_inheritance(&mut self, enabled: bool) {
        self.spatial_inheritance = enabled;
        self.propagate_spatial_now();
    }

    /// 当前空间继承开关。
    #[must_use]
    pub const fn spatial_inheritance(&self) -> bool {
        self.spatial_inheritance
    }

    /// docs/54 §10 批 R：开关"输入没变就跳过传播"。返回先前是否启用。
    ///
    /// - `true`（默认）：静态层级场景整趟跳过传播（世界值仍是上一帧算出来的那一份）。
    /// - `false`：**回滚档**——每帧全量传播，与批 R 之前逐字节一致。
    ///
    /// 跳过只依据"局部真源指纹 + 是否启用继承"，因此任何局部值/父子边/组件增删都会让下一帧退回
    /// 全量传播；指纹不参与渲染，也不是第二份真值。
    pub fn set_spatial_skip_unchanged(&mut self, enabled: bool) -> bool {
        let previous = self.spatial_skip_unchanged;
        self.spatial_skip_unchanged = enabled;
        if !enabled {
            // 关掉时清掉指纹：打开开关后第一帧必须重新全量传播一次（不能靠旧指纹跳过）。
            self.spatial_fingerprint = None;
        }
        previous
    }

    /// 当前是否启用"输入没变就跳过传播"。
    #[must_use]
    pub const fn spatial_skip_unchanged(&self) -> bool {
        self.spatial_skip_unchanged
    }

    /// 自上次取走以来"因为输入没变而跳过传播"的帧数（取走即清零）。
    pub fn take_spatial_skipped_frames(&mut self) -> u64 {
        std::mem::take(&mut self.spatial_skipped_frames)
    }

    /// 累计跳过帧数（不清零）。
    #[must_use]
    pub const fn spatial_skipped_frames(&self) -> u64 {
        self.spatial_skipped_frames
    }
    /// docs/54 §4.1：**场景声明的** `parent` 语义（未声明 ⇒ 组织关系）。宿主开关可以覆盖生效值，
    /// 但这一份永远是场景自己说的，因此"运行时为什么继承/不继承"有据可查。
    #[must_use]
    pub const fn spatial_mode(&self) -> crate::core::scene_space::SpatialMode {
        self.spatial_mode
    }

    /// 本帧是否真的在传播世界变换（开关打开**且**场景里存在父子关系）。
    #[must_use]
    pub const fn spatial_active(&self) -> bool {
        self.spatial_active
    }

    /// 最近一次空间传播的诊断（环、无法无损表达为 TRS 的父子组合）。空表示全部建立成功。
    #[must_use]
    pub fn last_spatial_report(&self) -> &SpatialReport {
        &self.last_spatial_report
    }

    /// WP7：声明谁有权写 `Transform`（默认 `Physics`）。见 [`PhysicsAuthority`]。
    pub const fn set_physics_authority(&mut self, authority: PhysicsAuthority) {
        self.physics_authority = authority;
    }

    /// 当前物理权威。
    #[must_use]
    pub const fn physics_authority(&self) -> PhysicsAuthority {
        self.physics_authority
    }

    /// WP7 回滚档：阶段顺序。`Legacy` 回到"物理步进即回写"的单系统顺序。
    pub const fn set_physics_stage_order(&mut self, order: PhysicsStageOrder) {
        self.physics_stage_order = order;
    }

    /// 当前阶段顺序。
    #[must_use]
    pub const fn physics_stage_order(&self) -> PhysicsStageOrder {
        self.physics_stage_order
    }

    /// WP6 回滚档：世界 Sprite 是否走带深度的通道（默认关闭）。
    pub fn set_world_sprite_depth(&mut self, enabled: bool) {
        self.world_sprite_depth = enabled;
    }

    /// 当前世界 Sprite 深度通道开关。
    #[must_use]
    pub const fn world_sprite_depth(&self) -> bool {
        self.world_sprite_depth
    }

    /// WP6：本帧世界 Sprite 的投影矩阵（宿主喂给 `Renderer::set_world_sprite_camera`）。
    /// 关闭时返回 `None`（渲染器不会用到它）。
    ///
    /// docs/54 §8-③/④：宿主**声明了主视图**时（混合 2D/3D 场景）世界 Sprite 会改用主视图
    /// 的那份矩阵与深度附件，本函数的值只作为纯 2D 画面的回退。
    #[must_use]
    pub fn sprite_view_projection(&self, width: u32, height: u32) -> Option<Mat4> {
        if !self.world_sprite_depth {
            return None;
        }
        let viewport = Viewport::new(width.max(1) as f32, height.max(1) as f32).ok()?;
        // 深度范围取正交视图的默认区间：精灵默认 `z = 0` 落在深度**中间**，向 -z 靠近相机、
        // 向 +z 远离（与 `RenderView::overlay_2d` / `main_2d` 用的是同一组常量）。
        self.camera
            .view_projection(
                viewport,
                crate::core::camera::ORTHO_NEAR,
                crate::core::camera::ORTHO_FAR,
            )
            .ok()
    }

    /// 场景里是否有 3D 内容（网格可见物）。有 ⇒ 宿主会给这一帧声明 3D 主视图，世界通道的
    /// 可见域因此是 3D 视图而不是 2D 相机（docs/54 §8-④：宁可保守不裁剪，也绝不误丢）。
    #[must_use]
    pub fn world_view_has_3d_content(&self) -> bool {
        self.script.with_world(|world| {
            !world
                .entities_with::<crate::core::mesh3d::MeshRenderer>()
                .is_empty()
        })
    }

    /// docs/54 §8-①：推进**场景声明的**粒子发射器。
    ///
    /// 每个带 `SceneParticleEmitter` 的实体按 `rate × dt` 累计发射，发射位置取它的**世界**位置
    /// （父链下的子实体因此跟着父一起动）。累计器写回组件，因此帧率变化不改变平均发射量。
    ///
    /// 返回本步发出的粒子数（诊断/验收用）。发射本身复用 Lua `particles.burst` 的同一实现。
    pub fn step_scene_emitters(&mut self, delta_seconds: f32) -> usize {
        let pending: Vec<(Vec2, ParticleEmitter)> = self.script.with_world_mut(|world| {
            let mut pending = Vec::new();
            for id in world.entities_with::<SceneParticleEmitter>() {
                let Some(mut scene) = world
                    .get::<SceneParticleEmitter>(id)
                    .ok()
                    .flatten()
                    .copied()
                else {
                    continue;
                };
                // 没有变换的发射器没有位置可发，跳过（与"世界变换缺席"的消费端约定一致）。
                let Some(position) = crate::core::hierarchy::world_transform_2d(world, id)
                    .ok()
                    .flatten()
                    .map(|transform| transform.position)
                else {
                    continue;
                };
                let count = scene.take_steps(delta_seconds);
                // 累计器必须写回：否则每步都从 0 开始，低速率发射器永远发不出粒子。
                let _ = world.insert(id, scene);
                if count == 0 {
                    continue;
                }
                let mut emitter = scene.emitter;
                emitter.count = count;
                pending.push((position, emitter));
            }
            pending
        });
        let emitted = pending.len();
        for (position, emitter) in pending {
            self.script.burst_particles(position, &emitter);
        }
        emitted
    }

    /// 最近一步由场景声明的发射器发出的粒子数。
    #[must_use]
    pub const fn last_scene_emission(&self) -> usize {
        self.last_scene_emission
    }

    /// docs/54 §8-②：最近一次场景加载里**坏的 `parent` 边**（悬空父/自环）的可读描述
    /// （空 = 每条 `parent` 都指向同一文档里的实体且不是自己）。
    #[must_use]
    pub fn scene_parent_issues(&self) -> Vec<String> {
        self.script.scene_parent_issues()
    }

    /// WP7：最近一步 `physics_sync` 同步的刚体数 `(2D, 3D)`——0 表示这一步没做步前同步
    /// （默认 authority=Physics，或回滚档 `Legacy`）。
    #[must_use]
    pub const fn last_physics_sync(&self) -> (usize, usize) {
        self.last_physics_sync
    }

    /// docs/54 §4.1：最近一步每实体脚本调用的实体数（诊断/验收用）。
    #[must_use]
    pub const fn last_entity_script_calls(&self) -> usize {
        self.last_entity_script_calls
    }

    /// docs/54 §4.4：当前登记在身份表里的运行时资源（按句柄排序）。
    ///
    /// 这是**跨帧**的表（不像 `texture_paths` 每帧重建），因此可以直接断言"同一路径恒同句柄"。
    /// 返回克隆而不是引用：身份表在 `RefCell` 里（3D 提取是 `&self`，也要能登记网格身份），
    /// 借用作用域没法延长到调用方。
    #[must_use]
    pub fn asset_entries(&self) -> Vec<AssetRecord> {
        self.assets
            .borrow()
            .entries()
            .into_iter()
            .map(|(_, record)| record.clone())
            .collect()
    }

    /// docs/54 §4.3：本帧三种显式显示模式各路由了几个实体 `(world, overlay, screen)`。
    ///
    /// 未声明模式的实体不计数（它们跟随全局开关），因此这三个数只在场景真的用了显式模式时非零——
    /// 也就是"这段几何走哪条路"变成可断言的事实。
    #[must_use]
    pub const fn render_mode_routed(&self) -> (usize, usize, usize) {
        self.render_mode_routed
    }

    /// docs/54 §4.3：加载期点名的"多种可绘制组件"冲突。
    #[must_use]
    pub fn renderable_conflicts(&self) -> Vec<String> {
        self.script.scene_renderable_conflicts()
    }

    /// 某个路径当前的运行时句柄（未登记则 `None`）。
    #[must_use]
    pub fn asset_handle(&self, path: &str) -> Option<AssetHandle> {
        self.assets.borrow().handle_of(path)
    }

    /// docs/54 §4.2：声明本帧两个视图的**可见遮罩**（`world` 对应 `FrameViews::main.visibility`、
    /// `overlay` 对应 `FrameViews::overlay.visibility`）。传同一个 `FrameViews` 值给渲染器与本函数，
    /// 因此不存在第二份真源；不调用则全可见（既有行为与像素不变）。
    pub const fn set_view_visibility(&mut self, world: u32, overlay: u32) {
        self.view_world_mask = world;
        self.view_overlay_mask = overlay;
    }

    /// 本帧两个视图的可见遮罩。
    #[must_use]
    pub const fn view_visibility(&self) -> (u32, u32) {
        (self.view_world_mask, self.view_overlay_mask)
    }

    /// 最近一次提取里被视图遮罩拦掉的实体数（世界遮罩与 overlay 遮罩合计）。
    ///
    /// 取走即清空（与 `take_script_micros` 同一语义），因此调用方拿到的是"自上次取走以来"的数量。
    pub fn take_view_filtered(&self) -> usize {
        self.view_filtered.replace(0)
    }

    /// 看一眼当前累计值（不清空）。
    #[must_use]
    pub fn view_filtered(&self) -> usize {
        self.view_filtered.get()
    }

    /// 传播一次世界变换（幂等）。提取入口（`vertices` / `scene_3d`）与本函数的调用点
    /// 都会走它，因此"这一帧看到的世界位置"永远来自同一次传播。
    pub fn propagate_spatial_now(&mut self) {
        let enabled = self.spatial_inheritance;
        // 只有**真的存在层级**时才传播：平面场景（每个实体都没有 `Parent`）里世界值恒等于局部值，
        // 传播纯属白做——而提取路径是按这个布尔量选组件遍历的，所以它必须与实际写入的组件一致。
        //
        // docs/54 §10 批 R：传播的输入只有四样（参与集合、局部值、父子边、是否启用）。前三样哈希成
        // 一个指纹，**与上一帧相同就整趟跳过**——静态层级场景因此不再付"每实体 3 次组件查找 + 矩阵
        // 组合 + 写入"那三成帧成本。指纹只是"上一帧算出来的世界值还有效吗"的判据，不参与渲染，
        // 也不是第二份真值；任何一位变了（组件增删/实体重生/换父/局部值改动）都会退回全量传播。
        //
        // docs/54 §10 批 S：指纹只说"**有东西**变了"，增量路径再说"**哪些**实体变了"，于是只重算
        // 那些实体的子树。两级判据分工明确：一级是最便宜的哈希（静态帧 0.48 ms/万实体），二级是
        // 逐实体精确快照比较（只在指纹变化时才付）。回滚档是 `set_spatial_incremental(false)`。
        let fingerprint = self
            .script
            .with_world(crate::core::hierarchy::local_fingerprint);
        let (_, _, parents) = fingerprint;
        let active_now = enabled && parents > 0;
        if self.spatial_skip_unchanged
            && self.spatial_fingerprint == Some(fingerprint)
            && self.spatial_active == active_now
        {
            self.spatial_skipped_frames = self.spatial_skipped_frames.saturating_add(1);
            self.spatial_path_counts[0] = self.spatial_path_counts[0].saturating_add(1);
            self.last_spatial_path = crate::core::hierarchy::SpatialPath::Skipped;
            self.last_spatial_recomputed = 0;
            return;
        }
        let incremental = self.spatial_incremental_enabled;
        let (outcome, active) = {
            // 分字段借用：`script` 与增量状态是两个不相干的字段。
            let script = &mut self.script;
            let state = &mut self.spatial_incremental;
            let outcome = script.with_world_mut(|world| {
                if incremental {
                    crate::core::hierarchy::propagate_spatial_incremental(world, state, active_now)
                } else {
                    crate::core::hierarchy::SpatialOutcome {
                        report: crate::core::hierarchy::propagate_spatial(world, active_now),
                        path: crate::core::hierarchy::SpatialPath::Full,
                        recomputed: 0,
                    }
                }
            });
            (outcome, active_now)
        };
        let report = outcome.report;
        self.spatial_fingerprint = Some(fingerprint);
        self.spatial_active = active;
        self.last_spatial_path = outcome.path;
        self.last_spatial_recomputed = outcome.recomputed;
        let slot = match outcome.path {
            crate::core::hierarchy::SpatialPath::Skipped => 0,
            crate::core::hierarchy::SpatialPath::Full => 1,
            crate::core::hierarchy::SpatialPath::Incremental => 2,
        };
        self.spatial_path_counts[slot] = self.spatial_path_counts[slot].saturating_add(1);
        self.spatial_recomputed_total = self
            .spatial_recomputed_total
            .saturating_add(outcome.recomputed as u64);
        if !report.diagnostics.is_empty() && self.last_spatial_report.diagnostics.is_empty() {
            for diagnostic in &report.diagnostics {
                tracing::warn!(
                    target: "scene",
                    entity = ?diagnostic.entity,
                    reason = ?diagnostic.reason,
                    "[scene] 空间继承未能建立：该实体退回局部值（不做近似）"
                );
            }
        }
        self.last_spatial_report = report;
    }

    /// docs/54 §10 批 S：**逐实体增量传播**的开关（默认开启）。返回前一个值。
    ///
    /// 关掉就是回滚档：指纹变化后一律全量传播（与批 S 之前逐字节一致），世界值不变、只是更慢。
    /// 两个方向都会清空缓存，因此重新打开后的第一帧必定全量传播一次。
    pub fn set_spatial_incremental(&mut self, enabled: bool) -> bool {
        let previous = self.spatial_incremental_enabled;
        self.spatial_incremental_enabled = enabled;
        self.spatial_incremental.reset();
        previous
    }

    /// 当前是否启用逐实体增量传播。
    #[must_use]
    pub const fn spatial_incremental(&self) -> bool {
        self.spatial_incremental_enabled
    }

    /// 最近一次传播走的路径（`Skipped` / `Full` / `Incremental`）。
    #[must_use]
    pub const fn last_spatial_path(&self) -> crate::core::hierarchy::SpatialPath {
        self.last_spatial_path
    }

    /// 最近一次传播**真正重算**的实体数（增量路径 = 脏子树大小）。
    #[must_use]
    pub const fn last_spatial_recomputed(&self) -> usize {
        self.last_spatial_recomputed
    }

    /// 累计的三条路径调用次数 `[跳过, 全量, 增量]`（不清零）。
    ///
    /// 一步里传播会被调用多次（步末 + `spatial_propagate` 帧系统），因此"最后一帧走了哪条路"
    /// 需要看计数而不是看 `last_spatial_path`。
    #[must_use]
    pub const fn spatial_path_counts(&self) -> [u64; 3] {
        self.spatial_path_counts
    }

    /// 取走三条路径的调用次数（取走即清零）。
    pub fn take_spatial_path_counts(&mut self) -> [u64; 3] {
        std::mem::take(&mut self.spatial_path_counts)
    }

    /// 累计"真正重算的实体数"（不清零）。
    #[must_use]
    pub const fn spatial_recomputed_total(&self) -> u64 {
        self.spatial_recomputed_total
    }

    /// 取走"真正重算的实体数"（取走即清零）。**这是逐实体增量的判据**：一帧里只挪一个叶子时，
    /// 一整个窗口里重算的实体数应该等于帧数，而不是帧数 × 实体数。
    pub fn take_spatial_recomputed_total(&mut self) -> u64 {
        std::mem::take(&mut self.spatial_recomputed_total)
    }

    /// 当前 2D 相机中心（世界坐标）。编辑器 Scene 视图用它定位设计分辨率包围框，
    /// 使包围框与游戏取景范围一致（默认 `DEFAULT_2D_CAMERA_CENTER`）。
    pub fn camera_center_2d(&self) -> Vec2 {
        self.camera.center
    }

    /// 当前 2D 相机。
    #[must_use]
    pub const fn camera_2d(&self) -> Camera2D {
        self.camera
    }

    /// WP5：本帧的 2D overlay 视图（相机 + 视口 + 清屏 + 顺序的唯一说法）。
    ///
    /// 与 `vertices(width, height)` **同源**（同一个 `self.camera`），因此渲染器不需要
    /// 再自己从别处拼一个相机；宿主把两份视图交给 `Renderer::set_frame_views` 即可。
    pub fn render_view_2d(&self, width: u32, height: u32) -> RenderView {
        // 窗口最小化时宽高可能是 0；`Viewport::new` 拒绝非正数，这里退到 1×1。
        let viewport =
            Viewport::new(width.max(1) as f32, height.max(1) as f32).unwrap_or(Viewport {
                width: 1.0,
                height: 1.0,
            });
        RenderView::overlay_2d(self.camera, viewport)
    }

    /// 设置 Canvas 空间 UI 的参考分辨率（设计分辨率）。同步给 Lua 侧，
    /// 保证渲染与按钮命中测试（`process_ui_clicks`）用同一参考。
    pub fn set_ui_reference(&mut self, reference: [f32; 2]) {
        self.ui_reference = reference;
        self.script.set_ui_reference(reference);
    }

    /// 是否渲染屏幕空间 UI 覆盖层。编辑器场景视图置 false（UI 由 egui 在
    /// 设计分辨率包围框内绘制）；游戏预览保持 true。
    pub fn set_render_ui_overlay(&mut self, enabled: bool) {
        self.render_ui_overlay = enabled;
    }

    /// 设置每精灵顶点生成的并行度（WP8）。`Parallelism::SEQUENTIAL` 即回滚单线程；
    /// 两条路径的输出逐字节相同，因此这是一个纯粹的性能开关，不是行为开关。
    pub fn set_sprite_parallelism(&mut self, parallelism: Parallelism) {
        self.sprite_parallelism = parallelism;
    }

    /// P1-③：系统级数据并行开关（粒子积分等系统内纯数据子任务）。默认取检测到的
    /// 核数；`Parallelism::SEQUENTIAL` 回滚单线程——输出逐位相同（见 `particle` 的测试）。
    pub fn set_system_parallelism(&mut self, parallelism: Parallelism) {
        self.system_parallelism = parallelism;
    }

    /// 当前系统级并行开关。
    #[must_use]
    pub const fn system_parallelism(&self) -> Parallelism {
        self.system_parallelism
    }

    /// P1-①：渲染级空间索引（视口候选预筛）开关。返回旧值（回滚档）；关闭时清掉
    /// 网格缓存（下次打开重建）。与 `set_spatial_index_cell_size` 配套。
    pub fn set_spatial_index(&mut self, enabled: bool) -> bool {
        let previous = self.spatial_index;
        self.spatial_index = enabled;
        if !enabled {
            self.spatial_grid = None;
            self.spatial_index_fingerprint = None;
        }
        previous
    }

    /// P1-①：空间索引网格 cell 边长（世界单位）。返回旧值；只影响性能不影响正确性。
    pub fn set_spatial_index_cell_size(&mut self, cell_size: f32) -> f32 {
        let previous = self.spatial_index_cell_size;
        self.spatial_index_cell_size = cell_size;
        previous
    }

    /// WP7 ②：collider 调试绘制开关。默认关闭；Lua 可用 `physics.debug_draw(true)` 打开。
    pub fn set_collider_debug_draw(&mut self, enabled: bool) {
        self.collider_debug_draw = enabled;
    }

    /// 当前 collider 调试绘制开关。
    #[must_use]
    pub const fn collider_debug_draw(&self) -> bool {
        self.collider_debug_draw
    }

    /// docs/54 §8-⑥：3D collider 调试绘制开关。默认关闭；Lua 用
    /// `physics3d.debug_draw(true)` 打开，编辑器/验收也可以直接设。
    pub fn set_collider_debug_draw_3d(&mut self, enabled: bool) {
        self.collider_debug_draw_3d = enabled;
    }

    /// 当前 3D collider 调试绘制开关。
    #[must_use]
    pub const fn collider_debug_draw_3d(&self) -> bool {
        self.collider_debug_draw_3d
    }

    /// WP4 ④：视口外瓦片剔除开关（默认开启）。关掉即"整层全画"，输出像素必须与开启时
    /// **逐字节相同**——这是该优化的回滚档，也是它正确性的对拍基准。
    pub fn set_tilemap_culling(&mut self, enabled: bool) {
        self.tilemap_culling = enabled;
    }

    /// 当前视口外瓦片剔除开关。
    #[must_use]
    pub const fn tilemap_culling(&self) -> bool {
        self.tilemap_culling
    }

    /// WP6：视口外精灵剔除开关（默认开启）。关掉即"所有精灵都生成顶点"，像素必须与开启时
    /// **逐字节相同**——这是回滚档，也是正确性的对拍基准。
    pub fn set_sprite_culling(&mut self, enabled: bool) {
        self.sprite_culling = enabled;
    }

    /// 当前视口外精灵剔除开关。
    #[must_use]
    pub const fn sprite_culling(&self) -> bool {
        self.sprite_culling
    }

    /// 把 collider 调试绘制追加到 overlay 缓冲（WP7 ②）。
    ///
    /// 数据源是**物理世界当前姿态**（`PhysicsWorld::debug_shapes`），不是场景里的组件串：
    /// 组件只在实例化时解析一次，而动态刚体每步都在动，调试绘制要看的正是后者。
    /// 动态体画绿、固定体画青、sensor 画黄——三者物理响应完全不同，颜色不分辨会误判。
    ///
    /// 全部用白色纹理，因此只追加**一个** draw range；画在世界内容之上、屏幕 UI 之下。
    fn push_collider_debug(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        viewport: Viewport,
    ) {
        if !self.collider_debug_draw {
            return;
        }
        let shapes = self.script.collider_debug_shapes();
        if shapes.is_empty() {
            return;
        }
        // 预算项每帧一次：逐段投影因此不再校验相机、不再算三角函数。
        let Ok(camera) = OrthoTerms::new(self.camera, viewport) else {
            return;
        };
        let start = vertices.len();
        for shape in shapes {
            // 传感器黄、动态体绿、固定体青：三种响应**不是**同一件事，画成一个颜色会让
            // "物理到底动了什么"看不出来（docs/54 §8-⑥ 的调试契约）。
            let color = if shape.sensor {
                [1.0, 0.85, 0.2, 1.0]
            } else if shape.dynamic {
                [0.2, 1.0, 0.35, 1.0]
            } else {
                [0.25, 0.75, 1.0, 1.0]
            };
            // 轮廓本身由 `ColliderShape::wireframe_edges` 给出（局部坐标，2D/3D 同一套语义），
            // 这里只做姿态变换与投影。折线首尾相接，再逐段投影成细四边形。
            for (from, to) in shape.wireframe_segments() {
                // 非有限端点跳过该段（与旧的"投影失败即放弃"同一语义）。
                if !from.x.is_finite()
                    || !from.y.is_finite()
                    || !to.x.is_finite()
                    || !to.y.is_finite()
                {
                    continue;
                }
                let from = camera.world_to_screen(from);
                let to = camera.world_to_screen(to);
                push_screen_line(vertices, [from.x, from.y], [to.x, to.y], color, viewport);
            }
        }
        let count = vertices.len() - start;
        if count > 0 {
            draws.push(SceneDraw {
                texture: 0, // 0 = 内置白色纹理
                start: start as u32,
                count: count as u32,
                sdf: false,
                bitmap: false,
                vector: false,
                render_layer: RenderLayer::Overlay2D,
            });
        }
    }

    /// 当前每精灵顶点生成的并行度。
    #[must_use]
    pub const fn sprite_parallelism(&self) -> Parallelism {
        self.sprite_parallelism
    }

    /// docs/54 §8-⑥：把 **3D** collider 调试线框追加到 overlay 缓冲。
    ///
    /// 与 2D 那条的唯一区别是投影：2D 用 `Camera2D::world_to_screen`，这里用本帧 3D pass
    /// 实际用的那台相机（`active_camera3d`）+ `picking::screen_from_world_unclipped`——
    /// 同一份投影数学，因此"看到的位置"和"射线命中的位置"必然一致（§8-⑦ 的要求）。
    /// 线框的边由 `ColliderShape3d::wireframe_edges` 给出（段数固定，可断言）。
    fn push_collider_debug_3d(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        viewport: Viewport,
    ) {
        if !self.collider_debug_draw_3d {
            return;
        }
        let shapes = self.script.collider_debug_shapes_3d();
        if shapes.is_empty() {
            return;
        }
        // 每帧烘焙一次 3D 视图：逐段投影因此不再重建矩阵（此前每个端点都会重建一次
        // `view_projection_matrix`，等于每段两次 4×4 乘法加两次校验）。
        // 拆分批 8：同帧同视口直接吃 `vertices()` 定帧的那一份（不再多解析一次相机）。
        let Some(view) = self.frame_camera_view(viewport) else {
            return;
        };
        let start = vertices.len();
        for shape in shapes {
            let color = if shape.sensor {
                [1.0, 0.85, 0.2, 1.0]
            } else if shape.dynamic {
                [0.2, 1.0, 0.35, 1.0]
            } else {
                [0.25, 0.75, 1.0, 1.0]
            };
            for (from, to) in shape.wireframe_segments() {
                let (Some(from), Some(to)) = (
                    crate::core::picking::screen_from_view_unclipped(view, from, viewport),
                    crate::core::picking::screen_from_view_unclipped(view, to, viewport),
                ) else {
                    continue; // 相机背后/远裁剪面之外：不画，也不猜
                };
                push_screen_line(vertices, [from.x, from.y], [to.x, to.y], color, viewport);
            }
        }
        let count = vertices.len() - start;
        if count > 0 {
            draws.push(SceneDraw {
                texture: 0, // 0 = 内置白色纹理
                start: start as u32,
                count: count as u32,
                sdf: false,
                bitmap: false,
                vector: false,
                render_layer: RenderLayer::Overlay2D,
            });
        }
    }

    /// Updates one authored entity's 2D transform in-place for editor drag
    /// preview. Avoids rebuilding Lua/ECS for every mouse-move revision.
    pub fn set_scene_entity_transform_preview(
        &mut self,
        id: SceneEntityId,
        position: Vec2,
        rotation: f32,
        scale: Vec2,
    ) -> bool {
        let Some(handle) = self.script.scene_entity_handle(id) else {
            return false;
        };
        let mut updated = false;
        self.script.with_world_mut(|world| {
            if let Ok(Some(transform)) = world.get_mut::<Transform>(handle) {
                transform.position = position;
                transform.rotation = rotation;
                transform.scale = scale;
                updated = true;
            }
        });
        if updated {
            // 拆分批 8：拖拽预览改的是世界姿态——`pose=transform` 的相机可能就在被拖的父链上，
            // 因此本帧已定帧的 3D 视图作废（编辑器每帧重渲染，下一次消费会重新定帧）。
            self.invalidate_frame_view_3d();
        }
        updated
    }

    /// 宿主侧写入一个瓦片格（WP4 ④）：编辑器画笔的实时预览与基准都要能在**不重建整个运行时**
    /// 的前提下改一格。走的是与 Lua `tilemap.set_cell` 同一个 `Tilemap::set_cell`，因此校验
    /// 与边界规则只有一份。返回 `false` 表示该实体不存在或没有 `Tilemap` 组件。
    pub fn set_tilemap_cell(
        &mut self,
        id: SceneEntityId,
        x: usize,
        y: usize,
        value: u8,
    ) -> anyhow::Result<bool> {
        let Some(handle) = self.script.scene_entity_handle(id) else {
            return Ok(false);
        };
        let mut updated = false;
        let mut failure: Option<anyhow::Error> = None;
        self.script
            .with_world_mut(|world| match world.get_mut::<Tilemap>(handle) {
                Ok(Some(tilemap)) => match tilemap.set_cell(x, y, value) {
                    Ok(()) => updated = true,
                    Err(error) => failure = Some(error.into()),
                },
                Ok(None) => {}
                Err(error) => failure = Some(error.into()),
            });
        match failure {
            Some(message) => Err(message),
            None => Ok(updated),
        }
    }

    /// Updates one authored UI entity's four-edge offsets in-place for editor
    /// drag preview. The document is only committed when the drag ends.
    pub fn set_scene_entity_ui_layout_preview(
        &mut self,
        id: SceneEntityId,
        offsets: [f32; 4],
    ) -> bool {
        let Some(handle) = self.script.scene_entity_handle(id) else {
            return false;
        };
        let mut updated = false;
        self.script.with_world_mut(|world| {
            if let Ok(Some(layout)) = world.get_mut::<UiRectLayout>(handle) {
                layout.offsets = offsets;
                updated = true;
            }
        });
        if updated {
            // Runtime UI overlay resolves from this snapshot, not the ECS copy.
            // `ui_scene_nodes` is keyed by visible entity order, while `id` is
            // the sparse authoring ID. The live ECS slot index is that order.
            let layout_key = handle.index() as u64;
            if let Some(node) = self.ui_scene_nodes.get_mut(&layout_key)
                && let Some(layout) = node.layout.as_mut()
            {
                layout.offsets = offsets;
            }
            self.ui_layout_rects.clear();
        }
        updated
    }

    /// Returns exact resolved UI rects by authored entity ID. These are the
    /// same rects consumed by the next game-screen render, so editor selection
    /// geometry and rendering share one layout resolution.
    pub fn scene_entity_ui_layout_rects(
        &mut self,
        ids: impl IntoIterator<Item = SceneEntityId>,
        viewport: [f32; 2],
    ) -> BTreeMap<u64, UiRect> {
        self.ensure_scene_ui_layout(viewport);
        ids.into_iter()
            .filter_map(|id| {
                let handle = self.script.scene_entity_handle(id)?;
                self.ui_layout_rects
                    .get(&(handle.index() as u64))
                    .map(|node| (id.0, node.rect))
            })
            .collect()
    }

    /// 覆盖 project.toml 的文本渲染模式（编辑器保存设置后重建运行时可注入）。
    pub fn set_text_render_mode(&mut self, mode: TextRenderMode) {
        self.text_render_mode = mode;
    }

    /// 注入共享字体图集（编辑器在运行时重建时复用，避免每帧重新生成 MSDF 字形）。
    pub fn set_font_atlas(&mut self, font_atlas: Option<Arc<Mutex<FontAtlas>>>) {
        self.font_atlas = font_atlas;
    }

    /// 注入共享默认矢量轮廓图集（编辑器跨运行时重建复用）。
    pub fn set_vector_atlas(&mut self, vector_atlas: Option<VectorFontHandle>) {
        self.vector_atlas = vector_atlas;
    }

    fn init(
        project_root: Option<&Path>,
        scene: Scene,
        run_script: bool,
        packaged: bool,
    ) -> anyhow::Result<Self> {
        // docs/54 §4.1：`parent` 的默认语义由**场景自身**声明，而不是由宿主猜。未声明 ⇒ 组织
        // 关系（v2 历史语义），`spatial=local` ⇒ 父子参与空间组合（迁移后的场景）。宿主仍可用
        // `set_spatial_inheritance` 覆盖这次判断（回滚档）。
        let spatial_mode = crate::core::scene_space::spatial_mode(&scene);
        if let Some(issue) = crate::core::scene_space::spatial_mode_issue(&scene) {
            tracing::warn!(target: "scene", scene = %scene.name, "{issue}");
        }
        let mut script = LuaRuntime::new()?;
        // Project root must be set before instantiation so scene components that
        // load assets (e.g. Animation3D) can resolve their mesh paths.
        if let Some(root) = project_root {
            script.set_project_root(root.to_path_buf());
        }
        let instantiated = if run_script {
            script.instantiate_scene(&scene)?
        } else {
            script.instantiate_scene_with_editor_visibility(&scene, true)?
        };
        tracing::info!(target: "scene", "[scene] loaded name={} entities={}", scene.name, scene.entities.len());
        tracing::info!(target: "ecs", "[ecs] world initialized entities={instantiated}");
        if run_script && let Some(root) = project_root {
            script.set_audio_asset_dir(root.join("assets").join("audio"));
            // P3 用户数据目录：发布包（`<exe>/pack` + `release.toml`，或 `--pack`）
            // 把存档与玩家设置写到用户目录（`%APPDATA%\<app>`），不写程序目录；
            // 开发工程保持工程内 `.cache/save.sav` 与工程根 `settings.toml`。
            let roots = crate::core::user_dir::data_roots(root, packaged);
            if let Err(error) = crate::core::user_dir::ensure_save_dir(&roots) {
                tracing::warn!(target: "save", "{}", error);
            }
            if roots.packaged {
                script.set_settings_root(roots.settings_dir.clone());
                match crate::core::user_dir::seed_settings(&roots, root) {
                    Ok(true) => tracing::info!(
                        target: "app",
                        path = %roots.settings_file().display(),
                        "[app] player settings seeded from the release package"
                    ),
                    Ok(false) => {}
                    Err(error) => tracing::warn!(target: "app", "{}", error),
                }
                tracing::info!(
                    target: "save",
                    save = %roots.save_file().display(),
                    settings = %roots.settings_file().display(),
                    app = %roots.app_name,
                    "[save] release layout: user data directory in use"
                );
            }
            script.set_save_path(roots.save_file());
            let script_path = root.join("scripts/main.lua");
            if script_path.is_file() {
                script.execute_project_script(&script_path)?;
            } else {
                tracing::info!(target: "script", path = %script_path.display(), "[script] game project has no scripts/main.lua; script runtime idle");
            }
        }
        let text_render_mode = project_text_render_mode(project_root);
        // docs/54 §8-①：场景声明了自己的 `Camera2D`（正交相机）就用它当**初始**相机；
        // 没声明则保持历史默认（中心 = 设计分辨率中心、zoom = 1）。之后 Lua `camera2d.*`
        // 与相机控制器照旧移动它——声明的相机是起点，不是第二份姿态真源。
        let declared_camera = script.with_world(declared_camera_2d_in);
        let camera = match declared_camera {
            Some(camera) => camera,
            None => Camera2D::new(crate::core::camera::DEFAULT_2D_CAMERA_CENTER, 1.0)?,
        };
        if declared_camera.is_some() {
            tracing::info!(
                target: "render",
                center = ?camera.center,
                zoom = camera.zoom,
                "[render] 2D camera comes from the scene's Camera2D component"
            );
        }
        tracing::info!(target: "render", "[render] SpriteBatch and Camera2D initialized");
        // 字体图集不再在此无条件加载：`load_project_font_atlas` 会按 chars.txt 预热
        // 全部 MSDF 字形，debug 构建下约 2s/项目（实测 ui_demo 135 字 ≈ 2.3s）。
        // 编辑器场景视图/预览通过 `set_font_atlas` 注入共享（已预热）图集；未注入
        // 时由 `ensure_font_atlas` 在首次排版文本时按需加载。这样场景编辑（revision
        // 变化 → 运行时重建）不再每次重建都白白预热一遍被丢弃的图集。
        let ui_scene_nodes = build_ui_scene_nodes(&scene, !run_script);
        let mut scene = Self {
            script,
            script_micros: 0,
            sprite_batch: SpriteBatch::new(),
            sprite_parallelism: Parallelism::detected(),
            system_parallelism: Parallelism::detected(),
            spatial_index: false,
            spatial_grid: None,
            spatial_index_fingerprint: None,
            spatial_index_cell_size: 256.0,
            collider_debug_draw: false,
            collider_debug_draw_3d: false,
            view_world_mask: crate::core::visibility::ALL_LAYERS,
            view_overlay_mask: crate::core::visibility::ALL_LAYERS,
            view_filtered: std::cell::Cell::new(0),
            tilemap_culling: true,
            sprite_culling: true,
            camera,
            frame_view_3d: None,
            adopted_camera_2d_source: None,
            viewport_width: 1280,
            viewport_height: 720,
            ui_reference: UI_REFERENCE_SIZE,
            render_ui_overlay: true,
            project_root: project_root.map(|root| root.to_path_buf()),
            texture_dims: HashMap::new(),
            texture_paths: Vec::new(),
            texture_lookup: HashMap::new(),
            assets: std::cell::RefCell::new(AssetStore::new()),
            texture_handles: Vec::new(),
            overlay_batch: SpriteBatch::with_capacity(16),
            screen_batch: SpriteBatch::with_capacity(16),
            render_mode_routed: (0, 0, 0),
            tilemap_cache: HashMap::new(),
            font_atlas: None,
            vector_atlas: None,
            custom_vector_atlases: Arc::new(Mutex::new(HashMap::new())),
            // 调试插桩：首次 vector text layout 日志状态。
            last_font_version: u64::MAX,
            custom_font_atlases: Arc::new(Mutex::new(HashMap::new())),
            custom_font_versions: HashMap::new(),
            frame_font_keys: Vec::new(),
            missing_fonts: HashSet::new(),
            missing_vector_fonts: HashSet::new(),
            text_render_mode,
            ui_scene_nodes,
            ui_layout_rects: BTreeMap::new(),
            step_scheduler: Scheduler::new(),
            frame_scheduler: Scheduler::new(),
            step_system_names: Vec::new(),
            frame_system_names: Vec::new(),
            pending_step_input: None,
            last_schedule: None,
            gamepad: GamepadPoller::platform_default(),
            // WP2 裁定：`parent` 一律按局部空间解释。默认打开；没有 `Parent` 的场景
            // （现有全部示例场景）不产生任何传播成本，回滚档是 `set_spatial_inheritance(false)`。
            spatial_inheritance: spatial_mode.inherits(),
            spatial_mode,
            spatial_fingerprint: None,
            spatial_skip_unchanged: true,
            spatial_skipped_frames: 0,
            spatial_incremental: crate::core::hierarchy::SpatialIncremental::default(),
            spatial_incremental_enabled: true,
            last_spatial_path: crate::core::hierarchy::SpatialPath::Full,
            last_spatial_recomputed: 0,
            spatial_path_counts: [0; 3],
            spatial_recomputed_total: 0,
            last_spatial_report: SpatialReport::default(),
            spatial_active: false,
            world_sprite_depth: false,
            physics_authority: PhysicsAuthority::default(),
            physics_stage_order: PhysicsStageOrder::default(),
            last_scene_emission: 0,
            last_physics_sync: (0, 0),
            last_entity_script_calls: 0,
        };
        scene.register_builtin_frame_systems();
        Ok(scene)
    }

    /// Registers the built-in fixed-stage systems of the runtime frame
    /// pipeline (M18-03).
    ///
    /// The per-step systems preserve the legacy manual orchestration order:
    /// input apply → Lua `update()` → UI click dispatch → 2D/3D physics →
    /// particles → game clock → pending scene transition. The frame systems
    /// run once per `run_script_steps` call: velocity integration (Simulation
    /// stage, after the per-step scene transition) then animation state
    /// machines, matching the legacy post-loop block.
    fn register_builtin_frame_systems(&mut self) {
        self.add_builtin_step_system(System::new(
            "input_apply",
            ScheduleStage::Input,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, _| {
                if let Some(input) = scene.pending_step_input.clone() {
                    scene.script.set_input(input);
                }
                Ok(())
            },
        ));
        self.add_builtin_step_system(System::new(
            "lua_update",
            ScheduleStage::Script,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, _| {
                let result = scene
                    .script
                    .call_update()
                    .map_err(|error| SystemError::new(error.to_string()))?;
                scene.script_micros = scene
                    .script_micros
                    .saturating_add(scene.script.last_update_micros());
                tracing::debug!(target: "game", "[simulation] fixed update returned: {result}");
                Ok(())
            },
        ));
        self.add_builtin_step_system(System::new(
            "ui_click_dispatch",
            ScheduleStage::Script,
            ["lua_update"],
            |scene: &mut RuntimeScene, _| {
                let Some(input) = scene.pending_step_input.clone() else {
                    return Err(SystemError::new("pending step input is missing"));
                };
                let targets = scene.compute_click_targets();
                scene
                    .script
                    .process_ui_clicks(&input, &targets)
                    .map_err(|error| SystemError::new(error.to_string()))
            },
        ));
        // docs/54 §4.1：每实体脚本在**工程级脚本之后**跑（`lua_update` 先跑，它会做全局的输入/
        // 相机等准备），按实体 id 升序调用，因此"谁先动"是确定的。挂在 `Script` 格而不是新开一个
        // 阶段：它与工程级脚本共享同一套脚本侧状态（世界、输入、逻辑时钟）。
        self.add_builtin_step_system(System::new(
            "entity_scripts",
            ScheduleStage::Script,
            ["ui_click_dispatch"],
            |scene: &mut RuntimeScene, delta_seconds: f32| {
                let called = scene
                    .script
                    .call_entity_scripts(delta_seconds)
                    .map_err(|error| SystemError::new(error.to_string()))?;
                scene.last_entity_script_calls = called;
                Ok(())
            },
        ));
        self.add_builtin_step_system(System::new(
            "physics_sync",
            ScheduleStage::Sync,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, _| {
                // WP7：authority = Script 时，物理步之前把 ECS 的权威位置推给模拟世界。
                // **2D 与 3D 都推**（此前只推 2D，"3D 脚本权威"实际上不存在）。
                // `Legacy` 阶段顺序是回滚档：那时没有统一 Sync 格，脚本权威也不做步前同步。
                if scene.physics_authority.syncs_before_step()
                    && scene.physics_stage_order == PhysicsStageOrder::Unified
                {
                    let synced_2d = scene.script.sync_kinematic_bodies();
                    let synced_3d = scene.script.sync_kinematic_bodies_3d();
                    scene.last_physics_sync = (synced_2d, synced_3d);
                } else {
                    scene.last_physics_sync = (0, 0);
                }
                Ok(())
            },
        ));
        self.add_builtin_step_system(System::new(
            "physics2d_step",
            ScheduleStage::Physics,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, delta_seconds: f32| {
                scene.script.step_physics(delta_seconds);
                // 回滚档：收敛前 2D 是"步进即回写"，且它的回写夹在 3D 推进之前。
                if scene.physics_stage_order == PhysicsStageOrder::Legacy {
                    scene.script.writeback_physics();
                }
                Ok(())
            },
        ));
        self.add_builtin_step_system(System::new(
            "physics3d_step",
            ScheduleStage::Physics,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, delta_seconds: f32| {
                scene.script.step_physics3d(delta_seconds);
                Ok(())
            },
        ));
        self.add_builtin_step_system(System::new(
            "physics_writeback",
            ScheduleStage::Writeback,
            ["physics2d_step", "physics3d_step"],
            |scene: &mut RuntimeScene, _| {
                // WP7：2D 与 3D 的**所有**推进都发生在**任何**回写之前——此前 2D 的回写夹在
                // 3D 推进之前。authority = Script 时跳过回写（ECS 才是权威）。
                //
                // `Legacy` 阶段顺序是**真回滚档**：那时 2D 已在 `physics2d_step` 里"步进即回写"，
                // 这里只处理 3D（顺序与收敛前一致：2D 回写 → 3D 推进 → 3D 回写）。
                if scene.physics_stage_order == PhysicsStageOrder::Legacy {
                    if scene.physics_authority.writes_back_to_ecs() {
                        scene.script.writeback_physics3d();
                    } else {
                        scene.script.discard_pending_writeback();
                    }
                    return Ok(());
                }
                if scene.physics_authority.writes_back_to_ecs() {
                    scene.script.writeback_physics();
                    scene.script.writeback_physics3d();
                } else {
                    // 不回写也要把暂存清掉，避免它在下一步被误当成"本步的移动"。
                    scene.script.discard_pending_writeback();
                }
                Ok(())
            },
        ));
        self.add_builtin_step_system(System::new(
            "scene_emitters_step",
            ScheduleStage::Simulation,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, delta_seconds: f32| {
                // docs/54 §8-①：场景声明的发射器先发射，再统一推进粒子（见 particles_step 的依赖）。
                scene.last_scene_emission = scene.step_scene_emitters(delta_seconds);
                Ok(())
            },
        ));
        self.add_builtin_step_system(System::new(
            "particles_step",
            ScheduleStage::Simulation,
            ["scene_emitters_step"],
            |scene: &mut RuntimeScene, delta_seconds: f32| {
                scene.script.step_particles(delta_seconds);
                Ok(())
            },
        ));
        self.add_builtin_step_system(System::new(
            "clock_step",
            ScheduleStage::Simulation,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, delta_seconds: f32| {
                scene.script.step_clock(delta_seconds);
                Ok(())
            },
        ));
        self.add_builtin_step_system(System::new(
            "scene_transition",
            ScheduleStage::Simulation,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, _| {
                // Apply a pending scene.load so level switches happen between
                // steps.
                scene
                    .script
                    .process_scene_transition()
                    .map_err(|error| SystemError::new(error.to_string()))
            },
        ));
        // 换图会把世界连同场景声明的 `Camera` 一起重建，宿主必须**紧跟其后**重新读一次
        // bounds（否则"声明的相机"只在启动场景生效）。挂在同一格、排在 scene_transition
        // 之后，因此新 bounds 在同一个固定步里就被 `step_camera2d` 夹取，渲染不会看到
        // 越界的一帧。
        self.add_builtin_step_system(System::new(
            "scene_camera_sync",
            ScheduleStage::Simulation,
            ["scene_transition"],
            |scene: &mut RuntimeScene, _| {
                scene.sync_camera_from_scene();
                Ok(())
            },
        ));
        // 以下为每次 run_script_steps 调用只执行一次的帧级系统：原实现里它们
        // 在固定步循环结束后统一推进（elapsed = delta × steps），顺序保持
        // integrate_velocity → AnimationStateMachine → Animation3D。
        self.add_builtin_frame_system(System::new(
            "integrate_velocity",
            ScheduleStage::Simulation,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, delta_seconds: f32| {
                scene.script.with_world_mut(|world| {
                    let _ = integrate_velocity(world, delta_seconds);
                });
                Ok(())
            },
        ));
        self.add_builtin_frame_system(System::new(
            "camera2d_controller",
            ScheduleStage::Simulation,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, delta_seconds: f32| {
                scene.step_camera2d(delta_seconds);
                Ok(())
            },
        ));
        self.add_builtin_frame_system(System::new(
            "spatial_propagate",
            ScheduleStage::Spatial,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, _| {
                // WP7：`GlobalTransform` 传播是流水线上的显式一格（回写之后、提取之前）。
                scene.propagate_spatial_now();
                Ok(())
            },
        ));
        self.add_builtin_frame_system(System::new(
            "animation_state_advance",
            ScheduleStage::Animation,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, delta_seconds: f32| {
                // WP4 ⑦：推进的同时把落在本步的**帧事件**收集起来交给 Lua
                // （`animation.events()` 轮询，和 `physics.events()` 同一套用法）。
                // 借用顺序：先在 world 借用内收集，借用结束后再写进 Lua 运行时。
                let fired = scene.script.with_world_mut(|world| {
                    let mut fired: Vec<(EntityId, AnimationFrameEvent)> = Vec::new();
                    world.for_each_mut::<AnimationStateMachine>(|id, machine| {
                        machine.advance(delta_seconds);
                        for event in machine.take_frame_events() {
                            fired.push((id, event));
                        }
                    });
                    fired
                });
                scene.script.push_animation_events(fired);
                // docs/54 §4.4：动画片段也进运行时身份表。键是片段的**内容签名**（贴图 + 帧数 +
                // 帧率 + 循环 + 事件），因此同一段片段在进程内恒同句柄——"这份动画是哪一个"跨帧可比。
                //
                // 这里**就地**先查后插（而不是先收集 Vec<String>）：每帧每实体只有一次哈希查找，
                // 且只在**首次**登记时才分配字符串。收集一轮键的做法本身就会给每帧每实体各来一次
                // String 分配——那正是本批要修掉的那类开销。
                {
                    let mut assets = scene.assets.borrow_mut();
                    scene.script.with_world(|world| {
                        world.for_each::<AnimationStateMachine>(|_, machine| {
                            let key = machine.clip_resource_key();
                            if assets.handle_of(key).is_none() {
                                assets.intern(
                                    key,
                                    AssetRecord {
                                        path: key.to_owned(),
                                        kind: crate::core::resource_kind::ResourceKind::Animation,
                                    },
                                );
                            }
                        });
                    });
                }
                Ok(())
            },
        ));
        self.add_builtin_frame_system(System::new(
            "animation3d_advance",
            ScheduleStage::Animation,
            [] as [&str; 0],
            |scene: &mut RuntimeScene, delta_seconds: f32| {
                scene.script.with_world_mut(|world| {
                    world.for_each_mut::<Animation3D>(|_, animation| {
                        animation.advance(delta_seconds)
                    })
                });
                // docs/54 §4.4：3D 骨骼动画的片段身份 = `mesh + clip`（路径在实体的 `MeshRenderer`
                // 上，`Animation3D` 自己不存）。这里**每帧**组合一次键：骨骼实体数量少（一帧几个到
                // 几十个），而 `Animation3D` 不知道网格路径、没有可缓存的完整键——这条与 2D 热路径
                // （成千上万精灵）刻意不同，2D 那边是缓存键 + 就地查表、零分配。
                {
                    let mut assets = scene.assets.borrow_mut();
                    scene.script.with_world(|world| {
                        world.for_each2::<MeshRenderer, Animation3D>(|_, renderer, animation| {
                            let key = animation.resource_key(&renderer.mesh);
                            if assets.handle_of(&key).is_none() {
                                assets.intern(
                                    key.clone(),
                                    AssetRecord {
                                        path: key,
                                        kind: crate::core::resource_kind::ResourceKind::Animation,
                                    },
                                );
                            }
                        });
                    });
                }
                Ok(())
            },
        ));
    }

    /// 换图后同步 2D 相机：重新读新场景声明的 `Camera`，并把 **bounds 换成新地图的**。
    ///
    /// 为什么需要它：`process_scene_transition` 会**重建整个世界**（`ScriptState::world =
    /// World::new()` 后重新实例化），于是切图时场景声明的相机是**新的一份** `Camera2D`；
    /// 而宿主只在 `init`（启动场景）时读过一次 `declared_camera`。结果是"声明的相机只在
    /// 启动场景生效"——每张玩法地图各声明一份 bounds 的工程（`examples/game_jhyt`）里，
    /// 只有开工那张图受约束，之后所有地图的相机都退回"无 bounds"。
    ///
    /// 只取 `bounds`，**不取 center/zoom**：换图时相机姿态的权威是每帧的 `camera2d.*`
    /// 请求，把它重置回新场景的作者中心会在换图那一帧闪一下（与"场景声明的相机是**初始**
    /// 相机，不是第二份姿态真源"这条既有裁定一致）。新 bounds 在同一个固定步里就会被
    /// `step_camera2d` 夹取，因此不会渲染出越界的一帧。
    fn sync_camera_from_scene(&mut self) {
        // 拆分批 9：同步的是**当前生效**那台的 bounds（显式选中优先，否则声明的第一台）——
        // 脚本显式选了另一台 2D 相机时，换图不该把它的 bounds 换成"第一台"的。
        let Some(declared) = self.effective_camera_2d_payload() else {
            return;
        };
        if self.camera.bounds != declared.bounds {
            tracing::info!(
                target: "render",
                scene = ?self.script.scene_name(),
                bounds = ?declared.bounds,
                "[render] 2D camera bounds refreshed from the new scene"
            );
        }
        self.camera.bounds = declared.bounds;
    }

    /// 拆分批 9：**本帧生效**的 2D 相机载荷（显式选中优先，否则排序序第一台启用的正交相机）。
    ///
    /// 与 `active_camera3d` 是同一条规则的两个投影分支：同一个 `CameraSelection`、同样的
    /// `enabled` 回退与告警。区别只在"谁拥有运动"——2D 的 center/zoom 之后由
    /// `Camera2dController`（跟随/死区/平滑/抖动/脚本请求）继续决定。
    fn effective_camera_2d_payload(&self) -> Option<Camera2D> {
        let selection = CameraSelection {
            active: self.script.active_camera().map(|id| id.index() as u64),
        };
        self.script.with_world(|world| {
            selected_camera_2d_in(world, selection)
                .map(|(_, payload)| payload)
                // 没有选中 2D 相机（未选择 / 选中的是透视相机）时，换图同步的 bounds 取自
                // 场景声明的第一台启用正交相机 —— 与 `init` 的初始种子同一口径。
                .or_else(|| declared_camera_2d_in(world))
        })
    }

    /// 每帧推进 2D 相机控制器：解析跟随目标的世界位置 → 控制器算 center → 复用瓦片图
    /// 那套 `constrained_camera_center` 做 bounds 钳制（避免出现两套夹取规则）。
    ///
    /// 不跟随时控制器原样返回当前中心，因此 `camera2d.set` 的效果不会被覆盖。
    fn step_camera2d(&mut self, delta_seconds: f32) {
        // 拆分批 9：`camera.set_active` 是**一条**选择链——它选中的正交相机就是 2D 相机的来源。
        // 选中项**变化**时采用一次它的**载荷**（center/zoom/rotation/bounds）；同一台不再重复采用。
        //
        // 为什么"变化时才采用"而不是"每帧取生效载荷"：2D 的 center/zoom 之后归控制器（跟随/死区/
        // 平滑/抖动/请求），每帧重读作者写的初始值会把控制器算出的取景逐帧抹平——`camera2d.follow`
        // 与 `shake` 会静默失效。为什么"选中的是透视相机时不重置"：切换 3D 相机是常见操作，
        // 它绝不该把 2D 取景弹回作者值（见 `selected_camera_2d_in` 的语义说明）。
        let selection = CameraSelection {
            active: self.script.active_camera().map(|id| id.index() as u64),
        };
        let source = self
            .script
            .with_world(|world| selected_camera_2d_in(world, selection));
        match source {
            Some((id, payload)) if Some(id) != self.adopted_camera_2d_source => {
                self.adopted_camera_2d_source = Some(id);
                tracing::info!(
                    target: "render",
                    center = ?payload.center,
                    zoom = payload.zoom,
                    "[render] 2D camera adopted from the entity selected by camera.set_active"
                );
                self.camera = payload;
            }
            Some(_) => {} // 同一台：不重复采用（否则每帧把控制器的取景抹平）
            None => self.adopted_camera_2d_source = None, // 选择离开 2D 相机 ⇒ 之后再选中会重新采用
        }
        // WP5：请求消费、跟随、advance、bounds 钳制的**次序**现在固定在
        // `Camera2dController::resolve_camera` 内部，宿主不再自己拼这套顺序，也不会出现
        // "宿主与控制器各夹一次 bounds"的分叉。
        let follow = self
            .script
            .camera2d_controller(|controller| controller.follow_target());
        let desired = follow.and_then(|entity| {
            self.script.with_world(|world| {
                // WP2：跟随目标跟的是**世界**位置（父链下的子实体会跟着父一起动）。
                crate::core::hierarchy::world_transform_2d(world, entity)
                    .ok()
                    .flatten()
                    .map(|transform| transform.position)
            })
        });
        let viewport = Viewport::new(
            self.viewport_width.max(1) as f32,
            self.viewport_height.max(1) as f32,
        )
        .unwrap_or_else(|_| Viewport::new(1.0, 1.0).expect("1x1 viewport is valid"));
        let current = self.camera;
        self.camera = self.script.camera2d_controller_mut(|controller| {
            controller.resolve_camera(current, desired, viewport, delta_seconds)
        });
        // 回填真实状态：`camera2d.get` 必须读到宿主实际在用的值。
        let (center, zoom) = (self.camera.center, self.camera.zoom);
        self.script
            .camera2d_controller_mut(|controller| controller.report(center, zoom));
    }

    /// Advances the runtime by `steps` fixed steps through the shared frame
    /// schedule (M18-03).
    ///
    /// This is the single scheduling entry used by the standalone game shell,
    /// the editor Play preview and headless runners. Every fixed step runs the
    /// per-step pipeline (input → Lua update → UI clicks → physics →
    /// particles → clock → scene transition); after the loop the frame
    /// pipeline runs once (velocity integration → animation advance) with the
    /// accumulated elapsed time, matching the legacy manual orchestration.
    ///
    /// Registered systems are validated by the [`Scheduler`]: missing
    /// dependencies, dependency cycles and system panics surface as errors
    /// naming the offending system and stage. Per-system timings are merged
    /// into [`Self::last_schedule_report`].
    pub fn run_script_steps(
        &mut self,
        steps: u32,
        delta_seconds: f32,
        input: &InputSnapshot,
    ) -> anyhow::Result<()> {
        self.script.set_delta(delta_seconds);
        // M20-06：把真实手柄设备状态合并进本帧输入（只改按钮/轴，不影响键鼠）。
        // 无设备时 XInput 返回未连接，快照与传入值等价，headless 行为不变。
        // M22/W3：轴判定阈值与 `[input] nav_threshold` 同源（Lua `input.set_nav_settings`
        // / `load_settings` 写入 `ScriptState`），每帧同步给设备层，轴的数字边沿因此
        // 与设置文件里的手感一致。
        //
        // M22/W4：**只在真有固定步的帧采样设备**。0 步帧（帧率高于逻辑步，关 vsync 时常见）
        // 里键鼠边沿会被宿主保留到下一有效帧（`game_app.rs` 只在 `steps > 0` 时
        // `finish_frame`），若此时仍 poll 手柄，边沿会写进这个随后被丢弃的 clone 而**永久
        // 丢失**（`poller.last` 已推进，下一帧不会再产生同一变化）。跳过采样后两条路径语义一致。
        let mut step_input = input.clone();
        if steps > 0 {
            self.gamepad
                .set_axis_threshold(self.script.nav_settings().threshold);
            self.gamepad.poll_into(&mut step_input);
        }
        // 调度器实例跨帧保留：借出 → 运行 → 放回。系统本体一直归调度器所有，
        // 因此这里不存在"注册失败丢系统"的路径（此前用 `drain` + `break`，
        // 提前退出会让尚未搬迁的系统随 Drain 一起被丢弃）。
        let mut step_scheduler = std::mem::take(&mut self.step_scheduler);
        let mut frame_scheduler = std::mem::take(&mut self.frame_scheduler);
        let outcome = self.run_pipelines(
            &mut step_scheduler,
            &mut frame_scheduler,
            steps,
            delta_seconds,
            &mut step_input,
        );
        self.step_scheduler = step_scheduler;
        self.frame_scheduler = frame_scheduler;
        // M22/W2：把本帧的震动请求交给设备层（Lua `input.rumble` 只登记请求；到期归零由
        // `GamepadPoller` 的状态机负责）。失败路径不取走请求也没问题——下一帧会覆盖。
        if outcome.is_ok()
            && let Some((low, high, ms)) = self.script.take_rumble_request()
        {
            self.gamepad.rumble(low, high, ms);
        }
        self.pending_step_input = None;
        // WP2：本帧脚本/物理都改完局部变换后，统一传播一次世界变换——渲染与 Lua 的世界位置
        // 因此都来自同一次传播（3D 提取是 `&self`，不能自己触发传播，所以必须在这里做）。
        self.propagate_spatial_now();
        // 拆分批 8：这一步可能改过世界（脚本换选中相机、移动父链、换场景），因此在这之前定下的
        // "本帧 3D 视图"不再可信——作废，让下一次消费重新定帧。放在这里（而不是开头）是为了让
        // "先定帧后步进"的宿主顺序也不会拿到步进前的相机。
        self.invalidate_frame_view_3d();
        // BGM 曲尾续播：rodio 0.19 的 repeat_infinite 源会被 sink 判定为已播完（实测起播
        // ~3s 即 empty → 全程无声），因此循环改为"单次 append + 这里每帧补挂"。
        // 失败帧不补（脚本可能整体异常，交给下一帧）。
        if outcome.is_ok() {
            self.script.maintain_music();
        }
        outcome
    }

    /// 暂停 BGM（编辑器暂停时调用；不释放 sink）。
    pub fn pause_music(&self) {
        self.script.pause_music();
    }

    /// 恢复 BGM。
    pub fn resume_music(&self) {
        self.script.resume_music();
    }

    /// Executes the per-step pipeline `steps` times and the frame pipeline
    /// once, accumulating schedule timings into `last_schedule`.
    fn run_pipelines(
        &mut self,
        step_scheduler: &mut Scheduler<RuntimeScene>,
        frame_scheduler: &mut Scheduler<RuntimeScene>,
        steps: u32,
        delta_seconds: f32,
        step_input: &mut InputSnapshot,
    ) -> anyhow::Result<()> {
        let mut report = FrameScheduleReport {
            steps,
            ..FrameScheduleReport::default()
        };
        for step in 0..steps {
            self.pending_step_input = Some(step_input.clone());
            let step_report = step_scheduler.run(self, delta_seconds)?;
            report.merge(&step_report);
            // One-shot edges are consumed by the first fixed step of a frame;
            // clear them so catch-up steps in the same frame don't re-fire
            // just_pressed actions (e.g. F4 switching scenes twice).
            if step == 0 {
                step_input.finish_frame();
            }
        }
        let elapsed = delta_seconds * steps as f32;
        let frame_report = frame_scheduler.run(self, elapsed)?;
        report.merge(&frame_report);
        self.last_schedule = Some(report);
        if tracing::enabled!(tracing::Level::DEBUG)
            && let Some(report) = &self.last_schedule
        {
            let systems = report
                .timings
                .iter()
                .map(|timing| {
                    format!(
                        "{}={:.3}ms",
                        timing.name,
                        timing.elapsed.as_secs_f64() * 1000.0
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            tracing::debug!(target: "scene", "[schedule] steps={} total={:.3}ms systems=[{}]", report.steps, report.total_elapsed.as_secs_f64() * 1000.0, systems);
        }
        Ok(())
    }

    /// Returns the aggregated schedule statistics of the most recent
    /// [`Self::run_script_steps`] call, if any (M18-03 性能统计出口).
    #[must_use]
    pub fn last_schedule_report(&self) -> Option<&FrameScheduleReport> {
        self.last_schedule.as_ref()
    }

    /// 替换手柄后端（M20-06）：headless 验收用 [`crate::host::gamepad::FakeBackend`] 脚本化设备事件，
    /// 正式运行时用平台默认（Windows XInput）。
    pub fn set_gamepad_backend(&mut self, backend: Box<dyn GamepadBackend>) {
        self.gamepad = GamepadPoller::new(backend);
    }

    /// 应用玩家配置的摇杆/扳机死区（M20-08，来自 `settings.toml` 的 `[input]` 段）。
    ///
    /// 实现上是**重建平台默认后端**，因此：
    /// * 非 Windows 平台无设备后端，调用为 no-op（静默降级语义不变）；
    /// * 会覆盖此前 `set_gamepad_backend` 注入的后端——宿主启动顺序应是
    ///   `RuntimeScene::new` → `set_gamepad_deadzone`（headless 再注入 Fake）。
    pub fn set_gamepad_deadzone(&mut self, deadzone: GamepadDeadzone) {
        self.gamepad = GamepadPoller::platform_default_with_deadzone(deadzone);
    }

    /// 当前是否检测到手柄设备（M20-06 诊断/UI 可用）。
    #[must_use]
    pub fn gamepad_connected(&self) -> bool {
        self.gamepad.connected()
    }

    /// 手柄后端名称（诊断）。
    #[must_use]
    pub fn gamepad_backend_name(&self) -> &'static str {
        self.gamepad.backend_name()
    }

    /// Grants temporary access to the underlying Lua runtime — the escape
    /// hatch for registered frame systems (and tests) that need to drive
    /// script APIs from a `&mut RuntimeScene` context.
    pub fn with_script<R>(&mut self, visit: impl FnOnce(&mut LuaRuntime) -> R) -> R {
        visit(&mut self.script)
    }

    /// docs/54 §4.1：热重载**脚本层的全部内容**——工程级脚本 + 每实体脚本。
    ///
    /// 宿主此前只调 `reload_if_changed()`，那只会看到 `scripts/main.lua`；实体脚本改了却"状态
    /// 未变"会让"热重载不生效"变成一个没有线索的问题。这个入口一次返回两件事：
    /// 工程级脚本的状态与**被重载的实体脚本文件数**。
    pub fn reload_scripts(&mut self) -> anyhow::Result<(ReloadStatus, usize)> {
        let status = self.script.reload_if_changed()?;
        let entity_scripts = self.script.reload_entity_scripts()?;
        Ok((status, entity_scripts))
    }

    /// 注册内建 step 系统。名字是静态唯一的；重复只可能是编程错误，而这里的
    /// 失败路径**不静默吞掉**：记 error 日志，并由 `registered_system_names`
    /// 相关测试与调度器自身的重复检测兜住。真正面向调用方的
    /// [`Self::add_step_system`] 会把错误返回给调用者。
    fn add_builtin_step_system(&mut self, system: System<RuntimeScene>) {
        let name = system.name().to_owned();
        if let Err(error) = self.step_scheduler.add_system(system) {
            tracing::error!(
                target: "engine",
                "[runtime] builtin step system `{name}` registration failed: {error}"
            );
            return;
        }
        self.step_system_names.push(name);
    }

    /// 注册内建 frame 系统（语义同 [`Self::add_builtin_step_system`]）。
    fn add_builtin_frame_system(&mut self, system: System<RuntimeScene>) {
        let name = system.name().to_owned();
        if let Err(error) = self.frame_scheduler.add_system(system) {
            tracing::error!(
                target: "engine",
                "[runtime] builtin frame system `{name}` registration failed: {error}"
            );
            return;
        }
        self.frame_system_names.push(name);
    }

    /// Registers an extra system that runs on every fixed step of
    /// [`Self::run_script_steps`] (M18-03).
    ///
    /// The system joins the shared schedule: it declares a
    /// [`ScheduleStage`] and dependencies on other registered systems, and
    /// the scheduler validates the resulting order (cached until the
    /// registration changes). Duplicate names are rejected here; missing
    /// dependencies and cycles surface as locatable errors from
    /// [`Self::run_script_steps`].
    pub fn add_step_system(&mut self, system: System<RuntimeScene>) -> Result<(), SchedulerError> {
        let name = system.name().to_owned();
        self.ensure_unique_system_name(&name)?;
        self.step_scheduler.add_system(system)?;
        self.step_system_names.push(name);
        Ok(())
    }

    /// Registers an extra system that runs once per
    /// [`Self::run_script_steps`] call (M18-03). See [`Self::add_step_system`].
    pub fn add_frame_system(&mut self, system: System<RuntimeScene>) -> Result<(), SchedulerError> {
        let name = system.name().to_owned();
        self.ensure_unique_system_name(&name)?;
        self.frame_scheduler.add_system(system)?;
        self.frame_system_names.push(name);
        Ok(())
    }

    /// Returns the registered per-step and frame system names in execution
    /// (registration) order, for diagnostics and tests.
    #[must_use]
    pub fn registered_system_names(&self) -> (Vec<String>, Vec<String>) {
        (
            self.step_system_names.clone(),
            self.frame_system_names.clone(),
        )
    }

    fn ensure_unique_system_name(&self, name: &str) -> Result<(), SchedulerError> {
        if self
            .step_system_names
            .iter()
            .chain(self.frame_system_names.iter())
            .any(|registered| registered == name)
        {
            return Err(SchedulerError::DuplicateSystem {
                name: name.to_owned(),
            });
        }
        Ok(())
    }

    /// 计算当前所有 `UiEvents` 实体的命中目标（屏幕像素）。
    ///
    /// Image/Text 用 UI 解析（viewport=实际渲染尺寸、reference=设计分辨率），
    /// Sprite 用相机投影到屏幕；供 `process_ui_clicks` 点选分发。
    fn compute_click_targets(&mut self) -> Vec<UiClickTarget> {
        let viewport = Viewport {
            width: self.viewport_width.max(1) as f32,
            height: self.viewport_height.max(1) as f32,
        };
        let viewport_arr = [viewport.width, viewport.height];
        self.ensure_scene_ui_layout(viewport_arr);
        // 先提取 UiEvents 实体及其视觉组件（world 借用结束后再计算，避免借用冲突）。
        type UiEventComponents = (
            EntityId,
            Option<UiImage>,
            Option<UiText>,
            Option<(Sprite, Transform)>,
            Option<crate::core::ui_events::UiEvents>,
        );
        let entities: Vec<UiEventComponents> = self.script.with_world(|world| {
            let mut out = Vec::new();
            for id in world.entities_with::<crate::core::ui_events::UiEvents>() {
                let image = world.get::<UiImage>(id).ok().flatten().cloned();
                let text = world.get::<UiText>(id).ok().flatten().cloned();
                let sprite = world.get::<Sprite>(id).ok().flatten().cloned();
                let transform = world.get::<Transform>(id).ok().flatten().cloned();
                let events = world
                    .get::<crate::core::ui_events::UiEvents>(id)
                    .ok()
                    .flatten()
                    .cloned();
                out.push((id, image, text, sprite.zip(transform), events));
            }
            out
        });
        let camera = OrthoTerms::new(self.camera, viewport).ok();
        entities
            .into_iter()
            .filter_map(|(id, image, text, sprite, events)| {
                let name = self.script.entity_name(id)?;
                let rect: ([f32; 2], [f32; 2]) = if image.is_some() {
                    let layout_rect = self.scene_ui_layout_rect(id, viewport_arr)?;
                    (
                        [layout_rect.x, layout_rect.y],
                        [layout_rect.w, layout_rect.h],
                    )
                } else if let Some(text) = text {
                    let layout_rect = self.scene_ui_layout_rect(id, viewport_arr)?;
                    self.text_hit_rect(&layout_rect, &text)?
                } else if let Some((sprite, transform)) = sprite {
                    sprite_screen_rect(&sprite, &transform, camera?)?
                } else {
                    return None;
                };
                let functions =
                    events.map_or_else(Vec::new, |events| UiEvents::click_functions(&events));
                Some(UiClickTarget {
                    name,
                    origin: rect.0,
                    size: rect.1,
                    functions,
                })
            })
            .collect()
    }

    /// 文本 hit 矩形：与编辑器 UI 包围框同款——resolve 出网格原点，度量字形
    /// bbox，返回 `(glyph_origin, glyph_size)`（屏幕像素）。
    fn text_hit_rect(
        &mut self,
        layout_rect: &UiRect,
        text: &UiText,
    ) -> Option<([f32; 2], [f32; 2])> {
        let (_, glyph_bounds) = self.measure_scene_ui_text(text);
        let origin = [layout_rect.x, layout_rect.y];
        let glyph_origin = [origin[0] + glyph_bounds[0], origin[1] + glyph_bounds[1]];
        let glyph_size = [
            (glyph_bounds[2] - glyph_bounds[0]).max(0.0),
            (glyph_bounds[3] - glyph_bounds[1]).max(0.0),
        ];
        Some((glyph_origin, glyph_size))
    }

    /// Gathers the per-frame 3D render state from the runtime world: the active
    /// camera (Lua-selected, else the first `Camera3D` entity, else a default),
    /// the active light, and every entity with both a `Transform3D` and a
    /// `MeshRenderer`. Returns `None` when the scene has no 3D drawables.
    ///
    /// 视口是**唯一**的宽高比来源（相机拆分批 7）：这里不再有任何"把比例写回相机"的动作，
    /// 组件载荷与场景里写的一字不差。
    pub fn scene_3d(&self, viewport: Viewport) -> Option<Scene3d> {
        self.scene_3d_with_camera(viewport, None)
    }

    /// 带可选相机覆盖的 3D 提取（M11-04 / R-05）：编辑器 Scene 3D 视图与运行时
    /// 共用同一条 ECS drawable/component 提取路径（单一事实来源），编辑器传入
    /// 视图导航相机作为覆盖（不改场景数据），运行时传入 `None` 走
    /// `active_camera3d`（Lua 选择 / 首个 Camera3D / 默认）。
    ///
    /// 覆盖相机同样**不被改写**：视口比例只进 [`Scene3d::view`]（烘焙结果），不进相机载荷。
    pub fn scene_3d_with_camera(
        &self,
        viewport: Viewport,
        camera_override: Option<Camera3D>,
    ) -> Option<Scene3d> {
        // Runtime extraction intentionally reads ECS state so physics, scripts,
        // active camera/light selection, and animation palettes remain live.
        // docs/54 §4.2：3D 网格同样吃**世界视图**的可见遮罩。
        let view_mask = self.view_world_mask;
        let mut filtered = 0usize;
        // P1-②：LOD 距离用相机位置。优先级：编辑器覆盖相机 → 本帧已定帧相机。
        // 刻意不在这里兜底 `active_camera3d()`：那是一次解析（有账本断言钉住解析次数）；
        // 没有覆盖/定帧时 LOD 退基础级（`renderer.mesh`），行为与未启用 LOD 逐字节相同。
        let lod_camera_position: Option<Vec3> =
            camera_override.map(|camera| camera.position).or_else(|| {
                self.frame_view_3d
                    .as_ref()
                    .map(|frame| frame.camera.position)
            });
        let drawables = self.script.with_world(|world| {
            let mut drawables = Vec::new();
            // WP2：与 2D 精灵同一条约定——开关每帧选定一次遍历用的组件（开启继承时是
            // `WorldTransform3D`，此时每个有 `Transform3D` 的实体都已拿到世界变换），
            // 因此不必逐实体"先试世界再退回局部"而多付一次 `get`。
            let mut collect = |id: EntityId, model: Mat4, renderer: &MeshRenderer| {
                if !crate::core::visibility::entity_visible_in(world, id, view_mask) {
                    filtered += 1;
                    return;
                }
                let name = self
                    .script
                    .entity_name(id)
                    .unwrap_or_else(|| format!("Entity {}", id.index()));
                let palette = world
                    .get::<Animation3D>(id)
                    .ok()
                    .flatten()
                    .map(|animation| animation.joint_matrices())
                    .unwrap_or_default();
                let bounds = if renderer.mesh == crate::core::mesh3d::GROUND_PLANE_SENTINEL {
                    Scene3dBounds {
                        local_aabb: crate::core::math3d::Aabb3::new(
                            Vec3::new(-100.0, -0.05, -100.0),
                            Vec3::new(100.0, 0.05, 100.0),
                        )
                        .unwrap_or_else(|_| crate::core::math3d::Aabb3 {
                            min: Vec3::new(-1.0, -1.0, -1.0),
                            max: Vec3::new(1.0, 1.0, 1.0),
                        }),
                        source: BoundsSource::Fallback,
                    }
                } else {
                    Scene3dBounds {
                        local_aabb: crate::core::math3d::Aabb3 {
                            min: Vec3::new(-0.5, -0.5, -0.5),
                            max: Vec3::new(0.5, 0.5, 0.5),
                        },
                        source: BoundsSource::Fallback,
                    }
                };
                // P1-②：LOD（距离驱动网格选择）。`LodGroup` 的 `levels[0]` 是基础级；
                // `select(distance)` 取最后一个 threshold<=distance 的级别。没有 LodGroup
                // 或没有可用相机时退 `renderer.mesh`（原行为，像素逐字节不变）。
                let mesh_path = match (
                    lod_camera_position,
                    world.get::<crate::core::lod::LodGroup>(id).ok().flatten(),
                ) {
                    (Some(camera_pos), Some(group)) => {
                        let model_pos = Vec3::new(
                            model.columns[3][0],
                            model.columns[3][1],
                            model.columns[3][2],
                        );
                        let distance = (model_pos - camera_pos).length();
                        group.select(distance).to_owned()
                    }
                    _ => renderer.mesh.clone(),
                };
                // WP6：3D 提取不再每帧克隆 mesh 路径（数量级 ≤13 mesh，但同类分配问题
                // 一并消除）。先借用身份表做零分配的 `handle_of` 查找，未注册时才走
                // intern（首帧/换网格各一次）；`mesh_path` 直接 move 进 drawable。
                let mesh_handle = {
                    let assets = self.assets.borrow();
                    assets.handle_of(mesh_path.as_str())
                };
                let mesh_handle = Some(match mesh_handle {
                    Some(handle) => handle,
                    None => {
                        self.assets
                            .borrow_mut()
                            .intern(
                                mesh_path.as_str(),
                                AssetRecord {
                                    path: mesh_path.clone(),
                                    kind: crate::core::resource_kind::ResourceKind::Model,
                                },
                            )
                            .0
                    }
                });
                drawables.push(Scene3dDrawable {
                    entity: SceneEntityId(id.index() as u64),
                    name,
                    mesh_path,
                    // docs/54 §4.4：网格也走同一套运行时身份（`ResourceKind::Model`）。3D 提取是
                    // `&self`，因此身份表在 `RefCell` 里——同一网格跨帧恒同句柄，渲染器据此做
                    // O(1) 的网格缓存查找并认出失效引用。
                    mesh_handle,
                    model,
                    palette,
                    bounds,
                });
            };
            if self.spatial_active {
                world.for_each2::<GlobalTransform, MeshRenderer>(|id, global, renderer| {
                    // 直接用派生矩阵：它是 `Global(parent) * Local(child)` 的真值，因此剪切/镜像的
                    // 父子组合也能拿到正确的世界矩阵（旧路径此时只能退回局部值）。
                    collect(id, global.matrix(), renderer);
                });
            } else {
                world.for_each2::<Transform3D, MeshRenderer>(|id, transform, renderer| {
                    if let Ok(model) = transform.model_matrix() {
                        collect(id, model, renderer);
                    }
                });
            }
            drawables
        });
        self.view_filtered.set(self.view_filtered.get() + filtered);
        if drawables.is_empty() {
            return None;
        }
        // 编辑器视图覆盖相机时不触碰场景数据（不写 ECS、不改脚本状态），仅作为
        // 渲染视角覆盖；运行时传 None 则按 Lua 选择 / 首个 Camera3D / 默认解析。
        //
        // 拆分批 8：无覆盖时优先复用**本帧已定帧**的那一份——相机与视图都来自同一次解析/烘焙。
        // 视图只跟（相机 × 视口）有关，因此视口不同时**换烘焙、不换相机**：相机不依赖视口，
        // 再解析一次只会白白多一趟遍历（也让"同帧只解析一次"这条账本数字失真）。
        let (camera, view) = match (camera_override, self.frame_view_3d) {
            (Some(camera), _) => (
                camera,
                RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
            ),
            (None, Some(frame)) if frame.viewport == viewport => (frame.camera, frame.view),
            (None, Some(frame)) => (
                frame.camera,
                RenderView::main_3d(frame.camera, viewport, DEFAULT_3D_CLEAR),
            ),
            (None, None) => {
                let camera = self.active_camera3d();
                (
                    camera,
                    RenderView::main_3d(camera, viewport, DEFAULT_3D_CLEAR),
                )
            }
        };
        let light = self.active_light();
        Some(Scene3d {
            camera,
            // 拆分批 7：视图在提取处成帧，渲染器（含"宿主未声明 FrameViews"的历史回退）与
            // 宿主声明的主视图因此消费同一份视口派生矩阵。
            view,
            light,
            drawables,
        })
    }

    /// 本帧的 3D 视图烘焙结果（拆分批 8）：同帧复用定帧时的那一份（`vertices()` 定的），
    /// 视口不同则只重新烘焙（相机不依赖视口，不必再解析一次）。`None` = 相机退化（位置与目标
    /// 重合等），消费方据此走各自的历史回退。
    fn frame_camera_view(&self, viewport: Viewport) -> Option<CameraView> {
        if let Some(frame) = self.frame_view_3d {
            if frame.viewport == viewport {
                return frame.view.baked();
            }
            return CameraView::from_camera3d(frame.camera, viewport).ok();
        }
        CameraView::from_camera3d(self.active_camera3d(), viewport).ok()
    }

    /// 拆分批 8：世界或相机姿态可能变了 ⇒ 作废本帧定下的 3D 视图。
    ///
    /// 所有"宿主能在帧内调用、且可能改到**生效相机或其姿态**"的入口都必须走它（不变量是
    /// **帧内一切消费方看到同一台相机**；代价只是下一次消费多一趟 O(#相机) 的解析）。用一个
    /// 有名字的函数而不是撒一行 `= None`：漏掉一处就是一条静默的陈旧路径，而名字能把
    /// "这里发生了相机相关的世界变更"写在调用点上。
    fn invalidate_frame_view_3d(&mut self) {
        #[cfg(test)]
        crate::core::camera::ledger::count_frame_view_invalidations();
        self.frame_view_3d = None;
    }

    /// 测试用：本帧已定帧的 3D 视图（拆分批 8）。`None` = 未定帧或已被世界变更作废。
    #[cfg(test)]
    pub(crate) fn frame_view_3d(&self) -> Option<RenderView> {
        self.frame_view_3d.map(|frame| frame.view)
    }

    /// Resolves the active camera: Lua's `camera.set_active` selection, else the
    /// first entity with a `Camera3D` component, else a fixed default.
    ///
    /// **不改写相机**（拆分批 7）：返回的就是组件载荷本身。此前这里按传入的窗口比例覆写
    /// `aspect_ratio`，于是"组件里存的比例"与"渲染/拾取实际用的比例"成为两份状态。
    fn active_camera3d(&self) -> Camera3D {
        #[cfg(test)]
        crate::core::camera::ledger::count_camera3d_resolution();
        let fallback = Camera3D::perspective(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::ZERO,
            std::f32::consts::FRAC_PI_2,
            0.1,
            100.0,
        )
        .unwrap_or_else(|_| Camera3D {
            position: Vec3::new(0.0, 0.0, 5.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_y: std::f32::consts::FRAC_PI_2,
            near: 0.1,
            far: 100.0,
            pose_from_transform: false,
            enabled: true,
        });
        // WP5：多相机选择只有一条规则（显式选中优先，否则排序序第一个），由
        // `CameraSelection` 表述；宿主只在遍历候选时套用它，不再自己写一遍判断。
        // docs/54 §4.2：`enabled=false` 的相机不参与选择（既不当"第一个"，也不响应显式选中）。
        let selection = CameraSelection {
            active: self.script.active_camera().map(|id| id.index() as u64),
        };
        let camera = self.script.with_world(|world| {
            // 单趟遍历、**不建候选表**：显式选中优先，否则排序序第一个启用项。
            // （此前每调用一次就分配一个 `Vec<(u64, Camera3D)>`，而它每帧至少被调 2~3 次：
            // `scene_3d_with_camera`、`set_camera3d_view`、3D 调试线框。）
            let mut chosen: Option<Camera3D> = None;
            let mut fallback: Option<Camera3D> = None;
            let mut selected_disabled = false;
            world.for_each::<crate::core::camera::Camera>(|id, camera| {
                // 唯一组件、按投影分支取值：正交相机不属于 3D pass 的候选（既不参与
                // "第一个"回退，也不响应显式选中——与它此前根本不在 `Camera3D` 存储里一致）。
                let Some(component) = camera.as_perspective() else {
                    return;
                };
                if !component.enabled {
                    selected_disabled |= selection.is_selected(id.index() as u64);
                    return;
                }
                if chosen.is_some() {
                    // 已命中显式选中项，且回退项也已就位：剩下的候选不必再解析姿态。
                    return;
                }
                // WP5：声明了 `pose=transform` 的相机，姿态改由**实体的 Transform3D** 提供
                // （位置 + -Z 视线 + +Y 上方向），本组件的 position/target 退化为回退值。
                // 没有 Transform3D 时保持组件姿态（scene_extract 会为此报诊断）。
                let camera = if component.pose_from_transform {
                    // docs/54 §8-⑦：姿态取实体的**世界**变换（父链组合后的结果）。取局部值
                    // 会让挂在父下的相机看不清"它到底在看哪里"；这里用 lazy 版本，因此不依赖
                    // "调用方本帧已经跑过传播"这个隐含顺序。
                    match crate::core::hierarchy::world_transform_3d_lazy(world, id) {
                        Ok(Some(pose)) => component.with_pose(pose).unwrap_or(component),
                        _ => component,
                    }
                } else {
                    component
                };
                if selection.is_selected(id.index() as u64) {
                    chosen = Some(camera);
                } else if fallback.is_none() {
                    fallback = Some(camera);
                }
            });
            if selected_disabled {
                tracing::warn!(
                    target: "render",
                    "[render] `camera.set_active` 选中的相机 `enabled=false`，已回退到下一台可用相机"
                );
            }
            chosen.or(fallback)
        });
        camera.unwrap_or(fallback)
    }

    /// Resolves the active light: Lua's `light.set_active` selection, else the
    /// first entity with a `Light` component, else a default directional light.
    fn active_light(&self) -> Light {
        let selected = self.script.active_light();
        self.script
            .with_world(|world| {
                let mut selected_light = None;
                let mut fallback = None;
                world.for_each::<Light>(|id, light| {
                    if fallback.is_none() {
                        fallback = Some(*light);
                    }
                    if selected == Some(id) {
                        selected_light = Some(*light);
                    }
                });
                selected_light.or(fallback)
            })
            .unwrap_or_default()
    }

    pub fn vertices(&mut self, width: u32, height: u32) -> anyhow::Result<SceneMesh> {
        self.viewport_width = width.max(1);
        self.viewport_height = height.max(1);
        // 拆分批 8：新的一帧 ⇒ 上一帧定下的 3D 视图不再可用（下面按本帧视口重新定帧）。
        // 这里刻意**不**走 `invalidate_frame_view_3d()`：那是"世界变更把定帧值作废"的账，
        // 帧边界不是世界变更（账本里两者分开记，混在一起就看不出"到底是谁作废的"）。
        self.frame_view_3d = None;
        self.script.set_ui_viewport(width, height);
        // WP2：提取前先把局部变换传播成世界变换（开启空间继承时才有实际效果；关闭时这一步
        // 只会清掉遗留的世界变换组件，消费者读到的仍是局部值 = 旧行为）。
        self.propagate_spatial_now();
        let viewport = Viewport::new(width.max(1) as f32, height.max(1) as f32)?;
        // WP4 ⑧：把本帧渲染用的 2D 相机与视口回填给 Lua，供 `camera2d.screen_to_world`
        // 一类换算使用。帧首回填 ⇒ 脚本看到的是上一帧末相机控制器算出的、也就是玩家刚看到的
        // 那一帧的投影；脚本在首帧渲染前调用换算会报错而不是猜一个默认相机。
        self.script
            .set_camera_view(self.camera, [width.max(1) as f32, height.max(1) as f32]);
        // docs/54 §8-⑦：把本帧 **3D pass 实际用的那台相机**回填给 Lua，于是 `camera3d.ray`
        // 与编辑器 3D 点选走的是同一份投影数学（同一条 `Camera3D::ray_from_screen`）。
        // 拆分批 7：回填的是**组件载荷本身**——宽高比由 Lua 侧的视口给出，不再写回相机。
        // 拆分批 8：这里同时是**定帧点**——解析一次 `active_camera3d()`、烘焙一次视图，之后同帧的
        // `scene_3d` 与 3D 调试线框复用同一份，于是"渲染用的相机"与"回填给 Lua 的相机"由**构造**
        // 决定同源，而不是靠"两步之间没人改世界"的巧合。
        let camera3d = self.active_camera3d();
        self.script
            .set_camera3d_view(camera3d, [width.max(1) as f32, height.max(1) as f32]);
        self.frame_view_3d = Some(FrameView3d {
            camera: camera3d,
            viewport,
            view: RenderView::main_3d(camera3d, viewport, DEFAULT_3D_CLEAR),
        });
        self.sprite_batch.clear();
        // WP6：收集阶段的可见域。开着时 `SpriteBatch::push` 直接丢掉不可能落在视口内的精灵，
        // 省下每项 80 字节的写入与后续排序——5 万精灵档实测这部分占掉整帧的大头（docs/53 §3.9）。
        // 世界矩形只看相机，与 `sprite_culling` 同一个开关：关掉即全收，像素不变。
        //
        // docs/54 §8-④：**世界通道的可见域是宿主声明的世界视图，不是 2D 相机**。2D 相机活在
        // 设计分辨率像素空间（默认中心 (640,360)），而 3D 世界视图里 `(-40, 0)` 之类坐标清晰
        // 可见；场景里有 3D 内容时宿主会声明 3D 主视图，此时按 2D 相机剔除会在**收集阶段**就把
        // 世界内容丢掉（`wp0_mixed_golden` 抓到的真实误丢）。因此有 3D 内容时世界通道全收：
        // 保守不剔除，绝不误丢。纯 2D 画面仍按 2D 视口裁剪，5 万精灵档的收益不变。
        let world_view_is_3d = self.world_sprite_depth && self.world_view_has_3d_content();
        let cull_bounds = if self.sprite_culling && !world_view_is_3d {
            Some(self.camera.visible_world_bounds(viewport)?)
        } else {
            None
        };
        self.sprite_batch.set_cull_bounds(cull_bounds);
        self.texture_paths.clear();
        // docs/54 §4.4：本帧的路径与去重表重建，但**身份表不清空**——句柄因此跨帧稳定。
        self.texture_lookup.clear();
        self.texture_handles.clear();
        // docs/54 §4.3：显式模式的批次每帧重建；没有实体声明模式时它们恒为空（零成本）。
        self.overlay_batch.clear();
        self.screen_batch.clear();
        self.overlay_batch.set_cull_bounds(cull_bounds);
        self.screen_batch.set_cull_bounds(cull_bounds);
        self.render_mode_routed = (0, 0, 0);
        // 每帧先清空：布局阶段（push_scene_ui 等）再把本帧用到的自定义字体 key 记入。
        self.frame_font_keys.clear();
        // Tile maps draw first (layer 0), sprite entities on top (layer 1+).
        //
        // WP6 顶点单趟化：瓦片与精灵的顶点**直接以 NDC 追加进同一个最终缓冲**，不再先各自
        // 物化一份 `SpriteVertex`（10k 精灵实测 1.9 MB）再由宿主逐顶点转换。draw range 由
        // 各段按当前缓冲长度自行偏移，因此两段可以连续追加而无需事后再合并。
        let mut vertices: Vec<OverlayVertex> = Vec::new();
        let mut draw_ranges: Vec<SpriteDrawBatch> = Vec::new();
        // WP6：世界 Sprite 深度通道的输出（世界坐标 + z）。关闭时全程为空，渲染器不执行该
        // 通道；打开时精灵与瓦片层都往这里写。
        let mut world_sprites: Vec<WorldSpriteVertex> = Vec::new();
        let mut world_sprite_draws: Vec<SpriteDrawBatch> = Vec::new();
        self.append_tilemap_vertices(
            &mut vertices,
            &mut world_sprites,
            &mut draw_ranges,
            &mut world_sprite_draws,
            viewport,
        )?;
        // WP6：精灵直接写进批处理，不再每帧先造一份
        // `(Transform, Sprite, Option<AnimationStateMachine>, Option<String>, Option<SpriteSorting>)`
        // 中间表——那份表每帧要搬上兆字节，还要为每个精灵克隆资源路径与排序层字符串。
        // 纹理注册顺序与旧路径完全一致（同一遍历顺序、同一注册点），因此纹理下标不变。
        {
            let RuntimeScene {
                script,
                sprite_batch,
                overlay_batch,
                screen_batch,
                texture_paths,
                texture_lookup,
                assets,
                texture_handles,
                ..
            } = self;
            let mut failure: Option<anyhow::Error> = None;
            let inherit = self.spatial_active;
            // docs/54 §4.2：世界视图的可见遮罩。被拦掉的精灵**不占序号、不产顶点**——
            // "不在这个视图里"不是"透明"。
            let view_mask = self.view_world_mask;
            let mut filtered = 0usize;
            let mut routed = (0usize, 0usize, 0usize);
            // P1-①：渲染级空间索引（视口候选预筛）。指纹未变则复用网格；变了则整体重建；
            // 查询得到视口候选集合，收集阶段只处理候选（静态场景收集 O(N) → O(视口内)）。
            // 与 `cull_bounds` 同条件：3D 世界视图（world_view_is_3d）下遮挡来自深度，网格
            // 预筛不生效；`sprite_culling` 关闭时同样不预筛（全收，像素不变）。
            let candidates: Option<std::collections::HashSet<EntityId>> =
                if self.spatial_index && self.sprite_culling && !world_view_is_3d {
                    let fingerprint = script.with_world(crate::core::hierarchy::local_fingerprint);
                    if self.spatial_index_fingerprint != Some(fingerprint) {
                        // 重建：与提取同一对组件族（inherit 分支同源），只取位置/缩放/尺寸；
                        // for_each2 按 EntityId 升序，rebuild 要求升序输入。
                        let cell_size = self.spatial_index_cell_size;
                        let grid = script.with_world(|world| {
                            let mut items: Vec<(EntityId, crate::core::spatial_index::Circle)> =
                                Vec::new();
                            if inherit {
                                world.for_each2::<GlobalTransform, Sprite>(|id, global, sprite| {
                                    if let Some(plane) = global.plane() {
                                        items.push((
                                            id,
                                            crate::core::spatial_index::Circle::from_sprite(
                                                plane.position,
                                                plane.scale,
                                                sprite.size,
                                            ),
                                        ));
                                    }
                                });
                            } else {
                                world.for_each2::<Transform, Sprite>(|id, transform, sprite| {
                                    items.push((
                                        id,
                                        crate::core::spatial_index::Circle::from_sprite(
                                            transform.position,
                                            transform.scale,
                                            sprite.size,
                                        ),
                                    ));
                                });
                            }
                            let mut grid =
                                crate::core::spatial_index::SpatialGrid2D::new(cell_size);
                            grid.rebuild(items);
                            grid
                        });
                        self.spatial_grid = Some(grid);
                        self.spatial_index_fingerprint = Some(fingerprint);
                    }
                    // 查询候选（CameraBounds → 查询 Bounds）。
                    cull_bounds.map(|bounds| {
                        let query_bounds = crate::core::spatial_index::Bounds {
                            min_x: bounds.min.x,
                            min_y: bounds.min.y,
                            max_x: bounds.max.x,
                            max_y: bounds.max.y,
                        };
                        self.spatial_grid
                            .as_ref()
                            .map(|grid| {
                                grid.query(query_bounds)
                                    .into_iter()
                                    .collect::<std::collections::HashSet<EntityId>>()
                            })
                            .unwrap_or_default()
                    })
                } else {
                    None
                };
            script.with_world(|world| {
                let mut index = 0usize;
                // WP2：提取用的变换来源由空间继承开关**每帧选定一次**：开启时整条换成
                // `WorldTransform`，关闭时仍走 `Transform`。刻意不做"每个实体先试世界再退回局部"
                // ——那会给每个精灵多一次 `get`（两层哈希），WP6 已量到那正是 5 万精灵档的大头。
                // 不变量（见 `core::hierarchy`）：开启继承时**每个有局部变换的实体都有世界变换**
                // （组合不了或成环的保留局部值当世界值），所以整条替换不会漏画任何一个精灵。
                let mut collect = |id: EntityId, transform: &Transform, sprite: &Sprite| {
                    if failure.is_some() {
                        return;
                    }
                    if !crate::core::visibility::entity_visible_in(world, id, view_mask) {
                        filtered += 1;
                        return;
                    }
                    // 与旧实现一致：序号在"通过 Transform 检查"后前进，缺资源的精灵
                    // 仍然占一个序号（否则默认绘制层会整体前移）。
                    let layer_index = index;
                    index += 1;
                    // P1-①：空间索引预筛——不在视口候选里的实体在构建 item 之前就跳过；
                    // 与原路径 push 被 cull 拒绝的净效果一致（像素逐字节相同，docs/53 §3.9）。
                    if candidates.as_ref().is_some_and(|set| !set.contains(&id)) {
                        return;
                    }
                    let (texture, sprite) =
                        match world.get::<AnimationStateMachine>(id).ok().flatten() {
                            Some(machine) => {
                                let (uv_min, uv_max) = machine.current_frame_uv();
                                let sampled = match sprite.with_uv(uv_min, uv_max) {
                                    Ok(sampled) => sampled,
                                    Err(error) => {
                                        failure = Some(error.into());
                                        return;
                                    }
                                };
                                let sheet = machine.current_clip().sheet.as_str();
                                (
                                    texture_id_of(
                                        sheet,
                                        assets,
                                        texture_paths,
                                        texture_lookup,
                                        texture_handles,
                                    ),
                                    sampled,
                                )
                            }
                            None => match world.get::<SpriteAsset>(id).ok().flatten() {
                                Some(asset) => (
                                    texture_id_of(
                                        asset.path(),
                                        assets,
                                        texture_paths,
                                        texture_lookup,
                                        texture_handles,
                                    ),
                                    *sprite,
                                ),
                                None => return,
                            },
                        };
                    let layer = world
                        .get::<SpriteSorting>(id)
                        .ok()
                        .flatten()
                        .map(SpriteSorting::effective_layer)
                        .unwrap_or(1 + layer_index as i32);
                    // docs/54 §4.3：显式显示模式把这段几何路由到对应通道；未声明 = 跟随全局开关。
                    let item = SpriteBatchItem::new(texture, layer, sprite, *transform);
                    match world
                        .get::<crate::core::render_mode::RenderMode>(id)
                        .ok()
                        .flatten()
                        .copied()
                    {
                        Some(crate::core::render_mode::RenderMode::Overlay) => {
                            routed.1 += 1;
                            overlay_batch.push(item);
                        }
                        Some(crate::core::render_mode::RenderMode::Screen) => {
                            routed.2 += 1;
                            screen_batch.push(item);
                        }
                        Some(crate::core::render_mode::RenderMode::World) => {
                            routed.0 += 1;
                            sprite_batch.push(item);
                        }
                        None => sprite_batch.push(item),
                    }
                };
                // `for_each2` 把变换直接交到手上，省掉每个精灵一次 `get::<Transform>`——那是一次
                // `TypeId` 表查询 + 一次实体表查询（两层哈希），5 万精灵档实测就是它的主要成本
                // （docs/53 §3.10）。两者都按"排序实体序"遍历，且没有变换的精灵本来就被跳过
                // （不推进序号），因此语义不变。
                if inherit {
                    world.for_each2::<GlobalTransform, Sprite>(|id, global, sprite| {
                        // 平面视图：不开继承时走局部值，开启时走派生结果（压不平时已退回局部值）。
                        if let Some(plane) = global.plane() {
                            collect(id, &plane, sprite);
                        }
                    });
                } else {
                    world.for_each2::<Transform, Sprite>(|id, transform, sprite| {
                        collect(id, transform, sprite);
                    });
                }
            });
            if let Some(message) = failure {
                return Err(message);
            }
            self.view_filtered.set(self.view_filtered.get() + filtered);
            self.render_mode_routed = routed;
        }
        // docs/54 §4.3：一等 Quad（几何 + 材质，贴图可选）。**刻意走与精灵完全相同的那条路**：
        // Quad 在提取阶段降级成 `Sprite`（`Quad::to_sprite`：几何/材质/深度原样带过去，纹理区域
        // 取整张贴图），因此排序、剔除、深度、可见遮罩、世界/overlay 两条通道全部与精灵同源——
        // "Sprite 与 Quad 共享 RenderWorld 语义"是结构上成立的，而不是两套并行实现靠约定对齐。
        {
            let RuntimeScene {
                script,
                sprite_batch,
                overlay_batch,
                screen_batch,
                texture_paths,
                texture_lookup,
                assets,
                texture_handles,
                ..
            } = self;
            let mut failure: Option<anyhow::Error> = None;
            let inherit = self.spatial_active;
            let view_mask = self.view_world_mask;
            let mut filtered = 0usize;
            let white = crate::core::quad::QUAD_DEFAULT_TEXTURE;
            let mut routed = (0usize, 0usize, 0usize);
            script.with_world(|world| {
                let mut index = 0usize;
                let mut collect = |id: EntityId, transform: &Transform, quad: &Quad| {
                    if failure.is_some() {
                        return;
                    }
                    if !crate::core::visibility::entity_visible_in(world, id, view_mask) {
                        filtered += 1;
                        return;
                    }
                    // docs/54 §4.3：一个实体只能有一种可绘制表达。同时挂 `Sprite` 与 `Quad` 时按
                    // 写下来的优先级只画 `Sprite`（否则会画两遍）；加载期已经就这个冲突点过名。
                    if world.get::<Sprite>(id).ok().flatten().is_some() {
                        return;
                    }
                    let layer_index = index;
                    index += 1;
                    let sprite = match quad.to_sprite() {
                        Ok(sprite) => sprite,
                        Err(error) => {
                            failure = Some(error.into());
                            return;
                        }
                    };
                    // 贴图注册与精灵同一张表：同一个路径只会得到一个下标，因此 Quad 与 Sprite
                    // 引用同一张贴图时能合进同一个绘制段（不会因为"类型不同"而多一次 draw）。
                    let path = quad.texture.as_deref().unwrap_or(white);
                    let texture =
                        texture_id_of(path, assets, texture_paths, texture_lookup, texture_handles);
                    let layer = if quad.sorting == crate::core::sprite::SpriteSorting::default() {
                        // 与精灵同一套默认层规则（按序号）。两个集合的序号各自从 0 起，因此同层时
                        // 后收集的 Quad 画在精灵之上——色块/进度条底这类用途本来就该压在上面。
                        1 + layer_index as i32
                    } else {
                        quad.sorting.effective_layer()
                    };
                    // docs/54 §4.3：显式显示模式把这段几何路由到对应通道；未声明 = 跟随全局开关。
                    let mode = world
                        .get::<crate::core::render_mode::RenderMode>(id)
                        .ok()
                        .flatten()
                        .copied();
                    let item = SpriteBatchItem::new(texture, layer, sprite, *transform);
                    match mode {
                        Some(crate::core::render_mode::RenderMode::Overlay) => {
                            routed.1 += 1;
                            overlay_batch.push(item);
                        }
                        Some(crate::core::render_mode::RenderMode::Screen) => {
                            routed.2 += 1;
                            screen_batch.push(item);
                        }
                        Some(crate::core::render_mode::RenderMode::World) => {
                            routed.0 += 1;
                            sprite_batch.push(item);
                        }
                        None => sprite_batch.push(item),
                    }
                };
                if inherit {
                    world.for_each2::<GlobalTransform, Quad>(|id, global, quad| {
                        if let Some(plane) = global.plane() {
                            collect(id, &plane, quad);
                        }
                    });
                } else {
                    world.for_each2::<Transform, Quad>(|id, transform, quad| {
                        collect(id, transform, quad);
                    });
                }
            });
            if let Some(message) = failure {
                return Err(message);
            }
            self.view_filtered.set(self.view_filtered.get() + filtered);
            self.render_mode_routed = (
                self.render_mode_routed.0 + routed.0,
                self.render_mode_routed.1 + routed.1,
                self.render_mode_routed.2 + routed.2,
            );
        }
        self.push_particles()?;
        // 精灵沿用现有的排序与错误语义，但顶点直接按 NDC 追加到 `vertices` 之后（瓦片之上）。
        // **必须预留容量**：追加式写入若从零一路翻倍到 6 万顶点，光增长就多分配约一倍字节
        // （实测 10k 精灵档 4.53 → 7.53 MB，比单趟化省下的还多）。按每精灵 6 个顶点一次开够；
        // `reserve` 在容量已足时是空操作，不会浪费瓦片层已预留的部分。
        // docs/54 §4.3：**显式显示模式**让同一场景混用世界通道与屏幕空间内容。
        //
        // `mode=world` 是请求世界通道：只要场景里有实体这样声明，这一帧就走世界通道（与宿主开关
        // 同一效果，通道是帧级的——默认实体也跟着进，这条写进 docs/16 避免意外）。
        // `mode=overlay` / `mode=screen` 是退出世界通道：它们**永远**走 NDC overlay 通道，
        // 分别落在 `Overlay2D` 与 `ScreenUI` 桶里（HUD 语义：画在世界内容之上）。
        let explicit_world = self.render_mode_routed.0 > 0;
        let world_channel = self.world_sprite_depth || explicit_world;
        if world_channel {
            // WP6：世界 Sprite 走带深度的通道——世界坐标 + `Sprite::z`，投影交给顶点着色器。
            // 排序/分段/剔除与 overlay 路径同源（同一个批处理、同一把剔除量尺）。
            //
            // docs/54 §8-④：世界通道的剔除量尺必须与世界视图一致——有 3D 内容时收集阶段
            // 已经按"全收"处理（见上面的 `world_view_is_3d`），这里同样不再用 2D 矩形过滤。
            let world_cull_bounds = if world_view_is_3d { None } else { cull_bounds };
            self.sprite_batch.build_world_into(
                &mut world_sprites,
                &mut world_sprite_draws,
                world_cull_bounds,
                self.sprite_parallelism,
            )?;
        } else {
            vertices.reserve(self.sprite_batch.len() * 6);
            draw_ranges.reserve(2);
            self.sprite_batch.build_overlay_into(
                self.camera,
                viewport,
                &mut vertices,
                &mut draw_ranges,
                self.sprite_parallelism,
                self.sprite_culling,
            )?;
        }
        // 显式屏幕空间的批次：无论这一帧走不走世界通道都走 overlay 通道。
        let mut overlay_ranges: Vec<SpriteDrawBatch> = Vec::new();
        let mut screen_ranges: Vec<SpriteDrawBatch> = Vec::new();
        if !self.overlay_batch.is_empty() {
            self.overlay_batch.build_overlay_into(
                self.camera,
                viewport,
                &mut vertices,
                &mut overlay_ranges,
                self.sprite_parallelism,
                self.sprite_culling,
            )?;
        }
        if !self.screen_batch.is_empty() {
            self.screen_batch.build_overlay_into(
                self.camera,
                viewport,
                &mut vertices,
                &mut screen_ranges,
                self.sprite_parallelism,
                self.sprite_culling,
            )?;
        }

        let mut vector_vertices = Vec::with_capacity(256);
        let mut vector_glyphs = Vec::with_capacity(64);
        let mut vector_segments = Vec::with_capacity(1024);
        tracing::debug!(
            target: "render",
            "[render] batch items={} vertices={} draw_ranges={}",
            self.sprite_batch.len(),
            vertices.len(),
            draw_ranges.len()
        );
        let mut draws: Vec<SceneDraw> = draw_ranges
            .iter()
            .map(|draw| SceneDraw {
                texture: draw.texture_id + 1, // 0 is reserved for the white texture
                start: draw.vertex_start as u32,
                count: draw.vertex_count as u32,
                sdf: false,
                bitmap: false,
                vector: false,
                render_layer: RenderLayer::World2D,
            })
            .collect();
        // docs/54 §4.3：显式屏幕空间的批次接在默认批次之后，桶分别是 `Overlay2D` 与 `ScreenUI`
        // （分桶顺序由渲染器保证：World2D → Overlay2D → ScreenUI），因此声明了模式的实体画在世界
        // 内容之上，而 `mode=screen` 又被其它 2D 内容盖不住。
        for (ranges, layer) in [
            (&overlay_ranges, RenderLayer::Overlay2D),
            (&screen_ranges, RenderLayer::ScreenUI),
        ] {
            draws.extend(ranges.iter().map(|draw| SceneDraw {
                texture: draw.texture_id + 1,
                start: draw.vertex_start as u32,
                count: draw.vertex_count as u32,
                sdf: false,
                bitmap: false,
                vector: false,
                render_layer: layer,
            }));
        }
        // WP7 ②：collider 调试绘制（世界内容之上、屏幕 UI 之下）。Lua 的开关请求
        // 在这里被取走，与 camera2d 的请求/消费模式一致。
        if let Some(enabled) = self.script.take_collider_debug_request() {
            self.collider_debug_draw = enabled;
        }
        if let Some(enabled) = self.script.take_collider_debug_request_3d() {
            self.collider_debug_draw_3d = enabled;
        }
        self.push_collider_debug(&mut vertices, &mut draws, viewport);
        self.push_collider_debug_3d(&mut vertices, &mut draws, viewport);
        // 世界空间文字：投影锚点后走同一套字形管线；绘制在精灵之上、屏幕 UI 之下。
        // 不受 `render_ui_overlay` 开关影响——它是世界内容，不是屏幕 UI。
        self.push_world_text(
            &mut vertices,
            &mut world_sprites,
            &mut draws,
            &mut world_sprite_draws,
            &mut vector_vertices,
            &mut vector_glyphs,
            &mut vector_segments,
            width,
            height,
        )?;
        // Screen-space UI (health bars, prompt text) draws last, on top。
        // 编辑器场景视图置 render_ui_overlay=false：UI 由 egui 在设计分辨率包围框内
        // 绘制布局框，跳过此 NDC 覆盖层避免冲突。
        if self.render_ui_overlay {
            self.push_ui_panels(&mut vertices, &mut draws, width, height)?;
            self.push_ui_images(&mut vertices, &mut draws, width, height)?;
            self.push_ui_grids(&mut vertices, &mut draws, width, height)?;
            self.push_ui_bars(&mut vertices, &mut draws, width, height)?;
            self.push_ui_text(
                &mut vertices,
                &mut draws,
                &mut vector_vertices,
                &mut vector_glyphs,
                &mut vector_segments,
                width,
                height,
            )?;
            // Scene-node UI components (Text / Image / Button) drawn on very top.
            self.push_scene_ui(
                &mut vertices,
                &mut draws,
                &mut vector_vertices,
                &mut vector_glyphs,
                &mut vector_segments,
                width,
                height,
            )?;
        }
        // UI 合批：相邻元素在顶点缓冲里连续，合并同纹理同 sdf 的 draw，减少
        // draw call（如一排同纹理方块/面板 → 一次绘制）。
        merge_adjacent_draws(&mut draws);
        // 每帧的 UI 指标不再打 INFO 日志（避免持续刷屏）；需要时由下方 debug 级输出。
        tracing::debug!(
            target: "render",
            "[render] mesh vertices={} draws={} paths={}",
            vertices.len(),
            draws.len(),
            self.texture_paths.len()
        );
        let font_atlas = if let Some(atlas) = &self.font_atlas {
            let atlas = atlas.lock().expect("font atlas lock");
            let version = atlas.version();
            // 仅当本帧网格会被渲染器实际消费（vertices 非空）时才推进“已上报”
            // 版本号。空网格帧渲染器整体跳过字体上传；若此刻推进版本号，下一个
            // 可绘制帧会以“已上报”发送空像素，而渲染器从未上传过该版本 →
            // write_texture 以空数据写整张图集 → wgpu 校验崩溃。
            let changed = version != self.last_font_version && !vertices.is_empty();
            if !vertices.is_empty() {
                self.last_font_version = version;
            }
            Some(FontAtlasPixels {
                width: atlas.width(),
                height: atlas.height(),
                pixels: if changed {
                    atlas.pixels().to_vec()
                } else {
                    Vec::new()
                },
                version,
            })
        } else {
            None
        };
        let custom_font_atlases = self.snapshot_custom_fonts(!vertices.is_empty());
        // WP6 世界通道的**跨产出者**排序：瓦片层、精灵、世界文字是三个各自有序的产出者
        // （瓦片与世界文字都在世界平面 z=0，精灵带自己的 z）。不写深度时顺序就是一切，
        // 因此在交给渲染器之前按"先层、再 z 从远到近、最后原有先后"整体重排一次，
        // 让半透明走画家算法正确叠加。
        sort_world_ranges(&mut world_sprites, &mut world_sprite_draws);
        Ok(SceneMesh {
            vertices,
            draws,
            world_sprites,
            world_sprite_draws,
            paths: self.texture_paths.clone(),
            // docs/54 §4.4：与 `paths` 同序的稳定身份，交给渲染器做 O(1) 的贴图缓存查找。
            path_handles: self.texture_handles.clone(),
            font_atlas,
            custom_font_atlases,
            vector_vertices,
            vector_glyphs,
            vector_segments,
        })
    }

    /// 生成本帧用到（且自上次上报后版本变化）的自定义字体图集像素，随 SceneMesh
    /// 交给渲染器上传/重绑。`pixels` 空 = 未变化（与默认图集 `font_atlas` 语义一致）。
    ///
    /// `will_draw` 必须与默认图集用同一个判据（本帧 `vertices` 非空）：渲染器在
    /// `vertices` 为空时**整体跳过** 2D 通道，自定义字体的上传与 bind 建立也随之
    /// 跳过。若此时仍把版本记为"已上报"，下一个可绘制帧会以"未变化"（空像素）
    /// 发送，而渲染器从未上传过该版本 → 该自定义字体永久不显示。
    fn snapshot_custom_fonts(&mut self, will_draw: bool) -> Vec<CustomFontAtlasPixels> {
        let mut seen: HashSet<String> = HashSet::new();
        let keys: Vec<String> = std::mem::take(&mut self.frame_font_keys)
            .into_iter()
            .filter(|key| seen.insert(key.clone()))
            .collect();
        let mut out = Vec::new();
        for key in keys {
            let handle = match self.custom_font_atlases.lock() {
                Ok(map) => map.get(&key).cloned(),
                Err(_) => None,
            };
            let Some(handle) = handle else {
                continue;
            };
            let atlas = handle.lock().expect("font atlas lock");
            let version = atlas.version();
            let changed = self.custom_font_versions.get(&key).copied() != Some(version);
            if will_draw {
                self.custom_font_versions.insert(key.clone(), version);
            }
            out.push(CustomFontAtlasPixels {
                key,
                pixels: FontAtlasPixels {
                    width: atlas.width(),
                    height: atlas.height(),
                    // 未被绘制的帧不复制像素：既不浪费内存，也不推进"已上报"版本。
                    pixels: if changed && will_draw {
                        atlas.pixels().to_vec()
                    } else {
                        Vec::new()
                    },
                    version,
                },
            });
        }
        out
    }

    /// Pushes every active particle as a small white-texture quad in world
    /// space, so they sit with the scene and fade out with their color alpha.
    fn push_particles(&mut self) -> anyhow::Result<()> {
        // 直接在借用内部写入批次：旧实现先把整条粒子数组 `to_vec()` 拷一份，10k 粒子
        // 每帧白拷 840 KB（`wp0-v4` 的粒子阶梯）。
        let RuntimeScene {
            script,
            sprite_batch,
            texture_paths,
            texture_lookup,
            assets,
            texture_handles,
            ..
        } = self;
        let texture = texture_id_of(
            WHITE_TEXTURE_PATH,
            assets,
            texture_paths,
            texture_lookup,
            texture_handles,
        );
        let mut failure: Option<anyhow::Error> = None;
        script.with_particles(|particles| {
            for particle in particles {
                let size = particle.size();
                // 尺寸曲线收到 0 时该粒子不该再占一个四边形。
                if size <= 0.0 {
                    continue;
                }
                let color = particle.color();
                let sprite = match Sprite::new(Vec2::new(size, size))
                    .and_then(|sprite| sprite.with_tint([color[0], color[1], color[2], color[3]]))
                {
                    Ok(sprite) => sprite,
                    Err(error) => {
                        failure = Some(error.into());
                        return;
                    }
                };
                // 自转复用 Transform，与精灵走同一条变换路径。
                let transform = Transform {
                    rotation: particle.rotation,
                    ..Transform::from_position(particle.position)
                };
                sprite_batch.push(SpriteBatchItem::new(texture, 1, sprite, transform));
            }
        });
        match failure {
            Some(message) => Err(message),
            None => Ok(()),
        }
    }

    /// E16：实体带 `UiLayout` 组件时，其四边 Anchor+Offset 布局优先于
    /// Text/Image 的内联位置数据。把解析矩形回写为 Screen/[0,0] 内联值即可
    /// 复用既有渲染路径——视觉属性（纹理/颜色/文本）仍来自原组件。
    fn scene_ui_layout_rect(&self, id: EntityId, _viewport: [f32; 2]) -> Option<UiRect> {
        self.ui_layout_rects
            .get(&(id.index() as u64))
            .map(|node| node.rect)
    }

    fn ensure_scene_ui_layout(&mut self, viewport: [f32; 2]) {
        let (mut resolved, _) = resolve_scene_ui_layout(&self.ui_scene_nodes, viewport);
        // The resolver also gives non-layout roots a preferred-size rect at the
        // origin. That default is for hierarchy traversal; only authored layouts
        // or container placements may override it.
        resolved.retain(|id, resolved| {
            resolved.container_controlled
                || self
                    .ui_scene_nodes
                    .get(id)
                    .is_some_and(|node| node.layout.is_some())
        });
        self.ui_layout_rects = resolved;
    }

    /// Appends Lua-created `ui.panel` quads using the white texture.
    fn push_ui_panels(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<()> {
        let panels = self.script.with_dynamic_ui(|panels, _, _| panels.clone());
        for panel in panels.values().filter(|panel| panel.visible) {
            let (origin, size) = resolve_ui_rect(
                panel.position,
                panel.size,
                panel.space,
                panel.anchor,
                [width.max(1) as f32, height.max(1) as f32],
                self.ui_reference,
            );
            let start = vertices.len() as u32;
            if panel.shape == 1 {
                // 圆形（`ui.circle`）：2D 管线拓扑是 triangle list（矩形 6 顶点），圆必须同样
                // 展开成三角列表——每段写 (圆心, 环[i], 环[i+1])。三角扇顶点序（圆心 + 一圈
                // 环）在 triangle list 下会被切成碎三角形：一个楔形 + 沿圆周的一串细片，
                // 画面上就是"轮廓圆 + 跨圆交叉细三角"的伪影（虚拟摇杆/按钮实测踩中）。
                let sw = width.max(1) as f32;
                let sh = height.max(1) as f32;
                let cx = origin[0] + size[0] / 2.0;
                let cy = origin[1] + size[1] / 2.0;
                let r = size[0].min(size[1]) / 2.0;
                let to_ndc =
                    |x: f32, y: f32| -> [f32; 2] { [x / sw * 2.0 - 1.0, 1.0 - y / sh * 2.0] };
                let v = |x: f32, y: f32| OverlayVertex {
                    position: to_ndc(x, y),
                    uv: [0.0, 0.0],
                    color: panel.color,
                    sdf: 0,
                };
                const SEGMENTS: usize = 32;
                let center = v(cx, cy);
                for i in 0..SEGMENTS {
                    let a0 = i as f32 / SEGMENTS as f32 * std::f32::consts::PI * 2.0;
                    let a1 = (i + 1) as f32 / SEGMENTS as f32 * std::f32::consts::PI * 2.0;
                    vertices.push(center);
                    vertices.push(v(cx + a0.cos() * r, cy + a0.sin() * r));
                    vertices.push(v(cx + a1.cos() * r, cy + a1.sin() * r));
                }
            } else {
                vertices.extend(screen_rect_ndc(
                    origin[0],
                    origin[1],
                    size[0],
                    size[1],
                    panel.color,
                    width,
                    height,
                ));
            }
            draws.push(SceneDraw {
                texture: 0, // white
                start,
                count: vertices.len() as u32 - start,
                sdf: false,
                bitmap: false,
                vector: false,
                render_layer: RenderLayer::ScreenUI,
            });
        }
        Ok(())
    }

    /// Appends Lua-created `ui.image` quads above panels.
    fn push_ui_images(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<()> {
        let images = self.script.with_dynamic_ui(|_, images, _| images.clone());
        for image in images.values() {
            self.push_image_quad(vertices, draws, width, height, image)?;
        }
        Ok(())
    }

    /// Appends Lua-created `ui.grid` cells and their item icon textures.
    fn push_ui_grids(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<()> {
        let grids = self.script.with_dynamic_ui(|_, _, grids| grids.clone());
        for grid in grids.values() {
            let (origin, total) = resolve_ui_rect(
                grid.position,
                grid.total_size(),
                grid.space,
                grid.anchor,
                [width.max(1) as f32, height.max(1) as f32],
                self.ui_reference,
            );
            let start = vertices.len() as u32;
            vertices.extend(screen_rect_ndc(
                origin[0],
                origin[1],
                total[0],
                total[1],
                grid.background,
                width,
                height,
            ));
            draws.push(SceneDraw {
                texture: 0,
                start,
                count: vertices.len() as u32 - start,
                sdf: false,
                bitmap: false,
                vector: false,
                render_layer: RenderLayer::ScreenUI,
            });
            for index in 1..=grid.columns * grid.rows {
                let Some((cell_origin, cell_size)) = grid.cell_rect(
                    index,
                    [width.max(1) as f32, height.max(1) as f32],
                    self.ui_reference,
                ) else {
                    continue;
                };
                let cell = grid.cells.get(index - 1).cloned().unwrap_or_default();
                let start = vertices.len() as u32;
                vertices.extend(screen_rect_ndc(
                    cell_origin[0],
                    cell_origin[1],
                    cell_size[0],
                    cell_size[1],
                    cell.color,
                    width,
                    height,
                ));
                draws.push(SceneDraw {
                    texture: 0,
                    start,
                    count: vertices.len() as u32 - start,
                    sdf: false,
                    bitmap: false,
                    vector: false,
                    render_layer: RenderLayer::ScreenUI,
                });
                if let Some(texture) = cell.texture {
                    let image = DynamicUiImage {
                        texture,
                        position: cell_origin,
                        size: cell_size,
                        color: [cell.color[0], cell.color[1], cell.color[2]],
                        space: crate::core::ui::UiLayoutSpace::Screen,
                        anchor: [0.0, 0.0],
                        nine_slice: None,
                    };
                    self.push_image_quad(vertices, draws, width, height, &image)?;
                }
            }
        }
        Ok(())
    }

    /// Appends screen-space health bars (background + fill) as NDC quads using
    /// the white texture.
    fn push_ui_bars(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<()> {
        let bars = self.script.with_ui(|_, bars| bars.clone());
        for bar in bars.values() {
            let start = vertices.len() as u32;
            let background = [0.12, 0.12, 0.15, 0.75];
            let fill_width = (bar.size[0] * bar.ratio).max(0.0);
            vertices.extend_from_slice(&screen_rect_ndc(
                bar.position[0],
                bar.position[1],
                bar.size[0],
                bar.size[1],
                background,
                width,
                height,
            ));
            vertices.extend_from_slice(&screen_rect_ndc(
                bar.position[0],
                bar.position[1],
                fill_width,
                bar.size[1],
                bar.color,
                width,
                height,
            ));
            let count = vertices.len() as u32 - start;
            draws.push(SceneDraw {
                texture: 0, // white
                start,
                count,
                sdf: false,
                bitmap: false,
                vector: false,
                render_layer: RenderLayer::ScreenUI,
            });
        }
        Ok(())
    }

    /// Appends screen-space text glyphs (SDF font atlas) as NDC quads. Shares
    /// `layout_text` with the scene-node UI Text component.
    #[allow(clippy::too_many_arguments)]
    fn push_ui_text(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        vector_vertices: &mut Vec<FrameVectorTextVertex>,
        vector_glyphs: &mut Vec<VectorGlyphSnapshot>,
        vector_segments: &mut Vec<VectorSegmentSnapshot>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<()> {
        let texts = self.script.with_ui(|texts, _| texts.clone());
        let visible: Vec<_> = texts
            .values()
            .filter(|text| text.visible)
            .map(|text| {
                (
                    text.position,
                    text.text.clone(),
                    text.font_size.map_or(text.scale, ui_text_effective_scale),
                )
            })
            .collect();
        if visible.is_empty() {
            return Ok(());
        }
        for (position, text, scale) in visible {
            let use_vector = self.should_render_vector(None, scale, text.chars().count());
            if !use_vector
                || !self.push_vector_text_run(
                    vector_vertices,
                    vector_glyphs,
                    vector_segments,
                    &text,
                    position,
                    scale,
                    [1.0, 1.0, 1.0],
                    None,
                    width,
                    height,
                )?
            {
                let (mesh, is_sdf) = self.layout_text(&text, position, scale, [1.0, 1.0, 1.0])?;
                let is_bitmap = mesh.bitmap;
                let text_vertices = text_to_ndc(mesh, is_sdf, width, height);
                let font_key = if is_sdf {
                    FONT_ATLAS_TEXTURE_PATH.to_owned()
                } else {
                    ATLAS_TEXTURE_PATH.to_owned()
                };
                self.push_text_run(
                    vertices,
                    draws,
                    text_vertices,
                    text::TextRunKind {
                        sdf: is_sdf,
                        bitmap: is_bitmap,
                        font_key: &font_key,
                    },
                    RenderLayer::ScreenUI,
                );
            }
        }
        Ok(())
    }

    /// Pushes scene-node UI components (`Text` / `Image` / `Button`) as NDC
    /// overlay quads on top of the Lua UI. Order: images, buttons, then text.
    /// Buttons render by composing the same image/text helpers used elsewhere.
    #[allow(clippy::too_many_arguments)]
    fn push_scene_ui(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        vector_vertices: &mut Vec<FrameVectorTextVertex>,
        vector_glyphs: &mut Vec<VectorGlyphSnapshot>,
        vector_segments: &mut Vec<VectorSegmentSnapshot>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<()> {
        let viewport = [width.max(1) as f32, height.max(1) as f32];
        self.ensure_scene_ui_layout(viewport);
        // docs/54 §4.2：实体身份的 UI 吃 **overlay 视图**的可见遮罩；按名字注册的画布 UI
        // （面板/血条/网格）不是实体，因此不受遮罩影响。
        let overlay_mask = self.view_overlay_mask;
        let mut filtered = 0usize;
        let images: Vec<(EntityId, UiImage)> = self.script.with_world(|world| {
            let mut images = Vec::new();
            world.for_each::<UiImage>(|id, image| {
                if crate::core::visibility::entity_visible_in(world, id, overlay_mask) {
                    images.push((id, image.clone()));
                } else {
                    filtered += 1;
                }
            });
            images
        });
        for (id, image) in images {
            if let Some(rect) = self.scene_ui_layout_rect(id, viewport) {
                let image = DynamicUiImage {
                    texture: image.texture,
                    position: [rect.x, rect.y],
                    size: [rect.w, rect.h],
                    color: image.color,
                    space: crate::core::ui::UiLayoutSpace::Screen,
                    anchor: [0.0, 0.0],
                    nine_slice: image.nine_slice,
                };
                self.push_image_quad(vertices, draws, width, height, &image)?;
            }
        }
        let texts: Vec<(EntityId, UiText)> = self.script.with_world(|world| {
            let mut texts = Vec::new();
            world.for_each::<UiText>(|id, text| {
                // `ui.scene_text_visible(name, false)` 按实体名隐藏场景文字：
                // 实体仍在场景与 UiLayout 布局里，仅跳过绘制（语义与 web 兼容层一致）。
                // P3-②：换成免克隆判定（旧实现每文字每帧 clone 整份隐藏集）。
                let name_hidden = self.script.is_entity_scene_text_hidden(id);
                if crate::core::visibility::entity_visible_in(world, id, overlay_mask)
                    && !name_hidden
                {
                    texts.push((id, text.clone()));
                } else {
                    filtered += 1;
                }
            });
            texts
        });
        self.view_filtered.set(self.view_filtered.get() + filtered);
        // 关键：先把本帧全部文本的字形预热进图集，再逐条构建网格。若在排版过程
        // 中才光栅化，图集会增长（1024→2048…）；`FontAtlas::grow` 只重缩放已缓存
        // 字形的归一化 UV，不会修正已经写进顶点缓冲的 UV，于是同一帧内先构建的
        // 网格会用旧尺寸的 UV 在更大的图集里采样到 2 倍坐标处的错误字形，表现为
        // 白块 / 缺笔画。预热后图集尺寸在本帧保持稳定。
        self.prewarm_frame_glyphs(&texts);
        for (id, text) in texts {
            let rect = self.scene_ui_layout_rect(id, viewport);
            if let Some(rect) = rect {
                // 点锚点（w=h=0）的 Text：按 pivot 在该点居中文字。
                // 否则文字从 pivot 点开始左对齐向下排，偏出按钮中心。
                let pivot = self
                    .ui_scene_nodes
                    .get(&(id.index() as u64))
                    .and_then(|n| n.layout.as_ref())
                    .map(|l| l.pivot)
                    .unwrap_or([0.5, 0.5]);
                let mut position = [rect.x, rect.y];
                // 先判断渲染路径，measure 必须与渲染走同一路径，否则字体度量不一致。
                let use_vector = self.should_render_vector(
                    Some(text.font.as_str()),
                    text.effective_scale(),
                    text.text.chars().count(),
                );
                if rect.w < 1.0 && rect.h < 1.0 {
                    // 收集实际渲染顶点的包围盒（origin=[0,0] 时）。
                    let mut bbox: Option<(f32, f32, f32, f32)> = None;
                    if use_vector
                        && let Some(handle) = self.ensure_vector_font(if text.font.is_empty() {
                            ""
                        } else {
                            text.font.as_str()
                        })
                        && let Ok(mut atlas) = handle.lock()
                    {
                        let style = crate::core::text::TextStyle {
                            scale: text.effective_scale(),
                            color: [1.0, 1.0, 1.0, 1.0],
                            ..Default::default()
                        };
                        if let Ok(verts) = crate::core::text::TextLayout::build_vector(
                            &text.text,
                            [0.0, 0.0],
                            style,
                            &mut atlas,
                        ) {
                            bbox = Some(verts.iter().fold(
                                (f32::MAX, f32::MIN, f32::MAX, f32::MIN),
                                |(a, b, c, d), v| {
                                    (
                                        a.min(v.position[0]),
                                        b.max(v.position[0]),
                                        c.min(v.position[1]),
                                        d.max(v.position[1]),
                                    )
                                },
                            ));
                        }
                    }
                    if bbox.is_none()
                        && let Ok((measure, _)) = self.layout_text(
                            &text.text,
                            [0.0, 0.0],
                            text.effective_scale(),
                            text.color,
                        )
                    {
                        bbox = Some(measure.vertices.iter().fold(
                            (f32::MAX, f32::MIN, f32::MAX, f32::MIN),
                            |(a, b, c, d), v| {
                                (
                                    a.min(v.position[0]),
                                    b.max(v.position[0]),
                                    c.min(v.position[1]),
                                    d.max(v.position[1]),
                                )
                            },
                        ));
                    }
                    if let Some((min_x, max_x, min_y, max_y)) = bbox {
                        position[0] = rect.x - min_x * (1.0 - pivot[0]) - max_x * pivot[0];
                        position[1] = rect.y - min_y * (1.0 - pivot[1]) - max_y * pivot[1];
                    }
                }
                // `layout_text` already emits absolute screen-space vertices;
                // adding the layout origin again would displace scene text by
                // its full top-left position (the visible "text ran away" bug).
                if use_vector
                    && self.push_vector_text_run(
                        vector_vertices,
                        vector_glyphs,
                        vector_segments,
                        &text.text,
                        position,
                        text.effective_scale(),
                        text.color,
                        Some(&text.font),
                        width,
                        height,
                    )?
                {
                    continue;
                }
                let (mesh, is_sdf, font_key) = self.layout_scene_text(&text, position)?;
                let is_bitmap = mesh.bitmap;
                let text_vertices = text_to_ndc(mesh, is_sdf, width, height);
                self.push_text_run(
                    vertices,
                    draws,
                    text_vertices,
                    text::TextRunKind {
                        sdf: is_sdf,
                        bitmap: is_bitmap,
                        font_key: &font_key,
                    },
                    RenderLayer::ScreenUI,
                );
            }
        }
        Ok(())
    }

    /// Pushes a screen-space textured quad for the `Image` component.
    fn push_image_quad(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        draws: &mut Vec<SceneDraw>,
        width: u32,
        height: u32,
        image: &DynamicUiImage,
    ) -> anyhow::Result<()> {
        let sw = width.max(1) as f32;
        let sh = height.max(1) as f32;
        let (origin, size) = resolve_ui_rect(
            image.position,
            image.size,
            image.space,
            image.anchor,
            [sw, sh],
            self.ui_reference,
        );
        let color = [image.color[0], image.color[1], image.color[2], 1.0];
        // 九宫格路径：把目标矩形拆成 3×3 格。无切片 / 纹理尺寸取不到时回退单四边形。
        if let Some(slice) = image
            .nine_slice
            .filter(|s| s.iter().all(|v| v.is_finite() && *v >= 0.0) && s.iter().any(|v| *v > 0.0))
            && let Some((tw, th)) = self.texture_dimensions(&image.texture)
        {
            // 把作者空间边框映射到解析后的目标空间，使角部在 Canvas 缩放下
            // 保持纹理像素密度（Screen / scale=1 时比值为 1）。
            let scale_x = if image.size[0] > f32::EPSILON {
                size[0] / image.size[0]
            } else {
                1.0
            };
            let scale_y = if image.size[1] > f32::EPSILON {
                size[1] / image.size[1]
            } else {
                1.0
            };
            let slice = [
                slice[0] * scale_x,
                slice[1] * scale_x,
                slice[2] * scale_y,
                slice[3] * scale_y,
            ];
            let cells = nine_slice_cells(size[0], size[1], tw as f32, th as f32, slice);
            let start = vertices.len() as u32;
            for cell in cells {
                let cw = cell.dest[2] - cell.dest[0];
                let ch = cell.dest[3] - cell.dest[1];
                if cw <= 0.0 || ch <= 0.0 {
                    continue;
                }
                let x0 = (origin[0] + cell.dest[0]) / sw * 2.0 - 1.0;
                let y0 = 1.0 - (origin[1] + cell.dest[1]) / sh * 2.0;
                let x1 = (origin[0] + cell.dest[2]) / sw * 2.0 - 1.0;
                let y1 = 1.0 - (origin[1] + cell.dest[3]) / sh * 2.0;
                vertices.extend(nine_slice_quad(x0, y0, x1, y1, cell.src, color));
            }
            if vertices.len() as u32 > start {
                draws.push(SceneDraw {
                    texture: self.texture_id(&image.texture) + 1,
                    start,
                    count: vertices.len() as u32 - start,
                    sdf: false,
                    bitmap: false,
                    vector: false,
                    render_layer: RenderLayer::ScreenUI,
                });
            }
            return Ok(());
        }
        let x0 = origin[0] / sw * 2.0 - 1.0;
        let y0 = 1.0 - origin[1] / sh * 2.0;
        let x1 = (origin[0] + size[0]) / sw * 2.0 - 1.0;
        let y1 = 1.0 - (origin[1] + size[1]) / sh * 2.0;
        let quad = [
            OverlayVertex {
                position: [x0, y0],
                uv: [0.0, 0.0],
                color,
                sdf: 0,
            },
            OverlayVertex {
                position: [x1, y0],
                uv: [1.0, 0.0],
                color,
                sdf: 0,
            },
            OverlayVertex {
                position: [x1, y1],
                uv: [1.0, 1.0],
                color,
                sdf: 0,
            },
            OverlayVertex {
                position: [x0, y0],
                uv: [0.0, 0.0],
                color,
                sdf: 0,
            },
            OverlayVertex {
                position: [x1, y1],
                uv: [1.0, 1.0],
                color,
                sdf: 0,
            },
            OverlayVertex {
                position: [x0, y1],
                uv: [0.0, 1.0],
                color,
                sdf: 0,
            },
        ];
        let start = vertices.len() as u32;
        vertices.extend(quad);
        draws.push(SceneDraw {
            texture: self.texture_id(&image.texture) + 1,
            start,
            count: vertices.len() as u32 - start,
            sdf: false,
            bitmap: false,
            vector: false,
            render_layer: RenderLayer::ScreenUI,
        });
        Ok(())
    }

    /// Deduplicates a texture asset path and returns its id for batch items.
    fn texture_id(&mut self, path: &str) -> u32 {
        texture_id_of(
            path,
            &self.assets,
            &mut self.texture_paths,
            &mut self.texture_lookup,
            &mut self.texture_handles,
        )
    }

    /// 纹理像素尺寸（惰性、按 path 缓存），仅供九宫格 UV 计算。只读 PNG 头，
    /// 不做全量解码。路径解析与渲染器一致（`project_root.join(path)`，无根则用
    /// 原始路径），并走与纹理加载相同的 pack/decode 解包。读取失败 → `None` →
    /// 回退单四边形。
    fn texture_dimensions(&mut self, path: &str) -> Option<(u32, u32)> {
        if let Some(dims) = self.texture_dims.get(path) {
            return Some(*dims);
        }
        let resolved = self
            .project_root
            .as_ref()
            .map(|root| root.join(path))
            .unwrap_or_else(|| PathBuf::from(path));
        let dims = fs::read(&resolved)
            .ok()
            .and_then(|bytes| crate::core::pack::decode(&bytes).ok())
            .and_then(|bytes| {
                image::ImageReader::new(std::io::Cursor::new(bytes))
                    .with_guessed_format()
                    .ok()?
                    .into_dimensions()
                    .ok()
            });
        if let Some(dims) = dims {
            self.texture_dims.insert(path.to_owned(), dims);
        }
        dims
    }

    /// Builds the 2D mesh for all Tilemap components using a tilemap-specific
    /// hot path that precomputes shared camera/rotation terms once per frame.
    ///
    /// 缓存命中时**不克隆地图**：把上一帧的缓存整体取出，命中的条目直接移动进新缓存，
    /// 只有真正变化的地图才克隆重建。旧实现每帧为每个 Tilemap 克隆整张地图，100×100
    /// 实测每帧 8.5 MB 分配（`wp0-v4` 的 tilemap 阶梯）。
    fn append_tilemap_vertices(
        &mut self,
        vertices: &mut Vec<OverlayVertex>,
        world_vertices: &mut Vec<WorldSpriteVertex>,
        overlay_draws: &mut Vec<SpriteDrawBatch>,
        world_draws: &mut Vec<SpriteDrawBatch>,
        viewport: Viewport,
    ) -> anyhow::Result<()> {
        let Ok(camera) = OrthoTerms::new(self.camera, viewport) else {
            // 相机退化（zoom 非正等）时不产瓦片顶点：与精灵路径一致，等宿主修好相机再画。
            return Ok(());
        };
        // WP4 ④：视口外瓦片只在 `vertices()` 之外还被用到时不剔除（不存在这种调用），
        // 因此这里直接取运行时开关；关掉即像素对拍用的"整层全画"档。
        let cull = self.tilemap_culling;
        // WP6：世界 Sprite 深度通道打开时，瓦片层也产出**世界坐标**顶点（与精灵一致），
        // 因此它同样参与深度测试——此前只有精灵有世界顶点，瓦片会被挡在通道外面。
        let world_pass = self.world_sprite_depth;
        // 与世界 Sprite 同一条规矩：世界通道的可见域由宿主声明的世界视图决定，有 3D 内容时
        // 一律不按 2D 相机裁剪（宁可多画，绝不误丢）。
        let cull_bounds = if cull && world_pass && !self.world_view_has_3d_content() {
            self.camera.visible_world_bounds(viewport).ok()
        } else {
            None
        };
        {
            let RuntimeScene {
                script,
                tilemap_cache,
                texture_paths,
                texture_lookup,
                assets,
                texture_handles,
                ..
            } = self;
            let mut previous = std::mem::take(tilemap_cache);
            let mut next_cache = HashMap::with_capacity(previous.len());
            let mut failure: Option<anyhow::Error> = None;
            let inherit = self.spatial_active;
            script.with_world(|world| {
                // WP2：瓦片层挂在**世界**位置上（继承开启时读传播结果，关闭时读局部值）。
                // 缓存比较用的也是同一个值，因此开关切换会让缓存自然失效、不会画在旧位置。
                let mut collect = |entity: EntityId, transform: &Transform, tilemap: &Tilemap| {
                    if failure.is_some() {
                        return;
                    }
                    let texture = texture_id_of(
                        &tilemap.texture,
                        assets,
                        texture_paths,
                        texture_lookup,
                        texture_handles,
                    );
                    let mut layer = match previous.remove(&entity) {
                        Some(layer)
                            if layer.tilemap == *tilemap
                                && layer.transform.scale == transform.scale =>
                        {
                            // 命中：瓦片与缩放都没变。`center_offset` 相对层原点，
                            // 与 position/rotation 无关——只同步 transform，不重建任何块。
                            layer
                        }
                        Some(mut layer)
                            if layer.tilemap.width == tilemap.width
                                && layer.tilemap.height == tilemap.height =>
                        {
                            // 未命中但**尺寸相同**：逐块比对 tiles 切片，只重建变化的块
                            // （scale 变化时脏重建会把每一块都判为变化，等于整层重建）。
                            let rebuilt =
                                rebuild_dirty_chunks(&mut layer, tilemap, transform.scale);
                            if rebuilt > 0 {
                                tracing::debug!(
                                    target: "scene",
                                    "tilemap chunk rebuild: {} chunk(s)",
                                    rebuilt
                                );
                            }
                            layer.tilemap = tilemap.clone();
                            layer
                        }
                        // 尺寸不同（室外 40×23 → 室内 20×12 换图）或首次出现：整层重建。
                        // 不能走 `rebuild_dirty_chunks`——它按旧层的 chunk 网格
                        // （`oy..oy+h`、`ox..ox+w` 可达旧图的 40×23）去索引**新图**的
                        // `tiles`（宽只有 20、len 240），会越界 panic。
                        _ => match build_all_chunks(*transform, tilemap.clone()) {
                            Ok(layer) => layer,
                            Err(error) => {
                                failure = Some(error);
                                return;
                            }
                        },
                    };
                    layer.transform = *transform;
                    // 预留本层顶点容量：`Vec::new()` 一路翻倍到 6 万顶点会额外搬上兆字节
                    // （`wp0-v4` tilemap 阶梯实测的每帧分配里，增长churn 占约 1.9 MB）。
                    if world_pass {
                        world_vertices.reserve(layer.total * 6);
                        world_draws.reserve(1);
                        append_tilemap_layer_world(
                            world_vertices,
                            world_draws,
                            texture,
                            &layer,
                            cull_bounds,
                        );
                    } else {
                        vertices.reserve(layer.total * 6);
                        overlay_draws.reserve(1);
                        append_tilemap_layer(
                            vertices,
                            overlay_draws,
                            texture,
                            camera,
                            &layer,
                            cull,
                        );
                    }
                    next_cache.insert(entity, layer);
                };
                if inherit {
                    world.for_each2::<GlobalTransform, Tilemap>(|entity, global, tilemap| {
                        if let Some(plane) = global.plane() {
                            collect(entity, &plane, tilemap);
                        }
                    });
                } else {
                    world.for_each2::<Transform, Tilemap>(|entity, transform, tilemap| {
                        collect(entity, transform, tilemap);
                    });
                }
            }); // 先提交已处理部分，再报错：出错帧不清空整张缓存，下一帧能继续复用。
            *tilemap_cache = next_cache;
            if let Some(message) = failure {
                return Err(message);
            }
        }
        Ok(())
    }
}

/// 屏幕空间的一条细线 → 6 个 NDC 顶点（两个三角形）。
///
/// 用四边形而不是 `LineStrip`：overlay 管线只有三角形，且这样与精灵/文字走同一条顶点路径。
fn push_screen_line(
    vertices: &mut Vec<OverlayVertex>,
    from: [f32; 2],
    to: [f32; 2],
    color: [f32; 4],
    viewport: Viewport,
) {
    const HALF_THICKNESS: f32 = 0.75;
    let dx = to[0] - from[0];
    let dy = to[1] - from[1];
    let length = (dx * dx + dy * dy).sqrt();
    if length <= f32::EPSILON {
        return;
    }
    let normal = [-dy / length * HALF_THICKNESS, dx / length * HALF_THICKNESS];
    let corners = [
        [from[0] - normal[0], from[1] - normal[1]],
        [from[0] + normal[0], from[1] + normal[1]],
        [to[0] + normal[0], to[1] + normal[1]],
        [to[0] - normal[0], to[1] - normal[1]],
    ];
    let n = |corner: usize| {
        OverlayVertex::from_ndc(ndc_from_screen(corners[corner], viewport), [0.0; 2], color)
    };
    vertices.extend_from_slice(&[n(0), n(1), n(2), n(0), n(2), n(3)]);
}

/// 纹理路径 → 每帧下标注册。
///
/// 抽成自由函数是为了让 `vertices` 能在 `world` 借用内部直接写入这几张表，而不必先
/// 克隆一份中间表——那样每帧要为每个精灵克隆一次资源路径（见该处注释）。
///
/// docs/54 §4.4：返回的是**本帧路径下标**（渲染器据此取路径）。同时把该路径的**稳定句柄**追加进
/// `handles`（与 `paths` 同序），并保证同一路径在进程内恒同句柄——身份因此跨帧可比（`assets`
/// 不清空，而 `paths`/`handles` 每帧重建）。
fn texture_id_of(
    path: &str,
    assets: &std::cell::RefCell<AssetStore<AssetRecord>>,
    paths: &mut Vec<String>,
    lookup: &mut HashMap<String, u32>,
    handles: &mut Vec<AssetHandle>,
) -> u32 {
    if let Some(index) = lookup.get(path) {
        return *index;
    }
    // 身份表（跨帧）与路径表（本帧）各查一次哈希，都是 O(1)：批量精灵档因此不会因为引入身份
    // 而多出任何线性扫描。
    let (handle, _) = assets.borrow_mut().intern(
        path,
        AssetRecord {
            path: path.to_owned(),
            kind: ResourceKind::from_path(path),
        },
    );
    let index = paths.len() as u32;
    paths.push(path.to_owned());
    handles.push(handle);
    lookup.insert(path.to_owned(), index);
    index
}

/// Captures the serialized scene hierarchy and UI layout data once. Runtime
/// entity slots are allocated in scene iteration order, so layout nodes are
/// keyed by that order rather than by the scene file's sparse entity ids.
///
/// **只为参与场景 UI 的实体建节点**：带 UI 组件的实体，以及它们的祖先与后代。
/// 祖先用于解析父链，后代是因为容器的子节点自身可能没有 UI 组件却被容器摆放。
/// 其余实体（例如上万个纯精灵）不建节点——否则每帧的布局解算会随**总实体数**增长
/// （`wp0-v4` 实测 10k 纯精灵场景每帧多出 3,339 次分配，正是这张全量节点表造成的）。
fn build_ui_scene_nodes(scene: &Scene, honor_editor_hidden: bool) -> BTreeMap<u64, UiSceneNode> {
    /// 决定实体是否参与场景 UI 的组件名。
    const UI_COMPONENTS: [&str; 6] = [
        "UiLayout",
        "Container",
        "Text",
        "Image",
        "UiEvents",
        "ButtonStates",
    ];
    let mut id_to_key = BTreeMap::new();
    for entity in scene.entities.values() {
        if honor_editor_hidden && scene.is_entity_hidden_with_ancestors(entity.id) {
            continue;
        }
        id_to_key.insert(entity.id.0, id_to_key.len() as u64);
    }
    // 相关集合：UI 实体 + 祖先 + 后代。
    let mut relevant = HashSet::new();
    for entity in scene.entities.values() {
        if UI_COMPONENTS
            .iter()
            .any(|name| entity.components.contains_key(*name))
        {
            relevant.insert(entity.id.0);
        }
    }
    let mut children: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    for entity in scene.entities.values() {
        if let Some(parent) = entity.parent {
            children.entry(parent.0).or_default().push(entity.id.0);
        }
    }
    let mut frontier = relevant.iter().copied().collect::<Vec<_>>();
    while let Some(id) = frontier.pop() {
        if let Some(entity) = scene.entities.get(&SceneEntityId(id))
            && let Some(parent) = entity.parent
            && relevant.insert(parent.0)
        {
            frontier.push(parent.0);
        }
        if let Some(kids) = children.get(&id) {
            for kid in kids {
                if relevant.insert(*kid) {
                    frontier.push(*kid);
                }
            }
        }
    }
    let mut nodes = BTreeMap::new();
    for entity in scene.entities.values() {
        if honor_editor_hidden && scene.is_entity_hidden_with_ancestors(entity.id) {
            continue;
        }
        if !relevant.contains(&entity.id.0) {
            continue;
        }
        let Some(&entity_key) = id_to_key.get(&entity.id.0) else {
            continue;
        };
        let layout = entity
            .components
            .get("UiLayout")
            .and_then(|raw| UiRectLayout::from_component_str(raw).ok());
        let container = entity
            .components
            .get("Container")
            .and_then(|raw| crate::core::ui_layout::ContainerStyle::from_component_str(raw).ok());
        let preferred_size = if entity.components.contains_key("Image") {
            layout
                .as_ref()
                .map(|layout| layout.resolve([0.0, 0.0]))
                .map_or([64.0, 64.0], |rect| [rect.w, rect.h])
        } else {
            [64.0, 64.0]
        };
        nodes.insert(
            entity_key,
            UiSceneNode {
                parent: entity.parent.and_then(|id| id_to_key.get(&id.0)).copied(),
                layout,
                container,
                preferred_size,
            },
        );
    }
    nodes
}

/// WP4 ④ chunk 级脏重建：瓦片块边长（格数）。块行主序拼接 == 原整层行主序。
const TILE_CHUNK_SIZE: usize = 32;

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
fn build_all_chunks(transform: Transform, tilemap: Tilemap) -> anyhow::Result<CachedTilemapLayer> {
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
fn rebuild_dirty_chunks(layer: &mut CachedTilemapLayer, tilemap: &Tilemap, scale: Vec2) -> usize {
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
fn append_tilemap_layer<V: OverlayVertexSink>(
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
fn append_tilemap_layer_world<V: WorldSpriteVertexSink>(
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
fn sort_world_ranges(vertices: &mut Vec<WorldSpriteVertex>, draws: &mut Vec<SpriteDrawBatch>) {
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

fn startup_scene_path(project_root: &Path) -> String {
    project_font::read_key(project_root, "startup_scene")
        .unwrap_or_else(|| "scenes/main.scene".to_owned())
}

/// project.toml 的 `text_render_mode` 键；无效/缺省回退 `auto`。
fn project_text_render_mode(project_root: Option<&Path>) -> TextRenderMode {
    project_root
        .and_then(|root| project_font::read_key(root, "text_render_mode"))
        .map(|mode| TextRenderMode::parse(&mode))
        .unwrap_or(TextRenderMode::Auto)
}

/// 加载项目字体并包装为可跨运行时共享的矢量轮廓缓存。
pub fn load_project_vector_atlas(
    project_root: Option<&Path>,
) -> Option<Arc<Mutex<FontVectorAtlas>>> {
    let root = project_root?;
    let primary = fs::read(root.join(project_font::primary_path(root))).ok()?;
    let primary = crate::core::pack::decode(&primary).ok()?;
    let fallback = project_font::fallback_path(root).and_then(|path| {
        let bytes = fs::read(root.join(path)).ok()?;
        crate::core::pack::decode(&bytes).ok()
    });
    FontVectorAtlas::load_with_fallback(&primary, fallback.as_deref())
        .ok()
        .map(|atlas| Arc::new(Mutex::new(atlas)))
}

/// 加载项目字体并包装为可跨运行时共享的图集（MSDF 字形按需生成，共享后只生成
/// 缺省路径是 NotoSansSC-VF.ttf，项目可通过 `font` 键切换字体；
/// 缺省字重是 Regular（400）。
pub fn load_project_font_atlas(project_root: Option<&Path>) -> Option<Arc<Mutex<FontAtlas>>> {
    let root = project_root?;
    let primary_path = {
        let path = root.join(project_font::primary_path(root));
        path.is_file().then_some(path)
    }?;
    let primary_bytes = fs::read(&primary_path).ok()?;
    let primary_bytes = crate::core::pack::decode(&primary_bytes).ok()?;
    // 可选后备字体（project.toml `font_fallback` 键），用于主字体缺失的字形。
    let fallback_bytes = project_font::fallback_path(root)
        .and_then(|font| {
            let path = root.join(font);
            path.is_file().then_some(path)
        })
        .and_then(|path| {
            let bytes = fs::read(&path).ok()?;
            crate::core::pack::decode(&bytes).ok()
        });
    let font_weight = project_font::weight(root);
    let mut atlas = match FontAtlas::load_with_fallback_and_weight(
        &primary_bytes,
        fallback_bytes.as_deref(),
        font_weight,
    ) {
        Ok(atlas) => atlas,
        Err(error) => {
            tracing::warn!(target: "render", error = %error, "[render] font atlas load failed; text uses placeholders");
            return None;
        }
    };
    tracing::info!(
        target: "render",
        path = %primary_path.display(),
        weight = font_weight,
        "[render] font atlas ready"
    );
    // 首包预热：把打包时收集的字符清单（chars.txt）预生成进图集，首帧文字不卡。
    prewarm_font_atlas(root, &mut atlas);
    Some(Arc::new(Mutex::new(atlas)))
}

/// 从项目根 `chars.txt`（打包时由 assetc 生成）读取字符并预热字形。
/// MSDF 光栅化跨线程并行，debug 构建下显著加速（ui_demo 135 字从 ~2.3s 降到 ~0.3s）。
fn prewarm_font_atlas(root: &Path, atlas: &mut FontAtlas) {
    let Ok(bytes) = fs::read(root.join("chars.txt")) else {
        return;
    };
    let Ok(bytes) = crate::core::pack::decode(&bytes) else {
        return;
    };
    let text = String::from_utf8_lossy(&bytes);
    let count = atlas.prewarm(text.chars());
    if count > 0 {
        tracing::info!(target: "render", count, "[render] font prewarmed {count} chars");
    }
}

pub fn load_custom_vector_atlas(
    project_root: Option<&Path>,
    font_asset: &str,
) -> Option<Arc<Mutex<FontVectorAtlas>>> {
    let root = project_root?;
    if font_asset.trim().is_empty() {
        return None;
    }
    let bytes = fs::read(root.join(font_asset)).ok()?;
    let bytes = crate::core::pack::decode(&bytes).ok()?;
    let fallback = project_font::fallback_path(root).and_then(|path| {
        let bytes = fs::read(root.join(path)).ok()?;
        crate::core::pack::decode(&bytes).ok()
    });
    FontVectorAtlas::load_with_fallback(&bytes, fallback.as_deref())
        .ok()
        .map(|atlas| Arc::new(Mutex::new(atlas)))
}

/// 读取 + pack 解码 + 项目 fallback 字体 + 项目字重。**不做** chars.txt 预热
/// （字形按需光栅化）。任何失败返回 `None` → 调用方回退默认字体，不中断渲染
/// （wasm / 发布包缺该字体时自动降级）。
pub fn load_custom_font_atlas(project_root: Option<&Path>, font_asset: &str) -> Option<FontHandle> {
    let root = project_root?;
    if font_asset.trim().is_empty() {
        return None;
    }
    let path = root.join(font_asset);
    if !path.is_file() {
        return None;
    }
    let bytes = fs::read(&path).ok()?;
    let bytes = crate::core::pack::decode(&bytes).ok()?;
    // 与默认字体一致地附上项目后备字体，缺字时回退（如拉丁字体不含 CJK）。
    let fallback_bytes = project_font::fallback_path(root)
        .and_then(|font| {
            let path = root.join(font);
            path.is_file().then_some(path)
        })
        .and_then(|path| {
            let bytes = fs::read(&path).ok()?;
            crate::core::pack::decode(&bytes).ok()
        });
    let font_weight = project_font::weight(root);
    let atlas = match FontAtlas::load_with_fallback_and_weight(
        &bytes,
        fallback_bytes.as_deref(),
        font_weight,
    ) {
        Ok(atlas) => atlas,
        Err(error) => {
            tracing::warn!(
                target: "render",
                error = %error,
                path = %font_asset,
                "[render] custom font atlas load failed; text falls back to default"
            );
            return None;
        }
    };
    tracing::info!(target: "render", path = %font_asset, "[render] custom font atlas ready");
    Some(Arc::new(Mutex::new(atlas)))
}

/// 世界里声明的第一台**启用**的 2D 相机（`Camera projection=orthographic`），没有则 `None`。
///
/// **启动场景与换图读的是同一份口径**：`init` 用它决定初始相机，`sync_camera_from_scene`
/// 用它决定换图后的 bounds。两处各写一遍的话，"哪台算场景相机"就会有两个答案。
///
/// docs/54 §4.2：被禁用的相机不参与选择（与 3D 侧同一条规则），但过滤在遍历
/// **里面**——"第一台被禁用"不该挡掉后面启用的那台。
///
/// 运行时世界里只有**一个** `Camera` 组件，因此这里单趟遍历、按投影分支取值，
/// 不再需要"先取一份 `Vec<EntityId>` 再逐实体 `get`"。
fn declared_camera_2d_in(world: &crate::core::world::World) -> Option<Camera2D> {
    first_enabled_ortho_camera_in(world).map(|(_, camera)| camera)
}

/// 排序序第一台**启用**的正交相机（拆分批 9：这是"默认 2D 相机"的唯一定义处）。
fn first_enabled_ortho_camera_in(
    world: &crate::core::world::World,
) -> Option<(EntityId, Camera2D)> {
    let mut found = None;
    world.for_each::<crate::core::camera::Camera>(|id, camera| {
        if found.is_some() {
            return;
        }
        if let Some(orthographic) = camera.as_orthographic()
            && orthographic.enabled
        {
            found = Some((id, orthographic));
        }
    });
    found
}

/// 拆分批 9：**2D 相机的来源**——`camera.set_active` 选中的那台**正交**相机（与 3D pass 共用同
/// 一条选择链，只是候选按投影过滤）。
///
/// 语义（为什么与 `active_camera3d` 不完全对称）：
/// - 选中的是正交相机且启用 ⇒ 返回它（宿主采用它的**载荷**作为 2D 取景基准，即"换 2D 相机"）；
/// - 选中的正交相机被禁用 ⇒ 告警并回退到排序序第一台启用的正交相机（与 3D 侧选中禁用相机同一
///   规则、同一告警形状）；
/// - 没选中、或选中的是**透视**相机 ⇒ 返回 `None`＝**2D 取景保持现状**。这一条很关键：脚本切换
///   3D 相机（最常见操作）绝不该把 2D 相机重置回作者写的初始值——2D 的 center/zoom 归
///   `Camera2dController`（跟随/死区/平滑/抖动/请求），重置会在切相机那一帧闪一下。
fn selected_camera_2d_in(
    world: &crate::core::world::World,
    selection: CameraSelection,
) -> Option<(EntityId, Camera2D)> {
    let mut chosen = None;
    let mut selected_disabled = false;
    world.for_each::<crate::core::camera::Camera>(|id, camera| {
        // 正交相机是 2D 的唯一候选：选中的透视相机在这里被过滤掉（它服务 3D pass）。
        let Some(component) = camera.as_orthographic() else {
            return;
        };
        if !selection.is_selected(id.index() as u64) {
            return;
        }
        if component.enabled {
            chosen = Some((id, component));
        } else {
            selected_disabled = true;
        }
    });
    if chosen.is_some() {
        return chosen;
    }
    if selected_disabled {
        tracing::warn!(
            target: "render",
            "[render] `camera.set_active` 选中的相机 `enabled=false`，2D 视图已回退到下一台可用相机"
        );
        return first_enabled_ortho_camera_in(world);
    }
    None
}

/// Sprite 的世界矩形（pivot+缩放+旋转+位移）投影到屏幕的 AABB，供 UiEvents 命中。
fn sprite_screen_rect(
    sprite: &Sprite,
    transform: &Transform,
    camera: OrthoTerms,
) -> Option<([f32; 2], [f32; 2])> {
    // 局部四角：(-pivot*size) 到 ((1-pivot)*size)，经缩放/旋转/位移后投影。
    let pivot = sprite.pivot;
    let size = sprite.size;
    let local = [
        Vec2::new(-pivot.x * size.x, -pivot.y * size.y),
        Vec2::new((1.0 - pivot.x) * size.x, -pivot.y * size.y),
        Vec2::new((1.0 - pivot.x) * size.x, (1.0 - pivot.y) * size.y),
        Vec2::new(-pivot.x * size.x, (1.0 - pivot.y) * size.y),
    ];
    let (sin, cos) = transform.rotation.sin_cos();
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    for corner in local {
        let scaled = Vec2::new(corner.x * transform.scale.x, corner.y * transform.scale.y);
        let rotated = Vec2::new(
            scaled.x * cos - scaled.y * sin,
            scaled.x * sin + scaled.y * cos,
        );
        let world = Vec2::new(
            transform.position.x + rotated.x,
            transform.position.y + rotated.y,
        );
        let screen = camera.world_to_screen(world);
        // 非有限的世界坐标不产出命中矩形（与旧的"投影失败即放弃"同一语义）。
        if !screen.x.is_finite() || !screen.y.is_finite() {
            return None;
        }
        min[0] = min[0].min(screen.x);
        min[1] = min[1].min(screen.y);
        max[0] = max[0].max(screen.x);
        max[1] = max[1].max(screen.y);
    }
    Some((min, [max[0] - min[0], max[1] - min[1]]))
}

fn load_runtime_scene(project_root: Option<&Path>) -> Scene {
    project_root
        .and_then(|root| {
            let scene_path = startup_scene_path(root);
            Scene::load(root.join(scene_path)).ok()
        })
        // Standalone engine mode uses this engine-owned diagnostic scene and never
        // probes the current directory for game files.
        .unwrap_or_else(|| {
            let mut scene = Scene::new("runtime-main");
            let _ = scene.add_entity(SceneEntityId(1), "SpriteRoot", None);
            scene
                .entities
                .get_mut(&SceneEntityId(1))
                .unwrap()
                .components
                .insert(
                    "Transform".into(),
                    "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
                );
            scene
                .entities
                .get_mut(&SceneEntityId(1))
                .unwrap()
                .components
                .insert("Sprite".into(), "assets/textures/red-1x1.png".into());
            scene
        })
}

/// UI 合批：合并相邻且同纹理、同 sdf 的 draw。
///
/// 各元素在 `vertices()` 中按顺序追加到同一个顶点缓冲，因此相邻 draw 的顶点区间
/// 是连续的——把 `count` 合并到前一条即可，渲染结果不变（同纹理同管线，绘制顺序
/// 保持）。这能把一排同纹理的 UI 方块/面板从 N 次 draw call 降为 1 次。
fn merge_adjacent_draws(draws: &mut Vec<SceneDraw>) {
    let mut out: Vec<SceneDraw> = Vec::with_capacity(draws.len());
    for draw in draws.drain(..) {
        if let Some(last) = out.last_mut()
            && last.texture == draw.texture
            && last.start + last.count == draw.start
            && last.sdf == draw.sdf
            && last.vector == draw.vector
            // docs/54 §4.3：**层必须一起比**。曾经漏了这一项：相邻两段纹理相同、顶点连续时会被
            // 并成一段，并沿用前一段的 `render_layer`——于是 ScreenUI 的几何悄悄变成 Overlay2D，
            // HUD 隔离（§8-⑤ 的像素门）在有显式显示模式后会失效。
            && last.render_layer == draw.render_layer
        {
            // 相邻 draw 顶点连续（start == last.start + last.count），直接并 count。
            last.count += draw.count;
            continue;
        }
        out.push(draw);
    }
    *draws = out;
}

/// Converts a pixel-space text mesh into NDC overlay vertices.
pub(crate) fn text_to_ndc(
    mesh: TextMesh,
    is_sdf: bool,
    width: u32,
    height: u32,
) -> Vec<OverlayVertex> {
    text_to_ndc_with_offset(mesh, is_sdf, [0.0, 0.0], width, height)
}

/// Converts a pixel-space text mesh into NDC overlay vertices with an extra
/// pixel offset (used to center a button label).
fn text_to_ndc_with_offset(
    mesh: TextMesh,
    is_sdf: bool,
    offset: [f32; 2],
    width: u32,
    height: u32,
) -> Vec<OverlayVertex> {
    // 混合渲染：bitmap 模式 sdf=2（Nearest + 阈值化），MSDF 模式 sdf=1（Linear + smoothstep）。
    let sdf_value: u32 = if mesh.bitmap { 2 } else { u32::from(is_sdf) };
    mesh.vertices
        .into_iter()
        .map(|vertex| OverlayVertex {
            position: [
                (vertex.position[0] + offset[0]) / width.max(1) as f32 * 2.0 - 1.0,
                1.0 - (vertex.position[1] + offset[1]) / height.max(1) as f32 * 2.0,
            ],
            uv: vertex.uv,
            color: vertex.color,
            sdf: sdf_value,
        })
        .collect()
}

/// Builds a quad in NDC from a top-left-origin screen-space rectangle, sampling
/// the white texture with the given flat color.
/// 6-vertex textured quad in NDC with a per-cell UV rect (nine-slice cell).
/// Winding/UV order mirrors the single-quad code in `push_image_quad`.
fn nine_slice_quad(
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    uv: [f32; 4],
    color: [f32; 4],
) -> [OverlayVertex; 6] {
    let (u0, v0, u1, v1) = (uv[0], uv[1], uv[2], uv[3]);
    [
        OverlayVertex {
            position: [x0, y0],
            uv: [u0, v0],
            color,
            sdf: 0,
        },
        OverlayVertex {
            position: [x1, y0],
            uv: [u1, v0],
            color,
            sdf: 0,
        },
        OverlayVertex {
            position: [x1, y1],
            uv: [u1, v1],
            color,
            sdf: 0,
        },
        OverlayVertex {
            position: [x0, y0],
            uv: [u0, v0],
            color,
            sdf: 0,
        },
        OverlayVertex {
            position: [x1, y1],
            uv: [u1, v1],
            color,
            sdf: 0,
        },
        OverlayVertex {
            position: [x0, y1],
            uv: [u0, v1],
            color,
            sdf: 0,
        },
    ]
}

fn screen_rect_ndc(
    x: f32,
    y: f32,
    rect_w: f32,
    rect_h: f32,
    color: [f32; 4],
    screen_width: u32,
    screen_height: u32,
) -> [OverlayVertex; 6] {
    let sw = screen_width.max(1) as f32;
    let sh = screen_height.max(1) as f32;
    let x0 = x / sw * 2.0 - 1.0;
    let y0 = 1.0 - y / sh * 2.0;
    let x1 = (x + rect_w) / sw * 2.0 - 1.0;
    let y1 = 1.0 - (y + rect_h) / sh * 2.0;
    let uv = [0.0, 0.0];
    [
        OverlayVertex {
            position: [x0, y0],
            uv,
            color,
            sdf: 0,
        },
        OverlayVertex {
            position: [x1, y0],
            uv,
            color,
            sdf: 0,
        },
        OverlayVertex {
            position: [x1, y1],
            uv,
            color,
            sdf: 0,
        },
        OverlayVertex {
            position: [x0, y0],
            uv,
            color,
            sdf: 0,
        },
        OverlayVertex {
            position: [x1, y1],
            uv,
            color,
            sdf: 0,
        },
        OverlayVertex {
            position: [x0, y1],
            uv,
            color,
            sdf: 0,
        },
    ]
}

#[cfg(test)]
mod tilemap_chunk_tests {
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
}

#[cfg(test)]
mod sdf_tests {
    use super::*;
    use crate::core::camera::ledger;
    use crate::core::scene::Scene;

    // 示例重组后，展示工程从 examples/ 迁入 examples/game_api/（多项目布局）；
    // 此处路径同步修正，恢复 showcase/ui_test 等测试的真实覆盖（M18-03）。
    fn game_root() -> std::path::PathBuf {
        crate::core::games_dir().join("examples").join("game_api")
    }

    // 示例工程"逐场景实例化 + 步进"的门已迁到 `host::sample_project`
    // （`sample_project_runs_every_scene_headless`）：它**从 scenes/ 目录读**场景清单，
    // 因此新增/改名场景不再需要在这里手抄一份名单（名单漂移过：M18-03 与示例工程重建都踩过）。
    // 那一组门还一并覆盖脚本编译、API 覆盖率、组件覆盖率与入口菜单接线。

    fn production_sample_root() -> std::path::PathBuf {
        crate::core::games_dir().join("examples").join("game_api")
    }

    #[test]
    fn example_project_runs_with_new_api() {
        let root = crate::core::games_dir().join("examples").join("game_api");
        let mut runtime = RuntimeScene::new(Some(&root)).unwrap();
        runtime
            .run_script_steps(2, 1.0 / 60.0, &crate::core::input::InputSnapshot::default())
            .unwrap();
    }

    /// P4：`lua_ms` 指标真的被填了。此前 `begin_lua/end_lua` 只有单测在调，
    /// 真实运行的 `[perf] lua_ms=` 恒为 `0.000`——一条看着在工作、其实永远为零的指标。
    #[test]
    fn script_update_time_is_measured_and_taken_once() {
        let root = crate::core::games_dir().join("examples").join("game_api");
        let mut runtime = RuntimeScene::new(Some(&root)).unwrap();
        runtime
            .run_script_steps(2, 1.0 / 60.0, &crate::core::input::InputSnapshot::default())
            .unwrap();
        let micros = runtime.take_script_micros();
        assert!(micros > 0, "Lua update 耗时必须被测量到，得到 {micros}µs");
        // 取走即清零：0 步帧不该重复计入上一帧的耗时。
        assert_eq!(runtime.take_script_micros(), 0);
    }

    /// ui_test 示例场景：渲染出 UI/世界 draw、UiEvents 目标存在，且点击
    /// StartButton 会经注入的 on_click 分发（update 返回带实体名）。
    #[test]
    fn ui_test_scene_renders_ui_and_dispatches_clicks() {
        use crate::core::input::MouseButton;
        use crate::core::scene::Scene;

        let root = crate::core::games_dir().join("examples").join("game_api");
        let path = root.join("scenes/ui_test.scene");
        assert!(path.is_file(), "ui_test 示例场景缺失: {}", path.display());
        let scene = Scene::load(&path).unwrap();
        let mut runtime = RuntimeScene::from_scene(Some(&root), scene).unwrap();
        let mesh = runtime.vertices(1280, 720).expect("vertices");
        assert!(!mesh.draws.is_empty(), "ui_test 应渲染出可见 draw");
        let targets = runtime.compute_click_targets();
        assert!(
            targets.len() >= 2,
            "ui_test 应有多个 UiEvents 目标: {targets:#?}"
        );
        let target = targets
            .iter()
            .find(|target| target.name == "StartButton")
            .expect("StartButton UiEvents 目标");
        let script_path =
            std::env::temp_dir().join(format!("rustlua-ui-test-click-{}.lua", std::process::id()));
        std::fs::write(
            &script_path,
            "function on_click(name) clicked = name end\nfunction update() return tostring(clicked or 'nil') end",
        )
        .unwrap();
        runtime.script.execute_project_script(&script_path).unwrap();
        let cx = target.origin[0] + target.size[0] / 2.0;
        let cy = target.origin[1] + target.size[1] / 2.0;
        let mut input = InputSnapshot::default();
        input.cursor_moved(cx as f64, cy as f64);
        input.mouse_event(MouseButton::Left, true);
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("script step");
        let result = runtime.script.call_update().unwrap();
        assert!(
            result.contains("StartButton"),
            "点击应分发 on_click('StartButton'): {result}"
        );
        let _ = std::fs::remove_file(script_path);
    }

    /// 九宫格：带 `nine_slice` 的 Image 应渲染为 3×3 共 9 格（54 顶点），而非
    /// 单四边形（6 顶点）。用真实按钮纹理（192×64）验证 UV 分片路径。
    #[test]
    fn nine_slice_image_renders_nine_quads() {
        use crate::core::scene::{Scene, SceneEntityId};

        let root = crate::core::games_dir().join("examples").join("game_api");
        if !root.join("assets/textures/ui/button_green.png").is_file() {
            return;
        }
        let mut scene = Scene::new("nine-slice");
        scene.add_entity(SceneEntityId(1), "Panel", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Image".to_owned(),
                "texture=assets/textures/ui/button_green.png;color=1,1,1;nine_slice=16,16,16,16"
                    .to_owned(),
            );
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "UiLayout".to_owned(),
                "anchors=0,0,0,0;offsets=0,0,192,64".to_owned(),
            );
        let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).unwrap();
        let mesh = runtime.vertices(1280, 720).expect("vertices");
        assert!(
            mesh.draws.iter().any(|d| d.count == 54),
            "切片 Image 应产生 54 顶点（9 格 × 6）：{:#?}",
            mesh.draws.iter().map(|d| d.count).collect::<Vec<_>>()
        );
    }

    /// Scene `Text` layouts already contain absolute screen coordinates. A
    /// regression used to add the `UiLayout` origin a second time, pushing
    /// labels below/off their buttons.
    #[test]
    fn scene_ui_text_keeps_layout_origin_once() {
        use crate::core::scene::{Scene, SceneEntityId};

        let font_source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../editor/fonts/NotoSansSC-VF.ttf");
        if !font_source.is_file() {
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "rustlua-scene-text-origin-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let fonts_dir = root.join("assets").join("fonts");
        fs::create_dir_all(&fonts_dir).unwrap();
        fs::copy(&font_source, fonts_dir.join("NotoSansSC-VF.ttf")).unwrap();

        let mut scene = Scene::new("scene-text-origin");
        scene.add_entity(SceneEntityId(1), "Label", None).unwrap();
        let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        entity.components.insert(
            "UiLayout".to_owned(),
            "anchors=0,0,0,0;offsets=100,600,100,600".to_owned(),
        );
        entity.components.insert(
            "Text".to_owned(),
            "text=Press;font_size=48;color=1,1,1".to_owned(),
        );
        let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).unwrap();
        let mesh = runtime.vertices(1280, 720).expect("vertices");
        let _ = fs::remove_dir_all(&root);
        let text_y: Vec<f32> = mesh
            .vector_vertices
            .iter()
            .map(|vertex| vertex.position[1])
            .collect();
        assert!(!text_y.is_empty(), "scene text should emit vector vertices");
        let minimum_y = text_y.iter().copied().fold(f32::INFINITY, f32::min);
        assert!(
            minimum_y > -1.0,
            "layout text must stay on screen, min NDC y={minimum_y}"
        );
    }

    /// Runtime UI layout nodes are keyed by visible-entity order, not by the
    /// sparse authoring ID. Dragging entity 2 must never rewrite entity 1.
    #[test]
    fn ui_layout_preview_targets_authored_entity() {
        use crate::core::scene::{Scene, SceneEntityId};

        let mut scene = Scene::new("drag-preview");
        for id in [1, 2] {
            scene
                .add_entity(SceneEntityId(id), format!("Entity{id}"), None)
                .unwrap();
            let entity = scene.entities.get_mut(&SceneEntityId(id)).unwrap();
            entity.components.insert(
                "Text".to_owned(),
                format!("text=Text{id};font_size=24").to_owned(),
            );
            entity.components.insert(
                "UiLayout".to_owned(),
                format!("anchors=0,0,0,0;offsets={id}0,{id}0,{id}0,{id}0"),
            );
        }
        let mut runtime = RuntimeScene::from_scene_edit(None, scene).unwrap();
        assert!(
            runtime.set_scene_entity_ui_layout_preview(SceneEntityId(2), [50.0, 60.0, 70.0, 80.0])
        );
        let first = runtime.ui_scene_nodes[&0].layout.as_ref().unwrap().offsets;
        let second = runtime.ui_scene_nodes[&1].layout.as_ref().unwrap().offsets;
        assert_eq!(first, [10.0, 10.0, 10.0, 10.0]);
        assert_eq!(second, [50.0, 60.0, 70.0, 80.0]);
    }

    #[test]
    fn ui_layout_rects_expose_exact_preview_geometry() {
        use crate::core::scene::SceneEntityId;

        let mut scene = Scene::new("drag-preview-geometry");
        scene.add_entity(SceneEntityId(1), "Image", None).unwrap();
        let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        entity.components.insert(
            "Image".to_owned(),
            "texture=__white__;color=1,1,1".to_owned(),
        );
        entity.components.insert(
            "UiLayout".to_owned(),
            "anchors=0,0,0,0;offsets=10,20,30,40".to_owned(),
        );
        let mut runtime = RuntimeScene::from_scene_edit(None, scene).unwrap();
        assert!(
            runtime.set_scene_entity_ui_layout_preview(SceneEntityId(1), [50.0, 60.0, 90.0, 100.0])
        );
        let rects = runtime.scene_entity_ui_layout_rects([SceneEntityId(1)], [1000.0, 1000.0]);
        assert_eq!(rects.get(&1), Some(&UiRect::new(50.0, 60.0, 40.0, 40.0)));
    }

    /// The editor drag preview must affect the next rendered mesh, not only the
    /// internal layout snapshot.
    #[test]
    fn ui_layout_preview_changes_rendered_mesh() {
        use crate::core::scene::{Scene, SceneEntityId};

        let mut scene = Scene::new("drag-preview-render");
        for id in [1, 2] {
            scene
                .add_entity(SceneEntityId(id), format!("Entity{id}"), None)
                .unwrap();
            let entity = scene.entities.get_mut(&SceneEntityId(id)).unwrap();
            entity.components.insert(
                "Image".to_owned(),
                "texture=__white__;color=1,1,1".to_owned(),
            );
            entity.components.insert(
                "UiLayout".to_owned(),
                format!("anchors=0,0,0,0;offsets={id}0,{id}0,{id}0,{id}0"),
            );
        }
        let mut runtime = RuntimeScene::from_scene_edit(None, scene).unwrap();
        runtime.set_render_ui_overlay(true);
        let before = runtime.vertices(1000, 1000).unwrap();
        assert!(
            runtime
                .set_scene_entity_ui_layout_preview(SceneEntityId(2), [700.0, 700.0, 740.0, 740.0])
        );
        let after = runtime.vertices(1000, 1000).unwrap();
        let before_x = before
            .vertices
            .iter()
            .map(|vertex| vertex.position[0])
            .fold(f32::NEG_INFINITY, f32::max);
        let after_x = after
            .vertices
            .iter()
            .map(|vertex| vertex.position[0])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(after_x > before_x);
    }

    /// 首包预热：`load_project_font_atlas` 读到项目根 `chars.txt` 时预生成字形
    /// （图集 version 增加）；无 chars.txt 不预热。
    #[test]
    fn font_prewarm_from_chars_txt() {
        let root =
            std::env::temp_dir().join(format!("rustlua_font_prewarm_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let font_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/editor/fonts/NotoSansSC-VF.ttf");
        std::fs::create_dir_all(root.join("assets/fonts")).unwrap();
        std::fs::copy(&font_src, root.join("assets/fonts/font.ttf")).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "[project]\nfont = \"assets/fonts/font.ttf\"\n",
        )
        .unwrap();
        // 无 chars.txt：不预热，version 保持 0。
        let atlas = load_project_font_atlas(Some(&root)).expect("atlas");
        assert_eq!(atlas.lock().unwrap().version(), 0, "无 chars.txt 不应预热");
        // 写 chars.txt：预热后 version > 0（字形已生成）。
        std::fs::write(root.join("chars.txt"), "测试字体AB").unwrap();
        let atlas = load_project_font_atlas(Some(&root)).expect("atlas");
        assert!(atlas.lock().unwrap().version() > 0, "chars.txt 应触发预热");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 标题场景（`main.scene`）的手柄导航在**真实 RuntimeScene 路径**上可用。
    ///
    /// 编辑器 Play 预览走的就是这条路径（每帧全新键鼠快照 + `run_script_steps` 合并手柄），
    /// 而 M20-08 的验收示例是直接驱动 `LuaRuntime`，覆盖不到 `input_apply` 这一步。
    #[test]
    fn title_scene_gamepad_navigation_works_through_runtime_scene() {
        use crate::host::gamepad::{GamepadFrame, SlotBackend, button};

        let root = crate::core::games_dir().join("examples").join("game_jhyt");
        let scene_path = root.join("scenes/main.scene");
        if !scene_path.is_file() {
            return;
        }
        let scene = Scene::load(&scene_path).expect("load main.scene");
        let mut runtime =
            RuntimeScene::from_scene(Some(&root), scene).expect("runtime from main.scene");

        let idle = GamepadFrame::idle();
        let mut dpad_down = GamepadFrame::idle();
        dpad_down.buttons[button::DPAD_DOWN] = true;
        // ① 空闲帧（建立设备基线）→ ② 起按住 DPad 下。
        runtime.set_gamepad_backend(Box::new(SlotBackend::new(vec![
            [Some(idle), None, None, None],
            [Some(dpad_down), None, None, None],
        ])));

        let probe = |runtime: &mut RuntimeScene| -> String {
            runtime
                .script
                .execute_console("return tostring(require('ui.title_menu').selected_id())")
                .expect("probe title menu")
                .trim()
                .to_owned()
        };

        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect("idle step");
        let before = probe(&mut runtime);
        assert_ne!(before, "nil", "标题菜单应在 main.scene 上初始化");
        for _ in 0..3 {
            runtime
                .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
                .expect("dpad step");
        }
        let after = probe(&mut runtime);
        println!("title selected: before={before} after={after}");
        assert_ne!(after, before, "DPad 下应改变标题菜单选中项");
    }

    /// 标题场景（`main.scene`）的**摇杆**菜单导航在真实 RuntimeScene 路径上可用。
    ///
    /// 项目 `settings.toml` 把左摇杆绑给了 `move_*`（实机测试用），`ui_*` 未列在 `map` 里
    /// → `load_settings` 用默认绑定补齐（含 `padaxis:1±`），因此摇杆仍能选菜单。这条路径
    /// 依赖 **`ui_*` 的轴绑定 + 轴边沿**（M22/W3），与 M20-08 的「Lua 直接读 `pad_axis`」不同。
    #[test]
    fn title_scene_stick_navigation_works_through_runtime_scene() {
        use crate::host::gamepad::{GamepadFrame, SlotBackend};

        let root = crate::core::games_dir().join("examples").join("game_jhyt");
        let scene_path = root.join("scenes/main.scene");
        if !scene_path.is_file() {
            return;
        }
        let scene = Scene::load(&scene_path).expect("load main.scene");
        let mut runtime =
            RuntimeScene::from_scene(Some(&root), scene).expect("runtime from main.scene");

        let mut stick_down = GamepadFrame::idle();
        stick_down.axes[crate::host::gamepad::axis::LEFT_Y] = -0.9;
        runtime.set_gamepad_backend(Box::new(SlotBackend::new(vec![
            [Some(GamepadFrame::idle()), None, None, None],
            [Some(stick_down), None, None, None],
        ])));

        let probe = |runtime: &mut RuntimeScene| -> String {
            runtime
                .script
                .execute_console("return tostring(require('ui.title_menu').selected_id())")
                .expect("probe title menu")
                .trim()
                .to_owned()
        };

        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect("idle step");
        let before = probe(&mut runtime);
        for _ in 0..3 {
            runtime
                .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
                .expect("stick step");
        }
        let after = probe(&mut runtime);
        println!("title selected (stick): before={before} after={after}");
        assert_ne!(after, before, "摇杆下推应改变标题菜单选中项");
    }

    /// 编辑器 Play 预览每帧重建输入快照（`bridge_preview_input` 只桥接键鼠），
    /// 而独立 game 宿主跨帧复用同一快照。设备在被按住期间持续上报同一轴值时，
    /// 运行时必须把**当前**手柄状态写进本帧输入，而不是只写「与上一帧不同」的轴——
    /// 否则编辑器预览里摇杆推住不动就等于没有输入（回归：编辑器内摇杆无反应）。
    #[test]
    fn held_gamepad_stick_reaches_script_with_fresh_input_snapshot_per_frame() {
        /// 恒定设备帧：始终「按住」左摇杆上推（值不变 → 无变化边沿）。
        struct HeldStickBackend {
            frame: crate::host::gamepad::GamepadFrame,
        }

        impl crate::host::gamepad::GamepadBackend for HeldStickBackend {
            fn poll(&mut self) -> crate::host::gamepad::GamepadFrame {
                self.frame
            }

            fn name(&self) -> &'static str {
                "held-stick"
            }
        }

        let root = std::env::temp_dir().join(format!(
            "rustlua-preview-gamepad-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let scripts = root.join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(
            scripts.join("main.lua"),
            "args = {}\n\
             function update()\n\
               local s = input.pad_axis(1)\n\
               if type(s) == 'number' and s == s and s ~= 0 then args[#args + 1] = s end\n\
               return ''\n\
             end\n",
        )
        .unwrap();

        let mut runtime = RuntimeScene::from_scene(Some(&root), Scene::new("preview-gamepad"))
            .expect("runtime scene");
        let mut frame = crate::host::gamepad::GamepadFrame::idle();
        frame.axes[crate::host::gamepad::axis::LEFT_Y] = 0.8;
        runtime.set_gamepad_backend(Box::new(HeldStickBackend { frame }));

        // 与编辑器预览逐帧一致：每帧都是全新快照（不跨帧保留手柄状态）。
        for _ in 0..3 {
            runtime
                .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
                .expect("script step");
        }
        let sampled = runtime
            .script
            .execute_console("return tostring(#args) .. '|' .. tostring(args[1])")
            .expect("probe lua");
        let (count, value) = sampled.trim().split_once('|').expect("count|value");
        assert_eq!(
            count, "3",
            "每帧都应读到被按住摇杆的轴值（编辑器预览逐帧重建快照）"
        );
        let value: f32 = value.parse().expect("axis value");
        assert!(
            (value - 0.8).abs() < 1e-3,
            "轴值应保持设备上报值 0.8：{value}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 触屏虚拟键（`input.virtual_key`，`ui/virtual_pad.lua` 的摇杆与虚拟按钮）
    /// 必须**跨帧保持**，因为宿主每帧都用设备层重新采样的快照整份替换脚本输入。
    ///
    /// 回归（实机症状）：摇杆头能动、`stick=true`，但 `input.action_axis('move_left',
    /// 'move_right')` 恒为 `0.00`，角色不动——虚拟键在帧内注入后，被下一帧的
    /// `set_input` 整份替换清空，`update_movement` 永远读到 0。
    #[test]
    fn virtual_pad_key_injection_survives_fresh_input_snapshot_per_frame() {
        let root = std::env::temp_dir().join(format!(
            "rustlua-virtual-pad-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let scripts = root.join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        // 与触屏摇杆同构：按住期间每帧注入 `true`，抬起时注入 `false`。
        std::fs::write(
            scripts.join("main.lua"),
            "sticks = {}\n\
             held = false\n\
             function hold() held = true end\n\
             function release() held = false end\n\
             function update()\n\
               local before = input.action_axis('move_down', 'move_up')\n\
               input.virtual_key('keyw', held)\n\
               local after = input.action_axis('move_down', 'move_up')\n\
               if held then sticks[#sticks + 1] = tostring(before) .. '/' .. tostring(after) end\n\
               return ''\n\
             end\n",
        )
        .unwrap();

        let mut runtime = RuntimeScene::from_scene(Some(&root), Scene::new("virtual-pad"))
            .expect("runtime scene");
        // 与 `game_app.rs` / 编辑器预览逐帧一致：宿主持有一份**自己的**快照，每帧 clone
        // 后交给运行时；脚本对虚拟键的注入只落在运行时那一份克隆里，下一帧又被宿主的
        // 快照整份覆盖——这正是实机上「摇杆能动、角色不动」的帧边界。
        let mut host_input = InputSnapshot::default();
        let step = |runtime: &mut RuntimeScene, host_input: &mut InputSnapshot| {
            let frame = host_input.clone();
            runtime
                .run_script_steps(1, 1.0 / 60.0, &frame)
                .expect("script step");
            // 宿主侧只推进边沿/增量（`game_app.rs` 帧末），按住态由设备层重新采样。
            host_input.finish_frame();
        };
        step(&mut runtime, &mut host_input);
        runtime
            .script
            .execute_console("hold() return ''")
            .expect("hold stick");
        for _ in 0..3 {
            step(&mut runtime, &mut host_input);
        }
        let held = runtime
            .script
            .execute_console("return tostring(#sticks) .. '|' .. tostring(sticks[1]) .. '|' .. tostring(sticks[3])")
            .expect("probe held");
        runtime
            .script
            .execute_console("release() return ''")
            .expect("release stick");
        step(&mut runtime, &mut host_input);
        let released = runtime
            .script
            .execute_console("return tostring(#sticks)")
            .expect("probe released");
        let (count, first, last) = {
            let probe = held.trim().to_owned();
            let mut parts = probe.split('|');
            (
                parts.next().unwrap_or_default().to_owned(),
                parts.next().unwrap_or_default().to_owned(),
                parts.next().unwrap_or_default().to_owned(),
            )
        };
        // `before/after`：注入前的轴值 / 注入后的轴值。首帧注入前为 0（此前没有任何注入），
        // 但**第二帧起**注入前必须已是 +1——虚拟键要跨帧保持住，否则 `update_movement`
        // 在下一帧只会读到 0（实机「摇杆能动、角色不动」）。
        assert_eq!(count, "3", "按住期间每帧都应读到虚拟键轴值");
        assert_eq!(first, "0.0/1.0", "首帧：注入前无虚拟键、注入后立即可读");
        assert_eq!(
            last, "1.0/1.0",
            "第二帧起：注入前虚拟键必须仍被按住（跨帧保持）"
        );
        assert_eq!(released, "3", "释放后不再计入按住帧");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// M22/W4：**0 步帧不得吞掉手柄边沿**。
    ///
    /// 宿主侧键鼠边沿只在 `steps > 0` 时被 `finish_frame` 清掉（`game_app.rs`），所以 0 步帧
    /// （帧率高于逻辑步，关 vsync 时常见）里发生的按键会保留到下一有效帧。手柄此前在 0 步帧
    /// 也会 poll：边沿被写进那个随后丢弃的快照克隆，而 `poller.last` 已推进 → 下一帧不再产生
    /// 同一变化，边沿**永久丢失**。修复后 0 步帧不采样，两条路径语义一致。
    #[test]
    fn zero_step_frame_does_not_drop_gamepad_edges() {
        use crate::host::gamepad::{FakeBackend, GamepadFrame, button};

        let root = std::env::temp_dir().join(format!(
            "rustlua-zero-step-gamepad-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let scripts = root.join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(
            scripts.join("main.lua"),
            "presses = 0\n\
             function update()\n\
               if input.action_pressed('confirm') then presses = presses + 1 end\n\
               return ''\n\
             end\n",
        )
        .unwrap();

        let mut runtime = RuntimeScene::from_scene(Some(&root), Scene::new("zero-step-gamepad"))
            .expect("runtime scene");
        // 设备从第一帧起就按住 A：唯一一次按下边沿必须被某个**有效**帧看到。
        let mut held = GamepadFrame::idle();
        held.buttons[button::A] = true;
        runtime.set_gamepad_backend(Box::new(FakeBackend::new(vec![held])));

        // 0 步帧（此前会在这里 poll 并丢掉边沿）。
        runtime
            .run_script_steps(0, 1.0 / 60.0, &InputSnapshot::default())
            .expect("zero-step frame");
        // 第一个有效帧：应看到按下边沿。
        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect("first script step");
        // 第二个有效帧：按住不重复边沿。
        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect("second script step");

        let count = runtime
            .script
            .execute_console("return tostring(presses)")
            .expect("probe lua");
        assert_eq!(
            count.trim(),
            "1",
            "0 步帧不得吞掉手柄按下边沿；按住也不得重复边沿"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// M22/W2：Lua `input.rumble` 的请求在帧末到达设备层（含槽位与强度）。
    #[test]
    fn lua_rumble_reaches_device_layer() {
        use crate::host::gamepad::{GamepadFrame, SlotBackend};

        let root = std::env::temp_dir().join(format!(
            "rustlua-rumble-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let scripts = root.join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(
            scripts.join("main.lua"),
            "function update()\n\
               input.rumble(0.4, 0.6, 120)\n\
               return ''\n\
             end\n",
        )
        .unwrap();

        let mut runtime =
            RuntimeScene::from_scene(Some(&root), Scene::new("rumble")).expect("runtime scene");
        let backend = SlotBackend::new(vec![[Some(GamepadFrame::idle()), None, None, None]]);
        let log = backend.rumble_log();
        runtime.set_gamepad_backend(Box::new(backend));

        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect("script step");
        assert_eq!(
            log.calls().last().copied(),
            Some((0, 0.4, 0.6)),
            "Lua 震动请求应到达设备层"
        );
        // 请求取走后不重复触发（下一帧没有新请求）。
        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect("script step");
        assert_eq!(log.calls().len(), 2, "每帧一次请求，不做额外重发");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 俄罗斯方块工程冒烟：加载场景 + 脚本，跑几步（输入驱动），验证精灵/UI
    /// 渲染与 Lua 脚本不报错。
    #[test]
    fn tetris_project_runs_and_renders() {
        use crate::core::input::KeyCode;
        use crate::core::scene::Scene;

        let root = crate::core::fixture_games_dir().join("tetris");
        if !root.join("scenes/main.scene").is_file() {
            return;
        }
        let path = root.join("scenes/main.scene");
        assert!(path.is_file(), "tetris 场景缺失: {}", path.display());
        assert!(root.join("scripts/main.lua").is_file(), "tetris 脚本缺失");
        let scene = Scene::load(&path).unwrap();
        let mut runtime = RuntimeScene::from_scene(Some(&root), scene).unwrap();
        // on_create 已执行：背景面板 + 首块精灵 + 文字应渲染出 draw。
        let mesh = runtime.vertices(1280, 720).expect("vertices");
        assert!(
            !mesh.draws.is_empty(),
            "tetris 应渲染出 draw（UI 面板/精灵/文字）"
        );
        // 关键：必须存在 white.png 的精灵 draw（方块），否则只渲染了 UI。
        let has_sprite_draw = mesh.draws.iter().any(|d| {
            d.texture > 0
                && mesh.paths.get(d.texture as usize - 1).map(|p| p.as_str())
                    == Some("assets/textures/white.png")
        });
        assert!(
            has_sprite_draw,
            "应存在 white.png 精灵 draw（方块）：{:#?}",
            mesh.paths
        );
        // UI 合批：全部方块用同一纹理（white.png），应被合并成极少量 draw。
        let white_draws = mesh
            .draws
            .iter()
            .filter(|d| {
                d.texture > 0
                    && mesh.paths.get(d.texture as usize - 1).map(|p| p.as_str())
                        == Some("assets/textures/white.png")
            })
            .count();
        assert!(
            white_draws <= 2,
            "同纹理 UI 方块应合批为 ≤2 个 draw（初始 4 块），实际 {white_draws}"
        );
        // 模拟按键：右移 → 旋转 → 硬降，逐帧驱动。
        let mut input = InputSnapshot::default();
        input.key_event(KeyCode::KeyD, true);
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step d");
        input.key_event(KeyCode::KeyW, true);
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step w");
        input.key_event(KeyCode::Space, true);
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step space");
        // 多步后仍能渲染。
        let mesh = runtime.vertices(1280, 720).expect("vertices after steps");
        assert!(!mesh.draws.is_empty(), "多步后仍应渲染");
    }

    /// UiEvents 命中矩形：Image 用 UI 矩形、Text 用 SDF 字形 bbox、Sprite 用
    /// 相机投影到屏幕的 AABB。验证三类实体都能得到正确的 `(名字, origin, size)`。
    #[test]
    fn compute_click_targets_resolves_image_text_sprite() {
        use crate::core::scene::{Scene, SceneEntityId};

        let root = crate::core::games_dir().join("examples");
        let mut scene = Scene::new("click");
        let add = |scene: &mut Scene, id: u64, name: &str, components: &[(&str, &str)]| {
            scene.add_entity(SceneEntityId(id), name, None).unwrap();
            let entity = scene.entities.get_mut(&SceneEntityId(id)).unwrap();
            for (key, value) in components {
                entity.components.insert(key.to_string(), value.to_string());
            }
        };
        add(
            &mut scene,
            1,
            "ImgClick",
            &[
                ("Image", "texture=assets/textures/coin.png;color=1,1,1"),
                ("UiLayout", "anchors=0,0,0,0;offsets=10,20,74,52"),
                ("UiEvents", "event=click;function=on_click"),
            ],
        );
        add(
            &mut scene,
            2,
            "TxtClick",
            &[
                ("Text", "text=Go;font_size=24;color=1,1,1"),
                ("UiLayout", "anchors=0,0,0,0;offsets=5,5,5,5"),
                ("UiEvents", "event=click;function=on_click"),
            ],
        );
        add(
            &mut scene,
            3,
            "SprClick",
            &[
                (
                    "Transform",
                    "position=100,50,0;rotation=0,0,0,1;scale=1,1,1",
                ),
                (
                    "Sprite",
                    "texture=assets/textures/coin.png;size=32x32;pivot=0.5,0.5;tint=1,1,1,1",
                ),
                ("UiEvents", "event=click;function=on_click"),
            ],
        );
        let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).unwrap();
        runtime.set_camera_2d(Camera2D::new(Vec2::new(0.0, 0.0), 1.0).expect("default camera"));
        let targets = runtime.compute_click_targets();
        let img = targets
            .iter()
            .find(|target| target.name == "ImgClick")
            .expect("image click target");
        assert_eq!(img.origin, [10.0, 20.0], "Image origin 来自 UiLayout");
        assert_eq!(img.size, [64.0, 32.0], "Image size 来自 UiLayout");
        assert!(
            targets.iter().any(|target| target.name == "TxtClick"),
            "Text click target 存在"
        );
        let spr = targets
            .iter()
            .find(|target| target.name == "SprClick")
            .expect("sprite click target");
        // 相机在原点、viewport 1280×720：sprite(100,50) 32×32 pivot 0.5
        // → 屏幕 AABB 左上 (724, 294)、尺寸 (32, 32)。
        assert_eq!(spr.origin, [724.0, 294.0]);
        assert_eq!(spr.size, [32.0, 32.0]);
    }

    /// E16：命中矩形完全来自四边 Anchor+Offset 布局；Image 只携带视觉属性。
    #[test]
    fn ui_layout_component_defines_click_rect() {
        use crate::core::scene::{Scene, SceneEntityId};

        let root = crate::core::games_dir().join("examples");
        let mut scene = Scene::new("layout");
        scene
            .add_entity(SceneEntityId(1), "StretchImg", None)
            .unwrap();
        let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
        entity.components.insert(
            "Image".to_owned(),
            "texture=assets/textures/coin.png".to_owned(),
        );
        entity.components.insert(
            "UiLayout".to_owned(),
            crate::core::ui_layout::UiRectLayout {
                anchors: [0.0, 0.0, 1.0, 0.0],
                offsets: [10.0, 20.0, -10.0, 80.0],
                ..crate::core::ui_layout::UiRectLayout::default()
            }
            .to_component_str(),
        );
        entity.components.insert(
            "UiEvents".to_owned(),
            crate::core::ui_events::UiEvents::click("on_click").to_component_str(),
        );
        let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).unwrap();
        let targets = runtime.compute_click_targets();
        let img = targets
            .iter()
            .find(|target| target.name == "StretchImg")
            .expect("layout click target");
        // viewport 1280×720：stretch 水平 → (10, 20, 1260, 60)。
        assert_eq!(img.origin, [10.0, 20.0]);
        assert_eq!(img.size, [1260.0, 60.0]);
    }

    #[test]
    fn load_runtime_scene_uses_project_startup_scene() {
        let root =
            std::env::temp_dir().join(format!("rustlua-startup-scene-{}", std::process::id()));
        let scenes = root.join("scenes");
        std::fs::create_dir_all(&scenes).unwrap();
        std::fs::write(
            root.join("project.toml"),
            "[project]\nname = \"test\"\nstartup_scene = \"scenes/custom.scene\"\n",
        )
        .unwrap();
        Scene::new("custom-startup")
            .save(scenes.join("custom.scene"))
            .unwrap();
        let scene = load_runtime_scene(Some(&root));
        assert_eq!(scene.name, "custom-startup");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn ui_test_scene_emits_sdf_text_draws() {
        let root = game_root();
        if !root.join("scenes/ui_test.scene").is_file() {
            return;
        }
        let path = root.join("scenes/ui_test.scene");
        assert!(path.is_file(), "测试场景资源缺失: {}", path.display());
        let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load ui_test: {e}"));
        let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime");
        // 本用例断言的是 **SDF 字形**，因此自己钉住渲染模式，不继承工程配置
        // （`game_api/project.toml` 现为 `text_render_mode = "vector"`）。
        runtime.set_text_render_mode(TextRenderMode::Sdf);
        let mesh = runtime.vertices(640, 360).expect("vertices");
        let sdf_count = mesh.draws.iter().filter(|d| d.sdf).count();
        eprintln!(
            "ui_test: draws={} sdf={} text_entities={}",
            mesh.draws.len(),
            sdf_count,
            runtime
                .script
                .with_world(|w| w.entities_with::<UiText>().len())
        );
        eprintln!(
            "ui_test: draws={} sdf={} text_entities={}",
            mesh.draws.len(),
            sdf_count,
            runtime
                .script
                .with_world(|w| w.entities_with::<UiText>().len())
        );
        assert!(sdf_count > 0, "ui_test scene should emit SDF text draws");
        assert!(
            mesh.vertices.iter().any(|v| v.sdf == 1),
            "SDF vertices present"
        );
    }

    /// 世界文字（WP4）：锚点跟随实体的世界位置与相机。
    ///
    /// 场景里只有 WorldText 实体，因此产出的顶点全部来自文字；相机右移 100 世界单位后，
    /// 同一个世界锚点必须投影到更靠左的屏幕位置。
    #[test]
    fn world_text_anchors_to_world_position_and_follows_camera() {
        let mut scene = Scene::new("world_text");
        scene.add_entity(SceneEntityId(1), "Label", None).unwrap();
        {
            let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
            entity.components.insert(
                "Transform".into(),
                "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
            entity.components.insert(
                "WorldText".into(),
                "text=HP;font_size=24;color=1,1,1;offset=0,-20;centered=true".into(),
            );
        }
        let mut runtime = RuntimeScene::from_scene_edit(None, scene).expect("runtime");
        let base = Camera2D::new(Vec2::new(320.0, 180.0), 1.0).expect("camera");
        runtime.set_camera_2d(base);
        let first = runtime.vertices(640, 360).expect("vertices");
        assert!(!first.vertices.is_empty(), "世界文字应产出顶点");

        runtime.set_camera_2d(Camera2D {
            center: Vec2::new(base.center.x + 100.0, base.center.y),
            ..base
        });
        let second = runtime.vertices(640, 360).expect("vertices");
        let leftmost = |mesh: &SceneMesh| {
            mesh.vertices
                .iter()
                .map(|vertex| vertex.position[0])
                .fold(f32::INFINITY, f32::min)
        };
        assert!(
            leftmost(&second) < leftmost(&first),
            "相机右移 100 世界单位后文字应左移：{} vs {}",
            leftmost(&first),
            leftmost(&second)
        );
    }

    /// 2D 相机跟随（WP4）：脚本登记跟随目标后，宿主每帧按控制器结果移动相机，并把真实值
    /// 回填给 `camera2d.get`（读到的是宿主在用的值，不是脚本的自述）。
    #[test]
    fn camera2d_controller_follows_scripted_target() {
        let mut scene = Scene::new("camera2d");
        scene.add_entity(SceneEntityId(1), "Player", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "position=400,200,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
        let mut runtime = RuntimeScene::from_scene_edit(None, scene).expect("runtime");
        let player = runtime
            .script
            .with_world(|world| world.entities_with::<Transform>().first().copied())
            .expect("player entity");
        runtime
            .script
            .camera2d_controller_mut(|controller| controller.follow(Some(player)));
        runtime.set_camera_2d(Camera2D::new(Vec2::zero(), 1.0).expect("camera"));

        let input = InputSnapshot::default();
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("one frame");
        let center = runtime.camera_center_2d();
        assert!(
            (center.x - 400.0).abs() < 1e-3 && (center.y - 200.0).abs() < 1e-3,
            "无平滑时相机应在第一帧直接到位：{center:?}"
        );
        // 回填一致性：Lua 侧 `camera2d.get` 必须等于宿主实际中心。
        assert_eq!(
            runtime.script.camera2d_controller(|c| c.center()),
            center,
            "camera2d.get 应读到宿主真实值"
        );

        // 平滑跟随：目标移动后不应一步到位。
        runtime
            .script
            .camera2d_controller_mut(|controller| controller.set_smoothing(6.0));
        runtime.script.with_world_mut(|world| {
            world
                .get_mut::<Transform>(player)
                .unwrap()
                .unwrap()
                .position = Vec2::new(600.0, 200.0);
        });
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("one frame");
        let smoothed = runtime.camera_center_2d();
        assert!(
            smoothed.x > 400.0 && smoothed.x < 600.0,
            "平滑跟随应逐步推进：{smoothed:?}"
        );
    }

    /// WP6 排序契约（"不写深度 + 按 z 排序"）：世界内容必须按**画家顺序**交给渲染器——
    /// 同层内远的先画、近的后画，且层的先后压过 z。这条钉的是"宿主把顺序排好"这件事：
    /// 渲染器不写深度，顺序错了就会画错（渲染器侧有自己的像素级测试）。
    #[test]
    fn world_channel_is_sorted_far_to_near_within_a_layer() {
        let root = game_root();
        let mut scene = crate::core::scene::Scene::new("world-sort");
        // 三个精灵：刻意让"插入顺序"与 z 顺序相反，并且给两个不同的排序层。
        let specs = [
            (1_u64, "Near", "-0.5", "default"),
            (2, "Far", "0.5", "default"),
            (3, "BackLayer", "0.9", "background"),
        ];
        for (id, name, z, layer) in specs {
            let entity_id = crate::core::scene::SceneEntityId(id);
            scene.add_entity(entity_id, name, None).unwrap();
            let entity = scene.entities.get_mut(&entity_id).unwrap();
            entity.components.insert(
                "Transform".into(),
                "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
            entity.components.insert(
                "Sprite".into(),
                format!(
                    "texture=assets/textures/player.png;size=16x16;pivot=0.5,0.5;\
                     sorting_layer={layer};z={z}"
                ),
            );
        }
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
        runtime.set_world_sprite_depth(true);
        let mesh = runtime.vertices(640, 360).expect("world vertices");
        assert_eq!(mesh.world_sprites.len(), 18, "三个精灵各 6 个顶点");

        // 每个四边形第一个顶点的 z，按绘制顺序列出。
        let quad_starts: Vec<usize> = mesh
            .world_sprite_draws
            .iter()
            .flat_map(|draw| {
                (0..draw.vertex_count / 6).map(move |quad| draw.vertex_start + quad * 6)
            })
            .collect();
        let quad_zs: Vec<f32> = quad_starts
            .iter()
            .map(|start| mesh.world_sprites[*start].position[2])
            .collect();
        assert_eq!(
            quad_zs,
            vec![0.9, 0.5, -0.5],
            "应先是 background 层（z=0.9，层优先），再是同层内远→近（0.5 → -0.5）"
        );
        // 分段必须深度一致：同一分段内的 z 只能有一个值（否则"分段一个 z"的前提不成立）。
        for draw in &mesh.world_sprite_draws {
            let first = mesh.world_sprites[draw.vertex_start].position[2];
            assert!(
                (draw.vertex_start..draw.vertex_start + draw.vertex_count)
                    .all(|index| mesh.world_sprites[index].position[2] == first),
                "分段内混了不同 z"
            );
        }
    }
    /// docs/54 §4.3：**一等 Quad** 走的是与精灵完全相同的那条路。
    ///
    /// 判据有三条，缺一条"Quad 是一等可绘制对象"就不成立：
    /// ① 一个没有贴图的 Quad 也能产几何（`texture` 省略 ⇒ 内置白纹理，纯色矩形不再需要内部哨兵）；
    /// ② 几何/材质/深度/排序与精灵同源（同一个批处理 ⇒ 顶点数、z、分段规则一致）；
    /// ③ `Quad` 组件真的留在 ECS 里（不是实例化时被悄悄降级成 `Sprite`），因此运行时可查可改。
    #[test]
    fn quad_is_a_first_class_renderable_sharing_the_sprite_path() {
        let root = crate::core::games_dir().join("examples").join("game_api");
        if !root.join("scenes/main.scene").is_file() {
            return;
        }
        let mut scene = Scene::new("quad_primitives");
        // ① 纯色矩形：连 texture 都不写。
        scene
            .add_entity(SceneEntityId(1), "Backdrop", None)
            .unwrap();
        // ② 带贴图 + 显式排序层的矩形。
        scene.add_entity(SceneEntityId(2), "Bar", None).unwrap();
        {
            let backdrop = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
            backdrop.components.insert(
                "Transform".into(),
                "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
            backdrop
                .components
                .insert("Quad".into(), "size=64,32;color=0.25,0.5,1,1".into());
            let bar = scene.entities.get_mut(&SceneEntityId(2)).unwrap();
            bar.components.insert(
                "Transform".into(),
                "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
            bar.components.insert(
                "Quad".into(),
                "size=120,8;color=1,1,1,1;texture=assets/textures/ui.png;z=-0.5;layer=actor;order=2"
                    .into(),
            );
        }
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
        runtime.set_world_sprite_depth(true);
        let mesh = runtime.vertices(640, 360).expect("world vertices");
        assert_eq!(
            mesh.world_sprites.len(),
            12,
            "两个 Quad 各 6 个顶点（与精灵同一套几何）"
        );
        // 没有贴图的 Quad 采样内置白纹理；有贴图的注册进同一张纹理表（因此两者能合段）。
        assert!(
            mesh.paths
                .iter()
                .any(|path| path == crate::core::quad::QUAD_DEFAULT_TEXTURE),
            "缺省贴图必须解析到内置白纹理：{:?}",
            mesh.paths
        );
        assert!(
            mesh.paths
                .iter()
                .any(|path| path == "assets/textures/ui.png"),
            "显式贴图必须注册：{:?}",
            mesh.paths
        );
        // ②/③ 组件留在 ECS 里，且几何/材质按组件串生效。
        runtime.script.with_world(|world| {
            let mut quads: Vec<Quad> = Vec::new();
            world.for_each2::<Transform, Quad>(|id, _, quad| {
                assert!(
                    world.get::<Sprite>(id).expect("world read").is_none(),
                    "Quad 不该同时被写成 Sprite（否则会画两遍）"
                );
                quads.push(quad.clone());
            });
            assert_eq!(
                quads.len(),
                2,
                "两个 Quad 都必须留在 ECS 里（不能被降级成 Sprite）"
            );
            let plain = quads
                .iter()
                .find(|quad| quad.texture.is_none())
                .expect("应有那个没写 texture 的 Quad");
            assert_eq!(plain.color, [0.25, 0.5, 1.0, 1.0]);
            assert_eq!(plain.size, Vec2::new(64.0, 32.0));
        });
        // 分段按**层升序**绘制（层越大越靠上，与精灵同一套规则）：默认层的矩形先画，
        // `layer=actor;order=2` 的那个后画，因此压在上面。
        assert!(
            mesh.world_sprite_draws.len() >= 2,
            "两个不同层的 Quad 应落在不同分段：{:?}",
            mesh.world_sprite_draws.len()
        );
        let last = mesh.world_sprite_draws.last().expect("至少一个分段");
        let last_z = mesh.world_sprites[last.vertex_start].position[2];
        assert!(
            (last_z - -0.5).abs() < 1e-6,
            "显式排序层的 Quad 应画在后面（更靠上），实际末段 z={last_z}"
        );
    }

    /// docs/54 §4.1：每实体脚本必须**真的在固定步流水线里跑**——只断言"能手动调用"是不够的，
    /// 组件化脚本要替代的正是"工程脚本自己遍历实体"的那套手工分派。
    #[test]
    fn entity_script_component_runs_inside_the_step_pipeline() {
        let root =
            std::env::temp_dir().join(format!("rustlua-rs-entity-script-{}", std::process::id()));
        let scripts = root.join("scripts");
        std::fs::create_dir_all(&scripts).expect("create scripts dir");
        std::fs::write(
            scripts.join("spin.lua"),
            "function on_spawn(id) spawn_seen = (spawn_seen or 0) + 1 end\n\
             function on_update(id, dt)\n\
               local t = transform.get(id)\n\
               transform.set(id, t.x, t.y + 20 * dt)\n\
             end\n",
        )
        .expect("write script");

        let mut scene = Scene::new("entity_script_pipeline");
        scene.add_entity(SceneEntityId(1), "Spinner", None).unwrap();
        {
            let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
            entity.components.insert(
                "Transform".into(),
                "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
            entity
                .components
                .insert("Script".into(), "file=scripts/spin.lua".into());
        }
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
        let scripted_position = |runtime: &RuntimeScene| -> Vec2 {
            runtime.script.with_world(|world| {
                let mut position = None;
                world.for_each2::<Transform, crate::core::entity_script::EntityScript>(
                    |_, transform, _| position = Some(transform.position),
                );
                position.expect("带 Script 的实体必须有 Transform")
            })
        };
        assert!(
            scripted_position(&runtime).y.abs() < 1e-6,
            "脚本不该在实例化时就动实体：{:?}",
            scripted_position(&runtime)
        );
        runtime
            .run_script_steps(1, 0.5, &crate::core::input::InputSnapshot::default())
            .expect("step runtime");
        assert_eq!(
            runtime.last_entity_script_calls(),
            1,
            "这一步应有 1 个实体脚本被调用"
        );
        let after = scripted_position(&runtime);
        assert!(
            (after.y - 10.0).abs() < 1e-4,
            "脚本应按 20*dt 移动实体（dt=0.5）：实际 {after:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// docs/54 §4.4：**网格也走同一套运行时身份**——3D 提取是 `&self`，因此身份表在 `RefCell`
    /// 里；同一网格跨帧恒同句柄、不同网格不同句柄，且身份表记录的 `ResourceKind` 是 `Model`。
    #[test]
    fn mesh_identity_is_stable_across_frames() {
        let root = crate::core::games_dir().join("examples").join("game_api");
        if !root.join("scenes/main.scene").is_file() {
            return;
        }
        let mut scene = Scene::new("mesh_identity");
        for (id, mesh) in [
            (SceneEntityId(1), crate::core::mesh3d::GROUND_PLANE_SENTINEL),
            (SceneEntityId(2), "assets/models/missing.glb"),
        ] {
            scene.add_entity(id, format!("Mesh{}", id.0), None).unwrap();
            let entity = scene.entities.get_mut(&id).unwrap();
            entity.components.insert(
                "Transform".into(),
                "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
            entity
                .components
                .insert("MeshRenderer".into(), format!("mesh={mesh}"));
        }
        let runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
        let first = runtime
            .scene_3d(Viewport::new(16.0, 9.0).unwrap())
            .expect("first frame 3d");
        assert_eq!(first.drawables.len(), 2, "两个网格都应在 drawable 里");
        let handles: Vec<_> = first
            .drawables
            .iter()
            .map(|drawable| drawable.mesh_handle.expect("运行时必须给网格身份"))
            .collect();
        assert_ne!(handles[0], handles[1], "不同网格必须是不同句柄");
        let second = runtime
            .scene_3d(Viewport::new(16.0, 9.0).unwrap())
            .expect("second frame 3d");
        let second_handles: Vec<_> = second
            .drawables
            .iter()
            .map(|drawable| drawable.mesh_handle.expect("身份"))
            .collect();
        assert_eq!(handles, second_handles, "同一网格在相邻两帧必须同句柄");
        let record = runtime
            .asset_entries()
            .into_iter()
            .find(|record| record.path == crate::core::mesh3d::GROUND_PLANE_SENTINEL)
            .expect("网格身份应在表里");
        assert_eq!(record.kind, crate::core::resource_kind::ResourceKind::Model);
    }

    /// docs/54 §4.4：运行时资源身份**跨帧稳定**——同一个路径在相邻两帧必须是同一个句柄，
    /// 这正是"这一帧的 draw 引用的是哪个资源"可以跨帧比较的前提（`texture_paths` 的下标每帧
    /// 重建，本身不是身份）。
    #[test]
    fn texture_identity_is_stable_across_frames() {
        let root = crate::core::games_dir().join("examples").join("game_api");
        if !root.join("scenes/main.scene").is_file() {
            return;
        }
        let mut scene = Scene::new("asset_identity");
        scene.add_entity(SceneEntityId(1), "Hero", None).unwrap();
        {
            let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
            entity.components.insert(
                "Transform".into(),
                "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
            entity.components.insert(
                "Sprite".into(),
                "texture=assets/textures/player.png;size=32x32".into(),
            );
        }
        scene.add_entity(SceneEntityId(2), "Bar", None).unwrap();
        {
            let entity = scene.entities.get_mut(&SceneEntityId(2)).unwrap();
            entity.components.insert(
                "Transform".into(),
                "position=640,300,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
            // 第二个资源（内置白纹理）：必须与贴图拿到**不同**的句柄。
            entity
                .components
                .insert("Quad".into(), "size=64,8;color=1,1,1,1".into());
        }
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
        let first = runtime.vertices(640, 360).expect("first frame");
        assert_eq!(
            first.path_handles.len(),
            first.paths.len(),
            "句柄必须与路径同序同长"
        );
        assert_eq!(first.paths.len(), 2, "本帧应注册两个资源：贴图 + 白纹理");
        let second = runtime.vertices(640, 360).expect("second frame");
        assert_eq!(
            first.path_handles, second.path_handles,
            "同一路径在相邻两帧必须拿到同一个句柄"
        );
        assert_eq!(first.paths, second.paths);
        assert_ne!(
            first.path_handles[0], first.path_handles[1],
            "不同资源必须是不同句柄"
        );
        // 身份表里的记录确实是路径本体（不是"每帧重新编号"的副本）。
        let entries = runtime.asset_entries();
        assert_eq!(entries.len(), 2, "身份表应恰好登记两个资源");
        let mut paths: Vec<&str> = entries.iter().map(|record| record.path.as_str()).collect();
        paths.sort_unstable();
        assert_eq!(
            paths,
            vec!["__white__", "assets/textures/player.png"],
            "身份表记录的就是资源路径"
        );
        assert_eq!(
            entries
                .iter()
                .find(|record| record.path == "assets/textures/player.png")
                .map(|record| record.kind),
            Some(ResourceKind::Texture)
        );
    }

    /// docs/54 §10 批 R：**输入没变就跳过传播**——静态层级场景不再付那三成帧成本，而任何一处
    /// 局部真源变化都必须立刻退回全量传播。
    ///
    /// 判据分四段，缺一段这条优化就不成立：① 不动的帧确实在跳过（世界值仍正确）；② 改**自己**的局部值
    /// 会传播；③ 改**父**的局部值也会传播（子自己的局部没变，但世界值必须跟着父走——这一条最容易漏）；
    /// ④ 回滚档（`set_spatial_skip_unchanged(false)`）每帧都传播。
    #[test]
    fn static_hierarchy_skips_propagation_until_something_changes() {
        let root = crate::core::games_dir().join("examples").join("game_api");
        if !root.join("scenes/main.scene").is_file() {
            return;
        }
        let parent = SceneEntityId(1);
        let child = SceneEntityId(2);
        let mut scene = Scene::new("spatial_skip");
        scene.metadata.insert("spatial".into(), "local".into());
        scene.add_entity(parent, "Rig", None).unwrap();
        scene.add_entity(child, "Part", Some(parent)).unwrap();
        scene.entities.get_mut(&parent).unwrap().components.insert(
            "Transform".into(),
            "position=10,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        scene.entities.get_mut(&child).unwrap().components.insert(
            "Transform".into(),
            "position=2,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
        let input = crate::core::input::InputSnapshot::default();
        let child_world_x = |runtime: &RuntimeScene| {
            runtime
                .world_transform_2d_of(child)
                .expect("child world transform")
                .position
                .x
        };

        // ① 连跑三步：除第一帧外都应跳过，世界值保持 (12, 0)。
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step");
        assert_eq!(child_world_x(&runtime), 12.0);
        let skipped_before = runtime.spatial_skipped_frames();
        for _ in 0..3 {
            runtime
                .run_script_steps(1, 1.0 / 60.0, &input)
                .expect("step");
        }
        // 每步会调用两次传播（步末一次 + `spatial_propagate` 帧系统一次），因此用下界断言：
        // "不动的帧确实在跳过"这件事不依赖调用次数。
        assert!(
            runtime.spatial_skipped_frames() >= skipped_before + 3,
            "输入没变的帧必须整趟跳过传播（实际 {}，起点 {skipped_before}）",
            runtime.spatial_skipped_frames()
        );
        assert_eq!(child_world_x(&runtime), 12.0, "跳过不等于世界值失效");

        // ② 改自己的局部值 ⇒ 必须传播。
        let moved = crate::core::spatial_transform::LocalTransform {
            translation: crate::core::math3d::Vec3::new(5.0, 0.0, 0.0),
            ..crate::core::spatial_transform::LocalTransform::identity()
        };
        runtime
            .set_local_transform(child, moved)
            .expect("write child local");
        let skipped = runtime.spatial_skipped_frames();
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step");
        // 一步里有两次传播调用（步末 + 帧系统）：变化后的**第一次**必须真传播，第二次才可以跳过。
        // 因此这里判两件事：世界值真的更新了（说明没跳过），且跳过增量至多 1。
        assert_eq!(child_world_x(&runtime), 15.0, "10 + 5：局部值变了必须传播");
        assert!(
            runtime.spatial_skipped_frames() - skipped <= 1,
            "变化后的第一次传播不许跳过（跳过增量 {}）",
            runtime.spatial_skipped_frames() - skipped
        );

        // ③ 改**父**的局部值：子自己的局部没变，但世界值必须跟着父走。
        let parent_moved = crate::core::spatial_transform::LocalTransform {
            translation: crate::core::math3d::Vec3::new(20.0, 0.0, 0.0),
            ..crate::core::spatial_transform::LocalTransform::identity()
        };
        runtime
            .set_local_transform(parent, parent_moved)
            .expect("write parent local");
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step");
        assert_eq!(
            child_world_x(&runtime),
            25.0,
            "父的局部值变化必须传到子的世界值（指纹覆盖父链，不只是自己的局部）"
        );

        // ④ 回滚档：关掉跳过 ⇒ 每帧都传播。
        assert!(
            runtime.set_spatial_skip_unchanged(false),
            "默认是开启的（返回前一个值）"
        );
        assert!(!runtime.spatial_skip_unchanged());
        let skipped = runtime.spatial_skipped_frames();
        for _ in 0..3 {
            runtime
                .run_script_steps(1, 1.0 / 60.0, &input)
                .expect("step");
        }
        assert_eq!(
            runtime.spatial_skipped_frames(),
            skipped,
            "回滚档下不许跳过任何一帧"
        );
        assert_eq!(child_world_x(&runtime), 25.0);
        // 再打开：第一帧必须重新全量传播一次（清过指纹），之后才允许跳过。
        assert!(!runtime.set_spatial_skip_unchanged(true));
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step");
        let skipped = runtime.spatial_skipped_frames();
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step");
        assert!(
            runtime.spatial_skipped_frames() > skipped,
            "重新打开后第一帧全量传播、之后允许跳过"
        );
        assert_eq!(child_world_x(&runtime), 25.0);
    }

    /// docs/54 §10 批 S：**逐实体增量**在真实帧里只重算变了的子树。
    ///
    /// 判据分四段：① 首帧全量（建立缓存）；② 只挪链尾 ⇒ 路径是增量且**重算数 = 1**（不是实体
    /// 总数）；③ 挪链根 ⇒ 重算整条链（世界值是沿父链组合的，父一变整棵子树过期）；④ 回滚档
    /// （`set_spatial_incremental(false)`）每帧全量、世界值一致。
    #[test]
    fn incremental_propagation_recomputes_only_changed_subtrees() {
        let root = crate::core::games_dir().join("examples").join("game_api");
        if !root.join("scenes/main.scene").is_file() {
            return;
        }
        // 一条 3 层的链 root → middle → leaf，每层局部 x 偏移。
        let (ring, middle, leaf) = (SceneEntityId(1), SceneEntityId(2), SceneEntityId(3));
        let mut scene = Scene::new("spatial_incremental");
        scene.metadata.insert("spatial".into(), "local".into());
        scene.add_entity(ring, "Rig", None).unwrap();
        scene.add_entity(middle, "Arm", Some(ring)).unwrap();
        scene.add_entity(leaf, "Hand", Some(middle)).unwrap();
        for (id, x) in [(ring, 10.0), (middle, 2.0), (leaf, 1.0)] {
            scene.entities.get_mut(&id).unwrap().components.insert(
                "Transform".into(),
                format!("position={x},0,0;rotation=0,0,0,1;scale=1,1,1"),
            );
        }
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
        let input = InputSnapshot::default();
        let world_x = |runtime: &RuntimeScene, entity: SceneEntityId| {
            runtime
                .world_transform_2d_of(entity)
                .unwrap_or_else(|| panic!("entity {entity:?} 没有世界变换"))
                .position
                .x
        };
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step");
        assert_eq!(world_x(&runtime, leaf), 13.0);
        // ① 首帧没有缓存可比 ⇒ 全量。
        assert!(
            runtime.spatial_path_counts()[1] >= 1,
            "首帧必须全量传播一次（计数 {:?}）",
            runtime.spatial_path_counts()
        );

        // ② 只挪链尾：一帧里重算的实体数应当是 1。
        let moved_leaf = crate::core::spatial_transform::LocalTransform {
            translation: crate::core::math3d::Vec3::new(5.0, 0.0, 0.0),
            ..crate::core::spatial_transform::LocalTransform::identity()
        };
        runtime
            .set_local_transform(leaf, moved_leaf)
            .expect("write leaf local");
        let _ = runtime.take_spatial_path_counts();
        let _ = runtime.take_spatial_recomputed_total();
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step");
        assert_eq!(world_x(&runtime, leaf), 17.0, "10 + 2 + 5");
        assert_eq!(
            runtime.take_spatial_recomputed_total(),
            1,
            "只挪一个叶子必须只重算 1 个实体（步末那次；帧系统那次看到指纹没变会跳过）"
        );
        let counts = runtime.take_spatial_path_counts();
        assert!(counts[2] >= 1, "本步应当走过增量路径（计数 {counts:?}）");

        // ③ 挪链根：整条链都要重算（3 个实体）。
        let moved_root = crate::core::spatial_transform::LocalTransform {
            translation: crate::core::math3d::Vec3::new(20.0, 0.0, 0.0),
            ..crate::core::spatial_transform::LocalTransform::identity()
        };
        runtime
            .set_local_transform(ring, moved_root)
            .expect("write root local");
        let _ = runtime.take_spatial_recomputed_total();
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step");
        assert_eq!(world_x(&runtime, leaf), 27.0, "20 + 2 + 5");
        assert_eq!(
            runtime.take_spatial_recomputed_total(),
            3,
            "挪链根必须重算整条链（父的世界值一变，整棵子树过期）"
        );

        // ④ 回滚档（`set_spatial_incremental(false)`）：值变化后一律全量，不再走增量。
        //    （"每帧都传播"是另一个开关 `set_spatial_skip_unchanged` 的语义，批 R 的测试管它。）
        assert!(
            runtime.set_spatial_incremental(false),
            "默认开启（返回前一个值）"
        );
        assert!(!runtime.spatial_incremental());
        let _ = runtime.take_spatial_path_counts();
        for step in 0..3 {
            let moved = crate::core::spatial_transform::LocalTransform {
                translation: crate::core::math3d::Vec3::new(1.0 + step as f32, 0.0, 0.0),
                ..crate::core::spatial_transform::LocalTransform::identity()
            };
            runtime
                .set_local_transform(leaf, moved)
                .expect("write leaf local");
            runtime
                .run_script_steps(1, 1.0 / 60.0, &input)
                .expect("step");
        }
        let counts = runtime.take_spatial_path_counts();
        assert_eq!(counts[2], 0, "回滚档下不许走增量路径（计数 {counts:?}）");
        assert!(
            counts[1] >= 3,
            "回滚档下每次值变化都要全量（计数 {counts:?}）"
        );
        assert_eq!(world_x(&runtime, leaf), 25.0, "20 + 2 + 3");
        // 重新打开：缓存被清，因此**下一次值变化**必须全量一次（此后又能走增量）。
        assert!(!runtime.set_spatial_incremental(true));
        let _ = runtime.take_spatial_path_counts();
        runtime
            .set_local_transform(leaf, moved_leaf)
            .expect("write leaf local");
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step");
        assert_eq!(world_x(&runtime, leaf), 27.0, "20 + 2 + 5");
        let counts = runtime.spatial_path_counts();
        assert!(
            counts[1] >= 1,
            "重开后的第一次值变化必须全量（计数 {counts:?}）"
        );
        assert_eq!(
            counts[2], 0,
            "缓存刚建好，这一步还走不了增量（计数 {counts:?}）"
        );
        // 再改一次值 ⇒ 走增量，且只重算那一个叶子。
        let moved_again = crate::core::spatial_transform::LocalTransform {
            translation: crate::core::math3d::Vec3::new(6.0, 0.0, 0.0),
            ..crate::core::spatial_transform::LocalTransform::identity()
        };
        runtime
            .set_local_transform(leaf, moved_again)
            .expect("write leaf local");
        let _ = runtime.take_spatial_recomputed_total();
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step");
        assert_eq!(world_x(&runtime, leaf), 28.0, "20 + 2 + 6");
        assert_eq!(
            runtime.take_spatial_recomputed_total(),
            1,
            "缓存建好之后的第二次值变化又只重算那个叶子"
        );
    }

    /// docs/54 §4.1：**文档侧与运行时侧的局部变换必须是同一份规范表示**。
    ///
    /// 这条是"运行时只有一份权威 `LocalTransform`"在**表示层**的可执行判据：对一个真实场景，逐实体
    /// 比对 `RuntimeScene::local_transform_of`（运行时从 ECS 的边界格式转出来的 `LocalTransform`）
    /// 与 `scene_space::local_transforms`（文档侧直接解析出来的 `LocalTransform`）。两者任何一处改了
    /// 转换口径都会在这里失败——这正是"两份真源"最容易被引入的地方。
    #[test]
    fn runtime_and_document_agree_on_the_canonical_local_transform() {
        let root = crate::core::games_dir().join("examples").join("game_api");
        let path = root.join("scenes/index.scene");
        if !path.is_file() {
            return;
        }
        let scene = Scene::load(&path).expect("load shipped scene");
        let document = crate::core::scene_space::local_transforms(&scene);
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene.clone()).expect("runtime");
        assert!(!document.is_empty(), "场景应当带局部变换");
        for entity in scene.entities.values() {
            let Some(expected) = document.get(&entity.id).copied() else {
                continue;
            };
            let actual = runtime
                .local_transform_of(entity.id)
                .unwrap_or_else(|| panic!("运行时缺少 {} 的局部变换", entity.name));
            assert!(
                (actual.translation - expected.translation).length() < 1e-5
                    && (actual.scale - expected.scale).length() < 1e-5,
                "{} 的局部变换两侧不一致：文档={expected:?} 运行时={actual:?}",
                entity.name
            );
        }

        // 写回：统一表示写进实体自己的空间，round-trip 必须回到同一个值。
        let target = scene
            .entities
            .values()
            .find(|entity| document.contains_key(&entity.id))
            .expect("至少一个带变换的实体")
            .id;
        let mut moved = document[&target];
        moved.translation = moved.translation + crate::core::math3d::Vec3::new(3.0, 4.0, 0.0);
        assert!(
            runtime
                .set_local_transform(target, moved)
                .expect("写回平坦值"),
            "实体本来就有局部变换，应当写回成功"
        );
        let read_back = runtime.local_transform_of(target).expect("读回");
        assert!(
            (read_back.translation - moved.translation).length() < 1e-5,
            "写回后读到的应是同一个值：{read_back:?} vs {moved:?}"
        );
    }

    /// docs/54 §4.1：写回非平面值到**只有 2D `Transform`** 的实体必须**报错**，而不是悄悄压平
    /// （手抄 XY 会把旋转与缩放丢掉）。
    #[test]
    fn non_planar_local_transform_is_refused_for_2d_entities() {
        let root = crate::core::games_dir().join("examples").join("game_api");
        if !root.join("scenes/main.scene").is_file() {
            return;
        }
        let mut scene = Scene::new("local_transform_write");
        scene.add_entity(SceneEntityId(1), "Flat", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "position=1,2,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
        let planar = crate::core::spatial_transform::LocalTransform {
            translation: crate::core::math3d::Vec3::new(5.0, 6.0, 0.0),
            rotation: crate::core::math3d::Quat::identity(),
            scale: crate::core::math3d::Vec3::new(2.0, 2.0, 1.0),
        };
        assert!(
            runtime
                .set_local_transform(SceneEntityId(1), planar)
                .unwrap()
        );
        let tilted = crate::core::spatial_transform::LocalTransform {
            // 沿 z 抬起：2D 平面装不下。
            translation: crate::core::math3d::Vec3::new(5.0, 6.0, 3.0),
            ..planar
        };
        let error = runtime
            .set_local_transform(SceneEntityId(1), tilted)
            .expect_err("非平面值必须被拒绝");
        assert!(error.to_string().contains("not planar"), "{error}");
        // 拒绝之后原值必须原样保留（不留半写状态）。
        let unchanged = runtime
            .local_transform_of(SceneEntityId(1))
            .expect("局部变换仍在");
        assert!((unchanged.translation.z).abs() < 1e-6);
        assert!((runtime.local_transform_of(SceneEntityId(99))).is_none());
    }
    /// docs/54 §4.4：**动画片段也有运行时身份**——推进动画时把片段的内容签名 intern 进与贴图/网格
    /// 同一张身份表，因此"这份动画是哪一个"跨帧可比，而 `ResourceKind::Animation` 让它在诊断里
    /// 一眼可辨。
    #[test]
    fn animation_clip_identity_is_stable_across_frames() {
        let root = crate::core::games_dir().join("examples").join("game_api");
        if !root.join("scenes/main.scene").is_file() {
            return;
        }
        let mut scene = Scene::new("clip_identity");
        scene.add_entity(SceneEntityId(1), "Hero", None).unwrap();
        {
            let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
            entity.components.insert(
                "Transform".into(),
                "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
            entity.components.insert(
                "Sprite".into(),
                "texture=assets/textures/player.png;size=32x32".into(),
            );
            entity.components.insert(
                "Animation".into(),
                "default=idle;clip:idle:assets/textures/hero-idle.png:4:4:true".into(),
            );
        }
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
        // 第一步：推进动画时把片段身份 intern 进身份表。
        runtime
            .run_script_steps(1, 1.0 / 60.0, &crate::core::input::InputSnapshot::default())
            .expect("step");
        let clip_key = |runtime: &RuntimeScene| -> String {
            let mut found = None;
            runtime.script.with_world(|world| {
                world.for_each::<crate::core::animation::AnimationStateMachine>(|_, machine| {
                    found = Some(machine.clip_resource_key().to_owned());
                });
            });
            found.expect("场景里的实体应当有动画状态机")
        };
        let key = clip_key(&runtime);
        let first = runtime
            .asset_handle(&key)
            .expect("推进动画后片段身份应已登记");
        let record = runtime
            .asset_entries()
            .into_iter()
            .find(|record| record.path == key)
            .expect("身份表里应有这条片段");
        assert_eq!(
            record.kind,
            crate::core::resource_kind::ResourceKind::Animation,
            "片段身份必须标成动画资源"
        );
        let sizes = runtime.asset_entries().len();
        runtime
            .run_script_steps(1, 1.0 / 60.0, &crate::core::input::InputSnapshot::default())
            .expect("second step");
        assert_eq!(clip_key(&runtime), key, "片段内容没变，身份键也不该变");
        assert_eq!(
            runtime.asset_handle(&key),
            Some(first),
            "同一段片段在相邻两步必须同句柄"
        );
        assert_eq!(
            runtime.asset_entries().len(),
            sizes,
            "已登记过的片段不该在第二帧重复登记"
        );
    }
    /// docs/54 §4.3：**一个实体只有一种可绘制表达**。
    ///
    /// 同时挂 `Sprite` 与 `Quad` 时：① 加载期点名（否则"为什么另一个没画出来"无从解释）；
    /// ② 渲染只按写下来的优先级画一种（`Sprite` 优先）——此前两条收集路径各自 push，会**画两遍**。
    #[test]
    fn conflicting_renderables_are_reported_and_only_one_draws() {
        let root = crate::core::games_dir().join("examples").join("game_api");
        if !root.join("scenes/main.scene").is_file() {
            return;
        }
        let mut scene = Scene::new("renderable_conflict");
        scene.add_entity(SceneEntityId(1), "Both", None).unwrap();
        {
            let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
            entity.components.insert(
                "Transform".into(),
                "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
            entity.components.insert(
                "Sprite".into(),
                "texture=assets/textures/player.png;size=32x32".into(),
            );
            entity
                .components
                .insert("Quad".into(), "size=64,64;color=1,0,0,1".into());
        }
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
        let conflicts = runtime.renderable_conflicts();
        assert_eq!(conflicts.len(), 1, "冲突必须被点名：{conflicts:?}");
        assert!(
            conflicts[0].contains("Both") && conflicts[0].contains("sprite + quad"),
            "诊断要点名实体与冲突项：{conflicts:?}"
        );
        runtime.set_world_sprite_depth(true);
        let mesh = runtime.vertices(640, 360).expect("vertices");
        assert_eq!(
            mesh.world_sprites.len(),
            6,
            "只应画精灵那一种（6 顶点），Quad 不许再画一遍"
        );
    }
    /// docs/54 §4.3：`merge_adjacent_draws` **必须把 `render_layer` 一起比**。
    ///
    /// 回归背景：合并条件是"纹理相同 + 顶点连续"，漏了层以后，相邻的 ScreenUI 段会被并进前一段
    /// 并沿用它的 `render_layer`——几何还在、层却变了，HUD 隔离（§8-⑤）会静默失效。
    #[test]
    fn merging_adjacent_draws_never_crosses_render_layers() {
        let mut draws = vec![
            SceneDraw::overlay(1, 0, 6),
            // 与上一段纹理相同、顶点连续，但层不同：不许并。
            SceneDraw::screen_ui(1, 6, 6),
            // 同层且连续：应当并成一段。
            SceneDraw::screen_ui(1, 12, 6),
        ];
        merge_adjacent_draws(&mut draws);
        assert_eq!(
            draws
                .iter()
                .map(|draw| (draw.render_layer, draw.start, draw.count))
                .collect::<Vec<_>>(),
            vec![
                (RenderLayer::Overlay2D, 0, 6),
                (RenderLayer::ScreenUI, 6, 12),
            ],
            "跨层不许合并，同层连续才合并"
        );
    }
    /// docs/54 §4.3：**三种显式 2D 显示模式**让同一场景混用世界通道与屏幕空间内容。
    ///
    /// 判据是"几何真的走了哪条路"：默认实体进世界通道；`mode=overlay` / `mode=screen` 留在屏幕
    /// 空间（分别落 `Overlay2D` / `ScreenUI` 桶，因此画在世界内容之上）；`mode=world` 请求世界
    /// 通道——即使宿主开关是关的，只要场景里有实体这样声明，这一帧就走世界通道。
    #[test]
    fn explicit_render_modes_route_geometry_to_the_right_channel() {
        let root = crate::core::games_dir().join("examples").join("game_api");
        if !root.join("scenes/main.scene").is_file() {
            return;
        }
        let mut scene = Scene::new("render_modes");
        for (id, name, mode) in [
            (SceneEntityId(1), "Default", None),
            (SceneEntityId(2), "Hud", Some("mode=overlay")),
            (SceneEntityId(3), "ScreenUi", Some("mode=screen")),
        ] {
            scene.add_entity(id, name, None).unwrap();
            let entity = scene.entities.get_mut(&id).unwrap();
            entity.components.insert(
                "Transform".into(),
                "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
            entity.components.insert(
                "Sprite".into(),
                "texture=assets/textures/player.png;size=32x32".into(),
            );
            if let Some(mode) = mode {
                entity.components.insert("RenderMode".into(), mode.into());
            }
        }
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
        // 宿主打开世界通道：默认实体进世界通道，显式屏幕空间的两个留下。
        runtime.set_world_sprite_depth(true);
        let mesh = runtime.vertices(640, 360).expect("vertices");
        assert_eq!(
            runtime.render_mode_routed(),
            (0, 1, 1),
            "本帧应路由 1 个 overlay、1 个 screen，且没有显式 world"
        );
        assert_eq!(
            mesh.world_sprites.len(),
            6,
            "只有默认实体进了世界通道（6 顶点）"
        );
        let layers: Vec<RenderLayer> = mesh.draws.iter().map(|draw| draw.render_layer).collect();
        assert!(
            layers.contains(&RenderLayer::Overlay2D),
            "`mode=overlay` 的实体必须落在 Overlay2D 桶：{layers:?}"
        );
        assert!(
            layers.contains(&RenderLayer::ScreenUI),
            "`mode=screen` 的实体必须落在 ScreenUI 桶：{layers:?}"
        );
        assert_eq!(
            layers
                .iter()
                .filter(|layer| **layer == RenderLayer::World2D)
                .count(),
            0,
            "世界通道那一帧里不该再有 World2D 的 NDC 绘制"
        );

        // 宿主**关掉**世界通道，但场景里有实体声明 `mode=world` ⇒ 这一帧照样走世界通道。
        let mut scene = Scene::new("render_modes_world");
        scene.add_entity(SceneEntityId(1), "Forced", None).unwrap();
        {
            let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
            entity.components.insert(
                "Transform".into(),
                "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
            entity.components.insert(
                "Sprite".into(),
                "texture=assets/textures/player.png;size=32x32".into(),
            );
            entity
                .components
                .insert("RenderMode".into(), "mode=world".into());
        }
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
        assert!(!runtime.world_sprite_depth(), "宿主开关刻意保持关闭");
        let mesh = runtime.vertices(640, 360).expect("vertices");
        assert_eq!(runtime.render_mode_routed(), (1, 0, 0));
        assert_eq!(
            mesh.world_sprites.len(),
            6,
            "`mode=world` 必须让这一帧走世界通道"
        );
    }
    /// WP6：世界文字也进世界坐标深度通道——它此前是唯一仍留在屏幕空间的世界内容。    /// 转换走 `view_projection` 的**逆映射**，因此屏幕观感与 overlay 路径逐个像素一致，
    /// 但顶点有了世界坐标与 `z`，可以和精灵/瓦片一起参与深度测试。
    #[test]
    fn world_sprite_depth_moves_world_text_into_the_world_channel() {
        // 与 `world_text_anchors_to_world_position_and_follows_camera` 同一套场景/相机参数，
        // 保证走的是同一条字形管线（否则会因项目根不同而落到矢量字体分支，测不到本路径）。
        let mut scene = Scene::new("world_text_depth");
        scene.add_entity(SceneEntityId(1), "Label", None).unwrap();
        {
            let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
            entity.components.insert(
                "Transform".into(),
                "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
            entity.components.insert(
                "WorldText".into(),
                "text=HP;font_size=24;color=1,1,1;offset=0,-20;centered=true".into(),
            );
        }
        let mut runtime = RuntimeScene::from_scene_edit(None, scene).expect("runtime");
        runtime.set_camera_2d(Camera2D::new(Vec2::new(320.0, 180.0), 1.0).expect("camera"));

        let overlay = runtime.vertices(640, 360).expect("overlay vertices");
        assert!(!overlay.vertices.is_empty(), "世界文字应产出 overlay 顶点");
        assert!(overlay.world_sprites.is_empty());

        // 世界通道：顶点落到**实体世界坐标附近**（`WorldText` 的锚点就是实体位置 (0, 0)；
        // 相机中心只是取景，不改变锚点），并在**世界**绘制列表里登记分段。
        runtime.set_world_sprite_depth(true);
        let world = runtime.vertices(640, 360).expect("world vertices");
        assert!(
            !world.world_sprites.is_empty(),
            "世界文字应产出世界坐标顶点"
        );
        let (min_x, max_x) = world
            .world_sprites
            .iter()
            .map(|vertex| vertex.position[0])
            .fold((f32::MAX, f32::MIN), |(min, max), x| {
                (min.min(x), max.max(x))
            });
        assert!(
            min_x > -60.0 && max_x < 120.0,
            "文字顶点应落在实体世界坐标 (0, 0) 附近（而不是 NDC）：{min_x}..{max_x}"
        );
        assert!(
            world
                .world_sprites
                .iter()
                .all(|vertex| vertex.position[2] == 0.0),
            "世界文字在世界平面 z=0"
        );
        assert!(
            !world.world_sprite_draws.is_empty(),
            "世界文字的绘制分段必须登记在世界列表里（否则渲染器不会画它）"
        );
    }
    /// WP6：开启世界深度通道后，**瓦片层**也要产出世界坐标顶点（此前只有精灵有），
    /// 否则瓦片会被挡在深度通道之外——世界内容一半有深度、一半没有，遮挡必然错。
    #[test]
    fn world_sprite_depth_moves_tilemaps_into_the_world_channel() {
        let root = game_root();
        let mut scene = crate::core::scene::Scene::new("tilemap-world-pass");
        let id = crate::core::scene::SceneEntityId(1);
        scene.add_entity(id, "Map", None).unwrap();
        let entity = scene.entities.get_mut(&id).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        // 2×2 瓦片地图：用真实 tileset 纹理，保证纹理注册与 UV 都走正常路径。
        entity.components.insert(
            "Tilemap".into(),
            "tile_size=32x32;width=2;height=2;texture=assets/textures/tileset.png;\
             tiles=1111;solid=0000"
                .into(),
        );
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");

        // 默认（overlay 路径）：瓦片是 NDC 顶点，没有世界顶点。
        let overlay = runtime.vertices(640, 360).expect("overlay vertices");
        assert!(
            overlay.vertices.len() >= 24,
            "4 个瓦片应产出 24 个 overlay 顶点：{}",
            overlay.vertices.len()
        );
        assert!(overlay.world_sprites.is_empty(), "关闭时不应有世界顶点");

        // 打开世界通道：瓦片改为世界坐标（中心 (640, 360) 附近、z = 0），overlay 里不再有瓦片。
        runtime.set_world_sprite_depth(true);
        let world = runtime.vertices(640, 360).expect("world vertices");
        assert!(
            world.world_sprites.len() >= 24,
            "瓦片应产出世界顶点：{}",
            world.world_sprites.len()
        );
        assert!(
            world.vertices.len() < overlay.vertices.len(),
            "瓦片已从 overlay 路径移出：{} vs {}",
            world.vertices.len(),
            overlay.vertices.len()
        );
        assert!(
            !world.world_sprite_draws.is_empty(),
            "瓦片的世界顶点必须**登记在世界绘制分段里**（否则渲染器根本不会画它）"
        );
        assert!(
            world
                .world_sprite_draws
                .iter()
                .all(|draw| draw.render_layer == crate::core::layer::RenderLayer::World2D),
            "世界通道的分段必须是 World2D"
        );
        assert!(
            world.draws.iter().all(|draw| draw.count > 0),
            "overlay 分段必须仍然自洽"
        );
        // 世界坐标而不是 NDC：数值应落在千级像素附近，而不是 [-1, 1]。
        let max_x = world
            .world_sprites
            .iter()
            .map(|vertex| vertex.position[0])
            .fold(f32::MIN, f32::max);
        assert!(max_x > 100.0, "顶点应是世界坐标：max_x={max_x}");
        assert!(
            world
                .world_sprites
                .iter()
                .all(|vertex| vertex.position[2] == 0.0),
            "瓦片层就在世界平面 z=0"
        );
    }
    /// WP6 回滚档：开关默认关闭（精灵走 overlay 的 NDC 顶点），打开后精灵改走世界坐标 +
    /// 深度的通道，两条路径的精灵集合相同（同一个批处理），只是产出形态不同。
    #[test]
    fn world_sprite_depth_flag_switches_between_overlay_and_world_vertices() {
        let root = game_root();
        let mut scene = crate::core::scene::Scene::new("world-sprite-flag");
        let id = crate::core::scene::SceneEntityId(1);
        scene.add_entity(id, "Hero", None).unwrap();
        let entity = scene.entities.get_mut(&id).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity.components.insert(
            "Sprite".into(),
            "texture=assets/textures/player.png;size=16x16;pivot=0.5,0.5;z=-0.25".into(),
        );
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");

        // 默认：overlay 路径——有 NDC 顶点、没有世界顶点。
        assert!(!runtime.world_sprite_depth());
        assert!(runtime.sprite_view_projection(640, 360).is_none());
        let overlay = runtime.vertices(640, 360).expect("overlay vertices");
        assert!(!overlay.vertices.is_empty(), "overlay 应有 NDC 顶点");
        assert!(overlay.world_sprites.is_empty(), "关闭时不应产出世界顶点");

        // 打开：世界路径——世界坐标顶点（含 `z=-0.25`），overlay 精灵顶点为空。
        runtime.set_world_sprite_depth(true);
        assert!(runtime.sprite_view_projection(640, 360).is_some());
        let world = runtime.vertices(640, 360).expect("world vertices");
        assert!(!world.world_sprites.is_empty(), "打开时应产出世界顶点");
        assert_eq!(world.world_sprites.len(), 6, "一个精灵 6 个顶点");
        assert_eq!(
            world.world_sprites[0].position[2], -0.25,
            "z 来自 Sprite::z"
        );
        assert!(
            (world.world_sprites[0].position[0] - 632.0).abs() < 1e-3,
            "世界坐标应为 640-8：{:?}",
            world.world_sprites[0].position
        );
        assert!(world.world_sprites[0].position[0] > 1.0, "世界坐标不是 NDC");
        assert!(!world.world_sprite_draws.is_empty(), "应有世界绘制分段");

        // 关回去：逐字节回到 overlay 路径。
        runtime.set_world_sprite_depth(false);
        let back = runtime.vertices(640, 360).expect("overlay vertices again");
        assert_eq!(back.vertices.len(), overlay.vertices.len());
        assert!(back.world_sprites.is_empty());
    }

    /// docs/54 §8-④：世界通道的可见域是**宿主声明的世界视图**，不是 2D 相机。
    ///
    /// 回归背景（`wp0_mixed_golden` 抓到的真实误丢）：2D 相机活在设计分辨率像素空间
    /// （默认中心 (640,360)），而 3D 世界视图里 `(-40, 0)` 这类坐标清晰可见。场景里有 3D 内容时
    /// 宿主会声明 3D 主视图，此时若仍按 2D 相机裁剪，世界内容会被**静默丢掉**。
    #[test]
    fn world_channel_does_not_cull_by_the_2d_camera_when_the_scene_has_3d_content() {
        let root = game_root();
        let mut scene = Scene::new("world-view-cull");
        let sprite = crate::core::scene::SceneEntityId(1);
        scene.add_entity(sprite, "FarSprite", None).unwrap();
        let entity = scene.entities.get_mut(&sprite).unwrap();
        // 默认 2D 相机中心 (640,360)、zoom 1 ⇒ 世界 (-40,0) 落在 2D 视口之外。
        entity.components.insert(
            "Transform".into(),
            "position=-40,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity.components.insert(
            "Sprite".into(),
            "texture=assets/textures/red-1x1.png;size=2x2;pivot=0.5,0.5;z=0".into(),
        );

        // ① 纯 2D 画面：世界通道仍按 2D 视口剔除（性能档，既有行为不变）。
        let mut flat =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene.clone()).expect("flat");
        flat.set_world_sprite_depth(true);
        assert!(!flat.world_view_has_3d_content());
        let flat_mesh = flat.vertices(1280, 720).expect("flat vertices");
        assert!(
            flat_mesh.world_sprites.is_empty(),
            "纯 2D 画面应继续按 2D 视口剔除视口外的精灵"
        );

        // ② 同一个精灵 + 一个网格：世界视图变成 3D，世界通道不得再按 2D 相机剔除。
        let wall = crate::core::scene::SceneEntityId(2);
        scene.add_entity(wall, "Wall", None).unwrap();
        let entity = scene.entities.get_mut(&wall).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity
            .components
            .insert("MeshRenderer".into(), "mesh=__ground_plane__".into());
        let mut mixed = RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("mixed");
        mixed.set_world_sprite_depth(true);
        assert!(
            mixed.world_view_has_3d_content(),
            "场景里有 MeshRenderer 时必须认定为有 3D 内容"
        );
        let mixed_mesh = mixed.vertices(1280, 720).expect("mixed vertices");
        assert_eq!(
            mixed_mesh.world_sprites.len(),
            6,
            "有 3D 视图时世界内容不得被 2D 相机剔除（这正是被 golden 抓到的误丢）"
        );
    }

    /// docs/54 §8-①：**场景声明的发射器**要真的持续发射（而不是只被解析出来躺在组件表里）。
    ///
    /// 走的是真实固定步流水线（`run_script_steps`），因此覆盖"系统注册顺序 + 累计器写回 +
    /// 位置取世界变换 + 复用 Lua burst 实现"整条链。
    #[test]
    fn scene_declared_emitter_emits_continuously_through_the_step_pipeline() {
        let root = game_root();
        let mut scene = Scene::new("scene-emitter");
        let source = crate::core::scene::SceneEntityId(1);
        scene.add_entity(source, "Sparks", None).unwrap();
        let entity = scene.entities.get_mut(&source).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        // 60 粒子/秒、步长 1/60 秒 ⇒ 每步恰好 1 个。
        entity.components.insert(
            "ParticleEmitter".into(),
            "count=2;rate=60;speed=10;life=0.5;size=2;color=1,1,1,1".into(),
        );
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");
        let input = crate::core::input::InputSnapshot::default();
        for _ in 0..3 {
            runtime
                .run_script_steps(1, 1.0 / 60.0, &input)
                .expect("step");
        }
        assert_eq!(
            runtime.last_scene_emission(),
            1,
            "60/秒 ÷ 60 步/秒 ⇒ 每步一个粒子"
        );
        assert_eq!(
            runtime.with_script(|script| script.particle_count()),
            3,
            "三步应累计三个粒子（寿命 0.5s，期间都还在）"
        );

        // burst-only（rate = 0）的发射器不参与连续发射。
        let mut burst_scene = Scene::new("burst-only");
        let id = crate::core::scene::SceneEntityId(1);
        burst_scene.add_entity(id, "Puff", None).unwrap();
        let entity = burst_scene.entities.get_mut(&id).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity
            .components
            .insert("ParticleEmitter".into(), "count=8;life=0.5".into());
        let mut burst_runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), burst_scene).expect("runtime");
        burst_runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("step");
        assert_eq!(burst_runtime.last_scene_emission(), 0);
        assert_eq!(
            burst_runtime.with_script(|script| script.particle_count()),
            0
        );
    }

    /// docs/54 §8-①：场景声明的**正交** `Camera` 成为**初始** 2D 相机；没声明时保持历史默认。
    #[test]
    fn scene_declared_camera2d_becomes_the_initial_runtime_camera() {
        let root = game_root();
        let mut scene = Scene::new("scene-camera2d");
        let id = crate::core::scene::SceneEntityId(1);
        scene.add_entity(id, "Main2D", None).unwrap();
        scene.entities.get_mut(&id).unwrap().components.insert(
            "Camera".into(),
            "projection=orthographic;center=100,50;zoom=2".into(),
        );
        let runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");
        let camera = runtime.camera_2d();
        assert_eq!(camera.center, Vec2::new(100.0, 50.0));
        assert_eq!(camera.zoom, 2.0);

        let plain = Scene::new("no-camera");
        let default_runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), plain).expect("runtime");
        assert_eq!(
            default_runtime.camera_2d(),
            Camera2D::new(crate::core::camera::DEFAULT_2D_CAMERA_CENTER, 1.0).unwrap(),
            "没有声明相机时必须是历史默认（既有工程逐字节不变）"
        );
    }

    /// docs/54 §8-⑥：**3D 也有脚本权威**（此前只有 2D 接线，3D 的"authority"字段对 3D 无效）。
    ///
    /// 两步都断言：`Script` 下步前把 ECS 位置推给 3D 模拟、步后不回写（脚本设的位置留在 ECS）；
    /// 默认 `Physics` 下模拟结果回写 ECS（脚本的移动被模拟覆盖）。
    #[test]
    fn three_d_physics_authority_syncs_before_step_and_skips_writeback() {
        let root = game_root();
        let build = || {
            let mut scene = Scene::new("authority-3d");
            let ball = crate::core::scene::SceneEntityId(1);
            scene.add_entity(ball, "Ball", None).unwrap();
            let entity = scene.entities.get_mut(&ball).unwrap();
            entity.components.insert(
                "Transform".into(),
                "position=0,5,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
            entity.components.insert(
                "Collider3d".into(),
                "shape=ball;radius=0.5;dynamic=true".into(),
            );
            scene
        };
        let input = crate::core::input::InputSnapshot::default();
        let drop_one_step = 1.0 / 60.0;

        // ① authority = Script：步前同步（3D 计数为 1），步后不回写 ⇒ ECS 保留脚本设的位置。
        let mut scripted =
            RuntimeScene::from_scene_edit(Some(root.as_path()), build()).expect("runtime");
        scripted.set_physics_authority(PhysicsAuthority::Script);
        let handle = scripted
            .with_script(|script| script.scene_entity_handle(crate::core::scene::SceneEntityId(1)))
            .expect("handle");
        scripted.with_script(|script| {
            script.with_world_mut(|world| {
                let transform = world
                    .get_mut::<Transform3D>(handle)
                    .expect("get")
                    .expect("has transform3d");
                transform.position = Vec3::new(0.0, 9.0, 0.0);
            });
        });
        scripted
            .run_script_steps(1, drop_one_step, &input)
            .expect("step");
        assert_eq!(
            scripted.last_physics_sync(),
            (0, 1),
            "脚本权威下必须在物理步之前同步 3D 刚体（2D 场景里没有 2D 刚体，因此是 (0, 1)）"
        );
        let position = scripted
            .world_transform_3d_of(crate::core::scene::SceneEntityId(1))
            .expect("world")
            .position;
        assert!(
            (position.y - 9.0).abs() < 1e-4,
            "脚本权威下 ECS 位置不得被回写覆盖，实际 {position:?}"
        );

        // ② 默认 authority = Physics：模拟是权威 ⇒ ECS 回到模拟结果（脚本那次移动被覆盖）。
        let mut simulated =
            RuntimeScene::from_scene_edit(Some(root.as_path()), build()).expect("runtime");
        let handle = simulated
            .with_script(|script| script.scene_entity_handle(crate::core::scene::SceneEntityId(1)))
            .expect("handle");
        simulated.with_script(|script| {
            script.with_world_mut(|world| {
                let transform = world
                    .get_mut::<Transform3D>(handle)
                    .expect("get")
                    .expect("has transform3d");
                transform.position = Vec3::new(0.0, 9.0, 0.0);
            });
        });
        simulated
            .run_script_steps(1, drop_one_step, &input)
            .expect("step");
        assert_eq!(
            simulated.last_physics_sync(),
            (0, 0),
            "默认权威不做步前同步"
        );
        let position = simulated
            .world_transform_3d_of(crate::core::scene::SceneEntityId(1))
            .expect("world")
            .position;
        assert!(
            (position.y - 5.0).abs() < 0.05,
            "模拟权威下 ECS 应被回写成模拟结果（≈ 5 起落一步），实际 {position:?}"
        );
    }

    /// docs/54 §8-⑥ / §7：`PhysicsStageOrder::Legacy` 是**真回滚档**（此前是个没接线、没人读的字段）。
    #[test]
    fn legacy_stage_order_actually_changes_the_pipeline() {
        let root = game_root();
        let mut scene = Scene::new("legacy-order");
        let ball = crate::core::scene::SceneEntityId(1);
        scene.add_entity(ball, "Ball", None).unwrap();
        let entity = scene.entities.get_mut(&ball).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=0,5,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity
            .components
            .insert("Collider3d".into(), "shape=ball;radius=0.5".into());

        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");
        runtime.set_physics_authority(PhysicsAuthority::Script);
        runtime.set_physics_stage_order(PhysicsStageOrder::Legacy);
        runtime
            .run_script_steps(1, 1.0 / 60.0, &crate::core::input::InputSnapshot::default())
            .expect("step");
        assert_eq!(
            runtime.last_physics_sync(),
            (0, 0),
            "回滚档没有统一 Sync 格：脚本权威不做步前同步（与收敛前的单系统顺序一致）"
        );
    }

    /// docs/54 §8-⑦：**文档 / 运行时 / Lua** 三条路必须对同一个实体给出同一个世界位置。
    ///
    /// 这是本轮"同一空间语义"的收官断言：编辑态用 `scene_space`（文档），运行时用
    /// `RuntimeScene::world_transform_3d_of`（World + GlobalTransform），Lua 用 `transform3d.world`
    /// ——三处各算一次，任何一处退回局部值都会被这条抓住。
    #[test]
    fn document_runtime_and_lua_agree_on_the_world_transform() {
        let root = game_root();
        let rig = crate::core::scene::SceneEntityId(1);
        let child = crate::core::scene::SceneEntityId(2);
        let mut scene = Scene::new("three-way");
        // docs/54 §4.1：这条测试断言的是"父子参与空间组合"，因此场景必须**显式声明**它
        // （未声明 ⇒ 组织关系，运行时不会组合）。
        scene.metadata.insert("spatial".into(), "local".into());
        scene.add_entity(rig, "Rig", None).unwrap();
        scene.add_entity(child, "Child", Some(rig)).unwrap();
        // 统一 `Transform` 之后，空间种类由实体携带的**其它组件**推断
        //（`spatial_transform::entity_space`）：这条断言读的是 3D 世界变换（运行时 `Transform3D`
        // 与 Lua `transform3d.world`），因此父子都必须带 3D 专属组件，否则运行时会按 2D 落库。
        for id in [rig, child] {
            scene
                .entities
                .get_mut(&id)
                .unwrap()
                .components
                .insert("MeshRenderer".into(), "mesh=__ground_plane__".into());
        }
        scene.entities.get_mut(&rig).unwrap().components.insert(
            "Transform".into(),
            "position=10,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        scene.entities.get_mut(&child).unwrap().components.insert(
            "Transform".into(),
            "position=0,5,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );

        // ① 文档侧（编辑态，没有 ECS）。
        let document_world = crate::core::scene_space::world_transform_3d(&scene, child)
            .expect("document world transform");
        assert_eq!(document_world.position, Vec3::new(10.0, 5.0, 0.0));

        // ② 运行时侧（World + 传播）。
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");
        runtime.vertices(1280, 720).expect("vertices");
        let runtime_world = runtime
            .world_transform_3d_of(child)
            .expect("runtime world transform");
        assert_eq!(
            runtime_world.position, document_world.position,
            "运行时与文档的世界位置必须一致"
        );

        // ③ Lua 侧（同一实体，走 `transform3d.world`）。
        let handle = runtime
            .with_script(|script| script.scene_entity_handle(child))
            .map(crate::core::script::entity_lua_id)
            .expect("runtime entity handle");
        let source = format!(
            "function update()\n\
             local world = transform3d.world({handle})\n\
             assert(math.abs(world.x - 10) < 0.001, 'x')\n\
             assert(math.abs(world.y - 5) < 0.001, 'y')\n\
             assert(math.abs(world.z) < 0.001, 'z')\n\
             local local_pose = transform3d.get({handle})\n\
             assert(math.abs(local_pose.x) < 0.001, 'get 仍是局部值')\n\
             return 'ok' end",
        );
        let path = std::env::temp_dir().join(format!(
            "rustlua-three-way-{}-{}.lua",
            std::process::id(),
            handle
        ));
        std::fs::write(&path, source).expect("write lua script");
        runtime
            .with_script(|script| script.execute_project_script(&path))
            .expect("execute script");
        assert_eq!(
            runtime
                .with_script(|script| script.call_update())
                .expect("lua update"),
            "ok"
        );
        let _ = std::fs::remove_file(path);
    }

    /// docs/54 §8-②：「运行时只有一份权威 LocalTransform」在加载期就要可见：
    /// 坏 `parent` 边（悬空/自环）在加载期就被点名——不该只在"开启继承后逐帧传播"时才浮现。
    ///
    /// （第 57 轮起"一个实体同时带 `Transform` 与 `Transform3D`"这件事**无法再表达**：
    /// 变换只剩一个组件名，因此那条点位随之删除，见 `scene_space`。）
    #[test]
    fn loading_a_scene_reports_broken_parent_edges() {
        let root = game_root();
        let mut scene = Scene::new("parent-audit");
        let dangling = crate::core::scene::SceneEntityId(2);
        scene.add_entity(dangling, "Dangling", None).unwrap();
        scene
            .entities
            .get_mut(&dangling)
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
        // 手改成悬空父：`Scene::add_entity` 会拒绝这种边，因此只能直接写字段。
        scene.entities.get_mut(&dangling).unwrap().parent =
            Some(crate::core::scene::SceneEntityId(999));

        let runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");
        assert_eq!(
            runtime.scene_parent_issues(),
            vec!["Dangling → 不存在的父 999".to_owned()],
            "悬空父必须点名"
        );
    }

    /// docs/54 §4.2：`RenderView::visibility` 的实体侧**真的被消费**——被遮掉的实体不产顶点，
    /// 且计数能区分"被遮罩拦住"与"根本没提取到"。
    #[test]
    fn view_visibility_mask_filters_world_sprites() {
        use crate::core::visibility::ALL_LAYERS;
        let root = game_root();
        let masked = crate::core::scene::SceneEntityId(1);
        let plain = crate::core::scene::SceneEntityId(2);
        let mut scene = Scene::new("visibility-sprite");
        for (id, name) in [(masked, "Masked"), (plain, "Plain")] {
            scene.add_entity(id, name, None).unwrap();
            let entity = scene.entities.get_mut(&id).unwrap();
            // 放在默认 2D 相机中心（`DEFAULT_2D_CAMERA_CENTER`）附近：否则它们会被
            // **视口外剔除**挡掉，测出来的 0 顶点就不是遮罩造成的了。
            entity.components.insert(
                "Transform".into(),
                "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
            entity.components.insert(
                "Sprite".into(),
                "texture=assets/textures/red-1x1.png;size=2x2;pivot=0.500,0.500;\
                 tint=1.000,1.000,1.000,1.000;sorting_layer=default;order=0"
                    .into(),
            );
        }
        scene
            .entities
            .get_mut(&masked)
            .unwrap()
            .components
            .insert("Visibility".into(), "layers=0b10".into());
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");

        // 不声明视图遮罩 ⇒ 全可见，且没有任何实体被拦下（既有行为与像素不变）。
        assert_eq!(runtime.view_visibility(), (ALL_LAYERS, ALL_LAYERS));
        assert_eq!(
            runtime.vertices(320, 180).expect("vertices").vertices.len(),
            12,
            "两个精灵各 6 个顶点"
        );
        assert_eq!(runtime.view_filtered(), 0);

        // 世界视图只收 0b01：`layers=0b10` 的精灵被拦下，**没有声明 Visibility 的实体照画**。
        runtime.set_view_visibility(0b01, ALL_LAYERS);
        assert_eq!(
            runtime.vertices(320, 180).expect("vertices").vertices.len(),
            6,
            "被遮罩拦下的精灵不该产顶点"
        );
        assert_eq!(runtime.take_view_filtered(), 1, "被拦下的实体数要能报出来");
        assert_eq!(runtime.take_view_filtered(), 0, "取走即清空");

        // 只收 0b10：声明为 0b10 的实体可见；未声明的实体（全可见）同样可见。
        runtime.set_view_visibility(0b10, ALL_LAYERS);
        assert_eq!(
            runtime.vertices(320, 180).expect("vertices").vertices.len(),
            12
        );
    }

    /// docs/54 §4.2：3D 网格吃**世界**视图的遮罩（与 2D 世界内容同一条规则）。
    #[test]
    fn view_visibility_mask_filters_3d_meshes() {
        use crate::core::visibility::ALL_LAYERS;
        let root = game_root();
        let camera = crate::core::scene::SceneEntityId(1);
        let mesh = crate::core::scene::SceneEntityId(2);
        let mut scene = Scene::new("visibility-mesh");
        scene.add_entity(camera, "Cam", None).unwrap();
        scene.add_entity(mesh, "Box", None).unwrap();
        scene.entities.get_mut(&camera).unwrap().components.insert(
            "Camera".into(),
            "projection=perspective;position=0,0,5;target=0,0,0;fov=60;near=0.1;far=100".into(),
        );
        scene.entities.get_mut(&mesh).unwrap().components.insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        scene
            .entities
            .get_mut(&mesh)
            .unwrap()
            .components
            .insert("MeshRenderer".into(), "mesh=__ground_plane__".into());
        scene
            .entities
            .get_mut(&mesh)
            .unwrap()
            .components
            .insert("Visibility".into(), "layers=0b10".into());
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");

        let visible = runtime
            .scene_3d(Viewport::new(1.0, 1.0).unwrap())
            .expect("mesh visible by default");
        assert_eq!(visible.drawables.len(), 1);

        runtime.set_view_visibility(0b01, ALL_LAYERS);
        assert!(
            runtime.scene_3d(Viewport::new(1.0, 1.0).unwrap()).is_none(),
            "被世界遮罩排除的网格不该出现在提取结果里"
        );
        assert_eq!(runtime.take_view_filtered(), 1);

        runtime.set_view_visibility(0b10, ALL_LAYERS);
        assert_eq!(
            runtime
                .scene_3d(Viewport::new(1.0, 1.0).unwrap())
                .expect("mesh visible again")
                .drawables
                .len(),
            1
        );
    }

    /// docs/54 §4.2：实体身份的 UI 吃 **overlay** 遮罩（世界遮罩管不着它），
    /// 而按名字注册的画布 UI 不是实体、不受遮罩影响。
    #[test]
    fn view_visibility_mask_filters_entity_backed_ui_only() {
        use crate::core::visibility::ALL_LAYERS;
        let root = game_root();
        let panel = crate::core::scene::SceneEntityId(1);
        let mut scene = Scene::new("visibility-ui");
        scene.add_entity(panel, "Panel", None).unwrap();
        let entity = scene.entities.get_mut(&panel).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity.components.insert(
            "Image".into(),
            "texture=__white__;color=1.000,0.000,1.000".into(),
        );
        entity.components.insert(
            "UiLayout".into(),
            "anchors=0.5,0.5,0.5,0.5;offsets=-32.00,-32.00,32.00,32.00;\
             min_size=0.00,0.00;max_size=0.00,0.00;pivot=0.5000,0.5000"
                .into(),
        );
        entity
            .components
            .insert("Visibility".into(), "layers=0b01".into());
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");

        let all = runtime.vertices(320, 180).expect("vertices");
        assert_eq!(all.vertices.len(), 6, "一个 UI 面板 = 一个四边形");

        // 世界遮罩排除它、overlay 遮罩收它 ⇒ 仍然要画（UI 归 overlay 管）。
        runtime.set_view_visibility(0b10, ALL_LAYERS);
        assert_eq!(
            runtime.vertices(320, 180).expect("vertices").vertices.len(),
            6,
            "UI 不受世界遮罩影响"
        );

        // overlay 遮罩排除它 ⇒ 不产顶点，并被计数。
        runtime.set_view_visibility(ALL_LAYERS, 0b10);
        assert!(
            runtime
                .vertices(320, 180)
                .expect("vertices")
                .vertices
                .is_empty()
        );
        assert_eq!(runtime.take_view_filtered(), 1);
    }

    /// docs/54 §8-⑥：3D collider 调试绘制必须真的把**物理世界当前姿态**投影成 overlay 线框，
    /// 且用与 3D 主视图同一份投影数学（`picking::screen_from_world_unclipped`）。
    #[test]
    fn collider_debug_draw_3d_projects_the_live_physics_pose() {
        let root = game_root();
        let mut scene = Scene::new("collider-debug-3d");
        let cam = crate::core::scene::SceneEntityId(1);
        let wall = crate::core::scene::SceneEntityId(2);
        scene.add_entity(cam, "Cam", None).unwrap();
        scene.add_entity(wall, "Wall", None).unwrap();
        scene.entities.get_mut(&cam).unwrap().components.insert(
            "Camera".into(),
            "projection=perspective;position=0,0,5;target=0,0,0;fov=60;near=0.1;far=100".into(),
        );
        scene.entities.get_mut(&wall).unwrap().components.insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        scene.entities.get_mut(&wall).unwrap().components.insert(
            "Collider3d".into(),
            "shape=cuboid;half_width=1;half_height=1;half_z=1;dynamic=false".into(),
        );
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");
        let overlay_draws = |mesh: &SceneMesh| -> usize {
            mesh.draws
                .iter()
                .filter(|draw| draw.render_layer == RenderLayer::Overlay2D)
                .count()
        };

        // 关着的时候一条调试线都没有（这个场景里没有精灵/UI，因此顶点缓冲整体为空）。
        let off = runtime.vertices(320, 180).expect("vertices");
        assert_eq!(overlay_draws(&off), 0, "默认不画调试线框");
        assert!(off.vertices.is_empty(), "该场景没有其它 2D 内容");

        // 打开：固定体的 12 条棱各 6 个顶点，合成**一个** draw range（同一张白纹理）。
        runtime.set_collider_debug_draw_3d(true);
        let on = runtime.vertices(320, 180).expect("vertices");
        assert_eq!(overlay_draws(&on), 1, "调试线框合成一个 draw range");
        assert_eq!(on.vertices.len(), 12 * 6, "cuboid = 12 条棱 × 6 顶点");
        for vertex in &on.vertices {
            let [x, y] = vertex.position;
            assert!(x.is_finite() && y.is_finite(), "投影必须有限");
            assert!(
                x.abs() <= 1.0 && y.abs() <= 1.0,
                "居中的单位立方体必须投到视口内：({x}, {y})"
            );
        }
        // 关掉之后立刻回到"没有线"——开关是每帧的，不留残影。
        runtime.set_collider_debug_draw_3d(false);
        let off_again = runtime.vertices(320, 180).expect("vertices");
        assert_eq!(overlay_draws(&off_again), 0);
    }

    #[test]
    fn camera_pose_follows_the_parent_chain() {
        let root = game_root();
        let mut scene = Scene::new("camera-under-parent");
        let rig = crate::core::scene::SceneEntityId(1);
        let cam = crate::core::scene::SceneEntityId(2);
        let ground = crate::core::scene::SceneEntityId(3);
        scene.add_entity(rig, "Rig", None).unwrap();
        scene.add_entity(cam, "Cam", Some(rig)).unwrap();
        scene.add_entity(ground, "Ground", None).unwrap();
        // 相机姿态取的是**世界**变换（`world_transform_3d_lazy` 沿父链组合），父实体因此也必须
        // 落成 3D：空间种类由实体携带的其它组件推断（`spatial_transform::entity_space`），
        // 没有 3D 专属组件的父会按 2D 落库，组合会在这里断掉。
        scene
            .entities
            .get_mut(&rig)
            .unwrap()
            .components
            .insert("MeshRenderer".into(), "mesh=__ground_plane__".into());
        scene.entities.get_mut(&rig).unwrap().components.insert(
            "Transform".into(),
            "position=10,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        scene.entities.get_mut(&cam).unwrap().components.insert(
            "Transform".into(),
            "position=0,0,6;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        scene.entities.get_mut(&cam).unwrap().components.insert(
            "Camera".into(),
            "projection=perspective;position=0,0,99;target=0,0,0;fov=60;near=0.1;far=100;pose=transform".into(),
        );
        // `scene_3d` 需要至少一个 drawable 才会返回 Some。
        scene.entities.get_mut(&ground).unwrap().components.insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        scene
            .entities
            .get_mut(&ground)
            .unwrap()
            .components
            .insert("MeshRenderer".into(), "mesh=__ground_plane__".into());

        let runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");
        let camera = runtime
            .scene_3d(Viewport::new(16.0, 9.0).unwrap())
            .expect("3d scene")
            .camera;
        assert!(
            (camera.position.x - 10.0).abs() < 1e-4 && (camera.position.z - 6.0).abs() < 1e-4,
            "相机姿态应取世界变换（父 +10 x 再局部 z=6），实际 {:?}",
            camera.position
        );
    }
    /// WP2 的关键验收：**迁移无损**——对同一份"子按世界坐标写"的场景跑真实迁移
    /// （`scene_migration::migrate_scene_file`，含写回与读回校验），再把**迁移后的文件**
    /// 读回来在**开启空间继承**下渲染；两帧顶点必须一致。这条把"迁移 + 继承"的效果钉在
    /// **顶点级**证据上，并且钉的是**落盘之后**的数据——内存里换一组数值的等价不算数。
    #[test]
    fn migrated_child_renders_at_the_same_place_with_inheritance_enabled() {
        let root = game_root();
        let dir = std::env::temp_dir().join(format!(
            "rustlua-migration-vertex-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("hierarchy.scene");

        let mut scene = crate::core::scene::Scene::new("hierarchy-render");
        let parent = crate::core::scene::SceneEntityId(1);
        let child = crate::core::scene::SceneEntityId(2);
        scene.add_entity(parent, "Parent", None).unwrap();
        scene.add_entity(child, "Child", Some(parent)).unwrap();
        // 相机默认中心是 (640, 360)，精灵放这附近才不会被视口剔除掉（剔除是渲染路径的一部分）。
        for (id, x, scale, texture) in [
            (parent, "640", "2", "assets/textures/player.png"),
            (child, "650", "1", "assets/textures/coin.png"),
        ] {
            let entity = scene.entities.get_mut(&id).unwrap();
            entity.components.insert(
                "Transform".into(),
                format!("position={x},360,0;rotation=0,0,0,1;scale={scale},{scale},1"),
            );
            entity.components.insert(
                "Sprite".into(),
                format!("texture={texture};size=16x16;pivot=0.5,0.5"),
            );
        }
        scene.save(&path).expect("save legacy scene");

        // A：旧数据（子按世界坐标写）+ 关闭继承 = 迁移前的画面。
        let mut legacy =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene.clone()).expect("legacy");
        // A 档刻意关掉继承：它代表"迁移前的数据 + 迁移前的语义"。
        legacy.set_spatial_inheritance(false);
        assert!(!legacy.spatial_active(), "关掉后不应再有传播");
        let legacy_mesh = legacy.vertices(640, 360).expect("legacy vertices");
        assert!(
            !legacy_mesh.vertices.is_empty(),
            "样例场景应画出精灵（否则这条测试是空转）"
        );

        // B：跑真实**文件级**迁移（写回 + 读回校验），再把迁移后的文件读回来开继承渲染。
        let outcome = crate::core::scene_migration::migrate_scene_file(
            &path,
            crate::core::scene_migration::MigrationOptions {
                write: true,
                backup: false,
            },
        )
        .expect("migrate");
        assert!(
            outcome.written && outcome.verified,
            "必须真的写盘并读回校验：{outcome:?}"
        );
        assert!(outcome.is_lossless(), "这份两层场景应无损迁移：{outcome:?}");
        assert_eq!(
            outcome.converted, 1,
            "只有一个子实体需要改成局部坐标：{outcome:?}"
        );
        let migrated_scene =
            crate::core::scene::Scene::load(&path).expect("load migrated scene file");
        let mut migrated =
            RuntimeScene::from_scene_edit(Some(root.as_path()), migrated_scene).expect("migrated");
        migrated.set_spatial_inheritance(true);
        assert!(
            migrated.last_spatial_report().diagnostics.is_empty(),
            "{:?}",
            migrated.last_spatial_report()
        );
        let migrated_mesh = migrated.vertices(640, 360).expect("migrated vertices");

        let positions = |mesh: &SceneMesh| {
            mesh.vertices
                .iter()
                .map(|vertex| (vertex.position, vertex.uv))
                .collect::<Vec<_>>()
        };
        let legacy_positions = positions(&legacy_mesh);
        let migrated_positions = positions(&migrated_mesh);
        assert_eq!(
            legacy_positions.len(),
            migrated_positions.len(),
            "两帧顶点数必须相同（迁移不得多画/少画）"
        );
        for (index, (left, right)) in legacy_positions.iter().zip(&migrated_positions).enumerate() {
            assert!(
                (left.0[0] - right.0[0]).abs() < 1e-4 && (left.0[1] - right.0[1]).abs() < 1e-4,
                "第 {index} 个顶点位置不一致：{:?} vs {:?}",
                left.0,
                right.0
            );
            assert_eq!(left.1, right.1, "第 {index} 个顶点 UV 不一致");
        }

        std::fs::remove_dir_all(&dir).expect("clean temp dir");
    }
    /// WP6：绘制分段必须带上**正确的渲染层**——世界文字/精灵/瓦片是世界内容，
    /// 屏幕 UI 文本是 `ScreenUI`。这条把"哪段几何属于哪一层"从"看谁调了哪个函数"
    /// 变成可断言的事实（世界 Sprite 深度通道只吃世界层，靠的就是这个区分）。
    #[test]
    fn draw_ranges_carry_the_render_layer_of_their_producer() {
        let root = game_root();
        let build = |name: &str, component: (&str, &str)| {
            let mut scene = crate::core::scene::Scene::new(name);
            let id = crate::core::scene::SceneEntityId(1);
            scene.add_entity(id, "E", None).unwrap();
            let entity = scene.entities.get_mut(&id).unwrap();
            entity.components.insert(
                "Transform".into(),
                "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
            entity
                .components
                .insert(component.0.into(), component.1.into());
            scene
        };

        // 世界文字：世界内容。
        let mut world_runtime = RuntimeScene::from_scene_edit(
            Some(root.as_path()),
            build(
                "world-text-layer",
                ("WorldText", "text=HP;font_size=16;color=1,1,1;offset=0,0"),
            ),
        )
        .expect("runtime");
        // 本用例断言的是 **SDF 字形**分段，因此自己钉住渲染模式：`game_api/project.toml`
        // 现为 `text_render_mode = "vector"`（示例工程刻意走矢量轮廓），`auto` 也会因
        // 字号 < 2.0 之外的启发式而漂移。依赖工程配置会让这条测试随工程设置变红。
        world_runtime.set_text_render_mode(TextRenderMode::Sdf);
        let world_mesh = world_runtime.vertices(640, 360).expect("vertices");
        assert!(!world_mesh.draws.is_empty(), "世界文字应产出绘制分段");
        assert!(
            world_mesh
                .draws
                .iter()
                .all(|draw| draw.render_layer == crate::core::layer::RenderLayer::World2D),
            "世界文字必须标成 World2D：{:?}",
            world_mesh
                .draws
                .iter()
                .map(|draw| draw.render_layer)
                .collect::<Vec<_>>()
        );

        // 屏幕 UI 文本：ScreenUI（不受世界相机影响、不参与深度）。
        // 屏幕 UI 要参与布局才有绘制分段，因此这里显式给一个 `UiLayout`。
        let mut ui_scene = crate::core::scene::Scene::new("ui-text-layer");
        let ui_id = crate::core::scene::SceneEntityId(1);
        ui_scene.add_entity(ui_id, "E", None).unwrap();
        let ui_entity = ui_scene.entities.get_mut(&ui_id).unwrap();
        ui_entity.components.insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        ui_entity.components.insert(
            "UiLayout".into(),
            "anchors=0,0,0,0;offsets=0,0,160,24".into(),
        );
        ui_entity
            .components
            .insert("Text".into(), "text=Menu;font_size=16;color=1,1,1".into());
        let mut ui_runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), ui_scene).expect("runtime");
        // 同上：本用例断言 SDF 路径，不继承工程配置的 `vector`。
        ui_runtime.set_text_render_mode(TextRenderMode::Sdf);
        let ui_mesh = ui_runtime.vertices(640, 360).expect("vertices");
        assert!(!ui_mesh.draws.is_empty(), "屏幕 UI 文本应产出绘制分段");
        assert!(
            ui_mesh
                .draws
                .iter()
                .all(|draw| draw.render_layer == crate::core::layer::RenderLayer::ScreenUI),
            "屏幕 UI 文本必须标成 ScreenUI：{:?}",
            ui_mesh
                .draws
                .iter()
                .map(|draw| draw.render_layer)
                .collect::<Vec<_>>()
        );
    }
    /// WP2 ④：编辑器拾取用的"按场景实体 id 读世界变换"必须与渲染同源——带父子关系时给出
    /// **组合后**的位置，没有父子关系时给出局部值（两种情形都不需要调用方分支）。
    #[test]
    fn scene_entity_world_transform_follows_the_parent_chain() {
        let root = game_root();
        let mut scene = crate::core::scene::Scene::new("editor-picking");
        // docs/54 §4.1：断言的是父链组合，所以场景显式声明局部空间语义。
        scene.metadata.insert("spatial".into(), "local".into());
        let parent = crate::core::scene::SceneEntityId(1);
        let child = crate::core::scene::SceneEntityId(2);
        let lone = crate::core::scene::SceneEntityId(3);
        scene.add_entity(parent, "Parent", None).unwrap();
        scene.add_entity(child, "Child", Some(parent)).unwrap();
        scene.add_entity(lone, "Lone", None).unwrap();
        scene.entities.get_mut(&parent).unwrap().components.insert(
            "Transform".into(),
            "position=10,5,0;rotation=0,0,0,1;scale=2,2,1".into(),
        );
        scene.entities.get_mut(&child).unwrap().components.insert(
            "Transform".into(),
            "position=-3.5,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        scene.entities.get_mut(&lone).unwrap().components.insert(
            "Transform".into(),
            "position=7,9,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
        runtime.propagate_spatial_now();

        let world = runtime
            .world_transform_2d_of(child)
            .expect("child world transform");
        assert!(
            (world.position.x - 3.0).abs() < 1e-3 && (world.position.y - 5.0).abs() < 1e-3,
            "子实体应给出组合后的世界位置 (3, 5)：{world:?}"
        );
        assert!((world.scale.x - 2.0).abs() < 1e-3, "父缩放应作用到子");
        // 没有父的实体：世界值 = 局部值（编辑器因此不需要分支）。
        let alone = runtime
            .world_transform_2d_of(lone)
            .expect("lone world transform");
        assert!((alone.position.x - 7.0).abs() < 1e-3 && alone.scale.x == 1.0);
        // 未知场景实体：`None`（而不是编一个 0 位置的框）。
        assert!(
            runtime
                .world_transform_2d_of(crate::core::scene::SceneEntityId(99))
                .is_none()
        );
    }
    /// WP7：`PhysicsAuthority` 决定"谁有权写 Transform"。
    ///
    /// 场景：动态刚体在原点、零重力、无速度；**ECS 侧**把 Transform 改成 (100, 0)（等价于脚本
    /// 直接改 ECS，而不是改刚体）。两种权威下的结果必须不同：
    /// - `Physics`（默认）：模拟是权威 ⇒ 回写把 ECS 改回刚体的位置 0（脚本的位置被覆盖）；
    /// - `Script`：ECS 是权威 ⇒ 步前同步把 100 推给刚体、步后不回写 ⇒ ECS 保持 100。
    ///
    /// 这条同时证明「同步」与「回写」两格真的接在流水线上（而不只是注释里的顺序）。
    #[test]
    fn physics_authority_decides_who_owns_the_transform() {
        let root = game_root();
        let mut scene = crate::core::scene::Scene::new("authority");
        let id = crate::core::scene::SceneEntityId(1);
        scene.add_entity(id, "Crate", None).unwrap();
        let entity = scene.entities.get_mut(&id).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity
            .components
            .insert("Collider".into(), "shape=ball;radius=4".into());
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
        let moved = runtime.script.with_world(|world| {
            world
                .entities_with::<Transform>()
                .into_iter()
                .next()
                .expect("crate entity")
        });
        let input = InputSnapshot::default();

        // 默认：模拟是权威 ⇒ 脚本直接改 ECS 的位置会被回写覆盖。
        assert_eq!(runtime.physics_authority(), PhysicsAuthority::Physics);
        runtime.script.with_world_mut(|world| {
            world.get_mut::<Transform>(moved).unwrap().unwrap().position = Vec2::new(100.0, 0.0);
        });
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("steps");
        let after_physics = runtime
            .script
            .with_world(|world| world.get::<Transform>(moved).unwrap().unwrap().position);
        assert!(
            after_physics.x.abs() < 1e-3,
            "Physics 权威下模拟结果应写回 ECS（刚体仍在原点）：{after_physics:?}"
        );

        // Script 权威：ECS 是权威 ⇒ 步前同步、步后不回写，位置保持脚本写下的值。
        runtime.set_physics_authority(PhysicsAuthority::Script);
        runtime.script.with_world_mut(|world| {
            world.get_mut::<Transform>(moved).unwrap().unwrap().position = Vec2::new(100.0, 0.0);
        });
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("steps");
        let after_script = runtime
            .script
            .with_world(|world| world.get::<Transform>(moved).unwrap().unwrap().position);
        assert!(
            (after_script.x - 100.0).abs() < 1e-3,
            "Script 权威下 ECS 位置必须保留（步前同步、步后不回写）：{after_script:?}"
        );
    }

    /// WP2 + docs/54 §4.1：空间继承**默认由场景声明决定**，宿主开关只是覆盖。
    ///
    /// 场景声明 `spatial=local` ⇒ 默认就组合父子；显式关掉 ⇒ 退回局部值（回滚档）；再打开 ⇒
    /// 与第一次完全一致。未声明的场景默认是**组织关系**（另一条测试钉住）。
    #[test]
    fn spatial_inheritance_switches_between_local_and_world_positions() {
        let root = game_root();
        let mut scene = crate::core::scene::Scene::new("hierarchy");
        scene.metadata.insert("spatial".into(), "local".into());
        let parent = crate::core::scene::SceneEntityId(1);
        let child = crate::core::scene::SceneEntityId(2);
        scene.add_entity(parent, "Parent", None).unwrap();
        scene.add_entity(child, "Child", Some(parent)).unwrap();
        scene.entities.get_mut(&parent).unwrap().components.insert(
            "Transform".into(),
            "position=10,5,0;rotation=0,0,0,1;scale=2,2,1".into(),
        );
        // 迁移后的局部值：世界 x=3 ⇒ 局部 (3-10)/2 = -3.5。
        scene.entities.get_mut(&child).unwrap().components.insert(
            "Transform".into(),
            "position=-3.5,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
        let child_entity = runtime.script.with_world(|world| {
            world.entities_with::<Transform>().into_iter().find(|id| {
                world
                    .get::<crate::core::hierarchy::Parent>(*id)
                    .ok()
                    .flatten()
                    .is_some()
            })
        });
        let child_entity = child_entity.expect("child entity has a Parent component");

        let read_world = |runtime: &RuntimeScene| {
            runtime
                .script
                .with_world(|world| crate::core::hierarchy::world_transform_2d(world, child_entity))
                .unwrap()
                .expect("child transform")
        };

        // 场景声明了 `spatial=local`：默认就开，传播真的在跑，世界位置 = 父 ∘ 子 = (3, 5)。
        assert!(runtime.spatial_inheritance());
        runtime.propagate_spatial_now();
        assert!(runtime.spatial_active());
        assert!(runtime.last_spatial_report().diagnostics.is_empty());
        let composed = read_world(&runtime);
        assert!(
            (composed.position.x - 3.0).abs() < 1e-3 && (composed.position.y - 5.0).abs() < 1e-3,
            "开启继承后世界位置应为 (3, 5)：{composed:?}"
        );
        assert!((composed.scale.x - 2.0).abs() < 1e-3, "父缩放应作用到子");

        // 回滚档：关掉继承 ⇒ 世界值退回局部值（-3.5, 0），且不再有传播成本。
        runtime.set_spatial_inheritance(false);
        assert!(!runtime.spatial_active());
        let local = read_world(&runtime);
        assert!(
            (local.position.x + 3.5).abs() < 1e-3,
            "关掉继承后应退回局部值：{local:?}"
        );

        // 再打开：值与第一次开启时完全一致（开关可反复切换）。
        runtime.set_spatial_inheritance(true);
        assert_eq!(read_world(&runtime), composed);
    }

    /// docs/54 §4.1：**未声明空间语义的 v2 场景里，`parent` 只是组织关系**。
    ///
    /// 这条是本批的核心判据：此前运行时开关默认是开的，于是任何带 `parent` 边的 v2 场景都会被
    /// 自动按父子组合解释——旧场景（`parent` 只用于编辑器分组）会因此**静默改变画面**。
    /// 现在默认值来自场景自身：未声明 ⇒ 不组合，子实体读到的就是它自己的局部值。
    #[test]
    fn unmarked_v2_scenes_treat_parent_as_organizational_only() {
        let root = game_root();
        let parent = crate::core::scene::SceneEntityId(1);
        let child = crate::core::scene::SceneEntityId(2);
        let mut scene = crate::core::scene::Scene::new("organizational");
        scene.add_entity(parent, "Group", None).unwrap();
        scene.add_entity(child, "Member", Some(parent)).unwrap();
        scene.entities.get_mut(&parent).unwrap().components.insert(
            "Transform".into(),
            "position=10,5,0;rotation=0,0,0,1;scale=2,2,1".into(),
        );
        scene.entities.get_mut(&child).unwrap().components.insert(
            "Transform".into(),
            "position=1,2,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        let runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");

        assert_eq!(
            runtime.spatial_mode(),
            crate::core::scene_space::SpatialMode::Organizational,
            "未声明时场景语义应是组织关系"
        );
        assert!(
            !runtime.spatial_inheritance(),
            "未声明的 v2 场景默认不该建立父子空间继承"
        );
        let child_world = runtime
            .world_transform_2d_of(child)
            .expect("child world transform");
        assert!(
            (child_world.position.x - 1.0).abs() < 1e-6
                && (child_world.position.y - 2.0).abs() < 1e-6
                && (child_world.scale.x - 1.0).abs() < 1e-6,
            "组织关系下子实体的世界值就是它自己的局部值：{child_world:?}"
        );
        // 但 `parent` 边本身仍然落到 ECS 上（层级信息不丢，只是不参与空间组合）。
        let has_parent = runtime.script.with_world(|world| {
            world
                .entities_with::<crate::core::hierarchy::Parent>()
                .len()
        });
        assert_eq!(has_parent, 1, "组织关系不等于丢掉父子边");
    }

    /// docs/54 §4.1：显式声明与非法声明各有一条出口——`spatial=organizational` 等同未声明，
    /// 认不出的值按默认处理但**会被点名**（不静默吞掉）。
    #[test]
    fn explicit_and_invalid_spatial_declarations_are_reported() {
        let mut scene = crate::core::scene::Scene::new("declared");
        scene
            .metadata
            .insert("spatial".into(), "organizational".into());
        assert_eq!(
            crate::core::scene_space::spatial_mode(&scene),
            crate::core::scene_space::SpatialMode::Organizational
        );
        assert!(crate::core::scene_space::spatial_mode_issue(&scene).is_none());
        scene.metadata.insert("spatial".into(), "inherit".into());
        assert_eq!(
            crate::core::scene_space::spatial_mode(&scene),
            crate::core::scene_space::SpatialMode::Inherit
        );
        scene.metadata.insert("spatial".into(), "world".into());
        assert_eq!(
            crate::core::scene_space::spatial_mode(&scene),
            crate::core::scene_space::SpatialMode::Organizational,
            "认不出的值按 v2 默认处理"
        );
        let issue =
            crate::core::scene_space::spatial_mode_issue(&scene).expect("非法取值必须被点名");
        assert!(issue.contains("spatial=world"), "{issue}");
    }

    /// WP2：运行时的 `Parent` 组件来自场景的 `parent`（父子关系真的落到了 ECS 上）。
    #[test]
    fn scene_parents_become_runtime_parent_components() {
        let root = game_root();
        let mut scene = crate::core::scene::Scene::new("parents");
        let parent = crate::core::scene::SceneEntityId(1);
        let child = crate::core::scene::SceneEntityId(2);
        let orphan = crate::core::scene::SceneEntityId(3);
        scene.add_entity(parent, "Parent", None).unwrap();
        scene.add_entity(child, "Child", Some(parent)).unwrap();
        scene.add_entity(orphan, "Orphan", None).unwrap();
        for (id, x) in [(parent, 0.0_f32), (child, 4.0), (orphan, 8.0)] {
            scene.entities.get_mut(&id).unwrap().components.insert(
                "Transform".into(),
                format!("position={x},0,0;rotation=0,0,0,1;scale=1,1,1"),
            );
        }
        let runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
        let with_parent = runtime.script.with_world(|world| {
            world
                .entities_with::<crate::core::hierarchy::Parent>()
                .len()
        });
        assert_eq!(with_parent, 1, "只有子实体带 Parent");
    }
    /// WP5：宿主交给渲染器的 2D overlay 视图必须与 `vertices()` 用**同一个相机**——
    /// 收敛前"取景"（相机）与"清屏/视口"（渲染器写死）是两套说法，这里把它钉住。
    #[test]
    fn render_view_2d_shares_the_runtime_camera() {
        let mut scene = Scene::new("render-view");
        scene.add_entity(SceneEntityId(1), "Empty", None).unwrap();
        let runtime = RuntimeScene::from_scene_edit(None, scene).expect("runtime");
        let mut runtime = runtime;
        runtime.set_camera_2d(Camera2D::new(Vec2::new(120.0, 80.0), 2.0).expect("camera"));

        let view = runtime.render_view_2d(640, 360);
        assert_eq!(view.viewport, Viewport::new(640.0, 360.0).unwrap());
        assert_eq!(
            view.projection,
            crate::core::camera::Projection::Orthographic { zoom: 2.0 },
            "视图的投影必须来自运行时相机的 zoom"
        );
        // 2D overlay 是叠加层：不清屏（3D 主视图才清屏）。
        assert_eq!(view.clear, crate::core::camera::ClearPolicy::None);
        assert!((view.aspect() - 640.0 / 360.0).abs() < 1e-6);
        assert_eq!(
            runtime.camera_2d().center,
            runtime.camera_center_2d(),
            "视图相机与宿主相机是同一份状态"
        );

        // 0 尺寸（最小化窗口）不能造出非法视口。
        assert_eq!(
            runtime.render_view_2d(0, 0).viewport,
            Viewport::new(1.0, 1.0).unwrap()
        );
    }

    /// 运行态 3D drawable 使用序列化场景的实体名称（而非 `Entity {index}`）；
    /// 未命名的脚本实体才回退到索引名。
    #[test]
    fn runtime_drawables_use_scene_names() {
        let root = game_root();
        if !root.join("scenes/demo3d.scene").is_file() {
            return;
        }
        let path = root.join("scenes/demo3d.scene");
        let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load demo3d: {e}"));
        let runtime = RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime demo3d");
        let scene3d = runtime
            .scene_3d(Viewport::new(16.0, 9.0).unwrap())
            .expect("3d scene");
        let mut names: Vec<&str> = scene3d.drawables.iter().map(|d| d.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["Ground", "ShadowTriangle", "SkeletonWarrior"]);
        assert!(
            scene3d
                .drawables
                .iter()
                .all(|d| !d.name.starts_with("Entity ")),
            "drawable 名称应来自场景实体名，而非索引"
        );
    }

    /// 示例工程的 **场景 UI 场景**（`ui_test.scene`）要真的产出可见 UI draw，
    /// 并且场景侧的 UI 组件（`UiEvents` / `Image` / `Text`）都在运行时世界里。
    #[test]
    fn sample_project_ui_scene_instantiates_ui() {
        let root = production_sample_root();
        if !root.join("scenes/ui_test.scene").is_file() {
            return;
        }
        let path = root.join("scenes/ui_test.scene");
        let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load ui_test: {e}"));
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime ui_test");
        let mesh = runtime.vertices(1280, 720).expect("vertices");
        let counts = runtime.script.with_world(|world| {
            (
                world
                    .entities_with::<crate::core::ui_events::UiEvents>()
                    .len(),
                world.entities_with::<UiImage>().len(),
                world.entities_with::<UiText>().len(),
            )
        });
        assert!(
            counts.0 >= 2,
            "ui_test 场景的按钮是 UiEvents 实体，实际 {}",
            counts.0
        );
        assert!(counts.1 >= 1, "ui_test 场景应有 Image 实体");
        assert!(counts.2 >= 3, "ui_test 场景应有标题/按钮/HUD 文本");
        assert!(
            !mesh.draws.is_empty(),
            "ui_test 场景应产出可见的 2D/UI draw"
        );
    }

    /// 示例工程的 **玩法场景**（`api_gameplay.scene`）要带着它的地块、演员与世界文字
    /// 实例化出来，并真的渲染：这是"场景 + 脚本 + 渲染"三层的端到端样本。
    #[test]
    fn sample_project_gameplay_scene_instantiates_gameplay_entities() {
        let root = production_sample_root();
        if !root.join("scenes/api_gameplay.scene").is_file() {
            return;
        }
        let path = root.join("scenes/api_gameplay.scene");
        let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load api_gameplay: {e}"));
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime api_gameplay");
        let input = InputSnapshot::default();
        runtime
            .run_script_steps(1, 1.0 / 60.0, &input)
            .expect("script step");
        let mesh = runtime.vertices(1280, 720).expect("vertices");
        let counts = runtime.script.with_world(|world| {
            (
                world.entities_with::<crate::core::tilemap::Tilemap>().len(),
                world
                    .entities_with::<crate::core::sprite::SpriteAsset>()
                    .len(),
                world
                    .entities_with::<crate::core::world_text::WorldText>()
                    .len(),
                world
                    .entities_with::<crate::core::ui_events::UiEvents>()
                    .len(),
            )
        });
        assert_eq!(counts.0, 1, "玩法场景应有一块地块（Tilemap）");
        assert!(counts.1 >= 3, "玩法场景应有玩家/箱子/商人等演员");
        assert!(counts.2 >= 1, "玩法场景应有世界文字（FieldSign）");
        assert_eq!(counts.3, 0, "玩法场景不声明场景 UI 事件");
        assert!(
            runtime.texture_paths.len() >= 3,
            "玩法场景应加载地块/玩家/史莱姆纹理，实际 {:?}",
            runtime.texture_paths
        );
        assert!(
            mesh.draws.len() >= 3,
            "玩法场景应渲染地块与演员，实际 {} 个 draw",
            mesh.draws.len()
        );
    }

    /// 同一输入 scene 在静态 serialized extractor 与初始 runtime world 上：
    /// 每个 serialized 3D 实体都能找到同名、同 mesh path、同 transform 的
    /// runtime drawable；活动相机/光源与 serialized 的首个相机/光源一致
    /// （无 Lua 时 runtime 取首个组件实体）。物理/脚本/动画 palette 属运行态
    /// 差异，不在本测试断言逐帧相等。
    #[test]
    fn serialized_and_runtime_consistency_demo3d() {
        let root = game_root();
        if !root.join("scenes/demo3d.scene").is_file() {
            return;
        }
        let path = root.join("scenes/demo3d.scene");
        let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load demo3d: {e}"));
        let serialized = crate::core::scene_extract::extract_serialized_scene_3d(&scene);
        let runtime = RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime demo3d");
        let runtime3d = runtime
            .scene_3d(Viewport::new(16.0, 9.0).unwrap())
            .expect("3d scene");

        // serialized extractor 应有可断言的有效实体；合法性一致时无诊断。
        assert_eq!(serialized.entities.len(), runtime3d.drawables.len());
        assert!(serialized.diagnostics.is_empty());
        for serialized_entity in &serialized.entities {
            let drawable = runtime3d
                .drawables
                .iter()
                .find(|d| d.name == serialized_entity.name)
                .unwrap_or_else(|| panic!("runtime 缺少同名实体 {}", serialized_entity.name));
            assert_eq!(
                drawable.mesh_path, serialized_entity.mesh.mesh,
                "{} 的 mesh path 应一致",
                serialized_entity.name
            );
            for (row, col) in (0..4).flat_map(|r| (0..4).map(move |c| (r, c))) {
                let runtime_value = drawable.model.columns[col][row];
                let serialized_value = serialized_entity.model.columns[col][row];
                assert!(
                    (runtime_value - serialized_value).abs() < 1e-4,
                    "{} model[{col}][{row}] 不一致: {runtime_value} vs {serialized_value}",
                    serialized_entity.name
                );
            }
        }

        // 活动相机 = 首个 Camera3D 实体（from_scene_edit 无 Lua 选择），与 serialized 首个相机
        // **逐字段相同**：拆分批 7 之后宿主不往相机里写任何量（此前会把传入的 16:9 写回
        // `aspect_ratio`，于是"组件里存的比例"与"渲染/拾取实际用的比例"成了两份状态）。
        assert!(!serialized.cameras.is_empty(), "demo3d 应有相机");
        let serialized_camera = &serialized.cameras[0].1;
        let runtime_camera = &runtime3d.camera;
        assert_eq!(runtime_camera.position, serialized_camera.position);
        assert_eq!(runtime_camera.target, serialized_camera.target);
        assert_eq!(runtime_camera.up, serialized_camera.up);
        assert!((runtime_camera.fov_y - serialized_camera.fov_y).abs() < 1e-6);
        assert!((runtime_camera.near - serialized_camera.near).abs() < 1e-6);
        assert!((runtime_camera.far - serialized_camera.far).abs() < 1e-6);
        assert_eq!(
            runtime_camera.pose_from_transform,
            serialized_camera.pose_from_transform
        );
        assert_eq!(runtime_camera.enabled, serialized_camera.enabled);
        // 比例的唯一去处是**本帧视图**：传入 16:9 视口 ⇒ 视图比例就是 16:9，相机载荷一字未改。
        assert!(
            (runtime3d.view.aspect() - 16.0 / 9.0).abs() < 1e-6,
            "视图比例必须来自视口：{}",
            runtime3d.view.aspect()
        );

        // 活动光源 = 首个 Light 实体，与 serialized 首个光源一致。
        assert!(!serialized.lights.is_empty(), "demo3d 应有光源");
        let serialized_light = &serialized.lights[0].1;
        let runtime_light = &runtime3d.light;
        assert_eq!(runtime_light.direction, serialized_light.direction);
        assert_eq!(runtime_light.color, serialized_light.color);
        assert_eq!(runtime_light.intensity, serialized_light.intensity);
    }

    /// WP5：运行时的相机姿态也可以来自实体的 `Transform3D`（`pose=transform`）。
    /// 同一场景里放两台相机（一台跟随变换、一台 look-at），验证**只有声明的那台**改姿态。
    #[test]
    fn runtime_camera_pose_follows_transform_when_declared() {
        let root = game_root();
        let mut scene = crate::core::scene::Scene::new("camera-pose");
        let posed = crate::core::scene::SceneEntityId(1);
        let plain = crate::core::scene::SceneEntityId(2);
        let mesh = crate::core::scene::SceneEntityId(3);
        scene.add_entity(posed, "PosedCamera", None).unwrap();
        scene.add_entity(plain, "PlainCamera", None).unwrap();
        scene.add_entity(mesh, "Mesh", None).unwrap();
        // `scene_3d` 以 drawable 为准：没有网格实体时直接返回 None，放一个网格当锚点。
        scene.entities.get_mut(&mesh).unwrap().components.insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        scene
            .entities
            .get_mut(&mesh)
            .unwrap()
            .components
            .insert("MeshRenderer".into(), "mesh=assets/models/test.glb".into());
        scene.entities.get_mut(&posed).unwrap().components.insert(
            "Transform".into(),
            "position=20,5,0;rotation=0,0.7071068,0,0.7071068;scale=1,1,1".into(),
        );
        scene.entities.get_mut(&posed).unwrap().components.insert(
            "Camera".into(),
            "projection=perspective;position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100;pose=transform".into(),
        );
        scene.entities.get_mut(&plain).unwrap().components.insert(
            "Camera".into(),
            "projection=perspective;position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100".into(),
        );
        let runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
        let extracted = runtime
            .scene_3d(Viewport::new(16.0, 9.0).unwrap())
            .expect("3d scene");
        // 活动相机 = 排序序第一个（实体 1，即声明了 pose=transform 的那台）。
        let camera = extracted.camera;
        assert!(
            (camera.position.x - 20.0).abs() < 1e-3,
            "姿态应来自 Transform3D 的位置：{:?}",
            camera.position
        );
        // 绕 Y 转 90° ⇒ 视线朝 -X。
        let forward = (camera.target - camera.position).normalized().unwrap();
        assert!(
            (forward - Vec3::new(-1.0, 0.0, 0.0)).length() < 1e-3,
            "视线应来自变换的 -Z：{forward:?}"
        );
        // 未声明的那台（实体 2）不在 `Scene3d` 里——运行帧只带活动相机；它的 look-at 姿态
        // 由 `scene_extract` 的对拍测试覆盖（那里两台相机都在列表中，只有声明的那台被改姿态）。
        assert_eq!(extracted.drawables.len(), 1, "只有网格实体是 drawable");

        // 对照组：同样的 `Transform3D`，但相机没有声明 `pose=transform`，姿态必须保持组件值。
        let control_camera = crate::core::scene::SceneEntityId(1);
        let control_mesh = crate::core::scene::SceneEntityId(2);
        let mut control = crate::core::scene::Scene::new("camera-look-at");
        control
            .add_entity(control_camera, "PlainCamera", None)
            .unwrap();
        control.add_entity(control_mesh, "Mesh", None).unwrap();
        control
            .entities
            .get_mut(&control_camera)
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "position=20,5,0;rotation=0,0.7071068,0,0.7071068;scale=1,1,1".into(),
            );
        control
            .entities
            .get_mut(&control_camera)
            .unwrap()
            .components
            .insert(
                "Camera".into(),
                "projection=perspective;position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100".into(),
            );
        control
            .entities
            .get_mut(&control_mesh)
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
        control
            .entities
            .get_mut(&control_mesh)
            .unwrap()
            .components
            .insert("MeshRenderer".into(), "mesh=assets/models/test.glb".into());
        let control_runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), control).expect("runtime scene");
        let control_camera = control_runtime
            .scene_3d(Viewport::new(16.0, 9.0).unwrap())
            .expect("3d scene")
            .camera;
        assert!(
            (control_camera.position - Vec3::new(0.0, 0.0, 6.0)).length() < 1e-3,
            "未声明 pose=transform 时位置必须保持组件值：{:?}",
            control_camera.position
        );
        assert!(
            control_camera.target.length() < 1e-3,
            "未声明 pose=transform 时 target 必须保持组件值：{:?}",
            control_camera.target
        );
    }

    /// M11-04 / R-05：编辑器 3D 视图相机覆盖（`scene_3d_with_camera`）不修改场景
    /// 数据——传 `Some` 仅覆盖渲染视角，传 `None` 走 `active_camera3d`；覆盖前后
    /// drawables 一致，且覆盖后 `scene_3d` 仍回退到场景相机（证明覆盖是瞬时的视图参数，
    /// 未写入 ECS / 脚本状态）。
    ///
    /// 拆分批 7：覆盖相机也**不被改写**——比例只进本帧视图（`view.aspect()`），相机载荷
    /// 与传入值逐字段相同（此前 `aspect_ratio` 会被覆写成传入比例）。
    #[test]
    fn scene_3d_with_camera_override_keeps_scene_data_intact() {
        let root = game_root();
        if !root.join("scenes/demo3d.scene").is_file() {
            return;
        }
        let path = root.join("scenes/demo3d.scene");
        let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load demo3d: {e}"));
        let runtime = RuntimeScene::from_scene_edit(Some(&root), scene).expect("runtime demo3d");

        let before = runtime
            .scene_3d(Viewport::new(16.0, 9.0).unwrap())
            .expect("3d scene");
        let override_camera = Camera3D {
            position: Vec3::new(10.0, 20.0, 30.0),
            target: Vec3::new(-5.0, -6.0, -7.0),
            up: Vec3::Z,
            fov_y: 0.9,
            near: 0.5,
            far: 200.0,
            pose_from_transform: false,
            enabled: true,
        };
        let overridden = runtime
            .scene_3d_with_camera(Viewport::new(4.0, 3.0).unwrap(), Some(override_camera))
            .expect("3d scene");
        // 覆盖仅作用于**本帧视图**：相机载荷逐字段保持覆盖值，比例来自视口而不是相机。
        assert_eq!(overridden.camera.position, Vec3::new(10.0, 20.0, 30.0));
        assert_eq!(overridden.camera.target, Vec3::new(-5.0, -6.0, -7.0));
        assert!((overridden.camera.fov_y - 0.9).abs() < 1e-6);
        assert_eq!(
            overridden.camera, override_camera,
            "宿主不得改写覆盖相机（拆分批 7 之前它会在这里被写回 aspect）"
        );
        assert!((overridden.view.aspect() - 4.0 / 3.0).abs() < 1e-6);
        assert_ne!(
            overridden.view.matrix(),
            before.view.matrix(),
            "不同视口比例必须给出不同的投影矩阵"
        );
        assert_eq!(
            overridden.view.matrix(),
            Some(
                override_camera
                    .view_projection_matrix(Viewport::new(4.0, 3.0).unwrap())
                    .unwrap()
            ),
            "视图矩阵必须等于按视口比例现算的那一份"
        );
        // drawables 不受相机覆盖影响：数量与每项名称/mesh path 均一致。
        assert_eq!(overridden.drawables.len(), before.drawables.len());
        for (a, b) in overridden.drawables.iter().zip(before.drawables.iter()) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.mesh_path, b.mesh_path);
        }
        // 无覆盖路径 = scene_3d 路径（相机/光源/资源/视图一致），覆盖后场景数据未变。
        let after = runtime
            .scene_3d(Viewport::new(16.0, 9.0).unwrap())
            .expect("3d scene");
        assert_eq!(after.camera, before.camera);
        assert_eq!(after.light.direction, before.light.direction);
        assert_eq!(after.drawables.len(), before.drawables.len());
        assert_eq!(
            after.view.matrix(),
            before.view.matrix(),
            "覆盖是瞬时的：回到无覆盖路径必须给出同一份视图矩阵"
        );
    }

    /// 拆分批 8 的场景夹具：一台透视相机 + 一个网格（`scene_3d` 以 drawable 为准，没有网格就
    /// 直接返回 `None`），可选第二台相机与 `pose=transform`。
    fn frame_view_scene(
        name: &str,
        second_camera: Option<&str>,
        camera_payload: &str,
    ) -> crate::core::scene::Scene {
        let mut scene = crate::core::scene::Scene::new(name);
        let camera = crate::core::scene::SceneEntityId(1);
        let mesh = crate::core::scene::SceneEntityId(2);
        scene.add_entity(camera, "Camera", None).unwrap();
        scene.add_entity(mesh, "Mesh", None).unwrap();
        scene
            .entities
            .get_mut(&camera)
            .unwrap()
            .components
            .insert("Camera".into(), camera_payload.into());
        let entity = scene.entities.get_mut(&mesh).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity
            .components
            .insert("MeshRenderer".into(), "mesh=assets/models/test.glb".into());
        if let Some(payload) = second_camera {
            let second = crate::core::scene::SceneEntityId(3);
            scene.add_entity(second, "Camera2", None).unwrap();
            scene
                .entities
                .get_mut(&second)
                .unwrap()
                .components
                .insert("Camera".into(), payload.into());
        }
        scene
    }

    /// 拆分批 8：**一帧只解析一次生效 3D 相机**，渲染提取与 Lua 回填因此是同一次解析的产物。
    ///
    /// 这是"同源靠构造、不靠调用顺序"的账：批 8 之前同一帧里 `vertices()`（回填 Lua）与
    /// `scene_3d()`（渲染提取）各解析一次，两者一致靠的是"两步之间没有代码改过世界"这个隐含
    /// 约定；现在账本上的数字是 1。
    #[test]
    fn one_frame_resolves_the_active_3d_camera_once() {
        let root = game_root();
        let scene = frame_view_scene(
            "frame-view-once",
            None,
            "projection=perspective;position=1,2,3;target=0,0,0;fov=60;near=0.1;far=100",
        );
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
        let viewport = Viewport::new(640.0, 360.0).unwrap();

        ledger::reset();
        let _mesh = runtime.vertices(640, 360).expect("vertices");
        let frame = runtime.frame_view_3d().expect("vertices() 应已定帧");
        let scene3d = runtime.scene_3d(viewport).expect("3d scene");

        let counts = ledger::snapshot();
        assert_eq!(
            counts.camera3d_resolutions, 1,
            "一帧只该解析一次生效相机（定帧），实际 {}",
            counts.camera3d_resolutions
        );
        assert_eq!(
            counts.view_3d, 1,
            "3D 视图只该烘焙一次（定帧），实际 {}",
            counts.view_3d
        );
        // 定帧值就是渲染提取用的那一份：相机逐字段相同、矩阵逐位相同、比例来自视口。
        assert_eq!(scene3d.camera, runtime.active_camera3d());
        assert_eq!(scene3d.view.matrix(), frame.matrix());
        assert!((scene3d.view.aspect() - viewport.aspect()).abs() < 1e-6);
        assert_eq!(scene3d.view.viewport, viewport);
        // 相机确实是场景里声明的那台，而不是回退默认相机。
        assert!((scene3d.camera.position - Vec3::new(1.0, 2.0, 3.0)).length() < 1e-6);
    }

    /// 拆分批 8：视口不同 ⇒ **换烘焙、不换相机**（相机不依赖视口，再解析一次只会多一趟遍历）。
    #[test]
    fn a_different_viewport_rebakes_without_resolving_the_camera_again() {
        let root = game_root();
        let scene = frame_view_scene(
            "frame-view-viewport",
            None,
            "projection=perspective;position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100",
        );
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");

        ledger::reset();
        let _ = runtime.vertices(640, 360).expect("vertices");
        let square = Viewport::new(480.0, 480.0).unwrap();
        let scene3d = runtime.scene_3d(square).expect("3d scene");
        let counts = ledger::snapshot();
        assert_eq!(counts.camera3d_resolutions, 1, "换视口不该再解析一次相机");
        assert_eq!(
            counts.view_3d, 2,
            "第二个视口要单独烘焙一次（1 次定帧 + 1 次重烘焙）"
        );
        assert!(
            (scene3d.view.aspect() - 1.0).abs() < 1e-6,
            "比例必须来自传入视口"
        );
        assert_eq!(scene3d.view.viewport, square);
        assert_eq!(
            scene3d.view.matrix(),
            Some(
                scene3d
                    .camera
                    .view_projection_matrix(square)
                    .expect("matrix")
            ),
            "重烘焙必须等价于按该视口现算"
        );
    }

    /// 拆分批 8（负向）：定帧值**不能盖过**此后发生的相机相关世界变更。
    ///
    /// 这是引入缓存后最危险的一条路径：宿主若在 `vertices()` 之后用 `set_local_transform` 挪动
    /// 带 `pose=transform` 的相机（编辑器属性面板就是这么做的），渲染必须看到**新**姿态。
    #[test]
    fn local_transform_write_invalidates_the_frame_view() {
        let root = game_root();
        let mut scene = frame_view_scene(
            "frame-view-stale-transform",
            None,
            "projection=perspective;position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100;pose=transform",
        );
        // `pose=transform`：姿态来自实体自己的 `Transform`（组件名 `Transform` 解出的就是 3D 变换）。
        scene
            .entities
            .get_mut(&crate::core::scene::SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Transform".into(),
                "position=0,1,6;rotation=0,0,0,1;scale=1,1,1".into(),
            );
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
        let viewport = Viewport::new(640.0, 360.0).unwrap();

        ledger::reset();
        let _ = runtime.vertices(640, 360).expect("vertices");
        let pinned = runtime.scene_3d(viewport).expect("3d scene");
        assert!(
            (pinned.camera.position - Vec3::new(0.0, 1.0, 6.0)).length() < 1e-3,
            "定帧时相机应取 Transform 的 (0,1,6)：{:?}",
            pinned.camera.position
        );

        // 帧内改姿态：定帧值必须作废。
        let moved = runtime
            .set_local_transform(
                crate::core::scene::SceneEntityId(1),
                crate::core::spatial_transform::LocalTransform {
                    translation: Vec3::new(0.0, 1.0, 60.0),
                    rotation: crate::core::math3d::Quat::from_axis_angle(Vec3::Y, 0.0).unwrap(),
                    scale: Vec3::new(1.0, 1.0, 1.0),
                },
            )
            .expect("local transform write");
        assert!(moved, "写局部变换应成功");
        assert!(
            runtime.frame_view_3d().is_none(),
            "写局部变换之后定帧值必须作废"
        );
        let after = runtime.scene_3d(viewport).expect("3d scene");
        assert!(
            (after.camera.position - Vec3::new(0.0, 1.0, 60.0)).length() < 1e-3,
            "渲染必须看到新姿态，而不是定帧时的旧值：{:?}",
            after.camera.position
        );
    }

    /// 拆分批 8（负向）：脚本步进之后定帧值作废——脚本可能换掉生效相机或移动相机父链。
    #[test]
    fn script_steps_invalidate_the_frame_view() {
        let root = game_root();
        let scene = frame_view_scene(
            "frame-view-stale-steps",
            Some("projection=perspective;position=0,0,9;target=0,0,0;fov=45;near=0.1;far=100"),
            "projection=perspective;position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100",
        );
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");

        ledger::reset();
        let _ = runtime.vertices(640, 360).expect("vertices");
        assert!(runtime.frame_view_3d().is_some(), "定帧后应有本帧视图");
        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect("script step");
        assert!(
            runtime.frame_view_3d().is_none(),
            "步进之后定帧值必须作废（脚本可能换掉生效相机）"
        );
        let counts = ledger::snapshot();
        assert_eq!(counts.frame_view_invalidations, 1);
        // 作废之后的消费重新定帧：这一帧因此是 2 次解析（定帧 1 次 + 作废后 1 次），这是
        // "宁可多解析一次，也不拿步进前的相机去渲染步进后的世界"的代价，账本上看得见。
        let _ = runtime
            .scene_3d(Viewport::new(640.0, 360.0).unwrap())
            .expect("3d scene");
        assert_eq!(ledger::snapshot().camera3d_resolutions, 2);
    }

    /// 拆分批 8：宿主**没走** `vertices()` 就先调 `scene_3d`（示例/golden 里有这种顺序）时，
    /// 没有定帧值 ⇒ 按需解析 + 烘焙，行为与定帧前逐字节相同。
    #[test]
    fn scene_3d_without_a_frame_still_resolves_on_demand() {
        let root = game_root();
        let scene = frame_view_scene(
            "frame-view-on-demand",
            None,
            "projection=perspective;position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100",
        );
        let runtime = RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime");

        ledger::reset();
        let viewport = Viewport::new(16.0, 9.0).unwrap();
        let extracted = runtime.scene_3d(viewport).expect("3d scene");
        let counts = ledger::snapshot();
        assert_eq!(counts.camera3d_resolutions, 1);
        assert_eq!(counts.view_3d, 1);
        assert_eq!(
            extracted.view.matrix(),
            Some(
                extracted
                    .camera
                    .view_projection_matrix(viewport)
                    .expect("matrix")
            )
        );
        assert!((extracted.view.aspect() - 16.0 / 9.0).abs() < 1e-6);
    }

    /// 拆分批 8 的**账本门**：把"一帧里相机被投影/烘焙了几次"变成确定的数字。
    ///
    /// 这不是性能断言（测试里的时间不可信），而是**口径断言**：docs/54 批 3 写的是"宿主每帧
    /// 烘焙一次"，现实是"每个消费者每帧各烘焙一次"（overlay 批、瓦片层、世界文字、宿主的
    /// overlay 视图各自一份 2D 预算项）。数字写在这里，任何增减都得有人来解释：是有意，还是回退。
    #[test]
    fn frame_bake_ledger_is_pinned() {
        let root = game_root();
        let mut scene = crate::core::scene::Scene::new("frame-bake-ledger");
        let sprite = crate::core::scene::SceneEntityId(1);
        let tiles = crate::core::scene::SceneEntityId(2);
        let text = crate::core::scene::SceneEntityId(3);
        let camera = crate::core::scene::SceneEntityId(4);
        let mesh = crate::core::scene::SceneEntityId(5);
        for (id, name) in [
            (sprite, "Sprite"),
            (tiles, "Tiles"),
            (text, "Text"),
            (camera, "Camera"),
            (mesh, "Mesh"),
        ] {
            scene.add_entity(id, name, None).unwrap();
        }
        let payload = |scene: &mut crate::core::scene::Scene, id, key: &str, value: &str| {
            scene
                .entities
                .get_mut(&id)
                .unwrap()
                .components
                .insert(key.into(), value.into());
        };
        payload(
            &mut scene,
            sprite,
            "Transform",
            "position=100,100,0;rotation=0,0,0,1;scale=1,1,1",
        );
        payload(
            &mut scene,
            sprite,
            "Sprite",
            "texture=assets/textures/red-1x1.png;size=2x2;pivot=0.5,0.5;z=0",
        );
        payload(
            &mut scene,
            tiles,
            "Transform",
            "position=640,360,0;rotation=0,0,0,1;scale=1,1,1",
        );
        payload(
            &mut scene,
            tiles,
            "Tilemap",
            "tile_size=32x32;width=2;height=2;texture=assets/textures/tileset.png;tiles=1111;solid=0000",
        );
        payload(
            &mut scene,
            text,
            "Transform",
            "position=640,400,0;rotation=0,0,0,1;scale=1,1,1",
        );
        payload(
            &mut scene,
            text,
            "WorldText",
            "text=HP;font_size=24;color=1,1,1;offset=0,-20;centered=false",
        );
        payload(
            &mut scene,
            camera,
            "Camera",
            "projection=perspective;position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100",
        );
        payload(
            &mut scene,
            mesh,
            "Transform",
            "position=0,0,0;rotation=0,0,0,1;scale=1,1,1",
        );
        payload(
            &mut scene,
            mesh,
            "MeshRenderer",
            "mesh=assets/models/test.glb",
        );
        let mut runtime =
            RuntimeScene::from_scene_edit(Some(root.as_path()), scene).expect("runtime scene");
        let viewport = Viewport::new(640.0, 360.0).unwrap();

        // 一帧典型路径：宿主提取 2D 顶点 → 声明 2D overlay 视图 → 提取 3D 场景。
        ledger::reset();
        let _ = runtime.vertices(640, 360).expect("vertices");
        let _ = runtime.render_view_2d(640, 360);
        let _ = runtime.scene_3d(viewport).expect("3d scene");
        let counts = ledger::snapshot();

        // 3D 侧（拆分批 8 的成果）：相机解析 1 次、视图烘焙 1 次——渲染提取复用定帧值。
        assert_eq!(counts.camera3d_resolutions, 1);
        assert_eq!(counts.view_3d, 1);
        // 2D 侧：每个消费者各烘焙一次自己的预算项——① overlay 批 ② 瓦片层 ③ 世界文字
        // ④ 宿主的 overlay 视图（`render_view_2d`，它还会烘焙一份完整 2D 视图）。
        // 这 4 次是**有意保留**的：单次烘焙是常数级（validate + sin_cos + 几次乘法），4 次远低于
        // 一帧预算，改签名做帧级共享的收益不成比例（docs/54 §12 批 8 有记录）。
        assert_eq!(counts.view_2d, 1, "宿主的 overlay 视图（render_view_2d）");
        assert_eq!(
            counts.ortho_terms, 4,
            "2D 预算项：overlay 批 + 瓦片层 + 世界文字 + render_view_2d 内部各一次，实际 {}",
            counts.ortho_terms
        );
    }

    /// 拆分批 9 的夹具：两台正交相机（`Cam2dA` 在前）+ 一个精灵，可选把 A 设为 `enabled=false`；
    /// `with_3d` 时再加一台透视相机（`Cam3d`）与一个网格，让 `scene_3d` 有内容。
    fn camera2d_selection_scene(
        name: &str,
        disable_a: bool,
        with_3d: bool,
    ) -> crate::core::scene::Scene {
        let mut scene = crate::core::scene::Scene::new(name);
        let a = crate::core::scene::SceneEntityId(1);
        let b = crate::core::scene::SceneEntityId(2);
        let sprite = crate::core::scene::SceneEntityId(3);
        for (id, entity) in [(a, "Cam2dA"), (b, "Cam2dB"), (sprite, "Sprite")] {
            scene.add_entity(id, entity, None).unwrap();
        }
        let enabled = if disable_a { ";enabled=false" } else { "" };
        scene.entities.get_mut(&a).unwrap().components.insert(
            "Camera".into(),
            format!("projection=orthographic;center=100,100;zoom=2{enabled}"),
        );
        scene.entities.get_mut(&b).unwrap().components.insert(
            "Camera".into(),
            "projection=orthographic;center=500,500;zoom=0.5".into(),
        );
        let entity = scene.entities.get_mut(&sprite).unwrap();
        entity.components.insert(
            "Transform".into(),
            "position=100,100,0;rotation=0,0,0,1;scale=1,1,1".into(),
        );
        entity.components.insert(
            "Sprite".into(),
            "texture=assets/textures/red-1x1.png;size=16x16;pivot=0.5,0.5;z=0".into(),
        );
        if with_3d {
            let camera3d = crate::core::scene::SceneEntityId(4);
            let mesh = crate::core::scene::SceneEntityId(5);
            scene.add_entity(camera3d, "Cam3d", None).unwrap();
            scene.add_entity(mesh, "Mesh", None).unwrap();
            scene
                .entities
                .get_mut(&camera3d)
                .unwrap()
                .components
                .insert(
                    "Camera".into(),
                    "projection=perspective;position=0,0,6;target=0,0,0;fov=60;near=0.1;far=100"
                        .into(),
                );
            let entity = scene.entities.get_mut(&mesh).unwrap();
            entity.components.insert(
                "Transform".into(),
                "position=0,0,0;rotation=0,0,0,1;scale=1,1,1".into(),
            );
            entity
                .components
                .insert("MeshRenderer".into(), "mesh=assets/models/test.glb".into());
        }
        scene
    }

    /// 拆分批 9：`camera.set_active` 是**一条**选择链——选中的正交相机就是 2D 相机的来源，
    /// 宿主在**选择变化**时采用一次它的载荷（center/zoom）；同一台不重复采用（否则每帧会把
    /// 控制器算出的取景抹平），而选中**透视**相机时 2D 取景必须原地不动（切 3D 相机是常见操作）。
    #[test]
    fn camera_set_active_on_an_orthographic_camera_adopts_the_2d_payload() {
        let root = game_root();
        let mut runtime = RuntimeScene::from_scene_edit(
            Some(root.as_path()),
            camera2d_selection_scene("camera2d-select", false, true),
        )
        .expect("runtime scene");
        // 默认（无选中）：排序序第一台启用的正交相机 ⇒ Cam2dA。
        assert_eq!(runtime.camera.center, Vec2::new(100.0, 100.0));
        assert_eq!(runtime.camera.zoom, 2.0);

        // 选中 Cam2dB（正交）⇒ 采用它的载荷。
        runtime
            .with_script(|script| {
                script.execute_console("return camera.set_active(entity.find('Cam2dB'))")
            })
            .expect("select Cam2dB");
        runtime.step_camera2d(1.0 / 60.0);
        assert_eq!(
            runtime.camera.center,
            Vec2::new(500.0, 500.0),
            "选中正交相机后必须采用它的载荷"
        );
        assert_eq!(runtime.camera.zoom, 0.5);

        // 同一台再"选中"一次不重复采用：把相机挪走（控制器或宿主写入）之后不得被盖回去。
        runtime.set_camera_2d(Camera2D {
            center: Vec2::new(42.0, 24.0),
            ..runtime.camera
        });
        runtime
            .with_script(|script| {
                script.execute_console("return camera.set_active(entity.find('Cam2dB'))")
            })
            .expect("re-select Cam2dB");
        for _ in 0..3 {
            runtime.step_camera2d(1.0 / 60.0);
        }
        assert_eq!(
            runtime.camera.center,
            Vec2::new(42.0, 24.0),
            "同一台重复选中不该把控制器算出的取景抹平"
        );

        // 选中**透视**相机（切 3D 取景）：2D 取景必须原地不动。
        runtime
            .with_script(|script| {
                script.execute_console("return camera.set_active(entity.find('Cam3d'))")
            })
            .expect("select Cam3d");
        for _ in 0..2 {
            runtime.step_camera2d(1.0 / 60.0);
        }
        assert_eq!(
            runtime.camera.center,
            Vec2::new(42.0, 24.0),
            "切 3D 相机不该把 2D 相机弹回作者值"
        );

        // 再选回 Cam2dB ⇒ 来源变了，重新采用它的载荷（这就是"把 2D 取景复位回场景声明"的表达）。
        runtime
            .with_script(|script| {
                script.execute_console("return camera.set_active(entity.find('Cam2dB'))")
            })
            .expect("select Cam2dB again");
        runtime.step_camera2d(1.0 / 60.0);
        assert_eq!(runtime.camera.center, Vec2::new(500.0, 500.0));
    }

    /// 拆分批 9：选中的正交相机**被禁用**时告警并回退到下一台可用相机（与 3D 侧选中禁用相机
    /// 同一规则、同一告警形状）。
    #[test]
    fn camera_set_active_on_a_disabled_2d_camera_falls_back() {
        let root = game_root();
        let mut runtime = RuntimeScene::from_scene_edit(
            Some(root.as_path()),
            camera2d_selection_scene("camera2d-disabled", true, false),
        )
        .expect("runtime scene");
        // A 被禁用 ⇒ 初始取景来自 B（"第一台被禁用不该挡掉后面启用的那台"）。
        assert_eq!(runtime.camera.center, Vec2::new(500.0, 500.0));

        // 显式选中被禁用的 A：必须回退（而不是采用它的载荷）。
        runtime
            .with_script(|script| {
                script.execute_console("return camera.set_active(entity.find('Cam2dA'))")
            })
            .expect("select disabled Cam2dA");
        runtime.step_camera2d(1.0 / 60.0);
        assert_eq!(
            runtime.camera.center,
            Vec2::new(500.0, 500.0),
            "选中被禁用的 2D 相机必须回退到下一台可用相机（B）"
        );
        assert_eq!(runtime.camera.zoom, 0.5);
    }

    /// 拆分批 9（负向）：一条选择链上，**投影决定服务哪个 pass**——选中正交相机不得改变 3D pass
    /// 用的相机，反之选中透视相机也不得改变 2D 取景。
    #[test]
    fn camera_selection_serves_each_pass_by_projection() {
        let root = game_root();
        let mut runtime = RuntimeScene::from_scene_edit(
            Some(root.as_path()),
            camera2d_selection_scene("camera2d-vs-3d", false, true),
        )
        .expect("runtime scene");
        let viewport = Viewport::new(640.0, 360.0).unwrap();
        let before = runtime.scene_3d(viewport).expect("3d scene").camera;
        let framing_before = runtime.camera;

        // 选中正交相机：3D pass 不受影响（候选按投影过滤）。
        runtime
            .with_script(|script| {
                script.execute_console("return camera.set_active(entity.find('Cam2dB'))")
            })
            .expect("select Cam2dB");
        runtime.step_camera2d(1.0 / 60.0);
        let after = runtime.scene_3d(viewport).expect("3d scene").camera;
        assert_eq!(after, before, "选中正交相机不得改变 3D pass 用的相机");
        assert_eq!(
            runtime.camera.center,
            Vec2::new(500.0, 500.0),
            "2D 侧确实换了相机（否则这条测试没有区分力）"
        );

        // 反过来：选中透视相机，2D 取景必须保持（不是回到作者值）。
        runtime
            .with_script(|script| {
                script.execute_console("return camera.set_active(entity.find('Cam3d'))")
            })
            .expect("select Cam3d");
        for _ in 0..3 {
            runtime.step_camera2d(1.0 / 60.0);
        }
        assert_eq!(runtime.camera.center, Vec2::new(500.0, 500.0));
        assert_ne!(framing_before.center, runtime.camera.center);
        let after_3d = runtime.scene_3d(viewport).expect("3d scene").camera;
        assert_eq!(after_3d, before, "Cam3d 是唯一透视相机，3D pass 仍用它");
    }

    /// RuntimeScene 无 GPU 长会话：重复 create → run_script_steps → vertices → drop，
    /// 断言无 panic、帧顶点数/纹理路径容器有界（含字体图集反复加载/释放）。
    #[test]
    fn runtime_scene_create_step_drop_100_cycles() {
        let root = game_root();
        if !root.join("scenes/main.scene").is_file() {
            return;
        }
        let path = root.join("scenes/main.scene");
        assert!(path.is_file(), "测试场景资源缺失: {}", path.display());
        let scene = Scene::load(&path).unwrap_or_else(|e| panic!("load main: {e}"));
        let input = InputSnapshot::default();
        for cycle in 0..100 {
            let mut runtime =
                RuntimeScene::from_scene_edit(Some(&root), scene.clone()).expect("runtime main");
            for _ in 0..3 {
                runtime
                    .run_script_steps(1, 1.0 / 60.0, &input)
                    .expect("script step");
            }
            let mesh = runtime.vertices(640, 360).expect("vertices");
            assert!(
                mesh.vertices.len() < 1_000_000,
                "cycle {cycle}: 顶点数应有界: {}",
                mesh.vertices.len()
            );
            assert!(
                runtime.texture_paths.len() < 512,
                "cycle {cycle}: 纹理路径容器应有界: {}",
                runtime.texture_paths.len()
            );
            // drop 释放 Lua/世界/字体资源。
        }
    }

    /// 场景 Text 指定自定义字体时：该字体 key 出现在 SceneMesh 的自定义图集
    /// 上报与纹理路径里；默认文本在短文本场景改走矢量路径。用临时项目根存放
    /// 两个字体文件（同源，仅验证管线/命名空间，不验证字形差异）。
    #[test]
    fn scene_text_custom_font_surfaces_in_mesh() {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../editor/fonts/NotoSansSC-VF.ttf");
        if !source.is_file() {
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "rustlua-custom-font-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let fonts_dir = root.join("assets").join("fonts");
        let _ = fs::create_dir_all(&fonts_dir);
        fs::copy(&source, fonts_dir.join("NotoSansSC-VF.ttf")).expect("copy default font");
        fs::copy(&source, fonts_dir.join("Custom.ttf")).expect("copy custom font");

        let mut scene = Scene::new("custom-fonts");
        scene
            .add_entity(SceneEntityId(1), "DefaultText", None)
            .unwrap();
        scene
            .add_entity(SceneEntityId(2), "CustomText", None)
            .unwrap();
        let layout = "anchors=0,0,0,0;offsets=0,0,64,16".to_owned();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert("UiLayout".into(), layout.clone());
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert("Text".into(), "text=Default;font_size=24".into());
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert("UiLayout".into(), layout);
        scene
            .entities
            .get_mut(&SceneEntityId(2))
            .unwrap()
            .components
            .insert(
                "Text".into(),
                "text=Custom;font_size=24;font=assets/fonts/Custom.ttf".into(),
            );

        let result = RuntimeScene::from_scene_edit(Some(&root), scene)
            .and_then(|mut runtime| runtime.vertices(320, 240));
        let _ = fs::remove_dir_all(&root);
        let mesh = result.expect("vertices");
        assert!(
            mesh.custom_font_atlases
                .iter()
                .any(|font| font.key == "font://assets/fonts/Custom.ttf"),
            "custom font key should be reported to the renderer"
        );
        let paths: Vec<&str> = mesh.paths.iter().map(|path| path.as_str()).collect();
        assert!(
            paths.contains(&"font://assets/fonts/Custom.ttf"),
            "custom font path should be in mesh paths: {paths:?}"
        );
        assert!(
            !paths.contains(&FONT_ATLAS_TEXTURE_PATH),
            "default short text should use the vector path: {paths:?}"
        );
        assert!(
            mesh.vector_vertices.len() >= 4,
            "default short text should emit vector vertices"
        );
    }

    #[test]
    fn text_render_mode_forces_vector_or_sdf_for_custom_fonts() {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../editor/fonts/NotoSansSC-VF.ttf");
        if !source.is_file() {
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "rustlua-text-render-mode-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let fonts_dir = root.join("assets").join("fonts");
        fs::create_dir_all(&fonts_dir).unwrap();
        fs::copy(&source, fonts_dir.join("Custom.ttf")).unwrap();

        let mut scene = Scene::new("render-mode");
        scene.add_entity(SceneEntityId(1), "Label", None).unwrap();
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "UiLayout".into(),
                "anchors=0,0,0,0;offsets=20,20,240,80".into(),
            );
        scene
            .entities
            .get_mut(&SceneEntityId(1))
            .unwrap()
            .components
            .insert(
                "Text".into(),
                "text=Mode;font_size=48;font=assets/fonts/Custom.ttf".into(),
            );

        let project = |mode: &str| {
            fs::write(
                root.join("project.toml"),
                format!(
                    "[project]\nfont = \"assets/fonts/NotoSansSC-VF.ttf\"\ntext_render_mode = \"{mode}\"\n"
                ),
            )
            .unwrap();
        };
        project("vector");
        let vector = RuntimeScene::from_scene_edit(Some(&root), scene.clone())
            .and_then(|mut runtime| runtime.vertices(320, 240))
            .expect("vector vertices");
        assert!(!vector.vector_vertices.is_empty());
        assert!(
            vector.custom_font_atlases.is_empty(),
            "forced vector mode should not build custom SDF atlases"
        );

        project("sdf");
        let sdf = RuntimeScene::from_scene_edit(Some(&root), scene)
            .and_then(|mut runtime| runtime.vertices(320, 240))
            .expect("sdf vertices");
        assert!(sdf.vector_vertices.is_empty());
        assert!(
            sdf.custom_font_atlases
                .iter()
                .any(|font| font.key == "font://assets/fonts/Custom.ttf")
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn text_render_mode_parsing_uses_auto_fallback() {
        assert_eq!(TextRenderMode::parse("auto"), TextRenderMode::Auto);
        assert_eq!(TextRenderMode::parse(" Vector "), TextRenderMode::Vector);
        assert_eq!(TextRenderMode::parse("MSDF"), TextRenderMode::Sdf);
        assert_eq!(TextRenderMode::parse("bad"), TextRenderMode::Auto);
    }

    #[test]
    fn sourcehan_sdf_mode_emits_default_font_draws() {
        let root = crate::core::games_dir()
            .join("examples")
            .join("sourcehan_test");
        if !root.join("scenes/font_check.scene").is_file() {
            return;
        }
        let path = root.join("scenes/font_check.scene");
        let scene = Scene::load(&path).unwrap();
        let font_atlas = load_project_font_atlas(Some(&root)).expect("project font atlas");
        let mut runtime = RuntimeScene::from_scene_edit(Some(&root), scene).unwrap();
        runtime.set_font_atlas(Some(font_atlas));
        runtime.set_text_render_mode(TextRenderMode::Sdf);
        runtime.set_render_ui_overlay(false);
        let hidden = runtime.vertices(1280, 720).unwrap();
        runtime.set_render_ui_overlay(true);
        let mesh = runtime.vertices(1280, 720).unwrap();
        let sdf_draws = mesh.draws.iter().filter(|draw| draw.sdf).count();
        let font_paths = mesh
            .paths
            .iter()
            .filter(|path| path.as_str() == FONT_ATLAS_TEXTURE_PATH)
            .count();
        assert!(
            sdf_draws > 0 && font_paths > 0,
            "sdf draws={sdf_draws}, font paths={font_paths}, all={:?}",
            mesh.paths
        );
        // 空网格帧（overlay=false）不得消费版本号：首个可绘制帧必须携带全量
        // 像素（否则新渲染器会以空数据 write_texture 整张图集 → wgpu 崩溃）。
        assert!(
            hidden
                .font_atlas
                .as_ref()
                .is_some_and(|font| font.pixels.is_empty()),
            "empty-mesh frame must not consume the atlas version (deferred to drawable frame)"
        );
        assert!(
            mesh.font_atlas
                .as_ref()
                .is_some_and(|font| !font.pixels.is_empty()),
            "first drawable frame must carry full atlas pixels"
        );
    }
}

/// M18-03 共享帧调度管线测试：内置阶段顺序、注册 API、依赖/循环/panic 诊断
/// 与调度性能统计出口。这些测试是 headless 的：直接驱动 RuntimeScene。
#[cfg(test)]
mod frame_schedule_tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn sample_root() -> std::path::PathBuf {
        crate::core::games_dir().join("examples").join("game_api")
    }

    fn sample_runtime() -> RuntimeScene {
        let root = sample_root();
        RuntimeScene::new(Some(&root)).expect("runtime for game_api sample project")
    }

    /// **回归**：换图后必须重读新场景声明的 `Camera`，否则场景声明的 bounds 只在启动场景生效。
    ///
    /// 怎么坏的：`process_scene_transition` 重建整个世界（`World::new()` 后重新实例化），
    /// 换图后那台 `Camera` 是**新的一份**；宿主原先只在 `init`（启动场景）读过一次，
    /// 于是 `self.camera.bounds` 永远停在开工那张图的值上——每张地图各声明一份 bounds 的
    /// 工程（`examples/game_jhyt`）里，之后所有地图的相机都不受约束，而且**没有任何报错**。
    ///
    /// 断言三件事：① 启动场景（门面，无相机）用历史默认、无 bounds；② 换到声明了相机的
    /// 场景后 bounds 变成新场景那一份；③ **center 不被重置**（姿态的权威仍是每帧的
    /// `camera2d.*`，重置回作者中心会在换图那一帧闪一下）。
    #[test]
    fn scene_declared_camera_bounds_refresh_after_scene_switch() {
        let root = crate::core::games_dir().join("examples").join("game_jhyt");
        if !root.join("scenes/manor.scene").is_file() {
            eprintln!("跳过：checkout 内没有 examples/game_jhyt");
            return;
        }
        let mut runtime = RuntimeScene::new(Some(&root)).expect("runtime for game_jhyt");
        let idle = InputSnapshot::default();
        runtime.run_script_steps(1, 1.0 / 60.0, &idle).unwrap();
        assert_eq!(runtime.script.scene_name().as_deref(), Some("main"));
        assert_eq!(
            runtime.camera_2d().bounds,
            None,
            "门面 main.scene 没有声明相机 ⇒ 不该有 bounds"
        );

        // 进入山庄：bounds 换成地面瓦片图的矩形（40×23 格 × 32 = 1280×736）。
        runtime
            .with_script(|script| script.execute_console("on_click('BtnStart')"))
            .unwrap();
        runtime.run_script_steps(1, 1.0 / 60.0, &idle).unwrap();
        assert_eq!(runtime.script.scene_name().as_deref(), Some("manor"));
        let manor = runtime.camera_2d();
        let expected =
            CameraBounds::new(Vec2::new(0.0, 0.0), Vec2::new(1280.0, 736.0)).expect("manor bounds");
        assert_eq!(
            manor.bounds,
            Some(expected),
            "换图后应读入新场景声明的 bounds"
        );
        // ③ 姿态不被重置：`enter_playable_map` 先落点再 follow，下一帧相机中心 = 玩家坐标
        // （528,112）。若 bounds 同步顺手把 center 重置成场景作者值，这里会变成 (640,368)——
        // 注意 y 最终仍会被夹到 bounds 内（地图 736 ≥ 视口高），所以只断言 x 未被重置。
        runtime.vertices(720, 1280).expect("render viewport");
        runtime.run_script_steps(1, 1.0 / 60.0, &idle).unwrap();
        let followed = runtime.camera_2d();
        assert!(
            (followed.center.x - 528.0).abs() < 1.0,
            "换图后 center.x 应由跟随决定（528），实际 {}",
            followed.center.x
        );

        // 换到室内（20×12 = 640×384）：bounds 必须跟着地图换，而不是留在山庄那份。
        runtime
            .with_script(|script| script.execute_console("map[\"goto\"]('interior_herb')"))
            .unwrap();
        runtime.run_script_steps(1, 1.0 / 60.0, &idle).unwrap();
        assert_eq!(
            runtime.script.scene_name().as_deref(),
            Some("interior_herb")
        );
        let small = CameraBounds::new(Vec2::new(0.0, 0.0), Vec2::new(640.0, 384.0))
            .expect("interior bounds");
        assert_eq!(
            runtime.camera_2d().bounds,
            Some(small),
            "换到更小的地图后 bounds 应同步缩小"
        );
    }

    /// 固定步顺序必须与原手工编排一致：
    /// input_apply → lua_update → ui_click_dispatch → physics2d → physics3d →
    /// particles → clock → scene_transition；帧级系统每调用一次：
    /// integrate_velocity → animation_state_advance → animation3d_advance。
    #[test]
    fn builtin_pipeline_preserves_legacy_stage_order() {
        let mut runtime = sample_runtime();
        let order: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let recorder = |name: &'static str, order: Rc<RefCell<Vec<String>>>| {
            move |_scene: &mut RuntimeScene, _dt: f32| {
                order.borrow_mut().push(name.to_owned());
                Ok(())
            }
        };
        runtime
            .add_step_system(System::new(
                "user_script_probe",
                ScheduleStage::Script,
                ["lua_update"],
                recorder("user_script_probe", Rc::clone(&order)),
            ))
            .unwrap();
        runtime
            .add_step_system(System::new(
                "user_physics_probe",
                ScheduleStage::Physics,
                [] as [&str; 0],
                recorder("user_physics_probe", Rc::clone(&order)),
            ))
            .unwrap();
        runtime
            .run_script_steps(2, 1.0 / 60.0, &InputSnapshot::default())
            .expect("scheduled steps");
        // 两个用户探针各自每步执行一次，且 Script 阶段探针先于 Physics 探针。
        assert_eq!(
            order.borrow().as_slice(),
            [
                "user_script_probe",
                "user_physics_probe",
                "user_script_probe",
                "user_physics_probe",
            ]
        );
        // 调度统计的 timings 按「首次执行顺序」记录，完整反映共享管线的阶段顺序。
        // WP7 收敛后的统一序（负责人裁定）：脚本 → 静态/运动学同步 → 物理 → 动态回写 →
        // （模拟/动画）→ 世界变换传播；`spatial_propagate` 在动画之后，因为动画也会改局部变换，
        // 传播必须是**最后**一步，提取才能看到本帧的最终世界位置。
        let report = runtime.last_schedule_report().expect("schedule report");
        let names: Vec<&str> = report.timings.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "input_apply",
                "lua_update",
                "ui_click_dispatch",
                // docs/54 §4.1：每实体脚本紧跟工程级脚本（`ui_click_dispatch` 之后、用户系统之前）
                "entity_scripts",
                "user_script_probe",
                "physics_sync",
                "physics2d_step",
                "physics3d_step",
                "user_physics_probe",
                "physics_writeback",
                "scene_emitters_step",
                "particles_step",
                "clock_step",
                "scene_transition",
                // 换图后重读新场景声明的 `Camera`（bounds 必须跟着地图走），排在换图之后。
                "scene_camera_sync",
                "integrate_velocity",
                "camera2d_controller",
                "animation_state_advance",
                "animation3d_advance",
                "spatial_propagate",
            ]
        );
    }

    #[test]
    fn schedule_report_collects_steps_and_timings() {
        let mut runtime = sample_runtime();
        runtime
            .run_script_steps(3, 1.0 / 60.0, &InputSnapshot::default())
            .expect("scheduled steps");
        let report = runtime
            .last_schedule_report()
            .expect("schedule report is recorded");
        assert_eq!(report.steps, 3);
        let names: Vec<&str> = report.timings.iter().map(|t| t.name.as_str()).collect();
        for expected in [
            "input_apply",
            "lua_update",
            "ui_click_dispatch",
            "physics2d_step",
            "physics3d_step",
            "particles_step",
            "clock_step",
            "scene_transition",
            "integrate_velocity",
            "animation_state_advance",
            "animation3d_advance",
        ] {
            assert!(names.contains(&expected), "missing timing: {expected}");
        }
        assert!(report.total_elapsed >= report.timings.iter().map(|t| t.elapsed).sum::<Duration>());
        // 帧级系统只执行一次：其累计耗时应远小于每步系统（同量级下不可靠，
        // 仅验证 steps 计数与 timings 覆盖）。
        assert!(report.total_elapsed.as_nanos() > 0);
    }

    #[test]
    fn duplicate_system_names_are_rejected_at_registration() {
        let mut runtime = sample_runtime();
        let error = runtime
            .add_step_system(System::new(
                "lua_update",
                ScheduleStage::Script,
                [] as [&str; 0],
                |_: &mut RuntimeScene, _| Ok(()),
            ))
            .expect_err("duplicate builtin name must be rejected");
        assert!(matches!(error, SchedulerError::DuplicateSystem { .. }));
        runtime
            .add_frame_system(System::new(
                "frame_probe",
                ScheduleStage::Animation,
                [] as [&str; 0],
                |_: &mut RuntimeScene, _| Ok(()),
            ))
            .unwrap();
        let error = runtime
            .add_step_system(System::new(
                "frame_probe",
                ScheduleStage::Transform,
                [] as [&str; 0],
                |_: &mut RuntimeScene, _| Ok(()),
            ))
            .expect_err("cross-pipeline duplicate name must be rejected");
        assert!(matches!(error, SchedulerError::DuplicateSystem { .. }));
    }

    /// 回归：注册失败绝不能"吃掉"已经注册的系统。
    ///
    /// 旧实现每帧把系统 `drain` 进新调度器、遇到 `add_system` 失败就 `break`：
    /// `Drain` 在提前退出时会连同**尚未搬迁**的系统一起丢弃，一次失败的注册就能
    /// 让后续的内建系统（物理、时钟、动画）静默消失。现在系统所有权始终在调度器
    /// 里，被拒绝的注册只影响它自己。
    #[test]
    fn rejected_registration_never_drops_registered_systems() {
        let mut runtime = sample_runtime();
        let (step_names, frame_names) = runtime.registered_system_names();
        let builtin_steps = step_names.len();
        assert!(builtin_steps >= 8, "builtin step systems: {step_names:?}");
        assert!(!frame_names.is_empty());

        // 重名的注册必须被拒绝。
        let error = runtime
            .add_step_system(System::new(
                "lua_update",
                ScheduleStage::Script,
                [] as [&str; 0],
                |_: &mut RuntimeScene, _| Ok(()),
            ))
            .expect_err("duplicate name must be rejected");
        assert!(matches!(error, SchedulerError::DuplicateSystem { .. }));

        // 被拒绝之后：注册表一个不少，调度仍然完整跑通。
        let (step_names_after, frame_names_after) = runtime.registered_system_names();
        assert_eq!(step_names_after, step_names);
        assert_eq!(frame_names_after, frame_names);
        runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect("schedule must still run after a rejected registration");
        let names: Vec<&str> = runtime
            .last_schedule_report()
            .expect("schedule report is recorded")
            .timings
            .iter()
            .map(|timing| timing.name.as_str())
            .collect();
        for expected in [
            "input_apply",
            "lua_update",
            "physics2d_step",
            "clock_step",
            "integrate_velocity",
        ] {
            assert!(names.contains(&expected), "dropped system: {expected}");
        }
    }

    #[test]
    fn missing_dependency_is_diagnosed_with_system_names() {
        let mut runtime = sample_runtime();
        runtime
            .add_step_system(System::new(
                "orphan_probe",
                ScheduleStage::Script,
                ["no_such_system"],
                |_: &mut RuntimeScene, _| Ok(()),
            ))
            .unwrap();
        let error = runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect_err("missing dependency must fail the run");
        let message = error.to_string();
        assert!(
            message.contains("orphan_probe") && message.contains("no_such_system"),
            "diagnostic must name system and dependency: {error}"
        );
        // 失败后注册仍在（系统已放回），修复依赖后可重新执行。
        let (step_names, _) = runtime.registered_system_names();
        assert!(step_names.contains(&"orphan_probe".to_owned()));
    }

    #[test]
    fn dependency_cycle_is_diagnosed_with_the_cycle_members() {
        let mut runtime = sample_runtime();
        runtime
            .add_step_system(System::new(
                "cycle_a",
                ScheduleStage::Physics,
                ["cycle_b"],
                |_: &mut RuntimeScene, _| Ok(()),
            ))
            .unwrap();
        runtime
            .add_step_system(System::new(
                "cycle_b",
                ScheduleStage::Physics,
                ["cycle_a"],
                |_: &mut RuntimeScene, _| Ok(()),
            ))
            .unwrap();
        let error = runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect_err("dependency cycle must fail the run");
        let message = error.to_string();
        assert!(
            message.contains("cycle_a") && message.contains("cycle_b"),
            "cycle diagnostic must name the members: {error}"
        );
    }

    #[test]
    fn system_panic_is_diagnosed_and_registration_survives() {
        let mut runtime = sample_runtime();
        runtime
            .add_step_system(System::new(
                "panic_probe",
                ScheduleStage::Transform,
                [] as [&str; 0],
                |_: &mut RuntimeScene, _| panic!("probe-boom"),
            ))
            .unwrap();
        let error = runtime
            .run_script_steps(1, 1.0 / 60.0, &InputSnapshot::default())
            .expect_err("panicking system must fail the run");
        let message = error.to_string();
        assert!(
            message.contains("panic_probe") && message.contains("probe-boom"),
            "panic diagnostic must name system and payload: {error}"
        );
        let (step_names, _) = runtime.registered_system_names();
        assert!(step_names.contains(&"panic_probe".to_owned()));
    }

    #[test]
    fn systems_owning_script_state_run_inside_the_shared_schedule() {
        // 通过 with_script 在注册系统内驱动引擎脚本 API（Rust 侧玩法系统
        // 的官方接入路径），并验证 frame 系统每次调用只执行一次。
        let mut runtime = sample_runtime();
        let runs = Rc::new(RefCell::new(0u32));
        let counter = Rc::clone(&runs);
        runtime
            .add_frame_system(System::new(
                "frame_counter_probe",
                ScheduleStage::Transform,
                [] as [&str; 0],
                move |scene: &mut RuntimeScene, _dt: f32| {
                    *counter.borrow_mut() += 1;
                    scene.with_script(|script| {
                        script.set_delta(0.0);
                    });
                    Ok(())
                },
            ))
            .unwrap();
        runtime
            .run_script_steps(4, 1.0 / 60.0, &InputSnapshot::default())
            .expect("scheduled steps");
        assert_eq!(*runs.borrow(), 1, "frame systems run once per call");
        runtime
            .run_script_steps(0, 1.0 / 60.0, &InputSnapshot::default())
            .expect("zero-step call still runs the frame pipeline");
        assert_eq!(*runs.borrow(), 2);
    }
}

#[cfg(test)]
mod ui_circle_tests {
    use super::*;

    /// **回归**：`ui.circle` 的顶点必须是 triangle-list 序（每段 3 顶点）。
    ///
    /// 怎么坏的：圆形曾按三角扇顶点序写（圆心 + 一圈环 = 34 顶点），而 2D 管线拓扑是
    /// triangle list（矩形 6 顶点）——扇序被切成一个楔形 + 沿圆周的一串细片，画面上
    /// 虚拟摇杆/按钮变成"轮廓圆 + 跨圆交叉细三角"，且没有任何报错。
    ///
    /// 断言打在 [`RuntimeScene::push_ui_panels`] 而不是 `vertices()` 的 mesh 上：
    /// mesh 级 `merge_adjacent_draws` 会把圆与相邻白纹理 draw 合批，单个圆的
    /// 顶点数在 mesh 里看不见；这里直接取未合批的 draw 分段。
    #[test]
    fn circle_vertices_are_triangle_list_compatible() {
        let root = crate::core::games_dir().join("examples").join("game_api");
        let mut runtime = RuntimeScene::new(Some(&root)).expect("runtime for game_api sample");
        runtime
            .with_script(|lua| lua.execute_console("ui.circle('probe', 32, 18, 9, 1, 1, 1, 1)"))
            .expect("ui.circle probe");
        let mut vertices = Vec::new();
        let mut draws = Vec::new();
        runtime
            .push_ui_panels(&mut vertices, &mut draws, 64, 36)
            .expect("push_ui_panels");
        for draw in &draws {
            assert_eq!(
                draw.count % 3,
                0,
                "draw start={} count={} 不是 3 的倍数（triangle-list 拓扑）",
                draw.start,
                draw.count
            );
        }
        let circle = draws
            .iter()
            .find(|draw| draw.count == 96)
            .expect("圆应贡献 32 段 × 3 顶点 = 96 顶点");
        // 扇心复刻不变量：圆的每个三角形首顶点都是同一个圆心。
        let start = circle.start as usize;
        for triangle in 0..32 {
            assert_eq!(
                vertices[start + triangle * 3].position,
                vertices[start].position,
                "第 {triangle} 个三角形的首顶点必须是圆心"
            );
        }
    }
}
