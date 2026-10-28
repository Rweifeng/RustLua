//! P3-R04-3：固定轴 shader 变体身份与渲染管线缓存。
//!
//! 变体键只允许计划 §6.2 列出的**固定轴**，不做"任意字符串组合"：键是 `Copy + Hash` 的小结构，
//! 拼错的轴在编译期就不存在。渲染循环按 draw 的材质选键（目前是骨骼蒙皮开关 + 目标格式），
//! 构造期把常用组合一次性编译进 [`PipelineCache`]。
//!
//! 缓存有三条硬规矩：
//!
//! - **不静默淘汰**：超过 family/全局上限返回错误，已编译的管线不动。悄悄 evict 会让"这帧为什么
//!   卡了一下"变成无法复现的偶发问题。
//! - **同键只编译一次**：`get_or_create` 命中直接返回；`Arc` 让渲染循环借出管线时不必再分配。
//! - **统计可查**：命中/未命中/编译耗时/变体数由构造期请求累计，Profiler 与验收命令读同一份数。

use std::{collections::HashMap, fmt, hash::Hash, time::Duration};

// wasm32-unknown-unknown 的 std `Instant::now()` 直接 panic；`web_time` 在原生上是同一类型。
use web_time::Instant;

use rustlua_render::mesh3d::MaterialAlphaMode;
use rustlua_render::render_settings::IblMode;

/// 单个 family（网格 / 阴影）的变体上限。
pub const FAMILY_VARIANT_LIMIT: usize = 32;
/// 全部 family 合计的变体上限。
pub const GLOBAL_VARIANT_LIMIT: usize = 64;
/// 启动期变体预编译预算（毫秒）。超出只告警、不取消已经编译好的变体——预算是诊断信号，
/// 不是把管线建到一半就扔掉的开关。
pub const VARIANT_COMPILE_BUDGET_MS: f64 = 250.0;

/// 骨骼蒙皮变体轴。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SkinMode {
    Off,
    On,
}

impl SkinMode {
    /// 传给预处理器的 feature 列表；`Off` 是空集。
    #[must_use]
    pub const fn features(self) -> &'static [&'static str] {
        match self {
            Self::Off => &[],
            Self::On => &["SKIN"],
        }
    }
}

impl From<bool> for SkinMode {
    fn from(skinned: bool) -> Self {
        if skinned { Self::On } else { Self::Off }
    }
}

/// 逐实例模型矩阵的**取法**（P3-R06 / WebGL2 补片）：有存储缓冲的设备走 storage 数组，
/// 没有的（WebGL2 `max_storage_buffers_per_shader_stage=0`）走 64 字节 uniform。
///
/// 这不是变体键的一根轴：同一台设备上它恒定，同一份缓存里不会同时出现两种；它只决定
/// 预处理器给不给 `STORAGE_INSTANCES`，以及材质布局 binding 2 的缓冲类型。
/// 两条路画的**是同一批 draw**——无 storage 时实例化合批已关（每条 draw 一个模型）。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InstanceMode {
    /// `var<storage, read> instance_models: array<mat4x4<f32>>`，支持实例化合批。
    Storage,
    /// `var<uniform> instance_model: mat4x4<f32>`，每条 draw 一次绑定。
    Uniform,
}

impl InstanceMode {
    /// 传给预处理器的 feature 列表；`Uniform` 是空集（历史分支）。
    #[must_use]
    pub const fn features(self) -> &'static [&'static str] {
        match self {
            Self::Storage => &["STORAGE_INSTANCES"],
            Self::Uniform => &[],
        }
    }

    /// 设备能力（`max_storage_buffers_per_shader_stage > 0`）→ 取法。
    #[must_use]
    pub const fn from_storage_capable(storage_capable: bool) -> Self {
        if storage_capable {
            Self::Storage
        } else {
            Self::Uniform
        }
    }

    #[must_use]
    pub const fn uses_storage(self) -> bool {
        matches!(self, Self::Storage)
    }
}

/// 颜色目标轴。阴影不写颜色，构造键时固定为 [`ColorTarget::Ldr`]。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ColorTarget {
    Ldr,
    Hdr,
}

