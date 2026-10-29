//! Runtime scene: instantiates the project scene/scripts and produces
//! per-frame [`SceneMesh`](rustlua_render::frame::SceneMesh) (2D) and
//! [`Scene3d`](rustlua_render::frame::Scene3d) draw data for the renderer.
//!
//! 这两个链接写全路径：`SceneMesh`/`Scene3d` 的 `use` 跟着建帧代码搬进了 `frame_data.rs`，
//! 父模块只剩这段文档注释需要它们，短名会让 rustdoc 报 unresolved link。

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use rustlua_assets::asset_handle::{AssetHandle, AssetRecord, AssetStore};
use rustlua_assets::resource_kind::ResourceKind;
use rustlua_camera::camera::Camera3D;
use rustlua_camera::camera::{Camera2D, RenderView, Viewport};
use rustlua_ecs::entity::EntityId;
use rustlua_ecs::hierarchy::SpatialReport;
use rustlua_ecs::particle::{ParticleCpuReason, ParticlePlacement};
use rustlua_ecs::scheduler::{ScheduleReport, Scheduler, SystemTiming};
use rustlua_ecs::tilemap::Tilemap;
use rustlua_ecs::transform::{Transform, Vec2};
use rustlua_infra::par::Parallelism;
use rustlua_input::input::InputSnapshot;
use rustlua_physics::physics::PhysicsAuthority;
use rustlua_render::font::{FontAtlas, FontVectorAtlas, project_font};
use rustlua_render::sprite_batch::{OverlayVertexSink, SpriteBatch, ndc_from_screen};
use rustlua_render::text::TextMesh;
use rustlua_scene::scene::{Scene, SceneEntityId};
use rustlua_scripting::script::LuaRuntime;
use rustlua_ui::ui_scene_layout::{ResolvedUiNode, UiSceneNode, UiSceneNodeSource};

use crate::host::gamepad::{GamepadBackend, GamepadDeadzone, GamepadPoller};
use rustlua_render::frame::{
    FONT_ATLAS_TEXTURE_PATH, FontAtlasMap, HintedAtlasMap, OverlayVertex, SceneDraw,
    VectorAtlasMap, VectorGlyphSnapshot, VectorSegmentSnapshot,
    VectorTextVertex as FrameVectorTextVertex, WorldSpriteVertex, font_texture_path,
};

mod camera;
mod environment;
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
mod ui_bindings;
mod ui_controls;
mod ui_input;
mod ui_scroll;

