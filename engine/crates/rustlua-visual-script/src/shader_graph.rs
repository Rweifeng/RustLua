//! 面向非 shader 开发者的可视化 Shader Graph 数据层。
//!
//! 图资产是 JSON，编译结果是可热重载的 WGSL 片段。该模块不依赖 GPU，编辑器可以
//! 在没有显卡的情况下完成模板选择、参数编辑和错误提示。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const SHADER_GRAPH_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShaderGraph {
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub nodes: Vec<ShaderNode>,
    #[serde(default)]
    pub edges: Vec<ShaderEdge>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShaderNode {
    pub id: String,
    pub kind: ShaderNodeKind,
    #[serde(default)]
    pub position: [f32; 2],
    #[serde(default)]
    pub params: BTreeMap<String, f32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShaderNodeKind {
    Uv,
    Time,
    VertexColor,
    Texture,
    Constant,
    Add,
    Multiply,
    Sine,
    Fresnel,
    /// 扩张圆环：`1 - |r - fract(time*speed)|`，解锁冲击波/波纹推进。
    Ring,
    /// 光束：横贯 uv.y 的窄亮条，解锁激光/瞄准线。
    Beam,
    /// 场景颜色：采样当前画面，解锁染色/折射/扭曲类模板。
    SceneColor,
    /// UV 扭曲：沿时间/UV 抖动坐标，配合场景颜色做热浪与折射。
    DistortUv,
    /// 旋转 UV：绕中心旋转采样坐标（角度 = angle0 + time * speed）。
    RotateUv,
    /// 极坐标：输出 (归一化角度, 归一化半径)，用来做螺旋/隧道/环形采样。
    Polar,
    /// 角度遮罩：随时间旋转的扇形环带，输出 0..1 掩码（接 alpha），解锁刀光/扇形预警。
    Sector,
    /// 竖向扭曲：水平偏移随 y 与时间变化，适合水柱、热浪与水下折射。
    VerticalStretch,
    /// 方向渐变：沿指定方向的 0..1 渐变（输出 vec4），适合线性光带与遮罩。
    Gradient,
    /// 深度描边：邻居深度差分，输出边缘强度（接 alpha 就是屏幕描边）。
    DepthEdge,
    /// 深度雾：归一化深度做幂次，输出雾浓度（接 alpha 就是距离雾）。
    DepthFog,
    Output,
}

/// WGSL-compatible value categories carried by graph ports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShaderValueType {
    Scalar,
    Vec2,
    Vec3,
    Vec4,
}

/// 参数 uniform 的 `vec4` 槽位数（一个标量占一个槽的 x，颜色占 xyz）。
///
/// 硬上限：uniform 里是一个定长数组，槽位数固定才能让 buffer 大小恒定、改参数不重编管线。
pub const MAX_PARAM_SLOTS: usize = 16;

/// 参数 uniform 化的编译结果：源码 + 槽位表 + 默认值。
#[derive(Clone, Debug, PartialEq)]
pub struct CompiledShader {
    /// 完整 WGSL 片段（含 `GraphParams` uniform 声明）。
    pub source: String,
    /// 每个可调参数落到哪个 `vec4` 槽的哪个分量。
    pub params: Vec<ShaderParam>,
    /// 各槽的默认值（来自图里的 `params`，缺省用节点默认值）。
    pub slots: Vec<[f32; 4]>,
}

impl CompiledShader {
    /// 槽位值压平成 f32 数组（`slot_count * 4` 个），可直接写进 uniform buffer。
    #[must_use]
    pub fn slot_values(&self) -> Vec<f32> {
        self.slots
            .iter()
            .flat_map(|slot| slot.iter().copied())
            .collect()
    }

    /// 给非程序员看的命名参数表：`strength / speed / frequency / color_*` 等。
    ///
    /// 命名规则：同一个参数名全图唯一时直接用参数名（`strength`）；出现重名时带节点前缀
    /// （`flame_speed`），避免两张同名参数的节点互相覆盖。颜色三通道按 `_r/_g/_b` 展开。
    #[must_use]
    pub fn named_params(&self) -> Vec<NamedShaderParam> {
        use std::collections::BTreeMap;
        // (node, param) → 该参数占的 (slot, component) 列表
        let mut groups: BTreeMap<(String, &'static str), Vec<(usize, usize)>> = BTreeMap::new();
        for param in &self.params {
            groups
                .entry((param.node.clone(), param.param))
                .or_default()
                .push((param.slot, param.component));
        }
        let mut key_counts: BTreeMap<&'static str, usize> = BTreeMap::new();
        for (_, key) in groups.keys() {
            *key_counts.entry(key).or_default() += 1;
        }
        let mut named = Vec::new();
        for ((node, key), slots) in groups {
            let base = if key_counts.get(key).copied().unwrap_or(0) > 1 {
                let safe: String = node
                    .chars()
                    .map(|ch| {
                        if ch.is_ascii_alphanumeric() || ch == '_' {
                            ch
                        } else {
                            '_'
                        }
                    })
                    .collect();
                format!("{safe}_{key}")
            } else {
                key.to_owned()
            };
            let is_color = key == "color" && slots.len() >= 3;
            for (slot, component) in slots {
                let name = if is_color && component < 3 {
                    format!("{base}_{}", ["r", "g", "b"][component])
                } else {
                    base.clone()
                };
                named.push(NamedShaderParam {
                    name,
                    slot,
                    component,
                    default: self.slots.get(slot).copied().unwrap_or([0.0; 4])[component],
                });
            }
        }
        named
    }
}

/// 一个命名参数：名字 → `vec4` 槽号 + 分量 + 默认值。
#[derive(Clone, Debug, PartialEq)]
pub struct NamedShaderParam {
    pub name: String,
    pub slot: usize,
    pub component: usize,
    pub default: f32,
}

/// 一个参数在 uniform 里的落位：节点 id + 参数名 → `vec4` 槽号与分量（x=0..w=3）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShaderParam {
    pub node: String,
    pub param: &'static str,
    pub slot: usize,
    pub component: usize,
}