/// 着色模型轴（P3-R02）：`RenderSettings.pbr=off` 走旧管线，开启后走 Cook-Torrance。
///
/// 这个轴存在的唯一理由是**回滚档必须逐字节不变**：PBR 不是"改一份 shader"，而是同一份
/// 网格/阴影几何上的第二套片元着色。阴影管线与着色模型无关，[`ShaderVariantKey::shadow`]
/// 因此固定 `Legacy`，不会为 PBR 多编译一份阴影变体。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Shading {
    Legacy,
    Pbr,
}

impl Shading {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Pbr => "pbr",
        }
    }
}

/// 管线家族：上限按家族分别计算。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PipelineFamily {
    Mesh3d,
    Shadow,
}

impl PipelineFamily {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mesh3d => "mesh3d",
            Self::Shadow => "shadow",
        }
    }
}

/// 一条渲染管线的变体身份。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ShaderVariantKey {
    pub family: PipelineFamily,
    pub shading: Shading,
    pub skin: SkinMode,
    pub target: ColorTarget,
    /// P3-R02-4：alpha 模式决定 discard / 混合状态；Legacy 恒为 Opaque。
    pub alpha: MaterialAlphaMode,
    /// 双面材质关掉背面剔除。
    pub double_sided: bool,
    /// P3-R03：环境光照档位。Legacy 着色与阴影恒为 [`IblMode::Off`]。
    pub ibl: IblMode,
}

impl ShaderVariantKey {
    #[must_use]
    pub const fn mesh(shading: Shading, skin: SkinMode, target: ColorTarget) -> Self {
        Self {
            family: PipelineFamily::Mesh3d,
            shading,
            skin,
            target,
            alpha: MaterialAlphaMode::Opaque,
            double_sided: false,
            ibl: IblMode::Off,
        }
    }

    /// PBR 网格的 alpha / 双面变体；Legacy 着色忽略这两个轴（回滚档像素不变）。
    #[must_use]
    pub const fn mesh_with_alpha(
        shading: Shading,
        skin: SkinMode,
        target: ColorTarget,
        alpha: MaterialAlphaMode,
        double_sided: bool,
    ) -> Self {
        let (alpha, double_sided) = if matches!(shading, Shading::Legacy) {
            (MaterialAlphaMode::Opaque, false)
        } else {
            (alpha, double_sided)
        };
        Self {
            family: PipelineFamily::Mesh3d,
            shading,
            skin,
            target,
            alpha,
            double_sided,
            ibl: IblMode::Off,
        }
    }

    /// P3-R03：把 IBL 档位并入键。Legacy 与阴影忽略该轴（Off 的着色器与 R02 逐字节相同）。
    #[must_use]
    pub const fn with_ibl(mut self, ibl: IblMode) -> Self {
        self.ibl = if matches!(self.shading, Shading::Pbr)
            && matches!(self.family, PipelineFamily::Mesh3d)
        {
            ibl
        } else {
            IblMode::Off
        };
        self
    }

    /// 阴影管线没有颜色目标，`target` 恒为 LDR（避免制造同一条管线的两个键）；
    /// 它也不看着色模型，`shading` 恒为 Legacy（不为 PBR 重复编译阴影变体）。
    #[must_use]
    pub const fn shadow(skin: SkinMode) -> Self {
        Self {
            family: PipelineFamily::Shadow,
            shading: Shading::Legacy,
            skin,
            target: ColorTarget::Ldr,
            alpha: MaterialAlphaMode::Opaque,
            double_sided: false,
            ibl: IblMode::Off,
        }
    }

    /// 阴影支持 opaque/mask 与双面；blend 不进阴影通道（调用方负责过滤）。
    #[must_use]
    pub const fn shadow_with_alpha(
        skin: SkinMode,
        alpha: MaterialAlphaMode,
        double_sided: bool,
    ) -> Self {
        Self {
            family: PipelineFamily::Shadow,
            shading: Shading::Legacy,
            skin,
            target: ColorTarget::Ldr,
            alpha,
            double_sided,
            ibl: IblMode::Off,
        }
    }

