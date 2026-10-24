//! P3-R05-2/3：shader 源覆盖目录 → **原子热重载**。
//!
//! 这一层做的事很窄，但每条都是硬要求（计划 §6.5）：
//!
//! 1. **接口门**：新源与当前生效源的绑定接口（group/binding/地址空间/类型）必须逐 slot 相同。
//!    bind group layout 是 Rust 侧常量，不会跟着 shader 重建；接口漂移的 shader 塞进旧布局
//!    要么在校验期报错，要么更糟——读到错位的 buffer 而画面"看起来还行"。这里直接拒绝。
//! 2. **全成功才换**：本次要重建的每个变体先在局部建好，任意一步失败就返回错误、
//!    一个字节都不改（旧管线继续画）。半替换会让"哪条管线是新哪条是旧"变成不可复现的问题。
//! 3. **内容没变不重编**：变体的预处理结果与当前生效源逐字相同 ⇒ 跳过编译，记 `unchanged`。
//!    编辑器每次保存都会触发一次检查，不做这条就等于每 150ms 重编一遍管线。
//! 4. **坏源要能看懂**：解析/预处理/设备校验三类错误都带**原文件 + 变体名**，
//!    设备校验错误用 `push_error_scope` 变成数据而不是 panic/abort（与 `capabilities.rs`、
//!    `compute.rs` 同一条路）。
//!
//! 覆盖目录只在原生侧存在：wasm/release 没有入口（`host::shader_library` 本身也是
//! `#[cfg(not(target_arch = "wasm32"))]`）。

use std::{
    borrow::Cow,
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

// wasm32-unknown-unknown 的 std `Instant::now()` 直接 panic；`web_time` 在原生上是同一类型。
use web_time::Instant;

use super::DepthPass;
use super::Renderer;
use super::construct::{
    build_fullscreen_pipeline, build_textured_pipeline, build_vector_text_pipeline,
    build_world_sprite_pipeline, create_mesh_pipeline, create_shadow_pipeline,
    module_from_preprocessed, preprocessed_variant, variant_root,
};
use super::pipeline_cache::{ColorTarget, PipelineFamily, ShaderVariantKey, Shading, SkinMode};
use crate::shader::{ALL_SHADERS, ShaderSource};
use crate::shader_library::{ResolvedShader, ShaderLibrary, ShaderOrigin, ShaderOverrideError};
use crate::shader_preprocess::PreprocessedShader;
use crate::shader_reload::{
    ShaderValidateError, interface_of, line_of_offset, parse_interface, require_entry_points,
    validate_module,
};
use rustlua_render::render_settings::IblMode;

/// 单个根 shader 在本次重载里的状态（面板与 Console 要能认出"现在用的是哪份源"）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShaderSourceStatus {
    /// 稳定短名（`mesh3d` / `mesh3d_pbr` / `shadow` …）。
    pub name: &'static str,
    /// 仓库内源路径；覆盖文件应当叫同名文件。
    pub repo_path: &'static str,
    /// 生效源来自覆盖目录还是内置。
    pub overridden: bool,
    /// 覆盖文件的绝对路径（`overridden == false` 时为 `None`）。
    pub override_path: Option<PathBuf>,
    /// 生效源的 FNV-1a 哈希；与覆盖目录里的文件逐字节对应。
    pub hash: u64,
}

impl ShaderSourceStatus {
    /// `内置` 或覆盖文件路径（一行，供面板与摘要用）。
    #[must_use]
    pub fn origin_text(&self) -> String {
        match (&self.override_path, self.overridden) {
            (Some(path), true) => path.display().to_string(),
            _ => "内置".to_owned(),
        }
    }
}

/// 一次成功重载的结果。全部是**读数**，供 Profiler/Console/验收样例打印。
#[derive(Clone, Debug)]
pub struct ShaderReloadReport {
    /// 本次生效的覆盖目录（`None` = 内置源）。
    pub override_dir: Option<PathBuf>,
    /// 重建成功的变体数。
    pub rebuilt: usize,
    /// 内容与当前生效源一致、因此没有重编的变体数。
    pub unchanged: usize,
    /// 缓存里的变体总数（`rebuilt + unchanged == variants`，测试钉住这条不变量）。
    pub variants: usize,
    /// 涉及的根 shader 状态（按名字排序）。
    pub sources: Vec<ShaderSourceStatus>,
    /// 本次因"覆盖文件被删掉"而回退到内置源的 shader 名（计划要求：回退 + 告警）。
    pub fell_back_to_builtin: Vec<String>,
    /// CPU 侧校验（naga 解析 + 接口比对）耗时。
    pub validate_ms: f64,
    /// GPU 侧编译（shader module + render pipeline）耗时。
    pub compile_ms: f64,
}

