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
        Camera2D, CameraSelection, CameraView, DEFAULT_3D_CLEAR, OrthoTerms, RenderView, Viewport,
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
    script::{LuaRuntime, ReloadStatus, ScriptDiagnostic, ScriptErrorPolicy},
    sprite::{Sprite, SpriteAsset, SpriteSorting},
    sprite_batch::{
        OverlayVertexSink, SpriteBatch, SpriteBatchItem, SpriteDrawBatch, ndc_from_screen,
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
    FontAtlasPixels, OverlayVertex, Scene3d, Scene3dBounds, Scene3dDrawable, SceneDraw, SceneMesh,
    VectorAtlasMap, VectorFontHandle, VectorGlyphSnapshot, VectorSegmentSnapshot,
    VectorTextVertex as FrameVectorTextVertex, WHITE_TEXTURE_PATH, WorldSpriteVertex,
    font_texture_path,
};
use crate::host::gamepad::{GamepadBackend, GamepadDeadzone, GamepadPoller};

mod camera;
mod fonts;
/// 本模块的测试集合（WP12 阶段 1 从本文件搬出，100 项）：作为 `runtime_scene` 的后代，
/// 它们靠 `use super::*;` 的 glob 拿私有项，因此这里不需要为测试放宽任何可见性。
#[cfg(test)]
mod tests;
mod text;
mod tilemap;

use camera::{declared_camera_2d_in, selected_camera_2d_in, sprite_screen_rect};
pub use fonts::{
    load_custom_font_atlas, load_custom_vector_atlas, load_project_font_atlas,
    load_project_vector_atlas,
};
use tilemap::{
    append_tilemap_layer, append_tilemap_layer_world, build_all_chunks, rebuild_dirty_chunks,
    sort_world_ranges,
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
                // 不变量（见 `crate::core::hierarchy`）：开启继承时**每个有局部变换的实体都有世界变换**
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