impl ShaderNodeKind {
    /// 画布节点标题用的中文名。与 `hint_zh` 一起构成非程序员能读懂的节点说明。
    #[must_use]
    pub const fn name_zh(self) -> &'static str {
        match self {
            Self::Uv => "UV 坐标",
            Self::Time => "时间",
            Self::VertexColor => "顶点色",
            Self::Texture => "纹理采样",
            Self::Constant => "颜色常量",
            Self::Add => "相加",
            Self::Multiply => "相乘",
            Self::Sine => "正弦波形",
            Self::Fresnel => "边缘光",
            Self::Ring => "扩张圆环",
            Self::Beam => "光束",
            Self::SceneColor => "场景颜色",
            Self::DistortUv => "UV 扭曲",
            Self::RotateUv => "旋转 UV",
            Self::Polar => "极坐标",
            Self::Sector => "扇形遮罩",
            Self::VerticalStretch => "竖向扭曲",
            Self::Gradient => "方向渐变",
            Self::DepthEdge => "深度描边",
            Self::DepthFog => "深度雾",
            Self::Output => "片元输出",
        }
    }

    /// 一句话说明「这个节点做什么、结果拿来干什么」。
    #[must_use]
    pub const fn hint_zh(self) -> &'static str {
        match self {
            Self::Uv => "读取当前像素的纹理坐标，用来做扫描线、波纹和渐变",
            Self::Time => "游戏运行秒数，让效果动起来",
            Self::VertexColor => "模型/精灵自带的颜色与透明度",
            Self::Texture => "按 UV 采样纹理颜色；需要接到 UV 坐标才有变化",
            Self::Constant => "固定颜色，用作主色或染色",
            Self::Add => "把两路颜色相加，适合提亮和叠加",
            Self::Multiply => "把两路颜色相乘，适合染色和压暗",
            Self::Sine => "正弦波形；接时间会脉动，再接 UV 会沿 UV 扫描",
            Self::Fresnel => "边缘越亮：没接视角时用「离中心距离」近似，适合描边与光环",
            Self::Ring => "从中心向外扩散的圆环，适合冲击波、波纹推进与技能范围",
            Self::Beam => "横贯画面的窄亮条，适合激光、瞄准线与扫描束",
            Self::SceneColor => "采样已经画好的画面，适合染色、折射、扭曲与热浪",
            Self::DistortUv => "把坐标按时间和 UV 抖动，做出热浪、折射与玻璃扭曲",
            Self::RotateUv => "绕中心旋转采样坐标，适合漩涡、刀光与旋转传送门",
            Self::Polar => "把平面坐标变成(角度,半径)，适合螺旋、隧道与环形采样",
            Self::Sector => "随时间旋转的扇形环带，适合刀光弧、扇形技能预警与扫描雷达",
            Self::VerticalStretch => "水平偏移随时间与高度变化，适合水柱、热浪与水下折射",
            Self::Gradient => "沿指定方向的亮度渐变，适合线性光带、扫光与遮罩",
            Self::DepthEdge => "按邻近像素的深度差勾出轮廓，适合选中描边和 X 光",
            Self::DepthFog => "按到相机的距离生成雾浓度，适合远景雾和深水",
            Self::Output => "最终颜色与透明度输出到材质",
        }
    }

    /// 输入端口名（编辑器画布据此画入口圆点）。
    #[must_use]
    pub const fn input_ports(self) -> &'static [&'static str] {
        match self {
            Self::Sine => &["uv", "time"],
            Self::Fresnel => &["normal", "view"],
            Self::Add | Self::Multiply => &["a", "b"],
            Self::Texture => &["uv"],
            Self::Output => &["tint", "alpha"],
            Self::SceneColor
            | Self::DistortUv
            | Self::DepthEdge
            | Self::DepthFog
            | Self::RotateUv
            | Self::Polar
            | Self::Sector
            | Self::VerticalStretch => &["uv"],
            Self::Gradient
            | Self::Uv
            | Self::Time
            | Self::VertexColor
            | Self::Constant
            | Self::Ring
            | Self::Beam => &[],
        }
    }

    /// 输出端口名。当前所有能产出值的节点都只有一个 `value`。
    #[must_use]
    pub const fn output_ports(self) -> &'static [&'static str] {
        match self {
            Self::Output => &[],
            _ => &["value"],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShaderEdge {
    pub from: ShaderPort,
    pub to: ShaderPort,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShaderPort {
    pub node: String,
    pub port: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShaderTemplate {
    Dissolve,
    EnergyShield,
    Hologram,
    WaterRipple,
    Fire,
    MagicCircle,
    FresnelOutline,
    HitFlash,
    Scanlines,
    Portal,
    Lava,
    Poison,
    Ice,
    Lightning,
    Smoke,
    Bloom,
    Shadow,
    Rainbow,
    Underwater,
    SpeedLines,
    /// 状态染色：受击/中毒/冰冻之外的持续状态色。
    CharacterTint,
    /// 残影/隐身：透明度随波形变化 + 冷色偏移。
    Afterimage,
    /// 拾取物/宝箱闪烁高亮。
    PickupBlink,
    /// 低血量屏幕暗角。
    LowHealthVignette,
    /// 治疗/回复光环。
    HealAura,
    /// 技能预警范围提示。
    SkillWarning,
    /// 灼烧边缘：角色身上被点燃的火焰轮廓。
    BurnEdge,
    /// 冲击波：扩张圆环 + 脉冲。
    ImpactRing,
    /// 激光束：横贯画面的窄亮条。
    LaserBeam,
    /// 屏幕染色：采样画面再染色，适合受伤/夜色/毒雾滤镜。
    SceneTint,
    /// 热浪扭曲：画面按时间抖动，适合热浪/火焰上方的空气扭曲。
    HeatHaze,
    /// 折射：画面按偏移采样并偏冷色，适合玻璃、水面与能量场。
    SceneRefract,
    /// 深度描边：画面 + 深度边缘 alpha，适合选中/透视描边。
    DepthOutline,
    /// 深度雾：雾色 + 距离 alpha，适合远景雾与深水。
    DepthFogView,
    /// 环形刀光：扩张圆环 + 快速脉冲，适合斩击/挥砍轨迹。
    RingSlash,
    /// 极光：缓慢流动的绿色光带，适合夜晚天空与高纬度场景。
    Aurora,
    /// 星尘：高频闪烁的点状光，适合夜空与魔法粒子氛围。
    Starfield,
    /// 螺旋传送：极坐标采样画面，适合传送门与隧道。
    SpiralPortal,
    /// 漩涡扭曲：旋转采样画面，适合漩涡、时空扭曲与眩晕。
    VortexWarp,
    /// 刀光弧：旋转扇形环带，适合斩击轨迹。
    SlashArc,
    /// 扇形预警：宽扇区脉冲，适合技能范围提示。
    SectorWarning,
    /// 雨幕：竖排光束 + 高频闪烁，适合下雨与坠落的雨丝。
    Rain,
    /// 水柱扭曲：竖向抖动采样画面，适合瀑布、水柱与水下。
    WaterColumn,
    /// 线性扫光：方向渐变脉冲，适合 UI 扫光与技能充能。
    LightSweep,
    /// 散景光斑：柔和径向光晕，适合 bokeh、灯光光晕与氛围点光。
    Bokeh,
    /// 能量脉冲：扩张环的快速脉冲，适合能量爆发与冲击波。
    EnergyPulse,
    /// 胶片颗粒：高频噪点，适合复古滤镜、紧张与回忆氛围。
    FilmGrain,
}

impl ShaderTemplate {
    /// 顺序按分类分组：面板下拉框直接按 `ALL` 顺序分组显示，不必再维护一份排序表。
    pub const ALL: [Self; 47] = [
        // 角色与状态
        Self::Dissolve,
        Self::FresnelOutline,
        Self::HitFlash,
        Self::CharacterTint,
        Self::Ice,
        Self::Shadow,
        Self::Bloom,
        Self::Afterimage,
        Self::PickupBlink,
        Self::LowHealthVignette,
        // 技能与魔法
        Self::EnergyShield,
        Self::Hologram,
        Self::MagicCircle,
        Self::Portal,
        Self::Lightning,
        Self::Rainbow,
        Self::HealAura,
        Self::SkillWarning,
        Self::ImpactRing,
        Self::LaserBeam,
        // 环境与元素
        Self::WaterRipple,
        Self::Fire,
        Self::Lava,
        Self::Poison,
        Self::Smoke,
        Self::Underwater,
        Self::BurnEdge,
        // UI 与后处理
        Self::Scanlines,
        Self::SpeedLines,
        Self::SceneTint,
        Self::HeatHaze,
        Self::SceneRefract,
        Self::DepthOutline,
        Self::DepthFogView,
        Self::RingSlash,
        Self::Aurora,
        Self::Starfield,
        Self::SpiralPortal,
        Self::VortexWarp,
        Self::SlashArc,
        Self::SectorWarning,
        Self::Rain,
        Self::WaterColumn,
        Self::LightSweep,
        Self::Bokeh,
        Self::EnergyPulse,
        Self::FilmGrain,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Dissolve => "dissolve",
            Self::EnergyShield => "energy_shield",
            Self::Hologram => "hologram",
            Self::WaterRipple => "water_ripple",
            Self::Fire => "fire",
            Self::MagicCircle => "magic_circle",
            Self::FresnelOutline => "fresnel_outline",
            Self::HitFlash => "hit_flash",
            Self::Scanlines => "scanlines",
            Self::Portal => "portal",
            Self::Lava => "lava",
            Self::Poison => "poison",
            Self::Ice => "ice",
            Self::Lightning => "lightning",
            Self::Smoke => "smoke",
            Self::Bloom => "bloom",
            Self::Shadow => "shadow",
            Self::Rainbow => "rainbow",
            Self::Underwater => "underwater",
            Self::SpeedLines => "speed_lines",
            Self::CharacterTint => "character_tint",
            Self::Afterimage => "afterimage",
            Self::PickupBlink => "pickup_blink",
            Self::LowHealthVignette => "low_health_vignette",
            Self::HealAura => "heal_aura",
            Self::SkillWarning => "skill_warning",
            Self::BurnEdge => "burn_edge",
            Self::ImpactRing => "impact_ring",
            Self::LaserBeam => "laser_beam",
            Self::SceneTint => "scene_tint",
            Self::HeatHaze => "heat_haze",
            Self::SceneRefract => "scene_refract",
            Self::DepthOutline => "depth_outline",
            Self::DepthFogView => "depth_fog",
            Self::RingSlash => "ring_slash",
            Self::Aurora => "aurora",
            Self::Starfield => "starfield",
            Self::SpiralPortal => "spiral_portal",
            Self::VortexWarp => "vortex_warp",
            Self::Rain => "rain",
            Self::LightSweep => "light_sweep",
            Self::Bokeh => "bokeh",
            Self::EnergyPulse => "energy_pulse",
            Self::FilmGrain => "film_grain",
            Self::WaterColumn => "water_column",
            Self::SectorWarning => "sector_warning",
            Self::SlashArc => "slash_arc",
        }
    }

    pub const fn name_zh(self) -> &'static str {
        match self {
            Self::Dissolve => "溶解消散",
            Self::EnergyShield => "能量护盾",
            Self::Hologram => "全息扫描",
            Self::WaterRipple => "水波扭曲",
            Self::Fire => "火焰流动",
            Self::MagicCircle => "魔法阵",
            Self::FresnelOutline => "边缘描边",
            Self::HitFlash => "受击闪白",
            Self::Scanlines => "复古扫描线",
            Self::Portal => "传送门",
            Self::Lava => "熔岩流动",
            Self::Poison => "毒雾",
            Self::Ice => "冰霜结晶",
            Self::Lightning => "闪电脉冲",
            Self::Smoke => "烟雾翻涌",
            Self::Bloom => "辉光脉动",
            Self::Shadow => "暗影侵蚀",
            Self::Rainbow => "彩虹光谱",
            Self::Underwater => "水下色偏",
            Self::SpeedLines => "速度线",
            Self::CharacterTint => "状态染色",
            Self::Afterimage => "残影隐身",
            Self::PickupBlink => "拾取闪烁",
            Self::LowHealthVignette => "低血量暗角",
            Self::HealAura => "治疗光环",
            Self::SkillWarning => "技能预警圈",
            Self::BurnEdge => "灼烧边缘",
            Self::ImpactRing => "冲击波",
            Self::LaserBeam => "激光束",
            Self::SceneTint => "屏幕染色",
            Self::HeatHaze => "热浪扭曲",
            Self::SceneRefract => "屏幕折射",
            Self::DepthOutline => "深度描边",
            Self::DepthFogView => "深度雾",
            Self::RingSlash => "环形刀光",
            Self::Aurora => "极光",
            Self::Starfield => "星尘",
            Self::SpiralPortal => "螺旋传送",
            Self::VortexWarp => "漩涡扭曲",
            Self::Rain => "雨幕",
            Self::LightSweep => "线性扫光",
            Self::Bokeh => "散景光斑",
            Self::EnergyPulse => "能量脉冲",
            Self::FilmGrain => "胶片颗粒",
            Self::WaterColumn => "水柱扭曲",
            Self::SectorWarning => "扇形预警",
            Self::SlashArc => "刀光弧",
        }
    }

    pub const fn hint_zh(self) -> &'static str {
        match self {
            Self::Dissolve => "用噪声阈值控制透明度，适合角色消失、传送和物体生成。",
            Self::EnergyShield => "用 Fresnel 边缘光叠加扫描线，适合护盾、结界和安全区域。",
            Self::Hologram => "用时间和 UV 生成横向扫描线，适合全息投影、科幻 UI 和幽灵。",
            Self::WaterRipple => "用时间推动 UV 波纹，适合水面、热浪和空间扭曲。",
            Self::Fire => "用两层滚动噪声混合颜色，适合火焰、魔法火花和熔岩。",
            Self::MagicCircle => "用极坐标环带和旋转时间，适合魔法阵、技能预警和传送门。",
            Self::FresnelOutline => "根据视线夹角增强边缘亮度，适合选中描边和稀有物品高亮。",
            Self::HitFlash => "短时间把颜色推向白色，适合受击反馈和无敌帧提示。",
            Self::Scanlines => "叠加周期性横向亮暗条纹，适合 CRT、监控画面和复古界面。",
            Self::Portal => "用旋转波纹制造传送门能量，适合入口、出口和快速移动特效。",
            Self::Lava => "用暖色滚动波纹表现岩浆流动，适合熔岩、熔炉和火山场景。",
            Self::Poison => "用绿色脉动和透明度表现毒雾，适合中毒区域和药剂效果。",
            Self::Ice => "用冷色边缘光表现冰晶质感，适合冰冻、护甲和寒冰技能。",
            Self::Lightning => "用高频脉冲制造电光闪烁，适合闪电、雷击和电弧武器。",
            Self::Smoke => "用低频波纹表现烟雾厚度变化，适合尘埃、爆炸和遮蔽。",
            Self::Bloom => "用柔和脉动强调高亮区域，适合宝物、拾取物和魔法核心。",
            Self::Shadow => "用暗色透明波纹制造侵蚀感，适合暗影、诅咒和黑雾。",
            Self::Rainbow => "用时间驱动色彩变化，适合彩虹、棱镜和稀有品质反馈。",
            Self::Underwater => "用蓝绿色波动模拟水下光照，适合水底、潜水和潮湿区域。",
            Self::SpeedLines => "用快速时间波纹制造拉伸感，适合冲刺、飞行和高速移动。",
            Self::CharacterTint => "用状态色叠加在角色纹理上，适合中毒、灼烧和冰冻等持续状态。",
            Self::Afterimage => "用波形控制透明度并偏冷色，适合残影、隐身和闪避。",
            Self::PickupBlink => "用周期性亮度闪烁突出可拾取物，适合金币、掉落和宝箱。",
            Self::LowHealthVignette => "用边缘暗角提示危险，适合低血量、载具损坏和 Boss 狂暴。",
            Self::HealAura => "用绿色边缘光和脉动表现回复，适合治疗技能和回复区域。",
            Self::SkillWarning => "用红色边缘脉动标出技能范围，适合地面预警和范围伤害提示。",
            Self::BurnEdge => "用橙色边缘光和快速闪烁表现灼烧，适合点燃和持续伤害。",
            Self::ImpactRing => "扩张圆环配合脉冲，适合爆炸、落地冲击和范围技能。",
            Self::LaserBeam => "窄亮光束横贯画面，适合激光武器、瞄准线和扫描束。",
            Self::SceneTint => "采样已经画好的画面再染色，适合受伤滤镜、夜色和毒雾屏效。",
            Self::HeatHaze => "画面按时间抖动，适合热浪、火焰上方的空气扭曲和传送门。",
            Self::SceneRefract => "画面按偏移采样并偏冷，适合玻璃、水面与能量场折射。",
            Self::DepthOutline => "按深度差勾出物体轮廓，适合选中描边、X 光和剖面提示。",
            Self::DepthFogView => "按到相机的距离叠雾，适合远景雾、深水与恐怖氛围。",
            Self::RingSlash => "扩张圆环配合快速脉冲，适合斩击、挥砍与技能轨迹。",
            Self::Aurora => "缓慢流动的绿色光带，适合夜晚天空与高纬度环境。",
            Self::Starfield => "高频闪烁的点状光，适合夜空、魔法粒子与氛围点缀。",
            Self::SpiralPortal => "把画面按极坐标卷成螺旋，适合传送门、隧道与吸入效果。",
            Self::VortexWarp => "绕中心旋转采样画面，适合漩涡、时空扭曲与眩晕。",
            Self::Rain => "竖排光束配高频闪烁，适合下雨、雨丝与坠落特效。",
            Self::LightSweep => "方向渐变的脉冲光带，适合 UI 扫光与技能充能。",
            Self::Bokeh => "柔和径向光晕，适合 bokeh、灯光光晕与氛围点光。",
            Self::EnergyPulse => "扩张环的快速脉冲，适合能量爆发与冲击波。",
            Self::FilmGrain => "高频噪点，适合复古滤镜、紧张与回忆氛围。",
            Self::WaterColumn => "竖向抖动采样画面，适合瀑布、水柱与水下折射。",
            Self::SectorWarning => "宽扇区脉冲，适合技能范围预警与扫描雷达。",
            Self::SlashArc => "旋转扇形环带，适合挥砍轨迹、斩击与技能刀光。",
        }
    }

    /// 模板分类（角色 / 技能 / 环境 / UI / 后处理），供面板分组与文档检索。
    pub const fn category_zh(self) -> &'static str {
        match self {
            Self::Dissolve
            | Self::FresnelOutline
            | Self::HitFlash
            | Self::Ice
            | Self::Shadow
            | Self::Bloom
            | Self::CharacterTint
            | Self::Afterimage
            | Self::PickupBlink
            | Self::LowHealthVignette
            | Self::DepthOutline
            | Self::RingSlash
            | Self::SpiralPortal
            | Self::SlashArc => "角色/状态",
            Self::EnergyShield
            | Self::Hologram
            | Self::MagicCircle
            | Self::Portal
            | Self::Lightning
            | Self::Rainbow
            | Self::HealAura
            | Self::SkillWarning
            | Self::ImpactRing
            | Self::LaserBeam
            | Self::SectorWarning
            | Self::EnergyPulse => "技能/魔法",
            Self::WaterRipple
            | Self::Fire
            | Self::Lava
            | Self::Poison
            | Self::Smoke
            | Self::Underwater
            | Self::BurnEdge
            | Self::HeatHaze
            | Self::SceneRefract
            | Self::DepthFogView
            | Self::Aurora
            | Self::Starfield
            | Self::VortexWarp
            | Self::Rain
            | Self::WaterColumn => "环境/元素",
            Self::Scanlines
            | Self::SpeedLines
            | Self::SceneTint
            | Self::LightSweep
            | Self::Bokeh
            | Self::FilmGrain => "UI/后处理",
        }
    }

    /// 性能等级：L0 = 纯颜色运算（几乎零成本），L1 = 单次正弦/边缘运算，
    /// L2 = 纹理采样，L3 = 多次采样或全屏后处理。
    pub const fn perf_level(self) -> u8 {
        match self {
            Self::Dissolve
            | Self::WaterRipple
            | Self::Fire
            | Self::Lava
            | Self::Poison
            | Self::Smoke
            | Self::Hologram
            | Self::Underwater
            | Self::Scanlines
            | Self::SceneTint
            | Self::HeatHaze
            | Self::SceneRefract
            | Self::DepthOutline
            | Self::DepthFogView
            | Self::RingSlash
            | Self::Aurora
            | Self::Starfield
            | Self::SpiralPortal
            | Self::VortexWarp
            | Self::SlashArc
            | Self::SectorWarning
            | Self::Rain
            | Self::WaterColumn
            | Self::Shadow => 2,
            Self::EnergyShield
            | Self::MagicCircle
            | Self::Portal
            | Self::Ice
            | Self::Lightning
            | Self::FresnelOutline
            | Self::Bloom
            | Self::Rainbow
            | Self::SpeedLines
            | Self::HitFlash
            | Self::Afterimage
            | Self::PickupBlink
            | Self::LowHealthVignette
            | Self::CharacterTint
            | Self::HealAura
            | Self::SkillWarning
            | Self::BurnEdge
            | Self::ImpactRing
            | Self::LaserBeam
            | Self::LightSweep
            | Self::Bokeh
            | Self::EnergyPulse
            | Self::FilmGrain => 1,
        }
    }

    /// 给非 shader 开发者看的一段「先调什么」提示，和文档里的参数表同口径。
    pub const fn tune_zh(self) -> &'static str {
        match self {
            Self::Dissolve => "先调溶解阈值与边缘宽度，再选边缘颜色",
            Self::EnergyShield => "先调边缘强度与扫描速度",
            Self::Hologram => "先调扫描线密度与主色",
            Self::WaterRipple => "先调波纹密度与流速",
            Self::Fire => "先调流动速度与颜色梯度",
            Self::MagicCircle => "先调环带宽度与旋转速度",
            Self::FresnelOutline => "先调描边宽度与颜色",
            Self::HitFlash => "先调闪白强度和持续时间",
            Self::Scanlines => "先调条纹疏密与亮度",
            Self::Portal => "先调旋转速度与能量颜色",
            Self::Lava => "先调滚动速度与熔岩亮色",
            Self::Poison => "先调脉动速度与毒雾颜色",
            Self::Ice => "先调边缘强度与冰蓝色",
            Self::Lightning => "先调闪烁频率与电弧亮度",
            Self::Smoke => "先调翻涌速度与烟尘颜色",
            Self::Bloom => "先调脉动柔和度与高亮颜色",
            Self::Shadow => "先调侵蚀速度与暗色",
            Self::Rainbow => "先调色相循环速度",
            Self::Underwater => "先调波动速度与水色",
            Self::SpeedLines => "先调拉伸速度与线条亮度",
            Self::CharacterTint => "先调状态颜色与混合强度",
            Self::Afterimage => "先调残影透明度与时效",
            Self::PickupBlink => "先调闪烁频率与高亮颜色",
            Self::LowHealthVignette => "先调暗角强度与危险颜色",
            Self::HealAura => "先调光环范围与回复色",
            Self::SkillWarning => "先调预警脉动速度与警示色",
            Self::BurnEdge => "先调灼烧频率与火焰颜色",
            Self::ImpactRing => "先调扩散速度与环宽",
            Self::LaserBeam => "先调光束宽度与亮度",
            Self::SceneTint => "先调染色与强度",
            Self::HeatHaze => "先调扭曲强度与速度",
            Self::SceneRefract => "先调偏移量与折射颜色",
            Self::DepthOutline => "先调描边宽度与强度",
            Self::DepthFogView => "先调雾浓度与雾颜色",
            Self::RingSlash => "先调扩散速度与闪光频率",
            Self::Aurora => "先调光带流动速度与颜色",
            Self::Starfield => "先调闪烁频率与星点亮度",
            Self::SpiralPortal => "先调旋转速度与极坐标缩放",
            Self::VortexWarp => "先调旋转速度与扭曲强度",
            Self::Rain => "先调雨丝宽度、密度与亮度",
            Self::LightSweep => "先调方向与扫光速度",
            Self::Bokeh => "先调光晕强度与颜色",
            Self::EnergyPulse => "先调扩散速度与脉冲频率",
            Self::FilmGrain => "先调颗粒频率与强度",
            Self::WaterColumn => "先调抖幅与流速",
            Self::SectorWarning => "先调扇区角度、转速与警示色",
            Self::SlashArc => "先调弧宽、转速与半径",
        }
    }

    pub fn graph(self) -> ShaderGraph {
        let output = ShaderNode {
            id: "out".into(),
            kind: ShaderNodeKind::Output,
            position: [640.0, 0.0],
            params: BTreeMap::new(),
        };
        let (nodes, edges) = match self {
            // 角色/状态 -----------------------------------------------------------------
            Self::Dissolve => (
                vec![
                    node("uv", ShaderNodeKind::Uv),
                    node("noise", ShaderNodeKind::Texture),
                    node("time", ShaderNodeKind::Time),
                    wave("cut", 80.0),
                    color("edge", 0.15, 0.9, 1.0),
                    output.clone(),
                ],
                vec![
                    edge("uv", "value", "noise", "uv"),
                    edge("uv", "value", "cut", "uv"),
                    edge("time", "value", "cut", "time"),
                    edge("noise", "value", "out", "tint"),
                    edge("cut", "value", "out", "alpha"),
                ],
            ),
            Self::FresnelOutline => (
                vec![node("fresnel", ShaderNodeKind::Fresnel), output.clone()],
                vec![edge("fresnel", "value", "out", "tint")],
            ),
            Self::HitFlash => (
                vec![
                    node("base", ShaderNodeKind::VertexColor),
                    color("flash", 1.0, 0.95, 0.8),
                    mul("mix"),
                    node("time", ShaderNodeKind::Time),
                    wave("pulse", 30.0),
                    output.clone(),
                ],
                vec![
                    edge("base", "value", "mix", "a"),
                    edge("flash", "value", "mix", "b"),
                    edge("time", "value", "pulse", "time"),
                    edge("mix", "value", "out", "tint"),
                    edge("pulse", "value", "out", "alpha"),
                ],
            ),
            Self::Ice => (
                vec![
                    node("fresnel", ShaderNodeKind::Fresnel),
                    color("ice", 0.55, 0.85, 1.0),
                    mul("tint"),
                    node("time", ShaderNodeKind::Time),
                    wave("shimmer", 7.0),
                    output.clone(),
                ],
                vec![
                    edge("fresnel", "value", "tint", "a"),
                    edge("ice", "value", "tint", "b"),
                    edge("time", "value", "shimmer", "time"),
                    edge("tint", "value", "out", "tint"),
                    edge("shimmer", "value", "out", "alpha"),
                ],
            ),
            Self::Shadow => (
                vec![
                    node("uv", ShaderNodeKind::Uv),
                    node("noise", ShaderNodeKind::Texture),
                    color("dark", 0.18, 0.05, 0.3),
                    mul("tint"),
                    node("time", ShaderNodeKind::Time),
                    wave("creep", 2.0),
                    output.clone(),
                ],
                vec![
                    edge("uv", "value", "noise", "uv"),
                    edge("noise", "value", "tint", "a"),
                    edge("dark", "value", "tint", "b"),
                    edge("uv", "value", "creep", "uv"),
                    edge("time", "value", "creep", "time"),
                    edge("tint", "value", "out", "tint"),
                    edge("creep", "value", "out", "alpha"),
                ],
            ),
            Self::Bloom => (
                vec![
                    color("core", 1.0, 0.85, 0.4),
                    node("time", ShaderNodeKind::Time),
                    wave("glow", 6.0),
                    output.clone(),
                ],
                vec![
                    edge("core", "value", "out", "tint"),
                    edge("time", "value", "glow", "time"),
                    edge("glow", "value", "out", "alpha"),
                ],
            ),
            // 技能/魔法 -----------------------------------------------------------------
            Self::EnergyShield => (
                vec![
                    node("fresnel", ShaderNodeKind::Fresnel),
                    color("tint", 0.2, 0.7, 1.0),
                    mul("glow"),
                    node("uv", ShaderNodeKind::Uv),
                    node("time", ShaderNodeKind::Time),
                    wave("scan", 6.0),
                    output.clone(),
                ],
                vec![
                    edge("fresnel", "value", "glow", "a"),
                    edge("tint", "value", "glow", "b"),
                    edge("glow", "value", "out", "tint"),
                    edge("uv", "value", "scan", "uv"),
                    edge("time", "value", "scan", "time"),
                    edge("scan", "value", "out", "alpha"),
                ],
            ),
            Self::Hologram => (
                vec![
                    node("uv", ShaderNodeKind::Uv),
                    node("tex", ShaderNodeKind::Texture),
                    color("cyan", 0.1, 0.75, 1.0),
                    mul("tint"),
                    node("time", ShaderNodeKind::Time),
                    wave("scan", 24.0),
                    output.clone(),
                ],
                vec![
                    edge("uv", "value", "tex", "uv"),
                    edge("tex", "value", "tint", "a"),
                    edge("cyan", "value", "tint", "b"),
                    edge("uv", "value", "scan", "uv"),
                    edge("time", "value", "scan", "time"),
                    edge("tint", "value", "out", "tint"),
                    edge("scan", "value", "out", "alpha"),
                ],
            ),
            Self::MagicCircle => (
                vec![
                    node("uv", ShaderNodeKind::Uv),
                    node("tex", ShaderNodeKind::Texture),
                    color("rune", 0.7, 0.35, 1.0),
                    mul("tint"),
                    node("time", ShaderNodeKind::Time),
                    wave("pulse", 5.0),
                    output.clone(),
                ],
                vec![
                    edge("uv", "value", "tex", "uv"),
                    edge("tex", "value", "tint", "a"),
                    edge("rune", "value", "tint", "b"),
                    edge("uv", "value", "pulse", "uv"),
                    edge("time", "value", "pulse", "time"),
                    edge("tint", "value", "out", "tint"),
                    edge("pulse", "value", "out", "alpha"),
                ],
            ),
            Self::Portal => (
                vec![
                    node("fresnel", ShaderNodeKind::Fresnel),
                    color("energy", 0.3, 0.9, 1.0),
                    add("tint"),
                    node("uv", ShaderNodeKind::Uv),
                    node("time", ShaderNodeKind::Time),
                    wave("swirl", 9.0),
                    output.clone(),
                ],
                vec![
                    edge("fresnel", "value", "tint", "a"),
                    edge("energy", "value", "tint", "b"),
                    edge("uv", "value", "swirl", "uv"),
                    edge("time", "value", "swirl", "time"),
                    edge("tint", "value", "out", "tint"),
                    edge("swirl", "value", "out", "alpha"),
                ],
            ),
            Self::Lightning => (
                vec![
                    color("arc", 0.75, 0.9, 1.0),
                    node("time", ShaderNodeKind::Time),
                    wave("flicker", 60.0),
                    output.clone(),
                ],
                vec![
                    edge("arc", "value", "out", "tint"),
                    edge("time", "value", "flicker", "time"),
                    edge("flicker", "value", "out", "alpha"),
                ],
            ),
            Self::Rainbow => (
                vec![
                    color("spectrum", 0.9, 0.3, 0.75),
                    node("time", ShaderNodeKind::Time),
                    wave("cycle", 1.5),
                    output.clone(),
                ],
                vec![
                    edge("spectrum", "value", "out", "tint"),
                    edge("time", "value", "cycle", "time"),
                    edge("cycle", "value", "out", "alpha"),
                ],
            ),
            // 环境/元素 -----------------------------------------------------------------
            Self::WaterRipple => (
                vec![
                    node("uv", ShaderNodeKind::Uv),
                    node("tex", ShaderNodeKind::Texture),
                    color("water", 0.2, 0.55, 0.85),
                    add("tint"),
                    node("time", ShaderNodeKind::Time),
                    wave("ripple", 9.0),
                    output.clone(),
                ],
                vec![
                    edge("uv", "value", "tex", "uv"),
                    edge("tex", "value", "tint", "a"),
                    edge("water", "value", "tint", "b"),
                    edge("uv", "value", "ripple", "uv"),
                    edge("time", "value", "ripple", "time"),
                    edge("tint", "value", "out", "tint"),
                    edge("ripple", "value", "out", "alpha"),
                ],
            ),
            Self::Fire => (
                vec![
                    node("uv", ShaderNodeKind::Uv),
                    node("tex", ShaderNodeKind::Texture),
                    color("flame", 1.0, 0.45, 0.1),
                    mul("tint"),
                    node("time", ShaderNodeKind::Time),
                    wave("flicker", 16.0),
                    output.clone(),
                ],
                vec![
                    edge("uv", "value", "tex", "uv"),
                    edge("tex", "value", "tint", "a"),
                    edge("flame", "value", "tint", "b"),
                    edge("time", "value", "flicker", "time"),
                    edge("tint", "value", "out", "tint"),
                    edge("flicker", "value", "out", "alpha"),
                ],
            ),
            Self::Lava => (
                vec![
                    node("uv", ShaderNodeKind::Uv),
                    node("noise", ShaderNodeKind::Texture),
                    color("magma", 0.95, 0.3, 0.05),
                    mul("tint"),
                    node("time", ShaderNodeKind::Time),
                    wave("flow", 3.0),
                    output.clone(),
                ],
                vec![
                    edge("uv", "value", "noise", "uv"),
                    edge("noise", "value", "tint", "a"),
                    edge("magma", "value", "tint", "b"),
                    edge("time", "value", "flow", "time"),
                    edge("tint", "value", "out", "tint"),
                    edge("flow", "value", "out", "alpha"),
                ],
            ),
            Self::Poison => (
                vec![
                    node("uv", ShaderNodeKind::Uv),
                    node("tex", ShaderNodeKind::Texture),
                    color("toxic", 0.35, 0.85, 0.25),
                    mul("tint"),
                    node("time", ShaderNodeKind::Time),
                    wave("bubble", 4.0),
                    output.clone(),
                ],
                vec![
                    edge("uv", "value", "tex", "uv"),
                    edge("tex", "value", "tint", "a"),
                    edge("toxic", "value", "tint", "b"),
                    edge("uv", "value", "bubble", "uv"),
                    edge("time", "value", "bubble", "time"),
                    edge("tint", "value", "out", "tint"),
                    edge("bubble", "value", "out", "alpha"),
                ],
            ),
            Self::Smoke => (
                vec![
                    node("uv", ShaderNodeKind::Uv),
                    node("tex", ShaderNodeKind::Texture),
                    node("time", ShaderNodeKind::Time),
                    wave("drift", 2.0),
                    output.clone(),
                ],
                vec![
                    edge("uv", "value", "tex", "uv"),
                    edge("tex", "value", "out", "tint"),
                    edge("time", "value", "drift", "time"),
                    edge("drift", "value", "out", "alpha"),
                ],
            ),
            Self::Underwater => (
                vec![
                    node("uv", ShaderNodeKind::Uv),
                    node("tex", ShaderNodeKind::Texture),
                    color("deep", 0.15, 0.5, 0.8),
                    mul("tint"),
                    node("time", ShaderNodeKind::Time),
                    wave("caustic", 5.0),
                    output.clone(),
                ],
                vec![
                    edge("uv", "value", "tex", "uv"),
                    edge("tex", "value", "tint", "a"),
                    edge("deep", "value", "tint", "b"),
                    edge("uv", "value", "caustic", "uv"),
                    edge("time", "value", "caustic", "time"),
                    edge("tint", "value", "out", "tint"),
                    edge("caustic", "value", "out", "alpha"),
                ],
            ),
            // UI/后处理 -----------------------------------------------------------------
            Self::Scanlines => (
                vec![
                    node("uv", ShaderNodeKind::Uv),
                    node("tex", ShaderNodeKind::Texture),
                    node("time", ShaderNodeKind::Time),
                    wave("stripes", 140.0),
                    output.clone(),
                ],
                vec![
                    edge("uv", "value", "tex", "uv"),
                    edge("tex", "value", "out", "tint"),
                    edge("uv", "value", "stripes", "uv"),
                    edge("time", "value", "stripes", "time"),
                    edge("stripes", "value", "out", "alpha"),
                ],
            ),
            Self::SpeedLines => (
                vec![
                    color("streak", 1.0, 1.0, 1.0),
                    node("uv", ShaderNodeKind::Uv),
                    node("time", ShaderNodeKind::Time),
                    wave("rush", 90.0),
                    output.clone(),
                ],
                vec![
                    edge("streak", "value", "out", "tint"),
                    edge("uv", "value", "rush", "uv"),
                    edge("time", "value", "rush", "time"),
                    edge("rush", "value", "out", "alpha"),
                ],
            ),
            Self::CharacterTint => (
                vec![
                    node("base", ShaderNodeKind::VertexColor),
                    color("status", 0.35, 0.85, 0.45),
                    mul("tint"),
                    node("time", ShaderNodeKind::Time),
                    wave("breath", 3.0),
                    output.clone(),
                ],
                vec![
                    edge("base", "value", "tint", "a"),
                    edge("status", "value", "tint", "b"),
                    edge("time", "value", "breath", "time"),
                    edge("tint", "value", "out", "tint"),
                    edge("breath", "value", "out", "alpha"),
                ],
            ),
            Self::Afterimage => (
                vec![
                    node("base", ShaderNodeKind::VertexColor),
                    color("ghost", 0.4, 0.9, 1.0),
                    mul("tint"),
                    node("time", ShaderNodeKind::Time),
                    wave("fade", 18.0),
                    output.clone(),
                ],
                vec![
                    edge("base", "value", "tint", "a"),
                    edge("ghost", "value", "tint", "b"),
                    edge("time", "value", "fade", "time"),
                    edge("tint", "value", "out", "tint"),
                    edge("fade", "value", "out", "alpha"),
                ],
            ),
            Self::PickupBlink => (
                vec![
                    color("gold", 1.0, 0.85, 0.3),
                    node("time", ShaderNodeKind::Time),
                    wave("blink", 12.0),
                    output.clone(),
                ],
                vec![
                    edge("gold", "value", "out", "tint"),
                    edge("time", "value", "blink", "time"),
                    edge("blink", "value", "out", "alpha"),
                ],
            ),
            Self::LowHealthVignette => (
                vec![
                    node("fresnel", ShaderNodeKind::Fresnel),
                    color("danger", 0.85, 0.1, 0.15),
                    mul("tint"),
                    output.clone(),
                ],
                vec![
                    edge("fresnel", "value", "tint", "a"),
                    edge("danger", "value", "tint", "b"),
                    edge("tint", "value", "out", "tint"),
                ],
            ),
            Self::HealAura => (
                vec![
                    node("fresnel", ShaderNodeKind::Fresnel),
                    color("heal", 0.3, 1.0, 0.5),
                    mul("tint"),
                    node("uv", ShaderNodeKind::Uv),
                    node("time", ShaderNodeKind::Time),
                    wave("aura", 2.0),
                    output.clone(),
                ],
                vec![
                    edge("fresnel", "value", "tint", "a"),
                    edge("heal", "value", "tint", "b"),
                    edge("uv", "value", "aura", "uv"),
                    edge("time", "value", "aura", "time"),
                    edge("tint", "value", "out", "tint"),
                    edge("aura", "value", "out", "alpha"),
                ],
            ),
            Self::SkillWarning => (
                vec![
                    node("fresnel", ShaderNodeKind::Fresnel),
                    color("warn", 1.0, 0.25, 0.1),
                    mul("tint"),
                    node("time", ShaderNodeKind::Time),
                    wave("pulse", 3.0),
                    output.clone(),
                ],
                vec![
                    edge("fresnel", "value", "tint", "a"),
                    edge("warn", "value", "tint", "b"),
                    edge("time", "value", "pulse", "time"),
                    edge("tint", "value", "out", "tint"),
                    edge("pulse", "value", "out", "alpha"),
                ],
            ),
            Self::BurnEdge => (
                vec![
                    node("fresnel", ShaderNodeKind::Fresnel),
                    color("ember", 1.0, 0.35, 0.05),
                    mul("tint"),
                    node("uv", ShaderNodeKind::Uv),
                    node("time", ShaderNodeKind::Time),
                    wave("flicker", 11.0),
                    output.clone(),
                ],
                vec![
                    edge("fresnel", "value", "tint", "a"),
                    edge("ember", "value", "tint", "b"),
                    edge("uv", "value", "flicker", "uv"),
                    edge("time", "value", "flicker", "time"),
                    edge("tint", "value", "out", "tint"),
                    edge("flicker", "value", "out", "alpha"),
                ],
            ),
            Self::ImpactRing => (
                vec![
                    ring("shock", 0.6),
                    node("time", ShaderNodeKind::Time),
                    wave("pulse", 3.0),
                    output.clone(),
                ],
                vec![
                    edge("shock", "value", "out", "tint"),
                    edge("time", "value", "pulse", "time"),
                    edge("pulse", "value", "out", "alpha"),
                ],
            ),
            Self::LaserBeam => (
                vec![
                    beam("laser", 0.08),
                    node("time", ShaderNodeKind::Time),
                    wave("flicker", 30.0),
                    output.clone(),
                ],
                vec![
                    edge("laser", "value", "out", "tint"),
                    edge("time", "value", "flicker", "time"),
                    edge("flicker", "value", "out", "alpha"),
                ],
            ),
            Self::SceneTint => (
                vec![
                    node("scene", ShaderNodeKind::SceneColor),
                    color("tint", 0.85, 0.25, 0.2),
                    mul("grade"),
                    node("time", ShaderNodeKind::Time),
                    wave("breath", 2.0),
                    output.clone(),
                ],
                vec![
                    edge("scene", "value", "grade", "a"),
                    edge("tint", "value", "grade", "b"),
                    edge("grade", "value", "out", "tint"),
                    edge("time", "value", "breath", "time"),
                    edge("breath", "value", "out", "alpha"),
                ],
            ),
            Self::HeatHaze => (
                vec![
                    node("uv", ShaderNodeKind::Uv),
                    distort("warp", 0.02),
                    node("scene", ShaderNodeKind::SceneColor),
                    node("time", ShaderNodeKind::Time),
                    wave("pulse", 2.0),
                    output.clone(),
                ],
                vec![
                    edge("uv", "value", "warp", "uv"),
                    edge("warp", "value", "scene", "uv"),
                    edge("scene", "value", "out", "tint"),
                    edge("time", "value", "pulse", "time"),
                    edge("pulse", "value", "out", "alpha"),
                ],
            ),
            Self::SceneRefract => (
                vec![
                    node("uv", ShaderNodeKind::Uv),
                    distort("warp", 0.05),
                    node("scene", ShaderNodeKind::SceneColor),
                    color("glass", 0.55, 0.8, 1.0),
                    mul("tint"),
                    node("time", ShaderNodeKind::Time),
                    wave("pulse", 1.5),
                    output.clone(),
                ],
                vec![
                    edge("uv", "value", "warp", "uv"),
                    edge("warp", "value", "scene", "uv"),
                    edge("scene", "value", "tint", "a"),
                    edge("glass", "value", "tint", "b"),
                    edge("tint", "value", "out", "tint"),
                    edge("time", "value", "pulse", "time"),
                    edge("pulse", "value", "out", "alpha"),
                ],
            ),
            Self::DepthOutline => (
                vec![
                    node("uv", ShaderNodeKind::Uv),
                    depth_edge("edge", 0.002),
                    node("scene", ShaderNodeKind::SceneColor),
                    output.clone(),
                ],
                vec![
                    edge("uv", "value", "edge", "uv"),
                    edge("uv", "value", "scene", "uv"),
                    edge("scene", "value", "out", "tint"),
                    edge("edge", "value", "out", "alpha"),
                ],
            ),
            Self::DepthFogView => (
                vec![
                    node("uv", ShaderNodeKind::Uv),
                    depth_fog("fog", 2.0),
                    color("haze", 0.72, 0.78, 0.86),
                    output.clone(),
                ],
                vec![
                    edge("uv", "value", "fog", "uv"),
                    edge("haze", "value", "out", "tint"),
                    edge("fog", "value", "out", "alpha"),
                ],
            ),
            Self::RingSlash => (
                vec![
                    ring("blade", 1.2),
                    node("time", ShaderNodeKind::Time),
                    wave("flash", 7.0),
                    output.clone(),
                ],
                vec![
                    edge("blade", "value", "out", "tint"),
                    edge("time", "value", "flash", "time"),
                    edge("flash", "value", "out", "alpha"),
                ],
            ),
            Self::Aurora => (
                vec![
                    color("glow", 0.2, 0.95, 0.6),
                    node("uv", ShaderNodeKind::Uv),
                    node("time", ShaderNodeKind::Time),
                    wave("band", 4.0),
                    output.clone(),
                ],
                vec![
                    edge("glow", "value", "out", "tint"),
                    edge("uv", "value", "band", "uv"),
                    edge("time", "value", "band", "time"),
                    edge("band", "value", "out", "alpha"),
                ],
            ),
            Self::Starfield => (
                vec![
                    color("star", 1.0, 1.0, 0.95),
                    node("uv", ShaderNodeKind::Uv),
                    node("time", ShaderNodeKind::Time),
                    wave("twinkle", 45.0),
                    output.clone(),
                ],
                vec![
                    edge("star", "value", "out", "tint"),
                    edge("uv", "value", "twinkle", "uv"),
                    edge("time", "value", "twinkle", "time"),
                    edge("twinkle", "value", "out", "alpha"),
                ],
            ),
            Self::SpiralPortal => (
                vec![
                    node("uv", ShaderNodeKind::Uv),
                    node("polar", ShaderNodeKind::Polar),
                    node("scene", ShaderNodeKind::SceneColor),
                    node("time", ShaderNodeKind::Time),
                    wave("pull", 3.0),
                    output.clone(),
                ],
                vec![
                    edge("uv", "value", "polar", "uv"),
                    edge("polar", "value", "scene", "uv"),
                    edge("scene", "value", "out", "tint"),
                    edge("time", "value", "pull", "time"),
                    edge("pull", "value", "out", "alpha"),
                ],
            ),
            Self::VortexWarp => (
                vec![
                    node("uv", ShaderNodeKind::Uv),
                    rotate("spin", 0.0, 1.5),
                    node("scene", ShaderNodeKind::SceneColor),
                    node("time", ShaderNodeKind::Time),
                    wave("swirl", 2.0),
                    output.clone(),
                ],
                vec![
                    edge("uv", "value", "spin", "uv"),
                    edge("spin", "value", "scene", "uv"),
                    edge("scene", "value", "out", "tint"),
                    edge("time", "value", "swirl", "time"),
                    edge("swirl", "value", "out", "alpha"),
                ],
            ),
            Self::SlashArc => (
                vec![
                    node("uv", ShaderNodeKind::Uv),
                    sector("blade", 0.3, 0.38, 0.12, 1.6),
                    color("steel", 0.9, 0.95, 1.0),
                    output.clone(),
                ],
                vec![
                    edge("uv", "value", "blade", "uv"),
                    edge("steel", "value", "out", "tint"),
                    edge("blade", "value", "out", "alpha"),
                ],
            ),
            Self::SectorWarning => (
                vec![
                    node("uv", ShaderNodeKind::Uv),
                    sector("warn", 0.55, 0.5, 0.35, 0.6),
                    color("danger", 1.0, 0.3, 0.15),
                    output.clone(),
                ],
                vec![
                    edge("uv", "value", "warn", "uv"),
                    edge("danger", "value", "out", "tint"),
                    edge("warn", "value", "out", "alpha"),
                ],
            ),
            Self::Rain => (
                vec![
                    beam_v("streak", 0.02, 1.0),
                    color("rain", 0.65, 0.8, 0.95),
                    mul("tint"),
                    node("time", ShaderNodeKind::Time),
                    wave("pour", 60.0),
                    output.clone(),
                ],
                vec![
                    edge("streak", "value", "tint", "a"),
                    edge("rain", "value", "tint", "b"),
                    edge("tint", "value", "out", "tint"),
                    edge("time", "value", "pour", "time"),
                    edge("pour", "value", "out", "alpha"),
                ],
            ),
            Self::WaterColumn => (
                vec![
                    node("uv", ShaderNodeKind::Uv),
                    vertical_stretch("warp", 0.05, 1.2),
                    node("scene", ShaderNodeKind::SceneColor),
                    node("time", ShaderNodeKind::Time),
                    wave("flow", 2.0),
                    output.clone(),
                ],
                vec![
                    edge("uv", "value", "warp", "uv"),
                    edge("warp", "value", "scene", "uv"),
                    edge("scene", "value", "out", "tint"),
                    edge("time", "value", "flow", "time"),
                    edge("flow", "value", "out", "alpha"),
                ],
            ),
            Self::LightSweep => (
                vec![
                    gradient("sweep", 0.0),
                    node("time", ShaderNodeKind::Time),
                    wave("pulse", 1.5),
                    output.clone(),
                ],
                vec![
                    edge("sweep", "value", "out", "tint"),
                    edge("time", "value", "pulse", "time"),
                    edge("pulse", "value", "out", "alpha"),
                ],
            ),
            Self::Bokeh => (
                vec![
                    node("glow", ShaderNodeKind::Fresnel),
                    color("bokeh", 1.0, 0.85, 0.55),
                    mul("tint"),
                    node("time", ShaderNodeKind::Time),
                    wave("twinkle", 2.0),
                    output.clone(),
                ],
                vec![
                    edge("glow", "value", "tint", "a"),
                    edge("bokeh", "value", "tint", "b"),
                    edge("tint", "value", "out", "tint"),
                    edge("time", "value", "twinkle", "time"),
                    edge("twinkle", "value", "out", "alpha"),
                ],
            ),
            Self::EnergyPulse => (
                vec![
                    ring("pulse", 2.6),
                    node("time", ShaderNodeKind::Time),
                    wave("beat", 8.0),
                    output.clone(),
                ],
                vec![
                    edge("pulse", "value", "out", "tint"),
                    edge("time", "value", "beat", "time"),
                    edge("beat", "value", "out", "alpha"),
                ],
            ),
            Self::FilmGrain => (
                vec![
                    color("grain", 0.85, 0.85, 0.85),
                    node("uv", ShaderNodeKind::Uv),
                    node("time", ShaderNodeKind::Time),
                    wave("noise", 140.0),
                    output.clone(),
                ],
                vec![
                    edge("grain", "value", "out", "tint"),
                    edge("uv", "value", "noise", "uv"),
                    edge("time", "value", "noise", "time"),
                    edge("noise", "value", "out", "alpha"),
                ],
            ),
        };
        ShaderGraph {
            version: SHADER_GRAPH_VERSION,
            name: self.id().into(),
            nodes,
            edges,
        }
    }
}