impl ShaderReloadReport {
    /// 一行摘要（状态栏用；细节走 Console 与 `tracing`）。
    #[must_use]
    pub fn summary(&self) -> String {
        let dir = match &self.override_dir {
            Some(dir) => dir.display().to_string(),
            None => "内置源".to_owned(),
        };
        format!(
            "重载 {}/{} 个变体（未变 {}）· {dir} · 校验 {:.1}ms · 编译 {:.1}ms",
            self.rebuilt, self.variants, self.unchanged, self.validate_ms, self.compile_ms
        )
    }

    /// 生效源清单（多行，Console/日志用）。
    #[must_use]
    pub fn sources_text(&self) -> String {
        let mut out = String::new();
        for source in &self.sources {
            out.push_str(&format!(
                "  {} hash={:016x} <- {}\n",
                source.name,
                source.hash,
                source.origin_text()
            ));
        }
        out
    }
}

/// 重载失败的原因。除 `Override` 外每条都带**变体名**：同一份源在不同变体下可能只有一个失败。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShaderReloadError {
    /// 覆盖文件存在但读不出来（与"文件不存在 = 回退内置"严格分开）。
    Override { name: String, reason: String },
    /// 预处理失败（`//#if` 拼写/指令配对/include）。
    Preprocess { variant: String, message: String },
    /// `naga` 解析失败、缺入口点，或绑定接口与当前生效源不同。
    Validate(ShaderValidateError),
    /// 设备侧校验错误（wgpu）：着色器编译不过或管线描述不合法。
    Device { variant: String, message: String },
    /// 这台设备没有存储缓冲，3D 变体是占位管线，热重载没有可重建的对象。
    ///
    /// 这不是失败（调用方按"没有变化"处理），但必须**显式**说出来：否则作者会以为
    /// 是自己的覆盖文件写错了。
    PlaceholderOnly,
}

impl std::fmt::Display for ShaderReloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Override { name, reason } => {
                write!(f, "shader 覆盖文件读不出来（{reason}）：{name}")
            }
            Self::Preprocess { variant, message } => {
                write!(f, "预处理失败（变体 {variant}）：{message}")
            }
            Self::Validate(error) => write!(f, "{error}"),
            Self::Device { variant, message } => {
                write!(f, "GPU 管线重建失败（变体 {variant}）：{message}")
            }
            Self::PlaceholderOnly => write!(
                f,
                "设备不支持存储缓冲：3D 通道是占位管线，覆盖目录对本次绘制没有效果"
            ),
        }
    }
}

impl std::error::Error for ShaderReloadError {}

impl From<ShaderOverrideError> for ShaderReloadError {
    fn from(error: ShaderOverrideError) -> Self {
        Self::Override {
            name: error.name,
            reason: error.reason.to_owned(),
        }
    }
}

/// 生效源的一份快照。`text` 用 `Cow`：内置路径零拷贝，覆盖路径才分配。
///
/// 这份快照在**提交时**记进 `Renderer::shader_committed`：比较的基准是"管线里现在编译的是
/// 哪份源"，而不是"覆盖目录此刻解析出什么"。删掉覆盖文件时两者会分叉——按后者比较就查不出
/// "文件没了、画面还红着"这件事，回退也就永远不会发生。
#[derive(Clone)]
pub struct ActiveSource {
    text: Cow<'static, str>,
    /// 报错用的路径：有覆盖文件时是它的**绝对路径**（作者改的是这个文件，不是仓库里那份）。
    /// 预处理的 `root_name` 与 `naga` 的文件标签都用它，Console 指过去的行号才对得上。
    error_path: String,
    hash: u64,
    overridden: bool,
    override_path: Option<PathBuf>,
}

