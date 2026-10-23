//! P3-R01：工程/场景级的渲染能力开关（单一配置来源）。
//!
//! 为什么需要它：HDR 只是 P3 的第一项，后面的 PBR（R02）、IBL（R03）、shader 变体（R04）
//! 都要各自决定"这台设备上开不开、这个场景里开不开"。如果每个包自己发明一套配置，很快就会出现
//! "渲染器说 HDR 开了、材质层还按 LDR 算"的分叉。这里把所有开关收进一个只读值：
//!
//! - [`ToggleMode`] 表达 `auto | off | on` 三态；`auto` 的解析权在渲染器（能力探针 + 本帧内容），
//!   不在配置里假装知道设备能做什么。
//! - 默认值**全部保持今天的输出**（HDR/IBL/变体关、PBR 关）。这是 P3 计划 §4-3 的硬约束：
//!   2D 项目、旧 3D 场景、没有声明 `RenderSettings` 的场景，像素必须与收敛前一致。
//! - `pbr`/`ibl`/`variants` 现在只是被携带，消费它们的是后续 R02/R03/R04；先定字段是为了让
//!   后续包只往这里加行为，不再新增第二套配置。

/// 三态开关：`Auto` 由渲染器按能力与实际内容解析，`Off`/`On` 是宿主的明确意图。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ToggleMode {
    /// 能力允许且本帧有对应内容时才启用。
    Auto,
    /// 关闭（回滚档）。
    #[default]
    Off,
    /// 明确要求启用；能力不足时渲染器必须给出可读原因并回退，不 panic。
    On,
}

impl ToggleMode {
    /// 宿主是否**明确**要求启用（`On`）。`Auto` 不算明确要求。
    #[must_use]
    pub const fn is_on(self) -> bool {
        matches!(self, Self::On)
    }

    /// 宿主是否允许启用（`Auto` 或 `On`）。
    #[must_use]
    pub const fn is_enabled_by_policy(self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// 工程/场景级渲染开关。字段只描述**用户意图**，设备能力由 [`crate::render_settings`] 的
/// 使用方（渲染器能力快照）单独判断，两者不能混成一个布尔值。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderSettings {
    /// HDR 场景颜色 + Tonemap。默认关闭（保持今天的输出）。
    pub hdr: ToggleMode,
    /// PBR 材质（P3-R02）。默认关闭。
    pub pbr: bool,
    /// 基于图像的光照（P3-R03）。默认关闭。
    pub ibl: ToggleMode,
    /// shader 变体缓存（P3-R04）。默认关闭。
    pub variants: bool,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            hdr: ToggleMode::Off,
            pbr: false,
            ibl: ToggleMode::Off,
            variants: false,
        }
    }
}

impl RenderSettings {
    /// 全关：与未声明 `RenderSettings` 的行为相同。
    pub const DEFAULT: Self = Self {
        hdr: ToggleMode::Off,
        pbr: false,
        ibl: ToggleMode::Off,
        variants: false,
    };

    /// 只改 HDR 开关的构造器（golden/测试最常用）。
    #[must_use]
    pub const fn with_hdr(mut self, hdr: ToggleMode) -> Self {
        self.hdr = hdr;
        self
    }

    /// 只改 PBR 开关的构造器。PBR 没有"自动"档：材质本身没有足够的运行期信息判断
    /// 场景是否"想要"PBR，因此由宿主显式打开。
    #[must_use]
    pub const fn with_pbr(mut self, pbr: bool) -> Self {
        self.pbr = pbr;
        self
    }

    /// 本帧是否允许走 HDR 路径（`Auto`/`On`；实际是否启用还要看能力与内容）。
    #[must_use]
    pub const fn hdr_allowed(self) -> bool {
        self.hdr.is_enabled_by_policy()
    }
}

/// 解析本帧是否**真的**走 HDR 场景颜色（纯函数，便于在没有 GPU 的单测里钉死策略）。
///
/// 四个条件缺一不可：
/// 1. 宿主策略允许（`Auto`/`On`）；
/// 2. 设备能力完整（`Rgba16Float` 可渲染/读回 + 可线性过滤）；
/// 3. 本帧确有 3D / 世界内容——纯 2D 帧**不创建** HDR 目标，旧 golden 因此逐字节不变；
/// 4. 打开的后处理没有自带曝光（`exposure == 1.0`），否则同一份曝光会被乘两次。
///
/// 条件 4 不满足时这里返回 `false`（回退到 LDR 旧路径），调用方负责给出告警——静默降级是
/// 本仓明确禁止的失败类型。
#[must_use]
pub fn hdr_activation(
    settings: RenderSettings,
    capability: bool,
    scene_content: bool,
    post_exposure: Option<f32>,
) -> bool {
    settings.hdr_allowed()
        && capability
        && scene_content
        && post_exposure.is_none_or(|exposure| exposure == 1.0)
}

