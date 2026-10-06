//! Runtime scene: instantiates the project scene/scripts and produces
//! per-frame [`SceneMesh`](crate::host::frame::SceneMesh) (2D) and
//! [`Scene3d`](crate::host::frame::Scene3d) draw data for the renderer.
//!
//! 这两个链接写全路径：`SceneMesh`/`Scene3d` 的 `use` 跟着建帧代码搬进了 `frame_data.rs`，
//! 父模块只剩这段文档注释需要它们，短名会让 rustdoc 报 unresolved link。

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::core::{
    asset_handle::{AssetHandle, AssetRecord, AssetStore},
    camera::Camera3D,
    camera::{Camera2D, RenderView, Viewport},
    entity::EntityId,
    font::{FontAtlas, FontVectorAtlas, project_font},
    hierarchy::SpatialReport,
    input::InputSnapshot,
    par::Parallelism,
    physics::PhysicsAuthority,
    resource_kind::ResourceKind,
    scene::{Scene, SceneEntityId},
    scheduler::{ScheduleReport, Scheduler, SystemTiming},
    script::LuaRuntime,
    sprite_batch::{OverlayVertexSink, SpriteBatch, ndc_from_screen},
    text::TextMesh,
    tilemap::Tilemap,
    transform::{Transform, Vec2},
    ui_scene_layout::{ResolvedUiNode, UiSceneNode, UiSceneNodeSource},
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
mod schedule;
mod script;
mod spatial;
/// 本模块的测试集合（WP12 阶段 1 从本文件搬出，100 项）：作为 `runtime_scene` 的后代，
/// 它们靠 `use super::*;` 的 glob 拿私有项，因此这里不需要为测试放宽任何可见性。
#[cfg(test)]
mod tests;
mod text;
mod tilemap;

use camera::declared_camera_2d_in;
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
    /// 已经并进 `ui_scene_nodes` 的**运行期 spawn** 节点（脚本侧报告的那一份的副本）。
    /// 每帧与脚本的报告比对来判断"spawn/despawn 有没有改动布局"，没变就一个分配都不做。
    pub(crate) applied_prefab_ui_nodes: BTreeMap<u64, UiSceneNode>,
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
/// "哪些实体参与布局、`preferred_size` 怎么算"这套规则只有**一处**定义：
/// `crate::core::ui_scene_layout::build_ui_scene_nodes`。运行期 `prefab.spawn` 把同一份
/// 组件字符串落进活世界后走的是同一个函数（见 `rustlua-scripting` 的 prefab 节点登记），
/// 于是"场景里会画的 UI，spawn 出来不会画"这种各算一遍的分叉没有落脚的地方。
/// 这里只剩一层映射：文档实体 id → 活实体槽位序。
fn build_ui_scene_nodes(scene: &Scene, honor_editor_hidden: bool) -> BTreeMap<u64, UiSceneNode> {
    let mut id_to_key = BTreeMap::new();
    for entity in scene.entities.values() {
        if honor_editor_hidden && scene.is_entity_hidden_with_ancestors(entity.id) {
            continue;
        }
        id_to_key.insert(entity.id.0, id_to_key.len() as u64);
    }
    crate::core::ui_scene_layout::build_ui_scene_nodes(scene.entities.values().filter_map(
        |entity| {
            Some(UiSceneNodeSource {
                key: *id_to_key.get(&entity.id.0)?,
                parent: entity.parent.and_then(|id| id_to_key.get(&id.0)).copied(),
                components: &entity.components,
            })
        },
    ))
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