impl ActiveSource {
    /// 由一次解析结果构造快照。`resolved == None` 只可能是"名字不在注册表里"，那是
    /// 解析层与注册表脱节的实现错误，不能当成"没有覆盖文件"处理（那会悄悄换一份源）。
    fn new(
        root: &'static ShaderSource,
        resolved: Option<ResolvedShader>,
    ) -> Result<Self, ShaderReloadError> {
        let Some(resolved) = resolved else {
            return Err(ShaderReloadError::Override {
                name: root.name.to_owned(),
                reason: "名字不在 shader 注册表里".to_owned(),
            });
        };
        let override_path = match &resolved.origin {
            ShaderOrigin::Override { path } => Some(path.clone()),
            ShaderOrigin::Builtin => None,
        };
        Ok(Self {
            text: resolved.source,
            error_path: match &override_path {
                Some(path) => path.display().to_string(),
                None => resolved.repo_path.to_owned(),
            },
            hash: resolved.hash,
            overridden: resolved.origin.is_override(),
            override_path,
        })
    }
}

/// 一次重载过程中对每个根 shader 的记账（提交前全部只在局部）。
#[derive(Default)]
struct ReloadLedger {
    /// 根名 → 本次生效源的状态（报告按名字排序输出）。
    roots: HashMap<&'static str, ShaderSourceStatus>,
    /// 根名 → 提交后管线里编译的源（下一次重载的比较基准）。
    committed: HashMap<&'static str, ActiveSource>,
    /// 因覆盖文件被删除而回退内置源的 shader 名。
    fell_back: Vec<String>,
}

/// 重建 2D 固定槽位所需的管线布局，构造期存进渲染器。
///
/// 布局**不**跟着 shader 重建（和 `PipelineCache` 那三条 3D 布局同一个理由）：接口门就是为
/// 了防止"新 shader 塞进旧布局"。这里存的正是构造期那几份，重建管线时逐字段复用。
pub(super) struct FixedSlotLayouts {
    pub(super) textured: wgpu::PipelineLayout,
    pub(super) vector_text: wgpu::PipelineLayout,
    pub(super) world_sprite: wgpu::PipelineLayout,
    pub(super) post_process: wgpu::PipelineLayout,
}

/// 没有变体轴、直接存在渲染器字段上的 2D 管线槽位（P3-R05-3 的"全 shader"）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixedSlot {
    /// `textured.wgsl`：2D 叠层 + SDF 文本，同一份源、两条只差标签的管线。
    Textured,
    /// `vector_text.wgsl`。
    VectorText,
    /// `world_sprite.wgsl`：半透明 + 不透明，HDR 档再多两条只差格式的管线。
    WorldSprite,
    /// `post_process.wgsl`。
    PostProcess,
    /// `tonemap.wgsl`：能力不足时构造期根本没有这条管线。
    Tonemap,
}

impl FixedSlot {
    /// 每次重载都要检查的槽位（顺序稳定：报告与测试都按它排）。
    const ALL: [Self; 5] = [
        Self::Textured,
        Self::VectorText,
        Self::WorldSprite,
        Self::PostProcess,
        Self::Tonemap,
    ];

    /// 槽位对应的根 shader。
    fn root(self) -> &'static ShaderSource {
        match self {
            Self::Textured => &crate::shader::TEXTURED,
            Self::VectorText => &crate::shader::VECTOR_TEXT,
            Self::WorldSprite => &crate::shader::WORLD_SPRITE,
            Self::PostProcess => &crate::shader::POST_PROCESS,
            Self::Tonemap => &crate::shader::TONEMAP,
        }
    }

    /// 报错与日志用的槽位名（与管线标签同源）。
    fn label(self) -> &'static str {
        match self {
            Self::Textured => "textured",
            Self::VectorText => "vector_text",
            Self::WorldSprite => "world_sprite",
            Self::PostProcess => "post_process",
            Self::Tonemap => "tonemap",
        }
    }
}