    /// 日志/Profiler 用的稳定短名，例如 `mesh3d-pbr-skin-hdr`。
    ///
    /// Legacy 网格与阴影不带着色模型前缀，保持 R04 已有的日志/测试口径不变；
    /// 非默认的 alpha/双面组合追加后缀。
    #[must_use]
    pub fn label(self) -> String {
        let skin = match self.skin {
            SkinMode::Off => "static",
            SkinMode::On => "skin",
        };
        let target = match self.target {
            ColorTarget::Ldr => "ldr",
            ColorTarget::Hdr => "hdr",
        };
        let mut label = match self.shading {
            Shading::Legacy => format!("{}-{skin}-{target}", self.family.as_str()),
            Shading::Pbr => format!("{}-pbr-{skin}-{target}", self.family.as_str()),
        };
        match self.alpha {
            MaterialAlphaMode::Opaque => {}
            MaterialAlphaMode::Mask => label.push_str("-mask"),
            MaterialAlphaMode::Blend => label.push_str("-blend"),
        }
        if self.double_sided {
            label.push_str("-double");
        }
        match self.ibl {
            IblMode::Off => {}
            IblMode::Diffuse => label.push_str("-ibl-sh"),
            IblMode::Specular => label.push_str("-ibl-spec"),
            IblMode::Full => label.push_str("-ibl-full"),
        }
        label
    }
}

/// 缓存统计（构造期累计）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PipelineCacheStats {
    /// 请求的变体已经存在。
    pub hits: u64,
    /// 请求的变体需要编译（无论成功与否）。
    pub misses: u64,
    /// 实际花在创建调用里的总时长。
    pub compile_time: Duration,
    /// 当前缓存的变体数。
    pub variants: usize,
}

impl PipelineCacheStats {
    #[must_use]
    pub fn compile_ms(&self) -> f64 {
        self.compile_time.as_secs_f64() * 1000.0
    }

    /// 启动预编译是否在 [`VARIANT_COMPILE_BUDGET_MS`] 内。
    #[must_use]
    pub fn within_budget(&self) -> bool {
        self.compile_ms() <= VARIANT_COMPILE_BUDGET_MS
    }
}

/// 超限/创建失败。超限时缓存保持原样（不淘汰）。
#[derive(Debug, PartialEq, Eq)]
pub enum PipelineCacheError<E> {
    /// 新变体会突破 family 或全局上限。
    LimitExceeded {
        family: PipelineFamily,
        /// 新变体所属 family 的现有数量。
        family_count: usize,
        family_limit: usize,
        global_count: usize,
        global_limit: usize,
    },
    /// 创建回调失败（例如设备校验错误）。
    Create(E),
}

impl<E: fmt::Display> fmt::Display for PipelineCacheError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LimitExceeded {
                family,
                family_count,
                family_limit,
                global_count,
                global_limit,
            } => write!(
                f,
                "shader 变体超限：family={} {family_count}/{family_limit}，全局 {global_count}/{global_limit}；缓存不会淘汰旧变体",
                family.as_str()
            ),
            Self::Create(error) => write!(f, "创建管线失败：{error}"),
        }
    }
}

impl<E: fmt::Display + fmt::Debug> std::error::Error for PipelineCacheError<E> {}

/// 按 [`ShaderVariantKey`] 缓存的渲染管线。
///
/// `K` 固定为变体键，`V` 由调用方决定（渲染器用 `Arc<wgpu::RenderPipeline>`，单测用 `()`），
/// 因此上限/命中统计可以在不接触 GPU 的情况下验证。
pub struct PipelineCache<V> {
    entries: HashMap<ShaderVariantKey, V>,
    family_limit: usize,
    global_limit: usize,
    hits: u64,
    misses: u64,
    compile_time: Duration,
}