fn wave(id: &str, frequency: f32) -> ShaderNode {
    let mut node = node(id, ShaderNodeKind::Sine);
    node.params.insert("frequency".into(), frequency);
    node
}

fn ring(id: &str, speed: f32) -> ShaderNode {
    let mut node = node(id, ShaderNodeKind::Ring);
    node.params.insert("speed".into(), speed);
    node
}

fn rotate(id: &str, angle0: f32, speed: f32) -> ShaderNode {
    let mut node = node(id, ShaderNodeKind::RotateUv);
    node.params.insert("angle0".into(), angle0);
    node.params.insert("angle0".into(), angle0);
    node.params.insert("spin_speed".into(), speed);
    node
}

fn depth_edge(id: &str, width: f32) -> ShaderNode {
    let mut node = node(id, ShaderNodeKind::DepthEdge);
    node.params.insert("edge_width".into(), width);
    node
}

fn depth_fog(id: &str, power: f32) -> ShaderNode {
    let mut node = node(id, ShaderNodeKind::DepthFog);
    node.params.insert("fog_power".into(), power);
    node
}

fn distort(id: &str, strength: f32) -> ShaderNode {
    let mut node = node(id, ShaderNodeKind::DistortUv);
    node.params.insert("strength".into(), strength);
    node
}

