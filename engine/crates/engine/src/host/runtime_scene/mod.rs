//! Runtime scene: instantiates the project scene/scripts and produces
//! per-frame [`SceneMesh`] (2D) and [`Scene3d`] draw data for the renderer.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::core::{
    animation::{AnimationFrameEvent, AnimationStateMachine},
    animation3d::Animation3D,
    asset_handle::{AssetHandle, AssetRecord, AssetStore},
    camera::Camera3D,
    camera::{Camera2D, CameraSelection, RenderView, Viewport},
    entity::EntityId,
    font::{FontAtlas, FontVectorAtlas, project_font},
    hierarchy::SpatialReport,
    input::InputSnapshot,
    mesh3d::MeshRenderer,
    par::Parallelism,
    physics::PhysicsAuthority,
    resource_kind::ResourceKind,
    scene::{Scene, SceneEntityId},
    scheduler::{
        ScheduleReport, ScheduleStage, Scheduler, SchedulerError, System, SystemError, SystemTiming,
    },
    script::{LuaRuntime, ReloadStatus, ScriptDiagnostic, ScriptErrorPolicy},
    sprite_batch::{OverlayVertexSink, SpriteBatch, ndc_from_screen},
    text::TextMesh,
    tilemap::Tilemap,
    transform::{Transform, Vec2, integrate_velocity},
    ui_layout::UiRectLayout,
    ui_scene_layout::{ResolvedUiNode, UiSceneNode},
};

use crate::host::frame::{
    FONT_ATLAS_TEXTURE_PATH, FontAtlasMap, OverlayVertex, SceneDraw, VectorAtlasMap,
    VectorGlyphSnapshot, VectorSegmentSnapshot, VectorTextVertex as FrameVectorTextVertex,
    WorldSpriteVertex, font_texture_path,
};
use crate::host::gamepad::{GamepadBackend, GamepadDeadzone, GamepadPoller};

mod camera;
mod fonts;
mod frame_data;
mod mesh_ui;
mod scene;
mod spatial;
/// 本模块的测试集合（WP12 阶段 1 从本文件搬出，100 项）：作为 `runtime_scene` 的后代，
/// 它们靠 `use super::*;` 的 glob 拿私有项，因此这里不需要为测试放宽任何可见性。
#[cfg(test)]
mod tests;
mod text;
mod tilemap;

use camera::{declared_camera_2d_in, selected_camera_2d_in};
pub use fonts::{
    load_custom_font_atlas, load_custom_vector_atlas, load_project_font_atlas,
    load_project_vector_atlas,
};

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
    /// 即完全回滚到单线程，输出逐字节相同（见 `crate::core::par` 与 `sprite_batch` 的测试）。
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

    /// 设置脚本错误处置策略（见 [`ScriptErrorPolicy`]）。
    ///
    /// **默认严格**：脚本报错 ⇒ `run_script_steps` 返回 `Err`。无头样例、soak 与性能
    /// 基线全都 `unwrap()` 这个结果，因此验收路径"脚本一错就判红"的语义不因容错能力
    /// 的加入而改变。交互宿主（`game.exe`、编辑器 Play）显式切到
    /// [`ScriptErrorPolicy::Contain`]：错误记进诊断与计数，游戏继续跑，而不是让玩家
    /// 的一局因为某个 Lua 分支写错而中断——此前那条路径会直接终止进程，并连带跳过
    /// 本步的物理/粒子/场景切换。
    pub fn set_script_error_policy(&mut self, policy: ScriptErrorPolicy) {
        self.script.set_error_policy(policy);
    }

    /// 当前脚本错误处置策略。
    #[must_use]
    pub fn script_error_policy(&self) -> ScriptErrorPolicy {
        self.script.error_policy()
    }

    /// 自本次会话开始被**容错**的脚本错误总数。
    ///
    /// 容错不等于静默：宿主与验收脚本用这个计数把"脚本出过错"变成可断言的信号
    /// （严格档下错误走 `Err`，因此该计数只在容错档增长）。
    #[must_use]
    pub fn script_error_total(&self) -> u64 {
        self.script.script_error_total()
    }

    /// 因连续报错被停用的脚本回调（可读键，按字典序）。
    #[must_use]
    pub fn script_disabled_callbacks(&self) -> Vec<String> {
        self.script.disabled_callbacks()
    }

    /// 脚本侧结构化诊断（Console 面板的数据源）。
    #[must_use]
    pub fn script_diagnostics(&self) -> Vec<ScriptDiagnostic> {
        self.script.diagnostics()
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