/// 本帧实际使用的 IBL 档位（P3-R03）。它既是 `RenderSettings.ibl` 策略与设备能力的解析结果，
/// 也是 `ShaderVariantKey.ibl` 的取值——Off 的着色器与 R02 逐字节相同。
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum IblMode {
    /// 不采样环境：保留 R02 的常数环境光。
    #[default]
    Off,
    /// 只做 SH9 漫反射（所有设备可用，也是 Tier C 的回退档）。
    Diffuse,
    /// 只做预过滤 cubemap 镜面（浮点立方体贴图不可用时的显式档，不静默降级）。
    Specular,
    /// 漫反射 + 镜面（Tier A/B 的完整档）。
    Full,
}

impl IblMode {
    /// 传给 shader 预处理器的 feature；`Off` 是空集。
    #[must_use]
    pub const fn features(self) -> &'static [&'static str] {
        match self {
            Self::Off => &[],
            Self::Diffuse => &["IBL_DIFFUSE"],
            Self::Specular => &["IBL_SPECULAR"],
            Self::Full => &["IBL_DIFFUSE", "IBL_SPECULAR"],
        }
    }

    /// 写进环境 uniform `params.z` 的稳定编码（0/1/2/3，按位 1=漫反射、2=镜面）。
    #[must_use]
    pub const fn code(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::Diffuse => 1,
            Self::Specular => 2,
            Self::Full => 3,
        }
    }

    #[must_use]
    pub const fn has_diffuse(self) -> bool {
        matches!(self, Self::Diffuse | Self::Full)
    }

    #[must_use]
    pub const fn has_specular(self) -> bool {
        matches!(self, Self::Specular | Self::Full)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Diffuse => "diffuse",
            Self::Specular => "specular",
            Self::Full => "full",
        }
    }
}

/// `Environment` 场景字段的运行时形态（P3-R03-4 由组件/场景解析后写入）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvironmentSettings {
    /// 环境辐射倍率；0 表示环境项不贡献（但仍占用绑定，避免切管线）。
    pub intensity: f32,
    /// 绕 Y 轴旋转（弧度）；只影响环境采样方向，不影响直接光。
    pub rotation: f32,
}

impl Default for EnvironmentSettings {
    fn default() -> Self {
        Self {
            intensity: 1.0,
            rotation: 0.0,
        }
    }
}

/// SH9 系数个数（每个系数 RGB 三个通道）。
pub const ENVIRONMENT_SH_COEFFICIENTS: usize = 27;

/// 环境光照的运行时形态：烘焙好的 SH9 + 用户参数（P3-R03-2 起渲染器消费）。
///
/// SH 系数已经按 `A_l/π` 预卷积（见 `rustlua_assets::env_asset`），因此 `sh_irradiance`
/// 的结果就是"E/π"，着色器直接乘反照率即可，不再额外除 π。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvironmentLighting {
    pub sh: [f32; ENVIRONMENT_SH_COEFFICIENTS],
    pub settings: EnvironmentSettings,
}

impl Default for EnvironmentLighting {
    fn default() -> Self {
        Self {
            sh: [0.0; ENVIRONMENT_SH_COEFFICIENTS],
            settings: EnvironmentSettings::default(),
        }
    }
}

impl EnvironmentLighting {
    /// 只有 SH 的构造器（参数取默认 intensity=1、rotation=0）。
    #[must_use]
    pub const fn from_sh(sh: [f32; ENVIRONMENT_SH_COEFFICIENTS]) -> Self {
        Self {
            sh,
            settings: EnvironmentSettings {
                intensity: 1.0,
                rotation: 0.0,
            },
        }
    }

    /// 环境 uniform 的 CPU 侧布局（40 个 f32 = 160 字节）：
    /// 9×`vec4`（rgb=SH 系数，a=0）+ `params`（intensity, rotation, mode code, 0）。
    ///
    /// 数组里多出的 a 通道是为了让 WGSL 侧能用 `array<vec4<f32>, 9>` 直接寻址——
    /// 每个系数独占一个 16 字节槽，不存在跨槽读取。
    #[must_use]
    pub fn uniform(&self, mode: IblMode) -> [f32; 40] {
        let mut data = [0.0_f32; 40];
        for (index, coefficient) in self.sh.chunks_exact(3).enumerate() {
            data[index * 4..index * 4 + 3].copy_from_slice(coefficient);
        }
        data[36] = self.settings.intensity;
        data[37] = self.settings.rotation;
        data[38] = mode.code() as f32;
        data
    }
}