fn vertical_stretch(id: &str, amount: f32, wave_speed: f32) -> ShaderNode {
    let mut node = node(id, ShaderNodeKind::VerticalStretch);
    node.params.insert("amount".into(), amount);
    node.params.insert("wave_speed".into(), wave_speed);
    node
}

fn gradient(id: &str, direction: f32) -> ShaderNode {
    let mut node = node(id, ShaderNodeKind::Gradient);
    node.params.insert("direction".into(), direction);
    node
}

fn sector(id: &str, arc_width: f32, radius: f32, thickness: f32, spin: f32) -> ShaderNode {
    let mut node = node(id, ShaderNodeKind::Sector);
    node.params.insert("arc_width".into(), arc_width);
    node.params.insert("radius".into(), radius);
    node.params.insert("thickness".into(), thickness);
    node.params.insert("spin".into(), spin);
    node
}

fn beam_v(id: &str, width: f32, vertical: f32) -> ShaderNode {
    let mut node = beam(id, width);
    node.params.insert("vertical".into(), vertical);
    node
}

fn beam(id: &str, width: f32) -> ShaderNode {
    let mut node = node(id, ShaderNodeKind::Beam);
    node.params.insert("width".into(), width);
    node
}

fn add(id: &str) -> ShaderNode {
    node(id, ShaderNodeKind::Add)
}