/// 一条槽位重建出来的管线。形状必须与构造期一一对应：重载既不能凭空造出构造期没有的管线
/// （HDR/SDF 缺档），也不能把它丢掉。
enum BuiltSlots {
    Textured {
        pipeline: wgpu::RenderPipeline,
        pipeline_sdf: wgpu::RenderPipeline,
    },
    VectorText {
        pipeline: wgpu::RenderPipeline,
    },
    WorldSprite {
        translucent: wgpu::RenderPipeline,
        opaque: wgpu::RenderPipeline,
        hdr: Option<(wgpu::RenderPipeline, wgpu::RenderPipeline)>,
    },
    PostProcess {
        pipeline: wgpu::RenderPipeline,
    },
    Tonemap {
        pipeline: Option<wgpu::RenderPipeline>,
    },
}

impl BuiltSlots {
    /// 提交：全部槽位编译成功后才会走到这里（原子的后半段）。
    fn commit(self, renderer: &mut Renderer) {
        match self {
            Self::Textured {
                pipeline,
                pipeline_sdf,
            } => {
                renderer.pipeline = pipeline;
                renderer.pipeline_sdf = pipeline_sdf;
            }
            Self::VectorText { pipeline } => renderer.vector_pipeline = pipeline,
            Self::WorldSprite {
                translucent,
                opaque,
                hdr,
            } => {
                renderer.world_sprite_pipeline = translucent;
                renderer.world_sprite_opaque_pipeline = opaque;
                if let Some((hdr_translucent, hdr_opaque)) = hdr {
                    renderer.hdr_world_sprite_pipeline = Some(hdr_translucent);
                    renderer.hdr_world_sprite_opaque_pipeline = Some(hdr_opaque);
                }
            }
            Self::PostProcess { pipeline } => renderer.post_process_pipeline = pipeline,
            Self::Tonemap { pipeline } => renderer.tonemap_pipeline = pipeline,
        }
    }
}

impl Renderer {
    /// 打开/关闭开发期覆盖目录，并按需重建全部已有变体。
    ///
    /// `None` 关掉覆盖（回到内置源）。返回 `Err` 时**什么都没改**：旧管线、旧源、旧目录全部
    /// 保持原样，调用方只负责把错误显示出来。
    pub fn set_shader_override_dir(
        &mut self,
        dir: Option<impl Into<PathBuf>>,
    ) -> Result<ShaderReloadReport, ShaderReloadError> {
        let library = match dir {
            Some(dir) => ShaderLibrary::with_override_dir(dir),
            None => ShaderLibrary::builtin(),
        };
        self.apply_shader_library(&library)
    }

    /// 当前生效的覆盖目录（`None` = 内置源）。
    #[must_use]
    pub fn shader_override_dir(&self) -> Option<&Path> {
        self.shader_library.override_dir()
    }

    /// 最近一次成功重载的读数（`None` = 从没重载过）。
    #[must_use]
    pub fn shader_reload_report(&self) -> Option<&ShaderReloadReport> {
        self.shader_reload.as_ref()
    }

    /// 当前生效的源清单（内置/覆盖 + 哈希）。面板显示"这帧用的到底是哪份 shader"就读它，
    /// 不必等到第一次重载。覆盖文件读不出来的那一项哈希记 `0`（面板要显示"存在但不可用"，
    /// 而不是把这一行悄悄藏起来）。
    #[must_use]
    pub fn shader_sources(&self) -> Vec<ShaderSourceStatus> {
        let mut seen = Vec::new();
        for root in ALL_SHADERS {
            let (overridden, override_path, hash) = match self.shader_library.resolve(root.name) {
                Ok(Some(resolved)) => match resolved.origin {
                    ShaderOrigin::Override { path } => (true, Some(path), resolved.hash),
                    ShaderOrigin::Builtin => (false, None, resolved.hash),
                },
                Ok(None) => continue,
                Err(error) => (true, Some(error.path), 0),
            };
            seen.push(ShaderSourceStatus {
                name: root.name,
                repo_path: root.path,
                overridden,
                override_path,
                hash,
            });
        }
        seen
    }

    /// 覆盖目录里有任何变化就重载；没有变化返回 `Ok(None)`，一次编译都不做。
    ///
    /// 轮询侧只需要 debounce（150ms）+ 这一条：文件事件不携带"变的是哪个 shader"，而变体
    /// 集合很小，逐变体比哈希比维护一张"路径 → 变体"反查表更不容易出错。
    pub fn reload_shaders_if_changed(
        &mut self,
    ) -> Result<Option<ShaderReloadReport>, ShaderReloadError> {
        if self.shader_library.override_dir().is_none() {
            return Ok(None);
        }
        let current = self.shader_library.clone();
        match self.apply_shader_library(&current) {
            Ok(report) if report.rebuilt == 0 => Ok(None),
            Ok(report) => Ok(Some(report)),
            Err(error) => Err(error),
        }
    }