use camera::declared_camera_2d_in;
pub use fonts::{
    SharedFontBytes, load_custom_font_atlas, load_custom_vector_atlas, load_project_font_atlas,
    load_project_vector_atlas,
};
// 编辑器 Play 期间的层级同步要用它列出脚本运行期生成的实体（见 `RuntimeScene::runtime_spawned_entities`）。
pub use environment::{
    EnvironmentApplyReport, EnvironmentReport, RuntimeSceneEnvironment, apply_to_renderer,
};
pub use scene::RuntimeSpawnedEntity;

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
    /// `audio` 绑定的能力单元（`docs/62` P1）：绑定住在 `rustlua-audio`，宿主在这里持有同一份
    /// 状态，用来定音效目录、暂停/恢复 BGM、每帧驱动曲尾续播。
    ///
    /// 与 `script` 是**两个独立对象**：`LuaRuntime` 不知道自己装了哪些领域模块，它只认注册表。
    pub(crate) audio: rustlua_lua_api::Handle<rustlua_audio::audio::AudioSystem>,
    /// `input` 绑定的能力单元（`docs/62` P1 阶段 3）：绑定住在 `rustlua-input`，宿主在这里
    /// 持有同一份状态——喂快照、读动作映射、把震动请求交给设备层都走它。
    ///
    /// 与 `script` 一样是"宿主每帧推、Lua 读"的单向关系：脚本不反向驱动宿主。
    pub(crate) input: rustlua_input::host::HostInput,
    /// 自上次 `take_script_micros` 以来所有固定步里 Lua `update()` 的累计耗时
    /// （微秒）。宿主把它送进 `PerformanceStats::lua_ms`，性能面板因此显示的是
    /// "Lua 花了多少"而不是恒为 0（P4 接线）。
    pub(crate) script_micros: u64,
    pub(crate) sprite_batch: SpriteBatch,
    /// 每精灵顶点生成的并行度（WP8）。默认取检测到的核数；`Parallelism::SEQUENTIAL`
    /// 即完全回滚到单线程，输出逐字节相同（见 `rustlua_infra::par` 与 `sprite_batch` 的测试）。
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
    pub(crate) spatial_grid: Option<rustlua_render::spatial_index::SpatialGrid2D>,
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
    /// 启动场景的来源（只在 `new`/`new_with_layout` 路径上有值；`from_scene*` 是显式文档）。
    /// 见 [`StartupSceneReport`]：宿主用它区分"独立运行时"与"真的加载了工程的启动场景"。
    pub(crate) startup_scene: Option<StartupSceneReport>,
    /// P3-R03-4：场景 `Environment` 组件的加载结果（没有声明时为 `None`）。
    pub(crate) environment: Option<environment::RuntimeSceneEnvironment>,
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
    /// 不能走 hinting 档的字体（读不到 / 不带 TrueType 指令表）。同一字体失败一次即记住，
    /// 后续帧直接回退 SDF，不再重复读盘或重复告警。
    pub(crate) missing_hinted_fonts: HashSet<String>,
    /// 设计分辨率 → 实际附件 的等比 contain 比例（宿主声明）。hinting 只在
    /// 1 texel = 1 物理像素时锐利，所以 ppem 要按**物理**像素取整；默认 1.0
    /// 表示"设计像素 = 物理像素"（离屏/编辑器/探针的常见形态）。
    pub(crate) render_scale: f32,
    /// hinting 覆盖率图集缓存（key = `font://hinted/<ppem>/<asset>`）。编辑器注入
    /// 同一实例供场景视图/预览跨运行时重建复用。
    pub(crate) hinted_atlases: HintedAtlasMap,
    /// 字体字节缓存（key = 字体资源路径，空串 = 默认字体）。同一字体在多个 ppem
    /// 下各建一张图集，字节只留一份；`None` 表示已知读不到，不再每次重试。
    pub(crate) hinted_font_bytes: HashMap<String, Option<SharedFontBytes>>,
    /// 每张 hinting 图集"上次上报版本"（语义同 `custom_font_versions`）。
    pub(crate) hinted_versions: HashMap<String, u64>,
    /// 本帧布局用到、待上报渲染器的 hinting 图集 key。
    pub(crate) frame_hinted_keys: Vec<String>,
    pub(crate) text_render_mode: TextRenderMode,
    pub(crate) ui_scene_nodes: BTreeMap<u64, UiSceneNode>,
    pub(crate) ui_layout_rects: BTreeMap<u64, ResolvedUiNode>,
    /// 交互路由开关（M80-01 起默认开；关闭后回到旧点击路径，见 `ui_input` 模块文档）。
    pub(crate) ui_interaction_enabled: bool,
    pub(crate) ui_keyboard_nav: bool,
    /// 跨帧交互状态：命中树（hover/pressed/focus/scroll 的唯一定义处）。
    pub(crate) ui_interaction: rustlua_ui::ui_interaction::UiInteraction,
    /// 上次建树时的数据签名；相等即不重建。
    pub(crate) ui_interaction_signature: Option<u64>,
    /// 交互 key → 该实体的结构化事件绑定。
    pub(crate) ui_bindings: BTreeMap<String, rustlua_ui::ui_events::UiEvents>,
    /// 交互 key → 实体名（key 在重名时带槽位后缀，派发仍用原名）。
    pub(crate) ui_key_names: BTreeMap<String, String>,
    /// 交互 key → 槽位（控件路由按槽位找 `Toggle`/`Slider`/`Scrollbar` 组件）。
    pub(crate) ui_key_slots: BTreeMap<String, u64>,
    /// M80-04：正在拖拽的滑块/滚动条（交互 key）；左键抬起即清空。
    pub(crate) ui_control_drag: Option<String>,
    /// 实体名 → 当前视觉状态（渲染侧按名字取状态贴图）。
    pub(crate) ui_visual_states: BTreeMap<String, rustlua_ui::ui_states::ButtonVisualState>,
    /// 实体名 → 进行中的状态底色过渡（M80-07）。没有配 `transition` 的节点不进这张表。
    pub(crate) ui_state_transitions: rustlua_ui::ui_states::UiStateTransitions,
    /// 实体名 → 滚动偏移（`ScrollView`；由交互树写入，布局解算时消费）。
    pub(crate) ui_scroll_offsets: BTreeMap<String, f32>,
    /// 槽位 → 滚动范围（内容尺寸来自布局树子节点包围盒）。
    pub(crate) ui_scroll_ranges: BTreeMap<u64, rustlua_ui::ui_interaction::UiScroll>,
    /// 槽位 → 可见裁剪矩形（滚动视口交集）；绘制与命中都读它。
    pub(crate) ui_clip_rects: BTreeMap<u64, rustlua_ui::ui_interaction::UiRect>,
    /// 槽位 → 相对布局矩形的滚动位移（内容空间 → 屏幕空间的偏移量）。
    ///
    /// 交互树按 `UiScroll` 自己平移子节点，喂进去的矩形必须是**扣回位移**的内容空间值；
    /// 绘制/旧点击路径则要屏幕空间值。两者相差的就是这张表。
    pub(crate) ui_scroll_shifts: BTreeMap<u64, [f32; 2]>,
    /// 槽位 → `CanvasGroup` 沿父链相乘后的最终 alpha（只存 ≠ 1 的条目）。
    pub(crate) ui_group_alpha: BTreeMap<u64, f32>,
    pub(crate) ui_focused: Option<String>,
    /// M76-02：`UiBindings` 的跨帧 resolve 状态（只发变化过的目标）。
    pub(crate) ui_binding_runtime: rustlua_ui::ui_binding::BindingRuntime,
    /// `image.visible=false` 的目标名（渲染与命中都读它；绑定删除后自动失效）。
    pub(crate) ui_binding_hidden_images: BTreeSet<String>,
    /// 最近一次 resolve 的诊断（目标实体/组件/进度条缺失）。
    pub(crate) ui_binding_report: Vec<String>,
    /// 已经并进 `ui_scene_nodes` 的**运行期 spawn** 节点（脚本侧报告的那一份的副本）。
    /// 判断"有没有改动"用的是**修订号**（见 [`Self::applied_prefab_ui_revision`]），
    /// 不是每帧比对这两张表——比对前得先把脚本那侧的表克隆出来，所以原注释里"没变就一个分配
    /// 都不做"在实现上是**假的**（克隆发生在比较之前，见 `docs/60` §10.3 第 2 条）。
    pub(crate) applied_prefab_ui_nodes: BTreeMap<u64, UiSceneNode>,
    /// 上一次并表时脚本侧报告的 `prefab_ui_nodes` 修订号：相等即"没变"，宿主**直接早退**，
    /// 一次 clone 与一次 O(n) 比较都不做。
    pub(crate) applied_prefab_ui_revision: u64,
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
    pub(crate) spatial_mode: rustlua_scene::scene_space::SpatialMode,
    /// docs/54 §10 批 R：上一帧传播用的**局部真源指纹**（`(hash, 组件数, Parent 数)`）。
    /// 与上一帧相同 ⇒ 世界值仍然是上一帧算出来的那一份，整趟跳过传播。
    pub(crate) spatial_fingerprint: Option<(u64, usize, usize)>,
    /// 跳过策略的开关（回滚档：`false` = 每帧全量传播，与收敛前逐字节一致）。
    pub(crate) spatial_skip_unchanged: bool,
    /// 累计"因为输入没变而跳过传播"的帧数（诊断/验收用）。
    pub(crate) spatial_skipped_frames: u64,
    /// docs/54 §10 批 S：**逐实体增量**传播的状态（上一帧的输入快照 + 父先序求解顺序）。
    pub(crate) spatial_incremental: rustlua_ecs::hierarchy::SpatialIncremental,
    /// 增量策略的开关（回滚档：`false` = 指纹变化后一律全量传播，与批 S 之前逐字节一致）。
    pub(crate) spatial_incremental_enabled: bool,
    /// 最近一次传播走的路径（`Skipped` / `Full` / `Incremental`）——门与验收读它。
    pub(crate) last_spatial_path: rustlua_ecs::hierarchy::SpatialPath,
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
    /// P3-R07-1：渲染器有没有 compute 能力（宿主拿到渲染器时告知一次）。
    ///
    /// 默认 `false`：没人接线时，`mode=gpu` 的发射器只是按"设备没有 compute"回 CPU，
    /// 报告不会谎称 GPU 已生效。
    pub(crate) particle_compute_available: bool,
    /// P3-R07-1：最近一步场景发射器的落点分布（诊断与验收用；不参与玩法）。
    pub(crate) last_particle_placement: ParticlePlacementReport,
    /// P3-R07-2：最近一次 GPU burst 派发的结果（诊断与验收用；不参与玩法）。
    pub(crate) last_particle_dispatch: ParticleDispatchReport,
    /// P3-R07-2：自上次派发以来，"入队了但宿主没来取"而被倒回 CPU 池的 burst 数。
    pub(crate) particle_deferred: usize,
    /// 已经就哪些回退原因喊过一层的位图（`ParticleCpuReason::index` 为位）。
    /// 按原因去重而不是"只喊一次"：同一步里一个 `continuous_rate` 加一个 `no_compute`
    /// 会逐帧交替出现，"上次原因"式的去重会每帧刷屏。
    pub(crate) particle_warned: u32,
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