fn mul(id: &str) -> ShaderNode {
    node(id, ShaderNodeKind::Multiply)
}

fn node(id: &str, kind: ShaderNodeKind) -> ShaderNode {
    ShaderNode {
        id: id.into(),
        kind,
        position: [0.0, 0.0],
        params: BTreeMap::new(),
    }
}
fn color(id: &str, r: f32, g: f32, b: f32) -> ShaderNode {
    let mut params = BTreeMap::new();
    params.insert("r".into(), r);
    params.insert("g".into(), g);
    params.insert("b".into(), b);
    ShaderNode {
        id: id.into(),
        kind: ShaderNodeKind::Constant,
        position: [0.0, 0.0],
        params,
    }
}
fn edge(a: &str, ap: &str, b: &str, bp: &str) -> ShaderEdge {
    ShaderEdge {
        from: ShaderPort {
            node: a.into(),
            port: ap.into(),
        },
        to: ShaderPort {
            node: b.into(),
            port: bp.into(),
        },
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ShaderGraphError {
    #[error("shader graph 版本不支持：{found}，需要 {expected}")]
    Version { found: u32, expected: u32 },
    #[error("节点 id 无效或重复：{0}")]
    NodeId(String),
    #[error("连线引用了不存在的节点：{0}")]
    MissingNode(String),
    #[error("输出节点必须存在且只能有一个")]
    Output,
    #[error("图存在循环，无法确定性生成 shader")]
    Cycle,
    #[error("节点 {node} 的端口不存在：{port}")]
    InvalidPort { node: String, port: String },
    #[error("输入端口重复连线：{node}.{port}")]
    DuplicateInput { node: String, port: String },
    #[error("节点 {node} 缺少输入 {port}")]
    MissingInput { node: String, port: String },
    #[error("图存在重复连线")]
    DuplicateEdge,
    #[error("节点 {0} 尚未实现代码生成，当前图不能编译")]
    UnsupportedNode(String),
    #[error("节点 {0} 的参数必须是有限数值")]
    InvalidParameter(String),
    #[error("端口类型不匹配：{from}.{from_port} ({from_type:?}) -> {to}.{to_port} ({to_type:?})")]
    TypeMismatch {
        from: String,
        from_port: String,
        from_type: ShaderValueType,
        to: String,
        to_port: String,
        to_type: ShaderValueType,
    },
    #[error("可调参数超过 {MAX_PARAM_SLOTS} 个槽位（实测需要 {0} 个）：请合并常量节点或拆分图")]
    TooManyParams(usize),
}

impl ShaderGraph {
    /// 工程内 `.sgraph` → 运行时叠加层 WGSL（`project.toml` 的 `[render] effect_overlay` 入口）。
    ///
    /// 编辑器与 `game` 运行时共用这一条路径：契约字符串只有一份，不会两边漂移。
    pub fn runtime_overlay_source(&self) -> Result<(String, CompiledShader), ShaderGraphError> {
        let compiled = self.compile_wgsl_uniform(1, 3)?;
        let body = &compiled.source;
        let source = format!(
            r#"{body}
struct EffectParams {{
    time: f32,
    width: f32,
    height: f32,
    _pad0: f32,
    tint: vec4<f32>,
}};
@group(0) @binding(0) var<uniform> effect: EffectParams;
struct EffectOut {{
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}};
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> EffectOut {{
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0)
    );
    var output: EffectOut;
    let corner = positions[index];
    output.position = vec4<f32>(corner, 0.0, 1.0);
    output.uv = vec2<f32>((corner.x + 1.0) * 0.5, (1.0 - corner.y) * 0.5);
    return output;
}}
@fragment
fn fs_main(input: EffectOut) -> @location(0) vec4<f32> {{
    return shader_graph_color(input.uv, effect.time, vec4<f32>(1.0));
}}
"#
        );
        Ok((source, compiled))
    }

    /// 从工程内的 `.sgraph` 文件加载运行时叠加层（路径必须是工程内相对路径）。
    pub fn load_runtime_overlay(
        root: &std::path::Path,
        relative: &str,
    ) -> Result<(String, CompiledShader), String> {
        if relative.is_empty()
            || relative.trim() != relative
            || std::path::Path::new(relative)
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(format!("特效路径必须是工程内相对路径：{relative}"));
        }
        let path = root.join(relative);
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
        Self::from_json(&text)
            .and_then(|graph| graph.runtime_overlay_source())
            .map_err(|error| format!("{relative}: {error}"))
    }

    pub fn from_json(text: &str) -> Result<Self, ShaderGraphError> {
        let graph: Self = serde_json::from_str(text)
            .map_err(|_| ShaderGraphError::NodeId("JSON 格式错误".into()))?;
        graph.validate()?;
        Ok(graph)
    }

    pub fn to_json(&self) -> String {
        format!(
            "{}\n",
            serde_json::to_string_pretty(self).expect("shader graph serializes")
        )
    }

    pub fn validate(&self) -> Result<(), ShaderGraphError> {
        if self.version != SHADER_GRAPH_VERSION {
            return Err(ShaderGraphError::Version {
                found: self.version,
                expected: SHADER_GRAPH_VERSION,
            });
        }
        let mut ids = BTreeSet::new();
        for n in &self.nodes {
            if n.params.values().any(|value| !value.is_finite()) {
                return Err(ShaderGraphError::InvalidParameter(n.id.clone()));
            }
            if n.id.is_empty()
                || !n
                    .id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                || !ids.insert(n.id.clone())
            {
                return Err(ShaderGraphError::NodeId(n.id.clone()));
            }
        }
        let outputs: Vec<_> = self
            .nodes
            .iter()
            .filter(|n| n.kind == ShaderNodeKind::Output)
            .collect();
        if outputs.len() != 1 {
            return Err(ShaderGraphError::Output);
        }
        let by_id: BTreeMap<_, _> = self.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        let mut inputs = BTreeSet::new();
        let mut edges = BTreeSet::new();
        for e in &self.edges {
            let from = by_id
                .get(e.from.node.as_str())
                .ok_or_else(|| ShaderGraphError::MissingNode(e.from.node.clone()))?;
            let to = by_id
                .get(e.to.node.as_str())
                .ok_or_else(|| ShaderGraphError::MissingNode(e.to.node.clone()))?;
            if !valid_port(from.kind, &e.from.port, false) {
                return Err(ShaderGraphError::InvalidPort {
                    node: e.from.node.clone(),
                    port: e.from.port.clone(),
                });
            }
            if !valid_port(to.kind, &e.to.port, true) {
                return Err(ShaderGraphError::InvalidPort {
                    node: e.to.node.clone(),
                    port: e.to.port.clone(),
                });
            }
            let from_type =
                port_type(from.kind, &e.from.port, false).expect("validated output port");
            let to_type = port_type(to.kind, &e.to.port, true).expect("validated input port");
            if from_type != to_type
                && !(from_type == ShaderValueType::Scalar && to_type == ShaderValueType::Vec4)
            {
                return Err(ShaderGraphError::TypeMismatch {
                    from: e.from.node.clone(),
                    from_port: e.from.port.clone(),
                    from_type,
                    to: e.to.node.clone(),
                    to_port: e.to.port.clone(),
                    to_type,
                });
            }
            if !inputs.insert((e.to.node.clone(), e.to.port.clone())) {
                return Err(ShaderGraphError::DuplicateInput {
                    node: e.to.node.clone(),
                    port: e.to.port.clone(),
                });
            }
            if !edges.insert((
                e.from.node.clone(),
                e.from.port.clone(),
                e.to.node.clone(),
                e.to.port.clone(),
            )) {
                return Err(ShaderGraphError::DuplicateEdge);
            }
        }
        // A directed dependency cycle makes deterministic code generation impossible.
        let mut state = BTreeMap::new();
        for node in &self.nodes {
            if has_cycle(node.id.as_str(), self, &mut state) {
                return Err(ShaderGraphError::Cycle);
            }
        }
        Ok(())
    }

    /// 生成可直接放入全屏/精灵 fragment shader 的 WGSL。输入统一为 UV、时间和顶点色，
    /// 因而模板可以在 2D Sprite、UI 和后处理预览中复用。
    pub fn compile_wgsl(&self) -> Result<String, ShaderGraphError> {
        self.compile_wgsl_with_binding_base(0)
    }

    /// 参数**写死成字面量**的编译结果（导出/审阅用）。
    ///
    /// 参数 uniform 化之后，运行时与预览都走 [`Self::compile_wgsl_uniform`]；这个入口保留给
    /// `.wgsl` 导出——导出的文件应该是自包含的，不依赖宿主的参数 buffer。
    pub fn compile_wgsl_with_binding_base(&self, base: u32) -> Result<String, ShaderGraphError> {
        Ok(self.compile(base, None)?.source)
    }

    /// 参数**从 uniform 读**的编译结果：改参数只需要重写 uniform，不必重编管线。
    ///
    /// 纹理/采样器绑在 `texture_base` / `texture_base + 1`，参数表绑在 `params_binding`。
    /// 预览用 `(0, 3)`（0/1 纹理、2 时间、3 参数），运行时叠加用 `(1, 3)`
    /// （0 叠加层 uniform、1/2 纹理、3 参数）。
    pub fn compile_wgsl_uniform(
        &self,
        texture_base: u32,
        params_binding: u32,
    ) -> Result<CompiledShader, ShaderGraphError> {
        self.compile(texture_base, Some(params_binding))
    }

    /// 按编译结果里的槽位表，从当前图读出每个 `vec4` 槽的实时值。
    ///
    /// 参数名或节点被删掉时对应分量为 `0.0`（槽位本身仍保留，buffer 大小不变）。
    #[must_use]
    pub fn param_values(&self, compiled: &CompiledShader) -> Vec<[f32; 4]> {
        let mut slots = vec![[0.0_f32; 4]; compiled.slots.len()];
        for param in &compiled.params {
            let Some(node) = self.nodes.iter().find(|node| node.id == param.node) else {
                continue;
            };
            let default = match param.param {
                "frequency" => 1.0,
                "power" => 3.0,
                "speed" => 0.6,
                "width" => 0.08,
                "strength" => 0.02,
                "edge_width" => 0.002,
                "angle0" => 0.0,
                "spin_speed" => 1.5,
                "fog_power" => 2.0,
                "vertical" => 0.0,
                "arc_width" => 0.35,
                "radius" => 0.35,
                "thickness" => 0.15,
                "spin" => 1.2,
                "amount" => 0.05,
                "wave_speed" => 1.2,
                "direction" => 0.0,
                _ => 0.0,
            };
            // 颜色常量在节点里存成 r/g/b 三个键，编译后合并成一个 vec4 槽的 x/y/z；
            // 按分量读回对应键，否则默认色永远是白色（改了颜色不重编就看不到）。
            let key = if param.param == "color" {
                ["r", "g", "b", "a"][param.component.min(3)]
            } else {
                param.param
            };
            let value = node.params.get(key).copied().unwrap_or(default);
            if let Some(slot) = slots.get_mut(param.slot) {
                slot[param.component] = value;
            }
        }
        slots
    }

    fn compile(
        &self,
        base: u32,
        params_binding: Option<u32>,
    ) -> Result<CompiledShader, ShaderGraphError> {
        self.validate()?;
        let output = self
            .nodes
            .iter()
            .find(|n| n.kind == ShaderNodeKind::Output)
            .expect("validated output");
        let mut params: Vec<ShaderParam> = Vec::new();
        let mut slot_count = 0usize;
        let mut next_slot =
            |params: &mut Vec<ShaderParam>, node: &str, key: &'static str, components: usize| {
                // 一个标量占一个槽的 x；颜色占 xyz。槽位按「先到先得」分配，拓扑序遍历保证确定性。
                let slot = slot_count;
                slot_count += 1;
                for component in 0..components {
                    params.push(ShaderParam {
                        node: node.to_owned(),
                        param: key,
                        slot,
                        component,
                    });
                }
                slot
            };
        // 第一遍：分配槽位（生成表达式时必须已经知道自己的槽号）。
        for n in topo_order(self) {
            match n.kind {
                ShaderNodeKind::Sine => {
                    next_slot(&mut params, &n.id, "frequency", 1);
                }
                ShaderNodeKind::Fresnel => {
                    next_slot(&mut params, &n.id, "power", 1);
                }
                ShaderNodeKind::Constant => {
                    next_slot(&mut params, &n.id, "color", 3);
                }
                ShaderNodeKind::Ring => {
                    next_slot(&mut params, &n.id, "speed", 1);
                }
                ShaderNodeKind::Beam => {
                    next_slot(&mut params, &n.id, "width", 1);
                    next_slot(&mut params, &n.id, "vertical", 1);
                }
                ShaderNodeKind::Sector => {
                    next_slot(&mut params, &n.id, "arc_width", 1);
                    next_slot(&mut params, &n.id, "radius", 1);
                    next_slot(&mut params, &n.id, "thickness", 1);
                    next_slot(&mut params, &n.id, "spin", 1);
                }
                ShaderNodeKind::VerticalStretch => {
                    next_slot(&mut params, &n.id, "amount", 1);
                    next_slot(&mut params, &n.id, "wave_speed", 1);
                }
                ShaderNodeKind::Gradient => {
                    next_slot(&mut params, &n.id, "direction", 1);
                }
                ShaderNodeKind::DistortUv => {
                    next_slot(&mut params, &n.id, "strength", 1);
                }
                ShaderNodeKind::RotateUv => {
                    next_slot(&mut params, &n.id, "angle0", 1);
                    next_slot(&mut params, &n.id, "spin_speed", 1);
                }
                ShaderNodeKind::DepthEdge => {
                    next_slot(&mut params, &n.id, "edge_width", 1);
                }
                ShaderNodeKind::DepthFog => {
                    next_slot(&mut params, &n.id, "fog_power", 1);
                }
                _ => {}
            }
        }
        if slot_count > MAX_PARAM_SLOTS {
            return Err(ShaderGraphError::TooManyParams(slot_count));
        }
        let param_expr = |node: &str, key: &str, component: usize| -> String {
            match params_binding {
                None => "0.0".into(),
                Some(_) => {
                    let slot = params
                        .iter()
                        .find(|param| {
                            param.node == node && param.param == key && param.component == component
                        })
                        .map(|param| param.slot)
                        .unwrap_or(0);
                    format!("graph_params.p[{slot}].{}", ["x", "y", "z", "w"][component])
                }
            }
        };
        let mut lines = vec![
            "// Generated by RustLua Shader Graph; do not edit.".into(),
            format!("@group(0) @binding({base}) var shader_graph_texture: texture_2d<f32>;"),
            format!(
                "@group(0) @binding({}) var shader_graph_sampler: sampler;",
                base + 1
            ),
            // M91：场景颜色固定绑 4/5（预览与运行时叠加层同一编号）。
        ];
        // M92：只有真的采样场景色/深度的图才声明对应绑定——运行时可据此判断
        // 需不需要走"离屏渲染 + 回拷"，纯程序化模板直接叠最终目标。
        if self
            .nodes
            .iter()
            .any(|node| node.kind == ShaderNodeKind::SceneColor)
        {
            lines.push(
                "@group(0) @binding(4) var shader_graph_scene_texture: texture_2d<f32>;".into(),
            );
            lines.push("@group(0) @binding(5) var shader_graph_scene_sampler: sampler;".into());
        }
        if self.nodes.iter().any(|node| {
            matches!(
                node.kind,
                ShaderNodeKind::DepthEdge | ShaderNodeKind::DepthFog
            )
        }) {
            lines.push(
                "@group(0) @binding(6) var shader_graph_depth_texture: texture_depth_2d;".into(),
            );
            lines.push("@group(0) @binding(7) var shader_graph_depth_sampler: sampler;".into());
        }
        if let Some(binding) = params_binding {
            lines.push(format!(
                "struct GraphParams {{ p: array<vec4<f32>, {MAX_PARAM_SLOTS}>, }};"
            ));
            lines.push(format!(
                "@group(0) @binding({binding}) var<uniform> graph_params: GraphParams;"
            ));
        }
        lines.push(
            "fn shader_graph_color(uv: vec2<f32>, time: f32, vertex_color: vec4<f32>) -> vec4<f32> {"
                .into(),
        );
        for n in topo_order(self) {
            let v = format!("sg_{}", n.id.replace('-', "_"));
            match n.kind {
                ShaderNodeKind::Uv => lines.push(format!("    let {v} = uv;")),
                ShaderNodeKind::Time => lines.push(format!("    let {v} = time;")),
                ShaderNodeKind::VertexColor => lines.push(format!("    let {v} = vertex_color;")),
                ShaderNodeKind::Constant => match params_binding {
                    None => lines.push(format!(
                        "    let {v} = vec4<f32>({:.4}, {:.4}, {:.4}, 1.0);",
                        n.params.get("r").copied().unwrap_or(1.0),
                        n.params.get("g").copied().unwrap_or(1.0),
                        n.params.get("b").copied().unwrap_or(1.0)
                    )),
                    Some(_) => lines.push(format!(
                        "    let {v} = vec4<f32>({}, {}, {}, 1.0);",
                        param_expr(&n.id, "color", 0),
                        param_expr(&n.id, "color", 1),
                        param_expr(&n.id, "color", 2),
                    )),
                },
                ShaderNodeKind::Sine => {
                    // `time` 是波形推进，`uv` 是空间相位：两者都接上就是"沿 UV 推进的扫描/波纹"，
                    // 这正是扫描线、水波、条纹类模板的差别所在；只接 time 则是整体脉动。
                    let time_source = input_expr(self, n, "time").unwrap_or_else(|| "time".into());
                    let frequency = match params_binding {
                        None => {
                            let value = n.params.get("frequency").copied().unwrap_or(1.0);
                            format!("{value:.4}")
                        }
                        Some(_) => param_expr(&n.id, "frequency", 0),
                    };
                    match input_expr(self, n, "uv") {
                        Some(uv) => lines.push(format!(
                            "    let {v} = 0.5 + 0.5 * sin({frequency} * ({time_source}) + dot(({uv}), vec2<f32>(6.2831853, 3.1415927)));"
                        )),
                        None => lines.push(format!(
                            "    let {v} = 0.5 + 0.5 * sin({frequency} * ({time_source}));"
                        )),
                    }
                }
                ShaderNodeKind::Fresnel => {
                    let power = match params_binding {
                        None => {
                            let value = n.params.get("power").copied().unwrap_or(3.0);
                            format!("{value:.4}")
                        }
                        Some(_) => param_expr(&n.id, "power", 0),
                    };
                    match (
                        input_expr(self, n, "normal"),
                        input_expr(self, n, "view"),
                    ) {
                        // 有视角向量时用真实 Fresnel；2D/Sprite 场景没有法线，退化成
                        // 「离中心越远越亮」的径向边缘，否则 dot(同一向量) 恒为 0、边缘永远黑。
                        (Some(normal), Some(view)) => lines.push(format!(
                            "    let {v} = vec4<f32>(pow(1.0 - abs(dot(normalize({normal}), normalize({view}))), {power}));"
                        )),
                        _ => lines.push(format!(
                            "    let {v} = vec4<f32>(pow(clamp(length(uv - vec2<f32>(0.5, 0.5)) * 2.0, 0.0, 1.0), {power}));"
                        )),
                    }
                }
                ShaderNodeKind::Ring => {
                    let speed = match params_binding {
                        None => {
                            let value = n.params.get("speed").copied().unwrap_or(0.6);
                            format!("{value:.4}")
                        }
                        Some(_) => param_expr(&n.id, "speed", 0),
                    };
                    lines.push(format!(
                        "    let {v} = vec4<f32>(pow(clamp(1.0 - abs(length(uv - vec2<f32>(0.5, 0.5)) * 2.0 - fract(time * {speed})), 0.0, 1.0), 8.0));"
                    ));
                }
                ShaderNodeKind::RotateUv => {
                    let uv = input_expr(self, n, "uv").unwrap_or_else(|| "uv".into());
                    let angle0 = match params_binding {
                        None => {
                            let value = n.params.get("angle0").copied().unwrap_or(0.0);
                            format!("{value:.4}")
                        }
                        Some(_) => param_expr(&n.id, "angle0", 0),
                    };
                    let speed = match params_binding {
                        None => {
                            let value = n.params.get("spin_speed").copied().unwrap_or(1.5);
                            format!("{value:.4}")
                        }
                        Some(_) => param_expr(&n.id, "spin_speed", 0),
                    };
                    // WGSL 没有块表达式，展开成多条 let；变量名带节点前缀防撞名。
                    lines.push(format!("    let {v}_c = ({uv}) - vec2<f32>(0.5, 0.5);"));
                    lines.push(format!(
                        "    let {v}_angle = ({angle0}) + time * ({speed});"
                    ));
                    lines.push(format!("    let {v}_s = sin({v}_angle);"));
                    lines.push(format!("    let {v}_co = cos({v}_angle);"));
                    lines.push(format!("    let {v} = vec2<f32>(({v}_c).x * ({v}_co) - ({v}_c).y * ({v}_s), ({v}_c).x * ({v}_s) + ({v}_c).y * ({v}_co)) + vec2<f32>(0.5, 0.5);"));
                }
                ShaderNodeKind::Polar => {
                    let uv = input_expr(self, n, "uv").unwrap_or_else(|| "uv".into());
                    lines.push(format!("    let {v}_c = ({uv}) - vec2<f32>(0.5, 0.5);"));
                    lines.push(format!("    let {v} = vec2<f32>(atan2(({v}_c).y, ({v}_c).x) / 6.2831853 + 0.5, clamp(length({v}_c) * 2.0, 0.0, 1.0));"));
                }
                ShaderNodeKind::Sector => {
                    let uv = input_expr(self, n, "uv").unwrap_or_else(|| "uv".into());
                    let scalar = |key: &'static str, fallback: f32| match params_binding {
                        None => {
                            let value = n.params.get(key).copied().unwrap_or(fallback);
                            format!("{value:.4}")
                        }
                        Some(_) => param_expr(&n.id, key, 0),
                    };
                    let arc_width = scalar("arc_width", 0.35);
                    let radius = scalar("radius", 0.35);
                    let thickness = scalar("thickness", 0.15);
                    let spin = scalar("spin", 1.2);
                    lines.push(format!("    let {v}_c = ({uv}) - vec2<f32>(0.5, 0.5);"));
                    lines.push(format!("    let {v}_r = length({v}_c);"));
                    lines.push(format!(
                        "    let {v}_a = atan2(({v}_c).y, ({v}_c).x) / 6.2831853 + 0.5;"
                    ));
                    lines.push(format!(
                        "    let {v}_d = abs(fract({v}_a - fract(time * ({spin})) + 0.5) - 0.5);"
                    ));
                    lines.push(format!(
                        "    let {v}_arc = smoothstep(({arc_width}), 0.0, {v}_d);"
                    ));
                    lines.push(format!("    let {v}_band = smoothstep(({thickness}), 0.0, abs({v}_r - ({radius})));"));
                    lines.push(format!("    let {v} = {v}_arc * {v}_band;"));
                }
                ShaderNodeKind::VerticalStretch => {
                    let uv = input_expr(self, n, "uv").unwrap_or_else(|| "uv".into());
                    let scalar = |key: &'static str, fallback: f32| match params_binding {
                        None => {
                            let value = n.params.get(key).copied().unwrap_or(fallback);
                            format!("{value:.4}")
                        }
                        Some(_) => param_expr(&n.id, key, 0),
                    };
                    let amount = scalar("amount", 0.05);
                    let wave_speed = scalar("wave_speed", 1.2);
                    lines.push(format!("    let {v}_c = ({uv}) - vec2<f32>(0.5, 0.5);"));
                    lines.push(format!(
                        "    let {v} = vec2<f32>(({uv}).x + sin(({uv}).y * 18.0 + time * ({wave_speed})) * ({amount}), ({uv}).y);"
                    ));
                }
                ShaderNodeKind::Gradient => {
                    let direction = match params_binding {
                        None => {
                            let value = n.params.get("direction").copied().unwrap_or(0.0);
                            format!("{value:.4}")
                        }
                        Some(_) => param_expr(&n.id, "direction", 0),
                    };
                    lines.push(format!(
                        "    let {v}_dir = vec2<f32>(cos({direction}), sin({direction}));"
                    ));
                    lines.push(format!(
                        "    let {v} = vec4<f32>(clamp(dot(uv - vec2<f32>(0.5, 0.5), {v}_dir) + 0.5, 0.0, 1.0));"
                    ));
                }
                ShaderNodeKind::SceneColor => {
                    let uv = input_expr(self, n, "uv").unwrap_or_else(|| "uv".into());
                    lines.push(format!("    let {v} = textureSample(shader_graph_scene_texture, shader_graph_scene_sampler, {uv});"));
                }
                ShaderNodeKind::DistortUv => {
                    let uv = input_expr(self, n, "uv").unwrap_or_else(|| "uv".into());
                    let strength = match params_binding {
                        None => {
                            let value = n.params.get("strength").copied().unwrap_or(0.02);
                            format!("{value:.4}")
                        }
                        Some(_) => param_expr(&n.id, "strength", 0),
                    };
                    lines.push(format!("    let {v} = ({uv}) + vec2<f32>(sin(time * 3.1 + uv.y * 20.0), cos(time * 2.7 + uv.x * 20.0)) * {strength};"));
                }
                ShaderNodeKind::DepthEdge => {
                    let uv = input_expr(self, n, "uv").unwrap_or_else(|| "uv".into());
                    let width = match params_binding {
                        None => {
                            let value = n.params.get("edge_width").copied().unwrap_or(0.002);
                            format!("{value:.5}")
                        }
                        Some(_) => param_expr(&n.id, "edge_width", 0),
                    };
                    lines.push(format!("    let {v} = (abs(textureSample(shader_graph_depth_texture, shader_graph_depth_sampler, ({uv}) + vec2<f32>({width}, 0.0)) - textureSample(shader_graph_depth_texture, shader_graph_depth_sampler, ({uv}) - vec2<f32>({width}, 0.0))) + abs(textureSample(shader_graph_depth_texture, shader_graph_depth_sampler, ({uv}) + vec2<f32>(0.0, {width})) - textureSample(shader_graph_depth_texture, shader_graph_depth_sampler, ({uv}) - vec2<f32>(0.0, {width})))) * 200.0;"));
                }
                ShaderNodeKind::DepthFog => {
                    let uv = input_expr(self, n, "uv").unwrap_or_else(|| "uv".into());
                    let power = match params_binding {
                        None => {
                            let value = n.params.get("fog_power").copied().unwrap_or(2.0);
                            format!("{value:.4}")
                        }
                        Some(_) => param_expr(&n.id, "fog_power", 0),
                    };
                    lines.push(format!("    let {v} = pow(clamp(textureSample(shader_graph_depth_texture, shader_graph_depth_sampler, {uv}), 0.0, 1.0), {power});"));
                }
                ShaderNodeKind::Beam => {
                    let width = match params_binding {
                        None => {
                            let value = n.params.get("width").copied().unwrap_or(0.08);
                            format!("{value:.4}")
                        }
                        Some(_) => param_expr(&n.id, "width", 0),
                    };
                    let vertical = match params_binding {
                        None => {
                            let value = n.params.get("vertical").copied().unwrap_or(0.0);
                            format!("{value:.4}")
                        }
                        Some(_) => param_expr(&n.id, "vertical", 0),
                    };
                    lines.push(format!(
                        "    let {v}_axis = select(uv.y - 0.5, uv.x - 0.5, ({vertical}) > 0.5);"
                    ));
                    lines.push(format!(
                        "    let {v} = vec4<f32>(pow(clamp(1.0 - abs({v}_axis) / max({width}, 0.001), 0.0, 1.0), 6.0));"
                    ));
                }
                ShaderNodeKind::Add | ShaderNodeKind::Multiply => {
                    let a = input_expr(self, n, "a").unwrap_or_else(|| "vec4<f32>(0.0)".into());
                    let b = input_expr(self, n, "b").unwrap_or_else(|| "vec4<f32>(0.0)".into());
                    let op = if n.kind == ShaderNodeKind::Add {
                        "+"
                    } else {
                        "*"
                    };
                    lines.push(format!("    let {v} = ({a}) {op} ({b});"));
                }
                ShaderNodeKind::Texture => {
                    let uv = input_expr(self, n, "uv").unwrap_or_else(|| "uv".into());
                    lines.push(format!("    let {v} = textureSample(shader_graph_texture, shader_graph_sampler, {uv});"));
                }
                ShaderNodeKind::Output => {}
            }
        }
        let tint = input_expr(self, output, "tint")
            .or_else(|| input_expr(self, output, "color"))
            .unwrap_or_else(|| "vec4<f32>(1.0)".into());
        let alpha = input_expr(self, output, "alpha").unwrap_or_else(|| "1.0".into());
        lines.push(format!("    return vec4<f32>(({tint}).rgb * vertex_color.rgb, clamp(({alpha}) * vertex_color.a, 0.0, 1.0));"));
        lines.push("}".into());
        let source = lines.join("\n") + "\n";
        // 第二遍：用默认值填一份槽位值，让调用方"不设参数也能画出正确画面"。
        let compiled = CompiledShader {
            source,
            params,
            slots: vec![[0.0; 4]; MAX_PARAM_SLOTS],
        };
        let slots = self.param_values(&compiled);
        let mut compiled = compiled;
        compiled.slots = slots;
        Ok(compiled)
    }
}