/// P3-R03-4：环境采样的设备档位（对外报告与回退原因的唯一分类）。
///
/// - `A`：浮点 cubemap 可用 + 可线性过滤 = 预过滤 mip 直接三线性采样（完整质量）；
/// - `B`：浮点 cubemap 可用但只能最近邻 = 仍做镜面，但 mip 间不插值；
/// - `C`：浮点 cubemap 不可用 = 只做 SH9 漫反射；镜面关闭并报告原因。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IblTier {
    A,
    B,
    C,
}

impl IblTier {
    /// 稳定短名（日志/编辑器读数）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
        }
    }

    /// 是否支持预过滤 cubemap 镜面（C 档只做 SH）。
    #[must_use]
    pub const fn supports_specular(self) -> bool {
        !matches!(self, Self::C)
    }
}

/// 按设备能力解析环境档位（纯函数，编辑器/渲染器/测试共用一份）。
#[must_use]
pub const fn ibl_tier(hdr_color: bool, float_linear_filter: bool) -> IblTier {
    if !hdr_color {
        IblTier::C
    } else if float_linear_filter {
        IblTier::A
    } else {
        IblTier::B
    }
}

/// 解析本帧 IBL 档位（纯函数）。
///
/// - 没有环境或策略为 `Off` → `Off`（`pbr=off` 时调用方根本不走 PBR，因此也不会到这里）；
/// - 有环境且镜面 IBL 可用 → `Full`；
/// - 其余 → `Diffuse`（SH-only，Tier C 也能跑）。
///
/// `Auto` 与 `On` 的区别只在"没有环境"时体现：两者都不会凭空造环境；`On` 但设备不支持镜面时
/// 回退到 `Diffuse` 并由调用方报告原因，不静默回到 `Off`。
#[must_use]
pub fn resolve_ibl(
    settings: RenderSettings,
    has_environment: bool,
    specular_available: bool,
) -> IblMode {
    if !has_environment || !settings.ibl.is_enabled_by_policy() {
        return IblMode::Off;
    }
    if specular_available {
        IblMode::Full
    } else {
        IblMode::Diffuse
    }
}

/// 某一帧**实际使用**的场景颜色格式（指标与诊断用）。
///
/// 与 `wgpu::TextureFormat` 分开定义，是因为这个类型要进 [`crate::render_settings`] 的
/// 公开指标，不应把 GPU API 类型泄露到数据模型里；`as_str` 与 wgpu 的格式名对齐。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SceneColorFormat {
    /// 8 位 sRGB（今天的默认路径，也是回滚档）。
    #[default]
    Rgba8UnormSrgb,
    /// 8 位线性（保留给无法渲染浮点但需要线性中间色的后端；R01 不启用）。
    Rgba8Unorm,
    /// 16 位浮点线性（HDR 场景颜色）。
    Rgba16Float,
}