/// P3-R07-1：一步里**场景声明的发射器**的落点分布（诊断/验收用，不参与玩法）。
///
/// 语义边界：`gpu` 一侧的粒子只活在显存里，Lua 与玩法**读不到**它们的位置。R07-1 只交付
/// 落点判定与回退标记，GPU 派发在 R07-2 —— 所以本步的 `gpu` 恒为 0，`cpu` 是"照旧在 CPU
/// 上发射"的真实计数（含回退、含尚未接线的 gpu 请求）。字段按 R07-2 的形状一次定好，
/// 避免那时再改一遍所有读它的测试与报告。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ParticlePlacementReport {
    /// 本步在 CPU 侧发射的发射器数（每个都真的发了粒子）。
    pub cpu: usize,
    /// 本步真的由 GPU 模拟的发射器数（R07-1 = 0）。
    pub gpu: usize,
    /// 作者声明了 `mode=gpu` 的发射器数（无论最终落在哪侧）。
    pub gpu_requested: usize,
    /// 本步第一个回退原因（`None` = 没有"要了 gpu 却跑 cpu"）。
    pub fallback: Option<ParticleCpuReason>,
    /// 本步出现过的**全部**回退原因位图（`ParticleCpuReason::index` 为位）。
    /// 单靠 `fallback` 会漏掉同一步里的另一种原因，逐原因去重的日志因此要用它。
    pub fallback_bits: u32,
}