impl<V> Default for PipelineCache<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V> PipelineCache<V> {
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(FAMILY_VARIANT_LIMIT, GLOBAL_VARIANT_LIMIT)
    }

    /// 测试/特殊设备用：显式给 family 与全局上限。
    #[must_use]
    pub fn with_limits(family_limit: usize, global_limit: usize) -> Self {
        Self {
            entries: HashMap::new(),
            family_limit,
            global_limit,
            hits: 0,
            misses: 0,
            compile_time: Duration::ZERO,
        }
    }

    /// 纯查找（不改变统计）。渲染循环每帧按 draw 选管线走这里。
    #[must_use]
    pub fn get(&self, key: &ShaderVariantKey) -> Option<&V> {
        self.entries.get(key)
    }

    /// 已编译变体数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn stats(&self) -> PipelineCacheStats {
        PipelineCacheStats {
            hits: self.hits,
            misses: self.misses,
            compile_time: self.compile_time,
            variants: self.entries.len(),
        }
    }

    /// 取已有变体，否则调用 `create` 编译并缓存。
    ///
    /// 先检查上限再调用 `create`：超限时不会白编译一条随后被丢弃的管线。
    pub fn get_or_create<E>(
        &mut self,
        key: ShaderVariantKey,
        create: impl FnOnce() -> Result<V, E>,
    ) -> Result<&V, PipelineCacheError<E>> {
        if self.entries.contains_key(&key) {
            self.hits += 1;
            return Ok(self.entries.get(&key).expect("contains_key 已确认存在"));
        }
        self.misses += 1;
        let family_count = self
            .entries
            .keys()
            .filter(|existing| existing.family == key.family)
            .count();
        let global_count = self.entries.len();
        if family_count >= self.family_limit || global_count >= self.global_limit {
            return Err(PipelineCacheError::LimitExceeded {
                family: key.family,
                family_count,
                family_limit: self.family_limit,
                global_count,
                global_limit: self.global_limit,
            });
        }
        let started = Instant::now();
        let value = create().map_err(PipelineCacheError::Create)?;
        self.compile_time += started.elapsed();
        let inserted = self.entries.insert(key, value);
        debug_assert!(inserted.is_none(), "命中路径已排除重复键");
        Ok(self.entries.get(&key).expect("刚刚插入的变体必须存在"))
    }

    /// 已缓存变体的键，按 `label()` 升序（顺序稳定，日志与断言都可复现）。
    ///
    /// P3-R05-3 需要它：热重载要重建的是"**当前实际存在**的变体集合"，而不是理论上的全轴组合
    /// （后者会编译一批从未用过的管线，还会撞上限）。
    #[must_use]
    pub fn keys(&self) -> Vec<ShaderVariantKey> {
        let mut keys: Vec<ShaderVariantKey> = self.entries.keys().copied().collect();
        keys.sort_by_key(|key| key.label());
        keys
    }

    /// 用新管线替换同一个键（热重载的提交点）。
    ///
    /// 只有键**已经存在**时才替换并返回 `true`：替换不是新增，绝不能绕过上限、也不能把
    /// 一个不存在的变体悄悄建出来。旧值随返回的 `Option` 被 drop（GPU 对象引用计数归零后
    /// 由 wgpu 回收；正在引用的帧命令早已提交，不受影响）。
    pub fn replace(&mut self, key: ShaderVariantKey, value: V) -> bool {
        if !self.entries.contains_key(&key) {
            return false;
        }
        self.entries.insert(key, value);
        true
    }
}