    /// 重载的实现体：逐根 shader 校验 → 全部编译 → 一次提交。
    ///
    /// 检查范围是**全部** 8 个内置 shader：`PipelineCache` 里的 3D 变体（mesh3d /
    /// mesh3d_pbr / shadow 的每条轴）加上 5 个没有变体轴的 2D 固定槽位。计划 R05-3 的
    /// "全 shader"就是这个意思——只让 3D 通道热重载、改 `textured.wgsl` 没反应，是半个功能。
    fn apply_shader_library(
        &mut self,
        proposed: &ShaderLibrary,
    ) -> Result<ShaderReloadReport, ShaderReloadError> {
        let keys = self.pipelines.keys();
        if keys.is_empty() || !self.storage_capable {
            // 占位档（WebGL2 之类）里 3D 变体不是真管线；明确报告，不静默。
            return Err(ShaderReloadError::PlaceholderOnly);
        }
        let variant_count = keys.len() + FixedSlot::ALL.len();
        let started = Instant::now();
        let mut pending: Vec<(ShaderVariantKey, PreprocessedShader)> = Vec::new();
        let mut pending_slots: Vec<(FixedSlot, PreprocessedShader)> = Vec::new();
        let mut unchanged = 0usize;
        let mut ledger = ReloadLedger::default();
        for key in keys {
            let root = variant_root(&key);
            let (active, next) = self.stage_root(proposed, &mut ledger, root)?;
            match self.stage_variant(&active, &next, &key.label(), key.skin, key.ibl)? {
                Some(preprocessed) => pending.push((key, preprocessed)),
                None => unchanged += 1,
            }
        }
        // 2D 槽位没有变体轴：feature 表为空，预处理因此必须与内置源逐字节相同（见
        // `every_builtin_shader_is_byte_identical_after_preprocessing_with_no_features`）。
        for slot in FixedSlot::ALL {
            let (active, next) = self.stage_root(proposed, &mut ledger, slot.root())?;
            match self.stage_variant(&active, &next, slot.label(), SkinMode::Off, IblMode::Off)? {
                Some(preprocessed) => pending_slots.push((slot, preprocessed)),
                None => unchanged += 1,
            }
        }
        let validate_ms = started.elapsed().as_secs_f64() * 1000.0;

        // 全部编译：任何一条失败都不提交（旧管线原样保留）。
        let compiled = Instant::now();
        let mut built: Vec<(ShaderVariantKey, Arc<wgpu::RenderPipeline>)> = Vec::new();
        for (key, preprocessed) in pending {
            built.push((key, self.compile_variant(&key, &preprocessed)?));
        }
        let mut built_slots: Vec<BuiltSlots> = Vec::new();
        for (slot, preprocessed) in pending_slots {
            built_slots.push(self.compile_slot(slot, &preprocessed)?);
        }
        let compile_ms = compiled.elapsed().as_secs_f64() * 1000.0;

        // 提交点：替换缓存条目与固定槽位，再换掉生效源。
        let rebuilt = built.len() + built_slots.len();
        for (key, pipeline) in built {
            if !self.pipelines.replace(key, pipeline) {
                return Err(ShaderReloadError::Device {
                    variant: key.label(),
                    message: "变体在提交前已从缓存消失".to_owned(),
                });
            }
        }
        for slot in built_slots {
            slot.commit(self);
        }
        let mut sources: Vec<ShaderSourceStatus> = ledger.roots.into_values().collect();
        sources.sort_by_key(|source| source.name);
        let report = ShaderReloadReport {
            override_dir: proposed.override_dir().map(PathBuf::from),
            rebuilt,
            unchanged,
            variants: variant_count,
            sources,
            fell_back_to_builtin: ledger.fell_back,
            validate_ms,
            compile_ms,
        };
        tracing::info!(target: "render", "[render] shader hot reload: {}", report.summary());
        for name in &report.fell_back_to_builtin {
            tracing::warn!(
                target: "render",
                "[render] shader 覆盖文件已删除，{name} 回退内置源"
            );
        }
        for source in &report.sources {
            tracing::debug!(
                target: "render",
                "[render] shader {} hash={:016x} <- {}",
                source.name,
                source.hash,
                source.origin_text()
            );
        }
        self.shader_library = proposed.clone();
        // 提交成功才更新基准：拒绝/失败的路径上管线还是旧源，基准也必须是旧源。
        for (name, source) in ledger.committed {
            self.shader_committed.insert(name, source);
        }
        self.shader_reload = Some(report.clone());
        Ok(report)
    }