impl ParticlePlacementReport {
    /// 记一个发射器的判定结果。
    fn record(&mut self, placement: ParticlePlacement) {
        // R07-2 之前没有派发通道：判定为 `Gpu` 的发射器照旧在 CPU 池发射，原因记 not_wired。
        let effective = match placement {
            ParticlePlacement::Gpu => ParticleCpuReason::NotWired,
            ParticlePlacement::Cpu(reason) => reason,
        };
        self.cpu += 1;
        if effective != ParticleCpuReason::Requested {
            self.gpu_requested += 1;
        }
        if effective.is_fallback() {
            self.fallback.get_or_insert(effective);
            self.fallback_bits |= 1 << effective.index();
        }
    }
}

/// P3-R07-2：一次 **burst 派发通道**的结果（诊断/验收用，不参与玩法）。
///
/// 为什么场景发射器的报告（[`ParticlePlacementReport`]）不够用：发射器按 `rate` 连续发射，
/// 而 compute 侧只实现了一次性 burst，所以场景发射器恒定回退 `continuous_rate`——它测不出
/// 派发通道本身通不通。通道的一端是脚本/宿主的 `mode=gpu` burst（Lua `particles.burst`），
/// 另一端是渲染器的 GPU 粒子池，本结构就是这两端之间的账。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ParticleDispatchReport {
    /// 本次从脚本侧取到的 GPU burst 条数（含随后被拒的）。
    pub requested: usize,
    /// 渲染器**真的收下**的条数（保守口径：收下才加一）。
    pub gpu: usize,
    /// 退回 CPU 池的条数（能力缺失 / 参数不支持 / 池预算 / dispatch 失败）。
    pub fallback: usize,
    /// 上一次派发之后没被宿主取走、在本帧开头倒回 CPU 池的条数（`not_wired` 的实证）。
    pub deferred: usize,
    /// 脚本侧队列满、当场落 CPU 的累计次数增量。
    pub overflow: usize,
    /// 本次第一个回退原因（`None` = 全留在 GPU 或全在 CPU 池外）。
    pub fallback_reason: Option<ParticleCpuReason>,
    /// 派发并步进之后仍在 GPU 池里的系统数。
    pub systems: usize,
    /// 本次步进回收（寿命走完）的系统数。
    pub recycled: usize,
    /// GPU 侧累计步进次数（池里最老的系统）。
    pub steps: u64,
    /// GPU 池里的粒子总数（`read_gpu_particles` 的槽位数口径）。
    pub gpu_particles: usize,
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
    /// M72-02：逐格 tint（顶点色）。无 tint 的格是 `[1,1,1,1]`，与旧的硬编码常量一致。
    tint: [f32; 4],
}