impl<V> fmt::Debug for PipelineCache<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PipelineCache")
            .field("entries", &self.entries.len())
            .field("family_limit", &self.family_limit)
            .field("global_limit", &self.global_limit)
            .field("hits", &self.hits)
            .field("misses", &self.misses)
            .field("compile_time", &self.compile_time)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mesh(skin: SkinMode, target: ColorTarget) -> ShaderVariantKey {
        ShaderVariantKey::mesh(Shading::Legacy, skin, target)
    }

    /// 固定轴的每一种组合都是不同的键；阴影的 target 不制造重复。
    #[test]
    fn keys_are_unique_per_axis_combination() {
        let keys = [
            mesh(SkinMode::Off, ColorTarget::Ldr),
            mesh(SkinMode::On, ColorTarget::Ldr),
            mesh(SkinMode::Off, ColorTarget::Hdr),
            mesh(SkinMode::On, ColorTarget::Hdr),
            ShaderVariantKey::mesh(Shading::Pbr, SkinMode::Off, ColorTarget::Ldr),
            ShaderVariantKey::mesh(Shading::Pbr, SkinMode::On, ColorTarget::Hdr),
            ShaderVariantKey::shadow(SkinMode::Off),
            ShaderVariantKey::shadow(SkinMode::On),
        ];
        for (index, key) in keys.iter().enumerate() {
            for other in &keys[index + 1..] {
                assert_ne!(key, other);
            }
        }
        // 阴影无论传什么目标都归一到 LDR，且不带着色模型轴。
        assert_eq!(
            ShaderVariantKey::shadow(SkinMode::On),
            ShaderVariantKey {
                family: PipelineFamily::Shadow,
                shading: Shading::Legacy,
                skin: SkinMode::On,
                target: ColorTarget::Ldr,
                alpha: MaterialAlphaMode::Opaque,
                double_sided: false,
                ibl: IblMode::Off,
            }
        );
        assert_eq!(
            mesh(SkinMode::On, ColorTarget::Hdr).label(),
            "mesh3d-skin-hdr"
        );
        assert_eq!(
            ShaderVariantKey::mesh(Shading::Pbr, SkinMode::On, ColorTarget::Hdr).label(),
            "mesh3d-pbr-skin-hdr"
        );
        assert_eq!(SkinMode::On.features(), &["SKIN"]);
        assert!(SkinMode::Off.features().is_empty());
    }

    /// R02-4：alpha 与双面是真正的变体轴；Legacy 把两轴归零（回滚档不多编译、像素不变）。
    #[test]
    fn alpha_and_double_sided_are_variant_axes() {
        let base = ShaderVariantKey::mesh(Shading::Pbr, SkinMode::Off, ColorTarget::Ldr);
        let mask = ShaderVariantKey::mesh_with_alpha(
            Shading::Pbr,
            SkinMode::Off,
            ColorTarget::Ldr,
            MaterialAlphaMode::Mask,
            false,
        );
        let blend_double = ShaderVariantKey::mesh_with_alpha(
            Shading::Pbr,
            SkinMode::Off,
            ColorTarget::Ldr,
            MaterialAlphaMode::Blend,
            true,
        );
        assert_ne!(base, mask);
        assert_ne!(mask, blend_double);
        assert_eq!(mask.label(), "mesh3d-pbr-static-ldr-mask");
        assert_eq!(blend_double.label(), "mesh3d-pbr-static-ldr-blend-double");
        assert_eq!(
            ShaderVariantKey::mesh_with_alpha(
                Shading::Legacy,
                SkinMode::Off,
                ColorTarget::Ldr,
                MaterialAlphaMode::Blend,
                true,
            ),
            ShaderVariantKey::mesh(Shading::Legacy, SkinMode::Off, ColorTarget::Ldr),
            "Legacy 忽略 alpha/双面轴"
        );
        // 阴影没有颜色目标，但 alpha/双面仍然区分管线（mask 剔除 / 双面剔除状态）。
        assert_eq!(
            ShaderVariantKey::shadow_with_alpha(SkinMode::Off, MaterialAlphaMode::Mask, false)
                .label(),
            "shadow-static-ldr-mask"
        );
        assert_ne!(
            ShaderVariantKey::shadow(SkinMode::Off),
            ShaderVariantKey::shadow_with_alpha(SkinMode::Off, MaterialAlphaMode::Mask, false)
        );
    }

    /// R03：IBL 档位是 PBR 网格的变体轴；Legacy 与阴影把该轴归零，Off 与 R02 键完全相同。
    #[test]
    fn ibl_is_a_pbr_mesh_variant_axis() {
        let base = ShaderVariantKey::mesh(Shading::Pbr, SkinMode::Off, ColorTarget::Hdr);
        assert_eq!(base.ibl, IblMode::Off);
        let diffuse = base.with_ibl(IblMode::Diffuse);
        let full = base.with_ibl(IblMode::Full);
        assert_ne!(base, diffuse);
        assert_ne!(diffuse, full);
        assert_eq!(diffuse.ibl, IblMode::Diffuse);
        assert_eq!(full.label(), "mesh3d-pbr-static-hdr-ibl-full");
        assert_eq!(
            ShaderVariantKey::shadow(SkinMode::On).with_ibl(IblMode::Full),
            ShaderVariantKey::shadow(SkinMode::On),
            "阴影不看着色模型与环境轴"
        );
        assert_eq!(
            ShaderVariantKey::mesh(Shading::Legacy, SkinMode::Off, ColorTarget::Ldr)
                .with_ibl(IblMode::Diffuse),
            ShaderVariantKey::mesh(Shading::Legacy, SkinMode::Off, ColorTarget::Ldr),
            "Legacy 忽略 IBL 轴，回滚档不多编译"
        );
        assert_eq!(
            ShaderVariantKey::mesh(Shading::Pbr, SkinMode::On, ColorTarget::Ldr)
                .with_ibl(IblMode::Off)
                .label(),
            "mesh3d-pbr-skin-ldr"
        );
    }

    /// 同一个键请求两次只创建一次：命中计数增加、`create` 不执行第二次。
    #[test]
    fn duplicate_request_hits_the_cache() {
        let mut cache: PipelineCache<u32> = PipelineCache::new();
        let key = mesh(SkinMode::On, ColorTarget::Ldr);
        assert_eq!(*cache.get_or_create(key, || Ok::<_, ()>(7)).unwrap(), 7);
        assert_eq!(*cache.get_or_create(key, || Ok::<_, ()>(99)).unwrap(), 7);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.stats().hits, 1);
        assert_eq!(cache.stats().misses, 1);
        assert_eq!(cache.get(&key), Some(&7));
        assert_eq!(cache.get(&ShaderVariantKey::shadow(SkinMode::Off)), None);
    }

    /// family 上限：同 family 的第 N+1 个变体报错且不淘汰旧变体；另一个 family 不受影响。
    #[test]
    fn family_limit_rejects_without_eviction() {
        let mut cache: PipelineCache<u32> = PipelineCache::with_limits(2, 64);
        cache
            .get_or_create(mesh(SkinMode::Off, ColorTarget::Ldr), || Ok::<_, ()>(1))
            .unwrap();
        cache
            .get_or_create(mesh(SkinMode::On, ColorTarget::Ldr), || Ok::<_, ()>(2))
            .unwrap();
        let error = cache
            .get_or_create(mesh(SkinMode::Off, ColorTarget::Hdr), || Ok::<_, ()>(3))
            .unwrap_err();
        assert!(matches!(
            error,
            PipelineCacheError::LimitExceeded {
                family: PipelineFamily::Mesh3d,
                family_count: 2,
                family_limit: 2,
                ..
            }
        ));
        assert_eq!(cache.len(), 2, "超限不淘汰已有变体");
        assert_eq!(cache.get(&mesh(SkinMode::Off, ColorTarget::Ldr)), Some(&1));
        // 阴影 family 的计数独立。
        cache
            .get_or_create(ShaderVariantKey::shadow(SkinMode::Off), || Ok::<_, ()>(4))
            .unwrap();
        assert_eq!(cache.len(), 3);
    }

    /// 全局上限跨 family 生效；失败请求不增加变体数。
    #[test]
    fn global_limit_spans_families() {
        let mut cache: PipelineCache<u32> = PipelineCache::with_limits(8, 2);
        cache
            .get_or_create(mesh(SkinMode::Off, ColorTarget::Ldr), || Ok::<_, ()>(1))
            .unwrap();
        cache
            .get_or_create(ShaderVariantKey::shadow(SkinMode::Off), || Ok::<_, ()>(2))
            .unwrap();
        let error = cache
            .get_or_create(mesh(SkinMode::On, ColorTarget::Ldr), || Ok::<_, ()>(3))
            .unwrap_err();
        assert!(matches!(
            error,
            PipelineCacheError::LimitExceeded {
                global_count: 2,
                global_limit: 2,
                ..
            }
        ));
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.stats().misses, 3);
    }

    /// 创建失败不缓存任何东西，之后可以重试。
    #[test]
    fn failed_creation_can_be_retried() {
        let mut cache: PipelineCache<u32> = PipelineCache::new();
        let key = mesh(SkinMode::Off, ColorTarget::Hdr);
        let error = cache
            .get_or_create(key, || Err::<u32, _>("device lost"))
            .unwrap_err();
        assert_eq!(error, PipelineCacheError::Create("device lost"));
        assert!(cache.is_empty());
        assert_eq!(*cache.get_or_create(key, || Ok::<_, &str>(5)).unwrap(), 5);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.stats().misses, 2);
        assert_eq!(cache.stats().hits, 0);
    }

    /// 编译预算只看累计耗时，边界是“≤ 预算”。
    #[test]
    fn compile_budget_is_inclusive() {
        let within = PipelineCacheStats {
            compile_time: Duration::from_millis(250),
            ..Default::default()
        };
        assert!(within.within_budget());
        let over = PipelineCacheStats {
            compile_time: Duration::from_millis(251),
            ..Default::default()
        };
        assert!(!over.within_budget());
        assert!((VARIANT_COMPILE_BUDGET_MS - 250.0).abs() < f64::EPSILON);
    }
}