impl SceneColorFormat {
    /// 稳定短名（日志/Profiler）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rgba8UnormSrgb => "rgba8unorm-srgb",
            Self::Rgba8Unorm => "rgba8unorm",
            Self::Rgba16Float => "rgba16float",
        }
    }

    /// 是否是 HDR（浮点）场景颜色。
    #[must_use]
    pub const fn is_hdr(self) -> bool {
        matches!(self, Self::Rgba16Float)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §4-3：未声明设置时必须是旧路径。任何一项默认值被"顺手"改成启用，这条先红。
    #[test]
    fn defaults_keep_todays_output() {
        let settings = RenderSettings::default();
        assert_eq!(settings, RenderSettings::DEFAULT);
        assert_eq!(settings.hdr, ToggleMode::Off);
        assert!(!settings.hdr_allowed());
        assert!(!settings.pbr);
        assert_eq!(settings.ibl, ToggleMode::Off);
        assert!(!settings.variants);
    }

    #[test]
    fn toggle_modes_are_distinct() {
        assert!(ToggleMode::On.is_on());
        assert!(!ToggleMode::Auto.is_on());
        assert!(ToggleMode::Auto.is_enabled_by_policy());
        assert!(ToggleMode::On.is_enabled_by_policy());
        assert!(!ToggleMode::Off.is_enabled_by_policy());
    }

    #[test]
    fn scene_color_names_are_stable() {
        assert_eq!(SceneColorFormat::default().as_str(), "rgba8unorm-srgb");
        assert!(!SceneColorFormat::default().is_hdr());
        assert!(SceneColorFormat::Rgba16Float.is_hdr());
    }

    /// 纯 2D 帧即使宿主显式要求 HDR，也不得创建 HDR 目标（旧 golden 的零 diff 前提）。
    #[test]
    fn hdr_needs_policy_capability_content_and_unity_exposure() {
        let on = RenderSettings::default().with_hdr(ToggleMode::On);
        assert!(hdr_activation(on, true, true, None));
        assert!(hdr_activation(on, true, true, Some(1.0)));
        assert!(!hdr_activation(RenderSettings::default(), true, true, None));
        assert!(!hdr_activation(on, false, true, None));
        assert!(!hdr_activation(on, true, false, None));
        assert!(!hdr_activation(on, true, true, Some(2.0)));
        // Auto 不是明确要求，但策略上允许：能力与内容都满足时同样启用。
        let auto = RenderSettings::default().with_hdr(ToggleMode::Auto);
        assert!(hdr_activation(auto, true, true, None));
    }

    /// P3-R03：IBL 档位只看"策略 × 有没有环境 × 镜面能力"，Off 必须保留旧常数环境光。
    #[test]
    fn ibl_resolution_prefers_off_then_diffuse_then_full() {
        let off = RenderSettings::default();
        let on = RenderSettings {
            ibl: ToggleMode::On,
            ..RenderSettings::default()
        };
        let auto = RenderSettings {
            ibl: ToggleMode::Auto,
            ..RenderSettings::default()
        };
        assert_eq!(resolve_ibl(off, true, true), IblMode::Off);
        assert_eq!(resolve_ibl(on, false, true), IblMode::Off);
        assert_eq!(resolve_ibl(on, true, false), IblMode::Diffuse);
        assert_eq!(resolve_ibl(auto, true, false), IblMode::Diffuse);
        assert_eq!(resolve_ibl(on, true, true), IblMode::Full);
        assert_eq!(resolve_ibl(auto, true, true), IblMode::Full);
    }

    /// P3-R03-2：环境 uniform 40 f32——9 个 vec4 槽 + params（intensity/rotation/mode/0）。
    #[test]
    fn environment_uniform_packs_sh_into_vec4_slots() {
        let mut sh = [0.0_f32; ENVIRONMENT_SH_COEFFICIENTS];
        sh[0] = 1.0;
        sh[1] = 2.0;
        sh[2] = 3.0;
        sh[3] = 4.0;
        let lighting = EnvironmentLighting {
            sh,
            settings: EnvironmentSettings {
                intensity: 2.0,
                rotation: 0.5,
            },
        };
        let uniform = lighting.uniform(IblMode::Specular);
        assert_eq!(uniform.len(), 40);
        assert_eq!(&uniform[0..4], &[1.0, 2.0, 3.0, 0.0]);
        assert_eq!(&uniform[4..8], &[4.0, 0.0, 0.0, 0.0]);
        assert!(uniform[8..36].iter().all(|value| *value == 0.0));
        assert_eq!(&uniform[36..40], &[2.0, 0.5, 2.0, 0.0]);
    }

    /// P3-R03-4：档位只由两条独立能力决定；C 档必须回退 SH-only 且带可读原因。
    #[test]
    fn ibl_tier_reflects_float_cube_and_filtering() {
        assert_eq!(ibl_tier(true, true), IblTier::A);
        assert_eq!(ibl_tier(true, false), IblTier::B);
        assert_eq!(ibl_tier(false, true), IblTier::C);
        assert_eq!(ibl_tier(false, false), IblTier::C);
        assert!(IblTier::A.supports_specular());
        assert!(IblTier::B.supports_specular());
        assert!(!IblTier::C.supports_specular());
        assert_eq!(
            [
                IblTier::A.as_str(),
                IblTier::B.as_str(),
                IblTier::C.as_str()
            ],
            ["A", "B", "C"]
        );
    }

    #[test]
    fn ibl_modes_map_to_features_and_codes() {
        assert!(IblMode::Off.features().is_empty());
        assert_eq!(IblMode::Diffuse.features(), &["IBL_DIFFUSE"]);
        assert_eq!(IblMode::Specular.features(), &["IBL_SPECULAR"]);
        assert_eq!(IblMode::Full.features(), &["IBL_DIFFUSE", "IBL_SPECULAR"]);
        for (mode, code) in [
            (IblMode::Off, 0),
            (IblMode::Diffuse, 1),
            (IblMode::Specular, 2),
            (IblMode::Full, 3),
        ] {
            assert_eq!(mode.code(), code);
            assert_eq!(mode.has_diffuse(), code & 1 == 1);
            assert_eq!(mode.has_specular(), code & 2 == 2);
        }
        assert_eq!(IblMode::default(), IblMode::Off);
        assert_eq!(EnvironmentSettings::default().intensity, 1.0);
    }
}