/// 分量级相等：测试里按分量逐一比较（Vec2 未实现 PartialEq，数值由同一公式
/// 算出故精确相等）。M72-02 后 tint 也是产出像素的一部分，同样纳入比较。
impl PartialEq for CachedTileSprite {
    fn eq(&self, other: &Self) -> bool {
        self.center_offset.x == other.center_offset.x
            && self.center_offset.y == other.center_offset.y
            && self.uv_min.x == other.uv_min.x
            && self.uv_min.y == other.uv_min.y
            && self.uv_max.x == other.uv_max.x
            && self.uv_max.y == other.uv_max.y
            && self.tint == other.tint
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
    /// 全部文本用整数 ppem 的 hinting 覆盖率图集（`sdf=3`）：小字号下笔画被
    /// 字体自带的网格适配吸附到整像素，锐利度与原生 GDI/DirectWrite 一致。
    /// 字体没有 TrueType 指令表时仍可用（退化为未 hinting 的准确 AA），但大字号
    /// 会明显不如矢量档平滑——调用方按项目字号档位选择。
    Hinted,
}

impl TextRenderMode {
    fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "vector" | "outline" => Self::Vector,
            "sdf" | "msdf" => Self::Sdf,
            "hinted" | "hinting" | "ttf" => Self::Hinted,
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

    /// 脚本是否要求收字（`input.text_events{ capture = true }`，M79-04）。
    ///
    /// 宿主据此开/关系统输入法：Windows 上输入法一旦接管，玩法按键会先进组合窗
    /// （中文输入法下按 W 不再前进），所以默认必须关着；只有字段真的在等字才开。
    #[must_use]
    pub fn text_capture_requested(&self) -> bool {
        self.input.text_capture()
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
/// `rustlua_ui::ui_scene_layout::build_ui_scene_nodes`。运行期 `prefab.spawn` 把同一份
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
    rustlua_ui::ui_scene_layout::build_ui_scene_nodes_with_intrinsic(
        scene.entities.values().filter_map(|entity| {
            Some(UiSceneNodeSource {
                key: *id_to_key.get(&entity.id.0)?,
                parent: entity.parent.and_then(|id| id_to_key.get(&id.0)).copied(),
                components: &entity.components,
            })
        }),
        &mut |text, scale| {
            rustlua_render::text::measure_builtin_text(text, scale).map(|(size, _)| size)
        },
    )
}

/// 启动场景的来源。刻意分开"没声明"和"声明了但加载失败"：
/// 前者是引擎自带的诊断场景（独立运行时模式），后者是**真错误**，不能再悄悄换一张场景。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StartupSceneSource {
    /// 没有工程根，或 `project.toml` 没有 `startup_scene` 键：引擎自带诊断场景。
    Standalone,
    /// `project.toml` 声明并成功加载的启动场景。
    Project { path: PathBuf },
}

/// 启动场景的加载结果（宿主/测试可读，不再只有"一个 Scene"）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartupSceneReport {
    pub source: StartupSceneSource,
    /// `project.toml` 里声明的相对路径（未声明时 `None`）。
    pub requested: Option<String>,
}