fn valid_port(kind: ShaderNodeKind, port: &str, input: bool) -> bool {
    port_type(kind, port, input).is_some()
}

fn port_type(kind: ShaderNodeKind, port: &str, input: bool) -> Option<ShaderValueType> {
    if input {
        return match kind {
            ShaderNodeKind::Sine => match port {
                "uv" => Some(ShaderValueType::Vec2),
                "time" => Some(ShaderValueType::Scalar),
                _ => None,
            },
            ShaderNodeKind::Fresnel => match port {
                "normal" | "view" => Some(ShaderValueType::Vec3),
                _ => None,
            },
            ShaderNodeKind::Add | ShaderNodeKind::Multiply => match port {
                "a" | "b" => Some(ShaderValueType::Vec4),
                _ => None,
            },
            ShaderNodeKind::Output => match port {
                "color" | "tint" => Some(ShaderValueType::Vec4),
                "alpha" => Some(ShaderValueType::Scalar),
                _ => None,
            },
            ShaderNodeKind::Texture => match port {
                "uv" => Some(ShaderValueType::Vec2),
                _ => None,
            },
            ShaderNodeKind::SceneColor
            | ShaderNodeKind::DistortUv
            | ShaderNodeKind::DepthEdge
            | ShaderNodeKind::DepthFog
            | ShaderNodeKind::RotateUv
            | ShaderNodeKind::Polar
            | ShaderNodeKind::Sector
            | ShaderNodeKind::VerticalStretch => match port {
                "uv" => Some(ShaderValueType::Vec2),
                _ => None,
            },
            _ => None,
        };
    }
    match kind {
        ShaderNodeKind::Uv => (port == "value").then_some(ShaderValueType::Vec2),
        ShaderNodeKind::DistortUv => (port == "value").then_some(ShaderValueType::Vec2),
        ShaderNodeKind::DepthEdge | ShaderNodeKind::DepthFog => {
            (port == "value").then_some(ShaderValueType::Scalar)
        }
        ShaderNodeKind::RotateUv | ShaderNodeKind::Polar => {
            (port == "value").then_some(ShaderValueType::Vec2)
        }
        ShaderNodeKind::Sector => (port == "value").then_some(ShaderValueType::Scalar),
        ShaderNodeKind::VerticalStretch => (port == "value").then_some(ShaderValueType::Vec2),
        ShaderNodeKind::Gradient => (port == "value").then_some(ShaderValueType::Vec4),
        ShaderNodeKind::Time | ShaderNodeKind::Sine => {
            (port == "value").then_some(ShaderValueType::Scalar)
        }
        ShaderNodeKind::Fresnel
        | ShaderNodeKind::Ring
        | ShaderNodeKind::Beam
        | ShaderNodeKind::SceneColor => (port == "value").then_some(ShaderValueType::Vec4),
        ShaderNodeKind::VertexColor
        | ShaderNodeKind::Texture
        | ShaderNodeKind::Constant
        | ShaderNodeKind::Add
        | ShaderNodeKind::Multiply => (port == "value").then_some(ShaderValueType::Vec4),
        ShaderNodeKind::Output => None,
    }
}

