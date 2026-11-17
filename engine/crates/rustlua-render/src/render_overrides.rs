//! 运行期渲染调参槽（`render.*` 绑定的落点，P1 阶段 4e）。
//!
//! ## 为什么这些字段搬出 `ScriptState`
//!
//! 它们原本是 `ScriptState` 上 21 个散落的 `render_*: Option<...>` 字段。三处问题：
//!
//! 1. **域归属错了**：雾、云、水、曝光、后处理、剖切全都是渲染管线的参数，类型也全是
//!    原始标量，没有任何一条理由要求它们挤在脚本 VM 的状态结构里；
//! 2. **`ScriptState` 因此从 31 字段涨到 51 字段**（2026-10-21 收口后到 2026-11-17
//!    的增量里有 21 个是这一簇），把"按 owning crate 拆分"的成果又吃了回去；
//! 3. **宿主侧要 18 个一字排开的转发访问器**才能读到它们，每个都是
//!    `pub fn render_xxx(&self) -> Option<T> { self.state.borrow().render_xxx }`。
//!
//! 收成能力单元之后，写入方（`rustlua_scripting::api::render_tuning`）与读取方（宿主的
//! 渲染接线）拿的是**同一个 `Handle`**，与 `RenderableState` / `UiState` 同一套模式。
//!
//! ## 语义约定
//!
//! 每个字段都是 `Option`：`None` = **脚本没碰过**，宿主按场景/渲染器的默认值走。
//! 这个区分是有意的——`Some(0.0)`（真的想关掉雾）与 `None`（没表态）在渲染器里是两回事。
//!
//! 纹理/管线级开关**不在**这里：它们走 `RenderSettings` 与编辑器，避免脚本把渲染器状态
//! 改到无法回滚（见 `render_tuning` 模块头的原始约束，本次搬家没有放宽它）。

use rustlua_lua_api::Handle;

/// 脚本可改的渲染参数。字段含义见模块文档。
#[derive(Debug, Default)]
pub struct RenderOverrides {
    /// 雾密度（米制，0 = 无雾）。
    pub fog_density: Option<f32>,
    /// 雾颜色（线性 RGB）。
    pub fog_color: Option<[f32; 3]>,
    /// 云层覆盖率 0..=1。
    pub cloud_coverage: Option<f32>,
    /// 云层流动速度。
    pub cloud_speed: Option<f32>,
    /// 云层基准高度（米）。
    pub cloud_base: Option<f32>,
    /// 云影强度 0..=1。
    pub cloud_shadow_strength: Option<f32>,
    /// 屏幕闪白强度 0..=1（受击/爆炸反馈）。
    pub flash: Option<f32>,
    /// 水面染色（RGBA）。
    pub water_tint: Option<[f32; 4]>,
    /// 画质档位（0..=N，宿主映射到具体管线开关）。
    pub quality: Option<u8>,
    /// 地表覆盖（草/雪等两参数混合）。
    pub surface_cover: Option<[f32; 2]>,
    /// 曝光（EV 补偿）。
    pub exposure: Option<f32>,
    /// 水面涟漪参数（两参数）。
    pub ripple: Option<[f32; 2]>,
    /// 后处理（暗角, 颗粒）。
    pub post_fx: Option<[f32; 2]>,
    /// Shader Graph 叠加层的命名参数向量（按槽号写入 uniform）。
    pub effect_params: Option<Vec<[f32; 4]>>,
    /// 叠加层时间（秒）；`None` = 宿主按自己的时钟推进。
    pub effect_time: Option<f32>,
    /// 命名参数表（名字 → 槽号 + 分量数），宿主加载 `.sgraph` 后推来。
    pub effect_param_names: Option<Vec<(String, usize, usize)>>,
    /// 剖切口的实体封口深度（米，0 = 关）。
    pub clip_cap: Option<f32>,
    /// 封口是否也吃到单面网格；`None` = 保持宿主启动档。
    pub clip_cap_both_sides: Option<bool>,
    /// 水面折射的 Snell 屏幕空间步进数（0 = 关）。
    pub water_refract_steps: Option<u32>,
    /// 阴影投射距离上限（米）。
    pub shadow_caster_distance: Option<u32>,
    /// 上一帧的渲染统计读数（`[draws, triangles, culled, ...]`）。
    pub stats: Option<[u64; 4]>,
}

impl RenderOverrides {
    #[must_use]
    pub fn handle() -> Handle<Self> {
        Handle::new(Self::default(), "render_overrides")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认全 `None`：脚本没表态时宿主必须走渲染器默认值。
    #[test]
    fn defaults_are_all_unset() {
        let overrides = RenderOverrides::default();
        assert!(overrides.fog_density.is_none());
        assert!(overrides.quality.is_none());
        assert!(overrides.stats.is_none());
    }

    /// 两个句柄是**两份**状态：同进程多个运行时（编辑器预览 + AI 宿主）不得互相看到对方
    /// 的调参。这条与 `UiState` 的同类契约一致。
    #[test]
    fn handles_are_independent() {
        let first = RenderOverrides::handle();
        let second = RenderOverrides::handle();

        first.with_mut(|overrides| overrides.fog_density = Some(0.01));
        assert_eq!(first.with(|overrides| overrides.fog_density), Some(0.01));
        assert_eq!(second.with(|overrides| overrides.fog_density), None);
    }

    /// `Some(0.0)`（真的关掉）与 `None`（没表态）必须能区分——渲染器对这两者的处理不同。
    #[test]
    fn explicit_zero_differs_from_unset() {
        let overrides = RenderOverrides::handle();
        assert_eq!(overrides.with(|o| o.fog_density), None);
        overrides.with_mut(|o| o.fog_density = Some(0.0));
        assert_eq!(overrides.with(|o| o.fog_density), Some(0.0));
    }
}