    /// 记账"这个根 shader 本次生效的是哪份源"，并返回（管线里编译着的源, 解析出的新源）。
    ///
    /// 同一根 shader 被多个变体共用，两次解析之间如果覆盖目录被换掉，就会读出两份不同的源；
    /// 带着半套源继续编译是最难复现的失败，所以在这里停住。
    fn stage_root(
        &self,
        proposed: &ShaderLibrary,
        ledger: &mut ReloadLedger,
        root: &'static ShaderSource,
    ) -> Result<(ActiveSource, ActiveSource), ShaderReloadError> {
        let active = self.committed_source(root)?;
        let next = self.resolve_with(proposed, root)?;
        if active.overridden && !next.overridden {
            let name = root.name.to_owned();
            if !ledger.fell_back.contains(&name) {
                ledger.fell_back.push(name);
            }
        }
        let entered = ledger.roots.insert(
            root.name,
            ShaderSourceStatus {
                name: root.name,
                repo_path: root.path,
                overridden: next.overridden,
                override_path: next.override_path.clone(),
                hash: next.hash,
            },
        );
        if let Some(previous) = entered
            && (previous.overridden != next.overridden || previous.hash != next.hash)
        {
            return Err(ShaderReloadError::Override {
                name: root.name.to_owned(),
                reason: "同一 shader 的不同变体解析出不同源".to_owned(),
            });
        }
        ledger.committed.insert(root.name, next.clone());
        Ok((active, next))
    }

    /// 单个变体/槽位的判定：没变化返回 `None`（一次编译都不做）；有变化则先完整校验，
    /// 再过入口点与接口哈希两道门，返回要编进管线的那份预处理源。
    fn stage_variant(
        &self,
        active: &ActiveSource,
        next: &ActiveSource,
        variant: &str,
        skin: SkinMode,
        ibl: IblMode,
    ) -> Result<Option<PreprocessedShader>, ShaderReloadError> {
        if active.hash == next.hash {
            // 源文本逐字相同 ⇒ 这个根的这条变体没有变化（预处理是纯函数）。
            return Ok(None);
        }
        let pre = |source: &ActiveSource| {
            preprocessed_variant(&source.error_path, &source.text, skin, ibl).map_err(|error| {
                ShaderReloadError::Preprocess {
                    variant: variant.to_owned(),
                    message: error.to_string(),
                }
            })
        };
        let active_pre = pre(active)?;
        let next_pre = pre(next)?;
        if active_pre.source() == next_pre.source() {
            // 覆盖文件只改了未被这条变体选中的分支（例如只动 `//#if SKIN` 的另一侧）。
            return Ok(None);
        }
        // 完整校验（语法 + 类型/语义）在 GPU 之前跑一遍，并把生成源的行号映射回作者那份文件。
        let module = validate_module(&next.error_path, next_pre.source())
            .map_err(|error| ShaderReloadError::Validate(locate(error, &next_pre)))?;
        let interface = interface_of(&module);
        require_entry_points(&interface).map_err(ShaderReloadError::Validate)?;
        let active_interface = parse_interface(&active.error_path, active_pre.source())
            .map_err(|error| ShaderReloadError::Validate(locate(error, &active_pre)))?;
        if active_interface.hash() != interface.hash() {
            return Err(ShaderReloadError::Validate(
                ShaderValidateError::InterfaceChanged {
                    variant: variant.to_owned(),
                    diff: active_interface.describe_diff(&interface),
                },
            ));
        }
        Ok(Some(next_pre))
    }