fn has_cycle(id: &str, graph: &ShaderGraph, state: &mut BTreeMap<String, u8>) -> bool {
    match state.get(id).copied().unwrap_or(0) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    state.insert(id.to_string(), 1);
    for edge in graph.edges.iter().filter(|e| e.to.node == id) {
        if has_cycle(&edge.from.node, graph, state) {
            return true;
        }
    }
    state.insert(id.to_string(), 2);
    false
}

fn topo_order(graph: &ShaderGraph) -> Vec<&ShaderNode> {
    fn visit<'a>(
        id: &str,
        graph: &'a ShaderGraph,
        seen: &mut BTreeSet<String>,
        out: &mut Vec<&'a ShaderNode>,
    ) {
        if !seen.insert(id.to_string()) {
            return;
        }
        for edge in graph.edges.iter().filter(|e| e.to.node == id) {
            visit(&edge.from.node, graph, seen, out);
        }
        if let Some(node) = graph.nodes.iter().find(|n| n.id == id) {
            out.push(node);
        }
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(graph.nodes.len());
    for node in &graph.nodes {
        visit(&node.id, graph, &mut seen, &mut out);
    }
    out
}

fn input_expr(graph: &ShaderGraph, node: &ShaderNode, port: &str) -> Option<String> {
    let edge = graph
        .edges
        .iter()
        .find(|e| e.to.node == node.id && e.to.port == port)?;
    Some(format!("sg_{}", edge.from.node.replace('-', "_")))
}