/// 引擎自带的诊断场景：独立运行时（没有工程根）与"工程未声明启动场景"时使用。
fn standalone_scene() -> Scene {
    let mut scene = Scene::new("runtime-main");
    let _ = scene.add_entity(SceneEntityId(1), "SpriteRoot", None);
    let entity = scene.entities.get_mut(&SceneEntityId(1)).unwrap();
    entity.components.insert(
        "Transform".into(),
        "position=640,360,0;rotation=0,0,0,1;scale=1,1,1".into(),
    );
    entity.components.insert(
        "Sprite".into(),
        "texture=assets/textures/red-1x1.png".into(),
    );
    scene
}

/// project.toml 的 `text_render_mode` 键；无效/缺省回退 `auto`。
fn project_text_render_mode(project_root: Option<&Path>) -> TextRenderMode {
    project_root
        .and_then(|root| project_font::read_key(root, "text_render_mode"))
        .map(|mode| TextRenderMode::parse(&mode))
        .unwrap_or(TextRenderMode::Auto)
}

/// 读取项目声明的启动场景。
///
/// 三种结果必须分开：
/// - 没有工程根 → 引擎诊断场景（独立运行时，合法模式）；
/// - 工程**没有声明** `startup_scene` → 引擎诊断场景（同样是合法模式，记一条 info）；
/// - 工程**声明了**但加载失败 → `Err`。以前这里 `.ok()` 后落进合成场景：文件丢了、语法坏了，
///   宿主照样起画面，只是内容不是工程的——这是最典型的静默回退。
fn load_runtime_scene(project_root: Option<&Path>) -> anyhow::Result<(Scene, StartupSceneReport)> {
    let Some(root) = project_root else {
        return Ok((
            standalone_scene(),
            StartupSceneReport {
                source: StartupSceneSource::Standalone,
                requested: None,
            },
        ));
    };
    let Some(relative) = project_font::read_key(root, "startup_scene") else {
        tracing::info!(
            target: "scene",
            root = %root.display(),
            "[scene] project.toml has no startup_scene; using the engine diagnostic scene"
        );
        return Ok((
            standalone_scene(),
            StartupSceneReport {
                source: StartupSceneSource::Standalone,
                requested: None,
            },
        ));
    };
    // 启动场景是第一个上下文点；之后的切换由脚本层 `scene.load` 打点。
    rustlua_infra::vfs::note_load_context(&relative);
    let full = root.join(&relative);
    // 先走文件系统（桌面工程/发布包解包目录的常规路径，语义一个字都不改）。
    let loaded = Scene::load(&full).or_else(|error| {
        // 没有 `std::fs` 的宿主（web / 微信小游戏）：场景字节在宿主的资源表里，
        // 经 `vfs` provider 取。**这不是"第二条解析实现"**——同样落在
        // `Scene::from_packed_bytes` → `from_text`，所以两条路读出来的场景一致。
        // 失败时保留**第一次**的错误（它才是"文件到底在不在"的答案）。
        match rustlua_infra::vfs::read(&full) {
            Ok(bytes) => Scene::from_packed_bytes(&full, &bytes),
            Err(_) => Err(error),
        }
    });
    match loaded {
        Ok(scene) => {
            tracing::info!(
                target: "scene",
                path = %full.display(),
                entities = scene.entities.len(),
                "[scene] startup scene loaded"
            );
            Ok((
                scene,
                StartupSceneReport {
                    source: StartupSceneSource::Project { path: full },
                    requested: Some(relative),
                },
            ))
        }
        Err(error) => {
            tracing::error!(
                target: "scene",
                path = %full.display(),
                error = %error,
                "[scene] declared startup scene failed to load; refusing synthetic-scene fallback"
            );
            Err(anyhow::anyhow!(
                "project.toml 声明的启动场景 {} 加载失败：{error}",
                full.display()
            ))
        }
    }
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
    // 混合渲染：覆盖率图集（hinting）sdf=3（Nearest + 直接用覆盖率），bitmap 模式
    // sdf=2（Nearest + 阈值化），MSDF 模式 sdf=1（Linear + smoothstep）。
    let sdf_value: u32 = if mesh.coverage {
        3
    } else if mesh.bitmap {
        2
    } else {
        u32::from(is_sdf)
    };
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