    /// 当前**编译进管线**的源（内置，或上一次成功提交的覆盖）。
    fn committed_source(
        &self,
        root: &'static ShaderSource,
    ) -> Result<ActiveSource, ShaderReloadError> {
        if let Some(active) = self.shader_committed.get(root.name) {
            return Ok(active.clone());
        }
        // 还没提交过：构造期编译的就是注册表里的内置源，与解析层无关。
        self.resolve_with(&ShaderLibrary::builtin(), root)
    }

    fn resolve_with(
        &self,
        library: &ShaderLibrary,
        root: &'static ShaderSource,
    ) -> Result<ActiveSource, ShaderReloadError> {
        let resolved = library.resolve(root.name)?;
        ActiveSource::new(root, resolved)
    }

    /// 编译单个变体（shader module + render pipeline），设备校验错误返回 `Err`。
    ///
    /// 源在这里**已经**预处理并比对过接口，所以建模块不会再触发预处理失败——热重载的任何
    /// 一步失败都只是"这一批不提交"，旧管线继续生效。
    fn compile_variant(
        &self,
        key: &ShaderVariantKey,
        preprocessed: &PreprocessedShader,
    ) -> Result<Arc<wgpu::RenderPipeline>, ShaderReloadError> {
        let variant = key.label();
        let pipeline_label = format!("rustlua-{variant}-pipeline");
        let shader_label = format!("rustlua-{variant}-shader");
        let target_format = match key.target {
            ColorTarget::Ldr => self.config.format,
            ColorTarget::Hdr => wgpu::TextureFormat::Rgba16Float,
        };
        // 校验错误必须变成数据：没有 error scope，一条坏 shader 会把编辑器整个打挂
        // （native 是 panic，wasm 是 abort），而"改了 shader 结果进程没了"是最难查的形态。
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = module_from_preprocessed(&self.device, preprocessed, &shader_label);
        let pipeline = match (key.family, key.shading) {
            (PipelineFamily::Shadow, _) => Arc::new(create_shadow_pipeline(
                &self.device,
                &self.shadow_pipeline_layout,
                &module,
                &pipeline_label,
                key.double_sided,
            )),
            (PipelineFamily::Mesh3d, shading) => {
                let layout = match shading {
                    Shading::Legacy => &self.mesh_pipeline_layout,
                    Shading::Pbr => &self.pbr_pipeline_layout,
                };
                Arc::new(create_mesh_pipeline(
                    &self.device,
                    layout,
                    &module,
                    target_format,
                    &pipeline_label,
                    shading == Shading::Pbr,
                    key.alpha,
                    key.double_sided,
                ))
            }
        };
        if let Some(error) = pollster::block_on(self.device.pop_error_scope()) {
            return Err(ShaderReloadError::Device {
                variant,
                message: error.to_string(),
            });
        }
        Ok(pipeline)
    }

    /// 重建一条 2D 固定槽位的全部管线，设备校验错误同样走 error scope 变成数据。
    ///
    /// 每条管线用的都是构造期那套构建器（`build_*`），布局取自 [`Renderer::fixed_slot_layouts`]，
    /// 格式取自 `config.format`；HDR/SDF 这些"构造期有没有"的分支按当前字段的状态复现，
    /// 重建因此不会凭空多出一条管线，也不会少一条。
    fn compile_slot(
        &self,
        slot: FixedSlot,
        preprocessed: &PreprocessedShader,
    ) -> Result<BuiltSlots, ShaderReloadError> {
        let variant = slot.label().to_owned();
        let format = self.config.format;
        let shader_label = format!("rustlua-{}-shader", slot.label());
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = module_from_preprocessed(&self.device, preprocessed, &shader_label);
        let built = match slot {
            FixedSlot::Textured => {
                let layout = &self.fixed_slot_layouts.textured;
                BuiltSlots::Textured {
                    pipeline: build_textured_pipeline(
                        &self.device,
                        layout,
                        &module,
                        format,
                        "rustlua-textured-pipeline",
                    ),
                    pipeline_sdf: build_textured_pipeline(
                        &self.device,
                        layout,
                        &module,
                        format,
                        "rustlua-textured-sdf-pipeline",
                    ),
                }
            }
            FixedSlot::VectorText => BuiltSlots::VectorText {
                pipeline: build_vector_text_pipeline(
                    &self.device,
                    &self.fixed_slot_layouts.vector_text,
                    &module,
                    format,
                    "rustlua-vector-text-pipeline",
                ),
            },
            FixedSlot::WorldSprite => {
                let layout = &self.fixed_slot_layouts.world_sprite;
                let hdr = self.hdr_world_sprite_pipeline.as_ref().map(|_| {
                    (
                        build_world_sprite_pipeline(
                            &self.device,
                            layout,
                            &module,
                            DepthPass::WorldSprite,
                            wgpu::TextureFormat::Rgba16Float,
                            "rustlua-world-sprite-hdr-pipeline",
                        ),
                        build_world_sprite_pipeline(
                            &self.device,
                            layout,
                            &module,
                            DepthPass::WorldSpriteOpaque,
                            wgpu::TextureFormat::Rgba16Float,
                            "rustlua-world-sprite-opaque-hdr-pipeline",
                        ),
                    )
                });
                BuiltSlots::WorldSprite {
                    translucent: build_world_sprite_pipeline(
                        &self.device,
                        layout,
                        &module,
                        DepthPass::WorldSprite,
                        format,
                        "rustlua-world-sprite-pipeline",
                    ),
                    opaque: build_world_sprite_pipeline(
                        &self.device,
                        layout,
                        &module,
                        DepthPass::WorldSpriteOpaque,
                        format,
                        "rustlua-world-sprite-opaque-pipeline",
                    ),
                    hdr,
                }
            }
            FixedSlot::PostProcess => BuiltSlots::PostProcess {
                pipeline: build_fullscreen_pipeline(
                    &self.device,
                    &self.fixed_slot_layouts.post_process,
                    &module,
                    format,
                    "rustlua-post-process-pipeline",
                ),
            },
            // Tonemap 复用后处理的绑定布局（HDR 纹理 + 采样器 + uniform）。
            FixedSlot::Tonemap => BuiltSlots::Tonemap {
                pipeline: self.tonemap_pipeline.as_ref().map(|_| {
                    build_fullscreen_pipeline(
                        &self.device,
                        &self.fixed_slot_layouts.post_process,
                        &module,
                        format,
                        "rustlua-tonemap-pipeline",
                    )
                }),
            },
        };
        if let Some(error) = pollster::block_on(self.device.pop_error_scope()) {
            return Err(ShaderReloadError::Device {
                variant,
                message: error.to_string(),
            });
        }
        Ok(built)
    }
}

/// 把校验错误里**生成源**的行号换成作者那份文件的行号。
///
/// `//#if` 会删行，所以 naga 报的行号在覆盖文件里通常指到别的地方；不做这一步，Console 里的
/// "文件:行"就是错的，而一条指错行的错误比没有错误更难查。
///
/// 入口点级的校验错误整条源都没有位置（`offset == 0`）：那时**不**补行号——"第 1 行"会把
/// 作者支去查一行本来没问题的代码。
fn locate(error: ShaderValidateError, pre: &PreprocessedShader) -> ShaderValidateError {
    let ShaderValidateError::Validate { message, offset } = error else {
        return error;
    };
    let origin = match offset {
        0 => return ShaderValidateError::Validate { message, offset },
        offset => pre.origin_of(line_of_offset(pre.source(), offset)),
    };
    ShaderValidateError::Validate {
        message: match origin {
            Some(origin) => {
                format!("{message}\n  → {} 第 {} 行", origin.file, origin.line)
            }
            None => message,
        },
        offset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2D 槽位没有变体轴，重载期用**空 feature 表**预处理它们的源。这条断言钉住那个前提：
    /// 对内置源来说预处理必须是恒等，否则"重载后的 2D 管线"画的不是构造期那份程序——
    /// 而构造期根本不过预处理层（直接把注册表里的源交给 `create_shader_module`）。
    #[test]
    fn two_d_builtin_shaders_are_byte_identical_after_preprocessing() {
        for slot in FixedSlot::ALL {
            let root = slot.root();
            let preprocessed =
                preprocessed_variant(root.path, root.source, SkinMode::Off, IblMode::Off)
                    .unwrap_or_else(|error| panic!("{} 预处理失败：{error}", root.name));
            assert_eq!(
                preprocessed.source(),
                root.source,
                "{}：空 feature 表的预处理必须逐字节保持内置源",
                root.name
            );
        }
    }
}
